//! Folders: walking them, and making tidied copies (merged, flattened,
//! sorted by kind), lists of what's inside, and duplicate reports. The
//! originals are only ever read.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::engines::doc::thousands;
use crate::error::{Error, Result};
use crate::format::{Category, Format};
use crate::naming;

/// Most items Radlet walks in one go.
const MAX_ITEMS: usize = 500_000;

/// One item inside a folder.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Path from the folder walked.
    pub rel: PathBuf,
    pub path: PathBuf,
    pub dir: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub mode: u32,
}

/// Names never copied or listed: Finder's view settings, and Radlet's own
/// temporary files.
pub fn ignored(name: &str) -> bool {
    name == ".DS_Store" || name.starts_with(naming::TEMP_PREFIX)
}

fn mode_of(meta: &fs::Metadata) -> u32 {
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
}

fn sorted_children(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut children: Vec<fs::DirEntry> = fs::read_dir(dir)?.flatten().collect();
    children.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
    Ok(children)
}

/// Everything inside `root`, depth first, in name order. Symbolic links
/// are left out (never followed), as are [`ignored`] names.
pub fn walk(root: &Path, cancel: &AtomicBool) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(root.to_path_buf(), PathBuf::new())];
    while let Some((dir, rel)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let children = sorted_children(&dir)?;
        let mut subdirs = Vec::new();
        for child in children {
            let name = child.file_name();
            if ignored(&name.to_string_lossy()) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(child.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() || !(meta.is_dir() || meta.is_file()) {
                continue;
            }
            let entry = Entry {
                rel: rel.join(&name),
                path: child.path(),
                dir: meta.is_dir(),
                len: if meta.is_file() { meta.len() } else { 0 },
                modified: meta.modified().ok(),
                mode: mode_of(&meta),
            };
            if entry.dir {
                subdirs.push((entry.path.clone(), entry.rel.clone()));
            }
            out.push(entry);
            if out.len() > MAX_ITEMS {
                return Err(Error::TooLarge(format!(
                    "this folder holds more than {} items",
                    thousands(MAX_ITEMS as u64)
                )));
            }
        }
        // Reverse so the stack visits them in name order.
        stack.extend(subdirs.into_iter().rev());
    }
    // Depth-first order with each folder's contents after it.
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// The formats (by extension) of files inside `folders`, to decide which
/// conversions of their contents to offer. Looks at no more than
/// `limit` files, and never follows links.
pub fn formats_inside(folders: &[PathBuf], limit: usize) -> Vec<Format> {
    // The wheel waits for this, so it stops early on huge trees and slow
    // network shares: after `limit` entries (folders count) or a second
    // and a half, whichever comes first.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    let mut found: Vec<Format> = Vec::new();
    let mut seen = 0usize;
    let mut stack: Vec<PathBuf> = folders.to_vec();
    while let Some(dir) = stack.pop() {
        if std::time::Instant::now() > deadline {
            break;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen >= limit {
                return found;
            }
            let name = e.file_name();
            if ignored(&name.to_string_lossy()) {
                continue;
            }
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                if let Some(f) = Format::from_path(&e.path()) {
                    if !found.contains(&f) {
                        found.push(f);
                    }
                }
            }
        }
    }
    found
}

/// A path's parts joined with "/", for archives and reports.
pub fn slash_path(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Sizes as Finder shows them: "820 bytes", "12 KB", "3.4 MB", "1.2 GB".
pub fn size_label(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b if b >= 1_000 => format!("{:.0} KB", b as f64 / 1e3),
        1 => "1 byte".into(),
        b => format!("{b} bytes"),
    }
}

/// Local date and time: (year, month, day, hour, minute, second).
pub fn local_time(t: SystemTime) -> Option<(i32, u32, u32, u32, u32, u32)> {
    let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    #[cfg(unix)]
    {
        let tt = secs as libc::time_t;
        // SAFETY: localtime_r only writes into the tm we own.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let ok = unsafe { !libc::localtime_r(&tt, &mut tm).is_null() };
        if ok {
            return Some((
                tm.tm_year + 1900,
                (tm.tm_mon + 1) as u32,
                tm.tm_mday as u32,
                tm.tm_hour as u32,
                tm.tm_min as u32,
                tm.tm_sec as u32,
            ));
        }
    }
    // UTC, from days since 1970 (Howard Hinnant's civil-from-days).
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    Some((
        y,
        m,
        d,
        (rem / 3600) as u32,
        (rem % 3600 / 60) as u32,
        (rem % 60) as u32,
    ))
}

/// The moment a local date and time names.
pub fn from_local_time(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Option<SystemTime> {
    #[cfg(unix)]
    {
        // SAFETY: mktime reads and normalises the tm we own.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = y - 1900;
        tm.tm_mon = mo as i32 - 1;
        tm.tm_mday = d as i32;
        tm.tm_hour = h as i32;
        tm.tm_min = mi as i32;
        tm.tm_sec = s as i32;
        tm.tm_isdst = -1;
        let t = unsafe { libc::mktime(&mut tm) };
        if t >= 0 {
            return Some(UNIX_EPOCH + std::time::Duration::from_secs(t as u64));
        }
        None
    }
    #[cfg(not(unix))]
    {
        let (y, m) = if mo <= 2 {
            (y as i64 - 1, mo as i64 + 9)
        } else {
            (y as i64, mo as i64 - 3)
        };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * m + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        let secs = days * 86_400 + (h * 3600 + mi * 60 + s) as i64;
        (secs >= 0).then(|| UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
    }
}

fn stamp(t: SystemTime) -> String {
    match local_time(t) {
        Some((y, mo, d, h, mi, _)) => format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}"),
        None => String::new(),
    }
}

/// Copies a file, keeping its modification date. (On APFS the copy is a
/// clone, so it takes no extra space until one of them changes.)
pub fn copy_file(src: &Path, dst: &Path, modified: Option<SystemTime>) -> Result<()> {
    fs::copy(src, dst)?;
    if let Some(t) = modified {
        if let Ok(f) = fs::File::options().write(true).open(dst) {
            let _ = f.set_modified(t);
        }
    }
    Ok(())
}

/// `dir/name`, or `dir/name 2`, `dir/name 3`… if taken.
pub fn free_path(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let base = dir.join(name);
    (1..)
        .map(|n| naming::numbered(&base, n))
        .find(|p| fs::symlink_metadata(p).is_err())
        .expect("a free name")
}

/// True when two files hold the same bytes.
pub fn same_bytes(a: &Path, b: &Path) -> Result<bool> {
    let (ma, mb) = (fs::metadata(a)?, fs::metadata(b)?);
    if ma.len() != mb.len() || !ma.is_file() || !mb.is_file() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (fs::File::open(a)?, fs::File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

/// Copies one file into `dir` under its own name. A file already there
/// with the same bytes is kept as it is; one with other contents makes
/// this one "name 2". Returns whether a copy was made.
fn put(src: &Entry, dir: &Path) -> Result<bool> {
    let name = src.path.file_name().unwrap_or_default();
    let dest = dir.join(name);
    if fs::symlink_metadata(&dest).is_ok() {
        if same_bytes(&dest, &src.path)? {
            return Ok(false);
        }
        copy_file(&src.path, &free_path(dir, name), src.modified)?;
        return Ok(true);
    }
    copy_file(&src.path, &dest, src.modified)?;
    Ok(true)
}

fn total_bytes(entries: &[Entry]) -> u64 {
    entries.iter().map(|e| e.len).sum::<u64>().max(1)
}

/// Copies every folder's contents into `out`, keeping their structure.
pub fn merge(
    folders: &[PathBuf],
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let walked: Vec<Vec<Entry>> = folders
        .iter()
        .map(|f| walk(f, cancel))
        .collect::<Result<_>>()?;
    let total: u64 = walked.iter().map(|w| total_bytes(w)).sum();
    let mut done = 0u64;
    for entries in &walked {
        // Where each of this folder's subfolders went (renamed when a
        // file of the same name was already there).
        let mut dirs: HashMap<PathBuf, PathBuf> = HashMap::new();
        dirs.insert(PathBuf::new(), out.to_path_buf());
        for e in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let parent_rel = e.rel.parent().map(Path::to_path_buf).unwrap_or_default();
            let parent = dirs
                .get(&parent_rel)
                .cloned()
                .unwrap_or_else(|| out.join(&parent_rel));
            if e.dir {
                let name = e.path.file_name().unwrap_or_default();
                let mut dest = parent.join(name);
                if fs::symlink_metadata(&dest).is_ok_and(|m| !m.is_dir()) {
                    dest = free_path(&parent, name);
                }
                fs::create_dir_all(&dest)?;
                dirs.insert(e.rel.clone(), dest);
                continue;
            }
            fs::create_dir_all(&parent)?;
            put(e, &parent)?;
            done += e.len;
            progress(done as f32 / total as f32);
        }
    }
    Ok(())
}

/// Copies every file in `folder` and its subfolders into `out`, side by
/// side.
pub fn flatten(
    folder: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let entries = walk(folder, cancel)?;
    if !entries.iter().any(|e| e.dir) {
        return Err(Error::NothingToDo("it has no subfolders to flatten".into()));
    }
    let total = total_bytes(&entries);
    let mut done = 0u64;
    for e in entries.iter().filter(|e| !e.dir) {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        put(e, out)?;
        done += e.len;
        progress(done as f32 / total as f32);
    }
    Ok(())
}

/// The group a file goes in when sorting by kind.
pub fn kind_folder(path: &Path) -> &'static str {
    if let Some(f) = Format::from_path(path) {
        return match f.category() {
            Category::Image => "Images",
            Category::Document => "Documents",
            Category::Audio => "Audio",
            Category::Video => "Videos",
            Category::Text | Category::Subtitle => "Text",
            Category::Data => "Data",
            Category::Archive => "Archives",
            Category::Folder => "Folders",
        };
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "doc" | "pages" | "odt" | "key" | "ppt" | "pptx" | "odp" | "epub" | "xls" | "xlsx"
        | "numbers" | "ods" => "Documents",
        "psd" | "ai" | "sketch" | "fig" | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "eps"
        | "jfif" => "Images",
        "m4v" | "mpg" | "mpeg" | "3gp" => "Videos",
        "aac" | "mid" | "midi" | "caf" => "Audio",
        "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "dmg" | "iso" | "pkg" => {
            "Archives"
        }
        "ttf" | "otf" | "woff" | "woff2" | "ttc" => "Fonts",
        "py" | "js" | "ts" | "tsx" | "jsx" | "rs" | "swift" | "c" | "h" | "cpp" | "hpp" | "m"
        | "java" | "kt" | "go" | "rb" | "php" | "sh" | "zsh" | "css" | "scss" | "xml" | "yaml"
        | "yml" | "toml" | "sql" | "ipynb" => "Code",
        "app" => "Apps",
        _ => "Other",
    }
}

/// Copies `folder` into `out` with its files grouped by kind; subfolders
/// go into "Folders" unchanged.
pub fn sort_by_kind(
    folder: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let top: Vec<fs::DirEntry> = sorted_children(folder)?
        .into_iter()
        .filter(|e| !ignored(&e.file_name().to_string_lossy()))
        .filter(|e| fs::symlink_metadata(e.path()).is_ok_and(|m| !m.file_type().is_symlink()))
        .collect();
    if !top.iter().any(|e| e.path().is_file()) {
        return Err(Error::NothingToDo(
            "it has no files at its top level to sort".into(),
        ));
    }
    let n = top.len().max(1);
    for (i, child) in top.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let path = child.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            let group = out.join("Folders");
            fs::create_dir_all(&group)?;
            let dest = free_path(&group, &child.file_name());
            copy_tree(&path, &dest, cancel)?;
        } else if meta.is_file() {
            let group = out.join(kind_folder(&path));
            fs::create_dir_all(&group)?;
            copy_file(
                &path,
                &free_path(&group, &child.file_name()),
                meta.modified().ok(),
            )?;
        }
        progress((i + 1) as f32 / n as f32);
    }
    Ok(())
}

/// Copies a folder and everything in it to `dest` (which must not exist).
pub fn copy_tree(src: &Path, dest: &Path, cancel: &AtomicBool) -> Result<()> {
    fs::create_dir_all(dest)?;
    for e in walk(src, cancel)? {
        let to = dest.join(&e.rel);
        if e.dir {
            fs::create_dir_all(&to)?;
        } else {
            if let Some(p) = to.parent() {
                fs::create_dir_all(p)?;
            }
            copy_file(&e.path, &to, e.modified)?;
        }
    }
    Ok(())
}

/// What's in a folder: an indented tree with sizes (`Txt`), or one row
/// per item with its size and date (`Csv`).
pub fn list(folder: &Path, fmt: Format, cancel: &AtomicBool) -> Result<String> {
    let entries = walk(folder, cancel)?;
    let files = entries.iter().filter(|e| !e.dir).count() as u64;
    let dirs = entries.iter().filter(|e| e.dir).count() as u64;
    let bytes: u64 = entries.iter().map(|e| e.len).sum();
    let name = crate::format::display_name(folder);
    if fmt == Format::Csv {
        let mut rows = vec![vec![
            "Path".to_string(),
            "Name".into(),
            "Kind".into(),
            "Size (bytes)".into(),
            "Modified".into(),
        ]];
        for e in &entries {
            rows.push(vec![
                slash_path(&e.rel),
                e.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                if e.dir {
                    "Folder".into()
                } else {
                    kind_folder(&e.path).trim_end_matches('s').to_string()
                },
                if e.dir {
                    String::new()
                } else {
                    e.len.to_string()
                },
                e.modified.map(stamp).unwrap_or_default(),
            ]);
        }
        return crate::engines::data::write_rows(&rows, b',');
    }
    let mut out = format!(
        "{name}\n{} {} in {} {} · {}\n\n",
        thousands(files),
        if files == 1 { "file" } else { "files" },
        thousands(dirs),
        if dirs == 1 { "folder" } else { "folders" },
        size_label(bytes)
    );
    tree(folder, "", &mut out, cancel, 0)?;
    Ok(out)
}

fn tree(
    dir: &Path,
    prefix: &str,
    out: &mut String,
    cancel: &AtomicBool,
    depth: usize,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if depth > 64 {
        return Ok(());
    }
    let mut children: Vec<(String, PathBuf, fs::Metadata)> = sorted_children(dir)?
        .into_iter()
        .filter_map(|c| {
            let name = c.file_name().to_string_lossy().into_owned();
            let meta = fs::symlink_metadata(c.path()).ok()?;
            (!ignored(&name) && !meta.file_type().is_symlink()).then_some((name, c.path(), meta))
        })
        .collect();
    // Folders first, then files, each in name order.
    children.sort_by_key(|(n, _, m)| (!m.is_dir(), n.to_lowercase()));
    let n = children.len();
    for (i, (name, path, meta)) in children.into_iter().enumerate() {
        let last = i + 1 == n;
        let branch = if last { "└── " } else { "├── " };
        if meta.is_dir() {
            out.push_str(&format!("{prefix}{branch}{name}/\n"));
            let deeper = format!("{prefix}{}", if last { "    " } else { "│   " });
            tree(&path, &deeper, out, cancel, depth + 1)?;
        } else {
            out.push_str(&format!(
                "{prefix}{branch}{name}  {}\n",
                size_label(meta.len())
            ));
        }
    }
    Ok(())
}

/// A report of files with identical contents among `inputs` (folders
/// are searched through). Errors with NothingToDo when there are none.
pub fn duplicates(
    inputs: &[PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<String> {
    let single_folder = inputs.len() == 1 && inputs[0].is_dir();
    // (shown path, real path, size)
    let mut files: Vec<(String, PathBuf, u64)> = Vec::new();
    for input in inputs {
        let meta = fs::symlink_metadata(input)?;
        if meta.is_dir() {
            let top = crate::format::display_name(input);
            for e in walk(input, cancel)?.into_iter().filter(|e| !e.dir) {
                let rel = slash_path(&e.rel);
                let shown = if single_folder {
                    rel
                } else {
                    format!("{top}/{rel}")
                };
                files.push((shown, e.path, e.len));
            }
        } else if meta.is_file() {
            files.push((
                crate::format::display_name(input),
                input.clone(),
                meta.len(),
            ));
        }
    }
    // The same file reached twice (a folder and a folder inside it, or a
    // file chosen as well as its folder) is one file, not a duplicate.
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(fs::canonicalize(&f.1).unwrap_or_else(|_| f.1.clone())));
    let count = files.len() as u64;
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        if f.2 > 0 {
            by_size.entry(f.2).or_default().push(i);
        }
    }
    let candidates: Vec<Vec<usize>> = by_size.into_values().filter(|v| v.len() > 1).collect();
    let to_hash: u64 = candidates
        .iter()
        .flatten()
        .map(|&i| files[i].2)
        .sum::<u64>()
        .max(1);
    let mut hashed = 0u64;
    let mut groups: Vec<(u64, Vec<String>)> = Vec::new();
    for set in candidates {
        let mut by_hash: HashMap<String, Vec<usize>> = HashMap::new();
        for i in set {
            let h = crate::engines::sha256(&files[i].1, cancel)?;
            by_hash.entry(h).or_default().push(i);
            hashed += files[i].2;
            progress(hashed as f32 / to_hash as f32);
        }
        for same in by_hash.into_values().filter(|v| v.len() > 1) {
            let size = files[same[0]].2;
            let mut names: Vec<String> = same.iter().map(|&i| files[i].0.clone()).collect();
            names.sort();
            groups.push((size, names));
        }
    }
    if groups.is_empty() {
        return Err(Error::NothingToDo(format!(
            "no duplicates among {} {}",
            thousands(count),
            if count == 1 { "file" } else { "files" }
        )));
    }
    // Most space wasted first.
    groups.sort_by(|a, b| {
        let wa = a.0 * (a.1.len() as u64 - 1);
        let wb = b.0 * (b.1.len() as u64 - 1);
        wb.cmp(&wa).then_with(|| a.1.cmp(&b.1))
    });
    let copies: u64 = groups.iter().map(|g| g.1.len() as u64).sum();
    let wasted: u64 = groups.iter().map(|g| g.0 * (g.1.len() as u64 - 1)).sum();
    let what = if single_folder {
        crate::format::display_name(&inputs[0])
    } else {
        format!("{} items", inputs.len())
    };
    let mut out = format!(
        "Duplicates in {what}\n{} {} of identical files · {} files · {} could be freed by keeping one of each\n",
        thousands(groups.len() as u64),
        if groups.len() == 1 { "set" } else { "sets" },
        thousands(copies),
        size_label(wasted)
    );
    for (size, names) in &groups {
        out.push_str(&format!(
            "\n{} each, {} copies\n",
            size_label(*size),
            names.len()
        ));
        for n in names {
            out.push_str(&format!("  {n}\n"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_fixture(root: &Path) {
        fs::create_dir_all(root.join("a/deep")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("top.txt"), "top").unwrap();
        fs::write(root.join("photo.png"), "png-bytes").unwrap();
        fs::write(root.join("a/same.txt"), "same").unwrap();
        fs::write(root.join("a/deep/same.txt"), "same").unwrap();
        fs::write(root.join("b/same.txt"), "different").unwrap();
        fs::write(root.join(".DS_Store"), "junk").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("top.txt"), root.join("link.txt")).unwrap();
    }

    #[test]
    fn walking_skips_links_and_junk() {
        let dir = tempfile::tempdir().unwrap();
        tree_fixture(dir.path());
        let cancel = AtomicBool::new(false);
        let names: Vec<String> = walk(dir.path(), &cancel)
            .unwrap()
            .iter()
            .map(|e| slash_path(&e.rel))
            .collect();
        assert_eq!(
            names,
            [
                "a",
                "a/deep",
                "a/deep/same.txt",
                "a/same.txt",
                "b",
                "b/same.txt",
                "photo.png",
                "top.txt"
            ]
        );
    }

    #[test]
    fn flatten_merge_and_sort() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        tree_fixture(&src);
        let cancel = AtomicBool::new(false);

        let flat = dir.path().join("flat");
        fs::create_dir(&flat).unwrap();
        flatten(&src, &flat, &cancel, &mut |_| {}).unwrap();
        let mut names: Vec<String> = fs::read_dir(&flat)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        // The two identical "same.txt" are kept once; the different one
        // is renamed.
        assert_eq!(names, ["photo.png", "same 2.txt", "same.txt", "top.txt"]);

        let other = dir.path().join("other");
        fs::create_dir_all(other.join("a")).unwrap();
        fs::write(other.join("a/same.txt"), "same").unwrap();
        fs::write(other.join("top.txt"), "other top").unwrap();
        fs::write(other.join("b"), "a file named like a folder").unwrap();
        let merged = dir.path().join("merged");
        fs::create_dir(&merged).unwrap();
        merge(&[src.clone(), other], &merged, &cancel, &mut |_| {}).unwrap();
        assert_eq!(fs::read_to_string(merged.join("top.txt")).unwrap(), "top");
        assert_eq!(
            fs::read_to_string(merged.join("top 2.txt")).unwrap(),
            "other top"
        );
        assert!(
            !merged.join("a/same 2.txt").exists(),
            "identical file kept once"
        );
        assert_eq!(
            fs::read_to_string(merged.join("b 2")).unwrap(),
            "a file named like a folder"
        );
        assert!(merged.join("b/same.txt").is_file());

        let sorted = dir.path().join("sorted");
        fs::create_dir(&sorted).unwrap();
        sort_by_kind(&src, &sorted, &cancel, &mut |_| {}).unwrap();
        assert!(sorted.join("Images/photo.png").is_file());
        assert!(sorted.join("Text/top.txt").is_file());
        assert!(sorted.join("Folders/a/deep/same.txt").is_file());
        assert!(!sorted.join("Other").exists());
    }

    #[test]
    fn listing_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("Stuff");
        tree_fixture(&src);
        let cancel = AtomicBool::new(false);
        let txt = list(&src, Format::Txt, &cancel).unwrap();
        assert!(txt.starts_with("Stuff\n5 files in 3 folders · 29 bytes\n\n├── a/\n│   ├── deep/\n│   │   └── same.txt  4 bytes\n│   └── same.txt  4 bytes\n├── b/\n"), "{txt}");
        assert!(txt.ends_with("└── top.txt  3 bytes\n"), "{txt}");
        let csv = list(&src, Format::Csv, &cancel).unwrap();
        assert!(
            csv.starts_with("Path,Name,Kind,Size (bytes),Modified\na,a,Folder,,"),
            "{csv}"
        );
        assert!(csv.contains("\nphoto.png,photo.png,Image,9,"), "{csv}");

        let report = duplicates(std::slice::from_ref(&src), &cancel, &mut |_| {}).unwrap();
        assert!(
            report.starts_with(
                "Duplicates in Stuff\n1 set of identical files · 2 files · 4 bytes could be freed"
            ),
            "{report}"
        );
        assert!(report.contains("  a/deep/same.txt\n  a/same.txt\n"));
        let none = duplicates(&[src.join("b")], &cancel, &mut |_| {});
        assert!(matches!(none, Err(Error::NothingToDo(m)) if m == "no duplicates among 1 file"));
        // Overlapping choices don't make a file its own duplicate.
        let overlap = duplicates(
            &[src.join("b"), src.join("b/same.txt"), src.join("b")],
            &cancel,
            &mut |_| {},
        );
        assert!(matches!(overlap, Err(Error::NothingToDo(_))), "{overlap:?}");
    }

    #[test]
    fn times_round_trip() {
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let (y, mo, d, h, mi, s) = local_time(t).unwrap();
        assert_eq!(from_local_time(y, mo, d, h, mi, s), Some(t));
        assert_eq!(size_label(1_234_567_890), "1.2 GB");
        assert_eq!(size_label(1), "1 byte");
    }
}
