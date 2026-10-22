//! Which conversions and tools are possible, given the formats in a
//! selection and the helper programs available on this machine.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;

use crate::format::{Category, Format};
use crate::ops::{Action, CompressLevel, Rect, SplitMode, Step};
use crate::recipe::Recipe;

/// Optional external programs. Everything else is built in.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Capabilities {
    /// FFmpeg, for audio and video.
    pub ffmpeg: Option<PathBuf>,
    /// FFprobe, for progress and lossless remuxing. Optional even with FFmpeg.
    pub ffprobe: Option<PathBuf>,
    /// macOS `sips`, for HEIC and AVIF.
    pub sips: Option<PathBuf>,
    /// The PDFium library, for turning PDF pages into images.
    #[serde(default)]
    pub pdfium: Option<PathBuf>,
    /// The app's `veer-vision` helper (macOS), for text recognition,
    /// background removal and searchable PDFs.
    #[serde(default)]
    pub vision: Option<PathBuf>,
}

impl Capabilities {
    /// Looks for helper programs: `VEER_FFMPEG`/`VEER_FFPROBE` overrides,
    /// next to the running binary (bundled builds), then `PATH` and the
    /// usual Homebrew locations. `VEER_NO_FFMPEG=1` disables media support.
    pub fn detect() -> Capabilities {
        let no_ffmpeg = std::env::var_os("VEER_NO_FFMPEG").is_some_and(|v| v == "1");
        let ffmpeg = if no_ffmpeg {
            None
        } else {
            find_tool("ffmpeg", "VEER_FFMPEG")
        };
        let ffprobe = if ffmpeg.is_some() {
            find_tool("ffprobe", "VEER_FFPROBE")
        } else {
            None
        };
        let sips = Some(PathBuf::from("/usr/bin/sips")).filter(|p| is_executable(p));
        let no_vision = std::env::var_os("VEER_NO_VISION").is_some_and(|v| v == "1");
        let vision = if no_vision {
            None
        } else {
            find_tool("veer-vision", "VEER_VISION")
        };
        Capabilities {
            ffmpeg,
            ffprobe,
            sips,
            pdfium: find_pdfium(),
            vision,
        }
    }

    /// Process-wide capabilities, detected once.
    pub fn global() -> &'static Capabilities {
        static CAPS: OnceLock<Capabilities> = OnceLock::new();
        CAPS.get_or_init(Capabilities::detect)
    }

    pub fn none() -> Capabilities {
        Capabilities::default()
    }

    pub fn media(&self) -> bool {
        self.ffmpeg.is_some()
    }

    /// Formats that can be decoded into pixels.
    pub fn can_decode_raster(&self, f: Format) -> bool {
        f.is_builtin_raster() || (self.sips.is_some() && matches!(f, Format::Heic | Format::Avif))
    }

    /// Formats that pixels can be encoded into.
    pub fn can_encode_raster(&self, f: Format) -> bool {
        f.is_builtin_raster() || (self.sips.is_some() && f == Format::Heic)
    }
}

fn find_tool(name: &str, env_key: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env_key).map(PathBuf::from) {
        return is_executable(&p).then_some(p);
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe_dir() {
        dirs.push(dir);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.push("/opt/homebrew/bin".into());
    dirs.push("/usr/local/bin".into());
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

/// PDFium: `VEER_PDFIUM`, or next to the running binary (the app bundles
/// it in Contents/Helpers), or in a sibling Frameworks folder.
fn find_pdfium() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("VEER_PDFIUM").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let name = if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };
    let exe_dir = exe_dir()?;
    [exe_dir.join(name), exe_dir.join("../Frameworks").join(name)]
        .into_iter()
        .find(|p| p.is_file())
}

/// The folder holding the real binary, with symlinks resolved, so a
/// `/usr/local/bin/veer` link into the app still finds its bundled
/// libraries and tools.
fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent().map(Path::to_path_buf)
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

use Format::*;

const IMAGE_TARGETS: &[Format] = &[Jpeg, Png, Webp, Heic, Pdf, Tiff, Gif, Bmp, Ico, Tga, Qoi];
const AUDIO_TARGETS: &[Format] = &[Mp3, M4a, Wav, Flac, Aiff, Ogg, Opus, Wma];
const VIDEO_TARGETS: &[Format] = &[Mp4, Mov, Webm, Mkv, Gif, Mp3, M4a, Avi, Wmv];

/// The formats `from` can become, most useful first.
pub fn targets(from: Format, caps: &Capabilities) -> Vec<Format> {
    let mut out: Vec<Format> = Vec::new();
    if caps.can_decode_raster(from) {
        out.extend(
            IMAGE_TARGETS
                .iter()
                .copied()
                .filter(|&t| t == Pdf || caps.can_encode_raster(t)),
        );
    }
    // Text recognition reads images too.
    if caps.vision.is_some() && caps.can_decode_raster(from) {
        out.push(Txt);
    }
    match from.category() {
        Category::Audio if caps.media() => out.extend_from_slice(AUDIO_TARGETS),
        Category::Video if caps.media() => out.extend_from_slice(VIDEO_TARGETS),
        _ => {}
    }
    if from == Gif && caps.media() {
        out.extend_from_slice(&[Mp4, Webm, Mov]);
    }
    match from {
        Pdf => {
            if caps.pdfium.is_some() {
                out.extend_from_slice(&[Jpeg, Png, Webp, Tiff]);
            }
            out.push(Txt);
        }
        Srt => out.extend_from_slice(&[Vtt, Txt]),
        Vtt => out.extend_from_slice(&[Srt, Txt]),
        Txt => out.extend_from_slice(&[Srt, Vtt]),
        Md => out.extend_from_slice(&[Html, Txt]),
        _ => {}
    }
    let mut seen = Vec::with_capacity(out.len());
    out.retain(|&t| {
        t != from && !seen.contains(&t) && {
            seen.push(t);
            true
        }
    });
    out
}

pub fn can_convert(from: Format, to: Format, caps: &Capabilities) -> bool {
    targets(from, caps).contains(&to)
}

/// Targets offered for a whole selection: every file must either convert
/// to the target or already be in it, and at least one file must change.
pub fn selection_targets(formats: &[Option<Format>], caps: &Capabilities) -> Vec<Format> {
    let known: Vec<Format> = match formats.iter().copied().collect::<Option<Vec<_>>>() {
        Some(k) if !k.is_empty() => k,
        _ => return Vec::new(),
    };
    let mut candidates: Vec<Format> = Vec::new();
    for f in &known {
        for t in targets(*f, caps) {
            if !candidates.contains(&t) {
                candidates.push(t);
            }
        }
    }
    candidates.retain(|&t| known.iter().all(|&f| f == t || can_convert(f, t, caps)));
    candidates
}

/// Which formats a step accepts, used for tools and recipe validation.
pub fn step_accepts(step: &Step, f: Format, caps: &Capabilities) -> bool {
    match step {
        Step::Convert { to, .. } => f == *to || can_convert(f, *to, caps),
        Step::Resize { .. } => caps.can_decode_raster(f) && caps.can_encode_raster(f),
        Step::Rotate { .. } => (caps.can_decode_raster(f) && caps.can_encode_raster(f)) || f == Pdf,
        Step::PageNumbers { .. } | Step::Watermark { .. } => f == Pdf,
        Step::Trim { .. } | Step::NormalizeAudio => {
            caps.media() && matches!(f.category(), Category::Audio | Category::Video)
        }
        Step::Crop { .. } | Step::Redact { .. } => {
            (caps.can_decode_raster(f) && caps.can_encode_raster(f))
                || (caps.media() && f.category() == Category::Video)
        }
        Step::Pages { .. } => f == Pdf,
        Step::StripMetadata => {
            caps.can_decode_raster(f) && caps.can_encode_raster(f)
                || f == Pdf
                || (caps.media() && matches!(f.category(), Category::Audio | Category::Video))
        }
        Step::Compress { target_bytes, .. } => {
            matches!(f, Jpeg | Png | Webp | Tiff | Bmp | Pdf)
                || (caps.sips.is_some() && f == Heic)
                // A size target works out a bitrate from the length, which
                // needs FFprobe.
                || (caps.media()
                    && f.category() == Category::Video
                    && (target_bytes.is_none() || caps.ffprobe.is_some()))
        }
        Step::RemoveBackground { .. } => caps.vision.is_some() && caps.can_decode_raster(f),
        Step::MakeSearchable => caps.vision.is_some() && (f == Pdf || caps.can_decode_raster(f)),
        Step::Transcribe { to, .. } => {
            caps.vision.is_some()
                && caps.media()
                && caps.ffprobe.is_some()
                && matches!(to, Srt | Vtt | Txt)
                && matches!(f.category(), Category::Audio | Category::Video)
        }
    }
}

/// Format a step produces from `f` (assuming it accepts it).
pub fn step_output(step: &Step, f: Format) -> Format {
    match step {
        Step::Convert { to, .. } => *to,
        // Transparency needs a format with an alpha channel.
        Step::RemoveBackground { .. } if matches!(f, Png | Webp | Tiff) => f,
        Step::RemoveBackground { .. } => Png,
        Step::MakeSearchable => Pdf,
        Step::Transcribe { to, .. } => *to,
        Step::Compress { .. } if f.category() == Category::Video => Mp4,
        Step::Compress { .. } if matches!(f, Tiff | Bmp) => Jpeg,
        // Video crops and redactions are re-encoded as H.264.
        Step::Crop { .. } | Step::Redact { .. } if f.category() == Category::Video => {
            if f == Mov {
                Mov
            } else {
                Mp4
            }
        }
        _ => f,
    }
}

/// True if every step in the chain can run, starting from `f`.
pub fn chain_accepts(steps: &[Step], mut f: Format, caps: &Capabilities) -> bool {
    if steps.is_empty() {
        return false;
    }
    for step in steps {
        if !step_accepts(step, f, caps) {
            return false;
        }
        f = step_output(step, f);
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotGroup {
    Convert,
    Tool,
    Recipe,
}

/// One segment of the wheel. The shell sends `action` back unchanged to
/// run it, so the wheel needs no knowledge of formats or tools.
#[derive(Debug, Clone, Serialize)]
pub struct Slot {
    pub id: String,
    pub label: String,
    /// SF Symbol name.
    pub symbol: String,
    pub group: SlotGroup,
    pub action: Action,
    /// A tool window the shell should open to set options before running
    /// (`resize`, `crop`, `redact`, `compress`, `split_pdf`, `pages_pdf`).
    /// `action` is then the default the window starts from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<&'static str>,
    /// Quick presets of this slot, most useful first. The wheel's Orbit
    /// style shows the first two on its outer ring and Bloom fans out up
    /// to four; the Dial ignores them. A variant runs straight away, never
    /// through a window, and works for every file the slot does.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
}

/// One preset of a slot: a short label for the wheel ("Q 95", "50 %"), a
/// line saying what it does, and the action to send back.
#[derive(Debug, Clone, Serialize)]
pub struct Variant {
    /// Unique on the wheel: the slot's id, a dot, then the preset's key.
    pub id: String,
    pub label: String,
    pub hint: String,
    pub action: Action,
}

impl Slot {
    fn with_variants(mut self, variants: Vec<Variant>) -> Slot {
        self.variants = variants;
        self
    }
}

fn variant(slot: &str, key: &str, label: &str, hint: impl Into<String>, action: Action) -> Variant {
    Variant {
        id: format!("{slot}.{key}"),
        label: label.into(),
        hint: hint.into(),
        action,
    }
}

fn steps(steps: Vec<Step>) -> Action {
    Action::Steps { steps, label: None }
}

const MB: u64 = 1_000_000;

fn fit_under(megabytes: u64) -> Action {
    Action::single(Step::Compress {
        level: CompressLevel::Balanced,
        target_bytes: Some(megabytes * MB),
    })
}

/// Presets for converting to `to`. Quality presets need every file to be
/// an image (PDF pages and video frames take other routes).
fn convert_variants(to: Format, id: &str, images: bool, combinable: bool) -> Vec<Variant> {
    let quality = |key: &str, label: &str, q: u8| {
        variant(
            id,
            key,
            label,
            format!("{} at quality {q}", to.label()),
            steps(vec![Step::Convert {
                to,
                quality: Some(q),
            }]),
        )
    };
    match to {
        Jpeg | Webp if images => vec![
            variant(
                id,
                "smart",
                "Smart",
                format!("{}, the smallest that looks the same", to.label()),
                steps(vec![
                    Step::Convert { to, quality: None },
                    Step::compress(CompressLevel::Smart),
                ]),
            ),
            quality("best", "Best", 95),
            quality("small", "Small", 60),
        ],
        Heic if images => vec![quality("best", "Best", 92), quality("small", "Small", 60)],
        Pdf if combinable => vec![variant(
            id,
            "combine",
            "One PDF",
            "Everything in one PDF, in order",
            Action::CombinePdf,
        )],
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Wheel {
    pub convert: Vec<Slot>,
    pub tools: Vec<Slot>,
    pub recipes: Vec<Slot>,
}

/// Builds the wheel for a selection. `None` entries are files Veer doesn't
/// recognise; they still get file-level tools such as checksums.
pub fn wheel(formats: &[Option<Format>], caps: &Capabilities, recipes: &[Recipe]) -> Wheel {
    let mut w = Wheel::default();
    if formats.is_empty() {
        return w;
    }
    let known: Option<Vec<Format>> = formats.iter().copied().collect();
    let every =
        |test: &dyn Fn(Format) -> bool| known.as_ref().is_some_and(|k| k.iter().all(|&f| test(f)));
    let images = every(&|f| caps.can_decode_raster(f));
    let combinable = known
        .as_ref()
        .is_some_and(|k| k.len() >= 2 && k.iter().all(|&f| f == Pdf || caps.can_decode_raster(f)));

    for t in selection_targets(formats, caps) {
        let id = format!("to_{}", t.id());
        let variants = convert_variants(t, &id, images, combinable);
        w.convert.push(Slot {
            id,
            label: t.label().to_string(),
            symbol: t.category().symbol().to_string(),
            group: SlotGroup::Convert,
            action: Action::convert(t),
            window: None,
            variants,
        });
    }

    let all = |step: &Step| every(&|f| step_accepts(step, f, caps));
    let tool =
        |id: &str, label: &str, symbol: &str, action: Action, window: Option<&'static str>| Slot {
            id: id.into(),
            label: label.into(),
            symbol: symbol.into(),
            group: SlotGroup::Tool,
            action,
            window,
            variants: Vec::new(),
        };
    let pdfs = every(&|f| f == Pdf);
    let single = formats.len() == 1;

    let half = Step::resize_percent(50);
    if all(&half) {
        w.tools.push(
            tool(
                "resize",
                "Resize…",
                "arrow.up.left.and.arrow.down.right",
                Action::single(half.clone()),
                Some("resize"),
            )
            .with_variants(vec![
                variant(
                    "resize",
                    "half",
                    "50 %",
                    "Half the size",
                    Action::single(half),
                ),
                variant(
                    "resize",
                    "1920",
                    "1920 px",
                    "Longest side at most 1920 pixels",
                    Action::single(Step::resize_max(1920)),
                ),
                variant(
                    "resize",
                    "quarter",
                    "25 %",
                    "A quarter of the size",
                    Action::single(Step::resize_percent(25)),
                ),
                variant(
                    "resize",
                    "1280",
                    "1280 px",
                    "Longest side at most 1280 pixels",
                    Action::single(Step::resize_max(1280)),
                ),
            ]),
        );
    }
    let crop = Step::Crop { rect: Rect::FULL };
    if all(&crop) {
        w.tools.push(tool(
            "crop",
            "Crop…",
            "crop",
            Action::single(crop),
            Some("crop"),
        ));
        let redact = Step::Redact {
            rects: vec![],
            style: Default::default(),
        };
        w.tools.push(tool(
            "redact",
            "Redact…",
            "rectangle.dashed.badge.record",
            Action::single(redact),
            Some("redact"),
        ));
    }
    let compress = Step::compress(CompressLevel::Balanced);
    if all(&compress) {
        let c = |key: &str, label: &str, hint: &str, action: Action| {
            variant("compress", key, label, hint, action)
        };
        let strong = Action::single(Step::compress(CompressLevel::Strong));
        let presets = if every(&|f| f.category() == Category::Video) {
            vec![
                c(
                    "whatsapp",
                    "WhatsApp",
                    "Each video under 15 MB, for WhatsApp",
                    fit_under(15),
                ),
                c(
                    "discord",
                    "Discord",
                    "Each video under 19 MB, for Discord",
                    fit_under(19),
                ),
                c(
                    "email",
                    "Email",
                    "Each video under 14 MB, for email",
                    fit_under(14),
                ),
                c("strong", "Strong", "As small as it goes", strong),
            ]
        } else if every(&|f| matches!(f, Jpeg | Webp)) {
            vec![
                c(
                    "smart",
                    "Smart",
                    "The smallest that still looks the same",
                    Action::single(Step::compress(CompressLevel::Smart)),
                ),
                c("1mb", "1 MB", "Each file under 1 MB", fit_under(1)),
                c(
                    "5mb",
                    "5 MB",
                    "Each file under 5 MB, for website uploads",
                    fit_under(5),
                ),
                c("strong", "Strong", "As small as it goes", strong),
            ]
        } else if images {
            vec![
                c("1mb", "1 MB", "Each file under 1 MB", fit_under(1)),
                c(
                    "5mb",
                    "5 MB",
                    "Each file under 5 MB, for website uploads",
                    fit_under(5),
                ),
                c("strong", "Strong", "As small as it goes", strong),
            ]
        } else {
            vec![
                c(
                    "email",
                    "Email",
                    "Each file under 14 MB, for email",
                    fit_under(14),
                ),
                c("strong", "Strong", "As small as it goes", strong),
            ]
        };
        w.tools.push(
            tool(
                "compress",
                "Compress…",
                "archivebox",
                Action::single(compress),
                Some("compress"),
            )
            .with_variants(presets),
        );
    }
    if all(&Step::Rotate { degrees: 90 }) {
        w.tools.push(
            tool(
                "rotate_right",
                "Rotate right",
                "rotate.right",
                Action::single(Step::Rotate { degrees: 90 }),
                None,
            )
            .with_variants(vec![variant(
                "rotate_right",
                "180",
                "180°",
                "Turn upside down",
                Action::single(Step::Rotate { degrees: 180 }),
            )]),
        );
        w.tools.push(tool(
            "rotate_left",
            "Rotate left",
            "rotate.left",
            Action::single(Step::Rotate { degrees: 270 }),
            None,
        ));
    }
    let trim = Step::Trim {
        start: 0.0,
        end: None,
    };
    if all(&trim) {
        w.tools.push(tool(
            "trim",
            "Trim…",
            "timeline.selection",
            Action::single(trim),
            Some("trim"),
        ));
    }
    let videos = caps.media()
        && known
            .as_ref()
            .is_some_and(|k| k.iter().all(|f| f.category() == Category::Video));
    if videos {
        w.tools.push(tool(
            "make_gif",
            "Make GIF…",
            "photo.stack",
            Action::single(Step::Convert {
                to: Gif,
                quality: None,
            }),
            Some("make_gif"),
        ));
        if caps.ffprobe.is_some() {
            let stills = |count: u32| Action::ExtractFrames {
                count,
                format: None,
            };
            w.tools.push(
                tool(
                    "extract_frames",
                    "Save stills",
                    "film.stack",
                    stills(10),
                    None,
                )
                .with_variants(vec![
                    variant(
                        "extract_frames",
                        "5",
                        "5 stills",
                        "5 evenly spaced stills",
                        stills(5),
                    ),
                    variant(
                        "extract_frames",
                        "30",
                        "30 stills",
                        "30 evenly spaced stills",
                        stills(30),
                    ),
                ]),
            );
        }
    }
    let subtitles = Step::Transcribe {
        to: Srt,
        language: None,
    };
    if all(&subtitles) {
        w.tools.push(tool(
            "make_subtitles",
            "Make subtitles…",
            "captions.bubble",
            Action::single(subtitles),
            Some("subtitles"),
        ));
    }
    if all(&Step::NormalizeAudio) {
        w.tools.push(tool(
            "normalize_audio",
            "Even out volume",
            "speaker.wave.2",
            Action::single(Step::NormalizeAudio),
            None,
        ));
    }
    if pdfs {
        let split = |mode: SplitMode| Action::SplitPdf { mode };
        w.tools.push(
            tool(
                "split_pdf",
                "Split…",
                "scissors",
                split(SplitMode::EveryPage),
                Some("split_pdf"),
            )
            .with_variants(vec![
                variant(
                    "split_pdf",
                    "pages",
                    "Every page",
                    "One PDF per page",
                    split(SplitMode::EveryPage),
                ),
                variant(
                    "split_pdf",
                    "pairs",
                    "Pairs",
                    "One PDF per two pages",
                    split(SplitMode::Chunks { size: 2 }),
                ),
            ]),
        );
        if single {
            w.tools.push(tool(
                "pages_pdf",
                "Pages…",
                "square.grid.2x2",
                Action::single(Step::Pages {
                    pages: vec![],
                    rotate: vec![],
                }),
                Some("pages_pdf"),
            ));
        }
        w.tools.push(tool(
            "page_numbers",
            "Page numbers…",
            "number.square",
            Action::single(Step::PageNumbers {
                position: Default::default(),
                style: Default::default(),
                start: 1,
                size: None,
                skip_first: false,
            }),
            Some("page_numbers"),
        ));
        let mark = |text: &str| {
            Action::single(Step::Watermark {
                text: text.into(),
                opacity: 0.15,
                size: None,
                diagonal: true,
            })
        };
        w.tools.push(
            tool(
                "watermark",
                "Watermark…",
                "character.textbox",
                mark("DRAFT"),
                Some("watermark"),
            )
            .with_variants(vec![
                variant(
                    "watermark",
                    "draft",
                    "Draft",
                    "\"DRAFT\" across every page",
                    mark("DRAFT"),
                ),
                variant(
                    "watermark",
                    "copy",
                    "Copy",
                    "\"COPY\" across every page",
                    mark("COPY"),
                ),
                variant(
                    "watermark",
                    "sample",
                    "Sample",
                    "\"SAMPLE\" across every page",
                    mark("SAMPLE"),
                ),
            ]),
        );
        w.tools.push(tool(
            "pdf_password",
            "Password…",
            "lock.doc",
            Action::ProtectPdf {
                password: Default::default(),
            },
            Some("pdf_password"),
        ));
    }
    let cutout = Step::RemoveBackground { crop: false };
    if all(&cutout) {
        w.tools.push(
            tool(
                "remove_background",
                "Remove background",
                "person.and.background.dotted",
                Action::single(cutout),
                None,
            )
            .with_variants(vec![variant(
                "remove_background",
                "crop",
                "And crop",
                "Remove the background and crop to the subject",
                Action::single(Step::RemoveBackground { crop: true }),
            )]),
        );
    }
    if all(&Step::MakeSearchable) {
        w.tools.push(tool(
            "copy_text",
            "Copy text",
            "text.viewfinder",
            Action::CopyText,
            None,
        ));
        w.tools.push(tool(
            "make_searchable",
            "Make searchable PDF",
            "doc.text.magnifyingglass",
            Action::single(Step::MakeSearchable),
            None,
        ));
    }
    if all(&Step::StripMetadata) {
        // Opens a report of what the files reveal; without that window
        // it removes the metadata straight away.
        w.tools.push(
            tool(
                "privacy_check",
                "Privacy check…",
                "hand.raised",
                Action::single(Step::StripMetadata),
                Some("privacy"),
            )
            .with_variants(vec![variant(
                "privacy_check",
                "strip",
                "Remove all",
                "Remove location, camera and other metadata now",
                Action::single(Step::StripMetadata),
            )]),
        );
        w.tools.push(tool(
            "strip_metadata",
            "Remove metadata",
            "eye.slash",
            Action::single(Step::StripMetadata),
            None,
        ));
    }
    if combinable {
        w.tools.push(tool(
            "combine_pdf",
            "Combine into PDF",
            "doc.on.doc",
            Action::CombinePdf,
            None,
        ));
    }
    w.tools.push(tool(
        "checksum",
        "Copy SHA-256",
        "number",
        Action::Checksum,
        None,
    ));

    // Offer only presets every file can take (a size target on video
    // needs FFprobe, for one).
    let runnable = |v: &Variant| match &v.action {
        Action::Steps { steps, .. } => known
            .as_ref()
            .is_some_and(|k| k.iter().all(|&f| chain_accepts(steps, f, caps))),
        _ => true,
    };
    for slot in w.convert.iter_mut().chain(w.tools.iter_mut()) {
        slot.variants.retain(&runnable);
    }

    if let Some(k) = &known {
        for r in recipes {
            if k.iter().all(|&f| chain_accepts(&r.steps, f, caps)) {
                w.recipes.push(Slot {
                    id: format!("recipe_{}", r.slug()),
                    label: r.name.clone(),
                    symbol: "wand.and.stars".into(),
                    group: SlotGroup::Recipe,
                    action: r.action(),
                    window: None,
                    variants: Vec::new(),
                });
            }
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vision() -> Capabilities {
        Capabilities {
            vision: Some("/x/veer-vision".into()),
            ..Default::default()
        }
    }

    #[test]
    fn vision_tools_on_the_wheel() {
        let ids = |w: &Wheel| w.tools.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        let image = wheel(&[Some(Png)], &vision(), &[]);
        let tools = ids(&image);
        for id in ["remove_background", "copy_text", "make_searchable"] {
            assert!(tools.contains(&id.to_string()), "{id} in {tools:?}");
        }
        assert!(image.convert.iter().any(|s| s.id == "to_txt"));
        // More than twelve is fine: the app puts the rest under More….

        let pdf = ids(&wheel(&[Some(Pdf)], &vision(), &[]));
        assert!(
            pdf.contains(&"copy_text".to_string()) && pdf.contains(&"make_searchable".to_string())
        );
        assert!(!pdf.contains(&"remove_background".to_string()));

        let none = ids(&wheel(&[Some(Png)], &Capabilities::none(), &[]));
        assert!(!none
            .iter()
            .any(|t| t == "copy_text" || t == "remove_background"));

        assert_eq!(
            step_output(&Step::RemoveBackground { crop: false }, Jpeg),
            Png
        );
        assert_eq!(
            step_output(&Step::RemoveBackground { crop: false }, Webp),
            Webp
        );
        assert_eq!(step_output(&Step::MakeSearchable, Png), Pdf);
    }

    fn media() -> Capabilities {
        Capabilities {
            ffmpeg: Some("/x/ffmpeg".into()),
            ..Default::default()
        }
    }

    #[test]
    fn png_targets_without_helpers() {
        let t = targets(Png, &Capabilities::none());
        assert_eq!(t.first(), Some(&Jpeg));
        assert!(t.contains(&Webp) && t.contains(&Pdf));
        assert!(!t.contains(&Png), "never offers the source format");
        assert!(!t.contains(&Heic), "HEIC needs sips");
    }

    #[test]
    fn media_needs_ffmpeg() {
        assert!(targets(Mp3, &Capabilities::none()).is_empty());
        let t = targets(Mp4, &media());
        assert!(t.contains(&Gif) && t.contains(&Mp3) && !t.contains(&Mp4));
        let g = targets(Gif, &media());
        assert!(g.contains(&Png) && g.contains(&Mp4));
    }

    #[test]
    fn heic_with_sips() {
        let caps = Capabilities {
            sips: Some("/usr/bin/sips".into()),
            ..Default::default()
        };
        assert!(targets(Heic, &caps).contains(&Jpeg));
        assert!(targets(Png, &caps).contains(&Heic));
        assert!(targets(Heic, &Capabilities::none()).is_empty());
    }

    #[test]
    fn selection_rules() {
        let caps = Capabilities::none();
        let mixed = selection_targets(&[Some(Png), Some(Jpeg)], &caps);
        assert!(mixed.contains(&Jpeg), "PNG converts, JPEG already is");
        assert!(mixed.contains(&Webp));
        assert!(selection_targets(&[Some(Png), Some(Srt)], &caps).is_empty());
        assert!(selection_targets(&[Some(Png), None], &caps).is_empty());
    }

    #[test]
    fn wheel_for_images() {
        let w = wheel(&[Some(Png), Some(Jpeg)], &Capabilities::none(), &[]);
        let ids: Vec<&str> = w.tools.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "resize",
                "crop",
                "redact",
                "compress",
                "rotate_right",
                "rotate_left",
                "privacy_check",
                "strip_metadata",
                "combine_pdf",
                "checksum"
            ]
        );
        let windows: Vec<_> = w.tools.iter().filter_map(|s| s.window).collect();
        assert_eq!(windows, ["resize", "crop", "redact", "compress", "privacy"]);
        assert!(w.convert.iter().any(|s| s.id == "to_webp"));
    }

    #[test]
    fn pdf_tools_and_targets() {
        let w = wheel(&[Some(Pdf)], &Capabilities::none(), &[]);
        let ids: Vec<&str> = w.tools.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "compress",
                "rotate_right",
                "rotate_left",
                "split_pdf",
                "pages_pdf",
                "page_numbers",
                "watermark",
                "pdf_password",
                "privacy_check",
                "strip_metadata",
                "checksum"
            ]
        );
        let windows: Vec<_> = w.tools.iter().filter_map(|s| s.window).collect();
        assert_eq!(
            windows,
            [
                "compress",
                "split_pdf",
                "pages_pdf",
                "page_numbers",
                "watermark",
                "pdf_password",
                "privacy"
            ]
        );
        assert_eq!(targets(Pdf, &Capabilities::none()), vec![Txt]);
        let with_pdfium = Capabilities {
            pdfium: Some("/x/libpdfium.so".into()),
            ..Default::default()
        };
        assert_eq!(targets(Pdf, &with_pdfium), vec![Jpeg, Png, Webp, Tiff, Txt]);
        // Several PDFs: split each, combine, but no single-file page editor.
        let w = wheel(&[Some(Pdf), Some(Pdf)], &Capabilities::none(), &[]);
        let ids: Vec<&str> = w.tools.iter().map(|s| s.id.as_str()).collect();
        assert!(!ids.contains(&"pages_pdf"));
        assert!(ids.contains(&"split_pdf") && ids.contains(&"combine_pdf"));
    }

    #[test]
    fn video_tools_need_ffmpeg() {
        let w = wheel(&[Some(Mp4)], &media(), &[]);
        let ids: Vec<&str> = w.tools.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "crop",
                "redact",
                "compress",
                "trim",
                "make_gif",
                "normalize_audio",
                "privacy_check",
                "strip_metadata",
                "checksum"
            ]
        );
        // Stills need FFprobe to know the length.
        let probed = Capabilities {
            ffprobe: Some("/x/ffprobe".into()),
            ..media()
        };
        let w = wheel(&[Some(Mp4)], &probed, &[]);
        assert!(w.tools.iter().any(|s| s.id == "extract_frames"));
        let audio = wheel(&[Some(Mp3)], &media(), &[]);
        let ids: Vec<&str> = audio.tools.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "trim",
                "normalize_audio",
                "privacy_check",
                "strip_metadata",
                "checksum"
            ]
        );
        let crop = Step::Crop { rect: Rect::FULL };
        assert_eq!(step_output(&crop, Mov), Mov);
        assert_eq!(step_output(&crop, Webm), Mp4);
        assert!(!step_accepts(&crop, Mp4, &Capabilities::none()));
    }

    /// Every preset is unique, named after its slot, short enough for the
    /// outer ring, and runs on every file its slot does.
    fn check_variants(w: &Wheel, formats: &[Format], caps: &Capabilities) {
        let mut seen = std::collections::HashSet::new();
        for s in w.convert.iter().chain(&w.tools).chain(&w.recipes) {
            assert!(s.variants.len() <= 4, "{} has too many presets", s.id);
            for v in &s.variants {
                assert!(v.id.starts_with(&format!("{}.", s.id)), "{}", v.id);
                assert!(seen.insert(v.id.clone()), "duplicate {}", v.id);
                assert!(
                    !v.label.is_empty() && v.label.chars().count() <= 10,
                    "{}",
                    v.label
                );
                assert!(!v.hint.is_empty());
                if let Action::Steps { steps, .. } = &v.action {
                    for &f in formats {
                        assert!(chain_accepts(steps, f, caps), "{} on {f:?}", v.id);
                    }
                }
            }
        }
    }

    fn labels<'a>(w: &'a [Slot], id: &str) -> Vec<&'a str> {
        let slot = w
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("no {id}"));
        slot.variants.iter().map(|v| v.label.as_str()).collect()
    }

    #[test]
    fn presets_for_the_outer_ring() {
        let caps = Capabilities::none();
        let w = wheel(&[Some(Png), Some(Jpeg)], &caps, &[]);
        check_variants(&w, &[Png, Jpeg], &caps);
        assert_eq!(labels(&w.convert, "to_jpeg"), ["Smart", "Best", "Small"]);
        assert_eq!(labels(&w.convert, "to_pdf"), ["One PDF"]);
        assert!(labels(&w.convert, "to_tiff").is_empty());
        assert_eq!(
            labels(&w.tools, "resize"),
            ["50 %", "1920 px", "25 %", "1280 px"]
        );
        assert_eq!(labels(&w.tools, "compress"), ["1 MB", "5 MB", "Strong"]);
        assert_eq!(labels(&w.tools, "privacy_check"), ["Remove all"]);
        assert!(labels(&w.tools, "checksum").is_empty());

        // One file has nothing to combine; JPEGs alone get smart quality.
        let one = wheel(&[Some(Jpeg)], &caps, &[]);
        check_variants(&one, &[Jpeg], &caps);
        assert!(labels(&one.convert, "to_pdf").is_empty());
        assert_eq!(labels(&one.tools, "compress")[0], "Smart");

        // Size targets for video need FFprobe for the length.
        let video = wheel(&[Some(Mp4)], &media(), &[]);
        check_variants(&video, &[Mp4], &media());
        assert_eq!(labels(&video.tools, "compress"), ["Strong"]);
        let probed = Capabilities {
            ffprobe: Some("/x/ffprobe".into()),
            ..media()
        };
        let video = wheel(&[Some(Mp4)], &probed, &[]);
        check_variants(&video, &[Mp4], &probed);
        assert_eq!(
            labels(&video.tools, "compress"),
            ["WhatsApp", "Discord", "Email", "Strong"]
        );
        assert_eq!(
            labels(&video.tools, "extract_frames"),
            ["5 stills", "30 stills"]
        );
        assert!(video.convert.iter().all(|s| s.variants.is_empty()));
        let target = Step::Compress {
            level: CompressLevel::Balanced,
            target_bytes: Some(MB),
        };
        assert!(!step_accepts(&target, Mp4, &media()));
        assert!(step_accepts(&target, Mp4, &probed));
        assert!(step_accepts(&target, Jpeg, &media()));

        let pdfs = wheel(&[Some(Pdf), Some(Pdf)], &caps, &[]);
        check_variants(&pdfs, &[Pdf], &caps);
        assert_eq!(labels(&pdfs.tools, "split_pdf"), ["Every page", "Pairs"]);
        assert_eq!(
            labels(&pdfs.tools, "watermark"),
            ["Draft", "Copy", "Sample"]
        );
        assert_eq!(labels(&pdfs.tools, "compress"), ["Email", "Strong"]);

        // Sent only when there are some.
        let json = serde_json::to_value(&one.tools).unwrap();
        let checksum = json
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "checksum")
            .unwrap();
        assert!(checksum.get("variants").is_none());
        let resize = json
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "resize")
            .unwrap();
        assert_eq!(resize["variants"][1]["action"]["steps"][0]["max"], 1920);
        assert_eq!(resize["variants"][1]["id"], "resize.1920");
    }

    #[test]
    fn unknown_files_still_get_checksum() {
        let w = wheel(&[None], &Capabilities::none(), &[]);
        assert!(w.convert.is_empty());
        assert_eq!(w.tools.len(), 1);
        assert_eq!(w.tools[0].id, "checksum");
    }

    #[test]
    fn chains() {
        let caps = Capabilities::none();
        let web = [
            Step::resize_max(1600),
            Step::Convert {
                to: Webp,
                quality: Some(80),
            },
            Step::StripMetadata,
        ];
        assert!(chain_accepts(&web, Jpeg, &caps));
        assert!(!chain_accepts(&web, Pdf, &caps));
        assert!(!chain_accepts(&[], Jpeg, &caps));
    }
}
