//! Audio and video through FFmpeg, run as a separate program.
//!
//! Paths are passed with the `file:` protocol prefix so names such as
//! `concat:...` or `http:...` can't be interpreted as FFmpeg protocols.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use serde::Deserialize;

use crate::catalog::Capabilities;
use crate::error::{Error, Result};
use crate::format::{Category, Format};

/// Encoders compiled into the FFmpeg we found, listed once per process.
fn encoders(ffmpeg: &Path) -> &'static HashSet<String> {
    static ENCODERS: OnceLock<HashSet<String>> = OnceLock::new();
    ENCODERS.get_or_init(|| {
        let out = Command::new(ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .stderr(Stdio::null())
            .output();
        let text = out
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        text.lines()
            .filter_map(|l| {
                let mut parts = l.split_whitespace();
                let flags = parts.next()?;
                let name = parts.next()?;
                (flags.len() == 6 && (flags.starts_with('A') || flags.starts_with('V')))
                    .then(|| name.to_string())
            })
            .collect()
    })
}

fn pick(enc: &HashSet<String>, options: &[&'static str], what: &str) -> Result<&'static str> {
    options
        .iter()
        .copied()
        .find(|o| enc.contains(*o))
        .ok_or_else(|| Error::Ffmpeg(format!("this FFmpeg build has no {what} encoder")))
}

fn muxer(f: Format) -> &'static str {
    match f {
        Format::Mp3 => "mp3",
        Format::M4a => "ipod",
        Format::Wav => "wav",
        Format::Flac => "flac",
        Format::Ogg => "ogg",
        Format::Opus => "opus",
        Format::Aiff => "aiff",
        Format::Wma | Format::Wmv => "asf",
        Format::Mp4 => "mp4",
        Format::Mov => "mov",
        Format::Mkv => "matroska",
        Format::Webm => "webm",
        Format::Avi => "avi",
        Format::Gif => "gif",
        Format::Jpeg | Format::Png => "image2",
        _ => "null",
    }
}

/// Stream information from ffprobe, when available.
#[derive(Debug, Default, Clone)]
pub struct Probe {
    pub duration_s: Option<f64>,
    /// The main video stream's own length, when the container records it.
    /// It can be shorter than the file (audio that runs on).
    pub video_duration_s: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub has_video: bool,
}

#[derive(Deserialize)]
struct ProbeJson {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}
#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    duration: Option<String>,
    disposition: Option<ProbeDisposition>,
}
#[derive(Deserialize)]
struct ProbeDisposition {
    #[serde(default)]
    attached_pic: u8,
}
#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

pub fn probe(caps: &Capabilities, input: &Path) -> Probe {
    let Some(ffprobe) = &caps.ffprobe else {
        return Probe::default();
    };
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,duration:stream_disposition=attached_pic:format=duration",
            "-of",
            "json",
        ])
        .arg(file_url(input))
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else {
        return Probe::default();
    };
    let Ok(json) = serde_json::from_slice::<ProbeJson>(&out.stdout) else {
        return Probe::default();
    };
    let mut p = Probe {
        duration_s: json
            .format
            .and_then(|f| f.duration)
            .and_then(|d| d.parse().ok()),
        ..Default::default()
    };
    for s in json.streams {
        let cover_art = s.disposition.is_some_and(|d| d.attached_pic == 1);
        match s.codec_type.as_deref() {
            Some("video") if !cover_art && p.video_codec.is_none() => {
                p.has_video = true;
                p.video_codec = s.codec_name;
                p.video_duration_s = s
                    .duration
                    .and_then(|d| d.parse::<f64>().ok())
                    .filter(|d| d.is_finite() && *d > 0.0);
            }
            Some("audio") if p.audio_codec.is_none() => p.audio_codec = s.codec_name,
            _ => {}
        }
    }
    p
}

/// Every metadata tag in a media file (the container's, then each
/// stream's), as FFprobe reports them. None without FFprobe or for a file
/// it can't read.
pub fn probe_tags(caps: &Capabilities, input: &Path) -> Option<Vec<(String, String)>> {
    #[derive(Deserialize)]
    struct Tagged {
        #[serde(default)]
        tags: std::collections::BTreeMap<String, serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Tags {
        format: Option<Tagged>,
        #[serde(default)]
        streams: Vec<Tagged>,
    }
    let ffprobe = caps.ffprobe.as_ref()?;
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format_tags:stream_tags",
            "-of",
            "json",
        ])
        .arg(file_url(input))
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let parsed: Tags = serde_json::from_slice(&out.stdout).ok()?;
    let mut all = Vec::new();
    for t in parsed.format.into_iter().chain(parsed.streams) {
        for (k, v) in t.tags {
            let v = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            all.push((k, v));
        }
    }
    Some(all)
}

fn file_url(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    format!("file:{}", abs.display())
}

/// What a media job should produce.
#[derive(Debug, Clone)]
pub enum MediaJob {
    Convert(Format),
    /// Re-encode video at this H.264 quality (CRF) into MP4.
    CompressVideo(u8),
    /// Re-encode video into MP4 sized to about this many bytes.
    CompressVideoTarget(u64),
    /// Copy streams unchanged without global metadata and chapters.
    StripMetadata,
    /// Re-encode video through an FFmpeg filter chain (crop, redaction
    /// boxes) into `to`.
    Filter {
        vf: String,
        to: Format,
    },
    /// Keep `start` to `end` seconds, re-encoded so the cuts are exact.
    Trim {
        start: f64,
        end: Option<f64>,
    },
    /// Even out loudness (EBU R128, -16 LUFS); video is copied unchanged.
    Normalize,
    /// One still frame at `at` seconds, as JPEG or PNG.
    Frame {
        at: f64,
        to: Format,
    },
    /// Convert to `to`, always re-encoding (no stream copy).
    Reencode(Format),
}

/// Loudness target: -16 LUFS integrated, -1.5 dBTP peaks, as used for
/// podcasts and by streaming services.
const LOUDNORM: &str = "loudnorm=I=-16:TP=-1.5:LRA=11";

/// Encoder arguments for an audio format.
fn audio_codec_args(to: Format, enc: &HashSet<String>) -> Result<Vec<String>> {
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Ok(match to {
        Format::Mp3 => v(&["-c:a", pick(enc, &["libmp3lame"], "MP3")?, "-q:a", "2"]),
        Format::M4a => v(&[
            "-c:a",
            pick(enc, &["aac_at", "aac"], "AAC")?,
            "-b:a",
            "192k",
        ]),
        Format::Wav => v(&["-c:a", "pcm_s16le"]),
        Format::Aiff => v(&["-c:a", "pcm_s16be"]),
        Format::Flac => v(&["-c:a", "flac"]),
        Format::Ogg => {
            let c = pick(enc, &["libvorbis", "vorbis"], "Vorbis")?;
            let mut a = v(&["-c:a", c, "-q:a", "5"]);
            if c == "vorbis" {
                a.extend(v(&["-strict", "-2", "-ac", "2"]));
            }
            a
        }
        Format::Opus => {
            let c = pick(enc, &["libopus", "opus"], "Opus")?;
            let mut a = v(&["-c:a", c, "-b:a", "128k"]);
            if c == "opus" {
                a.extend(v(&["-strict", "-2", "-ar", "48000"]));
            }
            a
        }
        Format::Wma => v(&["-c:a", pick(enc, &["wmav2"], "WMA")?, "-b:a", "192k"]),
        other => {
            return Err(Error::UnsupportedRoute {
                from: "audio".into(),
                to: other.label().into(),
            })
        }
    })
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// Arguments that go before `-i`: fast, exact seeking.
pub fn input_args(job: &MediaJob) -> Vec<String> {
    match job {
        MediaJob::Trim { start, .. } if *start > 0.0 => vec!["-ss".into(), format!("{start:.3}")],
        MediaJob::Frame { at, .. } => vec!["-ss".into(), format!("{:.3}", at.max(0.0))],
        _ => Vec::new(),
    }
}

/// FFmpeg filter that crops to `rect`, keeping even dimensions for H.264.
pub fn crop_filter(rect: &crate::ops::Rect) -> String {
    format!(
        "crop=trunc(iw*{w:.6}/2)*2:trunc(ih*{h:.6}/2)*2:trunc(iw*{x:.6}):trunc(ih*{y:.6})",
        x = rect.x,
        y = rect.y,
        w = rect.w,
        h = rect.h
    )
}

/// FFmpeg filter that draws a solid black box over each rect.
pub fn redact_filter(rects: &[crate::ops::Rect]) -> String {
    rects
        .iter()
        .map(|r| {
            format!(
                "drawbox=x=iw*{x:.6}:y=ih*{y:.6}:w=iw*{w:.6}:h=ih*{h:.6}:color=black:t=fill",
                x = r.x,
                y = r.y,
                w = r.w,
                h = r.h
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Builds FFmpeg arguments (excluding the program, input and output).
pub fn plan_args(
    job: MediaJob,
    from: Format,
    probe: &Probe,
    enc: &HashSet<String>,
) -> Result<(Vec<String>, Format)> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    let to = match &job {
        MediaJob::Convert(t) | MediaJob::Reencode(t) => *t,
        MediaJob::CompressVideo(_) | MediaJob::CompressVideoTarget(_) => Format::Mp4,
        MediaJob::StripMetadata | MediaJob::Trim { .. } | MediaJob::Normalize => from,
        MediaJob::Filter { to, .. } => *to,
        MediaJob::Frame { to, .. } => *to,
    };
    let reencode = matches!(job, MediaJob::Reencode(_));
    match job {
        MediaJob::Trim { start, end } => {
            if let (Some(d), true) = (probe.duration_s, start > 0.0) {
                if start >= d {
                    return Err(Error::Invalid(format!(
                        "the start ({start:.1} s) is past the end ({d:.1} s)"
                    )));
                }
            }
            let (rest, _) = plan_args(MediaJob::Reencode(from), from, probe, enc)?;
            if let Some(end) = end {
                push(&["-t", &format!("{:.3}", end - start)]);
            }
            push(&strs(&rest));
        }
        MediaJob::Normalize => {
            if probe.audio_codec.is_none() && probe.duration_s.is_some() {
                return Err(Error::NothingToDo(
                    "this file has no sound to even out".into(),
                ));
            }
            if from.category() == Category::Video {
                push(&["-map", "0:v:0?", "-map", "0:a:0", "-c:v", "copy"]);
                let audio = match from {
                    Format::Webm => vec![
                        "-c:a".to_string(),
                        pick(enc, &["libopus", "libvorbis"], "Opus")?.to_string(),
                        "-b:a".into(),
                        "128k".into(),
                    ],
                    Format::Avi => audio_codec_args(Format::Mp3, enc)?,
                    Format::Wmv => audio_codec_args(Format::Wma, enc)?,
                    _ => audio_codec_args(Format::M4a, enc)?,
                };
                push(&strs(&audio));
                if matches!(from, Format::Mp4 | Format::Mov) {
                    push(&["-movflags", "+faststart"]);
                }
            } else {
                push(&["-vn", "-map", "0:a:0"]);
                push(&strs(&audio_codec_args(from, enc)?));
            }
            push(&["-af", LOUDNORM, "-ar", "48000"]);
        }
        MediaJob::Frame { to, .. } => {
            push(&["-map", "0:v:0", "-frames:v", "1", "-an", "-update", "1"]);
            if to == Format::Jpeg {
                push(&["-q:v", "2"]);
            }
        }
        MediaJob::Filter { vf, to } => {
            let v = pick(enc, &["libx264", "h264_videotoolbox"], "H.264")?;
            push(&["-map", "0:v:0", "-map", "0:a:0?", "-vf", &vf, "-c:v", v]);
            if v == "libx264" {
                push(&["-preset", "medium", "-crf", "20"]);
            } else {
                push(&["-q:v", "65"]);
            }
            push(&["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k"]);
            if matches!(to, Format::Mp4 | Format::Mov) {
                push(&["-movflags", "+faststart"]);
            }
        }
        MediaJob::CompressVideoTarget(bytes) => {
            let duration = probe.duration_s.filter(|d| *d > 0.0).ok_or_else(|| {
                Error::Missing("compressing to a size needs FFprobe to read the length".into())
            })?;
            // Bits per second for the whole file, minus 128 kb/s of audio
            // and 3% for the container.
            let total_kbps = bytes as f64 * 8.0 * 0.97 / duration / 1000.0;
            let video_kbps = (total_kbps - 128.0).floor();
            if video_kbps < 80.0 {
                return Err(Error::NothingToDo(format!(
                    "a {duration:.0}-second video can't be made that small and still be watchable"
                )));
            }
            let v = pick(enc, &["libx264", "h264_videotoolbox"], "H.264")?;
            let rate = format!("{video_kbps}k");
            let bufsize = format!("{}k", video_kbps * 2.0);
            push(&[
                "-map", "0:v:0", "-map", "0:a:0?", "-c:v", v, "-b:v", &rate, "-maxrate", &rate,
                "-bufsize", &bufsize,
            ]);
            if v == "libx264" {
                push(&["-preset", "medium"]);
            }
            push(&[
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-movflags",
                "+faststart",
            ]);
        }
        MediaJob::StripMetadata => {
            push(&[
                "-map",
                "0",
                "-map_metadata",
                "-1",
                "-map_chapters",
                "-1",
                "-c",
                "copy",
            ]);
            if to == Format::Mp4 || to == Format::Mov || to == Format::M4a {
                push(&[
                    "-fflags",
                    "+bitexact",
                    "-flags:v",
                    "+bitexact",
                    "-flags:a",
                    "+bitexact",
                ]);
            }
        }
        MediaJob::CompressVideo(crf) => {
            let v = pick(enc, &["libx264", "h264_videotoolbox"], "H.264")?;
            push(&["-map", "0:v:0", "-map", "0:a?", "-c:v", v]);
            if v == "libx264" {
                push(&["-preset", "medium", "-crf", &crf.to_string()]);
            } else {
                push(&["-q:v", "55"]);
            }
            push(&[
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-movflags",
                "+faststart",
            ]);
        }
        MediaJob::Convert(to) | MediaJob::Reencode(to) if to.category() == Category::Audio => {
            push(&["-vn", "-map", "0:a:0"]);
            push(&strs(&audio_codec_args(to, enc)?));
        }
        MediaJob::Convert(Format::Gif) => {
            push(&[
                "-an",
                "-vf",
                "fps=12,scale='min(720,iw)':-2:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4",
                "-loop",
                "0",
            ]);
        }
        MediaJob::Convert(to) | MediaJob::Reencode(to) => {
            // Same codecs, new container: copy streams instead of re-encoding.
            if !reencode && remux_ok(from, to, probe) {
                // Only the first audio track was checked, so only it is copied.
                push(&["-map", "0:v:0", "-map", "0:a:0?", "-c", "copy"]);
                // QuickTime and Quick Look only play HEVC tagged hvc1.
                if probe.video_codec.as_deref() == Some("hevc")
                    && matches!(to, Format::Mp4 | Format::Mov)
                {
                    push(&["-tag:v", "hvc1"]);
                }
                if matches!(to, Format::Mp4 | Format::Mov) {
                    push(&["-movflags", "+faststart"]);
                }
            } else {
                push(&["-map", "0:v:0", "-map", "0:a:0?"]);
                // GIF frames and odd sizes need even dimensions for H.264.
                let even = "scale=trunc(iw/2)*2:trunc(ih/2)*2";
                match to {
                    Format::Mp4 | Format::Mov | Format::Mkv => {
                        let v = pick(enc, &["libx264", "h264_videotoolbox", "mpeg4"], "video")?;
                        push(&["-c:v", v]);
                        match v {
                            "libx264" => push(&["-preset", "medium", "-crf", "20"]),
                            "h264_videotoolbox" => push(&["-q:v", "65"]),
                            _ => push(&["-q:v", "3"]),
                        }
                        push(&[
                            "-vf", even, "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k",
                        ]);
                        if to != Format::Mkv {
                            push(&["-movflags", "+faststart"]);
                        }
                    }
                    Format::Webm => {
                        let v = pick(enc, &["libvpx-vp9", "libvpx"], "VP9/VP8")?;
                        push(&[
                            "-c:v", v, "-crf", "32", "-b:v", "0", "-vf", even, "-pix_fmt",
                            "yuv420p",
                        ]);
                        if v == "libvpx-vp9" {
                            push(&["-row-mt", "1"]);
                        }
                        push(&[
                            "-c:a",
                            pick(enc, &["libopus", "libvorbis"], "Opus")?,
                            "-b:a",
                            "128k",
                        ]);
                    }
                    Format::Avi => {
                        push(&[
                            "-c:v",
                            pick(enc, &["mpeg4"], "MPEG-4")?,
                            "-q:v",
                            "3",
                            "-vf",
                            even,
                        ]);
                        let audio = if enc.contains("libmp3lame") {
                            "libmp3lame"
                        } else {
                            "pcm_s16le"
                        };
                        push(&["-c:a", audio]);
                    }
                    Format::Wmv => {
                        push(&[
                            "-c:v",
                            pick(enc, &["wmv2"], "WMV")?,
                            "-q:v",
                            "3",
                            "-vf",
                            even,
                        ]);
                        push(&["-c:a", pick(enc, &["wmav2"], "WMA")?, "-b:a", "192k"]);
                    }
                    other => {
                        return Err(Error::UnsupportedRoute {
                            from: from.label().into(),
                            to: other.label().into(),
                        })
                    }
                }
            }
        }
    }
    Ok((a, to))
}

fn remux_ok(from: Format, to: Format, p: &Probe) -> bool {
    if from == Format::Gif || !p.has_video {
        return false;
    }
    let v = p.video_codec.as_deref().unwrap_or("");
    let a = p.audio_codec.as_deref();
    match to {
        Format::Mp4 | Format::Mov => {
            matches!(v, "h264" | "hevc") && a.is_none_or(|a| matches!(a, "aac" | "mp3" | "alac"))
        }
        Format::Mkv => matches!(v, "h264" | "hevc" | "vp9" | "vp8" | "av1"),
        Format::Webm => {
            matches!(v, "vp9" | "vp8" | "av1") && a.is_none_or(|a| matches!(a, "opus" | "vorbis"))
        }
        _ => false,
    }
}

/// Runs FFmpeg, reporting progress (0..1) and honouring cancellation.
/// Without FFmpeg, jobs AVFoundation can do go to the Mac helper instead.
pub fn run(
    caps: &Capabilities,
    job: MediaJob,
    input: &Path,
    from: Format,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Format> {
    let Some(ffmpeg) = caps.ffmpeg.as_ref() else {
        // Without FFmpeg, a Mac still converts and trims what it plays.
        return crate::engines::avmedia::run(caps, job, input, from, out, cancel, progress);
    };
    let probe = probe(caps, input);
    let pre = input_args(&job);
    // Progress is measured against the length of what's written.
    let length = match &job {
        // An end past the file stops at the file's end.
        MediaJob::Trim { start, end } => end
            .map(|e| probe.duration_s.map_or(e, |d| e.min(d)))
            .or(probe.duration_s)
            .map(|e| e - start)
            .filter(|l| *l > 0.0),
        MediaJob::Frame { .. } => None,
        _ => probe.duration_s,
    };
    let (args, to) = plan_args(job, from, &probe, encoders(ffmpeg))?;
    let mut all: Vec<OsString> = pre.into_iter().map(OsString::from).collect();
    all.push("-i".into());
    all.push(file_url(input).into());
    all.extend(args.into_iter().map(OsString::from));
    all.extend(["-f".into(), muxer(to).into(), file_url(out).into()]);
    run_ffmpeg(ffmpeg, &all, length, None, cancel, progress)?;
    Ok(to)
}

/// Runs FFmpeg with `args` (after its own quiet, overwrite and progress
/// flags), reporting progress against `length` seconds of output. `cwd`
/// is where relative names in filters are found.
pub(crate) fn run_ffmpeg(
    ffmpeg: &Path,
    args: &[OsString],
    length: Option<f64>,
    cwd: Option<&Path>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-y",
        "-progress",
        "pipe:1",
        "-nostats",
    ])
    .args(args)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Ffmpeg(format!("couldn't start FFmpeg: {e}")))?;
    let stderr_tail = spawn_tail_reader(child.stderr.take());
    let (tx, rx) = mpsc::channel::<f32>();
    let duration_us = length.map(|d| d * 1e6).filter(|d| *d > 0.0);
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let (Some(v), Some(total)) = (line.strip_prefix("out_time_us="), duration_us) {
                    if let Ok(us) = v.trim().parse::<f64>() {
                        let _ = tx.send((us / total).clamp(0.0, 1.0) as f32);
                    }
                }
            }
        });
    }
    let status = loop {
        while let Ok(p) = rx.try_recv() {
            progress(p);
        }
        if cancel.load(Ordering::Relaxed) {
            kill(&mut child);
            return Err(Error::Cancelled);
        }
        match child.try_wait()? {
            Some(status) => break status,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    if !status.success() {
        let tail = stderr_tail.join().unwrap_or_default();
        let msg = tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("unknown error")
            .trim()
            .to_string();
        return Err(Error::Ffmpeg(msg));
    }
    progress(1.0);
    Ok(())
}

/// A video's picture size as it shows (turned upright) and its frame rate
/// (at most 60), from FFprobe.
pub fn video_shape(caps: &Capabilities, input: &Path) -> Option<(u32, u32, f64)> {
    #[derive(Deserialize)]
    struct Shape {
        #[serde(default)]
        streams: Vec<ShapeStream>,
    }
    #[derive(Deserialize)]
    struct ShapeStream {
        width: Option<u32>,
        height: Option<u32>,
        r_frame_rate: Option<String>,
        #[serde(default)]
        side_data_list: Vec<SideData>,
        #[serde(default)]
        tags: std::collections::BTreeMap<String, serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct SideData {
        rotation: Option<f64>,
    }
    let ffprobe = caps.ffprobe.as_ref()?;
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate:stream_side_data=rotation:stream_tags=rotate",
            "-of",
            "json",
        ])
        .arg(file_url(input))
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let shape: Shape = serde_json::from_slice(&out.stdout).ok()?;
    let s = shape.streams.into_iter().next()?;
    let (w, h) = (s.width?, s.height?);
    let rotation = s
        .side_data_list
        .iter()
        .find_map(|d| d.rotation)
        .or_else(|| s.tags.get("rotate")?.as_str()?.parse().ok())
        .unwrap_or(0.0);
    let turned = (rotation.round() as i64).rem_euclid(180) == 90;
    let fps = s
        .r_frame_rate
        .and_then(|r| {
            let (n, d) = r.split_once('/')?;
            let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
            (d > 0.0).then(|| n / d)
        })
        .filter(|f| f.is_finite() && *f >= 1.0)
        .unwrap_or(30.0)
        .min(60.0);
    Some(if turned { (h, w, fps) } else { (w, h, fps) })
}

/// Whether the FFmpeg we found has a filter (`subtitles` needs libass).
pub fn has_filter(ffmpeg: &Path, name: &str) -> bool {
    static FILTERS: OnceLock<HashSet<String>> = OnceLock::new();
    FILTERS
        .get_or_init(|| {
            let out = Command::new(ffmpeg)
                .args(["-hide_banner", "-filters"])
                .stderr(Stdio::null())
                .output();
            let text = out
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            text.lines()
                .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
                .collect()
        })
        .contains(name)
}

/// One clip to join: what it has, and how long its part lasts.
#[derive(Debug, Clone, Copy)]
pub struct JoinPart {
    pub has_audio: bool,
    pub seconds: f64,
}

/// The filter graph that puts clips one after another. With `video`
/// (width, height, frame rate), each picture is fitted into that shape,
/// centred on black. Each sound becomes 48 kHz stereo, padded or cut to
/// its clip's length so the next clip stays in step; a clip without sound
/// gets silence. Returns the graph and whether it has sound.
pub fn join_graph(parts: &[JoinPart], video: Option<(u32, u32, f64)>) -> (String, bool) {
    let sound = parts.iter().any(|p| p.has_audio);
    let mut graph = String::new();
    let mut inputs = String::new();
    for (i, p) in parts.iter().enumerate() {
        if let Some((w, h, fps)) = video {
            graph.push_str(&format!(
                "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,\
                 pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,fps={fps:.3},format=yuv420p[v{i}];"
            ));
            inputs.push_str(&format!("[v{i}]"));
        }
        if sound {
            let seconds = p.seconds;
            if p.has_audio {
                graph.push_str(&format!(
                    "[{i}:a:0]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,\
                     apad,atrim=duration={seconds:.3}[a{i}];"
                ));
            } else {
                graph.push_str(&format!(
                    "anullsrc=r=48000:cl=stereo,atrim=duration={seconds:.3}[a{i}];"
                ));
            }
            inputs.push_str(&format!("[a{i}]"));
        }
    }
    graph.push_str(&format!(
        "{inputs}concat=n={}:v={}:a={}",
        parts.len(),
        u8::from(video.is_some()),
        u8::from(sound)
    ));
    if video.is_some() {
        graph.push_str("[v]");
    }
    if sound {
        graph.push_str("[a]");
    }
    (graph, sound)
}

/// Joins `inputs` (all video, or all sound) into `out`: H.264 and AAC in
/// MP4, or AAC in M4A for sound.
pub fn join(
    caps: &Capabilities,
    inputs: &[std::path::PathBuf],
    video: bool,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let ffmpeg = caps.ffmpeg.as_ref().ok_or(Error::FfmpegMissing)?;
    if caps.ffprobe.is_none() {
        return Err(Error::Missing(
            "joining clips needs FFprobe to read their lengths".into(),
        ));
    }
    let mut parts = Vec::with_capacity(inputs.len());
    for p in inputs {
        let probe = probe(caps, p);
        let seconds = if video {
            probe.video_duration_s.or(probe.duration_s)
        } else {
            probe.duration_s
        }
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "couldn't read how long {} is",
                crate::format::display_name(p)
            ))
        })?;
        parts.push(JoinPart {
            has_audio: probe.audio_codec.is_some(),
            seconds,
        });
    }
    let shape = if video {
        let (w, h, fps) = video_shape(caps, &inputs[0])
            .ok_or_else(|| Error::Invalid("couldn't read the first clip's picture size".into()))?;
        // Even sides for H.264, and no larger than 4K.
        let scale = (3840.0 / f64::from(w.max(h))).min(1.0);
        let even = |x: u32| ((f64::from(x) * scale / 2.0).round() as u32 * 2).max(2);
        Some((even(w), even(h), fps))
    } else {
        None
    };
    let (graph, sound) = join_graph(&parts, shape);
    if !video && !sound {
        return Err(Error::NothingToDo("these files have no sound".into()));
    }
    let enc = encoders(ffmpeg);
    let mut args: Vec<OsString> = Vec::new();
    for p in inputs {
        args.push("-i".into());
        args.push(file_url(p).into());
    }
    let mut push = |xs: &[&str]| args.extend(xs.iter().map(OsString::from));
    push(&["-filter_complex", &graph]);
    if video {
        let v = pick(enc, &["libx264", "h264_videotoolbox"], "H.264")?;
        push(&["-map", "[v]", "-c:v", v]);
        if v == "libx264" {
            push(&["-preset", "medium", "-crf", "20"]);
        } else {
            push(&["-q:v", "65"]);
        }
        push(&["-pix_fmt", "yuv420p"]);
    }
    if sound {
        let a = pick(enc, &["aac_at", "aac"], "AAC")?;
        push(&["-map", "[a]", "-c:a", a, "-b:a", "192k"]);
    }
    push(&[
        "-movflags",
        "+faststart",
        "-f",
        if video { "mp4" } else { "ipod" },
    ]);
    args.push(file_url(out).into());
    let total: f64 = parts.iter().map(|p| p.seconds).sum();
    run_ffmpeg(ffmpeg, &args, Some(total), None, cancel, progress)
}

/// Draws the subtitles in `dir`/`subs` into the picture of `input`, as
/// H.264 and AAC in MP4. FFmpeg runs in `dir`, so the subtitle file's name
/// needs no escaping in the filter.
pub fn burn(
    caps: &Capabilities,
    input: &Path,
    dir: &Path,
    subs: &str,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<()> {
    let ffmpeg = caps.ffmpeg.as_ref().ok_or(Error::FfmpegMissing)?;
    debug_assert!(subs.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'));
    let enc = encoders(ffmpeg);
    let v = pick(enc, &["libx264", "h264_videotoolbox"], "H.264")?;
    let a = pick(enc, &["aac_at", "aac"], "AAC")?;
    let filter = format!("subtitles={subs}");
    let mut args: Vec<OsString> = vec!["-i".into(), file_url(input).into()];
    let mut push = |xs: &[&str]| args.extend(xs.iter().map(OsString::from));
    push(&["-map", "0:v:0", "-map", "0:a:0?", "-vf", &filter, "-c:v", v]);
    if v == "libx264" {
        push(&["-preset", "medium", "-crf", "20"]);
    } else {
        push(&["-q:v", "65"]);
    }
    push(&[
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        a,
        "-b:a",
        "192k",
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
    ]);
    args.push(file_url(out).into());
    let length = probe(caps, input).duration_s;
    run_ffmpeg(ffmpeg, &args, length, Some(dir), cancel, progress)
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Keeps only the last 8 KB of a stream so a chatty process can't fill memory.
pub(crate) fn spawn_tail_reader<R: Read + Send + 'static>(
    r: Option<R>,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let Some(mut r) = r else { return String::new() };
        let mut tail: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&buf[..n]);
            if tail.len() > 8192 {
                tail.drain(..tail.len() - 8192);
            }
        }
        String::from_utf8_lossy(&tail).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn audio_args_pick_available_encoders() {
        let e = enc(&["libmp3lame", "aac", "flac", "opus"]);
        let (a, to) = plan_args(
            MediaJob::Convert(Format::Mp3),
            Format::Wav,
            &Probe::default(),
            &e,
        )
        .unwrap();
        assert_eq!(to, Format::Mp3);
        assert!(a.windows(2).any(|w| w == ["-c:a", "libmp3lame"]));
        let (a, _) = plan_args(
            MediaJob::Convert(Format::Opus),
            Format::Wav,
            &Probe::default(),
            &e,
        )
        .unwrap();
        assert!(
            a.contains(&"-strict".to_string()),
            "native opus encoder needs -strict -2"
        );
        assert!(plan_args(
            MediaJob::Convert(Format::Wma),
            Format::Wav,
            &Probe::default(),
            &e
        )
        .is_err());
    }

    #[test]
    fn remux_when_codecs_fit() {
        let e = enc(&["libx264", "aac"]);
        let h264 = Probe {
            duration_s: Some(3.0),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            has_video: true,
            ..Default::default()
        };
        let (a, _) = plan_args(MediaJob::Convert(Format::Mp4), Format::Mkv, &h264, &e).unwrap();
        assert!(a.windows(2).any(|w| w == ["-c", "copy"]));
        let vp9 = Probe {
            video_codec: Some("vp9".into()),
            ..h264.clone()
        };
        let (a, _) = plan_args(MediaJob::Convert(Format::Mp4), Format::Webm, &vp9, &e).unwrap();
        assert!(a.windows(2).any(|w| w == ["-c:v", "libx264"]));
    }

    #[test]
    fn compress_targets_mp4() {
        let (a, to) = plan_args(
            MediaJob::CompressVideo(27),
            Format::Mov,
            &Probe::default(),
            &enc(&["libx264"]),
        )
        .unwrap();
        assert_eq!(to, Format::Mp4);
        assert!(a.windows(2).any(|w| w == ["-crf", "27"]));
    }

    #[test]
    fn filters_and_target_bitrate() {
        use crate::ops::Rect;
        let r = Rect {
            x: 0.1,
            y: 0.2,
            w: 0.5,
            h: 0.25,
        };
        assert_eq!(
            crop_filter(&r),
            "crop=trunc(iw*0.500000/2)*2:trunc(ih*0.250000/2)*2:trunc(iw*0.100000):trunc(ih*0.200000)"
        );
        assert_eq!(redact_filter(&[r, r]).matches("drawbox").count(), 2);

        let e = enc(&["libx264", "aac"]);
        let p = Probe {
            duration_s: Some(100.0),
            ..Default::default()
        };
        let (a, to) = plan_args(
            MediaJob::CompressVideoTarget(10_000_000),
            Format::Mov,
            &p,
            &e,
        )
        .unwrap();
        assert_eq!(to, Format::Mp4);
        // 10 MB over 100 s = 776 kb/s total, minus 128 for audio.
        assert!(a.windows(2).any(|w| w == ["-b:v", "648k"]), "{a:?}");
        assert!(plan_args(MediaJob::CompressVideoTarget(10_000), Format::Mov, &p, &e).is_err());
        assert!(plan_args(
            MediaJob::CompressVideoTarget(10_000_000),
            Format::Mov,
            &Probe::default(),
            &e
        )
        .is_err());
    }

    #[test]
    fn file_urls_are_absolute() {
        assert!(file_url(Path::new("relative/name.mp4")).starts_with("file:/"));
    }
}
