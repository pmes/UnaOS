//! §7.17 loop restoration: per 64-row stripe (offset by 8), per restoration unit, the separable
//! 7-tap Wiener filter (§7.17.4) or the dual self-guided box filters with projection (§7.17.2–3),
//! sourcing samples via the get-source-sample rule (§7.17.6: CDEF output inside the stripe,
//! deblocked-only output for the two lines above and below).

use crate::decode::{round2, FrameState, Plane};
use crate::obu::FrameHeader;
use crate::tables::*;

struct Src<'p> {
    cur: &'p Plane,  // UpscaledCurrFrame
    cdef: &'p Plane, // UpscaledCdefFrame
    stripe_start_y: isize,
    stripe_end_y: isize,
    plane_end_x: isize,
    plane_end_y: isize,
}

impl Src<'_> {
    #[inline]
    fn get(&self, x: isize, y: isize) -> i32 {
        let x = x.min(self.plane_end_x).max(0);
        let y = y.min(self.plane_end_y).max(0);
        if y < self.stripe_start_y {
            let y = (self.stripe_start_y - 2).max(y);
            self.cur.get(x as usize, y as usize) as i32
        } else if y > self.stripe_end_y {
            let y = (self.stripe_end_y + 2).min(y);
            self.cur.get(x as usize, y as usize) as i32
        } else {
            self.cdef.get(x as usize, y as usize) as i32
        }
    }
}

/// Returns LrFrame from UpscaledCurrFrame (`cur`) and UpscaledCdefFrame (`cdef`).
pub fn lr_frame(fs: &FrameState, h: &FrameHeader, cur: &[Plane; 3], cdef: &[Plane; 3]) -> [Plane; 3] {
    let mut out = [cdef[0].clone(), cdef[1].clone(), cdef[2].clone()];
    if !h.uses_lr {
        return out;
    }
    let mut y = 0;
    while y < h.frame_height as usize {
        let mut x = 0;
        while x < h.upscaled_width as usize {
            for plane in 0..fs.num_planes {
                if h.frame_restoration_type[plane] != RESTORE_NONE as u32 {
                    let row = y >> MI_SIZE_LOG2;
                    let col = x >> MI_SIZE_LOG2;
                    restore_block(fs, h, cur, cdef, &mut out, plane, row, col);
                }
            }
            x += MI_SIZE;
        }
        y += MI_SIZE;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn restore_block(fs: &FrameState, h: &FrameHeader, cur: &[Plane; 3], cdef: &[Plane; 3], out: &mut [Plane; 3], plane: usize, row: usize, col: usize) {
    let luma_y = row * MI_SIZE;
    let stripe_num = (luma_y + 8) / 64;
    let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (fs.ss_x, fs.ss_y) };
    let stripe_start_y = (-8 + stripe_num as isize * 64) >> sub_y;
    let stripe_end_y = stripe_start_y + (64 >> sub_y) - 1;
    let unit_size = h.loop_restoration_size[plane] as usize;
    let unit_rows = fs.lr_unit_rows[plane];
    let unit_cols = fs.lr_unit_cols[plane];
    let unit_row = (unit_rows - 1).min(((row * MI_SIZE + 8) >> sub_y) / unit_size);
    let unit_col = (unit_cols - 1).min(((col * MI_SIZE) >> sub_x) / unit_size);
    let plane_end_x = round2(h.upscaled_width as i64, sub_x as u32) as isize - 1;
    let plane_end_y = round2(h.frame_height as i64, sub_y as u32) as isize - 1;
    let x = ((col * MI_SIZE) >> sub_x) as isize;
    let y = ((row * MI_SIZE) >> sub_y) as isize;
    if x > plane_end_x || y > plane_end_y {
        return;
    }
    let w = ((MI_SIZE >> sub_x) as isize).min(plane_end_x - x + 1) as usize;
    let hh = ((MI_SIZE >> sub_y) as isize).min(plane_end_y - y + 1) as usize;
    let ui = unit_row * unit_cols + unit_col;
    let r_type = fs.lr_type[plane][ui] as usize;
    let src = Src { cur: &cur[plane], cdef: &cdef[plane], stripe_start_y, stripe_end_y, plane_end_x, plane_end_y };
    let bd = fs.bit_depth;
    if r_type == RESTORE_WIENER {
        wiener(&src, &mut out[plane], fs.lr_wiener[plane][ui], x, y, w, hh, bd);
    } else if r_type == RESTORE_SGRPROJ {
        self_guided(&src, &cdef[plane], &mut out[plane], fs.lr_sgr_set[plane][ui] as usize, fs.lr_sgr_xqd[plane][ui], x, y, w, hh, bd);
    }
}

fn wiener_coeff(c: [i8; 3]) -> [i32; 7] {
    let mut f = [0i32; 7];
    f[3] = 128;
    for i in 0..3 {
        let v = c[i] as i32;
        f[i] = v;
        f[6 - i] = v;
        f[3] -= 2 * v;
    }
    f
}

#[allow(clippy::too_many_arguments)]
fn wiener(src: &Src, out: &mut Plane, coeffs: [[i8; 3]; 2], x: isize, y: isize, w: usize, h: usize, bd: u32) {
    // rounding variables (§7.11.3.2) with isCompound = 0
    let mut inter_round0 = 3;
    let mut inter_round1 = 11;
    if bd == 12 {
        inter_round0 += 2;
        inter_round1 -= 2;
    }
    let vfilter = wiener_coeff(coeffs[0]);
    let hfilter = wiener_coeff(coeffs[1]);
    let offset = 1i32 << (bd + FILTER_BITS as u32 - inter_round0 - 1);
    let limit = (1i32 << (bd + 1 + FILTER_BITS as u32 - inter_round0)) - 1;
    let mut inter = [[0i32; 4]; 10];
    for r in 0..h + 6 {
        for c in 0..w {
            let mut s = 0i32;
            for t in 0..7 {
                s += hfilter[t] * src.get(x + c as isize + t as isize - 3, y + r as isize - 3);
            }
            let v = round2(s as i64, inter_round0) as i32;
            inter[r][c] = v.clamp(-offset, limit - offset);
        }
    }
    let maxv = (1i32 << bd) - 1;
    for r in 0..h {
        for c in 0..w {
            let mut s = 0i32;
            for t in 0..7 {
                s += vfilter[t] * inter[r + t][c];
            }
            let v = round2(s as i64, inter_round1) as i32;
            out.set((x + c as isize) as usize, (y + r as isize) as usize, v.clamp(0, maxv) as u16);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn box_filter(src: &Src, cdef: &Plane, x: isize, y: isize, w: usize, h: usize, set: usize, pass: usize, bd: u32) -> Option<[[i32; 4]; 4]> {
    let r = SGR_PARAMS[set][pass * 2] as isize;
    if r == 0 {
        return None;
    }
    let eps = SGR_PARAMS[set][pass * 2 + 1] as i64;
    let mut a_arr = [[0i64; 6]; 6];
    let mut b_arr = [[0i64; 6]; 6];
    let n = ((2 * r + 1) * (2 * r + 1)) as i64;
    let n2e = n * n * eps;
    let s = ((1i64 << SGRPROJ_MTABLE_BITS) + n2e / 2) / n2e;
    for i in -1..(h as isize + 1) {
        for j in -1..(w as isize + 1) {
            let mut a = 0i64;
            let mut b = 0i64;
            for dy in -r..=r {
                for dx in -r..=r {
                    let c = src.get(x + j + dx, y + i + dy) as i64;
                    a += c * c;
                    b += c;
                }
            }
            a = round2(a, 2 * (bd - 8));
            let d = round2(b, bd - 8);
            let p = (a * n - d * d).max(0);
            let z = round2(p * s, SGRPROJ_MTABLE_BITS as u32);
            let a2: i64 = if z >= 255 {
                256
            } else if z == 0 {
                1
            } else {
                ((z << SGRPROJ_SGR_BITS) + (z / 2)) / (z + 1)
            };
            let one_over_n = ((1i64 << SGRPROJ_RECIP_BITS) + (n / 2)) / n;
            let b2 = ((1i64 << SGRPROJ_SGR_BITS) - a2) * b * one_over_n;
            a_arr[(i + 1) as usize][(j + 1) as usize] = a2;
            b_arr[(i + 1) as usize][(j + 1) as usize] = round2(b2, SGRPROJ_RECIP_BITS as u32);
        }
    }
    let mut f = [[0i32; 4]; 4];
    for i in 0..h as isize {
        let mut shift = 5;
        if pass == 0 && (i & 1) != 0 {
            shift = 4;
        }
        for j in 0..w as isize {
            let mut a = 0i64;
            let mut b = 0i64;
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let weight: i64 = if pass == 0 {
                        if ((i + dy) & 1) != 0 { if dx == 0 { 6 } else { 5 } } else { 0 }
                    } else if dx == 0 || dy == 0 {
                        4
                    } else {
                        3
                    };
                    a += weight * a_arr[(i + dy + 1) as usize][(j + dx + 1) as usize];
                    b += weight * b_arr[(i + dy + 1) as usize][(j + dx + 1) as usize];
                }
            }
            let v = a * cdef.get((x + j) as usize, (y + i) as usize) as i64 + b;
            f[i as usize][j as usize] = round2(v, SGRPROJ_SGR_BITS as u32 + shift - SGRPROJ_RST_BITS as u32) as i32;
        }
    }
    Some(f)
}

#[allow(clippy::too_many_arguments)]
fn self_guided(src: &Src, cdef: &Plane, out: &mut Plane, set: usize, xqd: [i16; 2], x: isize, y: isize, w: usize, h: usize, bd: u32) {
    let flt0 = box_filter(src, cdef, x, y, w, h, set, 0, bd);
    let flt1 = box_filter(src, cdef, x, y, w, h, set, 1, bd);
    let w0 = xqd[0] as i64;
    let w1 = xqd[1] as i64;
    let w2 = (1i64 << SGRPROJ_PRJ_BITS) - w0 - w1;
    let maxv = (1i64 << bd) - 1;
    for i in 0..h {
        for j in 0..w {
            let u = (cdef.get((x + j as isize) as usize, (y + i as isize) as usize) as i64) << SGRPROJ_RST_BITS;
            let mut v = w1 * u;
            v += match &flt0 {
                Some(f) => w0 * f[i][j] as i64,
                None => w0 * u,
            };
            v += match &flt1 {
                Some(f) => w2 * f[i][j] as i64,
                None => w2 * u,
            };
            let s = round2(v, SGRPROJ_RST_BITS as u32 + SGRPROJ_PRJ_BITS as u32);
            out.set((x + j as isize) as usize, (y + i as isize) as usize, s.clamp(0, maxv) as u16);
        }
    }
}
