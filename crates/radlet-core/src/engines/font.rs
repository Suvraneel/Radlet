//! Fonts: TrueType and OpenType desktop fonts, and WOFF and WOFF2 web
//! fonts, into one another. The outlines are never touched; only the
//! packaging changes.
//!
//! - WOFF compresses each table with zlib.
//! - WOFF2 compresses all tables together with Brotli. Radlet writes the
//!   glyph tables unchanged (WOFF2's "null transform"), and reads WOFF2
//!   files that use the glyf, loca and hmtx transforms too.
//!
//! Font collections (.ttc) aren't handled.

use std::io::{Read, Write};
use std::path::Path;

use crate::error::{Error, Result};
use crate::format::Format;

/// Most bytes a font may unpack to.
const MAX_FONT: u64 = 256 << 20;
/// Most tables a font may have.
const MAX_TABLES: usize = 1024;

const TRUETYPE: u32 = 0x0001_0000;
const CFF: u32 = u32::from_be_bytes(*b"OTTO");
const COLLECTION: u32 = u32::from_be_bytes(*b"ttcf");

#[derive(Debug, Clone)]
struct Table {
    tag: [u8; 4],
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Font {
    flavor: u32,
    tables: Vec<Table>,
}

impl Font {
    fn table(&self, tag: &[u8; 4]) -> Option<&[u8]> {
        self.tables
            .iter()
            .find(|t| &t.tag == tag)
            .map(|t| t.data.as_slice())
    }
}

fn bad(what: &str) -> Error {
    Error::Invalid(format!("this font doesn't read ({what})"))
}

/// A cursor over big-endian font data.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("truncated"))?;
        let s = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| bad("truncated"))?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn tag(&mut self) -> Result<[u8; 4]> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }
    /// WOFF2's UIntBase128: up to five bytes, seven bits each.
    fn base128(&mut self) -> Result<u32> {
        let mut v: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            if i == 0 && b == 0x80 {
                return Err(bad("a number with a leading zero"));
            }
            if v & 0xFE00_0000 != 0 {
                return Err(bad("a number too large"));
            }
            v = (v << 7) | u32::from(b & 0x7F);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(bad("a number too long"))
    }
    /// WOFF2's 255UInt16.
    fn u255(&mut self) -> Result<u16> {
        Ok(match self.u8()? {
            253 => self.u16()?,
            255 => u16::from(self.u8()?) + 253,
            254 => u16::from(self.u8()?) + 506,
            c => u16::from(c),
        })
    }
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(4) {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
    }
    sum
}

fn base128(v: u32, out: &mut Vec<u8>) {
    let mut bytes = [0u8; 5];
    let mut n = 0;
    let mut rest = v;
    loop {
        bytes[n] = (rest & 0x7F) as u8;
        n += 1;
        rest >>= 7;
        if rest == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        out.push(bytes[i] | if i > 0 { 0x80 } else { 0 });
    }
}

// MARK: sfnt (TTF, OTF)

fn read_sfnt(bytes: &[u8]) -> Result<Font> {
    let mut r = Reader::new(bytes);
    let flavor = r.u32()?;
    if flavor == COLLECTION {
        return Err(Error::UnsupportedInput(
            "font collections (.ttc) aren't handled yet".into(),
        ));
    }
    if flavor != TRUETYPE && flavor != CFF && flavor != u32::from_be_bytes(*b"true") {
        return Err(bad("not a TrueType or OpenType font"));
    }
    let n = r.u16()? as usize;
    if n == 0 || n > MAX_TABLES {
        return Err(bad("an odd number of tables"));
    }
    r.take(6)?;
    let mut tables = Vec::with_capacity(n);
    // Tables may overlap; copies of them may not add up past the limit.
    let mut total = 0u64;
    for _ in 0..n {
        let tag = r.tag()?;
        let _sum = r.u32()?;
        let offset = r.u32()? as usize;
        let length = r.u32()? as usize;
        total += length as u64;
        if total > MAX_FONT {
            return Err(Error::TooLarge(
                "this font's tables add up to more than 256 MB".into(),
            ));
        }
        let data = offset
            .checked_add(length)
            .and_then(|end| bytes.get(offset..end))
            .ok_or_else(|| bad("a table outside the file"))?;
        tables.push(Table {
            tag,
            data: data.to_vec(),
        });
    }
    Ok(Font { flavor, tables })
}

fn write_sfnt(font: &Font) -> Vec<u8> {
    let mut tables: Vec<&Table> = font.tables.iter().collect();
    tables.sort_by_key(|t| t.tag);
    let n = tables.len();
    let mut pow = 1usize;
    let mut log = 0u16;
    while pow * 2 <= n {
        pow *= 2;
        log += 1;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&font.flavor.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&((pow * 16) as u16).to_be_bytes());
    out.extend_from_slice(&log.to_be_bytes());
    out.extend_from_slice(&((n * 16 - pow * 16) as u16).to_be_bytes());
    let mut offset = 12 + 16 * n;
    let mut head_at = None;
    for t in &tables {
        let mut data = t.data.clone();
        if &t.tag == b"head" && data.len() >= 12 {
            data[8..12].fill(0);
            head_at = Some(offset);
        }
        out.extend_from_slice(&t.tag);
        out.extend_from_slice(&checksum(&data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += pad4(data.len());
    }
    for t in &tables {
        let start = out.len();
        out.extend_from_slice(&t.data);
        if &t.tag == b"head" && t.data.len() >= 12 {
            out[start + 8..start + 12].fill(0);
        }
        out.resize(pad4(out.len()), 0);
    }
    if let Some(at) = head_at {
        let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[at + 8..at + 12].copy_from_slice(&adjust.to_be_bytes());
    }
    out
}

// MARK: WOFF

fn read_woff(bytes: &[u8]) -> Result<Font> {
    let mut r = Reader::new(bytes);
    if r.tag()? != *b"wOFF" {
        return Err(bad("not a WOFF file"));
    }
    let flavor = r.u32()?;
    let _length = r.u32()?;
    let n = r.u16()? as usize;
    if n == 0 || n > MAX_TABLES {
        return Err(bad("an odd number of tables"));
    }
    r.take(2 + 4 + 4 + 4 * 5)?;
    let mut tables = Vec::with_capacity(n);
    let mut total = 0u64;
    for _ in 0..n {
        let tag = r.tag()?;
        let offset = r.u32()? as usize;
        let comp = r.u32()? as usize;
        let orig = r.u32()? as usize;
        let _sum = r.u32()?;
        total += orig as u64;
        if total > MAX_FONT {
            return Err(Error::TooLarge(
                "this font unpacks to more than 256 MB".into(),
            ));
        }
        if comp > orig {
            return Err(bad("a table stored larger than it is"));
        }
        let stored = offset
            .checked_add(comp)
            .and_then(|end| bytes.get(offset..end))
            .ok_or_else(|| bad("a table outside the file"))?;
        let data = if comp < orig {
            let mut d = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(stored)
                .take(orig as u64 + 1)
                .read_to_end(&mut d)
                .map_err(|_| bad("a damaged table"))?;
            if d.len() != orig {
                return Err(bad("a table of the wrong size"));
            }
            d
        } else {
            stored.to_vec()
        };
        tables.push(Table { tag, data });
    }
    Ok(Font { flavor, tables })
}

fn write_woff(font: &Font) -> Result<Vec<u8>> {
    let sfnt_size = 12
        + 16 * font.tables.len()
        + font
            .tables
            .iter()
            .map(|t| pad4(t.data.len()))
            .sum::<usize>();
    let mut tables: Vec<&Table> = font.tables.iter().collect();
    tables.sort_by_key(|t| t.tag);
    let mut dir = Vec::new();
    let mut body = Vec::new();
    let start = 44 + 20 * tables.len();
    for t in &tables {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(&t.data)?;
        let packed = z.finish()?;
        let stored = if packed.len() < t.data.len() {
            packed
        } else {
            t.data.clone()
        };
        let mut sum_data = t.data.clone();
        if &t.tag == b"head" && sum_data.len() >= 12 {
            sum_data[8..12].fill(0);
        }
        dir.extend_from_slice(&t.tag);
        dir.extend_from_slice(&((start + body.len()) as u32).to_be_bytes());
        dir.extend_from_slice(&(stored.len() as u32).to_be_bytes());
        dir.extend_from_slice(&(t.data.len() as u32).to_be_bytes());
        dir.extend_from_slice(&checksum(&sum_data).to_be_bytes());
        body.extend_from_slice(&stored);
        body.resize(pad4(body.len()), 0);
    }
    let total = start + body.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&font.flavor.to_be_bytes());
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(sfnt_size as u32).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&[0u8; 20]);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&body);
    Ok(out)
}

// MARK: WOFF2

const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

struct Entry {
    tag: [u8; 4],
    version: u8,
    /// Bytes in the decompressed stream.
    stored: u32,
}

fn transformed(e: &Entry) -> bool {
    match &e.tag {
        b"glyf" | b"loca" => e.version == 0,
        _ => e.version != 0,
    }
}

fn read_woff2(bytes: &[u8]) -> Result<Font> {
    let mut r = Reader::new(bytes);
    if r.tag()? != *b"wOF2" {
        return Err(bad("not a WOFF2 file"));
    }
    let flavor = r.u32()?;
    if flavor == COLLECTION {
        return Err(Error::UnsupportedInput(
            "font collections (.ttc) aren't handled yet".into(),
        ));
    }
    let _length = r.u32()?;
    let n = r.u16()? as usize;
    if n == 0 || n > MAX_TABLES {
        return Err(bad("an odd number of tables"));
    }
    let _reserved = r.u16()?;
    let total_sfnt = r.u32()?;
    let compressed = r.u32()? as usize;
    r.take(4 + 4 * 5)?;
    if u64::from(total_sfnt) > MAX_FONT {
        return Err(Error::TooLarge(
            "this font unpacks to more than 256 MB".into(),
        ));
    }
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        let flags = r.u8()?;
        let tag = match flags & 0x3F {
            63 => r.tag()?,
            i => *KNOWN_TAGS[i as usize],
        };
        let version = flags >> 6;
        let orig = r.base128()?;
        let mut e = Entry {
            tag,
            version,
            stored: orig,
        };
        if transformed(&e) {
            e.stored = r.base128()?;
        }
        if &e.tag == b"loca" && transformed(&e) && e.stored != 0 {
            return Err(bad("a transformed loca table with data"));
        }
        entries.push(e);
    }
    let expected: u64 = entries.iter().map(|e| u64::from(e.stored)).sum();
    if expected > MAX_FONT {
        return Err(Error::TooLarge(
            "this font unpacks to more than 256 MB".into(),
        ));
    }
    let packed = r.take(compressed)?;
    let mut stream = Vec::with_capacity(expected as usize);
    brotli::Decompressor::new(packed, 4096)
        .take(expected + 1)
        .read_to_end(&mut stream)
        .map_err(|_| bad("damaged compressed data"))?;
    if stream.len() as u64 != expected {
        return Err(bad("compressed data of the wrong size"));
    }
    let mut slices: Vec<&[u8]> = Vec::with_capacity(n);
    let mut at = 0usize;
    for e in &entries {
        slices.push(&stream[at..at + e.stored as usize]);
        at += e.stored as usize;
    }

    let mut tables: Vec<Table> = Vec::with_capacity(n);
    // Glyph left edges, for an hmtx table stored without them.
    let mut x_mins: Option<Vec<i16>> = None;
    let glyf_at = entries.iter().position(|e| &e.tag == b"glyf");
    if let Some(gi) = glyf_at {
        if transformed(&entries[gi]) {
            let (glyf, loca, mins) = rebuild_glyf(slices[gi])?;
            tables.push(Table {
                tag: *b"glyf",
                data: glyf,
            });
            tables.push(Table {
                tag: *b"loca",
                data: loca,
            });
            x_mins = Some(mins);
        }
    }
    for (e, data) in entries.iter().zip(&slices) {
        if (&e.tag == b"glyf" || &e.tag == b"loca") && x_mins.is_some() {
            continue;
        }
        if &e.tag == b"hmtx" && transformed(e) {
            continue;
        }
        if transformed(e) && &e.tag != b"glyf" && &e.tag != b"loca" {
            return Err(bad("a table transformed in a way Radlet doesn't know"));
        }
        tables.push(Table {
            tag: e.tag,
            data: data.to_vec(),
        });
    }
    if let Some(hi) = entries
        .iter()
        .position(|e| &e.tag == b"hmtx" && transformed(e))
    {
        let font = Font {
            flavor,
            tables: tables.clone(),
        };
        let hmtx = rebuild_hmtx(slices[hi], &font, x_mins.as_deref())?;
        tables.push(Table {
            tag: *b"hmtx",
            data: hmtx,
        });
    }
    Ok(Font { flavor, tables })
}

/// Rebuilds glyf and loca from WOFF2's transformed glyf table. Returns
/// them and each glyph's left edge.
fn rebuild_glyf(data: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Vec<i16>)> {
    let mut h = Reader::new(data);
    let _reserved = h.u16()?;
    let options = h.u16()?;
    let num_glyphs = h.u16()? as usize;
    let index_format = h.u16()?;
    let mut sizes = [0usize; 7];
    for s in &mut sizes {
        *s = h.u32()? as usize;
    }
    let mut n_contours = Reader::new(h.take(sizes[0])?);
    let mut n_points = Reader::new(h.take(sizes[1])?);
    let mut flags = Reader::new(h.take(sizes[2])?);
    let mut glyphs = Reader::new(h.take(sizes[3])?);
    let mut composites = Reader::new(h.take(sizes[4])?);
    let mut bbox = Reader::new(h.take(sizes[5])?);
    let mut instructions = Reader::new(h.take(sizes[6])?);
    let bitmap_len = ((num_glyphs + 31) >> 5) << 2;
    let overlap = if options & 1 != 0 {
        Some(h.take(bitmap_len)?)
    } else {
        None
    };
    let bbox_bitmap = bbox.take(bitmap_len)?;
    let has_bit = |map: &[u8], i: usize| map[i >> 3] & (0x80 >> (i & 7)) != 0;

    let mut glyf = Vec::new();
    let mut offsets = Vec::with_capacity(num_glyphs + 1);
    let mut x_mins = Vec::with_capacity(num_glyphs);
    let long = index_format != 0;
    for i in 0..num_glyphs {
        offsets.push(glyf.len());
        let contours = n_contours.i16()?;
        let explicit = has_bit(bbox_bitmap, i);
        let mut glyph = Vec::new();
        let x_min;
        if contours == 0 {
            if explicit {
                return Err(bad("an empty glyph with a bounding box"));
            }
            x_min = 0;
        } else if contours > 0 {
            let mut ends = Vec::with_capacity(contours as usize);
            let mut total = 0usize;
            for _ in 0..contours {
                total += n_points.u255()? as usize;
                ends.push(
                    total
                        .checked_sub(1)
                        .ok_or_else(|| bad("an empty outline"))?,
                );
            }
            if total > 0xFFFF {
                return Err(bad("a glyph with too many points"));
            }
            let point_flags = flags.take(total)?;
            let mut points = Vec::with_capacity(total);
            let (mut x, mut y) = (0i32, 0i32);
            for &f in point_flags {
                let on = f >> 7 == 0;
                let f = f & 0x7F;
                let sign = |flag: u8, v: i32| if flag & 1 != 0 { v } else { -v };
                let (dx, dy) = if f < 10 {
                    (
                        0,
                        sign(f, ((i32::from(f) & 14) << 7) + i32::from(glyphs.u8()?)),
                    )
                } else if f < 20 {
                    (
                        sign(
                            f,
                            (((i32::from(f) - 10) & 14) << 7) + i32::from(glyphs.u8()?),
                        ),
                        0,
                    )
                } else if f < 84 {
                    let b0 = i32::from(f) - 20;
                    let b1 = i32::from(glyphs.u8()?);
                    (
                        sign(f, 1 + (b0 & 0x30) + (b1 >> 4)),
                        sign(f >> 1, 1 + ((b0 & 0x0C) << 2) + (b1 & 0x0F)),
                    )
                } else if f < 120 {
                    let b0 = i32::from(f) - 84;
                    let b = glyphs.take(2)?;
                    (
                        sign(f, 1 + ((b0 / 12) << 8) + i32::from(b[0])),
                        sign(f >> 1, 1 + (((b0 % 12) >> 2) << 8) + i32::from(b[1])),
                    )
                } else if f < 124 {
                    let b = glyphs.take(3)?;
                    let b2 = i32::from(b[1]);
                    (
                        sign(f, (i32::from(b[0]) << 4) + (b2 >> 4)),
                        sign(f >> 1, ((b2 & 0x0F) << 8) + i32::from(b[2])),
                    )
                } else {
                    let b = glyphs.take(4)?;
                    (
                        sign(f, (i32::from(b[0]) << 8) + i32::from(b[1])),
                        sign(f >> 1, (i32::from(b[2]) << 8) + i32::from(b[3])),
                    )
                };
                x += dx;
                y += dy;
                points.push((x, y, on));
            }
            let instruction_len = glyphs.u255()? as usize;
            let code = instructions.take(instruction_len)?;
            let (mut x0, mut y0, mut x1, mut y1) = if explicit {
                (
                    i32::from(bbox.i16()?),
                    i32::from(bbox.i16()?),
                    i32::from(bbox.i16()?),
                    i32::from(bbox.i16()?),
                )
            } else {
                (i32::MAX, i32::MAX, i32::MIN, i32::MIN)
            };
            if !explicit {
                for &(px, py, _) in &points {
                    x0 = x0.min(px);
                    y0 = y0.min(py);
                    x1 = x1.max(px);
                    y1 = y1.max(py);
                }
                if points.is_empty() {
                    (x0, y0, x1, y1) = (0, 0, 0, 0);
                }
            }
            x_min = x0 as i16;
            for v in [contours as i32, x0, y0, x1, y1] {
                glyph.extend_from_slice(&(v as i16).to_be_bytes());
            }
            for e in &ends {
                glyph.extend_from_slice(&(*e as u16).to_be_bytes());
            }
            glyph.extend_from_slice(&(instruction_len as u16).to_be_bytes());
            glyph.extend_from_slice(code);
            let overlaps = overlap.is_some_and(|m| has_bit(m, i));
            encode_points(&points, overlaps, &mut glyph);
        } else {
            // A composite: its components as stored, then its instructions.
            if !explicit {
                return Err(bad("a composite glyph without a bounding box"));
            }
            let b = bbox.take(8)?;
            x_min = i16::from_be_bytes([b[0], b[1]]);
            glyph.extend_from_slice(&(-1i16).to_be_bytes());
            glyph.extend_from_slice(b);
            let mut has_instructions = false;
            loop {
                let flag = composites.u16()?;
                let index = composites.u16()?;
                glyph.extend_from_slice(&flag.to_be_bytes());
                glyph.extend_from_slice(&index.to_be_bytes());
                let mut extra = if flag & 0x0001 != 0 { 4 } else { 2 };
                if flag & 0x0008 != 0 {
                    extra += 2;
                } else if flag & 0x0040 != 0 {
                    extra += 4;
                } else if flag & 0x0080 != 0 {
                    extra += 8;
                }
                glyph.extend_from_slice(composites.take(extra)?);
                has_instructions |= flag & 0x0100 != 0;
                if flag & 0x0020 == 0 {
                    break;
                }
            }
            if has_instructions {
                let len = glyphs.u255()? as usize;
                glyph.extend_from_slice(&(len as u16).to_be_bytes());
                glyph.extend_from_slice(instructions.take(len)?);
            }
        }
        x_mins.push(x_min);
        glyf.extend_from_slice(&glyph);
        let align = if long { 4 } else { 2 };
        while glyf.len() % align != 0 {
            glyf.push(0);
        }
        if glyf.len() as u64 > MAX_FONT {
            return Err(Error::TooLarge(
                "this font unpacks to more than 256 MB".into(),
            ));
        }
    }
    offsets.push(glyf.len());
    let mut loca = Vec::new();
    for o in offsets {
        if long {
            loca.extend_from_slice(&(o as u32).to_be_bytes());
        } else {
            if o / 2 > 0xFFFF {
                return Err(bad("glyphs too large for a short index"));
            }
            loca.extend_from_slice(&((o / 2) as u16).to_be_bytes());
        }
    }
    Ok((glyf, loca, x_mins))
}

/// A simple glyph's points in the glyf table's own packed form.
fn encode_points(points: &[(i32, i32, bool)], overlap: bool, out: &mut Vec<u8>) {
    let mut flags = Vec::with_capacity(points.len());
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let (mut px, mut py) = (0i32, 0i32);
    for (i, &(x, y, on)) in points.iter().enumerate() {
        let mut f = if on { 0x01 } else { 0 };
        if i == 0 && overlap {
            f |= 0x40;
        }
        let (dx, dy) = (x - px, y - py);
        (px, py) = (x, y);
        if dx == 0 {
            f |= 0x10;
        } else if dx.abs() < 256 {
            f |= 0x02 | if dx > 0 { 0x10 } else { 0 };
            xs.push(dx.unsigned_abs() as u8);
        } else {
            xs.extend_from_slice(&(dx as i16).to_be_bytes());
        }
        if dy == 0 {
            f |= 0x20;
        } else if dy.abs() < 256 {
            f |= 0x04 | if dy > 0 { 0x20 } else { 0 };
            ys.push(dy.unsigned_abs() as u8);
        } else {
            ys.extend_from_slice(&(dy as i16).to_be_bytes());
        }
        flags.push(f);
    }
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i];
        let mut run = 1;
        while i + run < flags.len() && flags[i + run] == f && run < 256 {
            run += 1;
        }
        if run > 1 {
            out.push(f | 0x08);
            out.push((run - 1) as u8);
        } else {
            out.push(f);
        }
        i += run;
    }
    out.extend_from_slice(&xs);
    out.extend_from_slice(&ys);
}

/// Rebuilds hmtx from WOFF2's transformed form, taking left side bearings
/// it left out from the glyphs' left edges.
fn rebuild_hmtx(data: &[u8], font: &Font, x_mins: Option<&[i16]>) -> Result<Vec<u8>> {
    let hhea = font.table(b"hhea").ok_or_else(|| bad("no hhea table"))?;
    let maxp = font.table(b"maxp").ok_or_else(|| bad("no maxp table"))?;
    let metrics = Reader::new(hhea.get(34..36).ok_or_else(|| bad("a short hhea"))?).u16()? as usize;
    let glyphs = Reader::new(maxp.get(4..6).ok_or_else(|| bad("a short maxp"))?).u16()? as usize;
    if metrics == 0 || metrics > glyphs {
        return Err(bad("odd horizontal metrics"));
    }
    let mut r = Reader::new(data);
    let flags = r.u8()?;
    let mut advances = Vec::with_capacity(metrics);
    for _ in 0..metrics {
        advances.push(r.u16()?);
    }
    let edge = |i: usize| -> Result<i16> {
        x_mins
            .and_then(|m| m.get(i).copied())
            .ok_or_else(|| bad("left side bearings left out without glyph outlines"))
    };
    let mut out = Vec::new();
    let mut lsbs = Vec::with_capacity(glyphs);
    for i in 0..metrics {
        lsbs.push(if flags & 1 != 0 { edge(i)? } else { r.i16()? });
    }
    for i in metrics..glyphs {
        lsbs.push(if flags & 2 != 0 { edge(i)? } else { r.i16()? });
    }
    for (i, lsb) in lsbs.iter().enumerate() {
        if i < metrics {
            out.extend_from_slice(&advances[i].to_be_bytes());
        }
        out.extend_from_slice(&lsb.to_be_bytes());
    }
    Ok(out)
}

fn write_woff2(font: &Font) -> Result<Vec<u8>> {
    // Sorted, with loca straight after glyf as WOFF2 expects.
    let mut tables: Vec<&Table> = font.tables.iter().collect();
    tables.sort_by_key(|t| t.tag);
    if let (Some(g), Some(l)) = (
        tables.iter().position(|t| &t.tag == b"glyf"),
        tables.iter().position(|t| &t.tag == b"loca"),
    ) {
        let loca = tables.remove(l);
        let g = if l < g { g - 1 } else { g };
        tables.insert(g + 1, loca);
    }
    let mut dir = Vec::new();
    let mut stream = Vec::new();
    for t in &tables {
        let index = KNOWN_TAGS.iter().position(|k| **k == t.tag);
        // glyf and loca as they are: the null transform (version 3).
        let version: u8 = if &t.tag == b"glyf" || &t.tag == b"loca" {
            3
        } else {
            0
        };
        match index {
            Some(i) => dir.push((version << 6) | i as u8),
            None => {
                dir.push((version << 6) | 63);
                dir.extend_from_slice(&t.tag);
            }
        }
        base128(t.data.len() as u32, &mut dir);
        stream.extend_from_slice(&t.data);
    }
    let mut packed = Vec::new();
    let params = brotli::enc::BrotliEncoderParams {
        quality: 11,
        lgwin: 22,
        mode: brotli::enc::backward_references::BrotliEncoderMode::BROTLI_MODE_FONT,
        size_hint: stream.len(),
        ..Default::default()
    };
    brotli::BrotliCompress(&mut stream.as_slice(), &mut packed, &params)?;
    let sfnt_size =
        12 + 16 * tables.len() + tables.iter().map(|t| pad4(t.data.len())).sum::<usize>();
    let header = 48;
    let total = pad4(header + dir.len() + packed.len());
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"wOF2");
    out.extend_from_slice(&font.flavor.to_be_bytes());
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(sfnt_size as u32).to_be_bytes());
    out.extend_from_slice(&(packed.len() as u32).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&[0u8; 20]);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&packed);
    out.resize(total, 0);
    Ok(out)
}

// MARK: Converting

fn read(bytes: &[u8], from: Format) -> Result<Font> {
    match from {
        Format::Ttf | Format::Otf => read_sfnt(bytes),
        Format::Woff => read_woff(bytes),
        Format::Woff2 => read_woff2(bytes),
        other => Err(Error::UnsupportedInput(format!(
            "{other} files aren't fonts"
        ))),
    }
}

/// Repackages the font at `input` as `to`.
pub fn convert(input: &Path, from: Format, to: Format, out: &Path) -> Result<()> {
    if std::fs::metadata(input)?.len() > MAX_FONT {
        return Err(Error::TooLarge("this font is larger than 256 MB".into()));
    }
    let font = read(&std::fs::read(input)?, from)?;
    if font.table(b"head").is_none() {
        return Err(bad("no head table"));
    }
    let bytes = match to {
        Format::Ttf if font.flavor == CFF => {
            return Err(Error::UnsupportedRoute {
                from: "a font with PostScript (CFF) outlines".into(),
                to: "TTF; convert it to OTF instead".into(),
            })
        }
        Format::Ttf | Format::Otf => write_sfnt(&font),
        Format::Woff => write_woff(&font)?,
        Format::Woff2 => write_woff2(&font)?,
        other => {
            return Err(Error::UnsupportedRoute {
                from: from.label().into(),
                to: other.label().into(),
            })
        }
    };
    std::fs::write(out, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/fonts")
                .join(name),
        )
        .unwrap()
    }

    /// Tables as a map, with head's checksum adjustment cleared.
    fn tables(font: &Font) -> std::collections::BTreeMap<[u8; 4], Vec<u8>> {
        font.tables
            .iter()
            .map(|t| {
                let mut d = t.data.clone();
                if &t.tag == b"head" {
                    d[8..12].fill(0);
                }
                (t.tag, d)
            })
            .collect()
    }

    #[test]
    fn woff_and_woff2_round_trip() {
        let ttf = read_sfnt(&fixture("radlet-test.ttf")).unwrap();
        let woff = read_woff(&write_woff(&ttf).unwrap()).unwrap();
        assert_eq!(tables(&woff), tables(&ttf));
        let woff2 = read_woff2(&write_woff2(&ttf).unwrap()).unwrap();
        assert_eq!(tables(&woff2), tables(&ttf));
        let back = read_sfnt(&write_sfnt(&woff2)).unwrap();
        assert_eq!(tables(&back), tables(&ttf));
        let otf = read_sfnt(&fixture("radlet-test.otf")).unwrap();
        let otf2 = read_woff2(&write_woff2(&otf).unwrap()).unwrap();
        assert_eq!(otf2.flavor, CFF);
        assert_eq!(tables(&otf2), tables(&otf));
    }

    #[test]
    fn whole_file_checksum_is_right() {
        let ttf = read_sfnt(&fixture("radlet-test.ttf")).unwrap();
        let bytes = write_sfnt(&ttf);
        assert_eq!(checksum(&bytes), 0xB1B0_AFBA);
    }

    #[test]
    fn transformed_woff2_from_other_tools() {
        // Made by fontTools, with glyf and loca transformed (and, in the
        // second, hmtx too): the glyph outlines must come back exactly.
        let ttf = read_sfnt(&fixture("radlet-test.ttf")).unwrap();
        for name in ["radlet-test.woff2", "radlet-test-hmtx.woff2"] {
            let woff2 = read_woff2(&fixture(name)).unwrap();
            let (a, b) = (tables(&woff2), tables(&ttf));
            for tag in [b"glyf", b"loca", b"hmtx", b"cmap", b"maxp"] {
                assert_eq!(
                    a.get(tag),
                    b.get(tag),
                    "{name}: {}",
                    String::from_utf8_lossy(tag)
                );
            }
        }
        let woff = read_woff(&fixture("radlet-test.woff")).unwrap();
        assert_eq!(tables(&woff), tables(&ttf));
    }

    #[test]
    fn numbers_and_damage() {
        let mut v = Vec::new();
        base128(63, &mut v);
        base128(1_000_000, &mut v);
        let mut r = Reader::new(&v);
        assert_eq!(
            (r.base128().unwrap(), r.base128().unwrap()),
            (63, 1_000_000)
        );
        assert!(
            Reader::new(&[0x80, 0x01]).base128().is_err(),
            "leading zero"
        );
        assert_eq!(Reader::new(&[253, 1, 0]).u255().unwrap(), 256);
        assert_eq!(Reader::new(&[254, 10]).u255().unwrap(), 516);
        let mut woff2 = fixture("radlet-test.woff2");
        let len = woff2.len();
        woff2.truncate(len - 40);
        assert!(read_woff2(&woff2).is_err());
        assert!(read_sfnt(b"ttcf\0\x01\0\0\0\0\0\x01").is_err());
    }
}
