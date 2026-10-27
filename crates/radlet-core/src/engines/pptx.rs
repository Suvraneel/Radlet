//! PowerPoint presentations (.pptx): the text of every slide, in order,
//! into the document model. Each slide becomes a section headed by its
//! title; its text boxes become paragraphs and lists, its tables tables,
//! and its speaker notes quotes. Pictures, charts and layout are left out.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::Reader;
use zip::ZipArchive;

use crate::engines::doc::{Block, Doc, Inline, InlineBuf, Style};
use crate::engines::docx::{core_title, entity, get, local, part};
use crate::error::{Error, Result};

/// Most slides read.
const MAX_SLIDES: usize = 5_000;
/// Most bytes of slide, notes and relationship XML read in all.
const MAX_TOTAL: usize = 256 << 20;

fn bad(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::UnsupportedInput(format!(
        "{} isn't a PowerPoint presentation Radlet can read ({why})",
        crate::format::display_name(path)
    ))
}

/// A part's relationships: id → (type, target resolved against the part's
/// folder).
fn rels(xml: &str, dir: &str) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local(&e) == "Relationship" => {
                if let (Some(id), Some(target)) = (get(&e, "Id"), get(&e, "Target")) {
                    let ty = get(&e, "Type").unwrap_or_default();
                    let external = get(&e, "TargetMode").is_some_and(|m| m == "External");
                    let target = if external {
                        target
                    } else {
                        resolve(dir, &target)
                    };
                    map.insert(id, (ty, target));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    map
}

/// `target` relative to the package folder `dir` ("ppt/slides" and
/// "../notesSlides/n1.xml" give "ppt/notesSlides/n1.xml").
fn resolve(dir: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        dir.split('/').filter(|p| !p.is_empty()).collect()
    };
    for piece in target.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn rels_name(part: &str) -> (String, String) {
    match part.rsplit_once('/') {
        Some((dir, file)) => (dir.to_string(), format!("{dir}/_rels/{file}.rels")),
        None => (String::new(), format!("_rels/{part}.rels")),
    }
}

/// The slides' parts, in the order they're shown.
fn slide_order(presentation: &str, rels: &HashMap<String, (String, String)>) -> Vec<String> {
    let mut out = Vec::new();
    let mut r = Reader::from_str(presentation);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local(&e) == "sldId" => {
                if let Some((_, target)) = get_rel_id(&e).and_then(|id| rels.get(&id)) {
                    out.push(target.clone());
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// The `r:id` attribute (not the plain `id` beside it).
fn get_rel_id(e: &BytesStart) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.as_ref().ends_with(":id"))
            .then(|| {
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .ok()
                    .map(|v| v.into_owned())
            })
            .flatten()
    })
}

fn end_name(e: &BytesEnd) -> String {
    e.local_name().as_ref().to_string()
}

/// What a shape holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Title,
    /// A placeholder whose paragraphs are bullets unless they say not.
    Body,
    Text,
    /// Dates, footers and slide numbers.
    Skip,
}

/// One slide's (or notes page's) text.
#[derive(Default)]
struct Slide {
    title: String,
    blocks: Vec<Block>,
    hidden: bool,
}

/// Reads a slide or notes page. With `notes`, only the notes text is kept.
fn read_slide(xml: &str, links: &HashMap<String, (String, String)>, notes: bool) -> Slide {
    let mut slide = Slide::default();
    let mut r = Reader::from_str(xml);
    let mut role = Role::Text;
    let mut depth_sp = 0usize;
    // The paragraph being read.
    let mut buf = InlineBuf::default();
    let mut style = Style::default();
    let mut level: u8 = 0;
    let mut bullet: Option<bool> = None; // Some(ordered) or None for "as the shape says"
    let mut no_bullet = false;
    let mut in_text = false;
    let mut in_para = false;
    // Tables.
    let mut table: Option<Vec<Vec<Inline>>> = None;
    let mut row: Vec<Inline> = Vec::new();
    let mut cell: Option<InlineBuf> = None;
    let mut header = false;

    while let Ok(ev) = r.read_event() {
        match ev {
            Event::Start(e) if local(&e) == "sld" => {
                slide.hidden = get(&e, "show").is_some_and(|v| v == "0" || v == "false");
            }
            Event::Start(e) if local(&e) == "sp" => {
                depth_sp += 1;
                role = Role::Text;
            }
            Event::End(e) if end_name(&e) == "sp" => {
                depth_sp = depth_sp.saturating_sub(1);
                role = Role::Text;
            }
            Event::Start(e) | Event::Empty(e) if local(&e) == "ph" && depth_sp > 0 => {
                role = match get(&e, "type").as_deref() {
                    Some("title" | "ctrTitle") => Role::Title,
                    Some("dt" | "ftr" | "sldNum" | "hdr" | "sldImg") => Role::Skip,
                    Some("subTitle") => Role::Text,
                    _ => Role::Body,
                };
            }
            Event::Start(e) if local(&e) == "tbl" => {
                table = Some(Vec::new());
                header = false;
            }
            Event::Start(e) | Event::Empty(e) if local(&e) == "tblPr" && table.is_some() => {
                header = get(&e, "firstRow").is_some_and(|v| v == "1" || v == "true");
            }
            Event::Start(e) if local(&e) == "tr" && table.is_some() => row = Vec::new(),
            Event::Start(e) if local(&e) == "tc" && table.is_some() => {
                cell = Some(InlineBuf::default())
            }
            Event::End(e) if end_name(&e) == "tc" => {
                if let Some(mut c) = cell.take() {
                    row.push(c.take());
                }
            }
            Event::End(e) if end_name(&e) == "tr" => {
                if let Some(t) = table.as_mut() {
                    t.push(std::mem::take(&mut row));
                }
            }
            Event::End(e) if end_name(&e) == "tbl" => {
                if let Some(rows) = table.take() {
                    if !notes && rows.iter().any(|r| r.iter().any(|c| !c.is_empty())) {
                        slide.blocks.push(Block::Table { rows, header });
                    }
                }
            }
            Event::Start(e) if local(&e) == "p" => {
                in_para = true;
                level = 0;
                bullet = None;
                no_bullet = false;
            }
            Event::Start(e) | Event::Empty(e) if local(&e) == "pPr" && in_para => {
                level = get(&e, "lvl")
                    .and_then(|l| l.parse::<u8>().ok())
                    .unwrap_or(0)
                    .min(5);
            }
            Event::Start(e) | Event::Empty(e) if in_para && local(&e) == "buNone" => {
                no_bullet = true
            }
            Event::Start(e) | Event::Empty(e) if in_para && local(&e) == "buChar" => {
                bullet = Some(false)
            }
            Event::Start(e) | Event::Empty(e) if in_para && local(&e) == "buAutoNum" => {
                bullet = Some(true)
            }
            Event::Start(e) | Event::Empty(e) if local(&e) == "rPr" && in_para => {
                style.bold = get(&e, "b").is_some_and(|v| v == "1" || v == "true");
                style.italic = get(&e, "i").is_some_and(|v| v == "1" || v == "true");
            }
            Event::Start(e) | Event::Empty(e) if local(&e) == "hlinkClick" && in_para => {
                style.link = get_rel_id(&e)
                    .and_then(|id| links.get(&id))
                    .map(|(_, t)| t.clone())
                    .filter(|t| t.starts_with("http") || t.starts_with("mailto:"));
            }
            Event::End(e) if end_name(&e) == "r" => style = Style::default(),
            Event::Start(e) if local(&e) == "t" && in_para => in_text = true,
            Event::End(e) if end_name(&e) == "t" => in_text = false,
            Event::Text(t) if in_text => {
                let text = t.xml10_content();
                match cell.as_mut() {
                    Some(c) => c.push(&text, &style),
                    None => buf.push(&text, &style),
                }
            }
            Event::GeneralRef(g) if in_text => {
                let text = entity(&g);
                match cell.as_mut() {
                    Some(c) => c.push(&text, &style),
                    None => buf.push(&text, &style),
                }
            }
            Event::Empty(e) if local(&e) == "br" && in_para => match cell.as_mut() {
                Some(c) => c.push("\n", &Style::default()),
                None => buf.push("\n", &Style::default()),
            },
            Event::End(e) if end_name(&e) == "p" => {
                in_para = false;
                if let Some(c) = cell.as_mut() {
                    // Paragraphs in a cell are lines of it.
                    if !c.is_blank() && c.last_char() != Some('\n') {
                        c.push("\n", &Style::default());
                    }
                    continue;
                }
                if buf.is_blank() {
                    buf = InlineBuf::default();
                    continue;
                }
                let text = buf.take();
                if notes {
                    if role == Role::Body {
                        slide.blocks.push(Block::Quote(text));
                    }
                    continue;
                }
                match role {
                    Role::Skip => {}
                    Role::Title => {
                        let plain: String = text.iter().map(|s| s.text.as_str()).collect();
                        if !slide.title.is_empty() {
                            slide.title.push(' ');
                        }
                        slide.title.push_str(plain.replace('\n', " ").trim());
                    }
                    Role::Body | Role::Text => {
                        let listed = !no_bullet && (bullet.is_some() || role == Role::Body);
                        slide.blocks.push(if listed {
                            Block::Item {
                                ordered: bullet == Some(true),
                                depth: level,
                                text,
                            }
                        } else {
                            Block::Paragraph(text)
                        });
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    slide
}

/// The text of a presentation, a section per slide (hidden slides left
/// out), with speaker notes as quotes.
pub fn read(path: &Path) -> Result<Doc> {
    let file = File::open(path)?;
    let mut zip = ZipArchive::new(BufReader::new(file)).map_err(|e| bad(path, e))?;
    let presentation =
        part(&mut zip, "ppt/presentation.xml")?.ok_or_else(|| bad(path, "no slides"))?;
    let (dir, rels_part) = rels_name("ppt/presentation.xml");
    let deck_rels = part(&mut zip, &rels_part)?
        .map(|x| rels(&x, &dir))
        .unwrap_or_default();
    let order = slide_order(&presentation, &deck_rels);
    if order.len() > MAX_SLIDES {
        return Err(Error::TooLarge(format!(
            "this presentation has more than {MAX_SLIDES} slides"
        )));
    }
    let mut doc = Doc {
        title: part(&mut zip, "docProps/core.xml")?.and_then(|x| core_title(&x)),
        blocks: Vec::new(),
    };
    let mut left = MAX_TOTAL;
    let mut spend = |text: Option<String>| -> Result<Option<String>> {
        if let Some(t) = &text {
            left = left.checked_sub(t.len()).ok_or_else(|| {
                Error::TooLarge("this presentation's text is larger than 256 MB".into())
            })?;
        }
        Ok(text)
    };
    let mut seen = std::collections::HashSet::new();
    for (i, name) in order.iter().enumerate() {
        // A slide listed twice is read once.
        if !seen.insert(name.as_str()) {
            continue;
        }
        let Some(xml) = spend(part(&mut zip, name)?)? else {
            continue;
        };
        let (dir, rels_part) = rels_name(name);
        let links = spend(part(&mut zip, &rels_part)?)?
            .map(|x| rels(&x, &dir))
            .unwrap_or_default();
        let slide = read_slide(&xml, &links, false);
        if slide.hidden {
            continue;
        }
        let title = if slide.title.is_empty() {
            format!("Slide {}", i + 1)
        } else {
            slide.title
        };
        doc.blocks.push(Block::Heading {
            level: 1,
            text: vec![crate::engines::doc::Span {
                text: title,
                style: Style::default(),
            }],
        });
        doc.blocks.extend(slide.blocks);
        let notes = links
            .values()
            .find(|(ty, _)| ty.ends_with("/notesSlide"))
            .map(|(_, t)| t.clone());
        if let Some(notes) = notes {
            if let Some(xml) = spend(part(&mut zip, &notes)?)? {
                doc.blocks
                    .extend(read_slide(&xml, &HashMap::new(), true).blocks);
            }
        }
    }
    if doc.blocks.is_empty() {
        return Err(Error::NothingToDo("there's no text on these slides".into()));
    }
    if doc.title.is_none() {
        doc.title = doc.blocks.iter().find_map(|b| match b {
            Block::Heading { text, .. } => text
                .first()
                .map(|s| s.text.clone())
                .filter(|t| !t.starts_with("Slide ")),
            _ => None,
        });
    }
    Ok(doc)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// A small presentation: a title slide, a slide with bullets, a table
    /// and notes, and a hidden slide.
    pub(crate) fn sample(path: &Path) {
        let p = |body: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"{body}"#
            )
        };
        let files: Vec<(&str, String)> = vec![
            ("[Content_Types].xml", r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#.into()),
            ("_rels/.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#.into()),
            ("ppt/presentation.xml", r#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="257" r:id="rId3"/><p:sldId id="256" r:id="rId2"/><p:sldId id="258" r:id="rId4"/></p:sldIdLst></p:presentation>"#.into()),
            ("ppt/_rels/presentation.xml.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide3.xml"/></Relationships>"#.into()),
            ("ppt/slides/slide1.xml", p(r#"><p:cSld><p:spTree>
                <p:sp><p:nvSpPr><p:cNvPr id="2" name="T"/><p:cNvSpPr/><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>The Engine &amp; Cards</a:t></a:r></a:p></p:txBody></p:sp>
                <p:sp><p:nvSpPr><p:cNvPr id="3" name="S"/><p:cNvSpPr/><p:nvPr><p:ph type="subTitle" idx="1"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:rPr i="1"/><a:t>A talk</a:t></a:r></a:p></p:txBody></p:sp>
                <p:sp><p:nvSpPr><p:cNvPr id="4" name="N"/><p:cNvSpPr/><p:nvPr><p:ph type="sldNum" idx="12"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>1</a:t></a:r></a:p></p:txBody></p:sp>
                </p:spTree></p:cSld></p:sld>"#)),
            ("ppt/slides/slide2.xml", p(r#"><p:cSld><p:spTree>
                <p:sp><p:nvSpPr><p:cNvPr id="2" name="T"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Why</a:t></a:r></a:p></p:txBody></p:sp>
                <p:sp><p:nvSpPr><p:cNvPr id="3" name="B"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:txBody>
                  <a:p><a:r><a:rPr b="1"/><a:t>Fast</a:t></a:r><a:r><a:t> and </a:t></a:r><a:r><a:rPr><a:hlinkClick r:id="rId9"/></a:rPr><a:t>open</a:t></a:r></a:p>
                  <a:p><a:pPr lvl="1"/><a:r><a:t>Really</a:t></a:r></a:p>
                  <a:p><a:pPr><a:buNone/></a:pPr><a:r><a:t>Plain line</a:t></a:r></a:p>
                </p:txBody></p:sp>
                <p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tblPr firstRow="1"/><a:tr><a:tc><a:txBody><a:p><a:r><a:t>Item</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>Qty</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr><a:tc><a:txBody><a:p><a:r><a:t>Tea</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>2</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>
                </p:spTree></p:cSld></p:sld>"#)),
            ("ppt/slides/_rels/slide2.xml.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide1.xml"/></Relationships>"#.into()),
            ("ppt/notesSlides/notesSlide1.xml", r#"<?xml version="1.0"?><p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="I"/><p:cNvSpPr/><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="N"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Say it slowly.</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#.into()),
            ("ppt/slides/slide3.xml", p(r#" show="0"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="T"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>Hidden</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#)),
        ];
        let mut zw = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, body) in files {
            zw.start_file(name, SimpleFileOptions::default()).unwrap();
            zw.write_all(body.as_bytes()).unwrap();
        }
        zw.finish().unwrap();
    }

    #[test]
    fn slides_become_sections() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("talk.pptx");
        sample(&deck);
        assert_eq!(
            crate::format::detect(&deck).unwrap(),
            crate::format::Format::Pptx
        );
        let doc = read(&deck).unwrap();
        assert_eq!(doc.title.as_deref(), Some("The Engine & Cards"));
        let plain = |i: &Inline| i.iter().map(|s| s.text.as_str()).collect::<String>();
        let summary: Vec<String> = doc
            .blocks
            .iter()
            .map(|b| match b {
                Block::Heading { level, text } => format!("h{level} {}", plain(text)),
                Block::Paragraph(t) => format!("p {}", plain(t)),
                Block::Item {
                    ordered,
                    depth,
                    text,
                } => {
                    format!(
                        "{} {depth} {}",
                        if *ordered { "ol" } else { "ul" },
                        plain(text)
                    )
                }
                Block::Quote(t) => format!("q {}", plain(t)),
                Block::Table { rows, header } => {
                    format!("table {}x{} {header}", rows.len(), rows[0].len())
                }
                _ => "?".into(),
            })
            .collect();
        assert_eq!(
            summary,
            [
                "h1 The Engine & Cards",
                "p A talk",
                "h1 Why",
                "ul 0 Fast and open",
                "ul 1 Really",
                "p Plain line",
                "table 2x2 true",
                "q Say it slowly.",
            ]
        );
        let Block::Item { text, .. } = &doc.blocks[3] else {
            panic!()
        };
        assert!(text[0].style.bold);
        assert_eq!(
            text.last().unwrap().style.link.as_deref(),
            Some("https://example.com/")
        );
    }

    #[test]
    fn presentations_become_documents() {
        let dir = tempfile::tempdir().unwrap();
        let deck = dir.path().join("talk.pptx");
        sample(&deck);
        let caps = crate::catalog::Capabilities::none();
        let targets = crate::catalog::targets(crate::format::Format::Pptx, &caps);
        assert_eq!(targets[0], crate::format::Format::Pdf);
        assert!(targets.contains(&crate::format::Format::Md));
        let md = dir.path().join("talk.md");
        let doc = crate::engines::doc::read(&deck, crate::format::Format::Pptx).unwrap();
        crate::engines::doc::write(&doc, crate::format::Format::Md, "talk", &md).unwrap();
        let text = std::fs::read_to_string(&md).unwrap();
        assert!(text.contains("# Why"), "{text}");
        assert!(text.contains("[open](https://example.com/)"), "{text}");
        assert!(text.contains("> Say it slowly."), "{text}");
        assert!(!text.contains("Hidden"), "{text}");
    }

    #[test]
    fn paths_inside_the_package() {
        assert_eq!(
            resolve("ppt/slides", "../notesSlides/n1.xml"),
            "ppt/notesSlides/n1.xml"
        );
        assert_eq!(resolve("ppt", "slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(resolve("ppt/slides", "/ppt/media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("ppt", "../../../etc/x"), "etc/x");
    }
}
