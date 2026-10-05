// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The OpenType `name` table (AETHERFONT, SR61): naming table formats 0 and 1, records decoded from
//! UTF-16BE (platform 0, platform 3 encodings 0/1/10) and Mac Roman (platform 1 encoding 0). A font is
//! found by its family names (name IDs 1 and 16, every language), full name (4) and PostScript name (6) —
//! what fontconfig indexes and what CSS `font-family` / `@font-face local()` match against.

use crate::reader::{slice, u16_at};
use crate::Font;
use alloc::string::String;
use alloc::vec::Vec;

/// Name IDs (OpenType `name`, "Name IDs").
pub const FAMILY: u16 = 1;
pub const SUBFAMILY: u16 = 2;
pub const FULL_NAME: u16 = 4;
pub const POSTSCRIPT: u16 = 6;
pub const TYPOGRAPHIC_FAMILY: u16 = 16;
pub const TYPOGRAPHIC_SUBFAMILY: u16 = 17;

/// One decoded name record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameRecord {
    pub platform: u16,
    pub encoding: u16,
    pub language: u16,
    pub name_id: u16,
    pub value: String,
}

/// Mac Roman 0x80..=0xFF (Apple's ROMAN.TXT).
const MAC_ROMAN_HIGH: [u16; 128] = [
    0x00C4, 0x00C5, 0x00C7, 0x00C9, 0x00D1, 0x00D6, 0x00DC, 0x00E1, 0x00E0, 0x00E2, 0x00E4, 0x00E3, 0x00E5, 0x00E7,
    0x00E9, 0x00E8, 0x00EA, 0x00EB, 0x00ED, 0x00EC, 0x00EE, 0x00EF, 0x00F1, 0x00F3, 0x00F2, 0x00F4, 0x00F6, 0x00F5,
    0x00FA, 0x00F9, 0x00FB, 0x00FC, 0x2020, 0x00B0, 0x00A2, 0x00A3, 0x00A7, 0x2022, 0x00B6, 0x00DF, 0x00AE, 0x00A9,
    0x2122, 0x00B4, 0x00A8, 0x2260, 0x00C6, 0x00D8, 0x221E, 0x00B1, 0x2264, 0x2265, 0x00A5, 0x00B5, 0x2202, 0x2211,
    0x220F, 0x03C0, 0x222B, 0x00AA, 0x00BA, 0x03A9, 0x00E6, 0x00F8, 0x00BF, 0x00A1, 0x00AC, 0x221A, 0x0192, 0x2248,
    0x2206, 0x00AB, 0x00BB, 0x2026, 0x00A0, 0x00C0, 0x00C3, 0x00D5, 0x0152, 0x0153, 0x2013, 0x2014, 0x201C, 0x201D,
    0x2018, 0x2019, 0x00F7, 0x25CA, 0x00FF, 0x0178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02, 0x2021, 0x00B7,
    0x201A, 0x201E, 0x2030, 0x00C2, 0x00CA, 0x00C1, 0x00CB, 0x00C8, 0x00CD, 0x00CE, 0x00CF, 0x00CC, 0x00D3, 0x00D4,
    0xF8FF, 0x00D2, 0x00DA, 0x00DB, 0x00D9, 0x0131, 0x02C6, 0x02DC, 0x00AF, 0x02D8, 0x02D9, 0x02DA, 0x00B8, 0x02DD,
    0x02DB, 0x02C7,
];

fn decode(platform: u16, encoding: u16, raw: &[u8]) -> Option<String> {
    let utf16 = matches!(platform, 0) || (platform == 3 && matches!(encoding, 0 | 1 | 10));
    if utf16 {
        let units = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]));
        return Some(char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect());
    }
    if platform == 1 && encoding == 0 {
        return Some(
            raw.iter()
                .map(|&b| if b < 0x80 { b as char } else { char::from_u32(MAC_ROMAN_HIGH[(b - 0x80) as usize] as u32).unwrap_or('\u{FFFD}') })
                .collect(),
        );
    }
    None
}

/// Every record of the font's `name` table that decodes (formats 0 and 1; language-tag records of format 1
/// keep their numeric language id). Empty when the table is missing or malformed.
pub fn records(font: &Font) -> Vec<NameRecord> {
    let mut out = Vec::new();
    let Some(t) = font.table(b"name") else { return out };
    let (Some(count), Some(storage)) = (u16_at(t, 2), u16_at(t, 4)) else { return out };
    for i in 0..count as usize {
        let r = 6 + 12 * i;
        let (Some(platform), Some(encoding), Some(language), Some(name_id), Some(len), Some(off)) =
            (u16_at(t, r), u16_at(t, r + 2), u16_at(t, r + 4), u16_at(t, r + 6), u16_at(t, r + 8), u16_at(t, r + 10))
        else {
            break;
        };
        let Some(raw) = slice(t, storage as usize + off as usize, len as usize) else { continue };
        if let Some(value) = decode(platform, encoding, raw) {
            out.push(NameRecord { platform, encoding, language, name_id, value });
        }
    }
    out
}

/// Whether a record is in English (Windows 0x0409 / any 0x??09, Mac 0, Unicode platform).
fn english(r: &NameRecord) -> bool {
    match r.platform {
        3 => r.language & 0xFF == 0x09,
        1 => r.language == 0,
        _ => true,
    }
}

/// The values of `name_id`, English first (Windows, then Mac, then Unicode platform), then other languages;
/// duplicates removed.
pub fn values(font: &Font, name_id: u16) -> Vec<String> {
    let recs = records(font);
    let mut v: Vec<(u8, String)> = recs
        .into_iter()
        .filter(|r| r.name_id == name_id && !r.value.is_empty())
        .map(|r| {
            let rank = match (english(&r), r.platform) {
                (true, 3) => 0,
                (true, 1) => 1,
                (true, _) => 2,
                _ => 3,
            };
            (rank, r.value)
        })
        .collect();
    v.sort_by_key(|(rank, _)| *rank);
    let mut out: Vec<String> = Vec::new();
    for (_, s) in v {
        if !out.iter().any(|o| o == &s) {
            out.push(s);
        }
    }
    out
}

/// The family names a font answers to: legacy family (ID 1) first, then the typographic family (ID 16),
/// all languages — fontconfig's `family` list for the face.
pub fn family_names(font: &Font) -> Vec<String> {
    let mut v = values(font, FAMILY);
    for s in values(font, TYPOGRAPHIC_FAMILY) {
        if !v.iter().any(|o| o == &s) {
            v.push(s);
        }
    }
    v
}

/// The English family name to display (ID 1).
pub fn family(font: &Font) -> Option<String> {
    values(font, FAMILY).into_iter().next()
}
