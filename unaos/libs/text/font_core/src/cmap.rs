//! `cmap` — character to glyph index mapping (OpenType spec, "cmap — Character to Glyph Index Mapping Table").
//! Formats 0, 4, 6, 12 and 13 are read; the subtable choice prefers a full-repertoire Unicode table
//! (3,10 / 0,4 / 0,6 format 12), then BMP Unicode (3,1 / 0,0..0,3 format 4), then Symbol (3,0), then Mac Roman (1,0).

use crate::reader::{u16_at, u32_at, slice, tail};

/// One cmap subtable, borrowed from the font.
#[derive(Clone, Copy, Debug)]
pub struct CmapSubtable<'a> {
    pub platform_id: u16,
    pub encoding_id: u16,
    pub format: u16,
    data: &'a [u8],
}

impl<'a> CmapSubtable<'a> {
    /// Map a code point through this subtable. `None` = not mapped (glyph 0 in the caller's terms).
    pub fn glyph(&self, cp: u32) -> Option<u16> {
        let d = self.data;
        let g = match self.format {
            0 => {
                if cp > 255 {
                    return None;
                }
                *d.get(6 + cp as usize)? as u16
            }
            4 => fmt4(d, cp)?,
            6 => {
                let first = u16_at(d, 6)? as u32;
                let count = u16_at(d, 8)? as u32;
                if cp < first || cp >= first + count {
                    return None;
                }
                u16_at(d, 10 + 2 * (cp - first) as usize)?
            }
            12 | 13 => {
                let n = u32_at(d, 12)? as usize;
                // Binary search the sequential map groups (sorted by startCharCode).
                let (mut lo, mut hi) = (0usize, n);
                let mut found = None;
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let base = 16usize.checked_add(mid.checked_mul(12)?)?;
                    let s = u32_at(d, base)?;
                    let e = u32_at(d, base + 4)?;
                    if cp < s {
                        hi = mid;
                    } else if cp > e {
                        lo = mid + 1;
                    } else {
                        let g0 = u32_at(d, base + 8)?;
                        let g = if self.format == 12 { g0.checked_add(cp - s)? } else { g0 };
                        found = Some(g);
                        break;
                    }
                }
                let g = found?;
                if g > 0xFFFF {
                    return None;
                }
                g as u16
            }
            _ => return None,
        };
        if g == 0 { None } else { Some(g) }
    }

    /// Every (code point, glyph) pair this subtable maps, in code point order (format 4/6/12/0 only).
    /// Used by the KATs to compare whole-table contents with the fontTools oracle.
    pub fn for_each(&self, mut f: impl FnMut(u32, u16)) {
        let d = self.data;
        match self.format {
            0 => {
                for cp in 0..256u32 {
                    if let Some(g) = self.glyph(cp) {
                        f(cp, g)
                    }
                }
            }
            4 => {
                let seg2 = match u16_at(d, 6) { Some(v) => v as usize, None => return };
                for s in 0..seg2 / 2 {
                    let (Some(end), Some(start)) = (u16_at(d, 14 + 2 * s), u16_at(d, 16 + seg2 + 2 * s)) else { return };
                    if start > end {
                        continue;
                    }
                    for cp in start as u32..=end as u32 {
                        if cp == 0xFFFF {
                            break;
                        }
                        if let Some(g) = self.glyph(cp) {
                            f(cp, g)
                        }
                    }
                }
            }
            6 => {
                let (Some(first), Some(count)) = (u16_at(d, 6), u16_at(d, 8)) else { return };
                for cp in first as u32..first as u32 + count as u32 {
                    if let Some(g) = self.glyph(cp) {
                        f(cp, g)
                    }
                }
            }
            12 | 13 => {
                let Some(n) = u32_at(d, 12) else { return };
                for i in 0..n as usize {
                    let base = 16 + 12 * i;
                    let (Some(s), Some(e)) = (u32_at(d, base), u32_at(d, base + 4)) else { return };
                    if e < s || e - s > 0x10_FFFF {
                        continue;
                    }
                    for cp in s..=e {
                        if let Some(g) = self.glyph(cp) {
                            f(cp, g)
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn fmt4(d: &[u8], cp: u32) -> Option<u16> {
    if cp > 0xFFFF {
        return None;
    }
    let c = cp as u16;
    let seg2 = u16_at(d, 6)? as usize;
    let segs = seg2 / 2;
    if segs == 0 {
        return None;
    }
    let end_base = 14;
    let start_base = end_base + seg2 + 2;
    let delta_base = start_base + seg2;
    let ro_base = delta_base + seg2;
    // Binary search for the first segment whose endCode >= c.
    let (mut lo, mut hi) = (0usize, segs);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if u16_at(d, end_base + 2 * mid)? < c {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo >= segs {
        return None;
    }
    let s = lo;
    let start = u16_at(d, start_base + 2 * s)?;
    if c < start {
        return None;
    }
    let delta = u16_at(d, delta_base + 2 * s)?;
    let ro = u16_at(d, ro_base + 2 * s)?;
    if ro == 0 {
        return Some(c.wrapping_add(delta));
    }
    // idRangeOffset is relative to its own position in the idRangeOffset array.
    let addr = ro_base + 2 * s + ro as usize + 2 * (c - start) as usize;
    let g = u16_at(d, addr)?;
    if g == 0 { Some(0) } else { Some(g.wrapping_add(delta)) }
}

/// The parsed `cmap` table: its subtable directory.
#[derive(Clone, Copy, Debug)]
pub struct Cmap<'a> {
    data: &'a [u8],
    count: u16,
}

impl<'a> Cmap<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        let count = u16_at(data, 2)?;
        slice(data, 4, count as usize * 8)?;
        Some(Cmap { data, count })
    }
    pub fn len(&self) -> usize {
        self.count as usize
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn subtable(&self, i: usize) -> Option<CmapSubtable<'a>> {
        if i >= self.count as usize {
            return None;
        }
        let rec = 4 + 8 * i;
        let platform_id = u16_at(self.data, rec)?;
        let encoding_id = u16_at(self.data, rec + 2)?;
        let off = u32_at(self.data, rec + 4)? as usize;
        let st = tail(self.data, off)?;
        let format = u16_at(st, 0)?;
        // Bound the subtable by its declared length so a lookup never reads a neighbour.
        let len = match format {
            0 | 2 | 4 | 6 => u16_at(st, 2)? as usize,
            8 | 10 | 12 | 13 | 14 => u32_at(st, 4)? as usize,
            _ => st.len(),
        };
        let data = slice(st, 0, len.min(st.len()))?;
        Some(CmapSubtable { platform_id, encoding_id, format, data })
    }
    /// The subtable a Unicode lookup should use (see module doc for the preference order).
    pub fn best(&self) -> Option<CmapSubtable<'a>> {
        let mut best: Option<(u8, CmapSubtable<'a>)> = None;
        for i in 0..self.len() {
            let Some(st) = self.subtable(i) else { continue };
            let rank = match (st.platform_id, st.encoding_id, st.format) {
                (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 6,
                (0, _, 12) => 5,
                (3, 1, 4) => 4,
                (0, _, 4) => 4,
                (3, 1, 6) | (0, _, 6) => 3,
                (3, 0, 4) => 2,
                (1, 0, 0) | (1, 0, 6) => 1,
                _ => 0,
            };
            if rank == 0 {
                continue;
            }
            if best.is_none_or(|(r, _)| rank > r) {
                best = Some((rank, st));
            }
        }
        best.map(|(_, s)| s)
    }
}
