//! UAX #9, the Unicode Bidirectional Algorithm (Unicode 17.0.0): paragraph levels (P2/P3), explicit embeddings,
//! overrides and isolates (X1–X8, depth 125), X9 removal, isolating run sequences (X10, BD13), weak types (W1–W7),
//! paired brackets (BD16, N0), neutrals (N1/N2), implicit levels (I1/I2), the line rules L1 and L2, and mirroring
//! (L4 via Bidi_Mirroring_Glyph). Bidi_Class, Bidi_Paired_Bracket(_Type) and Bidi_Mirroring_Glyph come from the UCD
//! files only (`ucd::bidi`, generated). `tests/bidi.rs` runs BidiTest.txt and BidiCharacterTest.txt in full.

use crate::ucd::{self, lookup};
use alloc::vec::Vec;

/// Bidi_Class.
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BidiClass {
    L, R, AL, EN, ES, ET, AN, CS, NSM, BN, B, S, WS, ON, LRE, LRO, RLE, RLO, PDF, LRI, RLI, FSI, PDI,
}
use BidiClass::*;

pub fn bidi_class(c: char) -> BidiClass {
    lookup(ucd::bidi::BIDI_CLASS, c as u32).unwrap_or(L)
}

/// Bidi_Mirroring_Glyph (rule L4).
pub fn mirrored(c: char) -> Option<char> {
    let t = ucd::bidi::MIRROR;
    let i = t.binary_search_by_key(&(c as u32), |p| p.0).ok()?;
    char::from_u32(t[i].1)
}

/// Bidi_Paired_Bracket and whether `c` opens (Bidi_Paired_Bracket_Type o) or closes (c).
pub fn paired_bracket(c: char) -> Option<(char, bool)> {
    let t = ucd::bidi::BRACKETS;
    let i = t.binary_search_by_key(&(c as u32), |p| p.0).ok()?;
    Some((char::from_u32(t[i].1)?, t[i].2))
}

/// Bracket identity for BD16: openers by themselves, closers by their paired opener, both through canonical
/// equivalence (U+2329/U+232A ≡ U+3008/U+3009, BD16's note).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bracket {
    pub key: u32,
    pub open: bool,
}

fn canonical_singleton(cp: u32) -> u32 {
    let t = ucd::norm::DECOMP;
    match t.binary_search_by_key(&cp, |r| r.0) {
        Ok(i) if t[i].2 == 1 => ucd::norm::DECOMP_DATA[t[i].1 as usize],
        _ => cp,
    }
}

pub fn bracket_of(c: char) -> Option<Bracket> {
    let (pair, open) = paired_bracket(c)?;
    let opener = if open { c as u32 } else { pair as u32 };
    Some(Bracket { key: canonical_singleton(opener), open })
}

const MAX_DEPTH: u8 = 125;

/// Whether rule X9 removes a character of this class.
#[inline]
pub fn removed_by_x9(c: BidiClass) -> bool {
    matches!(c, RLE | LRE | RLO | LRO | PDF | BN)
}

#[inline]
fn is_isolate_initiator(c: BidiClass) -> bool {
    matches!(c, LRI | RLI | FSI)
}

/// P2/P3 from `from`: the first strong type outside isolates; stops at an unmatched PDI when `in_isolate` and at
/// a paragraph separator. 0 = L, 1 = R/AL, None = no strong character.
fn first_strong(classes: &[BidiClass], from: usize, in_isolate: bool) -> Option<u8> {
    let mut depth = 0usize;
    for &c in &classes[from.min(classes.len())..] {
        match c {
            L if depth == 0 => return Some(0),
            R | AL if depth == 0 => return Some(1),
            LRI | RLI | FSI => depth += 1,
            PDI => {
                if depth > 0 {
                    depth -= 1;
                } else if in_isolate {
                    return None;
                }
            }
            B => return None,
            _ => {}
        }
    }
    None
}

/// The paragraph embedding level by rules P2 and P3 (0 when no strong character).
pub fn paragraph_level(classes: &[BidiClass]) -> u8 {
    first_strong(classes, 0, false).unwrap_or(0)
}

/// The result of resolving one paragraph.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub para_level: u8,
    /// Embedding level per character after I2 (before L1). Characters removed by X9 carry the level of the
    /// character before them (or the paragraph level), which is what a renderer needs; conformance tests skip them.
    pub levels: Vec<u8>,
}

#[inline]
fn strong_dir(t: BidiClass) -> Option<BidiClass> {
    match t {
        L => Some(L),
        R | AL | EN | AN => Some(R),
        _ => None,
    }
}

#[inline]
fn is_ni(t: BidiClass) -> bool {
    matches!(t, B | S | WS | ON | LRI | RLI | FSI | PDI)
}

/// Resolve the embedding levels of ONE paragraph (rules P2–I2). `brackets[i]` is the BD16 identity of character
/// `i` (None for non-brackets, and for every character when brackets are not wanted, as in BidiTest.txt).
/// `para_level`: Some(0|1) forces the direction; None applies P2/P3.
pub fn resolve(classes: &[BidiClass], brackets: &[Option<Bracket>], para_level: Option<u8>) -> Resolved {
    let n = classes.len();
    let para = para_level.unwrap_or_else(|| paragraph_level(classes));
    // BD9: matching PDIs.
    let mut matching = alloc::vec![usize::MAX; n];
    {
        let mut st: Vec<usize> = Vec::new();
        for i in 0..n {
            match classes[i] {
                LRI | RLI | FSI => st.push(i),
                PDI => {
                    if let Some(o) = st.pop() {
                        matching[o] = i;
                        matching[i] = o;
                    }
                }
                B => st.clear(),
                _ => {}
            }
        }
    }
    // X1–X8.
    #[derive(Clone, Copy)]
    struct Entry {
        level: u8,
        ovr: Option<BidiClass>,
        isolate: bool,
    }
    let mut stack: Vec<Entry> = alloc::vec![Entry { level: para, ovr: None, isolate: false }];
    let (mut oic, mut oec, mut vic) = (0usize, 0usize, 0usize);
    let mut levels = alloc::vec![para; n];
    let mut cls: Vec<BidiClass> = classes.to_vec();
    for i in 0..n {
        let t = classes[i];
        let top = *stack.last().unwrap();
        match t {
            RLE | LRE | RLO | LRO => {
                levels[i] = top.level;
                let new = if matches!(t, RLE | RLO) { (top.level + 1) | 1 } else { (top.level + 2) & !1 };
                if new <= MAX_DEPTH && oic == 0 && oec == 0 {
                    let ovr = match t {
                        RLO => Some(R),
                        LRO => Some(L),
                        _ => None,
                    };
                    stack.push(Entry { level: new, ovr, isolate: false });
                } else if oic == 0 {
                    oec += 1;
                }
            }
            RLI | LRI | FSI => {
                levels[i] = top.level;
                if let Some(o) = top.ovr {
                    cls[i] = o;
                }
                let rtl = match t {
                    RLI => true,
                    LRI => false,
                    _ => first_strong(classes, i + 1, true) == Some(1),
                };
                let new = if rtl { (top.level + 1) | 1 } else { (top.level + 2) & !1 };
                if new <= MAX_DEPTH && oic == 0 && oec == 0 {
                    vic += 1;
                    stack.push(Entry { level: new, ovr: None, isolate: true });
                } else {
                    oic += 1;
                }
            }
            PDI => {
                if oic > 0 {
                    oic -= 1;
                } else if vic > 0 {
                    oec = 0;
                    while !stack.last().unwrap().isolate {
                        stack.pop();
                    }
                    stack.pop();
                    vic -= 1;
                }
                let top = *stack.last().unwrap();
                levels[i] = top.level;
                if let Some(o) = top.ovr {
                    cls[i] = o;
                }
            }
            PDF => {
                if oic > 0 {
                } else if oec > 0 {
                    oec -= 1;
                } else if !top.isolate && stack.len() >= 2 {
                    stack.pop();
                }
                levels[i] = stack.last().unwrap().level;
            }
            B => levels[i] = para,
            BN => levels[i] = top.level,
            _ => {
                levels[i] = top.level;
                if let Some(o) = top.ovr {
                    cls[i] = o;
                }
            }
        }
    }
    // X9 + X10: level runs over the characters X9 keeps, chained into isolating run sequences.
    let kept: Vec<usize> = (0..n).filter(|&i| !removed_by_x9(classes[i])).collect();
    let mut runs: Vec<(usize, usize)> = Vec::new(); // ranges into `kept`
    {
        let mut s = 0;
        for k in 1..=kept.len() {
            if k == kept.len() || levels[kept[k]] != levels[kept[k - 1]] {
                if k > s {
                    runs.push((s, k));
                }
                s = k;
            }
        }
    }
    let explicit = levels.clone(); // sos/eos compare EXPLICIT levels, not ones I1/I2 already raised
    let mut run_of = alloc::vec![usize::MAX; n];
    for (ri, &(s, e)) in runs.iter().enumerate() {
        for &i in &kept[s..e] {
            run_of[i] = ri;
        }
    }
    for ri in 0..runs.len() {
        let first = kept[runs[ri].0];
        if classes[first] == PDI && matching[first] != usize::MAX {
            continue; // continues the sequence of its initiator
        }
        let mut seq: Vec<usize> = Vec::new();
        let mut r = ri;
        loop {
            let (s, e) = runs[r];
            seq.extend_from_slice(&kept[s..e]);
            let last = kept[e - 1];
            if is_isolate_initiator(classes[last]) && matching[last] != usize::MAX {
                let nr = run_of[matching[last]];
                if nr == usize::MAX || nr <= r {
                    break;
                }
                r = nr;
            } else {
                break;
            }
        }
        resolve_sequence(&seq, classes, &mut cls, &mut levels, &explicit, brackets, para, &kept);
    }
    // Removed characters take the level before them (renderers; tests skip them).
    let mut prev = para;
    for i in 0..n {
        if removed_by_x9(classes[i]) {
            levels[i] = prev;
        } else {
            prev = levels[i];
        }
    }
    Resolved { para_level: para, levels }
}

#[allow(clippy::too_many_arguments)]
fn resolve_sequence(
    seq: &[usize],
    orig: &[BidiClass],
    cls: &mut [BidiClass],
    levels: &mut [u8],
    explicit: &[u8],
    brackets: &[Option<Bracket>],
    para: u8,
    kept: &[usize],
) {
    let len = seq.len();
    let first = seq[0];
    let last = seq[len - 1];
    let level = levels[first];
    // sos / eos (X10): the higher of this level and the neighbouring kept character's level.
    let kpos = |i: usize| kept.binary_search(&i).unwrap();
    let prev_level = {
        let p = kpos(first);
        if p == 0 { para } else { explicit[kept[p - 1]] }
    };
    let next_level = if is_isolate_initiator(orig[last]) {
        para
    } else {
        let p = kpos(last);
        if p + 1 >= kept.len() { para } else { explicit[kept[p + 1]] }
    };
    let sos = if level.max(prev_level) & 1 == 1 { R } else { L };
    let eos = if level.max(next_level) & 1 == 1 { R } else { L };
    let mut t: Vec<BidiClass> = seq.iter().map(|&i| cls[i]).collect();
    // W1
    for k in 0..len {
        if t[k] == NSM {
            t[k] = if k == 0 {
                sos
            } else if matches!(t[k - 1], LRI | RLI | FSI | PDI) {
                ON
            } else {
                t[k - 1]
            };
        }
    }
    // W2, W3
    let mut last_strong = sos;
    for x in t.iter_mut() {
        match *x {
            L | R | AL => last_strong = *x,
            EN if last_strong == AL => *x = AN,
            _ => {}
        }
    }
    for x in t.iter_mut() {
        if *x == AL {
            *x = R;
        }
    }
    // W4
    for k in 1..len.saturating_sub(1) {
        if t[k] == ES && t[k - 1] == EN && t[k + 1] == EN {
            t[k] = EN;
        } else if t[k] == CS && t[k - 1] == t[k + 1] && matches!(t[k - 1], EN | AN) {
            t[k] = t[k - 1];
        }
    }
    // W5
    let mut k = 0;
    while k < len {
        if t[k] == ET {
            let s = k;
            while k < len && t[k] == ET {
                k += 1;
            }
            if (s > 0 && t[s - 1] == EN) || (k < len && t[k] == EN) {
                for x in &mut t[s..k] {
                    *x = EN;
                }
            }
        } else {
            k += 1;
        }
    }
    // W6
    for x in t.iter_mut() {
        if matches!(*x, ES | ET | CS) {
            *x = ON;
        }
    }
    // W7
    let mut last_strong = sos;
    for x in t.iter_mut() {
        match *x {
            L | R => last_strong = *x,
            EN if last_strong == L => *x = L,
            _ => {}
        }
    }
    let e = if level & 1 == 1 { R } else { L };
    // BD16 + N0: paired brackets.
    if brackets.iter().any(|b| b.is_some()) {
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        let mut st: Vec<(u32, usize)> = Vec::new();
        for k in 0..len {
            let i = seq[k];
            let Some(b) = brackets.get(i).copied().flatten() else { continue };
            if cls[i] != ON || t[k] != ON {
                continue;
            }
            if b.open {
                if st.len() == 63 {
                    break;
                }
                st.push((b.key, k));
            } else if let Some(p) = st.iter().rposition(|&(key, _)| key == b.key) {
                pairs.push((st[p].1, k));
                st.truncate(p);
            }
        }
        pairs.sort_unstable();
        for (o, c) in pairs {
            let mut found_e = false;
            let mut found_opp = false;
            for x in &t[o + 1..c] {
                if let Some(s) = strong_dir(*x) {
                    if s == e {
                        found_e = true;
                        break;
                    }
                    found_opp = true;
                }
            }
            let set = if found_e {
                e
            } else if found_opp {
                let mut ctx = sos;
                for x in t[..o].iter().rev() {
                    if let Some(s) = strong_dir(*x) {
                        ctx = s;
                        break;
                    }
                }
                if ctx != e { ctx } else { e }
            } else {
                continue;
            };
            t[o] = set;
            t[c] = set;
            for &b in &[o, c] {
                let mut k = b + 1;
                while k < len && orig[seq[k]] == NSM {
                    t[k] = set;
                    k += 1;
                }
            }
        }
    }
    // N1, N2
    let mut k = 0;
    while k < len {
        if is_ni(t[k]) {
            let s = k;
            while k < len && is_ni(t[k]) {
                k += 1;
            }
            let before = if s == 0 { sos } else { strong_dir(t[s - 1]).unwrap_or(e) };
            let after = if k == len { eos } else { strong_dir(t[k]).unwrap_or(e) };
            let v = if before == after { before } else { e };
            for x in &mut t[s..k] {
                *x = v;
            }
        } else {
            k += 1;
        }
    }
    // I1, I2
    for (k, &i) in seq.iter().enumerate() {
        let lv = levels[i];
        levels[i] = if lv & 1 == 0 {
            match t[k] {
                R => lv + 1,
                AN | EN => lv + 2,
                _ => lv,
            }
        } else {
            match t[k] {
                L | EN | AN => lv + 1,
                _ => lv,
            }
        };
        cls[i] = t[k];
    }
}

/// Rule L1 over one line `[start, end)` of a paragraph: segment and paragraph separators, and any run of
/// whitespace / isolate formatting characters (and X9-removed characters) before them or at the end of the line,
/// go to the paragraph level.
pub fn apply_l1(classes: &[BidiClass], levels: &mut [u8], para: u8, start: usize, end: usize) {
    let resettable = |c: BidiClass| matches!(c, WS | FSI | LRI | RLI | PDI) || removed_by_x9(c);
    let mut trailing = true;
    for i in (start..end).rev() {
        let c = classes[i];
        if matches!(c, S | B) {
            levels[i] = para;
            trailing = true;
        } else if trailing && resettable(c) {
            levels[i] = para;
        } else {
            trailing = false;
        }
    }
}

/// Rule L2: the visual order (left to right) of characters with the given levels, as indices into `levels`.
pub fn reorder(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    if levels.is_empty() {
        return order;
    }
    let max = *levels.iter().max().unwrap();
    let min_odd = *levels.iter().min().unwrap() | 1;
    let mut lv = max;
    while lv >= min_odd {
        let mut p = 0;
        while p < order.len() {
            if levels[order[p]] >= lv {
                let s = p;
                while p < order.len() && levels[order[p]] >= lv {
                    p += 1;
                }
                order[s..p].reverse();
            } else {
                p += 1;
            }
        }
        if lv == 0 {
            break;
        }
        lv -= 1;
    }
    order
}

/// Paragraph base direction request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// P2/P3 per paragraph (first strong character, LTR if none).
    Auto,
    Ltr,
    Rtl,
}

/// Bidi analysis of a text: per-char levels with paragraphs split at paragraph separators (P1).
#[derive(Clone, Debug)]
pub struct BidiInfo {
    pub chars: Vec<char>,
    /// Byte offset of each char (plus the text length at the end).
    pub offsets: Vec<usize>,
    pub classes: Vec<BidiClass>,
    pub levels: Vec<u8>,
    /// (first char, end char, paragraph level).
    pub paragraphs: Vec<(usize, usize, u8)>,
}

impl BidiInfo {
    pub fn new(text: &str, dir: Direction) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let mut offsets: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        offsets.push(text.len());
        let classes: Vec<BidiClass> = chars.iter().map(|&c| bidi_class(c)).collect();
        let brackets: Vec<Option<Bracket>> = chars.iter().map(|&c| bracket_of(c)).collect();
        let mut levels = alloc::vec![0u8; chars.len()];
        let mut paragraphs = Vec::new();
        let mut s = 0;
        while s < chars.len() {
            let mut e = s;
            while e < chars.len() && classes[e] != B {
                e += 1;
            }
            if e < chars.len() {
                e += 1; // the separator belongs to its paragraph
            }
            let forced = match dir {
                Direction::Auto => None,
                Direction::Ltr => Some(0),
                Direction::Rtl => Some(1),
            };
            let r = resolve(&classes[s..e], &brackets[s..e], forced);
            levels[s..e].copy_from_slice(&r.levels);
            paragraphs.push((s, e, r.para_level));
            s = e;
        }
        BidiInfo { chars, offsets, classes, levels, paragraphs }
    }

    /// Whether every character resolves to level 0 (nothing to reorder).
    pub fn is_pure_ltr(&self) -> bool {
        self.levels.iter().all(|&l| l == 0)
    }

    /// Levels of the chars in `[start, end)` (one line) after L1.
    pub fn line_levels(&self, start: usize, end: usize) -> Vec<u8> {
        let mut lv = self.levels.clone();
        for &(ps, pe, pl) in &self.paragraphs {
            let (s, e) = (ps.max(start), pe.min(end));
            if s < e {
                apply_l1(&self.classes, &mut lv, pl, s, e);
            }
        }
        lv[start..end].to_vec()
    }

    /// The line `[start, end)` (char indices) as level runs in visual order: (first char, end char, level).
    pub fn visual_runs(&self, start: usize, end: usize) -> Vec<(usize, usize, u8)> {
        let lv = self.line_levels(start, end);
        let mut runs: Vec<(usize, usize, u8)> = Vec::new();
        let mut s = 0;
        for k in 1..=lv.len() {
            if k == lv.len() || lv[k] != lv[s] {
                runs.push((start + s, start + k, lv[s]));
                s = k;
            }
        }
        // L2 on runs.
        let rl: Vec<u8> = runs.iter().map(|r| r.2).collect();
        reorder(&rl).into_iter().map(|i| runs[i]).collect()
    }

    /// The visual order of chars in `[start, end)` (indices into `chars`).
    pub fn visual_order(&self, start: usize, end: usize) -> Vec<usize> {
        let lv = self.line_levels(start, end);
        reorder(&lv).into_iter().map(|i| start + i).collect()
    }
}
