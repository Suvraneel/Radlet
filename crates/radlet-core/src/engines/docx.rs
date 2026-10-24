//! Word documents (.docx): reading text and structure into the document
//! model, and writing a clean document from it. Pictures, comments,
//! headers and footers are not carried over.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::Reader;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::engines::doc::{self, Block, Doc, Inline, InlineBuf, Style};
use crate::error::{Error, Result};

/// Largest part of a Word file read into memory.
const MAX_PART: u64 = 64 << 20;

fn bad(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::UnsupportedInput(format!(
        "{} isn't a Word document Radlet can read ({why})",
        crate::format::display_name(path)
    ))
}

fn part<R: Read + std::io::Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<Option<String>> {
    let mut f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(Error::UnsupportedInput(format!("can't read {name} ({e})"))),
    };
    if f.size() > MAX_PART {
        return Err(Error::TooLarge("this Word document is too large".into()));
    }
    let mut bytes = Vec::new();
    (&mut f).take(MAX_PART + 1).read_to_end(&mut bytes)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn local(e: &BytesStart) -> String {
    e.local_name().as_ref().to_string()
}

/// An attribute by local name ("val" finds `w:val`).
fn get(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == name)
            .then(|| {
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .ok()
                    .map(|v| v.into_owned())
            })
            .flatten()
    })
}

fn entity(r: &BytesRef) -> String {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return c.to_string();
    }
    match r.as_ref() {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    }
    .to_string()
}

/// `<w:b/>` or `<w:b w:val="1"/>` is on; `w:val="0"`/`false` is off.
fn on(e: &BytesStart) -> bool {
    !matches!(
        get(e, "val").as_deref(),
        Some("0" | "false" | "off" | "none")
    )
}

#[derive(Debug, Default, Clone)]
struct StyleInfo {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    num_id: Option<String>,
}

fn parse_styles(xml: &str) -> HashMap<String, StyleInfo> {
    let mut map = HashMap::new();
    let mut r = Reader::from_str(xml);
    let mut current: Option<(String, StyleInfo)> = None;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = local(&e);
                match name.as_str() {
                    "style" => {
                        if let Some((id, info)) = current.take() {
                            map.insert(id, info);
                        }
                        if let Some(id) = get(&e, "styleId") {
                            current = Some((id, StyleInfo::default()));
                        }
                    }
                    "name" => {
                        if let (Some((_, info)), Some(v)) = (&mut current, get(&e, "val")) {
                            info.name = v.to_lowercase();
                        }
                    }
                    "basedOn" => {
                        if let Some((_, info)) = &mut current {
                            info.based_on = get(&e, "val");
                        }
                    }
                    "outlineLvl" => {
                        if let Some((_, info)) = &mut current {
                            info.outline = get(&e, "val").and_then(|v| v.parse().ok());
                        }
                    }
                    "numId" => {
                        if let Some((_, info)) = &mut current {
                            info.num_id = get(&e, "val");
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "style" => {
                if let Some((id, info)) = current.take() {
                    map.insert(id, info);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    map
}

/// numId → (level → numbered).
fn parse_numbering(xml: &str) -> HashMap<String, HashMap<u8, bool>> {
    let mut abstracts: HashMap<String, HashMap<u8, bool>> = HashMap::new();
    let mut nums: HashMap<String, String> = HashMap::new();
    let mut r = Reader::from_str(xml);
    let mut abs: Option<String> = None;
    let mut lvl: Option<u8> = None;
    let mut num: Option<String> = None;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match local(&e).as_str() {
                "abstractNum" => abs = get(&e, "abstractNumId"),
                "lvl" => lvl = get(&e, "ilvl").and_then(|v| v.parse().ok()),
                "numFmt" => {
                    if let (Some(a), Some(l)) = (&abs, lvl) {
                        let numbered =
                            !matches!(get(&e, "val").as_deref(), Some("bullet" | "none"));
                        abstracts.entry(a.clone()).or_default().insert(l, numbered);
                    }
                }
                "num" => num = get(&e, "numId"),
                "abstractNumId" => {
                    if let (Some(n), Some(v)) = (&num, get(&e, "val")) {
                        nums.insert(n.clone(), v);
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                "abstractNum" => abs = None,
                "lvl" => lvl = None,
                "num" => num = None,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    nums.into_iter()
        .filter_map(|(n, a)| abstracts.get(&a).map(|levels| (n, levels.clone())))
        .collect()
}

/// Relationship id → target, for hyperlinks.
fn parse_rels(xml: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local(&e) == "Relationship" => {
                if let (Some(id), Some(target)) = (get(&e, "Id"), get(&e, "Target")) {
                    map.insert(id, target);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    map
}

fn main_part_name(rels: Option<&str>) -> String {
    let fallback = "word/document.xml".to_string();
    let Some(xml) = rels else { return fallback };
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local(&e) == "Relationship" => {
                let ty = get(&e, "Type").unwrap_or_default();
                if ty.ends_with("/officeDocument") {
                    if let Some(t) = get(&e, "Target") {
                        return t.trim_start_matches('/').to_string();
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => return fallback,
            _ => {}
        }
    }
}

fn core_title(xml: &str) -> Option<String> {
    let mut r = Reader::from_str(xml);
    let mut in_title = false;
    let mut title = String::new();
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "title" => in_title = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == "title" => break,
            Ok(Event::Text(t)) if in_title => title.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(g)) if in_title => title.push_str(&entity(&g)),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let t = title.trim().to_string();
    (!t.is_empty()).then_some(t)
}

struct Styles {
    styles: HashMap<String, StyleInfo>,
    numbering: HashMap<String, HashMap<u8, bool>>,
}

impl Styles {
    /// Walks a style and the styles it's based on.
    fn chain(&self, id: &str) -> Vec<(&str, &StyleInfo)> {
        let mut out = Vec::new();
        let mut cur = Some(id.to_string());
        for _ in 0..10 {
            let Some(c) = cur else { break };
            match self.styles.get_key_value(&c) {
                Some((k, info)) => {
                    out.push((k.as_str(), info));
                    cur = info.based_on.clone();
                }
                None => break,
            }
        }
        out
    }

    fn heading(&self, id: &str) -> Option<u8> {
        for (key, info) in self.chain(id) {
            if info.name == "title" {
                return Some(1);
            }
            if let Some(n) = info.name.strip_prefix("heading ") {
                return n.trim().parse().ok().filter(|l| (1..=6).contains(l));
            }
            if let Some(o) = info.outline.filter(|o| *o < 6) {
                return Some(o + 1);
            }
            if let Some(n) = key.strip_prefix("Heading") {
                return n.parse().ok().filter(|l| (1..=6).contains(l));
            }
        }
        // No styles part: Word's own ids.
        if self.styles.is_empty() {
            if id == "Title" {
                return Some(1);
            }
            return id
                .strip_prefix("Heading")
                .and_then(|n| n.parse().ok())
                .filter(|l| (1..=6).contains(l));
        }
        None
    }

    fn named(&self, id: &str, test: impl Fn(&str) -> bool) -> bool {
        self.chain(id).iter().any(|(_, i)| test(&i.name)) || test(&id.to_lowercase())
    }

    fn style_num(&self, id: &str) -> Option<String> {
        self.chain(id).iter().find_map(|(_, i)| i.num_id.clone())
    }

    fn numbered(&self, num_id: &str, level: u8) -> bool {
        self.numbering
            .get(num_id)
            .and_then(|l| l.get(&level).or_else(|| l.get(&0)))
            .copied()
            .unwrap_or(false)
    }
}

#[derive(Default)]
struct Para {
    style: Option<String>,
    num_id: Option<String>,
    level: u8,
    buf: InlineBuf,
}

#[derive(Default)]
struct Cell {
    paras: Vec<Inline>,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<Inline>>,
    row: Vec<Inline>,
    cell: Option<Cell>,
    header: bool,
}

/// Reads a .docx file.
pub fn read(path: &Path) -> Result<Doc> {
    let file = File::open(path)?;
    let mut zip = ZipArchive::new(file).map_err(|e| bad(path, e))?;
    let package_rels = part(&mut zip, "_rels/.rels")?;
    let main = main_part_name(package_rels.as_deref());
    let document = part(&mut zip, &main)?.ok_or_else(|| bad(path, "no document part"))?;
    let rels_name = match main.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{main}.rels"),
    };
    let styles = Styles {
        styles: part(&mut zip, "word/styles.xml")?
            .map(|x| parse_styles(&x))
            .unwrap_or_default(),
        numbering: part(&mut zip, "word/numbering.xml")?
            .map(|x| parse_numbering(&x))
            .unwrap_or_default(),
    };
    let rels = part(&mut zip, &rels_name)?
        .map(|x| parse_rels(&x))
        .unwrap_or_default();
    let title = part(&mut zip, "docProps/core.xml")?.and_then(|x| core_title(&x));
    let mut doc = parse_document(&document, &styles, &rels);
    doc.title = title;
    Ok(doc)
}

fn parse_document(xml: &str, styles: &Styles, rels: &HashMap<String, String>) -> Doc {
    let mut doc = Doc::default();
    let mut r = Reader::from_str(xml);
    let mut paras: Vec<Para> = Vec::new();
    let mut tables: Vec<Table> = Vec::new();
    let mut style = Style::default();
    let mut links: Vec<Option<String>> = Vec::new();
    let mut in_text = false;
    let mut in_run = 0u32;
    let mut in_rpr = false;
    let mut skip: Option<(String, u32)> = None;
    // Complex fields: instruction text, and whether a hyperlink was pushed.
    let mut field_instr: Option<String> = None;
    let mut in_instr = false;
    let mut field_links: Vec<bool> = Vec::new();
    let mut code_run: Vec<String> = Vec::new();

    let flush_code = |doc: &mut Doc, code: &mut Vec<String>| {
        if !code.is_empty() {
            doc.blocks.push(Block::Code(code.join("\n")));
            code.clear();
        }
    };

    loop {
        let ev = match r.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => ev,
        };
        if let Some((name, depth)) = &mut skip {
            match &ev {
                Event::Start(e) if local(e) == *name => *depth += 1,
                Event::End(e) if e.local_name().as_ref() == name => {
                    *depth -= 1;
                    if *depth == 0 {
                        skip = None;
                    }
                }
                _ => {}
            }
            continue;
        }
        let (e, empty) = match &ev {
            Event::Start(e) => (Some(e.clone()), false),
            Event::Empty(e) => (Some(e.clone()), true),
            _ => (None, false),
        };
        if let Some(e) = e {
            let name = local(&e);
            match name.as_str() {
                "Fallback" | "moveFrom" | "del" | "footnoteReference" | "endnoteReference" => {
                    if !empty {
                        skip = Some((name, 1));
                    }
                }
                "p" if !empty => paras.push(Para::default()),
                "pStyle" => {
                    if let Some(p) = paras.last_mut() {
                        p.style = get(&e, "val");
                    }
                }
                "ilvl" => {
                    if let Some(p) = paras.last_mut() {
                        p.level = get(&e, "val").and_then(|v| v.parse().ok()).unwrap_or(0);
                    }
                }
                "numId" => {
                    if let Some(p) = paras.last_mut() {
                        p.num_id = get(&e, "val");
                    }
                }
                "r" if !empty => {
                    in_run += 1;
                    style.bold = false;
                    style.italic = false;
                    style.code = false;
                }
                "rPr" if !empty => in_rpr = true,
                "b" if in_rpr => style.bold = on(&e),
                "i" if in_rpr => style.italic = on(&e),
                "rStyle" if in_rpr => {
                    if let Some(id) = get(&e, "val") {
                        if styles.named(&id, |n| n.contains("code") || n.contains("verbatim")) {
                            style.code = true;
                        }
                        if styles.named(&id, |n| n == "strong") {
                            style.bold = true;
                        }
                        if styles.named(&id, |n| n == "emphasis") {
                            style.italic = true;
                        }
                    }
                }
                "rFonts" if in_rpr => {
                    let font = get(&e, "ascii")
                        .or_else(|| get(&e, "hAnsi"))
                        .unwrap_or_default();
                    let f = font.to_lowercase();
                    if ["courier", "consolas", "menlo", "monaco", "mono"]
                        .iter()
                        .any(|m| f.contains(m))
                    {
                        style.code = true;
                    }
                }
                "t" if !empty && in_run > 0 => in_text = true,
                "tab" if in_run > 0 && !in_rpr => push_text(&mut paras, "\t", &style, &links),
                "br" | "cr" if in_run > 0 => {
                    let page = get(&e, "type").is_some_and(|t| t == "page" || t == "column");
                    if !page {
                        push_text(&mut paras, "\n", &style, &links);
                    }
                }
                "noBreakHyphen" if in_run > 0 => push_text(&mut paras, "-", &style, &links),
                "hyperlink" if !empty => {
                    let target = get(&e, "id")
                        .and_then(|id| rels.get(&id).cloned())
                        .or_else(|| get(&e, "anchor").map(|a| format!("#{a}")));
                    links.push(target);
                }
                "fldSimple" if !empty => {
                    let target = get(&e, "instr").and_then(|i| hyperlink_target(&i));
                    links.push(target);
                }
                "fldChar" => match get(&e, "fldCharType").as_deref() {
                    Some("begin") => field_instr = Some(String::new()),
                    Some("separate") => {
                        let target = field_instr.take().and_then(|i| hyperlink_target(&i));
                        field_links.push(target.is_some());
                        if target.is_some() {
                            links.push(target);
                        }
                    }
                    Some("end") => {
                        field_instr = None;
                        if field_links.pop() == Some(true) {
                            links.pop();
                        }
                    }
                    _ => {}
                },
                "instrText" if !empty => in_instr = true,
                "tbl" if !empty => tables.push(Table::default()),
                "tblHeader" => {
                    if let Some(t) = tables.last_mut() {
                        if t.rows.is_empty() {
                            t.header = true;
                        }
                    }
                }
                "tr" if !empty => {
                    if let Some(t) = tables.last_mut() {
                        t.row.clear();
                    }
                }
                "tc" if !empty => {
                    if let Some(t) = tables.last_mut() {
                        t.cell = Some(Cell::default());
                    }
                }
                _ => {}
            }
            continue;
        }
        match ev {
            Event::Text(t) => {
                let s = t.xml10_content();
                if in_instr {
                    if let Some(i) = &mut field_instr {
                        i.push_str(&s);
                    }
                } else if in_text {
                    push_text(&mut paras, &s, &style, &links);
                }
            }
            Event::GeneralRef(g) => {
                let s = entity(&g);
                if in_instr {
                    if let Some(i) = &mut field_instr {
                        i.push_str(&s);
                    }
                } else if in_text {
                    push_text(&mut paras, &s, &style, &links);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "instrText" => in_instr = false,
                "rPr" => in_rpr = false,
                "r" => in_run = in_run.saturating_sub(1),
                "hyperlink" | "fldSimple" => {
                    links.pop();
                }
                "p" => {
                    let Some(mut p) = paras.pop() else { continue };
                    let text = p.buf.take();
                    if let Some(cell) = tables.last_mut().and_then(|t| t.cell.as_mut()) {
                        cell.paras.push(text);
                        continue;
                    }
                    if !paras.is_empty() {
                        // A text box inside a paragraph: keep its text there.
                        if let Some(outer) = paras.last_mut() {
                            for s in text {
                                outer.buf.push(&s.text, &s.style);
                            }
                            outer.buf.push(" ", &Style::default());
                        }
                        continue;
                    }
                    let style_id = p.style.clone().unwrap_or_default();
                    let is_code = !style_id.is_empty()
                        && styles.named(&style_id, |n| {
                            n == "code"
                                || n.contains("source code")
                                || n.contains("preformatted")
                                || n.starts_with("code")
                        });
                    if is_code {
                        code_run.push(doc::plain(&text));
                        continue;
                    }
                    flush_code(&mut doc, &mut code_run);
                    if text.is_empty() {
                        continue;
                    }
                    let num = p
                        .num_id
                        .take()
                        .or_else(|| styles.style_num(&style_id))
                        .filter(|n| n != "0");
                    let block = if let Some(level) = styles.heading(&style_id) {
                        Block::Heading { level, text }
                    } else if let Some(n) = num {
                        Block::Item {
                            ordered: styles.numbered(&n, p.level),
                            depth: p.level.min(8),
                            text,
                        }
                    } else if styles.named(&style_id, |n| n.starts_with("list bullet")) {
                        Block::Item {
                            ordered: false,
                            depth: 0,
                            text,
                        }
                    } else if styles.named(&style_id, |n| n.starts_with("list number")) {
                        Block::Item {
                            ordered: true,
                            depth: 0,
                            text,
                        }
                    } else if styles.named(&style_id, |n| n.contains("quote")) {
                        Block::Quote(text)
                    } else {
                        Block::Paragraph(text)
                    };
                    doc.blocks.push(block);
                }
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        if let Some(cell) = t.cell.take() {
                            t.row.push(join_paras(cell.paras));
                        }
                    }
                }
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        let row = std::mem::take(&mut t.row);
                        if !row.is_empty() {
                            t.rows.push(row);
                        }
                    }
                }
                "tbl" => {
                    let Some(t) = tables.pop() else { continue };
                    if t.rows.is_empty() {
                        continue;
                    }
                    if let Some(cell) = tables.last_mut().and_then(|o| o.cell.as_mut()) {
                        // A table inside a cell: keep its text as lines.
                        for row in &t.rows {
                            let line: Vec<String> = row.iter().map(|c| doc::plain(c)).collect();
                            cell.paras.push(doc::text(&line.join(" ")));
                        }
                    } else {
                        flush_code(&mut doc, &mut code_run);
                        let mut rows = t.rows;
                        if t.header {
                            // Header cells are bold by nature.
                            for cell in rows[0].iter_mut() {
                                for s in cell.iter_mut() {
                                    s.style.bold = false;
                                }
                            }
                        }
                        doc.blocks.push(Block::Table {
                            rows,
                            header: t.header,
                        });
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    flush_code(&mut doc, &mut code_run);
    doc
}

fn push_text(paras: &mut [Para], text: &str, style: &Style, links: &[Option<String>]) {
    if let Some(p) = paras.last_mut() {
        let mut s = style.clone();
        s.link = links.iter().rev().flatten().next().cloned();
        p.buf.push(text, &s);
    }
}

fn join_paras(paras: Vec<Inline>) -> Inline {
    let mut out: Inline = Vec::new();
    for (i, p) in paras.into_iter().filter(|p| !p.is_empty()).enumerate() {
        if i > 0 {
            out.push(doc::Span {
                text: "\n".into(),
                style: Style::default(),
            });
        }
        out.extend(p);
    }
    out
}

/// `HYPERLINK "https://…"` → the address.
fn hyperlink_target(instr: &str) -> Option<String> {
    let rest = instr.trim().strip_prefix("HYPERLINK")?.trim();
    if let Some(q) = rest.strip_prefix('"') {
        q.split('"').next().map(str::to_string)
    } else {
        rest.split_whitespace().next().map(str::to_string)
    }
    .filter(|s| !s.is_empty() && !s.starts_with('\\'))
}

// ---------------------------------------------------------------------
// Writing

/// Escapes text for XML, dropping characters XML can't hold.
pub(crate) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || matches!(c as u32, 0xFFFE | 0xFFFF) => {}
            c => out.push(c),
        }
    }
    out
}

struct Writer {
    links: Vec<String>,
}

impl Writer {
    fn link_id(&mut self, url: &str) -> String {
        let i = match self.links.iter().position(|l| l == url) {
            Some(i) => i,
            None => {
                self.links.push(url.to_string());
                self.links.len() - 1
            }
        };
        format!("rIdL{}", i + 1)
    }

    fn runs(&mut self, inline: &Inline, force_bold: bool) -> String {
        let mut out = String::new();
        for s in inline {
            let mut rpr = String::new();
            if s.style.code {
                rpr.push_str("<w:rStyle w:val=\"CodeChar\"/>");
            }
            let link = s.style.link.as_deref().and_then(doc::safe_link);
            if link.is_some() && !s.style.code {
                rpr.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
            }
            if s.style.bold || force_bold {
                rpr.push_str("<w:b/>");
            }
            if s.style.italic {
                rpr.push_str("<w:i/>");
            }
            let rpr = if rpr.is_empty() {
                String::new()
            } else {
                format!("<w:rPr>{rpr}</w:rPr>")
            };
            let mut run = String::new();
            for (i, line) in s.text.split('\n').enumerate() {
                if i > 0 {
                    run.push_str("<w:br/>");
                }
                for (j, piece) in line.split('\t').enumerate() {
                    if j > 0 {
                        run.push_str("<w:tab/>");
                    }
                    if !piece.is_empty() {
                        run.push_str(&format!(
                            "<w:t xml:space=\"preserve\">{}</w:t>",
                            xml_escape(piece)
                        ));
                    }
                }
            }
            let r = format!("<w:r>{rpr}{run}</w:r>");
            match link {
                Some(url) => {
                    let id = self.link_id(url);
                    out.push_str(&format!("<w:hyperlink r:id=\"{id}\">{r}</w:hyperlink>"));
                }
                None => out.push_str(&r),
            }
        }
        out
    }
}

const TEXT_WIDTH: u32 = 9026;

fn styles_xml() -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/></w:style>"#,
    );
    let sizes = [36, 30, 26, 24, 22, 22];
    for (i, size) in sizes.iter().enumerate() {
        let n = i + 1;
        s.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="Heading{n}"><w:name w:val="heading {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="{}" w:after="120"/><w:outlineLvl w:val="{i}"/></w:pPr><w:rPr><w:b/><w:sz w:val="{size}"/><w:szCs w:val="{size}"/></w:rPr></w:style>"#,
            if n <= 2 { 360 } else { 240 }
        ));
    }
    s.push_str(
        r#"<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="60"/><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:ind w:left="720" w:right="720"/></w:pPr><w:rPr><w:i/><w:color w:val="595959"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="160" w:line="240" w:lineRule="auto"/><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:pPr><w:rPr><w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:cs="Courier New"/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style><w:style w:type="character" w:styleId="CodeChar"><w:name w:val="Code Char"/><w:basedOn w:val="DefaultParagraphFont"/><w:rPr><w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:cs="Courier New"/><w:sz w:val="20"/></w:rPr></w:style><w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:basedOn w:val="DefaultParagraphFont"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:semiHidden/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style></w:styles>"#,
    );
    s
}

fn numbering_xml(ordered_lists: u32) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
    );
    let bullets = ["•", "◦", "▪"];
    s.push_str(
        r#"<w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/>"#,
    );
    for i in 0..9u32 {
        s.push_str(&format!(
            r#"<w:lvl w:ilvl="{i}"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="{}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="360"/></w:pPr></w:lvl>"#,
            bullets[i as usize % 3],
            720 + 360 * i
        ));
    }
    s.push_str("</w:abstractNum>");
    let formats = ["decimal", "lowerLetter", "lowerRoman"];
    s.push_str(
        r#"<w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="hybridMultilevel"/>"#,
    );
    for i in 0..9u32 {
        s.push_str(&format!(
            r#"<w:lvl w:ilvl="{i}"><w:start w:val="1"/><w:numFmt w:val="{}"/><w:lvlText w:val="%{}."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="360"/></w:pPr></w:lvl>"#,
            formats[i as usize % 3],
            i + 1,
            720 + 360 * i
        ));
    }
    s.push_str("</w:abstractNum>");
    s.push_str(r#"<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#);
    for n in 0..ordered_lists {
        s.push_str(&format!(
            r#"<w:num w:numId="{}"><w:abstractNumId w:val="1"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num>"#,
            n + 2
        ));
    }
    s.push_str("</w:numbering>");
    s
}

/// Writes a .docx file.
pub fn write(d: &Doc, title: &str) -> Result<Vec<u8>> {
    let mut w = Writer { links: Vec::new() };
    let mut body = String::new();
    let mut ordered_lists = 0u32;
    let mut current_ordered: Option<u32> = None;
    let mut prev_item = false;
    for b in &d.blocks {
        let is_item = matches!(b, Block::Item { .. });
        if !is_item {
            current_ordered = None;
        }
        match b {
            Block::Heading { level, text } => body.push_str(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"Heading{}\"/></w:pPr>{}</w:p>",
                (*level).clamp(1, 6),
                w.runs(text, false)
            )),
            Block::Paragraph(text) => body.push_str(&format!("<w:p>{}</w:p>", w.runs(text, false))),
            Block::Item {
                ordered,
                depth,
                text,
            } => {
                let num = if *ordered {
                    let starts = !prev_item || (*depth == 0 && current_ordered.is_none());
                    if starts || current_ordered.is_none() {
                        ordered_lists += 1;
                        current_ordered = Some(ordered_lists + 1);
                    }
                    current_ordered.unwrap_or(2)
                } else {
                    if *depth == 0 {
                        current_ordered = None;
                    }
                    1
                };
                body.push_str(&format!(
                    "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{num}\"/></w:numPr></w:pPr>{}</w:p>",
                    (*depth).min(8),
                    w.runs(text, false)
                ));
            }
            Block::Quote(text) => body.push_str(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"Quote\"/></w:pPr>{}</w:p>",
                w.runs(text, false)
            )),
            Block::Code(code) => body.push_str(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"Code\"/></w:pPr>{}</w:p>",
                w.runs(&doc::text(code), false)
            )),
            Block::Rule => body.push_str(
                "<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr></w:pPr></w:p>",
            ),
            Block::Table { rows, header } => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let width = TEXT_WIDTH / cols as u32;
                body.push_str("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>");
                for _ in 0..cols {
                    body.push_str(&format!("<w:gridCol w:w=\"{width}\"/>"));
                }
                body.push_str("</w:tblGrid>");
                for (r, row) in rows.iter().enumerate() {
                    let head = *header && r == 0;
                    body.push_str("<w:tr>");
                    if head {
                        body.push_str("<w:trPr><w:tblHeader/></w:trPr>");
                    }
                    for c in 0..cols {
                        let empty = Vec::new();
                        let cell = row.get(c).unwrap_or(&empty);
                        body.push_str(&format!(
                            "<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/></w:tcPr><w:p>{}</w:p></w:tc>",
                            w.runs(cell, head)
                        ));
                    }
                    body.push_str("</w:tr>");
                }
                body.push_str("</w:tbl>");
            }
        }
        prev_item = is_item;
    }
    if !body.ends_with("</w:p>") {
        body.push_str("<w:p/>");
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#,
    );
    for (i, url) in w.links.iter().enumerate() {
        rels.push_str(&format!(
            r#"<Relationship Id="rIdL{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="{}" TargetMode="External"/>"#,
            i + 1,
            xml_escape(url)
        ));
    }
    rels.push_str("</Relationships>");
    let title = d.title.as_deref().unwrap_or(title);
    let core = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title></cp:coreProperties>"#,
        xml_escape(title)
    );
    let parts: [(&str, String); 7] = [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#.to_string()),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", rels),
        ("word/styles.xml", styles_xml()),
        ("word/numbering.xml", numbering_xml(ordered_lists)),
        ("docProps/core.xml", core),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, body) in parts {
        zip.start_file(name, opts)
            .map_err(|e| Error::Internal(format!("couldn't write the Word file ({e})")))?;
        zip.write_all(body.as_bytes())?;
    }
    let cursor = zip
        .finish()
        .map_err(|e| Error::Internal(format!("couldn't write the Word file ({e})")))?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::doc::{plain, Span};

    fn sample() -> Doc {
        let s = |t: &str| Span {
            text: t.into(),
            style: Style::default(),
        };
        Doc {
            title: Some("Plan & notes".into()),
            blocks: vec![
                Block::Heading {
                    level: 2,
                    text: vec![s("Week <1>")],
                },
                Block::Paragraph(vec![
                    s("Read "),
                    Span {
                        text: "this".into(),
                        style: Style {
                            bold: true,
                            italic: true,
                            link: Some("https://example.com/?a=1&b=2".into()),
                            ..Default::default()
                        },
                    },
                    s(" then\nrest\tlater"),
                ]),
                Block::Item {
                    ordered: true,
                    depth: 0,
                    text: vec![s("first")],
                },
                Block::Item {
                    ordered: false,
                    depth: 1,
                    text: vec![s("inner")],
                },
                Block::Paragraph(vec![s("break")]),
                Block::Item {
                    ordered: true,
                    depth: 0,
                    text: vec![s("again")],
                },
                Block::Quote(vec![s("wise words")]),
                Block::Code("if a {\n    b();\n}".into()),
                Block::Table {
                    rows: vec![
                        vec![vec![s("A")], vec![s("B")]],
                        vec![vec![s("1")], vec![s("2")]],
                    ],
                    header: true,
                },
            ],
        }
    }

    #[test]
    fn round_trips_through_word() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("plan.docx");
        std::fs::write(&p, write(&sample(), "plan").unwrap()).unwrap();
        // Detected as Word, not as a plain ZIP.
        assert_eq!(
            crate::format::detect(&p).unwrap(),
            crate::format::Format::Docx
        );
        let back = read(&p).unwrap();
        assert_eq!(back.title.as_deref(), Some("Plan & notes"));
        assert_eq!(back.blocks.len(), sample().blocks.len(), "{back:#?}");
        assert_eq!(back.blocks[0], sample().blocks[0]);
        assert_eq!(back.blocks[1], sample().blocks[1]);
        assert!(matches!(
            &back.blocks[2],
            Block::Item {
                ordered: true,
                depth: 0,
                ..
            }
        ));
        assert!(matches!(
            &back.blocks[3],
            Block::Item {
                ordered: false,
                depth: 1,
                ..
            }
        ));
        assert_eq!(back.blocks[6], sample().blocks[6]);
        assert_eq!(back.blocks[7], sample().blocks[7]);
        assert_eq!(back.blocks[8], sample().blocks[8]);
    }

    #[test]
    fn ordered_lists_restart_after_a_break() {
        let bytes = write(&sample(), "plan").unwrap();
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let numbering = part(&mut zip, "word/numbering.xml").unwrap().unwrap();
        assert!(numbering.contains(r#"w:numId="2""#) && numbering.contains(r#"w:numId="3""#));
        let document = part(&mut zip, "word/document.xml").unwrap().unwrap();
        assert!(document.contains("Week &lt;1&gt;"));
        // Well-formed all the way through.
        let mut r = quick_xml::Reader::from_str(&document);
        loop {
            match r.read_event() {
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("{e}"),
            }
        }
    }

    /// The parts Word itself writes that matter: styles by id with names,
    /// numbering, fields, text boxes and deleted text.
    #[test]
    fn reads_words_own_markup() {
        let styles = Styles {
            styles: parse_styles(
                r#"<w:styles xmlns:w="w"><w:style w:type="paragraph" w:styleId="berschrift1"><w:name w:val="heading 1"/></w:style><w:style w:type="paragraph" w:styleId="MyQuote"><w:name w:val="Intense Quote"/></w:style></w:styles>"#,
            ),
            numbering: HashMap::new(),
        };
        let xml = r#"<w:document xmlns:w="w" xmlns:r="r" xmlns:mc="mc"><w:body>
<w:p><w:pPr><w:pStyle w:val="berschrift1"/></w:pPr><w:r><w:t>Titel</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Tom &amp; </w:t></w:r><w:del><w:r><w:delText>gone</w:delText></w:r></w:del><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>HYPERLINK "https://a.b"</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>site</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="MyQuote"/><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t>Q</w:t></w:r></w:p>
<w:p><w:r><mc:AlternateContent><mc:Choice><w:t>once</w:t></mc:Choice><mc:Fallback><w:t>twice</w:t></mc:Fallback></mc:AlternateContent></w:r></w:p>
</w:body></w:document>"#;
        let d = parse_document(xml, &styles, &HashMap::new());
        assert!(
            matches!(&d.blocks[0], Block::Heading { level: 1, text } if plain(text) == "Titel"),
            "{d:#?}"
        );
        let Block::Paragraph(p) = &d.blocks[1] else {
            panic!("{d:#?}")
        };
        assert_eq!(plain(p), "Tom & site");
        assert_eq!(p.last().unwrap().style.link.as_deref(), Some("https://a.b"));
        assert_eq!(d.blocks[2], Block::Quote(doc::text("Q")));
        assert_eq!(d.blocks[3], Block::Paragraph(doc::text("once")));
    }
}
