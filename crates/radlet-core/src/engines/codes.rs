//! QR codes: read from pictures, and made from text.

use image::{DynamicImage, GrayImage, Luma};

use crate::error::{Error, Result};

/// Most text a QR code is made from: beyond this, codes get too dense to
/// scan reliably from a screen or print.
pub const MAX_QR_TEXT: usize = 1_200;

fn scan(img: &GrayImage) -> Vec<String> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let raw = img.as_raw();
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| raw[y * w + x]);
    let mut out = Vec::new();
    for grid in prepared.detect_grids() {
        if let Ok((_, content)) = grid.decode() {
            if !out.contains(&content) {
                out.push(content);
            }
        }
    }
    out
}

/// The contents of every QR code in a picture, in the order found.
pub fn read(img: &DynamicImage) -> Vec<String> {
    let luma = img.to_luma8();
    // Very large photos are scanned smaller first: much faster, and the
    // modules of a code that fills any useful part of the frame survive.
    let longest = luma.width().max(luma.height());
    if longest > 2400 {
        let small = image::imageops::resize(
            &luma,
            luma.width() * 2400 / longest,
            luma.height() * 2400 / longest,
            image::imageops::FilterType::Triangle,
        );
        let found = scan(&small);
        if !found.is_empty() {
            return found;
        }
    }
    let found = scan(&luma);
    if found.is_empty() && longest < 600 {
        // Tiny screenshots read better enlarged.
        let big = image::imageops::resize(
            &luma,
            luma.width() * 3,
            luma.height() * 3,
            image::imageops::FilterType::Nearest,
        );
        return scan(&big);
    }
    found
}

/// A QR code for `text` as a black-on-white picture with the standard
/// quiet zone, about 1000 pixels across.
pub fn make(text: &str) -> Result<GrayImage> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::NothingToDo(
            "there's no text to put in a QR code".into(),
        ));
    }
    if text.len() > MAX_QR_TEXT {
        return Err(Error::TooLarge(format!(
            "a QR code holds at most {MAX_QR_TEXT} characters here; this has {}",
            text.len()
        )));
    }
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M)
        .map_err(|e| Error::Invalid(format!("couldn't make a QR code ({e})")))?;
    let modules = code.width() as u32;
    let colors = code.to_colors();
    let quiet = 4;
    let side = modules + 2 * quiet;
    let scale = (1000 / side).max(4);
    let px = side * scale;
    let mut img = GrayImage::from_pixel(px, px, Luma([255]));
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (mx, my) = (i as u32 % modules, i as u32 / modules);
            let (x0, y0) = ((mx + quiet) * scale, (my + quiet) * scale);
            for y in y0..y0 + scale {
                for x in x0..x0 + scale {
                    img.put_pixel(x, y, Luma([0]));
                }
            }
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_codes_read_back() {
        let url = "https://example.com/menu?table=12";
        let img = make(url).unwrap();
        assert!(img.width() >= 900 && img.width() == img.height());
        assert_eq!(read(&DynamicImage::ImageLuma8(img.clone())), [url]);
        // Small, and set in a bigger, busier picture.
        let small = image::imageops::resize(&img, 180, 180, image::imageops::FilterType::Nearest);
        let mut photo = image::RgbImage::from_fn(3000, 2000, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 120])
        });
        image::imageops::overlay(
            &mut photo,
            &DynamicImage::ImageLuma8(small).to_rgb8(),
            2600,
            1600,
        );
        assert_eq!(read(&DynamicImage::ImageRgb8(photo)), [url]);
        assert!(read(&DynamicImage::ImageLuma8(GrayImage::from_pixel(
            50,
            50,
            Luma([255])
        )))
        .is_empty());
    }

    #[test]
    fn text_limits() {
        assert!(matches!(make("   "), Err(Error::NothingToDo(_))));
        assert!(matches!(make(&"x".repeat(5000)), Err(Error::TooLarge(_))));
        assert!(make("Grüße, 你好").is_ok());
    }
}
