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
//! `NSBitmapImageRep` and draws it aspect-fit, centered, on a dark field —
//! the smallest honest "it shows the picture" seam. The pixels are prepared
//! by the vessel (`facet` packs `lux`'s linear `RgbBuffer` through the sRGB
//! OETF); this view is a dumb blitter and owns no color math.
//!
//! Layout is derived from the live view bounds on every draw, so the image
//! rescales with the window. The euclase textured-quad (GPU) path is the
//! later promotion; this is the CPU-blit MVP.

use std::cell::RefCell;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{NSBitmapImageRep, NSImageRep, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

/// The console field the picture sits on (UnaOS Moonstone; matches the meter).
const FIELD: (f64, f64, f64) = (0x2D as f64 / 255.0, 0x2B as f64 / 255.0, 0x55 as f64 / 255.0);

// ---------------------------------------------------------------------------
// THE VIEW
// ---------------------------------------------------------------------------
pub struct FacetImageIvars {
    /// The decoded frame as an AppKit bitmap rep (8-bit sRGB RGBA), or `None`
    /// until one is set. Drawn aspect-fit on every `drawRect:`.
    rep: RefCell<Option<Retained<NSBitmapImageRep>>>,
    /// Pixel dimensions of the current frame (for aspect-fit math).
    size: RefCell<(u32, u32)>,
}

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
        // attended eye-witness). The aspect-fit math in `render` is pure
        // centering from the live bounds — orientation-independent — so the
        // default (unflipped) coordinate system is both correct and simplest.

        // Any resize re-derives the aspect-fit rect from the live bounds.
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: new_size] };
            self.setNeedsDisplay(true);
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
    /// (`width * height * 4` bytes, row-major, top row first, opaque alpha).
    /// The bytes are copied into an AppKit-owned bitmap rep and tagged sRGB so
    /// the display is color-managed. Main thread only.
    pub fn set_frame(&self, rgba: &[u8], width: u32, height: u32) {
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
        self.setNeedsDisplay(true);
    }

    // -- drawing ------------------------------------------------------------

    fn render(&self) {
        let bounds = self.bounds();

        // The field.
        let field = objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(
            FIELD.0, FIELD.1, FIELD.2, 1.0,
        );
        field.set();
        objc2_app_kit::NSRectFill(bounds);

        let rep_guard = self.ivars().rep.borrow();
        let Some(rep) = rep_guard.as_ref() else {
            return; // No frame yet.
        };
        let (iw, ih) = *self.ivars().size.borrow();
        if iw == 0 || ih == 0 {
            return;
        }

        // Aspect-fit the image inside the bounds, centered.
        let (bw, bh) = (bounds.size.width, bounds.size.height);
        let scale = (bw / iw as f64).min(bh / ih as f64).min(1e9);
        let dw = iw as f64 * scale;
        let dh = ih as f64 * scale;
        let dx = bounds.origin.x + (bw - dw) / 2.0;
        let dy = bounds.origin.y + (bh - dh) / 2.0;
        let dest = NSRect::new(NSPoint::new(dx, dy), NSSize::new(dw.max(1.0), dh.max(1.0)));

        let _: bool = unsafe {
            let rep_ref: &NSImageRep = rep.as_ref();
            msg_send![rep_ref, drawInRect: dest]
        };
    }
}

// ---------------------------------------------------------------------------
// BOOTSTRAP — the vessel-facing seam
// ---------------------------------------------------------------------------

/// Build the image view for a `facet` vessel window and load one frame.
///
/// The vessel decodes and packs the picture (`lux` linear `RgbBuffer` → 8-bit
/// sRGB RGBA) and hands the bytes here; the view copies them into an AppKit
/// bitmap rep and draws it aspect-fit. Returns the view to install as the
/// window's content view. Main thread only.
pub fn bootstrap_image_view(
    _window: &objc2_app_kit::NSWindow,
    rgba: &[u8],
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
    view.set_frame(rgba, width, height);

    Retained::into_super(view)
}
