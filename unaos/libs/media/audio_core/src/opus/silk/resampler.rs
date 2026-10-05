//! The SILK output resampler, decoder side (`silk/resampler.c`, `resampler_private_IIR_FIR.c`,
//! `resampler_private_up2_HQ.c`): 8/12/16 kHz up to 24 or 48 kHz (2x all-pass up-sampler + 12-phase FIR),
//! or a copy at equal rates.
use super::macros::*;
use super::tables::*;

const ORDER_FIR_12: usize = 8;

#[derive(Clone)]
pub struct Resampler {
    s_iir: [i32; 6],
    s_fir: [i16; 8],
    delay_buf: [i16; 48],
    func: u8, // 0 copy, 1 up2 HQ, 2 IIR_FIR
    batch_size: usize,
    inv_ratio_q16: i32,
    fs_in_khz: usize,
    fs_out_khz: usize,
    input_delay: usize,
}

static DELAY_MATRIX_DEC: [[u8; 5]; 3] = [[4, 0, 2, 0, 0], [0, 9, 4, 7, 4], [0, 3, 12, 7, 7]];
fn rate_id(r: i32) -> usize { ((((r >> 12) - (r > 16000) as i32) >> (r > 24000) as i32) - 1) as usize }

impl Default for Resampler {
    fn default() -> Self { Resampler { s_iir: [0; 6], s_fir: [0; 8], delay_buf: [0; 48], func: 0, batch_size: 0, inv_ratio_q16: 0, fs_in_khz: 0, fs_out_khz: 0, input_delay: 0 } }
}

fn up2_hq(s: &mut [i32; 6], out: &mut [i16], input: &[i16], len: usize) {
    for k in 0..len {
        let in32 = lshift(input[k] as i32, 10);
        let y = in32.wrapping_sub(s[0]);
        let x = smulwb(y, RESAMPLER_UP2_HQ_0[0]);
        let out32_1 = s[0].wrapping_add(x);
        s[0] = in32.wrapping_add(x);
        let y = out32_1.wrapping_sub(s[1]);
        let x = smulwb(y, RESAMPLER_UP2_HQ_0[1]);
        let out32_2 = s[1].wrapping_add(x);
        s[1] = out32_1.wrapping_add(x);
        let y = out32_2.wrapping_sub(s[2]);
        let x = smlawb(y, y, RESAMPLER_UP2_HQ_0[2]);
        let out32_1 = s[2].wrapping_add(x);
        s[2] = out32_2.wrapping_add(x);
        out[2 * k] = sat16(rshift_round(out32_1, 10)) as i16;
        let y = in32.wrapping_sub(s[3]);
        let x = smulwb(y, RESAMPLER_UP2_HQ_1[0]);
        let out32_1 = s[3].wrapping_add(x);
        s[3] = in32.wrapping_add(x);
        let y = out32_1.wrapping_sub(s[4]);
        let x = smulwb(y, RESAMPLER_UP2_HQ_1[1]);
        let out32_2 = s[4].wrapping_add(x);
        s[4] = out32_1.wrapping_add(x);
        let y = out32_2.wrapping_sub(s[5]);
        let x = smlawb(y, y, RESAMPLER_UP2_HQ_1[2]);
        let out32_1 = s[5].wrapping_add(x);
        s[5] = out32_2.wrapping_add(x);
        out[2 * k + 1] = sat16(rshift_round(out32_1, 10)) as i16;
    }
}

impl Resampler {
    pub fn new(fs_in: i32, fs_out: i32) -> Resampler {
        let mut s = Resampler::default();
        s.input_delay = DELAY_MATRIX_DEC[rate_id(fs_in)][rate_id(fs_out)] as usize;
        s.fs_in_khz = (fs_in / 1000) as usize;
        s.fs_out_khz = (fs_out / 1000) as usize;
        s.batch_size = s.fs_in_khz * 10;
        let mut up2x = 0;
        if fs_out > fs_in {
            if fs_out == fs_in * 2 { s.func = 1; } else { s.func = 2; up2x = 1; }
        } else if fs_out < fs_in {
            s.func = 3; // down-sampling never happens in the decoder at 24/48 kHz API rates
        } else {
            s.func = 0;
        }
        s.inv_ratio_q16 = lshift(lshift(fs_in, 14 + up2x) / fs_out, 2);
        while smulww(s.inv_ratio_q16, fs_out) < lshift(fs_in, up2x) { s.inv_ratio_q16 += 1; }
        s
    }

    fn iir_fir(&mut self, out: &mut [i16], input: &[i16], in_len: usize) -> usize {
        let mut buf = alloc::vec![0i16; 2 * self.batch_size + ORDER_FIR_12];
        buf[..ORDER_FIR_12].copy_from_slice(&self.s_fir);
        let inc = self.inv_ratio_q16;
        let mut in_off = 0usize;
        let mut in_len = in_len;
        let mut o = 0usize;
        let mut n_in;
        loop {
            n_in = in_len.min(self.batch_size);
            up2_hq(&mut self.s_iir, &mut buf[ORDER_FIR_12..], &input[in_off..], n_in);
            let max_index_q16 = lshift(n_in as i32, 17);
            let mut idx = 0i32;
            while idx < max_index_q16 {
                let ti = smulwb(idx & 0xFFFF, 12) as usize;
                let bp = (idx >> 16) as usize;
                let f = |r: usize, c: usize| RESAMPLER_FRAC_FIR_12[r * 4 + c] as i32;
                let mut r = smulbb(buf[bp] as i32, f(ti, 0));
                r = smlabb(r, buf[bp + 1] as i32, f(ti, 1));
                r = smlabb(r, buf[bp + 2] as i32, f(ti, 2));
                r = smlabb(r, buf[bp + 3] as i32, f(ti, 3));
                r = smlabb(r, buf[bp + 4] as i32, f(11 - ti, 3));
                r = smlabb(r, buf[bp + 5] as i32, f(11 - ti, 2));
                r = smlabb(r, buf[bp + 6] as i32, f(11 - ti, 1));
                r = smlabb(r, buf[bp + 7] as i32, f(11 - ti, 0));
                out[o] = sat16(rshift_round(r, 15)) as i16;
                o += 1;
                idx += inc;
            }
            in_off += n_in;
            in_len -= n_in;
            if in_len > 0 {
                buf.copy_within(n_in << 1..(n_in << 1) + ORDER_FIR_12, 0);
            } else {
                break;
            }
        }
        self.s_fir.copy_from_slice(&buf[n_in << 1..(n_in << 1) + ORDER_FIR_12]);
        o
    }

    fn run(&mut self, out: &mut [i16], input: &[i16], len: usize) {
        match self.func {
            1 => { let mut s = self.s_iir; up2_hq(&mut s, out, input, len); self.s_iir = s; }
            2 => { self.iir_fir(out, input, len); }
            _ => out[..len].copy_from_slice(&input[..len]),
        }
    }

    /// `silk_resampler`: `input` has `in_len` samples (≥ 1 ms); writes in_len * out/in samples.
    pub fn process(&mut self, out: &mut [i16], input: &[i16], in_len: usize) {
        let n = self.fs_in_khz - self.input_delay;
        self.delay_buf[self.input_delay..self.input_delay + n].copy_from_slice(&input[..n]);
        let db = self.delay_buf;
        let fin = self.fs_in_khz;
        let fout = self.fs_out_khz;
        self.run(out, &db[..fin], fin);
        self.run(&mut out[fout..], &input[n..], in_len - fin);
        let d = self.input_delay;
        self.delay_buf[..d].copy_from_slice(&input[in_len - d..in_len]);
    }
}
