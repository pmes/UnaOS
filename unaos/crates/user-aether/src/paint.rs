// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL (SR66): painting (CSS2 Appendix E, the subset M1 draws) into a back buffer the size of the window,
// then onto the compositor surface. Order per block box, in tree order: background (border box), borders (solid),
// the list marker, the line boxes' runs (an inline element's own background first, the text, its underline), then
// the children. The canvas takes the root's background, or the body's when the root has none (CSS2 §14.2). The
// whole page is translated up by the scroll offset and clipped to the window; boxes and lines wholly outside the
// window are skipped.

use alloc::vec::Vec;

use crate::layout::Laid;
use crate::style::Styled;
use crate::txt::Txt;

pub struct Canvas {
    pub px: Vec<u32>,
    pub w: i32,
    pub h: i32,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Canvas {
        Canvas { px: alloc::vec![0; (w * h) as usize], w, h }
    }

    pub fn fill(&mut self, x0: i32, y0: i32, w: i32, h: i32, c: u32) {
        let (xs, ys) = (x0.max(0), y0.max(0));
        let (xe, ye) = ((x0 + w).min(self.w), (y0 + h).min(self.h));
        for y in ys..ye {
            let row = (y * self.w) as usize;
            for x in xs..xe {
                self.px[row + x as usize] = c;
            }
        }
    }

    fn blend(&mut self, x: i32, y: i32, ink: u32, a: u8) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let i = (y * self.w + x) as usize;
            self.px[i] = font_core::ui::blend(self.px[i], ink, a);
        }
    }

    /// How many pixels differ from `bg` — the witness's `painted=`.
    pub fn count_not(&self, bg: u32) -> usize {
        self.px.iter().filter(|&&p| p != bg).count()
    }
}

fn fl(v: f32) -> i32 {
    font_core::fmath::floor(v) as i32
}

/// Paint the page with its top at `-scroll`; `bg` is the canvas colour.
pub fn paint(cv: &mut Canvas, root: &Laid, scroll: f32, bg: u32, sty: &Styled, txt: &mut Txt) {
    cv.fill(0, 0, cv.w, cv.h, bg);
    paint_box(cv, root, 0.0, -scroll, sty, txt);
}

fn paint_box(cv: &mut Canvas, l: &Laid, ox: f32, oy: f32, sty: &Styled, txt: &mut Txt) {
    let x = ox + l.x;
    let y = oy + l.y;
    let vh = cv.h as f32;
    if y > vh || y + l.h < 0.0 && l.kids.is_empty() && l.lines.is_empty() {
        return;
    }
    let s = &sty.styles[l.st];
    if !l.anon {
        if y + l.h >= 0.0 && y <= vh {
            if let Some(c) = s.bg {
                cv.fill(fl(x), fl(y), fl(x + l.w) - fl(x), fl(y + l.h) - fl(y), c);
            }
            let (bt, br, bb, bl) = (s.bw(0), s.bw(1), s.bw(2), s.bw(3));
            let (xi, yi, wi, hi) = (fl(x), fl(y), fl(x + l.w) - fl(x), fl(y + l.h) - fl(y));
            if bt > 0.0 {
                cv.fill(xi, yi, wi, bt as i32, s.bc(0));
            }
            if bb > 0.0 {
                cv.fill(xi, yi + hi - bb as i32, wi, bb as i32, s.bc(2));
            }
            if bl > 0.0 {
                cv.fill(xi, yi, bl as i32, hi, s.bc(3));
            }
            if br > 0.0 {
                cv.fill(xi + wi - br as i32, yi, br as i32, hi, s.bc(1));
            }
        }
        if let (Some(m), Some(fb)) = (&l.marker, l.first_base) {
            let base = y + fb;
            if base > -40.0 && base < vh + 40.0 {
                let mw = txt.measure(m, s);
                let gap = s.font_size * 0.5;
                let mx = x + l.cx - gap - mw;
                txt.draw(mx, base, m, s, &mut |gx, gy, a| cv.blend(gx, gy, s.color & 0x00FF_FFFF, a));
            }
        }
    }
    for line in &l.lines {
        let top = y + line.y;
        if top > vh || top + line.h < 0.0 {
            continue;
        }
        let base = font_core::fmath::round(top + line.base);
        for f in &line.frags {
            let fs = &sty.styles[f.st];
            let fx = x + l.cx + f.x;
            if f.own {
                if let Some(c) = fs.bg {
                    cv.fill(fl(fx), fl(top), fl(fx + f.w) - fl(fx) + 1, fl(top + line.h) - fl(top), c);
                }
            }
            let ink = fs.color & 0x00FF_FFFF;
            txt.draw(fx, base, &f.s, fs, &mut |gx, gy, a| cv.blend(gx, gy, ink, a));
            if fs.underline {
                let uy = base as i32 + 1 + (fs.font_size / 14.0) as i32;
                let th = if fs.font_size >= 20.0 { 2 } else { 1 };
                cv.fill(fl(fx), uy, fl(fx + f.w) - fl(fx), th, ink);
            }
        }
    }
    for k in &l.kids {
        paint_box(cv, k, x, y, sty, txt);
    }
}
