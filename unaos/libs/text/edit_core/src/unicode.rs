// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! UAX #29 text segmentation: extended grapheme clusters (rules GB3–GB999) and word boundaries
//! (WB3–WB999), over the UCD 17.0.0 property tables generated into `tables.rs` by `tools/gen_tables.py`.
//! Conformance: `tests/uax29.rs` replays GraphemeBreakTest-17.0.0 and WordBreakTest-17.0.0 in full.

use crate::tables::*;
use alloc::vec::Vec;

fn lookup(table: &[(u32, u32, u8)], c: char) -> u8 {
    let c = c as u32;
    match table.binary_search_by(|&(lo, hi, _)| {
        if hi < c {
            core::cmp::Ordering::Less
        } else if lo > c {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Equal
        }
    }) {
        Ok(i) => table[i].2,
        Err(_) => 0,
    }
}

fn in_set(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                core::cmp::Ordering::Less
            } else if lo > c {
                core::cmp::Ordering::Greater
            } else {
                core::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

pub(crate) fn gcb(c: char) -> u8 {
    lookup(GRAPHEME_BREAK, c)
}
pub(crate) fn wb(c: char) -> u8 {
    lookup(WORD_BREAK, c)
}
fn incb(c: char) -> u8 {
    lookup(INDIC_CONJUNCT_BREAK, c)
}
fn ext_pict(c: char) -> bool {
    in_set(EXTENDED_PICTOGRAPHIC, c)
}

/// Is a break allowed between char `i-1` and char `i` (grapheme rules; `i` in `1..n`).
fn grapheme_break(g: &[u8], ep: &[bool], ic: &[u8], i: usize) -> bool {
    let (a, b) = (g[i - 1], g[i]);
    if a == GCB_CR && b == GCB_LF {
        return false; // GB3
    }
    if matches!(a, GCB_CONTROL | GCB_CR | GCB_LF) || matches!(b, GCB_CONTROL | GCB_CR | GCB_LF) {
        return true; // GB4, GB5
    }
    if a == GCB_L && matches!(b, GCB_L | GCB_V | GCB_LV | GCB_LVT) {
        return false; // GB6
    }
    if matches!(a, GCB_LV | GCB_V) && matches!(b, GCB_V | GCB_T) {
        return false; // GB7
    }
    if matches!(a, GCB_LVT | GCB_T) && b == GCB_T {
        return false; // GB8
    }
    if matches!(b, GCB_EXTEND | GCB_ZWJ | GCB_SPACINGMARK) || a == GCB_PREPEND {
        return false; // GB9, GB9a, GB9b
    }
    if ic[i] == INCB_CONSONANT {
        // GB9c: Consonant [Extend Linker]* Linker [Extend Linker]* × Consonant
        let mut j = i;
        let mut linker = false;
        while j > 0 && matches!(ic[j - 1], INCB_EXTEND | INCB_LINKER) {
            linker |= ic[j - 1] == INCB_LINKER;
            j -= 1;
        }
        if linker && j > 0 && ic[j - 1] == INCB_CONSONANT {
            return false;
        }
    }
    if a == GCB_ZWJ && ep[i] {
        // GB11: ExtPict Extend* ZWJ × ExtPict
        let mut j = i - 1;
        while j > 0 && g[j - 1] == GCB_EXTEND {
            j -= 1;
        }
        if j > 0 && ep[j - 1] {
            return false;
        }
    }
    if a == GCB_REGIONAL_INDICATOR && b == GCB_REGIONAL_INDICATOR {
        // GB12, GB13: break only after an even run of RIs
        let mut k = i;
        let mut n = 0;
        while k > 0 && g[k - 1] == GCB_REGIONAL_INDICATOR {
            n += 1;
            k -= 1;
        }
        if n % 2 == 1 {
            return false;
        }
    }
    true // GB999
}

fn wb_ignorable(w: u8) -> bool {
    matches!(w, WB_EXTEND | WB_FORMAT | WB_ZWJ)
}
fn wb_newline(w: u8) -> bool {
    matches!(w, WB_NEWLINE | WB_CR | WB_LF)
}
fn ahletter(w: u8) -> bool {
    matches!(w, WB_ALETTER | WB_HEBREW_LETTER)
}
fn midnumletq(w: u8) -> bool {
    matches!(w, WB_MIDNUMLET | WB_SINGLE_QUOTE)
}

/// WB4: the index of the char that stands for position `i-1` once Extend/Format/ZWJ are attached to their
/// base (never across sot or a newline).
fn wb_eff_prev(w: &[u8], i: usize) -> usize {
    let mut k = i - 1;
    while k > 0 && wb_ignorable(w[k]) && !wb_newline(w[k - 1]) {
        k -= 1;
    }
    k
}

fn word_break(w: &[u8], ep: &[bool], i: usize) -> bool {
    let n = w.len();
    let (a, b) = (w[i - 1], w[i]);
    if a == WB_CR && b == WB_LF {
        return false; // WB3
    }
    if wb_newline(a) || wb_newline(b) {
        return true; // WB3a, WB3b
    }
    if a == WB_ZWJ && ep[i] {
        return false; // WB3c
    }
    if a == WB_WSEGSPACE && b == WB_WSEGSPACE {
        return false; // WB3d
    }
    if wb_ignorable(b) {
        return false; // WB4
    }
    let p = wb_eff_prev(w, i);
    let pa = w[p];
    let pp = if p > 0 { Some(w[wb_eff_prev(w, p)]) } else { None };
    let mut m = i + 1;
    while m < n && wb_ignorable(w[m]) {
        m += 1;
    }
    let nn = if m < n { Some(w[m]) } else { None };
    let pp_is = |f: fn(u8) -> bool| pp.map(f).unwrap_or(false);
    let nn_is = |f: fn(u8) -> bool| nn.map(f).unwrap_or(false);

    if ahletter(pa) && ahletter(b) {
        return false; // WB5
    }
    if ahletter(pa) && (b == WB_MIDLETTER || midnumletq(b)) && nn_is(ahletter) {
        return false; // WB6
    }
    if pp_is(ahletter) && (pa == WB_MIDLETTER || midnumletq(pa)) && ahletter(b) {
        return false; // WB7
    }
    if pa == WB_HEBREW_LETTER && b == WB_SINGLE_QUOTE {
        return false; // WB7a
    }
    if pa == WB_HEBREW_LETTER && b == WB_DOUBLE_QUOTE && nn == Some(WB_HEBREW_LETTER) {
        return false; // WB7b
    }
    if pp == Some(WB_HEBREW_LETTER) && pa == WB_DOUBLE_QUOTE && b == WB_HEBREW_LETTER {
        return false; // WB7c
    }
    if pa == WB_NUMERIC && b == WB_NUMERIC {
        return false; // WB8
    }
    if ahletter(pa) && b == WB_NUMERIC {
        return false; // WB9
    }
    if pa == WB_NUMERIC && ahletter(b) {
        return false; // WB10
    }
    if pp == Some(WB_NUMERIC) && (pa == WB_MIDNUM || midnumletq(pa)) && b == WB_NUMERIC {
        return false; // WB11
    }
    if pa == WB_NUMERIC && (b == WB_MIDNUM || midnumletq(b)) && nn == Some(WB_NUMERIC) {
        return false; // WB12
    }
    if pa == WB_KATAKANA && b == WB_KATAKANA {
        return false; // WB13
    }
    if (ahletter(pa) || matches!(pa, WB_NUMERIC | WB_KATAKANA | WB_EXTENDNUMLET)) && b == WB_EXTENDNUMLET {
        return false; // WB13a
    }
    if pa == WB_EXTENDNUMLET && (ahletter(b) || matches!(b, WB_NUMERIC | WB_KATAKANA)) {
        return false; // WB13b
    }
    if pa == WB_REGIONAL_INDICATOR && b == WB_REGIONAL_INDICATOR {
        // WB15, WB16: count the RI run (over WB4-attached chars) ending at `p`.
        let mut count = 0;
        let mut k = Some(p);
        while let Some(kk) = k {
            if w[kk] != WB_REGIONAL_INDICATOR {
                break;
            }
            count += 1;
            k = if kk > 0 { Some(wb_eff_prev(w, kk)) } else { None };
        }
        if count % 2 == 1 {
            return false;
        }
    }
    true // WB999
}

/// Byte offsets of every extended-grapheme-cluster boundary in `s`, `0` and `s.len()` included
/// (just `[0]` for the empty string).
pub fn grapheme_boundaries(s: &str) -> Vec<usize> {
    let offs: Vec<usize> = s.char_indices().map(|(o, _)| o).collect();
    let chars: Vec<char> = s.chars().collect();
    let g: Vec<u8> = chars.iter().map(|&c| gcb(c)).collect();
    let ep: Vec<bool> = chars.iter().map(|&c| ext_pict(c)).collect();
    let ic: Vec<u8> = chars.iter().map(|&c| incb(c)).collect();
    let mut out = Vec::with_capacity(chars.len() + 1);
    out.push(0);
    for i in 1..chars.len() {
        if grapheme_break(&g, &ep, &ic, i) {
            out.push(offs[i]);
        }
    }
    if !s.is_empty() {
        out.push(s.len());
    }
    out
}

/// Byte offsets of every UAX #29 word boundary in `s`, `0` and `s.len()` included.
pub fn word_boundaries(s: &str) -> Vec<usize> {
    let offs: Vec<usize> = s.char_indices().map(|(o, _)| o).collect();
    let chars: Vec<char> = s.chars().collect();
    let w: Vec<u8> = chars.iter().map(|&c| wb(c)).collect();
    let ep: Vec<bool> = chars.iter().map(|&c| ext_pict(c)).collect();
    let mut out = Vec::with_capacity(chars.len() + 1);
    out.push(0);
    for i in 1..chars.len() {
        if word_break(&w, &ep, i) {
            out.push(offs[i]);
        }
    }
    if !s.is_empty() {
        out.push(s.len());
    }
    out
}

/// The first grapheme boundary strictly after byte `at` (or `s.len()`).
pub fn next_grapheme(s: &str, at: usize) -> usize {
    grapheme_boundaries(s).into_iter().find(|&b| b > at).unwrap_or(s.len())
}

/// The last grapheme boundary strictly before byte `at` (or `0`).
pub fn prev_grapheme(s: &str, at: usize) -> usize {
    grapheme_boundaries(s).into_iter().rev().find(|&b| b < at).unwrap_or(0)
}

/// A "word character" for whole-word search: letters, digits, marks and connector punctuation (`_`).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
        || c == '_'
        || matches!(wb(c), WB_ALETTER | WB_HEBREW_LETTER | WB_NUMERIC | WB_KATAKANA | WB_EXTENDNUMLET)
        || (matches!(gcb(c), GCB_EXTEND | GCB_SPACINGMARK) && !c.is_whitespace())
}
