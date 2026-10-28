//! The person's own Shortcuts (the macOS app), run on files through
//! macOS's `shortcuts` tool. Each file goes in as the shortcut's input;
//! whatever the shortcut returns is saved beside it, never over it.
//!
//! The person chooses which shortcuts appear on the Control wheel, and
//! AI agents can't run them: a shortcut can do anything its author made
//! it do.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::catalog::Capabilities;
use crate::error::{Error, Result};
use crate::format;

/// A shortcut can wait on the person (a question, a permission), so it
/// gets as long as an app export.
pub const TIMEOUT: Duration = Duration::from_secs(3600);

/// Longest name accepted; the Shortcuts app's own limit is lower.
pub const MAX_NAME: usize = 200;

/// Rejects names that can't be a shortcut's, before anything runs.
pub fn check_name(name: &str) -> Result<&str> {
    let trimmed = name.trim();
    if trimmed.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(Error::Invalid("that isn't a shortcut's name".into()));
    }
    Ok(name)
}

fn tool(caps: &Capabilities) -> Result<&Path> {
    caps.shortcuts
        .as_deref()
        .ok_or_else(|| Error::Missing("running shortcuts needs macOS 12 or later".into()))
}

/// The person's shortcuts, by name, as the Shortcuts app lists them.
pub fn list(caps: &Capabilities) -> Result<Vec<String>> {
    let out = Command::new(tool(caps)?)
        .arg("list")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(Error::Helper("the Shortcuts list couldn't be read".into()));
    }
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && check_name(l).is_ok())
        .map(String::from)
        .collect();
    names.dedup();
    Ok(names)
}

/// Runs shortcut `name` with `input`, writing what it returns into
/// `scratch`. Returns those files, ready to publish: none when the
/// shortcut returns nothing (it might have shared or sent the file).
pub fn run(
    caps: &Capabilities,
    name: &str,
    input: &Path,
    scratch: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<PathBuf>> {
    let name = check_name(name)?;
    if !input.is_file() {
        return Err(Error::UnsupportedInput(
            "shortcuts run on files, not folders".into(),
        ));
    }
    let input = std::path::absolute(input)?;
    let out = scratch.join("output");
    let mut child = Command::new(tool(caps)?)
        .arg("run")
        .arg(name)
        .arg("--input-path")
        .arg(&input)
        .arg("--output-path")
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = crate::engines::media::spawn_tail_reader(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Timeout(TIMEOUT.as_secs()));
        }
        match child.try_wait()? {
            Some(s) => break s,
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    if !status.success() {
        let tail = stderr.join().unwrap_or_default();
        let why = tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        let lower = why.to_lowercase();
        return Err(
            if lower.contains("couldn't find")
                || lower.contains("not found")
                || lower.contains("no shortcut")
            {
                Error::Missing(format!(
                    "there's no shortcut called \u{201C}{name}\u{201D} any more"
                ))
            } else if why.is_empty() {
                Error::Helper(format!("\u{201C}{name}\u{201D} stopped without saying why"))
            } else {
                Error::Helper(format!("\u{201C}{name}\u{201D} stopped: {why}"))
            },
        );
    }
    // One result is a file; several may come as a folder of them.
    let mut found = Vec::new();
    if out.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&out)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        entries.sort();
        found.extend(entries);
    } else if out.is_file() {
        found.push(out);
    }
    Ok(found
        .into_iter()
        .filter(|p| crate::naming::size_of(p) > 0)
        .collect())
}

/// The extension a shortcut's result should have, from what's in it: its
/// own extension if it came with one, else its detected format, else text
/// or plain data.
pub fn extension_for(result: &Path) -> String {
    if let Some(ext) = result
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && e.len() <= 12 && e.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return ext.to_ascii_lowercase();
    }
    match format::detect(result) {
        Ok(f) if f != format::Format::Folder => f.extension().to_string(),
        _ => {
            let looks_like_text = std::fs::read(result)
                .map(|b| std::str::from_utf8(&b[..b.len().min(64 * 1024)]).is_ok())
                .unwrap_or(false);
            if looks_like_text { "txt" } else { "data" }.to_string()
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in for macOS's `shortcuts`: lists two shortcuts; "Copy"
    /// returns the input, "Pair" returns two files, "Quiet" returns
    /// nothing, and anything else isn't found.
    pub(crate) fn fake_tool(dir: &Path) -> PathBuf {
        let path = dir.join("shortcuts");
        std::fs::write(
            &path,
            r#"#!/bin/sh
if [ "$1" = list ]; then printf 'Copy\nPair\nQuiet\n'; exit 0; fi
name="$2"; input="$4"; output="$6"
case "$name" in
  Copy) cp "$input" "$output" ;;
  Pair) mkdir -p "$output"; printf 'one' > "$output/a.txt"; printf 'two' > "$output/b.txt" ;;
  Quiet) ;;
  Slow) sleep 30 ;;
  *) echo "Error: Couldn't find shortcut" >&2; exit 1 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn caps(dir: &Path) -> Capabilities {
        Capabilities {
            shortcuts: Some(fake_tool(dir)),
            ..Capabilities::none()
        }
    }

    #[test]
    fn lists_and_runs_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let caps = caps(dir.path());
        assert_eq!(list(&caps).unwrap(), ["Copy", "Pair", "Quiet"]);
        let input = dir.path().join("note.txt");
        std::fs::write(&input, "hello").unwrap();
        let never = AtomicBool::new(false);

        let scratch = tempfile::tempdir().unwrap();
        let got = run(&caps, "Copy", &input, scratch.path(), &never).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read_to_string(&got[0]).unwrap(), "hello");
        assert_eq!(extension_for(&got[0]), "txt");

        let scratch = tempfile::tempdir().unwrap();
        let got = run(&caps, "Pair", &input, scratch.path(), &never).unwrap();
        assert_eq!(got.len(), 2);

        let scratch = tempfile::tempdir().unwrap();
        assert!(run(&caps, "Quiet", &input, scratch.path(), &never)
            .unwrap()
            .is_empty());

        let scratch = tempfile::tempdir().unwrap();
        let gone = run(&caps, "Gone", &input, scratch.path(), &never).unwrap_err();
        assert!(gone.to_string().contains("no shortcut called"), "{gone}");
    }

    #[test]
    fn bad_names_and_folders_never_run() {
        let dir = tempfile::tempdir().unwrap();
        let caps = caps(dir.path());
        let never = AtomicBool::new(false);
        let input = dir.path().join("note.txt");
        std::fs::write(&input, "hello").unwrap();
        for bad in ["", "  ", "two\nlines", &"x".repeat(MAX_NAME + 1)] {
            assert!(
                run(&caps, bad, &input, dir.path(), &never).is_err(),
                "{bad:?}"
            );
        }
        assert!(matches!(
            run(&caps, "Copy", dir.path(), dir.path(), &never),
            Err(Error::UnsupportedInput(_))
        ));
        assert!(matches!(
            run(&Capabilities::none(), "Copy", &input, dir.path(), &never),
            Err(Error::Missing(_))
        ));
    }

    #[test]
    fn cancelling_stops_a_shortcut() {
        let dir = tempfile::tempdir().unwrap();
        let caps = caps(dir.path());
        let input = dir.path().join("note.txt");
        std::fs::write(&input, "hello").unwrap();
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        assert!(matches!(
            run(&caps, "Slow", &input, dir.path(), &stop),
            Err(Error::Cancelled)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn results_get_a_sensible_extension() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("output");
        image::RgbImage::new(2, 2)
            .save_with_format(&png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(extension_for(&png), "png");
        let named = dir.path().join("Report.PDF");
        std::fs::write(&named, "%PDF-1.4").unwrap();
        assert_eq!(extension_for(&named), "pdf");
        let blob = dir.path().join("blob");
        std::fs::write(&blob, [0xff, 0xfe, 0x00, 0x81]).unwrap();
        assert_eq!(extension_for(&blob), "data");
    }
}
