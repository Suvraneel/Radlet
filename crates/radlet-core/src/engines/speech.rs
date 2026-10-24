//! Subtitles and transcripts from speech, on the Mac.
//!
//! FFmpeg finds the pauses and cuts the sound into chunks of under a
//! minute (speech recognisers work best on short pieces), the
//! `radlet-vision` helper turns each chunk into timed words with Apple's
//! on-device speech recognition, and the words become subtitle cues here.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;

use crate::catalog::Capabilities;
use crate::engines::{media, vision};
use crate::error::{Error, Result};
use crate::format::Format;

/// One recognised word, in seconds from the start of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// One subtitle: up to two lines on screen from `start` to `end`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub lines: Vec<String>,
}

/// Longest recording transcribed, in seconds.
const MAX_LENGTH: f64 = 12.0 * 3600.0;

/// Chunks aim for this length, cut at a pause...
const CHUNK_TARGET: f64 = 45.0;
/// ...and are never longer than this.
const CHUNK_MAX: f64 = 58.0;

/// Splits `duration` seconds into chunks, cutting in the middle of a pause
/// near every `CHUNK_TARGET` seconds where there is one.
pub fn plan_chunks(duration: f64, silences: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut chunks = Vec::new();
    let mut start = 0.0;
    while duration - start > CHUNK_MAX {
        let earliest = start + CHUNK_TARGET * 0.6;
        let latest = start + CHUNK_MAX;
        // The pause whose middle is nearest the target, inside the window.
        let cut = silences
            .iter()
            .map(|(a, b)| (a + b) / 2.0)
            .filter(|m| *m >= earliest && *m <= latest)
            .min_by(|a, b| {
                let target = start + CHUNK_TARGET;
                (a - target).abs().total_cmp(&(b - target).abs())
            })
            .unwrap_or(start + CHUNK_TARGET);
        chunks.push((start, cut));
        start = cut;
    }
    if duration - start > 0.05 || chunks.is_empty() {
        chunks.push((start, duration));
    }
    chunks
}

/// Pauses in the sound, as (start, end) seconds, from FFmpeg's
/// silencedetect filter.
fn silences(caps: &Capabilities, input: &Path, cancel: &AtomicBool) -> Result<Vec<(f64, f64)>> {
    let ffmpeg = caps.ffmpeg.as_ref().ok_or(Error::FfmpegMissing)?;
    let mut child = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .args([
            "-vn",
            "-af",
            "silencedetect=noise=-32dB:d=0.35",
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Ffmpeg(format!("couldn't start FFmpeg: {e}")))?;
    let stderr = child.stderr.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut found = Vec::new();
        let mut open: Option<f64> = None;
        for line in BufReader::new(stderr)
            .lines()
            .map_while(std::result::Result::ok)
        {
            let value = |key: &str| -> Option<f64> {
                let at = line.find(key)? + key.len();
                line[at..].split_whitespace().next()?.parse().ok()
            };
            if let Some(s) = value("silence_start: ") {
                open = Some(s.max(0.0));
            } else if let (Some(e), Some(s)) = (value("silence_end: "), open.take()) {
                found.push((s, e));
            }
        }
        found
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        if child.try_wait()?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(reader.join().unwrap_or_default())
}

/// Writes `start`..`end` of the sound as 16 kHz mono WAV, the form speech
/// recognisers expect.
fn cut_wav(
    caps: &Capabilities,
    input: &Path,
    (start, end): (f64, f64),
    out: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let ffmpeg = caps.ffmpeg.as_ref().ok_or(Error::FfmpegMissing)?;
    let mut child = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
        .args(["-ss", &format!("{start:.3}"), "-i"])
        .arg(input)
        .args(["-t", &format!("{:.3}", (end - start).max(0.1))])
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-f",
            "wav",
        ])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Ffmpeg(format!("couldn't start FFmpeg: {e}")))?;
    let tail = media::spawn_tail_reader(child.stderr.take());
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        if let Some(s) = child.try_wait()? {
            break s;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        let msg = tail.join().unwrap_or_default();
        let last = msg
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(Error::Ffmpeg(format!("couldn't read the sound ({last})")));
    }
    Ok(())
}

#[derive(Deserialize)]
struct HelperWord {
    t: String,
    s: f64,
    d: f64,
}

#[derive(Deserialize)]
struct HelperAnswer {
    #[serde(default)]
    locale: String,
    chunks: Vec<Vec<HelperWord>>,
}

/// Recognised words of `input`'s speech, and the language they're in.
pub fn words(
    caps: &Capabilities,
    input: &Path,
    language: Option<&str>,
    scratch: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(Vec<Word>, String)> {
    if caps.vision.is_none() {
        return Err(Error::Missing(
            "subtitles use speech recognition in the Radlet app on macOS".into(),
        ));
    }
    let probe = media::probe(caps, input);
    if probe.audio_codec.is_none() {
        return Err(Error::NothingToDo("this file has no sound".into()));
    }
    let duration = probe
        .duration_s
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or_else(|| Error::Missing("subtitles need FFprobe to read the length".into()))?;
    if duration > MAX_LENGTH {
        return Err(Error::TooLarge(
            "subtitles work on recordings of up to 12 hours".into(),
        ));
    }
    let quiet = silences(caps, input, cancel)?;
    progress(0.05);
    let plan = plan_chunks(duration, &quiet);
    let dir = tempfile::Builder::new()
        .prefix("speech-")
        .tempdir_in(scratch)?;
    let mut chunks: Vec<PathBuf> = Vec::new();
    for (i, span) in plan.iter().enumerate() {
        let wav = dir.path().join(format!("chunk{i:04}.wav"));
        cut_wav(caps, input, *span, &wav, cancel)?;
        chunks.push(wav);
        progress(0.05 + 0.15 * (i + 1) as f32 / plan.len() as f32);
    }
    let answer_path = dir.path().join("words.json");
    vision::transcribe(caps, language, &answer_path, &chunks, cancel, &mut |p| {
        progress(0.2 + 0.78 * p)
    })?;
    let answer: HelperAnswer =
        serde_json::from_slice(&std::fs::read(&answer_path)?).map_err(|e| {
            Error::Helper(format!(
                "the speech helper's answer didn't make sense ({e})"
            ))
        })?;
    if answer.chunks.len() != plan.len() {
        return Err(Error::Helper(format!(
            "the speech helper answered for {} of {} pieces",
            answer.chunks.len(),
            plan.len()
        )));
    }
    let mut all = Vec::new();
    for ((start, end), list) in plan.iter().zip(answer.chunks) {
        for w in list {
            let text = w.t.trim().to_string();
            if text.is_empty() || !w.s.is_finite() || !w.d.is_finite() {
                continue;
            }
            let s = (start + w.s.max(0.0)).min(*end);
            all.push(Word {
                text,
                start: s,
                end: (s + w.d.max(0.0)).min(*end).max(s),
            });
        }
    }
    if all.is_empty() {
        return Err(Error::NothingToDo("no speech found".into()));
    }
    Ok((all, answer.locale))
}

/// Languages written without spaces between words.
fn unspaced(locale: &str) -> bool {
    let lang = locale
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        lang.as_str(),
        "zh" | "ja" | "th" | "lo" | "km" | "my" | "yue" | "wuu"
    )
}

/// Screen width of text: wide (CJK) characters count twice.
fn width(s: &str) -> usize {
    s.chars()
        .map(|c| match c as u32 {
            0x1100..=0x115F
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x3FFFD => 2,
            _ => 1,
        })
        .sum()
}

/// Most characters on one subtitle line, as broadcasters use.
const LINE: usize = 42;
/// Longest a cue stays on screen, and shortest.
const MAX_CUE: f64 = 6.0;
const MIN_CUE: f64 = 1.0;
/// A pause this long starts a new cue.
const PAUSE: f64 = 0.7;

fn join(words: &[Word], spaced: bool) -> String {
    let sep = if spaced { " " } else { "" };
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(sep)
}

fn ends_sentence(text: &str) -> bool {
    text.ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

/// Groups words into cues of at most two lines and six seconds, breaking
/// at sentence ends and pauses where possible.
pub fn cues(words: &[Word], locale: &str) -> Vec<Cue> {
    let spaced = !unspaced(locale);
    let mut cues: Vec<Cue> = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    let flush = |current: &mut Vec<Word>, cues: &mut Vec<Cue>| {
        if current.is_empty() {
            return;
        }
        let lines = two_lines(current, spaced);
        cues.push(Cue {
            start: current[0].start,
            end: current.last().expect("not empty").end,
            lines,
        });
        current.clear();
    };
    for w in words {
        if let Some(last) = current.last() {
            let mut with = current.clone();
            with.push(w.clone());
            // It must still fit on two lines of at most LINE each.
            let too_long = two_lines(&with, spaced).iter().any(|l| width(l) > LINE);
            let too_slow = w.end - current[0].start > MAX_CUE;
            let paused = w.start - last.end > PAUSE;
            let sentence = ends_sentence(&last.text) && last.end - current[0].start >= MIN_CUE;
            if too_long || too_slow || paused || sentence {
                flush(&mut current, &mut cues);
            }
        }
        current.push(w.clone());
    }
    flush(&mut current, &mut cues);
    // Keep short cues up long enough to read, without overlapping the next.
    for i in 0..cues.len() {
        let next = cues.get(i + 1).map(|c| c.start - 0.05).unwrap_or(f64::MAX);
        let c = &mut cues[i];
        if c.end - c.start < MIN_CUE {
            c.end = (c.start + MIN_CUE).min(next).max(c.end);
        }
    }
    cues
}

/// One or two lines: two when it doesn't fit on one, split at the word
/// boundary that balances them best.
fn two_lines(words: &[Word], spaced: bool) -> Vec<String> {
    let all = join(words, spaced);
    if width(&all) <= LINE || words.len() < 2 {
        return vec![all];
    }
    let best = (1..words.len())
        .min_by_key(|&k| {
            let (a, b) = (join(&words[..k], spaced), join(&words[k..], spaced));
            let over = width(&a).max(width(&b)).saturating_sub(LINE) * 100;
            over + width(&a).abs_diff(width(&b))
        })
        .expect("at least two words");
    vec![join(&words[..best], spaced), join(&words[best..], spaced)]
}

fn timestamp(seconds: f64, sep: char) -> String {
    let ms = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}{sep}{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, c) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            timestamp(c.start, ','),
            timestamp(c.end, ','),
            c.lines.join("\n")
        ));
    }
    out
}

pub fn to_vtt(cues: &[Cue]) -> String {
    // WebVTT cue text is markup: &, < and > must be escaped.
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut out = String::from("WEBVTT\n\n");
    for c in cues {
        let lines: Vec<String> = c.lines.iter().map(|l| escape(l)).collect();
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            timestamp(c.start, '.'),
            timestamp(c.end, '.'),
            lines.join("\n")
        ));
    }
    out
}

/// A plain transcript: sentences run on, and a pause of two seconds or
/// more starts a new paragraph.
pub fn to_text(words: &[Word], locale: &str) -> String {
    let spaced = !unspaced(locale);
    let mut out = String::new();
    let mut prev: Option<&Word> = None;
    for w in words {
        if let Some(p) = prev {
            if w.start - p.end >= 2.0 && ends_sentence(&p.text) {
                out.push_str("\n\n");
            } else if spaced {
                out.push(' ');
            }
        }
        out.push_str(&w.text);
        prev = Some(w);
    }
    out.push('\n');
    out
}

/// Subtitles or a transcript of `input`, in `to` (SRT, WebVTT or text).
pub fn transcribe(
    caps: &Capabilities,
    input: &Path,
    to: Format,
    language: Option<&str>,
    scratch: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<String> {
    let (words, locale) = words(caps, input, language, scratch, cancel, progress)?;
    Ok(match to {
        Format::Srt => to_srt(&cues(&words, &locale)),
        Format::Vtt => to_vtt(&cues(&words, &locale)),
        Format::Txt => to_text(&words, &locale),
        other => {
            return Err(Error::Invalid(format!(
                "subtitles are saved as SRT, WebVTT or text, not {other}"
            )))
        }
    })
}

/// A language tag as the helper takes it ("en-US", "pt_BR", "auto").
pub fn valid_language(tag: &str) -> bool {
    (2..=24).contains(&tag.len())
        && tag.starts_with(|c: char| c.is_ascii_alphabetic())
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, start: f64, end: f64) -> Word {
        Word {
            text: text.into(),
            start,
            end,
        }
    }

    #[test]
    fn chunks_cut_at_pauses_and_never_run_long() {
        assert_eq!(plan_chunks(30.0, &[]), [(0.0, 30.0)]);
        // A pause near 45 s is used; without pauses, 45 s cuts.
        let c = plan_chunks(100.0, &[(10.0, 10.5), (43.0, 44.0), (90.0, 91.0)]);
        assert_eq!(c[0], (0.0, 43.5));
        assert!(c.iter().all(|(a, b)| b - a <= CHUNK_MAX + 1e-9), "{c:?}");
        assert_eq!(c.last().unwrap().1, 100.0);
        let plain = plan_chunks(200.0, &[]);
        assert_eq!(plain[0], (0.0, 45.0));
        assert!(plain.windows(2).all(|p| p[0].1 == p[1].0));
    }

    #[test]
    fn cues_break_at_sentences_pauses_and_length() {
        let words = vec![
            w("Hello", 0.0, 0.4),
            w("there.", 0.45, 1.2),
            w("This", 1.3, 1.5),
            w("is", 1.55, 1.7),
            w("a", 1.75, 1.8),
            w("test", 1.85, 2.2),
            // A long pause.
            w("After", 4.0, 4.3),
            w("the", 4.35, 4.5),
            w("pause", 4.55, 5.0),
        ];
        let c = cues(&words, "en-US");
        let text: Vec<String> = c.iter().map(|c| c.lines.join(" / ")).collect();
        assert_eq!(text, ["Hello there.", "This is a test", "After the pause"]);
        assert_eq!(c[0].start, 0.0);
        assert!(c[1].end >= c[1].start + MIN_CUE - 1e-9);

        // A long run of words wraps onto two balanced lines, then a new cue.
        let many: Vec<Word> = (0..30)
            .map(|i| w("word", i as f64 * 0.15, i as f64 * 0.15 + 0.1))
            .collect();
        let c = cues(&many, "en");
        assert!(c.len() >= 2);
        for cue in &c {
            assert!(cue.lines.len() <= 2);
            assert!(cue.lines.iter().all(|l| width(l) <= LINE), "{cue:?}");
            assert!(cue.end - cue.start <= MAX_CUE + 1e-9);
        }
    }

    #[test]
    fn writers_and_unspaced_languages() {
        let c = vec![Cue {
            start: 3661.5,
            end: 3663.25,
            lines: vec!["Hi".into(), "there".into()],
        }];
        assert_eq!(
            to_srt(&c),
            "1\n01:01:01,500 --> 01:01:03,250\nHi\nthere\n\n"
        );
        assert_eq!(
            to_vtt(&c),
            "WEBVTT\n\n01:01:01.500 --> 01:01:03.250\nHi\nthere\n\n"
        );
        let zh = vec![w("你好", 0.0, 0.5), w("世界。", 0.5, 1.2)];
        assert_eq!(cues(&zh, "zh-CN")[0].lines, ["你好世界。"]);
        let words = vec![
            w("One.", 0.0, 0.5),
            w("Two.", 3.0, 3.5),
            w("three", 3.6, 4.0),
        ];
        assert_eq!(to_text(&words, "en-GB"), "One.\n\nTwo. three\n");
        assert!(valid_language("en-US") && valid_language("auto") && !valid_language("en US"));
        assert!(!valid_language("--help"));
        let marked = vec![Cue {
            start: 0.0,
            end: 1.0,
            lines: vec!["Fish & <chips>".into()],
        }];
        assert!(to_vtt(&marked).contains("Fish &amp; &lt;chips&gt;"));
        assert_eq!(width("日本"), 4);
    }
}
