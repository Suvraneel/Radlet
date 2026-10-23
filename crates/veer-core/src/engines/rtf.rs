//! Rich Text Format: reading text, emphasis, links, lists, headings and
//! tables into the document model, and writing RTF that TextEdit, Pages
//! and Word open.

use std::collections::HashMap;

use crate::engines::doc::{self, Block, Doc, Inline, InlineBuf, Style};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dest {
    Text,
    FontTable,
    StyleSheet,
    Info,
    Title,
    FieldInstr,
    ListText,
    Skip,
}

#[derive(Debug, Clone)]
struct Group {
    dest: Dest,
    bold: bool,
    italic: bool,
    font: i32,
    uc: u32,
    link: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct Para {
    list: bool,
    level: Option<u8>,
    indent: i32,
    outline: Option<u8>,
    style: Option<i32>,
    in_table: bool,
}

#[derive(Debug, Default)]
struct Font {
    name: String,
    modern: bool,
}

#[derive(Debug, Default)]
struct StyleDef {
    name: String,
    outline: Option<u8>,
}

/// Destinations whose contents are never text.
const SKIPPED: &[&str] = &[
    "colortbl",
    "pict",
    "object",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "footnote",
    "annotation",
    "xmlnstbl",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "revtbl",
    "filetbl",
    "pgdsctbl",
    "bkmkstart",
    "bkmkend",
    "atnid",
    "atnauthor",
    "pnseclvl",
    "pntxta",
    "pntxtb",
    "shp",
    "nonshppict",
    "fldtype",
    "author",
    "operator",
    "company",
    "comment",
    "doccomm",
    "keywords",
    "subject",
    "category",
    "hlinkbase",
    "mmathPr",
    "wgrffmtfilter",
    "xe",
    "tc",
    "txe",
    "do",
];

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

/// Mac Roman, for old files written with `\mac`.
fn mac_roman(b: u8) -> char {
    const HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø¿¡¬√ƒ≈∆«»… ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{F8FF}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";
    if b < 0x80 {
        b as char
    } else {
        HIGH.chars().nth((b - 0x80) as usize).unwrap_or('?')
    }
}

struct Reader {
    doc: Doc,
    groups: Vec<Group>,
    cur: Group,
    para: Para,
    buf: InlineBuf,
    fonts: HashMap<i32, Font>,
    font_entry: Option<i32>,
    styles: HashMap<i32, StyleDef>,
    style_entry: Option<i32>,
    field_instr: String,
    marker: String,
    title: String,
    skip_chars: u32,
    mac: bool,
    // Tables.
    cell: Inline,
    row: Vec<Inline>,
    rows: Vec<Vec<Inline>>,
    code: Vec<String>,
    /// The first half of a character outside the BMP, sent as two \u.
    pending_high: Option<u32>,
}

impl Reader {
    fn style(&self) -> Style {
        let code = self
            .fonts
            .get(&self.cur.font)
            .is_some_and(|f| f.modern || is_mono(&f.name));
        Style {
            bold: self.cur.bold,
            italic: self.cur.italic,
            code,
            link: self.cur.link.clone(),
        }
    }

    fn put(&mut self, c: char) {
        if self.skip_chars > 0 {
            self.skip_chars -= 1;
            return;
        }
        match self.cur.dest {
            Dest::Text => {
                let style = self.style();
                let mut b = [0u8; 4];
                self.buf.push(c.encode_utf8(&mut b), &style);
            }
            Dest::FontTable => {
                if let Some(n) = self.font_entry {
                    let f = self.fonts.entry(n).or_default();
                    if c == ';' {
                        self.font_entry = None;
                    } else {
                        f.name.push(c);
                    }
                }
            }
            Dest::StyleSheet => {
                if let Some(n) = self.style_entry {
                    let s = self.styles.entry(n).or_default();
                    if c == ';' {
                        self.style_entry = None;
                    } else {
                        s.name.push(c);
                    }
                }
            }
            Dest::Title => self.title.push(c),
            Dest::FieldInstr => self.field_instr.push(c),
            Dest::ListText => self.marker.push(c),
            Dest::Info | Dest::Skip => {}
        }
    }

    fn put_str(&mut self, s: &str) {
        for c in s.chars() {
            self.put(c);
        }
    }

    fn heading_level(&self) -> Option<u8> {
        if let Some(o) = self.para.outline.filter(|o| *o < 6) {
            return Some(o + 1);
        }
        let def = self.styles.get(&self.para.style?)?;
        if let Some(o) = def.outline.filter(|o| *o < 6) {
            return Some(o + 1);
        }
        let name = def.name.trim().to_lowercase();
        if name == "title" {
            return Some(1);
        }
        name.strip_prefix("heading ")?
            .trim()
            .parse()
            .ok()
            .filter(|l| (1..=6).contains(l))
    }

    fn flush_table(&mut self) {
        if !self.row.is_empty() {
            let row = std::mem::take(&mut self.row);
            self.rows.push(row);
        }
        if !self.rows.is_empty() {
            let rows = std::mem::take(&mut self.rows);
            // Bold header cells mark a header row.
            let mut rows = rows;
            let header = rows.len() > 1
                && rows[0]
                    .iter()
                    .all(|c| !c.is_empty() && c.iter().all(|s| s.style.bold));
            if header {
                for cell in rows[0].iter_mut() {
                    for s in cell.iter_mut() {
                        s.style.bold = false;
                    }
                }
            }
            self.doc.blocks.push(Block::Table { rows, header });
        }
    }

    fn flush_code(&mut self) {
        if !self.code.is_empty() {
            let code = std::mem::take(&mut self.code).join("\n");
            self.doc.blocks.push(Block::Code(code));
        }
    }

    /// Ends a paragraph (`\par`).
    fn end_para(&mut self) {
        let mut text = self.buf.take();
        let marker = std::mem::take(&mut self.marker);
        if self.para.in_table {
            self.flush_code();
            if !text.is_empty() {
                if !self.cell.is_empty() {
                    self.cell.push(doc::Span {
                        text: "\n".into(),
                        style: Style::default(),
                    });
                }
                self.cell.extend(text);
            }
            return;
        }
        self.flush_table();
        if text.is_empty() {
            return;
        }
        let all_code = text
            .iter()
            .all(|s| s.style.code || s.text.trim().is_empty());
        if all_code && self.heading_level().is_none() && !self.para.list {
            self.code.push(doc::plain(&text));
            return;
        }
        self.flush_code();
        // A bullet or number written as text, as this module writes lists.
        let mut written_marker: Option<bool> = None;
        if let Some(first) = text.first_mut() {
            for b in ["•\t", "◦\t", "▪\t", "-\t", "–\t"] {
                if let Some(rest) = first.text.strip_prefix(b) {
                    first.text = rest.to_string();
                    written_marker = Some(false);
                    break;
                }
            }
            if written_marker.is_none() {
                let digits = first.text.chars().take_while(char::is_ascii_digit).count();
                if digits > 0 && first.text[digits..].starts_with(".\t") {
                    first.text = first.text[digits + 2..].to_string();
                    written_marker = Some(true);
                }
            }
        }
        let text = doc::trim(text);
        let depth = self
            .para
            .level
            .unwrap_or_else(|| (self.para.indent.saturating_sub(360).max(0) / 360).min(8) as u8);
        let block = if let Some(level) = self.heading_level() {
            // Headings are bold by nature; the bold isn't emphasis.
            let mut merged = InlineBuf::default();
            for mut s in text {
                s.style.bold = false;
                merged.push(&s.text, &s.style);
            }
            Block::Heading {
                level,
                text: merged.take(),
            }
        } else if self.para.list || written_marker.is_some() {
            let m = marker.trim();
            let ordered = written_marker.unwrap_or_else(|| {
                m.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
                    && m.trim_end_matches(['.', ')']).len() < m.len()
            });
            Block::Item {
                ordered,
                depth,
                text,
            }
        } else {
            Block::Paragraph(text)
        };
        self.doc.blocks.push(block);
    }

    fn end_cell(&mut self) {
        self.flush_code();
        let leftover = self.buf.take();
        if !leftover.is_empty() {
            if !self.cell.is_empty() {
                self.cell.push(doc::Span {
                    text: "\n".into(),
                    style: Style::default(),
                });
            }
            self.cell.extend(leftover);
        }
        let cell = std::mem::take(&mut self.cell);
        self.row.push(doc::trim(cell));
    }

    fn control(&mut self, word: &str, param: Option<i32>, ignorable: bool) {
        let dest = self.cur.dest;
        if dest == Dest::Skip {
            return;
        }
        // Destinations.
        match word {
            "fonttbl" => {
                self.cur.dest = Dest::FontTable;
                return;
            }
            "stylesheet" => {
                self.cur.dest = Dest::StyleSheet;
                return;
            }
            "info" => {
                self.cur.dest = Dest::Info;
                return;
            }
            "title" if dest == Dest::Info => {
                self.cur.dest = Dest::Title;
                return;
            }
            "fldinst" => {
                self.field_instr.clear();
                self.cur.dest = Dest::FieldInstr;
                return;
            }
            "fldrslt" => {
                if let Some(url) = hyperlink_target(&self.field_instr) {
                    self.cur.link = Some(url);
                }
                self.cur.dest = Dest::Text;
                return;
            }
            "listtext" | "pntext" => {
                self.marker.clear();
                self.cur.dest = Dest::ListText;
                return;
            }
            w if SKIPPED.contains(&w) => {
                self.cur.dest = Dest::Skip;
                return;
            }
            _ if ignorable && !matches!(word, "fldinst") => {
                self.cur.dest = Dest::Skip;
                return;
            }
            _ => {}
        }
        if dest == Dest::Info {
            return;
        }
        match dest {
            Dest::FontTable => {
                match word {
                    "f" => {
                        let n = param.unwrap_or(0);
                        self.font_entry = Some(n);
                        self.fonts.entry(n).or_default();
                    }
                    "fmodern" => {
                        if let Some(n) = self.font_entry {
                            self.fonts.entry(n).or_default().modern = true;
                        }
                    }
                    _ => {}
                }
                return;
            }
            Dest::StyleSheet => {
                match word {
                    "s" => {
                        let n = param.unwrap_or(0);
                        self.style_entry = Some(n);
                        self.styles.entry(n).or_default();
                    }
                    "outlinelevel" => {
                        if let Some(n) = self.style_entry {
                            self.styles.entry(n).or_default().outline =
                                param.and_then(|p| u8::try_from(p).ok());
                        }
                    }
                    "cs" | "ds" | "ts" => self.style_entry = None,
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        let on = param != Some(0);
        match word {
            "par" | "sect" => self.end_para(),
            "pard" => {
                self.para = Para::default();
            }
            "plain" => {
                self.cur.bold = false;
                self.cur.italic = false;
                self.cur.font = 0;
            }
            "b" => self.cur.bold = on,
            "i" => self.cur.italic = on,
            "f" => self.cur.font = param.unwrap_or(0),
            "uc" => self.cur.uc = param.unwrap_or(1).max(0) as u32,
            "u" => {
                let mut n = param.unwrap_or(0);
                if n < 0 {
                    n += 65536;
                }
                let n = n as u32;
                // Surrogate halves arrive as two \u codes.
                if (0xD800..0xDC00).contains(&n) {
                    self.pending_high = Some(n);
                } else if (0xDC00..0xE000).contains(&n) {
                    if let Some(h) = self.pending_high.take() {
                        let c = 0x10000 + ((h - 0xD800) << 10) + (n - 0xDC00);
                        if let Some(c) = char::from_u32(c) {
                            self.put(c);
                        }
                    }
                } else if let Some(c) = char::from_u32(n) {
                    self.put(c);
                }
                self.skip_chars = self.cur.uc;
            }
            "line" => self.put('\n'),
            "tab" => self.put('\t'),
            "emdash" => self.put('—'),
            "endash" => self.put('–'),
            "bullet" => self.put('•'),
            "lquote" => self.put('‘'),
            "rquote" => self.put('’'),
            "ldblquote" => self.put('“'),
            "rdblquote" => self.put('”'),
            "emspace" | "enspace" | "qmspace" => self.put(' '),
            "mac" => self.mac = true,
            "ls" => self.para.list = true,
            "ilvl" => self.para.level = param.and_then(|p| u8::try_from(p).ok()).map(|l| l.min(8)),
            "li" => self.para.indent = param.unwrap_or(0),
            "outlinelevel" => self.para.outline = param.and_then(|p| u8::try_from(p).ok()),
            "s" => self.para.style = param,
            "intbl" => self.para.in_table = true,
            "cell" | "nestcell" => self.end_cell(),
            "row" | "nestrow" => {
                if !self.cell.is_empty() || !self.buf.is_blank() {
                    self.end_cell();
                }
                let row = std::mem::take(&mut self.row);
                if !row.is_empty() {
                    self.rows.push(row);
                }
            }
            _ => {}
        }
    }
}

fn is_mono(name: &str) -> bool {
    let n = name.to_lowercase();
    ["courier", "menlo", "monaco", "consolas", "mono"]
        .iter()
        .any(|m| n.contains(m))
}

fn hyperlink_target(instr: &str) -> Option<String> {
    let rest = instr.trim().strip_prefix("HYPERLINK")?.trim();
    let url = if let Some(q) = rest.strip_prefix('"') {
        q.split('"').next()?
    } else {
        rest.split_whitespace().next()?
    };
    (!url.is_empty() && !url.starts_with('\\')).then(|| url.to_string())
}

/// Reads an RTF document.
pub fn read(bytes: &[u8]) -> Result<Doc> {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(0);
    if !bytes[start..].starts_with(b"{\\rtf") {
        return Err(Error::UnsupportedInput("this isn't an RTF document".into()));
    }
    let root = Group {
        dest: Dest::Text,
        bold: false,
        italic: false,
        font: 0,
        uc: 1,
        link: None,
    };
    let mut r = Reader {
        doc: Doc::default(),
        groups: Vec::new(),
        cur: root,
        para: Para::default(),
        buf: InlineBuf::default(),
        fonts: HashMap::new(),
        font_entry: None,
        styles: HashMap::new(),
        style_entry: None,
        field_instr: String::new(),
        marker: String::new(),
        title: String::new(),
        skip_chars: 0,
        mac: false,
        cell: Vec::new(),
        row: Vec::new(),
        rows: Vec::new(),
        code: Vec::new(),
        pending_high: None,
    };
    let mut i = start;
    let mut ignorable = false;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'{' => {
                if r.groups.len() > 512 {
                    return Err(Error::Invalid("this RTF file nests too deeply".into()));
                }
                r.groups.push(r.cur.clone());
                r.skip_chars = 0;
                i += 1;
            }
            b'}' => {
                if let Some(g) = r.groups.pop() {
                    if r.cur.dest == Dest::FontTable {
                        r.font_entry = None;
                    }
                    r.cur = g;
                }
                r.skip_chars = 0;
                ignorable = false;
                i += 1;
            }
            b'\\' => {
                i += 1;
                let Some(&c) = bytes.get(i) else { break };
                if c.is_ascii_alphabetic() {
                    let s = i;
                    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = std::str::from_utf8(&bytes[s..i]).unwrap_or("");
                    let ps = i;
                    if i < bytes.len() && bytes[i] == b'-' {
                        i += 1;
                    }
                    while i < bytes.len() && bytes[i].is_ascii_digit() && i - ps < 11 {
                        i += 1;
                    }
                    let param = if i > ps {
                        std::str::from_utf8(&bytes[ps..i])
                            .ok()
                            .and_then(|p| p.parse::<i64>().ok())
                            .map(|p| p.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
                    } else {
                        None
                    };
                    if i < bytes.len() && bytes[i] == b' ' {
                        i += 1;
                    }
                    // A Unicode fallback being skipped can be a control word.
                    if r.skip_chars > 0 && word != "u" {
                        r.skip_chars -= 1;
                        continue;
                    }
                    let word = word.to_string();
                    r.control(&word, param, ignorable);
                    ignorable = false;
                } else {
                    i += 1;
                    match c {
                        b'\\' | b'{' | b'}' => r.put(c as char),
                        b'\'' => {
                            let hex = bytes
                                .get(i..i + 2)
                                .and_then(|h| std::str::from_utf8(h).ok());
                            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                                let ch = if r.mac { mac_roman(v) } else { cp1252(v) };
                                r.put(ch);
                                i += 2;
                            }
                        }
                        b'~' => r.put(' '),
                        b'_' => r.put('-'),
                        b'-' => {}
                        b'*' => ignorable = true,
                        b'\n' | b'\r' => r.control("par", None, false),
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                // Plain text, possibly UTF-8 from a lax writer.
                let end = bytes[i..]
                    .iter()
                    .position(|&x| matches!(x, b'{' | b'}' | b'\\' | b'\r' | b'\n'))
                    .map(|p| i + p)
                    .unwrap_or(bytes.len());
                let chunk = &bytes[i..end];
                match std::str::from_utf8(chunk) {
                    Ok(s) => r.put_str(s),
                    Err(_) => {
                        for &x in chunk {
                            let ch = if r.mac { mac_roman(x) } else { cp1252(x) };
                            r.put(ch);
                        }
                    }
                }
                i = end;
            }
        }
    }
    if !r.buf.is_blank() {
        r.end_para();
    }
    r.flush_table();
    r.flush_code();
    let title = r.title.trim().to_string();
    r.doc.title = (!title.is_empty()).then_some(title);
    Ok(r.doc)
}

// ---------------------------------------------------------------------
// Writing

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\line "),
            '\t' => out.push_str("\\tab "),
            '\r' => {}
            c if (c as u32) < 0x20 => {}
            c if (c as u32) < 0x80 => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    out
}

fn runs(inline: &Inline, force_bold: bool) -> String {
    let mut out = String::new();
    for s in inline {
        let mut fmt = String::new();
        if s.style.bold || force_bold {
            fmt.push_str("\\b");
        }
        if s.style.italic {
            fmt.push_str("\\i");
        }
        if s.style.code {
            fmt.push_str("\\f1\\fs20");
        }
        let body = escape(&s.text);
        let run = if fmt.is_empty() {
            body
        } else {
            format!("{{{fmt} {body}}}")
        };
        match s.style.link.as_deref().and_then(doc::safe_link) {
            Some(url) => out.push_str(&format!(
                "{{\\field{{\\*\\fldinst{{HYPERLINK \"{}\"}}}}{{\\fldrslt{{\\ul\\cf1 {run}}}}}}}",
                escape(url).replace('"', "%22")
            )),
            None => out.push_str(&run),
        }
    }
    out
}

/// Writes an RTF document.
pub fn write(d: &Doc) -> String {
    let mut out = String::from(
        "{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1\n{\\fonttbl{\\f0\\fswiss\\fcharset0 Helvetica;}{\\f1\\fmodern\\fcharset0 Courier New;}}\n{\\colortbl;\\red5\\green99\\blue193;}\n",
    );
    if let Some(t) = &d.title {
        out.push_str(&format!("{{\\info{{\\title {}}}}}\n", escape(t)));
    }
    out.push_str(
        "\\paperw11906\\paperh16838\\margl1440\\margr1440\\margt1440\\margb1440\n\\f0\\fs22\n",
    );
    let depths = {
        // Same rule as the other writers: one level deeper at most.
        let mut prev: Option<u8> = None;
        d.blocks
            .iter()
            .map(|b| match b {
                Block::Item { depth, .. } => {
                    let v = prev.map_or(0, |p| (*depth).min(p + 1)).min(8);
                    prev = Some(v);
                    v
                }
                _ => {
                    prev = None;
                    0
                }
            })
            .collect::<Vec<u8>>()
    };
    let mut counters = [0u32; 10];
    for (i, b) in d.blocks.iter().enumerate() {
        match b {
            Block::Heading { level, text } => {
                let l = (*level).clamp(1, 6);
                let size = [36, 30, 26, 24, 22, 22][(l - 1) as usize];
                out.push_str(&format!(
                    "\\pard\\outlinelevel{}\\sb240\\sa120\\keepn\\b\\fs{size} {}\\b0\\fs22\\par\n",
                    l - 1,
                    runs(text, false)
                ));
            }
            Block::Paragraph(text) => {
                out.push_str(&format!("\\pard\\sa160 {}\\par\n", runs(text, false)))
            }
            Block::Item { ordered, text, .. } => {
                let depth = depths[i] as usize;
                for c in counters.iter_mut().skip(depth + 1) {
                    *c = 0;
                }
                counters[depth] += 1;
                let marker = if *ordered {
                    format!("{}.", counters[depth])
                } else {
                    ["\\u8226?", "\\u9702?", "\\u9642?"][depth % 3].to_string()
                };
                out.push_str(&format!(
                    "\\pard\\ilvl{depth}\\li{}\\fi-360\\sa60 {marker}\\tab {}\\par\n",
                    720 + 360 * depth,
                    runs(text, false)
                ));
            }
            Block::Quote(text) => out.push_str(&format!(
                "\\pard\\li720\\ri720\\sa160\\i {}\\i0\\par\n",
                runs(text, false)
            )),
            Block::Code(code) => out.push_str(&format!(
                "\\pard\\sa160\\f1\\fs20 {}\\f0\\fs22\\par\n",
                escape(code)
            )),
            Block::Rule => out.push_str("\\pard\\brdrb\\brdrs\\brdrw10\\brsp20\\sa160 \\par\n"),
            Block::Table { rows, header } => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let width = 9026 / cols as i32;
                for (r, row) in rows.iter().enumerate() {
                    out.push_str("\\trowd\\trgaph108\\trleft0");
                    for c in 0..cols {
                        out.push_str(&format!(
                            "\\clbrdrt\\brdrs\\brdrw10\\clbrdrl\\brdrs\\brdrw10\\clbrdrb\\brdrs\\brdrw10\\clbrdrr\\brdrs\\brdrw10\\cellx{}",
                            width * (c as i32 + 1)
                        ));
                    }
                    out.push('\n');
                    for c in 0..cols {
                        let empty = Vec::new();
                        let cell = row.get(c).unwrap_or(&empty);
                        out.push_str(&format!(
                            "\\pard\\intbl {}\\cell\n",
                            runs(cell, *header && r == 0)
                        ));
                    }
                    out.push_str("\\row\n");
                }
                out.push_str("\\pard\\sa160\\par\n");
            }
        }
        if !matches!(b, Block::Item { .. }) {
            counters = [0; 10];
        }
    }
    out.push('}');
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::doc::{plain, Span};

    #[test]
    fn reads_what_textedit_writes() {
        let rtf = br#"{\rtf1\ansi\ansicpg1252\cocoartf2761
\cocoatextscaling0\cocoaplatform0{\fonttbl\f0\fswiss\fcharset0 Helvetica;\f1\fmodern\fcharset0 Courier;}
{\colortbl;\red255\green255\blue255;}
{\*\expandedcolortbl;;}
{\*\listtable{\list\listtemplateid1{\listlevel\levelnfc23{\leveltext\'01\uc0\u8226 ;}}\listid1}}
\paperw11900\paperh16840\margl1440\margr1440\vieww11520\viewh8400\viewkind0
\pard\tx720\pardirnatural\partightenfactor0

\f0\b\fs24 \cf0 Bold
\b0  and \i italic\i0  caf\'e9 \u8364 \'80 \{x\}\
\pard\tx220\li720\fi-720\pardirnatural\partightenfactor0
\ls1\ilvl0\cf0 {\listtext	\uc0\u8226 	}item one\
{\listtext	\uc0\u8226 	}item two\
\pard\pardirnatural
\f1 let x = 1;\
{\field{\*\fldinst{HYPERLINK "https://example.com"}}{\fldrslt \f0 link}}\
}"#;
        let d = read(rtf).unwrap();
        let b = &d.blocks;
        let Block::Paragraph(p) = &b[0] else {
            panic!("{b:#?}")
        };
        // \'80 is the fallback for \u8364, so it's skipped.
        assert_eq!(plain(p), "Bold and italic café € {x}");
        assert!(p[0].style.bold && p[0].text == "Bold");
        assert!(p.iter().any(|s| s.style.italic && s.text == "italic"));
        assert!(
            matches!(&b[1], Block::Item { ordered: false, text, .. } if plain(text) == "item one"),
            "{b:#?}"
        );
        assert!(matches!(&b[2], Block::Item { text, .. } if plain(text) == "item two"));
        assert_eq!(b[3], Block::Code("let x = 1;".into()));
        let Block::Paragraph(link) = &b[4] else {
            panic!("{b:#?}")
        };
        assert_eq!(link[0].style.link.as_deref(), Some("https://example.com"));
        assert_eq!(b.len(), 5);
    }

    #[test]
    fn round_trips() {
        let s = |t: &str| Span {
            text: t.into(),
            style: Style::default(),
        };
        let d = Doc {
            title: Some("Über".into()),
            blocks: vec![
                Block::Heading {
                    level: 2,
                    text: vec![s("Plan {A}")],
                },
                Block::Paragraph(vec![
                    s("Emoji 😀 and "),
                    Span {
                        text: "bold\\path".into(),
                        style: Style {
                            bold: true,
                            ..Default::default()
                        },
                    },
                ]),
                Block::Item {
                    ordered: true,
                    depth: 0,
                    text: vec![s("one")],
                },
                Block::Item {
                    ordered: false,
                    depth: 1,
                    text: vec![s("sub")],
                },
                Block::Code("a\n  b".into()),
                Block::Table {
                    rows: vec![
                        vec![vec![s("H1")], vec![s("H2")]],
                        vec![vec![s("x")], vec![s("y")]],
                    ],
                    header: true,
                },
            ],
        };
        let rtf = write(&d);
        assert!(
            rtf.contains("\\u-10179?\\u-8704?"),
            "emoji as a surrogate pair"
        );
        let back = read(rtf.as_bytes()).unwrap();
        assert_eq!(back.title.as_deref(), Some("Über"));
        assert_eq!(back.blocks[0], d.blocks[0]);
        assert_eq!(back.blocks[1], d.blocks[1]);
        assert!(
            matches!(
                &back.blocks[2],
                Block::Item {
                    ordered: true,
                    depth: 0,
                    ..
                }
            ),
            "{back:#?}"
        );
        assert!(matches!(
            &back.blocks[3],
            Block::Item {
                ordered: false,
                depth: 1,
                ..
            }
        ));
        assert_eq!(back.blocks[4], d.blocks[4]);
        let Block::Table { rows, header } = &back.blocks[5] else {
            panic!("{back:#?}")
        };
        assert!(*header);
        assert_eq!(plain(&rows[1][1]), "y");
        assert_eq!(back.blocks.len(), 6);
    }

    #[test]
    fn deep_lists_and_odd_numbers_are_safe() {
        let mut rtf = String::from("{\\rtf1 ");
        for l in 0..14 {
            rtf.push_str(&format!("\\pard\\ls1\\ilvl{l} item {l}\\par "));
        }
        rtf.push_str("\\pard\\li-2147483648 \\u8226?\tx\\par }");
        let d = read(rtf.as_bytes()).unwrap();
        assert!(d.blocks.iter().all(|b| match b {
            Block::Item { depth, .. } => *depth <= 8,
            _ => true,
        }));
        // Every writer copes.
        crate::engines::doc::to_text(&d, true);
        crate::engines::doc::to_markdown(&d);
        write(&d);
        crate::engines::docpdf::write(&d, "deep").unwrap();
    }

    #[test]
    fn rejects_non_rtf() {
        assert!(read(b"hello").is_err());
        // Deep nesting is refused rather than exhausting memory.
        let deep = format!("{{\\rtf1{}", "{".repeat(10_000));
        assert!(read(deep.as_bytes()).is_err());
    }
}
