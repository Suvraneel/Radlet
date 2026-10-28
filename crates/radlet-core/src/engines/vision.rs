//! Bridge to `radlet-vision`, the Mac app's small Swift helper for on-device
//! text recognition (Apple's Vision framework), subject masks and
//! searchable PDFs. It ships next to the `radlet` binary in the app bundle;
//! other platforms simply don't offer these tools.
//!
//! The helper's contract:
//!
//! ```text
//! radlet-vision text <input> <output.txt>        image or PDF; PDF pages that
//!                                              already have text use it
//! radlet-vision mask <input> <output.png>        8-bit grey foreground mask,
//!                                              the size of the upright image
//! radlet-vision searchable <input> <output.pdf>  image or PDF, with an
//!                                              invisible text layer
//! radlet-vision transcribe <language|auto> <output.json> <chunk.wav>...
//!                                              speech to timed words
//! radlet-vision document-pdf <input.html> <output.pdf>
//!                                              a document (as HTML Radlet
//!                                              wrote) laid out on A4 pages
//! radlet-vision faces <input> <output.json>      faces as [{"x","y","w","h"}],
//!                                              fractions from the top left
//! radlet-vision translate <to> <from|auto> <input.json> <output.json>
//!                                              a JSON list of strings,
//!                                              translated (macOS 26)
//! radlet-vision codes <input> <output.json>      barcodes and QR codes as
//!                                              [{"kind","value"}]
//! radlet-vision media <input> <output> <to> [options]
//!                                              audio and video with
//!                                              AVFoundation (see avmedia.rs)
//! ```
//!
//! `transcribe` writes `{"locale": "en-US", "chunks": [[{"t": "Hello,",
//! "s": 0.12, "d": 0.4}, ...], ...]}`: one list of words per chunk, with
//! start times and durations in seconds from the chunk's start.
//!
//! Exit 0 means done, 3 means nothing was found (no text, no subject), and
//! anything else is a failure. The last line on stderr explains. Lines of
//! the form `progress 0.42` on stdout report progress.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::catalog::Capabilities;
use crate::engines::media::spawn_tail_reader;
use crate::error::{Error, Result};

/// Exit status meaning "ran fine, found nothing".
const NOTHING_FOUND: i32 = 3;
/// The helper's exit code when something must be downloaded first; stderr
/// then has a line "need <what>" (see [`crate::error::Need::from_id`]).
const NEEDS: i32 = 4;

/// Recognised text of an image or PDF, ready to save or copy.
pub fn text(
    caps: &Capabilities,
    input: &Path,
    scratch: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<String> {
    let out = tempfile::Builder::new()
        .prefix("text-")
        .suffix(".txt")
        .tempfile_in(scratch)?
        .into_temp_path();
    run(caps, "text", input, &out, cancel, progress)?;
    let text = std::fs::read_to_string(&out)?;
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Err(Error::NothingToDo("no text found".into()));
    }
    Ok(format!("{text}\n"))
}

/// Writes a foreground mask for `input` (white = keep) to `out`.
pub fn mask(caps: &Capabilities, input: &Path, out: &Path, cancel: &AtomicBool) -> Result<()> {
    run(caps, "mask", input, out, cancel, &mut |_| {})
}

/// Writes `input` (an image or PDF) as a PDF whose text can be searched,
/// selected and copied.
pub fn searchable(
    caps: &Capabilities,
    input: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    run(caps, "searchable", input, out, cancel, progress)
}

/// Photos of paper pages as one scanned PDF: each page found in its photo,
/// squared up, evened out, with a text layer.
pub fn scan(
    caps: &Capabilities,
    photos: &[std::path::PathBuf],
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    std::fs::File::create(out)?;
    let mut args: Vec<&OsStr> = vec![OsStr::new("scan"), out.as_os_str()];
    args.extend(photos.iter().map(|p| p.as_os_str()));
    run_args(caps, &args, out, cancel, progress)
}

/// What to black out of a PDF's text.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct RedactTerms {
    pub words: Vec<String>,
    /// Any of "emails", "phones", "cards".
    pub kinds: Vec<&'static str>,
}

/// A copy of a PDF with matching text removed and boxed over: pages with
/// a match are redrawn as pictures with a fresh text layer; the rest stay
/// as they were.
pub fn redact_pdf(
    caps: &Capabilities,
    input: &Path,
    terms: &RedactTerms,
    scratch: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let spec = scratch.join("terms.json");
    std::fs::write(
        &spec,
        serde_json::to_vec(terms).map_err(|e| Error::Internal(e.to_string()))?,
    )?;
    std::fs::File::create(out)?;
    let args = [
        OsStr::new("redact-pdf"),
        input.as_os_str(),
        out.as_os_str(),
        spec.as_os_str(),
    ];
    run_args(caps, &args, out, cancel, progress)
}

/// Lays out a document, written by Radlet as plain HTML, on A4 pages with
/// the Mac's text engine, which has fonts for every script.
pub fn document_pdf(
    caps: &Capabilities,
    html: &Path,
    out: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    run(caps, "document-pdf", html, out, cancel, &mut |_| {})
}

/// Finds faces in an upright picture: rectangles as fractions of its
/// width and height, from the top left.
pub fn faces(
    caps: &Capabilities,
    input: &Path,
    scratch: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<crate::ops::Rect>> {
    let out = scratch.join("faces.json");
    std::fs::File::create(&out)?;
    run(caps, "faces", input, &out, cancel, &mut |_| {})?;
    let rects: Vec<crate::ops::Rect> = serde_json::from_slice(&std::fs::read(&out)?)
        .map_err(|e| Error::Helper(format!("the Vision helper's faces don't read ({e})")))?;
    Ok(rects)
}

/// A barcode or QR code the helper found.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Code {
    pub kind: String,
    pub value: String,
}

/// Barcodes and QR codes in a picture, top to bottom.
pub fn codes(
    caps: &Capabilities,
    input: &Path,
    scratch: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<Code>> {
    let out = scratch.join("codes.json");
    std::fs::File::create(&out)?;
    run(caps, "codes", input, &out, cancel, &mut |_| {})?;
    serde_json::from_slice(&std::fs::read(&out)?)
        .map_err(|e| Error::Helper(format!("the Vision helper's codes don't read ({e})")))
}

/// Translates strings into `to` (from `from`, or the detected language)
/// with the Mac's on-device translation.
pub fn translate(
    caps: &Capabilities,
    texts: &[String],
    to: &str,
    from: Option<&str>,
    scratch: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<String>> {
    let input = scratch.join("translate-in.json");
    let out = scratch.join("translate-out.json");
    std::fs::write(
        &input,
        serde_json::to_vec(texts).map_err(|e| Error::Internal(e.to_string()))?,
    )?;
    std::fs::File::create(&out)?;
    let args = [
        OsStr::new("translate"),
        OsStr::new(to),
        OsStr::new(from.unwrap_or("auto")),
        input.as_os_str(),
        out.as_os_str(),
    ];
    run_args(caps, &args, &out, cancel, progress)?;
    let back: Vec<String> = serde_json::from_slice(&std::fs::read(&out)?)
        .map_err(|e| Error::Helper(format!("the translation doesn't read ({e})")))?;
    if back.len() != texts.len() {
        return Err(Error::Helper("the translation came back incomplete".into()));
    }
    Ok(back)
}

/// Runs the helper's speech recognition on WAV chunks and writes its
/// JSON answer to `out`.
pub fn transcribe(
    caps: &Capabilities,
    language: Option<&str>,
    out: &Path,
    chunks: &[std::path::PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("transcribe"),
        OsStr::new(language.unwrap_or("auto")),
        out.as_os_str(),
    ];
    args.extend(chunks.iter().map(|c| c.as_os_str()));
    run_args(caps, &args, out, cancel, progress)
}

fn run(
    caps: &Capabilities,
    command: &str,
    input: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    run_args(
        caps,
        &[OsStr::new(command), input.as_os_str(), out.as_os_str()],
        out,
        cancel,
        progress,
    )
}

pub(crate) fn run_args(
    caps: &Capabilities,
    args: &[&OsStr],
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let helper = caps.vision.as_ref().ok_or_else(|| {
        Error::Missing(
            "text recognition, background removal and subtitles need the Radlet app on macOS"
                .into(),
        )
    })?;
    let mut child = Command::new(helper)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Helper(format!("couldn't start the Vision helper: {e}")))?;
    let stderr_tail = spawn_tail_reader(child.stderr.take());
    let (tx, rx) = mpsc::channel::<f32>();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some(Ok(p)) = line
                    .strip_prefix("progress ")
                    .map(|v| v.trim().parse::<f32>())
                {
                    let _ = tx.send(p.clamp(0.0, 1.0));
                }
            }
        });
    }
    let status = loop {
        while let Ok(p) = rx.try_recv() {
            progress(p);
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        match child.try_wait()? {
            Some(status) => break status,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let tail = move || stderr_tail.join().unwrap_or_default();
    let last_line = |t: &str| {
        t.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
    };
    match status.code() {
        // Outputs are created empty beforehand, so check there's something.
        Some(0) if std::fs::metadata(out).is_ok_and(|m| m.len() > 0) => {
            progress(1.0);
            Ok(())
        }
        Some(0) => Err(Error::Helper("the Vision helper wrote nothing".into())),
        Some(NOTHING_FOUND) => Err(Error::NothingToDo(
            last_line(&tail()).unwrap_or_else(|| "nothing found".into()),
        )),
        // Something to download first: a line "need <what>", then why.
        Some(NEEDS) => {
            let tail = tail();
            let mut lines = tail.lines().map(str::trim).filter(|l| !l.is_empty());
            let need = lines
                .clone()
                .find_map(|l| l.strip_prefix("need "))
                .and_then(crate::error::Need::from_id);
            let why = lines
                .rfind(|l| !l.starts_with("need "))
                .unwrap_or("something needs downloading first")
                .to_string();
            Err(match need {
                Some(need) => Error::Needs { need, message: why },
                None => Error::Helper(why),
            })
        }
        _ => Err(Error::Helper(last_line(&tail()).unwrap_or_else(|| {
            format!("the Vision helper stopped ({status})")
        }))),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn helper(dir: &Path, body: &str) -> Capabilities {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("radlet-vision");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Capabilities {
            vision: Some(script),
            ..Default::default()
        }
    }

    #[test]
    fn downloads_come_back_as_needs() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let cancel = AtomicBool::new(false);
        let caps = helper(
            dir.path(),
            "echo 'need speech_language' >&2\necho 'Turn on Dictation for French.' >&2\nexit 4",
        );
        let err = run_args(&caps, &[OsStr::new("x")], &out, &cancel, &mut |_| {}).unwrap_err();
        assert_eq!(err.need(), Some(crate::error::Need::SpeechLanguage));
        assert_eq!(err.to_string(), "Turn on Dictation for French.");
        assert_eq!(err.info().code, "needs");
        // An unknown need is still a plain failure.
        let caps = helper(dir.path(), "echo 'need moon' >&2\necho 'odd' >&2\nexit 4");
        let err = run_args(&caps, &[OsStr::new("x")], &out, &cancel, &mut |_| {}).unwrap_err();
        assert_eq!(err.need(), None);
        assert_eq!(err.to_string(), "odd");
    }
}

/// What the helper made of a picture: a short title (from Apple
/// Intelligence when it's on, else from Vision's labels and any text in
/// it), and, when asked for, a description.
#[derive(Debug, Default, serde::Deserialize)]
pub struct Described {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Looks at a picture on-device. With `alt`, also writes alt text, which
/// needs Apple Intelligence (an error with `need` otherwise).
pub fn describe(
    caps: &Capabilities,
    input: &Path,
    alt: bool,
    cancel: &AtomicBool,
) -> Result<Described> {
    let out = tempfile::Builder::new().suffix(".json").tempfile()?;
    run_args(
        caps,
        &[
            OsStr::new("describe"),
            OsStr::new(if alt { "alt" } else { "title" }),
            input.as_os_str(),
            out.path().as_os_str(),
        ],
        out.path(),
        cancel,
        &mut |_| {},
    )?;
    serde_json::from_slice(&std::fs::read(out.path())?)
        .map_err(|e| Error::Helper(format!("the Vision helper's answer couldn't be read ({e})")))
}

/// Has Apple Intelligence write `task` ("title" or "summary") for `text`,
/// on-device.
pub fn write(
    caps: &Capabilities,
    task: &str,
    text: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<String> {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("input.txt");
    let out = dir.path().join("output.txt");
    std::fs::write(&input, text)?;
    run_args(
        caps,
        &[
            OsStr::new("write"),
            OsStr::new(task),
            input.as_os_str(),
            out.as_os_str(),
        ],
        &out,
        cancel,
        progress,
    )?;
    Ok(std::fs::read_to_string(&out)?.trim().to_string())
}
