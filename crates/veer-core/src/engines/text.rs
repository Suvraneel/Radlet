//! Subtitles (SRT, WebVTT), Markdown and plain text.

use std::path::Path;

use crate::error::{Error, Result};
use crate::format::Format;

/// Largest text file we load into memory.
const MAX_TEXT_BYTES: u64 = 64 << 20;

/// Reads text, dropping a UTF-8 byte-order mark. Files that aren't valid
/// UTF-8 (old Windows subtitles) are read as Windows-1252.
pub fn read_text(path: &Path) -> Result<String> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_TEXT_BYTES {
        return Err(Error::TooLarge("text files can be at most 64 MB".into()));
    }
    let bytes = std::fs::read(path)?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    Ok(match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    })
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Parses SRT or WebVTT. Both use "start --> end" timing lines, so one
/// tolerant parser handles either (and the common mislabelled files).
pub fn parse_subtitles(src: &str) -> Result<Vec<Cue>> {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        let Some(pos) = lines.iter().position(|l| l.contains("-->")) else {
            continue;
        };
        let timing = lines[pos];
        let (a, b) = timing.split_once("-->").expect("checked above");
        // WebVTT allows cue settings after the end time.
        let b = b.split_whitespace().next().unwrap_or("");
        let (Some(start_ms), Some(end_ms)) = (parse_time(a.trim()), parse_time(b)) else {
            continue;
        };
        let text = lines[pos + 1..].join("\n");
        cues.push(Cue {
            start_ms,
            end_ms: end_ms.max(start_ms),
            text,
        });
    }
    if cues.is_empty() {
        return Err(Error::Invalid("no subtitle cues found".into()));
    }
    Ok(cues)
}

/// Accepts `HH:MM:SS,mmm`, `HH:MM:SS.mmm` and `MM:SS.mmm`.
fn parse_time(s: &str) -> Option<u64> {
    let s = s.replace(',', ".");
    let (hms, frac) = s.split_once('.').unwrap_or((s.as_str(), "0"));
    let parts: Vec<u64> = hms
        .split(':')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        _ => return None,
    };
    if m >= 60 || sec >= 60 {
        return None;
    }
    let digits: String = frac
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .take(3)
        .collect();
    let ms = format!("{digits:0<3}").parse::<u64>().ok()?;
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

fn fmt_time(ms: u64, sep: char) -> String {
    let (h, rem) = (ms / 3_600_000, ms % 3_600_000);
    let (m, rem) = (rem / 60_000, rem % 60_000);
    let (s, ms) = (rem / 1000, rem % 1000);
    format!("{h:02}:{m:02}:{s:02}{sep}{ms:03}")
}

/// Removes WebVTT voice/class tags, keeping SRT-compatible <b>, <i>, <u>.
fn srt_safe(text: &str) -> String {
    strip_tags(text, &["b", "i", "u"])
}

fn strip_tags(text: &str, keep: &[&str]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => {
                let tag = &rest[open + 1..open + close];
                let name = tag
                    .trim_start_matches('/')
                    .split([' ', '.'])
                    .next()
                    .unwrap_or("");
                if keep.contains(&name) {
                    out.push_str(&rest[open..=open + close]);
                }
                rest = &rest[open + close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn write_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, c) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            fmt_time(c.start_ms, ','),
            fmt_time(c.end_ms, ','),
            srt_safe(&c.text)
        ));
    }
    out
}

pub fn write_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for c in cues {
        // "-->" inside cue text would end the cue early in WebVTT.
        let text = c.text.replace("-->", "->");
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            fmt_time(c.start_ms, '.'),
            fmt_time(c.end_ms, '.'),
            text
        ));
    }
    out
}

/// Cue text only, without timings or markup. Repeated lines (common in
/// auto-generated captions) are collapsed.
pub fn subtitles_to_text(cues: &[Cue]) -> String {
    let mut out: Vec<String> = Vec::new();
    for c in cues {
        for line in strip_tags(&c.text, &[]).lines() {
            let line = line.trim();
            if !line.is_empty() && out.last().map(String::as_str) != Some(line) {
                out.push(line.to_string());
            }
        }
    }
    out.join("\n") + "\n"
}

/// Turns plain text into timed cues: one cue per line, timed at a
/// comfortable reading pace (2.5 words a second, 1.5 to 7 seconds each).
pub fn text_to_cues(text: &str) -> Result<Vec<Cue>> {
    let mut cues = Vec::new();
    let mut t = 0u64;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let words = line.split_whitespace().count() as u64;
        let dur = (words * 400).clamp(1_500, 7_000);
        cues.push(Cue {
            start_ms: t,
            end_ms: t + dur,
            text: line.to_string(),
        });
        t += dur;
    }
    if cues.is_empty() {
        return Err(Error::NothingToDo("the text file is empty".into()));
    }
    Ok(cues)
}

fn md_options() -> pulldown_cmark::Options {
    use pulldown_cmark::Options;
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

/// Renders Markdown as a standalone HTML page. Raw HTML in the source is
/// escaped rather than passed through, so the output can't run scripts.
pub fn markdown_to_html(src: &str, title: &str) -> String {
    use pulldown_cmark::{html, Event, Parser};
    let parser = Parser::new_ext(src, md_options()).map(|ev| match ev {
        Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),
        other => other,
    });
    let mut body = String::new();
    html::push_html(&mut body, parser);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>\nbody{{font:16px/1.6 -apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;max-width:46rem;margin:2rem auto;padding:0 1rem;color:#1d1d1f}}\npre,code{{font-family:ui-monospace,Menlo,monospace;font-size:.9em}}\npre{{background:#f5f5f7;padding:1rem;overflow:auto;border-radius:8px}}\ntable{{border-collapse:collapse}}td,th{{border:1px solid #d2d2d7;padding:.3rem .6rem}}\nimg{{max-width:100%}}\n@media (prefers-color-scheme:dark){{body{{background:#1d1d1f;color:#f5f5f7}}pre{{background:#2c2c2e}}td,th{{border-color:#48484a}}a{{color:#64a8ff}}}}\n</style>\n</head>\n<body>\n{}</body>\n</html>\n",
        escape_html(title),
        body
    )
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Plain text from Markdown: formatting removed, structure kept as lines.
pub fn markdown_to_text(src: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut out = String::new();
    for ev in Parser::new_ext(src, md_options()) {
        match ev {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push('\n'),
            Event::Start(Tag::Item) => out.push_str("- "),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Table,
            ) => out.push_str("\n\n"),
            Event::End(TagEnd::Item | TagEnd::TableRow | TagEnd::TableHead) => out.push('\n'),
            Event::End(TagEnd::TableCell) => out.push('\t'),
            _ => {}
        }
    }
    let collapsed: Vec<&str> = out.lines().map(str::trim_end).collect();
    let mut result = collapsed.join("\n");
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result.trim().to_string() + "\n"
}

/// Converts between text formats.
pub fn convert(input: &Path, from: Format, to: Format, out: &Path) -> Result<()> {
    let src = read_text(input)?;
    let result = match (from, to) {
        (Format::Srt | Format::Vtt, Format::Srt) => write_srt(&parse_subtitles(&src)?),
        (Format::Srt | Format::Vtt, Format::Vtt) => write_vtt(&parse_subtitles(&src)?),
        (Format::Srt | Format::Vtt, Format::Txt) => subtitles_to_text(&parse_subtitles(&src)?),
        (Format::Txt, Format::Srt) => write_srt(&text_to_cues(&src)?),
        (Format::Txt, Format::Vtt) => write_vtt(&text_to_cues(&src)?),
        (Format::Md, Format::Html) => {
            let title = input
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            markdown_to_html(&src, &title)
        }
        (Format::Md, Format::Txt) => markdown_to_text(&src),
        _ => {
            return Err(Error::UnsupportedRoute {
                from: from.label().into(),
                to: to.label().into(),
            })
        }
    };
    std::fs::write(out, result)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\r\n00:00:01,000 --> 00:00:02,500\r\nHello <i>there</i>\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nSecond line\r\nwraps\r\n\r\n";

    #[test]
    fn srt_round_trips_through_vtt() {
        let cues = parse_subtitles(SRT).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(
            cues[0],
            Cue {
                start_ms: 1000,
                end_ms: 2500,
                text: "Hello <i>there</i>".into()
            }
        );
        let vtt = write_vtt(&cues);
        assert!(vtt.starts_with("WEBVTT\n\n00:00:01.000 --> 00:00:02.500\n"));
        let back = parse_subtitles(&vtt).unwrap();
        assert_eq!(back, cues);
        assert_eq!(write_srt(&back), write_srt(&cues));
    }

    #[test]
    fn vtt_features() {
        let vtt = "WEBVTT\n\nNOTE a comment\n\nintro\n01:02.5 --> 01:04.000 align:start\n<v Roger>Hi</v> <c.loud>there</c>\n\n";
        let cues = parse_subtitles(vtt).unwrap();
        assert_eq!(cues[0].start_ms, 62_500);
        assert_eq!(
            write_srt(&cues),
            "1\n00:01:02,500 --> 00:01:04,000\nHi there\n\n"
        );
    }

    #[test]
    fn text_extraction_dedupes() {
        let cues = vec![
            Cue {
                start_ms: 0,
                end_ms: 1,
                text: "same".into(),
            },
            Cue {
                start_ms: 1,
                end_ms: 2,
                text: "same".into(),
            },
            Cue {
                start_ms: 2,
                end_ms: 3,
                text: "<i>next</i>".into(),
            },
        ];
        assert_eq!(subtitles_to_text(&cues), "same\nnext\n");
    }

    #[test]
    fn text_becomes_cues() {
        let cues = text_to_cues("One two three four five six seven eight\n\nShort\n").unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].end_ms, 3_200);
        assert_eq!(
            cues[1],
            Cue {
                start_ms: 3_200,
                end_ms: 4_700,
                text: "Short".into()
            }
        );
        assert!(text_to_cues("  \n ").is_err());
    }

    #[test]
    fn bad_times_rejected() {
        assert_eq!(parse_time("00:61:00,000"), None);
        assert_eq!(parse_time("1:02:03.4"), Some(3_723_400));
        assert!(parse_subtitles("not subtitles").is_err());
    }

    #[test]
    fn markdown_html_escapes_raw_html() {
        let html = markdown_to_html(
            "# Title\n\n<script>alert(1)</script>\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            "T<1>",
        );
        assert!(html.contains("<h1>Title</h1>"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("<table>"));
        assert!(html.contains("<title>T&lt;1&gt;</title>"));
    }

    #[test]
    fn markdown_text() {
        let t = markdown_to_text("# Head\n\nSome *bold* text\nwrapped.\n\n- one\n- two\n");
        assert_eq!(t, "Head\n\nSome bold text wrapped.\n\n- one\n- two\n");
    }

    #[test]
    fn windows_1252_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("w.srt");
        std::fs::write(&p, b"caf\xE9 \x93quoted\x94").unwrap();
        assert_eq!(read_text(&p).unwrap(), "café “quoted”");
    }
}
