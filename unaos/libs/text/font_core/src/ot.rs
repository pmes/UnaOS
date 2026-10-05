//! OpenType Layout lookup application over a glyph buffer (OpenType 1.9 GSUB/GPOS chapters): GSUB lookup types
//! 1 single, 2 multiple, 3 alternate, 4 ligature (with ligature-component tracking for marks), 5 contextual,
//! 6 chained contextual, 7 extension, 8 reverse chained single; GPOS 1 single, 2 pair, 3 cursive, 4 mark-to-base,
//! 5 mark-to-ligature, 6 mark-to-mark, 7 contextual, 8 chained contextual, 9 extension. Lookup flags (ignore
//! base/ligature/mark, mark attachment type, mark filtering sets), per-glyph feature masks, ZWJ/ZWNJ handling and
//! the attachment-offset propagation follow the semantics shaping engines (and so Chromium) implement: a glyph a
//! lookup does not "see" is skipped while matching, default-ignorable characters are skippable, contextual
//! back/lookahead ignores masks, nested lookups re-index the match positions when they change the buffer length.

use crate::layout::{
    class_of, coverage_index, read_value, sub, value_size, Gdef, LayoutTable, Lookup, Value,
    LOOKUP_IGNORE_MARKS, LOOKUP_MARK_ATTACHMENT_TYPE, LOOKUP_RIGHT_TO_LEFT, LOOKUP_USE_MARK_FILTERING_SET,
};
use crate::reader::{i16_at, u16_at};
use crate::Font;
use alloc::vec::Vec;

pub const GP_BASE: u16 = 0x02;
pub const GP_LIGATURE: u16 = 0x04;
pub const GP_MARK: u16 = 0x08;
const GP_IGNORE_FLAGS: u16 = 0x0E;

pub const ST_SUBSTITUTED: u8 = 1;
pub const ST_LIGATED: u8 = 2;
pub const ST_MULTIPLIED: u8 = 4;

pub const IGN_NONE: u8 = 0;
pub const IGN_ZWNJ: u8 = 1;
pub const IGN_ZWJ: u8 = 2;
pub const IGN_OTHER: u8 = 3;
/// Default ignorables shaping must not skip during GSUB (CGJ, Mongolian FVS, tags).
pub const IGN_HIDDEN: u8 = 4;

pub const ATTACH_MARK: u8 = 1;
pub const ATTACH_CURSIVE: u8 = 2;

const MAX_CONTEXT: usize = 64;
const MAX_NESTING: u32 = 6;

/// One glyph of the shaping buffer.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlyphInfo {
    pub glyph: u16,
    /// The (normalized) source character the glyph came from.
    pub cp: u32,
    /// Byte offset of the cluster's first character in the source text.
    pub cluster: usize,
    pub mask: u32,
    /// GP_* class bits | mark attachment class << 8.
    pub props: u16,
    /// ST_* flags.
    pub subst: u8,
    pub lig_id: u8,
    /// Component index (1-based) a mark/component belongs to; 0 for a ligature glyph itself.
    pub lig_comp: u8,
    /// Number of components of a ligature glyph (0 when not a ligature).
    pub lig_num: u8,
    /// Syllable serial (0 = none) for per-syllable features.
    pub syllable: u16,
    /// Shaper category / position scratch (Indic).
    pub cat: u8,
    pub ipos: u8,
    /// IGN_* — default-ignorable kind.
    pub ignorable: u8,
}

impl GlyphInfo {
    pub fn is_mark(&self) -> bool {
        self.props & GP_MARK != 0
    }
    pub fn is_ligature(&self) -> bool {
        self.props & GP_LIGATURE != 0
    }
    pub fn is_base(&self) -> bool {
        self.props & GP_BASE != 0
    }
    pub fn ligated(&self) -> bool {
        self.subst & ST_LIGATED != 0
    }
    pub fn multiplied(&self) -> bool {
        self.subst & ST_MULTIPLIED != 0
    }
    fn num_comps(&self) -> u8 {
        if self.is_ligature() && self.lig_num > 0 { self.lig_num } else { 1 }
    }
    fn comp(&self) -> u8 {
        if self.lig_num > 0 { 0 } else { self.lig_comp }
    }
}

/// One glyph's position (font units).
#[derive(Clone, Copy, Debug, Default)]
pub struct GlyphPosition {
    pub x_advance: i32,
    pub y_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
    /// Offset to the glyph this one is attached to (0 = none).
    pub chain: i32,
    pub attach: u8,
}

/// GDEF-derived properties of a glyph (`None` GDEF classes → synthesized from the character: nonspacing marks
/// are marks, everything else a base).
pub fn glyph_props(font: &Font, gid: u16, synth_mark: bool) -> u16 {
    match font.gdef.as_ref() {
        Some(g) if g.has_glyph_classes() => match g.glyph_class(gid) {
            1 => GP_BASE,
            2 => GP_LIGATURE,
            3 => GP_MARK | (g.mark_attach_class(gid) << 8),
            _ => 0,
        },
        _ => {
            if synth_mark { GP_MARK } else { GP_BASE }
        }
    }
}

/// A lookup to apply with the mask of the feature(s) that selected it.
#[derive(Clone, Copy, Debug)]
pub struct LookupReq {
    pub index: u16,
    pub mask: u32,
    pub auto_zwj: bool,
    pub auto_zwnj: bool,
    pub per_syllable: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Skip {
    Yes,
    No,
    Maybe,
}

/// The lookup-application context over a buffer.
pub struct Apply<'b, 'f> {
    pub font: &'b Font<'f>,
    gdef: Option<Gdef<'f>>,
    has_classes: bool,
    table: LayoutTable<'f>,
    gpos: bool,
    pub info: &'b mut Vec<GlyphInfo>,
    pub pos: &'b mut Vec<GlyphPosition>,
    pub rtl: bool,
    // The current lookup.
    mask: u32,
    flag: u16,
    mark_set: u16,
    auto_zwj: bool,
    auto_zwnj: bool,
    per_syllable: bool,
    next_lig_id: u8,
    depth: u32,
}

impl<'b, 'f> Apply<'b, 'f> {
    pub fn new(
        font: &'b Font<'f>,
        table: LayoutTable<'f>,
        gpos: bool,
        info: &'b mut Vec<GlyphInfo>,
        pos: &'b mut Vec<GlyphPosition>,
        rtl: bool,
        next_lig_id: u8,
    ) -> Self {
        let gdef = font.gdef;
        let has_classes = gdef.is_some_and(|g| g.has_glyph_classes());
        Apply {
            font,
            gdef,
            has_classes,
            table,
            gpos,
            info,
            pos,
            rtl,
            mask: !0,
            flag: 0,
            mark_set: 0,
            auto_zwj: true,
            auto_zwnj: true,
            per_syllable: false,
            next_lig_id,
            depth: 0,
        }
    }

    pub fn next_lig_id(&self) -> u8 {
        self.next_lig_id
    }

    fn alloc_lig_id(&mut self) -> u8 {
        let id = self.next_lig_id;
        self.next_lig_id = if self.next_lig_id >= 7 { 1 } else { self.next_lig_id + 1 };
        id
    }

    // ------------------------------------------------------------------ glyph filtering

    /// Whether the current lookup's flags admit glyph `i`.
    fn check_props(&self, i: usize, flag: u16, mark_set: u16) -> bool {
        let p = self.info[i].props;
        if p & flag & GP_IGNORE_FLAGS != 0 {
            return false;
        }
        if p & GP_MARK != 0 {
            if flag & LOOKUP_USE_MARK_FILTERING_SET != 0 {
                return self.gdef.is_some_and(|g| g.mark_set_covers(mark_set, self.info[i].glyph));
            }
            if flag & LOOKUP_MARK_ATTACHMENT_TYPE != 0 {
                return (flag & LOOKUP_MARK_ATTACHMENT_TYPE) == (p & LOOKUP_MARK_ATTACHMENT_TYPE);
            }
        }
        true
    }

    fn may_skip(&self, i: usize, flag: u16, context: bool) -> Skip {
        if !self.check_props(i, flag, self.mark_set) {
            return Skip::Yes;
        }
        let ig = if self.info[i].subst & ST_SUBSTITUTED != 0 { IGN_NONE } else { self.info[i].ignorable };
        if ig != IGN_NONE {
            let ignore_zwnj = self.gpos || (context && self.auto_zwnj);
            let ignore_zwj = context || self.auto_zwj;
            let ignore_hidden = self.gpos;
            if (ignore_zwnj || ig != IGN_ZWNJ) && (ignore_zwj || ig != IGN_ZWJ) && (ignore_hidden || ig != IGN_HIDDEN) {
                return Skip::Maybe;
            }
        }
        Skip::No
    }

    /// Search from `from` (exclusive) in `dir` (+1/-1) for the next glyph the matcher accepts. `m` returns
    /// Some(true/false) for a definite (non-)match or None for "any glyph". `context`: backtrack/lookahead, which
    /// ignore masks. `syllable`: restrict to that syllable (per-syllable features).
    fn seek(
        &self,
        from: usize,
        forward: bool,
        flag: u16,
        context: bool,
        syllable: u16,
        m: &mut dyn FnMut(usize) -> Option<bool>,
    ) -> Option<usize> {
        let len = self.info.len();
        let mut k = from as isize;
        loop {
            k += if forward { 1 } else { -1 };
            if k < 0 || k as usize >= len {
                return None;
            }
            let i = k as usize;
            let skip = self.may_skip(i, flag, context);
            if skip == Skip::Yes {
                continue;
            }
            let mask_ok = context || self.info[i].mask & self.mask != 0;
            let syl_ok = syllable == 0 || self.info[i].syllable == syllable;
            let matched = if !mask_ok || !syl_ok { Some(false) } else { m(i) };
            match matched {
                Some(true) => return Some(i),
                None if skip == Skip::No => return Some(i),
                _ => {}
            }
            if skip == Skip::No {
                return None;
            }
        }
    }

    fn syl(&self, i: usize) -> u16 {
        if self.per_syllable && !self.gpos { self.info[i].syllable } else { 0 }
    }

    // ------------------------------------------------------------------ buffer edits

    fn set_glyph(&mut self, i: usize, g: u16, class_guess: u16) {
        let info = &mut self.info[i];
        info.glyph = g;
        info.subst |= ST_SUBSTITUTED;
        if self.has_classes {
            info.props = glyph_props(self.font, g, false);
        } else if class_guess != 0 {
            info.props = class_guess;
        }
    }

    // ------------------------------------------------------------------ driving

    /// Apply one lookup across the whole buffer.
    pub fn apply_lookup(&mut self, req: &LookupReq) {
        let Some(l) = self.table.lookup(req.index) else { return };
        self.mask = req.mask;
        self.auto_zwj = req.auto_zwj;
        self.auto_zwnj = req.auto_zwnj;
        self.per_syllable = req.per_syllable;
        self.flag = l.flag;
        self.mark_set = l.mark_set;
        if !self.gpos && l.kind == 8 {
            let mut i = self.info.len();
            while i > 0 {
                i -= 1;
                if self.info[i].mask & self.mask != 0 && self.check_props(i, self.flag, self.mark_set) {
                    for st in &l.subtables {
                        if self.reverse_chain(st, i) {
                            break;
                        }
                    }
                }
            }
            return;
        }
        let mut i = 0;
        while i < self.info.len() {
            if self.info[i].mask & self.mask != 0 && self.check_props(i, self.flag, self.mark_set) {
                if let Some(next) = self.apply_at(&l, i) {
                    i = next.max(i + 1).min(self.info.len().max(i + 1));
                    continue;
                }
            }
            i += 1;
        }
    }

    /// Try the lookup's subtables at `i`; Some(next index) when one applied.
    fn apply_at(&mut self, l: &Lookup<'f>, i: usize) -> Option<usize> {
        for st in &l.subtables {
            let r = if self.gpos { self.gpos_subtable(l.kind, st, i) } else { self.gsub_subtable(l.kind, st, i) };
            if r.is_some() {
                return r;
            }
        }
        None
    }

    /// A nested lookup (from a SequenceLookupRecord) at `i`.
    fn recurse(&mut self, lookup_index: u16, i: usize) -> bool {
        if self.depth >= MAX_NESTING {
            return false;
        }
        let Some(l) = self.table.lookup(lookup_index) else { return false };
        let (sf, sm) = (self.flag, self.mark_set);
        self.flag = l.flag;
        self.mark_set = l.mark_set;
        self.depth += 1;
        let r = if !self.gpos && l.kind == 8 {
            l.subtables.iter().any(|st| self.reverse_chain(st, i))
        } else {
            self.apply_at(&l, i).is_some()
        };
        self.depth -= 1;
        self.flag = sf;
        self.mark_set = sm;
        r
    }

    // ------------------------------------------------------------------ GSUB

    fn gsub_subtable(&mut self, kind: u16, st: &[u8], i: usize) -> Option<usize> {
        let g = self.info[i].glyph;
        match kind {
            1 => {
                let ng = crate::layout::single_subst(st, g)?;
                self.set_glyph(i, ng, 0);
                Some(i + 1)
            }
            2 => {
                if u16_at(st, 0)? != 1 {
                    return None;
                }
                let ci = coverage_index(sub(st, 0, 2)?, g)?;
                if ci >= u16_at(st, 4)? {
                    return None;
                }
                let seq = sub(st, 0, 6 + 2 * ci as usize)?;
                let n = u16_at(seq, 0)? as usize;
                let glyphs: Vec<u16> = (0..n).filter_map(|k| u16_at(seq, 2 + 2 * k)).collect();
                if glyphs.len() != n {
                    return None;
                }
                if n == 1 {
                    self.set_glyph(i, glyphs[0], 0);
                    return Some(i + 1);
                }
                if n == 0 {
                    // Deleting a glyph: its cluster merges into the neighbour.
                    self.info.remove(i);
                    self.pos.remove(i);
                    return Some(i);
                }
                let base = self.info[i];
                let guess = if base.is_ligature() { GP_BASE } else { 0 };
                let mut out = Vec::with_capacity(n);
                for (k, &ng) in glyphs.iter().enumerate() {
                    let mut gi = base;
                    gi.glyph = ng;
                    gi.subst |= ST_SUBSTITUTED | ST_MULTIPLIED;
                    gi.lig_id = 0;
                    gi.lig_num = 0;
                    gi.lig_comp = (k + 1).min(15) as u8;
                    if self.has_classes {
                        gi.props = glyph_props(self.font, ng, false);
                    } else if guess != 0 {
                        gi.props = guess;
                    }
                    out.push(gi);
                }
                let p = self.pos[i];
                self.info.splice(i..i + 1, out);
                self.pos.splice(i..i + 1, core::iter::repeat_n(p, n));
                Some(i + n)
            }
            3 => {
                if u16_at(st, 0)? != 1 {
                    return None;
                }
                let ci = coverage_index(sub(st, 0, 2)?, g)?;
                if ci >= u16_at(st, 4)? {
                    return None;
                }
                let set = sub(st, 0, 6 + 2 * ci as usize)?;
                if u16_at(set, 0)? == 0 {
                    return None;
                }
                let ng = u16_at(set, 2)?;
                self.set_glyph(i, ng, 0);
                Some(i + 1)
            }
            4 => self.ligature(st, i),
            5 => self.context(st, i),
            6 => self.chain_context(st, i),
            _ => None,
        }
    }

    fn ligature(&mut self, st: &[u8], i: usize) -> Option<usize> {
        if u16_at(st, 0)? != 1 {
            return None;
        }
        let ci = coverage_index(sub(st, 0, 2)?, self.info[i].glyph)?;
        if ci >= u16_at(st, 4)? {
            return None;
        }
        let set = sub(st, 0, 6 + 2 * ci as usize)?;
        let nl = u16_at(set, 0)? as usize;
        let flag = self.flag;
        let syl = self.syl(i);
        'lig: for k in 0..nl {
            let Some(lig) = sub(set, 0, 2 + 2 * k) else { continue };
            let Some(lg) = u16_at(lig, 0) else { continue };
            let Some(cc) = u16_at(lig, 2) else { continue };
            let cc = cc as usize;
            if cc == 0 || cc > MAX_CONTEXT {
                continue;
            }
            let mut mp = alloc::vec![i];
            for j in 1..cc {
                let Some(want) = u16_at(lig, 4 + 2 * (j - 1)) else { continue 'lig };
                let last = *mp.last().unwrap();
                let info = &*self.info;
                let Some(p) = self.seek(last, true, flag, false, syl, &mut |q| Some(info[q].glyph == want)) else {
                    continue 'lig;
                };
                mp.push(p);
            }
            self.ligate(&mp, lg);
            return Some(i + 1);
        }
        None
    }

    /// Replace the components at `mp` by `lig` (HarfBuzz's ligate_input semantics for mark components).
    fn ligate(&mut self, mp: &[usize], lig: u16) {
        let first = mp[0];
        let last = *mp.last().unwrap();
        let mut is_mark_lig = self.info[first].is_mark();
        let mut is_base_lig = self.info[first].is_base();
        for &p in &mp[1..] {
            if !self.info[p].is_mark() {
                is_mark_lig = false;
                is_base_lig = false;
                break;
            }
        }
        let is_ligature = !is_base_lig && !is_mark_lig;
        let total: u32 = mp.iter().map(|&p| self.info[p].num_comps() as u32).sum();
        let lig_id = if is_ligature { self.alloc_lig_id() } else { 0 };
        merge_clusters(self.info, first, last + 1);
        let cluster = self.info[first].cluster;
        let mut last_lig_id = self.info[first].lig_id;
        let mut last_num = self.info[first].num_comps() as u32;
        let mut so_far = last_num;
        let mut head = self.info[first];
        head.glyph = lig;
        head.subst |= ST_SUBSTITUTED | ST_LIGATED;
        head.subst &= !ST_MULTIPLIED;
        head.cluster = cluster;
        if is_ligature {
            head.lig_id = lig_id;
            head.lig_num = total.min(15) as u8;
            head.lig_comp = 0;
        }
        if self.has_classes {
            head.props = glyph_props(self.font, lig, false);
        } else if is_ligature {
            head.props = GP_LIGATURE;
        }
        let mut out_info = alloc::vec![head];
        let mut out_pos = alloc::vec![self.pos[first]];
        let mut mi = 1;
        for k in first + 1..=last {
            if mi < mp.len() && k == mp[mi] {
                last_lig_id = self.info[k].lig_id;
                last_num = self.info[k].num_comps() as u32;
                so_far += last_num;
                mi += 1;
                continue;
            }
            let mut m = self.info[k];
            if is_ligature {
                let mut this_comp = m.comp() as u32;
                if this_comp == 0 {
                    this_comp = last_num;
                }
                let new_comp = so_far - last_num + this_comp.min(last_num);
                m.lig_id = lig_id;
                m.lig_comp = new_comp.min(15) as u8;
                m.lig_num = 0;
            }
            m.cluster = cluster;
            out_info.push(m);
            out_pos.push(self.pos[k]);
        }
        let n_out = out_info.len();
        self.info.splice(first..=last, out_info);
        self.pos.splice(first..=last, out_pos);
        if !is_mark_lig && last_lig_id != 0 {
            let mut k = first + n_out;
            while k < self.info.len() {
                let m = self.info[k];
                if m.lig_id != last_lig_id {
                    break;
                }
                let this_comp = m.comp() as u32;
                if this_comp == 0 {
                    break;
                }
                let new_comp = so_far - last_num + this_comp.min(last_num);
                self.info[k].lig_id = lig_id;
                self.info[k].lig_comp = new_comp.min(15) as u8;
                k += 1;
            }
        }
    }

    fn reverse_chain(&mut self, st: &[u8], i: usize) -> bool {
        (|| -> Option<bool> {
            if u16_at(st, 0)? != 1 {
                return None;
            }
            let ci = coverage_index(sub(st, 0, 2)?, self.info[i].glyph)?;
            let nb = u16_at(st, 4)? as usize;
            let la_at = 6 + 2 * nb;
            let nl = u16_at(st, la_at)? as usize;
            let sub_at = la_at + 2 + 2 * nl;
            let ng = u16_at(st, sub_at)?;
            if ci >= ng {
                return None;
            }
            let flag = self.flag;
            let mut p = i;
            for k in 0..nb {
                let cov = sub(st, 0, 6 + 2 * k)?;
                let info = &*self.info;
                p = self.seek(p, false, flag, true, 0, &mut |q| Some(coverage_index(cov, info[q].glyph).is_some()))?;
            }
            let mut p = i;
            for k in 0..nl {
                let cov = sub(st, 0, la_at + 2 + 2 * k)?;
                let info = &*self.info;
                p = self.seek(p, true, flag, true, 0, &mut |q| Some(coverage_index(cov, info[q].glyph).is_some()))?;
            }
            let g = u16_at(st, sub_at + 2 + 2 * ci as usize)?;
            self.set_glyph(i, g, 0);
            Some(true)
        })()
        .unwrap_or(false)
    }

    // ------------------------------------------------------------------ contextual (shared by GSUB 5/6, GPOS 7/8)

    /// Match `count - 1` input glyphs after `i`; `m(q, j)` tests buffer glyph `q` against input item `j` (1-based).
    fn match_input(&self, i: usize, count: usize, m: &dyn Fn(usize, usize) -> bool) -> Option<Vec<usize>> {
        if count == 0 || count > MAX_CONTEXT {
            return None;
        }
        let syl = self.syl(i);
        let mut mp = alloc::vec![i];
        for j in 1..count {
            let last = *mp.last().unwrap();
            let p = self.seek(last, true, self.flag, false, syl, &mut |q| Some(m(q, j)))?;
            mp.push(p);
        }
        Some(mp)
    }

    fn match_backtrack(&self, i: usize, count: usize, m: &dyn Fn(usize, usize) -> bool) -> bool {
        let mut p = i;
        for j in 0..count {
            match self.seek(p, false, self.flag, true, 0, &mut |q| Some(m(q, j))) {
                Some(q) => p = q,
                None => return false,
            }
        }
        true
    }

    fn match_lookahead(&self, last: usize, count: usize, m: &dyn Fn(usize, usize) -> bool) -> bool {
        let mut p = last;
        for j in 0..count {
            match self.seek(p, true, self.flag, true, 0, &mut |q| Some(m(q, j))) {
                Some(q) => p = q,
                None => return false,
            }
        }
        true
    }

    /// Apply SequenceLookupRecords over the matched positions; returns the index after the (adjusted) match.
    fn apply_records(&mut self, mut mp: Vec<usize>, records: &[u8], nrec: usize) -> usize {
        let mut count = mp.len();
        let mut end = *mp.last().unwrap() + 1;
        for r in 0..nrec {
            let Some(seq_idx) = u16_at(records, 4 * r) else { break };
            let Some(lk) = u16_at(records, 4 * r + 2) else { break };
            let idx = seq_idx as usize;
            if idx >= count {
                continue;
            }
            let orig_len = self.info.len() as isize;
            let at = mp[idx];
            if at >= self.info.len() {
                break;
            }
            if !self.recurse(lk, at) {
                continue;
            }
            let new_len = self.info.len() as isize;
            let mut delta = new_len - orig_len;
            if delta == 0 {
                continue;
            }
            end = (end as isize + delta).max(0) as usize;
            let mut next = idx + 1;
            if delta > 0 {
                if delta as usize + count > MAX_CONTEXT {
                    break;
                }
            } else {
                delta = delta.max(next as isize - count as isize);
                next = (next as isize - delta) as usize;
            }
            // Shift.
            let new_count = (count as isize + delta) as usize;
            let mut nmp = alloc::vec![0usize; new_count.max(1)];
            let nn = (next as isize + delta) as usize;
            nmp[..idx + 1].copy_from_slice(&mp[..idx + 1]);
            for k in next..count {
                let dst = (k as isize + delta) as usize;
                if dst < new_count {
                    nmp[dst] = mp[k];
                }
            }
            for j in idx + 1..nn.min(new_count) {
                nmp[j] = nmp[j - 1] + 1;
            }
            for v in nmp.iter_mut().take(new_count).skip(nn) {
                *v = (*v as isize + delta) as usize;
            }
            mp = nmp;
            count = new_count;
        }
        end.min(self.info.len())
    }

    fn context(&mut self, st: &[u8], i: usize) -> Option<usize> {
        let g = self.info[i].glyph;
        match u16_at(st, 0)? {
            1 => {
                let ci = coverage_index(sub(st, 0, 2)?, g)?;
                if ci >= u16_at(st, 4)? {
                    return None;
                }
                let set = sub(st, 0, 6 + 2 * ci as usize)?;
                for r in 0..u16_at(set, 0)? as usize {
                    let Some(rule) = sub(set, 0, 2 + 2 * r) else { continue };
                    let (Some(gc), Some(lc)) = (u16_at(rule, 0), u16_at(rule, 2)) else { continue };
                    let (gc, lc) = (gc as usize, lc as usize);
                    let info = &*self.info;
                    let m = |q: usize, j: usize| u16_at(rule, 4 + 2 * (j - 1)) == Some(info[q].glyph);
                    if let Some(mp) = self.match_input(i, gc, &m) {
                        let recs = rule.get(4 + 2 * (gc.max(1) - 1)..).unwrap_or(&[]);
                        return Some(self.apply_records(mp, recs, lc));
                    }
                }
                None
            }
            2 => {
                coverage_index(sub(st, 0, 2)?, g)?;
                let cd = sub(st, 0, 4)?;
                let c0 = class_of(cd, g);
                if c0 >= u16_at(st, 6)? {
                    return None;
                }
                let set = sub(st, 0, 8 + 2 * c0 as usize)?;
                for r in 0..u16_at(set, 0)? as usize {
                    let Some(rule) = sub(set, 0, 2 + 2 * r) else { continue };
                    let (Some(gc), Some(lc)) = (u16_at(rule, 0), u16_at(rule, 2)) else { continue };
                    let (gc, lc) = (gc as usize, lc as usize);
                    let info = &*self.info;
                    let m = |q: usize, j: usize| u16_at(rule, 4 + 2 * (j - 1)) == Some(class_of(cd, info[q].glyph));
                    if let Some(mp) = self.match_input(i, gc, &m) {
                        let recs = rule.get(4 + 2 * (gc.max(1) - 1)..).unwrap_or(&[]);
                        return Some(self.apply_records(mp, recs, lc));
                    }
                }
                None
            }
            3 => {
                let gc = u16_at(st, 2)? as usize;
                let lc = u16_at(st, 4)? as usize;
                if gc == 0 {
                    return None;
                }
                coverage_index(sub(st, 0, 6)?, g)?;
                let info = &*self.info;
                let m = |q: usize, j: usize| {
                    sub(st, 0, 6 + 2 * j).and_then(|c| coverage_index(c, info[q].glyph)).is_some()
                };
                let mp = self.match_input(i, gc, &m)?;
                let recs = st.get(6 + 2 * gc..).unwrap_or(&[]);
                Some(self.apply_records(mp, recs, lc))
            }
            _ => None,
        }
    }

    fn chain_context(&mut self, st: &[u8], i: usize) -> Option<usize> {
        let g = self.info[i].glyph;
        match u16_at(st, 0)? {
            1 => {
                let ci = coverage_index(sub(st, 0, 2)?, g)?;
                if ci >= u16_at(st, 4)? {
                    return None;
                }
                let set = sub(st, 0, 6 + 2 * ci as usize)?;
                for r in 0..u16_at(set, 0)? as usize {
                    let Some(rule) = sub(set, 0, 2 + 2 * r) else { continue };
                    let Some(res) = self.chain_rule(rule, i, &|_, x| x, &|_, x| x, &|_, x| x) else { continue };
                    return Some(res);
                }
                None
            }
            2 => {
                coverage_index(sub(st, 0, 2)?, g)?;
                let bcd = sub(st, 0, 4);
                let icd = sub(st, 0, 6)?;
                let lcd = sub(st, 0, 8);
                let c0 = class_of(icd, g);
                if c0 >= u16_at(st, 10)? {
                    return None;
                }
                let set = sub(st, 0, 12 + 2 * c0 as usize)?;
                let fb = move |_: usize, gid: u16| bcd.map_or(0, |c| class_of(c, gid));
                let fi = move |_: usize, gid: u16| class_of(icd, gid);
                let fl = move |_: usize, gid: u16| lcd.map_or(0, |c| class_of(c, gid));
                for r in 0..u16_at(set, 0)? as usize {
                    let Some(rule) = sub(set, 0, 2 + 2 * r) else { continue };
                    let Some(res) = self.chain_rule(rule, i, &fb, &fi, &fl) else { continue };
                    return Some(res);
                }
                None
            }
            3 => {
                let nb = u16_at(st, 2)? as usize;
                let in_at = 4 + 2 * nb;
                let ni = u16_at(st, in_at)? as usize;
                let la_at = in_at + 2 + 2 * ni;
                let nl = u16_at(st, la_at)? as usize;
                let rec_at = la_at + 2 + 2 * nl;
                let nrec = u16_at(st, rec_at)? as usize;
                if ni == 0 {
                    return None;
                }
                coverage_index(sub(st, 0, in_at + 2)?, g)?;
                let info = &*self.info;
                let cov = |at: usize, q: usize| sub(st, 0, at).and_then(|c| coverage_index(c, info[q].glyph)).is_some();
                let mi = |q: usize, j: usize| cov(in_at + 2 + 2 * j, q);
                let mb = |q: usize, j: usize| cov(4 + 2 * j, q);
                let ml = |q: usize, j: usize| cov(la_at + 2 + 2 * j, q);
                let mp = self.match_input(i, ni, &mi)?;
                if !self.match_backtrack(i, nb, &mb) || !self.match_lookahead(*mp.last().unwrap(), nl, &ml) {
                    return None;
                }
                let recs = st.get(rec_at + 2..).unwrap_or(&[]);
                Some(self.apply_records(mp, recs, nrec))
            }
            _ => None,
        }
    }

    /// One ChainRule / ChainClassRule (formats 1 and 2); `bm/im/lm` map a glyph to the value compared with the
    /// rule's arrays (identity for glyph rules, the class for class rules).
    fn chain_rule(
        &mut self,
        rule: &[u8],
        i: usize,
        bm: &dyn Fn(usize, u16) -> u16,
        im: &dyn Fn(usize, u16) -> u16,
        lm: &dyn Fn(usize, u16) -> u16,
    ) -> Option<usize> {
        let nb = u16_at(rule, 0)? as usize;
        let in_at = 2 + 2 * nb;
        let ni = u16_at(rule, in_at)? as usize;
        if ni == 0 {
            return None;
        }
        let la_at = in_at + 2 + 2 * (ni - 1);
        let nl = u16_at(rule, la_at)? as usize;
        let rec_at = la_at + 2 + 2 * nl;
        let nrec = u16_at(rule, rec_at)? as usize;
        let info = &*self.info;
        let mi = |q: usize, j: usize| u16_at(rule, in_at + 2 + 2 * (j - 1)) == Some(im(q, info[q].glyph));
        let mb = |q: usize, j: usize| u16_at(rule, 2 + 2 * j) == Some(bm(q, info[q].glyph));
        let ml = |q: usize, j: usize| u16_at(rule, la_at + 2 + 2 * j) == Some(lm(q, info[q].glyph));
        let mp = self.match_input(i, ni, &mi)?;
        if !self.match_backtrack(i, nb, &mb) || !self.match_lookahead(*mp.last().unwrap(), nl, &ml) {
            return None;
        }
        let recs = rule.get(rec_at + 2..).unwrap_or(&[]);
        Some(self.apply_records(mp, recs, nrec))
    }

    // ------------------------------------------------------------------ GPOS

    fn apply_value(&mut self, i: usize, v: &Value) {
        let p = &mut self.pos[i];
        p.x_offset += v.x_placement as i32;
        p.y_offset += v.y_placement as i32;
        p.x_advance += v.x_advance as i32;
    }

    fn gpos_subtable(&mut self, kind: u16, st: &[u8], i: usize) -> Option<usize> {
        let g = self.info[i].glyph;
        match kind {
            1 => {
                let ci = coverage_index(sub(st, 0, 2)?, g)?;
                let vf = u16_at(st, 4)?;
                let v = match u16_at(st, 0)? {
                    1 => read_value(st, 6, vf)?,
                    2 => {
                        if ci >= u16_at(st, 6)? {
                            return None;
                        }
                        read_value(st, 8 + ci as usize * value_size(vf), vf)?
                    }
                    _ => return None,
                };
                self.apply_value(i, &v);
                Some(i + 1)
            }
            2 => {
                coverage_index(sub(st, 0, 2)?, g)?;
                let flag = self.flag;
                let j = self.seek(i, true, flag, false, 0, &mut |_| None)?;
                let (v1, v2, has2) = crate::layout::pair_adjust(st, g, self.info[j].glyph)?;
                self.apply_value(i, &v1);
                self.apply_value(j, &v2);
                Some(if has2 { j + 1 } else { j })
            }
            3 => self.cursive(st, i),
            4 | 5 => self.mark_to_base_or_lig(st, i, kind == 5),
            6 => self.mark_to_mark(st, i),
            7 => self.context(st, i),
            8 => self.chain_context(st, i),
            _ => None,
        }
    }

    fn anchor(d: &[u8]) -> Option<(i32, i32)> {
        Some((i16_at(d, 2)? as i32, i16_at(d, 4)? as i32))
    }

    fn cursive(&mut self, st: &[u8], j: usize) -> Option<usize> {
        if u16_at(st, 0)? != 1 {
            return None;
        }
        let cov = sub(st, 0, 2)?;
        let n = u16_at(st, 4)?;
        let cj = coverage_index(cov, self.info[j].glyph)?;
        if cj >= n {
            return None;
        }
        let entry = Self::anchor(sub(st, 0, 6 + 4 * cj as usize)?)?;
        let flag = self.flag;
        let i = self.seek(j, false, flag, false, 0, &mut |_| None)?;
        let ci = coverage_index(cov, self.info[i].glyph)?;
        if ci >= n {
            return None;
        }
        let exit = Self::anchor(sub(st, 0, 6 + 4 * ci as usize + 2)?)?;
        let (exit_x, exit_y) = exit;
        let (entry_x, entry_y) = entry;
        if !self.rtl {
            self.pos[i].x_advance = exit_x + self.pos[i].x_offset;
            let d = entry_x + self.pos[j].x_offset;
            self.pos[j].x_advance -= d;
            self.pos[j].x_offset -= d;
        } else {
            let d = exit_x + self.pos[i].x_offset;
            self.pos[i].x_advance -= d;
            self.pos[i].x_offset -= d;
            self.pos[j].x_advance = entry_x + self.pos[j].x_offset;
        }
        let (mut child, mut parent) = (i, j);
        let mut y_offset = entry_y - exit_y;
        if flag & LOOKUP_RIGHT_TO_LEFT == 0 {
            core::mem::swap(&mut child, &mut parent);
            y_offset = -y_offset;
        }
        reverse_cursive_minor_offset(self.pos, child, parent);
        self.pos[child].attach = ATTACH_CURSIVE;
        self.pos[child].chain = parent as i32 - child as i32;
        self.pos[child].y_offset = y_offset;
        if self.pos[parent].chain == -self.pos[child].chain {
            self.pos[parent].chain = 0; // break a two-glyph loop
        }
        Some(j + 1)
    }

    /// GPOS 4/5: search back for the base (marks skipped) and attach.
    fn mark_to_base_or_lig(&mut self, st: &[u8], i: usize, lig: bool) -> Option<usize> {
        if u16_at(st, 0)? != 1 {
            return None;
        }
        let mark_index = coverage_index(sub(st, 0, 2)?, self.info[i].glyph)?;
        let base_cov = sub(st, 0, 4)?;
        let class_count = u16_at(st, 6)?;
        let mark_array = sub(st, 0, 8)?;
        let base_array = sub(st, 0, 10)?;
        // Search backwards for a glyph the IgnoreMarks matcher accepts.
        let mut found = None;
        let mut k = i;
        while k > 0 {
            k -= 1;
            let skip = self.may_skip(k, LOOKUP_IGNORE_MARKS, false);
            if skip == Skip::Yes {
                continue;
            }
            let mask_ok = self.info[k].mask & self.mask != 0;
            if !(mask_ok && skip == Skip::No) {
                continue;
            }
            if !lig && !self.accept_base(k) && coverage_index(base_cov, self.info[k].glyph).is_none() {
                continue;
            }
            found = Some(k);
            break;
        }
        let b = found?;
        let bi = coverage_index(base_cov, self.info[b].glyph)?;
        let anchor_table: &[u8];
        let row: u16;
        if lig {
            if bi >= u16_at(base_array, 0)? {
                return None;
            }
            let attach = sub(base_array, 0, 2 + 2 * bi as usize)?;
            let comp_count = u16_at(attach, 0)?;
            if comp_count == 0 {
                return None;
            }
            let (lig_id, mark_id, mark_comp) = (self.info[b].lig_id, self.info[i].lig_id, self.info[i].comp());
            let comp = if lig_id != 0 && lig_id == mark_id && mark_comp > 0 {
                (comp_count.min(mark_comp as u16)) - 1
            } else {
                comp_count - 1
            };
            anchor_table = attach;
            row = comp;
        } else {
            if bi >= u16_at(base_array, 0)? {
                return None;
            }
            anchor_table = base_array;
            row = bi;
        }
        self.mark_attach(mark_array, mark_index, anchor_table, row, class_count, b, i)
    }

    fn accept_base(&self, idx: usize) -> bool {
        let info = &self.info;
        !info[idx].multiplied()
            || info[idx].comp() == 0
            || idx == 0
            || info[idx - 1].is_mark()
            || !info[idx - 1].multiplied()
            || info[idx].lig_id != info[idx - 1].lig_id
            || info[idx].comp() != info[idx - 1].comp() + 1
    }

    #[allow(clippy::too_many_arguments)]
    fn mark_attach(
        &mut self,
        mark_array: &[u8],
        mark_index: u16,
        anchors: &[u8],
        row: u16,
        class_count: u16,
        base: usize,
        i: usize,
    ) -> Option<usize> {
        if mark_index >= u16_at(mark_array, 0)? {
            return None;
        }
        let rec = 2 + 4 * mark_index as usize;
        let mark_class = u16_at(mark_array, rec)?;
        if mark_class >= class_count {
            return None;
        }
        let mark_anchor = Self::anchor(sub(mark_array, 0, rec + 2)?)?;
        let at = 2 + 2 * (row as usize * class_count as usize + mark_class as usize);
        let base_anchor = Self::anchor(sub(anchors, 0, at)?)?;
        let p = &mut self.pos[i];
        p.x_offset = base_anchor.0 - mark_anchor.0;
        p.y_offset = base_anchor.1 - mark_anchor.1;
        p.attach = ATTACH_MARK;
        p.chain = base as i32 - i as i32;
        Some(i + 1)
    }

    fn mark_to_mark(&mut self, st: &[u8], i: usize) -> Option<usize> {
        if u16_at(st, 0)? != 1 {
            return None;
        }
        let m1 = coverage_index(sub(st, 0, 2)?, self.info[i].glyph)?;
        let flag = self.flag & !GP_IGNORE_FLAGS;
        let j = self.seek(i, false, flag, false, 0, &mut |_| None)?;
        if !self.info[j].is_mark() {
            return None;
        }
        let (id1, id2) = (self.info[i].lig_id, self.info[j].lig_id);
        let (c1, c2) = (self.info[i].comp(), self.info[j].comp());
        let good = if id1 == id2 { id1 == 0 || c1 == c2 } else { (id1 > 0 && c1 == 0) || (id2 > 0 && c2 == 0) };
        if !good {
            return None;
        }
        let m2 = coverage_index(sub(st, 0, 4)?, self.info[j].glyph)?;
        let class_count = u16_at(st, 6)?;
        let mark1_array = sub(st, 0, 8)?;
        let mark2_array = sub(st, 0, 10)?;
        if m2 >= u16_at(mark2_array, 0)? {
            return None;
        }
        self.mark_attach(mark1_array, m1, mark2_array, m2, class_count, j, i)
    }
}

/// Merge the clusters of `info[start..end]` to their minimum, extending the range over neighbours that share the
/// boundary glyphs' clusters (HarfBuzz's merge_clusters, cluster level 0).
pub fn merge_clusters(info: &mut [GlyphInfo], start: usize, end: usize) {
    if end > info.len() || end <= start + 1 {
        return;
    }
    let (mut start, mut end) = (start, end);
    let cluster = info[start..end].iter().map(|g| g.cluster).min().unwrap();
    if cluster != info[end - 1].cluster {
        while end < info.len() && info[end - 1].cluster == info[end].cluster {
            end += 1;
        }
    }
    if cluster != info[start].cluster {
        while start > 0 && info[start - 1].cluster == info[start].cluster {
            start -= 1;
        }
    }
    for g in info[start..end].iter_mut() {
        g.cluster = cluster;
    }
}

fn reverse_cursive_minor_offset(pos: &mut [GlyphPosition], i: usize, new_parent: usize) {
    reverse_cursive(pos, i, new_parent, 64)
}

fn reverse_cursive(pos: &mut [GlyphPosition], i: usize, new_parent: usize, depth: u32) {
    let chain = pos[i].chain;
    let kind = pos[i].attach;
    if chain == 0 || kind & ATTACH_CURSIVE == 0 || depth == 0 {
        return;
    }
    pos[i].chain = 0;
    let j = i as i64 + chain as i64;
    if j < 0 || j as usize >= pos.len() || j as usize == new_parent {
        return;
    }
    let j = j as usize;
    reverse_cursive(pos, j, new_parent, depth - 1);
    pos[j].y_offset = -pos[i].y_offset;
    pos[j].chain = -chain;
    pos[j].attach = kind;
}

/// Accumulate attachment offsets (marks onto bases, cursive chains), HarfBuzz's propagate_attachment_offsets.
pub fn propagate_attachments(pos: &mut [GlyphPosition], rtl: bool) {
    for i in 0..pos.len() {
        propagate(pos, i, rtl, 64);
    }
}

fn propagate(pos: &mut [GlyphPosition], i: usize, rtl: bool, nesting: u32) {
    let chain = pos[i].chain;
    let kind = pos[i].attach;
    if chain == 0 {
        return;
    }
    pos[i].chain = 0;
    let j = i as i64 + chain as i64;
    if j < 0 || j as usize >= pos.len() || nesting == 0 {
        return;
    }
    let j = j as usize;
    propagate(pos, j, rtl, nesting - 1);
    if kind & ATTACH_CURSIVE != 0 {
        pos[i].y_offset += pos[j].y_offset;
    } else {
        pos[i].x_offset += pos[j].x_offset;
        pos[i].y_offset += pos[j].y_offset;
        if j < i {
            if !rtl {
                for k in j..i {
                    pos[i].x_offset -= pos[k].x_advance;
                }
            } else {
                for k in j + 1..=i {
                    pos[i].x_offset += pos[k].x_advance;
                }
            }
        }
    }
}
