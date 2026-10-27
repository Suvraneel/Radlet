//! Excel workbooks (.xlsx): read sheet by sheet, and written from tables
//! with typed columns (numbers stay numbers, IDs with leading zeros stay
//! text), a bold header row that stays in view, and fitted widths.

use std::path::Path;

use calamine::{Data, Reader};
use rust_xlsxwriter::{Format as CellFormat, Workbook};

use crate::engines::data::{self, column_kinds, Kind};
use crate::engines::doc::{self, Block, Doc};
use crate::error::{Error, Result};
use crate::format::Format;

/// A sheet's name and its rows of cells, as text.
pub type Sheet = (String, Vec<Vec<String>>);

/// Most a workbook may unpack to (it is a ZIP of XML).
const MAX_UNPACKED: u64 = 512 << 20;
/// Excel's own limits.
const MAX_ROWS: usize = 1_048_576;
const MAX_COLS: usize = 16_384;
const MAX_CELL_CHARS: usize = 32_767;

fn bad(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::UnsupportedInput(format!(
        "{} isn't a workbook Radlet can read ({why})",
        crate::format::display_name(path)
    ))
}

/// Refuses workbooks that would unpack into more than [`MAX_UNPACKED`].
fn check_size(path: &Path) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(path, e))?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let f = zip.by_index_raw(i).map_err(|e| bad(path, e))?;
        total = total.saturating_add(f.size());
    }
    if total > MAX_UNPACKED {
        return Err(Error::TooLarge(
            "this workbook unpacks to more than 512 MB".into(),
        ));
    }
    Ok(())
}

/// A cell as text, as Excel would show it in a CSV export.
fn cell(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Data::DateTime(dt) => {
            if dt.is_duration() {
                let secs = (dt.as_f64() * 86_400.0).round() as i64;
                format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
            } else {
                let (y, mo, d, h, mi, s, _) = dt.to_ymd_hms_milli();
                if (h, mi, s) == (0, 0, 0) {
                    format!("{y:04}-{mo:02}-{d:02}")
                } else {
                    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
                }
            }
        }
        Data::Error(e) => e.to_string(),
    }
}

/// Most non-empty cells, and most cells laid out in rows, read from one
/// workbook. Cells are streamed, so a sheet with one value in A1 and one
/// in XFD1048576 is refused instead of filling memory with empty cells.
const MAX_CELLS: usize = 10_000_000;
/// Most cells in a sheet's used area (rows × columns): tables are padded
/// to their widest row on the way out.
const MAX_AREA: usize = 25_000_000;

fn too_big(what: &str) -> Error {
    Error::TooLarge(format!("this workbook {what}, more than Radlet reads"))
}

/// One sheet's rows, from its first used row and column, each row ending
/// at its last non-empty cell.
fn read_sheet<R: std::io::Read + std::io::Seek>(
    book: &mut calamine::Xlsx<R>,
    name: &str,
    path: &Path,
    budget: &mut usize,
) -> Result<Vec<Vec<String>>> {
    let mut reader = book
        .worksheet_cells_reader(name)
        .map_err(|e| bad(path, e))?;
    let mut cells: Vec<(u32, u32, String)> = Vec::new();
    while let Some(c) = reader.next_cell().map_err(|e| bad(path, e))? {
        let text = cell(&Data::from(c.get_value().clone()));
        if text.is_empty() {
            continue;
        }
        let (r, col) = c.get_position();
        if r as usize >= MAX_ROWS || col as usize >= MAX_COLS {
            return Err(bad(path, "a cell lies outside Excel's grid"));
        }
        if *budget == 0 {
            return Err(too_big("has over 10 million filled cells"));
        }
        *budget -= 1;
        cells.push((r, col, text));
    }
    if cells.is_empty() {
        return Ok(Vec::new());
    }
    cells.sort_unstable_by_key(|c| (c.0, c.1));
    let top = cells[0].0;
    let left = cells.iter().map(|c| c.1).min().unwrap_or(0);
    let bottom = cells[cells.len() - 1].0;
    let right = cells.iter().map(|c| c.1).max().unwrap_or(0);
    let area = ((bottom - top) as usize + 1).saturating_mul((right - left) as usize + 1);
    if area > MAX_AREA {
        return Err(too_big("spreads over more than 25 million cells"));
    }
    let mut rows: Vec<Vec<String>> = vec![Vec::new(); (bottom - top) as usize + 1];
    for (r, col, text) in cells {
        let row = &mut rows[(r - top) as usize];
        let at = (col - left) as usize;
        if row.len() <= at {
            row.resize(at + 1, String::new());
        }
        row[at] = text;
    }
    Ok(rows)
}

/// Every sheet with something in it, in workbook order.
pub fn read(path: &Path) -> Result<Vec<Sheet>> {
    check_size(path)?;
    let mut book: calamine::Xlsx<_> = calamine::open_workbook(path).map_err(|e| bad(path, e))?;
    let mut budget = MAX_CELLS;
    let mut out = Vec::new();
    for name in book.sheet_names() {
        let rows = read_sheet(&mut book, &name, path, &mut budget)?;
        if !rows.is_empty() {
            out.push((name, rows));
        }
    }
    Ok(out)
}

/// Most bytes an old Excel workbook may have (it's read whole).
const MAX_XLS: u64 = 200 << 20;
/// Most cells in the used area of an old Excel or OpenDocument sheet,
/// which is read whole rather than streamed (about 600 MB at worst).
const MAX_DENSE: usize = 3_000_000;
/// Most bytes of an OpenDocument spreadsheet's content.xml.
const MAX_ODS_XML: u64 = 256 << 20;

/// The used area an OpenDocument spreadsheet would expand to, from its
/// rows and cells and their repeat counts, before anything is expanded: a
/// few hundred bytes can describe millions of repeated cells.
fn ods_area(path: &Path) -> Result<usize> {
    use quick_xml::events::Event;
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(path, e))?;
    let entry = zip.by_name("content.xml").map_err(|e| bad(path, e))?;
    if entry.size() > MAX_ODS_XML {
        return Err(too_big("is larger than 256 MB unpacked"));
    }
    let mut xml = String::new();
    entry
        .take(MAX_ODS_XML)
        .read_to_string(&mut xml)
        .map_err(|e| bad(path, e))?;
    let repeat = |e: &quick_xml::events::BytesStart, name: &str| -> usize {
        e.attributes()
            .flatten()
            .find(|a| a.key.local_name().as_ref() == name)
            .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1)
    };
    let valued = |e: &quick_xml::events::BytesStart| {
        e.attributes()
            .flatten()
            .any(|a| a.key.local_name().as_ref() == "value-type")
    };
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut worst = 0usize;
    // The current sheet: rows so far, and its widest filled row.
    let (mut rows, mut width) = (0usize, 0usize);
    // The current row: its repeat count, the column reached, and the end
    // of its last filled cell.
    let (mut row_repeat, mut col, mut row_end) = (1usize, 0usize, 0usize);
    loop {
        match reader.read_event().map_err(|e| bad(path, e))? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "table" => {
                (rows, width) = (0, 0);
            }
            Event::Start(e) if e.local_name().as_ref() == "table-row" => {
                (row_repeat, col, row_end) = (repeat(&e, "number-rows-repeated"), 0, 0);
            }
            Event::Empty(e) if e.local_name().as_ref() == "table-row" => {
                rows = rows.saturating_add(repeat(&e, "number-rows-repeated"));
            }
            Event::Start(e) | Event::Empty(e)
                if matches!(e.local_name().as_ref(), "table-cell" | "covered-table-cell") =>
            {
                let n = repeat(&e, "number-columns-repeated");
                col = col.saturating_add(n);
                if valued(&e) {
                    row_end = col;
                }
            }
            Event::End(e) if e.local_name().as_ref() == "table-row" => {
                rows = rows.saturating_add(row_repeat);
                // Up to its last filled row, a sheet is as wide as its
                // widest filled row.
                if row_end > 0 {
                    width = width.max(row_end);
                    worst = worst.max(rows.saturating_mul(width));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(worst)
}

/// One sheet of an old Excel or OpenDocument workbook, which calamine
/// reads whole: from the first used cell, each row ending at its last
/// non-empty cell.
fn range_rows(range: &calamine::Range<Data>, budget: &mut usize) -> Result<Vec<Vec<String>>> {
    let (height, width) = range.get_size();
    if height.saturating_mul(width) > MAX_DENSE {
        return Err(too_big("spreads over more than 3 million cells"));
    }
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(height);
    for row in range.rows() {
        let mut out: Vec<String> = row.iter().map(cell).collect();
        while out.last().is_some_and(String::is_empty) {
            out.pop();
        }
        let filled = out.iter().filter(|c| !c.is_empty()).count();
        if filled > *budget {
            return Err(too_big("has over 10 million filled cells"));
        }
        *budget -= filled;
        rows.push(out);
    }
    // Trim empty rows at the ends, and empty columns on the left.
    while rows.last().is_some_and(Vec::is_empty) {
        rows.pop();
    }
    let first = rows
        .iter()
        .position(|r| !r.is_empty())
        .unwrap_or(rows.len());
    rows.drain(..first);
    let left = rows
        .iter()
        .filter_map(|r| r.iter().position(|c| !c.is_empty()))
        .min()
        .unwrap_or(0);
    if left > 0 {
        for r in &mut rows {
            r.drain(..left.min(r.len()));
        }
    }
    Ok(rows)
}

fn read_ranges<R: Reader<std::io::BufReader<std::fs::File>>>(
    mut book: R,
    path: &Path,
) -> Result<Vec<Sheet>>
where
    R::Error: std::fmt::Display,
{
    let mut budget = MAX_CELLS;
    let mut out = Vec::new();
    for name in book.sheet_names() {
        let range = book.worksheet_range(&name).map_err(|e| bad(path, e))?;
        let rows = range_rows(&range, &mut budget)?;
        if !rows.is_empty() {
            out.push((name, rows));
        }
    }
    Ok(out)
}

/// A path as a SQLite URI path: everything but plain characters escaped.
fn uri_path(path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = String::new();
    for b in abs.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Most bytes of text read out of a database.
const MAX_SQLITE_TEXT: usize = 256 << 20;
/// Longest value SQLite may make or return (16 MB).
const MAX_SQLITE_VALUE: i32 = 16 << 20;

/// Opens a database to read. Normally read-only and immutable, so nothing
/// is written beside it, whatever state it's in. One with a write-ahead log
/// beside it is read from a private copy of both, so its latest changes
/// are seen; the original is still never written.
fn open_sqlite(path: &Path) -> Result<(rusqlite::Connection, Option<tempfile::TempDir>)> {
    use rusqlite::{Connection, OpenFlags};
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let wal = std::path::PathBuf::from(wal);
    let has_wal = std::fs::metadata(&wal).is_ok_and(|m| m.is_file() && m.len() > 0);
    if has_wal {
        let dir = tempfile::tempdir()?;
        let copy = dir.path().join("db");
        std::fs::copy(path, &copy)?;
        std::fs::copy(&wal, dir.path().join("db-wal"))?;
        let db = Connection::open_with_flags(
            &copy,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| bad(path, e))?;
        return Ok((db, Some(dir)));
    }
    let uri = format!("file:{}?mode=ro&immutable=1", uri_path(path));
    let db = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| bad(path, e))?;
    Ok((db, None))
}

/// A SQLite database's tables, a sheet each: the column names, then the
/// rows. Columns worked out when read (generated, or hidden) are left out,
/// so the file's schema runs no code; so are tables that can't be read
/// here (virtual tables from extensions).
fn read_sqlite(path: &Path) -> Result<Vec<Sheet>> {
    use rusqlite::config::DbConfig;
    use rusqlite::limits::Limit;
    use rusqlite::types::ValueRef;
    let (db, _copy) = open_sqlite(path)?;
    let setup = || -> rusqlite::Result<()> {
        db.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        db.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_SQLITE_VALUE)?;
        // Views and triggers can't run code from the file's schema.
        db.execute_batch("PRAGMA trusted_schema = OFF; PRAGMA query_only = ON;")
    };
    setup().map_err(|e| bad(path, e))?;
    let names: Vec<String> = {
        let mut stmt = db
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY rowid",
            )
            .map_err(|e| bad(path, e))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| bad(path, e))?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(|e| bad(path, e))?
    };
    let quote = |n: &str| format!("\"{}\"", n.replace('"', "\"\""));
    let mut budget = MAX_CELLS;
    let mut text_left = MAX_SQLITE_TEXT;
    let mut out = Vec::new();
    for name in names {
        // Stored columns only: 0 is plain, 3 a generated column whose
        // values are stored; 1 (hidden) and 2 (worked out on reading) aren't.
        let Ok(columns) = db
            .prepare(&format!("PRAGMA table_xinfo({})", quote(&name)))
            .and_then(|mut stmt| {
                stmt.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(6)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
        else {
            continue;
        };
        let columns: Vec<String> = columns
            .into_iter()
            .filter(|(_, hidden)| matches!(hidden, 0 | 3))
            .map(|(c, _)| c)
            .collect();
        if columns.is_empty() {
            continue;
        }
        let list = columns
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        let Ok(mut stmt) = db.prepare(&format!("SELECT {list} FROM {}", quote(&name))) else {
            continue;
        };
        let width = columns.len();
        let mut rows = vec![columns];
        let Ok(mut query) = stmt.query([]) else {
            continue;
        };
        loop {
            let row = match query.next() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => return Err(bad(path, e)),
            };
            if budget < width {
                return Err(too_big("has over 10 million values"));
            }
            budget -= width;
            let mut cells = Vec::with_capacity(width);
            for i in 0..width {
                let value = match row.get_ref(i).map_err(|e| bad(path, e))? {
                    ValueRef::Null => String::new(),
                    ValueRef::Integer(v) => v.to_string(),
                    ValueRef::Real(v) => cell(&Data::Float(v)),
                    ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                    ValueRef::Blob(b) => format!("(binary, {} bytes)", b.len()),
                };
                text_left = text_left
                    .checked_sub(value.len())
                    .ok_or_else(|| too_big("holds more than 256 MB of text"))?;
                cells.push(value);
            }
            // A table's rows all have its columns, empty or not.
            rows.push(cells);
        }
        out.push((name, rows));
    }
    Ok(out)
}

/// Every sheet with something in it, from Excel, old Excel or OpenDocument,
/// or every table of a SQLite database.
pub fn read_book(path: &Path, fmt: Format) -> Result<Vec<Sheet>> {
    match fmt {
        Format::Xlsx => read(path),
        Format::Sqlite => read_sqlite(path),
        Format::Xls => {
            if std::fs::metadata(path)?.len() > MAX_XLS {
                return Err(Error::TooLarge(
                    "this workbook is larger than 200 MB".into(),
                ));
            }
            let book: calamine::Xls<_> = calamine::open_workbook(path).map_err(|e| bad(path, e))?;
            read_ranges(book, path)
        }
        Format::Ods => {
            check_size(path)?;
            if ods_area(path)? > MAX_DENSE {
                return Err(too_big("spreads over more than 3 million cells"));
            }
            let book: calamine::Ods<_> = calamine::open_workbook(path).map_err(|e| bad(path, e))?;
            read_ranges(book, path)
        }
        other => Err(Error::UnsupportedInput(format!(
            "{other} files aren't workbooks"
        ))),
    }
}

/// A file Radlet can make a sheet from: its rows, named after the file.
pub fn as_sheets(path: &Path, fmt: Format) -> Result<Vec<Sheet>> {
    let name = doc::title_of(path);
    let rows = match fmt {
        f if f.is_workbook() => return read_book(path, f),
        Format::Csv | Format::Tsv => data::read_rows(path, fmt)?,
        Format::Json => data::json_to_rows(&crate::engines::text::read_text(path)?)?,
        other => {
            return Err(Error::UnsupportedInput(format!(
                "{other} files aren't tables"
            )))
        }
    };
    if rows.is_empty() {
        return Err(Error::NothingToDo("the table is empty".into()));
    }
    Ok(vec![(name, rows)])
}

/// A sheet name Excel accepts: at most 31 characters, none of `[]:*?/\`,
/// not starting or ending with an apostrophe, and unique.
fn sheet_name(wanted: &str, used: &mut Vec<String>) -> String {
    let clean: String = wanted
        .chars()
        .map(|c| if "[]:*?/\\".contains(c) { '-' } else { c })
        .collect();
    let clean = clean.trim().trim_matches('\'').to_string();
    let base: String = if clean.is_empty() {
        "Sheet".into()
    } else {
        clean
    };
    let fit = |s: &str, room: usize| s.chars().take(room).collect::<String>();
    let mut name = fit(&base, 31);
    let mut n = 2;
    while used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
        let tail = format!(" {n}");
        name = format!("{}{tail}", fit(&base, 31 - tail.len()));
        n += 1;
    }
    used.push(name.clone());
    name
}

fn xlsx_err(e: rust_xlsxwriter::XlsxError) -> Error {
    Error::Internal(format!("couldn't write the workbook ({e})"))
}

/// Writes sheets into a new workbook. The first row of each is the
/// header.
pub fn write(sheets: &[Sheet]) -> Result<Vec<u8>> {
    let mut book = Workbook::new();
    let bold = CellFormat::new().set_bold();
    let mut used = Vec::new();
    for (name, rows) in sheets {
        if rows.len() > MAX_ROWS {
            return Err(Error::TooLarge(format!(
                "Excel sheets hold at most {} rows",
                doc::thousands(MAX_ROWS as u64)
            )));
        }
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols > MAX_COLS {
            return Err(Error::TooLarge(format!(
                "Excel sheets hold at most {} columns",
                doc::thousands(MAX_COLS as u64)
            )));
        }
        let kinds = column_kinds(rows.get(1..).unwrap_or(&[]), cols);
        let ws = book.add_worksheet();
        ws.set_name(sheet_name(name, &mut used)).map_err(xlsx_err)?;
        for (r, row) in rows.iter().enumerate() {
            for (c, value) in row.iter().enumerate() {
                if value.is_empty() {
                    continue;
                }
                let (r32, c16) = (r as u32, c as u16);
                let text: String = value.chars().take(MAX_CELL_CHARS).collect();
                if r == 0 {
                    ws.write_string_with_format(r32, c16, text, &bold)
                        .map_err(xlsx_err)?;
                    continue;
                }
                match kinds[c] {
                    Kind::Number => match value.parse::<f64>() {
                        Ok(n) => ws.write_number(r32, c16, n),
                        Err(_) => ws.write_string(r32, c16, text),
                    },
                    Kind::Bool => ws.write_boolean(r32, c16, value.eq_ignore_ascii_case("true")),
                    Kind::Text => ws.write_string(r32, c16, text),
                }
                .map_err(xlsx_err)?;
            }
        }
        if rows.len() > 1 {
            ws.set_freeze_panes(1, 0).map_err(xlsx_err)?;
        }
        // Fitting widths reads every cell; skip it for very long sheets.
        if rows.len() <= 50_000 {
            ws.set_autofit_max_width(400).autofit();
        }
    }
    if sheets.is_empty() {
        book.add_worksheet();
    }
    book.save_to_buffer().map_err(xlsx_err)
}

/// A workbook as a document: each sheet a table, under its name when
/// there are several.
pub fn doc(path: &Path, fmt: Format) -> Result<Doc> {
    let sheets = read_book(path, fmt)?;
    if sheets.is_empty() {
        return Err(Error::NothingToDo("the workbook is empty".into()));
    }
    let several = sheets.len() > 1;
    let mut d = Doc::default();
    for (name, rows) in sheets {
        if several {
            d.blocks.push(Block::Heading {
                level: 2,
                text: doc::text(&name),
            });
        }
        d.blocks.push(Block::Table {
            rows: rows
                .into_iter()
                .map(|r| r.iter().map(|c| doc::text(c)).collect())
                .collect(),
            header: true,
        });
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(t: &[&[&str]]) -> Vec<Vec<String>> {
        t.iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect()
    }

    #[test]
    fn workbooks_round_trip_with_types() {
        let dir = tempfile::tempdir().unwrap();
        let sheets = vec![
            (
                "Orders".to_string(),
                rows(&[
                    &["Item", "Qty", "Zip", "Paid", "Note"],
                    &["Tea", "2", "02134", "true", "=1+1"],
                    &["Coffee", "10.5", "10001", "FALSE", ""],
                ]),
            ),
            ("Orders".to_string(), rows(&[&["only a header"]])),
            ("Bad/name:with*chars?".to_string(), rows(&[&["a"], &["b"]])),
        ];
        let bytes = write(&sheets).unwrap();
        let p = dir.path().join("book.xlsx");
        std::fs::write(&p, bytes).unwrap();
        assert_eq!(crate::format::detect(&p).unwrap(), Format::Xlsx);
        let back = read(&p).unwrap();
        let names: Vec<&str> = back.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(names, ["Orders", "Orders 2", "Bad-name-with-chars-"]);
        assert_eq!(back[0].1[1], ["Tea", "2", "02134", "TRUE", "=1+1"]);
        assert_eq!(back[0].1[2], ["Coffee", "10.5", "10001", "FALSE"]);
        // Numbers really are numbers in the file; formulas stay text.
        let mut book: calamine::Xlsx<_> = calamine::open_workbook(&p).unwrap();
        let range = book.worksheet_range("Orders").unwrap();
        assert_eq!(range.get_value((1, 1)), Some(&Data::Float(2.0)));
        assert_eq!(range.get_value((1, 2)), Some(&Data::String("02134".into())));
        assert_eq!(range.get_value((1, 3)), Some(&Data::Bool(true)));
        assert_eq!(range.get_value((1, 4)), Some(&Data::String("=1+1".into())));

        let d = doc(&p, Format::Xlsx).unwrap();
        assert!(
            matches!(&d.blocks[0], Block::Heading { text, .. } if doc::plain(text) == "Orders")
        );
        assert!(matches!(&d.blocks[1], Block::Table { rows, .. } if rows.len() == 3));
    }

    #[test]
    fn sparse_sheets_are_refused_not_expanded() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("far.xlsx");
        let mut book = Workbook::new();
        let ws = book.add_worksheet();
        ws.write_string(0, 0, "here").unwrap();
        ws.write_string(1_048_575, 16_383, "and far away").unwrap();
        std::fs::write(&p, book.save_to_buffer().unwrap()).unwrap();
        let started = std::time::Instant::now();
        assert!(matches!(read(&p), Err(Error::TooLarge(_))));
        assert!(started.elapsed().as_secs() < 5);

        // A table that starts at C3 reads from its first cell, as before.
        let p = dir.path().join("offset.xlsx");
        let mut book = Workbook::new();
        let ws = book.add_worksheet();
        ws.write_string(2, 2, "a").unwrap();
        ws.write_string(2, 4, "c").unwrap();
        ws.write_number(4, 3, 7.0).unwrap();
        std::fs::write(&p, book.save_to_buffer().unwrap()).unwrap();
        let back = read(&p).unwrap();
        assert_eq!(back[0].1, [vec!["a", "", "c"], vec![], vec!["", "7"]]);
    }

    #[test]
    fn names_are_made_valid() {
        let mut used = Vec::new();
        assert_eq!(sheet_name("'quoted'", &mut used), "quoted");
        let long = "x".repeat(40);
        assert_eq!(sheet_name(&long, &mut used).chars().count(), 31);
        let again = sheet_name(&long, &mut used);
        assert!(again.ends_with(" 2") && again.chars().count() == 31);
        assert_eq!(sheet_name("", &mut used), "Sheet");
    }
}
