// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The Unicode the BERT tokenizer needs: three general-category predicates and canonical
//! decomposition (UAX #15 NFD: full canonical decomposition, Hangul syllables by the §3.12
//! algorithm, then the canonical ordering of combining marks).
//!
//! **Versions.** The ids this tokenizer must reproduce are the reference's (HF `tokenizers`), and
//! the reference's categories come from UnicodeData.txt 8.0.0 and its NFD from 9.0.0 — so the
//! tables in `unicode_tables.rs` are generated (by `tools/gen_unicode.py`, data only) from those
//! two releases. Whitespace and lowercase come from `core` (`char::is_whitespace`,
//! `char::to_lowercase`), as they do in the reference.

use alloc::vec::Vec;

use crate::unicode_tables::{CCC, DECOMP, DECOMP_CHARS, MARK_NONSPACING, OTHER, PUNCTUATION};

fn in_ranges(t: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    t.binary_search_by(|&(a, b)| {
        if b < c {
            core::cmp::Ordering::Less
        } else if a > c {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Equal
        }
    })
    .is_ok()
}

/// Cc, Cf or Co (UnicodeData 8.0.0).
pub fn is_other(c: char) -> bool {
    in_ranges(OTHER, c)
}

/// Mn (UnicodeData 8.0.0).
pub fn is_mark_nonspacing(c: char) -> bool {
    in_ranges(MARK_NONSPACING, c)
}

/// P* (UnicodeData 8.0.0).
pub fn is_punctuation(c: char) -> bool {
    in_ranges(PUNCTUATION, c)
}

/// Canonical_Combining_Class (UnicodeData 9.0.0).
pub fn ccc(c: char) -> u8 {
    let c = c as u32;
    match CCC.binary_search_by(|&(a, b, _)| {
        if b < c {
            core::cmp::Ordering::Less
        } else if a > c {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Equal
        }
    }) {
        Ok(i) => CCC[i].2,
        Err(_) => 0,
    }
}

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = 21 * T_COUNT;
const S_COUNT: u32 = 19 * N_COUNT;

/// Push the full canonical decomposition of `c` onto `out` (unordered).
fn decompose(c: char, out: &mut Vec<char>) {
    let cp = c as u32;
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let s = cp - S_BASE;
        let push = |out: &mut Vec<char>, v: u32| out.extend(char::from_u32(v));
        push(out, L_BASE + s / N_COUNT);
        push(out, V_BASE + (s % N_COUNT) / T_COUNT);
        if s % T_COUNT != 0 {
            push(out, T_BASE + s % T_COUNT);
        }
        return;
    }
    match DECOMP.binary_search_by_key(&cp, |&(k, _, _)| k) {
        Ok(i) => {
            let (_, off, len) = DECOMP[i];
            out.extend(DECOMP_CHARS[off as usize..off as usize + len as usize].iter().filter_map(|&v| char::from_u32(v)));
        }
        Err(_) => out.push(c),
    }
}

/// NFD of `chars`: decompose, then stably sort every run of non-starters by combining class.
pub fn nfd(chars: impl IntoIterator<Item = char>) -> Vec<char> {
    let mut out = Vec::new();
    for c in chars {
        decompose(c, &mut out);
    }
    let mut i = 0;
    while i < out.len() {
        if ccc(out[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < out.len() && ccc(out[i]) != 0 {
            i += 1;
        }
        // Insertion sort: stable, runs are short.
        for j in start + 1..i {
            let mut k = j;
            while k > start && ccc(out[k - 1]) > ccc(out[k]) {
                out.swap(k - 1, k);
                k -= 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn s(v: Vec<char>) -> String {
        v.into_iter().collect()
    }

    #[test]
    fn categories() {
        assert!(is_other('\u{7}') && is_other('\u{200B}') && is_other('\u{E000}') && is_other('\u{F8FF}'));
        assert!(is_other('\u{E001}') && is_other('\u{F0001}') && is_other('\u{10FFFD}')); // Co ranges
        assert!(!is_other('\u{8E2}')); // Cf since 9.0 — not in 8.0
        assert!(!is_other('a') && is_other('\t')); // Cc (the tokenizer exempts \t \n \r itself)
        assert!(is_mark_nonspacing('\u{301}') && !is_mark_nonspacing('\u{903}'));
        assert!(is_punctuation('¿') && is_punctuation('、') && !is_punctuation('$') && !is_punctuation('a'));
    }

    #[test]
    fn nfd_known_answers() {
        assert_eq!(s(nfd("é".chars())), "e\u{301}");
        assert_eq!(s(nfd("ǖ".chars())), "u\u{308}\u{304}");
        assert_eq!(s(nfd("한".chars())), "\u{1112}\u{1161}\u{11AB}");
        assert_eq!(s(nfd("가".chars())), "\u{1100}\u{1161}");
        // Canonical ordering: dot below (220) before acute (230) whatever the input order.
        assert_eq!(s(nfd("a\u{301}\u{323}".chars())), "a\u{323}\u{301}");
        assert_eq!(s(nfd("\u{1E69}".chars())), "s\u{323}\u{307}");
        assert_eq!(s(nfd("\u{212B}".chars())), "A\u{30A}"); // singleton
        assert_eq!(s(nfd("\u{FB01}".chars())), "\u{FB01}"); // compatibility only: untouched
    }
}
