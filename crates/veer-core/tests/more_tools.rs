//! Excel, folder contents, QR codes, icons, hidden faces and translation,
//! end to end.

#![allow(clippy::cloned_ref_to_slice_refs)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use veer_core::format::{detect, Category, Format};
use veer_core::job::{self, Outcome, Status};
use veer_core::ops::{Action, IconKind, RedactStyle, Step};
use veer_core::{Capabilities, OutputLocation};

fn run_with(caps: &Capabilities, inputs: &[PathBuf], action: Action) -> Vec<Outcome> {
    let units = job::plan(inputs, &action, &OutputLocation::BesideSource, "t");
    let cancel = AtomicBool::new(false);
    units
        .iter()
        .map(|u| job::execute(u, caps, &cancel, &mut |_| {}))
        .collect()
}

fn one_with(caps: &Capabilities, inputs: &[PathBuf], action: Action) -> Outcome {
    let mut all = run_with(caps, inputs, action);
    assert_eq!(all.len(), 1);
    all.remove(0)
}

fn one(inputs: &[PathBuf], action: Action) -> Outcome {
    one_with(&Capabilities::none(), inputs, action)
}

fn done(o: &Outcome) -> PathBuf {
    assert_eq!(o.status, Status::Done, "{:?}", o.error);
    o.output.clone().expect("an output")
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn excel_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("orders.csv");
    std::fs::write(&csv, "item,qty,zip\nTea,2,02134\nCoffee,10.5,10001\n").unwrap();
    let xlsx = done(&one(&[csv.clone()], Action::convert(Format::Xlsx)));
    assert_eq!(name(&xlsx), "orders.xlsx");
    assert_eq!(detect(&xlsx).unwrap(), Format::Xlsx);
    let back = done(&one(&[xlsx.clone()], Action::convert(Format::Tsv)));
    assert_eq!(
        std::fs::read_to_string(&back).unwrap(),
        "item\tqty\tzip\nTea\t2\t02134\nCoffee\t10.5\t10001\n"
    );

    // Several tables make one workbook; a workbook of several sheets makes
    // one CSV per sheet.
    let json = dir.path().join("people.json");
    std::fs::write(&json, r#"[{"name":"Ada","born":1815}]"#).unwrap();
    let book = done(&one(&[csv.clone(), json.clone()], Action::CombineWorkbook));
    assert_eq!(name(&book), "orders (combined).xlsx");
    let o = one(&[book.clone()], Action::convert(Format::Csv));
    assert_eq!(o.status, Status::Done, "{:?}", o.error);
    let mut names: Vec<String> = o.outputs.iter().map(|p| name(p)).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "orders (combined) - orders.csv",
            "orders (combined) - people.csv"
        ]
    );
    let people = o
        .outputs
        .iter()
        .find(|p| name(p).contains("people"))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(people).unwrap(),
        "name,born\nAda,1815\n"
    );

    let md = done(&one(&[book.clone()], Action::convert(Format::Md)));
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(
        text.contains("## people") && text.contains("| Ada | 1815 |"),
        "{text}"
    );
    let pdf = done(&one(&[book], Action::convert(Format::Pdf)));
    assert!(lopdf::Document::load(&pdf).is_ok());
}

#[test]
fn converting_what_is_inside_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("Trip");
    std::fs::create_dir_all(f.join("day 1")).unwrap();
    image::RgbImage::from_pixel(8, 6, image::Rgb([200, 10, 10]))
        .save(f.join("day 1/a.png"))
        .unwrap();
    image::RgbImage::from_pixel(8, 6, image::Rgb([10, 200, 10]))
        .save(f.join("b.bmp"))
        .unwrap();
    image::RgbImage::from_pixel(8, 6, image::Rgb([10, 10, 200]))
        .save(f.join("already.jpg"))
        .unwrap();
    std::fs::write(f.join("broken.png"), "not a picture").unwrap();
    std::fs::write(f.join("notes.txt"), "not an image").unwrap();
    // An icon Veer made earlier: a picture by name, but not one to convert.
    let logo =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9])));
    std::fs::write(
        f.join("logo.icns"),
        veer_core::engines::icons::icns(&logo).unwrap(),
    )
    .unwrap();

    let action = Action::ConvertInside {
        steps: vec![Step::Convert {
            to: Format::Jpeg,
            quality: None,
        }],
        only: Category::Image,
        label: Some("JPEG".into()),
    };
    let out = done(&one(&[f.clone()], action.clone()));
    assert_eq!(name(&out), "Trip (JPEG)");
    assert_eq!(detect(&out.join("day 1/a.jpg")).unwrap(), Format::Jpeg);
    assert!(out.join("b.jpg").is_file());
    assert!(!out.join("already.jpg").exists(), "already JPEG");
    assert!(!out.join("notes.txt").exists());
    let report = std::fs::read_to_string(out.join("Not converted.txt")).unwrap();
    assert!(report.starts_with("broken.png: "), "{report}");
    assert!(!report.contains("logo.icns"), "{report}");
    // The original is untouched.
    assert!(f.join("day 1/a.png").is_file() && !f.join("day 1/a.jpg").exists());

    let empty = dir.path().join("Empty");
    std::fs::create_dir(&empty).unwrap();
    assert_eq!(one(&[empty], action).status, Status::Skipped);
}

#[test]
fn documents_inside_a_folder_become_pdfs() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("Notes");
    std::fs::create_dir(&f).unwrap();
    std::fs::write(f.join("a.md"), "# A\n\nHello.\n").unwrap();
    std::fs::write(f.join("b.txt"), "Plain text.\n").unwrap();
    std::fs::write(f.join("c.csv"), "x,y\n1,2\n").unwrap();
    let paths = [f.clone()];
    let w =
        veer_core::catalog::wheel_for(&paths, &[Some(Format::Folder)], &Capabilities::none(), &[]);
    let slot = w
        .convert
        .iter()
        .find(|s| s.id == "inside_pdf")
        .expect("offered");
    let out = done(&one(&[f], slot.action.clone()));
    assert!(lopdf::Document::load(out.join("a.pdf")).is_ok());
    assert!(out.join("b.pdf").is_file());
    assert!(!out.join("c.pdf").exists(), "tables aren't documents");
}

#[test]
fn qr_codes_and_icons() {
    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("wifi.txt");
    std::fs::write(&txt, "WIFI:S:Home;T:WPA;P:secret;;\n").unwrap();
    let png = done(&one(&[txt], Action::MakeQr));
    assert_eq!(name(&png), "wifi (QR code).png");
    let read = one(&[png.clone()], Action::ReadCodes);
    assert_eq!(read.value.as_deref(), Some("WIFI:S:Home;T:WPA;P:secret;;"));
    let plain = dir.path().join("plain.png");
    image::RgbImage::from_pixel(40, 40, image::Rgb([255, 255, 255]))
        .save(&plain)
        .unwrap();
    assert_eq!(
        one(&[plain.clone()], Action::ReadCodes).status,
        Status::Skipped
    );

    let svg = dir.path().join("logo.svg");
    std::fs::write(&svg, r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#f60"/></svg>"##).unwrap();
    let fav = done(&one(
        &[svg.clone()],
        Action::IconSet {
            set: IconKind::Favicon,
        },
    ));
    assert_eq!(name(&fav), "logo (favicons)");
    for f in [
        "favicon.ico",
        "favicon.svg",
        "apple-touch-icon.png",
        "site.webmanifest",
        "head.html",
    ] {
        assert!(fav.join(f).is_file(), "{f}");
    }
    let ios = done(&one(
        &[plain.clone()],
        Action::IconSet { set: IconKind::Ios },
    ));
    assert!(ios.join("AppIcon.appiconset/Contents.json").is_file());
    let icns = done(&one(&[svg], Action::convert(Format::Icns)));
    assert_eq!(name(&icns), "logo.icns");
    assert!(std::fs::read(&icns).unwrap().starts_with(b"icns"));
}

/// A stand-in for the Mac helper: one face in the middle, and
/// "translation" into capitals.
#[cfg(unix)]
fn fake_helper(dir: &Path) -> Capabilities {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("veer-vision");
    std::fs::write(
        &script,
        r#"#!/bin/sh
case "$1" in
  faces) printf '[{"x":0.4,"y":0.4,"w":0.2,"h":0.2}]' > "$3";;
  translate) tr '[:lower:]' '[:upper:]' < "$4" > "$5";;
  *) echo "unknown command" >&2; exit 2;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    Capabilities {
        vision: Some(script),
        ..Default::default()
    }
}

#[cfg(unix)]
#[test]
fn faces_and_translation_through_the_helper() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fake_helper(dir.path());
    let photo = dir.path().join("group.png");
    image::RgbImage::from_fn(100, 100, |x, y| {
        image::Rgb([(x * 2) as u8, (y * 2) as u8, 128])
    })
    .save(&photo)
    .unwrap();
    let hidden = done(&one_with(
        &caps,
        &[photo.clone()],
        Action::single(Step::BlurFaces {
            style: RedactStyle::Black,
        }),
    ));
    assert_eq!(name(&hidden), "group (faces hidden).png");
    let img = image::open(&hidden).unwrap().to_rgb8();
    assert_eq!(img.get_pixel(50, 50).0, [0, 0, 0], "the face is covered");
    assert_eq!(img.get_pixel(5, 5).0, [10, 10, 128], "the rest isn't");

    let md = dir.path().join("note.md");
    std::fs::write(&md, "# Hello\n\nSee *you* soon.\n\n```\nkeep me\n```\n").unwrap();
    let fr = done(&one_with(
        &caps,
        &[md],
        Action::single(Step::Translate {
            to: "fr".into(),
            from: None,
        }),
    ));
    assert_eq!(name(&fr), "note (fr).md");
    let text = std::fs::read_to_string(&fr).unwrap();
    assert!(text.starts_with("# HELLO\n\nSEE YOU SOON."), "{text}");
    assert!(text.contains("keep me"), "code isn't translated");

    let srt = dir.path().join("talk.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,000\nhello there\n\n").unwrap();
    let out = done(&one_with(
        &caps,
        &[srt],
        Action::single(Step::Translate {
            to: "de".into(),
            from: Some("en".into()),
        }),
    ));
    assert!(std::fs::read_to_string(&out)
        .unwrap()
        .contains("00:00:01,000 --> 00:00:02,000\nHELLO THERE"));

    // Without the helper, neither is offered.
    let w = veer_core::catalog::wheel(&[Some(Format::Png)], &Capabilities::none(), &[]);
    assert!(!w.tools.iter().any(|s| s.id == "blur_faces"));
    let w = veer_core::catalog::wheel(&[Some(Format::Md)], &caps, &[]);
    assert_eq!(
        w.tools
            .iter()
            .find(|s| s.id == "translate")
            .and_then(|s| s.window),
        Some("translate")
    );
}
