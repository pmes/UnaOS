// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (SOFTY B629: the Softy window chrome — the head, the frame, the three caption buttons at the right)
//!
//! Under Softy the compositor still lays every window out on the SAME geometry (`wm::outer_box`, `BORDER`,
//! `TITLE_H` — the tiler, the resize zones and the drag are untouched); this module only re-skins it and answers
//! where the caption buttons are. Two entries, each reached by ONE line-neutral hook in `wm.rs`:
//!
//! * [`paint`] — from `wm::paint_window`, after the stock chrome block and before the content: the frame and head
//!   are repainted in Softy's tone (a light head, a 1 px border in the accent on the focused window, 8 px top
//!   corners, the app icon and the caption at the left, minimise / maximise / close as three flat buttons at the
//!   RIGHT, the close button red under the pointer). The content paints over its own rect afterwards, as before.
//! * [`ctrl_at`] — from `wm::control_hit` and `wm::title_bar_hit`: the button under a panel point. Since every
//!   router (close, minimise, zoom, the title drag's exclusion) asks `control_hit`, the drawn button and the
//!   pressed button are one rectangle by construction — the WMCTRL rule, kept.
//!
//! Hover: [`hover_track`] (from the taskbar's per-pass compose) reads the pointer, asks `wm::control_hit`, and on a
//! change damages the two heads involved; [`paint`] reads the latch. No pressed state yet (SOFTY.md, owed).

use core::sync::atomic::{AtomicU64, Ordering};

use super::paint::{self as p, Surf};
use super::palette as k;
use crate::video::wm::{self, Ctrl, Window};

/// The button the pointer is over: `win << 8 | (slot + 1)`, 0 = none.
static HOVER: AtomicU64 = AtomicU64::new(0);

/// One composed head row (the widest box the panel can hold); try-locked, never waited on (the compositor holds
/// the window table with interrupts masked while it paints).
static ROW: [crate::sync::Mutex<[u32; crate::video::strip::MAX_STRIP_W]>; 2] =
    [const { crate::sync::Mutex::new([0u32; crate::video::strip::MAX_STRIP_W]) }; 2];

/// The head's corner radius (`wm::outside_top_corner` cuts the same arc under Softy, so a press there is the desktop's).
#[allow(non_snake_case)]
#[inline]
pub fn RADIUS() -> usize {
    k::px(k::WIN_R)
}

/// The head: the top border plus the title strip (the band the buttons fill top to bottom).
#[inline]
fn head_h() -> usize {
    wm::BORDER() + wm::TITLE_H()
}

/// The three buttons, left to right, in `wm`'s vocabulary.
const BTNS: [Ctrl; 3] = [Ctrl::Minimise, Ctrl::Zoom, Ctrl::Close];

/// Box-local x of the first button (the three end 1 px inside the right border).
#[inline]
fn btn_x0(bw: usize) -> usize {
    bw.saturating_sub(1 + 3 * k::px(k::BTN_W))
}

/// **The caption button under panel point `(px, py)`** on row `r`, or `None`. The caller has already established
/// that the row draws controls (`wm::controls`).
pub fn ctrl_at(r: &Window, px: usize, py: usize) -> Option<Ctrl> {
    let (bx, by, bw, _bh) = wm::outer_box(r);
    let x0 = bx + btn_x0(bw);
    let bwid = k::px(k::BTN_W);
    if py <= by || py >= by + head_h() || px < x0 || px >= x0 + 3 * bwid {
        return None;
    }
    Some(BTNS[((px - x0) / bwid).min(2)])
}

fn slot_of(c: Ctrl) -> usize {
    match c {
        Ctrl::Minimise => 0,
        Ctrl::Zoom => 1,
        Ctrl::Close => 2,
    }
}

/// The hovered slot on window `id`, if any.
fn hover_on(id: wm::WinId) -> Option<usize> {
    let h = HOVER.load(Ordering::Relaxed);
    if h != 0 && (h >> 8) as wm::WinId == id { Some(((h & 0xFF) - 1) as usize) } else { None }
}

/// The registrar key an icon is looked up by: the caption's first word, lower-cased (`appres`'s own rule).
fn icon_key(title: &[u8], buf: &mut [u8; 16]) -> usize {
    let end = title.iter().position(|&c| c == b' ' || c == b':' || c == 0).unwrap_or(title.len()).min(16);
    for (d, s) in buf.iter_mut().zip(title[..end].iter()) {
        *d = s.to_ascii_lowercase();
    }
    end
}

/// A small monogram tile for an app the registrar draws no icon for: a rounded square in a calm hue with a white
/// title-bar stroke — a window, drawn.
pub fn monogram(s: &mut Surf, x: usize, y: usize, size: usize, hue: u32) {
    p::rrect(s, (x, y, size, size), size / 4, hue, 255);
    let inset = (size / 5).max(1);
    p::hairline_h(s, x + inset, y + inset + size / 8, size.saturating_sub(2 * inset), k::ON_ACCENT, 230);
}

/// A hue for a title the table names no colour for (FNV over its bytes).
pub fn hue_of(title: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for &b in title {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    k::APP_HUES[(h as usize) % k::APP_HUES.len()]
}

/// Compose box-local row `j` of the frame into `buf[..bw]`.
#[allow(clippy::too_many_arguments)]
fn compose_row(buf: &mut [u32], bw: usize, bh: usize, j: usize, id: wm::WinId, focused: bool, ctrls: bool, title: &[u8]) {
    let face = if focused { k::SURFACE } else { k::SURFACE_IDLE };
    let edge = if focused { k::ACCENT } else { k::EDGE_IDLE };
    let mut s = Surf::row(buf, bw, j);
    p::fill(&mut s, (0, j, bw, 1), face, 255);
    let head = head_h();
    let (b, t) = (wm::BORDER(), wm::TITLE_H());
    if j < head {
        if ctrls {
            let bwid = k::px(k::BTN_W);
            let x0 = btn_x0(bw);
            let g = k::px(k::GLYPH);
            let gt = k::px(k::GLYPH_T);
            let gy = (head + 1) / 2 - g / 2;
            let hov = hover_on(id);
            for (i, c) in BTNS.iter().enumerate() {
                let x = x0 + i * bwid;
                let hot = hov == Some(i);
                let close = *c == Ctrl::Close;
                if hot && close {
                    p::fill(&mut s, (x, 1, bwid, head - 1), k::CLOSE_HOT, 255);
                } else if hot {
                    p::fill(&mut s, (x, 1, bwid, head - 1), k::PRESS, k::BTN_HOVER_A);
                }
                let ink = if hot && close { k::CLOSE_INK } else if focused { k::TEXT } else { k::TEXT_IDLE };
                let gx = x + (bwid - g) / 2;
                match c {
                    Ctrl::Minimise => p::hairline_h(&mut s, gx, gy + g / 2, g, ink, 255),
                    Ctrl::Zoom => p::stroke_rrect(&mut s, (gx, gy, g, g), k::px(2), ink, 255),
                    Ctrl::Close => {
                        p::line(&mut s, gx, gy, gx + g - 1, gy + g - 1, gt, ink, 255);
                        p::line(&mut s, gx + g - 1, gy, gx, gy + g - 1, gt, ink, 255);
                    }
                }
            }
        }
        // The app's icon and the caption, at the left, centred in the title strip.
        let isz = k::px(k::WIN_ICON);
        let (ix, iy) = (b + k::px(8), b + t.saturating_sub(isz) / 2);
        let mut kb = [0u8; 16];
        let kl = icon_key(title, &mut kb);
        let key = core::str::from_utf8(&kb[..kl]).unwrap_or("");
        if key.is_empty() || !p::app_icon(&mut s, key, ix, iy, isz) {
            monogram(&mut s, ix, iy, isz, hue_of(title));
        }
        let fc = crate::video::text::Face::Chrome;
        let tx = ix + isz + k::px(k::WIN_ICON_GAP);
        let limit = if ctrls { btn_x0(bw) } else { bw.saturating_sub(b) };
        let ty = b + t.saturating_sub(fc.cell_h()) / 2;
        p::text(&mut s, tx, ty, limit.saturating_sub(tx + k::px(8)), title, if focused { k::TEXT } else { k::TEXT_IDLE }, false, fc);
    }
    // The 1 px border: rounded at the top (the stroke runs past the bottom so its lower arcs never land), square at
    // the bottom; then the corners outside the arc are the desktop's.
    let rad = RADIUS();
    p::stroke_rrect(&mut s, (0, 0, bw, bh + rad), rad, edge, 255);
    if j + 1 == bh {
        p::hairline_h(&mut s, 0, j, bw, edge, 255);
    }
    p::cut_top_corners(&mut s, (0, 0, bw, bh), rad, k::DESKTOP);
}

/// **Repaint row `r`'s frame and head in Softy's tone** into `dst` (a band whose origin is panel `(ox, oy)`).
/// `ctrls` = the row draws a control cluster (`wm::controls`); `title` = the kernel's copy of the caption.
pub fn paint(dst: &crate::video::FrameBuffer, r: &Window, ox: usize, oy: usize, focused: bool, ctrls: bool, title: &[u8]) {
    let (bx, by, bw, bh) = wm::outer_box(r);
    if bw == 0 || bh == 0 || bw > crate::video::strip::MAX_STRIP_W {
        return;
    }
    let Some(mut g) = ROW[0].try_lock().or_else(|| ROW[1].try_lock()) else { return };
    let (dw, dh) = (dst.info().width, dst.info().height);
    let head = head_h();
    let b = wm::BORDER();
    for j in 0..bh {
        let py = by + j;
        if py < oy {
            continue;
        }
        let ly = py - oy;
        if ly >= dh {
            break;
        }
        let put = |i: usize, c: u32| {
            let x = bx + i;
            if x >= ox && x - ox < dw {
                dst.put_pixel(x - ox, ly, c);
            }
        };
        if j >= head && j + b < bh {
            // A content row: only the two side frames are chrome — the border pixel, then the face.
            let (face, edge) = if focused { (k::SURFACE, k::ACCENT) } else { (k::SURFACE_IDLE, k::EDGE_IDLE) };
            for i in 0..b.min(bw) {
                let c = if i == 0 { edge } else { face };
                put(i, c);
                put(bw - 1 - i, c);
            }
            continue;
        }
        let buf = &mut g[..bw];
        compose_row(buf, bw, bh, j, r.id, focused, ctrls, title);
        for i in 0..bw {
            put(i, buf[i]);
        }
    }
}

/// The pointer's caption button, latched for [`paint`]; a change damages the heads it left and entered. Called
/// once per composite pass from the taskbar (Softy only). `(cx, cy)` is the pointer on the panel.
pub fn hover_track(cx: i32, cy: i32) {
    let now = match wm::hit_test(cx, cy) {
        Some((win, _, _)) => match wm::control_hit(win, cx, cy) {
            Some(c) => (win as u64) << 8 | (slot_of(c) as u64 + 1),
            None => 0,
        },
        None => 0,
    };
    let was = HOVER.swap(now, Ordering::Relaxed);
    if was != now {
        damage_head((was >> 8) as wm::WinId);
        damage_head((now >> 8) as wm::WinId);
    }
}

fn damage_head(id: wm::WinId) {
    if id == 0 {
        return;
    }
    if let Some(i) = wm::info(id) {
        let (b, t) = (wm::BORDER(), wm::TITLE_H());
        let _ = wm::damage_intersecting(i.x.saturating_sub(b), i.y.saturating_sub(t + b), i.w * i.scale + 2 * b, t + b);
    }
}

/// Forget the hover (a look switch).
pub fn hover_clear() {
    HOVER.store(0, Ordering::Relaxed);
}
