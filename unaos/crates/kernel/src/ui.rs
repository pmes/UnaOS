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

//! UI-1 — the UI metrics layer: the ONE place on-screen UI geometry comes from.
//!
//! **THE METRICS RULE (standing directive, UI-1):** *no absolute pixel sizes in UI code.*
//! The scale is `video::dpi`'s (the panel's ppi / 96 at the half pixel — UIMETRICS, B372, retired
//! the old `height / 900` rule), and every
//! UI dimension — glyph cell, line pitch, margins, cursor, meter geometry — derives from
//! [`Metrics`]. UI code never names a pixel count; it asks the metrics. That is what makes
//! the same console/meters read correctly on a 640×480 QEMU panel, the 1280×800 OVMF GUI,
//! and a 2880×1800 Retina panel alike.
//!
//! The one deliberate exception is the pre-heap boot console (`video/fbcon.rs`), which keeps
//! its own unscaled 8-px font: it exists to get *something* legible on screen before the
//! allocator (and thus this layer's consumers) are up, and it is out of the GUI's life cycle.
//!
//! Everything here is integer maths (no float in the kernel) and allocation-free, so the
//! metrics can be derived per call — there is no global to initialise and no state to go
//! stale if a future surface changes mode.

/// The base bitmap font cell in pixels (`font8x8` glyphs are 8×8) — the one place its size
/// is named. Every text metric is `BASE_CELL * scale` derived.
pub const BASE_CELL: usize = 8;

/// The scale cap — beyond 4× legibility gains nothing and glyph blocks get blocky-huge.
pub const SCALE_MAX: usize = 4;

/// UIMETRICS (rmbp-ledger B372, R85 item 11 "the theme scales by DPI") — the theme's BASE lengths: the
/// crispy kit's `metrics.*` read as 96-ppi CSS px. Every furniture length on the glass is one of these
/// times `video::dpi`'s scale (`dpi::px_at`, rounded up to the whole pixel). This table is the ONLY place
/// a furniture pixel count is written; `theme`, `wm`, `menubar`, `dock`, `crystal`, `strip` and `winmenu`
/// read [`Metrics`]. At scale 1.0 every length is the `const` it replaced, byte for byte, EXCEPT the control
/// disc (the seat, B372: the 24 was the kit's 12 at 2x in device px — the dpi scale now does that 2x, so the
/// base is the kit's 12: 30 px at 2.5); the battery and the crystal keep their own 24 so they keep their size.
pub mod base {
    /// `metrics.frame` — frame thickness.
    pub const FRAME: usize = 5;
    /// `metrics.bevel` — bevel thickness.
    pub const BEVEL: usize = 1;
    /// `metrics.title_height` — the title strip, and the menu bar (the same strip, the same job).
    pub const TITLE_HEIGHT: usize = 34;
    /// `metrics.corner_radius` — the two top corners of a window head.
    pub const CORNER_RADIUS: usize = 12;
    /// `metrics.widget_radius` — buttons and other raised controls.
    pub const WIDGET_RADIUS: usize = 8;
    /// `metrics.well_radius` — recessed regions.
    pub const WELL_RADIUS: usize = 15;
    /// `metrics.scrollbar_width`.
    pub const SCROLLBAR_WIDTH: usize = 12;
    /// `metrics.button_height` — also the dock tile's height.
    pub const BUTTON_HEIGHT: usize = 28;
    /// `metrics.button_pad_x`.
    pub const BUTTON_PAD_X: usize = 18;
    /// `metrics.gap` — the standard gap between controls (and the strips' `PAD`).
    pub const GAP: usize = 12;
    /// `metrics.control_box` — the title-bar disc's DIAMETER: the kit's 12 (the seat on B372: Peter's 24 of
    /// 2026-08-09 was the kit's 12 at a 2x panel in device px, and the dpi scale now supplies the 2x — 30 px at 2.5).
    pub const CONTROL_BOX: usize = 12;
    /// The menu bar's BATTERY body width (height is half) — the 24 it had when it was sized off the disc, kept.
    pub const BATTERY_BOX: usize = 24;
    /// The menu bar's CRYSTAL footprint — the 24 it had when it was sized off the disc, kept.
    pub const CRYSTAL_BOX: usize = 24;
    /// `metrics.text_px` — nominal text size.
    pub const TEXT_PX: usize = 15;
    /// The default size of a kernel window's content (Settings' 520x448 logical box).
    pub const WIN_W: usize = 520;
    /// See [`WIN_W`].
    pub const WIN_H: usize = 448;
}

/// Scale-derived UI geometry — UIMETRICS: ONE runtime table, a pure function of `video::dpi`'s scale
/// (x2: `2` = 1.0 … `8` = 4.0) and [`base`]. Build the panel's with [`Metrics::panel`] (latched for the
/// boot), a fixed one with [`Metrics::for_scale`] (fixtures that want scale 1 whatever the panel), or any
/// with the `const fn` [`Metrics::at`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// Integer glyph magnification (each font pixel renders as a `scale`×`scale` block):
    /// `floor(s2 / 2)` clamped to `1..=SCALE_MAX` — 2 at 221 ppi, 1 without an EDID.
    pub scale: usize,
    /// Glyph cell width in pixels (`BASE_CELL * scale`). The text advance per character —
    /// and the cursor is EXACTLY one `cell_w`×`cell_h` cell, by construction.
    pub cell_w: usize,
    /// Glyph cell height in pixels (`BASE_CELL * scale`).
    pub cell_h: usize,
    /// Text row pitch: `cell_h` plus half-cell leading (1.5-line rhythm). The input-line
    /// clear strip is exactly this tall.
    pub line_h: usize,
    /// Page margin — the left/top inset of screen-owning views. One line-height, so the
    /// margin breathes with the type.
    pub margin: usize,
    /// UIMETRICS: the dpi scale x2 every length below is multiplied by.
    pub s2: u32,
    /// The panel's effective ppi (0 = unknown: no EDID, scale 1.0).
    pub ppi: u32,
    /// Window frame thickness.
    pub frame: usize,
    /// Bevel thickness.
    pub bevel: usize,
    /// The title strip above each window's content.
    pub title_h: usize,
    /// The menu bar's height (the title strip's).
    pub bar_h: usize,
    /// Window head corner radius.
    pub corner: usize,
    /// Widget corner radius.
    pub widget_r: usize,
    /// Well corner radius.
    pub well_r: usize,
    /// Scrollbar width.
    pub scroll_w: usize,
    /// Button height (and the dock tile's).
    pub button_h: usize,
    /// Button horizontal padding.
    pub button_pad: usize,
    /// The standard gap (and the strips' `PAD`).
    pub gap: usize,
    /// The title-bar disc's diameter.
    pub ctrl_box: usize,
    /// Its radius.
    pub ctrl_r: usize,
    /// Nominal text size, px.
    pub text_px: usize,
    /// The chrome text cell (captions, the bar, the crystal menu, dock labels): the atlas's chrome cell x scale.
    pub chrome_cw: usize,
    /// See [`Metrics::chrome_cw`].
    pub chrome_ch: usize,
    /// The window text cell (the body atlas cell x scale): the console's grid and a native window's line.
    pub grid_cw: usize,
    /// See [`Metrics::grid_cw`].
    pub grid_ch: usize,
    /// A kernel window's default content size.
    pub win_w: usize,
    /// See [`Metrics::win_w`].
    pub win_h: usize,
}

/// `n` base px at scale x2 `s2`, rounded up to the whole pixel — `video::dpi::px_at`'s rule, restated
/// here as a `const fn` so [`Metrics::at`] stays one.
#[inline]
pub const fn px_at(n: usize, s2: u32) -> usize {
    (n * s2 as usize + 1) / 2
}

impl Metrics {
    /// The metrics at scale x2 `s2` (clamped to 1.0 ..= 4.0) for a panel of `ppi` (0 = unknown).
    pub const fn at(s2: u32, ppi: u32) -> Metrics {
        let s2 = if s2 < 2 { 2 } else if s2 > 8 { 8 } else { s2 };
        let mut scale = (s2 / 2) as usize;
        if scale < 1 {
            scale = 1;
        }
        if scale > SCALE_MAX {
            scale = SCALE_MAX;
        }
        let cell = BASE_CELL * scale;
        let line_h = cell + cell / 2;
        let ctrl_box = px_at(base::CONTROL_BOX, s2);
        let title_h = px_at(base::TITLE_HEIGHT, s2);
        Metrics {
            scale,
            cell_w: cell,
            cell_h: cell,
            line_h,
            margin: line_h,
            s2,
            ppi,
            frame: px_at(base::FRAME, s2),
            bevel: px_at(base::BEVEL, s2),
            title_h,
            bar_h: title_h,
            corner: px_at(base::CORNER_RADIUS, s2),
            widget_r: px_at(base::WIDGET_RADIUS, s2),
            well_r: px_at(base::WELL_RADIUS, s2),
            scroll_w: px_at(base::SCROLLBAR_WIDTH, s2),
            button_h: px_at(base::BUTTON_HEIGHT, s2),
            button_pad: px_at(base::BUTTON_PAD_X, s2),
            gap: px_at(base::GAP, s2),
            ctrl_box,
            ctrl_r: ctrl_box / 2,
            text_px: px_at(base::TEXT_PX, s2),
            chrome_cw: px_at(crate::video::font::CHROME_CELL_W, s2),
            chrome_ch: px_at(crate::video::font::CHROME_CELL_H, s2),
            grid_cw: px_at(crate::video::font::CELL_W, s2),
            grid_ch: px_at(crate::video::font::CELL_H, s2),
            win_w: px_at(base::WIN_W, s2),
            win_h: px_at(base::WIN_H, s2),
        }
    }

    /// A fixed integer glyph scale whatever the panel (fixtures, and the window surfaces the compositor
    /// still magnifies): [`Metrics::at`] with `s2 = 2 * scale`.
    pub const fn for_scale(scale: usize) -> Metrics {
        Metrics::at((scale * 2) as u32, 0)
    }

    /// THE panel's metrics: [`Metrics::at`] the scale `video::dpi` latched for this boot (latching it on
    /// the first call that finds a panel). Two atomic loads once latched.
    pub fn panel() -> Metrics {
        let (s2, ppi) = crate::video::dpi::metrics_scale();
        Metrics::at(s2, ppi)
    }

    /// Width of `n` characters of text, in pixels (the advance is one cell per char).
    #[inline]
    pub const fn text_w(&self, n: usize) -> usize {
        n * self.cell_w
    }

    /// A base (96-ppi) length at this scale.
    #[inline]
    pub const fn px(&self, n: usize) -> usize {
        px_at(n, self.s2)
    }

    /// The theme's relations, checked on these metrics — what used to be `theme.rs`'s `const _`
    /// assertions. `Err(name)` names the first relation that fails.
    pub const fn check(&self) -> Result<(), &'static str> {
        if self.frame == 0 || self.bevel == 0 || self.title_h == 0 || self.scroll_w == 0 || self.button_h == 0 || self.button_pad == 0 || self.gap == 0 || self.ctrl_box == 0 || self.text_px == 0 {
            return Err("positive");
        }
        if self.bevel >= self.frame {
            return Err("bevel<frame");
        }
        if self.corner >= self.title_h {
            return Err("corner<title");
        }
        if self.ctrl_box >= self.title_h {
            return Err("disc<title");
        }
        if self.title_h < self.ctrl_box + 2 * self.bevel {
            return Err("disc-clearance");
        }
        if self.ctrl_r == 0 {
            return Err("disc-radius");
        }
        if 2 * self.widget_r > self.button_h {
            return Err("widget<button");
        }
        if self.well_r >= self.title_h {
            return Err("well<title");
        }
        if self.chrome_ch > self.title_h || self.chrome_ch > self.bar_h {
            return Err("chrome-cell<strip");
        }
        Ok(())
    }

}

/// UIMETRICS: a base (96-ppi) length at the panel's scale — what every furniture accessor and a native
/// kernel window lays out with. One relaxed load and a multiply once the scale is latched.
#[inline]
pub fn px(n: usize) -> usize {
    px_at(n, crate::video::dpi::s2())
}

/// UIMETRICS: the theme's relations hold at EVERY scale 1.0 ..= 4.0 — proven at compile time, so the
/// runtime assert at ignition (`video::metrics::ignite`) can only fail on a table edit this proof
/// would already have refused.
const _: () = {
    let mut s2 = 2u32;
    while s2 <= 8 {
        assert!(Metrics::at(s2, 0).check().is_ok(), "ui::Metrics relation fails at some scale");
        s2 += 1;
    }
};
