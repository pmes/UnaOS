//! SILK, the LPC layer of Opus (RFC 6716 §4.2), fixed point exactly as the normative reference
//! (`silk/dec_API.c` and the files named in each module): LBRR/VAD flags, mid/side stereo with the
//! predictor, per-channel frame decoding, and the resampler to the API rate.
pub mod channel;
pub mod dsp;
pub mod macros;
pub mod resampler;
pub mod tables;

use super::range::RangeDecoder;
use channel::*;
use macros::*;
use tables::*;
use alloc::vec;

#[derive(Clone, Default)]
pub struct DecControl {
    pub n_channels_api: usize,
    pub n_channels_internal: usize,
    pub api_sample_rate: i32,
    pub internal_sample_rate: i32,
    pub payload_size_ms: i32,
    pub prev_pitch_lag: i32,
}

#[derive(Clone, Default)]
struct Stereo {
    pred_prev_q13: [i32; 2],
    s_mid: [i16; 2],
    s_side: [i16; 2],
}

pub struct SilkDecoder {
    pub ch: [ChannelState; 2],
    stereo: Stereo,
    n_channels_api: usize,
    n_channels_internal: usize,
    prev_decode_only_middle: bool,
}

fn stereo_decode_pred(dec: &mut RangeDecoder, pred_q13: &mut [i32; 2]) {
    let mut ix = [[0i32; 3]; 2];
    let n = dec.icdf(&STEREO_PRED_JOINT_ICDF, 8) as i32;
    ix[0][2] = n / 5;
    ix[1][2] = n - 5 * ix[0][2];
    for i in 0..2 {
        ix[i][0] = dec.icdf(&UNIFORM3_ICDF, 8) as i32;
        ix[i][1] = dec.icdf(&UNIFORM5_ICDF, 8) as i32;
    }
    for i in 0..2 {
        ix[i][0] += 3 * ix[i][2];
        let low_q13 = STEREO_PRED_QUANT_Q13[ix[i][0] as usize] as i32;
        let step_q13 = smulwb(STEREO_PRED_QUANT_Q13[ix[i][0] as usize + 1] as i32 - low_q13, fix_const(0.5 / 5.0, 16));
        pred_q13[i] = smlabb(low_q13, step_q13, 2 * ix[i][1] + 1);
    }
    pred_q13[0] -= pred_q13[1];
}

fn stereo_ms_to_lr(st: &mut Stereo, x1: &mut [i16], x2: &mut [i16], pred_q13: &[i32; 2], fs_khz: i32, frame_length: usize) {
    x1[..2].copy_from_slice(&st.s_mid);
    x2[..2].copy_from_slice(&st.s_side);
    st.s_mid.copy_from_slice(&x1[frame_length..frame_length + 2]);
    st.s_side.copy_from_slice(&x2[frame_length..frame_length + 2]);
    let mut pred0 = st.pred_prev_q13[0];
    let mut pred1 = st.pred_prev_q13[1];
    let interp = (8 * fs_khz) as usize;
    let denom_q16 = (1i32 << 16) / (8 * fs_khz);
    let delta0 = rshift_round(smulbb(pred_q13[0] - st.pred_prev_q13[0], denom_q16), 16);
    let delta1 = rshift_round(smulbb(pred_q13[1] - st.pred_prev_q13[1], denom_q16), 16);
    for n in 0..frame_length {
        if n < interp {
            pred0 += delta0;
            pred1 += delta1;
        } else if n == interp {
            pred0 = pred_q13[0];
            pred1 = pred_q13[1];
        }
        let mut sum = lshift(add_lshift32(x1[n] as i32 + x1[n + 2] as i32, x1[n + 1] as i32, 1), 9);
        sum = smlawb(lshift(x2[n + 1] as i32, 8), sum, pred0);
        sum = smlawb(sum, lshift(x1[n + 1] as i32, 11), pred1);
        x2[n + 1] = sat16(rshift_round(sum, 8)) as i16;
    }
    if frame_length <= interp { /* loop above never reached the switch; nothing more to do */ }
    st.pred_prev_q13 = *pred_q13;
    for n in 0..frame_length {
        let sum = x1[n + 1] as i32 + x2[n + 1] as i32;
        let diff = x1[n + 1] as i32 - x2[n + 1] as i32;
        x1[n + 1] = sat16(sum) as i16;
        x2[n + 1] = sat16(diff) as i16;
    }
}

impl SilkDecoder {
    pub fn new() -> SilkDecoder {
        SilkDecoder { ch: [ChannelState::new(), ChannelState::new()], stereo: Stereo::default(), n_channels_api: 0, n_channels_internal: 0, prev_decode_only_middle: false }
    }
    /// `silk_ResetDecoder`.
    pub fn reset(&mut self) {
        for c in self.ch.iter_mut() { c.reset(); }
        self.stereo = Stereo::default();
        self.prev_decode_only_middle = false;
    }

    /// `silk_Decode`: one SILK frame (10 or 20 ms) into `out` (interleaved at the API rate/channels).
    /// Returns the number of samples per channel written.
    pub fn decode(&mut self, ctl: &mut DecControl, lost_flag: i32, new_packet: bool, dec: &mut RangeDecoder, out: &mut [i16]) -> crate::Result<usize> {
        let nci = ctl.n_channels_internal;
        let mut decode_only_middle = false;
        let mut ms_pred_q13 = [0i32; 2];
        if new_packet {
            for n in 0..nci { self.ch[n].n_frames_decoded = 0; }
        }
        if nci > self.n_channels_internal {
            self.ch[1] = ChannelState::new();
        }
        let stereo_to_mono = nci == 1 && self.n_channels_internal == 2 && ctl.internal_sample_rate == 1000 * self.ch[0].fs_khz;
        if self.ch[0].n_frames_decoded == 0 {
            for n in 0..nci {
                let (fpp, nb) = match ctl.payload_size_ms {
                    0 | 10 => (1, 2),
                    20 => (1, 4),
                    40 => (2, 4),
                    60 => (3, 4),
                    _ => return Err(crate::Error::Invalid("SILK frame size")),
                };
                self.ch[n].n_frames_per_packet = fpp;
                self.ch[n].nb_subfr = nb;
                let fs_khz_dec = (ctl.internal_sample_rate >> 10) + 1;
                if fs_khz_dec != 8 && fs_khz_dec != 12 && fs_khz_dec != 16 { return Err(crate::Error::Invalid("SILK rate")); }
                self.ch[n].set_fs(fs_khz_dec, ctl.api_sample_rate);
            }
        }
        if ctl.n_channels_api == 2 && nci == 2 && (self.n_channels_api == 1 || self.n_channels_internal == 1) {
            self.stereo.pred_prev_q13 = [0; 2];
            self.stereo.s_side = [0; 2];
            self.ch[1].resampler = self.ch[0].resampler.clone();
        }
        self.n_channels_api = ctl.n_channels_api;
        self.n_channels_internal = nci;
        if lost_flag != FLAG_PACKET_LOST && self.ch[0].n_frames_decoded == 0 {
            for n in 0..nci {
                for i in 0..self.ch[n].n_frames_per_packet { self.ch[n].vad_flags[i] = dec.bit_logp(1) as i32; }
                self.ch[n].lbrr_flag = dec.bit_logp(1) as i32;
            }
            for n in 0..nci {
                self.ch[n].lbrr_flags = [0; 3];
                if self.ch[n].lbrr_flag != 0 {
                    if self.ch[n].n_frames_per_packet == 1 {
                        self.ch[n].lbrr_flags[0] = 1;
                    } else {
                        let icdf: &[u8] = if self.ch[n].n_frames_per_packet == 2 { &LBRR_FLAGS_2_ICDF } else { &LBRR_FLAGS_3_ICDF };
                        let sym = dec.icdf(icdf, 8) as i32 + 1;
                        for i in 0..self.ch[n].n_frames_per_packet { self.ch[n].lbrr_flags[i] = (sym >> i) & 1; }
                    }
                }
            }
            if lost_flag == FLAG_DECODE_NORMAL {
                for i in 0..self.ch[0].n_frames_per_packet {
                    for n in 0..nci {
                        if self.ch[n].lbrr_flags[i] != 0 {
                            let mut pulses = [0i16; MAX_FRAME_LENGTH];
                            if nci == 2 && n == 0 {
                                stereo_decode_pred(dec, &mut ms_pred_q13);
                                if self.ch[1].lbrr_flags[i] == 0 {
                                    decode_only_middle = dec.icdf(&STEREO_ONLY_CODE_MID_ICDF, 8) != 0;
                                }
                            }
                            let cond = if i > 0 && self.ch[n].lbrr_flags[i - 1] != 0 { CODE_CONDITIONALLY } else { CODE_INDEPENDENTLY };
                            self.ch[n].decode_indices(dec, i, true, cond);
                            let (st, qo, fl) = (self.ch[n].indices.signal_type as i32, self.ch[n].indices.quant_offset_type as i32, self.ch[n].frame_length);
                            decode_pulses(dec, &mut pulses, st, qo, fl);
                        }
                    }
                }
            }
        }
        if nci == 2 {
            let fd = self.ch[0].n_frames_decoded;
            if lost_flag == FLAG_DECODE_NORMAL || (lost_flag == FLAG_DECODE_LBRR && self.ch[0].lbrr_flags[fd] == 1) {
                stereo_decode_pred(dec, &mut ms_pred_q13);
                if (lost_flag == FLAG_DECODE_NORMAL && self.ch[1].vad_flags[fd] == 0) || (lost_flag == FLAG_DECODE_LBRR && self.ch[1].lbrr_flags[fd] == 0) {
                    decode_only_middle = dec.icdf(&STEREO_ONLY_CODE_MID_ICDF, 8) != 0;
                } else {
                    decode_only_middle = false;
                }
            } else {
                ms_pred_q13 = self.stereo.pred_prev_q13;
            }
        }
        if nci == 2 && !decode_only_middle && self.prev_decode_only_middle {
            let c1 = &mut self.ch[1];
            c1.out_buf.fill(0);
            c1.slpc_q14_buf = [0; MAX_LPC_ORDER];
            c1.lag_prev = 100;
            c1.last_gain_index = 10;
            c1.prev_signal_type = TYPE_NO_VOICE_ACTIVITY;
            c1.first_frame_after_reset = true;
        }
        let fl = self.ch[0].frame_length;
        let mut tmp = [vec![0i16; fl + 2], vec![0i16; fl + 2]];
        let has_side = if lost_flag == FLAG_DECODE_NORMAL {
            !decode_only_middle
        } else {
            !self.prev_decode_only_middle || (nci == 2 && lost_flag == FLAG_DECODE_LBRR && self.ch[1].lbrr_flags[self.ch[1].n_frames_decoded] == 1)
        };
        let mut n_samples_out_dec = 0usize;
        for n in 0..nci {
            if n == 0 || has_side {
                let frame_index = self.ch[0].n_frames_decoded as i32 - n as i32;
                let cond = if frame_index <= 0 {
                    CODE_INDEPENDENTLY
                } else if lost_flag == FLAG_DECODE_LBRR {
                    if self.ch[n].lbrr_flags[frame_index as usize - 1] != 0 { CODE_CONDITIONALLY } else { CODE_INDEPENDENTLY }
                } else if n > 0 && self.prev_decode_only_middle {
                    CODE_INDEPENDENTLY_NO_LTP_SCALING
                } else {
                    CODE_CONDITIONALLY
                };
                n_samples_out_dec = self.ch[n].decode_frame(dec, &mut tmp[n][2..], lost_flag, cond);
            } else {
                for v in tmp[n][2..2 + n_samples_out_dec].iter_mut() { *v = 0; }
            }
            self.ch[n].n_frames_decoded += 1;
        }
        if ctl.n_channels_api == 2 && nci == 2 {
            let (a, b) = tmp.split_at_mut(1);
            stereo_ms_to_lr(&mut self.stereo, &mut a[0], &mut b[0], &ms_pred_q13, self.ch[0].fs_khz, n_samples_out_dec);
        } else {
            tmp[0][..2].copy_from_slice(&self.stereo.s_mid);
            self.stereo.s_mid.copy_from_slice(&tmp[0][n_samples_out_dec..n_samples_out_dec + 2]);
        }
        let n_out = (n_samples_out_dec as i32 * ctl.api_sample_rate / smulbb(self.ch[0].fs_khz, 1000)) as usize;
        let mut rs = vec![0i16; n_out];
        let napi = ctl.n_channels_api;
        for n in 0..napi.min(nci) {
            self.ch[n].resampler.process(&mut rs, &tmp[n][1..], n_samples_out_dec);
            for i in 0..n_out { out[n + napi * i] = rs[i]; }
        }
        if napi == 2 && nci == 1 {
            if stereo_to_mono {
                self.ch[1].resampler.process(&mut rs, &tmp[0][1..], n_samples_out_dec);
                for i in 0..n_out { out[1 + 2 * i] = rs[i]; }
            } else {
                for i in 0..n_out { out[1 + 2 * i] = out[2 * i]; }
            }
        }
        if self.ch[0].prev_signal_type == TYPE_VOICED {
            let mult = [6, 4, 3];
            ctl.prev_pitch_lag = self.ch[0].lag_prev * mult[((self.ch[0].fs_khz - 8) >> 2) as usize];
        } else {
            ctl.prev_pitch_lag = 0;
        }
        if lost_flag == FLAG_PACKET_LOST {
            for i in 0..self.n_channels_internal { self.ch[i].last_gain_index = 10; }
        } else {
            self.prev_decode_only_middle = decode_only_middle;
        }
        Ok(n_out)
    }
}
