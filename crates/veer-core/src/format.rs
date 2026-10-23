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
    /// Scalable vector graphics, rendered to pixels when converted.
    Svg,
    // Documents
    Pdf,
    Docx,
    Rtf,
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
    // Data
    Csv,
    Tsv,
    Json,
    // Archives
    Zip,
    /// A folder on disk. Not a file format, but it gets its own actions.
    Folder,
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
    Data,
    Archive,
    Folder,
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
            Category::Data => "tablecells",
            Category::Archive => "doc.zipper",
            Category::Folder => "folder",
        }
    }
}

use Format::*;

impl Format {
    pub const ALL: &'static [Format] = &[
        Png, Jpeg, Webp, Gif, Bmp, Tiff, Ico, Tga, Qoi, Heic, Avif, Svg, Pdf, Docx, Rtf, Mp3, M4a,
        Wav, Flac, Ogg, Opus, Aiff, Wma, Mp4, Mov, Mkv, Webm, Avi, Wmv, Txt, Md, Html, Srt, Vtt,
        Csv, Tsv, Json, Zip, Folder,
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
            Svg => "svg",
            Pdf => "pdf",
            Docx => "docx",
            Rtf => "rtf",
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
            Csv => "csv",
            Tsv => "tsv",
            Json => "json",
            Zip => "zip",
            Folder => "folder",
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
            Svg => "SVG",
            Pdf => "PDF",
            Docx => "Word",
            Rtf => "RTF",
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
            Csv => "CSV",
            Tsv => "TSV",
            Json => "JSON",
            Zip => "ZIP",
            Folder => "Folder",
        }
    }

    /// File extension written for outputs.
    pub fn extension(self) -> &'static str {
        match self {
            Jpeg => "jpg",
            Tiff => "tiff",
            Aiff => "aiff",
            Folder => "",
            other => other.id(),
        }
    }

    pub fn category(self) -> Category {
        match self {
            Png | Jpeg | Webp | Gif | Bmp | Tiff | Ico | Tga | Qoi | Heic | Avif | Svg => {
                Category::Image
            }
            Pdf | Docx | Rtf => Category::Document,
            Mp3 | M4a | Wav | Flac | Ogg | Opus | Aiff | Wma => Category::Audio,
            Mp4 | Mov | Mkv | Webm | Avi | Wmv => Category::Video,
            Txt | Md | Html => Category::Text,
            Srt | Vtt => Category::Subtitle,
            Csv | Tsv | Json => Category::Data,
            Zip => Category::Archive,
            Folder => Category::Folder,
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
            "svgz" => Svg,
            "word" => Docx,
            "tab" => Tsv,
            "geojson" => Json,
            _ => return Format::ALL.iter().copied().find(|f| f.id() == n),
        };
        Some(f)
    }

    pub fn from_path(path: &Path) -> Option<Format> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Format::parse)
            .filter(|f| *f != Folder)
    }

    /// True for formats Veer can decode into pixels with its built-in codecs.
    pub fn is_builtin_raster(self) -> bool {
        matches!(self, Png | Jpeg | Webp | Gif | Bmp | Tiff | Ico | Tga | Qoi)
    }

    /// Documents and text Veer can read as structured text (headings,
    /// paragraphs, lists) and write back out in another of these formats.
    pub fn is_document_text(self) -> bool {
        matches!(self, Txt | Md | Html | Docx | Rtf)
    }

    /// Plain-text formats whose bytes are the text itself.
    pub fn is_plain_text(self) -> bool {
        matches!(
            self,
            Txt | Md | Html | Srt | Vtt | Csv | Tsv | Json | Svg | Rtf
        )
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
        return Ok(Folder);
    }
    let mut header = Vec::with_capacity(SNIFF_LEN);
    File::open(path)?
        .take(SNIFF_LEN as u64)
        .read_to_end(&mut header)?;
    let by_ext = Format::from_path(path);
    let unsupported = || {
        Error::UnsupportedInput(format!(
            "{} isn't a file type Veer can work with",
            display_name(path)
        ))
    };
    if header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06") {
        return classify_zip(path, by_ext).ok_or_else(unsupported);
    }
    if let Some(f) = sniff(&header, by_ext) {
        return Ok(f);
    }
    match by_ext {
        // Formats with no reliable signature are trusted by extension.
        Some(f @ (Tga | Txt | Md | Html | Srt | Vtt | Csv | Tsv | Json | Svg)) => Ok(f),
        _ => Err(unsupported()),
    }
}

/// A ZIP container: a Word document, a plain archive, or something built
/// on ZIP that Veer doesn't handle (spreadsheets, slides, e-books, apps).
fn classify_zip(path: &Path, by_ext: Option<Format>) -> Option<Format> {
    let file = File::open(path).ok()?;
    let names: Vec<String> = match zip::ZipArchive::new(file) {
        Ok(z) => z
            .file_names()
            .take(10_000)
            .filter_map(|n| n.ok().map(|n| n.into_owned()))
            .collect(),
        // Damaged or not really a ZIP: trust only a .zip name.
        Err(_) => return (by_ext == Some(Zip)).then_some(Zip),
    };
    let has = |n: &str| names.iter().any(|x| x == n);
    if has("word/document.xml") {
        return Some(Docx);
    }
    let container = has("[Content_Types].xml")
        || has("mimetype")
        || has("META-INF/MANIFEST.MF")
        || has("AndroidManifest.xml");
    match by_ext {
        Some(Zip) => Some(Zip),
        // Something else built on ZIP, named as such: leave it alone.
        _ if container => None,
        None => Some(Zip),
        Some(_) => None,
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
    let weak_ok = !matches!(
        hint,
        Some(Txt | Md | Html | Srt | Vtt | Csv | Tsv | Json | Svg | Rtf)
    );
    if starts(b"{\\rtf") {
        return Some(Rtf);
    }
    if is_svg(h) {
        return Some(Svg);
    }
    // Compressed SVG.
    if starts(&[0x1F, 0x8B]) && hint == Some(Svg) {
        return Some(Svg);
    }
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

/// True when the first element of an XML-ish document is `<svg`, after
/// any byte-order mark, XML declaration, comments and doctype.
fn is_svg(h: &[u8]) -> bool {
    let mut rest = h.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(h);
    for _ in 0..64 {
        let skip = rest.iter().take_while(|b| b.is_ascii_whitespace()).count();
        rest = &rest[skip..];
        if rest.starts_with(b"<?") {
            match find(rest, b"?>") {
                Some(i) => rest = &rest[i + 2..],
                None => return false,
            }
        } else if rest.starts_with(b"<!--") {
            match find(rest, b"-->") {
                Some(i) => rest = &rest[i + 3..],
                None => return false,
            }
        } else if rest.starts_with(b"<!") {
            // A doctype may carry an internal subset: <!DOCTYPE svg [ ... ]>
            let close = find(rest, b">");
            let subset = find(rest, b"[").filter(|&b| close.is_none_or(|c| b < c));
            let end = match subset {
                Some(_) => find(rest, b"]>").map(|i| i + 2),
                None => close.map(|i| i + 1),
            };
            match end {
                Some(i) => rest = &rest[i..],
                None => return false,
            }
        } else {
            return rest.starts_with(b"<svg")
                && rest
                    .get(4)
                    .is_some_and(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/'));
        }
    }
    false
}

fn find(h: &[u8], needle: &[u8]) -> Option<usize> {
    h.windows(needle.len()).position(|w| w == needle)
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
    fn documents_vectors_and_folders() {
        assert_eq!(sniff(b"{\\rtf1\\ansi hello}", None), Some(Rtf));
        let svgs: [&[u8]; 4] = [
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            b"\xEF\xBB\xBF<?xml version=\"1.0\"?>\n<!-- made by hand -->\n<svg>",
            b"<?xml version=\"1.0\"?><!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x.dtd\" [ <!ENTITY a \"b\"> ]><svg width=\"1\">",
            b"<!DOCTYPE svg><svg\n>",
        ];
        for s in svgs {
            assert_eq!(sniff(s, None), Some(Svg), "{}", String::from_utf8_lossy(s));
        }
        assert_eq!(sniff(b"<svgfoo>", None), None);
        assert_eq!(sniff(b"<!DOCTYPE html><html>", Some(Html)), None);

        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect(dir.path()).unwrap(), Folder);
        let csv = dir.path().join("t.csv");
        std::fs::write(&csv, "a,b\n1,2\n").unwrap();
        assert_eq!(detect(&csv).unwrap(), Csv);
        let json = dir.path().join("t.json");
        std::fs::write(&json, "{\"a\": 1}").unwrap();
        assert_eq!(detect(&json).unwrap(), Json);
        // ZIPs that are really something else aren't offered as archives.
        let fake = dir.path().join("sheet.xlsx");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&fake).unwrap());
            z.start_file(
                "[Content_Types].xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.finish().unwrap();
        }
        assert!(detect(&fake).is_err());
        let plain = dir.path().join("archive.zip");
        std::fs::copy(&fake, &plain).unwrap();
        assert_eq!(detect(&plain).unwrap(), Zip);
        assert_eq!(Folder.extension(), "");
        assert_eq!(Format::parse("docx"), Some(Docx));
        assert_eq!(Docx.category(), Category::Document);
        assert_eq!(Csv.category().symbol(), "tablecells");
    }

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
            if *f != Folder {
                assert_eq!(Format::parse(f.extension()), Some(*f));
            }
        }
    }
}
