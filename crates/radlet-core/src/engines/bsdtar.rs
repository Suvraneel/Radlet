//! 7-Zip, RAR, TAR and disc images, through libarchive's `bsdtar` (on a
//! Mac, the system's own `tar`). Radlet extracts them the way it extracts
//! ZIPs: never outside the destination, without links or macOS clutter,
//! and refusing archives that would expand far beyond their size. It also
//! packs folders as .tar.gz.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::catalog::Capabilities;
use crate::engines::media::spawn_tail_reader;
use crate::error::{Error, Result};

/// Most entries an archive may have.
const MAX_ENTRIES: usize = 200_000;
/// Most bytes one extraction may write.
const MAX_TOTAL: u64 = 64 << 30;

fn tool(caps: &Capabilities) -> Result<&Path> {
    caps.bsdtar
        .as_deref()
        .ok_or_else(|| Error::Missing("7-Zip, RAR, TAR and disc images need macOS's tar".into()))
}

/// Entries and declared size, from `bsdtar -tv` (ls-style lines, the size
/// in the fifth column). Read as it comes, so a listing of millions of
/// entries stops at the limit, and Cancel works while a big archive is read.
fn listing(caps: &Capabilities, archive: &Path, cancel: &AtomicBool) -> Result<(usize, u64)> {
    use std::io::{BufRead, BufReader};
    let mut child = Command::new(tool(caps)?)
        .arg("-tvf")
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = spawn_tail_reader(child.stderr.take());
    let mut entries = 0usize;
    let mut total = 0u64;
    let mut stop = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            entries += 1;
            if let Some(size) = line
                .split_whitespace()
                .nth(4)
                .and_then(|s| s.parse::<u64>().ok())
            {
                total = total.saturating_add(size);
            }
            if entries > MAX_ENTRIES {
                stop = Some(Error::TooLarge(format!(
                    "this archive holds more than {} items",
                    crate::engines::doc::thousands(MAX_ENTRIES as u64)
                )));
            } else if entries.is_multiple_of(256) && cancel.load(Ordering::Relaxed) {
                stop = Some(Error::Cancelled);
            }
            if stop.is_some() {
                break;
            }
        }
    }
    if let Some(e) = stop {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let status = child.wait()?;
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if !status.success() {
        return Err(failure(archive, &stderr.join().unwrap_or_default()));
    }
    Ok((entries, total))
}

fn failure(archive: &Path, stderr: &str) -> Error {
    let clean = |l: &str| {
        l.trim()
            .trim_start_matches("bsdtar: ")
            .trim_start_matches("tar: ")
            .to_string()
    };
    let lines: Vec<String> = stderr
        .lines()
        .map(clean)
        .filter(|l| !l.is_empty() && !l.starts_with("Error exit delayed"))
        .collect();
    // bsdtar carries on past a bad entry, so the reason can be on any line.
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("passphrase") || lower.contains("password") || lower.contains("encrypt") {
        return Error::UnsupportedInput("password-protected archives can't be opened yet".into());
    }
    let reason = lines.first().map(String::as_str).unwrap_or("unknown error");
    Error::UnsupportedInput(format!(
        "{} couldn't be opened ({reason})",
        crate::format::display_name(archive)
    ))
}

/// Bytes under `dir`, not following links.
fn size_of(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => total += e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => {}
            }
        }
    }
    total
}

/// Leaves only ordinary files and folders, readable and removable by
/// their owner: links, pipes, devices and macOS clutter go.
fn tidy(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    fn usable(path: &Path, bits: u32) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::symlink_metadata(path)?.permissions().mode();
        if mode & bits != bits {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | bits))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    fn usable(_: &Path, _: u32) -> Result<()> {
        Ok(())
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        usable(&d, 0o700)?;
        for e in std::fs::read_dir(&d)?.flatten() {
            let path = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            let kind = e.file_type()?;
            if kind.is_dir() && name == "__MACOSX" {
                usable(&path, 0o700)?;
                std::fs::remove_dir_all(&path)?;
            } else if kind.is_dir() {
                stack.push(path);
            } else if !kind.is_file() || name == ".DS_Store" || name.starts_with("._") {
                std::fs::remove_file(&path)?;
            } else {
                usable(&path, 0o600)?;
            }
        }
    }
    Ok(())
}

/// Extracts `archive` into the empty folder `into`.
pub fn extract(
    caps: &Capabilities,
    archive: &Path,
    into: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let (_, declared) = listing(caps, archive, cancel)?;
    let size = std::fs::metadata(archive)?.len();
    // As for ZIPs: at most a thousand times the archive, at least 1 GB.
    let limit = MAX_TOTAL.min(size.saturating_mul(1000).max(1 << 30));
    let bomb = || {
        Error::TooLarge(
            "this archive would expand to far more than its size; it may be an archive bomb".into(),
        )
    };
    if declared > limit {
        return Err(bomb());
    }
    progress(0.05);
    // bsdtar already refuses absolute paths, ".." and writing through links.
    let mut child = Command::new(tool(caps)?)
        .arg("-x")
        .arg("--no-same-owner")
        .args([
            "--exclude",
            "__MACOSX",
            "--exclude",
            "._*",
            "--exclude",
            ".DS_Store",
        ])
        .arg("-C")
        .arg(into)
        .arg("-f")
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = spawn_tail_reader(child.stderr.take());
    let mut checked = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        // Headers can understate what an archive holds: watch the disk too.
        if checked.elapsed() > Duration::from_secs(1) {
            checked = Instant::now();
            let written = size_of(into);
            if written > limit {
                let _ = child.kill();
                let _ = child.wait();
                return Err(bomb());
            }
            if declared > 0 {
                progress((0.05 + 0.9 * written as f32 / declared as f32).min(0.95));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        return Err(failure(archive, &stderr.join().unwrap_or_default()));
    }
    tidy(into)?;
    progress(1.0);
    Ok(())
}

/// Packs `folder` as a .tar.gz at `out`, with the folder at its top.
pub fn pack(
    caps: &Capabilities,
    folder: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    // A bare name ("Trip") has an empty parent: the current folder.
    let parent = folder
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = folder
        .file_name()
        .ok_or_else(|| Error::Invalid("this folder has no name to pack it under".into()))?;
    let mut child = Command::new(tool(caps)?)
        // Without this, macOS's tar adds "._" files for extended attributes.
        .env("COPYFILE_DISABLE", "1")
        .arg("-c")
        .arg("-z")
        .args(["--exclude", ".DS_Store"])
        .arg("-f")
        .arg(out)
        .arg("-C")
        .arg(parent)
        .arg("--")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = spawn_tail_reader(child.stderr.take());
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        let tail = stderr.join().unwrap_or_default();
        return Err(Error::Invalid(format!(
            "couldn't pack the folder ({})",
            tail.lines().last().unwrap_or("unknown error").trim()
        )));
    }
    progress(1.0);
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn caps() -> Option<Capabilities> {
        let c = Capabilities::detect();
        c.bsdtar.is_some().then_some(c)
    }

    #[test]
    fn packs_and_extracts_safely() {
        let Some(caps) = caps() else {
            eprintln!("skipped: no bsdtar");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("Trip");
        std::fs::create_dir_all(src.join("day 1")).unwrap();
        std::fs::write(src.join("day 1/notes.txt"), "sunny").unwrap();
        std::fs::write(src.join(".DS_Store"), "x").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.join("link")).unwrap();
        let fifo = std::ffi::CString::new(src.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
        let cancel = AtomicBool::new(false);
        let tgz = dir.path().join("Trip.tar.gz");
        pack(&caps, &src, &tgz, &cancel, &mut |_| {}).unwrap();
        assert_eq!(
            crate::format::detect(&tgz).unwrap(),
            crate::format::Format::Tar
        );
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract(&caps, &tgz, &out, &cancel, &mut |_| {}).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("Trip/day 1/notes.txt")).unwrap(),
            "sunny"
        );
        assert!(!out.join("Trip/.DS_Store").exists());
        assert!(
            std::fs::symlink_metadata(out.join("Trip/link")).is_err(),
            "links are dropped"
        );
        assert!(
            std::fs::symlink_metadata(out.join("Trip/pipe")).is_err(),
            "so are pipes"
        );
    }

    #[test]
    fn hostile_paths_stay_inside() {
        let Some(caps) = caps() else {
            eprintln!("skipped: no bsdtar");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        // A tar whose entry climbs out with "..", written by hand.
        let mut header = [0u8; 512];
        let name = b"../escaped.txt";
        header[..name.len()].copy_from_slice(name);
        header[100..107].copy_from_slice(b"0000644");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        header[124..135].copy_from_slice(b"00000000002");
        header[136..147].copy_from_slice(b"00000000000");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&b| b as u32).sum();
        header[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        let mut tar = header.to_vec();
        let mut data = [0u8; 512];
        data[..2].copy_from_slice(b"hi");
        tar.extend_from_slice(&data);
        tar.extend_from_slice(&[0u8; 1024]);
        let path = dir.path().join("evil.tar");
        std::fs::write(&path, tar).unwrap();
        let out = dir.path().join("inside");
        std::fs::create_dir(&out).unwrap();
        let _ = extract(&caps, &path, &out, &AtomicBool::new(false), &mut |_| {});
        assert!(!dir.path().join("escaped.txt").exists());
    }
}
