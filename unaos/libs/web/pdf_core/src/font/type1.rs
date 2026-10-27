// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The PDF side of Type 1 programs (ISO 32000-1 §9.9): a `/FontFile` stream holds the cleartext portion
//! (`/Length1` bytes), the eexec-encrypted binary portion (`/Length2`) and the trailing zeros (`/Length3`). font_core's
//! reader finds the `eexec` boundary itself (and reads PFB / PFA too), so this glue only decodes the stream and keeps
//! the declared portions when the lengths are sane; the font program's own encoding is the base encoding of a
//! Type 1 font whose dictionary names none (§9.6.6.2).

use alloc::string::String;

use super::encoding::Encoding;
use crate::object::Stream;
use crate::Document;
use font_core::type1::Type1Font;

/// Read a `/FontFile` stream as a Type 1 program.
pub fn load(doc: &Document, s: &Stream) -> Option<Type1Font> {
    let data = doc.decode(s).ok()?.data;
    let l1 = s.dict.int(b"Length1").and_then(|v| usize::try_from(v).ok()).unwrap_or(0);
    let l2 = s.dict.int(b"Length2").and_then(|v| usize::try_from(v).ok()).unwrap_or(0);
    let end = l1.checked_add(l2).filter(|&e| l1 > 0 && l2 > 0 && e <= data.len()).unwrap_or(data.len());
    Type1Font::parse(&data[..end])
}

/// The program's built-in encoding as a code -> name table.
pub fn builtin_encoding(f: &Type1Font) -> Encoding {
    Encoding::from_fn(|c| f.encoding_name(c).map(String::from))
}
