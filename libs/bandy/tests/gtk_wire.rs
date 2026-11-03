//! VUG-FACE-BUTTONS-GTK (LEDGER SR488): KATs of the wire quartzite's GTK image surface sends for the right / middle /
//! extra buttons, the wheel and the pinch. The tested source is quartzite's ONE `libs/quartzite/src/platforms/gtk/wire.rs`,
//! INCLUDED here by `#[path]` (no copy; the seat's grant, MEASURE SR488): quartzite's gtk feature needs gtk4 dev headers
//! the build seat lacks, and wire.rs names only `bandy::` paths. Each golden is byte-equal to the SR477 macOS one in
//! `smessage_kats.rs` (kat_browser_aux_button, kat_browser_aux_button_mods_surface, kat_browser_pinch).

#[path = "../../quartzite/src/platforms/gtk/wire.rs"]
mod wire;

use bandy::SMessage;

fn json(m: Option<SMessage>) -> String {
    serde_json::to_string(&m.expect("a message")).expect("serialize")
}

const FRAME: (u32, u32) = (100, 80);

#[test]
fn gdk_buttons_map_onto_appkit_numbers() {
    for none in [0, 1, 4, 5, 6, 7] {
        assert_eq!(wire::aux_button(none), None, "gdk {none}");
    }
    assert_eq!(wire::aux_button(3), Some(bandy::BUTTON_RIGHT));
    assert_eq!(wire::aux_button(2), Some(bandy::BUTTON_MIDDLE));
    assert_eq!(wire::aux_button(8), Some(3));
    assert_eq!(wire::aux_button(9), Some(4));
    assert_eq!(wire::aux_button(u32::MAX), Some(u8::MAX));
}

#[test]
fn right_press_is_the_mac_wire() {
    assert_eq!(
        json(wire::aux_wire(3, true, 12.5, 40.0, FRAME, 0, 0)),
        r#"{"BrowserAuxButton":{"button":1,"down":true,"x":12.5,"y":40.0}}"#
    );
}

#[test]
fn middle_release_with_mods_and_surface_is_the_mac_wire() {
    assert_eq!(
        json(wire::aux_wire(2, false, 1.0, 2.0, FRAME, bandy::MOD_CONTROL, 3)),
        r#"{"BrowserAuxButton":{"button":2,"down":false,"x":1.0,"y":2.0,"mods":2,"surface":3}}"#
    );
}

#[test]
fn press_on_the_frame_release_anywhere() {
    assert!(wire::aux_wire(3, true, -1.0, 5.0, FRAME, 0, 0).is_none(), "press left of the frame");
    assert!(wire::aux_wire(3, true, 100.0, 5.0, FRAME, 0, 0).is_none(), "press at the right edge");
    assert!(wire::aux_wire(3, true, 5.0, 80.0, FRAME, 0, 0).is_none(), "press at the bottom edge");
    assert!(wire::aux_wire(3, true, 99.5, 79.5, FRAME, 0, 0).is_some(), "press inside the last pixel");
    assert_eq!(
        json(wire::aux_wire(3, false, -5.0, 300.0, FRAME, 0, 0)),
        r#"{"BrowserAuxButton":{"button":1,"down":false,"x":-5.0,"y":300.0}}"#
    );
    assert!(wire::aux_wire(3, true, 1.0, 1.0, (0, 0), 0, 0).is_none(), "no frame: no press");
    assert!(wire::aux_wire(3, false, 1.0, 1.0, (0, 80), 0, 0).is_none(), "no frame: no release");
    assert!(wire::aux_wire(1, true, 1.0, 1.0, FRAME, 0, 0).is_none(), "the primary button is BrowserButton's");
    assert!(wire::aux_wire(0, false, 1.0, 1.0, FRAME, 0, 0).is_none(), "a denied drag's end");
}

#[test]
fn wheel_and_touchpad_scroll() {
    assert_eq!(json(wire::scroll_wire(0.0, 1.0, true, 0, 0)), r#"{"BrowserScroll":[0.0,3.0]}"#);
    assert_eq!(json(wire::scroll_wire(0.0, -1.0, true, 0, 0)), r#"{"BrowserScroll":[0.0,-3.0]}"#);
    assert_eq!(json(wire::scroll_wire(2.5, 0.0, false, 0, 2)), r#"{"BrowserScroll":[7.5,0.0,2]}"#);
    assert!(wire::scroll_wire(0.0, 0.0, true, 0, 0).is_none());
    // Shift turns a wheel's vertical notch horizontal (AppKit's mouse-wheel rule), not a touchpad's delta.
    assert_eq!(json(wire::scroll_wire(0.0, 1.0, true, bandy::MOD_SHIFT, 0)), r#"{"BrowserScroll":[3.0,0.0]}"#);
    assert_eq!(json(wire::scroll_wire(0.0, 1.0, false, bandy::MOD_SHIFT, 0)), r#"{"BrowserScroll":[0.0,3.0]}"#);
    assert_eq!(json(wire::scroll_wire(1.0, 1.0, true, bandy::MOD_SHIFT, 0)), r#"{"BrowserScroll":[3.0,3.0]}"#);
}

#[test]
fn pinch_sends_increments() {
    let mut last = 1.0;
    let at = Some((3.0, 4.0));
    assert_eq!(json(wire::pinch_wire(&mut last, 1.25, at, FRAME, 0)), r#"{"BrowserPinch":{"scale":1.25,"x":3.0,"y":4.0}}"#);
    assert!(wire::pinch_wire(&mut last, 1.25, at, FRAME, 0).is_none(), "an increment of 1 sends nothing");
    assert_eq!(json(wire::pinch_wire(&mut last, 1.5625, at, FRAME, 0)), r#"{"BrowserPinch":{"scale":1.25,"x":3.0,"y":4.0}}"#);
    assert_eq!(last, 1.5625);
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(wire::pinch_wire(&mut last, bad, at, FRAME, 0).is_none());
        assert_eq!(last, 1.5625, "an invalid scale keeps last");
    }
}

#[test]
fn pinch_off_the_frame_advances_and_the_product_is_the_scale() {
    let mut last = 1.0;
    let mut product = 1.0;
    for (scale, at) in [(1.1, Some((1.0, 1.0))), (1.3, Some((-4.0, 1.0))), (0.9, None), (0.8, Some((50.0, 40.0)))] {
        if let Some(SMessage::BrowserPinch { scale: step, .. }) = wire::pinch_wire(&mut last, scale, at, FRAME, 0) {
            product *= step;
        }
        assert_eq!(last, scale);
    }
    // The two off-frame steps are not sent (the macOS rule), but the next sent increment is against the true scale.
    assert!((product - 1.1 * (0.8 / 0.9)).abs() < 1e-12, "{product}");
}
