//! §7.11.2 intra prediction (DC, V/H and the six other directional modes with angle deltas,
//! intra edge filtering and upsampling, SMOOTH/SMOOTH_V/SMOOTH_H, PAETH, recursive filter intra),
//! §7.11.4 palette prediction and §7.11.5 chroma-from-luma.

use crate::decode::{round2, Dec};
use crate::tables::*;

const OFF: usize = 16; // AboveRow[ -16 .. ] lives at index 0

fn sm_weights(log2: u32) -> &'static [u8] {
    match log2 {
        2 => &SM_WEIGHTS_TX_4X4,
        3 => &SM_WEIGHTS_TX_8X8,
        4 => &SM_WEIGHTS_TX_16X16,
        5 => &SM_WEIGHTS_TX_32X32,
        _ => &SM_WEIGHTS_TX_64X64,
    }
}

/// §7.11.2.9
fn intra_edge_filter_strength(w: i32, h: i32, filter_type: bool, delta: i32) -> u32 {
    let d = delta.abs();
    let blk_wh = w + h;
    let mut strength = 0;
    if !filter_type {
        if blk_wh <= 8 {
            if d >= 56 {
                strength = 1;
            }
        } else if blk_wh <= 12 {
            if d >= 40 {
                strength = 1;
            }
        } else if blk_wh <= 16 {
            if d >= 40 {
                strength = 1;
            }
        } else if blk_wh <= 24 {
            if d >= 8 {
                strength = 1;
            }
            if d >= 16 {
                strength = 2;
            }
            if d >= 32 {
                strength = 3;
            }
        } else if blk_wh <= 32 {
            strength = 1;
            if d >= 4 {
                strength = 2;
            }
            if d >= 32 {
                strength = 3;
            }
        } else {
            strength = 3;
        }
    } else if blk_wh <= 8 {
        if d >= 40 {
            strength = 1;
        }
        if d >= 64 {
            strength = 2;
        }
    } else if blk_wh <= 16 {
        if d >= 20 {
            strength = 1;
        }
        if d >= 48 {
            strength = 2;
        }
    } else if blk_wh <= 24 {
        if d >= 4 {
            strength = 3;
        }
    } else {
        strength = 3;
    }
    strength
}

/// §7.11.2.10
fn intra_edge_upsample(w: i32, h: i32, filter_type: bool, delta: i32) -> bool {
    let d = delta.abs();
    let blk_wh = w + h;
    if d <= 0 || d >= 40 {
        false
    } else if !filter_type {
        blk_wh <= 16
    } else {
        blk_wh <= 8
    }
}

/// §7.11.2.12 on buf (index OFF == element 0 of the spec's array, so buf[OFF-1] is [-1]).
fn edge_filter(buf: &mut [i32], sz: usize, strength: u32) {
    if strength == 0 {
        return;
    }
    let mut edge = [0i32; 160];
    for i in 0..sz {
        edge[i] = buf[OFF + i - 1];
    }
    for i in 1..sz {
        let mut s = 0;
        for j in 0..INTRA_EDGE_TAPS {
            let k = (i as isize - 2 + j as isize).clamp(0, sz as isize - 1) as usize;
            s += INTRA_EDGE_KERNEL[strength as usize - 1][j] as i32 * edge[k];
        }
        buf[OFF + i - 1] = (s + 8) >> 4;
    }
}

/// §7.11.2.11
fn edge_upsample(buf: &mut [i32], num_px: usize, bit_depth: u32) {
    let mut dup = [0i32; 64];
    dup[0] = buf[OFF - 1];
    for i in -1..(num_px as isize) {
        dup[(i + 2) as usize] = buf[(OFF as isize + i) as usize];
    }
    dup[num_px + 2] = buf[OFF + num_px - 1];
    buf[OFF - 2] = dup[0];
    let maxv = (1i32 << bit_depth) - 1;
    for i in 0..num_px {
        let mut s = -dup[i] + 9 * dup[i + 1] + 9 * dup[i + 2] - dup[i + 3];
        s = (round2(s as i64, 4) as i32).clamp(0, maxv);
        buf[(OFF as isize + 2 * i as isize - 1) as usize] = s;
        buf[OFF + 2 * i] = dup[i + 2];
    }
}

impl<'a, 'f> Dec<'a, 'f> {
    fn is_smooth(&self, row: usize, col: usize, plane: usize) -> bool {
        let i = self.fs.mi(row, col);
        let mode = if plane == 0 { self.fs.y_modes[i] as usize } else { self.fs.uv_modes[i] as usize };
        mode == SMOOTH_PRED || mode == SMOOTH_V_PRED || mode == SMOOTH_H_PRED
    }

    /// §7.11.2.8 get_filter_type
    fn get_filter_type(&self, plane: usize) -> bool {
        let mut above_smooth = false;
        let mut left_smooth = false;
        let (ssx, ssy) = (self.fs.ss_x, self.fs.ss_y);
        if if plane == 0 { self.avail_u } else { self.avail_u_chroma } {
            let mut r = self.mi_row as isize - 1;
            let mut c = self.mi_col as isize;
            if plane > 0 {
                if ssx != 0 && (self.mi_col & 1) == 0 {
                    c += 1;
                }
                if ssy != 0 && (self.mi_row & 1) != 0 {
                    r -= 1;
                }
            }
            above_smooth = self.is_smooth(r as usize, c as usize, plane);
        }
        if if plane == 0 { self.avail_l } else { self.avail_l_chroma } {
            let mut r = self.mi_row as isize;
            let mut c = self.mi_col as isize - 1;
            if plane > 0 {
                if ssx != 0 && (self.mi_col & 1) != 0 {
                    c -= 1;
                }
                if ssy != 0 && (self.mi_row & 1) == 0 {
                    r += 1;
                }
            }
            left_smooth = self.is_smooth(r as usize, c as usize, plane);
        }
        above_smooth || left_smooth
    }

    /// predict_intra (§7.11.2.1)
    #[allow(clippy::too_many_arguments)]
    pub fn predict_intra(
        &mut self,
        plane: usize,
        x: usize,
        y: usize,
        have_left: bool,
        have_above: bool,
        have_above_right: bool,
        have_below_left: bool,
        mode: usize,
        log2w: u32,
        log2h: u32,
    ) {
        let w = 1usize << log2w;
        let h = 1usize << log2h;
        let bd = self.fs.bit_depth;
        let (ssx, ssy) = if plane > 0 { (self.fs.ss_x, self.fs.ss_y) } else { (0, 0) };
        let max_x = ((self.fs.mi_cols * MI_SIZE) >> ssx) as isize - 1;
        let max_y = ((self.fs.mi_rows * MI_SIZE) >> ssy) as isize - 1;
        let mut above = [0i32; 300];
        let mut left = [0i32; 300];
        {
            let p = &self.fs.planes[plane];
            let base = 1i32 << (bd - 1);
            for i in 0..(w + h) {
                above[OFF + i] = if !have_above && have_left {
                    p.get(x - 1, y) as i32
                } else if !have_above && !have_left {
                    base - 1
                } else {
                    let above_limit = max_x.min(x as isize + if have_above_right { 2 * w } else { w } as isize - 1);
                    p.get(above_limit.min((x + i) as isize) as usize, y - 1) as i32
                };
                left[OFF + i] = if !have_left && have_above {
                    p.get(x, y - 1) as i32
                } else if !have_left && !have_above {
                    base + 1
                } else {
                    let left_limit = max_y.min(y as isize + if have_below_left { 2 * h } else { h } as isize - 1);
                    p.get(x - 1, left_limit.min((y + i) as isize) as usize) as i32
                };
            }
            let corner = if have_above && have_left {
                p.get(x - 1, y - 1) as i32
            } else if have_above {
                p.get(x, y - 1) as i32
            } else if have_left {
                p.get(x - 1, y) as i32
            } else {
                base
            };
            above[OFF - 1] = corner;
            left[OFF - 1] = corner;
        }
        let maxv = (1i32 << bd) - 1;
        let mut pred = [0i32; 64 * 64];
        let is_directional = (V_PRED..=D67_PRED).contains(&mode);
        if plane == 0 && self.use_filter_intra {
            // §7.11.2.3 recursive intra prediction
            let w4 = w >> 2;
            let h2 = h >> 1;
            for i2 in 0..h2 {
                for j4 in 0..w4 {
                    let mut pp = [0i32; 7];
                    for i in 0..7 {
                        pp[i] = if i < 5 {
                            if i2 == 0 {
                                above[(OFF as isize + ((j4 << 2) + i) as isize - 1) as usize]
                            } else if j4 == 0 && i == 0 {
                                left[OFF + (i2 << 1) - 1]
                            } else {
                                pred[((i2 << 1) - 1) * 64 + (j4 << 2) + i - 1]
                            }
                        } else if j4 == 0 {
                            left[OFF + (i2 << 1) + i - 5]
                        } else {
                            pred[((i2 << 1) + i - 5) * 64 + (j4 << 2) - 1]
                        };
                    }
                    for i1 in 0..2 {
                        for j1 in 0..4 {
                            let mut pr = 0i32;
                            for i in 0..7 {
                                pr += INTRA_FILTER_TAPS[self.filter_intra_mode][(i1 << 2) + j1][i] as i32 * pp[i];
                            }
                            let v = round2_signed(pr, INTRA_FILTER_SCALE_BITS as u32).clamp(0, maxv);
                            pred[((i2 << 1) + i1) * 64 + (j4 << 2) + j1] = v;
                        }
                    }
                }
            }
        } else if is_directional {
            // §7.11.2.4
            let angle_delta = if plane == 0 { self.angle_delta_y } else { self.angle_delta_uv };
            let p_angle = MODE_TO_ANGLE[mode] as i32 + angle_delta * ANGLE_STEP as i32;
            let mut upsample_above = 0u32;
            let mut upsample_left = 0u32;
            if self.seq.enable_intra_edge_filter {
                if p_angle != 90 && p_angle != 180 {
                    if p_angle > 90 && p_angle < 180 && (w + h) >= 24 {
                        // filter corner
                        let s = left[OFF] * 5 + above[OFF - 1] * 6 + above[OFF] * 5;
                        let v = round2(s as i64, 4) as i32;
                        left[OFF - 1] = v;
                        above[OFF - 1] = v;
                    }
                    let filter_type = self.get_filter_type(plane);
                    if have_above {
                        let strength = intra_edge_filter_strength(w as i32, h as i32, filter_type, p_angle - 90);
                        let num_px = w.min((max_x - x as isize + 1) as usize) + if p_angle < 90 { h } else { 0 } + 1;
                        edge_filter(&mut above, num_px, strength);
                    }
                    if have_left {
                        let strength = intra_edge_filter_strength(w as i32, h as i32, filter_type, p_angle - 180);
                        let num_px = h.min((max_y - y as isize + 1) as usize) + if p_angle > 180 { w } else { 0 } + 1;
                        edge_filter(&mut left, num_px, strength);
                    }
                }
                let filter_type = self.get_filter_type(plane);
                upsample_above = intra_edge_upsample(w as i32, h as i32, filter_type, p_angle - 90) as u32;
                let num_px = w + if p_angle < 90 { h } else { 0 };
                if upsample_above != 0 {
                    edge_upsample(&mut above, num_px, bd);
                }
                upsample_left = intra_edge_upsample(w as i32, h as i32, filter_type, p_angle - 180) as u32;
                let num_px = h + if p_angle > 180 { w } else { 0 };
                if upsample_left != 0 {
                    edge_upsample(&mut left, num_px, bd);
                }
            }
            let dx = if p_angle < 90 {
                DR_INTRA_DERIVATIVE[p_angle as usize] as i32
            } else if p_angle > 90 && p_angle < 180 {
                DR_INTRA_DERIVATIVE[(180 - p_angle) as usize] as i32
            } else {
                0
            };
            let dy = if p_angle > 90 && p_angle < 180 {
                DR_INTRA_DERIVATIVE[(p_angle - 90) as usize] as i32
            } else if p_angle > 180 {
                DR_INTRA_DERIVATIVE[(270 - p_angle) as usize] as i32
            } else {
                0
            };
            let a = |i: i32| above[(OFF as i32 + i) as usize];
            let l = |i: i32| left[(OFF as i32 + i) as usize];
            for i in 0..h as i32 {
                for j in 0..w as i32 {
                    let v = if p_angle < 90 {
                        let idx = (i + 1) * dx;
                        let base = (idx >> (6 - upsample_above)) + (j << upsample_above);
                        let shift = ((idx << upsample_above) >> 1) & 0x1f;
                        let max_base_x = ((w + h - 1) << upsample_above) as i32;
                        if base < max_base_x {
                            round2((a(base) * (32 - shift) + a(base + 1) * shift) as i64, 5) as i32
                        } else {
                            a(max_base_x)
                        }
                    } else if p_angle > 90 && p_angle < 180 {
                        let idx = (j << 6) - (i + 1) * dx;
                        let base = idx >> (6 - upsample_above);
                        if base >= -(1 << upsample_above) {
                            let shift = ((idx << upsample_above) >> 1) & 0x1f;
                            round2((a(base) * (32 - shift) + a(base + 1) * shift) as i64, 5) as i32
                        } else {
                            let idx = (i << 6) - (j + 1) * dy;
                            let base = idx >> (6 - upsample_left);
                            let shift = ((idx << upsample_left) >> 1) & 0x1f;
                            round2((l(base) * (32 - shift) + l(base + 1) * shift) as i64, 5) as i32
                        }
                    } else if p_angle > 180 {
                        let idx = (j + 1) * dy;
                        let base = (idx >> (6 - upsample_left)) + (i << upsample_left);
                        let shift = ((idx << upsample_left) >> 1) & 0x1f;
                        round2((l(base) * (32 - shift) + l(base + 1) * shift) as i64, 5) as i32
                    } else if p_angle == 90 {
                        a(j)
                    } else {
                        l(i)
                    };
                    pred[i as usize * 64 + j as usize] = v;
                }
            }
        } else if mode == SMOOTH_PRED {
            let wx = sm_weights(log2w);
            let wy = sm_weights(log2h);
            for i in 0..h {
                for j in 0..w {
                    let s = wy[i] as i32 * above[OFF + j]
                        + (256 - wy[i] as i32) * left[OFF + h - 1]
                        + wx[j] as i32 * left[OFF + i]
                        + (256 - wx[j] as i32) * above[OFF + w - 1];
                    pred[i * 64 + j] = round2(s as i64, 9) as i32;
                }
            }
        } else if mode == SMOOTH_V_PRED {
            let wy = sm_weights(log2h);
            for i in 0..h {
                for j in 0..w {
                    let s = wy[i] as i32 * above[OFF + j] + (256 - wy[i] as i32) * left[OFF + h - 1];
                    pred[i * 64 + j] = round2(s as i64, 8) as i32;
                }
            }
        } else if mode == SMOOTH_H_PRED {
            let wx = sm_weights(log2w);
            for i in 0..h {
                for j in 0..w {
                    let s = wx[j] as i32 * left[OFF + i] + (256 - wx[j] as i32) * above[OFF + w - 1];
                    pred[i * 64 + j] = round2(s as i64, 8) as i32;
                }
            }
        } else if mode == DC_PRED {
            let v = if have_left && have_above {
                let mut sum = 0i32;
                for k in 0..h {
                    sum += left[OFF + k];
                }
                for k in 0..w {
                    sum += above[OFF + k];
                }
                sum += ((w + h) >> 1) as i32;
                sum / (w + h) as i32
            } else if have_left {
                let mut sum = 0i32;
                for k in 0..h {
                    sum += left[OFF + k];
                }
                ((sum + (h >> 1) as i32) >> log2h).clamp(0, maxv)
            } else if have_above {
                let mut sum = 0i32;
                for k in 0..w {
                    sum += above[OFF + k];
                }
                ((sum + (w >> 1) as i32) >> log2w).clamp(0, maxv)
            } else {
                1 << (bd - 1)
            };
            for i in 0..h {
                for j in 0..w {
                    pred[i * 64 + j] = v;
                }
            }
        } else {
            // PAETH (§7.11.2.2)
            let tl = above[OFF - 1];
            for i in 0..h {
                for j in 0..w {
                    let a = above[OFF + j];
                    let l = left[OFF + i];
                    let base = a + l - tl;
                    let p_left = (base - l).abs();
                    let p_top = (base - a).abs();
                    let p_top_left = (base - tl).abs();
                    pred[i * 64 + j] = if p_left <= p_top && p_left <= p_top_left {
                        l
                    } else if p_top <= p_top_left {
                        a
                    } else {
                        tl
                    };
                }
            }
        }
        let p = &mut self.fs.planes[plane];
        for i in 0..h {
            for j in 0..w {
                p.set(x + j, y + i, pred[i * 64 + j] as u16);
            }
        }
    }

    /// §7.11.4
    pub fn predict_palette(&mut self, plane: usize, start_x: usize, start_y: usize, x: usize, y: usize, tx_sz: usize) {
        let w = TX_WIDTH[tx_sz] as usize;
        let h = TX_HEIGHT[tx_sz] as usize;
        let palette = match plane {
            0 => self.palette_colors_y,
            1 => self.palette_colors_u,
            _ => self.palette_colors_v,
        };
        for i in 0..h {
            for j in 0..w {
                let idx = if plane == 0 {
                    self.color_map_y[(y * 4 + i) * 64 + x * 4 + j]
                } else {
                    self.color_map_uv[(y * 4 + i) * 64 + x * 4 + j]
                };
                self.fs.planes[plane].set(start_x + j, start_y + i, palette[idx as usize]);
            }
        }
    }

    /// §7.11.5
    pub fn predict_chroma_from_luma(&mut self, plane: usize, start_x: usize, start_y: usize, tx_sz: usize) {
        let w = TX_WIDTH[tx_sz] as usize;
        let h = TX_HEIGHT[tx_sz] as usize;
        let sub_x = self.fs.ss_x;
        let sub_y = self.fs.ss_y;
        let alpha = if plane == 1 { self.cfl_alpha_u } else { self.cfl_alpha_v };
        let mut l = [0i32; 32 * 32];
        let mut luma_avg: i64 = 0;
        {
            let luma = &self.fs.planes[0];
            for i in 0..h {
                let luma_y = ((start_y + i) << sub_y).min(self.max_luma_h - (1 << sub_y));
                for j in 0..w {
                    let luma_x = ((start_x + j) << sub_x).min(self.max_luma_w - (1 << sub_x));
                    let mut t = 0i32;
                    for dy in 0..=sub_y {
                        for dx in 0..=sub_x {
                            t += luma.get(luma_x + dx, luma_y + dy) as i32;
                        }
                    }
                    let v = t << (3 - sub_x - sub_y);
                    l[i * 32 + j] = v;
                    luma_avg += v as i64;
                }
            }
        }
        let luma_avg = round2(luma_avg, TX_WIDTH_LOG2[tx_sz] as u32 + TX_HEIGHT_LOG2[tx_sz] as u32) as i32;
        let maxv = (1i32 << self.fs.bit_depth) - 1;
        let p = &mut self.fs.planes[plane];
        for i in 0..h {
            for j in 0..w {
                let dc = p.get(start_x + j, start_y + i) as i32;
                let scaled_luma = round2_signed(alpha * (l[i * 32 + j] - luma_avg), 6);
                p.set(start_x + j, start_y + i, (dc + scaled_luma).clamp(0, maxv) as u16);
            }
        }
    }
}

/// Round2Signed
pub fn round2_signed(x: i32, n: u32) -> i32 {
    if x >= 0 { round2(x as i64, n) as i32 } else { -(round2(-(x as i64), n) as i32) }
}

#[cfg(test)]
mod tests {
    use crate::decode::{Dec, FrameState};
    use crate::obu::{FrameHeader, SequenceHeader};
    use crate::tables::*;

    fn setup() -> (SequenceHeader, FrameHeader) {
        let mut s = SequenceHeader::default();
        s.color_config.bit_depth = 8;
        s.color_config.num_planes = 1;
        s.color_config.subsampling_x = 1;
        s.color_config.subsampling_y = 1;
        let h = FrameHeader { mi_cols: 16, mi_rows: 16, frame_width: 64, frame_height: 64, upscaled_width: 64, ..Default::default() };
        (s, h)
    }

    /// Fill the row above (y = 7, x = 7..) and the column left (x = 7, y = 7..) of the 8x8 block
    /// at (8, 8); returns the predicted block.
    fn run(mode: usize, above: impl Fn(usize) -> u16, left: impl Fn(usize) -> u16, corner: u16, fi: Option<usize>) -> [[u16; 8]; 8] {
        let (s, h) = setup();
        let mut fs = FrameState::new(&s, &h);
        for i in 0..24 {
            fs.planes[0].set(8 + i, 7, above(i));
            fs.planes[0].set(7, 8 + i, left(i));
        }
        fs.planes[0].set(7, 7, corner);
        let mut d = Dec::new(&s, &h, &mut fs, crate::image::Filters::default());
        d.mi_row = 2;
        d.mi_col = 2;
        if let Some(m) = fi {
            d.use_filter_intra = true;
            d.filter_intra_mode = m;
        }
        d.predict_intra(0, 8, 8, true, true, true, true, mode, 3, 3);
        let mut out = [[0u16; 8]; 8];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = d.fs.planes[0].get(8 + j, 8 + i);
            }
        }
        out
    }

    #[test]
    fn dc_v_h_paeth_smooth() {
        // DC: (8 * 100 + 8 * 50 + 8) / 16 = 75
        let p = run(DC_PRED, |_| 100, |_| 50, 0, None);
        assert!(p.iter().flatten().all(|&v| v == 75));
        // V copies above, H copies left
        let p = run(V_PRED, |i| 10 + i as u16, |_| 0, 0, None);
        for row in p {
            assert_eq!(row, [10, 11, 12, 13, 14, 15, 16, 17]);
        }
        let p = run(H_PRED, |_| 0, |i| 20 + i as u16, 0, None);
        for (i, row) in p.iter().enumerate() {
            assert!(row.iter().all(|&v| v == 20 + i as u16));
        }
        // PAETH with a flat top equal to the corner picks left everywhere
        let p = run(PAETH_PRED, |_| 40, |i| 60 + i as u16, 40, None);
        for (i, row) in p.iter().enumerate() {
            assert!(row.iter().all(|&v| v == 60 + i as u16), "{row:?}");
        }
        // SMOOTH of constant edges is that constant
        for m in [SMOOTH_PRED, SMOOTH_V_PRED, SMOOTH_H_PRED] {
            let p = run(m, |_| 90, |_| 90, 90, None);
            assert!(p.iter().flatten().all(|&v| v == 90));
        }
    }

    #[test]
    fn directional_45_is_a_diagonal_shift() {
        // pAngle 45: dx = Dr_Intra_Derivative[45] = 64 -> pred[i][j] = AboveRow[i + j + 1]
        assert_eq!(DR_INTRA_DERIVATIVE[45], 64);
        let p = run(D45_PRED, |i| 3 * i as u16, |_| 0, 0, None);
        for i in 0..8 {
            for j in 0..8 {
                let k = (i + j + 1).min(15); // maxBaseX = w + h - 1 = 15
                assert_eq!(p[i][j], 3 * k as u16, "({i},{j})");
            }
        }
    }

    #[test]
    fn filter_intra_has_unit_gain() {
        // every Intra_Filter_Taps row sums to 16 (1.0 in INTRA_FILTER_SCALE_BITS), so flat edges
        // predict flat for all five recursive filter modes
        for m in 0..5 {
            let p = run(DC_PRED, |_| 123, |_| 123, 123, Some(m));
            assert!(p.iter().flatten().all(|&v| v == 123), "mode {m}");
        }
    }
}
