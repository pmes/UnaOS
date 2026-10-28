// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The interactive form (ISO 32000-1 §12.7): the AcroForm's field tree, fully qualified names, inherited attributes,
//! and filling a field as object edits.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use super::FieldRef;
use super::appearance;
use crate::doc::Document;
use crate::object::{Dict, Object, Ref, text_string};
use crate::{Error, MAX_DEPTH};

/// The most fields read from one AcroForm.
pub const MAX_FIELDS: usize = 1 << 16;

/// Field flags (/Ff, §12.7.3.1, §12.7.4).
pub mod ff {
    pub const READ_ONLY: u32 = 1;
    pub const REQUIRED: u32 = 1 << 1;
    pub const NO_EXPORT: u32 = 1 << 2;
    pub const MULTILINE: u32 = 1 << 12;
    pub const PASSWORD: u32 = 1 << 13;
    pub const NO_TOGGLE_TO_OFF: u32 = 1 << 14;
    pub const RADIO: u32 = 1 << 15;
    pub const PUSHBUTTON: u32 = 1 << 16;
    pub const COMBO: u32 = 1 << 17;
    pub const COMB: u32 = 1 << 24;
    pub const RADIOS_IN_UNISON: u32 = 1 << 25;
}

/// The field type (/FT with the button flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Checkbox,
    Radio,
    Choice,
    Button,
    Signature,
    Unknown,
}

/// One terminal field.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The field dictionary's reference.
    pub obj: Option<Ref>,
    /// The fully qualified name (the /T chain joined by `.`).
    pub name: String,
    pub kind: FieldKind,
    /// /V (inherited), null when absent.
    pub value: Object,
    /// /Opt: each option's display text.
    pub options: Vec<String>,
    /// /Ff (inherited; [`ff`]).
    pub flags: u32,
    /// The first widget's /Rect.
    pub rect: Option<[f64; 4]>,
    /// The widget annotations (the field's own reference when field and widget are merged).
    pub widgets: Vec<Ref>,
    /// A button's on state: the first /AP /N state name other than /Off among its widgets.
    pub on_state: Option<Vec<u8>>,
}

/// The AcroForm.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Form {
    pub fields: Vec<Field>,
    /// /NeedAppearances.
    pub need_appearances: bool,
    /// What was met and not read, by name: "XFA" (the AcroForm fields are still read), "field loop".
    pub refused: Vec<&'static str>,
}

/// `key` from `d` or the nearest /Parent that has it (non-null), up to [`MAX_DEPTH`] levels: the ONE inheritance walk
/// (/FT /Ff /V /DV /DA /Q).
pub fn inherited(doc: &Document, d: &Dict, key: &[u8]) -> Option<Object> {
    let mut cur = d.clone();
    for _ in 0..MAX_DEPTH {
        if let Some(o) = cur.get(key) {
            let o = doc.get(o);
            if !o.is_null() {
                return Some(o);
            }
        }
        let parent = doc.get(cur.get(b"Parent")?);
        match parent {
            Object::Dict(p) => cur = p,
            _ => return None,
        }
    }
    None
}

/// The fully qualified field name: every /T from the root down, joined by `.`.
pub fn full_name(doc: &Document, d: &Dict) -> String {
    let mut parts = Vec::new();
    let mut cur = d.clone();
    for _ in 0..MAX_DEPTH {
        let t = cur.get(b"T").map(|o| doc.get(o));
        if let Some(s) = t.as_ref().and_then(Object::as_str) {
            parts.push(text_string(s));
        }
        let parent = cur.get(b"Parent").map(|o| doc.get(o));
        match parent {
            Some(Object::Dict(p)) => cur = p,
            _ => break,
        }
    }
    parts.reverse();
    parts.join(".")
}

/// The field type from /FT and /Ff.
pub fn kind_of(ft: Option<&[u8]>, flags: u32) -> FieldKind {
    match ft {
        Some(b"Tx") => FieldKind::Text,
        Some(b"Btn") if flags & ff::PUSHBUTTON != 0 => FieldKind::Button,
        Some(b"Btn") if flags & ff::RADIO != 0 => FieldKind::Radio,
        Some(b"Btn") => FieldKind::Checkbox,
        Some(b"Ch") => FieldKind::Choice,
        Some(b"Sig") => FieldKind::Signature,
        _ => FieldKind::Unknown,
    }
}

/// A widget's field: the widget itself when it carries /T or /FT (merged), else its /Parent.
pub(crate) fn field_of(doc: &Document, obj: Option<Ref>, w: &Dict) -> Option<FieldRef> {
    if w.contains(b"T") || w.contains(b"FT") {
        return Some(FieldRef { obj, name: full_name(doc, w) });
    }
    let p = w.get(b"Parent")?;
    let Object::Dict(pd) = doc.get(p) else { return None };
    Some(FieldRef { obj: p.as_ref(), name: full_name(doc, &pd) })
}

/// The state names of a widget's /AP /N dictionary.
fn states(doc: &Document, w: &Dict) -> Vec<Vec<u8>> {
    let Some(ap) = w.get(b"AP").map(|o| doc.get(o)) else { return Vec::new() };
    let n = ap.as_dict().and_then(|d| d.get(b"N")).map(|o| doc.get(o)).unwrap_or(Object::Null);
    match n {
        Object::Dict(s) => s.iter().map(|(k, _)| k.to_vec()).collect(),
        _ => Vec::new(),
    }
}

impl Document {
    /// The AcroForm (`None` when the catalog has none).
    pub fn form(&self) -> Option<Form> {
        let cat = self.catalog()?;
        let af = self.get(cat.get(b"AcroForm")?);
        let af = af.as_dict()?;
        let mut form = Form {
            need_appearances: matches!(af.get(b"NeedAppearances").map(|o| self.get(o)), Some(Object::Bool(true))),
            ..Form::default()
        };
        if af.get(b"XFA").is_some_and(|o| !self.get(o).is_null()) {
            form.refused.push("XFA");
        }
        let fields = self.get(af.get(b"Fields").unwrap_or(&Object::Null));
        let mut seen = BTreeSet::new();
        for f in fields.as_array().unwrap_or(&[]) {
            walk_field(self, f, 0, &mut seen, &mut form);
        }
        Some(form)
    }
}

/// One node of the field tree: kids with /T are fields (recursed), kids without are this field's widgets.
fn walk_field(doc: &Document, o: &Object, depth: usize, seen: &mut BTreeSet<Ref>, form: &mut Form) {
    if depth > MAX_DEPTH || form.fields.len() >= MAX_FIELDS {
        return;
    }
    if let Some(r) = o.as_ref() {
        if !seen.insert(r) {
            if !form.refused.contains(&"field loop") {
                form.refused.push("field loop");
            }
            return;
        }
    }
    let Object::Dict(d) = doc.get(o) else { return };
    let kids = doc.get(d.get(b"Kids").unwrap_or(&Object::Null));
    let mut child_fields = Vec::new();
    let mut widgets: Vec<(Option<Ref>, Dict)> = Vec::new();
    for k in kids.as_array().unwrap_or(&[]) {
        if let Object::Dict(kd) = doc.get(k) {
            if kd.contains(b"T") {
                child_fields.push(k.clone());
            } else {
                widgets.push((k.as_ref(), kd));
            }
        }
    }
    if !child_fields.is_empty() {
        for k in &child_fields {
            walk_field(doc, k, depth + 1, seen, form);
        }
        return;
    }
    if widgets.is_empty() && d.name(b"Subtype") == Some(&b"Widget"[..]) {
        widgets.push((o.as_ref(), d.clone()));
    }
    let ft = inherited(doc, &d, b"FT");
    let flags = inherited(doc, &d, b"Ff").and_then(|o| o.as_int()).unwrap_or(0) as u32;
    let kind = kind_of(ft.as_ref().and_then(Object::as_name), flags);
    let opt = doc.get(d.get(b"Opt").unwrap_or(&Object::Null));
    let options = opt
        .as_array()
        .unwrap_or(&[])
        .iter()
        .filter_map(|e| match doc.get(e) {
            Object::String(s) => Some(text_string(&s)),
            Object::Array(pair) => pair.get(1).map(|x| doc.get(x)).and_then(|x| x.as_str().map(text_string)),
            _ => None,
        })
        .collect();
    let on_state = if matches!(kind, FieldKind::Checkbox | FieldKind::Radio) {
        widgets.iter().flat_map(|(_, w)| states(doc, w)).find(|s| s.as_slice() != b"Off")
    } else {
        None
    };
    form.fields.push(Field {
        obj: o.as_ref(),
        name: full_name(doc, &d),
        kind,
        value: inherited(doc, &d, b"V").unwrap_or(Object::Null),
        options,
        flags,
        rect: widgets.first().and_then(|(_, w)| doc.rect(w.get(b"Rect"))),
        widgets: widgets.iter().filter_map(|(r, _)| *r).collect(),
        on_state,
    });
}

/// A text string's bytes: PDFDocEncoding when every character is one it shares with Latin-1, else UTF-16BE with a BOM.
fn encode_text(v: &str) -> Vec<u8> {
    let plain = |c: char| matches!(c as u32, 0x09 | 0x0A | 0x0D | 0x20..=0x7E | 0xA1..=0xAC | 0xAE..=0xFF);
    if v.chars().all(plain) {
        return v.chars().map(|c| c as u32 as u8).collect();
    }
    let mut out = alloc::vec![0xFE, 0xFF];
    for u in v.encode_utf16() {
        out.extend_from_slice(&u.to_be_bytes());
    }
    out
}

/// The edited copy of object `r` (resolved the first time).
fn slot<'e>(doc: &Document, edits: &'e mut BTreeMap<Ref, Dict>, r: Ref) -> Result<&'e mut Dict, Error> {
    if !edits.contains_key(&r) {
        let Object::Dict(d) = doc.resolve(r)? else { return Err(Error::Unsupported("field is not a dictionary")) };
        edits.insert(r, d);
    }
    edits.get_mut(&r).ok_or(Error::Missing)
}

impl Form {
    /// FILL (§12.7.4): set `field` to `value` and return the object edits an incremental update writes: each changed
    /// dictionary under its own reference, then the NEW appearance streams under new numbers from the trailer's /Size
    /// (else the object count + 1) upward. Text and choice fields: /V (a text string) and each widget's /AP /N, the
    /// regenerated appearance (`appearance::generate`). Check boxes and radio buttons: /V the state name (`Off` or
    /// empty = off) and each widget's /AS (its state when it has that state, else /Off), an appearance generated only
    /// for a widget without states. A merged field and widget get ONE edit. PDF-WRITE's `save_update` (not folded at
    /// this cut) is the writer that serializes the Vec.
    pub fn set_value(&self, doc: &Document, field: &Field, value: &str) -> Result<Vec<(Ref, Object)>, Error> {
        let fref = field.obj.ok_or(Error::Unsupported("direct field"))?;
        let mut edits: BTreeMap<Ref, Dict> = BTreeMap::new();
        let mut added: Vec<(Ref, Object)> = Vec::new();
        let size = doc.trailer().get(b"Size").map(|o| doc.get(o)).and_then(|o| o.as_int());
        let mut next = match size {
            Some(n) if n > 0 && n <= u32::MAX as i64 => n as u32,
            _ => doc.object_count() as u32 + 1,
        };
        let mut new_ap = |added: &mut Vec<(Ref, Object)>, wd: &Dict| -> Result<Object, Error> {
            let s = appearance::generate(doc, wd).ok_or(Error::Unsupported("widget appearance"))?;
            let r = Ref { num: next, generation: 0 };
            next += 1;
            added.push((r, Object::Stream(s)));
            let mut ap = Dict::new();
            ap.insert(b"N".to_vec(), Object::Ref(r));
            Ok(Object::Dict(ap))
        };
        match field.kind {
            FieldKind::Text | FieldKind::Choice => {
                let v = Object::String(encode_text(value));
                slot(doc, &mut edits, fref)?.insert(b"V".to_vec(), v.clone());
                for &w in &field.widgets {
                    // The appearance is generated from the widget with the NEW value (a separate widget reads /V
                    // through /Parent, which the document still holds as the old one).
                    let mut wd = slot(doc, &mut edits, w)?.clone();
                    wd.insert(b"V".to_vec(), v.clone());
                    let ap = new_ap(&mut added, &wd)?;
                    slot(doc, &mut edits, w)?.insert(b"AP".to_vec(), ap);
                }
            }
            FieldKind::Checkbox | FieldKind::Radio => {
                let on: Vec<u8> = if value.is_empty() { b"Off".to_vec() } else { value.as_bytes().to_vec() };
                slot(doc, &mut edits, fref)?.insert(b"V".to_vec(), Object::Name(on.clone()));
                for &w in &field.widgets {
                    let wd = slot(doc, &mut edits, w)?.clone();
                    let st = states(doc, &wd);
                    if st.is_empty() {
                        let mut g = wd;
                        g.insert(b"AS".to_vec(), Object::Name(on.clone()));
                        let ap = new_ap(&mut added, &g)?;
                        let e = slot(doc, &mut edits, w)?;
                        e.insert(b"AS".to_vec(), Object::Name(on.clone()));
                        e.insert(b"AP".to_vec(), ap);
                    } else {
                        let pick = if st.contains(&on) { on.clone() } else { b"Off".to_vec() };
                        slot(doc, &mut edits, w)?.insert(b"AS".to_vec(), Object::Name(pick));
                    }
                }
            }
            FieldKind::Signature => return Err(Error::Unsupported("signature field")),
            FieldKind::Button => return Err(Error::Unsupported("pushbutton value")),
            FieldKind::Unknown => return Err(Error::Unsupported("field type")),
        }
        Ok(edits.into_iter().map(|(r, d)| (r, Object::Dict(d))).chain(added).collect())
    }
}
