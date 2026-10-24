//! Privacy check: what a file says about where, when, on what and by whom
//! it was made, and what else it carries that the picture or page doesn't
//! show. Each finding says whether "Remove metadata" takes it out.

use std::io::Read;
use std::path::Path;

use serde::Serialize;

use crate::catalog::Capabilities;
use crate::engines::media;
use crate::error::{Error, Result};
use crate::format::{self, Category, Format};

/// What a finding reveals, most sensitive first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Where it was made.
    Location,
    /// Who made or owns it: names, serial numbers.
    Identity,
    /// The camera, phone or scanner.
    Device,
    /// When it was made or changed.
    Time,
    /// The apps that made or edited it.
    Software,
    /// Content you can't see: previews, earlier versions, attachments.
    Hidden,
    /// Titles, descriptions, comments.
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub kind: Kind,
    /// "Location", "Camera", "Author"…
    pub label: String,
    /// What the file says, shortened.
    pub detail: String,
    /// Whether Remove metadata takes it out.
    pub removable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub format: Format,
    pub findings: Vec<Finding>,
}

/// Most of a file is read to look for metadata; beyond this only the start
/// is searched (metadata sits near the start or end of most formats).
const READ_LIMIT: u64 = 64 << 20;

/// Checks one file. Damaged files can't crash the caller: a surprise in
/// one file is reported as that file's error.
pub fn check(path: &Path, caps: &Capabilities) -> Result<Report> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_file(path, caps)))
        .unwrap_or_else(|_| Err(Error::Internal("this file couldn't be checked".into())))
}

fn check_file(path: &Path, caps: &Capabilities) -> Result<Report> {
    let fmt = format::detect(path)?;
    let mut f = Findings::default();
    match fmt.category() {
        Category::Image => image(path, fmt, &mut f)?,
        Category::Document if fmt == Format::Pdf => pdf(path, &mut f)?,
        Category::Audio | Category::Video => media_tags(path, caps, &mut f)?,
        _ => {}
    }
    let mut findings = f.0;
    // Only what Remove metadata can actually do here counts as removable.
    if !crate::catalog::step_accepts(&crate::ops::Step::StripMetadata, fmt, caps) {
        for x in &mut findings {
            x.removable = false;
        }
    }
    findings.sort_by_key(|x| x.kind);
    Ok(Report {
        format: fmt,
        findings,
    })
}

#[derive(Default)]
struct Findings(Vec<Finding>);

impl Findings {
    fn add(&mut self, kind: Kind, label: &str, detail: impl AsRef<str>, removable: bool) {
        let detail = tidy(detail.as_ref());
        if detail.is_empty() {
            return;
        }
        // The same fact often appears twice (EXIF and XMP): list it once.
        if self
            .0
            .iter()
            .any(|x| x.label == label && x.detail.eq_ignore_ascii_case(&detail))
        {
            return;
        }
        self.0.push(Finding {
            kind,
            label: label.into(),
            detail,
            removable,
            latitude: None,
            longitude: None,
        });
    }

    fn place(&mut self, lat: f64, lon: f64, removable: bool) {
        if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0 {
            return;
        }
        // (0, 0) is a placeholder some apps write, not a place.
        if lat == 0.0 && lon == 0.0 {
            return;
        }
        if self
            .0
            .iter()
            .any(|x| x.kind == Kind::Location && x.latitude.is_some())
        {
            return;
        }
        self.0.push(Finding {
            kind: Kind::Location,
            label: "Location".into(),
            detail: format_place(lat, lon),
            removable,
            latitude: Some((lat * 1e6).round() / 1e6),
            longitude: Some((lon * 1e6).round() / 1e6),
        });
    }
}

fn format_place(lat: f64, lon: f64) -> String {
    format!(
        "{:.5}° {}, {:.5}° {}",
        lat.abs(),
        if lat >= 0.0 { "N" } else { "S" },
        lon.abs(),
        if lon >= 0.0 { "E" } else { "W" }
    )
}

/// Printable, single-line and at most 160 characters.
fn tidy(s: &str) -> String {
    let one: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if one.chars().count() > 160 {
        one.chars().take(157).collect::<String>() + "…"
    } else {
        one
    }
}

fn read_start(path: &Path) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(READ_LIMIT)
        .read_to_end(&mut data)?;
    Ok(data)
}

// ---------------------------------------------------------------------------
// Images

fn image(path: &Path, fmt: Format, f: &mut Findings) -> Result<()> {
    let data = read_start(path)?;
    if let Ok(exif) = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(&data)) {
        exif_findings(&exif, f);
    }
    match fmt {
        Format::Jpeg => jpeg_extras(&data, f),
        Format::Png => png_extras(&data, f),
        Format::Gif => gif_extras(&data, f),
        _ => {}
    }
    if let Some(xmp) = find_xmp(&data) {
        xmp_findings(&xmp, true, f);
    }
    Ok(())
}

fn ascii(field: &exif::Field) -> Option<String> {
    match &field.value {
        exif::Value::Ascii(parts) => {
            let s = parts
                .iter()
                .map(|p| String::from_utf8_lossy(p).trim().to_string())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            (!s.is_empty()).then_some(s)
        }
        exif::Value::Undefined(bytes, _) => {
            // UserComment: an 8-byte character code, then the text.
            let text = if bytes.len() > 8 {
                &bytes[8..]
            } else {
                &bytes[..]
            };
            let s = String::from_utf8_lossy(text)
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_string();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    }
}

fn degrees(field: Option<&exif::Field>) -> Option<f64> {
    match &field?.value {
        exif::Value::Rational(v) if v.len() == 3 => {
            let parts: Vec<f64> = v.iter().map(|r| r.to_f64()).collect();
            if parts.iter().any(|p| !p.is_finite()) {
                return None;
            }
            Some(parts[0] + parts[1] / 60.0 + parts[2] / 3600.0)
        }
        _ => None,
    }
}

fn exif_findings(exif: &exif::Exif, f: &mut Findings) {
    use exif::{In, Tag};
    let get = |tag| exif.get_field(tag, In::PRIMARY).and_then(ascii);
    let lat = degrees(exif.get_field(Tag::GPSLatitude, In::PRIMARY));
    let lon = degrees(exif.get_field(Tag::GPSLongitude, In::PRIMARY));
    if let (Some(mut lat), Some(mut lon)) = (lat, lon) {
        if get(Tag::GPSLatitudeRef).is_some_and(|r| r.starts_with('S')) {
            lat = -lat;
        }
        if get(Tag::GPSLongitudeRef).is_some_and(|r| r.starts_with('W')) {
            lon = -lon;
        }
        f.place(lat, lon, true);
    }
    let make = get(Tag::Make).unwrap_or_default();
    let model = get(Tag::Model).unwrap_or_default();
    let camera = if model.to_lowercase().starts_with(&make.to_lowercase()) {
        model
    } else {
        format!("{make} {model}")
    };
    f.add(Kind::Device, "Camera", camera.trim(), true);
    let lens = [get(Tag::LensMake), get(Tag::LensModel)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    f.add(Kind::Device, "Lens", lens, true);
    for (tag, label) in [
        (Tag::BodySerialNumber, "Camera serial number"),
        (Tag::LensSerialNumber, "Lens serial number"),
        (Tag::CameraOwnerName, "Camera owner"),
        (Tag::Artist, "Author"),
        (Tag::Copyright, "Copyright"),
        (Tag::ImageUniqueID, "Unique photo ID"),
    ] {
        if let Some(v) = get(tag) {
            f.add(Kind::Identity, label, v, true);
        }
    }
    if let Some(taken) = get(Tag::DateTimeOriginal).or_else(|| get(Tag::DateTime)) {
        let zone = get(Tag::OffsetTimeOriginal).unwrap_or_default();
        f.add(
            Kind::Time,
            "Taken",
            format!("{} {zone}", exif_date(&taken)),
            true,
        );
    }
    if let Some(sw) = get(Tag::Software) {
        f.add(Kind::Software, "Software", sw, true);
    }
    for (tag, label) in [
        (Tag::ImageDescription, "Description"),
        (Tag::UserComment, "Comment"),
    ] {
        if let Some(v) = get(tag) {
            f.add(Kind::Other, label, v, true);
        }
    }
    // IFD1 is a preview only when it points at JPEG data; in a multi-page
    // TIFF it's the second page.
    if exif
        .get_field(Tag::JPEGInterchangeFormat, In::THUMBNAIL)
        .is_some()
    {
        f.add(
            Kind::Hidden,
            "Preview image",
            "A small copy of the picture, which can show it as it was before cropping or editing",
            true,
        );
    }
}

/// "2024:05:03 14:22:10" → "2024-05-03 14:22".
fn exif_date(s: &str) -> String {
    let b = s.as_bytes();
    let parts = (s.get(0..4), s.get(5..7), s.get(8..10), s.get(11..16));
    match parts {
        (Some(y), Some(m), Some(d), Some(t)) if b[4] == b':' && b[7] == b':' => {
            format!("{y}-{m}-{d} {t}")
        }
        _ => s.to_string(),
    }
}

fn jpeg_extras(data: &[u8], f: &mut Findings) {
    let mut multi_picture = false;
    let mut i = 2;
    while i + 4 <= data.len() && data[i] == 0xFF {
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if len < 2 || i + 2 + len > data.len() {
            return;
        }
        let body = &data[i + 4..i + 2 + len];
        match marker {
            0xFE => f.add(Kind::Other, "Comment", String::from_utf8_lossy(body), true),
            0xED if body.starts_with(b"Photoshop 3.0") => f.add(
                Kind::Other,
                "Captions and keywords",
                "IPTC data, often with names, places and copyright",
                true,
            ),
            0xE2 if body.starts_with(b"MPF\0") => {
                multi_picture = true;
                f.add(
                    Kind::Hidden,
                    "Extra pictures",
                    "More images stored after the main one (such as a depth map)",
                    true,
                )
            }
            0xDA => {
                // Entropy-coded data: find its end, then see what follows.
                let mut j = i + 2 + len;
                while j + 1 < data.len() {
                    if data[j] == 0xFF && data[j + 1] == 0xD9 {
                        let rest = data.len() - (j + 2);
                        // With MPF, the extra pictures are this data.
                        if rest > 32 && !multi_picture {
                            f.add(
                                Kind::Hidden,
                                "Data after the picture",
                                format!(
                                    "{} of extra data, such as a second image",
                                    crate::engines::image::human(rest as u64)
                                ),
                                true,
                            );
                        }
                        return;
                    }
                    j += 1;
                }
                return;
            }
            _ => {}
        }
        i += 2 + len;
    }
}

fn png_extras(data: &[u8], f: &mut Findings) {
    let mut i = 8;
    while i + 12 <= data.len() {
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let kind = &data[i + 4..i + 8];
        let Some(body) = data.get(i + 8..i + 8 + len) else {
            return;
        };
        match kind {
            b"tEXt" | b"zTXt" | b"iTXt" => {
                if let Some((key, text)) = png_text(kind, body) {
                    png_text_finding(&key, &text, f);
                }
            }
            b"tIME" if body.len() == 7 => {
                let year = u16::from_be_bytes([body[0], body[1]]);
                f.add(
                    Kind::Time,
                    "Last changed",
                    format!(
                        "{year}-{:02}-{:02} {:02}:{:02}",
                        body[2], body[3], body[4], body[5]
                    ),
                    true,
                );
            }
            b"IEND" => return,
            _ => {}
        }
        i += 12 + len;
    }
}

fn inflate(data: &[u8]) -> Option<String> {
    let mut out = String::new();
    flate2::read::ZlibDecoder::new(data)
        .take(1 << 20)
        .read_to_string(&mut out)
        .ok()?;
    Some(out)
}

fn png_text(kind: &[u8], body: &[u8]) -> Option<(String, String)> {
    let nul = body.iter().position(|&b| b == 0)?;
    let key = String::from_utf8_lossy(&body[..nul]).into_owned();
    let rest = &body[nul + 1..];
    let text = match kind {
        b"tEXt" => rest.iter().map(|&b| b as char).collect(),
        b"zTXt" => inflate(rest.get(1..)?)?,
        _ => {
            // iTXt: compression flag, method, language\0, translated key\0, text.
            let compressed = *rest.first()? == 1;
            let rest = rest.get(2..)?;
            let lang_end = rest.iter().position(|&b| b == 0)?;
            let rest = &rest[lang_end + 1..];
            let key_end = rest.iter().position(|&b| b == 0)?;
            let text = &rest[key_end + 1..];
            if compressed {
                inflate(text)?
            } else {
                String::from_utf8_lossy(text).into_owned()
            }
        }
    };
    Some((key, text))
}

fn png_text_finding(key: &str, text: &str, f: &mut Findings) {
    if key == "XML:com.adobe.xmp" {
        xmp_findings(text, true, f);
        return;
    }
    let (kind, label) = match key.to_ascii_lowercase().as_str() {
        "author" | "artist" => (Kind::Identity, "Author"),
        "copyright" => (Kind::Identity, "Copyright"),
        "software" => (Kind::Software, "Software"),
        "creation time" => (Kind::Time, "Created"),
        "source" => (Kind::Device, "Made with"),
        "comment" => (Kind::Other, "Comment"),
        "description" => (Kind::Other, "Description"),
        "title" => (Kind::Other, "Title"),
        "disclaimer" | "warning" => (Kind::Other, "Note"),
        // Raw profiles and app data: show that something is there.
        _ if key.starts_with("Raw profile type") => (Kind::Other, "Embedded profile data"),
        _ => (Kind::Other, "Text"),
    };
    let detail = if label == "Embedded profile data" {
        key.trim_start_matches("Raw profile type ").to_string()
    } else if label == "Text" {
        format!("{key}: {text}")
    } else {
        text.to_string()
    };
    f.add(kind, label, detail, true);
}

fn gif_extras(data: &[u8], f: &mut Findings) {
    // Header and logical screen descriptor, then the global colour table.
    if data.len() < 13 {
        return;
    }
    let mut i = 13;
    if data[10] & 0x80 != 0 {
        i += 3 << ((data[10] & 0x07) + 1);
    }
    let sub_blocks = |mut j: usize| -> (Vec<u8>, usize) {
        let mut out = Vec::new();
        while j < data.len() {
            let n = data[j] as usize;
            j += 1;
            if n == 0 {
                break;
            }
            out.extend_from_slice(data.get(j..j + n).unwrap_or(&[]));
            j += n;
        }
        (out, j)
    };
    while i < data.len() {
        match data[i] {
            0x21 if i + 1 < data.len() => {
                let label = data[i + 1];
                if label == 0xFE {
                    let (text, next) = sub_blocks(i + 2);
                    f.add(Kind::Other, "Comment", String::from_utf8_lossy(&text), true);
                    i = next;
                } else {
                    // Skip the extension's fixed block, then its data.
                    let fixed = data.get(i + 2).copied().unwrap_or(0) as usize;
                    let (_, next) = sub_blocks(i + 3 + fixed);
                    i = next;
                }
            }
            0x2C => {
                if i + 10 > data.len() {
                    return;
                }
                let mut j = i + 10;
                if data[i + 9] & 0x80 != 0 {
                    j += 3 << ((data[i + 9] & 0x07) + 1);
                }
                // LZW minimum code size, then the image data.
                let (_, next) = sub_blocks(j + 1);
                i = next;
            }
            _ => return,
        }
    }
}

// ---------------------------------------------------------------------------
// XMP (in images, PDFs and some videos)

fn find_xmp(data: &[u8]) -> Option<String> {
    let start = find(data, b"<x:xmpmeta")?;
    let end = find(&data[start..], b"</x:xmpmeta>")? + start + 12;
    Some(String::from_utf8_lossy(&data[start..end]).into_owned())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#xA;", " ")
        .replace("&amp;", "&")
}

/// A simple property, written as an attribute (`ns:Key="v"`) or an
/// element (`<ns:Key>v</ns:Key>`).
fn xmp_value(xmp: &str, key: &str) -> Option<String> {
    let attr = format!("{key}=\"");
    if let Some(at) = xmp.find(&attr) {
        let rest = &xmp[at + attr.len()..];
        let v = &rest[..rest.find('"')?];
        return Some(unescape(v));
    }
    let open = format!("<{key}>");
    let at = xmp.find(&open)?;
    let rest = &xmp[at + open.len()..];
    let v = &rest[..rest.find(&format!("</{key}>"))?];
    // Containers (rdf:Seq, rdf:Alt) hold list items.
    if v.contains("<rdf:li") {
        let items: Vec<String> = v
            .split("<rdf:li")
            .skip(1)
            .filter_map(|li| {
                let start = li.find('>')? + 1;
                let end = li.find("</rdf:li>")?;
                (start <= end).then(|| unescape(li[start..end].trim()))
            })
            .filter(|s| !s.is_empty())
            .collect();
        return (!items.is_empty()).then(|| items.join(", "));
    }
    Some(unescape(v.trim()))
}

/// "37,46.494N" (XMP's GPS form) → 37.7749.
fn xmp_coordinate(v: &str) -> Option<f64> {
    let v = v.trim();
    let hemi = v.chars().last()?;
    let body = &v[..v.len() - hemi.len_utf8()];
    let parts: Vec<f64> = body
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    let deg = match parts.as_slice() {
        [d, m] => d + m / 60.0,
        [d, m, s] => d + m / 60.0 + s / 3600.0,
        _ => return None,
    };
    Some(if matches!(hemi, 'S' | 'W') { -deg } else { deg })
}

fn xmp_findings(xmp: &str, removable: bool, f: &mut Findings) {
    if let (Some(lat), Some(lon)) = (
        xmp_value(xmp, "exif:GPSLatitude").and_then(|v| xmp_coordinate(&v)),
        xmp_value(xmp, "exif:GPSLongitude").and_then(|v| xmp_coordinate(&v)),
    ) {
        f.place(lat, lon, removable);
    }
    let place: Vec<String> = [
        "Iptc4xmpCore:Location",
        "photoshop:City",
        "photoshop:State",
        "photoshop:Country",
    ]
    .iter()
    .filter_map(|k| xmp_value(xmp, k))
    .filter(|v| !v.is_empty())
    .collect();
    if !place.is_empty() {
        f.add(Kind::Location, "Place name", place.join(", "), removable);
    }
    for (key, kind, label) in [
        ("dc:creator", Kind::Identity, "Author"),
        ("dc:rights", Kind::Identity, "Copyright"),
        ("xmpRights:Owner", Kind::Identity, "Owner"),
        ("photoshop:AuthorsPosition", Kind::Identity, "Author's role"),
        ("xmp:CreatorTool", Kind::Software, "Software"),
        ("pdf:Producer", Kind::Software, "Made by"),
        ("tiff:Make", Kind::Device, "Camera make"),
        ("tiff:Model", Kind::Device, "Camera"),
        ("exif:DateTimeOriginal", Kind::Time, "Taken"),
        ("xmp:CreateDate", Kind::Time, "Created"),
        ("xmp:ModifyDate", Kind::Time, "Changed"),
        ("dc:title", Kind::Other, "Title"),
        ("dc:description", Kind::Other, "Description"),
        ("exif:UserComment", Kind::Other, "Comment"),
    ] {
        if let Some(v) = xmp_value(xmp, key) {
            let v = if kind == Kind::Time { iso_date(&v) } else { v };
            f.add(kind, label, v, removable);
        }
    }
    if xmp.contains("xmpMM:History") || xmp.contains("photoshop:History") {
        f.add(
            Kind::Hidden,
            "Editing history",
            "A record of the edits made, and in which apps",
            removable,
        );
    }
    if xmp.contains("xmpMM:DerivedFrom") || xmp.contains("xmpMM:Ingredients") {
        f.add(
            Kind::Hidden,
            "Source files",
            "Names or IDs of the files this was made from",
            removable,
        );
    }
}

/// "2024-05-03T14:22:10+02:00" → "2024-05-03 14:22 +02:00".
fn iso_date(s: &str) -> String {
    let s = s.trim();
    match (s.get(..10), s.get(11..16), s.as_bytes().get(10)) {
        (Some(day), Some(time), Some(b'T')) => {
            let zone = s
                .get(19..)
                .map(|z| z.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit()))
                .unwrap_or("");
            format!("{day} {time} {zone}").trim().to_string()
        }
        _ => s.to_string(),
    }
}

// ---------------------------------------------------------------------------
// PDF

/// A PDF text string: UTF-16 with a byte-order mark, UTF-8 with one, or
/// PDFDocEncoding (close enough to Latin-1 for names and titles).
fn pdf_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_be_bytes(c))
            .collect();
        String::from_utf16_lossy(&units)
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&bytes[3..]).into_owned()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// "D:20240503142210+02'00'" → "2024-05-03 14:22".
fn pdf_date(s: &str) -> String {
    let d = s.trim_start_matches("D:");
    let b = d.as_bytes();
    if b.len() >= 12 && b[..12].iter().all(u8::is_ascii_digit) {
        format!(
            "{}-{}-{} {}:{}",
            &d[0..4],
            &d[4..6],
            &d[6..8],
            &d[8..10],
            &d[10..12]
        )
    } else if b.len() >= 8 && b[..8].iter().all(u8::is_ascii_digit) {
        format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8])
    } else {
        s.to_string()
    }
}

fn pdf(path: &Path, f: &mut Findings) -> Result<()> {
    use lopdf::Object;
    let raw = read_start(path)?;
    let doc = crate::engines::pdf::load(path).map_err(|e| match e {
        Error::Pdf(m) if m.contains("password") => Error::Invalid(
            "this PDF is password-protected; remove the password first to check it".into(),
        ),
        other => other,
    })?;
    let resolve = |o: &Object| -> Option<Object> {
        match o {
            Object::Reference(id) => doc.get_object(*id).ok().cloned(),
            other => Some(other.clone()),
        }
    };
    let dict_of = |o: Option<&Object>| -> Option<lopdf::Dictionary> {
        match resolve(o?)? {
            Object::Dictionary(d) => Some(d),
            _ => None,
        }
    };

    // Document information.
    if let Some(info) = dict_of(doc.trailer.get(b"Info").ok()) {
        for (key, kind, label) in [
            (b"Author".as_slice(), Kind::Identity, "Author"),
            (b"Creator", Kind::Software, "Made with"),
            (b"Producer", Kind::Software, "Saved by"),
            (b"CreationDate", Kind::Time, "Created"),
            (b"ModDate", Kind::Time, "Changed"),
            (b"Title", Kind::Other, "Title"),
            (b"Subject", Kind::Other, "Subject"),
            (b"Keywords", Kind::Other, "Keywords"),
        ] {
            if let Some(Object::String(bytes, _)) = info.get(key).ok().and_then(resolve) {
                let text = pdf_text(&bytes);
                let text = if kind == Kind::Time {
                    pdf_date(&text)
                } else {
                    text
                };
                f.add(kind, label, text, true);
            }
        }
    }
    let catalog = doc.catalog().ok().cloned().unwrap_or_default();
    if let Some(Object::Stream(s)) = catalog.get(b"Metadata").ok().and_then(resolve) {
        if let Ok(content) = s.get_plain_content_with_limit(8 << 20) {
            if let Some(xmp) = find_xmp(&content) {
                xmp_findings(&xmp, true, f);
            }
        }
    }
    if catalog.has(b"PieceInfo") {
        f.add(
            Kind::Hidden,
            "App data",
            "Private data from the app that made it, sometimes a whole editable copy",
            true,
        );
    }

    // Earlier versions kept by incremental saves: each save ends with its
    // own "startxref <offset>". A linearized ("fast web view") file's
    // first-page trailer says "startxref 0", which doesn't count.
    let earlier = startxref_offsets(&raw)
        .iter()
        .filter(|&&o| o > 0)
        .count()
        .saturating_sub(1);
    if earlier > 0 {
        f.add(
            Kind::Hidden,
            "Earlier versions",
            format!(
                "Saved over {earlier} time{}; deleted or replaced content may still be inside",
                if earlier == 1 { "" } else { "s" }
            ),
            true,
        );
    }

    // Comments and their authors, attachments.
    let mut authors: Vec<String> = Vec::new();
    let mut comments = 0;
    let mut attached = 0;
    for page in doc.get_pages().values() {
        let Ok(page) = doc.get_dictionary(*page) else {
            continue;
        };
        let annots = match page.get(b"Annots").ok().and_then(resolve) {
            Some(Object::Array(a)) => a,
            _ => continue,
        };
        for a in annots.iter().filter_map(|a| dict_of(Some(a))) {
            let subtype = a.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"");
            match subtype {
                b"Link" | b"Widget" | b"Popup" => {}
                b"FileAttachment" => attached += 1,
                _ => {
                    comments += 1;
                    if let Some(Object::String(t, _)) = a.get(b"T").ok().and_then(resolve) {
                        let name = tidy(&pdf_text(&t));
                        if !name.is_empty() && !authors.contains(&name) {
                            authors.push(name);
                        }
                    }
                }
            }
        }
    }
    if !authors.is_empty() {
        f.add(Kind::Identity, "Comment authors", authors.join(", "), true);
    }
    if comments > 0 {
        f.add(
            Kind::Other,
            "Comments",
            format!(
                "{comments} comment{} or markup{}",
                if comments == 1 { "" } else { "s" },
                if comments == 1 { "" } else { "s" }
            ),
            false,
        );
    }
    let names = dict_of(catalog.get(b"Names").ok());
    let embedded = names
        .as_ref()
        .and_then(|n| dict_of(n.get(b"EmbeddedFiles").ok()))
        .map(|tree| name_tree_count(&doc, &tree, &mut Walk::default()))
        .unwrap_or(0);
    if embedded + attached > 0 {
        let n = embedded + attached;
        f.add(
            Kind::Hidden,
            "Attached files",
            format!("{n} file{} inside the PDF", if n == 1 { "" } else { "s" }),
            false,
        );
    }
    let scripts = names.as_ref().is_some_and(|n| n.has(b"JavaScript"))
        || catalog.has(b"AA")
        || dict_of(catalog.get(b"OpenAction").ok())
            .is_some_and(|a| a.get(b"S").and_then(Object::as_name).ok() == Some(b"JavaScript"));
    if scripts {
        f.add(
            Kind::Hidden,
            "Scripts",
            "Runs code when opened or used",
            false,
        );
    }
    if let Some(form) = dict_of(catalog.get(b"AcroForm").ok()) {
        let filled = match form.get(b"Fields").ok().and_then(resolve) {
            Some(Object::Array(fields)) => fields
                .iter()
                .map(|field| filled_fields(&doc, field, &mut Walk::default()))
                .sum::<usize>(),
            _ => 0,
        };
        if filled > 0 {
            f.add(
                Kind::Other,
                "Filled-in form",
                format!(
                    "{filled} field{} with answers",
                    if filled == 1 { "" } else { "s" }
                ),
                false,
            );
        }
    }
    Ok(())
}

/// The "startxref" offsets in a PDF, in file order.
fn startxref_offsets(raw: &[u8]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(pos) = find(&raw[at..], b"startxref") {
        let mut i = at + pos + 9;
        while i < raw.len() && raw[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < raw.len() && raw[i].is_ascii_digit() {
            i += 1;
        }
        if let Some(n) = std::str::from_utf8(&raw[start..i])
            .ok()
            .and_then(|s| s.parse().ok())
        {
            out.push(n);
        }
        at = i.max(at + pos + 9);
    }
    out
}

/// Guards a walk through a PDF's object graph against loops and size:
/// each object is visited once, and only so many in all.
#[derive(Default)]
struct Walk {
    seen: std::collections::HashSet<lopdf::ObjectId>,
    steps: usize,
}

impl Walk {
    /// False when this object was seen before or the walk is long enough.
    fn enter(&mut self, id: Option<lopdf::ObjectId>, depth: usize) -> bool {
        self.steps += 1;
        if self.steps > 100_000 || depth > 32 {
            return false;
        }
        id.is_none_or(|id| self.seen.insert(id))
    }
}

fn name_tree_count(doc: &lopdf::Document, node: &lopdf::Dictionary, walk: &mut Walk) -> usize {
    name_tree_walk(doc, node, walk, 0)
}

fn name_tree_walk(
    doc: &lopdf::Document,
    node: &lopdf::Dictionary,
    walk: &mut Walk,
    depth: usize,
) -> usize {
    use lopdf::Object;
    let mut n = 0;
    if let Ok(Object::Array(names)) = node.get(b"Names") {
        n += names.len() / 2;
    }
    if let Ok(Object::Array(kids)) = node.get(b"Kids") {
        for kid in kids {
            if let Ok(id) = kid.as_reference() {
                if !walk.enter(Some(id), depth + 1) {
                    continue;
                }
                if let Ok(d) = doc.get_dictionary(id) {
                    n += name_tree_walk(doc, d, walk, depth + 1);
                }
            }
        }
    }
    n
}

fn filled_fields(doc: &lopdf::Document, field: &lopdf::Object, walk: &mut Walk) -> usize {
    filled_walk(doc, field, walk, 0)
}

fn filled_walk(
    doc: &lopdf::Document,
    field: &lopdf::Object,
    walk: &mut Walk,
    depth: usize,
) -> usize {
    use lopdf::Object;
    if !walk.enter(field.as_reference().ok(), depth) {
        return 0;
    }
    let dict = match field {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        Object::Dictionary(d) => Some(d),
        _ => None,
    };
    let Some(dict) = dict else {
        return 0;
    };
    let own = match dict.get(b"V") {
        Ok(Object::String(s, _)) if !s.is_empty() => 1,
        Ok(Object::Name(n)) if n != b"Off" => 1,
        Ok(Object::Array(a)) if !a.is_empty() => 1,
        _ => 0,
    };
    let kids = match dict.get(b"Kids") {
        Ok(Object::Array(kids)) => kids
            .iter()
            .map(|k| filled_walk(doc, k, walk, depth + 1))
            .sum(),
        _ => 0,
    };
    own + kids
}

// ---------------------------------------------------------------------------
// Audio and video

/// ISO 6709, as phones write it: "+37.7749-122.4194+010.000/".
fn iso6709(s: &str) -> Option<(f64, f64)> {
    let s = s.trim().trim_end_matches('/');
    let mut cuts: Vec<usize> = s
        .char_indices()
        .filter(|&(i, c)| i > 0 && (c == '+' || c == '-'))
        .map(|(i, _)| i)
        .collect();
    cuts.push(s.len());
    let lat: f64 = s.get(..cuts[0])?.parse().ok()?;
    let lon: f64 = s.get(cuts[0]..*cuts.get(1)?)?.parse().ok()?;
    Some((lat, lon))
}

fn media_tags(path: &Path, caps: &Capabilities, f: &mut Findings) -> Result<()> {
    let tags = media::probe_tags(caps, path).ok_or_else(|| {
        if caps.ffprobe.is_none() {
            Error::Missing("checking audio and video needs FFprobe".into())
        } else {
            Error::UnsupportedInput("FFprobe couldn't read this file".into())
        }
    })?;
    let mut make = String::new();
    let mut model = String::new();
    for (key, value) in &tags {
        let k = key.to_ascii_lowercase();
        let k = k.as_str();
        match k {
            "location" | "location-eng" | "com.apple.quicktime.location.iso6709" => {
                if let Some((lat, lon)) = iso6709(value) {
                    f.place(lat, lon, true);
                }
            }
            "com.apple.quicktime.make" | "make" | "com.android.manufacturer" => {
                make = value.clone()
            }
            "com.apple.quicktime.model" | "model" | "com.android.model" => model = value.clone(),
            "com.apple.quicktime.software" | "software" | "com.android.version" => {
                f.add(Kind::Software, "Software", value, true)
            }
            "creation_time" | "com.apple.quicktime.creationdate" | "date" => {
                f.add(Kind::Time, "Created", iso_date(value), true)
            }
            "artist" | "author" | "com.apple.quicktime.author" | "composer" | "album_artist" => {
                f.add(Kind::Identity, "Names", value, true)
            }
            "copyright" | "com.apple.quicktime.copyright" => {
                f.add(Kind::Identity, "Copyright", value, true)
            }
            "comment" | "description" | "com.apple.quicktime.description" => {
                f.add(Kind::Other, "Comment", value, true)
            }
            "title" | "com.apple.quicktime.title" => f.add(Kind::Other, "Title", value, true),
            "com.apple.quicktime.location.name" => f.add(Kind::Location, "Place name", value, true),
            _ => {}
        }
    }
    let device = if model.to_lowercase().starts_with(&make.to_lowercase()) {
        model
    } else {
        format!("{make} {model}")
    };
    f.add(Kind::Device, "Device", device.trim(), true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(r: &Report) -> Vec<(Kind, String)> {
        r.findings
            .iter()
            .map(|x| (x.kind, x.label.clone()))
            .collect()
    }

    /// A JPEG with EXIF (GPS, camera, serial, date, software, a
    /// thumbnail), a comment, and data after the picture.
    fn revealing_jpeg(dir: &Path) -> std::path::PathBuf {
        use exif::experimental::Writer;
        use exif::{Field, In, Rational, Tag, Value};
        let dms = |d: u32, m: u32, s: u32| {
            Value::Rational(vec![
                Rational { num: d, denom: 1 },
                Rational { num: m, denom: 1 },
                Rational { num: s, denom: 100 },
            ])
        };
        let ascii = |s: &str| Value::Ascii(vec![s.as_bytes().to_vec()]);
        let fields = [
            Field {
                tag: Tag::GPSLatitudeRef,
                ifd_num: In::PRIMARY,
                value: ascii("N"),
            },
            Field {
                tag: Tag::GPSLatitude,
                ifd_num: In::PRIMARY,
                value: dms(37, 46, 2964),
            },
            Field {
                tag: Tag::GPSLongitudeRef,
                ifd_num: In::PRIMARY,
                value: ascii("W"),
            },
            Field {
                tag: Tag::GPSLongitude,
                ifd_num: In::PRIMARY,
                value: dms(122, 25, 984),
            },
            Field {
                tag: Tag::Make,
                ifd_num: In::PRIMARY,
                value: ascii("Apple"),
            },
            Field {
                tag: Tag::Model,
                ifd_num: In::PRIMARY,
                value: ascii("iPhone 17 Pro"),
            },
            Field {
                tag: Tag::BodySerialNumber,
                ifd_num: In::PRIMARY,
                value: ascii("F2LXK0001"),
            },
            Field {
                tag: Tag::DateTimeOriginal,
                ifd_num: In::PRIMARY,
                value: ascii("2026:05:03 14:22:10"),
            },
            Field {
                tag: Tag::Software,
                ifd_num: In::PRIMARY,
                value: ascii("26.0"),
            },
        ];
        let mut thumb = Vec::new();
        image::RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0]))
            .write_to(
                &mut std::io::Cursor::new(&mut thumb),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let mut w = Writer::new();
        for f in &fields {
            w.push_field(f);
        }
        w.set_jpeg(&thumb, In::THUMBNAIL);
        let mut tiff = std::io::Cursor::new(Vec::new());
        w.write(&mut tiff, false).unwrap();
        let tiff = tiff.into_inner();

        let mut plain = Vec::new();
        image::RgbImage::from_pixel(16, 16, image::Rgb([200, 30, 30]))
            .write_to(
                &mut std::io::Cursor::new(&mut plain),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let mut out = vec![0xFF, 0xD8];
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        let comment = b"for Sam's eyes only";
        out.extend_from_slice(&[0xFF, 0xFE]);
        out.extend_from_slice(&((comment.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(comment);
        out.extend_from_slice(&plain[2..]);
        out.extend_from_slice(&[0u8; 300]);
        let path = dir.join("photo.jpg");
        std::fs::write(&path, out).unwrap();
        path
    }

    #[test]
    fn photos_reveal_place_device_and_more() {
        let dir = tempfile::tempdir().unwrap();
        let path = revealing_jpeg(dir.path());
        let r = check(&path, &Capabilities::none()).unwrap();
        let place = &r.findings[0];
        assert_eq!(place.kind, Kind::Location);
        assert!(
            (place.latitude.unwrap() - 37.774_9).abs() < 1e-3,
            "{place:?}"
        );
        assert!(
            (place.longitude.unwrap() + 122.419_4).abs() < 1e-3,
            "{place:?}"
        );
        let labels = kinds(&r);
        for want in [
            (Kind::Identity, "Camera serial number"),
            (Kind::Device, "Camera"),
            (Kind::Time, "Taken"),
            (Kind::Software, "Software"),
            (Kind::Hidden, "Preview image"),
            (Kind::Hidden, "Data after the picture"),
            (Kind::Other, "Comment"),
        ] {
            assert!(
                labels.contains(&(want.0, want.1.to_string())),
                "missing {want:?} in {labels:?}"
            );
        }
        let camera = r.findings.iter().find(|x| x.label == "Camera").unwrap();
        assert_eq!(camera.detail, "Apple iPhone 17 Pro");
        assert!(r.findings.iter().all(|x| x.removable));

        // After Remove metadata, nothing is left to report.
        let clean = dir.path().join("clean.jpg");
        let data = std::fs::read(&path).unwrap();
        std::fs::write(&clean, crate::engines::meta::strip_jpeg(&data, 1).unwrap()).unwrap();
        assert_eq!(
            check(&clean, &Capabilities::none()).unwrap().findings,
            vec![]
        );
    }

    #[test]
    fn png_text_and_xmp() {
        let dir = tempfile::tempdir().unwrap();
        let mut png = Vec::new();
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let chunk = |kind: &[u8], body: &[u8]| {
            let mut c = (body.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(body);
            c.extend_from_slice(&[0, 0, 0, 0]);
            c
        };
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF><rdf:Description xmp:CreatorTool="Pixelmator Pro 3.6" exif:GPSLatitude="51,30.444N" exif:GPSLongitude="0,7.668W"><dc:creator><rdf:Seq><rdf:li>Ada &amp; Co</rdf:li></rdf:Seq></dc:creator><xmpMM:History/></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        itxt.extend_from_slice(xmp.as_bytes());
        let insert = png.len() - 12;
        let mut extra = chunk(b"tEXt", b"Author\0Jo Bloggs");
        extra.extend(chunk(b"iTXt", &itxt));
        extra.extend(chunk(b"tIME", &[0x07, 0xEA, 5, 3, 14, 22, 10]));
        png.splice(insert..insert, extra);
        let path = dir.path().join("shot.png");
        std::fs::write(&path, png).unwrap();
        let r = check(&path, &Capabilities::none()).unwrap();
        let get = |label: &str| {
            r.findings
                .iter()
                .find(|x| x.label == label)
                .map(|x| x.detail.clone())
        };
        let authors: Vec<&str> = r
            .findings
            .iter()
            .filter(|x| x.label == "Author")
            .map(|x| x.detail.as_str())
            .collect();
        assert_eq!(authors, ["Jo Bloggs", "Ada & Co"], "{:?}", r.findings);
        assert_eq!(get("Software").as_deref(), Some("Pixelmator Pro 3.6"));
        assert_eq!(get("Last changed").as_deref(), Some("2026-05-03 14:22"));
        assert!(get("Editing history").is_some());
        let place = r.findings.iter().find(|x| x.latitude.is_some()).unwrap();
        assert!((place.latitude.unwrap() - 51.5074).abs() < 1e-3);
        assert!((place.longitude.unwrap() + 0.1278).abs() < 1e-3);
    }

    #[test]
    fn videos_reveal_where_and_on_what() {
        let caps = Capabilities::detect();
        let (Some(ffmpeg), Some(_)) = (caps.ffmpeg.as_ref(), caps.ffprobe.as_ref()) else {
            eprintln!("skipping: needs ffmpeg and ffprobe");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("clip.mov");
        let ok = std::process::Command::new(ffmpeg)
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x48:rate=5",
                "-t",
                "1",
            ])
            .args(["-metadata", "location=+48.8584+002.2945/"])
            .args(["-metadata", "com.apple.quicktime.make=Apple"])
            .args(["-metadata", "com.apple.quicktime.model=iPhone 17"])
            .args(["-metadata", "creation_time=2026-05-03T14:22:10Z"])
            .args([
                "-movflags",
                "use_metadata_tags",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let r = check(&clip, &caps).unwrap();
        let place = r
            .findings
            .iter()
            .find(|x| x.latitude.is_some())
            .expect("a location");
        assert!((place.latitude.unwrap() - 48.8584).abs() < 1e-4);
        assert!((place.longitude.unwrap() - 2.2945).abs() < 1e-4);
        assert!(
            r.findings
                .iter()
                .any(|x| x.label == "Device" && x.detail == "Apple iPhone 17"),
            "{:?}",
            r.findings
        );
        assert!(r.findings.iter().any(|x| x.kind == Kind::Time));
    }

    #[test]
    fn helpers_read_dates_and_places() {
        assert_eq!(pdf_date("D:20260503142210+02'00'"), "2026-05-03 14:22");
        assert_eq!(
            iso_date("2026-05-03T14:22:10.000+02:00"),
            "2026-05-03 14:22 +02:00"
        );
        assert_eq!(
            iso6709("+37.7749-122.4194+010.000/"),
            Some((37.7749, -122.4194))
        );
        assert_eq!(iso6709("-33.8688+151.2093/"), Some((-33.8688, 151.2093)));
        assert_eq!(xmp_coordinate("37,46.494N"), Some(37.0 + 46.494 / 60.0));
        assert_eq!(pdf_text(&[0xFE, 0xFF, 0, b'A', 0, b'l']), "Al");
        assert_eq!(tidy("a\n\tb   c"), "a b c");
        // Damaged dates come back as they are, never a crash.
        assert_eq!(
            exif_date("2024:05:0\u{FFFD} 14:22:10"),
            "2024:05:0\u{FFFD} 14:22:10"
        );
        assert_eq!(iso_date("2024-05-03T14:2é0"), "2024-05-03T14:2é0");
        assert_eq!(exif_date("2024:05:03"), "2024:05:03");
        // Linearized first-page trailers say "startxref 0".
        assert_eq!(
            startxref_offsets(b"%PDF\nstartxref\n0\n%%EOF\n...startxref\r\n1234\n%%EOF"),
            [0, 1234]
        );
    }

    #[test]
    fn looping_pdfs_finish() {
        use lopdf::{dictionary, Document, Object, Stream};
        let dir = tempfile::tempdir().unwrap();
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let content = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
            }),
        );
        // A name tree and a form field that list themselves as children.
        let tree = doc.new_object_id();
        doc.objects.insert(
            tree,
            Object::Dictionary(dictionary! {
                "Kids" => vec![tree.into(); 8],
            }),
        );
        let field = doc.new_object_id();
        doc.objects.insert(
            field,
            Object::Dictionary(dictionary! {
                "V" => Object::string_literal("x"), "Kids" => vec![field.into(); 8],
            }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages_id,
            "Names" => dictionary! { "EmbeddedFiles" => tree },
            "AcroForm" => dictionary! { "Fields" => vec![field.into()] },
        });
        doc.trailer.set("Root", catalog);
        let path = dir.path().join("loop.pdf");
        doc.save(&path).unwrap();
        let started = std::time::Instant::now();
        let r = check(&path, &Capabilities::none()).unwrap();
        assert!(started.elapsed().as_secs() < 5);
        assert!(r.findings.iter().any(|x| x.label == "Filled-in form"));
    }

    #[test]
    fn pdfs_reveal_authors_versions_and_attachments() {
        use lopdf::{dictionary, Document, Object, Stream};
        let dir = tempfile::tempdir().unwrap();
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let content = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let note = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text",
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "T" => Object::string_literal("Riley Chen"),
            "Contents" => Object::string_literal("Cut this paragraph"),
        });
        // A second comment written inline in the page's list.
        let inline = Object::Dictionary(dictionary! {
            "Type" => "Annot", "Subtype" => "Highlight",
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "T" => Object::string_literal("Sam Lee"),
        });
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Annots" => vec![note.into(), inline],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
            }),
        );
        let file = doc.add_object(
            dictionary! { "Type" => "Filespec", "F" => Object::string_literal("budget.xlsx") },
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => pages_id,
            "Names" => dictionary! {
                "EmbeddedFiles" => dictionary! { "Names" => vec![Object::string_literal("budget.xlsx"), file.into()] },
                "JavaScript" => dictionary! {},
            },
        });
        doc.trailer.set("Root", catalog);
        let info = doc.add_object(dictionary! {
            "Author" => Object::string_literal("Riley Chen"),
            "Producer" => Object::string_literal("Quartz PDFContext"),
            "CreationDate" => Object::string_literal("D:20260503142210Z"),
        });
        doc.trailer.set("Info", info);
        let path = dir.path().join("memo.pdf");
        doc.save(&path).unwrap();
        // An incremental update: the title changed, the old one kept.
        let bytes = std::fs::read(&path).unwrap();
        let prev = Document::load_mem(&bytes).unwrap();
        let mut inc = lopdf::IncrementalDocument::create_from(bytes, prev);
        inc.opt_clone_object_to_new_document(info).unwrap();
        inc.new_document
            .get_dictionary_mut(info)
            .unwrap()
            .set("Title", Object::string_literal("Final memo"));
        inc.save(&path).unwrap();

        let r = check(&path, &Capabilities::none()).unwrap();
        let get = |label: &str| r.findings.iter().find(|x| x.label == label).cloned();
        assert_eq!(get("Author").unwrap().detail, "Riley Chen");
        assert_eq!(
            get("Comment authors").unwrap().detail,
            "Riley Chen, Sam Lee"
        );
        assert_eq!(get("Created").unwrap().detail, "2026-05-03 14:22");
        assert_eq!(get("Saved by").unwrap().detail, "Quartz PDFContext");
        assert!(get("Earlier versions").unwrap().removable);
        let attached = get("Attached files").unwrap();
        assert_eq!(attached.detail, "1 file inside the PDF");
        assert!(!attached.removable);
        assert!(get("Scripts").is_some());

        // Remove metadata takes out everything it says it does.
        let clean = dir.path().join("clean.pdf");
        crate::engines::pdf::strip_metadata(&path, &clean).unwrap();
        let after = check(&clean, &Capabilities::none()).unwrap();
        assert!(
            after.findings.iter().all(|x| !x.removable),
            "{:?}",
            after.findings
        );
        assert!(after.findings.iter().any(|x| x.label == "Attached files"));
    }
}
