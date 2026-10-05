//! UAX #15 normalization forms NFD and NFC (Unicode 17.0.0): canonical decomposition (recursive, Hangul
//! algorithmic), the canonical ordering algorithm by Canonical_Combining_Class, and canonical composition with
//! the blocking rule, excluding Full_Composition_Exclusion. Tables in `ucd::norm` are generated from
//! UnicodeData.txt and DerivedNormalizationProps.txt. `tests/normalize.rs` runs NormalizationTest-17.0.0.txt.

use crate::ucd::{self, lookup};
use alloc::string::String;
use alloc::vec::Vec;

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// Canonical_Combining_Class.
pub fn ccc(c: char) -> u8 {
    lookup(ucd::norm::CCC, c as u32).unwrap_or(0)
}

/// One level of the canonical decomposition mapping (non-Hangul).
pub fn decomposition(c: char) -> Option<&'static [u32]> {
    let t = ucd::norm::DECOMP;
    let i = t.binary_search_by_key(&(c as u32), |r| r.0).ok()?;
    let (_, s, n) = t[i];
    Some(&ucd::norm::DECOMP_DATA[s as usize..s as usize + n as usize])
}

fn decompose_into(cp: u32, out: &mut Vec<char>) {
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let si = cp - S_BASE;
        out.push(char::from_u32(L_BASE + si / N_COUNT).unwrap());
        out.push(char::from_u32(V_BASE + (si % N_COUNT) / T_COUNT).unwrap());
        if si % T_COUNT != 0 {
            out.push(char::from_u32(T_BASE + si % T_COUNT).unwrap());
        }
        return;
    }
    let Some(c) = char::from_u32(cp) else { return };
    match decomposition(c) {
        Some(d) => {
            for &x in d {
                decompose_into(x, out);
            }
        }
        None => out.push(c),
    }
}

/// Canonical ordering: stable sort of each run of non-starters by ccc.
fn reorder(v: &mut [char]) {
    let mut i = 0;
    while i < v.len() {
        if ccc(v[i]) == 0 {
            i += 1;
            continue;
        }
        let s = i;
        while i < v.len() && ccc(v[i]) != 0 {
            i += 1;
        }
        v[s..i].sort_by_key(|&c| ccc(c));
    }
}

/// NFD as chars.
pub fn nfd_chars(s: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        decompose_into(c as u32, &mut out);
    }
    reorder(&mut out);
    out
}

pub fn nfd(s: &str) -> String {
    nfd_chars(s).into_iter().collect()
}

/// The primary composite of a canonical pair, if any.
pub fn compose_pair(a: char, b: char) -> Option<char> {
    let (a, b) = (a as u32, b as u32);
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return char::from_u32(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a) && (a - S_BASE) % T_COUNT == 0 && (T_BASE + 1..T_BASE + T_COUNT).contains(&b) {
        return char::from_u32(a + (b - T_BASE));
    }
    let t = ucd::norm::COMPOSE;
    let i = t.binary_search_by(|r| (r.0, r.1).cmp(&(a, b))).ok()?;
    char::from_u32(t[i].2)
}

/// Canonical composition (UAX #15 §1.3 / D117) over an NFD sequence, in place.
pub fn compose(v: &mut Vec<char>) {
    if v.is_empty() {
        return;
    }
    let mut out: Vec<char> = Vec::with_capacity(v.len());
    let mut starter: Option<usize> = None; // index in `out`
    let mut last_ccc: i32 = -1;
    for &c in v.iter() {
        let cc = ccc(c) as i32;
        if let Some(si) = starter {
            // Blocked if a character between the starter and c has ccc 0 or ccc >= cc.
            let blocked = last_ccc != -1 && (last_ccc == 0 || last_ccc >= cc);
            if !blocked {
                if let Some(p) = compose_pair(out[si], c) {
                    out[si] = p;
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
    *v = out;
}

pub fn nfc_chars(s: &str) -> Vec<char> {
    let mut v = nfd_chars(s);
    compose(&mut v);
    v
}

pub fn nfc(s: &str) -> String {
    nfc_chars(s).into_iter().collect()
}
