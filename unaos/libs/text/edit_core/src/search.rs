// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Literal search: case-sensitive or case-insensitive (full lowercase mapping per char, so `İ` and `ß`
//! fold the way `char::to_lowercase` says), optionally whole-word. A small regex is a later cut (SR73).

use crate::unicode::is_word_char;
use alloc::vec::Vec;
use core::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOpts {
    pub case_insensitive: bool,
    /// The match may not continue a word on either side (a word char next to a word char at the edge).
    pub whole_word: bool,
}

/// If `needle_folded` matches `hay` starting at byte `at`, the byte where the match ends.
fn match_at(hay: &str, at: usize, needle: &str, needle_folded: &[char], ci: bool) -> Option<usize> {
    if !ci {
        return if hay[at..].starts_with(needle) { Some(at + needle.len()) } else { None };
    }
    let mut k = 0;
    for (i, c) in hay[at..].char_indices() {
        for lc in c.to_lowercase() {
            if k >= needle_folded.len() || needle_folded[k] != lc {
                return None;
            }
            k += 1;
        }
        if k == needle_folded.len() {
            return Some(at + i + c.len_utf8());
        }
    }
    None
}

/// Every non-overlapping match of `needle` in `hay`, left to right, as byte ranges on char boundaries.
pub fn find_all(hay: &str, needle: &str, opts: SearchOpts) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let folded: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let mut at = 0;
    while at < hay.len() {
        if let Some(end) = match_at(hay, at, needle, &folded, opts.case_insensitive) {
            let ok = !opts.whole_word || {
                let before = hay[..at].chars().next_back();
                let first = hay[at..end].chars().next();
                let last = hay[at..end].chars().next_back();
                let after = hay[end..].chars().next();
                let joins = |x: Option<char>, y: Option<char>| match (x, y) {
                    (Some(x), Some(y)) => is_word_char(x) && is_word_char(y),
                    _ => false,
                };
                !joins(before, first) && !joins(last, after)
            };
            if ok {
                out.push(at..end);
                at = end;
                continue;
            }
        }
        at += hay[at..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    out
}
