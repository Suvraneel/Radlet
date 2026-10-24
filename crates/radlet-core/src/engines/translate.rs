//! Translating documents and subtitles with the Mac's on-device
//! translation (through the Vision helper). The structure stays: each
//! paragraph, heading, list item, table cell and subtitle is translated
//! on its own; code isn't translated.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::catalog::Capabilities;
use crate::engines::doc::{self, Block, Inline, InlineBuf, Style};
use crate::engines::{text, vision};
use crate::error::{Error, Result};
use crate::format::Format;

/// Translates `path` (a `fmt` file) into `to`, writing the same format
/// to `out`.
#[allow(clippy::too_many_arguments)]
pub fn translate_file(
    path: &Path,
    fmt: Format,
    to: &str,
    from: Option<&str>,
    title: &str,
    caps: &Capabilities,
    scratch: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    out: &Path,
) -> Result<()> {
    let mut run = |texts: Vec<String>| -> Result<Vec<String>> {
        if texts.iter().all(|t| t.trim().is_empty()) {
            return Err(Error::NothingToDo("there's no text to translate".into()));
        }
        vision::translate(caps, &texts, to, from, scratch, cancel, progress)
    };
    match fmt {
        Format::Srt | Format::Vtt => {
            let mut cues = text::parse_subtitles(&text::read_text(path)?)?;
            let done = run(cues.iter().map(|c| c.text.clone()).collect())?;
            for (c, t) in cues.iter_mut().zip(done) {
                c.text = t;
            }
            let body = if fmt == Format::Srt {
                text::write_srt(&cues)
            } else {
                text::write_vtt(&cues)
            };
            std::fs::write(out, body)?;
        }
        Format::Txt => {
            // Paragraph by paragraph, keeping the file's own layout.
            let src = text::read_text(path)?.replace("\r\n", "\n");
            let paras: Vec<String> = src.split("\n\n").map(str::to_string).collect();
            let done = run(paras.clone())?;
            let joined: Vec<String> = paras
                .iter()
                .zip(done)
                .map(|(orig, t)| {
                    if orig.trim().is_empty() {
                        orig.clone()
                    } else {
                        t
                    }
                })
                .collect();
            std::fs::write(out, joined.join("\n\n"))?;
        }
        f if f.is_document_text() => {
            let mut d = doc::read(path, f)?;
            let texts = collect(&d);
            let done = run(texts)?;
            apply(&mut d, done);
            doc::write(&d, f, title, out)?;
        }
        other => {
            return Err(Error::UnsupportedInput(format!(
                "{other} files can't be translated"
            )))
        }
    }
    Ok(())
}

/// Every piece of text to translate, in document order.
fn collect(d: &doc::Doc) -> Vec<String> {
    let mut out = Vec::new();
    for b in &d.blocks {
        match b {
            Block::Heading { text, .. }
            | Block::Paragraph(text)
            | Block::Item { text, .. }
            | Block::Quote(text) => out.push(doc::plain(text)),
            Block::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        out.push(doc::plain(cell));
                    }
                }
            }
            Block::Code(_) | Block::Rule => {}
        }
    }
    out
}

/// The translation of an inline: one style when the original had one,
/// plain otherwise (word order changes, so mixed styles can't be kept).
fn restyle(orig: &Inline, translated: String) -> Inline {
    let first = orig.first().map(|s| s.style.clone()).unwrap_or_default();
    let style = if orig.iter().all(|s| s.style == first) {
        first
    } else {
        Style::default()
    };
    let mut buf = InlineBuf::default();
    buf.push(&translated, &style);
    buf.take()
}

fn apply(d: &mut doc::Doc, done: Vec<String>) {
    let mut it = done.into_iter();
    for b in &mut d.blocks {
        match b {
            Block::Heading { text, .. }
            | Block::Paragraph(text)
            | Block::Item { text, .. }
            | Block::Quote(text) => {
                if let Some(t) = it.next() {
                    *text = restyle(text, t);
                }
            }
            Block::Table { rows, .. } => {
                for row in rows.iter_mut() {
                    for cell in row.iter_mut() {
                        if let Some(t) = it.next() {
                            *cell = restyle(cell, t);
                        }
                    }
                }
            }
            Block::Code(_) | Block::Rule => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_survives() {
        let mut d = doc::from_markdown("# Hello\n\nA **bold** word and `code`.\n\n```\nlet x = 1;\n```\n\n| a | b |\n| --- | --- |\n| c | d |\n");
        let texts = collect(&d);
        assert_eq!(
            texts,
            ["Hello", "A bold word and code.", "a", "b", "c", "d"]
        );
        apply(&mut d, texts.iter().map(|t| t.to_uppercase()).collect());
        let md = doc::to_markdown(&d);
        assert!(
            md.starts_with("# HELLO\n\nA BOLD WORD AND CODE.\n\n```\nlet x = 1;\n```"),
            "{md}"
        );
        assert!(md.contains("| C | D |"));
        // One style throughout is kept.
        let mut b = doc::from_markdown("*all italic*");
        apply(&mut b, vec!["tout en italique".into()]);
        let Block::Paragraph(p) = &b.blocks[0] else {
            panic!()
        };
        assert!(p[0].style.italic);
    }
}
