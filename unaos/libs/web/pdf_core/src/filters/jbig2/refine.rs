// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. JBIG2 generic refinement (T.88 6.3: templates 0 and 1, the AT pixels, TPGRON) and the
//! refinement region segment (7.4.7) — JBIG2HUFF SR336. Decodes through the one MQ coder.

use alloc::vec;

use super::super::mq::MqDecoder;
use super::{byte, region_info, Bitmap};
use crate::Error;

/// The refinement decoding parameters (6.3.2): template, TPGRON and the AT pixels (RA1 on the bitmap being decoded,
/// RA2 on the reference; template 0 only).
#[derive(Clone, Copy)]
pub(super) struct Refine {
    pub(super) template: u8,
    pub(super) tpgron: bool,
    pub(super) at: [(i32, i32); 2],
}

/// The refinement contexts (13 bits for template 0).
pub(super) const CONTEXTS: usize = 1 << 13;

/// 6.3.5.6: a `w` x `h` bitmap refining `r`, the reference pixel of (x, y) at (x - dx, y - dy). The contexts in the bit
/// order of 6.3.5.3 Figures 12-13; TPGRON's pseudo-pixel context is the reference's centre pixel alone.
#[allow(clippy::too_many_arguments)]
pub(super) fn refine(
    dec: &mut MqDecoder,
    cx: &mut [u8],
    w: usize,
    h: usize,
    r: &Bitmap,
    dx: i64,
    dy: i64,
    g: &Refine,
) -> Result<Bitmap, Error> {
    let mut bm = Bitmap::new(w, h, 0)?;
    let sltp = if g.template == 0 { 0x100 } else { 0x080 };
    let a1 = (g.at[0].0 as isize, g.at[0].1 as isize);
    let a2 = (g.at[1].0 as isize, g.at[1].1 as isize);
    let mut ltp = 0u8;
    for y in 0..h {
        if g.tpgron {
            ltp ^= dec.decode(&mut cx[sltp]);
        }
        let yi = y as isize;
        let ry = yi - dy as isize;
        for x in 0..w {
            let xi = x as isize;
            let rx = xi - dx as isize;
            let q = |ox: isize, oy: isize| r.get(rx + ox, ry + oy);
            if ltp == 1 {
                let v = q(0, 0);
                if (-1..=1).all(|oy| (-1..=1).all(|ox| q(ox, oy) == v)) {
                    bm.data[y * w + x] = v as u8;
                    continue;
                }
            }
            let p = |ox: isize, oy: isize| bm.get(xi + ox, yi + oy);
            let c = if g.template == 0 {
                p(-1, 0)
                    | p(1, -1) << 1
                    | p(0, -1) << 2
                    | p(a1.0, a1.1) << 3
                    | q(1, 1) << 4
                    | q(0, 1) << 5
                    | q(-1, 1) << 6
                    | q(1, 0) << 7
                    | q(0, 0) << 8
                    | q(-1, 0) << 9
                    | q(1, -1) << 10
                    | q(0, -1) << 11
                    | q(a2.0, a2.1) << 12
            } else {
                p(-1, 0)
                    | p(1, -1) << 1
                    | p(0, -1) << 2
                    | p(-1, -1) << 3
                    | q(1, 1) << 4
                    | q(0, 1) << 5
                    | q(1, 0) << 6
                    | q(0, 0) << 7
                    | q(-1, 0) << 8
                    | q(0, -1) << 9
            };
            bm.data[y * w + x] = dec.decode(&mut cx[c as usize]);
        }
    }
    Ok(bm)
}

/// The refinement AT bytes (RA1x, RA1y, RA2x, RA2y) at `i`.
pub(super) fn at_pixels(b: &[u8], i: usize) -> Result<[(i32, i32); 2], Error> {
    let s = |k: usize| -> Result<i32, Error> { Ok(byte(b, i + k)? as i8 as i32) };
    Ok([(s(0)?, s(1)?), (s(2)?, s(3)?)])
}

/// 7.4.7: a refinement region segment's bitmap, refining `reference` (the referred intermediate region, or the page's
/// area under the region) with GRREFERENCEDX = GRREFERENCEDY = 0.
pub(super) fn region(b: &[u8], reference: &Bitmap) -> Result<Bitmap, Error> {
    let (w, h, ..) = region_info(b)?;
    let f = byte(b, 17)?;
    let template = f & 1;
    let (at, i) = if template == 0 { (at_pixels(b, 18)?, 22) } else { ([(0, 0); 2], 18) };
    let mut dec = MqDecoder::new(b.get(i..).unwrap_or(&[]));
    let mut cx = vec![0u8; CONTEXTS];
    refine(&mut dec, &mut cx, w, h, reference, 0, 0, &Refine { template, tpgron: f & 2 != 0, at })
}
