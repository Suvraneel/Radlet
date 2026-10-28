//! What's in a file, in words, made on-device: a short title for Rename's
//! `{content}`, alt text for pictures, and summaries of documents. Apple
//! Intelligence writes them when this Mac has it turned on; titles fall
//! back to what Vision sees in a picture, or a document's first line.

use std::path::Path;

use crate::catalog::Capabilities;
use crate::engines::{doc, vision};
use crate::error::{Error, Need, Result};
use crate::format::{self, display_name, Format};
use crate::naming;
use crate::pipeline::{self, Context, Produced};

/// Longest title used in a file name.
const MAX_TITLE: usize = 60;

/// A document's text is cut to this much for a title (its start says
/// what it is).
const TITLE_TEXT: usize = 6_000;

/// The most text summarised: the model reads it in parts and then
/// combines them.
const SUMMARY_TEXT: usize = 48_000;

/// Pictures the Vision helper can look at.
pub fn is_picture(f: Format, caps: &Capabilities) -> bool {
    f != Format::Svg && caps.can_decode_raster(f)
}

/// Documents with text to summarise or name: PDFs, and what Copy text
/// reads as text.
pub fn has_text(f: Format, caps: &Capabilities) -> bool {
    f == Format::Pdf
        || caps.reads_document(f)
        || matches!(f, Format::Txt | Format::Md | Format::Html | Format::Rtf)
}

/// A name for what's in `input`, for Rename: "Golden retriever on a
/// beach", "Invoice 2024-113". None when there's nothing to go on.
pub fn title(input: &Path, caps: &Capabilities, ctx: &mut Context<'_>) -> Result<Option<String>> {
    let fmt = format::detect(input)?;
    if is_picture(fmt, caps) && caps.vision.is_some() {
        return match vision::describe(caps, input, false, ctx.cancel) {
            Ok(d) => Ok(d.title.as_deref().and_then(clean_title)),
            Err(Error::NothingToDo(_)) => Ok(None),
            Err(e) => Err(e),
        };
    }
    if !has_text(fmt, caps) {
        return Ok(None);
    }
    let text = match document_text(input, ctx) {
        Ok(t) => t,
        Err(Error::NothingToDo(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    if caps.apple_intelligence {
        let head: String = text.chars().take(TITLE_TEXT).collect();
        match vision::write(caps, "title", &head, ctx.cancel, &mut |_| {}) {
            Ok(t) => {
                if let Some(t) = clean_title(&t) {
                    return Ok(Some(t));
                }
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            // Apple Intelligence off, or it declined: the first line will do.
            Err(_) => {}
        }
    }
    Ok(first_line_title(&text))
}

/// Alt text for a picture, for the clipboard.
pub fn describe(input: &Path, ctx: &mut Context<'_>) -> Result<Produced> {
    let caps = ctx.caps;
    let fmt = format::detect(input)?;
    if !is_picture(fmt, caps) {
        return Err(Error::UnsupportedInput(format!(
            "{fmt} files aren't pictures to describe"
        )));
    }
    require(caps)?;
    let d = vision::describe(caps, input, true, ctx.cancel)?;
    let text = d
        .description
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::NothingToDo("nothing to describe in this picture".into()))?;
    (ctx.progress)(1.0);
    Ok(Produced {
        output: None,
        outputs: Vec::new(),
        value: Some(text),
    })
}

/// A summary of a document, beside it as "report (summary).md".
pub fn summarize(input: &Path, ctx: &mut Context<'_>) -> Result<Produced> {
    let caps = ctx.caps;
    let fmt = format::detect(input)?;
    if !has_text(fmt, caps) {
        return Err(Error::UnsupportedInput(format!(
            "{fmt} files have no text to summarise"
        )));
    }
    require(caps)?;
    let text = document_text(input, ctx)?;
    if text.split_whitespace().count() < 40 {
        return Err(Error::NothingToDo(format!(
            "{} is too short to need a summary",
            display_name(input)
        )));
    }
    let cut: String = text.chars().take(SUMMARY_TEXT).collect();
    let summary = vision::write(caps, "summary", &cut, ctx.cancel, &mut *ctx.progress)?;
    let mut doc = format!("# {}\n\n{}\n", doc::title_of(input), summary.trim());
    if cut.len() < text.len() {
        doc.push_str(
            "\n*Summarised from the start of the document; it was too long to read whole.*\n",
        );
    }
    doc.push_str("\n*Written on this Mac by Apple Intelligence. Check it against the document.*\n");
    let dir = ctx.output.dir_for(input);
    let tmp = naming::temp_in(&dir, ctx.temp_tag, "md")?;
    std::fs::write(tmp.path(), doc)?;
    naming::inherit_permissions(input, tmp.path());
    let desired = naming::desired_path(input, &dir, "md", Some("summary"));
    let out = naming::publish(tmp.into_temp_path(), &desired)?;
    (ctx.progress)(1.0);
    Ok(Produced::one(out))
}

/// Says what's needed when this Mac can't have Apple Intelligence at all.
fn require(caps: &Capabilities) -> Result<()> {
    if caps.vision.is_none() {
        return Err(Error::Missing("this needs the Radlet app on macOS".into()));
    }
    if !caps.apple_intelligence {
        return Err(Error::Needs {
            need: Need::AppleIntelligence,
            message: "this needs Apple Intelligence, on macOS 26 or later with Apple silicon"
                .into(),
        });
    }
    Ok(())
}

/// A document's text, as Copy text reads it (scans are recognised).
fn document_text(input: &Path, ctx: &mut Context<'_>) -> Result<String> {
    let text = pipeline::copy_text(input, ctx)?.value.unwrap_or_default();
    if text.trim().is_empty() {
        return Err(Error::NothingToDo("there's no text in it".into()));
    }
    Ok(text)
}

/// The first line that reads like a title: a few words, not a number.
fn first_line_title(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.chars().filter(|c| c.is_alphabetic()).count() >= 3)
        .take(5)
        .find_map(|l| {
            let words: Vec<&str> = l.split_whitespace().take(10).collect();
            clean_title(&words.join(" "))
        })
}

/// A title fit for a file name: one line, no characters Finder can't
/// show, no wrapping quotes or trailing punctuation, at most 60
/// characters cut at a word.
pub fn clean_title(raw: &str) -> Option<String> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .trim_start_matches(|c: char| matches!(c, '-' | '*' | '#' | '•') || c.is_whitespace());
    let mut s: String = line
        .replace(": ", " - ")
        .chars()
        .map(|c| match c {
            '/' | ':' | '\\' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    // Labels such as "Title:" or "File name:" from a chatty answer.
    for prefix in ["title -", "file name -", "filename -", "name -"] {
        if s.to_lowercase().starts_with(prefix) {
            s = s[prefix.len()..].to_string();
        }
    }
    let quotes: &[char] = &['"', '\'', '“', '”', '‘', '’', '`', '«', '»'];
    let mut s = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| {
            quotes.contains(&c) || matches!(c, '.' | ',' | ';' | '!' | '?' | '-')
        })
        .trim()
        .to_string();
    // A model may add an extension; the file keeps its own.
    if let Some(dot) = s.rfind('.') {
        let ext = &s[dot + 1..];
        if (2..=4).contains(&ext.len())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && !ext.chars().all(|c| c.is_ascii_digit())
        {
            s.truncate(dot);
        }
    }
    if s.chars().count() > MAX_TITLE {
        let cut: String = s.chars().take(MAX_TITLE).collect();
        s = match cut.rfind(' ') {
            Some(i) if i > MAX_TITLE / 2 => cut[..i].to_string(),
            _ => cut,
        };
    }
    let s = s.trim().trim_end_matches(['-', ',']).trim().to_string();
    (s.chars().any(char::is_alphanumeric) && !s.starts_with('.')).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_fit_for_file_names() {
        assert_eq!(
            clean_title("Golden retriever on a beach").as_deref(),
            Some("Golden retriever on a beach")
        );
        assert_eq!(
            clean_title("\"Quarterly report: Q3 2026.\"\n\nMore").as_deref(),
            Some("Quarterly report - Q3 2026")
        );
        assert_eq!(
            clean_title("Title: Team offsite plan").as_deref(),
            Some("Team offsite plan")
        );
        assert_eq!(
            clean_title("- Invoice 2024-113.pdf").as_deref(),
            Some("Invoice 2024-113")
        );
        assert_eq!(
            clean_title("Version 2.10").as_deref(),
            Some("Version 2.10"),
            "numbers aren't extensions"
        );
        assert_eq!(clean_title("a/b\\c").as_deref(), Some("a-b-c"));
        assert_eq!(clean_title("   \n\n  "), None);
        assert_eq!(clean_title("..."), None);
        assert_eq!(clean_title(".hidden"), Some("hidden".into()));
        let long = "word ".repeat(30);
        let t = clean_title(&long).unwrap();
        assert!(t.chars().count() <= MAX_TITLE && !t.ends_with(' '), "{t:?}");
    }

    #[test]
    fn first_lines_make_fallback_titles() {
        assert_eq!(
            first_line_title("\n\n12\nMeeting notes, 3 March\nAgenda").as_deref(),
            Some("Meeting notes, 3 March")
        );
        assert_eq!(first_line_title("1 2 3\n---\n"), None);
    }

    #[test]
    fn titles_for_documents_without_apple_intelligence() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("untitled.md");
        std::fs::write(
            &note,
            "# Garden plan for spring\n\nTomatoes, beans and basil.\n",
        )
        .unwrap();
        let caps = Capabilities::none();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut noop = |_p: f32| {};
        let out = crate::naming::OutputLocation::BesideSource;
        let mut ctx = Context {
            caps: &caps,
            cancel: &cancel,
            progress: &mut noop,
            output: &out,
            temp_tag: "t",
        };
        assert_eq!(
            title(&note, &caps, &mut ctx).unwrap().as_deref(),
            Some("Garden plan for spring")
        );
        let blank = dir.path().join("blank.txt");
        std::fs::write(&blank, "   \n").unwrap();
        assert_eq!(title(&blank, &caps, &mut ctx).unwrap(), None);
        // Summaries and descriptions say what they need.
        let long = dir.path().join("long.txt");
        std::fs::write(&long, "word ".repeat(200)).unwrap();
        let mac = Capabilities {
            vision: Some("/x/radlet-vision".into()),
            ..Capabilities::none()
        };
        let mut ctx = Context {
            caps: &mac,
            cancel: &cancel,
            progress: &mut noop,
            output: &out,
            temp_tag: "t",
        };
        match summarize(&long, &mut ctx) {
            Err(Error::Needs { need, .. }) => assert_eq!(need, Need::AppleIntelligence),
            other => panic!("{other:?}"),
        }
    }
}
