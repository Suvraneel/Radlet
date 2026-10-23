//! Changes to existing PDFs: rotating pages, page numbers, watermarks,
//! shrinking embedded images, and adding or removing a password.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, RgbImage};
use lopdf::content::{Content, Operation};
use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{
    dictionary, Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId,
    Permissions, SaveOptions, Stream, StringFormat,
};

use crate::engines::image::human;
use crate::engines::pdf;
use crate::error::{Error, Result};
use crate::ops::{CompressLevel, NumberPosition, NumberStyle};

// ---------------------------------------------------------------------------
// Rotation

/// A page attribute from the page itself or, for the attributes pages
/// inherit, the nearest ancestor in the page tree that has it.
fn page_attr(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let dict = doc.get_dictionary(page).ok()?;
    match dict.get(key) {
        Ok(v) => Some(v.clone()),
        Err(_) => pdf::inherited(doc, page, key),
    }
}

/// A page's rotation (0, 90, 180 or 270), however it's stored: an
/// integer or a real, directly or by reference, on the page or inherited.
fn rotation(doc: &Document, page: ObjectId) -> i64 {
    let value = match page_attr(doc, page, b"Rotate") {
        Some(Object::Reference(id)) => doc.get_object(id).ok().cloned(),
        other => other,
    };
    let degrees = value.and_then(|v| v.as_float().ok()).unwrap_or(0.0);
    if !degrees.is_finite() {
        return 0;
    }
    ((degrees as f64 / 90.0).round() as i64 * 90).rem_euclid(360)
}

/// Adds `degrees` (90, 180 or 270) to a page's rotation.
pub(crate) fn turn_page(doc: &mut Document, page: ObjectId, degrees: u32) -> Result<()> {
    if ![90, 180, 270].contains(&degrees) {
        return Err(Error::Invalid(
            "pages turn by 90, 180 or 270 degrees".into(),
        ));
    }
    let turned = (rotation(doc, page) + degrees as i64).rem_euclid(360);
    doc.get_dictionary_mut(page)?.set("Rotate", turned);
    Ok(())
}

/// Saves an edited document, compressing only streams added unfiltered.
fn save(doc: &mut Document, out: &Path) -> Result<()> {
    doc.compress();
    let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
    doc.save_to(&mut file)
        .map_err(|e| Error::Pdf(format!("couldn't save the PDF ({e})")))?;
    file.flush()?;
    Ok(())
}

/// The document's pages, in order; an error if it has none.
fn pages(doc: &Document) -> Result<Vec<ObjectId>> {
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    if pages.is_empty() {
        return Err(Error::Pdf("this PDF has no pages".into()));
    }
    Ok(pages)
}

/// Turns every page clockwise. The document is edited in place, so
/// bookmarks, links, form fields and its title are kept.
pub fn rotate_all(path: &Path, degrees: u32, out: &Path) -> Result<()> {
    let mut doc = pdf::load(path)?;
    for id in pages(&doc)? {
        turn_page(&mut doc, id, degrees)?;
    }
    save(&mut doc, out)
}

// ---------------------------------------------------------------------------
// Stamping text on pages

/// A page as the reader sees it: its upright size, and the matrix from
/// upright coordinates (origin bottom left) to the page's own space, which
/// accounts for its crop box and rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PageView {
    pub width: f32,
    pub height: f32,
    pub matrix: [f32; 6],
}

fn number_array(doc: &Document, obj: &Object) -> Option<[f32; 4]> {
    let obj = match obj {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        o => o,
    };
    let a = obj.as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let mut v = [0f32; 4];
    for (slot, item) in v.iter_mut().zip(a) {
        *slot = match item {
            Object::Reference(id) => doc.get_object(*id).ok()?.as_float().ok()?,
            o => o.as_float().ok()?,
        };
    }
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

pub(crate) fn page_view(doc: &Document, page: ObjectId) -> Result<PageView> {
    doc.get_dictionary(page)?;
    let media = page_attr(doc, page, b"MediaBox")
        .and_then(|o| number_array(doc, &o))
        .filter(|m| m[2] > m[0] && m[3] > m[1])
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let crop = page_attr(doc, page, b"CropBox")
        .and_then(|o| number_array(doc, &o))
        .filter(|c| c[2] > c[0] && c[3] > c[1])
        .unwrap_or(media);
    let [x0, y0, x1, y1] = crop;
    let (bw, bh) = (x1 - x0, y1 - y0);
    let r = rotation(doc, page);
    // The viewer turns the page clockwise by `r`; drawing through the
    // inverse turn makes text come out upright.
    Ok(match r {
        90 => PageView {
            width: bh,
            height: bw,
            matrix: [0.0, 1.0, -1.0, 0.0, x0 + bw, y0],
        },
        180 => PageView {
            width: bw,
            height: bh,
            matrix: [-1.0, 0.0, 0.0, -1.0, x0 + bw, y0 + bh],
        },
        270 => PageView {
            width: bh,
            height: bw,
            matrix: [0.0, -1.0, 1.0, 0.0, x0, y0 + bh],
        },
        _ => PageView {
            width: bw,
            height: bh,
            matrix: [1.0, 0.0, 0.0, 1.0, x0, y0],
        },
    })
}

/// Helvetica advance widths (per 1000 units of text size) for character
/// codes 32 to 126.
pub(crate) const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // 32-47
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // 48-63
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // 64-79
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // 80-95
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // 96-111
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // 112-126
];

/// Width of WinAnsi-encoded text at 1 pt.
fn text_width(bytes: &[u8]) -> f32 {
    bytes
        .iter()
        .map(|&b| match b {
            32..=126 => HELVETICA[(b - 32) as usize],
            _ => 556,
        } as f32)
        .sum::<f32>()
        / 1000.0
}

/// Encodes text for the standard Helvetica font (Windows-1252). Letters
/// outside it can't be shown without embedding a font.
fn win_ansi(text: &str) -> Result<Vec<u8>> {
    let extra = |c: char| -> Option<u8> {
        Some(match c {
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
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '˜' => 0x98,
            '™' => 0x99,
            'š' => 0x9A,
            '›' => 0x9B,
            'œ' => 0x9C,
            'ž' => 0x9E,
            'Ÿ' => 0x9F,
            _ => return None,
        })
    };
    text.chars()
        .map(|c| match c as u32 {
            0x20..=0x7E | 0xA0..=0xFF => Ok(c as u8),
            _ => extra(c).ok_or_else(|| {
                Error::Invalid(format!(
                    "\"{c}\" can't be printed here; use Latin letters, numbers and punctuation"
                ))
            }),
        })
        .collect()
}

fn real(v: f32) -> Object {
    Object::Real(v)
}

/// One line of text to print on a page, in upright page coordinates.
struct TextStamp {
    /// WinAnsi-encoded text.
    bytes: Vec<u8>,
    size: f32,
    gray: f32,
    matrix: [f32; 6],
}

fn text_ops(text: TextStamp, font: &[u8]) -> Vec<Operation> {
    vec![
        Operation::new("BT", vec![]),
        Operation::new("Tf", vec![Object::Name(font.to_vec()), real(text.size)]),
        Operation::new("g", vec![real(text.gray)]),
        Operation::new("Tm", text.matrix.iter().map(|&v| real(v)).collect()),
        Operation::new(
            "Tj",
            vec![Object::String(text.bytes, StringFormat::Hexadecimal)],
        ),
        Operation::new("ET", vec![]),
    ]
}

/// `base`, or `base1`, `base2`… if a resource by that name already exists.
fn free_name(dict: &Dictionary, base: &str) -> Vec<u8> {
    std::iter::once(base.to_string())
        .chain((1..).map(|n| format!("{base}{n}")))
        .map(String::into_bytes)
        .find(|k| !dict.has(k))
        .expect("an unused name exists")
}

/// How a page's own content leaves the graphics state stack: how many
/// `Q`s it has without a matching `q` (`under`), and how many `q`s are
/// still open at the end (`open`). Stamps must restore both, or they're
/// drawn inside the page's leftover transformations.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
struct SaveBalance {
    under: usize,
    open: usize,
}

fn save_balance(doc: &Document, contents: &[Object]) -> SaveBalance {
    let mut balance = SaveBalance::default();
    for c in contents {
        let stream = match c {
            Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_stream().ok()),
            _ => None,
        };
        let Some(Ok(data)) = stream.map(|s| s.get_plain_content_with_limit(64 << 20)) else {
            continue;
        };
        count_saves(&data, &mut balance);
    }
    SaveBalance {
        under: balance.under.min(256),
        open: balance.open.min(256),
    }
}

/// Counts `q` and `Q` operators in content, skipping strings, names,
/// comments and inline image data.
fn count_saves(data: &[u8], balance: &mut SaveBalance) {
    let space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0');
    let delim = |b: u8| {
        matches!(
            b,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
    };
    let n = data.len();
    let mut i = 0;
    while i < n {
        let b = data[i];
        if space(b) {
            i += 1;
            continue;
        }
        match b {
            b'%' => {
                while i < n && data[i] != b'\n' && data[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => {
                let mut level = 0usize;
                while i < n {
                    match data[i] {
                        b'\\' => i += 1,
                        b'(' => level += 1,
                        b')' => {
                            level = level.saturating_sub(1);
                            if level == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'<' if data.get(i + 1) == Some(&b'<') => i += 2,
            b'<' => {
                while i < n && data[i] != b'>' {
                    i += 1;
                }
                i += 1;
            }
            b'/' => {
                i += 1;
                while i < n && !space(data[i]) && !delim(data[i]) {
                    i += 1;
                }
            }
            b'>' | b'[' | b']' | b'{' | b'}' | b')' => i += 1,
            _ => {
                let start = i;
                while i < n && !space(data[i]) && !delim(data[i]) {
                    i += 1;
                }
                match &data[start..i] {
                    b"q" => balance.open += 1,
                    b"Q" if balance.open > 0 => balance.open -= 1,
                    b"Q" => balance.under += 1,
                    b"ID" => {
                        // Binary image data runs to "EI" between spaces.
                        i += 1;
                        while i + 1 < n {
                            if data[i] == b'E'
                                && data[i + 1] == b'I'
                                && space(data[i - 1])
                                && data.get(i + 2).is_none_or(|&c| space(c) || delim(c))
                            {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Resolves a dictionary that may be stored by reference.
fn owned_dict(doc: &Document, obj: Option<&Object>) -> Dictionary {
    match obj {
        Some(Object::Dictionary(d)) => d.clone(),
        Some(Object::Reference(id)) => doc.get_dictionary(*id).cloned().unwrap_or_default(),
        _ => Dictionary::new(),
    }
}

/// The page's content streams, references unwrapped.
fn content_refs(doc: &Document, page: &Dictionary) -> Vec<Object> {
    match page.get(b"Contents") {
        Ok(Object::Array(a)) => a.clone(),
        Ok(Object::Reference(id)) => match doc.get_object(*id) {
            Ok(Object::Array(a)) => a.clone(),
            _ => vec![Object::Reference(*id)],
        },
        _ => Vec::new(),
    }
}

/// Draws text on top of every page. `draw` returns the text for a page (in
/// upright coordinates) or None to leave it alone. The page's own content
/// is wrapped in q/Q, balanced against any it leaves open, so its graphics
/// state can't leak into the stamp. The document is edited in place, so
/// bookmarks, links, form fields and metadata are kept.
fn stamp(
    path: &Path,
    out: &Path,
    opacity: Option<f32>,
    mut draw: impl FnMut(usize, usize, &PageView) -> Result<Option<TextStamp>>,
) -> Result<()> {
    let mut doc = pdf::load(path)?;
    let ids = pages(&doc)?;
    let total = ids.len();
    let font = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let state = opacity.map(|a| {
        doc.add_object(dictionary! { "Type" => "ExtGState", "ca" => real(a), "CA" => real(a) })
    });
    // Shared by pages whose content is balanced the same way.
    let mut wrappers: HashMap<SaveBalance, (ObjectId, ObjectId)> = HashMap::new();
    for (i, &page) in ids.iter().enumerate() {
        let view = page_view(&doc, page)?;
        let Some(text) = draw(i, total, &view)? else {
            continue;
        };
        // Resources may be inherited or shared; the page gets its own copy
        // (entries inside stay shared references).
        let mut resources = owned_dict(&doc, page_attr(&doc, page, b"Resources").as_ref());
        let mut fonts = owned_dict(&doc, resources.get(b"Font").ok());
        let font_name = free_name(&fonts, "VeerF");
        fonts.set(font_name.clone(), font);
        resources.set("Font", fonts);
        let mut ops = vec![
            Operation::new("q", vec![]),
            Operation::new("cm", view.matrix.iter().map(|&v| real(v)).collect()),
        ];
        if let Some(gs) = state {
            let mut states = owned_dict(&doc, resources.get(b"ExtGState").ok());
            let gs_name = free_name(&states, "VeerGS");
            states.set(gs_name.clone(), gs);
            resources.set("ExtGState", states);
            ops.push(Operation::new("gs", vec![Object::Name(gs_name)]));
        }
        ops.extend(text_ops(text, &font_name));
        ops.push(Operation::new("Q", vec![]));
        let content = Content { operations: ops }
            .encode()
            .map_err(|e| Error::Pdf(format!("couldn't write the page text ({e})")))?;
        let added = doc.add_object(Stream::new(Dictionary::new(), content));

        let originals = content_refs(&doc, doc.get_dictionary(page)?);
        let balance = save_balance(&doc, &originals);
        let (open, close) = *wrappers.entry(balance).or_insert_with(|| {
            let open = "q\n".repeat(1 + balance.under).into_bytes();
            let close = format!("\n{}", "Q\n".repeat(1 + balance.open)).into_bytes();
            (
                doc.add_object(Stream::new(Dictionary::new(), open)),
                doc.add_object(Stream::new(Dictionary::new(), close)),
            )
        });
        let mut contents = vec![Object::Reference(open)];
        contents.extend(originals);
        contents.push(Object::Reference(close));
        contents.push(Object::Reference(added));

        let dict = doc.get_dictionary_mut(page)?;
        dict.set("Resources", resources);
        dict.set("Contents", contents);
    }
    save(&mut doc, out)
}

/// Options for [`page_numbers`].
#[derive(Debug, Clone, Copy)]
pub struct NumberOptions {
    pub position: NumberPosition,
    pub style: NumberStyle,
    pub start: u32,
    pub size: f32,
    pub skip_first: bool,
}

/// Prints a number on every page.
pub fn page_numbers(path: &Path, opts: NumberOptions, out: &Path) -> Result<()> {
    if !(4.0..=72.0).contains(&opts.size) {
        return Err(Error::Invalid("page numbers must be 4 to 72 points".into()));
    }
    stamp(path, out, None, |i, total, view| {
        if opts.skip_first && i == 0 {
            return Ok(None);
        }
        let n = opts.start.saturating_add(i as u32);
        let last = opts.start.saturating_add(total.saturating_sub(1) as u32);
        let bytes = win_ansi(&opts.style.label(n, last))?;
        let width = text_width(&bytes) * opts.size;
        // Half an inch from the edges, less on small pages.
        let margin = 36f32.min(view.width * 0.06).min(view.height * 0.06);
        use NumberPosition::*;
        let x = match opts.position {
            BottomCenter | TopCenter => (view.width - width) / 2.0,
            BottomRight | TopRight => view.width - margin - width,
            BottomLeft | TopLeft => margin,
        };
        let y = match opts.position {
            BottomCenter | BottomRight | BottomLeft => margin * 0.75,
            TopCenter | TopRight | TopLeft => view.height - margin * 0.75 - opts.size * 0.72,
        };
        Ok(Some(TextStamp {
            bytes,
            size: opts.size,
            gray: 0.0,
            matrix: [1.0, 0.0, 0.0, 1.0, x, y],
        }))
    })
}

/// Options for [`watermark`].
#[derive(Debug, Clone)]
pub struct WatermarkOptions {
    pub text: String,
    pub opacity: f32,
    pub size: Option<f32>,
    pub diagonal: bool,
}

/// Prints faint text across every page.
pub fn watermark(path: &Path, opts: &WatermarkOptions, out: &Path) -> Result<()> {
    let text = opts.text.trim();
    if text.is_empty() {
        return Err(Error::Invalid("enter the watermark text".into()));
    }
    if text.chars().count() > 200 {
        return Err(Error::Invalid(
            "keep the watermark under 200 characters".into(),
        ));
    }
    if !(0.05..=1.0).contains(&opts.opacity) {
        return Err(Error::Invalid("watermark opacity must be 0.05 to 1".into()));
    }
    if let Some(s) = opts.size {
        if !(6.0..=300.0).contains(&s) {
            return Err(Error::Invalid(
                "watermark text must be 6 to 300 points".into(),
            ));
        }
    }
    let bytes = win_ansi(text)?;
    let unit = text_width(&bytes).max(0.1);
    stamp(path, out, Some(opts.opacity), |_, _, view| {
        let (w, h) = (view.width, view.height);
        let angle = if opts.diagonal { h.atan2(w) } else { 0.0 };
        let span = if opts.diagonal { w.hypot(h) } else { w };
        let size = opts
            .size
            .unwrap_or_else(|| (span * 0.7 / unit).min(h * 0.4).clamp(8.0, 250.0));
        let (sin, cos) = angle.sin_cos();
        let width = unit * size;
        let cap = 0.72 * size;
        // Centre the text box on the page.
        let x = w / 2.0 - width / 2.0 * cos + cap / 2.0 * sin;
        let y = h / 2.0 - width / 2.0 * sin - cap / 2.0 * cos;
        Ok(Some(TextStamp {
            bytes: bytes.clone(),
            size,
            gray: 0.3,
            matrix: [cos, sin, -sin, cos, x, y],
        }))
    })
}

// ---------------------------------------------------------------------------
// Shrinking

/// Makes a PDF smaller: images are scaled down to a sensible size and
/// recompressed, and the file structure is packed. With `target`, tries
/// stronger levels until the file fits.
pub fn shrink(path: &Path, level: CompressLevel, target: Option<u64>, out: &Path) -> Result<()> {
    let levels = match target {
        None => vec![level],
        Some(_) => vec![
            CompressLevel::Light,
            CompressLevel::Balanced,
            CompressLevel::Strong,
        ],
    };
    let mut best: Option<Vec<u8>> = None;
    for l in levels {
        let bytes = shrink_to_bytes(path, l)?;
        let fits = target.is_some_and(|t| bytes.len() as u64 <= t);
        if best.as_ref().is_none_or(|b| bytes.len() < b.len()) {
            best = Some(bytes);
        }
        if fits {
            break;
        }
    }
    let best = best.expect("at least one level ran");
    if let Some(t) = target {
        if best.len() as u64 > t {
            return Err(Error::NothingToDo(format!(
                "couldn't get this PDF below {} (smallest was {})",
                human(t),
                human(best.len() as u64)
            )));
        }
    }
    std::fs::write(out, best)?;
    Ok(())
}

struct ShrinkPlan {
    /// Longest side kept, in pixels.
    max_side: u32,
    quality: u8,
    /// Also turn losslessly stored images into JPEG.
    lossless_to_jpeg: bool,
}

fn plan(level: CompressLevel) -> ShrinkPlan {
    match level {
        CompressLevel::Light => ShrinkPlan {
            max_side: 2500,
            quality: level.image_quality(),
            lossless_to_jpeg: false,
        },
        CompressLevel::Balanced | CompressLevel::Smart => ShrinkPlan {
            max_side: 1800,
            quality: level.image_quality(),
            lossless_to_jpeg: false,
        },
        CompressLevel::Strong => ShrinkPlan {
            max_side: 1200,
            quality: level.image_quality(),
            lossless_to_jpeg: true,
        },
    }
}

fn shrink_to_bytes(path: &Path, level: CompressLevel) -> Result<Vec<u8>> {
    let mut doc = pdf::load(path)?;
    let plan = plan(level);
    let images: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter(|(_, o)| {
            o.as_stream()
                .ok()
                .and_then(|s| s.dict.get(b"Subtype").ok())
                .and_then(|t| t.as_name().ok())
                == Some(b"Image".as_slice())
        })
        .map(|(id, _)| *id)
        .collect();
    // Soft masks and stencil masks other images point at must keep their
    // exact pixels.
    let masks: std::collections::HashSet<ObjectId> = images
        .iter()
        .filter_map(|id| doc.get_object(*id).ok()?.as_stream().ok())
        .flat_map(|s| {
            [b"SMask".as_slice(), b"Mask".as_slice()]
                .into_iter()
                .filter_map(|k| s.dict.get(k).and_then(Object::as_reference).ok())
        })
        .collect();
    for id in images {
        if masks.contains(&id) {
            continue;
        }
        // An image we can't safely rework is left exactly as it was.
        if let Ok(Some(stream)) = recompress_image(&doc, id, &plan) {
            doc.objects.insert(id, Object::Stream(stream));
        }
    }
    doc.prune_objects();
    doc.compress();
    let mut buf = Vec::new();
    doc.save_with_options(
        &mut buf,
        SaveOptions::builder()
            .use_object_streams(true)
            .use_xref_streams(true)
            .compression_level(9)
            .build(),
    )
    .map_err(|e| Error::Pdf(format!("couldn't save the PDF ({e})")))?;
    Ok(buf)
}

/// Colour components of an image's colour space: 1 or 3 for the spaces we
/// rework, None for anything else (CMYK, indexed, separations...).
fn components(doc: &Document, space: &Object) -> Option<usize> {
    let space = match space {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        o => o,
    };
    match space {
        Object::Name(n) if n == b"DeviceRGB" => Some(3),
        Object::Name(n) if n == b"DeviceGray" => Some(1),
        Object::Array(a) if a.len() == 2 && a[0].as_name().ok() == Some(b"ICCBased".as_slice()) => {
            let profile = match &a[1] {
                Object::Reference(id) => doc.get_object(*id).ok()?.as_stream().ok()?,
                _ => return None,
            };
            match profile.dict.get(b"N").and_then(Object::as_i64).ok()? {
                1 => Some(1),
                3 => Some(3),
                _ => None,
            }
        }
        _ => None,
    }
}

fn recompress_image(doc: &Document, id: ObjectId, plan: &ShrinkPlan) -> Result<Option<Stream>> {
    let stream = doc.get_object(id)?.as_stream()?;
    let d = &stream.dict;
    // Stencil masks, colour-key masks (lossy pixels would break the exact
    // match), Decode arrays, and soft masks with a pre-blended Matte colour
    // are left alone.
    if d.get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false)
        || d.has(b"Decode")
        || d.has(b"Mask")
    {
        return Ok(None);
    }
    if let Ok(mask) = d.get(b"SMask").and_then(Object::as_reference) {
        if doc
            .get_object(mask)
            .and_then(Object::as_stream)
            .map(|m| m.dict.has(b"Matte"))
            .unwrap_or(true)
        {
            return Ok(None);
        }
    }
    if d.get(b"BitsPerComponent").and_then(Object::as_i64).ok() != Some(8) {
        return Ok(None);
    }
    let (Ok(w), Ok(h)) = (
        d.get(b"Width").and_then(Object::as_i64),
        d.get(b"Height").and_then(Object::as_i64),
    ) else {
        return Ok(None);
    };
    if w <= 0 || h <= 0 || w > 40_000 || h > 40_000 {
        return Ok(None);
    }
    let (w, h) = (w as u32, h as u32);
    let Some(channels) = d.get(b"ColorSpace").ok().and_then(|cs| components(doc, cs)) else {
        return Ok(None);
    };
    let filter = match d.get(b"Filter") {
        Ok(Object::Name(n)) => Some(n.clone()),
        Ok(Object::Array(a)) if a.len() == 1 => a[0].as_name().ok().map(<[u8]>::to_vec),
        _ => None,
    };
    let jpeg = match filter.as_deref() {
        Some(b"DCTDecode") => true,
        Some(b"FlateDecode") => false,
        _ => return Ok(None),
    };
    // A JPEG whose colour transform is overridden, or a Flate image with a
    // predictor, would decode with the wrong colours here.
    if d.has(b"DecodeParms") {
        return Ok(None);
    }
    let img = if jpeg {
        let decoded =
            image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg)?;
        let expected = if channels == 1 { 1 } else { 3 };
        if decoded.color().channel_count() as usize != expected
            || decoded.width() != w
            || decoded.height() != h
        {
            return Ok(None);
        }
        decoded
    } else {
        let raw = stream
            .decompressed_content_with_limit(1 << 30)
            .map_err(|e| Error::Pdf(e.to_string()))?;
        if raw.len() != (w as usize) * (h as usize) * channels {
            return Ok(None);
        }
        if channels == 1 {
            DynamicImage::ImageLuma8(
                GrayImage::from_raw(w, h, raw)
                    .ok_or_else(|| Error::Pdf("bad image data".into()))?,
            )
        } else {
            DynamicImage::ImageRgb8(
                RgbImage::from_raw(w, h, raw).ok_or_else(|| Error::Pdf("bad image data".into()))?,
            )
        }
    };
    let longest = w.max(h);
    let scaled = longest > plan.max_side;
    let img = if scaled {
        let f = plan.max_side as f64 / longest as f64;
        let nw = ((w as f64 * f).round() as u32).max(1);
        let nh = ((h as f64 * f).round() as u32).max(1);
        img.resize_exact(nw, nh, FilterType::Lanczos3)
    } else {
        img
    };
    let as_jpeg = jpeg || plan.lossless_to_jpeg;
    if !as_jpeg && !scaled {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    if as_jpeg {
        JpegEncoder::new_with_quality(&mut bytes, plan.quality).encode_image(&img)?;
    } else {
        let raw = if channels == 1 {
            img.to_luma8().into_raw()
        } else {
            img.to_rgb8().into_raw()
        };
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
        enc.write_all(&raw)?;
        bytes = enc.finish()?;
    }
    if !scaled && bytes.len() >= stream.content.len() {
        return Ok(None);
    }
    let mut dict = d.clone();
    dict.set("Width", img.width() as i64);
    dict.set("Height", img.height() as i64);
    dict.set(
        "Filter",
        Object::Name(if as_jpeg {
            b"DCTDecode".to_vec()
        } else {
            b"FlateDecode".to_vec()
        }),
    );
    dict.remove(b"DecodeParms");
    dict.remove(b"Length");
    Ok(Some(Stream::new(dict, bytes).with_compression(false)))
}

// ---------------------------------------------------------------------------
// Passwords

/// Locks a PDF with `password` (AES-256, needed to open it).
pub fn protect(path: &Path, password: &str, out: &Path) -> Result<()> {
    if password.is_empty() {
        return Err(Error::Invalid("enter a password".into()));
    }
    let mut doc = pdf::load(path).map_err(|e| match e {
        Error::Pdf(m) if m.contains("password") => Error::NothingToDo(
            "this PDF already has a password; remove it first to change it".into(),
        ),
        other => other,
    })?;
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| Error::Internal(format!("no randomness: {e}")))?;
    if !doc.trailer.has(b"ID") {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|e| Error::Internal(format!("no randomness: {e}")))?;
        let id = Object::String(id.to_vec(), StringFormat::Hexadecimal);
        doc.trailer.set("ID", vec![id.clone(), id]);
    }
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let version = EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: password,
        user_password: password,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version)
        .map_err(|e| Error::Invalid(format!("this password can't be used ({e})")))?;
    doc.encrypt(&state)
        .map_err(|e| Error::Pdf(format!("couldn't lock the PDF ({e})")))?;
    mark_aes256(&mut doc)?;
    let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
    doc.save_to(&mut file)
        .map_err(|e| Error::Pdf(format!("couldn't save the PDF ({e})")))?;
    file.flush()?;
    Ok(())
}

/// Adds what some readers need to recognise AES-256 encryption: key
/// lengths in the Encrypt dictionary and its crypt filter, and PDF 1.7
/// with Adobe extension level 8 (where AES-256 was introduced). Without
/// these, Poppler-based readers (Evince, Okular) show blank pages.
fn mark_aes256(doc: &mut Document) -> Result<()> {
    let id = doc
        .trailer
        .get(b"Encrypt")
        .and_then(Object::as_reference)
        .map_err(|_| Error::Internal("the encrypted PDF has no Encrypt dictionary".into()))?;
    let encrypt = doc.get_dictionary_mut(id)?;
    encrypt.set("Length", 256);
    if let Ok(Object::Dictionary(filters)) = encrypt.get_mut(b"CF") {
        if let Ok(Object::Dictionary(std_cf)) = filters.get_mut(b"StdCF") {
            std_cf.set("Length", 32);
            std_cf.set("AuthEvent", Object::Name(b"DocOpen".to_vec()));
        }
    }
    if doc.version.as_str() < "2.0" {
        doc.version = "1.7".into();
        let catalog = doc.catalog_mut()?;
        catalog.set(
            "Extensions",
            dictionary! { "ADBE" => dictionary! { "BaseVersion" => Object::Name(b"1.7".to_vec()), "ExtensionLevel" => 8 } },
        );
    }
    Ok(())
}

/// Removes the password from a PDF.
pub fn unlock(path: &Path, password: &str, out: &Path) -> Result<()> {
    let options = lopdf::LoadOptions {
        password: Some(password.to_string()),
        max_decompressed_size: Some(512 << 20),
        ..Default::default()
    };
    let mut doc = Document::load_with_options(path, options).map_err(|e| match e {
        lopdf::Error::InvalidPassword => Error::Invalid("that password isn't right".into()),
        other => Error::Pdf(format!("couldn't read the PDF ({other})")),
    })?;
    if doc.is_encrypted() {
        return Err(Error::Invalid("that password isn't right".into()));
    }
    if !doc.was_encrypted() {
        return Err(Error::NothingToDo("this PDF has no password".into()));
    }
    doc.encryption_state = None;
    let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
    doc.save_to(&mut file)
        .map_err(|e| Error::Pdf(format!("couldn't save the PDF ({e})")))?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Capabilities;
    use crate::engines::pdf::PdfBuilder;
    use crate::format::Format;

    fn pdf_with_pages(dir: &Path, sizes: &[(u32, u32)]) -> std::path::PathBuf {
        let caps = Capabilities::none();
        let mut b = PdfBuilder::new();
        for (i, (w, h)) in sizes.iter().enumerate() {
            let p = dir.join(format!("p{i}.png"));
            image::RgbImage::from_pixel(*w, *h, image::Rgb([255, 255, 255]))
                .save(&p)
                .unwrap();
            b.add_file(&p, Format::Png, &caps).unwrap();
        }
        let out = dir.join("doc.pdf");
        b.finish(&out).unwrap();
        out
    }

    fn rotations(path: &Path) -> Vec<i64> {
        let doc = Document::load(path).unwrap();
        doc.get_pages()
            .values()
            .map(|id| {
                doc.get_dictionary(*id)
                    .unwrap()
                    .get(b"Rotate")
                    .and_then(Object::as_i64)
                    .unwrap_or(0)
            })
            .collect()
    }

    #[test]
    fn rotating_all_and_single_pages() {
        let dir = tempfile::tempdir().unwrap();
        let src = pdf_with_pages(dir.path(), &[(100, 200), (100, 200), (100, 200)]);
        let all = dir.path().join("all.pdf");
        rotate_all(&src, 90, &all).unwrap();
        assert_eq!(rotations(&all), [90, 90, 90]);
        let again = dir.path().join("again.pdf");
        rotate_all(&all, 270, &again).unwrap();
        assert_eq!(rotations(&again), [0, 0, 0]);

        let some = dir.path().join("some.pdf");
        let turns = [crate::ops::PageTurn {
            page: 3,
            degrees: 180,
        }];
        pdf::select_pages(&src, &[3, 1], &turns, &some).unwrap();
        assert_eq!(rotations(&some), [180, 0]);
        let bad = [crate::ops::PageTurn {
            page: 2,
            degrees: 90,
        }];
        assert!(pdf::select_pages(&src, &[1], &bad, &some).is_err());
        assert!(rotate_all(&src, 45, &some).is_err());
    }

    #[test]
    fn upright_view_of_rotated_and_cropped_pages() {
        let mut doc = Document::with_version("1.7");
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()],
            "CropBox" => vec![50.into(), 100.into(), 550.into(), 700.into()],
            "Rotate" => 90,
        });
        let v = page_view(&doc, page).unwrap();
        assert_eq!((v.width, v.height), (600.0, 500.0));
        // The upright bottom-left corner lands at the crop box's bottom
        // right in page space, as a 90° clockwise turn requires.
        let [a, b, c, d, e, f] = v.matrix;
        let map = |u: f32, w: f32| (a * u + c * w + e, b * u + d * w + f);
        assert_eq!(map(0.0, 0.0), (550.0, 100.0));
        assert_eq!(map(600.0, 500.0), (50.0, 700.0));
    }

    #[test]
    fn latin_text_only() {
        assert_eq!(win_ansi("Page 3 – Draft").unwrap(), b"Page 3 \x96 Draft");
        assert!(win_ansi("草稿").is_err());
        assert!((text_width(b"10") - 1.112).abs() < 1e-6);
    }

    #[test]
    fn numbers_and_watermark_are_added_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let src = pdf_with_pages(dir.path(), &[(612, 792), (792, 612)]);
        let numbered = dir.path().join("numbered.pdf");
        page_numbers(
            &src,
            NumberOptions {
                position: NumberPosition::BottomRight,
                style: NumberStyle::PageOfTotal,
                start: 5,
                size: 10.0,
                skip_first: false,
            },
            &numbered,
        )
        .unwrap();
        let doc = Document::load(&numbered).unwrap();
        let text = doc.extract_text(&[1, 2]).unwrap();
        assert!(
            text.contains("Page 5 of 6") && text.contains("Page 6 of 6"),
            "{text}"
        );

        let marked = dir.path().join("marked.pdf");
        let opts = WatermarkOptions {
            text: "DRAFT".into(),
            opacity: 0.2,
            size: None,
            diagonal: true,
        };
        watermark(&src, &opts, &marked).unwrap();
        let doc = Document::load(&marked).unwrap();
        assert!(doc.extract_text(&[1]).unwrap().contains("DRAFT"));
        let page = doc.get_pages()[&1];
        let resources = doc.get_dictionary(page).unwrap().get(b"Resources").unwrap();
        assert!(resources.as_dict().unwrap().has(b"ExtGState"));

        let bad = WatermarkOptions {
            text: "  ".into(),
            ..opts
        };
        assert!(watermark(&src, &bad, &marked).is_err());
    }

    #[test]
    fn shrinking_scales_big_images() {
        let dir = tempfile::tempdir().unwrap();
        // A noisy 3000 x 2000 photo, embedded as its original JPEG.
        let jpg = dir.path().join("photo.jpg");
        let mut seed = 7u32;
        let img = image::RgbImage::from_fn(3000, 2000, |x, y| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            image::Rgb([(x % 256) as u8, (y % 256) as u8, (seed >> 24) as u8])
        });
        DynamicImage::ImageRgb8(img).save(&jpg).unwrap();
        let mut b = PdfBuilder::new();
        b.add_file(&jpg, Format::Jpeg, &Capabilities::none())
            .unwrap();
        let src = dir.path().join("big.pdf");
        b.finish(&src).unwrap();

        let small = dir.path().join("small.pdf");
        shrink(&src, CompressLevel::Balanced, None, &small).unwrap();
        let before = std::fs::metadata(&src).unwrap().len();
        let after = std::fs::metadata(&small).unwrap().len();
        assert!(after < before / 2, "{before} → {after}");
        let doc = Document::load(&small).unwrap();
        let widths: Vec<i64> = doc
            .objects
            .values()
            .filter_map(|o| o.as_stream().ok())
            .filter(|s| {
                s.dict.get(b"Subtype").and_then(|t| t.as_name()).ok() == Some(b"Image".as_slice())
            })
            .map(|s| s.dict.get(b"Width").unwrap().as_i64().unwrap())
            .collect();
        assert_eq!(widths, [1800]);

        // A target the file can't reach is reported, not half-done.
        let tiny = dir.path().join("tiny.pdf");
        let err = shrink(&src, CompressLevel::Balanced, Some(100), &tiny).unwrap_err();
        assert!(matches!(err, Error::NothingToDo(_)), "{err:?}");
        assert!(!tiny.exists());
    }

    #[test]
    fn protect_and_unlock_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let src = pdf_with_pages(dir.path(), &[(200, 200)]);
        let locked = dir.path().join("locked.pdf");
        protect(&src, "s3cret pass", &locked).unwrap();
        // Without the password the pages can't be read.
        assert!(pdf::load(&locked).is_err());
        assert!(matches!(
            protect(&locked, "again", &dir.path().join("x.pdf")),
            Err(Error::NothingToDo(_))
        ));
        let wrong = unlock(&locked, "nope", &dir.path().join("w.pdf"));
        assert!(
            matches!(wrong, Err(Error::Invalid(ref m)) if m.contains("isn't right")),
            "{wrong:?}"
        );
        let open = dir.path().join("open.pdf");
        unlock(&locked, "s3cret pass", &open).unwrap();
        assert_eq!(pdf::page_count(&open).unwrap(), 1);
        assert!(matches!(
            unlock(&open, "s3cret pass", &dir.path().join("y.pdf")),
            Err(Error::NothingToDo(_))
        ));
        assert!(protect(&src, "", &dir.path().join("z.pdf")).is_err());

        // What Poppler-based readers need to recognise AES-256.
        let raw = Document::load_with_options(
            &locked,
            lopdf::LoadOptions {
                password: None,
                ..Default::default()
            },
        );
        if let Ok(doc) = raw {
            let id = doc.trailer.get(b"Encrypt").unwrap().as_reference().unwrap();
            let e = doc.get_dictionary(id).unwrap();
            assert_eq!(e.get(b"Length").unwrap().as_i64().unwrap(), 256);
            assert_eq!(e.get(b"V").unwrap().as_i64().unwrap(), 5);
        }
        let bytes = std::fs::read(&locked).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.7"));
        assert!(bytes.windows(8).any(|w| w == b"/DocOpen"));
    }

    #[test]
    fn counting_saves_skips_strings_names_and_images() {
        let mut b = SaveBalance::default();
        count_saves(b"q 1 0 0 1 0 0 cm (a Q ( q ) \\) Q) Tj Q", &mut b);
        assert_eq!(b, SaveBalance { under: 0, open: 0 });
        count_saves(
            b"/Q gs % Q Q Q\n q BI /W 1 /H 1 ID \x00Q q\x01 EI q",
            &mut b,
        );
        assert_eq!(b, SaveBalance { under: 0, open: 2 });
        count_saves(b"Q Q Q", &mut b);
        assert_eq!(b, SaveBalance { under: 1, open: 0 });
        let mut hex = SaveBalance::default();
        count_saves(b"<51> Tj <</Q 1>> BDC q", &mut hex);
        assert_eq!(hex.open, 1);
    }

    /// A one-page PDF built by hand: inherited attributes on the page
    /// tree, an outline and a title, and content that leaves a flip open.
    fn tricky_pdf(dir: &Path) -> std::path::PathBuf {
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let content = doc.add_object(Stream::new(
            Dictionary::new(),
            b"1 0 0 -1 0 792 cm q 0 0 10 10 re f".to_vec(),
        ));
        let gs = doc.add_object(dictionary! { "Type" => "ExtGState", "ca" => 0.5 });
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page.into()],
                "Count" => 1,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Rotate" => Object::Real(90.0),
                "Resources" => dictionary! { "ExtGState" => dictionary! { "VeerGS" => gs } },
            }),
        );
        let outlines = doc.add_object(dictionary! { "Type" => "Outlines", "Count" => 0 });
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "Outlines" => outlines,
        });
        doc.trailer.set("Root", catalog);
        let info = doc.add_object(dictionary! {
            "Title" => Object::string_literal("Annual report"),
        });
        doc.trailer.set("Info", info);
        let path = dir.join("tricky.pdf");
        doc.save(&path).unwrap();
        path
    }

    #[test]
    fn stamps_keep_the_document_and_respect_its_quirks() {
        let dir = tempfile::tempdir().unwrap();
        let src = tricky_pdf(dir.path());
        let doc = Document::load(&src).unwrap();
        let page = *doc.get_pages().values().next().unwrap();
        // Inherited media box and a real-valued rotation.
        let v = page_view(&doc, page).unwrap();
        assert_eq!((v.width, v.height), (792.0, 612.0));

        let out = dir.path().join("marked.pdf");
        let opts = WatermarkOptions {
            text: "DRAFT".into(),
            opacity: 0.2,
            size: None,
            diagonal: true,
        };
        watermark(&src, &opts, &out).unwrap();
        let twice = dir.path().join("twice.pdf");
        watermark(&out, &opts, &twice).unwrap();

        let doc = Document::load(&twice).unwrap();
        // Title and outline survive.
        let info = doc.trailer.get(b"Info").unwrap().as_reference().unwrap();
        let title = doc.get_dictionary(info).unwrap().get(b"Title").unwrap();
        assert_eq!(title.as_str().unwrap(), b"Annual report");
        assert!(doc.catalog().unwrap().has(b"Outlines"));
        // Each watermark has its own state; the page's own isn't replaced.
        let page = *doc.get_pages().values().next().unwrap();
        let res = owned_dict(&doc, page_attr(&doc, page, b"Resources").as_ref());
        let states = owned_dict(&doc, res.get(b"ExtGState").ok());
        assert_eq!(states.len(), 3, "{states:?}");
        // The page leaves one save open, so the first wrapper closes two;
        // after that the content is balanced and the second closes one.
        let q_count = |doc: &Document, at: usize| {
            let page = *doc.get_pages().values().next().unwrap();
            let contents = content_refs(doc, doc.get_dictionary(page).unwrap());
            let id = contents[at].as_reference().unwrap();
            let data = doc
                .get_object(id)
                .unwrap()
                .as_stream()
                .unwrap()
                .get_plain_content()
                .unwrap();
            data.iter().filter(|&&c| c == b'Q').count()
        };
        let once = Document::load(&out).unwrap();
        assert_eq!(q_count(&once, 2), 2);
        assert_eq!(q_count(&doc, 5), 1);
        // The rotation was kept, and turning everything adds to it.
        let turned = dir.path().join("turned.pdf");
        rotate_all(&twice, 90, &turned).unwrap();
        assert_eq!(rotations(&turned), [180]);
        let doc = Document::load(&turned).unwrap();
        assert!(doc.catalog().unwrap().has(b"Outlines"));
    }
}
