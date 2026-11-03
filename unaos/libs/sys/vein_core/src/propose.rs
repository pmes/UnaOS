// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! VUGVEINASK (LEDGER SR504; docs/dev/USERLAND/VUG.md §7, BP-vug-7: AI proposes, geometry disposes) — the ONE
//! structured exchange Vug makes with a model: `propose`. The request is a fixed system prompt ([`SYSTEM`]), the
//! design document's text, a measurement table and the photos as image blocks, encoded for either dialect
//! ([`crate::claude::encode_image_body`]: Claude Messages base64 image sources before the text block;
//! [`crate::gemini::encode_image_body`]: Gemini `inlineData` parts and `responseMimeType: application/json`). It
//! streams through the SAME [`crate::client::exchange_dialect`] a chat turn does; the concatenated answer text goes
//! to [`parse`], which takes ONLY a JSON array of `{"stmt", "confidence", "why"}` objects and refuses any deviation
//! by name ([`Refusal`]). Vein never writes a coordinate: a proposal is a statement in the vug-doc grammar, checked
//! here by its SURFACE only (one line, the first word a verb shape `[a-z][a-z0-9_-]*`, doc_core's `valid_verb`,
//! cited not linked: a sys core does not depend on a cad core); doc_core reads, replays and verifies it later
//! (VUG-INTENT SR505). Offline the answer is [`ECHO_ANSWER`] — no proposals, never a fake one.

use crate::Out;
use crate::claude::{self, Dialect, Params};
use crate::gemini;
#[cfg(feature = "alloc")]
use crate::json;

/// The fixed Vug system prompt (a caller cannot swap the rules). It carries no backslash and no control character.
pub const SYSTEM: &str = "You are the design-intent reader of Vug, a CAD program. You are given photos of a part, its design document in the vug-doc text grammar, and a table of measurements (fitted outline primitives with their sigma, or scan shapes with their support and rms). Propose design INTENT only: a round dimension the measurement was evidently made at, a standard size, a constraint (parallel, perpendicular, equal, concentric, symmetric, tangent), a feature name. Every proposal is exactly ONE statement in the vug-doc grammar, on one line: <verb> <id> key=value ... (for example: parallel c8 sketch=s1 a=l1 b=l3). Never write a coordinate or a size the measurements did not give; never invent an entity the document does not name. Give each proposal a confidence from 0 to 1 and the reason in one sentence. Answer with a JSON array and NOTHING else, no prose and no code fence: [{\"stmt\":\"<one vug-doc statement>\",\"confidence\":0.9,\"why\":\"<one sentence>\"}]. If you have nothing to propose, answer [].";

/// Room for the answer (a list of one-line proposals).
pub const MAX_TOKENS: u32 = 4096;
/// The media types both dialects take (Claude: jpeg / png / gif / webp; Gemini: png / jpeg / webp / heic / heif).
pub const MEDIA_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];
/// Images per request (Claude lowers the pixel limit to 2000x2000 past 20 images).
pub const IMAGES_MAX: usize = 20;
/// Raw bytes per image: base64 5_000_000 bytes, inside Claude's 5 MB per image.
pub const IMAGE_BYTES_MAX: usize = 3_750_000;
/// Raw bytes of all images: base64 16 MB, under Gemini's 20 MB inline request with room for the text.
pub const IMAGES_BYTES_MAX: usize = 12_000_000;
/// The offline answer: no proposals (the Echo road still reaches every result by hand, BP-vug-7).
pub const ECHO_ANSWER: &str = "[]";

/// One photo, borrowed: its media type and its encoded bytes (PNG / JPEG / WebP; the caller scales it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRef<'a> {
    pub media_type: &'a str,
    pub bytes: &'a [u8],
}

/// One propose request, borrowed (the encode road needs no allocator).
#[derive(Debug, Clone, Copy)]
pub struct Propose<'a> {
    /// The design document (doc_core `write`).
    pub context: &'a str,
    /// The measurement table (outline primitives with sigma, or scan shapes with support / rms).
    pub measurements: &'a str,
    pub images: &'a [ImageRef<'a>],
}

/// Why a propose request was not encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskRefusal {
    /// Image `i` is not one of [`MEDIA_TYPES`].
    MediaType(usize),
    /// Image `i` has no bytes.
    EmptyImage(usize),
    /// Image `i` is over [`IMAGE_BYTES_MAX`].
    ImageTooLarge(usize),
    /// All images together are over [`IMAGES_BYTES_MAX`].
    ImagesTooLarge,
    /// More than [`IMAGES_MAX`] images.
    TooManyImages,
    /// The caller's buffer is too small.
    OutTooSmall,
}

impl AskRefusal {
    pub fn name(self) -> &'static str {
        match self {
            AskRefusal::MediaType(_) => "MediaType",
            AskRefusal::EmptyImage(_) => "EmptyImage",
            AskRefusal::ImageTooLarge(_) => "ImageTooLarge",
            AskRefusal::ImagesTooLarge => "ImagesTooLarge",
            AskRefusal::TooManyImages => "TooManyImages",
            AskRefusal::OutTooSmall => "OutTooSmall",
        }
    }
}

/// The images against the limits both dialects share.
pub fn check(p: &Propose<'_>) -> Result<(), AskRefusal> {
    if p.images.len() > IMAGES_MAX {
        return Err(AskRefusal::TooManyImages);
    }
    let mut total = 0usize;
    for (i, im) in p.images.iter().enumerate() {
        if !MEDIA_TYPES.contains(&im.media_type) {
            return Err(AskRefusal::MediaType(i));
        }
        if im.bytes.is_empty() {
            return Err(AskRefusal::EmptyImage(i));
        }
        if im.bytes.len() > IMAGE_BYTES_MAX {
            return Err(AskRefusal::ImageTooLarge(i));
        }
        total += im.bytes.len();
    }
    if total > IMAGES_BYTES_MAX {
        return Err(AskRefusal::ImagesTooLarge);
    }
    Ok(())
}

/// The user turn's text, in order (joined into ONE text block after the images).
pub fn texts<'a>(p: &Propose<'a>) -> [&'a str; 5] {
    ["Design document (vug-doc):\n", p.context, "\n\nMeasurements:\n", p.measurements, "\n"]
}

/// Encode the streaming propose request body for `dialect` into `out` (the head is the dialect's own
/// `request_head`; the Gemini path is [`gemini::path`]).
pub fn encode(p: &Propose<'_>, dialect: Dialect, model: &str, out: &mut [u8]) -> Result<usize, AskRefusal> {
    check(p)?;
    let params = Params::new(model, MAX_TOKENS, SYSTEM);
    let t = texts(p);
    let n = match dialect {
        Dialect::Claude => claude::encode_image_body(&params, p.images, &t, out),
        Dialect::Gemini => gemini::encode_image_body(&params, p.images, &t, true, out),
    };
    n.ok_or(AskRefusal::OutTooSmall)
}

/// The `alloc` convenience over [`encode`]: one allocation from an upper bound (every escaped byte at most 6).
#[cfg(feature = "alloc")]
pub fn request(p: &Propose<'_>, dialect: Dialect, model: &str) -> Result<alloc::vec::Vec<u8>, AskRefusal> {
    check(p)?;
    let text: usize = texts(p).iter().map(|t| t.len()).sum();
    let imgs: usize = p.images.iter().map(|im| 4 * im.bytes.len().div_ceil(3) + 6 * im.media_type.len() + 128).sum();
    let cap = 512 + 6 * (model.len() + SYSTEM.len() + text) + imgs;
    let mut v = alloc::vec![0u8; cap];
    let n = encode(p, dialect, model, &mut v)?;
    v.truncate(n);
    Ok(v)
}

/// RFC 4648 §4 base64 (with padding) of `data` into `o` (no allocation; the host's std encoder is gneiss_pal's).
pub fn base64_into(data: &[u8], o: &mut Out<'_>) {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        let q = [
            A[(n >> 18) as usize & 63],
            A[(n >> 12) as usize & 63],
            if c.len() > 1 { A[(n >> 6) as usize & 63] } else { b'=' },
            if c.len() > 2 { A[n as usize & 63] } else { b'=' },
        ];
        o.put(&q);
    }
}

/// The most proposals one answer may carry.
#[cfg(feature = "alloc")]
pub const PROPOSALS_MAX: usize = 32;

/// One proposal as the model answered it (the wire form; doc_core's `Proposal` is the document's, SR505).
#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// One vug-doc statement line (surface-checked here, validated by doc_core).
    pub stmt: alloc::string::String,
    /// 0..=1.
    pub confidence: f64,
    /// The reason, one line.
    pub why: alloc::string::String,
}

/// Why an answer was refused (an index names the array element).
#[cfg(feature = "alloc")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The answer is not a JSON array.
    NotAnArray,
    /// Text before or after the array (a sentence, a code fence).
    ProseAroundJson,
    /// The array or an object in it does not scan (a missing or doubled comma, an unclosed value).
    BadJson,
    NotAnObject(usize),
    /// A key other than stmt / confidence / why, or one of them twice.
    ExtraKey(usize),
    MissingKey(usize),
    /// Not a JSON number, or outside 0..=1.
    BadConfidence(usize),
    /// stmt or why is not a decodable JSON string.
    NotAString(usize),
    MultiLineStmt(usize),
    /// Empty, or the first word is not a vug-doc verb shape.
    BadStmt(usize),
    /// Empty or multi-line.
    BadWhy(usize),
    /// More than [`PROPOSALS_MAX`].
    TooMany,
}

#[cfg(feature = "alloc")]
impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Refusal::NotAnArray => "NotAnArray",
            Refusal::ProseAroundJson => "ProseAroundJson",
            Refusal::BadJson => "BadJson",
            Refusal::NotAnObject(_) => "NotAnObject",
            Refusal::ExtraKey(_) => "ExtraKey",
            Refusal::MissingKey(_) => "MissingKey",
            Refusal::BadConfidence(_) => "BadConfidence",
            Refusal::NotAString(_) => "NotAString",
            Refusal::MultiLineStmt(_) => "MultiLineStmt",
            Refusal::BadStmt(_) => "BadStmt",
            Refusal::BadWhy(_) => "BadWhy",
            Refusal::TooMany => "TooMany",
        }
    }
}

/// The answer text to its proposals, strictly: a JSON array (whitespace around it only) of objects with exactly
/// the keys `stmt`, `confidence`, `why`; every deviation is a named [`Refusal`].
#[cfg(feature = "alloc")]
pub fn parse(answer: &str) -> Result<alloc::vec::Vec<Proposal>, Refusal> {
    let t = answer.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\n'));
    let b = t.as_bytes();
    let Some(&first) = b.first() else { return Err(Refusal::NotAnArray) };
    if first != b'[' {
        return Err(if b.contains(&b'[') { Refusal::ProseAroundJson } else { Refusal::NotAnArray });
    }
    let end = json::value_end(b).ok_or(Refusal::BadJson)?;
    if b[end - 1] != b']' {
        return Err(Refusal::BadJson);
    }
    if end != b.len() {
        return Err(Refusal::ProseAroundJson);
    }
    let mut out = alloc::vec::Vec::new();
    let (mut pos, mut i) = (1usize, 0usize);
    while let Some(el) = json::index(b, i) {
        let s = off(b, el);
        if !gap(b, pos, s, i > 0) {
            return Err(Refusal::BadJson);
        }
        if i >= PROPOSALS_MAX {
            return Err(Refusal::TooMany);
        }
        out.push(one(el, i)?);
        pos = s + el.len();
        i += 1;
    }
    if !gap(b, pos, b.len() - 1, false) {
        return Err(Refusal::BadJson);
    }
    Ok(out)
}

/// The offset of the sub-slice `s` inside `base`.
#[cfg(feature = "alloc")]
fn off(base: &[u8], s: &[u8]) -> usize {
    s.as_ptr() as usize - base.as_ptr() as usize
}

/// `b[from..to]` is JSON whitespace around exactly one `,` when `comma`, none otherwise.
#[cfg(feature = "alloc")]
fn gap(b: &[u8], from: usize, to: usize, comma: bool) -> bool {
    if from > to {
        return false;
    }
    let mut commas = 0usize;
    for &c in &b[from..to] {
        if c == b',' {
            commas += 1;
        } else if !matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
            return false;
        }
    }
    commas == usize::from(comma)
}

/// Element `i` of the answer array.
#[cfg(feature = "alloc")]
fn one(el: &[u8], i: usize) -> Result<Proposal, Refusal> {
    if el.first() != Some(&b'{') {
        return Err(Refusal::NotAnObject(i));
    }
    if el.last() != Some(&b'}') {
        return Err(Refusal::BadJson);
    }
    let (mut stmt, mut conf, mut why): (Option<&[u8]>, Option<&[u8]>, Option<&[u8]>) = (None, None, None);
    let (mut pos, mut k) = (1usize, 0usize);
    while let Some((key, val)) = json::entry(el, k) {
        let ks = off(el, key) - 1; // the key's opening quote
        if !gap(el, pos, ks, k > 0) {
            return Err(Refusal::BadJson);
        }
        let slot = match key {
            b"stmt" => &mut stmt,
            b"confidence" => &mut conf,
            b"why" => &mut why,
            _ => return Err(Refusal::ExtraKey(i)),
        };
        if slot.is_some() {
            return Err(Refusal::ExtraKey(i));
        }
        *slot = Some(val);
        pos = off(el, val) + val.len();
        k += 1;
    }
    if !gap(el, pos, el.len() - 1, false) {
        return Err(Refusal::BadJson);
    }
    let (Some(s), Some(c), Some(w)) = (stmt, conf, why) else { return Err(Refusal::MissingKey(i)) };
    let confidence = json::number(c).filter(|v| (0.0..=1.0).contains(v)).ok_or(Refusal::BadConfidence(i))?;
    let stmt = string(s).ok_or(Refusal::NotAString(i))?;
    if stmt.contains(|ch: char| ch == '\n' || ch == '\r') {
        return Err(Refusal::MultiLineStmt(i));
    }
    let verb = stmt.split(|ch: char| ch == ' ' || ch == '\t').next().unwrap_or("");
    if !verb_shape(verb) {
        return Err(Refusal::BadStmt(i));
    }
    let why = string(w).ok_or(Refusal::NotAString(i))?;
    if why.trim().is_empty() || why.contains(|ch: char| ch == '\n' || ch == '\r') {
        return Err(Refusal::BadWhy(i));
    }
    Ok(Proposal { stmt, confidence, why })
}

/// A raw JSON string value, decoded.
#[cfg(feature = "alloc")]
fn string(raw: &[u8]) -> Option<alloc::string::String> {
    if raw.first() != Some(&b'"') {
        return None;
    }
    let mut v = raw.to_vec();
    let n = v.len();
    let r = json::unescape_in_place(&mut v, 0, n)?;
    alloc::string::String::from_utf8(v[r].to_vec()).ok()
}

/// The vug-doc verb shape (doc_core lex.rs `valid_verb`, cited): `[a-z][a-z0-9_-]*`.
#[cfg(feature = "alloc")]
fn verb_shape(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}
