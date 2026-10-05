//! OpenType Layout common tables (OpenType spec, "OpenType Layout Common Table Formats"): ScriptList,
//! FeatureList, LookupList, Coverage 1/2, ClassDef 1/2; `GSUB` lookup types 1 (single), 4 (ligature) and 7
//! (extension); `GPOS` lookup types 2 (pair adjustment, formats 1 and 2) and 9 (extension); `GDEF` glyph
//! classes; and the legacy `kern` table (format 0, Microsoft and Apple headers).

use crate::reader::{i16_at, u16_at, u32_at};
use alloc::vec::Vec;

pub type Tag = [u8; 4];

/// Coverage table lookup → coverage index.
pub fn coverage_index(d: &[u8], gid: u16) -> Option<u16> {
    match u16_at(d, 0)? {
        1 => {
            let n = u16_at(d, 2)? as usize;
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let g = u16_at(d, 4 + 2 * mid)?;
                if g < gid {
                    lo = mid + 1;
                } else if g > gid {
                    hi = mid;
                } else {
                    return Some(mid as u16);
                }
            }
            None
        }
        2 => {
            let n = u16_at(d, 2)? as usize;
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let b = 4 + 6 * mid;
                let s = u16_at(d, b)?;
                let e = u16_at(d, b + 2)?;
                if gid < s {
                    hi = mid;
                } else if gid > e {
                    lo = mid + 1;
                } else {
                    return u16_at(d, b + 4)?.checked_add(gid - s);
                }
            }
            None
        }
        _ => None,
    }
}

/// ClassDef table lookup (class 0 when not listed).
pub fn class_of(d: &[u8], gid: u16) -> u16 {
    fn inner(d: &[u8], gid: u16) -> Option<u16> {
        match u16_at(d, 0)? {
            1 => {
                let start = u16_at(d, 2)?;
                let n = u16_at(d, 4)?;
                if gid < start || gid - start >= n {
                    return Some(0);
                }
                u16_at(d, 6 + 2 * (gid - start) as usize)
            }
            2 => {
                let n = u16_at(d, 2)? as usize;
                let (mut lo, mut hi) = (0usize, n);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let b = 4 + 6 * mid;
                    let s = u16_at(d, b)?;
                    let e = u16_at(d, b + 2)?;
                    if gid < s {
                        hi = mid;
                    } else if gid > e {
                        lo = mid + 1;
                    } else {
                        return u16_at(d, b + 4);
                    }
                }
                Some(0)
            }
            _ => Some(0),
        }
    }
    inner(d, gid).unwrap_or(0)
}

pub(crate) fn sub(d: &[u8], base: usize, off_at: usize) -> Option<&[u8]> {
    let off = u16_at(d, off_at)? as usize;
    if off == 0 {
        return None;
    }
    d.get(base.checked_add(off)?..)
}

/// One lookup, its type resolved through any Extension wrapper.
#[derive(Clone, Debug)]
pub struct Lookup<'a> {
    pub kind: u16,
    pub flag: u16,
    /// MarkFilteringSet (meaningful when `flag & LOOKUP_USE_MARK_FILTERING_SET`).
    pub mark_set: u16,
    pub subtables: Vec<&'a [u8]>,
}

/// The ScriptList / FeatureList / LookupList trio shared by GSUB and GPOS.
#[derive(Clone, Copy, Debug)]
pub struct LayoutTable<'a> {
    data: &'a [u8],
    scripts: &'a [u8],
    features: &'a [u8],
    lookups: &'a [u8],
    ext_kind: u16,
}

pub const LOOKUP_IGNORE_BASE: u16 = 0x0002;
pub const LOOKUP_IGNORE_LIGATURES: u16 = 0x0004;
pub const LOOKUP_IGNORE_MARKS: u16 = 0x0008;
pub const LOOKUP_RIGHT_TO_LEFT: u16 = 0x0001;
pub const LOOKUP_USE_MARK_FILTERING_SET: u16 = 0x0010;
pub const LOOKUP_MARK_ATTACHMENT_TYPE: u16 = 0xFF00;

impl<'a> LayoutTable<'a> {
    fn parse(data: &'a [u8], ext_kind: u16) -> Option<Self> {
        if u16_at(data, 0)? != 1 {
            return None;
        }
        Some(LayoutTable {
            data,
            scripts: sub(data, 0, 4)?,
            features: sub(data, 0, 6)?,
            lookups: sub(data, 0, 8)?,
            ext_kind,
        })
    }

    pub fn raw(&self) -> &'a [u8] {
        self.data
    }

    fn find_script(&self, tag: Tag) -> Option<&'a [u8]> {
        let n = u16_at(self.scripts, 0)? as usize;
        for i in 0..n {
            let rec = 2 + 6 * i;
            if self.scripts.get(rec..rec + 4)? == tag {
                return sub(self.scripts, 0, rec + 4);
            }
        }
        None
    }

    /// Script tags present.
    pub fn script_tags(&self) -> Vec<Tag> {
        let n = u16_at(self.scripts, 0).unwrap_or(0) as usize;
        (0..n)
            .filter_map(|i| {
                let s = self.scripts.get(2 + 6 * i..6 + 6 * i)?;
                Some([s[0], s[1], s[2], s[3]])
            })
            .collect()
    }

    /// The lookup indices the given features select for `script` (default language system), sorted and
    /// de-duplicated — the order in which a shaper applies them. Falls back to `DFLT`, `dflt`, then `latn`.
    pub fn lookups_for(&self, script: Tag, features: &[Tag]) -> Vec<u16> {
        let mut out = Vec::new();
        let s = self
            .find_script(script)
            .or_else(|| self.find_script(*b"DFLT"))
            .or_else(|| self.find_script(*b"dflt"))
            .or_else(|| self.find_script(*b"latn"));
        let Some(s) = s else { return out };
        let Some(ls) = sub(s, 0, 0) else { return out };
        let req = u16_at(ls, 2).unwrap_or(0xFFFF);
        let n = u16_at(ls, 4).unwrap_or(0) as usize;
        let mut idxs: Vec<u16> = (0..n).filter_map(|i| u16_at(ls, 6 + 2 * i)).collect();
        if req != 0xFFFF {
            idxs.push(req);
        }
        let nf = u16_at(self.features, 0).unwrap_or(0);
        for fi in idxs {
            if fi >= nf {
                continue;
            }
            let rec = 2 + 6 * fi as usize;
            let Some(tag) = self.features.get(rec..rec + 4) else { continue };
            if !features.iter().any(|t| t == tag) {
                continue;
            }
            let Some(f) = sub(self.features, 0, rec + 4) else { continue };
            let cnt = u16_at(f, 2).unwrap_or(0) as usize;
            for k in 0..cnt {
                if let Some(l) = u16_at(f, 4 + 2 * k) {
                    out.push(l);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The LangSys (default language) for the first of `scripts` present, falling back to `DFLT`, `dflt`, `latn`.
    fn lang_sys(&self, scripts: &[Tag]) -> Option<&'a [u8]> {
        let s = scripts
            .iter()
            .find_map(|&t| self.find_script(t))
            .or_else(|| self.find_script(*b"DFLT"))
            .or_else(|| self.find_script(*b"dflt"))
            .or_else(|| self.find_script(*b"latn"))?;
        sub(s, 0, 0)
    }

    /// Whether any of `scripts` is present (no fallback).
    pub fn has_script(&self, scripts: &[Tag]) -> bool {
        scripts.iter().any(|&t| self.find_script(t).is_some())
    }

    /// The lookup indices of `feature` in the default LangSys of the first matching script (None if the feature is
    /// not in that LangSys). The required feature counts under its own tag.
    pub fn feature_lookups(&self, scripts: &[Tag], feature: Tag) -> Option<Vec<u16>> {
        let ls = self.lang_sys(scripts)?;
        let req = u16_at(ls, 2).unwrap_or(0xFFFF);
        let n = u16_at(ls, 4).unwrap_or(0) as usize;
        let nf = u16_at(self.features, 0).unwrap_or(0);
        let mut found = false;
        let mut out = Vec::new();
        let idxs = (0..n).filter_map(|i| u16_at(ls, 6 + 2 * i)).chain((req != 0xFFFF).then_some(req));
        for fi in idxs {
            if fi >= nf {
                continue;
            }
            let rec = 2 + 6 * fi as usize;
            if self.features.get(rec..rec + 4) != Some(&feature[..]) {
                continue;
            }
            found = true;
            let Some(f) = sub(self.features, 0, rec + 4) else { continue };
            let cnt = u16_at(f, 2).unwrap_or(0) as usize;
            out.extend((0..cnt).filter_map(|k| u16_at(f, 4 + 2 * k)));
        }
        found.then_some(out)
    }

    pub fn lookup_count(&self) -> usize {
        u16_at(self.lookups, 0).unwrap_or(0) as usize
    }

    pub fn lookup(&self, i: u16) -> Option<Lookup<'a>> {
        if i as usize >= self.lookup_count() {
            return None;
        }
        let l = sub(self.lookups, 0, 2 + 2 * i as usize)?;
        let mut kind = u16_at(l, 0)?;
        let is_ext = kind == self.ext_kind;
        let flag = u16_at(l, 2)?;
        let n = u16_at(l, 4)? as usize;
        let mark_set = if flag & LOOKUP_USE_MARK_FILTERING_SET != 0 { u16_at(l, 6 + 2 * n).unwrap_or(0) } else { 0 };
        let mut subtables = Vec::with_capacity(n);
        for k in 0..n {
            let Some(st) = sub(l, 0, 6 + 2 * k) else { continue };
            if is_ext {
                // Extension: format 1, extensionLookupType, Offset32 from the extension subtable.
                if u16_at(st, 0)? != 1 {
                    continue;
                }
                let real = u16_at(st, 2)?;
                let off = u32_at(st, 4)? as usize;
                let Some(t) = st.get(off..) else { continue };
                kind = real;
                subtables.push(t);
            } else {
                subtables.push(st);
            }
        }
        Some(Lookup { kind, flag, mark_set, subtables })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Gsub<'a>(pub LayoutTable<'a>);
#[derive(Clone, Copy, Debug)]
pub struct Gpos<'a>(pub LayoutTable<'a>);

impl<'a> Gsub<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        LayoutTable::parse(d, 7).map(Gsub)
    }
}
impl<'a> Gpos<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        LayoutTable::parse(d, 9).map(Gpos)
    }
}

/// GSUB lookup type 1 — single substitution.
pub fn single_subst(st: &[u8], gid: u16) -> Option<u16> {
    let cov = sub(st, 0, 2)?;
    let ci = coverage_index(cov, gid)?;
    match u16_at(st, 0)? {
        1 => Some(gid.wrapping_add(i16_at(st, 4)? as u16)),
        2 => {
            let n = u16_at(st, 4)?;
            if ci >= n {
                return None;
            }
            u16_at(st, 6 + 2 * ci as usize)
        }
        _ => None,
    }
}

/// GSUB lookup type 4 — ligature substitution. `next(k)` yields the k-th following (non-skipped) glyph.
/// Returns (ligature glyph, number of components consumed including the first).
pub fn ligature_subst(st: &[u8], gid: u16, next: impl Fn(usize) -> Option<u16>) -> Option<(u16, usize)> {
    if u16_at(st, 0)? != 1 {
        return None;
    }
    let cov = sub(st, 0, 2)?;
    let ci = coverage_index(cov, gid)?;
    let nsets = u16_at(st, 4)?;
    if ci >= nsets {
        return None;
    }
    let set = sub(st, 0, 6 + 2 * ci as usize)?;
    let nl = u16_at(set, 0)? as usize;
    'lig: for k in 0..nl {
        let Some(lig) = sub(set, 0, 2 + 2 * k) else { continue };
        let Some(lg) = u16_at(lig, 0) else { continue };
        let Some(cc) = u16_at(lig, 2) else { continue };
        if cc == 0 {
            continue;
        }
        for j in 1..cc as usize {
            let Some(want) = u16_at(lig, 4 + 2 * (j - 1)) else { continue 'lig };
            if next(j) != Some(want) {
                continue 'lig;
            }
        }
        return Some((lg, cc as usize));
    }
    None
}

/// A decoded ValueRecord (device tables ignored: no hinting).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Value {
    pub x_placement: i16,
    pub y_placement: i16,
    pub x_advance: i16,
    pub y_advance: i16,
}

pub(crate) fn value_size(fmt: u16) -> usize {
    (fmt & 0xFF).count_ones() as usize * 2
}

pub(crate) fn read_value(d: &[u8], off: usize, fmt: u16) -> Option<Value> {
    let mut v = Value::default();
    let mut p = off;
    if fmt & 1 != 0 {
        v.x_placement = i16_at(d, p)?;
        p += 2;
    }
    if fmt & 2 != 0 {
        v.y_placement = i16_at(d, p)?;
        p += 2;
    }
    if fmt & 4 != 0 {
        v.x_advance = i16_at(d, p)?;
        p += 2;
    }
    if fmt & 8 != 0 {
        v.y_advance = i16_at(d, p)?;
    }
    Some(v)
}

/// GPOS lookup type 2 — pair adjustment. Returns (value for first, value for second, second has a record).
pub fn pair_adjust(st: &[u8], first: u16, second: u16) -> Option<(Value, Value, bool)> {
    let cov = sub(st, 0, 2)?;
    let ci = coverage_index(cov, first)?;
    let vf1 = u16_at(st, 4)?;
    let vf2 = u16_at(st, 6)?;
    let (s1, s2) = (value_size(vf1), value_size(vf2));
    match u16_at(st, 0)? {
        1 => {
            let n = u16_at(st, 8)?;
            if ci >= n {
                return None;
            }
            let ps = sub(st, 0, 10 + 2 * ci as usize)?;
            let cnt = u16_at(ps, 0)? as usize;
            let rec = 2 + s1 + s2;
            let (mut lo, mut hi) = (0usize, cnt);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let b = 2 + rec * mid;
                let g = u16_at(ps, b)?;
                if g < second {
                    lo = mid + 1;
                } else if g > second {
                    hi = mid;
                } else {
                    return Some((read_value(ps, b + 2, vf1)?, read_value(ps, b + 2 + s1, vf2)?, vf2 != 0));
                }
            }
            None
        }
        2 => {
            let cd1 = sub(st, 0, 8)?;
            let cd2 = sub(st, 0, 10)?;
            let n1 = u16_at(st, 12)?;
            let n2 = u16_at(st, 14)?;
            let c1 = class_of(cd1, first);
            let c2 = class_of(cd2, second);
            if c1 >= n1 || c2 >= n2 {
                return None;
            }
            let rec = s1 + s2;
            let b = 16 + (c1 as usize * n2 as usize + c2 as usize) * rec;
            Some((read_value(st, b, vf1)?, read_value(st, b + s1, vf2)?, vf2 != 0))
        }
        _ => None,
    }
}

/// `GDEF`: glyph classes (1 base, 2 ligature, 3 mark, 4 component), mark attachment classes and (v1.2+) mark
/// glyph sets.
#[derive(Clone, Copy, Debug)]
pub struct Gdef<'a> {
    glyph_classes: Option<&'a [u8]>,
    mark_attach: Option<&'a [u8]>,
    mark_sets: Option<&'a [u8]>,
}

impl<'a> Gdef<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        if u16_at(d, 0)? != 1 {
            return None;
        }
        let minor = u16_at(d, 2)?;
        let mark_sets = if minor >= 2 { sub(d, 0, 12) } else { None };
        Some(Gdef { glyph_classes: sub(d, 0, 4), mark_attach: sub(d, 0, 10), mark_sets })
    }
    pub fn has_glyph_classes(&self) -> bool {
        self.glyph_classes.is_some()
    }
    pub fn glyph_class(&self, gid: u16) -> u16 {
        self.glyph_classes.map_or(0, |c| class_of(c, gid))
    }
    pub fn mark_attach_class(&self, gid: u16) -> u16 {
        self.mark_attach.map_or(0, |c| class_of(c, gid))
    }
    /// Whether mark glyph set `set` (MarkGlyphSetsDef) covers `gid`.
    pub fn mark_set_covers(&self, set: u16, gid: u16) -> bool {
        let Some(m) = self.mark_sets else { return false };
        if u16_at(m, 0) != Some(1) {
            return false;
        }
        let n = u16_at(m, 2).unwrap_or(0);
        if set >= n {
            return false;
        }
        let Some(off) = u32_at(m, 4 + 4 * set as usize) else { return false };
        m.get(off as usize..).and_then(|c| coverage_index(c, gid)).is_some()
    }
    /// Whether a lookup with `flag` skips `gid`.
    pub fn skips(&self, flag: u16, gid: u16) -> bool {
        match self.glyph_class(gid) {
            1 => flag & LOOKUP_IGNORE_BASE != 0,
            2 => flag & LOOKUP_IGNORE_LIGATURES != 0,
            3 => flag & LOOKUP_IGNORE_MARKS != 0,
            _ => false,
        }
    }
}

/// The legacy `kern` table, horizontal format-0 subtables only.
#[derive(Clone, Copy, Debug)]
pub struct Kern<'a> {
    data: &'a [u8],
    apple: bool,
}

impl<'a> Kern<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        let v = u16_at(d, 0)?;
        if v == 0 {
            Some(Kern { data: d, apple: false })
        } else if u32_at(d, 0)? == 0x0001_0000 {
            Some(Kern { data: d, apple: true })
        } else {
            None
        }
    }

    /// Format-0 subtables as (pairs data, pair count).
    fn subtables(&self) -> Vec<(&'a [u8], usize)> {
        let mut out = Vec::new();
        let d = self.data;
        let (n, mut off) = if self.apple {
            (u32_at(d, 4).unwrap_or(0) as usize, 8usize)
        } else {
            (u16_at(d, 2).unwrap_or(0) as usize, 4usize)
        };
        for _ in 0..n.min(64) {
            let (len, fmt, horizontal, cross, hdr) = if self.apple {
                let Some(len) = u32_at(d, off) else { break };
                let Some(cov) = u16_at(d, off + 4) else { break };
                (len as usize, cov & 0xFF, cov & 0x8000 == 0, cov & 0x4000 != 0, 8usize)
            } else {
                let Some(len) = u16_at(d, off + 2) else { break };
                let Some(cov) = u16_at(d, off + 4) else { break };
                (len as usize, cov >> 8, cov & 1 != 0, cov & 4 != 0, 6usize)
            };
            let np = u16_at(d, off + hdr).unwrap_or(0) as usize;
            // A big format-0 subtable overflows the 16-bit length field; trust nPairs instead.
            let len = if fmt == 0 { len.max(hdr + 8 + 6 * np) } else { len };
            if fmt == 0 && horizontal && !cross {
                {
                    if let Some(p) = d.get(off + hdr + 8..) {
                        out.push((p, np));
                    }
                }
            }
            if len == 0 {
                break;
            }
            off = off.saturating_add(len);
        }
        out
    }

    /// Kerning between a glyph pair (font units), summed over subtables.
    pub fn pair(&self, left: u16, right: u16) -> i16 {
        let key = ((left as u32) << 16) | right as u32;
        let mut total: i32 = 0;
        for (p, n) in self.subtables() {
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let Some(k) = u32_at(p, 6 * mid) else { break };
                if k < key {
                    lo = mid + 1;
                } else if k > key {
                    hi = mid;
                } else {
                    total += i16_at(p, 6 * mid + 4).unwrap_or(0) as i32;
                    break;
                }
            }
        }
        total.clamp(i16::MIN as i32, i16::MAX as i32) as i16
    }

    pub fn pair_count(&self) -> usize {
        self.subtables().iter().map(|s| s.1).sum()
    }
}
