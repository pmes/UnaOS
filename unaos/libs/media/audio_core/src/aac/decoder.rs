// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The AAC raw_data_block decoder (ISO/IEC 14496-3 §4.4.2 syntax, §4.6 tools): SCE/CPE/LFE, CCE (parsed,
//! not applied), DSE, PCE, FIL; ics_info with window grouping, section data, scalefactors, pulse data,
//! TNS, spectral Huffman decoding, inverse quantisation, M/S, intensity stereo, perceptual noise
//! substitution, and the filterbank (sine/KBD windows, the four window sequences, overlap-add).
//! Object types: AAC LC (2) and AAC Main (1) without prediction state is refused; LC is the target.
use alloc::vec;
use alloc::vec::Vec;

use super::tables as t;
use crate::bits::BitReader;
use crate::math;
use crate::{Error, Result};

const ONLY_LONG: u8 = 0;
const LONG_START: u8 = 1;
const EIGHT_SHORT: u8 = 2;
const LONG_STOP: u8 = 3;

const ZERO_HCB: u8 = 0;
const ESC_HCB: u8 = 11;
const NOISE_HCB: u8 = 13;
const INTENSITY_HCB2: u8 = 14;
const INTENSITY_HCB: u8 = 15;

// ------------------------------------------------------------------ Huffman

/// A binary code tree built from (code, length) pairs; leaves carry the symbol index.
pub(crate) struct Huff {
    nodes: Vec<[u32; 2]>,
}

const LEAF: u32 = 0x8000_0000;

impl Huff {
    pub(crate) fn new<C: Copy + Into<u32>>(codes: &[C], lens: &[u8]) -> Huff {
        let mut nodes: Vec<[u32; 2]> = vec![[0, 0]];
        for (sym, (&c, &l)) in codes.iter().zip(lens).enumerate() {
            let c: u32 = c.into();
            let mut n = 0usize;
            for b in (0..l).rev() {
                let bit = ((c >> b) & 1) as usize;
                if b == 0 {
                    nodes[n][bit] = LEAF | sym as u32;
                } else {
                    if nodes[n][bit] == 0 {
                        nodes.push([0, 0]);
                        let k = (nodes.len() - 1) as u32;
                        nodes[n][bit] = k;
                    }
                    n = nodes[n][bit] as usize;
                }
            }
        }
        Huff { nodes }
    }
    #[inline]
    pub(crate) fn decode(&self, r: &mut BitReader) -> Result<usize> {
        let mut n = 0usize;
        loop {
            let v = self.nodes[n][r.read(1)? as usize];
            if v & LEAF != 0 { return Ok((v & !LEAF) as usize); }
            if v == 0 { return Err(Error::Invalid("aac: bad huffman code")); }
            n = v as usize;
        }
    }
}

// ------------------------------------------------------------------ configuration

/// Element kinds (§4.4.2.1, Table 4.85).
pub const ID_SCE: u32 = 0;
pub const ID_CPE: u32 = 1;
pub const ID_CCE: u32 = 2;
pub const ID_LFE: u32 = 3;
pub const ID_DSE: u32 = 4;
pub const ID_PCE: u32 = 5;
pub const ID_FIL: u32 = 6;
pub const ID_END: u32 = 7;

/// A program_config_element (§4.4.1.2), reduced to what the channel layout needs.
#[derive(Clone, Debug, Default)]
pub struct Pce {
    pub sf_index: u8,
    /// (is_cpe, tag) in order: front, side, back.
    pub front: Vec<(bool, u8)>,
    pub side: Vec<(bool, u8)>,
    pub back: Vec<(bool, u8)>,
    pub lfe: Vec<u8>,
}

impl Pce {
    pub fn channels(&self) -> usize {
        let c = |v: &Vec<(bool, u8)>| v.iter().map(|&(p, _)| if p { 2 } else { 1 }).sum::<usize>();
        c(&self.front) + c(&self.side) + c(&self.back) + self.lfe.len()
    }
    /// Parse; `start` is the bit position the byte_alignment() inside is relative to.
    pub fn parse(r: &mut BitReader, start: usize) -> Result<Pce> {
        let mut p = Pce::default();
        let _tag = r.read(4)?;
        let _object_type = r.read(2)?;
        p.sf_index = r.read(4)? as u8;
        let nf = r.read(4)?;
        let ns = r.read(4)?;
        let nb = r.read(4)?;
        let nl = r.read(2)?;
        let na = r.read(3)?;
        let nc = r.read(4)?;
        if r.bit()? { r.read(4)?; }
        if r.bit()? { r.read(4)?; }
        if r.bit()? { r.read(3)?; }
        for (n, v) in [(nf, &mut p.front), (ns, &mut p.side), (nb, &mut p.back)] {
            for _ in 0..n { let cpe = r.bit()?; v.push((cpe, r.read(4)? as u8)); }
        }
        for _ in 0..nl { p.lfe.push(r.read(4)? as u8); }
        for _ in 0..na { r.read(4)?; }
        for _ in 0..nc { r.read(5)?; }
        // byte_alignment() relative to the start of the enclosing structure
        let rel = r.bit_pos() - start;
        r.skip((8 - rel % 8) % 8)?;
        let cb = r.read(8)? as usize;
        r.skip(8 * cb)?;
        Ok(p)
    }
}

/// Speaker roles, in WAVE channel-mask order (the order handed out).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Role { FL, FR, FC, Lfe, BL, BR, FLC, FRC, BC, SL, SR, Other(u8) }

/// Where each element's channels go: per element kind (SCE, CPE, LFE), the n-th occurrence in a frame
/// maps to output channel(s).
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub channels: usize,
    sce: Vec<usize>,
    cpe: Vec<(usize, usize)>,
    lfe: Vec<usize>,
}

impl Layout {
    /// From a PCE: front elements run from the centre outwards, so with two front pairs the first is the
    /// inner (FLC/FRC) one; sides are SL/SR, back pairs BL/BR, a back single BC.
    pub fn from_pce(p: &Pce) -> Layout {
        // (kind 0=sce 1=cpe 2=lfe, roles) in element order of appearance per kind
        let mut items: Vec<(u8, Role, Option<Role>)> = vec![];
        let front_pairs = p.front.iter().filter(|e| e.0).count();
        let mut fp = 0;
        let mut other = 0u8;
        let mut oth = || { other += 1; Role::Other(other) };
        let mut fc_used = false;
        for &(cpe, _) in &p.front {
            if cpe {
                let inner = front_pairs >= 2 && fp == 0;
                fp += 1;
                if inner { items.push((1, Role::FLC, Some(Role::FRC))); }
                else if fp <= 2 { items.push((1, Role::FL, Some(Role::FR))); }
                else { let a = oth(); let b = oth(); items.push((1, a, Some(b))); }
            } else if !fc_used { fc_used = true; items.push((0, Role::FC, None)); }
            else { items.push((0, oth(), None)); }
        }
        let mut side_used = false;
        for &(cpe, _) in &p.side {
            if cpe && !side_used { side_used = true; items.push((1, Role::SL, Some(Role::SR))); }
            else if cpe { let a = oth(); let b = oth(); items.push((1, a, Some(b))); }
            else { items.push((0, oth(), None)); }
        }
        let (mut back_pair, mut back_single) = (false, false);
        for &(cpe, _) in &p.back {
            if cpe && !back_pair { back_pair = true; items.push((1, Role::BL, Some(Role::BR))); }
            else if !cpe && !back_single { back_single = true; items.push((0, Role::BC, None)); }
            else if cpe { let a = oth(); let b = oth(); items.push((1, a, Some(b))); }
            else { items.push((0, oth(), None)); }
        }
        let mut lfe_used = false;
        for _ in &p.lfe {
            if !lfe_used { lfe_used = true; items.push((2, Role::Lfe, None)); } else { items.push((2, oth(), None)); }
        }
        // order the output channels by role
        let mut roles: Vec<Role> = vec![];
        for it in &items { roles.push(it.1); if let Some(r) = it.2 { roles.push(r); } }
        roles.sort();
        let idx = |r: Role| roles.iter().position(|&x| x == r).unwrap();
        let mut l = Layout { channels: roles.len(), ..Default::default() };
        for it in &items {
            match it.0 {
                0 => l.sce.push(idx(it.1)),
                1 => l.cpe.push((idx(it.1), idx(it.2.unwrap()))),
                _ => l.lfe.push(idx(it.1)),
            }
        }
        l
    }
    /// The default layouts of channelConfiguration 1–7 (Table 1.19), expressed as PCEs.
    pub fn from_config(cc: u8) -> Option<Layout> {
        let s = (false, 0u8);
        let c = (true, 0u8);
        let p = match cc {
            1 => Pce { front: vec![s], ..Default::default() },
            2 => Pce { front: vec![c], ..Default::default() },
            3 => Pce { front: vec![s, c], ..Default::default() },
            4 => Pce { front: vec![s, c], back: vec![s], ..Default::default() },
            5 => Pce { front: vec![s, c], back: vec![c], ..Default::default() },
            6 => Pce { front: vec![s, c], back: vec![c], lfe: vec![0], ..Default::default() },
            7 => Pce { front: vec![s, c, c], back: vec![c], lfe: vec![0], ..Default::default() },
            _ => return None,
        };
        Some(Layout::from_pce(&p))
    }
}

// ------------------------------------------------------------------ channel state

#[derive(Clone)]
struct Tns {
    n_filt: [u8; 8],
    // per window, up to 3 filters (long: up to 3, short: 1)
    length: [[u8; 4]; 8],
    order: [[u8; 4]; 8],
    direction: [[bool; 4]; 8],
    lpc: [[[f32; 20]; 4]; 8],
}

impl Default for Tns {
    fn default() -> Tns { Tns { n_filt: [0; 8], length: [[0; 4]; 8], order: [[0; 4]; 8], direction: [[false; 4]; 8], lpc: [[[0.0; 20]; 4]; 8] } }
}

#[derive(Clone)]
struct Ics {
    window_sequence: u8,
    window_shape: u8,
    max_sfb: usize,
    num_windows: usize,
    group_len: Vec<usize>,
    /// swb offsets of the current window length
    swb: &'static [u16],
    num_swb: usize,
    tns_max_bands: usize,
    predictor: bool,
}

impl Default for Ics {
    fn default() -> Ics {
        Ics { window_sequence: 0, window_shape: 0, max_sfb: 0, num_windows: 1, group_len: vec![1], swb: &t::SWB_1024_48, num_swb: 0, tns_max_bands: 0, predictor: false }
    }
}

/// One decoded individual_channel_stream, before the filterbank.
struct Chan {
    ics: Ics,
    band_type: [u8; 8 * 64],
    /// scalefactor (normal), noise energy (PNS) or intensity position, per [group][sfb]
    sf: [i32; 8 * 64],
    tns_present: bool,
    tns: Tns,
    coef: [f32; 1024],
}

impl Chan {
    fn new() -> Chan { Chan { ics: Ics::default(), band_type: [0; 512], sf: [0; 512], tns_present: false, tns: Tns::default(), coef: [0.0; 1024] } }
}

/// Filterbank state of one output channel.
#[derive(Clone)]
struct Overlap {
    saved: Vec<f32>,
    prev_shape: u8,
}

// ------------------------------------------------------------------ the decoder

pub struct AacDecoder {
    pub sf_index: usize,
    pub layout: Layout,
    sf_huff: Huff,
    spec_huff: Vec<Huff>,
    pow43: Vec<f32>,
    imdct_long: crate::vorbis::Imdct,
    imdct_short: crate::vorbis::Imdct,
    /// windows: [shape][long=0/short=1] rising halves (len 1024 / 128)
    win: [[Vec<f32>; 2]; 2],
    ov: Vec<Overlap>,
    random: u32,
    chans: [Chan; 2],
    /// per output channel decoded this frame
    pub out: Vec<Vec<f32>>,
    /// Set when the stream carries an SBR extension (decoded here as the AAC-LC core only).
    pub sbr_seen: bool,
    /// The PCE seen in-band (channelConfiguration 0).
    pub pce: Option<Pce>,
    scratch: Vec<f32>,
}

fn kbd(n: usize, alpha: f64) -> Vec<f32> {
    // Kaiser-Bessel derived window, rising half of length n/2 (§4.6.11.3.2)
    let half = n / 2;
    let i0 = |x: f64| {
        let mut s = 1.0f64;
        let mut term = 1.0f64;
        let q = x * x / 4.0;
        for k in 1..60 { term *= q / (k * k) as f64; s += term; if term < 1e-30 * s { break; } }
        s
    };
    let pa = core::f64::consts::PI * alpha;
    let w: Vec<f64> = (0..=half).map(|j| {
        let r = (j as f64 - half as f64 / 2.0) / (half as f64 / 2.0);
        i0(pa * math::sqrt((1.0 - r * r).max(0.0)))
    }).collect();
    let total: f64 = w.iter().sum();
    let mut acc = 0.0;
    (0..half).map(|i| { acc += w[i]; math::sqrt(acc / total) as f32 }).collect()
}

fn sine(n: usize) -> Vec<f32> {
    (0..n / 2).map(|i| math::sin(core::f64::consts::PI / n as f64 * (i as f64 + 0.5)) as f32).collect()
}

impl AacDecoder {
    pub fn new(sf_index: usize, layout: Layout) -> Result<AacDecoder> {
        if sf_index > 12 { return Err(Error::Invalid("aac: sampling frequency index")); }
        let sf_huff = Huff::new(&t::SF_CODE, &t::SF_BITS);
        let spec_huff = (0..11).map(|i| Huff::new(t::SPEC_CODES[i], t::SPEC_BITS[i])).collect();
        let pow43 = (0..8192).map(|i| math::pow(i as f64, 4.0 / 3.0) as f32).collect();
        let n = layout.channels;
        Ok(AacDecoder {
            sf_index,
            layout,
            sf_huff,
            spec_huff,
            pow43,
            imdct_long: crate::vorbis::Imdct::new(2048, [2048, 2048]),
            imdct_short: crate::vorbis::Imdct::new(256, [256, 256]),
            win: [[sine(2048), sine(256)], [kbd(2048, 4.0), kbd(256, 6.0)]],
            ov: vec![Overlap { saved: vec![0.0; 1024], prev_shape: 0 }; n],
            random: 0x1f2e_3d4c,
            chans: [Chan::new(), Chan::new()],
            out: vec![vec![0.0; 1024]; n],
            sbr_seen: false,
            pce: None,
            scratch: vec![0.0; 2048],
        })
    }

    /// Decode one raw_data_block from `r` (which is left after the ID_END and byte-aligned relative to
    /// `start`). Output: `self.out`, 1024 samples per channel, float with full scale 1.0.
    pub fn decode_block(&mut self, r: &mut BitReader, start: usize) -> Result<()> {
        for o in self.out.iter_mut() { o.iter_mut().for_each(|x| *x = 0.0); }
        let (mut nsce, mut ncpe, mut nlfe) = (0usize, 0usize, 0usize);
        let mut done = vec![false; self.layout.channels];
        loop {
            let id = r.read(3)?;
            match id {
                ID_END => break,
                ID_SCE | ID_LFE => {
                    let _tag = r.read(4)?;
                    self.decode_ics(r, 0, false)?;
                    let target = if id == ID_SCE { let t = self.layout.sce.get(nsce).copied(); nsce += 1; t } else { let t = self.layout.lfe.get(nlfe).copied(); nlfe += 1; t };
                    self.spectral_tools(0);
                    if let Some(ch) = target { self.synth(0, ch); done[ch] = true; }
                }
                ID_CPE => {
                    let _tag = r.read(4)?;
                    self.decode_cpe(r)?;
                    let target = self.layout.cpe.get(ncpe).copied();
                    ncpe += 1;
                    self.spectral_tools(0);
                    self.spectral_tools(1);
                    if let Some((a, b)) = target { self.synth(0, a); self.synth(1, b); done[a] = true; done[b] = true; }
                }
                ID_CCE => { self.skip_cce(r)?; }
                ID_DSE => {
                    let _tag = r.read(4)?;
                    let align = r.bit()?;
                    let mut cnt = r.read(8)? as usize;
                    if cnt == 255 { cnt += r.read(8)? as usize; }
                    if align { let rel = r.bit_pos() - start; r.skip((8 - rel % 8) % 8)?; }
                    r.skip(8 * cnt)?;
                }
                ID_PCE => {
                    let p = Pce::parse(r, start)?;
                    if self.pce.is_none() { self.pce = Some(p); }
                }
                ID_FIL => {
                    let mut cnt = r.read(4)? as usize;
                    if cnt == 15 { cnt += r.read(8)? as usize - 1; }
                    if cnt > 0 {
                        // extension_type: EXT_SBR_DATA (13) / EXT_SBR_DATA_CRC (14) mark HE-AAC
                        let ty = r.peek(4) as u32;
                        if ty == 13 || ty == 14 { self.sbr_seen = true; }
                    }
                    r.skip(8 * cnt)?;
                }
                _ => return Err(Error::Invalid("aac: element id")),
            }
        }
        // channels the layout names but the frame did not carry still run the filterbank (silence in,
        // overlap out), so a dropped element fades rather than clicks
        for ch in 0..self.layout.channels {
            if !done[ch] {
                self.chans[0].coef = [0.0; 1024];
                self.chans[0].ics = Ics { window_shape: self.ov[ch].prev_shape, ..Ics::default() };
                self.synth(0, ch);
            }
        }
        let rel = r.bit_pos() - start;
        r.skip((8 - rel % 8) % 8).ok();
        Ok(())
    }

    fn ics_info(&self, r: &mut BitReader) -> Result<Ics> {
        if r.bit()? { /* ics_reserved_bit: tolerated */ }
        let mut ics = Ics { window_sequence: r.read(2)? as u8, window_shape: r.read(1)? as u8, ..Ics::default() };
        let sfi = self.sf_index;
        if ics.window_sequence == EIGHT_SHORT {
            ics.max_sfb = r.read(4)? as usize;
            let grouping = r.read(7)?;
            ics.group_len = vec![1];
            for b in (0..7).rev() {
                if (grouping >> b) & 1 != 0 { *ics.group_len.last_mut().unwrap() += 1; } else { ics.group_len.push(1); }
            }
            ics.num_windows = 8;
            ics.swb = t::SWB_128[sfi];
            ics.num_swb = t::NUM_SWB_128[sfi] as usize;
            ics.tns_max_bands = t::TNS_MAX_BANDS_128[sfi] as usize;
        } else {
            ics.max_sfb = r.read(6)? as usize;
            ics.num_windows = 1;
            ics.group_len = vec![1];
            ics.swb = t::SWB_1024[sfi];
            ics.num_swb = t::NUM_SWB_1024[sfi] as usize;
            ics.tns_max_bands = t::TNS_MAX_BANDS_1024[sfi] as usize;
            ics.predictor = r.bit()?;
            if ics.predictor { return Err(Error::Unsupported("aac: prediction (AAC Main/LTP)")); }
        }
        if ics.max_sfb > ics.num_swb { return Err(Error::Invalid("aac: max_sfb")); }
        Ok(ics)
    }

    fn decode_cpe(&mut self, r: &mut BitReader) -> Result<()> {
        let common = r.bit()?;
        let mut ms_present = 0u32;
        let mut ms = [false; 512];
        if common {
            let ics = self.ics_info(r)?;
            ms_present = r.read(2)?;
            if ms_present == 3 { return Err(Error::Invalid("aac: ms_mask_present 3")); }
            let n = ics.group_len.len() * ics.max_sfb;
            if ms_present == 1 { for m in ms.iter_mut().take(n) { *m = r.bit()?; } }
            if ms_present == 2 { for m in ms.iter_mut().take(n) { *m = true; } }
            self.chans[0].ics = ics.clone();
            self.chans[1].ics = ics;
        }
        self.decode_ics(r, 0, common)?;
        self.decode_ics(r, 1, common)?;
        if common {
            // §4.6.13.3: PNS bands coded in both channels with ms_used carry the same noise (correlated)
            let [c0, c1] = &mut self.chans;
            let ics = &c0.ics;
            let mut gw = 0;
            for (g, &gl) in ics.group_len.iter().enumerate() {
                for sfb in 0..ics.max_sfb {
                    let i = g * ics.max_sfb + sfb;
                    let (a, b) = (ics.swb[sfb] as usize, ics.swb[sfb + 1] as usize);
                    let (t0, t1) = (c0.band_type[i], c1.band_type[i]);
                    if ms[i] && t0 == NOISE_HCB && t1 == NOISE_HCB {
                        let f = math::exp2(0.25 * (c1.sf[i] - c0.sf[i]) as f64) as f32;
                        for w in gw..gw + gl { for k in a..b { c1.coef[w * 128 + k] = c0.coef[w * 128 + k] * f; } }
                    } else if ms[i] && t0 < NOISE_HCB && t1 < NOISE_HCB {
                        // M/S (§4.6.8.1): not on noise or intensity bands
                        for w in gw..gw + gl {
                            for k in a..b {
                                let (l, rr) = (c0.coef[w * 128 + k], c1.coef[w * 128 + k]);
                                c0.coef[w * 128 + k] = l + rr;
                                c1.coef[w * 128 + k] = l - rr;
                            }
                        }
                    }
                }
                gw += gl;
            }
        }
        // intensity stereo (§4.6.8.2): the right channel's IS bands are scaled copies of the left
        {
            let [c0, c1] = &mut self.chans;
            let ics = &c1.ics;
            let mut gw = 0;
            for (g, &gl) in ics.group_len.iter().enumerate() {
                for sfb in 0..ics.max_sfb {
                    let i = g * ics.max_sfb + sfb;
                    let bt = c1.band_type[i];
                    if bt == INTENSITY_HCB || bt == INTENSITY_HCB2 {
                        let mut c = if bt == INTENSITY_HCB { 1.0f32 } else { -1.0 };
                        if ms_present == 1 && ms[i] { c = -c; }
                        let scale = c * math::exp2(-0.25 * c1.sf[i] as f64) as f32;
                        let (a, b) = (ics.swb[sfb] as usize, ics.swb[sfb + 1] as usize);
                        for w in gw..gw + gl { for k in a..b { c1.coef[w * 128 + k] = c0.coef[w * 128 + k] * scale; } }
                    }
                }
                gw += gl;
            }
        }
        Ok(())
    }

    /// individual_channel_stream (§4.4.2.7) into chans[ci], dequantised (and PNS filled).
    fn decode_ics(&mut self, r: &mut BitReader, ci: usize, common: bool) -> Result<()> {
        let global_gain = r.read(8)? as i32;
        if !common { let ics = self.ics_info(r)?; self.chans[ci].ics = ics; }
        let ics = self.chans[ci].ics.clone();
        let ng = ics.group_len.len();
        let ch = &mut self.chans[ci];
        // section_data
        let bits = if ics.window_sequence == EIGHT_SHORT { 3 } else { 5 };
        let esc = (1u32 << bits) - 1;
        ch.band_type = [0; 512];
        for g in 0..ng {
            let mut k = 0;
            while k < ics.max_sfb {
                let cb = r.read(4)? as u8;
                if cb == 12 { return Err(Error::Invalid("aac: reserved codebook")); }
                let mut len = 0usize;
                loop { let inc = r.read(bits)?; len += inc as usize; if inc != esc { break; } }
                if k + len > ics.max_sfb { return Err(Error::Invalid("aac: section past max_sfb")); }
                for b in k..k + len { ch.band_type[g * ics.max_sfb + b] = cb; }
                k += len;
                if len == 0 && r.bits_left() == 0 { return Err(Error::Eof); }
            }
        }
        // scale_factor_data (§4.4.2.7, Table 4.47)
        let mut sfv = global_gain;
        let mut noise = global_gain - 90;
        let mut is_pos = 0i32;
        let mut noise_first = true;
        ch.sf = [0; 512];
        for g in 0..ng {
            for sfb in 0..ics.max_sfb {
                let i = g * ics.max_sfb + sfb;
                match ch.band_type[i] {
                    ZERO_HCB => {}
                    INTENSITY_HCB | INTENSITY_HCB2 => {
                        is_pos += self.sf_huff.decode(r)? as i32 - 60;
                        ch.sf[i] = is_pos;
                    }
                    NOISE_HCB => {
                        if noise_first { noise_first = false; noise += r.read(9)? as i32 - 256; } else { noise += self.sf_huff.decode(r)? as i32 - 60; }
                        ch.sf[i] = noise;
                    }
                    _ => {
                        sfv += self.sf_huff.decode(r)? as i32 - 60;
                        if !(0..=255).contains(&sfv) { return Err(Error::Invalid("aac: scalefactor out of range")); }
                        ch.sf[i] = sfv;
                    }
                }
            }
        }
        // pulse_data
        let mut pulses: Vec<(usize, i32)> = vec![];
        if r.bit()? {
            if ics.window_sequence == EIGHT_SHORT { return Err(Error::Invalid("aac: pulse in short window")); }
            let n = r.read(2)? + 1;
            let start_sfb = r.read(6)? as usize;
            if start_sfb >= ics.num_swb { return Err(Error::Invalid("aac: pulse_start_sfb")); }
            let mut pos = ics.swb[start_sfb] as usize;
            for k in 0..n {
                let off = r.read(5)? as usize;
                pos += off;
                let _ = k;
                let amp = r.read(4)? as i32;
                if pos >= 1024 { return Err(Error::Invalid("aac: pulse position")); }
                pulses.push((pos, amp));
            }
        }
        // tns_data
        ch.tns_present = r.bit()?;
        if ch.tns_present {
            let short = ics.window_sequence == EIGHT_SHORT;
            let max_order = if short { 7 } else { 12 };
            for w in 0..ics.num_windows {
                let nf = r.read(if short { 1 } else { 2 })? as usize;
                ch.tns.n_filt[w] = nf as u8;
                if nf == 0 { continue; }
                let coef_res = r.read(1)?;
                for f in 0..nf {
                    ch.tns.length[w][f] = r.read(if short { 4 } else { 6 })? as u8;
                    let order = r.read(if short { 3 } else { 5 })? as usize;
                    if order > max_order { return Err(Error::Invalid("aac: tns order")); }
                    ch.tns.order[w][f] = order as u8;
                    if order == 0 { continue; }
                    ch.tns.direction[w][f] = r.bit()?;
                    let compress = r.read(1)?;
                    let res_bits = coef_res + 3;
                    let nbits = res_bits - compress;
                    // §4.6.9.3 tns_decode_coef
                    let iqfac = ((1u32 << (res_bits - 1)) as f64 - 0.5) / (core::f64::consts::PI / 2.0);
                    let iqfac_m = ((1u32 << (res_bits - 1)) as f64 + 0.5) / (core::f64::consts::PI / 2.0);
                    let mut tmp = [0f64; 20];
                    for item in tmp.iter_mut().take(order) {
                        let v = r.read(nbits)? as i32;
                        let v = (v << (32 - nbits)) >> (32 - nbits);
                        *item = math::sin(v as f64 / if v >= 0 { iqfac } else { iqfac_m });
                    }
                    let mut a = [0f64; 21];
                    a[0] = 1.0;
                    for m in 1..=order {
                        let mut b = a;
                        for i in 1..m { b[i] = a[i] + tmp[m - 1] * a[m - i]; }
                        a[1..m].copy_from_slice(&b[1..m]);
                        a[m] = tmp[m - 1];
                    }
                    for i in 0..order { ch.tns.lpc[w][f][i] = a[i + 1] as f32; }
                }
            }
        }
        // gain_control_data: AAC SSR only
        if r.bit()? { return Err(Error::Unsupported("aac: gain control (SSR)")); }
        // spectral_data (§4.4.2.7, Table 4.50) into quantised integers, window-major (w*128 + k)
        let mut q = [0i32; 1024];
        let mut gw = 0usize;
        for g in 0..ng {
            let gl = ics.group_len[g];
            for sfb in 0..ics.max_sfb {
                let cb = ch.band_type[g * ics.max_sfb + sfb];
                if cb == ZERO_HCB || cb >= NOISE_HCB { continue; }
                let hc = &self.spec_huff[cb as usize - 1];
                let (a, b) = (ics.swb[sfb] as usize, ics.swb[sfb + 1] as usize);
                for w in gw..gw + gl {
                    let base = w * 128;
                    let mut k = a;
                    while k < b {
                        let idx = hc.decode(r)? as i32;
                        match cb {
                            1 | 2 => {
                                q[base + k] = idx / 27 - 1;
                                q[base + k + 1] = (idx / 9) % 3 - 1;
                                q[base + k + 2] = (idx / 3) % 3 - 1;
                                q[base + k + 3] = idx % 3 - 1;
                                k += 4;
                            }
                            3 | 4 => {
                                let v = [idx / 27, (idx / 9) % 3, (idx / 3) % 3, idx % 3];
                                for (j, &x) in v.iter().enumerate() {
                                    q[base + k + j] = if x != 0 && r.bit()? { -x } else { x };
                                }
                                k += 4;
                            }
                            5 | 6 => {
                                q[base + k] = idx / 9 - 4;
                                q[base + k + 1] = idx % 9 - 4;
                                k += 2;
                            }
                            _ => {
                                let m = match cb { 7 | 8 => 8, 9 | 10 => 13, _ => 17 };
                                let v = [idx / m, idx % m];
                                let mut s = [false; 2];
                                for j in 0..2 { if v[j] != 0 { s[j] = r.bit()?; } }
                                for j in 0..2 {
                                    let mut x = v[j];
                                    if cb == ESC_HCB && x == 16 {
                                        // escape: N ones, a zero, then N+4 bits; value 2^(N+4) + bits
                                        let mut n = 0u32;
                                        while r.bit()? { n += 1; if n > 8 { return Err(Error::Invalid("aac: escape too long")); } }
                                        x = (1 << (n + 4)) + r.read(n + 4)? as i32;
                                    }
                                    q[base + k + j] = if s[j] { -x } else { x };
                                }
                                k += 2;
                            }
                        }
                    }
                }
            }
            gw += gl;
        }
        for &(p, amp) in &pulses {
            if q[p] > 0 { q[p] += amp; } else { q[p] -= amp; }
        }
        // inverse quantisation and scaling (§4.6.1.3, §4.6.2.3); PNS (§4.6.13)
        ch.coef = [0.0; 1024];
        let mut gw = 0usize;
        for g in 0..ng {
            let gl = ics.group_len[g];
            for sfb in 0..ics.max_sfb {
                let i = g * ics.max_sfb + sfb;
                let cb = ch.band_type[i];
                let (a, b) = (ics.swb[sfb] as usize, ics.swb[sfb + 1] as usize);
                if cb == NOISE_HCB {
                    let gain = math::exp2(0.25 * ch.sf[i] as f64);
                    for w in gw..gw + gl {
                        let mut e = 0f64;
                        for k in a..b {
                            self.random = self.random.wrapping_mul(1664525).wrapping_add(1013904223);
                            let v = self.random as i32 as f32;
                            ch.coef[w * 128 + k] = v;
                            e += v as f64 * v as f64;
                        }
                        let s = if e > 0.0 { (gain / math::sqrt(e)) as f32 } else { 0.0 };
                        for k in a..b { ch.coef[w * 128 + k] *= s; }
                    }
                } else if cb != ZERO_HCB && cb < NOISE_HCB {
                    let gain = math::exp2(0.25 * (ch.sf[i] - 100) as f64) as f32;
                    for w in gw..gw + gl {
                        for k in a..b {
                            let x = q[w * 128 + k];
                            let m = x.unsigned_abs() as usize;
                            let v = if m < 8192 { self.pow43[m] } else { math::pow(m as f64, 4.0 / 3.0) as f32 };
                            ch.coef[w * 128 + k] = if x < 0 { -v * gain } else { v * gain };
                        }
                    }
                }
            }
            gw += gl;
        }
        Ok(())
    }

    /// TNS (§4.6.9) on chans[ci], after the stereo tools.
    fn spectral_tools(&mut self, ci: usize) {
        let ch = &mut self.chans[ci];
        if !ch.tns_present { return; }
        let ics = &ch.ics;
        let mmm = ics.tns_max_bands.min(ics.max_sfb);
        if mmm == 0 { return; }
        for w in 0..ics.num_windows {
            let mut bottom = ics.num_swb;
            for f in 0..ch.tns.n_filt[w] as usize {
                let top = bottom;
                bottom = top.saturating_sub(ch.tns.length[w][f] as usize);
                let order = ch.tns.order[w][f] as usize;
                if order == 0 { continue; }
                let start = ics.swb[bottom.min(mmm)] as usize;
                let end = ics.swb[top.min(mmm)] as usize;
                if end <= start { continue; }
                let size = end - start;
                let lpc = &ch.tns.lpc[w][f];
                let base = w * 128;
                // all-pole filter in the frequency direction: y[n] = x[n] - Σ lpc[i]·y[n-i]
                if ch.tns.direction[w][f] {
                    for m in 0..size {
                        let n = base + end - 1 - m;
                        let mut y = ch.coef[n];
                        for i in 1..=order.min(m) { y -= lpc[i - 1] * ch.coef[n + i]; }
                        ch.coef[n] = y;
                    }
                } else {
                    for m in 0..size {
                        let n = base + start + m;
                        let mut y = ch.coef[n];
                        for i in 1..=order.min(m) { y -= lpc[i - 1] * ch.coef[n - i]; }
                        ch.coef[n] = y;
                    }
                }
            }
        }
    }

    /// Filterbank (§4.6.11): IMDCT, windowing by sequence and shape, overlap-add into out[och].
    fn synth(&mut self, ci: usize, och: usize) {
        let ch = &self.chans[ci];
        let seq = ch.ics.window_sequence;
        let shape = ch.ics.window_shape as usize;
        let prev = self.ov[och].prev_shape as usize;
        // spec scale: x = (2/N)·Σ, N = 2048 (long) or 256 (short); float full scale is 32768
        let z = &mut self.scratch;
        z.iter_mut().for_each(|x| *x = 0.0);
        let wl_prev = &self.win[prev][0];
        let wl = &self.win[shape][0];
        let ws_prev = &self.win[prev][1];
        let ws = &self.win[shape][1];
        if seq == EIGHT_SHORT {
            let mut y = [0f32; 256];
            let k = 2.0 / 256.0 / 32768.0;
            for w in 0..8 {
                self.imdct_short.inverse(&ch.coef[w * 128..w * 128 + 128], &mut y);
                let left = if w == 0 { ws_prev } else { ws };
                for n in 0..128 { z[448 + w * 128 + n] += y[n] * left[n] * k; }
                for n in 0..128 { z[448 + w * 128 + 128 + n] += y[128 + n] * ws[127 - n] * k; }
            }
        } else {
            let mut y = vec![0f32; 2048];
            self.imdct_long.inverse(&ch.coef, &mut y);
            let k = 2.0 / 2048.0 / 32768.0;
            match seq {
                ONLY_LONG => {
                    for n in 0..1024 { z[n] = y[n] * wl_prev[n] * k; }
                    for n in 0..1024 { z[1024 + n] = y[1024 + n] * wl[1023 - n] * k; }
                }
                LONG_START => {
                    for n in 0..1024 { z[n] = y[n] * wl_prev[n] * k; }
                    for n in 1024..1472 { z[n] = y[n] * k; }
                    for n in 0..128 { z[1472 + n] = y[1472 + n] * ws[127 - n] * k; }
                }
                _ => {
                    debug_assert_eq!(seq, LONG_STOP);
                    for n in 0..128 { z[448 + n] = y[448 + n] * ws_prev[n] * k; }
                    for n in 576..1024 { z[n] = y[n] * k; }
                    for n in 0..1024 { z[1024 + n] = y[1024 + n] * wl[1023 - n] * k; }
                }
            }
        }
        let o = &mut self.ov[och];
        let out = &mut self.out[och];
        for n in 0..1024 { out[n] = z[n] + o.saved[n]; }
        o.saved.copy_from_slice(&z[1024..2048]);
        o.prev_shape = shape as u8;
    }

    /// coupling_channel_element (§4.4.2.3): parsed so the frame stays in step; the coupling itself is not
    /// applied (no encoder in use emits CCE; owed).
    fn skip_cce(&mut self, r: &mut BitReader) -> Result<()> {
        let _tag = r.read(4)?;
        let ind_sw = r.bit()?;
        let num_coupled = r.read(3)?;
        let mut num_gain = 0;
        for _ in 0..=num_coupled {
            num_gain += 1;
            let is_cpe = r.bit()?;
            let _sel = r.read(4)?;
            if is_cpe {
                let l = r.bit()?;
                let rr = r.bit()?;
                if l && rr { num_gain += 1; }
            }
        }
        let _domain = r.bit()?;
        let _sign = r.bit()?;
        let _scale = r.read(2)?;
        // decode the ICS into slot 1's scratch (its result is discarded)
        let save = core::mem::replace(&mut self.chans[1].coef, [0.0; 1024]);
        self.decode_ics(r, 1, false)?;
        self.chans[1].coef = save;
        let ics = self.chans[1].ics.clone();
        for c in 1..num_gain {
            let cge = if ind_sw { true } else { r.bit()? };
            if cge { self.sf_huff.decode(r)?; } else {
                for g in 0..ics.group_len.len() {
                    for sfb in 0..ics.max_sfb {
                        if self.chans[1].band_type[g * ics.max_sfb + sfb] != ZERO_HCB { self.sf_huff.decode(r)?; }
                    }
                }
            }
            let _ = c;
        }
        Ok(())
    }
}
