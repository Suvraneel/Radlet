//! ZIP archives: made from files and folders (optionally locked with a
//! password, AES-256), and extracted safely (no paths outside the
//! destination, no links, bounded size). Deflate, Deflate64, LZMA, XZ and
//! Zstandard entries are read, and AES or traditional ZIP encryption with
//! the password given.

use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::engines::folder::{self, from_local_time, local_time, slash_path, Entry};
use crate::error::{Error, Result};

/// Most entries an archive may have.
const MAX_ENTRIES: usize = 200_000;
/// Most bytes one extraction may write.
const MAX_TOTAL: u64 = 64 << 30;

/// Already-compressed files are stored as they are; squeezing them again
/// costs time and saves nothing.
fn stored(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "heic"
            | "heif"
            | "avif"
            | "mp3"
            | "m4a"
            | "aac"
            | "ogg"
            | "opus"
            | "flac"
            | "mp4"
            | "mov"
            | "m4v"
            | "mkv"
            | "webm"
            | "avi"
            | "zip"
            | "gz"
            | "tgz"
            | "bz2"
            | "xz"
            | "7z"
            | "rar"
            | "zst"
            | "docx"
            | "xlsx"
            | "pptx"
            | "pages"
            | "numbers"
            | "key"
            | "dmg"
            | "epub"
            | "jar"
            | "apk"
    )
}

fn zip_time(t: Option<SystemTime>) -> DateTime {
    t.and_then(local_time)
        .and_then(|(y, mo, d, h, mi, s)| {
            DateTime::from_date_and_time(
                u16::try_from(y).ok()?,
                mo as u8,
                d as u8,
                h as u8,
                mi as u8,
                s as u8,
            )
            .ok()
        })
        .unwrap_or_default()
}

fn options<'k>(
    entry: &Entry,
    password: Option<&'k str>,
) -> zip::write::FileOptions<'k, 'static, ()> {
    let method = if entry.dir || stored(&entry.path) {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .last_modified_time(zip_time(entry.modified))
        .unix_permissions(entry.mode & 0o777)
        .large_file(entry.len >= u32::MAX as u64);
    match password {
        Some(p) if !entry.dir => options.with_aes_encryption(AesMode::Aes256, p),
        _ => options,
    }
}

fn zip_err(e: zip::result::ZipError) -> Error {
    Error::Internal(format!("couldn't write the ZIP ({e})"))
}

/// Writes `items` (files and folders, each under its own name) into a
/// new ZIP at `out`. With `password`, every file in it is locked with
/// AES-256 (names stay visible, as in any ZIP).
pub fn zip(
    items: &[PathBuf],
    out: &Path,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    if password.is_some_and(str::is_empty) {
        return Err(Error::Invalid("the password is empty".into()));
    }
    // Everything to write: (name in the archive, entry).
    let mut plan: Vec<(String, Entry)> = Vec::new();
    let mut tops: Vec<String> = Vec::new();
    for item in items {
        let meta = fs::symlink_metadata(item)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let base = crate::format::display_name(item);
        // Two items with the same name (from different folders) mustn't
        // collide inside the archive.
        let mut top = base.clone();
        let mut n = 2;
        while tops.contains(&top) {
            top = match base.rsplit_once('.') {
                Some((stem, ext)) if !meta.is_dir() && !stem.is_empty() => {
                    format!("{stem} {n}.{ext}")
                }
                _ => format!("{base} {n}"),
            };
            n += 1;
        }
        tops.push(top.clone());
        let entry = Entry {
            rel: PathBuf::from(&top),
            path: item.clone(),
            dir: meta.is_dir(),
            len: if meta.is_file() { meta.len() } else { 0 },
            modified: meta.modified().ok(),
            mode: {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode()
                }
                #[cfg(not(unix))]
                {
                    if meta.is_dir() {
                        0o755
                    } else {
                        0o644
                    }
                }
            },
        };
        if entry.dir {
            plan.push((format!("{top}/"), entry));
            for e in folder::walk(item, cancel)? {
                let name = format!(
                    "{top}/{}{}",
                    slash_path(&e.rel),
                    if e.dir { "/" } else { "" }
                );
                plan.push((name, e));
            }
        } else if meta.is_file() {
            plan.push((top, entry));
        }
    }
    if plan.is_empty() {
        return Err(Error::NothingToDo(
            "there's nothing to put in the archive".into(),
        ));
    }
    let total: u64 = plan.iter().map(|(_, e)| e.len).sum::<u64>().max(1);
    let mut done = 0u64;
    let mut zw = ZipWriter::new(BufWriter::new(File::create(out)?));
    let mut buf = vec![0u8; 1 << 20];
    for (name, e) in &plan {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if e.dir {
            zw.add_directory(name.trim_end_matches('/'), options(e, None))
                .map_err(zip_err)?;
            continue;
        }
        zw.start_file(name.as_str(), options(e, password))
            .map_err(zip_err)?;
        let mut f = File::open(&e.path)?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            zw.write_all(&buf[..n])?;
            done += n as u64;
            progress(done as f32 / total as f32);
        }
    }
    let mut w = zw.finish().map_err(zip_err)?;
    w.flush()?;
    Ok(())
}

fn open_err(path: &Path, e: zip::result::ZipError) -> Error {
    Error::UnsupportedInput(format!(
        "{} isn't a ZIP archive Radlet can open ({e})",
        crate::format::display_name(path)
    ))
}

/// Extracts `archive` into the empty folder `into`, decrypting with
/// `password` when it's locked. Entries that would land outside it, links,
/// and Finder's resource-fork folders are skipped; an archive that would
/// expand beyond reason is refused.
pub fn extract(
    archive: &Path,
    into: &Path,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    // A generous ratio: real archives rarely shrink below a thousandth.
    extract_bounded(archive, into, password, 1 << 30, 1000, cancel, progress)
}

/// Whether any file in the ZIP at `path` is locked with a password.
pub fn is_locked(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(mut z) = ZipArchive::new(BufReader::new(file)) else {
        return false;
    };
    (0..z.len().min(MAX_ENTRIES)).any(|i| z.by_index_raw(i).is_ok_and(|f| f.encrypted()))
}

fn locked() -> Error {
    Error::PasswordNeeded("this ZIP is locked with a password".into())
}

/// [`extract`] writing at most `ratio` times the archive's size (but
/// always allowing `floor` bytes, and never more than 64 GB).
#[allow(clippy::too_many_arguments)]
fn extract_bounded(
    archive: &Path,
    into: &Path,
    password: Option<&str>,
    floor: u64,
    ratio: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let file = File::open(archive)?;
    let size = file.metadata()?.len();
    let mut z = ZipArchive::new(BufReader::new(file)).map_err(|e| open_err(archive, e))?;
    if z.len() > MAX_ENTRIES {
        return Err(Error::TooLarge(format!(
            "this archive holds more than {} items",
            crate::engines::doc::thousands(MAX_ENTRIES as u64)
        )));
    }
    let limit = MAX_TOTAL.min(size.saturating_mul(ratio).max(floor));
    let mut declared = 0u64;
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(|e| open_err(archive, e))?;
        declared = declared.saturating_add(f.size());
    }
    let bomb = || {
        Error::TooLarge(
            "this archive would expand to far more than its size; it may be a zip bomb".into(),
        )
    };
    if declared > limit {
        return Err(bomb());
    }
    let total = declared.max(1);
    let mut written = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    for i in 0..z.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let opened = match password {
            Some(p) => z.by_index_decrypt(i, p.as_bytes()),
            None => z.by_index(i),
        };
        let mut f = match opened {
            Ok(f) => f,
            Err(zip::result::ZipError::UnsupportedArchive(msg)) => {
                return Err(if msg == zip::result::ZipError::PASSWORD_REQUIRED {
                    locked()
                } else {
                    Error::UnsupportedInput(format!(
                        "this archive uses something Radlet can't open ({msg})"
                    ))
                })
            }
            Err(zip::result::ZipError::InvalidPassword) => return Err(wrong_password()),
            Err(e) => return Err(open_err(archive, e)),
        };
        let encrypted = f.encrypted();
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        let skip = rel.components().any(|c| match c {
            Component::Normal(s) => {
                let s = s.to_string_lossy();
                s == "__MACOSX" || folder::ignored(&s)
            }
            _ => true,
        });
        if skip || f.is_symlink() || rel.as_os_str().is_empty() {
            continue;
        }
        let dest = into.join(&rel);
        if f.is_dir() {
            fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        // An archive can name the same file twice; keep both.
        let dest = if fs::symlink_metadata(&dest).is_ok() {
            folder::free_path(
                dest.parent().unwrap_or(into),
                dest.file_name().unwrap_or_default(),
            )
        } else {
            dest
        };
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)?;
        loop {
            let n = f.read(&mut buf).map_err(|e| {
                // The old ZIP encryption lets some wrong passwords through;
                // the checksum then fails.
                if encrypted && password.is_some() {
                    wrong_password()
                } else {
                    Error::UnsupportedInput(format!(
                        "couldn't read {} from the archive ({e})",
                        slash_path(&rel)
                    ))
                }
            })?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > limit {
                return Err(bomb());
            }
            out.write_all(&buf[..n])?;
            progress((written as f32 / total as f32).min(1.0));
        }
        drop(out);
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            // Keep the execute bits; never make files writable by others.
            let m = ((mode & 0o777) | 0o600) & !0o022;
            let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(m));
        }
        if let Some(dt) = f.last_modified() {
            let t = from_local_time(
                dt.year() as i32,
                dt.month() as u32,
                dt.day() as u32,
                dt.hour() as u32,
                dt.minute() as u32,
                dt.second() as u32,
            );
            if let (Some(t), Ok(file)) = (t, OpenOptions::new().write(true).open(&dest)) {
                let _ = file.set_modified(t);
            }
        }
    }
    Ok(())
}

fn wrong_password() -> Error {
    Error::PasswordNeeded("that password doesn't open this ZIP".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zips_and_extracts_files_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("Project");
        fs::create_dir_all(src.join("sub/empty")).unwrap();
        fs::write(src.join("readme.md"), "# Hi\n".repeat(100)).unwrap();
        fs::write(src.join("sub/data.bin"), [7u8; 3000]).unwrap();
        fs::write(src.join(".DS_Store"), "junk").unwrap();
        let lone = dir.path().join("readme.md");
        fs::write(&lone, "another readme").unwrap();
        let cancel = AtomicBool::new(false);
        let out = dir.path().join("a.zip");
        zip(&[src.clone(), lone], &out, None, &cancel, &mut |_| {}).unwrap();
        assert_eq!(
            crate::format::detect(&out).unwrap(),
            crate::format::Format::Zip
        );

        let mut z = ZipArchive::new(File::open(&out).unwrap()).unwrap();
        let mut names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "Project/",
                "Project/readme.md",
                "Project/sub/",
                "Project/sub/data.bin",
                "Project/sub/empty/",
                "readme.md"
            ]
        );
        assert_eq!(
            z.by_name("Project/readme.md").unwrap().compression(),
            CompressionMethod::Deflated
        );

        let into = dir.path().join("x");
        fs::create_dir(&into).unwrap();
        extract(&out, &into, None, &cancel, &mut |_| {}).unwrap();
        assert_eq!(
            fs::read(into.join("Project/sub/data.bin")).unwrap(),
            [7u8; 3000]
        );
        assert!(into.join("Project/sub/empty").is_dir());
        assert_eq!(
            fs::read_to_string(into.join("readme.md")).unwrap(),
            "another readme"
        );
        assert!(!into.join("Project/.DS_Store").exists());
    }

    fn crafted(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zw = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in entries {
            zw.start_file(*name, SimpleFileOptions::default()).unwrap();
            zw.write_all(body).unwrap();
        }
        zw.finish().unwrap().into_inner()
    }

    #[test]
    fn hostile_archives_stay_inside() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.zip");
        fs::write(
            &bad,
            crafted(&[
                ("../escape.txt", b"no"),
                ("/abs.txt", b"no"),
                ("ok/../../escape2.txt", b"no"),
                ("__MACOSX/._x", b"fork"),
                ("fine.txt", b"yes"),
            ]),
        )
        .unwrap();
        let into = dir.path().join("out");
        fs::create_dir(&into).unwrap();
        let cancel = AtomicBool::new(false);
        extract(&bad, &into, None, &cancel, &mut |_| {}).unwrap();
        assert!(!dir.path().join("escape.txt").exists());
        assert!(!dir.path().join("escape2.txt").exists());
        let mut names: Vec<String> = fs::read_dir(&into)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        // "/abs.txt" lands inside (or is dropped); the escapes are dropped.
        assert!(names.contains(&"fine.txt".to_string()), "{names:?}");
        assert!(
            !names
                .iter()
                .any(|n| n.contains("escape") || n == "__MACOSX"),
            "{names:?}"
        );
    }

    #[test]
    fn zip_bombs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        // 20 MB of zeros squeezes into about 20 KB; with the limit scaled
        // down (100 times the size, 1 MB at least) it's refused.
        let mut zw = ZipWriter::new(BufWriter::new(
            File::create(dir.path().join("bomb.zip")).unwrap(),
        ));
        zw.start_file(
            "zeros",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..20 {
            zw.write_all(&chunk).unwrap();
        }
        zw.finish().unwrap();
        let into = dir.path().join("out");
        fs::create_dir(&into).unwrap();
        let cancel = AtomicBool::new(false);
        let bomb = dir.path().join("bomb.zip");
        let r = extract_bounded(&bomb, &into, None, 1 << 20, 100, &cancel, &mut |_| {});
        assert!(matches!(r, Err(Error::TooLarge(_))), "{r:?}");
        // The real limits let it through.
        extract(&bomb, &into, None, &cancel, &mut |_| {}).unwrap();
        assert_eq!(fs::metadata(into.join("zeros")).unwrap().len(), 20 << 20);
    }

    #[test]
    fn passwords_and_other_methods() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let src = dir.path().join("secret.txt");
        fs::write(&src, b"the plans").unwrap();
        let locked_zip = dir.path().join("locked.zip");
        zip(
            std::slice::from_ref(&src),
            &locked_zip,
            Some("hunter2"),
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        assert!(is_locked(&locked_zip));
        assert!(!fs::read(&locked_zip)
            .unwrap()
            .windows(9)
            .any(|w| w == b"the plans"));
        let code = |r: Result<()>| r.unwrap_err().code();
        let into = dir.path().join("a");
        fs::create_dir(&into).unwrap();
        assert_eq!(
            code(extract(&locked_zip, &into, None, &cancel, &mut |_| {})),
            "password_needed"
        );
        let into = dir.path().join("b");
        fs::create_dir(&into).unwrap();
        assert_eq!(
            code(extract(
                &locked_zip,
                &into,
                Some("wrong"),
                &cancel,
                &mut |_| {}
            )),
            "password_needed"
        );
        let into = dir.path().join("c");
        fs::create_dir(&into).unwrap();
        extract(&locked_zip, &into, Some("hunter2"), &cancel, &mut |_| {}).unwrap();
        assert_eq!(fs::read(into.join("secret.txt")).unwrap(), b"the plans");

        // Zstandard and XZ entries, as other tools write them.
        let other = dir.path().join("methods.zip");
        let mut zw = ZipWriter::new(File::create(&other).unwrap());
        for (name, method) in [
            ("z.txt", CompressionMethod::Zstd),
            ("x.txt", CompressionMethod::Xz),
        ] {
            zw.start_file(
                name,
                SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
            zw.write_all(&b"radlet ".repeat(100)).unwrap();
        }
        zw.finish().unwrap();
        assert!(!is_locked(&other));
        let into = dir.path().join("d");
        fs::create_dir(&into).unwrap();
        extract(&other, &into, None, &cancel, &mut |_| {}).unwrap();
        for name in ["z.txt", "x.txt"] {
            assert_eq!(fs::read(into.join(name)).unwrap(), b"radlet ".repeat(100));
        }
    }
}
