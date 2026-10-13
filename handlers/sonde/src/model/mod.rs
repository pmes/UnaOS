// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The probe pane — Sonde's face MODEL, std-free (`core` + `alloc` only; every path below is spelled
//! `alloc::` / `core::` so the module lifts into a `no_std` ring-3 program unchanged).
//!
//! R108, THE PANE CONTRACT: a SURFACE (RGBA pixels out), an INPUT STREAM (pointer, scroll, keys in) and a
//! RESIZE — nothing widget-shaped. The GTK face (`libs/quartzite/src/platforms/gtk/probe_view.rs`) paints
//! the surface and sends input; a UnaOS-native face does the same over its own glass.
//!
//! What the pane shows: the DEPTH stream through `probe_core::ramp`'s colour ramp in the left half, the
//! COLOUR (or infrared) stream in the right half, each aspect-fitted into its half. The pointer over the
//! depth half is read back as a depth sample and a 3D point (`probe_core::depth`), which is the one
//! piece of state a face shows beside the pixels ([`ProbePane::readout`]). Scroll moves the ramp's far
//! limit; `+` / `-` do the same by key, `r` resets.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use probe_core::depth::{self, DepthScale, Intrinsics, Point3};
use probe_core::frame::Frame;
use probe_core::ramp;

/// One input event, as the face sends it. The wire form is the text of `SMessage::Input { text }`:
/// `move <x> <y>` (surface pixels), `leave`, `scroll <dy>` (+ = away), `key <name>`, `resize <w> <h>`.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneInput {
    PointerMove { x: f32, y: f32 },
    PointerLeave,
    Scroll { dy: f32 },
    Key(String),
    Resize { width: u32, height: u32 },
}

impl PaneInput {
    pub fn parse(text: &str) -> Option<PaneInput> {
        let mut it = text.split_ascii_whitespace();
        let verb = it.next()?;
        let mut num = || it.next().and_then(|t| t.parse::<f32>().ok());
        let out = match verb {
            "move" => PaneInput::PointerMove { x: num()?, y: num()? },
            "leave" => PaneInput::PointerLeave,
            "scroll" => PaneInput::Scroll { dy: num()? },
            "resize" => {
                let (w, h) = (num()?, num()?);
                if !(w >= 1.0 && h >= 1.0 && w <= 16384.0 && h <= 16384.0) {
                    return None;
                }
                PaneInput::Resize { width: w as u32, height: h as u32 }
            }
            "key" => {
                let rest = text.trim_start().strip_prefix("key")?.trim();
                if rest.is_empty() {
                    return None;
                }
                PaneInput::Key(rest.to_string())
            }
            _ => return None,
        };
        Some(out)
    }

    pub fn to_text(&self) -> String {
        match self {
            PaneInput::PointerMove { x, y } => alloc::format!("move {x} {y}"),
            PaneInput::PointerLeave => "leave".to_string(),
            PaneInput::Scroll { dy } => alloc::format!("scroll {dy}"),
            PaneInput::Key(k) => alloc::format!("key {k}"),
            PaneInput::Resize { width, height } => alloc::format!("resize {width} {height}"),
        }
    }
}

/// What the pointer is over, on the depth half.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Readout {
    /// The depth pixel under the pointer.
    pub px: u32,
    pub py: u32,
    /// Metres there (`None` = a hole).
    pub metres: Option<f32>,
    /// The deprojected point (`None` = a hole, or no intrinsics).
    pub point: Option<Point3>,
}

/// A rectangle on the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32 && y >= self.y as f32 && x < (self.x + self.w) as f32 && y < (self.y + self.h) as f32
    }
}

/// The largest `sw:sh` rectangle centred in `area`.
pub fn fit(area: Rect, sw: u32, sh: u32) -> Rect {
    if sw == 0 || sh == 0 || area.w == 0 || area.h == 0 {
        return Rect { x: area.x, y: area.y, w: 0, h: 0 };
    }
    // width-limited unless that overflows the height
    let (mut w, mut h) = (area.w, (area.w as u64 * sh as u64 / sw as u64) as u32);
    if h > area.h {
        h = area.h;
        w = (area.h as u64 * sw as u64 / sh as u64) as u32;
    }
    Rect { x: area.x + (area.w - w) / 2, y: area.y + (area.h - h) / 2, w: w.max(1), h: h.max(1) }
}

pub const NEAR_M: f32 = 0.2;
pub const FAR_DEFAULT_M: f32 = 4.0;
pub const FAR_MAX_M: f32 = 20.0;

pub struct ProbePane {
    width: u32,
    height: u32,
    surface: Vec<u8>,
    near_m: f32,
    far_m: f32,
    pointer: Option<(f32, f32)>,
    depth: Option<(Frame, DepthScale, Option<Intrinsics>)>,
    color: Option<Frame>,
}

impl ProbePane {
    pub fn new(width: u32, height: u32) -> ProbePane {
        let (width, height) = (width.max(1), height.max(1));
        let mut p = ProbePane {
            width,
            height,
            surface: Vec::new(),
            near_m: NEAR_M,
            far_m: FAR_DEFAULT_M,
            pointer: None,
            depth: None,
            color: None,
        };
        p.compose();
        p
    }

    /// The surface: RGBA8, `width * height * 4` bytes, row-major, top row first.
    pub fn surface(&self) -> (&[u8], u32, u32) {
        (&self.surface, self.width, self.height)
    }

    pub fn range(&self) -> (f32, f32) {
        (self.near_m, self.far_m)
    }

    /// The left half (depth) and right half (colour) of the surface.
    pub fn halves(&self) -> (Rect, Rect) {
        let lw = self.width / 2;
        (
            Rect { x: 0, y: 0, w: lw, h: self.height },
            Rect { x: lw, y: 0, w: self.width - lw, h: self.height },
        )
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.compose();
    }

    pub fn set_depth(&mut self, frame: Frame, scale: DepthScale, intrinsics: Option<Intrinsics>) {
        self.depth = Some((frame, scale, intrinsics));
        self.compose();
    }

    pub fn set_color(&mut self, frame: Frame) {
        self.color = Some(frame);
        self.compose();
    }

    /// Apply one input event; true when the surface (or the readout) changed.
    pub fn input(&mut self, ev: &PaneInput) -> bool {
        match ev {
            PaneInput::PointerMove { x, y } => {
                self.pointer = Some((*x, *y));
                true
            }
            PaneInput::PointerLeave => self.pointer.take().is_some(),
            PaneInput::Scroll { dy } => {
                // one notch = 10 % of the far limit; never inside the near limit, never past FAR_MAX_M
                let f = (self.far_m * (1.0 + 0.1 * dy)).clamp(self.near_m + 0.1, FAR_MAX_M);
                self.set_far(f)
            }
            PaneInput::Key(k) => match k.as_str() {
                "plus" | "+" | "KP_Add" => self.set_far((self.far_m * 1.25).min(FAR_MAX_M)),
                "minus" | "-" | "KP_Subtract" => self.set_far((self.far_m * 0.8).max(self.near_m + 0.1)),
                "r" | "R" => self.set_far(FAR_DEFAULT_M),
                _ => false,
            },
            PaneInput::Resize { width, height } => {
                let changed = *width != self.width || *height != self.height;
                if changed {
                    self.resize(*width, *height);
                }
                changed
            }
        }
    }

    fn set_far(&mut self, f: f32) -> bool {
        if f == self.far_m {
            return false;
        }
        self.far_m = f;
        self.compose();
        true
    }

    /// The depth rectangle the current depth frame is drawn in (aspect-fitted in the left half).
    pub fn depth_rect(&self) -> Option<Rect> {
        let (d, _) = self.halves();
        self.depth.as_ref().map(|(f, _, _)| fit(d, f.width, f.height))
    }

    /// The pointer read back against the depth frame: which pixel, how far, where in space.
    pub fn readout(&self) -> Option<Readout> {
        let (x, y) = self.pointer?;
        let (f, scale, intr) = self.depth.as_ref()?;
        let r = self.depth_rect()?;
        if !r.contains(x, y) || r.w == 0 || r.h == 0 {
            return None;
        }
        let px = (((x - r.x as f32) / r.w as f32) * f.width as f32) as u32;
        let py = (((y - r.y as f32) / r.h as f32) * f.height as f32) as u32;
        let (px, py) = (px.min(f.width - 1), py.min(f.height - 1));
        let metres = depth::metres_at(f, *scale, px, py);
        let point = match (metres, intr) {
            (Some(m), Some(k)) => k.deproject(px as f32 + 0.5, py as f32 + 0.5, m),
            _ => None,
        };
        Some(Readout { px, py, metres, point })
    }

    /// Recompose the whole surface: background, depth ramp left, colour right.
    fn compose(&mut self) {
        let (w, h) = (self.width, self.height);
        self.surface = vec![0u8; w as usize * h as usize * 4];
        // background: a dark slate, so a hole (black) in the ramp is distinguishable from no picture
        for px in self.surface.chunks_exact_mut(4) {
            px.copy_from_slice(&[24, 26, 30, 255]);
        }
        let (dl, cr) = self.halves();
        if let Some((f, scale, _)) = &self.depth {
            let rgba = ramp::depth_to_rgba(f, *scale, self.near_m, self.far_m);
            let r = fit(dl, f.width, f.height);
            ramp::blit_scaled(&mut self.surface, w, h, r.x, r.y, r.w, r.h, &rgba, f.width, f.height);
        }
        if let Some(f) = &self.color {
            let rgba = ramp::to_rgba(f, DepthScale::D400_DEFAULT, self.near_m, self.far_m);
            let r = fit(cr, f.width, f.height);
            ramp::blit_scaled(&mut self.surface, w, h, r.x, r.y, r.w, r.h, &rgba, f.width, f.height);
        }
    }
}
