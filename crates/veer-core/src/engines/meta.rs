//! Lossless metadata removal for JPEG, PNG and WebP, done by dropping
//! container segments rather than re-encoding pixels. Colour profiles are
//! kept because they change how the image looks, not who took it.

use crate::error::{Error, Result};

/// Removes EXIF, XMP, IPTC, comments and other application segments from a
/// JPEG, keeping JFIF, ICC profiles and the Adobe colour-transform marker.
/// If `orientation` isn't 1, a minimal EXIF block holding only the
/// orientation is written so the photo still displays upright. Data after
/// the end-of-image marker (embedded previews, depth maps) is dropped.
pub fn strip_jpeg(data: &[u8], orientation: u8) -> Result<Vec<u8>> {
    let bad = || Error::Invalid("damaged JPEG".into());
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err(bad());
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut wrote_orientation = orientation == 1;
    let mut i = 2;
    loop {
        // Skip fill bytes between segments.
        while i < data.len() && data[i] == 0xFF && data.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if i + 1 >= data.len() || data[i] != 0xFF {
            return Err(bad());
        }
        let marker = data[i + 1];
        match marker {
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                return Ok(out);
            }
            0xD0..=0xD7 | 0x01 => {
                out.extend_from_slice(&data[i..i + 2]);
                i += 2;
                continue;
            }
            _ => {}
        }
        if i + 4 > data.len() {
            return Err(bad());
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let end = i + 2 + len;
        if len < 2 || end > data.len() {
            return Err(bad());
        }
        let payload = &data[i + 4..end];
        let keep = match marker {
            0xE0 => true,                                  // JFIF
            0xE2 => payload.starts_with(b"ICC_PROFILE\0"), // colour profile
            0xEE => payload.starts_with(b"Adobe"),         // colour transform
            0xE1 | 0xE3..=0xED | 0xEF | 0xFE => false,     // EXIF, XMP, IPTC, comments...
            _ => true,                                     // tables, frame and scan headers
        };
        if !wrote_orientation && marker != 0xE0 {
            out.extend_from_slice(&exif_orientation_segment(orientation));
            wrote_orientation = true;
        }
        if keep {
            out.extend_from_slice(&data[i..end]);
        }
        i = end;
        if marker == 0xDA {
            // Entropy-coded scan data runs until the next real marker.
            let start = i;
            while i + 1 < data.len() {
                if data[i] == 0xFF && data[i + 1] != 0x00 && !(0xD0..=0xD7).contains(&data[i + 1]) {
                    break;
                }
                i += 1;
            }
            if i + 1 >= data.len() {
                return Err(bad());
            }
            out.extend_from_slice(&data[start..i]);
        }
    }
}

/// An APP1 segment containing only EXIF tag 0x0112 (orientation).
fn exif_orientation_segment(orientation: u8) -> Vec<u8> {
    let mut tiff: Vec<u8> = Vec::new();
    tiff.extend_from_slice(b"MM\0\x2A");
    tiff.extend_from_slice(&8u32.to_be_bytes()); // IFD0 offset
    tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
    tiff.extend_from_slice(&0x0112u16.to_be_bytes());
    tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_be_bytes());
    tiff.extend_from_slice(&[0, orientation, 0, 0]);
    tiff.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
    let mut seg = vec![0xFF, 0xE1];
    let len = (2 + 6 + tiff.len()) as u16;
    seg.extend_from_slice(&len.to_be_bytes());
    seg.extend_from_slice(b"Exif\0\0");
    seg.extend_from_slice(&tiff);
    seg
}

/// Removes text, EXIF and timestamp chunks from a PNG.
pub fn strip_png(data: &[u8]) -> Result<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    let bad = || Error::Invalid("damaged PNG".into());
    if !data.starts_with(SIG) {
        return Err(bad());
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(SIG);
    let mut i = SIG.len();
    while i + 12 <= data.len() {
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let end = i.checked_add(12 + len).ok_or_else(bad)?;
        if end > data.len() {
            return Err(bad());
        }
        let kind = &data[i + 4..i + 8];
        let drop = matches!(
            kind,
            b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME" | b"dSIG"
        );
        if !drop {
            out.extend_from_slice(&data[i..end]);
        }
        i = end;
        if kind == b"IEND" {
            return Ok(out);
        }
    }
    Err(bad())
}

struct RiffChunk<'a> {
    fourcc: [u8; 4],
    body: &'a [u8],
}

fn webp_chunks(data: &[u8]) -> Result<Vec<RiffChunk<'_>>> {
    let bad = || Error::Invalid("damaged WebP".into());
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err(bad());
    }
    let riff_end =
        (u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize + 8).min(data.len());
    let mut chunks = Vec::new();
    let mut i = 12;
    while i + 8 <= riff_end {
        let fourcc = [data[i], data[i + 1], data[i + 2], data[i + 3]];
        let len = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        let end = i + 8 + len;
        if end > riff_end {
            return Err(bad());
        }
        chunks.push(RiffChunk {
            fourcc,
            body: &data[i + 8..end],
        });
        i = end + (len & 1);
    }
    if chunks.is_empty() {
        return Err(bad());
    }
    Ok(chunks)
}

fn write_webp(chunks: &[(&[u8; 4], std::borrow::Cow<'_, [u8]>)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WEBP");
    for (fourcc, data) in chunks {
        body.extend_from_slice(*fourcc);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        if data.len() & 1 == 1 {
            body.push(0);
        }
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

const VP8X_ICC: u8 = 0x20;
const VP8X_ALPHA: u8 = 0x10;
const VP8X_EXIF: u8 = 0x08;
const VP8X_XMP: u8 = 0x04;

/// Removes EXIF and XMP chunks from a WebP.
pub fn strip_webp(data: &[u8]) -> Result<Vec<u8>> {
    use std::borrow::Cow;
    let chunks = webp_chunks(data)?;
    let kept: Vec<(&[u8; 4], Cow<'_, [u8]>)> = chunks
        .iter()
        .filter(|c| &c.fourcc != b"EXIF" && &c.fourcc != b"XMP ")
        .map(|c| {
            if &c.fourcc == b"VP8X" && !c.body.is_empty() {
                let mut b = c.body.to_vec();
                b[0] &= !(VP8X_EXIF | VP8X_XMP);
                (&c.fourcc, Cow::Owned(b))
            } else {
                (&c.fourcc, Cow::Borrowed(c.body))
            }
        })
        .collect();
    Ok(write_webp(&kept))
}

/// Embeds an ICC profile into a WebP produced by libwebp's simple encoder,
/// which can't write one itself.
pub fn webp_add_icc(
    data: &[u8],
    icc: &[u8],
    width: u32,
    height: u32,
    has_alpha: bool,
) -> Result<Vec<u8>> {
    use std::borrow::Cow;
    let chunks = webp_chunks(data)?;
    let mut out: Vec<(&[u8; 4], Cow<'_, [u8]>)> = Vec::new();
    if &chunks[0].fourcc == b"VP8X" {
        let mut vp8x = chunks[0].body.to_vec();
        if vp8x.is_empty() {
            return Err(Error::Invalid("damaged WebP".into()));
        }
        vp8x[0] |= VP8X_ICC;
        out.push((b"VP8X", Cow::Owned(vp8x)));
        out.push((b"ICCP", Cow::Borrowed(icc)));
        out.extend(
            chunks[1..]
                .iter()
                .filter(|c| &c.fourcc != b"ICCP")
                .map(|c| (&c.fourcc, Cow::Borrowed(c.body))),
        );
    } else {
        let mut vp8x = vec![VP8X_ICC | if has_alpha { VP8X_ALPHA } else { 0 }, 0, 0, 0];
        vp8x.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        vp8x.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        out.push((b"VP8X", Cow::Owned(vp8x)));
        out.push((b"ICCP", Cow::Borrowed(icc)));
        out.extend(chunks.iter().map(|c| (&c.fourcc, Cow::Borrowed(c.body))));
    }
    Ok(write_webp(&out))
}

/// Walks a GIF's blocks, returning the frame count and (when `strip` is
/// set) a copy without comments and non-animation application blocks such
/// as XMP. Pixel data is copied unchanged, so animation survives.
fn walk_gif(data: &[u8], strip: bool) -> Result<(usize, Vec<u8>)> {
    let bad = || Error::Invalid("damaged GIF".into());
    if data.len() < 13 || !(data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        return Err(bad());
    }
    let table = |packed: u8| {
        if packed & 0x80 != 0 {
            3usize << ((packed & 7) + 1)
        } else {
            0
        }
    };
    let skip_sub_blocks = |mut j: usize| -> Result<usize> {
        loop {
            let n = *data.get(j).ok_or_else(bad)? as usize;
            j += 1;
            if n == 0 {
                return Ok(j);
            }
            j += n;
        }
    };
    let mut i = 13 + table(data[10]);
    if i > data.len() {
        return Err(bad());
    }
    let mut out = Vec::with_capacity(if strip { data.len() } else { 0 });
    if strip {
        out.extend_from_slice(&data[..i]);
    }
    let mut frames = 0;
    loop {
        match *data.get(i).ok_or_else(bad)? {
            0x3B => {
                if strip {
                    out.push(0x3B);
                }
                return Ok((frames, out));
            }
            0x2C => {
                let packed = *data.get(i + 9).ok_or_else(bad)?;
                let start = i;
                // Descriptor (10 bytes), local colour table, LZW code size.
                let end = skip_sub_blocks(i + 10 + table(packed) + 1)?;
                if end > data.len() {
                    return Err(bad());
                }
                if strip {
                    out.extend_from_slice(&data[start..end]);
                }
                frames += 1;
                i = end;
            }
            0x21 => {
                let label = *data.get(i + 1).ok_or_else(bad)?;
                let end = skip_sub_blocks(i + 2)?;
                if end > data.len() {
                    return Err(bad());
                }
                let app_id = data.get(i + 3..i + 14);
                let keep = match label {
                    0xFE => false,
                    0xFF => matches!(app_id, Some(b"NETSCAPE2.0") | Some(b"ANIMEXTS1.0")),
                    _ => true,
                };
                if strip && keep {
                    out.extend_from_slice(&data[i..end]);
                }
                i = end;
            }
            _ => return Err(bad()),
        }
    }
}

/// Removes comments and metadata blocks from a GIF, keeping every frame.
pub fn strip_gif(data: &[u8]) -> Result<Vec<u8>> {
    walk_gif(data, true).map(|(_, out)| out)
}

/// Number of frames in a GIF (1 for a still image).
pub fn gif_frames(data: &[u8]) -> Result<usize> {
    walk_gif(data, false).map(|(n, _)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut s = vec![0xFF, marker];
        s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        s.extend_from_slice(payload);
        s
    }

    #[test]
    fn jpeg_segments_filtered() {
        let mut j = vec![0xFF, 0xD8];
        j.extend(seg(0xE0, b"JFIF\0\x01\x01"));
        j.extend(seg(0xE1, b"Exif\0\0secret-gps"));
        j.extend(seg(0xE2, b"ICC_PROFILE\0\x01\x01data"));
        j.extend(seg(0xFE, b"a comment"));
        j.extend(seg(0xDB, &[0u8; 65]));
        j.extend(seg(0xDA, &[1, 1, 0, 0, 63, 0]));
        j.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]);
        j.extend_from_slice(&[0xFF, 0xD9]);
        j.extend_from_slice(b"trailing preview with Exif");

        let out = strip_jpeg(&j, 1).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(!s.contains("secret-gps") && !s.contains("comment") && !s.contains("trailing"));
        assert!(s.contains("JFIF") && s.contains("ICC_PROFILE"));
        assert!(
            out.windows(7)
                .any(|w| w == [0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]),
            "scan data intact"
        );
        assert!(out.ends_with(&[0xFF, 0xD9]));
    }

    #[test]
    fn jpeg_orientation_kept() {
        let mut j = vec![0xFF, 0xD8];
        j.extend(seg(0xE1, b"Exif\0\0junk"));
        j.extend(seg(0xDA, &[1, 1, 0, 0, 63, 0]));
        j.extend_from_slice(&[0x00, 0xFF, 0xD9]);
        let out = strip_jpeg(&j, 6).unwrap();
        assert!(!String::from_utf8_lossy(&out).contains("junk"));
        let at = out.windows(6).position(|w| w == b"Exif\0\0").unwrap() + 6;
        let o = image::metadata::Orientation::from_exif_chunk(&out[at..at + 26]);
        assert_eq!(o.map(|o| o.to_exif()), Some(6));
    }

    #[test]
    fn png_text_removed() {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            c.extend_from_slice(&[0, 0, 0, 0]);
            c
        };
        p.extend(chunk(b"IHDR", &[0; 13]));
        p.extend(chunk(b"tEXt", b"Author\0me"));
        p.extend(chunk(b"iCCP", b"icc"));
        p.extend(chunk(b"IDAT", b"pixels"));
        p.extend(chunk(b"IEND", b""));
        let out = strip_png(&p).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(!s.contains("Author"));
        assert!(s.contains("iCCP") && s.contains("IDAT") && s.contains("IEND"));
    }

    #[test]
    fn webp_exif_removed_and_icc_added() {
        let img = image::RgbImage::from_pixel(3, 2, image::Rgb([10, 20, 30]));
        let enc = webp::Encoder::from_rgb(img.as_raw(), 3, 2)
            .encode_simple(false, 80.0)
            .unwrap();
        let with_icc = webp_add_icc(&enc, b"fake-icc-profile", 3, 2, false).unwrap();
        let chunks = webp_chunks(&with_icc).unwrap();
        assert_eq!(&chunks[0].fourcc, b"VP8X");
        assert_eq!(&chunks[1].fourcc, b"ICCP");
        assert_eq!(chunks[1].body, b"fake-icc-profile");

        // Add a fake EXIF chunk, then strip it.
        let mut parts: Vec<(&[u8; 4], std::borrow::Cow<'_, [u8]>)> = chunks
            .iter()
            .map(|c| (&c.fourcc, std::borrow::Cow::Borrowed(c.body)))
            .collect();
        parts.push((b"EXIF", std::borrow::Cow::Borrowed(b"gps")));
        let dirty = write_webp(&parts);
        let clean = strip_webp(&dirty).unwrap();
        assert!(webp_chunks(&clean)
            .unwrap()
            .iter()
            .all(|c| &c.fourcc != b"EXIF"));
        let decoded = image::load_from_memory(&clean).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
    }

    #[test]
    fn gif_strip_keeps_frames_and_drops_comments() {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, RgbaImage};
        let mut buf = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut buf);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..4u8 {
                let img = RgbaImage::from_pixel(5, 5, image::Rgba([i * 60, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(
                    img,
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                ))
                .unwrap();
            }
        }
        // Insert a comment and an XMP application block before the trailer.
        let mut dirty = buf[..buf.len() - 1].to_vec();
        dirty.extend_from_slice(&[0x21, 0xFE, 6]);
        dirty.extend_from_slice(b"secret");
        dirty.push(0);
        dirty.extend_from_slice(&[0x21, 0xFF, 11]);
        dirty.extend_from_slice(b"XMP DataXMP");
        dirty.extend_from_slice(&[4, b'g', b'p', b's', b'!', 0]);
        dirty.push(0x3B);

        assert_eq!(gif_frames(&dirty).unwrap(), 4);
        let clean = strip_gif(&dirty).unwrap();
        let s = String::from_utf8_lossy(&clean);
        assert!(!s.contains("secret") && !s.contains("XMP DataXMP"));
        assert!(s.contains("NETSCAPE2.0"), "looping kept");
        assert_eq!(gif_frames(&clean).unwrap(), 4);
        assert!(strip_gif(b"GIF89a\x01").is_err());
        assert!(gif_frames(&dirty[..dirty.len() - 3]).is_err());
    }
}
