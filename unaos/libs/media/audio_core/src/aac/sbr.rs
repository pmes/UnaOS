// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core (audio_core). SBR, spectral band replication (ISO/IEC 14496-3 §4.6.18), the
//! HE-AAC tool: the AAC-LC core decodes the lower half of the band at half the output rate; this module rebuilds
//! the upper half from it and the SBR side data, and doubles the rate (HEAACSBR, LEDGER SR198).
//!
//! * [`Sbr::read`] — sbr_extension_data (§4.4.2.8 Tables 4.62-4.74): sbr_header (a changed header rebuilds the
//!   frequency band tables, §4.6.18.3), sbr_single/channel_pair_element: sbr_grid, sbr_dtdf, sbr_invf,
//!   sbr_envelope, sbr_noise, sinusoidal coding, and sbr_extension: parametric stereo (bs_extension_id 2) sets
//!   `ps_seen` and, with PS synthesis on (`Sbr::new`'s `ps`), is parsed by [`Ps::read`] (HEAACSBR-PS, LEDGER SR214).
//! * Parametric stereo (ISO/IEC 14496-3 §8.6.4, the file's tail): on a mono core the PS stage runs on the X matrix
//!   after HF adjustment and before synthesis — hybrid analysis, decorrelation, IID/ICC/IPD/OPD mixing, hybrid
//!   synthesis — and the right channel goes through the same 64-band synthesis on a second channel's delay line.
//! * [`Sbr::apply`] — per channel: the 32-band analysis QMF bank (§4.6.18.4.1), HF generation with the inverse
//!   filtering chirp (§4.6.18.6), HF adjustment: mapping, envelope estimation, gain with the limiter and boost,
//!   smoothing, noise floor and sinusoids (§4.6.18.7), then the 64-band synthesis QMF bank (§4.6.18.4.2).
//!   An element (or LFE, or frame) without SBR data runs analysis -> synthesis with no high band: a plain 2x
//!   upsampling with the same delay, so every channel of the stream stays aligned.
//!
//! Written from the specification's equations. FFmpeg 6.1's aacsbr (LGPL-2.1-or-later) was the cross-reference
//! for the order of the per-frame state updates (what is carried from frame to frame) and, decoded to FLAC, is the
//! KAT's reference (tests/aac_kat.rs `heaac_vs_ffmpeg`); its code is not ported. Both QMF banks evaluate the spec's
//! sums through the crate's one complex FFT (`crate::vorbis::Imdct::fft`) — the identities are derived above
//! `analysis` and `synthesis` and held to the direct sums by `tests::qmf_matches_the_definition`.
//! Float throughout; the spec's 16-bit sample scale is applied at the analysis input and removed at the synthesis
//! output (the envelope energies are coded on that scale). 1024-sample frames only (960: owed, HEAACSBR-DS).
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use super::decoder::Huff;
use super::tables as t;
use crate::bits::BitReader;
use crate::math;
use crate::vorbis::Imdct;
use crate::{Error, Result};

/// QMF slots per frame (numTimeSlots 16 x RATE 2, 1024-sample frames).
const SLOTS: usize = 32;
/// t_HFGen: lowband slots carried over from the previous frame.
const T_HFGEN: usize = 8;
/// t_HFAdj: the HF adjustment's slot offset into X_high.
const T_HFADJ: usize = 2;
const NOISE_FLOOR_OFFSET: i32 = 6;
/// The spec's sample scale (the envelope energies are coded for 16-bit PCM).
const SCALE: f32 = 32768.0;
/// Upper bound of the master / high table length (k2 - k0 <= 48).
const MAXB: usize = 49;
/// Synthesis delay line: 1280 taps, kept in a buffer twice the moving part (one copy every nine slots).
const SYN_BUF: usize = (1280 - 128) * 2;

// codebook indices (the tables' order)
const T_ENV_15: usize = 0;
const F_ENV_15: usize = 1;
const T_ENV_BAL_15: usize = 2;
const F_ENV_BAL_15: usize = 3;
const T_ENV_30: usize = 4;
const F_ENV_30: usize = 5;
const T_ENV_BAL_30: usize = 6;
const F_ENV_BAL_30: usize = 7;
const T_NOISE_30: usize = 8;
const T_NOISE_BAL_30: usize = 9;
/// largest absolute value of each codebook (symbol index i is the value i - LAV)
const LAV: [i32; 10] = [60, 60, 24, 24, 31, 31, 12, 12, 31, 12];

const FIXFIX: u32 = 0;
const FIXVAR: u32 = 1;
const VARFIX: u32 = 2;
const VARVAR: u32 = 3;

type Cx = [f32; 2];

/// The header fields whose change rebuilds the frequency band tables (§4.6.18.3, `bs_header_flag` reset rule).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Spectrum {
    start_freq: u8,
    stop_freq: u8,
    xover_band: u8,
    freq_scale: u8,
    alter_scale: u8,
    noise_bands: u8,
}

/// One SCE/CPE's SBR header and the frequency band tables derived from it.
#[derive(Clone)]
struct Elem {
    /// A valid header was read and the tables built: SBR data is decoded and applied.
    start: bool,
    /// This frame carried sbr_data for the element.
    have_data: bool,
    spectrum: Option<Spectrum>,
    amp_res_header: u8,
    limiter_bands: u8,
    limiter_gains: u8,
    interpol_freq: bool,
    smoothing_mode: bool,
    /// The header changed this frame (the tables were rebuilt; the smoothing history restarts).
    reset: bool,
    coupling: bool,
    /// [previous frame, this frame]
    kx: [usize; 2],
    m: [usize; 2],
    /// `read` already moved this frame's kx/M to the previous slot.
    pushed: bool,
    k0: usize,
    n_master: usize,
    f_master: [usize; MAXB],
    /// [low, high] resolution band counts
    n: [usize; 2],
    f_high: [usize; MAXB],
    f_low: [usize; MAXB],
    n_q: usize,
    f_noise: [usize; 6],
    n_lim: usize,
    f_lim: [usize; MAXB + 8],
    num_patches: usize,
    patch_num: [usize; 6],
    patch_start: [usize; 6],
}

impl Elem {
    fn new() -> Elem {
        Elem {
            start: false, have_data: false, spectrum: None, amp_res_header: 0, limiter_bands: 2, limiter_gains: 2,
            interpol_freq: true, smoothing_mode: true, reset: false, coupling: false, kx: [32, 32], m: [0, 0],
            pushed: false, k0: 0, n_master: 0, f_master: [0; MAXB], n: [0; 2], f_high: [0; MAXB], f_low: [0; MAXB],
            n_q: 0, f_noise: [0; 6], n_lim: 0, f_lim: [0; MAXB + 8], num_patches: 0, patch_num: [0; 6], patch_start: [0; 6],
        }
    }
    /// Pure upsampling until the next valid header (kx' = 32, M' = 0).
    fn turnoff(&mut self) {
        self.start = false;
        self.have_data = false;
        self.kx[1] = 32;
        self.m[1] = 0;
        self.spectrum = None;
    }
}

/// One output channel's SBR state: the filterbank delay lines and what the next frame's decode reads back.
struct Chan {
    /// analysis input, chronological: 288 carried samples + the frame's 1024
    ana: Vec<f32>,
    syn: Vec<f32>,
    syn_off: usize,
    /// the last t_HFGen analysis slots of the previous frame
    w_prev: Vec<[Cx; 32]>,
    /// Y of [two buffers] x 38 slots x 64 bands; `y_cur` is this frame's
    y: [Vec<[Cx; 64]>; 2],
    y_cur: usize,
    // the frame grid
    num_env: usize,
    num_noise: usize,
    frame_class: u32,
    amp_res: u8,
    t_env: [usize; 7],
    t_q: [usize; 3],
    /// [0] = the previous frame's last
    freq_res: [u8; 7],
    /// [l_APrev flag, l_A]
    e_a: [i32; 2],
    t_env_num_env_old: usize,
    df_env: [bool; 6],
    df_noise: [bool; 2],
    /// [this frame, previous frame]
    invf: [[u8; 5]; 2],
    env_q: [[i32; MAXB]; 7],
    noise_q: [[i32; 5]; 3],
    env: [[f32; MAXB]; 7],
    noise: [[f32; 5]; 3],
    add_harmonic_flag: bool,
    add_harmonic: [bool; MAXB],
    s_indexmapped: [[u8; MAXB]; 7],
    bw: [f32; 5],
    g_temp: Vec<[f32; MAXB]>,
    q_temp: Vec<[f32; MAXB]>,
    index_noise: usize,
    index_sine: usize,
}

impl Chan {
    fn new() -> Chan {
        Chan {
            ana: vec![0.0; 288 + 1024],
            syn: vec![0.0; SYN_BUF],
            syn_off: SYN_BUF - (1280 - 128),
            w_prev: vec![[[0.0; 2]; 32]; T_HFGEN],
            y: [vec![[[0.0; 2]; 64]; 38], vec![[[0.0; 2]; 64]; 38]],
            y_cur: 0,
            num_env: 0, num_noise: 0, frame_class: 0, amp_res: 0, t_env: [0; 7], t_q: [0; 3], freq_res: [0; 7],
            e_a: [-1, -1], t_env_num_env_old: 0, df_env: [false; 6], df_noise: [false; 2], invf: [[0; 5]; 2],
            env_q: [[0; MAXB]; 7], noise_q: [[0; 5]; 3], env: [[0.0; MAXB]; 7], noise: [[0.0; 5]; 3],
            add_harmonic_flag: false, add_harmonic: [false; MAXB], s_indexmapped: [[0; MAXB]; 7], bw: [0.0; 5],
            g_temp: vec![[0.0; MAXB]; 42], q_temp: vec![[0.0; MAXB]; 42], index_noise: 0, index_sine: 0,
        }
    }
}

/// Per-frame working arrays (shared by the channels in turn).
struct Scratch {
    w: Vec<[Cx; 32]>,
    x_low: Vec<[Cx; 40]>,
    x_high: Vec<[Cx; 40]>,
    alpha0: [Cx; 64],
    alpha1: [Cx; 64],
    e_orig: [[f32; MAXB]; 6],
    q_mapped: [[f32; MAXB]; 6],
    s_mapped: [[bool; MAXB]; 6],
    e_curr: [[f32; MAXB]; 6],
    q_m: [[f32; MAXB]; 6],
    s_m: [[f32; MAXB]; 6],
    gain: [[f32; MAXB]; 6],
    x_re: Vec<[f32; 64]>,
    x_im: Vec<[f32; 64]>,
    /// HEAACSBR-PS: the right channel's X (the PS stage's second output)
    x2_re: Vec<[f32; 64]>,
    x2_im: Vec<[f32; 64]>,
}

pub(crate) struct Sbr {
    /// fs of the SBR output (twice the core's)
    rate: u32,
    elems: Vec<Elem>,
    chans: Vec<Chan>,
    books: Vec<Huff>,
    fft64: Imdct,
    fft128: Imdct,
    /// analysis: pre-twiddle e^{-i pi n/64} (64), post-twiddle 2 SCALE e^{-i pi (k+1/2)/128} (32)
    ana_pre: Vec<Cx>,
    ana_post: Vec<Cx>,
    /// synthesis: pre-twiddle e^{-i 255 pi (k+1/2)/128} (64), post (cos, sin)(pi n/128) / (64 SCALE) (128)
    syn_pre: Vec<Cx>,
    syn_post: Vec<Cx>,
    s: Box<Scratch>,
    /// A parametric stereo extension was seen.
    pub(crate) ps_seen: bool,
    /// HEAACSBR-PS (LEDGER SR214): the parametric stereo state when PS synthesis is on (a mono core, output channel 0;
    /// the right channel's synthesis delay line is the last `chans` entry).
    ps: Option<Box<Ps>>,
}

fn cis(a: f64) -> Cx { let (s, c) = math::sincos(a); [c as f32, s as f32] }

/// lrintf: round to nearest, ties to even.
fn lrint(x: f32) -> i32 {
    let f = math::floor(x as f64);
    let d = x as f64 - f;
    let mut r = f as i32;
    if d > 0.5 || (d == 0.5 && r & 1 == 1) { r += 1; }
    r
}

fn exp2i(x: i32) -> f32 { math::ldexp(1.0, x) as f32 }

/// §4.6.18.3.2: num_bands band widths between start and stop, geometric, rounded (fMaster helper).
fn make_bands(bands: &mut [i32], start: usize, stop: usize, num: usize) {
    let base = math::pow(stop as f64 / start as f64, 1.0 / num as f64) as f32;
    let mut prod = start as f32;
    let mut prev = start as i32;
    for b in bands.iter_mut().take(num - 1) {
        prod *= base;
        let present = lrint(prod);
        *b = present - prev;
        prev = present;
    }
    bands[num - 1] = stop as i32 - prev;
}

fn bad() -> Error { Error::Invalid("aac: SBR data") }

impl Sbr {
    /// SBR for a core decoding at `core_rate` with `channels` output channels; `ps`: parametric stereo synthesis on
    /// a mono core (one more output channel, synthesised from the first).
    pub(crate) fn new(core_rate: u32, channels: usize, ps: bool) -> Sbr {
        let books = (0..10).map(|i| {
            let (c, l): (&[u32], &[u8]) = match i {
                T_ENV_15 => (&t::SBR_T_HUFFMAN_ENV_1_5DB_CODES, &t::SBR_T_HUFFMAN_ENV_1_5DB_BITS),
                F_ENV_15 => (&t::SBR_F_HUFFMAN_ENV_1_5DB_CODES, &t::SBR_F_HUFFMAN_ENV_1_5DB_BITS),
                T_ENV_BAL_15 => (&t::SBR_T_HUFFMAN_ENV_BAL_1_5DB_CODES, &t::SBR_T_HUFFMAN_ENV_BAL_1_5DB_BITS),
                F_ENV_BAL_15 => (&t::SBR_F_HUFFMAN_ENV_BAL_1_5DB_CODES, &t::SBR_F_HUFFMAN_ENV_BAL_1_5DB_BITS),
                T_ENV_30 => (&t::SBR_T_HUFFMAN_ENV_3_0DB_CODES, &t::SBR_T_HUFFMAN_ENV_3_0DB_BITS),
                F_ENV_30 => (&t::SBR_F_HUFFMAN_ENV_3_0DB_CODES, &t::SBR_F_HUFFMAN_ENV_3_0DB_BITS),
                T_ENV_BAL_30 => (&t::SBR_T_HUFFMAN_ENV_BAL_3_0DB_CODES, &t::SBR_T_HUFFMAN_ENV_BAL_3_0DB_BITS),
                F_ENV_BAL_30 => (&t::SBR_F_HUFFMAN_ENV_BAL_3_0DB_CODES, &t::SBR_F_HUFFMAN_ENV_BAL_3_0DB_BITS),
                T_NOISE_30 => (&t::SBR_T_HUFFMAN_NOISE_3_0DB_CODES, &t::SBR_T_HUFFMAN_NOISE_3_0DB_BITS),
                _ => (&t::SBR_T_HUFFMAN_NOISE_BAL_3_0DB_CODES, &t::SBR_T_HUFFMAN_NOISE_BAL_3_0DB_BITS),
            };
            Huff::new(c, l)
        }).collect();
        let pi = core::f64::consts::PI;
        let ana_pre = (0..64).map(|n| cis(-pi * n as f64 / 64.0)).collect();
        let ana_post = (0..32).map(|k| { let c = cis(-pi * (k as f64 + 0.5) / 128.0); [2.0 * SCALE * c[0], 2.0 * SCALE * c[1]] }).collect();
        let syn_pre = (0..64).map(|k| cis(-255.0 * pi * (k as f64 + 0.5) / 128.0)).collect();
        let syn_post = (0..128).map(|n| { let c = cis(pi * n as f64 / 128.0); let g = 1.0 / (64.0 * SCALE); [c[0] * g, c[1] * g] }).collect();
        Sbr {
            rate: core_rate * 2,
            elems: Vec::new(),
            chans: (0..channels + ps as usize).map(|_| Chan::new()).collect(),
            books,
            fft64: Imdct::new(256, [256, 256]),
            fft128: Imdct::new(512, [512, 512]),
            ana_pre, ana_post, syn_pre, syn_post,
            s: Box::new(Scratch {
                w: vec![[[0.0; 2]; 32]; SLOTS],
                x_low: vec![[[0.0; 2]; 40]; 32],
                x_high: vec![[[0.0; 2]; 40]; 64],
                alpha0: [[0.0; 2]; 64],
                alpha1: [[0.0; 2]; 64],
                e_orig: [[0.0; MAXB]; 6],
                q_mapped: [[0.0; MAXB]; 6],
                s_mapped: [[false; MAXB]; 6],
                e_curr: [[0.0; MAXB]; 6],
                q_m: [[0.0; MAXB]; 6],
                s_m: [[0.0; MAXB]; 6],
                gain: [[0.0; MAXB]; 6],
                x_re: vec![[0.0; 64]; 38],
                x_im: vec![[0.0; 64]; 38],
                x2_re: vec![[0.0; 64]; 38],
                x2_im: vec![[0.0; 64]; 38],
            }),
            ps_seen: false,
            ps: if ps { Some(Box::new(Ps::new())) } else { None },
        }
    }

    /// Start of a raw_data_block: no element has SBR data yet.
    pub(crate) fn begin_frame(&mut self) {
        for e in self.elems.iter_mut() { e.have_data = false; }
        if let Some(p) = self.ps.as_mut() { p.read_this_frame = false; }
    }

    fn elem(&mut self, i: usize) -> &mut Elem {
        while self.elems.len() <= i { self.elems.push(Elem::new()); }
        &mut self.elems[i]
    }

    /// sbr_extension_data (Table 4.62) for element `elem` (the SCE or CPE just decoded, output channels `ochs`),
    /// read from `r` after the extension_type nibble; `crc` for EXT_SBR_DATA_CRC. The caller lands on the fill
    /// element's end whatever this consumed. An error leaves the element in pure upsampling.
    pub(crate) fn read(&mut self, r: &mut BitReader, elem: usize, ochs: &[usize], crc: bool) -> Result<()> {
        let res = self.read_inner(r, elem, ochs, crc);
        if res.is_err() {
            self.elem(elem).turnoff();
            for &c in ochs { if let Some(ch) = self.chans.get_mut(c) { ch.e_a[1] = -1; } }
        }
        res
    }

    fn read_inner(&mut self, r: &mut BitReader, elem: usize, ochs: &[usize], crc: bool) -> Result<()> {
        if ochs.iter().any(|&c| c >= self.chans.len()) || ochs.is_empty() || ochs.len() > 2 { return Err(bad()); }
        let rate = self.rate;
        let e = self.elem(elem);
        e.kx[0] = e.kx[1];
        e.m[0] = e.m[1];
        e.pushed = true;
        e.reset = false;
        if crc { r.skip(10)?; }
        if r.bit()? { read_header(e, r)?; }
        if e.reset {
            if make_tables(e, rate).is_err() { e.turnoff(); }
            for &c in ochs { self.chans[c].index_noise = 0; }
        }
        let e = &mut self.elems[elem];
        if !e.start { return Ok(()); }
        let cpe = ochs.len() == 2;
        let mut e = e.clone();
        let res = self.read_data(&mut e, r, ochs, cpe);
        self.elems[elem] = e;
        res?;
        self.elems[elem].have_data = true;
        Ok(())
    }

    /// sbr_single_channel_element / sbr_channel_pair_element (Tables 4.65 / 4.66) and sbr_extended_data.
    fn read_data(&mut self, e: &mut Elem, r: &mut BitReader, ochs: &[usize], cpe: bool) -> Result<()> {
        if !cpe {
            if r.bit()? { r.skip(4)?; }
            let c = ochs[0];
            read_grid(e, &mut self.chans[c], r)?;
            read_dtdf(&mut self.chans[c], r)?;
            read_invf(e, &mut self.chans[c], r)?;
            self.read_envelope(e, c, r, false)?;
            self.read_noise(e, c, r, false)?;
            read_harmonic(e, &mut self.chans[c], r)?;
        } else {
            if r.bit()? { r.skip(8)?; }
            let (a, b) = (ochs[0], ochs[1]);
            e.coupling = r.bit()?;
            if e.coupling {
                read_grid(e, &mut self.chans[a], r)?;
                copy_grid(&mut self.chans, a, b);
                read_dtdf(&mut self.chans[a], r)?;
                read_dtdf(&mut self.chans[b], r)?;
                read_invf(e, &mut self.chans[a], r)?;
                let inv = self.chans[a].invf[0];
                let cb = &mut self.chans[b];
                cb.invf[1] = cb.invf[0];
                cb.invf[0] = inv;
                self.read_envelope(e, a, r, false)?;
                self.read_noise(e, a, r, false)?;
                self.read_envelope(e, b, r, true)?;
                self.read_noise(e, b, r, true)?;
            } else {
                read_grid(e, &mut self.chans[a], r)?;
                read_grid(e, &mut self.chans[b], r)?;
                read_dtdf(&mut self.chans[a], r)?;
                read_dtdf(&mut self.chans[b], r)?;
                read_invf(e, &mut self.chans[a], r)?;
                read_invf(e, &mut self.chans[b], r)?;
                self.read_envelope(e, a, r, false)?;
                self.read_envelope(e, b, r, false)?;
                self.read_noise(e, a, r, false)?;
                self.read_noise(e, b, r, false)?;
            }
            read_harmonic(e, &mut self.chans[a], r)?;
            read_harmonic(e, &mut self.chans[b], r)?;
        }
        // sbr_extended_data: bs_extension_size bytes of (bs_extension_id, payload) pairs
        if r.bit()? {
            let mut cnt = r.read(4)? as usize;
            if cnt == 15 { cnt += r.read(8)? as usize; }
            let mut left = 8 * cnt;
            while left > 7 {
                let id = r.read(2)?;
                left -= 2;
                // EXTENSION_ID_PS: parametric stereo, parsed once per frame when PS synthesis is on, else stepped over
                if id == 2 { self.ps_seen = true; }
                let used = match self.ps.as_mut() { Some(p) if id == 2 && !p.read_this_frame => p.read(r, left), _ => 0 };
                if used == 0 { break; }
                left -= used;
            }
            r.skip(left)?;
        }
        Ok(())
    }

    /// sbr_envelope (Table 4.70): delta-coded scalefactors in frequency (f) or time (t).
    fn read_envelope(&mut self, e: &Elem, c: usize, r: &mut BitReader, balance_ch: bool) -> Result<()> {
        let ch = &mut self.chans[c];
        let coupled = balance_ch && e.coupling;
        let delta = if coupled { 2 } else { 1 };
        let (bits, tb, fb) = match (coupled, ch.amp_res != 0) {
            (true, true) => (5, T_ENV_BAL_30, F_ENV_BAL_30),
            (true, false) => (6, T_ENV_BAL_15, F_ENV_BAL_15),
            (false, true) => (6, T_ENV_30, F_ENV_30),
            (false, false) => (7, T_ENV_15, F_ENV_15),
        };
        let odd = e.n[1] & 1;
        for i in 0..ch.num_env {
            let res = ch.freq_res[i + 1] as usize;
            let nb = e.n[res];
            if ch.df_env[i] {
                let book = &self.books[tb];
                for j in 0..nb {
                    // the previous envelope at the matching band of its own resolution
                    let k = if ch.freq_res[i + 1] == ch.freq_res[i] { j } else if res == 1 { (j + odd) >> 1 } else if j == 0 { 0 } else { 2 * j - odd };
                    let v = ch.env_q[i][k.min(MAXB - 1)] + delta * (book.decode(r)? as i32 - LAV[tb]);
                    if !(0..=127).contains(&v) { return Err(bad()); }
                    ch.env_q[i + 1][j] = v;
                }
            } else {
                let book = &self.books[fb];
                ch.env_q[i + 1][0] = delta * r.read(bits)? as i32;
                for j in 1..nb {
                    let v = ch.env_q[i + 1][j - 1] + delta * (book.decode(r)? as i32 - LAV[fb]);
                    if !(0..=127).contains(&v) { return Err(bad()); }
                    ch.env_q[i + 1][j] = v;
                }
            }
        }
        ch.env_q[0] = ch.env_q[ch.num_env];
        Ok(())
    }

    /// sbr_noise (Table 4.71).
    fn read_noise(&mut self, e: &Elem, c: usize, r: &mut BitReader, balance_ch: bool) -> Result<()> {
        let ch = &mut self.chans[c];
        let coupled = balance_ch && e.coupling;
        let delta = if coupled { 2 } else { 1 };
        let (tb, fb) = if coupled { (T_NOISE_BAL_30, F_ENV_BAL_30) } else { (T_NOISE_30, F_ENV_30) };
        for i in 0..ch.num_noise {
            if ch.df_noise[i] {
                for j in 0..e.n_q {
                    let v = ch.noise_q[i][j] + delta * (self.books[tb].decode(r)? as i32 - LAV[tb]);
                    if !(0..=30).contains(&v) { return Err(bad()); }
                    ch.noise_q[i + 1][j] = v;
                }
            } else {
                ch.noise_q[i + 1][0] = delta * r.read(5)? as i32;
                for j in 1..e.n_q {
                    let v = ch.noise_q[i + 1][j - 1] + delta * (self.books[fb].decode(r)? as i32 - LAV[fb]);
                    if !(0..=30).contains(&v) { return Err(bad()); }
                    ch.noise_q[i + 1][j] = v;
                }
            }
        }
        ch.noise_q[0] = ch.noise_q[ch.num_noise];
        Ok(())
    }

    /// Rebuild the 2x output of one element (`elem`: None for a channel no element carried) on its output
    /// channels: `out[och][0..1024]` is the core's PCM on entry, `out[och][0..2048]` the SBR output on return.
    pub(crate) fn apply(&mut self, elem: Option<usize>, ochs: &[usize], out: &mut [Vec<f32>]) {
        let mut e = match elem { Some(i) => self.elem(i).clone(), None => Elem::new() };
        if !e.pushed { e.kx[0] = e.kx[1]; e.m[0] = e.m[1]; } else { e.pushed = false; }
        if e.start && !e.have_data {
            // §4.6.18: no sbr_data for a started element: back to pure upsampling until the next header
            e.turnoff();
            for &c in ochs { if let Some(ch) = self.chans.get_mut(c) { ch.e_a[1] = -1; } }
        }
        if e.start { self.dequant(&e, ochs); }
        for &c in ochs {
            if c >= self.chans.len() || out.get(c).map(|o| o.len() < 2048).unwrap_or(true) { continue; }
            self.analysis(c, &out[c][..1024]);
            self.lf_gen(&e, c);
            self.chans[c].y_cur ^= 1;
            if e.start {
                self.inverse_filter(e.k0);
                self.chirp(&e, c);
                if self.hf_gen(&e, c) {
                    self.mapping(&e, c);
                    self.env_estimate(&e, c);
                    self.gain_calc(&e, c);
                    self.assemble(&e, c);
                }
            }
            self.x_gen(&e, c);
            if self.ps.is_some() && c == 0 && ochs.len() == 1 && out.get(1).map(|o| o.len() >= 2048).unwrap_or(false) { self.ps_synthesis(&e, out); } else { self.synthesis(c, &mut out[c][..2048]); }
        }
        e.have_data = false;
        e.reset = false;
        if let Some(i) = elem { self.elems[i] = e; }
    }

    // ------------------------------------------------------------------ dequantisation (§4.6.18.3.5)

    fn dequant(&mut self, e: &Elem, ochs: &[usize]) {
        let pow_half = |q: i32| -> f32 { exp2i(q >> 1) * if q & 1 != 0 { core::f32::consts::SQRT_2 } else { 1.0 } };
        if ochs.len() == 2 && e.coupling {
            let (a, b) = (ochs[0], ochs[1]);
            let amp = self.chans[a].amp_res != 0;
            let pan = if amp { 12 } else { 24 };
            for env in 1..=self.chans[a].num_env {
                let nb = e.n[self.chans[a].freq_res[env] as usize];
                for k in 0..nb {
                    let (qa, qb) = (self.chans[a].env_q[env][k], self.chans[b].env_q[env][k]);
                    let (mut t1, t2) = if amp { (exp2i(qa + 7), exp2i(pan - qb)) } else { (pow_half(qa) * exp2i(7), pow_half(pan - qb)) };
                    if t1 > 1e20 { t1 = 1.0; }
                    let fac = t1 / (1.0 + t2);
                    self.chans[a].env[env][k] = fac;
                    self.chans[b].env[env][k] = fac * t2;
                }
            }
            for env in 1..=self.chans[a].num_noise {
                for k in 0..e.n_q {
                    let t1 = exp2i(NOISE_FLOOR_OFFSET - self.chans[a].noise_q[env][k] + 1);
                    let t2 = exp2i(12 - self.chans[b].noise_q[env][k]);
                    let fac = t1 / (1.0 + t2);
                    self.chans[a].noise[env][k] = fac;
                    self.chans[b].noise[env][k] = fac * t2;
                }
            }
        } else {
            for &c in ochs {
                let Some(ch) = self.chans.get_mut(c) else { continue };
                for env in 1..=ch.num_env {
                    let nb = e.n[ch.freq_res[env] as usize];
                    for k in 0..nb {
                        let q = ch.env_q[env][k];
                        let mut v = if ch.amp_res != 0 { exp2i(q + 6) } else { pow_half(q) * exp2i(6) };
                        if v > 1e20 { v = 1.0; }
                        ch.env[env][k] = v;
                    }
                }
                for env in 1..=ch.num_noise {
                    for k in 0..e.n_q { ch.noise[env][k] = exp2i(NOISE_FLOOR_OFFSET - ch.noise_q[env][k]); }
                }
            }
        }
    }

    // ------------------------------------------------------------------ QMF banks (§4.6.18.4)

    /// Analysis, per slot l (§4.6.18.4.1): x = the newest 320 samples (newest first), z[n] = x[n] c[2n],
    /// u[n] = Σ_j z[n + 64j], X[k] = Σ_{n<64} 2 u[n] e^{iπ(k+½)(2n−½)/64}, k < 32.
    /// Identity: (k+½)(2n−½)π/64 = 2πkn/64 + πn/64 − π(k+½)/128, so with F = FFT64(u[n] e^{−iπn/64})
    /// (forward, e^{−2πikn/64}): X[k] = 2 e^{−iπ(k+½)/128} conj(F[k]).
    fn analysis(&mut self, c: usize, input: &[f32]) {
        let ch = &mut self.chans[c];
        ch.ana.copy_within(1024..1024 + 288, 0);
        ch.ana[288..288 + 1024].copy_from_slice(input);
        let (mut re, mut im) = ([0f32; 64], [0f32; 64]);
        for l in 0..SLOTS {
            let x = &ch.ana[32 * l..32 * l + 320];
            let mut u = [0f32; 64];
            for (n, un) in u.iter_mut().enumerate() {
                let mut s = 0.0;
                for j in 0..5 { let i = n + 64 * j; s += x[319 - i] * t::SBR_QMF_C[2 * i]; }
                *un = s;
            }
            for n in 0..64 { re[n] = u[n] * self.ana_pre[n][0]; im[n] = u[n] * self.ana_pre[n][1]; }
            self.fft64.fft(&mut re, &mut im);
            for k in 0..32 {
                let (fr, fi) = (re[k], -im[k]);
                let q = self.ana_post[k];
                self.s.w[l][k] = [fr * q[0] - fi * q[1], fr * q[1] + fi * q[0]];
            }
        }
    }

    /// Synthesis, per slot (§4.6.18.4.2): v shifted by 128, v[n] = (1/64) Σ_{k<64} Re(X[k] e^{iπ(k+½)(2n−255)/128}),
    /// n < 128; g from v (g[128i+j] = v[256i+j], g[128i+64+j] = v[256i+192+j]), w = g·c, out[j] = Σ_{i<10} w[64i+j].
    /// Identity: (k+½)(2n−255)π/128 = 2πkn/128 + πn/128 − 255π(k+½)/128, so with Y[k] = X[k] e^{−i255π(k+½)/128}
    /// (zero for k ≥ 64) and G = FFT128(conj Y): v[n] = (1/64)(cos(πn/128) G_re[n] + sin(πn/128) G_im[n]).
    fn synthesis(&mut self, c: usize, out: &mut [f32]) {
        for l in 0..SLOTS { self.synth_slot(c, l, &mut out[64 * l..64 * l + 64]); }
    }

    /// One synthesis slot: X slot `l` into 64 output samples.
    fn synth_slot(&mut self, c: usize, l: usize, dst: &mut [f32]) {
        let ch = &mut self.chans[c];
        let (mut re, mut im) = ([0f32; 128], [0f32; 128]);
        {
            if ch.syn_off < 128 {
                let keep = 1280 - 128;
                ch.syn.copy_within(0..keep, SYN_BUF - keep);
                ch.syn_off = SYN_BUF - keep - 128;
            } else {
                ch.syn_off -= 128;
            }
            for k in 0..64 {
                let (xr, xi) = (self.s.x_re[l][k], self.s.x_im[l][k]);
                let p = self.syn_pre[k];
                re[k] = xr * p[0] - xi * p[1];
                im[k] = -(xr * p[1] + xi * p[0]);
            }
            self.fft128.fft(&mut re, &mut im);
            let o = ch.syn_off;
            for n in 0..128 {
                let p = self.syn_post[n];
                ch.syn[o + n] = p[0] * re[n] + p[1] * im[n];
            }
            let v = &ch.syn[o..o + 1280];
            for (j, d) in dst.iter_mut().enumerate() {
                let mut s = 0.0;
                for i in 0..5 {
                    s += v[256 * i + j] * t::SBR_QMF_C[128 * i + j];
                    s += v[256 * i + 192 + j] * t::SBR_QMF_C[128 * i + 64 + j];
                }
                *d = s;
            }
        }
    }

    // ------------------------------------------------------------------ HF generation (§4.6.18.6)

    /// X_low: this frame's analysis slots at t_HFGen.., the previous frame's last t_HFGen slots before them.
    fn lf_gen(&mut self, e: &Elem, c: usize) {
        let s = &mut *self.s;
        let ch = &mut self.chans[c];
        for row in s.x_low.iter_mut() { *row = [[0.0; 2]; 40]; }
        for k in 0..e.kx[1].min(32) {
            for i in T_HFGEN..SLOTS + T_HFGEN { s.x_low[k][i] = s.w[i - T_HFGEN][k]; }
        }
        for k in 0..e.kx[0].min(32) {
            for i in 0..T_HFGEN { s.x_low[k][i] = ch.w_prev[i][k]; }
        }
        for i in 0..T_HFGEN { ch.w_prev[i] = s.w[SLOTS - T_HFGEN + i]; }
    }

    /// Inverse filtering (§4.6.18.6.2): the covariance-method prediction coefficients α0, α1 of each low band.
    fn inverse_filter(&mut self, k0: usize) {
        let s = &mut *self.s;
        for k in 0..k0.min(32) {
            let x = &s.x_low[k];
            // phi(i, j) = Σ_{n=0}^{37} x[n+2-i] conj(x[n+2-j]) over the 38 slots ending at slot 39
            let corr = |i: usize, j: usize| -> Cx {
                let (mut a, mut b) = (0f32, 0f32);
                for n in 0..38 {
                    let (p, q) = (x[n + 2 - i], x[n + 2 - j]);
                    a += p[0] * q[0] + p[1] * q[1];
                    b += p[1] * q[0] - p[0] * q[1];
                }
                [a, b]
            };
            let (p01, p02, p11, p12, p22) = (corr(0, 1), corr(0, 2), corr(1, 1)[0], corr(1, 2), corr(2, 2)[0]);
            let d = p22 * p11 - (p12[0] * p12[0] + p12[1] * p12[1]) / (1.0 + 1e-6);
            let mut a1 = [0f32; 2];
            if d != 0.0 {
                a1[0] = (p01[0] * p12[0] - p01[1] * p12[1] - p02[0] * p11) / d;
                a1[1] = (p01[0] * p12[1] + p01[1] * p12[0] - p02[1] * p11) / d;
            }
            let mut a0 = [0f32; 2];
            if p11 != 0.0 {
                a0[0] = -(p01[0] + a1[0] * p12[0] + a1[1] * p12[1]) / p11;
                a0[1] = -(p01[1] + a1[1] * p12[0] - a1[0] * p12[1]) / p11;
            }
            if a1[0] * a1[0] + a1[1] * a1[1] >= 16.0 || a0[0] * a0[0] + a0[1] * a0[1] >= 16.0 { a0 = [0.0; 2]; a1 = [0.0; 2]; }
            s.alpha0[k] = a0;
            s.alpha1[k] = a1;
        }
    }

    /// Chirp factors (§4.6.18.6.2, Table 4.158): bwArray per noise band from bs_invf_mode, smoothed.
    fn chirp(&mut self, e: &Elem, c: usize) {
        const BW: [f32; 4] = [0.0, 0.75, 0.9, 0.98];
        let ch = &mut self.chans[c];
        for i in 0..e.n_q {
            let mut nb = if ch.invf[0][i] + ch.invf[1][i] == 1 { 0.6 } else { BW[ch.invf[0][i] as usize & 3] };
            nb = if nb < ch.bw[i] { 0.75 * nb + 0.25 * ch.bw[i] } else { 0.90625 * nb + 0.09375 * ch.bw[i] };
            ch.bw[i] = if nb < 0.015625 { 0.0 } else { nb };
        }
    }

    /// HF generator (§4.6.18.6.3): each patch copies low bands up, filtered by the chirped prediction.
    /// `false` when a band has no noise band (the frame's adjustment is skipped).
    fn hf_gen(&mut self, e: &Elem, c: usize) -> bool {
        let s = &mut *self.s;
        let ch = &self.chans[c];
        let (start, end) = (2 * ch.t_env[0] + T_HFADJ, (2 * ch.t_env[ch.num_env] + T_HFADJ).min(40));
        let mut k = e.kx[1];
        let mut g: isize = 0;
        for j in 0..e.num_patches {
            for x in 0..e.patch_num[j] {
                let p = e.patch_start[j] + x;
                while g <= e.n_q as isize && k >= e.f_noise[g as usize] { g += 1; }
                g -= 1;
                if g < 0 || k >= 64 || p >= 32 { return false; }
                let bw = ch.bw[g as usize];
                let (a1, a0) = (s.alpha1[p], s.alpha0[p]);
                let al = [a1[0] * bw * bw, a1[1] * bw * bw, a0[0] * bw, a0[1] * bw];
                for i in start..end {
                    let (x2, x1, x0) = (s.x_low[p][i - 2], s.x_low[p][i - 1], s.x_low[p][i]);
                    s.x_high[k][i] = [
                        x2[0] * al[0] - x2[1] * al[1] + x1[0] * al[2] - x1[1] * al[3] + x0[0],
                        x2[1] * al[0] + x2[0] * al[1] + x1[1] * al[2] + x1[0] * al[3] + x0[1],
                    ];
                }
                k += 1;
            }
        }
        while k < (e.kx[1] + e.m[1]).min(64) { s.x_high[k] = [[0.0; 2]; 40]; k += 1; }
        true
    }

    // ------------------------------------------------------------------ HF adjustment (§4.6.18.7)

    /// Mapping (§4.6.18.7.2): envelope, noise floor and sinusoid flags onto QMF bands kx..kx+M.
    fn mapping(&mut self, e: &Elem, c: usize) {
        let s = &mut *self.s;
        let ch = &mut self.chans[c];
        let kx = e.kx[1];
        for row in ch.s_indexmapped.iter_mut().skip(1) { *row = [0; MAXB]; }
        for env in 0..ch.num_env.min(5) {
            let res = ch.freq_res[env + 1] as usize;
            let table = if res == 1 { &e.f_high } else { &e.f_low };
            let ilim = e.n[res];
            for i in 0..ilim {
                for m in table[i]..table[i + 1] { s.e_orig[env][m - kx] = ch.env[env + 1][i]; }
            }
            let kq = usize::from(ch.num_noise > 1 && ch.t_env[env] >= ch.t_q[1]);
            for i in 0..e.n_q {
                for m in e.f_noise[i]..e.f_noise[i + 1] { s.q_mapped[env][m - kx] = ch.noise[kq + 1][i]; }
            }
            if ch.add_harmonic_flag {
                for i in 0..e.n[1] {
                    let mid = (e.f_high[i] + e.f_high[i + 1]) >> 1;
                    let on = ch.add_harmonic[i] && (env as i32 >= ch.e_a[1] || ch.s_indexmapped[0][mid - kx] == 1);
                    ch.s_indexmapped[env + 1][mid - kx] = on as u8;
                }
            }
            for i in 0..ilim {
                let any = (table[i]..table[i + 1]).any(|m| ch.s_indexmapped[env + 1][m - kx] != 0);
                for m in table[i]..table[i + 1] { s.s_mapped[env][m - kx] = any; }
            }
        }
        ch.s_indexmapped[0] = ch.s_indexmapped[ch.num_env.min(6)];
    }

    /// Estimation of the current envelope (§4.6.18.7.3): mean energy of X_high per envelope, per band
    /// (interpolated) or per scalefactor band.
    fn env_estimate(&mut self, e: &Elem, c: usize) {
        let s = &mut *self.s;
        let ch = &self.chans[c];
        let kx = e.kx[1];
        let energy = |row: &[Cx; 40], a: usize, b: usize| -> f32 { row[a..b].iter().map(|v| v[0] * v[0] + v[1] * v[1]).sum() };
        for env in 0..ch.num_env.min(5) {
            let (ilb, iub) = (2 * ch.t_env[env] + T_HFADJ, (2 * ch.t_env[env + 1] + T_HFADJ).min(40));
            let len = (ch.t_env[env + 1] - ch.t_env[env]) as f32 * 2.0;
            if e.interpol_freq {
                for m in 0..e.m[1] { s.e_curr[env][m] = energy(&s.x_high[m + kx], ilb, iub) / len; }
            } else {
                let res = ch.freq_res[env + 1] as usize;
                let table = if res == 1 { &e.f_high } else { &e.f_low };
                for p in 0..e.n[res] {
                    let (lo, hi) = (table[p], table[p + 1]);
                    let sum: f32 = (lo..hi).map(|k| energy(&s.x_high[k], ilb, iub)).sum();
                    let v = sum / (len * (hi - lo) as f32);
                    for k in lo..hi { s.e_curr[env][k - kx] = v; }
                }
            }
        }
    }

    /// Gains (§4.6.18.7.4/.5): levels of noise and sinusoids, the gain per band, limited per limiter band and
    /// compensated by the boost.
    fn gain_calc(&mut self, e: &Elem, c: usize) {
        const LIMGAIN: [f32; 4] = [0.70795, 1.0, 1.41254, 1e10];
        let s = &mut *self.s;
        let ch = &self.chans[c];
        let kx = e.kx[1];
        for env in 0..ch.num_env.min(5) {
            let delta = !(env as i32 == ch.e_a[1] || env as i32 == ch.e_a[0]);
            let dl = if delta { 1.0 } else { 0.0 };
            for k in 0..e.n_lim {
                let (lo, hi) = (e.f_lim[k] - kx, e.f_lim[k + 1] - kx);
                for m in lo..hi {
                    let (eo, qm, ec) = (s.e_orig[env][m], s.q_mapped[env][m], s.e_curr[env][m]);
                    let tmp = eo / (1.0 + qm);
                    s.q_m[env][m] = math::sqrt((tmp * qm) as f64) as f32;
                    s.s_m[env][m] = math::sqrt((tmp * ch.s_indexmapped[env + 1][m] as f32) as f64) as f32;
                    let g = if !s.s_mapped[env][m] { eo / ((1.0 + ec) * (1.0 + qm * dl)) } else { eo * qm / ((1.0 + ec) * (1.0 + qm)) };
                    s.gain[env][m] = math::sqrt(g as f64) as f32 + f32::MIN_POSITIVE;
                }
                let (mut so, mut sc) = (0f32, 0f32);
                for m in lo..hi { so += s.e_orig[env][m]; sc += s.e_curr[env][m]; }
                let gmax = (LIMGAIN[e.limiter_gains as usize & 3] * math::sqrt(((f32::EPSILON + so) / (f32::EPSILON + sc)) as f64) as f32).min(1e5);
                for m in lo..hi {
                    let qmax = s.q_m[env][m] * gmax / s.gain[env][m];
                    s.q_m[env][m] = s.q_m[env][m].min(qmax);
                    s.gain[env][m] = s.gain[env][m].min(gmax);
                }
                let (mut so, mut sc) = (0f32, 0f32);
                for m in lo..hi {
                    so += s.e_orig[env][m];
                    let noise = if delta && s.s_m[env][m] == 0.0 { s.q_m[env][m] * s.q_m[env][m] } else { 0.0 };
                    sc += s.e_curr[env][m] * s.gain[env][m] * s.gain[env][m] + s.s_m[env][m] * s.s_m[env][m] + noise;
                }
                let boost = (math::sqrt(((f32::EPSILON + so) / (f32::EPSILON + sc)) as f64) as f32).min(1.584893192);
                for m in lo..hi {
                    s.gain[env][m] *= boost;
                    s.q_m[env][m] *= boost;
                    s.s_m[env][m] *= boost;
                }
            }
        }
    }

    /// Assembling the HF signal (§4.6.18.7.5): smoothed gains on X_high, plus the noise floor (Table 4.A.90's V)
    /// or the sinusoids, into this frame's Y.
    fn assemble(&mut self, e: &Elem, c: usize) {
        const H_SMOOTH: [f32; 5] = [0.33333333333333, 0.30150283239582, 0.21816949906249, 0.11516383427084, 0.03183050093751];
        const PHI_RE: [f32; 4] = [1.0, 0.0, -1.0, 0.0];
        const PHI_IM: [f32; 4] = [0.0, 1.0, 0.0, -1.0];
        let s = &mut *self.s;
        let ch = &mut self.chans[c];
        let h_sl = if e.smoothing_mode { 0 } else { 4 };
        let (kx, mm) = (e.kx[1], e.m[1]);
        let t0 = 2 * ch.t_env[0];
        if e.reset {
            for i in 0..h_sl {
                ch.g_temp[i + t0][..mm].copy_from_slice(&s.gain[0][..mm]);
                ch.q_temp[i + t0][..mm].copy_from_slice(&s.q_m[0][..mm]);
            }
        } else if h_sl > 0 {
            let old = 2 * ch.t_env_num_env_old;
            for i in 0..4 {
                ch.g_temp[i + t0] = ch.g_temp[i + old];
                ch.q_temp[i + t0] = ch.q_temp[i + old];
            }
        }
        for env in 0..ch.num_env.min(5) {
            for i in 2 * ch.t_env[env]..2 * ch.t_env[env + 1] {
                ch.g_temp[h_sl + i][..mm].copy_from_slice(&s.gain[env][..mm]);
                ch.q_temp[h_sl + i][..mm].copy_from_slice(&s.q_m[env][..mm]);
            }
        }
        let (mut inoise, mut isine) = (ch.index_noise, ch.index_sine);
        let yc = ch.y_cur;
        for env in 0..ch.num_env.min(5) {
            let transient = env as i32 == ch.e_a[0] || env as i32 == ch.e_a[1];
            for i in 2 * ch.t_env[env]..2 * ch.t_env[env + 1] {
                let (mut gf, mut qf) = ([0f32; MAXB], [0f32; MAXB]);
                if h_sl > 0 && !transient {
                    for m in 0..mm {
                        for j in 0..=h_sl {
                            gf[m] += ch.g_temp[i + h_sl - j][m] * H_SMOOTH[j];
                            qf[m] += ch.q_temp[i + h_sl - j][m] * H_SMOOTH[j];
                        }
                    }
                } else {
                    gf[..mm].copy_from_slice(&ch.g_temp[i + h_sl][..mm]);
                    qf[..mm].copy_from_slice(&ch.q_temp[i + h_sl][..mm]);
                }
                let y = &mut ch.y[yc][i];
                let mut sign = if kx & 1 == 0 { 1.0f32 } else { -1.0 };
                for m in 0..mm {
                    let xh = s.x_high[m + kx][i + T_HFADJ];
                    let mut v = [xh[0] * gf[m], xh[1] * gf[m]];
                    inoise = (inoise + 1) & 0x1ff;
                    let sm = s.s_m[env][m];
                    if sm != 0.0 {
                        v[0] += sm * PHI_RE[isine];
                        v[1] += sm * PHI_IM[isine] * sign;
                    } else if !transient {
                        v[0] += qf[m] * t::SBR_NOISE[inoise][0];
                        v[1] += qf[m] * t::SBR_NOISE[inoise][1];
                    }
                    y[m + kx] = v;
                    sign = -sign;
                }
                // the noise index has advanced by M this slot (once per band, noise used or not)
                isine = (isine + 1) & 3;
            }
        }
        ch.index_noise = inoise;
        ch.index_sine = isine;
    }

    /// X (§4.6.18.8): the lowband below kx and this frame's Y above it, with the previous frame's Y in the first
    /// slots its last envelope spilled into.
    fn x_gen(&mut self, e: &Elem, c: usize) {
        let s = &mut *self.s;
        let ch = &self.chans[c];
        let (y0, y1) = (&ch.y[ch.y_cur ^ 1], &ch.y[ch.y_cur]);
        let i_temp = (2 * ch.t_env_num_env_old).saturating_sub(SLOTS).min(SLOTS);
        for l in 0..38 { s.x_re[l] = [0.0; 64]; s.x_im[l] = [0.0; 64]; }
        let (kx0, m0, kx1, m1) = (e.kx[0].min(32), e.m[0], e.kx[1].min(32), e.m[1]);
        for l in 0..i_temp {
            for k in 0..kx0 { let v = s.x_low[k][l + T_HFADJ]; s.x_re[l][k] = v[0]; s.x_im[l][k] = v[1]; }
            for k in kx0..(kx0 + m0).min(64) { let v = y0[l + SLOTS][k]; s.x_re[l][k] = v[0]; s.x_im[l][k] = v[1]; }
        }
        for l in i_temp..38 {
            for k in 0..kx1 { let v = s.x_low[k][l + T_HFADJ]; s.x_re[l][k] = v[0]; s.x_im[l][k] = v[1]; }
        }
        for l in i_temp..SLOTS {
            for k in kx1..(kx1 + m1).min(64) { let v = y1[l][k]; s.x_re[l][k] = v[0]; s.x_im[l][k] = v[1]; }
        }
    }
}

// ------------------------------------------------------------------ bitstream (Tables 4.63-4.69)

/// sbr_header (Table 4.63). Sets `reset` when the band-table fields change.
fn read_header(e: &mut Elem, r: &mut BitReader) -> Result<()> {
    e.start = true;
    let old_limiter = e.limiter_bands;
    e.amp_res_header = r.read(1)? as u8;
    let start_freq = r.read(4)? as u8;
    let stop_freq = r.read(4)? as u8;
    let xover_band = r.read(3)? as u8;
    r.skip(2)?;
    let extra1 = r.bit()?;
    let extra2 = r.bit()?;
    let (freq_scale, alter_scale, noise_bands) = if extra1 { (r.read(2)? as u8, r.read(1)? as u8, r.read(2)? as u8) } else { (2, 1, 2) };
    let sp = Spectrum { start_freq, stop_freq, xover_band, freq_scale, alter_scale, noise_bands };
    if e.spectrum != Some(sp) { e.reset = true; }
    e.spectrum = Some(sp);
    if extra2 {
        e.limiter_bands = r.read(2)? as u8;
        e.limiter_gains = r.read(2)? as u8;
        e.interpol_freq = r.bit()?;
        e.smoothing_mode = r.bit()?;
    } else {
        e.limiter_bands = 2;
        e.limiter_gains = 2;
        e.interpol_freq = true;
        e.smoothing_mode = true;
    }
    if e.limiter_bands != old_limiter && !e.reset && e.n_master > 0 { make_lim(e); }
    Ok(())
}

/// The frequency band tables (§4.6.18.3): f_master, the derived high/low/noise tables, the patches and the
/// limiter table. `Err` for a header the spec's requirements refuse.
fn make_tables(e: &mut Elem, rate: u32) -> Result<()> {
    let sp = e.spectrum.ok_or_else(bad)?;
    let row = match rate {
        16000 => 0,
        22050 => 1,
        24000 => 2,
        32000 => 3,
        44100 | 48000 | 64000 => 4,
        88200 | 96000 => 5,
        _ => return Err(Error::Unsupported("aac: SBR output rate")),
    };
    let tmp: u32 = if rate < 32000 { 3000 } else if rate < 64000 { 4000 } else { 5000 };
    let start_min = (((tmp << 7) + (rate >> 1)) / rate) as usize;
    let stop_min = (((tmp << 8) + (rate >> 1)) / rate) as usize;
    let k0 = (start_min as i32 + t::SBR_OFFSET[row][sp.start_freq as usize] as i32).max(1) as usize;
    let mut k2 = match sp.stop_freq {
        0..=13 => {
            let mut dk = [0i32; 13];
            make_bands(&mut dk, stop_min, 64, 13);
            dk.sort_unstable();
            let s: i32 = dk[..sp.stop_freq as usize].iter().sum();
            (stop_min as i32 + s).max(0) as usize
        }
        14 => 2 * k0,
        _ => 3 * k0,
    };
    k2 = k2.min(64);
    let max_sub = if rate <= 32000 { 48 } else if rate == 44100 { 35 } else { 32 };
    if k2 <= k0 || k2 - k0 > max_sub { return Err(bad()); }
    e.k0 = k0;
    let fm = &mut e.f_master;
    if sp.freq_scale == 0 {
        let dk = sp.alter_scale as usize + 1;
        let n_master = ((k2 - k0 + (dk & 2)) >> dk) << 1;
        if n_master == 0 || sp.xover_band as usize >= n_master || n_master >= MAXB { return Err(bad()); }
        let mut d = [dk as i32; MAXB];
        let k2diff = k2 as i32 - k0 as i32 - (n_master * dk) as i32;
        if k2diff < 0 {
            d[1] -= 1;
            if k2diff < -1 { d[2] -= 1; }
        } else if k2diff > 0 {
            d[n_master] += 1;
        }
        fm[0] = k0;
        for k in 1..=n_master {
            let v = fm[k - 1] as i32 + d[k];
            if v <= 0 { return Err(bad()); }
            fm[k] = v as usize;
        }
        e.n_master = n_master;
    } else {
        let half = 7 - sp.freq_scale as i32;
        let two = 49 * k2 > 110 * k0;
        let k1 = if two { 2 * k0 } else { k2 };
        let nb0 = 2 * lrint(half as f32 * math::log2((k1 as f32 / k0 as f32) as f64) as f32);
        if nb0 <= 0 || nb0 as usize >= MAXB { return Err(bad()); }
        let nb0 = nb0 as usize;
        let mut vk0 = [0i32; MAXB];
        make_bands(&mut vk0[1..], k0, k1, nb0);
        vk0[1..=nb0].sort_unstable();
        let vdk0_max = vk0[nb0];
        vk0[0] = k0 as i32;
        for k in 1..=nb0 {
            if vk0[k] <= 0 { return Err(bad()); }
            vk0[k] += vk0[k - 1];
        }
        if two {
            let warp = if sp.alter_scale != 0 { 0.769_230_77f32 } else { 1.0 };
            let nb1 = 2 * lrint(half as f32 * warp * math::log2((k2 as f32 / k1 as f32) as f64) as f32);
            if nb1 <= 0 || (nb0 + nb1 as usize) >= MAXB { return Err(bad()); }
            let nb1 = nb1 as usize;
            let mut vk1 = [0i32; MAXB];
            make_bands(&mut vk1[1..], k1, k2, nb1);
            let vdk1_min = *vk1[1..=nb1].iter().min().unwrap_or(&0);
            if vdk1_min < vdk0_max {
                vk1[1..=nb1].sort_unstable();
                let change = (vdk0_max - vk1[1]).min((vk1[nb1] - vk1[1]) >> 1);
                vk1[1] += change;
                vk1[nb1] -= change;
            }
            vk1[1..=nb1].sort_unstable();
            vk1[0] = k1 as i32;
            for k in 1..=nb1 {
                if vk1[k] <= 0 { return Err(bad()); }
                vk1[k] += vk1[k - 1];
            }
            let n_master = nb0 + nb1;
            if sp.xover_band as usize >= n_master { return Err(bad()); }
            for k in 0..=nb0 { fm[k] = vk0[k] as usize; }
            for k in 1..=nb1 { fm[nb0 + k] = vk1[k] as usize; }
            e.n_master = n_master;
        } else {
            if sp.xover_band as usize >= nb0 { return Err(bad()); }
            for k in 0..=nb0 { fm[k] = vk0[k] as usize; }
            e.n_master = nb0;
        }
    }
    // derived tables (§4.6.18.3.2)
    let xo = sp.xover_band as usize;
    let n_high = e.n_master - xo;
    let n_low = (n_high + 1) >> 1;
    e.n = [n_low, n_high];
    for k in 0..=n_high { e.f_high[k] = e.f_master[xo + k]; }
    e.m[1] = e.f_high[n_high] - e.f_high[0];
    e.kx[1] = e.f_high[0];
    if e.kx[1] + e.m[1] > 64 || e.kx[1] > 32 { e.kx[1] = 32; e.m[1] = 0; return Err(bad()); }
    e.f_low[0] = e.f_high[0];
    let odd = n_high & 1;
    for k in 1..=n_low { e.f_low[k] = e.f_high[2 * k - odd]; }
    let nq = lrint(sp.noise_bands as f32 * math::log2((k2 as f32 / e.kx[1] as f32) as f64) as f32).max(1) as usize;
    if nq > 5 { return Err(bad()); }
    e.n_q = nq;
    e.f_noise[0] = e.f_low[0];
    let mut tt = 0usize;
    for k in 1..=nq {
        tt += (n_low - tt) / (nq + 1 - k);
        e.f_noise[k] = e.f_low[tt];
    }
    make_patches(e, rate)?;
    make_lim(e);
    Ok(())
}

/// Patch construction (§4.6.18.6.3, Figure 4.48).
fn make_patches(e: &mut Elem, rate: u32) -> Result<()> {
    let (k0, kx, m) = (e.k0, e.kx[1], e.m[1]);
    let mut msb = k0;
    let mut usb = kx;
    let goal = (((1000u32 << 11) + (rate >> 1)) / rate) as usize;
    e.num_patches = 0;
    let mut k = if goal < kx + m { let mut k = 0; while k < e.n_master && e.f_master[k] < goal { k += 1; } k } else { e.n_master };
    let (mut last_k, mut last_msb) = (usize::MAX, usize::MAX);
    let mut sb;
    loop {
        if k == last_k && msb == last_msb { return Err(bad()); }
        last_k = k;
        last_msb = msb;
        let mut odd;
        let mut i = k;
        loop {
            sb = e.f_master[i];
            odd = (sb + k0) & 1;
            if i == 0 || sb <= k0 - 1 + msb - odd { break; }
            i -= 1;
        }
        if e.num_patches > 5 { return Err(bad()); }
        let n = sb.saturating_sub(usb);
        e.patch_num[e.num_patches] = n;
        e.patch_start[e.num_patches] = (k0 as isize - odd as isize - n as isize).max(0) as usize;
        if n > 0 {
            usb = sb;
            msb = sb;
            e.num_patches += 1;
        } else {
            msb = kx;
        }
        if e.f_master[k] < sb + 3 { k = e.n_master; }
        if sb == kx + m { break; }
    }
    if e.num_patches > 1 && e.patch_num[e.num_patches - 1] < 3 { e.num_patches -= 1; }
    Ok(())
}

/// The limiter band table (§4.6.18.3.2 / §4.6.18.7.4): the low table plus the patch borders, thinned to at most
/// bs_limiter_bands per octave.
fn make_lim(e: &mut Elem) {
    if e.limiter_bands == 0 {
        e.f_lim[0] = e.f_low[0];
        e.f_lim[1] = e.f_low[e.n[0]];
        e.n_lim = 1;
        return;
    }
    const WARP: [f32; 3] = [1.327_151_7, 1.185_092_8, 1.119_871_6];
    let lim = WARP[(e.limiter_bands as usize - 1).min(2)];
    let mut borders = [0usize; 7];
    borders[0] = e.kx[1];
    for k in 1..=e.num_patches { borders[k] = borders[k - 1] + e.patch_num[k - 1]; }
    let mut tab: Vec<usize> = e.f_low[..=e.n[0]].to_vec();
    if e.num_patches > 1 { tab.extend_from_slice(&borders[1..e.num_patches]); }
    tab.sort_unstable();
    let is_border = |v: usize| borders[..=e.num_patches].contains(&v);
    // thin in place: out = last kept, in = next candidate
    let mut out = 0usize;
    let mut n_lim = tab.len() - 1;
    let mut inp = 1usize;
    while out < n_lim && inp < tab.len() {
        let (vi, vo) = (tab[inp], tab[out]);
        if vi as f32 >= vo as f32 * lim {
            out += 1; tab[out] = vi; inp += 1;
        } else if vi == vo || !is_border(vi) {
            inp += 1; n_lim -= 1;
        } else if !is_border(vo) {
            tab[out] = vi; inp += 1; n_lim -= 1;
        } else {
            out += 1; tab[out] = vi; inp += 1;
        }
    }
    e.n_lim = n_lim.min(MAXB + 7);
    for k in 0..=e.n_lim { e.f_lim[k] = tab[k]; }
}

/// ceil(log2(n + 1)) for the bs_pointer width
const CEIL_LOG2: [u32; 6] = [0, 1, 2, 2, 3, 3];

/// sbr_grid (Table 4.67): the frame class, the envelope and noise time borders, the frequency resolutions.
fn read_grid(e: &Elem, ch: &mut Chan, r: &mut BitReader) -> Result<()> {
    let old_num = ch.num_env;
    ch.freq_res[0] = ch.freq_res[old_num];
    ch.amp_res = e.amp_res_header;
    ch.t_env_num_env_old = ch.t_env[old_num];
    let mut trail = 16usize;
    let mut pointer = 0u32;
    let class = r.read(2)?;
    let num;
    match class {
        FIXFIX => {
            num = 1usize << r.read(2)?;
            if num > 4 { return Err(bad()); }
            if num == 1 { ch.amp_res = 0; }
            ch.t_env[0] = 0;
            ch.t_env[num] = trail;
            let step = (trail + (num >> 1)) / num;
            for i in 0..num - 1 { ch.t_env[i + 1] = ch.t_env[i] + step; }
            let fr = r.read(1)? as u8;
            for i in 1..=num { ch.freq_res[i] = fr; }
        }
        FIXVAR => {
            trail += r.read(2)? as usize;
            let nrt = r.read(2)? as usize;
            num = nrt + 1;
            ch.t_env[0] = 0;
            ch.t_env[num] = trail;
            for i in 0..nrt {
                let d = 2 * r.read(2)? as usize + 2;
                ch.t_env[num - 1 - i] = ch.t_env[num - i].checked_sub(d).ok_or_else(bad)?;
            }
            pointer = r.read(CEIL_LOG2[num])?;
            for i in 0..num { ch.freq_res[num - i] = r.read(1)? as u8; }
        }
        VARFIX => {
            ch.t_env[0] = r.read(2)? as usize;
            let nrl = r.read(2)? as usize;
            num = nrl + 1;
            ch.t_env[num] = trail;
            for i in 0..nrl { ch.t_env[i + 1] = ch.t_env[i] + 2 * r.read(2)? as usize + 2; }
            pointer = r.read(CEIL_LOG2[num])?;
            for i in 1..=num { ch.freq_res[i] = r.read(1)? as u8; }
        }
        _ => {
            debug_assert_eq!(class, VARVAR);
            ch.t_env[0] = r.read(2)? as usize;
            trail += r.read(2)? as usize;
            let nrl = r.read(2)? as usize;
            let nrt = r.read(2)? as usize;
            num = nrl + nrt + 1;
            if num > 5 { return Err(bad()); }
            ch.t_env[num] = trail;
            for i in 0..nrl { ch.t_env[i + 1] = ch.t_env[i] + 2 * r.read(2)? as usize + 2; }
            for i in 0..nrt {
                let d = 2 * r.read(2)? as usize + 2;
                ch.t_env[num - 1 - i] = ch.t_env[num - i].checked_sub(d).ok_or_else(bad)?;
            }
            pointer = r.read(CEIL_LOG2[num])?;
            for i in 1..=num { ch.freq_res[i] = r.read(1)? as u8; }
        }
    }
    ch.num_env = num;
    ch.frame_class = class;
    if pointer as usize > num + 1 { return Err(bad()); }
    for i in 1..=num { if ch.t_env[i - 1] >= ch.t_env[i] { return Err(bad()); } }
    ch.num_noise = if num > 1 { 2 } else { 1 };
    ch.t_q[0] = ch.t_env[0];
    ch.t_q[ch.num_noise] = ch.t_env[num];
    if ch.num_noise > 1 {
        let p = pointer as usize;
        let idx = if class == FIXFIX { num >> 1 } else if class & 1 == 1 { num - (p.max(2) - 1) } else if p == 0 { 1 } else if p == 1 { num - 1 } else { p - 1 };
        ch.t_q[1] = ch.t_env[idx.min(num)];
    }
    // l_APrev: the previous frame's transient envelope was its last one
    ch.e_a[0] = -((ch.e_a[1] != old_num as i32) as i32);
    ch.e_a[1] = -1;
    if class & 1 == 1 && pointer > 0 {
        ch.e_a[1] = num as i32 + 1 - pointer as i32;
    } else if class == VARFIX && pointer > 1 {
        ch.e_a[1] = pointer as i32 - 1;
    }
    Ok(())
}

/// The coupled channel takes the grid read for the first; what it carries from its own previous frame stays.
fn copy_grid(chans: &mut [Chan], a: usize, b: usize) {
    let (src_num, t_env, t_q, freq_res, amp_res, num_noise, class, ea1) = {
        let s = &chans[a];
        (s.num_env, s.t_env, s.t_q, s.freq_res, s.amp_res, s.num_noise, s.frame_class, s.e_a[1])
    };
    let d = &mut chans[b];
    d.freq_res[0] = d.freq_res[d.num_env];
    d.t_env_num_env_old = d.t_env[d.num_env];
    d.e_a[0] = -((d.e_a[1] != d.num_env as i32) as i32);
    d.freq_res[1..].copy_from_slice(&freq_res[1..]);
    d.t_env = t_env;
    d.t_q = t_q;
    d.num_env = src_num;
    d.amp_res = amp_res;
    d.num_noise = num_noise;
    d.frame_class = class;
    d.e_a[1] = ea1;
}

/// sbr_dtdf (Table 4.68).
fn read_dtdf(ch: &mut Chan, r: &mut BitReader) -> Result<()> {
    for i in 0..ch.num_env { ch.df_env[i] = r.bit()?; }
    for i in 0..ch.num_noise { ch.df_noise[i] = r.bit()?; }
    Ok(())
}

/// sbr_invf (Table 4.69).
fn read_invf(e: &Elem, ch: &mut Chan, r: &mut BitReader) -> Result<()> {
    ch.invf[1] = ch.invf[0];
    for i in 0..e.n_q { ch.invf[0][i] = r.read(2)? as u8; }
    Ok(())
}

/// bs_add_harmonic_flag and the per-band flags.
fn read_harmonic(e: &Elem, ch: &mut Chan, r: &mut BitReader) -> Result<()> {
    ch.add_harmonic_flag = r.bit()?;
    if ch.add_harmonic_flag { for i in 0..e.n[1] { ch.add_harmonic[i] = r.bit()?; } }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn books() -> [(&'static [u32], &'static [u8], usize); 10] {
        [
            (&t::SBR_T_HUFFMAN_ENV_1_5DB_CODES, &t::SBR_T_HUFFMAN_ENV_1_5DB_BITS, 121),
            (&t::SBR_F_HUFFMAN_ENV_1_5DB_CODES, &t::SBR_F_HUFFMAN_ENV_1_5DB_BITS, 121),
            (&t::SBR_T_HUFFMAN_ENV_BAL_1_5DB_CODES, &t::SBR_T_HUFFMAN_ENV_BAL_1_5DB_BITS, 49),
            (&t::SBR_F_HUFFMAN_ENV_BAL_1_5DB_CODES, &t::SBR_F_HUFFMAN_ENV_BAL_1_5DB_BITS, 49),
            (&t::SBR_T_HUFFMAN_ENV_3_0DB_CODES, &t::SBR_T_HUFFMAN_ENV_3_0DB_BITS, 63),
            (&t::SBR_F_HUFFMAN_ENV_3_0DB_CODES, &t::SBR_F_HUFFMAN_ENV_3_0DB_BITS, 63),
            (&t::SBR_T_HUFFMAN_ENV_BAL_3_0DB_CODES, &t::SBR_T_HUFFMAN_ENV_BAL_3_0DB_BITS, 25),
            (&t::SBR_F_HUFFMAN_ENV_BAL_3_0DB_CODES, &t::SBR_F_HUFFMAN_ENV_BAL_3_0DB_BITS, 25),
            (&t::SBR_T_HUFFMAN_NOISE_3_0DB_CODES, &t::SBR_T_HUFFMAN_NOISE_3_0DB_BITS, 63),
            (&t::SBR_T_HUFFMAN_NOISE_BAL_3_0DB_CODES, &t::SBR_T_HUFFMAN_NOISE_BAL_3_0DB_BITS, 25),
        ]
    }

    /// HEAACSBR table KAT: each codebook has the spec's size (2·LAV + 1 symbols), is a complete prefix code
    /// (Kraft sum exactly 1, no codeword a prefix of another) and every symbol decodes back through `Huff`.
    /// HEAACSBR-PS (SR214): the ten PS codebooks are held to the same (sizes 61 61 29 29 15 15 8 8 8 8).
    #[test]
    fn codebooks_are_complete_prefix_codes() {
        for (bi, (codes, lens, size)) in books().iter().enumerate() {
            assert_eq!(*size as i32, 2 * LAV[bi] + 1, "book {} LAV", bi);
            check_book(bi, codes, lens, *size);
        }
        for (bi, (codes, lens, size)) in super::ps_tests::books().iter().enumerate() { check_book(100 + bi, codes, lens, *size); }
    }

    fn check_book(bi: usize, codes: &[u32], lens: &[u8], size: usize) {
        assert_eq!(codes.len(), size, "book {}", bi);
        assert_eq!(lens.len(), size, "book {}", bi);
        let max = *lens.iter().max().unwrap() as u32;
        let kraft: u64 = lens.iter().map(|&l| 1u64 << (max - l as u32)).sum();
        assert_eq!(kraft, 1u64 << max, "book {}: Kraft sum", bi);
        for i in 0..size {
            for j in 0..size {
                if i == j || lens[i] > lens[j] { continue; }
                let pre = codes[j] >> (lens[j] - lens[i]);
                assert!(pre != codes[i], "book {}: code {} is a prefix of {}", bi, i, j);
            }
        }
        let h = Huff::new(codes, lens);
        for sym in 0..size {
            let l = lens[sym] as u32;
            let v = (codes[sym] as u64) << (64 - l);
            let bytes = v.to_be_bytes();
            let mut r = BitReader::new(&bytes);
            assert_eq!(h.decode(&mut r).unwrap(), sym, "book {} symbol {}", bi, sym);
            assert_eq!(r.bit_pos(), l as usize);
        }
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n).map(|_| { s = s.wrapping_mul(1664525).wrapping_add(1013904223); (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5 }).collect()
    }

    /// The FFT forms of both banks equal the spec's definition sums (§4.6.18.4.1 / .2) on random data.
    #[test]
    fn qmf_matches_the_definition() {
        let pi = core::f64::consts::PI;
        let mut sbr = Sbr::new(22050, 1, false);
        let input = noise(1024, 7);
        sbr.analysis(0, &input);
        // the definition, for slot 31 (the newest 320 samples: 288 carried zeros + 1024 → indices 992..1312)
        let ch = &sbr.chans[0];
        let l = 31;
        let x: Vec<f64> = (0..320).map(|n| ch.ana[32 * l + 319 - n] as f64).collect();
        let mut u = [0f64; 64];
        for n in 0..64 { for j in 0..5 { u[n] += x[n + 64 * j] * t::SBR_QMF_C[2 * (n + 64 * j)] as f64; } }
        let mut err = 0f64;
        let mut mag = 0f64;
        for k in 0..32 {
            let (mut re, mut im) = (0f64, 0f64);
            for n in 0..64 {
                let a = pi * (k as f64 + 0.5) * (2.0 * n as f64 - 0.5) / 64.0;
                re += 2.0 * u[n] * a.cos();
                im += 2.0 * u[n] * a.sin();
            }
            let got = sbr.s.w[l][k];
            let (gr, gi) = (got[0] as f64 / SCALE as f64, got[1] as f64 / SCALE as f64);
            err = err.max((gr - re).abs()).max((gi - im).abs());
            mag = mag.max(re.abs()).max(im.abs());
        }
        assert!(err < 1e-4 * mag.max(1.0), "analysis: max err {} (scale {})", err, mag);
        // synthesis of one slot from fresh state: the 128 new entries of v
        let mut sbr = Sbr::new(22050, 1, false);
        let xr = noise(64, 11);
        let xi = noise(64, 13);
        for k in 0..64 { sbr.s.x_re[0][k] = xr[k]; sbr.s.x_im[0][k] = xi[k]; }
        let mut out = vec![0f32; 64];
        sbr.synth_slot(0, 0, &mut out);
        let ch = &sbr.chans[0];
        let o0 = ch.syn_off;
        let mut err = 0f64;
        for n in 0..128 {
            let mut v = 0f64;
            for k in 0..64 {
                let a = pi * (k as f64 + 0.5) * (2.0 * n as f64 - 255.0) / 128.0;
                v += (xr[k] as f64 * a.cos() - xi[k] as f64 * a.sin()) / 64.0;
            }
            let got = ch.syn[o0 + n] as f64 * SCALE as f64;
            err = err.max((got - v).abs());
        }
        assert!(err < 1e-5, "synthesis: max err {}", err);
    }

    /// The prototype window and both banks reconstruct: a frame of pure upsampling (no high band) returns the
    /// input at twice the rate, delayed by the banks' 577 output samples plus the 6-slot (384) HF-adjust offset.
    #[test]
    fn qmf_round_trip() {
        let mut sbr = Sbr::new(22050, 1, false);
        let n = 1024 * 12;
        let x: Vec<f32> = (0..n).map(|i| 0.3 * math::sin(0.05 * i as f64) as f32 + 0.2 * math::sin(1.3 * i as f64) as f32).collect();
        let mut y = Vec::new();
        for f in 0..n / 1024 {
            let mut out = vec![vec![0f32; 2048]];
            out[0][..1024].copy_from_slice(&x[f * 1024..f * 1024 + 1024]);
            sbr.apply(None, &[0], &mut out);
            y.extend_from_slice(&out[0]);
        }
        // least-squares gain per candidate delay (the banks are near-perfect-reconstruction: gain ~0.999)
        let mut best = (0f64, 0usize, 0f64);
        for d in 900..1100 {
            let (mut rr, mut ry) = (0f64, 0f64);
            for i in 2000..10000 { let r = x[i] as f64; rr += r * r; ry += r * y[2 * i + d] as f64; }
            let g = ry / rr;
            if g.abs() < 0.5 { continue; }
            let mut e = 0f64;
            for i in 2000..10000 { let r = x[i] as f64; let v = y[2 * i + d] as f64 / g - r; e += v * v; }
            let snr = 10.0 * math::log10(rr / e);
            if snr > best.0 { best = (snr, d, g); }
        }
        assert!(best.0 > 60.0 && (best.2 - 1.0).abs() < 0.01, "round trip SNR {:.1} dB at delay {} gain {:.4}", best.0, best.1, best.2);
    }

    /// Frequency tables for every SBR output rate and a spread of headers keep the spec's requirements.
    #[test]
    fn frequency_tables_hold_their_invariants() {
        let mut built = 0;
        for &rate in &[16000u32, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000] {
            for start in 0..16u8 {
                for stop in 0..16u8 {
                    for &(fs, alt) in &[(0u8, 0u8), (1, 0), (2, 1), (3, 0)] {
                        let mut e = Elem::new();
                        e.spectrum = Some(Spectrum { start_freq: start, stop_freq: stop, xover_band: 0, freq_scale: fs, alter_scale: alt, noise_bands: 2 });
                        if make_tables(&mut e, rate).is_err() { continue; }
                        built += 1;
                        for k in 1..=e.n_master { assert!(e.f_master[k] > e.f_master[k - 1]); }
                        assert!(e.kx[1] + e.m[1] <= 64 && e.kx[1] <= 32);
                        assert_eq!(e.n[0], e.n[1] - e.n[1] / 2);
                        assert!(e.n_q >= 1 && e.n_q <= 5);
                        assert_eq!(e.f_lim[0], e.f_low[0]);
                        // strictly increasing inside kx..=kx+M (it ends below kx+M when a final short patch was dropped)
                        for k in 1..=e.n_lim { assert!(e.f_lim[k] > e.f_lim[k - 1] && e.f_lim[k] <= e.kx[1] + e.m[1]); }
                        let covered: usize = e.patch_num[..e.num_patches].iter().sum();
                        assert!(covered <= e.m[1] && e.num_patches >= 1, "rate {} start {} stop {}", rate, start, stop);
                    }
                }
            }
        }
        assert!(built > 500, "only {} headers built", built);
    }
}

// ------------------------------------------------------------------ parametric stereo (§8.6.4; HEAACSBR-PS, LEDGER SR214)
//
// HE-AAC v2: the mono core's X matrix (after HF adjustment, 38 slots: 32 + the 6-slot lowband lookahead x_gen leaves in
// slots 32..37) is split by the hybrid filterbank (QMF band 0 into an 8-band complex split merged to 6, bands 1 and 2
// into 2 real bands each, bands 3..63 passed through: 71 hybrid bands, 20 stereo parameter bands), decorrelated
// (3-link all-pass with fractional delays below hybrid band 30, a 14-slot delay to band 41, 1 slot above, with the
// transient attenuation), mixed by the IID/ICC (+IPD/OPD) rotation interpolated across the envelopes, and summed back
// into the QMF bands: L into X, R into X2. The 20-band bank is used for every stream; parameters coded at 10 or 34
// band resolution are mapped to 20 (Table 8.46's maps; the 34-band bank is owed, handlers.md HEAACSBR-PS34).
// Written from the specification's equations; FFmpeg 6.1.1's aacps (LGPL-2.1-or-later) was the cross-reference for
// the per-frame state order (the H history, the fake final envelope, the IPD/OPD smoothing history) and its decode is
// the KAT's reference (tests/aac_kat.rs `heaac_vs_ffmpeg`); its code is not ported.

const PS_ENVS: usize = 6;
const PS_PARS: usize = 34;
/// hybrid bands (20-band configuration)
const PS_BANDS: usize = 71;
const PS_PAR_BANDS: usize = 20;
const PS_IPD_BANDS: usize = 11;
const PS_AP_BANDS: usize = 30;
const PS_DECAY_CUTOFF: f32 = 10.0;
const PS_DECAY_SLOPE: f32 = 0.05;
const PS_SHORT_DELAY_BAND: usize = 42;
const PS_MAX_DELAY: usize = 14;
const PS_NUM_ENV: [[usize; 4]; 2] = [[0, 1, 2, 4], [1, 2, 3, 4]];
const PS_NR_IIDICC: [usize; 6] = [10, 20, 34, 10, 20, 34];
const PS_NR_IPDOPD: [usize; 6] = [5, 11, 17, 5, 11, 17];
// codebook indices (the tables' order)
const PS_IID_DF1: usize = 0;
const PS_IID_DT1: usize = 1;
const PS_IID_DF0: usize = 2;
const PS_IID_DT0: usize = 3;
const PS_ICC_DF: usize = 4;
const PS_ICC_DT: usize = 5;
const PS_IPD_DF: usize = 6;
const PS_IPD_DT: usize = 7;
const PS_OPD_DF: usize = 8;
const PS_OPD_DT: usize = 9;

type PsPar = [[i8; PS_PARS]; PS_ENVS];

fn atan2(y: f64, x: f64) -> f64 {
    let pi = core::f64::consts::PI;
    if x > 0.0 { math::atan(y / x) } else if x < 0.0 { math::atan(y / x) + if y >= 0.0 { pi } else { -pi } } else if y > 0.0 { pi / 2.0 } else if y < 0.0 { -pi / 2.0 } else { 0.0 }
}

fn cmul(a: Cx, b: Cx) -> Cx { [a[0] * b[0] - a[1] * b[1], a[0] * b[1] + a[1] * b[0]] }

/// The parametric stereo state of one mono core: the parsed parameters and the filter / mixing history.
pub(crate) struct Ps {
    books: Vec<Huff>,
    /// a PS header was read: synthesis runs (else the mono X is copied to both channels)
    pub(crate) start: bool,
    pub(crate) read_this_frame: bool,
    enable_iid: bool,
    iid_quant: bool,
    nr_iid_par: usize,
    nr_ipdopd_par: usize,
    enable_icc: bool,
    icc_mode: usize,
    nr_icc_par: usize,
    enable_ext: bool,
    enable_ipdopd: bool,
    num_env_old: usize,
    num_env: usize,
    border: [i32; PS_ENVS + 1],
    iid_par: PsPar,
    icc_par: PsPar,
    ipd_par: PsPar,
    opd_par: PsPar,
    // computed tables
    ha: Vec<[[f32; 4]; 8]>,
    hb: Vec<[[f32; 4]; 8]>,
    pd_smooth: Vec<Cx>,
    phi_fract: [Cx; PS_AP_BANDS],
    q_fract: [[Cx; 3]; PS_AP_BANDS],
    /// the 8-band complex split of QMF band 0: [sub-band][tap 0..6]
    f20: [[Cx; 7]; 8],
    // filter and mixing history
    in_buf: [[Cx; 44]; 3],
    delay: Vec<[Cx; 32 + PS_MAX_DELAY]>,
    ap_delay: Vec<[[Cx; 37]; 3]>,
    peak_decay_nrg: [f32; PS_PAR_BANDS],
    power_smooth: [f32; PS_PAR_BANDS],
    peak_decay_diff_smooth: [f32; PS_PAR_BANDS],
    /// H11, H12, H21, H22 x [real, imaginary (IPD/OPD)] x envelope boundary (0 = the previous frame's last) x band
    h: [[[[f32; PS_PARS]; PS_ENVS + 1]; 2]; 4],
    ipd_hist: [i8; PS_IPD_BANDS],
    opd_hist: [i8; PS_IPD_BANDS],
    lbuf: Vec<[Cx; 32]>,
    rbuf: Vec<[Cx; 32]>,
}

impl Ps {
    pub(crate) fn new() -> Ps {
        let books = [
            (&t::PS_HUFF_IID_DF1_CODES[..], &t::PS_HUFF_IID_DF1_BITS[..]),
            (&t::PS_HUFF_IID_DT1_CODES[..], &t::PS_HUFF_IID_DT1_BITS[..]),
            (&t::PS_HUFF_IID_DF0_CODES[..], &t::PS_HUFF_IID_DF0_BITS[..]),
            (&t::PS_HUFF_IID_DT0_CODES[..], &t::PS_HUFF_IID_DT0_BITS[..]),
            (&t::PS_HUFF_ICC_DF_CODES[..], &t::PS_HUFF_ICC_DF_BITS[..]),
            (&t::PS_HUFF_ICC_DT_CODES[..], &t::PS_HUFF_ICC_DT_BITS[..]),
            (&t::PS_HUFF_IPD_DF_CODES[..], &t::PS_HUFF_IPD_DF_BITS[..]),
            (&t::PS_HUFF_IPD_DT_CODES[..], &t::PS_HUFF_IPD_DT_BITS[..]),
            (&t::PS_HUFF_OPD_DF_CODES[..], &t::PS_HUFF_OPD_DF_BITS[..]),
            (&t::PS_HUFF_OPD_DT_CODES[..], &t::PS_HUFF_OPD_DT_BITS[..]),
        ].iter().map(|&(c, l)| Huff::new(c, l)).collect();
        let pi = core::f64::consts::PI;
        // IPD/OPD smoothing (§8.6.4.6.3.2): the unit phasor of 1/4, 1/2, 1 weighted phases of the last three frames
        let pd = |k: usize| -> (f64, f64) { let (s, c) = math::sincos(k as f64 * pi / 4.0); (c, s) };
        let mut pd_smooth = vec![[0.0f32; 2]; 512];
        for p0 in 0..8 {
            for p1 in 0..8 {
                for p2 in 0..8 {
                    let (a, b, c) = (pd(p0), pd(p1), pd(p2));
                    let re = 0.25 * a.0 + 0.5 * b.0 + c.0;
                    let im = 0.25 * a.1 + 0.5 * b.1 + c.1;
                    let mag = math::sqrt(re * re + im * im);
                    pd_smooth[p0 * 64 + p1 * 8 + p2] = [(re / mag) as f32, (im / mag) as f32];
                }
            }
        }
        // mixing matrices (§8.6.4.6.3): A (icc_mode < 3) and B, per dequantised IID (15 default + 31 fine) and ICC
        let (mut ha, mut hb) = (vec![[[0.0f32; 4]; 8]; 46], vec![[[0.0f32; 4]; 8]; 46]);
        let sq2 = core::f64::consts::SQRT_2;
        for iid in 0..46 {
            let c = t::PS_IID_DEQUANT[iid] as f64;
            let c1 = sq2 / math::sqrt(1.0 + c * c);
            let c2 = c * c1;
            for icc in 0..8 {
                let alpha = 0.5 * t::PS_ACOS_ICC_INVQ[icc] as f64;
                let beta = alpha * (c1 - c2) / sq2;
                let (spa, cpa) = math::sincos(beta + alpha);
                let (sma, cma) = math::sincos(beta - alpha);
                ha[iid][icc] = [(c2 * cpa) as f32, (c1 * cma) as f32, (c2 * spa) as f32, (c1 * sma) as f32];
                let rho = (t::PS_ICC_INVQ[icc] as f64).max(0.05);
                let mut alpha = 0.5 * atan2(2.0 * c * rho, c * c - 1.0);
                let mu = c + 1.0 / c;
                let mu = math::sqrt(1.0 + (4.0 * rho * rho - 4.0) / (mu * mu));
                let gamma = math::atan(math::sqrt((1.0 - mu) / (1.0 + mu)));
                if alpha < 0.0 { alpha += pi / 2.0; }
                let (sa, ca) = math::sincos(alpha);
                let (sg, cg) = math::sincos(gamma);
                hb[iid][icc] = [(sq2 * ca * cg) as f32, (sq2 * sa * cg) as f32, (-sq2 * sa * sg) as f32, (sq2 * ca * sg) as f32];
            }
        }
        // decorrelator fractional delays (§8.6.4.5.2) at each all-pass band's centre frequency
        let (mut phi_fract, mut q_fract) = ([[0.0f32; 2]; PS_AP_BANDS], [[[0.0f32; 2]; 3]; PS_AP_BANDS]);
        for k in 0..PS_AP_BANDS {
            let fc = if k < 10 { t::PS_F_CENTER_20[k] as f64 * 0.125 } else { k as f64 - 6.5 };
            for m in 0..3 { q_fract[k][m] = cis(-pi * t::PS_FRACT_DELAY_LINKS[m] as f64 * fc); }
            phi_fract[k] = cis(-pi * t::PS_FRACT_DELAY_GAIN as f64 * fc);
        }
        // hybrid analysis (§8.6.4.3): g0 modulated to the 8 sub-band centres
        let mut f20 = [[[0.0f32; 2]; 7]; 8];
        for (q, row) in f20.iter_mut().enumerate() {
            for (n, f) in row.iter_mut().enumerate() {
                let c = cis(-2.0 * pi * (q as f64 + 0.5) * (n as f64 - 6.0) / 8.0);
                *f = [t::PS_G0_Q8[n] * c[0], t::PS_G0_Q8[n] * c[1]];
            }
        }
        Ps {
            books, start: false, read_this_frame: false, enable_iid: false, iid_quant: false, nr_iid_par: 0, nr_ipdopd_par: 0,
            enable_icc: false, icc_mode: 0, nr_icc_par: 0, enable_ext: false, enable_ipdopd: false, num_env_old: 0, num_env: 0,
            border: [0; PS_ENVS + 1], iid_par: [[0; PS_PARS]; PS_ENVS], icc_par: [[0; PS_PARS]; PS_ENVS],
            ipd_par: [[0; PS_PARS]; PS_ENVS], opd_par: [[0; PS_PARS]; PS_ENVS],
            ha, hb, pd_smooth, phi_fract, q_fract, f20,
            in_buf: [[[0.0; 2]; 44]; 3],
            delay: vec![[[0.0; 2]; 32 + PS_MAX_DELAY]; PS_BANDS],
            ap_delay: vec![[[[0.0; 2]; 37]; 3]; PS_AP_BANDS],
            peak_decay_nrg: [0.0; PS_PAR_BANDS], power_smooth: [0.0; PS_PAR_BANDS], peak_decay_diff_smooth: [0.0; PS_PAR_BANDS],
            h: [[[[0.0; PS_PARS]; PS_ENVS + 1]; 2]; 4],
            ipd_hist: [0; PS_IPD_BANDS], opd_hist: [0; PS_IPD_BANDS],
            lbuf: vec![[[0.0; 2]; 32]; PS_BANDS],
            rbuf: vec![[[0.0; 2]; 32]; PS_BANDS],
        }
    }

    /// ps_data (§8.3.1, Table 8.1) from `r`, at most `left` bits: the bits consumed, or 0 when the data is refused
    /// (then PS synthesis stops until the next header, the parameters are cleared and `r` is back where it was).
    pub(crate) fn read(&mut self, r: &mut BitReader, left: usize) -> usize {
        self.read_this_frame = true;
        let start = r.bit_pos();
        if self.read_inner(r).is_ok() && r.bit_pos() - start <= left { return r.bit_pos() - start; }
        self.start = false;
        for p in [&mut self.iid_par, &mut self.icc_par, &mut self.ipd_par, &mut self.opd_par] { *p = [[0; PS_PARS]; PS_ENVS]; }
        r.seek_bits(start);
        0
    }

    fn read_inner(&mut self, r: &mut BitReader) -> Result<()> {
        let header = r.bit()?;
        if header {
            self.enable_iid = r.bit()?;
            if self.enable_iid {
                let mode = r.read(3)? as usize;
                if mode > 5 { return Err(bad()); }
                self.nr_iid_par = PS_NR_IIDICC[mode];
                self.iid_quant = mode > 2;
                self.nr_ipdopd_par = PS_NR_IPDOPD[mode];
            }
            self.enable_icc = r.bit()?;
            if self.enable_icc {
                let mode = r.read(3)? as usize;
                if mode > 5 { return Err(bad()); }
                self.icc_mode = mode;
                self.nr_icc_par = PS_NR_IIDICC[mode];
            }
            self.enable_ext = r.bit()?;
        }
        let frame_class = r.bit()? as usize;
        self.num_env_old = self.num_env;
        self.num_env = PS_NUM_ENV[frame_class][r.read(2)? as usize];
        let ne = self.num_env;
        self.border[0] = -1;
        for e in 1..=ne {
            self.border[e] = if frame_class == 1 { r.read(5)? as i32 } else { ((e * 32) >> ne.trailing_zeros()) as i32 - 1 };
            if self.border[e] < self.border[e - 1] { return Err(bad()); }
        }
        let old = self.num_env_old;
        if self.enable_iid {
            let lim = 7 + 8 * self.iid_quant as i32;
            for e in 0..ne {
                let dt = r.bit()?;
                let book = match (dt, self.iid_quant) { (false, false) => PS_IID_DF0, (false, true) => PS_IID_DF1, (true, false) => PS_IID_DT0, (true, true) => PS_IID_DT1 };
                read_par(r, &self.books[book], t::PS_HUFF_OFFSET[book], 0, &mut self.iid_par, self.nr_iid_par, e, old, dt, |v| v.abs() <= lim)?;
            }
        } else {
            self.iid_par = [[0; PS_PARS]; PS_ENVS];
        }
        if self.enable_icc {
            for e in 0..ne {
                let dt = r.bit()?;
                let book = if dt { PS_ICC_DT } else { PS_ICC_DF };
                read_par(r, &self.books[book], t::PS_HUFF_OFFSET[book], 0, &mut self.icc_par, self.nr_icc_par, e, old, dt, |v| (0..=7).contains(&v))?;
            }
        } else {
            self.icc_par = [[0; PS_PARS]; PS_ENVS];
        }
        if self.enable_ext {
            let mut cnt = r.read(4)? as i32;
            if cnt == 15 { cnt += r.read(8)? as i32; }
            let mut cnt = 8 * cnt;
            while cnt > 7 {
                let id = r.read(2)?;
                let at = r.bit_pos();
                if id == 0 {
                    // ps_extension IPD/OPD (Table 8.6)
                    self.enable_ipdopd = r.bit()?;
                    if self.enable_ipdopd {
                        for e in 0..ne {
                            let dt = r.bit()?;
                            let book = if dt { PS_IPD_DT } else { PS_IPD_DF };
                            read_par(r, &self.books[book], 0, 7, &mut self.ipd_par, self.nr_ipdopd_par, e, old, dt, |_| true)?;
                            let dt = r.bit()?;
                            let book = if dt { PS_OPD_DT } else { PS_OPD_DF };
                            read_par(r, &self.books[book], 0, 7, &mut self.opd_par, self.nr_ipdopd_par, e, old, dt, |_| true)?;
                        }
                    }
                    r.skip(1)?;
                }
                cnt -= 2 + (r.bit_pos() - at) as i32;
            }
            if cnt < 0 { return Err(bad()); }
            r.skip(cnt as usize)?;
        }
        // a frame whose envelopes stop short of the last slot gets a final envelope repeating the last parameters
        if ne == 0 || self.border[ne] < 31 {
            let src = if ne > 0 { ne as i32 - 1 } else { old as i32 - 1 };
            if src >= 0 && src as usize != ne {
                let s = src as usize;
                if self.enable_iid { self.iid_par[ne] = self.iid_par[s]; }
                if self.enable_icc { self.icc_par[ne] = self.icc_par[s]; }
                if self.enable_ipdopd { self.ipd_par[ne] = self.ipd_par[s]; self.opd_par[ne] = self.opd_par[s]; }
            }
            if self.enable_iid && self.iid_par[ne][..self.nr_iid_par].iter().any(|&v| (v as i32).abs() > 7 + 8 * self.iid_quant as i32) { return Err(bad()); }
            if self.enable_icc && self.icc_par[ne][..self.nr_icc_par].iter().any(|&v| !(0..=7).contains(&v)) { return Err(bad()); }
            self.num_env += 1;
            self.border[self.num_env] = 31;
        }
        if !self.enable_ipdopd {
            self.ipd_par = [[0; PS_PARS]; PS_ENVS];
            self.opd_par = [[0; PS_PARS]; PS_ENVS];
        }
        if header { self.start = true; }
        Ok(())
    }

    /// The PS stage on one frame: `x` (the mono X, 38 slots) becomes L in its first 32 slots, `x2` becomes R.
    /// `top`: kx + M, the first QMF band with no signal (the delay lines above it are cleared).
    pub(crate) fn apply(&mut self, x_re: &mut [[f32; 64]], x_im: &mut [[f32; 64]], x2_re: &mut [[f32; 64]], x2_im: &mut [[f32; 64]], top: usize) {
        let top = (top + PS_BANDS - 64).min(PS_BANDS);
        for d in self.delay[top..].iter_mut() { *d = [[0.0; 2]; 32 + PS_MAX_DELAY]; }
        if top < PS_AP_BANDS { for d in self.ap_delay[top..].iter_mut() { *d = [[[0.0; 2]; 37]; 3]; } }
        self.hybrid_analysis(x_re, x_im);
        self.decorrelate();
        self.stereo_mix();
        hybrid_synthesis(&self.lbuf, x_re, x_im);
        hybrid_synthesis(&self.rbuf, x2_re, x2_im);
    }

    /// Hybrid analysis (§8.6.4.3) into `lbuf`: QMF band 0 through the 8-band complex filters (sub-bands 2+5 and 3+4
    /// merged), bands 1 and 2 through the 2-band real filter g1, bands 3..63 passed through. Each 13-tap filter is
    /// centred on tap 6, so slot j reads X slots j-6..j+6: the history is the previous frame's slots 26..31 and
    /// the lookahead is this frame's slots 32..37.
    fn hybrid_analysis(&mut self, x_re: &[[f32; 64]], x_im: &[[f32; 64]]) {
        for b in 0..3 {
            for j in 0..38 { self.in_buf[b][j + 6] = [x_re[j][b], x_im[j][b]]; }
        }
        let g1 = &t::PS_G1_Q2;
        for i in 0..32 {
            let w = &self.in_buf[0][i..i + 13];
            let mut tmp = [[0.0f32; 2]; 8];
            for (q, o) in tmp.iter_mut().enumerate() {
                let f = &self.f20[q];
                let mut acc = [f[6][0] * w[6][0], f[6][0] * w[6][1]];
                for j in 0..6 {
                    // taps j and 12 - j carry conjugate coefficients
                    let (a, b) = (w[j], w[12 - j]);
                    acc[0] += f[j][0] * (a[0] + b[0]) - f[j][1] * (a[1] - b[1]);
                    acc[1] += f[j][0] * (a[1] + b[1]) + f[j][1] * (a[0] - b[0]);
                }
                *o = acc;
            }
            self.lbuf[0][i] = tmp[6];
            self.lbuf[1][i] = tmp[7];
            self.lbuf[2][i] = tmp[0];
            self.lbuf[3][i] = tmp[1];
            self.lbuf[4][i] = [tmp[2][0] + tmp[5][0], tmp[2][1] + tmp[5][1]];
            self.lbuf[5][i] = [tmp[3][0] + tmp[4][0], tmp[3][1] + tmp[4][1]];
            // band 1's sum lands on the upper sub-band (7), band 2's on the lower (8): the bands alternate in direction
            for (band, (k_sum, k_diff)) in [(1usize, (7usize, 6usize)), (2, (8, 9))] {
                let w = &self.in_buf[band][i..i + 13];
                let inp = [g1[6] * w[6][0], g1[6] * w[6][1]];
                let mut op = [0.0f32; 2];
                for j in (0..6).step_by(2) {
                    op[0] += g1[j + 1] * (w[j + 1][0] + w[11 - j][0]);
                    op[1] += g1[j + 1] * (w[j + 1][1] + w[11 - j][1]);
                }
                self.lbuf[k_sum][i] = [inp[0] + op[0], inp[1] + op[1]];
                self.lbuf[k_diff][i] = [inp[0] - op[0], inp[1] - op[1]];
            }
            for k in 3..64 { self.lbuf[k + 7][i] = [x_re[i][k], x_im[i][k]]; }
        }
        for b in 0..3 { self.in_buf[b].copy_within(32..38, 0); }
    }

    /// Decorrelation (§8.6.4.5) of `lbuf` into `rbuf`, with the transient attenuation per parameter band.
    fn decorrelate(&mut self) {
        const PEAK_DECAY: f32 = 0.76592833836465;
        const A_SMOOTH: f32 = 0.25;
        const TRANSIENT_IMPACT: f32 = 1.5;
        let mut power = [[0.0f32; 32]; PS_PAR_BANDS];
        for k in 0..PS_BANDS {
            let i = t::PS_K_TO_I_20[k] as usize;
            for n in 0..32 { let v = self.lbuf[k][n]; power[i][n] += v[0] * v[0] + v[1] * v[1]; }
        }
        let mut gain = [[0.0f32; 32]; PS_PAR_BANDS];
        for i in 0..PS_PAR_BANDS {
            for n in 0..32 {
                self.peak_decay_nrg[i] = (PEAK_DECAY * self.peak_decay_nrg[i]).max(power[i][n]);
                self.power_smooth[i] += A_SMOOTH * (power[i][n] - self.power_smooth[i]);
                self.peak_decay_diff_smooth[i] += A_SMOOTH * (self.peak_decay_nrg[i] - power[i][n] - self.peak_decay_diff_smooth[i]);
                let denom = TRANSIENT_IMPACT * self.peak_decay_diff_smooth[i];
                gain[i][n] = if denom > self.power_smooth[i] { self.power_smooth[i] / denom } else { 1.0 };
            }
        }
        for k in 0..PS_BANDS {
            let i = t::PS_K_TO_I_20[k] as usize;
            let d = &mut self.delay[k];
            d.copy_within(32..32 + PS_MAX_DELAY, 0);
            d[PS_MAX_DELAY..].copy_from_slice(&self.lbuf[k]);
            if k < PS_AP_BANDS {
                // H(z) = z^-2 phi_fract Π_m (Q_fract z^-d(m) - a(m) g) / (1 - a(m) g Q_fract z^-d(m)), d = 3, 4, 5
                let g_decay = (1.0 - PS_DECAY_SLOPE * (k as f32 - PS_DECAY_CUTOFF)).clamp(0.0, 1.0);
                let ag = [t::PS_ALLPASS_A[0] * g_decay, t::PS_ALLPASS_A[1] * g_decay, t::PS_ALLPASS_A[2] * g_decay];
                let ap = &mut self.ap_delay[k];
                for m in 0..3 { ap[m].copy_within(32..37, 0); }
                for n in 0..32 {
                    let mut v = cmul(d[PS_MAX_DELAY - 2 + n], self.phi_fract[k]);
                    for m in 0..3 {
                        let link = cmul(ap[m][n + 2 - m], self.q_fract[k][m]);
                        let next = [link[0] - ag[m] * v[0], link[1] - ag[m] * v[1]];
                        ap[m][n + 5] = [v[0] + ag[m] * next[0], v[1] + ag[m] * next[1]];
                        v = next;
                    }
                    self.rbuf[k][n] = [gain[i][n] * v[0], gain[i][n] * v[1]];
                }
            } else {
                let lag = if k < PS_SHORT_DELAY_BAND { 14 } else { 1 };
                for n in 0..32 { let v = d[PS_MAX_DELAY - lag + n]; self.rbuf[k][n] = [gain[i][n] * v[0], gain[i][n] * v[1]]; }
            }
        }
    }

    /// Stereo mixing (§8.6.4.6): per envelope and parameter band the rotation from IID/ICC (+ IPD/OPD phases),
    /// interpolated slot by slot from the previous envelope's, applied to (s = lbuf, d = rbuf) -> (L, R).
    fn stereo_mix(&mut self) {
        let (mut iid, mut icc, mut ipd, mut opd) = ([[0i8; PS_PARS]; PS_ENVS], [[0i8; PS_PARS]; PS_ENVS], [[0i8; PS_PARS]; PS_ENVS], [[0i8; PS_PARS]; PS_ENVS]);
        let ne = self.num_env.min(PS_ENVS - 1);
        if self.num_env_old > 0 {
            let o = self.num_env_old.min(PS_ENVS);
            for hm in self.h.iter_mut() { for part in hm.iter_mut() { part[0] = part[o]; } }
        }
        for e in 0..ne {
            remap20(&mut iid[e], &self.iid_par[e], self.nr_iid_par, true);
            remap20(&mut icc[e], &self.icc_par[e], self.nr_icc_par, true);
            if self.enable_ipdopd {
                remap20(&mut ipd[e], &self.ipd_par[e], self.nr_ipdopd_par, false);
                remap20(&mut opd[e], &self.opd_par[e], self.nr_ipdopd_par, false);
            }
        }
        let lut = if self.icc_mode < 3 { &self.ha } else { &self.hb };
        let ipdopd = self.enable_ipdopd;
        for e in 0..ne {
            for b in 0..PS_PAR_BANDS {
                let idx = (iid[e][b] as i32 + 7 + 23 * self.iid_quant as i32).clamp(0, 45) as usize;
                let mut hh = lut[idx][(icc[e][b] as usize).min(7)];
                if ipdopd && b < PS_IPD_BANDS {
                    let oi = self.opd_hist[b] as usize * 8 + (opd[e][b] as usize & 7);
                    let ii = self.ipd_hist[b] as usize * 8 + (ipd[e][b] as usize & 7);
                    let (o, p) = (self.pd_smooth[oi], self.pd_smooth[ii]);
                    self.opd_hist[b] = (oi & 0x3f) as i8;
                    self.ipd_hist[b] = (ii & 0x3f) as i8;
                    let adj = [o[0] * p[0] + o[1] * p[1], o[1] * p[0] - o[0] * p[1]];
                    let im = [hh[0] * o[1], hh[1] * adj[1], hh[2] * o[1], hh[3] * adj[1]];
                    hh = [hh[0] * o[0], hh[1] * adj[0], hh[2] * o[0], hh[3] * adj[0]];
                    for j in 0..4 { self.h[j][1][e + 1][b] = im[j]; }
                }
                for j in 0..4 { self.h[j][0][e + 1][b] = hh[j]; }
            }
            let (start, stop) = (self.border[e], self.border[e + 1]);
            if stop <= start { continue; }
            let len = (stop - start) as usize;
            let width = 1.0 / len as f32;
            for k in 0..PS_BANDS {
                let b = t::PS_K_TO_I_20[k] as usize;
                let mut h0 = [0.0f32; 4];
                let mut h1 = [0.0f32; 4];
                let mut s0 = [0.0f32; 4];
                let mut s1 = [0.0f32; 4];
                for j in 0..4 {
                    h0[j] = self.h[j][0][e][b];
                    s0[j] = (self.h[j][0][e + 1][b] - h0[j]) * width;
                    if ipdopd {
                        // the two lowest hybrid bands carry negative frequencies: their phase turns the other way
                        h1[j] = if k <= 1 { -self.h[j][1][e][b] } else { self.h[j][1][e][b] };
                        s1[j] = (self.h[j][1][e + 1][b] - h1[j]) * width;
                    }
                }
                let first = (start + 1) as usize;
                for n in first..(first + len).min(32) {
                    for j in 0..4 { h0[j] += s0[j]; h1[j] += s1[j]; }
                    let (l, r) = (self.lbuf[k][n], self.rbuf[k][n]);
                    self.lbuf[k][n] = [h0[0] * l[0] + h0[2] * r[0] - h1[0] * l[1] - h1[2] * r[1], h0[0] * l[1] + h0[2] * r[1] + h1[0] * l[0] + h1[2] * r[0]];
                    self.rbuf[k][n] = [h0[1] * l[0] + h0[3] * r[0] - h1[1] * l[1] - h1[3] * r[1], h0[1] * l[1] + h0[3] * r[1] + h1[1] * l[0] + h1[3] * r[0]];
                }
            }
        }
    }
}

/// One envelope's parameters (iid/icc/ipd/opd) of `num` bands, delta coded in frequency or in time (`dt`: against the
/// previous envelope, or the previous frame's last); symbol - `offset`, `mask` (IPD/OPD: modulo 8), refused unless `ok`.
#[allow(clippy::too_many_arguments)]
fn read_par(r: &mut BitReader, book: &Huff, offset: i32, mask: i32, par: &mut PsPar, num: usize, e: usize, num_env_old: usize, dt: bool, ok: impl Fn(i32) -> bool) -> Result<()> {
    let prev = if e > 0 { e - 1 } else { num_env_old.saturating_sub(1) };
    let mut acc = 0i32;
    for b in 0..num.min(PS_PARS) {
        let d = book.decode(r)? as i32 - offset;
        let v = if dt { par[prev][b] as i32 + d } else { acc + d };
        let v = if mask != 0 { v & mask } else { v };
        acc = v;
        if !ok(v) { return Err(bad()); }
        par[e][b] = v as i8;
    }
    Ok(())
}

/// Parameters at 10 or 34 band resolution (IID/ICC; 5 or 17 for IPD/OPD) on the 20-band grid (Table 8.46); `full`
/// false maps only the IPD/OPD bands.
fn remap20(out: &mut [i8; PS_PARS], par: &[i8; PS_PARS], num: usize, full: bool) {
    let p = |i: usize| par[i] as i32;
    match num {
        34 | 17 => {
            let m: [i32; 11] = [(2 * p(0) + p(1)) / 3, (p(1) + 2 * p(2)) / 3, (2 * p(3) + p(4)) / 3, (p(4) + 2 * p(5)) / 3, (p(6) + p(7)) / 2,
                (p(8) + p(9)) / 2, p(10), p(11), (p(12) + p(13)) / 2, (p(14) + p(15)) / 2, p(16)];
            for (o, v) in out.iter_mut().zip(m) { *o = v as i8; }
            if full {
                let m: [i32; 9] = [p(17), p(18), p(19), (p(20) + p(21)) / 2, (p(22) + p(23)) / 2, (p(24) + p(25)) / 2, (p(26) + p(27)) / 2,
                    (p(28) + p(29) + p(30) + p(31)) / 4, (p(32) + p(33)) / 2];
                for (o, v) in out[11..].iter_mut().zip(m) { *o = v as i8; }
            }
        }
        10 | 5 => {
            let n = if full { 10 } else { out[10] = 0; 5 };
            for b in (0..n).rev() { out[2 * b] = par[b]; out[2 * b + 1] = par[b]; }
        }
        _ => *out = *par,
    }
}

/// Hybrid synthesis (§8.6.4.7): the sub-bands of QMF bands 0..2 summed, bands 3..63 passed back, into X's 32 slots.
fn hybrid_synthesis(buf: &[[Cx; 32]], x_re: &mut [[f32; 64]], x_im: &mut [[f32; 64]]) {
    for n in 0..32 {
        let sum = |a: usize, b: usize| -> Cx { buf[a..b].iter().fold([0.0f32; 2], |s, v| [s[0] + v[n][0], s[1] + v[n][1]]) };
        for (k, v) in [sum(0, 6), sum(6, 8), sum(8, 10)].into_iter().enumerate() { x_re[n][k] = v[0]; x_im[n][k] = v[1]; }
        for k in 3..64 { x_re[n][k] = buf[k + 7][n][0]; x_im[n][k] = buf[k + 7][n][1]; }
    }
}

impl Sbr {
    /// HEAACSBR-PS: the mono channel's X through the PS stage (or, before the first PS header, copied: the duplicate),
    /// L synthesised on channel 0 into `out[0]`, R on the last channel's delay line into `out[1]`.
    fn ps_synthesis(&mut self, e: &Elem, out: &mut [Vec<f32>]) {
        let top = (e.kx[1] + e.m[1]).min(64);
        {
            let s = &mut *self.s;
            let Some(ps) = self.ps.as_mut() else { return };
            if ps.start {
                ps.apply(&mut s.x_re, &mut s.x_im, &mut s.x2_re, &mut s.x2_im, top);
            } else {
                s.x2_re.copy_from_slice(&s.x_re);
                s.x2_im.copy_from_slice(&s.x_im);
            }
        }
        self.synthesis(0, &mut out[0][..2048]);
        core::mem::swap(&mut self.s.x_re, &mut self.s.x2_re);
        core::mem::swap(&mut self.s.x_im, &mut self.s.x2_im);
        let rc = self.chans.len() - 1;
        self.synthesis(rc, &mut out[1][..2048]);
        core::mem::swap(&mut self.s.x_re, &mut self.s.x2_re);
        core::mem::swap(&mut self.s.x_im, &mut self.s.x2_im);
    }
}

#[cfg(test)]
pub(crate) mod ps_tests {
    use super::*;

    /// The ten PS codebooks with the spec's sizes (held to complete prefix codes by tests::codebooks_are_complete_prefix_codes).
    pub(crate) fn books() -> [(&'static [u32], &'static [u8], usize); 10] {
        [
            (&t::PS_HUFF_IID_DF1_CODES, &t::PS_HUFF_IID_DF1_BITS, 61),
            (&t::PS_HUFF_IID_DT1_CODES, &t::PS_HUFF_IID_DT1_BITS, 61),
            (&t::PS_HUFF_IID_DF0_CODES, &t::PS_HUFF_IID_DF0_BITS, 29),
            (&t::PS_HUFF_IID_DT0_CODES, &t::PS_HUFF_IID_DT0_BITS, 29),
            (&t::PS_HUFF_ICC_DF_CODES, &t::PS_HUFF_ICC_DF_BITS, 15),
            (&t::PS_HUFF_ICC_DT_CODES, &t::PS_HUFF_ICC_DT_BITS, 15),
            (&t::PS_HUFF_IPD_DF_CODES, &t::PS_HUFF_IPD_DF_BITS, 8),
            (&t::PS_HUFF_IPD_DT_CODES, &t::PS_HUFF_IPD_DT_BITS, 8),
            (&t::PS_HUFF_OPD_DF_CODES, &t::PS_HUFF_OPD_DF_BITS, 8),
            (&t::PS_HUFF_OPD_DT_CODES, &t::PS_HUFF_OPD_DT_BITS, 8),
        ]
    }

    struct Lcg(u32);
    impl Lcg {
        fn next(&mut self) -> f32 { self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223); (self.0 >> 8) as f32 / (1u32 << 24) as f32 - 0.5 }
    }

    /// One envelope over the whole frame with every parameter band at (`iid`, `icc`), as a parsed header leaves it.
    fn set_params(ps: &mut Ps, iid: i8, icc: i8, quant: bool, icc_mode: usize) {
        ps.start = true;
        ps.enable_iid = true;
        ps.enable_icc = true;
        ps.iid_quant = quant;
        ps.icc_mode = icc_mode;
        ps.nr_iid_par = 20;
        ps.nr_icc_par = 20;
        ps.num_env = 1;
        ps.border[0] = -1;
        ps.border[1] = 31;
        for b in 0..PS_PARS { ps.iid_par[0][b] = iid; ps.icc_par[0][b] = icc; }
    }

    /// L/R power ratio (dB) and normalised coherence Re(Σ L R*) / sqrt(Σ|L|² Σ|R|²).
    fn ratio_coherence(l: &[Cx], r: &[Cx]) -> (f64, f64) {
        let (mut pl, mut pr, mut x) = (0f64, 0f64, 0f64);
        for (a, b) in l.iter().zip(r) {
            pl += (a[0] * a[0] + a[1] * a[1]) as f64;
            pr += (b[0] * b[0] + b[1] * b[1]) as f64;
            x += (a[0] * b[0] + a[1] * b[1]) as f64;
        }
        (10.0 * math::log10(pl / pr), x / math::sqrt(pl * pr))
    }

    /// HEAACSBR-PS unit KAT (mixing): with s and d of equal power and orthogonal (d = i s, unit modulus, random phase),
    /// every IID (15 default, 31 fine steps) x ICC (8) on mixing A and B gives L/R the IID's level difference within
    /// 0.5 dB and the ICC's coherence within 0.02 (mixing B: max(ICC, 0.05), its definition).
    #[test]
    fn mixing_matches_iid_icc() {
        let mut ps = Ps::new();
        let mut rng = Lcg(5);
        let mut checked = 0;
        for (quant, steps) in [(false, 7i8), (true, 15)] {
            for iid in -steps..=steps {
                for icc in 0..8i8 {
                    for mode in [0usize, 3] {
                        set_params(&mut ps, iid, icc, quant, mode);
                        // frame 1 ramps H from the previous frame's; frame 2 holds it
                        for frame in 0..2 {
                            ps.num_env_old = frame;
                            for k in 0..PS_BANDS {
                                for n in 0..32 {
                                    let c = cis(2.0 * core::f64::consts::PI * rng.next() as f64);
                                    ps.lbuf[k][n] = c;
                                    ps.rbuf[k][n] = [-c[1], c[0]];
                                }
                            }
                            ps.stereo_mix();
                        }
                        let l: Vec<Cx> = ps.lbuf.iter().flat_map(|b| b.iter().copied()).collect();
                        let r: Vec<Cx> = ps.rbuf.iter().flat_map(|b| b.iter().copied()).collect();
                        let (ratio, coh) = ratio_coherence(&l, &r);
                        let want_ratio = 20.0 * math::log10(t::PS_IID_DEQUANT[(iid as i32 + 7 + 23 * quant as i32) as usize] as f64);
                        let icc_v = t::PS_ICC_INVQ[icc as usize] as f64;
                        let want_coh = if mode < 3 { icc_v } else { icc_v.max(0.05) };
                        assert!((ratio - want_ratio).abs() < 0.5, "iid {} (fine {}) icc {} mode {}: ratio {:.3} dB, want {:.3}", iid, quant, icc, mode, ratio, want_ratio);
                        assert!((coh - want_coh).abs() < 0.02, "iid {} (fine {}) icc {} mode {}: coherence {:.4}, want {:.4}", iid, quant, icc, mode, coh, want_coh);
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, (15 + 31) * 8 * 2);
    }

    /// Run `frames` frames of a continuous complex noise stream in the QMF domain through the whole PS stage (each
    /// frame's X = 38 stream slots, the last 6 being the next frame's first: x_gen's lookahead); returns per frame
    /// (input slots, L, R) as flat [band-major] vectors of the 32 output slots.
    fn run_stage(ps: &mut Ps, frames: usize) -> Vec<(Vec<Cx>, Vec<Cx>, Vec<Cx>)> {
        let mut rng = Lcg(9);
        let stream: Vec<[Cx; 64]> = (0..32 * frames + 6).map(|_| { let mut s = [[0.0f32; 2]; 64]; for v in s.iter_mut() { *v = [rng.next(), rng.next()]; } s }).collect();
        let mut out = Vec::new();
        for f in 0..frames {
            let (mut xr, mut xi) = (vec![[0.0f32; 64]; 38], vec![[0.0f32; 64]; 38]);
            let (mut x2r, mut x2i) = (vec![[0.0f32; 64]; 38], vec![[0.0f32; 64]; 38]);
            for l in 0..38 { for k in 0..64 { xr[l][k] = stream[32 * f + l][k][0]; xi[l][k] = stream[32 * f + l][k][1]; } }
            ps.num_env_old = if f == 0 { 0 } else { 1 };
            ps.apply(&mut xr, &mut xi, &mut x2r, &mut x2i, 64);
            let flat = |re: &[[f32; 64]], im: &[[f32; 64]]| -> Vec<Cx> { (0..64).flat_map(|k| (0..32).map(move |n| [re[n][k], im[n][k]])).collect() };
            let st = &stream;
            let inp: Vec<Cx> = (0..64).flat_map(|k| (0..32).map(move |n| st[32 * f + n][k])).collect();
            out.push((inp, flat(&xr, &xi), flat(&x2r, &x2i)));
        }
        out
    }

    /// HEAACSBR-PS unit KAT (passthrough): with no IID and no ICC (the bear v2 stream's PS) the whole stage —
    /// hybrid analysis, decorrelation, mixing, hybrid synthesis — returns the input on both channels, slot-aligned,
    /// within 1e-5 relative (the hybrid filters sum to a unit impulse at their centre tap).
    #[test]
    fn identity_is_passthrough() {
        let mut ps = Ps::new();
        set_params(&mut ps, 0, 0, false, 0);
        ps.enable_iid = false;
        ps.enable_icc = false;
        for (f, (inp, l, r)) in run_stage(&mut ps, 5).into_iter().enumerate().skip(1) {
            let p: f64 = inp.iter().map(|v| (v[0] * v[0] + v[1] * v[1]) as f64).sum();
            for (name, got) in [("L", &l), ("R", &r)] {
                let e: f64 = inp.iter().zip(got.iter()).map(|(a, b)| { let (d0, d1) = ((a[0] - b[0]) as f64, (a[1] - b[1]) as f64); d0 * d0 + d1 * d1 }).sum();
                assert!(math::sqrt(e / p) < 1e-5, "frame {} {}: relative error {:e}", f, name, math::sqrt(e / p));
            }
        }
    }

    /// HEAACSBR-PS unit KAT (whole stage, IID): IID +10 dB, ICC 1 on noise: L/R power within 0.5 dB of 10 dB and
    /// coherence above 0.98, through the decorrelator and the hybrid bank.
    #[test]
    fn stage_iid() {
        let mut ps = Ps::new();
        set_params(&mut ps, 4, 0, false, 0);
        let want = 20.0 * math::log10(t::PS_IID_DEQUANT[11] as f64);
        for (f, (_, l, r)) in run_stage(&mut ps, 5).into_iter().enumerate().skip(1) {
            let (ratio, coh) = ratio_coherence(&l, &r);
            assert!((ratio - want).abs() < 0.5 && (want - 10.0).abs() < 0.01, "frame {}: ratio {:.3} dB, want {:.3}", f, ratio, want);
            assert!(coh > 0.98, "frame {}: coherence {:.4}", f, coh);
        }
    }
}
