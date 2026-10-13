// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FONTSCRAFT (rmbp-ledger B640, R124): OpenType font variations, from the specification ("OpenType Font
//! Variations Overview", `fvar`, `avar`, `gvar`, `HVAR`, "Item Variation Store", "DeltaSetIndexMap").
//!
//! * [`Fvar`]: the axes (tag, min, default, max) and [`Fvar::normalize`] — user value → default-relative
//!   normalized coordinate in [-1, 1], rounded to the F2Dot14 grid the way the spec says, then through `avar`'s
//!   segment maps ([`Avar`], version 1 maps) and rounded again.
//! * [`ItemVariationStore`]: region lists and delta sets (16/8-bit and 32/16-bit rows); [`region_scalar`].
//! * [`Hvar`]: advance-width deltas through the optional DeltaSetIndexMap.
//! * [`Mvar`] (FONTS2, B650): the metrics variations table — per-instance deltas of the line metrics (`hasc`,
//!   `hdsc`, `hlgp`), the clipping box (`hcla`, `hcld`), x-height and cap height (`xhgt`, `cpht`), the underline
//!   (`undo`, `unds`) and every other tag the table carries, through its own ItemVariationStore.
//! * [`Gvar`]: per-glyph tuple variations — shared and embedded peaks, intermediate regions, shared and private
//!   packed point numbers, packed deltas (zero / byte / word / long runs) and the interpolation of untouched points
//!   (IUP) contour by contour; the four phantom points carry the advance when a face has no `HVAR`.
//!
//! The caller holds normalized coordinates ([`MAX_AXES`] of them, `f32` on the F2Dot14 grid) in the [`crate::Font`]
//! it instanced; every function here is a pure read of the table bytes. Nothing allocates per glyph beyond the
//! delta vectors; a malformed table yields no deltas (the default instance), never a panic.

use crate::reader::{i16_at, slice, u16_at, u32_at, Reader};
use alloc::vec;
use alloc::vec::Vec;

/// The most axes an instance carries (Inter has 2, Noto Sans 2, Roboto Flex 13 — the extra are ignored).
pub const MAX_AXES: usize = 8;

/// One `fvar` axis, in user units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub tag: [u8; 4],
    pub min: f32,
    pub default: f32,
    pub max: f32,
    /// `HIDDEN_AXIS` (0x0001) and the axis name id, as the table stores them.
    pub flags: u16,
    pub name_id: u16,
}

fn fixed(v: u32) -> f32 {
    v as i32 as f32 / 65536.0
}

/// Round to the F2Dot14 grid (the spec rounds the normalized value before and after `avar`).
pub fn f2dot14_round(v: f32) -> f32 {
    crate::fmath::round(v * 16384.0) / 16384.0
}

/// `fvar` — font variations table.
#[derive(Clone, Copy, Debug)]
pub struct Fvar<'a> {
    data: &'a [u8],
    axes_off: usize,
    pub axis_count: u16,
    axis_size: usize,
}

impl<'a> Fvar<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        if u16_at(d, 0)? != 1 {
            return None;
        }
        let axes_off = u16_at(d, 4)? as usize;
        let axis_count = u16_at(d, 8)?;
        let axis_size = u16_at(d, 10)? as usize;
        if axis_size < 20 || axis_count == 0 {
            return None;
        }
        slice(d, axes_off, axis_count as usize * axis_size)?;
        Some(Fvar { data: d, axes_off, axis_count, axis_size })
    }

    pub fn axis(&self, i: usize) -> Option<Axis> {
        if i >= self.axis_count as usize {
            return None;
        }
        let o = self.axes_off + i * self.axis_size;
        let t = slice(self.data, o, 4)?;
        let mut a = Axis {
            tag: [t[0], t[1], t[2], t[3]],
            min: fixed(u32_at(self.data, o + 4)?),
            default: fixed(u32_at(self.data, o + 8)?),
            max: fixed(u32_at(self.data, o + 12)?),
            flags: u16_at(self.data, o + 16)?,
            name_id: u16_at(self.data, o + 18)?,
        };
        // The spec requires min <= default <= max; a table that breaks it is read as a fixed axis.
        if !(a.min <= a.default && a.default <= a.max) {
            a.min = a.default;
            a.max = a.default;
        }
        Some(a)
    }

    pub fn axes(&self) -> Vec<Axis> {
        (0..self.axis_count as usize).filter_map(|i| self.axis(i)).collect()
    }

    /// The index of axis `tag`.
    pub fn index_of(&self, tag: [u8; 4]) -> Option<usize> {
        (0..self.axis_count as usize).find(|&i| self.axis(i).is_some_and(|a| a.tag == tag))
    }

    /// Default normalization of a user value on axis `i` (no `avar`), on the F2Dot14 grid.
    pub fn default_normalize(&self, i: usize, v: f32) -> f32 {
        let Some(a) = self.axis(i) else { return 0.0 };
        let v = v.clamp(a.min, a.max);
        let n = if v < a.default {
            if a.default > a.min { (v - a.default) / (a.default - a.min) } else { 0.0 }
        } else if v > a.default {
            if a.max > a.default { (v - a.default) / (a.max - a.default) } else { 0.0 }
        } else {
            0.0
        };
        f2dot14_round(n.clamp(-1.0, 1.0))
    }

    /// Full normalization of a user value on axis `i`: default normalization, then `avar`'s map for that axis.
    pub fn normalize(&self, i: usize, v: f32, avar: Option<&Avar>) -> f32 {
        let n = self.default_normalize(i, v);
        match avar {
            Some(m) => f2dot14_round(m.map(i, n)),
            None => n,
        }
    }
}

/// `avar` — axis variations (version 1 segment maps; a version 2 table's extra item variation store is not read,
/// its version 1 part still applies).
#[derive(Clone, Copy, Debug)]
pub struct Avar<'a> {
    data: &'a [u8],
    axis_count: u16,
}

impl<'a> Avar<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        let major = u16_at(d, 0)?;
        if major != 1 && major != 2 {
            return None;
        }
        Some(Avar { data: d, axis_count: u16_at(d, 6)? })
    }

    /// Map normalized coordinate `n` of axis `axis` through its segment map (identity when the map is absent or
    /// has fewer than the required -1/0/1 entries — the spec's "ignore a malformed map").
    pub fn map(&self, axis: usize, n: f32) -> f32 {
        if axis >= self.axis_count as usize {
            return n;
        }
        let mut off = 8usize;
        for _ in 0..axis {
            let Some(c) = u16_at(self.data, off) else { return n };
            off += 2 + 4 * c as usize;
        }
        let Some(count) = u16_at(self.data, off) else { return n };
        let count = count as usize;
        if count < 3 {
            return n;
        }
        let pair = |k: usize| -> Option<(f32, f32)> {
            let p = off + 2 + 4 * k;
            Some((i16_at(self.data, p)? as f32 / 16384.0, i16_at(self.data, p + 2)? as f32 / 16384.0))
        };
        let Some(first) = pair(0) else { return n };
        if n <= first.0 {
            return first.1;
        }
        let mut prev = first;
        for k in 1..count {
            let Some(cur) = pair(k) else { return n };
            if n <= cur.0 {
                if cur.0 == prev.0 {
                    return cur.1;
                }
                return prev.1 + (cur.1 - prev.1) * (n - prev.0) / (cur.0 - prev.0);
            }
            prev = cur;
        }
        prev.1
    }
}

/// The scalar of one variation region for `coords` (Item Variation Store: "Algorithm for interpolation of
/// instance values"). Each axis triple is (start, peak, end) in F2Dot14.
pub fn region_scalar(axes: &[(f32, f32, f32)], coords: &[f32]) -> f32 {
    let mut s = 1.0f32;
    for (i, &(start, peak, end)) in axes.iter().enumerate() {
        let v = coords.get(i).copied().unwrap_or(0.0);
        if peak == 0.0 || start > peak || peak > end || (start < 0.0 && end > 0.0) || v == peak {
            continue;
        }
        if v <= start || v >= end {
            return 0.0;
        }
        s *= if v < peak { (v - start) / (peak - start) } else { (end - v) / (end - peak) };
    }
    s
}

/// An ItemVariationStore (`HVAR`, `MVAR`, `GDEF`, …).
#[derive(Clone, Copy, Debug)]
pub struct ItemVariationStore<'a> {
    data: &'a [u8],
    regions: &'a [u8],
    region_axes: u16,
    region_count: u16,
    data_count: u16,
}

impl<'a> ItemVariationStore<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        if u16_at(d, 0)? != 1 {
            return None;
        }
        let rl = u32_at(d, 2)? as usize;
        let regions = d.get(rl..)?;
        let region_axes = u16_at(regions, 0)?;
        let region_count = u16_at(regions, 2)?;
        slice(regions, 4, region_axes as usize * region_count as usize * 6)?;
        Some(ItemVariationStore { data: d, regions, region_axes, region_count, data_count: u16_at(d, 6)? })
    }

    fn region(&self, r: u16, coords: &[f32]) -> f32 {
        if r >= self.region_count {
            return 0.0;
        }
        let n = self.region_axes as usize;
        let mut axes = [(0f32, 0f32, 0f32); MAX_AXES];
        let base = 4 + r as usize * n * 6;
        let f = |o: usize| i16_at(self.regions, o).map_or(0.0, |v| v as f32 / 16384.0);
        for (a, slot) in axes.iter_mut().enumerate().take(n.min(MAX_AXES)) {
            let o = base + a * 6;
            *slot = (f(o), f(o + 2), f(o + 4));
        }
        // An axis past MAX_AXES with a non-zero peak cannot be evaluated: the coordinate there is the default (0),
        // so its factor is 0 unless the peak is 0 — read it exactly.
        for a in MAX_AXES..n {
            let o = base + a * 6;
            let (start, peak, end) = (f(o), f(o + 2), f(o + 4));
            if peak != 0.0 && !(start > peak || peak > end || (start < 0.0 && end > 0.0)) {
                return 0.0;
            }
        }
        region_scalar(&axes[..n.min(MAX_AXES)], coords)
    }

    /// The interpolated delta of item (`outer`, `inner`) at `coords`. 0 for an index outside the store.
    pub fn delta(&self, outer: u16, inner: u16, coords: &[f32]) -> f32 {
        if outer >= self.data_count || outer == 0xFFFF {
            return 0.0;
        }
        let Some(off) = u32_at(self.data, 8 + 4 * outer as usize) else { return 0.0 };
        let Some(ivd) = self.data.get(off as usize..) else { return 0.0 };
        let (Some(items), Some(wdc), Some(nreg)) = (u16_at(ivd, 0), u16_at(ivd, 2), u16_at(ivd, 4)) else { return 0.0 };
        if inner >= items {
            return 0.0;
        }
        let long = wdc & 0x8000 != 0;
        let words = (wdc & 0x7FFF) as usize;
        let nreg = nreg as usize;
        if words > nreg {
            return 0.0;
        }
        let row = if long { words * 4 + (nreg - words) * 2 } else { words * 2 + (nreg - words) };
        let rows = 6 + 2 * nreg;
        let at = rows + inner as usize * row;
        let mut r = match Reader::at(ivd, at) {
            Some(r) => r,
            None => return 0.0,
        };
        let mut sum = 0f32;
        for k in 0..nreg {
            let d = if k < words {
                if long { r.i32() } else { r.i16().map(|v| v as i32) }
            } else if long {
                r.i16().map(|v| v as i32)
            } else {
                r.i8().map(|v| v as i32)
            };
            let Some(d) = d else { return 0.0 };
            if d == 0 {
                continue;
            }
            let Some(ri) = u16_at(ivd, 6 + 2 * k) else { return 0.0 };
            let s = self.region(ri, coords);
            if s != 0.0 {
                sum += d as f32 * s;
            }
        }
        sum
    }
}

/// A DeltaSetIndexMap: glyph (or item) → (outer, inner). `None` means the implicit map (outer 0, inner = index).
#[derive(Clone, Copy, Debug)]
pub struct DeltaSetIndexMap<'a> {
    data: &'a [u8],
    fmt: u8,
    entry: u8,
    count: u32,
}

impl<'a> DeltaSetIndexMap<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        let fmt = *d.first()?;
        let entry = *d.get(1)?;
        let count = match fmt {
            0 => u16_at(d, 2)? as u32,
            1 => u32_at(d, 2)?,
            _ => return None,
        };
        Some(DeltaSetIndexMap { data: d, fmt, entry, count })
    }

    pub fn get(&self, i: u32) -> Option<(u16, u16)> {
        if self.count == 0 {
            return None;
        }
        let i = i.min(self.count - 1) as usize;
        let size = (((self.entry & 0x30) >> 4) + 1) as usize;
        let inner_bits = ((self.entry & 0x0F) + 1) as u32;
        let base = if self.fmt == 0 { 4 } else { 6 };
        let b = slice(self.data, base + i * size, size)?;
        let mut v = 0u32;
        for &x in b {
            v = (v << 8) | x as u32;
        }
        Some(((v >> inner_bits) as u16, (v & ((1u32 << inner_bits) - 1)) as u16))
    }
}

/// `HVAR` — horizontal metrics variations (advance widths; side-bearing maps are not needed by a renderer that
/// takes bearings from the varied outline).
#[derive(Clone, Copy, Debug)]
pub struct Hvar<'a> {
    pub store: ItemVariationStore<'a>,
    advance_map: Option<DeltaSetIndexMap<'a>>,
}

impl<'a> Hvar<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        if u16_at(d, 0)? != 1 {
            return None;
        }
        let store = ItemVariationStore::parse(d.get(u32_at(d, 4)? as usize..)?)?;
        let am = u32_at(d, 8)? as usize;
        let advance_map = if am == 0 { None } else { Some(DeltaSetIndexMap::parse(d.get(am..)?)?) };
        Some(Hvar { store, advance_map })
    }

    /// The advance-width delta of `gid` at `coords`, in font units (fractional).
    pub fn advance_delta(&self, gid: u16, coords: &[f32]) -> f32 {
        let (outer, inner) = match &self.advance_map {
            Some(m) => match m.get(gid as u32) {
                Some(e) => e,
                None => return 0.0,
            },
            None => (0, gid),
        };
        self.store.delta(outer, inner, coords)
    }
}

/// `MVAR` — metrics variations (OpenType 1.9, "MVAR — Metrics Variations Table", FONTS2 B650). The header is
/// majorVersion 1, minorVersion, reserved, valueRecordSize (≥ 8), valueRecordCount, itemVariationStoreOffset
/// (Offset16 from the table start); then valueRecordCount ValueRecords of valueTag, deltaSetOuterIndex,
/// deltaSetInnerIndex, sorted by tag. A metric's delta at an instance is its delta set interpolated in the store;
/// a tag the table does not carry has delta 0, as does a table without a store.
#[derive(Clone, Copy, Debug)]
pub struct Mvar<'a> {
    data: &'a [u8],
    rec_size: usize,
    rec_count: usize,
    store: ItemVariationStore<'a>,
}

/// The `MVAR` value tags the kernel's text seam reads (the spec's table of tags; each names the sfnt field it moves).
pub const MVAR_HASC: [u8; 4] = *b"hasc";
pub const MVAR_HDSC: [u8; 4] = *b"hdsc";
pub const MVAR_HLGP: [u8; 4] = *b"hlgp";
pub const MVAR_HCLA: [u8; 4] = *b"hcla";
pub const MVAR_HCLD: [u8; 4] = *b"hcld";
pub const MVAR_XHGT: [u8; 4] = *b"xhgt";
pub const MVAR_CPHT: [u8; 4] = *b"cpht";
pub const MVAR_UNDO: [u8; 4] = *b"undo";
pub const MVAR_UNDS: [u8; 4] = *b"unds";

impl<'a> Mvar<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        if u16_at(d, 0)? != 1 {
            return None;
        }
        let rec_size = u16_at(d, 6)? as usize;
        let rec_count = u16_at(d, 8)? as usize;
        let ivs = u16_at(d, 10)? as usize;
        if rec_size < 8 || ivs == 0 {
            return None;
        }
        slice(d, 12, rec_count * rec_size)?;
        let store = ItemVariationStore::parse(d.get(ivs..)?)?;
        Some(Mvar { data: d, rec_size, rec_count, store })
    }

    /// How many value records the table carries.
    pub fn record_count(&self) -> usize {
        self.rec_count
    }

    /// The value tags, in table order.
    pub fn tags(&self) -> Vec<[u8; 4]> {
        (0..self.rec_count)
            .filter_map(|i| {
                let t = slice(self.data, 12 + i * self.rec_size, 4)?;
                Some([t[0], t[1], t[2], t[3]])
            })
            .collect()
    }

    /// The table carries `tag`.
    pub fn has(&self, tag: [u8; 4]) -> bool {
        self.record(tag).is_some()
    }

    fn record(&self, tag: [u8; 4]) -> Option<(u16, u16)> {
        for i in 0..self.rec_count {
            let o = 12 + i * self.rec_size;
            if slice(self.data, o, 4)? == tag {
                return Some((u16_at(self.data, o + 4)?, u16_at(self.data, o + 6)?));
            }
        }
        None
    }

    /// The delta of metric `tag` at `coords`, in fractional font units (0 for a tag the table does not carry).
    pub fn delta(&self, tag: [u8; 4], coords: &[f32]) -> f32 {
        match self.record(tag) {
            Some((outer, inner)) => self.store.delta(outer, inner, coords),
            None => 0.0,
        }
    }
}

/// `gvar` — glyph variations.
#[derive(Clone, Copy, Debug)]
pub struct Gvar<'a> {
    data: &'a [u8],
    axis_count: u16,
    shared: &'a [u8],
    shared_count: u16,
    glyph_count: u16,
    long_offsets: bool,
    array: usize,
}

const EMBEDDED_PEAK_TUPLE: u16 = 0x8000;
const INTERMEDIATE_REGION: u16 = 0x4000;
const PRIVATE_POINT_NUMBERS: u16 = 0x2000;
const TUPLE_INDEX_MASK: u16 = 0x0FFF;
const SHARED_POINT_NUMBERS: u16 = 0x8000;
const COUNT_MASK: u16 = 0x0FFF;

/// Packed point numbers: `None` = every point.
fn read_points(r: &mut Reader) -> Option<Option<Vec<u16>>> {
    let b0 = r.u8()? as usize;
    let count = if b0 == 0 {
        return Some(None);
    } else if b0 & 0x80 != 0 {
        ((b0 & 0x7F) << 8) | r.u8()? as usize
    } else {
        b0
    };
    let mut pts = Vec::with_capacity(count);
    let mut last = 0u16;
    while pts.len() < count {
        let ctl = r.u8()?;
        let run = (ctl & 0x7F) as usize + 1;
        for _ in 0..run {
            if pts.len() >= count {
                break;
            }
            let d = if ctl & 0x80 != 0 { r.u16()? } else { r.u8()? as u16 };
            last = last.wrapping_add(d);
            pts.push(last);
        }
    }
    Some(Some(pts))
}

/// Packed deltas: `count` values.
fn read_deltas(r: &mut Reader, count: usize) -> Option<Vec<i32>> {
    let mut v = Vec::with_capacity(count);
    while v.len() < count {
        let ctl = r.u8()?;
        let run = (ctl & 0x3F) as usize + 1;
        for _ in 0..run {
            if v.len() >= count {
                break;
            }
            let d = match ctl & 0xC0 {
                0x80 => 0,
                0x40 => r.i16()? as i32,
                0xC0 => r.i32()?,
                _ => r.i8()? as i32,
            };
            v.push(d);
        }
    }
    Some(v)
}

/// Interpolate untouched points (gvar "Inferred deltas for un-referenced point numbers"), one axis.
fn iup_axis(orig: &[f32], delta: &mut [f32], touched: &[bool], start: usize, end: usize) {
    let idx: Vec<usize> = (start..=end).filter(|&i| touched[i]).collect();
    if idx.is_empty() {
        return;
    }
    if idx.len() == 1 {
        let d = delta[idx[0]];
        for i in start..=end {
            if !touched[i] {
                delta[i] = d;
            }
        }
        return;
    }
    let n = end - start + 1;
    for (k, &p1) in idx.iter().enumerate() {
        let p2 = idx[(k + 1) % idx.len()];
        // Untouched points strictly between p1 and p2, walking forward around the contour.
        let mut i = if p1 == end { start } else { p1 + 1 };
        let mut steps = 0;
        while i != p2 && steps < n {
            let (a, b) = (orig[p1], orig[p2]);
            let (da, db) = (delta[p1], delta[p2]);
            let x = orig[i];
            delta[i] = if a == b {
                if da == db { da } else { 0.0 }
            } else {
                let (lo, hi, dlo, dhi) = if a < b { (a, b, da, db) } else { (b, a, db, da) };
                if x <= lo {
                    dlo
                } else if x >= hi {
                    dhi
                } else {
                    dlo + (x - lo) * (dhi - dlo) / (hi - lo)
                }
            };
            i = if i == end { start } else { i + 1 };
            steps += 1;
        }
    }
}

impl<'a> Gvar<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Self> {
        if u16_at(d, 0)? != 1 {
            return None;
        }
        let axis_count = u16_at(d, 4)?;
        let shared_count = u16_at(d, 6)?;
        let shared_off = u32_at(d, 8)? as usize;
        let glyph_count = u16_at(d, 12)?;
        let flags = u16_at(d, 14)?;
        let array = u32_at(d, 16)? as usize;
        let shared = slice(d, shared_off, shared_count as usize * axis_count as usize * 2).unwrap_or(&[]);
        let long_offsets = flags & 1 != 0;
        slice(d, 20, (glyph_count as usize + 1) * if long_offsets { 4 } else { 2 })?;
        Some(Gvar { data: d, axis_count, shared, shared_count, glyph_count, long_offsets, array })
    }

    fn glyph_data(&self, gid: u16) -> Option<&'a [u8]> {
        if gid >= self.glyph_count {
            return None;
        }
        let i = gid as usize;
        let (a, b) = if self.long_offsets {
            (u32_at(self.data, 20 + 4 * i)? as usize, u32_at(self.data, 24 + 4 * i)? as usize)
        } else {
            (u16_at(self.data, 20 + 2 * i)? as usize * 2, u16_at(self.data, 22 + 2 * i)? as usize * 2)
        };
        if b <= a {
            return None;
        }
        slice(self.data, self.array + a, b - a)
    }

    fn tuple(&self, d: &[u8], off: usize, k: usize) -> f32 {
        i16_at(d, off + 2 * k).map_or(0.0, |v| v as f32 / 16384.0)
    }

    /// The scalar of one tuple variation for `coords`.
    fn tuple_scalar(&self, peak: &[f32], inter: Option<(&[f32], &[f32])>, coords: &[f32]) -> f32 {
        let mut s = 1.0f32;
        for (i, &p) in peak.iter().enumerate() {
            if p == 0.0 {
                continue;
            }
            let v = coords.get(i).copied().unwrap_or(0.0);
            if v == p {
                continue;
            }
            if let Some((st, en)) = inter {
                let (a, b) = (st[i], en[i]);
                if v < a || v > b {
                    return 0.0;
                }
                s *= if v < p {
                    if p == a { 1.0 } else { (v - a) / (p - a) }
                } else if p == b {
                    1.0
                } else {
                    (b - v) / (b - p)
                };
            } else {
                if v == 0.0 || v < p.min(0.0) || v > p.max(0.0) {
                    return 0.0;
                }
                s *= v / p;
            }
        }
        s
    }

    /// The (dx, dy) deltas of glyph `gid` with `n` points (the outline's points then the four phantom points; for a
    /// composite, one per component then the four phantoms) at `coords`. `orig` and `ends` (contour end indices) are
    /// needed to infer untouched points; pass `None` for a composite (no inference: untouched means zero).
    /// `None` when the glyph has no variation data or no tuple applies.
    pub fn deltas(&self, gid: u16, n: usize, outline: Option<(&[(f32, f32)], &[usize])>, coords: &[f32]) -> Option<Vec<(f32, f32)>> {
        if coords.iter().all(|&c| c == 0.0) {
            return None;
        }
        let gd = self.glyph_data(gid)?;
        let tvc = u16_at(gd, 0)?;
        let count = (tvc & COUNT_MASK) as usize;
        let data_off = u16_at(gd, 2)? as usize;
        let mut ser = Reader::at(gd, data_off)?;
        let shared_pts = if tvc & SHARED_POINT_NUMBERS != 0 { read_points(&mut ser)? } else { None };
        let ac = self.axis_count as usize;
        let mut out: Vec<(f32, f32)> = vec![(0.0, 0.0); n];
        let mut any = false;
        let mut hdr = 4usize;
        let mut peak = [0f32; 64];
        let mut ist = [0f32; 64];
        let mut ien = [0f32; 64];
        if ac > 64 {
            return None;
        }
        for _ in 0..count {
            let size = u16_at(gd, hdr)? as usize;
            let ti = u16_at(gd, hdr + 2)?;
            let mut p = hdr + 4;
            if ti & EMBEDDED_PEAK_TUPLE != 0 {
                for (k, slot) in peak.iter_mut().enumerate().take(ac) {
                    *slot = self.tuple(gd, p, k);
                }
                p += 2 * ac;
            } else {
                let idx = (ti & TUPLE_INDEX_MASK) as usize;
                if idx >= self.shared_count as usize {
                    return None;
                }
                for (k, slot) in peak.iter_mut().enumerate().take(ac) {
                    *slot = self.tuple(self.shared, idx * ac * 2, k);
                }
            }
            let inter = if ti & INTERMEDIATE_REGION != 0 {
                for k in 0..ac {
                    ist[k] = self.tuple(gd, p, k);
                    ien[k] = self.tuple(gd, p + 2 * ac, k);
                }
                p += 4 * ac;
                true
            } else {
                false
            };
            hdr = p;
            let chunk_start = ser;
            let scalar = self.tuple_scalar(&peak[..ac], if inter { Some((&ist[..ac], &ien[..ac])) } else { None }, coords);
            // Advance the serialized-data cursor past this tuple whatever its scalar.
            let mut chunk = chunk_start;
            if chunk.skip(size).is_none() {
                return None;
            }
            ser = chunk;
            if scalar == 0.0 {
                continue;
            }
            let mut r = chunk_start;
            let pts = if ti & PRIVATE_POINT_NUMBERS != 0 { read_points(&mut r)? } else { shared_pts.clone() };
            let npts = pts.as_ref().map_or(n, |v| v.len());
            let xs = read_deltas(&mut r, npts)?;
            let ys = read_deltas(&mut r, npts)?;
            any = true;
            match pts {
                None => {
                    for (i, o) in out.iter_mut().enumerate() {
                        o.0 += xs[i] as f32 * scalar;
                        o.1 += ys[i] as f32 * scalar;
                    }
                }
                Some(list) => {
                    let mut dx = vec![0f32; n];
                    let mut dy = vec![0f32; n];
                    let mut touched = vec![false; n];
                    for (k, &pi) in list.iter().enumerate() {
                        let pi = pi as usize;
                        if pi < n {
                            dx[pi] += xs[k] as f32;
                            dy[pi] += ys[k] as f32;
                            touched[pi] = true;
                        }
                    }
                    if let Some((orig, ends)) = outline {
                        let ox: Vec<f32> = orig.iter().map(|p| p.0).collect();
                        let oy: Vec<f32> = orig.iter().map(|p| p.1).collect();
                        let mut start = 0usize;
                        for &e in ends {
                            if e >= orig.len() || e < start {
                                break;
                            }
                            iup_axis(&ox, &mut dx, &touched, start, e);
                            iup_axis(&oy, &mut dy, &touched, start, e);
                            start = e + 1;
                        }
                    }
                    for i in 0..n {
                        out[i].0 += dx[i] * scalar;
                        out[i].1 += dy[i] * scalar;
                    }
                }
            }
        }
        if any { Some(out) } else { None }
    }
}
