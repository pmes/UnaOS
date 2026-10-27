// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCRC (LEDGER SR218): rate control — a per-picture QP from a bit budget, then a per-row QP
//! written as mb_qp_delta.
//!
//! CHARTER: Stria (A/V) — seam `shared-core` (h264enc_core's one rate model; it only chooses the
//! value of `Encoder::qp` before each macroblock, the quantiser and the writers are the encoder's).
//!
//! The model is integer only (`no_std`, no float):
//! - the budget: R = bits_per_second * fps_den / fps_num bits per picture; a buffer of one second
//!   (B = bits_per_second). [`RcMode::Cbr`] keeps a leaky bucket (fullness f starts at B/2,
//!   f = max(0, f + bits - R), never above B); [`RcMode::Vbr`] keeps a bucket drained at the max
//!   rate (size max_bits_per_second) and a signed long-term error sum(bits) - R * n.
//! - the complexity of a picture by pre-analysis, one SAD per macroblock row ([`Encoder::rc_analyse`]):
//!   an I picture's luma gradient (left and upper neighbour), a P picture's difference from its
//!   reference at zero motion; S' = S + 64 per macroblock.
//! - the R-Q model per picture type: k = bits * Qstep(qp) / S' of the last picture of that type;
//!   the bits of the next one at q are predicted as k * S' / Qstep(q) ([`step16`] = H.264's Qstep x 16).
//! - the target: the GOP's budget shared by the I / P ratio of the models (an I picture gets
//!   R * gop * r / (r + gop - 1)); CBR corrects it by the bucket's distance from the planned
//!   fullness (1/8 of it) and caps it at the bucket's room; VBR scales it by the picture's complexity
//!   against its type's running mean (x1/2 ..x4: a complex picture spends more), corrects 1/32 of
//!   the long-term error and caps it at the max bucket's room.
//! - the picture QP: the smallest q in [ref - 6, ref + 6] (ref the last QP of the type) and in
//!   [16, 51] whose predicted bits meet the target, written as slice_qp_delta; within the picture
//!   each macroblock row's QP moves by up to +-3 from it by the running bits against the row's share
//!   of the target (the rows' SAD), written as mb_qp_delta (lib.rs `write_mb`, QPY,pred tracked as
//!   h264_core's `mb.rs` does).
//!
//! Owed (handlers.md §5 H264ENC-RC): two-pass, scene-cut IDRs, per-MB activity masking, HRD signalling.

use alloc::vec::Vec;

use h264_core::tables::QPC;

use crate::{Encoder, Error, Result};

/// The lowest QP the rate controller picks (the encoder's configured floor).
pub const QP_MIN: i32 = 16;
/// The highest QP the rate controller picks (the Recommendation's QPY limit).
pub const QP_MAX: i32 = 51;
/// The largest change of a picture's QP from the last picture of its type.
pub const PIC_STEP: i32 = 6;
/// The largest offset of a macroblock row's QP from its picture's QP.
pub const ROW_STEP: i32 = 3;

/// How the bit budget is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcMode {
    /// Constant bit rate: a one-second leaky bucket at `bits_per_second` that never overflows.
    Cbr,
    /// Variable bit rate: the mean holds at `bits_per_second`, a complex picture may spend more, up
    /// to a one-second bucket drained at `max_bits_per_second`.
    Vbr { max_bits_per_second: u32 },
}

/// The rate control of a stream ([`crate::Config::rate`]); `fps_num / fps_den` pictures per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateControl {
    pub bits_per_second: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub mode: RcMode,
}

/// H.264's quantiser step Qstep(qp) x 16 (Qstep doubles every 6 QP: 0.625, 0.6875, 0.8125, 0.875,
/// 1, 1.125 at qp 0..5).
pub fn step16(qp: i32) -> u64 {
    [10u64, 11, 13, 14, 16, 18][(qp % 6) as usize] << (qp / 6)
}

/// The last coded picture of one type (I or P): what the R-Q model is fitted to.
#[derive(Debug, Clone, Copy, Default)]
struct Model {
    coded: bool,
    bits: u64,
    step: u64,
    s: u64,
    qp: i32,
    /// Running mean of S' (1/8 per picture).
    avg_s: u64,
}

impl Model {
    /// Predicted bits of a picture of complexity `s` at `qp`.
    fn predict(&self, s: u64, qp: i32) -> u128 {
        self.bits as u128 * self.step as u128 * s as u128 / (self.s.max(1) as u128 * step16(qp) as u128)
    }
}

/// The rate controller's state, owned by its [`Encoder`].
#[derive(Debug, Clone)]
pub(crate) struct Rc {
    rate: RateControl,
    gop: i128,
    /// Bits per picture at the mean rate, and the one-second buffer.
    r: i64,
    buf: i64,
    /// CBR bucket fullness and its planned value.
    full: i64,
    plan: i64,
    /// VBR: bits per picture at the max rate, its bucket's size and fullness.
    rmax: i64,
    bmax: i64,
    fmax: i64,
    /// sum(bits) - R * pictures.
    err: i64,
    /// [I, P].
    types: [Model; 2],
    last_qp: i32,
    // The picture being coded.
    idr: bool,
    rows: Vec<u64>,
    s: u64,
    share: i64,
    target: i64,
    pic_qp: i32,
    /// The lowest and highest row QP of the picture.
    pub(crate) span: (i32, i32),
}

impl Rc {
    pub(crate) fn new(rate: RateControl, gop: u32, start_qp: i32) -> Result<Rc> {
        let max = match rate.mode {
            RcMode::Cbr => rate.bits_per_second,
            RcMode::Vbr { max_bits_per_second } => max_bits_per_second,
        };
        if rate.bits_per_second == 0 || rate.fps_num == 0 || rate.fps_den == 0 || max < rate.bits_per_second {
            return Err(Error::Rate);
        }
        let per = |bps: u32| ((bps as u64 * rate.fps_den as u64 / rate.fps_num as u64).max(1)) as i64;
        let buf = rate.bits_per_second as i64;
        Ok(Rc {
            rate,
            gop: gop as i128,
            r: per(rate.bits_per_second),
            buf,
            full: buf / 2,
            plan: buf / 2,
            rmax: per(max),
            bmax: max as i64,
            fmax: 0,
            err: 0,
            types: [Model::default(); 2],
            last_qp: start_qp,
            idr: true,
            rows: Vec::new(),
            s: 0,
            share: 0,
            target: 0,
            pic_qp: start_qp,
            span: (start_qp, start_qp),
        })
    }

    /// Choose the QP of the next picture from its rows' complexity.
    fn picture(&mut self, idr: bool, rows: Vec<u64>) -> i32 {
        let t = if idr { 0 } else { 1 };
        let s: u64 = rows.iter().sum();
        if idr {
            self.plan = self.buf / 2;
        }
        // The I / P ratio at equal QP, x16 (k * S' = bits * step of the last picture of each type).
        let [mi, mp] = self.types;
        let r16: i128 = if mi.coded && mp.coded {
            ((16 * mi.bits as u128 * mi.step as u128) / (mp.bits as u128 * mp.step as u128).max(1)).clamp(16, 256) as i128
        } else {
            64
        };
        let den = r16 + 16 * (self.gop - 1);
        let share = (self.r as i128 * self.gop * if idr { r16 } else { 16 } / den) as i64;
        let mut target = share;
        match self.rate.mode {
            RcMode::Cbr => {
                target += (self.plan - self.full) / 8;
                target = target.min(self.buf - self.full).max(self.r / 8);
            }
            RcMode::Vbr { .. } => {
                let avg = self.types[t].avg_s;
                if avg > 0 {
                    target = (target as i128 * s as i128 / avg as i128).clamp(target as i128 / 2, target as i128 * 4) as i64;
                }
                target -= self.err / 32;
                target = target.min(self.bmax - self.fmax).max(self.r / 8);
            }
        }
        let m = self.types[t];
        let refq = if m.coded { m.qp } else { self.last_qp };
        let (lo, hi) = ((refq - PIC_STEP).max(QP_MIN), (refq + PIC_STEP).min(QP_MAX));
        let qp = if m.coded { (lo..=hi).find(|&q| m.predict(s, q) <= target as u128).unwrap_or(hi) } else { refq };
        self.idr = idr;
        self.rows = rows;
        self.s = s;
        self.share = share;
        self.target = target;
        self.pic_qp = qp;
        self.span = (qp, qp);
        qp
    }

    /// The QP of macroblock row `row` (> 0) after `bits` bits of slice data.
    fn row_qp(&mut self, row: usize, bits: u64) -> i32 {
        let cum: u64 = self.rows[..row].iter().sum();
        let expect = self.target as i128 * cum as i128 / self.s.max(1) as i128;
        let dev = bits as i128 - expect;
        let off = (dev * 8 / self.target.max(1) as i128).clamp(-ROW_STEP as i128, ROW_STEP as i128) as i32;
        let qp = (self.pic_qp + off).clamp(QP_MIN, QP_MAX);
        self.span = (self.span.0.min(qp), self.span.1.max(qp));
        qp
    }

    /// Fit the model and move the buckets by the coded picture's `bits`.
    fn update(&mut self, bits: u64) {
        let m = &mut self.types[if self.idr { 0 } else { 1 }];
        m.coded = true;
        m.bits = bits.max(1);
        m.step = step16(self.pic_qp);
        m.s = self.s.max(1);
        m.qp = self.pic_qp;
        m.avg_s = if m.avg_s == 0 { self.s } else { m.avg_s - m.avg_s / 8 + self.s / 8 };
        self.last_qp = self.pic_qp;
        let b = bits as i64;
        self.full = (self.full + b - self.r).max(0);
        self.plan += self.share - self.r;
        self.fmax = (self.fmax + b - self.rmax).max(0);
        self.err += b - self.r;
    }
}

impl Encoder {
    /// The QP of the next macroblocks: QPY and its QPc (chroma_qp_index_offset 0).
    pub(crate) fn set_qp(&mut self, qp: i32) {
        self.qp = qp;
        self.qpc = QPC[qp as usize] as i32;
    }

    /// The picture's complexity per macroblock row: the luma gradient of an I picture, the zero-motion
    /// difference from the reference of a P picture, plus 64 per macroblock.
    pub(crate) fn rc_analyse(&self, idr: bool) -> Vec<u64> {
        let (src, refp) = (&self.src[0], &self.refp[0]);
        let w = src.w;
        let mut rows = Vec::with_capacity(self.mb_h);
        for mby in 0..self.mb_h {
            let mut s = 64 * self.mb_w as u64;
            for y in mby * 16..mby * 16 + 16 {
                for x in 0..w {
                    let p = src.data[y * w + x] as i32;
                    if idr {
                        if x > 0 {
                            s += (p - src.data[y * w + x - 1] as i32).unsigned_abs() as u64;
                        }
                        if y > 0 {
                            s += (p - src.data[(y - 1) * w + x] as i32).unsigned_abs() as u64;
                        }
                    } else {
                        s += (p - refp.data[y * w + x] as i32).unsigned_abs() as u64;
                    }
                }
            }
            rows.push(s);
        }
        rows
    }

    /// The QP of the next picture (SliceQPY): the rate controller's, else the constant QP.
    pub(crate) fn rc_picture(&mut self, idr: bool) -> i32 {
        if self.rc.is_none() {
            return self.cfg.qp as i32;
        }
        let rows = self.rc_analyse(idr);
        let qp = self.rc.as_mut().map_or(self.cfg.qp as i32, |rc| rc.picture(idr, rows));
        self.set_qp(qp);
        qp
    }

    /// Before macroblock row `row`: its QP from the `bits` of slice data written so far.
    pub(crate) fn rc_row(&mut self, row: usize, bits: usize) {
        if row == 0 {
            return;
        }
        if let Some(qp) = self.rc.as_mut().map(|rc| rc.row_qp(row, bits as u64)) {
            self.set_qp(qp);
        }
    }

    /// After the picture: its `bits` (every NAL unit of the access unit) into the model.
    pub(crate) fn rc_update(&mut self, bits: u64) {
        if let Some(rc) = self.rc.as_mut() {
            rc.update(bits);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Yuv420};
    use alloc::vec;

    #[test]
    fn step16_doubles_every_six() {
        assert_eq!(step16(4), 16); // Qstep(4) = 1
        for q in 0..46 {
            assert_eq!(step16(q + 6), 2 * step16(q));
            assert!(step16(q + 1) > step16(q));
        }
    }

    #[test]
    fn predicted_bits_fall_with_qp() {
        let m = Model { coded: true, bits: 8000, step: step16(26), s: 1000, qp: 26, avg_s: 1000 };
        assert_eq!(m.predict(1000, 26), 8000);
        assert_eq!(m.predict(1000, 32), 4000);
        assert_eq!(m.predict(2000, 26), 16000);
        for q in 16..51 {
            assert!(m.predict(1000, q + 1) <= m.predict(1000, q));
        }
    }

    #[test]
    fn refused_rates() {
        let ok = RateControl { bits_per_second: 64_000, fps_num: 30, fps_den: 1, mode: RcMode::Cbr };
        assert!(Rc::new(ok, 30, 26).is_ok());
        for bad in [
            RateControl { bits_per_second: 0, ..ok },
            RateControl { fps_num: 0, ..ok },
            RateControl { fps_den: 0, ..ok },
            RateControl { mode: RcMode::Vbr { max_bits_per_second: 32_000 }, ..ok },
        ] {
            assert_eq!(Rc::new(bad, 30, 26).err(), Some(Error::Rate));
        }
    }

    /// M2: a textured sequence far from its target moves the QP within pictures (rows at different
    /// QPs, so mb_qp_delta is written non-zero; tests/rc.rs proves the decode equals the recon).
    #[test]
    fn rows_take_different_qps() {
        let (w, h) = (64usize, 64usize);
        let rate = RateControl { bits_per_second: 20_000, fps_num: 30, fps_den: 1, mode: RcMode::Cbr };
        let mut enc = Encoder::new(Config { width: w as u32, height: h as u32, qp: 20, gop: 8, rate: Some(rate), ..Default::default() }).unwrap();
        let mut spread = 0;
        for t in 0..16usize {
            let y: Vec<u8> = (0..w * h).map(|i| ((i % w) * 3 + (i / w) * (i / w) / 8 + t * 5 + ((i * 7919) % 13)) as u8).collect();
            let c = vec![128u8; w * h / 4];
            enc.encode(&Yuv420 { width: w as u32, height: h as u32, y: &y, u: &c, v: &c, y_stride: w, uv_stride: w / 2 }).unwrap();
            let span = enc.rc.as_ref().unwrap().span;
            spread = spread.max(span.1 - span.0);
        }
        assert!(spread > 0, "no picture moved its rows' QP");
    }
}
