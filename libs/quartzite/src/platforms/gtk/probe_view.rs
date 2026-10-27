// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! REALSENSE (LEDGER SR92, R112, R104, R108): Sonde's GTK face — the other-OS port of the probe pane.
//!
//! R104: this is the GTK face and it STAYS; the UnaOS-native face is inserted beside it, over the same
//! model (`handlers/sonde/src/model`). R108: the face is exactly a SURFACE painter, an INPUT source and a
//! RESIZE source — the pane decides everything else:
//!
//! * SURFACE — `SMessage::SurfaceBlit { url == id }` carries the pane's RGBA8 pixels (depth as a colour
//!   ramp beside the colour stream, composed by the model); they are swizzled to Cairo's native-endian
//!   ARGB32 by the Surface contract's one converter (`crate::tetra::surface_argb32`, NLE2ORDER SR227) and
//!   painted aspect-fitted, letterboxed in the window's background.
//! * INPUT — pointer motion, leave, scroll and keys go out as `SMessage::Input { target: id, text }` in the
//!   model's grammar (`move <x> <y>` in SURFACE pixels — the inverse of the paint transform — `leave`,
//!   `scroll <dy>`, `key <name>`).
//! * RESIZE — the drawing area's allocation goes out as `resize <w> <h>`, so the pane recomposes at the
//!   window's size instead of being scaled.
//!
//! quartzite does not link `sonde` or `probe_core` (a face never owns the model); the strings above are
//! the whole contract, and `sonde::model::PaneInput::parse` is their one reader.

use gtk4::prelude::*;
use gtk4::DrawingArea;
use std::cell::RefCell;
use std::rc::Rc;

use bandy::{SMessage, Synapse};
use crate::NativeView;

/// The paint transform of a `sw x sh` surface into a `ww x wh` widget: (scale, x offset, y offset).
fn paint_transform(sw: u32, sh: u32, ww: i32, wh: i32) -> (f64, f64, f64) {
    if sw == 0 || sh == 0 || ww <= 0 || wh <= 0 {
        return (1.0, 0.0, 0.0);
    }
    let s = (ww as f64 / sw as f64).min(wh as f64 / sh as f64);
    (s, (ww as f64 - sw as f64 * s) / 2.0, (wh as f64 - sh as f64 * s) / 2.0)
}

/// The probe pane's GTK face for the surface `id` (Sonde's `PANE_URL`, `sonde://pane`).
pub fn bootstrap_probe_view(id: &str, synapse: Synapse) -> NativeView {
    let area = DrawingArea::builder().hexpand(true).vexpand(true).build();
    area.set_focusable(true);
    area.set_can_focus(true);

    // (cairo-ordered pixels, width, height) of the last surface.
    let surface: Rc<RefCell<(Vec<u8>, u32, u32)>> = Rc::new(RefCell::new((Vec::new(), 0, 0)));

    {
        let surface = surface.clone();
        area.set_draw_func(move |_area, cr, ww, wh| {
            cr.set_source_rgb(0.094, 0.102, 0.118);
            let _ = cr.paint();
            let s = surface.borrow();
            let (sw, sh) = (s.1, s.2);
            if s.0.len() < sw as usize * sh as usize * 4 || sw == 0 || sh == 0 {
                return;
            }
            if let Ok(img) = gtk4::cairo::ImageSurface::create_for_data(
                s.0.clone(),
                gtk4::cairo::Format::ARgb32,
                sw as i32,
                sh as i32,
                (sw * 4) as i32,
            ) {
                let (k, ox, oy) = paint_transform(sw, sh, ww, wh);
                cr.translate(ox, oy);
                cr.scale(k, k);
                let _ = cr.set_source_surface(&img, 0.0, 0.0);
                let _ = cr.paint();
            }
        });
    }

    let send = {
        let synapse = synapse.clone();
        let target = id.to_string();
        move |text: String| synapse.fire(SMessage::Input { target: target.clone(), text })
    };

    // RESIZE: the allocation is the pane's size.
    {
        let send = send.clone();
        area.connect_resize(move |_, w, h| {
            if w > 0 && h > 0 {
                send(format!("resize {w} {h}"));
            }
        });
    }

    // INPUT: pointer, in surface pixels (the inverse of the paint transform).
    {
        let motion = gtk4::EventControllerMotion::new();
        let send_move = send.clone();
        let surf = surface.clone();
        let da = area.clone();
        motion.connect_motion(move |_, x, y| {
            let s = surf.borrow();
            let (k, ox, oy) = paint_transform(s.1, s.2, da.width(), da.height());
            send_move(format!("move {} {}", (x - ox) / k, (y - oy) / k));
        });
        let send_leave = send.clone();
        motion.connect_leave(move |_| send_leave("leave".to_string()));
        area.add_controller(motion);
    }
    {
        let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
        let send = send.clone();
        scroll.connect_scroll(move |_, _dx, dy| {
            send(format!("scroll {dy}"));
            gtk4::glib::Propagation::Stop
        });
        area.add_controller(scroll);
    }
    {
        let click = gtk4::GestureClick::new();
        let da = area.clone();
        click.connect_pressed(move |_, _, _, _| {
            da.grab_focus();
        });
        area.add_controller(click);
    }
    {
        let key = gtk4::EventControllerKey::new();
        let send = send.clone();
        key.connect_key_pressed(move |_, keyval, _code, _mods| match keyval.name() {
            Some(name) => {
                send(format!("key {}", name.as_str()));
                gtk4::glib::Propagation::Stop
            }
            None => gtk4::glib::Propagation::Proceed,
        });
        area.add_controller(key);
    }

    // SURFACE: the pane's pixels for this id.
    {
        let da = area.clone();
        let target = id.to_string();
        let mut rx = synapse.subscribe();
        gtk4::glib::spawn_future_local(async move {
            loop {
                match rx.recv().await {
                    Ok(SMessage::SurfaceBlit { url, width, height, pixels }) if url == target => {
                        *surface.borrow_mut() = (crate::tetra::surface_argb32(&pixels), width, height);
                        da.queue_draw();
                    }
                    Ok(_) => {}
                    // a lagged face skips to the newest surface; a closed bus ends the face's listener
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    area.upcast::<gtk4::Widget>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_fits_and_centres() {
        assert_eq!(paint_transform(200, 100, 400, 400), (2.0, 0.0, 100.0));
        assert_eq!(paint_transform(100, 200, 400, 200), (1.0, 150.0, 0.0));
        assert_eq!(paint_transform(0, 0, 10, 10), (1.0, 0.0, 0.0));
    }
}
