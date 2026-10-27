//! Audio and video through macOS's own AVFoundation, for Macs without
//! FFmpeg. The `radlet-vision` helper does the work (its Media.swift):
//!
//! ```text
//! radlet-vision media <input> <output> <to> [start=<s>] [end=<s>] [size=<WxH>]
//!                     [target=<bytes>] [reencode] [frames]
//! radlet-vision join <output> <to> <input>...
//! radlet-vision burn <video> <cues.json> <output> <to>
//! ```
//!
//! It reads what Macs make and play (MP4 and MOV video; M4A, MP3, WAV,
//! AIFF and FLAC sound; GIF frames) and writes MP4, MOV, M4A, WAV and AIFF.
//! Without options, MP4 ↔ MOV keeps the streams as they are when it can.
//! Exit 3 means nothing to do (no sound to keep, or a compressed copy that
//! came out no smaller). FFmpeg, when it's installed, is always used
//! instead: it does more, and the same way on every Mac.

use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::catalog::Capabilities;
use crate::engines::media::MediaJob;
use crate::engines::vision;
use crate::error::{Error, Result};
use crate::format::Format;

use Format::*;

/// Video AVFoundation reads.
const VIDEO_IN: &[Format] = &[Mp4, Mov];
/// Sound AVFoundation reads.
const AUDIO_IN: &[Format] = &[M4a, Mp3, Wav, Aiff, Flac];
/// What it writes.
const WRITES: &[Format] = &[Mp4, Mov, M4a, Wav, Aiff];

const VIDEO_TARGETS: &[Format] = &[Mp4, Mov, M4a, Wav, Aiff];
const AUDIO_TARGETS: &[Format] = &[M4a, Wav, Aiff];

/// Whether AVFoundation opens this format.
pub fn reads(f: Format) -> bool {
    VIDEO_IN.contains(&f) || AUDIO_IN.contains(&f)
}

/// Conversions offered for `from`, most useful first.
pub fn targets(from: Format) -> &'static [Format] {
    if VIDEO_IN.contains(&from) {
        VIDEO_TARGETS
    } else if AUDIO_IN.contains(&from) {
        AUDIO_TARGETS
    } else {
        &[]
    }
}

/// A part of a file can be kept in its own format.
pub fn trims(f: Format) -> bool {
    reads(f) && WRITES.contains(&f)
}

/// Video that can be compressed (always into MP4).
pub fn compresses(f: Format) -> bool {
    VIDEO_IN.contains(&f)
}

/// A GIF's frames become MP4 or MOV video.
pub const FROM_GIF: &[Format] = &[Mp4, Mov];

/// Whether the helper can do this job for a file in `from`.
pub fn supports(job: &MediaJob, from: Format) -> bool {
    match job {
        MediaJob::Convert(to) if from == Gif => FROM_GIF.contains(to),
        MediaJob::Convert(to) | MediaJob::Reencode(to) => {
            reads(from) && (targets(from).contains(to) || (*to == from && WRITES.contains(to)))
        }
        MediaJob::Trim { .. } => trims(from),
        MediaJob::CompressVideo(_) | MediaJob::CompressVideoTarget(_) => compresses(from),
        _ => false,
    }
}

/// The export preset for a compression level, by its H.264 quality: the
/// lighter the compression, the larger the picture it keeps.
fn size_for(crf: u8) -> &'static str {
    match crf {
        0..=23 => "1920x1080",
        24..=28 => "1280x720",
        _ => "960x540",
    }
}

/// The helper's arguments after `media <input> <output>`.
fn plan(job: &MediaJob, from: Format) -> Result<(Vec<String>, Format)> {
    let (to, extra): (Format, Vec<String>) = match job {
        MediaJob::Convert(to) if from == Gif => (*to, vec!["frames".into()]),
        MediaJob::Convert(to) => (*to, vec![]),
        MediaJob::Reencode(to) => (*to, vec!["reencode".into()]),
        MediaJob::Trim { start, end } => {
            let mut v = vec![format!("start={start:.3}")];
            if let Some(end) = end {
                if end <= start {
                    return Err(Error::Invalid("the end comes before the start".into()));
                }
                v.push(format!("end={end:.3}"));
            }
            (from, v)
        }
        MediaJob::CompressVideo(crf) => (Mp4, vec![format!("size={}", size_for(*crf))]),
        MediaJob::CompressVideoTarget(bytes) => (Mp4, vec![format!("target={bytes}")]),
        _ => return Err(Error::FfmpegMissing),
    };
    let mut args = vec![to.id().to_string()];
    args.extend(extra);
    Ok((args, to))
}

/// Runs a job through the helper, like `media::run` does through FFmpeg.
pub fn run(
    caps: &Capabilities,
    job: MediaJob,
    input: &Path,
    from: Format,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Format> {
    if !caps.avfoundation || caps.vision.is_none() || !supports(&job, from) {
        return Err(Error::FfmpegMissing);
    }
    let (words, to) = plan(&job, from)?;
    // The helper's contract: outputs exist (empty) beforehand, and a run that
    // leaves one empty failed.
    std::fs::File::create(out)?;
    let mut args: Vec<OsString> = vec![
        "media".into(),
        input.as_os_str().to_owned(),
        out.as_os_str().to_owned(),
    ];
    args.extend(words.into_iter().map(OsString::from));
    let refs: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
    vision::run_args(caps, &refs, out, cancel, progress)?;
    Ok(to)
}

fn helper_ready(caps: &Capabilities) -> Result<()> {
    if caps.avfoundation && caps.vision.is_some() {
        Ok(())
    } else {
        Err(Error::FfmpegMissing)
    }
}

/// Joins clips one after another through the helper: MP4 or MOV video
/// (each picture fitted into the first's shape), or M4A sound.
pub fn join(
    caps: &Capabilities,
    inputs: &[std::path::PathBuf],
    to: Format,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    helper_ready(caps)?;
    std::fs::File::create(out)?;
    let mut args: Vec<OsString> = vec!["join".into(), out.as_os_str().to_owned(), to.id().into()];
    args.extend(inputs.iter().map(|p| p.as_os_str().to_owned()));
    let refs: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
    vision::run_args(caps, &refs, out, cancel, progress)
}

/// Draws subtitles (`cues`, a JSON list of {"s", "e", "t"}) into a video's
/// picture through the helper.
pub fn burn(
    caps: &Capabilities,
    input: &Path,
    cues: &Path,
    to: Format,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    helper_ready(caps)?;
    std::fs::File::create(out)?;
    let args: Vec<&std::ffi::OsStr> = vec![
        "burn".as_ref(),
        input.as_os_str(),
        cues.as_os_str(),
        out.as_os_str(),
        to.id().as_ref(),
    ];
    vision::run_args(caps, &args, out, cancel, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_mac_handles_without_ffmpeg() {
        assert!(supports(&MediaJob::Convert(Mp4), Mov));
        assert!(supports(&MediaJob::Convert(M4a), Mp4), "sound from video");
        assert!(supports(&MediaJob::Convert(Wav), Flac));
        assert!(!supports(&MediaJob::Convert(Mp3), Wav), "no MP3 encoder");
        assert!(!supports(&MediaJob::Convert(Mp4), Mkv), "no Matroska");
        assert!(
            !supports(&MediaJob::Convert(Mov), M4a),
            "no picture to show"
        );
        assert!(supports(
            &MediaJob::Trim {
                start: 1.0,
                end: None
            },
            Wav
        ));
        assert!(!supports(
            &MediaJob::Trim {
                start: 1.0,
                end: None
            },
            Mp3
        ));
        assert!(supports(&MediaJob::CompressVideo(27), Mov));
        assert!(supports(&MediaJob::CompressVideoTarget(10_000_000), Mp4));
        assert!(supports(&MediaJob::Convert(Mp4), Gif), "a GIF's frames");
        assert!(!supports(&MediaJob::Convert(M4a), Gif), "no sound in a GIF");
        assert!(!supports(&MediaJob::CompressVideo(27), Webm));
        assert!(
            !supports(&MediaJob::Normalize, Wav),
            "loudness needs FFmpeg"
        );
    }

    #[test]
    fn helper_arguments() {
        let (a, to) = plan(&MediaJob::Convert(M4a), Mov).unwrap();
        assert_eq!((a, to), (vec!["m4a".to_string()], M4a));
        let (a, to) = plan(
            &MediaJob::Trim {
                start: 1.5,
                end: Some(4.0),
            },
            Mp4,
        )
        .unwrap();
        assert_eq!(a, ["mp4", "start=1.500", "end=4.000"]);
        assert_eq!(to, Mp4);
        let (a, to) = plan(&MediaJob::CompressVideo(23), Mov).unwrap();
        assert_eq!((a, to), (vec!["mp4".into(), "size=1920x1080".into()], Mp4));
        assert_eq!(size_for(27), "1280x720");
        let (a, to) = plan(&MediaJob::CompressVideoTarget(19_000_000), Mov).unwrap();
        assert_eq!((a, to), (vec!["mp4".into(), "target=19000000".into()], Mp4));
        let (a, _) = plan(&MediaJob::Convert(Mov), Gif).unwrap();
        assert_eq!(a, ["mov", "frames"]);
        assert_eq!(size_for(31), "960x540");
        assert!(plan(
            &MediaJob::Trim {
                start: 5.0,
                end: Some(2.0)
            },
            Wav
        )
        .is_err());
    }
}
