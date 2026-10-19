//! CHARTER: Stria — driver
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! UVCCAM (rmbp-ledger B686, R130) — the camera's PICTURE: the frames the xHCI isochronous stream assembles,
//! decoded or converted, shown in a window at the stream's rate.
//!
//! # WHAT WAS ALREADY IN THE TREE, AND WHAT THIS FILE ADDS
//!
//! The descriptor census and the PROBE negotiation are CAMERA1's (`drivers/uvc.rs`, rmbp-ledger B143); the COMMIT,
//! the isochronous IN ring with frame-ID scheduling and the payload-header frame assembler are XHCIISO's
//! (`drivers/xhci/isoch.rs` + `uvc::probe_iso`, B534). Every one of them ends at `uvc::latest_frame` — the
//! assembler's front buffer — and nothing in the tree ever READ it: no frame of any camera reached the glass.
//! This file is that consumer, and only that:
//!
//! * **M1, the one-line census** — [`census`] prints `[uvc] dev=<vid:pid> formats=<n> mjpeg=<w>x<h>@<fps>
//!   <fourcc>=<w>x<h>@<fps> … frames=<n>` from CAMERA1's `Census` (the default frame of each format at its default
//!   interval). The vid:pid is recorded by the controller's two enumeration paths ([`note_dev`]); the census is
//!   handed in by `probe_iso`.
//! * **M3, the picture** — [`arm`] is told the committed stream's geometry and FourCC; [`pump`] (the controller's
//!   service ladder, beside `service_uvc`) starts ONE `uvc-view` task per armed stream. The task copies the newest
//!   assembled frame OUT of the stream lock (the claim path waits on that lock, so nothing is decoded under it),
//!   then: an MJPEG frame (or any frame that opens with SOI `FF D8`) goes to the tree's one JPEG decoder,
//!   `pixel_core::decode_jpeg` — which installs the T.81 Annex K.3 Huffman tables before any DHT, so a Motion-JPEG
//!   frame without DHT decodes; a YUY2 or NV12 frame is converted (BT.601 / SMPTE 170M limited range — UVC 1.5
//!   §3.9.2.6, the Color Matching descriptor's defaults when a camera declares none) by nearest sampling straight
//!   into the window surface, 0RGB, at the window's size. The window opens on the FIRST good frame (no empty box),
//!   aspect kept, never upscaled, at most two thirds of the work area.
//! * **The wire** — every 5 s while streaming: `[uvc] stream fps=<n> frames=<n> dropped=<n> …` — `fps` is the
//!   CAMERA's assembled frames per second over the window, `frames` the frames PRESENTED to the window so far,
//!   `dropped` the frames the camera completed that the window never showed (a newer one replaced them first).
//! * **`tests uvccam`** — the pure halves over bytes written here: the census of a two-format camera descriptor
//!   (MJPEG 1920x1080 + YUY2 640x480, every length checked against its table), a static PROBE block resolved
//!   against that census, YUY2 / NV12 conversion of known colours, an MJPEG frame without DHT through the JPEG
//!   decoder, and the scaler; then the live stream's counters over 2 s, or `stream=skip(no-device)`.
//!
//! # SEAMS
//!
//! This file calls `uvc::latest_frame` / `uvc::parse` / `uvc::parse_probe` (the class driver), `pixel_core` (the
//! shared decoder) and, on x86 `wc` only, `video::wm` as a kernel app does (create / present / close of its own
//! row). It never calls `pal::push_event` (a camera is a capture device, not an input decoder — BP-input-14) and
//! never touches a register. The viewer is x86 `wc` only: the rMBP is the bench; an aarch64 image compiles the
//! census, the converters and the fixture, and its stream is counted by `tests uvc`.
//!
//! # THE CALLERS IN `drivers/uvc.rs`
//!
//! `arm` / `disarm` / `census` are called from `drivers/uvc.rs` (`stream_arm`, `stream_disarm`, after
//! `print_census` in `probe_iso`) by same-line folds; `probe_iso` commits an uncompressed format whose default
//! frame fits the 4 MiB store, else the first MJPEG format (FourCC `MJPG`), which this file decodes.

use crate::drivers::uvc::{self as class, Census, Kind};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

// ── M1: who the camera is, and its one-line census ──────────────────────────────────────────────────────

const IDENT_MAX: usize = 8;
/// (slot, vid, pid) of devices the controller enumerated as camera candidates. Slot 0 = free.
static IDENT: crate::sync::Mutex<[(u8, u16, u16); IDENT_MAX]> = crate::sync::Mutex::new([(0, 0, 0); IDENT_MAX]);
static IDENT_NEXT: AtomicU8 = AtomicU8::new(0);

/// The controller's enumeration paths (root "no driver for device class", hub-downstream walk): remember the
/// slot's identity so the census line can name the camera. Costs no traffic; a slot re-enumerated overwrites.
pub fn note_dev(slot: u8, vid: u16, pid: u16) {
    if slot == 0 {
        return;
    }
    let mut t = IDENT.lock();
    let i = match t.iter().position(|e| e.0 == slot) {
        Some(i) => i,
        None => match t.iter().position(|e| e.0 == 0) {
            Some(i) => i,
            None => IDENT_NEXT.fetch_add(1, Ordering::Relaxed) as usize % IDENT_MAX,
        },
    };
    t[i] = (slot, vid, pid);
}

fn ident_of(slot: u8) -> Option<(u16, u16)> {
    IDENT.lock().iter().find(|e| e.0 == slot && slot != 0).map(|e| (e.1, e.2))
}

/// `dwFrameInterval` is in 100 ns units (UVC 1.1 §3.9.2.6 frame descriptors, Table 3-2 of both payload specs).
fn fps_of(interval: u32) -> u32 {
    if interval == 0 { 0 } else { 10_000_000 / interval }
}

/// The census line for `c`, `id` = (vid, pid). PURE (the fixture compares it byte for byte).
pub fn census_line(out: &mut String, id: Option<(u16, u16)>, c: &Census) {
    out.clear();
    match id {
        Some((v, p)) => {
            let _ = write!(out, "[uvc] dev={:04x}:{:04x}", v, p);
        }
        None => out.push_str("[uvc] dev=-"),
    }
    let _ = write!(out, " formats={}", c.nfmt);
    let mut frames = 0usize;
    for f in c.fmts[..c.nfmt.min(c.fmts.len())].iter() {
        frames += f.nframes;
        out.push(' ');
        match f.kind {
            Kind::Mjpeg => out.push_str("mjpeg"),
            Kind::Other => out.push_str("other"),
            Kind::Uncompressed => {
                // The FourCC is the GUID's first four bytes (Uncompressed payload §3.1.1); lower-cased, and only
                // [0-9a-z] reach the wire so a malformed GUID cannot inject bytes into the log.
                let mut any = false;
                for b in f.guid[..4].iter() {
                    let ch = b.to_ascii_lowercase();
                    if ch.is_ascii_alphanumeric() {
                        out.push(ch as char);
                        any = true;
                    }
                }
                if !any {
                    out.push_str("raw");
                }
            }
        }
        let fr = f.frames[..f.nframes.min(f.frames.len())].iter().find(|fr| fr.index == f.default_frame).or(f.frames[..f.nframes.min(f.frames.len())].first());
        match fr {
            Some(fr) => {
                let _ = write!(out, "={}x{}@{}", fr.width, fr.height, fps_of(fr.default_interval));
            }
            None => out.push_str("=none"),
        }
    }
    let _ = write!(out, " frames={}", frames);
    if c.ran_short || c.overflowed {
        let _ = write!(out, " ran_short={} overflowed={}", c.ran_short as u8, c.overflowed as u8);
    }
}

/// `probe_iso`, after its full census: one line naming the camera and its formats.
pub fn census(slot: u8, c: &Census) {
    let mut s = String::new();
    census_line(&mut s, ident_of(slot), c);
    serial_println!("{}", s);
}

// ── The armed stream, as the viewer sees it ─────────────────────────────────────────────────────────────

static ARMED: AtomicBool = AtomicBool::new(false);
/// Bumped at every arm, so a viewer started for one stream never presents another's frames.
static GENERATION: AtomicU32 = AtomicU32::new(0);
static GEOM: AtomicU32 = AtomicU32::new(0);
static FOURCC: AtomicU32 = AtomicU32::new(0);
static SLOT: AtomicU8 = AtomicU8::new(0);
/// PTPCAM (rmbp-ledger B687): where the viewer's frames come from — the UVC stream's assembler, or a Sony body in
/// PC-Remote mode over PTP (`ptp::liveview_frame`, one JPEG per GetObject(0xFFFFC002)). One viewer, one window.
const SRC_UVC: u8 = 0;
const SRC_PTP: u8 = 1;
static SOURCE: AtomicU8 = AtomicU8::new(SRC_UVC);
/// PTPCAM: Space in the Camera window (a PTP source) asks the viewer task to fire the shutter — the key path
/// never waits on the cable; the task, which already owns the camera's cadence, does.
static SHUTTER_REQ: AtomicBool = AtomicBool::new(false);

/// Camera frames the viewer saw complete (sequence advance), frames it presented, frames it never showed,
/// frames its decoder or converter refused.
static CAM: AtomicU64 = AtomicU64::new(0);
static SHOWN: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static BAD: AtomicU64 = AtomicU64::new(0);

/// `uvc::stream_arm`: the committed stream is `w x h`, FourCC `cc` (`MJPG` for a Motion-JPEG commit).
pub fn arm(slot: u8, w: u16, h: u16, cc: [u8; 4]) {
    SOURCE.store(SRC_UVC, Ordering::Relaxed);
    GEOM.store(((w as u32) << 16) | h as u32, Ordering::Relaxed);
    FOURCC.store(u32::from_le_bytes(cc), Ordering::Relaxed);
    SLOT.store(slot, Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::AcqRel);
    ARMED.store(true, Ordering::Release);
    serial_println!("[uvc] view armed slot={} {}x{} fourcc={}", slot, w, h, cc_str(cc).as_str());
    cam_opened(slot); // CAMDOT (B702): the stream count the menu bar's camera dot is painted from
}

/// `uvc::stream_disarm`: the stream is gone. The viewer closes its window on its next pass.
pub fn disarm() {
    if SOURCE.load(Ordering::Relaxed) == SRC_UVC {
        if ARMED.swap(false, Ordering::AcqRel) {
            cam_closed(); // CAMDOT (B702): the count drops with the driver's own disarm, never on an app's word
        }
    }
}

/// PTPCAM: arm the viewer on a PTP live-view source (the session opened on a Sony body in PC-Remote mode, or the
/// `ptp view` verb). The frames are JPEGs, so the geometry comes from the first decode.
pub fn arm_ptp(slot: u8) {
    SOURCE.store(SRC_PTP, Ordering::Relaxed);
    GEOM.store(0, Ordering::Relaxed);
    FOURCC.store(u32::from_le_bytes(*b"MJPG"), Ordering::Relaxed);
    SLOT.store(slot, Ordering::Relaxed);
    SHUTTER_REQ.store(false, Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::AcqRel);
    ARMED.store(true, Ordering::Release);
    serial_println!("[uvc] view armed slot={} source=ptp liveview", slot);
    cam_opened(slot); // CAMDOT (B702): a PTP live view is a camera stream too
}

/// PTPCAM: the PTP camera went away; a UVC-armed viewer is left alone.
pub fn disarm_ptp() {
    if SOURCE.load(Ordering::Relaxed) == SRC_PTP {
        if ARMED.swap(false, Ordering::AcqRel) {
            cam_closed(); // CAMDOT (B702)
        }
    }
}

/// PTPCAM: a key offered to the Camera window (the desktop's key chain): Space fires the shutter of a PTP source
/// while the window holds focus. `false` = not ours, the chain goes on.
pub fn key_route(ev: crate::pal::Event) -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    return view::key_route(ev);
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        let _ = ev;
        false
    }
}

/// The controller's service ladder (beside `service_uvc`): start the viewer for an armed stream. Atomics only
/// unless a viewer has to be started; never decodes, never presents.
#[inline]
pub fn pump() {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    view::pump();
}

fn cc_str(cc: [u8; 4]) -> String {
    cc.iter().map(|b| if (0x20..0x7f).contains(b) { *b as char } else { '.' }).collect()
}

// ── M3: the pixels (pure — the fixture drives the same functions the viewer does) ────────────────────────

/// What a frame's bytes are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pix {
    Yuy2,
    Nv12,
    Mjpeg,
    Unsupported,
}

/// SOI (`FF D8`, T.81 Table B.1) first: a frame that IS a JPEG is decoded as one whatever the commit named.
pub fn pix_of(cc: [u8; 4], frame: &[u8]) -> Pix {
    if frame.len() >= 2 && frame[0] == 0xFF && frame[1] == 0xD8 {
        return Pix::Mjpeg;
    }
    match &cc {
        b"YUY2" | b"YUYV" => Pix::Yuy2,
        b"NV12" => Pix::Nv12,
        b"MJPG" => Pix::Mjpeg,
        _ => Pix::Unsupported,
    }
}

/// 0RGB, the window surface's word (the wm's ARGB8888 with the alpha byte unused).
#[inline]
pub fn pack(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

#[inline]
fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Y'CbCr → R'G'B', ITU-R BT.601 (SMPTE 170M) limited range, 8-bit fixed point:
/// R = 1.164(Y-16) + 1.596(Cr-128); G = 1.164(Y-16) - 0.392(Cb-128) - 0.813(Cr-128); B = 1.164(Y-16) + 2.017(Cb-128),
/// scaled by 256 (298, 409, 100, 208, 516) with rounding.
#[inline]
pub fn yuv(y: u8, u: u8, v: u8) -> u32 {
    let c = 298 * (y as i32 - 16);
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    pack(clamp8((c + 409 * e + 128) >> 8), clamp8((c - 100 * d - 208 * e + 128) >> 8), clamp8((c + 516 * d + 128) >> 8))
}

/// Bytes a whole `w x h` frame of `px` needs (Uncompressed payload §2: YUY2 is 4:2:2 packed, 2 bytes a pixel;
/// NV12 is the Y plane then one interleaved Cb/Cr plane at half height).
pub fn raw_len(px: Pix, w: usize, h: usize) -> usize {
    match px {
        Pix::Yuy2 => w * h * 2,
        Pix::Nv12 => w * h + w * h.div_ceil(2),
        _ => 0,
    }
}

/// The column map of a nearest-sampled `sw` → `dw` row, into `xs` (reused across frames).
fn column_map(xs: &mut Vec<usize>, sw: usize, dw: usize) -> bool {
    xs.clear();
    if xs.try_reserve(dw).is_err() {
        return false;
    }
    for c in 0..dw {
        xs.push((c * sw / dw).min(sw - 1));
    }
    true
}

/// Nearest-sample a YUY2 / NV12 frame `src` (`sw x sh`) into `dst` (`dw x dh`, stride `dw`). `false` (and `dst`
/// untouched) when the frame is shorter than its geometry says or the format is not raw.
pub fn sample_raw(px: Pix, src: &[u8], sw: usize, sh: usize, dst: &mut [u32], dw: usize, dh: usize, xs: &mut Vec<usize>) -> bool {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || dst.len() < dw * dh || (px == Pix::Nv12 && sw % 2 != 0) {
        return false;
    }
    let need = raw_len(px, sw, sh);
    if need == 0 || src.len() < need || !column_map(xs, sw, dw) {
        return false;
    }
    for r in 0..dh {
        let sy = (r * sh / dh).min(sh - 1);
        let row = &mut dst[r * dw..r * dw + dw];
        match px {
            Pix::Yuy2 => {
                // Y0 Cb Y1 Cr per pixel pair (the macropixel).
                let base = sy * sw * 2;
                for (c, out) in row.iter_mut().enumerate() {
                    let sx = xs[c];
                    let m = base + (sx & !1) * 2;
                    *out = yuv(src[m + (sx & 1) * 2], src[m + 1], src[m + 3]);
                }
            }
            Pix::Nv12 => {
                let yb = sy * sw;
                let cb = sw * sh + (sy / 2) * sw;
                for (c, out) in row.iter_mut().enumerate() {
                    let sx = xs[c];
                    let k = cb + (sx & !1);
                    *out = yuv(src[yb + sx], src[k], src[k + 1]);
                }
            }
            _ => return false,
        }
    }
    true
}

/// Nearest-sample a decoded RGBA image (`pixel_core::Image::rgba`) into `dst`.
pub fn sample_rgba(rgba: &[u8], sw: usize, sh: usize, dst: &mut [u32], dw: usize, dh: usize, xs: &mut Vec<usize>) -> bool {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || dst.len() < dw * dh || rgba.len() < sw * sh * 4 || !column_map(xs, sw, dw) {
        return false;
    }
    for r in 0..dh {
        let sy = (r * sh / dh).min(sh - 1);
        let srow = &rgba[sy * sw * 4..(sy + 1) * sw * 4];
        for (c, out) in dst[r * dw..r * dw + dw].iter_mut().enumerate() {
            let p = &srow[xs[c] * 4..xs[c] * 4 + 3];
            *out = pack(p[0], p[1], p[2]);
        }
    }
    true
}

/// The window's content size for a `sw x sh` picture in a `maxw x maxh` box: aspect kept, never upscaled.
pub fn fit(sw: usize, sh: usize, maxw: usize, maxh: usize) -> (usize, usize) {
    if sw == 0 || sh == 0 || maxw == 0 || maxh == 0 {
        return (0, 0);
    }
    let mut w = sw.min(maxw);
    let mut h = (sh * w / sw).max(1);
    if h > maxh {
        h = maxh;
        w = (sw * h / sh).max(1);
    }
    (w, h)
}

// ── The viewer (x86 `wc`: the bench's window system) ────────────────────────────────────────────────────

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod view {
    use super::*;
    use crate::video::wm;

    /// A kernel row of its own (`wm::is_kernel_owner`'s band; 0x62 is unused by every other kernel window).
    const OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x62;
    /// `uvc-view`'s stack: the JPEG decoder's state is heap-backed; 128 KiB is its call chain with margin.
    const STACK: usize = 128 * 1024;
    /// The pass: 8 ms, so a 60 fps camera is never sampled twice per frame late; the task exists only while a
    /// stream is armed, so an idle machine pays nothing.
    const PASS_MS: u64 = 8;
    /// The stream line's period.
    const STAT_MS: u64 = 5_000;

    static TASK: AtomicBool = AtomicBool::new(false);
    static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);

    pub fn key_route(ev: crate::pal::Event) -> bool {
        let id = WIN.load(Ordering::Acquire);
        if id == wm::WIN_NONE || SOURCE.load(Ordering::Relaxed) != SRC_PTP || wm::focus_asid() != OWNER {
            return false;
        }
        match ev {
            crate::pal::Event::Key(b' ') => {
                SHUTTER_REQ.store(true, Ordering::Release);
                true
            }
            _ => false,
        }
    }

    /// CAMDOT (B702): a viewer task is alive (it, not [`super::revoked`], stops the stream it holds).
    pub fn running() -> bool {
        TASK.load(Ordering::Acquire)
    }

    pub fn pump() {
        if ARMED.load(Ordering::Acquire) && !TASK.swap(true, Ordering::AcqRel) {
            crate::arch::sched::spawn_stack("uvc-view", task, 0, crate::arch::sched::CPU_AUTO, crate::arch::sched::PRIO_NORMAL, STACK);
        }
    }

    /// The window's outer box limit: two thirds of the work area (the panel less the bar and the dock rows).
    fn bounds() -> Option<(usize, usize, usize, usize)> {
        let (pw, ph) = {
            let fb = crate::video::WRITER.lock();
            if !fb.is_ready() {
                return None;
            }
            let i = fb.info();
            (i.width, i.height)
        };
        let top = crate::ui_status::top_chrome_h(pw, ph);
        let work = ph.saturating_sub(top).saturating_sub(crate::ui_status::chrome_h(ph));
        let maxw = (pw * 2 / 3).saturating_sub(2 * wm::BORDER());
        let maxh = (work * 2 / 3).saturating_sub(wm::TITLE_H() + 2 * wm::BORDER());
        Some((pw, ph, maxw, maxh))
    }

    struct Win {
        id: wm::WinId,
        surf: Vec<u32>,
        w: usize,
        h: usize,
        ox: usize,
        oy: usize,
    }

    /// Size the window for a `sw x sh` picture and allocate its surface. The ROW is created by [`create`] only after
    /// the first frame is in the surface, so the glass never shows an empty box.
    fn open(sw: usize, sh: usize) -> Result<Win, &'static str> {
        let (pw, ph, maxw, maxh) = bounds().ok_or("no-panel")?;
        let (w, h) = fit(sw, sh, maxw, maxh);
        if w < 16 || h < 16 {
            return Err("panel-cannot-seat");
        }
        let mut surf: Vec<u32> = Vec::new();
        if surf.try_reserve_exact(w * h).is_err() {
            return Err("alloc");
        }
        surf.resize(w * h, 0);
        let (_s, ow, oh) = wm::spawn_geometry_native(w, h).ok_or("geometry-unavailable")?;
        let top = crate::ui_status::top_chrome_h(pw, ph);
        let ox = pw.saturating_sub(ow) / 2;
        let oy = top + ph.saturating_sub(top).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 3;
        Ok(Win { id: wm::WIN_NONE, surf, w, h, ox, oy })
    }

    fn create(win: &mut Win) -> bool {
        let base = win.surf.as_mut_ptr() as usize;
        let id = wm::create_at_native(OWNER, base, win.w * win.h * 4, win.w as u32, win.h as u32, (win.w * 4) as u32, b"Camera", win.ox + wm::BORDER(), win.oy + wm::TITLE_H() + wm::BORDER());
        if id == wm::WIN_NONE {
            return false;
        }
        win.id = id;
        WIN.store(id, Ordering::Release);
        wm::winid_register_holder(&WIN, "uvc-view");
        true
    }

    /// The row goes first (the compositor stops reading the surface), then the surface is dropped by the caller.
    fn close(win: &mut Option<Win>) {
        let id = WIN.swap(wm::WIN_NONE, Ordering::AcqRel);
        if id != wm::WIN_NONE {
            wm::close(id);
        }
        *win = None;
    }

    /// CAMDOT (B702) M2: THE ask — `<app> wants the camera`, Not now · Allow (the default, rightmost; Escape and the
    /// close box are Not now). QUEUE-ONLY (`dialog::post`); the answer arrives on the press route in [`cam_answer`].
    fn cam_ask() -> bool {
        use crate::video::dialog::{self, Act, Dlg, Icon};
        let msg = alloc::format!("{} wants the camera", CAM_APP);
        let mut d = Dlg::new(Icon::Caution, b"Camera", msg.as_bytes(), b"The camera dot in the menu bar shows while it is on.\nSettings > Privacy revokes it.", &[b"Not now", b"Allow"]);
        d.act = Act::Hook(cam_answer as fn(bool) as usize);
        let posted = dialog::post(d);
        if posted {
            ASK_UP.store(true, Ordering::Release);
            ASKS.fetch_add(1, Ordering::Relaxed);
        }
        serial_println!("[cam] ask app={} domain={} posted={}", CAM_APP, CAM_DOMAIN, posted as u8);
        posted
    }

    /// The dialog's hook (press/key route; queue-only): Allow is KEPT in Principia (`prefs::decide`, queue-only); Not
    /// now is not kept — the next stream asks again.
    fn cam_answer(ok: bool) {
        ASK_UP.store(false, Ordering::Release);
        let a = answer_of(ok, crate::video::dialog::last_button());
        if a == ANS_ALLOW {
            crate::prefs::decide(CAM_DOMAIN, CAM_APP, "-", ANS_ALLOW_WORD);
        }
        serial_println!("[cam] answer={} app={}", if a == ANS_ALLOW { ANS_ALLOW_WORD } else { "not-now" }, CAM_APP);
        ANSWER.store(a, Ordering::Release);
    }

    fn task(_: usize) {
        let generation = GENERATION.load(Ordering::Acquire);
        let g = GEOM.load(Ordering::Relaxed);
        let (gw, gh) = ((g >> 16) as usize, (g & 0xFFFF) as usize);
        let cc = FOURCC.load(Ordering::Relaxed).to_le_bytes();
        let ptp_src = SOURCE.load(Ordering::Relaxed) == SRC_PTP;
        let mut said_ptp_err = false;
        serial_println!("[uvc] view start slot={} {}x{} fourcc={}", SLOT.load(Ordering::Relaxed), gw, gh, cc_str(cc).as_str());
        let mut copy: Vec<u8> = Vec::new();
        let mut want = 0usize;
        let mut xs: Vec<usize> = Vec::new();
        let mut last_seq = 0u32;
        let mut win: Option<Win> = None;
        let mut user_closed = false;
        let mut said_unsupported = false;
        let mut said_err = false;
        let t0 = crate::arch::ms();
        let mut t_stat = t0;
        let mut cam_stat = CAM.load(Ordering::Relaxed);
        let mut render_us_max = 0u64;
        // CAMDOT (B702) M2: the app's grant — Principia's kept answer first, else ONE ask; no frame is read before it.
        let mut granted = cam_consult();
        let mut ask_try: Option<u64> = None;
        let why = loop {
            if !ARMED.load(Ordering::Acquire) {
                break "stream-disarmed";
            }
            if GENERATION.load(Ordering::Acquire) != generation {
                break "stream-rearmed";
            }
            if REVOKED.load(Ordering::Acquire) {
                break "revoked";
            }
            if !granted {
                match ANSWER.load(Ordering::Acquire) {
                    ANS_ALLOW => {
                        granted = true;
                        cam_granted("ask");
                    }
                    ANS_NOT_NOW => break "not-now",
                    _ => {
                        let now = crate::arch::ms();
                        let due = ask_try.map(|t| now.wrapping_sub(t) >= ASK_RETRY_MS).unwrap_or(true);
                        if due && !ASK_UP.load(Ordering::Acquire) && crate::boot::phase() == crate::boot::Phase::Desktop {
                            ask_try = Some(now);
                            let _ = cam_ask();
                        }
                        crate::arch::sched::sleep_ms(PASS_MS);
                        continue;
                    }
                }
            }
            // A user close (the close disc → `wm::close` clears `WIN` through the holder registry): drop the
            // surface now that no row reads it, and stay closed for this stream.
            if win.is_some() && WIN.load(Ordering::Acquire) == wm::WIN_NONE {
                win = None;
                user_closed = true;
                serial_println!("[uvc] view closed by user — frames still counted");
                if ptp_src {
                    disarm_ptp(); // PTPCAM: a closed live view stops pulling frames over the cable; `ptp view` reopens it
                    break "closed-by-user";
                }
            }
            // COPY OUT under the stream lock (a memcpy, never an allocation: the claim path waits on this lock).
            let mut got: Option<u32> = None;
            if ptp_src {
                // PTPCAM: the shutter first (Space in the window), then one live-view JPEG over the cable. The fetch IS
                // the cadence: GetObject returns when the camera has a frame.
                if SHUTTER_REQ.swap(false, Ordering::AcqRel) {
                    match crate::drivers::xhci::ptp::shutter() {
                        Ok(added) => serial_println!("[uvc] view shutter fired source=ptp object_added={}", added as u8),
                        Err(e) => serial_println!("[uvc] view shutter refused: {}", e),
                    }
                }
                match crate::drivers::xhci::ptp::liveview_frame(0) {
                    Ok(f) => {
                        copy.clear();
                        if copy.try_reserve(f.len()).is_err() {
                            break "alloc-copy";
                        }
                        copy.extend_from_slice(&f);
                        got = Some(last_seq.wrapping_add(1).max(1));
                    }
                    Err(e) => {
                        if !crate::drivers::xhci::ptp::is_open() {
                            break "ptp-session-closed";
                        }
                        BAD.fetch_add(1, Ordering::Relaxed);
                        if !said_ptp_err {
                            said_ptp_err = true;
                            serial_println!("[uvc] view ptp frame refused: {} (first refusal; counted as bad=)", e);
                        }
                        crate::arch::sched::sleep_ms(50);
                    }
                }
            } else {
            let _ = class::latest_frame(&mut |b, seq| {
                if seq == last_seq {
                    return;
                }
                if copy.capacity() < b.len() {
                    want = b.len();
                    return;
                }
                copy.clear();
                copy.extend_from_slice(b);
                got = Some(seq);
            });
            if want > copy.capacity() {
                copy.clear();
                if copy.try_reserve_exact(want).is_err() {
                    break "alloc-copy";
                }
                want = 0;
                continue;
            }
            }
            if let Some(seq) = got {
                let step = if last_seq == 0 { 1 } else { seq.wrapping_sub(last_seq) as u64 };
                CAM.fetch_add(step, Ordering::Relaxed);
                if last_seq != 0 && step > 1 {
                    DROPPED.fetch_add(step - 1, Ordering::Relaxed);
                }
                last_seq = seq;
                if !user_closed {
                    let r0 = crate::arch::now_cycles();
                    let px = pix_of(cc, &copy);
                    let mut img: Option<pixel_core::Image> = None;
                    let (sw, sh) = match px {
                        Pix::Mjpeg => match pixel_core::decode_jpeg(&copy) {
                            Ok(i) => {
                                let d = (i.width as usize, i.height as usize);
                                img = Some(i);
                                d
                            }
                            Err(_) => (0, 0),
                        },
                        Pix::Yuy2 | Pix::Nv12 => (gw, gh),
                        Pix::Unsupported => (0, 0),
                    };
                    if px == Pix::Unsupported {
                        if !said_unsupported {
                            said_unsupported = true;
                            serial_println!("[uvc] view unsupported fourcc={} bytes={} — counted, not shown", cc_str(cc).as_str(), copy.len());
                        }
                    } else if sw == 0 {
                        BAD.fetch_add(1, Ordering::Relaxed);
                        if !said_err {
                            said_err = true;
                            serial_println!("[uvc] view decode refused fourcc={} bytes={} (first refusal; counted as bad=)", cc_str(cc).as_str(), copy.len());
                        }
                    } else {
                        if win.is_none() {
                            match open(sw, sh) {
                                Ok(w) => win = Some(w),
                                Err(e) => break e,
                            }
                        }
                        let w = win.as_mut().unwrap();
                        let ok = match &img {
                            Some(i) => sample_rgba(&i.rgba, sw, sh, &mut w.surf, w.w, w.h, &mut xs),
                            None => sample_raw(px, &copy, sw, sh, &mut w.surf, w.w, w.h, &mut xs),
                        };
                        if !ok {
                            BAD.fetch_add(1, Ordering::Relaxed);
                        } else if w.id == wm::WIN_NONE {
                            if !create(w) {
                                break "create-failed";
                            }
                            serial_println!("[uvc] view open win={} src={}x{} shown={}x{} fourcc={}", w.id, sw, sh, w.w, w.h, cc_str(cc).as_str());
                            SHOWN.fetch_add(1, Ordering::Relaxed);
                        } else if WIN.load(Ordering::Acquire) == w.id {
                            let _ = wm::present(w.id);
                            SHOWN.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    let dt = crate::arch::now_cycles().wrapping_sub(r0);
                    let hz = crate::arch::apic::tsc_hz();
                    if hz != 0 {
                        render_us_max = render_us_max.max(dt.saturating_mul(1_000_000) / hz);
                    }
                }
            }
            let now = crate::arch::ms();
            if now.wrapping_sub(t_stat) >= STAT_MS {
                let cam = CAM.load(Ordering::Relaxed);
                let fps = (cam - cam_stat) * 1000 / now.wrapping_sub(t_stat).max(1);
                let (sw, sh) = win.as_ref().map(|w| (w.w, w.h)).unwrap_or((0, 0));
                serial_println!(
                    "[uvc] stream fps={} frames={} dropped={} bad={} src={}x{} fourcc={} shown={}x{} render_us_max={} window={}",
                    fps, SHOWN.load(Ordering::Relaxed), DROPPED.load(Ordering::Relaxed), BAD.load(Ordering::Relaxed),
                    gw, gh, cc_str(cc).as_str(), sw, sh, render_us_max,
                    if user_closed { "closed-by-user" } else if win.is_some() { "open" } else { "none-yet" }
                );
                t_stat = now;
                cam_stat = cam;
                render_us_max = 0;
            }
            crate::arch::sched::sleep_ms(if ptp_src { 1 } else { PASS_MS });
        };
        close(&mut win);
        if why == "revoked" || why == "not-now" {
            stop_stream(why); // CAMDOT (B702): the user's No / Revoke ends the stream itself, so the dot goes out with it
        }
        serial_println!(
            "[uvc] view end reason={} frames={} dropped={} bad={} camera_frames={} ms={}",
            why, SHOWN.load(Ordering::Relaxed), DROPPED.load(Ordering::Relaxed), BAD.load(Ordering::Relaxed),
            CAM.load(Ordering::Relaxed), crate::arch::ms().wrapping_sub(t0)
        );
        TASK.store(false, Ordering::Release);
    }
}

// ── CAMDOT (rmbp-ledger B702, R130, BP-bar-10/12): the in-use light and the grant ─────────────────────────────────
//
// M1 THE COUNT. A stream is OPEN from the class driver's `stream_arm` (the assembler takes the camera's bytes) to its
// `stream_disarm` — [`arm`] / [`disarm`] above, the driver's own two calls. [`open_streams`] is that count (the class
// driver holds ONE stream, so 0 or 1); the menu bar's camera item (`video::menubar::camdot`) reads it on every
// composite and paints the camera glyph and the accent dot while it is not 0. No app holds a handle on the dot: an
// app can neither paint it nor hide it, and it is on from the first byte the assembler keeps — before the ask is
// answered — so a picture can never reach RAM with the dot off. Microphone: no seam (HDA has no capture stream in
// the tree; the codec walk's `capture` reads amp caps only), so there is no mic item.
//
// M2 THE GRANT. The app is the viewer (`uvc-view`, the window "Camera"). Before it copies one frame it consults
// Principia (`prefs::decided("camera", "Camera", "-")`, key `system.decisions.camera.Camera-<fnv64>`); with no kept
// answer it raises ONE ask (`<app> wants the camera`: Not now · Allow). Allow is kept (`prefs::decide`) and listed on
// Settings > Privacy with every other decision (REASK's rows); Not now is not kept, and the stream is stopped. A
// Revoke there calls [`revoked`] BEFORE the store write; the viewer sees it on its next 8 ms pass, closes its window
// and stops the stream (`uvc::stream_disarm`), so the dot goes out with it. With no viewer alive, [`revoked`]
// stops the stream itself.
//
// M3 THE WIRE. `[cam] stream owner=<uvc or uvc-view or -> open=<n> dot=<on or off>` at every change of owner or
// count (the judge's answer); the painter's own edge `[cam] dot painted=<on or off> owner= open= x=`;
// `[cam] revoke app= stopped_ms=`; `tests camdot`.

/// The app the grant is for (the viewer's window title).
pub const CAM_APP: &str = "Camera";
/// The decisions' `<domain>` segment (`system.decisions.camera.*`).
pub const CAM_DOMAIN: &str = "camera";
/// The kept answer's word.
const ANS_ALLOW_WORD: &str = "allow";
/// The ask's state: none yet, Allow, Not now.
const ANS_NONE: u8 = 0;
const ANS_ALLOW: u8 = 1;
const ANS_NOT_NOW: u8 = 2;
/// A post the dialog queue refused (one already pending) is retried after this long.
#[allow(dead_code)]
const ASK_RETRY_MS: u64 = 500;
/// "Within one frame": a revoke stops the stream inside one 60 Hz frame.
const REVOKE_FRAME_MS: u64 = 17;
/// Who holds the open stream: nobody, the driver (no app granted yet), the viewer.
const OWNER_NONE: u8 = 0;
const OWNER_DRIVER: u8 = 1;
const OWNER_VIEW: u8 = 2;

static ANSWER: AtomicU8 = AtomicU8::new(ANS_NONE);
#[allow(dead_code)]
static ASK_UP: AtomicBool = AtomicBool::new(false);
#[allow(dead_code)]
static ASKS: AtomicU32 = AtomicU32::new(0);
static REVOKED: AtomicBool = AtomicBool::new(false);
static REVOKE_AT: AtomicU64 = AtomicU64::new(0);
/// The last revoke's ms from [`revoked`] to the stream's disarm (`u64::MAX` = none yet).
static REVOKE_MS: AtomicU64 = AtomicU64::new(u64::MAX);
/// Streams opened since boot (the fixture's `opens=`).
static OPENS: AtomicU32 = AtomicU32::new(0);
static OWNER: AtomicU8 = AtomicU8::new(OWNER_NONE);

/// **THE COUNT the menu bar paints from**: open UVC streams (relaxed load; legal from `strip::compose_all`).
#[inline]
pub fn open_streams() -> u32 {
    ARMED.load(Ordering::Acquire) as u32
}

/// The owning task's name for the wire.
pub fn owner_name() -> &'static str {
    match OWNER.load(Ordering::Relaxed) {
        OWNER_DRIVER => "uvc",
        OWNER_VIEW => "uvc-view",
        _ => "-",
    }
}

/// The judge: the dot is on while any stream is open. PURE.
pub fn dot_word(open: u32) -> &'static str {
    if open != 0 { "on" } else { "off" }
}

/// The ask's answer from the dialog's `(ok, button)`: the default (Allow, index 1) grants; anything else is Not now.
/// PURE.
pub fn answer_of(ok: bool, ix: u8) -> u8 {
    if ok && ix == 1 { ANS_ALLOW } else { ANS_NOT_NOW }
}

fn say_stream() {
    let n = open_streams();
    serial_println!("[cam] stream owner={} open={} dot={}", owner_name(), n, dot_word(n));
}

fn cam_opened(slot: u8) {
    REVOKED.store(false, Ordering::Release);
    OWNER.store(OWNER_DRIVER, Ordering::Relaxed);
    let n = OPENS.fetch_add(1, Ordering::Relaxed) + 1;
    serial_println!("[cam] open slot={} opens={}", slot, n);
    say_stream();
}

fn cam_closed() {
    OWNER.store(OWNER_NONE, Ordering::Relaxed);
    say_stream();
}

/// The viewer's first question: is the grant kept? (`[decision] consult …` on the wire when it is.)
#[allow(dead_code)]
fn cam_consult() -> bool {
    ANSWER.store(ANS_NONE, Ordering::Release);
    let kept = crate::prefs::decided(CAM_DOMAIN, CAM_APP, "-").as_deref() == Some(ANS_ALLOW_WORD);
    if kept {
        cam_granted("decision");
    }
    kept
}

#[allow(dead_code)]
fn cam_granted(via: &str) {
    OWNER.store(OWNER_VIEW, Ordering::Relaxed);
    serial_println!("[cam] grant app={} answer={} via={}", CAM_APP, ANS_ALLOW_WORD, via);
    say_stream();
}

/// Settings > Privacy's Revoke (the latched key, before the store write): the camera's decision stops the stream.
pub fn revoked(key: &str) {
    if key != crate::prefs::decision_key(CAM_DOMAIN, CAM_APP) {
        return;
    }
    if !ARMED.load(Ordering::Acquire) {
        serial_println!("[cam] revoke app={} open=0 (nothing streaming; the next stream asks)", CAM_APP);
        return;
    }
    REVOKE_AT.store(crate::arch::ms(), Ordering::Relaxed);
    REVOKED.store(true, Ordering::Release);
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    if view::running() {
        return; // the viewer stops it on its next pass (it holds the frames and the window)
    }
    stop_stream("revoked");
}

/// End the stream (the class driver's own disarm: the assembler and its buffers go, the count drops, the dot goes
/// out). Task / service context only — never the claim path.
fn stop_stream(why: &str) {
    class::stream_disarm();
    if REVOKED.swap(false, Ordering::AcqRel) {
        let ms = crate::arch::ms().wrapping_sub(REVOKE_AT.load(Ordering::Relaxed));
        REVOKE_MS.store(ms, Ordering::Relaxed);
        serial_println!("[cam] revoke app={} stopped_ms={} why={}", CAM_APP, ms, why);
    } else {
        serial_println!("[cam] stop app={} why={}", CAM_APP, why);
    }
}

// ── `tests uvccam` ──────────────────────────────────────────────────────────────────────────────────────

/// A two-format camera's configuration descriptor, 285 bytes, written from the tables (USB 2.0 Tables 9-10, 9-12,
/// 9-13 and the IAD ECN; UVC 1.1 Tables 3-3 VC header, 3-6 camera terminal, 3-8 processing unit, 3-5 output
/// terminal, 3-13 VS input header, 3-19 colour matching; the MJPEG and Uncompressed payload specs' Tables 3-1 /
/// 3-2): IAD (interfaces 0..1) · VC { header bcdUVC 1.10, 48 MHz, total 51 · camera terminal 1 · processing unit 2
/// · output terminal 3 } · VS alt 0 { input header 2 formats ep 0x81 total 183 · MJPEG fmt 1 { 1920x1080 @
/// 333333 / 666666 (default) · 1280x720 @ 333333 } · YUY2 fmt 2 { 640x480 @ 333333 (default) · 1280x720 @
/// 1000000 } · colour matching (BT.709 / BT.709 / SMPTE 170M) } · VS alt 1 { isochronous IN 0x81, 1024 x 3 }.
#[rustfmt::skip]
const CAMBLOB: [u8; 285] = [
    0x09, 0x02, 0x1d, 0x01, 0x02, 0x01, 0x00, 0x80, 0xfa, 0x08, 0x0b, 0x00, 0x02, 0x0e, 0x03, 0x00, 0x00, 0x09,
    0x04, 0x00, 0x00, 0x00, 0x0e, 0x01, 0x00, 0x00, 0x0d, 0x24, 0x01, 0x10, 0x01, 0x33, 0x00, 0x00, 0x6c, 0xdc,
    0x02, 0x01, 0x01, 0x12, 0x24, 0x02, 0x01, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03,
    0x0a, 0x00, 0x00, 0x0b, 0x24, 0x05, 0x02, 0x01, 0x00, 0x00, 0x02, 0x5b, 0x00, 0x00, 0x09, 0x24, 0x03, 0x03,
    0x01, 0x01, 0x00, 0x02, 0x00, 0x09, 0x04, 0x01, 0x00, 0x00, 0x0e, 0x02, 0x00, 0x00, 0x0f, 0x24, 0x01, 0x02,
    0xb7, 0x00, 0x81, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x0b, 0x24, 0x06, 0x01, 0x02, 0x01, 0x01,
    0x00, 0x00, 0x00, 0x00, 0x22, 0x24, 0x07, 0x01, 0x00, 0x80, 0x07, 0x38, 0x04, 0x00, 0x40, 0xfa, 0x01, 0x00,
    0x80, 0x53, 0x3b, 0x00, 0x48, 0x3f, 0x00, 0x15, 0x16, 0x05, 0x00, 0x02, 0x15, 0x16, 0x05, 0x00, 0x2a, 0x2c,
    0x0a, 0x00, 0x1e, 0x24, 0x07, 0x02, 0x00, 0x00, 0x05, 0xd0, 0x02, 0x00, 0x00, 0xe1, 0x00, 0x00, 0x00, 0x5e,
    0x1a, 0x00, 0x20, 0x1c, 0x00, 0x15, 0x16, 0x05, 0x00, 0x01, 0x15, 0x16, 0x05, 0x00, 0x1b, 0x24, 0x04, 0x02,
    0x02, 0x59, 0x55, 0x59, 0x32, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71, 0x10,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x1e, 0x24, 0x05, 0x01, 0x00, 0x80, 0x02, 0xe0, 0x01, 0x00, 0x00, 0x4b, 0x00,
    0x00, 0x00, 0xca, 0x08, 0x00, 0x60, 0x09, 0x00, 0x15, 0x16, 0x05, 0x00, 0x01, 0x15, 0x16, 0x05, 0x00, 0x1e,
    0x24, 0x05, 0x02, 0x00, 0x00, 0x05, 0xd0, 0x02, 0x00, 0x00, 0xe1, 0x00, 0x00, 0x00, 0x5e, 0x1a, 0x00, 0x20,
    0x1c, 0x00, 0x40, 0x42, 0x0f, 0x00, 0x01, 0x40, 0x42, 0x0f, 0x00, 0x06, 0x24, 0x0d, 0x01, 0x01, 0x04, 0x09,
    0x04, 0x01, 0x01, 0x01, 0x0e, 0x02, 0x00, 0x00, 0x07, 0x05, 0x81, 0x05, 0x00, 0x14, 0x01,
];

/// An 8x8 baseline JPEG (the tree's `libs/lux/tests/fixtures/tiny_red.jpg`, identical to raw_core's
/// `synth_preview.jpg`) with its four DHT segments REMOVED — exactly what a Motion-JPEG camera sends (no DHT;
/// the decoder must supply T.81 Annex K.3's tables). The removed tables were K.3's own, so the picture is unchanged.
#[rustfmt::skip]
const MJPEG_NO_DHT: [u8; 200] = [
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x02, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
    0x00, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x08, 0x00, 0x08, 0x03, 0x00, 0x11, 0x00, 0x01, 0x11, 0x01, 0x02, 0x11,
    0x01, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x03, 0x02, 0x02, 0x03, 0x02, 0x02, 0x03, 0x03, 0x03, 0x03, 0x04, 0x03, 0x03,
    0x04, 0x05, 0x08, 0x05, 0x05, 0x04, 0x04, 0x05, 0x0a, 0x07, 0x07, 0x06, 0x08, 0x0c, 0x0a, 0x0c, 0x0c, 0x0b, 0x0a,
    0x0b, 0x0b, 0x0d, 0x0e, 0x12, 0x10, 0x0d, 0x0e, 0x11, 0x0e, 0x0b, 0x0b, 0x10, 0x16, 0x10, 0x11, 0x13, 0x14, 0x15,
    0x15, 0x15, 0x0c, 0x0f, 0x17, 0x18, 0x16, 0x14, 0x18, 0x12, 0x14, 0x15, 0x14, 0xff, 0xdb, 0x00, 0x43, 0x01, 0x03,
    0x04, 0x04, 0x05, 0x04, 0x05, 0x09, 0x05, 0x05, 0x09, 0x14, 0x0d, 0x0b, 0x0d, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
    0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
    0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0x14,
    0x14, 0x14, 0x14, 0x14, 0x14, 0x14, 0xff, 0xda, 0x00, 0x0c, 0x03, 0x00, 0x00, 0x01, 0x11, 0x02, 0x11, 0x00, 0x3f,
    0x00, 0xf9, 0xd2, 0xbf, 0x0c, 0x3f, 0xd5, 0x33, 0xff, 0xd9,
];

/// Register `tests uvccam` once (called from `tests::ensure_shellux`, both arches).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("uvccam", fixture);
        crate::tests::register("camdot", camdot_fixture); // CAMDOT (B702)
    }
}

/// The fixture's tally: every check names its field, the first miss is printed with what it read and wanted.
struct Score {
    fails: u32,
    first: (&'static str, u64, u64),
}

impl Score {
    fn check(&mut self, name: &'static str, got: u64, want: u64) {
        if got != want {
            if self.fails == 0 {
                self.first = (name, got, want);
            }
            self.fails += 1;
        }
    }
}

fn fixture() {
    let mut sc = Score { fails: 0, first: ("", 0, 0) };
    // ── M1: the census of the blob, field by field, then the line byte for byte.
    let c = class::parse(&CAMBLOB);
    sc.check("cfg.wTotalLength", u16::from_le_bytes([CAMBLOB[2], CAMBLOB[3]]) as u64, CAMBLOB.len() as u64);
    sc.check("census.ran_short", c.ran_short as u64, 0);
    sc.check("census.overflowed", c.overflowed as u64, 0);
    sc.check("census.iad", c.have_iad as u64, 1);
    sc.check("census.vc_total", c.vc_total_len as u64, 51);
    sc.check("census.clock", c.clock_hz as u64, 48_000_000);
    sc.check("census.vs_intf", c.vs_intf as u64, 1);
    sc.check("census.vs_ep", c.vs_ep as u64, 0x81);
    sc.check("census.formats", c.nfmt as u64, 2);
    sc.check("census.declared", c.declared_formats as u64, 2);
    let mj = c.fmts[0];
    sc.check("mjpeg.kind", (mj.kind == Kind::Mjpeg) as u64, 1);
    sc.check("mjpeg.frames", mj.nframes as u64, 2);
    sc.check("mjpeg.f1.w", mj.frames[0].width as u64, 1920);
    sc.check("mjpeg.f1.h", mj.frames[0].height as u64, 1080);
    sc.check("mjpeg.f1.intervals", mj.frames[0].n_intervals as u64, 2);
    sc.check("mjpeg.f1.iv1", mj.frames[0].intervals[1] as u64, 666_666);
    sc.check("mjpeg.f2.w", mj.frames[1].width as u64, 1280);
    let yu = c.fmts[1];
    sc.check("yuy2.kind", (yu.kind == Kind::Uncompressed) as u64, 1);
    sc.check("yuy2.fourcc", u32::from_le_bytes([yu.guid[0], yu.guid[1], yu.guid[2], yu.guid[3]]) as u64, u32::from_le_bytes(*b"YUY2") as u64);
    sc.check("yuy2.bpp", yu.bits_per_pixel as u64, 16);
    sc.check("yuy2.f1.w", yu.frames[0].width as u64, 640);
    sc.check("yuy2.f1.h", yu.frames[0].height as u64, 480);
    sc.check("yuy2.f2.default", yu.frames[1].default_interval as u64, 1_000_000);
    sc.check("alts", c.nalt as u64, 1);
    sc.check("alt1.mps", c.alts[0].mps as u64, 1024);
    sc.check("alt1.mult", c.alts[0].mult as u64, 3);
    let mut line = String::new();
    census_line(&mut line, Some((0xFEED, 0x0C0A)), &c);
    sc.check("census.line", (line.as_str() == "[uvc] dev=feed:0c0a formats=2 mjpeg=1920x1080@30 yuy2=640x480@30 frames=4") as u64, 1);
    let parse_ok = sc.fails == 0;
    // ── M2: a PROBE/COMMIT block (UVC 1.1 §4.3.1.1, Table 4-47) as the device would return it for YUY2 frame 1, read
    // by the class driver's own `parse_probe`, then resolved against the census: the frame size the block names
    // must be the frame's arithmetic size, and the payload must fit the one alternate.
    let mut pb = [0u8; 26];
    pb[0] = 0x01; // bmHint: dwFrameInterval fixed (Table 4-48)
    pb[2] = 2; // bFormatIndex
    pb[3] = 1; // bFrameIndex
    pb[4..8].copy_from_slice(&333_333u32.to_le_bytes());
    pb[18..22].copy_from_slice(&614_400u32.to_le_bytes());
    pb[22..26].copy_from_slice(&3_072u32.to_le_bytes());
    let mut probe_ok = false;
    match class::parse_probe(&pb) {
        Some(p) => {
            sc.check("probe.hint", p.hint as u64, 1);
            sc.check("probe.interval", p.frame_interval as u64, 333_333);
            let f = c.fmts[..c.nfmt].iter().find(|f| f.index == p.format_index);
            let fr = f.and_then(|f| f.frames[..f.nframes].iter().find(|fr| fr.index == p.frame_index).map(|fr| (f.bits_per_pixel, fr.width, fr.height)));
            match fr {
                Some((bpp, w, h)) => {
                    sc.check("probe.frame_bytes", (w as u64) * (h as u64) * bpp as u64 / 8, p.max_video_frame_size as u64);
                    let esit = c.alts[0].mps as u64 * c.alts[0].mult as u64;
                    sc.check("probe.payload_fits", (p.max_payload_transfer_size as u64 <= esit) as u64, 1);
                    probe_ok = true;
                }
                None => sc.check("probe.resolves", 0, 1),
            }
            sc.check("probe.runt_refused", class::parse_probe(&pb[..25]).is_none() as u64, 1);
        }
        None => sc.check("probe.parsed", 0, 1),
    }
    let probe_ok = probe_ok && sc.fails == 0;
    // ── M3: the pixels. YUY2 2x2: row 0 = black, white (Y 16 / 235, Cb = Cr = 128); row 1 = BT.601 red (Y 81, Cb 90,
    // Cr 240) on both pixels.
    let mut xs: Vec<usize> = Vec::new();
    let black = pack(0, 0, 0);
    let white = pack(255, 255, 255);
    let red = pack(255, 0, 0);
    let yuy2: [u8; 8] = [16, 128, 235, 128, 81, 90, 81, 240];
    let mut d4 = [7u32; 4];
    sc.check("yuy2.pix", (pix_of(*b"YUY2", &yuy2) == Pix::Yuy2) as u64, 1);
    sc.check("yuy2.sampled", sample_raw(Pix::Yuy2, &yuy2, 2, 2, &mut d4, 2, 2, &mut xs) as u64, 1);
    sc.check("yuy2.black", d4[0] as u64, black as u64);
    sc.check("yuy2.white", d4[1] as u64, white as u64);
    sc.check("yuy2.red0", d4[2] as u64, red as u64);
    sc.check("yuy2.red1", d4[3] as u64, red as u64);
    sc.check("yuy2.short_refused", sample_raw(Pix::Yuy2, &yuy2[..7], 2, 2, &mut d4, 2, 2, &mut xs) as u64, 0);
    // Downscale 4x2 → 2x1: columns 0 and 2 of row 0 (white, then black).
    let yuy4: [u8; 16] = [235, 128, 16, 128, 16, 128, 235, 128, 81, 90, 81, 240, 81, 90, 81, 240];
    let mut d2 = [7u32; 2];
    sc.check("scale.sampled", sample_raw(Pix::Yuy2, &yuy4, 4, 2, &mut d2, 2, 1, &mut xs) as u64, 1);
    sc.check("scale.c0", d2[0] as u64, white as u64);
    sc.check("scale.c1", d2[1] as u64, black as u64);
    // NV12 2x4: Y rows [16 235] [16 235] [81 81] [81 81]; Cb/Cr rows (128,128) then (90,240).
    let nv12: [u8; 12] = [16, 235, 16, 235, 81, 81, 81, 81, 128, 128, 90, 240];
    let mut d8 = [7u32; 8];
    sc.check("nv12.len", raw_len(Pix::Nv12, 2, 4) as u64, 12);
    sc.check("nv12.sampled", sample_raw(Pix::Nv12, &nv12, 2, 4, &mut d8, 2, 4, &mut xs) as u64, 1);
    sc.check("nv12.black", d8[2] as u64, black as u64);
    sc.check("nv12.white", d8[3] as u64, white as u64);
    sc.check("nv12.red", d8[7] as u64, red as u64);
    // MJPEG: a frame with no DHT, through the tree's decoder, then sampled down to 4x4.
    sc.check("mjpeg.pix", (pix_of(*b"YUY2", &MJPEG_NO_DHT) == Pix::Mjpeg) as u64, 1);
    let mut mjpeg_ok = false;
    match pixel_core::decode_jpeg(&MJPEG_NO_DHT) {
        Ok(img) => {
            sc.check("mjpeg.w", img.width as u64, 8);
            sc.check("mjpeg.h", img.height as u64, 8);
            let mut d16 = [7u32; 16];
            sc.check("mjpeg.sampled", sample_rgba(&img.rgba, img.width as usize, img.height as usize, &mut d16, 4, 4, &mut xs) as u64, 1);
            let (r, g, b) = ((d16[5] >> 16) & 0xFF, (d16[5] >> 8) & 0xFF, d16[5] & 0xFF);
            sc.check("mjpeg.is_red", (r > g + 100 && r > b + 100) as u64, 1);
            mjpeg_ok = true;
        }
        Err(_) => sc.check("mjpeg.decoded", 0, 1),
    }
    // The window box: aspect kept, never upscaled.
    sc.check("fit.down.w", fit(1920, 1080, 960, 600).0 as u64, 960);
    sc.check("fit.down.h", fit(1920, 1080, 960, 600).1 as u64, 540);
    sc.check("fit.noup", (fit(640, 480, 960, 600) == (640, 480)) as u64, 1);
    sc.check("fit.tall", (fit(1080, 1920, 960, 600) == (337, 600)) as u64, 1);
    let convert_ok = sc.fails == 0 && mjpeg_ok;
    // ── The live stream, if one is armed: 2 s of the viewer's counters.
    let mut stream = String::new();
    let mut live_ok = true;
    if !ARMED.load(Ordering::Acquire) {
        stream.push_str("skip(no-device)");
    } else {
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        {
            let (c0, s0, d0) = (CAM.load(Ordering::Relaxed), SHOWN.load(Ordering::Relaxed), DROPPED.load(Ordering::Relaxed));
            let t0 = crate::arch::ms();
            crate::arch::sched::sleep_ms(2_000);
            let dt = crate::arch::ms().wrapping_sub(t0).max(1);
            let cam = CAM.load(Ordering::Relaxed) - c0;
            let shown = SHOWN.load(Ordering::Relaxed) - s0;
            live_ok = cam > 0;
            let _ = write!(stream, "live fps={} frames={} dropped={}", cam * 1000 / dt, shown, DROPPED.load(Ordering::Relaxed) - d0);
        }
        #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
        stream.push_str("skip(no-viewer)");
    }
    let ok = |b: bool| if b { "ok" } else { "FAIL" };
    let pass = sc.fails == 0 && parse_ok && probe_ok && convert_ok && live_ok;
    serial_println!("{}", line);
    if sc.fails == 0 {
        serial_println!(
            ":: UVCCAM: parse={} formats={} probe={}(static) convert={}(yuy2,nv12,mjpeg) stream={} -> {} ::",
            ok(parse_ok), c.nfmt, ok(probe_ok), ok(convert_ok), stream, if pass { "PASS" } else { "FAIL reason=no-frames" }
        );
    } else {
        serial_println!(
            ":: UVCCAM: parse={} formats={} probe={}(static) convert={} failures={} first={} got={} want={} stream={} -> FAIL ::",
            ok(parse_ok), c.nfmt, ok(probe_ok), ok(convert_ok), sc.fails, sc.first.0, sc.first.1, sc.first.2, stream
        );
    }
}

// ── `tests camdot` (CAMDOT B702) ────────────────────────────────────────────────────────────────────────────

/// The pure legs (the judge, the decision's key, the answer map), then — with a camera streaming on x86 `wc` — the
/// live legs: the menu bar PAINTS the dot (its own edge counter), the Privacy tab's revoke seam ([`revoked`], the
/// same call the Revoke button makes; the kept decision is not touched) stops the stream inside one frame, and the
/// bar paints the dot out. The live leg ENDS the stream (replug the camera to stream again). No device (QEMU has no
/// usb camera in the lanes, and UVCCAM has no synthetic stream): `-> SKIP(no-device)` after the pure legs.
fn camdot_fixture() {
    let mut sc = Score { fails: 0, first: ("", 0, 0) };
    sc.check("judge.off", (dot_word(0) == "off") as u64, 1);
    sc.check("judge.on", (dot_word(1) == "on") as u64, 1);
    sc.check("judge.many", (dot_word(3) == "on") as u64, 1);
    let key = crate::prefs::decision_key(CAM_DOMAIN, CAM_APP);
    sc.check("key.domain", key.starts_with("decisions.camera.Camera-") as u64, 1);
    sc.check("key.per_app", (crate::prefs::decision_key(CAM_DOMAIN, "Stria") != key) as u64, 1);
    sc.check("answer.allow", answer_of(true, 1) as u64, ANS_ALLOW as u64);
    sc.check("answer.not_now", answer_of(false, 0) as u64, ANS_NOT_NOW as u64);
    sc.check("answer.close_box", answer_of(false, 1) as u64, ANS_NOT_NOW as u64);
    let logic = sc.fails == 0;
    let opens = OPENS.load(Ordering::Relaxed);
    #[allow(unused_mut)]
    let mut counts = (0u32, 0u32);
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        counts = crate::video::menubar::camdot::counts();
    }
    let ok = |b: bool| if b { "ok" } else { "FAIL" };
    if !ARMED.load(Ordering::Acquire) {
        if logic {
            serial_println!(
                ":: CAMDOT: opens={} dot_on={} dot_off={} revoke_ms=- logic=ok mic=no-seam -> SKIP(no-device) ::",
                opens, counts.0, counts.1
            );
        } else {
            serial_println!(
                ":: CAMDOT: opens={} dot_on={} dot_off={} revoke_ms=- logic=FAIL first={} got={} want={} mic=no-seam -> FAIL ::",
                opens, counts.0, counts.1, sc.first.0, sc.first.1, sc.first.2
            );
        }
        return;
    }
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        use crate::video::menubar::camdot;
        let wait = |ms: u64, f: &dyn Fn() -> bool| -> bool {
            let t0 = crate::arch::ms();
            while !f() {
                if crate::arch::ms().wrapping_sub(t0) >= ms {
                    return false;
                }
                crate::arch::sched::sleep_ms(5);
            }
            true
        };
        let lit = wait(2_000, &|| camdot::painted());
        REVOKE_MS.store(u64::MAX, Ordering::Relaxed);
        revoked(&key);
        let stopped = wait(1_000, &|| !ARMED.load(Ordering::Acquire) && REVOKE_MS.load(Ordering::Relaxed) != u64::MAX);
        let dark = wait(2_000, &|| !camdot::painted());
        let ms = REVOKE_MS.load(Ordering::Relaxed);
        let (on, off) = camdot::counts();
        let pass = logic && lit && stopped && dark && ms <= REVOKE_FRAME_MS;
        let ms_s = if ms == u64::MAX { String::from("-") } else { alloc::format!("{}", ms) };
        serial_println!(
            ":: CAMDOT: opens={} dot_on={} dot_off={} revoke_ms={} logic={} lit={} stopped={} dark={} mic=no-seam stream=stopped(replug) -> {} ::",
            opens, on, off, ms_s, ok(logic), ok(lit), ok(stopped), ok(dark), if pass { "PASS" } else { "FAIL" }
        );
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    serial_println!(
        ":: CAMDOT: opens={} dot_on={} dot_off={} revoke_ms=- logic={} mic=no-seam -> SKIP(no-bar) ::",
        opens, counts.0, counts.1, ok(logic)
    );
}
