// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Sample adaptive offset (8.7.3), per CTB and component, from a copy of the deblocked picture: band offset (the four
//! consecutive bands from sao_band_position) and edge offset (the four SaoEoClass directions; a sample whose neighbour
//! lies outside the picture, across a slice boundary the loop filters may not cross, or across a tile boundary when
//! loop_filter_across_tiles_enabled_flag is 0, is left as it is). Applied where the CTB's slice has slice_sao_luma_flag /
//! slice_sao_chroma_flag; pcm (with pcm_loop_filter_disabled_flag) and transquant bypass samples are left as they are.

use crate::picture::{Picture, F_BYPASS, F_PCM, NO_SLICE};
use crate::ps::{Geometry, Pps, Sps};

/// hPos / vPos of the two neighbours for SaoEoClass 0..3 (Table 8-13 of the 2013 numbering).
const EO_POS: [[(i32, i32); 2]; 4] = [[(-1, 0), (1, 0)], [(0, -1), (0, 1)], [(-1, -1), (1, 1)], [(1, -1), (-1, 1)]];

pub fn sao_picture(pic: &mut Picture, geo: &Geometry, sps: &Sps, pps: &Pps) {
    if !sps.sample_adaptive_offset_enabled_flag {
        return;
    }
    let nctb = pic.sao.len();
    let any = (0..nctb).any(|i| pic.ctb_slice[i] != NO_SLICE && pic.sao[i].type_idx.iter().any(|&t| t != 0));
    if !any {
        return;
    }
    let src = [pic.planes[0].clone(), pic.planes[1].clone(), pic.planes[2].clone()];
    let pcm_lf_off = sps.pcm.is_some_and(|p| p.loop_filter_disabled);
    let log2 = geo.log2_ctb;
    for rs in 0..nctb {
        let s = pic.ctb_slice[rs];
        if s == NO_SLICE {
            continue;
        }
        let info = pic.slices[s as usize];
        let params = pic.sao[rs];
        let (rx, ry) = (rs % geo.width_ctbs as usize, rs / geo.width_ctbs as usize);
        for c in 0..3 {
            let ty = params.type_idx[c];
            if ty == 0 || (c == 0 && !info.sao_luma) || (c > 0 && !info.sao_chroma) {
                continue;
            }
            let scale = if c == 0 { 1 } else { 2 };
            let bd = if c == 0 { sps.bit_depth_luma } else { sps.bit_depth_chroma };
            let max = (1i32 << bd) - 1;
            let pl = &src[c];
            let size = (1usize << log2) / scale;
            let (x0, y0) = (rx * size, ry * size);
            let mut band_table = [0usize; 32];
            for k in 0..4 {
                band_table[(k + params.band_position[c] as usize) & 31] = k + 1;
            }
            let off = params.offset_val[c];
            for y in y0..(y0 + size).min(pl.h) {
                for x in x0..(x0 + size).min(pl.w) {
                    let (xl, yl) = (x * scale, y * scale);
                    let fi = pic.flags[pic.idx4(xl, yl)];
                    if (fi & F_PCM != 0 && pcm_lf_off) || fi & F_BYPASS != 0 {
                        continue;
                    }
                    let cur = pl.at(x, y) as i32;
                    let idx = if ty == 1 {
                        band_table[(cur >> (bd - 5)) as usize]
                    } else {
                        let mut sum = 0i32;
                        let mut skip = false;
                        for &(hx, vy) in &EO_POS[params.eo_class[c] as usize] {
                            let (xn, yn) = (x as i32 + hx, y as i32 + vy);
                            if xn < 0 || yn < 0 || xn as usize >= pl.w || yn as usize >= pl.h {
                                skip = true;
                                break;
                            }
                            let (xnl, ynl) = (xn as usize * scale, yn as usize * scale);
                            let cn = geo.ctb_rs(xnl as u32, ynl as u32);
                            let sn = pic.ctb_slice[cn as usize];
                            if sn == NO_SLICE {
                                skip = true;
                                break;
                            }
                            let ninfo = pic.slices[sn as usize];
                            if ninfo.addr_rs != info.addr_rs {
                                let zn = geo.zs[(ynl >> 2) * geo.w4 + (xnl >> 2)];
                                let zc = geo.zs[(yl >> 2) * geo.w4 + (xl >> 2)];
                                if (zn < zc && !info.loop_filter_across_slices) || (zc < zn && !ninfo.loop_filter_across_slices) {
                                    skip = true;
                                    break;
                                }
                            }
                            if !pps.loop_filter_across_tiles_enabled_flag && geo.tile_of_rs(cn) != geo.tile_of_rs(rs as u32) {
                                skip = true;
                                break;
                            }
                            sum += (cur - pl.at(xn as usize, yn as usize) as i32).signum();
                        }
                        if skip {
                            continue;
                        }
                        let e = (2 + sum) as usize;
                        if e <= 2 {
                            if e == 2 { 0 } else { e + 1 }
                        } else {
                            e
                        }
                    };
                    if idx == 0 {
                        continue;
                    }
                    let v = (cur + off[idx] as i32).clamp(0, max);
                    pic.planes[c].set(x, y, v as u16);
                }
            }
        }
    }
}
