//! §7.12.2 dequantization functions, §7.12.3 the reconstruct process, and §7.13 the inverse
//! transforms: DCT 4..64 (butterfly network of §7.13.2.3), ADST 4/8/16, identity 4..32, and the
//! Walsh-Hadamard transform for lossless blocks, combined by the 2D inverse transform (§7.13.3).

use crate::decode::{round2, Dec};
use crate::tables::*;

fn brev(num_bits: u32, x: usize) -> usize {
    let mut t = 0;
    for i in 0..num_bits {
        let bit = (x >> i) & 1;
        t += bit << (num_bits - 1 - i);
    }
    t
}

fn cos128(angle: i32) -> i64 {
    let angle2 = angle & 255;
    if angle2 <= 64 {
        COS128_LOOKUP[angle2 as usize] as i64
    } else if angle2 <= 128 {
        -(COS128_LOOKUP[(128 - angle2) as usize] as i64)
    } else if angle2 <= 192 {
        -(COS128_LOOKUP[(angle2 - 128) as usize] as i64)
    } else {
        COS128_LOOKUP[(256 - angle2) as usize] as i64
    }
}
fn sin128(angle: i32) -> i64 {
    cos128(angle - 64)
}

/// The array T and the butterfly functions B and H.
struct Tx<'t> {
    t: &'t mut [i32],
    r: u32,
}

impl Tx<'_> {
    #[inline]
    fn b(&mut self, a: usize, b: usize, angle: i32, flip: bool) {
        let x = self.t[a] as i64 * cos128(angle) - self.t[b] as i64 * sin128(angle);
        let y = self.t[a] as i64 * sin128(angle) + self.t[b] as i64 * cos128(angle);
        self.t[a] = round2(x, 12) as i32;
        self.t[b] = round2(y, 12) as i32;
        if flip {
            self.t.swap(a, b);
        }
    }
    #[inline]
    fn h(&mut self, a: usize, b: usize, flip: bool) {
        let (a, b) = if flip { (b, a) } else { (a, b) };
        let x = self.t[a] as i64;
        let y = self.t[b] as i64;
        let lo = -(1i64 << (self.r - 1));
        let hi = (1i64 << (self.r - 1)) - 1;
        self.t[a] = (x + y).clamp(lo, hi) as i32;
        self.t[b] = (x - y).clamp(lo, hi) as i32;
    }

    /// §7.13.2.3 inverse DCT (includes the permutation of §7.13.2.2).
    fn idct(&mut self, n: u32) {
        let n0 = 1usize << n;
        let mut copy = [0i32; 64];
        copy[..n0].copy_from_slice(&self.t[..n0]);
        for i in 0..n0 {
            self.t[i] = copy[brev(n, i)];
        }
        if n == 6 {
            for i in 0..16 {
                self.b(32 + i, 63 - i, 63 - 4 * brev(4, i) as i32, false);
            }
        }
        if n >= 5 {
            for i in 0..8 {
                self.b(16 + i, 31 - i, 6 + ((brev(3, 7 - i) as i32) << 3), false);
            }
        }
        if n == 6 {
            for i in 0..16 {
                self.h(32 + i * 2, 33 + i * 2, i & 1 != 0);
            }
        }
        if n >= 4 {
            for i in 0..4 {
                self.b(8 + i, 15 - i, 12 + ((brev(2, 3 - i) as i32) << 4), false);
            }
        }
        if n >= 5 {
            for i in 0..8 {
                self.h(16 + 2 * i, 17 + 2 * i, i & 1 != 0);
            }
        }
        if n == 6 {
            for i in 0..4 {
                for j in 0..2 {
                    self.b(62 - i * 4 - j, 33 + i * 4 + j, 60 - 16 * brev(2, i) as i32 + 64 * j as i32, true);
                }
            }
        }
        if n >= 3 {
            for i in 0..2 {
                self.b(4 + i, 7 - i, 56 - 32 * i as i32, false);
            }
        }
        if n >= 4 {
            for i in 0..4 {
                self.h(8 + 2 * i, 9 + 2 * i, i & 1 != 0);
            }
        }
        if n >= 5 {
            for i in 0..2 {
                for j in 0..2 {
                    self.b(30 - 4 * i - j, 17 + 4 * i + j, 24 + ((j as i32) << 6) + (((1 - i) as i32) << 5), true);
                }
            }
        }
        if n == 6 {
            for i in 0..8 {
                for j in 0..2 {
                    self.h(32 + i * 4 + j, 35 + i * 4 - j, i & 1 != 0);
                }
            }
        }
        for i in 0..2 {
            self.b(2 * i, 2 * i + 1, 32 + 16 * i as i32, i == 0);
        }
        if n >= 3 {
            for i in 0..2 {
                self.h(4 + 2 * i, 5 + 2 * i, i != 0);
            }
        }
        if n >= 4 {
            for i in 0..2 {
                self.b(14 - i, 9 + i, 48 + 64 * i as i32, true);
            }
        }
        if n >= 5 {
            for i in 0..4 {
                for j in 0..2 {
                    self.h(16 + 4 * i + j, 19 + 4 * i - j, i & 1 != 0);
                }
            }
        }
        if n == 6 {
            for i in 0..2 {
                for j in 0..4 {
                    self.b(61 - i * 8 - j, 34 + i * 8 + j, 56 - i as i32 * 32 + (j as i32 >> 1) * 64, true);
                }
            }
        }
        for i in 0..2 {
            self.h(i, 3 - i, false);
        }
        if n >= 3 {
            self.b(6, 5, 32, true);
        }
        if n >= 4 {
            for i in 0..2 {
                for j in 0..2 {
                    self.h(8 + 4 * i + j, 11 + 4 * i - j, i != 0);
                }
            }
        }
        if n >= 5 {
            for i in 0..4 {
                self.b(29 - i, 18 + i, 48 + (i as i32 >> 1) * 64, true);
            }
        }
        if n == 6 {
            for i in 0..4 {
                for j in 0..4 {
                    self.h(32 + 8 * i + j, 39 + 8 * i - j, i & 1 != 0);
                }
            }
        }
        if n >= 3 {
            for i in 0..4 {
                self.h(i, 7 - i, false);
            }
        }
        if n >= 4 {
            for i in 0..2 {
                self.b(13 - i, 10 + i, 32, true);
            }
        }
        if n >= 5 {
            for i in 0..2 {
                for j in 0..4 {
                    self.h(16 + i * 8 + j, 23 + i * 8 - j, i != 0);
                }
            }
        }
        if n == 6 {
            for i in 0..8 {
                self.b(59 - i, 36 + i, if i < 4 { 48 } else { 112 }, true);
            }
        }
        if n >= 4 {
            for i in 0..8 {
                self.h(i, 15 - i, false);
            }
        }
        if n >= 5 {
            for i in 0..4 {
                self.b(27 - i, 20 + i, 32, true);
            }
        }
        if n == 6 {
            for i in 0..8 {
                self.h(32 + i, 47 - i, false);
                self.h(48 + i, 63 - i, true);
            }
        }
        if n >= 5 {
            for i in 0..16 {
                self.h(i, 31 - i, false);
            }
        }
        if n == 6 {
            for i in 0..8 {
                self.b(55 - i, 40 + i, 32, true);
            }
        }
        if n == 6 {
            for i in 0..32 {
                self.h(i, 63 - i, false);
            }
        }
    }

    fn adst_in_perm(&mut self, n: u32) {
        let n0 = 1usize << n;
        let mut copy = [0i32; 16];
        copy[..n0].copy_from_slice(&self.t[..n0]);
        for i in 0..n0 {
            let idx = if i & 1 != 0 { i - 1 } else { n0 - i - 1 };
            self.t[i] = copy[idx];
        }
    }
    fn adst_out_perm(&mut self, n: u32) {
        let n0 = 1usize << n;
        let mut copy = [0i32; 16];
        copy[..n0].copy_from_slice(&self.t[..n0]);
        for i in 0..n0 {
            let a = (i >> 3) & 1;
            let b = ((i >> 2) & 1) ^ ((i >> 3) & 1);
            let c = ((i >> 1) & 1) ^ ((i >> 2) & 1);
            let d = (i & 1) ^ ((i >> 1) & 1);
            let idx = ((d << 3) | (c << 2) | (b << 1) | a) >> (4 - n);
            self.t[i] = if i & 1 != 0 { -copy[idx] } else { copy[idx] };
        }
    }
    fn adst4(&mut self) {
        const SINPI_1_9: i64 = 1321;
        const SINPI_2_9: i64 = 2482;
        const SINPI_3_9: i64 = 3344;
        const SINPI_4_9: i64 = 3803;
        let t: [i64; 4] = [self.t[0] as i64, self.t[1] as i64, self.t[2] as i64, self.t[3] as i64];
        let mut s = [0i64; 7];
        s[0] = SINPI_1_9 * t[0];
        s[1] = SINPI_2_9 * t[0];
        s[2] = SINPI_3_9 * t[1];
        s[3] = SINPI_4_9 * t[2];
        s[4] = SINPI_1_9 * t[2];
        s[5] = SINPI_2_9 * t[3];
        s[6] = SINPI_4_9 * t[3];
        let a7 = t[0] - t[2];
        let b7 = a7 + t[3];
        s[0] += s[3];
        s[1] -= s[4];
        s[3] = s[2];
        s[2] = SINPI_3_9 * b7;
        s[0] += s[5];
        s[1] -= s[6];
        let x0 = s[0] + s[3];
        let x1 = s[1] + s[3];
        let x2 = s[2];
        let mut x3 = s[0] + s[1];
        x3 -= s[3];
        self.t[0] = round2(x0, 12) as i32;
        self.t[1] = round2(x1, 12) as i32;
        self.t[2] = round2(x2, 12) as i32;
        self.t[3] = round2(x3, 12) as i32;
    }
    fn adst8(&mut self) {
        self.adst_in_perm(3);
        for i in 0..4 {
            self.b(2 * i, 2 * i + 1, 60 - 16 * i as i32, true);
        }
        for i in 0..4 {
            self.h(i, 4 + i, false);
        }
        for i in 0..2 {
            self.b(4 + 3 * i, 5 + i, 48 - 32 * i as i32, true);
        }
        for i in 0..2 {
            for j in 0..2 {
                self.h(4 * j + i, 2 + 4 * j + i, false);
            }
        }
        for i in 0..2 {
            self.b(2 + 4 * i, 3 + 4 * i, 32, true);
        }
        self.adst_out_perm(3);
    }
    fn adst16(&mut self) {
        self.adst_in_perm(4);
        for i in 0..8 {
            self.b(2 * i, 2 * i + 1, 62 - 8 * i as i32, true);
        }
        for i in 0..8 {
            self.h(i, 8 + i, false);
        }
        for i in 0..2 {
            self.b(8 + 2 * i, 9 + 2 * i, 56 - 32 * i as i32, true);
            self.b(13 + 2 * i, 12 + 2 * i, 8 + 32 * i as i32, true);
        }
        for i in 0..4 {
            for j in 0..2 {
                self.h(8 * j + i, 4 + 8 * j + i, false);
            }
        }
        for i in 0..2 {
            for j in 0..2 {
                self.b(4 + 8 * j + 3 * i, 5 + 8 * j + i, 48 - 32 * i as i32, true);
            }
        }
        for i in 0..2 {
            for j in 0..4 {
                self.h(4 * j + i, 2 + 4 * j + i, false);
            }
        }
        for i in 0..4 {
            self.b(2 + 4 * i, 3 + 4 * i, 32, true);
        }
        self.adst_out_perm(4);
    }
    fn adst(&mut self, n: u32) {
        match n {
            2 => self.adst4(),
            3 => self.adst8(),
            _ => self.adst16(),
        }
    }
    fn identity(&mut self, n: u32) {
        let n0 = 1usize << n;
        for i in 0..n0 {
            let v = self.t[i] as i64;
            self.t[i] = match n {
                2 => round2(v * 5793, 12),
                3 => v * 2,
                4 => round2(v * 11586, 12),
                _ => v * 4,
            } as i32;
        }
    }
    fn wht(&mut self, shift: u32) {
        let mut a = self.t[0] >> shift;
        let mut c = self.t[1] >> shift;
        let mut d = self.t[2] >> shift;
        let mut b = self.t[3] >> shift;
        a += c;
        d -= b;
        let e = (a - d) >> 1;
        b = e - b;
        c = e - c;
        a -= b;
        d += c;
        self.t[0] = a;
        self.t[1] = b;
        self.t[2] = c;
        self.t[3] = d;
    }
}

fn row_kind(t: usize) -> u8 {
    // 0 = DCT, 1 = ADST, 2 = identity (for the row / horizontal 1D transform)
    match t {
        DCT_DCT | ADST_DCT | FLIPADST_DCT | H_DCT => 0,
        DCT_ADST | ADST_ADST | DCT_FLIPADST | FLIPADST_FLIPADST | ADST_FLIPADST | FLIPADST_ADST | H_ADST | H_FLIPADST => 1,
        _ => 2,
    }
}
fn col_kind(t: usize) -> u8 {
    match t {
        DCT_DCT | DCT_ADST | DCT_FLIPADST | V_DCT => 0,
        ADST_DCT | ADST_ADST | FLIPADST_DCT | FLIPADST_FLIPADST | ADST_FLIPADST | FLIPADST_ADST | V_ADST | V_FLIPADST => 1,
        _ => 2,
    }
}

/// The 2D inverse transform (§7.13.3) of `dequant` (row-major, stride 64) into `residual`
/// (row-major, stride 64).
pub fn inverse_transform_2d(dequant: &[i32], residual: &mut [i32], tx_sz: usize, tx_type: usize, lossless: bool, bit_depth: u32) {
    let log2w = TX_WIDTH_LOG2[tx_sz] as u32;
    let log2h = TX_HEIGHT_LOG2[tx_sz] as u32;
    let w = 1usize << log2w;
    let h = 1usize << log2h;
    let row_shift = if lossless { 0 } else { TRANSFORM_ROW_SHIFT[tx_sz] as u32 };
    let col_shift = if lossless { 0 } else { 4 };
    let row_clamp_range = bit_depth + 8;
    let col_clamp_range = (bit_depth + 6).max(16);
    let mut t = [0i32; 64];
    let rk = row_kind(tx_type);
    let ck = col_kind(tx_type);
    for i in 0..h {
        for j in 0..w {
            t[j] = if i < 32 && j < 32 { dequant[i * 64 + j] } else { 0 };
        }
        if (log2w as i32 - log2h as i32).abs() == 1 {
            for j in 0..w {
                t[j] = round2(t[j] as i64 * 2896, 12) as i32;
            }
        }
        let mut tx = Tx { t: &mut t, r: row_clamp_range };
        if lossless {
            tx.wht(2);
        } else if rk == 0 {
            tx.idct(log2w);
        } else if rk == 1 {
            tx.adst(log2w);
        } else {
            tx.identity(log2w);
        }
        for j in 0..w {
            residual[i * 64 + j] = round2(t[j] as i64, row_shift) as i32;
        }
    }
    let lo = -(1i32 << (col_clamp_range - 1));
    let hi = (1i32 << (col_clamp_range - 1)) - 1;
    for i in 0..h {
        for j in 0..w {
            residual[i * 64 + j] = residual[i * 64 + j].clamp(lo, hi);
        }
    }
    for j in 0..w {
        for i in 0..h {
            t[i] = residual[i * 64 + j];
        }
        let mut tx = Tx { t: &mut t, r: col_clamp_range };
        if lossless {
            tx.wht(0);
        } else if ck == 0 {
            tx.idct(log2h);
        } else if ck == 1 {
            tx.adst(log2h);
        } else {
            tx.identity(log2h);
        }
        for i in 0..h {
            residual[i * 64 + j] = round2(t[i] as i64, col_shift) as i32;
        }
    }
}

impl<'a, 'f> Dec<'a, 'f> {
    fn qidx(&self) -> i32 {
        self.hdr.get_qindex(false, self.segment_id, self.current_q_index)
    }
    fn dc_q(&self, b: i32) -> i32 {
        DC_QLOOKUP[((self.fs.bit_depth - 8) >> 1) as usize][b.clamp(0, 255) as usize] as i32
    }
    fn ac_q(&self, b: i32) -> i32 {
        AC_QLOOKUP[((self.fs.bit_depth - 8) >> 1) as usize][b.clamp(0, 255) as usize] as i32
    }
    pub fn get_dc_quant(&self, plane: usize) -> i32 {
        let q = self.qidx();
        match plane {
            0 => self.dc_q(q + self.hdr.delta_q_y_dc),
            1 => self.dc_q(q + self.hdr.delta_q_u_dc),
            _ => self.dc_q(q + self.hdr.delta_q_v_dc),
        }
    }
    pub fn get_ac_quant(&self, plane: usize) -> i32 {
        let q = self.qidx();
        match plane {
            0 => self.ac_q(q),
            1 => self.ac_q(q + self.hdr.delta_q_u_ac),
            _ => self.ac_q(q + self.hdr.delta_q_v_ac),
        }
    }

    /// §7.12.3 reconstruct
    pub fn reconstruct(&mut self, plane: usize, x: usize, y: usize, tx_sz: usize) {
        let dq_denom: i64 = match tx_sz {
            TX_32X32 | TX_16X32 | TX_32X16 | TX_16X64 | TX_64X16 => 2,
            TX_64X64 | TX_32X64 | TX_64X32 => 4,
            _ => 1,
        };
        let log2w = TX_WIDTH_LOG2[tx_sz] as u32;
        let log2h = TX_HEIGHT_LOG2[tx_sz] as u32;
        let w = 1usize << log2w;
        let h = 1usize << log2h;
        let tw = w.min(32);
        let th = h.min(32);
        let t = self.plane_tx_type;
        let flip_ud = matches!(t, FLIPADST_DCT | FLIPADST_ADST | V_FLIPADST | FLIPADST_FLIPADST);
        let flip_lr = matches!(t, DCT_FLIPADST | ADST_FLIPADST | H_FLIPADST | FLIPADST_FLIPADST);
        let bd = self.fs.bit_depth;
        let mut dequant = alloc::vec![0i32; 64 * 64];
        let dcq = self.get_dc_quant(plane) as i64;
        let acq = self.get_ac_quant(plane) as i64;
        let qm_level = if self.hdr.using_qmatrix { self.hdr.seg_qm_level[plane][self.segment_id] } else { 15 };
        let lim = 1i64 << (7 + bd);
        for i in 0..th {
            for j in 0..tw {
                let qv = self.quant[i * tw + j];
                if qv == 0 {
                    continue;
                }
                let q = if i == 0 && j == 0 { dcq } else { acq };
                let q2 = if self.hdr.using_qmatrix && t < IDTX && qm_level < 15 {
                    round2(q * QUANTIZER_MATRIX[qm_level as usize][(plane > 0) as usize][QM_OFFSET[tx_sz] as usize + i * tw + j] as i64, 5)
                } else {
                    q
                };
                let dq = qv as i64 * q2;
                let sign: i64 = if dq < 0 { -1 } else { 1 };
                let dq2 = sign * ((dq.abs() & 0xFFFFFF) / dq_denom);
                dequant[i * 64 + j] = dq2.clamp(-lim, lim - 1) as i32;
            }
        }
        let mut residual = alloc::vec![0i32; 64 * 64];
        inverse_transform_2d(&dequant, &mut residual, tx_sz, t, self.lossless, bd);
        let maxv = (1i32 << bd) - 1;
        let p = &mut self.fs.planes[plane];
        for i in 0..h {
            for j in 0..w {
                let xx = if flip_lr { w - j - 1 } else { j };
                let yy = if flip_ud { h - i - 1 } else { i };
                let v = p.get(x + xx, y + yy) as i32 + residual[i * 64 + j];
                p.set(x + xx, y + yy, v.clamp(0, maxv) as u16);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::f64::consts::PI;

    /// The integer DCT must track the real inverse DCT-II closely (KAT against the textbook
    /// formula: x[n] = sum_k c_k X[k] cos(pi (2n+1) k / 2N), c_0 = 1/sqrt2, scaled by sqrt(2/N)*...
    /// AV1's network computes x = sum X[k] * cos(...) with DC weight 1/sqrt(2) and no 2/N scale).
    #[test]
    fn idct_matches_float() {
        for n in 2..=6u32 {
            let len = 1usize << n;
            let mut input = [0i32; 64];
            for k in 0..len {
                input[k] = ((k as i32 * 37 + 11) % 61 - 30) * 64;
            }
            let mut t = input;
            let mut tx = Tx { t: &mut t, r: 24 };
            tx.idct(n);
            for x in 0..len {
                let mut s = 0.0f64;
                for k in 0..len {
                    let ck = if k == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                    s += ck * input[k] as f64 * (PI * (2 * x + 1) as f64 * k as f64 / (2.0 * len as f64)).cos();
                }
                let err = (t[x] as f64 - s).abs();
                assert!(err < 2.0 + len as f64 * 0.1, "n={n} x={x} int={} float={s}", t[x]);
            }
        }
    }

    /// ADST4 against its definition: x[n] = sum_k X[k] * sin(pi (n+1)(2k+1) / 9) * (2*sqrt2/3)
    /// with AV1's integer scaling (sinpi constants are 4096*2*sqrt(2)/3*sin(k*pi/9)).
    #[test]
    fn adst4_matches_float() {
        let input = [1000i32, -700, 300, 50];
        let mut t = [0i32; 64];
        t[..4].copy_from_slice(&input);
        let mut tx = Tx { t: &mut t, r: 24 };
        tx.adst4();
        for n in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += input[k] as f64 * (PI * ((n + 1) * (2 * k + 1)) as f64 / 9.0).sin();
            }
            s *= 2.0 * 2f64.sqrt() / 3.0;
            assert!((t[n] as f64 - s).abs() < 2.0, "n={n} {} vs {s}", t[n]);
        }
    }

    /// WHT round trip on a lossless block: an all-DC input spreads evenly.
    #[test]
    fn wht_dc() {
        let mut deq = [0i32; 64 * 64];
        deq[0] = 4 * 16; // pre-scale shift 2 on rows
        let mut res = [0i32; 64 * 64];
        inverse_transform_2d(&deq, &mut res, TX_4X4, DCT_DCT, true, 8);
        let v = res[0];
        for i in 0..4 {
            for j in 0..4 {
                assert_eq!(res[i * 64 + j], v);
            }
        }
    }
}
