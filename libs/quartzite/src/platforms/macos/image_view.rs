// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The image view: a CPU raster surface for the `facet` viewer vessel.
//!
//! A view that holds one decoded 8-bit sRGB RGBA frame as an
//! `NSBitmapImageRep` and draws it through a live zoom/pan transform on a dark
//! field — the smallest honest "it shows the picture, and you can inspect it"
//! seam. The pixels are prepared by the vessel (`facet` packs `lux`'s linear
//! `RgbBuffer` through the sRGB OETF); this view is a dumb blitter and owns no
//! color math.
//!
//! FACET-2 gives the static viewer hands:
//!   * scroll-wheel / trackpad-pinch **zoom about the cursor**,
//!   * click-drag **pan**,
//!   * `0` / `f` **reset-to-fit**,
//!   * a live **pixel readout** overlay (source pixel coords + packed 8-bit
//!     sRGB + the source linear RGB `lux` decoded) that follows the mouse.
//!
//! # Input idiom (precedent for quartzite pointer handling)
//!
//! [`super::tone_panel`] established the *control* idiom (AppKit target-action
//! into a `define_class!` view whose ivars hold Rust callbacks). This view
//! establishes the *pointer* idiom for a self-contained interactive surface:
//! the `NSView` handles its own `scrollWheel:`/`magnifyWithEvent:`/
//! `mouseDown:`/`mouseDragged:`/`mouseMoved:`/`keyDown:` overrides directly,
//! keeps all interaction state in `Cell`/`RefCell` ivars, and re-derives the
//! whole layout from the live bounds on every `drawRect:`. Mouse-move tracking
//! rides an `NSTrackingArea` rebuilt from `updateTrackingAreas`. `NSEvent` and
//! `NSTrackingArea` are reached via `class!` + `msg_send!` (as FACET-1 reached
//! `NSColorSpace`), so quartzite's `Cargo.toml` stays zero-diff — no new
//! objc2-app-kit feature. The view is not flipped (see the note in the class
//! body): the picture draws right-side-up and the source-row math compensates.

use std::cell::{Cell, RefCell};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSBezierPath, NSBitmapImageRep, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSImageRep, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString,
};

/// The console field the picture sits on (UnaOS Moonstone; matches the meter).
const FIELD: (f64, f64, f64) = (0x2D as f64 / 255.0, 0x2B as f64 / 255.0, 0x55 as f64 / 255.0);

/// Zoom multiplier bounds (on top of the aspect-fit base scale; `1.0` = fit).
const MIN_ZOOM: f64 = 0.1;
const MAX_ZOOM: f64 = 64.0;

/// Readout overlay palette.
const OVERLAY_BG: (f64, f64, f64, f64) = (0.0, 0.0, 0.0, 0.66);
const OVERLAY_TEXT: u32 = 0xE8E4F0;

// -- NSTrackingAreaOptions bits (we reach NSTrackingArea generically) --------
const TRK_MOUSE_ENTERED_EXITED: usize = 0x01;
const TRK_MOUSE_MOVED: usize = 0x02;
const TRK_ACTIVE_IN_KEY_WINDOW: usize = 0x20;
const TRK_IN_VISIBLE_RECT: usize = 0x200;

// ---------------------------------------------------------------------------
// THE VIEW
// ---------------------------------------------------------------------------
pub struct FacetImageIvars {
    /// The decoded frame as an AppKit bitmap rep (8-bit sRGB RGBA), or `None`
    /// until one is set. Drawn through the zoom/pan transform every `drawRect:`.
    rep: RefCell<Option<Retained<NSBitmapImageRep>>>,
    /// Pixel dimensions of the current frame (for the fit + readout math).
    size: RefCell<(u32, u32)>,
    /// A copy of the packed 8-bit sRGB RGBA bytes, kept for the pixel readout
    /// (`width * height * 4`, row-major, top row first).
    rgba: RefCell<Vec<u8>>,
    /// The source linear RGB `lux` decoded (`width * height * 3`, top row
    /// first). Empty if the vessel did not supply it; the readout then omits
    /// the linear line.
    linear: RefCell<Vec<f32>>,

    /// User zoom multiplier on top of the aspect-fit base scale (`1.0` = fit).
    zoom: Cell<f64>,
    /// Pan offset, in view points, added to the centered image origin.
    pan: Cell<(f64, f64)>,
    /// Last cursor position (view points), or `None` when the pointer is
    /// outside — drives the readout overlay.
    cursor: Cell<Option<(f64, f64)>>,
    /// Last drag anchor (window points) while a pan is in progress.
    drag_anchor: Cell<Option<(f64, f64)>>,
    /// The live mouse-move tracking area (rebuilt from `updateTrackingAreas`).
    tracking: RefCell<Option<Retained<AnyObject>>>,
    /// Browser mode: when set, pointer input forwards to the engine over
    /// this synapse (clicks as surface-pixel BrowserClick, wheel as
    /// BrowserScroll) instead of driving facet zoom/pan.
    synapse: RefCell<Option<bandy::Synapse>>,

    /// True between `viewWillStartLiveResize` and `viewDidEndLiveResize` —
    /// i.e. while the user is dragging a window edge. Resize notifications to
    /// the engine are throttled for the duration (see `maybe_fire_resize`).
    live_resize: Cell<bool>,
    /// When the last `BrowserResize` went out, for the live-resize throttle.
    last_resize_fire: Cell<Option<std::time::Instant>>,
    /// The geometry of the last `BrowserResize` we fired — the engine gets a
    /// given size at most once (dedupes the guaranteed end-of-drag fire against
    /// a throttled one that already carried the same size).
    last_resize_sent: Cell<Option<(u32, u32)>>,
}

/// Coarse tracking cadence during a live-resize drag: one `BrowserResize` per
/// this interval, plus a guaranteed final one when the drag ends.
const LIVE_RESIZE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

define_class!(
    #[unsafe(super(NSView))]
    #[name = "UnaFacetImageView"]
    #[ivars = FacetImageIvars]
    pub struct FacetImageView;

    impl FacetImageView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(FacetImageIvars {
                rep: RefCell::new(None),
                size: RefCell::new((0, 0)),
                rgba: RefCell::new(Vec::new()),
                linear: RefCell::new(Vec::new()),
                zoom: Cell::new(1.0),
                pan: Cell::new((0.0, 0.0)),
                cursor: Cell::new(None),
                drag_anchor: Cell::new(None),
                tracking: RefCell::new(None),
                synapse: RefCell::new(None),
                live_resize: Cell::new(false),
                last_resize_fire: Cell::new(None),
                last_resize_sent: Cell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            self.render();
        }

        // NOTE: deliberately NOT flipped. `NSImageRep drawInRect:` does not
        // compensate for a flipped coordinate context, so an `isFlipped = YES`
        // override mirrors the picture top-to-bottom (caught by the FACET-1
        // attended eye-witness). The fit/pan math and the source-row lookup in
        // the readout both compensate for the bottom-left origin, so the
        // default (unflipped) coordinate system is correct and simplest.

        // Any resize re-derives the layout from the live bounds.
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: new_size] };
            // Browser surface: the viewport changed — the ENGINE reflows the
            // page to the new size and blits fresh pixels; nothing scales.
            // A relayout costs real time on a heavy page, so a drag gets a
            // coarse cadence, not one reflow per AppKit resize event. Any
            // resize that is NOT a drag (zoom button, programmatic setFrame,
            // window restore) still fires immediately.
            self.maybe_fire_resize(new_size);
            self.setNeedsDisplay(true);
        }

        // -- live resize: throttle during the drag, one guaranteed final -----
        #[unsafe(method(viewWillStartLiveResize))]
        fn view_will_start_live_resize(&self) {
            let _: () = unsafe { msg_send![super(self), viewWillStartLiveResize] };
            self.ivars().live_resize.set(true);
        }

        #[unsafe(method(viewDidEndLiveResize))]
        fn view_did_end_live_resize(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidEndLiveResize] };
            self.ivars().live_resize.set(false);
            // The drag settled: the engine must end up laid out for the size
            // the user actually let go at, whatever the throttle last sent.
            self.fire_resize(self.frame().size);
            self.setNeedsDisplay(true);
        }

        // -- first responder: keyDown needs it; install on join --------------
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> objc2::runtime::Bool {
            // Clean resignation when another responder takes focus (e.g., the URL
            // TextField). Return YES to allow the transition.
            objc2::runtime::Bool::YES
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            // Become first responder only in facet mode (not browser mode).
            // In browser mode, let the window manage focus so the URL TextField
            // can accept input naturally; the view becomes first responder on click.
            if self.ivars().synapse.borrow().is_none() {
                if let Some(window) = self.window() {
                    let _: bool = unsafe { msg_send![&window, makeFirstResponder: self] };
                }
            }
        }

        // -- mouse-move tracking --------------------------------------------
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            self.rebuild_tracking_area();
        }

        // -- pointer input (the precedent-setting idiom) --------------------
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &AnyObject) {
            let dy: f64 = unsafe { msg_send![event, scrollingDeltaY] };
            if let Some(syn) = self.ivars().synapse.borrow().as_ref() {
                // Browser surface: wheel scrolls the page (deltaY > 0 is
                // fingers-down/content-up in natural scrolling; the engine
                // wants positive dy = scroll further down the document).
                if dy != 0.0 {
                    syn.fire(bandy::SMessage::BrowserScroll(0.0, -dy * 3.0));
                }
                return;
            }
            let precise: bool = unsafe { msg_send![event, hasPreciseScrollingDeltas] };
            if dy == 0.0 {
                return;
            }
            // Precise (trackpad) deltas are many points per gesture; line-based
            // wheel deltas are a few units per notch — scale so both feel sane,
            // and clamp the per-event factor so no single event teleports.
            let k = if precise { 0.006 } else { 0.10 };
            let factor = (dy * k).exp().clamp(0.5, 2.0);
            let at = self.event_point(event);
            self.zoom_about(factor, at);
        }

        #[unsafe(method(magnifyWithEvent:))]
        fn magnify_with_event(&self, event: &AnyObject) {
            // `magnification` is the incremental pinch delta for this event.
            let mag: f64 = unsafe { msg_send![event, magnification] };
            if mag == 0.0 || self.ivars().synapse.borrow().is_some() {
                return;
            }
            let at = self.event_point(event);
            self.zoom_about(1.0 + mag, at);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &AnyObject) {
            // Anchor the pan and take key focus for reset-to-fit.
            let win = self.event_window_point(event);
            self.ivars().drag_anchor.set(Some(win));
            if let Some(window) = self.window() {
                let _: bool = unsafe { msg_send![&window, makeFirstResponder: self] };
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &AnyObject) {
            if self.ivars().synapse.borrow().is_some() {
                return; // browser surface: no picture panning
            }
            let (nx, ny) = self.event_window_point(event);
            if let Some((ax, ay)) = self.ivars().drag_anchor.get() {
                let (px, py) = self.ivars().pan.get();
                // Unflipped view: a window-point delta maps 1:1 to the image
                // origin, so the picture tracks the cursor.
                self.ivars().pan.set((px + (nx - ax), py + (ny - ay)));
                self.ivars().drag_anchor.set(Some((nx, ny)));
                // Keep the readout live under the moving cursor.
                self.ivars().cursor.set(Some(self.event_point(event)));
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &AnyObject) {
            self.ivars().drag_anchor.set(None);
            let syn = self.ivars().synapse.borrow().clone();
            if let Some(syn) = syn {
                // Map the view point through the aspect-fit transform into
                // surface pixels (unflipped view: invert Y for the top-left
                // origin the engine uses).
                let (vx, vy) = self.event_point(event);
                let bounds = self.bounds();
                if let Some((dx, dy, dw, dh)) = self.image_dest(bounds) {
                    let (iw, ih) = *self.ivars().size.borrow();
                    if dw > 0.0 && dh > 0.0 && iw > 0 && ih > 0 {
                        let px = (vx - dx) / dw * iw as f64;
                        let py = ih as f64 - ((vy - dy) / dh * ih as f64);
                        if px >= 0.0 && px < iw as f64 && py >= 0.0 && py < ih as f64 {
                            syn.fire(bandy::SMessage::BrowserClick(px, py));
                        }
                    }
                }
            }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &AnyObject) {
            self.ivars().cursor.set(Some(self.event_point(event)));
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &AnyObject) {
            self.ivars().cursor.set(Some(self.event_point(event)));
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &AnyObject) {
            self.ivars().cursor.set(None);
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &AnyObject) {
            let chars: Option<Retained<NSString>> =
                unsafe { msg_send![event, charactersIgnoringModifiers] };
            let text = chars.map(|s| s.to_string()).unwrap_or_default();
            // Browser surface: keys are page input (focused field editing),
            // not viewer shortcuts.
            if let Some(syn) = self.ivars().synapse.borrow().as_ref() {
                let mut it = text.chars();
                match (it.next(), it.next()) {
                    (Some('\u{7f}'), None) | (Some('\u{8}'), None) => {
                        syn.fire(bandy::SMessage::BrowserKey("BackSpace".into()));
                    }
                    (Some('\r'), None) | (Some('\n'), None) => {
                        syn.fire(bandy::SMessage::BrowserKey("Return".into()));
                    }
                    (Some(c), _) if !c.is_control() => {
                        syn.fire(bandy::SMessage::BrowserText(text));
                    }
                    _ => {}
                }
                return;
            }
            let handled = match text.as_str() {
                "0" | "f" | "F" => {
                    self.reset_to_fit();
                    true
                }
                _ => false,
            };
            if !handled {
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }
    }
);

impl FacetImageView {
    /// Build the view. Main thread only (the marker proves it).
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let _ = mtm;
        let this: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        unsafe { msg_send![this, initWithFrame: frame] }
    }

    /// Hand the view a decoded frame: tightly packed 8-bit sRGB RGBA
    /// (`width * height * 4` bytes, row-major, top row first, opaque alpha) and
    /// — for the pixel readout — the source *linear* RGB `lux` decoded
    /// (`width * height * 3` f32, same row order; pass `&[]` to omit the linear
    /// readout line). The sRGB bytes are copied into an AppKit-owned bitmap rep
    /// tagged sRGB so the display is color-managed; a copy of both buffers is
    /// retained for the readout. Main thread only.
    pub fn set_frame(&self, rgba: &[u8], linear: &[f32], width: u32, height: u32) {
        let expected = width as usize * height as usize * 4;
        if width == 0 || height == 0 || rgba.len() < expected {
            log::warn!(
                "[FACET] set_frame: bad buffer ({} bytes for {}x{}, need {})",
                rgba.len(),
                width,
                height,
                expected
            );
            return;
        }

        let bytes_per_row = width as usize * 4;
        let rep: Retained<NSBitmapImageRep> = unsafe {
            let alloc: Allocated<NSBitmapImageRep> = msg_send![NSBitmapImageRep::class(), alloc];
            // dataPlanes = NULL: AppKit allocates its own backing buffer that
            // we then fill. bitsPerPixel = 32, bytesPerRow = 4*w: no row
            // padding, so a single contiguous copy is exact.
            let planes: *mut *mut u8 = std::ptr::null_mut();
            let rep: Option<Retained<NSBitmapImageRep>> = msg_send![
                alloc,
                initWithBitmapDataPlanes: planes,
                pixelsWide: width as isize,
                pixelsHigh: height as isize,
                bitsPerSample: 8isize,
                samplesPerPixel: 4isize,
                hasAlpha: true,
                isPlanar: false,
                // NSDeviceRGBColorSpace; retagged to sRGB below.
                colorSpaceName: &*objc2_foundation::NSString::from_str("NSDeviceRGBColorSpace"),
                bytesPerRow: bytes_per_row as isize,
                bitsPerPixel: 32isize,
            ];
            match rep {
                Some(r) => r,
                None => {
                    log::warn!("[FACET] set_frame: NSBitmapImageRep alloc failed");
                    return;
                }
            }
        };

        // Copy our pixels into the rep's backing store.
        let data: *mut u8 = unsafe { msg_send![&rep, bitmapData] };
        if data.is_null() {
            log::warn!("[FACET] set_frame: rep has no bitmapData");
            return;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), data, expected);
        }

        // Retag (reinterpret, not convert) to sRGB: the vessel already packed
        // sRGB-encoded values, so tagging the rep sRGB makes AppKit present
        // them faithfully under color management.
        let rep = unsafe {
            let srgb: *mut AnyObject = msg_send![class!(NSColorSpace), sRGBColorSpace];
            if srgb.is_null() {
                rep
            } else {
                let retagged: Option<Retained<NSBitmapImageRep>> =
                    msg_send![&rep, bitmapImageRepByRetaggingWithColorSpace: srgb];
                retagged.unwrap_or(rep)
            }
        };

        *self.ivars().rep.borrow_mut() = Some(rep);
        *self.ivars().size.borrow_mut() = (width, height);
        *self.ivars().rgba.borrow_mut() = rgba[..expected].to_vec();
        // Keep the linear buffer for readout only if it is the expected length.
        let lin_expected = width as usize * height as usize * 3;
        if linear.len() >= lin_expected {
            *self.ivars().linear.borrow_mut() = linear[..lin_expected].to_vec();
        } else {
            self.ivars().linear.borrow_mut().clear();
        }
        self.reset_to_fit();
    }

    // -- browser resize notification ----------------------------------------

    /// Notify the engine of a new viewport size, honouring the live-resize
    /// throttle. No-op outside browser mode.
    fn maybe_fire_resize(&self, new_size: NSSize) {
        if self.ivars().synapse.borrow().is_none() {
            return;
        }
        if self.ivars().live_resize.get() {
            let now = std::time::Instant::now();
            if let Some(last) = self.ivars().last_resize_fire.get() {
                if now.duration_since(last) < LIVE_RESIZE_INTERVAL {
                    // Skipped: the drag keeps going and `viewDidEndLiveResize`
                    // guarantees the final size reaches the engine regardless.
                    return;
                }
            }
        }
        self.fire_resize(new_size);
    }

    /// Fire `BrowserResize` for `size` unless the engine already has exactly
    /// that geometry. Browser mode only.
    fn fire_resize(&self, size: NSSize) {
        let syn = self.ivars().synapse.borrow().clone();
        let Some(syn) = syn else { return };
        let (w, h) = (size.width.max(1.0) as u32, size.height.max(1.0) as u32);
        if self.ivars().last_resize_sent.get() == Some((w, h)) {
            return;
        }
        self.ivars().last_resize_sent.set(Some((w, h)));
        self.ivars().last_resize_fire.set(Some(std::time::Instant::now()));
        syn.fire(bandy::SMessage::BrowserResize(w, h));
    }

    // -- interaction --------------------------------------------------------

    /// Reset the zoom to aspect-fit and clear any pan.
    fn reset_to_fit(&self) {
        self.ivars().zoom.set(1.0);
        self.ivars().pan.set((0.0, 0.0));
        self.setNeedsDisplay(true);
    }

    /// Multiply the zoom by `factor` (clamped) while keeping the source pixel
    /// under view point `at` stationary — the "zoom about the cursor" law.
    fn zoom_about(&self, factor: f64, at: (f64, f64)) {
        let bounds = self.bounds();
        let Some((dx, dy, dw, dh)) = self.image_dest(bounds) else {
            return;
        };
        if dw <= 0.0 || dh <= 0.0 {
            return;
        }

        let old_zoom = self.ivars().zoom.get();
        let new_zoom = (old_zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if new_zoom == old_zoom {
            return;
        }

        // Image fraction under the cursor, invariant across the zoom.
        let fx = (at.0 - dx) / dw;
        let fy = (at.1 - dy) / dh;

        self.ivars().zoom.set(new_zoom);

        // New image size and the pan that pins (fx, fy) back under `at`.
        let (iw, ih) = *self.ivars().size.borrow();
        let base = self.base_scale(bounds, iw, ih);
        let ndw = iw as f64 * base * new_zoom;
        let ndh = ih as f64 * base * new_zoom;
        let (bw, bh) = (bounds.size.width, bounds.size.height);
        // dx' = origin.x + (bw - ndw)/2 + pan.x  and  dx' = at.0 - fx*ndw.
        let pan_x = (at.0 - fx * ndw) - bounds.origin.x - (bw - ndw) / 2.0;
        let pan_y = (at.1 - fy * ndh) - bounds.origin.y - (bh - ndh) / 2.0;
        self.ivars().pan.set((pan_x, pan_y));
        self.setNeedsDisplay(true);
    }

    /// The aspect-fit base scale for the current bounds (image points → view
    /// points at `zoom == 1.0`).
    fn base_scale(&self, bounds: NSRect, iw: u32, ih: u32) -> f64 {
        if iw == 0 || ih == 0 {
            return 1.0;
        }
        let (bw, bh) = (bounds.size.width, bounds.size.height);
        (bw / iw as f64).min(bh / ih as f64)
    }

    /// The image's destination rect (origin x/y, width, height) under the
    /// current zoom + pan, or `None` if no frame is loaded.
    fn image_dest(&self, bounds: NSRect) -> Option<(f64, f64, f64, f64)> {
        let (iw, ih) = *self.ivars().size.borrow();
        if iw == 0 || ih == 0 {
            return None;
        }
        // Browser surface: the engine owns layout — draw its pixels 1:1,
        // pinned to the TOP-left (unflipped view: top = origin.y + height).
        if self.ivars().synapse.borrow().is_some() {
            let (dw, dh) = (iw as f64, ih as f64);
            let dx = bounds.origin.x;
            let dy = bounds.origin.y + bounds.size.height - dh;
            return Some((dx, dy, dw, dh));
        }
        let base = self.base_scale(bounds, iw, ih);
        let scale = base * self.ivars().zoom.get();
        let dw = iw as f64 * scale;
        let dh = ih as f64 * scale;
        let (bw, bh) = (bounds.size.width, bounds.size.height);
        let (px, py) = self.ivars().pan.get();
        let dx = bounds.origin.x + (bw - dw) / 2.0 + px;
        let dy = bounds.origin.y + (bh - dh) / 2.0 + py;
        Some((dx, dy, dw, dh))
    }

    /// Convert an `NSEvent`'s location to this view's coordinates (view points).
    fn event_point(&self, event: &AnyObject) -> (f64, f64) {
        let win: NSPoint = unsafe { msg_send![event, locationInWindow] };
        let p: NSPoint = unsafe { msg_send![self, convertPoint: win, fromView: None::<&NSView>] };
        (p.x, p.y)
    }

    /// The raw window-space location of an `NSEvent` (for drag deltas, which we
    /// want independent of the view's own transform).
    fn event_window_point(&self, event: &AnyObject) -> (f64, f64) {
        let win: NSPoint = unsafe { msg_send![event, locationInWindow] };
        (win.x, win.y)
    }

    /// (Re)build the mouse-move tracking area from the current visible rect.
    fn rebuild_tracking_area(&self) {
        if let Some(old) = self.ivars().tracking.borrow_mut().take() {
            let _: () = unsafe { msg_send![self, removeTrackingArea: &*old] };
        }
        let opts: usize = TRK_MOUSE_ENTERED_EXITED
            | TRK_MOUSE_MOVED
            | TRK_ACTIVE_IN_KEY_WINDOW
            | TRK_IN_VISIBLE_RECT;
        let bounds = self.bounds();
        let area: Retained<AnyObject> = unsafe {
            let a: *mut AnyObject = msg_send![class!(NSTrackingArea), alloc];
            let a: *mut AnyObject = msg_send![
                a,
                initWithRect: bounds,
                options: opts,
                owner: self,
                userInfo: None::<&AnyObject>,
            ];
            match Retained::from_raw(a) {
                Some(r) => r,
                None => {
                    log::warn!("[FACET] tracking area init failed");
                    return;
                }
            }
        };
        let _: () = unsafe { msg_send![self, addTrackingArea: &*area] };
        *self.ivars().tracking.borrow_mut() = Some(area);
    }

    // -- drawing ------------------------------------------------------------

    fn render(&self) {
        let bounds = self.bounds();

        // The field. In browser mode the bitmap is drawn 1:1 at the top-left,
        // so during a live resize the view is routinely larger than the last
        // blit — the uncovered band is page background (white), not the
        // viewer's dark console field, so a widening drag reads as "the page
        // has not caught up yet" rather than as garbage.
        let browser = self.ivars().synapse.borrow().is_some();
        let field = if browser {
            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 1.0)
        } else {
            NSColor::colorWithSRGBRed_green_blue_alpha(FIELD.0, FIELD.1, FIELD.2, 1.0)
        };
        field.set();
        objc2_app_kit::NSRectFill(bounds);

        let rep_guard = self.ivars().rep.borrow();
        let Some(rep) = rep_guard.as_ref() else {
            return; // No frame yet.
        };
        let Some((dx, dy, dw, dh)) = self.image_dest(bounds) else {
            return;
        };
        let dest = NSRect::new(
            NSPoint::new(dx, dy),
            NSSize::new(dw.max(1.0), dh.max(1.0)),
        );

        let _: bool = unsafe {
            let rep_ref: &NSImageRep = rep.as_ref();
            msg_send![rep_ref, drawInRect: dest]
        };

        // The pixel readout overlay (facet only — a browser page is not
        // an inspected picture).
        if !browser {
            self.draw_readout(bounds, (dx, dy, dw, dh));
        }
    }

    /// Map the current cursor to a source pixel and draw the readout overlay.
    fn draw_readout(&self, bounds: NSRect, dest: (f64, f64, f64, f64)) {
        let Some((cx, cy)) = self.ivars().cursor.get() else {
            return;
        };
        let (dx, dy, dw, dh) = dest;
        if dw <= 0.0 || dh <= 0.0 {
            return;
        }
        // Fraction across the drawn image; bail if the cursor is off it.
        let fx = (cx - dx) / dw;
        let fy = (cy - dy) / dh;
        if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
            return;
        }
        let (iw, ih) = *self.ivars().size.borrow();
        if iw == 0 || ih == 0 {
            return;
        }
        let sx = ((fx * iw as f64) as u32).min(iw - 1);
        // Unflipped view: fy == 1 is the TOP of the drawn image, which is
        // source row 0 (the picture draws right-side-up). Flip to a top-down
        // row index.
        let sy = (((1.0 - fy) * ih as f64) as u32).min(ih - 1);

        let idx = (sy as usize * iw as usize + sx as usize) * 4;
        let rgba = self.ivars().rgba.borrow();
        if idx + 3 >= rgba.len() {
            return;
        }
        let (r, g, b) = (rgba[idx], rgba[idx + 1], rgba[idx + 2]);

        let linear = self.ivars().linear.borrow();
        let lin = {
            let li = (sy as usize * iw as usize + sx as usize) * 3;
            if li + 2 < linear.len() {
                Some((linear[li], linear[li + 1], linear[li + 2]))
            } else {
                None
            }
        };

        let mut lines = vec![
            format!("({sx}, {sy})"),
            format!("sRGB  {r:>3} {g:>3} {b:>3}"),
            format!("#{r:02X}{g:02X}{b:02X}"),
        ];
        if let Some((lr, lg, lb)) = lin {
            lines.push(format!("lin   {lr:.3} {lg:.3} {lb:.3}"));
        }

        self.draw_overlay(bounds, &lines);
    }

    /// Draw a small text panel of `lines` in the lower-left of the bounds.
    fn draw_overlay(&self, bounds: NSRect, lines: &[String]) {
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(11.0, unsafe {
            objc2_app_kit::NSFontWeightRegular
        });
        let attrs = text_attrs(&font, OVERLAY_TEXT);

        let pad = 8.0;
        let line_h = 15.0;
        let mut text_w: f64 = 0.0;
        for line in lines {
            let ns = NSString::from_str(line);
            let sz: NSSize = unsafe { msg_send![&*ns, sizeWithAttributes: &*attrs] };
            text_w = text_w.max(sz.width);
        }
        let panel_w = text_w + pad * 2.0;
        let panel_h = line_h * lines.len() as f64 + pad * 2.0;
        let margin = 12.0;
        let px = bounds.origin.x + margin;
        let py = bounds.origin.y + margin;
        let panel = NSRect::new(NSPoint::new(px, py), NSSize::new(panel_w, panel_h));

        // Rounded translucent backing.
        let bg = NSColor::colorWithSRGBRed_green_blue_alpha(
            OVERLAY_BG.0,
            OVERLAY_BG.1,
            OVERLAY_BG.2,
            OVERLAY_BG.3,
        );
        bg.set();
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(panel, 6.0, 6.0);
        path.fill();

        // Lines top-to-bottom (unflipped: the top line has the highest y).
        for (i, line) in lines.iter().enumerate() {
            let ns = NSString::from_str(line);
            let ty = py + panel_h - pad - line_h * (i as f64 + 1.0) + 2.0;
            let at = NSPoint::new(px + pad, ty);
            let _: () = unsafe { msg_send![&*ns, drawAtPoint: at, withAttributes: &*attrs] };
        }
    }
}

// -- text helpers -----------------------------------------------------------

fn text_attrs(font: &NSFont, rgb: u32) -> Retained<NSDictionary<NSString, AnyObject>> {
    let (r, g, b) = (
        ((rgb >> 16) & 0xFF) as f64 / 255.0,
        ((rgb >> 8) & 0xFF) as f64 / 255.0,
        (rgb & 0xFF) as f64 / 255.0,
    );
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0);
    let keys: [&NSString; 2] = [
        unsafe { NSFontAttributeName },
        unsafe { NSForegroundColorAttributeName },
    ];
    let values: [&AnyObject; 2] = [font.as_ref(), color.as_ref()];
    NSDictionary::from_slices(&keys, &values)
}

// ---------------------------------------------------------------------------
// BOOTSTRAP — the vessel-facing seam
// ---------------------------------------------------------------------------

/// Build the image view for a `facet` vessel window and load one frame.
///
/// The vessel decodes and packs the picture (`lux` linear `RgbBuffer` → 8-bit
/// sRGB RGBA) and hands the packed bytes here, plus the source linear RGB for
/// the pixel readout (pass `&[]` to omit it). The view copies them, draws the
/// picture aspect-fit, and thereafter handles its own zoom/pan/readout input.
/// Returns the view to install as the window's content view. Main thread only.
pub fn bootstrap_image_view(
    _window: &objc2_app_kit::NSWindow,
    rgba: &[u8],
    linear: &[f32],
    width: u32,
    height: u32,
) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_image_view must run on the main thread");

    // Open at the image's native size, sensibly capped so a large photo does
    // not spawn a giant window; the view rescales from the live bounds after.
    let init_w = (width as f64).clamp(240.0, 1600.0);
    let init_h = (height as f64).clamp(160.0, 1000.0);
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(init_w, init_h));

    let view = FacetImageView::new(mtm, frame);
    view.set_frame(rgba, linear, width, height);

    Retained::into_super(view)
}

pub fn bootstrap_image_surface(id: &str, synapse: bandy::Synapse) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_image_surface must run on the main thread");

    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
    let view = FacetImageView::new(mtm, frame);
    *view.ivars().synapse.borrow_mut() = Some(synapse.clone());

    let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(view.clone(), mtm));
    let mut rx = synapse.subscribe();

    let target_id = id.to_string();
    // The AppKit main thread has no tokio reactor; give the subscription loop
    // its own thread + current-thread runtime and hop frames back via GCD.
    std::thread::Builder::new().name("surface-blit".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("surface subscription runtime");
        rt.block_on(async move {
        loop {
            match rx.recv().await {
                Ok(bandy::SMessage::SurfaceBlit { url, width, height, pixels }) => {
                    if url == target_id {
                        let bound = bound.clone();
                        dispatch2::DispatchQueue::main().exec_async(move || {
                            let mtm = MainThreadMarker::new().unwrap();
                            bound.get(mtm).set_frame(&pixels, &[], width, height);
                        });
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        });
    }).expect("spawn surface-blit thread");

    Retained::into_super(view)
}
