//! A small document model shared by the text formats: headings,
//! paragraphs, list items, quotes, code, rules and tables, with bold,
//! italic, code and links inside. TXT, Markdown, HTML, Word, RTF and
//! (for tables) CSV are each read into it and written out of it, so every
//! pair converts without a converter per pair.

use std::path::Path;

use crate::error::{Error, Result};
use crate::format::Format;

/// How a run of text looks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

/// A run of text in one style. `\n` inside is a line break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

pub type Inline = Vec<Span>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Level 1 (title) to 6.
    Heading {
        level: u8,
        text: Inline,
    },
    Paragraph(Inline),
    /// One list item. Items in a row form a list; `depth` 0 is the outer
    /// list. Numbers are worked out when writing.
    Item {
        ordered: bool,
        depth: u8,
        text: Inline,
    },
    Quote(Inline),
    Code(String),
    Rule,
    /// Rows of cells; with `header`, the first row is the header.
    Table {
        rows: Vec<Vec<Inline>>,
        header: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

/// Collects text into spans, merging runs that look the same.
#[derive(Debug, Default)]
pub struct InlineBuf {
    spans: Inline,
}

impl InlineBuf {
    pub fn push(&mut self, text: &str, style: &Style) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            if last.style == *style {
                last.text.push_str(text);
                return;
            }
        }
        self.spans.push(Span {
            text: text.to_string(),
            style: style.clone(),
        });
    }

    /// True when there's nothing but white space.
    pub fn is_blank(&self) -> bool {
        self.spans.iter().all(|s| s.text.trim().is_empty())
    }

    /// The last character collected, if any.
    pub fn last_char(&self) -> Option<char> {
        self.spans.last().and_then(|s| s.text.chars().last())
    }

    /// The spans collected so far, trimmed at both ends.
    pub fn take(&mut self) -> Inline {
        trim(std::mem::take(&mut self.spans))
    }
}

/// Removes white space at the start and end of an inline.
pub fn trim(mut spans: Inline) -> Inline {
    while let Some(first) = spans.first_mut() {
        let t = first.text.trim_start().to_string();
        if t.is_empty() {
            spans.remove(0);
        } else {
            first.text = t;
            break;
        }
    }
    while let Some(last) = spans.last_mut() {
        let t = last.text.trim_end().to_string();
        if t.is_empty() {
            spans.pop();
        } else {
            last.text = t;
            break;
        }
    }
    spans
}

pub fn plain(inline: &[Span]) -> String {
    inline.iter().map(|s| s.text.as_str()).collect()
}

pub fn text(t: &str) -> Inline {
    if t.is_empty() {
        Vec::new()
    } else {
        vec![Span {
            text: t.to_string(),
            style: Style::default(),
        }]
    }
}

impl Doc {
    /// Every word, block by block, with no decoration: for counting words.
    pub fn words_text(&self) -> String {
        let mut out = String::new();
        for b in &self.blocks {
            match b {
                Block::Heading { text, .. }
                | Block::Paragraph(text)
                | Block::Item { text, .. }
                | Block::Quote(text) => out.push_str(&plain(text)),
                Block::Code(c) => out.push_str(c),
                Block::Rule => {}
                Block::Table { rows, .. } => {
                    for row in rows {
                        for cell in row {
                            out.push_str(&plain(cell));
                            out.push('\t');
                        }
                        out.push('\n');
                    }
                }
            }
            out.push('\n');
        }
        out
    }
}

/// Words and reading time: "1,234 words · 6 min read". Scripts written
/// without spaces (Chinese, Japanese, Thai) count each character as a
/// word, as word processors do.
pub fn word_count_label(text: &str) -> String {
    let words = count_words(text);
    let minutes = words.div_ceil(238).max(1);
    let noun = if words == 1 { "word" } else { "words" };
    format!("{} {noun} · {minutes} min read", thousands(words))
}

pub fn count_words(text: &str) -> u64 {
    let mut words = 0u64;
    for token in text.split_whitespace() {
        let mut in_word = false;
        for c in token.chars() {
            if is_spaceless_script(c) {
                words += 1;
                in_word = false;
            } else if c.is_alphanumeric() {
                if !in_word {
                    words += 1;
                    in_word = true;
                }
            } else if !matches!(c, '\'' | '’' | '-' | '.' | ',' | '_') {
                in_word = false;
            }
        }
    }
    words
}

fn is_spaceless_script(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF // CJK
        | 0x0E00..=0x0E7F // Thai
        | 0x0E80..=0x0EFF // Lao
        | 0x1000..=0x109F // Myanmar
        | 0x1780..=0x17FF // Khmer
        | 0x20000..=0x2FA1F)
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------
// Plain text

/// Reads plain text: blank lines separate paragraphs; a paragraph whose
/// every line starts with "- ", "* ", "• " or "1. " is a list.
pub fn from_text(src: &str) -> Doc {
    let src = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut doc = Doc::default();
    for para in src.split("\n\n") {
        let lines: Vec<&str> = para.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            continue;
        }
        let items: Option<Vec<(bool, u8, &str)>> = lines.iter().map(|l| list_line(l)).collect();
        match items {
            Some(items) => {
                for (ordered, depth, rest) in items {
                    doc.blocks.push(Block::Item {
                        ordered,
                        depth,
                        text: text(rest),
                    });
                }
            }
            None => {
                let body = lines
                    .iter()
                    .map(|l| l.trim_end())
                    .collect::<Vec<_>>()
                    .join("\n");
                doc.blocks.push(Block::Paragraph(text(body.trim())));
            }
        }
    }
    doc
}

/// "  - item" → (unordered, depth 1, "item").
fn list_line(line: &str) -> Option<(bool, u8, &str)> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    let depth = (indent / 2).min(8) as u8;
    for marker in ["- ", "* ", "• ", "◦ ", "▪ "] {
        if let Some(rest) = t.strip_prefix(marker) {
            return Some((false, depth, rest.trim()));
        }
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, depth, r.trim()));
        }
    }
    None
}

/// Item depths can only go one deeper than the item before; the first
/// item of a list is at depth 0.
fn normalized_depths(blocks: &[Block]) -> Vec<u8> {
    let mut out = Vec::with_capacity(blocks.len());
    let mut prev: Option<u8> = None;
    for b in blocks {
        match b {
            Block::Item { depth, .. } => {
                let d = match prev {
                    None => 0,
                    Some(p) => (*depth).min(p + 1),
                }
                .min(8);
                out.push(d);
                prev = Some(d);
            }
            _ => {
                out.push(0);
                prev = None;
            }
        }
    }
    out
}

/// Numbers for ordered items, counted per depth.
fn item_numbers(blocks: &[Block], depths: &[u8]) -> Vec<u32> {
    let mut counters = [0u32; 10];
    let mut out = Vec::with_capacity(blocks.len());
    let mut prev_ordered = [false; 10];
    for (b, &d) in blocks.iter().zip(depths) {
        let d = d as usize;
        match b {
            Block::Item { ordered, .. } => {
                for c in counters.iter_mut().skip(d + 1) {
                    *c = 0;
                }
                if prev_ordered[d] != *ordered {
                    counters[d] = 0;
                }
                prev_ordered[d] = *ordered;
                counters[d] += 1;
                out.push(counters[d]);
            }
            _ => {
                counters = [0; 10];
                prev_ordered = [false; 10];
                out.push(0);
            }
        }
    }
    out
}

fn inline_text(inline: &[Span]) -> String {
    let mut out = String::new();
    for s in inline {
        out.push_str(&s.text);
        if let Some(url) = &s.style.link {
            let shown = s.text.trim();
            let bare = url.trim_start_matches("mailto:");
            if !url.starts_with('#') && shown != url && shown != bare {
                out.push_str(&format!(" ({url})"));
            }
        }
    }
    out
}

/// Separator before block `i`: list items in a row share lines, but a
/// numbered list straight after a bulleted one (or the reverse) gets a
/// blank line, so it reads as a list of its own.
fn separator(blocks: &[Block], depths: &[u8], i: usize) -> &'static str {
    let (Some(Block::Item { ordered, .. }), Some(prev)) = (blocks.get(i), i.checked_sub(1)) else {
        return "\n\n";
    };
    if !matches!(blocks[prev], Block::Item { .. }) {
        return "\n\n";
    }
    if depths[i] == 0 {
        // The list the previous items belong to, at the outer level.
        let top = (0..i)
            .rev()
            .take_while(|&j| matches!(blocks[j], Block::Item { .. }))
            .find(|&j| depths[j] == 0);
        if let Some(j) = top {
            if let Block::Item { ordered: o, .. } = &blocks[j] {
                if o != ordered {
                    return "\n\n";
                }
            }
        }
    }
    "\n"
}

/// Plain text. With `decorate`, the first two heading levels are
/// underlined, code is indented and tables are lined up, as a text file
/// would be written by hand; without, it's the bare text, for the
/// clipboard.
pub fn to_text(doc: &Doc, decorate: bool) -> String {
    let depths = normalized_depths(&doc.blocks);
    let numbers = item_numbers(&doc.blocks, &depths);
    let mut out = String::new();
    for (i, b) in doc.blocks.iter().enumerate() {
        if !out.is_empty() {
            out.push_str(separator(&doc.blocks, &depths, i));
        }
        match b {
            Block::Heading { level, text } => {
                let t = inline_text(text).replace('\n', " ");
                out.push_str(&t);
                if decorate && *level <= 2 {
                    let mark = if *level == 1 { '=' } else { '-' };
                    out.push('\n');
                    out.extend(std::iter::repeat_n(mark, t.chars().count().max(3)));
                }
            }
            Block::Paragraph(text) => out.push_str(&inline_text(text)),
            Block::Item { ordered, text, .. } => {
                let indent = "  ".repeat(depths[i] as usize);
                let marker = if *ordered {
                    format!("{}. ", numbers[i])
                } else {
                    "• ".to_string()
                };
                let pad = format!("{indent}{}", " ".repeat(marker.chars().count()));
                let body = inline_text(text).replace('\n', &format!("\n{pad}"));
                out.push_str(&format!("{indent}{marker}{body}"));
            }
            Block::Quote(text) => {
                let body = inline_text(text);
                let lines: Vec<String> = body.lines().map(|l| format!("> {l}")).collect();
                out.push_str(&lines.join("\n"));
            }
            Block::Code(code) => {
                if decorate {
                    let lines: Vec<String> = code
                        .lines()
                        .map(|l| {
                            if l.is_empty() {
                                String::new()
                            } else {
                                format!("    {l}")
                            }
                        })
                        .collect();
                    out.push_str(&lines.join("\n"));
                } else {
                    out.push_str(code);
                }
            }
            Block::Rule => {
                if decorate {
                    out.push_str("* * *");
                } else if out.ends_with("\n\n") {
                    out.truncate(out.len() - 2);
                }
            }
            Block::Table { rows, header } => out.push_str(&table_text(rows, *header, decorate)),
        }
    }
    let mut result = out.trim_end().to_string();
    result.push('\n');
    result
}

fn table_text(rows: &[Vec<Inline>], header: bool, decorate: bool) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| inline_text(c).replace('\n', " "))
                .collect()
        })
        .collect();
    if !decorate {
        return cells
            .iter()
            .map(|r| r.join("\t"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    let cols = cells.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for r in &cells {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count()).min(40);
        }
    }
    let line = |r: &Vec<String>| {
        let mut s = String::new();
        for (i, w) in widths.iter().enumerate() {
            let c = r.get(i).map(String::as_str).unwrap_or("");
            s.push_str(c);
            if i + 1 < cols {
                let n = c.chars().count();
                s.push_str(&" ".repeat(w.saturating_sub(n) + 2));
            }
        }
        s.trim_end().to_string()
    };
    let mut out = Vec::new();
    for (i, r) in cells.iter().enumerate() {
        out.push(line(r));
        if i == 0 && header && cells.len() > 1 {
            out.push(
                widths
                    .iter()
                    .map(|w| "-".repeat((*w).max(3)))
                    .collect::<Vec<_>>()
                    .join("  "),
            );
        }
    }
    out.join("\n")
}

// ---------------------------------------------------------------------
// Markdown

fn md_options() -> pulldown_cmark::Options {
    use pulldown_cmark::Options;
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

/// Reads Markdown (CommonMark with tables, task lists and footnotes).
pub fn from_markdown(src: &str) -> Doc {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    struct State {
        doc: Doc,
        buf: InlineBuf,
        bold: u32,
        italic: u32,
        links: Vec<String>,
        lists: Vec<bool>,
        items: u32,
        quote: u32,
        code: Option<String>,
        heading: Option<u8>,
        table: Option<(Vec<Vec<Inline>>, Vec<Inline>)>,
        in_cell: bool,
    }
    impl State {
        fn style(&self) -> Style {
            Style {
                bold: self.bold > 0,
                italic: self.italic > 0,
                code: false,
                link: self.links.last().cloned(),
            }
        }
        fn flush(&mut self) {
            if self.in_cell || self.heading.is_some() {
                return;
            }
            if self.buf.is_blank() {
                self.buf.take();
                return;
            }
            let text = self.buf.take();
            let block = if self.items > 0 && !self.lists.is_empty() {
                Block::Item {
                    ordered: *self.lists.last().unwrap_or(&false),
                    depth: (self.lists.len() - 1).min(8) as u8,
                    text,
                }
            } else if self.quote > 0 {
                Block::Quote(text)
            } else {
                Block::Paragraph(text)
            };
            self.doc.blocks.push(block);
        }
        fn push(&mut self, t: &str) {
            if let Some(code) = &mut self.code {
                code.push_str(t);
            } else {
                let style = self.style();
                self.buf.push(t, &style);
            }
        }
    }

    let mut st = State {
        doc: Doc::default(),
        buf: InlineBuf::default(),
        bold: 0,
        italic: 0,
        links: Vec::new(),
        lists: Vec::new(),
        items: 0,
        quote: 0,
        code: None,
        heading: None,
        table: None,
        in_cell: false,
    };
    for ev in Parser::new_ext(src, md_options()) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    st.flush();
                    st.heading = Some(level as u8);
                }
                Tag::BlockQuote(_) => {
                    st.flush();
                    st.quote += 1;
                }
                Tag::CodeBlock(_) => {
                    st.flush();
                    st.code = Some(String::new());
                }
                Tag::List(start) => {
                    st.flush();
                    st.lists.push(start.is_some());
                }
                Tag::Item => {
                    st.flush();
                    st.items += 1;
                }
                Tag::FootnoteDefinition(name) => {
                    st.flush();
                    let label = format!("[^{name}]: ");
                    st.push(&label);
                }
                Tag::Table(_) => {
                    st.flush();
                    st.table = Some((Vec::new(), Vec::new()));
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some((_, row)) = &mut st.table {
                        row.clear();
                    }
                }
                Tag::TableCell => {
                    st.in_cell = true;
                    st.buf.take();
                }
                Tag::Emphasis => st.italic += 1,
                Tag::Strong => st.bold += 1,
                Tag::Link { dest_url, .. } => st.links.push(dest_url.to_string()),
                Tag::Image { .. } => {}
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if st.items > 0 {
                        // Paragraphs of one item stay together.
                        if !st.buf.is_blank() {
                            st.push("\n");
                        }
                    } else {
                        st.flush();
                    }
                }
                TagEnd::Heading(_) => {
                    let level = st.heading.take().unwrap_or(1);
                    let text = st.buf.take();
                    if !text.is_empty() {
                        st.doc.blocks.push(Block::Heading { level, text });
                    }
                }
                TagEnd::BlockQuote(_) => {
                    st.flush();
                    st.quote = st.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    if let Some(code) = st.code.take() {
                        st.doc
                            .blocks
                            .push(Block::Code(code.trim_end_matches('\n').to_string()));
                    }
                }
                TagEnd::List(_) => {
                    st.flush();
                    st.lists.pop();
                }
                TagEnd::Item => {
                    st.flush();
                    st.items = st.items.saturating_sub(1);
                }
                TagEnd::FootnoteDefinition => st.flush(),
                TagEnd::TableCell => {
                    let cell = st.buf.take();
                    st.in_cell = false;
                    if let Some((_, row)) = &mut st.table {
                        row.push(cell);
                    }
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some((rows, row)) = &mut st.table {
                        rows.push(std::mem::take(row));
                    }
                }
                TagEnd::Table => {
                    if let Some((rows, _)) = st.table.take() {
                        st.doc.blocks.push(Block::Table { rows, header: true });
                    }
                }
                TagEnd::Emphasis => st.italic = st.italic.saturating_sub(1),
                TagEnd::Strong => st.bold = st.bold.saturating_sub(1),
                TagEnd::Link => {
                    st.links.pop();
                }
                _ => {}
            },
            Event::Text(t) => st.push(&t),
            Event::Code(t) => {
                let mut style = st.style();
                style.code = true;
                st.buf.push(&t, &style);
            }
            Event::SoftBreak => st.push(" "),
            Event::HardBreak => st.push("\n"),
            Event::Rule => {
                st.flush();
                st.doc.blocks.push(Block::Rule);
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                let t = strip_tags(&h);
                if !t.trim().is_empty() {
                    st.push(&t);
                }
            }
            Event::TaskListMarker(done) => st.push(if done { "[x] " } else { "[ ] " }),
            Event::FootnoteReference(name) => {
                let r = format!("[^{name}]");
                st.push(&r);
            }
            _ => {}
        }
    }
    st.flush();
    st.doc
}

/// Text with HTML tags and comments taken out.
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let end = if after.starts_with("<!--") {
            after.find("-->").map(|e| e + 3)
        } else {
            after.find('>').map(|e| e + 1)
        };
        match end {
            Some(e) => rest = &after[e..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn md_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Escapes what would start a heading, list or quote at a line start.
fn md_line_starts(s: &str) -> String {
    s.split('\n')
        .map(|line| {
            let first = line.chars().next();
            match first {
                Some('#' | '-' | '+' | '=' | '|') => format!("\\{line}"),
                Some(c) if c.is_ascii_digit() => {
                    let digits = line.chars().take_while(char::is_ascii_digit).count();
                    let rest = &line[digits..];
                    if rest.starts_with(". ")
                        || rest.starts_with(") ")
                        || rest == "."
                        || rest == ")"
                    {
                        format!("{}\\{}", &line[..digits], rest)
                    } else {
                        line.to_string()
                    }
                }
                _ => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn md_inline(inline: &[Span], in_table: bool) -> String {
    let mut out = String::new();
    for s in inline {
        if s.text.is_empty() {
            continue;
        }
        let lead: String = s.text.chars().take_while(|c| c.is_whitespace()).collect();
        let body_end = s.text.trim_end().len();
        let body = &s.text[lead.len()..body_end.max(lead.len())];
        let trail = &s.text[body_end.max(lead.len())..];
        let ws = |w: &str| {
            if in_table {
                w.replace('\n', " ")
            } else {
                w.replace('\n', "\\\n")
            }
        };
        out.push_str(&ws(&lead));
        if !body.is_empty() {
            let mut core = if s.style.code {
                let one_line = body.replace('\n', " ");
                let longest = one_line
                    .split(|c| c != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or(0);
                let fence = "`".repeat(longest + 1);
                let pad = if one_line.starts_with('`') || one_line.ends_with('`') {
                    " "
                } else {
                    ""
                };
                format!("{fence}{pad}{one_line}{pad}{fence}")
            } else {
                let escaped = md_escape(body);
                let escaped = if in_table {
                    escaped.replace('|', "\\|").replace('\n', " ")
                } else {
                    escaped.replace('\n', "\\\n")
                };
                match (s.style.bold, s.style.italic) {
                    (true, true) => format!("***{escaped}***"),
                    (true, false) => format!("**{escaped}**"),
                    (false, true) => format!("*{escaped}*"),
                    _ => escaped,
                }
            };
            if let Some(url) = s.style.link.as_deref().and_then(safe_link) {
                let url = if url.contains([' ', '(', ')', '<', '>']) {
                    format!("<{}>", url.replace(['<', '>'], ""))
                } else {
                    url.to_string()
                };
                core = format!("[{core}]({url})");
            }
            out.push_str(&core);
        }
        out.push_str(&ws(trail));
    }
    out
}

/// Writes Markdown (CommonMark, with GitHub tables).
pub fn to_markdown(doc: &Doc) -> String {
    let depths = normalized_depths(&doc.blocks);
    let numbers = item_numbers(&doc.blocks, &depths);
    let mut out = String::new();
    for (i, b) in doc.blocks.iter().enumerate() {
        if !out.is_empty() {
            out.push_str(separator(&doc.blocks, &depths, i));
        }
        match b {
            Block::Heading { level, text } => {
                let t = md_inline(text, true);
                out.push_str(&format!(
                    "{} {}",
                    "#".repeat((*level).clamp(1, 6) as usize),
                    t
                ));
            }
            Block::Paragraph(text) => out.push_str(&md_line_starts(&md_inline(text, false))),
            Block::Item { ordered, text, .. } => {
                let indent = "    ".repeat(depths[i] as usize);
                let marker = if *ordered {
                    format!("{}. ", numbers[i])
                } else {
                    "- ".to_string()
                };
                let pad = format!("{indent}{}", " ".repeat(marker.len()));
                let body = md_line_starts(&md_inline(text, false));
                let body = body.replace('\n', &format!("\n{pad}"));
                out.push_str(&format!("{indent}{marker}{body}"));
            }
            Block::Quote(text) => {
                let body = md_line_starts(&md_inline(text, false));
                let lines: Vec<String> = body
                    .split('\n')
                    .map(|l| format!("> {l}").trim_end().to_string())
                    .collect();
                out.push_str(&lines.join("\n"));
            }
            Block::Code(code) => {
                let longest = code
                    .lines()
                    .map(|l| l.trim_start().chars().take_while(|&c| c == '`').count())
                    .max()
                    .unwrap_or(0);
                let fence = "`".repeat(longest.max(2) + 1);
                out.push_str(&format!("{fence}\n{code}\n{fence}"));
            }
            Block::Rule => out.push_str("---"),
            Block::Table { rows, .. } => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
                let line = |r: Option<&Vec<Inline>>| {
                    let cells: Vec<String> = (0..cols)
                        .map(|c| {
                            r.and_then(|r| r.get(c))
                                .map(|cell| md_inline(cell, true))
                                .unwrap_or_default()
                        })
                        .collect();
                    format!("| {} |", cells.join(" | "))
                };
                let mut lines = vec![line(rows.first())];
                lines.push(format!("|{}|", vec![" --- "; cols].join("|")));
                for r in rows.iter().skip(1) {
                    lines.push(line(Some(r)));
                }
                out.push_str(&lines.join("\n"));
            }
        }
    }
    out.push('\n');
    out
}

// ---------------------------------------------------------------------
// HTML

pub(crate) fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Links that can't run anything: web, mail, in-page and relative.
pub fn safe_link(url: &str) -> Option<&str> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    let scheme_end = lower.find(':');
    let first_slash = lower.find(['/', '?', '#']);
    let has_scheme = match (scheme_end, first_slash) {
        (Some(c), Some(s)) => c < s,
        (Some(_), None) => true,
        _ => false,
    };
    let ok = !has_scheme
        || lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("mailto:");
    (ok && !u.is_empty()).then_some(u)
}

/// A standalone HTML page. `print` leaves out the dark-mode colours and
/// page-width limit, for turning into PDF pages.
pub fn html_page(title: &str, body: &str, print: bool) -> String {
    let style = if print {
        "body{font:11pt/1.5 -apple-system,\"Helvetica Neue\",Helvetica,Arial,sans-serif;color:#000;margin:0}\npre,code{font-family:Menlo,Courier,monospace;font-size:.9em}\npre{background:#f2f2f2;padding:.6em}\nblockquote{color:#444;border-left:3px solid #ccc;margin-left:0;padding-left:1em}\ntable{border-collapse:collapse}td,th{border:1px solid #999;padding:.2em .5em}\nh1{font-size:20pt}h2{font-size:16pt}h3{font-size:13pt}h4,h5,h6{font-size:11pt}\n"
            .to_string()
    } else {
        "body{font:16px/1.6 -apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;max-width:46rem;margin:2rem auto;padding:0 1rem;color:#1d1d1f}\npre,code{font-family:ui-monospace,Menlo,monospace;font-size:.9em}\npre{background:#f5f5f7;padding:1rem;overflow:auto;border-radius:8px}\ntable{border-collapse:collapse}td,th{border:1px solid #d2d2d7;padding:.3rem .6rem}\nimg{max-width:100%}\n@media (prefers-color-scheme:dark){body{background:#1d1d1f;color:#f5f5f7}pre{background:#2c2c2e}td,th{border-color:#48484a}a{color:#64a8ff}}\n"
            .to_string()
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>\n{style}</style>\n</head>\n<body>\n{body}</body>\n</html>\n",
        escape_html(title)
    )
}

fn html_inline(inline: &[Span]) -> String {
    let mut out = String::new();
    for s in inline {
        let mut t = escape_html(&s.text).replace('\n', "<br>\n");
        if s.style.code {
            t = format!("<code>{t}</code>");
        }
        if s.style.italic {
            t = format!("<em>{t}</em>");
        }
        if s.style.bold {
            t = format!("<strong>{t}</strong>");
        }
        if let Some(url) = s.style.link.as_deref().and_then(safe_link) {
            t = format!("<a href=\"{}\">{t}</a>", escape_html(url));
        }
        out.push_str(&t);
    }
    out
}

/// The body of an HTML page for a document.
pub fn html_body(doc: &Doc) -> String {
    let depths = normalized_depths(&doc.blocks);
    let mut out = String::new();
    // Open lists: one entry per depth, true for numbered.
    let mut open: Vec<bool> = Vec::new();
    let close_all = |open: &mut Vec<bool>, out: &mut String| {
        while let Some(o) = open.pop() {
            out.push_str(if o {
                "</li>\n</ol>\n"
            } else {
                "</li>\n</ul>\n"
            });
        }
    };
    for (i, b) in doc.blocks.iter().enumerate() {
        if let Block::Item { ordered, text, .. } = b {
            let d = depths[i] as usize;
            while open.len() > d + 1 {
                let o = open.pop().unwrap_or(false);
                out.push_str(if o {
                    "</li>\n</ol>\n"
                } else {
                    "</li>\n</ul>\n"
                });
            }
            if open.len() == d + 1 && open[d] != *ordered {
                let o = open.pop().unwrap_or(false);
                out.push_str(if o {
                    "</li>\n</ol>\n"
                } else {
                    "</li>\n</ul>\n"
                });
            }
            if open.len() == d + 1 {
                out.push_str("</li>\n<li>");
            } else {
                out.push_str(if *ordered {
                    "\n<ol>\n<li>"
                } else {
                    "\n<ul>\n<li>"
                });
                open.push(*ordered);
            }
            out.push_str(&html_inline(text));
            continue;
        }
        close_all(&mut open, &mut out);
        match b {
            Block::Heading { level, text } => {
                let l = (*level).clamp(1, 6);
                out.push_str(&format!("<h{l}>{}</h{l}>\n", html_inline(text)));
            }
            Block::Paragraph(text) => out.push_str(&format!("<p>{}</p>\n", html_inline(text))),
            Block::Quote(text) => out.push_str(&format!(
                "<blockquote><p>{}</p></blockquote>\n",
                html_inline(text)
            )),
            Block::Code(code) => {
                out.push_str(&format!("<pre><code>{}</code></pre>\n", escape_html(code)))
            }
            Block::Rule => out.push_str("<hr>\n"),
            Block::Table { rows, header } => {
                out.push_str("<table>\n");
                for (r, row) in rows.iter().enumerate() {
                    let head = *header && r == 0;
                    if head {
                        out.push_str("<thead>\n");
                    } else if r == 0 || (*header && r == 1) {
                        out.push_str("<tbody>\n");
                    }
                    out.push_str("<tr>");
                    for cell in row {
                        let tag = if head { "th" } else { "td" };
                        out.push_str(&format!("<{tag}>{}</{tag}>", html_inline(cell)));
                    }
                    out.push_str("</tr>\n");
                    if head {
                        out.push_str("</thead>\n");
                    }
                }
                if rows.len() > usize::from(*header) {
                    out.push_str("</tbody>\n");
                }
                out.push_str("</table>\n");
            }
            Block::Item { .. } => unreachable!("handled above"),
        }
    }
    close_all(&mut open, &mut out);
    out
}

pub fn to_html(doc: &Doc, title: &str, print: bool) -> String {
    let title = doc.title.as_deref().unwrap_or(title);
    html_page(title, &html_body(doc), print)
}

// ---------------------------------------------------------------------
// Reading and writing files

/// Reads any document Radlet understands into the model. `caps` is for
/// PDFs, whose text may need the Vision helper.
pub fn read(path: &Path, fmt: Format) -> Result<Doc> {
    use crate::engines::{data, docx, htmlread, rtf, text};
    let doc = match fmt {
        Format::Txt => from_text(&text::read_text(path)?),
        Format::Md => from_markdown(&text::read_text(path)?),
        Format::Html => htmlread::parse(&text::read_text(path)?),
        Format::Docx => docx::read(path)?,
        Format::Rtf => rtf::read(&std::fs::read(path)?)?,
        Format::Csv | Format::Tsv => data::table_doc(path, fmt)?,
        Format::Xlsx => crate::engines::sheet::doc(path)?,
        Format::Srt | Format::Vtt => {
            let cues = text::parse_subtitles(&text::read_text(path)?)?;
            from_text(&text::subtitles_to_text(&cues))
        }
        Format::Pdf => from_text(&crate::engines::pdf::to_text(path)?),
        other => {
            return Err(Error::UnsupportedInput(format!(
                "{other} files aren't documents"
            )))
        }
    };
    Ok(doc)
}

/// Writes a document as `to` (TXT, Markdown, HTML, Word or RTF).
pub fn write(doc: &Doc, to: Format, title: &str, out: &Path) -> Result<()> {
    use crate::engines::{docx, rtf};
    match to {
        Format::Txt => std::fs::write(out, to_text(doc, true))?,
        Format::Md => std::fs::write(out, to_markdown(doc))?,
        Format::Html => std::fs::write(out, to_html(doc, title, false))?,
        Format::Docx => std::fs::write(out, docx::write(doc, title)?)?,
        Format::Rtf => std::fs::write(out, rtf::write(doc))?,
        other => {
            return Err(Error::UnsupportedRoute {
                from: "a document".into(),
                to: other.label().into(),
            })
        }
    }
    Ok(())
}

/// Formats [`write`] can produce.
pub fn writable(f: Format) -> bool {
    matches!(
        f,
        Format::Txt | Format::Md | Format::Html | Format::Docx | Format::Rtf
    )
}

/// The title for a file: its name without the extension.
pub fn title_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Document".into())
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

    fn b(t: &str) -> Span {
        Span {
            text: t.into(),
            style: Style {
                bold: true,
                ..Default::default()
            },
        }
    }

    pub(crate) fn sample() -> Doc {
        Doc {
            title: None,
            blocks: vec![
                Block::Heading {
                    level: 1,
                    text: vec![s("Trip notes")],
                },
                Block::Paragraph(vec![
                    s("Pack "),
                    b("light"),
                    s(" and see "),
                    Span {
                        text: "the map".into(),
                        style: Style {
                            link: Some("https://example.com/map".into()),
                            ..Default::default()
                        },
                    },
                    s("."),
                ]),
                Block::Item {
                    ordered: false,
                    depth: 0,
                    text: vec![s("Passport")],
                },
                Block::Item {
                    ordered: false,
                    depth: 1,
                    text: vec![s("Copy too")],
                },
                Block::Item {
                    ordered: true,
                    depth: 0,
                    text: vec![s("Book *train*")],
                },
                Block::Code("let x = 1;\nlet y = 2;".into()),
                Block::Table {
                    rows: vec![
                        vec![vec![s("Day")], vec![s("City")]],
                        vec![vec![s("1")], vec![s("Lyon | Paris")]],
                    ],
                    header: true,
                },
            ],
        }
    }

    #[test]
    fn markdown_round_trips() {
        let md = to_markdown(&sample());
        assert!(
            md.starts_with(
                "# Trip notes\n\nPack **light** and see [the map](https://example.com/map)."
            ),
            "{md}"
        );
        assert!(
            md.contains("- Passport\n    - Copy too\n\n1. Book \\*train\\*"),
            "{md}"
        );
        assert!(md.contains("```\nlet x = 1;"), "{md}");
        assert!(md.contains("| 1 | Lyon \\| Paris |"), "{md}");
        let back = from_markdown(&md);
        assert_eq!(back.blocks.len(), sample().blocks.len(), "{back:#?}");
        assert_eq!(back.blocks[1], sample().blocks[1]);
        assert_eq!(back.blocks[4], sample().blocks[4]);
        assert_eq!(back.blocks[6], sample().blocks[6]);
        assert!(matches!(back.blocks[3], Block::Item { depth: 1, .. }));
    }

    #[test]
    fn text_and_html_writers() {
        let t = to_text(&sample(), true);
        assert!(
            t.starts_with(
                "Trip notes\n==========\n\nPack light and see the map (https://example.com/map)."
            ),
            "{t}"
        );
        assert!(
            t.contains("• Passport\n  • Copy too\n\n1. Book *train*"),
            "{t}"
        );
        assert!(t.contains("    let x = 1;"));
        assert!(
            t.contains("Day  City\n---  ------------\n1    Lyon | Paris"),
            "{t}"
        );
        let bare = to_text(&sample(), false);
        assert!(bare.starts_with("Trip notes\n\nPack light"));

        let h = html_body(&sample());
        assert!(h.contains("<h1>Trip notes</h1>"));
        assert!(h.contains("<strong>light</strong>"));
        assert!(h.contains("<a href=\"https://example.com/map\">the map</a>"));
        assert!(h.contains("<ul>\n<li>Passport\n<ul>\n<li>Copy too</li>\n</ul>\n</li>\n</ul>\n\n<ol>\n<li>Book *train*</li>\n</ol>"), "{h}");
        assert!(h.contains("<thead>\n<tr><th>Day</th><th>City</th></tr>\n</thead>\n<tbody>"));
        let page = to_html(&sample(), "Notes <1>", true);
        assert!(page.contains("<title>Notes &lt;1&gt;</title>"));
        assert!(
            !page.contains("prefers-color-scheme"),
            "print pages stay black on white"
        );
    }

    #[test]
    fn plain_text_reading() {
        let d = from_text("Groceries\r\n\r\n- eggs\n- milk\n  - oat\n\n1. one\n2) two\n\nJust a line\nand another  \n");
        assert_eq!(d.blocks.len(), 7, "{d:#?}");
        assert_eq!(d.blocks[0], Block::Paragraph(vec![s("Groceries")]));
        assert!(
            matches!(&d.blocks[3], Block::Item { ordered: false, depth: 1, text } if plain(text) == "oat")
        );
        assert!(matches!(&d.blocks[5], Block::Item { ordered: true, .. }));
        assert_eq!(
            d.blocks[6],
            Block::Paragraph(vec![s("Just a line\nand another")])
        );
    }

    #[test]
    fn links_that_could_run_code_are_dropped() {
        assert_eq!(safe_link("https://a.b/c"), Some("https://a.b/c"));
        assert_eq!(safe_link("notes/other.html"), Some("notes/other.html"));
        assert_eq!(safe_link("#top"), Some("#top"));
        assert_eq!(safe_link(" JavaScript:alert(1)"), None);
        assert_eq!(safe_link("data:text/html,x"), None);
        let d = Doc {
            title: None,
            blocks: vec![Block::Paragraph(vec![Span {
                text: "x".into(),
                style: Style {
                    link: Some("javascript:alert(1)".into()),
                    ..Default::default()
                },
            }])],
        };
        assert_eq!(html_body(&d), "<p>x</p>\n");
    }

    #[test]
    fn counting_words() {
        assert_eq!(count_words("Hello, world! It's a well-known fact."), 6);
        assert_eq!(count_words("日本語の文"), 5);
        assert_eq!(count_words("  "), 0);
        assert_eq!(word_count_label("one"), "1 word · 1 min read");
        let many = "word ".repeat(1234);
        assert_eq!(word_count_label(&many), "1,234 words · 6 min read");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn escaping_line_starts() {
        let d = Doc {
            title: None,
            blocks: vec![Block::Paragraph(vec![s(
                "# not a heading\n1. not a list\n- nor this",
            )])],
        };
        let md = to_markdown(&d);
        let back = from_markdown(&md);
        assert_eq!(back.blocks.len(), 1, "{md}");
        assert_eq!(
            plain(match &back.blocks[0] {
                Block::Paragraph(t) => t,
                other => panic!("{other:?}"),
            }),
            "# not a heading\n1. not a list\n- nor this"
        );
    }
}
