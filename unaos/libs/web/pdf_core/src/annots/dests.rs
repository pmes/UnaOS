// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Destinations (ISO 32000-1 §12.3.2): explicit arrays, the catalog's /Dests dictionary (PDF 1.1) and the /Names /Dests
//! name tree (§7.9.6), a `<< /D [...] >>` dictionary unwrapped.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::MAX_DEPTH;
use crate::doc::Document;
use crate::object::{Object, Ref};

/// How the destination page is shown (§12.3.2.2, Table 151); a `null` coordinate keeps the viewer's current one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Xyz { left: Option<f64>, top: Option<f64>, zoom: Option<f64> },
    Fit,
    FitH(Option<f64>),
    FitV(Option<f64>),
    /// left, bottom, right, top.
    FitR([f64; 4]),
    FitB,
    FitBH(Option<f64>),
    FitBV(Option<f64>),
}

/// A destination in this document: the 0-based page index and the view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dest {
    pub page: usize,
    pub view: View,
}

/// A destination as written (an array, a name, a string, or a dictionary with /D) resolved against the page list.
pub fn resolve(doc: &Document, o: &Object, page_refs: &[Option<Ref>]) -> Option<Dest> {
    let mut cur = doc.get(o);
    // A name or string -> its entry (an array or a << /D >> dictionary) -> the array.
    for _ in 0..4 {
        match cur {
            Object::Array(a) => return explicit(doc, &a, page_refs),
            Object::Name(n) => cur = named(doc, &n)?,
            Object::String(s) => cur = named(doc, &s)?,
            Object::Dict(d) => cur = doc.get(d.get(b"D")?),
            _ => return None,
        }
    }
    None
}

/// An explicit destination `[page /Kind args...]`: the page as a reference to a page object (an integer is taken as
/// a page index, as some writers do).
pub fn explicit(doc: &Document, a: &[Object], page_refs: &[Option<Ref>]) -> Option<Dest> {
    let page = match a.first()? {
        Object::Ref(r) => page_refs.iter().position(|p| *p == Some(*r))?,
        o => usize::try_from(doc.get(o).as_int()?).ok().filter(|&i| i < page_refs.len())?,
    };
    let num = |i: usize| a.get(i).map(|o| doc.get(o)).and_then(|o| o.as_f64());
    let kind = a.get(1).map(|o| doc.get(o)).unwrap_or(Object::Null);
    let view = match kind.as_name().unwrap_or(&b"Fit"[..]) {
        b"XYZ" => View::Xyz { left: num(2), top: num(3), zoom: num(4).filter(|z| *z != 0.0) },
        b"FitH" => View::FitH(num(2)),
        b"FitV" => View::FitV(num(2)),
        b"FitR" => View::FitR([num(2)?, num(3)?, num(4)?, num(5)?]),
        b"FitB" => View::FitB,
        b"FitBH" => View::FitBH(num(2)),
        b"FitBV" => View::FitBV(num(2)),
        _ => View::Fit,
    };
    Some(Dest { page, view })
}

/// A named destination's entry (direct): the catalog's /Dests dictionary by name, then the /Names /Dests name tree.
pub fn named(doc: &Document, name: &[u8]) -> Option<Object> {
    let cat = doc.catalog()?;
    if let Object::Dict(d) = doc.get(cat.get(b"Dests").unwrap_or(&Object::Null)) {
        if let Some(o) = d.get(name) {
            let o = doc.get(o);
            if !o.is_null() {
                return Some(o);
            }
        }
    }
    let names = doc.get(cat.get(b"Names")?);
    let tree = doc.get(names.as_dict()?.get(b"Dests")?);
    let mut seen = BTreeSet::new();
    name_tree(doc, &tree, name, 0, &mut seen)
}

/// Look `key` up in a name tree node: its /Names pairs, else its /Kids whose /Limits admit it (a kid reached twice is
/// skipped).
fn name_tree(doc: &Document, node: &Object, key: &[u8], depth: usize, seen: &mut BTreeSet<Ref>) -> Option<Object> {
    if depth > MAX_DEPTH {
        return None;
    }
    let d = node.as_dict()?;
    let limits = doc.get(d.get(b"Limits").unwrap_or(&Object::Null));
    if let Some([lo, hi]) = limits.as_array() {
        let (lo, hi) = (doc.get(lo), doc.get(hi));
        if let (Some(lo), Some(hi)) = (lo.as_str(), hi.as_str()) {
            if key < lo || key > hi {
                return None;
            }
        }
    }
    let pairs = doc.get(d.get(b"Names").unwrap_or(&Object::Null));
    for pair in pairs.as_array().unwrap_or(&[]).chunks(2) {
        if pair.len() == 2 && doc.get(&pair[0]).as_str() == Some(key) {
            return Some(doc.get(&pair[1]));
        }
    }
    let kids = doc.get(d.get(b"Kids").unwrap_or(&Object::Null));
    for k in kids.as_array().unwrap_or(&[]) {
        if let Some(r) = k.as_ref() {
            if !seen.insert(r) {
                continue;
            }
        }
        if let Some(o) = name_tree(doc, &doc.get(k), key, depth + 1, seen) {
            return Some(o);
        }
    }
    None
}

impl Document {
    /// A named destination resolved to its page and view.
    pub fn dest(&self, name: &[u8]) -> Option<Dest> {
        let refs: Vec<Option<Ref>> = self.pages().ok()?.iter().map(|p| p.obj).collect();
        resolve(self, &Object::String(name.to_vec()), &refs)
    }
}
