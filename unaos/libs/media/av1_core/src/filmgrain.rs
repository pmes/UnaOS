//! §7.18.3 film grain synthesis: the 16-bit LFSR random number process, auto-regressive grain
//! generation for luma and chroma (with the luma correlation term), the piecewise-linear scaling
//! lookup, 32-line noise stripes with optional block overlap, and the final blend (with the
//! restricted-range clip). Applied to the output arrays only — reference frames keep the
//! un-grained samples (§7.20 stores LrFrame).

use crate::obu::FilmGrainParams;
use crate::tables::GAUSSIAN_SEQUENCE;
use alloc::vec;
use alloc::vec::Vec;

#[inline]
fn round2(x: i32, n: u32) -> i32 {
    if n == 0 { x } else { (x + (1 << (n - 1))) >> n }
}

struct Rng(u32);
impl Rng {
    /// get_random_number( bits ) (§7.18.3.2)
    fn get(&mut self, bits: u32) -> u32 {
        let r = self.0;
        let bit = ((r >> 0) ^ (r >> 1) ^ (r >> 3) ^ (r >> 12)) & 1;
        let r = (r >> 1) | (bit << 15);
        self.0 = r;
        (r >> (16 - bits)) & ((1 << bits) - 1)
    }
}

/// The film grain synthesis process (§7.18.3) on output planes `y`, `u`, `v` of a `w` x `h` frame.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    g: &FilmGrainParams,
    bit_depth: u32,
    mono: bool,
    sub_x: u32,
    sub_y: u32,
    matrix_coefficients: u8,
    w: usize,
    h: usize,
    y: &mut [u16],
    u: &mut [u16],
    v: &mut [u16],
) {
    let num_planes = if mono { 1 } else { 3 };
    let grain_center = 128i32 << (bit_depth - 8);
    let grain_min = -grain_center;
    let grain_max = (256i32 << (bit_depth - 8)) - 1 - grain_center;
    let mut rng = Rng(g.grain_seed as u32);
    // ---- generate grain (§7.18.3.3)
    let mut luma_grain = vec![[0i32; 82]; 73];
    let shift = 12 - bit_depth + g.grain_scale_shift as u32;
    for row in luma_grain.iter_mut() {
        for v in row.iter_mut() {
            let gg = if g.num_y_points > 0 { GAUSSIAN_SEQUENCE[rng.get(11) as usize] as i32 } else { 0 };
            *v = round2(gg, shift);
        }
    }
    let ar_shift = g.ar_coeff_shift_minus_6 as u32 + 6;
    let lag = g.ar_coeff_lag as i32;
    for yy in 3..73usize {
        for xx in 3..82 - 3usize {
            let mut s = 0i32;
            let mut pos = 0;
            'outer: for dr in -lag..=0 {
                for dc in -lag..=lag {
                    if dr == 0 && dc == 0 {
                        break 'outer;
                    }
                    let c = g.ar_coeffs_y_plus_128.get(pos).copied().unwrap_or(128) as i32 - 128;
                    s += luma_grain[(yy as i32 + dr) as usize][(xx as i32 + dc) as usize] * c;
                    pos += 1;
                }
            }
            luma_grain[yy][xx] = (luma_grain[yy][xx] + round2(s, ar_shift)).clamp(grain_min, grain_max);
        }
    }
    let chroma_w = if sub_x != 0 { 44 } else { 82 };
    let chroma_h = if sub_y != 0 { 38 } else { 73 };
    let mut cb_grain = vec![vec![0i32; chroma_w]; chroma_h];
    let mut cr_grain = vec![vec![0i32; chroma_w]; chroma_h];
    if !mono {
        rng.0 = g.grain_seed as u32 ^ 0xb524;
        for row in cb_grain.iter_mut() {
            for v in row.iter_mut() {
                let gg = if g.num_cb_points > 0 || g.chroma_scaling_from_luma { GAUSSIAN_SEQUENCE[rng.get(11) as usize] as i32 } else { 0 };
                *v = round2(gg, shift);
            }
        }
        rng.0 = g.grain_seed as u32 ^ 0x49d8;
        for row in cr_grain.iter_mut() {
            for v in row.iter_mut() {
                let gg = if g.num_cr_points > 0 || g.chroma_scaling_from_luma { GAUSSIAN_SEQUENCE[rng.get(11) as usize] as i32 } else { 0 };
                *v = round2(gg, shift);
            }
        }
        for yy in 3..chroma_h {
            for xx in 3..chroma_w - 3 {
                let mut s0 = 0i32;
                let mut s1 = 0i32;
                let mut pos = 0;
                'outer2: for dr in -lag..=0 {
                    for dc in -lag..=lag {
                        let c0 = g.ar_coeffs_cb_plus_128.get(pos).copied().unwrap_or(128) as i32 - 128;
                        let c1 = g.ar_coeffs_cr_plus_128.get(pos).copied().unwrap_or(128) as i32 - 128;
                        if dr == 0 && dc == 0 {
                            if g.num_y_points > 0 {
                                let mut luma = 0i32;
                                let luma_x = ((xx - 3) << sub_x) + 3;
                                let luma_y = ((yy - 3) << sub_y) + 3;
                                for i in 0..=sub_y as usize {
                                    for j in 0..=sub_x as usize {
                                        luma += luma_grain[luma_y + i][luma_x + j];
                                    }
                                }
                                luma = round2(luma, sub_x + sub_y);
                                s0 += luma * c0;
                                s1 += luma * c1;
                            }
                            break 'outer2;
                        }
                        s0 += cb_grain[(yy as i32 + dr) as usize][(xx as i32 + dc) as usize] * c0;
                        s1 += cr_grain[(yy as i32 + dr) as usize][(xx as i32 + dc) as usize] * c1;
                        pos += 1;
                    }
                }
                cb_grain[yy][xx] = (cb_grain[yy][xx] + round2(s0, ar_shift)).clamp(grain_min, grain_max);
                cr_grain[yy][xx] = (cr_grain[yy][xx] + round2(s1, ar_shift)).clamp(grain_min, grain_max);
            }
        }
    }
    // ---- scaling lookup (§7.18.3.4)
    let mut scaling_lut = [[0i32; 256]; 3];
    for plane in 0..num_planes {
        let (xs, ys): (&[u8], &[u8]) = if plane == 0 || g.chroma_scaling_from_luma {
            (&g.point_y_value, &g.point_y_scaling)
        } else if plane == 1 {
            (&g.point_cb_value, &g.point_cb_scaling)
        } else {
            (&g.point_cr_value, &g.point_cr_scaling)
        };
        let num_points = xs.len();
        if num_points == 0 {
            continue;
        }
        for x in 0..xs[0] as usize {
            scaling_lut[plane][x] = ys[0] as i32;
        }
        for i in 0..num_points - 1 {
            let delta_y = ys[i + 1] as i32 - ys[i] as i32;
            let delta_x = xs[i + 1] as i32 - xs[i] as i32;
            let delta = delta_y * ((65536 + (delta_x >> 1)) / delta_x);
            for x in 0..delta_x {
                let v = ys[i] as i32 + ((x * delta + 32768) >> 16);
                scaling_lut[plane][(xs[i] as i32 + x) as usize] = v;
            }
        }
        for x in xs[num_points - 1] as usize..256 {
            scaling_lut[plane][x] = ys[num_points - 1] as i32;
        }
    }
    let scale_lut = |plane: usize, index: i32| -> i32 {
        let shift = bit_depth - 8;
        let x = index >> shift;
        let rem = index - (x << shift);
        if bit_depth == 8 || x == 255 {
            scaling_lut[plane][x as usize]
        } else {
            let start = scaling_lut[plane][x as usize];
            let end = scaling_lut[plane][x as usize + 1];
            start + round2((end - start) * rem, shift)
        }
    };
    // ---- add noise (§7.18.3.5): noise stripes
    let stripes = (h + 1) / 2 / 16 + 2;
    let sw = [w + 64, (w >> sub_x) + 64, (w >> sub_x) + 64];
    let sh = [34usize, 34 >> sub_y, 34 >> sub_y];
    let mut noise_stripe: Vec<[Vec<i32>; 3]> = (0..stripes).map(|_| [vec![0; sw[0] * sh[0]], vec![0; sw[1] * sh[1]], vec![0; sw[2] * sh[2]]]).collect();
    let mut luma_num = 0usize;
    let mut yy = 0usize;
    while yy < (h + 1) / 2 {
        rng.0 = g.grain_seed as u32;
        rng.0 ^= (((luma_num * 37 + 178) & 255) << 8) as u32;
        rng.0 ^= ((luma_num * 173 + 105) & 255) as u32;
        let mut xx = 0usize;
        while xx < (w + 1) / 2 {
            let rand = rng.get(8) as usize;
            let offset_x = rand >> 4;
            let offset_y = rand & 15;
            for plane in 0..num_planes {
                let psx = if plane > 0 { sub_x } else { 0 };
                let psy = if plane > 0 { sub_y } else { 0 };
                let pox = if psx != 0 { 6 + offset_x } else { 9 + offset_x * 2 };
                let poy = if psy != 0 { 6 + offset_y } else { 9 + offset_y * 2 };
                let stride = sw[plane];
                for i in 0..(34 >> psy) {
                    for j in 0..(34 >> psx) {
                        let mut gv = match plane {
                            0 => luma_grain[poy + i][pox + j],
                            1 => cb_grain[poy + i][pox + j],
                            _ => cr_grain[poy + i][pox + j],
                        };
                        let ns = &mut noise_stripe[luma_num][plane];
                        if psx == 0 {
                            let idx = i * stride + xx * 2 + j;
                            if j < 2 && g.overlap_flag && xx > 0 {
                                let old = ns[idx];
                                gv = if j == 0 { old * 27 + gv * 17 } else { old * 17 + gv * 27 };
                                gv = round2(gv, 5).clamp(grain_min, grain_max);
                            }
                            ns[idx] = gv;
                        } else {
                            let idx = i * stride + xx + j;
                            if j == 0 && g.overlap_flag && xx > 0 {
                                let old = ns[idx];
                                gv = old * 23 + gv * 22;
                                gv = round2(gv, 5).clamp(grain_min, grain_max);
                            }
                            ns[idx] = gv;
                        }
                    }
                }
            }
            xx += 16;
        }
        luma_num += 1;
        yy += 16;
    }
    // noise image
    let mut noise_image: [Vec<i32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for plane in 0..num_planes {
        let psx = if plane > 0 { sub_x } else { 0 } as usize;
        let psy = if plane > 0 { sub_y } else { 0 } as usize;
        let pw = (w + psx) >> psx;
        let ph = (h + psy) >> psy;
        let stride = sw[plane];
        let mut img = vec![0i32; pw * ph];
        for y2 in 0..ph {
            let ln = y2 >> (5 - psy);
            let i = y2 - (ln << (5 - psy));
            for x2 in 0..pw {
                let mut gv = noise_stripe[ln][plane][i * stride + x2];
                if psy == 0 {
                    if i < 2 && ln > 0 && g.overlap_flag {
                        let old = noise_stripe[ln - 1][plane][(i + 32) * stride + x2];
                        gv = if i == 0 { old * 27 + gv * 17 } else { old * 17 + gv * 27 };
                        gv = round2(gv, 5).clamp(grain_min, grain_max);
                    }
                } else if i < 1 && ln > 0 && g.overlap_flag {
                    let old = noise_stripe[ln - 1][plane][(i + 16) * stride + x2];
                    gv = old * 23 + gv * 22;
                    gv = round2(gv, 5).clamp(grain_min, grain_max);
                }
                img[y2 * pw + x2] = gv;
            }
        }
        noise_image[plane] = img;
    }
    // blend
    let (min_value, max_luma, max_chroma) = if g.clip_to_restricted_range {
        let min_v = 16i32 << (bit_depth - 8);
        let max_l = 235i32 << (bit_depth - 8);
        let max_c = if matrix_coefficients == crate::tables::MC_IDENTITY as u8 { max_l } else { 240i32 << (bit_depth - 8) };
        (min_v, max_l, max_c)
    } else {
        let m = (256i32 << (bit_depth - 8)) - 1;
        (0, m, m)
    };
    let scaling_shift = g.grain_scaling_minus_8 as u32 + 8;
    let maxv = (1i32 << bit_depth) - 1;
    if !mono {
        let cw = (w + sub_x as usize) >> sub_x;
        let ch = (h + sub_y as usize) >> sub_y;
        for y2 in 0..ch {
            for x2 in 0..cw {
                let luma_x = x2 << sub_x;
                let luma_y = y2 << sub_y;
                let luma_next_x = (luma_x + 1).min(w - 1);
                let average_luma = if sub_x != 0 {
                    round2(y[luma_y * w + luma_x] as i32 + y[luma_y * w + luma_next_x] as i32, 1)
                } else {
                    y[luma_y * w + luma_x] as i32
                };
                if g.num_cb_points > 0 || g.chroma_scaling_from_luma {
                    let orig = u[y2 * cw + x2] as i32;
                    let merged = if g.chroma_scaling_from_luma {
                        average_luma
                    } else {
                        let combined = average_luma * (g.cb_luma_mult as i32 - 128) + orig * (g.cb_mult as i32 - 128);
                        ((combined >> 6) + ((g.cb_offset as i32 - 256) << (bit_depth - 8))).clamp(0, maxv)
                    };
                    let noise = round2(scale_lut(1, merged) * noise_image[1][y2 * cw + x2], scaling_shift);
                    u[y2 * cw + x2] = (orig + noise).clamp(min_value, max_chroma) as u16;
                }
                if g.num_cr_points > 0 || g.chroma_scaling_from_luma {
                    let orig = v[y2 * cw + x2] as i32;
                    let merged = if g.chroma_scaling_from_luma {
                        average_luma
                    } else {
                        let combined = average_luma * (g.cr_luma_mult as i32 - 128) + orig * (g.cr_mult as i32 - 128);
                        ((combined >> 6) + ((g.cr_offset as i32 - 256) << (bit_depth - 8))).clamp(0, maxv)
                    };
                    let noise = round2(scale_lut(2, merged) * noise_image[2][y2 * cw + x2], scaling_shift);
                    v[y2 * cw + x2] = (orig + noise).clamp(min_value, max_chroma) as u16;
                }
            }
        }
    }
    if g.num_y_points > 0 {
        for y2 in 0..h {
            for x2 in 0..w {
                let orig = y[y2 * w + x2] as i32;
                let noise = round2(scale_lut(0, orig) * noise_image[0][y2 * w + x2], scaling_shift);
                y[y2 * w + x2] = (orig + noise).clamp(min_value, max_luma) as u16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lfsr_matches_the_spec_recurrence() {
        // bit = r0 ^ r1 ^ r3 ^ r12, shifted in at bit 15
        let mut r = Rng(1);
        assert_eq!(r.get(16), 0x8000);
        let mut r = Rng(0);
        assert_eq!(r.get(11), 0); // the all-zero state is a fixed point
    }
}
