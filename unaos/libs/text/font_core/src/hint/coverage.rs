//! The OpenType-feature side of the auto-hinter's style coverage — what FreeType gets from HarfBuzz
//! (`afshaper.c`: `hb_ot_layout_collect_lookups`, `hb_ot_layout_lookup_collect_glyphs`,
//! `hb_ot_layout_lookup_would_substitute`, and shaping a blue character with a style's feature), done on
//! font_core's own GSUB/GPOS readers:
//!
//! - [`gsub_outputs`]: every glyph a set of GSUB lookups can produce (types 1–4, 8; contextual 5/6 through the
//!   lookups they call, recursively);
//! - [`gpos_inputs`]: every glyph a set of GPOS lookups positions (the first coverage, pair second glyphs, mark
//!   and base coverages);
//! - [`would_substitute`]: whether a lookup replaces a single glyph on its own;
//! - [`apply_single`]: one glyph through a feature's lookups (single, first alternate, multiple), the glyph a
//!   feature style measures its blue zones on.

use crate::layout::{coverage_index, LayoutTable};
use crate::reader::u16_at;
use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

fn sub(d: &[u8], off_at: usize) -> Option<&[u8]> {
    let off = u16_at(d, off_at)? as usize;
    if off == 0 {
        return None;
    }
    d.get(off..)
}

/// Every glyph id a Coverage table lists.
fn coverage_glyphs(cov: &[u8], out: &mut Vec<u16>) {
    match u16_at(cov, 0) {
        Some(1) => {
            let n = u16_at(cov, 2).unwrap_or(0) as usize;
            out.extend((0..n).filter_map(|i| u16_at(cov, 4 + 2 * i)));
        }
        Some(2) => {
            let n = u16_at(cov, 2).unwrap_or(0) as usize;
            for i in 0..n {
                let (Some(s), Some(e)) = (u16_at(cov, 4 + 6 * i), u16_at(cov, 6 + 6 * i)) else { break };
                if e >= s && e - s < 0x8000 {
                    out.extend(s..=e);
                }
            }
        }
        _ => {}
    }
}

/// Every glyph a ClassDef table assigns to a non-zero class.
fn classdef_glyphs(cd: &[u8], out: &mut Vec<u16>) {
    match u16_at(cd, 0) {
        Some(1) => {
            let start = u16_at(cd, 2).unwrap_or(0);
            let n = u16_at(cd, 4).unwrap_or(0) as usize;
            for i in 0..n {
                if u16_at(cd, 6 + 2 * i).unwrap_or(0) != 0 {
                    out.push(start.wrapping_add(i as u16));
                }
            }
        }
        Some(2) => {
            let n = u16_at(cd, 2).unwrap_or(0) as usize;
            for i in 0..n {
                let (Some(s), Some(e), Some(c)) = (u16_at(cd, 4 + 6 * i), u16_at(cd, 6 + 6 * i), u16_at(cd, 8 + 6 * i)) else { break };
                if c != 0 && e >= s && e - s < 0x8000 {
                    out.extend(s..=e);
                }
            }
        }
        _ => {}
    }
}

/// The lookup indices a contextual (GSUB 5/6, GPOS 7/8) subtable calls.
fn nested_lookups(st: &[u8], chained: bool) -> Vec<u16> {
    let mut out = Vec::new();
    let recs = |d: &[u8], at: usize, n: usize, out: &mut Vec<u16>| {
        for k in 0..n {
            if let Some(l) = u16_at(d, at + 4 * k + 2) {
                out.push(l);
            }
        }
    };
    match u16_at(st, 0) {
        Some(f @ (1 | 2)) => {
            // (Chain)SubRuleSet / SubClassSet arrays, each rule ending in its lookup records
            let n = u16_at(st, if f == 1 { 4 } else if chained { 10 } else { 6 }).unwrap_or(0) as usize;
            let base = if f == 1 { 6 } else if chained { 12 } else { 8 };
            for i in 0..n {
                let Some(set) = sub(st, base + 2 * i) else { continue };
                let nr = u16_at(set, 0).unwrap_or(0) as usize;
                for r in 0..nr {
                    let Some(rule) = sub(set, 2 + 2 * r) else { continue };
                    if chained {
                        let bt = u16_at(rule, 0).unwrap_or(0) as usize;
                        let mut p = 2 + 2 * bt;
                        let inp = u16_at(rule, p).unwrap_or(0) as usize;
                        p += 2 + 2 * inp.saturating_sub(1);
                        let la = u16_at(rule, p).unwrap_or(0) as usize;
                        p += 2 + 2 * la;
                        let nl = u16_at(rule, p).unwrap_or(0) as usize;
                        recs(rule, p + 2, nl, &mut out);
                    } else {
                        let inp = u16_at(rule, 0).unwrap_or(0) as usize;
                        let nl = u16_at(rule, 2).unwrap_or(0) as usize;
                        recs(rule, 4 + 2 * inp.saturating_sub(1), nl, &mut out);
                    }
                }
            }
        }
        Some(3) => {
            if chained {
                let bt = u16_at(st, 2).unwrap_or(0) as usize;
                let mut p = 4 + 2 * bt;
                let inp = u16_at(st, p).unwrap_or(0) as usize;
                p += 2 + 2 * inp;
                let la = u16_at(st, p).unwrap_or(0) as usize;
                p += 2 + 2 * la;
                let nl = u16_at(st, p).unwrap_or(0) as usize;
                recs(st, p + 2, nl, &mut out);
            } else {
                let inp = u16_at(st, 2).unwrap_or(0) as usize;
                let nl = u16_at(st, 4).unwrap_or(0) as usize;
                recs(st, 6 + 2 * inp, nl, &mut out);
            }
        }
        _ => {}
    }
    out
}

/// `hb_ot_layout_lookup_collect_glyphs(…, glyphs_output)` over GSUB `lookups`.
pub fn gsub_outputs(t: &LayoutTable, lookups: &[u16]) -> BTreeSet<u16> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<u16> = lookups.to_vec();
    while let Some(li) = stack.pop() {
        if !seen.insert(li) {
            continue;
        }
        let Some(l) = t.lookup(li) else { continue };
        for st in &l.subtables {
            match l.kind {
                1 => {
                    let Some(cov) = sub(st, 2) else { continue };
                    match u16_at(st, 0) {
                        Some(1) => {
                            let delta = u16_at(st, 4).unwrap_or(0);
                            let mut g = Vec::new();
                            coverage_glyphs(cov, &mut g);
                            out.extend(g.into_iter().map(|x| x.wrapping_add(delta)));
                        }
                        Some(2) => {
                            let n = u16_at(st, 4).unwrap_or(0) as usize;
                            out.extend((0..n).filter_map(|i| u16_at(st, 6 + 2 * i)));
                        }
                        _ => {}
                    }
                }
                2 | 3 => {
                    let n = u16_at(st, 4).unwrap_or(0) as usize;
                    for i in 0..n {
                        let Some(seq) = sub(st, 6 + 2 * i) else { continue };
                        let k = u16_at(seq, 0).unwrap_or(0) as usize;
                        out.extend((0..k).filter_map(|j| u16_at(seq, 2 + 2 * j)));
                    }
                }
                4 => {
                    let n = u16_at(st, 4).unwrap_or(0) as usize;
                    for i in 0..n {
                        let Some(set) = sub(st, 6 + 2 * i) else { continue };
                        let k = u16_at(set, 0).unwrap_or(0) as usize;
                        for j in 0..k {
                            if let Some(lig) = sub(set, 2 + 2 * j) {
                                if let Some(g) = u16_at(lig, 0) {
                                    out.push(g);
                                }
                            }
                        }
                    }
                }
                5 | 6 => stack.extend(nested_lookups(st, l.kind == 6)),
                8 => {
                    let bt = u16_at(st, 4).unwrap_or(0) as usize;
                    let la = u16_at(st, 6 + 2 * bt).unwrap_or(0) as usize;
                    let p = 8 + 2 * bt + 2 * la;
                    let n = u16_at(st, p).unwrap_or(0) as usize;
                    out.extend((0..n).filter_map(|i| u16_at(st, p + 2 + 2 * i)));
                }
                _ => {}
            }
        }
    }
    out.into_iter().collect()
}

/// The input glyphs of GPOS `lookups` (`hb_ot_layout_lookup_collect_glyphs(…, glyphs_input)`, the common
/// subtable shapes).
pub fn gpos_inputs(t: &LayoutTable, lookups: &[u16]) -> BTreeSet<u16> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<u16> = lookups.to_vec();
    while let Some(li) = stack.pop() {
        if !seen.insert(li) {
            continue;
        }
        let Some(l) = t.lookup(li) else { continue };
        for st in &l.subtables {
            match l.kind {
                1 | 3 => {
                    if let Some(c) = sub(st, 2) {
                        coverage_glyphs(c, &mut out);
                    }
                }
                2 => {
                    if let Some(c) = sub(st, 2) {
                        coverage_glyphs(c, &mut out);
                    }
                    match u16_at(st, 0) {
                        Some(1) => {
                            let (vf1, vf2) = (u16_at(st, 4).unwrap_or(0), u16_at(st, 6).unwrap_or(0));
                            let rec = 2 + 2 * (vf1.count_ones() + vf2.count_ones()) as usize;
                            let n = u16_at(st, 8).unwrap_or(0) as usize;
                            for i in 0..n {
                                let Some(set) = sub(st, 10 + 2 * i) else { continue };
                                let k = u16_at(set, 0).unwrap_or(0) as usize;
                                out.extend((0..k).filter_map(|j| u16_at(set, 2 + rec * j)));
                            }
                        }
                        Some(2) => {
                            if let Some(cd2) = sub(st, 10) {
                                classdef_glyphs(cd2, &mut out);
                            }
                        }
                        _ => {}
                    }
                }
                4..=6 => {
                    for at in [2usize, 4] {
                        if let Some(c) = sub(st, at) {
                            coverage_glyphs(c, &mut out);
                        }
                    }
                }
                7 | 8 => stack.extend(nested_lookups(st, l.kind == 8)),
                _ => {}
            }
        }
    }
    out.into_iter().collect()
}

/// `hb_ot_layout_lookup_would_substitute(face, lookup, &gid, 1, zero_context = true)`.
pub fn would_substitute(t: &LayoutTable, lookup: u16, gid: u16) -> bool {
    let Some(l) = t.lookup(lookup) else { return false };
    l.subtables.iter().any(|st| match l.kind {
        1..=3 => sub(st, 2).and_then(|c| coverage_index(c, gid)).is_some(),
        4 => {
            let Some(ci) = sub(st, 2).and_then(|c| coverage_index(c, gid)) else { return false };
            let Some(set) = sub(st, 6 + 2 * ci as usize) else { return false };
            let k = u16_at(set, 0).unwrap_or(0) as usize;
            (0..k).any(|j| sub(set, 2 + 2 * j).and_then(|lig| u16_at(lig, 2)) == Some(1))
        }
        _ => false,
    })
}

/// A single glyph through GSUB `lookups` in order: single substitution, the first alternate, or a multiple
/// substitution's sequence (each output glyph continues through the later lookups).
pub fn apply_single(t: &LayoutTable, lookups: &[u16], gid: u16) -> Vec<u16> {
    let mut glyphs = vec![gid];
    for &li in lookups {
        let Some(l) = t.lookup(li) else { continue };
        let mut next = Vec::with_capacity(glyphs.len());
        for &g in &glyphs {
            let mut rep: Option<Vec<u16>> = None;
            for st in &l.subtables {
                let Some(ci) = sub(st, 2).and_then(|c| coverage_index(c, g)) else { continue };
                rep = match (l.kind, u16_at(st, 0)) {
                    (1, Some(1)) => Some(vec![g.wrapping_add(u16_at(st, 4).unwrap_or(0))]),
                    (1, Some(2)) => u16_at(st, 6 + 2 * ci as usize).map(|x| vec![x]),
                    (2, _) => sub(st, 6 + 2 * ci as usize).map(|seq| {
                        let k = u16_at(seq, 0).unwrap_or(0) as usize;
                        (0..k).filter_map(|j| u16_at(seq, 2 + 2 * j)).collect()
                    }),
                    (3, _) => sub(st, 6 + 2 * ci as usize).and_then(|set| u16_at(set, 2)).map(|x| vec![x]),
                    _ => None,
                };
                if rep.is_some() {
                    break;
                }
            }
            match rep {
                Some(r) => next.extend(r),
                None => next.push(g),
            }
        }
        glyphs = next;
    }
    glyphs
}
