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
//! (Moonstone field, lilac/purple segments, dim empty segments for idle
//! cores). The view renders whatever loads it is fed via
//! [`SegmentMeterView::set_loads`]; [`bootstrap_meter`] wires it to the
//! Synapse so the vessel that owns the window never touches AppKit.
//!
//! Layout is derived from the live view bounds on every draw — there is no
//! hardcoded pixel geometry, so the row scales with the window.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use dispatch2::MainThreadBound;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSView,
    NSWindow,
};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString,
};

use bandy::SMessage;

// ---------------------------------------------------------------------------
// PALETTE — mirrors the kernel's vug meter palette (crates/kernel/src/vug.rs)
// ---------------------------------------------------------------------------
/// The UnaOS console field color.
pub const MOONSTONE: u32 = 0x2D2B55;
/// An unlit segment: alive-but-empty, barely off the field.
const DIM: u32 = 0x2A2432;
/// The bright end of the meter ramp.
const LILAC: u32 = 0xB36BFF;
/// The deep end of the meter ramp.
const PURPLE: u32 = 0x9B59B6;
/// Legends and core numbers.
pub(crate) const LABEL: u32 = 0x8A8296;

/// Segments per core bar (the LED-ladder resolution; widths scale with the window).
const SEGMENTS: usize = 6;

fn channels(rgb: u32) -> (f64, f64, f64) {
    (
        ((rgb >> 16) & 0xFF) as f64 / 255.0,
        ((rgb >> 8) & 0xFF) as f64 / 255.0,
        (rgb & 0xFF) as f64 / 255.0,
    )
}

pub(crate) fn ns_color(rgb: u32) -> Retained<NSColor> {
    let (r, g, b) = channels(rgb);
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0)
}

/// Linear blend between two palette colors, `t` in `0.0..=1.0`.
fn ns_color_lerp(from: u32, to: u32, t: f64) -> Retained<NSColor> {
    let (r0, g0, b0) = channels(from);
    let (r1, g1, b1) = channels(to);
    NSColor::colorWithSRGBRed_green_blue_alpha(
        r0 + (r1 - r0) * t,
        g0 + (g1 - g0) * t,
        b0 + (b1 - b0) * t,
        1.0,
    )
}

// ---------------------------------------------------------------------------
// THE VIEW
// ---------------------------------------------------------------------------
pub struct SegmentMeterIvars {
    /// Per-core load fractions in `0.0..=1.0`, one entry per core.
    loads: RefCell<Vec<f32>>,
    /// The row legend drawn once at the left ("CPU" for pulse; e.g. "LVL"
    /// for an audio level bar).
    legend: RefCell<String>,
    /// Whether each cell carries its 1-based index (per-core bars want it;
    /// a single level bar does not).
    numbered: Cell<bool>,
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
                legend: RefCell::new("CPU".to_string()),
                numbered: Cell::new(true),
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

    /// [`Self::new`] with a custom legend and numbering choice. The defaults
    /// (`"CPU"`, numbered) are pulse's per-core row; the phonolite level bar
    /// uses `("LVL", false)`. Main thread only.
    pub fn new_styled(
        mtm: MainThreadMarker,
        frame: NSRect,
        legend: &str,
        numbered: bool,
    ) -> Retained<Self> {
        let this = Self::new(mtm, frame);
        *this.ivars().legend.borrow_mut() = legend.to_string();
        this.ivars().numbered.set(numbered);
        this
    }

    /// Feed the meter a fresh set of per-core loads (`0.0..=1.0` each) and
    /// schedule a redraw. Main thread only.
    pub fn set_loads(&self, loads: Vec<f32>) {
        *self.ivars().loads.borrow_mut() = loads;
        self.setNeedsDisplay(true);
    }

    /// Write a pixel-true PNG of the meter as it currently renders. The view
    /// snapshots itself through AppKit's display cache, so this needs no
    /// screen-recording entitlement. Main thread only.
    pub fn write_snapshot(&self, path: &str) {
        let rect = self.bounds();
        let Some(rep) = self.bitmapImageRepForCachingDisplayInRect(rect) else {
            log::warn!("[METER] snapshot: no bitmap rep for {rect:?}");
            return;
        };
        self.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
        let props = NSDictionary::new();
        let Some(data) = (unsafe {
            rep.representationUsingType_properties(
                objc2_app_kit::NSBitmapImageFileType::PNG,
                &props,
            )
        }) else {
            log::warn!("[METER] snapshot: PNG encode failed");
            return;
        };
        let ok = data.writeToFile_atomically(&NSString::from_str(path), true);
        log::info!("[METER] snapshot -> {path} (ok={ok})");
    }

    // -- drawing ------------------------------------------------------------

    fn render(&self) {
        let bounds = self.bounds();
        let (w, h) = (bounds.size.width, bounds.size.height);

        // The field.
        ns_color(MOONSTONE).set();
        objc2_app_kit::NSRectFill(bounds);

        let loads = self.ivars().loads.borrow().clone();

        // Everything below derives from the live bounds.
        let font_size = (h * 0.30).clamp(9.0, 17.0);
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(font_size, unsafe {
            objc2_app_kit::NSFontWeightMedium
        });
        let label_attrs = text_attrs(&font, LABEL);

        let margin = (w * 0.02).max(8.0);
        let mid_y = h / 2.0;

        // The row legend, once at the left: `CPU 1 ▮▮▮▯ 2 ▮▯▯▯ …`
        let legend = NSString::from_str(&self.ivars().legend.borrow());
        let legend_size = measure(&legend, &label_attrs);
        draw_text(
            &legend,
            NSPoint::new(margin, mid_y - legend_size.height / 2.0),
            &label_attrs,
        );

        if loads.is_empty() {
            // No beat yet: the legend alone on the field, first sample lands
            // within one cadence tick.
            return;
        }

        // Per-core cells split the remaining width evenly.
        let row_x = margin + legend_size.width + legend_size.width * 0.5;
        let avail_w = (w - margin - row_x).max(1.0);
        let n = loads.len();
        let cell_w = avail_w / n as f64;
        let cell_gap = cell_w * 0.10;

        // Widest core number ("12"-class) sets the number column inside a
        // cell (an unnumbered meter spends that column on the bar instead).
        let numbered = self.ivars().numbered.get();
        let widest = NSString::from_str(&n.to_string());
        let num_w = if numbered {
            measure(&widest, &label_attrs).width
        } else {
            0.0
        };

        let bar_h = (h * 0.42).max(4.0);

        for (i, load) in loads.iter().enumerate() {
            let cell_x = row_x + cell_w * i as f64;

            // The core number, right-aligned in its column, 1-based.
            if numbered {
                let num = NSString::from_str(&(i + 1).to_string());
                let num_size = measure(&num, &label_attrs);
                draw_text(
                    &num,
                    NSPoint::new(
                        cell_x + num_w - num_size.width,
                        mid_y - num_size.height / 2.0,
                    ),
                    &label_attrs,
                );
            }

            // The segment ladder: chunky LED blocks, taller than wide, never
            // barcode stripes — the block proportions derive from the pitch.
            let bar_x = cell_x + num_w + num_w * 0.45;
            let bar_w = (cell_x + cell_w - cell_gap - bar_x).max(1.0);
            let seg_pitch = bar_w / SEGMENTS as f64;
            let seg_w = (seg_pitch * 0.72).max(1.0);
            let seg_h = bar_h.min(seg_w * 2.2).max(4.0);
            let seg_y = mid_y - seg_h / 2.0;
            let radius = (seg_w.min(seg_h) * 0.22).min(3.0);

            let lit = ((*load as f64) * SEGMENTS as f64).round() as usize;
            let lit = lit.min(SEGMENTS);

            for s in 0..SEGMENTS {
                let seg_rect = NSRect::new(
                    NSPoint::new(bar_x + seg_pitch * s as f64, seg_y),
                    NSSize::new(seg_w, seg_h),
                );
                if s < lit {
                    // The lilac/purple ramp: deep at the base, bright at the peak.
                    let t = if SEGMENTS > 1 {
                        s as f64 / (SEGMENTS - 1) as f64
                    } else {
                        0.0
                    };
                    ns_color_lerp(PURPLE, LILAC, t).set();
                } else {
                    // Alive but empty: the dim segment keeps idle cores visible.
                    ns_color(DIM).set();
                }
                let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    seg_rect, radius, radius,
                );
                path.fill();
            }
        }
    }
}

// -- text helpers -----------------------------------------------------------

type TextAttrs = Retained<NSDictionary<NSString, AnyObject>>;

fn text_attrs(font: &NSFont, color: u32) -> TextAttrs {
    let color = ns_color(color);
    let keys: [&NSString; 2] = [
        unsafe { NSFontAttributeName },
        unsafe { NSForegroundColorAttributeName },
    ];
    let values: [&AnyObject; 2] = [font.as_ref(), color.as_ref()];
    NSDictionary::from_slices(&keys, &values)
}

fn measure(text: &NSString, attrs: &TextAttrs) -> NSSize {
    unsafe { msg_send![text, sizeWithAttributes: &**attrs] }
}

fn draw_text(text: &NSString, at: NSPoint, attrs: &TextAttrs) {
    let _: () = unsafe { msg_send![text, drawAtPoint: at, withAttributes: &**attrs] };
}

// ---------------------------------------------------------------------------
// BOOTSTRAP — the bus-facing seam
// ---------------------------------------------------------------------------

/// Build the segment meter view for a vessel window and wire it to the Synapse.
///
/// The vessel hands over a broadcast receiver; the meter listens for
/// [`SMessage::CorePulse`] beats on it and repaints on the main thread (the
/// same `MainThreadBound` + main-queue dispatch bridge the workspace spline
/// uses). Returns the view to install as the window's content view.
pub fn bootstrap_meter(
    _window: &NSWindow,
    mut rx_synapse: tokio::sync::broadcast::Receiver<SMessage>,
) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_meter must run on the main thread");

    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 96.0));
    let view = SegmentMeterView::new(mtm, frame);

    let bound = Arc::new(MainThreadBound::new(view.clone(), mtm));

    // Dev affordance: `UNAOS_METER_SNAPSHOT=/path.png` writes a pixel-true PNG
    // of the meter (the view snapshots itself — no screen-recording permission
    // involved) after `UNAOS_METER_SNAPSHOT_DELAY_MS` (default 1500) — long
    // enough for the first real beats to land. Used for landing-report
    // screenshots and headless visual checks.
    if let Ok(snap_path) = std::env::var("UNAOS_METER_SNAPSHOT") {
        let delay_ms = std::env::var("UNAOS_METER_SNAPSHOT_DELAY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1500);
        let snap_bound = bound.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            dispatch2::DispatchQueue::main().exec_async(move || {
                let mtm = MainThreadMarker::new().unwrap();
                snap_bound.get(mtm).write_snapshot(&snap_path);
            });
        });
    }

    tokio::spawn(async move {
        loop {
            match rx_synapse.recv().await {
                Ok(SMessage::CorePulse { loads }) => {
                    let bound = bound.clone();
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        let mtm = MainThreadMarker::new().unwrap();
                        bound.get(mtm).set_loads(loads);
                    });
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    Retained::into_super(view)
}
