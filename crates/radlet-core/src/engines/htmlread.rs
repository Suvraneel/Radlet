//! Reads HTML into the document model. A forgiving tag scanner, not a
//! browser: it keeps headings, paragraphs, lists, quotes, code, tables,
//! emphasis and links, and drops scripts, styles and everything in
//! `<head>` but the title.

use crate::engines::doc::{Block, Doc, Inline, InlineBuf, Style};

/// Elements whose start or end separates paragraphs.
const BLOCKS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "aside",
    "nav",
    "figure",
    "figcaption",
    "address",
    "dd",
    "dt",
    "dl",
    "center",
    "form",
    "fieldset",
    "details",
    "summary",
    "body",
    "html",
];

/// Elements whose contents are never shown.
const HIDDEN: &[&str] = &[
    "script", "style", "head", "template", "noscript", "iframe", "object", "svg", "math", "select",
    "button", "canvas", "audio", "video",
];

struct Tag<'a> {
    name: String,
    end: bool,
    attrs: &'a str,
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let before_ok = at == 0 || bytes[at - 1].is_ascii_whitespace();
        let rest = lower[at + name.len()..].trim_start();
        if before_ok && rest.starts_with('=') {
            let offset = attrs.len() - rest.len() + 1;
            let value = attrs[offset..].trim_start();
            let v = match value.chars().next() {
                Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
                _ => value
                    .split(|c: char| c.is_ascii_whitespace() || c == '>')
                    .next()
                    .unwrap_or(""),
            };
            return Some(decode_entities(v));
        }
        from = at + name.len();
    }
    None
}

/// Decodes character references: named ones in common use, and numbers.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after
            .char_indices()
            .take(32)
            .find(|(_, c)| *c == ';')
            .map(|(e, _)| e);
        let decoded = end.and_then(|e| {
            let name = &after[..e];
            let c = if let Some(num) = name.strip_prefix('#') {
                let n = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse().ok(),
                }?;
                char::from_u32(n).filter(|c| *c != '\0')?
            } else {
                named_entity(name)?
            };
            Some((c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &after[e + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "sbquo" => '‚',
        "bdquo" => '„',
        "bull" => '•',
        "middot" => '·',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "laquo" => '«',
        "raquo" => '»',
        "iexcl" => '¡',
        "iquest" => '¿',
        "shy" => '\u{ad}',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "check" => '✓',
        "minus" => '−',
        "eacute" => 'é',
        "egrave" => 'è',
        "agrave" => 'à',
        "aacute" => 'á',
        "ccedil" => 'ç',
        "uuml" => 'ü',
        "ouml" => 'ö',
        "auml" => 'ä',
        "szlig" => 'ß',
        "ntilde" => 'ñ',
        _ => return None,
    })
}

struct Reader {
    doc: Doc,
    buf: InlineBuf,
    bold: u32,
    italic: u32,
    code: u32,
    links: Vec<Option<String>>,
    lists: Vec<bool>,
    in_item: bool,
    quote: u32,
    pre: Option<String>,
    heading: Option<u8>,
    table: Vec<Table>,
    title: Option<String>,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<Inline>>,
    row: Vec<Inline>,
    /// The first row is all `<th>` cells.
    header_first: bool,
    in_cell: bool,
    saved: Option<InlineBuf>,
}

impl Reader {
    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0,
            italic: self.italic > 0,
            code: self.code > 0,
            link: self.links.iter().rev().flatten().next().cloned(),
        }
    }

    fn in_cell(&self) -> bool {
        self.table.last().is_some_and(|t| t.in_cell)
    }

    /// Ends the paragraph being collected.
    fn flush(&mut self) {
        if self.in_cell() {
            if self.buf.last_char().is_some_and(|c| c != '\n') {
                let style = self.style();
                self.buf.push("\n", &style);
            }
            return;
        }
        if self.buf.is_blank() {
            self.buf.take();
            return;
        }
        let text = self.buf.take();
        let block = if let Some(level) = self.heading {
            Block::Heading { level, text }
        } else if self.in_item && !self.lists.is_empty() {
            // Later paragraphs of the same item continue it as a new item
            // at the same depth.
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

    fn text(&mut self, raw: &str) {
        if let Some(pre) = &mut self.pre {
            pre.push_str(&decode_entities(raw));
            return;
        }
        let decoded = decode_entities(raw);
        // White space collapses to one space, as a browser shows it.
        let mut collapsed = String::with_capacity(decoded.len());
        let mut space = matches!(self.buf.last_char(), None | Some(' ' | '\n'));
        for c in decoded.chars() {
            if c.is_whitespace() && c != '\u{a0}' {
                if !space {
                    collapsed.push(' ');
                    space = true;
                }
            } else {
                collapsed.push(if c == '\u{a0}' { ' ' } else { c });
                space = false;
            }
        }
        let style = self.style();
        self.buf.push(&collapsed, &style);
    }

    fn start(&mut self, name: &str, attrs: &str) {
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = Some(name.as_bytes()[1] - b'0');
            }
            "br" => {
                let style = self.style();
                self.buf.push("\n", &style);
            }
            "hr" => {
                self.flush();
                if !self.in_cell() {
                    self.doc.blocks.push(Block::Rule);
                }
            }
            "ul" | "ol" | "menu" => {
                self.flush();
                self.lists.push(name == "ol");
                self.in_item = false;
            }
            "li" => {
                self.flush();
                self.in_item = true;
            }
            "blockquote" => {
                self.flush();
                self.quote += 1;
            }
            "pre" => {
                self.flush();
                self.pre = Some(String::new());
            }
            "b" | "strong" => self.bold += 1,
            "i" | "em" | "cite" | "var" | "dfn" => self.italic += 1,
            "code" | "kbd" | "samp" | "tt" => self.code += 1,
            "a" => self.links.push(attr(attrs, "href")),
            "img" => {
                if let Some(alt) = attr(attrs, "alt").filter(|a| !a.trim().is_empty()) {
                    self.text(&alt);
                }
            }
            "table" => {
                self.flush();
                let saved = std::mem::take(&mut self.buf);
                self.table.push(Table {
                    saved: Some(saved),
                    ..Default::default()
                });
            }
            "tr" => {
                // `</td>` and `</tr>` may be left out: a new row ends the
                // last one.
                let pending = self.buf.take();
                if let Some(t) = self.table.last_mut() {
                    if t.in_cell {
                        t.row.push(pending);
                        t.in_cell = false;
                    }
                    if !t.row.is_empty() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
            }
            "td" | "th" => {
                if let Some(t) = self.table.last_mut() {
                    if t.in_cell {
                        // The previous cell wasn't closed.
                        let cell = self.buf.take();
                        t.row.push(cell);
                    }
                    t.in_cell = true;
                }
                self.buf.take();
            }
            n if BLOCKS.contains(&n) => self.flush(),
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = None;
            }
            "ul" | "ol" | "menu" => {
                self.flush();
                self.lists.pop();
                self.in_item = !self.lists.is_empty();
            }
            "li" => {
                self.flush();
                self.in_item = false;
            }
            "blockquote" => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            "pre" => {
                if let Some(code) = self.pre.take() {
                    let code = code.trim_matches('\n').to_string();
                    if self.in_cell() {
                        let style = self.style();
                        self.buf.push(&code, &style);
                    } else if !code.trim().is_empty() {
                        self.doc.blocks.push(Block::Code(code));
                    }
                }
            }
            "b" | "strong" => self.bold = self.bold.saturating_sub(1),
            "i" | "em" | "cite" | "var" | "dfn" => self.italic = self.italic.saturating_sub(1),
            "code" | "kbd" | "samp" | "tt" => self.code = self.code.saturating_sub(1),
            "a" => {
                self.links.pop();
            }
            "td" | "th" => {
                let cell = self.buf.take();
                if let Some(t) = self.table.last_mut() {
                    if t.in_cell {
                        t.row.push(cell);
                        t.in_cell = false;
                    }
                }
            }
            "tr" => {
                if let Some(t) = self.table.last_mut() {
                    if t.in_cell {
                        let cell = self.buf.take();
                        t.row.push(cell);
                        t.in_cell = false;
                    }
                    if !t.row.is_empty() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
            }
            "table" => {
                if let Some(mut t) = self.table.pop() {
                    if t.in_cell {
                        t.row.push(self.buf.take());
                    }
                    if !t.row.is_empty() {
                        t.rows.push(std::mem::take(&mut t.row));
                    }
                    self.buf = t.saved.take().unwrap_or_default();
                    if t.rows.is_empty() {
                        return;
                    }
                    let header = t.header_first;
                    if self.in_cell() {
                        // A table inside a table: keep its text.
                        let style = self.style();
                        for row in &t.rows {
                            let line: Vec<String> =
                                row.iter().map(|c| crate::engines::doc::plain(c)).collect();
                            self.buf.push(&line.join(" "), &style);
                            self.buf.push("\n", &style);
                        }
                    } else {
                        self.doc.blocks.push(Block::Table {
                            rows: t.rows,
                            header,
                        });
                    }
                }
            }
            n if BLOCKS.contains(&n) => self.flush(),
            _ => {}
        }
    }
}

/// Parses an HTML document.
pub fn parse(html: &str) -> Doc {
    let mut r = Reader {
        doc: Doc::default(),
        buf: InlineBuf::default(),
        bold: 0,
        italic: 0,
        code: 0,
        links: Vec::new(),
        lists: Vec::new(),
        in_item: false,
        quote: 0,
        pre: None,
        heading: None,
        table: Vec::new(),
        title: None,
    };
    let mut rest = html;
    let mut hidden: Option<String> = None;
    let mut header_first: Vec<Option<bool>> = Vec::new();
    // Lowercased once, for finding end tags without caring about case.
    // ASCII lowercasing keeps every byte where it was.
    let lower = html.to_ascii_lowercase();
    let find_ci = |rest: &str, needle: &str| -> Option<usize> {
        lower[html.len() - rest.len()..].find(needle)
    };
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if hidden.is_none() {
                r.text(rest);
            }
            break;
        };
        if lt > 0 && hidden.is_none() {
            r.text(&rest[..lt]);
        }
        let after = &rest[lt..];
        if after.starts_with("<!--") {
            rest = after.find("-->").map(|e| &after[e + 3..]).unwrap_or("");
            continue;
        }
        if after.starts_with("<!") || after.starts_with("<?") {
            rest = after.find('>').map(|e| &after[e + 1..]).unwrap_or("");
            continue;
        }
        let Some(tag) = scan_tag(after) else {
            // A lone "<" is text.
            if hidden.is_none() {
                r.text("<");
            }
            rest = &after[1..];
            continue;
        };
        let len = tag.1;
        let tag = tag.0;
        rest = &after[len..];
        if let Some(h) = &hidden {
            if tag.end && tag.name == *h {
                hidden = None;
            } else if h == "head" && tag.name == "title" && !tag.end {
                let end = find_ci(rest, "</title").unwrap_or(rest.len());
                let t = decode_entities(rest[..end].trim());
                if !t.is_empty() {
                    r.title = Some(t);
                }
                rest = &rest[end..];
            }
            continue;
        }
        if !tag.end && HIDDEN.contains(&tag.name.as_str()) {
            let self_closing = tag.attrs.trim_end().ends_with('/');
            if self_closing {
                continue;
            }
            if tag.name == "head" {
                hidden = Some("head".into());
            } else {
                // Skip to the matching end tag. Without one, a script or
                // style runs to the end (as in a browser); anything else is
                // taken as empty rather than losing the rest.
                let close = format!("</{}", tag.name);
                match find_ci(rest, &close) {
                    Some(i) => {
                        let tail = &rest[i..];
                        rest = tail.find('>').map(|e| &tail[e + 1..]).unwrap_or("");
                    }
                    None if matches!(tag.name.as_str(), "script" | "style") => rest = "",
                    None => {}
                }
            }
            continue;
        }
        if tag.name == "title" && !tag.end {
            let end = find_ci(rest, "</title").unwrap_or(rest.len());
            let t = decode_entities(rest[..end].trim());
            if !t.is_empty() {
                r.title = Some(t);
            }
            rest = &rest[end..];
            continue;
        }
        // Track whether each table's first row is all header cells.
        match (tag.name.as_str(), tag.end) {
            ("table", false) => header_first.push(None),
            ("td", false) => {
                if let Some(h) = header_first.last_mut() {
                    if r.table.last().is_some_and(|t| t.rows.is_empty()) {
                        *h = Some(false);
                    }
                }
            }
            ("th", false) => {
                if let Some(h) = header_first.last_mut() {
                    if h.is_none() && r.table.last().is_some_and(|t| t.rows.is_empty()) {
                        *h = Some(true);
                    }
                }
            }
            _ => {}
        }
        if tag.end {
            if tag.name == "table" {
                let first = header_first.pop().flatten().unwrap_or(false);
                if let Some(t) = r.table.last_mut() {
                    t.header_first = first;
                }
            }
            r.end(&tag.name);
        } else {
            r.start(&tag.name, tag.attrs);
            let self_closing = tag.attrs.trim_end().ends_with('/');
            if self_closing && !matches!(tag.name.as_str(), "br" | "hr" | "img") {
                r.end(&tag.name);
            }
        }
    }
    r.flush();
    if let Some(code) = r.pre.take() {
        if !code.trim().is_empty() {
            r.doc.blocks.push(Block::Code(code));
        }
    }
    r.doc.title = r.title;
    r.doc
}

/// Reads `<name attrs>` or `</name>` at the start of `s`, returning the
/// tag and its length in bytes.
fn scan_tag(s: &str) -> Option<(Tag<'_>, usize)> {
    let bytes = s.as_bytes();
    let mut i = 1;
    let end = bytes.get(i) == Some(&b'/');
    if end {
        i += 1;
    }
    let name_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    if i == name_start || !bytes[name_start].is_ascii_alphabetic() {
        return None;
    }
    let name = s[name_start..i].to_ascii_lowercase();
    let attrs_start = i;
    // Find the closing '>' outside quotes.
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => {
                return Some((
                    Tag {
                        name,
                        end,
                        attrs: &s[attrs_start..i],
                    },
                    i + 1,
                ));
            }
            None => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::doc::plain;

    #[test]
    fn reads_structure_and_skips_noise() {
        let html = r#"<!DOCTYPE html><html><head><title>My &amp; notes</title>
<style>p { color: red }</style><script>alert("<p>no</p>")</script></head>
<body><!-- hidden --><h1>Big   <em>idea</em></h1>
<p>One <b>bold</b> and <a href="https://x.y/?a=1&amp;b=2">a link</a>.<br>Next line</p>
<ul><li>First<ul><li>Inner</li></ul></li><li>Second</li></ul>
<ol><li>Step</li></ol>
<blockquote><p>Quoted</p></blockquote>
<pre><code>fn main() {
    &lt;tag&gt;
}</code></pre>
<table><tr><th>Name</th><th>Qty</th></tr><tr><td>Tea</td><td>2</td></tr></table>
<p>Caf&eacute; &#233; &#x1F600; a&nbsp;b 3 &lt; 4 &unknown;</p>
</body></html>"#;
        let d = parse(html);
        assert_eq!(d.title.as_deref(), Some("My & notes"));
        let b = &d.blocks;
        assert!(
            matches!(&b[0], Block::Heading { level: 1, text } if plain(text) == "Big idea"),
            "{b:#?}"
        );
        let Block::Paragraph(p) = &b[1] else {
            panic!("{b:#?}")
        };
        assert_eq!(plain(p), "One bold and a link.\nNext line");
        assert!(p.iter().any(|s| s.style.bold && s.text == "bold"));
        assert!(p
            .iter()
            .any(|s| s.style.link.as_deref() == Some("https://x.y/?a=1&b=2")));
        assert!(
            matches!(&b[2], Block::Item { ordered: false, depth: 0, text } if plain(text) == "First")
        );
        assert!(matches!(&b[3], Block::Item { depth: 1, text, .. } if plain(text) == "Inner"));
        assert!(matches!(&b[4], Block::Item { depth: 0, text, .. } if plain(text) == "Second"));
        assert!(matches!(&b[5], Block::Item { ordered: true, .. }));
        assert!(matches!(&b[6], Block::Quote(t) if plain(t) == "Quoted"));
        assert_eq!(b[7], Block::Code("fn main() {\n    <tag>\n}".into()));
        let Block::Table { rows, header } = &b[8] else {
            panic!("{b:#?}")
        };
        assert!(*header);
        assert_eq!(plain(&rows[1][0]), "Tea");
        let Block::Paragraph(p) = &b[9] else { panic!() };
        assert_eq!(plain(p), "Café é 😀 a b 3 < 4 &unknown;");
        assert_eq!(b.len(), 10);
    }

    #[test]
    fn tolerates_left_out_tags_and_empty_elements() {
        let d = parse("<table><tr><td>a</td><td>b</td><tr><td>c<td>d</table>");
        let Block::Table { rows, .. } = &d.blocks[0] else {
            panic!("{:#?}", d.blocks)
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(plain(&rows[0][1]), "b");
        assert_eq!(plain(&rows[1][1]), "d");
        let d = parse("<p>before</p><svg width='0' height='0'/><p>after</p><script>never closed");
        assert_eq!(d.blocks.len(), 2, "{:#?}", d.blocks);
        // Long pages with many hidden elements stay fast.
        let big = "<svg><path/></svg><p>x</p>".repeat(20_000);
        let t = std::time::Instant::now();
        assert_eq!(parse(&big).blocks.len(), 20_000);
        assert!(t.elapsed().as_secs() < 20);
    }

    #[test]
    fn tolerates_broken_markup() {
        let d = parse("Loose text <b>unclosed <i>tags</b> and a < sign\n<p>para");
        assert_eq!(d.blocks.len(), 2, "{:#?}", d.blocks);
        let d = parse("<table><td>a<td>b</table>");
        assert!(matches!(&d.blocks[0], Block::Table { rows, header: false } if rows[0].len() == 2));
    }
}
