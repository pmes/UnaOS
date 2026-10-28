// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Appearance streams (ISO 32000-1 §12.5.5): where each one lands on the page, which ones a screen view paints, and
//! the GENERATED appearance of a widget that has none (§12.7.3.3), written as a form XObject content stream so PDF2's
//! interpreter paints it like any other (its text through the caller's `TextSink`; the check mark as a path).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use svg_core::geom::Transform;

use super::form::{FieldKind, inherited, kind_of};
use crate::content::ops;
use crate::doc::{Document, Page};
use crate::object::{Dict, Object, Stream, text_string};

/// §12.5.5: the matrix A that maps the appearance's /BBox, transformed by its /Matrix, onto the annotation's /Rect
/// (scale and translate). The full mapping is A x Matrix; `None` when either box is degenerate.
pub fn placement(rect: [f64; 4], bbox: [f64; 4], matrix: Transform) -> Option<Transform> {
    let pts = [(bbox[0], bbox[1]), (bbox[2], bbox[1]), (bbox[0], bbox[3]), (bbox[2], bbox[3])];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (x, y) in pts {
        let (tx, ty) = matrix.apply(x, y);
        x0 = x0.min(tx);
        y0 = y0.min(ty);
        x1 = x1.max(tx);
        y1 = y1.max(ty);
    }
    let (bw, bh) = (x1 - x0, y1 - y0);
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    if !(bw > 1e-9 && bh > 1e-9 && rw > 0.0 && rh > 0.0) {
        return None;
    }
    let (sx, sy) = (rw / bw, rh / bh);
    let a = Transform::new(sx, 0.0, 0.0, sy, rect[0] - x0 * sx, rect[1] - y0 * sy);
    a.is_finite().then_some(a)
}

/// A six-number /Matrix.
fn matrix_of(doc: &Document, o: Option<&Object>) -> Option<Transform> {
    let a = doc.get(o?);
    let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| doc.get(x).as_f64()).collect();
    (v.len() == 6).then(|| Transform::new(v[0], v[1], v[2], v[3], v[4], v[5]))
}

/// What a screen view paints after the page's content, in /Annots order: each visible annotation's normal appearance
/// (a widget of a field without one: the generated appearance) with its §12.5.5 placement A.
pub fn painted(page: &Page, doc: &Document) -> Vec<(Transform, Stream)> {
    let mut out = Vec::new();
    for a in page.annots_with(doc, &[]) {
        if !a.visible() {
            continue;
        }
        let s = match a.appearance {
            Some(s) => s,
            None if a.subtype == b"Widget" && a.field.is_some() => match generate(doc, &a.dict) {
                Some(s) => s,
                None => continue,
            },
            None => continue,
        };
        let Some(bbox) = doc.rect(s.dict.get(b"BBox")) else { continue };
        let m = matrix_of(doc, s.dict.get(b"Matrix")).unwrap_or(Transform::IDENTITY);
        if let Some(t) = placement(a.rect, bbox, m) {
            out.push((t, s));
        }
    }
    out
}

/// The default appearance string's parts (§12.7.3.3): the font resource name, the size (0 = auto) and the colour
/// operator with its operands.
struct Da {
    font: Vec<u8>,
    size: f64,
    colour: (Vec<f64>, Vec<u8>),
}

fn parse_da(b: &[u8]) -> Da {
    let mut da = Da { font: b"Helv".to_vec(), size: 0.0, colour: (alloc::vec![0.0], b"g".to_vec()) };
    for op in ops(b) {
        match op.operator.as_slice() {
            b"Tf" => {
                if let Some(n) = op.operands.first().and_then(Object::as_name) {
                    da.font = n.to_vec();
                }
                if let Some(s) = op.operands.get(1).and_then(Object::as_f64) {
                    da.size = s;
                }
            }
            b"g" | b"rg" | b"k" => da.colour = (op.operands.iter().filter_map(Object::as_f64).collect(), op.operator.clone()),
            _ => {}
        }
    }
    da
}

/// A number as content-stream text (three decimals, trailing zeros dropped).
fn n(v: f64) -> String {
    let mut s = format!("{v:.3}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" { String::from("0") } else { s }
}

/// The colour operator as text; `stroke` upper-cases it (g -> G, rg -> RG, k -> K).
fn colour_op(c: &(Vec<f64>, Vec<u8>), stroke: bool) -> String {
    let mut s: String = c.0.iter().map(|v| n(*v) + " ").collect();
    let op = String::from_utf8_lossy(&c.1).into_owned();
    s.push_str(&if stroke { op.to_uppercase() } else { op });
    s
}

/// The bytes a value shows: a text string's own bytes (a UTF-16 or UTF-8 one mapped to Latin-1, `?` past it); a
/// choice's first value; a name's bytes.
fn value_bytes(doc: &Document, v: &Object) -> Vec<u8> {
    match v {
        Object::String(s) if s.starts_with(&[0xFE, 0xFF]) || s.starts_with(&[0xEF, 0xBB, 0xBF]) => {
            text_string(s).chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect()
        }
        Object::String(s) => s.clone(),
        Object::Name(nm) => nm.clone(),
        Object::Array(a) => a.first().map(|o| value_bytes(doc, &doc.get(o))).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// A literal string's body: `\`, `(`, `)` escaped, CR and LF as `\r` and `\n`.
fn escape(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for &c in b {
        match c {
            b'\\' | b'(' | b')' => out.extend_from_slice(&[b'\\', c]),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\n' => out.extend_from_slice(b"\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// The /AcroForm /DR font named `name`, as written.
fn dr_font(doc: &Document, acro: &Dict, name: &[u8]) -> Option<Object> {
    let dr = doc.get(acro.get(b"DR")?);
    let fonts = doc.get(dr.as_dict()?.get(b"Font")?);
    Some(fonts.as_dict()?.get(name)?.clone())
}

/// The Helvetica fallback: a standard-14 Type 1 font dictionary.
fn helvetica() -> Object {
    let mut d = Dict::new();
    d.insert(b"Type".to_vec(), Object::Name(b"Font".to_vec()));
    d.insert(b"Subtype".to_vec(), Object::Name(b"Type1".to_vec()));
    d.insert(b"BaseFont".to_vec(), Object::Name(b"Helvetica".to_vec()));
    d.insert(b"Encoding".to_vec(), Object::Name(b"WinAnsiEncoding".to_vec()));
    Object::Dict(d)
}

/// The generated normal appearance of a widget without /AP (§12.7.3.3), from the widget's (inherited) /FT /Ff /V /DA
/// and /AS: a text or choice field's value on one line, left-aligned, in /DA's font (the /AcroForm /DR font of that
/// name, else Helvetica) and size (0 = auto: min(12, (h - 4) x 0.75)); a check box's check (ZapfDingbats a4 drawn as
/// a stroked path) or a radio button's dot when its state is not /Off, an empty stream when it is. `None` for other
/// kinds or an empty /Rect. /BBox [0 0 w h], so its placement is a translation onto /Rect.
pub fn generate(doc: &Document, w: &Dict) -> Option<Stream> {
    let r = doc.rect(w.get(b"Rect"))?;
    let (wd, ht) = (r[2] - r[0], r[3] - r[1]);
    if !(wd > 0.0 && ht > 0.0) {
        return None;
    }
    let ft = inherited(doc, w, b"FT");
    let ff = inherited(doc, w, b"Ff").and_then(|o| o.as_int()).unwrap_or(0) as u32;
    let kind = kind_of(ft.as_ref().and_then(Object::as_name), ff);
    let acro = doc.catalog().and_then(|c| c.get(b"AcroForm").map(|o| doc.get(o))).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let da_bytes = inherited(doc, w, b"DA")
        .or_else(|| acro.get(b"DA").map(|o| doc.get(o)))
        .and_then(|o| o.as_str().map(<[u8]>::to_vec))
        .unwrap_or_else(|| b"/Helv 0 Tf 0 g".to_vec());
    let da = parse_da(&da_bytes);
    let mut body: Vec<u8> = Vec::new();
    let mut resources = Dict::new();
    match kind {
        FieldKind::Text | FieldKind::Choice => {
            let value = inherited(doc, w, b"V").map(|v| value_bytes(doc, &v)).unwrap_or_default();
            let size = if da.size > 0.0 { da.size } else { (12.0f64).min((ht - 4.0) * 0.75).max(1.0) };
            let y = (ht - size) / 2.0 + 0.22 * size;
            body.extend_from_slice(
                format!(
                    "/Tx BMC\nq\n1 1 {} {} re W n\nBT\n{}\n/{} {} Tf\n2 {} Td\n(",
                    n(wd - 2.0),
                    n(ht - 2.0),
                    colour_op(&da.colour, false),
                    String::from_utf8_lossy(&da.font),
                    n(size),
                    n(y)
                )
                .as_bytes(),
            );
            body.extend_from_slice(&escape(&value));
            body.extend_from_slice(b") Tj\nET\nQ\nEMC\n");
            let mut fonts = Dict::new();
            fonts.insert(da.font.clone(), dr_font(doc, &acro, &da.font).unwrap_or_else(helvetica));
            resources.insert(b"Font".to_vec(), Object::Dict(fonts));
        }
        FieldKind::Checkbox | FieldKind::Radio => {
            let state = w
                .get(b"AS")
                .map(|o| doc.get(o))
                .or_else(|| inherited(doc, w, b"V"))
                .and_then(|o| o.as_name().map(<[u8]>::to_vec))
                .unwrap_or_else(|| b"Off".to_vec());
            if state.as_slice() != b"Off" {
                let m = wd.min(ht);
                if kind == FieldKind::Checkbox {
                    body.extend_from_slice(
                        format!(
                            "q\n{}\n{} w 1 J 1 j\n{} {} m\n{} {} l\n{} {} l\nS\nQ\n",
                            colour_op(&da.colour, true),
                            n(0.1 * m),
                            n(0.2 * wd),
                            n(0.5 * ht),
                            n(0.4 * wd),
                            n(0.25 * ht),
                            n(0.8 * wd),
                            n(0.8 * ht)
                        )
                        .as_bytes(),
                    );
                } else {
                    let (cx, cy, rr) = (wd / 2.0, ht / 2.0, 0.3 * m);
                    let k = 0.5523 * rr;
                    body.extend_from_slice(
                        format!(
                            "q\n{}\n{} {} m\n{} {} {} {} {} {} c\n{} {} {} {} {} {} c\n{} {} {} {} {} {} c\n{} {} {} {} {} {} c\nf\nQ\n",
                            colour_op(&da.colour, false),
                            n(cx + rr), n(cy),
                            n(cx + rr), n(cy + k), n(cx + k), n(cy + rr), n(cx), n(cy + rr),
                            n(cx - k), n(cy + rr), n(cx - rr), n(cy + k), n(cx - rr), n(cy),
                            n(cx - rr), n(cy - k), n(cx - k), n(cy - rr), n(cx), n(cy - rr),
                            n(cx + k), n(cy - rr), n(cx + rr), n(cy - k), n(cx + rr), n(cy)
                        )
                        .as_bytes(),
                    );
                }
            }
        }
        _ => return None,
    }
    let mut dict = Dict::new();
    dict.insert(b"Type".to_vec(), Object::Name(b"XObject".to_vec()));
    dict.insert(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()));
    dict.insert(
        b"BBox".to_vec(),
        Object::Array(alloc::vec![Object::Int(0), Object::Int(0), Object::Real(wd), Object::Real(ht)]),
    );
    dict.insert(b"Resources".to_vec(), Object::Dict(resources));
    dict.insert(b"Length".to_vec(), Object::Int(body.len() as i64));
    Some(Stream { dict, data: body })
}
