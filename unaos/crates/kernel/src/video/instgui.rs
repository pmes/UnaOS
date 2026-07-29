// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! INSTGUI — the first graphical installer (x86, `wc` + `instgui` features).
//!
//! A kernel-owned compositor window, drawn in the CRISPY theme's const table
//! ([`super::theme`]), that walks Peter through: **choose disk → erase warning →
//! install → verdict**. It is a *face* on the installer engine, never a new
//! authority: every write still goes through [`crate::install`]'s engine, whose
//! blank-check refusal and verify ladder are untouched. The GUI cannot arm
//! anything the engine would refuse; a non-blank target surfaces the engine's
//! own refusal as the verdict screen. The attended Enter on the warning screen
//! IS the fresh operator go the bench law requires — this module exists to make
//! that consent informed (device identity + size on glass, in theme).
//!
//! ### Front-buffer discipline
//! Every pixel lands in the cached-RAM [`SURF`]; presentation is `wm`'s. This
//! module never touches the framebuffer.
//!
//! ### Input
//! The main loop offers keys via [`consume_key`] BEFORE the console's
//! `handle_key`; the GUI consumes them only while its window is open, so the
//! console keeps working the moment the installer closes (Esc from the chooser).

use crate::drivers::block;
use super::theme;
use super::wm;

/// Content surface dimensions (pixels). The compositor's scale rule may magnify.
const W: usize = 520;
const H: usize = 396;

/// Text scale over the raw font8x8 cell: 2 → 16 px glyphs, 30 cols × comfortable rows.
const TS: usize = 2;
const CELL: usize = 8 * TS;

#[repr(align(64))]
struct Surf([u32; W * H]);
/// SAFETY: written only from `repaint`, which runs on the BSP main loop (the
/// same thread that calls `consume_key`), and read by `wm`'s composite. The
/// window is created after the first full paint; later paints are racy against
/// a composite read only in the benign present-tear sense every app surface
/// shares (`wm` copies rows; a mid-paint read shows a mixed frame, corrected by
/// the follow-up present).
static mut SURF: Surf = Surf([0; W * H]);

#[derive(Clone, Copy, PartialEq)]
enum State {
    Choose,
    Warn,
    Running,
    Done(bool),
    Closed,
}

static STATE: spin::Mutex<State> = spin::Mutex::new(State::Closed);
static WIN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(wm::WIN_NONE);
/// Which list row is selected (the device list is tiny; a u8 outlives it).
static SEL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

use core::sync::atomic::Ordering;

/// The device rows the chooser shows. Single-controller world today: the
/// registered BLOCK_DEVICE plus (if distinct by slot) the USB geometry row.
fn devices(out: &mut [Option<block::BlockDeviceInfo>; 2]) -> usize {
    let mut n = 0;
    if let Some(i) = block::info() {
        out[0] = Some(i);
        n = 1;
    }
    if let Some(u) = block::usb_info() {
        if out[0].map(|i| i.slot_id) != Some(u.slot_id) {
            out[n] = Some(u);
            n += 1;
        }
    }
    n
}

// ---------------------------------------------------------------- painting --

fn fill(px: &mut [u32], x: usize, y: usize, w: usize, h: usize, c: u32) {
    for row in y..(y + h).min(H) {
        let base = row * W;
        for col in x..(x + w).min(W) {
            px[base + col] = c;
        }
    }
}

/// One-pixel-line rectangle outline (the theme's FRAME_LINE weight is drawn by
/// repeating this at inset offsets — bevel light/shadow pairs give the relief).
fn rect(px: &mut [u32], x: usize, y: usize, w: usize, h: usize, c: u32) {
    fill(px, x, y, w, 1, c);
    fill(px, x, y + h - 1, w, 1, c);
    fill(px, x, y, 1, h, c);
    fill(px, x + w - 1, y, 1, h, c);
}

/// CRISPY raised/sunken bevel: light on top+left when raised, inverted when sunken.
fn bevel(px: &mut [u32], x: usize, y: usize, w: usize, h: usize, raised: bool) {
    let (lt, rb) = if raised {
        (theme::BEVEL_LIGHT, theme::BEVEL_SHADOW)
    } else {
        (theme::BEVEL_SHADOW, theme::BEVEL_LIGHT)
    };
    for i in 0..theme::BEVEL {
        fill(px, x + i, y + i, w - 2 * i, 1, lt);
        fill(px, x + i, y + i, 1, h - 2 * i, lt);
        fill(px, x + i, y + h - 1 - i, w - 2 * i, 1, rb);
        fill(px, x + w - 1 - i, y + i, 1, h - 2 * i, rb);
    }
}

fn text(px: &mut [u32], x: usize, y: usize, s: &[u8], fg: u32) {
    let mut cx = x;
    for &ch in s {
        if ch == b'\n' || cx + CELL > W {
            break;
        }
        let bitmap = font8x8::legacy::BASIC_LEGACY[ch.min(127) as usize];
        for (ry, rowbits) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if rowbits & (1 << rx) != 0 {
                    fill(px, cx + rx * TS, y + ry * TS, TS, TS, fg);
                }
            }
        }
        cx += CELL;
    }
}

/// Format a byte count as whole gibibytes/mebibytes into `buf`, returning the slice.
fn fmt_size(buf: &mut [u8; 16], bytes: u64) -> &[u8] {
    let (val, unit) = if bytes >= 1 << 30 {
        (bytes >> 30, b"GiB")
    } else {
        (bytes >> 20, b"MiB")
    };
    let mut i = buf.len();
    buf[i - 3..].copy_from_slice(unit);
    i -= 4;
    buf[i] = b' ';
    let mut v = val.max(1);
    while v > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    &buf[i..]
}

fn button(px: &mut [u32], x: usize, y: usize, w: usize, label: &[u8], primary: bool) {
    let h = theme::BUTTON_HEIGHT + 6;
    fill(px, x, y, w, h, if primary { theme::BUTTON_FACE } else { theme::CHROME_FACE });
    bevel(px, x, y, w, h, true);
    let tx = x + (w.saturating_sub(label.len() * CELL)) / 2;
    text(px, tx, y + (h - CELL) / 2, label, theme::BUTTON_TEXT);
    if primary {
        rect(px, x - 2, y - 2, w + 4, h + 4, theme::ACCENT);
    }
}

fn repaint() {
    let st = *STATE.lock();
    // SAFETY: see `SURF`.
    let px = unsafe { &mut (*core::ptr::addr_of_mut!(SURF)).0 };

    // Chrome-adjacent frame: wm draws the real title strip; inside, the CRISPY
    // content well — sunken bevel around CONTENT_FILL.
    fill(px, 0, 0, W, H, theme::CHROME_FACE);
    bevel(px, 4, 4, W - 8, H - 8, false);
    fill(px, 4 + theme::BEVEL, 4 + theme::BEVEL, W - 8 - 2 * theme::BEVEL, H - 8 - 2 * theme::BEVEL, theme::CONTENT_FILL);

    let lx = 24;
    match st {
        State::Choose => {
            text(px, lx, 20, b"Install UnaOS", theme::CONTENT_TEXT);
            fill(px, lx, 42, W - 2 * lx, 2, theme::FRAME_LINE);
            text(px, lx, 54, b"Choose a target disk:", theme::CONTENT_TEXT);

            let mut devs: [Option<block::BlockDeviceInfo>; 2] = [None; 2];
            let n = devices(&mut devs);
            let sel = SEL.load(Ordering::Relaxed) as usize;
            if n == 0 {
                // Storage enumerates asynchronously (USB bring-up finishes well after the
                // compositor activates), so "none yet" is the normal opening state — say so,
                // and keep re-checking (`service`) rather than freezing the first answer.
                text(px, lx, 84, b"No disks yet - waiting for USB", theme::CONTENT_TEXT);
                text(px, lx, 84 + CELL + 6, b"enumeration. Attach a disk and", theme::TITLE_TEXT_INACTIVE);
                text(px, lx, 84 + 2 * (CELL + 6), b"it appears here by itself.", theme::TITLE_TEXT_INACTIVE);
            }
            for i in 0..n {
                let d = devs[i].unwrap();
                let ry = 84 + i * (CELL + 22);
                let row_bg = if i == sel { theme::SCROLL_THUMB } else { theme::CONTENT_FILL };
                fill(px, lx, ry - 4, W - 2 * lx, CELL + 12, row_bg);
                if i == sel {
                    rect(px, lx, ry - 4, W - 2 * lx, CELL + 12, theme::ACCENT);
                }
                let mut line = [b' '; 40];
                line[..4].copy_from_slice(b"slot");
                line[4] = b'0' + (d.slot_id % 10);
                line[6..6 + 16].copy_from_slice(&d.product);
                let mut sz = [0u8; 16];
                let s = fmt_size(&mut sz, d.num_blocks * d.block_size as u64);
                let tail = 40 - s.len();
                line[tail..].copy_from_slice(s);
                text(px, lx + 8, ry, &line[..(W - 2 * lx - 16) / CELL], theme::CONTENT_TEXT);
            }
            // Exits are ALWAYS on screen: an installer that can only go forward is a trap.
            text(px, lx, H - 100, b"w/s select   Enter continue", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, H - 100 + CELL + 4, b"Esc boot this live system", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, H - 100 + 2 * (CELL + 4), b"q  halt the machine", theme::TITLE_TEXT_INACTIVE);
            if n > 0 {
                button(px, W - 190, H - 52, 160, b"Continue", true);
            }
        }
        State::Warn => {
            // The warning panel: CRISPY has no alarm red by design; the accent
            // frame + pressed-face field + explicit words carry the weight.
            text(px, lx, 20, b"Erase and install?", theme::CONTENT_TEXT);
            fill(px, lx, 42, W - 2 * lx, 2, theme::FRAME_LINE);
            fill(px, lx, 58, W - 2 * lx, 120, theme::BUTTON_FACE_PRESSED);
            bevel(px, lx, 58, W - 2 * lx, 120, false);
            rect(px, lx + 2, 60, W - 2 * lx - 4, 116, theme::ACCENT);
            text(px, lx + 14, 72, b"THIS ERASES EVERYTHING", theme::CONTENT_TEXT);
            text(px, lx + 14, 72 + CELL + 6, b"on the selected disk.", theme::CONTENT_TEXT);
            let mut devs: [Option<block::BlockDeviceInfo>; 2] = [None; 2];
            let n = devices(&mut devs);
            let sel = (SEL.load(Ordering::Relaxed) as usize).min(n.saturating_sub(1));
            if let Some(d) = devs.get(sel).copied().flatten() {
                let mut line = [b' '; 24];
                line[..16].copy_from_slice(&d.product);
                line[17] = b's';
                line[18] = b'l';
                line[19] = b'o';
                line[20] = b't';
                line[22] = b'0' + (d.slot_id % 10);
                text(px, lx + 14, 72 + 2 * (CELL + 6), &line, theme::CONTENT_TEXT);
            }
            text(px, lx, 200, b"The engine refuses non-blank", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, 200 + CELL + 4, b"targets (blank-check guard).", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, H - 92, b"Enter install    Esc back", theme::TITLE_TEXT_INACTIVE);
            button(px, W - 190, H - 52, 160, b"Install", true);
            button(px, W - 370, H - 52, 160, b"Back", false);
        }
        State::Running => {
            text(px, lx, 20, b"Installing...", theme::CONTENT_TEXT);
            fill(px, lx, 42, W - 2 * lx, 2, theme::FRAME_LINE);
            text(px, lx, 84, b"GPT + FAT32 + payload + verify", theme::CONTENT_TEXT);
            text(px, lx, 84 + CELL + 6, b"Progress on the console window.", theme::TITLE_TEXT_INACTIVE);
            // Indeterminate bar: the engine is synchronous; this frame shows
            // during the run because we present before calling it.
            fill(px, lx, 150, W - 2 * lx, 22, theme::SCROLL_TRACK);
            bevel(px, lx, 150, W - 2 * lx, 22, false);
            fill(px, lx + 4, 154, (W - 2 * lx) / 3, 14, theme::ACCENT);
        }
        State::Done(ok) => {
            text(px, lx, 20, if ok { b"Install PASSED".as_slice() } else { b"Install refused/failed".as_slice() }, theme::CONTENT_TEXT);
            fill(px, lx, 42, W - 2 * lx, 2, theme::FRAME_LINE);
            if ok {
                text(px, lx, 84, b"GPT written, ESP formatted,", theme::CONTENT_TEXT);
                text(px, lx, 84 + CELL + 6, b"payload verified extent-by-", theme::CONTENT_TEXT);
                text(px, lx, 84 + 2 * (CELL + 6), b"extent. See console verdicts.", theme::CONTENT_TEXT);
            } else {
                text(px, lx, 84, b"The engine declined - most", theme::CONTENT_TEXT);
                text(px, lx, 84 + CELL + 6, b"often the blank-check guard", theme::CONTENT_TEXT);
                text(px, lx, 84 + 2 * (CELL + 6), b"(target not blank). Console", theme::CONTENT_TEXT);
                text(px, lx, 84 + 3 * (CELL + 6), b"has the exact refusal.", theme::CONTENT_TEXT);
            }
            text(px, lx, H - 92, b"Esc close", theme::TITLE_TEXT_INACTIVE);
            button(px, W - 190, H - 52, 160, b"Close", true);
        }
        State::Closed => {}
    }

    let id = WIN.load(Ordering::Relaxed);
    if id != wm::WIN_NONE {
        wm::present(id);
    }
}

// ------------------------------------------------------------------- verbs --

/// Open the installer window (called from `wcx::activate` when the `instgui`
/// feature is armed). Spawn-place discipline: geometry settled before the row.
pub fn open() {
    let mut st = STATE.lock();
    if *st != State::Closed {
        return;
    }
    *st = State::Choose;
    drop(st);
    repaint(); // full first paint BEFORE the window names the surface
    let (_s, ow, oh) = match wm::spawn_geometry(W, H) {
        Some(g) => g,
        None => {
            serial_println!("[wc-x] instgui DECLINE reason=geometry-unavailable");
            *STATE.lock() = State::Closed;
            return;
        }
    };
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        let i = fb.info();
        (i.width, i.height)
    };
    let ox = (pw.saturating_sub(ow)) / 2;
    let oy = (ph.saturating_sub(oh)) / 3; // upper-third center: reads as a dialog
    let surf = core::ptr::addr_of_mut!(SURF) as usize;
    let id = wm::create_at(
        0,
        surf,
        W * H * 4,
        W as u32,
        H as u32,
        (W * 4) as u32,
        b"install unaos",
        ox + wm::BORDER,
        oy + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        serial_println!("[wc-x] instgui DECLINE reason=create-failed");
        *STATE.lock() = State::Closed;
        return;
    }
    WIN.store(id, Ordering::Relaxed);
    // The dialog is modal over the GLASS: the console keeps taking glyphs and serial keeps
    // every line, but it stops presenting until we close. Without this, each boot message
    // repaints the console window AND (through wm's upward occlusion closure) this dialog —
    // ~24 ms of GOP writes per line, which reads as a hard flicker.
    super::fbcon::console_present_suspend(true);
    serial_println!("[wc-x] instgui open win={} box={}x{} at ({},{}) (console presents suspended)", id, ow, oh, ox, oy);
    repaint();
}

fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id != wm::WIN_NONE {
        wm::close(id);
    }
    *STATE.lock() = State::Closed;
    // The console gets the glass back and repaints everything it accumulated.
    super::fbcon::console_present_suspend(false);
    serial_println!("[wc-x] instgui closed — console presents resumed, booting on");
}

/// Main-loop hook, every frame: re-check the disk list (storage enumerates long after the
/// dialog opens) and repaint only when it actually changed, so this costs nothing per frame
/// on a settled machine.
pub fn service() {
    if *STATE.lock() != State::Choose {
        return;
    }
    let mut devs: [Option<block::BlockDeviceInfo>; 2] = [None; 2];
    let n = devices(&mut devs);
    let sig = devs
        .iter()
        .flatten()
        .fold(n as u64, |a, d| a ^ (d.slot_id as u64) << 8 ^ d.num_blocks);
    if LAST_SIG.swap(sig, Ordering::Relaxed) != sig {
        // USB-UNPLUG: the list can now SHRINK (a disconnect retracts the block-registry entry via
        // `block::unpublish_usb_geometry`), which it never could before — attach was the only event
        // the registry reported. A selection index left pointing past the end of the shortened list
        // would silently be clamped to row 0 by the warning screen, i.e. the operator's highlighted
        // choice would become a DIFFERENT disk than the one they were looking at. Clamp here, where
        // the change is detected and before the repaint that redraws the highlight.
        let last = n.saturating_sub(1) as u8;
        if SEL.load(Ordering::Relaxed) > last {
            SEL.store(last, Ordering::Relaxed);
        }
        repaint();
    }
}

/// Signature of the last disk list painted (count ^ slot ^ size), so `service` repaints on a
/// real change rather than every frame.
static LAST_SIG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// Main-loop hook: returns true if the key belonged to the installer.
pub fn consume_key(c: u8) -> bool {
    let st = *STATE.lock();
    if st == State::Closed {
        return false;
    }
    // Halt is offered from every screen: an installer must always be leaveable.
    if c == b'q' && st != State::Running {
        serial_println!("[wc-x] instgui halt requested — parking the machine (power off is safe)");
        close();
        loop {
            crate::hlt();
        }
    }
    match (st, c) {
        (State::Choose, b'\x1b') => close(),
        (State::Choose, b'w') | (State::Choose, b'A') => {
            let s = SEL.load(Ordering::Relaxed);
            SEL.store(s.saturating_sub(1), Ordering::Relaxed);
            repaint();
        }
        (State::Choose, b's') | (State::Choose, b'B') => {
            let mut devs: [Option<block::BlockDeviceInfo>; 2] = [None; 2];
            let n = devices(&mut devs);
            let s = SEL.load(Ordering::Relaxed) as usize;
            SEL.store(((s + 1).min(n.saturating_sub(1))) as u8, Ordering::Relaxed);
            repaint();
        }
        (State::Choose, b'\r') | (State::Choose, b'\n') => {
            // No disk, no warning screen — advancing to "erase what?" would be nonsense.
            let mut devs: [Option<block::BlockDeviceInfo>; 2] = [None; 2];
            if devices(&mut devs) == 0 {
                return true;
            }
            *STATE.lock() = State::Warn;
            repaint();
        }
        (State::Warn, b'\x1b') => {
            *STATE.lock() = State::Choose;
            repaint();
        }
        (State::Warn, b'\r') | (State::Warn, b'\n') => {
            // The attended go. Present the Running frame FIRST (the engine is
            // synchronous on this thread), then run, then verdict.
            *STATE.lock() = State::Running;
            repaint();
            serial_println!("[wc-x] instgui install-go (attended Enter on warn screen)");
            let ok = crate::install::run_gui();
            *STATE.lock() = State::Done(ok);
            repaint();
        }
        (State::Done(_), b'\x1b') | (State::Done(_), b'\r') | (State::Done(_), b'\n') => close(),
        _ => {}
    }
    true
}
