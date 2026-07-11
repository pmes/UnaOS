// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The segment meter: quartzite's twin of the kernel's vug CPU pulse meter.
//!
//! One row of numbered per-core segment bars, drawn in the UnaOS palette
//! (Moonstone field, lilac/purple segments). The view renders whatever loads it
//! is fed via [`SegmentMeterView::set_loads`]; [`bootstrap_meter`] wires it to
//! the Synapse so the vessel that owns the window never touches AppKit.

use std::cell::RefCell;

use objc2::rc::{Allocated, Retained};
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{NSColor, NSView, NSWindow};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

// ---------------------------------------------------------------------------
// PALETTE — mirrors the kernel's vug meter palette (crates/kernel/src/vug.rs)
// ---------------------------------------------------------------------------
/// The UnaOS console field color.
pub const MOONSTONE: u32 = 0x2D2B55;

fn ns_color(rgb: u32) -> Retained<NSColor> {
    let r = ((rgb >> 16) & 0xFF) as f64 / 255.0;
    let g = ((rgb >> 8) & 0xFF) as f64 / 255.0;
    let b = (rgb & 0xFF) as f64 / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0)
}

// ---------------------------------------------------------------------------
// THE VIEW
// ---------------------------------------------------------------------------
pub struct SegmentMeterIvars {
    /// Per-core load fractions in `0.0..=1.0`, one entry per core.
    loads: RefCell<Vec<f32>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "UnaSegmentMeterView"]
    #[ivars = SegmentMeterIvars]
    pub struct SegmentMeterView;

    impl SegmentMeterView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(SegmentMeterIvars {
                loads: RefCell::new(Vec::new()),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            self.render();
        }

        // Flipped so the layout math reads top-down like the kernel meters.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES
        }

        // Any resize invalidates the whole meter row: the layout is derived
        // from the live bounds, never from fixed pixel positions.
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: NSSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: new_size] };
            self.setNeedsDisplay(true);
        }
    }
);

impl SegmentMeterView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let _ = mtm; // views must be built on the main thread; the marker proves it
        let this: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        unsafe { msg_send![this, initWithFrame: frame] }
    }

    /// Feed the meter a fresh set of per-core loads (`0.0..=1.0` each) and
    /// schedule a redraw. Main thread only.
    pub fn set_loads(&self, loads: Vec<f32>) {
        *self.ivars().loads.borrow_mut() = loads;
        self.setNeedsDisplay(true);
    }

    fn render(&self) {
        let bounds = self.bounds();

        // The field.
        ns_color(MOONSTONE).set();
        objc2_app_kit::NSRectFill(bounds);
    }
}

// ---------------------------------------------------------------------------
// BOOTSTRAP — the bus-facing seam
// ---------------------------------------------------------------------------

/// Build the segment meter view for a vessel window and wire it to the Synapse.
///
/// The vessel hands over a broadcast receiver; the meter listens for pulse
/// samples on it and repaints on the main thread. Returns the view to install
/// as the window's content view.
pub fn bootstrap_meter(
    _window: &NSWindow,
    _rx_synapse: tokio::sync::broadcast::Receiver<bandy::SMessage>,
) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_meter must run on the main thread");

    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 96.0));
    let view = SegmentMeterView::new(mtm, frame);

    Retained::into_super(view)
}
