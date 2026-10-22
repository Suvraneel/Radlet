//! File formats Veer knows about, and content-based detection.
//!
//! Detection trusts the file's bytes over its extension, so a PNG saved as
//! `photo.jpg` is still treated as a PNG. The extension only breaks ties
//! between formats that share a container (MP4 vs M4A, WMV vs WMA).

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    // Raster images
    Png,
    Jpeg,
    Webp,
    Gif,
    Bmp,
    Tiff,
    Ico,
    Tga,
    Qoi,
    Heic,
    Avif,
    // Documents
    Pdf,
    // Audio
    Mp3,
    M4a,
    Wav,
    Flac,
    Ogg,
    Opus,
    Aiff,
    Wma,
    // Video
    Mp4,
    Mov,
    Mkv,
    Webm,
    Avi,
    Wmv,
    // Text
    Txt,
    Md,
    Html,
    Srt,
    Vtt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Image,
    Document,
    Audio,
    Video,
    Text,
    Subtitle,
}

impl Category {
    /// SF Symbol name the Mac shell uses for this category.
    pub fn symbol(self) -> &'static str {
        match self {
            Category::Image => "photo",
            Category::Document => "doc.richtext",
            Category::Audio => "waveform",
            Category::Video => "film",
            Category::Text => "doc.plaintext",
            Category::Subtitle => "captions.bubble",
        }
    }
}

use Format::*;

impl Format {
    pub const ALL: &'static [Format] = &[
        Png, Jpeg, Webp, Gif, Bmp, Tiff, Ico, Tga, Qoi, Heic, Avif, Pdf, Mp3, M4a, Wav, Flac, Ogg,
        Opus, Aiff, Wma, Mp4, Mov, Mkv, Webm, Avi, Wmv, Txt, Md, Html, Srt, Vtt,
    ];

    /// Stable identifier used on the wire and in recipes.
    pub fn id(self) -> &'static str {
        match self {
            Png => "png",
            Jpeg => "jpeg",
            Webp => "webp",
            Gif => "gif",
            Bmp => "bmp",
            Tiff => "tiff",
            Ico => "ico",
            Tga => "tga",
            Qoi => "qoi",
            Heic => "heic",
            Avif => "avif",
            Pdf => "pdf",
            Mp3 => "mp3",
            M4a => "m4a",
            Wav => "wav",
            Flac => "flac",
            Ogg => "ogg",
            Opus => "opus",
            Aiff => "aiff",
            Wma => "wma",
            Mp4 => "mp4",
            Mov => "mov",
            Mkv => "mkv",
            Webm => "webm",
            Avi => "avi",
            Wmv => "wmv",
            Txt => "txt",
            Md => "md",
            Html => "html",
            Srt => "srt",
            Vtt => "vtt",
        }
    }

    /// Name shown on a wheel slot.
    pub fn label(self) -> &'static str {
        match self {
            Png => "PNG",
            Jpeg => "JPEG",
            Webp => "WebP",
            Gif => "GIF",
            Bmp => "BMP",
            Tiff => "TIFF",
            Ico => "ICO",
            Tga => "TGA",
            Qoi => "QOI",
            Heic => "HEIC",
            Avif => "AVIF",
            Pdf => "PDF",
            Mp3 => "MP3",
            M4a => "M4A",
            Wav => "WAV",
            Flac => "FLAC",
            Ogg => "OGG",
            Opus => "Opus",
            Aiff => "AIFF",
            Wma => "WMA",
            Mp4 => "MP4",
            Mov => "MOV",
            Mkv => "MKV",
            Webm => "WebM",
            Avi => "AVI",
            Wmv => "WMV",
            Txt => "Text",
            Md => "Markdown",
            Html => "HTML",
            Srt => "SRT",
            Vtt => "VTT",
        }
    }

    /// File extension written for outputs.
    pub fn extension(self) -> &'static str {
        match self {
            Jpeg => "jpg",
            Tiff => "tiff",
            Aiff => "aiff",
            other => other.id(),
        }
    }

    pub fn category(self) -> Category {
        match self {
            Png | Jpeg | Webp | Gif | Bmp | Tiff | Ico | Tga | Qoi | Heic | Avif => Category::Image,
            Pdf => Category::Document,
            Mp3 | M4a | Wav | Flac | Ogg | Opus | Aiff | Wma => Category::Audio,
            Mp4 | Mov | Mkv | Webm | Avi | Wmv => Category::Video,
            Txt | Md | Html => Category::Text,
            Srt | Vtt => Category::Subtitle,
        }
    }

    /// Parses a format name or common alias (`jpg`, `tif`, `markdown`, ...).
    pub fn parse(name: &str) -> Option<Format> {
        let n = name.trim().trim_start_matches('.').to_ascii_lowercase();
        let f = match n.as_str() {
            "jpg" | "jpe" | "jfif" => Jpeg,
            "tif" => Tiff,
            "aif" | "aifc" => Aiff,
            "markdown" | "mdown" | "mkd" => Md,
            "htm" => Html,
            "text" => Txt,
            "heif" | "hif" => Heic,
            "oga" => Ogg,
            "m4v" => Mp4,
            "qt" => Mov,
            _ => return Format::ALL.iter().copied().find(|f| f.id() == n),
        };
        Some(f)
    }

    pub fn from_path(path: &Path) -> Option<Format> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Format::parse)
    }

    /// True for formats Veer can decode into pixels with its built-in codecs.
    pub fn is_builtin_raster(self) -> bool {
        matches!(self, Png | Jpeg | Webp | Gif | Bmp | Tiff | Ico | Tga | Qoi)
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

const SNIFF_LEN: usize = 4096;

/// Detects a file's format from its first bytes, falling back to the
/// extension for formats without a signature (TGA and text formats).
pub fn detect(path: &Path) -> Result<Format> {
    let meta = std::fs::metadata(path)?;
    if meta.is_dir() {
        return Err(Error::UnsupportedInput(format!(
            "{} is a folder",
            display_name(path)
        )));
    }
    let mut header = Vec::with_capacity(SNIFF_LEN);
    File::open(path)?
        .take(SNIFF_LEN as u64)
        .read_to_end(&mut header)?;
    let by_ext = Format::from_path(path);
    if let Some(f) = sniff(&header, by_ext) {
        return Ok(f);
    }
    match by_ext {
        // Formats with no reliable signature are trusted by extension.
        Some(f @ (Tga | Txt | Md | Html | Srt | Vtt)) => Ok(f),
        _ => Err(Error::UnsupportedInput(format!(
            "{} isn't a file type Veer can work with",
            display_name(path)
        ))),
    }
}

/// Identifies a format from leading bytes. `hint` (usually the extension)
/// resolves containers that hold more than one kind of media.
pub fn sniff(h: &[u8], hint: Option<Format>) -> Option<Format> {
    let starts = |sig: &[u8]| h.starts_with(sig);
    let at = |off: usize, sig: &[u8]| h.len() >= off + sig.len() && &h[off..off + sig.len()] == sig;

    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some(Png);
    }
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Some(Jpeg);
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some(Gif);
    }
    if starts(b"RIFF") {
        if at(8, b"WEBP") {
            return Some(Webp);
        }
        if at(8, b"WAVE") {
            return Some(Wav);
        }
        if at(8, b"AVI ") {
            return Some(Avi);
        }
    }
    // Weak signatures (two-byte magic, a marker that may sit after junk,
    // MPEG frame sync) are ignored for files named as text, so a note that
    // starts with "BM" or quotes "%PDF-" stays text.
    let weak_ok = !matches!(hint, Some(Txt | Md | Html | Srt | Vtt));
    if weak_ok && starts(b"BM") && h.len() >= 18 {
        let dib = u32::from_le_bytes([h[14], h[15], h[16], h[17]]);
        if matches!(dib, 12 | 16 | 40 | 52 | 56 | 64 | 108 | 124) {
            return Some(Bmp);
        }
    }
    if starts(b"II*\0") || starts(b"MM\0*") {
        // Many camera RAW formats are TIFF containers; only accept real TIFFs.
        return match hint {
            None | Some(Tiff) => Some(Tiff),
            _ => None,
        };
    }
    if starts(&[0, 0, 1, 0]) && h.len() >= 6 && hint == Some(Ico) {
        return Some(Ico);
    }
    if starts(b"qoif") {
        return Some(Qoi);
    }
    if starts(b"%PDF-") || (weak_ok && h[..h.len().min(1024)].windows(5).any(|w| w == b"%PDF-")) {
        return Some(Pdf);
    }
    if starts(b"fLaC") {
        return Some(Flac);
    }
    if starts(b"OggS") {
        let opus = h.windows(8).any(|w| w == b"OpusHead");
        return Some(if opus { Opus } else { Ogg });
    }
    if starts(b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) {
        return Some(Aiff);
    }
    if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let webm = h.windows(4).any(|w| w == b"webm");
        return Some(if webm { Webm } else { Mkv });
    }
    if starts(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]) {
        return Some(if hint == Some(Wma) { Wma } else { Wmv });
    }
    if at(4, b"ftyp") {
        return Some(classify_ftyp(h, hint));
    }
    if starts(b"ID3") {
        return Some(Mp3);
    }
    // MPEG audio frame sync, layer III (rules out AAC/ADTS, which uses layer 0).
    if weak_ok && h.len() >= 2 && h[0] == 0xFF && (h[1] & 0xE0) == 0xE0 && (h[1] & 0x06) == 0x02 {
        return Some(Mp3);
    }
    None
}

fn classify_ftyp(h: &[u8], hint: Option<Format>) -> Format {
    let fallback = || match hint {
        Some(M4a) => M4a,
        Some(Mov) => Mov,
        _ => Mp4,
    };
    let Some(major) = h.get(8..12) else {
        return fallback();
    };
    let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
    let end = size.clamp(16, 256).min(h.len());
    let compatible: Vec<&[u8]> = h.get(16..end).unwrap_or(&[]).chunks_exact(4).collect();
    let has = |b: &[u8]| major == b || compatible.contains(&b);

    if has(b"avif") || has(b"avis") {
        return Avif;
    }
    if [
        b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
    ]
    .iter()
    .any(|b| major == *b)
    {
        return Heic;
    }
    if major == b"qt  " {
        return Mov;
    }
    if major.starts_with(b"M4A") || major.starts_with(b"M4B") || major.starts_with(b"M4P") {
        return M4a;
    }
    fallback()
}

pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_trusts_bytes_over_extension() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(sniff(png, Some(Jpeg)), Some(Png));
    }

    #[test]
    fn sniff_containers() {
        let mut m4a = vec![0, 0, 0, 0x20];
        m4a.extend_from_slice(b"ftypM4A \0\0\0\0M4A isommp42\0\0\0\0");
        assert_eq!(sniff(&m4a, None), M4a.into());

        let mut heic = vec![0, 0, 0, 0x18];
        heic.extend_from_slice(b"ftypheic\0\0\0\0mif1heic");
        assert_eq!(sniff(&heic, None), Some(Heic));

        let mut avif = vec![0, 0, 0, 0x1c];
        avif.extend_from_slice(b"ftypavif\0\0\0\0avifmif1miaf");
        assert_eq!(sniff(&avif, None), Some(Avif));

        let mut isom = vec![0, 0, 0, 0x18];
        isom.extend_from_slice(b"ftypisom\0\0\0\0isomiso2");
        assert_eq!(sniff(&isom, None), Some(Mp4));
        assert_eq!(sniff(&isom, Some(M4a)), Some(M4a));
    }

    #[test]
    fn sniff_mp3_not_aac() {
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x00], None), Some(Mp3));
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50, 0x80], None), None);
        assert_eq!(sniff(b"ID3\x04\0", None), Some(Mp3));
    }

    #[test]
    fn tiff_based_raw_is_rejected() {
        assert_eq!(sniff(b"II*\0\x08\0\0\0", Some(Tiff)), Some(Tiff));
        assert_eq!(sniff(b"II*\0\x08\0\0\0", None), Some(Tiff));
        assert_eq!(sniff(b"II*\0\x08\0\0\0", Some(Txt)), None);
    }

    #[test]
    fn short_and_hostile_headers_never_panic() {
        for len in 0..40 {
            let mut h = vec![0u8; len];
            if len >= 8 {
                h[4..8].copy_from_slice(b"ftyp");
            }
            let _ = sniff(&h, None);
            let _ = sniff(&h, Some(M4a));
        }
        assert_eq!(sniff(b"\0\0\0\x0cftypisom", None), Some(Mp4));
        assert_eq!(sniff(b"\xff\xff\xff\xffftypM4A ", None), Some(M4a));
    }

    #[test]
    fn text_files_are_not_mistaken_for_binaries() {
        assert_eq!(
            sniff(b"BMW notes: the M3 is fast and fun to drive", Some(Md)),
            None
        );
        assert_eq!(
            sniff(b"BMW notes: the M3 is fast and fun to drive", None),
            None
        );
        assert_eq!(
            sniff(b"Spec quote: files start with %PDF-1.7 marker", Some(Txt)),
            None
        );
        assert_eq!(sniff(b"%PDF-1.7\n", Some(Txt)), Some(Pdf));
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        assert_eq!(sniff(&bmp, None), Some(Bmp));
    }

    #[test]
    fn parse_aliases() {
        assert_eq!(Format::parse("JPG"), Some(Jpeg));
        assert_eq!(Format::parse(".tif"), Some(Tiff));
        assert_eq!(Format::parse("markdown"), Some(Md));
        assert_eq!(Format::parse("webm"), Some(Webm));
        assert_eq!(Format::parse("doc"), None);
    }

    #[test]
    fn ids_round_trip() {
        for f in Format::ALL {
            assert_eq!(Format::parse(f.id()), Some(*f));
            assert_eq!(Format::parse(f.extension()), Some(*f));
        }
    }
}
