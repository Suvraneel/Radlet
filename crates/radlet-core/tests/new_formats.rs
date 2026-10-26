//! Formats and routes added for 0.2: audio and video through the Mac
//! helper without FFmpeg, and barcodes through Vision.

#![allow(clippy::cloned_ref_to_slice_refs)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use radlet_core::format::{detect, Format};
use radlet_core::job::{self, Outcome, Status};
use radlet_core::ops::{Action, Step};
use radlet_core::{Capabilities, OutputLocation};

fn one_with(caps: &Capabilities, inputs: &[PathBuf], action: Action) -> Outcome {
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

/// A stand-in for the Mac helper's media and codes commands: media copies
/// its input and logs its arguments; codes finds one EAN-13.
#[cfg(unix)]
fn fake_mac(dir: &Path) -> (Capabilities, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let log = dir.join("helper.log");
    let script = dir.join("radlet-vision");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
case "$1" in
  media) shift; echo "media $(basename "$1") $3 $4 $5" >> '{log}'; echo "progress 0.5"; cp "$1" "$2";;
  codes) printf '[{{"kind":"EAN-13","value":"4006381333931"}}]' > "$3";;
  *) echo "unknown command" >&2; exit 2;;
esac
"#,
            log = log.display()
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

#[cfg(unix)]
#[test]
fn audio_and_video_without_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let (caps, log) = fake_mac(dir.path());
    let clip = dir.path().join("clip.mov");
    mov(&clip);
    assert_eq!(detect(&clip).unwrap(), Format::Mov);

    let mp4 = done(&one_with(
        &caps,
        &[clip.clone()],
        Action::convert(Format::Mp4),
    ));
    assert_eq!(name(&mp4), "clip.mp4");
    let sound = done(&one_with(
        &caps,
        &[clip.clone()],
        Action::convert(Format::M4a),
    ));
    assert_eq!(name(&sound), "clip.m4a");
    let trimmed = done(&one_with(
        &caps,
        &[clip.clone()],
        Action::single(Step::Trim {
            start: 1.0,
            end: Some(2.5),
        }),
    ));
    assert!(name(&trimmed).starts_with("clip"), "{}", name(&trimmed));
    let smaller = done(&one_with(
        &caps,
        &[clip.clone()],
        Action::single(Step::compress(radlet_core::ops::CompressLevel::Balanced)),
    ));
    assert!(name(&smaller).ends_with(".mp4"));
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("media clip.mov mp4"), "{calls}");
    assert!(calls.contains("media clip.mov m4a"), "{calls}");
    assert!(calls.contains("mov start=1.000 end=2.500"), "{calls}");
    assert!(calls.contains("mp4 size=1280x720"), "{calls}");

    // What a Mac can't play still needs FFmpeg.
    let mkv = dir.path().join("film.mkv");
    std::fs::write(&mkv, [0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0]).unwrap();
    let o = one_with(&caps, &[mkv], Action::convert(Format::Mp4));
    assert_ne!(o.status, Status::Done);
}

#[cfg(unix)]
#[test]
fn barcodes_through_vision() {
    let dir = tempfile::tempdir().unwrap();
    let (caps, _) = fake_mac(dir.path());
    let photo = dir.path().join("box.png");
    image::RgbImage::from_pixel(20, 20, image::Rgb([255, 255, 255]))
        .save(&photo)
        .unwrap();
    let o = one_with(&caps, &[photo.clone()], Action::ReadCodes);
    assert_eq!(o.status, Status::Done, "{:?}", o.error);
    assert_eq!(o.value.as_deref(), Some("4006381333931"));
    // Without the helper, a blank picture has no QR code.
    let o = one_with(&Capabilities::none(), &[photo], Action::ReadCodes);
    assert_ne!(o.status, Status::Done);
}
