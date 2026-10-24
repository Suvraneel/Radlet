//! Output naming and atomic, never-overwrite publishing.
//!
//! Outputs are written to a hidden temporary file in the destination folder
//! and then hard-linked into place under a free name. Linking fails if the
//! name is taken, so two jobs can't claim the same name and nothing the user
//! already has is ever replaced. On volumes without hard links we fall back
//! to reserving the name with an exclusive create, then renaming over it.

use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::{NamedTempFile, TempPath};

use crate::error::{Error, Result};

/// Where finished files go.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum OutputLocation {
    /// Next to each original (default).
    #[default]
    BesideSource,
    /// Into one folder.
    Directory(PathBuf),
}

impl OutputLocation {
    pub fn dir_for(&self, input: &Path) -> PathBuf {
        match self {
            OutputLocation::BesideSource => input
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            OutputLocation::Directory(d) => d.clone(),
        }
    }
}

/// Prefix of every temporary file Radlet creates in a destination folder.
pub const TEMP_PREFIX: &str = ".radlet-";

/// The name an output would ideally get: the input's stem, an optional
/// suffix in parentheses, and the new extension.
pub fn desired_path(input: &Path, dir: &Path, ext: &str, suffix: Option<&str>) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    let name = match suffix {
        Some(s) if !s.is_empty() => format!("{stem} ({s}).{ext}"),
        _ => format!("{stem}.{ext}"),
    };
    dir.join(name)
}

/// `photo.jpg`, `photo 2.jpg`, `photo 3.jpg`, ... (Finder's convention).
pub fn numbered(path: &Path, n: u32) -> PathBuf {
    if n <= 1 {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{stem} {n}.{}", ext.to_string_lossy()),
        None => format!("{stem} {n}"),
    };
    path.with_file_name(name)
}

/// Creates a hidden temporary file in `dir` for an output with extension
/// `ext`. `tag` lets a supervisor find and delete leftovers if a worker is
/// killed mid-write.
pub fn temp_in(dir: &Path, tag: &str, ext: &str) -> Result<NamedTempFile> {
    fs::create_dir_all(dir)?;
    let prefix = format!("{TEMP_PREFIX}{tag}-");
    let suffix = format!(".{ext}");
    Ok(tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(&suffix)
        .tempfile_in(dir)?)
}

/// Moves a finished temporary file to the first free name based on
/// `desired`, without ever replacing an existing file.
pub fn publish(tmp: TempPath, desired: &Path) -> Result<PathBuf> {
    for n in 1..=9_999 {
        let candidate = numbered(desired, n);
        match fs::hard_link(&tmp, &candidate) {
            Ok(()) => {
                // Dropping `tmp` removes the temporary name; the data stays
                // reachable under `candidate`.
                drop(tmp);
                return Ok(candidate);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => return publish_without_links(tmp, desired, n),
        }
    }
    Err(Error::Invalid(format!(
        "no free name for {}",
        desired.display()
    )))
}

fn publish_without_links(tmp: TempPath, desired: &Path, start: u32) -> Result<PathBuf> {
    for n in start..=9_999 {
        let candidate = numbered(desired, n);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(placeholder) => {
                drop(placeholder);
                // Renaming over our own empty placeholder is safe: we just
                // created it exclusively.
                return match tmp.persist(&candidate) {
                    Ok(()) => Ok(candidate),
                    Err(e) => {
                        let _ = fs::remove_file(&candidate);
                        Err(Error::Io(e.error))
                    }
                };
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Err(Error::Invalid(format!(
        "no free name for {}",
        desired.display()
    )))
}

/// Gives an output the same read/write permissions as its source.
/// Temporary files are created owner-only (0600); without this, outputs
/// would be unreadable to other users and apps that expect 0644.
pub fn inherit_permissions(source: &Path, output: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(source) {
            let mode = (meta.permissions().mode() & 0o666) | 0o600;
            let _ = fs::set_permissions(output, fs::Permissions::from_mode(mode));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (source, output);
    }
}

/// Deletes temporary files left in `dir` by a worker that was killed.
pub fn remove_leftovers(dir: &Path, tag: &str) {
    let prefix = format!("{TEMP_PREFIX}{tag}-");
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                let path = entry.path();
                // Only our own temporary folders are removed recursively.
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let _ = fs::remove_dir_all(&path);
                } else {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
}

/// Creates a hidden temporary folder in `dir` for outputs that will be
/// published together (pages of a PDF, split parts).
pub fn temp_dir_in(dir: &Path, tag: &str) -> Result<tempfile::TempDir> {
    fs::create_dir_all(dir)?;
    let prefix = format!("{TEMP_PREFIX}{tag}-");
    Ok(tempfile::Builder::new().prefix(&prefix).tempdir_in(dir)?)
}

/// `name`, `name 2`, `name 3`, ... for folders (no extension handling).
pub fn numbered_dir(path: &Path, n: u32) -> PathBuf {
    if n <= 1 {
        return path.to_path_buf();
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name} {n}"))
}

/// Moves every file in the temporary folder `tmp` into a new folder named
/// like `desired` (or `desired 2`, ...), never merging into an existing one.
/// Returns the folder's path.
pub fn publish_dir(tmp: tempfile::TempDir, desired: &Path) -> Result<PathBuf> {
    let out = publish_dir_contents(tmp.path(), desired)?;
    drop(tmp);
    Ok(out)
}

/// Moves everything inside `src` into a new folder named like `desired`
/// (or `desired 2`, ...), never merging into an existing one.
pub fn publish_dir_contents(src: &Path, desired: &Path) -> Result<PathBuf> {
    for n in 1..=9_999 {
        let candidate = numbered_dir(desired, n);
        match fs::create_dir(&candidate) {
            Ok(()) => {
                for entry in fs::read_dir(src)? {
                    let entry = entry?;
                    fs::rename(entry.path(), candidate.join(entry.file_name()))?;
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Err(Error::Invalid(format!(
        "no free name for {}",
        desired.display()
    )))
}

/// Publishes a group of finished files that live in `tmp`: up to four go
/// next to each other in `dir` under their desired names; more go into one
/// folder named `folder`, so a 200-page PDF doesn't flood the folder.
/// Returns the top-level items created.
pub fn publish_group(
    tmp: tempfile::TempDir,
    files: Vec<(PathBuf, String)>,
    dir: &Path,
    folder: &str,
) -> Result<Vec<PathBuf>> {
    const BESIDE_LIMIT: usize = 4;
    if files.len() <= BESIDE_LIMIT {
        let mut out = Vec::with_capacity(files.len());
        for (path, name) in files {
            out.push(publish(TempPath::try_from_path(path)?, &dir.join(name))?);
        }
        drop(tmp);
        return Ok(out);
    }
    // Rename into final names inside the temporary folder, then move the
    // whole set at once.
    // Two outputs may want the same name (or names this disk treats as
    // the same, by case or accents): later ones are numbered, never
    // written over.
    for (path, name) in &files {
        let wanted = tmp.path().join(name);
        let mut target = wanted.clone();
        let mut n = 2;
        while fs::symlink_metadata(&target).is_ok() {
            target = numbered(&wanted, n);
            n += 1;
        }
        fs::rename(path, &target)?;
    }
    Ok(vec![publish_dir(tmp, &dir.join(folder))?])
}

/// Total size of a file, or of every file inside a folder.
pub fn size_of(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::read_dir(path)
            .map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn group_names_that_clash_are_numbered() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = temp_dir_in(dir.path(), "t").unwrap();
        let mut files = Vec::new();
        for i in 0..5 {
            let p = tmp.path().join(format!("raw{i}"));
            fs::write(&p, i.to_string()).unwrap();
            let name = if i < 2 {
                "same.csv".to_string()
            } else {
                format!("{i}.csv")
            };
            files.push((p, name));
        }
        let out = publish_group(tmp, files, dir.path(), "Set").unwrap();
        assert_eq!(out.len(), 1);
        let mut names: Vec<String> = fs::read_dir(&out[0])
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), 5, "{names:?}");
        assert!(names.contains(&"same.csv".to_string()));
    }

    #[test]
    fn desired_and_numbered_names() {
        let p = desired_path(Path::new("/a/photo.png"), Path::new("/a"), "jpg", None);
        assert_eq!(p, PathBuf::from("/a/photo.jpg"));
        let p = desired_path(
            Path::new("/a/photo.png"),
            Path::new("/out"),
            "png",
            Some("resized"),
        );
        assert_eq!(p, PathBuf::from("/out/photo (resized).png"));
        assert_eq!(
            numbered(Path::new("/a/photo.jpg"), 3),
            PathBuf::from("/a/photo 3.jpg")
        );
        assert_eq!(
            numbered(Path::new("/a/photo.jpg"), 1),
            PathBuf::from("/a/photo.jpg")
        );
    }

    #[test]
    fn publish_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("photo.jpg");
        fs::write(&existing, b"original").unwrap();

        let mut t = temp_in(dir.path(), "t1", "jpg").unwrap();
        t.write_all(b"new").unwrap();
        let out = publish(t.into_temp_path(), &existing).unwrap();

        assert_eq!(out, dir.path().join("photo 2.jpg"));
        assert_eq!(fs::read(&existing).unwrap(), b"original");
        assert_eq!(fs::read(&out).unwrap(), b"new");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "temporary file removed after publish");
    }

    #[test]
    fn concurrent_publishes_get_distinct_names() {
        let dir = tempfile::tempdir().unwrap();
        let desired = dir.path().join("out.txt");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.path().to_path_buf();
                let desired = desired.clone();
                std::thread::spawn(move || {
                    let mut t = temp_in(&dir, &format!("c{i}"), "txt").unwrap();
                    write!(t, "{i}").unwrap();
                    publish(t.into_temp_path(), &desired).unwrap()
                })
            })
            .collect();
        let mut outs: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        outs.sort();
        outs.dedup();
        assert_eq!(outs.len(), 8);
    }

    #[test]
    fn groups_go_beside_or_into_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let make = |n: usize| {
            let tmp = temp_dir_in(dir.path(), "g").unwrap();
            let files: Vec<(PathBuf, String)> = (1..=n)
                .map(|i| {
                    let p = tmp.path().join(format!("raw{i}"));
                    fs::write(&p, format!("{i}")).unwrap();
                    (p, format!("doc - page {i}.txt"))
                })
                .collect();
            (tmp, files)
        };
        let (tmp, files) = make(2);
        let out = publish_group(tmp, files, dir.path(), "doc (pages)").unwrap();
        assert_eq!(
            out,
            vec![
                dir.path().join("doc - page 1.txt"),
                dir.path().join("doc - page 2.txt")
            ]
        );

        let (tmp, files) = make(6);
        let out = publish_group(tmp, files, dir.path(), "doc (pages)").unwrap();
        assert_eq!(out, vec![dir.path().join("doc (pages)")]);
        assert_eq!(fs::read_dir(&out[0]).unwrap().count(), 6);
        assert_eq!(size_of(&out[0]), 6);

        // A second run never merges into the first folder.
        let (tmp, files) = make(5);
        let out = publish_group(tmp, files, dir.path(), "doc (pages)").unwrap();
        assert_eq!(out, vec![dir.path().join("doc (pages) 2")]);
        let hidden: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(hidden.is_empty(), "temporary folders removed");
    }

    #[test]
    fn leftovers_are_removed_by_tag() {
        let dir = tempfile::tempdir().unwrap();
        let keep = temp_in(dir.path(), "other", "png")
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        let gone = temp_in(dir.path(), "job7", "png")
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        let gone_dir = temp_dir_in(dir.path(), "job7").unwrap().keep();
        fs::write(gone_dir.join("page.png"), b"x").unwrap();
        remove_leftovers(dir.path(), "job7");
        assert!(keep.exists());
        assert!(!gone.exists());
        assert!(!gone_dir.exists());
    }
}
