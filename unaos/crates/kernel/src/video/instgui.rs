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
    /// INSTALL-SEL: the operator's chosen disk was not in the block registry at go-time. A distinct
    /// screen rather than a generic failure, because the honest thing to tell someone whose disk was
    /// unplugged mid-dialog is exactly that — and, critically, that nothing was written to anything.
    Gone,
    Closed,
}

static STATE: spin::Mutex<State> = spin::Mutex::new(State::Closed);
static WIN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(wm::WIN_NONE);
/// Which list row is selected (the device list is tiny; a u8 outlives it).
static SEL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// INSTALL-SEL: the identity the operator COMMITTED to when they left the chooser, plus the row it
/// occupied on their screen (carried for the witness only).
///
/// The row index is not, and never was, a safe handoff. `SEL` names a position in a list that is
/// rebuilt from the live registry on every frame, and since `block::unpublish_usb_geometry` that list
/// can shrink under the dialog. Clamping the index (as `service` does) keeps the HIGHLIGHT honest, but
/// an index handed to the engine would still mean "whatever is at that position when the engine looks",
/// which is a different disk than the one the warning screen described. So the moment the operator
/// presses Enter on the chooser we freeze the row into a `block::BlockDeviceId` — registry handle,
/// slot, geometry — and everything downstream (the warning screen's text, the engine's bind) resolves
/// THAT against the live registry. A list change between frames can then only make the identity fail
/// to resolve, which is a refusal; it can never silently retarget the install.
static PENDING: spin::Mutex<Option<(block::BlockDeviceId, u8)>> = spin::Mutex::new(None);

use core::sync::atomic::Ordering;

/// One row of the chooser: the device's geometry (for painting) alongside the durable identity that
/// names it independently of this frame's list.
#[derive(Clone, Copy)]
struct Row {
    id: block::BlockDeviceId,
    info: block::BlockDeviceInfo,
    /// INSTALL-SELF: this device carries the FAT volume serial the kernel booted from — it is the boot
    /// device (or a byte clone of it). Shown, marked, and NOT selectable. See
    /// [`crate::install::selfguard`] for why the row is kept on screen instead of hidden.
    boot: bool,
}

/// The device rows the chooser shows. Single-controller world today: the
/// registered BLOCK_DEVICE plus (if distinct by slot) the USB geometry row.
///
/// INSTALL-SEL: each row now carries the registry handle it was read from, so the two rows stay
/// distinguishable to the engine and not merely to the eye. On x86 the one stick is published into
/// both handles and the slot-equality test below collapses them to a single row, exactly as before.
/// INSTALL-SELF: rows now carry the boot-device verdict, resolved through the SAME
/// [`crate::install::selfguard`] the engine consults at go-time (cached per block-registry signature,
/// so this costs nothing on the per-frame repaint path). One resolver for glass and engine, for the
/// same reason INSTALL-SEL gave the warning screen one: a UI that decides eligibility on its own can
/// disagree with the thing that does the erasing.
fn devices(out: &mut [Option<Row>; 2]) -> usize {
    let mut n = 0;
    if let Some(i) = block::info() {
        let id = i.id(block::BlockHandle::Global);
        out[0] = Some(Row { id, info: i, boot: is_boot(id) });
        n = 1;
    }
    if let Some(u) = block::usb_info() {
        if out[0].map(|r| r.info.slot_id) != Some(u.slot_id) {
            let id = u.id(block::BlockHandle::Usb);
            out[n] = Some(Row { id, info: u, boot: is_boot(id) });
            n += 1;
        }
    }
    n
}

fn is_boot(id: block::BlockDeviceId) -> bool {
    use crate::install::selfguard::{classify, Verdict};
    classify(id) == Verdict::BootDevice
}

/// INSTALL-SELF: the first row the operator is allowed to choose, or `None` when every attached disk
/// is the boot device. Selection can never land on a marked row, so the chooser cannot hand the engine
/// a target the engine would refuse.
fn first_selectable(devs: &[Option<Row>; 2], n: usize) -> Option<usize> {
    (0..n).find(|&i| devs[i].is_some_and(|r| !r.boot))
}

/// The next selectable row in `dir` (+1 down / -1 up) from `cur`, or `cur` if there is none that way.
/// Marked rows are stepped OVER rather than stopped on: an inert highlight on a row that cannot be
/// chosen reads as a broken installer.
fn step_selectable(devs: &[Option<Row>; 2], n: usize, cur: usize, dir: isize) -> usize {
    let mut i = cur as isize;
    loop {
        i += dir;
        if i < 0 || i >= n as isize {
            return cur;
        }
        if devs[i as usize].is_some_and(|r| !r.boot) {
            return i as usize;
        }
    }
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
    // content well — sunken bevel around the content surface.
    //
    // PAPER: the well is the kit's `content_surface.Paper` material, not a flat `CONTENT_FILL`.
    // This is the ONE consumer touch: the kit puts paper under CONTENT, never under the desktop
    // (white board 2026-08-08), and this well is the only kernel-drawn content surface in the tree
    // — every other window's pixels belong to a ring-3 app. `super::paper` derives its base from
    // `theme::CONTENT_FILL`, so the flat fill this replaces is exactly the texture's mean; nothing
    // drawn on top (text, rows, buttons) moves by a pixel.
    fill(px, 0, 0, W, H, theme::CHROME_FACE);
    bevel(px, 4, 4, W - 8, H - 8, false);
    super::paper::fill_rect(
        px,
        W,
        4 + theme::BEVEL,
        4 + theme::BEVEL,
        W - 8 - 2 * theme::BEVEL,
        H - 8 - 2 * theme::BEVEL,
    );

    let lx = 24;
    match st {
        State::Choose => {
            text(px, lx, 20, b"Install UnaOS", theme::CONTENT_TEXT);
            fill(px, lx, 42, W - 2 * lx, 2, theme::FRAME_LINE);
            text(px, lx, 54, b"Choose a target disk:", theme::CONTENT_TEXT);

            let mut devs: [Option<Row>; 2] = [None; 2];
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
            let mut boot_rows = 0usize;
            for i in 0..n {
                let row = devs[i].unwrap();
                let d = row.info;
                let ry = 84 + i * (CELL + 22);
                // INSTALL-SELF: a marked row never carries the selection highlight — selection cannot
                // land on it (see `step_selectable`), so painting one would be a lie about what Enter
                // would do.
                let selected = i == sel && !row.boot;
                let row_bg = if selected { theme::SCROLL_THUMB } else { theme::CONTENT_FILL };
                fill(px, lx, ry - 4, W - 2 * lx, CELL + 12, row_bg);
                if selected {
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
                // INSTALL-SELF: the mark. The visible width of a row is 28 cells; `slot<n>` + the
                // 16-byte product name ends at 22, so the tag lands in the tail the size string is
                // already clipped out of. Dimmed text carries the "not available" reading the CRISPY
                // theme uses everywhere else for an inert control.
                let fg = if row.boot {
                    line[22..27].copy_from_slice(b" BOOT");
                    theme::TITLE_TEXT_INACTIVE
                } else {
                    theme::CONTENT_TEXT
                };
                if row.boot {
                    boot_rows += 1;
                }
                text(px, lx + 8, ry, &line[..(W - 2 * lx - 16) / CELL], fg);
            }
            let selectable = first_selectable(&devs, n).is_some();
            // INSTALL-SELF: say WHY a listed disk cannot be chosen. A greyed row with no explanation
            // is the kind of thing an operator works around by rebooting into something less careful.
            if boot_rows > 0 {
                let ry = 84 + n * (CELL + 22) + 4;
                text(px, lx, ry, b"BOOT = the disk this system", theme::TITLE_TEXT_INACTIVE);
                text(px, lx, ry + CELL + 4, b"booted from. Not installable.", theme::TITLE_TEXT_INACTIVE);
            }
            if n > 0 && !selectable {
                let ry = 84 + n * (CELL + 22) + 2 * (CELL + 4) + 8;
                text(px, lx, ry, b"Attach another disk to", theme::CONTENT_TEXT);
                text(px, lx, ry + CELL + 4, b"install onto.", theme::CONTENT_TEXT);
            }
            // Exits are ALWAYS on screen: an installer that can only go forward is a trap.
            text(px, lx, H - 100, b"w/s select   Enter continue", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, H - 100 + CELL + 4, b"Esc boot this live system", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, H - 100 + 2 * (CELL + 4), b"q  halt the machine", theme::TITLE_TEXT_INACTIVE);
            if n > 0 && selectable {
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
            // INSTALL-SEL: the device named here is resolved from the COMMITTED identity through
            // `block::lookup` — the same function, against the same live registry, that
            // `install::BlockTarget::bind_id` calls at go-time. It is deliberately NOT read from the
            // highlighted chooser row: the row is a position in a list that is rebuilt every frame,
            // and the whole defect was that glass and engine were reading two different things. With
            // one resolver there is nothing left to disagree about — if this screen can name a disk,
            // the engine binds that disk, and if it cannot, the engine refuses.
            let pending = *PENDING.lock();
            let bound = pending.and_then(|(id, _)| block::lookup(id).map(|d| (id, d)));
            match bound {
                Some((_, d)) => {
                    text(px, lx + 14, 72, b"THIS ERASES EVERYTHING", theme::CONTENT_TEXT);
                    text(px, lx + 14, 72 + CELL + 6, b"on the disk named below:", theme::CONTENT_TEXT);
                    // Product is the operator-legible name; vendor + slot + size are what let them
                    // tell two similar sticks apart on a bench where both are plugged in.
                    text(px, lx + 14, 72 + 2 * (CELL + 6), &d.product, theme::CONTENT_TEXT);
                    let mut line = [b' '; 26];
                    line[..8].copy_from_slice(&d.vendor);
                    line[9..13].copy_from_slice(b"slot");
                    line[13] = b'0' + (d.slot_id % 10);
                    let mut sz = [0u8; 16];
                    let s = fmt_size(&mut sz, d.num_blocks * d.block_size as u64);
                    let tail = 26 - s.len();
                    line[tail..].copy_from_slice(s);
                    text(px, lx + 14, 72 + 3 * (CELL + 6), &line, theme::CONTENT_TEXT);
                }
                None => {
                    // The chosen disk left between the chooser and this frame (a disconnect retracts
                    // its registry entry). Say so plainly and withdraw the Install affordance — the
                    // engine would refuse anyway, but an installer must not offer a go it knows is
                    // dead, and it must never quietly re-aim at a disk still in the list.
                    text(px, lx + 14, 72, b"The disk you chose is no", theme::CONTENT_TEXT);
                    text(px, lx + 14, 72 + CELL + 6, b"longer attached.", theme::CONTENT_TEXT);
                    text(px, lx + 14, 72 + 2 * (CELL + 6), b"Nothing has been written.", theme::CONTENT_TEXT);
                    text(px, lx + 14, 72 + 3 * (CELL + 6), b"Esc back, then choose again.", theme::TITLE_TEXT_INACTIVE);
                }
            }
            text(px, lx, 200, b"The engine refuses non-blank", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, 200 + CELL + 4, b"targets (blank-check guard).", theme::TITLE_TEXT_INACTIVE);
            if bound.is_some() {
                text(px, lx, H - 92, b"Enter install    Esc back", theme::TITLE_TEXT_INACTIVE);
                button(px, W - 190, H - 52, 160, b"Install", true);
            } else {
                text(px, lx, H - 92, b"Esc back", theme::TITLE_TEXT_INACTIVE);
            }
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
        State::Gone => {
            // INSTALL-SEL: the go-time refusal. Reached when the identity resolved on the warning
            // screen but not at the instant the engine bound it (or when it never resolved and a key
            // still arrived). The one thing worth saying loudest is the thing an operator will
            // actually worry about: no disk was touched, including the ones still attached.
            text(px, lx, 20, b"Target disk is gone", theme::CONTENT_TEXT);
            fill(px, lx, 42, W - 2 * lx, 2, theme::FRAME_LINE);
            text(px, lx, 84, b"The disk you selected was not", theme::CONTENT_TEXT);
            text(px, lx, 84 + CELL + 6, b"attached when the install was", theme::CONTENT_TEXT);
            text(px, lx, 84 + 2 * (CELL + 6), b"about to begin. NOTHING was", theme::CONTENT_TEXT);
            text(px, lx, 84 + 3 * (CELL + 6), b"written - not to it, and not", theme::CONTENT_TEXT);
            text(px, lx, 84 + 4 * (CELL + 6), b"to any other disk.", theme::CONTENT_TEXT);
            text(px, lx, H - 92, b"Enter choose again   Esc close", theme::TITLE_TEXT_INACTIVE);
            button(px, W - 190, H - 52, 160, b"Choose", true);
        }
        State::Closed => {}
    }

    let id = WIN.load(Ordering::Relaxed);
    if id != wm::WIN_NONE {
        wm::present(id);
    }
}

// ------------------------------------------------------------------- verbs --

/// Open the installer window (called from `desktop_uefi::activate` when the `instgui`
/// feature is armed). Spawn-place discipline: geometry settled before the row.
pub fn open() {
    let mut st = STATE.lock();
    if *st != State::Closed {
        return;
    }
    *st = State::Choose;
    drop(st);
    // INSTALL-SEL: a fresh dialog starts with no committed target. (The static outlives any single
    // dialog, and a commitment from a previous open would be a stale name for a disk chosen under a
    // different list.)
    *PENDING.lock() = None;
    // INSTALL-SELF: open on a row the operator is allowed to choose. `SEL` outlives the dialog, and
    // row 0 is the boot device on exactly the machine this arc exists for (booted from the only stick
    // attached), so defaulting to 0 would open with the highlight on an unusable row.
    {
        let mut devs: [Option<Row>; 2] = [None; 2];
        let n = devices(&mut devs);
        SEL.store(first_selectable(&devs, n).unwrap_or(0) as u8, Ordering::Relaxed);
    }
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
    WIN.store(id, Ordering::Relaxed); wm::winid_register_holder(&WIN, "instgui"); // WINID (SO1(b)) — ⚠ SAME-LINE fold, line-NEUTRAL. Registered on the same argument as pulsewin's and quarry's cells: this one is cleared by this module's own `close()` and by nothing else.
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
    *PENDING.lock() = None;
    // The console gets the glass back and repaints everything it accumulated.
    super::fbcon::console_present_suspend(false);
    serial_println!("[wc-x] instgui closed — console presents resumed, booting on");
}

/// Main-loop hook, every frame: re-check the disk list (storage enumerates long after the
/// dialog opens) and repaint only when it actually changed, so this costs nothing per frame
/// on a settled machine.
pub fn service() {
    // INSTALL-SEL: the WARNING screen is now live too — its device name is resolved from the registry
    // on every paint, so a disk that vanishes while the operator is reading the warning must flip that
    // screen to its "no longer attached" face rather than leaving a stale name on glass in front of an
    // Install button. Both screens are driven off the same list signature.
    let st = *STATE.lock();
    if st != State::Choose && st != State::Warn {
        return;
    }
    let mut devs: [Option<Row>; 2] = [None; 2];
    let n = devices(&mut devs);
    let sig = devs
        .iter()
        .flatten()
        .fold(n as u64, |a, r| a ^ (r.info.slot_id as u64) << 8 ^ r.info.num_blocks);
    if LAST_SIG.swap(sig, Ordering::Relaxed) != sig {
        // USB-UNPLUG: the list can now SHRINK (a disconnect retracts the block-registry entry via
        // `block::unpublish_usb_geometry`), which it never could before — attach was the only event
        // the registry reported. A selection index left pointing past the end of the shortened list
        // would silently be clamped to row 0 by the warning screen, i.e. the operator's highlighted
        // choice would become a DIFFERENT disk than the one they were looking at. Clamp here, where
        // the change is detected and before the repaint that redraws the highlight.
        // (INSTALL-SEL note: the clamp keeps the HIGHLIGHT truthful, which is all an index can do.
        // What the engine receives is no longer this index but the identity frozen at Enter — see
        // `PENDING` — so a shrink can no longer retarget an install even in the window between this
        // detection and the next frame.)
        // (INSTALL-SELF: the clamp now also has to land on a SELECTABLE row — a shrink can leave the
        // index on a disk the boot-device guard marked, and a highlight on a row Enter refuses is the
        // same class of lie the row clamp was added to prevent. `first_selectable` is the fallback
        // rather than row 0 for exactly that reason.)
        let last = n.saturating_sub(1) as u8;
        let cur = SEL.load(Ordering::Relaxed);
        let cur = if cur > last { last } else { cur };
        let fixed = match devs.get(cur as usize).copied().flatten() {
            Some(r) if !r.boot => cur,
            _ => first_selectable(&devs, n).unwrap_or(cur as usize) as u8,
        };
        SEL.store(fixed, Ordering::Relaxed);
        repaint();
    }
}

/// Signature of the last disk list painted (count ^ slot ^ size), so `service` repaints on a
/// real change rather than every frame.
static LAST_SIG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// The installer's `q` = halt, as the word is meant on a machine that can be switched off.
///
/// Live bench evidence (rMBP 2012, attended boot): the old `loop { hlt() }` stopped the kernel but
/// left the laptop powered — fans on, battery draining, and the only way out a power-button hold.
/// On x86 this now asks ACPI for a real S5 soft-off; `acpi_power::poweroff` falls back to exactly
/// that `hlt` loop, with a witness line naming the reason, whenever the firmware tables do not
/// yield the sleep type honestly (see the refusal list in `arch::x86_64::acpi_power`). On aarch64
/// there is no equivalent yet — the Pi has no soft-off at all and the Jetson's path is PSCI, which
/// belongs to that track's lane — so it keeps parking the CPU.
fn halt_machine() -> ! {
    #[cfg(target_arch = "x86_64")]
    crate::arch::acpi_power::poweroff();
    #[cfg(not(target_arch = "x86_64"))]
    crate::hlt_loop();
}

/// Main-loop hook: returns true if the key belonged to the installer.
pub fn consume_key(c: u8) -> bool {
    let st = *STATE.lock();
    if st == State::Closed {
        return false;
    }
    // Halt is offered from every screen: an installer must always be leaveable.
    if c == b'q' && st != State::Running {
        serial_println!("[wc-x] instgui halt requested — powering the machine off");
        close();
        halt_machine();
    }
    match (st, c) {
        (State::Choose, b'\x1b') => close(),
        // INSTALL-SELF: both directions step over rows the guard marked, so the highlight can only ever
        // rest on a disk the engine would accept.
        (State::Choose, b'w') | (State::Choose, b'A') => {
            let mut devs: [Option<Row>; 2] = [None; 2];
            let n = devices(&mut devs);
            let s = SEL.load(Ordering::Relaxed) as usize;
            SEL.store(step_selectable(&devs, n, s, -1) as u8, Ordering::Relaxed);
            repaint();
        }
        (State::Choose, b's') | (State::Choose, b'B') => {
            let mut devs: [Option<Row>; 2] = [None; 2];
            let n = devices(&mut devs);
            let s = SEL.load(Ordering::Relaxed) as usize;
            SEL.store(step_selectable(&devs, n, s, 1) as u8, Ordering::Relaxed);
            repaint();
        }
        (State::Choose, b'\r') | (State::Choose, b'\n') => {
            // INSTALL-SEL: this is the commit point. Freeze the highlighted ROW into a durable
            // identity here, while the list the operator is looking at is still the list on screen,
            // and hand THAT forward. Everything past this line resolves the identity; nothing past
            // this line uses the index. The row number rides along only so the go-time witness can be
            // read against what the operator saw.
            let mut devs: [Option<Row>; 2] = [None; 2];
            let n = devices(&mut devs);
            // No disk, no warning screen — advancing to "erase what?" would be nonsense. Likewise a
            // selection that does not name a row (the list shrank in this very frame) must not
            // advance: there is nothing to warn about, and inventing a substitute is the defect.
            let sel = SEL.load(Ordering::Relaxed) as usize;
            let Some(row) = devs.get(sel).copied().flatten().filter(|_| n > 0) else {
                return true;
            };
            // INSTALL-SELF: the last UI gate. Selection is supposed to be unable to rest here, so
            // reaching this branch means the guard's verdict changed under the dialog (a volume with
            // the boot serial appeared on the highlighted disk between frames). Refuse to advance and
            // say so on the wire — the engine would refuse anyway, and an installer must not walk an
            // operator up to a go it already knows is dead.
            if row.boot {
                serial_println!(
                    "[wc-x] instgui Enter on the BOOT device (row {}, slot {}) — not selectable, refusing to advance",
                    sel, row.id.slot_id
                );
                repaint();
                return true;
            }
            *PENDING.lock() = Some((row.id, sel as u8));
            serial_println!(
                "[wc-x] instgui selected row {} -> {:?} slot {} ({} sectors)",
                sel, row.id.handle, row.id.slot_id, row.id.num_blocks
            );
            *STATE.lock() = State::Warn;
            repaint();
        }
        (State::Warn, b'\x1b') => {
            *PENDING.lock() = None;
            *STATE.lock() = State::Choose;
            repaint();
        }
        (State::Warn, b'\r') | (State::Warn, b'\n') => {
            // The attended go. Present the Running frame FIRST (the engine is
            // synchronous on this thread), then run, then verdict.
            //
            // INSTALL-SEL: the go carries the committed identity. There is no "current selection"
            // read here and no fallback: if `PENDING` is somehow empty (it cannot be, on the path
            // that reaches Warn, but an empty target is the one thing that must never be guessed at)
            // we take the same refusal road as a vanished disk.
            let pending = *PENDING.lock();
            let Some((id, row)) = pending else {
                serial_println!("[wc-x] instgui install-go with NO committed target — refusing");
                *STATE.lock() = State::Gone;
                repaint();
                return true;
            };
            *STATE.lock() = State::Running;
            repaint();
            serial_println!("[wc-x] instgui install-go (attended Enter on warn screen)");
            match crate::install::run_gui(id, row) {
                crate::install::GuiOutcome::Pass => *STATE.lock() = State::Done(true),
                crate::install::GuiOutcome::Refused => *STATE.lock() = State::Done(false),
                // The engine bound nothing and wrote nothing; the operator gets the specific screen.
                crate::install::GuiOutcome::TargetGone => *STATE.lock() = State::Gone,
            }
            repaint();
        }
        (State::Gone, b'\r') | (State::Gone, b'\n') => {
            // Back to the chooser with the stale commitment dropped, so the next Enter freezes a
            // fresh identity out of the current list rather than reviving a dead one.
            *PENDING.lock() = None;
            *STATE.lock() = State::Choose;
            repaint();
        }
        (State::Gone, b'\x1b') => close(),
        (State::Done(_), b'\x1b') | (State::Done(_), b'\r') | (State::Done(_), b'\n') => close(),
        _ => {}
    }
    true
}
