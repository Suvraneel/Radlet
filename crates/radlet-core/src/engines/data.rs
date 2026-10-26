//! Data files: CSV, TSV and JSON, converted between each other and into
//! tables for documents; JSON re-indented or minified.

use std::path::Path;

use crate::engines::doc::{self, Block, Doc};
use crate::engines::text::read_text;
use crate::error::{Error, Result};
use crate::format::Format;

/// The separator a CSV file uses: comma, semicolon (spreadsheets in many
/// European languages), tab or bar, by counting them in the first line.
fn sniff_delimiter(text: &str) -> u8 {
    let first = text.lines().next().unwrap_or("");
    let mut counts = [(b',', 0usize), (b';', 0), (b'\t', 0), (b'|', 0)];
    let mut quoted = false;
    for c in first.bytes() {
        if c == b'"' {
            quoted = !quoted;
        } else if !quoted {
            for (d, n) in counts.iter_mut() {
                if c == *d {
                    *n += 1;
                }
            }
        }
    }
    counts
        .iter()
        .copied()
        .max_by_key(|(d, n)| (*n, *d == b','))
        .filter(|(_, n)| *n > 0)
        .map_or(b',', |(d, _)| d)
}

fn parse_rows(text: &str, delimiter: u8) -> Result<Vec<Vec<String>>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record =
            record.map_err(|e| Error::Invalid(format!("couldn't read the table ({e})")))?;
        rows.push(record.iter().map(str::to_string).collect::<Vec<_>>());
    }
    // A last line holding only white space isn't a row.
    while rows
        .last()
        .is_some_and(|r: &Vec<String>| r.iter().all(|c| c.trim().is_empty()))
    {
        rows.pop();
    }
    Ok(rows)
}

/// The rows of a CSV or TSV file.
pub fn read_rows(path: &Path, fmt: Format) -> Result<Vec<Vec<String>>> {
    let text = read_text(path)?;
    let delimiter = if fmt == Format::Tsv {
        b'\t'
    } else {
        sniff_delimiter(&text)
    };
    parse_rows(&text, delimiter)
}

pub fn write_rows(rows: &[Vec<String>], delimiter: u8) -> Result<String> {
    let mut w = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    for r in rows {
        w.write_record(r)
            .map_err(|e| Error::Internal(format!("couldn't write the table ({e})")))?;
    }
    let bytes = w
        .into_inner()
        .map_err(|e| Error::Internal(format!("couldn't write the table ({e})")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Header names that are unique and not empty.
fn header_names(header: &[String], cols: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(cols);
    for i in 0..cols {
        let base = header
            .get(i)
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| format!("column {}", i + 1));
        let mut name = base.clone();
        let mut n = 2;
        while out.contains(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        out.push(name);
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Number,
    Bool,
    Text,
}

/// A plain decimal number: no leading zeros (so postcodes and IDs stay
/// text), no exponent, at most 15 digits before the point.
pub(crate) fn is_number(t: &str) -> bool {
    let unsigned = t.strip_prefix('-').unwrap_or(t);
    let (int, frac) = match unsigned.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (unsigned, None),
    };
    !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && (int == "0" || !int.starts_with('0'))
        && int.len() <= 15
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

/// A column holds numbers (or true/false) only when every filled cell
/// does; otherwise all of it stays text, so one column has one type.
pub(crate) fn column_kinds(body: &[Vec<String>], cols: usize) -> Vec<Kind> {
    (0..cols)
        .map(|i| {
            let cells: Vec<&str> = body
                .iter()
                .filter_map(|r| r.get(i).map(String::as_str))
                .filter(|c| !c.is_empty())
                .collect();
            if cells.is_empty() {
                Kind::Text
            } else if cells.iter().all(|c| is_number(c)) {
                Kind::Number
            } else if cells
                .iter()
                .all(|c| c.eq_ignore_ascii_case("true") || c.eq_ignore_ascii_case("false"))
            {
                Kind::Bool
            } else {
                Kind::Text
            }
        })
        .collect()
}

fn json_cell(s: &str, kind: Kind) -> String {
    match kind {
        Kind::Number | Kind::Bool if s.is_empty() => "null".into(),
        Kind::Number => s.to_string(),
        Kind::Bool => s.to_ascii_lowercase(),
        Kind::Text => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
    }
}

/// Rows to a JSON array of objects, keyed by the first row.
pub fn rows_to_json(rows: &[Vec<String>]) -> String {
    let Some((header, body)) = rows.split_first() else {
        return "[]\n".into();
    };
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let names = header_names(header, cols);
    if body.is_empty() {
        return "[]\n".into();
    }
    let kinds = column_kinds(body, cols);
    let mut out = String::from("[\n");
    for (r, row) in body.iter().enumerate() {
        out.push_str("  {\n");
        for (i, name) in names.iter().enumerate() {
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            out.push_str(&format!(
                "    {}: {}{}\n",
                serde_json::to_string(name).unwrap_or_default(),
                json_cell(cell, kinds[i]),
                if i + 1 < names.len() { "," } else { "" }
            ));
        }
        out.push_str(if r + 1 < body.len() {
            "  },\n"
        } else {
            "  }\n"
        });
    }
    out.push_str("]\n");
    out
}

fn cell_text(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// JSON to rows: an array of objects (keys become the header), an array
/// of arrays, or one object. A single list wrapped in an object (such as
/// `{"items": [...]}`) is unwrapped.
pub fn json_to_rows(src: &str) -> Result<Vec<Vec<String>>> {
    use serde_json::Value;
    let src = src.trim_start_matches('\u{feff}');
    let value: Value = serde_json::from_str(src)
        .map_err(|e| Error::Invalid(format!("this isn't valid JSON ({e})")))?;
    let value = match value {
        Value::Object(map) if map.len() == 1 && map.values().all(Value::is_array) => map
            .into_iter()
            .next()
            .map(|(_, v)| v)
            .unwrap_or(Value::Null),
        other => other,
    };
    let items: Vec<Value> = match value {
        Value::Array(a) => a,
        Value::Object(o) => vec![Value::Object(o)],
        _ => {
            return Err(Error::UnsupportedInput(
                "this JSON holds a single value, not a table".into(),
            ))
        }
    };
    if items.is_empty() {
        return Err(Error::NothingToDo("this JSON list is empty".into()));
    }
    if items.iter().all(|v| v.is_object()) {
        let mut keys: Vec<String> = Vec::new();
        for v in &items {
            if let Value::Object(o) = v {
                for k in o.keys() {
                    if !keys.contains(k) {
                        keys.push(k.clone());
                    }
                }
            }
        }
        let mut rows = vec![keys.clone()];
        for v in &items {
            if let Value::Object(o) = v {
                rows.push(
                    keys.iter()
                        .map(|k| o.get(k).map(cell_text).unwrap_or_default())
                        .collect(),
                );
            }
        }
        return Ok(rows);
    }
    if items.iter().all(|v| v.is_array()) {
        return Ok(items
            .iter()
            .map(|v| match v {
                Value::Array(a) => a.iter().map(cell_text).collect(),
                _ => Vec::new(),
            })
            .collect());
    }
    let mut rows = vec![vec!["value".to_string()]];
    rows.extend(items.iter().map(|v| vec![cell_text(v)]));
    Ok(rows)
}

/// A CSV or TSV file as a document holding one table.
pub fn table_doc(path: &Path, fmt: Format) -> Result<Doc> {
    let rows = read_rows(path, fmt)?;
    if rows.is_empty() {
        return Err(Error::NothingToDo("the table is empty".into()));
    }
    Ok(Doc {
        title: None,
        blocks: vec![Block::Table {
            rows: rows
                .into_iter()
                .map(|r| r.iter().map(|c| doc::text(c)).collect())
                .collect(),
            header: true,
        }],
    })
}

/// Converts between CSV, TSV and JSON, JSON and YAML, TOML or XML, or a
/// table into a document.
pub fn convert(input: &Path, from: Format, to: Format, out: &Path) -> Result<()> {
    let delim = |f: Format| if f == Format::Tsv { b'\t' } else { b',' };
    let result = match (from, to) {
        (
            Format::Json | Format::Yaml | Format::Toml | Format::Xml,
            Format::Json | Format::Yaml | Format::Toml | Format::Xml,
        ) => crate::engines::tree::convert(&read_text(input)?, from, to)?,
        (Format::Csv | Format::Tsv, Format::Csv | Format::Tsv) => {
            write_rows(&read_rows(input, from)?, delim(to))?
        }
        (Format::Csv | Format::Tsv, Format::Json) => rows_to_json(&read_rows(input, from)?),
        (Format::Json, Format::Csv | Format::Tsv) => {
            write_rows(&json_to_rows(&read_text(input)?)?, delim(to))?
        }
        (Format::Csv | Format::Tsv, t) if doc::writable(t) => {
            let d = table_doc(input, from)?;
            return doc::write(&d, t, &doc::title_of(input), out);
        }
        (Format::Csv | Format::Tsv | Format::Json, Format::Xlsx) => {
            let sheets = crate::engines::sheet::as_sheets(input, from)?;
            std::fs::write(out, crate::engines::sheet::write(&sheets)?)?;
            return Ok(());
        }
        (f, t) if f.is_workbook() && doc::writable(t) => {
            let d = crate::engines::sheet::doc(input, f)?;
            return doc::write(&d, t, &doc::title_of(input), out);
        }
        (Format::Xls | Format::Ods, Format::Xlsx) => {
            let sheets = crate::engines::sheet::read_book(input, from)?;
            if sheets.is_empty() {
                return Err(Error::NothingToDo("the workbook is empty".into()));
            }
            std::fs::write(out, crate::engines::sheet::write(&sheets)?)?;
            return Ok(());
        }
        (f, Format::Csv | Format::Tsv | Format::Json) if f.is_workbook() => {
            // The first sheet with anything in it (the pipeline splits a
            // workbook of several sheets into one file each).
            let sheets = crate::engines::sheet::read_book(input, f)?;
            let rows = sheets
                .into_iter()
                .map(|(_, rows)| rows)
                .next()
                .ok_or_else(|| Error::NothingToDo("the workbook is empty".into()))?;
            if to == Format::Json {
                rows_to_json(&rows)
            } else {
                write_rows(&rows, delim(to))?
            }
        }
        _ => {
            return Err(Error::UnsupportedRoute {
                from: from.label().into(),
                to: to.label().into(),
            })
        }
    };
    std::fs::write(out, result)?;
    Ok(())
}

/// Re-indents JSON with two spaces, or removes every space it doesn't
/// need. Strings, numbers and key order are copied exactly.
pub fn format_json(src: &str, minify: bool) -> Result<String> {
    let src = src.trim_start_matches('\u{feff}');
    serde_json::from_str::<serde::de::IgnoredAny>(src)
        .map_err(|e| Error::Invalid(format!("this isn't valid JSON ({e})")))?;
    let bytes = src.as_bytes();
    let ws = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
    let mut out = String::with_capacity(src.len() + src.len() / 4);
    let mut depth = 0usize;
    let newline = |out: &mut String, depth: usize| {
        if !minify {
            out.push('\n');
            for _ in 0..depth {
                out.push_str("  ");
            }
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                out.push_str(&src[start..i.min(bytes.len())]);
            }
            b'{' | b'[' => {
                let close = if c == b'{' { b'}' } else { b']' };
                let mut j = i + 1;
                while j < bytes.len() && ws(bytes[j]) {
                    j += 1;
                }
                out.push(c as char);
                if bytes.get(j) == Some(&close) {
                    out.push(close as char);
                    i = j + 1;
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                    i += 1;
                }
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c as char);
                i += 1;
            }
            b',' => {
                out.push(',');
                newline(&mut out, depth);
                i += 1;
            }
            b':' => {
                out.push(':');
                if !minify {
                    out.push(' ');
                }
                i += 1;
            }
            b if ws(b) => i += 1,
            _ => {
                let start = i;
                while i < bytes.len()
                    && !ws(bytes[i])
                    && !matches!(bytes[i], b',' | b'}' | b']' | b':')
                {
                    i += 1;
                }
                out.push_str(&src[start..i]);
            }
        }
    }
    if !minify {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimiters_are_sniffed() {
        assert_eq!(sniff_delimiter("a,b,c\n1,2,3"), b',');
        assert_eq!(sniff_delimiter("Name;Price;\"Note, with comma\"\n"), b';');
        assert_eq!(sniff_delimiter("a\tb\n"), b'\t');
        assert_eq!(sniff_delimiter("single"), b',');
    }

    #[test]
    fn csv_to_json_and_back() {
        let rows = parse_rows(
            "name,zip,qty,price,ok,name\nTea,02134,2,3.50,true,x\n\"Cof, fee\",10001,-1,1e5,no\n\n",
            b',',
        )
        .unwrap();
        let json = rows_to_json(&rows);
        assert_eq!(
            json,
            "[\n  {\n    \"name\": \"Tea\",\n    \"zip\": \"02134\",\n    \"qty\": 2,\n    \"price\": \"3.50\",\n    \"ok\": \"true\",\n    \"name 2\": \"x\"\n  },\n  {\n    \"name\": \"Cof, fee\",\n    \"zip\": \"10001\",\n    \"qty\": -1,\n    \"price\": \"1e5\",\n    \"ok\": \"no\",\n    \"name 2\": \"\"\n  }\n]\n"
        );
        let back = json_to_rows(&json).unwrap();
        assert_eq!(back[0], ["name", "zip", "qty", "price", "ok", "name 2"]);
        assert_eq!(back[1], ["Tea", "02134", "2", "3.50", "true", "x"]);
        let csv = write_rows(&back, b',').unwrap();
        assert!(csv.contains("\"Cof, fee\",10001,-1,1e5,no,\n"), "{csv}");
    }

    #[test]
    fn numeric_columns() {
        let rows = parse_rows("n,ok\n1.5,true\n,false\n", b',').unwrap();
        assert_eq!(
            rows_to_json(&rows),
            "[\n  {\n    \"n\": 1.5,\n    \"ok\": true\n  },\n  {\n    \"n\": null,\n    \"ok\": false\n  }\n]\n"
        );
    }

    #[test]
    fn json_shapes() {
        let wrapped =
            json_to_rows(r#"{"items":[{"a":1,"b":{"c":true}},{"b":null,"d":"x"}]}"#).unwrap();
        assert_eq!(wrapped[0], ["a", "b", "d"]);
        assert_eq!(wrapped[1], ["1", "{\"c\":true}", ""]);
        assert_eq!(wrapped[2], ["", "", "x"]);
        assert_eq!(json_to_rows("[[1,2],[3]]").unwrap()[1], ["3"]);
        assert_eq!(json_to_rows("[1,\"two\"]").unwrap()[2], ["two"]);
        assert!(json_to_rows("42").is_err());
        assert!(json_to_rows("{oops").is_err());
    }

    #[test]
    fn json_formatting_keeps_everything() {
        let src = "{\"b\":1.50,\"a\":[1, 2 ,{}],\"s\":\"x, \\\"y\\\": [z]\",\"e\":[ ],\"u\":\"é\"}";
        let pretty = format_json(src, false).unwrap();
        assert_eq!(
            pretty,
            "{\n  \"b\": 1.50,\n  \"a\": [\n    1,\n    2,\n    {}\n  ],\n  \"s\": \"x, \\\"y\\\": [z]\",\n  \"e\": [],\n  \"u\": \"é\"\n}\n"
        );
        let mini = format_json(&pretty, true).unwrap();
        assert_eq!(
            mini,
            "{\"b\":1.50,\"a\":[1,2,{}],\"s\":\"x, \\\"y\\\": [z]\",\"e\":[],\"u\":\"é\"}"
        );
        assert!(matches!(
            format_json("{\"a\":}", false),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn tables_become_documents() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.csv");
        std::fs::write(&p, "a;b\n1;x|y\n").unwrap();
        let out = dir.path().join("t.md");
        convert(&p, Format::Csv, Format::Md, &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "| a | b |\n| --- | --- |\n| 1 | x\\|y |\n"
        );
        let tsv = dir.path().join("t.tsv");
        convert(&p, Format::Csv, Format::Tsv, &tsv).unwrap();
        assert_eq!(std::fs::read_to_string(&tsv).unwrap(), "a\tb\n1\tx|y\n");
    }
}
