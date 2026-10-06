// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! WINSNAP (R75) — window snapping: drag a title to a panel edge, or press Cmd/Ctrl+Alt+arrow.
//!
//! Declared from the TAIL of `wm.rs` (`#[path]`, a CHILD module) so it reaches the window table,
//! `zoom`, the vacate steps and the `DRAG_*` cells with no widening of any of them. `wm.rs` carries
//! four one-line seams: `motion` (drag_motion), `end` (drag_end), `cancel` (drag_cancel) and the
//! router's `key`. Nothing else in `wm.rs` knows this file exists.
//!
//! What a snap CAN change is the zoom's set: origin and integer `scale`. The surface `w`/`h` are the
//! owner's and stay put (see `wm::zoom`), so "half" is the half-screen ZONE the window is anchored in
//! at the largest scale that fits it; when WINRESIZE lands real resizing, `snap_zone` is the one place
//! to ask for the zone rect itself.
//!
//! Pre-snap placement lives in a side table keyed by window id (not on the row, so `Window` is
//! untouched). An entry is trusted only while the row still sits exactly where the snap put it.
use super::*;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering::Relaxed};

const Z_NONE: u8 = 0;
const Z_LEFT: u8 = 1;
const Z_RIGHT: u8 = 2;
const Z_TL: u8 = 3;
const Z_TR: u8 = 4;
const Z_BL: u8 = 5;
const Z_BR: u8 = 6;
const Z_ZOOM: u8 = 7;
/// Pointer within this many px of a panel edge arms a zone.
const EDGE: i32 = 8;
/// Pointer travel from the grab before an un-snap lifts the window.
const UNSNAP_PX: i64 = 6;

fn zname(z: u8) -> &'static str {
    match z {
        Z_LEFT => "left",
        Z_RIGHT => "right",
        Z_TL => "tl",
        Z_TR => "tr",
        Z_BL => "bl",
        Z_BR => "br",
        Z_ZOOM => "zoom",
        _ => "none",
    }
}

#[derive(Clone, Copy)]
struct Entry {
    live: bool,
    owner: u64,
    saved: (usize, usize, usize),
    snapped: (usize, usize, usize),
}
const NO_ENTRY: Entry = Entry { live: false, owner: 0, saved: (0, 0, 0), snapped: (0, 0, 0) };
static SNAP: spin::Mutex<alloc::vec::Vec<Entry>> = spin::Mutex::new(alloc::vec::Vec::new()); // WINDOWCAP-2: one entry per slot, grown on first snap

/// The zone the live drag's pointer is in (preview armed), or `Z_NONE`.
static ZONE: AtomicU8 = AtomicU8::new(Z_NONE);
static PTICK: AtomicU32 = AtomicU32::new(0);
static PAINTED: AtomicU32 = AtomicU32::new(0);
static SNAPS: AtomicU32 = AtomicU32::new(0);
static RESTORES: AtomicU32 = AtomicU32::new(0);
static UNSNAPS: AtomicU32 = AtomicU32::new(0);
static LOG: AtomicU32 = AtomicU32::new(0);

fn act(id: WinId, zone: &str, r: (usize, usize, usize, usize), via: &str) {
    if LOG.fetch_add(1, Relaxed) >= 64 {
        return;
    }
    serial_println!("[wm-act] snap win={} zone={} rect={}x{}+{}+{} via={}", id, zone, r.2, r.3, r.0, r.1, via);
}

/// `(pw, ph, work_top, work_h)` — non-blocking, so the input band can call it.
fn panel() -> Option<(usize, usize, usize, usize)> {
    let pi = crate::video::panel_info_nonblocking()?;
    let (pw, ph) = (pi.width, pi.height);
    Some((pw, ph, work_top(pw, ph), work_h(pw, ph)))
}

/// The zone rectangle `(x, y, w, h)` in panel pixels, menu bar and status chrome excluded.
fn zone_rect(z: u8, pw: usize, wtop: usize, uh: usize) -> (usize, usize, usize, usize) {
    let (hw, hh) = (pw / 2, uh / 2);
    match z {
        Z_LEFT => (0, wtop, hw, uh),
        Z_RIGHT => (hw, wtop, pw - hw, uh),
        Z_TL => (0, wtop, hw, hh),
        Z_TR => (hw, wtop, pw - hw, hh),
        Z_BL => (0, wtop + hh, hw, uh - hh),
        Z_BR => (hw, wtop + hh, pw - hw, uh - hh),
        _ => (0, wtop, pw, uh),
    }
}

/// Which zone does panel point `(x, y)` arm? Pure.
fn zone_at(x: i32, y: i32, pw: usize, wtop: usize, uh: usize) -> u8 {
    let (pw_i, e) = (pw as i32, EDGE);
    let left = x <= e;
    let right = x >= pw_i - 1 - e;
    if y <= e && !left && !right {
        return Z_ZOOM;
    }
    if !left && !right {
        return Z_NONE;
    }
    let top_band = (wtop + uh / 6) as i32;
    let bot_band = (wtop + uh - uh / 6) as i32;
    if y < top_band {
        if left { Z_TL } else { Z_TR }
    } else if y >= bot_band {
        if left { Z_BL } else { Z_BR }
    } else if left {
        Z_LEFT
    } else {
        Z_RIGHT
    }
}

/// Erase `b` and repaint whatever is under it — the same five-step vacate `zoom` performs.
fn vacate(b: (usize, usize, usize, usize)) {
    let barrier = DrainBarrier::drain();
    erase(&[b]);
    damage_intersecting(b.0, b.1, b.2, b.3);
    crate::video::screen::request_full_present();
    drop(barrier);
    composite();
}

/// Write the geometry of `id` (origin, scale), pin it, discard zoom's memory, vacate the old box.
/// `None` for a dead/compat/kernel row.
fn apply(id: WinId, x: usize, y: usize, scale: usize) -> Option<()> {
    let mut t = table();
    let vac = match row_mut(&mut t, id) {
        None => return None,
        Some(r) => {
            if r.compat || r.owner_asid == 0 {
                return None;
            }
            let before = outer_box(r);
            r.x = x;
            r.y = y;
            r.scale = scale.max(1);
            r.zoom_saved = None;
            r.pinned = true;
            r.damage_all();
            if outer_box(r) != before { Some(before) } else { None }
        }
    };
    drop(t);
    if let Some(b) = vac {
        vacate(b);
    }
    Some(())
}

fn geom(id: WinId) -> Option<(usize, usize, usize, usize, usize, u64, Option<(usize, usize, usize)>)> {
    let t = table();
    row(&t, id).filter(|r| !r.compat && r.owner_asid != 0).map(|r| (r.x, r.y, r.w, r.h, r.scale, r.owner_asid, r.zoom_saved))
}

fn outer_of(id: WinId) -> (usize, usize, usize, usize) {
    let t = table();
    row(&t, id).map(|r| outer_box(r)).unwrap_or((0, 0, 0, 0))
}

/// The entry for `id` if the row still sits where the snap put it.
fn entry_valid(id: WinId) -> Option<Entry> {
    let slot = (id as usize).wrapping_sub(1);
    let e = SNAP.lock().get(slot).copied()?; // WINDOWCAP-2: an id never snapped has no entry
    let (x, y, _, _, s, owner, _) = geom(id)?;
    if e.live && e.owner == owner && e.snapped == (x, y, s) { Some(e) } else { None }
}

fn set_entry(id: WinId, e: Entry) {
    let slot = (id as usize).wrapping_sub(1);
    if id != WIN_NONE {
        let mut t = SNAP.lock(); // WINDOWCAP-2: grow to the slot
        if t.len() <= slot { t.resize(slot + 1, NO_ENTRY); }
        t[slot] = e;
    }
}

/// Snap `id` into `zone`. Returns the outcome token.
fn snap_zone(id: WinId, zone: u8, via: &str) -> &'static str {
    let Some((pw, ph, wtop, uh)) = panel() else { return "nofb" };
    let Some((x, y, w, h, scale, owner, zs)) = geom(id) else { return "norow" };
    let keep = entry_valid(id);
    let pre = match (keep, zs) {
        (Some(e), _) => e.saved,
        (None, Some(z)) => z,
        (None, None) => (x, y, scale),
    };
    if zone == Z_ZOOM {
        if zs.is_some() && keep.is_none() {
            return "zoom-nochange";
        }
        if keep.is_some() {
            // snapped (not zoomed): drop the snap memory's geometry and zoom from the pre-snap placement.
            let _ = apply(id, pre.0, pre.1, pre.2);
        }
        let tok = zoom(id);
        let now = geom(id).map(|g| (g.0, g.1, g.4)).unwrap_or((x, y, scale));
        set_entry(id, Entry { live: true, owner, saved: pre, snapped: now });
        SNAPS.fetch_add(1, Relaxed);
        act(id, "zoom", outer_of(id), via);
        return tok;
    }
    let (zx, zy, zw, zh) = zone_rect(zone, pw, wtop, uh);
    let s = zoom_scale(zw, zh, ph, w, h);
    let (nx, ny) = (zx + BORDER(), zy + TITLE_H() + BORDER());
    if apply(id, nx, ny, s).is_none() {
        return "declined";
    }
    set_entry(id, Entry { live: true, owner, saved: pre, snapped: (nx, ny, s.max(1)) });
    SNAPS.fetch_add(1, Relaxed);
    act(id, zname(zone), outer_of(id), via);
    "snapped"
}

/// Cmd/Ctrl+Alt+Down: put the pre-snap placement back (or un-zoom a plain zoom).
fn restore(id: WinId, via: &str) -> &'static str {
    if let Some(e) = entry_valid(id) {
        set_entry(id, NO_ENTRY);
        let _ = apply(id, e.saved.0, e.saved.1, e.saved.2);
        RESTORES.fetch_add(1, Relaxed);
        act(id, "restore", outer_of(id), via);
        return "restored";
    }
    if matches!(geom(id), Some((.., Some(_)))) {
        let tok = zoom(id);
        RESTORES.fetch_add(1, Relaxed);
        act(id, "restore", outer_of(id), via);
        return tok;
    }
    "nothing-to-restore"
}

/// The focused app window: the topmost non-compat row owned by the focus holder.
fn focused_win() -> Option<WinId> {
    let asid = focus_asid();
    if asid == 0 {
        return None;
    }
    let t = table();
    let mut best: Option<(u32, WinId)> = None;
    for r in t.rows.iter() {
        if r.used && !r.compat && r.owner_asid == asid && best.map_or(true, |(z, _)| r.z >= z) {
            best = Some((r.z as u32, r.id));
        }
    }
    best.map(|b| b.1)
}

// ---- the four wm.rs seams ----------------------------------------------------------------------

/// Router seam: a snap chord. Returns `true` when the action was ours (consumed even with no window).
pub fn key(a: crate::video::keymap::Action) -> bool {
    use crate::video::keymap::Action as A;
    let z = match a {
        A::SnapLeft => Z_LEFT,
        A::SnapRight => Z_RIGHT,
        A::SnapZoom => Z_ZOOM,
        A::SnapRestore => Z_NONE,
        _ => return false,
    };
    if let Some(id) = focused_win() {
        if z == Z_NONE { restore(id, "key"); } else { snap_zone(id, z, "key"); }
    }
    true
}

/// `drag_motion` seam: un-snap a snapped window that is being dragged away, then track the edge zone.
pub(super) fn motion(id: WinId, x: i32, y: i32) {
    unsnap(id, x, y);
    let Some((pw, _ph, wtop, uh)) = panel() else { return };
    // The preview arms only once the gesture has actually moved something (a grab that starts on the
    // edge must not preview before the hand has gone anywhere).
    let z = if DRAG_MOVES.load(Relaxed) > 0 { zone_at(x, y, pw, wtop, uh) } else { Z_NONE };
    let old = ZONE.swap(z, Relaxed);
    if z != old {
        if old != Z_NONE {
            vacate(zone_rect(old, pw, wtop, uh));
        }
        PTICK.store(0, Relaxed);
        if z != Z_NONE {
            paint(zone_rect(z, pw, wtop, uh));
        }
    } else if z != Z_NONE && PTICK.fetch_add(1, Relaxed) % 4 == 3 {
        paint(zone_rect(z, pw, wtop, uh)); // the window's own composites overdraw part of the ring
    }
}

/// `drag_end` seam: the release snaps to the live zone; the preview goes either way.
pub(super) fn end(id: WinId) {
    let z = ZONE.swap(Z_NONE, Relaxed);
    if z == Z_NONE {
        return;
    }
    if let Some((pw, _, wtop, uh)) = panel() {
        vacate(zone_rect(z, pw, wtop, uh));
    }
    snap_zone(id, z, "drag");
}

/// `drag_cancel` seam: a belt release (`release-level`) is still a release; every other cancel
/// (focus key, window closed) just drops the preview.
pub(super) fn cancel(id: WinId, why: &str) {
    if why == "release-level" {
        end(id);
    } else if ZONE.load(Relaxed) != Z_NONE {
        let z = ZONE.swap(Z_NONE, Relaxed);
        if let Some((pw, _, wtop, uh)) = panel() {
            vacate(zone_rect(z, pw, wtop, uh));
        }
    }
}

/// M3 — dragging a snapped window's title away restores its pre-snap size under the cursor.
fn unsnap(id: WinId, x: i32, y: i32) {
    let Some(e) = entry_valid(id) else { return };
    let Some((rx, ry, w, _h, scale, _o, _z)) = geom(id) else { return };
    let (offx, offy) = (DRAG_OFF_X.load(Relaxed), DRAG_OFF_Y.load(Relaxed));
    let (gx, gy) = (rx as i64 + offx, ry as i64 + offy);
    if (x as i64 - gx).abs() < UNSNAP_PX && (y as i64 - gy).abs() < UNSNAP_PX {
        return;
    }
    let ss = e.saved.2.max(1);
    let (ow_old, ow_new) = ((w * scale + 2 * BORDER()) as i64, (w * ss + 2 * BORDER()) as i64);
    let fx = (x as i64 - (rx as i64 - BORDER() as i64)).clamp(0, ow_old);
    let nx = (x as i64 - fx * ow_new / ow_old.max(1) + BORDER() as i64).max(BORDER() as i64);
    let ny = (y as i64 - offy).max(0);
    set_entry(id, NO_ENTRY);
    if apply(id, nx as usize, ny as usize, ss).is_some() {
        DRAG_OFF_X.store(x as i64 - nx, Relaxed);
        DRAG_LAST_X.store(nx, Relaxed);
        DRAG_LAST_Y.store(ny, Relaxed);
        UNSNAPS.fetch_add(1, Relaxed);
        act(id, "unsnap", outer_of(id), "drag");
    }
}

/// The translucent preview: a 3 px checkerboard ring (every other pixel written, none read, so it
/// costs no framebuffer reads and the desktop shows through the gaps). Written to the front buffer;
/// `vacate` of the same rect is its eraser.
fn paint(r: (usize, usize, usize, usize)) {
    if crate::video::dimidle::blanked() {
        return;
    }
    let fb = crate::arch::without_interrupts(|| crate::video::WRITER.try_lock().map(|g| *g));
    let Some(fb) = fb else { return };
    if !fb.is_ready() || r.2 < 16 || r.3 < 16 {
        return;
    }
    const RING: usize = 3;
    const INK: u32 = crate::video::theme::SNAP_INK;
    let (x0, y0, x1, y1) = (r.0 + 2, r.1 + 2, r.0 + r.2 - 2, r.1 + r.3 - 2);
    let mut y = y0;
    while y < y1 {
        let edge_row = y < y0 + RING || y >= y1 - RING;
        let mut x = x0;
        while x < x1 {
            if (edge_row || x < x0 + RING || x >= x1 - RING) && (x + y) & 1 == 0 {
                fb.put_pixel(x, y, INK);
            }
            x += 1;
        }
        y += 1;
    }
    fb.flush_rect(r.0, r.1, r.2, r.3);
    PAINTED.fetch_add(1, Relaxed);
}

// ---- the fixture: `tests winsnap` ----------------------------------------------------------------

#[cfg(feature = "witness")]
#[repr(align(16))]
struct Surf([u32; FIX_W * FIX_H]);
#[cfg(feature = "witness")]
static SURF: Surf = Surf([crate::video::theme::fixture::STEEL; FIX_W * FIX_H]);

/// Drives a real drag through the router (`wc_click_route_at` grab, a routed `MouseAbsolute` to the
/// left edge, `Button(0)` release), asserts the row sits in the left half and the preview painted;
/// Cmd+Alt+Down (`wc_focus_key`) restores; then the other six zones by the chord/zone path, and an
/// un-snap by dragging the snapped title away.
#[cfg(feature = "witness")]
pub fn selftest() {
    use crate::arch::x86_64::syscall as sc;
    use crate::pal::Event;
    use crate::video::keymap::Action;
    static DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, Relaxed) {
        return;
    }
    let Some((pw, ph, wtop, uh)) = panel() else {
        serial_println!(":: WINSNAP: zones=0 -> SKIP (framebuffer not ready) ::");
        return;
    };
    if pw < 256 || ph < 256 {
        serial_println!(":: WINSNAP: zones=0 -> SKIP (panel {}x{} too small) ::", pw, ph);
        return;
    }
    const OWNER: u64 = 3; // slot 2, a real ring-3-band owner (the wmdirect idiom)
    const OTHER: u64 = 4;
    let s = &raw const SURF as usize;
    let len = core::mem::size_of_val(&SURF);
    let w = create(OWNER, s, len, FIX_W as u32, FIX_H as u32, FIX_STRIDE as u32, b"snap");
    let wo = create(OTHER, s, len, FIX_W as u32, FIX_H as u32, FIX_STRIDE as u32, b"snpo");
    if w == WIN_NONE || wo == WIN_NONE {
        serial_println!(":: WINSNAP: zones=0 -> SKIP (window table full) ::");
        close(w);
        close(wo);
        return;
    }
    let saved_focus = sc::user_input_active();
    let (s0, a0, u0) = (SNAPS.load(Relaxed), RESTORES.load(Relaxed), UNSNAPS.load(Relaxed));
    let (ox, oy) = (pw / 3, ph / 3 + TITLE_H() + BORDER());
    move_to(w, ox, oy);
    move_to(wo, pw / 3, ph / 3 * 2 + TITLE_H() + BORDER());
    let Some(i0) = info(w) else {
        serial_println!(":: WINSNAP: zones=0 -> SKIP (row vanished) ::");
        close(w);
        close(wo);
        return;
    };
    let pre = (i0.x, i0.y, i0.scale);

    // Leg 1 — the zone map, pure: seven distinct zones from seven points.
    let mid = (wtop + uh / 2) as i32;
    let pts = [
        (2, mid, Z_LEFT),
        (pw as i32 - 2, mid, Z_RIGHT),
        (2, wtop as i32 + 2, Z_TL),
        (pw as i32 - 2, wtop as i32 + 2, Z_TR),
        (2, (wtop + uh) as i32 - 2, Z_BL),
        (pw as i32 - 2, (wtop + uh) as i32 - 2, Z_BR),
        (pw as i32 / 2, 1, Z_ZOOM),
    ];
    let mut seen = 0u32;
    for &(px, py, z) in pts.iter() {
        if zone_at(px, py, pw, wtop, uh) == z {
            seen |= 1 << z;
        }
    }
    let zones = seen.count_ones();
    let centre_none = zone_at(pw as i32 / 2, mid, pw, wtop, uh) == Z_NONE;

    // Leg 2 — the drag, through the router. Focus parked on the other window so the press MOVES `w`.
    let hid = |v: i32, span: usize| -> i32 { ((v as i64 * 32767) / (span.max(1) as i64 - 1).max(1)) as i32 };
    let tpx = (i0.x + 1) as i32;
    let tpy = (i0.y - TITLE_H() / 2 - BORDER()) as i32;
    sc::user_input_set_active(OTHER);
    focus_changed(OTHER);
    crate::pal::cursor::set_button_level(true);
    DBL_OFF.store(true, Relaxed);
    let grabbed = sc::wc_click_route_at(Event::Button(1), tpx, tpy) && drag_active() == w;
    let drag_to = |px: i32, py: i32| {
        crate::pal::cursor::set_abs(hid(px, pw), hid(py, ph), pw as i32, ph as i32);
        let raw = Event::MouseAbsolute { x: hid(px, pw), y: hid(py, ph) };
        let _ = sc::wc_route_event(raw);
        sc::wc_route_tail(raw);
        drag_motion(px, py); // the router's tail is paced; this is the unpaced step so the leg cannot lose a report to the throttle
    };
    let p0 = PAINTED.load(Relaxed);
    drag_to(pw as i32 / 3, mid); // a first real move (arms DRAG_MOVES)
    drag_to(2, mid); // the edge
    let zone_live = ZONE.load(Relaxed) == Z_LEFT;
    let preview_ok = zone_live && PAINTED.load(Relaxed) > p0;
    crate::pal::cursor::set_button_level(false);
    let released = sc::wc_click_route_at(Event::Button(0), 2, mid);
    let (lx, ly, lw, lh) = zone_rect(Z_LEFT, pw, wtop, uh);
    let ob = outer_of(w);
    let left_ok = grabbed
        && released
        && drag_active() == WIN_NONE
        && ZONE.load(Relaxed) == Z_NONE
        && ob.0 == lx
        && ob.1 == ly
        && ob.0 + ob.2 <= lx + lw
        && ob.1 + ob.3 <= ly + lh
        && entry_valid(w).is_some();

    // Leg 3 — M3: drag the snapped title away; the pre-snap scale comes back and the drag goes on.
    let un_ok = match info(w) {
        Some(i) => {
            let (px, py) = ((i.x + 1) as i32, (i.y - TITLE_H() / 2 - BORDER()) as i32);
            sc::user_input_set_active(OTHER);
            focus_changed(OTHER);
            crate::pal::cursor::set_button_level(true);
            let g = sc::wc_click_route_at(Event::Button(1), px, py) && drag_active() == w;
            drag_to(px + 40, py + 40);
            let j = info(w);
            crate::pal::cursor::set_button_level(false);
            sc::wc_click_route_at(Event::Button(0), px + 40, py + 40);
            g && j.map(|j| j.scale) == Some(pre.2) && entry_valid(w).is_none() && UNSNAPS.load(Relaxed) > u0
        }
        None => false,
    };

    // Leg 4 — the chords, through the router seam `wc_focus_key`: left, right, zoom, then Down.
    sc::user_input_set_active(OWNER);
    focus_changed(OWNER);
    let k_left = sc::wc_focus_key(Event::Action(Action::SnapLeft));
    let left2 = outer_of(w).0 == lx;
    let k_right = sc::wc_focus_key(Event::Action(Action::SnapRight));
    let (rx, ..) = zone_rect(Z_RIGHT, pw, wtop, uh);
    let right_ok = outer_of(w).0 == rx;
    let k_zoom = sc::wc_focus_key(Event::Action(Action::SnapZoom));
    let zoomed = matches!(geom(w), Some((.., Some(_))));
    let k_down = sc::wc_focus_key(Event::Action(Action::SnapRestore));
    let back = info(w).map(|i| (i.x, i.y, i.scale));
    // After the un-snap leg the row sits wherever the drag left it, so "restore" is judged against
    // the placement a chord-snap SAVED: re-run left -> Down from a known origin.
    move_to(w, ox, oy);
    let known = info(w).map(|i| (i.x, i.y, i.scale));
    let k_l2 = sc::wc_focus_key(Event::Action(Action::SnapLeft));
    let snapped_l = outer_of(w).0 == lx && entry_valid(w).is_some();
    let k_d2 = sc::wc_focus_key(Event::Action(Action::SnapRestore));
    let restored = info(w).map(|i| (i.x, i.y, i.scale)) == known && known.is_some();
    let chords_ok = k_left && left2 && k_right && right_ok && k_zoom && zoomed && k_down && back.is_some() && k_l2 && snapped_l && k_d2 && restored;

    // Leg 5 — the four quarters by `snap_zone` (the drag path's own call), each inside its rect.
    let mut quarters = 0;
    for z in [Z_TL, Z_TR, Z_BL, Z_BR] {
        move_to(w, ox, oy);
        snap_zone(w, z, "fixture");
        let (qx, qy, qw, qh) = zone_rect(z, pw, wtop, uh);
        let b = outer_of(w);
        if b.0 == qx && b.1 == qy && b.0 + b.2 <= qx + qw && b.1 + b.3 <= qy + qh {
            quarters += 1;
        }
        restore(w, "fixture");
    }

    DBL_OFF.store(false, Relaxed);
    close(w);
    close(wo);
    composite();
    sc::user_input_set_active(saved_focus);
    let snaps = SNAPS.load(Relaxed) - s0;
    let restores = RESTORES.load(Relaxed) - a0;
    let ok = zones == 7 && centre_none && left_ok && preview_ok && un_ok && chords_ok && quarters == 4 && snaps >= 8 && restores >= 3;
    serial_println!(
        ":: WINSNAP: zones={} snaps={} restores={} preview_ok={} unsnaps={} left={} chords={} quarters={} rect={}x{}+{}+{} -> {} ::",
        zones, snaps, restores, preview_ok, UNSNAPS.load(Relaxed) - u0, left_ok, chords_ok, quarters, ob.2, ob.3, ob.0, ob.1,
        if ok { "PASS" } else { "FAIL" }
    );
}
