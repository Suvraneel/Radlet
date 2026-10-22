//! PDF creation, merging, text extraction and metadata removal.

use std::io::Write;
use std::path::Path;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use image::{ColorType, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

use crate::catalog::Capabilities;
use crate::engines::image::{self as img_engine, Picture};
use crate::error::{Error, Result};
use crate::format::Format;

/// Pages inherit these from their parents in the page tree; we copy them
/// onto each page before re-parenting it.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Builds one PDF from images and existing PDFs, in order.
pub struct PdfBuilder {
    doc: Document,
    pages_id: ObjectId,
    kids: Vec<Object>,
}

impl Default for PdfBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfBuilder {
    pub fn new() -> Self {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        PdfBuilder {
            doc,
            pages_id,
            kids: Vec::new(),
        }
    }

    pub fn page_count(&self) -> usize {
        self.kids.len()
    }

    /// Adds any supported input: a PDF's pages, or an image as one page.
    pub fn add_file(&mut self, path: &Path, format: Format, caps: &Capabilities) -> Result<()> {
        match format {
            Format::Pdf => self.add_pdf(path),
            Format::Jpeg => self.add_jpeg(path, caps),
            f if caps.can_decode_raster(f) => {
                let pic = img_engine::decode(path, f, caps)?;
                self.add_picture(&pic)
            }
            f => Err(Error::UnsupportedInput(format!(
                "{f} files can't go into a PDF"
            ))),
        }
    }

    /// Embeds a JPEG's original bytes (no recompression) when it is upright
    /// RGB or greyscale; otherwise decodes and re-encodes losslessly.
    fn add_jpeg(&mut self, path: &Path, caps: &Capabilities) -> Result<()> {
        let mut reader = ImageReader::open(path)?;
        reader.set_format(ImageFormat::Jpeg);
        reader.limits(img_engine::limits());
        let mut decoder = reader.into_decoder()?;
        let (w, h) = decoder.dimensions();
        let color = decoder.color_type();
        let upright = decoder
            .orientation()
            .map(|o| o.to_exif() == 1)
            .unwrap_or(true);
        let icc = decoder.icc_profile().ok().flatten();
        let colorspace = match color {
            ColorType::Rgb8 => Some(("DeviceRGB", 3)),
            ColorType::L8 => Some(("DeviceGray", 1)),
            _ => None,
        };
        let bytes = std::fs::read(path)?;
        // The decoder reports CMYK JPEGs as RGB after converting them, so
        // check the real component count before passing bytes through.
        let components_match =
            colorspace.is_some_and(|(_, channels)| jpeg_components(&bytes) == Some(channels as u8));
        match colorspace {
            Some((space, channels)) if upright && components_match => {
                let cs = self.colorspace(space, channels, icc.as_deref());
                let dict = dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => w as i64,
                    "Height" => h as i64,
                    "ColorSpace" => cs,
                    "BitsPerComponent" => 8,
                    "Filter" => "DCTDecode",
                };
                let image_id = self
                    .doc
                    .add_object(Stream::new(dict, bytes).with_compression(false));
                self.add_image_page(image_id, w, h);
                Ok(())
            }
            _ => {
                let pic = img_engine::decode(path, Format::Jpeg, caps)?;
                self.add_picture(&pic)
            }
        }
    }

    /// Adds decoded pixels as a page, with transparency as a soft mask.
    pub fn add_picture(&mut self, pic: &Picture) -> Result<()> {
        let img = &pic.img;
        let (w, h) = (img.width(), img.height());
        let gray = !img.color().has_color();
        let (pixels, space, channels) = if gray {
            (img.to_luma8().into_raw(), "DeviceGray", 1)
        } else {
            (img.to_rgb8().into_raw(), "DeviceRGB", 3)
        };
        let cs = self.colorspace(space, channels, pic.icc.as_deref());
        let mut dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "ColorSpace" => cs,
            "BitsPerComponent" => 8,
            "Filter" => "FlateDecode",
        };
        if img.color().has_alpha() {
            let alpha: Vec<u8> = match img {
                DynamicImage::ImageLumaA8(b) => b.pixels().map(|p| p.0[1]).collect(),
                _ => img.to_rgba8().pixels().map(|p| p.0[3]).collect(),
            };
            if alpha.iter().any(|&a| a != 255) {
                let mask = dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => w as i64,
                    "Height" => h as i64,
                    "ColorSpace" => "DeviceGray",
                    "BitsPerComponent" => 8,
                    "Filter" => "FlateDecode",
                };
                let mask_id = self
                    .doc
                    .add_object(Stream::new(mask, deflate(&alpha)?).with_compression(false));
                dict.set("SMask", mask_id);
            }
        }
        let image_id = self
            .doc
            .add_object(Stream::new(dict, deflate(&pixels)?).with_compression(false));
        self.add_image_page(image_id, w, h);
        Ok(())
    }

    /// Uses the image's own colour profile when it matches its channels.
    fn colorspace(&mut self, device: &str, channels: i64, icc: Option<&[u8]>) -> Object {
        let matches = icc.is_some_and(|p| {
            p.len() > 20
                && match channels {
                    3 => &p[16..20] == b"RGB ",
                    1 => &p[16..20] == b"GRAY",
                    _ => false,
                }
        });
        match icc {
            Some(profile) if matches => {
                let compressed = deflate(profile).unwrap_or_else(|_| profile.to_vec());
                let dict = dictionary! { "N" => channels, "Alternate" => device, "Filter" => "FlateDecode" };
                let id = self
                    .doc
                    .add_object(Stream::new(dict, compressed).with_compression(false));
                Object::Array(vec![
                    Object::Name(b"ICCBased".to_vec()),
                    Object::Reference(id),
                ])
            }
            _ => Object::Name(device.as_bytes().to_vec()),
        }
    }

    /// One page per image. Large images are scaled so the longest side fits
    /// an A4 page length (11.69 in); small images keep 72 dpi.
    fn add_image_page(&mut self, image_id: ObjectId, w: u32, h: u32) {
        let longest = w.max(h) as f64;
        let dpi = (longest / 11.69).max(72.0);
        let pw = (w as f64 * 72.0 / dpi).max(1.0);
        let ph = (h as f64 * 72.0 / dpi).max(1.0);
        let content = format!("q {pw:.3} 0 0 {ph:.3} 0 0 cm /Im0 Do Q");
        let content_id = self
            .doc
            .add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let page = dictionary! {
            "Type" => "Page",
            "Parent" => self.pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw as f32), Object::Real(ph as f32)],
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
        };
        let page_id = self.doc.add_object(page);
        self.kids.push(page_id.into());
    }

    /// Appends every page of an existing PDF.
    pub fn add_pdf(&mut self, path: &Path) -> Result<()> {
        self.import_pages(load(path)?, None)
    }

    /// Appends `pages` (numbered from 1, in the order given) of an already
    /// loaded PDF, or all of them. Pages left out are not copied.
    pub fn import_pages(&mut self, mut src: Document, pages: Option<&[u32]>) -> Result<()> {
        src.renumber_objects_with(self.doc.max_id + 1);
        let all = src.get_pages();
        if all.is_empty() {
            return Err(Error::Pdf("this PDF has no pages".into()));
        }
        let chosen: Vec<ObjectId> = match pages {
            None => all.values().copied().collect(),
            Some(numbers) => {
                validate_pages(numbers, all.len() as u32)?;
                numbers.iter().map(|n| all[n]).collect()
            }
        };
        let dropped: Vec<ObjectId> = all
            .values()
            .copied()
            .filter(|id| !chosen.contains(id))
            .collect();
        for &page_id in &chosen {
            let mut dict = src.get_dictionary(page_id)?.clone();
            for key in INHERITED {
                if !dict.has(key) {
                    if let Some(v) = inherited(&src, page_id, key) {
                        dict.set(key, v);
                    }
                }
            }
            dict.set("Parent", self.pages_id);
            src.objects.insert(page_id, Object::Dictionary(dict));
            self.kids.push(page_id.into());
        }
        let skip: Vec<ObjectId> = [b"Root".as_slice(), b"Info".as_slice()]
            .iter()
            .filter_map(|k| src.trailer.get(k).and_then(Object::as_reference).ok())
            .chain(dropped)
            .collect();
        for (id, obj) in std::mem::take(&mut src.objects) {
            if skip.contains(&id) || is_structural(&obj) {
                continue;
            }
            self.doc.objects.insert(id, obj);
        }
        self.doc.max_id = self.doc.max_id.max(src.max_id);
        Ok(())
    }

    /// Page object ids, in order.
    pub(crate) fn page_ids(&self) -> Vec<ObjectId> {
        self.kids
            .iter()
            .filter_map(|k| k.as_reference().ok())
            .collect()
    }

    pub(crate) fn doc_mut(&mut self) -> &mut Document {
        &mut self.doc
    }

    /// Writes the PDF to `out`.
    pub fn finish(mut self, out: &Path) -> Result<()> {
        if self.kids.is_empty() {
            return Err(Error::NothingToDo("nothing to put in the PDF".into()));
        }
        let count = self.kids.len() as i64;
        let pages = dictionary! { "Type" => "Pages", "Kids" => self.kids, "Count" => count };
        self.doc
            .objects
            .insert(self.pages_id, Object::Dictionary(pages));
        let catalog = self
            .doc
            .add_object(dictionary! { "Type" => "Catalog", "Pages" => self.pages_id });
        self.doc.trailer.set("Root", catalog);
        self.doc.prune_objects();
        // Compresses only streams we created without a filter (page contents).
        self.doc.compress();
        let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
        self.doc.save_to(&mut file)?;
        file.flush()?;
        Ok(())
    }
}

/// Number of colour components in a JPEG's frame header (1 grey, 3 colour,
/// 4 CMYK/YCCK), or None if no frame header is found.
fn jpeg_components(data: &[u8]) -> Option<u8> {
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let is_sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            return data.get(i + 9).copied();
        }
        if marker == 0xDA || len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

fn is_structural(obj: &Object) -> bool {
    let ty = match obj {
        Object::Dictionary(d) => d.get_type().ok(),
        Object::Stream(s) => s.dict.get_type().ok(),
        _ => None,
    };
    matches!(ty, Some(b"Pages" | b"Catalog" | b"XRef" | b"ObjStm"))
}

/// Walks up the page tree for an inherited attribute.
pub(crate) fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = doc
        .get_dictionary(page)
        .ok()?
        .get(b"Parent")
        .and_then(Object::as_reference)
        .ok();
    for _ in 0..64 {
        let id = current?;
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(v) = dict.get(key) {
            return Some(v.clone());
        }
        current = dict.get(b"Parent").and_then(Object::as_reference).ok();
    }
    None
}

pub(crate) fn load(path: &Path) -> Result<Document> {
    // Bounded so a small, hostile PDF can't inflate into gigabytes.
    let options = lopdf::LoadOptions::with_max_decompressed_size(512 << 20);
    let doc = Document::load_with_options(path, options)
        .map_err(|e| Error::Pdf(format!("couldn't read the PDF ({e})")))?;
    if doc.is_encrypted() {
        return Err(Error::Pdf("this PDF is password-protected".into()));
    }
    Ok(doc)
}

fn deflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(data.len() / 2), Compression::default());
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

/// Extracts selectable text, page by page.
pub fn to_text(path: &Path) -> Result<String> {
    let doc = load(path)?;
    let mut out = String::new();
    for page in doc.get_pages().keys() {
        if let Ok(text) = doc.extract_text_with_limit(&[*page], 256 << 20) {
            out.push_str(text.trim_end());
            out.push_str("\n\n");
        }
    }
    if out.trim().is_empty() {
        return Err(Error::NothingToDo(
            "this PDF has no selectable text. It may be a scan: the Veer app on macOS can recognise it".into(),
        ));
    }
    Ok(out.trim_end().to_string() + "\n")
}

/// Removes the document information dictionary, XMP metadata, apps'
/// private data and the names of comment authors. Saving rewrites the
/// file, which also drops earlier versions kept by incremental saves.
pub fn strip_metadata(path: &Path, out: &Path) -> Result<()> {
    let mut doc = load(path)?;
    doc.trailer.remove(b"Info");
    if let Ok(catalog) = doc.catalog_mut() {
        catalog.remove(b"Metadata");
        catalog.remove(b"PieceInfo");
    }
    // On form fields T is the field's name, which they need; on comments
    // and markup it's the author.
    fn forget_author(a: &mut Dictionary) {
        let subtype = a.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"");
        if !matches!(subtype, b"Widget" | b"Link") {
            a.remove(b"T");
        }
    }
    let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let mut annots: Vec<ObjectId> = Vec::new();
    for id in page_ids {
        // The annotation list may be stored by reference.
        let list = match doc.get_dictionary(id).and_then(|p| p.get(b"Annots")) {
            Ok(Object::Reference(r)) => Some(*r),
            _ => None,
        };
        if let Ok(page) = doc.get_dictionary_mut(id) {
            page.remove(b"Metadata");
            page.remove(b"PieceInfo");
            if let Ok(Object::Array(a)) = page.get_mut(b"Annots") {
                for item in a.iter_mut() {
                    match item {
                        Object::Reference(r) => annots.push(*r),
                        Object::Dictionary(d) => forget_author(d),
                        _ => {}
                    }
                }
            }
        }
        if let Some(r) = list {
            if let Ok(Object::Array(a)) = doc.get_object_mut(r) {
                for item in a.iter_mut() {
                    match item {
                        Object::Reference(r) => annots.push(*r),
                        Object::Dictionary(d) => forget_author(d),
                        _ => {}
                    }
                }
            }
        }
    }
    for id in annots {
        if let Ok(a) = doc.get_dictionary_mut(id) {
            forget_author(a);
        }
    }
    doc.prune_objects();
    let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
    doc.save_to(&mut file)?;
    file.flush()?;
    Ok(())
}

/// Checks a page list: non-empty, every page exists, no page twice.
fn validate_pages(numbers: &[u32], count: u32) -> Result<()> {
    if numbers.is_empty() {
        return Err(Error::Invalid("choose at least one page".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for &n in numbers {
        if n == 0 || n > count {
            return Err(Error::Invalid(format!(
                "page {n} doesn't exist (the PDF has {count})"
            )));
        }
        if !seen.insert(n) {
            return Err(Error::Invalid(format!("page {n} is listed twice")));
        }
    }
    Ok(())
}

/// Builds a PDF of `pages` from `path`, turning those in `rotate`
/// (reorder, delete and rotate in one go).
pub fn select_pages(
    path: &Path,
    pages: &[u32],
    rotate: &[crate::ops::PageTurn],
    out: &Path,
) -> Result<()> {
    let mut b = PdfBuilder::new();
    b.import_pages(load(path)?, Some(pages))?;
    let ids = b.page_ids();
    for turn in rotate {
        let at = pages
            .iter()
            .position(|&p| p == turn.page)
            .ok_or_else(|| Error::Invalid(format!("page {} is rotated but not kept", turn.page)))?;
        crate::engines::pdfedit::turn_page(b.doc_mut(), ids[at], turn.degrees)?;
    }
    b.finish(out)
}

/// Turns a split request into page groups (numbered from 1).
pub fn split_groups(mode: &crate::ops::SplitMode, count: u32) -> Result<Vec<Vec<u32>>> {
    use crate::ops::SplitMode;
    if count == 0 {
        return Err(Error::Pdf("this PDF has no pages".into()));
    }
    let groups = match mode {
        SplitMode::EveryPage => (1..=count).map(|p| vec![p]).collect(),
        SplitMode::Chunks { size } => {
            if *size == 0 {
                return Err(Error::Invalid("pages per part must be at least 1".into()));
            }
            (1..=count)
                .collect::<Vec<u32>>()
                .chunks(*size as usize)
                .map(<[u32]>::to_vec)
                .collect()
        }
        SplitMode::Ranges { ranges } => parse_ranges(ranges, count)?,
    };
    if groups.len() < 2 && !matches!(mode, SplitMode::Ranges { .. }) {
        return Err(Error::NothingToDo(
            "this PDF has only one part to split into".into(),
        ));
    }
    Ok(groups)
}

/// Parses "1-3, 5, 8-" into page groups. An open start or end runs to the
/// first or last page.
pub fn parse_ranges(spec: &str, count: u32) -> Result<Vec<Vec<u32>>> {
    let bad =
        |part: &str| Error::Invalid(format!("\"{part}\" isn't a page range (try 1-3, 5, 8-)"));
    let mut groups = Vec::new();
    for part in spec
        .split([',', ';'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let start = if a.is_empty() {
            1
        } else {
            a.parse::<u32>().map_err(|_| bad(part))?
        };
        let end = if b.is_empty() {
            count
        } else {
            b.parse::<u32>().map_err(|_| bad(part))?
        };
        if start == 0 || start > end {
            return Err(bad(part));
        }
        if end > count {
            return Err(Error::Invalid(format!(
                "page {end} doesn't exist (the PDF has {count})"
            )));
        }
        groups.push((start..=end).collect());
    }
    if groups.is_empty() {
        return Err(Error::Invalid("enter at least one page range".into()));
    }
    Ok(groups)
}

/// Splits `path` into one PDF per group, written as `out_dir/<index>.pdf`.
/// Returns the files in group order.
pub fn split_into(
    path: &Path,
    groups: &[Vec<u32>],
    out_dir: &Path,
) -> Result<Vec<std::path::PathBuf>> {
    let src = load(path)?;
    let mut outs = Vec::with_capacity(groups.len());
    for (i, group) in groups.iter().enumerate() {
        let mut b = PdfBuilder::new();
        b.import_pages(src.clone(), Some(group))?;
        let out = out_dir.join(format!("part{i}.pdf"));
        b.finish(&out)?;
        outs.push(out);
    }
    Ok(outs)
}

/// A short label for a page group: "page 4" or "pages 1-3".
pub fn group_label(group: &[u32]) -> String {
    match (group.first(), group.last()) {
        (Some(a), Some(b)) if a == b => format!("page {a}"),
        (Some(a), Some(b)) if group.len() as u32 == b - a + 1 => format!("pages {a}-{b}"),
        _ => format!("{} pages", group.len()),
    }
}

/// Number of pages, for reporting.
pub fn page_count(path: &Path) -> Result<usize> {
    Ok(load(path)?.get_pages().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(dir: &Path, name: &str, w: u32, h: u32, alpha: bool) -> std::path::PathBuf {
        let p = dir.join(name);
        if alpha {
            image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 100]))
                .save(&p)
                .unwrap();
        } else {
            image::RgbImage::from_pixel(w, h, image::Rgb([1, 2, 3]))
                .save(&p)
                .unwrap();
        }
        p
    }

    #[test]
    fn images_and_pdfs_combine_in_order() {
        let caps = Capabilities::none();
        let dir = tempfile::tempdir().unwrap();
        let a = write_png(dir.path(), "a.png", 30, 20, true);
        let jpg = dir.path().join("b.jpg");
        image::RgbImage::from_pixel(50, 40, image::Rgb([200, 100, 0]))
            .save(&jpg)
            .unwrap();

        let first = dir.path().join("first.pdf");
        let mut b = PdfBuilder::new();
        b.add_file(&a, Format::Png, &caps).unwrap();
        b.add_file(&jpg, Format::Jpeg, &caps).unwrap();
        b.finish(&first).unwrap();
        assert_eq!(page_count(&first).unwrap(), 2);

        let merged = dir.path().join("merged.pdf");
        let mut b = PdfBuilder::new();
        b.add_file(&first, Format::Pdf, &caps).unwrap();
        b.add_file(&a, Format::Png, &caps).unwrap();
        b.add_file(&first, Format::Pdf, &caps).unwrap();
        b.finish(&merged).unwrap();
        let doc = Document::load(&merged).unwrap();
        let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
        assert_eq!(pages.len(), 5);
        // Every page keeps its resources and media box after re-parenting.
        for id in pages {
            let d = doc.get_dictionary(id).unwrap();
            assert!(d.has(b"Resources") && d.has(b"MediaBox"));
        }
    }

    #[test]
    fn jpeg_is_embedded_without_recompression() {
        let caps = Capabilities::none();
        let dir = tempfile::tempdir().unwrap();
        let jpg = dir.path().join("p.jpg");
        image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([x as u8 * 4, y as u8 * 4, 9]))
            .save(&jpg)
            .unwrap();
        let original = std::fs::read(&jpg).unwrap();
        let out = dir.path().join("p.pdf");
        let mut b = PdfBuilder::new();
        b.add_file(&jpg, Format::Jpeg, &caps).unwrap();
        b.finish(&out).unwrap();
        let pdf = std::fs::read(&out).unwrap();
        assert!(pdf
            .windows(original.len())
            .any(|w| w == original.as_slice()));
    }

    fn numbered_pdf(dir: &Path, pages: u32) -> std::path::PathBuf {
        // Each page is a different width so order can be checked.
        let caps = Capabilities::none();
        let mut b = PdfBuilder::new();
        for i in 1..=pages {
            let p = dir.join(format!("p{i}.png"));
            image::RgbImage::from_pixel(10 * i, 20, image::Rgb([i as u8, 0, 0]))
                .save(&p)
                .unwrap();
            b.add_file(&p, Format::Png, &caps).unwrap();
        }
        let out = dir.join(format!("doc{pages}.pdf"));
        b.finish(&out).unwrap();
        out
    }

    fn page_widths(path: &Path) -> Vec<f32> {
        let doc = Document::load(path).unwrap();
        doc.get_pages()
            .values()
            .map(|id| {
                let d = doc.get_dictionary(*id).unwrap();
                let mb = d.get(b"MediaBox").unwrap().as_array().unwrap();
                mb[2].as_float().unwrap()
            })
            .collect()
    }

    #[test]
    fn select_reorders_and_drops_pages() {
        let dir = tempfile::tempdir().unwrap();
        let src = numbered_pdf(dir.path(), 4);
        let widths = page_widths(&src);
        let out = dir.path().join("sel.pdf");
        select_pages(&src, &[3, 1], &[], &out).unwrap();
        assert_eq!(page_widths(&out), vec![widths[2], widths[0]]);
        // The dropped pages' images aren't carried along.
        assert!(std::fs::metadata(&out).unwrap().len() < std::fs::metadata(&src).unwrap().len());
        assert!(select_pages(&src, &[1, 1], &[], &out).is_err());
        assert!(select_pages(&src, &[5], &[], &out).is_err());
        assert!(select_pages(&src, &[], &[], &out).is_err());
    }

    #[test]
    fn split_modes() {
        use crate::ops::SplitMode;
        assert_eq!(
            split_groups(&SplitMode::EveryPage, 3).unwrap(),
            vec![vec![1], vec![2], vec![3]]
        );
        assert_eq!(
            split_groups(&SplitMode::Chunks { size: 2 }, 5).unwrap(),
            vec![vec![1, 2], vec![3, 4], vec![5]]
        );
        assert_eq!(
            split_groups(
                &SplitMode::Ranges {
                    ranges: "1-2, 4, 5-".into()
                },
                6
            )
            .unwrap(),
            vec![vec![1, 2], vec![4], vec![5, 6]]
        );
        assert_eq!(parse_ranges("-2", 4).unwrap(), vec![vec![1, 2]]);
        assert!(parse_ranges("3-1", 4).is_err());
        assert!(parse_ranges("1-9", 4).is_err());
        assert!(parse_ranges("a", 4).is_err());
        assert!(parse_ranges(" , ", 4).is_err());
        assert!(matches!(
            split_groups(&SplitMode::EveryPage, 1),
            Err(Error::NothingToDo(_))
        ));
        assert_eq!(group_label(&[4]), "page 4");
        assert_eq!(group_label(&[1, 2, 3]), "pages 1-3");
    }

    #[test]
    fn split_writes_each_part() {
        let dir = tempfile::tempdir().unwrap();
        let src = numbered_pdf(dir.path(), 5);
        let out_dir = dir.path().join("parts");
        std::fs::create_dir(&out_dir).unwrap();
        let parts = split_into(&src, &[vec![1, 2], vec![3], vec![4, 5]], &out_dir).unwrap();
        let counts: Vec<usize> = parts.iter().map(|p| page_count(p).unwrap()).collect();
        assert_eq!(counts, vec![2, 1, 2]);
    }

    #[test]
    fn jpeg_component_count() {
        let dir = tempfile::tempdir().unwrap();
        let rgb = dir.path().join("c.jpg");
        image::RgbImage::new(8, 8).save(&rgb).unwrap();
        assert_eq!(jpeg_components(&std::fs::read(&rgb).unwrap()), Some(3));
        let grey = dir.path().join("g.jpg");
        image::GrayImage::new(8, 8).save(&grey).unwrap();
        assert_eq!(jpeg_components(&std::fs::read(&grey).unwrap()), Some(1));
        // A hand-made CMYK frame header.
        let mut cmyk = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0, 8, 0, 8, 4];
        cmyk.extend_from_slice(&[0; 12]);
        assert_eq!(jpeg_components(&cmyk), Some(4));
        assert_eq!(jpeg_components(&[0xFF, 0xD8, 0xFF]), None);
    }

    #[test]
    fn metadata_is_removed() {
        let caps = Capabilities::none();
        let dir = tempfile::tempdir().unwrap();
        let png = write_png(dir.path(), "x.png", 10, 10, false);
        let plain = dir.path().join("plain.pdf");
        let mut b = PdfBuilder::new();
        b.add_file(&png, Format::Png, &caps).unwrap();
        b.finish(&plain).unwrap();

        let mut doc = Document::load(&plain).unwrap();
        let info =
            doc.add_object(dictionary! { "Author" => Object::string_literal("Secret Author") });
        doc.trailer.set("Info", info);
        let tagged = dir.path().join("tagged.pdf");
        doc.save(&tagged).unwrap();
        assert!(String::from_utf8_lossy(&std::fs::read(&tagged).unwrap()).contains("Secret Author"));

        let clean = dir.path().join("clean.pdf");
        strip_metadata(&tagged, &clean).unwrap();
        assert!(!String::from_utf8_lossy(&std::fs::read(&clean).unwrap()).contains("Secret Author"));
        assert_eq!(page_count(&clean).unwrap(), 1);
    }

    #[test]
    fn image_only_pdf_has_no_text() {
        let caps = Capabilities::none();
        let dir = tempfile::tempdir().unwrap();
        let png = write_png(dir.path(), "x.png", 10, 10, false);
        let out = dir.path().join("x.pdf");
        let mut b = PdfBuilder::new();
        b.add_file(&png, Format::Png, &caps).unwrap();
        b.finish(&out).unwrap();
        assert!(matches!(to_text(&out), Err(Error::NothingToDo(_))));
    }
}
