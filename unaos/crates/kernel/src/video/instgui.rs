// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! INSTGUI — the first graphical installer (x86, `wc` + `instgui` features).
//!
//! A kernel-owned compositor window, drawn in the CRISPY theme's const table
//! ([`super::theme`]), that walks Peter through: **choose disk → census → install
//! one partition → verdict**.
//!
//! ### INSTALLVERB: the go-button is two presses, and the first one only reads
//! The dialog's first form ran the WHOLE-DISK engine on one attended Enter. On the bench rMBP that
//! button is aimed at the disk Catalina lives on (rmbp-ledger B91), and RULINGS R25 is about
//! exactly that disk: *"if UnaOS saw catalina and immediately formatted the disk as an alien
//! enemy"*. So the press that used to install now takes the read-only census
//! ([`crate::install::partition::census`]) and paints what is on the medium; the SECOND press, on a
//! partition the engine's own ladder passed, calls
//! [`crate::install::partition::install_into_partition`] for that ONE partition. The whole-disk
//! engine is reachable from this dialog only when the census found no volume that is not ours —
//! asked at the affordance and asked again at the go. It is a *face* on the installer engine, never a new
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
use super::font;
use super::theme;
use super::wm;

/// Content surface dimensions (pixels). The compositor's scale rule may magnify.
const W: usize = 520;
const H: usize = 396;

/// FONTSURF (SO48) — **the dialog's face, and the metric split that came with it.**
///
/// WAS: `TS = 2`, `CELL = 8 * TS` — the raw `font8x8` cell block-replicated 2x into 16 px glyphs,
/// one square 16x16 cell serving as BOTH the advance and the row height. That is the 1-bit face
/// `video::font`'s module doc lists as a gap rather than a fold, and at 2x every set bit is a 2x2
/// square of flat ink: the staircase is the glyph, magnified. The compositor may then magnify the
/// whole surface again on a dense panel ([`wm`]'s integer upscale), which multiplies the block, not
/// the detail.
///
/// NOW: the shared anti-aliased face at [`font::Face::Body`], through [`font::draw_text`].
///
/// The two constants are SPLIT because the square cell was an artefact of the 1-bit table and not a
/// metric anyone chose: a 16 px mono face is 7 px wide, which is the advance Noto's own side
/// bearings give it. [`CELL_H`] is [`font::CELL_H`] = 16, **the same 16 the old `CELL` was**, so
/// every vertical position in this module's layout is unchanged to the pixel; [`CELL_W`] is the
/// face's advance and is the only axis that moves. Lines are therefore narrower and the hand-broken
/// copy below keeps its breaks — a line that fitted at 16 px per character cannot fail to fit at 7.
const CELL_W: usize = font::CELL_W;
const CELL_H: usize = font::CELL_H;
/// The face this dialog draws with. `Body` and not `Chrome`: the installer's surface is a block of
/// prose sized by how much of it must fit, not a piece of furniture sized by the theme's bar.
const FACE: font::Face = font::Face::Body;

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
    /// INSTALLVERB: **the first press, and it writes nothing.** The go-button used to run the
    /// whole-disk engine on the disk the operator had just highlighted — on the bench rMBP, that
    /// button is pointed at Catalina's disk (rmbp-ledger B91). It now runs the READ-ONLY census
    /// (`install::partition::census`) and paints what is actually on the medium, partition by
    /// partition, with the refusal each one would give. The SECOND press, on a selected empty
    /// partition, is the only thing in this dialog that can write, and it writes through
    /// `install_into_partition` — one partition, never the disk.
    Census,
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
/// QUITLEAK — **closes that ran [`close`]'s teardown.** A `Quit` that took a bare `wm::close(win)`
/// leaves A29's WINID holder registry to clear [`WIN`], so every END-STATE question about the
/// window answers exactly as a correct close would — and the console stays suspended for the rest
/// of the boot. This counter is the discriminator, as `pulsewin::CLOSES` is for A30's pulse round.
static CLOSES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// QUITLEAK — **the last value THIS MODULE published to `fbcon::console_present_suspend`.**
///
/// Said precisely, because it is a mirror and not the flag: `fbcon`'s `CONSOLE_PRESENT_SUSPENDED`
/// has no getter, and `video/fbcon.rs` is outside this arc's file list, so the fixture reads what
/// this module pushed rather than what `fbcon` holds. That is enough to convict the defect it is
/// aimed at and the reason is the defect's own shape: the stranding is that NOBODY CALLS the
/// resume, so the mirror is still `true` — it cannot be faked green by a close that skipped the
/// call, because the mirror is written on the same line as the call and by nothing else. What it
/// does NOT cover is a third party storing into `fbcon`'s cell behind this module's back
/// (`video/login.rs` is the only other caller in the tree, and only while its own screen is up).
/// A one-line `fbcon::console_present_suspended()` getter would close that gap; it is reported.
static SUSPEND_MIRROR: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Which list row is selected (the device list is tiny; a u8 outlives it).
static SEL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// INSTALLVERB: one partition as the census painted it. A SNAPSHOT, not a live read — the census is
/// taken once, on the press that enters [`State::Census`], and the screen shows exactly what that
/// read found. Re-probing on every repaint would let the glass and the go disagree, which is the
/// defect INSTALL-SEL spent an arc removing one level up.
#[derive(Clone, Copy)]
struct PartRow {
    index: u32,
    mib: u64,
    /// What the content probe found (`empty`, `FAT`, `APFS`, `HFS+`, `UNAFS`, `ESP`, `unknown`).
    tag: &'static str,
    /// **Is this slot EMPTY — nobody's?** This, and not the full refusal ladder, is what selection
    /// turns on, and the split is deliberate. The question the DIALOG must answer structurally is
    /// R25's: never offer somebody else's volume. Every other question — size, ESP type, transport,
    /// boot device — belongs to the engine, which re-asks all of them at the go and names the one it
    /// refuses on. Keying selection on the preview instead would make the dialog's reach depend on
    /// `INSTALL_PREVIEW_TREE_BYTES`, an APPROXIMATION of the tree size (see its doc in `shell.rs`),
    /// so an over-estimate would silently hide a partition the engine would have taken.
    empty: bool,
    /// The engine's PREVIEW verdict for this slot, as `check_partition` returned it: `None` when
    /// every guard passed, `Some(reason)` carrying the API's own stable token otherwise.
    refusal: Option<&'static str>,
}

/// INSTALLVERB: **a request to open the dialog, to be honoured by the MAIN LOOP and not by the
/// caller.** `install --gui` arrives on the console's key path, deep inside the shell's dispatch,
/// and [`open`] is a spawn-place operation: it takes `wm::spawn_geometry`, creates a window, takes
/// the framebuffer's info lock and presents. Called straight from that context the first attempt
/// FAULTED — `[panel-owner] panel-ownership-handover … site=fbcon::panic_screen` and a `#DB` with a
/// junk frame, on the very pass the window was created. So the verb sets this flag and [`service`],
/// which already runs every main-loop pass and is where `desktop_uefi::activate` opens the dialog
/// from, does the opening. Same door as the boot path, one place, no second spawn context.
static OPEN_REQ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// INSTALLVERB: the census the current dialog is showing, or empty when none has been taken.
static PARTS: spin::Mutex<alloc::vec::Vec<PartRow>> = spin::Mutex::new(alloc::vec::Vec::new());
/// INSTALLVERB: which partition row is highlighted. Selection can only rest on an installable row
/// (see [`step_part`]), so the second press can never be aimed at a stranger's volume.
static PSEL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// INSTALLVERB: did the chosen disk carry a GPT this kernel could read? Three-valued in effect —
/// `false` here plus [`WHOLE_OK`] `true` is the blank scratch (no table at all, the whole-disk demo's
/// own disk); `true` plus `WHOLE_OK` `false` is Peter's rMBP.
static HAS_GPT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// INSTALLVERB: **may the whole-disk engine be offered for this disk at all?** `false` the moment
/// the census finds one foreign or friend volume — R25 (*"if UnaOS saw catalina and immediately
/// formatted the disk as an alien enemy"*), and rmbp-ledger B91's ordering constraint: the guard
/// lands before the SATA write path that would make the rMBP's internal SSD reachable. Set ONLY by
/// [`run_census`], from `partition::check_whole_disk`'s own answer, and re-asked at the go.
static WHOLE_OK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

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

// ------------------------------------------------- INSTALLVERB: the census --

/// INSTALLVERB: the SHORT form of a refusal, for a 28-cell row. The FULL token — `partition-not-
/// empty`, `partition-foreign-type`, `transport-read-only` — is on the serial wire from
/// `Refusal::say`, unchanged and unparaphrased, because that token is the API and the docs' refusal
/// table is keyed on it. This is the glass's abbreviation of it and nothing else reads it.
fn glass_reason(token: &'static str) -> &'static str {
    match token {
        "partition-not-empty" => "in use",
        "partition-foreign-type" => "not ours",
        "partition-is-esp" => "ESP",
        "partition-too-small" => "too small",
        "transport-read-only" => "read-only",
        "boot-device" => "boot disk",
        "no-such-partition" => "no slot",
        _ => "refused",
    }
}

/// INSTALLVERB: take the census of the committed disk and publish it for the screen.
///
/// READ-ONLY, top to bottom: `census` probes each partition's head through the DISK target,
/// `check_whole_disk` and `check_partition` are pure functions over what it found, and no writable
/// partition target is built anywhere in here. Every refusal it evaluates is ALSO said on the wire
/// by the API itself, so a run that only censused still leaves the full verdict table in the log.
fn run_census(id: block::BlockDeviceId) {
    use crate::install::{partition, InstallTarget};
    let mut rows: alloc::vec::Vec<PartRow> = alloc::vec::Vec::new();
    let mut has_gpt = false;
    let mut whole_ok = false;
    match crate::install::BlockTarget::bind_id(id) {
        Err(e) => {
            serial_println!("[wc-x] instgui census — the disk did not bind ({:?}); nothing read, nothing written", e);
        }
        Ok(t) => match partition::census(&t) {
            Err(e) => {
                // NO READABLE GPT. That is the blank scratch disk the whole-disk demo exists for,
                // and it is the ONE shape in which that demo stays reachable: there are no
                // partitions to census, so there is no foreign volume to stand it down.
                // PARTINSTALL's words for the other case: "stood down by content".
                serial_println!(
                    "[wc-x] instgui census — no readable GPT ({:?}): no partitions to install into, and the whole-disk demo stays available on this disk",
                    e
                );
                whole_ok = true;
            }
            Ok(c) => {
                has_gpt = true;
                partition::print_census(&t.id(), &c);
                whole_ok = match partition::check_whole_disk(&c) {
                    Ok(()) => true,
                    Err(r) => {
                        r.say("instgui:disk");
                        false
                    }
                };
                for row in &c.rows {
                    let refusal = match partition::check_partition(
                        &c,
                        id,
                        row.entry.index,
                        crate::shell::INSTALL_PREVIEW_TREE_BYTES,
                        false,
                    ) {
                        Ok(()) => None,
                        Err(r) => {
                            r.say(&alloc::format!("instgui:part{}", row.entry.index));
                            Some(r.reason())
                        }
                    };
                    rows.push(PartRow {
                        index: row.entry.index,
                        mib: row.entry.sectors() * 512 / (1024 * 1024),
                        tag: row.content.tag(),
                        empty: row.content.is_installable(),
                        refusal,
                    });
                }
            }
        },
    }
    let installable = rows.iter().filter(|r| r.refusal.is_none()).count();
    HAS_GPT.store(has_gpt, Ordering::Relaxed);
    WHOLE_OK.store(whole_ok, Ordering::Relaxed);
    // Open on a slot that is nobody's — preferring one the preview also passed, so the highlight
    // lands on the partition an operator would pick when there is one.
    PSEL.store(
        rows.iter()
            .position(|r| r.empty && r.refusal.is_none())
            .or_else(|| rows.iter().position(|r| r.empty))
            .unwrap_or(0) as u8,
        Ordering::Relaxed,
    );
    *PARTS.lock() = rows;
    serial_println!(
        "[wc-x] instgui census step=1 gpt={} parts={} installable={} whole_disk_offered={} — READ-ONLY, nothing written",
        has_gpt as u8,
        PARTS.lock().len(),
        installable,
        whole_ok as u8
    );
}

/// INSTALLVERB: the next EMPTY partition row in `dir`, or `cur` when there is none that way.
///
/// Rows carrying somebody's filesystem are stepped over the way the device chooser steps over the
/// boot device, and that is the structural half of the R25 guard on this screen: selection CANNOT
/// rest on a stranger's volume, so the second press cannot be aimed at one. Rows that are empty but
/// which the engine would refuse for another reason (too small, ESP-typed, a transport that cannot
/// write) DO take the highlight, carry that reason on the glass, and get it again from the engine
/// itself when pressed — see [`PartRow::empty`] for why the dialog does not arbitrate those.
fn step_part(cur: usize, dir: isize) -> usize {
    let parts = PARTS.lock();
    let mut i = cur as isize;
    loop {
        i += dir;
        if i < 0 || i >= parts.len() as isize {
            return cur;
        }
        if parts[i as usize].empty {
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

/// FONTSURF (SO48) — the dialog's text, on the shared anti-aliased face.
///
/// The `\n` break is kept and applied HERE rather than being handed to [`font::draw_text`]: the
/// shared blit is deliberately face-and-clip only, with no opinion about control bytes, and this
/// module is the only caller in the tree that treats a newline as an end-of-string. Everything
/// after it — the advance, the all-or-nothing clip at `W`, the pen — is the shared blit's, so this
/// surface and any other that adopts the seam truncate identically.
fn text(px: &mut [u32], x: usize, y: usize, s: &[u8], fg: u32) {
    let n = s.iter().position(|&c| c == b'\n').unwrap_or(s.len());
    font::draw_text(px, W, W, H, x, y, &s[..n], fg, false, FACE);
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
    let tx = x + (w.saturating_sub(label.len() * CELL_W)) / 2;
    text(px, tx, y + (h - CELL_H) / 2, label, theme::BUTTON_TEXT);
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
                text(px, lx, 84 + CELL_H + 6, b"enumeration. Attach a disk and", theme::TITLE_TEXT_INACTIVE);
                text(px, lx, 84 + 2 * (CELL_H + 6), b"it appears here by itself.", theme::TITLE_TEXT_INACTIVE);
            }
            let mut boot_rows = 0usize;
            for i in 0..n {
                let row = devs[i].unwrap();
                let d = row.info;
                let ry = 84 + i * (CELL_H + 22);
                // INSTALL-SELF: a marked row never carries the selection highlight — selection cannot
                // land on it (see `step_selectable`), so painting one would be a lie about what Enter
                // would do.
                let selected = i == sel && !row.boot;
                let row_bg = if selected { theme::SCROLL_THUMB } else { theme::CONTENT_FILL };
                fill(px, lx, ry - 4, W - 2 * lx, CELL_H + 12, row_bg);
                if selected {
                    rect(px, lx, ry - 4, W - 2 * lx, CELL_H + 12, theme::ACCENT);
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
                text(px, lx + 8, ry, &line[..(W - 2 * lx - 16) / CELL_W], fg);
            }
            let selectable = first_selectable(&devs, n).is_some();
            // INSTALL-SELF: say WHY a listed disk cannot be chosen. A greyed row with no explanation
            // is the kind of thing an operator works around by rebooting into something less careful.
            if boot_rows > 0 {
                let ry = 84 + n * (CELL_H + 22) + 4;
                text(px, lx, ry, b"BOOT = the disk this system", theme::TITLE_TEXT_INACTIVE);
                text(px, lx, ry + CELL_H + 4, b"booted from. Not installable.", theme::TITLE_TEXT_INACTIVE);
            }
            if n > 0 && !selectable {
                let ry = 84 + n * (CELL_H + 22) + 2 * (CELL_H + 4) + 8;
                text(px, lx, ry, b"Attach another disk to", theme::CONTENT_TEXT);
                text(px, lx, ry + CELL_H + 4, b"install onto.", theme::CONTENT_TEXT);
            }
            // Exits are ALWAYS on screen: an installer that can only go forward is a trap.
            text(px, lx, H - 100, b"w/s select   Enter continue", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, H - 100 + CELL_H + 4, b"Esc boot this live system", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, H - 100 + 2 * (CELL_H + 4), b"q  halt the machine", theme::TITLE_TEXT_INACTIVE);
            if n > 0 && selectable {
                button(px, W - 190, H - 52, 160, b"Continue", true);
            }
        }
        State::Census => {
            // INSTALLVERB: THE FIRST PRESS' SCREEN. Nothing here can write; it is the census, on
            // glass, in the same words the serial log carries. What the operator is being shown is
            // the DISK'S OWN CONTENT — read off the medium, not read off the partition table's
            // declarations — because that is what decides whether a slot is ours to take.
            text(px, lx, 20, b"What is on this disk", theme::CONTENT_TEXT);
            fill(px, lx, 42, W - 2 * lx, 2, theme::FRAME_LINE);
            let parts = PARTS.lock();
            let psel = PSEL.load(Ordering::Relaxed) as usize;
            let whole_ok = WHOLE_OK.load(Ordering::Relaxed);
            let has_gpt = HAS_GPT.load(Ordering::Relaxed);
            let selectable = parts.iter().any(|p| p.empty);
            if !has_gpt {
                text(px, lx, 62, b"No partition table here.", theme::CONTENT_TEXT);
                text(px, lx, 62 + CELL_H + 6, b"Nothing to install into.", theme::TITLE_TEXT_INACTIVE);
            }
            for (i, pr) in parts.iter().enumerate() {
                let ry = 62 + i * (CELL_H + 10);
                if ry + CELL_H > H - 118 {
                    break; // the fixture disk has five; a bigger table simply paints what fits
                }
                let sel = i == psel && pr.empty;
                fill(px, lx, ry - 3, W - 2 * lx, CELL_H + 6, if sel { theme::SCROLL_THUMB } else { theme::CONTENT_FILL });
                if sel {
                    rect(px, lx, ry - 3, W - 2 * lx, CELL_H + 6, theme::ACCENT);
                }
                let label = match pr.refusal {
                    None => "install here",
                    Some(t) => glass_reason(t),
                };
                let line = alloc::format!("p{} {:>5}M {:<7} {}", pr.index, pr.mib, pr.tag, label);
                let fg = if pr.refusal.is_none() { theme::CONTENT_TEXT } else { theme::TITLE_TEXT_INACTIVE };
                text(px, lx + 6, ry, line.as_bytes(), fg);
            }
            // THE WHOLE-DISK AFFORDANCE, AND WHERE IT IS NOT. On a disk carrying anything that is
            // not ours the button is not disabled, not confirmed twice, not hidden behind a
            // modifier — it is ABSENT, and the reason is on the glass in place of it. R25.
            let ry = H - 116;
            if whole_ok {
                text(px, lx, ry, b"d  erase the WHOLE disk", theme::CONTENT_TEXT);
            } else {
                text(px, lx, ry, b"Whole-disk install is not", theme::TITLE_TEXT_INACTIVE);
                text(px, lx, ry + CELL_H + 2, b"offered: this disk holds", theme::TITLE_TEXT_INACTIVE);
                text(px, lx, ry + 2 * (CELL_H + 2), b"volumes that are not ours.", theme::TITLE_TEXT_INACTIVE);
            }
            text(px, lx, H - 52 + 4, if selectable {
                b"w/s pick  Enter install  Esc".as_slice()
            } else {
                b"Esc back   q halt".as_slice()
            }, theme::TITLE_TEXT_INACTIVE);
            if selectable {
                button(px, W - 190, H - 52, 160, b"Install", true);
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
                    text(px, lx + 14, 72 + CELL_H + 6, b"on the disk named below:", theme::CONTENT_TEXT);
                    // Product is the operator-legible name; vendor + slot + size are what let them
                    // tell two similar sticks apart on a bench where both are plugged in.
                    text(px, lx + 14, 72 + 2 * (CELL_H + 6), &d.product, theme::CONTENT_TEXT);
                    let mut line = [b' '; 26];
                    line[..8].copy_from_slice(&d.vendor);
                    line[9..13].copy_from_slice(b"slot");
                    line[13] = b'0' + (d.slot_id % 10);
                    let mut sz = [0u8; 16];
                    let s = fmt_size(&mut sz, d.num_blocks * d.block_size as u64);
                    let tail = 26 - s.len();
                    line[tail..].copy_from_slice(s);
                    text(px, lx + 14, 72 + 3 * (CELL_H + 6), &line, theme::CONTENT_TEXT);
                }
                None => {
                    // The chosen disk left between the chooser and this frame (a disconnect retracts
                    // its registry entry). Say so plainly and withdraw the Install affordance — the
                    // engine would refuse anyway, but an installer must not offer a go it knows is
                    // dead, and it must never quietly re-aim at a disk still in the list.
                    text(px, lx + 14, 72, b"The disk you chose is no", theme::CONTENT_TEXT);
                    text(px, lx + 14, 72 + CELL_H + 6, b"longer attached.", theme::CONTENT_TEXT);
                    text(px, lx + 14, 72 + 2 * (CELL_H + 6), b"Nothing has been written.", theme::CONTENT_TEXT);
                    text(px, lx + 14, 72 + 3 * (CELL_H + 6), b"Esc back, then choose again.", theme::TITLE_TEXT_INACTIVE);
                }
            }
            // INSTALLVERB: say WHICH engine this screen arms. It is the WHOLE-DISK one, and this
            // screen is now reachable only from a census that found nothing on the disk that is not
            // ours — so the sentence names the disk, not a blank-check.
            // FONTS2X carry: the SENTENCE is hw-rmbp's (INSTALLVERB replaced the blank-check wording
            // ad62cf09 still carried); the CELL -> CELL_H rename is FONTS2X's, and `font::CELL_H` is
            // the same 16 the old `CELL` was, so the layout is unchanged to the pixel.
            text(px, lx, 200, b"This erases the WHOLE disk.", theme::CONTENT_TEXT);
            text(px, lx, 200 + CELL_H + 4, b"The census found nothing here", theme::TITLE_TEXT_INACTIVE);
            text(px, lx, 200 + 2 * (CELL_H + 4), b"that is not ours.", theme::TITLE_TEXT_INACTIVE);
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
            text(px, lx, 84 + CELL_H + 6, b"Progress on the console window.", theme::TITLE_TEXT_INACTIVE);
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
                text(px, lx, 84 + CELL_H + 6, b"payload verified extent-by-", theme::CONTENT_TEXT);
                text(px, lx, 84 + 2 * (CELL_H + 6), b"extent. See console verdicts.", theme::CONTENT_TEXT);
            } else {
                // FONTS2X carry: hw-rmbp's sentence, FONTS2X's CELL -> CELL_H.
                text(px, lx, 84, b"The installer declined and", theme::CONTENT_TEXT);
                text(px, lx, 84 + CELL_H + 6, b"wrote nothing. The console", theme::CONTENT_TEXT);
                text(px, lx, 84 + 2 * (CELL_H + 6), b"log names the exact reason", theme::CONTENT_TEXT);
                text(px, lx, 84 + 3 * (CELL_H + 6), b"it gave.", theme::CONTENT_TEXT);
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
            text(px, lx, 84 + CELL_H + 6, b"attached when the install was", theme::CONTENT_TEXT);
            text(px, lx, 84 + 2 * (CELL_H + 6), b"about to begin. NOTHING was", theme::CONTENT_TEXT);
            text(px, lx, 84 + 3 * (CELL_H + 6), b"written - not to it, and not", theme::CONTENT_TEXT);
            text(px, lx, 84 + 4 * (CELL_H + 6), b"to any other disk.", theme::CONTENT_TEXT);
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
        b"Install UnaOS",
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
    super::fbcon::console_present_suspend(true); SUSPEND_MIRROR.store(true, Ordering::Release); // QUITLEAK — the mirror moves on the same line as the call it mirrors, so the two can never drift apart by an edit that touches one of them.
    serial_println!("[wc-x] instgui open win={} box={}x{} at ({},{}) (console presents suspended)", id, ow, oh, ox, oy);
    repaint();
}

/// QUITLEAK — **`pub`, and the visibility is the fix.** This dialog is the one window in the tree
/// whose teardown includes RESUMING THE CONSOLE'S PRESENTS, so a `Quit` that reaped the row with a
/// bare `wm::close(win)` left a desktop that never presents the console again — the same stranding
/// class `video/login.rs`'s LOGINCLOSE heals, arriving by a different door. `winmenu`'s app-menu
/// `Quit` arm therefore calls this for the window [`win`] names, and `wm::close` only for everything
/// else. Safe to call when not open: the `WIN` swap is the guard, and the resume below is idempotent.
///
/// POLARITY, stated because the module is narrowly gated: `instgui` compiles only under
/// `all(target_arch = "x86_64", feature = "wc", feature = "instgui")`, so making this `pub` widens
/// nothing on aarch64, adds no name to the knob-off `kernel8.img` (which has neither `wc` nor this
/// module) and adds no byte to either image `./arroyo knoboff wc` measures. The `winmenu` call site
/// carries the same three-term `cfg`, not `wc` alone.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id != wm::WIN_NONE {
        wm::close(id);
    }
    *STATE.lock() = State::Closed;
    *PENDING.lock() = None;
    // INSTALLVERB: the census dies with the dialog. It is a SNAPSHOT of one disk at one instant,
    // and a snapshot kept across a close would be the stalest possible thing to reopen onto.
    *PARTS.lock() = alloc::vec::Vec::new();
    WHOLE_OK.store(false, Ordering::Relaxed);
    HAS_GPT.store(false, Ordering::Relaxed);
    // The console gets the glass back and repaints everything it accumulated.
    super::fbcon::console_present_suspend(false); SUSPEND_MIRROR.store(false, Ordering::Release); CLOSES.fetch_add(1, Ordering::Release); // QUITLEAK — the mirror on the same line as the call, and the counter that says this path ran: a bypassed `Quit` reaches neither, which is what `winmenu::appquit_selftest` scores.
    serial_println!(
        "[wc-x] instgui closed win={} — console presents resumed (suspended={}), booting on (closes={})",
        id,
        SUSPEND_MIRROR.load(Ordering::Acquire) as u32,
        CLOSES.load(Ordering::Relaxed)
    );
}

/// INSTALLVERB: ask for the dialog on the next main-loop pass. The shell's `install --gui` calls
/// this and returns; nothing is created on the caller's stack. Idempotent, and harmless when the
/// dialog is already up ([`open`] returns on a non-`Closed` state).
pub fn request_open() {
    OPEN_REQ.store(true, Ordering::Relaxed);
}

/// QUITLEAK — **the dialog's window id, or [`wm::WIN_NONE`].** `winmenu`'s `Quit` arm asks this
/// whether the window it is about to reap is the installer's, the way it asks `pulsewin::win`.
pub fn win() -> wm::WinId {
    WIN.load(Ordering::Relaxed)
}

/// QUITLEAK — `(closes, presents_suspended_mirror)`. See [`CLOSES`] and [`SUSPEND_MIRROR`] for
/// exactly what the second value is and is not. Read by `winmenu::appquit_selftest`.
pub fn close_census() -> (u32, bool) {
    (CLOSES.load(Ordering::Acquire), SUSPEND_MIRROR.load(Ordering::Acquire))
}

/// QUITLEAK — is the dialog up? The fixture asks before it does anything, so a boot that opened the
/// installer gets it back exactly as it was found.
pub fn is_open() -> bool {
    *STATE.lock() != State::Closed
}

/// Main-loop hook, every frame: re-check the disk list (storage enumerates long after the
/// dialog opens) and repaint only when it actually changed, so this costs nothing per frame
/// on a settled machine.
pub fn service() {
    // INSTALLVERB: honour a deferred open FIRST, on the main loop, where every other window in this
    // module is created. See [`OPEN_REQ`] for the fault that put this here rather than at the verb.
    if OPEN_REQ.swap(false, Ordering::Relaxed) {
        open();
    }
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
            // INSTALLVERB: **THE FIRST PRESS ENDS HERE, IN A READ.** It used to end on the erase
            // warning, one Enter away from `run_gui` and the whole-disk engine — which on the bench
            // rMBP is the engine pointed at Catalina's disk (rmbp-ledger B91). It now takes the
            // census and shows it. The operator's next decision is made against what is ACTUALLY on
            // the medium rather than against a sentence about blank-checks.
            run_census(row.id);
            *STATE.lock() = State::Census;
            repaint();
        }
        // INSTALLVERB: the census screen's keys. w/s step between INSTALLABLE partitions only.
        (State::Census, b'w') | (State::Census, b'A') => {
            let s = PSEL.load(Ordering::Relaxed) as usize;
            PSEL.store(step_part(s, -1) as u8, Ordering::Relaxed);
            repaint();
        }
        (State::Census, b's') | (State::Census, b'B') => {
            let s = PSEL.load(Ordering::Relaxed) as usize;
            PSEL.store(step_part(s, 1) as u8, Ordering::Relaxed);
            repaint();
        }
        (State::Census, b'\x1b') => {
            *PARTS.lock() = alloc::vec::Vec::new();
            *STATE.lock() = State::Choose;
            repaint();
        }
        // INSTALLVERB: **THE SECOND PRESS — the only key in this dialog that writes.** It installs
        // into the ONE partition the highlight names, through
        // `install::partition::install_into_partition`, which re-runs its own census and its whole
        // refusal ladder at go-time: the snapshot on screen selects a target, it never authorises
        // one. `as_esp` is `false` here and there is no key that makes it true — the type-GUID edit
        // is an operator asking for it knowingly at the verb, not a button.
        (State::Census, b'\r') | (State::Census, b'\n') => {
            let pending = *PENDING.lock();
            let Some((id, _)) = pending else {
                serial_println!("[wc-x] instgui install-go with NO committed target — refusing");
                *STATE.lock() = State::Gone;
                repaint();
                return true;
            };
            let sel = PSEL.load(Ordering::Relaxed) as usize;
            let target = PARTS.lock().get(sel).copied();
            let Some(pr) = target.filter(|p| p.empty) else {
                // Selection cannot rest on a refused row, so reaching this means there is no
                // installable row at all. Say so and stay put; an installer must not invent a
                // target because a key was pressed.
                serial_println!(
                    "[wc-x] instgui Enter on the census with no empty partition — nothing to install into, nothing written"
                );
                repaint();
                return true;
            };
            *STATE.lock() = State::Running;
            repaint();
            serial_println!(
                "[wc-x] instgui install-go step=2 part={} (attended Enter on the census screen)",
                pr.index
            );
            match crate::install::partition::install_into_partition(id, pr.index, false) {
                Ok(w) if w.verified == w.files && w.files > 0 => {
                    serial_println!(
                        ":: INSTGUI: wrote part={} files={} bytes={} verified={}/{} -> PASS ::",
                        w.index, w.files, w.bytes, w.verified, w.files
                    );
                    *STATE.lock() = State::Done(true);
                }
                Ok(w) => {
                    serial_println!(
                        ":: INSTGUI: wrote part={} files={} bytes={} verified={}/{} -> FAIL ::",
                        w.index, w.files, w.bytes, w.verified, w.files
                    );
                    *STATE.lock() = State::Done(false);
                }
                Err(e) => {
                    // The refusal itself is already on the wire in the API's own words, with its
                    // stable `reason=` token (`Refusal::say`). This line is the GUI's disposition,
                    // not a second verdict.
                    serial_println!(
                        "[wc-x] instgui part-install part={} refused ({:?}) — nothing was written",
                        pr.index, e
                    );
                    *STATE.lock() = State::Done(false);
                }
            }
            repaint();
        }
        // INSTALLVERB: `d` — the WHOLE-DISK demo, and the only door left to it in this dialog. It
        // opens only when the census found no volume that is not ours; on any other disk the key
        // does nothing but say why. Checked HERE and again at the go below, because a guard asked
        // once at the affordance is a UI filter and not a guard.
        (State::Census, b'd') => {
            if WHOLE_OK.load(Ordering::Relaxed) {
                serial_println!(
                    "[wc-x] instgui whole-disk demo requested — the census found no foreign volume on this disk"
                );
                *STATE.lock() = State::Warn;
            } else {
                serial_println!(
                    ":: INSTGUI: whole-disk target REFUSED at the affordance — this disk carries volumes that are not ours (R25) -> guard OK ::"
                );
            }
            repaint();
        }
        (State::Warn, b'\x1b') => {
            // INSTALLVERB: back to the CENSUS the operator came from, with the commitment intact —
            // the disk has not changed and re-reading it would only cost them their place.
            *STATE.lock() = State::Census;
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
            // INSTALLVERB: **THE GUARD, ASKED AGAIN AT THE GO.** `d` already refused to open this
            // screen for a disk with volumes that are not ours; this is the same question at the
            // instant the engine would run, because the affordance and the act are different
            // moments and only the second one writes. There is no path from this dialog to the
            // whole-disk engine that does not pass through both.
            if !WHOLE_OK.load(Ordering::Relaxed) {
                serial_println!(
                    ":: INSTGUI: whole-disk go REFUSED — the census found volumes that are not ours on this disk (R25); nothing written -> guard OK ::"
                );
                *STATE.lock() = State::Census;
                repaint();
                return true;
            }
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
