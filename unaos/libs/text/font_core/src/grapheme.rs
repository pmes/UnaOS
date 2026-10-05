//! UAX #29 extended grapheme clusters (Unicode 17.0.0): rules GB3–GB13 incl. GB9c (Indic conjunct breaks) and
//! GB11 (emoji ZWJ sequences), from Grapheme_Cluster_Break, Indic_Conjunct_Break and Extended_Pictographic
//! (`ucd::grapheme`, generated). `tests/grapheme.rs` runs GraphemeBreakTest-17.0.0.txt in full.

use crate::ucd::{self, in_set, lookup, Gcb, InCb};
use alloc::vec::Vec;

pub fn grapheme_break_property(c: char) -> Gcb {
    lookup(ucd::grapheme::GCB, c as u32).unwrap_or(Gcb::Other)
}

pub fn indic_conjunct_break(c: char) -> InCb {
    lookup(ucd::grapheme::INCB, c as u32).unwrap_or(InCb::None)
}

pub fn is_extended_pictographic(c: char) -> bool {
    in_set(ucd::grapheme::EXT_PICT, c as u32)
}

/// Grapheme cluster boundaries as char indices: `out[i]` is true when a boundary falls BEFORE char `i`;
/// `out[0]` and `out[n]` are true (GB1, GB2).
pub fn boundaries(text: &str) -> Vec<bool> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = alloc::vec![false; n + 1];
    if n == 0 {
        return out;
    }
    out[0] = true;
    out[n] = true;
    let p: Vec<Gcb> = chars.iter().map(|&c| grapheme_break_property(c)).collect();
    // GB9c state: seen InCB=Consonant followed only by Extend/Linker, and whether a Linker occurred.
    let mut conj_consonant = false;
    let mut conj_linker = false;
    // GB11 state: ExtPict Extend* (then ZWJ).
    let mut pict = false;
    // GB12/13: count of RI in the current run.
    let mut ri = 0usize;
    for i in 0..n {
        if i > 0 {
            out[i] = decide(p[i - 1], p[i], chars[i], conj_consonant && conj_linker, pict, ri);
        }
        // Update states with char i.
        let c = chars[i];
        match indic_conjunct_break(c) {
            InCb::Consonant => {
                conj_consonant = true;
                conj_linker = false;
            }
            InCb::Linker if conj_consonant => conj_linker = true,
            InCb::Extend if conj_consonant => {}
            _ => {
                conj_consonant = false;
                conj_linker = false;
            }
        }
        if is_extended_pictographic(c) {
            pict = true;
        } else if !(p[i] == Gcb::Extend || (p[i] == Gcb::ZWJ && pict && i > 0 && p[i - 1] != Gcb::ZWJ)) {
            pict = false;
        }
        if p[i] == Gcb::Regional_Indicator {
            ri += 1;
        } else {
            ri = 0;
        }
    }
    out
}

fn decide(a: Gcb, b: Gcb, bc: char, conj: bool, pict: bool, ri: usize) -> bool {
    use Gcb::*;
    if a == CR && b == LF {
        return false; // GB3
    }
    if matches!(a, Control | CR | LF) || matches!(b, Control | CR | LF) {
        return true; // GB4, GB5
    }
    if a == L && matches!(b, L | V | LV | LVT) {
        return false; // GB6
    }
    if matches!(a, LV | V) && matches!(b, V | T) {
        return false; // GB7
    }
    if matches!(a, LVT | T) && b == T {
        return false; // GB8
    }
    if matches!(b, Extend | ZWJ) {
        return false; // GB9
    }
    if b == SpacingMark {
        return false; // GB9a
    }
    if a == Prepend {
        return false; // GB9b
    }
    if conj && indic_conjunct_break(bc) == InCb::Consonant {
        return false; // GB9c
    }
    if a == ZWJ && pict && is_extended_pictographic(bc) {
        return false; // GB11
    }
    if a == Regional_Indicator && b == Regional_Indicator && ri % 2 == 1 {
        return false; // GB12, GB13
    }
    true // GB999
}

/// Grapheme clusters as byte ranges.
pub fn clusters(text: &str) -> Vec<(usize, usize)> {
    let b = boundaries(text);
    let offs: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(core::iter::once(text.len())).collect();
    let mut out = Vec::new();
    let mut s = 0;
    for i in 1..b.len() {
        if b[i] {
            out.push((offs[s], offs[i]));
            s = i;
        }
    }
    out
}
