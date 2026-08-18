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

//! The video subsystem.
//!
//! - [`framebuffer::FrameBuffer`] — the one pixel-format-aware drawing surface; every pixel on
//!   screen goes through it.
//! - [`screen::Screen`] — a double-buffered surface with damage tracking, built on two
//!   `FrameBuffer`s (the framebuffer + a cached-RAM back buffer). The steady-state GUI renderer.
//! - [`fbcon`] — the boot/panic text console (a log sink for hardware with no serial port),
//!   drawn straight to its own `FrameBuffer` handle (it runs pre-heap, so no back buffer).
//! - [`wm`] — the window table and compositor: user surfaces composited onto the panel with
//!   kernel-drawn chrome. `screen::present_surface` is a compat shim over its window 0.
//! - [`WRITER`] — the framebuffer the GUI's `Screen` flushes to and that fbcon mirrors onto.
//!
//! `WRITER` and `fbcon` are handles to the *same* physical framebuffer; they are used at
//! different times (fbcon during boot, the GUI after a successful boot repaints over it) and
//! each is serialised by its own lock.

pub mod fbcon;
pub mod framebuffer;
// CRISPY-PI: the Crispy theme lifted from `kits/crispy/theme.json` (@ us-crispy 08b42ede) into a
// (theme is declared below with its CRISPY-PI authorship note — a duplicate declaration here was
// the MERGE-SWEEP defect: both branches added `pub mod theme;` independently and git auto-merged
// them into two declarations WITHOUT raising a conflict. One survives, with the fuller provenance.)
pub mod screen;
// WC-A: the window table + compositor. Owns which pixels of which surface reach the panel; the
// aarch64 window syscalls (WC-B) are thin fail-closed wrappers over its API.
pub mod wm;
// CURSOR-1: the system cursor sprite, drawn into the FRONT framebuffer as the last painter of a
// pass (save-under, not a recomposite) so it sits on top of both the console and every window.
// Pointer position + auto-hide state stay in `pal::cursor`; this module only paints.
pub mod cursor;
// VWIT: headless regression witness for the damage-tracked `Screen` present path (arch-neutral;
// runs only when the `tste` self-test is invoked). See `docs/dev/OS/08_VIDEO/engine.md` §7.
pub mod witness;
// WC-F: the scan-out ground-truth probe — the live FrameBuffer checked against the firmware's own
// geometry, plus a twin-pattern render (compositor addressing vs firmware-pitch addressing) that
// discriminates a blit-path defect from a scan-out one. `witness`-gated AND aarch64-only, so the
// flashable Pi media and every x86 artifact are byte-identical with it absent.
#[cfg(all(target_arch = "aarch64", feature = "witness", feature = "baremetal"))]
pub mod wcf;
// WC-G: the window PRESENT path instrumented while it runs — four checksums of one surface around
// one blit, a scan-out read-back, and the blit's duration, which together separate a source race
// from a coherency fault from a blit-path defect from an unbuffered-copy timing defect. Also carries
// WC-H's `[wc-h]` staged-composite lines and WC-K/WC-L's `[wc-k]` erase lines. `witness`-gated and
// nothing else — unlike `wcf`, which stays aarch64-only because it talks to the VideoCore mailbox —
// so every artifact stays byte-identical with the knob off, on both arches.
#[cfg(feature = "witness")]
pub mod wcg;
// VPERF instrumentation (counters, fbmem readout, PCI display probe, scripted scroll scenario).
// x86-only AND knob-gated: with the feature off — or on aarch64 regardless — nothing here
// compiles, so those artifacts stay byte-identical.
#[cfg(all(target_arch = "x86_64", feature = "videobench"))]
pub mod vperf;
// WC-X86: the compositor's ACTIVATION on the x86 panel path — the seam at the end of the Kepler
// takeover, the CONSOLE WINDOW (fbcon's glyphs routed into a compositor window), and the deferred
// launch of the DESKTOP APP — the ring-3 `STAT.ELF` that replaced the kernel-drawn demo window in
// the kernel-apps eviction. Changes nothing in `wm`
// or `cursor` (both are already arch-neutral); it only gives them a window to composite. x86-only
// AND knob-gated (`UNAOS_WC=1`): aarch64 stays BYTE-IDENTICAL (no aarch64 file is touched), and a
// knob-off x86 build carries no feature code and no behaviour change — but it is NOT byte-identical
// since the kernel-apps eviction, differing by relocated rodata immediates in
// `arch/x86_64/syscall.rs` plus symbol-string-table length. Measured, not assumed; see the `wc` knob
// comment in `arroyo` for the hashes and the byte counts.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub mod wcx;
// DOCK: the bottom strip — one tile per live window, INCLUDING the ones that are not on the panel,
// and a press that raises and un-hides the window it names. Peter's ruling, white board Q10
// (2026-08-09): "mac has had the dock forever so we should have a doc and all macos like
// experience". A window switcher, not an app launcher — there is no app grid here and no second
// launch path.
//
// PI-DESK — the gate is no longer `wcx`'s alone. It is **the x86 panel path (`UNAOS_WC=1`) OR the Pi
// panel path (`UNAOS_PIDESK=1`)**, and the two halves are independent: a knob-off aarch64 build
// compiles nothing here and its `kernel8.img` stays BYTE-IDENTICAL, and x86 keeps exactly the `wc`
// term it always had. Nothing in this module became arch-neutral by being compiled on a second arch
// — it already was, bar one reach into `arch::x86_64::syscall` for focus, which is seamed at its own
// call site the way `wm` seams `wcx`. The same gate is on `strip`, `menubar` and `crystal` below.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "pidesk")))]
pub mod dock;
// STRIPFACTOR: the furniture-strip PRIMITIVE — edge-anchored geometry with floors, the staged
// row-run painter, the vacated-pixel erase, the damage slot, the cost ledger, and the TENANT
// REGISTRY `wm::erase_clip` walks for occlusion citizenship. Peter's direction 2026-08-11: UnaOS is
// a spatial game-engine OS and the desktop is one shell on it — "we will not always have a menu
// bar" — so the kernel's contribution is the mechanism, not any particular strip. `dock` is tenant
// #1 and `menubar` is tenant #2. Same gate as `dock` (PI-DESK: x86+`wc` OR aarch64+`pidesk`).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "pidesk")))]
pub mod strip;
// MENUBAR: the top strip — tenant #2 of `strip`, and DEFAULT OFF. Inert chrome plus the focused
// window's caption and a UTC clock; a press falls through, because opening menus belongs to the
// renderer-agnostic menu PROTOCOL whose design ledger is at the foot of that file. Deleting it costs
// one registry entry, one line of `strip::compose_all`, and this declaration. Same gate as `dock`
// (PI-DESK: x86+`wc` OR aarch64+`pidesk`) — and DEFAULT OFF stays default off on the Pi too: Peter's
// "we will not always have a menu bar" is a runtime statement, and compiling the tenant does not
// enable it.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "pidesk")))]
pub mod menubar;
// CRYSTAL: the SHARD menu — UnaOS's first LIVE menu, hung off the brand crystal in the menu bar. A
// kernel-owned SYSTEM menu (About This Shard · Sleep · Restart · Shut Down), not the renderer-agnostic
// app menu PROTOCOL (whose design ledger lives at the foot of `menubar.rs`). The dropdown is a
// transient surface composited through `strip::paint` at the tail beside the dock and bar; the click
// arm lives in `arch/x86_64/syscall.rs::wc_click_route_at` — and, since PI-DESK, in
// `arch/aarch64/syscall.rs::wc_click_route`, both through the ONE shared router
// [`strip::press_route`] rather than two copies of the ordering rule. Same gate as
// `dock`/`strip`/`menubar` (x86+`wc` OR aarch64+`pidesk`).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "pidesk")))]
pub mod crystal;
// CRISPY-PI theme const table — carried verbatim from hw-pi4 (single-author law: edits flow
// through the pi4 seat from the taste-gate). Declared for INSTGUI's use; full chrome wiring
// (wm/fbcon reading these instead of their own constants) remains the later lockstep arc.
pub mod theme;
// PAPER — the Crispy kit's `content_surface.Paper` material, the multi-octave noise generator
// `theme.rs` and engine.md §9 deliberately left unlifted, ported to integer Q16 (the kernel has no
// float and no libm). A CONTENT surface only — Peter's white-board ruling is that the paper is NOT
// the desktop background. Arch-neutral and unconditional: one 88 KiB `.bss` tile generated once on
// first use, blitted thereafter.
pub mod paper;
// CERAMIC — brushed anodised aluminium for the window CHROME, the counterpart to paper's content
// surface (Peter's directive 2026-08-09: "texture to the window borders scrollbars buttons etc and
// have paper for text surfaces"). DERIVED, not lifted — the kit carries no ceramic block, and the
// module header says so rather than dressing the parameters up as a citation. A per-ROW material
// (the brush runs along x), so it is 512 bytes of `.bss` and costs no per-pixel work at all: it
// reuses paper's Q16 primitives and modulates the existing theme roles, never replaces them.
pub mod ceramic;
// KNURL — the diamond crosshatch milled into the three title-bar control discs (Peter's ruling
// 2026-08-09: "same color as mac but knurled if possible to add more texture"). DERIVED, not
// lifted, on ceramic's terms. Two line families at exactly +/-45 degrees — the level sets of
// `x + y` and `x - y`, so the angles are integer expressions and cost no rotation arithmetic —
// summed through paper's shared Q16 sine. Periodic rather than noisy, because a knurl is MILLED:
// the tile is the field's fundamental domain, 64 bytes of `.bss`, one cache line.
pub mod knurl;
// INSTGUI — the first graphical installer: a kernel-owned, CRISPY-themed window over the
// installer engine (choose disk → erase warning → engine run → verdict). x86-only, and gated
// on BOTH `wc` (it is a compositor client) and `instgui` (`UNAOS_INSTGUI=1`).
#[cfg(all(target_arch = "x86_64", feature = "wc", feature = "instgui"))]
pub mod instgui;

pub use framebuffer::FrameBuffer;
pub use screen::Screen;

use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use spin::Mutex;
use unaos_boot_info::FrameBufferInfo;

/// The primary display surface. The GUI renderer (`pal::TargetPal` → `console`) draws here.
/// Initialised once in `kernel_main` from `BootInfo` (UEFI GOP, or the Pi mailbox framebuffer).
pub static WRITER: Mutex<FrameBuffer> = Mutex::new(FrameBuffer::new());

/// The panel background the GUI paints over — Can-Am dark grey, `#1E1E1E`.
pub const PANEL_BG: u32 = 0x001E_1E1E;

/// Paint the panel background once at boot, and witness the framebuffer geometry the bootloader
/// handed us.
///
/// Called from `kernel_main` immediately after [`WRITER`] is attached, on both arches. The serial
/// lines are the FB-geometry witness a metal capture reads before any GUI exists — they are the
/// first independent statement of what the firmware actually gave us (size, stride, pixel format),
/// so a wrong-geometry boot is nameable from the log alone. The paint goes through a plain
/// [`FrameBuffer`] handle rather than `WRITER` so format and bounds are handled in exactly one
/// place and no lock is held across the fill.
pub fn init_panel(base: usize, len: usize, info: FrameBufferInfo) {
    serial_println!(":: FB Init ::");
    serial_println!(":: FB Size: {}x{} (stride {}) ::", info.width, info.height, info.stride);
    serial_println!(":: FB Format: {:?} ::", info.pixel_format);

    let mut surface = FrameBuffer::new();
    surface.init(base, len, info);
    surface.fill_screen(PANEL_BG);

    serial_println!(":: Framebuffer painted #1E1E1E ::");

    // WEDGE-12 (F6) — size the compositor's staging buffer HERE, where the panel's geometry has just
    // become known, IRQs are live and no composite pass exists yet. `wm`'s staged presents run inside
    // `SYS_WIN_PRESENT`'s IRQ mask, so a buffer that grew on the pass would be a masked acquisition of
    // the global heap `Mutex` — the F1-F5 family defect with the widest-shared lock in the kernel.
    // Growing it once, from here, is what lets the masked paths be allocation-free. WEDGE-12 M2:
    // this now sizes ONE ENTRY PER LIVE CORE, not just the BSP's — which is sound at this call
    // site precisely because SMP bring-up has already published its core set several hundred lines
    // above (see `wm::live_core_count` for the ordering argument and the count's source,
    // `wm::stage_secondary_target` for the per-arch heap arithmetic, and `[wedge12]` for the
    // per-entry census).
    wm::reserve_stage(&info);
}

// ---------------------------------------------------------------------------------------------
// EDID-CARRY — the panel's own descriptor, from firmware to the display lane.
//
// The UEFI bootloader has always read the panel's EDID (EFI_EDID_ACTIVE_PROTOCOL, falling back to
// EFI_EDID_DISCOVERED_PROTOCOL) and then thrown the bytes away, keeping only the native width and
// height for the mode-selection branch. Width and height cannot program a display pipe: the pixel
// clock and the h/v blanking and sync numbers live in the block and were discarded with it. The
// bootloader now carries the 128-byte base block through `BootInfo::edid_block`; this module is
// where it lands, is checked, and is published.
//
// The block is stored raw and validated on the way out, so the wire and the consumers disagree
// only in the direction that is safe: [`init_edid`]'s witness line reports exactly what arrived
// (including a bad header or a bad checksum), while [`edid_block`] — the accessor a mode-set is
// meant to build on — returns nothing unless the block passed both checks.
// ---------------------------------------------------------------------------------------------

/// EDID 1.x fixed header pattern — the first eight bytes of every base block.
const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// The raw base block as it arrived from the bootloader. `None` until [`init_edid`] runs, and after
/// it whenever the firmware exposed no EDID at all.
static EDID_BLOCK: Mutex<Option<[u8; 128]>> = Mutex::new(None);

/// Set only when the stored block's header magic AND 128-byte checksum both passed.
static EDID_OK: AtomicBool = AtomicBool::new(false);

/// The firmware-reported size of the whole EDID in bytes (0 = none). Greater than 128 means
/// extension blocks exist and were NOT carried — only the base block is.
static EDID_TOTAL_LEN: AtomicU16 = AtomicU16::new(0);

/// The panel's EDID base block, or `None` if this boot has no trustworthy one.
///
/// **This is the supported reader.** `BootInfo` is consumed by `arch::memory::init` early in
/// `kernel_main`, so after that point nothing but this static can answer the question.
///
/// Fail-closed: the 128 bytes come back only when the EDID header magic matched and the block's
/// bytes summed to 0 mod 256. `None` therefore means one of — no EDID protocol on the firmware's
/// GOP handle, a non-UEFI boot (aarch64 bare-metal), a bad header, or a bad checksum. Which one it
/// was is on the serial wire, in [`init_edid`]'s `:: video: edid ... ::` line; use
/// [`edid_block_raw`] if the bytes themselves are what you need to look at.
///
/// **For the iGPU display lane (Flight 1c).** The bytes are the EDID BASE block, verbatim. The
/// offsets a mode-set wants, all inside the first Detailed Timing Descriptor at `[54..72]`:
/// `[54..56]` pixel clock in 10 kHz units, little-endian (0 there means the descriptor is a monitor
/// descriptor, not a timing); `[56]`/`[58]>>4` horizontal active; `[57]`/`[58]&0xF` horizontal
/// blanking; `[59]`/`[61]>>4` vertical active; `[60]`/`[61]&0xF` vertical blanking; `[62..66]` the
/// four sync numbers (h offset, h width, then v offset and v width as the two nibbles of `[64]`,
/// with every high bit pair packed into `[65]`). Byte `[126]` is the extension-block count — see
/// [`edid_total_len`] for whether any existed.
///
/// This block is the FIRMWARE's view of the panel. A rung-3 I2C-over-AUX read talks to the panel
/// directly; the two agreeing is a result, and the two disagreeing is a finding.
pub fn edid_block() -> Option<[u8; 128]> {
    if !EDID_OK.load(Ordering::Relaxed) {
        return None;
    }
    *EDID_BLOCK.lock()
}

/// The EDID base block exactly as it was carried, **without** the header/checksum gate that
/// [`edid_block`] applies. For diagnosis only: a block that reaches here but not `edid_block` is
/// one whose own integrity checks failed, and nothing may be programmed into a display pipe from it.
pub fn edid_block_raw() -> Option<[u8; 128]> {
    *EDID_BLOCK.lock()
}

/// The firmware-reported size of the whole EDID in bytes; 0 when none was read. A value above 128
/// means the panel published extension blocks (CEA-861 timings and the like) which this path does
/// NOT carry — only the base block crosses `BootInfo`.
pub fn edid_total_len() -> u16 {
    EDID_TOTAL_LEN.load(Ordering::Relaxed)
}

/// Publish the panel's EDID base block from `BootInfo`, and witness it.
///
/// Called from `kernel_main` alongside the framebuffer handoff, before `arch::memory::init`
/// consumes `BootInfo`, on both arches and in every build. The one serial line it prints is the
/// only independent statement of what the firmware knew about the panel, and it is written so that
/// it can say NO: `present=0` when nothing was carried, `hdr=BAD` when the 128 bytes are not an
/// EDID base block, `sum=BAD` when they are one but corrupt. On QEMU, where no EDID protocol is
/// published, `present=0` is the expected reading and is not a fault.
///
/// `valid` is the bootloader's "I copied 128 bytes from firmware" flag — it says nothing about the
/// content, which is what this function is here to check.
pub fn init_edid(block: &[u8; 128], valid: bool, total_len: u16) {
    if !valid {
        serial_println!(":: video: edid present=0 hdr=- sum=- native=- len=0 ::");
        return;
    }

    let hdr_ok = block[0..8] == EDID_HEADER;
    // EDID 1.x §3.1: the 128 bytes of the base block sum to 0 mod 256, checksum byte included.
    let sum_ok = block.iter().fold(0u8, |acc, b| acc.wrapping_add(*b)) == 0;

    // First Detailed Timing Descriptor, offset 54. A zero pixel clock marks it a monitor descriptor
    // rather than a timing, and then there is no native mode to report.
    let d = &block[54..72];
    let pclk_khz = ((d[0] as u32) | ((d[1] as u32) << 8)) * 10;
    let (nat_w, nat_h) = if pclk_khz == 0 {
        (0u32, 0u32)
    } else {
        (
            (d[2] as u32) | (((d[4] as u32) >> 4) << 8),
            (d[5] as u32) | (((d[7] as u32) >> 4) << 8),
        )
    };
    let ext = block[126];

    *EDID_BLOCK.lock() = Some(*block);
    EDID_OK.store(hdr_ok && sum_ok, Ordering::Relaxed);
    EDID_TOTAL_LEN.store(total_len, Ordering::Relaxed);

    let yn = |b: bool| if b { "OK" } else { "BAD" };
    serial_println!(
        ":: video: edid present=1 hdr={} sum={} native={}x{} pclk_khz={} ext={} len={} ::",
        yn(hdr_ok),
        yn(sum_ok),
        nat_w,
        nat_h,
        pclk_khz,
        ext,
        total_len
    );
}

// ── CONSWIN-PI / MENUBAR-PI ─────────────────────────────────────────────────────────────────────
// The Pi's DESKTOP-READY seam — the aarch64 counterpart of `wcx`'s activation, and the module that
// finally claims `menubar`'s tenancy on this arch. Declared at the FILE TAIL, not beside its sibling
// tenants above, for the reason this track has proved twice over: a `mod` line inserted higher up
// renumbers every panic `Location` recorded below it in this file, and those records live in the
// loadable image whose knob-off byte-identity is the Pi track's standing proof. Nothing is below
// this, so nothing moves. `pidesk` off is a compile-time absence and the image never contains it.
#[cfg(all(target_arch = "aarch64", feature = "pidesk"))]
pub mod pidesk;

// ── PULSEWIN ────────────────────────────────────────────────────────────────────────────────────
// The core-load instrument in a compositor WINDOW, whose menu switches between the Pi's LED lamp
// face and the x86 app's ten-segment face. Gated like the furniture family — `wc` on x86, `pidesk`
// on the Pi — and deliberately NOT on an arch: it is experience-layer code with no hardware in it,
// so it builds once and runs on every chip. Appended below `pidesk` for `pidesk`'s own reason, five
// lines up: nothing is below this either, so nothing moves.
#[cfg(any(
    all(target_arch = "x86_64", feature = "wc"),
    all(target_arch = "aarch64", feature = "pidesk")
))]
pub mod pulsewin;
// ── QUARRY ──────────────────────────────────────────────────────────────────────────────────────
// UnaOS's FILE MANAGER (Peter, 2026-08-17: "Tree on left and start with detailed list view on
// right"). A kernel-owned compositor window over the VFS mount table — tree of the mounted volumes on
// the left, name/size/modified list on the right, and the first scrolling anything in this tree has
// had. Same gate as the furniture family above (x86 `wc` OR aarch64 `pidesk`) so BOTH arches
// type-check it, with the implementation armed separately by `UNAOS_QUARRY=1`; see `quarry.rs` for
// why the knob lives inside the module rather than on this line, and
// `docs/dev/OS/05_USER_EXPERIENCE/quarry.md` for the design of record — including why it is a kernel
// window today and exactly what the ring-3 port is waiting on.
//
// Declared BELOW `pidesk` for `pidesk`'s own reason, restated because it is the trap: a `mod` line
// inserted higher up renumbers every panic `Location` recorded below it in this file. Nothing is
// below this, so nothing moves, and a knob-off `kernel8.img` is byte-identical.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "pidesk")))]
pub mod quarry;
