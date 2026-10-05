//! Unicode character data for the lexer, String.prototype (case mapping, normalization) and RegExp (property
//! escapes, case folding). Every table under this module is generated from UCD 17.0.0 by tools/gen_unicode.py.

#![allow(clippy::unreadable_literal)]

pub mod case;
pub mod emoji;
pub mod gc;
pub mod norm;
pub mod props;
pub mod script;

use alloc::vec::Vec;

/// Binary search a sorted flat `[start, end, ...]` table.
pub fn in_table(t: &[u32], cp: u32) -> bool {
    let n = t.len() / 2;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if cp < t[mid * 2] {
            hi = mid;
        } else if cp > t[mid * 2 + 1] {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

/// Look up a code point in a flat `[start, end, value]` triple table.
fn triple_lookup(t: &[u32], cp: u32) -> Option<u32> {
    let n = t.len() / 3;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if cp < t[mid * 3] {
            hi = mid;
        } else if cp > t[mid * 3 + 1] {
            lo = mid + 1;
        } else {
            return Some(t[mid * 3 + 2]);
        }
    }
    None
}

/// Look up `cp` in a sorted `[key, value]` pair table.
fn pair_lookup(t: &[u32], cp: u32) -> Option<u32> {
    let n = t.len() / 2;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let k = t[mid * 2];
        if cp < k {
            hi = mid;
        } else if cp > k {
            lo = mid + 1;
        } else {
            return Some(t[mid * 2 + 1]);
        }
    }
    None
}

pub fn is_id_start(cp: u32) -> bool {
    if cp < 128 {
        return (cp as u8).is_ascii_alphabetic();
    }
    in_table(&props::P_ID_START, cp)
}

pub fn is_id_continue(cp: u32) -> bool {
    if cp < 128 {
        return (cp as u8).is_ascii_alphanumeric() || cp == b'_' as u32;
    }
    in_table(&props::P_ID_CONTINUE, cp)
}

/// General_Category index into `gc::GC_NAMES`.
pub fn general_category(cp: u32) -> u32 {
    triple_lookup(&gc::GC_RANGES, cp).unwrap_or(2)
}

pub fn gc_index(name: &str) -> usize {
    gc::GC_NAMES.iter().position(|n| *n == name).unwrap_or(0)
}

/// ECMAScript WhiteSpace (§12.2): TAB, VT, FF, ZWNBSP and any Zs code point.
pub fn is_js_whitespace(cp: u32) -> bool {
    match cp {
        0x09 | 0x0B | 0x0C | 0x20 | 0xA0 | 0xFEFF => true,
        _ if cp < 0x80 => false,
        _ => general_category(cp) == gc_index("Zs") as u32,
    }
}

pub fn is_line_terminator(cp: u32) -> bool {
    matches!(cp, 0x0A | 0x0D | 0x2028 | 0x2029)
}

/// StrWhiteSpaceChar: WhiteSpace or LineTerminator.
pub fn is_str_whitespace(cp: u32) -> bool {
    is_js_whitespace(cp) || is_line_terminator(cp)
}

pub fn binary_property(name: &str) -> Option<&'static [u32]> {
    props::BINARY_PROPERTIES.binary_search_by(|(n, _)| (*n).cmp(name)).ok().map(|i| props::BINARY_PROPERTIES[i].1)
}

pub fn gc_value(name: &str) -> Option<u32> {
    gc::GC_ALIASES.binary_search_by(|(n, _)| (*n).cmp(name)).ok().map(|i| gc::GC_ALIASES[i].1)
}

pub fn script_value(name: &str) -> Option<u16> {
    script::SC_ALIASES.binary_search_by(|(n, _)| (*n).cmp(name)).ok().map(|i| script::SC_ALIASES[i].1)
}

pub fn string_property(name: &str) -> Option<&'static [u32]> {
    emoji::STRING_PROPERTIES.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

pub fn script_of(cp: u32) -> u16 {
    match triple_lookup(&script::SC_RANGES, cp) {
        Some(v) => v as u16,
        None => script::SC_NAMES.iter().position(|n| *n == "Zzzz").unwrap_or(0) as u16,
    }
}

/// Does `cp` have script `sc` in its Script_Extensions?
pub fn has_script_extension(cp: u32, sc: u16) -> bool {
    match triple_lookup(&script::SCX_RANGES, cp) {
        Some(off) => {
            let off = off as usize;
            let n = script::SCX_SETS[off] as usize;
            script::SCX_SETS[off + 1..off + 1 + n].contains(&sc)
        }
        None => script_of(cp) == sc,
    }
}

/// Collect the ranges of a General_Category mask as flat pairs.
pub fn gc_ranges(mask: u32) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let t = &gc::GC_RANGES;
    for i in 0..t.len() / 3 {
        if mask & (1 << t[i * 3 + 2]) != 0 {
            push_range(&mut out, t[i * 3], t[i * 3 + 1]);
        }
    }
    out
}

pub fn script_ranges(sc: u16, extensions: bool) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    if !extensions {
        let t = &script::SC_RANGES;
        let unknown = script::SC_NAMES.iter().position(|n| *n == "Zzzz").unwrap_or(usize::MAX) as u16;
        if sc == unknown {
            // Zzzz is everything not listed.
            let mut next = 0u32;
            for i in 0..t.len() / 3 {
                if t[i * 3] > next {
                    push_range(&mut out, next, t[i * 3] - 1);
                }
                next = t[i * 3 + 1] + 1;
            }
            if next <= 0x10FFFF {
                push_range(&mut out, next, 0x10FFFF);
            }
            return out;
        }
        for i in 0..t.len() / 3 {
            if t[i * 3 + 2] == sc as u32 {
                push_range(&mut out, t[i * 3], t[i * 3 + 1]);
            }
        }
        return out;
    }
    // Script_Extensions: walk all code points that are either listed in SCX_RANGES or have Script = sc.
    let base = script_ranges(sc, false);
    let t = &script::SCX_RANGES;
    let mut cps: Vec<(u32, u32)> = Vec::new();
    // Base ranges minus code points that have an explicit extension set.
    let mut i = 0;
    while i < base.len() {
        let (mut a, b) = (base[i], base[i + 1]);
        for j in 0..t.len() / 3 {
            let (s, e) = (t[j * 3], t[j * 3 + 1]);
            if e < a || s > b {
                continue;
            }
            if s > a {
                cps.push((a, s - 1));
            }
            a = e + 1;
            if a > b {
                break;
            }
        }
        if a <= b {
            cps.push((a, b));
        }
        i += 2;
    }
    for j in 0..t.len() / 3 {
        let off = t[j * 3 + 2] as usize;
        let n = script::SCX_SETS[off] as usize;
        if script::SCX_SETS[off + 1..off + 1 + n].contains(&sc) {
            cps.push((t[j * 3], t[j * 3 + 1]));
        }
    }
    cps.sort_unstable();
    for (a, b) in cps {
        push_range(&mut out, a, b);
    }
    out
}

fn push_range(out: &mut Vec<u32>, a: u32, b: u32) {
    let n = out.len();
    if n >= 2 && out[n - 1] + 1 >= a {
        if b > out[n - 1] {
            out[n - 1] = b;
        }
    } else {
        out.push(a);
        out.push(b);
    }
}

// ------------------------------------------------------------------------------------------------ case mapping

pub fn simple_upper(cp: u32) -> u32 {
    if cp < 128 {
        return (cp as u8).to_ascii_uppercase() as u32;
    }
    pair_lookup(&case::SIMPLE_UPPER, cp).unwrap_or(cp)
}

pub fn simple_lower(cp: u32) -> u32 {
    if cp < 128 {
        return (cp as u8).to_ascii_lowercase() as u32;
    }
    pair_lookup(&case::SIMPLE_LOWER, cp).unwrap_or(cp)
}

/// Simple case folding (CaseFolding.txt status C + S).
pub fn simple_fold(cp: u32) -> u32 {
    if cp < 128 {
        return (cp as u8).to_ascii_lowercase() as u32;
    }
    pair_lookup(&case::SIMPLE_FOLD, cp).unwrap_or(cp)
}

fn special(idx: &[u32], data: &'static [u32], cp: u32) -> Option<&'static [u32]> {
    let n = idx.len() / 3;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let k = idx[mid * 3];
        if cp < k {
            hi = mid;
        } else if cp > k {
            lo = mid + 1;
        } else {
            let off = idx[mid * 3 + 1] as usize;
            let len = idx[mid * 3 + 2] as usize;
            return Some(&data[off..off + len]);
        }
    }
    None
}

/// Full uppercase mapping (UnicodeData + unconditional SpecialCasing).
pub fn to_upper_full(cp: u32, out: &mut Vec<u32>) {
    if cp < 128 {
        out.push((cp as u8).to_ascii_uppercase() as u32);
        return;
    }
    if let Some(s) = special(&case::SPECIAL_UPPER, &case::SPECIAL_UPPER_DATA, cp) {
        out.extend_from_slice(s);
    } else {
        out.push(simple_upper(cp));
    }
}

/// Full lowercase mapping without the Final_Sigma context (handled by the caller).
pub fn to_lower_full(cp: u32, out: &mut Vec<u32>) {
    if cp < 128 {
        out.push((cp as u8).to_ascii_lowercase() as u32);
        return;
    }
    if let Some(s) = special(&case::SPECIAL_LOWER, &case::SPECIAL_LOWER_DATA, cp) {
        out.extend_from_slice(s);
    } else {
        out.push(simple_lower(cp));
    }
}

pub fn is_cased(cp: u32) -> bool {
    in_table(&props::P_CASED, cp)
}

pub fn is_case_ignorable(cp: u32) -> bool {
    in_table(&props::P_CASE_IGNORABLE, cp)
}

/// Iterate every code point that has a simple fold mapping, as (cp, folded).
pub fn fold_pairs() -> impl Iterator<Item = (u32, u32)> {
    case::SIMPLE_FOLD.chunks(2).map(|c| (c[0], c[1]))
}

pub fn upper_pairs() -> impl Iterator<Item = (u32, u32)> {
    case::SIMPLE_UPPER.chunks(2).map(|c| (c[0], c[1]))
}

pub fn special_upper_keys() -> impl Iterator<Item = u32> {
    case::SPECIAL_UPPER.chunks(3).map(|c| c[0])
}

// ------------------------------------------------------------------------------------------------ normalization

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

pub fn ccc(cp: u32) -> u8 {
    if cp < 0x300 {
        return 0;
    }
    triple_lookup(&norm::CCC, cp).unwrap_or(0) as u8
}

fn decomp_entry(cp: u32) -> Option<(bool, &'static [u32])> {
    let t = &norm::DECOMP;
    let n = t.len() / 2;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let k = t[mid * 2];
        if cp < k {
            hi = mid;
        } else if cp > k {
            lo = mid + 1;
        } else {
            let v = t[mid * 2 + 1];
            let off = (v >> 6) as usize;
            let len = ((v >> 1) & 31) as usize;
            return Some((v & 1 == 1, &norm::DECOMP_DATA[off..off + len]));
        }
    }
    None
}

fn decompose_into(cp: u32, compat: bool, out: &mut Vec<u32>) {
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let s = cp - S_BASE;
        out.push(L_BASE + s / N_COUNT);
        out.push(V_BASE + (s % N_COUNT) / T_COUNT);
        let t = T_BASE + s % T_COUNT;
        if t != T_BASE {
            out.push(t);
        }
        return;
    }
    if cp >= 0xA0 {
        if let Some((is_compat, d)) = decomp_entry(cp) {
            if !is_compat || compat {
                for &c in d {
                    decompose_into(c, compat, out);
                }
                return;
            }
        }
    }
    out.push(cp);
}

fn compose_pair(a: u32, b: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return Some(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a) && (a - S_BASE) % T_COUNT == 0 && (T_BASE + 1..T_BASE + T_COUNT).contains(&b) {
        return Some(a + (b - T_BASE));
    }
    let t = &norm::COMPOSE;
    let n = t.len() / 3;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let k = (t[mid * 3], t[mid * 3 + 1]);
        if (a, b) < k {
            hi = mid;
        } else if (a, b) > k {
            lo = mid + 1;
        } else {
            return Some(t[mid * 3 + 2]);
        }
    }
    None
}

/// Normalize a code point sequence. `form`: 0 NFC, 1 NFD, 2 NFKC, 3 NFKD.
pub fn normalize(input: &[u32], form: u8) -> Vec<u32> {
    let compat = form >= 2;
    let compose = form == 0 || form == 2;
    let mut d: Vec<u32> = Vec::with_capacity(input.len());
    for &c in input {
        decompose_into(c, compat, &mut d);
    }
    // Canonical ordering: stable sort runs of non-starters by ccc.
    let mut i = 0;
    while i < d.len() {
        if ccc(d[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < d.len() && ccc(d[i]) != 0 {
            i += 1;
        }
        d[start..i].sort_by_key(|&c| ccc(c));
    }
    if !compose {
        return d;
    }
    // Canonical composition.
    let mut out: Vec<u32> = Vec::with_capacity(d.len());
    let mut starter: Option<usize> = None;
    let mut last_ccc: i32 = -1;
    for &c in &d {
        let cc = ccc(c) as i32;
        if let Some(si) = starter {
            let blocked = last_ccc != -1 && (last_ccc >= cc || last_ccc == 0);
            // last_ccc == -1 means the previous char is the starter itself.
            if !blocked {
                if let Some(comp) = compose_pair(out[si], c) {
                    out[si] = comp;
                    continue;
                }
            }
        }
        if cc == 0 {
            starter = Some(out.len());
            last_ccc = -1;
        } else {
            last_ccc = cc;
        }
        out.push(c);
    }
    out
}
