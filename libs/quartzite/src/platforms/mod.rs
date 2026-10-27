// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The Embassies.
//!
//! Each module represents a sovereign territory we operate within.
//! We adapt to their customs (APIs) without compromising our core logic.

#[cfg(feature = "gtk")]
pub mod gtk;

#[cfg(feature = "gnome")]
pub mod gnome;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(feature = "qt")]
pub mod qt;

/// AETHERSURFACEHEIGHT (LEDGER SR411): the page surface's geometry, written once. THE RULE: the page has ONE scale.
/// The engine's viewport is the view's size in points (CSS px at zoom 100; `BrowserResize`), the frame and the texture
/// are that size, and the backing scale enters only at the drawable (`bounds x backingScaleFactor`), which the layer
/// fills by linear upsampling; the quad (`tetra::surface_ndc`) covers the view exactly when the frame is the viewport.
/// AppKit's `image_view::fire_resize` (the viewport) and `gpu_view::sync_metal_layer` (the drawable) read it, and
/// `gpu_view`'s `[surface] geometry:` line prints `witness`. Host-testable: no platform type crosses it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeometry {
    /// The view in whole points, at least 1x1: what `BrowserResize` carries.
    pub viewport: (u32, u32),
    /// The view x the backing scale, rounded, at least 1x1: the drawable in pixels.
    pub drawable: (u32, u32),
    /// The frame the engine last sent (the texture's size).
    pub frame: (u32, u32),
    /// The frame's quad in the view (`tetra::surface_ndc`); `None` for an empty view or frame.
    pub quad: Option<[f32; 4]>,
    view: (f64, f64),
    scale: f64,
}

impl PageGeometry {
    /// The frame fills the view exactly (quad `[-1, 1, 1, -1]`).
    pub fn covers(&self) -> bool {
        self.quad == Some([-1.0, 1.0, 1.0, -1.0])
    }

    /// `view=<w>x<h>pt scale=<s> viewport=<w>x<h> frame=<w>x<h> drawable=<w>x<h>px covers=<yes|no>`.
    pub fn witness(&self) -> String {
        format!(
            "view={}x{}pt scale={} viewport={}x{} frame={}x{} drawable={}x{}px covers={}",
            self.view.0, self.view.1, self.scale, self.viewport.0, self.viewport.1, self.frame.0, self.frame.1,
            self.drawable.0, self.drawable.1, if self.covers() { "yes" } else { "no" }
        )
    }
}

/// AETHERSURFACEHEIGHT (SR411): the page geometry for a view of `view` points at backing `scale` (a non-positive
/// scale reads as 1) showing a `frame` (texture) of that many pixels.
pub fn page_geometry(view: (f64, f64), scale: f64, frame: (u32, u32)) -> PageGeometry {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let viewport = (view.0.max(1.0) as u32, view.1.max(1.0) as u32);
    let drawable = ((view.0 * scale).round().max(1.0) as u32, (view.1 * scale).round().max(1.0) as u32);
    let quad = crate::tetra::surface_ndc(view.0, view.1, frame.0, frame.1);
    PageGeometry { viewport, drawable, frame, quad, view, scale }
}
