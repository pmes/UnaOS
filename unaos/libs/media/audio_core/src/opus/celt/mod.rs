//! CELT, the MDCT layer of Opus (RFC 6716 §4.3), fixed point exactly as the normative reference decoder
//! (`celt/celt_decoder.c`) computes it: frame header flags, energy, allocation, PVQ shapes, the inverse
//! MDCT, the pitch post-filter, de-emphasis, and both packet-loss concealments (pitch-based and noise).
pub mod bands;
pub mod energy;
pub mod fft;
pub mod fixed;
pub mod lpc;
pub mod rate;
pub mod tables;

use super::range::{RangeDecoder, BITRES};
use bands::*;
use fixed::*;
use rate::*;
use tables::*;
use alloc::vec;
use alloc::vec::Vec;

const DECODE_BUFFER_SIZE: usize = 2048;
const LPC_ORDER: usize = 24;
const MAX_PERIOD: usize = 1024;
const PLC_PITCH_LAG_MAX: usize = 720;
const PLC_PITCH_LAG_MIN: usize = 100;
const COMBFILTER_MINPERIOD: i32 = 15;
const SPREAD_NORMAL: i32 = 2;

pub struct CeltDecoder {
    pub channels: usize,
    pub stream_channels: usize,
    pub start: usize,
    pub end: usize,
    pub disable_inv: bool,
    pub rng: u32,
    pub error: bool,
    last_pitch_index: i32,
    loss_duration: i32,
    skip_plc: bool,
    postfilter_period: i32,
    postfilter_period_old: i32,
    postfilter_gain: i32,
    postfilter_gain_old: i32,
    postfilter_tapset: i32,
    postfilter_tapset_old: i32,
    prefilter_and_fold: bool,
    preemph_mem: [i32; 2],
    decode_mem: [Vec<i32>; 2],
    lpc: [[i16; LPC_ORDER]; 2],
    old_band_e: [i16; 2 * NB_EBANDS],
    old_log_e: [i16; 2 * NB_EBANDS],
    old_log_e2: [i16; 2 * NB_EBANDS],
    background_log_e: [i16; 2 * NB_EBANDS],
}

#[allow(clippy::too_many_arguments)]
fn comb_filter(x: &mut [i32], x_off: usize, mut out: Option<&mut [i32]>, t0: i32, t1: i32, n: usize, g0: i32, g1: i32, tapset0: i32, tapset1: i32, window: &[i16], overlap: usize) {
    const GAINS: [[i32; 3]; 3] = [[10048, 7112, 4248], [15200, 8784, 0], [26208, 3280, 0]];
    if g0 == 0 && g1 == 0 {
        if let Some(o) = out.as_deref_mut() { o[..n].copy_from_slice(&x[x_off..x_off + n]); }
        return;
    }
    let t0 = t0.max(COMBFILTER_MINPERIOD) as usize;
    let t1 = t1.max(COMBFILTER_MINPERIOD) as usize;
    let g00 = mult16_16_p15(g0, GAINS[tapset0 as usize][0]);
    let g01 = mult16_16_p15(g0, GAINS[tapset0 as usize][1]);
    let g02 = mult16_16_p15(g0, GAINS[tapset0 as usize][2]);
    let g10 = mult16_16_p15(g1, GAINS[tapset1 as usize][0]);
    let g11 = mult16_16_p15(g1, GAINS[tapset1 as usize][1]);
    let g12 = mult16_16_p15(g1, GAINS[tapset1 as usize][2]);
    let b = x_off;
    let mut x1 = x[b + 1 - t1];
    let mut x2 = x[b - t1];
    let mut x3 = x[b - t1 - 1];
    let mut x4 = x[b - t1 - 2];
    let overlap = if g0 == g1 && t0 == t1 && tapset0 == tapset1 { 0 } else { overlap };
    let mut i = 0usize;
    while i < overlap {
        let x0 = x[b + i + 2 - t1];
        let f = mult16_16_q15(window[i] as i32, window[i] as i32);
        let mut y = x[b + i]
            .wrapping_add(mult16_32_q15(mult16_16_q15(Q15ONE - f, g00), x[b + i - t0]))
            .wrapping_add(mult16_32_q15(mult16_16_q15(Q15ONE - f, g01), x[b + i + 1 - t0].wrapping_add(x[b + i - t0 - 1])))
            .wrapping_add(mult16_32_q15(mult16_16_q15(Q15ONE - f, g02), x[b + i + 2 - t0].wrapping_add(x[b + i - t0 - 2])))
            .wrapping_add(mult16_32_q15(mult16_16_q15(f, g10), x2))
            .wrapping_add(mult16_32_q15(mult16_16_q15(f, g11), x1.wrapping_add(x3)))
            .wrapping_add(mult16_32_q15(mult16_16_q15(f, g12), x0.wrapping_add(x4)));
        y = saturate(y, SIG_SAT);
        match out.as_deref_mut() { Some(o) => o[i] = y, None => x[b + i] = y }
        x4 = x3; x3 = x2; x2 = x1; x1 = x0;
        i += 1;
    }
    if g1 == 0 {
        if let Some(o) = out.as_deref_mut() { o[overlap..n].copy_from_slice(&x[b + overlap..b + n]); }
        return;
    }
    // comb_filter_const_c (generic C version)
    let t = t1;
    let mut x4 = x[b + i - t - 2];
    let mut x3 = x[b + i - t - 1];
    let mut x2 = x[b + i - t];
    let mut x1 = x[b + i + 1 - t];
    while i < n {
        let x0 = x[b + i + 2 - t];
        let mut y = x[b + i]
            .wrapping_add(mult16_32_q15(g10, x2))
            .wrapping_add(mult16_32_q15(g11, x1.wrapping_add(x3)))
            .wrapping_add(mult16_32_q15(g12, x0.wrapping_add(x4)));
        y = saturate(y, SIG_SAT);
        match out.as_deref_mut() { Some(o) => o[i] = y, None => x[b + i] = y }
        x4 = x3; x3 = x2; x2 = x1; x1 = x0;
        i += 1;
    }
}

fn tf_decode(start: usize, end: usize, is_transient: bool, tf_res: &mut [i32], lm: i32, dec: &mut RangeDecoder) {
    let mut budget = dec.storage * 8;
    let mut tell = dec.tell() as u32;
    let mut logp = if is_transient { 2 } else { 4 };
    let tf_select_rsv = lm > 0 && tell + logp + 1 <= budget;
    budget -= tf_select_rsv as u32;
    let mut tf_changed = 0;
    let mut curr = 0;
    for i in start..end {
        if tell + logp <= budget {
            curr ^= dec.bit_logp(logp) as i32;
            tell = dec.tell() as u32;
            tf_changed |= curr;
        }
        tf_res[i] = curr;
        logp = if is_transient { 4 } else { 5 };
    }
    let mut tf_select = 0;
    let it = 4 * is_transient as usize;
    if tf_select_rsv && TF_SELECT_TABLE[lm as usize][it + tf_changed as usize] != TF_SELECT_TABLE[lm as usize][it + 2 + tf_changed as usize] {
        tf_select = dec.bit_logp(1) as usize;
    }
    for i in start..end {
        tf_res[i] = TF_SELECT_TABLE[lm as usize][it + 2 * tf_select + tf_res[i] as usize] as i32;
    }
}

impl CeltDecoder {
    pub fn new(channels: usize) -> CeltDecoder {
        let mut d = CeltDecoder {
            channels,
            stream_channels: channels,
            start: 0,
            end: NB_EBANDS,
            disable_inv: channels == 1,
            rng: 0,
            error: false,
            last_pitch_index: 0,
            loss_duration: 0,
            skip_plc: false,
            postfilter_period: 0,
            postfilter_period_old: 0,
            postfilter_gain: 0,
            postfilter_gain_old: 0,
            postfilter_tapset: 0,
            postfilter_tapset_old: 0,
            prefilter_and_fold: false,
            preemph_mem: [0; 2],
            decode_mem: [vec![0; DECODE_BUFFER_SIZE + OVERLAP], vec![0; DECODE_BUFFER_SIZE + OVERLAP]],
            lpc: [[0; LPC_ORDER]; 2],
            old_band_e: [0; 2 * NB_EBANDS],
            old_log_e: [0; 2 * NB_EBANDS],
            old_log_e2: [0; 2 * NB_EBANDS],
            background_log_e: [0; 2 * NB_EBANDS],
        };
        d.reset();
        d
    }

    /// `OPUS_RESET_STATE`.
    pub fn reset(&mut self) {
        self.rng = 0;
        self.error = false;
        self.last_pitch_index = 0;
        self.loss_duration = 0;
        self.postfilter_period = 0;
        self.postfilter_period_old = 0;
        self.postfilter_gain = 0;
        self.postfilter_gain_old = 0;
        self.postfilter_tapset = 0;
        self.postfilter_tapset_old = 0;
        self.prefilter_and_fold = false;
        self.preemph_mem = [0; 2];
        for m in self.decode_mem.iter_mut() { for v in m.iter_mut() { *v = 0; } }
        self.lpc = [[0; LPC_ORDER]; 2];
        self.old_band_e = [0; 2 * NB_EBANDS];
        self.background_log_e = [0; 2 * NB_EBANDS];
        self.old_log_e = [-(28 << DB_SHIFT) as i16; 2 * NB_EBANDS];
        self.old_log_e2 = [-(28 << DB_SHIFT) as i16; 2 * NB_EBANDS];
        self.skip_plc = true;
    }

    fn deemphasis(&mut self, n: usize, pcm: &mut [i16], accum: bool) {
        let cc = self.channels;
        let coef0 = PREEMPH0;
        for c in 0..cc {
            let mut m = self.preemph_mem[c];
            let base = DECODE_BUFFER_SIZE - n;
            let x = &self.decode_mem[c];
            for j in 0..n {
                let tmp = x[base + j].wrapping_add(m);
                m = mult16_32_q15(coef0, tmp);
                if accum {
                    pcm[j * cc + c] = sat16(pcm[j * cc + c] as i32 + sig2word16(tmp) as i32) as i16;
                } else {
                    pcm[j * cc + c] = sig2word16(tmp);
                }
            }
            self.preemph_mem[c] = m;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn synthesis(&mut self, x: &[i16], old_band_e: &[i16], start: usize, eff_end: usize, c: usize, cc: usize, is_transient: bool, lm: i32, silence: bool) {
        let m = 1i32 << lm;
        let n = SHORT_MDCT_SIZE << lm;
        let (b, nb, shift) = if is_transient { (m as usize, SHORT_MDCT_SIZE, MAX_LM as usize) } else { (1, SHORT_MDCT_SIZE << lm, (MAX_LM - lm) as usize) };
        let mut freq = vec![0i32; n];
        let out_off = DECODE_BUFFER_SIZE - n;
        if cc == 2 && c == 1 {
            denormalise_bands(x, &mut freq, old_band_e, start, eff_end, m, 1, silence);
            let freq2 = freq.clone();
            for bi in 0..b { fft::mdct_backward(&freq2, bi, &mut self.decode_mem[0][out_off + nb * bi..], &WINDOW120, OVERLAP, shift, b); }
            for bi in 0..b { fft::mdct_backward(&freq, bi, &mut self.decode_mem[1][out_off + nb * bi..], &WINDOW120, OVERLAP, shift, b); }
        } else if cc == 1 && c == 2 {
            let mut freq2 = vec![0i32; n];
            denormalise_bands(x, &mut freq, old_band_e, start, eff_end, m, 1, silence);
            denormalise_bands(&x[n..], &mut freq2, &old_band_e[NB_EBANDS..], start, eff_end, m, 1, silence);
            for i in 0..n { freq[i] = (freq[i] >> 1).wrapping_add(freq2[i] >> 1); }
            for bi in 0..b { fft::mdct_backward(&freq, bi, &mut self.decode_mem[0][out_off + nb * bi..], &WINDOW120, OVERLAP, shift, b); }
        } else {
            for ch in 0..cc {
                denormalise_bands(&x[ch * n..], &mut freq, &old_band_e[ch * NB_EBANDS..], start, eff_end, m, 1, silence);
                for bi in 0..b { fft::mdct_backward(&freq, bi, &mut self.decode_mem[ch][out_off + nb * bi..], &WINDOW120, OVERLAP, shift, b); }
            }
        }
        for ch in 0..cc {
            for v in self.decode_mem[ch][out_off..out_off + n].iter_mut() { *v = saturate(*v, SIG_SAT); }
        }
    }

    fn prefilter_and_fold(&mut self, n: usize) {
        let overlap = OVERLAP;
        let mut etmp = vec![0i32; overlap];
        for c in 0..self.channels {
            let off = DECODE_BUFFER_SIZE - n;
            comb_filter(&mut self.decode_mem[c], off, Some(&mut etmp), self.postfilter_period_old, self.postfilter_period, overlap,
                -self.postfilter_gain_old, -self.postfilter_gain, self.postfilter_tapset_old, self.postfilter_tapset, &[], 0);
            for i in 0..overlap / 2 {
                self.decode_mem[c][off + i] = mult16_32_q15(WINDOW120[i] as i32, etmp[overlap - 1 - i])
                    .wrapping_add(mult16_32_q15(WINDOW120[overlap - i - 1] as i32, etmp[i]));
            }
        }
    }

    fn plc_pitch_search(&self, c: usize) -> i32 {
        let mut lp = vec![0i16; DECODE_BUFFER_SIZE >> 1];
        let mems: Vec<&[i32]> = (0..c).map(|ch| &self.decode_mem[ch][..]).collect();
        lpc::pitch_downsample(&mems, &mut lp, DECODE_BUFFER_SIZE);
        let pi = lpc::pitch_search(&lp[PLC_PITCH_LAG_MAX >> 1..], &lp, DECODE_BUFFER_SIZE - PLC_PITCH_LAG_MAX, PLC_PITCH_LAG_MAX - PLC_PITCH_LAG_MIN);
        PLC_PITCH_LAG_MAX as i32 - pi
    }

    fn decode_lost(&mut self, n: usize, lm: i32) {
        let c = self.channels;
        let overlap = OVERLAP;
        let loss_duration = self.loss_duration;
        let start = self.start;
        let noise_based = loss_duration >= 40 || start != 0 || self.skip_plc;
        if noise_based {
            let end = self.end;
            let eff_end = start.max(end.min(NB_EBANDS));
            let mut x = vec![0i16; c * n];
            for ch in 0..c { self.decode_mem[ch].copy_within(n..DECODE_BUFFER_SIZE + overlap, 0); }
            if self.prefilter_and_fold { self.prefilter_and_fold(n); }
            let decay = if loss_duration == 0 { 1536 } else { 512 };
            for ch in 0..c {
                for i in start..end {
                    let k = ch * NB_EBANDS + i;
                    self.old_band_e[k] = (self.background_log_e[k] as i32).max(self.old_band_e[k] as i32 - decay) as i16;
                }
            }
            let mut seed = self.rng;
            for ch in 0..c {
                for i in start..eff_end {
                    let boffs = n * ch + ((EBAND5MS[i] as usize) << lm);
                    let blen = ((EBAND5MS[i + 1] - EBAND5MS[i]) as usize) << lm;
                    for j in 0..blen {
                        seed = lcg_rand(seed);
                        x[boffs + j] = ((seed as i32) >> 20) as i16;
                    }
                    renormalise_vector(&mut x[boffs..boffs + blen], blen, Q15ONE);
                }
            }
            self.rng = seed;
            let obe = self.old_band_e;
            self.synthesis(&x, &obe, start, eff_end, c, c, false, lm, false);
            self.prefilter_and_fold = false;
            self.skip_plc = true;
        } else {
            let mut fade = Q15ONE;
            let pitch_index;
            if loss_duration == 0 {
                pitch_index = self.plc_pitch_search(c);
                self.last_pitch_index = pitch_index;
            } else {
                pitch_index = self.last_pitch_index;
                fade = 26214;
            }
            let pitch_index = pitch_index as usize;
            let exc_length = (2 * pitch_index).min(MAX_PERIOD);
            let mut exc_buf = vec![0i16; MAX_PERIOD + LPC_ORDER];
            let mut fir_tmp = vec![0i16; exc_length];
            let window = &WINDOW120;
            for ch in 0..c {
                let mut s1 = 0i32;
                {
                    let buf = &self.decode_mem[ch];
                    for i in 0..MAX_PERIOD + LPC_ORDER {
                        exc_buf[i] = sround16(buf[DECODE_BUFFER_SIZE - MAX_PERIOD - LPC_ORDER + i], SIG_SHIFT) as i16;
                    }
                }
                let exc0 = LPC_ORDER; // exc = exc_buf + LPC_ORDER
                if loss_duration == 0 {
                    let mut ac = [0i32; LPC_ORDER + 1];
                    lpc::celt_autocorr(&exc_buf[exc0..], &mut ac, Some(window), overlap, LPC_ORDER, MAX_PERIOD);
                    ac[0] = ac[0].wrapping_add(ac[0] >> 13);
                    for i in 1..=LPC_ORDER { ac[i] = ac[i].wrapping_sub(mult16_32_q15((2 * i * i) as i32, ac[i])); }
                    lpc::celt_lpc(&mut self.lpc[ch], &ac, LPC_ORDER);
                    loop {
                        let mut tmp = Q15ONE;
                        let mut sum = 1i32 << SIG_SHIFT;
                        for i in 0..LPC_ORDER { sum += (self.lpc[ch][i] as i32).abs(); }
                        if sum < 65535 { break; }
                        for i in 0..LPC_ORDER {
                            tmp = mult16_16_q15(32440, tmp);
                            self.lpc[ch][i] = mult16_16_q15(self.lpc[ch][i] as i32, tmp) as i16;
                        }
                    }
                }
                {
                    let lpcc = self.lpc[ch];
                    lpc::celt_fir(&exc_buf, exc0 + MAX_PERIOD - exc_length, &lpcc, &mut fir_tmp, exc_length, LPC_ORDER);
                    exc_buf[exc0 + MAX_PERIOD - exc_length..exc0 + MAX_PERIOD].copy_from_slice(&fir_tmp);
                }
                let decay;
                {
                    let mut e1 = 1i32;
                    let mut e2 = 1i32;
                    let shift = 0.max(2 * celt_zlog2(celt_maxabs16(&exc_buf[exc0 + MAX_PERIOD - exc_length..exc0 + MAX_PERIOD])) - 20);
                    let decay_length = exc_length >> 1;
                    for i in 0..decay_length {
                        let e = exc_buf[exc0 + MAX_PERIOD - decay_length + i] as i32;
                        e1 = e1.wrapping_add(mult16_16(e, e) >> shift);
                        let e = exc_buf[exc0 + MAX_PERIOD - 2 * decay_length + i] as i32;
                        e2 = e2.wrapping_add(mult16_16(e, e) >> shift);
                    }
                    e1 = e1.min(e2);
                    decay = celt_sqrt(frac_div32(e1 >> 1, e2)) as i16 as i32;
                }
                let buf = &mut self.decode_mem[ch];
                buf.copy_within(n..DECODE_BUFFER_SIZE, 0);
                let extrapolation_offset = MAX_PERIOD - pitch_index;
                let extrapolation_len = n + overlap;
                let mut attenuation = mult16_16_q15(fade, decay) as i16 as i32;
                let mut j = 0usize;
                for i in 0..extrapolation_len {
                    if j >= pitch_index {
                        j -= pitch_index;
                        attenuation = mult16_16_q15(attenuation, decay) as i16 as i32;
                    }
                    buf[DECODE_BUFFER_SIZE - n + i] = shl32(mult16_16_q15(attenuation, exc_buf[exc0 + extrapolation_offset + j] as i32) as i16 as i32, SIG_SHIFT);
                    let tmp = sround16(buf[DECODE_BUFFER_SIZE - MAX_PERIOD - n + extrapolation_offset + j], SIG_SHIFT);
                    s1 = s1.wrapping_add(mult16_16(tmp, tmp) >> 10);
                    j += 1;
                }
                {
                    let mut lpc_mem = [0i16; LPC_ORDER];
                    for i in 0..LPC_ORDER { lpc_mem[i] = sround16(buf[DECODE_BUFFER_SIZE - n - 1 - i], SIG_SHIFT) as i16; }
                    let lpcc = self.lpc[ch];
                    lpc::celt_iir(buf, DECODE_BUFFER_SIZE - n, &lpcc, extrapolation_len, LPC_ORDER, &mut lpc_mem);
                    for i in 0..extrapolation_len { buf[DECODE_BUFFER_SIZE - n + i] = saturate(buf[DECODE_BUFFER_SIZE - n + i], SIG_SAT); }
                }
                {
                    let mut s2 = 0i32;
                    for i in 0..extrapolation_len {
                        let tmp = sround16(buf[DECODE_BUFFER_SIZE - n + i], SIG_SHIFT);
                        s2 = s2.wrapping_add(mult16_16(tmp, tmp) >> 10);
                    }
                    if !(s1 > s2 >> 2) {
                        for i in 0..extrapolation_len { buf[DECODE_BUFFER_SIZE - n + i] = 0; }
                    } else if s1 < s2 {
                        let ratio = celt_sqrt(frac_div32((s1 >> 1) + 1, s2 + 1)) as i16 as i32;
                        for i in 0..overlap {
                            let tmp_g = (Q15ONE - mult16_16_q15(window[i] as i32, Q15ONE - ratio)) as i16 as i32;
                            buf[DECODE_BUFFER_SIZE - n + i] = mult16_32_q15(tmp_g, buf[DECODE_BUFFER_SIZE - n + i]);
                        }
                        for i in overlap..extrapolation_len {
                            buf[DECODE_BUFFER_SIZE - n + i] = mult16_32_q15(ratio, buf[DECODE_BUFFER_SIZE - n + i]);
                        }
                    }
                }
            }
            self.prefilter_and_fold = true;
        }
        self.loss_duration = 10000.min(loss_duration + (1 << lm));
    }

    /// `celt_decode_with_ec`: decode one CELT frame of `frame_size` samples (48 kHz) into interleaved `pcm`.
    /// `data` None (or ≤ 1 byte) runs the concealment. `ext` is the range decoder a hybrid frame shares
    /// with SILK. Returns the frame size.
    pub fn decode<'a, 'b>(&mut self, data: Option<&'a [u8]>, pcm: &mut [i16], frame_size: usize, ext: Option<&'b mut RangeDecoder<'a>>, accum: bool) -> crate::Result<usize> {
        let cc = self.channels;
        let c = self.stream_channels;
        let start = self.start;
        let end = self.end;
        let mut lm = 0i32;
        while lm <= MAX_LM { if SHORT_MDCT_SIZE << lm == frame_size { break; } lm += 1; }
        if lm > MAX_LM { return Err(crate::Error::Invalid("CELT frame size")); }
        let m = 1i32 << lm;
        let len = data.map(|d| d.len()).unwrap_or(0);
        if len > 1275 { return Err(crate::Error::Invalid("CELT frame too long")); }
        let n = (m as usize) * SHORT_MDCT_SIZE;
        let eff_end = end.min(NB_EBANDS);
        let data = match data {
            Some(d) if d.len() > 1 => d,
            _ => {
                self.decode_lost(n, lm);
                self.deemphasis(n, pcm, accum);
                return Ok(frame_size);
            }
        };
        if self.loss_duration == 0 { self.skip_plc = false; }
        let mut own;
        let dec: &mut RangeDecoder = match ext {
            Some(d) => d,
            None => { own = RangeDecoder::new(data); &mut own }
        };
        if c == 1 {
            for i in 0..NB_EBANDS { self.old_band_e[i] = self.old_band_e[i].max(self.old_band_e[NB_EBANDS + i]); }
        }
        let mut total_bits = (len * 8) as i32;
        let mut tell = dec.tell();
        let silence;
        if tell >= total_bits { silence = true; }
        else if tell == 1 { silence = dec.bit_logp(15); }
        else { silence = false; }
        if silence {
            tell = (len * 8) as i32;
            dec.nbits_total += tell - dec.tell();
        }
        let mut postfilter_gain = 0;
        let mut postfilter_pitch = 0;
        let mut postfilter_tapset = 0;
        if start == 0 && tell + 16 <= total_bits {
            if dec.bit_logp(1) {
                let octave = dec.dec_uint(6) as i32;
                postfilter_pitch = (16 << octave) + dec.bits((4 + octave) as u32) as i32 - 1;
                let qg = dec.bits(3) as i32;
                if dec.tell() + 2 <= total_bits { postfilter_tapset = dec.icdf(&TAPSET_ICDF, 2) as i32; }
                postfilter_gain = 3072 * (qg + 1);
            }
            tell = dec.tell();
        }
        let is_transient;
        if lm > 0 && tell + 3 <= total_bits {
            is_transient = dec.bit_logp(3);
            tell = dec.tell();
        } else {
            is_transient = false;
        }
        let short_blocks = is_transient;
        let intra_ener = if tell + 3 <= total_bits { dec.bit_logp(3) } else { false };
        if !intra_ener && self.loss_duration != 0 {
            for ch in 0..2 {
                let missing = 10.min(self.loss_duration >> lm);
                let safety = if lm == 0 { 1536 } else if lm == 1 { 512 } else { 0 };
                for i in start..end {
                    let k = ch * NB_EBANDS + i;
                    if (self.old_band_e[k] as i32) < (self.old_log_e[k] as i32).max(self.old_log_e2[k] as i32) {
                        let mut e0 = self.old_band_e[k] as i32;
                        let e1 = self.old_log_e[k] as i32;
                        let e2 = self.old_log_e2[k] as i32;
                        let slope = (e1 - e0).max((e2 - e0) >> 1);
                        e0 -= 0.max((1 + missing) * slope);
                        self.old_band_e[k] = (-(20 << DB_SHIFT)).max(e0) as i16;
                    } else {
                        self.old_band_e[k] = self.old_band_e[k].min(self.old_log_e[k]).min(self.old_log_e2[k]);
                    }
                    self.old_band_e[k] = (self.old_band_e[k] as i32 - safety) as i16;
                }
            }
        }
        energy::unquant_coarse(start, end, &mut self.old_band_e, intra_ener, dec, c, lm);
        let mut tf_res = [0i32; NB_EBANDS];
        tf_decode(start, end, is_transient, &mut tf_res, lm, dec);
        tell = dec.tell();
        let mut spread_decision = SPREAD_NORMAL;
        if tell + 4 <= total_bits { spread_decision = dec.icdf(&SPREAD_ICDF, 5) as i32; }
        let mut cap = [0i32; NB_EBANDS];
        init_caps(&mut cap, lm, c as i32);
        let mut offsets = [0i32; NB_EBANDS];
        let mut dynalloc_logp = 6;
        total_bits <<= BITRES;
        let mut tellf = dec.tell_frac() as i32;
        for i in start..end {
            let width = (c as i32 * (eb(i + 1) - eb(i))) << lm;
            let quanta = (width << BITRES).min((6 << BITRES).max(width));
            let mut dynalloc_loop_logp = dynalloc_logp;
            let mut boost = 0;
            while tellf + (dynalloc_loop_logp << BITRES) < total_bits && boost < cap[i] {
                let flag = dec.bit_logp(dynalloc_loop_logp as u32);
                tellf = dec.tell_frac() as i32;
                if !flag { break; }
                boost += quanta;
                total_bits -= quanta;
                dynalloc_loop_logp = 1;
            }
            offsets[i] = boost;
            if boost > 0 { dynalloc_logp = 2.max(dynalloc_logp - 1); }
        }
        let alloc_trim = if tellf + (6 << BITRES) <= total_bits { dec.icdf(&TRIM_ICDF, 7) as i32 } else { 5 };
        let mut bits = (((len * 8) as i32) << BITRES) - dec.tell_frac() as i32 - 1;
        let anti_collapse_rsv = if is_transient && lm >= 2 && bits >= (lm + 2) << BITRES { 1 << BITRES } else { 0 };
        bits -= anti_collapse_rsv;
        let mut pulses = [0i32; NB_EBANDS];
        let mut fine_quant = [0i32; NB_EBANDS];
        let mut fine_priority = [0i32; NB_EBANDS];
        let alloc = compute_allocation(start, end, &offsets, &cap, alloc_trim, bits, &mut pulses, &mut fine_quant, &mut fine_priority, c as i32, lm, dec);
        energy::unquant_fine(start, end, &mut self.old_band_e, &fine_quant, dec, c);
        for ch in 0..cc { self.decode_mem[ch].copy_within(n..DECODE_BUFFER_SIZE + OVERLAP, 0); }
        let mut collapse_masks = vec![0u8; c * NB_EBANDS];
        let mut x = vec![0i16; c * n];
        let mut rng = self.rng;
        quant_all_bands(start, end, &mut x, c, &mut collapse_masks, &pulses, short_blocks, spread_decision, alloc.dual_stereo, alloc.intensity,
            &tf_res, ((len * 8) << BITRES) as i32 - anti_collapse_rsv, alloc.balance, dec, lm, alloc.coded_bands, &mut rng, self.disable_inv);
        self.rng = rng;
        let anti_collapse_on = if anti_collapse_rsv > 0 { dec.bits(1) != 0 } else { false };
        energy::unquant_finalise(start, end, &mut self.old_band_e, &fine_quant, &fine_priority, (len * 8) as i32 - dec.tell(), dec, c);
        if anti_collapse_on {
            anti_collapse(&mut x, &collapse_masks, lm, c, n, start, end, &self.old_band_e, &self.old_log_e, &self.old_log_e2, &pulses, self.rng);
        }
        if silence {
            for i in 0..c * NB_EBANDS { self.old_band_e[i] = -(28 << DB_SHIFT) as i16; }
        }
        if self.prefilter_and_fold { self.prefilter_and_fold(n); }
        let obe = self.old_band_e;
        self.synthesis(&x, &obe, start, eff_end, c, cc, is_transient, lm, silence);
        for ch in 0..cc {
            self.postfilter_period = self.postfilter_period.max(COMBFILTER_MINPERIOD);
            self.postfilter_period_old = self.postfilter_period_old.max(COMBFILTER_MINPERIOD);
            let off = DECODE_BUFFER_SIZE - n;
            comb_filter(&mut self.decode_mem[ch], off, None, self.postfilter_period_old, self.postfilter_period, SHORT_MDCT_SIZE,
                self.postfilter_gain_old, self.postfilter_gain, self.postfilter_tapset_old, self.postfilter_tapset, &WINDOW120, OVERLAP);
            if lm != 0 {
                comb_filter(&mut self.decode_mem[ch], off + SHORT_MDCT_SIZE, None, self.postfilter_period, postfilter_pitch, n - SHORT_MDCT_SIZE,
                    self.postfilter_gain, postfilter_gain, self.postfilter_tapset, postfilter_tapset, &WINDOW120, OVERLAP);
            }
        }
        self.postfilter_period_old = self.postfilter_period;
        self.postfilter_gain_old = self.postfilter_gain;
        self.postfilter_tapset_old = self.postfilter_tapset;
        self.postfilter_period = postfilter_pitch;
        self.postfilter_gain = postfilter_gain;
        self.postfilter_tapset = postfilter_tapset;
        if lm != 0 {
            self.postfilter_period_old = self.postfilter_period;
            self.postfilter_gain_old = self.postfilter_gain;
            self.postfilter_tapset_old = self.postfilter_tapset;
        }
        if c == 1 {
            let (a, b) = self.old_band_e.split_at_mut(NB_EBANDS);
            b.copy_from_slice(a);
        }
        if !is_transient {
            self.old_log_e2 = self.old_log_e;
            self.old_log_e = self.old_band_e;
        } else {
            for i in 0..2 * NB_EBANDS { self.old_log_e[i] = self.old_log_e[i].min(self.old_band_e[i]); }
        }
        let max_background_increase = 160.min(self.loss_duration + m) * 1;
        for i in 0..2 * NB_EBANDS {
            self.background_log_e[i] = (self.background_log_e[i] as i32 + max_background_increase).min(self.old_band_e[i] as i32) as i16;
        }
        for ch in 0..2 {
            for i in (0..start).chain(end..NB_EBANDS) {
                let k = ch * NB_EBANDS + i;
                self.old_band_e[k] = 0;
                self.old_log_e[k] = -(28 << DB_SHIFT) as i16;
                self.old_log_e2[k] = -(28 << DB_SHIFT) as i16;
            }
        }
        self.rng = dec.rng;
        self.deemphasis(n, pcm, accum);
        self.loss_duration = 0;
        self.prefilter_and_fold = false;
        if dec.tell() > 8 * len as i32 { return Err(crate::Error::Invalid("CELT frame overrun")); }
        if dec.error { self.error = true; }
        Ok(frame_size)
    }
}
