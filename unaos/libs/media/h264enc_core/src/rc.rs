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
//! - B pictures (H264ENCBFRAMES SR333): a third model; the GOP's budget is shared I : P : B = r : 1 : 1/2
//!   (a B gets half a P's share: nB = (gop - 1) x b / (b + 1) B pictures per GOP at b B frames); a B
//!   picture's complexity is the smaller of its zero-motion differences from the two anchors, and its
//!   first QP is the last picture's + [`crate::B_QP_OFFSET`].
//!
//! - the one CBR constructor (H264ENC-CBRONE SR371): [`RateControl::cbr`], which Stria's `with_bitrate` and the kernel's export call.
//!
//! - H264ENCVBRCRF (SR383): [`RateControl::vbr`] beside it; constant quality [`RcMode::Crf`] through [`RateControl::crf`] (the
//!   picture QP is the constant-QP rule's at base crf — I and P at crf, a B picture its B offset above — clamped to the floor; no
//!   bit budget, no row modulation); the floor of every rate-controlled QP is [`crate::Config::qp_min`] (None = [`QP_MIN`]),
//!   read through [`crate::Config::qp_floor`].
//!
//! Owed (handlers.md §5 H264ENC-RC): none of its own (scene-cut IDRs: lib.rs, SR680; per-MB activity masking: lib.rs H264ENCAQ, SR693; two-pass: [`RcMode::TwoPass`], this file's H264ENCTWOPASS tail block, SR698; HRD signalling of the bucket: this file's H264ENCHRD tail block, SR699).

use alloc::vec::Vec;

use h264_core::tables::QPC;

use crate::{Encoder, Error, Pic, Result, B_QP_OFFSET};

/// The lowest QP the rate controller picks by default: the default of [`crate::Config::qp_min`] (SR383).
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
    /// Constant quality (H264ENCVBRCRF SR383), crf 0..=51: each picture's QP is the constant-QP rule's at base crf (B pictures
    /// their offset above), clamped to [`crate::Config::qp_floor`]..=51; `bits_per_second` is not read.
    Crf(u8),
    /// Two-pass (H264ENCTWOPASS SR698): the CBR bucket at `bits_per_second`, each picture's start QP planned from a first pass's
    /// [`PicStat`]s ([`crate::Encoder::planned`], x264's pass-2 allocation in [`pass2_qps`]); without a plan, [`RcMode::Cbr`].
    TwoPass,
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

/// The last coded picture of one type (I, P or B): what the R-Q model is fitted to.
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
    /// B pictures between anchors (SR333).
    bframes: i128,
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
    /// [I, P, B].
    types: [Model; 3],
    last_qp: i32,
    /// The lowest picture / row QP ([`crate::Config::qp_floor`], SR383).
    floor: i32,
    // The picture being coded: its type index (0 I, 1 P, 2 B).
    t: usize,
    rows: Vec<u64>,
    s: u64,
    share: i64,
    target: i64,
    pic_qp: i32,
    /// The lowest and highest row QP of the picture.
    pub(crate) span: (i32, i32),
    /// The two-pass plan (H264ENCTWOPASS SR698; None without [`crate::Encoder::planned`]).
    pass2: Option<Pass2>,
}

impl Rc {
    pub(crate) fn new(rate: RateControl, gop: u32, start_qp: i32, floor: i32) -> Result<Rc> {
        let max = match rate.mode {
            RcMode::Cbr | RcMode::Crf(_) | RcMode::TwoPass => rate.bits_per_second,
            RcMode::Vbr { max_bits_per_second } => max_bits_per_second,
        };
        let budget_ok = match rate.mode {
            RcMode::Crf(c) => c as i32 <= QP_MAX,
            _ => rate.bits_per_second != 0 && max >= rate.bits_per_second,
        };
        if !budget_ok || rate.fps_num == 0 || rate.fps_den == 0 || !(QP_MIN..=QP_MAX).contains(&floor) {
            return Err(Error::Rate);
        }
        let start_qp = start_qp.max(floor);
        let per = |bps: u32| ((bps as u64 * rate.fps_den as u64 / rate.fps_num as u64).max(1)) as i64;
        let buf = rate.bits_per_second as i64;
        Ok(Rc {
            rate,
            gop: gop as i128,
            bframes: 0,
            r: per(rate.bits_per_second),
            buf,
            full: buf / 2,
            plan: buf / 2,
            rmax: per(max),
            bmax: max as i64,
            fmax: 0,
            err: 0,
            types: [Model::default(); 3],
            last_qp: start_qp,
            floor,
            t: 0,
            rows: Vec::new(),
            s: 0,
            share: 0,
            target: 0,
            pic_qp: start_qp,
            span: (start_qp, start_qp),
            pass2: None,
        })
    }

    /// The stream's B pictures between anchors (SR333): the GOP budget's B share.
    pub(crate) fn with_bframes(mut self, b: u8) -> Rc {
        self.bframes = b as i128;
        self
    }

    /// Choose the QP of the next picture (type index `t`: 0 I, 1 P, 2 B) from its rows' complexity.
    fn picture(&mut self, t: usize, rows: Vec<u64>) -> i32 {
        let idr = t == 0;
        let s: u64 = rows.iter().sum();
        if idr {
            self.plan = self.buf / 2;
        }
        // The I / P ratio at equal QP, x16 (k * S' = bits * step of the last picture of each type).
        let [mi, mp, _] = self.types;
        let r16: i128 = if mi.coded && mp.coded {
            ((16 * mi.bits as u128 * mi.step as u128) / (mp.bits as u128 * mp.step as u128).max(1)).clamp(16, 256) as i128
        } else {
            64
        };
        // Per GOP: one I, nb B pictures at half a P's weight (8 of 16), the rest P (b = 0: nb = 0).
        let nb = (self.gop - 1) * self.bframes / (self.bframes + 1);
        let den = r16 + 16 * (self.gop - 1 - nb) + 8 * nb;
        let share = (self.r as i128 * self.gop * [r16, 16, 8][t] / den) as i64;
        let mut target = share;
        match self.rate.mode {
            RcMode::Cbr | RcMode::TwoPass => {
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
            // Constant quality never reaches the budget (rc_picture holds its QP, SR383).
            RcMode::Crf(_) => {}
        }
        let m = self.types[t];
        let refq = if m.coded {
            m.qp
        } else if t == 2 {
            (self.last_qp + B_QP_OFFSET).min(QP_MAX)
        } else {
            self.last_qp
        };
        let (lo, hi) = ((refq - PIC_STEP).max(self.floor), (refq + PIC_STEP).min(QP_MAX));
        let qp = if m.coded { (lo..=hi).find(|&q| m.predict(s, q) <= target as u128).unwrap_or(hi) } else { refq.max(self.floor) }; let (share, target, qp) = self.pass2_pick(t, s, share, target, qp); // H264ENCTWOPASS (SR698): the plan's start QP
        self.t = t;
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
        if self.crf().is_some() {
            return self.pic_qp;
        }
        let cum: u64 = self.rows[..row].iter().sum();
        let expect = self.target as i128 * cum as i128 / self.s.max(1) as i128;
        let dev = bits as i128 - expect;
        let off = (dev * 8 / self.target.max(1) as i128).clamp(-ROW_STEP as i128, ROW_STEP as i128) as i32;
        let qp = (self.pic_qp + off).clamp(self.floor, QP_MAX);
        self.span = (self.span.0.min(qp), self.span.1.max(qp));
        qp
    }

    /// Fit the model and move the buckets by the coded picture's `bits`.
    fn update(&mut self, bits: u64) {
        if self.crf().is_some() {
            return;
        }
        let m = &mut self.types[self.t];
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
        self.err += b - self.r; self.pass2_note(b); // H264ENCTWOPASS (SR698)
    }

    /// Under [`RcMode::Crf`] (SR383) its base QP.
    fn crf(&self) -> Option<i32> {
        match self.rate.mode {
            RcMode::Crf(c) => Some(c as i32),
            _ => None,
        }
    }

    /// Hold the picture at `qp` (constant quality, SR383): its rows keep it.
    fn hold(&mut self, qp: i32) {
        self.pic_qp = qp;
        self.span = (qp, qp);
    }
}

impl Encoder {
    /// The QP of the next macroblocks: QPY and its QPc (chroma_qp_index_offset 0).
    pub(crate) fn set_qp(&mut self, qp: i32) {
        self.qp = qp;
        self.qpc = QPC[qp as usize] as i32;
    }

    /// The picture's complexity per macroblock row: the luma gradient of an I picture, the zero-motion
    /// difference from the reference of a P picture (of a B picture the smaller of its differences from
    /// the two anchors, SR333), plus 64 per macroblock.
    pub(crate) fn rc_analyse(&self, pic: Pic) -> Vec<u64> {
        let idr = pic == Pic::Idr;
        let (src, refp, fwd) = (&self.src[0], &self.refp[0], &self.fwd[0]);
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
                    } else if pic == Pic::B {
                        s += (p - refp.data[y * w + x] as i32).unsigned_abs().min((p - fwd.data[y * w + x] as i32).unsigned_abs()) as u64;
                    } else {
                        s += (p - refp.data[y * w + x] as i32).unsigned_abs() as u64;
                    }
                }
            }
            rows.push(s);
        }
        rows
    }

    /// The QP of the next picture (SliceQPY): the rate controller's, else the constant QP (a B picture's
    /// [`B_QP_OFFSET`] above it, SR333; the pyramid's reference B [`crate::B_REF_QP_OFFSET`], SR349).
    pub(crate) fn rc_picture(&mut self, pic: Pic) -> i32 {
        if self.rc.is_none() {
            return self.fixed_qp(self.cfg.qp as i32, pic);
        }
        // Constant quality (SR383): the constant-QP rule at base crf, clamped to the floor; no analysis, no budget.
        if let Some((crf, floor)) = self.rc.as_ref().and_then(|rc| rc.crf().map(|c| (c, rc.floor))) {
            let qp = self.fixed_qp(crf, pic).clamp(floor, QP_MAX);
            if let Some(rc) = self.rc.as_mut() {
                rc.hold(qp);
            }
            self.set_qp(qp);
            return qp;
        }
        let rows = self.rc_analyse(pic);
        let t = match pic {
            Pic::Idr => 0,
            Pic::P => 1,
            Pic::B => 2,
        };
        let qp = self.rc.as_mut().map_or(self.cfg.qp as i32, |rc| rc.picture(t, rows));
        self.set_qp(qp);
        qp
    }

    /// The constant-QP rule (SR333 / SR349) at `base`: I and P pictures at `base`, a B picture [`B_QP_OFFSET`] above it (the
    /// pyramid's reference B [`crate::B_REF_QP_OFFSET`]), at most [`QP_MAX`]. `rate: None` at `cfg.qp`, [`RcMode::Crf`] at crf.
    fn fixed_qp(&self, base: i32, pic: Pic) -> i32 {
        if pic == Pic::B { (base + if self.tier == crate::Tier::Ref { crate::B_REF_QP_OFFSET } else { B_QP_OFFSET }).min(QP_MAX) } else { base }
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

impl RateControl {
    /// A constant bit rate of `kbps` kbit/s (`bits_per_second = kbps * 1000`, saturating) at `fps = (num, den)`:
    /// `num / den` pictures per second. A time base `tb` (seconds per tick, `1/fps`) passes `(tb.den, tb.num)`.
    pub const fn cbr(kbps: u32, fps: (u32, u32)) -> RateControl {
        RateControl { bits_per_second: kbps.saturating_mul(1000), fps_num: fps.0, fps_den: fps.1, mode: RcMode::Cbr }
    }

    /// H264ENCVBRCRF (SR383): a variable bit rate of mean `kbps` and max `max_kbps` kbit/s (each x 1000, saturating) at `fps`
    /// as [`RateControl::cbr`] takes it.
    pub const fn vbr(kbps: u32, max_kbps: u32, fps: (u32, u32)) -> RateControl {
        RateControl { bits_per_second: kbps.saturating_mul(1000), fps_num: fps.0, fps_den: fps.1, mode: RcMode::Vbr { max_bits_per_second: max_kbps.saturating_mul(1000) } }
    }

    /// H264ENCVBRCRF (SR383): constant quality `crf` (0..=51; above 51 refused by `Encoder::new`) at `fps` as
    /// [`RateControl::cbr`] takes it; no bit budget (`bits_per_second` 0).
    pub const fn crf(crf: u8, fps: (u32, u32)) -> RateControl {
        RateControl { bits_per_second: 0, fps_num: fps.0, fps_den: fps.1, mode: RcMode::Crf(crf) }
    }
}

impl crate::Config {
    /// The floor of every rate-controlled QP (SR383): [`crate::Config::qp_min`], else [`QP_MIN`].
    pub fn qp_floor(&self) -> i32 {
        self.qp_min.map_or(QP_MIN, i32::from)
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
        let ok = RateControl::cbr(64, (30, 1));
        assert!(Rc::new(ok, 30, 26, QP_MIN).is_ok());
        for bad in [
            RateControl { bits_per_second: 0, ..ok },
            RateControl { fps_num: 0, ..ok },
            RateControl { fps_den: 0, ..ok },
            RateControl { mode: RcMode::Vbr { max_bits_per_second: 32_000 }, ..ok },
            RateControl::crf(52, (30, 1)),
            RateControl::crf(26, (0, 1)),
        ] {
            assert_eq!(Rc::new(bad, 30, 26, QP_MIN).err(), Some(Error::Rate));
        }
        // SR383: CRF needs no bit rate; the floor is refused outside 16..=51.
        assert!(Rc::new(RateControl::crf(51, (30, 1)), 30, 26, QP_MIN).is_ok());
        for floor in [15, 52] {
            assert_eq!(Rc::new(ok, 30, 26, floor).err(), Some(Error::Rate));
        }
    }

    /// M2: a textured sequence far from its target moves the QP within pictures (rows at different
    /// QPs, so mb_qp_delta is written non-zero; tests/rc.rs proves the decode equals the recon).
    #[test]
    fn rows_take_different_qps() {
        let (w, h) = (64usize, 64usize);
        let rate = RateControl::cbr(20, (30, 1));
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

    /// H264ENC-CBRONE (SR371): the constructor builds the struct both callers wrote by hand.
    #[test]
    fn cbr_constructor_equals_the_literal() {
        assert_eq!(RateControl::cbr(200, (25, 1)), RateControl { bits_per_second: 200_000, fps_num: 25, fps_den: 1, mode: RcMode::Cbr });
        assert_eq!(RateControl::cbr(40, (30_000, 1_001)), RateControl { bits_per_second: 40_000, fps_num: 30_000, fps_den: 1_001, mode: RcMode::Cbr });
        assert_eq!(RateControl::cbr(u32::MAX, (25, 1)).bits_per_second, u32::MAX);
    }

    /// H264ENCVBRCRF (SR383): the CRF picture QP is the constant-QP rule at base crf (I and P at crf, a B picture 2 above, the
    /// pyramid's reference B 1 above), clamped to the floor (`Config::qp_min`, default 16) and to 51.
    #[test]
    fn crf_picture_qps_pinned() {
        let enc = |crf: u8, qp_min: Option<u8>| {
            Encoder::new(Config { width: 32, height: 32, qp: 26, gop: 8, rate: Some(RateControl::crf(crf, (30, 1))), qp_min, ..Default::default() }).unwrap()
        };
        let qps = |e: &mut Encoder| {
            e.tier = crate::Tier::Plain;
            let (i, p, b) = (e.rc_picture(Pic::Idr), e.rc_picture(Pic::P), e.rc_picture(Pic::B));
            e.tier = crate::Tier::Ref;
            let r = e.rc_picture(Pic::B);
            [i, p, b, r]
        };
        assert_eq!(qps(&mut enc(26, None)), [26, 26, 28, 27]);
        assert_eq!(qps(&mut enc(10, None)), [16, 16, 16, 16]);
        assert_eq!(qps(&mut enc(50, None)), [50, 50, 51, 51]);
        assert_eq!(qps(&mut enc(26, Some(30))), [30, 30, 30, 30]);
        let mut e = enc(26, None);
        e.rc_picture(Pic::P);
        e.rc_row(3, 1 << 20);
        assert_eq!(e.qp, 26, "a CRF picture's rows keep its QP");
    }
}

// ---- H264ENCTWOPASS (LEDGER SR698): two-pass rate control (named tail block) ----
// x264's pass 2 (encoder/ratecontrol.c `init_pass2`) in the rate model's own integers: the first pass records each coded picture
// ([`PicStat`], `Encoder::note_stat` on `code`'s rc_update line); its complexity is bits x step16(SliceQPY), the R-Q relation
// `Model::predict` already holds (bits x step = const for a picture), so a picture's bits at q are predicted complexity / step16(q).
// [`pass2_qps`] gives each anchor a qscale ~ complexity^(1 - qcomp) (qcomp 0.6), an I picture the complexity of its next P (else the
// previous) and [`IP_QP_OFFSET`] below it, a B picture its preceding anchor's QP + `B_QP_OFFSET`, and solves the scale by ONE
// bisection so the predicted total meets the target. In the second pass [`RcMode::TwoPass`] keeps the CBR bucket: `Rc::picture`'s
// QP line hands its choice to `pass2_pick`, which starts the picture at its planned QP moved by the remaining budget against the
// remaining plan, then raises it while the type's model predicts more than the bucket's room; `row_qp` corrects the rows. Owed:
// H264ENC-TWOPASS-FILE (BUILT by SR719, the H264ENCTWOPASSFILE tail block), H264ENC-TWOPASS-QCOMP and H264ENC-TWOPASS-BLUR (BUILT by SR736, the H264ENCTWOPASSBLUR tail block), H264ENC-TWOPASS-KERNEL.

/// One coded picture as the first pass saw it (H264ENCTWOPASS SR698): its type, its access unit's bits (every NAL unit), its
/// SliceQPY and its complexity bits x [`step16`]`(qp)`; its intra macroblocks of its mbs (x264's imb / nmb, H264ENCTWOPASSBLURINTRA SR751).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PicStat {
    pub kind: Pic,
    pub bits: u64,
    pub qp: i32,
    pub complexity: u64,
    pub intra_mbs: u32,
    pub mbs: u32,
}

/// x264's qcomp 0.6 as a fraction (3, 5): an anchor's qscale goes as complexity^(1 - 3/5). The knob is [`Pass2Opts::qcomp`] (SR736), this its default.
pub const QCOMP: (i64, i64) = (3, 5);
/// An I picture's planned QP below its P neighbour's: x264's ip_factor 1.4 (6 x log2 1.4 = 2.9).
pub const IP_QP_OFFSET: i32 = 3;

/// log2(max(e, 1)) with 16 fractional bits: the integer part by `ilog2`, the fraction by repeated squaring of the Q16 mantissa
/// (the tree's ONE log2: lib.rs `log2_q4` is its top four fraction bits).
pub fn log2_q16(e: u64) -> i64 {
    let e = e.max(1);
    let n = e.ilog2();
    let mut y: u64 = (((e as u128) << 16) >> n) as u64;
    let mut r = (n as i64) << 16;
    for b in (0..16).rev() {
        y = (y * y) >> 16;
        if y >= 2 << 16 {
            y >>= 1;
            r |= 1 << b;
        }
    }
    r
}

/// The QP of a picture whose planned QP is `q16` (Q16, [`plan_q16`]), `off` below, rounded and clamped `floor..=51`.
fn plan_qp(q16: i64, off: i32, floor: i32) -> i32 {
    ((q16 + (1 << 15)) >> 16).saturating_sub(off as i64).clamp(floor as i64, QP_MAX as i64) as i32
}

/// The planned QP of every picture of a first pass (H264ENCTWOPASS SR698, x264's `init_pass2`) for `target_bits` over them all,
/// each within `floor..=51`: the smallest scale whose predicted total (sum of complexity / step16(qp)) is at most the target.
pub fn pass2_qps(stats: &[PicStat], target_bits: u64, floor: i32, opts: &Pass2Opts) -> Vec<i32> {
    let blurred = blur_complexity(stats, opts.cplxblur);
    let logs: Vec<i64> = stats
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let a = if p.kind == Pic::Idr && !opts.i_from_gop { (i + 1..stats.len()).chain((0..i).rev()).find(|&j| stats[j].kind == Pic::P).unwrap_or(i) } else { i };
            log2_q16(blurred[a])
        })
        .collect();
    let at = |s: i64| -> Vec<i32> { pass2_at(stats, &logs, s, floor, opts) };
    let total = |q: &[i32]| -> u128 { stats.iter().zip(q).map(|(p, &q)| p.complexity as u128 / step16(q) as u128).sum() };
    let (mut lo, mut hi) = (-(64i64 << 16), 64i64 << 16);
    if total(&at(lo)) <= target_bits as u128 {
        return at(lo);
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if total(&at(mid)) <= target_bits as u128 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    at(hi)
}

/// The second pass's plan ([`crate::Encoder::planned`]): per picture its type, planned QP and predicted bits; the pictures coded so
/// far, their bits, and the target over the whole plan.
#[derive(Debug, Clone)]
struct Pass2 {
    kinds: Vec<Pic>,
    qps: Vec<i32>,
    bits: Vec<u64>,
    n: usize,
    spent: i64,
    total: i64,
}

impl Rc {
    /// Under a plan, picture `t` (its index's type matching): (share, target, QP) from the plan, the remaining budget against the
    /// remaining plan, and the bucket; otherwise the CBR choice unchanged.
    fn pass2_pick(&self, t: usize, s: u64, share: i64, target: i64, qp: i32) -> (i64, i64, i32) {
        let Some(p) = self.pass2.as_ref() else { return (share, target, qp) };
        if p.n >= p.qps.len() || p.kinds[p.n] != [Pic::Idr, Pic::P, Pic::B][t] {
            return (share, target, qp);
        }
        let rp = p.bits[p.n..].iter().sum::<u64>().max(1) as i64;
        let rb = (p.total - p.spent).clamp(rp / 2, 2 * rp).max(1);
        let share = (p.bits[p.n].max(1) as i128 * rb as i128 / rp as i128) as i64;
        let off = ((6 * (log2_q16(rp as u64) - log2_q16(rb as u64)) + (1 << 15)) >> 16).clamp(-PIC_STEP as i64, PIC_STEP as i64) as i32;
        let mut qp = (p.qps[p.n] + off).clamp(self.floor, QP_MAX);
        let room = (self.buf - self.full).max(self.r / 8);
        let m = self.types[t];
        while m.coded && qp < QP_MAX && m.predict(s, qp) > room as u128 {
            qp += 1;
        }
        (share, share.min(self.buf - self.full).max(self.r / 8), qp)
    }

    /// After a picture under a plan: its `bits` spent, the next index.
    fn pass2_note(&mut self, bits: i64) {
        if let Some(p) = self.pass2.as_mut() {
            p.spent += bits;
            p.n += 1;
        }
    }
}

impl RateControl {
    /// H264ENCTWOPASS (SR698): two-pass at a mean of `kbps` kbit/s (x 1000, saturating) at `fps` as [`RateControl::cbr`] takes it;
    /// the plan enters through [`crate::Encoder::planned`].
    pub const fn two_pass(kbps: u32, fps: (u32, u32)) -> RateControl {
        RateControl { bits_per_second: kbps.saturating_mul(1000), fps_num: fps.0, fps_den: fps.1, mode: RcMode::TwoPass }
    }
}

impl Encoder {
    /// H264ENCTWOPASS (SR698): plan the second pass from a first pass's `stats` (the same frames, the same picture types in coding
    /// order). An empty slice returns the encoder unchanged; a plan needs [`RcMode::TwoPass`] (else [`Error::Rate`]). The target is
    /// `bits_per_second` x pictures x fps_den / fps_num.
    pub fn planned(self, stats: &[PicStat]) -> Result<Encoder> {
        self.planned_with(stats, &Pass2Opts::default()) // H264ENCTWOPASSBLUR (SR736): x264's default blurs and I rule
    }

    /// H264ENCTWOPASS (SR698): one record per coded picture, in coding order, under every rate mode.
    pub fn stats(&self) -> &[PicStat] {
        &self.stats
    }

    /// After picture `kind` at SliceQPY `qp` with `intra_mbs` intra macroblocks: its record from the access unit's NAL units.
    pub(crate) fn note_stat(&mut self, kind: Pic, qp: i32, intra_mbs: u32, nals: &[Vec<u8>]) {
        let bits: u64 = nals.iter().filter(|n| n[0] & 0x1f != h264_core::nal::NAL_FILLER).map(|n| n.len() as u64 * 8).sum();
        self.stats.push(PicStat { kind, bits, qp, complexity: bits * step16(qp), intra_mbs, mbs: (self.mb_w * self.mb_h) as u32 });
    }
}

#[cfg(test)]
mod twopass_tests {
    use super::*;

    /// The ONE log2: AQ's Q4 is the top four fraction bits of the Q16.
    #[test]
    fn log2_q4_is_log2_q16_top_bits() {
        for e in [0u32, 1, 2, 3, 5, 7, 100, 1000, 4095, 65_535, 1 << 20, 123_456_789, u32::MAX] {
            assert_eq!(crate::log2_q4(e), log2_q16(e as u64) >> 12, "{e}");
        }
        assert_eq!(log2_q16(1 << 40), 40 << 16);
    }

    #[test]
    fn two_pass_constructor_equals_the_literal() {
        assert_eq!(RateControl::two_pass(200, (25, 1)), RateControl { bits_per_second: 200_000, fps_num: 25, fps_den: 1, mode: RcMode::TwoPass });
    }
}

// ---- H264ENCHRD (LEDGER SR699): the bucket's HRD view (named tail block) ----
// The HRD the stream signals IS this file's bucket: CBR `buf` / `full` drained at the mean rate (cbr_flag 1), VBR `bmax` / `fmax`
// drained at the max rate (cbr_flag 0); two-pass (SR698) keeps the CBR bucket and is signalled as it. The one correction: `r` is floor(bps x den / num), while a decoder drains the exact
// rate, so after n pictures the CBR model is fuller than the decoder's mirror by floor(n X) - n r bits; `hrd_bucket` removes it.
impl Rc {
    /// The bucket as hrd_parameters + timing_info (None under CRF: no bucket).
    pub(crate) fn hrd_params(&self) -> Option<crate::sps::Hrd> {
        let (bit_rate, cpb_size, cbr) = match self.rate.mode {
            RcMode::Cbr | RcMode::TwoPass => (self.rate.bits_per_second, self.buf as u32, true),
            RcMode::Vbr { max_bits_per_second } => (max_bits_per_second, self.bmax as u32, false),
            RcMode::Crf(_) => return None,
        };
        Some(crate::sps::Hrd { bit_rate, cpb_size, cbr, num_units_in_tick: self.rate.fps_den, time_scale: self.rate.fps_num.saturating_mul(2) })
    }

    /// Before picture `n` (pictures coded so far): (cbr, BitRate, CpbSize, the fullness in bits, the next picture's drain).
    /// CBR: the exact-rate fullness and drain floor((n + 1) X) - floor(n X), X = bps x den / num; VBR: the max bucket's own.
    pub(crate) fn hrd_bucket(&self, n: u64) -> Option<(bool, i64, i64, i64, i64)> {
        match self.rate.mode {
            RcMode::Cbr | RcMode::TwoPass => {
                let (bps, den, num) = (self.rate.bits_per_second as i128, self.rate.fps_den as i128, self.rate.fps_num as i128);
                let drained = |k: i128| k * bps * den / num;
                let n = n as i128;
                let full = self.full as i128 - (drained(n) - n * self.r as i128);
                Some((true, bps as i64, self.buf, full as i64, (drained(n + 1) - drained(n)) as i64))
            }
            RcMode::Vbr { max_bits_per_second } => Some((false, max_bits_per_second as i64, self.bmax, self.fmax, self.rmax)),
            RcMode::Crf(_) => None,
        }
    }

    /// Filler bits (CBR) into the bucket before `update` (which never sees them: the R-Q model fits the coded bits only).
    pub(crate) fn hrd_pad(&mut self, bits: u64) {
        self.full += bits as i64;
        self.err += bits as i64;
    }
}

impl RateControl {
    /// Under the HRD (`on`, CBR / VBR) the rates rounded down to a multiple of 64 (BitRate's finest unit, E.2.2), so the
    /// signalled BitRate and CpbSize are the bucket's exactly; zero after rounding or a time_scale (2 fps_num) past u32 is
    /// [`Error::Rate`]. Off, or CRF, unchanged.
    pub(crate) fn for_hrd(self, on: bool) -> Result<RateControl> {
        let q = |v: u32| v & !63;
        match (on, self.mode) {
            (false, _) | (_, RcMode::Crf(_)) => Ok(self),
            (true, mode) => {
                let mode = match mode {
                    RcMode::Vbr { max_bits_per_second } => RcMode::Vbr { max_bits_per_second: q(max_bits_per_second) },
                    m => m,
                };
                if q(self.bits_per_second) == 0 || self.fps_num > u32::MAX / 2 {
                    return Err(Error::Rate);
                }
                Ok(RateControl { bits_per_second: q(self.bits_per_second), mode, ..self })
            }
        }
    }
}

// ---- H264ENCTWOPASSFILE (LEDGER SR719): the first pass's records as text (named tail block) ----
// x264's `--pass 1 --stats f` / `--pass 2 --stats f`: the [`PicStat`]s of a first pass written once, read by a later process. ONE text
// form, here beside the record (the crate is no_std + alloc: out through `core::fmt::Write`, in from a `&str`; the file I/O is the
// caller's, tools/nle). A header line `h264enc-stats v<STATS_VERSION> frames=<n> fps=<num>/<den>`, then one line per picture in coding
// order: `<I|P|B> <bits> <qp> <complexity> <intra_mbs>/<mbs>` (v2, H264ENCTWOPASSBLURINTRA SR751: a v1 text is refused as `Header`), each '\n'-terminated. The reader re-checks the record's own invariant (complexity = bits x
// step16(qp), `Encoder::note_stat`'s rule) and refuses by name. Owed: H264ENC-TWOPASS-FILE-RESCALE (a pass 2 over a different frame count).

/// The stats text form's version (the header's `v<n>`).
pub const STATS_VERSION: u32 = 2;

/// A first pass read back ([`read_stats`]): the frame rate its header names and its records in coding order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsFile {
    pub fps: (u32, u32),
    pub pics: Vec<PicStat>,
}

/// A stats text [`read_stats`] refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsError {
    /// No first line, or not `h264enc-stats v<STATS_VERSION> frames=<n> fps=<num>/<den>` (num and den nonzero).
    Header,
    /// File line `n` (1-based) is not `<I|P|B> <bits> <qp> <complexity> <intra_mbs>/<mbs>` with qp in 0..=51, complexity = bits x step16(qp), intra_mbs <= mbs.
    Line(usize),
    /// The header's frame count against the picture lines the text holds.
    Count { header: usize, lines: usize },
}

impl core::fmt::Display for StatsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StatsError::Header => write!(f, "h264enc: stats: bad header (want `h264enc-stats v{STATS_VERSION} frames=<n> fps=<num>/<den>`)"),
            StatsError::Line(n) => write!(f, "h264enc: stats: bad line {n} (want `<I|P|B> <bits> <qp> <complexity> <intra_mbs>/<mbs>`)"),
            StatsError::Count { header, lines } => write!(f, "h264enc: stats: the header says {header} pictures, the text holds {lines}"),
        }
    }
}

/// The one-letter code of a picture type in the stats text.
fn kind_code(k: Pic) -> char {
    match k {
        Pic::Idr => 'I',
        Pic::P => 'P',
        Pic::B => 'B',
    }
}

fn kind_of(s: &str) -> Option<Pic> {
    match s {
        "I" => Some(Pic::Idr),
        "P" => Some(Pic::P),
        "B" => Some(Pic::B),
        _ => None,
    }
}

/// Write a first pass's records at frame rate `fps` (num, den) as the stats text: the header, then one line per picture.
pub fn write_stats(pics: &[PicStat], fps: (u32, u32), w: &mut impl core::fmt::Write) -> core::fmt::Result {
    writeln!(w, "h264enc-stats v{STATS_VERSION} frames={} fps={}/{}", pics.len(), fps.0, fps.1)?;
    for p in pics {
        writeln!(w, "{} {} {} {} {}/{}", kind_code(p.kind), p.bits, p.qp, p.complexity, p.intra_mbs, p.mbs)?;
    }
    Ok(())
}

/// Read the stats text [`write_stats`] writes; a refusal names the header, the line, or the count.
pub fn read_stats(text: &str) -> core::result::Result<StatsFile, StatsError> {
    let mut lines = text.lines();
    let head = lines.next().ok_or(StatsError::Header)?;
    let (frames, fps) = parse_stats_header(head).ok_or(StatsError::Header)?;
    let mut pics = Vec::with_capacity(frames.min(1 << 20));
    for (i, l) in lines.enumerate() {
        pics.push(parse_stats_line(l).ok_or(StatsError::Line(i + 2))?);
    }
    if pics.len() != frames {
        return Err(StatsError::Count { header: frames, lines: pics.len() });
    }
    Ok(StatsFile { fps, pics })
}

fn parse_stats_header(l: &str) -> Option<(usize, (u32, u32))> {
    let mut f = l.split(' ');
    if f.next()? != "h264enc-stats" || f.next()?.strip_prefix('v')?.parse::<u32>().ok()? != STATS_VERSION {
        return None;
    }
    let frames = f.next()?.strip_prefix("frames=")?.parse().ok()?;
    let (num, den) = f.next()?.strip_prefix("fps=")?.split_once('/')?;
    let fps: (u32, u32) = (num.parse().ok()?, den.parse().ok()?);
    (f.next().is_none() && fps.0 != 0 && fps.1 != 0).then_some((frames, fps))
}

fn parse_stats_line(l: &str) -> Option<PicStat> {
    let mut f = l.split(' ');
    let kind = kind_of(f.next()?)?;
    let bits: u64 = f.next()?.parse().ok()?;
    let qp: i32 = f.next()?.parse().ok()?;
    let complexity: u64 = f.next()?.parse().ok()?;
    let (intra_mbs, mbs) = f.next()?.split_once('/')?;
    let (intra_mbs, mbs): (u32, u32) = (intra_mbs.parse().ok()?, mbs.parse().ok()?);
    if f.next().is_some() || !(0..=QP_MAX).contains(&qp) || bits.checked_mul(step16(qp)) != Some(complexity) || intra_mbs > mbs {
        return None;
    }
    Some(PicStat { kind, bits, qp, complexity, intra_mbs, mbs })
}

// ---- H264ENCTWOPASSRESCALE (LEDGER SR727): a first pass rescaled to another frame count (named tail block) ----
// A pass 2 over a different frame count (a trimmed timeline, a range exported from a whole-timeline pass 1) plans from the first
// pass's complexity curve resampled to the new count; `Encoder::planned` / `pass2_qps` then plan whatever length they are given,
// unchanged. UnaOS's own rule, not x264's (x264 refuses a pass 2 with more frames than its stats and truncates one with fewer).
// Owed: H264ENC-TWOPASS-RESCALE-KIND (a resampled kind that misses the encoder's own I), H264ENC-TWOPASS-RESCALE-RANGE.

/// `stats` (m records) resampled to `n` pictures (H264ENCTWOPASSRESCALE SR727): picture j covers the records
/// [j m / n, max((j + 1) m / n, j m / n + 1)), its complexity the integer mean of theirs, its kind and qp the covered span's
/// lower-middle record's (so 30 -> 15 keeps record 0's I), its bits mean / step16(qp) and its complexity bits x step16(qp), so a
/// rescaled record keeps [`PicStat`]'s invariant and round-trips through [`write_stats`] / [`read_stats`]. m == n returns the
/// records unchanged; m == 0 or n == 0 an empty plan.
pub fn rescale_stats(stats: &[PicStat], n: usize) -> Vec<PicStat> {
    let m = stats.len();
    if m == n {
        return stats.to_vec();
    }
    if m == 0 || n == 0 {
        return Vec::new();
    }
    (0..n)
        .map(|j| {
            let start = (j as u128 * m as u128 / n as u128) as usize;
            let end = (((j as u128 + 1) * m as u128 / n as u128) as usize).max(start + 1).min(m);
            let span = &stats[start..end];
            let mean = (span.iter().map(|p| p.complexity as u128).sum::<u128>() / span.len() as u128) as u64;
            let c = &span[(span.len() - 1) / 2];
            let bits = mean / step16(c.qp);
            PicStat { kind: c.kind, bits, qp: c.qp, complexity: bits * step16(c.qp), intra_mbs: (span.iter().map(|p| p.intra_mbs as u64).sum::<u64>() / span.len() as u64) as u32, mbs: (span.iter().map(|p| p.mbs as u64).sum::<u64>() / span.len() as u64) as u32 }
        })
        .collect()
}

// ---- H264ENCTWOPASSBLUR (LEDGER SR736): x264's complexity blur, qblur and GOP-mean I rule, the qcomp knob (named tail block) ----
// x264's `init_pass2` (encoder/ratecontrol.c) in the plan's integers, inside [`pass2_qps`]: each picture's complexity is first
// BLURRED ([`blur_complexity`]: a fixed gaussian exp(-j^2/200) over 2 x cplxblur pictures each side, the walk cut at an IDR), its
// anchor QP planned from the blurred value at qscale ~ complexity^(1 - qcomp) ([`Pass2Opts::qcomp`], the knob), an I picture given
// the mean planned QP of the P pictures that follow it up to the next I less [`IP_QP_OFFSET`] (`get_diff_limited_q`, run backward;
// its own when none follows), then the anchors' QP curve blurred over same-type neighbours ([`blur_qscale`], qblur), rounded and
// clamped; a B picture its anchor's + `B_QP_OFFSET` as SR698. The bisection and the predicted total (each picture's OWN complexity /
// step16) are SR698's. Departures, named: the intra mask WAS the kind (an IDR all intra, a P / B none; x264's weight by the intra
// macroblock count since H264ENCTWOPASSBLURINTRA SR751), and qblur averages the Q16 QP (log qscale) where x264 averages the linear qscale. [`Pass2Opts`] with cplxblur 0,
// qblur 0 and `i_from_gop` false plans SR698's exact QPs. Owed: H264ENC-TWOPASS-BLUR-INTRA, H264ENC-TWOPASS-QBLUR-LINEAR,
// H264ENC-TWOPASS-QPSTEP, H264ENC-TWOPASS-BLUR-RATETOL, H264ENC-TWOPASS-QCOMP-CLI, H264ENC-TWOPASS-BLUR-RUNTIME.

/// The options of a second pass's plan (H264ENCTWOPASSBLUR SR736); Default = x264's (qcomp 0.6, cplxblur 20, qblur 0.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pass2Opts {
    /// x264's qcompress as (num, den), 0 <= num <= den, den > 0: an anchor's qscale ~ complexity^(1 - num / den); (1, 1) one QP
    /// for every P picture (constant-QP-like), (0, 1) qscale ~ complexity (equal bits per picture).
    pub qcomp: (i64, i64),
    /// x264's cplxblur in pictures: the complexity gaussian's window, 2 x cplxblur each side; 0 = each picture's own.
    pub cplxblur: u32,
    /// x264's qblur in Q8 (128 = 0.5): the QP curve's gaussian exp(-d^2 / qblur^2) over (qblur x 4) | 1 pictures; below 64, none.
    pub qblur_q8: u32,
    /// x264's I rule (the mean planned QP of the P pictures after it up to the next I, less [`IP_QP_OFFSET`]); false = SR698's
    /// (its next P's complexity, else its previous P's, less [`IP_QP_OFFSET`]).
    pub i_from_gop: bool,
}

impl Default for Pass2Opts {
    fn default() -> Pass2Opts {
        Pass2Opts { qcomp: QCOMP, cplxblur: 20, qblur_q8: 128, i_from_gop: true }
    }
}

impl Pass2Opts {
    /// The qcomp fraction is within 0..=1 with a positive denominator.
    pub fn valid(&self) -> bool {
        self.qcomp.1 > 0 && (0..=self.qcomp.1).contains(&self.qcomp.0)
    }
}

/// e^-k x 2^32, k = 0..=11 (e^-12 x 65536 < 0.5).
const E_NEG_Q32: [u64; 12] = [4294967296, 1580030169, 581260615, 213833830, 78665070, 28939262, 10646160, 3916503, 1440801, 530041, 194991, 71733];

/// round(65536 x e^-t) for t = `t_q16` / 65536, in integers: e^-k from [`E_NEG_Q32`] for the integer part, e^-f by its Taylor series
/// in Q32 for the fraction. The ONE exponential of both blurs.
pub fn exp_neg_q16(t_q16: u64) -> u64 {
    let k = (t_q16 >> 16) as usize;
    if k >= E_NEG_Q32.len() {
        return 0;
    }
    let f = (t_q16 & 0xffff) as i128;
    let (mut term, mut sum) = (1i128 << 32, 1i128 << 32);
    for n in 1..16i128 {
        term = -((term * f) >> 16) / n;
        sum += term;
    }
    let q32 = (E_NEG_Q32[k] as u128 * sum.max(0) as u128) >> 32;
    ((q32 + (1 << 15)) >> 16) as u64
}

/// Each picture's complexity blurred as x264's `init_pass2` blurs it (H264ENCTWOPASSBLUR SR736): the weighted mean, weights
/// exp(-j^2/200) in Q16, of the pictures j = 1 .. < 2 x `cplxblur` after it and j = 0 ..= 2 x `cplxblur` before it, each scaled by
/// x264's running intra weight ([`intra_keep_q16`], SR751: after it multiplied before a picture is summed, before it after; the walk
/// stops below 1/10000, so an all-intra picture (an IDR) cuts it). `cplxblur` 0 returns each picture's own complexity.
pub fn blur_complexity(stats: &[PicStat], cplxblur: u32) -> Vec<u64> {
    let n = stats.len();
    let w = 2 * cplxblur as usize;
    let g = |j: usize| exp_neg_q16(((j as u64 * j as u64) << 16) / 200) as u128;
    (0..n)
        .map(|i| {
            let (mut ws, mut cs, mut wt) = (0u128, 0u128, 1u128 << 16);
            for j in 1..w.min(n - i) {
                wt = (wt * intra_keep_q16(&stats[i + j])) >> 16;
                if wt * 10_000 < 1 << 16 {
                    break;
                }
                ws += wt * g(j);
                cs += wt * g(j) * stats[i + j].complexity as u128;
            }
            wt = 1 << 16;
            for j in 0..=w.min(i) {
                ws += wt * g(j);
                cs += wt * g(j) * stats[i - j].complexity as u128;
                wt = (wt * intra_keep_q16(&stats[i - j])) >> 16;
                if wt * 10_000 < 1 << 16 {
                    break;
                }
            }
            (cs / ws.max(1)) as u64
        })
        .collect()
}

/// The anchors' planned QPs `q16` (Q16) blurred as x264 blurs qscale (H264ENCTWOPASSBLUR SR736): over the (qblur x 4) | 1 pictures
/// centred on each, same kind only, weights exp(-d^2 / qblur^2) with qblur = `qblur_q8` / 256, floor of the weighted mean. B
/// entries are returned unchanged (a B follows its anchor); a window of one picture (`qblur_q8` < 64) returns `q16` unchanged.
pub fn blur_qscale(q16: &[i64], kinds: &[Pic], qblur_q8: u32) -> Vec<i64> {
    let half = ((((qblur_q8 as u64 * 4) >> 8) | 1) / 2) as usize;
    if half == 0 || q16.is_empty() {
        return q16.to_vec();
    }
    let qq = qblur_q8 as u128 * qblur_q8 as u128;
    (0..q16.len())
        .map(|i| {
            if kinds[i] == Pic::B {
                return q16[i];
            }
            let (mut num, mut den) = (0i128, 0i128);
            for idx in i.saturating_sub(half)..=(i + half).min(q16.len() - 1) {
                if kinds[idx] != kinds[i] {
                    continue;
                }
                let d = idx.abs_diff(i) as u128;
                let c = exp_neg_q16((((d * d) << 32) / qq).min(u64::MAX as u128) as u64) as i128;
                num += c * q16[idx] as i128;
                den += c;
            }
            num.div_euclid(den.max(1)) as i64
        })
        .collect()
}

/// The planned QP (Q16, unrounded) of a picture whose anchor complexity has log2 `l` (Q16) at scale log2 `s` (Q16):
/// log2 step16(q) = 4 + (q - 4) / 6, so q = 6 (s + (1 - qcomp) l) - 20.
fn plan_q16(s: i64, l: i64, qcomp: (i64, i64)) -> i64 {
    6 * s + 6 * l * (qcomp.1 - qcomp.0) / qcomp.1 - (20 << 16)
}

/// [`pass2_qps`]'s plan at scale `s` (H264ENCTWOPASSBLUR SR736): every anchor's Q16 QP from its (blurred) log complexity, the I
/// rule, [`blur_qscale`], then rounded and clamped `floor..=51`; a B its previous anchor's QP + `B_QP_OFFSET`.
fn pass2_at(stats: &[PicStat], logs: &[i64], s: i64, floor: i32, opts: &Pass2Opts) -> Vec<i32> {
    let n = stats.len();
    let kinds: Vec<Pic> = stats.iter().map(|p| p.kind).collect();
    let mut q16: Vec<i64> = logs.iter().map(|&l| plan_q16(s, l, opts.qcomp)).collect();
    let mut off = alloc::vec![0i32; n];
    for i in 0..n {
        if kinds[i] != Pic::Idr {
            continue;
        }
        off[i] = IP_QP_OFFSET;
        if opts.i_from_gop {
            let ps: Vec<i64> = (i + 1..n).take_while(|&j| kinds[j] != Pic::Idr).filter(|&j| kinds[j] == Pic::P).map(|j| q16[j]).collect();
            if ps.is_empty() {
                off[i] = 0;
            } else {
                q16[i] = ps.iter().sum::<i64>().div_euclid(ps.len() as i64);
            }
        }
    }
    let q16 = blur_qscale(&q16, &kinds, opts.qblur_q8);
    let mut out = Vec::with_capacity(n);
    let mut anchor: Option<i32> = None;
    for i in 0..n {
        let q = match (kinds[i], anchor) {
            (Pic::B, Some(a)) => (a + B_QP_OFFSET).clamp(floor, QP_MAX),
            _ => plan_qp(q16[i], off[i], floor),
        };
        if kinds[i] != Pic::B {
            anchor = Some(q);
        }
        out.push(q);
    }
    out
}

impl Encoder {
    /// H264ENCTWOPASSBLUR (SR736): [`Encoder::planned`] with the plan's options. An empty slice returns the encoder unchanged; a
    /// plan needs [`RcMode::TwoPass`] and valid `opts` ([`Pass2Opts::valid`]), else [`Error::Rate`]. The target is
    /// `bits_per_second` x pictures x fps_den / fps_num.
    pub fn planned_with(mut self, stats: &[PicStat], opts: &Pass2Opts) -> Result<Encoder> {
        if stats.is_empty() {
            return Ok(self);
        }
        let Some(rc) = self.rc.as_mut().filter(|rc| rc.rate.mode == RcMode::TwoPass) else { return Err(Error::Rate) };
        if !opts.valid() {
            return Err(Error::Rate);
        }
        let rate = rc.rate;
        let total = (rate.bits_per_second as u128 * stats.len() as u128 * rate.fps_den as u128 / rate.fps_num as u128) as u64;
        let qps = pass2_qps(stats, total, rc.floor, opts);
        let bits = stats.iter().zip(&qps).map(|(p, &q)| p.complexity / step16(q)).collect();
        rc.pass2 = Some(Pass2 { kinds: stats.iter().map(|p| p.kind).collect(), qps, bits, n: 0, spent: 0, total: total as i64 });
        Ok(self)
    }
}

// ---- H264ENCTWOPASSBLURINTRA (LEDGER SR751): x264's intra weight of the complexity blur's chain (named tail block) ----
// x264's `init_pass2` (ratecontrol.c) scales each link of the blur's walk by `1 - pow(i_count / nmb, 2)`, the picture's intra
// macroblock share squared ([`PicStat::intra_mbs`] / [`PicStat::mbs`], counted by `Encoder::note_stat`'s caller per picture), so a
// mostly-intra P cuts the chain too. The coder codes intra macroblocks in an IDR only today (every P / B carries 0): the weight is
// then exactly SR736's binary chain. Owed: H264ENC-P-INTRA (the P / B macroblock's intra decision).

/// x264's per-link blur weight of `p`, 1 - (intra_mbs / mbs)^2, in Q16 (H264ENCTWOPASSBLURINTRA SR751): 65536 with no intra
/// macroblock, 0 when all are intra; intra_mbs is clamped to mbs, and mbs 0 keeps 65536.
pub fn intra_keep_q16(p: &PicStat) -> u128 {
    let (i, m) = (p.intra_mbs.min(p.mbs) as u128, p.mbs.max(1) as u128);
    (1 << 16) - ((i * i) << 16) / (m * m)
}
