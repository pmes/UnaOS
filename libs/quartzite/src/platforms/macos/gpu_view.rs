// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The GPU image view: a `CAMetalLayer`-hosting surface for the `facet`
//! viewer vessel (FACET-GPU).
//!
//! The dependency direction is deliberate: **quartzite stays wgpu-free**.
//! This view owns the AppKit half only — a layer-hosting `NSView` whose layer
//! is a `CAMetalLayer` (reached via `class!`/`msg_send!`, the FACET-1/2
//! tradition, so quartzite's `Cargo.toml` stays zero-diff), plus the FACET-2
//! interaction state machine ported verbatim (zoom-about-cursor, drag-pan,
//! reset-to-fit, live pixel readout). The vessel (`facet`) hands the layer
//! pointer to `euclase`, which builds the wgpu surface and the textured quad;
//! what comes back across the seam is one opaque [`GpuRenderer`] closure the
//! view invokes on demand (input events + resize — a static image needs no
//! display link).
//!
//! # Ownership / safety shape (the euclase `Cortex<'static>` invariant)
//!
//! The view owns *both* sides: the `CAMetalLayer` is its hosted backing layer
//! (retained by `setLayer:`) and the renderer closure — which owns the wgpu
//! `Surface` built over that layer — lives in the view's ivars.
//! `define_class!` drops the Rust ivars (and with them the renderer, the
//! `Cortex`, and its `Surface`) before the `NSView` superclass `dealloc`
//! releases the layer, so the layer strictly outlives the surface.
//!
//! # Coordinates
//!
//! Unflipped, like the CPU image view (never `isFlipped` a view drawing a
//! picture — the FACET-1 eye-witness lesson). The dest-rect math is identical
//! to `image_view.rs`; it is handed to the renderer in view points
//! (bottom-left origin, y-up) and euclase's `view_rect_to_ndc` maps it to
//! clip space. The readout's source-row flip is the same `1 - fy` as the CPU
//! path. Retina: `drawableSize = bounds × backingScaleFactor`, maintained on
//! `setFrameSize:` AND `viewDidChangeBackingProperties`.
//!
//! # Color
//!
//! The layer's `colorspace` is pinned to sRGB at creation: euclase renders
//! into an sRGB swapchain, and tagging the layer sRGB makes the window server
//! match the CPU path's AppKit-managed presentation on a P3 display.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSView, NSWindow,
};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString,
};

/// Zoom multiplier bounds (on top of the aspect-fit base scale; `1.0` = fit).
/// Identical to the CPU image view.
const MIN_ZOOM: f64 = 0.1;
const MAX_ZOOM: f64 = 64.0;

/// Readout overlay palette (identical to the CPU image view).
const OVERLAY_BG: (f64, f64, f64, f64) = (0.0, 0.0, 0.0, 0.66);
const OVERLAY_TEXT: u32 = 0xE8E4F0;

// -- NSTrackingAreaOptions bits (we reach NSTrackingArea generically) --------
const TRK_MOUSE_ENTERED_EXITED: usize = 0x01;
const TRK_MOUSE_MOVED: usize = 0x02;
const TRK_ACTIVE_IN_KEY_WINDOW: usize = 0x20;
const TRK_IN_VISIBLE_RECT: usize = 0x200;

// ---------------------------------------------------------------------------
// THE RENDERER SEAM (what facet installs; quartzite never names wgpu)
// ---------------------------------------------------------------------------

/// Everything the vessel's renderer needs for one on-demand frame.
pub struct GpuFrameParams {
    /// The image's destination rect `(dx, dy, dw, dh)` in view points —
    /// bottom-left origin, y-up (the unflipped convention). Exactly the tuple
    /// the CPU path's `image_dest` produces.
    pub dest: (f64, f64, f64, f64),
    /// The view bounds size `(w, h)` in view points.
    pub bounds: (f64, f64),
    /// The layer's current drawable size in PIXELS (`bounds × scale`) — the
    /// renderer reconfigures its swapchain when this changes.
    pub drawable: (u32, u32),
}

/// The opaque render closure the vessel installs: owns the whole GPU side
/// (surface, device, pipeline, uploaded frame) and draws one frame per call.
pub type GpuRenderer = Box<dyn FnMut(&GpuFrameParams) + 'static>;

// ---------------------------------------------------------------------------
// THE VIEW
// ---------------------------------------------------------------------------
pub struct FacetGpuIvars {
    /// Pixel dimensions of the current frame (for the fit + readout math).
    size: RefCell<(u32, u32)>,
    /// A copy of the packed 8-bit sRGB RGBA bytes, kept for the pixel readout
    /// (`width * height * 4`, row-major, top row first).
    rgba: RefCell<Vec<u8>>,
    /// The source linear RGB `lux` decoded (`width * height * 3`, top row
    /// first). Empty if the vessel did not supply it.
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

    /// The vessel-installed GPU renderer (owns the wgpu side; see the module
    /// doc for the drop-order invariant).
    renderer: RefCell<Option<GpuRenderer>>,
    /// The hosted `CAMetalLayer` (also retained by the view as its layer; we
    /// keep our own handle for drawableSize/contentsScale maintenance).
    metal_layer: RefCell<Option<Retained<AnyObject>>>,
    /// Readout overlay: rounded translucent backing `CALayer` and its
    /// `CATextLayer`, lazily built as sublayers of the metal layer.
    overlay_bg: RefCell<Option<Retained<AnyObject>>>,
    overlay_text: RefCell<Option<Retained<AnyObject>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "UnaFacetGpuView"]
    #[ivars = FacetGpuIvars]
    pub struct FacetGpuView;

    impl FacetGpuView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(FacetGpuIvars {
                size: RefCell::new((0, 0)),
                rgba: RefCell::new(Vec::new()),
                linear: RefCell::new(Vec::new()),
                zoom: Cell::new(1.0),
                pan: Cell::new((0.0, 0.0)),
                cursor: Cell::new(None),
                drag_anchor: Cell::new(None),
                tracking: RefCell::new(None),
                renderer: RefCell::new(None),
                metal_layer: RefCell::new(None),
                overlay_bg: RefCell::new(None),
                overlay_text: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        // NOTE: deliberately NOT flipped, matching the CPU image view. The
        // dest-rect math and the readout's source-row lookup both assume the
        // bottom-left origin, and CALayer geometry on macOS is bottom-left
        // too — no hidden flip anywhere (the only flip in the GPU path is the
        // texcoord v = 1 - y in euclase's shader).

        // Any resize re-derives drawableSize + layout from the live bounds.
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: new_size] };
            self.request_render();
        }

        // Retina/display migration: contentsScale + drawableSize follow the
        // backing store.
        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidChangeBackingProperties] };
            self.request_render();
        }

        // -- first responder: keyDown needs it; install on join --------------
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            // Become first responder so reset-to-fit key events land here even
            // before the first click; draw the first in-window frame.
            if let Some(window) = self.window() {
                let _: bool = unsafe { msg_send![&window, makeFirstResponder: self] };
            }
            self.request_render();
        }

        // -- mouse-move tracking --------------------------------------------
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            self.rebuild_tracking_area();
        }

        // -- pointer input (verbatim FACET-2 state machine) ------------------
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &AnyObject) {
            let dy: f64 = unsafe { msg_send![event, scrollingDeltaY] };
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
            if mag == 0.0 {
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
            let (nx, ny) = self.event_window_point(event);
            if let Some((ax, ay)) = self.ivars().drag_anchor.get() {
                let (px, py) = self.ivars().pan.get();
                // Unflipped view: a window-point delta maps 1:1 to the image
                // origin, so the picture tracks the cursor.
                self.ivars().pan.set((px + (nx - ax), py + (ny - ay)));
                self.ivars().drag_anchor.set(Some((nx, ny)));
                // Keep the readout live under the moving cursor.
                self.ivars().cursor.set(Some(self.event_point(event)));
                self.request_render();
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &AnyObject) {
            self.ivars().drag_anchor.set(None);
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &AnyObject) {
            self.ivars().cursor.set(Some(self.event_point(event)));
            self.refresh_overlay_only();
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &AnyObject) {
            self.ivars().cursor.set(Some(self.event_point(event)));
            // Cursor motion changes only the readout — update the overlay
            // layers without re-presenting the (unchanged) GPU frame.
            self.refresh_overlay_only();
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &AnyObject) {
            self.ivars().cursor.set(None);
            self.refresh_overlay_only();
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &AnyObject) {
            let chars: Option<Retained<NSString>> =
                unsafe { msg_send![event, charactersIgnoringModifiers] };
            let handled = match chars.map(|s| s.to_string()) {
                Some(s) if s == "0" || s == "f" || s == "F" => {
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

impl FacetGpuView {
    /// Build the view. Main thread only (the marker proves it).
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let _ = mtm;
        let this: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        unsafe { msg_send![this, initWithFrame: frame] }
    }

    /// Store the frame's dimensions and readout buffers (the pixels
    /// themselves travel to the GPU through the vessel's renderer; this view
    /// keeps copies only for the dest math and the pixel readout).
    fn set_frame_data(&self, rgba: &[u8], linear: &[f32], width: u32, height: u32) {
        let expected = width as usize * height as usize * 4;
        *self.ivars().size.borrow_mut() = (width, height);
        *self.ivars().rgba.borrow_mut() = rgba[..expected].to_vec();
        let lin_expected = width as usize * height as usize * 3;
        if linear.len() >= lin_expected {
            *self.ivars().linear.borrow_mut() = linear[..lin_expected].to_vec();
        } else {
            self.ivars().linear.borrow_mut().clear();
        }
        self.ivars().zoom.set(1.0);
        self.ivars().pan.set((0.0, 0.0));
    }

    // -- interaction (verbatim FACET-2) ---------------------------------------

    /// Reset the zoom to aspect-fit and clear any pan.
    fn reset_to_fit(&self) {
        self.ivars().zoom.set(1.0);
        self.ivars().pan.set((0.0, 0.0));
        self.request_render();
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
        self.request_render();
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

    // -- the GPU seam ----------------------------------------------------------

    /// Keep the metal layer's `contentsScale` and `drawableSize` in lockstep
    /// with the live bounds and backing scale. Returns the drawable size in
    /// pixels.
    fn sync_layer_metrics(&self) -> (u32, u32) {
        let layer = self.ivars().metal_layer.borrow().clone();
        let Some(layer) = layer else {
            return (0, 0);
        };
        let scale: f64 = match self.window() {
            Some(w) => unsafe { msg_send![&w, backingScaleFactor] },
            None => unsafe { msg_send![&*layer, contentsScale] },
        };
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let bounds = self.bounds();
        let want = NSSize::new(
            (bounds.size.width * scale).round().max(1.0),
            (bounds.size.height * scale).round().max(1.0),
        );
        unsafe {
            let cur_scale: f64 = msg_send![&*layer, contentsScale];
            let cur: NSSize = msg_send![&*layer, drawableSize];
            if cur_scale != scale || cur.width != want.width || cur.height != want.height {
                let _: () = msg_send![class!(CATransaction), begin];
                let _: () = msg_send![class!(CATransaction), setDisableActions: true];
                let _: () = msg_send![&*layer, setContentsScale: scale];
                let _: () = msg_send![&*layer, setDrawableSize: want];
                let _: () = msg_send![class!(CATransaction), commit];
            }
        }
        (want.width as u32, want.height as u32)
    }

    /// The on-demand redraw: sync the layer metrics, hand the current dest
    /// rect to the vessel's renderer, then refresh the readout overlay.
    fn request_render(&self) {
        let drawable = self.sync_layer_metrics();
        let bounds = self.bounds();
        let dest = self.image_dest(bounds);
        if let (Some(dest), (1.., 1..)) = (dest, drawable) {
            if let Some(renderer) = self.ivars().renderer.borrow_mut().as_mut() {
                renderer(&GpuFrameParams {
                    dest,
                    bounds: (bounds.size.width, bounds.size.height),
                    drawable,
                });
            }
        }
        self.refresh_overlay(dest);
    }

    /// Update only the readout overlay (cursor motion over a static frame —
    /// the GPU image is unchanged, so no re-present).
    fn refresh_overlay_only(&self) {
        let bounds = self.bounds();
        let dest = self.image_dest(bounds);
        self.refresh_overlay(dest);
    }

    // -- the readout overlay (CALayer text over the metal layer) ---------------

    /// Map the current cursor to a source pixel and refresh the overlay
    /// layers — the same math as the CPU path's `draw_readout`, rendered as a
    /// `CATextLayer` over the metal layer instead of AppKit text in
    /// `drawRect:` (a layer-hosting view has no drawRect).
    fn refresh_overlay(&self, dest: Option<(f64, f64, f64, f64)>) {
        let lines = self.readout_lines(dest);
        match lines {
            Some(lines) => self.show_overlay(&lines),
            None => self.hide_overlay(),
        }
    }

    /// The readout text, or `None` when the cursor is off the image.
    /// Row math is verbatim FACET-2: unflipped view, `fy == 1` is the TOP of
    /// the drawn image = source row 0, so flip to a top-down row index.
    fn readout_lines(&self, dest: Option<(f64, f64, f64, f64)>) -> Option<Vec<String>> {
        let (cx, cy) = self.ivars().cursor.get()?;
        let (dx, dy, dw, dh) = dest?;
        if dw <= 0.0 || dh <= 0.0 {
            return None;
        }
        // Fraction across the drawn image; bail if the cursor is off it.
        let fx = (cx - dx) / dw;
        let fy = (cy - dy) / dh;
        if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
            return None;
        }
        let (iw, ih) = *self.ivars().size.borrow();
        if iw == 0 || ih == 0 {
            return None;
        }
        let sx = ((fx * iw as f64) as u32).min(iw - 1);
        let sy = (((1.0 - fy) * ih as f64) as u32).min(ih - 1);

        let idx = (sy as usize * iw as usize + sx as usize) * 4;
        let rgba = self.ivars().rgba.borrow();
        if idx + 3 >= rgba.len() {
            return None;
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
        Some(lines)
    }

    /// Lazily build the overlay layers (rounded translucent backing +
    /// text layer) as sublayers of the metal layer.
    fn ensure_overlay_layers(&self) -> Option<(Retained<AnyObject>, Retained<AnyObject>)> {
        if let (Some(bg), Some(text)) = (
            self.ivars().overlay_bg.borrow().clone(),
            self.ivars().overlay_text.borrow().clone(),
        ) {
            return Some((bg, text));
        }
        let metal = self.ivars().metal_layer.borrow().clone()?;
        unsafe {
            let bg: *mut AnyObject = msg_send![class!(CALayer), new];
            let bg = Retained::from_raw(bg)?;
            let _: () = msg_send![&*bg, setCornerRadius: 6.0f64];
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(
                OVERLAY_BG.0,
                OVERLAY_BG.1,
                OVERLAY_BG.2,
                OVERLAY_BG.3,
            );
            let cg: *mut c_void = msg_send![&*color, CGColor];
            if !cg.is_null() {
                let _: () = msg_send![&*bg, setBackgroundColor: cg];
            }

            let text: *mut AnyObject = msg_send![class!(CATextLayer), new];
            let text = Retained::from_raw(text)?;
            let _: () = msg_send![&*text, setWrapped: false];

            let _: () = msg_send![&*metal, addSublayer: &*bg];
            let _: () = msg_send![&*bg, addSublayer: &*text];

            *self.ivars().overlay_bg.borrow_mut() = Some(bg.clone());
            *self.ivars().overlay_text.borrow_mut() = Some(text.clone());
            Some((bg, text))
        }
    }

    /// Lay the panel out in the lower-left (same margins, pad, line height,
    /// font, and palette as the CPU overlay) and set the text.
    fn show_overlay(&self, lines: &[String]) {
        let Some((bg, text)) = self.ensure_overlay_layers() else {
            return;
        };
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
        let bounds = self.bounds();
        let panel = NSRect::new(
            NSPoint::new(bounds.origin.x + margin, bounds.origin.y + margin),
            NSSize::new(panel_w, panel_h),
        );
        let text_frame = NSRect::new(
            NSPoint::new(pad, pad),
            NSSize::new(text_w, panel_h - pad * 2.0),
        );

        let joined = lines.join("\n");
        let ns = NSString::from_str(&joined);
        unsafe {
            // The attributed string carries font + color, so CATextLayer's
            // typesetting matches the sizeWithAttributes measurement above.
            let astr: *mut AnyObject = msg_send![class!(NSAttributedString), alloc];
            let astr: *mut AnyObject =
                msg_send![astr, initWithString: &*ns, attributes: &*attrs];
            let Some(astr) = Retained::from_raw(astr) else {
                return;
            };

            let scale: f64 = match self.window() {
                Some(w) => msg_send![&w, backingScaleFactor],
                None => 1.0,
            };
            let scale = if scale > 0.0 { scale } else { 1.0 };

            let _: () = msg_send![class!(CATransaction), begin];
            let _: () = msg_send![class!(CATransaction), setDisableActions: true];
            let _: () = msg_send![&*bg, setFrame: panel];
            let _: () = msg_send![&*bg, setHidden: false];
            let _: () = msg_send![&*text, setFrame: text_frame];
            let _: () = msg_send![&*text, setContentsScale: scale];
            let _: () = msg_send![&*text, setString: &*astr];
            let _: () = msg_send![class!(CATransaction), commit];
        }
    }

    fn hide_overlay(&self) {
        if let Some(bg) = self.ivars().overlay_bg.borrow().clone() {
            unsafe {
                let _: () = msg_send![class!(CATransaction), begin];
                let _: () = msg_send![class!(CATransaction), setDisableActions: true];
                let _: () = msg_send![&*bg, setHidden: true];
                let _: () = msg_send![class!(CATransaction), commit];
            }
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

/// Build the GPU image view for a `facet` vessel window.
///
/// Mirrors [`super::image_view::bootstrap_image_view`]'s seam shape, plus the
/// GPU inversion: after the `CAMetalLayer` exists, `make_renderer` is called
/// (on the main thread, exactly once) with the raw layer pointer and the
/// initial drawable size in pixels. The vessel builds the whole wgpu side
/// there — surface over the layer, device, textured quad, frame upload — and
/// returns the render closure; returning `None` (GPU init failed) makes this
/// bootstrap return `None` so the vessel can fall back to the CPU view.
///
/// The layer pointer is valid for the duration of `make_renderer` AND for the
/// life of the returned closure: the view retains the layer as its hosted
/// backing layer, and the closure is stored in the view's ivars, which drop
/// before the layer is released (see the module doc). Main thread only.
pub fn bootstrap_gpu_view<F>(
    window: &NSWindow,
    rgba: &[u8],
    linear: &[f32],
    width: u32,
    height: u32,
    make_renderer: F,
) -> Option<Retained<NSView>>
where
    F: FnOnce(*mut c_void, u32, u32) -> Option<GpuRenderer>,
{
    let mtm = MainThreadMarker::new().expect("bootstrap_gpu_view must run on the main thread");

    let expected = width as usize * height as usize * 4;
    if width == 0 || height == 0 || rgba.len() < expected {
        log::warn!(
            "[FACET] bootstrap_gpu_view: bad buffer ({} bytes for {}x{}, need {})",
            rgba.len(),
            width,
            height,
            expected
        );
        return None;
    }

    // Open at the image's native size, sensibly capped, exactly like the CPU
    // path; the view rescales from the live bounds after.
    let init_w = (width as f64).clamp(240.0, 1600.0);
    let init_h = (height as f64).clamp(160.0, 1000.0);
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(init_w, init_h));

    let view = FacetGpuView::new(mtm, frame);

    // The CAMetalLayer, via class!/msg_send! — quartzite's Cargo.toml stays
    // zero-diff (no objc2-quartz-core edge).
    let layer: Retained<AnyObject> = unsafe {
        let l: *mut AnyObject = msg_send![class!(CAMetalLayer), new];
        match Retained::from_raw(l) {
            Some(l) => l,
            None => {
                log::warn!("[FACET] bootstrap_gpu_view: CAMetalLayer alloc failed (no Metal?)");
                return None;
            }
        }
    };

    let scale: f64 = unsafe { msg_send![window, backingScaleFactor] };
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let dw = (init_w * scale).round().max(1.0);
    let dh = (init_h * scale).round().max(1.0);
    unsafe {
        let _: () = msg_send![&*layer, setContentsScale: scale];
        let _: () = msg_send![&*layer, setDrawableSize: NSSize::new(dw, dh)];

        // Pin the layer to sRGB: euclase renders into an sRGB swapchain, and
        // tagging the layer makes the window server present those bytes the
        // same way AppKit presents the CPU path's sRGB-tagged bitmap on a P3
        // display — the eye-witness color axis.
        let srgb_ns: *mut AnyObject = msg_send![class!(NSColorSpace), sRGBColorSpace];
        if !srgb_ns.is_null() {
            let cg: *mut c_void = msg_send![srgb_ns, CGColorSpace];
            if !cg.is_null() {
                let _: () = msg_send![&*layer, setColorspace: cg];
            }
        }

        // Layer-HOSTING: setLayer: before setWantsLayer: (the AppKit contract
        // for supplying your own layer).
        let _: () = msg_send![&view, setLayer: &*layer];
        let _: () = msg_send![&view, setWantsLayer: true];
    }

    view.set_frame_data(rgba, linear, width, height);

    // Hand the vessel the layer; get back the renderer (or bail to CPU).
    let ptr = Retained::as_ptr(&layer) as *mut AnyObject as *mut c_void;
    let renderer = make_renderer(ptr, dw as u32, dh as u32)?;

    *view.ivars().renderer.borrow_mut() = Some(renderer);
    *view.ivars().metal_layer.borrow_mut() = Some(layer);
    view.request_render();

    Some(Retained::into_super(view))
}
