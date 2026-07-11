// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The tone panel: quartzite's first *input*-control surface.
//!
//! Everything before this file only displayed (the meter, the spline); this
//! panel sets the idiom for controls flowing the other way: AppKit
//! target-action into a `define_class!` view whose ivars hold plain Rust
//! callbacks, so the vessel that owns the window never touches AppKit — it
//! hands over closures and receives values.
//!
//! Layout: a Start/Stop button, a log-scale frequency slider, a linear gain
//! slider (each with a live value label), and a [`SegmentMeterView`] level
//! bar. The panel listens for [`SMessage::Spectrum`] beats on the Synapse
//! and paints `magnitude[0]` as the level — the same bus→main-thread bridge
//! idiom as [`super::meter::bootstrap_meter`]. Nothing here ever runs on the
//! audio thread.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use dispatch2::MainThreadBound;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{ClassType, DefinedClass, define_class, msg_send};
use objc2_app_kit::{NSButton, NSSlider, NSTextField, NSView, NSWindow};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSPoint, NSRect, NSSize, NSString};

use bandy::SMessage;

use super::meter::{LABEL, MOONSTONE, SegmentMeterView, ns_color};

/// The panel's designed content size (width, height). Pass this to
/// `Backend::new_vessel` so the window opens exactly around the layout;
/// autoresizing masks keep the rows sensible if the user resizes.
pub const PANEL_SIZE: (f64, f64) = (460.0, 224.0);

// -- autoresizing masks (AppKit NSAutoresizingMaskOptions bits) --------------
const MASK_FLEX_LEFT: usize = 1; // NSViewMinXMargin
const MASK_FLEX_WIDTH: usize = 2; // NSViewWidthSizable
const MASK_FLEX_BELOW: usize = 8; // NSViewMinYMargin (pin to top)
const MASK_FLEX_ABOVE: usize = 32; // NSViewMaxYMargin (pin to bottom)

// ---------------------------------------------------------------------------
// THE VESSEL-FACING CONTRACT
// ---------------------------------------------------------------------------

/// Static shape of the tone controls: ranges and initial values.
#[derive(Debug, Clone, Copy)]
pub struct ToneControlConfig {
    /// Bottom of the (log-scale) frequency slider, Hz.
    pub freq_min_hz: f64,
    /// Top of the (log-scale) frequency slider, Hz.
    pub freq_max_hz: f64,
    /// Where the frequency slider starts, Hz.
    pub initial_freq_hz: f64,
    /// Top of the (linear) gain slider; bottom is 0.
    pub gain_max: f64,
    /// Where the gain slider starts.
    pub initial_gain: f64,
    /// Whether the tone is already sounding when the panel appears (sets the
    /// button's initial title).
    pub initially_running: bool,
}

/// What the vessel wants to hear from the controls. All callbacks run on the
/// main thread, from AppKit target-action — never from the audio callback.
pub struct ToneCallbacks {
    /// Start/Stop clicked; the bool is the *new* running state.
    pub on_running_changed: Box<dyn FnMut(bool)>,
    /// Frequency slider moved; value already mapped to Hz.
    pub on_frequency_hz: Box<dyn FnMut(f64)>,
    /// Gain slider moved; value in `0.0..=gain_max`.
    pub on_gain: Box<dyn FnMut(f64)>,
}

// ---------------------------------------------------------------------------
// SLIDER MAPPING — pure, unit-tested
// ---------------------------------------------------------------------------

/// Map a slider position `t` in `0.0..=1.0` to Hz on a log scale over
/// `min_hz..=max_hz` (equal slider distance = equal musical interval).
pub fn slider_to_hz(t: f64, min_hz: f64, max_hz: f64) -> f64 {
    min_hz * (max_hz / min_hz).powf(t.clamp(0.0, 1.0))
}

/// Inverse of [`slider_to_hz`]: where on the slider a frequency sits.
pub fn hz_to_slider(hz: f64, min_hz: f64, max_hz: f64) -> f64 {
    ((hz / min_hz).ln() / (max_hz / min_hz).ln()).clamp(0.0, 1.0)
}

fn format_hz(hz: f64) -> String {
    format!("{hz:.0} Hz")
}

fn format_gain(gain: f64) -> String {
    format!("{gain:.2}")
}

// ---------------------------------------------------------------------------
// THE PANEL VIEW
// ---------------------------------------------------------------------------

pub struct TonePanelIvars {
    callbacks: RefCell<Option<ToneCallbacks>>,
    running: Cell<bool>,
    freq_min_hz: Cell<f64>,
    freq_max_hz: Cell<f64>,

    // Subview handles the action methods update. (The view hierarchy also
    // retains them; these are the typed references.)
    button: RefCell<Option<Retained<NSButton>>>,
    freq_value: RefCell<Option<Retained<NSTextField>>>,
    gain_value: RefCell<Option<Retained<NSTextField>>>,
    meter: RefCell<Option<Retained<SegmentMeterView>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "UnaTonePanelView"]
    #[ivars = TonePanelIvars]
    pub struct TonePanelView;

    impl TonePanelView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(TonePanelIvars {
                callbacks: RefCell::new(None),
                running: Cell::new(false),
                freq_min_hz: Cell::new(55.0),
                freq_max_hz: Cell::new(1760.0),
                button: RefCell::new(None),
                freq_value: RefCell::new(None),
                gain_value: RefCell::new(None),
                meter: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        // The field behind the controls: the UnaOS Moonstone, same as the meter.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            ns_color(MOONSTONE).set();
            objc2_app_kit::NSRectFill(self.bounds());
        }

        // --- target-action landing pads (the input-control idiom) ----------

        #[unsafe(method(toggleTone:))]
        fn toggle_tone(&self, _sender: &AnyObject) {
            let now_running = !self.ivars().running.get();
            self.ivars().running.set(now_running);
            if let Some(button) = self.ivars().button.borrow().as_ref() {
                let title = NSString::from_str(if now_running { "Stop" } else { "Start" });
                let _: () = unsafe { msg_send![&**button, setTitle: &*title] };
            }
            if let Some(cbs) = self.ivars().callbacks.borrow_mut().as_mut() {
                (cbs.on_running_changed)(now_running);
            }
        }

        #[unsafe(method(frequencyChanged:))]
        fn frequency_changed(&self, sender: &AnyObject) {
            let t: f64 = unsafe { msg_send![sender, doubleValue] };
            let hz = slider_to_hz(
                t,
                self.ivars().freq_min_hz.get(),
                self.ivars().freq_max_hz.get(),
            );
            if let Some(label) = self.ivars().freq_value.borrow().as_ref() {
                let text = NSString::from_str(&format_hz(hz));
                let _: () = unsafe { msg_send![&**label, setStringValue: &*text] };
            }
            if let Some(cbs) = self.ivars().callbacks.borrow_mut().as_mut() {
                (cbs.on_frequency_hz)(hz);
            }
        }

        #[unsafe(method(gainChanged:))]
        fn gain_changed(&self, sender: &AnyObject) {
            // The gain slider is linear: doubleValue IS the gain.
            let gain: f64 = unsafe { msg_send![sender, doubleValue] };
            if let Some(label) = self.ivars().gain_value.borrow().as_ref() {
                let text = NSString::from_str(&format_gain(gain));
                let _: () = unsafe { msg_send![&**label, setStringValue: &*text] };
            }
            if let Some(cbs) = self.ivars().callbacks.borrow_mut().as_mut() {
                (cbs.on_gain)(gain);
            }
        }
    }
);

impl TonePanelView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let _ = mtm; // views must be built on the main thread; the marker proves it
        let this: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        unsafe { msg_send![this, initWithFrame: frame] }
    }

    /// Feed the level bar a fresh `0.0..=1.0` value. Main thread only.
    fn set_level(&self, level: f32) {
        if let Some(meter) = self.ivars().meter.borrow().as_ref() {
            meter.set_loads(vec![level]);
        }
    }

    /// Build the rows. Runs once, on the main thread, before callbacks are
    /// installed (programmatic value-setting fires no actions anyway).
    fn build_controls(&self, mtm: MainThreadMarker, config: &ToneControlConfig) {
        self.ivars().running.set(config.initially_running);
        self.ivars().freq_min_hz.set(config.freq_min_hz);
        self.ivars().freq_max_hz.set(config.freq_max_hz);

        // -- Start/Stop button (chat_manager target-action idiom) ----------
        let button_frame = NSRect::new(NSPoint::new(16.0, 164.0), NSSize::new(120.0, 32.0));
        let button: Allocated<NSButton> = unsafe { msg_send![NSButton::class(), alloc] };
        let button: Retained<NSButton> = unsafe { msg_send![button, initWithFrame: button_frame] };
        unsafe {
            let title =
                NSString::from_str(if config.initially_running { "Stop" } else { "Start" });
            let _: () = msg_send![&button, setTitle: &*title];
            let _: () = msg_send![&button, setButtonType: 7isize]; // MomentaryPushIn
            let _: () = msg_send![&button, setBezelStyle: 1isize]; // Rounded
            let _: () = msg_send![&button, setAutoresizingMask: MASK_FLEX_BELOW];
            let _: () = msg_send![&button, setTarget: self];
            let _: () = msg_send![&button, setAction: objc2::sel!(toggleTone:)];
        }
        self.addSubview(&button);
        *self.ivars().button.borrow_mut() = Some(button);

        // -- frequency row --------------------------------------------------
        let freq_t = hz_to_slider(config.initial_freq_hz, config.freq_min_hz, config.freq_max_hz);
        self.add_label("freq", NSRect::new(NSPoint::new(16.0, 128.0), NSSize::new(44.0, 20.0)), MASK_FLEX_BELOW);
        let freq_slider = self.add_slider(
            NSRect::new(NSPoint::new(68.0, 124.0), NSSize::new(280.0, 24.0)),
            0.0,
            1.0,
            freq_t,
            objc2::sel!(frequencyChanged:),
        );
        let _ = freq_slider;
        let freq_value = self.add_label(
            &format_hz(config.initial_freq_hz),
            NSRect::new(NSPoint::new(356.0, 128.0), NSSize::new(88.0, 20.0)),
            MASK_FLEX_BELOW | MASK_FLEX_LEFT,
        );
        *self.ivars().freq_value.borrow_mut() = Some(freq_value);

        // -- gain row ---------------------------------------------------------
        self.add_label("gain", NSRect::new(NSPoint::new(16.0, 92.0), NSSize::new(44.0, 20.0)), MASK_FLEX_BELOW);
        let gain_slider = self.add_slider(
            NSRect::new(NSPoint::new(68.0, 88.0), NSSize::new(280.0, 24.0)),
            0.0,
            config.gain_max,
            config.initial_gain,
            objc2::sel!(gainChanged:),
        );
        let _ = gain_slider;
        let gain_value = self.add_label(
            &format_gain(config.initial_gain),
            NSRect::new(NSPoint::new(356.0, 92.0), NSSize::new(88.0, 20.0)),
            MASK_FLEX_BELOW | MASK_FLEX_LEFT,
        );
        *self.ivars().gain_value.borrow_mut() = Some(gain_value);

        // -- level bar (the meter, restyled) ----------------------------------
        let meter_frame = NSRect::new(NSPoint::new(16.0, 16.0), NSSize::new(428.0, 56.0));
        let meter = SegmentMeterView::new_styled(mtm, meter_frame, "LVL", false);
        unsafe {
            let _: () =
                msg_send![&*meter, setAutoresizingMask: MASK_FLEX_WIDTH | MASK_FLEX_ABOVE];
        }
        self.addSubview(&meter);
        *self.ivars().meter.borrow_mut() = Some(meter);
    }

    fn add_slider(
        &self,
        frame: NSRect,
        min: f64,
        max: f64,
        value: f64,
        action: objc2::runtime::Sel,
    ) -> Retained<NSSlider> {
        let slider: Allocated<NSSlider> = unsafe { msg_send![NSSlider::class(), alloc] };
        let slider: Retained<NSSlider> = unsafe { msg_send![slider, initWithFrame: frame] };
        unsafe {
            let _: () = msg_send![&slider, setMinValue: min];
            let _: () = msg_send![&slider, setMaxValue: max];
            let _: () = msg_send![&slider, setDoubleValue: value];
            let _: () = msg_send![&slider, setContinuous: objc2::runtime::Bool::YES];
            let _: () = msg_send![&slider, setAutoresizingMask: MASK_FLEX_WIDTH | MASK_FLEX_BELOW];
            let _: () = msg_send![&slider, setTarget: self];
            let _: () = msg_send![&slider, setAction: action];
        }
        self.addSubview(&slider);
        slider
    }

    fn add_label(&self, text: &str, frame: NSRect, mask: usize) -> Retained<NSTextField> {
        let label: Allocated<NSTextField> = unsafe { msg_send![NSTextField::class(), alloc] };
        let label: Retained<NSTextField> = unsafe { msg_send![label, initWithFrame: frame] };
        unsafe {
            let _: () = msg_send![&label, setStringValue: &*NSString::from_str(text)];
            let _: () = msg_send![&label, setBordered: objc2::runtime::Bool::NO];
            let _: () = msg_send![&label, setDrawsBackground: objc2::runtime::Bool::NO];
            let _: () = msg_send![&label, setEditable: objc2::runtime::Bool::NO];
            let _: () = msg_send![&label, setSelectable: objc2::runtime::Bool::NO];
            let _: () = msg_send![&label, setAutoresizingMask: mask];
            let _: () = msg_send![&label, setTextColor: &*ns_color(LABEL)];
            let font = objc2_app_kit::NSFont::monospacedDigitSystemFontOfSize_weight(
                12.0,
                objc2_app_kit::NSFontWeightMedium,
            );
            let _: () = msg_send![&label, setFont: &*font];
        }
        self.addSubview(&label);
        label
    }

    /// Write a pixel-true PNG of the panel as it currently renders (the view
    /// snapshots itself through AppKit's display cache — no screen-recording
    /// entitlement). Main thread only.
    fn write_snapshot(&self, path: &str) {
        let rect = self.bounds();
        let Some(rep) = self.bitmapImageRepForCachingDisplayInRect(rect) else {
            log::warn!("[TONEPANEL] snapshot: no bitmap rep for {rect:?}");
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
            log::warn!("[TONEPANEL] snapshot: PNG encode failed");
            return;
        };
        let ok = data.writeToFile_atomically(&NSString::from_str(path), true);
        log::info!("[TONEPANEL] snapshot -> {path} (ok={ok})");
    }
}

// ---------------------------------------------------------------------------
// BOOTSTRAP — the bus-facing seam
// ---------------------------------------------------------------------------

/// Build the tone panel for a vessel window: controls wired to the vessel's
/// callbacks, level bar wired to the Synapse.
///
/// The vessel hands over its callbacks and a broadcast receiver; the panel
/// listens for [`SMessage::Spectrum`] beats and paints `magnitude[0]` as the
/// level bar on the main thread (the same `MainThreadBound` + main-queue
/// dispatch bridge as [`super::meter::bootstrap_meter`]). Returns the view to
/// install as the window's content view.
///
/// Dev affordance: `UNAOS_PANEL_SNAPSHOT=/path.png` writes a pixel-true PNG
/// of the panel after `UNAOS_PANEL_SNAPSHOT_DELAY_MS` (default 1500) — the
/// pulse meter's snapshot idiom, for landing screenshots and headless visual
/// checks.
pub fn bootstrap_tone_panel(
    _window: &NSWindow,
    config: ToneControlConfig,
    callbacks: ToneCallbacks,
    mut rx_synapse: tokio::sync::broadcast::Receiver<SMessage>,
) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_tone_panel must run on the main thread");

    let frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(PANEL_SIZE.0, PANEL_SIZE.1),
    );
    let panel = TonePanelView::new(mtm, frame);
    panel.build_controls(mtm, &config);
    *panel.ivars().callbacks.borrow_mut() = Some(callbacks);

    let bound = Arc::new(MainThreadBound::new(panel.clone(), mtm));

    if let Ok(snap_path) = std::env::var("UNAOS_PANEL_SNAPSHOT") {
        let delay_ms = std::env::var("UNAOS_PANEL_SNAPSHOT_DELAY_MS")
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
                Ok(SMessage::Spectrum { magnitude }) => {
                    let level = magnitude.first().copied().unwrap_or(0.0);
                    let bound = bound.clone();
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        let mtm = MainThreadMarker::new().unwrap();
                        bound.get(mtm).set_level(level);
                    });
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    Retained::into_super(panel)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f64 = 55.0;
    const MAX: f64 = 1760.0; // 5 octaves above MIN

    #[test]
    fn slider_endpoints_map_to_range_edges() {
        assert!((slider_to_hz(0.0, MIN, MAX) - MIN).abs() < 1e-9);
        assert!((slider_to_hz(1.0, MIN, MAX) - MAX).abs() < 1e-9);
    }

    #[test]
    fn slider_midpoint_is_the_geometric_mean() {
        let mid = slider_to_hz(0.5, MIN, MAX);
        assert!((mid - (MIN * MAX).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn a440_sits_three_fifths_up_the_five_octave_slider() {
        // 440 = 55 * 2^3 over a 2^5 range: exactly t = 3/5.
        let t = hz_to_slider(440.0, MIN, MAX);
        assert!((t - 0.6).abs() < 1e-12);
        assert!((slider_to_hz(t, MIN, MAX) - 440.0).abs() < 1e-9);
    }

    #[test]
    fn mapping_clamps_out_of_range_input() {
        assert!((slider_to_hz(-1.0, MIN, MAX) - MIN).abs() < 1e-9);
        assert!((slider_to_hz(2.0, MIN, MAX) - MAX).abs() < 1e-9);
        assert_eq!(hz_to_slider(1.0, MIN, MAX), 0.0);
        assert_eq!(hz_to_slider(1e6, MIN, MAX), 1.0);
    }
}
