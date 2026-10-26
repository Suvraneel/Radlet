//! Subtitles (SRT, WebVTT), Markdown and plain text.

use std::path::Path;

use crate::error::{Error, Result};
use crate::format::Format;

/// Largest text file we load into memory.
const MAX_TEXT_BYTES: u64 = 64 << 20;

/// Reads text, dropping a UTF-8 byte-order mark. Files that aren't valid
/// UTF-8 (old Windows subtitles) are read as Windows-1252; files with a
/// UTF-16 byte-order mark as UTF-16.
pub fn read_text(path: &Path) -> Result<String> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_TEXT_BYTES {
        return Err(Error::TooLarge("text files can be at most 64 MB".into()));
    }
    Ok(decode(&std::fs::read(path)?))
}

/// Text from bytes in UTF-8, UTF-16 (with a byte-order mark) or, failing
/// those, Windows-1252.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = |be: bool| -> String {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if be {
                    u16::from_be_bytes(c)
                } else {
                    u16::from_le_bytes(c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return utf16(false);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return utf16(true);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

/// Tidies a text file: UTF-8 without a byte-order mark, `\n` line
/// endings, no spaces or tabs at line ends, no stray control characters,
/// and exactly one newline at the end. In Markdown, a line break written
/// as two trailing spaces becomes a backslash, which means the same.
pub fn clean(raw: &[u8], markdown: bool) -> Result<String> {
    if raw.len() as u64 > MAX_TEXT_BYTES {
        return Err(Error::TooLarge("text files can be at most 64 MB".into()));
    }
    let text = decode(raw).replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<char> = None;
    for (i, line) in lines.iter().enumerate() {
        let body: String = line
            .trim_end()
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect();
        let opener = body.trim_start();
        if markdown && line.len() - line.trim_start().len() < 4 {
            for f in ['`', '~'] {
                if opener.starts_with(&f.to_string().repeat(3)) {
                    fence = match fence {
                        None => Some(f),
                        Some(open) if open == f => None,
                        other => other,
                    };
                }
            }
        }
        let next_has_text = lines.get(i + 1).is_some_and(|n| !n.trim().is_empty());
        let hard_break = markdown
            && fence.is_none()
            && line.ends_with("  ")
            && !body.trim().is_empty()
            && next_has_text;
        // An indented line may be code, where a backslash would show; it
        // keeps its two spaces instead.
        let indented = line.starts_with("    ") || line.starts_with('\t');
        out.push_str(&body);
        if hard_break && indented {
            out.push_str("  ");
        } else if hard_break {
            out.push('\\');
        }
        out.push('\n');
    }
    let trimmed = out.trim_end_matches('\n');
    Ok(if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
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
/// escaped rather than passed through, and links or pictures that could
/// run something (`javascript:`, `data:`) are dropped, keeping their
/// text, so the output can't run scripts.
pub fn markdown_to_html(src: &str, title: &str) -> String {
    use crate::engines::doc::safe_link;
    use pulldown_cmark::{html, Event, Parser, Tag, TagEnd};
    // For each open link or picture: whether it was dropped.
    let mut dropped: Vec<bool> = Vec::new();
    let parser = Parser::new_ext(src, md_options()).filter_map(move |ev| match ev {
        Event::Html(h) | Event::InlineHtml(h) => Some(Event::Text(h)),
        Event::Start(Tag::Link { ref dest_url, .. })
        | Event::Start(Tag::Image { ref dest_url, .. }) => {
            let drop = safe_link(dest_url).is_none();
            dropped.push(drop);
            (!drop).then_some(ev)
        }
        Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => {
            let drop = dropped.pop().unwrap_or(false);
            (!drop).then_some(ev)
        }
        other => Some(other),
    });
    let mut body = String::new();
    html::push_html(&mut body, parser);
    crate::engines::doc::html_page(title, &body, false)
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

/// `H:MM:SS.cc`, as SubStation Alpha writes times.
fn ass_time(ms: u64) -> String {
    let cs = ms / 10;
    format!(
        "{}:{:02}:{:02}.{:02}",
        cs / 360_000,
        cs / 6_000 % 60,
        cs / 100 % 60,
        cs % 100
    )
}

/// Parses SubStation Alpha (ASS or SSA): the `Dialogue:` lines of the
/// `[Events]` section, in the order its `Format:` line gives.
pub fn parse_ass(src: &str) -> Result<Vec<Cue>> {
    let mut fields: Vec<String> = Vec::new();
    let mut in_events = false;
    let mut cues = Vec::new();
    for line in src.lines().map(str::trim) {
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Format:") {
            fields = rest
                .split(',')
                .map(|f| f.trim().to_ascii_lowercase())
                .collect();
        } else if let Some(rest) = line.strip_prefix("Dialogue:") {
            if fields.is_empty() {
                fields = [
                    "layer", "start", "end", "style", "name", "marginl", "marginr", "marginv",
                    "effect", "text",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect();
            }
            // The text is last and may hold commas.
            let parts: Vec<&str> = rest.splitn(fields.len(), ',').collect();
            let get = |name: &str| {
                fields
                    .iter()
                    .position(|f| f == name)
                    .and_then(|i| parts.get(i))
            };
            let (Some(start), Some(end), Some(text)) = (get("start"), get("end"), get("text"))
            else {
                continue;
            };
            let (Some(start_ms), Some(end_ms)) = (parse_time(start.trim()), parse_time(end.trim()))
            else {
                continue;
            };
            cues.push(Cue {
                start_ms,
                end_ms: end_ms.max(start_ms),
                text: ass_text(text),
            });
        }
    }
    if cues.is_empty() {
        return Err(Error::Invalid("no subtitle lines found".into()));
    }
    cues.sort_by_key(|c| c.start_ms);
    Ok(cues)
}

/// SubStation text to SRT-style text: line breaks, and italic, bold and
/// underline kept as tags; other styling dropped.
fn ass_text(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        for code in rest[open + 1..open + close].split('\\') {
            let tag = match code {
                "i1" => "<i>",
                "i0" => "</i>",
                "b1" => "<b>",
                "b0" => "</b>",
                "u1" => "<u>",
                "u0" => "</u>",
                _ => "",
            };
            out.push_str(tag);
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out.replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ")
}

pub fn write_ass(cues: &[Cue]) -> String {
    let mut out = String::from(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\nWrapStyle: 0\n\n\
         [V4+ Styles]\n\
         Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, \
         Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, \
         Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,Helvetica,64,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,\
         1,3,1,2,60,60,50,1\n\n\
         [Events]\n\
         Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    for c in cues {
        let text = srt_safe(&c.text)
            .replace("<i>", "{\\i1}")
            .replace("</i>", "{\\i0}")
            .replace("<b>", "{\\b1}")
            .replace("</b>", "{\\b0}")
            .replace("<u>", "{\\u1}")
            .replace("</u>", "{\\u0}")
            .replace('\n', "\\N");
        out.push_str(&format!(
            "Dialogue: 0,{},{},Default,,0,0,0,,{}\n",
            ass_time(c.start_ms),
            ass_time(c.end_ms),
            text
        ));
    }
    out
}

/// Parses SubViewer (SBV): `0:00:01.000,0:00:04.000`, then the text.
pub fn parse_sbv(src: &str) -> Result<Vec<Cue>> {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    for block in normalized.split("\n\n") {
        let mut lines = block.lines().filter(|l| !l.trim().is_empty());
        let Some(timing) = lines.next() else { continue };
        let Some((a, b)) = timing.split_once(',') else {
            continue;
        };
        let (Some(start_ms), Some(end_ms)) = (parse_time(a.trim()), parse_time(b.trim())) else {
            continue;
        };
        cues.push(Cue {
            start_ms,
            end_ms: end_ms.max(start_ms),
            text: lines.collect::<Vec<_>>().join("\n"),
        });
    }
    if cues.is_empty() {
        return Err(Error::Invalid("no subtitle cues found".into()));
    }
    Ok(cues)
}

pub fn write_sbv(cues: &[Cue]) -> String {
    let t = |ms: u64| {
        let s = fmt_time(ms, '.');
        // SubViewer writes hours without a leading zero.
        s.strip_prefix('0').map(str::to_string).unwrap_or(s)
    };
    let mut out = String::new();
    for c in cues {
        out.push_str(&format!(
            "{},{}\n{}\n\n",
            t(c.start_ms),
            t(c.end_ms),
            strip_tags(&c.text, &[])
        ));
    }
    out
}

/// A TTML time: clock time (`00:00:01.500`, or with frames as
/// `00:00:01:12`) or an offset (`1.5s`, `1500ms`, `2m`, `1h`, `36f`).
fn ttml_time(s: &str, fps: f64) -> Option<u64> {
    let s = s.trim();
    for (unit, scale) in [
        ("ms", 1.0),
        ("h", 3_600_000.0),
        ("m", 60_000.0),
        ("s", 1000.0),
    ] {
        if let Some(n) = s.strip_suffix(unit) {
            return n
                .trim()
                .parse::<f64>()
                .ok()
                .map(|v| (v * scale).round() as u64);
        }
    }
    if let Some(n) = s.strip_suffix('f') {
        return n
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| (v / fps * 1000.0).round() as u64);
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 4 {
        let base = parse_time(&parts[..3].join(":"))?;
        let frames = parts[3].parse::<f64>().ok()?;
        return Some(base + (frames / fps * 1000.0).round() as u64);
    }
    parse_time(s)
}

/// Parses Timed Text (TTML, DFXP): each `<p>` with `begin` and `end` (or
/// `dur`); `<br/>` breaks lines.
pub fn parse_ttml(src: &str) -> Result<Vec<Cue>> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(src);
    let attr = |e: &quick_xml::events::BytesStart, name: &str| {
        e.attributes().flatten().find_map(|a| {
            (a.key.local_name().as_ref() == name)
                .then(|| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
                .flatten()
                .map(|v| v.into_owned())
        })
    };
    let mut fps = 30.0;
    let mut cues = Vec::new();
    let mut current: Option<(u64, u64, String)> = None;
    let mut depth = 0usize;
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| Error::Invalid(format!("this TTML file doesn't read ({e})")))?;
        match ev {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "tt" => {
                if let Some(rate) = attr(&e, "frameRate").and_then(|r| r.parse::<f64>().ok()) {
                    if rate > 0.0 {
                        fps = rate;
                    }
                }
            }
            Event::Start(e) if e.local_name().as_ref() == "p" && current.is_none() => {
                let begin = attr(&e, "begin").and_then(|b| ttml_time(&b, fps));
                let end = attr(&e, "end")
                    .and_then(|t| ttml_time(&t, fps))
                    .or_else(|| {
                        let dur = attr(&e, "dur").and_then(|d| ttml_time(&d, fps))?;
                        Some(begin? + dur)
                    });
                if let (Some(b), Some(t)) = (begin, end) {
                    current = Some((b, t.max(b), String::new()));
                    depth = 0;
                }
            }
            Event::Start(_) if current.is_some() => depth += 1,
            Event::Empty(e) if e.local_name().as_ref() == "br" => {
                if let Some((_, _, text)) = &mut current {
                    text.push('\n');
                }
            }
            Event::Text(t) => {
                if let Some((_, _, text)) = &mut current {
                    text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(r) => {
                if let Some((_, _, text)) = &mut current {
                    match r.resolve_char_ref() {
                        Ok(Some(c)) => text.push(c),
                        _ => text.push_str(match r.as_ref() {
                            "amp" => "&",
                            "lt" => "<",
                            "gt" => ">",
                            "quot" => "\"",
                            "apos" => "'",
                            _ => "",
                        }),
                    }
                }
            }
            Event::End(e) if current.is_some() => {
                if depth > 0 {
                    depth -= 1;
                } else if e.local_name().as_ref() == "p" {
                    let (start_ms, end_ms, text) = current.take().expect("checked");
                    let text = text
                        .lines()
                        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                        .filter(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        cues.push(Cue {
                            start_ms,
                            end_ms,
                            text,
                        });
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if cues.is_empty() {
        return Err(Error::Invalid("no subtitle cues found".into()));
    }
    cues.sort_by_key(|c| c.start_ms);
    Ok(cues)
}

pub fn write_ttml(cues: &[Cue]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <tt xmlns=\"http://www.w3.org/ns/ttml\">\n  <body>\n    <div>\n",
    );
    for c in cues {
        let lines: Vec<String> = strip_tags(&c.text, &[])
            .lines()
            .map(|l| quick_xml::escape::escape(l).into_owned())
            .collect();
        out.push_str(&format!(
            "      <p begin=\"{}\" end=\"{}\">{}</p>\n",
            fmt_time(c.start_ms, '.'),
            fmt_time(c.end_ms, '.'),
            lines.join("<br/>")
        ));
    }
    out.push_str("    </div>\n  </body>\n</tt>\n");
    out
}

/// Cues from any subtitle format Radlet reads.
pub fn read_cues(src: &str, from: Format) -> Result<Vec<Cue>> {
    match from {
        Format::Ass => parse_ass(src),
        Format::Sbv => parse_sbv(src),
        Format::Ttml => parse_ttml(src),
        _ => parse_subtitles(src),
    }
}

/// Cues in a subtitle format, or as plain text.
pub fn write_cues(cues: &[Cue], to: Format) -> Result<String> {
    Ok(match to {
        Format::Srt => write_srt(cues),
        Format::Vtt => write_vtt(cues),
        Format::Ass => write_ass(cues),
        Format::Sbv => write_sbv(cues),
        Format::Ttml => write_ttml(cues),
        Format::Txt => subtitles_to_text(cues),
        other => {
            return Err(Error::UnsupportedRoute {
                from: "subtitles".into(),
                to: other.label().into(),
            })
        }
    })
}

/// Converts between text formats.
pub fn convert(input: &Path, from: Format, to: Format, out: &Path) -> Result<()> {
    let src = read_text(input)?;
    let result = match (from, to) {
        (f, t) if f.is_subtitles() => write_cues(&read_cues(&src, f)?, t)?,
        (Format::Txt, t) if t.is_subtitles() => write_cues(&text_to_cues(&src)?, t)?,
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
    fn markdown_html_drops_script_links() {
        let html = markdown_to_html(
            "[a](javascript:alert(1)) [b](https://x.y) ![c](data:image/png;base64,AA) ![d](pic.png)",
            "t",
        );
        assert!(
            !html.contains("javascript:") && !html.contains("data:"),
            "{html}"
        );
        assert!(
            html.contains(">a<") || html.contains(" a ") || html.contains("<p>a "),
            "{html}"
        );
        assert!(html.contains("<a href=\"https://x.y\">b</a>"));
        assert!(html.contains("c") && html.contains("src=\"pic.png\""));
    }

    #[test]
    fn cleaning_text() {
        let raw = b"\xEF\xBB\xBFline one  \r\nline two\t\r\n\x07bell\n\n\n";
        assert_eq!(clean(raw, false).unwrap(), "line one\nline two\nbell\n");
        // Markdown hard breaks survive, but not inside code or at a
        // paragraph's end.
        let md = "a  \nb  \n\n```\ncode  \nmore\n```\n\n    indented  \n    code\n";
        assert_eq!(
            clean(md.as_bytes(), true).unwrap(),
            "a\\\nb\n\n```\ncode\nmore\n```\n\n    indented  \n    code\n"
        );
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("hé \r\n".encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        assert_eq!(clean(&utf16, false).unwrap(), "hé\n");
        assert_eq!(clean(b"\n\n", false).unwrap(), "");
    }

    #[test]
    fn windows_1252_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("w.srt");
        std::fs::write(&p, b"caf\xE9 \x93quoted\x94").unwrap();
        assert_eq!(read_text(&p).unwrap(), "café “quoted”");
    }
    #[test]
    fn subtitle_formats_round_trip() {
        let srt = "1\n00:00:01,000 --> 00:00:03,500\n<i>Hello</i>, world\nsecond line\n\n2\n00:01:02,250 --> 00:01:04,000\nBye & see you\n\n";
        let cues = parse_subtitles(srt).unwrap();
        for f in [Format::Ass, Format::Sbv, Format::Ttml, Format::Vtt] {
            let written = write_cues(&cues, f).unwrap();
            let back = read_cues(&written, f).unwrap();
            assert_eq!(back.len(), 2, "{f}");
            assert_eq!(back[0].start_ms, 1000, "{f}");
            assert_eq!(back[0].end_ms, 3500, "{f}");
            assert_eq!(back[1].start_ms, 62_250, "{f}");
            assert!(
                back[1].text.contains("Bye & see you"),
                "{f}: {:?}",
                back[1].text
            );
            assert!(back[0].text.contains("second line"), "{f}");
        }
        let ass = write_ass(&cues);
        assert!(ass.contains("Dialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,{\\i1}Hello{\\i0}, world\\Nsecond line"), "{ass}");
        assert_eq!(
            read_cues(&ass, Format::Ass).unwrap()[0].text,
            "<i>Hello</i>, world\nsecond line"
        );
    }

    #[test]
    fn subtitle_formats_from_other_tools() {
        let ssa = "[Script Info]\nScriptType: v4.00\n\n[Events]\nFormat: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: Marked=0,0:00:05.00,0:00:06.00,Default,,0,0,0,,{\\pos(10,10)}Hi, there\n";
        let c = parse_ass(ssa).unwrap();
        assert_eq!((c[0].start_ms, c[0].text.as_str()), (5000, "Hi, there"));
        let ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttp="http://www.w3.org/ns/ttml#parameter" ttp:frameRate="25"><body><div><p begin="00:00:01:12" dur="2s">One <span>two</span><br/>three</p><p begin="4.5s" end="6000ms">Four</p></div></body></tt>"#;
        let c = parse_ttml(ttml).unwrap();
        assert_eq!((c[0].start_ms, c[0].end_ms), (1480, 3480));
        assert_eq!(c[0].text, "One two\nthree");
        assert_eq!((c[1].start_ms, c[1].end_ms), (4500, 6000));
        let sbv = "0:00:00.599,0:00:04.160\n>> Welcome\n\n0:00:04.160,0:00:06.770\nback\n";
        let c = parse_sbv(sbv).unwrap();
        assert_eq!((c[1].start_ms, c[1].text.as_str()), (4160, "back"));
        assert_eq!(
            write_sbv(&c[..1]),
            "0:00:00.599,0:00:04.160\n>> Welcome\n\n"
        );
    }
}
