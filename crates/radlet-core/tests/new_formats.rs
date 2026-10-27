//! Formats and routes added for 0.2: audio and video through the Mac
//! helper without FFmpeg, barcodes through Vision, PDF to Word, YAML, TOML
//! and XML, OpenDocument spreadsheets, more subtitle formats, and 7-Zip,
//! RAR, TAR and disc images through bsdtar.

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

#[test]
fn pdf_to_word() {
    let dir = tempfile::tempdir().unwrap();
    let md = dir.path().join("letter.md");
    std::fs::write(&md, "# Dear Ada\n\nThe engine works.\n\n- gears\n- cards\n").unwrap();
    let caps = Capabilities::none();
    let pdf = done(&one_with(&caps, &[md], Action::convert(Format::Pdf)));
    let docx = done(&one_with(&caps, &[pdf], Action::convert(Format::Docx)));
    assert_eq!(name(&docx), "letter.docx");
    assert_eq!(detect(&docx).unwrap(), Format::Docx);
    let text = done(&one_with(&caps, &[docx], Action::convert(Format::Txt)));
    let text = std::fs::read_to_string(text).unwrap();
    assert!(text.contains("The engine works."), "{text}");
}

#[test]
fn config_files() {
    let dir = tempfile::tempdir().unwrap();
    let caps = Capabilities::none();
    let json = dir.path().join("settings.json");
    std::fs::write(&json, r#"{"name":"Radlet","port":8080,"tags":["a","b"]}"#).unwrap();
    let yaml = done(&one_with(
        &caps,
        &[json.clone()],
        Action::convert(Format::Yaml),
    ));
    assert_eq!(name(&yaml), "settings.yaml");
    let text = std::fs::read_to_string(&yaml).unwrap();
    assert!(text.contains("port: 8080"), "{text}");
    let toml = done(&one_with(&caps, &[yaml], Action::convert(Format::Toml)));
    assert!(std::fs::read_to_string(&toml)
        .unwrap()
        .contains("name = \"Radlet\""));
    let xml = done(&one_with(&caps, &[json], Action::convert(Format::Xml)));
    assert_eq!(detect(&xml).unwrap(), Format::Xml);
    let back = done(&one_with(&caps, &[xml], Action::convert(Format::Json)));
    assert!(std::fs::read_to_string(back)
        .unwrap()
        .contains("\"Radlet\""));
}

#[test]
fn opendocument_spreadsheets() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let ods = dir.path().join("budget.ods");
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&ods).unwrap());
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        z.start_file("mimetype", stored).unwrap();
        z.write_all(b"application/vnd.oasis.opendocument.spreadsheet")
            .unwrap();
        z.start_file("content.xml", stored).unwrap();
        z.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2">
<office:body><office:spreadsheet>
<table:table table:name="Budget">
<table:table-row><table:table-cell office:value-type="string"><text:p>item</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>cost</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell office:value-type="string"><text:p>tea</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="4.5"><text:p>4.5</text:p></table:table-cell></table:table-row>
</table:table>
</office:spreadsheet></office:body></office:document-content>"#,
        )
        .unwrap();
        z.start_file("META-INF/manifest.xml", stored).unwrap();
        z.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/></manifest:manifest>"#).unwrap();
        z.finish().unwrap();
    }
    assert_eq!(detect(&ods).unwrap(), Format::Ods);
    let caps = Capabilities::none();
    let csv = done(&one_with(
        &caps,
        &[ods.clone()],
        Action::convert(Format::Csv),
    ));
    assert_eq!(
        std::fs::read_to_string(csv).unwrap(),
        "item,cost\ntea,4.5\n"
    );
    let xlsx = done(&one_with(&caps, &[ods], Action::convert(Format::Xlsx)));
    assert_eq!(detect(&xlsx).unwrap(), Format::Xlsx);

    // A few hundred bytes describing millions of cells are refused before
    // they're expanded.
    let bomb = dir.path().join("bomb.ods");
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&bomb).unwrap());
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        z.start_file("mimetype", stored).unwrap();
        z.write_all(b"application/vnd.oasis.opendocument.spreadsheet")
            .unwrap();
        z.start_file("content.xml", stored).unwrap();
        z.write_all(br#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"><office:body><office:spreadsheet><table:table table:name="S"><table:table-row table:number-rows-repeated="2000"><table:table-cell table:number-columns-repeated="2000" office:value-type="float" office:value="1"/></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#).unwrap();
        z.finish().unwrap();
    }
    let o = one_with(&caps, &[bomb], Action::convert(Format::Csv));
    assert_ne!(o.status, Status::Done);
    assert!(
        format!("{:?}", o.error).contains("3 million"),
        "{:?}",
        o.error
    );
}

#[test]
fn subtitles_into_other_formats() {
    let dir = tempfile::tempdir().unwrap();
    let srt = dir.path().join("talk.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,000\nHello\n\n").unwrap();
    let caps = Capabilities::none();
    let ass = done(&one_with(
        &caps,
        &[srt.clone()],
        Action::convert(Format::Ass),
    ));
    assert_eq!(name(&ass), "talk.ass");
    assert!(std::fs::read_to_string(&ass)
        .unwrap()
        .contains("Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Hello"));
    let ttml = done(&one_with(&caps, &[ass], Action::convert(Format::Ttml)));
    let back = done(&one_with(&caps, &[ttml], Action::convert(Format::Vtt)));
    assert!(std::fs::read_to_string(back)
        .unwrap()
        .contains("00:00:01.000 --> 00:00:02.000\nHello"));
}

#[cfg(unix)]
#[test]
fn archives_through_bsdtar() {
    let caps = Capabilities::detect();
    if caps.bsdtar.is_none() {
        eprintln!("skipped: no bsdtar");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Photos");
    std::fs::create_dir_all(folder.join("2026")).unwrap();
    std::fs::write(folder.join("2026/a.txt"), "a").unwrap();
    let tgz = done(&one_with(
        &caps,
        &[folder.clone()],
        Action::convert(Format::Tar),
    ));
    assert_eq!(name(&tgz), "Photos.tar.gz");
    std::fs::remove_dir_all(&folder).unwrap();
    let out = done(&one_with(&caps, &[tgz], Action::convert(Format::Folder)));
    assert_eq!(name(&out), "Photos");
    assert_eq!(
        std::fs::read_to_string(out.join("2026/a.txt")).unwrap(),
        "a"
    );
}

#[test]
fn sqlite_tables_become_spreadsheets() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("shop.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE orders (id INTEGER PRIMARY KEY, item TEXT, qty REAL, photo BLOB);
             INSERT INTO orders (item, qty, photo) VALUES ('Tea', 2, x'00010203'), ('Coffee', 10.5, NULL);
             CREATE TABLE people (name TEXT);
             INSERT INTO people VALUES ('Ada');",
        )
        .unwrap();
    }
    assert_eq!(detect(&db).unwrap(), Format::Sqlite);
    let caps = Capabilities::none();
    let o = one_with(&caps, &[db.clone()], Action::convert(Format::Csv));
    assert_eq!(o.status, Status::Done, "{:?}", o.error);
    let orders = o
        .outputs
        .iter()
        .find(|p| name(p).contains("orders"))
        .expect("a CSV for the orders table");
    assert_eq!(
        std::fs::read_to_string(orders).unwrap(),
        "id,item,qty,photo\n1,Tea,2,\"(binary, 4 bytes)\"\n2,Coffee,10.5,\n"
    );
    let xlsx = done(&one_with(
        &caps,
        &[db.clone()],
        Action::convert(Format::Xlsx),
    ));
    assert_eq!(name(&xlsx), "shop.xlsx");
    // Reading never changes the database.
    let before = std::fs::read(&db).unwrap();
    let _ = one_with(&caps, &[db.clone()], Action::convert(Format::Json));
    assert_eq!(std::fs::read(&db).unwrap(), before);

    // A database copied with its write-ahead log keeps its latest rows;
    // columns worked out on reading are left out.
    let live = dir.path().join("live.db");
    let conn = rusqlite::Connection::open(&live).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;
         CREATE TABLE t (a TEXT, b TEXT GENERATED ALWAYS AS (upper(a)) VIRTUAL);
         INSERT INTO t (a) VALUES ('x');",
    )
    .unwrap();
    let copy = dir.path().join("copy.db");
    std::fs::copy(&live, &copy).unwrap();
    std::fs::copy(
        dir.path().join("live.db-wal"),
        dir.path().join("copy.db-wal"),
    )
    .unwrap();
    drop(conn);
    let csv = done(&one_with(
        &caps,
        &[copy.clone()],
        Action::convert(Format::Csv),
    ));
    assert_eq!(std::fs::read_to_string(&csv).unwrap(), "a\nx\n");
    assert!(
        dir.path().join("copy.db-wal").exists(),
        "the log is left as it was"
    );
}

#[test]
fn fonts_for_the_web_and_back() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fonts");
    let dir = tempfile::tempdir().unwrap();
    let caps = Capabilities::none();
    for (file, fmt) in [
        ("radlet-test.ttf", Format::Ttf),
        ("radlet-test.otf", Format::Otf),
        ("radlet-test.woff", Format::Woff),
        ("radlet-test.woff2", Format::Woff2),
    ] {
        let src = dir.path().join(file);
        std::fs::copy(fixtures.join(file), &src).unwrap();
        assert_eq!(detect(&src).unwrap(), fmt, "{file}");
    }
    let ttf = dir.path().join("radlet-test.ttf");
    let woff2 = dir.path().join("web.woff2");
    std::fs::copy(&ttf, dir.path().join("web.ttf")).unwrap();
    let made = done(&one_with(
        &caps,
        &[dir.path().join("web.ttf")],
        Action::convert(Format::Woff2),
    ));
    assert_eq!(name(&made), "web.woff2");
    std::fs::rename(&made, &woff2).unwrap();
    std::fs::remove_file(dir.path().join("web.ttf")).unwrap();
    let back = done(&one_with(&caps, &[woff2], Action::convert(Format::Ttf)));
    assert_eq!(detect(&back).unwrap(), Format::Ttf);
    // The same tables come back (the checksum adjustment aside).
    let a = std::fs::read(&ttf).unwrap();
    let b = std::fs::read(&back).unwrap();
    assert_eq!(a.len(), b.len());
    // A font with PostScript outlines can't pretend to be a TTF.
    let otf_woff = done(&one_with(
        &caps,
        &[dir.path().join("radlet-test.otf")],
        Action::convert(Format::Woff),
    ));
    let o = one_with(&caps, &[otf_woff], Action::convert(Format::Ttf));
    assert_ne!(o.status, Status::Done);
}

#[test]
fn epub_books_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let md = dir.path().join("book.md");
    std::fs::write(
        &md,
        "# The Engine\n\nIt *works*.\n\n# The Cards\n\nPunched, in order.\n\n- one\n- two\n",
    )
    .unwrap();
    let caps = Capabilities::none();
    let epub = done(&one_with(&caps, &[md], Action::convert(Format::Epub)));
    assert_eq!(name(&epub), "book.epub");
    assert_eq!(detect(&epub).unwrap(), Format::Epub);
    // The mimetype comes first, stored, as readers require.
    let bytes = std::fs::read(&epub).unwrap();
    assert_eq!(&bytes[30..38], b"mimetype");
    assert_eq!(&bytes[38..58], b"application/epub+zip");
    let txt = done(&one_with(
        &caps,
        &[epub.clone()],
        Action::convert(Format::Txt),
    ));
    let text = std::fs::read_to_string(&txt).unwrap();
    assert!(text.contains("The Engine"), "{text}");
    assert!(text.contains("Punched, in order."), "{text}");
    assert!(
        text.find("The Engine").unwrap() < text.find("The Cards").unwrap(),
        "chapters in order"
    );
    let pdf = done(&one_with(&caps, &[epub], Action::convert(Format::Pdf)));
    assert_eq!(detect(&pdf).unwrap(), Format::Pdf);
}
