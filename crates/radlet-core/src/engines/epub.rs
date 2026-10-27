//! E-books: EPUB 3 written from a document (a chapter for each top-level
//! heading, with a table of contents for EPUB 3 and EPUB 2 readers), and
//! EPUBs read back as one document, chapter by chapter in reading order.
//! E-books protected with DRM are refused, not read as noise.

use std::io::{Read, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::engines::doc::{self, Block, Doc};
use crate::error::{Error, Result};

/// Most bytes read from one e-book's chapters.
const MAX_TEXT: u64 = 256 << 20;

fn bad(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::UnsupportedInput(format!(
        "{} isn't an e-book Radlet can read ({why})",
        crate::format::display_name(path)
    ))
}

/// Text safe inside XML: markup escaped, and control characters (which
/// XML forbids) dropped.
fn xml_text(s: &str) -> String {
    doc::escape_html(s)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t' | '\r'))
        .collect()
}

fn plain(inline: &doc::Inline) -> String {
    inline.iter().map(|s| s.text.as_str()).collect::<String>()
}

/// The date and time now, as EPUB wants it: 2026-10-27T09:30:00Z.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

fn uuid() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The document split into chapters at its top-level headings.
fn chapters(d: &Doc) -> Vec<(String, Doc)> {
    let top = d
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Heading { level, .. } => Some(*level),
            _ => None,
        })
        .min();
    let mut out: Vec<(String, Doc)> = Vec::new();
    for b in &d.blocks {
        let starts = matches!(b, Block::Heading { level, .. } if Some(*level) == top);
        if starts || out.is_empty() {
            let title = match b {
                Block::Heading { text, .. } if starts => plain(text),
                _ => String::new(),
            };
            out.push((title, Doc::default()));
        }
        out.last_mut()
            .expect("pushed above")
            .1
            .blocks
            .push(b.clone());
    }
    if out.is_empty() {
        out.push((String::new(), Doc::default()));
    }
    out
}

fn xhtml(title: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
         <head>\n<meta charset=\"utf-8\"/>\n<title>{}</title>\n\
         <link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>\n</head>\n<body>\n{body}</body>\n</html>\n",
        xml_text(title)
    )
}

const STYLE: &str = "body{font-family:serif;line-height:1.5;margin:0 5%}\n\
h1,h2,h3{font-family:sans-serif;line-height:1.2}\n\
pre,code{font-family:monospace;font-size:.9em}\npre{white-space:pre-wrap}\n\
blockquote{margin-left:1.5em;font-style:italic}\n\
table{border-collapse:collapse}td,th{border:1px solid #999;padding:.2em .4em}\n";

/// Writes `d` as an EPUB 3 e-book at `out`.
pub fn write(d: &Doc, title: &str, out: &Path) -> Result<()> {
    let title = d.title.as_deref().unwrap_or(title);
    let parts = chapters(d);
    let id = uuid();
    let file = std::fs::File::create(out)?;
    let mut z = ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let packed = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let zerr =
        |e: zip::result::ZipError| Error::Internal(format!("couldn't write the e-book ({e})"));
    // The mimetype comes first, uncompressed, as readers expect.
    z.start_file("mimetype", stored).map_err(zerr)?;
    z.write_all(b"application/epub+zip")?;
    z.start_file("META-INF/container.xml", packed)
        .map_err(zerr)?;
    z.write_all(
        b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<container version=\"1.0\" \
          xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n<rootfiles>\n\
          <rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/>\n\
          </rootfiles>\n</container>\n",
    )?;
    z.start_file("OEBPS/style.css", packed).map_err(zerr)?;
    z.write_all(STYLE.as_bytes())?;

    let mut manifest = String::new();
    let mut spine = String::new();
    let mut nav = String::new();
    let mut ncx = String::new();
    for (i, (name, part)) in parts.iter().enumerate() {
        let n = i + 1;
        let href = format!("chapter-{n:03}.xhtml");
        let label = if name.trim().is_empty() {
            if i == 0 {
                title.to_string()
            } else {
                format!("Part {n}")
            }
        } else {
            name.trim().to_string()
        };
        let body = doc::html_body(part)
            .replace("<br>", "<br/>")
            .replace("<hr>", "<hr/>")
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t' | '\r'))
            .collect::<String>();
        z.start_file(format!("OEBPS/{href}"), packed)
            .map_err(zerr)?;
        z.write_all(xhtml(&label, &body).as_bytes())?;
        manifest.push_str(&format!(
            "<item id=\"c{n}\" href=\"{href}\" media-type=\"application/xhtml+xml\"/>\n"
        ));
        spine.push_str(&format!("<itemref idref=\"c{n}\"/>\n"));
        nav.push_str(&format!(
            "<li><a href=\"{href}\">{}</a></li>\n",
            xml_text(&label)
        ));
        ncx.push_str(&format!(
            "<navPoint id=\"p{n}\" playOrder=\"{n}\"><navLabel><text>{}</text></navLabel>\
             <content src=\"{href}\"/></navPoint>\n",
            xml_text(&label)
        ));
    }
    z.start_file("OEBPS/nav.xhtml", packed).map_err(zerr)?;
    z.write_all(
        xhtml(
            "Contents",
            &format!(
                "<nav epub:type=\"toc\" id=\"toc\">\n<h1>Contents</h1>\n<ol>\n{nav}</ol>\n</nav>\n"
            ),
        )
        .as_bytes(),
    )?;
    z.start_file("OEBPS/toc.ncx", packed).map_err(zerr)?;
    z.write_all(
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" \
             version=\"2005-1\">\n<head><meta name=\"dtb:uid\" content=\"urn:uuid:{id}\"/></head>\n\
             <docTitle><text>{}</text></docTitle>\n<navMap>\n{ncx}</navMap>\n</ncx>\n",
            xml_text(title)
        )
        .as_bytes(),
    )?;
    z.start_file("OEBPS/content.opf", packed).map_err(zerr)?;
    z.write_all(
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" \
             version=\"3.0\" unique-identifier=\"id\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
             <dc:identifier id=\"id\">urn:uuid:{id}</dc:identifier>\n<dc:title>{}</dc:title>\n\
             <dc:language>en</dc:language>\n<meta property=\"dcterms:modified\">{}</meta>\n\
             </metadata>\n<manifest>\n\
             <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n\
             <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n\
             <item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n{manifest}</manifest>\n\
             <spine toc=\"ncx\">\n{spine}</spine>\n</package>\n",
            xml_text(title),
            utc_now()
        )
        .as_bytes(),
    )?;
    z.finish().map_err(zerr)?;
    Ok(())
}

// MARK: Reading

fn attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == name)
            .then(|| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
            .flatten()
            .map(|v| v.into_owned())
    })
}

/// A relative link resolved against a folder inside the archive.
fn join(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let decoded = percent_decode(href);
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for p in decoded.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let digit = |c: u8| (c as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (digit(b[i + 1]), digit(b[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn entry(z: &mut ZipArchive<std::fs::File>, name: &str, budget: &mut u64) -> Option<String> {
    let f = z.by_name(name).ok()?;
    let mut s = String::new();
    let mut limited = f.take(*budget);
    let mut raw = Vec::new();
    limited.read_to_end(&mut raw).ok()?;
    *budget = budget.saturating_sub(raw.len() as u64);
    s.push_str(&crate::engines::text::decode(&raw));
    Some(s)
}

/// Reads an EPUB as one document, in reading order.
pub fn read(path: &Path) -> Result<Doc> {
    use quick_xml::events::Event;
    let mut z = ZipArchive::new(std::fs::File::open(path)?).map_err(|e| bad(path, e))?;
    let mut budget = MAX_TEXT;
    let container = entry(&mut z, "META-INF/container.xml", &mut budget)
        .ok_or_else(|| bad(path, "no META-INF/container.xml"))?;
    let mut opf_path = None;
    let mut r = quick_xml::Reader::from_str(&container);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.local_name().as_ref() == "rootfile" => {
                opf_path = attr(&e, "full-path");
                break;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let opf_path = opf_path.ok_or_else(|| bad(path, "no package file"))?;
    let base = opf_path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();
    let opf =
        entry(&mut z, &opf_path, &mut budget).ok_or_else(|| bad(path, "a missing package file"))?;

    let mut title = None;
    let mut items: Vec<(String, String)> = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut in_title = false;
    let mut r = quick_xml::Reader::from_str(&opf);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "item" => {
                    if let (Some(id), Some(href)) = (attr(&e, "id"), attr(&e, "href")) {
                        items.push((id, href));
                    }
                }
                "itemref" => {
                    let linear = attr(&e, "linear").is_none_or(|l| l != "no");
                    if let (Some(id), true) = (attr(&e, "idref"), linear) {
                        order.push(id);
                    }
                }
                "title" if title.is_none() => in_title = true,
                _ => {}
            },
            Ok(Event::Text(t)) if in_title => {
                title = Some(t.xml10_content().trim().to_string());
                in_title = false;
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "title" => in_title = false,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    // DRM encrypts the chapters themselves; fonts alone are often
    // obfuscated, which doesn't matter for the text.
    let hrefs: std::collections::HashMap<&str, &str> = items
        .iter()
        .map(|(id, href)| (id.as_str(), href.as_str()))
        .collect();
    if let Some(enc) = entry(&mut z, "META-INF/encryption.xml", &mut budget) {
        let protected = order.iter().any(|id| {
            hrefs
                .get(id.as_str())
                .is_some_and(|href| !href.is_empty() && enc.contains(href))
        });
        if protected {
            return Err(Error::UnsupportedInput(
                "this e-book is protected by DRM, so its text can't be read".into(),
            ));
        }
    }
    let mut out = Doc {
        title: title.filter(|t| !t.is_empty()),
        blocks: Vec::new(),
    };
    for id in &order {
        let Some(href) = hrefs.get(id.as_str()) else {
            continue;
        };
        let Some(html) = entry(&mut z, &join(&base, href), &mut budget) else {
            continue;
        };
        out.blocks
            .extend(crate::engines::htmlread::parse(&html).blocks);
        if budget == 0 {
            return Err(Error::TooLarge(
                "this e-book's text is larger than 256 MB".into(),
            ));
        }
    }
    if out.blocks.is_empty() {
        return Err(Error::NothingToDo("there's no text in this e-book".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_back() {
        let d = doc::from_markdown(
            "Before it all.\n\n# One\n\nThe *first* chapter & more.\n\n- a\n- b\n\n# Two\n\nLine<br>\n\n---\n\nEnd.\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let book = dir.path().join("book.epub");
        write(&d, "My book", &book).unwrap();
        assert_eq!(
            crate::format::detect(&book).unwrap(),
            crate::format::Format::Epub
        );
        let mut z = ZipArchive::new(std::fs::File::open(&book).unwrap()).unwrap();
        assert_eq!(z.by_index(0).unwrap().name().unwrap(), "mimetype");
        let mut chapter = String::new();
        z.by_name("OEBPS/chapter-002.xhtml")
            .unwrap()
            .read_to_string(&mut chapter)
            .unwrap();
        assert!(
            chapter.contains("<em>first</em> chapter &amp; more"),
            "{chapter}"
        );
        // Each chapter is well-formed XML.
        for i in 1..=3 {
            let mut x = String::new();
            z.by_name(&format!("OEBPS/chapter-{i:03}.xhtml"))
                .unwrap()
                .read_to_string(&mut x)
                .unwrap();
            let mut r = quick_xml::Reader::from_str(&x);
            loop {
                match r.read_event() {
                    Ok(quick_xml::events::Event::Eof) => break,
                    Ok(_) => {}
                    Err(e) => panic!("chapter {i}: {e}\n{x}"),
                }
            }
        }
        let back = read(&book).unwrap();
        assert_eq!(back.title.as_deref(), Some("My book"));
        let text = doc::to_text(&back, false);
        assert!(
            text.contains("Before it all.") && text.contains("End."),
            "{text}"
        );
        assert!(text.find("One").unwrap() < text.find("Two").unwrap());
    }

    #[test]
    fn links_inside_the_archive() {
        assert_eq!(
            join("OEBPS/text", "../images/a%20b.png#x"),
            "OEBPS/images/a b.png"
        );
        assert_eq!(join("", "ch1.xhtml"), "ch1.xhtml");
        assert!(utc_now().ends_with('Z') && utc_now().len() == 20);
    }

    #[test]
    fn odd_escapes_in_links() {
        assert_eq!(percent_decode("a%20b.xhtml"), "a b.xhtml");
        assert_eq!(percent_decode("%aé.xhtml"), "%aé.xhtml");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
