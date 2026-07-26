// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! WC-X86 — bring the window compositor up on the x86 panel path.
//!
//! [`super::wm`] has always been arch-neutral: it composites through the generic
//! [`super::FrameBuffer`] held in [`super::WRITER`], which on x86 already wraps the UEFI GOP
//! surface. What the x86 path lacked was not a compositor — it was an *activation*: nothing ever
//! created a window, so the table stayed empty and the pass had nothing to paint. This module is
//! that activation and nothing else. **It does not modify `wm` or `cursor` in any way**; every
//! pixel it puts on the panel goes through `wm`'s own public verbs.
//!
//! ### Where it runs, and why exactly there
//!
//! At the END of `kepler_display::takeover_display`, on the line after
//! [`super::fbcon::panel_console_resume`]. That seam is the first moment on the metal path where
//! the panel is settled: the takeover has repointed the scan-out, the calibration pattern has been
//! drawn, held and cleared, and the console has been re-homed on the real surface. Activating
//! *before* it would put windows on a surface the takeover is about to repoint — the compositor
//! would be fighting the takeover for the same pixels, and the takeover would win, silently.
//!
//! ### Console pixel ownership — RULED: the console is a WINDOW
//!
//! Peter has ruled (2026-07-26): console-becomes-a-window, not a reserved band. The provisional
//! band — a clamp on the console's row grid, with the compositor confined below it — is gone, and so
//! is the `panel_console_reserve_band` call that installed it. What replaces it is
//! [`super::fbcon::panel_console_window_open`]: the console allocates a cached-RAM surface, opens an
//! ordinary row in the window table over it, and paints its glyphs there. Z-order now does the job
//! the band was doing, which is the whole reason the band was only ever provisional.
//!
//! The console window is created FIRST and the demo window second, so the demo's later `z` puts it
//! in front — the sitting shows two real windows with real occlusion between them, rather than two
//! windows that were carefully arranged never to meet.
//!
//! Serial is unaffected by any of this: `serial_println!` still writes the UART on every line, and
//! the FTDI mirror the bench reads is unconditional in this build. The window is where the text is
//! *drawn*, not where it is *logged*.
//!
//! ### Damage-row present
//!
//! Not re-implemented here, and deliberately: `wm::composite` already presents by damage. It paints
//! only the outer boxes of the windows in its dirty set (closed upwards over occlusion), it stages
//! them in cached RAM and copies them out as full-width row runs, and it cleans exactly the row span
//! it touched via `FrameBuffer::flush_range`. Nothing about that is aarch64-specific — on x86 the
//! cache clean is the documented no-op and the row-run copy is the same code. The x86 present is
//! therefore already row-damaged; this module's contribution is to SAY SO on the wire
//! (`[wc-x] present ... rows=A..B`) so a bench photo can be checked against a row span rather than
//! against a claim.
//!
//! ### Front-buffer discipline
//!
//! Every pixel this module authors lands in [`DEMO_SURF`], a cached-RAM kernel surface. It performs
//! no framebuffer write of its own and reads the front buffer never — `wm` owns the panel writes
//! and does them through its staged path. There is no direct-front-buffer fallback in this file
//! because there is no framebuffer write in this file.

use super::wm;

/// The demo surface's source dimensions, in pixels. Small on purpose: the compositor's own scale
/// rule magnifies it to the panel (4x on a ≤1799-row panel), so a small source is what exercises the
/// upscale path, and a small source is also what keeps this buffer in BSS at a size nobody has to
/// think about (96 * 64 * 4 = 24 KiB).
const DEMO_W: usize = 96;
const DEMO_H: usize = 64;

/// The demo surface, in ARGB8888 with a stride of exactly one row. Kernel-owned cached RAM: the
/// compositor reads it as a source and never writes it, and nothing in EL0 can reach it.
///
/// `static mut` rather than a `Mutex`: `wm::create` takes the surface as a raw address it will read
/// from composite context, so the buffer must have a stable address independent of any guard. It is
/// written exactly once, by [`paint_demo`], before the window that references it exists.
#[repr(align(4))]
struct DemoSurface([u32; DEMO_W * DEMO_H]);
static mut DEMO_SURF: DemoSurface = DemoSurface([0; DEMO_W * DEMO_H]);

/// The window id [`activate`] opened, or [`wm::WIN_NONE`] if it has not run or declined. Read only
/// by the witness lines below.
static DEMO_WIN: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(wm::WIN_NONE);

/// Gap in panel pixels between the demo window's outer box and the panel edge. Matches `wm`'s own
/// tiling gap in spirit; it is only used to inset the pinned demo window from the bottom-right
/// corner, so the frame the compositor draws is visible on all four sides in a bench photograph.
const EDGE_GAP: usize = 8;

/// Fill [`DEMO_SURF`] with a calibration pattern.
///
/// **Not theme, not chrome.** The window's border and title strip are painted by the compositor from
/// `wm`'s own constants, as they are for every window; nothing here invents a chrome colour or a
/// desktop colour, and this module deliberately defines no colour table of its own. What it draws is
/// *content* — the kind of content a calibration target is made of, chosen so that a photograph of
/// the panel answers questions:
///
///   * a 1-px frame in white — proves the window's content extent, and where it sits relative to the
///     kernel-drawn border (the two must be adjacent, never overlapping);
///   * pure red / green / blue vertical bars — prove the channel order survives the GOP layout
///     conversion (`FrameBuffer::put_pixel` re-encodes per layout; a swapped pair shows instantly);
///   * a corner-to-corner diagonal — proves the nearest-neighbour upscale is isotropic; a stride
///     error in the blit turns a diagonal into a staircase with a visible slope change;
///   * a single-pixel checkerboard block — the highest spatial frequency the surface can carry, so
///     any row that the present drops or duplicates shows up as a band of flat colour.
fn paint_demo() {
    // SAFETY: called exactly once, from `activate`, before the window that names this buffer is
    // created — so no compositor pass can be reading it, and there is no other writer at all.
    let px = unsafe { &mut (*core::ptr::addr_of_mut!(DEMO_SURF)).0 };
    for y in 0..DEMO_H {
        for x in 0..DEMO_W {
            let c = if x == 0 || y == 0 || x == DEMO_W - 1 || y == DEMO_H - 1 {
                0x00FF_FFFF
            } else if x * DEMO_H == y * DEMO_W {
                0x00FF_FFFF
            } else if y >= DEMO_H - 12 && x >= DEMO_W - 12 {
                // Single-pixel checkerboard, bottom-right.
                if (x + y) & 1 == 0 { 0x00FF_FFFF } else { 0x0000_0000 }
            } else if y < 12 {
                // Channel bars across the top third of the width each.
                match x * 3 / DEMO_W {
                    0 => 0x00FF_0000,
                    1 => 0x0000_FF00,
                    _ => 0x0000_00FF,
                }
            } else {
                0x0000_0000
            };
            px[y * DEMO_W + x] = c;
        }
    }
}

/// WC-X86 — activate the compositor on the x86 panel. Idempotent: a second call is a no-op.
///
/// Fail-closed at every step. A panel that is not ready, a console that will not yield its band, a
/// window table that will not take a row — each returns after saying which one it was, and each
/// leaves the panel exactly as the takeover left it. There is no path here that half-activates.
pub fn activate() {
    use core::sync::atomic::Ordering;
    if DEMO_WIN.load(Ordering::Relaxed) != wm::WIN_NONE {
        return;
    }

    // The panel, as the compositor will see it. Read through the same `WRITER` handle `wm` uses, so
    // the geometry the band is computed from is the geometry the windows are placed against.
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            serial_println!("[wc-x] activate DECLINE reason=fb-not-ready");
            return;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    // RULED — the console becomes a window. Opened FIRST so the demo below carries the higher z.
    // Its own witness lines (`[wc-x] console-window …`) report the geometry and the panic fallback.
    let cwin = super::fbcon::panel_console_window_open();
    if cwin == wm::WIN_NONE {
        serial_println!("[wc-x] activate DECLINE reason=console-window-declined");
        return;
    }
    serial_println!("[wc-x] activate panel={}x{} console_win={}", pw, ph, cwin);

    paint_demo();
    // Taking the ADDRESS of a `static mut` is safe — no reference is formed. The one place a
    // reference is formed is `paint_demo`, which carries the safety argument for it.
    let surf = core::ptr::addr_of_mut!(DEMO_SURF) as usize;
    let surf_len = DEMO_W * DEMO_H * 4;
    // Owner ASID 0 — this window belongs to the KERNEL, not to any address space. It is therefore
    // outside the focus ring (`focus_ring` skips owner 0) and outside `close_owner`'s reach, which
    // is the correct reading: no EL0 task may present, move or close it.
    let id = wm::create(
        0,
        surf,
        surf_len,
        DEMO_W as u32,
        DEMO_H as u32,
        (DEMO_W * 4) as u32,
        b"unaos wc",
    );
    if id == wm::WIN_NONE {
        serial_println!("[wc-x] activate DECLINE reason=create-failed");
        return;
    }

    // Pin the demo's OUTER box into the panel's bottom-right corner. `move_to` takes the CONTENT
    // origin, so the chrome offsets come back off the outer origin; it also pins the row, so the
    // automatic tiling leaves it here rather than packing it to the top-left over the console.
    // Overlapping the console window is intended: the demo was created later, so it has the higher
    // z, and the overlap is what makes the occlusion visible in a bench photograph.
    let (ow, oh) = match wm::info(id) {
        Some(i) => (
            i.w * i.scale + 2 * wm::BORDER,
            i.h * i.scale + wm::TITLE_H + 2 * wm::BORDER,
        ),
        None => {
            serial_println!("[wc-x] activate DECLINE reason=row-vanished");
            return;
        }
    };
    let ox = pw.saturating_sub(ow).saturating_sub(EDGE_GAP);
    let oy = ph
        .saturating_sub(crate::ui_status::chrome_h(ph))
        .saturating_sub(oh)
        .saturating_sub(EDGE_GAP);
    wm::move_to(id, ox + wm::BORDER, oy + wm::TITLE_H + wm::BORDER);

    DEMO_WIN.store(id, Ordering::Relaxed);
    let ok = wm::present(id);

    if let Some(i) = wm::info(id) {
        let y0 = i.y.saturating_sub(wm::TITLE_H + wm::BORDER);
        let y1 = (i.y + i.h * i.scale + wm::BORDER).min(ph);
        serial_println!(
            "[wc-x] demo win={} surf={}x{} at ({},{}) scale={}x z={}",
            i.id, i.w, i.h, i.x, i.y, i.scale, i.z
        );
        serial_println!("[wc-x] present win={} rows={}..{} ok={}", i.id, y0, y1, ok);
    }
}
