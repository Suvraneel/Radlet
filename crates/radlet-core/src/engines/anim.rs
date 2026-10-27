//! Moving pictures between GIF, WebP and PNG (APNG): every frame and its
//! timing carried over, instead of keeping only the first frame.

use std::borrow::Cow;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom};
use std::path::Path;

use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frame, ImageDecoder, RgbaImage};

use crate::engines::image::limits;
use crate::error::{Error, Result};
use crate::format::Format;

/// Most frames read.
const MAX_FRAMES: usize = 3_000;
/// Most bytes of decoded frames held at once (1 GB).
const MAX_PIXELS_BYTES: u64 = 1 << 30;

/// Formats that can hold animation.
pub fn animatable(f: Format) -> bool {
    matches!(f, Format::Gif | Format::Webp | Format::Png)
}

/// Whether the file has more than one frame, from its structure alone
/// (nothing is decoded).
pub fn is_animated(path: &Path, f: Format) -> bool {
    match f {
        Format::Gif => std::fs::read(path)
            .ok()
            .and_then(|data| crate::engines::meta::gif_frames(&data).ok())
            .is_some_and(|n| n > 1),
        // VP8X's animation flag.
        Format::Webp => {
            let mut head = [0u8; 21];
            File::open(path)
                .and_then(|mut file| file.read_exact(&mut head))
                .is_ok()
                && &head[12..16] == b"VP8X"
                && head[20] & 0x02 != 0
        }
        // An acTL chunk before the image data, however much comes first.
        Format::Png => apng_control(path).unwrap_or(false),
        _ => false,
    }
}

fn apng_control(path: &Path) -> std::io::Result<bool> {
    let mut file = BufReader::new(File::open(path)?);
    file.seek(SeekFrom::Start(8))?;
    let mut header = [0u8; 8];
    for _ in 0..10_000 {
        if file.read_exact(&mut header).is_err() {
            return Ok(false);
        }
        match &header[4..8] {
            b"acTL" => return Ok(true),
            b"IDAT" | b"IEND" => return Ok(false),
            _ => {}
        }
        let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        file.seek(SeekFrom::Current(i64::from(len) + 4))?;
    }
    Ok(false)
}

fn frames(path: &Path, f: Format) -> Result<Vec<Frame>> {
    let reader = BufReader::new(File::open(path)?);
    // Every decoder is held to the usual size and memory limits before a
    // frame is made.
    let iter: image::Frames<'_> = match f {
        Format::Gif => {
            let mut d = GifDecoder::new(reader)?;
            d.set_limits(limits())?;
            d.into_frames()
        }
        Format::Webp => {
            let mut d = WebPDecoder::new(reader)?;
            d.set_limits(limits())?;
            d.into_frames()
        }
        Format::Png => PngDecoder::with_limits(reader, limits())?
            .apng()?
            .into_frames(),
        other => {
            return Err(Error::UnsupportedInput(format!(
                "{other} pictures don't move"
            )))
        }
    };
    let mut out = Vec::new();
    let mut bytes = 0u64;
    for frame in iter {
        let frame = frame?;
        bytes += frame.buffer().as_raw().len() as u64;
        if out.len() >= MAX_FRAMES || bytes > MAX_PIXELS_BYTES {
            return Err(Error::TooLarge(
                "this animation has too many frames to convert".into(),
            ));
        }
        out.push(frame);
    }
    if out.is_empty() {
        return Err(Error::NothingToDo("there are no frames in it".into()));
    }
    Ok(out)
}

/// A frame's delay in milliseconds. Browsers show GIF delays of 10 ms or
/// less as 100 ms; so does this, for GIFs only.
fn delay_ms(frame: &Frame, from: Format) -> u32 {
    let (n, d) = frame.delay().numer_denom_ms();
    let ms = n.checked_div(d).unwrap_or(0);
    if from == Format::Gif && ms <= 10 {
        100
    } else {
        ms.max(1)
    }
}

/// Encodes full-canvas RGBA frames, each with its delay, as an animated
/// WebP that loops forever. The last frame keeps its own delay.
fn encode_webp(
    frames: &[(Cow<'_, [u8]>, u32)],
    w: u32,
    h: u32,
    config: &webp::WebPConfig,
) -> Result<Vec<u8>> {
    use libwebp_sys as sys;
    let failed = |what: &str| Error::Invalid(format!("WebP encoding failed ({what})"));
    struct Encoder(*mut sys::WebPAnimEncoder);
    impl Drop for Encoder {
        fn drop(&mut self) {
            // SAFETY: made by WebPAnimEncoderNewInternal, deleted once.
            unsafe { sys::WebPAnimEncoderDelete(self.0) }
        }
    }
    // SAFETY: libwebp's animation encoder used as its header describes:
    // options initialised before use, every picture imported from a buffer
    // of w × h × 4 bytes and freed after it's added, and the assembled data
    // copied out before it's freed.
    unsafe {
        let abi = sys::WebPGetMuxABIVersion();
        let mut options = std::mem::MaybeUninit::<sys::WebPAnimEncoderOptions>::uninit();
        if sys::WebPAnimEncoderOptionsInitInternal(options.as_mut_ptr(), abi) == 0 {
            return Err(failed("options"));
        }
        let mut options = options.assume_init();
        options.anim_params.loop_count = 0;
        let enc = Encoder(sys::WebPAnimEncoderNewInternal(
            w as i32, h as i32, &options, abi,
        ));
        if enc.0.is_null() {
            return Err(failed("encoder"));
        }
        let mut t: i32 = 0;
        for (data, delay) in frames {
            if data.len() != (w as usize) * (h as usize) * 4 {
                return Err(failed("frame size"));
            }
            let mut pic = sys::WebPPicture::new().map_err(|_| failed("picture"))?;
            pic.use_argb = 1;
            pic.width = w as i32;
            pic.height = h as i32;
            if sys::WebPPictureImportRGBA(&mut pic, data.as_ptr(), (w * 4) as i32) == 0 {
                sys::WebPPictureFree(&mut pic);
                return Err(failed("frame"));
            }
            let added = sys::WebPAnimEncoderAdd(enc.0, &mut pic, t, config);
            sys::WebPPictureFree(&mut pic);
            if added == 0 {
                return Err(failed("frame"));
            }
            t = t.saturating_add(i32::try_from(*delay).unwrap_or(i32::MAX));
        }
        // The end time sets the last frame's delay.
        if sys::WebPAnimEncoderAdd(enc.0, std::ptr::null_mut(), t, std::ptr::null()) == 0 {
            return Err(failed("end"));
        }
        let mut data = std::mem::MaybeUninit::<sys::WebPData>::zeroed().assume_init();
        sys::WebPDataInit(&mut data);
        if sys::WebPAnimEncoderAssemble(enc.0, &mut data) == 0 {
            return Err(failed("assembly"));
        }
        let bytes = std::slice::from_raw_parts(data.bytes, data.size).to_vec();
        sys::WebPDataClear(&mut data);
        Ok(bytes)
    }
}

/// A frame's pixels on the whole `w` × `h` canvas. Frames usually come
/// composited (used as they are); any that don't are placed on one.
fn full_canvas(f: &Frame, w: u32, h: u32) -> Cow<'_, [u8]> {
    let b = f.buffer();
    if b.dimensions() == (w, h) && f.left() == 0 && f.top() == 0 {
        return Cow::Borrowed(b.as_raw().as_slice());
    }
    let mut c = RgbaImage::new(w, h);
    image::imageops::overlay(&mut c, b, i64::from(f.left()), i64::from(f.top()));
    Cow::Owned(c.into_raw())
}

/// Converts the animation at `path` into `to` at `out`. `quality` (1–100)
/// makes WebP output lossy; without it, GIFs become lossless WebPs and
/// the rest lossy at 82.
pub fn convert(
    path: &Path,
    from: Format,
    to: Format,
    quality: Option<u8>,
    out: &Path,
) -> Result<()> {
    let frames = frames(path, from)?;
    let (w, h) = frames[0].buffer().dimensions();
    let canvas = |f| full_canvas(f, w, h);
    match to {
        Format::Gif => {
            let mut enc = GifEncoder::new_with_speed(BufWriter::new(File::create(out)?), 10);
            enc.set_repeat(Repeat::Infinite)?;
            for f in &frames {
                let delay = image::Delay::from_numer_denom_ms(delay_ms(f, from), 1);
                let img = RgbaImage::from_raw(w, h, canvas(f).into_owned())
                    .ok_or_else(|| Error::Internal("a frame lost its size".into()))?;
                enc.encode_frame(Frame::from_parts(img, 0, 0, delay))?;
            }
        }
        Format::Webp => {
            if w > 16_383 || h > 16_383 {
                return Err(Error::TooLarge(
                    "WebP images can be at most 16,383 pixels a side".into(),
                ));
            }
            let mut config = webp::WebPConfig::new()
                .map_err(|_| Error::Internal("couldn't set up the WebP encoder".into()))?;
            // Lossless suits GIF-like art and is still smaller than the GIF.
            config.lossless = i32::from(from == Format::Gif && quality.is_none());
            config.quality = f32::from(quality.unwrap_or(82).clamp(1, 100));
            let list: Vec<(Cow<'_, [u8]>, u32)> = frames
                .iter()
                .map(|f| (canvas(f), delay_ms(f, from)))
                .collect();
            std::fs::write(out, encode_webp(&list, w, h, &config)?)?;
        }
        Format::Png => {
            let file = BufWriter::new(File::create(out)?);
            let mut enc = png::Encoder::new(file, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_animated(frames.len() as u32, 0)
                .map_err(|e| Error::Invalid(format!("APNG encoding failed: {e}")))?;
            let mut writer = enc
                .write_header()
                .map_err(|e| Error::Invalid(format!("APNG encoding failed: {e}")))?;
            for f in &frames {
                let ms = delay_ms(f, from).min(u32::from(u16::MAX));
                writer
                    .set_frame_delay(ms as u16, 1000)
                    .and_then(|_| writer.write_image_data(&canvas(f)))
                    .map_err(|e| Error::Invalid(format!("APNG encoding failed: {e}")))?;
            }
            writer
                .finish()
                .map_err(|e| Error::Invalid(format!("APNG encoding failed: {e}")))?;
        }
        other => {
            return Err(Error::UnsupportedRoute {
                from: "an animation".into(),
                to: other.label().into(),
            })
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(path: &Path, n: u32) {
        let mut enc = GifEncoder::new(File::create(path).unwrap());
        enc.set_repeat(Repeat::Infinite).unwrap();
        for i in 0..n {
            let img = RgbaImage::from_pixel(
                8,
                6,
                image::Rgba([(i * 60) as u8, 0, 255 - (i * 60) as u8, 255]),
            );
            enc.encode_frame(Frame::from_parts(
                img,
                0,
                0,
                image::Delay::from_numer_denom_ms(150, 1),
            ))
            .unwrap();
        }
    }

    #[test]
    fn frames_survive_every_way() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.gif");
        gif(&src, 3);
        assert!(is_animated(&src, Format::Gif));
        let still = dir.path().join("still.gif");
        gif(&still, 1);
        assert!(!is_animated(&still, Format::Gif));
        for to in [Format::Webp, Format::Png] {
            let out = dir.path().join(format!("a.{}", to.id()));
            convert(&src, Format::Gif, to, None, &out).unwrap();
            assert!(is_animated(&out, to), "{to}");
            let back = frames(&out, to).unwrap();
            assert_eq!(back.len(), 3, "{to}");
            assert_eq!(delay_ms(&back[1], to), 150, "{to}");
            let again = dir.path().join(format!("again-{}.gif", to.id()));
            convert(&out, to, Format::Gif, None, &again).unwrap();
            assert_eq!(frames(&again, Format::Gif).unwrap().len(), 3, "{to}");
        }
    }

    #[test]
    fn the_last_frame_keeps_its_delay() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("slow end.gif");
        {
            let mut enc = GifEncoder::new(File::create(&src).unwrap());
            for (i, ms) in [100u32, 100, 2000].into_iter().enumerate() {
                let img = RgbaImage::from_pixel(4, 4, image::Rgba([i as u8 * 90, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(
                    img,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(ms, 1),
                ))
                .unwrap();
            }
        }
        let webp = dir.path().join("out.webp");
        convert(&src, Format::Gif, Format::Webp, None, &webp).unwrap();
        let delays: Vec<u32> = frames(&webp, Format::Webp)
            .unwrap()
            .iter()
            .map(|f| delay_ms(f, Format::Webp))
            .collect();
        assert_eq!(delays, [100, 100, 2000]);
    }

    #[test]
    fn a_huge_canvas_is_refused_before_decoding() {
        // A GIF claiming 60000 × 60000 with two tiny frames.
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&60000u16.to_le_bytes());
        gif.extend_from_slice(&60000u16.to_le_bytes());
        gif.extend_from_slice(&[0, 0, 0]);
        for _ in 0..2 {
            gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0x80, 0, 0, 0, 255, 255, 255]);
            gif.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
        }
        gif.push(0x3B);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.gif");
        std::fs::write(&path, &gif).unwrap();
        assert!(is_animated(&path, Format::Gif));
        let out = dir.path().join("huge.webp");
        assert!(convert(&path, Format::Gif, Format::Webp, None, &out).is_err());
    }
}
