// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `facet-view` — the viewer's brain, platform-free: input in, Facet requests out, frames back.
//!
//! The vessel is a CLIENT of the Images handler: it never decodes, edits or scales a pixel itself.
//! [`Viewer`] turns window input (keys as `BrowserText`/`BrowserKey`, the wheel as `BrowserScroll`,
//! size as `BrowserResize` — what quartzite's image surface emits) into [`FacetCommand`]s, and
//! turns Facet's `ImageRendered` answers into the `SurfaceBlit` the surface draws. The same
//! controller runs under the macOS window and under the headless witness (`--keys ... --out`).
//!
//! Shortcuts (see [`SHORTCUTS`]): view — `+`/`=` zoom in, `-` zoom out, `0` fit, `1` 100 %,
//! `r`/`R` turn the view clockwise/counter-clockwise, `h`/`v` flip the view, arrows pan; edits —
//! `]`/`[` rotate the picture, `m` mirror it, `x` crop to what is visible, `b`/`B` darker/brighter,
//! `c`/`C` less/more contrast, `u`/`U` undo/redo, `!` reset, `e` export PNG beside the file.

use bandy::signals::{FacetCommand, FacetEdit, FacetFormat, FacetImageInfo, FacetView, FacetZoom};
use bandy::{Origin, SMessage};

/// The surface id the vessel blits to (quartzite's `bootstrap_image_surface` filters on it).
pub const SURFACE: &str = "facet://view";

/// The shortcut table, for the README and the window's help.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("+ / =", "zoom in (x1.25)"),
    ("-", "zoom out (/1.25)"),
    ("0", "fit (shrink to the window, never enlarge)"),
    ("1", "actual size (100 %)"),
    ("r / R", "turn the view 90 degrees clockwise / counter-clockwise"),
    ("h / v", "flip the view left-right / top-bottom"),
    ("arrows", "pan 32 px"),
    ("] / [", "edit: rotate the picture clockwise / counter-clockwise"),
    ("m", "edit: mirror the picture left-right"),
    ("x", "edit: crop to the visible region"),
    ("b / B", "edit: brightness 0.9x / 1.1x"),
    ("c / C", "edit: contrast 0.9x / 1.1x"),
    ("u / U", "undo / redo"),
    ("!", "reset every edit (undoable)"),
    ("e", "export the edited picture as <name>.facet.png beside it"),
];

/// Receipts the vessel stamps: `'V'` in the top byte, a counter below.
const RECEIPT_TAG: u64 = (b'V' as u64) << 56;

/// The viewer's state and its request/answer bookkeeping.
pub struct Viewer {
    pub path: String,
    principal: Origin,
    pub handle: Option<u64>,
    pub info: Option<FacetImageInfo>,
    pub view: FacetView,
    pub viewport: (u32, u32),
    next_receipt: u64,
    /// The receipt of the newest render request; older frames are dropped.
    want_frame: Option<u64>,
    /// The last message Facet refused with, for the title bar / stderr.
    pub last_error: Option<String>,
    /// The last export written.
    pub exported: Option<String>,
}

impl Viewer {
    pub fn new(path: &str, principal: Origin, viewport: (u32, u32)) -> Self {
        Viewer {
            path: path.to_string(),
            principal,
            handle: None,
            info: None,
            view: FacetView::default(),
            viewport: (viewport.0.max(1), viewport.1.max(1)),
            next_receipt: 1,
            want_frame: None,
            last_error: None,
            exported: None,
        }
    }

    fn receipt(&mut self) -> u64 {
        let r = RECEIPT_TAG | self.next_receipt;
        self.next_receipt += 1;
        r
    }

    /// Is `cmd` an answer to one of this viewer's requests?
    pub fn owns(&self, cmd: &FacetCommand) -> bool {
        !cmd.is_request() && cmd.receipt_id().is_some_and(|r| r >> 56 == RECEIPT_TAG >> 56 && r & !(0xFF << 56) < self.next_receipt)
    }

    /// The first request: open the file.
    pub fn start(&mut self) -> Vec<FacetCommand> {
        let receipt_id = self.receipt();
        vec![FacetCommand::ImageOpen { receipt_id, principal: self.principal.clone(), path: self.path.clone() }]
    }

    fn render(&mut self) -> Vec<FacetCommand> {
        let Some(handle) = self.handle else { return Vec::new() };
        let receipt_id = self.receipt();
        self.want_frame = Some(receipt_id);
        let (width, height) = (self.viewport.0.min(facet::MAX_VIEWPORT), self.viewport.1.min(facet::MAX_VIEWPORT));
        vec![FacetCommand::ImageRender { receipt_id, handle, width, height, view: self.view.clone() }]
    }

    fn edit(&mut self, edit: FacetEdit) -> Vec<FacetCommand> {
        let Some(handle) = self.handle else { return Vec::new() };
        let receipt_id = self.receipt();
        vec![FacetCommand::ImageEdit { receipt_id, handle, edit }]
    }

    /// The edited picture's size as the view displays it (after the view's turns).
    fn displayed_size(&self) -> (u32, u32) {
        let (w, h) = self.info.as_ref().map_or((1, 1), |i| (i.width, i.height));
        if self.view.quarter_turns % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// The scale the current view shows the picture at.
    pub fn scale(&self) -> f64 {
        let (w, h) = self.displayed_size();
        facet::view::scale(self.view.zoom, w, h, self.viewport.0, self.viewport.1)
    }

    fn zoom_by(&mut self, factor: f64) {
        let p = (self.scale() * 100.0 * factor).round().clamp(1.0, 6400.0) as u32;
        self.view.zoom = FacetZoom::Percent(p);
    }

    /// The visible region of the picture, in the edited picture's own coordinates:
    /// `(x, y, width, height)`, or `None` when nothing is visible.
    pub fn visible_region(&self) -> Option<(u32, u32, u32, u32)> {
        let (tw, th) = self.displayed_size();
        let (vw, vh) = self.viewport;
        let (left, top, dw, dh, s) = facet::view::placement(tw, th, vw, vh, &self.view);
        // Visible span of the displayed picture, in displayed-picture pixels.
        let span = |origin: i64, d: u32, v: u32, t: u32| -> Option<(u32, u32)> {
            let a = (-origin).max(0) as f64;
            let b = ((v as i64 - origin).min(d as i64)) as f64;
            if b <= a {
                return None;
            }
            let lo = ((a / s).floor() as u32).min(t - 1);
            let hi = ((b / s).ceil() as u32).clamp(lo + 1, t);
            Some((lo, hi))
        };
        let (x0, x1) = span(left, dw, vw, tw)?;
        let (y0, y1) = span(top, dh, vh, th)?;
        // Back through the view transform (rotation, then flips) to picture coordinates.
        let (iw, ih) = self.info.as_ref().map_or((1, 1), |i| (i.width, i.height));
        // Inverse of rule 1 (flips, then clockwise turns) for each turn count.
        let inv = |tx: u32, ty: u32| -> (u32, u32) {
            let (x, y) = match self.view.quarter_turns % 4 {
                1 => (ty, ih - 1 - tx),
                2 => (iw - 1 - tx, ih - 1 - ty),
                3 => (iw - 1 - ty, tx),
                _ => (tx, ty),
            };
            let x = if self.view.flip_h { iw - 1 - x } else { x };
            let y = if self.view.flip_v { ih - 1 - y } else { y };
            (x, y)
        };
        let (ax, ay) = inv(x0, y0);
        let (bx, by) = inv(x1 - 1, y1 - 1);
        let (lx, hx) = (ax.min(bx), ax.max(bx));
        let (ly, hy) = (ay.min(by), ay.max(by));
        Some((lx, ly, hx - lx + 1, hy - ly + 1))
    }

    /// One shortcut. Returns the Facet requests it causes.
    pub fn key(&mut self, key: &str) -> Vec<FacetCommand> {
        const PAN: i32 = 32;
        match key {
            "+" | "=" => self.zoom_by(1.25),
            "-" => self.zoom_by(1.0 / 1.25),
            "0" => {
                self.view.zoom = FacetZoom::Fit;
                self.view.pan_x = 0;
                self.view.pan_y = 0;
            }
            "1" => self.view.zoom = FacetZoom::Actual,
            "r" => self.view.quarter_turns = (self.view.quarter_turns + 1) % 4,
            "R" => self.view.quarter_turns = (self.view.quarter_turns + 3) % 4,
            "h" => self.view.flip_h = !self.view.flip_h,
            "v" => self.view.flip_v = !self.view.flip_v,
            // AppKit's arrow keys arrive as the private-use function-key characters.
            "\u{F700}" | "Up" => self.view.pan_y += PAN,
            "\u{F701}" | "Down" => self.view.pan_y -= PAN,
            "\u{F702}" | "Left" => self.view.pan_x += PAN,
            "\u{F703}" | "Right" => self.view.pan_x -= PAN,
            "]" => return self.edit(FacetEdit::Rotate { quarter_turns: 1 }),
            "[" => return self.edit(FacetEdit::Rotate { quarter_turns: 3 }),
            "m" => return self.edit(FacetEdit::Flip { horizontal: true }),
            "x" => {
                return match self.visible_region() {
                    Some((x, y, width, height)) => {
                        self.view.pan_x = 0;
                        self.view.pan_y = 0;
                        self.edit(FacetEdit::Crop { x, y, width, height })
                    }
                    None => Vec::new(),
                };
            }
            "b" => return self.edit(FacetEdit::Adjust { brightness: 0.9, contrast: 1.0 }),
            "B" => return self.edit(FacetEdit::Adjust { brightness: 1.1, contrast: 1.0 }),
            "c" => return self.edit(FacetEdit::Adjust { brightness: 1.0, contrast: 0.9 }),
            "C" => return self.edit(FacetEdit::Adjust { brightness: 1.0, contrast: 1.1 }),
            "u" => return self.edit(FacetEdit::Undo),
            "U" => return self.edit(FacetEdit::Redo),
            "!" => return self.edit(FacetEdit::Reset),
            "e" => {
                let Some(handle) = self.handle else { return Vec::new() };
                let receipt_id = self.receipt();
                let p = std::path::Path::new(&self.path);
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
                let out = p.with_file_name(format!("{stem}.facet.png")).to_string_lossy().into_owned();
                return vec![FacetCommand::ImageExport { receipt_id, handle, path: out, format: FacetFormat::Png, overwrite: true }];
            }
            _ => return Vec::new(),
        }
        self.render()
    }

    /// The wheel: zoom by a factor of 1.1 per notch-equivalent (positive `dy` zooms out, as the
    /// surface reports scroll-down).
    pub fn scroll(&mut self, dy: f64) -> Vec<FacetCommand> {
        if dy == 0.0 || self.handle.is_none() {
            return Vec::new();
        }
        self.zoom_by(if dy < 0.0 { 1.1 } else { 1.0 / 1.1 });
        self.render()
    }

    pub fn resize(&mut self, w: u32, h: u32) -> Vec<FacetCommand> {
        self.viewport = (w.max(1), h.max(1));
        self.render()
    }

    /// One answer from Facet. Returns the follow-up requests and, for the frame we wanted, the
    /// blit for the surface.
    pub fn answer(&mut self, cmd: &FacetCommand) -> (Vec<FacetCommand>, Option<SMessage>) {
        if !self.owns(cmd) {
            return (Vec::new(), None);
        }
        match cmd {
            FacetCommand::ImageOpened { handle, info, .. } => {
                self.handle = Some(*handle);
                self.info = Some(info.clone());
                (self.render(), None)
            }
            FacetCommand::ImageInfoIs { info, .. } => {
                self.info = Some(info.clone());
                (self.render(), None)
            }
            FacetCommand::ImageRendered { receipt_id, width, height, rgba, .. } if self.want_frame == Some(*receipt_id) => {
                let blit = SMessage::SurfaceBlit { url: SURFACE.into(), width: *width, height: *height, pixels: rgba.clone() };
                (Vec::new(), Some(blit))
            }
            FacetCommand::ImageExported { path, .. } => {
                self.exported = Some(path.clone());
                (Vec::new(), None)
            }
            FacetCommand::ImageError { message, .. } => {
                self.last_error = Some(message.clone());
                (Vec::new(), None)
            }
            _ => (Vec::new(), None),
        }
    }

    /// One window event as quartzite's image surface delivers it.
    pub fn input(&mut self, msg: &SMessage) -> Vec<FacetCommand> {
        match msg {
            SMessage::BrowserText(t) => t.chars().flat_map(|c| self.key(&c.to_string())).collect(),
            SMessage::BrowserKey(k) => self.key(k),
            SMessage::BrowserScroll(_, dy) => self.scroll(*dy),
            SMessage::BrowserResize(w, h) => self.resize(*w, *h),
            _ => Vec::new(),
        }
    }

    /// The window title.
    pub fn title(&self) -> String {
        let name = std::path::Path::new(&self.path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match &self.info {
            Some(i) => format!("{name} — {}x{} {} — {:.0} %", i.width, i.height, i.format, self.scale() * 100.0),
            None => format!("{name} — facet"),
        }
    }
}
