//! JSON, YAML, TOML and XML into one another, through a JSON value.
//!
//! - YAML: anchors and aliases are expanded, and `<<` merge keys merged,
//!   within a budget, so a "billion laughs" file is refused instead of
//!   filling memory.
//! - TOML: dates and times become strings in JSON and YAML.
//! - XML ↔ JSON uses the common mapping: an element is a key; attributes
//!   are `"@name"`; text next to attributes or children is `"#text"`;
//!   repeated elements become arrays; an element with only text is that
//!   text.

use std::collections::HashMap;

use serde_json::{Map, Number, Value};
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};
use yaml_rust2::Yaml;

use crate::error::{Error, Result};
use crate::format::Format;

/// Most values a YAML file may expand to (aliases count each time).
const MAX_NODES: usize = 2_000_000;
/// Most bytes read.
const MAX_INPUT: usize = 50 << 20;
/// Deepest XML nesting read.
const MAX_DEPTH: usize = 256;

fn invalid(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("this {what} file doesn't read ({e})"))
}

/// Converts `src` from one of JSON, YAML, TOML and XML into another.
pub fn convert(src: &str, from: Format, to: Format) -> Result<String> {
    if src.len() > MAX_INPUT {
        return Err(Error::TooLarge(format!(
            "this {from} file is larger than 50 MB"
        )));
    }
    let value = match from {
        Format::Json => serde_json::from_str(src).map_err(|e| invalid("JSON", e))?,
        Format::Yaml => from_yaml(src)?,
        Format::Toml => from_toml(src)?,
        Format::Xml => from_xml(src)?,
        other => {
            return Err(Error::UnsupportedRoute {
                from: other.label().into(),
                to: to.label().into(),
            })
        }
    };
    match to {
        Format::Json => {
            let mut s =
                serde_json::to_string_pretty(&value).map_err(|e| Error::Internal(e.to_string()))?;
            s.push('\n');
            Ok(s)
        }
        Format::Yaml => to_yaml(&value),
        Format::Toml => to_toml(&value),
        Format::Xml => Ok(to_xml(&value)),
        other => Err(Error::UnsupportedRoute {
            from: from.label().into(),
            to: other.label().into(),
        }),
    }
}

// MARK: YAML

enum Frame {
    Seq {
        items: Vec<Value>,
        anchor: usize,
        nodes: usize,
    },
    Map {
        map: Map<String, Value>,
        key: Option<String>,
        merges: Vec<Map<String, Value>>,
        anchor: usize,
        nodes: usize,
    },
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    docs: Vec<Value>,
    anchors: HashMap<usize, (Value, usize)>,
    used: usize,
    error: Option<Error>,
}

impl Builder {
    fn spend(&mut self, nodes: usize) -> bool {
        self.used = self.used.saturating_add(nodes);
        if self.used > MAX_NODES && self.error.is_none() {
            self.error = Some(Error::TooLarge(
                "this YAML file expands to more than 2 million values".into(),
            ));
        }
        self.error.is_none()
    }

    /// A finished value, placed in its parent.
    fn place(&mut self, value: Value, nodes: usize, anchor: usize) {
        if anchor > 0 {
            self.anchors.insert(anchor, (value.clone(), nodes));
        }
        match self.stack.last_mut() {
            None => self.docs.push(value),
            Some(Frame::Seq {
                items, nodes: n, ..
            }) => {
                items.push(value);
                *n += nodes;
            }
            Some(Frame::Map {
                map,
                key,
                merges,
                nodes: n,
                ..
            }) => {
                *n += nodes;
                match key.take() {
                    None => *key = Some(key_text(value)),
                    Some(k) if k == "<<" => match value {
                        Value::Object(m) => merges.push(m),
                        Value::Array(list) => {
                            merges.extend(list.into_iter().filter_map(|v| match v {
                                Value::Object(m) => Some(m),
                                _ => None,
                            }))
                        }
                        other => {
                            map.insert(k, other);
                        }
                    },
                    Some(k) => {
                        map.insert(k, value);
                    }
                }
            }
        }
    }
}

/// A mapping key as text: strings as they are, anything else as JSON.
fn key_text(v: Value) -> String {
    match v {
        Value::String(s) => s,
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn scalar(v: String, style: TScalarStyle, tag: Option<&yaml_rust2::parser::Tag>) -> Value {
    if style != TScalarStyle::Plain || tag.is_some_and(|t| t.suffix == "str") {
        return Value::String(v);
    }
    match Yaml::from_str(&v) {
        Yaml::Integer(i) => Value::from(i),
        Yaml::Real(r) => r
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::String(r)),
        Yaml::Boolean(b) => Value::Bool(b),
        Yaml::Null => Value::Null,
        _ => Value::String(v),
    }
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, ev: Event, _mark: Marker) {
        if self.error.is_some() {
            return;
        }
        match ev {
            Event::Scalar(v, style, anchor, tag) => {
                if self.spend(1) {
                    let value = scalar(v, style, tag.as_ref());
                    self.place(value, 1, anchor);
                }
            }
            Event::Alias(id) => {
                let Some((value, nodes)) = self.anchors.get(&id).cloned() else {
                    self.error = Some(Error::Invalid(
                        "this YAML file refers to an anchor it doesn't define".into(),
                    ));
                    return;
                };
                if self.spend(nodes) {
                    self.place(value, nodes, 0);
                }
            }
            Event::SequenceStart(anchor, _) => {
                if self.spend(1) {
                    self.stack.push(Frame::Seq {
                        items: Vec::new(),
                        anchor,
                        nodes: 1,
                    });
                }
            }
            Event::MappingStart(anchor, _) => {
                if self.spend(1) {
                    self.stack.push(Frame::Map {
                        map: Map::new(),
                        key: None,
                        merges: Vec::new(),
                        anchor,
                        nodes: 1,
                    });
                }
            }
            Event::SequenceEnd => {
                if let Some(Frame::Seq {
                    items,
                    anchor,
                    nodes,
                }) = self.stack.pop()
                {
                    self.place(Value::Array(items), nodes, anchor);
                }
            }
            Event::MappingEnd => {
                if let Some(Frame::Map {
                    mut map,
                    merges,
                    anchor,
                    nodes,
                    ..
                }) = self.stack.pop()
                {
                    // Keys written in the mapping win over merged ones.
                    for m in merges {
                        for (k, v) in m {
                            map.entry(k).or_insert(v);
                        }
                    }
                    self.place(Value::Object(map), nodes, anchor);
                }
            }
            _ => {}
        }
    }
}

fn from_yaml(src: &str) -> Result<Value> {
    let mut builder = Builder::default();
    let mut parser = Parser::new_from_str(src);
    parser
        .load(&mut builder, true)
        .map_err(|e| invalid("YAML", e))?;
    if let Some(e) = builder.error {
        return Err(e);
    }
    let mut docs = builder.docs;
    Ok(match docs.len() {
        0 => Value::Null,
        1 => docs.remove(0),
        // Several documents ("---" between them) become a list.
        _ => Value::Array(docs),
    })
}

/// A scalar as YAML: plain when it reads back as the same string, double
/// quoted (JSON's escaping is valid YAML) otherwise.
fn yaml_string(s: &str) -> String {
    let plain = !s.is_empty()
        && s.trim() == s
        && s.chars().next().is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '(' ))
        && s.chars().all(|c| c.is_alphanumeric() || " _./@()+-".contains(c))
        && !s.contains(" -")
        // Words and numbers YAML would read as something else ("yes",
        // "0o17", "1e3", ".5") stay quoted.
        && matches!(Yaml::from_str(s), Yaml::String(_))
        && !matches!(
            s.to_ascii_lowercase().as_str(),
            "y" | "n" | "yes" | "no" | "on" | "off" | "true" | "false" | "null" | "~"
        )
        && !s.starts_with(|c: char| c.is_ascii_digit() || c == '.');
    if plain {
        s.to_string()
    } else {
        Value::String(s.to_string()).to_string()
    }
}

fn yaml_scalar(v: &Value) -> Option<String> {
    Some(match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => yaml_string(s),
        Value::Array(a) if a.is_empty() => "[]".into(),
        Value::Object(m) if m.is_empty() => "{}".into(),
        _ => return None,
    })
}

/// Block-style YAML, two spaces per level.
fn write_yaml(out: &mut String, v: &Value, depth: usize) {
    let pad = "  ".repeat(depth);
    match v {
        Value::Object(m) if !m.is_empty() => {
            for (k, v) in m {
                out.push_str(&format!("{pad}{}:", yaml_string(k)));
                match yaml_scalar(v) {
                    Some(s) => out.push_str(&format!(" {s}\n")),
                    None => {
                        out.push('\n');
                        write_yaml(out, v, depth + 1);
                    }
                }
            }
        }
        Value::Array(a) if !a.is_empty() => {
            for v in a {
                match yaml_scalar(v) {
                    Some(s) => out.push_str(&format!("{pad}- {s}\n")),
                    None => {
                        // A mapping or list inside a list starts on the
                        // dash's line.
                        let mut inner = String::new();
                        write_yaml(&mut inner, v, depth + 1);
                        let body = inner.trim_start();
                        out.push_str(&format!("{pad}- {body}"));
                    }
                }
            }
        }
        other => {
            out.push_str(&pad);
            out.push_str(&yaml_scalar(other).unwrap_or_default());
            out.push('\n');
        }
    }
}

fn to_yaml(v: &Value) -> Result<String> {
    let mut out = String::new();
    write_yaml(&mut out, v, 0);
    Ok(out)
}

// MARK: TOML

fn json_of_toml(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(f.to_string())),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(json_of_toml).collect()),
        toml::Value::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), json_of_toml(v)))
                .collect(),
        ),
    }
}

fn from_toml(src: &str) -> Result<Value> {
    let table: toml::Table = toml::from_str(src).map_err(|e| invalid("TOML", e.message()))?;
    Ok(json_of_toml(&toml::Value::Table(table)))
}

fn toml_of(v: &Value, path: &str) -> Result<toml::Value> {
    Ok(match v {
        Value::Null => {
            return Err(Error::Invalid(format!(
                "TOML has no empty values, and {} is null",
                if path.is_empty() { "the file" } else { path }
            )))
        }
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => toml::Value::Integer(i),
            None => toml::Value::Float(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(a) => toml::Value::Array(
            a.iter()
                .enumerate()
                .map(|(i, v)| toml_of(v, &format!("{path}[{i}]")))
                .collect::<Result<_>>()?,
        ),
        Value::Object(m) => {
            let mut t = toml::Table::new();
            for (k, v) in m {
                let at = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                t.insert(k.clone(), toml_of(v, &at)?);
            }
            toml::Value::Table(t)
        }
    })
}

fn to_toml(v: &Value) -> Result<String> {
    if !v.is_object() {
        return Err(Error::Invalid(
            "TOML needs keys and values at the top, not a list or a single value".into(),
        ));
    }
    let toml::Value::Table(t) = toml_of(v, "")? else {
        unreachable!("an object becomes a table")
    };
    toml::to_string_pretty(&t).map_err(|e| Error::Invalid(format!("couldn't write TOML ({e})")))
}

// MARK: XML

fn from_xml(src: &str) -> Result<Value> {
    use quick_xml::events::Event as X;
    // Text isn't trimmed piece by piece (an entity splits it): each
    // element's text is tidied once it's complete.
    let mut reader = quick_xml::Reader::from_str(src);
    // Open elements: (name, attributes and children, text).
    let mut stack: Vec<(String, Map<String, Value>, String)> = Vec::new();
    let mut root: Option<(String, Value)> = None;
    let mut nodes = 0usize;
    let attrs = |e: &quick_xml::events::BytesStart| -> Map<String, Value> {
        e.attributes()
            .flatten()
            .map(|a| {
                let v = a
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map(|v| v.into_owned())
                    .unwrap_or_default();
                (format!("@{}", a.key.as_ref()), Value::String(v))
            })
            .collect()
    };
    // An element's value: its text alone when that's all it has.
    let finish = |fields: Map<String, Value>, text: String| -> Value {
        if fields.is_empty() {
            // Only indentation is no text at all.
            if text.trim().is_empty() {
                Value::String(String::new())
            } else {
                Value::String(text)
            }
        } else {
            let mut fields = fields;
            let text = text.trim();
            if !text.is_empty() {
                fields.insert("#text".into(), Value::String(text.to_string()));
            }
            Value::Object(fields)
        }
    };
    fn add(parent: &mut Map<String, Value>, name: String, value: Value) {
        match parent.get_mut(&name) {
            Some(Value::Array(list)) => list.push(value),
            Some(existing) => {
                let first = existing.take();
                *existing = Value::Array(vec![first, value]);
            }
            None => {
                parent.insert(name, value);
            }
        }
    }
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| invalid("XML", format!("at byte {}: {e}", reader.error_position())))?;
        nodes += 1;
        if nodes > MAX_NODES {
            return Err(Error::TooLarge(
                "this XML file has more than 2 million parts".into(),
            ));
        }
        match ev {
            X::Start(e) => {
                // Deep nesting would overflow the stack later on.
                if stack.len() >= MAX_DEPTH {
                    return Err(Error::TooLarge(format!(
                        "this XML file nests more than {MAX_DEPTH} levels deep"
                    )));
                }
                stack.push((e.name().as_ref().to_string(), attrs(&e), String::new()))
            }
            X::Empty(e) => {
                let value = finish(attrs(&e), String::new());
                let name = e.name().as_ref().to_string();
                match stack.last_mut() {
                    Some((_, fields, _)) => add(fields, name, value),
                    None if root.is_none() => root = Some((name, value)),
                    None => {}
                }
            }
            X::Text(t) => {
                if let Some((_, _, text)) = stack.last_mut() {
                    text.push_str(&t.xml10_content());
                }
            }
            X::CData(c) => {
                if let Some((_, _, text)) = stack.last_mut() {
                    text.push_str(&c.xml10_content());
                }
            }
            X::GeneralRef(r) => {
                if let Some((_, _, text)) = stack.last_mut() {
                    let s = match r.resolve_char_ref() {
                        Ok(Some(c)) => c.to_string(),
                        _ => match r.as_ref() {
                            "amp" => "&".into(),
                            "lt" => "<".into(),
                            "gt" => ">".into(),
                            "quot" => "\"".into(),
                            "apos" => "'".into(),
                            other => format!("&{other};"),
                        },
                    };
                    text.push_str(&s);
                }
            }
            X::End(_) => {
                let Some((name, fields, text)) = stack.pop() else {
                    break;
                };
                let value = finish(fields, text);
                match stack.last_mut() {
                    Some((_, parent, _)) => add(parent, name, value),
                    None if root.is_none() => root = Some((name, value)),
                    None => {}
                }
            }
            X::Eof => break,
            _ => {}
        }
    }
    let (name, value) = root.ok_or_else(|| Error::NothingToDo("this XML file is empty".into()))?;
    let mut top = Map::new();
    top.insert(name, value);
    Ok(Value::Object(top))
}

/// A name XML accepts: letters, digits, `_`, `-`, `.` and `:`, not
/// starting with a digit, `-` or `.`.
fn xml_name(key: &str) -> String {
    let mut s: String = key
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') {
        s.insert(0, '_');
    }
    s
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn write_element(out: &mut String, name: &str, v: &Value, depth: usize) {
    let pad = "  ".repeat(depth);
    let tag = xml_name(name);
    match v {
        // A list repeats the element.
        Value::Array(items) => {
            for item in items {
                write_element(out, name, item, depth);
            }
        }
        Value::Object(m) => {
            out.push_str(&format!("{pad}<{tag}"));
            for (k, v) in m.iter().filter(|(k, _)| k.starts_with('@')) {
                let value = quick_xml::escape::escape(text_of(v));
                out.push_str(&format!(" {}=\"{}\"", xml_name(&k[1..]), value));
            }
            let text = m.get("#text").map(text_of).unwrap_or_default();
            let children: Vec<(&String, &Value)> = m
                .iter()
                .filter(|(k, _)| !k.starts_with('@') && k.as_str() != "#text")
                .collect();
            if children.is_empty() && text.is_empty() {
                out.push_str("/>\n");
            } else if children.is_empty() {
                out.push_str(&format!(">{}</{tag}>\n", quick_xml::escape::escape(text)));
            } else {
                out.push_str(">\n");
                if !text.is_empty() {
                    out.push_str(&format!("{pad}  {}\n", quick_xml::escape::escape(text)));
                }
                for (k, v) in children {
                    write_element(out, k, v, depth + 1);
                }
                out.push_str(&format!("{pad}</{tag}>\n"));
            }
        }
        Value::Null => out.push_str(&format!("{pad}<{tag}/>\n")),
        other => out.push_str(&format!(
            "{pad}<{tag}>{}</{tag}>\n",
            quick_xml::escape::escape(text_of(other))
        )),
    }
}

fn to_xml(v: &Value) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    match v {
        // One key at the top is the root element.
        Value::Object(m) if m.len() == 1 && !m.keys().next().unwrap().starts_with('@') => {
            let (k, v) = m.iter().next().unwrap();
            match v {
                Value::Array(_) => {
                    out.push_str("<root>\n");
                    write_element(&mut out, k, v, 1);
                    out.push_str("</root>\n");
                }
                _ => write_element(&mut out, k, v, 0),
            }
        }
        Value::Array(_) => {
            out.push_str("<root>\n");
            write_element(&mut out, "item", v, 1);
            out.push_str("</root>\n");
        }
        other => write_element(&mut out, "root", other, 0),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trips_with_anchors_and_merges() {
        let src = "defaults: &d\n  retries: 3\n  verbose: false\nprod:\n  <<: *d\n  verbose: true\n  hosts: [a, b]\nzip: \"02139\"\nratio: 0.5\nempty: ~\n";
        let json = convert(src, Format::Yaml, Format::Json).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["prod"]["retries"], 3, "merged from the anchor");
        assert_eq!(v["prod"]["verbose"], true, "the mapping's own key wins");
        assert_eq!(v["prod"]["hosts"][1], "b");
        assert_eq!(v["zip"], "02139", "quoted stays text");
        assert_eq!(v["ratio"], 0.5);
        assert!(v["empty"].is_null());
        let back = convert(&json, Format::Json, Format::Yaml).unwrap();
        let again: Value =
            serde_json::from_str(&convert(&back, Format::Yaml, Format::Json).unwrap()).unwrap();
        assert_eq!(again, v);
        assert!(!back.starts_with("---"));
    }

    #[test]
    fn yaml_keeps_strings_strings() {
        let src = r##"{"a":"0o17","b":"yes","c":"2026-10-27","d":"two\nlines","e":" pad","f":"plain text","g":[{"x":1,"y":[1,[2,3]]},[]],"h":{},"i":"#tag","j":"k: v","k":null,"l":"1e3"}"##;
        let v: Value = serde_json::from_str(src).unwrap();
        let yaml = convert(src, Format::Json, Format::Yaml).unwrap();
        assert!(yaml.contains("f: plain text\n"), "{yaml}");
        let back: Value =
            serde_json::from_str(&convert(&yaml, Format::Yaml, Format::Json).unwrap()).unwrap();
        assert_eq!(back, v, "{yaml}");
    }

    #[test]
    fn deep_xml_is_refused() {
        let xml = format!("{}{}", "<a>".repeat(10_000), "</a>".repeat(10_000));
        assert!(matches!(
            convert(&xml, Format::Xml, Format::Json),
            Err(Error::TooLarge(_))
        ));
    }

    #[test]
    fn billion_laughs_is_refused() {
        let mut src = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
        for i in 1..9 {
            let refs = vec![format!("*a{}", i - 1); 10].join(", ");
            src.push_str(&format!("a{i}: &a{i} [{refs}]\n"));
        }
        assert!(matches!(
            convert(&src, Format::Yaml, Format::Json),
            Err(Error::TooLarge(_))
        ));
    }

    #[test]
    fn toml_both_ways() {
        let src = "title = \"Radlet\"\nwhen = 2026-10-27T09:00:00Z\n[server]\nport = 8080\nhosts = [\"a\", \"b\"]\n";
        let v: Value =
            serde_json::from_str(&convert(src, Format::Toml, Format::Json).unwrap()).unwrap();
        assert_eq!(v["server"]["port"], 8080);
        assert_eq!(v["when"], "2026-10-27T09:00:00Z");
        let toml = convert(
            "{\"a\": 1, \"t\": {\"b\": [1, 2]}}",
            Format::Json,
            Format::Toml,
        )
        .unwrap();
        assert!(toml.contains("a = 1") && toml.contains("[t]"), "{toml}");
        assert!(matches!(
            convert("{\"a\": null}", Format::Json, Format::Toml),
            Err(Error::Invalid(_))
        ));
        assert!(convert("[1, 2]", Format::Json, Format::Toml).is_err());
    }

    #[test]
    fn xml_both_ways() {
        let src = r#"<?xml version="1.0"?><library open="yes"><book id="1"><title>Dune &amp; more</title></book><book id="2"><title>Emma</title></book><note>hi<![CDATA[ <b>]]></note></library>"#;
        let v: Value =
            serde_json::from_str(&convert(src, Format::Xml, Format::Json).unwrap()).unwrap();
        assert_eq!(v["library"]["@open"], "yes");
        assert_eq!(v["library"]["book"][0]["@id"], "1");
        assert_eq!(v["library"]["book"][0]["title"], "Dune & more");
        assert_eq!(v["library"]["book"][1]["title"], "Emma");
        assert_eq!(v["library"]["note"], "hi <b>");
        let xml = convert(&v.to_string(), Format::Json, Format::Xml).unwrap();
        assert!(xml.contains("<library open=\"yes\">"), "{xml}");
        assert!(xml.contains("<title>Dune &amp; more</title>"), "{xml}");
        let again: Value =
            serde_json::from_str(&convert(&xml, Format::Xml, Format::Json).unwrap()).unwrap();
        assert_eq!(again, v);
        assert!(convert("<a><b></a>", Format::Xml, Format::Json).is_err());
        let list = convert("[1, {\"x y\": 2}]", Format::Json, Format::Xml).unwrap();
        assert!(list.contains("<x_y>2</x_y>"), "{list}");
    }
}
