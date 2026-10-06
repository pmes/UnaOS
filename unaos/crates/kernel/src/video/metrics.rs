// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! CHARTER: Kernel — wm
//!
//! UIMETRICS (rmbp-ledger B372, R85 item 11 "the theme scales by DPI") — the ignition check and the witness
//! of `ui::Metrics`, the ONE runtime table every furniture length and every native kernel window is laid out
//! from.
//!
//! * [`ignite`] — called where the console takes the panel's face (`fbcon`'s two arm sites, which latch
//!   `video::dpi`): the boot says `[ui] metrics ppi=… scale=… bar=… title=… frame=… gap=… disc=… chrome=WxH
//!   cell=WxH glyph=k asserts=tests`. The furniture's relations no longer run here (SANITYLEGS, B488 — a stale
//!   one rebooted every flight-26 boot): the constant ones are `const _` asserts, the metric ones `tests sanity`.
//! * [`consts_disagreeing`] — the furniture readers (`theme::*()`, `wm::TITLE_H()`, `strip::PAD()`, the chrome
//!   and grid cells, the bar / dock / crystal cells) that do not answer what `ui::Metrics::panel()` says: 0 is
//!   "no const left behind".
//! * `tests metrics` — `:: UIMETRICS: ppi=<p> scale=<s> bar=<px> title=<px> cell=<w>x<h> magnified=<n> consts=<n> -> PASS ::`.

use core::sync::atomic::{AtomicBool, Ordering};

static IGNITED: AtomicBool = AtomicBool::new(false);

/// The `[ui] metrics` line, once per boot (no checks: R80, SANITYLEGS).
pub fn ignite() {
    if IGNITED.swap(true, Ordering::AcqRel) {
        return;
    }
    let m = crate::ui::Metrics::panel();
    // SANITYLEGS (B488, R80): the relation checks that used to run (and PANIC, i.e. reboot) here are `tests sanity`
    // now — flight 26 looped on one of them. The constant halves are `const _` asserts the compile legs prove.
    serial_println!(
        "[ui] metrics ppi={} scale={} s2={} bar={} title={} frame={} gap={} disc={} chrome={}x{} cell={}x{} glyph={} win={}x{} consts={} asserts=tests",
        m.ppi,
        crate::video::dpi::scale_str(m.s2),
        m.s2,
        m.bar_h,
        m.title_h,
        m.frame,
        m.gap,
        m.ctrl_box,
        m.chrome_cw,
        m.chrome_ch,
        m.grid_cw,
        m.grid_ch,
        m.scale,
        m.win_w,
        m.win_h,
        consts_disagreeing()
    );
}

/// How many furniture readers disagree with `ui::Metrics::panel()` (0 = every one reads the table).
pub fn consts_disagreeing() -> usize {
    use crate::video::{theme, wm};
    let m = crate::ui::Metrics::panel();
    let mut pairs: alloc::vec::Vec<(usize, usize)> = alloc::vec![
        (theme::FRAME(), m.frame),
        (theme::BEVEL(), m.bevel),
        (theme::TITLE_HEIGHT(), m.title_h),
        (theme::CORNER_RADIUS(), m.corner),
        (theme::WIDGET_RADIUS(), m.widget_r),
        (theme::WELL_RADIUS(), m.well_r),
        (theme::SCROLLBAR_WIDTH(), m.scroll_w),
        (theme::BUTTON_HEIGHT(), m.button_h),
        (theme::BUTTON_PAD_X(), m.button_pad),
        (theme::GAP(), m.gap),
        (theme::CONTROL_BOX(), m.ctrl_box),
        (theme::CONTROL_RADIUS(), m.ctrl_r),
        (theme::TEXT_PX(), m.text_px),
        (wm::TITLE_H(), m.title_h),
        (wm::BORDER(), m.frame),
        (wm::TITLE_CELL_W(), m.chrome_cw),
        (wm::TITLE_CELL_H(), m.chrome_ch),
        (crate::video::text::chrome_cell().0, m.chrome_cw),
        (crate::video::text::chrome_cell().1, m.chrome_ch),
        (crate::video::text::grid_cell().0, m.grid_cw),
        (crate::video::text::grid_cell().1, m.grid_ch),
        (crate::video::text::Face::Chrome.cell_h(), m.chrome_ch),
    ];
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    pairs.extend_from_slice(&[
        (crate::video::strip::PAD(), m.gap),
        (crate::video::menubar::BAR_CELL_W(), m.chrome_cw),
        (crate::video::menubar::BAR_CELL_H(), m.chrome_ch),
        (crate::video::crystal::DROP_CELL_W(), m.chrome_cw),
        (crate::video::crystal::DROP_CELL_H(), m.chrome_ch),
        (crate::video::dock::STRIP_H(), m.button_h + 2 * m.gap),
    ]);
    pairs.iter().filter(|(a, b)| a != b).count()
}

/// `tests metrics`: the latched table against the furniture and the live window table.
pub fn fixture() {
    ignite();
    let m = crate::ui::Metrics::panel();
    let consts = consts_disagreeing();
    let (mag, native, live) = crate::video::wm::uimetrics_census();
    let check = m.check();
    let latched = crate::video::dpi::latched();
    let pass = check.is_ok() && consts == 0 && mag == 0 && latched;
    serial_println!(
        "[ui] metrics fixture latched={} check={} native={} live={} chrome={}x{} win={}x{} disc={} gap={} frame={}",
        latched,
        check.err().unwrap_or("ok"),
        native,
        live,
        m.chrome_cw,
        m.chrome_ch,
        m.win_w,
        m.win_h,
        m.ctrl_box,
        m.gap,
        m.frame
    );
    serial_println!(
        ":: UIMETRICS: ppi={} scale={} bar={} title={} cell={}x{} magnified={} consts={} -> {} ::",
        m.ppi,
        crate::video::dpi::scale_str(m.s2),
        m.bar_h,
        m.title_h,
        m.grid_cw,
        m.grid_ch,
        mag,
        consts,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// Register `tests metrics` once (called from `tests::shell_verb`).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("metrics", fixture);
        crate::tests::register("sanity", sanity_fixture); // SANITYLEGS (B488)
    }
}

// ── Native windows: logical layout, physical pixels ──────────────────────────────────────────────────────────
//
// A kernel window's painter keeps its layout in LOGICAL px (96-ppi CSS px — the numbers it always had) and
// paints a surface of PHYSICAL px (`ui::px` of its logical size) that the compositor shows at scale 1. The
// helpers below are the one conversion: a coordinate EDGE is `floor(v x s2 / 2)` (adjacent rects tile with no
// gap or overlap), a size is `ui::px` (rounded up), text is drawn at the edge of its logical origin in the
// dpi-sized faces (`Face::Ui` proportional, `Face::Grid` for the body/mono face, `Face::Chrome`), and a press
// at a physical surface pixel maps back with [`to_logical`]. Nothing is magnified: every pixel is drawn.

/// The latched scale x2 (2 = 1.0).
#[inline]
pub fn s2() -> usize {
    crate::video::dpi::s2() as usize
}

/// A logical coordinate's physical edge.
#[inline]
pub fn edge(v: usize) -> usize {
    v * s2() / 2
}

/// A logical length's physical size (rounded up) — a native surface's extent.
#[inline]
pub fn size(v: usize) -> usize {
    crate::ui::px(v)
}

/// A physical surface coordinate (a press) back to logical px.
#[inline]
pub fn to_logical(p: usize) -> usize {
    p * 2 / s2().max(1)
}

/// The face a native window draws `face` with: the 7x16 body atlas becomes the dpi-sized mono grid face.
#[inline]
pub fn native_face(face: crate::video::text::Face) -> crate::video::text::Face {
    match face {
        crate::video::text::Face::Body => crate::video::text::Face::Grid,
        f => f,
    }
}

/// `face`'s cell height in LOGICAL px (what a logical layout centres a line with).
#[inline]
pub fn lcell_h(face: crate::video::text::Face) -> usize {
    to_logical(native_face(face).cell_h())
}

/// `face`'s cell advance in LOGICAL px.
#[inline]
pub fn lcell_w(face: crate::video::text::Face) -> usize {
    to_logical(native_face(face).cell_w())
}

/// The width `s` takes in `face`, in LOGICAL px (rounded up).
pub fn ladvance(s: &[u8], bold: bool, face: crate::video::text::Face) -> usize {
    let p = crate::video::text::advance(s, bold, native_face(face));
    (p * 2).div_ceil(s2().max(1))
}

/// Fill the logical rect `(x, y, w, h)` of a native surface `px` whose PHYSICAL stride (and width) is `pw`.
pub fn fill(px: &mut [u32], pw: usize, x: usize, y: usize, w: usize, h: usize, c: u32) {
    let (x0, x1, y0, y1) = (edge(x), edge(x + w).min(pw), edge(y), edge(y + h));
    if x0 >= x1 {
        return;
    }
    for yy in y0..y1 {
        let r = yy * pw;
        if r + x1 > px.len() {
            break;
        }
        px[r + x0..r + x1].fill(c);
    }
}

/// Draw `s` with its line top at the logical `(x, y)` on a native surface (`pw` x `ph` physical, stride `pw`),
/// clipped at the LOGICAL width `clip_w` (and the surface); returns the logical pen x after the last glyph.
#[allow(clippy::too_many_arguments)]
pub fn text(px: &mut [u32], pw: usize, ph: usize, clip_w: usize, x: usize, y: usize, s: &[u8], ink: u32, bold: bool, face: crate::video::text::Face) -> usize {
    let pen = crate::video::text::draw_text(px, pw, size(clip_w).min(pw), ph, edge(x), edge(y), s, ink, bold, native_face(face));
    to_logical(pen)
}

/// SECREVIEW F1 (B442): the leaked per-module surface (`dialog`, `notify`, `bezel`, `instgui`, `login`), sized for
/// the LIVE scale. Each module used to leak one buffer of `n` words at first use and then build a slice of the
/// CURRENT `n` over it on every call; `dpi::set_live` (PREFSUI's Resolution dropdown) raises the scale at run time,
/// so the slice ran past the allocation (a heap overwrite by the painter). Word 0 of each allocation holds its
/// capacity; a larger `n` leaks a fresh buffer (an old one stays valid for a reader that still holds it).
pub fn leaked_surf(at: &core::sync::atomic::AtomicUsize, n: usize) -> &'static mut [u32] {
    use core::sync::atomic::Ordering;
    loop {
        let p = at.load(Ordering::Acquire);
        // SAFETY: a nonzero `p` is only ever a leaked `[u32]` of `cap + 1` words whose word 0 is `cap`, written
        // before the pointer is published below; `n <= cap` bounds the slice past the header word.
        if p != 0 && unsafe { *(p as *const u32) } as usize >= n {
            return unsafe { core::slice::from_raw_parts_mut((p as *mut u32).add(1), n) };
        }
        let b: &'static mut [u32] = alloc::boxed::Box::leak(alloc::vec![0u32; n + 1].into_boxed_slice());
        b[0] = n as u32;
        let _ = at.compare_exchange(p, b.as_mut_ptr() as usize, Ordering::AcqRel, Ordering::Acquire);
    }
}

// ── SANITYLEGS (rmbp-ledger B488) — the furniture's layout relations, as a `tests` leg ───────────────────────
//
// Flight 26 (image 19) rebooted on EVERY boot at `winmenu.rs:2269 assertion failed: BAR_BOXES_MAX ==
// MENU_TITLES_MAX + 2`: a runtime assert over constants, run once at boot by `ignite`, that no compile leg ever
// evaluated. Every such assert whose operands are `const` items is now a `const _: () = assert!(…)` in place (the
// compile legs prove it); the ones over the DPI-latched metric readers record into a [`Sane`] here, run by
// `tests sanity` and never at boot (R80). GATE-SANITY (`unaos/scripts/sanity-legs.py`) refuses a new runtime assert
// over consts. Witness: `:: SANITY: const_asserts=<n> runtime_moved=<n> left=<n> -> PASS|FAIL ::`, `left` = the
// moved relations that do NOT hold on this boot's latched metrics.

/// The `const _` sanity asserts SANITYLEGS placed in the furniture files (GATE-SANITY counts them and refuses a
/// mismatch, so this number cannot drift from the tree).
pub const SANITY_CONST_ASSERTS: usize = 17;

/// A relation recorder: `t(cond)` counts a checked relation and names the first that fails by its source line.
pub struct Sane {
    pub n: usize,
    pub bad: usize,
    pub first: Option<&'static core::panic::Location<'static>>,
}

impl Sane {
    pub const fn new() -> Self {
        Sane { n: 0, bad: 0, first: None }
    }
    #[track_caller]
    pub fn t(&mut self, ok: bool) {
        self.n += 1;
        if !ok {
            self.bad += 1;
            if self.first.is_none() {
                self.first = Some(core::panic::Location::caller());
            }
        }
    }
}

/// Every moved relation, on the latched metrics. Returns the recorder.
pub fn sanity_run() -> Sane {
    let mut ck = Sane::new();
    ck.t(crate::ui::Metrics::panel().check().is_ok());
    crate::video::theme::uimetrics_sanity_positive(&mut ck);
    crate::video::theme::uimetrics_sanity_relations(&mut ck);
    crate::video::wm::uimetrics_sanity_title_cell(&mut ck);
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        crate::video::strip::uimetrics_sanity(&mut ck);
        crate::video::dock::uimetrics_sanity(&mut ck);
        crate::video::menubar::uimetrics_sanity(&mut ck);
        crate::video::crystal::uimetrics_sanity(&mut ck);
        crate::video::winmenu::uimetrics_sanity(&mut ck);
    }
    ck
}

/// `tests sanity`.
pub fn sanity_fixture() {
    let ck = sanity_run();
    if let Some(l) = ck.first {
        serial_println!("[sanity] first failing relation at {}:{} (of {} failing)", l.file(), l.line(), ck.bad);
    }
    serial_println!(
        ":: SANITY: const_asserts={} runtime_moved={} left={} -> {} ::",
        SANITY_CONST_ASSERTS,
        ck.n,
        ck.bad,
        if ck.bad == 0 { "PASS" } else { "FAIL" }
    );
}
