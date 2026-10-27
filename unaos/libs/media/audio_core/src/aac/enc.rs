// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core: the AAC-LC encoder (ISO/IEC 14496-3 §4), the decoder's tables and transform reused.
//!
//! NLE6EXPORTAAC (rmbp-ledger B960; handlers.md §5 NLE6's NLE6EXPORT-AAC). The editor's export (Stria's `nle export
//! --audio aac`, the kernel's `nle export --aac KBPS`) encodes its sound here; `no_std` + `alloc`, both rings link it.
//!
//! * ONE TRANSFORM: the forward MDCT is the windowed N-sample block folded into N/2 values and put through the
//!   decoder's OWN inverse transform (`crate::vorbis::Imdct::inverse`, the object `AacDecoder` builds): the DCT-IV is
//!   its own inverse, so the forward spectrum is read back from the middle half of that call's output. Scale: X = 2·Σ
//!   w·x·cos in i16 units, the pair of the decoder's 2/N/32768 synthesis (exact reconstruction, `aac_enc_sine_snr`).
//!   N is 2048 (long) or 256 (the eight short windows, AACENCSHORT LEDGER SR375): one fold, `Encoder::mdct`.
//! * ONE TABLE SET: the scalefactor and spectral Huffman codes are `super::tables`' `SF_CODE`/`SF_BITS`/`SPEC_CODES`/
//!   `SPEC_BITS`, the bands `SWB_1024`/`SWB_128`, the windows the decoder's `sine`, |q|^(4/3) its `pow43_table`.
//! * BLOCK SWITCHING (AACENCSHORT, LEDGER SR375): an attack detector (8 sub-blocks of 128 high-passed samples per block,
//!   an attack where one carries `ATTACK_RATIO` times the mean of the 8 before it and at least `ATTACK_MIN`) over a
//!   ONE-FRAME LOOKAHEAD (`encode` holds one input frame: API latency only, the stream's delay is still [`PRIMING`]) drives
//!   LONG → LONG_START → EIGHT_SHORT → LONG_STOP with sine shapes. WINDOW GROUPING (AACENCGROUPING, LEDGER SR378): the
//!   detector's own 8 sub-block energies of the block (sub-block s = short window s's centre, summed over the channels)
//!   join window w to w-1's group unless the larger of the two exceeds `GROUP_RATIO` times the smaller (both floored by
//!   `ATTACK_MIN`: `grouping`); the spectrum is interleaved into the decoder's group → band → window order (`interleave`),
//!   so a group's band is one range sharing one scalefactor and codebook. M/S (AACENCSHORT): a stereo frame is always a
//!   CPE with `common_window` 1; a band goes M/S when its perceptual-entropy estimate is lower that way (`ms_decide`),
//!   ms_mask_present 0/1/2. Not built: PNS, intensity, SBR.
//! * TNS (AACENCTNS, LEDGER SR381; §4.6.9): per channel per window, the spectrum from `TNS_START_HZ` to the decoder's
//!   `TNS_MAX_BANDS` limit is LPC-analysed (Levinson-Durbin, order <= 12 long / 7 short) over its BAND-NORMALISED
//!   autocorrelation (AACENCTNSNORM, LEDGER SR385; 3GPP TS 26.403's detection: each band weighted by 1/sqrt of its mean
//!   energy, the weights smoothed across bands, `tns_weights`, so a tonal frame's strong bands and their leakage do not
//!   read as a predictable spectrum) and, on ANY block where that prediction gain exceeds `TNS_GAIN_NORM` 2.1 (SR381's
//!   attack gate retired to the KAT switch `tns_gate`; the raw autocorrelation and `TNS_GAIN` 1.4 are the `tns_norm`
//!   off arm), its parcor coefficients are quantised (4 / 3 bits), turned into the filter by
//!   the decoder's own `tns_lpc` and the spectrum replaced by the residual before the two loops (`tns_window`); the
//!   decoder's `spectral_tools` synthesis inverts it exactly. The thresholds are taken from the unfiltered spectrum and
//!   divided by the synthesis filter's noise gain. TWO FILTERS AND coef_compress (AACENCTNSMULTI, LEDGER SR390): a long
//!   window whose filter fired may split at the band b* minimising the two ranges' residual energies after their
//!   quantised filters (`tns_window` on scratch copies), filter 0 the top range, filter 1 the one below (`tns_multi`);
//!   that candidate is coded beside the one-filter one and kept when the frame's quantised noise over the mask is
//!   lower (`nmr`, from `band`). A filter whose indices fit one bit fewer is sent coef_compress. Filters stay upward
//!   (AACENC-TNS-DOWN owed: the NMR trial picked downward filters that raised the short-block pre-echo).
//! * THE PSY MODEL (AACENCTNS, LEDGER SR381; `allowed`): the band energies spread by a two-slope Bark function (3GPP TS
//!   26.403: `SPREAD_LO_DB` 30 / `SPREAD_HI_DB` 15 dB per Bark, one `Spread` table per block type built in `new`), under a
//!   tonality offset (`TONAL_DB` 18 for a tonal band, `NOISE_DB` 6 for a flat one). THE TONALITY (AACENCPSYPREDDEFAULT,
//!   LEDGER SR396; SR385's opt-in made the default): a long block's band tonality is the complex unpredictability of
//!   3GPP TS 26.403 / ISO/IEC 13818-7 model 2 over the two previous long blocks (`chaos_tonality`): the spectrum is the
//!   MDCT and its MDST (the sine counterpart, the complex spectrum's imaginary part, `Encoder::mdst`: the ONE transform
//!   on the time-reversed windowed block, MDST_k(z) = -(-1)^k MDCT_k(z reversed), so no second window or transform),
//!   and each line's (r, φ) is predicted as (2r1 - r2, 2φ1 - φ2). A magnitude-only predictor read the 1 kHz tone as noise
//!   (its MDCT magnitudes cycle every 3 blocks: tonality 0.00 / 0.61 / 0.00; the complex one 1.00 on each); an MDST
//!   estimated from the MDCT's neighbour bins was measured and rejected (residual -2.9 dB, the tone 0.21 / 0.09 / 0.00).
//!   A short block, a block without two long predecessors and a band whose history is silent take the spectral
//!   flatness instead (Johnston; the `Psy::Flatness` switch takes it everywhere). A band under the
//!   floor below is not coded; a band the model masks keeps a base scalefactor (never coarser than the last that codes
//!   anything) the inner loop's spare bits reach (the seat's ruling: with the masked bands dropped, the 1 kHz tone fell to
//!   36.7 dB). THE BIT RESERVOIR: a frame borrows from the bits earlier frames left (capped at `AU_MAX_BITS` a channel) and
//!   banks the mean frame less its real bits, so a silent frame funds the next transient and the mean bitrate holds.
//!   AACENCTNSMULTI (LEDGER SR390): the lend follows the frame's perceptual entropy (3GPP TS 26.403 §5.4's Σ n·log2(E/T),
//!   `frame_pe`) against its running mean, on §5.6's save / spend shape over the bank's fill (`res_lend`): a high-PE
//!   frame takes more than the mean (the whole bank at twice the mean PE or after silence), a low-PE frame less.
//! * THE QUANTISER (the two loops): a per-band threshold from the psy model (floored at an
//!   absolute level and at the frame's mask floor, `MASK_DB` under its loudest band per coefficient, so a tone's window
//!   leakage is not coded at the tone's SNR; measured on `aac_enc_sine_snr`'s 1 kHz tone at 128 kb/s stereo, 1356 bits a
//!   channel: MASK_DB 40 -> 40.2 dB at 75 B/frame, 50 -> 49.6 dB at 155 B, 60 (taken) -> 56.8 dB at 247 B; with no floor
//!   14.4 dB, the leakage bands eating the budget) — the OUTER loop takes, per band, the coarsest scalefactor whose measured quantisation noise stays
//!   under it; the INNER loop shifts every band by one global offset, binary-searched so the frame's counted bits fit
//!   the budget (`bitrate · 1024 / rate / channels` a channel; a CPE's two channels share ONE offset against twice that,
//!   AACENCGROUPING SR378, so an M/S frame spends S's leftover on M); a band above the bitrate's cutoff or quantised to
//!   nothing is ZERO_HCB; consecutive scalefactors are held within ±60 by coarsening the finer band.
//! * OUTPUT: one raw_data_block per [`AccessUnit`] (MP4's sample), [`Encoder::adts`] frames one for a `.aac` stream,
//!   [`Encoder::asc`] is the AudioSpecificConfig. The first unit carries the 1024-sample MDCT priming ([`PRIMING`]): an
//!   MP4 writer trims it with an edit list (demux_core `build::mp4a_edit_list`).
use alloc::vec;
use alloc::vec::Vec;

use super::decoder::{pow43_table, sine};
use super::tables as t;
use crate::math;
use crate::vorbis::Imdct;
use crate::{Error, Result};

/// Samples per channel per access unit.
pub const FRAME: usize = 1024;
/// The decoder's output leads the input by one frame (the MDCT overlap): the samples an edit list skips.
pub const PRIMING: u32 = 1024;

/// One encoded raw_data_block and its first sample's position (in samples per channel, the priming included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessUnit {
    pub data: Vec<u8>,
    pub pts: u64,
}

/// The psy model's signal-to-mask offsets (AACENCTNS, LEDGER SR381): a tonal band's (SR378's fixed SNR_DB 18) and a
/// noise-like band's (dB), the spectral flatness that counts as fully tonal (Johnston, dB), and the absolute floor
/// (noise energy per coefficient, i16 units: well under one LSB of white noise, which is ~4096 in this spectrum's scale).
const TONAL_DB: f64 = 18.0;
const NOISE_DB: f64 = 6.0;
const SFM_TONAL_DB: f64 = -60.0;
/// AACENCTNSNORM (LEDGER SR385): `Psy::Pred`'s chaos -> tonality map (ISO/IEC 13818-7 psychoacoustic model 2):
/// tonality = clamp(PRED_A - PRED_B · ln c, 0, 1); AACENCPSYPREDDEFAULT (SR396): c the complex unpredictability, the default.
const PRED_A: f64 = -0.299;
const PRED_B: f64 = 0.43;
const ATH: f64 = 1000.0;
/// The two-slope Bark spreading (3GPP TS 26.403's AAC psy model): dB per Bark toward lower bands and toward higher.
const SPREAD_LO_DB: f64 = 30.0;
const SPREAD_HI_DB: f64 = 15.0;
/// The frame's mask floor: no band's noise need sit lower than this many dB under the loudest band (per coefficient);
/// AACENCTNS keeps it as the floor under the spread threshold.
const MASK_DB: f64 = 60.0;
/// TNS (§4.6.9, AACENCTNS): a filter where the spectrum's prediction gain exceeds TNS_GAIN, of order at most
/// TNS_ORDER_LONG / TNS_ORDER_SHORT, over the bands from TNS_START_HZ to the TNS_MAX_BANDS table's limit.
const TNS_GAIN: f64 = 1.4;
/// AACENCTNSNORM (LEDGER SR385): the gate on the band-normalised prediction gain, every block. Measured on the
/// tone + noise-burst fixture: the stationary tonal blocks peak at 1.9, the burst blocks start at 2.4 (raw 1.7-4.2 vs
/// 2.6+: they overlapped, hence SR381's attack gate); 2.1 is their geometric middle.
const TNS_GAIN_NORM: f64 = 2.1;
const TNS_ORDER_LONG: usize = 12;
const TNS_ORDER_SHORT: usize = 7;
const TNS_START_HZ: f64 = 1500.0;
/// The largest frame a channel may spend (bits, the decoder's input buffer, §4.5.3): the bit reservoir's bound.
const AU_MAX_BITS: usize = 6144;
/// The frame's bits outside the channels' budget: element id + tag (+ common_window), ID_END, the alignment; and room
/// for the largest M/S mask.
const FRAME_OVERHEAD: usize = 3 + 4 + 1 + 3 + 7;
const MS_MASK_MAX: usize = 128;
/// The largest quantised magnitude the escape codebook carries (§4.6.1: 8191).
const MAX_Q: i32 = 8191;
/// window_sequence values (§4.6.11, the decoder's).
const ONLY_LONG: u8 = 0;
const LONG_START: u8 = 1;
const EIGHT_SHORT: u8 = 2;
const LONG_STOP: u8 = 3;
/// The attack detector (AACENCSHORT): a 128-sample sub-block's high-passed energy over the mean of the 8 before it, and
/// its absolute floor (i16² summed over the sub-block: an rms step of ~88 LSB).
const ATTACK_RATIO: f64 = 10.0;
const ATTACK_MIN: f64 = 1.0e6;
/// The grouping rule (AACENCGROUPING): neighbouring short windows share a group while their sub-block energies (each
/// plus `ATTACK_MIN`) lie within this ratio (6 dB).
const GROUP_RATIO: f64 = 4.0;

/// The private MSB-first bit writer (BITWRITER-ONE owed: h264enc_core's lives in a crate this one does not link).
struct Bits {
    buf: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn new() -> Bits { Bits { buf: Vec::new(), acc: 0, n: 0 } }
    fn put(&mut self, n: u32, v: u32) {
        for i in (0..n).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 8 { self.buf.push(self.acc as u8); self.acc = 0; self.n = 0; }
        }
    }
    /// Zero-padded to the byte (the raw_data_block's own alignment).
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 { self.buf.push((self.acc << (8 - self.n)) as u8); }
        self.buf
    }
}

/// The bands a block codes (AACENCSHORT): long, one window of `SWB_1024` bands; EIGHT_SHORT, eight windows of `SWB_128`
/// bands in `ng` groups (AACENCGROUPING). Band `i` is group `i / nb`, band `i % nb` — the decoder's scalefactor order.
#[derive(Clone, Copy)]
struct Grid {
    short: bool,
    /// window groups (long: 1), the windows in each, the 7 scale_factor_grouping bits on the wire
    ng: usize,
    glen: [usize; 8],
    bits: u32,
    swb: &'static [u16],
    /// bands per window below the bitrate's cutoff
    nb: usize,
}

impl Grid {
    fn bands(&self) -> usize { self.ng * self.nb }
    /// Coefficient range of band `i` in the group-interleaved 1024-value block (`interleave`): group g starts at 128 ×
    /// the windows before it, its band b runs `swb[b]·gl .. swb[b+1]·gl` past that (the decoder's g → sfb → window order).
    fn rng(&self, i: usize) -> (usize, usize) {
        let (gi, b) = (i / self.nb, i % self.nb);
        let o = 128 * self.glen[..gi].iter().sum::<usize>();
        let gl = self.glen[gi];
        (o + self.swb[b] as usize * gl, o + self.swb[b + 1] as usize * gl)
    }
}

/// One channel's coded frame.
struct Coded {
    max_sfb: usize,
    global_gain: i32,
    /// per group and band (`g * max_sfb + b`): codebook (0 = ZERO_HCB) and scalefactor
    cb: Vec<u8>,
    sf: Vec<i32>,
    q: Vec<i32>,
    bits: usize,
}

impl Coded {
    /// Re-strided to `m >= max_sfb` bands a window (a CPE's common max_sfb), the new bands ZERO_HCB.
    fn pad(&mut self, m: usize, ng: usize) {
        if m == self.max_sfb { return; }
        let mut cb = vec![0u8; ng * m];
        let mut sf = vec![0i32; ng * m];
        for w in 0..ng {
            for b in 0..self.max_sfb {
                cb[w * m + b] = self.cb[w * self.max_sfb + b];
                sf[w * m + b] = self.sf[w * self.max_sfb + b];
            }
        }
        self.cb = cb;
        self.sf = sf;
        self.max_sfb = m;
    }
}

/// The psy model's tonality source (AACENCTNSNORM, LEDGER SR385): the complex unpredictability of the band's MDCT +
/// MDST over the two previous long blocks (the default since AACENCPSYPREDDEFAULT, LEDGER SR396), or the band's
/// spectral flatness everywhere (the switch).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Psy {
    /// the switch: only the KATs choose it today (no caller sets it)
    #[cfg_attr(not(test), allow(dead_code))]
    Flatness,
    Pred,
}

/// The AAC-LC encoder: 1 or 2 channels at one of the 13 AAC rates, a target bitrate.
pub struct Encoder {
    rate: u32,
    channels: usize,
    sf_index: usize,
    bitrate: u32,
    mdct: Imdct,
    /// the 256-point transform of the short windows (AACENCSHORT; built as the decoder's `imdct_short`)
    mdct_s: Imdct,
    /// the sine window's rising half (1024), and the short one's (128)
    win: Vec<f32>,
    win_s: Vec<f32>,
    pow43: Vec<f32>,
    /// per channel: the previous block (the first half of the next MDCT input)
    hist: Vec<Vec<f32>>,
    /// interleaved input short of a frame
    pend: Vec<i16>,
    /// the frame coded next, held for the one-frame lookahead the block decision needs
    held: Option<Vec<i16>>,
    units: u64,
    /// bands at or above this one are never coded (the bitrate's bandwidth); the short windows' own count
    max_band: usize,
    max_band_s: usize,
    budget: usize,
    flushed: bool,
    /// the block coded next is EIGHT_SHORT (its attack seen); the last block's window_sequence
    short_next: bool,
    prev_seq: u8,
    /// per channel: the last 8 sub-block energies and the last sample (the detector's high-pass)
    det: Vec<([f64; 8], f32)>,
    /// block switching, M/S, window grouping and the one CPE budget on (the in-module KATs turn them off for the
    /// long-only / L-R / eight-group / per-channel-budget comparisons)
    blocks: bool,
    ms: bool,
    group: bool,
    cpe: bool,
    /// AACENCTNS (LEDGER SR381): the spreading tables (long, short), the bit reservoir (bits banked, its bound, a frame's
    /// largest set budget over all channels), TNS and the reservoir on (KAT switches), the windows the last unit filtered
    spread_l: Spread,
    spread_s: Spread,
    res: usize,
    res_max: usize,
    au_max: usize,
    tns: bool,
    reservoir: bool,
    tns_windows: u8,
    /// AACENCTNSNORM (LEDGER SR385): the band-normalised TNS detector on, SR381's attack gate off (KAT switches), the
    /// tonality source, and per channel the two previous long spectra `Psy::Pred` reads (kept only under Pred; each the
    /// complex spectrum, 1024 MDCT then 1024 MDST values, AACENCPSYPREDDEFAULT SR396)
    tns_norm: bool,
    tns_gate: bool,
    psy: Psy,
    pred: Vec<(Option<Vec<f32>>, Option<Vec<f32>>)>,
    /// AACENCTNSMULTI (LEDGER SR390): two filters / coef_compress on (off: one uncompressed filter a window) and the PE
    /// lend on (off: SR381's lend-all), the frame PE's running mean, the last unit's active filters and bits
    /// coef_compress saved, and the reservoir's signed lend
    tns_multi: bool,
    res_pe: bool,
    pe_mean: f64,
    tns_filters: u8,
    tns_saved: usize,
    lend: isize,
}

impl Encoder {
    /// `bitrate` in bits per second (the mean frame budget; frames borrow from the bit reservoir, AACENCTNS). Refused: a rate that is not an AAC
    /// sampling rate, more than two channels, a zero bitrate.
    pub fn new(rate: u32, channels: u16, bitrate: u32) -> Result<Encoder> {
        let sf_index = super::RATES.iter().position(|&r| r == rate).ok_or(Error::Unsupported("aac-enc: sample rate (not an AAC rate)"))?;
        if !(1..=2).contains(&channels) { return Err(Error::Unsupported("aac-enc: channels (mono or stereo only)")); }
        if bitrate == 0 { return Err(Error::Invalid("aac-enc: bitrate 0")); }
        let ch = channels as usize;
        let total = (bitrate as u64 * FRAME as u64 / rate as u64) as usize;
        // frame overhead: element id + tag (+ common_window), ID_END, the alignment
        let budget = (total.saturating_sub(FRAME_OVERHEAD) / ch).clamp(64, 6144 - 16);
        let au_max = (AU_MAX_BITS * ch).saturating_sub(FRAME_OVERHEAD + MS_MASK_MAX);
        let bpc = bitrate / channels as u32;
        let cutoff = match bpc { b if b >= 96_000 => 20_000, b if b >= 64_000 => 16_000, b if b >= 48_000 => 14_000, b if b >= 32_000 => 11_000, b if b >= 24_000 => 8_000, _ => 6_000 }.min(rate / 2);
        let bin = (cutoff as u64 * 2 * FRAME as u64 / rate as u64) as u16;
        let swb = t::SWB_1024[sf_index];
        let num_swb = t::NUM_SWB_1024[sf_index] as usize;
        let max_band = (0..num_swb).take_while(|&b| swb[b] < bin).count();
        // the same cutoff in the short windows' 128 bins
        let bin_s = (cutoff as u64 * 2 * 128 / rate as u64) as u16;
        let swb_s = t::SWB_128[sf_index];
        let num_swb_s = t::NUM_SWB_128[sf_index] as usize;
        let max_band_s = (0..num_swb_s).take_while(|&b| swb_s[b] < bin_s).count();
        Ok(Encoder {
            rate,
            channels: ch,
            sf_index,
            bitrate,
            mdct: Imdct::new(2048, [2048, 2048]),
            mdct_s: Imdct::new(256, [256, 256]),
            win: sine(2048),
            win_s: sine(256),
            pow43: pow43_table(),
            hist: vec![vec![0.0; FRAME]; ch],
            pend: Vec::new(),
            held: None,
            units: 0,
            max_band,
            max_band_s,
            budget,
            flushed: false,
            short_next: false,
            prev_seq: ONLY_LONG,
            det: vec![([0f64; 8], 0f32); ch],
            blocks: true,
            ms: true,
            group: true,
            cpe: true,
            spread_l: spread_table(swb, rate as f64 / 2048.0),
            spread_s: spread_table(swb_s, rate as f64 / 256.0),
            res: 0,
            res_max: au_max.saturating_sub(budget * ch),
            au_max,
            tns: true,
            reservoir: true,
            tns_windows: 0,
            tns_norm: true,
            tns_gate: false,
            psy: Psy::Pred,
            pred: vec![(None, None); ch],
            tns_multi: true,
            res_pe: true,
            pe_mean: 0.0,
            tns_filters: 0,
            tns_saved: 0,
            lend: 0,
        })
    }

    pub fn rate(&self) -> u32 { self.rate }
    pub fn channels(&self) -> u16 { self.channels as u16 }
    pub fn bitrate(&self) -> u32 { self.bitrate }
    pub fn sf_index(&self) -> usize { self.sf_index }

    /// The AudioSpecificConfig (§1.6.2.1): AAC LC (2), the sampling frequency index, the channel configuration, a
    /// GASpecificConfig of 1024-sample frames, no core coder, no extension.
    pub fn asc(&self) -> [u8; 2] {
        let v = (2u16 << 11) | ((self.sf_index as u16) << 7) | ((self.channels as u16) << 3);
        v.to_be_bytes()
    }

    /// Interleaved PCM in; every whole frame out as an access unit, one frame behind the input (the lookahead the
    /// block decision needs; [`Encoder::flush`] returns the held frame).
    pub fn encode(&mut self, pcm: &[i16]) -> Vec<AccessUnit> {
        self.pend.extend_from_slice(pcm);
        let n = FRAME * self.channels;
        let mut out = Vec::new();
        while self.pend.len() >= n {
            let next: Vec<i16> = self.pend.drain(..n).collect();
            self.push(next, &mut out);
        }
        out
    }

    /// The end: the held frame, the partial frame padded with silence, then one silent frame that drains the overlap.
    /// Encoding `L` frames in all yields `ceil(L / 1024) + 1` units. A second call yields nothing.
    pub fn flush(&mut self) -> Vec<AccessUnit> {
        if self.flushed { return Vec::new(); }
        self.flushed = true;
        let n = FRAME * self.channels;
        let mut out = Vec::new();
        if !self.pend.is_empty() {
            let mut block = core::mem::take(&mut self.pend);
            block.resize(n, 0);
            self.push(block, &mut out);
        }
        self.push(vec![0i16; n], &mut out);
        if let Some(cur) = self.held.take() { out.push(self.frame(&cur, &vec![0i16; n])); }
        out
    }

    /// One input frame in: the held frame is coded with this one as its lookahead, this one is held. The first frame
    /// primes the detector over the silence before it (block 0 is never short).
    fn push(&mut self, next: Vec<i16>, out: &mut Vec<AccessUnit>) {
        match self.held.take() {
            None => {
                let ch = self.channels;
                let mut look = vec![0i16; FRAME * ch];
                look[FRAME / 2 * ch..].copy_from_slice(&next[..FRAME / 2 * ch]);
                let _ = self.attack(&look);
            }
            Some(cur) => out.push(self.frame(&cur, &next)),
        }
        self.held = Some(next);
    }

    /// The attack detector over one block's window (1024 interleaved frames, the block's middle: its last 512 input
    /// samples' predecessor half and the next frame's first 512): 8 sub-blocks of 128 first-difference samples per channel,
    /// an attack where a sub-block's energy is over `ATTACK_MIN` and `ATTACK_RATIO` times the mean of the 8 before it.
    fn attack(&mut self, win: &[i16]) -> bool {
        let ch = self.channels;
        let mut hit = false;
        for c in 0..ch {
            let (hist, last) = &mut self.det[c];
            for s in 0..8 {
                let mut e = 0f64;
                for n in 0..128 {
                    let x = win[(s * 128 + n) * ch + c] as f32;
                    let d = (x - *last) as f64;
                    e += d * d;
                    *last = x;
                }
                let mean = hist.iter().sum::<f64>() / 8.0;
                if e > ATTACK_MIN && e > ATTACK_RATIO * mean { hit = true; }
                hist.rotate_left(1);
                hist[7] = e;
            }
        }
        hit
    }

    /// One ICS grid per window_sequence; EIGHT_SHORT groups its windows by `grouping` over the block's sub-block energies
    /// `env` (eight groups with `group` off), the group lengths read off the bits as the decoder's `ics_info` does.
    fn grid(&self, seq: u8, env: &[f64; 8]) -> Grid {
        if seq == EIGHT_SHORT {
            let bits = if self.group { grouping(env) } else { 0 };
            let (mut glen, mut ng) = ([0usize; 8], 1usize);
            glen[0] = 1;
            for w in 1..8 {
                if (bits >> (7 - w)) & 1 != 0 { glen[ng - 1] += 1; } else { glen[ng] = 1; ng += 1; }
            }
            Grid { short: true, ng, glen, bits, swb: t::SWB_128[self.sf_index], nb: self.max_band_s }
        } else {
            Grid { short: false, ng: 1, glen: [1, 0, 0, 0, 0, 0, 0, 0], bits: 0, swb: t::SWB_1024[self.sf_index], nb: self.max_band }
        }
    }

    /// One unit as an ADTS frame (§1.A.2: 7-byte header, no CRC, buffer fullness 0x7FF, one raw block).
    pub fn adts(&self, au: &AccessUnit) -> Vec<u8> {
        let len = au.data.len() + 7;
        let cc = self.channels as u8;
        let mut h = vec![
            0xFF,
            0xF1,
            (1 << 6) | ((self.sf_index as u8) << 2) | ((cc >> 2) & 1),
            ((cc & 3) << 6) | ((len >> 11) & 3) as u8,
            ((len >> 3) & 0xFF) as u8,
            (((len & 7) << 5) as u8) | 0x1F,
            0xFC,
        ];
        h.extend_from_slice(&au.data);
        h
    }

    /// The forward MDCT of a windowed N-sample block (N/2 coefficients; N = 2048, or 256 for a short window): the fold,
    /// the decoder's inverse of that size, the middle half read back (v[j] = -y[3N/4 - 1 - j]), scaled 2·Σ.
    fn mdct(&self, z: &[f32]) -> Vec<f32> {
        let tr = if z.len() == 256 { &self.mdct_s } else { &self.mdct };
        let (n2, n4) = (z.len() / 2, z.len() / 4);
        let mut u = vec![0f32; n2];
        for m in 0..n4 { u[m] = -z[3 * n4 - 1 - m] - z[3 * n4 + m]; }
        for m in n4..n2 { u[m] = z[m - n4] - z[3 * n4 - 1 - m]; }
        let mut y = vec![0f32; z.len()];
        tr.inverse(&u, &mut y);
        (0..n2).map(|j| -2.0 * y[3 * n4 - 1 - j]).collect()
    }

    /// The MDST of a windowed N-sample block (AACENCPSYPREDDEFAULT, LEDGER SR396): the ONE transform on the block
    /// reversed, MDST_k(z) = -(-1)^k · MDCT_k(z[N-1-n]) (checked against the direct sine sum by `aac_enc_psy_pred`).
    fn mdst(&self, z: &[f32]) -> Vec<f32> {
        let r: Vec<f32> = z.iter().rev().copied().collect();
        self.mdct(&r).into_iter().enumerate().map(|(k, v)| if k % 2 == 0 { -v } else { v }).collect()
    }

    /// The analysis window of `seq` over a raw 2048-sample block (the decoder's `synth` mirrored, sine shapes) and its
    /// spectrum: 1024 long coefficients, or eight short windows of 128 (window-major). With `mdst` (AACENCPSYPREDDEFAULT
    /// SR396) a long block's MDST of the same windowed block too (`Psy::Pred`); a short block has none.
    fn analyse(&self, seq: u8, raw: &[f32], mdst: bool) -> (Vec<f32>, Option<Vec<f32>>) {
        let (wl, ws) = (&self.win, &self.win_s);
        if seq == EIGHT_SHORT {
            let mut x = vec![0f32; 1024];
            let mut z = vec![0f32; 256];
            for w in 0..8 {
                let o = 448 + w * 128;
                for n in 0..128 {
                    z[n] = raw[o + n] * ws[n];
                    z[128 + n] = raw[o + 128 + n] * ws[127 - n];
                }
                x[w * 128..w * 128 + 128].copy_from_slice(&self.mdct(&z));
            }
            return (x, None);
        }
        let mut z = vec![0f32; 2048];
        if seq == LONG_STOP {
            for n in 0..128 { z[448 + n] = raw[448 + n] * ws[n]; }
            for n in 576..1024 { z[n] = raw[n]; }
        } else {
            for n in 0..1024 { z[n] = raw[n] * wl[n]; }
        }
        if seq == LONG_START {
            for n in 1024..1472 { z[n] = raw[n]; }
            for n in 0..128 { z[1472 + n] = raw[1472 + n] * ws[127 - n]; }
        } else {
            for n in 0..1024 { z[1024 + n] = raw[1024 + n] * wl[1023 - n]; }
        }
        (self.mdct(&z), if mdst { Some(self.mdst(&z)) } else { None })
    }

    fn frame(&mut self, cur: &[i16], next: &[i16]) -> AccessUnit {
        let ch = self.channels;
        let n = FRAME * ch;
        // the NEXT block's attack window: this frame's last 512 samples and the next frame's first 512
        let mut look = Vec::with_capacity(n);
        look.extend_from_slice(&cur[n / 2..]);
        look.extend_from_slice(&next[..n / 2]);
        // THIS block's 8 sub-block energies (computed when the last frame looked ahead), summed over the channels: the
        // grouping signal (a common_window CPE shares one ics_info), read before `attack` moves on to the next block
        let env: [f64; 8] = core::array::from_fn(|s| self.det.iter().map(|d| d.0[s]).sum());
        let attack = self.attack(&look) && self.blocks;
        // the sequence state machine (one decision for both channels: a common_window CPE needs it)
        let seq = if self.short_next || (self.prev_seq == EIGHT_SHORT && attack) {
            EIGHT_SHORT
        } else if attack {
            LONG_START
        } else if self.prev_seq == EIGHT_SHORT {
            LONG_STOP
        } else {
            ONLY_LONG
        };
        self.short_next = attack;
        self.prev_seq = seq;
        let g = self.grid(seq, &env);
        // per channel: the unfiltered spectrum in grid order, its thresholds and floors, the SR381 base candidate's
        // filtered window-major spectrum and side, and (AACENCTNSMULTI) the multi candidate where it differs
        let mut orig: Vec<Vec<f32>> = Vec::with_capacity(ch);
        let mut a0: Vec<Vec<f64>> = Vec::with_capacity(ch);
        let mut fl0: Vec<Vec<f64>> = Vec::with_capacity(ch);
        let mut filt: Vec<Vec<f32>> = Vec::with_capacity(ch);
        let mut base: Vec<TnsSide> = Vec::with_capacity(ch);
        let mut multi: Vec<Option<(Vec<f32>, TnsSide)>> = Vec::with_capacity(ch);
        for c in 0..ch {
            let mut raw = vec![0f32; 2048];
            for i in 0..FRAME {
                raw[i] = self.hist[c][i];
                let s = cur[i * ch + c] as f32;
                raw[FRAME + i] = s;
                self.hist[c][i] = s;
            }
            // AACENCPSYPREDDEFAULT (SR396): under `Psy::Pred` a long block's MDST too, from the same windowed block
            let (mut s, sine) = self.analyse(seq, &raw, self.psy == Psy::Pred && !g.short);
            // AACENCTNS: the thresholds from the unfiltered spectrum, then TNS on the window-major spectrum, the thresholds
            // of the filtered bands lowered by the synthesis filter's noise gain (`candidate`)
            let o = if g.short { interleave(&s, &g) } else { s.clone() };
            // AACENCTNSNORM: `Psy::Pred` reads the two previous long spectra (a short block clears them); AACENCPSYPREDDEFAULT
            // (SR396): each the complex spectrum, the MDCT then its MDST
            let pred = if self.psy == Psy::Pred { core::mem::replace(&mut self.pred[c], (None, None)) } else { (None, None) };
            let cur: Option<Vec<f32>> = sine.map(|m| { let mut z = o.clone(); z.extend_from_slice(&m); z });
            let prev = match (&cur, &pred) { (Some(z), (Some(p1), Some(p2))) => Some((&z[..], &p1[..], &p2[..])), _ => None };
            let (a, fl) = self.allowed(&o, &g, prev);
            if self.psy == Psy::Pred { self.pred[c] = if g.short { (None, None) } else { (cur, pred.0) }; }
            // AACENCTNSNORM: every block whose band-normalised prediction gain passes; SR381's attack gate (LONG_START /
            // EIGHT_SHORT / LONG_STOP only) is the `tns_gate` KAT switch
            let gate = self.tns && (!self.tns_gate || seq != ONLY_LONG);
            let unf = if gate && self.tns_multi { Some(s.clone()) } else { None };
            let mut t = if gate { self.tns_block(&mut s, &g) } else { TnsSide::none(g.short) };
            if self.tns_multi { t.compress(); }
            let m = match unf {
                Some(mut u) if t.any() => {
                    let mut tm = self.tns_multi(&mut u, &g, &t);
                    tm.compress();
                    if tm.plain() { None } else { Some((u, tm)) }
                }
                _ => None,
            };
            orig.push(o);
            a0.push(a);
            fl0.push(fl);
            filt.push(s);
            base.push(t);
            multi.push(m);
        }
        let (x, allowed, floors, ms0) = self.candidate(&g, &orig, &a0, &fl0, &filt, &base);
        let mut ms = ms0;
        // AACENCTNSMULTI (RES-PE): the reservoir lends by the frame's perceptual entropy (off: SR381's whole bank)
        let lend: isize = if !self.reservoir { 0 } else if self.res_pe { self.res_lend(frame_pe(&x, &allowed, &g)) } else { self.res as isize };
        self.lend = lend;
        let side: Vec<usize> = base.iter().map(|t| t.bits()).collect();
        let mut coded: Vec<Coded> = self.code(&x, &g, &allowed, &floors, &side, lend);
        let mut tns = base;
        // AACENCTNSMULTI: THE FRAME TRIAL — the multi candidate through the same lift → ms_decide → code at the same lend,
        // kept when its frame NMR is lower
        if multi.iter().any(|m| m.is_some()) {
            let (mut filt_m, mut sides_m) = (filt.clone(), tns.clone());
            for (c, m) in multi.into_iter().enumerate() {
                if let Some((u, tm)) = m { filt_m[c] = u; sides_m[c] = tm; }
            }
            let (xm, am, flm, msm) = self.candidate(&g, &orig, &a0, &fl0, &filt_m, &sides_m);
            let side_m: Vec<usize> = sides_m.iter().map(|t| t.bits()).collect();
            let cm = self.code(&xm, &g, &am, &flm, &side_m, lend);
            if self.nmr(&xm, &cm, &am, &g) < self.nmr(&x, &coded, &allowed, &g) {
                coded = cm;
                tns = sides_m;
                ms = msm;
            }
        }
        self.tns_windows = tns.iter().map(|t| t.filtered()).sum();
        self.tns_filters = tns.iter().map(|t| t.filters()).sum();
        self.tns_saved = tns.iter().map(|t| t.saved()).sum();
        let mut w = Bits::new();
        if ch == 1 {
            w.put(3, super::decoder::ID_SCE);
            w.put(4, 0);
            write_ics(&mut w, &coded[0], &g, Some(seq), &tns[0]);
        } else {
            let m = coded[0].max_sfb.max(coded[1].max_sfb);
            for c in coded.iter_mut() { c.pad(m, g.ng); }
            w.put(3, super::decoder::ID_CPE);
            w.put(4, 0);
            w.put(1, 1); // common_window: one ics_info, the M/S mask
            write_info(&mut w, seq, m, g.bits);
            let mut used: Vec<bool> = Vec::with_capacity(g.ng * m);
            for grp in 0..g.ng { for b in 0..m { used.push(ms[grp * g.nb + b]); } }
            let present = if !used.iter().any(|&u| u) { 0 } else if used.iter().all(|&u| u) { 2 } else { 1 };
            w.put(2, present);
            if present == 1 { for &u in &used { w.put(1, u as u32); } }
            write_ics(&mut w, &coded[0], &g, None, &tns[0]);
            write_ics(&mut w, &coded[1], &g, None, &tns[1]);
        }
        w.put(3, super::decoder::ID_END);
        let au = AccessUnit { data: w.finish(), pts: self.units * FRAME as u64 };
        // AACENCTNS: the reservoir banks the mean frame less the unit's REAL bits (the M/S mask and alignment included)
        if self.reservoir { self.res = (self.res + self.budget * ch + FRAME_OVERHEAD).saturating_sub(au.data.len() * 8).min(self.res_max); }
        self.units += 1;
        au
    }

    /// The psy model (AACENCTNS, LEDGER SR381): per band of the grid, the noise energy it may carry — the band energies
    /// spread across each group's bands by the block type's two-slope Bark table (`Spread`, the max of each band's own
    /// and its neighbours' attenuated), under a tonality offset from the band's spectral flatness (`TONAL_DB` for a tonal
    /// band, `NOISE_DB` for a flat one), floored at the absolute level and at the frame's mask floor (`MASK_DB` under its
    /// loudest band per coefficient). Also returned: that floor alone per band (the outer loop's coded-or-not line, the
    /// seat's ruling (B): a band the model masks is still coded when spare bits reach it).
    /// AACENCTNSNORM (SR385): with `prev` (the two previous long spectra, `Psy::Pred`) the tonality is `chaos_tonality`'s.
    /// AACENCPSYPREDDEFAULT (SR396): `prev` = (this block's complex spectrum, the two previous), each 1024 MDCT then 1024
    /// MDST values; a band whose range is all zero in either previous spectrum (silent history: nothing to predict
    /// from) takes the flatness tonality.
    fn allowed(&self, x: &[f32], g: &Grid, prev: Option<(&[f32], &[f32], &[f32])>) -> (Vec<f64>, Vec<f64>) {
        let nb = g.bands();
        let sp = if g.short { &self.spread_s } else { &self.spread_l };
        let energy: Vec<f64> = (0..nb).map(|i| { let (lo, hi) = g.rng(i); x[lo..hi].iter().map(|&v| v as f64 * v as f64).sum() }).collect();
        let floor = (0..nb).map(|i| { let (lo, hi) = g.rng(i); energy[i] / (hi - lo) as f64 }).fold(0f64, f64::max) * db(-MASK_DB);
        let mut s = energy.clone();
        for gi in 0..g.ng {
            let o = gi * g.nb;
            for b in 1..g.nb { s[o + b] = s[o + b].max(s[o + b - 1] * sp.up[b]); }
            for b in (0..g.nb.saturating_sub(1)).rev() { s[o + b] = s[o + b].max(s[o + b + 1] * sp.down[b]); }
        }
        let fl: Vec<f64> = (0..nb).map(|i| { let (lo, hi) = g.rng(i); ATH.max(floor) * (hi - lo) as f64 }).collect();
        let a = (0..nb).map(|i| {
            let (lo, hi) = g.rng(i);
            let n = (hi - lo) as f64;
            // spectral flatness: geometric over arithmetic mean of the coefficients' energies (+1: no log of zero)
            let geo = x[lo..hi].iter().map(|&v| math::ln(v as f64 * v as f64 + 1.0)).sum::<f64>() / n;
            let sfm = 10.0 * math::log10(math::exp(geo) / (energy[i] / n + 1.0));
            let alpha = match prev {
                Some((z, p1, p2)) if !silent(p1, lo, hi) && !silent(p2, lo, hi) => chaos_tonality(z, p1, p2, lo, hi),
                _ => (sfm / SFM_TONAL_DB).clamp(0.0, 1.0),
            };
            let off = NOISE_DB + alpha * (TONAL_DB - NOISE_DB);
            (s[i] * db(-off)).max(fl[i])
        }).collect();
        (a, fl)
    }

    /// TNS (§4.6.9, AACENCTNS) over a block's window-major spectrum `s`, each window analysed and filtered in place.
    fn tns_block(&self, s: &mut [f32], g: &Grid) -> TnsSide {
        let mut side = TnsSide::none(g.short);
        let (bins, num_swb, max_bands, order, res_bits) = if g.short {
            (128usize, t::NUM_SWB_128[self.sf_index] as usize, t::TNS_MAX_BANDS_128[self.sf_index] as usize, TNS_ORDER_SHORT, 3u32)
        } else {
            (1024usize, t::NUM_SWB_1024[self.sf_index] as usize, t::TNS_MAX_BANDS_1024[self.sf_index] as usize, TNS_ORDER_LONG, 4u32)
        };
        let hz = self.rate as f64 / (2.0 * bins as f64);
        let start = (0..num_swb).find(|&b| g.swb[b] as f64 * hz >= TNS_START_HZ).unwrap_or(num_swb);
        let end = max_bands.min(g.nb);
        if end <= start { return side; }
        let length = (num_swb - start) as u8;
        let (lo, hi) = (g.swb[start] as usize, g.swb[end] as usize);
        for w in 0..if g.short { 8 } else { 1 } {
            let win = &mut s[w * bins..(w + 1) * bins];
            // AACENCTNSNORM (SR385): the band-normalised detector's weights
            let wts = if self.tns_norm { Some(tns_weights(win, g.swb, start, end)) } else { None };
            if let Some((p, idx, gain)) = tns_window(win, lo, hi, order, res_bits, wts.as_deref()) {
                side.nf[w] = 1;
                side.f[w][0] = TnsFilter { length, bands: (start, end), order: p as u8, compress: false, idx, gain };
            }
        }
        side
    }

    /// The two loops on the channels' spectra over the grid, each band held under its `allowed` noise: the OUTER loop per
    /// channel, the INNER loop per budget set — every channel in ONE set against `budget · channels` (AACENCGROUPING: a
    /// CPE's common global offset, so an M/S frame spends S's leftover on M), or with `cpe` off one set per channel.
    /// AACENCTNS: a set's budget also takes its share of the reservoir's `lend` bits (AACENCTNSMULTI: signed — a low-PE
    /// frame lends back, floored at 64 bits a channel; capped at the AU max) and pays its channels' TNS `side` bits.
    fn code(&self, xs: &[Vec<f32>], g: &Grid, allowed: &[Vec<f64>], floors: &[Vec<f64>], side: &[usize], lend: isize) -> Vec<Coded> {
        let nb = g.bands();
        let mut aa: Vec<Vec<f32>> = Vec::with_capacity(xs.len());
        let mut bases: Vec<Vec<Option<(i32, i32)>>> = Vec::with_capacity(xs.len());
        for ((x, allowed), floor) in xs.iter().zip(allowed).zip(floors) {
            let a: Vec<f32> = x.iter().map(|&v| math::pow(v.abs() as f64, 0.75) as f32).collect();
            // the OUTER loop: per band, the coarsest scalefactor within its threshold, never coarser than the last that codes
            // anything (None: the band is under the floor, not coded — AACENCTNS ruling (B): a masked band over the floor
            // keeps a base the inner loop's spare bits reach)
            let mut base: Vec<Option<(i32, i32)>> = Vec::with_capacity(nb); // (sf, the finest sf without overflow)
            for i in 0..nb {
                let (lo, hi) = g.rng(i);
                let e: f64 = x[lo..hi].iter().map(|&v| v as f64 * v as f64).sum();
                let allowed = allowed[i];
                if e <= floor[i] { base.push(None); continue; }
                // the finest legal sf: |q| <= MAX_Q (maxq falls as sf rises)
                let (mut l, mut h) = (0i32, 255i32);
                while l < h { let m = (l + h) / 2; if self.band(x, &a, lo, hi, m).1 <= MAX_Q { h = m; } else { l = m + 1; } }
                let fine = l;
                // the coarsest sf whose noise stays under the threshold
                let (mut l, mut h) = (fine, 255i32);
                while l < h { let m = (l + h + 1) / 2; if self.band(x, &a, lo, hi, m).0 <= allowed { l = m; } else { h = m - 1; } }
                let coarse = l;
                // the coarsest sf that still codes a nonzero value (binds only on a band the model masks)
                let (mut l, mut h) = (fine, 255i32);
                while l < h { let m = (l + h + 1) / 2; if self.band(x, &a, lo, hi, m).1 >= 1 { l = m; } else { h = m - 1; } }
                base.push(Some((coarse.min(l), fine)));
            }
            aa.push(a);
            bases.push(base);
        }
        let sets: Vec<Vec<usize>> = if self.cpe { vec![(0..xs.len()).collect()] } else { (0..xs.len()).map(|c| vec![c]).collect() };
        let mut out: Vec<Coded> = Vec::with_capacity(xs.len());
        for set in &sets {
            let budget = ((self.budget * set.len()) as isize + lend / sets.len() as isize).max(64 * set.len() as isize) as usize;
            let budget = budget.min(self.au_max * set.len() / xs.len()).saturating_sub(set.iter().map(|&c| side[c]).sum::<usize>());
            let run = |gofs: i32, top: usize| -> Vec<Coded> { set.iter().map(|&c| self.plan(&xs[c], &aa[c], &bases[c], gofs, g, top)).collect() };
            let bits = |cs: &[Coded]| cs.iter().map(|c| c.bits).sum::<usize>();
            // the INNER loop: the smallest global offset whose frame fits the budget
            let (mut l, mut h) = (-60i32, 60i32);
            while l < h {
                let m = l + (h - l) / 2;
                if bits(&run(m, g.nb)) <= budget { h = m; } else { l = m + 1; }
            }
            let mut cs = run(l, g.nb);
            // still over at the coarsest offset: drop the top bands (of every group, every channel of the set) until it fits
            let mut top = g.nb;
            while bits(&cs) > budget && top > 0 {
                top -= 1;
                cs = run(60, top);
            }
            out.extend(cs);
        }
        out
    }

    /// Band `lo..hi` quantised at `sf`: (the noise energy, the largest |q|).
    fn band(&self, x: &[f32], a: &[f32], lo: usize, hi: usize, sf: i32) -> (f64, i32) {
        let qs = math::exp2(-0.1875 * (sf - 100) as f64) as f32;
        let ds = math::exp2(0.25 * (sf - 100) as f64) as f32;
        let mut noise = 0f64;
        let mut mq = 0i32;
        for k in lo..hi {
            let q = (a[k] * qs + 0.4054) as i32;
            mq = mq.max(q);
            let d = if q < 8192 { self.pow43[q as usize] * ds } else { 0.0 };
            let r = (x[k].abs() - d) as f64;
            noise += r * r;
        }
        (noise, mq)
    }

    /// Quantise every band at its base sf + `gofs` (never finer than its overflow limit), hold the scalefactor chain
    /// within ±60, pick the codebooks, count the bits. Bands `top..` of every window are not coded.
    fn plan(&self, x: &[f32], a: &[f32], base: &[Option<(i32, i32)>], gofs: i32, g: &Grid, top: usize) -> Coded {
        let nb = g.bands();
        let mut q = vec![0i32; 1024];
        let mut sf: Vec<i32> = vec![0; nb];
        let mut on = vec![false; nb];
        let quant = |q: &mut [i32], i: usize, s: i32| -> bool {
            let qs = math::exp2(-0.1875 * (s - 100) as f64) as f32;
            let (lo, hi) = g.rng(i);
            let mut any = false;
            for k in lo..hi {
                let m = ((a[k] * qs + 0.4054) as i32).min(MAX_Q);
                q[k] = if x[k] < 0.0 { -m } else { m };
                any |= m != 0;
            }
            any
        };
        for i in 0..nb {
            if i % g.nb >= top { continue; }
            if let Some((s, fine)) = base[i] {
                let s = (s + gofs).clamp(fine, 255);
                sf[i] = s;
                on[i] = quant(&mut q, i, s);
            }
        }
        // the chain: consecutive coded scalefactors differ by at most 60 — the finer band is coarsened (never finer, so
        // never an overflow); a band coarsened to nothing drops out and the chain is checked again
        loop {
            let idx: Vec<usize> = (0..nb).filter(|&i| on[i]).collect();
            let mut changed = false;
            for p in idx.windows(2) {
                let (i, j) = (p[0], p[1]);
                if sf[j] - sf[i] > 60 { sf[i] = sf[j] - 60; on[i] = quant(&mut q, i, sf[i]); changed = true; break; }
                if sf[i] - sf[j] > 60 { sf[j] = sf[i] - 60; on[j] = quant(&mut q, j, sf[j]); changed = true; break; }
            }
            if !changed { break; }
        }
        for i in 0..nb {
            if !on[i] { let (lo, hi) = g.rng(i); for k in lo..hi { q[k] = 0; } }
        }
        let max_sfb = (0..nb).filter(|&i| on[i]).map(|i| i % g.nb + 1).max().unwrap_or(0);
        let mut cb = vec![0u8; g.ng * max_sfb];
        let mut sfc = vec![0i32; g.ng * max_sfb];
        // global_gain, ics_info (short: 4-bit max_sfb + 7 grouping bits), pulse/tns/gain_control flags
        let mut bits = if g.short { 8 + 15 + 3 } else { 8 + 11 + 3 };
        for w in 0..g.ng {
            for b in 0..max_sfb {
                let i = w * g.nb + b;
                if on[i] {
                    let (lo, hi) = g.rng(i);
                    let (book, n) = best_book(&q[lo..hi]);
                    cb[w * max_sfb + b] = book;
                    sfc[w * max_sfb + b] = sf[i];
                    bits += n;
                }
            }
        }
        // section data: runs of one codebook per group (3-bit lengths, escape 7, short; 5-bit, escape 31, long)
        let (sb, esc) = if g.short { (3, 7) } else { (5, 31) };
        for w in 0..g.ng {
            let row = &cb[w * max_sfb..(w + 1) * max_sfb];
            let mut b = 0;
            while b < max_sfb {
                let mut e = b;
                while e < max_sfb && row[e] == row[b] { e += 1; }
                bits += 4 + sb * ((e - b) / esc + 1);
                b = e;
            }
        }
        let global_gain = cb.iter().position(|&c| c != 0).map(|j| sfc[j]).unwrap_or(100);
        let mut prev = global_gain;
        for j in 0..cb.len() {
            if cb[j] != 0 { bits += t::SF_BITS[(sfc[j] - prev + 60) as usize] as usize; prev = sfc[j]; }
        }
        Coded { max_sfb, global_gain, cb, sf: sfc, q, bits }
    }
}

/// M/S per band (AACENCSHORT): M = (L+R)/2, S = (L-R)/2 where the perceptual-entropy estimate is lower — the product
/// of max(E/T, 1) over the two channels, M/S with both thresholds min(T_L, T_R)/2 (noise_M + noise_S lands in each of L
/// and R). An M/S band's coefficients and thresholds are replaced in place; the mask (one flag per grid band) returned.
fn ms_decide(x: &mut [Vec<f32>], a: &mut [Vec<f64>], g: &Grid) -> Vec<bool> {
    let mut ms = vec![false; g.bands()];
    let (xl, xr) = x.split_at_mut(1);
    let (xl, xr) = (&mut xl[0], &mut xr[0]);
    let (al, ar) = a.split_at_mut(1);
    let (al, ar) = (&mut al[0], &mut ar[0]);
    let pe = |e: f64, t: f64| (e / t).max(1.0);
    for i in 0..g.bands() {
        let (lo, hi) = g.rng(i);
        let (mut el, mut er, mut em, mut es) = (0f64, 0f64, 0f64, 0f64);
        for k in lo..hi {
            let (l, r) = (xl[k] as f64, xr[k] as f64);
            let (m, s) = ((l + r) * 0.5, (l - r) * 0.5);
            el += l * l;
            er += r * r;
            em += m * m;
            es += s * s;
        }
        let tm = al[i].min(ar[i]) * 0.5;
        if tm <= 0.0 { continue; }
        if pe(em, tm) * pe(es, tm) < pe(el, al[i]) * pe(er, ar[i]) {
            ms[i] = true;
            for k in lo..hi {
                let (l, r) = (xl[k], xr[k]);
                xl[k] = (l + r) * 0.5;
                xr[k] = (l - r) * 0.5;
            }
            al[i] = tm;
            ar[i] = tm;
        }
    }
    ms
}

/// The grouping rule (AACENCGROUPING, LEDGER SR378): the 7 scale_factor_grouping bits from a block's 8 sub-block
/// energies (`Encoder::attack`'s, sub-block s = short window s's centre). Window w joins window w-1's group (bit 7 - w)
/// while the larger of the two energies plus `ATTACK_MIN` is within `GROUP_RATIO` of the smaller plus `ATTACK_MIN`.
fn grouping(env: &[f64; 8]) -> u32 {
    let mut bits = 0u32;
    for w in 1..8 {
        let (a, b) = (env[w - 1], env[w]);
        if a.max(b) + ATTACK_MIN <= GROUP_RATIO * (a.min(b) + ATTACK_MIN) { bits |= 1 << (7 - w); }
    }
    bits
}

/// An EIGHT_SHORT spectrum (window-major, `w * 128 + k`) in the decoder's group → band → window order over every
/// `SWB_128` band of the grid's groups: what `Grid::rng` indexes.
fn interleave(x: &[f32], g: &Grid) -> Vec<f32> {
    let mut out = Vec::with_capacity(x.len());
    let mut w0 = 0;
    for gi in 0..g.ng {
        let gl = g.glen[gi];
        for b in 0..g.swb.len() - 1 {
            let (lo, hi) = (g.swb[b] as usize, g.swb[b + 1] as usize);
            for w in w0..w0 + gl { out.extend_from_slice(&x[w * 128 + lo..w * 128 + hi]); }
        }
        w0 += gl;
    }
    out
}

/// ics_info (§4.4.2.1): EIGHT_SHORT carries a 4-bit max_sfb and the 7 grouping bits (`grouping`: bit 6 joins window
/// 1 to window 0's group; 0 is eight groups), the long sequences a 6-bit max_sfb and predictor_data_present 0; sine shape.
fn write_info(w: &mut Bits, seq: u8, max_sfb: usize, grouping: u32) {
    w.put(1, 0); // ics_reserved_bit
    w.put(2, seq as u32);
    w.put(1, 0); // window_shape: sine
    if seq == EIGHT_SHORT {
        w.put(4, max_sfb as u32);
        w.put(7, grouping); // scale_factor_grouping (AACENCGROUPING)
    } else {
        w.put(6, max_sfb as u32);
        w.put(1, 0); // predictor_data_present
    }
}

/// individual_channel_stream (§4.4.2.7): the ics_info when `info` (an SCE; a common_window CPE wrote it once), no pulse
/// or gain control, the channel's tns_data when it filtered (AACENCTNS); section data and scalefactors per group, spectral data group → band → window (one range of
/// the interleaved block per group band: the decoder's per-window reads concatenated).
fn write_ics(w: &mut Bits, c: &Coded, g: &Grid, info: Option<u8>, tns: &TnsSide) {
    w.put(8, c.global_gain as u32);
    if let Some(seq) = info { write_info(w, seq, c.max_sfb, g.bits); }
    let m = c.max_sfb;
    let (sb, esc) = if g.short { (3u32, 7usize) } else { (5u32, 31usize) };
    for win in 0..g.ng {
        let row = &c.cb[win * m..(win + 1) * m];
        let mut b = 0;
        while b < m {
            let mut e = b;
            while e < m && row[e] == row[b] { e += 1; }
            w.put(4, row[b] as u32);
            let mut len = e - b;
            while len >= esc { w.put(sb, esc as u32); len -= esc; }
            w.put(sb, len as u32);
            b = e;
        }
    }
    let mut prev = c.global_gain;
    for j in 0..c.cb.len() {
        if c.cb[j] != 0 {
            let i = (c.sf[j] - prev + 60) as usize;
            w.put(t::SF_BITS[i] as u32, t::SF_CODE[i]);
            prev = c.sf[j];
        }
    }
    w.put(1, 0); // pulse_data_present
    w.put(1, tns.any() as u32); // tns_data_present (AACENCTNS)
    if tns.any() { tns.write(w); }
    w.put(1, 0); // gain_control_data_present
    for grp in 0..g.ng {
        for b in 0..m {
            let cb = c.cb[grp * m + b];
            if cb != 0 { let (lo, hi) = g.rng(grp * g.nb + b); write_band(w, cb, &c.q[lo..hi]); }
        }
    }
}

/// The largest magnitude each spectral codebook carries, whether it is signed (§4.6.3, Table 4.152).
fn book_range(cb: u8) -> (i32, bool) {
    match cb { 1 | 2 => (1, true), 3 | 4 => (2, false), 5 | 6 => (4, true), 7 | 8 => (7, false), 9 | 10 => (12, false), _ => (MAX_Q, false) }
}

/// The codeword index of a quad (books 1-4) or pair (5-11): the inverse of `decode_ics`'s unpacking.
fn index(cb: u8, v: &[i32]) -> usize {
    match cb {
        1 | 2 => (27 * (v[0] + 1) + 9 * (v[1] + 1) + 3 * (v[2] + 1) + (v[3] + 1)) as usize,
        3 | 4 => (27 * v[0].abs() + 9 * v[1].abs() + 3 * v[2].abs() + v[3].abs()) as usize,
        5 | 6 => (9 * (v[0] + 4) + (v[1] + 4)) as usize,
        _ => {
            let m = match cb { 7 | 8 => 8, 9 | 10 => 13, _ => 17 };
            (m * v[0].abs().min(16) + v[1].abs().min(16)) as usize
        }
    }
}

/// The escape's length for |v| >= 16: N ones, a zero, N + 4 bits.
fn esc_n(v: i32) -> u32 { (31 - (v as u32).leading_zeros()) - 4 }

/// Bits of `q` in codebook `cb` (codeword + sign bits + escapes).
fn book_bits(cb: u8, q: &[i32]) -> usize {
    let (_, signed) = book_range(cb);
    let step = if cb <= 4 { 4 } else { 2 };
    let lens = t::SPEC_BITS[cb as usize - 1];
    let mut n = 0usize;
    for v in q.chunks(step) {
        n += lens[index(cb, v)] as usize;
        if !signed { n += v.iter().filter(|&&x| x != 0).count(); }
        if cb == 11 { for &x in v { if x.abs() >= 16 { n += 2 * esc_n(x.abs()) as usize + 5; } } }
    }
    n
}

/// The cheapest codebook that carries the band: (book, bits).
fn best_book(q: &[i32]) -> (u8, usize) {
    let m = q.iter().map(|x| x.abs()).max().unwrap_or(0);
    if m == 0 { return (0, 0); }
    (1u8..=11).filter(|&cb| m <= book_range(cb).0).map(|cb| (cb, book_bits(cb, q))).min_by_key(|&(_, n)| n).unwrap_or((11, 0))
}

/// spectral_data for one band in codebook `cb` (§4.4.2.7, Table 4.50; the order `decode_ics` reads).
fn write_band(w: &mut Bits, cb: u8, q: &[i32]) {
    let (_, signed) = book_range(cb);
    let step = if cb <= 4 { 4 } else { 2 };
    let (codes, lens) = (t::SPEC_CODES[cb as usize - 1], t::SPEC_BITS[cb as usize - 1]);
    for v in q.chunks(step) {
        let i = index(cb, v);
        w.put(lens[i] as u32, codes[i] as u32);
        if !signed { for &x in v { if x != 0 { w.put(1, (x < 0) as u32); } } }
        if cb == 11 {
            for &x in v {
                let m = x.abs();
                if m >= 16 {
                    let n = esc_n(m);
                    for _ in 0..n { w.put(1, 1); }
                    w.put(1, 0);
                    w.put(n + 4, (m - (1 << (n + 4))) as u32);
                }
            }
        }
    }
}

// ── AACENCTNS (LEDGER SR381): TNS analysis, the spreading tables ──

/// dB to a power ratio.
fn db(x: f64) -> f64 { math::exp2(x / 10.0 * core::f64::consts::LOG2_10) }

/// One channel's TNS side data for a block (§4.4.2.7 tns_data): per window `nf` filters (AACENCTNSMULTI, LEDGER SR390:
/// up to 2 a long window, 1 a short one), filter 0 the top one (its range ends at the decoder's num_swb), filter 1 the
/// `length` bands below it.
#[derive(Clone)]
struct TnsSide {
    short: bool,
    nf: [u8; 8],
    f: [[TnsFilter; 2]; 8],
}

/// One TNS filter: its `length` on the wire, the grid bands it filtered (`bands.0..bands.1`), the order (0: a length
/// only; upward: AACENC-TNS-DOWN owed), coef_compress, the quantised
/// parcor indices and the synthesis filter's noise gain Σh².
#[derive(Clone, Copy)]
struct TnsFilter {
    length: u8,
    bands: (usize, usize),
    order: u8,
    compress: bool,
    idx: [i32; 12],
    gain: f64,
}

impl TnsFilter {
    const OFF: TnsFilter = TnsFilter { length: 0, bands: (0, 0), order: 0, compress: false, idx: [0; 12], gain: 1.0 };
}

impl TnsSide {
    fn none(short: bool) -> TnsSide { TnsSide { short, nf: [0; 8], f: [[TnsFilter::OFF; 2]; 8] } }
    fn windows(&self) -> usize { if self.short { 8 } else { 1 } }
    /// Window `w`'s filters with an order (the ones that filter).
    fn active(&self, w: usize) -> impl Iterator<Item = &TnsFilter> + '_ { self.f[w][..self.nf[w] as usize].iter().filter(|f| f.order > 0) }
    fn any(&self) -> bool { (0..self.windows()).any(|w| self.active(w).next().is_some()) }
    /// The windows that filter (the `tns_windows` stat), the filters, the bits coef_compress saved.
    fn filtered(&self) -> u8 { (0..self.windows()).filter(|&w| self.active(w).next().is_some()).count() as u8 }
    fn filters(&self) -> u8 { (0..self.windows()).map(|w| self.active(w).count() as u8).sum() }
    fn saved(&self) -> usize { (0..self.windows()).map(|w| self.active(w).filter(|f| f.compress).map(|f| f.order as usize).sum::<usize>()).sum() }
    /// One filter a window (a multi candidate like this one equals its base).
    fn plain(&self) -> bool { (0..self.windows()).all(|w| self.nf[w] <= 1) }
    /// coef_compress (AACENCTNSMULTI) on every filter whose indices all fit one bit fewer (the decoder's sign extension
    /// then yields the same indices).
    fn compress(&mut self) {
        let half = 1i32 << (if self.short { 3 } else { 4 } - 1);
        for w in 0..self.windows() {
            for f in self.f[w].iter_mut() {
                let o = f.order as usize;
                f.compress = o > 0 && f.idx[..o].iter().all(|&v| v >= -(half / 2) && v < half / 2);
            }
        }
    }
    /// Bits of tns_data (the present flag is `plan`'s): per window n_filt, then coef_res, and per filter length, order
    /// and, when it filters, direction, coef_compress and the indices.
    fn bits(&self) -> usize {
        if !self.any() { return 0; }
        let (nf, len, ord, res) = if self.short { (1, 4, 3, 3) } else { (2, 6, 5, 4) };
        (0..self.windows())
            .map(|w| {
                let n = self.nf[w] as usize;
                nf + if n > 0 {
                    1 + self.f[w][..n].iter().map(|f| { let o = f.order as usize; len + ord + if o > 0 { 2 + (res - f.compress as usize) * o } else { 0 } }).sum::<usize>()
                } else {
                    0
                }
            })
            .sum()
    }
    fn write(&self, w: &mut Bits) {
        let (nf, len, ord, res) = if self.short { (1, 4, 3, 3u32) } else { (2, 6, 5, 4u32) };
        for win in 0..self.windows() {
            let n = self.nf[win] as usize;
            w.put(nf, n as u32);
            if n == 0 { continue; }
            w.put(1, if self.short { 0 } else { 1 }); // coef_res: 3 bits short, 4 long
            for f in &self.f[win][..n] {
                let o = f.order as usize;
                w.put(len, f.length as u32);
                w.put(ord, o as u32);
                if o == 0 { continue; }
                w.put(1, 0); // direction: upward (AACENC-TNS-DOWN owed: a downward filter needs a pre-echo-aware cost)
                w.put(1, f.compress as u32); // coef_compress (AACENCTNSMULTI)
                let nb = res - f.compress as u32;
                for &v in &f.idx[..o] { w.put(nb, (v as u32) & ((1 << nb) - 1)); }
            }
        }
    }
    /// The thresholds of the grid's bands inside a filter's range divided by that filter's noise gain (a band two
    /// filters' windows cover in a short group: the largest).
    fn lift(&self, a: &mut [f64], g: &Grid) {
        if !self.any() { return; }
        let mut w0 = 0;
        for gi in 0..g.ng {
            let gl = g.glen[gi];
            for b in 0..g.nb {
                let gain = (w0..w0 + gl).flat_map(|w| self.active(w)).filter(|f| b >= f.bands.0 && b < f.bands.1).map(|f| f.gain).fold(1f64, f64::max);
                a[gi * g.nb + b] /= gain;
            }
            w0 += gl;
        }
    }
}

/// TNS on one window's spectrum `x` over `lo..hi`: the autocorrelation — of x·w with the band weights `w`
/// (AACENCTNSNORM, LEDGER SR385: `tns_weights`, gated at `TNS_GAIN_NORM`), of x itself without (`TNS_GAIN`) —,
/// Levinson-Durbin to `order` parcor coefficients, and where the prediction gain passes, the coefficients quantised to
/// `res_bits` (trailing zeros trim the
/// order), turned into the LPC by the decoder's own `tns_lpc`, and the spectrum replaced by the prediction residual
/// (the FIR inverse of the decoder's all-pole synthesis, from the top down). Returns (order, indices, the synthesis
/// filter's noise gain Σh²).
fn tns_window(x: &mut [f32], lo: usize, hi: usize, order: usize, res_bits: u32, w: Option<&[f64]>) -> Option<(usize, [i32; 12], f64)> {
    let n = hi.saturating_sub(lo);
    if n <= 2 * order { return None; }
    let mut r = [0f64; 13];
    for (k, rk) in r.iter_mut().enumerate().take(order + 1) {
        *rk = match w {
            Some(w) => (lo..hi - k).map(|i| x[i] as f64 * w[i] * x[i + k] as f64 * w[i + k]).sum(),
            None => (lo..hi - k).map(|i| x[i] as f64 * x[i + k] as f64).sum(),
        };
    }
    if r[0] <= 0.0 { return None; }
    let mut a = [0f64; 13];
    a[0] = 1.0;
    let mut k = [0f64; 12];
    let mut err = r[0];
    for m in 1..=order {
        let acc = r[m] + (1..m).map(|i| a[i] * r[m - i]).sum::<f64>();
        let km = (-acc / err).clamp(-0.9999, 0.9999);
        let prev = a;
        for i in 1..m { a[i] = prev[i] + km * prev[m - i]; }
        a[m] = km;
        k[m - 1] = km;
        err *= 1.0 - km * km;
        if err <= 0.0 { return None; }
    }
    if r[0] / err <= if w.is_some() { TNS_GAIN_NORM } else { TNS_GAIN } { return None; }
    let half = (1i32 << (res_bits - 1)) as f64;
    let (iqfac, iqfac_m) = ((half - 0.5) / (core::f64::consts::PI / 2.0), (half + 0.5) / (core::f64::consts::PI / 2.0));
    let mut idx = [0i32; 12];
    for m in 0..order {
        let asin = math::atan(k[m] / math::sqrt(1.0 - k[m] * k[m]));
        let q = math::round(asin * if k[m] >= 0.0 { iqfac } else { iqfac_m }) as i32;
        idx[m] = q.clamp(-(half as i32), half as i32 - 1);
    }
    let p = (0..order).rev().find(|&m| idx[m] != 0).map(|m| m + 1)?;
    let lpc = super::decoder::tns_lpc(&idx[..p], res_bits);
    for i in (lo..hi).rev() {
        let mut e = x[i];
        for j in 1..=p.min(i - lo) { e += lpc[j - 1] * x[i - j]; }
        x[i] = e;
    }
    let mut h = vec![0f64; n];
    h[0] = 1.0;
    let mut gain = 1.0;
    for i in 1..n {
        let mut y = 0f64;
        for j in 1..=p.min(i) { y -= lpc[j - 1] as f64 * h[i - j]; }
        h[i] = y;
        gain += y * y;
    }
    Some((p, idx, gain))
}

/// AACENCTNSNORM (LEDGER SR385): the band normalisation of TNS detection (3GPP TS 26.403; the FDK encoder's weighted
/// spectrum): per scalefactor band `start..end` of the window `x` (band edges `swb`), 1/sqrt(mean energy + 1), smoothed
/// half-and-half forward then backward across the bands, spread over the band's lines (zero outside the range).
fn tns_weights(x: &[f32], swb: &[u16], start: usize, end: usize) -> Vec<f64> {
    let mut wb: Vec<f64> = (start..end)
        .map(|b| {
            let (lo, hi) = (swb[b] as usize, swb[b + 1] as usize);
            let e = x[lo..hi].iter().map(|&v| v as f64 * v as f64).sum::<f64>() / (hi - lo) as f64;
            1.0 / math::sqrt(e + 1.0)
        })
        .collect();
    for i in 1..wb.len() { wb[i] = 0.5 * wb[i - 1] + 0.5 * wb[i]; }
    for i in (0..wb.len().saturating_sub(1)).rev() { wb[i] = 0.5 * wb[i + 1] + 0.5 * wb[i]; }
    let mut w = vec![0f64; x.len()];
    for (j, b) in (start..end).enumerate() {
        for v in w[swb[b] as usize..swb[b + 1] as usize].iter_mut() { *v = wb[j]; }
    }
    w
}

/// AACENCTNSNORM (LEDGER SR385), `Psy::Pred`; AACENCPSYPREDDEFAULT (LEDGER SR396) the complex predictor of 3GPP TS 26.403
/// / ISO/IEC 13818-7 model 2: band `lo..hi`'s tonality from the unpredictability of its COMPLEX spectrum. `z`, `p1`,
/// `p2` (this block, the two previous long blocks) each hold 1024 MDCT then 1024 MDST values: per line z = X + jS,
/// r = |z|, the prediction r̂ = 2·r1 - r2 at the phase 2·φ1 - φ2 (the unit phasor (z1/r1)²·conj(z2/r2): no trig; 1 where
/// a previous line is zero), the chaos c = |z - r̂·e^{jφ̂}| / (r + |r̂|), the band's energy-weighted mean c mapped by
/// PRED_A - PRED_B · ln c into 0..1. (SR385's magnitude-only r̂ = 2|X1| - |X2| read a tone whose MDCT magnitudes cycle
/// as noise; deleted.)
fn chaos_tonality(z: &[f32], p1: &[f32], p2: &[f32], lo: usize, hi: usize) -> f64 {
    const H: usize = FRAME;
    let (mut cw, mut ew) = (0f64, 0f64);
    for i in lo..hi {
        let (xr, xi) = (z[i] as f64, z[H + i] as f64);
        let (ar, ai) = (p1[i] as f64, p1[H + i] as f64);
        let (br, bi) = (p2[i] as f64, p2[H + i] as f64);
        let r = math::sqrt(xr * xr + xi * xi);
        let r1 = math::sqrt(ar * ar + ai * ai);
        let r2 = math::sqrt(br * br + bi * bi);
        let rh = 2.0 * r1 - r2;
        let (ur, ui) = if r1 > 1e-9 && r2 > 1e-9 {
            let (a, b) = (ar / r1, ai / r1);
            let (sr, si) = (a * a - b * b, 2.0 * a * b);
            let (c, d) = (br / r2, -bi / r2);
            (sr * c - si * d, sr * d + si * c)
        } else {
            (1.0, 0.0)
        };
        let (pr, pi) = (rh * ur, rh * ui);
        let c = math::sqrt((xr - pr) * (xr - pr) + (xi - pi) * (xi - pi)) / (r + rh.abs() + 1e-9);
        cw += c * r * r;
        ew += r * r;
    }
    let chaos = if ew > 0.0 { cw / ew } else { 1.0 };
    (PRED_A - PRED_B * math::ln(chaos.max(1e-9))).clamp(0.0, 1.0)
}

/// AACENCPSYPREDDEFAULT (SR396): band `lo..hi` of a stored complex spectrum is all zero (MDCT and MDST): silent history.
fn silent(p: &[f32], lo: usize, hi: usize) -> bool { p[lo..hi].iter().chain(p[FRAME + lo..FRAME + hi].iter()).all(|&v| v == 0.0) }

/// The two-slope Bark spreading of one block type's bands (3GPP TS 26.403's AAC psy model): `up[b]` the power factor
/// from band b-1 into b (`SPREAD_HI_DB` per Bark), `down[b]` from band b+1 into b (`SPREAD_LO_DB` per Bark).
struct Spread {
    up: Vec<f64>,
    down: Vec<f64>,
}

/// The spreading table over `swb` (band edges in bins of `hz` each), the Bark distance between band centres.
fn spread_table(swb: &[u16], hz: f64) -> Spread {
    let n = swb.len() - 1;
    let z: Vec<f64> = (0..n).map(|b| bark((swb[b] as f64 + swb[b + 1] as f64) * 0.5 * hz)).collect();
    let up = (0..n).map(|b| if b == 0 { 0.0 } else { db(-SPREAD_HI_DB * (z[b] - z[b - 1])) }).collect();
    let down = (0..n).map(|b| if b + 1 == n { 0.0 } else { db(-SPREAD_LO_DB * (z[b + 1] - z[b])) }).collect();
    Spread { up, down }
}

/// Hz to Bark (Zwicker): 13·atan(0.00076 f) + 3.5·atan((f / 7500)²).
fn bark(f: f64) -> f64 { 13.0 * math::atan(0.00076 * f) + 3.5 * math::atan((f / 7500.0) * (f / 7500.0)) }


// ── tns_multi (AACENCTNSMULTI, LEDGER SR390): two filters a long window, coef_compress, the frame trial ──
impl Encoder {
    /// The multi candidate over a block's UNFILTERED window-major spectrum `s` (filtered in place), refining only the
    /// long windows where the base filter fired (so the filtered windows are the base's). The measured rule is the
    /// residual energy of the QUANTISED filter over its range (`tns_window` on a scratch copy, with the range's own
    /// AACENCTNSNORM weights; the range's own energy where it does not fire): the window splits at the band b*
    /// minimising the top range's [b*, end) plus the bottom's [start, b*) — the top filter stops where extending it down
    /// raises the error — when that sum is below the single filter's. Upward only (AACENC-TNS-DOWN owed: measured, the
    /// NMR trial picked downward filters that raised the short-block pre-echo 5.721e4 -> 1.190e5).
    fn tns_multi(&self, s: &mut [f32], g: &Grid, base: &TnsSide) -> TnsSide {
        let mut side = base.clone();
        if g.short { return side; } // n_filt is 1 bit a short window
        let num_swb = t::NUM_SWB_1024[self.sf_index] as usize;
        let edge = |b: usize| g.swb[b] as usize;
        // filter range b0..b1 of `x` (AACENCTNSNORM: the range's own band weights, as `tns_block` takes them)
        let run = |x: &mut [f32], b0: usize, b1: usize| -> Option<(usize, [i32; 12], f64)> {
            let wts = if self.tns_norm { Some(tns_weights(x, g.swb, b0, b1)) } else { None };
            tns_window(x, edge(b0), edge(b1), TNS_ORDER_LONG, 4, wts.as_deref())
        };
        // a range's residual energy after its quantised filter (its own energy where it does not fire), and whether it fired
        let seg = |x: &[f32], b0: usize, b1: usize| -> (f64, bool) {
            let (lo, hi) = (edge(b0), edge(b1));
            let mut y = x.to_vec();
            let fired = run(&mut y, b0, b1).is_some();
            (y[lo..hi].iter().map(|&v| v as f64 * v as f64).sum::<f64>(), fired)
        };
        let f0 = side.f[0][0];
        let (start, end) = f0.bands;
        let x = &mut s[..1024];
        if side.nf[0] == 0 { return side; }
        let single = seg(x, start, end);
        // (summed residual, b, the top range fired, the bottom fired)
        let mut split: Option<(f64, usize, bool, bool)> = None;
        for b in start + 1..end {
            let (top, bot) = (seg(x, b, end), seg(x, start, b));
            if !top.1 && !bot.1 { continue; }
            let e = top.0 + bot.0;
            if split.map_or(true, |sp| e < sp.0) { split = Some((e, b, top.1, bot.1)); }
        }
        // the same `tns_window` calls the rule measured, now on the candidate's spectrum
        let apply = |x: &mut [f32], b0: usize, b1: usize, length: u8| -> TnsFilter {
            let mut f = TnsFilter { length, bands: (b0, b1), ..TnsFilter::OFF };
            if let Some((p, idx, gain)) = run(x, b0, b1) {
                f.order = p as u8;
                f.idx = idx;
                f.gain = gain;
            }
            f
        };
        match split {
            Some((e, b, top, bot)) if e < single.0 => {
                let ft = if top { apply(x, b, end, (num_swb - b) as u8) } else { TnsFilter { length: (num_swb - b) as u8, bands: (b, end), ..TnsFilter::OFF } };
                if bot {
                    let fb = apply(x, start, b, (b - start) as u8);
                    side.nf[0] = 2;
                    side.f[0] = [ft, fb];
                } else {
                    side.nf[0] = 1;
                    side.f[0] = [ft, TnsFilter::OFF];
                }
            }
            // the single filter: the base's own (the same call on the same spectrum)
            _ => side.f[0][0] = apply(x, start, end, f0.length),
        }
        side
    }

    /// One candidate's coding inputs: per channel the thresholds and floors lifted by its TNS side, the spectrum (the
    /// filtered one where the side filters, in grid order), then M/S over the pair.
    fn candidate(&self, g: &Grid, orig: &[Vec<f32>], a0: &[Vec<f64>], fl0: &[Vec<f64>], filt: &[Vec<f32>], sides: &[TnsSide]) -> (Vec<Vec<f32>>, Vec<Vec<f64>>, Vec<Vec<f64>>, Vec<bool>) {
        let ch = orig.len();
        let mut x: Vec<Vec<f32>> = Vec::with_capacity(ch);
        let mut allowed: Vec<Vec<f64>> = Vec::with_capacity(ch);
        let mut floors: Vec<Vec<f64>> = Vec::with_capacity(ch);
        for c in 0..ch {
            let (mut a, mut fl) = (a0[c].clone(), fl0[c].clone());
            let t = &sides[c];
            t.lift(&mut a, g);
            t.lift(&mut fl, g);
            x.push(if !t.any() { orig[c].clone() } else if g.short { interleave(&filt[c], g) } else { filt[c].clone() });
            allowed.push(a);
            floors.push(fl);
        }
        let ms = if ch == 2 && self.ms { ms_decide(&mut x, &mut allowed, g) } else { vec![false; g.bands()] };
        (x, allowed, floors, ms)
    }

    /// The frame trial's cost: the quantised error at the budget over the mask, Σ over the channels' grid bands of the
    /// noise (`band`, the outer loop's own measure, at a coded band's final scalefactor; the band's energy where it is
    /// not coded) over its `allowed` threshold.
    fn nmr(&self, xs: &[Vec<f32>], cs: &[Coded], allowed: &[Vec<f64>], g: &Grid) -> f64 {
        let mut total = 0f64;
        for ((x, c), al) in xs.iter().zip(cs).zip(allowed) {
            let a: Vec<f32> = x.iter().map(|&v| math::pow(v.abs() as f64, 0.75) as f32).collect();
            for i in 0..g.bands() {
                let (gi, b) = (i / g.nb, i % g.nb);
                let (lo, hi) = g.rng(i);
                let j = gi * c.max_sfb + b;
                let noise = if b < c.max_sfb && c.cb[j] != 0 { self.band(x, &a, lo, hi, c.sf[j]).0 } else { x[lo..hi].iter().map(|&v| v as f64 * v as f64).sum() };
                total += noise / al[i];
            }
        }
        total
    }
}

// ── res_pe (AACENCTNSMULTI, LEDGER SR390): the bit reservoir lends by perceptual entropy (3GPP TS 26.403 §5.4 / §5.6) ──
/// The frame PE's running mean (frames), the PE ratio at which a frame takes the whole bank, and the largest share of
/// the mean frame a low-PE frame saves (26.403's maxBitSave for long blocks).
const PE_AVG: f64 = 16.0;
const R_SPEND: f64 = 2.0;
const SAVE_MAX: f64 = 0.3;

impl Encoder {
    /// The reservoir's signed lend for a frame of perceptual entropy `pe` (the bit-demand curve over the fill f =
    /// res / res_max, r = pe over the running mean): spend = res · clamp(f + (r − 1)/(R_SPEND − 1), 0, 1), save = the
    /// mean frame · SAVE_MAX · max(0, 1 − r) · (1 − f); lend = spend − save. Then the mean moves (the first frame with
    /// a PE seeds it).
    fn res_lend(&mut self, pe: f64) -> isize {
        let mean = (self.budget * self.channels + FRAME_OVERHEAD) as f64;
        let r = if pe <= 0.0 { 0.0 } else if self.pe_mean <= 0.0 { R_SPEND } else { pe / self.pe_mean };
        if self.pe_mean <= 0.0 { if pe > 0.0 { self.pe_mean = pe; } } else { self.pe_mean += (pe - self.pe_mean) / PE_AVG; }
        let f = if self.res_max == 0 { 1.0 } else { self.res as f64 / self.res_max as f64 };
        let spend = self.res as f64 * (f + (r - 1.0) / (R_SPEND - 1.0)).clamp(0.0, 1.0);
        let save = mean * SAVE_MAX * (1.0 - r).max(0.0) * (1.0 - f);
        (spend - save) as isize
    }
}

/// The frame's perceptual entropy (26.403 §5.4's form): Σ over the channels' grid bands of `pe_band`.
fn frame_pe(xs: &[Vec<f32>], allowed: &[Vec<f64>], g: &Grid) -> f64 {
    let mut pe = 0f64;
    for (x, a) in xs.iter().zip(allowed) {
        for i in 0..g.bands() {
            let (lo, hi) = g.rng(i);
            let e: f64 = x[lo..hi].iter().map(|&v| v as f64 * v as f64).sum();
            pe += pe_band(e, a[i], (hi - lo) as f64);
        }
    }
    pe
}

/// One band's PE: n · log2(E / T) where the energy E is over the threshold T, else 0.
fn pe_band(e: f64, t: f64, n: f64) -> f64 { if t > 0.0 && e > t { n * math::log2(e / t) } else { 0.0 } }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aac::{AacDecoder, AdtsStream, Asc, Layout};
    use crate::bits::BitReader;

    /// Decode raw units through OUR decoder: per channel, i16-scaled f64.
    fn decode(e: &Encoder, units: &[AccessUnit]) -> Vec<Vec<f64>> {
        let mut d = AacDecoder::new(e.sf_index(), Layout::from_config(e.channels() as u8).unwrap()).unwrap();
        let mut out = vec![Vec::new(); e.channels() as usize];
        for u in units {
            let mut r = BitReader::new(&u.data);
            d.decode_block(&mut r, 0).unwrap();
            for (c, o) in out.iter_mut().enumerate() { o.extend(d.out[c].iter().map(|&v| v as f64 * 32768.0)); }
        }
        out
    }

    fn sine_pcm(rate: u32, frames: usize, hz: f64, amp: f64) -> Vec<i16> {
        let mut v = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let p = 2.0 * core::f64::consts::PI * hz * n as f64 / rate as f64;
            v.push((amp * math::sin(p)) as i16);
            v.push((amp * math::sin(p + 0.7)) as i16);
        }
        v
    }

    /// The forward transform (the fold over the decoder's `Imdct::inverse`) against the definition, N = 2048.
    #[test]
    fn aac_enc_mdct_matches_direct() {
        let e = Encoder::new(48000, 1, 64000).unwrap();
        let z: Vec<f32> = (0..2048).map(|n| (math::sin(n as f64 * 0.37) * 1000.0 + math::cos(n as f64 * 0.011) * 300.0) as f32).collect();
        let x = e.mdct(&z);
        let mut worst = 0f64;
        let mut peak = 0f64;
        for k in (0..1024).step_by(37) {
            let mut s = 0f64;
            for n in 0..2048 {
                s += z[n] as f64 * math::cos(2.0 * core::f64::consts::PI / 2048.0 * (n as f64 + 0.5 + 512.0) * (k as f64 + 0.5));
            }
            worst = worst.max((2.0 * s - x[k] as f64).abs());
            peak = peak.max((2.0 * s).abs());
        }
        assert!(worst <= peak * 1e-4, "worst {worst} peak {peak}");
    }

    /// The AudioSpecificConfig bytes, and our parser reads them back.
    #[test]
    fn aac_enc_asc_bytes() {
        let a = Encoder::new(48000, 2, 128000).unwrap().asc();
        assert_eq!(a, [0x11, 0x90]);
        let b = Encoder::new(44100, 1, 64000).unwrap().asc();
        assert_eq!(b, [0x12, 0x08]);
        let p = Asc::parse(&a).unwrap();
        assert_eq!((p.object_type, p.sf_index, p.channel_config, p.sbr), (2, 3, 2, false));
        assert!(Encoder::new(44000, 2, 128000).is_err());
        assert!(Encoder::new(48000, 3, 128000).is_err());
    }

    /// A 1 kHz sine at 128 kb/s stereo decodes through OUR decoder at >= 30 dB SNR, within the bitrate. Measured: 56.8 dB
    /// (AACENCTNSNORM SR385: 56.9/57.5, TNS now on its stationary blocks) at MASK_DB 60 (49.6 at 50, 40.2 at 40, 14.4 with no mask floor; 34.0 / 54.3 dB at 3000 / 6000 bits a channel
    /// without it: the transform and bitstream are exact, the allocation is the psy model's).
    #[test]
    fn aac_enc_sine_snr() {
        let frames = 48000;
        let pcm = sine_pcm(48000, frames, 1000.0, 16000.0);
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        assert_eq!(units.len(), frames.div_ceil(1024) + 1);
        for (i, u) in units.iter().enumerate() { assert_eq!(u.pts, i as u64 * 1024); }
        let bytes: usize = units.iter().map(|u| u.data.len()).sum();
        let secs = units.len() as f64 * 1024.0 / 48000.0;
        assert!(bytes as f64 * 8.0 <= 128000.0 * secs * 1.05, "{bytes} bytes over {secs} s");
        let d = decode(&e, &units);
        for c in 0..2 {
            let (mut s, mut n) = (0f64, 0f64);
            for i in 2048..frames - 2048 {
                let x = pcm[i * 2 + c] as f64;
                let y = d[c][i + PRIMING as usize];
                s += x * x;
                n += (x - y) * (x - y);
            }
            let snr = 10.0 * math::log10(s / n.max(1e-9));
            std::println!("aac_enc_sine_snr: channel {c} SNR {snr:.1} dB");
            assert!(snr >= 30.0, "channel {c}: SNR {snr:.1} dB");
            // AACENCTNS (LEDGER SR381): the psy model, TNS and the reservoir hold SR378's tone within 1 dB
            assert!(snr >= SINE_SR378[c] - 1.0, "channel {c}: SNR {snr:.1} dB, SR378 {:.1}", SINE_SR378[c]);
            // AACENCTNSNORM (LEDGER SR385): the band-normalised gate on every block keeps or beats SR381's tone (56.9/57.5)
            assert!(snr >= SINE_SR381[c], "channel {c}: SNR {snr:.1} dB, SR381 {:.1}", SINE_SR381[c]);
        }
    }

    /// Silence costs a few bytes a frame (max_sfb 0 in both channels).
    #[test]
    fn aac_enc_silence_tiny() {
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        let mut units = e.encode(&vec![0i16; 4096 * 2]);
        units.extend(e.flush());
        assert_eq!(units.len(), 5);
        for u in &units { assert!(u.data.len() <= 8, "{} bytes", u.data.len()); }
        let d = decode(&e, &units);
        assert!(d.iter().all(|c| c.iter().all(|&v| v.abs() < 1.0)));
    }

    /// ADTS framing round trip: the frames through `AdtsStream` decode to what the raw units decode to.
    #[test]
    fn aac_enc_adts_round_trip() {
        use crate::Source;
        let pcm = sine_pcm(44100, 10000, 440.0, 8000.0);
        let mut e = Encoder::new(44100, 2, 96000).unwrap();
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let stream: Vec<u8> = units.iter().flat_map(|u| e.adts(u)).collect();
        let raw = decode(&e, &units);
        let bs = crate::io::ByteStream::new(alloc::boxed::Box::new(crate::io::VecReader::new(stream)));
        let mut s = AdtsStream::new(bs).unwrap();
        assert_eq!((s.info().rate, s.info().channels), (44100, 2));
        let mut got = vec![Vec::new(); 2];
        let mut pcm_b = crate::Pcm::default();
        while s.block(&mut pcm_b).unwrap() {
            for c in 0..2 { got[c].extend(pcm_b.flt[c][..pcm_b.frames].iter().map(|&v| v as f64 * 32768.0)); }
        }
        assert_eq!(got[0].len(), units.len() * 1024);
        for c in 0..2 { assert!(got[c].iter().zip(&raw[c]).all(|(a, b)| (a - b).abs() < 1e-3)); }
    }

    // ---- AACENCSHORT (LEDGER SR375): block switching and M/S

    /// The unit's window_sequence off the wire: after id/tag, an SCE's global_gain (8) or a CPE's common_window (1),
    /// then ics_reserved_bit (1) and the 2-bit sequence.
    fn wire_seq(u: &AccessUnit) -> u32 {
        let mut r = BitReader::new(&u.data);
        let id = r.read(3).unwrap();
        r.read(4).unwrap();
        if id == crate::aac::decoder::ID_CPE { r.read(1).unwrap(); } else { r.read(8).unwrap(); }
        r.read(1).unwrap();
        r.read(2).unwrap()
    }

    /// A CPE unit's M/S mask off the wire: (ms_mask_present, bands flagged, bands in the mask = groups × max_sfb).
    fn wire_ms(u: &AccessUnit) -> (u32, usize, usize) {
        let mut r = BitReader::new(&u.data);
        assert_eq!(r.read(3).unwrap(), crate::aac::decoder::ID_CPE);
        r.read(4).unwrap();
        assert_eq!(r.read(1).unwrap(), 1, "common_window");
        r.read(1).unwrap();
        let seq = r.read(2).unwrap();
        r.read(1).unwrap();
        let (groups, max_sfb) = if seq == 2 { let m = r.read(4).unwrap() as usize; (8 - r.read(7).unwrap().count_ones() as usize, m) } else { let m = r.read(6).unwrap() as usize; r.read(1).unwrap(); (1, m) };
        let present = r.read(2).unwrap();
        let n = groups * max_sfb;
        let on = match present { 1 => (0..n).filter(|_| r.read(1).unwrap() == 1).count(), 2 => n, _ => 0 };
        (present, on, n)
    }

    /// An EIGHT_SHORT unit's 7 scale_factor_grouping bits off the wire (AACENCGROUPING); None for a long sequence.
    fn wire_grouping(u: &AccessUnit) -> Option<u32> {
        let mut r = BitReader::new(&u.data);
        let id = r.read(3).unwrap();
        r.read(4).unwrap();
        if id == crate::aac::decoder::ID_CPE { r.read(1).unwrap(); } else { r.read(8).unwrap(); }
        r.read(1).unwrap();
        let seq = r.read(2).unwrap();
        r.read(1).unwrap();
        if seq != 2 { return None; }
        r.read(4).unwrap();
        Some(r.read(7).unwrap())
    }

    /// Castanet-like bursts on silence: 64 samples of a decaying 3 kHz tone (amplitude 16000) at each position; the
    /// second channel at half level.
    fn bursts(frames: usize, at: &[usize], ch: usize) -> Vec<i16> {
        let mut v = vec![0i16; frames * ch];
        for &p in at {
            for n in 0..64 {
                let s = 16000.0 * math::exp(-(n as f64) / 12.0) * math::sin(2.0 * core::f64::consts::PI * 3000.0 * n as f64 / 48000.0);
                for c in 0..ch { v[(p + n) * ch + c] = (s * if c == 0 { 1.0 } else { 0.5 }) as i16; }
            }
        }
        v
    }

    // floors under the measured numbers (LEDGER SR375): the bursts' SNR 15.5 / 16.1 dB at 128 kb/s stereo; the pre-echo
    // 10.1 dB lower with short windows (4.37e8 -> 4.27e7, mono 64 kb/s; SNR 2.6 -> 15.5 dB); M/S on 1025 of 1025 bands.
    // AACENCGROUPING (LEDGER SR378): grouped, the bursts 15.1 / 15.8 dB (eight groups 15.5 / 16.2), the short units 1011
    // bytes (eight groups 997: those frames sit at the budget, the saved side bits go to the spectrum; the group band's
    // pooled threshold costs the 0.4 dB, AACENC-GROUP-THRESHOLD owed); the pre-echo 4.37e8 -> 4.26e7, 10.1 dB (floor 10)
    const SHORT_SNR_FLOOR: f64 = 12.0;
    const PRE_ECHO_DB: f64 = 10.0;
    const MS_BANDS_FLOOR: f64 = 0.9;

    const BURSTS: [usize; 3] = [10000, 20000, 30000];
    /// AACENCGROUPING (LEDGER SR378): the grouping bits of the three short units, read off the wire.
    const CLICK_GROUPS: [u32; 3] = [0x4F, 0x3F, 0x7C];

    /// The 256-point fold (the short windows) against the definition, as `aac_enc_mdct_matches_direct` does for 2048.
    #[test]
    fn aac_enc_short_mdct_matches_direct() {
        let e = Encoder::new(48000, 1, 64000).unwrap();
        let z: Vec<f32> = (0..256).map(|n| (math::sin(n as f64 * 0.37) * 1000.0 + math::cos(n as f64 * 0.011) * 300.0) as f32).collect();
        let x = e.mdct(&z);
        assert_eq!(x.len(), 128);
        let mut worst = 0f64;
        let mut peak = 0f64;
        for k in 0..128 {
            let mut s = 0f64;
            for n in 0..256 {
                s += z[n] as f64 * math::cos(2.0 * core::f64::consts::PI / 256.0 * (n as f64 + 0.5 + 64.0) * (k as f64 + 0.5));
            }
            worst = worst.max((2.0 * s - x[k] as f64).abs());
            peak = peak.max((2.0 * s).abs());
        }
        assert!(worst <= peak * 1e-4, "worst {worst} peak {peak}");
    }

    /// The three bursts at 128 kb/s stereo with window grouping on or off: (units, per-channel SNR, the short units' bytes).
    fn clicks(group: bool) -> (Vec<AccessUnit>, [f64; 2], usize) {
        let frames = 40960;
        let pcm = bursts(frames, &BURSTS, 2);
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        e.group = group;
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let d = decode(&e, &units);
        let mut snr = [0f64; 2];
        for c in 0..2 {
            let (mut s, mut n) = (0f64, 0f64);
            for i in 0..frames {
                let x = pcm[i * 2 + c] as f64;
                let y = d[c][i + PRIMING as usize];
                s += x * x;
                n += (x - y) * (x - y);
            }
            snr[c] = 10.0 * math::log10(s / n.max(1e-9));
        }
        let short = units.iter().filter(|u| wire_seq(u) == 2).map(|u| u.data.len()).sum();
        (units, snr, short)
    }

    /// The window sequence around three bursts, read off the wire and PINNED: block u's attack window is input
    /// [(u-1)·1024 + 512, u·1024 + 512), so a burst at p makes unit (p - 512) / 1024 + 1 EIGHT_SHORT, the one before
    /// LONG_START, the one after LONG_STOP; every other unit ONLY_LONG. The stream decodes through the decoder's short path.
    /// AACENCGROUPING: the short units' grouping bits are PINNED (`CLICK_GROUPS`; the burst sits in sub-block 2, 0 and 6
    /// of units 10 / 20 / 29), the same sequence with grouping off and eight groups there.
    #[test]
    fn aac_enc_short_click_sequence() {
        let (units, snr, short) = clicks(true);
        let (units8, snr8, short8) = clicks(false);
        assert_eq!(units.len(), 41);
        for (i, u) in units.iter().enumerate() { assert_eq!(u.pts, i as u64 * 1024); }
        let seq: Vec<u32> = units.iter().map(wire_seq).collect();
        let mut want = vec![0u32; 41];
        for u in [9usize, 19, 28] { want[u] = 1; want[u + 1] = 2; want[u + 2] = 3; }
        assert_eq!(seq, want);
        assert_eq!(units8.iter().map(wire_seq).collect::<Vec<u32>>(), want);
        let grp: Vec<Option<u32>> = units.iter().map(wire_grouping).collect();
        let mut want_grp = vec![None; 41];
        for (k, u) in [10usize, 20, 29].into_iter().enumerate() { want_grp[u] = Some(CLICK_GROUPS[k]); }
        assert_eq!(grp, want_grp);
        for (k, u) in [10usize, 20, 29].into_iter().enumerate() {
            let groups = 8 - CLICK_GROUPS[k].count_ones();
            assert!((2..=8).contains(&groups), "unit {u}: {groups} groups");
            assert_eq!(wire_grouping(&units8[u]), Some(0));
        }
        std::println!("aac_enc_short_click_sequence: grouping {:#04x} {:#04x} {:#04x}; short units {short} bytes (eight groups {short8}); SNR {:.1}/{:.1} dB (eight groups {:.1}/{:.1})", CLICK_GROUPS[0], CLICK_GROUPS[1], CLICK_GROUPS[2], snr[0], snr[1], snr8[0], snr8[1]);
        for c in 0..2 { assert!(snr[c] >= SHORT_SNR_FLOOR, "channel {c}: SNR {:.1} dB", snr[c]); }
    }

    /// Mono bursts at 64 kb/s, block switching on vs off: (the decoded energy in [p - 768, p - 64) summed over the
    /// bursts — the input is silence there, so all of it is pre-echo — and the whole signal's SNR).
    fn pre_echo(blocks: bool) -> (f64, f64) {
        let frames = 40960;
        let pcm = bursts(frames, &BURSTS, 1);
        let mut e = Encoder::new(48000, 1, 64000).unwrap();
        e.blocks = blocks;
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let d = decode(&e, &units);
        let mut pre = 0f64;
        for &p in &BURSTS { for i in p - 768..p - 64 { pre += d[0][i + PRIMING as usize] * d[0][i + PRIMING as usize]; } }
        let (mut s, mut n) = (0f64, 0f64);
        for i in 0..frames {
            let x = pcm[i] as f64;
            let y = d[0][i + PRIMING as usize];
            s += x * x;
            n += (x - y) * (x - y);
        }
        (pre, 10.0 * math::log10(s / n.max(1e-9)))
    }

    /// Short windows hold the bursts' quantisation noise near them: the pre-echo falls by at least PRE_ECHO_DB.
    #[test]
    fn aac_enc_short_pre_echo() {
        let (ps, ss) = pre_echo(true);
        let (pl, sl) = pre_echo(false);
        let db = 10.0 * math::log10(pl / ps.max(1e-9));
        std::println!("aac_enc_short_pre_echo: pre-echo long {pl:.3e} short {ps:.3e} ({db:.1} dB lower), SNR long {sl:.1} short {ss:.1} dB");
        assert!(db >= PRE_ECHO_DB, "pre-echo only {db:.1} dB lower");
        // AACENCTNS (LEDGER SR381): TNS, the psy model and the reservoir together, against SR378's block switching alone
        let vs = 10.0 * math::log10(PRE_ECHO_SR378 / ps.max(1e-9));
        std::println!("aac_enc_short_pre_echo: short {ps:.3e} vs SR378's {PRE_ECHO_SR378:.3e} ({vs:.1} dB lower)");
        assert!(vs >= PRE_ECHO_VS_SR378_DB, "only {vs:.1} dB under SR378");
    }

    /// L = a broadband noise, R = L + an independent ±8: (units, total bytes, per-channel SNR) with M/S on or off and the
    /// one CPE budget on or off.
    /// AACENCPSYPREDDEFAULT (SR396): the default's tonality source, `Psy::Pred`.
    fn correlated(ms: bool, cpe: bool) -> (Vec<AccessUnit>, usize, [f64; 2]) { correlated_psy(ms, cpe, Psy::Pred) }

    /// `correlated` with the psy model's tonality source (AACENCTNSNORM, LEDGER SR385: `aac_enc_psy_pred`).
    fn correlated_psy(ms: bool, cpe: bool, psy: Psy) -> (Vec<AccessUnit>, usize, [f64; 2]) {
        let frames = 24000;
        let (mut a, mut b) = (0x1234_5678u32, 0x9abc_def0u32);
        let mut pcm = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            a = a.wrapping_mul(1664525).wrapping_add(1013904223);
            b = b.wrapping_mul(22695477).wrapping_add(1);
            let l = ((a >> 8) % 12001) as i32 - 6000;
            let r = l + ((b >> 8) % 17) as i32 - 8;
            pcm.push(l as i16);
            pcm.push(r as i16);
        }
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        e.ms = ms;
        e.cpe = cpe;
        e.reservoir = cpe; // AACENCTNS: the per-channel arms measure the idle bits a reservoir would re-spend
        e.psy = psy;
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let bytes = units.iter().map(|u| u.data.len()).sum();
        let d = decode(&e, &units);
        let mut snr = [0f64; 2];
        for c in 0..2 {
            let (mut s, mut n) = (0f64, 0f64);
            for i in 2048..frames - 2048 {
                let x = pcm[i * 2 + c] as f64;
                let y = d[c][i + PRIMING as usize];
                s += x * x;
                n += (x - y) * (x - y);
            }
            snr[c] = 10.0 * math::log10(s / n.max(1e-9));
        }
        (units, bytes, snr)
    }

    /// A correlated pair (L ≈ R) takes M/S on most bands (read off the wire), costs fewer bytes than the independent
    /// L/R coding (S falls under its threshold and codes nothing) and decodes through the decoder's M/S path no worse.
    /// Both at SR375's per-channel budget (`cpe` off): the byte saving is that budget's; `aac_enc_cpe_budget` spends it.
    #[test]
    fn aac_enc_ms_correlated() {
        let (u_ms, b_ms, s_ms) = correlated(true, false);
        let (u_lr, b_lr, s_lr) = correlated(false, false);
        let (mut on, mut all) = (0usize, 0usize);
        for u in &u_ms { let (_, o, n) = wire_ms(u); on += o; all += n; }
        for u in &u_lr { assert_eq!(wire_ms(u).0, 0); }
        let frac = on as f64 / all.max(1) as f64;
        std::println!("aac_enc_ms_correlated: M/S on {on}/{all} bands ({:.1}%), bytes M/S {b_ms} L/R {b_lr}, SNR M/S {:.1}/{:.1} L/R {:.1}/{:.1} dB", frac * 100.0, s_ms[0], s_ms[1], s_lr[0], s_lr[1]);
        assert!(frac >= MS_BANDS_FLOOR, "M/S on {on}/{all}");
        assert!(b_ms < b_lr, "M/S {b_ms} bytes, L/R {b_lr}");
        for c in 0..2 { assert!(s_ms[c] >= s_lr[c] - 1.0, "channel {c}: M/S {:.1} dB, L/R {:.1} dB", s_ms[c], s_lr[c]); }
    }

    // ---- AACENCGROUPING (LEDGER SR378): one budget across the CPE

    /// The correlated pair with M/S: one budget (the common global offset over both channels) against SR375's
    /// per-channel budget. S codes almost nothing, so its leftover now funds M: the SNR rises and the stream stays within
    /// the bitrate's bytes (no slack). Measured (LEDGER SR378): 8434 bytes at 4.9/4.9 dB against 4205 bytes at 3.1/3.1 dB;
    /// the bitrate allows 8533. The L/R stream's bytes (8173) are the wrong bound: each L/R channel is held to its own
    /// budget and leaves ~3% of it unspent, which one shared budget spends.
    #[test]
    fn aac_enc_cpe_budget() {
        let (u_one, b_one, s_one) = correlated(true, true);
        let (_, b_two, s_two) = correlated(true, false);
        let (_, b_lr, _) = correlated(false, false);
        std::println!("aac_enc_cpe_budget: M/S one budget {b_one} bytes SNR {:.1}/{:.1} dB, per-channel {b_two} bytes SNR {:.1}/{:.1} dB, L/R {b_lr} bytes", s_one[0], s_one[1], s_two[0], s_two[1]);
        for c in 0..2 { assert!(s_one[c] > s_two[c], "channel {c}: one budget {:.1} dB, per-channel {:.1} dB", s_one[c], s_two[c]); }
        // AACENCTNS (LEDGER SR381): no worse than SR378's one budget (4.9 dB)
        for c in 0..2 { assert!(s_one[c] >= CPE_SR378, "channel {c}: one budget {:.1} dB, SR378 {CPE_SR378}", s_one[c]); }
        let cap = 128000 * u_one.len() * 1024 / 48000 / 8;
        assert!(b_one <= cap, "one budget {b_one} bytes, the bitrate's {cap}");
    }

    // ---- AACENCTNS (LEDGER SR381): TNS, the spread/tonality psy model, the bit reservoir

    /// SR378's measured sine SNR (dB, per channel) and its one-budget correlated SNR: the floors this arc keeps.
    const SINE_SR378: [f64; 2] = [56.5, 57.1];
    const CPE_SR378: f64 = 4.9;
    /// AACENCTNSNORM (LEDGER SR385): SR381's measured sine SNR, the floor this arc keeps or beats.
    const SINE_SR381: [f64; 2] = [56.3, 56.8];
    /// AACENCTNS: (unit, windows TNS filtered) on the tone + noise-burst stream — LONG_START / EIGHT_SHORT / LONG_STOP of
    /// each burst (`aac_enc_short_click_sequence`'s units), the short units' filtered windows of eight. AACENCTNSNORM
    /// (LEDGER SR385; SR381's pin (9,1) (10,3) (11,1) (19,1) (20,4) (21,1) (28,1) (29,7) (30,1)): unit 0 joins (the
    /// tone's onset over the encoder's silent history), unit 29 filters 6 windows of eight; no stationary block filters.
    const TNS_UNITS: [(usize, u8); 10] = [(0, 1), (9, 1), (10, 3), (11, 1), (19, 1), (20, 4), (21, 1), (28, 1), (29, 6), (30, 1)];
    /// SR381 9 dB (measured 10.5); AACENCTNSNORM (SR385) measured 11.2 (6.248e7 -> 4.731e6), the floor raised to 10.5.
    const TNS_PRE_ECHO_DB: f64 = 10.5;
    /// SR378's short-block pre-echo (`aac_enc_short_pre_echo`, block switching alone) and the margin this arc keeps under it.
    const PRE_ECHO_SR378: f64 = 4.257e7;
    const PRE_ECHO_VS_SR378_DB: f64 = 20.0;
    /// SR381 (586, 160); AACENCTNSNORM (SR385): (566, 168), the burst's neighbours now filtered by the normalised gate.
    const RESERVOIR_BYTES: (usize, usize) = (566, 168);
    /// AACENCTNSMULTI (LEDGER SR390): the tone + noise-burst units whose long window carries two filters.
    /// AACENCPSYPREDDEFAULT (SR396; SR390's [0, 9, 11, 19, 28] under the flatness tonality): unit 9's NMR trial now keeps
    /// the one-filter candidate.
    const TWO_FILTER_UNITS: [usize; 4] = [0, 11, 19, 28];

    /// A tone (440 Hz + 2 harmonics) with 512-sample white-noise bursts (abrupt onset) at `BURSTS`, mono.
    fn tone_bursts(frames: usize) -> Vec<i16> {
        let mut v = Vec::with_capacity(frames);
        let mut a = 0x2468_ace1u32;
        for n in 0..frames {
            let p = 2.0 * core::f64::consts::PI * 440.0 * n as f64 / 48000.0;
            let mut x = 3000.0 * math::sin(p) + 1500.0 * math::sin(2.0 * p) + 750.0 * math::sin(3.0 * p);
            if BURSTS.iter().any(|&b| n >= b && n < b + 512) {
                a = a.wrapping_mul(1664525).wrapping_add(1013904223);
                x += ((a >> 8) % 24001) as f64 - 12000.0;
            }
            v.push(x as i16);
        }
        v
    }

    /// The tone + bursts at 64 kb/s mono, TNS on or off, fed a frame at a time: (units, the windows TNS filtered per
    /// unit of the encode phase, the error energy in [p - 768, p - 64) summed over the bursts, the whole signal's SNR).
    fn tns_run(tns: bool) -> (Vec<AccessUnit>, Vec<u8>, f64, f64) {
        let frames = 40960;
        let pcm = tone_bursts(frames);
        let mut e = Encoder::new(48000, 1, 64000).unwrap();
        e.tns = tns;
        let mut units = Vec::new();
        let mut filt = Vec::new();
        for f in pcm.chunks(1024) {
            for u in e.encode(f) { units.push(u); filt.push(e.tns_windows); }
        }
        units.extend(e.flush());
        let d = decode(&e, &units);
        let mut pre = 0f64;
        for &p in &BURSTS { for i in p - 768..p - 64 { let r = pcm[i] as f64 - d[0][i + PRIMING as usize]; pre += r * r; } }
        let (mut s, mut n) = (0f64, 0f64);
        for i in 0..frames {
            let x = pcm[i] as f64;
            let r = x - d[0][i + PRIMING as usize];
            s += x * x;
            n += r * r;
        }
        (units, filt, pre, 10.0 * math::log10(s / n.max(1e-9)))
    }

    /// The silence clicks (`bursts`, mono 64 kb/s, block switching on) at the FIXED per-frame budget (reservoir off),
    /// TNS on or off: the decoded energy in [p - 768, p - 64) summed over the bursts (the input is silence there).
    fn tns_clicks(tns: bool) -> f64 {
        let frames = 40960;
        let pcm = bursts(frames, &BURSTS, 1);
        let mut e = Encoder::new(48000, 1, 64000).unwrap();
        e.tns = tns;
        e.reservoir = false;
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let d = decode(&e, &units);
        let mut pre = 0f64;
        for &p in &BURSTS { for i in p - 768..p - 64 { pre += d[0][i + PRIMING as usize] * d[0][i + PRIMING as usize]; } }
        pre
    }

    /// TNS on the bursts: on the tone + noise-burst stream only the blocks around the bursts and the tone's onset filter
    /// (the per-unit record PINNED, `TNS_UNITS`; stationary tonal blocks carry none: AACENCTNSNORM's normalised gate) and the stream decodes through the
    /// decoder's TNS synthesis; at the fixed budget TNS lowers the silence clicks' pre-echo by at least `TNS_PRE_ECHO_DB`.
    #[test]
    fn aac_enc_tns_burst() {
        let (units, filt, pre_on, snr_on) = tns_run(true);
        let (units_off, filt_off, pre_off, snr_off) = tns_run(false);
        assert_eq!(units.len(), units_off.len());
        assert!(filt_off.iter().all(|&f| f == 0));
        let on: Vec<(usize, u8)> = filt.iter().copied().enumerate().filter(|&(_, f)| f > 0).collect();
        let (c_on, c_off) = (tns_clicks(true), tns_clicks(false));
        let db = 10.0 * math::log10(c_off / c_on.max(1e-9));
        std::println!("aac_enc_tns_burst: tns units {on:?}; tone+bursts error before the bursts tns {pre_on:.3e} off {pre_off:.3e}, SNR tns {snr_on:.1} off {snr_off:.1} dB; clicks at the fixed budget tns {c_on:.3e} off {c_off:.3e} ({db:.1} dB lower)");
        for u in [10usize, 20, 29] { assert_eq!(wire_seq(&units[u]), 2); assert!(filt[u] > 0, "unit {u}: no TNS"); }
        // AACENCTNSNORM (SR385): the gate is the band-normalised gain, no longer the block type: the one ONLY_LONG unit
        // that filters is unit 0, the tone's onset; the stationary tonal blocks carry none
        for &(u, _) in &on { assert!(u == 0 || wire_seq(&units[u]) != 0, "unit {u}: TNS on a stationary ONLY_LONG block"); }
        assert_eq!(on, TNS_UNITS.to_vec());
        assert!(db >= TNS_PRE_ECHO_DB, "pre-echo only {db:.1} dB lower");
    }

    /// The bit reservoir: silence, then one burst (mono 64 kb/s). The silent frames bank their budget, so the burst's
    /// EIGHT_SHORT unit spends more than one frame's bits (within the AU max) — PINNED against the reservoir off — and
    /// the stream stays within the bitrate.
    #[test]
    fn aac_enc_reservoir() {
        let run = |res: bool| -> (Vec<AccessUnit>, usize) {
            let frames = 16384;
            let pcm = bursts(frames, &[10000], 1);
            let mut e = Encoder::new(48000, 1, 64000).unwrap();
            e.reservoir = res;
            let mut units = e.encode(&pcm);
            units.extend(e.flush());
            (units, e.budget)
        };
        let (on, budget) = run(true);
        let (off, _) = run(false);
        let u = (10000 - 512) / 1024 + 1;
        assert_eq!((wire_seq(&on[u]), wire_seq(&off[u])), (2, 2));
        let (b_on, b_off) = (on[u].data.len(), off[u].data.len());
        let total: usize = on.iter().map(|u| u.data.len()).sum();
        let cap = 64000 * on.len() * 1024 / 48000 / 8;
        std::println!("aac_enc_reservoir: burst unit {u} {b_on} bytes (reservoir off {b_off}), budget {} bytes a frame; stream {total} bytes, the bitrate's {cap}", budget / 8);
        assert!(b_off * 8 <= budget + FRAME_OVERHEAD + MS_MASK_MAX, "off: {b_off} bytes");
        assert!(b_on * 8 > budget + FRAME_OVERHEAD, "on: {b_on} bytes");
        assert!(b_on * 8 <= AU_MAX_BITS, "on: {b_on} bytes over the AU max");
        assert_eq!((b_on, b_off), RESERVOIR_BYTES);
        assert!(total <= cap, "{total} bytes, the bitrate's {cap}");
    }
    // ---- AACENCTNSNORM (LEDGER SR385): the band-normalised TNS gate, the opt-in unpredictability tonality

    /// The tone + noise-burst stream (64 kb/s mono) with the TNS detector's switches: the windows filtered per unit of
    /// the encode phase.
    fn tns_filtered(norm: bool, gate: bool) -> (Vec<AccessUnit>, Vec<u8>) {
        let pcm = tone_bursts(40960);
        let mut e = Encoder::new(48000, 1, 64000).unwrap();
        e.tns_norm = norm;
        e.tns_gate = gate;
        let mut units = Vec::new();
        let mut filt = Vec::new();
        for f in pcm.chunks(1024) {
            for u in e.encode(f) { units.push(u); filt.push(e.tns_windows); }
        }
        (units, filt)
    }

    /// The 1 kHz sine at 128 kb/s stereo (`aac_enc_sine_snr`'s fixture) through `set`: per-channel SNR and the bytes.
    fn sine_with(set: impl Fn(&mut Encoder)) -> ([f64; 2], usize) {
        let frames = 48000;
        let pcm = sine_pcm(48000, frames, 1000.0, 16000.0);
        let mut e = Encoder::new(48000, 2, 128000).unwrap();
        set(&mut e);
        let mut units = e.encode(&pcm);
        units.extend(e.flush());
        let d = decode(&e, &units);
        let mut snr = [0f64; 2];
        for c in 0..2 {
            let (mut s, mut n) = (0f64, 0f64);
            for i in 2048..frames - 2048 {
                let x = pcm[i * 2 + c] as f64;
                let y = d[c][i + PRIMING as usize];
                s += x * x;
                n += (x - y) * (x - y);
            }
            snr[c] = 10.0 * math::log10(s / n.max(1e-9));
        }
        (snr, units.iter().map(|u| u.data.len()).sum())
    }

    /// The band-normalised gate (3GPP TS 26.403's detection): on the tone + noise-burst stream the RAW detector with no
    /// attack gate (`tns_norm` and `tns_gate` off: what SR381 had to gate) filters every stationary tonal block — the
    /// 440 Hz tone's leakage reads as a predictable spectrum (raw gains 1.7-4.2) — and the normalised one filters none of
    /// them (1.1-1.9, under `TNS_GAIN_NORM`), while every burst block filters under both. On the 1 kHz sine the attack
    /// gate's retirement costs nothing (the arc's rule: keep the gate if it cost more than 0.3 dB; measured +0.6 dB).
    #[test]
    fn aac_enc_tns_norm() {
        let (units, norm) = tns_filtered(true, false);
        let (_, raw) = tns_filtered(false, false);
        let bursts = [9usize, 10, 11, 19, 20, 21, 28, 29, 30];
        let tonal: Vec<usize> = (1..units.len()).filter(|u| !bursts.contains(u) && wire_seq(&units[*u]) == 0).collect();
        let raw_on = tonal.iter().filter(|&&u| raw[u] > 0).count();
        let norm_on = tonal.iter().filter(|&&u| norm[u] > 0).count();
        let (sine, sine_b) = sine_with(|_| {});
        let (sine_381, sine_381_b) = sine_with(|e| { e.tns_norm = false; e.tns_gate = true; });
        std::println!("aac_enc_tns_norm: stationary tonal units {} filtered raw {raw_on} normalised {norm_on}; burst units raw {:?} normalised {:?}; sine {:.1}/{:.1} dB {sine_b} B (SR381 arm {:.1}/{:.1} dB {sine_381_b} B)",
            tonal.len(), bursts.map(|u| raw[u]), bursts.map(|u| norm[u]), sine[0], sine[1], sine_381[0], sine_381[1]);
        assert!(tonal.len() >= 25, "{} stationary units", tonal.len());
        assert_eq!(raw_on, tonal.len(), "the raw detector filtered {raw_on} of {} stationary tonal units", tonal.len());
        assert_eq!(norm_on, 0, "the normalised detector filtered {norm_on} stationary tonal units");
        for u in bursts { assert!(raw[u] > 0 && norm[u] > 0, "burst unit {u}: raw {} normalised {}", raw[u], norm[u]); }
        for c in 0..2 { assert!(sine[c] >= sine_381[c] - 0.3, "channel {c}: {:.1} dB, the SR381 arm {:.1} dB", sine[c], sine_381[c]); }
    }

    /// AACENCPSYPREDDEFAULT (LEDGER SR396): `Psy::Pred`, THE DEFAULT — the complex unpredictability over the MDCT and its
    /// MDST. (i) `mdst` (the ONE transform on the reversed block) against the direct sine sum; (ii) the phase claim: the
    /// 1 kHz tone's band (its MDCT magnitudes cycle every 3 blocks) reads tonality >= 0.99 on three consecutive long
    /// blocks, where SR385's magnitude-only rule (r̂ = 2|X1| - |X2|, here test-local) read < 0.7 on each (measured
    /// 1.00 / 1.00 / 1.00 against 0.00 / 0.61 / 0.00); (iii) against the `Psy::Flatness` switch the sine is kept or
    /// beaten, the correlated CPE within the bitrate at SR378's floor, the M/S pair no worse (measured: sine 56.95/57.54
    /// against 56.88/57.50, CPE 4.96 at 8419 bytes of 8533 against 4.97, M/S 3.11 against 2.98 dB).
    #[test]
    fn aac_enc_psy_pred() {
        let e = Encoder::new(48000, 1, 64000).unwrap();
        // (i) the MDST
        let z: Vec<f32> = (0..2048).map(|n| (math::sin(n as f64 * 0.37) * 1000.0 + math::cos(n as f64 * 0.011) * 300.0) as f32).collect();
        let m = e.mdst(&z);
        let (mut worst, mut peak) = (0f64, 0f64);
        for k in (0..1024).step_by(37) {
            let mut s = 0f64;
            for n in 0..2048 {
                s += z[n] as f64 * math::sin(2.0 * core::f64::consts::PI / 2048.0 * (n as f64 + 0.5 + 512.0) * (k as f64 + 0.5));
            }
            worst = worst.max((2.0 * s - m[k] as f64).abs());
            peak = peak.max((2.0 * s).abs());
        }
        assert!(worst <= peak * 1e-4, "mdst worst {worst} peak {peak}");
        // (ii) the 1 kHz tone's band over three consecutive long blocks
        let pcm = sine_pcm(48000, 8192, 1000.0, 16000.0);
        let mono: Vec<f32> = pcm.iter().step_by(2).map(|&v| v as f32).collect();
        let cplx = |t: usize| -> Vec<f32> {
            let (x, s) = e.analyse(ONLY_LONG, &mono[t * 1024..t * 1024 + 2048], true);
            let mut v = x;
            v.extend_from_slice(&s.unwrap());
            v
        };
        let swb = t::SWB_1024[e.sf_index()];
        let band = (0..t::NUM_SWB_1024[e.sf_index()] as usize).find(|&b| swb[b + 1] as usize > 43).unwrap(); // 1 kHz: bin 42.7
        let (lo, hi) = (swb[band] as usize, swb[band + 1] as usize);
        let magnitude = |x: &[f32], p1: &[f32], p2: &[f32]| -> f64 {
            let (mut cw, mut ew) = (0f64, 0f64);
            for i in lo..hi {
                let r = (x[i] as f64).abs();
                let rh = 2.0 * (p1[i] as f64).abs() - (p2[i] as f64).abs();
                cw += (r - rh).abs() / (r + rh.abs() + 1e-9) * r * r;
                ew += r * r;
            }
            (PRED_A - PRED_B * math::ln((cw / ew).max(1e-9))).clamp(0.0, 1.0)
        };
        let mut tone = [(0f64, 0f64); 3];
        for (k, t0) in (0..3).enumerate() {
            let (z0, z1, z2) = (cplx(t0 + 2), cplx(t0 + 1), cplx(t0));
            tone[k] = (chaos_tonality(&z0, &z1, &z2, lo, hi), magnitude(&z0, &z1, &z2));
        }
        // (iii) the floors against the switch
        let (sine, sine_b) = sine_with(|e| e.psy = Psy::Pred);
        let (flat, flat_b) = sine_with(|e| e.psy = Psy::Flatness);
        let (u, bytes, cpe) = correlated_psy(true, true, Psy::Pred);
        let (_, _, ms) = correlated_psy(true, false, Psy::Pred);
        let (_, _, ms_flat) = correlated_psy(true, false, Psy::Flatness);
        let cap = 128000 * u.len() * 1024 / 48000 / 8;
        std::println!("aac_enc_psy_pred: mdst worst {worst:.3e} peak {peak:.3e}; 1 kHz band {band} tonality complex {:.2}/{:.2}/{:.2} magnitude {:.2}/{:.2}/{:.2}; sine {:.2}/{:.2} dB {sine_b} B (flatness {:.2}/{:.2} dB {flat_b} B); correlated one budget {:.2}/{:.2} dB {bytes} bytes, the bitrate's {cap}; M/S {:.2}/{:.2} dB (flatness {:.2}/{:.2})",
            tone[0].0, tone[1].0, tone[2].0, tone[0].1, tone[1].1, tone[2].1, sine[0], sine[1], flat[0], flat[1], cpe[0], cpe[1], ms[0], ms[1], ms_flat[0], ms_flat[1]);
        for (c, m) in tone { assert!(c >= 0.99 && m < 0.7, "tone band: complex {c:.3}, magnitude {m:.3}"); }
        for c in 0..2 { assert!(sine[c] >= flat[c], "channel {c}: Pred {:.2} dB, flatness {:.2}", sine[c], flat[c]); }
        for c in 0..2 { assert!(cpe[c] >= CPE_SR378, "channel {c}: Pred {:.2} dB, SR378 {CPE_SR378}", cpe[c]); }
        assert!(bytes <= cap, "Pred CPE {bytes} bytes, the bitrate's {cap}");
        for c in 0..2 { assert!(ms[c] >= ms_flat[c], "channel {c}: M/S Pred {:.2} dB, flatness {:.2}", ms[c], ms_flat[c]); }
    }

    /// AACENCPSYPREDDEFAULT (LEDGER SR396): `Psy::Flatness`, the switch (SR381-SR390's default): the spectral-flatness
    /// tonality everywhere still holds SR381's sine and SR378's one-budget CPE within the bitrate, and both decode
    /// (measured: sine 56.88/57.50 dB 14270 B, CPE 4.97/4.97 dB 8458 bytes of 8533).
    #[test]
    fn aac_enc_psy_flat() {
        let (sine, sine_b) = sine_with(|e| e.psy = Psy::Flatness);
        let (u, bytes, cpe) = correlated_psy(true, true, Psy::Flatness);
        let cap = 128000 * u.len() * 1024 / 48000 / 8;
        std::println!("aac_enc_psy_flat: sine {:.2}/{:.2} dB {sine_b} B; correlated one budget {:.2}/{:.2} dB {bytes} bytes, the bitrate's {cap}", sine[0], sine[1], cpe[0], cpe[1]);
        for c in 0..2 { assert!(sine[c] >= SINE_SR381[c], "channel {c}: flatness {:.2} dB, SR381 {:.1}", sine[c], SINE_SR381[c]); }
        for c in 0..2 { assert!(cpe[c] >= CPE_SR378, "channel {c}: flatness {:.2} dB, SR378 {CPE_SR378}", cpe[c]); }
        assert!(bytes <= cap, "flatness CPE {bytes} bytes, the bitrate's {cap}");
    }

    // ---- AACENCTNSMULTI (LEDGER SR390): two filters, coef_compress; the PE lend

    /// Mono 48 kHz at `kbps`, the multi candidate on or off, fed a frame at a time: (units, per unit of the encode phase
    /// (filtered windows, active filters, bits coef_compress saved), the whole signal's SNR).
    fn multi_run(pcm: &[i16], kbps: u32, multi: bool) -> (Vec<AccessUnit>, Vec<(u8, u8, usize)>, f64) {
        let mut e = Encoder::new(48000, 1, kbps * 1000).unwrap();
        e.tns_multi = multi;
        let mut units = Vec::new();
        let mut st = Vec::new();
        for f in pcm.chunks(1024) {
            for u in e.encode(f) { units.push(u); st.push((e.tns_windows, e.tns_filters, e.tns_saved)); }
        }
        units.extend(e.flush());
        let d = decode(&e, &units);
        let (mut s, mut n) = (0f64, 0f64);
        for (i, &x) in pcm.iter().enumerate() {
            let r = x as f64 - d[0][i + PRIMING as usize];
            s += x as f64 * x as f64;
            n += r * r;
        }
        (units, st, 10.0 * math::log10(s / n.max(1e-9)))
    }

    /// TNS-MULTI: on the tone + noise bursts (64 kb/s mono) long windows carry two filters (TWO_FILTER_UNITS, pinned),
    /// coef_compress saves bits, the filtered windows stay the base's (TNS_UNITS), and the stream's SNR holds; every
    /// stream decodes through our decoder's TNS synthesis (n_filt 2, coef_compress 1). Off: one uncompressed filter a
    /// window (SR385's stream).
    #[test]
    fn aac_enc_tns_multi() {
        let tb = tone_bursts(40960);
        let (u_on, st_on, snr_on) = multi_run(&tb, 64, true);
        let (u_off, st_off, snr_off) = multi_run(&tb, 64, false);
        assert_eq!(u_on.len(), u_off.len());
        assert_eq!(st_on.iter().map(|s| s.0).collect::<Vec<u8>>(), st_off.iter().map(|s| s.0).collect::<Vec<u8>>());
        assert!(st_off.iter().all(|s| s.1 == s.0 && s.2 == 0));
        let two: Vec<usize> = st_on.iter().enumerate().filter(|(_, s)| s.1 > s.0).map(|(u, _)| u).collect();
        let saved: usize = st_on.iter().map(|s| s.2).sum();
        std::println!("aac_enc_tns_multi: tone+bursts two-filter units {two:?}, coef_compress saved {saved} bits, SNR multi {snr_on:.2} off {snr_off:.2} dB");
        for &u in &two { assert_ne!(wire_seq(&u_on[u]), 2, "unit {u}: two filters on a short window"); }
        assert_eq!(two, TWO_FILTER_UNITS.to_vec());
        assert!(saved > 0, "coef_compress saved nothing");
        assert!(snr_on >= snr_off - 0.5, "tone+bursts: multi {snr_on:.2} dB, off {snr_off:.2} dB");
    }

    /// RES-PE: on the tone + noise bursts (64 kb/s mono) the reservoir lends by perceptual entropy — each burst's attack
    /// blocks get more than the mean frame (measured 384 / 383 / 377 bytes, the mean 170; SR381's flat lend 219 / 231 /
    /// 227), the stationary ONLY_LONG units less (median 143; flat 159), the stream within the bitrate (6832 of 6997).
    #[test]
    fn aac_enc_res_pe() {
        let pcm = tone_bursts(40960);
        let run = |pe: bool| -> (Vec<AccessUnit>, usize) {
            let mut e = Encoder::new(48000, 1, 64000).unwrap();
            e.res_pe = pe;
            let mut units = e.encode(&pcm);
            units.extend(e.flush());
            (units, (e.budget + FRAME_OVERHEAD) / 8)
        };
        let (on, mean) = run(true);
        let (off, _) = run(false);
        let peaks = |u: &[AccessUnit]| -> Vec<usize> {
            BURSTS.iter().map(|&p| { let u0 = p / 1024; (u0 - 1..=u0 + 1).filter(|&i| matches!(wire_seq(&u[i]), 1 | 2)).map(|i| u[i].data.len()).max().unwrap_or(0) }).collect()
        };
        let still = |u: &[AccessUnit]| -> usize {
            let mut v: Vec<usize> = (12..u.len() - 2).filter(|&i| wire_seq(&u[i]) == 0).map(|i| u[i].data.len()).collect();
            v.sort_unstable();
            v[v.len() / 2]
        };
        let (p_on, p_off, s_on, s_off) = (peaks(&on), peaks(&off), still(&on), still(&off));
        let total: usize = on.iter().map(|u| u.data.len()).sum();
        let total_off: usize = off.iter().map(|u| u.data.len()).sum();
        let cap = 64000 * on.len() * 1024 / 48000 / 8;
        std::println!("aac_enc_res_pe: mean frame {mean} bytes; burst peaks PE {p_on:?} flat {p_off:?}; stationary median PE {s_on} flat {s_off}; stream PE {total} flat {total_off}, the bitrate's {cap}");
        for &b in &p_on { assert!(b > mean, "burst unit {b} bytes, the mean {mean}"); }
        assert!(s_on < mean, "stationary median {s_on} bytes, the mean {mean}");
        assert!(total <= cap, "{total} bytes, the bitrate's {cap}");
    }
}
