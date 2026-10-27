//! Keynote, Pages and Numbers documents, exported by the apps themselves
//! (when they're installed) through AppleScript: Pages to PDF, Word, EPUB,
//! RTF and text; Numbers to PDF and Excel; Keynote to PDF and PowerPoint.
//! PowerPoint files go to PDF through Keynote too, so their slides look as
//! they were made. The other text formats come from those exports through
//! the usual document converters.
//!
//! Paths reach the script as arguments, never inside its text, so no file
//! name can change what it does. An app that wasn't running is quit again
//! afterwards.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::catalog::Capabilities;
use crate::engines::doc;
use crate::error::{Error, Result};
use crate::format::Format;

use Format::*;

/// Longest an export may take.
const TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    Keynote,
    Pages,
    Numbers,
}

impl App {
    fn bundle_id(self) -> &'static str {
        match self {
            App::Keynote => "com.apple.iWork.Keynote",
            App::Pages => "com.apple.iWork.Pages",
            App::Numbers => "com.apple.iWork.Numbers",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            App::Keynote => "Keynote",
            App::Pages => "Pages",
            App::Numbers => "Numbers",
        }
    }

    /// Whether this Mac has the app, in /Applications or ~/Applications.
    pub fn installed(self) -> bool {
        if !cfg!(target_os = "macos") {
            return false;
        }
        let bundle = format!("{}.app", self.name());
        let mut places = vec![Path::new("/Applications").join(&bundle)];
        if let Some(home) = std::env::var_os("HOME") {
            places.push(Path::new(&home).join("Applications").join(&bundle));
        }
        places
            .iter()
            .any(|p| p.join("Contents/Info.plist").is_file())
    }

    /// What the app writes itself, and the AppleScript name of each.
    fn exports(self) -> &'static [(Format, &'static str)] {
        match self {
            App::Pages => &[
                (Pdf, "PDF"),
                (Docx, "Microsoft Word"),
                (Epub, "EPUB"),
                (Rtf, "formatted text"),
                (Txt, "unformatted text"),
            ],
            App::Numbers => &[(Pdf, "PDF"), (Xlsx, "Microsoft Excel")],
            App::Keynote => &[(Pdf, "PDF"), (Pptx, "Microsoft PowerPoint")],
        }
    }
}

/// The app that opens `f` (PowerPoint opens in Keynote).
pub fn app_for(f: Format) -> Option<App> {
    match f {
        Key | Pptx => Some(App::Keynote),
        Pages => Some(App::Pages),
        Numbers => Some(App::Numbers),
        _ => None,
    }
}

fn available(caps: &Capabilities, app: App) -> bool {
    match app {
        App::Keynote => caps.keynote,
        App::Pages => caps.pages,
        App::Numbers => caps.numbers,
    }
}

/// Conversions offered for an iWork document on this Mac, most useful
/// first: what the app exports, then what those exports become.
pub fn targets(caps: &Capabilities, from: Format) -> Vec<Format> {
    let Some(app) = app_for(from).filter(|a| available(caps, *a)) else {
        return Vec::new();
    };
    match from {
        Pages => vec![Pdf, Docx, Md, Txt, Html, Rtf, Epub],
        Numbers => vec![Pdf, Xlsx, Csv, Json],
        Key => vec![Pdf, Pptx, Docx, Md, Txt, Html],
        // PowerPoint's own reader covers its text; Keynote adds the look.
        Pptx if app == App::Keynote => vec![Pdf, Key],
        _ => Vec::new(),
    }
}

/// Whether this conversion goes through an iWork app here.
pub fn handles(caps: &Capabilities, from: Format, to: Format) -> bool {
    targets(caps, from).contains(&to)
}

/// The AppleScript that opens argument 1 in `app` and exports it to
/// argument 2 as `kind` (or, for Keynote's own format, saves it).
fn script(app: App, kind: Option<&str>) -> String {
    let id = app.bundle_id();
    let write = match kind {
        Some(kind) => format!("export d to outFile as {kind}"),
        None => "save d in outFile".to_string(),
    };
    format!(
        r#"on run argv
    set inFile to POSIX file (item 1 of argv)
    set outFile to POSIX file (item 2 of argv)
    set wasRunning to running of application id "{id}"
    with timeout of 600 seconds
        tell application id "{id}"
            set countBefore to count of documents
            set d to open inFile
            -- A document that was open already (perhaps with edits) is
            -- left open; only one opened here is closed.
            set openedHere to (count of documents) > countBefore
            try
                {write}
            on error message number n
                if openedHere then close d saving no
                if not wasRunning and (count of documents) is 0 then quit
                error message number n
            end try
            if openedHere then close d saving no
            if not wasRunning and (count of documents) is 0 then quit
        end tell
    end timeout
end run"#
    )
}

/// Holds a lock that every Radlet process shares, so exports run one at a
/// time: two at once would open, close and quit the app under each other.
struct ExportLock(#[allow(dead_code)] std::fs::File);

impl ExportLock {
    fn take(cancel: &AtomicBool) -> Result<ExportLock> {
        let path = std::env::temp_dir().join("radlet-iwork.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let started = Instant::now();
            // SAFETY: flock on a descriptor this function owns; the lock
            // goes with the file when it's dropped.
            while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                if started.elapsed() > TIMEOUT {
                    return Err(Error::Timeout(TIMEOUT.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        Ok(ExportLock(file))
    }
}

/// Runs osascript, one export at a time, waiting at most [`TIMEOUT`] and
/// stopping when cancelled.
fn run_script(text: &str, input: &Path, out: &Path, cancel: &AtomicBool) -> Result<()> {
    let osascript = Path::new("/usr/bin/osascript");
    if !osascript.is_file() {
        return Err(Error::Missing("iWork documents need macOS".into()));
    }
    // POSIX file reads a relative path from the root of the disk.
    let input = std::path::absolute(input)?;
    let out = std::path::absolute(out)?;
    let _lock = ExportLock::take(cancel)?;
    let mut child = Command::new(osascript)
        .arg("-e")
        .arg(text)
        .arg(&input)
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
            .trim();
        // -1743: the user hasn't let Radlet control the app.
        return Err(if why.contains("-1743") || why.contains("Not authorized") {
            Error::Missing(
                "Radlet isn't allowed to use this app. Allow it in System Settings → Privacy & \
                 Security → Automation"
                    .into(),
            )
        } else {
            Error::Helper(format!("the export failed ({why})"))
        });
    }
    if !out.exists() {
        return Err(Error::Helper("the app exported nothing".into()));
    }
    Ok(())
}

/// Has the app write `input` as `to` at `out`.
fn export(app: App, input: &Path, to: Format, out: &Path, cancel: &AtomicBool) -> Result<()> {
    let kind = if app == App::Keynote && to == Key {
        None
    } else {
        Some(
            app.exports()
                .iter()
                .find(|(f, _)| *f == to)
                .map(|(_, k)| *k)
                .ok_or_else(|| Error::UnsupportedRoute {
                    from: app.name().into(),
                    to: to.label().into(),
                })?,
        )
    };
    // The apps add the extension a format expects, so they're given a
    // name that has it.
    let named = out.with_extension(to.extension());
    let _ = std::fs::remove_file(&named);
    run_script(&script(app, kind), input, &named, cancel)?;
    if named != out {
        std::fs::rename(&named, out)?;
    }
    Ok(())
}

/// Converts an iWork document (or a PowerPoint through Keynote) into `to`
/// at `out`, through the app's own export and, for other formats, a
/// document conversion of that. `scratch` holds the in-between file.
#[allow(clippy::too_many_arguments)]
pub fn convert(
    caps: &Capabilities,
    input: &Path,
    from: Format,
    to: Format,
    title: &str,
    scratch: &Path,
    out: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let app = app_for(from).ok_or_else(|| Error::UnsupportedRoute {
        from: from.label().into(),
        to: to.label().into(),
    })?;
    if !available(caps, app) {
        return Err(Error::Missing(format!(
            "{} files need {} on this Mac",
            from.label(),
            app.name()
        )));
    }
    if to == Key || app.exports().iter().any(|(f, _)| *f == to) {
        return export(app, input, to, out, cancel);
    }
    // Through what the app writes that the document converters read.
    let via = match app {
        App::Pages => Docx,
        App::Keynote => Pptx,
        App::Numbers => Xlsx,
    };
    let middle = scratch.join(format!("export.{}", via.extension()));
    export(app, input, via, &middle, cancel)?;
    if via == Xlsx {
        crate::engines::data::convert(&middle, via, to, out)
    } else {
        let d = doc::read(&middle, via)?;
        doc::write(&d, to, title, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac() -> Capabilities {
        Capabilities {
            keynote: true,
            pages: true,
            numbers: true,
            ..Default::default()
        }
    }

    #[test]
    fn routes_follow_the_apps() {
        let caps = mac();
        assert_eq!(targets(&caps, Pages)[0], Pdf);
        assert!(handles(&caps, Numbers, Xlsx));
        assert!(handles(&caps, Key, Md), "through PowerPoint");
        assert!(
            handles(&caps, Pptx, Pdf),
            "PowerPoint looks right through Keynote"
        );
        assert!(
            !handles(&caps, Pptx, Md),
            "PowerPoint's text is read directly"
        );
        let none = Capabilities::none();
        assert!(targets(&none, Pages).is_empty());
        assert!(!handles(&none, Pptx, Pdf));
    }

    #[test]
    fn scripts_take_paths_as_arguments() {
        let s = script(App::Pages, Some("Microsoft Word"));
        assert!(s.contains(r#"tell application id "com.apple.iWork.Pages""#));
        assert!(s.contains("export d to outFile as Microsoft Word"));
        assert!(s.contains("item 1 of argv") && s.contains("item 2 of argv"));
        assert!(s.contains("if openedHere then close d saving no"));
        assert!(s.contains("if not wasRunning and (count of documents) is 0 then quit"));
        assert!(
            s.contains(r#"set wasRunning to running of application id "com.apple.iWork.Pages""#)
        );
        assert!(script(App::Keynote, None).contains("save d in outFile"));
    }
}
