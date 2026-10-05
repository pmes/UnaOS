//! One SILK channel (`silk/decode_frame.c`, `decode_indices.c`, `decode_pulses.c`, `shell_coder.c`,
//! `code_signs.c`, `decode_parameters.c`, `gain_quant.c`, `decode_pitch.c`, `decode_core.c`, `PLC.c`,
//! `CNG.c`, `decoder_set_fs.c`, `init_decoder.c`), RFC 6716 §4.2.7.
use super::super::range::RangeDecoder;
use super::dsp::*;
use super::macros::*;
use super::resampler::Resampler;
use super::tables::*;
use alloc::vec;

pub const MAX_LPC_ORDER: usize = 16;
pub const MAX_NB_SUBFR: usize = 4;
pub const MAX_FRAME_LENGTH: usize = 320;
pub const MAX_SUB_FRAME_LENGTH: usize = 80;
pub const LTP_ORDER: usize = 5;
pub const TYPE_NO_VOICE_ACTIVITY: i32 = 0;
pub const TYPE_UNVOICED: i32 = 1;
pub const TYPE_VOICED: i32 = 2;
pub const CODE_INDEPENDENTLY: i32 = 0;
pub const CODE_INDEPENDENTLY_NO_LTP_SCALING: i32 = 1;
pub const CODE_CONDITIONALLY: i32 = 2;
pub const FLAG_DECODE_NORMAL: i32 = 0;
pub const FLAG_PACKET_LOST: i32 = 1;
pub const FLAG_DECODE_LBRR: i32 = 2;

#[derive(Clone, Default)]
pub struct Indices {
    pub gains_indices: [i8; MAX_NB_SUBFR],
    pub ltp_index: [i8; MAX_NB_SUBFR],
    pub nlsf_indices: [i8; MAX_LPC_ORDER + 1],
    pub lag_index: i16,
    pub contour_index: i8,
    pub signal_type: i8,
    pub quant_offset_type: i8,
    pub nlsf_interp_coef_q2: i8,
    pub per_index: i8,
    pub ltp_scale_index: i8,
    pub seed: i8,
}

#[derive(Clone, Default)]
pub struct Control {
    pub pitch_l: [i32; MAX_NB_SUBFR],
    pub gains_q16: [i32; MAX_NB_SUBFR],
    pub pred_coef_q12: [[i16; MAX_LPC_ORDER]; 2],
    pub ltp_coef_q14: [i16; LTP_ORDER * MAX_NB_SUBFR],
    pub ltp_scale_q14: i32,
}

#[derive(Clone)]
pub struct Plc {
    pub pitch_l_q8: i32,
    pub ltp_coef_q14: [i16; LTP_ORDER],
    pub prev_lpc_q12: [i16; MAX_LPC_ORDER],
    pub last_frame_lost: bool,
    pub rand_seed: i32,
    pub rand_scale_q14: i16,
    pub conc_energy: i32,
    pub conc_energy_shift: i32,
    pub prev_ltp_scale_q14: i16,
    pub prev_gain_q16: [i32; 2],
    pub fs_khz: i32,
    pub nb_subfr: usize,
    pub subfr_length: usize,
}

#[derive(Clone)]
pub struct Cng {
    pub exc_buf_q14: [i32; MAX_FRAME_LENGTH],
    pub smth_nlsf_q15: [i16; MAX_LPC_ORDER],
    pub synth_state: [i32; MAX_LPC_ORDER],
    pub smth_gain_q16: i32,
    pub rand_seed: i32,
    pub fs_khz: i32,
}

#[derive(Clone)]
pub struct ChannelState {
    pub prev_gain_q16: i32,
    pub exc_q14: [i32; MAX_FRAME_LENGTH],
    pub slpc_q14_buf: [i32; MAX_LPC_ORDER],
    pub out_buf: [i16; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH],
    pub lag_prev: i32,
    pub last_gain_index: i8,
    pub fs_khz: i32,
    pub fs_api_hz: i32,
    pub nb_subfr: usize,
    pub frame_length: usize,
    pub subfr_length: usize,
    pub ltp_mem_length: usize,
    pub lpc_order: usize,
    pub prev_nlsf_q15: [i16; MAX_LPC_ORDER],
    pub first_frame_after_reset: bool,
    pub pitch_lag_low_bits_icdf: &'static [u8],
    pub pitch_contour_icdf: &'static [u8],
    pub n_frames_decoded: usize,
    pub n_frames_per_packet: usize,
    pub ec_prev_signal_type: i32,
    pub ec_prev_lag_index: i16,
    pub vad_flags: [i32; 3],
    pub lbrr_flag: i32,
    pub lbrr_flags: [i32; 3],
    pub resampler: Resampler,
    pub nlsf_cb: &'static NlsfCb,
    pub indices: Indices,
    pub cng: Cng,
    pub loss_cnt: i32,
    pub prev_signal_type: i32,
    pub plc: Plc,
}

impl ChannelState {
    pub fn new() -> ChannelState {
        let mut s = ChannelState {
            prev_gain_q16: 0,
            exc_q14: [0; MAX_FRAME_LENGTH],
            slpc_q14_buf: [0; MAX_LPC_ORDER],
            out_buf: [0; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH],
            lag_prev: 0,
            last_gain_index: 0,
            fs_khz: 0,
            fs_api_hz: 0,
            nb_subfr: 0,
            frame_length: 0,
            subfr_length: 0,
            ltp_mem_length: 0,
            lpc_order: 0,
            prev_nlsf_q15: [0; MAX_LPC_ORDER],
            first_frame_after_reset: false,
            pitch_lag_low_bits_icdf: &UNIFORM4_ICDF,
            pitch_contour_icdf: &PITCH_CONTOUR_ICDF,
            n_frames_decoded: 0,
            n_frames_per_packet: 0,
            ec_prev_signal_type: 0,
            ec_prev_lag_index: 0,
            vad_flags: [0; 3],
            lbrr_flag: 0,
            lbrr_flags: [0; 3],
            resampler: Resampler::default(),
            nlsf_cb: &NLSF_CB_NB_MB,
            indices: Indices::default(),
            cng: Cng { exc_buf_q14: [0; MAX_FRAME_LENGTH], smth_nlsf_q15: [0; MAX_LPC_ORDER], synth_state: [0; MAX_LPC_ORDER], smth_gain_q16: 0, rand_seed: 0, fs_khz: 0 },
            loss_cnt: 0,
            prev_signal_type: 0,
            plc: Plc { pitch_l_q8: 0, ltp_coef_q14: [0; LTP_ORDER], prev_lpc_q12: [0; MAX_LPC_ORDER], last_frame_lost: false, rand_seed: 0, rand_scale_q14: 0,
                conc_energy: 0, conc_energy_shift: 0, prev_ltp_scale_q14: 0, prev_gain_q16: [0; 2], fs_khz: 0, nb_subfr: 0, subfr_length: 0 },
        };
        s.reset();
        s
    }

    /// `silk_reset_decoder`: clears everything from `prev_gain_Q16` on (the whole channel state here, since
    /// nothing in this port precedes it), then the CNG and PLC resets.
    pub fn reset(&mut self) {
        let keep_pitch_icdf = (self.pitch_lag_low_bits_icdf, self.pitch_contour_icdf, self.nlsf_cb);
        self.prev_gain_q16 = 65536;
        self.exc_q14 = [0; MAX_FRAME_LENGTH];
        self.slpc_q14_buf = [0; MAX_LPC_ORDER];
        self.out_buf = [0; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH];
        self.lag_prev = 0;
        self.last_gain_index = 0;
        self.fs_khz = 0;
        self.fs_api_hz = 0;
        self.nb_subfr = 0;
        self.frame_length = 0;
        self.subfr_length = 0;
        self.ltp_mem_length = 0;
        self.lpc_order = 0;
        self.prev_nlsf_q15 = [0; MAX_LPC_ORDER];
        self.first_frame_after_reset = true;
        // the codebook/table pointers are zeroed by the C memset too; they are always re-set by
        // silk_decoder_set_fs before use (fs_kHz = 0 forces it)
        let _ = keep_pitch_icdf;
        self.n_frames_decoded = 0;
        self.n_frames_per_packet = 0;
        self.ec_prev_signal_type = 0;
        self.ec_prev_lag_index = 0;
        self.vad_flags = [0; 3];
        self.lbrr_flag = 0;
        self.lbrr_flags = [0; 3];
        self.resampler = Resampler::default();
        self.indices = Indices::default();
        self.cng = Cng { exc_buf_q14: [0; MAX_FRAME_LENGTH], smth_nlsf_q15: [0; MAX_LPC_ORDER], synth_state: [0; MAX_LPC_ORDER], smth_gain_q16: 0, rand_seed: 0, fs_khz: 0 };
        self.loss_cnt = 0;
        self.prev_signal_type = 0;
        self.plc = Plc { pitch_l_q8: 0, ltp_coef_q14: [0; LTP_ORDER], prev_lpc_q12: [0; MAX_LPC_ORDER], last_frame_lost: false, rand_seed: 0, rand_scale_q14: 0,
            conc_energy: 0, conc_energy_shift: 0, prev_ltp_scale_q14: 0, prev_gain_q16: [0; 2], fs_khz: 0, nb_subfr: 0, subfr_length: 0 };
        self.cng_reset();
        self.plc_reset();
    }

    fn cng_reset(&mut self) {
        let step = 32767 / (self.lpc_order as i32 + 1);
        let mut acc = 0i32;
        for i in 0..self.lpc_order {
            acc += step;
            self.cng.smth_nlsf_q15[i] = acc as i16;
        }
        self.cng.smth_gain_q16 = 0;
        self.cng.rand_seed = 3176576;
    }

    fn plc_reset(&mut self) {
        self.plc.pitch_l_q8 = lshift(self.frame_length as i32, 8 - 1);
        self.plc.prev_gain_q16 = [65536, 65536];
        self.plc.subfr_length = 20;
        self.plc.nb_subfr = 2;
    }

    pub fn set_fs(&mut self, fs_khz: i32, fs_api_hz: i32) {
        self.subfr_length = smulbb(5, fs_khz) as usize;
        let frame_length = smulbb(self.nb_subfr as i32, self.subfr_length as i32) as usize;
        if self.fs_khz != fs_khz || self.fs_api_hz != fs_api_hz {
            self.resampler = Resampler::new(smulbb(fs_khz, 1000), fs_api_hz);
            self.fs_api_hz = fs_api_hz;
        }
        if self.fs_khz != fs_khz || frame_length != self.frame_length {
            if fs_khz == 8 {
                self.pitch_contour_icdf = if self.nb_subfr == MAX_NB_SUBFR { &PITCH_CONTOUR_NB_ICDF } else { &PITCH_CONTOUR_10_MS_NB_ICDF };
            } else {
                self.pitch_contour_icdf = if self.nb_subfr == MAX_NB_SUBFR { &PITCH_CONTOUR_ICDF } else { &PITCH_CONTOUR_10_MS_ICDF };
            }
            if self.fs_khz != fs_khz {
                self.ltp_mem_length = smulbb(20, fs_khz) as usize;
                if fs_khz == 8 || fs_khz == 12 {
                    self.lpc_order = 10;
                    self.nlsf_cb = &NLSF_CB_NB_MB;
                } else {
                    self.lpc_order = 16;
                    self.nlsf_cb = &NLSF_CB_WB;
                }
                self.pitch_lag_low_bits_icdf = match fs_khz { 16 => &UNIFORM8_ICDF, 12 => &UNIFORM6_ICDF, _ => &UNIFORM4_ICDF };
                self.first_frame_after_reset = true;
                self.lag_prev = 100;
                self.last_gain_index = 10;
                self.prev_signal_type = TYPE_NO_VOICE_ACTIVITY;
                self.out_buf = [0; MAX_FRAME_LENGTH + 2 * MAX_SUB_FRAME_LENGTH];
                self.slpc_q14_buf = [0; MAX_LPC_ORDER];
            }
            self.fs_khz = fs_khz;
            self.frame_length = frame_length;
        }
    }

    pub fn decode_indices(&mut self, dec: &mut RangeDecoder, frame_index: usize, decode_lbrr: bool, cond_coding: i32) {
        let ix = if decode_lbrr || self.vad_flags[frame_index] != 0 {
            dec.icdf(&TYPE_OFFSET_VAD_ICDF, 8) as i32 + 2
        } else {
            dec.icdf(&TYPE_OFFSET_NO_VAD_ICDF, 8) as i32
        };
        self.indices.signal_type = (ix >> 1) as i8;
        self.indices.quant_offset_type = (ix & 1) as i8;
        if cond_coding == CODE_CONDITIONALLY {
            self.indices.gains_indices[0] = dec.icdf(&DELTA_GAIN_ICDF, 8) as i8;
        } else {
            let st = self.indices.signal_type as usize;
            self.indices.gains_indices[0] = lshift(dec.icdf(&GAIN_ICDF[st * 8..], 8) as i32, 3) as i8;
            self.indices.gains_indices[0] = self.indices.gains_indices[0].wrapping_add(dec.icdf(&UNIFORM8_ICDF, 8) as i8);
        }
        for i in 1..self.nb_subfr {
            self.indices.gains_indices[i] = dec.icdf(&DELTA_GAIN_ICDF, 8) as i8;
        }
        let cb = self.nlsf_cb;
        self.indices.nlsf_indices[0] = dec.icdf(&cb.cb1_icdf[(self.indices.signal_type as usize >> 1) * cb.n_vectors..], 8) as i8;
        let mut ec_ix = [0i16; MAX_LPC_ORDER];
        let mut pred_q8 = [0u8; MAX_LPC_ORDER];
        nlsf_unpack(&mut ec_ix, &mut pred_q8, cb, self.indices.nlsf_indices[0] as usize);
        for i in 0..cb.order {
            let mut ix = dec.icdf(&cb.ec_icdf[ec_ix[i] as usize..], 8) as i32;
            if ix == 0 {
                ix -= dec.icdf(&NLSF_EXT_ICDF, 8) as i32;
            } else if ix == 8 {
                ix += dec.icdf(&NLSF_EXT_ICDF, 8) as i32;
            }
            self.indices.nlsf_indices[i + 1] = (ix - 4) as i8;
        }
        if self.nb_subfr == MAX_NB_SUBFR {
            self.indices.nlsf_interp_coef_q2 = dec.icdf(&NLSF_INTERPOLATION_FACTOR_ICDF, 8) as i8;
        } else {
            self.indices.nlsf_interp_coef_q2 = 4;
        }
        if self.indices.signal_type as i32 == TYPE_VOICED {
            let mut decode_absolute = true;
            if cond_coding == CODE_CONDITIONALLY && self.ec_prev_signal_type == TYPE_VOICED {
                let mut delta = dec.icdf(&PITCH_DELTA_ICDF, 8) as i32;
                if delta > 0 {
                    delta -= 9;
                    self.indices.lag_index = (self.ec_prev_lag_index as i32 + delta) as i16;
                    decode_absolute = false;
                }
            }
            if decode_absolute {
                self.indices.lag_index = (dec.icdf(&PITCH_LAG_ICDF, 8) as i32 * (self.fs_khz >> 1)) as i16;
                self.indices.lag_index = (self.indices.lag_index as i32 + dec.icdf(self.pitch_lag_low_bits_icdf, 8) as i32) as i16;
            }
            self.ec_prev_lag_index = self.indices.lag_index;
            self.indices.contour_index = dec.icdf(self.pitch_contour_icdf, 8) as i8;
            self.indices.per_index = dec.icdf(&LTP_PER_INDEX_ICDF, 8) as i8;
            for k in 0..self.nb_subfr {
                self.indices.ltp_index[k] = dec.icdf(ltp_gain_icdf(self.indices.per_index as usize), 8) as i8;
            }
            if cond_coding == CODE_INDEPENDENTLY {
                self.indices.ltp_scale_index = dec.icdf(&LTPSCALE_ICDF, 8) as i8;
            } else {
                self.indices.ltp_scale_index = 0;
            }
        }
        self.ec_prev_signal_type = self.indices.signal_type as i32;
        self.indices.seed = dec.icdf(&UNIFORM4_ICDF, 8) as i8;
    }

    fn decode_parameters(&mut self, ctrl: &mut Control, cond_coding: i32) {
        // gains dequant
        let mut prev_ind = self.last_gain_index as i32;
        for k in 0..self.nb_subfr {
            if k == 0 && cond_coding != CODE_CONDITIONALLY {
                prev_ind = (self.indices.gains_indices[k] as i32).max(prev_ind - 16);
            } else {
                let ind_tmp = self.indices.gains_indices[k] as i32 + -4;
                let thr = 2 * 36 - 64 + prev_ind;
                if ind_tmp > thr { prev_ind += lshift(ind_tmp, 1) - thr; } else { prev_ind += ind_tmp; }
            }
            prev_ind = limit(prev_ind, 0, 63);
            // OFFSET = 2*128/6 + 16*128 = 2090, INV_SCALE_Q16 = 65536*((86*128)/6)/63 = 1907825
            ctrl.gains_q16[k] = log2lin((smulwb(1907825, prev_ind) + 2090).min(3967));
            prev_ind = prev_ind as i8 as i32;
        }
        self.last_gain_index = prev_ind as i8;
        let mut nlsf_q15 = [0i16; MAX_LPC_ORDER];
        let mut nlsf0_q15 = [0i16; MAX_LPC_ORDER];
        nlsf_decode(&mut nlsf_q15, &self.indices.nlsf_indices, self.nlsf_cb);
        nlsf2a(&mut ctrl.pred_coef_q12[1], &nlsf_q15, self.lpc_order);
        if self.first_frame_after_reset { self.indices.nlsf_interp_coef_q2 = 4; }
        if self.indices.nlsf_interp_coef_q2 < 4 {
            for i in 0..self.lpc_order {
                nlsf0_q15[i] = (self.prev_nlsf_q15[i] as i32
                    + ((self.indices.nlsf_interp_coef_q2 as i32 * (nlsf_q15[i] as i32 - self.prev_nlsf_q15[i] as i32)) >> 2)) as i16;
            }
            nlsf2a(&mut ctrl.pred_coef_q12[0], &nlsf0_q15, self.lpc_order);
        } else {
            ctrl.pred_coef_q12[0] = ctrl.pred_coef_q12[1];
        }
        self.prev_nlsf_q15[..self.lpc_order].copy_from_slice(&nlsf_q15[..self.lpc_order]);
        if self.loss_cnt != 0 {
            bwexpander(&mut ctrl.pred_coef_q12[0], self.lpc_order, 63570);
            bwexpander(&mut ctrl.pred_coef_q12[1], self.lpc_order, 63570);
        }
        if self.indices.signal_type as i32 == TYPE_VOICED {
            // silk_decode_pitch
            let (cb, cbk_size): (&[i8], usize) = if self.fs_khz == 8 {
                if self.nb_subfr == 4 { (&CB_LAGS_STAGE2, 11) } else { (&CB_LAGS_STAGE2_10_MS, 3) }
            } else if self.nb_subfr == 4 { (&CB_LAGS_STAGE3, 34) } else { (&CB_LAGS_STAGE3_10_MS, 12) };
            let min_lag = smulbb(2, self.fs_khz);
            let max_lag = smulbb(18, self.fs_khz);
            let lag = min_lag + self.indices.lag_index as i32;
            for k in 0..self.nb_subfr {
                ctrl.pitch_l[k] = limit(lag + cb[k * cbk_size + self.indices.contour_index as usize] as i32, min_lag, max_lag);
            }
            let cbk = ltp_vq(self.indices.per_index as usize);
            for k in 0..self.nb_subfr {
                let ix = self.indices.ltp_index[k] as usize;
                for i in 0..LTP_ORDER {
                    ctrl.ltp_coef_q14[k * LTP_ORDER + i] = lshift(cbk[ix * LTP_ORDER + i] as i32, 7) as i16;
                }
            }
            ctrl.ltp_scale_q14 = LTP_SCALES_TABLE_Q14[self.indices.ltp_scale_index as usize];
        } else {
            ctrl.pitch_l = [0; MAX_NB_SUBFR];
            ctrl.ltp_coef_q14 = [0; LTP_ORDER * MAX_NB_SUBFR];
            self.indices.per_index = 0;
            ctrl.ltp_scale_q14 = 0;
        }
    }

    fn decode_core(&mut self, ctrl: &mut Control, xq: &mut [i16], pulses: &[i16]) {
        let mut slpt = vec![0i16; self.ltp_mem_length];
        let mut sltp_q15 = vec![0i32; self.ltp_mem_length + self.frame_length];
        let mut res_q14 = vec![0i32; self.subfr_length];
        let mut slpc_q14 = vec![0i32; self.subfr_length + MAX_LPC_ORDER];
        let offset_q10 = QUANTIZATION_OFFSETS_Q10[(self.indices.signal_type >> 1) as usize][self.indices.quant_offset_type as usize];
        let nlsf_interpolation_flag = self.indices.nlsf_interp_coef_q2 < 4;
        let mut rand_seed = self.indices.seed as i32;
        for i in 0..self.frame_length {
            rand_seed = rand(rand_seed);
            let mut e = lshift(pulses[i] as i32, 14);
            if e > 0 { e -= 80 << 4; } else if e < 0 { e += 80 << 4; }
            e += offset_q10 << 4;
            if rand_seed < 0 { e = -e; }
            self.exc_q14[i] = e;
            rand_seed = rand_seed.wrapping_add(pulses[i] as i32);
        }
        slpc_q14[..MAX_LPC_ORDER].copy_from_slice(&self.slpc_q14_buf);
        let mut pexc = 0usize;
        let mut pxq = 0usize;
        let mut sltp_buf_idx = self.ltp_mem_length;
        let mut lag = 0i32;
        for k in 0..self.nb_subfr {
            let a_q12_tmp = ctrl.pred_coef_q12[k >> 1];
            let mut signal_type = self.indices.signal_type as i32;
            let gain_q10 = ctrl.gains_q16[k] >> 6;
            let mut inv_gain_q31 = inverse32_var_q(ctrl.gains_q16[k], 47);
            let gain_adj_q16;
            if ctrl.gains_q16[k] != self.prev_gain_q16 {
                gain_adj_q16 = div32_var_q(self.prev_gain_q16, ctrl.gains_q16[k], 16);
                for i in 0..MAX_LPC_ORDER { slpc_q14[i] = smulww(gain_adj_q16, slpc_q14[i]); }
            } else {
                gain_adj_q16 = 1 << 16;
            }
            self.prev_gain_q16 = ctrl.gains_q16[k];
            if self.loss_cnt != 0 && self.prev_signal_type == TYPE_VOICED && self.indices.signal_type as i32 != TYPE_VOICED && k < MAX_NB_SUBFR / 2 {
                for v in ctrl.ltp_coef_q14[k * LTP_ORDER..k * LTP_ORDER + LTP_ORDER].iter_mut() { *v = 0; }
                ctrl.ltp_coef_q14[k * LTP_ORDER + LTP_ORDER / 2] = 4096; // SILK_FIX_CONST(0.25, 14)
                signal_type = TYPE_VOICED;
                ctrl.pitch_l[k] = self.lag_prev;
            }
            let b_off = k * LTP_ORDER;
            if signal_type == TYPE_VOICED {
                lag = ctrl.pitch_l[k];
                if k == 0 || (k == 2 && nlsf_interpolation_flag) {
                    let start_idx = self.ltp_mem_length - lag as usize - self.lpc_order - LTP_ORDER / 2;
                    if k == 2 {
                        let n = 2 * self.subfr_length;
                        let lm = self.ltp_mem_length;
                        self.out_buf[lm..lm + n].copy_from_slice(&xq[..n]);
                    }
                    let a = ctrl.pred_coef_q12[k >> 1];
                    let inp = &self.out_buf[start_idx + k * self.subfr_length..];
                    lpc_analysis_filter(&mut slpt[start_idx..], inp, &a, self.ltp_mem_length - start_idx, self.lpc_order);
                    if k == 0 { inv_gain_q31 = lshift(smulwb(inv_gain_q31, ctrl.ltp_scale_q14), 2); }
                    for i in 0..(lag as usize + LTP_ORDER / 2) {
                        sltp_q15[sltp_buf_idx - i - 1] = smulwb(inv_gain_q31, slpt[self.ltp_mem_length - i - 1] as i32);
                    }
                } else if gain_adj_q16 != 1 << 16 {
                    for i in 0..(lag as usize + LTP_ORDER / 2) {
                        sltp_q15[sltp_buf_idx - i - 1] = smulww(gain_adj_q16, sltp_q15[sltp_buf_idx - i - 1]);
                    }
                }
            }
            let use_res: bool = signal_type == TYPE_VOICED;
            if use_res {
                let b = &ctrl.ltp_coef_q14[b_off..b_off + LTP_ORDER];
                let mut pl = sltp_buf_idx - lag as usize + LTP_ORDER / 2;
                for i in 0..self.subfr_length {
                    let mut p = 2i32;
                    p = smlawb(p, sltp_q15[pl], b[0] as i32);
                    p = smlawb(p, sltp_q15[pl - 1], b[1] as i32);
                    p = smlawb(p, sltp_q15[pl - 2], b[2] as i32);
                    p = smlawb(p, sltp_q15[pl - 3], b[3] as i32);
                    p = smlawb(p, sltp_q15[pl - 4], b[4] as i32);
                    pl += 1;
                    res_q14[i] = add_lshift32(self.exc_q14[pexc + i], p, 1);
                    sltp_q15[sltp_buf_idx] = lshift(res_q14[i], 1);
                    sltp_buf_idx += 1;
                }
            }
            for i in 0..self.subfr_length {
                let mut p = (self.lpc_order >> 1) as i32;
                for j in 0..self.lpc_order {
                    p = smlawb(p, slpc_q14[MAX_LPC_ORDER + i - j - 1], a_q12_tmp[j] as i32);
                }
                let r = if use_res { res_q14[i] } else { self.exc_q14[pexc + i] };
                slpc_q14[MAX_LPC_ORDER + i] = add_sat32(r, lshift_sat32(p, 4));
                xq[pxq + i] = sat16(rshift_round(smulww(slpc_q14[MAX_LPC_ORDER + i], gain_q10), 8)) as i16;
            }
            slpc_q14.copy_within(self.subfr_length..self.subfr_length + MAX_LPC_ORDER, 0);
            pexc += self.subfr_length;
            pxq += self.subfr_length;
        }
        self.slpc_q14_buf.copy_from_slice(&slpc_q14[..MAX_LPC_ORDER]);
    }

    fn plc_update(&mut self, ctrl: &Control) {
        self.prev_signal_type = self.indices.signal_type as i32;
        let mut ltp_gain_q14 = 0i32;
        if self.indices.signal_type as i32 == TYPE_VOICED {
            let mut j = 0usize;
            while (j * self.subfr_length) < ctrl.pitch_l[self.nb_subfr - 1] as usize {
                if j == self.nb_subfr { break; }
                let mut temp = 0i32;
                for i in 0..LTP_ORDER { temp += ctrl.ltp_coef_q14[(self.nb_subfr - 1 - j) * LTP_ORDER + i] as i32; }
                if temp > ltp_gain_q14 {
                    ltp_gain_q14 = temp;
                    let o = smulbb((self.nb_subfr - 1 - j) as i32, LTP_ORDER as i32) as usize;
                    self.plc.ltp_coef_q14.copy_from_slice(&ctrl.ltp_coef_q14[o..o + LTP_ORDER]);
                    self.plc.pitch_l_q8 = lshift(ctrl.pitch_l[self.nb_subfr - 1 - j], 8);
                }
                j += 1;
            }
            self.plc.ltp_coef_q14 = [0; LTP_ORDER];
            self.plc.ltp_coef_q14[LTP_ORDER / 2] = ltp_gain_q14 as i16;
            if ltp_gain_q14 < 11469 {
                let tmp = lshift(11469, 10);
                let scale_q10 = tmp / ltp_gain_q14.max(1);
                for i in 0..LTP_ORDER { self.plc.ltp_coef_q14[i] = (smulbb(self.plc.ltp_coef_q14[i] as i32, scale_q10) >> 10) as i16; }
            } else if ltp_gain_q14 > 15565 {
                let tmp = lshift(15565, 14);
                let scale_q14 = tmp / ltp_gain_q14.max(1);
                for i in 0..LTP_ORDER { self.plc.ltp_coef_q14[i] = (smulbb(self.plc.ltp_coef_q14[i] as i32, scale_q14) >> 14) as i16; }
            }
        } else {
            self.plc.pitch_l_q8 = lshift(smulbb(self.fs_khz, 18), 8);
            self.plc.ltp_coef_q14 = [0; LTP_ORDER];
        }
        self.plc.prev_lpc_q12[..self.lpc_order].copy_from_slice(&ctrl.pred_coef_q12[1][..self.lpc_order]);
        self.plc.prev_ltp_scale_q14 = ctrl.ltp_scale_q14 as i16;
        self.plc.prev_gain_q16.copy_from_slice(&ctrl.gains_q16[self.nb_subfr - 2..self.nb_subfr]);
        self.plc.subfr_length = self.subfr_length;
        self.plc.nb_subfr = self.nb_subfr;
    }

    fn plc_conceal(&mut self, ctrl: &mut Control, frame: &mut [i16]) {
        let mut sltp_q14 = vec![0i32; self.ltp_mem_length + self.frame_length];
        let mut sltp = vec![0i16; self.ltp_mem_length];
        let prev_gain_q10 = [self.plc.prev_gain_q16[0] >> 6, self.plc.prev_gain_q16[1] >> 6];
        if self.first_frame_after_reset { self.plc.prev_lpc_q12 = [0; MAX_LPC_ORDER]; }
        // silk_PLC_energy
        let (energy1, shift1, energy2, shift2);
        {
            let sl = self.subfr_length;
            let mut exc_buf = vec![0i16; 2 * sl];
            for k in 0..2 {
                for i in 0..sl {
                    exc_buf[k * sl + i] = sat16(smulww(self.exc_q14[i + (k + self.nb_subfr - 2) * sl], prev_gain_q10[k]) >> 8) as i16;
                }
            }
            let (e1, s1) = sum_sqr_shift(&exc_buf, sl);
            let (e2, s2) = sum_sqr_shift(&exc_buf[sl..], sl);
            energy1 = e1; shift1 = s1; energy2 = e2; shift2 = s2;
        }
        let rand_off = if (energy1 >> shift2) < (energy2 >> shift1) {
            0.max((self.plc.nb_subfr as i32 - 1) * self.plc.subfr_length as i32 - 128) as usize
        } else {
            0.max(self.plc.nb_subfr as i32 * self.plc.subfr_length as i32 - 128) as usize
        };
        let mut b_q14 = self.plc.ltp_coef_q14;
        let mut rand_scale_q14 = self.plc.rand_scale_q14 as i32;
        const HARM_ATT_Q15: [i32; 2] = [32440, 31130];
        const PLC_RAND_ATTENUATE_V_Q15: [i32; 2] = [31130, 26214];
        const PLC_RAND_ATTENUATE_UV_Q15: [i32; 2] = [32440, 29491];
        let li = (self.loss_cnt.min(1)) as usize;
        let harm_gain_q15 = HARM_ATT_Q15[li];
        let mut rand_gain_q15 = if self.prev_signal_type == TYPE_VOICED { PLC_RAND_ATTENUATE_V_Q15[li] } else { PLC_RAND_ATTENUATE_UV_Q15[li] };
        bwexpander(&mut self.plc.prev_lpc_q12, self.lpc_order, fix_const(0.99, 16));
        let a_q12 = self.plc.prev_lpc_q12;
        if self.loss_cnt == 0 {
            rand_scale_q14 = 1 << 14;
            if self.prev_signal_type == TYPE_VOICED {
                for i in 0..LTP_ORDER { rand_scale_q14 -= b_q14[i] as i32; }
                rand_scale_q14 = (rand_scale_q14 as i16).max(3277) as i32;
                rand_scale_q14 = (smulbb(rand_scale_q14, self.plc.prev_ltp_scale_q14 as i32) >> 14) as i16 as i32;
            } else {
                let inv_gain_q30 = lpc_inverse_pred_gain(&self.plc.prev_lpc_q12, self.lpc_order);
                let mut down = ((1i32 << 30) >> 3).min(inv_gain_q30);
                down = ((1i32 << 30) >> 8).max(down);
                down = lshift(down, 3);
                rand_gain_q15 = smulwb(down, rand_gain_q15) >> 14;
            }
        }
        let mut rand_seed = self.plc.rand_seed;
        let mut lag = rshift_round(self.plc.pitch_l_q8, 8);
        let mut sltp_buf_idx = self.ltp_mem_length;
        let idx = self.ltp_mem_length - lag as usize - self.lpc_order - LTP_ORDER / 2;
        {
            let inp = &self.out_buf[idx..];
            lpc_analysis_filter(&mut sltp[idx..], inp, &a_q12, self.ltp_mem_length - idx, self.lpc_order);
        }
        let mut inv_gain_q30 = inverse32_var_q(self.plc.prev_gain_q16[1], 46);
        inv_gain_q30 = inv_gain_q30.min(i32::MAX >> 1);
        for i in idx + self.lpc_order..self.ltp_mem_length { sltp_q14[i] = smulwb(inv_gain_q30, sltp[i] as i32); }
        for _ in 0..self.nb_subfr {
            let mut pl = sltp_buf_idx - lag as usize + LTP_ORDER / 2;
            for _ in 0..self.subfr_length {
                let mut p = 2i32;
                p = smlawb(p, sltp_q14[pl], b_q14[0] as i32);
                p = smlawb(p, sltp_q14[pl - 1], b_q14[1] as i32);
                p = smlawb(p, sltp_q14[pl - 2], b_q14[2] as i32);
                p = smlawb(p, sltp_q14[pl - 3], b_q14[3] as i32);
                p = smlawb(p, sltp_q14[pl - 4], b_q14[4] as i32);
                pl += 1;
                rand_seed = rand(rand_seed);
                let ri = ((rand_seed >> 25) & 127) as usize;
                sltp_q14[sltp_buf_idx] = lshift(smlawb(p, self.exc_q14[rand_off + ri], rand_scale_q14), 2);
                sltp_buf_idx += 1;
            }
            for j in 0..LTP_ORDER { b_q14[j] = (smulbb(harm_gain_q15, b_q14[j] as i32) >> 15) as i16; }
            rand_scale_q14 = (smulbb(rand_scale_q14, rand_gain_q15) >> 15) as i16 as i32;
            self.plc.pitch_l_q8 = smlawb(self.plc.pitch_l_q8, self.plc.pitch_l_q8, 655);
            self.plc.pitch_l_q8 = self.plc.pitch_l_q8.min(lshift(smulbb(18, self.fs_khz), 8));
            lag = rshift_round(self.plc.pitch_l_q8, 8);
        }
        let base = self.ltp_mem_length - MAX_LPC_ORDER;
        sltp_q14[base..base + MAX_LPC_ORDER].copy_from_slice(&self.slpc_q14_buf);
        for i in 0..self.frame_length {
            let mut p = (self.lpc_order >> 1) as i32;
            for j in 0..self.lpc_order { p = smlawb(p, sltp_q14[base + MAX_LPC_ORDER + i - j - 1], a_q12[j] as i32); }
            let k = base + MAX_LPC_ORDER + i;
            sltp_q14[k] = add_sat32(sltp_q14[k], lshift_sat32(p, 4));
            frame[i] = sat16(sat16(rshift_round(smulww(sltp_q14[k], prev_gain_q10[1]), 8))) as i16;
        }
        let fl = self.frame_length;
        self.slpc_q14_buf.copy_from_slice(&sltp_q14[base + fl..base + fl + MAX_LPC_ORDER]);
        self.plc.rand_seed = rand_seed;
        self.plc.rand_scale_q14 = rand_scale_q14 as i16;
        self.plc.ltp_coef_q14 = b_q14;
        for i in 0..MAX_NB_SUBFR { ctrl.pitch_l[i] = lag; }
    }

    fn plc(&mut self, ctrl: &mut Control, frame: &mut [i16], lost: bool) {
        if self.fs_khz != self.plc.fs_khz {
            self.plc_reset();
            self.plc.fs_khz = self.fs_khz;
        }
        if lost {
            self.plc_conceal(ctrl, frame);
            self.loss_cnt += 1;
        } else {
            self.plc_update(ctrl);
        }
    }

    fn plc_glue_frames(&mut self, frame: &mut [i16], length: usize) {
        if self.loss_cnt != 0 {
            let (e, s) = sum_sqr_shift(frame, length);
            self.plc.conc_energy = e;
            self.plc.conc_energy_shift = s;
            self.plc.last_frame_lost = true;
        } else {
            if self.plc.last_frame_lost {
                let (mut energy, energy_shift) = sum_sqr_shift(frame, length);
                if energy_shift > self.plc.conc_energy_shift {
                    self.plc.conc_energy >>= energy_shift - self.plc.conc_energy_shift;
                } else if energy_shift < self.plc.conc_energy_shift {
                    energy >>= self.plc.conc_energy_shift - energy_shift;
                }
                if energy > self.plc.conc_energy {
                    let lz = clz32(self.plc.conc_energy) - 1;
                    self.plc.conc_energy = lshift(self.plc.conc_energy, lz);
                    energy >>= (24 - lz).max(0);
                    let frac_q24 = self.plc.conc_energy / energy.max(1);
                    let mut gain_q16 = lshift(sqrt_approx(frac_q24), 4);
                    let mut slope_q16 = ((1i32 << 16) - gain_q16) / length as i32;
                    slope_q16 = lshift(slope_q16, 2);
                    for i in 0..length {
                        frame[i] = smulwb(gain_q16, frame[i] as i32) as i16;
                        gain_q16 += slope_q16;
                        if gain_q16 > 1 << 16 { break; }
                    }
                }
            }
            self.plc.last_frame_lost = false;
        }
    }

    fn cng(&mut self, ctrl: &Control, frame: &mut [i16], length: usize) {
        if self.fs_khz != self.cng.fs_khz {
            self.cng_reset();
            self.cng.fs_khz = self.fs_khz;
        }
        if self.loss_cnt == 0 && self.prev_signal_type == TYPE_NO_VOICE_ACTIVITY {
            for i in 0..self.lpc_order {
                self.cng.smth_nlsf_q15[i] = (self.cng.smth_nlsf_q15[i] as i32
                    + smulwb(self.prev_nlsf_q15[i] as i32 - self.cng.smth_nlsf_q15[i] as i32, 16348)) as i16;
            }
            let mut max_gain_q16 = 0;
            let mut subfr = 0;
            for i in 0..self.nb_subfr {
                if ctrl.gains_q16[i] > max_gain_q16 { max_gain_q16 = ctrl.gains_q16[i]; subfr = i; }
            }
            let sl = self.subfr_length;
            self.cng.exc_buf_q14.copy_within(0..(self.nb_subfr - 1) * sl, sl);
            self.cng.exc_buf_q14[..sl].copy_from_slice(&self.exc_q14[subfr * sl..subfr * sl + sl]);
            for i in 0..self.nb_subfr {
                self.cng.smth_gain_q16 = self.cng.smth_gain_q16.wrapping_add(smulwb(ctrl.gains_q16[i].wrapping_sub(self.cng.smth_gain_q16), 4634));
                if smulww(self.cng.smth_gain_q16, 46396) > ctrl.gains_q16[i] { self.cng.smth_gain_q16 = ctrl.gains_q16[i]; }
            }
        }
        if self.loss_cnt != 0 {
            let mut sig = vec![0i32; length + MAX_LPC_ORDER];
            let mut gain_q16 = smulww(self.plc.rand_scale_q14 as i32, self.plc.prev_gain_q16[1]);
            if gain_q16 >= (1 << 21) || self.cng.smth_gain_q16 > (1 << 23) {
                gain_q16 = smultt(gain_q16, gain_q16);
                gain_q16 = sub_lshift32(smultt(self.cng.smth_gain_q16, self.cng.smth_gain_q16), gain_q16, 5);
                gain_q16 = lshift(sqrt_approx(gain_q16), 16);
            } else {
                gain_q16 = smulww(gain_q16, gain_q16);
                gain_q16 = sub_lshift32(smulww(self.cng.smth_gain_q16, self.cng.smth_gain_q16), gain_q16, 5);
                gain_q16 = lshift(sqrt_approx(gain_q16), 8);
            }
            let gain_q10 = gain_q16 >> 6;
            // silk_CNG_exc
            let mut exc_mask = 255usize;
            while exc_mask > length { exc_mask >>= 1; }
            let mut seed = self.cng.rand_seed;
            for i in 0..length {
                seed = rand(seed);
                let idx = ((seed >> 24) as usize) & exc_mask;
                sig[MAX_LPC_ORDER + i] = self.cng.exc_buf_q14[idx];
            }
            self.cng.rand_seed = seed;
            let mut a_q12 = [0i16; MAX_LPC_ORDER];
            let nl = self.cng.smth_nlsf_q15;
            nlsf2a(&mut a_q12, &nl, self.lpc_order);
            sig[..MAX_LPC_ORDER].copy_from_slice(&self.cng.synth_state);
            for i in 0..length {
                let mut p = (self.lpc_order >> 1) as i32;
                for j in 0..self.lpc_order { p = smlawb(p, sig[MAX_LPC_ORDER + i - j - 1], a_q12[j] as i32); }
                sig[MAX_LPC_ORDER + i] = add_sat32(sig[MAX_LPC_ORDER + i], lshift_sat32(p, 4));
                frame[i] = sat16(frame[i] as i32 + sat16(rshift_round(smulww(sig[MAX_LPC_ORDER + i], gain_q10), 8))) as i16;
            }
            self.cng.synth_state.copy_from_slice(&sig[length..length + MAX_LPC_ORDER]);
        } else {
            for v in self.cng.synth_state[..self.lpc_order].iter_mut() { *v = 0; }
        }
    }

    /// `silk_decode_frame`. Returns the frame length.
    pub fn decode_frame(&mut self, dec: &mut RangeDecoder, out: &mut [i16], lost_flag: i32, cond_coding: i32) -> usize {
        let l = self.frame_length;
        let mut ctrl = Control::default();
        if lost_flag == FLAG_DECODE_NORMAL || (lost_flag == FLAG_DECODE_LBRR && self.lbrr_flags[self.n_frames_decoded] == 1) {
            let mut pulses = vec![0i16; (l + 15) & !15];
            let fi = self.n_frames_decoded;
            self.decode_indices(dec, fi, lost_flag != 0, cond_coding);
            decode_pulses(dec, &mut pulses, self.indices.signal_type as i32, self.indices.quant_offset_type as i32, self.frame_length);
            self.decode_parameters(&mut ctrl, cond_coding);
            self.decode_core(&mut ctrl, out, &pulses);
            let mv_len = self.ltp_mem_length - self.frame_length;
            self.out_buf.copy_within(self.frame_length..self.frame_length + mv_len, 0);
            self.out_buf[mv_len..mv_len + self.frame_length].copy_from_slice(&out[..self.frame_length]);
            self.plc(&mut ctrl, out, false);
            self.loss_cnt = 0;
            self.prev_signal_type = self.indices.signal_type as i32;
            self.first_frame_after_reset = false;
        } else {
            self.plc(&mut ctrl, out, true);
            let mv_len = self.ltp_mem_length - self.frame_length;
            self.out_buf.copy_within(self.frame_length..self.frame_length + mv_len, 0);
            self.out_buf[mv_len..mv_len + self.frame_length].copy_from_slice(&out[..self.frame_length]);
        }
        self.cng(&ctrl, out, l);
        self.plc_glue_frames(out, l);
        self.lag_prev = ctrl.pitch_l[self.nb_subfr - 1];
        l
    }
}

fn shell_split(dec: &mut RangeDecoder, p: i32, table: &[u8]) -> (i16, i16) {
    if p > 0 {
        let c1 = dec.icdf(&table[SHELL_CODE_TABLE_OFFSETS[p as usize] as usize..], 8) as i32;
        (c1 as i16, (p - c1) as i16)
    } else {
        (0, 0)
    }
}

fn shell_decoder(p0: &mut [i16], dec: &mut RangeDecoder, pulses4: i32) {
    let mut p3 = [0i16; 2];
    let mut p2 = [0i16; 4];
    let mut p1 = [0i16; 8];
    (p3[0], p3[1]) = shell_split(dec, pulses4, &SHELL_CODE_TABLE3);
    (p2[0], p2[1]) = shell_split(dec, p3[0] as i32, &SHELL_CODE_TABLE2);
    (p1[0], p1[1]) = shell_split(dec, p2[0] as i32, &SHELL_CODE_TABLE1);
    (p0[0], p0[1]) = shell_split(dec, p1[0] as i32, &SHELL_CODE_TABLE0);
    (p0[2], p0[3]) = shell_split(dec, p1[1] as i32, &SHELL_CODE_TABLE0);
    (p1[2], p1[3]) = shell_split(dec, p2[1] as i32, &SHELL_CODE_TABLE1);
    (p0[4], p0[5]) = shell_split(dec, p1[2] as i32, &SHELL_CODE_TABLE0);
    (p0[6], p0[7]) = shell_split(dec, p1[3] as i32, &SHELL_CODE_TABLE0);
    (p2[2], p2[3]) = shell_split(dec, p3[1] as i32, &SHELL_CODE_TABLE2);
    (p1[4], p1[5]) = shell_split(dec, p2[2] as i32, &SHELL_CODE_TABLE1);
    (p0[8], p0[9]) = shell_split(dec, p1[4] as i32, &SHELL_CODE_TABLE0);
    (p0[10], p0[11]) = shell_split(dec, p1[5] as i32, &SHELL_CODE_TABLE0);
    (p1[6], p1[7]) = shell_split(dec, p2[3] as i32, &SHELL_CODE_TABLE1);
    (p0[12], p0[13]) = shell_split(dec, p1[6] as i32, &SHELL_CODE_TABLE0);
    (p0[14], p0[15]) = shell_split(dec, p1[7] as i32, &SHELL_CODE_TABLE0);
}

pub fn decode_pulses(dec: &mut RangeDecoder, pulses: &mut [i16], signal_type: i32, quant_offset_type: i32, frame_length: usize) {
    let mut sum_pulses = [0i32; 20];
    let mut n_lshifts = [0i32; 20];
    let rate_level = dec.icdf(&RATE_LEVELS_ICDF[(signal_type >> 1) as usize * 9..], 8);
    let mut iter = frame_length >> 4;
    if iter * 16 < frame_length { iter += 1; }
    let cdf = &PULSES_PER_BLOCK_ICDF[rate_level * 18..];
    for i in 0..iter {
        n_lshifts[i] = 0;
        sum_pulses[i] = dec.icdf(cdf, 8) as i32;
        while sum_pulses[i] == 17 {
            n_lshifts[i] += 1;
            let off = 9 * 18 + (n_lshifts[i] == 10) as usize;
            sum_pulses[i] = dec.icdf(&PULSES_PER_BLOCK_ICDF[off..], 8) as i32;
        }
    }
    for i in 0..iter {
        if sum_pulses[i] > 0 {
            shell_decoder(&mut pulses[i * 16..i * 16 + 16], dec, sum_pulses[i]);
        } else {
            for v in pulses[i * 16..i * 16 + 16].iter_mut() { *v = 0; }
        }
    }
    for i in 0..iter {
        if n_lshifts[i] > 0 {
            let nls = n_lshifts[i];
            for k in 0..16 {
                let mut abs_q = pulses[i * 16 + k] as i32;
                for _ in 0..nls {
                    abs_q = lshift(abs_q, 1);
                    abs_q += dec.icdf(&LSB_ICDF, 8) as i32;
                }
                pulses[i * 16 + k] = abs_q as i16;
            }
            sum_pulses[i] |= nls << 5;
        }
    }
    // silk_decode_signs
    let mut icdf = [0u8, 0u8];
    let ii = smulbb(7, quant_offset_type + (signal_type << 1)) as usize;
    let icdf_ptr = &SIGN_ICDF[ii..];
    let length = (frame_length + 8) >> 4;
    for i in 0..length {
        let p = sum_pulses[i];
        if p > 0 {
            icdf[0] = icdf_ptr[(p & 0x1F).min(6) as usize];
            for j in 0..16 {
                let q = &mut pulses[i * 16 + j];
                if *q > 0 {
                    let s = dec.icdf(&icdf, 8) as i32;
                    *q = (*q as i32 * (lshift(s, 1) - 1)) as i16;
                }
            }
        }
    }
}
