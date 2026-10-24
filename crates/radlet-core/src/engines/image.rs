//! Raster decoding, encoding and pixel operations.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::codecs::tiff::TiffEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
};

use crate::catalog::Capabilities;
use crate::engines::{meta, sips};
use crate::error::{Error, Result};
use crate::format::Format;
use crate::ops::{Rect, RedactStyle};

/// Decoded pixels plus the colour profile, which is carried to the output.
pub struct Picture {
    pub img: DynamicImage,
    pub icc: Option<Vec<u8>>,
}

/// Largest image Radlet will decode: 40,000 px a side and 1 GiB of pixels.
pub fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(40_000);
    l.max_image_height = Some(40_000);
    l.max_alloc = Some(1 << 30);
    l
}

pub fn image_format(f: Format) -> Option<ImageFormat> {
    Some(match f {
        Format::Png => ImageFormat::Png,
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Webp => ImageFormat::WebP,
        Format::Gif => ImageFormat::Gif,
        Format::Bmp => ImageFormat::Bmp,
        Format::Tiff => ImageFormat::Tiff,
        Format::Ico => ImageFormat::Ico,
        Format::Tga => ImageFormat::Tga,
        Format::Qoi => ImageFormat::Qoi,
        _ => return None,
    })
}

/// Decodes an image and applies its EXIF orientation, so outputs are
/// upright even though EXIF isn't copied across.
pub fn decode(path: &Path, format: Format, caps: &Capabilities) -> Result<Picture> {
    if let Some(fmt) = image_format(format) {
        let mut reader = ImageReader::open(path)?;
        reader.set_format(fmt);
        reader.limits(limits());
        let mut decoder = reader.into_decoder()?;
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        let icc = decoder.icc_profile().ok().flatten();
        let mut img = DynamicImage::from_decoder(decoder)?;
        img.apply_orientation(orientation);
        return Ok(Picture { img, icc });
    }
    if format == Format::Svg {
        return crate::engines::svg::decode(path);
    }
    if matches!(format, Format::Heic | Format::Avif) {
        let scratch = tempfile::tempdir()?;
        let png = scratch.path().join("decoded.png");
        sips::convert(caps, path, &png, "png", None)?;
        return decode(&png, Format::Png, caps);
    }
    Err(Error::UnsupportedInput(format!(
        "{format} isn't an image Radlet can open"
    )))
}

/// Reads only the EXIF orientation of a JPEG, without decoding pixels.
pub fn jpeg_orientation(path: &Path) -> u8 {
    let read = || -> Option<u8> {
        let mut reader = ImageReader::open(path).ok()?;
        reader.set_format(ImageFormat::Jpeg);
        let mut decoder = reader.into_decoder().ok()?;
        decoder.orientation().ok().map(Orientation::to_exif)
    };
    read().unwrap_or(1)
}

/// Encoding settings for one output.
#[derive(Debug, Clone, Copy)]
pub struct EncodeOptions {
    /// 1-100 for lossy formats. 100 makes WebP lossless.
    pub quality: Option<u8>,
    /// Spend extra time for smaller lossless files (PNG).
    pub smallest: bool,
}

pub const DEFAULT_JPEG_QUALITY: u8 = 88;
pub const DEFAULT_WEBP_QUALITY: u8 = 82;
pub const DEFAULT_HEIC_QUALITY: u8 = 80;

/// Encodes pixels into `format` at `out`.
pub fn encode(
    pic: &Picture,
    format: Format,
    opts: EncodeOptions,
    out: &Path,
    caps: &Capabilities,
) -> Result<()> {
    let icc = pic.icc.as_deref();
    let img = &pic.img;
    let file = || -> Result<BufWriter<File>> { Ok(BufWriter::new(File::create(out)?)) };
    match format {
        Format::Jpeg => {
            let q = opts.quality.unwrap_or(DEFAULT_JPEG_QUALITY).clamp(1, 100);
            let mut w = file()?;
            let mut enc = JpegEncoder::new_with_quality(&mut w, q);
            if let Some(icc) = icc {
                let _ = enc.set_icc_profile(icc.to_vec());
            }
            if img.color().has_color() {
                let rgb = flatten_rgb(img);
                enc.write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )?;
            } else {
                let l = flatten_luma(img);
                enc.write_image(l.as_raw(), l.width(), l.height(), ExtendedColorType::L8)?;
            }
            w.flush()?;
        }
        Format::Png => {
            let img = drop_opaque_alpha(normalize_for_png(img));
            let mut w = file()?;
            let compression = if opts.smallest {
                CompressionType::Best
            } else {
                CompressionType::Default
            };
            let mut enc = PngEncoder::new_with_quality(&mut w, compression, PngFilter::Adaptive);
            if let Some(icc) = icc {
                let _ = enc.set_icc_profile(icc.to_vec());
            }
            enc.write_image(
                img.as_bytes(),
                img.width(),
                img.height(),
                img.color().into(),
            )?;
            w.flush()?;
        }
        Format::Webp => {
            let (w, h) = (img.width(), img.height());
            if w > 16_383 || h > 16_383 {
                return Err(Error::TooLarge(
                    "WebP images can be at most 16,383 pixels a side".into(),
                ));
            }
            let q = opts.quality.unwrap_or(DEFAULT_WEBP_QUALITY).clamp(1, 100);
            let alpha = img.color().has_alpha() && !is_opaque(img);
            let bytes = if alpha {
                img.to_rgba8().into_raw()
            } else {
                img.to_rgb8().into_raw()
            };
            let enc = if alpha {
                webp::Encoder::from_rgba(&bytes, w, h)
            } else {
                webp::Encoder::from_rgb(&bytes, w, h)
            };
            let mem = enc
                .encode_simple(q >= 100, q as f32)
                .map_err(|e| Error::Invalid(format!("WebP encoding failed: {e:?}")))?;
            let data = match icc {
                Some(icc) => meta::webp_add_icc(&mem, icc, w, h, alpha)?,
                None => mem.to_vec(),
            };
            std::fs::write(out, data)?;
        }
        Format::Tiff => {
            let img = drop_opaque_alpha(normalize_8_or_16(img));
            let mut w = file()?;
            let mut enc = TiffEncoder::new(&mut w);
            if let Some(icc) = icc {
                let _ = enc.set_icc_profile(icc.to_vec());
            }
            enc.write_image(
                img.as_bytes(),
                img.width(),
                img.height(),
                img.color().into(),
            )?;
            w.flush()?;
        }
        Format::Gif | Format::Bmp | Format::Tga | Format::Qoi => {
            let fmt = image_format(format).expect("raster format");
            let keep_alpha = format == Format::Gif || (img.color().has_alpha() && !is_opaque(img));
            let img = if keep_alpha {
                DynamicImage::ImageRgba8(img.to_rgba8())
            } else {
                DynamicImage::ImageRgb8(img.to_rgb8())
            };
            let mut w = file()?;
            img.write_to(&mut w, fmt)?;
            w.flush()?;
        }
        Format::Ico => {
            // Icons are at most 256 x 256.
            let fitted = if img.width() > 256 || img.height() > 256 {
                img.resize(256, 256, FilterType::Lanczos3)
            } else {
                img.clone()
            };
            let mut w = file()?;
            DynamicImage::ImageRgba8(fitted.to_rgba8()).write_to(&mut w, ImageFormat::Ico)?;
            w.flush()?;
        }
        Format::Icns => std::fs::write(out, crate::engines::icons::icns(img)?)?,
        Format::Heic => {
            let scratch = tempfile::tempdir()?;
            let png = scratch.path().join("source.png");
            encode(
                pic,
                Format::Png,
                EncodeOptions {
                    quality: None,
                    smallest: false,
                },
                &png,
                caps,
            )?;
            let q = opts.quality.unwrap_or(DEFAULT_HEIC_QUALITY).clamp(1, 100);
            sips::convert(caps, &png, out, "heic", Some(q))?;
        }
        other => return Err(Error::Invalid(format!("{other} isn't an image format"))),
    }
    Ok(())
}

/// How to resize: exactly one field set. `max` and `percent` never
/// enlarge; `width` and `height` are exact and keep the aspect ratio.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResizeSpec {
    pub max: Option<u32>,
    pub percent: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Longest side an exact resize may produce.
const MAX_RESIZE_SIDE: u64 = 16_384;

pub fn resize(img: DynamicImage, spec: ResizeSpec) -> Result<DynamicImage> {
    let (w, h) = (img.width() as u64, img.height() as u64);
    let set = [spec.max, spec.percent, spec.width, spec.height]
        .iter()
        .filter(|v| v.is_some())
        .count();
    if set != 1 {
        return Err(Error::Invalid(
            "resize needs exactly one of: max, percent, width, height".into(),
        ));
    }
    let exact = |nw: u64, nh: u64| -> Result<DynamicImage> {
        let (nw, nh) = (nw.max(1), nh.max(1));
        if nw > MAX_RESIZE_SIDE || nh > MAX_RESIZE_SIDE {
            return Err(Error::TooLarge(format!(
                "images can be resized to at most {MAX_RESIZE_SIDE} pixels a side"
            )));
        }
        if (nw, nh) == (w, h) {
            return Ok(img.clone());
        }
        Ok(img.resize_exact(nw as u32, nh as u32, FilterType::Lanczos3))
    };
    match spec {
        ResizeSpec { max: Some(0), .. }
        | ResizeSpec { width: Some(0), .. }
        | ResizeSpec {
            height: Some(0), ..
        } => Err(Error::Invalid("sizes must be at least 1 pixel".into())),
        ResizeSpec {
            percent: Some(0), ..
        } => Err(Error::Invalid("percentage must be above 0".into())),
        ResizeSpec { max: Some(m), .. } => {
            if w.max(h) <= m as u64 {
                Ok(img)
            } else {
                Ok(img.resize(m, m, FilterType::Lanczos3))
            }
        }
        ResizeSpec {
            percent: Some(p), ..
        } => {
            if p > 100 {
                return Err(Error::NothingToDo(
                    "pictures aren't enlarged (only SVGs are drawn larger)".into(),
                ));
            }
            if p == 100 {
                return Ok(img);
            }
            exact(w * p as u64 / 100, h * p as u64 / 100)
        }
        ResizeSpec {
            width: Some(nw), ..
        } => exact(nw as u64, (h * nw as u64 + w / 2) / w),
        ResizeSpec {
            height: Some(nh), ..
        } => exact((w * nh as u64 + h / 2) / h, nh as u64),
        _ => unreachable!("exactly one field is set"),
    }
}

/// Keeps only `rect` (fractions of the image).
pub fn crop(img: DynamicImage, rect: Rect) -> Result<DynamicImage> {
    if !rect.is_valid() {
        return Err(Error::Invalid(
            "the crop area must lie inside the image".into(),
        ));
    }
    let (x, y, w, h) = rect.to_pixels(img.width(), img.height());
    if (x, y, w, h) == (0, 0, img.width(), img.height()) {
        return Err(Error::NothingToDo(
            "the crop area is the whole image".into(),
        ));
    }
    Ok(img.crop_imm(x, y, w, h))
}

/// Covers `rects` (fractions of the image). Works on 8-bit RGBA, so the
/// output never contains the original pixels of a covered area.
pub fn redact(img: DynamicImage, rects: &[Rect], style: RedactStyle) -> Result<DynamicImage> {
    if rects.is_empty() {
        return Err(Error::Invalid("draw at least one area to redact".into()));
    }
    if let Some(bad) = rects.iter().find(|r| !r.is_valid()) {
        return Err(Error::Invalid(format!(
            "redaction area {bad:?} lies outside the image"
        )));
    }
    let had_alpha = img.color().has_alpha();
    let mut rgba = img.to_rgba8();
    let (iw, ih) = rgba.dimensions();
    for r in rects {
        let (x0, y0, w, h) = r.to_pixels(iw, ih);
        match style {
            RedactStyle::Black => {
                for y in y0..y0 + h {
                    for x in x0..x0 + w {
                        rgba.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                    }
                }
            }
            RedactStyle::Pixelate => {
                // Blocks of about a tenth of the area's short side, at
                // least 12 px, so text can't be read back.
                let block = (w.min(h) / 10).max(12);
                pixelate(&mut rgba, x0, y0, w, h, block);
            }
            RedactStyle::Blur => {
                let region = image::imageops::crop_imm(&rgba, x0, y0, w, h).to_image();
                // Pixelate first so no detail survives, then blur the blocks.
                let mut region = region;
                pixelate(&mut region, 0, 0, w, h, (w.min(h) / 16).max(8));
                let sigma = (w.min(h) as f32 / 8.0).clamp(6.0, 60.0);
                let blurred = image::imageops::blur(&region, sigma);
                image::imageops::replace(&mut rgba, &blurred, x0 as i64, y0 as i64);
            }
        }
    }
    let out = DynamicImage::ImageRgba8(rgba);
    Ok(if had_alpha {
        out
    } else {
        DynamicImage::ImageRgb8(out.to_rgb8())
    })
}

/// Writes 8-bit pixels as a quickly compressed PNG, for handing to a
/// helper program.
pub fn save_png_fast(img: &DynamicImage, out: &Path) -> Result<()> {
    let img = if img.color().has_alpha() {
        DynamicImage::ImageRgba8(img.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(img.to_rgb8())
    };
    let mut w = BufWriter::new(File::create(out)?);
    PngEncoder::new_with_quality(&mut w, CompressionType::Fast, PngFilter::Adaptive).write_image(
        img.as_bytes(),
        img.width(),
        img.height(),
        img.color().into(),
    )?;
    w.flush()?;
    Ok(())
}

/// Uses a grey mask (white = keep) as transparency. Pixels that end up
/// (almost) invisible are cleared, so the removed background can't be
/// recovered from the file. With `crop`, trims to what's left.
pub fn apply_mask(img: &DynamicImage, mask: &image::GrayImage, crop: bool) -> Result<DynamicImage> {
    let (w, h) = (img.width(), img.height());
    let mask = if mask.dimensions() == (w, h) {
        mask.clone()
    } else {
        image::imageops::resize(mask, w, h, FilterType::Triangle)
    };
    let mut rgba = img.to_rgba8();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (x, y, p) in rgba.enumerate_pixels_mut() {
        let keep = mask.get_pixel(x, y).0[0] as u32;
        let a = (p.0[3] as u32 * keep + 127) / 255;
        if a <= 8 {
            p.0 = [0, 0, 0, 0];
        } else {
            p.0[3] = a as u8;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x1 < x0 || y1 < y0 {
        return Err(Error::NothingToDo("no subject found".into()));
    }
    let out = if crop {
        image::imageops::crop_imm(&rgba, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image()
    } else {
        rgba
    };
    Ok(DynamicImage::ImageRgba8(out))
}

fn pixelate(img: &mut image::RgbaImage, x0: u32, y0: u32, w: u32, h: u32, block: u32) {
    let mut by = y0;
    while by < y0 + h {
        let bh = block.min(y0 + h - by);
        let mut bx = x0;
        while bx < x0 + w {
            let bw = block.min(x0 + w - bx);
            let mut sum = [0u64; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = img.get_pixel(x, y).0;
                    for c in 0..4 {
                        sum[c] += p[c] as u64;
                    }
                }
            }
            let n = (bw * bh) as u64;
            let avg = image::Rgba([
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                (sum[3] / n) as u8,
            ]);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    img.put_pixel(x, y, avg);
                }
            }
            bx += bw;
        }
        by += bh;
    }
}

/// Encodes into `out`, aiming for at most `target` bytes: searches quality
/// for lossy formats, and shrinks the image when quality alone isn't
/// enough (the only option for lossless formats).
pub fn encode_to_target(
    pic: &Picture,
    format: Format,
    target: u64,
    out: &Path,
    caps: &Capabilities,
) -> Result<()> {
    let lossy = matches!(format, Format::Jpeg | Format::Webp | Format::Heic);
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX);
    let mut current = Picture {
        img: pic.img.clone(),
        icc: pic.icc.clone(),
    };
    let mut smallest_seen = u64::MAX;
    for _ in 0..10 {
        if lossy {
            // Highest quality in 20..=92 that fits.
            let (mut lo, mut hi, mut best) = (20u8, 92u8, None);
            while lo <= hi {
                let q = lo + (hi - lo) / 2;
                encode(
                    &current,
                    format,
                    EncodeOptions {
                        quality: Some(q),
                        smallest: true,
                    },
                    out,
                    caps,
                )?;
                let s = size(out);
                smallest_seen = smallest_seen.min(s);
                if s <= target {
                    best = Some(q);
                    lo = q + 1;
                } else if q == 0 {
                    break;
                } else {
                    hi = q - 1;
                }
            }
            if let Some(q) = best {
                // The last attempt may not have been the best fit.
                encode(
                    &current,
                    format,
                    EncodeOptions {
                        quality: Some(q),
                        smallest: true,
                    },
                    out,
                    caps,
                )?;
                return Ok(());
            }
        } else {
            encode(
                &current,
                format,
                EncodeOptions {
                    quality: None,
                    smallest: true,
                },
                out,
                caps,
            )?;
            let s = size(out);
            smallest_seen = smallest_seen.min(s);
            if s <= target {
                return Ok(());
            }
        }
        let (w, h) = (current.img.width(), current.img.height());
        if w <= 32 || h <= 32 {
            break;
        }
        let (nw, nh) = ((w as f64 * 0.8) as u32, (h as f64 * 0.8) as u32);
        current.img = current
            .img
            .resize_exact(nw.max(1), nh.max(1), FilterType::Lanczos3);
    }
    Err(Error::NothingToDo(format!(
        "couldn't get below {} (smallest was {})",
        human(target),
        human(smallest_seen)
    )))
}

/// Mean SSIM a "Smart" result must reach against the original, and the
/// share of the picture (its worst 1%) that may fall below `SMART_LOW`.
/// Calibrated on photos, scans and screenshots: at these levels JPEG
/// lands near quality 80-90 for photos, where artifacts are hard to see.
const SMART_MEAN: f64 = 0.98;
const SMART_LOW: f64 = 0.88;

/// Structural similarity (SSIM) of two greyscale pictures of the same
/// size, over 8 x 8 windows: the mean, and the 1st percentile (how the
/// worst-looking parts fare, such as text and sharp edges).
pub(crate) fn ssim(a: &image::GrayImage, b: &image::GrayImage) -> (f64, f64) {
    assert_eq!(a.dimensions(), b.dimensions(), "SSIM compares equal sizes");
    let (w, h) = a.dimensions();
    if w < 8 || h < 8 {
        // Too small for windows: compare pixel by pixel.
        let same = a.as_raw() == b.as_raw();
        return if same { (1.0, 1.0) } else { (0.0, 0.0) };
    }
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    // Overlapping windows on all but huge pictures.
    let stride = if (w as u64) * (h as u64) > 24_000_000 {
        8
    } else {
        4
    };
    let (pa, pb) = (a.as_raw(), b.as_raw());
    let mut sum = 0.0;
    let mut count = 0u64;
    // A histogram of window scores, for the percentile without storing them.
    let mut bins = vec![0u64; 2001];
    let mut y = 0;
    while y + 8 <= h {
        let mut x = 0;
        while x + 8 <= w {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for dy in 0..8 {
                let row = ((y + dy) * w + x) as usize;
                for i in row..row + 8 {
                    let (va, vb) = (pa[i] as u64, pb[i] as u64);
                    sa += va;
                    sb += vb;
                    saa += va * va;
                    sbb += vb * vb;
                    sab += va * vb;
                }
            }
            let n = 64.0;
            let (ma, mb) = (sa as f64 / n, sb as f64 / n);
            let va = saa as f64 / n - ma * ma;
            let vb = sbb as f64 / n - mb * mb;
            let cov = sab as f64 / n - ma * mb;
            let score = ((2.0 * ma * mb + C1) * (2.0 * cov + C2))
                / ((ma * ma + mb * mb + C1) * (va + vb + C2));
            sum += score;
            count += 1;
            bins[((score.clamp(-1.0, 1.0) + 1.0) * 1000.0).round() as usize] += 1;
            x += stride;
        }
        y += stride;
    }
    let mean = sum / count as f64;
    let mut seen = 0u64;
    let wanted = (count as f64 * 0.01).ceil() as u64;
    let mut low = 1.0;
    for (i, n) in bins.iter().enumerate() {
        seen += n;
        if seen >= wanted.max(1) {
            low = i as f64 / 1000.0 - 1.0;
            break;
        }
    }
    (mean, low)
}

/// Colour (Cb and Cr of YCbCr) at half size, flattened over white. JPEG
/// and WebP store colour at half size too, so this is where they lose it.
pub(crate) fn chroma(img: &DynamicImage) -> (image::GrayImage, image::GrayImage) {
    let rgb = flatten_rgb(img);
    let (w, h) = ((rgb.width() / 2).max(1), (rgb.height() / 2).max(1));
    let mut cb = image::GrayImage::new(w, h);
    let mut cr = image::GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (mut r, mut g, mut b, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (px, py) = (2 * x + dx, 2 * y + dy);
                if px < rgb.width() && py < rgb.height() {
                    let p = rgb.get_pixel(px, py).0;
                    r += p[0] as f32;
                    g += p[1] as f32;
                    b += p[2] as f32;
                    n += 1.0;
                }
            }
            let (r, g, b) = (r / n, g / n, b / n);
            let u = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
            let v = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
            cb.put_pixel(x, y, image::Luma([u.round().clamp(0.0, 255.0) as u8]));
            cr.put_pixel(x, y, image::Luma([v.round().clamp(0.0, 255.0) as u8]));
        }
    }
    (cb, cr)
}

/// SSIM a Smart result's colour must keep (mean, and its worst 1%), so
/// coloured text and edges as bright as their background stay clean.
/// JPEG keeps colour well; WebP is the one this holds back.
const SMART_CHROMA_MEAN: f64 = 0.95;
const SMART_CHROMA_LOW: f64 = 0.85;

/// Encodes JPEG or WebP at the lowest quality whose result still looks
/// the same as `pic` (by SSIM of brightness and colour), and returns that
/// quality.
pub fn encode_looking_the_same(
    pic: &Picture,
    format: Format,
    out: &Path,
    caps: &Capabilities,
) -> Result<u8> {
    if !matches!(format, Format::Jpeg | Format::Webp) {
        return Err(Error::Invalid(
            "Smart quality works for JPEG and WebP".into(),
        ));
    }
    let reference = flatten_luma(&pic.img);
    let (ref_cb, ref_cr) = chroma(&pic.img);
    let looks_same = |q: u8| -> Result<bool> {
        encode(
            pic,
            format,
            EncodeOptions {
                quality: Some(q),
                smallest: true,
            },
            out,
            caps,
        )?;
        let back = image::open(out)?;
        if (back.width(), back.height()) != (pic.img.width(), pic.img.height()) {
            return Ok(false);
        }
        let (mean, low) = ssim(&reference, &flatten_luma(&back));
        if mean < SMART_MEAN || low < SMART_LOW {
            return Ok(false);
        }
        let (cb, cr) = chroma(&back);
        let ok = |(mean, low): (f64, f64)| mean >= SMART_CHROMA_MEAN && low >= SMART_CHROMA_LOW;
        Ok(ok(ssim(&ref_cb, &cb)) && ok(ssim(&ref_cr, &cr)))
    };
    // Lowest quality in 40..=92 that passes; quality rarely needs more.
    let (mut lo, mut hi, mut best) = (40u8, 92u8, None);
    while lo <= hi {
        let q = lo + (hi - lo) / 2;
        if looks_same(q)? {
            best = Some(q);
            hi = q - 1;
        } else {
            lo = q + 1;
        }
    }
    let q = best.unwrap_or(95);
    encode(
        pic,
        format,
        EncodeOptions {
            quality: Some(q),
            smallest: true,
        },
        out,
        caps,
    )?;
    Ok(q)
}

pub(crate) fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b if b >= 1_000 => format!("{:.0} KB", b as f64 / 1e3),
        b => format!("{b} B"),
    }
}

pub fn rotate(img: DynamicImage, degrees: u32) -> Result<DynamicImage> {
    match degrees % 360 {
        90 => Ok(img.rotate90()),
        180 => Ok(img.rotate180()),
        270 => Ok(img.rotate270()),
        _ => Err(Error::Invalid(
            "rotation must be 90, 180 or 270 degrees".into(),
        )),
    }
}

fn is_opaque(img: &DynamicImage) -> bool {
    match img {
        DynamicImage::ImageRgba8(b) => b.pixels().all(|p| p.0[3] == 255),
        DynamicImage::ImageLumaA8(b) => b.pixels().all(|p| p.0[1] == 255),
        DynamicImage::ImageRgba16(b) => b.pixels().all(|p| p.0[3] == u16::MAX),
        DynamicImage::ImageLumaA16(b) => b.pixels().all(|p| p.0[1] == u16::MAX),
        DynamicImage::ImageRgba32F(b) => b.pixels().all(|p| p.0[3] >= 1.0),
        _ => !img.color().has_alpha(),
    }
}

/// Drops an alpha channel that is fully opaque, for smaller lossless files.
fn drop_opaque_alpha(img: DynamicImage) -> DynamicImage {
    if !img.color().has_alpha() || !is_opaque(&img) {
        return img;
    }
    match img {
        DynamicImage::ImageRgba8(_) => DynamicImage::ImageRgb8(img.to_rgb8()),
        DynamicImage::ImageLumaA8(_) => DynamicImage::ImageLuma8(img.to_luma8()),
        DynamicImage::ImageRgba16(_) => DynamicImage::ImageRgb16(img.to_rgb16()),
        DynamicImage::ImageLumaA16(_) => DynamicImage::ImageLuma16(img.to_luma16()),
        other => other,
    }
}

fn normalize_for_png(img: &DynamicImage) -> DynamicImage {
    match img {
        DynamicImage::ImageRgb32F(_) => DynamicImage::ImageRgb16(img.to_rgb16()),
        DynamicImage::ImageRgba32F(_) => DynamicImage::ImageRgba16(img.to_rgba16()),
        other => other.clone(),
    }
}

fn normalize_8_or_16(img: &DynamicImage) -> DynamicImage {
    match img {
        // The TIFF encoder has no grey-with-alpha mode.
        DynamicImage::ImageLumaA8(_) => DynamicImage::ImageRgba8(img.to_rgba8()),
        DynamicImage::ImageLumaA16(_) => DynamicImage::ImageRgba16(img.to_rgba16()),
        DynamicImage::ImageRgb32F(_) => DynamicImage::ImageRgb16(img.to_rgb16()),
        DynamicImage::ImageRgba32F(_) => DynamicImage::ImageRgba16(img.to_rgba16()),
        other => other.clone(),
    }
}

/// Composites transparent pixels onto white for formats without alpha.
fn flatten_rgb(img: &DynamicImage) -> image::RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u32;
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])])
    })
}

fn flatten_luma(img: &DynamicImage) -> image::GrayImage {
    if !img.color().has_alpha() {
        return img.to_luma8();
    }
    let la = img.to_luma_alpha8();
    image::GrayImage::from_fn(la.width(), la.height(), |x, y| {
        let p = la.get_pixel(x, y).0;
        let a = p[1] as u32;
        image::Luma([((p[0] as u32 * a + 255 * (255 - a) + 127) / 255) as u8])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with smooth areas, fine texture and sharp edges, like a
    /// photo with some text in it.
    fn photo_like(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32, y as f32);
            let base = fx / w as f32 * 200.0 + 20.0;
            let tex = (fx / 5.0).sin() * (fy / 7.0).cos() * 18.0;
            let edge = if (y / 6) % 9 == 0 && (x / 3) % 4 != 0 && y > h / 2 {
                -120.0
            } else {
                0.0
            };
            let v = |off: f32| (base + off + tex + edge).clamp(0.0, 255.0) as u8;
            image::Rgb([v(0.0), v(fy / h as f32 * 80.0 - 40.0), v(30.0)])
        }))
    }

    #[test]
    #[ignore]
    fn ssim_curve() {
        let dir = tempfile::tempdir().unwrap();
        let caps = Capabilities::none();
        let files = std::env::var("RADLET_SSIM_FILES").unwrap_or_default();
        for f in files.split(':').filter(|f| !f.is_empty()) {
            let pic = Picture {
                img: image::open(f).unwrap(),
                icc: None,
            };
            let r = flatten_luma(&pic.img);
            let orig = std::fs::metadata(f).unwrap().len();
            eprintln!(
                "{f} ({}x{}, {orig} bytes)",
                pic.img.width(),
                pic.img.height()
            );
            for (fmt, ext) in [(Format::Jpeg, "jpg"), (Format::Webp, "webp")] {
                for q in [40u8, 50, 60, 70, 75, 80, 85, 90, 95] {
                    let out = dir.path().join(format!("x.{ext}"));
                    encode(
                        &pic,
                        fmt,
                        EncodeOptions {
                            quality: Some(q),
                            smallest: true,
                        },
                        &out,
                        &caps,
                    )
                    .unwrap();
                    let back = image::open(&out).unwrap();
                    let (m, l) = ssim(&r, &flatten_luma(&back));
                    let (rcb, rcr) = chroma(&pic.img);
                    let (cb, cr) = chroma(&back);
                    let (b, r2) = (ssim(&rcb, &cb), ssim(&rcr, &cr));
                    eprintln!(
                        "  {ext} q{q}: mean {m:.4} low {l:.3} cb {:.4}/{:.3} cr {:.4}/{:.3} size {}",
                        b.0,
                        b.1,
                        r2.0,
                        r2.1,
                        std::fs::metadata(&out).unwrap().len()
                    );
                }
            }
        }
    }

    #[test]
    fn ssim_scores_sameness() {
        let a = photo_like(160, 120).to_luma8();
        assert_eq!(ssim(&a, &a), (1.0, 1.0));
        let mut b = a.clone();
        for p in b.pixels_mut().step_by(3) {
            p.0[0] = p.0[0].wrapping_add(90);
        }
        let (mean, low) = ssim(&a, &b);
        assert!(mean < 0.9 && low <= mean, "{mean} {low}");
    }

    #[test]
    fn smart_quality_is_smaller_but_looks_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let caps = Capabilities::none();
        let pic = Picture {
            img: photo_like(640, 480),
            icc: None,
        };
        for format in [Format::Jpeg, Format::Webp] {
            let smart = dir.path().join(format!("smart.{}", format.extension()));
            let q = encode_looking_the_same(&pic, format, &smart, &caps).unwrap();
            assert!((40..=95).contains(&q), "{format}: {q}");
            let usual = dir.path().join(format!("usual.{}", format.extension()));
            encode(
                &pic,
                format,
                EncodeOptions {
                    quality: Some(95),
                    smallest: true,
                },
                &usual,
                &caps,
            )
            .unwrap();
            let size = |p: &Path| std::fs::metadata(p).unwrap().len();
            assert!(
                size(&smart) < size(&usual),
                "{format}: {} vs {}",
                size(&smart),
                size(&usual)
            );
            let (mean, low) = ssim(
                &pic.img.to_luma8(),
                &image::open(&smart).unwrap().to_luma8(),
            );
            assert!(
                mean >= SMART_MEAN && low >= SMART_LOW,
                "{format}: {mean} {low}"
            );
        }
        // A flat picture needs very little.
        let flat = Picture {
            img: DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                200,
                200,
                image::Rgb([90, 140, 200]),
            )),
            icc: None,
        };
        let out = dir.path().join("flat.jpg");
        assert_eq!(
            encode_looking_the_same(&flat, Format::Jpeg, &out, &caps).unwrap(),
            40
        );
    }

    fn pic(img: DynamicImage) -> Picture {
        Picture { img, icc: None }
    }

    #[test]
    fn every_builtin_format_round_trips() {
        let caps = Capabilities::none();
        let src = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(40, 30, |x, y| {
            image::Rgba([
                (x * 6) as u8,
                (y * 8) as u8,
                128,
                if x < 20 { 255 } else { 128 },
            ])
        }));
        let dir = tempfile::tempdir().unwrap();
        for f in [
            Format::Png,
            Format::Jpeg,
            Format::Webp,
            Format::Gif,
            Format::Bmp,
            Format::Tiff,
            Format::Ico,
            Format::Tga,
            Format::Qoi,
        ] {
            let out = dir.path().join(format!("out.{}", f.extension()));
            encode(
                &pic(src.clone()),
                f,
                EncodeOptions {
                    quality: None,
                    smallest: false,
                },
                &out,
                &caps,
            )
            .unwrap();
            assert_eq!(
                crate::format::detect(&out).unwrap(),
                f,
                "{f} detected after encode"
            );
            let back = decode(&out, f, &caps).unwrap();
            assert_eq!(
                (back.img.width(), back.img.height()),
                (40, 30),
                "{f} dimensions"
            );
        }
    }

    #[test]
    fn grey_alpha_converts_everywhere() {
        let caps = Capabilities::none();
        let la = DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_pixel(
            6,
            6,
            image::LumaA([90, 128]),
        ));
        let dir = tempfile::tempdir().unwrap();
        for f in [
            Format::Png,
            Format::Jpeg,
            Format::Webp,
            Format::Tiff,
            Format::Bmp,
            Format::Gif,
            Format::Tga,
            Format::Qoi,
            Format::Ico,
        ] {
            let out = dir.path().join(format!("la.{}", f.extension()));
            encode(
                &pic(la.clone()),
                f,
                EncodeOptions {
                    quality: None,
                    smallest: false,
                },
                &out,
                &caps,
            )
            .unwrap_or_else(|e| panic!("{f}: {e}"));
        }
    }

    #[test]
    fn jpeg_flattens_onto_white() {
        let transparent = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([0, 0, 0, 0]),
        ));
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("t.jpg");
        encode(
            &pic(transparent),
            Format::Jpeg,
            EncodeOptions {
                quality: Some(95),
                smallest: false,
            },
            &out,
            &Capabilities::none(),
        )
        .unwrap();
        let back = image::open(&out).unwrap().to_rgb8();
        assert!(back.get_pixel(4, 4).0.iter().all(|&c| c > 245));
    }

    #[test]
    fn icc_profile_survives_png_and_webp() {
        let caps = Capabilities::none();
        let p = Picture {
            img: DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                4,
                4,
                image::Rgb([200, 10, 10]),
            )),
            icc: Some(b"not-a-real-icc-but-carried".to_vec()),
        };
        let dir = tempfile::tempdir().unwrap();
        for f in [Format::Png, Format::Webp, Format::Jpeg] {
            let out = dir.path().join(format!("c.{}", f.extension()));
            encode(
                &p,
                f,
                EncodeOptions {
                    quality: None,
                    smallest: false,
                },
                &out,
                &caps,
            )
            .unwrap();
            let back = decode(&out, f, &caps).unwrap();
            assert_eq!(
                back.icc.as_deref(),
                Some(&b"not-a-real-icc-but-carried"[..]),
                "{f}"
            );
        }
    }

    #[test]
    fn resize_rules() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(400, 200));
        let spec = |max, percent, width, height| ResizeSpec {
            max,
            percent,
            width,
            height,
        };
        let r = resize(img.clone(), spec(Some(100), None, None, None)).unwrap();
        assert_eq!((r.width(), r.height()), (100, 50));
        let r = resize(img.clone(), spec(Some(1000), None, None, None)).unwrap();
        assert_eq!((r.width(), r.height()), (400, 200), "max never enlarges");
        let r = resize(img.clone(), spec(None, Some(25), None, None)).unwrap();
        assert_eq!((r.width(), r.height()), (100, 50));
        let r = resize(img.clone(), spec(None, None, Some(800), None)).unwrap();
        assert_eq!(
            (r.width(), r.height()),
            (800, 400),
            "exact width may enlarge"
        );
        let r = resize(img.clone(), spec(None, None, None, Some(50))).unwrap();
        assert_eq!((r.width(), r.height()), (100, 50));
        assert!(resize(img.clone(), spec(None, None, None, None)).is_err());
        assert!(resize(img.clone(), spec(Some(10), Some(10), None, None)).is_err());
        assert!(resize(img.clone(), spec(None, None, Some(0), None)).is_err());
        assert!(resize(img, spec(None, None, Some(20_000), None)).is_err());
    }

    #[test]
    fn crop_by_fractions() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(400, 200, |x, _| {
            image::Rgb([(x / 2) as u8, 0, 0])
        }));
        let c = crop(
            img.clone(),
            Rect {
                x: 0.5,
                y: 0.25,
                w: 0.5,
                h: 0.5,
            },
        )
        .unwrap();
        assert_eq!((c.width(), c.height()), (200, 100));
        assert_eq!(c.to_rgb8().get_pixel(0, 0).0[0], 100, "starts at x = 200");
        assert!(matches!(
            crop(img.clone(), Rect::FULL),
            Err(Error::NothingToDo(_))
        ));
        assert!(crop(
            img,
            Rect {
                x: 0.9,
                y: 0.0,
                w: 0.5,
                h: 1.0
            }
        )
        .is_err());
    }

    #[test]
    fn redaction_removes_original_pixels() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(200, 100, |x, y| {
            image::Rgb([
                (x * 7 % 256) as u8,
                (y * 13 % 256) as u8,
                ((x + y) % 256) as u8,
            ])
        }));
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        };
        let black = redact(img.clone(), &[area], RedactStyle::Black)
            .unwrap()
            .to_rgb8();
        assert!((0..100).all(|x| (0..100).all(|y| black.get_pixel(x, y).0 == [0, 0, 0])));
        assert_eq!(
            black.get_pixel(150, 50),
            img.to_rgb8().get_pixel(150, 50),
            "outside untouched"
        );
        assert_eq!(black.dimensions(), (200, 100));

        for style in [RedactStyle::Pixelate, RedactStyle::Blur] {
            let out = redact(img.clone(), &[area], style).unwrap().to_rgb8();
            let original = img.to_rgb8();
            let same = (0..100)
                .flat_map(|x| (0..100).map(move |y| (x, y)))
                .filter(|&(x, y)| out.get_pixel(x, y) == original.get_pixel(x, y))
                .count();
            assert!(same < 300, "{style:?} left {same} original pixels");
        }
        assert!(redact(img.clone(), &[], RedactStyle::Black).is_err());
        assert!(redact(
            img,
            &[Rect {
                x: 0.8,
                y: 0.0,
                w: 0.5,
                h: 0.5
            }],
            RedactStyle::Black
        )
        .is_err());
    }

    #[test]
    fn target_size_is_met() {
        let caps = Capabilities::none();
        let noisy = DynamicImage::ImageRgb8(image::RgbImage::from_fn(800, 600, |x, y| {
            let v = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) as u8;
            image::Rgb([v, v.wrapping_mul(3), v.wrapping_add(x as u8)])
        }));
        let dir = tempfile::tempdir().unwrap();
        for (f, target) in [
            (Format::Jpeg, 60_000u64),
            (Format::Webp, 60_000),
            (Format::Png, 200_000),
        ] {
            let out = dir.path().join(format!("t.{}", f.extension()));
            encode_to_target(&pic(noisy.clone()), f, target, &out, &caps).unwrap();
            let size = std::fs::metadata(&out).unwrap().len();
            assert!(size <= target, "{f}: {size} > {target}");
        }
        let out = dir.path().join("impossible.png");
        assert!(matches!(
            encode_to_target(&pic(noisy), Format::Png, 10, &out, &caps),
            Err(Error::NothingToDo(_))
        ));
    }

    #[test]
    fn rotate_rules() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(4, 2));
        assert_eq!(rotate(img.clone(), 90).unwrap().width(), 2);
        assert_eq!(rotate(img.clone(), 180).unwrap().width(), 4);
        assert!(rotate(img, 45).is_err());
    }
}
