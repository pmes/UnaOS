// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The cross-reference: where each object lives (ISO 32000-1 §7.5.4 tables, §7.5.8 streams, §7.5.6 incremental
//! updates) and the reconstruction scan for files whose xref is broken.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::filters;
use crate::lexer::{Lexer, Token, find, is_delim, is_white, rfind};
use crate::object::{Dict, Object, parse_indirect, parse_object};
use crate::{Error, MAX_OBJECTS};

/// Where one object lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefEntry {
    /// Free (or deleted by an update): resolves to null.
    Free,
    /// An indirect object at a byte offset.
    InFile { offset: usize, generation: u16 },
    /// Object `idx` of the object stream `stm` (§7.5.7).
    InStream { stm: u32, idx: u32 },
}

/// How the cross-reference was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefKind {
    /// The newest section is a classic `xref` table.
    Table,
    /// The newest section is an xref stream.
    Stream,
    /// The xref was broken and the file was scanned for objects.
    Rebuilt,
}

impl XrefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            XrefKind::Table => "table",
            XrefKind::Stream => "stream",
            XrefKind::Rebuilt => "rebuilt",
        }
    }
}

/// A loaded cross-reference: every section merged, newest first.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub entries: BTreeMap<u32, XrefEntry>,
    pub trailer: Dict,
    pub kind: XrefKind,
}

/// The offset after the last `startxref` in the file's last 1024 bytes.
pub fn find_startxref(buf: &[u8]) -> Option<usize> {
    let from = buf.len().saturating_sub(1024);
    let at = rfind(&buf[from..], b"startxref")? + from;
    match Lexer::at(buf, at + 9).next() {
        Some(Token::Int(n)) if n >= 0 && (n as u64) < buf.len() as u64 => Some(n as usize),
        _ => None,
    }
}

/// The most sections one /Prev chain may hold.
const MAX_SECTIONS: usize = 4096;

/// Load the cross-reference from `startxref`, following /Prev and /XRefStm. A section already present in the
/// merged map keeps its newer entry. A broken newest section is an error (the caller reconstructs); a broken older
/// section ends the chain.
pub fn load(buf: &[u8]) -> Result<Loaded, Error> {
    let start = find_startxref(buf).ok_or(Error::Syntax("no startxref", buf.len()))?;
    let mut entries = BTreeMap::new();
    let mut trailer = Dict::new();
    let mut kind = XrefKind::Table;
    let mut visited = BTreeSet::new();
    let mut next = Some(start);
    let mut first = true;
    while let Some(off) = next {
        if !visited.insert(off) || visited.len() > MAX_SECTIONS {
            break;
        }
        let (mut map, tr, is_stream) = match section(buf, off) {
            Ok(s) => s,
            Err(e) if first => return Err(e),
            Err(_) => break,
        };
        // §7.5.8.4 hybrid file: the hidden xref stream supplies what the table leaves free.
        if let Some(x) = tr.int(b"XRefStm") {
            if x >= 0 && visited.insert(x as usize) {
                if let Ok((hidden, _, _)) = section(buf, x as usize) {
                    for (num, e) in hidden {
                        match map.get(&num) {
                            None | Some(XrefEntry::Free) => {
                                map.insert(num, e);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        for (num, e) in map {
            entries.entry(num).or_insert(e);
        }
        if entries.len() > MAX_OBJECTS {
            return Err(Error::TooLarge);
        }
        if first {
            kind = if is_stream { XrefKind::Stream } else { XrefKind::Table };
        }
        // The newest trailer wins; older ones fill in keys it lacks.
        for (k, v) in tr.iter() {
            if !trailer.contains(k) && k != b"Prev" && k != b"XRefStm" {
                trailer.insert(k.to_vec(), v.clone());
            }
        }
        next = tr.int(b"Prev").filter(|&p| p >= 0 && (p as u64) < buf.len() as u64).map(|p| p as usize);
        first = false;
    }
    Ok(Loaded { entries, trailer, kind })
}

type Section = (BTreeMap<u32, XrefEntry>, Dict, bool);

/// One section at `off`: a classic table and its trailer, or an xref stream and its dictionary.
fn section(buf: &[u8], off: usize) -> Result<Section, Error> {
    if off >= buf.len() {
        return Err(Error::Syntax("xref offset past the end", off));
    }
    let mut lx = Lexer::at(buf, off);
    lx.skip_ws();
    if buf[lx.pos..].starts_with(b"xref") {
        lx.pos += 4;
        let (map, tr) = table(&mut lx)?;
        return Ok((map, tr, false));
    }
    let (_, obj) = parse_indirect(buf, lx.pos, &mut |_| None)?;
    let Object::Stream(s) = obj else { return Err(Error::Syntax("startxref names neither xref nor a stream", off)) };
    if s.dict.name(b"Type") != Some(&b"XRef"[..]) {
        return Err(Error::Syntax("xref stream without /Type /XRef", off));
    }
    let map = stream_entries(&s)?;
    Ok((map, s.dict, true))
}

/// §7.5.4: subsections `start count`, then `count` entries `offset gen n|f`. Read as tokens, so 19- and 21-byte
/// rows (a wrong end-of-line) read like 20-byte ones.
fn table(lx: &mut Lexer<'_>) -> Result<(BTreeMap<u32, XrefEntry>, Dict), Error> {
    let mut map = BTreeMap::new();
    loop {
        let at = lx.pos;
        match lx.next() {
            Some(Token::Keyword(b"trailer")) => break,
            Some(Token::Int(start)) => {
                let count = match lx.next() {
                    Some(Token::Int(c)) if c >= 0 => c,
                    _ => return Err(Error::Syntax("xref subsection count", at)),
                };
                if start < 0 || count as usize > MAX_OBJECTS {
                    return Err(Error::TooLarge);
                }
                for i in 0..count {
                    let row = lx.pos;
                    let (Some(Token::Int(o)), Some(Token::Int(g)), Some(Token::Keyword(k))) =
                        (lx.next(), lx.next(), lx.next())
                    else {
                        return Err(Error::Syntax("xref row", row));
                    };
                    let num = start + i;
                    if num > u32::MAX as i64 {
                        break;
                    }
                    let e = match k {
                        b"n" if o > 0 => XrefEntry::InFile { offset: o as usize, generation: g.clamp(0, 65535) as u16 },
                        b"n" | b"f" => XrefEntry::Free,
                        _ => return Err(Error::Syntax("xref row type", row)),
                    };
                    map.entry(num as u32).or_insert(e);
                }
            }
            _ => return Err(Error::Syntax("xref table", at)),
        }
    }
    let tr = parse_object(lx, 0)?;
    let Object::Dict(tr) = tr else { return Err(Error::Syntax("trailer is not a dictionary", lx.pos)) };
    Ok((map, tr))
}

/// §7.5.8.2-3: the rows of an xref stream (/W field widths, /Index subsections).
fn stream_entries(s: &crate::object::Stream) -> Result<BTreeMap<u32, XrefEntry>, Error> {
    let data = filters::decode(s, &|o| o.clone())?.data;
    let w: Vec<usize> = s
        .dict
        .get(b"W")
        .and_then(Object::as_array)
        .map(|a| a.iter().map(|o| o.as_int().unwrap_or(-1)).filter(|&v| (0..=8).contains(&v)).map(|v| v as usize).collect())
        .unwrap_or_default();
    if w.len() != 3 || w.iter().sum::<usize>() == 0 {
        return Err(Error::Syntax("xref stream /W", 0));
    }
    let size = s.dict.int(b"Size").unwrap_or(0).max(0);
    let index: Vec<i64> = match s.dict.get(b"Index").and_then(Object::as_array) {
        Some(a) => a.iter().filter_map(Object::as_int).collect(),
        None => alloc::vec![0, size],
    };
    let width = w[0] + w[1] + w[2];
    let field = |row: &[u8], from: usize, n: usize| row[from..from + n].iter().fold(0u64, |acc, &b| acc << 8 | b as u64);
    let mut map = BTreeMap::new();
    let mut pos = 0usize;
    for pair in index.chunks(2) {
        let (start, count) = (pair[0], *pair.get(1).unwrap_or(&0));
        if start < 0 || count < 0 || count as usize > MAX_OBJECTS {
            return Err(Error::TooLarge);
        }
        for i in 0..count {
            if pos + width > data.len() {
                return Ok(map);
            }
            let row = &data[pos..pos + width];
            pos += width;
            let t = if w[0] == 0 { 1 } else { field(row, 0, w[0]) };
            let f2 = field(row, w[0], w[1]);
            let f3 = field(row, w[0] + w[1], w[2]);
            let num = start + i;
            if num > u32::MAX as i64 {
                break;
            }
            let e = match t {
                0 => XrefEntry::Free,
                1 => XrefEntry::InFile { offset: f2 as usize, generation: f3.min(65535) as u16 },
                2 => XrefEntry::InStream { stm: f2.min(u32::MAX as u64) as u32, idx: f3.min(u32::MAX as u64) as u32 },
                // §7.5.8.3: an unknown type is a reference to the null object.
                _ => continue,
            };
            map.entry(num as u32).or_insert(e);
        }
    }
    Ok(map)
}

/// The reconstruction scan: every `N G obj` in the file (a later one wins, as an update would), and every
/// `trailer` dictionary merged (a later key wins). Object streams and a trailer-less file's catalog are found by
/// the document, which can resolve objects.
pub fn reconstruct(buf: &[u8]) -> (BTreeMap<u32, XrefEntry>, Dict) {
    let mut entries = BTreeMap::new();
    let mut i = 0;
    while let Some(p) = find(buf, b"obj", i) {
        i = p + 3;
        let after_ok = buf.get(p + 3).is_none_or(|&b| is_white(b) || is_delim(b));
        if !after_ok || p == 0 || !is_white(buf[p - 1]) {
            continue;
        }
        let mut j = p;
        while j > 0 && is_white(buf[j - 1]) {
            j -= 1;
        }
        let ge = j;
        while j > 0 && buf[j - 1].is_ascii_digit() {
            j -= 1;
        }
        let gs = j;
        let mut k = j;
        while k > 0 && is_white(buf[k - 1]) {
            k -= 1;
        }
        let ne = k;
        while k > 0 && buf[k - 1].is_ascii_digit() {
            k -= 1;
        }
        let ns = k;
        if gs == ge || ne == gs || ns == ne || ge - gs > 5 || ne - ns > 10 {
            continue;
        }
        if ns > 0 && !(is_white(buf[ns - 1]) || is_delim(buf[ns - 1])) {
            continue;
        }
        let num = buf[ns..ne].iter().fold(0u64, |a, &d| a * 10 + (d - b'0') as u64);
        let generation = buf[gs..ge].iter().fold(0u64, |a, &d| a * 10 + (d - b'0') as u64);
        if num > u32::MAX as u64 || generation > 65535 {
            continue;
        }
        entries.insert(num as u32, XrefEntry::InFile { offset: ns, generation: generation as u16 });
        if entries.len() > MAX_OBJECTS {
            break;
        }
    }
    let mut trailer = Dict::new();
    let mut t = 0;
    while let Some(p) = find(buf, b"trailer", t) {
        t = p + 7;
        let mut lx = Lexer::at(buf, t);
        if let Ok(Object::Dict(d)) = parse_object(&mut lx, 0) {
            for (k, v) in d.0 {
                trailer.insert(k, v);
            }
        }
    }
    trailer.0.retain(|(k, _)| k.as_slice() != b"Prev" && k.as_slice() != b"XRefStm");
    (entries, trailer)
}
