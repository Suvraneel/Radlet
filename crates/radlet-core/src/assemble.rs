//! Actions that make something new out of several files: clips joined end
//! to end, subtitles drawn into videos, and photos of paper pages made into
//! a scanned PDF.

use std::path::{Path, PathBuf};

use crate::engines::{avmedia, media, text, vision};
use crate::error::{Error, Result};
use crate::format::{self, display_name, Category, Format};
use crate::naming;
use crate::pipeline::{Context, Produced};

/// Most photos in one scan.
const MAX_SCAN_PAGES: usize = 500;

/// Joins the selected clips, in order, into one file beside the first:
/// videos into an MP4, sound into an M4A.
pub fn join_media(inputs: &[PathBuf], ctx: &mut Context<'_>) -> Result<Produced> {
    if inputs.len() < 2 {
        return Err(Error::Invalid("choose two or more clips to join".into()));
    }
    let formats = inputs
        .iter()
        .map(|p| format::detect(p))
        .collect::<Result<Vec<_>>>()?;
    let video = formats.iter().all(|f| f.category() == Category::Video);
    let audio = formats.iter().all(|f| f.category() == Category::Audio);
    if !video && !audio {
        return Err(Error::UnsupportedInput(
            "join videos with videos, or sound with sound".into(),
        ));
    }
    let to = if video { Format::Mp4 } else { Format::M4a };
    let first = &inputs[0];
    let dir = ctx.output.dir_for(first);
    let tmp = naming::temp_in(&dir, ctx.temp_tag, to.extension())?;
    if ctx.caps.media() {
        media::join(
            ctx.caps,
            inputs,
            video,
            tmp.path(),
            ctx.cancel,
            &mut *ctx.progress,
        )?;
    } else if ctx.caps.native_media() {
        if let Some((p, f)) = inputs
            .iter()
            .zip(&formats)
            .find(|(_, f)| !avmedia::reads(**f))
        {
            return Err(Error::Missing(format!(
                "joining {} files needs FFmpeg (brew install ffmpeg); {} is one",
                f.label(),
                display_name(p)
            )));
        }
        avmedia::join(
            ctx.caps,
            inputs,
            to,
            tmp.path(),
            ctx.cancel,
            &mut *ctx.progress,
        )?;
    } else {
        return Err(Error::FfmpegMissing);
    }
    naming::inherit_permissions(first, tmp.path());
    let desired = naming::desired_path(first, &dir, to.extension(), Some("joined"));
    let out = naming::publish(tmp.into_temp_path(), &desired)?;
    Ok(Produced::one(out))
}

/// The name a subtitle file shares with its video: "film" for film.srt,
/// film.en.srt and film.forced.en.vtt.
fn base_name(p: &Path) -> String {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name.split('.').next().unwrap_or_default().to_string()
}

/// A file and what it is.
type Typed<'a> = (&'a PathBuf, Format);

/// Pairs each video with its subtitles: the file named like it, or, for
/// one video, the only subtitle file chosen.
fn pair_subtitles<'a>(
    videos: &[Typed<'a>],
    subs: &[Typed<'a>],
) -> Result<Vec<(Typed<'a>, Typed<'a>)>> {
    let mut pairs = Vec::new();
    for &(video, vf) in videos {
        let stem = video
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let named = subs.iter().copied().find(|(s, _)| {
            let name = s
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            name.starts_with(&format!("{stem}.")) || base_name(s) == stem
        });
        let chosen = named.or_else(|| (subs.len() == 1 && videos.len() == 1).then(|| subs[0]));
        let Some(sub) = chosen else {
            return Err(Error::Invalid(format!(
                "no subtitles are named like {}. Name them after the video, or choose one video at a time",
                display_name(video)
            )));
        };
        pairs.push(((video, vf), sub));
    }
    Ok(pairs)
}

/// Each selected video with its subtitles drawn into the picture, beside
/// it as "<video> (subtitled).mp4".
pub fn burn_subtitles(inputs: &[PathBuf], ctx: &mut Context<'_>) -> Result<Produced> {
    let mut videos = Vec::new();
    let mut subs = Vec::new();
    for p in inputs {
        let f = format::detect(p)?;
        if f.category() == Category::Video {
            videos.push((p, f));
        } else if f.is_subtitles() {
            subs.push((p, f));
        } else {
            return Err(Error::UnsupportedInput(format!(
                "{} is neither a video nor subtitles",
                display_name(p)
            )));
        }
    }
    if videos.is_empty() || subs.is_empty() {
        return Err(Error::Invalid(
            "choose a video and its subtitles (SRT, VTT, ASS, SBV or TTML)".into(),
        ));
    }
    let pairs = pair_subtitles(&videos, &subs)?;
    let caps = ctx.caps;
    let ffmpeg = caps
        .ffmpeg
        .as_ref()
        .is_some_and(|f| media::has_filter(f, "subtitles"));
    let native = caps.avfoundation && caps.vision.is_some();
    let count = pairs.len();
    let mut outputs = Vec::with_capacity(count);
    for (i, ((video, vf), (sub, sf))) in pairs.into_iter().enumerate() {
        if ctx.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let use_native = !ffmpeg && native && avmedia::reads(vf);
        if !ffmpeg && !use_native {
            return Err(match &caps.ffmpeg {
                Some(_) => Error::Missing(
                    "this FFmpeg can't draw subtitles: it was built without libass. \
                     Install one that has it (brew install ffmpeg)"
                        .into(),
                ),
                None if native => Error::Missing(format!(
                    "drawing subtitles into {} video needs FFmpeg (brew install ffmpeg)",
                    vf.label()
                )),
                None => Error::FfmpegMissing,
            });
        }
        let cues = text::read_cues(&text::read_text(sub)?, sf)?;
        if cues.is_empty() {
            return Err(Error::NothingToDo(format!(
                "{} has no lines",
                display_name(sub)
            )));
        }
        let to = if use_native && vf == Format::Mov {
            Format::Mov
        } else {
            Format::Mp4
        };
        let dir = ctx.output.dir_for(video);
        let scratch = tempfile::tempdir()?;
        let tmp = naming::temp_in(&dir, ctx.temp_tag, to.extension())?;
        let mut progress = |f: f32| (ctx.progress)((i as f32 + f) / count as f32);
        if use_native {
            let lines: Vec<serde_json::Value> = cues
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "s": c.start_ms as f64 / 1000.0,
                        "e": c.end_ms as f64 / 1000.0,
                        "t": text::strip_tags(&c.text, &[]),
                    })
                })
                .collect();
            let list = scratch.path().join("cues.json");
            std::fs::write(
                &list,
                serde_json::to_vec(&lines).map_err(|e| Error::Internal(e.to_string()))?,
            )?;
            avmedia::burn(
                caps,
                video,
                &list,
                to,
                tmp.path(),
                ctx.cancel,
                &mut progress,
            )?;
        } else {
            // An ASS file keeps its own styling; anything else goes as SRT.
            let name = if sf == Format::Ass {
                "subs.ass"
            } else {
                "subs.srt"
            };
            let body = if sf == Format::Ass {
                text::read_text(sub)?
            } else {
                text::write_srt(&cues)
            };
            std::fs::write(scratch.path().join(name), body)?;
            media::burn(
                caps,
                video,
                scratch.path(),
                name,
                tmp.path(),
                ctx.cancel,
                &mut progress,
            )?;
        }
        naming::inherit_permissions(video, tmp.path());
        let desired = naming::desired_path(video, &dir, to.extension(), Some("subtitled"));
        outputs.push(naming::publish(tmp.into_temp_path(), &desired)?);
    }
    Ok(Produced::many(outputs))
}

/// The selected photos of paper pages, in order, as one scanned PDF
/// beside the first.
pub fn scan_documents(inputs: &[PathBuf], ctx: &mut Context<'_>) -> Result<Produced> {
    let first = inputs
        .first()
        .ok_or_else(|| Error::Invalid("choose photos of the pages to scan".into()))?;
    if inputs.len() > MAX_SCAN_PAGES {
        return Err(Error::TooLarge(format!(
            "scan at most {MAX_SCAN_PAGES} pages at a time"
        )));
    }
    for p in inputs {
        let f = format::detect(p)?;
        if !ctx.caps.can_decode_raster(f) || f == Format::Svg {
            return Err(Error::UnsupportedInput(format!(
                "{} isn't a photo",
                display_name(p)
            )));
        }
    }
    if ctx.caps.vision.is_none() {
        return Err(Error::Missing(
            "scanning pages needs the Radlet app on macOS".into(),
        ));
    }
    let dir = ctx.output.dir_for(first);
    let tmp = naming::temp_in(&dir, ctx.temp_tag, "pdf")?;
    vision::scan(ctx.caps, inputs, tmp.path(), ctx.cancel, &mut *ctx.progress)?;
    naming::inherit_permissions(first, tmp.path());
    let desired = naming::desired_path(first, &dir, "pdf", Some("scanned"));
    let out = naming::publish(tmp.into_temp_path(), &desired)?;
    Ok(Produced::one(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitles_find_their_videos() {
        let (a, b, s1, s2) = (
            PathBuf::from("/x/Holiday.mov"),
            PathBuf::from("/x/talk.mp4"),
            PathBuf::from("/x/talk.en.srt"),
            PathBuf::from("/x/holiday.vtt"),
        );
        let videos = [(&a, Format::Mov), (&b, Format::Mp4)];
        let subs = [(&s1, Format::Srt), (&s2, Format::Vtt)];
        let pairs = pair_subtitles(&videos, &subs).unwrap();
        assert_eq!(pairs[0].1 .0, &s2);
        assert_eq!(pairs[1].1 .0, &s1);
        // One subtitle file goes with one video whatever their names.
        let other = PathBuf::from("/x/captions.srt");
        let pairs = pair_subtitles(&videos[..1], &[(&other, Format::Srt)]).unwrap();
        assert_eq!(pairs[0].1 .0, &other);
        // Two videos and an unrelated file: which goes where isn't clear.
        assert!(pair_subtitles(&videos, &[(&other, Format::Srt)]).is_err());
    }
}
