//! Bridge to `veer-vision`, the Mac app's small Swift helper for on-device
//! text recognition (Apple's Vision framework), subject masks and
//! searchable PDFs. It ships next to the `veer` binary in the app bundle;
//! other platforms simply don't offer these tools.
//!
//! The helper's contract:
//!
//! ```text
//! veer-vision text <input> <output.txt>        image or PDF; PDF pages that
//!                                              already have text use it
//! veer-vision mask <input> <output.png>        8-bit grey foreground mask,
//!                                              the size of the upright image
//! veer-vision searchable <input> <output.pdf>  image or PDF, with an
//!                                              invisible text layer
//! veer-vision transcribe <language|auto> <output.json> <chunk.wav>...
//!                                              speech to timed words
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

fn run_args(
    caps: &Capabilities,
    args: &[&OsStr],
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let helper = caps.vision.as_ref().ok_or_else(|| {
        Error::Missing(
            "text recognition, background removal and subtitles need the Veer app on macOS".into(),
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
    let message = || {
        stderr_tail
            .join()
            .unwrap_or_default()
            .lines()
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
            message().unwrap_or_else(|| "nothing found".into()),
        )),
        _ => Err(Error::Helper(message().unwrap_or_else(|| {
            format!("the Vision helper stopped ({status})")
        }))),
    }
}
