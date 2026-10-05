//! §7.16 the upscaling process (super-resolution): each row of the decoded (downscaled) frame is
//! resampled horizontally to UpscaledWidth with the 8-tap Upscale_Filter at 1/64-sample phases,
//! stepping in units of 1/16384 sample from an initial offset that centres the error.

use crate::decode::{round2, FrameState, Plane};
use crate::obu::FrameHeader;
use crate::tables::*;

/// Upscale `input` (CurrFrame or CdefFrame) to UpscaledWidth. Returns the input when
/// use_superres is 0.
pub fn upscale(fs: &FrameState, h: &FrameHeader, input: &[Plane; 3]) -> [Plane; 3] {
    if !h.use_superres {
        return [input[0].clone(), input[1].clone(), input[2].clone()];
    }
    let maxv = (1i64 << fs.bit_depth) - 1;
    let mk = |plane: usize| -> Plane {
        let (sub_x, _) = if plane == 0 { (0, 0) } else { (fs.ss_x, fs.ss_y) };
        Plane::new(((h.upscaled_width as usize + sub_x) >> sub_x) + 160, input[plane].rows)
    };
    let mut out = [mk(0), mk(1), mk(2)];
    for plane in 0..fs.num_planes {
        let (sub_x, sub_y) = if plane == 0 { (0u32, 0u32) } else { (fs.ss_x as u32, fs.ss_y as u32) };
        let downscaled_plane_w = round2(h.frame_width as i64, sub_x);
        let upscaled_plane_w = round2(h.upscaled_width as i64, sub_x);
        let plane_h = round2(h.frame_height as i64, sub_y);
        let sb = SUPERRES_SCALE_BITS as u32;
        let step_x = ((downscaled_plane_w << sb) + (upscaled_plane_w / 2)) / upscaled_plane_w;
        let err = upscaled_plane_w * step_x - (downscaled_plane_w << sb);
        let mut initial_subpel_x = (-((upscaled_plane_w - downscaled_plane_w) << (sb - 1)) + upscaled_plane_w / 2) / upscaled_plane_w
            + (1 << (SUPERRES_EXTRA_BITS - 1))
            - err / 2;
        initial_subpel_x &= SUPERRES_SCALE_MASK as i64;
        let mi_w = (fs.mi_cols >> sub_x) as i64;
        let min_x = 0i64;
        let max_x = mi_w * MI_SIZE as i64 - 1;
        let src = &input[plane];
        let dst = &mut out[plane];
        for y in 0..plane_h as usize {
            for x in 0..upscaled_plane_w {
                let src_x = -(1i64 << sb) + initial_subpel_x + x * step_x;
                let src_x_px = src_x >> sb;
                let src_x_subpel = ((src_x & SUPERRES_SCALE_MASK as i64) >> SUPERRES_EXTRA_BITS) as usize;
                let mut sum = 0i64;
                for k in 0..SUPERRES_FILTER_TAPS {
                    let sample_x = (src_x_px + k as i64 - SUPERRES_FILTER_OFFSET as i64).clamp(min_x, max_x);
                    sum += src.get(sample_x as usize, y) as i64 * UPSCALE_FILTER[src_x_subpel][k] as i64;
                }
                dst.set(x as usize, y, round2(sum, FILTER_BITS as u32).clamp(0, maxv) as u16);
            }
        }
    }
    out
}
