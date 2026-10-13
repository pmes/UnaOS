// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Inter prediction (8.4): motion vector prediction (8.4.1.3 with the neighbouring partitions of
//! 8.4.1.3.2), P_Skip (8.4.1.1), B direct prediction — spatial (8.4.1.2.2) and temporal
//! (8.4.1.2.3) with the co-located block of 8.4.1.2.1 — fractional sample interpolation
//! (8.4.2.2.1 luma 6-tap quarter-sample, 8.4.2.2.2 chroma eighth-sample bilinear) and the weighted
//! sample prediction of 8.4.2.3 (default, explicit 8.4.2.3.2 and implicit 8.4.2.3.1 weights).
//!
//! Motion is derived partition by partition in decoding order and written to the picture's motion
//! field at once, so a later partition of the same macroblock sees an earlier one as an
//! available neighbour and never sees a later one (6.4.11.7).

use crate::mb::{mb_partitions, num_parts, part_rect, sub_partitions, sub_pos, Mb, SliceCtx, SHAPE_16X8, SHAPE_8X16, SHAPE_8X8};
use crate::picture::{blk_at, MbKind, Mv, Plane, Picture};
use crate::ps::SliceType;
use crate::{Error, Result};

/// A neighbouring partition's motion for one list (8.4.1.3.2): availability, refIdxLXN (-1 when
/// the partition is intra, does not use the list, or is unavailable) and mvLXN.
#[derive(Clone, Copy)]
struct NbMv {
    avail: bool,
    r: i32,
    mv: Mv,
}

const NB_NONE: NbMv = NbMv { avail: false, r: -1, mv: Mv::ZERO };

/// Weights for one prediction: (logWD, w0, w1, o0, o1) per component.
#[derive(Clone, Copy)]
struct Wp {
    log_wd: i32,
    w: [i32; 2],
    o: [i32; 2],
}

/// The motion of one block: (refIdxL0, refIdxL1) (-1 = list unused) and (mvL0, mvL1).
#[derive(Clone, Copy)]
pub struct BlockMotion {
    pub r: [i32; 2],
    pub mv: [Mv; 2],
}

fn median(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

fn min_positive(x: i32, y: i32) -> i32 {
    if x >= 0 && y >= 0 { x.min(y) } else { x.max(y) }
}

impl<'a> SliceCtx<'a> {
    /// Motion of the neighbouring 4x4 location (x, y) (in 4x4 units, relative to the macroblock)
    /// for `list`; inside the current macroblock a block at or after luma4x4BlkIdx `limit` is not
    /// yet decoded.
    fn nb_mv(&self, addr: usize, list: usize, x: isize, y: isize, limit: usize) -> NbMv {
        let Some((m, nx, ny)) = self.nb(addr, x, y, 4) else { return NB_NONE };
        if m == addr && blk_at(nx, ny) >= limit {
            return NB_NONE;
        }
        let g = self.g4(m, nx, ny);
        let r = self.pic.motion.ref_idx[g][list] as i32;
        NbMv { avail: true, r, mv: if r >= 0 { self.pic.motion.mv[g][list] } else { Mv::ZERO } }
    }

    /// A, B and C (D when C is unavailable) of a partition (8.4.1.3.2).
    fn nb_abc(&self, addr: usize, list: usize, x: usize, y: usize, w: usize) -> [NbMv; 3] {
        let limit = blk_at(x, y);
        let (x, y, w) = (x as isize, y as isize, w as isize);
        let a = self.nb_mv(addr, list, x - 1, y, limit);
        let b = self.nb_mv(addr, list, x, y - 1, limit);
        let mut c = self.nb_mv(addr, list, x + w, y - 1, limit);
        if !c.avail {
            c = self.nb_mv(addr, list, x - 1, y - 1, limit);
        }
        [a, b, c]
    }

    /// mvpLX (8.4.1.3) of the partition at (x, y) of width w (4x4 units) with refIdxLX `r`;
    /// `dir` = Some((shape, mbPartIdx)) enables the 16x8 / 8x16 directional rules.
    fn mvp(&self, addr: usize, list: usize, x: usize, y: usize, w: usize, r: i32, dir: Option<(u8, usize)>) -> Mv {
        let [a, b, c] = self.nb_abc(addr, list, x, y, w);
        match dir {
            Some((SHAPE_16X8, 0)) if b.r == r => return b.mv,
            Some((SHAPE_16X8, 1)) if a.r == r => return a.mv,
            Some((SHAPE_8X16, 0)) if a.r == r => return a.mv,
            Some((SHAPE_8X16, 1)) if c.r == r => return c.mv,
            _ => {}
        }
        // 8.4.1.3.1
        let (b, c) = if !b.avail && !c.avail && a.avail { (a, a) } else { (b, c) };
        let hits = (a.r == r) as u32 + (b.r == r) as u32 + (c.r == r) as u32;
        if hits == 1 {
            if a.r == r {
                return a.mv;
            }
            if b.r == r {
                return b.mv;
            }
            return c.mv;
        }
        Mv::new(median(a.mv.x as i32, b.mv.x as i32, c.mv.x as i32), median(a.mv.y as i32, b.mv.y as i32, c.mv.y as i32))
    }

    /// The DPB id of RefPicListX[r].
    fn ref_pic(&self, list: usize, r: i32) -> Result<&'a Picture> {
        let idx = *self.lists[list].get(r as usize).ok_or(Error::Invalid("refIdx beyond the reference list"))?;
        self.dpb.get(idx).ok_or(Error::Invalid("reference list entry is \"no reference picture\""))
    }

    /// Store the motion of a rectangle (x, y, w, h in 4x4 units) of macroblock `addr`.
    fn put_motion(&mut self, addr: usize, (x, y, w, h): (usize, usize, usize, usize), m: BlockMotion) -> Result<()> {
        let mut ids = [-1i32; 2];
        for l in 0..2 {
            if m.r[l] >= 0 {
                ids[l] = self.ref_pic(l, m.r[l])?.id;
            }
        }
        for yy in y..y + h {
            for xx in x..x + w {
                let g = self.g4(addr, xx, yy);
                let mo = &mut self.pic.motion;
                mo.ref_idx[g] = [m.r[0] as i8, m.r[1] as i8];
                mo.ref_id[g] = ids;
                mo.mv[g] = [if m.r[0] >= 0 { m.mv[0] } else { Mv::ZERO }, if m.r[1] >= 0 { m.mv[1] } else { Mv::ZERO }];
            }
        }
        Ok(())
    }

    /// P_Skip motion (8.4.1.1).
    fn p_skip_motion(&self, addr: usize) -> BlockMotion {
        let a = self.nb_mv(addr, 0, -1, 0, 0);
        let b = self.nb_mv(addr, 0, 0, -1, 0);
        let zero = !a.avail || !b.avail || (a.r == 0 && a.mv == Mv::ZERO) || (b.r == 0 && b.mv == Mv::ZERO);
        let mv = if zero { Mv::ZERO } else { self.mvp(addr, 0, 0, 0, 4, 0, None) };
        BlockMotion { r: [0, -1], mv: [mv, Mv::ZERO] }
    }

    /// The co-located 4x4 block (8.4.1.2.1) for the 4x4 block (x, y) of the macroblock:
    /// (mvCol, refIdxCol, DPB id of the picture refIdxCol refers to).
    fn colocated(&self, addr: usize, x: usize, y: usize) -> Result<(Mv, i32, i32)> {
        let col = self.ref_pic(1, 0)?;
        let (cx, cy) = if self.sps.direct_8x8_inference { (if x < 2 { 0 } else { 3 }, if y < 2 { 0 } else { 3 }) } else { (x, y) };
        let g = self.g4(addr, cx, cy);
        if g >= col.motion.ref_idx.len() {
            return Err(Error::Invalid("co-located picture has another size"));
        }
        let r = col.motion.ref_idx[g];
        if r[0] >= 0 {
            Ok((col.motion.mv[g][0], r[0] as i32, col.motion.ref_id[g][0]))
        } else if r[1] >= 0 {
            Ok((col.motion.mv[g][1], r[1] as i32, col.motion.ref_id[g][1]))
        } else {
            Ok((Mv::ZERO, -1, -1))
        }
    }

    /// Spatial direct (8.4.1.2.2): the macroblock-wide reference indices and motion vector
    /// predictors, and directZeroPredictionFlag.
    fn spatial_direct_base(&self, addr: usize) -> ([i32; 2], [Mv; 2], bool) {
        let mut r = [-1i32; 2];
        for l in 0..2 {
            let [a, b, c] = self.nb_abc(addr, l, 0, 0, 4);
            r[l] = min_positive(a.r, min_positive(b.r, c.r));
        }
        let mut zero = false;
        if r[0] < 0 && r[1] < 0 {
            r = [0, 0];
            zero = true;
        }
        let mut mvp = [Mv::ZERO; 2];
        for l in 0..2 {
            if r[l] >= 0 && !zero {
                mvp[l] = self.mvp(addr, l, 0, 0, 4, r[l], None);
            }
        }
        (r, mvp, zero)
    }

    /// Direct-mode motion of the 4x4 block (x, y) (8.4.1.2).
    fn direct_motion(&self, addr: usize, x: usize, y: usize, base: &Option<([i32; 2], [Mv; 2], bool)>) -> Result<BlockMotion> {
        let (mv_col, r_col, id_col) = self.colocated(addr, x, y)?;
        if self.sh.direct_spatial {
            let (r, mvp, zero) = base.ok_or(Error::Invalid("spatial direct without its base"))?;
            let l1 = self.ref_pic(1, 0)?;
            let col_zero = !l1.is_long() && r_col == 0 && (-1..=1).contains(&mv_col.x) && (-1..=1).contains(&mv_col.y);
            let mut mv = [Mv::ZERO; 2];
            for l in 0..2 {
                if !(zero || r[l] < 0 || (r[l] == 0 && col_zero)) {
                    mv[l] = mvp[l];
                }
            }
            Ok(BlockMotion { r, mv })
        } else {
            // Temporal (8.4.1.2.3): refIdxL0 = the lowest index in RefPicList0 of the picture the
            // co-located block referenced; refIdxL1 = 0.
            let r0 = if r_col < 0 {
                0
            } else {
                let mut found = None;
                for (i, &idx) in self.lists[0].iter().enumerate() {
                    if self.dpb.get(idx).is_some_and(|p| p.id == id_col) {
                        found = Some(i as i32);
                        break;
                    }
                }
                found.ok_or(Error::Invalid("temporal direct: co-located reference not in RefPicList0"))?
            };
            let p0 = self.ref_pic(0, r0)?;
            let p1 = self.ref_pic(1, 0)?;
            let td = (p1.poc - p0.poc).clamp(-128, 127);
            let (mv0, mv1) = if p0.is_long() || td == 0 {
                (mv_col, Mv::ZERO)
            } else {
                let tb = (self.pic.poc - p0.poc).clamp(-128, 127);
                let tx = (16384 + (td / 2).abs()) / td;
                let dsf = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
                let m0 = Mv::new((dsf * mv_col.x as i32 + 128) >> 8, (dsf * mv_col.y as i32 + 128) >> 8);
                (m0, Mv::new(m0.x as i32 - mv_col.x as i32, m0.y as i32 - mv_col.y as i32))
            };
            Ok(BlockMotion { r: [r0, 0], mv: [mv0, mv1] })
        }
    }

    /// Derive the motion of inter macroblock `addr` and form its prediction samples in the
    /// current picture (8.4).
    pub fn inter_mb(&mut self, addr: usize, mb: &Mb) -> Result<()> {
        let st = self.slice_type();
        match mb.kind {
            MbKind::PSkip => {
                let m = self.p_skip_motion(addr);
                self.put_motion(addr, (0, 0, 4, 4), m)?;
                self.predict(addr, (0, 0, 4, 4), m)?;
            }
            MbKind::BSkip | MbKind::BDirect => {
                let base = if self.sh.direct_spatial { Some(self.spatial_direct_base(addr)) } else { None };
                for i in 0..4 {
                    self.direct_8x8(addr, i, &base)?;
                }
            }
            _ => {
                let (shape, pf) = mb_partitions(st, mb.mb_type);
                if shape == SHAPE_8X8 {
                    let mut base = None;
                    for i in 0..4 {
                        let (bx, by, _, _) = part_rect(SHAPE_8X8, i);
                        if st == SliceType::B && mb.sub[i] == 0 {
                            if base.is_none() && self.sh.direct_spatial {
                                base = Some(self.spatial_direct_base(addr));
                            }
                            self.direct_8x8(addr, i, &base)?;
                            continue;
                        }
                        let (np, w, h, spf) = sub_partitions(st, mb.sub[i]);
                        for j in 0..np {
                            let (sx, sy) = sub_pos(w, h, j);
                            let (x, y) = (bx + sx, by + sy);
                            let mut m = BlockMotion { r: [-1; 2], mv: [Mv::ZERO; 2] };
                            for l in 0..2 {
                                if spf >> l & 1 == 1 {
                                    let r = mb.ref_idx[l][i] as i32;
                                    let p = self.mvp(addr, l, x, y, w, r, None);
                                    let d = mb.mvd[l][i][j];
                                    m.r[l] = r;
                                    m.mv[l] = Mv::new(p.x as i32 + d[0], p.y as i32 + d[1]);
                                }
                            }
                            self.put_motion(addr, (x, y, w, h), m)?;
                            self.predict(addr, (x, y, w, h), m)?;
                        }
                    }
                } else {
                    for p in 0..num_parts(shape) {
                        let (x, y, w, h) = part_rect(shape, p);
                        let mut m = BlockMotion { r: [-1; 2], mv: [Mv::ZERO; 2] };
                        for l in 0..2 {
                            if pf[p] >> l & 1 == 1 {
                                let r = mb.ref_idx[l][p] as i32;
                                let pr = self.mvp(addr, l, x, y, w, r, Some((shape, p)));
                                let d = mb.mvd[l][p][0];
                                m.r[l] = r;
                                m.mv[l] = Mv::new(pr.x as i32 + d[0], pr.y as i32 + d[1]);
                            }
                        }
                        self.put_motion(addr, (x, y, w, h), m)?;
                        self.predict(addr, (x, y, w, h), m)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Direct prediction of sub-macroblock `i` (an 8x8 quadrant): per 4x4 block, or per 8x8 with
    /// direct_8x8_inference_flag (the co-located corner block decides the whole quadrant).
    fn direct_8x8(&mut self, addr: usize, i: usize, base: &Option<([i32; 2], [Mv; 2], bool)>) -> Result<()> {
        let (bx, by, _, _) = part_rect(SHAPE_8X8, i);
        if self.sps.direct_8x8_inference {
            let m = self.direct_motion(addr, bx, by, base)?;
            self.put_motion(addr, (bx, by, 2, 2), m)?;
            self.predict(addr, (bx, by, 2, 2), m)?;
        } else {
            for j in 0..4 {
                let (x, y) = (bx + j % 2, by + j / 2);
                let m = self.direct_motion(addr, x, y, base)?;
                self.put_motion(addr, (x, y, 1, 1), m)?;
                self.predict(addr, (x, y, 1, 1), m)?;
            }
        }
        Ok(())
    }

    /// The weights of one block (8.4.2.3): None = default prediction.
    fn weights(&self, m: &BlockMotion) -> Result<Option<[Wp; 3]>> {
        let bi = m.r[0] >= 0 && m.r[1] >= 0;
        if self.sh.slice_type == SliceType::B && self.pps.weighted_bipred_idc == 2 {
            if !bi {
                return Ok(None);
            }
            // Implicit (8.4.2.3.1).
            let p0 = self.ref_pic(0, m.r[0])?;
            let p1 = self.ref_pic(1, m.r[1])?;
            let td = (p1.poc - p0.poc).clamp(-128, 127);
            let (mut w0, mut w1) = (32, 32);
            if td != 0 && !p0.is_long() && !p1.is_long() {
                let tb = (self.pic.poc - p0.poc).clamp(-128, 127);
                let tx = (16384 + (td / 2).abs()) / td;
                let dsf = ((tb * tx + 32) >> 6).clamp(-1024, 1023);
                if (-64..=128).contains(&(dsf >> 2)) {
                    w0 = 64 - (dsf >> 2);
                    w1 = dsf >> 2;
                }
            }
            let wp = Wp { log_wd: 5, w: [w0, w1], o: [0, 0] };
            return Ok(Some([wp; 3]));
        }
        let Some(pw) = self.sh.weights.as_ref() else { return Ok(None) };
        let mut out = [Wp { log_wd: 0, w: [0; 2], o: [0; 2] }; 3];
        for c in 0..3 {
            out[c].log_wd = if c == 0 { pw.luma_log2 } else { pw.chroma_log2 } as i32;
            for l in 0..2 {
                if m.r[l] >= 0 {
                    let r = (m.r[l] as usize).min(31);
                    let (w, o) = if c == 0 { pw.luma[l][r] } else { pw.chroma[l][r][c - 1] };
                    out[c].w[l] = w;
                    out[c].o[l] = o;
                }
            }
        }
        Ok(Some(out))
    }

    /// Form the prediction samples of a rectangle (4x4 units) of macroblock `addr` from its
    /// motion, writing them into the current picture.
    fn predict(&mut self, addr: usize, (x, y, w, h): (usize, usize, usize, usize), m: BlockMotion) -> Result<()> {
        let (px, py) = ((addr % self.mb_w) * 16 + x * 4, (addr / self.mb_w) * 16 + y * 4);
        let (bw, bh) = (w * 4, h * 4);
        let wp = self.weights(&m)?;
        let mut pred = [[[0u8; 256]; 2]; 3];
        let mut used = [false; 2];
        for l in 0..2 {
            if m.r[l] < 0 {
                continue;
            }
            used[l] = true;
            let rp = self.ref_pic(l, m.r[l])?;
            let mv = m.mv[l];
            luma_mc(&rp.planes[0], px as i32, py as i32, mv, bw, bh, &mut pred[0][l]);
            for c in 1..3 {
                chroma_mc(&rp.planes[c], (px / 2) as i32, (py / 2) as i32, mv, bw / 2, bh / 2, &mut pred[c][l]);
            }
        }
        for c in 0..3 {
            let (cx, cy, cw, ch) = if c == 0 { (px, py, bw, bh) } else { (px / 2, py / 2, bw / 2, bh / 2) };
            let plane = &mut self.pic.planes[c];
            for yy in 0..ch {
                for xx in 0..cw {
                    let k = yy * 16 + xx;
                    let (a, b) = (pred[c][0][k] as i32, pred[c][1][k] as i32);
                    let v = match (&wp, used) {
                        (None, [true, true]) => (a + b + 1) >> 1,
                        (None, [true, false]) => a,
                        (None, _) => b,
                        (Some(wp), u) => {
                            let wp = &wp[c];
                            let lw = wp.log_wd;
                            if u[0] && u[1] {
                                ((a * wp.w[0] + b * wp.w[1] + (1 << lw)) >> (lw + 1)) + ((wp.o[0] + wp.o[1] + 1) >> 1)
                            } else {
                                let (s, l) = if u[0] { (a, 0) } else { (b, 1) };
                                if lw >= 1 { ((s * wp.w[l] + (1 << (lw - 1))) >> lw) + wp.o[l] } else { s * wp.w[l] + wp.o[l] }
                            }
                        }
                    };
                    plane.data[(cy + yy) * plane.w + cx + xx] = v.clamp(0, 255) as u8;
                }
            }
        }
        Ok(())
    }
}

#[inline]
fn tap6(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> i32 {
    a - 5 * b + 20 * c + 20 * d - 5 * e + f
}

#[inline]
fn clip1(v: i32) -> i32 {
    v.clamp(0, 255)
}

/// Luma sample interpolation (8.4.2.2.1) of a bw x bh block whose top-left full-sample position
/// is (x0, y0), displaced by the quarter-sample `mv`, into `out` (row stride 16).
pub fn luma_mc(p: &Plane, x0: i32, y0: i32, mv: Mv, bw: usize, bh: usize, out: &mut [u8; 256]) {
    let (xi, yi) = (x0 + (mv.x as i32 >> 2), y0 + (mv.y as i32 >> 2));
    let (xf, yf) = (mv.x as i32 & 3, mv.y as i32 & 3);
    // Window of reference samples with 2 before and 3 after, edge-clamped.
    let mut win = [0i32; 21 * 21];
    let ww = bw + 5;
    for r in 0..bh + 5 {
        for c in 0..ww {
            win[r * 21 + c] = p.at(xi + c as i32 - 2, yi + r as i32 - 2) as i32;
        }
    }
    let s = |x: i32, y: i32| win[((y + 2) * 21 + x + 2) as usize];
    let b1 = |x: i32, y: i32| tap6(s(x - 2, y), s(x - 1, y), s(x, y), s(x + 1, y), s(x + 2, y), s(x + 3, y));
    let h1 = |x: i32, y: i32| tap6(s(x, y - 2), s(x, y - 1), s(x, y), s(x, y + 1), s(x, y + 2), s(x, y + 3));
    let bv = |x: i32, y: i32| clip1((b1(x, y) + 16) >> 5);
    let hv = |x: i32, y: i32| clip1((h1(x, y) + 16) >> 5);
    let jv = |x: i32, y: i32| clip1((tap6(b1(x, y - 2), b1(x, y - 1), b1(x, y), b1(x, y + 1), b1(x, y + 2), b1(x, y + 3)) + 512) >> 10);
    for y in 0..bh as i32 {
        for x in 0..bw as i32 {
            let v = match (xf, yf) {
                (0, 0) => s(x, y),
                (0, 1) => (s(x, y) + hv(x, y) + 1) >> 1,
                (0, 2) => hv(x, y),
                (0, 3) => (s(x, y + 1) + hv(x, y) + 1) >> 1,
                (1, 0) => (s(x, y) + bv(x, y) + 1) >> 1,
                (1, 1) => (bv(x, y) + hv(x, y) + 1) >> 1,
                (1, 2) => (hv(x, y) + jv(x, y) + 1) >> 1,
                (1, 3) => (hv(x, y) + bv(x, y + 1) + 1) >> 1,
                (2, 0) => bv(x, y),
                (2, 1) => (bv(x, y) + jv(x, y) + 1) >> 1,
                (2, 2) => jv(x, y),
                (2, 3) => (jv(x, y) + bv(x, y + 1) + 1) >> 1,
                (3, 0) => (s(x + 1, y) + bv(x, y) + 1) >> 1,
                (3, 1) => (bv(x, y) + hv(x + 1, y) + 1) >> 1,
                (3, 2) => (jv(x, y) + hv(x + 1, y) + 1) >> 1,
                _ => (hv(x + 1, y) + bv(x, y + 1) + 1) >> 1,
            };
            out[(y * 16 + x) as usize] = v as u8;
        }
    }
}

/// Chroma sample interpolation (8.4.2.2.2, ChromaArrayType 1: the luma vector in eighth chroma
/// samples) of a bw x bh chroma block at (x0, y0) into `out` (row stride 16).
pub fn chroma_mc(p: &Plane, x0: i32, y0: i32, mv: Mv, bw: usize, bh: usize, out: &mut [u8; 256]) {
    let (xi, yi) = (x0 + (mv.x as i32 >> 3), y0 + (mv.y as i32 >> 3));
    let (xf, yf) = (mv.x as i32 & 7, mv.y as i32 & 7);
    for y in 0..bh as i32 {
        for x in 0..bw as i32 {
            let a = p.at(xi + x, yi + y) as i32;
            let b = p.at(xi + x + 1, yi + y) as i32;
            let c = p.at(xi + x, yi + y + 1) as i32;
            let d = p.at(xi + x + 1, yi + y + 1) as i32;
            let v = ((8 - xf) * (8 - yf) * a + xf * (8 - yf) * b + (8 - xf) * yf * c + xf * yf * d + 32) >> 6;
            out[(y * 16 + x) as usize] = v as u8;
        }
    }
}
