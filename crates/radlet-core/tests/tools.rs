//! Joined clips, burned-in subtitles, size targets, scanned pages, text
//! blacked out of PDFs, and moving pictures, end to end. FFmpeg's routes
//! run when FFmpeg is installed; the Mac helper's are checked against a
//! stand-in that records what it was asked.

#![allow(clippy::cloned_ref_to_slice_refs)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use radlet_core::format::{detect, Format};
use radlet_core::job::{self, Outcome, Status};
use radlet_core::ops::{Action, CompressLevel, Step};
use radlet_core::{Capabilities, OutputLocation};

fn run(caps: &Capabilities, inputs: &[PathBuf], action: Action) -> Outcome {
    let units = job::plan(inputs, &action, &OutputLocation::BesideSource, "t");
    assert_eq!(units.len(), 1);
    job::execute(&units[0], caps, &AtomicBool::new(false), &mut |_| {})
}

fn done(o: &Outcome) -> PathBuf {
    assert_eq!(o.status, Status::Done, "{:?}", o.error);
    o.output.clone().expect("an output")
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Makes a test clip with FFmpeg: `w`×`h` picture (none when 0) for
/// `seconds`, with a tone when `sound`.
fn clip(ffmpeg: &Path, out: &Path, w: u32, h: u32, seconds: f64, sound: bool) {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-v", "error", "-y"]);
    if w > 0 {
        cmd.args(["-f", "lavfi", "-i"])
            .arg(format!("testsrc=size={w}x{h}:rate=25:duration={seconds}"));
    }
    if sound {
        cmd.args(["-f", "lavfi", "-i"])
            .arg(format!("sine=frequency=440:duration={seconds}"));
    }
    if w > 0 {
        cmd.args(["-pix_fmt", "yuv420p"]);
    }
    let status = cmd.arg(out).status().unwrap();
    assert!(status.success(), "couldn't make {}", out.display());
}

/// Length, picture size and whether there's sound, from FFprobe.
fn probe(ffprobe: &Path, file: &Path) -> (f64, Option<(u32, u32)>, bool) {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height",
            "-of",
            "json",
        ])
        .arg(file)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let seconds = v["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.0);
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let size = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .map(|s| {
            (
                s["width"].as_u64().unwrap() as u32,
                s["height"].as_u64().unwrap() as u32,
            )
        });
    let sound = streams.iter().any(|s| s["codec_type"] == "audio");
    (seconds, size, sound)
}

#[test]
fn clips_through_ffmpeg() {
    let caps = Capabilities::detect();
    let (Some(ffmpeg), Some(ffprobe)) = (caps.ffmpeg.clone(), caps.ffprobe.clone()) else {
        eprintln!("skipped: no FFmpeg");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.mp4");
    let b = dir.path().join("b.mp4");
    clip(&ffmpeg, &a, 320, 240, 1.0, true);
    // Upright, silent and a different shape: fitted in, with silence.
    clip(&ffmpeg, &b, 120, 240, 1.5, false);

    let joined = done(&run(&caps, &[a.clone(), b.clone()], Action::JoinMedia));
    assert_eq!(name(&joined), "a (joined).mp4");
    let (seconds, size, sound) = probe(&ffprobe, &joined);
    assert!((seconds - 2.5).abs() < 0.2, "{seconds} s");
    assert_eq!(size, Some((320, 240)));
    assert!(sound);

    // Sound joins into M4A.
    let x = dir.path().join("x.wav");
    let y = dir.path().join("y.wav");
    clip(&ffmpeg, &x, 0, 0, 1.0, true);
    clip(&ffmpeg, &y, 0, 0, 0.5, true);
    let m4a = done(&run(&caps, &[x.clone(), y.clone()], Action::JoinMedia));
    assert_eq!(name(&m4a), "x (joined).m4a");
    let (seconds, size, _) = probe(&ffprobe, &m4a);
    assert!((seconds - 1.5).abs() < 0.2, "{seconds} s");
    assert_eq!(size, None);
    // Videos and sound don't mix.
    let o = run(&caps, &[a.clone(), x.clone()], Action::JoinMedia);
    assert_ne!(o.status, Status::Done);

    if radlet_core::engines::media::has_filter(&ffmpeg, "subtitles") {
        let srt = dir.path().join("a.srt");
        std::fs::write(&srt, "1\n00:00:00,100 --> 00:00:00,900\n<i>Hello</i>\n\n").unwrap();
        let burned = done(&run(
            &caps,
            &[srt.clone(), a.clone()],
            Action::BurnSubtitles,
        ));
        assert_eq!(name(&burned), "a (subtitled).mp4");
        let (seconds, size, sound) = probe(&ffprobe, &burned);
        assert!((seconds - 1.0).abs() < 0.2, "{seconds} s");
        assert_eq!(size, Some((320, 240)));
        assert!(sound);
        // Two videos, each with its own subtitles.
        let srt_b = dir.path().join("b.en.srt");
        std::fs::write(&srt_b, "1\n00:00:00,000 --> 00:00:01,000\nHi\n\n").unwrap();
        let o = run(
            &caps,
            &[a.clone(), b.clone(), srt.clone(), srt_b],
            Action::BurnSubtitles,
        );
        assert_eq!(o.status, Status::Done, "{:?}", o.error);
        assert_eq!(o.outputs.len(), 2);
    }
}

/// A stand-in for the Mac helper: logs each call, copies the files it's
/// given to read beside the log, and writes something for each output.
#[cfg(unix)]
fn fake_mac(dir: &Path) -> (Capabilities, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let log = dir.join("helper.log");
    let script = dir.join("radlet-vision");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
d='{dir}'
case "$1" in
  media) echo "media $(basename "$2") $4 $5 $6" >> "$d/helper.log"; cp "$2" "$3";;
  join) out="$2"; to="$3"; shift 3; names=""; for f in "$@"; do names="$names $(basename "$f")"; done
        echo "join $to$names" >> "$d/helper.log"; cat "$@" > "$out";;
  burn) echo "burn $(basename "$2") $5" >> "$d/helper.log"; cp "$3" "$d/cues-seen.json"; cp "$2" "$4";;
  scan) out="$2"; shift 2; echo "scan $#" >> "$d/helper.log"; printf '%%PDF-1.4\n' > "$out";;
  redact-pdf) echo "redact $(basename "$2")" >> "$d/helper.log"; cp "$4" "$d/terms-seen.json"; cp "$2" "$3";;
  *) echo "unknown command" >&2; exit 2;;
esac
"#,
            dir = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let caps = Capabilities {
        vision: Some(script),
        avfoundation: true,
        ..Default::default()
    };
    (caps, log)
}

/// The first bytes of a QuickTime movie.
fn mov(path: &Path) {
    let mut bytes = vec![0, 0, 0, 0x14];
    bytes.extend_from_slice(b"ftypqt  \0\0\0\0qt  ");
    bytes.resize(64, 0);
    std::fs::write(path, bytes).unwrap();
}

/// A GIF of `n` frames, each 150 ms.
fn moving_gif(path: &Path, n: u8) {
    use image::codecs::gif::{GifEncoder, Repeat};
    let mut enc = GifEncoder::new(std::fs::File::create(path).unwrap());
    enc.set_repeat(Repeat::Infinite).unwrap();
    for i in 0..n {
        let img = image::RgbaImage::from_pixel(12, 8, image::Rgba([i * 80, 40, 200, 255]));
        enc.encode_frame(image::Frame::from_parts(
            img,
            0,
            0,
            image::Delay::from_numer_denom_ms(150, 1),
        ))
        .unwrap();
    }
}

#[cfg(unix)]
#[test]
fn video_tools_on_a_mac_without_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let (caps, log) = fake_mac(dir.path());
    let first = dir.path().join("first.mov");
    let second = dir.path().join("second.mov");
    mov(&first);
    mov(&second);

    let joined = done(&run(
        &caps,
        &[first.clone(), second.clone()],
        Action::JoinMedia,
    ));
    assert_eq!(name(&joined), "first (joined).mp4");

    let srt = dir.path().join("first.srt");
    std::fs::write(
        &srt,
        "1\n00:00:01,000 --> 00:00:02,500\n<i>Hello</i> there\n\n",
    )
    .unwrap();
    let burned = done(&run(&caps, &[first.clone(), srt], Action::BurnSubtitles));
    assert_eq!(name(&burned), "first (subtitled).mov");
    let cues: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("cues-seen.json")).unwrap()).unwrap();
    assert_eq!(
        cues,
        serde_json::json!([{ "s": 1.0, "e": 2.5, "t": "Hello there" }])
    );

    let fitted = done(&run(
        &caps,
        &[first.clone()],
        Action::single(Step::Compress {
            level: CompressLevel::Balanced,
            target_bytes: Some(19_000_000),
        }),
    ));
    assert!(name(&fitted).ends_with(".mp4"), "{}", name(&fitted));

    let gif = dir.path().join("moving.gif");
    moving_gif(&gif, 3);
    let video = done(&run(&caps, &[gif], Action::convert(Format::Mov)));
    assert_eq!(name(&video), "moving.mov");

    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("join mp4 first.mov second.mov"), "{calls}");
    assert!(calls.contains("burn first.mov mov"), "{calls}");
    assert!(calls.contains("mp4 target=19000000"), "{calls}");
    assert!(calls.contains("media moving.gif mov frames"), "{calls}");

    // A Mac doesn't play Matroska.
    let mkv = dir.path().join("film.mkv");
    std::fs::write(&mkv, [0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0]).unwrap();
    let o = run(&caps, &[first, mkv], Action::JoinMedia);
    assert_ne!(o.status, Status::Done);
}

#[cfg(unix)]
#[test]
fn scanning_and_blacking_out_text_through_the_helper() {
    let dir = tempfile::tempdir().unwrap();
    let (caps, log) = fake_mac(dir.path());
    let photos: Vec<PathBuf> = ["page 1.jpg", "page 2.png"]
        .iter()
        .map(|n| {
            let p = dir.path().join(n);
            image::RgbImage::from_pixel(30, 40, image::Rgb([250, 250, 245]))
                .save(&p)
                .unwrap();
            p
        })
        .collect();
    let scanned = done(&run(&caps, &photos, Action::ScanDocuments));
    assert_eq!(name(&scanned), "page 1 (scanned).pdf");
    assert_eq!(detect(&scanned).unwrap(), Format::Pdf);

    let md = dir.path().join("letter.md");
    std::fs::write(&md, "# Letter\n\nWrite to jane@example.com.\n").unwrap();
    let pdf = done(&run(
        &Capabilities::none(),
        &[md],
        Action::convert(Format::Pdf),
    ));
    let redact = |words: Vec<String>, emails: bool| {
        Action::single(Step::RedactText {
            words,
            emails,
            phones: false,
            cards: false,
        })
    };
    let blacked = done(&run(
        &caps,
        &[pdf.clone()],
        redact(vec![" Jane Doe ".into(), "".into()], true),
    ));
    assert_eq!(name(&blacked), "letter (redacted).pdf");
    let terms: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("terms-seen.json")).unwrap())
            .unwrap();
    assert_eq!(
        terms,
        serde_json::json!({ "words": ["Jane Doe"], "kinds": ["emails"] })
    );
    // Nothing to look for is a mistake, not an empty job.
    let o = run(&caps, &[pdf], redact(vec!["  ".into()], false));
    assert_ne!(o.status, Status::Done);

    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("scan 2"), "{calls}");
    assert!(calls.contains("redact letter.pdf"), "{calls}");

    let ids =
        |w: &radlet_core::catalog::Wheel| w.tools.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
    let w = radlet_core::catalog::wheel(&[Some(Format::Jpeg), Some(Format::Png)], &caps, &[]);
    assert!(ids(&w).contains(&"scan_document".to_string()));
    let w = radlet_core::catalog::wheel(&[Some(Format::Pdf)], &caps, &[]);
    assert_eq!(
        w.tools
            .iter()
            .find(|s| s.id == "redact_text")
            .and_then(|s| s.window),
        Some("redact_text")
    );
    let w = radlet_core::catalog::wheel(&[Some(Format::Pdf)], &Capabilities::none(), &[]);
    assert!(!ids(&w).contains(&"redact_text".to_string()));
}

#[test]
fn moving_pictures_keep_their_frames() {
    let dir = tempfile::tempdir().unwrap();
    let gif = dir.path().join("wave.gif");
    moving_gif(&gif, 4);
    let caps = Capabilities::none();
    let webp = done(&run(&caps, &[gif.clone()], Action::convert(Format::Webp)));
    let bytes = std::fs::read(&webp).unwrap();
    assert_eq!(&bytes[12..16], b"VP8X");
    assert_ne!(bytes[20] & 0x02, 0, "animated");
    let png = done(&run(&caps, &[webp], Action::convert(Format::Png)));
    let bytes = std::fs::read(&png).unwrap();
    assert!(bytes.windows(4).any(|w| w == b"acTL"), "an animated PNG");
    let back = done(&run(&caps, &[png], Action::convert(Format::Gif)));
    let frames = {
        use image::AnimationDecoder;
        let file = std::io::BufReader::new(std::fs::File::open(&back).unwrap());
        image::codecs::gif::GifDecoder::new(file)
            .unwrap()
            .into_frames()
            .count()
    };
    assert_eq!(frames, 4);
    // Steps that work on one picture don't silently drop the rest.
    let o = run(&caps, &[gif], Action::single(Step::resize_percent(50)));
    assert_ne!(o.status, Status::Done);
    assert!(
        o.error
            .as_ref()
            .unwrap()
            .message
            .contains("moving pictures"),
        "{:?}",
        o.error
    );
}
