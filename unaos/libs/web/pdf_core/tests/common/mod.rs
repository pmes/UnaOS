// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The PDF test builder (PDF1, LEDGER SR300): one list of objects written as a classic-xref file, as an xref-stream
//! + object-stream file, or as a hybrid (/XRefStm); plus incremental updates. PDF2 / PDF3 reuse it.
#![allow(dead_code)]

pub mod mq;

use std::collections::BTreeMap;

/// One object's body.
#[derive(Clone)]
pub enum Body {
    /// Object text, e.g. `<< /Type /Catalog /Pages 2 0 R >>`.
    Plain(Vec<u8>),
    /// A stream: the dictionary entries other than /Length (e.g. `/Filter /FlateDecode`), and the stored bytes.
    Stream(Vec<u8>, Vec<u8>),
}

pub struct PdfBuilder {
    pub version: &'static str,
    pub objs: BTreeMap<u32, Body>,
    pub root: u32,
    pub info: Option<u32>,
    /// Extra trailer entries, e.g. `/Encrypt 9 0 R`.
    pub trailer_extra: String,
}

fn obj_bytes(num: u32, body: &Body) -> Vec<u8> {
    let mut out = format!("{num} 0 obj\n").into_bytes();
    match body {
        Body::Plain(t) => out.extend_from_slice(t),
        Body::Stream(d, data) => {
            out.extend_from_slice(b"<< ");
            out.extend_from_slice(d);
            out.extend_from_slice(format!(" /Length {} >>\nstream\n", data.len()).as_bytes());
            out.extend_from_slice(data);
            out.extend_from_slice(b"\nendstream");
        }
    }
    out.extend_from_slice(b"\nendobj\n");
    out
}

impl PdfBuilder {
    pub fn new(root: u32) -> Self {
        PdfBuilder { version: "1.4", objs: BTreeMap::new(), root, info: None, trailer_extra: String::new() }
    }

    pub fn obj(&mut self, num: u32, text: &str) -> &mut Self {
        self.objs.insert(num, Body::Plain(text.as_bytes().to_vec()));
        self
    }

    pub fn stream(&mut self, num: u32, dict: &str, data: &[u8]) -> &mut Self {
        self.objs.insert(num, Body::Stream(dict.as_bytes().to_vec(), data.to_vec()));
        self
    }

    fn max(&self) -> u32 {
        *self.objs.keys().max().unwrap_or(&0)
    }

    fn header(&self) -> Vec<u8> {
        let mut out = format!("%PDF-{}\n", self.version).into_bytes();
        out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");
        out
    }

    fn trailer_keys(&self, size: u32) -> String {
        let mut t = format!("/Size {size} /Root {} 0 R", self.root);
        if let Some(i) = self.info {
            t.push_str(&format!(" /Info {i} 0 R"));
        }
        if !self.trailer_extra.is_empty() {
            t.push(' ');
            t.push_str(&self.trailer_extra);
        }
        t
    }

    /// A classic file: every object top-level, one `xref` table, `trailer`, `startxref`.
    pub fn classic(&self) -> Vec<u8> {
        let mut out = self.header();
        let mut offs = BTreeMap::new();
        for (n, b) in &self.objs {
            offs.insert(*n, out.len());
            out.extend_from_slice(&obj_bytes(*n, b));
        }
        let size = self.max() + 1;
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for n in 1..size {
            match offs.get(&n) {
                Some(o) => out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes()),
                None => out.extend_from_slice(b"0000000000 00000 f \n"),
            }
        }
        out.extend_from_slice(format!("trailer\n<< {} >>\nstartxref\n{xref}\n%%EOF\n", self.trailer_keys(size)).as_bytes());
        out
    }

    /// Plain objects (all but `keep_top`) packed in one object stream; streams top-level.
    fn pack(&self, keep_top: &[u32]) -> (Vec<u8>, BTreeMap<u32, usize>, BTreeMap<u32, u32>, u32) {
        let stm = self.max() + 1;
        let mut out = self.header();
        let mut offs = BTreeMap::new();
        let mut packed = Vec::new();
        for (n, b) in &self.objs {
            match b {
                Body::Plain(t) if !keep_top.contains(n) => packed.push((*n, t.clone())),
                _ => {
                    offs.insert(*n, out.len());
                    out.extend_from_slice(&obj_bytes(*n, b));
                }
            }
        }
        let mut head = String::new();
        let mut body = Vec::new();
        let mut idx = BTreeMap::new();
        for (i, (n, t)) in packed.iter().enumerate() {
            head.push_str(&format!("{n} {} ", body.len()));
            body.extend_from_slice(t);
            body.push(b'\n');
            idx.insert(*n, i as u32);
        }
        let mut data = head.clone().into_bytes();
        data.extend_from_slice(&body);
        let dict = format!("/Type /ObjStm /N {} /First {} /Filter /FlateDecode", packed.len(), head.len());
        offs.insert(stm, out.len());
        out.extend_from_slice(&obj_bytes(stm, &Body::Stream(dict.into_bytes(), zlib_stored(&data))));
        (out, offs, idx, stm)
    }

    /// The rows (/W [1 4 2]) for objects 0..size, PNG-Up predicted and stored-zlib wrapped.
    fn xref_rows(size: u32, offs: &BTreeMap<u32, usize>, idx: &BTreeMap<u32, u32>, stm: u32) -> Vec<u8> {
        let mut rows = Vec::new();
        for n in 0..size {
            let (t, f2, f3): (u8, u32, u16) = if let Some(o) = offs.get(&n) {
                (1, *o as u32, 0)
            } else if let Some(i) = idx.get(&n) {
                (2, stm, *i as u16)
            } else {
                (0, 0, if n == 0 { 65535 } else { 0 })
            };
            let mut r = vec![t];
            r.extend_from_slice(&f2.to_be_bytes());
            r.extend_from_slice(&f3.to_be_bytes());
            rows.push(r);
        }
        let mut pred = Vec::new();
        let mut prev = vec![0u8; 7];
        for r in rows {
            pred.push(2);
            for i in 0..7 {
                pred.push(r[i].wrapping_sub(prev[i]));
            }
            prev = r;
        }
        zlib_stored(&pred)
    }

    /// An xref-stream file: every plain object in one object stream, streams top-level, one xref stream.
    pub fn compressed(&self) -> Vec<u8> {
        let (mut out, mut offs, idx, stm) = self.pack(&[]);
        let xnum = stm + 1;
        let size = xnum + 1;
        let xref = out.len();
        offs.insert(xnum, xref);
        let rows = Self::xref_rows(size, &offs, &idx, stm);
        let dict = format!(
            "/Type /XRef {} /W [1 4 2] /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 7 >>",
            self.trailer_keys(size)
        );
        out.extend_from_slice(&obj_bytes(xnum, &Body::Stream(dict.into_bytes(), rows)));
        out.extend_from_slice(format!("startxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    /// A hybrid file (§7.5.8.4): a classic table listing the top-level objects (packed ones marked free) whose
    /// trailer's /XRefStm names an xref stream listing the packed ones.
    pub fn hybrid(&self) -> Vec<u8> {
        let (mut out, offs, idx, stm) = self.pack(&[self.root]);
        let xnum = stm + 1;
        let size = xnum + 1;
        let xs = out.len();
        let rows = Self::xref_rows(size, &BTreeMap::new(), &idx, stm);
        let dict = format!(
            "/Type /XRef /Size {size} /W [1 4 2] /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 7 >>"
        );
        out.extend_from_slice(&obj_bytes(xnum, &Body::Stream(dict.into_bytes(), rows)));
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for n in 1..size {
            match offs.get(&n) {
                Some(o) => out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes()),
                None => out.extend_from_slice(b"0000000000 00000 f \n"),
            }
        }
        out.extend_from_slice(
            format!("trailer\n<< {} /XRefStm {xs} >>\nstartxref\n{xref}\n%%EOF\n", self.trailer_keys(size)).as_bytes(),
        );
        out
    }
}

/// An incremental update (§7.5.6): `objs` appended to `base`, a new table for them, a trailer with /Prev.
pub fn incremental(base: &[u8], objs: &[(u32, Body)], root: u32, size: u32) -> Vec<u8> {
    let prev = startxref_of(base);
    let mut out = base.to_vec();
    let mut offs = Vec::new();
    for (n, b) in objs {
        offs.push((*n, out.len()));
        out.extend_from_slice(&obj_bytes(*n, b));
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n");
    for (n, o) in &offs {
        out.extend_from_slice(format!("{n} 1\n{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root {root} 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// The offset the last `startxref` names.
pub fn startxref_of(pdf: &[u8]) -> usize {
    let s = String::from_utf8_lossy(pdf);
    let at = s.rfind("startxref").unwrap() + 9;
    s[at..].split_whitespace().next().unwrap().parse().unwrap()
}

/// Every occurrence of `from` in `hay` replaced by `to`, byte for byte (the builder's header comment is binary).
pub fn replace_bytes(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// A zlib stream (RFC 1950) of stored DEFLATE blocks.
pub fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(65535).collect() };
    for (i, c) in chunks.iter().enumerate() {
        out.push(if i + 1 == chunks.len() { 1 } else { 0 });
        let len = c.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    out.extend_from_slice(&(b << 16 | a).to_be_bytes());
    out
}

/// An LZW encoder (§7.4.4.2) matching the decoder's code-width rule for `early`.
pub fn lzw_encode(data: &[u8], early: bool) -> Vec<u8> {
    let e = early as u32;
    let width = |n: u32| if n >= 2048 { 12 } else if n >= 1024 { 11 } else if n >= 512 { 10 } else { 9 };
    let mut out = Vec::new();
    let (mut acc, mut nbits) = (0u64, 0u32);
    let mut put = |v: u32, w: u32, out: &mut Vec<u8>| {
        acc = (acc << w) | v as u64;
        nbits += w;
        while nbits >= 8 {
            out.push((acc >> (nbits - 8)) as u8);
            nbits -= 8;
        }
        acc &= (1u64 << nbits) - 1;
    };
    let mut dict: BTreeMap<(u32, u8), u32> = BTreeMap::new();
    let mut next = 258u32;
    // The decoder's next code when it reads the code being written; it grows after every code but the first.
    let mut dn = 258u32;
    let mut emitted = false;
    put(256, 9, &mut out);
    let mut cur: Option<u32> = None;
    let mut emit = |c: u32, out: &mut Vec<u8>, put: &mut dyn FnMut(u32, u32, &mut Vec<u8>)| {
        put(c, width(dn + e), out);
        if emitted {
            dn += 1;
        }
        emitted = true;
    };
    for &b in data {
        match cur {
            None => cur = Some(b as u32),
            Some(c) => match dict.get(&(c, b)) {
                Some(&k) => cur = Some(k),
                None => {
                    emit(c, &mut out, &mut put);
                    dict.insert((c, b), next);
                    next += 1;
                    cur = Some(b as u32);
                }
            },
        }
    }
    if let Some(c) = cur {
        emit(c, &mut out, &mut put);
    }
    let w = width(dn + e);
    put(257, w, &mut out);
    if nbits > 0 {
        out.push((acc << (8 - nbits)) as u8);
    }
    out
}

/// PDFFORMS (SR326): the builder's annotation + AcroForm helper. Catalog 1 (`catalog_extra` inside it, e.g.
/// `/AcroForm 30 0 R`), page tree 2, page `i` at 3 + 2i (40 x 40 pt, empty /Resources, `page_extra` inside it, e.g.
/// `/Annots [20 0 R]`) with its content stream at 4 + 2i; `extra` objects (number, dictionary text or stream
/// dictionary entries, stream bytes) numbered by the caller from 20.
pub fn annot_pdf(catalog_extra: &str, pages: &[(&str, &str)], extra: &[(u32, &str, Option<&[u8]>)]) -> Vec<u8> {
    let mut b = PdfBuilder::new(1);
    b.obj(1, &format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"));
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    b.obj(2, &format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()));
    for (i, (page_extra, content)) in pages.iter().enumerate() {
        let n = 3 + 2 * i as u32;
        b.obj(n, &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Resources << >> {page_extra} /Contents {} 0 R >>", n + 1));
        b.stream(n + 1, "", content.as_bytes());
    }
    for (n, d, data) in extra {
        match data {
            Some(bytes) => b.stream(*n, d, bytes),
            None => b.obj(*n, d),
        };
    }
    b.classic()
}
