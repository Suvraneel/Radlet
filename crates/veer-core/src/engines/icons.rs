//! App and web icons from one picture: a macOS .icns file, a favicon set
//! for websites, and an iOS app icon set for Xcode.

use std::io::Cursor;
use std::path::Path;

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType, ImageFormat, RgbaImage};

use crate::error::{Error, Result};

/// Largest icon made (macOS and iOS want 1024 pixels).
const LARGEST: u32 = 1024;

/// The picture centred on a transparent square, so nothing is cropped or
/// stretched. Big pictures are first made at most [`LARGEST`] pixels
/// across, so each icon size is resized from that, not the original.
pub fn square(img: &DynamicImage) -> RgbaImage {
    let rgba = if img.width().max(img.height()) > LARGEST {
        img.resize(LARGEST, LARGEST, FilterType::Lanczos3)
            .to_rgba8()
    } else {
        img.to_rgba8()
    };
    let (w, h) = rgba.dimensions();
    let side = w.max(h);
    if w == h {
        return rgba;
    }
    let mut canvas = RgbaImage::new(side, side);
    image::imageops::overlay(
        &mut canvas,
        &rgba,
        ((side - w) / 2) as i64,
        ((side - h) / 2) as i64,
    );
    canvas
}

fn sized(sq: &RgbaImage, px: u32) -> RgbaImage {
    if sq.width() == px {
        sq.clone()
    } else {
        image::imageops::resize(sq, px, px, FilterType::Lanczos3)
    }
}

fn png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img.clone()).write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// Transparent areas filled with white: iOS app icons and Apple touch
/// icons can't be see-through (iOS would fill them with black).
fn opaque(img: &RgbaImage) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let a = p.0[3] as u32;
        for c in 0..3 {
            p.0[c] = ((p.0[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
        p.0[3] = 255;
    }
    out
}

/// A macOS icon file holding every size from 16 to 1024 pixels.
pub fn icns(img: &DynamicImage) -> Result<Vec<u8>> {
    let sq = square(img);
    // Each entry's type and size; the @2x types share pixels with the
    // next size up.
    const ENTRIES: [(&[u8; 4], u32); 11] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"icp6", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic13", 256),
        (b"ic14", 512),
    ];
    let mut body = Vec::new();
    let mut cache: Vec<(u32, Vec<u8>)> = Vec::new();
    for (kind, px) in ENTRIES {
        let data = match cache.iter().find(|(p, _)| *p == px) {
            Some((_, d)) => d.clone(),
            None => {
                let d = png(&sized(&sq, px))?;
                cache.push((px, d.clone()));
                d
            }
        };
        body.extend_from_slice(kind);
        body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// A Windows/website .ico file with 16, 32 and 48 pixel versions.
pub fn ico(sq: &RgbaImage) -> Result<Vec<u8>> {
    let frames: Vec<IcoFrame> = [16u32, 32, 48]
        .iter()
        .map(|&px| {
            let s = sized(sq, px);
            IcoFrame::as_png(s.as_raw(), px, px, ExtendedColorType::Rgba8)
        })
        .collect::<std::result::Result<_, _>>()?;
    let mut out = Vec::new();
    IcoEncoder::new(&mut out).encode_images(&frames)?;
    Ok(out)
}

/// Every file of a website's icon set, written into `dir`: favicons,
/// the Apple touch icon, Android/PWA icons, a web manifest, the source
/// SVG when there is one, and the HTML that links them.
pub fn favicons(img: &DynamicImage, svg_source: Option<&Path>, dir: &Path) -> Result<()> {
    let sq = square(img);
    std::fs::write(dir.join("favicon.ico"), ico(&sq)?)?;
    std::fs::write(dir.join("favicon-16x16.png"), png(&sized(&sq, 16))?)?;
    std::fs::write(dir.join("favicon-32x32.png"), png(&sized(&sq, 32))?)?;
    std::fs::write(
        dir.join("apple-touch-icon.png"),
        png(&opaque(&sized(&sq, 180)))?,
    )?;
    std::fs::write(
        dir.join("android-chrome-192x192.png"),
        png(&sized(&sq, 192))?,
    )?;
    std::fs::write(
        dir.join("android-chrome-512x512.png"),
        png(&sized(&sq, 512))?,
    )?;
    std::fs::write(
        dir.join("site.webmanifest"),
        "{\n  \"icons\": [\n    { \"src\": \"/android-chrome-192x192.png\", \"sizes\": \"192x192\", \"type\": \"image/png\" },\n    { \"src\": \"/android-chrome-512x512.png\", \"sizes\": \"512x512\", \"type\": \"image/png\" }\n  ]\n}\n",
    )?;
    let mut head = String::from(
        "<!-- Put these files at the root of your site and these lines in <head>. -->\n",
    );
    if let Some(svg) = svg_source {
        std::fs::copy(svg, dir.join("favicon.svg"))?;
        head.push_str("<link rel=\"icon\" href=\"/favicon.svg\" type=\"image/svg+xml\">\n");
    }
    head.push_str(
        "<link rel=\"icon\" href=\"/favicon.ico\" sizes=\"48x48\">\n<link rel=\"icon\" type=\"image/png\" sizes=\"32x32\" href=\"/favicon-32x32.png\">\n<link rel=\"icon\" type=\"image/png\" sizes=\"16x16\" href=\"/favicon-16x16.png\">\n<link rel=\"apple-touch-icon\" href=\"/apple-touch-icon.png\">\n<link rel=\"manifest\" href=\"/site.webmanifest\">\n",
    );
    std::fs::write(dir.join("head.html"), head)?;
    Ok(())
}

/// An Xcode app icon set (one 1024-pixel, opaque picture, which Xcode
/// sizes for every device), written as `AppIcon.appiconset` in `dir`.
pub fn ios(img: &DynamicImage, dir: &Path) -> Result<()> {
    let set = dir.join("AppIcon.appiconset");
    std::fs::create_dir_all(&set)?;
    let sq = square(img);
    std::fs::write(
        set.join("AppIcon-1024.png"),
        png(&opaque(&sized(&sq, 1024)))?,
    )?;
    std::fs::write(
        set.join("Contents.json"),
        "{\n  \"images\" : [\n    {\n      \"filename\" : \"AppIcon-1024.png\",\n      \"idiom\" : \"universal\",\n      \"platform\" : \"ios\",\n      \"size\" : \"1024x1024\"\n    }\n  ],\n  \"info\" : {\n    \"author\" : \"xcode\",\n    \"version\" : 1\n  }\n}\n",
    )?;
    Ok(())
}

/// The sizes inside an .icns file, for checks.
pub fn icns_entries(data: &[u8]) -> Result<Vec<(String, usize)>> {
    if !data.starts_with(b"icns") || data.len() < 8 {
        return Err(Error::Invalid("not an icns file".into()));
    }
    let mut out = Vec::new();
    let mut i = 8;
    while i + 8 <= data.len() {
        let kind = String::from_utf8_lossy(&data[i..i + 4]).into_owned();
        let len = u32::from_be_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        if len < 8 || i + len > data.len() {
            return Err(Error::Invalid("damaged icns entry".into()));
        }
        out.push((kind, len - 8));
        i += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide() -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            300,
            100,
            image::Rgba([10, 120, 250, 255]),
        ))
    }

    #[test]
    fn icns_holds_every_size() {
        let data = icns(&wide()).unwrap();
        let entries = icns_entries(&data).unwrap();
        let kinds: Vec<&str> = entries.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "icp4", "icp5", "icp6", "ic07", "ic08", "ic09", "ic10", "ic11", "ic12", "ic13",
                "ic14"
            ]
        );
        assert_eq!(
            u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize,
            data.len()
        );
        // The 1024 entry is a 1024-pixel PNG, padded to a square.
        let start = 8 + entries.iter().take(6).map(|e| e.1 + 8).sum::<usize>() + 8;
        let big = image::load_from_memory(&data[start..start + entries[6].1]).unwrap();
        assert_eq!((big.width(), big.height()), (1024, 1024));
        assert_eq!(
            big.to_rgba8().get_pixel(512, 10).0[3],
            0,
            "padding is transparent"
        );
        assert_eq!(big.to_rgba8().get_pixel(512, 512).0, [10, 120, 250, 255]);
        // A long, thin picture is shrunk before it's squared.
        let thin = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            40_000,
            40,
            image::Rgba([1, 2, 3, 255]),
        ));
        let started = std::time::Instant::now();
        assert_eq!(square(&thin).dimensions(), (1024, 1024));
        assert!(started.elapsed().as_secs() < 5);
    }

    #[test]
    fn favicon_and_ios_sets() {
        let dir = tempfile::tempdir().unwrap();
        favicons(&wide(), None, dir.path()).unwrap();
        let ico = image::load_from_memory(&std::fs::read(dir.path().join("favicon.ico")).unwrap())
            .unwrap();
        assert_eq!(ico.width(), 48);
        let touch = image::open(dir.path().join("apple-touch-icon.png")).unwrap();
        assert_eq!(touch.width(), 180);
        assert_eq!(
            touch.to_rgba8().get_pixel(90, 2).0,
            [255, 255, 255, 255],
            "opaque"
        );
        assert!(std::fs::read_to_string(dir.path().join("head.html"))
            .unwrap()
            .contains("site.webmanifest"));
        ios(&wide(), dir.path()).unwrap();
        let icon = image::open(dir.path().join("AppIcon.appiconset/AppIcon-1024.png")).unwrap();
        assert_eq!(icon.width(), 1024);
        assert!(!icon.color().has_alpha() || icon.to_rgba8().pixels().all(|p| p.0[3] == 255));
    }
}
