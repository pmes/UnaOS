//! CHARTER: VUG-FACE-BUTTONS-GTK (LEDGER SR488) — the GTK image surface's non-primary pointer wire as pure fns: a gdk
//! button, scroll delta or zoom scale in, the `SMessage` the macOS surface (SR477) sends for the same gesture out. No gtk
//! types and only `bandy::` paths, so bandy's tests include this ONE file by `#[path]` (quartzite's gtk feature needs the
//! gtk4 dev headers the build seat lacks).

use bandy::SMessage;

/// The point (x, y) in surface pixels lies on a frame of `frame` (width, height); false with no frame. The macOS
/// `surface_point` rule.
fn on_frame(x: f64, y: f64, frame: (u32, u32)) -> bool {
    frame.0 > 0 && frame.1 > 0 && x >= 0.0 && x < frame.0 as f64 && y >= 0.0 && y < frame.1 as f64
}

/// A gdk button number as `BrowserAuxButton`'s `button` (AppKit's `buttonNumber`): gdk 3 (secondary) is
/// `bandy::BUTTON_RIGHT`, 2 is `bandy::BUTTON_MIDDLE`, 8 / 9 (back / forward) are 3 / 4 and on up; `None` for the
/// primary button (`BrowserButton`'s), for 0 and for 4-7 (the legacy scroll buttons GTK4 never delivers as presses).
pub fn aux_button(gdk: u32) -> Option<u8> {
    match gdk {
        2 => Some(bandy::BUTTON_MIDDLE),
        3 => Some(bandy::BUTTON_RIGHT),
        n if n >= 8 => Some((n - 5).min(u8::MAX as u32) as u8),
        _ => None,
    }
}

/// A press (`down`) or release of gdk button `gdk_button` at (x, y) as `BrowserAuxButton`: the press only on the
/// frame, the release anywhere while a frame exists (the macOS `aux_button` rule); `None` for a button `aux_button`
/// does not name. `mods` is bandy's `MOD_*` bits.
pub fn aux_wire(gdk_button: u32, down: bool, x: f64, y: f64, frame: (u32, u32), mods: u8, surface: u32) -> Option<SMessage> {
    let button = aux_button(gdk_button)?;
    let placed = if down { on_frame(x, y, frame) } else { frame.0 > 0 && frame.1 > 0 };
    placed.then_some(SMessage::BrowserAuxButton { button, down, x, y, mods, surface })
}

/// A GTK scroll delta as `BrowserScroll`: GTK's sign is already the wire's (+dy further down the document, +dx further
/// right) and both units scale by 3 as the macOS `scrollWheel:` does (a wheel notch is 1.0 in `ScrollUnit::Wheel`, a
/// touchpad delta is logical pixels ~ AppKit points). A wheel (`wheel`) turned with Shift and no horizontal delta
/// scrolls horizontally, as AppKit does for a mouse wheel. `None` when both deltas are 0.
pub fn scroll_wire(dx: f64, dy: f64, wheel: bool, mods: u8, surface: u32) -> Option<SMessage> {
    let (dx, dy) = if wheel && mods & bandy::MOD_SHIFT != 0 && dx == 0.0 { (dy, 0.0) } else { (dx, dy) };
    (dx != 0.0 || dy != 0.0).then_some(SMessage::BrowserScroll(dx * 3.0, dy * 3.0, surface))
}

/// A GestureZoom `scale-changed` (`scale` absolute since the gesture began) as `BrowserPinch`'s incremental factor
/// `scale / *last` (the macOS `1 + magnification`), `*last` being 1.0 at the gesture's begin. `*last` advances on every
/// valid scale, also off the frame, so each increment is against GTK's current scale (the product of every
/// increment, sent or dropped off the frame, is GTK's scale); `None` (and `*last` kept) for
/// a scale that is not finite or not above 0, `None` for an increment of exactly 1 or a centre `at` off the frame.
pub fn pinch_wire(last: &mut f64, scale: f64, at: Option<(f64, f64)>, frame: (u32, u32), surface: u32) -> Option<SMessage> {
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let step = scale / *last;
    *last = scale;
    let (x, y) = at.filter(|&(x, y)| on_frame(x, y, frame))?;
    (step != 1.0).then_some(SMessage::BrowserPinch { scale: step, x, y, surface })
}
