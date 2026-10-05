//! The complex-script shapers the shaping pipeline calls between GSUB stages:
//!
//! - **Arabic joining** (Unicode chapter 9.2 + ArabicShaping.txt): every character's Joining_Type (C as D,
//!   transparent T skipped) decides isol/fina/medi/init, delivered as per-glyph feature masks.
//! - **Indic (Devanagari)**: categories from Indic_Syllabic_Category / Indic_Positional_Category, the syllable
//!   grammar (consonant, vowel, standalone, broken clusters with a dotted circle), the initial reordering (base
//!   consonant search with below/post-base forms tested against the font, reph detection through `rphf`, pre-base
//!   matras to the front, positions sorted), basic-feature masks (rphf, half, blwf, abvf, pstf) and the final
//!   reordering after the basic features (pre-base matra next to the last unformed halant, reph to its final
//!   position) — the model Microsoft's Indic specification and HarfBuzz (and so Chromium) implement.
//! - **Thai/Lao**: SARA AM (U+0E33 / U+0EB3) decomposed to NIKHAHIT + SARA AA with the NIKHAHIT moved before
//!   preceding above-base marks.

use crate::layout::{class_of, coverage_index, sub, Tag};
use crate::ot::GlyphInfo;
use crate::reader::u16_at;
use crate::shape::Plan;
use crate::ucd::{self, Gc, Ipc, Isc, Jt};
use crate::Font;
use alloc::vec::Vec;

// ------------------------------------------------------------------------------------------------------ Thai

fn thai_above(cp: u32) -> bool {
    matches!(cp, 0x0E31 | 0x0E34..=0x0E37 | 0x0E47..=0x0E4E | 0x0EB1 | 0x0EB4..=0x0EB7 | 0x0EBB | 0x0EC8..=0x0ECD)
}

pub(crate) fn thai_preprocess(chars: &mut Vec<(u32, usize)>) {
    let mut out: Vec<(u32, usize)> = Vec::with_capacity(chars.len() + 2);
    let mut remap: Option<(usize, usize)> = None;
    for &(cp, cl0) in chars.iter() {
        let cl = match remap {
            Some((from, to)) if from == cl0 => to,
            _ => cl0,
        };
        if cp != 0x0E33 && cp != 0x0EB3 {
            out.push((cp, cl));
            continue;
        }
        let (nikhahit, sara_aa) = if cp == 0x0E33 { (0x0E4D, 0x0E32) } else { (0x0ECD, 0x0EB2) };
        // Move NIKHAHIT back over the above-base marks before it.
        let mut k = out.len();
        while k > 0 && thai_above(out[k - 1].0) {
            k -= 1;
        }
        // NIKHAHIT is combining: the decomposition merges into the preceding cluster (and so do the characters
        // after it that shared the SARA AM's cluster).
        let from = k.saturating_sub(1);
        let ncl = out[from..].iter().map(|x| x.1).chain(core::iter::once(cl)).min().unwrap();
        out.insert(k, (nikhahit, ncl));
        for x in out[from..].iter_mut() {
            x.1 = ncl;
        }
        out.push((sara_aa, ncl));
        remap = Some((cl0, ncl));
    }
    *chars = out;
}

/// Insert U+25CC inside Indic_Shaping_Invalid_Cluster sequences (before their last character).
pub(crate) fn vowel_constraints(chars: &mut Vec<(u32, usize)>) {
    if !chars.iter().any(|c| (0x0900..0x0DFF).contains(&c.0) || (0x1000..0x109F).contains(&c.0) || (0x1780..0x17FF).contains(&c.0)) {
        return;
    }
    let mut out: Vec<(u32, usize)> = Vec::with_capacity(chars.len() + 2);
    let mut i = 0;
    while i < chars.len() {
        let hit = ucd::vowel_constraints::INVALID_CLUSTERS
            .iter()
            .find(|q| i + q.len() <= chars.len() && q.iter().zip(&chars[i..]).all(|(a, b)| *a == b.0));
        if let Some(q) = hit {
            let n = q.len();
            out.extend_from_slice(&chars[i..i + n - 1]);
            out.push((0x25CC, chars[i + n - 1].1));
            i += n - 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    *chars = out;
}

/// Hebrew: patah/qamats + sheva/hiriq + meteg/below — the meteg goes before the sheva/hiriq (traditional order).
pub(crate) fn reorder_marks_hebrew(out: &mut [(u32, usize)], mcc: &[u8], start: usize, end: usize) {
    for i in start + 2..end {
        let (c0, c1, c2) = (mcc[i - 2], mcc[i - 1], mcc[i]);
        if (c0 == 20 || c0 == 21) && (c1 == 22 || c1 == 23) && (c2 == 25 || c2 == 220) {
            let cl = out[i - 1].1.min(out[i].1);
            out.swap(i - 1, i);
            out[i - 1].1 = cl;
            out[i].1 = cl;
            break;
        }
    }
}

/// Arabic: modifier combining marks (UTR #53 MCM) at ccc 220/230 move to the front of the mark sequence.
pub(crate) fn reorder_marks_arabic(out: &mut [(u32, usize)], mcc: &mut [u8], start: usize, end: usize) {
    const MCM: [u32; 14] = [0x0654, 0x0655, 0x0658, 0x06DC, 0x06E3, 0x06E7, 0x06E8, 0x08CA, 0x08CB, 0x08CD, 0x08CE, 0x08CF, 0x08D3, 0x08F3];
    let mut start = start;
    let mut i = start;
    for cc in [220u8, 230] {
        while i < end && mcc[i] < cc {
            i += 1;
        }
        if i == end {
            break;
        }
        if mcc[i] > cc {
            continue;
        }
        let mut j = i;
        while j < end && mcc[j] == cc && MCM.contains(&out[j].0) {
            j += 1;
        }
        if i == j {
            continue;
        }
        let m = out[start..j].iter().map(|x| x.1).min().unwrap();
        for x in out[start..j].iter_mut() {
            x.1 = m;
        }
        out[start..j].rotate_left(i - start);
        mcc[start..j].rotate_left(i - start);
        let new_start = start + j - i;
        let new_cc = if cc == 220 { 25 } else { 26 };
        while start < new_start {
            mcc[start] = new_cc;
            start += 1;
        }
        i = j;
    }
}

// ------------------------------------------------------------------------------------------------------ Arabic

pub(crate) fn arabic_setup_masks(plan: &Plan, info: &mut [GlyphInfo]) {
    let jt = |g: &GlyphInfo| -> Jt {
        match char::from_u32(g.cp).map(ucd::joining_type).unwrap_or(Jt::U) {
            Jt::C => Jt::D,
            j => j,
        }
    };
    let types: Vec<Jt> = info.iter().map(jt).collect();
    let feats = [*b"isol", *b"fina", *b"medi", *b"init"];
    let masks: Vec<u32> = feats.iter().map(|&t| plan.mask(t)).collect();
    let n = info.len();
    for i in 0..n {
        let t = types[i];
        if matches!(t, Jt::T | Jt::U) {
            continue;
        }
        let prev = (0..i).rev().map(|k| types[k]).find(|&k| k != Jt::T);
        let next = (i + 1..n).map(|k| types[k]).find(|&k| k != Jt::T);
        let jp = matches!(t, Jt::R | Jt::D) && matches!(prev, Some(Jt::L | Jt::D));
        let jn = matches!(t, Jt::L | Jt::D) && matches!(next, Some(Jt::R | Jt::D));
        let form = match (jp, jn) {
            (true, true) => 2,
            (true, false) => 1,
            (false, true) => 3,
            (false, false) => 0,
        };
        info[i].mask |= masks[form];
    }
}

// ------------------------------------------------------------------------------------------------------ Indic

// Categories.
const X: u8 = 0;
const C: u8 = 1;
const V: u8 = 2;
const N: u8 = 3;
const H: u8 = 4;
const ZWNJ: u8 = 5;
const ZWJ: u8 = 6;
const M: u8 = 7;
const SM: u8 = 8;
const A: u8 = 9;
const PLACEHOLDER: u8 = 10;
const DOTTEDCIRCLE: u8 = 11;
const RA: u8 = 12;
const SYMBOL: u8 = 13;

// Positions (Microsoft / HarfBuzz order).
const POS_START: u8 = 0;
const POS_RA_TO_BECOME_REPH: u8 = 1;
const POS_PRE_M: u8 = 2;
const POS_PRE_C: u8 = 3;
const POS_BASE_C: u8 = 4;
const POS_BELOW_C: u8 = 8;
const POS_AFTER_SUB: u8 = 9;
const POS_POST_C: u8 = 11;
const POS_SMVD: u8 = 13;
const POS_END: u8 = 14;

// Syllable kinds (low 4 bits of `syllable`).
const K_CONSONANT: u16 = 0;
const K_VOWEL: u16 = 1;
const K_STANDALONE: u16 = 2;
const K_SYMBOL: u16 = 3;
const K_BROKEN: u16 = 4;
const K_OTHER: u16 = 5;

const VIRAMA: u32 = 0x094D;

fn is_consonant(c: u8) -> bool {
    matches!(c, C | RA | V | PLACEHOLDER | DOTTEDCIRCLE)
}
fn is_joiner(c: u8) -> bool {
    matches!(c, ZWJ | ZWNJ)
}

fn indic_category(cp: u32) -> (u8, u8) {
    let Some(ch) = char::from_u32(cp) else { return (X, POS_END) };
    if cp == 0x200C {
        return (ZWNJ, POS_END);
    }
    if cp == 0x200D {
        return (ZWJ, POS_END);
    }
    if cp == 0x25CC {
        return (DOTTEDCIRCLE, POS_BASE_C);
    }
    match ucd::indic_syllabic_category(ch) {
        Isc::Consonant | Isc::Consonant_Dead => {
            if cp == 0x0930 {
                (RA, POS_BASE_C)
            } else {
                (C, POS_BASE_C)
            }
        }
        Isc::Vowel_Independent => (V, POS_BASE_C),
        Isc::Vowel_Dependent => {
            let p = match ucd::indic_positional_category(ch) {
                Ipc::Left => POS_PRE_M,
                _ => POS_AFTER_SUB,
            };
            (M, p)
        }
        Isc::Nukta => (N, POS_END),
        Isc::Virama => (H, POS_END),
        Isc::Bindu | Isc::Visarga | Isc::Syllable_Modifier => (SM, POS_SMVD),
        Isc::Cantillation_Mark | Isc::Tone_Mark | Isc::Gemination_Mark => (A, POS_SMVD),
        Isc::Avagraha => (SYMBOL, POS_END),
        Isc::Consonant_Placeholder => (PLACEHOLDER, POS_BASE_C),
        _ => (X, POS_END),
    }
}

/// Syllable grammar (HarfBuzz's indic machine, the parts Devanagari uses). Returns (end, kind) from `i`.
fn match_syllable(cat: &[u8], i: usize) -> (usize, u16) {
    let n = cat.len();
    let at = |k: usize| if k < n { cat[k] } else { 255 };
    let nukta = |mut k: usize| {
        let mut c = 0;
        while c < 2 && at(k) == N {
            k += 1;
            c += 1;
        }
        k
    };
    let cn = |k: usize| -> Option<usize> {
        if !matches!(at(k), C | RA) {
            return None;
        }
        let mut k = k + 1;
        if at(k) == ZWJ {
            k += 1;
        }
        Some(nukta(k))
    };
    let halant_group = |k: usize| -> Option<usize> {
        let mut k = k;
        if is_joiner(at(k)) && at(k + 1) == H {
            k += 1;
        }
        if at(k) != H {
            return None;
        }
        k += 1;
        if at(k) == ZWJ {
            k += 1;
            if at(k) == N {
                k += 1;
            }
        }
        Some(k)
    };
    let syllable_tail = |k: usize| -> usize {
        let mut k = k;
        let s = k;
        let mut j = k;
        if is_joiner(at(j)) {
            j += 1;
        }
        if at(j) == SM {
            j += 1;
            if at(j) == SM {
                j += 1;
            }
            if at(j) == ZWNJ {
                j += 1;
            }
            k = j;
        } else {
            k = s;
        }
        while at(k) == A {
            k += 1;
        }
        k
    };
    let complex_tail = |k: usize| -> usize {
        let mut k = k;
        let mut reps = 0;
        while reps < 4 {
            let Some(h) = halant_group(k) else { break };
            let Some(c) = cn(h) else { break };
            k = c;
            reps += 1;
        }
        // final_halant_group | matra_group*
        let fin = if at(k) == H && at(k + 1) == ZWNJ { Some(k + 2) } else { halant_group(k) };
        if let Some(f) = fin {
            k = f;
        } else {
            loop {
                let mut j = k;
                while is_joiner(at(j)) {
                    j += 1;
                }
                if at(j) != M {
                    break;
                }
                j += 1;
                if at(j) == N {
                    j += 1;
                }
                if at(j) == H {
                    j += 1;
                }
                k = j;
            }
        }
        syllable_tail(k)
    };
    let reph = |k: usize| if at(k) == RA && at(k + 1) == H { Some(k + 2) } else { None };
    let mut best = (i + 1, K_OTHER);
    let consider = |end: usize, kind: u16, best: &mut (usize, u16)| {
        if end > best.0 || (end == best.0 && best.1 == K_OTHER && end > i) {
            *best = (end, kind);
        }
    };
    if let Some(c) = cn(i) {
        consider(complex_tail(c), K_CONSONANT, &mut best);
    }
    for start in [Some(i), reph(i)].into_iter().flatten() {
        if at(start) == V {
            let k = nukta(start + 1);
            let e = complex_tail(k).max(if at(k) == ZWJ { k + 1 } else { k });
            consider(e, K_VOWEL, &mut best);
        }
    }
    {
        let s = if at(i) == PLACEHOLDER { Some(i + 1) } else { None };
        let s2 = [Some(i), reph(i)].into_iter().flatten().find(|&k| at(k) == DOTTEDCIRCLE).map(|k| k + 1);
        for st in [s, s2].into_iter().flatten() {
            consider(complex_tail(nukta(st)), K_STANDALONE, &mut best);
        }
    }
    if at(i) == SYMBOL {
        let k = if at(i + 1) == N { i + 2 } else { i + 1 };
        consider(syllable_tail(k), K_SYMBOL, &mut best);
    }
    {
        // broken: reph? n? complex_tail, non-empty and starting with an Indic mark.
        let s = reph(i).unwrap_or(i);
        let e = complex_tail(nukta(s));
        if e > i && matches!(cat[i], N | H | M | SM | A | ZWJ | ZWNJ | RA) {
            consider(e, K_BROKEN, &mut best);
        }
    }
    best
}

/// Lookup indices of `feature` for the plan's script.
fn feature_lookups(font: &Font, plan: &Plan, feature: Tag) -> Option<Vec<u16>> {
    font.gsub.and_then(|g| g.0.feature_lookups(&plan.script_tags, feature))
}

/// Whether any lookup of `lookups` would substitute exactly the glyph sequence `glyphs` (zero context).
fn would_substitute(font: &Font, lookups: &[u16], glyphs: &[u16]) -> bool {
    let Some(gsub) = font.gsub else { return false };
    for &li in lookups {
        let Some(l) = gsub.0.lookup(li) else { continue };
        for st in &l.subtables {
            if would_apply(l.kind, st, glyphs) {
                return true;
            }
        }
    }
    false
}

fn would_apply(kind: u16, st: &[u8], g: &[u16]) -> bool {
    (|| -> Option<bool> {
        let n = g.len();
        match kind {
            1..=3 => Some(n == 1 && coverage_index(sub(st, 0, 2)?, g[0]).is_some()),
            4 => {
                let ci = coverage_index(sub(st, 0, 2)?, g[0])?;
                let set = sub(st, 0, 6 + 2 * ci as usize)?;
                for k in 0..u16_at(set, 0)? as usize {
                    let lig = sub(set, 0, 2 + 2 * k)?;
                    if u16_at(lig, 2)? as usize == n && (1..n).all(|j| u16_at(lig, 4 + 2 * (j - 1)) == Some(g[j])) {
                        return Some(true);
                    }
                }
                Some(false)
            }
            5 | 6 => {
                let fmt = u16_at(st, 0)?;
                let chain = kind == 6;
                match (fmt, chain) {
                    (3, false) => {
                        let gc = u16_at(st, 2)? as usize;
                        Some(gc == n && (0..n).all(|j| sub(st, 0, 6 + 2 * j).and_then(|c| coverage_index(c, g[j])).is_some()))
                    }
                    (3, true) => {
                        let nb = u16_at(st, 2)? as usize;
                        let in_at = 4 + 2 * nb;
                        let ni = u16_at(st, in_at)? as usize;
                        let nl = u16_at(st, in_at + 2 + 2 * ni)? as usize;
                        Some(nb == 0 && nl == 0 && ni == n
                            && (0..n).all(|j| sub(st, 0, in_at + 2 + 2 * j).and_then(|c| coverage_index(c, g[j])).is_some()))
                    }
                    (1, _) | (2, _) => {
                        let ci = coverage_index(sub(st, 0, 2)?, g[0])?;
                        let (cd, set) = if fmt == 1 {
                            (None, sub(st, 0, 6 + 2 * ci as usize)?)
                        } else if !chain {
                            let cd = sub(st, 0, 4)?;
                            (Some(cd), sub(st, 0, 8 + 2 * class_of(cd, g[0]) as usize)?)
                        } else {
                            let cd = sub(st, 0, 6)?;
                            (Some(cd), sub(st, 0, 12 + 2 * class_of(cd, g[0]) as usize)?)
                        };
                        let val = |x: u16| cd.map_or(x, |c| class_of(c, x));
                        for r in 0..u16_at(set, 0)? as usize {
                            let rule = sub(set, 0, 2 + 2 * r)?;
                            let (cnt_at, seq_at) = if chain {
                                let nb = u16_at(rule, 0)? as usize;
                                if nb != 0 {
                                    continue;
                                }
                                (2usize, 4usize)
                            } else {
                                (0, 4)
                            };
                            let gc = u16_at(rule, cnt_at)? as usize;
                            if gc != n {
                                continue;
                            }
                            if chain && u16_at(rule, seq_at + 2 * (gc - 1))? != 0 {
                                continue;
                            }
                            if (1..n).all(|j| u16_at(rule, seq_at + 2 * (j - 1)) == Some(val(g[j]))) {
                                return Some(true);
                            }
                        }
                        Some(false)
                    }
                    _ => Some(false),
                }
            }
            _ => Some(false),
        }
    })()
    .unwrap_or(false)
}

pub(crate) fn indic_setup(font: &Font, _plan: &Plan, info: &mut Vec<GlyphInfo>) {
    for g in info.iter_mut() {
        let (c, p) = indic_category(g.cp);
        g.cat = c;
        g.ipos = p;
    }
    let cats: Vec<u8> = info.iter().map(|g| g.cat).collect();
    let mut serial = 1u16;
    let mut i = 0;
    let mut out: Vec<GlyphInfo> = Vec::with_capacity(info.len() + 4);
    while i < info.len() {
        let (end, kind) = match_syllable(&cats, i);
        let syl = (serial << 4) | kind;
        serial = if serial >= 0x0FFF { 1 } else { serial + 1 };
        let s = i;
        if kind == K_BROKEN && font.glyph_index('\u{25CC}') != 0 {
            // Insert a dotted circle at the start of the broken cluster (a Ra+H reph does not count as Repha).
            let mut dc = info[s];
            dc.cp = 0x25CC;
            dc.cat = DOTTEDCIRCLE;
            dc.ipos = POS_BASE_C;
            dc.ignorable = 0;
            dc.syllable = syl;
            out.push(dc);
        }
        for g in &info[s..end] {
            out.push(GlyphInfo { syllable: syl, ..*g });
        }
        i = end;
    }
    *info = out;
}

fn syllable_ranges(info: &[GlyphInfo]) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut s = 0;
    for k in 1..=info.len() {
        if k == info.len() || info[k].syllable != info[s].syllable {
            v.push((s, k));
            s = k;
        }
    }
    v
}

fn merge_clusters(info: &mut [GlyphInfo], s: usize, e: usize) {
    crate::ot::merge_clusters(info, s, e)
}

struct IndicFeatures {
    rphf: Option<Vec<u16>>,
    blwf: Vec<u16>,
    pstf: Vec<u16>,
    pref: Vec<u16>,
    vatu: Vec<u16>,
}

impl IndicFeatures {
    fn load(font: &Font, plan: &Plan) -> Self {
        IndicFeatures {
            rphf: feature_lookups(font, plan, *b"rphf"),
            blwf: feature_lookups(font, plan, *b"blwf").unwrap_or_default(),
            pstf: feature_lookups(font, plan, *b"pstf").unwrap_or_default(),
            pref: feature_lookups(font, plan, *b"pref").unwrap_or_default(),
            vatu: feature_lookups(font, plan, *b"vatu").unwrap_or_default(),
        }
    }
}

fn consonant_position(font: &Font, f: &IndicFeatures, consonant: u16, virama: u16) -> u8 {
    let a = [virama, consonant];
    let b = [consonant, virama];
    let ws = |l: &[u16]| would_substitute(font, l, &a) || would_substitute(font, l, &b);
    if ws(&f.blwf) || ws(&f.vatu) {
        POS_BELOW_C
    } else if ws(&f.pstf) || ws(&f.pref) {
        POS_POST_C
    } else {
        POS_BASE_C
    }
}

pub(crate) fn indic_initial_reordering(font: &Font, plan: &Plan, info: &mut [GlyphInfo]) {
    let f = IndicFeatures::load(font, plan);
    let virama = font.glyph_index(char::from_u32(VIRAMA).unwrap());
    if virama != 0 {
        for g in info.iter_mut() {
            if g.ipos == POS_BASE_C && matches!(g.cat, C | RA) {
                g.ipos = consonant_position(font, &f, g.glyph, virama);
            }
        }
    }
    for (s, e) in syllable_ranges(info) {
        let kind = info[s].syllable & 0xF;
        if matches!(kind, K_CONSONANT | K_VOWEL | K_STANDALONE | K_BROKEN) {
            reorder_consonant_syllable(font, plan, &f, info, s, e);
        }
    }
}

fn reorder_consonant_syllable(font: &Font, plan: &Plan, f: &IndicFeatures, info: &mut [GlyphInfo], start: usize, end: usize) {
    // 1. Base consonant (BASE_POS_LAST), with reph detection (REPH_MODE_IMPLICIT).
    let mut base = end;
    let mut has_reph = false;
    let mut limit = start;
    if let Some(rphf) = &f.rphf {
        if start + 3 <= end && !is_joiner(info[start + 2].cat) {
            if would_substitute(font, rphf, &[info[start].glyph, info[start + 1].glyph]) {
                limit += 2;
                while limit < end && is_joiner(info[limit].cat) {
                    limit += 1;
                }
                base = start;
                has_reph = true;
            }
        }
    }
    {
        let mut i = end;
        let mut seen_below = false;
        loop {
            i -= 1;
            if is_consonant(info[i].cat) {
                if info[i].ipos != POS_BELOW_C && (info[i].ipos != POS_POST_C || seen_below) {
                    base = i;
                    break;
                }
                if info[i].ipos == POS_BELOW_C {
                    seen_below = true;
                }
                base = i;
            } else if start < i && info[i].cat == ZWJ && info[i - 1].cat == H {
                break;
            }
            if i <= limit {
                break;
            }
        }
    }
    if has_reph && base == start && limit - base <= 2 {
        has_reph = false;
    }
    // 2. Positions.
    for g in info[start..base.min(end)].iter_mut() {
        g.ipos = g.ipos.min(POS_PRE_C);
    }
    if base < end {
        info[base].ipos = POS_BASE_C;
    }
    if has_reph {
        info[start].ipos = POS_RA_TO_BECOME_REPH;
    }
    // Attach misc marks to the previous character to move with them.
    {
        let mut last_pos = POS_START;
        for i in start..end {
            let c = info[i].cat;
            if matches!(c, ZWJ | ZWNJ | N | H) {
                info[i].ipos = last_pos;
                if c == H && info[i].ipos == POS_PRE_M {
                    for j in (start + 1..=i).rev() {
                        if info[j - 1].ipos != POS_PRE_M {
                            info[i].ipos = info[j - 1].ipos;
                            break;
                        }
                    }
                }
            } else if info[i].ipos != POS_SMVD {
                last_pos = info[i].ipos;
            }
        }
    }
    // Post-base consonants own anything before them since the last consonant or matra.
    {
        let mut last = base;
        for i in base + 1..end {
            if is_consonant(info[i].cat) {
                for j in last + 1..i {
                    if info[j].ipos < POS_SMVD {
                        info[j].ipos = info[i].ipos;
                    }
                }
                last = i;
            } else if info[i].cat == M {
                last = i;
            }
        }
    }
    // Stable sort by position, remembering where each glyph came from.
    let mut origin: Vec<usize> = (0..end - start).collect();
    {
        let seg = &mut info[start..end];
        let mut tagged: Vec<(u8, usize, GlyphInfo)> = seg.iter().enumerate().map(|(k, g)| (g.ipos, k, *g)).collect();
        tagged.sort_by_key(|t| (t.0, t.1));
        for (k, t) in tagged.into_iter().enumerate() {
            seg[k] = t.2;
            origin[k] = t.1;
        }
    }
    // Find base again; flip a left-matra sequence.
    let mut first_left = end;
    let mut last_left = end;
    base = end;
    for i in start..end {
        if info[i].ipos == POS_BASE_C {
            base = i;
            break;
        } else if info[i].ipos == POS_PRE_M {
            if first_left == end {
                first_left = i;
            }
            last_left = i;
        }
    }
    if first_left < last_left {
        info[first_left..=last_left].reverse();
        origin[first_left - start..=last_left - start].reverse();
        let mut i = first_left;
        for j in first_left..=last_left {
            if info[j].cat == M {
                info[i..=j].reverse();
                origin[i - start..=j - start].reverse();
                i = j + 1;
            }
        }
    }
    // Post-base glyphs that moved: merge the clusters each permutation cycle spans (pre-base is handled by the
    // final reordering).
    if end - start > 127 {
        merge_clusters(info, base, end);
    } else {
        let mut done = alloc::vec![false; end - start];
        for i in base..end {
            if done[i - start] {
                continue;
            }
            let (mut lo, mut hi) = (i, i);
            let mut j = start + origin[i - start];
            while j != i {
                lo = lo.min(j);
                hi = hi.max(j);
                done[j - start] = true;
                j = start + origin[j - start];
            }
            done[i - start] = true;
            merge_clusters(info, lo.max(base), hi + 1);
        }
    }
    // 3. Masks.
    let rphf = plan.mask(*b"rphf");
    let half = plan.mask(*b"half");
    let blwf = plan.mask(*b"blwf");
    let abvf = plan.mask(*b"abvf");
    let pstf = plan.mask(*b"pstf");
    let mut i = start;
    while i < end && info[i].ipos == POS_RA_TO_BECOME_REPH {
        info[i].mask |= rphf;
        i += 1;
    }
    for g in info[start..base.min(end)].iter_mut() {
        g.mask |= half | blwf;
    }
    for g in info[(base + 1).min(end)..end].iter_mut() {
        g.mask |= blwf | abvf | pstf;
    }
    // A ZWNJ disables HALF on what precedes it (back to the previous consonant).
    for i in base + 1..end {
        if info[i].cat == ZWNJ {
            let mut j = i;
            loop {
                j -= 1;
                info[j].mask &= !half;
                if j <= start || is_consonant(info[j].cat) {
                    break;
                }
            }
        }
    }
}

pub(crate) fn indic_final_reordering(_font: &Font, plan: &Plan, info: &mut [GlyphInfo]) {
    let init = plan.mask(*b"init");
    for (start, end) in syllable_ranges(info) {
        let kind = info[start].syllable & 0xF;
        if !matches!(kind, K_CONSONANT | K_VOWEL | K_STANDALONE | K_BROKEN) {
            continue;
        }
        final_reorder_syllable(info, start, end);
        if info[start].ipos == POS_PRE_M {
            let word_start = start == 0
                || !matches!(
                    char::from_u32(info[start - 1].cp).map(ucd::general_category),
                    Some(Gc::Cf | Gc::Ll | Gc::Lm | Gc::Lo | Gc::Lt | Gc::Lu | Gc::Mc | Gc::Me | Gc::Mn)
                );
            if word_start {
                info[start].mask |= init;
            }
        }
    }
}

fn final_reorder_syllable(info: &mut [GlyphInfo], start: usize, end: usize) {
    let is_halant = |g: &GlyphInfo| g.cat == H;
    // Find base again.
    let mut base = start;
    while base < end {
        if info[base].ipos >= POS_BASE_C {
            if start < base && info[base].ipos > POS_BASE_C {
                base -= 1;
            }
            break;
        }
        base += 1;
    }
    if base == end && start < base && info[base - 1].cat == ZWJ {
        base -= 1;
    }
    if base < end {
        while start < base && matches!(info[base].cat, N | H) {
            base -= 1;
        }
    }
    // Reorder matras.
    if start + 1 < end && start < base {
        let mut new_pos = if base == end { base.saturating_sub(2) } else { base - 1 };
        'search: loop {
            while new_pos > start && !matches!(info[new_pos].cat, M | H) {
                new_pos -= 1;
            }
            if is_halant(&info[new_pos]) && info[new_pos].ipos != POS_PRE_M {
                if new_pos + 1 < end && info[new_pos + 1].cat == ZWJ && new_pos > start {
                    new_pos -= 1;
                    continue 'search;
                }
            } else {
                new_pos = start;
            }
            break;
        }
        if start < new_pos && info[new_pos].ipos != POS_PRE_M {
            let mut i = new_pos;
            while i > start {
                if info[i - 1].ipos == POS_PRE_M {
                    let old = i - 1;
                    if old < base && base <= new_pos {
                        base -= 1;
                    }
                    let tmp = info[old];
                    info.copy_within(old + 1..=new_pos, old);
                    info[new_pos] = tmp;
                    merge_clusters(info, new_pos, (base + 1).min(end));
                    new_pos -= 1;
                }
                i -= 1;
            }
        } else {
            for i in start..base {
                if info[i].ipos == POS_PRE_M {
                    merge_clusters(info, i, (base + 1).min(end));
                    break;
                }
            }
        }
    }
    // Reorder reph (REPH_POS_BEFORE_POST: steps 2, 5, 6).
    if start + 1 < end && info[start].ipos == POS_RA_TO_BECOME_REPH && info[start].ligated() && !info[start].multiplied() {
        let mut new_reph = start + 1;
        while new_reph < base && !is_halant(&info[new_reph]) {
            new_reph += 1;
        }
        let found = new_reph < base && is_halant(&info[new_reph]);
        if found {
            if new_reph + 1 < base && is_joiner(info[new_reph + 1].cat) {
                new_reph += 1;
            }
        } else {
            new_reph = end - 1;
            while new_reph > start && info[new_reph].ipos == POS_SMVD {
                new_reph -= 1;
            }
            if is_halant(&info[new_reph]) {
                for i in base + 1..new_reph {
                    if info[i].cat == M {
                        new_reph -= 1;
                    }
                }
            }
        }
        merge_clusters(info, start, new_reph + 1);
        let reph = info[start];
        info.copy_within(start + 1..=new_reph, start);
        info[new_reph] = reph;
    }
}
