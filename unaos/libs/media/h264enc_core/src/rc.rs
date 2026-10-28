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
//! Owed (handlers.md §5 H264ENC-RC): two-pass, scene-cut IDRs, per-MB activity masking, HRD signalling.

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
}

impl Rc {
    pub(crate) fn new(rate: RateControl, gop: u32, start_qp: i32, floor: i32) -> Result<Rc> {
        let max = match rate.mode {
            RcMode::Cbr | RcMode::Crf(_) => rate.bits_per_second,
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
        let qp = if m.coded { (lo..=hi).find(|&q| m.predict(s, q) <= target as u128).unwrap_or(hi) } else { refq.max(self.floor) };
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
        self.err += b - self.r;
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
