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
//! - the picture QP (`Rc::picture`): a coded type the smallest q in [refq - [`PIC_STEP`], refq + [`PIC_STEP`]] and in [floor, 51] whose predicted bits meet the
//!   target, an uncoded type refq; refq = the type's last QP, the first P after an IDR the I QP + [`IP_QP_OFFSET`] (SR844, pinned by `firstp_ipoffset_steady_scene_idr`),
//!   an uncoded type the last picture's (+ [`B_QP_OFFSET`] for a B); a two-pass plan replaces it (`pass2_pick`, [`qpstep_clamp`] SR769), written as slice_qp_delta; within the picture
//!   each macroblock row's QP moves by up to +-[`ROW_STEP`] 3 from it by the running bits against the row's share of the target (the rows' SAD);
//!   under VBR `Rc::room_step` (H264ENCHRDVBRUNDERFLOW SR1050) raises the rows by up to [`VBV_ROW_STEP`] 6 more while the projected picture would underflow the max bucket, never falling within the picture (pinned by tests/hrd.rs `i_vbr_knife_edge_does_not_underflow`);
//!   written as mb_qp_delta (lib.rs `write_mb`, QPY,pred tracked as h264_core's `mb.rs` does).
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
//! - two-pass (H264ENCTWOPASS SR698, x264's `init_pass2`): pass 1 records a [`PicStat`] per coded picture (`Encoder::note_stat`: bits, SliceQPY, complexity = bits x [`step16`]`(qp)`);
//!   [`crate::Encoder::planned`] / [`crate::Encoder::planned_with`] plan pass 2 by [`pass2_qps`]: an anchor's qscale ~ its [`blur_complexity`]-blurred complexity^(1 - qcomp), an I the mean planned QP of its GOP's Ps less [`IP_QP_OFFSET`], the curve held by [`qpstep_limit`] (SR763) and blurred by [`blur_qscale`] (linear qscale, SR1042), a B its anchor's + [`B_QP_OFFSET`], the scale by ONE bisection to the target (`k_raw_reproduces_sr698`, `l_default_blur_lands_within_two_percent`);
//!   [`Pass2Opts`]' Default is x264's (qcomp 0.6, cplxblur 20, qblur 0.5, the GOP-mean I rule, qpstep [`QP_STEP`] 4; H264ENCTWOPASSBLUR SR736).
//!   Pass 2: [`RcMode::TwoPass`] keeps the CBR bucket and target; `pass2_pick` replaces the picture-QP window: the planned QP + round(6 log2(remaining plan / remaining budget)) clamped +-[`PIC_STEP`] and to [floor, 51], then [`qpstep_clamp`] against the type's last pass-2 QP or, for the first P, [`qpstep_seed`] (SR769 / SR804, the one bound [`qpstep_bound`] SR1026),
//!   then raised while the type's model predicts more than the bucket's room (`b_two_pass_lands_within_two_percent`, `z_first_p_held_against_the_seed`, `x_the_bucket_raise_wins_over_the_clamp`; all tests/twopass.rs).
//!   The records as text: [`write_stats`] / [`read_stats`] (H264ENCTWOPASSFILE SR719, `d_stats_text_round_trips`); a pass 1 over another frame count: [`rescale_stats`] (H264ENCTWOPASSRESCALE SR727, `g_rescale_30_to_15_means_pairs`).
//!
//! - scene cuts and the GOP start: the detector is lib.rs' `Encoder::scenecut_key` (H264ENCSCENECUT SR680, x264's `scenecut_internal`; `scenecut_lookahead` under bframes), on by [`crate::Config::scenecut`]; a cut codes an IDR and the rate model resets nothing beyond it (H264ENCSCENECUTRCRESET SR834: on fixtures A flat -> texture, B static -> motion, C motion -> static the first P mispredicts the same at a scheduled IDR in the same place, so a detected-only reset is the wrong rule; `scenecut_rc_first_p_table`).
//!   The first P after any IDR windows on the I QP + [`IP_QP_OFFSET`] (SR844, above) but predicts from the stale P model: of (a) the stale model, (b) the I's bits moved by the QP step, (c) the I model on the P's S', (d) (a) damped toward (c), none brings 12 of the 14 budget-binding runs within 2 QP of the oracle (a 7, b 2, c 6, d 7), so (a) stays (H264ENCRCFIRSTPPREDICTION SR1002, `firstp_prediction_table`).
//!   The first B after an IDR (bframes 2 codes I B B P as I P B B, so `last_qp` is the first P's) takes the stale B model's QP from the second GOP on; the arm first P QP + [`B_QP_OFFSET`] fixes 2 of the 6 binding misses and worsens 5 of 12 runs, so the stale B stays (H264ENCRCFIRSTBOFFSET SR1010, `firstb_offset_table`); both residual mispredictions are H264ENC-RC-CPLX-WINDOW's (x264's stream-long rate factor).
//!
//! Owed (handlers.md §5 H264ENC-RC): H264ENC-RC-CPLX-WINDOW (above); the kernel export (`video/export.rs`) calls only [`RateControl::cbr`] / [`RateControl::crf`], no two-pass road (H264ENC-TWOPASS-KERNEL, owed with SR698); per-MB activity masking: lib.rs H264ENCAQ, SR693; HRD signalling of the bucket: this file's H264ENCHRD tail block, SR699.

use alloc::vec::Vec;

use h264_core::tables::QPC;

use crate::{Encoder, Error, Pic, Result, B_QP_OFFSET};

/// The lowest QP the rate controller picks by default: the default of [`crate::Config::qp_min`] (SR383).
pub const QP_MIN: i32 = 16;
/// The highest QP the rate controller picks (the Recommendation's QPY limit).
pub const QP_MAX: i32 = 51;
/// The half-width of a picture's QP window around its refq (`Rc::picture`, the module doc's rule).
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
    /// H264ENCHRDVBRUNDERFLOW (SR1050): VBR, the room bmax - fmax at the picture's start and the room term its rows reached.
    room: (i64, i32),
    /// sum(bits) - R * pictures.
    err: i64,
    /// [I, P, B].
    types: [Model; 3],
    last_qp: i32,
    after_idr: bool, // H264ENCRCFIRSTPIPOFFSET (SR844): an I was coded and no P since
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
            room: (0, 0),
            err: 0,
            types: [Model::default(); 3],
            last_qp: start_qp,
            after_idr: false,
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
                target = target.min(self.bmax - self.fmax).max(self.r / 8); self.room = (self.bmax - self.fmax, 0); // H264ENCHRDVBRUNDERFLOW (SR1050): the room at the picture's start
            }
            // Constant quality never reaches the budget (rc_picture holds its QP, SR383).
            RcMode::Crf(_) => {}
        }
        let m = self.types[t];
        let refq = if t == 1 && self.after_idr { (self.types[0].qp + IP_QP_OFFSET).min(QP_MAX) } else if m.coded { // H264ENCRCFIRSTPIPOFFSET (SR844): the first P after an IDR windows on the I QP + IP_QP_OFFSET (x264 ip_factor), prediction kept
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
        let qp = (self.pic_qp + off + self.room_step(row, bits)).clamp(self.floor, QP_MAX); // H264ENCHRDVBRUNDERFLOW (SR1050): the VBR room term (0 otherwise)
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
        self.last_qp = self.pic_qp; self.after_idr = self.t == 0 || (self.after_idr && self.t == 2); // H264ENCRCFIRSTPIPOFFSET (SR844): set on an I, kept through B pictures, cleared on the next P
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
    total: i64, qpstep: u8, // H264ENCTWOPASSQPSTEPRUNTIME (SR769): Pass2Opts::qpstep, the runtime clamp's step
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
        let mut qp = (p.qps[p.n] + off).clamp(self.floor, QP_MAX); qp = qpstep_clamp(qp, self.pass2_last(t).or_else(|| qpstep_seed(p.n.checked_sub(1).map(|i| (p.kinds[i], self.last_qp)), p.kinds[p.n])), p.qpstep, (off < 0, off > 0 && p.n > 3)); // H264ENCTWOPASSQPSTEPRUNTIME (SR769): x264's lstep clamp before the bucket
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
// macroblock count since H264ENCTWOPASSBLURINTRA SR751), and qblur averaged the Q16 QP (log qscale) where x264 averages the linear qscale (linear since SR1042). [`Pass2Opts`] with cplxblur 0,
// qblur 0 and `i_from_gop` false plans SR698's exact QPs. Owed: H264ENC-TWOPASS-BLUR-INTRA, H264ENC-TWOPASS-QBLUR-LINEAR (BUILT by SR1042, the H264ENCTWOPASSQBLURLINEAR tail block),
// H264ENC-TWOPASS-QPSTEP (BUILT by SR763, the H264ENCTWOPASSQPSTEP tail block), H264ENC-TWOPASS-BLUR-RATETOL, H264ENC-TWOPASS-QCOMP-CLI, H264ENC-TWOPASS-BLUR-RUNTIME.

/// The options of a second pass's plan (H264ENCTWOPASSBLUR SR736); Default = x264's (qcomp 0.6, cplxblur 20, qblur 0.5, qpstep 4).
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
    /// x264's qpstep in QP (H264ENCTWOPASSQPSTEP SR763): a planned P / I within this of the next same-type one ([`qpstep_limit`]); 0 = none.
    pub qpstep: u8,
}

impl Default for Pass2Opts {
    fn default() -> Pass2Opts {
        Pass2Opts { qcomp: QCOMP, cplxblur: 20, qblur_q8: 128, i_from_gop: true, qpstep: QP_STEP }
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
/// centred on each, same kind only, weights exp(-d^2 / qblur^2) with qblur = `qblur_q8` / 256, the mean of the LINEAR qscale (SR1042:
/// [`qp2qscale_q16`] / [`qscale2qp_q16`]). B entries are returned unchanged (a B follows its anchor); a window of one picture (`qblur_q8` < 64) returns `q16` unchanged.
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
            let win = i.saturating_sub(half)..=(i + half).min(q16.len() - 1);
            let m = win.clone().filter(|&idx| kinds[idx] == kinds[i]).map(|idx| q16[idx]).max().unwrap_or(q16[i]);
            let (mut num, mut den) = (0u128, 0u128);
            for idx in win {
                if kinds[idx] != kinds[i] {
                    continue;
                }
                let d = idx.abs_diff(i) as u128;
                let c = exp_neg_q16((((d * d) << 32) / qq).min(u64::MAX as u128) as u64) as u128;
                num += c * qp2qscale_q16(m - q16[idx]);
                den += c << 64;
            }
            m + qscale2qp_q16(num, den)
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
    let mut q16: Vec<i64> = logs.iter().map(|&l| plan_q16(s, l, opts.qcomp)).collect(); qpstep_limit(&mut q16, &kinds, opts.qpstep, Pic::P); // H264ENCTWOPASSQPSTEP (SR763): the P clamp before the I rule
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
    qpstep_limit(&mut q16, &kinds, opts.qpstep, Pic::Idr); let q16 = blur_qscale(&q16, &kinds, opts.qblur_q8);
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
        rc.pass2 = Some(Pass2 { kinds: stats.iter().map(|p| p.kind).collect(), qps, bits, n: 0, spent: 0, total: total as i64, qpstep: opts.qpstep }); // H264ENCTWOPASSQPSTEPRUNTIME (SR769): the runtime clamp's step
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

// ---- H264ENCTWOPASSQPSTEP (LEDGER SR763): x264's get_diff_limited_q qpstep clamp over the plan (named tail block) ----
// x264's `init_pass2` (ratecontrol.c) runs `get_diff_limited_q` BACKWARD over each scale's planned qscales, before the qblur and the
// clip: a picture whose next non-B picture (the last one visited) is of its own type is held within lstep = 2^(qpstep / 6) of that
// picture's (already limited) qscale, i.e. within +-qpstep QP: our Q16 QP is that log domain, so no conversion. Same type only: a B
// is never limited (the last non-B is never a B), and ip_factor / pb_factor are the I / B rules ([`IP_QP_OFFSET`],
// `B_QP_OFFSET`), not offsets of this clamp. [`pass2_at`] limits the P pictures before its I rule (x264's I reads the LIMITED P
// qps) and the I pictures after it; an I only against the next I with no P between them nor after that next I (x264's
// `last_accum_p_norm < 1` read as "no P": exact while every P carries no intra macroblock, SR751). A lasting complexity step plans
// a ramp before it, as x264. Departures, named: x264's `tex_bits == 0` P rule is not ported (the records split no texture bits);
// qpstep 0 = no limit (x264's 0 would hold every same-type picture equal). Owed: H264ENC-TWOPASS-QPSTEP-RUNTIME (BUILT by SR769, the
// H264ENCTWOPASSQPSTEPRUNTIME tail block), H264ENC-TWOPASS-QPSTEP-CLI.

/// x264's `--qpstep` default (`i_qp_step` 4): [`Pass2Opts::qpstep`]'s Default.
pub const QP_STEP: u8 = 4;

/// The planned Q16 QPs `q16` of `kind` pictures (H264ENCTWOPASSQPSTEP SR763) each held within `qpstep` QP of the next non-B
/// picture's when that one is of the same `kind`, walking backward (the next one already held); an [`Pic::Idr`] only when no P
/// follows that next I before the non-B after it. B entries and other kinds are untouched; `qpstep` 0 returns `q16` unchanged.
pub fn qpstep_limit(q16: &mut [i64], kinds: &[Pic], qpstep: u8, kind: Pic) {
    if qpstep == 0 {
        return;
    }
    let step = (qpstep as i64) << 16;
    // `next`: the next non-B picture after i; `next_p`: whether the non-B picture after `next` is a P.
    let (mut next, mut next_p): (Option<usize>, bool) = (None, false);
    for i in (0..q16.len()).rev() {
        if kinds[i] == Pic::B {
            continue;
        }
        if let Some(j) = next {
            if kinds[i] == kind && kinds[j] == kind && (kind != Pic::Idr || !next_p) {
                q16[i] = qpstep_bound(q16[i], q16[j], step, (false, false));
            }
        }
        next_p = next.is_some_and(|j| kinds[j] == Pic::P);
        next = Some(i);
    }
}

// ---- H264ENCTWOPASSQPSTEPRUNTIME (LEDGER SR769): x264's lstep clamp on the pass-2 runtime pick (named tail block) ----
// A NAMED DEPARTURE: x264's 2-pass `rate_estimate_qscale` (ratecontrol.c) has no runtime neighbour clamp (its lmin / lmax there are
// qp_min / qp_max; its 2-pass smoothness is init_pass2's get_diff_limited_q alone, our [`qpstep_limit`]). The clamp ported here is
// x264's 1-PASS ABR rule: `lmin = last_qscale_for[type] / lstep; lmax = last_qscale_for[type] * lstep; if (overflow > 1.1 &&
// i_frame > 3) lmax *= lstep; else if (overflow < 0.9) lmin /= lstep;` (asymmetric: "symmetric would prevent overflow control"),
// then `vbv_pass1`, the bucket. `pass2_pick` runs it after the plan + `off` (x264's overflow term) and before its bucket raise,
// which still wins: a bucket overflow is never traded for smoothness. last_qscale_for is `Rc.types[t].qp` (the final pick,
// written by `update`); a P against the last coded P, an I only when the previous non-B was an I (x264's I after a P is its
// I rule, our plan's), a B never (x264's B path returns before the clamp; the plan-time rule skips B too, the plan's B being
// anchor + 2). Departures, named: our overflow is `off`'s sign (off >= 1 ~ overflow >= 1.06 against x264's 1.1, the rounding),
// `p.n > 3` for `i_frame > 3`; x264's frame-0 seeding last_qscale_for[P] = I x ip_factor is not ported (the first P is unclamped).
// qpstep 0 = no clamp (RAW byte-identical). Owed: H264ENC-TWOPASS-QPSTEP-ONE-RULE, H264ENC-TWOPASS-QPSTEP-FIRSTP.

/// `qp` held within `qpstep` QP of `last` (H264ENCTWOPASSQPSTEPRUNTIME SR769, x264's ABR lmin / lmax), the low bound a further
/// `qpstep` lower when `wide.0` (under budget), the high bound a further `qpstep` higher when `wide.1` (over budget); `last`
/// None or `qpstep` 0 returns `qp` unchanged.
pub fn qpstep_clamp(qp: i32, last: Option<i32>, qpstep: u8, wide: (bool, bool)) -> i32 {
    let Some(l) = last.filter(|_| qpstep > 0) else { return qp };
    let s = qpstep as i32;
    (qpstep_bound((qp as i64) << 16, (l as i64) << 16, (s as i64) << 16, wide) >> 16) as i32
}

impl Rc {
    /// Under a plan, the last coded QP the runtime clamp holds picture type `t` against (H264ENCTWOPASSQPSTEPRUNTIME SR769): a P's
    /// last coded P; an I's last coded I only when the previous non-B picture of the plan was an I; a B none.
    fn pass2_last(&self, t: usize) -> Option<i32> {
        let p = self.pass2.as_ref()?;
        let after_i = p.kinds[..p.n].iter().rev().find(|&&k| k != Pic::B) == Some(&Pic::Idr);
        let m = self.types[t];
        (m.coded && (t == 1 || (t == 0 && after_i))).then_some(m.qp)
    }
}

// ---- H264ENCTWOPASSQPSTEPFIRSTP (LEDGER SR804): the frame-0 seed of the runtime clamp (named tail block) ----
// SR769's owed H264ENC-TWOPASS-QPSTEP-FIRSTP. x264's `ratecontrol_new` seeds `last_qscale_for[type]` at init (recalled:
// qp2qscale(ABR_INIT_QP), 24 in 1-pass ABR), so its lstep clamp holds the stream's FIRST P too, against the seed. Here the first P
// has no last coded P (`Rc::pass2_last` None: `types[1].coded` false), so `pass2_pick` holds it against [`qpstep_seed`]: the last
// coded picture of any type (`Rc.last_qp`, its kind the plan's previous entry) moved to the P level, an I by [`IP_QP_OFFSET`]
// (the plan's I rule read backward), a B by -`B_QP_OFFSET`. Only the P is seeded, the one x264 seed reachable under our rules: a B
// is never clamped (x264's B path returns before lmin / lmax) and an I after a P keeps the plan's I rule (SR769). Departure,
// named: the seed is the coded neighbour, not x264's constant 24 (a constant would drag a QP-40 planned stream's first P toward
// 24). qpstep 0 = no clamp (qpstep_clamp's own rule). `types` is never reset, so a P after a scene-cut I is held against the last
// P before the cut. Owed: H264ENC-TWOPASS-QPSTEP-SCENECUT.

/// The QP the runtime clamp holds the first `want` picture against when its type has no last coded picture (H264ENCTWOPASSQPSTEPFIRSTP
/// SR804): `last_any`'s QP moved to the P level (an I + [`IP_QP_OFFSET`], a P unchanged, a B - `B_QP_OFFSET`); None when
/// `last_any` is None or `want` is not [`Pic::P`].
pub fn qpstep_seed(last_any: Option<(Pic, i32)>, want: Pic) -> Option<i32> {
    let (kind, qp) = last_any.filter(|_| want == Pic::P)?;
    Some(match kind {
        Pic::Idr => qp + IP_QP_OFFSET,
        Pic::P => qp,
        Pic::B => qp - B_QP_OFFSET,
    })
}

// ---- H264ENCSCENECUTRCRESET (LEDGER SR834): the rate model across a detected scene cut, measured (named tail block) ----
// x264 resets nothing at a scenecut IDR beyond its I picture; here the P model (k = bits x Qstep / S' of the last P) is
// complexity-normalised, so the first P after a cut is predicted from the OLD scene's k times the NEW picture's S'. This module
// records, per picture around a cut, the model's prediction against the coded bits (fixtures A flat -> texture, B static ->
// motion, C motion -> static; controls: scenecut None with a GOP-aligned scheduled IDR at the cut, and a scheduled IDR inside a
// steady scene) and pins the first P's numbers at rich (200 / 400 kbps) and budget-binding (20-100 kbps) rates with an ORACLE QP
// column (the QP `picture`'s window picks when it knows the picture's true k). MEASURED-NOT-NEEDED: the mispredict is the same at a
// scheduled IDR in the same place (a detected-only reset is the wrong rule), the binding QP error is PIC_STEP-window-bound (A 20 kbps:
// the oracle also picks 22 = the stale P QP + PIC_STEP, so no k reseed fixes it) and the first P's k over the I's k spans
// 0.13..3.18 (no ratio to seed from), so nothing in the rate model changes here. Owed: H264ENC-RC-FIRSTP-IPOFFSET (the first P after
// any IDR windowed on the I's QP + IP_QP_OFFSET, prediction kept: every rate-controlled pin moves), H264ENC-RC-CBR-IDR-OVERFLOW.

#[cfg(test)]
mod scenecut_rc_tests {
    extern crate std;
    use super::*;
    use crate::{Config, Yuv420};
    use alloc::vec;
    use alloc::vec::Vec;
    use std::println;

    const W: usize = 64;
    const H: usize = 64;
    pub(super) const CUT: usize = 10;

    /// A fixed LCG texture (luma 60..=140) of width 2W, seeded `seed`, windowed at column offset `off` (wrapping): `off` moving
    /// with t is a scene panning left.
    fn pattern(seed: u64, off: usize) -> [Vec<u8>; 3] {
        let mut s = seed;
        let big: Vec<u8> = (0..2 * W * H)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (60 + (s >> 33) % 81) as u8
            })
            .collect();
        let mut y = vec![0u8; W * H];
        for r in 0..H {
            for c in 0..W {
                y[r * W + c] = big[r * 2 * W + (c + off) % (2 * W)];
            }
        }
        [y, vec![128; W * H / 4], vec![128; W * H / 4]]
    }

    fn flat(v: u8) -> [Vec<u8>; 3] {
        [vec![v; W * H], vec![128; W * H / 4], vec![128; W * H / 4]]
    }

    /// One coded picture: (key, type index, S', the model's prediction at the picked QP (0 when its type was uncoded), the
    /// coded bits, the QP, the target, the CBR bucket fullness after it, the ORACLE QP: the one `picture`'s window
    /// ([refq - PIC_STEP, refq + PIC_STEP] in [floor, QP_MAX]) picks when the model knows the picture's true k, i.e. its coded
    /// bits moved by Qstep (bits x step16(qp) / step16(q)); the picked QP when its type was uncoded).
    type Row = (bool, usize, u64, u64, u64, i32, i64, i64, i32);

    /// Encode `n` frames of `src` at 64x64 CBR `kbps` at 30 fps, `gop`, `scenecut`: every picture's [`Row`], and the bucket's size.
    fn run(kbps: u32, gop: u32, sc: Option<u8>, n: usize, src: &dyn Fn(usize) -> [Vec<u8>; 3]) -> (Vec<Row>, i64) {
        let cfg = Config { width: W as u32, height: H as u32, qp: 26, gop, scenecut: sc, rate: Some(RateControl::cbr(kbps, (30, 1))), ..Default::default() };
        let mut enc = Encoder::new(cfg).unwrap();
        let mut rows = Vec::new();
        for t in 0..n {
            let p = src(t);
            let before = enc.rc.clone().unwrap();
            let au = enc.encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap();
            let rc = enc.rc.as_ref().unwrap();
            let m = before.types[rc.t];
            let pred = if m.coded { m.predict(rc.s, rc.pic_qp) as u64 } else { 0 };
            let bits = rc.types[rc.t].bits;
            let oracle = if m.coded {
                let rq = if rc.t == 1 && before.after_idr { (before.types[0].qp + IP_QP_OFFSET).min(QP_MAX) } else { m.qp }; // SR844: the window `picture` uses
                let (lo, hi) = ((rq - PIC_STEP).max(before.floor), (rq + PIC_STEP).min(QP_MAX));
                let at = |q: i32| bits as u128 * step16(rc.pic_qp) as u128 / step16(q) as u128;
                (lo..=hi).find(|&q| at(q) <= rc.target as u128).unwrap_or(hi)
            } else {
                rc.pic_qp
            };
            rows.push((au.keyframe, rc.t, rc.s, pred, bits, rc.pic_qp, rc.target, rc.full, oracle));
        }
        (rows, enc.rc.as_ref().unwrap().buf)
    }

    /// Print pictures cut-2..=cut+6 and return the first P after the cut's actual / predicted ratio x 100 (0: unpredicted).
    fn table(name: &str, rows: &[Row]) -> u64 {
        println!("== {name}");
        for (t, r) in rows.iter().enumerate().take(CUT + 7).skip(CUT - 2) {
            println!(
                "t={t:2} kind={} key={} S'={:6} pred={:6} actual={:6} qp={:2} oracle_qp={:2} target={:6} full={:6}",
                ["I", "P", "B"][r.1], r.0, r.2, r.3, r.4, r.5, r.8, r.6, r.7
            );
        }
        let p = &rows[CUT + 1];
        let ratio = if p.3 > 0 { p.4 * 100 / p.3 } else { 0 };
        println!(
            "   first P after the cut: actual / pred = {}.{:02}; qp {} vs oracle {}; actual / target = {}/{}",
            ratio / 100,
            ratio % 100,
            p.5,
            p.8,
            p.4,
            p.6
        );
        ratio
    }

    pub(super) fn fixture(f: char) -> impl Fn(usize) -> [Vec<u8>; 3] {
        move |t| match f {
            'A' => if t < CUT { flat(100) } else { pattern(0x2545_f491_4f6c_dd1d, 0) },
            'B' => if t < CUT { pattern(1, 0) } else { pattern(2, 3 * t) },
            'C' => if t < CUT { pattern(1, 3 * t) } else { pattern(2, 0) },
            _ => pattern(2, 3 * t),
        }
    }

    /// The measured first P after the IDR at t = 10, per run: (fixture, kbps, scenecut Some(40) at gop 30 (true) or the control
    /// scenecut None at gop 10 (the scheduled IDR at the same picture), [pred, actual, qp, oracle qp, target], peak bucket fullness).
    pub(super) const PINS: [(char, u32, bool, [i64; 5], i64); 21] = [
        ('A', 200, true, [256, 2936, 25, 25, 19666], 93766), /* was [705, 9856, 16, 16, 19666], 93766 (SR844) */
        ('A', 200, false, [245, 2840, 29, 29, 14738], 93766), /* was [1104, 13248, 16, 16, 14737], 93766 (SR844) */
        ('A', 40, true, [273, 1928, 29, 29, 2829], 22875), /* was [1229, 12376, 16, 22, 2828], 28404 (SR844) */
        ('A', 40, false, [296, 1968, 29, 30, 1849], 19099), /* was [1332, 12880, 16, 22, 1847], 28228 (SR844) */
        ('A', 20, true, [311, 1040, 29, 32, 800], 14654), /* was [700, 7736, 22, 22, 799], 20664 (SR844) */
        ('A', 20, false, [375, 2032, 29, 41, 468], 14048), /* was [844, 9584, 22, 22, 466], 20912 (SR844) */
        ('B', 400, true, [814, 3392, 17, 17, 37262], 202811), /* was [918, 3664, 16, 16, 37301], 202811 (SR844) */
        ('B', 400, false, [800, 3392, 17, 17, 26612], 202811), /* was [918, 3664, 16, 16, 26616], 202811 (SR844) */
        ('B', 100, true, [4349, 3552, 17, 17, 4421], 64844), /* was [3964, 1536, 22, 16, 4334], 65321 (SR844) */
        ('B', 100, false, [4806, 792, 32, 22, 2910], 61043), /* was [8833, 1248, 26, 20, 2832], 61043 (SR844) */
        ('B', 50, true, [2659, 600, 34, 30, 992], 41642), /* was [783, 1672, 23, 28, 1058], 41528 (SR844) */
        ('B', 50, false, [321, 1112, 28, 38, 344], 36578), /* was [1765, 552, 32, 32, 264], 37218 (SR844) */
        ('C', 400, true, [99, 1648, 17, 17, 35468], 202811), /* was [114, 2016, 16, 16, 35412], 202811 (SR844) */
        ('C', 400, false, [98, 1648, 17, 17, 24780], 202811), /* was [112, 2016, 16, 16, 24762], 202811 (SR844) */
        ('C', 100, true, [235, 2672, 18, 18, 3693], 67561), /* was [285, 4016, 16, 17, 3704], 68740 (SR844) */
        ('C', 100, false, [280, 2480, 22, 22, 2714], 61043), /* was [333, 3664, 20, 23, 2740], 61043 (SR844) */
        ('C', 50, true, [397, 608, 27, 29, 530], 39090), /* was [389, 608, 27, 29, 521], 39322 (SR844) */
        ('C', 50, false, [641, 1016, 30, 34, 641], 36534), /* was [305, 352, 32, 32, 471], 36534 (SR844) */
        ('D', 400, false, [1709, 3392, 17, 17, 24721], 203139), /* was [1819, 3664, 16, 16, 24694], 203139 (SR844) */
        ('D', 100, false, [2217, 2952, 22, 23, 2753], 61419), /* was [2600, 3576, 21, 24, 2736], 61419 (SR844) */
        ('D', 50, false, [638, 384, 39, 35, 661], 36750), /* was [465, 376, 38, 36, 495], 36750 (SR844) */
    ];

    /// The measured table (MEASURE SR834): the first P after a detected cut mispredicts 3.99x-17.68x at the 200 / 400 kbps rows
    /// and by the same at the scheduled IDR in the same place (an any-IDR effect); once the budget binds its QP sits 1-6 off the
    /// oracle (PIC_STEP-bound), A 40 kbps spends 4.4x its target, and A 20 kbps overflows the bucket (20664 / 20912 > 20000),
    /// pinned as measured, not asserted away (owed H264ENC-RC-CBR-IDR-OVERFLOW). Every other run stays inside its bucket.
    /// SR844 (H264ENCRCFIRSTPIPOFFSET): the first P after any IDR windows on the I QP + IP_QP_OFFSET and the oracle is re-centred
    /// on the same window; every run now holds `peak <= buf` (A 20 kbps 14654 / 14048), the invariant asserted on all 21. The QP /
    /// oracle columns stay MEASURED: the kept prediction's stale k moves the first P up to 10 QP off the oracle inside the window
    /// (B 100 kbps control: pred 4806, actual 792), owed H264ENC-RC-FIRSTP-PREDICTION.
    #[test]
    fn scenecut_rc_first_p_table() {
        let n = CUT + 7;
        for &(f, kbps, cut, want, peak_want) in PINS.iter() {
            let (gop, sc) = if cut { (30, Some(40)) } else { (CUT as u32, None) };
            let (rows, buf) = run(kbps, gop, sc, n, &fixture(f));
            assert!(rows[CUT].0 && rows[CUT].1 == 0, "{f} {kbps} {cut}: picture {CUT} is an IDR (detected, or the control's scheduled one)");
            let peak = rows.iter().map(|r| r.7).max().unwrap();
            table(&alloc::format!("{f} {kbps} kbps, {}", if cut { "scenecut Some(40), gop 30" } else { "control: scenecut None, gop 10" }), &rows);
            let p = &rows[CUT + 1];
            assert_eq!(p.1, 1, "{f} {kbps} {cut}: picture {} is a P", CUT + 1);
            assert_eq!([p.3 as i64, p.4 as i64, p.5 as i64, p.8 as i64, p.6], want, "{f} {kbps} {cut}: the first P after the IDR");
            assert_eq!(peak, peak_want, "{f} {kbps} {cut}: the peak bucket fullness");
            assert!(peak <= buf, "{f} {kbps} {cut}: the CBR bucket overflowed ({peak} > {buf})"); // SR844: was `peak > buf` iff A 20 kbps (SR834's measured overflow)
        }
    }

    // ---- H264ENCRCFIRSTPIPOFFSET (LEDGER SR844): the first P after an IDR windowed on the I QP + IP_QP_OFFSET (named tail block) ----
    /// A scheduled IDR inside a steady scene (fixture D, scenecut None, gop 10): at 100 / 50 kbps the first P after the IDR at 10
    /// picks inside [I QP + IP_QP_OFFSET - PIC_STEP, I QP + IP_QP_OFFSET + PIC_STEP], its (I QP, QP, oracle QP) pinned as measured,
    /// the bucket never overflows; the stream's first P (types[1] uncoded) takes the I QP + IP_QP_OFFSET itself.
    #[test]
    fn firstp_ipoffset_steady_scene_idr() {
        for (kbps, want) in [(100u32, [25i32, 22, 23]), (50, [32, 39, 35])] {
            let (rows, buf) = run(kbps, CUT as u32, None, CUT + 7, &fixture('D'));
            assert!(rows[CUT].0 && rows[CUT].1 == 0 && rows[CUT + 1].1 == 1, "D {kbps}: an IDR at {CUT}, then a P");
            let (iqp, p) = (rows[CUT].5, &rows[CUT + 1]);
            let c = (iqp + IP_QP_OFFSET).min(QP_MAX);
            assert!((c - PIC_STEP..=c + PIC_STEP).contains(&p.5), "D {kbps}: the first P's QP {} outside I QP {iqp} + {IP_QP_OFFSET} +- {PIC_STEP}", p.5);
            assert_eq!([iqp, p.5, p.8], want, "D {kbps}: (I QP, first P QP, oracle QP) measured (SR844)");
            assert_eq!(rows[1].5, (rows[0].5 + IP_QP_OFFSET).min(QP_MAX), "D {kbps}: the stream's first P at the I QP + IP_QP_OFFSET");
            let peak = rows.iter().map(|r| r.7).max().unwrap();
            assert!(peak <= buf, "D {kbps}: the CBR bucket overflowed ({peak} > {buf})");
        }
    }
}

// ---- H264ENCRCFIRSTPPREDICTION (LEDGER SR1002): what predicts the first P after an IDR, measured (named tail block) ----
// SR844 windows the first P after any IDR on the I QP + IP_QP_OFFSET but keeps the stale P model's prediction (k of the previous
// GOP's last P). This module measures, on SR834's 21 runs, what predicts that one P: (a) the stale P model (today), (b) the I's
// bits moved by the QP step (no complexity term), (c) the I's bits per complexity on the P's own S' (= the I model's `predict`),
// (d) k damped from (a) toward (c) by 1/2. T1 is static on today's stream (each candidate's bits at the picked QP vs actual, and the
// QP the window would pick with it against SR844's oracle); T2 re-runs the stream with (c) / (d) injected for that one pick only
// (the P model's `s` rewritten so `predict` equals the candidate; bits / step / qp untouched, so r16 / share / target / refq are
// today's and `update` refits from the coded P as today). x264 (e): 1-pass ABR picks a picture's qscale from the stream-long
// complexity rate factor (wanted_bits_window / cplxr_sum, the I from the P by ip_factor), its per-type predictor (not reset at a
// scenecut) only clips for VBV: there is no per-picture reseed to copy, owed H264ENC-RC-CPLX-WINDOW.

#[cfg(test)]
mod firstp_prediction_tests {
    extern crate std;
    use super::scenecut_rc_tests::{fixture, CUT, PINS};
    use super::*;
    use crate::{Config, Yuv420};
    use alloc::vec::Vec;
    use std::println;

    const W: usize = 64;
    const H: usize = 64;

    /// The candidate injected for the first P after the IDR at CUT (T2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Cand {
        C,
        D,
    }

    /// One coded picture: (type index, QP, bits, CBR fullness after it, oracle QP (the picked QP when its type was uncoded)).
    type Pic1 = (usize, i32, u64, i64, i32);

    /// The `s` that makes `mp.predict` equal candidate `c` (A = mp.bits x mp.step, C = mi.bits x mi.step; k = A / s).
    fn injected_s(c: Cand, mi: Model, mp: Model) -> u64 {
        let (a, cc) = (mp.bits as u128 * mp.step as u128, mi.bits as u128 * mi.step as u128);
        let s = match c {
            Cand::C => a * mi.s as u128 / cc.max(1),
            Cand::D => 2 * a * mp.s as u128 * mi.s as u128 / (a * mi.s as u128 + cc * mp.s as u128).max(1),
        };
        (s as u64).max(1)
    }

    /// Encode `n` frames of `src` at 64x64 CBR `kbps` 30 fps (SR834's `run` shape), `inject` the first P after the IDR at CUT: every
    /// picture's [`Pic1`], the rate controller before and after that first P (before the injection), and the bucket size.
    fn run_with(kbps: u32, gop: u32, sc: Option<u8>, n: usize, src: &dyn Fn(usize) -> [Vec<u8>; 3], inject: Option<Cand>) -> (Vec<Pic1>, Rc, Rc, i64) {
        let cfg = Config { width: W as u32, height: H as u32, qp: 26, gop, scenecut: sc, rate: Some(RateControl::cbr(kbps, (30, 1))), ..Default::default() };
        let mut enc = Encoder::new(cfg).unwrap();
        let mut pics = Vec::new();
        let (mut first_before, mut first_after) = (None, None);
        for t in 0..n {
            let p = src(t);
            let before = enc.rc.clone().unwrap();
            if t == CUT + 1 {
                if let Some(c) = inject {
                    let r = enc.rc.as_mut().unwrap();
                    let (mi, mp) = (r.types[0], r.types[1]);
                    assert!(mi.coded && mp.coded && r.after_idr, "inject: both models coded and after an IDR");
                    r.types[1].s = injected_s(c, mi, mp);
                }
            }
            enc.encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap();
            let rc = enc.rc.as_ref().unwrap();
            let m = before.types[rc.t];
            let bits = rc.types[rc.t].bits;
            let oracle = if m.coded {
                let rq = if rc.t == 1 && before.after_idr { (before.types[0].qp + IP_QP_OFFSET).min(QP_MAX) } else { m.qp };
                let (lo, hi) = ((rq - PIC_STEP).max(before.floor), (rq + PIC_STEP).min(QP_MAX));
                let at = |q: i32| bits as u128 * step16(rc.pic_qp) as u128 / step16(q) as u128;
                (lo..=hi).find(|&q| at(q) <= rc.target as u128).unwrap_or(hi)
            } else {
                rc.pic_qp
            };
            if t == CUT + 1 {
                first_before = Some(before);
                first_after = Some(rc.clone());
            }
            pics.push((rc.t, rc.pic_qp, bits, rc.full, oracle));
        }
        let buf = enc.rc.as_ref().unwrap().buf;
        (pics, first_before.unwrap(), first_after.unwrap(), buf)
    }

    /// T1 for one run: per candidate (a)..(d), [actual x 100 / predicted at the picked QP, the window's pick with it - oracle], and
    /// the oracle QP.
    fn t1(before: &Rc, after: &Rc, oracle: i32) -> [[i64; 2]; 4] {
        let (mi, mp) = (before.types[0], before.types[1]);
        let (s, q, target, actual) = (after.s, after.pic_qp, after.target, after.types[1].bits);
        let pred = |c: usize, q: i32| -> u128 {
            let a = mp.predict(s, q);
            let cc = mi.predict(s, q);
            match c {
                0 => a,
                1 => mi.bits as u128 * mi.step as u128 / step16(q) as u128,
                2 => cc,
                _ => (a + cc) / 2,
            }
        };
        let rq = (mi.qp + IP_QP_OFFSET).min(QP_MAX);
        let (lo, hi) = ((rq - PIC_STEP).max(before.floor), (rq + PIC_STEP).min(QP_MAX));
        let mut out = [[0i64; 2]; 4];
        for (c, o) in out.iter_mut().enumerate() {
            let pick = (lo..=hi).find(|&k| pred(c, k) <= target as u128).unwrap_or(hi);
            *o = [(actual as u128 * 100 / pred(c, q).max(1)) as i64, (pick - oracle) as i64];
        }
        out
    }

    /// The measured table (MEASURE SR1002), in [`PINS`]'s order: (fixture, kbps, cut, T1 [window pick - oracle QP] for (a) (b) (c)
    /// (d), T2 [first P QP, peak fullness] with (c) injected then (d)). Within 2 QP of the oracle on the 14 binding rows (kbps <= 100):
    /// T1 a 7, b 2, c 6, d 7; T2 c 6, d 7, and (d) moves later pictures up to 8 QP further from their oracles (C 50 control): no
    /// candidate reaches the 12 / 14 the rule asked, so nothing in `picture` changes (MEASURED-NOT-NEEDED, SR809 / SR834 precedent).
    const TABLE: [(char, u32, bool, [i64; 4], [i64; 4]); 21] = [
        ('A', 200, true, [0, 1, 0, 0], [25, 93766, 25, 93766]),
        ('A', 200, false, [0, 2, 0, 0], [29, 93766, 29, 93766]),
        ('A', 40, true, [0, 12, 0, 0], [29, 22875, 29, 22875]),
        ('A', 40, false, [-1, 11, 0, -1], [30, 19099, 29, 19099]),
        ('A', 20, true, [-3, 9, 6, 0], [38, 13662, 32, 14410]),
        ('A', 20, false, [-12, 0, 0, -3], [41, 11978, 38, 11978]),
        ('B', 400, true, [0, 0, 0, 0], [17, 202811, 17, 202811]),
        ('B', 400, false, [0, 2, 0, 0], [17, 202811, 17, 202811]),
        ('B', 100, true, [0, 12, 11, 7], [28, 64625, 24, 64625]),
        ('B', 100, false, [10, 10, 10, 10], [32, 61043, 32, 61043]),
        ('B', 50, true, [4, 4, 4, 4], [34, 41642, 34, 41642]),
        ('B', 50, false, [-10, 0, 0, 0], [38, 36578, 38, 36578]),
        ('C', 400, true, [0, 0, 0, 0], [17, 202811, 17, 202811]),
        ('C', 400, false, [0, 3, 0, 0], [17, 202811, 17, 202811]),
        ('C', 100, true, [0, 12, 0, 0], [18, 67561, 18, 67561]),
        ('C', 100, false, [0, 12, 0, 0], [22, 61043, 22, 61043]),
        ('C', 50, true, [-2, 10, 9, 5], [38, 39090, 34, 39090]),
        ('C', 50, false, [-4, 7, 6, 2], [40, 36534, 36, 36534]),
        ('D', 400, false, [0, 3, 0, 0], [17, 203139, 17, 203139]),
        ('D', 100, false, [-1, 11, 11, 7], [34, 61419, 30, 61419]),
        ('D', 50, false, [4, 6, 6, 6], [41, 36750, 41, 36750]),
    ];

    #[test]
    fn firstp_prediction_table() {
        let n = CUT + 7;
        let mut within = [0u32; 2];
        let mut within_t1 = [0u32; 4];
        let mut binding = 0u32;
        for (&(f, kbps, cut, _, _), &(tf, tk, tc, want1, want2)) in PINS.iter().zip(TABLE.iter()) {
            assert_eq!((f, kbps, cut), (tf, tk, tc), "TABLE follows PINS' order");
            let (gop, sc) = if cut { (30, Some(40)) } else { (CUT as u32, None) };
            let (base, before, after, buf) = run_with(kbps, gop, sc, n, &fixture(f), None);
            assert!(before.after_idr && after.t == 1, "{f} {kbps} {cut}: the first P after the IDR at {CUT}");
            let oracle = base[CUT + 1].4;
            let e = t1(&before, &after, oracle);
            assert_eq!(e[0][1] as i32 + oracle, base[CUT + 1].1, "{f} {kbps} {cut}: (a) is today's pick");
            let bind = kbps <= 100;
            binding += bind as u32;
            assert_eq!([e[0][1], e[1][1], e[2][1], e[3][1]], want1, "{f} {kbps} {cut}: T1 window pick - oracle for (a)..(d)");
            for (w, x) in within_t1.iter_mut().zip(e.iter()) {
                *w += (bind && x[1].abs() <= 2) as u32;
            }
            println!(
                "T1 {f} {kbps:3} {}: qp={:2} oracle={:2} | a {:4}% {:+3} | b {:4}% {:+3} | c {:4}% {:+3} | d {:4}% {:+3}",
                if cut { "cut " } else { "ctrl" },
                base[CUT + 1].1,
                oracle,
                e[0][0], e[0][1], e[1][0], e[1][1], e[2][0], e[2][1], e[3][0], e[3][1]
            );
            for (i, c) in [Cand::C, Cand::D].into_iter().enumerate() {
                let (pics, _, _, _) = run_with(kbps, gop, sc, n, &fixture(f), Some(c));
                let peak = pics.iter().map(|r| r.3).max().unwrap();
                let p = pics[CUT + 1];
                assert_eq!([p.1 as i64, peak], [want2[2 * i], want2[2 * i + 1]], "{f} {kbps} {cut} {c:?}: T2 first P QP and peak");
                assert!(peak <= buf, "{f} {kbps} {cut} {c:?}: the CBR bucket overflowed ({peak} > {buf})");
                let d1 = (p.1 - oracle).abs();
                if bind && d1 <= 2 {
                    within[i] += 1;
                }
                // a later picture further from its own oracle than in the baseline: the largest growth of |qp - oracle|.
                let grow = (CUT + 2..n).map(|t| (pics[t].1 - pics[t].4).abs() - (base[t].1 - base[t].4).abs()).max().unwrap();
                let moved = (CUT + 2..n).map(|t| (pics[t].1 - base[t].1).abs()).max().unwrap();
                println!(
                    "T2 {f} {kbps:3} {} {c:?}: firstP qp={:2} bits={:5} |qp-oracle|={} peak={:6}/{} second P qp {}->{} bits {}->{} later maxdq={} grow={}",
                    if cut { "cut " } else { "ctrl" },
                    p.1, p.2, d1, peak, buf, base[CUT + 2].1, pics[CUT + 2].1, base[CUT + 2].2, pics[CUT + 2].2, moved, grow
                );
            }
        }
        println!("binding rows {binding}: within 2 QP of the oracle: c {} d {}", within[0], within[1]);
        assert_eq!((binding, within_t1, within), (14, [7, 2, 6, 7], [6, 7]), "binding rows; within 2 QP: T1 (a)..(d), T2 (c) (d)");
    }
}

// ---- H264ENCRCFIRSTBOFFSET (LEDGER SR1010): the first B after an IDR, measured (named tail block) ----
// With bframes 2 the display order I B B P after an IDR is coded I P B B (`group`: the P first, then the pyramid's Ref B, then its
// Outer B), so at the first B `last_qp` is the first P's QP and SR844's `after_idr` is already cleared by that P. From the second GOP
// on the B model is coded (the previous GOP's last B), so `picture`'s refq is that STALE B QP and its k; at the stream's first B the
// model is uncoded and refq is `last_qp + B_QP_OFFSET`, the anchor + offset. This module drives SR834's fixtures through `push` /
// `flush` and REPLAYS `Rc::picture` / `Rc::update` on a clone of the rate controller per push (I / P at their coded QP and bits, each
// B on its true S': `rc_analyse`'s B branch over the reconstructions of its two references), asserting the replay faithful (each B's
// QP == the coded one; the post-push bucket and B model == the encoder's). T2 re-runs with `types[2].qp` set to the first P's QP +
// B_QP_OFFSET before the group's push: exactly the candidate arm (types[2].qp is read only by refq's coded arm).
// MEASURED (SR1010): the arm fixes 2 of the 6 binding misses and worsens 5 of 12 rows, so `picture` is unchanged (the PINS doc).

#[cfg(test)]
mod firstb_offset_tests {
    extern crate std;
    use super::scenecut_rc_tests::{fixture, CUT};
    use super::*;
    use crate::{Config, Yuv420};
    use alloc::vec::Vec;
    use std::println;

    const W: usize = 64;
    const H: usize = 64;

    /// One coded picture in coding order: type index, display index, reference (nal_ref_idc > 0), QP, bits, luma reconstruction.
    struct Coded {
        t: usize,
        disp: u64,
        refd: bool,
        qp: i32,
        bits: u64,
        rec: Vec<u8>,
    }

    /// What the replay saw at one B: after_idr, the B model before it, last_qp, refq, S', target, predicted bits at the pick, the pick,
    /// the coded bits, the floor.
    #[derive(Debug, Clone, Copy)]
    struct BRow {
        after_idr: bool,
        m: Model,
        last: i32,
        anchor: i32,
        refq: i32,
        s: u64,
        target: i64,
        pred: u64,
        qp: i32,
        bits: u64,
        floor: i32,
    }

    /// SR844's oracle over [lo, hi]: the first QP whose bits (the coded bits moved by Qstep) fit the target, else hi.
    fn oracle(b: &BRow, lo: i32, hi: i32) -> i32 {
        let at = |q: i32| b.bits as u128 * step16(b.qp) as u128 / step16(q) as u128;
        (lo.max(b.floor)..=hi.min(QP_MAX)).find(|&q| at(q) <= b.target as u128).unwrap_or(hi.min(QP_MAX))
    }

    /// One run's record: every coded picture, each B's [`BRow`] by coding index, the push index that coded each picture, the peak
    /// CBR fullness (after every picture, from the faithful replay) and the bucket size.
    struct Run {
        coded: Vec<Coded>,
        brows: Vec<(usize, BRow)>,
        push_of: Vec<usize>,
        peak: i64,
        buf: i64,
    }

    /// Push `n` frames of `src` (then flush) at 64x64 CBR `kbps` 30 fps, bframes 2, `gop`, `scenecut`; `inject` = (push index, QP)
    /// writes `types[2].qp` before that push.
    fn run_b(kbps: u32, gop: u32, sc: Option<u8>, n: usize, src: &dyn Fn(usize) -> [Vec<u8>; 3], inject: Option<(usize, i32)>) -> Run {
        let cfg = Config { width: W as u32, height: H as u32, qp: 26, gop, bframes: 2, scenecut: sc, rate: Some(RateControl::cbr(kbps, (30, 1))), ..Default::default() };
        let mut enc = Encoder::new(cfg).unwrap();
        let mut run = Run { coded: Vec::new(), brows: Vec::new(), push_of: Vec::new(), peak: 0, buf: enc.rc.as_ref().unwrap().buf };
        for step in 0..=n {
            if let Some((at, q)) = inject {
                if at == step {
                    enc.rc.as_mut().unwrap().types[2].qp = q;
                }
            }
            let before = enc.rc.clone().unwrap();
            let k0 = enc.stats.len();
            let out = if step < n {
                let p = src(step);
                enc.push(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap()
            } else {
                enc.flush().unwrap()
            };
            assert_eq!(out.len(), enc.reconstructions().len());
            assert_eq!(enc.stats.len() - k0, out.len());
            let first = run.coded.len();
            for (i, au) in out.iter().enumerate() {
                let st = enc.stats[k0 + i];
                let t = match st.kind {
                    Pic::Idr => 0,
                    Pic::P => 1,
                    Pic::B => 2,
                };
                let refd = au.nals.iter().any(|x| matches!(x[0] & 0x1f, 1 | 5) && (x[0] >> 5) & 3 != 0);
                run.coded.push(Coded { t, disp: au.pts - 1, refd, qp: st.qp, bits: st.bits, rec: enc.reconstructions()[i][0].clone() });
                run.push_of.push(step);
            }
            // The replay of this push on a clone of the rate controller.
            let mut rp = before;
            let mut saw_b = false;
            for j in first..run.coded.len() {
                let c = &run.coded[j];
                if c.t != 2 {
                    rp.picture(c.t, alloc::vec![1]);
                    rp.pic_qp = c.qp;
                    rp.update(c.bits);
                    run.peak = run.peak.max(rp.full);
                    continue;
                }
                saw_b = true;
                let past = run.coded[..j].iter().filter(|r| r.refd && r.disp < c.disp).max_by_key(|r| r.disp).expect("a past reference");
                let fut = run.coded[..j].iter().filter(|r| r.refd && r.disp > c.disp).min_by_key(|r| r.disp).expect("a future reference");
                let frame = src(c.disp as usize);
                let y = &frame[0];
                let s = 64 * (W / 16 * H / 16) as u64
                    + (0..W * H).map(|i| (y[i] as i32 - past.rec[i] as i32).unsigned_abs().min((y[i] as i32 - fut.rec[i] as i32).unsigned_abs()) as u64).sum::<u64>();
                let m = rp.types[2];
                let (after_idr, last, anchor) = (rp.after_idr, rp.last_qp, rp.types[1].qp);
                let refq = if m.coded { m.qp } else { (last + B_QP_OFFSET).min(QP_MAX) };
                let qp = rp.picture(2, alloc::vec![s]);
                assert_eq!(qp, c.qp, "replay: the B at display {} picked {qp}, the encoder {}", c.disp, c.qp);
                let pred = if m.coded { m.predict(s, qp) as u64 } else { 0 };
                run.brows.push((j, BRow { after_idr, m, last, anchor, refq, s, target: rp.target, pred, qp, bits: c.bits, floor: rp.floor }));
                rp.update(c.bits);
                run.peak = run.peak.max(rp.full);
            }
            let rc = enc.rc.as_ref().unwrap();
            assert_eq!((rp.full, rp.plan, rp.last_qp, rp.err), (rc.full, rc.plan, rc.last_qp, rc.err), "replay: the bucket after push {step}");
            if saw_b {
                let (a, b) = (rp.types[2], rc.types[2]);
                assert_eq!((a.coded, a.bits, a.step, a.s, a.qp, a.avg_s), (b.coded, b.bits, b.step, b.s, b.qp, b.avg_s), "replay: the B model after push {step}");
            }
        }
        run
    }

    /// The coding indices of the IDR at display CUT and of the first two Bs coded after it.
    fn after_cut(run: &Run) -> (usize, usize, usize) {
        let i = run.coded.iter().position(|c| c.t == 0 && c.disp == CUT as u64).expect("an IDR at CUT");
        let mut bs = run.brows.iter().map(|b| b.0).filter(|&j| j > i);
        (i, bs.next().expect("a first B after the IDR"), bs.next().expect("a second B after the IDR"))
    }

    fn brow(run: &Run, j: usize) -> BRow {
        run.brows.iter().find(|b| b.0 == j).unwrap().1
    }

    /// The measured table (MEASURE SR1010), per run: (fixture, kbps, scenecut Some(40) at gop 30 (true) or the control scenecut None
    /// at gop 10, B1 = the first B coded after the IDR at CUT [refq (the stale B QP), anchor (the first P's QP), QP, oracle_full,
    /// T2 QP (the arm `types[1].qp + B_QP_OFFSET` injected), T2 oracle_full], B2 [QP, oracle_full], peak fullness). On the 12 binding
    /// rows (kbps <= 100) B1 sits > PIC_STEP off oracle_full on 6 (A 50, C 100, C 50); the arm brings 2 within PIC_STEP (A 50), grows
    /// |B1 - oracle| on 5 (B 100 cut 3 -> 16) and a later B by 7 (B 100 cut): MEASURED-NOT-NEEDED, `picture` unchanged; the misses
    /// sit in the stale B prediction (pred / actual 0.13x..7.3x), owed with H264ENC-RC-CPLX-WINDOW.
    const PINS: [(char, u32, bool, [i32; 6], [i32; 2], i64); 18] = [
        ('A', 200, true, [16, 25, 16, 16, 21, 16], [16, 16], 93790),
        ('A', 200, false, [16, 29, 16, 16, 25, 16], [16, 16], 93782),
        ('A', 100, true, [16, 29, 16, 19, 25, 20], [16, 16], 47123),
        ('A', 100, false, [16, 29, 16, 20, 25, 22], [16, 16], 47115),
        ('A', 50, true, [16, 29, 16, 26, 25, 26], [21, 16], 32032),
        ('A', 50, false, [16, 29, 16, 30, 25, 28], [22, 16], 28798),
        ('B', 200, true, [16, 29, 16, 16, 25, 16], [16, 16], 109502),
        ('B', 200, false, [16, 29, 16, 16, 25, 16], [16, 16], 108998),
        ('B', 100, true, [17, 31, 23, 20, 36, 20], [20, 22], 77840),
        ('B', 100, false, [20, 32, 26, 20, 33, 19], [20, 22], 67064),
        ('B', 50, true, [25, 35, 31, 34, 43, 38], [33, 34], 45696),
        ('B', 50, false, [25, 37, 31, 35, 42, 37], [34, 35], 39480),
        ('C', 200, true, [22, 29, 16, 26, 25, 26], [21, 16], 118574),
        ('C', 200, false, [23, 29, 17, 28, 25, 26], [23, 16], 114406),
        ('C', 100, true, [32, 36, 36, 19, 36, 19], [30, 41], 72945),
        ('C', 100, false, [35, 33, 37, 20, 37, 20], [31, 38], 69214),
        ('C', 50, true, [43, 41, 37, 29, 37, 29], [31, 47], 47506),
        ('C', 50, false, [43, 41, 37, 28, 37, 28], [31, 47], 46834),
    ];

    #[test]
    fn firstb_offset_table() {
        let n = CUT + 7;
        let (mut binding, mut miss_today, mut fixed, mut grew, mut later_bad) = (0u32, 0u32, 0u32, 0u32, 0u32);
        for &(f, kbps, cut, want1, want2, peak_want) in PINS.iter() {
            let (gop, sc) = if cut { (30, Some(40)) } else { (CUT as u32, None) };
            let name = alloc::format!("{f} {kbps:3} {}", if cut { "cut " } else { "ctrl" });
            let base = run_b(kbps, gop, sc, n, &fixture(f), None);
            assert!(base.peak <= base.buf, "{name}: the CBR bucket overflowed ({} > {})", base.peak, base.buf);
            assert_eq!(base.peak, peak_want, "{name}: the peak bucket fullness");
            // The stream's first B: the uncoded road, the first P's QP + B_QP_OFFSET.
            let (j0, s0) = base.brows[0];
            println!("FB0 {name}: disp={} coded_model={} last={} refq={} qp={}", base.coded[j0].disp, s0.m.coded, s0.last, s0.refq, s0.qp);
            assert!(!s0.m.coded && s0.qp == (s0.anchor + B_QP_OFFSET).min(QP_MAX), "{name}: the stream's first B at the anchor + B_QP_OFFSET");
            let (iidr, j1, j2) = after_cut(&base);
            let order: Vec<(usize, u64)> = base.coded[iidr..=j2].iter().map(|c| (c.t, c.disp)).collect();
            assert_eq!(order, [(0, CUT as u64), (1, CUT as u64 + 3), (2, CUT as u64 + 1), (2, CUT as u64 + 2)], "{name}: I B B P coded I P B B");
            let (b1, b2) = (brow(&base, j1), brow(&base, j2));
            assert!(!b1.after_idr && b1.m.coded && b1.refq == b1.m.qp && b1.last == b1.anchor, "{name}: B1 on the stale road, after_idr cleared by the P");
            let (of1, of2) = (oracle(&b1, b1.floor, QP_MAX), oracle(&b2, b2.floor, QP_MAX));
            println!(
                "FB1 {name}: refq={} anchor={} qp={} oracle_win={} oracle_full={of1} pred={} actual={} target={} S'={} | FB2 qp={} oracle_full={of2}",
                b1.refq, b1.anchor, b1.qp, oracle(&b1, b1.refq - PIC_STEP, b1.refq + PIC_STEP), b1.pred, b1.bits, b1.target, b1.s, b2.qp
            );
            // T2: the candidate arm (types[2].qp = the first P's QP + B_QP_OFFSET before the group's push).
            let alt = (b1.anchor + B_QP_OFFSET).min(QP_MAX);
            let inj = run_b(kbps, gop, sc, n, &fixture(f), Some((base.push_of[j1], alt)));
            assert!(inj.peak <= inj.buf, "{name}: T2 overflowed ({} > {})", inj.peak, inj.buf);
            let (_, k1, _) = after_cut(&inj);
            let c1 = brow(&inj, k1);
            assert_eq!((c1.anchor, c1.refq), (b1.anchor, alt), "{name}: T2 injected the arm");
            let oc1 = oracle(&c1, c1.floor, QP_MAX);
            println!("T2 {name}: B1 qp {}->{} oracle_full {of1}->{oc1} peak {}->{} / {}", b1.qp, c1.qp, base.peak, inj.peak, inj.buf);
            assert_eq!(([b1.refq, b1.anchor, b1.qp, of1, c1.qp, oc1], [b2.qp, of2]), (want1, want2), "{name}: B1 / T2 / B2 measured");
            if kbps <= 100 {
                let (d0, d1) = ((b1.qp - of1).abs(), (c1.qp - oc1).abs());
                // later Bs (coding order after B1): the largest growth of |qp - oracle_full| under the arm.
                let mut grow = 0;
                for (&(jb, bb), &(jc, bc)) in base.brows.iter().zip(inj.brows.iter()) {
                    if jb > j1 && jc == jb {
                        grow = grow.max((bc.qp - oracle(&bc, bc.floor, QP_MAX)).abs() - (bb.qp - oracle(&bb, bb.floor, QP_MAX)).abs());
                    }
                }
                binding += 1;
                miss_today += (d0 > PIC_STEP) as u32;
                fixed += (d0 > PIC_STEP && d1 <= PIC_STEP) as u32;
                grew += (d1 > d0) as u32;
                later_bad += (grow > PIC_STEP) as u32;
            }
        }
        println!("binding rows {binding}: B1 > PIC_STEP off oracle_full today {miss_today}; the arm brings within PIC_STEP {fixed}; grows |B1 - oracle| {grew}; a later B grows > PIC_STEP {later_bad}");
        assert_eq!((binding, miss_today, fixed, grew, later_bad), (12, 6, 2, 5, 1), "the measured refutation of the first-B arm (SR1010)");
    }
}

// ---- H264ENCTWOPASSQPSTEPONERULE (LEDGER SR1026): x264's get_diff_limited_q bound as one Q16 core (named tail block) ----
// SR769's owed H264ENC-TWOPASS-QPSTEP-ONE-RULE. x264 holds a qscale within lstep = 2^(qpstep / 6) of its same-type neighbour in two
// places: init_pass2's get_diff_limited_q over the plan ([`qpstep_limit`], SR763) and the ABR lmin / lmax at runtime
// ([`qpstep_clamp`], SR769, its asymmetric widening on under / over-spend). Both bodies now call this ONE bound in the Q16 QP (log)
// domain: the plan with wide (false, false) over its Q16 slice, the runtime with its integer QPs shifted to Q16 and back (exact:
// every bound is then a multiple of 65536). The wrappers keep their signatures, callers and qpstep-0 / no-last guards; the plan's
// QPs and the coded bytes are unmoved (tests/twopass.rs zb-zc). Owed: H264ENC-TWOPASS-QPSTEP-DOC.

/// `q` (Q16 QP) held within `step_q16` of `last` (H264ENCTWOPASSQPSTEPONERULE SR1026), the low bound a further `step_q16` lower
/// when `wide.0`, the high bound a further `step_q16` higher when `wide.1`. Equal to `clamp` for `step_q16 >= 0` (what both callers
/// pass); a negative step never panics (max then min).
pub fn qpstep_bound(q: i64, last: i64, step_q16: i64, wide: (bool, bool)) -> i64 {
    let lo = last - step_q16 - if wide.0 { step_q16 } else { 0 };
    let hi = last + step_q16 + if wide.1 { step_q16 } else { 0 };
    q.max(lo).min(hi)
}

// ---- H264ENCHRDVBRUNDERFLOW (LEDGER SR1050): the VBR row-QP room term (named tail block) ----
// A VBR picture whose bits pass the max bucket's room at its start (`room.0` = bmax - fmax, set in `picture`'s VBR arm) underflows
// the CPB `hrd_au` signals (the bucket's mirror); `update` keeps the fill unclamped so the event stays visible. Before each row
// (`row_qp`, rows > 0) the picture's bits are projected: the bits so far plus the rest at the rate so far by the rows' SAD share
// (`row_qp`'s own share), each QP step of the term re-projecting the rest at the Qstep ratio (x264 `x264_ratecontrol_mb`'s VBV
// loop); the term rises while the projection passes the room less 1/16 (the SPS / PPS / SEI, slice header and NAL bytes `bits`
// does not count), never falls within the picture, at most [`VBV_ROW_STEP`]. CBR / two-pass / CRF: 0. Owed: H264ENC-HRD-VBR-REENCODE.

/// The largest room term of a VBR picture's rows (SR1050).
pub const VBV_ROW_STEP: i32 = 6;

impl Rc {
    /// The VBR room term of row `row` (> 0) after `bits` bits of slice data; 0 under any other mode.
    fn room_step(&mut self, row: usize, bits: u64) -> i32 {
        if !matches!(self.rate.mode, RcMode::Vbr { .. }) {
            return 0;
        }
        let cum: u64 = self.rows[..row].iter().sum();
        let rest = bits as i128 * self.s.saturating_sub(cum) as i128 / cum.max(1) as i128;
        let limit = (self.room.0 - self.room.0 / 16) as i128;
        let at = step16((self.pic_qp + self.room.1).min(QP_MAX)) as i128;
        let mut v = self.room.1;
        while v < VBV_ROW_STEP && bits as i128 + rest * at / step16((self.pic_qp + v).min(QP_MAX)) as i128 > limit {
            v += 1;
        }
        self.room.1 = v;
        v
    }
}

// ---- H264ENCTWOPASSQBLURLINEAR (LEDGER SR1042): qblur over the linear qscale (named tail block) ----
// SR736's owed H264ENC-TWOPASS-QBLUR-LINEAR. x264's `init_pass2` smooth-curve loop averages the LINEAR qscale (qp2qscale(qp) =
// 0.85 x 2^((qp - 12) / 6)) with the gaussian alone as the weight; [`blur_qscale`] now does the same in integers: the 0.85 and the
// 12 are a common factor that cancels in the weighted mean, so each same-kind qscale is taken RELATIVE to the window's highest QP m
// (2^(-(m - q) / 6), in (0, 1], [`qp2qscale_q16`] over the tree's ONE exponential [`exp_neg_q16`]) and the mean goes back to QP
// through the ONE log2 [`log2_q16`] ([`qscale2qp_q16`]). No f64: the crate is no_std and links into the soft-float kernel. A mean
// of 2^(q / 6) is never below the mean of q (Jensen), so the blur now plans each picture at a HIGHER-or-equal QP than SR736's
// (a 30 / 20 / 30 spike at qblur 0.5: the middle 20.35 -> 20.64); a flat window blurs to itself exactly; qblur < 0.25 stays the
// identity. x264's second blur carries no complexity weight (H264ENC-TWOPASS-QBLUR-WEIGHT-DOMAIN measured not needed).

/// round(2^32 ln 2).
const LN2_Q32: u128 = 2977044472;

/// 2^(-d / 6) in Q64 for d = `d_q16` / 65536 (H264ENCTWOPASSQBLURLINEAR SR1042): a linear qscale d QP below the window's highest,
/// relative to it; a negative d is 0. d = 6k + f: the mantissa round(65536 e^-(f ln2 / 6)) in (32768, 65536] from [`exp_neg_q16`],
/// shifted to Q64 and down by k (exact when f = 0); k >= 48 is 0 (below 2^-48 of the highest: no weight).
pub fn qp2qscale_q16(d_q16: i64) -> u128 {
    let d = d_q16.max(0) as u64;
    let six = 6u64 << 16;
    let k = d / six;
    if k >= 48 {
        return 0;
    }
    let f = (d % six) as u128;
    let t = ((f * LN2_Q32 / 6 + (1 << 31)) >> 32) as u64;
    ((exp_neg_q16(t) as u128) << 48) >> k
}

/// 6 log2(`num` / `den`) in Q16 QP (H264ENCTWOPASSQBLURLINEAR SR1042): a qscale ratio back to a QP difference, by [`log2_q16`]
/// of both shifted together until the larger fits a u64.
pub fn qscale2qp_q16(num: u128, den: u128) -> i64 {
    let s = (128 - num.max(den).leading_zeros()).saturating_sub(64);
    6 * (log2_q16((num >> s) as u64) - log2_q16((den >> s) as u64))
}