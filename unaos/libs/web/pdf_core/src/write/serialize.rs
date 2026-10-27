// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF-WRITE (SR325): the object serializer, the one file writer (classic xref) and [`Edit`] (full and incremental save).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use crate::doc::Document;
use crate::object::{Dict, Object, Ref};
use crate::xref::{self, XrefKind};
use crate::{Error, MAX_DEPTH, MAX_OBJECTS};

/// The deepest nesting written; anything deeper is written `null` (parsed objects are already bounded by [`MAX_DEPTH`]).
const MAX_WRITE_DEPTH: usize = 2 * MAX_DEPTH;

/// Write one direct object in PDF syntax (§7.3). A stream can only be an indirect object's body: met here, it is `null`.
pub fn write_object(out: &mut Vec<u8>, o: &Object) {
    write_depth(out, o, 0);
}

fn write_depth(out: &mut Vec<u8>, o: &Object, depth: usize) {
    if depth > MAX_WRITE_DEPTH {
        out.extend_from_slice(b"null");
        return;
    }
    match o {
        Object::Null | Object::Stream(_) => out.extend_from_slice(b"null"),
        Object::Bool(true) => out.extend_from_slice(b"true"),
        Object::Bool(false) => out.extend_from_slice(b"false"),
        Object::Int(i) => {
            let mut s = String::new();
            let _ = write!(s, "{i}");
            out.extend_from_slice(s.as_bytes());
        }
        Object::Real(r) => write_real(out, *r),
        Object::String(s) => write_string(out, s),
        Object::Name(n) => write_name(out, n),
        Object::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_depth(out, x, depth + 1);
            }
            out.push(b']');
        }
        Object::Dict(d) => write_dict(out, d, depth),
        Object::Ref(r) => {
            let mut s = String::new();
            let _ = write!(s, "{} {} R", r.num, r.generation);
            out.extend_from_slice(s.as_bytes());
        }
    }
}

fn write_dict(out: &mut Vec<u8>, d: &Dict, depth: usize) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        write_name(out, k);
        out.push(b' ');
        write_depth(out, v, depth + 1);
        out.push(b' ');
    }
    out.extend_from_slice(b">>");
}

/// A real as the shortest decimal that reads back to the same f64 (Rust's `{}` never uses an exponent, which PDF
/// forbids); an integral value keeps a `.0` so it stays a real; a non-finite value is 0.
fn write_real(out: &mut Vec<u8>, v: f64) {
    if !v.is_finite() {
        out.push(b'0');
        return;
    }
    let mut s = String::new();
    let _ = write!(s, "{v}");
    if s == "-0" {
        s = String::from("0");
    }
    if !s.contains('.') {
        s.push_str(".0");
    }
    out.extend_from_slice(s.as_bytes());
}

/// A content-stream number: an integer when the value is integral, else the shortest decimal.
pub(crate) fn write_num(out: &mut Vec<u8>, v: f64) {
    if !v.is_finite() {
        out.push(b'0');
        return;
    }
    let mut s = String::new();
    if v > -1e15 && v < 1e15 && v == (v as i64) as f64 {
        let _ = write!(s, "{}", v as i64);
    } else {
        let _ = write!(s, "{v}");
    }
    out.extend_from_slice(s.as_bytes());
}

/// A string: hex when more than a quarter of its bytes are outside printable ASCII, else literal with `\`, `(`, `)`
/// and CR escaped (a bare CR would read back as LF, §7.3.4.2).
fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    let odd = s.iter().filter(|&&b| !(0x20..=0x7E).contains(&b)).count();
    if odd * 4 > s.len() {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        out.push(b'<');
        for &b in s {
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 15) as usize]);
        }
        out.push(b'>');
        return;
    }
    out.push(b'(');
    for &b in s {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'(' => out.extend_from_slice(b"\\("),
            b')' => out.extend_from_slice(b"\\)"),
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(b),
        }
    }
    out.push(b')');
}

/// A name: `/` then its bytes, `#xx` for whitespace, delimiters, `#` and anything outside 0x21..=0x7E (§7.3.5).
fn write_name(out: &mut Vec<u8>, n: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'/');
    for &b in n {
        if !(0x21..=0x7E).contains(&b) || b"#()<>[]{}/%".contains(&b) {
            out.push(b'#');
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 15) as usize]);
        } else {
            out.push(b);
        }
    }
}

/// `num gen obj ... endobj`; a stream's /Length is its data's length.
fn write_indirect(out: &mut Vec<u8>, r: Ref, o: &Object) {
    let mut head = String::new();
    let _ = write!(head, "{} {} obj\n", r.num, r.generation);
    out.extend_from_slice(head.as_bytes());
    match o {
        Object::Stream(s) => {
            let mut d = s.dict.clone();
            d.insert(b"Length".to_vec(), Object::Int(s.data.len() as i64));
            write_dict(out, &d, 0);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&s.data);
            out.extend_from_slice(b"\nendstream");
        }
        other => write_object(out, other),
    }
    out.extend_from_slice(b"\nendobj\n");
}

/// One classic-xref row (20 bytes, §7.5.4).
fn xref_row(out: &mut Vec<u8>, offset: usize, generation: u16, kind: char) {
    let mut s = String::new();
    let _ = write!(s, "{offset:010} {generation:05} {kind}\r\n");
    out.extend_from_slice(s.as_bytes());
}

fn trailer_tail(out: &mut Vec<u8>, trailer: &Dict, xref_at: usize) {
    out.extend_from_slice(b"trailer\n");
    write_dict(out, trailer, 0);
    let mut s = String::new();
    let _ = write!(s, "\nstartxref\n{xref_at}\n%%EOF\n");
    out.extend_from_slice(s.as_bytes());
}

/// THE file writer (save and the Builder): header, `objects` in order, one classic table `0 Size` (entry 0 and every
/// number not written free), `trailer` with /Size, `startxref`, `%%EOF`. Object 0 is never written.
pub(crate) fn write_file(version: &str, objects: &[(Ref, Object)], trailer: &Dict) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-");
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(b"\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
    for (r, o) in objects {
        if r.num == 0 || offsets.contains_key(&r.num) {
            continue;
        }
        offsets.insert(r.num, (out.len(), r.generation));
        write_indirect(&mut out, *r, o);
    }
    let size = offsets.keys().next_back().map_or(1, |&n| n + 1);
    let xref_at = out.len();
    let mut s = String::new();
    let _ = write!(s, "xref\n0 {size}\n");
    out.extend_from_slice(s.as_bytes());
    for n in 0..size {
        match offsets.get(&n) {
            _ if n == 0 => xref_row(&mut out, 0, 65535, 'f'),
            Some(&(off, g)) => xref_row(&mut out, off, g, 'n'),
            None => xref_row(&mut out, 0, 0, 'f'),
        }
    }
    let mut t = trailer.clone();
    t.insert(b"Size".to_vec(), Object::Int(size as i64));
    trailer_tail(&mut out, &t, xref_at);
    out
}

/// A text string (§7.9.2.2): the bytes themselves when every char is printable ASCII, else UTF-16BE after `FE FF`.
pub(crate) fn text_bytes(text: &str) -> Vec<u8> {
    if text.chars().all(|c| (' '..='~').contains(&c) || c == '\t' || c == '\n' || c == '\r') {
        return text.as_bytes().to_vec();
    }
    let mut out = alloc::vec![0xFE, 0xFF];
    for u in text.encode_utf16() {
        out.extend_from_slice(&u.to_be_bytes());
    }
    out
}

fn remove_key(d: &mut Dict, key: &[u8]) {
    d.0.retain(|(k, _)| k.as_slice() != key);
}

/// A stream as saved: the security handler already decrypted it, so a /Crypt filter (and its /DecodeParms slot) is
/// dropped; /Length becomes direct (an indirect /Length's object is then no longer reachable).
fn normalise(o: Object) -> Object {
    let Object::Stream(mut s) = o else { return o };
    let is_crypt = |o: &Object| o.as_name() == Some(&b"Crypt"[..]);
    match s.dict.get(b"Filter").cloned() {
        Some(f @ Object::Name(_)) if is_crypt(&f) => {
            remove_key(&mut s.dict, b"Filter");
            remove_key(&mut s.dict, b"DecodeParms");
        }
        Some(Object::Array(a)) if a.iter().any(is_crypt) => {
            let parms = s.dict.get(b"DecodeParms").cloned();
            let keep: Vec<usize> = (0..a.len()).filter(|&i| !is_crypt(&a[i])).collect();
            let filters: Vec<Object> = keep.iter().map(|&i| a[i].clone()).collect();
            let new_parms: Option<Object> = match parms {
                Some(Object::Array(p)) => {
                    let np: Vec<Object> = keep.iter().map(|&i| p.get(i).cloned().unwrap_or(Object::Null)).collect();
                    if np.iter().all(Object::is_null) { None } else { Some(Object::Array(np)) }
                }
                // A single parameter dictionary belongs to the first filter.
                Some(d @ Object::Dict(_)) if keep.first() == Some(&0) => Some(Object::Array(alloc::vec![d])),
                _ => None,
            };
            remove_key(&mut s.dict, b"DecodeParms");
            if filters.is_empty() {
                remove_key(&mut s.dict, b"Filter");
            } else {
                s.dict.insert(b"Filter".to_vec(), Object::Array(filters));
                if let Some(p) = new_parms {
                    s.dict.insert(b"DecodeParms".to_vec(), p);
                }
            }
        }
        _ => {}
    }
    s.dict.insert(b"Length".to_vec(), Object::Int(s.data.len() as i64));
    Object::Stream(s)
}

/// Every reference inside `o` (a stream's dictionary included).
fn refs_of(o: &Object, out: &mut Vec<Ref>, depth: usize) {
    if depth > MAX_WRITE_DEPTH {
        return;
    }
    match o {
        Object::Ref(r) => out.push(*r),
        Object::Array(a) => a.iter().for_each(|x| refs_of(x, out, depth + 1)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| refs_of(v, out, depth + 1)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| refs_of(v, out, depth + 1)),
        _ => {}
    }
}

/// Changes held over a borrowed [`Document`] (which is immutable): replaced and added objects, and the trailer the
/// saved file carries (/Root, /Info, /ID; never /Encrypt).
pub struct Edit<'a> {
    doc: &'a Document,
    changed: BTreeMap<u32, (u16, Object)>,
    trailer: Dict,
    next: Option<u32>,
}

impl<'a> Edit<'a> {
    pub fn new(doc: &'a Document) -> Self {
        let mut trailer = Dict::new();
        for (k, v) in doc.trailer().iter() {
            if matches!(k, b"Root" | b"Info" | b"ID") {
                trailer.insert(k.to_vec(), v.clone());
            }
        }
        Edit { doc, changed: BTreeMap::new(), trailer, next: None }
    }

    /// The trailer the saved file carries (/Size and /Prev are added at save).
    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    /// Object `r` as it will be saved: the changed one, else the document's (null when it does not resolve).
    pub fn get(&self, r: Ref) -> Object {
        match self.changed.get(&r.num) {
            Some((_, o)) => o.clone(),
            None => self.doc.resolve(r).unwrap_or(Object::Null),
        }
    }

    /// Replace object `r`.
    pub fn set(&mut self, r: Ref, obj: Object) {
        self.changed.insert(r.num, (r.generation, obj));
    }

    /// Add a new object under the next free number.
    pub fn add(&mut self, obj: Object) -> Ref {
        let base = match self.next {
            Some(n) => n,
            None => {
                let size = self.doc.trailer().int(b"Size").filter(|&s| s > 0 && s <= u32::MAX as i64).unwrap_or(1) as u32;
                let reach = self.collect().keys().next_back().map_or(1, |&n| n.saturating_add(1));
                size.max(reach)
            }
        };
        let over = self.changed.keys().next_back().map_or(1, |&n| n.saturating_add(1));
        let num = base.max(over);
        self.next = Some(num.saturating_add(1));
        let r = Ref { num, generation: 0 };
        self.set(r, obj);
        r
    }

    /// Set one /Info text entry (`Title`, `Author`, ...): the indirect /Info dictionary is replaced; a missing (or direct)
    /// one becomes a new indirect object the trailer names.
    pub fn set_info(&mut self, key: &[u8], text: &str) {
        let value = Object::String(text_bytes(text));
        match self.trailer.get(b"Info").cloned() {
            Some(Object::Ref(r)) => {
                let mut d = match self.get(r) {
                    Object::Dict(d) => d,
                    _ => Dict::new(),
                };
                d.insert(key.to_vec(), value);
                self.set(r, Object::Dict(d));
            }
            other => {
                let mut d = match other {
                    Some(Object::Dict(d)) => d,
                    _ => Dict::new(),
                };
                d.insert(key.to_vec(), value);
                let r = self.add(Object::Dict(d));
                self.trailer.insert(b"Info".to_vec(), Object::Ref(r));
            }
        }
    }

    /// Every object reachable from the trailer, as it will be written (streams normalised), by number.
    fn collect(&self) -> BTreeMap<u32, (Ref, Object)> {
        let mut out = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut work = Vec::new();
        for (_, v) in self.trailer.iter() {
            refs_of(v, &mut work, 0);
        }
        work.reverse();
        while let Some(r) = work.pop() {
            if r.num == 0 || !seen.insert(r.num) {
                continue;
            }
            if seen.len() > MAX_OBJECTS {
                break;
            }
            let generation = self.changed.get(&r.num).map_or(r.generation, |c| c.0);
            let obj = normalise(self.get(r));
            let mut found = Vec::new();
            refs_of(&obj, &mut found, 0);
            found.reverse();
            work.extend(found);
            out.insert(r.num, (Ref { num: r.num, generation }, obj));
        }
        out
    }

    /// The header version: the source's when it reads as 1.4 or later, else 1.4.
    fn version(&self) -> String {
        let v = self.doc.version();
        let b = v.as_bytes();
        let shaped = b.len() == 3 && b[0].is_ascii_digit() && b[1] == b'.' && b[2].is_ascii_digit();
        if shaped && v >= "1.4" { String::from(v) } else { String::from("1.4") }
    }

    /// The whole file: every reachable object (numbers and generations kept), one classic xref table, the trailer
    /// without /Encrypt (the document's objects are already decrypted: saving decrypts).
    pub fn save(&self) -> Vec<u8> {
        let objs: Vec<(Ref, Object)> = self.collect().into_values().collect();
        write_file(&self.version(), &objs, &self.trailer)
    }

    /// An incremental update (§7.5.6): `original` (the bytes this document was parsed from) followed by only the
    /// changed objects, a table for them and a trailer whose /Prev is the original's `startxref`. Refused for an
    /// encrypted document (the appended objects would have to be encrypted: owed) and for an original whose xref was
    /// rebuilt or has no `startxref` (save in full). No changes: the original unchanged.
    pub fn save_update(&self, original: &[u8]) -> Result<Vec<u8>, Error> {
        if self.doc.security().is_some() {
            return Err(Error::Unsupported("save_update of an encrypted file: re-encryption owed"));
        }
        if self.doc.xref_kind() == XrefKind::Rebuilt {
            return Err(Error::Unsupported("save_update of a rebuilt xref: save in full"));
        }
        let prev = xref::find_startxref(original).ok_or(Error::Unsupported("save_update: no startxref: save in full"))?;
        if self.changed.is_empty() {
            return Ok(original.to_vec());
        }
        let mut out = original.to_vec();
        if !matches!(out.last(), Some(b'\n' | b'\r')) {
            out.push(b'\n');
        }
        let mut offs: Vec<(u32, u16, usize)> = Vec::new();
        for (&num, (generation, o)) in &self.changed {
            if num == 0 {
                continue;
            }
            offs.push((num, *generation, out.len()));
            write_indirect(&mut out, Ref { num, generation: *generation }, &normalise(o.clone()));
        }
        let xref_at = out.len();
        out.extend_from_slice(b"xref\n");
        let mut i = 0;
        while i < offs.len() {
            let mut j = i + 1;
            while j < offs.len() && offs[j].0 == offs[j - 1].0 + 1 {
                j += 1;
            }
            let mut s = String::new();
            let _ = write!(s, "{} {}\n", offs[i].0, j - i);
            out.extend_from_slice(s.as_bytes());
            for &(_, g, off) in &offs[i..j] {
                xref_row(&mut out, off, g, 'n');
            }
            i = j;
        }
        let old = self.doc.trailer().int(b"Size").filter(|&s| s > 0).unwrap_or(0);
        let new = offs.last().map_or(0, |o| o.0 as i64 + 1);
        let mut t = self.trailer.clone();
        t.insert(b"Size".to_vec(), Object::Int(old.max(new)));
        t.insert(b"Prev".to_vec(), Object::Int(prev as i64));
        trailer_tail(&mut out, &t, xref_at);
        Ok(out)
    }
}

impl Document {
    /// PDF-WRITE (SR325): the whole document saved as a new file (`Edit::new(self).save()`): decrypted, classic xref.
    pub fn save(&self) -> Vec<u8> {
        Edit::new(self).save()
    }
}
