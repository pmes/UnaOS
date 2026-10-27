// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Simple-font encodings (ISO 32000-1 §9.6.6, Annex D) and glyph names -> Unicode (the Adobe Glyph List).
//!
//! StandardEncoding is font_core's (`font_core::type1::standard_encoding_name`, one table for both crates). The
//! tables below are GENERATED, not typed: the PDF3 generator (`gen_tables.py`, sha256
//! 6c1608d2c462311e9829c67ebe7c83ec6f908d4e7854e047b66fd87b0a65b722, kept in the arc's scratch) reads fontTools 4.53.1's
//! AGLFN 1.7 / legacy AGL and MacRoman, and CPython's cp1252 codec; WinAnsi follows Annex D (codes 160 = space,
//! 173 = hyphen, the unused codes above 32 = bullet), MacRoman Annex D (202 = space, 240 undefined).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::Document;
use crate::object::Object;

/// A simple font's resolved code -> glyph name table (256 entries; `None` = .notdef / undefined).
#[derive(Clone, Debug, PartialEq)]
pub struct Encoding {
    pub names: Vec<Option<String>>,
}

/// The base encodings by their PDF names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Standard,
    WinAnsi,
    MacRoman,
}

impl Base {
    pub fn from_name(n: &[u8]) -> Option<Base> {
        match n {
            b"StandardEncoding" => Some(Base::Standard),
            b"WinAnsiEncoding" => Some(Base::WinAnsi),
            // MacExpertEncoding's expert glyph set is owed; its codes read as MacRoman's would be wrong, so Standard.
            b"MacRomanEncoding" => Some(Base::MacRoman),
            b"MacExpertEncoding" => Some(Base::Standard),
            _ => None,
        }
    }
    pub fn name(self, code: u8) -> Option<&'static str> {
        let n = match self {
            Base::Standard => return font_core::type1::standard_encoding_name(code),
            Base::WinAnsi => WIN_ANSI[code as usize],
            Base::MacRoman => MAC_ROMAN[code as usize],
        };
        if n.is_empty() { None } else { Some(n) }
    }
}

impl Encoding {
    /// A table filled from `base` (a closure, so a font program's built-in encoding serves too).
    pub fn from_fn(base: impl Fn(u8) -> Option<String>) -> Encoding {
        Encoding { names: (0..=255u8).map(base).collect() }
    }

    pub fn from_base(base: Base) -> Encoding {
        Encoding::from_fn(|c| base.name(c).map(String::from))
    }

    /// An empty table (every code undefined): a symbolic font with no built-in encoding read.
    pub fn empty() -> Encoding {
        Encoding { names: vec![None; 256] }
    }

    /// Apply a /Differences array (§9.6.6.1): `code /name /name ... code /name ...`.
    pub fn apply_differences(&mut self, doc: &Document, diffs: &[Object]) {
        let mut code: Option<usize> = None;
        for o in diffs {
            match doc.get(o) {
                Object::Int(i) => code = usize::try_from(i).ok(),
                Object::Real(r) if r >= 0.0 => code = Some(r as usize),
                Object::Name(n) => {
                    if let Some(c) = code {
                        if c < 256 {
                            self.names[c] = Some(String::from_utf8_lossy(&n).into_owned());
                        }
                        code = Some(c + 1);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn name(&self, code: u8) -> Option<&str> {
        self.names.get(code as usize)?.as_deref()
    }
}

/// The MacRoman code of a glyph name (TrueType (1,0) cmap lookups, §9.6.6.4).
pub fn mac_roman_code(name: &str) -> Option<u8> {
    MAC_ROMAN.iter().position(|n| !n.is_empty() && *n == name).map(|c| c as u8)
}

fn hex_cp(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(s, 16).ok()
}

fn push_cp(out: &mut String, cp: u32) -> Option<()> {
    out.push(char::from_u32(cp)?);
    Some(())
}

/// A glyph name -> its Unicode text (AGL specification §2): the part before the first `.` is split at `_`; each
/// component is an AGL name, `uniXXXX[XXXX...]` (BMP, no surrogates) or `uXXXX`..`uXXXXXX`. `None` if any component
/// maps to nothing.
pub fn agl(name: &str) -> Option<String> {
    let base = name.split('.').next()?;
    if base.is_empty() {
        return None;
    }
    let mut out = String::new();
    for comp in base.split('_') {
        if let Ok(i) = AGL.binary_search_by(|e| e.0.cmp(comp)) {
            push_cp(&mut out, AGL[i].1 as u32)?;
        } else if let Some(h) = comp.strip_prefix("uni").filter(|h| h.len() >= 4 && h.len() % 4 == 0) {
            for k in (0..h.len()).step_by(4) {
                let cp = hex_cp(&h[k..k + 4])?;
                if (0xD800..0xE000).contains(&cp) {
                    return None;
                }
                push_cp(&mut out, cp)?;
            }
        } else if let Some(h) = comp.strip_prefix('u').filter(|h| (4..=6).contains(&h.len())) {
            push_cp(&mut out, hex_cp(h)?)?;
        } else {
            return None;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

#[rustfmt::skip]
mod tables {
    /// WinAnsiEncoding (ISO 32000-1 Annex D.2): code -> glyph name ("" = undefined).
    pub static WIN_ANSI: [&str; 256] = [
        "", "", "", "", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand", "quotesingle",
        "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period", "slash",
        "zero", "one", "two", "three", "four", "five", "six", "seven",
        "eight", "nine", "colon", "semicolon", "less", "equal", "greater", "question",
        "at", "A", "B", "C", "D", "E", "F", "G",
        "H", "I", "J", "K", "L", "M", "N", "O",
        "P", "Q", "R", "S", "T", "U", "V", "W",
        "X", "Y", "Z", "bracketleft", "backslash", "bracketright", "asciicircum", "underscore",
        "grave", "a", "b", "c", "d", "e", "f", "g",
        "h", "i", "j", "k", "l", "m", "n", "o",
        "p", "q", "r", "s", "t", "u", "v", "w",
        "x", "y", "z", "braceleft", "bar", "braceright", "asciitilde", "bullet",
        "Euro", "bullet", "quotesinglbase", "florin", "quotedblbase", "ellipsis", "dagger", "daggerdbl",
        "circumflex", "perthousand", "Scaron", "guilsinglleft", "OE", "bullet", "Zcaron", "bullet",
        "bullet", "quoteleft", "quoteright", "quotedblleft", "quotedblright", "bullet", "endash", "emdash",
        "tilde", "trademark", "scaron", "guilsinglright", "oe", "bullet", "zcaron", "Ydieresis",
        "space", "exclamdown", "cent", "sterling", "currency", "yen", "brokenbar", "section",
        "dieresis", "copyright", "ordfeminine", "guillemotleft", "logicalnot", "hyphen", "registered", "macron",
        "degree", "plusminus", "twosuperior", "threesuperior", "acute", "mu", "paragraph", "periodcentered",
        "cedilla", "onesuperior", "ordmasculine", "guillemotright", "onequarter", "onehalf", "threequarters", "questiondown",
        "Agrave", "Aacute", "Acircumflex", "Atilde", "Adieresis", "Aring", "AE", "Ccedilla",
        "Egrave", "Eacute", "Ecircumflex", "Edieresis", "Igrave", "Iacute", "Icircumflex", "Idieresis",
        "Eth", "Ntilde", "Ograve", "Oacute", "Ocircumflex", "Otilde", "Odieresis", "multiply",
        "Oslash", "Ugrave", "Uacute", "Ucircumflex", "Udieresis", "Yacute", "Thorn", "germandbls",
        "agrave", "aacute", "acircumflex", "atilde", "adieresis", "aring", "ae", "ccedilla",
        "egrave", "eacute", "ecircumflex", "edieresis", "igrave", "iacute", "icircumflex", "idieresis",
        "eth", "ntilde", "ograve", "oacute", "ocircumflex", "otilde", "odieresis", "divide",
        "oslash", "ugrave", "uacute", "ucircumflex", "udieresis", "yacute", "thorn", "ydieresis",
    ];

    /// MacRomanEncoding (ISO 32000-1 Annex D.2): code -> glyph name ("" = undefined).
    pub static MAC_ROMAN: [&str; 256] = [
        "", "", "", "", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand", "quotesingle",
        "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period", "slash",
        "zero", "one", "two", "three", "four", "five", "six", "seven",
        "eight", "nine", "colon", "semicolon", "less", "equal", "greater", "question",
        "at", "A", "B", "C", "D", "E", "F", "G",
        "H", "I", "J", "K", "L", "M", "N", "O",
        "P", "Q", "R", "S", "T", "U", "V", "W",
        "X", "Y", "Z", "bracketleft", "backslash", "bracketright", "asciicircum", "underscore",
        "grave", "a", "b", "c", "d", "e", "f", "g",
        "h", "i", "j", "k", "l", "m", "n", "o",
        "p", "q", "r", "s", "t", "u", "v", "w",
        "x", "y", "z", "braceleft", "bar", "braceright", "asciitilde", "",
        "Adieresis", "Aring", "Ccedilla", "Eacute", "Ntilde", "Odieresis", "Udieresis", "aacute",
        "agrave", "acircumflex", "adieresis", "atilde", "aring", "ccedilla", "eacute", "egrave",
        "ecircumflex", "edieresis", "iacute", "igrave", "icircumflex", "idieresis", "ntilde", "oacute",
        "ograve", "ocircumflex", "odieresis", "otilde", "uacute", "ugrave", "ucircumflex", "udieresis",
        "dagger", "degree", "cent", "sterling", "section", "bullet", "paragraph", "germandbls",
        "registered", "copyright", "trademark", "acute", "dieresis", "notequal", "AE", "Oslash",
        "infinity", "plusminus", "lessequal", "greaterequal", "yen", "mu", "partialdiff", "summation",
        "product", "pi", "integral", "ordfeminine", "ordmasculine", "Omega", "ae", "oslash",
        "questiondown", "exclamdown", "logicalnot", "radical", "florin", "approxequal", "Delta", "guillemotleft",
        "guillemotright", "ellipsis", "space", "Agrave", "Atilde", "Otilde", "OE", "oe",
        "endash", "emdash", "quotedblleft", "quotedblright", "quoteleft", "quoteright", "divide", "lozenge",
        "ydieresis", "Ydieresis", "fraction", "currency", "guilsinglleft", "guilsinglright", "fi", "fl",
        "daggerdbl", "periodcentered", "quotesinglbase", "quotedblbase", "perthousand", "Acircumflex", "Ecircumflex", "Aacute",
        "Edieresis", "Egrave", "Iacute", "Icircumflex", "Idieresis", "Igrave", "Oacute", "Ocircumflex",
        "", "Ograve", "Uacute", "Ucircumflex", "Ugrave", "dotlessi", "circumflex", "tilde",
        "macron", "breve", "dotaccent", "ring", "cedilla", "hungarumlaut", "ogonek", "caron",
    ];

    /// AGLFN 1.7 plus every name the Standard / WinAnsi / MacRoman tables use (from the full AGL), 591 names, sorted
    /// by name (byte order): glyph name -> code point.
    pub static AGL: [(&str, u16); 591] = [
        ("A", 0x0041), ("AE", 0x00C6), ("AEacute", 0x01FC), ("Aacute", 0x00C1), ("Abreve", 0x0102),
        ("Acircumflex", 0x00C2), ("Adieresis", 0x00C4), ("Agrave", 0x00C0), ("Alpha", 0x0391), ("Alphatonos", 0x0386),
        ("Amacron", 0x0100), ("Aogonek", 0x0104), ("Aring", 0x00C5), ("Aringacute", 0x01FA), ("Atilde", 0x00C3),
        ("B", 0x0042), ("Beta", 0x0392), ("C", 0x0043), ("Cacute", 0x0106), ("Ccaron", 0x010C),
        ("Ccedilla", 0x00C7), ("Ccircumflex", 0x0108), ("Cdotaccent", 0x010A), ("Chi", 0x03A7), ("D", 0x0044),
        ("Dcaron", 0x010E), ("Dcroat", 0x0110), ("Delta", 0x2206), ("E", 0x0045), ("Eacute", 0x00C9),
        ("Ebreve", 0x0114), ("Ecaron", 0x011A), ("Ecircumflex", 0x00CA), ("Edieresis", 0x00CB), ("Edotaccent", 0x0116),
        ("Egrave", 0x00C8), ("Emacron", 0x0112), ("Eng", 0x014A), ("Eogonek", 0x0118), ("Epsilon", 0x0395),
        ("Epsilontonos", 0x0388), ("Eta", 0x0397), ("Etatonos", 0x0389), ("Eth", 0x00D0), ("Euro", 0x20AC),
        ("F", 0x0046), ("G", 0x0047), ("Gamma", 0x0393), ("Gbreve", 0x011E), ("Gcaron", 0x01E6),
        ("Gcircumflex", 0x011C), ("Gdotaccent", 0x0120), ("H", 0x0048), ("H18533", 0x25CF), ("H18543", 0x25AA),
        ("H18551", 0x25AB), ("H22073", 0x25A1), ("Hbar", 0x0126), ("Hcircumflex", 0x0124), ("I", 0x0049),
        ("IJ", 0x0132), ("Iacute", 0x00CD), ("Ibreve", 0x012C), ("Icircumflex", 0x00CE), ("Idieresis", 0x00CF),
        ("Idotaccent", 0x0130), ("Ifraktur", 0x2111), ("Igrave", 0x00CC), ("Imacron", 0x012A), ("Iogonek", 0x012E),
        ("Iota", 0x0399), ("Iotadieresis", 0x03AA), ("Iotatonos", 0x038A), ("Itilde", 0x0128), ("J", 0x004A),
        ("Jcircumflex", 0x0134), ("K", 0x004B), ("Kappa", 0x039A), ("L", 0x004C), ("Lacute", 0x0139),
        ("Lambda", 0x039B), ("Lcaron", 0x013D), ("Ldot", 0x013F), ("Lslash", 0x0141), ("M", 0x004D),
        ("Mu", 0x039C), ("N", 0x004E), ("Nacute", 0x0143), ("Ncaron", 0x0147), ("Ntilde", 0x00D1),
        ("Nu", 0x039D), ("O", 0x004F), ("OE", 0x0152), ("Oacute", 0x00D3), ("Obreve", 0x014E),
        ("Ocircumflex", 0x00D4), ("Odieresis", 0x00D6), ("Ograve", 0x00D2), ("Ohorn", 0x01A0), ("Ohungarumlaut", 0x0150),
        ("Omacron", 0x014C), ("Omega", 0x2126), ("Omegatonos", 0x038F), ("Omicron", 0x039F), ("Omicrontonos", 0x038C),
        ("Oslash", 0x00D8), ("Oslashacute", 0x01FE), ("Otilde", 0x00D5), ("P", 0x0050), ("Phi", 0x03A6),
        ("Pi", 0x03A0), ("Psi", 0x03A8), ("Q", 0x0051), ("R", 0x0052), ("Racute", 0x0154),
        ("Rcaron", 0x0158), ("Rfraktur", 0x211C), ("Rho", 0x03A1), ("S", 0x0053), ("SF010000", 0x250C),
        ("SF020000", 0x2514), ("SF030000", 0x2510), ("SF040000", 0x2518), ("SF050000", 0x253C), ("SF060000", 0x252C),
        ("SF070000", 0x2534), ("SF080000", 0x251C), ("SF090000", 0x2524), ("SF100000", 0x2500), ("SF110000", 0x2502),
        ("SF190000", 0x2561), ("SF200000", 0x2562), ("SF210000", 0x2556), ("SF220000", 0x2555), ("SF230000", 0x2563),
        ("SF240000", 0x2551), ("SF250000", 0x2557), ("SF260000", 0x255D), ("SF270000", 0x255C), ("SF280000", 0x255B),
        ("SF360000", 0x255E), ("SF370000", 0x255F), ("SF380000", 0x255A), ("SF390000", 0x2554), ("SF400000", 0x2569),
        ("SF410000", 0x2566), ("SF420000", 0x2560), ("SF430000", 0x2550), ("SF440000", 0x256C), ("SF450000", 0x2567),
        ("SF460000", 0x2568), ("SF470000", 0x2564), ("SF480000", 0x2565), ("SF490000", 0x2559), ("SF500000", 0x2558),
        ("SF510000", 0x2552), ("SF520000", 0x2553), ("SF530000", 0x256B), ("SF540000", 0x256A), ("Sacute", 0x015A),
        ("Scaron", 0x0160), ("Scedilla", 0x015E), ("Scircumflex", 0x015C), ("Sigma", 0x03A3), ("T", 0x0054),
        ("Tau", 0x03A4), ("Tbar", 0x0166), ("Tcaron", 0x0164), ("Theta", 0x0398), ("Thorn", 0x00DE),
        ("U", 0x0055), ("Uacute", 0x00DA), ("Ubreve", 0x016C), ("Ucircumflex", 0x00DB), ("Udieresis", 0x00DC),
        ("Ugrave", 0x00D9), ("Uhorn", 0x01AF), ("Uhungarumlaut", 0x0170), ("Umacron", 0x016A), ("Uogonek", 0x0172),
        ("Upsilon", 0x03A5), ("Upsilon1", 0x03D2), ("Upsilondieresis", 0x03AB), ("Upsilontonos", 0x038E), ("Uring", 0x016E),
        ("Utilde", 0x0168), ("V", 0x0056), ("W", 0x0057), ("Wacute", 0x1E82), ("Wcircumflex", 0x0174),
        ("Wdieresis", 0x1E84), ("Wgrave", 0x1E80), ("X", 0x0058), ("Xi", 0x039E), ("Y", 0x0059),
        ("Yacute", 0x00DD), ("Ycircumflex", 0x0176), ("Ydieresis", 0x0178), ("Ygrave", 0x1EF2), ("Z", 0x005A),
        ("Zacute", 0x0179), ("Zcaron", 0x017D), ("Zdotaccent", 0x017B), ("Zeta", 0x0396), ("a", 0x0061),
        ("aacute", 0x00E1), ("abreve", 0x0103), ("acircumflex", 0x00E2), ("acute", 0x00B4), ("acutecomb", 0x0301),
        ("adieresis", 0x00E4), ("ae", 0x00E6), ("aeacute", 0x01FD), ("agrave", 0x00E0), ("aleph", 0x2135),
        ("alpha", 0x03B1), ("alphatonos", 0x03AC), ("amacron", 0x0101), ("ampersand", 0x0026), ("angle", 0x2220),
        ("angleleft", 0x2329), ("angleright", 0x232A), ("anoteleia", 0x0387), ("aogonek", 0x0105), ("approxequal", 0x2248),
        ("aring", 0x00E5), ("aringacute", 0x01FB), ("arrowboth", 0x2194), ("arrowdblboth", 0x21D4), ("arrowdbldown", 0x21D3),
        ("arrowdblleft", 0x21D0), ("arrowdblright", 0x21D2), ("arrowdblup", 0x21D1), ("arrowdown", 0x2193), ("arrowleft", 0x2190),
        ("arrowright", 0x2192), ("arrowup", 0x2191), ("arrowupdn", 0x2195), ("arrowupdnbse", 0x21A8), ("asciicircum", 0x005E),
        ("asciitilde", 0x007E), ("asterisk", 0x002A), ("asteriskmath", 0x2217), ("at", 0x0040), ("atilde", 0x00E3),
        ("b", 0x0062), ("backslash", 0x005C), ("bar", 0x007C), ("beta", 0x03B2), ("block", 0x2588),
        ("braceleft", 0x007B), ("braceright", 0x007D), ("bracketleft", 0x005B), ("bracketright", 0x005D), ("breve", 0x02D8),
        ("brokenbar", 0x00A6), ("bullet", 0x2022), ("c", 0x0063), ("cacute", 0x0107), ("caron", 0x02C7),
        ("carriagereturn", 0x21B5), ("ccaron", 0x010D), ("ccedilla", 0x00E7), ("ccircumflex", 0x0109), ("cdotaccent", 0x010B),
        ("cedilla", 0x00B8), ("cent", 0x00A2), ("chi", 0x03C7), ("circle", 0x25CB), ("circlemultiply", 0x2297),
        ("circleplus", 0x2295), ("circumflex", 0x02C6), ("club", 0x2663), ("colon", 0x003A), ("colonmonetary", 0x20A1),
        ("comma", 0x002C), ("congruent", 0x2245), ("copyright", 0x00A9), ("currency", 0x00A4), ("d", 0x0064),
        ("dagger", 0x2020), ("daggerdbl", 0x2021), ("dcaron", 0x010F), ("dcroat", 0x0111), ("degree", 0x00B0),
        ("delta", 0x03B4), ("diamond", 0x2666), ("dieresis", 0x00A8), ("dieresistonos", 0x0385), ("divide", 0x00F7),
        ("dkshade", 0x2593), ("dnblock", 0x2584), ("dollar", 0x0024), ("dong", 0x20AB), ("dotaccent", 0x02D9),
        ("dotbelowcomb", 0x0323), ("dotlessi", 0x0131), ("dotmath", 0x22C5), ("e", 0x0065), ("eacute", 0x00E9),
        ("ebreve", 0x0115), ("ecaron", 0x011B), ("ecircumflex", 0x00EA), ("edieresis", 0x00EB), ("edotaccent", 0x0117),
        ("egrave", 0x00E8), ("eight", 0x0038), ("element", 0x2208), ("ellipsis", 0x2026), ("emacron", 0x0113),
        ("emdash", 0x2014), ("emptyset", 0x2205), ("endash", 0x2013), ("eng", 0x014B), ("eogonek", 0x0119),
        ("epsilon", 0x03B5), ("epsilontonos", 0x03AD), ("equal", 0x003D), ("equivalence", 0x2261), ("estimated", 0x212E),
        ("eta", 0x03B7), ("etatonos", 0x03AE), ("eth", 0x00F0), ("exclam", 0x0021), ("exclamdbl", 0x203C),
        ("exclamdown", 0x00A1), ("existential", 0x2203), ("f", 0x0066), ("female", 0x2640), ("fi", 0xFB01),
        ("figuredash", 0x2012), ("filledbox", 0x25A0), ("filledrect", 0x25AC), ("five", 0x0035), ("fiveeighths", 0x215D),
        ("fl", 0xFB02), ("florin", 0x0192), ("four", 0x0034), ("fraction", 0x2044), ("franc", 0x20A3),
        ("g", 0x0067), ("gamma", 0x03B3), ("gbreve", 0x011F), ("gcaron", 0x01E7), ("gcircumflex", 0x011D),
        ("gdotaccent", 0x0121), ("germandbls", 0x00DF), ("gradient", 0x2207), ("grave", 0x0060), ("gravecomb", 0x0300),
        ("greater", 0x003E), ("greaterequal", 0x2265), ("guillemotleft", 0x00AB), ("guillemotright", 0x00BB), ("guilsinglleft", 0x2039),
        ("guilsinglright", 0x203A), ("h", 0x0068), ("hbar", 0x0127), ("hcircumflex", 0x0125), ("heart", 0x2665),
        ("hookabovecomb", 0x0309), ("house", 0x2302), ("hungarumlaut", 0x02DD), ("hyphen", 0x002D), ("i", 0x0069),
        ("iacute", 0x00ED), ("ibreve", 0x012D), ("icircumflex", 0x00EE), ("idieresis", 0x00EF), ("igrave", 0x00EC),
        ("ij", 0x0133), ("imacron", 0x012B), ("infinity", 0x221E), ("integral", 0x222B), ("integralbt", 0x2321),
        ("integraltp", 0x2320), ("intersection", 0x2229), ("invbullet", 0x25D8), ("invcircle", 0x25D9), ("invsmileface", 0x263B),
        ("iogonek", 0x012F), ("iota", 0x03B9), ("iotadieresis", 0x03CA), ("iotadieresistonos", 0x0390), ("iotatonos", 0x03AF),
        ("itilde", 0x0129), ("j", 0x006A), ("jcircumflex", 0x0135), ("k", 0x006B), ("kappa", 0x03BA),
        ("kgreenlandic", 0x0138), ("l", 0x006C), ("lacute", 0x013A), ("lambda", 0x03BB), ("lcaron", 0x013E),
        ("ldot", 0x0140), ("less", 0x003C), ("lessequal", 0x2264), ("lfblock", 0x258C), ("lira", 0x20A4),
        ("logicaland", 0x2227), ("logicalnot", 0x00AC), ("logicalor", 0x2228), ("longs", 0x017F), ("lozenge", 0x25CA),
        ("lslash", 0x0142), ("ltshade", 0x2591), ("m", 0x006D), ("macron", 0x00AF), ("male", 0x2642),
        ("minus", 0x2212), ("minute", 0x2032), ("mu", 0x00B5), ("multiply", 0x00D7), ("musicalnote", 0x266A),
        ("musicalnotedbl", 0x266B), ("n", 0x006E), ("nacute", 0x0144), ("napostrophe", 0x0149), ("ncaron", 0x0148),
        ("nine", 0x0039), ("notelement", 0x2209), ("notequal", 0x2260), ("notsubset", 0x2284), ("ntilde", 0x00F1),
        ("nu", 0x03BD), ("numbersign", 0x0023), ("o", 0x006F), ("oacute", 0x00F3), ("obreve", 0x014F),
        ("ocircumflex", 0x00F4), ("odieresis", 0x00F6), ("oe", 0x0153), ("ogonek", 0x02DB), ("ograve", 0x00F2),
        ("ohorn", 0x01A1), ("ohungarumlaut", 0x0151), ("omacron", 0x014D), ("omega", 0x03C9), ("omega1", 0x03D6),
        ("omegatonos", 0x03CE), ("omicron", 0x03BF), ("omicrontonos", 0x03CC), ("one", 0x0031), ("onedotenleader", 0x2024),
        ("oneeighth", 0x215B), ("onehalf", 0x00BD), ("onequarter", 0x00BC), ("onesuperior", 0x00B9), ("onethird", 0x2153),
        ("openbullet", 0x25E6), ("ordfeminine", 0x00AA), ("ordmasculine", 0x00BA), ("orthogonal", 0x221F), ("oslash", 0x00F8),
        ("oslashacute", 0x01FF), ("otilde", 0x00F5), ("p", 0x0070), ("paragraph", 0x00B6), ("parenleft", 0x0028),
        ("parenright", 0x0029), ("partialdiff", 0x2202), ("percent", 0x0025), ("period", 0x002E), ("periodcentered", 0x00B7),
        ("perpendicular", 0x22A5), ("perthousand", 0x2030), ("peseta", 0x20A7), ("phi", 0x03C6), ("phi1", 0x03D5),
        ("pi", 0x03C0), ("plus", 0x002B), ("plusminus", 0x00B1), ("prescription", 0x211E), ("product", 0x220F),
        ("propersubset", 0x2282), ("propersuperset", 0x2283), ("proportional", 0x221D), ("psi", 0x03C8), ("q", 0x0071),
        ("question", 0x003F), ("questiondown", 0x00BF), ("quotedbl", 0x0022), ("quotedblbase", 0x201E), ("quotedblleft", 0x201C),
        ("quotedblright", 0x201D), ("quoteleft", 0x2018), ("quotereversed", 0x201B), ("quoteright", 0x2019), ("quotesinglbase", 0x201A),
        ("quotesingle", 0x0027), ("r", 0x0072), ("racute", 0x0155), ("radical", 0x221A), ("rcaron", 0x0159),
        ("reflexsubset", 0x2286), ("reflexsuperset", 0x2287), ("registered", 0x00AE), ("revlogicalnot", 0x2310), ("rho", 0x03C1),
        ("ring", 0x02DA), ("rtblock", 0x2590), ("s", 0x0073), ("sacute", 0x015B), ("scaron", 0x0161),
        ("scedilla", 0x015F), ("scircumflex", 0x015D), ("second", 0x2033), ("section", 0x00A7), ("semicolon", 0x003B),
        ("seven", 0x0037), ("seveneighths", 0x215E), ("shade", 0x2592), ("sigma", 0x03C3), ("sigma1", 0x03C2),
        ("similar", 0x223C), ("six", 0x0036), ("slash", 0x002F), ("smileface", 0x263A), ("space", 0x0020),
        ("spade", 0x2660), ("sterling", 0x00A3), ("suchthat", 0x220B), ("summation", 0x2211), ("sun", 0x263C),
        ("t", 0x0074), ("tau", 0x03C4), ("tbar", 0x0167), ("tcaron", 0x0165), ("therefore", 0x2234),
        ("theta", 0x03B8), ("theta1", 0x03D1), ("thorn", 0x00FE), ("three", 0x0033), ("threeeighths", 0x215C),
        ("threequarters", 0x00BE), ("threesuperior", 0x00B3), ("tilde", 0x02DC), ("tildecomb", 0x0303), ("tonos", 0x0384),
        ("trademark", 0x2122), ("triagdn", 0x25BC), ("triaglf", 0x25C4), ("triagrt", 0x25BA), ("triagup", 0x25B2),
        ("two", 0x0032), ("twodotenleader", 0x2025), ("twosuperior", 0x00B2), ("twothirds", 0x2154), ("u", 0x0075),
        ("uacute", 0x00FA), ("ubreve", 0x016D), ("ucircumflex", 0x00FB), ("udieresis", 0x00FC), ("ugrave", 0x00F9),
        ("uhorn", 0x01B0), ("uhungarumlaut", 0x0171), ("umacron", 0x016B), ("underscore", 0x005F), ("underscoredbl", 0x2017),
        ("union", 0x222A), ("universal", 0x2200), ("uogonek", 0x0173), ("upblock", 0x2580), ("upsilon", 0x03C5),
        ("upsilondieresis", 0x03CB), ("upsilondieresistonos", 0x03B0), ("upsilontonos", 0x03CD), ("uring", 0x016F), ("utilde", 0x0169),
        ("v", 0x0076), ("w", 0x0077), ("wacute", 0x1E83), ("wcircumflex", 0x0175), ("wdieresis", 0x1E85),
        ("weierstrass", 0x2118), ("wgrave", 0x1E81), ("x", 0x0078), ("xi", 0x03BE), ("y", 0x0079),
        ("yacute", 0x00FD), ("ycircumflex", 0x0177), ("ydieresis", 0x00FF), ("yen", 0x00A5), ("ygrave", 0x1EF3),
        ("z", 0x007A), ("zacute", 0x017A), ("zcaron", 0x017E), ("zdotaccent", 0x017C), ("zero", 0x0030),
        ("zeta", 0x03B6),
    ];
}
pub use tables::{AGL, MAC_ROMAN, WIN_ANSI};
