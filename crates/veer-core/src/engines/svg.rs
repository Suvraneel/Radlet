//! SVG: drawn into pixels at any size (resvg), or turned into a vector
//! PDF page (svg2pdf).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use image::{DynamicImage, RgbaImage};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{fontdb, ImageHrefResolver, Options, Tree};

use crate::engines::image::{Picture, ResizeSpec};
use crate::error::{Error, Result};

/// Largest SVG file Veer reads.
const MAX_SVG_BYTES: u64 = 64 << 20;

/// Longest side a drawing may have, and the most memory its pixels may
/// take (the same bounds as decoded images).
const MAX_SIDE: u32 = 16_384;
const MAX_PIXEL_BYTES: u64 = 1 << 30;

/// An SVG converted without a size asked for is drawn with its longest
/// side at least this long: an icon that says it is 24 pixels wide is
/// still useful as a PNG.
pub const DEFAULT_LONGEST: u32 = 1024;

/// System fonts, for SVGs with text. Loaded once, on first use.
fn fonts() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

/// Pictures an SVG links to are loaded only from its own folder (or
/// embedded in it), never from elsewhere on the disk or the network.
fn options(path: &Path) -> Options<'static> {
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let root = std::fs::canonicalize(&dir).ok();
    let default = ImageHrefResolver::default_string_resolver();
    let resolve_string = Box::new(move |href: &str, opts: &Options| {
        let root = root.as_ref()?;
        let rel = Path::new(href);
        if rel.is_absolute() || href.contains("://") {
            return None;
        }
        let real = std::fs::canonicalize(root.join(rel)).ok()?;
        if !real.starts_with(root) || is_gzip_file(&real) {
            return None;
        }
        default(real.to_str()?, opts)
    });
    // Compressed drawings inside a drawing would unpack without a limit.
    let data_default = ImageHrefResolver::default_data_resolver();
    let resolve_data = Box::new(
        move |mime: &str, data: std::sync::Arc<Vec<u8>>, opts: &Options| {
            if data.starts_with(&[0x1F, 0x8B]) {
                return None;
            }
            data_default(mime, data, opts)
        },
    );
    Options {
        resources_dir: Some(dir),
        fontdb: fonts(),
        image_href_resolver: ImageHrefResolver {
            resolve_data,
            resolve_string,
        },
        ..Options::default()
    }
}

fn is_gzip_file(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|_| head == [0x1F, 0x8B])
}

/// Parses an SVG (or gzip-compressed SVGZ) file.
pub fn load(path: &Path) -> Result<Tree> {
    if std::fs::metadata(path)?.len() > MAX_SVG_BYTES {
        return Err(Error::TooLarge("SVG files can be at most 64 MB".into()));
    }
    let raw = std::fs::read(path)?;
    // SVGZ is unpacked here, with a limit, rather than by usvg without one.
    let data = if raw.starts_with(&[0x1F, 0x8B]) {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice())
            .take(MAX_SVG_BYTES + 1)
            .read_to_end(&mut out)
            .map_err(|e| Error::UnsupportedInput(format!("this SVGZ file is damaged ({e})")))?;
        if out.len() as u64 > MAX_SVG_BYTES {
            return Err(Error::TooLarge(
                "SVG files can be at most 64 MB unpacked".into(),
            ));
        }
        out
    } else {
        raw
    };
    Tree::from_data(&data, &options(path)).map_err(|e| {
        Error::UnsupportedInput(format!(
            "{} isn't an SVG Veer can draw ({e})",
            crate::format::display_name(path)
        ))
    })
}

/// The size the SVG gives itself, in pixels.
pub fn natural_size(tree: &Tree) -> (f32, f32) {
    let s = tree.size();
    (s.width(), s.height())
}

fn clamp_size(w: f64, h: f64) -> Result<(u32, u32)> {
    let (w, h) = (w.round().max(1.0), h.round().max(1.0));
    if w > MAX_SIDE as f64 || h > MAX_SIDE as f64 {
        return Err(Error::TooLarge(format!(
            "SVGs can be drawn at most {MAX_SIDE} pixels a side"
        )));
    }
    let (w, h) = (w as u32, h as u32);
    if w as u64 * h as u64 * 4 > MAX_PIXEL_BYTES {
        return Err(Error::TooLarge("that drawing would be too large".into()));
    }
    Ok((w, h))
}

/// Size for a conversion with no size asked for: the SVG's own, with the
/// longest side brought up to at least [`DEFAULT_LONGEST`].
pub fn default_size(natural: (f32, f32)) -> Result<(u32, u32)> {
    let (w, h) = (natural.0 as f64, natural.1 as f64);
    let longest = w.max(h).max(1.0);
    let scale = (DEFAULT_LONGEST as f64 / longest).max(1.0);
    clamp_size(w * scale, h * scale)
}

/// Size for a resize. A vector has no pixels to lose, so `max` sets the
/// longest side and `percent` may go above 100.
pub fn sized(natural: (f32, f32), spec: ResizeSpec) -> Result<(u32, u32)> {
    let (w, h) = (natural.0.max(1.0) as f64, natural.1.max(1.0) as f64);
    let set = [spec.max, spec.percent, spec.width, spec.height]
        .iter()
        .filter(|v| v.is_some())
        .count();
    if set != 1 {
        return Err(Error::Invalid(
            "resize needs exactly one of: max, percent, width, height".into(),
        ));
    }
    match spec {
        ResizeSpec {
            max: Some(0) | None,
            percent: Some(0) | None,
            width: Some(0) | None,
            height: Some(0) | None,
        } => Err(Error::Invalid("sizes must be above 0".into())),
        ResizeSpec { max: Some(m), .. } => {
            let s = m as f64 / w.max(h);
            clamp_size(w * s, h * s)
        }
        ResizeSpec {
            percent: Some(p), ..
        } => clamp_size(w * p as f64 / 100.0, h * p as f64 / 100.0),
        ResizeSpec {
            width: Some(nw), ..
        } => clamp_size(nw as f64, h * nw as f64 / w),
        ResizeSpec {
            height: Some(nh), ..
        } => clamp_size(w * nh as f64 / h, nh as f64),
    }
}

/// Draws the whole SVG into a `width` x `height` picture with
/// transparency.
pub fn render(tree: &Tree, width: u32, height: u32) -> Result<Picture> {
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| Error::TooLarge("that drawing would be too large".into()))?;
    let (nw, nh) = natural_size(tree);
    let transform =
        Transform::from_scale(width as f32 / nw.max(1e-3), height as f32 / nh.max(1e-3));
    resvg::render(tree, transform, &mut pixmap.as_mut());
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let img = RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| Error::Internal("SVG drawing has the wrong size".into()))?;
    Ok(Picture {
        img: DynamicImage::ImageRgba8(img),
        icc: None,
    })
}

/// Draws an SVG at its default conversion size.
pub fn decode(path: &Path) -> Result<Picture> {
    let tree = load(path)?;
    let (w, h) = default_size(natural_size(&tree))?;
    render(&tree, w, h)
}

/// Draws an SVG at the size a resize asks for.
pub fn decode_sized(path: &Path, spec: ResizeSpec) -> Result<Picture> {
    let tree = load(path)?;
    let (w, h) = sized(natural_size(&tree), spec)?;
    render(&tree, w, h)
}

/// A one-page PDF of the SVG that stays vector: lines and text stay sharp
/// at any zoom. The page is the SVG's own size (96 pixels to the inch).
pub fn to_pdf(path: &Path) -> Result<Vec<u8>> {
    let tree = load(path)?;
    svg2pdf::to_pdf(
        &tree,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions { dpi: 96.0 },
    )
    .map_err(|e| Error::Pdf(format!("couldn't turn the SVG into a PDF ({e})")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12" viewBox="0 0 24 12"><rect x="0" y="0" width="12" height="12" fill="#ff0000"/></svg>"##;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn draws_at_default_and_asked_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "box.svg", BOX);
        let pic = decode(&p).unwrap();
        assert_eq!((pic.img.width(), pic.img.height()), (1024, 512));
        let rgba = pic.img.to_rgba8();
        // Left half red, right half see-through.
        assert_eq!(rgba.get_pixel(100, 100).0, [255, 0, 0, 255]);
        assert_eq!(rgba.get_pixel(900, 100).0[3], 0);

        let two = decode_sized(
            &p,
            ResizeSpec {
                percent: Some(200),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((two.img.width(), two.img.height()), (48, 24));
        let wide = sized(
            (24.0, 12.0),
            ResizeSpec {
                width: Some(300),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(wide, (300, 150));
        let max = sized(
            (24.0, 12.0),
            ResizeSpec {
                max: Some(512),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(max, (512, 256));
        assert!(sized(
            (24.0, 12.0),
            ResizeSpec {
                percent: Some(100_000),
                ..Default::default()
            }
        )
        .is_err());
        // Large drawings keep their own size.
        assert_eq!(default_size((3000.0, 1500.0)).unwrap(), (3000, 1500));
    }

    #[test]
    fn vector_pdf_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "box.svg", BOX);
        let pdf = to_pdf(&p).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        let doc = lopdf::Document::load_mem(&pdf).unwrap();
        assert_eq!(doc.get_pages().len(), 1);

        let bad = write(dir.path(), "bad.svg", "<svg");
        assert!(matches!(decode(&bad), Err(Error::UnsupportedInput(_))));
    }

    #[test]
    fn svgz_is_unpacked_with_a_limit() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("box.svgz");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(BOX.as_bytes()).unwrap();
        std::fs::write(&p, gz.finish().unwrap()).unwrap();
        assert_eq!(
            crate::format::detect(&p).unwrap(),
            crate::format::Format::Svg
        );
        assert_eq!(decode(&p).unwrap().img.width(), 1024);
        // 70 MB of spaces packs into a few kilobytes but is refused.
        let bomb = dir.path().join("bomb.svgz");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(b"<svg xmlns=\"http://www.w3.org/2000/svg\">")
            .unwrap();
        let spaces = vec![b' '; 1 << 20];
        for _ in 0..70 {
            gz.write_all(&spaces).unwrap();
        }
        std::fs::write(&bomb, gz.finish().unwrap()).unwrap();
        assert!(matches!(load(&bomb), Err(Error::TooLarge(_))));
    }

    #[test]
    fn pictures_only_from_beside_the_svg() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("art");
        std::fs::create_dir(&inner).unwrap();
        let png = dir.path().join("secret.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]))
            .save(&png)
            .unwrap();
        let near = inner.join("near.png");
        std::fs::copy(&png, &near).unwrap();
        let svg = |href: &str| {
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image width="4" height="4" xlink:href="{href}"/></svg>"#
            )
        };
        let blue_at = |p: &Path| {
            let pic = decode_sized(
                p,
                ResizeSpec {
                    percent: Some(100),
                    ..Default::default()
                },
            )
            .unwrap();
            pic.img.to_rgba8().get_pixel(2, 2).0
        };
        let ok = write(&inner, "ok.svg", &svg("near.png"));
        assert_eq!(blue_at(&ok), [0, 0, 255, 255]);
        let up = write(&inner, "up.svg", &svg("../secret.png"));
        assert_eq!(blue_at(&up)[3], 0, "outside its folder");
        let abs = write(&inner, "abs.svg", &svg(png.to_str().unwrap()));
        assert_eq!(blue_at(&abs)[3], 0, "absolute path");
    }
}
