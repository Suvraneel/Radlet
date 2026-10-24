//! Documents as PDF pages. On a Mac, the Vision helper lays the document
//! out with the system's text engine, which has fonts for every script.
//! Everywhere else (and if the helper fails) a small built-in writer does
//! it with the standard PDF fonts: Helvetica and Courier, which cover
//! Western European text; other letters print as "?".

use std::sync::atomic::AtomicBool;

use lopdf::{dictionary, Document, Object, Stream, StringFormat};

use crate::catalog::Capabilities;
use crate::engines::doc::{self, Block, Doc, Inline, Span, Style};
use crate::engines::pdfedit::HELVETICA;
use crate::engines::vision;
use crate::error::{Error, Result};

const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const MARGIN_X: f32 = 64.0;
const MARGIN_TOP: f32 = 64.0;
const MARGIN_BOTTOM: f32 = 64.0;
const WIDTH: f32 = PAGE_W - 2.0 * MARGIN_X;
const TOP: f32 = PAGE_H - MARGIN_TOP;

const BODY: f32 = 10.5;
const BODY_LEADING: f32 = 15.0;
const LINK_RGB: &str = "0.02 0.36 0.72 rg";

/// A4 PDF bytes for a document: the Mac's text engine when the helper is
/// there, the built-in writer otherwise.
pub fn render(d: &Doc, title: &str, caps: &Capabilities, cancel: &AtomicBool) -> Result<Vec<u8>> {
    if caps.vision.is_some() {
        match render_with_helper(d, title, caps, cancel) {
            Ok(bytes) => return Ok(bytes),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => {}
        }
    }
    write(d, title)
}

fn render_with_helper(
    d: &Doc,
    title: &str,
    caps: &Capabilities,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    let scratch = tempfile::tempdir()?;
    let html = scratch.path().join("document.html");
    std::fs::write(&html, doc::to_html(d, title, true))?;
    let out = scratch.path().join("document.pdf");
    std::fs::File::create(&out)?;
    vision::document_pdf(caps, &html, &out, cancel)?;
    let bytes = std::fs::read(&out)?;
    // Only a PDF with pages counts.
    let pages = Document::load_mem(&bytes)
        .map(|d| d.get_pages().len())
        .unwrap_or(0);
    if pages == 0 {
        return Err(Error::Helper("the helper's PDF has no pages".into()));
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Font {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
    MonoBold,
}

impl Font {
    const ALL: [Font; 6] = [
        Font::Regular,
        Font::Bold,
        Font::Italic,
        Font::BoldItalic,
        Font::Mono,
        Font::MonoBold,
    ];

    fn name(self) -> &'static str {
        match self {
            Font::Regular => "F1",
            Font::Bold => "F2",
            Font::Italic => "F3",
            Font::BoldItalic => "F4",
            Font::Mono => "F5",
            Font::MonoBold => "F6",
        }
    }

    fn base(self) -> &'static str {
        match self {
            Font::Regular => "Helvetica",
            Font::Bold => "Helvetica-Bold",
            Font::Italic => "Helvetica-Oblique",
            Font::BoldItalic => "Helvetica-BoldOblique",
            Font::Mono => "Courier",
            Font::MonoBold => "Courier-Bold",
        }
    }

    fn of(style: &Style, bold: bool, italic: bool, mono: bool) -> Font {
        let bold = bold || style.bold;
        let italic = italic || style.italic;
        if mono || style.code {
            return if bold { Font::MonoBold } else { Font::Mono };
        }
        match (bold, italic) {
            (false, false) => Font::Regular,
            (true, false) => Font::Bold,
            (false, true) => Font::Italic,
            (true, true) => Font::BoldItalic,
        }
    }

    fn bold(self) -> bool {
        matches!(self, Font::Bold | Font::BoldItalic | Font::MonoBold)
    }

    /// Width of encoded text at 1 pt.
    fn width(self, bytes: &[u8]) -> f32 {
        if matches!(self, Font::Mono | Font::MonoBold) {
            return bytes.len() as f32 * 0.6;
        }
        let bold = self.bold();
        bytes
            .iter()
            .map(|&b| match b {
                32..=126 if bold => HELVETICA_BOLD[(b - 32) as usize],
                32..=126 => HELVETICA[(b - 32) as usize],
                _ => high_width(b, bold),
            } as f32)
            .sum::<f32>()
            / 1000.0
    }
}

/// Helvetica-Bold advance widths for codes 32 to 126.
const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, // 32-47
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, // 48-63
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, // 64-79
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556, // 80-95
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, // 96-111
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, // 112-126
];

/// Widths of the Windows-1252 letters above 127: accented letters are as
/// wide as their base letter; the rest are close enough for line breaks.
fn high_width(b: u8, bold: bool) -> u16 {
    let base = match b {
        0xC0..=0xC5 => Some(b'A'),
        0xC7 => Some(b'C'),
        0xC8..=0xCB => Some(b'E'),
        0xCC..=0xCF => Some(b'I'),
        0xD0 => Some(b'D'),
        0xD1 => Some(b'N'),
        0xD2..=0xD6 | 0xD8 => Some(b'O'),
        0xD9..=0xDC => Some(b'U'),
        0xDD | 0x9F => Some(b'Y'),
        0xE0..=0xE5 => Some(b'a'),
        0xE7 => Some(b'c'),
        0xE8..=0xEB => Some(b'e'),
        0xEC..=0xEF => Some(b'i'),
        0xF1 => Some(b'n'),
        0xF2..=0xF6 | 0xF8 => Some(b'o'),
        0xF9..=0xFC => Some(b'u'),
        0xFD | 0xFF => Some(b'y'),
        0x8A => Some(b'S'),
        0x9A => Some(b's'),
        0x8E => Some(b'Z'),
        0x9E => Some(b'z'),
        _ => None,
    };
    if let Some(c) = base {
        let table = if bold { &HELVETICA_BOLD } else { &HELVETICA };
        return table[(c - 32) as usize];
    }
    match b {
        0xC6 | 0x85 | 0x89 | 0x97 | 0x99 | 0x8C => 1000,
        0xE6 | 0x9C => 944,
        0xDF => 611,
        0xDE | 0xFE => 667,
        0xF0 => 611,
        0xD7 | 0xF7 | 0xAC | 0xB1 => 584,
        0x95 => 350,
        0x91 | 0x92 | 0x82 => {
            if bold {
                278
            } else {
                222
            }
        }
        0x93 | 0x94 | 0x84 => {
            if bold {
                500
            } else {
                333
            }
        }
        0xA9 | 0xAE => 737,
        0xB0 => 400,
        0xA0 | 0xB7 => 278,
        0xBC..=0xBE => 834,
        0x8B | 0x9B | 0x88 | 0x98 | 0xA1 | 0xA8 | 0xAF | 0xB4 | 0xB8 => 333,
        0xBF => 611,
        _ => 556,
    }
}

/// Text in Windows-1252 for the standard fonts. A few common symbols are
/// spelled out; anything else outside it prints as "?".
fn encode(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        let code = c as u32;
        if (0x20..=0x7E).contains(&code) || (0xA0..=0xFF).contains(&code) {
            out.push(code as u8);
            continue;
        }
        let b = match c {
            '€' => 0x80,
            '‚' => 0x82,
            'ƒ' => 0x83,
            '„' => 0x84,
            '…' => 0x85,
            '†' => 0x86,
            '‡' => 0x87,
            'ˆ' => 0x88,
            '‰' => 0x89,
            'Š' => 0x8A,
            '‹' => 0x8B,
            'Œ' => 0x8C,
            'Ž' => 0x8E,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' | '◦' | '▪' | '●' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '˜' => 0x98,
            '™' => 0x99,
            'š' => 0x9A,
            '›' => 0x9B,
            'œ' => 0x9C,
            'ž' => 0x9E,
            'Ÿ' => 0x9F,
            '\t' => {
                out.extend_from_slice(b"    ");
                continue;
            }
            _ => {
                let spelled: &[u8] = match c {
                    '→' => b"->",
                    '←' => b"<-",
                    '⇒' => b"=>",
                    '≤' => b"<=",
                    '≥' => b">=",
                    '≠' => b"!=",
                    '−' | '‐' | '‑' => b"-",
                    '✓' | '✔' => b"v",
                    '✗' | '✘' => b"x",
                    '\u{2009}' | '\u{200A}' | '\u{2002}' | '\u{2003}' | '\u{202F}' => b" ",
                    '\u{200B}' | '\u{FEFF}' | '\u{AD}' => b"",
                    _ => b"?",
                };
                out.extend_from_slice(spelled);
                continue;
            }
        };
        out.push(b);
    }
    out
}

/// A PDF literal string.
fn pdf_string(bytes: &[u8], out: &mut Vec<u8>) {
    out.push(b'(');
    for &b in bytes {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
}

#[derive(Debug, Clone)]
struct Piece {
    bytes: Vec<u8>,
    font: Font,
    link: Option<String>,
    width: f32,
}

#[derive(Debug)]
enum Tok {
    Word(Vec<Piece>),
    /// A space, in the font of the text it sits in.
    Space(Font),
    Break,
}

/// Splits spans into words (which may mix styles, as in "**bold**,"),
/// spaces and line breaks.
fn tokenize(inline: &[Span], size: f32, bold: bool, italic: bool, mono: bool) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut word: Vec<Piece> = Vec::new();
    let finish_word = |word: &mut Vec<Piece>, toks: &mut Vec<Tok>| {
        if !word.is_empty() {
            toks.push(Tok::Word(std::mem::take(word)));
        }
    };
    for s in inline {
        let font = Font::of(&s.style, bold, italic, mono);
        let link = s
            .style
            .link
            .as_deref()
            .and_then(doc::safe_link)
            .map(str::to_string);
        let mut piece = String::new();
        let flush_piece = |piece: &mut String, word: &mut Vec<Piece>| {
            if !piece.is_empty() {
                let bytes = encode(piece);
                let width = font.width(&bytes) * size;
                word.push(Piece {
                    bytes,
                    font,
                    link: link.clone(),
                    width,
                });
                piece.clear();
            }
        };
        for c in s.text.chars() {
            if c == '\n' {
                flush_piece(&mut piece, &mut word);
                finish_word(&mut word, &mut toks);
                toks.push(Tok::Break);
            } else if mono && c == '\t' {
                piece.push_str("    ");
            } else if c.is_whitespace() && c != '\u{a0}' && !(mono && c == ' ') {
                flush_piece(&mut piece, &mut word);
                finish_word(&mut word, &mut toks);
                if !matches!(toks.last(), Some(Tok::Space(_)) | Some(Tok::Break) | None) {
                    toks.push(Tok::Space(font));
                }
            } else {
                piece.push(c);
            }
        }
        flush_piece(&mut piece, &mut word);
    }
    finish_word(&mut word, &mut toks);
    toks
}

#[derive(Debug, Default)]
struct Line {
    items: Vec<(f32, Piece)>,
    width: f32,
}

/// Fills lines no wider than `max`, breaking at spaces, and inside words
/// only when a word alone is too wide.
fn layout(toks: Vec<Tok>, size: f32, max: f32) -> Vec<Line> {
    let mut lines = vec![Line::default()];
    let mut pending_space = 0.0f32;
    for t in toks {
        match t {
            Tok::Break => {
                lines.push(Line::default());
                pending_space = 0.0;
            }
            Tok::Space(font) => {
                if lines.last().is_some_and(|l| !l.items.is_empty()) {
                    pending_space = font.width(b" ") * size;
                }
            }
            Tok::Word(pieces) => {
                let w: f32 = pieces.iter().map(|p| p.width).sum();
                let line = lines.last_mut().expect("at least one line");
                let start = if line.items.is_empty() {
                    0.0
                } else {
                    line.width + pending_space
                };
                if start + w > max && !line.items.is_empty() {
                    lines.push(Line::default());
                }
                let line = lines.last_mut().expect("at least one line");
                if line.items.is_empty() && w > max {
                    // Too wide on its own: break it by characters.
                    for p in pieces {
                        let mut chunk: Vec<u8> = Vec::new();
                        for b in p.bytes {
                            let next = p.font.width(&[b]) * size;
                            let line = lines.last_mut().expect("line");
                            let used = line.width + p.font.width(&chunk) * size;
                            if used + next > max && (!chunk.is_empty() || !line.items.is_empty()) {
                                let width = p.font.width(&chunk) * size;
                                if !chunk.is_empty() {
                                    let x = line.width;
                                    line.items.push((
                                        x,
                                        Piece {
                                            bytes: std::mem::take(&mut chunk),
                                            font: p.font,
                                            link: p.link.clone(),
                                            width,
                                        },
                                    ));
                                }
                                lines.push(Line::default());
                            }
                            chunk.push(b);
                        }
                        if !chunk.is_empty() {
                            let width = p.font.width(&chunk) * size;
                            let line = lines.last_mut().expect("line");
                            let x = line.width;
                            line.items.push((
                                x,
                                Piece {
                                    bytes: chunk,
                                    font: p.font,
                                    link: p.link,
                                    width,
                                },
                            ));
                            line.width = x + width;
                        }
                    }
                } else {
                    let mut x = if line.items.is_empty() {
                        0.0
                    } else {
                        line.width + pending_space
                    };
                    for p in pieces {
                        let w = p.width;
                        line.items.push((x, p));
                        x += w;
                    }
                    line.width = x;
                }
                pending_space = 0.0;
            }
        }
    }
    // Recompute widths after character breaks.
    for l in &mut lines {
        l.width = l.items.last().map_or(0.0, |(x, p)| x + p.width);
    }
    lines
}

#[derive(Default)]
struct Page {
    ops: Vec<u8>,
    links: Vec<([f32; 4], String)>,
}

struct Pager {
    pages: Vec<Page>,
    y: f32,
}

impl Pager {
    fn new() -> Pager {
        Pager {
            pages: vec![Page::default()],
            y: TOP,
        }
    }

    fn at_top(&self) -> bool {
        self.y >= TOP - 0.01
    }

    fn new_page(&mut self) {
        self.pages.push(Page::default());
        self.y = TOP;
    }

    /// Starts a new page unless `h` points fit on this one (or the page
    /// is still empty).
    fn need(&mut self, h: f32) {
        if self.y - h < MARGIN_BOTTOM && !self.at_top() {
            self.new_page();
        }
    }

    /// Vertical space, dropped at the top of a page.
    /// Vertical space, dropped at the top of a page. Running past the
    /// bottom starts no page; whatever comes next does, if anything.
    fn gap(&mut self, h: f32) {
        if !self.at_top() {
            self.y -= h;
        }
    }

    fn page(&mut self) -> &mut Page {
        self.pages.last_mut().expect("at least one page")
    }

    fn op(&mut self, s: &str) {
        let ops = &mut self.page().ops;
        ops.extend_from_slice(s.as_bytes());
        ops.push(b'\n');
    }

    fn fill(&mut self, gray: f32, x: f32, y: f32, w: f32, h: f32) {
        self.op(&format!("{gray:.3} g {x:.2} {y:.2} {w:.2} {h:.2} re f 0 g"));
    }

    /// Draws one line of text with its baseline at `y`. Words in the same
    /// font go out as one string with real spaces, so copied text reads
    /// right.
    fn text_line(&mut self, line: &Line, x0: f32, y: f32, size: f32, gray: f32) {
        if line.items.is_empty() {
            return;
        }
        // Runs: (x, end, font, link, bytes).
        type Run = (f32, f32, Font, Option<String>, Vec<u8>);
        let mut runs: Vec<Run> = Vec::new();
        for (x, p) in &line.items {
            match runs.last_mut() {
                Some(r) if r.2 == p.font && r.3 == p.link => {
                    if *x > r.1 + 0.01 {
                        r.4.push(b' ');
                    }
                    r.4.extend_from_slice(&p.bytes);
                    r.1 = x + p.width;
                }
                _ => runs.push((*x, x + p.width, p.font, p.link.clone(), p.bytes.clone())),
            }
        }
        let mut ops: Vec<u8> = b"BT\n".to_vec();
        let mut links = Vec::new();
        for (x, end, font, link, bytes) in &runs {
            let color = if link.is_some() {
                LINK_RGB.to_string()
            } else {
                format!("{gray:.3} g")
            };
            ops.extend_from_slice(
                format!(
                    "{color} /{} {size:.2} Tf 1 0 0 1 {:.2} {y:.2} Tm ",
                    font.name(),
                    x0 + x
                )
                .as_bytes(),
            );
            pdf_string(bytes, &mut ops);
            ops.extend_from_slice(b" Tj\n");
            if let Some(url) = link {
                links.push((
                    [x0 + x, y - size * 0.25, x0 + end, y + size * 0.85],
                    url.clone(),
                ));
            }
        }
        ops.extend_from_slice(b"ET 0 g\n");
        let page = self.page();
        page.ops.extend_from_slice(&ops);
        page.links.extend(links);
    }
}

struct Para {
    size: f32,
    leading: f32,
    bold: bool,
    italic: bool,
    mono: bool,
    gray: f32,
}

impl Para {
    fn body() -> Para {
        Para {
            size: BODY,
            leading: BODY_LEADING,
            bold: false,
            italic: false,
            mono: false,
            gray: 0.0,
        }
    }

    fn lines(&self, inline: &[Span], width: f32) -> Vec<Line> {
        layout(
            tokenize(inline, self.size, self.bold, self.italic, self.mono),
            self.size,
            width,
        )
    }
}

/// Places lines from the current position, breaking pages between lines.
/// `marker` is drawn left of the first line (a bullet or number).
fn place(
    pg: &mut Pager,
    lines: &[Line],
    x: f32,
    p: &Para,
    marker: Option<(f32, &Line)>,
    bar: bool,
) {
    for (i, line) in lines.iter().enumerate() {
        pg.need(p.leading);
        let baseline = pg.y - p.leading * 0.75;
        if bar {
            pg.fill(0.8, x - 12.0, pg.y - p.leading, 2.0, p.leading);
        }
        pg.text_line(line, x, baseline, p.size, p.gray);
        if i == 0 {
            if let Some((mx, m)) = marker {
                pg.text_line(m, mx, baseline, p.size, p.gray);
            }
        }
        pg.y -= p.leading;
    }
}

fn marker_line(text: &str, font: Font, size: f32) -> Line {
    let bytes = encode(text);
    let width = font.width(&bytes) * size;
    Line {
        items: vec![(
            0.0,
            Piece {
                bytes,
                font,
                link: None,
                width,
            },
        )],
        width,
    }
}

fn table(pg: &mut Pager, rows: &[Vec<Inline>], header: bool) {
    let size = 9.5;
    let leading = 13.0;
    let pad = 4.0;
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    // Natural widths: each column's longest cell on one line.
    let mut natural = vec![36.0f32; cols];
    for (r, row) in rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let bold = header && r == 0;
            let w: f32 = layout(tokenize(cell, size, bold, false, false), size, f32::MAX)
                .iter()
                .map(|l| l.width)
                .fold(0.0, f32::max);
            natural[c] = natural[c].max((w + 2.0 * pad).min(240.0));
        }
    }
    let total: f32 = natural.iter().sum();
    let widths: Vec<f32> = if total <= WIDTH {
        natural
    } else {
        let scaled: Vec<f32> = natural
            .iter()
            .map(|w| (w * WIDTH / total).max(36.0))
            .collect();
        let t: f32 = scaled.iter().sum();
        scaled.iter().map(|w| w * WIDTH / t).collect()
    };
    let lay_out = |row: &[Inline], bold: bool| -> Vec<Vec<Line>> {
        (0..cols)
            .map(|c| {
                let empty = Vec::new();
                let cell = row.get(c).unwrap_or(&empty);
                layout(
                    tokenize(cell, size, bold, false, false),
                    size,
                    widths[c] - 2.0 * pad,
                )
            })
            .collect()
    };
    let lines_in = |cells: &[Vec<Line>]| cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
    // Draws lines `from..to` of every cell as one band of the row.
    let band = |pg: &mut Pager, cells: &[Vec<Line>], from: usize, to: usize, bold: bool| {
        let h = (to - from) as f32 * leading + 2.0 * pad;
        let top = pg.y;
        let mut x = MARGIN_X;
        for (c, lines) in cells.iter().enumerate() {
            if bold {
                pg.fill(0.93, x, top - h, widths[c], h);
            }
            for (i, line) in lines.iter().enumerate().take(to).skip(from) {
                let baseline = top - pad - leading * ((i - from) as f32) - leading * 0.75;
                pg.text_line(line, x + pad, baseline, size, 0.0);
            }
            pg.op(&format!(
                "0.6 G 0.5 w {x:.2} {:.2} {:.2} {h:.2} re S 0 G",
                top - h,
                widths[c]
            ));
            x += widths[c];
        }
        pg.y = top - h;
    };
    let page_h = TOP - MARGIN_BOTTOM;
    let head_cells = header.then(|| lay_out(&rows[0], true));
    // The header is repeated on each new page, unless it's huge itself.
    let repeat = head_cells
        .as_ref()
        .filter(|h| lines_in(h) as f32 * leading + 2.0 * pad < page_h / 3.0);
    let new_page = |pg: &mut Pager| {
        pg.new_page();
        if let Some(h) = repeat {
            band(pg, h, 0, lines_in(h), true);
        }
    };
    for (r, row) in rows.iter().enumerate() {
        let head = header && r == 0;
        let owned;
        let cells: &Vec<Vec<Line>> = match (&head_cells, head) {
            (Some(h), true) => h,
            _ => {
                owned = lay_out(row, false);
                &owned
            }
        };
        let n = lines_in(cells);
        let h = n as f32 * leading + 2.0 * pad;
        // A row that fits on a page is kept in one piece.
        if h <= page_h && pg.y - h < MARGIN_BOTTOM && !pg.at_top() {
            if head {
                pg.new_page();
            } else {
                new_page(pg);
            }
        }
        // A taller one continues on the next page, line by line.
        let mut from = 0;
        while from < n {
            let room = pg.y - MARGIN_BOTTOM - 2.0 * pad;
            let mut fit = (room / leading).floor().max(0.0) as usize;
            if fit == 0 {
                if pg.at_top() {
                    fit = 1;
                } else {
                    new_page(pg);
                    continue;
                }
            }
            let to = (from + fit).min(n);
            band(pg, cells, from, to, head);
            from = to;
        }
    }
}

/// The built-in writer: A4 pages, Helvetica text.
pub fn write(d: &Doc, title: &str) -> Result<Vec<u8>> {
    let mut pg = Pager::new();
    let blocks = &d.blocks;
    let mut prev_depth: Option<u8> = None;
    let mut counters = [0u32; 10];
    for (i, b) in blocks.iter().enumerate() {
        let next_is_item = matches!(blocks.get(i + 1), Some(Block::Item { .. }));
        match b {
            Block::Heading { level, text } => {
                let l = (*level).clamp(1, 6) as usize - 1;
                let size = [20.0, 16.0, 13.5, 12.0, 11.0, 10.5][l];
                let p = Para {
                    size,
                    leading: size * 1.3,
                    bold: true,
                    ..Para::body()
                };
                pg.gap([14.0, 12.0, 10.0, 8.0, 6.0, 6.0][l]);
                let lines = p.lines(text, WIDTH);
                // Keep a heading with the start of what follows it.
                pg.need(lines.len() as f32 * p.leading + 2.0 * BODY_LEADING);
                place(&mut pg, &lines, MARGIN_X, &p, None, false);
                pg.gap(6.0);
            }
            Block::Paragraph(text) => {
                let p = Para::body();
                let lines = p.lines(text, WIDTH);
                place(&mut pg, &lines, MARGIN_X, &p, None, false);
                pg.gap(8.0);
            }
            Block::Item {
                ordered,
                depth,
                text,
            } => {
                let depth = match prev_depth {
                    None => 0,
                    Some(p) => (*depth).min(p + 1),
                }
                .min(8);
                for c in counters.iter_mut().skip(depth as usize + 1) {
                    *c = 0;
                }
                counters[depth as usize] += 1;
                prev_depth = Some(depth);
                let p = Para::body();
                let indent = 18.0 * (depth as f32 + 1.0);
                let lines = p.lines(text, WIDTH - indent);
                let (mark, mx) = if *ordered {
                    let m = marker_line(
                        &format!("{}.", counters[depth as usize]),
                        Font::Regular,
                        BODY,
                    );
                    let x = MARGIN_X + indent - 5.0 - m.width;
                    (m, x)
                } else {
                    let m = marker_line("•", Font::Regular, BODY);
                    (m, MARGIN_X + indent - 11.0)
                };
                place(
                    &mut pg,
                    &lines,
                    MARGIN_X + indent,
                    &p,
                    Some((mx, &mark)),
                    false,
                );
                pg.gap(if next_is_item { 2.0 } else { 8.0 });
            }
            Block::Quote(text) => {
                let p = Para {
                    italic: true,
                    gray: 0.3,
                    ..Para::body()
                };
                let lines = p.lines(text, WIDTH - 18.0);
                place(&mut pg, &lines, MARGIN_X + 18.0, &p, None, true);
                pg.gap(8.0);
            }
            Block::Code(code) => {
                let p = Para {
                    size: 9.0,
                    leading: 12.0,
                    mono: true,
                    ..Para::body()
                };
                let lines = p.lines(&doc::text(code), WIDTH - 12.0);
                let pad = 5.0;
                let n = lines.len();
                for (j, line) in lines.iter().enumerate() {
                    let top_pad = if j == 0 { pad } else { 0.0 };
                    let bottom_pad = if j + 1 == n { pad } else { 0.0 };
                    let h = p.leading + top_pad + bottom_pad;
                    pg.need(h);
                    pg.fill(0.95, MARGIN_X, pg.y - h, WIDTH, h);
                    let baseline = pg.y - top_pad - p.leading * 0.75;
                    pg.text_line(line, MARGIN_X + 6.0, baseline, p.size, 0.0);
                    pg.y -= h;
                }
                pg.gap(8.0);
            }
            Block::Rule => {
                pg.need(16.0);
                pg.gap(6.0);
                let y = pg.y;
                pg.op(&format!(
                    "0.75 G 0.75 w {MARGIN_X:.2} {y:.2} m {:.2} {y:.2} l S 0 G",
                    MARGIN_X + WIDTH
                ));
                pg.gap(10.0);
            }
            Block::Table { rows, header } => {
                table(&mut pg, rows, *header);
                pg.gap(10.0);
            }
        }
        if !matches!(b, Block::Item { .. }) {
            prev_depth = None;
            counters = [0; 10];
        }
    }
    finish(pg, d.title.as_deref().unwrap_or(title))
}

/// A PDF text string: plain when ASCII, otherwise UTF-16 with a BOM.
fn text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

/// Link addresses must be ASCII; anything else is percent-encoded.
fn ascii_uri(url: &str) -> String {
    let mut out = String::new();
    for b in url.bytes() {
        if b.is_ascii_graphic() {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn finish(pg: Pager, title: &str) -> Result<Vec<u8>> {
    let mut d = Document::with_version("1.7");
    let pages_id = d.new_object_id();
    let mut fonts = lopdf::Dictionary::new();
    for f in Font::ALL {
        let id = d.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => f.base(),
            "Encoding" => "WinAnsiEncoding",
        });
        fonts.set(f.name(), id);
    }
    let resources = d.add_object(dictionary! { "Font" => fonts });
    let mut kids = Vec::new();
    for page in pg.pages {
        let content = d.add_object(Stream::new(lopdf::Dictionary::new(), page.ops));
        let mut dict = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(PAGE_W), Object::Real(PAGE_H)],
            "Contents" => content,
            "Resources" => resources,
        };
        if !page.links.is_empty() {
            let annots: Vec<Object> = page
                .links
                .into_iter()
                .map(|(r, url)| {
                    Object::Dictionary(dictionary! {
                        "Type" => "Annot",
                        "Subtype" => "Link",
                        "Rect" => r.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
                        "Border" => vec![0.into(), 0.into(), 0.into()],
                        "A" => dictionary! {
                            "S" => "URI",
                            "URI" => Object::string_literal(ascii_uri(&url)),
                        },
                    })
                })
                .collect();
            dict.set("Annots", annots);
        }
        kids.push(Object::Reference(d.add_object(dict)));
    }
    let count = kids.len() as i64;
    d.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = d.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    let info = d.add_object(dictionary! {
        "Title" => text_string(title),
        "Producer" => Object::string_literal("Radlet"),
    });
    d.trailer.set("Root", catalog);
    d.trailer.set("Info", info);
    d.compress();
    let mut out = Vec::new();
    d.save_to(&mut out)
        .map_err(|e| Error::Pdf(format!("couldn't write the PDF ({e})")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: &str) -> Span {
        Span {
            text: t.into(),
            style: Style::default(),
        }
    }

    #[test]
    fn lines_wrap_at_spaces() {
        let toks = tokenize(&[s("aaa bbb ccc")], 10.0, false, false, false);
        let width_aaa = Font::Regular.width(b"aaa") * 10.0;
        let space = Font::Regular.width(b" ") * 10.0;
        let lines = layout(toks, 10.0, width_aaa * 2.0 + space + 0.1);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].items.len(), 2);
        // A word too wide for a line is broken by characters.
        let long = layout(
            tokenize(&[s("abcdefghij")], 10.0, false, false, false),
            10.0,
            20.0,
        );
        assert!(long.len() > 2);
        assert!(long.iter().all(|l| l.width <= 20.01), "{long:?}");
        // Styles inside one word stay together.
        let mixed = vec![
            Span {
                text: "bold".into(),
                style: Style {
                    bold: true,
                    ..Default::default()
                },
            },
            s(", then"),
        ];
        let toks = tokenize(&mixed, 10.0, false, false, false);
        assert!(matches!(&toks[0], Tok::Word(p) if p.len() == 2));
    }

    #[test]
    fn writes_a_readable_pdf() {
        let mut blocks = vec![
            Block::Heading {
                level: 1,
                text: vec![s("Café notes (draft)")],
            },
            Block::Paragraph(vec![
                s("See "),
                Span {
                    text: "the site".into(),
                    style: Style {
                        link: Some("https://example.com/é".into()),
                        ..Default::default()
                    },
                },
                s(" → now 😀"),
            ]),
            Block::Item {
                ordered: true,
                depth: 0,
                text: vec![s("first")],
            },
            Block::Code("fn main() {}".into()),
            Block::Table {
                rows: vec![
                    vec![doc::text("A"), doc::text("B")],
                    vec![doc::text("1"), doc::text("2")],
                ],
                header: true,
            },
        ];
        for i in 0..120 {
            blocks.push(Block::Paragraph(vec![s(&format!(
                "Paragraph {i} with some words to fill the line and wrap it onto another line"
            ))]));
        }
        let d = Doc {
            title: Some("Notes ü".into()),
            blocks,
        };
        let bytes = write(&d, "x").unwrap();
        let pdf = Document::load_mem(&bytes).unwrap();
        assert!(pdf.get_pages().len() >= 3, "pages break");
        let text = pdf.extract_text(&[1]).unwrap();
        assert!(text.contains("Café notes"), "{text}");
        assert!(text.contains("-> now ?"), "{text}");
        let info = pdf.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let title = pdf.get_dictionary(info).unwrap().get(b"Title").unwrap();
        assert!(matches!(title, Object::String(b, _) if b.starts_with(&[0xFE, 0xFF])));
        // The link is clickable, with an ASCII address.
        let page1 = pdf.get_pages()[&1];
        let annots = pdf.get_dictionary(page1).unwrap().get(b"Annots").unwrap();
        let first = &annots.as_array().unwrap()[0];
        let uri = first
            .as_dict()
            .unwrap()
            .get(b"A")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"URI")
            .unwrap();
        assert_eq!(uri.as_str().unwrap(), b"https://example.com/%C3%A9");
    }

    #[test]
    fn tall_table_rows_continue_on_the_next_page() {
        let long = "word ".repeat(4000);
        let d = Doc {
            title: None,
            blocks: vec![
                Block::Table {
                    rows: vec![
                        vec![doc::text("Head")],
                        vec![doc::text(&long)],
                        vec![doc::text("after")],
                    ],
                    header: true,
                },
                Block::Paragraph(doc::text("end")),
            ],
        };
        let bytes = write(&d, "t").unwrap();
        let pdf = Document::load_mem(&bytes).unwrap();
        let pages = pdf.get_pages();
        assert!(pages.len() >= 3, "{}", pages.len());
        let mut words = 0;
        for (n, id) in &pages {
            let text = pdf.extract_text(&[*n]).unwrap();
            words += text.matches("word").count();
            // Every page has text, and nothing is drawn below the margin.
            assert!(!text.trim().is_empty(), "page {n} is blank");
            let content = pdf.get_page_content(*id);
            let ops = lopdf::content::Content::decode(&content).unwrap();
            for op in ops.operations.iter().filter(|o| o.operator == "Tm") {
                let y = op.operands[5].as_float().unwrap();
                assert!(y >= MARGIN_BOTTOM - 1.0, "text at y={y} on page {n}");
            }
            if *n > 1 {
                assert!(text.contains("Head"), "header repeated on page {n}");
            }
        }
        assert_eq!(words, 4000);
        let last = pdf.extract_text(&[pages.len() as u32]).unwrap();
        assert!(last.contains("after") && last.contains("end"));
    }

    #[test]
    fn empty_document_is_one_blank_page() {
        let bytes = write(&Doc::default(), "Empty").unwrap();
        assert_eq!(Document::load_mem(&bytes).unwrap().get_pages().len(), 1);
    }
}
