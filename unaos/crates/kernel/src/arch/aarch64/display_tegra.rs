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

//! JD1 — Jetson Orin (Tegra234) display: inherit the firmware's live scanout framebuffer.
//!
//! The panel is dark under UnaOS not because the display is off — it is very much on: NVIDIA's UEFI
//! ran its display pipeline, drove the monitor to 1920×1200, and the DCE (the RISC-V ucontroller that
//! owns nvdisplay) is *still scanning out* a framebuffer from a DRAM carveout when it hands off to us.
//! The problem is only that the UEFI **GOP is `BltOnly`** (JM7): it exposes no CPU-linear framebuffer,
//! so `BootInfo::framebuffer_addr == 0` and fbcon stays inert. `Blt()` is a boot-service and is gone
//! after ExitBootServices — the GOP route to pixels is a dead end on this firmware.
//!
//! But the firmware hands the framebuffer off a **different** way, on purpose. Its display-handoff
//! method defaults to *SIMPLEFB* (edk2-nvidia `DisplayDeviceTreeHelperLib`): it writes a
//! `compatible = "simple-framebuffer"` node into the device tree it exposes (geometry on the node, the
//! **physical** base/size in the node's `memory-region` reserved-memory `reg`, with `iommu-addresses`
//! declaring the display IOMMU *identity* map). So the live scanout physical address is sitting in the
//! DTB the bootloader already captured into `BootInfo::dtb_addr`.
//!
//! JD1 therefore **inherits, it does not re-init** (the JB6→JB9 XUSB lesson applied to display): it
//! reads that base+geometry+format from the DTB — a pure RAM walk, no display MMIO, no SMMU
//! translation, no double-buffer active/assembly hazard, and crucially no touch of a possibly-
//! powergated block — maps the carveout Normal-WB, and hands `{addr, len, geometry}` to the existing
//! `FrameBuffer`/`fbcon` machinery. The DCE keeps scanning out that DRAM, so CPU writes into it appear
//! on the panel (cleaned to the Point of Coherency by fbcon's `flush_*`, the Pi-HVS recipe — the DCE
//! does not snoop the CPU cache).
//!
//! A read-only nvdisplay *register* survey (`JD1_DC_PROBE`) is kept as a **default-off** fallback for
//! the bench, for the case where the firmware published no simple-framebuffer node into the DTB we
//! received. See its doc for why it is off by default.

use crate::video::FrameBuffer;
use unaos_boot_info::{FrameBufferInfo, PixelFormat};

use super::fdt_tegra;

/// Default-OFF fallback: a read-only sweep of the nvdisplay window registers to read the live scanout
/// base the DCE programmed. OFF because (a) the DTB `simple-framebuffer` handoff is the safe primary
/// path, and (b) if the display block were powergated the first register read would be EL3-fatal (the
/// JX1 lesson) — the panel is lit so it is *believed* powered, but the DTB path proves pixels without
/// betting on that. Flip to `true` at the bench, panel confirmed lit, only if the DTB handoff is
/// absent (`JD1 — no simple-framebuffer node in DTB`).
pub const JD1_DC_PROBE: bool = false;

/// Seconds to hold the JD1 test pattern on the panel before fbcon paints the boot log over it — long
/// enough to read the colour bars by eye and confirm the pixel-format decode at the bench. 0 = no
/// hold (the bars just flash). A `CNTPCT` busy-wait (see `busy_wait_secs`); set back to 0 once the
/// format is confirmed so the boot is not slowed.
pub const JD1_TEST_PATTERN_HOLD_SECS: u64 = 3;

/// The inherited scanout: physical base, byte length of the visible image, and pixel layout —
/// exactly what `fbcon::init` / `video::WRITER` need. `len` bounds every draw to the visible frame.
pub struct ScanoutFb {
    pub base: u64,
    pub len: usize,
    pub info: FrameBufferInfo,
}

/// Map a simplefb `format` string to UnaOS's `FrameBuffer` layout: `(pixel_format, bytes_per_pixel)`.
///
/// The simplefb strings name the pixel value's channels **MSB→LSB**; it is stored little-endian, so
/// the in-memory byte order is the reverse — which is what `FrameBuffer::put_pixel` writes. Reverse
/// the named order to get memory order: `"x8r8g8b8"` → `[B,G,R,X]` = UnaOS `Bgr`; `"x8b8g8r8"` →
/// `[R,G,B,X]` = `Rgb`. The rule is uniform across bpp — the *last* named colour is the first byte.
fn parse_format(fmt: &[u8]) -> Option<(PixelFormat, usize)> {
    match fmt {
        b"x8r8g8b8" | b"a8r8g8b8" => Some((PixelFormat::Bgr, 4)),
        b"x8b8g8r8" | b"a8b8g8r8" => Some((PixelFormat::Rgb, 4)),
        // 24bpp packed (rare for a scanout): same MSB→LSB naming — `r8g8b8` = memory `[B,G,R]` = Bgr,
        // `b8g8r8` = memory `[R,G,B]` = Rgb (DRM_FORMAT_RGB888/BGR888).
        b"r8g8b8" => Some((PixelFormat::Bgr, 3)),
        b"b8g8r8" => Some((PixelFormat::Rgb, 3)),
        _ => None,
    }
}

/// JD1 survey: dump the DTB display handoff, resolve the firmware's live scanout framebuffer, print
/// the `:: tegra: JD1 — scanout: … ::` verdict, and return it. Read-only; no display MMIO (unless the
/// default-off `JD1_DC_PROBE` fallback is enabled). `None` = the firmware published no usable
/// simple-framebuffer handoff into our DTB (or its geometry failed sanity) — the caller boots headless
/// (fbcon stays inert), exactly as today.
pub fn jd1_survey(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<ScanoutFb> {
    // 1. Human-readable dump (always) — shows exactly what the firmware published, so a bench boot is
    //    self-diagnosing even when the strict resolver rejects what it found.
    fdt_tegra::jd1_dump(dtb_addr, dtb_size, ram_gib_mask);

    // 2. Optional read-only DC register cross-check (default off; the DTB handoff is primary + safe).
    if JD1_DC_PROBE {
        jd1_dc_survey(dtb_addr, dtb_size, ram_gib_mask);
    }

    // 3. Resolve the simple-framebuffer handoff (the safe primary path). The bare `?` this used to be
    //    was the boot-3 class exactly: the resolver's `None` abandoned the whole JD1 inheritance and
    //    the caller's `if let Some(fb)` has no `else`, so a headless boot printed NOTHING here — the
    //    absence of `panel LIVE` was ambiguous between "no handoff published" and "never reached".
    //    The resolver now names its own rung (`JD1-SFB STOP`); this line is the verdict beside it.
    let Some(sfb) = fdt_tegra::nvdisplay_simplefb(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!(
            ":: tegra: JD1 — no usable simple-framebuffer handoff resolved from the DTB (see the JD1-SFB line above for the rung); HEADLESS — fbcon stays inert, no blit ::"
        );
        return None;
    };

    // 4. Decode format + geometry into the FrameBuffer layout.
    let Some((pixel_format, bpp)) = parse_format(&sfb.format[..sfb.format_len]) else {
        serial_println!(
            ":: tegra: JD1 — scanout fmt '{}' unsupported (need 32bpp x8r8g8b8/x8b8g8r8); headless ::",
            core::str::from_utf8(&sfb.format[..sfb.format_len]).unwrap_or("?"),
        );
        return None;
    };
    let width = sfb.width as usize;
    let height = sfb.height as usize;
    // The simplefb `stride` is in BYTES; `FrameBufferInfo::stride` is in PIXELS. Fall back to a tight
    // stride if the node omitted it.
    let stride_bytes = if sfb.stride != 0 { sfb.stride as usize } else { width * bpp };
    let stride_px = stride_bytes / bpp;
    let len = stride_bytes * height;

    // A real scanout: sensible dimensions, a DRAM base (>= 0x8000_0000, GiB 2+), a stride that at
    // least spans a row, and a carveout large enough for the visible image.
    let sane = (16..=8192).contains(&width)
        && (16..=8192).contains(&height)
        && stride_px >= width
        && sfb.base >= 0x8000_0000
        && (sfb.size as usize) >= len
        && len != 0;

    let info = FrameBufferInfo { width, height, stride: stride_px, bytes_per_pixel: bpp, pixel_format };
    serial_println!(
        ":: tegra: JD1 — scanout: base={:#x} size={:#x} {}x{} stride={}B fmt={} ({:?}) src={} sane={} ::",
        sfb.base,
        sfb.size,
        width,
        height,
        stride_bytes,
        core::str::from_utf8(&sfb.format[..sfb.format_len]).unwrap_or("?"),
        pixel_format,
        if sfb.via_memregion { "memory-region" } else { "node-reg" },
        sane,
    );
    if !sane {
        serial_println!(":: tegra: JD1 — scanout geometry/base failed sanity; headless (no blit) ::");
        return None;
    } #[cfg(feature = "jd1dc")] JD1DC_SCANOUT.store(sfb.base, core::sync::atomic::Ordering::Relaxed); // JD1-DC — latch the base JD1 has just ACCEPTED (post-sanity, so it is the base fbcon/WRITER are about to be seeded with), so the BPMP-guarded nvdisplay probe can compare a window's START_ADDR against the inherited scanout without re-walking the DTB and without hardcoding 0x279e00000. APPENDED to an existing line: knob-off this statement is cfg-erased and not one `Location` in this file moves.
    Some(ScanoutFb { base: sfb.base, len, info })
}

/// Busy-wait ~`secs` seconds on the always-on system counter (`CNTPCT_EL0`) — so the JD1 test
/// pattern lingers on the panel before fbcon paints over it. A pure system-register spin: no GIC/
/// timer (this runs pre-JM4 at EL2) and no MMIO; `CNTFRQ_EL0` gives the counter rate. Bounded, and
/// short next to the seconds the boot already spends in USB enumeration.
fn busy_wait_secs(secs: u64) {
    if secs == 0 {
        return;
    }
    let (freq, start): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) start, options(nomem, nostack, preserves_flags));
    }
    if freq == 0 {
        return;
    }
    let target = start.wrapping_add(secs.saturating_mul(freq));
    loop {
        let now: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack, preserves_flags));
        }
        if now >= target {
            break;
        }
        core::hint::spin_loop();
    }
}

/// Paint a distinctive test pattern into the inherited scanout so a bench boot can confirm — before
/// fbcon takes the screen for the boot log — that the inherited `{base, stride, format}` actually
/// land on the panel: a wrong stride shears the bars, a wrong format swaps blue↔red, a wrong base
/// shows nothing. Eight colour bars + a bright frame. Flushed to RAM for the (non-coherent) DCE, then
/// held on screen for `JD1_TEST_PATTERN_HOLD_SECS` so it is legible before fbcon paints over it.
///
/// SAFETY: `base`/`len` come from `jd1_survey`, which already bounded the base to DRAM and `len` to
/// the carveout size; `FrameBuffer::put_pixel`/`fill_rect` are themselves bounds-checked against
/// `len`, so a mis-sized geometry cannot write past the mapped framebuffer. The write target does not
/// overlap the running kernel by construction: the scanout is a firmware *reserved* carveout
/// (`CARVEOUT_DISP_EARLY_BOOT_FB`), disjoint from the LOADER memory the kernel image + boot stack
/// occupy — `map_fb_region`/`ram_gib_mask` are built from Usable|Bootloader regions, never Reserved.
pub fn jd1_test_pattern(fb: &ScanoutFb) {
    let mut surf = FrameBuffer::new();
    surf.init(fb.base as usize, fb.len, fb.info);
    let w = fb.info.width;
    let h = fb.info.height;
    // Colours are written as 0x00RRGGBB; `put_pixel` reorders per the pixel format, so a *correct*
    // format shows them in the named order and a wrong one swaps red/blue — the point of the bars.
    let bars = [
        0x0000_0000u32, // black
        0x0000_00FF,    // blue
        0x0000_FF00,    // green
        0x0000_FFFF,    // cyan
        0x00FF_0000,    // red
        0x00FF_00FF,    // magenta
        0x00FF_FF00,    // yellow
        0x00FF_FFFF,    // white
    ];
    let bw = (w / bars.len()).max(1);
    for (i, &c) in bars.iter().enumerate() {
        surf.fill_rect(i * bw, 0, bw, h, c);
    }
    // A bright 4-pixel frame around the visible area — a wrong stride/height makes it wrap or shear.
    let edge = 0x00FF_FFFFu32;
    surf.fill_rect(0, 0, w, 4, edge);
    surf.fill_rect(0, h.saturating_sub(4), w, 4, edge);
    surf.fill_rect(0, 0, 4, h, edge);
    surf.fill_rect(w.saturating_sub(4), 0, 4, h, edge);
    surf.flush_all();
    // Hold the pattern on the panel so it is legible before fbcon paints the boot log over it.
    busy_wait_secs(JD1_TEST_PATTERN_HOLD_SECS);
}

/// Default-OFF read-only fallback (see [`JD1_DC_PROBE`]): sweep the nvdisplay window registers for
/// the live scanout base the DCE programmed, for when the firmware published no simple-framebuffer
/// handoff. Reads ONLY within the DTB-resolved display-block aperture (already mapped Device-nGnRE in
/// the GiB-0 device window). The read value is an SMMU IOVA (identity-mapped on this firmware) with
/// bit 39 a GPU sector-swizzle flag to mask.
///
/// nvdisplay per-window aperture, derived from Linux Tegra DRM `hub.c` — whose `of_match` ends at
/// `nvidia,tegra194-display`, so this layout is **T186/T194's and is NOT documented for T234**:
/// window `i` registers live at `head_base + 0x2800 + 0xC00*i`, with (byte offsets within the
/// aperture) `WIN_OPTIONS`(WIN_ENABLE bit30) `+0x600`, `WINDOWGROUP_SET_CONTROL`(OWNER low nibble)
/// `+0x608`, `COLOR_DEPTH` `+0x60c`, `SIZE`(output) `+0x614`, `CROPPED_SIZE`(source) `+0x618`,
/// `PLANAR_STORAGE`(stride/64) `+0x624`, `START_ADDR`(lo) `+0x700`, `SURFACE_KIND`(0=pitch) `+0x72c`,
/// `START_ADDR_HI` `+0x734` — all plain config registers (read-safe, not read-to-clear). The
/// per-head stride `0x10000` (4 heads) is T194-derived and **UNCONFIRMED — no nvdisplay register has
/// ever been read on this board**; see the JD1-DC-MODEL block at this file's tail, which tests it.
fn jd1_dc_survey(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    let Some((base, size)) = fdt_tegra::nvdisplay_base(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!(":: tegra: JD1-DC — no display@ node in DTB; DC register survey skipped ::");
        return;
    };
    serial_println!(
        ":: tegra: JD1-DC — display block {:#x} size={:#x}; read-only window sweep (announce before touch) ::",
        base,
        size,
    );
    let rd = |off: u64| unsafe { core::ptr::read_volatile((base + off) as *const u32) };
    for &head_off in &[0u64, 0x10000, 0x20000, 0x30000] {
        // Keep every read strictly inside the block's own decoded aperture (each head is 0x10000).
        if head_off + 0x2800 + 6 * 0xC00 > size {
            break;
        }
        for win in 0u64..6 {
            let ap = head_off + 0x2800 + 0xC00 * win;
            let opts = rd(ap + 0x600);
            let lo = rd(ap + 0x700);
            let hi = rd(ap + 0x734);
            if opts == 0 && lo == 0 && hi == 0 {
                continue; // empty window — skip to keep the log readable
            }
            let base_iova = (((hi as u64) << 32) | lo as u64) & !(1u64 << 39);
            serial_println!(
                ":: tegra: JD1-DC — head+{:#x} win{}: OPTIONS={:#x}(en={}) OWNER={:#x} START={:#x} SIZE={:#x} CROP={:#x} STRIDE/64={:#x} COLOR={:#x} KIND={:#x} ::",
                head_off,
                win,
                opts,
                (opts >> 30) & 1,
                rd(ap + 0x608) & 0xf,
                base_iova,
                rd(ap + 0x614),
                rd(ap + 0x618),
                rd(ap + 0x624),
                rd(ap + 0x60c),
                rd(ap + 0x72c),
            );
        }
    }
}

// =================================================================================================
// ORIN-WM1 — the Orin's FIRST real compositor window on the inherited scanout. `orindesk`, DEFAULT OFF.
// =================================================================================================
//
// WHAT THIS IS, AND WHAT IT DELIBERATELY IS NOT. This is ONE window: `reserve_stage`, one heap-backed
// ARGB surface, one `create_at` row, one `present`, one `composite`. It is NOT the desktop. It does not
// call `desktop_firmware::activate`, does not enable furniture, and touches no `dock`/`strip`/`menubar`/`crystal`
// seam — the Pi hit a 16 KiB kernel-stack overflow in the desktop-ARMING CASCADE twice, and that failure
// is not reproducible on the QEMU gate, so the cascade stays unarmed until the rung that owns it. One
// window is the whole rung: it settles, on metal, whether the compositor `wm` already links into the
// Orin kernel can also RUN there.
//
// WHY IT IS SMALL. `video/mod.rs` declares `pub mod wm;` UNCONDITIONALLY, so the ~20k-line compositor
// has been compiling into the jetson image all along; `main.rs`'s tegra path already calls
// `wm::retile_on_ready()` at the JD1/JD2 seam, so `wm` is linked AND reached on this board today. Every
// verb below — `reserve_stage`, `spawn_geometry`, `create_at`, `present_outcome`, `composite` — is an
// UNGATED `pub fn` in `wm.rs`. Nothing in `video/` is edited by this rung; the whole of it is this file
// plus one appended statement at the call site.
//
// THE ONE REAL GAP IT CLOSES. The Orin seeds `video::WRITER` DIRECTLY, in `tegra_early_stop`'s JD1
// block, and never calls `video::init_panel` — which is the only caller of `wm::reserve_stage` in the
// tree (`video/mod.rs`, the WEDGE-12 site). So the compositor's staging buffer was never allocated on
// this board and every composite would have fallen back to lazy growth. `reserve_stage` grows a `Vec`
// with `try_reserve`, so it MUST run after the heap exists — which on the tegra path is NOT
// `kernel_main`'s `memory::init` (the tegra path diverges before that line is ever reached) but
// `tegra_early_stop`'s own step 3c. Hence the call site: the statement appended to the
// `:: KERNEL HEAP ALLOCATED ::` line, the first instruction on this board at which all four of
// `reserve_stage`'s stated preconditions hold at once — panel geometry known (JD1 seeded `WRITER`
// ~170 lines above), heap up (the line it rides), IRQs live (JM4's `enable_irq`, ~15 lines above),
// and no composite pass in flight (single core, no scheduler yet).
//
// WHY NOT `jd2_console_pump`. That pump is a `sched::spawn`ed task and runs on a `TASK_STACK_SIZE`
// stack; `composite_inner` is the function whose aarch64 STACK EXHAUSTION is already on the ledger
// (occ62). `tegra_early_stop` runs on the boot stack. The rung that hands the panel to a desktop will
// have to answer the stack question; this one declines to ask it.
//
// WHAT LANDS ON GLASS. `wm` paints NO panel-wide backdrop on this build — the whole-panel `DESKTOP_BG`
// clear is `desktop_firmware`-gated on aarch64 (`wcf.rs` says so in its own NOCLEAR verdict) — so the composite
// writes the window's box and nothing else: the JD1 boot log keeps the rest of the panel. The blit path
// `flush_rect`s what it wrote, which is what makes the pixels visible to a DCE that scans the carveout
// from DRAM and does not snoop.
//
// DEFAULT OFF AND MEASURED. With `orindesk` unset every item below vanishes and the call site is one
// `#[cfg]`-erased statement, so the shipped jetson image is byte-identical to baseline (`esp-jetson`
// built either side of this change; sha256 of `kernel.elf` and of its `llvm-objcopy -O binary`
// flattening compared). The feature is standalone — like `smpmark`, it does NOT imply `tegra` — but
// every one of its sites is inside a `tegra`-gated module or a `tegra`-gated block, so
// `UNAOS_ORINDESK=1` alone compiles nothing: arm it as `UNAOS_TEGRA=1 UNAOS_ORINDESK=1 ./arroyo check`
// or `UNAOS_ORINDESK=1 ./arroyo esp-jetson` (which forces `tegra`). The ARMED polarity is type-checked
// by the `arm-tegra-orindesk` leg of `KERNEL_CFG_MATRIX`, never by the knob mapping.

/// ORIN-WM1 — the window's SURFACE STORE, held for the row's life.
///
/// `wm`'s table row holds a RAW POINTER into this buffer, so the allocation must outlive the call that
/// made it. Dropping it would leave the compositor blitting freed heap on every pass. The `pulsewin`
/// idiom (a module static, not a `mem::forget`) is used verbatim: moving the `Vec` in here moves the
/// three-word header, never the heap block the row points at.
#[cfg(feature = "orindesk")]
static ORINWM1_STORE: spin::Mutex<Option<alloc::vec::Vec<u8>>> = spin::Mutex::new(None);

/// ORIN-WM1 — the row's id, and the idempotence latch: a second call hands back the existing window
/// rather than minting a second one (and, more to the point, rather than leaking a second surface).
#[cfg(feature = "orindesk")]
static ORINWM1_WIN: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(crate::video::wm::WIN_NONE);

/// **ORIN-WM1 — mint and composite one real `wm` window on the Orin's inherited scanout.**
///
/// Returns the window id, or [`crate::video::wm::WIN_NONE`] on any decline. EVERY decline is named on
/// the wire and none of them is fatal to the caller: a headless Orin, or one whose heap cannot spare
/// the surface, boots exactly as it did before — this function is a leaf with no return value anyone
/// acts on.
///
/// Owner is [`crate::video::wm::KERNEL_OWNER_DESKTOP`] — kernel furniture. It is the value that module
/// reserves for "the next piece of kernel-owned desktop furniture", it is hittable (so a later pointer
/// rung can click it), and it is outside `focus_ring`/`close_owner`, so no ASID sweep can reap it.
///
/// The surface is `Bgr` + 4 bytes REGARDLESS of the panel's own format, and that is a statement about
/// `wm`, not about the Orin: `wm::draw_window` reads each source pixel as the little-endian word
/// `0x00RRGGBB`, and `FrameBuffer::put_pixel` on a `Bgr`/4 surface stores b,g,r at bytes 0,1,2 with
/// byte 3 left as the zero the allocation gave it. Same bytes, no conversion. The compositor converts
/// into whatever the PANEL is when it blits. Identical reasoning to `fbcon`'s console window and
/// `main.rs`'s `open_shell_window`.
#[cfg(feature = "orindesk")]
pub fn orin_wm1() -> crate::video::wm::WinId {
    use crate::video::wm;
    use core::sync::atomic::Ordering;

    let existing = ORINWM1_WIN.load(Ordering::Relaxed);
    if existing != wm::WIN_NONE {
        return existing;
    }

    // 1. THE PANEL. `WRITER` was seeded by the JD1 block above (`tegra_early_stop`), so on a boot that
    //    inherited a scanout this is ready and carries the firmware's real geometry. A headless boot
    //    (no `simple-framebuffer` handoff, or geometry that failed sanity) never seeded it — decline,
    //    named, and leave the boot untouched. Copy the info out and drop the lock immediately: nothing
    //    below may hold `WRITER` across a `wm` call, which is what keeps the WRITER/TABLE order acyclic.
    let info = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!("[orinwm1] DECLINE reason=no-panel (headless boot — no JD1 scanout)");
            return wm::WIN_NONE;
        }
        fb.info()
    };
    let (pw, ph) = (info.width, info.height);

    // 2. THE STAGING BUFFER — the gap this rung exists to close. `wm::reserve_stage` is called from
    //    `video::init_panel` on every other board and from NOWHERE on this one, because the Orin seeds
    //    `WRITER` by hand. Its `try_reserve` is why this whole function runs after the heap and not at
    //    the JD1 seed. Grow-only and idempotent; a short reserve is not a failure (the pass keeps
    //    trunk's lazy growth and declines rather than panicking), so it is REPORTED, not fatal.
    //    `stage_worst_case` caps at `MAX_STAGE_BYTES` = 4 MiB whatever the panel, against the 48 MiB
    //    aarch64 heap; `live_core_count` is 1 on tegra (not `baremetal`), so this sizes entry 0 alone.
    let staged = wm::reserve_stage(&info);

    // 3. THE CONTENT EXTENT — a third of the panel each way, floored so a small gate surface still
    //    yields a visible box. Deliberately NOT panel-sized: the point is a WINDOW, distinguishable at
    //    a glance from a full-screen paint, sitting over a boot log that stays readable around it.
    let cw = (pw / 3).max(160);
    let ch = (ph / 3).max(120);
    let stride = cw.saturating_mul(4);
    let Some(len) = ch.checked_mul(stride) else {
        serial_println!("[orinwm1] DECLINE reason=extent-overflow panel={}x{}", pw, ph);
        return wm::WIN_NONE;
    };
    if len == 0 {
        serial_println!("[orinwm1] DECLINE reason=empty-extent panel={}x{}", pw, ph);
        return wm::WIN_NONE;
    }

    // 4. THE SURFACE. `try_reserve_exact`, never a `vec![]`: at the bench panel (1920x1200) this asks
    //    for 640x400x4 = 1.0 MB of the 48 MiB aarch64 heap, and an exhausted heap must decline here
    //    rather than take the boot down through `handle_alloc_error`.
    let mut store: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if store.try_reserve_exact(len).is_err() {
        serial_println!("[orinwm1] DECLINE reason=alloc len={}", len);
        return wm::WIN_NONE;
    }
    store.resize(len, 0);
    let mut surf = FrameBuffer::new();
    // INVARIANT: `store` is at its final size and never grows, so this address stays valid for the
    // row's life (the `attach_shadow` / `Screen` back-store idiom).
    surf.init(
        store.as_mut_ptr() as usize,
        len,
        FrameBufferInfo {
            width: cw,
            height: ch,
            stride: cw,
            bytes_per_pixel: 4,
            pixel_format: PixelFormat::Bgr,
        },
    );

    // 5. UNMISTAKABLE CONTENT. Colours are written `0x00RRGGBB` — the same convention `jd1_test_pattern`
    //    uses — so this doubles as a format check on the composite path: quadrants in the named order
    //    mean the compositor's panel conversion is right, and swapped red/blue would name it wrong.
    //    A white frame proves the box's extent, and the magenta centre proves the interior is ours and
    //    not a stale boot-log glyph showing through.
    surf.fill_screen(0x0000_00FF); // blue field
    surf.fill_rect(0, 0, cw / 2, ch / 2, 0x00FF_0000); // red    — top-left
    surf.fill_rect(cw / 2, 0, cw - cw / 2, ch / 2, 0x0000_FF00); // green  — top-right
    surf.fill_rect(cw / 2, ch / 2, cw - cw / 2, ch - ch / 2, 0x00FF_FF00); // yellow — bottom-right
    surf.fill_rect(cw / 4, ch / 4, cw / 2, ch / 2, 0x00FF_00FF); // magenta centre block
    let f = 4usize;
    surf.fill_rect(0, 0, cw, f, 0x00FF_FFFF);
    surf.fill_rect(0, ch.saturating_sub(f), cw, f, 0x00FF_FFFF);
    surf.fill_rect(0, 0, f, ch, 0x00FF_FFFF);
    surf.fill_rect(cw.saturating_sub(f), 0, f, ch, 0x00FF_FFFF);

    // 6. SPAWN-PLACE — the outer box is sized BEFORE any row exists and the row is created THERE,
    //    pinned, so no pixel of this window is ever presented at a position it will not occupy.
    //    `create` + `move_to` reaches the same place but shows one frame at the tiler's origin and
    //    leaves a vacated box behind; the rMBP and the Pi both learned that on metal and there is no
    //    reason to relearn it here. Centred on the panel — `create_at` applies the same clamp `move_to`
    //    does, so a work-area reservation a later rung adds pushes this down rather than stranding it.
    let base = surf.base();
    let Some((scale, ow, oh)) = wm::spawn_geometry(cw, ch) else {
        serial_println!("[orinwm1] DECLINE reason=geometry-unavailable panel={}x{}", pw, ph);
        return wm::WIN_NONE;
    };
    let ox = pw.saturating_sub(ow) / 2;
    let oy = ph.saturating_sub(oh) / 2;
    let id = wm::create_at(
        wm::KERNEL_OWNER_DESKTOP,
        base,
        len,
        cw as u32,
        ch as u32,
        stride as u32,
        b"orin",
        ox + wm::BORDER,
        oy + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        // `store` drops here — no row points at it, so there is nothing to keep alive.
        serial_println!("[orinwm1] DECLINE reason=create-failed panel={}x{}", pw, ph);
        return wm::WIN_NONE;
    }
    // The row is live and holds a raw pointer into `store`: park it where it outlives this frame.
    // Moving the `Vec` moves its header, not its heap block.
    *ORINWM1_STORE.lock() = Some(store);
    ORINWM1_WIN.store(id, Ordering::Release);

    // 7. PRESENT + COMPOSITE. `create_at` already composited the new row; the explicit present is what
    //    this rung is actually measuring — the surface→panel path, end to end, on Orin silicon.
    //    `present_outcome` over `present` for the naming alone: `present`'s `bool` folds "the pass ran"
    //    into "the pass was suppressed", and on a rung whose entire verdict is "did pixels reach glass"
    //    those are the two answers that must not look alike.
    //    The trailing verdict is DERIVED from that outcome, never asserted: a line that says
    //    `-> COMPOSITED` on a pass that did not composite is exactly the silent-failure shape the
    //    verification law exists to forbid, and this is the only line a bench capture is counted from.
    let outcome = wm::present_outcome(id);
    wm::composite();
    let (pres, verdict) = match outcome {
        wm::Presented::Composited => ("Composited", "COMPOSITED"),
        wm::Presented::Coalesced => ("Coalesced", "COMPOSITED"),
        wm::Presented::Suppressed => ("Suppressed", "PRESENT-DECLINED"),
        wm::Presented::NoRow => ("NoRow", "PRESENT-DECLINED"),
    };
    serial_println!(
        "[orinwm1] win={} panel={}x{} surf={}x{} box={}x{} at ({},{}) scale={} stage={} present={} -> {}",
        id, pw, ph, cw, ch, ow, oh, ox, oy, scale, staged, pres, verdict
    );

    // 8. CHROME ON GLASS — READ THE PANEL BACK. See `orin_chrome_probe` for why the wire needed this
    //    and for what each sample can and cannot prove.
    orin_chrome_probe(id, ox, oy, ow, oh, cw, ch, scale);
    id
}

// -------------------------------------------------------------------------------------------------
// ORIN-CHROME — the GROUND-TRUTH read-back of the window's frame. `orindesk`, rides ORIN-WM1.
// -------------------------------------------------------------------------------------------------
//
// WHY THIS EXISTS, STATED AS THE ERROR IT PREVENTS. The orin-5 baton recorded, as a MEASURED fact,
// that boot7f's window "has NO FRAME ... `chrome_raster`, `CHROME_CELL`, `caption` are all 0 symbols
// in the flown ELF, dead-stripped." Every one of those three names is a COMPILE-TIME construct —
// `chrome_raster` is a `const fn` (`video/font.rs`) consumed only by `const CHROME_SIZE`,
// `CHROME_CELL_W`/`_H` are the `pub const`s it feeds, and the caption's painter is not called
// `caption` at all (it is `wm::draw_title`). Const-evaluated constants leave NO runtime symbol
// whether or not the chrome paints, so that check could not have come out any other way, and it was
// read as evidence of absence. It was wrong: `[crispy] theme=us-crispy-modern@0787ba9f frame=5
// bevel=1 title_h=34 …` is in the SAME boot7f capture, three lines above `[orinwm1]`, and
// `wm::crispy_witness` has exactly one caller — `paint_window`'s `if !r.compat` chrome arm, latched
// once per boot. The frame was painted on the first Orin window ever composited.
//
// So the gap this rung closes is not a missing painter. It is that NOTHING on this board's wire
// distinguishes "the chrome painter RAN" from "chrome pixels are IN THE SCANOUT". `[crispy]` proves
// the first and cannot prove the second; this panel is a DRAM carveout scanned out by a block that
// does not snoop, so the compose's trailing `flush_rect` is a real step that can fail on its own. A
// read-back is the only answer that is about the GLASS.
//
// WHAT IS SAMPLED, AND WHY THESE POINTS. `paint_window` machines the window FACE and the TITLE STRIP
// through `video::ceramic` (a per-row modulation of up to ~2 % of a channel) and the control discs
// through `ceramic` + `video::knurl`, so none of those pixels equals a theme constant and none can
// carry an EXACT verdict. The KEYLINE and the two BEVEL hairlines are documented in that same
// function as deliberately NOT machined ("a single-pixel edge has no room to show a grain"), so they
// are `theme::FRAME_LINE`, `theme::BEVEL_LIGHT` and `theme::BEVEL_SHADOW` exactly — six pixels whose
// expected value is a constant this file can compare against without re-deriving one line of chrome
// arithmetic. Their coordinates are `paint_window`'s own `fill_rect_v` extents read off the outer
// box: keyline on all four edges at `kw = theme::BEVEL`, bevel light one row inside the top, bevel
// shadow one row inside the bottom. All six are sampled at the box's MID-EDGE, which clears
// `theme::CORNER_RADIUS` at both ends by construction (2*12 « 650).
//
// THE CONTENT PROBE IS THE DISCRIMINATOR, and it is what keeps this line from raising a false alarm.
// The surface's magenta centre block is written by step 5 as `0x00FF_00FF` and reaches the panel at
// `scale = 1` through the same compose and the same `flush_rect` as the frame. Frame probes all
// missing WITH the content probe hitting means chrome specifically did not land — the shape the
// baton feared. Both missing means nothing from this pass reached the scanout and the frame is not
// the story. Two ceramic-modulated samples (the strip's blank right end, the face beside the
// content) are printed RAW beside them for a reader to compare against `[crispy]`'s own `face=` /
// `title_act=` fields; a verdict on those would be a verdict on the material, not on the frame.
//
// SAFETY AND COST. `FrameBuffer::read_pixel` is the compositor's own verify primitive — bounds-checked
// against the mapped length, `None` off-panel, and the one place the read-back ban is lifted by name.
// Nine reads, once, on the boot core with no scheduler running. `WRITER` is copied out and its guard
// dropped in a single statement, so no `wm` lock is ever taken under it (ORIN-WM1's acyclic rule);
// `close_box_rect` takes the window TABLE and is called BEFORE that copy, never under it.
#[cfg(feature = "orindesk")]
fn orin_chrome_probe(
    id: crate::video::wm::WinId,
    ox: usize,
    oy: usize,
    ow: usize,
    oh: usize,
    cw: usize,
    ch: usize,
    scale: usize,
) {
    use crate::video::{theme, wm};

    // Read BEFORE `WRITER` is taken: `control_disc_rect` locks the window table, and the
    // WRITER -> TABLE order this file keeps acyclic forbids that nesting.
    let disc = wm::close_box_rect(id);

    let fb = *crate::video::WRITER.lock();
    if !fb.is_ready() {
        serial_println!("[orinchrome] DECLINE reason=no-panel");
        return;
    }

    // `paint_window`'s own widths, not a second copy of them: the keyline is `kw = theme::BEVEL`
    // thick, and the bevel is `theme::BEVEL` thick drawn inset by the keyline's width.
    let kw = theme::BEVEL;
    let mx = ox + ow / 2; // mid-edge column — clear of both top corner arcs
    let my = oy + oh / 2; // mid-edge row
    let probes: [(&str, usize, usize, u32); 6] = [
        ("kl_top", mx, oy, theme::FRAME_LINE),
        ("kl_bot", mx, oy + oh - kw, theme::FRAME_LINE),
        ("kl_left", ox, my, theme::FRAME_LINE),
        ("kl_right", ox + ow - kw, my, theme::FRAME_LINE),
        ("bev_lt", mx, oy + kw, theme::BEVEL_LIGHT),
        ("bev_sh", mx, oy + oh - kw - theme::BEVEL, theme::BEVEL_SHADOW),
    ];
    let mut hit = 0usize;
    let mut read = 0usize;
    for (name, x, y, want) in probes.iter() {
        match fb.read_pixel(*x, *y) {
            Some(got) => {
                read += 1;
                if got == *want {
                    hit += 1;
                }
                serial_println!(
                    "[orinchrome] probe={} at ({},{}) got={:#08x} want={:#08x} -> {}",
                    name,
                    x,
                    y,
                    got,
                    want,
                    if got == *want { "MATCH" } else { "MISS" }
                );
            }
            None => serial_println!(
                "[orinchrome] probe={} at ({},{}) -> UNMAPPED (off-panel, or past the mapped length)",
                name,
                x,
                y
            ),
        }
    }

    // THE DISCRIMINATOR — the surface's magenta centre block, unmodulated. The content rectangle is
    // `paint_window`'s own (`w * scale` by `h * scale` at the content origin), so this is the centre
    // of the block at ANY integer scale the placer picks, not only at the bench panel's 1.
    let (cx, cy) = (
        ox + wm::BORDER + cw * scale / 2,
        oy + wm::TITLE_H + wm::BORDER + ch * scale / 2,
    );
    let content = fb.read_pixel(cx, cy);
    let content_ok = content == Some(0x00FF_00FF);

    // CERAMIC'S POPULATION — printed raw, never judged.
    let strip = fb.read_pixel(ox + ow - wm::BORDER - 2, oy + wm::BORDER + wm::TITLE_H / 2);
    let face = fb.read_pixel(ox + kw + theme::BEVEL, cy);
    let ctrl = disc.and_then(|(bx, by, d)| fb.read_pixel(bx + d / 2, by + d / 2));

    // The verdict is DERIVED from the counts above, never asserted — the law `[orinwm1]`'s own
    // trailing verdict is written under.
    let verdict = if read == 0 {
        "UNREADABLE"
    } else if hit == read {
        "CHROME-ON-GLASS"
    } else if hit > 0 {
        "CHROME-PARTIAL"
    } else if content_ok {
        "CHROME-MISSING"
    } else {
        "COMPOSITE-NOT-ON-GLASS"
    };
    serial_println!(
        "[orinchrome] win={} box={}x{} at ({},{}) frame={}/{} content={:#08x}@({},{}) {} strip={:#08x} face={:#08x} ctrl={:#08x} (ceramic — raw, compare with [crispy]) -> {}",
        id,
        ow,
        oh,
        ox,
        oy,
        hit,
        read,
        content.unwrap_or(0),
        cx,
        cy,
        if content_ok { "MATCH" } else { "MISS" },
        strip.unwrap_or(0),
        face.unwrap_or(0),
        ctrl.unwrap_or(0),
        verdict
    );
}

// =================================================================================================
// JD1-DC — the BPMP-GUARDED, READ-ONLY nvdisplay register probe. `jd1dc`, DEFAULT OFF.
// =================================================================================================
//
// THE QUESTION THIS RUNG ANSWERS, IN ONE ATTENDED BOOT. Can the CCPLEX see the Tegra234 nvdisplay
// registers at all, and if it can, do they still hold what the firmware programmed? Everything above
// this line on the Orin's display stack is INFERENCE from a DRAM carveout: JD1 inherits a scanout
// address out of the DTB and paints into it, and the panel lighting up proves only that SOMETHING is
// still scanning that memory out. It has never proved that the display block itself answers a CPU
// read. `jd1_dc_survey` (above) was written to answer exactly that and has never run — it could not,
// because it sits inside `jd1_survey`, ~70 lines before the BPMP channel exists, and it must not run
// without a guard (see THE GUARD below). This block is that survey given the guard, the ordering and
// the verdicts it needs to be fired once and read once.
//
// WHAT IS KNOWN GOING IN (2026-08-22, primary sources, not inference). The Tegra234 display
// controller IS directly programmable from the CCPLEX by plain MMIO, and NVIDIA's own UEFI proves it
// on this exact board: `edk2-nvidia`'s `Silicon/NVIDIA/Drivers/NvDisplayControllerDxe/NvDisplayHw.c`
// does raw `MmioRead32`/`MmioWrite32` against this aperture — reading `DISPLAY_FE_SW_SYS_CAP`
// (+0x0003_0000) to enumerate heads and SORs, then programming `DISPLAY_FE_CMGR_CLK_RG`/`_SOR` — and
// the string `dce` does not appear anywhere in that tree. UEFI sets the mode on this board's
// DisplayPort, draws its splash and runs its menu without ever speaking to the DCE. The DCE-RPC path
// is NVIDIA's *Linux runtime* driver architecture, one of two that exist; it is not a hardware gate,
// and the "display is behind a hypervisor" reports trace to DRIVE OS automotive virtualization, not
// to Jetson. Since UEFI reaches this aperture from the CCPLEX here, MB1/MB2's Security Configuration
// Registers already grant CCPLEX access by default. The block itself is the standard NVIDIA
// `NV_PDISP` block rebased to offset 0 of the 0x1380_0000 aperture (RM's
// `kdispGetBaseOffset_v04_02()` returns `0x0 - DRF_BASE(NV_PDISP)`), so the register semantics are
// the familiar NVIDIA display ones rather than a Tegra-only invention.
//
// THAT INVERTS THE READING OF ONE OUTCOME, WHICH IS WHY IT IS SPELLED OUT ON THE WIRE. "All-ones /
// all-zero / garbage" used to be the expected answer. It is now the SURPRISING one: on a board where
// UEFI demonstrably drives these very registers from this very core, a garbage read is a finding
// about OUR access path — our aperture mapping, our probe point's power/clock state, or an SCR the
// firmware left narrower than UEFI's own — and NOT a finding about Tegra234. The verdict line says
// so in as many words, because the next reader of that capture would otherwise conclude "the DCE
// holds it", which is now known to be false.
//
// THE GUARD, WHICH IS THE WHOLE SAFETY STORY. A read of a POWER-GATED Tegra block is EL3-FATAL: the
// JX1 event took an SError with `ESR 0xbe000011` (EC=0x2F) and NVIDIA's BL31 printed "Unhandled
// Exception in EL3" — the boot is over, no handler of ours runs, nothing more reaches the wire. So
// before the FIRST nvdisplay read this rung asks the only authority that knows — BPMP, over the
// HSP+IVC channel JB1b proved — for the display domain's state, by MRQ_PG `CMD_PG_GET_STATE`, and
// reads nothing at all unless every domain the DTB lists for that node answers `err=0 state=0x1`.
// There is no CPU-side register that could answer instead: on Tegra234 the DISP power domain, its
// ~60 clocks, its resets and its ISO bandwidth are BPMP's alone. The guard is not belt-and-braces;
// it is the only way to earn the first read.
//
// AND THE GUARD NEVER POWERS ANYTHING. `MRQ_PG SET_STATE` on the display domain is deliberately not
// not NAMED from here (`bpmp_tegra` keeps `CMD_PG_SET_STATE`/`PG_STATE_ON` private and exposes only
// the getter to this feature). CORRECTION, 2026-08-22, from the landing panel — `ab168ba2`'s message
// called this "unreachable" and that OVERSTATES IT. `Chan::transfer` is `pub` and takes the MRQ and
// payload as raw `u32`s, and this block holds a `&Chan`; `chan.transfer(66, &[1, id, 1])` would send
// MRQ_PG CMD_PG_SET_STATE PG_STATE_ON from right here. Private consts stop it being NAMED, nothing
// stops it being SENT. So this is a convention with a speed bump, NOT a type-system guarantee — and
// the danger of the stronger wording is specific: a later rung "just needs the domain on", writes the
// three numerals instead of adding a wrapper, and the review that follows reads the old claim,
// believes the compiler forbids it, and does not look. Powering that domain —
// or worse, cycling it — would tear down the live scanout the whole JD1 inheritance rests on, and is
// unrecoverable for that boot. Domain not ON is a REFUSAL, printed and named, not a problem to fix.
//
// NOT ONE REGISTER IS WRITTEN. Every nvdisplay access below is `core::ptr::read_volatile`. There is
// no `write_volatile` in this block and there must not be: the DCE R5 is live on the other side of
// this aperture running 11.8 MiB of authenticated firmware, and a write is a two-writer race against
// it. `JD1_DC_PROBE`'s own inherit-don't-reinit rule, one rung further out. The census line carries
// `writes=0` so the capture certifies it too.
//
// WHERE IT RUNS, AND WHY THAT POINT AND NO OTHER. `tegra_early_stop`, as the last statement of the
// BPMP block — which is the only instruction in the boot where BOTH of its ordering constraints
// hold. BPMP-FIRST: the channel is established ~60 lines earlier (`jb1b_ping`), and without it the
// guard cannot be asked, so the probe cannot sit at its old site inside `jd1_survey`. JD1-FIRST: the
// JD1 scanout resolution, `map_fb_region`, `fbcon::init` and the `WRITER` seed are all ~90 lines
// EARLIER, and that ordering is what makes the fail-back structural rather than hopeful — the panel,
// the serial console and the shell are already alive on a framebuffer resolved by a pure DTB RAM
// walk, so if this probe goes wrong it costs the experiment and not the boot. Being last in the BPMP
// block is the third, weaker reason: every other diagnostic that block produces has already reached
// the wire before the one read that could end the boot.
//
// DEFAULT OFF AND MEASURED. With `jd1dc` unset every item below vanishes, both edits to
// already-compiled code are statements APPENDED to existing lines (so no `core::panic::Location` in
// `display_tegra.rs`, `fdt_tegra.rs` or `main.rs` moves — the line-shift class that silently breaks
// byte-identity), and the shipped jetson image is byte-identical to baseline. Arm with
// `UNAOS_TEGRA=1 UNAOS_JD1DC=1`; the ARMED polarity is type-checked by the `arm-tegra-jd1dc` leg of
// `KERNEL_CFG_MATRIX`, never by the knob mapping. Witness on the wire: `JD1-DC VERDICT=`.

/// JD1-DC: the scanout base JD1 resolved and accepted this boot, latched by `jd1_survey` so the
/// probe can test a window's `START_ADDR` against it. 0 = JD1 resolved no scanout (headless boot) —
/// which is exactly the case the DC survey was originally written as the fallback for, so the probe
/// still runs and simply reports that it has nothing to compare against.
#[cfg(feature = "jd1dc")]
static JD1DC_SCANOUT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// **JD1-DC — the guarded, read-only nvdisplay register probe.** One attended boot, four
/// distinguishable outcomes, no writes.
///
/// Exactly one `JD1-DC VERDICT=` line is printed on every path that reaches an nvdisplay read or
/// declines to, and the four outcomes the flight is designed around read as:
///
/// * `VERDICT=REACHABLE` — registers decode AND a window's `START_ADDR` equals the inherited scanout
///   base. The CCPLEX can see nvdisplay and we are looking at what the firmware programmed.
/// * `VERDICT=DECODES-NOMATCH` — the FE capability word decodes but no `START_ADDR` matched. SINCE
///   JX1 THIS IS THE ONLY OUTCOME THE WINDOW HALF CAN PRODUCE, and it is produced with `swept=0`:
///   the Tegra194 window sweep is gated to an empty slice (its first read was EL3-fatal on this
///   silicon), so no window is ever read, none is ever enabled, and no `START_ADDR` can ever match.
///   The verdict now rests entirely on the FE capability word. It is NOT evidence that the scanout
///   is fed from a window the sweep missed — nothing was swept. The window question moved to
///   `JX2-VERDICT=`, which asks it in the NVC67D channel model this silicon actually presents.
/// * `VERDICT=NOT-DECODING` — all-ones / all-zero / no enabled window. See the block comment: on this
///   board that is a finding about OUR access path, not about the silicon.
/// * `VERDICT=REFUSED` — the guard said no (or could not be asked) and NOT ONE register was read.
///
/// The fifth outcome prints nothing of ours by construction: an EL3-fatal SError ends the boot inside
/// the read. That is why the last line before every new touch names the exact address about to be
/// read and says that its own silence is the result.
#[cfg(feature = "jd1dc")]
pub fn jd1_dc_probe(chan: &super::bpmp_tegra::Chan, dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    use core::sync::atomic::Ordering;

    // 1. THE APERTURE — resolved from the live DTB, never hardcoded (verify-don't-assume), and a pure
    //    RAM walk: no MMIO yet. GiB 0 is already mapped Device-nGnRE unconditionally by `mmu_tegra`,
    //    so there is no mapping work to do and no new translation to get wrong.
    let Some((base, size)) = fdt_tegra::nvdisplay_base(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=REFUSED reason=no-aperture — the firmware DTB carries no display@ node with a usable reg; nothing to read, and NOT ONE nvdisplay register was read ::"
        );
        return;
    };
    // 2. THE GUARD'S SUBJECT — the BPMP resource ids of the SAME node, off the same tree. Matching on
    //    the same `display@` substring `nvdisplay_base` matched is what makes "the domain we prove ON
    //    owns the aperture we read" true by construction instead of by assumption.
    let Some(ids) = fdt_tegra::node_ids(dtb_addr, dtb_size, ram_gib_mask, b"display@") else {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=REFUSED reason=no-ids — display@ resolved an aperture ({:#x}) but no clocks/resets/power-domains (see the JD1-DC-IDS STOP line above); the domain state cannot be proven, so NOT ONE nvdisplay register was read ::",
            base,
        );
        return;
    };
    if ids.n_pds == 0 {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=REFUSED reason=no-power-domains — display@ lists {} clocks and {} resets but NO power-domains, so MRQ_PG has no id to ask about and the JX1 rule cannot be satisfied; NOT ONE nvdisplay register was read (next step: look for the power-domains property on the display-hub@ parent) ::",
            ids.n_clocks,
            ids.n_resets,
        );
        return;
    }
    // 3. THE GUARD ITSELF — read-only MRQ_PG GET_STATE, every domain the node lists, ALL must be ON.
    //    A gated block's first read is EL3-fatal (JX1), so this is the price of the first touch.
    for i in 0..ids.n_pds {
        let id = ids.pds[i];
        match super::bpmp_tegra::pg_get_state(chan, id) {
            Some((err, state)) => {
                serial_println!(
                    ":: tegra: JD1-DC — PG {} GET_STATE (read-only, no SET_STATE anywhere in this rung) err={} state={:#x} (need err=0 state=0x1) ::",
                    id,
                    err,
                    state,
                );
                if err != 0 || state != 1 {
                    serial_println!(
                        ":: tegra: JD1-DC VERDICT=REFUSED reason=domain-not-on — display@ power domain {} answered err={} state={:#x}; a read of a gated block is EL3-FATAL (JX1: SError ESR 0xbe000011 EC=0x2F, BL31 'Unhandled Exception in EL3'), so NOT ONE nvdisplay register was read. The domain is deliberately NOT powered on from here — SET_STATE on DISP would tear down the inherited scanout and is unrecoverable for this boot ::",
                        id,
                        err,
                        state,
                    );
                    return;
                }
            }
            None => {
                serial_println!(
                    ":: tegra: JD1-DC VERDICT=REFUSED reason=pg-timeout — MRQ_PG GET_STATE for domain {} got no response frame in 100 ms; the domain state is UNKNOWN and unknown is not ON, so NOT ONE nvdisplay register was read ::",
                    id,
                );
                return;
            }
        }
    }

    // ---- past this point, and only past this point, nvdisplay MMIO is touched ----
    let scanout = JD1DC_SCANOUT.load(Ordering::Relaxed);
    serial_println!(
        ":: tegra: JD1-DC — GUARD PASSED: all {} display@ power domain(s) ON. Aperture {:#x} size={:#x}; JD1 inherited scanout base = {:#x} ({}) ::",
        ids.n_pds,
        base,
        size,
        scanout,
        if scanout != 0 { "resolved this boot — START_ADDR will be compared against it" } else { "NOT resolved this boot — no comparison possible, raw values only" },
    );
    let rd = |off: u64| unsafe { core::ptr::read_volatile((base + off) as *const u32) };

    // 4. THE LANDMARK, AND THE FIRST TOUCH. `DISPLAY_FE_SW_SYS_CAP` (+0x0003_0000) is the register
    //    NVIDIA's own UEFI reads FIRST on this aperture to enumerate the block's heads and SORs
    //    (`NvDisplayHw.c`), which makes it both the best-evidenced safe read available to us and the
    //    single most decisive value on the flight: a sane head/SOR capability word here says "we see
    //    what UEFI sees" before any window state is interpreted. It is also a cross-check on the
    //    survey's head model — the four-heads-at-0x10000-stride layout below would put head 3 at this
    //    very offset, and both cannot be right.
    //    BOUNDED BEFORE TOUCHED, both candidates. `size` is the DTB's own declaration of the block's
    //    decode window; a read past it is precisely the unverified-aperture touch that made JX1 fatal,
    //    and the window sweep's per-head bound below cannot cover this landmark because it runs first.
    //    If the aperture is too small for EITHER the FE cap word or a whole head-0 window bank, there
    //    is nothing this rung may legally read and it refuses rather than trimming the sweep down to
    //    something whose silence would be indistinguishable from a dead block.
    let have_cap = size >= 0x0003_0004;
    let have_win0 = 0x2800 + 6 * 0xC00 <= size;
    if !have_cap && !have_win0 {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=REFUSED reason=aperture-too-small — display@ declares reg size={:#x}, which holds neither DISPLAY_FE_SW_SYS_CAP (+{:#x}) nor a complete head-0 window bank (+{:#x}); reading past a DTB-declared aperture is the JX1 class itself, so NOT ONE nvdisplay register was read ::",
            size,
            0x0003_0004u64,
            0x2800 + 6 * 0xC00u64,
        );
        return;
    }
    let first_read_off: u64 = if have_cap { 0x0003_0000 } else { 0x2800 + 0x600 };
    serial_println!(
        ":: tegra: JD1-DC — FIRST TOUCH of a new MMIO class: about to read {} @{:#x}. If this is the LAST line on the wire, THAT read was EL3-fatal (JX1 class) and the boot ended inside it ::",
        if have_cap { "DISPLAY_FE_SW_SYS_CAP" } else { "head+0x0 win0 WIN_OPTIONS (aperture too small for FE_SW_SYS_CAP)" },
        base + first_read_off,
    );
    let first = rd(first_read_off);
    serial_println!(
        ":: tegra: JD1-DC — FIRST READ SURVIVED: {}={:#010x} — the CCPLEX decodes this aperture without an EL3 abort ::",
        if have_cap { "DISPLAY_FE_SW_SYS_CAP" } else { "head+0x0 win0 WIN_OPTIONS" },
        first,
    ); jd1_dc_model(base, size, have_cap, first); jx2_nvc67d_status(base, size); // JD1-DC-MODEL — the WHICH-CHIP discriminator, four more read-only reads, appended here because this is the first instruction at which an nvdisplay read is known non-fatal and the long window sweep has not yet risked the boot. Without it `VERDICT=DECODES-NOMATCH` is AMBIGUOUS between "the aperture is not live" and "our register map is Tegra194's and this silicon is Tegra234" — two answers that send the display arc in opposite directions. See the JD1-DC-MODEL block at this file's tail. APPENDED to this line, never a new one: knob-off it is cfg-erased and not one `core::panic::Location` below moves. || AND JX2-NVC67D, appended to the same line for the same two reasons: this is still the first instruction at which an nvdisplay read is KNOWN non-fatal on this boot, and appending rather than adding a statement line keeps every `core::panic::Location` below unmoved. It runs AFTER the discriminator because it CONSUMES the discriminator's answer — boot7f's `MODEL-VERDICT=NVDISPLAY-CLASS-C670` / `FE_CLASSES=0xc6700410` is the entire licence for its offsets — and it SUPERSEDES the JX1-gated Tegra194 window sweep immediately below, which is left in place as the record of why. See the JX2-NVC67D block at this file's tail.

    // 5. THE WINDOW SWEEP — read-only, every field, every window, EMPTY WINDOWS INCLUDED. The legacy
    //    `jd1_dc_survey` skips all-zero windows to keep its log short; this rung prints them, because
    //    "every window read zero" and "the sweep never ran" must not look alike in a capture that a
    //    decision rests on, and because a later save/restore rung needs the full field set from the
    //    same data. Offsets are the Linux Tegra DRM `hub.c` window layout documented on
    //    `jd1_dc_survey` — T186/T194's, NOT documented for T234, which is exactly what JD1-DC-MODEL
    //    tests; per-head stride 0x10000 (unconfirmed), bounded by the DTB-declared aperture size.
    let mut swept = 0u32;
    let mut all_ones = 0u32;
    let mut all_zero = 0u32;
    let mut enabled = 0u32;
    let mut matched = 0u32;
    let mut heads = 0u32;
    let mut match_head = 0u64;
    let mut match_win = 0u64;
    // ── JX1-WINSWEEP: THE SWEEP BELOW IS EL3-FATAL ON THIS SILICON. MEASURED, boot7e 2026-08-25. ──
    //
    // On the FIRST flight of this rung the announce fired and the board died on the very next
    // instruction:
    //
    //   :: JD1-DC — head+0x0: about to read win0 WIN_OPTIONS @0x13802e00 (announce before touch) ::
    //   Unhandled Exception in EL3.   x1 (ESR_EL3) = 0xbe000011      <- JX1 signature
    //
    // WHY, and it is not a mystery any more — the discriminator that ran seconds earlier answered it:
    //
    //   MODEL-VERDICT=NVDISPLAY-CLASS-C670   FE_CLASSES=0xc6700410 -> NVD_40 / class NVC67D, Ampere ga10x
    //   FE_HW_SYS_CAP =0x00100303 -> 2 heads, 2 SORs
    //   FE_HW_SYS_CAPB=0x0000000f -> FOUR windows exist
    //
    // This sweep's geometry is Tegra186/194's: FOUR head banks at a 0x10000 stride, SIX windows per
    // head at 0x2800 + 0xC00*win. The silicon is Ampere NVD_40 with TWO heads and FOUR windows total,
    // and its window state does not live at those offsets at all. `head+0x0 win0 WIN_OPTIONS`
    // (+0x2800+0x600 = +0x2e00) is not a register on this part; the fabric refused the read and BL31
    // took an EL3 abort. The DTB-declared `size=0xeffff` bound could never have caught it — the offset
    // is INSIDE the aperture and still not decodable.
    //
    // GATED OFF rather than deleted: every line of it is the correct shape for the answer we now want,
    // and the raw dump it emits is exactly what a save/restore would be reconstructed from. It comes
    // back when its offsets are rewritten against the NVC67D class-channel model (NVIDIA/open-gpu-doc,
    // MIT; `clc67d.h`), which is the next display rung's job — and note the model is CHANNEL-based
    // (`UPDATE` at method 0x200), so the replacement is unlikely to be a flat MMIO sweep at all.
    //
    // The FIRST-TOUCH announce is what made this diagnosable in one boot instead of a bisect: the
    // silence after it names the fatal register exactly. Keep that discipline in the replacement.
    // THE WIRE HAS TO SAY THIS, not just the source. `swept=0` on a capture is otherwise indis-
    // tinguishable from "the sweep ran and found nothing", and the DECODES-NOMATCH line below used to
    // offer exactly that false cause. One line, printed unconditionally, so no future capture can be
    // read the wrong way round.
    serial_println!(
        ":: tegra: JX2-SWEEPDISABLED — the Tegra186/194 window sweep below is GATED TO AN EMPTY SLICE and DOES NOT RUN. Every count it feeds is therefore zero BY CONSTRUCTION (swept=0 enabled=0 all_ones=0 all_zero=0 heads=0), not by measurement: nothing was read, so nothing could match. WHY: boot7e, 2026-08-25 — its first read, head+0x0 win0 WIN_OPTIONS @+0x2e00, was EL3-fatal (SError ESR 0xbe000011, BL31 'Unhandled Exception in EL3') on silicon that boot7f then identified as NVDISPLAY class NVC67D / NVD_40, whose window state is not MMIO at those offsets or any others. The replacement is the JX2-NVC67D rung, whose JX2-VERDICT= line above is where the window question is now answered ::"
    );
    #[allow(unused_variables)]
    for &head_off in &[] as &[u64] {
        if head_off + 0x2800 + 6 * 0xC00 > size {
            serial_println!(
                ":: tegra: JD1-DC — head+{:#x} would leave the DTB-declared aperture (size={:#x}); this head and every one after it is NOT read ::",
                head_off,
                size,
            );
            break;
        }
        serial_println!(
            ":: tegra: JD1-DC — head+{:#x}: about to read win0 WIN_OPTIONS @{:#x} (new decode region — announce before touch) ::",
            head_off,
            base + head_off + 0x2800 + 0x600,
        );
        heads += 1;
        for win in 0u64..6 {
            let ap = head_off + 0x2800 + 0xC00 * win;
            // Every field the sweep touches, read once and reported raw — a later save/restore can be
            // reconstructed from these lines alone.
            let opts = rd(ap + 0x600); // WIN_OPTIONS      (WIN_ENABLE = bit 30)
            let owner = rd(ap + 0x608); // WINDOWGROUP_SET_CONTROL (OWNER = low nibble)
            let depth = rd(ap + 0x60c); // COLOR_DEPTH
            let osize = rd(ap + 0x614); // SIZE            (output extent)
            let crop = rd(ap + 0x618); // CROPPED_SIZE     (source extent)
            let planar = rd(ap + 0x624); // PLANAR_STORAGE (pitch, in 64-byte units)
            let lo = rd(ap + 0x700); // START_ADDR (lo)
            let kind = rd(ap + 0x72c); // SURFACE_KIND     (0 = pitch)
            let hi = rd(ap + 0x734); // START_ADDR_HI
            swept += 1;
            let fields = [opts, owner, depth, osize, crop, planar, lo, kind, hi];
            if fields.iter().all(|&v| v == 0xFFFF_FFFF) {
                all_ones += 1;
            }
            if fields.iter().all(|&v| v == 0) {
                all_zero += 1;
            }
            let en = (opts >> 30) & 1;
            if en == 1 {
                enabled += 1;
            }
            // The read value is an SMMU IOVA (identity-mapped on this firmware) with bit 39 a GPU
            // sector-swizzle flag to mask before comparing against a physical address.
            let iova = (((hi as u64) << 32) | lo as u64) & !(1u64 << 39);
            if scanout != 0 && iova == scanout {
                matched += 1;
                match_head = head_off;
                match_win = win;
            }
            serial_println!(
                ":: tegra: JD1-DC RAW head+{:#x} win{} @{:#x}: OPTIONS={:#010x}(en={}) OWNER={:#010x} COLOR={:#010x} SIZE={:#010x} CROP={:#010x} PLANAR={:#010x} START_LO={:#010x} START_HI={:#010x} KIND={:#010x} | derived (T194 layout): out={}x{} src={}x{} pitch={}B iova={:#x}{} ::",
                head_off,
                win,
                base + ap,
                opts,
                en,
                owner,
                depth,
                osize,
                crop,
                planar,
                lo,
                hi,
                kind,
                osize & 0xffff,
                (osize >> 16) & 0xffff,
                crop & 0xffff,
                (crop >> 16) & 0xffff,
                (planar & 0x1fff) * 64,
                iova,
                if scanout != 0 && iova == scanout { " == JD1 SCANOUT" } else { "" },
            );
        }
    }

    // 6. THE CENSUS AND THE VERDICT — always both, always exactly one verdict, so one capture decides.
    serial_println!(
        ":: tegra: JD1-DC CENSUS — heads={} windows={} all-ones={} all-zero={} WIN_ENABLE={} START==JD1-scanout={} | reads={} writes=0 ::",
        heads,
        swept,
        all_ones,
        all_zero,
        enabled,
        matched,
        swept * 9 + 1 + JD1DC_MODEL_READS.load(Ordering::Relaxed), // +1 = the FE cap word read as `first`; + however many of the JD1-DC-MODEL reads passed their own bounds check (0..4). A census that under-counts its own reads is an instrument that lies about its own footprint, and `writes=0` beside it only means anything if the read count is exact.
    );
    if matched > 0 {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=REACHABLE — nvdisplay decodes for the CCPLEX AND head+{:#x} win{} holds START_ADDR {:#x}, the very base JD1 inherited: we can see what UEFI programmed. The pipe is reachable from this core by plain MMIO, exactly as edk2-nvidia's NvDisplayHw.c reaches it ::",
            match_head,
            match_win,
            scanout,
        );
    // A SANE FE CAPABILITY WORD IS EVIDENCE OF DECODE IN ITS OWN RIGHT, independent of every window:
    // it is the register UEFI enumerates heads and SORs from, and neither all-ones nor all-zero is a
    // plausible value for it. Folding it into the "decodes" test is what keeps the verdict honest in
    // the two cases the window census alone would misread — an aperture too small for a whole head-0
    // bank (`swept == 0`), and a decoding block whose windows are simply all disabled.
    } else if (have_cap && first != 0xFFFF_FFFF && first != 0) || enabled > 0 {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=DECODES-NOMATCH — the aperture DECODES ({} window(s) at WIN_ENABLE=1 out of {} swept), but no START_ADDR equalled the JD1 scanout base {:#x}{}. Reachability is ESTABLISHED — by the FE capability word alone. READ THE WINDOW COUNTS AS ZERO BY CONSTRUCTION, NOT AS A MEASUREMENT: since JX1 the Tegra194 sweep is gated to an empty slice and never runs, so swept=0 and enabled=0 always, and the earlier reading of swept=0 as 'the aperture held the FE capability word but no complete head-0 window bank' was a FALSE CAUSE and is retracted here. Nothing was swept, so nothing could match, and this verdict says NOTHING about where the scanout is fed from. That question moved to the JX2-VERDICT= line above. Compare the RAW lines above against DISPLAY_FE_SW_SYS_CAP={:#010x}, which is UEFI's own head/SOR enumeration. THIS VERDICT DOES NOT SAY WHY: on its own it cannot separate 'the aperture is live and the window is somewhere we did not look' from 'our window offsets are Tegra194's and this silicon is not Tegra194'. The JD1-DC MODEL-VERDICT line ABOVE is the one that separates them — read it first ::",
            enabled,
            swept,
            scanout,
            if scanout == 0 { " (JD1 resolved NO scanout this boot, so no comparison was possible — the match test is vacuous here, not failed)" } else { "" },
            first,
        );
    } else {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=NOT-DECODING — {} of {} windows read all-ones and {} read all-zero, none enabled. THOSE THREE COUNTS ARE ZERO BY CONSTRUCTION SINCE JX1 (the Tegra194 sweep is gated to an empty slice and never runs), so they are not evidence of anything; the only load-bearing term in this verdict is the FE capability word. READ THIS CAREFULLY: UEFI drives THESE registers on THIS board from THIS core by plain MmioRead32/MmioWrite32 (edk2-nvidia NvDisplayControllerDxe/NvDisplayHw.c), and the string 'dce' appears nowhere in that tree — so this result is NOT 'the DCE holds the block'. It is a finding about OUR access path: our aperture/mapping, the domain or clock state at our probe point, or an SCR narrower for us than for UEFI. FE_SW_SYS_CAP read {:#010x}. AND IT IS NOT A STATEMENT ABOUT THE WINDOW OFFSETS EITHER — a wrong register map produces DECODES-NOMATCH, not this; if the JD1-DC MODEL-VERDICT line above says the aperture answered anything at all, prefer that line's reading of it over this one ::",
            all_ones,
            swept,
            all_zero,
            first,
        );
    }
}

// =================================================================================================
// ORIN-CLICK — rung 3 of the Orin desktop ladder. `orinclick`, DEFAULT OFF.
// =================================================================================================
//
// WHAT THIS RUNG IS. `jd2_console_pump`'s `Event::Button` arm has, since JD20, LOGGED the press and
// dropped it ("no UI action wired yet", main.rs). Every other board routes that edge into the window
// layer through `arch/aarch64/syscall.rs::wc_click_route`; the Orin was the one arch where the router
// existed, compiled, and had no caller (orin-desktop.md §3.4). This block is that caller, plus the
// instrument that says on the wire whether it worked.
//
// WHY IT IS RUNG 3 AND NOT RUNG 4. `video/desktop_firmware.rs:39-44` states the CONSOLEWIN law: the console
// window carries a minimise disc and THE ONLY ROUTE BACK FROM THAT PARK IS THE DOCK. The dock is a
// route back only once clicks route. Landing "console as a window" before this rung would ship a
// minimise button that is a one-way trip — "a control that hides a window with no way back is worse
// than no control". So this is the enabling safety precondition, and nothing here routes the console
// into a `wm` row.
//
// WHAT THIS RUNG IS NOT — the §5.2 STOP-LINE, restated because this file is where it would be broken.
// The Pi overflowed a 16 KiB kernel stack in the desktop-arming cascade TWICE on consecutive metal
// boots (boots 10 and 11), and neither reproduces on any QEMU gate in this tree. Boot 11's victim was
// `quarry::open()` running SYNCHRONOUSLY AT CLICK-ROUTER DEPTH on the input-drain task — i.e. exactly
// this call stack. This rung therefore does NOT arm `desktop_firmware`: `orinclick` implies `tegra_el0` and
// NOTHING ELSE, so every furniture arm inside `wc_click_route` (`strip::press_route`,
// `quarry::service`, `pulsewin::press_route`, `quarry::press_route`, the DRAG-PI chrome arm and the
// SHELLWIN-PI furniture arm) is `#[cfg(feature = "desktop_firmware")]` and COMPILED OUT. What is left is the
// window half: `wm::hit_test`, `wm::close_box_hit`/`minimise_hit`/`zoom_hit`, `focus_changed`, and
// `user_input_set_active` — none of which opens a file, and none of which is on either recorded
// overflow's path. No dock, no strip, no menubar, no crystal, no `render_service`.
//
// WHY `orinclick` IMPLIES `tegra_el0`, and why that is the SHAPE OF THE CONFIGURATION rather than a
// wider net. `wc_click_route` lives in `arch/aarch64/syscall.rs`, and `arch/aarch64/mod.rs:46` gates
// `pub mod syscall;` on `feature = "aarch64_el0"`. `baremetal` implies `pi`
// and `pi` + `tegra` is a hard `compile_error!` (`arch/aarch64/serial.rs:22`), so on the Orin the ONLY
// satisfiable term is `tegra_el0`. A standalone `orinclick = []` in the `orindesk`/`jd1dc`/`smpmark`
// mould would have been a knob that compiles NOTHING unless the operator happens to also set
// `UNAOS_TEGRA_EL0=1` — a vacuous gate wearing a green verdict, the defect class `arroyo`'s own
// KERNEL_CFG_MATRIX preamble is written against. `tegra_el0` implies `tegra`, so
// `UNAOS_ORINCLICK=1 ./arroyo check` and `UNAOS_ORINCLICK=1 ./arroyo esp-jetson` are both
// self-sufficient. The ARMED polarity is type-checked by the `arm-tegra-orinclick` leg of
// KERNEL_CFG_MATRIX, and the `desktop_firmware` CROSS by `arm-tegra-desk` — never by the knob mapping.
//
// DEFAULT OFF AND MEASURED. With `orinclick` unset every item below vanishes and the two call sites
// in `main.rs` are `#[cfg]`-erased STATEMENTS APPENDED TO EXISTING LINES, so no line moves in any file
// compiled knob-off (the panic-`Location` line-shift class is how byte-identity is usually lost
// silently) and the jetson image is byte-identical to baseline. Measured, not argued — see
// orin-desktop.md §3.7.
//
// ── THE INSTRUMENT ───────────────────────────────────────────────────────────────────────────────
//
// THE PROBLEM THIS INSTRUMENT EXISTS TO SOLVE IS NOT "did the click work". It is: **an ABSENCE is
// only evidence if the thing that would have produced it was actually attempted.** `[clickroute]`
// missing from a capture where nobody touched the mouse is not a failing test, it is an UNRUN one,
// and the router's own lines cannot tell those apart — `wc_click_route` prints on exactly two of its
// arms (a press that MOVED focus to a window, and a miss while some app held focus) and is SILENT on
// a re-click of the focused window, on every release, and on every press while focus is already the
// shell, which on a fresh Orin boot is all of them.
//
// So three kinds of line, and the census is the load-bearing one:
//
//   1. `[orinclick] arm ...`   — ONCE, from the phase-2 drain loop's own cadence. Proves the caller
//      is wired and names what it has to aim at. Its verdict is DECLINED, not passed, when the `wm`
//      table is empty — which is the DEFAULT armed boot (`orindesk` off), so this instrument's
//      non-pass path is the one a real boot takes.
//   2. `[orinclick] edge=... -> VERDICT` — one per Button event, verdict DERIVED from the hit-test,
//      the focus either side of the call, and the router's own return value. Never asserted.
//   3. `[orinclick] census ... -> VERDICT` — every ~10 s FROM INSIDE THE DRAIN LOOP, unconditionally,
//      whether or not anything was clicked.
//
// WHAT THE CENSUS BUYS, spelled out because it is the whole design:
//
//   * `census ... btn=0 -> IDLE-NO-CLICKS`  =  the pump is ALIVE and NOBODY CLICKED. UNRUN, not
//     failed. This is the line that makes a missing `[clickroute]` interpretable.
//   * `census ... btn=N ... -> FAIL/DECLINE` = clicks arrived and did not route. FAILED.
//   * `arm` printed and the census then STOPS = the drain task is DEAD or wedged. This matters here
//     specifically: pi's boot 11 killed cpu 3 and `[el0live] verdict=LIVE` printed ONE LINE LATER,
//     while `:: SCHED: load ::` read `c3=100%` for the corpse. Nothing in this tree inherits a dead
//     task's singleton roles (`steal_ok` is false for every explicitly-pinned task and there is no
//     re-home path at all), so if this pump dies, clicks stop routing permanently and no other
//     instrument will say so. This census cannot make that mistake, because it is not an observer of
//     the routing task — it IS the routing task. A dead task prints nothing.
//   * `arm` ABSENT while `:: tegra: JD2 — console OWNS the panel` is present = the drain loop was
//     entered and did not survive its first quarter-second. `arm` absent AND that line absent = the
//     knob is off, or the boot was headless and the pump delegated to `kbd_pump_body`.
//   * `seq=` increments by exactly 1 per census, so a gap names a LOST serial line rather than
//     letting one evaporate (the SERWIT law), and `up=` is read from `CNTPCT_EL0` at print time on
//     this task's own core, so two consecutive census lines cannot carry the same clock.

/// ORIN-CLICK — Button events handed to this seam by `jd2_console_pump`. The denominator of everything.
#[cfg(feature = "orinclick")]
static CLK_BTN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-CLICK — our OWN mirror of the button mask, used to classify press/release/no-edge as the PUMP
/// saw it. Deliberately not the router's `CLICK_PREV_MASK` (which is private, and which this call
/// swaps): keeping a second mirror means the verdict below is an independent reading rather than a
/// restatement of the router's own bookkeeping. On tegra this seam is `wc_click_route`'s only caller
/// (`route_input_to_active_el0`'s call site is `#[cfg(feature = "baremetal")]`), so the two mirrors
/// track each other; if a future caller appears they can disagree, and the `[orinclick]` line and the
/// router's adjacent `[clickroute]` line are then both on the wire to be compared.
#[cfg(feature = "orinclick")]
static CLK_PREV_MASK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-CLICK — press edges, release edges, and calls that carried no edge at all.
#[cfg(feature = "orinclick")]
static CLK_PRESS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinclick")]
static CLK_REL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinclick")]
static CLK_NOEDGE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-CLICK — press dispositions. `RAISED` is the rung's whole point (a click moved focus to the
/// window under the cursor); `SAME` is a re-click of the already-focused window; `MISS` is the
/// desktop/console; `CONSUMED` is a furniture or control arm (only reachable on a `desktop_firmware` build).
#[cfg(feature = "orinclick")]
static CLK_RAISED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinclick")]
static CLK_SAME: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinclick")]
static CLK_MISS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinclick")]
static CLK_CONSUMED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-CLICK — **the FAIL counter.** A press that hit a window the focus was NOT on, was not
/// consumed by any arm, and did not move the focus: the router failing the one contract this rung
/// exists to exercise. Sticky — once nonzero the census reports FAIL for the rest of the boot.
#[cfg(feature = "orinclick")]
static CLK_STUCK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-CLICK — hit-tests that ran with NO panel geometry (`panel_info_nonblocking` refused because
/// `WRITER` was contended), so the cursor position was the (0,0) clamp `click_pointer_pos` gives and
/// the hit-test is not a statement about where the operator pointed. Counted, never silently folded
/// into the miss count.
#[cfg(feature = "orinclick")]
static CLK_NOGEOM: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-CLICK — per-edge lines emitted, and the ones a chattering button cost us. `CLK_LOG_MAX` is a
/// LIFETIME cap, not a rate: clicks are human-rate by construction, so hitting it at all means a
/// stuck switch or a decoder fault, and the census names the suppressed count rather than letting
/// the loss go unrecorded.
#[cfg(feature = "orinclick")]
static CLK_LOGGED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinclick")]
const CLK_LOG_MAX: u32 = 512;
/// ORIN-CLICK — census bookkeeping: the arm latch, the tick the last census printed at, its sequence
/// number, and the `CNTPCT_EL0` reading taken when the seam armed.
#[cfg(feature = "orinclick")]
static CLK_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "orinclick")]
static CLK_CENSUS_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "orinclick")]
static CLK_CENSUS_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinclick")]
static CLK_T0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// ORIN-CLICK — census cadence, in `jd2_console_pump` sweep ticks. The pump's sweep tick is
/// `CNTFRQ_EL0 / 4`, i.e. ~250 ms, so 40 ticks is ~10 s: slow enough to be free at 115200 baud, fast
/// enough that "the census stopped" localises a dead pump to within one line of the death.
#[cfg(feature = "orinclick")]
const CLK_CENSUS_PERIOD: u64 = 40;

/// ORIN-CLICK — `CNTPCT_EL0` and `CNTFRQ_EL0`, read on the calling core. The pump is a cooperative
/// EL1 task with no timer IRQ after the JM6 drop (the JD3 timerless mechanism), so the free-running
/// system counter is the only clock available to it — the same one `jd2_console_pump` already builds
/// its 8 s phase-1 deadline and its sweep cadence out of.
#[cfg(feature = "orinclick")]
fn clk_now_freq() -> (u64, u64) {
    let (now, freq): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, preserves_flags));
    }
    (now, freq)
}

/// ORIN-CLICK — the pointer position the ROUTER will use, read the same way the router's own private
/// `click_pointer_pos` reads it (`arch/aarch64/syscall.rs:13636`): panel geometry non-blocking, then
/// `pal::cursor::pos` clamped to it. Returns `None` for the geometry when `WRITER` was contended, so
/// the caller can say so on the wire instead of reporting a (0,0) clamp as if it were a hit-test.
#[cfg(feature = "orinclick")]
fn clk_pointer_pos() -> (Option<(i32, i32)>, i32, i32) {
    let geom = crate::video::panel_info_nonblocking().map(|i| (i.width as i32, i.height as i32));
    let (w, h) = geom.unwrap_or((0, 0));
    let (x, y) = crate::pal::cursor::pos(w, h);
    (geom, x, y)
}

/// **ORIN-CLICK — route ONE pointer button edge from the Orin console pump into the window layer.**
///
/// Called from `jd2_console_pump`'s `Event::Button` arm (`main.rs`), immediately after the JD20 log
/// line that arm has always emitted. That line is deliberately KEPT: it is the raw evidence that an
/// event reached the PUMP, which is a different claim from "the router made a decision", and the two
/// being separable is what lets a capture distinguish a decoder fault from a routing fault. (It is
/// also what makes the knob-off image byte-identical — removing it would move every line below it in
/// `main.rs`, and panic `Location` records embed line numbers.)
///
/// The whole routing decision is `wc_click_route`'s; this function adds no policy and takes no arm of
/// its own. It reads the state either side of the call and NAMES what happened:
///
/// | verdict | meaning |
/// | --- | --- |
/// | `RAISED` | press hit a window that did not hold focus, and focus moved to it — the rung's contract |
/// | `HIT-SAME` | press hit the already-focused window; nothing to move |
/// | `CONSUMED` | press was taken by a control or furniture arm (close/minimise/zoom, or `desktop_firmware` chrome) |
/// | `MISS-SHELL` | press hit no window while an app held focus; consumed, focus returned to the shell |
/// | `MISS-IDLE` | press hit no window and focus was already the shell; not consumed, nothing to do |
/// | `MISS-FULLSCREEN` | press hit no window but the focused app owns the panel through the compat row |
/// | `RELEASE-DROPPED` / `RELEASE-DELIVERED` | the release edge, following the press's target |
/// | `NO-EDGE` | the mask did not change — a re-report of a held button |
/// | `DECLINE reason=no-geometry` | `WRITER` was contended, so the cursor read is the (0,0) clamp and the hit-test says nothing |
/// | **`FAIL reason=no-raise`** | press hit an unfocused window, was not consumed, and focus DID NOT MOVE |
/// | **`FAIL reason=miss-unhandled`** | press hit nothing while an app held focus, and was neither consumed nor excused by a full-screen compat row |
///
/// **What reachable boot state makes this print FAIL?** `no-raise` fires whenever the router returns
/// `false` from a press on an unfocused window without having moved `USER_INPUT_ACTIVE` — which is
/// what a broken `user_input_set_active`, a `focus_changed` that declined the owner, or a widened
/// `#[cfg]` that compiled out the `owner != cur` arm would each produce. It is also the honest
/// reading of the ONE benign race this seam has: `hit_test` is asked here and again inside the
/// router under two separate acquisitions of `wm`'s `TABLE`, so a row closing between them would be
/// seen as a hit here and a miss there. On this branch nothing closes rows on the Orin (the only
/// minter is `orin_wm1`, which is idempotent and never closes), so that race is theoretical — and if
/// it ever fires, the router's own `[clickroute]` line is printed adjacent to this one and the two
/// disagreeing is exactly the evidence needed. `miss-unhandled` fires when the router's miss arm
/// neither consumes nor is excused, i.e. its `cur != 0` guard and `USER_INPUT_ACTIVE` have drifted
/// apart between the two reads.
///
/// **A note on where the keyboard goes, because it is a real consequence and not an oversight.** With
/// `desktop_firmware` OFF the SHELLWIN-PI arm (`is_kernel_owner` -> hand the keyboard to asid 0) is compiled
/// out, so a press on `orin_wm1`'s row — owner `wm::KERNEL_OWNER_DESKTOP` — takes the ordinary
/// `owner != cur` arm and leaves `USER_INPUT_ACTIVE` holding that kernel pseudo-ASID. On the Orin
/// that is INERT for the keyboard and it was verified rather than assumed: the only consumer of
/// `USER_INPUT_ACTIVE` for keystrokes is `pump_usb_into_gui`'s `user_input_active() != 0` branch
/// (`main.rs:3887`), which is `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]` and does
/// not exist on tegra; `jd2_console_pump` feeds every `Event::Key` straight through `handle_key`
/// regardless of focus. The focus either side of the call is printed on every line so the operator
/// can see the pseudo-ASID land rather than having to take this paragraph on trust. When rung 5 arms
/// `desktop_firmware`, the SHELLWIN-PI arm compiles in and takes over — no change is owed here.
#[cfg(feature = "orinclick")]
pub fn orin_click(mask: u8) {
    use core::sync::atomic::Ordering;
    use crate::arch::aarch64::syscall as sc;
    use crate::video::wm;

    CLK_BTN.fetch_add(1, Ordering::Relaxed);
    let prev = CLK_PREV_MASK.swap(mask as u32, Ordering::Relaxed) as u8;
    let (geom, x, y) = clk_pointer_pos();
    if geom.is_none() {
        CLK_NOGEOM.fetch_add(1, Ordering::Relaxed);
    }
    let pressed = mask & !prev;
    let released = prev & !mask;

    // The hit-test is asked BEFORE the call, because the router consumes the edge and the answer is
    // not recoverable afterwards. `hit_test` takes and releases `wm`'s TABLE; the router takes it
    // again. Sequential, never nested — nothing here holds a lock across the call.
    let target = if pressed != 0 { wm::hit_test(x, y) } else { None };
    let focus_before = sc::user_input_active();
    let consumed = sc::wc_click_route(crate::pal::Event::Button(mask));
    let focus_after = sc::user_input_active();

    let (edge, verdict) = if pressed != 0 {
        CLK_PRESS.fetch_add(1, Ordering::Relaxed);
        let v = if geom.is_none() {
            "DECLINE reason=no-geometry"
        } else {
            match target {
                Some((_win, owner, _z)) => {
                    if consumed {
                        CLK_CONSUMED.fetch_add(1, Ordering::Relaxed);
                        "CONSUMED"
                    } else if owner == focus_before {
                        CLK_SAME.fetch_add(1, Ordering::Relaxed);
                        "HIT-SAME"
                    } else if focus_after == owner {
                        CLK_RAISED.fetch_add(1, Ordering::Relaxed);
                        "RAISED"
                    } else {
                        CLK_STUCK.fetch_add(1, Ordering::Relaxed);
                        "FAIL reason=no-raise"
                    }
                }
                None => {
                    if consumed {
                        CLK_MISS.fetch_add(1, Ordering::Relaxed);
                        "MISS-SHELL"
                    } else if focus_before == 0 {
                        CLK_MISS.fetch_add(1, Ordering::Relaxed);
                        "MISS-IDLE"
                    } else if wm::compat_live() {
                        CLK_MISS.fetch_add(1, Ordering::Relaxed);
                        "MISS-FULLSCREEN"
                    } else {
                        CLK_STUCK.fetch_add(1, Ordering::Relaxed);
                        "FAIL reason=miss-unhandled"
                    }
                }
            }
        };
        ("press", v)
    } else if released != 0 {
        CLK_REL.fetch_add(1, Ordering::Relaxed);
        ("release", if consumed { "RELEASE-DROPPED" } else { "RELEASE-DELIVERED" })
    } else {
        CLK_NOEDGE.fetch_add(1, Ordering::Relaxed);
        ("none", "NO-EDGE")
    };

    if CLK_LOGGED.fetch_add(1, Ordering::Relaxed) < CLK_LOG_MAX {
        let (hit, win, owner) = match target {
            Some((w, o, _)) => ("yes", w as u64, o),
            None => ("no", u64::from(wm::WIN_NONE), 0),
        };
        serial_println!(
            "[orinclick] edge={} btn={:#04x} at ({},{}) geom={} hit={} win={} owner={:#x} focus {:#x}->{:#x} consumed={} -> {}",
            edge, mask, x, y, if geom.is_some() { "yes" } else { "REFUSED" },
            hit, win, owner, focus_before, focus_after, consumed as u8, verdict
        );
    } orin_drag_edge(); // DRAGDEAD (A27) — the RELEASE half of the gesture's report. `wc_click_route` above already called `wm::drag_end()` (arch/aarch64/syscall.rs:14485, at the TOP of the release arm), so the drag is over by the time control is back here and this is the first place that can say so. Sited AFTER the `[orinclick] edge=` line on purpose: a capture reads the router's verdict and then the gesture's outcome, in that order, and `[dragroute] end` is deliberately not folded INTO that line because it is a different claim on a different cadence (once per gesture, not once per edge). Costs one relaxed load per button edge when no drag is live. ⚠ SAME-LINE fold, line-NEUTRAL: this block is `orinclick`-gated but the FILE is not, so a line added here would renumber every panic `Location` below it in a knob-off jetson image.
}

/// **ORIN-CLICK — the ARM line and the periodic CENSUS, emitted from inside the pump's own drain loop.**
///
/// Called from `jd2_console_pump`'s phase-2 idle cadence (`main.rs`), on the same ~250 ms sweep tick
/// the VUGRAS writeback sweep rides. `tick` is that loop's own counter; this function decides how
/// often to print, so the CADENCE IS THE PUMP'S and a stalled pump cannot produce a census line.
///
/// FIRST call emits the ARM line and nothing else. Its verdict is derived from the panel and the `wm`
/// table as they actually are:
///
/// | verdict | meaning |
/// | --- | --- |
/// | `ARMED` | a panel, and at least one row in the `wm` table for a click to land on |
/// | `DECLINE reason=no-target` | the router is wired and the table is EMPTY — every press will take the miss arm. **This is the DEFAULT armed boot**: `orinclick` without `orindesk` mints no window, so nothing on this Orin is clickable and the instrument says so instead of reporting a healthy arm |
/// | `DECLINE reason=no-panel` | `WRITER` carries zero geometry (structurally unreachable from phase 2, which is only entered on a non-zero panel width — printed rather than assumed away) |
/// | `DECLINE reason=panel-locked` | `panel_info_nonblocking` refused; the geometry every hit-test clamps to is unknown this instant |
///
/// `rows=` is `wm::count()`, which counts every USED row including a COMPAT (full-screen) row that
/// `hit_test` deliberately skips, so `compat=` is printed beside it: `rows=1 compat=1` means the one
/// row is not hittable and `-> ARMED` would be over-claiming. This is stated rather than corrected
/// because narrowing the count would need a new accessor in `video/wm.rs`, a shared lane.
///
/// EVERY LATER call prints the census on the cadence, unconditionally — including, and especially,
/// when nothing has happened:
///
/// | verdict | meaning |
/// | --- | --- |
/// | **`FAIL reason=stuck-focus`** | at least one press hit an unfocused window and did not move the focus. Sticky for the rest of the boot |
/// | `IDLE-NO-CLICKS` | the pump is alive and NO button event has arrived. **UNRUN, not failed** — this is the line that makes a missing `[clickroute]` interpretable |
/// | `DECLINE reason=no-target` | button events arrived but the `wm` table is empty; there is nothing on this panel to click |
/// | `DECLINE reason=release-only` | button events arrived and not one of them was a press edge — a decoder emitting only the up half |
/// | `DECLINE reason=all-miss` | presses arrived, rows exist, and every single press landed off every window |
/// | `DECLINE reason=geometry-refused` | every button event this boot ran against a refused panel read |
/// | `ROUTING` | at least one press reached a window (raised, re-clicked or consumed) and nothing is stuck |
///
/// **What reachable boot state makes this print FAIL?** `stuck-focus` is the propagation of
/// `orin_click`'s `FAIL reason=no-raise`, so it fires for every state that one does. And the
/// DECLINEs are not decoration: `no-target` is what the default armed boot prints, on every census,
/// until `UNAOS_ORINDESK=1` puts a row on the panel.
///
/// **What the wire shows if this task DIES** — the hazard this census is shaped around. The lines
/// simply STOP. That is the honest answer and it is the only one available, because nothing in this
/// tree inherits a dead task's singleton roles: `steal_ok` is false for every explicitly-pinned task
/// and there is no re-home path anywhere in the source, so a dead `jd2_console_pump` means clicks
/// stop routing for the rest of the boot with no other subsystem noticing. The reason this instrument
/// can be trusted about that where the liveness instruments could not — pi's boot 11 printed
/// `[el0live] verdict=LIVE` one line after the synchronous exception that killed cpu 3, and
/// `:: SCHED: load ::` read `c3=100%` for the dead core — is that it is not an OBSERVER of the
/// routing task. It is the routing task, printing on its own core off its own counter. It cannot
/// report liveness it does not have.
#[cfg(feature = "orinclick")]
pub fn orin_click_census(tick: u64) {
    use core::sync::atomic::Ordering;
    use crate::arch::aarch64::syscall as sc;
    use crate::video::wm;

    // FOOTPRINT: this runs on the INPUT DRAIN LOOP, ~4x/s. `wm::count` and `wm::compat_live` each take
    // `wm`'s TABLE, and the one thing this seam must never do is add lock traffic to the path whose
    // death it exists to report. So the cadence gate is decided FIRST, off two system-register reads,
    // and the table is touched only on the ~1-in-40 pass that actually prints. Sequential acquisitions,
    // never nested, and never under `WRITER` — the WRITER/TABLE order stays acyclic (`orin_wm1`'s rule).
    let (now, freq) = clk_now_freq();
    let armed = CLK_ARMED.swap(true, Ordering::Relaxed);
    if armed && tick.wrapping_sub(CLK_CENSUS_TICK.load(Ordering::Relaxed)) < CLK_CENSUS_PERIOD {
        return;
    }
    let rows = wm::count();
    let compat = wm::compat_live();

    if !armed {
        CLK_T0.store(now, Ordering::Relaxed);
        CLK_CENSUS_TICK.store(tick, Ordering::Relaxed);
        let geom = crate::video::panel_info_nonblocking();
        let (pw, ph) = geom.map_or((0, 0), |i| (i.width, i.height));
        let verdict = if geom.is_none() {
            "DECLINE reason=panel-locked"
        } else if pw == 0 || ph == 0 {
            "DECLINE reason=no-panel"
        } else if rows == 0 {
            "DECLINE reason=no-target (wm table empty — arm UNAOS_ORINDESK=1 for a row to click)"
        } else {
            "ARMED"
        };
        serial_println!(
            "[orinclick] arm panel={}x{} rows={} compat={} focus={:#x} pidesk={} t={} -> {}",
            pw, ph, rows, compat as u8, sc::user_input_active(),
            cfg!(feature = "desktop_firmware") as u8, tick, verdict
        );
        return;
    }

    if tick.wrapping_sub(CLK_CENSUS_TICK.load(Ordering::Relaxed)) < CLK_CENSUS_PERIOD {
        return;
    }
    CLK_CENSUS_TICK.store(tick, Ordering::Relaxed); let ptrarms = ptrpoll_witness(tick); // CLICKDEAD — the `[ptrpoll]` line, on THIS census pass and no new cadence of its own. Returns the pointer-pipeline arm balance so the census line below can carry it as `reports=`. ⚠ LINE-NEUTRAL fold: the body lives at the FILE TAIL (nothing follows it), so no line in this file moves and the knob-off image's panic-`Location` byte-identity is untouched.
    let seq = CLK_CENSUS_SEQ.fetch_add(1, Ordering::Relaxed) + 1;

    let btn = CLK_BTN.load(Ordering::Relaxed);
    let press = CLK_PRESS.load(Ordering::Relaxed);
    let rel = CLK_REL.load(Ordering::Relaxed);
    let noedge = CLK_NOEDGE.load(Ordering::Relaxed);
    let raised = CLK_RAISED.load(Ordering::Relaxed);
    let same = CLK_SAME.load(Ordering::Relaxed);
    let miss = CLK_MISS.load(Ordering::Relaxed);
    let consumed = CLK_CONSUMED.load(Ordering::Relaxed);
    let stuck = CLK_STUCK.load(Ordering::Relaxed);
    let nogeom = CLK_NOGEOM.load(Ordering::Relaxed);
    let logged = CLK_LOGGED.load(Ordering::Relaxed);
    let dropped = logged.saturating_sub(CLK_LOG_MAX.min(logged));

    let verdict = if stuck != 0 {
        "FAIL reason=stuck-focus"
    } else if btn == 0 {
        "IDLE-NO-CLICKS"
    } else if nogeom >= btn {
        "DECLINE reason=geometry-refused"
    } else if rows == 0 {
        "DECLINE reason=no-target"
    } else if press == 0 {
        "DECLINE reason=release-only"
    } else if raised == 0 && same == 0 && consumed == 0 {
        "DECLINE reason=all-miss"
    } else {
        "ROUTING"
    };

    let up = if freq == 0 { 0 } else { now.wrapping_sub(CLK_T0.load(Ordering::Relaxed)) / freq };
    serial_println!(
        "[orinclick] census seq={} t={} up={}s btn={} press={} rel={} noedge={} raised={} same={} miss={} consumed={} stuck={} nogeom={} dropped={} rows={} compat={} focus={:#x} reports={} -> {}",
        seq, tick, up, btn, press, rel, noedge, raised, same, miss, consumed, stuck,
        nogeom, dropped, rows, compat as u8, sc::user_input_active(), ptrarms, verdict
    );
}

// =================================================================================================
// JD1-DC-MODEL — WHICH REGISTER MODEL DOES THIS SILICON PRESENT? `jd1dc`, DEFAULT OFF.
// =================================================================================================
//
// THE DEFECT THIS FIXES, AND IT IS A DEFECT IN THE INSTRUMENT, NOT IN THE CODE UNDER TEST.
// JD1-DC as it stood could print `VERDICT=DECODES-NOMATCH` for two reasons that point in OPPOSITE
// directions, and the wire could not tell them apart:
//
//   (a) the aperture is live, and the scanout is fed from a window this sweep did not cover; or
//   (b) the aperture is live, our reads are perfectly legal — and the register map we read it
//       through is Tegra194's, while this silicon is Tegra234.
//
// (a) says "sweep wider". (b) says "throw the map away". Flying a probe whose headline verdict is
// ambiguous between them spends an attended bench session on an unreadable answer. The reason to
// take (b) seriously: upstream Linux `drm/tegra` — the source `jd1_dc_survey`'s window offsets are
// derived from — has NO Tegra234 support at all. Its `of_match` tables end at `nvidia,tegra194-dc` /
// `-display` / `-sor`, and `tegra234.dtsi` carries no `display@13800000` node. NVIDIA's own T234
// display driver uses a different model entirely: NVDisplay class channels (`NVC67D`).
//
// WHAT THIS RUNG ADDS: FOUR MORE READ-ONLY READS, and one verdict line that decomposes the one
// above. Every read is `read_volatile`, every read is bounds-checked against the DTB-declared
// aperture length (never a constant), and every read sits INSIDE the BPMP `MRQ_PG GET_STATE` guard —
// this function is only ever called after that guard has passed and after the FIRST TOUCH read has
// already survived. NOT ONE REGISTER IS WRITTEN, here or anywhere in JD1-DC; the DCE R5 is live on
// the other side of this aperture and a write is a two-writer race against it.
//
// THE FOUR OFFSETS AND WHERE EACH COMES FROM. The hypothesis under test is that the Tegra234
// nvdisplay aperture is the standard NVIDIA `NV_PDISP` block rebased to offset 0, so that Tegra byte
// offset X == `NV_PDISP`-relative address X == open-gpu-doc "withoffset" address `0x610000 + X`.
// All four are quoted from NVIDIA/open-gpu-doc (MIT), `manuals/{volta/gv100,turing/tu102,
// ampere/ga102}/dev_display_withoffset.ref.txt` — the definitions are IDENTICAL in all three
// architectures, which is why a single wrong-generation guess cannot explain a match:
//
//   +0x00000  NV_PDISP_FE_CLASSES        0x00610000  CLASS_ID 31:16, CLASS_REV 15:8, API_REV 7:4,
//                                                    HW_REV 3:0                    <== DISCRIMINATOR
//   +0x00060  NV_PDISP_FE_HW_SYS_CAP     0x00610060  HEAD0..7_EXISTS 0:7, SOR0..7_EXISTS 8:15
//   +0x00064  NV_PDISP_FE_HW_SYS_CAPB    0x00610064  WINDOW0..31_EXISTS 0:31
//   +0x004E0  NV_PDISP_FE_CHNCTL_CORE    0x006104E0  ALLOCATION 0:0, CONNECTION 1:1,
//                                                    PUTPTR_WRITE 4:4, EFI 5:5, SKIP_NOTIF 9:9,
//                                                    IGNORE_INTERLOCK 11:11,
//                                                    ERRCHECK_WHEN_DISCONNECTED 12:12,
//                                                    TRASH_MODE 14:13, INTR_DURING_SHTDWN 15:15
//
// TWO CORRECTIONS TO THE PLAN THIS RUNG WAS WRITTEN FROM, both of which change what gets read:
//
//   1. `+0x30000` — the probe's existing FIRST TOUCH, which edk2-nvidia's `NvDisplayHw.c` calls
//      `DISPLAY_FE_SW_SYS_CAP` — is NOT documented under that name in open-gpu-doc, and a source
//      sweep of the three display manuals plus open-gpu-kernel-modules' `swref/published/disp/*`
//      found no `SW_SYS` symbol at all. What IS documented there is the APERTURE:
//      `NV_PDISP_FE_SW  0x00640FFF:0x00640000` (open-gpu-kernel-modules
//      `src/common/inc/swref/published/disp/v03_00/dev_disp.h`), i.e. NV_PDISP-relative 0x30000, a
//      4 KiB software-written mirror region. So the existing first read is a read of a real named
//      aperture at a register identity that rests on edk2 plus an inference from nouveau's
//      `gv100_disp_init()` (which masks `0x100 << i` at `0x640000` and `1 << i` at `0x640004` —
//      the HW_SYS_CAP / CAPB bit layouts exactly). `+0x60` and `+0x64` are the HARDWARE registers
//      the plan wanted the evidence of, primary-sourced, so they are read too and the two are
//      cross-checked against each other on the wire.
//   2. The core-channel class number is `NVC67D = 0x0000C67D` (`classes/display/clc67d.h`) but the
//      CLASS_ID FIELD does NOT hold `0xC67D`. The manuals' reset values are `0xC3700310` (gv100),
//      `0xC5700400` (tu102), `0xC6700410` (ga102) — CLASS_ID `0xC370`/`0xC570`/`0xC670`, matching
//      the "Software Class Number" column of `classes/display/README.txt`. The trailing `D` is the
//      core-channel nibble of the class NUMBER, not part of the ID in the register. A probe
//      comparing against `0xC67D` would never match and would report a live Ampere-class display
//      block as an unknown model. That is the exact class of confident-wrong-decode this rung is
//      supposed to prevent, so the table below uses `0xC?70` and says why.
//
// WHAT THIS RUNG DOES NOT KNOW, said here rather than implied by silence. That the Tegra234
// aperture is NV_PDISP rebased to offset 0 is a HYPOTHESIS. No NVIDIA source states it; no Tegra
// nvdisplay register header exists publicly; `tegra234.dtsi` has no display node to check against.
// Testing that hypothesis is the entire purpose of this rung, which is why a NEGATIVE result here
// is a real result and is given its own named verdict rather than being folded into "garbage".
//
// AND IT IS UNFLOWN. JD1-DC has never executed on hardware — three armed images have been staged and
// zero have been booted. Every sentence above about what a register WILL read is a prediction.

/// JD1-DC-MODEL: how many of this rung's reads actually happened, so the CENSUS line's `reads=`
/// count stays exact. A bounds refusal decrements nothing — it simply never increments.
#[cfg(feature = "jd1dc")]
static JD1DC_MODEL_READS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// JD1-DC-MODEL: one bounded, announced, read-only 32-bit MMIO read.
///
/// Carries the FIRST-TOUCH / READ-SURVIVED pair per read rather than once for the rung, which is
/// what makes SILENCE attributable: if the boot ends after `NEXT TOUCH … FE_CHNCTL_CORE` with no
/// matching `READ SURVIVED`, THAT read was the EL3-fatal one and the capture names it. A single
/// pair around all four reads would leave "which one killed it" unanswerable.
///
/// `None` = the offset lies outside the DTB-declared aperture and NOTHING was read. Reading past a
/// DTB-declared aperture is the JX1 class itself, so the bound is checked against the length the
/// device tree gave us, never against a hardcoded `0xEFFFF`.
#[cfg(feature = "jd1dc")]
#[inline(never)]
fn jd1_dc_model_read(base: u64, size: u64, off: u64, name: &str) -> Option<u32> {
    if off.saturating_add(4) > size {
        serial_println!(
            ":: tegra: JD1-DC-MODEL — {} @+{:#x} needs 4 bytes and the display@ node declares reg size={:#x}; that read would leave the DTB-declared aperture, so it was NOT performed and this register is UNKNOWN this boot ::",
            name,
            off,
            size,
        );
        return None;
    }
    serial_println!(
        ":: tegra: JD1-DC-MODEL — NEXT TOUCH: about to read {} @{:#x} (read-only; this rung issues no MMIO write of any kind, so an audit grep for the write intrinsic finds nothing but prose in this file). If this is the LAST line on the wire, THAT read was EL3-fatal (JX1 class) and the boot ended inside it ::",
        name,
        base + off,
    );
    let v = unsafe { core::ptr::read_volatile((base + off) as *const u32) };
    JD1DC_MODEL_READS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    serial_println!(
        ":: tegra: JD1-DC-MODEL — READ SURVIVED: {} @{:#x} = {:#010x} ::",
        name,
        base + off,
        v,
    );
    Some(v)
}

/// JD1-DC-MODEL: NVDisplay core-channel CLASS_ID -> the generation that presents it.
///
/// Quoted from NVIDIA/open-gpu-doc `classes/display/README.txt` ("Software Class Number" column)
/// and cross-checked against the `NV_PDISP_FE_CLASSES_0` reset values in the gv100 / tu102 / ga102
/// `dev_display_withoffset.ref.txt` manuals. NOTE the trailing `0`, not `D` — see the block comment.
///
/// `None` = not a class id this table knows. That is NOT the same as "not a display block": a
/// generation newer than this table (Blackwell's `0xCA70` is here, but its successor is not) would
/// land here too, which is why the caller distinguishes "class-SHAPED but unknown" from "not a
/// class id at all" instead of collapsing both into a failure.
#[cfg(feature = "jd1dc")]
fn jd1_dc_class_name(id: u32) -> Option<&'static str> {
    match id {
        0xC370 => Some("NVD_20 / class NVC37D — Volta gv100"),
        0xC570 => Some("NVD_30 / class NVC57D — Turing tu10x,tu11x"),
        0xC670 => Some("NVD_40 / class NVC67D — Ampere ga10x (the class NVIDIA's own Tegra234 display driver drives)"),
        0xC770 => Some("NVD_40 / class NVC77D — Ada ad10x"),
        0xCA70 => Some("NVD_50 / class NVCA7D — Blackwell gb20x"),
        _ => None,
    }
}

/// **JD1-DC-MODEL — the WHICH-CHIP discriminator.** Four read-only reads and exactly one
/// `MODEL-VERDICT=` line, on an axis ORTHOGONAL to `JD1-DC VERDICT=`.
///
/// The two axes answer different questions and must not be conflated: `VERDICT=` answers "does the
/// aperture decode, and did a window hold the scanout base"; `MODEL-VERDICT=` answers "and through
/// WHICH register map were we reading". `DECODES-NOMATCH` + `MODEL-VERDICT=NVDISPLAY-CLASS-C670`
/// means the sweep's offsets are wrong, not the aperture. `DECODES-NOMATCH` +
/// `MODEL-VERDICT=DECODES-NOT-NVDISPLAY` means the opposite: keep the offsets, widen the sweep.
///
/// The verdicts, and — the part that matters for an instrument — WHAT REACHABLE STATE MAKES EACH
/// PRINT:
///
/// * `MODEL-VERDICT=NVDISPLAY-CLASS-<id>` — `FE_CLASSES` CLASS_ID matched [`jd1_dc_class_name`].
///   Prints when the aperture is NV_PDISP rebased to offset 0 and the silicon is a generation this
///   table knows. Says WHICH.
/// * `MODEL-VERDICT=NVDISPLAY-CLASS-UNKNOWN-<hex>` — CLASS_ID is class-SHAPED (`0xC??0`) but not in
///   the table. Prints on a generation newer than this table. Still confirms the model.
/// * `MODEL-VERDICT=DECODES-NOT-NVDISPLAY` — at least one read returned a non-trivial value, so the
///   aperture answers, but `FE_CLASSES` holds nothing class-shaped in EITHER half-word. Prints when
///   offset 0 of this aperture is not `NV_PDISP_FE_CLASSES` — which is what the Tegra194 flat `DC_*`
///   model predicts, since its word 0x000 is `DC_CMD_GENERAL_INCR_SYNCPT`. This is the verdict that
///   SURVIVES the map we already have.
/// * `MODEL-VERDICT=NOT-DECODING` — every read this rung performed returned `0x00000000` or
///   `0xFFFFFFFF`. Prints when the aperture answers nothing distinguishable, and says so about the
///   ACCESS PATH, not about the silicon.
/// * `MODEL-VERDICT=REFUSED reason=no-reads` — every offset lay outside the DTB-declared aperture,
///   so nothing was read. Prints on a display node whose `reg` size is smaller than `0x4E4`.
///
/// Each of the four reads also carries its own sub-line with its own failing condition, named on the
/// wire, so a verdict cannot be reached without the evidence that produced it being legible.
#[cfg(feature = "jd1dc")]
fn jd1_dc_model(base: u64, size: u64, have_cap: bool, sw_cap: u32) {
    // Offsets: NV_PDISP-relative == open-gpu-doc "withoffset" minus 0x610000. See the block comment
    // for the quoted define of each.
    const OFF_FE_CLASSES: u64 = 0x0000_0000;
    const OFF_FE_HW_SYS_CAP: u64 = 0x0000_0060;
    const OFF_FE_HW_SYS_CAPB: u64 = 0x0000_0064;
    const OFF_FE_CHNCTL_CORE: u64 = 0x0000_04E0;
    // Every bit CHNCTL_CORE has a documented field for, in tu102/ga102 (gv100 adds SKIP_SEMA 10:10,
    // which is included so the mask is the UNION and cannot produce a false "undocumented bit").
    const CHNCTL_DOCUMENTED: u32 = 0xFE33;

    serial_println!(
        ":: tegra: JD1-DC-MODEL — the WHICH-REGISTER-MODEL discriminator: 4 read-only reads inside the same BPMP power guard, testing the hypothesis that this aperture is NVIDIA NV_PDISP rebased to offset 0 (Tegra byte offset X == open-gpu-doc withoffset 0x610000+X). If it is, the window offsets JD1-DC sweeps — Linux drm/tegra hub.c, whose of_match ENDS AT tegra194 — have no documented basis on this chip ::"
    );

    let classes = jd1_dc_model_read(base, size, OFF_FE_CLASSES, "NV_PDISP_FE_CLASSES (+0x00000)");
    let hw_cap = jd1_dc_model_read(base, size, OFF_FE_HW_SYS_CAP, "NV_PDISP_FE_HW_SYS_CAP (+0x00060)");
    let hw_capb = jd1_dc_model_read(base, size, OFF_FE_HW_SYS_CAPB, "NV_PDISP_FE_HW_SYS_CAPB (+0x00064)");
    let chnctl = jd1_dc_model_read(base, size, OFF_FE_CHNCTL_CORE, "NV_PDISP_FE_CHNCTL_CORE (+0x004e0)");

    let trivial = |v: u32| v == 0 || v == 0xFFFF_FFFF;

    // ---- FE_CLASSES: the discriminator, decoded field by field (all four fields DOCUMENTED) ----
    match classes {
        Some(v) if !trivial(v) => {
            let id = (v >> 16) & 0xFFFF;
            serial_println!(
                ":: tegra: JD1-DC-MODEL — FE_CLASSES={:#010x}: CLASS_ID(31:16)={:#06x} CLASS_REV(15:8)={:#04x} API_REV(7:4)={:#03x} HW_REV(3:0)={:#03x} -> {} | reference reset values: gv100 0xC3700310, tu102 0xC5700400, ga102 0xC6700410 ::",
                v,
                id,
                (v >> 8) & 0xFF,
                (v >> 4) & 0xF,
                v & 0xF,
                match jd1_dc_class_name(id) {
                    Some(n) => n,
                    None if (id & 0xF00F) == 0xC000 => "CLASS-SHAPED (0xC??0) but not in this table — a display generation newer than the sources this rung was built from; the NV_PDISP model still holds",
                    None => "NOT a class id in any shape this table recognises — offset 0 of this aperture is probably NOT NV_PDISP_FE_CLASSES",
                },
            );
        }
        Some(v) => serial_println!(
            ":: tegra: JD1-DC-MODEL — FE_CLASSES={:#010x} — trivial (all-zero or all-ones). A live NV_PDISP block cannot read 0 or ~0 here: CLASS_ID is a hardwired R--4R field with a nonzero reset value on every documented architecture. This is a statement about our ACCESS PATH, not about the silicon ::",
            v,
        ),
        None => serial_println!(
            ":: tegra: JD1-DC-MODEL — FE_CLASSES was NOT READ (outside the DTB-declared aperture); the model question is UNANSWERED this boot ::"
        ),
    }

    // ---- FE_HW_SYS_CAP / CAPB: the head / SOR / window census, and the sanity test on it ----
    match hw_cap {
        Some(v) if !trivial(v) => {
            let heads = v & 0xFF;
            let sors = (v >> 8) & 0xFF;
            serial_println!(
                ":: tegra: JD1-DC-MODEL — FE_HW_SYS_CAP={:#010x}: HEAD_EXISTS(0:7)={:#04x} ({} head(s)) SOR_EXISTS(8:15)={:#04x} ({} SOR(s)) upper(31:16)={:#06x}{}{} ::",
                v,
                heads,
                heads.count_ones(),
                sors,
                sors.count_ones(),
                (v >> 16) & 0xFFFF,
                if heads == 0 { " | FAILING: ZERO heads exist. A display block with no head is not a display block — this REFUTES the +0x60 == NV_PDISP_FE_HW_SYS_CAP mapping rather than reporting the hardware" } else { "" },
                if heads.count_ones() > 4 { " | IMPLAUSIBLE: more than 4 heads. Tegra234 documents exactly two head pixel clocks (TEGRA234_CLK_NVDISPLAY_P0/_P1, dt-bindings/clock/tegra234-clock.h) and no public source accounts for more than 4 heads on this SoC" } else { "" },
            );
        }
        Some(v) => serial_println!(
            ":: tegra: JD1-DC-MODEL — FE_HW_SYS_CAP={:#010x} — trivial. Like FE_CLASSES this is a hardwired capability word; 0 means 'no head and no SOR exists', ~0 means 'all eight of each', and neither is a possible Tegra234. Treat as an access-path finding ::",
            v,
        ),
        None => serial_println!(":: tegra: JD1-DC-MODEL — FE_HW_SYS_CAP was NOT READ (outside the DTB-declared aperture) ::"),
    }
    match hw_capb {
        Some(v) if !trivial(v) => serial_println!(
            ":: tegra: JD1-DC-MODEL — FE_HW_SYS_CAPB={:#010x}: WINDOW_EXISTS(0:31) -> {} window(s) exist. JD1-DC's sweep assumes SIX windows per head at 0xC00 stride inside a 0x10000 head bank; a count that is not a multiple of the swept 6, or a count of 0, means the sweep's window model does not describe this block ::",
            v,
            v.count_ones(),
        ),
        Some(v) => serial_println!(
            ":: tegra: JD1-DC-MODEL — FE_HW_SYS_CAPB={:#010x} — trivial. 0 = no window exists at all, in which case JD1-DC's ENTIRE window sweep is reading registers that do not exist; ~0 = all 32 exist, which no Tegra234 source accounts for ::",
            v,
        ),
        None => serial_println!(":: tegra: JD1-DC-MODEL — FE_HW_SYS_CAPB was NOT READ (outside the DTB-declared aperture) ::"),
    }
    // The two capability words are read from two different evidence classes — +0x60/+0x64 are
    // primary-sourced NVIDIA manual registers, +0x30000 is edk2's name for an offset inside the
    // documented NV_PDISP_FE_SW aperture — so making them agree (or not) on the wire is worth more
    // than either alone. Disagreement is not an error here; it is the finding.
    if have_cap {
        match hw_cap {
            Some(hw) => serial_println!(
                ":: tegra: JD1-DC-MODEL — CAP CROSS-CHECK: +0x30000 (edk2 'DISPLAY_FE_SW_SYS_CAP', inside the documented NV_PDISP_FE_SW 0x640FFF:0x640000 aperture) = {:#010x} -> heads={:#04x} sors={:#04x}; +0x00060 (NV_PDISP_FE_HW_SYS_CAP, open-gpu-doc) = {:#010x} -> heads={:#04x} sors={:#04x} -> {} ::",
                sw_cap, sw_cap & 0xFF, (sw_cap >> 8) & 0xFF,
                hw, hw & 0xFF, (hw >> 8) & 0xFF,
                if sw_cap == hw {
                    "AGREE — the software mirror holds what the hardware register holds, which corroborates BOTH the edk2 register identity at +0x30000 and the NV_PDISP-at-offset-0 hypothesis"
                } else if trivial(sw_cap) != trivial(hw) {
                    "DISAGREE, one of them trivially — only the NON-trivial one is evidence; the trivial one says its offset is not the register we named"
                } else {
                    "DISAGREE — both decode, neither mirrors the other. At most one of the two register identities is right, and this rung cannot say which. NOTE the SW register identity is the weaker of the two: open-gpu-doc documents the APERTURE at NV_PDISP-relative 0x30000 but no register inside it"
                },
            ),
            None => serial_println!(
                ":: tegra: JD1-DC-MODEL — CAP CROSS-CHECK not possible: +0x30000 read {:#010x} but +0x00060 was not read ::",
                sw_cap,
            ),
        }
    } else {
        serial_println!(
            ":: tegra: JD1-DC-MODEL — CAP CROSS-CHECK not possible: the aperture was too small for +0x30000, so JD1-DC's FIRST TOUCH was a window register and there is no software-mirror value to compare ::"
        );
    }

    // ---- FE_CHNCTL_CORE: what UEFI left behind on the core channel ----
    match chnctl {
        Some(v) if v != 0xFFFF_FFFF => {
            let undoc = v & !CHNCTL_DOCUMENTED;
            serial_println!(
                ":: tegra: JD1-DC-MODEL — FE_CHNCTL_CORE={:#010x}: ALLOCATION(0)={} CONNECTION(1)={} PUTPTR_WRITE(4)={} EFI(5)={} SKIP_NOTIF(9)={} IGNORE_INTERLOCK(11)={} ERRCHECK_WHEN_DISCONNECTED(12)={} TRASH_MODE(14:13)={} INTR_DURING_SHTDWN(15)={}{}{} ::",
                v,
                if v & 1 != 0 { "ALLOCATE" } else { "DEALLOCATE" },
                if v & 2 != 0 { "CONNECT" } else { "DISCONNECT" },
                (v >> 4) & 1,
                if (v >> 5) & 1 != 0 { "ENABLE — UEFI left the core channel flagged as EFI-owned" } else { "DISABLE" },
                (v >> 9) & 1,
                (v >> 11) & 1,
                (v >> 12) & 1,
                (v >> 13) & 3,
                (v >> 15) & 1,
                if v == 0 { " | ALL-ZERO, WHICH IS A LEGITIMATE VALUE AND NOT A DECODE FAILURE: every field's documented INIT is 0, so this reads as 'no core channel allocated, not connected, EFI off'. If the panel is lit while this reads zero, the scanout is NOT being driven through an allocated core channel from this register file — which is itself the finding" } else { "" },
                if undoc != 0 { " | SUSPECT: bits outside EVERY documented field of this register are set — see the mask in the block comment. Either this offset is not FE_CHNCTL_CORE, or this generation defines fields the sources this rung was built from do not" } else { "" },
            );
        }
        Some(v) => serial_println!(
            ":: tegra: JD1-DC-MODEL — FE_CHNCTL_CORE={:#010x} — all-ones. This register's documented fields occupy bits 0:15 only, so ~0 is not a decodable value; the offset is not answering ::",
            v,
        ),
        None => serial_println!(":: tegra: JD1-DC-MODEL — FE_CHNCTL_CORE was NOT READ (outside the DTB-declared aperture); what UEFI left on the core channel is UNKNOWN this boot ::"),
    }

    // ---- THE VERDICT: exactly one line, on the model axis ----
    let read_any = classes.is_some() || hw_cap.is_some() || hw_capb.is_some() || chnctl.is_some();
    let any_nontrivial = [classes, hw_cap, hw_capb, chnctl].iter().any(|o| matches!(o, Some(v) if !trivial(*v)));
    if !read_any {
        serial_println!(
            ":: tegra: JD1-DC-MODEL MODEL-VERDICT=REFUSED reason=no-reads — every one of the four offsets lay outside the display@ node's declared reg size ({:#x}); NOT ONE of this rung's registers was read and the register model is UNDETERMINED. This does NOT mean the aperture is dead — it means the DTB gave us less aperture than the smallest of these offsets needs ::",
            size,
        );
        return;
    }
    if !any_nontrivial {
        serial_println!(
            ":: tegra: JD1-DC-MODEL MODEL-VERDICT=NOT-DECODING — every read this rung performed returned 0x00000000 or 0xFFFFFFFF. WHAT THIS DOES NOT MEAN: it is not evidence that Tegra234's display block is unreachable in principle — NVIDIA's own UEFI drives this aperture from this core on this board by plain MmioRead32/MmioWrite32. It is a finding about OUR access path (aperture/mapping, the clock state at our probe point, or an SCR narrower for us than for UEFI), and it leaves the register-model question UNANSWERED rather than answering it negatively ::"
        );
        return;
    }
    // From here on the verdict is a statement ABOUT FE_CLASSES, so the two states in which we have no
    // FE_CLASSES value get their own verdicts rather than being folded into one that would claim
    // something about a register this boot did not read (or read as noise). Saying
    // "+0x00000 holds nothing class-shaped" about a word we never fetched is exactly the kind of
    // unearned claim this rung exists to stop.
    let Some(cls) = classes else {
        serial_println!(
            ":: tegra: JD1-DC-MODEL MODEL-VERDICT=UNDETERMINED reason=discriminator-not-read — the aperture ANSWERS (at least one of this rung's other reads returned a non-trivial value), but FE_CLASSES @+0x00000 was outside the display@ node's declared reg size ({:#x}) and was NEVER FETCHED. The register model is UNDETERMINED — not refuted. Whatever the head/SOR/window lines above showed, no claim about +0x00000 may be read off this boot ::",
            size,
        );
        return;
    };
    if trivial(cls) {
        serial_println!(
            ":: tegra: JD1-DC-MODEL MODEL-VERDICT=DISCRIMINATOR-TRIVIAL — the aperture ANSWERS somewhere (another read came back non-trivial) but FE_CLASSES @+0x00000 read {:#010x}. CLASS_ID is a hardwired read-only field with a nonzero reset value on every documented NVDisplay generation, so this REFUTES 'this aperture is NV_PDISP rebased to offset 0'. It does NOT refute much else, and specifically it is CONSISTENT with the flat Tegra DC_* model JD1-DC already uses, whose word 0x000 is DC_CMD_GENERAL_INCR_SYNCPT and may legitimately read zero on an idle block. Weaker evidence than DECODES-NOT-NVDISPLAY, and reported separately for that reason ::",
            cls,
        );
        return;
    }
    let id = (cls >> 16) & 0xFFFF;
    let shaped = (id & 0xF00F) == 0xC000 && id != 0;
    match jd1_dc_class_name(id) {
        Some(name) => serial_println!(
            ":: tegra: JD1-DC-MODEL MODEL-VERDICT=NVDISPLAY-CLASS-{:04X} — {}. The aperture at {:#x} IS NVIDIA NV_PDISP rebased to offset 0, and this silicon presents that class. CONSEQUENCE FOR THE VERDICT BELOW: JD1-DC's window offsets come from Linux drm/tegra hub.c, whose of_match ends at tegra194 and which describes a DIFFERENT register model — so a DECODES-NOMATCH below is expected, is NOT evidence against reachability, and the correct next step is to replace the window map with the NV_PDISP one (open-gpu-doc, MIT), not to sweep wider. WHAT THIS DOES NOT MEAN: it does not say the display is programmable from here, only that we now know which manual describes it ::",
            id,
            name,
            base,
        ),
        _ if shaped => serial_println!(
            ":: tegra: JD1-DC-MODEL MODEL-VERDICT=NVDISPLAY-CLASS-UNKNOWN-{:04X} — FE_CLASSES holds a CLASS-SHAPED id (0xC??0) that this rung's table does not name. The NV_PDISP-at-offset-0 model is CONFIRMED; the generation is newer than, or otherwise absent from, NVIDIA/open-gpu-doc's classes/display/README.txt as read for this rung. The raw word is on the FE_CLASSES line above and the decode of the id itself is UNKNOWN — deliberately not guessed ::",
            id,
        ),
        _ => serial_println!(
            ":: tegra: JD1-DC-MODEL MODEL-VERDICT=DECODES-NOT-NVDISPLAY — the aperture ANSWERS (at least one of this rung's reads returned a non-trivial value) but +0x00000 holds nothing class-shaped: CLASS_ID={:#06x} from FE_CLASSES={:#010x}. So this aperture is NOT NV_PDISP rebased to offset 0, and the surviving candidate is the model JD1-DC already uses — the flat Tegra DC_* layout, whose word 0x000 is DC_CMD_GENERAL_INCR_SYNCPT and would read exactly like this. WHAT THIS DOES NOT MEAN: it does not CONFIRM the T194 map — it removes the strongest reason to doubt it. Only a window START_ADDR matching the JD1 scanout base (VERDICT=REACHABLE below) confirms it ::",
            id,
            cls,
        ),
    }
}

// ══════════════════════════════════════════════════════════════════════════════════════════════
// JX2-NVC67D — THE REPLACEMENT FOR THE JX1-GATED WINDOW SWEEP, WRITTEN AGAINST THE REGISTER MODEL
// THE SILICON ACTUALLY PRESENTS.
//
// WHAT boot7e AND boot7f ESTABLISHED, and this rung starts from rather than re-asks:
//
//   FIRST READ SURVIVED: DISPLAY_FE_SW_SYS_CAP=0x00100303   <- the CCPLEX decodes this aperture
//   MODEL-VERDICT=NVDISPLAY-CLASS-C670   FE_CLASSES=0xc6700410 -> NVD_40 / class NVC67D, ga10x
//   FE_HW_SYS_CAP =0x00100303 -> 2 heads, 2 SORs
//   FE_HW_SYS_CAPB=0x0000000f -> 4 windows
//   FE_CHNCTL_CORE=0x00000021 -> ALLOCATION(0)=1, EFI(5)=1 — UEFI left the CORE CHANNEL EFI-OWNED
//
// So the aperture IS NVIDIA `NV_PDISP` rebased to offset 0, and it is the Ampere NVD_40 generation
// whose core channel is class `NVC67D`. That is exactly the map the JX1-gated sweep did NOT use:
// that sweep carried Tegra186/194's flat `DC_*` geometry (four head banks at 0x10000 stride, six
// windows per head at 0x2800 + 0xC00*win) and its very first read, `head+0x0 win0 WIN_OPTIONS`
// @+0x2e00, was EL3-FATAL (SError ESR 0xbe000011, BL31 "Unhandled Exception in EL3") even though
// +0x2e00 is INSIDE the DTB-declared `size=0xeffff`. NOT ONE T194 offset is resurrected here.
//
// WHY THIS IS NOT A SWEEP AT ALL, which is the substantive design change. On NVD_40 the per-window
// and per-head surface state is NOT MMIO. It is pushed through a CHANNEL as methods — `NVC67D_UPDATE`
// at method offset 0x00000200, `NVC67D_WINDOW_SET_CONTROL(a)` at 0x00001000 + a*0x80 with its
// `OWNER` field naming the head (NVIDIA/open-gpu-kernel-modules `clc67d.h`, MIT) — and the ARM/ASSY
// pair that a save/restore would want is read back THROUGH that channel's pushbuffer/PIO region, not
// out of the FE register block. There is no FE-level register that spells a window's START_ADDR.
// This rung therefore does NOT try: it reads the FE block's own CHANNEL STATE, which is the honest
// FE-level answer to "what did the firmware leave running, and is anything of ours allowed near it".
//
// AND IT OPENS NO CHANNEL. `FE_CHNCTL_CORE` bit 5 says the core channel is EFI-owned; taking it, or
// touching any CHNCTL bit, would be a two-writer race against the firmware that is currently feeding
// the panel this boot's console is on. NOT ONE REGISTER IS WRITTEN by this rung — every offset below
// is documented `R--4R`/`R--4A` (read-only) except the two `CHNCTL` words, which are read, not written.
//
// THE OFFSETS, all quoted from NVIDIA/open-gpu-doc (MIT), `manuals/ampere/ga102/
// dev_display_withoffset.ref.txt`, NV_PDISP-relative == "withoffset" address minus 0x610000:
//
//   +0x00018  NV_PDISP_FE_IP_VER              0x00610018  DEV 7:0, ECO 15:8, MINOR 23:16, MAJOR 31:24
//   +0x00068  NV_PDISP_FE_HW_LOCK_PIN_CAP     0x00610068  FLIP_LOCK_PINS 3:0, SCAN_LOCK_PINS 7:4,
//                                                         STEREO_PINS 11:8
//   +0x00074  NV_PDISP_FE_MISC_CONFIGA        0x00610074  NUM_HEADS 3:0, NUM_SORS 11:8,
//                                                         NUM_WINDOWS 25:20   <== SECOND CENSUS
//   +0x004E0  NV_PDISP_FE_CHNCTL_CORE         0x006104E0  (re-read; decoded by JD1-DC-MODEL)
//   +0x004E4  NV_PDISP_FE_CHNCTL_WIN(i)       0x006104E4+i*4   __SIZE_1 = 32
//   +0x00630  NV_PDISP_FE_CHNSTATUS_CORE      0x00610630  STG1_STATE 3:0, STG2_STATE 7:4,
//                                                         STATE 20:16, FIRSTTIME 24, METHOD_FIFO 25,
//                                                         READ_PENDING 26, NOTIF_WRITE_PENDING 27,
//                                                         SUBDEVICE_STATUS 29, QUIESCENT 30,
//                                                         METHOD_EXEC 31          <== THE RUNG'S POINT
//   +0x00664  NV_PDISP_FE_CHNSTATUS_WIN(i)    0x00610664+i*4   __SIZE_1 = 32
//   +0x006E4  NV_PDISP_FE_CHNSTATUS_WINIM(i)  0x006106E4+i*4   __SIZE_1 = 32
//   +0x00784  NV_PDISP_FE_CHNSTATUS_CURS(i)   0x00610784+i*4   __SIZE_1 = 8
//   +0x01C00  NV_PDISP_FE_RM_INTR_STAT_HEAD_TIMING(i)  0x00611C00+i*4  __SIZE_1 = 8
//   +0x01C24  NV_PDISP_FE_RM_INTR_STAT_EXC_WIN         0x00611C24
//   +0x01C28  NV_PDISP_FE_RM_INTR_STAT_EXC_WINIM       0x00611C28
//   +0x01C2C  NV_PDISP_FE_RM_INTR_STAT_EXC_OTHER       0x00611C2C
//   +0x01C30  NV_PDISP_FE_RM_INTR_STAT_CTRL_DISP       0x00611C30
//   +0x01C34  NV_PDISP_FE_RM_INTR_STAT_OR              0x00611C34
//
// READ ORDER IS A RISK ORDER, deliberately. `CHNSTATUS_CORE` and the three capability words share
// page 0x610000 with the four offsets boot7f already proved answer, so they go first and cheapest.
// The `RM_INTR_STAT` block at +0x01C00 is a DIFFERENT 4 KiB page that NOTHING has yet touched on
// this silicon, so it goes LAST: if that page is the one the fabric refuses, everything above it is
// already on the wire and the capture is still worth a boot. JX1 is the whole argument for this —
// in-aperture is NOT the same as decodable, and only ordering decides what survives being wrong.
//
// FIRST-TOUCH DISCIPLINE, KEPT VERBATIM FROM JX1. Every read announces the exact register name,
// array index and absolute address BEFORE the load and prints the value after. Silence between the
// two convicts that read by name, which is what turned JX1 into one boot instead of a bisect. The
// bound is always the DTB-declared `size`, never a hardcoded 0xEFFFF. A refused read prints its own
// `OUTOFAPERTURE` line and the decode below simply does not run — an absent decode line is never
// silently confused with a zero value.
// ══════════════════════════════════════════════════════════════════════════════════════════════

/// JX2-NVC67D: one bounded, announced, read-only 32-bit MMIO read.
///
/// `idx` is the array subscript for the `(i)`-indexed registers and `-1` for the scalar ones, so the
/// announce line names the exact element and not merely the family — `CHNSTATUS_WIN` is four
/// registers and "which one killed the boot" has to be answerable from the last line on the wire.
///
/// `None` = the offset lies outside the DTB-declared aperture and NOTHING was read.
#[cfg(feature = "jd1dc")]
#[inline(never)]
fn jx2_read(base: u64, size: u64, off: u64, name: &str, idx: i32) -> Option<u32> {
    if off.saturating_add(4) > size {
        serial_println!(
            ":: tegra: JX2-NVC67D OUTOFAPERTURE: {} index={} @+{:#x} needs 4 bytes and the display@ node declares reg size={:#x}; that read would leave the DTB-declared aperture, so it was NOT performed and this register is UNKNOWN this boot ::",
            name,
            idx,
            off,
            size,
        );
        return None;
    }
    serial_println!(
        ":: tegra: JX2-NVC67D NEXTTOUCH: about to read {} index={} @{:#x} (read-only; this rung issues no MMIO write of any kind and opens no display channel). If this is the LAST line on the wire, THAT read was EL3-fatal (JX1 class: SError ESR 0xbe000011, EC=0x2F, BL31 'Unhandled Exception in EL3') and the boot ended inside it ::",
        name,
        idx,
        base + off,
    );
    let v = unsafe { core::ptr::read_volatile((base + off) as *const u32) };
    serial_println!(
        ":: tegra: JX2-NVC67D READSURVIVED: {} index={} @{:#x} = {:#010x} ::",
        name,
        idx,
        base + off,
        v,
    );
    Some(v)
}

/// JX2-NVC67D: `NV_PDISP_FE_CHNSTATUS_CORE_STATE` (20:16). Enum quoted verbatim from
/// NVIDIA/open-gpu-doc `manuals/ampere/ga102/dev_display_withoffset.ref.txt`.
#[cfg(feature = "jd1dc")]
fn jx2_core_state(s: u32) -> &'static str {
    match s {
        0x00 => "DEALLOC",
        0x01 => "DEALLOC_LIMBO",
        0x02 => "VBIOS_INIT1",
        0x03 => "VBIOS_INIT2",
        0x04 => "VBIOS_OPERATION",
        0x05 => "EFI_INIT1",
        0x06 => "EFI_INIT2",
        0x07 => "EFI_OPERATION",
        0x08 => "UNCONNECTED",
        0x09 => "INIT1",
        0x0A => "INIT2",
        0x0B => "IDLE",
        0x0C => "BUSY",
        0x0D => "SHUTDOWN1",
        0x0E => "SHUTDOWN2",
        _ => "NOT-A-DOCUMENTED-STATE (ga102 defines 0x0..0xE only; a value above that is evidence about the offset, not about the channel)",
    }
}

/// JX2-NVC67D: `NV_PDISP_FE_CHNSTATUS_WIN_STATE` / `..._WINIM_STATE` (19:16), same enum for both.
#[cfg(feature = "jd1dc")]
fn jx2_win_state(s: u32) -> &'static str {
    match s {
        0x0 => "DEALLOC",
        0x1 => "UNCONNECTED",
        0x2 => "INIT1",
        0x3 => "INIT2",
        0x4 => "IDLE",
        0x5 => "BUSY",
        0x6 => "SHUTDOWN1",
        0x7 => "SHUTDOWN2",
        _ => "NOT-A-DOCUMENTED-STATE",
    }
}

/// JX2-NVC67D: `NV_PDISP_FE_CHNSTATUS_WIN_UPD_STATE` (11:8) — where an in-flight `UPDATE` is parked.
#[cfg(feature = "jd1dc")]
fn jx2_upd_state(s: u32) -> &'static str {
    match s {
        0x0 => "INIT",
        0x1 => "IDLE",
        0x2 => "WAIT_BLOCK",
        0x3 => "WAIT_MPI",
        0x4 => "WAIT_ILK_PH_1",
        0x5 => "WAIT_STATE_ERRCHK",
        0x6 => "WAIT_RDY_TO_FLIP",
        0x7 => "WAIT_ILK_PH_2",
        0x8 => "CHECK_PEND_LOADV",
        0x9 => "SEND_UPD",
        0xA => "WAIT_PRM",
        0xB => "EXCEPTION",
        0xC => "WAIT_ILK_ABORT",
        _ => "NOT-A-DOCUMENTED-STATE",
    }
}

/// JX2-NVC67D: the NVC67D-correct read-only channel-state probe.
///
/// Called from the same place, and under the same BPMP `MRQ_PG GET_STATE` power guard, as
/// [`jd1_dc_model`] — i.e. only after the display@ power domains are known ON and only after the
/// aperture's first read is known non-fatal. Supersedes the JX1-gated Tegra194 window sweep, which
/// is left in place above (gated to an empty slice) because it is the record of why this exists.
///
/// Emits exactly one `JX2-VERDICT=` line, on an axis ORTHOGONAL to both `JD1-DC VERDICT=`
/// (reachability) and `MODEL-VERDICT=` (which register map). This one answers: **what state did the
/// firmware leave the display channels in, and does the FE block agree that a scanout is running.**
///
/// * `JX2-VERDICT=REFUSED` — the DTB aperture held none of this rung's offsets; nothing was read.
/// * `JX2-VERDICT=NOT-DECODING` — every read returned `0x00000000` or `0xFFFFFFFF`. A finding about
///   our access path, NOT about the silicon: boot7f read four non-trivial words out of this aperture.
/// * `JX2-VERDICT=CORE-NOT-READ` — other reads answered but `CHNSTATUS_CORE` itself was refused.
/// * `JX2-VERDICT=EFI-OWNED-LIVE` — core channel `STATE` is one of `EFI_INIT1`/`EFI_INIT2`/
///   `EFI_OPERATION`, which corroborates `CHNCTL_CORE` bit 5 from the far side of the block: the
///   firmware still owns the scanout and any takeover must displace it deliberately, not by accident.
/// * `JX2-VERDICT=NOT-EFI-OWNED` — `STATE` is something else, named in the line. This is the case
///   that would change the display arc's plan, so it is reported separately rather than folded in.
#[cfg(feature = "jd1dc")]
fn jx2_nvc67d_status(base: u64, size: u64) {
    const OFF_IP_VER: u64 = 0x0000_0018;
    const OFF_HW_LOCK_PIN_CAP: u64 = 0x0000_0068;
    const OFF_MISC_CONFIGA: u64 = 0x0000_0074;
    const OFF_CHNCTL_CORE: u64 = 0x0000_04E0;
    const OFF_CHNCTL_WIN: u64 = 0x0000_04E4;
    const OFF_CHNSTATUS_CORE: u64 = 0x0000_0630;
    const OFF_CHNSTATUS_WIN: u64 = 0x0000_0664;
    const OFF_CHNSTATUS_WINIM: u64 = 0x0000_06E4;
    const OFF_CHNSTATUS_CURS: u64 = 0x0000_0784;
    const OFF_INTR_HEAD_TIMING: u64 = 0x0000_1C00;
    // Every bit CHNCTL_WIN has a documented field for on ga102 — ALLOCATION 0, CONNECTION 1,
    // IN_ORDER 2, PUTPTR_WRITE 4, SKIP_SYNCPOINT 6, IGNORE_TIMESTAMP 7, IGNORE_PI 8, SKIP_NOTIF 9,
    // SKIP_SEMA 10, IGNORE_INTERLOCK 11, TRASH_MODE 14:13.
    const CHNCTL_WIN_DOCUMENTED: u32 = 0x6FD7;
    // `__SIZE_1` caps from the manual, so a corrupt census can never walk this rung off the end of
    // the register file: WIN/WINIM are 32 deep, CURS and HEAD_TIMING are 8 deep. The extra clamp to
    // 8 windows is ours — this part has four, and a census claiming 32 is itself the finding.
    const MAX_WIN: u32 = 8;
    const MAX_HEAD: u32 = 8;

    serial_println!(
        ":: tegra: JX2-NVC67D RUNG-BEGIN — read-only NVC67D channel-state probe, superseding the JX1-gated Tegra194 window sweep. boot7f settled the map (FE_CLASSES=0xc6700410 -> NVD_40 / class NVC67D, Ampere ga10x), so these offsets come from NVIDIA/open-gpu-doc manuals/ampere/ga102/dev_display_withoffset.ref.txt, NOT from Linux drm/tegra hub.c. NVD_40 window surface state is CHANNEL state (NVC67D_UPDATE @ method 0x200, clc67d.h), not MMIO, so there is nothing here to sweep — what the FE block CAN answer is which channels the firmware left alive, and that is all this rung asks. NOT ONE REGISTER IS WRITTEN and NO CHANNEL IS OPENED: FE_CHNCTL_CORE bit 5 says the core channel is EFI-owned and it is feeding the panel this console is on ::"
    );

    let mut reads = 0u32;
    let mut nontrivial = 0u32;
    let mut rd = |off: u64, name: &str, idx: i32| -> Option<u32> {
        let r = jx2_read(base, size, off, name, idx);
        if let Some(v) = r {
            reads += 1;
            if v != 0 && v != 0xFFFF_FFFF {
                nontrivial += 1;
            }
        }
        r
    };

    // ---- 1. THE NAMED NEXT READ. Cheapest, most decisive, same page as everything boot7f proved. --
    let core_st = rd(OFF_CHNSTATUS_CORE, "NV_PDISP_FE_CHNSTATUS_CORE (+0x00630)", -1);
    if let Some(v) = core_st {
        serial_println!(
            ":: tegra: JX2-NVC67D CORECHN={:#010x}: STATE(20:16)={:#04x} -> {} | STG1_STATE(3:0)={:#03x} STG2_STATE(7:4)={:#03x} FIRSTTIME(24)={} METHOD_FIFO(25)={} READ_PENDING(26)={} NOTIF_WRITE_PENDING(27)={} SUBDEVICE_STATUS(29)={} QUIESCENT(30)={} METHOD_EXEC(31)={} ::",
            v,
            (v >> 16) & 0x1F,
            jx2_core_state((v >> 16) & 0x1F),
            v & 0xF,
            (v >> 4) & 0xF,
            if (v >> 24) & 1 == 1 { "YES" } else { "NO" },
            if (v >> 25) & 1 == 1 { "NOTEMPTY" } else { "EMPTY" },
            if (v >> 26) & 1 == 1 { "YES" } else { "NO" },
            if (v >> 27) & 1 == 1 { "YES" } else { "NO" },
            if (v >> 29) & 1 == 1 { "ACTIVE" } else { "INACTIVE" },
            if (v >> 30) & 1 == 1 { "YES" } else { "NO" },
            if (v >> 31) & 1 == 1 { "RUNNING" } else { "IDLE" },
        );
    }

    // ---- 2. CHNCTL_CORE, RE-READ. JD1-DC-MODEL already decoded it field by field a few lines
    //         earlier; the value is repeated here for ONE reason — it is the pair to the CHNSTATUS
    //         word above, and a disagreement between "EFI owns it" (CHNCTL bit 5) and "the channel
    //         is not in an EFI state" (CHNSTATUS STATE) is the single most informative thing this
    //         rung could find. Re-reading also shows whether the value MOVED across the probe.
    let core_ctl = rd(OFF_CHNCTL_CORE, "NV_PDISP_FE_CHNCTL_CORE (+0x004e0, re-read)", -1);
    if let Some(v) = core_ctl {
        serial_println!(
            ":: tegra: JX2-NVC67D CORECTL={:#010x}: ALLOCATION(0)={} CONNECTION(1)={} PUTPTR_WRITE(4)={} EFI(5)={} — boot7f measured 0x00000021 here; a DIFFERENT value now means the firmware moved the core channel between JD1-DC-MODEL and this rung, which no read of ours can cause ::",
            v,
            v & 1,
            (v >> 1) & 1,
            (v >> 4) & 1,
            (v >> 5) & 1,
        );
    }

    // ---- 3. THE SECOND CENSUS. FE_MISC_CONFIGA counts heads/SORs/windows independently of
    //         FE_HW_SYS_CAP/CAPB, which JD1-DC-MODEL read from the other side of the block. Two
    //         hardwired census words that DISAGREE would refute the offset, not the hardware — which
    //         is why the disagreement is called out on the wire instead of one being trusted.
    let ip_ver = rd(OFF_IP_VER, "NV_PDISP_FE_IP_VER (+0x00018)", -1);
    if let Some(v) = ip_ver {
        serial_println!(
            ":: tegra: JX2-NVC67D IPVER={:#010x}: MAJOR(31:24)={} MINOR(23:16)={} ECO(15:8)={} DEV(7:0)={} — the NVDisplay IP revision, independent of FE_CLASSES; a MAJOR of 4 corroborates NVD_40 from a second hardwired word ::",
            v,
            (v >> 24) & 0xFF,
            (v >> 16) & 0xFF,
            (v >> 8) & 0xFF,
            v & 0xFF,
        );
    }
    let cfga = rd(OFF_MISC_CONFIGA, "NV_PDISP_FE_MISC_CONFIGA (+0x00074)", -1);
    let (n_win, n_head, census_src) = match cfga {
        Some(v) if v != 0 && v != 0xFFFF_FFFF => {
            let nh = (v & 0xF).min(MAX_HEAD);
            let ns = (v >> 8) & 0xF;
            let nw = ((v >> 20) & 0x3F).min(MAX_WIN);
            serial_println!(
                ":: tegra: JX2-NVC67D CENSUS={:#010x}: NUM_HEADS(3:0)={} NUM_SORS(11:8)={} NUM_WINDOWS(25:20)={} — boot7f's OTHER census, FE_HW_SYS_CAP=0x00100303 / FE_HW_SYS_CAPB=0x0000000f, said 2 heads / 2 SORs / 4 windows. These two words are hardwired and must agree; if they do not, ONE of the two offsets is not the register this rung thinks it is, and no head or window count read off this boot may be trusted ::",
                v,
                v & 0xF,
                ns,
                (v >> 20) & 0x3F,
            );
            (
                if nw == 0 { 4 } else { nw },
                if nh == 0 { 2 } else { nh },
                "FE_MISC_CONFIGA (this boot)",
            )
        }
        _ => (
            4,
            2,
            "boot7f fallback (FE_HW_SYS_CAP=0x00100303, FE_HW_SYS_CAPB=0x0000000f) — FE_MISC_CONFIGA was refused or trivial this boot",
        ),
    };
    let lockpin = rd(OFF_HW_LOCK_PIN_CAP, "NV_PDISP_FE_HW_LOCK_PIN_CAP (+0x00068)", -1);
    if let Some(v) = lockpin {
        serial_println!(
            ":: tegra: JX2-NVC67D LOCKPIN={:#010x}: FLIP_LOCK_PINS(3:0)={} SCAN_LOCK_PINS(7:4)={} STEREO_PINS(11:8)={} — the raster-lock pin census. Read here only because it is another hardwired capability word on a page already proven to answer, so a trivial value is a cheap independent check on the access path ::",
            v,
            v & 0xF,
            (v >> 4) & 0xF,
            (v >> 8) & 0xF,
        );
    }

    serial_println!(
        ":: tegra: JX2-NVC67D WALK — {} window channel(s) and {} head(s); source of those counts: {}. Manual caps: CHNSTATUS_WIN/WINIM __SIZE_1=32, CHNSTATUS_CURS and RM_INTR_STAT_HEAD_TIMING __SIZE_1=8, and this rung clamps to 8 of each on top ::",
        n_win,
        n_head,
        census_src,
    );

    // ---- 4. THE WINDOW CHANNELS. This is the T194 sweep's question asked in the right register
    //         model: not "what surface is window N showing" (that is channel state, unreachable from
    //         here without taking a channel we must not take) but "does window channel N exist, is it
    //         allocated, and is its state machine parked or running".
    for w in 0..n_win as u64 {
        let ctl = rd(
            OFF_CHNCTL_WIN + w * 4,
            "NV_PDISP_FE_CHNCTL_WIN(i) (+0x004e4 + i*4)",
            w as i32,
        );
        if let Some(v) = ctl {
            let undoc = v & !CHNCTL_WIN_DOCUMENTED;
            serial_println!(
                ":: tegra: JX2-NVC67D WINCTL={:#010x} win={}: ALLOCATION(0)={} CONNECTION(1)={} IN_ORDER(2)={} PUTPTR_WRITE(4)={} SKIP_SYNCPOINT(6)={} IGNORE_TIMESTAMP(7)={} IGNORE_PI(8)={} SKIP_NOTIF(9)={} SKIP_SEMA(10)={} IGNORE_INTERLOCK(11)={} TRASH_MODE(14:13)={} undocumented-bits={:#010x}{} — NOTE there is NO owner field here: on NVC67D a window's head is set by the CORE channel method NVC67D_WINDOW_SET_CONTROL(a)_OWNER (clc67d.h), so window-to-head binding is NOT readable from the FE block and this rung does not pretend to report it ::",
                v,
                w,
                v & 1,
                (v >> 1) & 1,
                (v >> 2) & 1,
                (v >> 4) & 1,
                (v >> 6) & 1,
                (v >> 7) & 1,
                (v >> 8) & 1,
                (v >> 9) & 1,
                (v >> 10) & 1,
                (v >> 11) & 1,
                (v >> 13) & 3,
                undoc,
                if undoc != 0 { " | SUSPECT: bits outside every documented ga102 field are set — either this offset is not CHNCTL_WIN, or NVD_40 defines fields the ga102 manual does not" } else { "" },
            );
        }
        let st = rd(
            OFF_CHNSTATUS_WIN + w * 4,
            "NV_PDISP_FE_CHNSTATUS_WIN(i) (+0x00664 + i*4)",
            w as i32,
        );
        if let Some(v) = st {
            serial_println!(
                ":: tegra: JX2-NVC67D WINCHN={:#010x} win={}: STATE(19:16)={:#03x} -> {} | UPD_STATE(11:8)={:#03x} -> {} | STG1(3:0)={:#03x} STG2(7:4)={:#03x} FIRSTTIME(24)={} METHOD_FIFO(25)={} READ_PENDING(26)={} WRITE_PENDING(27)={} SUBDEVICE_STATUS(29)={} QUIESCENT(30)={} METHOD_EXEC(31)={} ::",
                v,
                w,
                (v >> 16) & 0xF,
                jx2_win_state((v >> 16) & 0xF),
                (v >> 8) & 0xF,
                jx2_upd_state((v >> 8) & 0xF),
                v & 0xF,
                (v >> 4) & 0xF,
                if (v >> 24) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 25) & 1 == 1 { "NOTEMPTY" } else { "EMPTY" },
                if (v >> 26) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 27) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 29) & 1 == 1 { "ACTIVE" } else { "INACTIVE" },
                if (v >> 30) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 31) & 1 == 1 { "RUNNING" } else { "IDLE" },
            );
        }
        let im = rd(
            OFF_CHNSTATUS_WINIM + w * 4,
            "NV_PDISP_FE_CHNSTATUS_WINIM(i) (+0x006e4 + i*4)",
            w as i32,
        );
        if let Some(v) = im {
            serial_println!(
                ":: tegra: JX2-NVC67D WINIMCHN={:#010x} win={}: STATE(19:16)={:#03x} -> {} | MP_STATE(3:0)={:#03x} FIRSTTIME(24)={} METHOD_FIFO(25)={} READ_PENDING(26)={} WRITE_PENDING(27)={} SUBDEVICE_STATUS(29)={} QUIESCENT(30)={} METHOD_EXEC(31)={} — the window IMMEDIATE channel, the low-latency sibling of the window channel above; same STATE enum ::",
                v,
                w,
                (v >> 16) & 0xF,
                jx2_win_state((v >> 16) & 0xF),
                v & 0xF,
                if (v >> 24) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 25) & 1 == 1 { "NOTEMPTY" } else { "EMPTY" },
                if (v >> 26) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 27) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 29) & 1 == 1 { "ACTIVE" } else { "INACTIVE" },
                if (v >> 30) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 31) & 1 == 1 { "RUNNING" } else { "IDLE" },
            );
        }
    }

    // ---- 5. THE CURSOR CHANNELS, one per head. Narrower field set than WIN: STATE is 18:16 here,
    //         three bits, not four — the manual's own layout, not a typo carried over.
    for h in 0..n_head as u64 {
        let cu = rd(
            OFF_CHNSTATUS_CURS + h * 4,
            "NV_PDISP_FE_CHNSTATUS_CURS(i) (+0x00784 + i*4)",
            h as i32,
        );
        if let Some(v) = cu {
            serial_println!(
                ":: tegra: JX2-NVC67D CURSCHN={:#010x} head={}: STATE(18:16)={:#03x} -> {} | MP_STATE(3:0)={:#03x} FIRSTTIME(24)={} METHOD_EXEC(31)={} — ga102 documents only DEALLOC(0), INIT1(2), IDLE(4) and BUSY(5) for this field, so a value outside that set is named UNDOCUMENTED rather than guessed ::",
                v,
                h,
                (v >> 16) & 7,
                match (v >> 16) & 7 {
                    0 => "DEALLOC",
                    2 => "INIT1",
                    4 => "IDLE",
                    5 => "BUSY",
                    _ => "NOT-A-DOCUMENTED-STATE",
                },
                v & 0xF,
                if (v >> 24) & 1 == 1 { "YES" } else { "NO" },
                if (v >> 31) & 1 == 1 { "RUNNING" } else { "IDLE" },
            );
        }
    }

    // ---- 6. LAST, AND ON PURPOSE: the +0x01C00 interrupt-status page. A DIFFERENT 4 KiB page from
    //         every offset above and from every offset boot7f proved, so it carries the JX1 risk that
    //         "inside the DTB aperture" does not retire. Everything of value is already on the wire
    //         by the time this runs. HEAD_TIMING is the prize: LOADV/LAST_DATA/VBLANK latched on a
    //         head is the FE block agreeing that a raster is actually running on it.
    for h in 0..n_head as u64 {
        let t = rd(
            OFF_INTR_HEAD_TIMING + h * 4,
            "NV_PDISP_FE_RM_INTR_STAT_HEAD_TIMING(i) (+0x01c00 + i*4, FIRST TOUCH OF THE 0x611xxx PAGE)",
            h as i32,
        );
        if let Some(v) = t {
            serial_println!(
                ":: tegra: JX2-NVC67D HEADTIMING={:#010x} head={}: LOADV(0)={} LAST_DATA(1)={} VBLANK(2)={} VACTIVE_SPACE_VBLANK(3)={} RG_STALL(4)={} RG_LINE_A(5)={} RG_LINE_B(6)={} SEC_POLICY(8)={} — these are LATCHED status bits, read-only (R--4R) and NOT cleared by this rung. A head with VBLANK or LAST_DATA latched is a head the raster generator has been running on since the last clear, which corroborates the inherited scanout from the display block rather than from the framebuffer ::",
                v,
                h,
                v & 1,
                (v >> 1) & 1,
                (v >> 2) & 1,
                (v >> 3) & 1,
                (v >> 4) & 1,
                (v >> 5) & 1,
                (v >> 6) & 1,
                (v >> 8) & 1,
            );
        }
    }
    for &(off, full, short) in &[
        (
            0x0000_1C24u64,
            "NV_PDISP_FE_RM_INTR_STAT_EXC_WIN (+0x01c24)",
            "EXC_WIN (per-window-channel exception, bit i = window channel i)",
        ),
        (
            0x0000_1C28u64,
            "NV_PDISP_FE_RM_INTR_STAT_EXC_WINIM (+0x01c28)",
            "EXC_WINIM (per-window-immediate-channel exception)",
        ),
        (
            0x0000_1C2Cu64,
            "NV_PDISP_FE_RM_INTR_STAT_EXC_OTHER (+0x01c2c)",
            "EXC_OTHER (core and cursor channel exceptions)",
        ),
        (
            0x0000_1C30u64,
            "NV_PDISP_FE_RM_INTR_STAT_CTRL_DISP (+0x01c30)",
            "CTRL_DISP (display controller level events)",
        ),
        (
            0x0000_1C34u64,
            "NV_PDISP_FE_RM_INTR_STAT_OR (+0x01c34)",
            "OR (output resource / SOR level events)",
        ),
    ] {
        if let Some(v) = rd(off, full, -1) {
            serial_println!(
                ":: tegra: JX2-NVC67D INTRSTAT={:#010x} reg={} — raw, undecoded per bit on purpose: what matters at this rung is whether ANY exception is latched on a channel the firmware owns. Nonzero here alongside a healthy CHNSTATUS means something already went wrong on that channel before we arrived, and it was not us ::",
                v,
                short,
            );
        }
    }

    // ---- 7. THE VERDICT: exactly one line, on the channel-state axis ----
    if reads == 0 {
        serial_println!(
            ":: tegra: JX2-NVC67D JX2-VERDICT=REFUSED reason=no-reads — every offset this rung wanted lay outside the display@ node's declared reg size ({:#x}); NOT ONE register was read and the channel state is UNDETERMINED this boot. This does NOT mean the block is dead — it means the DTB gave us less aperture than the smallest of these offsets needs ::",
            size,
        );
        return;
    }
    if nontrivial == 0 {
        serial_println!(
            ":: tegra: JX2-NVC67D JX2-VERDICT=NOT-DECODING — all {} read(s) this rung performed returned 0x00000000 or 0xFFFFFFFF. WHAT THIS DOES NOT MEAN: boot7f read four non-trivial words out of this same aperture in the same boot flow, so this is not evidence that the block is unreachable. It is a finding about OUR access path at THIS point in the boot — and note that an all-zero CHNSTATUS set is ALSO the legitimate reading of 'every channel is in DEALLOC or INIT', which is why this verdict refuses to choose between the two ::",
            reads,
        );
        return;
    }
    let Some(cs) = core_st else {
        serial_println!(
            ":: tegra: JX2-NVC67D JX2-VERDICT=CORE-NOT-READ — the aperture ANSWERS ({} of {} reads came back non-trivial) but NV_PDISP_FE_CHNSTATUS_CORE @+0x00630 was outside the declared reg size ({:#x}) and was NEVER FETCHED. Whatever the window and head lines above showed, no claim about who owns the core channel may be read off this boot ::",
            nontrivial,
            reads,
            size,
        );
        return;
    };
    let state = (cs >> 16) & 0x1F;
    if matches!(state, 0x05 | 0x06 | 0x07) {
        serial_println!(
            ":: tegra: JX2-NVC67D JX2-VERDICT=EFI-OWNED-LIVE — CHNSTATUS_CORE={:#010x}, STATE={:#04x} -> {}. The core channel is in a firmware-owned state, which corroborates FE_CHNCTL_CORE bit 5 (EFI=1) from the opposite side of the block: two independent registers agreeing that the firmware still drives this display. CONSEQUENCE: the inherited scanout JD1 is drawing into is presented by a channel WE DO NOT OWN, so the next display rung is a deliberate handoff — allocate our own window channel and bind it with NVC67D_WINDOW_SET_CONTROL_OWNER through a core channel we have taken — never an opportunistic MMIO poke. WHAT THIS DOES NOT MEAN: it does not say the handoff will work, only that nothing is currently free for the taking ::",
            cs,
            state,
            jx2_core_state(state),
        );
    } else {
        serial_println!(
            ":: tegra: JX2-NVC67D JX2-VERDICT=NOT-EFI-OWNED — CHNSTATUS_CORE={:#010x}, STATE={:#04x} -> {}, which is NOT one of EFI_INIT1, EFI_INIT2 or EFI_OPERATION. Read this against FE_CHNCTL_CORE, whose EFI bit boot7f measured SET: control says the firmware owns the core channel and status says the channel is not in a firmware state. Exactly one of the two is being misread, and the cheapest thing that settles it is the CORECTL line above — if it no longer reads 0x00000021 the firmware moved, and if it does, then this offset or this decode is wrong and the window and head lines above inherit that doubt ::",
            cs,
            state,
            jx2_core_state(state),
        );
    }
}

// =================================================================================================
// ORIN-CONWIN — RUNG 4 OF THE ORIN DESKTOP LADDER: THE CONSOLE AS A WINDOW. `orinconwin`, DEFAULT OFF.
// =================================================================================================
//
// WHAT THIS RUNG IS. `orin-desktop.md` §6 rung 4: *"route the JD2 console into a `wm` row;
// `fbcon::console_is_routed`; skip the handoff detach when routing succeeded"*. Both halves land here:
// this function opens the console window on `tegra_early_stop`'s terminus line, and
// `jd2_console_pump`'s phase-2 `fbcon::detach()` is folded to
// `if !tegra_conwin_live() { …detach(); }` so a routed console is NOT frozen at the handoff.
//
// NO SECOND CONSOLE RENDERER, and that is the shape the rung was asked for. Every verb below is the
// SHARED implementation the Pi and x86 already reach — `fbcon::panel_console_face_arm`,
// `fbcon::panel_console_window_open`, `fbcon::console_is_routed`, `dock::Layout::for_panel`,
// `wm::reserve_stage`, `wm::present_outcome`, `wm::composite`. There is one
// `panel_console_window_open`, one `route_present_banded`, one `Pending`, and this board now runs
// those same bytes. Nothing in `video/` is edited by this rung; the whole of it is this file's tail
// block, one appended statement and one in-place fold in `main.rs`, one `[features]` entry and the
// arroyo wiring.
//
// ── THE ORDERING RULE (§6.1), AND WHY IT IS A BRANCH RATHER THAN A COMMENT ───────────────────────
//
// `video/desktop_firmware.rs:39-44` states the CONSOLEWIN law, inherited from `desktop_uefi`: the console window carries
// a minimise disc, the only route back from that park is the dock, and *"a control that hides a window
// with no way back is worse than no control"*. §6.1 turns that into an obligation on THIS rung:
//
//     "Rung 4 may not ship a console window on an image where `orinclick` is off. The knob and the
//      console window have to travel together, or the minimise disc is a one-way trip again — the
//      `#[cfg]` cannot express the law, so the rung-4 arc has to."
//
// So `orinconwin` does NOT imply `orindesk` or `orinclick`. Both are read at runtime through `cfg!()`
// — the `main.rs::TEGRADESK_CLICK_ROUTED` idiom, and NOT a literal `true`, for that constant's own
// reason: an assertion that clicks route, on an image that has none, is the one-way trip re-entered
// through the very construct meant to prevent it. An image missing either knob gets the DECLINE below
// and no console window, which is §6.1 enforced by codegen instead of by discipline.
//
// WHY `orindesk` IS IN THE CONJUNCTION and not just `orinclick`. §6.1's letter names `orinclick`
// alone; the conjunction is strictly stronger and it is deliberate. §6.1's own caveat records that on
// an `orinclick` image with no row on the panel the router prints `DECLINE reason=no-target` for every
// press — i.e. the "way back" is unexercisable and its verdict unreadable. `orindesk` puts ORIN-WM1's
// row on the glass, so the console window is not the only clickable thing on the panel and a raise can
// be told from a miss. The rung declines rather than shipping a console window onto a panel where the
// only evidence about the route back would be its own absence.
//
// THE DECLINE PRINTS, and that is load-bearing rather than tidy. An instrument that cannot fire in the
// state it exists for is an absent one. `[orinconwin] gate …` is emitted UNCONDITIONALLY, above every
// refusal, carrying `orindesk=`/`orinclick=` read off the build — so a capture always names the
// ordering rule's two terms whichever way they point. The refusal itself is ONE `serial_println!` with
// a `held` string chosen by a `match` over both terms, for the reason `main.rs`'s DESKSEAM stop-line
// block records as MEASURED: written as two sequential `if !CONST` blocks the second one's string is
// dead code the moment the first const is `false`, and `LC_ALL=C grep -a -o` on the armed artifact
// found one reason and not the other.
//
// ── WHAT IT DOES NOT DO: THE §5.2 STOP-LINE IS NOT CROSSED ───────────────────────────────────────
//
// `desktop_firmware::activate()` is NOT called. §5.2 blocks the desktop-ARMING CASCADE — the Pi overflowed a
// 16 KiB kernel stack in it on two consecutive metal boots and no QEMU gate in this tree can stack the
// preemption frame that does it. This rung takes exactly the two steps of `activate`'s sequence the
// console window needs (2a FONT-PI, 2-3 CONSOLEWIN) and none of the rest: no PIDESK DESKTOP-CLEAR
// (which would paint over ORIN-WM1's row and whose soundness argument is an empty window table — the
// floor `main.rs`'s DESKSEAM refuses on), no `menubar::set_enabled`, no crystal, no `render_service`,
// no window population. `quarry` is not implied, so `quarry::open()` — boot 11's ACTUAL overflow, at
// click-router depth — is the `#[cfg(not(feature = "quarry"))]` `false` stub in this build.
//
// WHAT `desktop_firmware` DOES BRING INTO THE ROUTER, stated because §3.7 promised the opposite for `orinclick`
// alone and the difference must not pass unnoticed: `wc_click_route`'s furniture arms
// (`strip::press_route` -> `crystal::press_at` + `dock::press_at`, `pulsewin::press_route`, the DRAG-PI
// chrome arm, the SHELLWIN-PI arm) are compiled IN on an `orinconwin` image. That is not a tolerated
// widening — it is the rung's precondition. `dock::press_at` IS §6.1's route back; without `desktop_firmware`
// there is no dock in the image at all (`video/mod.rs` gates the whole furniture family on it), so a
// minimise disc really would be one-way. `pulsewin::press_route` returns on a NONE window id — the id
// is non-NONE only once an `orinrender` image has armed and opened the pulse window (PAINTPULSE) — and
// `quarry::press_route` is the stub, so on a plain `orinconwin` build both deep arms are unreachable.
//
// ── WHERE IT RUNS, AND THE STACK ─────────────────────────────────────────────────────────────────
//
// `tegra_early_stop`'s terminus line — the boot core's own entry frame, the same placement and the
// same argument DESKSEAM records: panel seeded (JD1), heap carved (step 3c), IRQs live (JM4), SMP
// secondaries kicked, EL2->EL1 dropped, run queue populated but not driven. `panel_console_window_open`
// ends in `create_at`, which composites, and `composite_inner` is the function whose aarch64 stack
// exhaustion is on the ledger (occ62) — so it runs HERE and not in `jd2_console_pump`, which is a
// `sched::spawn`ed task on a `TASK_STACK_SIZE` stack. ORIN-WM1 made the same choice for the same
// reason.
//
// NOT MEASURED, stated rather than implied: once the route is installed every subsequent
// `serial_println!` reaches `route_present_banded` from WHATEVER stack is printing, not from the boot
// stack. That is exactly what the Pi ships (paced at 60 Hz, damage-limited), and the Orin's own
// `[u7stk]` high-water for it has never been read on this board. §7's standing note applies: every
// stack number quoted for this ladder so far is a Pi number.
//
// ── THE OTHER WRITER, AND WHY IT DOES NOT ERASE THE WINDOW ────────────────────────────────────────
//
// §7 left this open and named it rung-4 territory: `jd2_console_pump` owns the panel through a
// double-buffered `Screen` whose `pal.render()` blits the console back buffer, and whether a
// composited row survives that blit was unmeasured. The answer in SOURCE is that it does —
// `Screen::present_background` subtracts the window layer (`wm::occluders`, the WC-I loop) on BOTH of
// its cfg arms, the aarch64 one included, so the desktop present never writes a pixel inside a live
// window's box. That is a source reading, NOT a metal measurement, and this rung does not claim
// otherwise: the on-glass half stays owed to a bench boot.
//
// ── DEFAULT OFF AND MEASURED ──────────────────────────────────────────────────────────────────────
//
// With `orinconwin` unset every item below vanishes, the terminus call site is one `#[cfg]`-erased
// statement, and the phase-2 guard folds to the bare `fbcon::detach()` it has always been
// (`#[inline(always)]` on a constant `false`). No line moves in any file compiled knob-off — this is a
// FILE-TAIL block, and both `main.rs` edits are same-line. The feature is NOT standalone (see
// `Cargo.toml`): it implies `desktop_firmware` + `tegra_el0`, and `tegra_el0` implies `tegra`, so
// `UNAOS_ORINCONWIN=1 ./arroyo esp-jetson` builds the armed configuration with no second knob — and
// prints the ordering-rule DECLINE, because neither `orindesk` nor `orinclick` came with it.

/// ORIN-CONWIN — one-shot latch. `tegra_early_stop` runs once per boot on the boot core, so this
/// cannot fire today; it is here for `desktop_firmware::activate`'s own reason — `panel_console_window_open` is
/// idempotent behind `CONSOLE_WIN` and would hand the same row straight back, but a second pass would
/// re-arm the face and re-present, and a seam that cannot say it has already run cannot be told from
/// one that declined.
#[cfg(feature = "orinconwin")]
static ORINCONWIN_ENTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// ORIN-CONWIN ORDERING TERM 1 — **is there a row on this panel to click?** `cfg!`, never a literal:
/// `orinconwin` does not imply `orindesk`, and asserting a target that the build does not contain is
/// the defect `main.rs::TEGRADESK_CLICK_ROUTED` was written to avoid. See the ORDERING RULE above.
#[cfg(feature = "orinconwin")]
const ORINCONWIN_DESK_ROW: bool = cfg!(feature = "orindesk");

/// ORIN-CONWIN ORDERING TERM 2 — **do clicks route on this image?** §6.1's binding term: *"Rung 4 may
/// not ship a console window on an image where `orinclick` is off."* DERIVED FROM THE BUILD, for
/// `TEGRADESK_CLICK_ROUTED`'s reason. Routing itself is UNFLOWN — no Orin has booted `orinclick` — so
/// this const says the caller is COMPILED IN, never that a press has ever reached the window layer.
#[cfg(feature = "orinconwin")]
const ORINCONWIN_CLICK_ROUTED: bool = cfg!(feature = "orinclick");

/// **ORIN-CONWIN — route the Orin's kernel console into a `wm` row, or decline and say why.**
///
/// Returns `true` iff the console is ROUTED, read back from [`crate::video::fbcon::console_is_routed`]
/// rather than inferred from this function's own control flow — `desktop_firmware::activate`'s discipline, for
/// its reason: a route declined deep inside the open path must never be reported as installed by a
/// caller reading a stale local. That return value is the ONE fact the caller acts on: it is what
/// `jd2_console_pump`'s phase-2 detach is guarded by.
///
/// Every decline is named on the wire and none of them is fatal: a headless Orin, a panel too small to
/// guarantee the dock, an image the ordering rule refuses, or a console that would not open all boot
/// exactly as they did before, with the detach taken as it always was.
#[cfg(feature = "orinconwin")]
pub fn orin_conwin() -> bool {
    use crate::video::{dock, fbcon, wm};
    use core::sync::atomic::Ordering;

    if ORINCONWIN_ENTERED.swap(true, Ordering::AcqRel) {
        serial_println!("[orinconwin] DECLINE reason=already-armed (the seam is one-shot; a second pass would re-arm the face and re-present a row panel_console_window_open would hand straight back)");
        return fbcon::console_is_routed();
    }

    // 1. THE PANEL, live off the surface the compositor composites onto, never assumed. Copy the info
    //    out and drop `WRITER` immediately: nothing below may hold it across a `wm` call, which is what
    //    keeps the WRITER/TABLE acquisition order acyclic (ORIN-WM1's rule). A headless Orin — no DTB
    //    `simple-framebuffer` handoff, or geometry that failed JD1's sanity — never seeded `WRITER`.
    let info = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!("[orinconwin] DECLINE reason=no-panel (headless boot — JD1 seeded no scanout; there is no glass for a console window to be composited onto)");
            return false;
        }
        fb.info()
    };
    let (pw, ph) = (info.width, info.height);

    // 2. THE STAGING BUFFER (§3.3). Grow-only and idempotent, so this is safe beside ORIN-WM1's own
    //    call and DESKSEAM's; it is repeated here because neither of those is guaranteed to be in the
    //    image. `wm`'s staged presents run inside `SYS_WIN_PRESENT`'s IRQ mask, so a buffer that GREW
    //    on the pass would be a masked acquisition of the global heap `Mutex` — the F1-F5 defect
    //    family — and the routed console presents on every printed line. A short reserve is REPORTED,
    //    not fatal: `wm` keeps its lazy-growth fallback.
    let staged = wm::reserve_stage(&info);

    // 3. THE CONSOLEWIN LAW'S GEOMETRY HALF, evaluated with the SAME call `desktop_firmware` makes —
    //    `MAX_WINDOWS`, not the live count, because the check must hold for every table state the boot
    //    can reach. Pure integer geometry; paints nothing.
    let dock_ok = dock::Layout::for_panel(wm::MAX_WINDOWS, pw, ph).is_some();
    let live = wm::count();
    let routed_before = fbcon::console_is_routed();

    // THE CENSUS. Printed UNCONDITIONALLY, above every refusal below it, so a capture always carries
    // both ordering terms and every floor's measured value rather than the first one that said no.
    serial_println!(
        "[orinconwin] gate panel={}x{}x{} stage={} table={} dock={} route={} orindesk={} orinclick={} rows={}",
        pw, ph, info.bytes_per_pixel,
        staged, live,
        if dock_ok { "GRANTED" } else { "WITHHELD" },
        if routed_before { "ROUTED" } else { "UNROUTED" },
        ORINCONWIN_DESK_ROW as u8,
        ORINCONWIN_CLICK_ROUTED as u8,
        wm::MAX_WINDOWS
    );

    // 4. THE ORDERING RULE (§6.1) — ONE decision and ONE line, a `match` over BOTH terms rather than
    //    the first that said no, for the reason DESKSEAM measured on its own artifact.
    if !(ORINCONWIN_DESK_ROW && ORINCONWIN_CLICK_ROUTED) {
        let held = match (ORINCONWIN_DESK_ROW, ORINCONWIN_CLICK_ROUTED) {
            (false, false) => "no-desk-row+clicks-unrouted",
            (true, false) => "clicks-unrouted",
            _ => "no-desk-row",
        };
        serial_println!("[orinconwin] DECLINE reason=ordering-rule held={} panel={}x{} dock={} (§6.1, not negotiable: the console window carries a minimise disc and video/pidesk.rs:39-44 states the CONSOLEWIN law — the only route back from that park is the dock, and the dock is a route back only once clicks route, so rung 4 may not ship a console window on an image where orinclick is off. UNAOS_ORINDESK is in the conjunction too: without a second row on the panel every press takes the router's no-target arm and the route back has no readable verdict. Arm UNAOS_ORINCONWIN=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1 together or ship no console window)", held, pw, ph, if dock_ok { "GRANTED" } else { "WITHHELD" });
        return false;
    }

    // 5. THE CONSOLEWIN LAW'S REFUSAL. Narrowed exactly as `desktop_firmware` narrows it: this guards the console
    //    WINDOW and nothing else, because the law's own justification is about ONE control on ONE
    //    window. Nothing else on this boot is withheld by it — there is no bar here to follow.
    if !dock_ok {
        serial_println!("[orinconwin] DECLINE reason=dock-cannot-host-full-strip panel={}x{} rows={} (the console's minimise disc would have no way back — dock::Layout::for_panel returns None when the strip will not fit at MAX_WINDOWS, and the check is made against MAX_WINDOWS rather than the live count because it must hold for every table state this boot can reach)", pw, ph, wm::MAX_WINDOWS);
        return false;
    }

    // 6. FONT-PI ON THE ORIN — the console leaves font8x8 BEFORE the window is sized, by necessity:
    //    `panel_console_window_open` reads `c.cell_w`/`c.cell_h` to size the surface in whole cells, so
    //    it must see the face's cell and not the bitmap's. On the 1920x1200 bench panel the 8x8 cell at
    //    scale 1 is ~0.8 mm — metal sitting #30 recorded it as simply not visible — and a console
    //    window whose text cannot be read would make this rung's own metal witness unreadable. A
    //    decline here is reported and NOT fatal: the window still opens, with the bitmap cell.
    let face_cell = fbcon::panel_console_face_arm();
    if face_cell.is_none() {
        serial_println!("[orinconwin] console-face DECLINE reason=console-not-ready (the console keeps font8x8; the window still opens, sized in 8x8 cells)");
    }

    // 7. THE CONSOLE BECOMES A WINDOW. The shared implementation, reached from a third seam — its own
    //    `[wc-x] console-window …` line reports the geometry and the panic fallback, emitted by the
    //    `fbcon` code the Pi widened rather than by anything tegra-specific.
    let win = fbcon::panel_console_window_open();
    let routed = fbcon::console_is_routed();
    if win == wm::WIN_NONE || !routed {
        serial_println!("[orinconwin] DECLINE reason=open-declined win={} route={} panel={}x{} (fbcon named its own reason on the line above this one — console not ready, allocation refused, geometry unavailable, create failed or install contended; the boot continues and jd2_console_pump takes its detach exactly as it always did)", win, routed, pw, ph);
        return false;
    }

    // 8. PRESENT + COMPOSITE — `present_outcome` over `present` for the naming alone, ORIN-WM1's
    //    reason: `present`'s `bool` folds "the pass ran" into "the pass was suppressed", and on a rung
    //    whose verdict is "did the console's pixels reach glass" those two must not look alike. The
    //    trailing verdict is DERIVED from the outcome CROSSED with the route read back, never asserted.
    let outcome = wm::present_outcome(win);
    wm::composite();
    let (pres, ok) = match outcome {
        wm::Presented::Composited => ("Composited", true),
        wm::Presented::Coalesced => ("Coalesced", true),
        wm::Presented::Suppressed => ("Suppressed", false),
        wm::Presented::NoRow => ("NoRow", false),
    };
    let (cw, ch) = face_cell.unwrap_or((0, 0));
    serial_println!(
        "[orinconwin] win={} panel={}x{} cell={}x{} stage={} table={} present={} route={} live={} -> {}",
        win, pw, ph, cw, ch, staged, wm::count(), pres, routed,
        // LIVE vs FROZEN is the rung's second half. It USED TO BE A COMPILE-TIME LITERAL — the string
        // `"LIVE"`, asserted from the build rather than measured — which is the one thing the block
        // four lines above this forbids of every other field on this line ("DERIVED from the outcome
        // CROSSED with the route read back, never asserted"). It is now a READ-BACK, taken here at
        // print time, of the same `CONSOLE_WIN` cell `tegra_conwin_live()` answers off. Not redundant
        // with `route=`: that value was sampled BEFORE `present_outcome` and `composite` ran, this one
        // after, so a route dropped by the present pass can no longer print `live=LIVE`.
        //
        // ⚠ WHAT THIS READ-BACK DOES NOT PROVE — stated here so the next reader does not re-derive it.
        // `fbcon::detach()` sets `GUI_ACTIVE` and does NOT clear `CONSOLE_WIN`, so after a detach
        // `console_is_routed()` still answers TRUE while `fbcon::_print` returns at its first test and
        // no further glyph ever reaches the window. This sample is taken before any terminus detach
        // can have run, so the strongest thing it can say is "the route is installed at this instant",
        // never "no later detach freezes it". That second half is a BEHAVIOURAL guarantee owned by the
        // guards on the two detach sites — `main.rs`'s phase-2 line and, as of this arc, the
        // `tegra_rast_demo_maybe` line that used to detach unguarded AFTER this rung had installed the
        // route and printed `live=LIVE` — and not by this field. A read-back that could close the gap
        // needs `GUI_ACTIVE` exposed from `video/fbcon.rs`; that file is outside this arc's lane.
        if fbcon::console_is_routed() { "LIVE" } else { "FROZEN" },
        if ok && routed { "ROUTED" } else { "PRESENT-DECLINED" }
    );
    routed
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// ORIN-TENANT — rung 6 of the Orin desktop ladder. `orintenant`, DEFAULT OFF.
//
// EL0 WINDOW TENANTS: an EL0 program (`run /fat/VUG.ELF`) owns a compositor window through the
// arch-neutral SYS_WIN_* surface instead of the raw panel. The verbs themselves are NOT this rung's —
// they are the shared WC-B implementation `arch/aarch64/syscall.rs` has carried since the Pi arc, and
// they compile on this board under `tegra_el0` alone. What this rung adds is (a) the CRYSTAL-HD
// geometry parity fix in `mmu_tegra_el0.rs` (unconditional under `tegra_el0` — see the block comment
// there for the two live defects it removes), and (b) THIS instrument: the arming point that gives
// the EL0 present path an unmasked staging buffer, and the `[orintenant]` witness family that lets a
// metal flight adjudicate "vug owns a window" from the wire.
//
// THE OWNERSHIP MODEL, stated because the x86 seat's standing warning (rmbp 6, 2026-08-25) is about
// exactly this seam: THE TENANT SURFACE IS KERNEL-OWNED, NEVER TASK-OWNED. The compositor row's
// `surf` pointer names the slot's FB backing — `alloc_zeroed`ed once per slot from the kernel heap
// (`mmu_tegra_el0::SLOT_BACKING`), never freed, recycled per tenant with a `build_slot` scrub — and
// is mapped INTO the tenant at the fixed window VA, not lent BY it. No compositor pointer ever names
// memory whose lifetime is the EL0 task's. On task exit the teardown funnel
// (`syscall::clear_handle_row` -> `win_close_asid` -> `wm::close_owner` -> `close_compat` ->
// `focus_release`) unmaps the surface leaves and retires every row the ASID owned, so a window row
// can never outlive its owner into the next tenant's frames — that funnel predates this rung and is
// shared with the Pi; the `[orintenant] reap` line below is its wire witness on this board.
//
// CLOSE POLICY, decided and stated (the boot7h contrast): kernel furniture REFUSES close
// (`close_owner` on a pseudo-ASID -> `furniture-refused`, the CONSOLEWIN law). A TENANT CLOSES: the
// close disc on an EL0-owned window runs the ungated CLOSE-CLEAN chain — `close_owner(asid)` kills
// the process with `EXEC_CLOSED_STATUS`, `run` reports `closed (window close box)`, and the same
// teardown funnel reaps the row. Nothing here changes that; the census counts it.
//
// MINIMISE, reported rather than repainted as policy: the minimise arm in `wc_click_route` is
// ungated, so a tenant CAN be parked on any image. The routes back are the dock (`desktop_firmware` aboard —
// the conjunction image) or kill/exit; the dock round-trip is the ladder's next attended item
// (§3.9.1) and is NOT claimed here. The census prints `desktop_firmware=` so a capture names which image shape
// the park happened on.
//
// WITNESSES (tokens all LONGER than 8 bytes — LLVM immediate-encodes shorter ones invisibly to an
// artifact grep):
//   1. `[orintenant] arm ...`         — ONCE, from the terminus line: panel, stage reserve, caps.
//   2. `[orintenant] create ... -> V` — one per SYS_WIN_CREATE outcome (TENANT-WINDOW /
//      HEADLESS-COMPOSITOR-REFUSED / DECLINE reason=geometry-over-max).
//   3. `[orintenant] close ...`       — one per owner SYS_WIN_CLOSE.
//   4. `[orintenant] reap ...`        — the exit-path sweep, when it actually reaped rows.
//   5. `[orintenant] census ... -> V` — every ~10 s from the pump's own drain loop (rung 3's idiom:
//      the census IS the liveness; `IDLE-NO-TENANTS` is UNRUN, never PASS).
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// ORIN-TENANT — lifetime counters, bumped from the four line-neutral `syscall.rs` call sites and
/// read by the census. Creates that returned an id; creates the compositor refused a row for
/// (`wm_id == WIN_NONE` — the verbs still work, nothing reaches the panel); geometry refusals
/// (`-EINVAL` over the 288x288 cap — the exact answer the pre-parity kernel gave the shipped vug,
/// so this counter going nonzero after the fix is a FAIL the census names); owner closes; and rows
/// reaped by the exit-path teardown funnel.
#[cfg(feature = "orintenant")]
static TEN_CREATES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orintenant")]
static TEN_HEADLESS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orintenant")]
static TEN_GEOM_REFUSED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orintenant")]
static TEN_CLOSES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orintenant")]
static TEN_REAPED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-TENANT — per-ASID rows reaped by the CURRENT teardown pass (index = asid, 0 unused). Bumped
/// row-by-row inside `win_close_asid`'s masked loop (a bump is lock-safe where a print is not),
/// swapped to 0 and PRINTED by `orin_tenant_note_reap_done` after the funnel releases `WINDOWS`.
/// One teardown of one ASID runs on one core, so the pair cannot interleave against itself.
#[cfg(feature = "orintenant")]
static TEN_REAP_PENDING: [core::sync::atomic::AtomicU32; 9] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 9];
/// **ORIN-TENANTFAULT — the per-ASID "this address space died on an EL0 fault" flag** (index = asid,
/// 0 unused), set by `orin_tenant_note_el0_fault` from the EL0 fault handler and CONSUMED by
/// `orin_tenant_note_reap_done` on the teardown that follows.
///
/// WHY A FLAG AND NOT A COUNTER AT FAULT TIME. The census's question is not "did anything at EL0
/// fault this boot" — `el0-wild-write` and the whole M6b fixture cascade fault BY DESIGN and would
/// swamp it. The question is "did a WINDOW TENANT die on a fault", and the only site that knows an
/// ASID owned window rows is the teardown funnel, which runs afterwards. So the fault leaves a mark
/// keyed by ASID, and the reap — which counts the rows — decides whether that mark meant a tenant.
/// A faulting ASID that owned no rows clears its flag and is counted by nothing, which is exactly
/// right: `el0-hello` and every non-window fixture stay off this instrument.
///
/// The flag is swapped (never merely stored) at both ends so a RECYCLED ASID cannot inherit a
/// departed program's fault — the same recycled-slot hazard `clear_input_row` and the takeover-latch
/// CAS in `clear_handle_row` are written against.
#[cfg(feature = "orintenant")]
static TEN_FAULT_ASID: [core::sync::atomic::AtomicBool; 9] =
    [const { core::sync::atomic::AtomicBool::new(false) }; 9];
/// ORIN-TENANTFAULT — tenants (ASIDs that owned at least one window row) whose rows were reaped
/// after an EL0 fault. Boot-scoped and STICKY on purpose: a crash that happened is a fact about this
/// boot, and a later well-behaved tenant must not be able to retire it. Read by the census, where it
/// outranks `TENANT-LIVE` for that reason.
#[cfg(feature = "orintenant")]
static TEN_FAULTS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-TENANTFAULT — EL0 faults seen by `orin_tenant_note_el0_fault` whose ASID was out of the
/// flag table's range (`asid >= TEN_FAULT_ASID.len()`), i.e. faults this instrument structurally
/// CANNOT attribute. Named on the census line rather than dropped: `orin_tenant_note_reap_row` has
/// the same bound and drops the same way, so a boot that reaches those ASIDs must say that its
/// close/reap accounting is incomplete instead of printing a clean verdict over a blind spot.
#[cfg(feature = "orintenant")]
static TEN_FAULT_UNATTRIB: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-TENANT — census bookkeeping, `orinclick`'s shape: arm latch, last-census tick, sequence,
/// and the CNTPCT reading at arm time.
#[cfg(feature = "orintenant")]
static TEN_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "orintenant")]
static TEN_CENSUS_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "orintenant")]
static TEN_CENSUS_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orintenant")]
static TEN_T0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// ORIN-TENANT — census cadence in pump sweep ticks (~250 ms each): 40 ≈ 10 s, `orinclick`'s number
/// for `orinclick`'s reason (free at 115200 baud; a stopped census localises a dead pump).
#[cfg(feature = "orintenant")]
const TEN_CENSUS_PERIOD: u64 = 40;
/// ORIN-TENANT — per-event line budget. Creates/closes are program-rate, not frame-rate, so this is
/// a stuck-loop guard, not a rate limit: an EL0 program looping on SYS_WIN_CREATE would otherwise
/// own the UART. The census reports the suppressed remainder.
#[cfg(feature = "orintenant")]
static TEN_LOGGED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orintenant")]
const TEN_LOG_MAX: u32 = 256;

/// ORIN-TENANT — CNTPCT/CNTFRQ on the calling core (the pump is timerless-cooperative post-JM6; the
/// free-running counter is its only clock — `clk_now_freq`'s reason, duplicated because that fn is
/// `orinclick`-gated and this rung must not imply that knob).
#[cfg(feature = "orintenant")]
fn ten_now_freq() -> (u64, u64) {
    let (now, freq): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, preserves_flags));
    }
    (now, freq)
}

/// **ORIN-TENANT — the arming point.** Appended to `tegra_early_stop`'s terminus line (zero source
/// lines added — the tegra knob-off byte-identity constraint), after `orin_conwin`'s statement so a
/// conjunction boot's console row is already in the table this census counts.
///
/// Two jobs, in order:
///   1. **Reserve the compositor staging buffer** (§3.3, the F-family reason). `wm`'s staged
///      presents run inside `SYS_WIN_PRESENT`'s IRQ mask; on an image where no other rung called
///      `reserve_stage` (plain `orintenant`, no `orindesk`/`orinconwin`), the tenant's FIRST present
///      would grow the stage under that mask — a masked acquisition of the global heap `Mutex`.
///      `reserve_stage` is idempotent and grow-only, so repeating it beside the other rungs' calls
///      costs nothing on the conjunction image.
///   2. **Print the pre-state** the flight scores create lines against: panel, stage bytes, live
///      table rows, the negotiable cap (`FB_WIN_MAX_*` — 288x288 iff the parity fix is aboard, so
///      the arm line itself is the parity witness), slot count, and which sibling knobs are aboard.
///
/// A headless boot DECLINES, named: with no panel there is no stage to size and nothing for a
/// tenant present to reach — the verbs still answer (fail-closed at the `wm` layer, `WIN_NONE`
/// rows), and the census will report any such creates as HEADLESS rather than losing them.
#[cfg(feature = "orintenant")]
pub fn orin_tenant_arm() {
    use crate::video::wm;
    use core::sync::atomic::Ordering;

    let (now, _) = ten_now_freq();
    TEN_T0.store(now, Ordering::Relaxed);
    let info = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!(
                "[orintenant] arm -> DECLINE reason=no-panel (headless boot — JD1 seeded no scanout; \
                 SYS_WIN_* still answers, every row is compositor-refused and the census will say so)"
            );
            TEN_ARMED.store(true, Ordering::Release);
            return;
        }
        fb.info()
    };
    let staged = wm::reserve_stage(&info);
    serial_println!(
        "[orintenant] arm panel={}x{}x{} stage={} table={} cap={}x{} rslots={} uslots={} orindesk={} orinclick={} orinconwin={} pidesk={} -> ARMED",
        info.width, info.height, info.bytes_per_pixel,
        staged, wm::count(),
        crate::arch::aarch64::uslots::FB_WIN_MAX_W, crate::arch::aarch64::uslots::FB_WIN_MAX_H,
        crate::arch::aarch64::uslots::FB_WIN_SLOTS, crate::arch::aarch64::uslots::USER_SLOTS,
        cfg!(feature = "orindesk") as u8, cfg!(feature = "orinclick") as u8,
        cfg!(feature = "orinconwin") as u8, cfg!(feature = "desktop_firmware") as u8
    );
    TEN_ARMED.store(true, Ordering::Release);
}

/// ORIN-TENANT — a SYS_WIN_CREATE the geometry gate refused (`-EINVAL`, over the negotiated cap).
/// Called from the refusal line itself, BEFORE the verb takes any lock, so the print is unheld.
/// This is the exact wire the pre-parity kernel would have produced for the shipped vug; after the
/// parity fix it is reachable only by a program asking past 288 — and the census turns a nonzero
/// count into `FAIL reason=geometry-refused`, because a shipped binary being refused geometry is
/// rung 6's own defect class, not an app bug to shrug at.
#[cfg(feature = "orintenant")]
pub fn orin_tenant_note_refuse(w: u64, h: u64) {
    use core::sync::atomic::Ordering;
    TEN_GEOM_REFUSED.fetch_add(1, Ordering::Relaxed);
    if TEN_LOGGED.fetch_add(1, Ordering::Relaxed) < TEN_LOG_MAX {
        serial_println!(
            "[orintenant] create surf={}x{} -> DECLINE reason=geometry-over-max cap={}x{}",
            w, h,
            crate::arch::aarch64::uslots::FB_WIN_MAX_W,
            crate::arch::aarch64::uslots::FB_WIN_MAX_H
        );
    }
}

/// ORIN-TENANT — a SYS_WIN_CREATE that returned an id. `wm_bound` is whether the compositor minted
/// a row (`wm_id != WIN_NONE`); false is the HEADLESS shape — verbs succeed, nothing reaches the
/// panel — which the census surfaces as a DECLINE rather than letting a green exit code stand over
/// an invisible window.
///
/// PRINTED UNDER THE `WINDOWS` HOLD (the verb holds it to its last expression), and that is
/// deliberate rather than an oversight: the global order is `WINDOWS` ⊃ `wm::TABLE` ⊃ `WRITER`
/// (WC-B's own doc), a routed console's `serial_println!` descends exactly that way
/// (`route_present_banded` -> `TABLE`, then per-band `WRITER` micro-holds), and nothing under
/// `video/` can re-enter the syscall layer. Cost is one bounded UART/composite per CREATE — a
/// program-rate event, the same class of masked hold `sys_win_present` already takes per frame.
#[cfg(feature = "orintenant")]
pub fn orin_tenant_note_create(asid: u64, id: u64, w: u32, h: u32, wm_bound: bool) {
    use core::sync::atomic::Ordering;
    TEN_CREATES.fetch_add(1, Ordering::Relaxed);
    if !wm_bound {
        TEN_HEADLESS.fetch_add(1, Ordering::Relaxed);
    }
    if TEN_LOGGED.fetch_add(1, Ordering::Relaxed) < TEN_LOG_MAX {
        serial_println!(
            "[orintenant] create asid={} win={} surf={}x{} wm-bound={} -> {}",
            asid, id, w, h, wm_bound as u8,
            if wm_bound { "TENANT-WINDOW" } else { "HEADLESS-COMPOSITOR-REFUSED" }
        );
    }
}

/// ORIN-TENANT — an owner's own SYS_WIN_CLOSE completed (row freed, surface unmapped, compositor
/// row destroyed). Printed after the verb released `WINDOWS` and ran the wm drain barrier.
#[cfg(feature = "orintenant")]
pub fn orin_tenant_note_close(asid: u64, id: u64) {
    use core::sync::atomic::Ordering;
    TEN_CLOSES.fetch_add(1, Ordering::Relaxed);
    if TEN_LOGGED.fetch_add(1, Ordering::Relaxed) < TEN_LOG_MAX {
        serial_println!("[orintenant] close asid={} win={} -> CLOSED-BY-OWNER", asid, id);
    }
}

/// ORIN-TENANT — one row reaped by `win_close_asid`'s masked sweep (exit-path teardown). Bump only:
/// this runs inside the `WINDOWS` hold in an already-IRQ-masked teardown, where a print would be a
/// gratuitous hold extension; `orin_tenant_note_reap_done` prints the total once the funnel is past
/// the lock.
#[cfg(feature = "orintenant")]
#[inline(never)] // cold (task-exit only), and a `bl` in the artifact is the reachability proof this family carries
pub fn orin_tenant_note_reap_row(asid: u64) {
    use core::sync::atomic::Ordering;
    if (asid as usize) < TEN_REAP_PENDING.len() {
        TEN_REAP_PENDING[asid as usize].fetch_add(1, Ordering::Relaxed);
    }
}

/// ORIN-TENANT — the exit-path reap's wire witness, printed from `clear_handle_row` after
/// `win_close_asid` returned (WINDOWS released; the funnel's own IRQ mask still held, as every
/// teardown print on this arch is). Silent when the dying ASID owned no windows — el0-hello and
/// every non-window fixture exit without a line, so the reap wire is evidence, not noise.
#[cfg(feature = "orintenant")]
#[inline(never)] // cold (task-exit only), and a `bl` in the artifact is the reachability proof this family carries
pub fn orin_tenant_note_reap_done(asid: u64) {
    use core::sync::atomic::Ordering;
    if (asid as usize) >= TEN_REAP_PENDING.len() {
        return;
    }
    // ORIN-TENANTFAULT — consume the fault flag FIRST, and unconditionally, so it is cleared even on
    // the `n == 0` return below. A faulting ASID that owned no windows must leave the flag table
    // clean: otherwise the next program recycled into that slot would inherit the mark and its
    // perfectly ordinary exit would be counted as a crash.
    let faulted = TEN_FAULT_ASID[asid as usize].swap(false, Ordering::AcqRel);
    let n = TEN_REAP_PENDING[asid as usize].swap(0, Ordering::Relaxed);
    if n == 0 {
        return;
    }
    TEN_REAPED.fetch_add(n, Ordering::Relaxed);
    // Rows AND a fault: this ASID was a window tenant and it did not leave on its own feet. One bump
    // per TENANT, not per row — the census's `faults=` counts crashed programs, and a program that
    // owned three windows crashed once.
    if faulted {
        TEN_FAULTS.fetch_add(1, Ordering::Relaxed);
    }
    if TEN_LOGGED.fetch_add(1, Ordering::Relaxed) < TEN_LOG_MAX {
        serial_println!(
            "[orintenant] reap asid={} rows={} faulted={} -> {}",
            asid, n, faulted as u8,
            if faulted {
                "TENANT-REAPED-AFTER-FAULT (the owner took an EL0 fault and was killed; the [orintenant] fault line above names the syndrome. These rows were reclaimed BY THE KERNEL, not surrendered by the program — this is the shape TENANT-EXITED-CLEAN used to swallow)"
            } else {
                "TENANT-REAPED (exit-path funnel: win_close_asid unmapped + freed, wm::close_owner retires the compositor rows next)"
            }
        );
    }
}

/// **ORIN-TENANTFAULT — an EL0 fault reaches the tenant wire.** Called from
/// `exceptions::aarch64_el0_fault_handler` (same-line append, per that file's PANIC-Location rule)
/// after `record_el0_kill` and before `sched::exit()`, which never returns.
///
/// THE DEFECT THIS CLOSES. `orin_tenant_census`'s `TENANT-EXITED-CLEAN` fired on `closes=0 reaped=1`,
/// so a tenant that CRASHED and a tenant that exited without closing its window produced the same
/// PASS. The census could not tell them apart because nothing on the fault path was wired to it:
/// `aarch64_el0_fault_handler` prints its `:: EL0 FAULT: … ::` line and routes the kill into
/// `record_el0_kill`, whose every arm lands in an M6b/U-series fixture counter that the tenant rung
/// does not read and must not read (those fixtures fault by design). The syndrome was therefore on
/// the wire but not in the verdict, and a reader had to correlate two unrelated line families by eye.
///
/// WHAT IT PRINTS, and why each field. The tenant (`asid`, `name`, `pid` — the pid is the number
/// `jobs` prints and `kill` takes, KEYSTAT's argument for putting it on the fault line at all); THE
/// EL, both halves, MEASURED rather than asserted — `from-el` off `SPSR_EL1.M[3:2]` (the EL the
/// exception was taken FROM) and `at-el` off `CurrentEL` (the EL it was taken AT) — because this
/// vector is reachable only from a lower EL and a line that merely repeats that assumption is worth
/// nothing when the assumption is what broke; and the syndrome (`esr` whole, `ec`/`iss` decoded,
/// `elr`, `far`). `far` prints `--` unless the EC makes it architecturally valid, exactly as the
/// handler's own line does: for every other EC it holds a stale value and printing it would be an
/// invention.
///
/// LOCK AND CONTEXT DISCIPLINE — this runs at EL1 with DAIF masked on the faulting task's own kernel
/// stack. It reads three system registers, touches one atomic, and prints. It takes NO kernel lock
/// of its own: in particular it does not ask `orin_tenant_win_stats` how many rows this ASID owns,
/// which would be a `WINDOWS` acquisition from a fault handler against a holder that could be
/// mid-composite on another core. The row count is not needed here — `orin_tenant_note_reap_done`
/// has it a few microseconds later, on the teardown this fault is about to cause, and that is where
/// the fault is converted into a tenant verdict.
///
/// THE ASID comes from `TTBR0_EL1[63:48]`, one register read, because that IS the identity of the
/// address space that faulted (`mmu_tegra_el0::slot_ttbr0` composes it as `l1_pa | (asid << 48)`).
/// The alternative — a `current_asid()` accessor on `sched::Task` — would be a new public seam in a
/// file this arc has no reason to touch, to recover a number the hardware is already holding.
#[cfg(feature = "orintenant")]
#[inline(never)] // cold (fault-path only), and a `bl` in the artifact is the reachability proof this family carries
pub fn orin_tenant_note_el0_fault(name: &str, pid: u64, esr: u64, elr: u64, far: u64, far_valid: bool) {
    use core::sync::atomic::Ordering;
    let (ttbr0, spsr, cur): (u64, u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, SPSR_EL1", out(reg) spsr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CurrentEL", out(reg) cur, options(nomem, nostack, preserves_flags));
    }
    let asid = (ttbr0 >> 48) & 0xFFFF;
    // SPSR_EL1.M[3:0]: bit 4 clear == AArch64, and M[3:2] is then the EL the exception came from.
    let from_el = (spsr >> 2) & 0b11;
    let at_el = (cur >> 2) & 0b11;
    let ec = (esr >> 26) & 0x3F;
    let iss = esr & 0x1FF_FFFF;
    if (asid as usize) < TEN_FAULT_ASID.len() {
        TEN_FAULT_ASID[asid as usize].store(true, Ordering::Release);
    } else {
        TEN_FAULT_UNATTRIB.fetch_add(1, Ordering::Relaxed);
    }
    // NOT under TEN_LOG_MAX: the suppression budget exists to keep a chatty create/close/reap stream
    // off a 115200-baud wire, and a fault is neither chatty nor routine. A tenant crash that went
    // unprinted because three windows opened first is the exact failure this function was written to
    // end. The EL0 fault path is already a dead end for the task — it cannot recur on this ASID.
    if far_valid {
        serial_println!(
            "[orintenant] fault asid={} task='{}' pid={} from-el={} at-el={} esr={:#x} ec={:#04x} iss={:#x} elr={:#x} far={:#x} attributed={} -> TENANT-EL0-FAULT",
            asid, name, pid, from_el, at_el, esr, ec, iss, elr, far,
            ((asid as usize) < TEN_FAULT_ASID.len()) as u8
        );
    } else {
        serial_println!(
            "[orintenant] fault asid={} task='{}' pid={} from-el={} at-el={} esr={:#x} ec={:#04x} iss={:#x} elr={:#x} far=-- attributed={} -> TENANT-EL0-FAULT",
            asid, name, pid, from_el, at_el, esr, ec, iss, elr,
            ((asid as usize) < TEN_FAULT_ASID.len()) as u8
        );
    }
}

/// **ORIN-TENANT — the ~10 s census, from `jd2_console_pump`'s idle sweep** (appended line-neutral
/// beside `orin_click_census`'s call). Rung 3's whole argument applies verbatim: this seam's death
/// is a dead pump, nothing re-homes a dead pump's roles, and a census that stops IS the report. It
/// prints on its own core off its own CNTPCT, so it cannot report liveness it does not have;
/// `seq=` increments by one per line so a serial gap names itself.
///
/// **THE FALSE PASS THIS LADDER USED TO CARRY (ORIN-TENANTFAULT).** `TENANT-EXITED-CLEAN` was
/// documented as *"tenants existed and every one left through close/reap"* and tested as
/// `creates != 0` with `rows == 0` — which is not that claim at all. It fired on `closes=0 reaped=1`:
/// a program whose window was reclaimed BY THE KERNEL'S teardown funnel, having never called
/// `SYS_WIN_CLOSE`, was reported as a clean exit. Three different histories printed the same PASS:
/// the tenant closed its window and exited; the tenant exited (or was killed) without closing; the
/// tenant CRASHED. The word "clean" was doing no work, because `reaped` — the counter that says the
/// kernel had to clean up after the program — was not in the test.
///
/// Two changes close it. (1) The exit arms are split by `(closes, reaped)` rather than collapsed, so
/// a surrender and a reclamation get different names. (2) `faults` — window tenants killed by an EL0
/// fault, wired in by `orin_tenant_note_el0_fault` + `orin_tenant_note_reap_done` — becomes an INPUT,
/// so the crash is discriminated by evidence from the fault path instead of being inferred from a
/// row count that cannot see it.
///
/// Verdict ladder (first match wins), each reachable and none constant:
///   * `FAIL reason=geometry-refused`  — a create was refused over the cap: the pre-parity defect
///     observed live; must never print on a post-parity image running the shipped fixtures.
///   * `FAIL reason=tenant-faulted`    — at least one window tenant was killed by an EL0 fault.
///     ABOVE `TENANT-LIVE` and above the DECLINE, deliberately: the counter is boot-scoped and
///     sticky, a crash that happened is a fact about this boot, and a later healthy tenant must not
///     be able to retire it. A verdict that a survivor can mask is not a crash report.
///   * `DECLINE reason=headless-rows`  — creates succeeded but the compositor refused rows (no
///     panel, or `wm` table full): verbs green, glass empty, said out loud.
///   * `TENANT-LIVE`                   — at least one EL0-owned row is in the table NOW.
///   * `IDLE-NO-TENANTS`               — nobody ran an EL0 window program. **UNRUN, never PASS.**
///   * `FAIL reason=exit-unaccounted`  — every row is gone but `closes + reaped != creates`, so rows
///     left by a path neither witness saw. A self-check on the instrument, not a claim about the
///     panel, and it has a KNOWN live cause: `orin_tenant_note_reap_row` and `TEN_FAULT_ASID` are
///     both bounded at 9 ASIDs and silently drop past it. On a boot that reaches those slots the
///     accounting IS incomplete, and this arm says so rather than printing a clean verdict over the
///     blind spot.
///   * `TENANT-EXITED-CLEAN`           — `closes == creates`: every window was surrendered by its
///     own owner through `SYS_WIN_CLOSE`. The kernel reclaimed nothing. **This is the only arm that
///     now means what the name says.**
///   * `TENANT-EXITED-UNCLOSED`        — `closes == 0`: no owner ever closed; every row was reclaimed
///     by the exit-path funnel. NOT a failure on its own — a program may legally exit holding a
///     window — but not a clean exit either, and it is the shape a crash also takes. Read it beside
///     `faults=`: with `faults=0` this is a program that left its windows behind; with `faults != 0`
///     the ladder never reaches here, because the FAIL arm above claimed it.
///   * `TENANT-EXITED-PARTIAL`         — both counters nonzero: some windows surrendered, some
///     reclaimed. Its own name because averaging it into either neighbour would hide whichever half
///     is the defect.
#[cfg(feature = "orintenant")]
pub fn orin_tenant_census(tick: u64) {
    use crate::arch::aarch64::syscall as sc;
    use core::sync::atomic::Ordering;

    // FOOTPRINT — rung 3's rule: cadence decided first, off register reads; the WINDOWS table is
    // touched only on the ~1-in-40 pass that prints. `orin_tenant_win_stats` is a masked micro-hold
    // (count + fold, no pixel work), sequential with — never nested inside — any lock here.
    if !TEN_ARMED.load(Ordering::Acquire) {
        return;
    }
    if tick.wrapping_sub(TEN_CENSUS_TICK.load(Ordering::Relaxed)) < TEN_CENSUS_PERIOD {
        return;
    }
    TEN_CENSUS_TICK.store(tick, Ordering::Relaxed);
    let seq = TEN_CENSUS_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let (now, freq) = ten_now_freq();
    let up = if freq == 0 { 0 } else { now.wrapping_sub(TEN_T0.load(Ordering::Relaxed)) / freq };

    let (rows, bound, presents) = sc::orin_tenant_win_stats();
    let creates = TEN_CREATES.load(Ordering::Relaxed);
    let headless = TEN_HEADLESS.load(Ordering::Relaxed);
    let refused = TEN_GEOM_REFUSED.load(Ordering::Relaxed);
    let closes = TEN_CLOSES.load(Ordering::Relaxed);
    let reaped = TEN_REAPED.load(Ordering::Relaxed);
    let logged = TEN_LOGGED.load(Ordering::Relaxed);
    let suppressed = logged.saturating_sub(TEN_LOG_MAX.min(logged));

    // ORIN-TENANTFAULT — the two new inputs. `faults` is tenants (row-owning ASIDs) killed by an EL0
    // fault; `unattrib` is faults this instrument could not key to an ASID at all (the 9-slot bound).
    let faults = TEN_FAULTS.load(Ordering::Relaxed);
    let unattrib = TEN_FAULT_UNATTRIB.load(Ordering::Relaxed);
    // Every created row ends up either surrendered by its owner (`closes`) or reclaimed by the exit
    // funnel (`reaped`); a closed row is freed and cannot then be reaped, so with `rows == 0` the two
    // must sum to `creates`. `saturating_add` because the sum is a verdict input, not arithmetic to
    // be trusted — a wrap here would silently produce a PASS.
    let accounted = closes.saturating_add(reaped);
    let verdict = if refused != 0 {
        "FAIL reason=geometry-refused"
    } else if faults != 0 {
        "FAIL reason=tenant-faulted"
    } else if creates != 0 && headless != 0 {
        "DECLINE reason=headless-rows"
    } else if rows != 0 {
        "TENANT-LIVE"
    } else if creates == 0 {
        "IDLE-NO-TENANTS"
    } else if accounted != creates {
        "FAIL reason=exit-unaccounted"
    } else if closes == creates {
        "TENANT-EXITED-CLEAN"
    } else if closes == 0 {
        "TENANT-EXITED-UNCLOSED"
    } else {
        "TENANT-EXITED-PARTIAL"
    };
    serial_println!(
        "[orintenant] census seq={} t={} up={}s rows={} bound={} creates={} headless={} refused={} closes={} reaped={} faults={} unattrib={} presents={} suppressed={} focus={:#x} pidesk={} -> {}",
        seq, tick, up, rows, bound, creates, headless, refused, closes, reaped, faults, unattrib,
        presents, suppressed, sc::user_input_active(), cfg!(feature = "desktop_firmware") as u8, verdict
    );
}


// =================================================================================================
// ORIN-SUPSTATE — SMP-redesign Candidate A, arc 1: the console-surface STATE LIFT. `supstate`,
// DEFAULT OFF.
// =================================================================================================
//
// THE DEFECT THIS ARC ADDRESSES (F2 of the redesign record, ~/.claude/plans/unaos/review/
// orin-smp-redesign.md): the whole interactive surface is ONE task on ONE core, and its entire
// state — `Screen`, `TargetPal`, `Console` — lives on that task's own stack (`main.rs`,
// `jd2_console_pump` phase 2). When the task dies, the state is unreachable even in principle:
// there is nothing to hand a successor and no successor to hand it to. The census doc above
// (`orin_click_census`) records the consequence in this file's own words: nothing in this tree
// inherits a dead task's singleton roles.
//
// WHAT THIS MODULE IS. The ownership half of Candidate A: a module-owned handle for the console
// surface, so the state OUTLIVES whichever task is currently driving it and a successor task COULD
// adopt it. It follows the `ORINWM1_STORE` mould already in this file — a `spin::Mutex` static in
// this track's lane — not a new invention. `TargetPal` is NOT stored: it is a borrow
// (`pub surface: &'a mut Screen`), i.e. a VIEW, and storing it would make the handle
// self-referential. It is reconstructed per lock scope from the stored `Screen` — via the public
// field, deliberately NOT via `TargetPal::new`, because `new` prints the `:: UI1:` metrics line and
// per-frame reconstruction would spam the wire. `TargetPal::new` is called exactly once, at
// install, so the armed boot carries the same single UI1 line the baseline boot does.
//
// WHAT THIS MODULE IS NOT — arc 2's supervisor. There is no reap path, no restart, no generation
// sweep here: A's supervisor needs a clock that survives a wedged task, core 0 has none post the
// JM6 drop, and A-alone would ship a watchdog that cannot fire (the redesign record's own words:
// "A alone is not deliverable; A-on-B is"). The `generation` field exists so arc 2 has something to
// bump; nothing in arc 1 reads it back.
//
// LOCK DISCIPLINE, stated before the lock exists. `SURFACE` is strictly OUTERMOST: it may be held
// while `video::WRITER` or `wm`'s TABLE are taken-and-released beneath it (a `handle_key` command
// does both), and NOTHING that holds `WRITER` or TABLE ever takes `SURFACE` — the only callers of
// this module are the console roles themselves. Beneath `SURFACE`, the existing rule is unchanged:
// WRITER before TABLE, sequential, never nested (`orin_wm1`'s rule, restated at
// `orin_click_census`). So the order is acyclic by construction: SURFACE -> WRITER -> TABLE.
//
// THE COOPERATIVE-CORE RULE, which is the one real hazard. Core 0 has NO preemption (F4: the tegra
// path never sets `SCHED_ACTIVE`), so a bare `lock()` spin against a holder that yielded would be a
// LIVELOCK: the spinner never yields, the holder never runs. Every acquisition in this module is
// therefore `try_lock` + `yield_now` on failure — a waiter that cannot get the lock gives the core
// back. The holder side has the complementary obligation, and one deliberate exception: the
// DISPATCHER holds `SURFACE` across `handle_key`, whose full-screen commands (`vug`, `gneiss`)
// yield from their own drain loops. That is safe under this discipline — the waiters yield, so the
// holder's command keeps running and the lock is released when the command returns — and it is
// exactly today's semantics: while a foreground command runs, the monolithic pump was blocked
// inside `handle_key` and presented nothing either.

/// ORIN-SUPSTATE — the module-owned console surface: the `Screen` + `Console` pair that was
/// stack-local to `jd2_console_pump` since JD2. `TargetPal` is a view, reconstructed per lock scope
/// (see the module header).
#[cfg(feature = "supstate")]
pub struct SupSurface {
    pub screen: crate::video::Screen,
    pub console: crate::console::Console,
    /// Adoption counter for arc 2's supervisor: bumped by `sup_install`, read back by nothing in
    /// arc 1. Starts at 0; the first install makes it 1.
    pub generation: u32,
}

/// ORIN-SUPSTATE — the handle. `None` until the pump's phase 2 installs the surface it built.
/// `spin::Mutex` in the `ORINWM1_STORE` mould; every access obeys the module header's discipline.
#[cfg(feature = "supstate")]
static SUP_SURFACE: spin::Mutex<Option<SupSurface>> = spin::Mutex::new(None);

/// ORIN-SUPSTATE — install (or re-install: adoption) the console surface into the module handle.
/// Called once from the pump's phase 2 today; arc 2's restart path would call it again, which is
/// why it returns the new generation instead of asserting on a prior tenant.
#[cfg(feature = "supstate")]
pub fn sup_install(screen: crate::video::Screen, console: crate::console::Console) -> u32 {
    loop {
        if let Some(mut guard) = SUP_SURFACE.try_lock() {
            let generation = guard.as_ref().map_or(0, |s| s.generation) + 1;
            *guard = Some(SupSurface { screen, console, generation });
            serial_println!(
                "[supstate] lift gen={} screen+console module-owned (task-stack no longer the sole holder) -> ADOPTABLE",
                generation
            );
            return generation;
        }
        crate::arch::sched::yield_now();
    }
}

/// ORIN-SUPSTATE — run `f` against the module-owned surface. Returns `None` iff no surface is
/// installed (phase 2 not reached, or a headless boot where phase 2 never runs); the lock itself is
/// always acquired eventually — `try_lock` + `yield_now`, per the cooperative-core rule above.
#[cfg(feature = "supstate")]
pub fn sup_with_surface<R>(
    f: impl FnOnce(&mut crate::video::Screen, &mut crate::console::Console) -> R,
) -> Option<R> {
    loop {
        if let Some(mut guard) = SUP_SURFACE.try_lock() {
            return match guard.as_mut() {
                Some(s) => Some(f(&mut s.screen, &mut s.console)),
                None => { SUP_NOSURF.fetch_add(1, core::sync::atomic::Ordering::Relaxed); None } // ORIN-SUPSOUND — the `None` arm is structurally unreachable from the roles (spawned after `sup_install`), so a nonzero `nosurf=` on the census is a real defect rather than a mode.
            };
        }
        SUP_SURF_WAIT.fetch_add(1, core::sync::atomic::Ordering::Relaxed); crate::arch::sched::yield_now(); // ORIN-SUPSOUND — count the refusal BEFORE giving the core back: `surfwait=` is the contention meter for the arc's one long hold (dispatcher across `handle_key`) and, under ORIN-BSPRUN, for a holder caught off-CPU by a quantum expiry.
    }
}

// ── ORIN-SUPSTATE milestone 2: the role seams ────────────────────────────────────────────────────
//
// Three task identities, one surface, two hand-off seams. The INPUT SOURCE (the original
// `jd2-console` task) pushes keys here and posts presentation work to the frame board; the
// DISPATCHER (`jd2-dispatch`) pops keys and runs the shell under the surface lock; the PRESENTER
// (`jd2-present`) drains the board and owns every flush to glass. Both seams are LEAF locks:
// pushed/popped with nothing else held, never held across a yield — so on this no-preemption core
// they are uncontended at every acquisition by construction, and the `try_lock` + `yield_now`
// discipline below is defence, not load-bearing.

/// ORIN-SUPSTATE — acquire a supstate lock under the cooperative-core rule: `try_lock`, and give
/// the core back on failure. Never a bare `lock()` spin (see the module header's livelock note).
#[cfg(feature = "supstate")]
fn sup_lock<T>(m: &spin::Mutex<T>) -> impl core::ops::DerefMut<Target = T> + '_ {
    loop {
        if let Some(g) = m.try_lock() {
            return g;
        }
        SUP_SEAM_WAIT.fetch_add(1, core::sync::atomic::Ordering::Relaxed); crate::arch::sched::yield_now(); // ORIN-SUPSOUND — the module header argues both LEAF seams are uncontended by construction on a cooperative core; `seamwait=` is that argument's falsifier, and a preemptive terminus is exactly what can falsify it.
    }
}

/// ORIN-SUPSTATE — the key seam, input source -> dispatcher. Capacity mirrors the PAL event ring's
/// depth (64): the input source checks `sup_key_full` BEFORE popping the PAL queue, so a key is
/// never popped-then-lost — when the dispatcher is 64 keys behind, events simply stay in the PAL
/// ring exactly as they do today while the monolithic pump is busy.
#[cfg(feature = "supstate")]
static SUP_KEYQ: spin::Mutex<alloc::collections::VecDeque<u8>> =
    spin::Mutex::new(alloc::collections::VecDeque::new());
#[cfg(feature = "supstate")]
const SUP_KEYQ_CAP: usize = 64;

/// ORIN-SUPSTATE — true when the key seam cannot take another key without exceeding its bound.
#[cfg(feature = "supstate")]
pub fn sup_key_full() -> bool {
    let full = sup_lock(&SUP_KEYQ).len() >= SUP_KEYQ_CAP;
    if full { SUP_KEY_BACKPRESS.fetch_add(1, core::sync::atomic::Ordering::Relaxed); } // ORIN-SUPSOUND — BACKPRESSURE, not loss: the keys stay in the PAL ring. It is nonetheless the ONLY observable that the dispatcher has fallen a full 64 keys behind, and the seam had no witness of any kind before this.
    full
}

/// ORIN-SUPSTATE — push one key for the dispatcher. Returns false (and drops nothing the caller
/// did not already hold) iff the bound would be exceeded — unreachable when the caller honours
/// `sup_key_full`, kept as a hard bound rather than an assumption.
#[cfg(feature = "supstate")]
pub fn sup_key_push(c: u8) -> bool {
    let mut q = sup_lock(&SUP_KEYQ);
    if q.len() >= SUP_KEYQ_CAP {
        SUP_KEY_DROPPED.fetch_add(1, core::sync::atomic::Ordering::Relaxed); // ORIN-SUPSOUND — THE DROP WITNESS. Unreachable while the caller honours `sup_key_full` before every pop, so this is a tripwire, not a mode: nonzero means a keystroke was LOST and the census raises `FAIL reason=key-dropped`.
        return false;
    }
    q.push_back(c);
    SUP_KEY_PUSHED.fetch_add(1, core::sync::atomic::Ordering::Relaxed); // ORIN-SUPSOUND — the seam's flow numerator; `pushed - popped` on the census is its standing depth.
    true
}

/// ORIN-SUPSTATE — pop one key as the dispatcher. FIFO; `None` when idle.
#[cfg(feature = "supstate")]
pub fn sup_key_pop() -> Option<u8> {
    let c = sup_lock(&SUP_KEYQ).pop_front();
    if c.is_some() { SUP_KEY_POPPED.fetch_add(1, core::sync::atomic::Ordering::Relaxed); } // ORIN-SUPSOUND — the seam's flow denominator; a dispatcher that stopped draining shows as a growing `depth=` long before the 64-deep bound is reached.
    c
}

/// ORIN-SUPSTATE — the frame board, input source / dispatcher -> presenter. A fixed-size
/// COALESCING board rather than a queue: relative motion accumulates, absolute position is
/// last-writer-wins, and the key-repaint mark is idempotent — exactly the per-frame coalescing the
/// legacy loop's `pending_rel`/`pending_abs`/`key_repainted` locals performed, made cross-task. So
/// the board is bounded by its own type and can never grow, drop, or reorder work.
#[cfg(feature = "supstate")]
#[derive(Default)]
pub struct SupFrameBoard {
    /// Accumulated relative pointer motion since the presenter's last pass.
    pub rel: Option<(i32, i32)>,
    /// Last absolute pointer position since the presenter's last pass (raw HID 0..=32767).
    pub abs: Option<(i32, i32)>,
    /// A key repainted the console into the back buffer; the presenter re-composites the cursor on
    /// top and presents.
    pub key_repaint: bool,
}

#[cfg(feature = "supstate")]
static SUP_FRAMES: spin::Mutex<SupFrameBoard> =
    spin::Mutex::new(SupFrameBoard { rel: None, abs: None, key_repaint: false });

/// ORIN-SUPSTATE — post one input frame's coalesced pointer activity (input source side).
#[cfg(feature = "supstate")]
pub fn sup_frame_pointer(rel: Option<(i32, i32)>, abs: Option<(i32, i32)>) {
    let mut b = sup_lock(&SUP_FRAMES);
    if let Some((dx, dy)) = rel {
        let (ax, ay) = b.rel.unwrap_or((0, 0));
        b.rel = Some((ax + dx, ay + dy));
    }
    if abs.is_some() {
        b.abs = abs;
    }
}

/// ORIN-SUPSTATE — mark that a key repainted the console (dispatcher side).
#[cfg(feature = "supstate")]
pub fn sup_frame_key_repaint() {
    sup_lock(&SUP_FRAMES).key_repaint = true;
}

/// ORIN-SUPSTATE — take the whole board (presenter side), leaving it empty.
#[cfg(feature = "supstate")]
pub fn sup_frame_take() -> SupFrameBoard {
    core::mem::take(&mut *sup_lock(&SUP_FRAMES))
}


// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// ORIN-LADDER — the two rungs orin 6 left owed. `orinladder`, DEFAULT OFF.
//
// orin-desktop.md §3.9.1 ("What this flight still did NOT establish") names exactly two items after
// boot7h, and neither has an instrument:
//
//   (a) **Glyphs-on-glass for win=2.** No probe reads the CONSOLE WINDOW's surface back off the
//       scanout. `[orinconwin] … -> ROUTED` proves the compositor believes it presented, and the
//       operator's ~107-minute use of the shell through the window is strong human evidence, but
//       neither is a READ-BACK. `[orinchrome]` closed exactly this question for rung 0's win=1 and
//       the same shape is owed here.
//   (b) **The dock round-trip.** `presses=0 raises=0 unhides=0` on every `[dock]` line of boot7h —
//       the minimise disc has never been pressed on this board. "The dock is a route back" is the
//       load-bearing premise of §6.1's ordering rule, and it has been exercised only as GEOMETRY
//       (`dock=GRANTED`), never as a gesture.
//
// WHAT THIS RUNG ADDS, AND WHAT IT DELIBERATELY DOES NOT. It adds NO behaviour: not one pixel path,
// not one routing decision, not one new control. Everything the round trip needs already ships —
// `wc_click_route`'s `minimise_hit` arm (rung 3), `wm::minimise`, `strip::press_route` ->
// `dock::press_at`, and `focus_changed`'s raise+unhide. What is missing is an ADJUDICATOR: a wire
// that says which half of the trip happened, and a read-back that says whether the half that
// happened reached the glass. This block is that adjudicator and nothing else.
//
// ⚠ NOT ONE LINE OF `video/` IS TOUCHED BY THIS RUNG, and that is a deliberate lane decision, not a
// convenience. The `hw-rmbp` track carries an unlanded ~1200-line `wm.rs`/`screen.rs` delta that
// meets this branch at the next sync, so every fact this instrument needs is taken through an
// EXISTING public accessor:
//   * the console row is found by `owner_asid == wm::KERNEL_OWNER_CONSOLE` (a public const) over
//     `wm::info(1..=MAX_WINDOWS)` — no new "which window is the console" accessor is minted;
//   * geometry comes from `wm::info`, the control disc from `wm::control_disc_rect`, the dock's
//     strip from `dock::strip_rect`, its tile model from `wm::dock_scan`, and what its last press
//     did from `dock::last_press_outcome`;
//   * the read-back is `FrameBuffer::read_pixel`, the compositor's own verify primitive and the one
//     place the read-back ban is lifted by name — `orin_chrome_probe`'s reason, verbatim.
// If a fact needed a new signature, a new field or a reordering in `wm.rs`, the rung would have
// stopped and asked rather than taken it. None did.
//
// WHY THE KNOB IMPLIES THE §6.1 CONJUNCTION, and why that is forced rather than chosen.
// `orinladder = ["orinconwin", "orinclick", "orindesk"]`. That set is not a preference: `orin_conwin`
// itself REFUSES to open a console window unless BOTH `orindesk` and `orinclick` are in the build
// (`[orinconwin] DECLINE reason=ordering-rule`), and this instrument's whole subject is that window
// and its minimise disc. A standalone `orinladder = []` would arm a probe for a window the build
// guarantees does not exist, and a `["orinconwin"]` would arm one for a window `orin_conwin`
// declines to open. `orinclick` is also what makes the DISC a gesture rather than a decoration, and
// `orinconwin` transitively supplies `desktop_firmware` (the `dock`/`strip` modules) and `tegra_el0` ->
// `tegra`. So the closure is the flight image, exactly — this is `orinclick = ["tegra_el0"]`'s
// argument applied one rung up.
//
// DEFAULT OFF AND MEASURED. With `orinladder` unset every item below vanishes and the two call sites
// in `main.rs` — both LINE-NEUTRAL in-line appends, never new source lines — compile to nothing. The
// ARMED polarity is type-checked by the `arm-tegra-ladder` leg of `KERNEL_CFG_MATRIX`.
//
// THE WIRE, in five families:
//   1. `[oringlass] arm …`      — ONCE from the terminus, the win=2 read-back's first sample.
//   2. `[oringlass] probe=… `   — six frame probes per sample, `[orinchrome]`'s six constants.
//   3. `[oringlass] phase=… `   — the terminal glyph verdict for one sample.
//   4. `[orindock] arm …`       — ONCE: the minimise disc's rect and the dock strip's rect, so an
//                                 attended flight knows WHERE to click without guessing.
//   5. `[orindock] park/restore/census …` — the round trip's two halves as they happen, plus the
//                                 ~10 s census that says which half is outstanding.
// -------------------------------------------------------------------------------------------------

/// ORIN-LADDER — arm latch, census bookkeeping and the CNTPCT reading at arm time. `orintenant`'s
/// shape, which is `orinclick`'s shape.
#[cfg(feature = "orinladder")]
static LAD_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "orinladder")]
static LAD_CENSUS_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "orinladder")]
static LAD_CENSUS_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinladder")]
static LAD_T0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// ORIN-LADDER — the round-trip ledger. `parks` and `restores` are edges of the console row's own
/// visibility predicate (`z > shell_z()`, the SAME one `dock::press_at` reports `raised=` from), not
/// a second notion of "minimised" invented here. `dock_restores` is the subset of restores taken
/// with the dock's last consumed press reading `raise` — a `<TAB>` back to the window is a restore
/// but it is NOT the dock round trip, and the census refuses to credit it as one. `blank_restores`
/// counts restores whose read-back found no glyphs: "a restore that paints nothing".
#[cfg(feature = "orinladder")]
static LAD_PARKS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinladder")]
static LAD_RESTORES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinladder")]
static LAD_DOCK_RESTORES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinladder")]
static LAD_BLANK_RESTORES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinladder")]
static LAD_GOOD_RESTORES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-LADDER — the last sampled visibility of the console row, as a tri-state so the FIRST sample
/// cannot manufacture an edge: 0 = no row seen yet (or the row is gone), 1 = on the panel,
/// 2 = parked below the shell. Every transition between 1 and 2 is an edge and prints.
#[cfg(feature = "orinladder")]
static LAD_LAST_VIS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-LADDER — the tick at which the row last entered the parked state, for the census's
/// `parked=Ns`. Informational: no verdict below is timer-driven (see `FAIL reason=park-no-tile`,
/// which is STRUCTURAL — an operator who is merely slow must never read as a failure).
#[cfg(feature = "orinladder")]
static LAD_PARK_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// ORIN-LADDER — census cadence in pump sweep ticks (~250 ms each): 40 ≈ 10 s, rung 3's number for
/// rung 3's reason. NOTE the SAMPLING cadence is every tick and only the PRINT is at this period —
/// see `orin_ladder_census` for why a 10 s sampler would miss the event this rung exists to witness.
#[cfg(feature = "orinladder")]
const LAD_CENSUS_PERIOD: u64 = 40;
/// ORIN-LADDER — read-back budget. The probe is ~1030 `read_pixel` calls; a park/restore storm must
/// not turn the UART and the scanout into a treadmill. The census reports the suppressed remainder.
#[cfg(feature = "orinladder")]
const LAD_GLASS_MAX: u32 = 32;
#[cfg(feature = "orinladder")]
static LAD_GLASS_TAKEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-LADDER — how often the CENSUS takes a read-back of its own, in censuses (6 ≈ 60 s).
///
/// WHY THE CENSUS SAMPLES AT ALL, when the arm already took one. **Rung (a) must not depend on rung
/// (b)'s gesture.** The arm sample is taken at the terminus, moments after `panel_console_window_open`
/// re-rendered the console into the new surface, so it sees whatever few lines had been printed by
/// then — and if that window happened to be near-empty the sample would read `BLANK-NO-GLYPHS` for a
/// TIMING reason rather than a defect, with no second opinion available until somebody minimised and
/// restored. A sample on the first census (seq 1, ~10 s later, with the boot's own tail in the
/// window) and one a minute after that removes that ambiguity: a genuine blank stays blank across all
/// of them, and an arm-time artefact resolves on the next line. `seq == 1` takes one by construction
/// (`(1 - 1) % 6 == 0`), which is the sample that matters most.
#[cfg(feature = "orinladder")]
const LAD_GLASS_PERIOD: u32 = 6;

/// ORIN-LADDER — the console face's PAPER and INK, as the values `fbcon` writes them.
///
/// `fbcon.rs:114-115` — `FG_DEFAULT = 0x00C0_C0C0` ("light grey text"), `BG_DEFAULT = 0x0000_0000`
/// ("black background"). Both are private `const`s in that module, so they are restated here rather
/// than reached for: importing them would mean a `pub` edit to `video/fbcon.rs`, which is precisely
/// the shared-seam edit this rung refuses to make. `orin_chrome_probe` restates `0x00FF_00FF` for
/// the same reason and with the same provenance note.
///
/// ⚠ THE FACE IS ANTI-ALIASED (`panel_console_face_arm` sets `c.aa = true`), so a glyph's EDGE
/// pixels are alpha blends of these two and equal NEITHER. That is why the census below counts three
/// things and not two: `paper` (exact background), `stem` (exact foreground — a fully covered pixel
/// inside a stroke) and `ink` (everything that is not paper, blends included). A verdict resting on
/// `ink` alone would be satisfied by any foreign window overlapping the box; `stem` is what makes it
/// this console's own text.
#[cfg(feature = "orinladder")]
const LAD_PAPER: u32 = 0x0000_0000;
#[cfg(feature = "orinladder")]
const LAD_INK: u32 = 0x00C0_C0C0;

// ── ORIN-GLASSINK — why the two constants above are not enough, and what was added ───────────────
//
// boot7j read `paper=1001 ink=23 stem=0 -> INK-NO-STEM`, stably, across at least four censuses, with
// all six chrome probes MATCH and `onpanel=yes`. That verdict was then read as "the text is not on
// the glass" — and it does not support that reading. `ink` was defined as "not exactly `LAD_PAPER`"
// and `stem` as "exactly `LAD_INK`", so `ink=23 stem=0` says only *23 samples were neither exact
// black nor exact light-grey*. THREE different worlds produce that same pair:
//
//   1. **Anti-aliased text of this console's own colour**, whose sampled pixels all happen to be
//      partial-coverage blends. The face IS anti-aliased (`c.aa = true`, note above) and CRYSTAL-HD/AA
//      widened the edge population, so this is not hypothetical. Rung (a) should PASS here.
//   2. **Text in a colour that is not `LAD_INK`** — the restated constant is stale, or the face was
//      armed with another `FG`. The instrument must NAME the measured colour so the constant can be
//      corrected as a decision, not by construction.
//   3. **Something that is not this console's text at all** — the original `INK-NO-STEM` reading, and
//      still the right verdict for the residue.
//
// The one datum that separates them was the one the probe threw away: it reported `first=`, the FIRST
// sample (which was paper), and never what the 23 ink samples WERE. So the census below now also
// partitions the non-paper population by GEOMETRY IN COLOUR SPACE and carries the heaviest values:
//
//   * `blend` — the sample lies on the straight PAPER→INK ramp (see `lad_classify`): a partial
//     coverage blend of exactly the two documented console colours, and nothing else on a
//     black-paper panel makes those by accident.
//   * `off`   — non-paper, not exactly ink, and NOT on that ramp: a foreign colour.
//   * `ink1..ink3` / `n1..n3` — the heaviest non-paper values with their counts, so a dominant
//     foreign colour is named on the wire rather than inferred.
//
// ⚠ `LAD_INK` IS DELIBERATELY NOT CHANGED to whatever the board shows. A constant tuned to the
// observation would make the probe agree with reality by construction and prove nothing; if the
// evidence says it is wrong, `INK-OFF-COLOUR` reports the measured value and correcting it is a
// separate decision on separate evidence.

/// ORIN-GLASSINK — per-channel slack, in 8-bit levels, for "this sample is on the PAPER→INK ramp".
///
/// A blend at coverage `a` is `paper + a*(ink - paper)` per channel; the compositor's own rounding,
/// and any gamma applied to the coverage, move a channel by a level or two. 8 of 192 (the ramp's span
/// on this pair) is ~4% — wide enough that no genuine blend is called foreign, narrow enough that a
/// saturated colour never passes: pure white fails on the dominant channel's range test, pure blue
/// misses the ramp by 48960 against a 1536 budget (both worked in `lad_classify`'s comment).
#[cfg(feature = "orinladder")]
const LAD_BLEND_TOL: i32 = 8;

/// ORIN-GLASSINK — slots in the ink histogram. Misra-Gries with `k` counters is guaranteed to retain
/// every value whose true frequency exceeds `n/(k+1)`, so 6 slots cannot lose a value that is even a
/// seventh of the ink population — and the verdict below only ever asks about a value holding a
/// MAJORITY of it. Fixed-size, on the stack, no allocation: 48 bytes.
#[cfg(feature = "orinladder")]
const LAD_TOPN: usize = 6;

/// ORIN-GLASSINK — the four populations a sample can fall in. Plain `u8` tags rather than an enum:
/// they are counted into an array and printed, and an enum would buy nothing here.
#[cfg(feature = "orinladder")]
const LAD_CLASS_PAPER: u8 = 0;
#[cfg(feature = "orinladder")]
const LAD_CLASS_STEM: u8 = 1;
#[cfg(feature = "orinladder")]
const LAD_CLASS_BLEND: u8 = 2;
#[cfg(feature = "orinladder")]
const LAD_CLASS_OFF: u8 = 3;

/// ORIN-GLASSINK — the ink population is RAMP-DOMINATED when blends are at least this fraction of it
/// (3/4). Not "all of it": a caret, a cursor sprite edge or one stale pixel from the desktop behind
/// must not demote a window full of anti-aliased text, and a supermajority is the cheapest statement
/// that survives those without admitting a genuinely mixed field.
#[cfg(feature = "orinladder")]
const LAD_RAMP_NUM: usize = 3;
#[cfg(feature = "orinladder")]
const LAD_RAMP_DEN: usize = 4;

/// ORIN-LADDER — the read-back grid: `LAD_BANDS` scanlines spread down the content box, each
/// sampling `LAD_RUNS` CONTIGUOUS runs of `LAD_RUN` pixels. 8 x 4 x 32 = 1024 samples.
///
/// WHY CONTIGUOUS RUNS AND NOT AN EVEN GRID. At the bench panel the content box is ~1900 px wide and
/// the routed cell is 7 px, so an evenly spread 64-point scanline samples about one pixel per four
/// character cells and can miss every stroke on a sparse line. A 32-pixel run crosses ~4.5 cells
/// end to end, and four of them spread across the width sample four independent stretches of the
/// same text row. The question "is there text here" wants LOCAL density and GLOBAL spread, and this
/// is the cheapest shape that has both.
#[cfg(feature = "orinladder")]
const LAD_BANDS: usize = 8;
#[cfg(feature = "orinladder")]
const LAD_RUNS: usize = 4;
#[cfg(feature = "orinladder")]
const LAD_RUN: usize = 32;

/// ORIN-LADDER — CNTPCT/CNTFRQ on the calling core. Duplicated from `ten_now_freq`/`clk_now_freq`
/// for their reason: those are gated on knobs this rung must not imply.
#[cfg(feature = "orinladder")]
fn lad_now_freq() -> (u64, u64) {
    let (now, freq): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, preserves_flags));
    }
    (now, freq)
}

/// ORIN-LADDER — **the console window's table row, found by OWNER.**
///
/// `panel_console_window_open` creates its row with `wm::KERNEL_OWNER_CONSOLE`, a public const, and
/// that owner names exactly one row: `close_owner` refuses the reserved kernel band, so nothing else
/// can mint one. Walking `wm::info` over the id space is O(MAX_WINDOWS) = 12 table lookups and needs
/// no new accessor in `wm.rs` — which is the point (see this block's header).
///
/// `None` means there is no console WINDOW: `orin_conwin` declined, the image is not the conjunction,
/// or the window was closed. Every caller below names that case rather than treating it as a failure
/// of the round trip.
#[cfg(feature = "orinladder")]
fn lad_console_info() -> Option<crate::video::wm::WindowInfo> {
    use crate::video::wm;
    for id in 1..=(wm::MAX_WINDOWS as wm::WinId) {
        if let Some(i) = wm::info(id) {
            if i.owner_asid == wm::KERNEL_OWNER_CONSOLE {
                return Some(i);
            }
        }
    }
    None
}

/// ORIN-LADDER — is the console row ON THE PANEL? The compositor's own predicate, spelled the way
/// `dock::press_at` spells it when it reports `raised=`: `z > shell_z()`. There is no separate
/// "minimised" flag on this system and none is invented here — `wm::minimise` parks a row by
/// dropping its `z` to zero, and this is the reading of that.
#[cfg(feature = "orinladder")]
fn lad_on_panel(i: &crate::video::wm::WindowInfo) -> bool {
    i.z > crate::video::wm::shell_z()
}

/// ORIN-GLASSINK — one 8-bit channel of an `0x00RRGGBB` sample. `c` is 0 = R, 1 = G, 2 = B.
#[cfg(feature = "orinladder")]
fn lad_chan(v: u32, c: usize) -> i32 {
    ((v >> (16 - 8 * c)) & 0xFF) as i32
}

/// **ORIN-GLASSINK — which population a read-back sample belongs to.**
///
/// The question this answers is the one `INK-NO-STEM` could not: *is this pixel a partial-coverage
/// blend of the console's own two colours, or is it a foreign colour?* An anti-aliased glyph edge at
/// coverage `a` is `paper + a*(ink - paper)` in EVERY channel with the SAME `a`, so the test is not
/// "is it grey" (which would hard-code this particular pair's greyness) but "is it ON THE SEGMENT
/// between the two documented constants": recover `a` from the channel with the widest span, then
/// require the other two to agree with it.
///
/// Written against `LAD_PAPER`/`LAD_INK` as variables, never against black: if either constant is
/// ever corrected the test follows it, and a `paper == ink` pair (no ramp) degrades to "everything
/// non-paper is off-ramp" rather than to a division by zero.
///
/// Integer only, no division: `got ≈ span_c * a / span_d` is checked cross-multiplied, which also
/// scales the tolerance by `|span_d|` exactly once. ~12 adds and 6 multiplies, on the NON-PAPER
/// samples only — paper is an equality test in the caller's fast path.
#[cfg(feature = "orinladder")]
fn lad_classify(v: u32) -> u8 {
    if v == LAD_PAPER {
        return LAD_CLASS_PAPER;
    }
    if v == LAD_INK {
        return LAD_CLASS_STEM;
    }
    // The ramp's dominant channel: the one whose paper→ink span is widest, so the recovered coverage
    // has the most resolution available. For the documented pair all three spans are 192.
    let mut d = 0usize;
    let mut dspan = 0i32;
    for c in 0..3 {
        let s = lad_chan(LAD_INK, c) - lad_chan(LAD_PAPER, c);
        if s.abs() > dspan.abs() {
            dspan = s;
            d = c;
        }
    }
    if dspan == 0 {
        // paper == ink in every channel: there is no segment, so nothing can be "between" them.
        return LAD_CLASS_OFF;
    }
    let a = lad_chan(v, d) - lad_chan(LAD_PAPER, d);
    // MONOTONE: `a` must run in the ramp's own direction and not past its far end. This is the test
    // pure white fails (a = 255 against a span of 192) — a brighter-than-ink pixel is not a blend of
    // anything, it is a different colour that happens to share the hue.
    if (a ^ dspan) < 0 || a.abs() > dspan.abs() {
        return LAD_CLASS_OFF;
    }
    for c in 0..3 {
        let s = lad_chan(LAD_INK, c) - lad_chan(LAD_PAPER, c);
        let got = lad_chan(v, c) - lad_chan(LAD_PAPER, c);
        if (got * dspan - s * a).abs() > LAD_BLEND_TOL * dspan.abs() {
            return LAD_CLASS_OFF;
        }
    }
    LAD_CLASS_BLEND
}

/// **ORIN-GLASSINK — the ink histogram: Misra-Gries over `LAD_TOPN` fixed slots.**
///
/// The census must name the heaviest non-paper values, and it may not allocate, may not sort 1024
/// samples and may not keep them. Misra-Gries is the standard answer: a value already in the table
/// increments its counter; a new value takes a free slot; a new value with the table full decrements
/// EVERY counter by one (freeing whatever reaches zero) and is dropped. The guarantee that matters:
/// any value with true frequency above `n/(LAD_TOPN+1)` is still in the table at the end.
///
/// ⚠ THE COUNTS ARE EXACT UNTIL THE FIRST DECREMENT ROUND, AND LOWER BOUNDS AFTER IT. `evict` is
/// carried to the wire as `exact=yes|no` for exactly that reason — a lower bound presented as a count
/// would be the same class of overclaim this whole rung exists to remove. And `exact=no` is itself
/// evidence: it means the box held more than `LAD_TOPN` distinct non-paper values, which is the
/// signature of a scattered field rather than of text in one colour.
#[cfg(feature = "orinladder")]
fn lad_hist_add(
    hv: &mut [u32; LAD_TOPN],
    hc: &mut [u32; LAD_TOPN],
    distinct: &mut u32,
    evict: &mut u32,
    v: u32,
) {
    for s in 0..LAD_TOPN {
        if hc[s] != 0 && hv[s] == v {
            hc[s] += 1;
            return;
        }
    }
    for s in 0..LAD_TOPN {
        if hc[s] == 0 {
            hv[s] = v;
            hc[s] = 1;
            *distinct += 1;
            return;
        }
    }
    *evict += 1;
    for s in 0..LAD_TOPN {
        hc[s] -= 1;
    }
}

/// ORIN-GLASSINK — the `rank`-th heaviest slot (0-based) as `(value, count)`, or `(0, 0)` when the
/// table holds fewer than `rank + 1` occupied slots. `LAD_PAPER` is never admitted to the table, so a
/// `(0, 0)` answer is unambiguous on the wire: `n=0` means "no such entry".
///
/// A selection scan rather than a sort: `LAD_TOPN` is 6 and only three ranks are printed, so this is
/// 18 comparisons against the cost of moving the table around. Ties break toward the lower slot,
/// which is the earlier-first-seen value.
#[cfg(feature = "orinladder")]
fn lad_hist_rank(hv: &[u32; LAD_TOPN], hc: &[u32; LAD_TOPN], rank: usize) -> (u32, u32) {
    let mut taken = [false; LAD_TOPN];
    let mut out = (0u32, 0u32);
    for _ in 0..=rank {
        let mut best = LAD_TOPN;
        for s in 0..LAD_TOPN {
            if taken[s] || hc[s] == 0 {
                continue;
            }
            if best == LAD_TOPN || hc[s] > hc[best] {
                best = s;
            }
        }
        if best == LAD_TOPN {
            return (0, 0);
        }
        taken[best] = true;
        out = (hv[best], hc[best]);
    }
    out
}

/// ORIN-GLASSINK — does this `orin_glass_probe` verdict mean THIS CONSOLE'S TEXT reached the glass?
///
/// The single place that question is answered, because rung (b)'s ledger asks it too: `restore-blank`
/// is derived from it, and a verdict added to the passing set without updating that derivation would
/// turn a healthy anti-aliased restore into `FAIL reason=restore-blank`. Listed explicitly rather
/// than matched on the `GLYPHS-` prefix: an adjudicator that admits verdicts it has never seen is not
/// an adjudicator. Adding a passing verdict means adding it HERE, and the compiler will not remind
/// you — this note is the reminder.
#[cfg(feature = "orinladder")]
fn lad_glass_painted(v: &str) -> bool {
    matches!(
        v,
        "GLYPHS-ON-GLASS" | "GLYPHS-NO-CHROME" | "GLYPHS-AA-ON-GLASS" | "GLYPHS-AA-NO-CHROME"
    )
}

/// **ORIN-GLASS — rung (a): the win=2 GLYPHS-ON-GLASS read-back.**
///
/// The question, stated as the error it prevents. After boot7h the console window's evidence is
/// `[orinconwin] … present=Composited … -> ROUTED` plus an operator who typed at it for 107 minutes.
/// The first is the COMPOSITOR'S BELIEF — `present_outcome` reports that the pass ran, and this
/// panel is a DRAM carveout scanned out by a block that does not snoop, so the trailing `flush_rect`
/// is a real step that can fail on its own. The second is a human reading glass in a room, which is
/// not a capture. §3.9.1 asked for "a read-back instrument … the way `[orinchrome]` closed rung 0's",
/// and this is it, one rung up and with a different discriminator.
///
/// WHY THE DISCRIMINATOR IS INVERTED FROM `[orinchrome]`'s. That probe knew a constant it could
/// compare against inside the CONTENT (the magenta block `orin_wm1` writes) and used it to
/// discriminate "chrome missing" from "nothing landed". Here the content is TEXT: nobody can predict
/// which glyph is at any coordinate, and the face is anti-aliased, so no single pixel carries an
/// exact expectation. What IS predictable is the POPULATION — a text surface on the glass shows the
/// console's own paper AND fully covered strokes of its own ink, and no other object on this panel
/// shows that pair. So the roles swap: the CENSUS answers the rung's question, and the six FRAME
/// constants (`[orinchrome]`'s own, re-derived from this row's box) are the discriminator that
/// separates "the window is not there" from "the window is there and its text is not".
///
/// WHAT IS SAMPLED. `LAD_BANDS` scanlines down the content box, `LAD_RUNS` contiguous runs of
/// `LAD_RUN` pixels each; the content box is `wm::info`'s own `(x, y)` and `w*scale` by `h*scale`,
/// so this follows the placer at any integer scale rather than assuming the bench panel's 1. Each
/// sample is classified against the two documented console colours (see `LAD_PAPER`/`LAD_INK`).
///
/// THE VERDICT LADDER — first match wins, every arm reachable, none constant:
///
/// | verdict | what the wire is saying |
/// | --- | --- |
/// | `DECLINE reason=no-console-row` | no `wm` row is owned by `KERNEL_OWNER_CONSOLE`. Not a failure of this rung: the image is not the conjunction, or `orin_conwin` declined and named its own reason above |
/// | `DECLINE reason=no-panel` | headless boot — there is no scanout to read back |
/// | `UNREADABLE` | every sample fell outside the mapped length: the row's geometry and the panel's disagree. A defect, and one no present count could show |
/// | `WIN2-NOT-ON-GLASS` | not ONE sample is the console's background. Whatever occupies those panel coordinates, it is not this window's surface |
/// | `BLANK-NO-GLYPHS` | every sample IS the background: the surface reached the glass and its TEXT did not. The exact shape §3.9.1 could not rule out, and the one a present count can never see |
/// | `GLYPHS-AA-NO-CHROME` | no fully covered stroke, but the ink population is a supermajority of PAPER→INK blends AT TWO OR MORE COVERAGE LEVELS: anti-aliased text of this console's own colour, with the frame overdrawn |
/// | `GLYPHS-AA-ON-GLASS` | the same blend supermajority with the frame intact. **Rung (a) closed on anti-aliased evidence** — `stem=0` here is a property of the face, not a defect |
/// | `INK-FLAT-FILL` | a blend supermajority at exactly ONE level (`blevels=1`): a flat fill of a colour that happens to sit on the ramp, not text. `video::PANEL_BG` (`0x001E_1E1E`) is such a colour, so this is the arm that stops the desktop showing through from reading as a pass |
/// | `INK-OFF-COLOUR` | no stroke, no blend supermajority, and ONE off-ramp value holds a majority of the ink population. Text (or a fill) in a colour that is not `LAD_INK`; `ink1=` names the measured value. A finding about the CONSTANT, reported and not silently adopted |
/// | `INK-NO-STEM` | non-paper pixels inside the box, but not one fully covered stroke of the console's own ink, no blend supermajority and no dominant colour: scattered foreign values — something is over the content that is not this console's text |
/// | `GLYPHS-NO-CHROME` | the console's paper and its ink strokes are both on the glass, and the FRAME is not. Text landed, the window's own chrome was overdrawn — §3.8.1's measured JD2-blit overdraw, caught in the act |
/// | `GLYPHS-ON-GLASS` | frame and glyphs both read back at panel coordinates. **This is rung (a) closed** |
///
/// THE INK FIELDS, and the reading each one settles. `paper + ink == read` and
/// `stem + blend + off == ink` hold on every line, so a line that does not balance is a defect in the
/// instrument and not in the panel. `blend` counts samples on the PAPER→INK segment (`lad_classify`),
/// `blevels` is `0`/`1`/`2+` — how many DISTINCT blend levels were seen, which is what separates
/// anti-aliased text (several) from a flat fill of a ramp colour (exactly one).
/// `off` counts non-paper values that are not on it, and `ink1..ink3`/`n1..n3` are the heaviest
/// non-paper values with their counts (`n=0` = no such entry; `inkvals`/`exact` say whether the
/// histogram had to start decrementing — see `lad_hist_add`).
///
/// SAFETY AND COST. `read_pixel` is bounds-checked against the mapped length and answers `None`
/// off-panel. The `wm` reads all happen BEFORE `WRITER` is taken and its guard is dropped in a single
/// statement, so no window-table lock is ever held under the framebuffer lock (ORIN-WM1's acyclic
/// rule, and `orin_chrome_probe`'s own discipline). ~1030 reads, budgeted by `LAD_GLASS_MAX`.
#[cfg(feature = "orinladder")]
pub fn orin_glass_probe(phase: &str) -> &'static str {
    use crate::video::{theme, wm};

    // Read the TABLE first and hold nothing: `WRITER` is taken below and the two locks must never
    // nest in this direction.
    let Some(i) = lad_console_info() else {
        serial_println!(
            "[oringlass] phase={} -> DECLINE reason=no-console-row (no wm row carries KERNEL_OWNER_CONSOLE — this image opened no console window; orin_conwin named its own reason on its own line)",
            phase
        );
        return "DECLINE";
    };

    // The outer box, re-derived from the row exactly as `panel_console_window_open` built it:
    // `create_at` was handed the CONTENT origin (`ox + BORDER`, `oy + TITLE_H + BORDER`), so the
    // frame's own origin is that minus the same two terms. `w`/`h` are SOURCE pixels; the panel
    // extent is `* scale`.
    let cwp = i.w.saturating_mul(i.scale);
    let chp = i.h.saturating_mul(i.scale);
    let ox = i.x.saturating_sub(wm::BORDER);
    let oy = i.y.saturating_sub(wm::TITLE_H + wm::BORDER);
    let ow = cwp + 2 * wm::BORDER;
    let oh = chp + wm::TITLE_H + 2 * wm::BORDER;
    let on_panel = lad_on_panel(&i);

    let fb = *crate::video::WRITER.lock();
    if !fb.is_ready() {
        serial_println!("[oringlass] phase={} win={} -> DECLINE reason=no-panel", phase, i.id);
        return "DECLINE";
    }

    // THE DISCRIMINATOR — `[orinchrome]`'s six frame constants, at the box's mid-edges (which clear
    // `theme::CORNER_RADIUS` at both ends by construction). Deliberately NOT machined by `ceramic`:
    // `paint_window` documents the keyline and the two bevel hairlines as exact theme values,
    // because "a single-pixel edge has no room to show a grain".
    let kw = theme::BEVEL;
    let mx = ox + ow / 2;
    let my = oy + oh / 2;
    let probes: [(&str, usize, usize, u32); 6] = [
        ("kl_top", mx, oy, theme::FRAME_LINE),
        ("kl_bot", mx, oy + oh.saturating_sub(kw), theme::FRAME_LINE),
        ("kl_left", ox, my, theme::FRAME_LINE),
        ("kl_right", ox + ow.saturating_sub(kw), my, theme::FRAME_LINE),
        ("bev_lt", mx, oy + kw, theme::BEVEL_LIGHT),
        ("bev_sh", mx, oy + oh.saturating_sub(kw + theme::BEVEL), theme::BEVEL_SHADOW),
    ];
    let mut fhit = 0usize;
    let mut fread = 0usize;
    for (name, x, y, want) in probes.iter() {
        match fb.read_pixel(*x, *y) {
            Some(got) => {
                fread += 1;
                if got == *want {
                    fhit += 1;
                }
                serial_println!(
                    "[oringlass] probe={} at ({},{}) got={:#08x} want={:#08x} -> {}",
                    name, x, y, got, want,
                    if got == *want { "MATCH" } else { "MISS" }
                );
            }
            None => serial_println!(
                "[oringlass] probe={} at ({},{}) -> UNMAPPED (off-panel, or past the mapped length)",
                name, x, y
            ),
        }
    }

    // THE GLYPH CENSUS. Contiguous runs, spread — see `LAD_BANDS` for why that shape.
    let mut read = 0usize;
    let mut paper = 0usize;
    let mut ink = 0usize;
    let mut stem = 0usize;
    // ORIN-GLASSINK — the three populations `ink` used to hide, and the heaviest values in it.
    let mut blend = 0usize;
    let mut off = 0usize;
    // ORIN-GLASSINK — how many DISTINCT blend levels were seen, as the only distinction that matters:
    // one, or more than one. See `blend_multi`'s use in `ramp` below for why a single level is not
    // evidence of text.
    let mut blend_first: u32 = 0;
    let mut blend_seen = false;
    let mut blend_multi = false;
    let mut hv = [0u32; LAD_TOPN];
    let mut hc = [0u32; LAD_TOPN];
    let mut hdistinct = 0u32;
    let mut hevict = 0u32;
    let mut first: u32 = 0;
    let mut got_first = false;
    let mut uniform = true;
    for b in 0..LAD_BANDS {
        // The vertical centre of the b-th of LAD_BANDS equal bands of the content.
        let y = i.y + (2 * b + 1) * chp / (2 * LAD_BANDS);
        for r in 0..LAD_RUNS {
            // The run's centre at the r-th of LAD_RUNS equal columns, backed off by half a run and
            // clamped so a narrow content box cannot walk the run past its own right edge.
            let centre = (2 * r + 1) * cwp / (2 * LAD_RUNS);
            let start = centre.saturating_sub(LAD_RUN / 2).min(cwp.saturating_sub(LAD_RUN));
            for k in 0..LAD_RUN {
                let x = i.x + start + k;
                if let Some(v) = fb.read_pixel(x, y) {
                    read += 1;
                    if !got_first {
                        first = v;
                        got_first = true;
                    } else if v != first {
                        uniform = false;
                    }
                    // ORIN-GLASSINK — one classification per sample, and every counter derived from
                    // it. `paper`/`ink`/`stem` keep the exact meanings the wire has always given
                    // them (`ink` is still "not exactly paper", strokes and blends included); the
                    // arms below only split what `ink` was already counting.
                    match lad_classify(v) {
                        LAD_CLASS_PAPER => paper += 1,
                        LAD_CLASS_STEM => {
                            ink += 1;
                            stem += 1;
                            lad_hist_add(&mut hv, &mut hc, &mut hdistinct, &mut hevict, v);
                        }
                        LAD_CLASS_BLEND => {
                            ink += 1;
                            blend += 1;
                            if !blend_seen {
                                blend_first = v;
                                blend_seen = true;
                            } else if v != blend_first {
                                blend_multi = true;
                            }
                            lad_hist_add(&mut hv, &mut hc, &mut hdistinct, &mut hevict, v);
                        }
                        _ => {
                            ink += 1;
                            off += 1;
                            lad_hist_add(&mut hv, &mut hc, &mut hdistinct, &mut hevict, v);
                        }
                    }
                }
            }
        }
    }

    // ORIN-GLASSINK — the two readings the ink population can support on its own, computed BEFORE the
    // ladder so each one is a named statement rather than an expression buried in an `else if`.
    //
    // `ramp`: blends are a supermajority of the ink (`LAD_RAMP_NUM/DEN`) and there is at least one.
    // On a black-paper panel nothing but a coverage blend of these two colours lands on that segment,
    // so this is positive evidence of THIS console's own anti-aliased text — not merely the absence
    // of a counter-example.
    //
    // `offdom`: the single heaviest non-paper value is OFF the ramp and holds a strict majority of
    // the whole ink population. Majority-of-ink rather than majority-of-`off` on purpose: under
    // Misra-Gries the count is a lower bound (`exact=no`), and a bound that clears the larger
    // denominator clears the smaller one too, so the arm can never fire on a value that merely
    // survived the table.
    let (i1v, i1c) = lad_hist_rank(&hv, &hc, 0);
    let (i2v, i2c) = lad_hist_rank(&hv, &hc, 1);
    let (i3v, i3c) = lad_hist_rank(&hv, &hc, 2);
    //
    // ⚠ `blend_multi` IS LOAD-BEARING, and a host run of this file's own `lad_classify` is what found
    // out why. `video::PANEL_BG` is `0x001E_1E1E` — a GREY, therefore ON the black→light-grey ramp,
    // therefore a "blend" by the segment test. A box holding some paper and a lot of desktop would
    // otherwise clear the supermajority and read as a PASS. What separates a flat fill from
    // anti-aliased text is not the colour, it is the NUMBER OF COVERAGE LEVELS: a fill has exactly
    // one, and glyph edges sampled across many strokes have several. Requiring two distinct blend
    // values costs two locals and closes the only false-PASS path this rung has.
    let ramp = blend != 0 && blend_multi && blend * LAD_RAMP_DEN >= ink * LAD_RAMP_NUM;
    let flat = blend != 0 && !blend_multi && blend * LAD_RAMP_DEN >= ink * LAD_RAMP_NUM;
    let offdom = i1c != 0 && (i1c as usize) * 2 > ink && lad_classify(i1v) == LAD_CLASS_OFF;

    // DERIVED, never asserted — the whole point of the line.
    let verdict = if read == 0 {
        "UNREADABLE"
    } else if paper == 0 {
        "WIN2-NOT-ON-GLASS"
    } else if paper == read {
        "BLANK-NO-GLYPHS"
    } else if stem == 0 {
        // ORIN-GLASSINK — the bucket that used to be one answer and was three. Order matters:
        // `ramp` is checked first because a blend supermajority is a POSITIVE identification of this
        // console's colours and outranks any statement about a dominant foreign value.
        if ramp {
            if fread != 0 && fhit == fread {
                "GLYPHS-AA-ON-GLASS"
            } else {
                "GLYPHS-AA-NO-CHROME"
            }
        } else if flat {
            "INK-FLAT-FILL"
        } else if offdom {
            "INK-OFF-COLOUR"
        } else {
            "INK-NO-STEM"
        }
    } else if fread != 0 && fhit == fread {
        "GLYPHS-ON-GLASS"
    } else {
        "GLYPHS-NO-CHROME"
    };
    serial_println!(
        "[oringlass] phase={} win={} box={}x{} at ({},{}) content={}x{} at ({},{}) scale={} onpanel={} frame={}/{} samples={} read={} paper={} ink={} stem={} blend={} blevels={} off={} ink1={:#010x} n1={} ink2={:#010x} n2={} ink3={:#010x} n3={} inkvals={} exact={} first={:#08x} uniform={} -> {}",
        phase, i.id, ow, oh, ox, oy, cwp, chp, i.x, i.y, i.scale,
        if on_panel { "yes" } else { "PARKED" },
        fhit, fread,
        LAD_BANDS * LAD_RUNS * LAD_RUN,
        read, paper, ink, stem, blend,
        if !blend_seen {
            "0"
        } else if blend_multi {
            "2+"
        } else {
            "1"
        },
        off,
        i1v, i1c, i2v, i2c, i3v, i3c,
        hdistinct,
        if hevict == 0 { "yes" } else { "no" },
        first,
        if got_first && uniform { "yes" } else { "no" },
        verdict
    );
    verdict
}

/// ORIN-LADDER — the read-back with its budget applied. Returns the verdict, or `"BUDGET"` once the
/// budget is spent: a suppressed sample must not look like a passing one, so the census carries both
/// the taken count and the suppressed remainder rather than silently thinning the evidence.
#[cfg(feature = "orinladder")]
fn lad_glass_budgeted(phase: &str) -> &'static str {
    use core::sync::atomic::Ordering;
    if LAD_GLASS_TAKEN.fetch_add(1, Ordering::Relaxed) >= LAD_GLASS_MAX {
        return "BUDGET";
    }
    orin_glass_probe(phase)
}

/// **ORIN-LADDER — the arming point.** Appended to `tegra_early_stop`'s terminus line (ZERO source
/// lines added — the tegra knob-off byte-identity constraint), AFTER `orin_conwin`'s and
/// `orin_tenant_arm`'s statements so the console row this rung is entirely about is already in the
/// table when the first sample is taken.
///
/// Three jobs, in order:
///   1. Take the FIRST win=2 read-back, labelled `phase=arm`. It is the baseline every later sample
///      is read against, and on its own it is already rung (a)'s answer for the boot's opening state.
///   2. Print WHERE THE OPERATOR MUST CLICK. `wm::control_disc_rect(id, Ctrl::Minimise)` is the
///      painter's own accessor — the disc the compositor actually drew, never a re-derivation — and
///      `dock::strip_rect` is the registry hook `wm::erase_clip` reads, so the strip named here is
///      the strip that will be painted. An attended flight that has to GUESS at a 24-px disc on a
///      1920x1200 panel is a flight that reports "nothing happened" when the truth was "you missed".
///   3. Seed the visibility tri-state so the first census tick cannot manufacture a park edge out of
///      the boot's opening state.
///
/// Every decline is named and none is fatal: this rung reads, it never acts.
#[cfg(feature = "orinladder")]
pub fn orin_ladder_arm() {
    use crate::video::{dock, wm};
    use core::sync::atomic::Ordering;

    let (now, _) = lad_now_freq();
    LAD_T0.store(now, Ordering::Relaxed);

    // Rung (a), first sample.
    let glass = lad_glass_budgeted("arm");

    // Rung (b), the arm line. Panel first, and the guard dropped in one statement.
    let (pw, ph, ready) = {
        let fb = *crate::video::WRITER.lock();
        if fb.is_ready() {
            let info = fb.info();
            (info.width, info.height, true)
        } else {
            (0, 0, false)
        }
    };
    if !ready {
        serial_println!("[orindock] arm -> DECLINE reason=no-panel (headless boot — JD1 seeded no scanout; there is no dock, no disc and nothing to park)");
        LAD_ARMED.store(true, Ordering::Release);
        return;
    }

    let info = lad_console_info();
    let disc = info
        .as_ref()
        .and_then(|i| wm::control_disc_rect(i.id, wm::Ctrl::Minimise));
    let strip = dock::strip_rect(pw, ph);
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let (tiles, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));

    // The tri-state seed. `None` leaves it at 0 (no row seen), which is exactly right.
    if let Some(ref i) = info {
        LAD_LAST_VIS.store(if lad_on_panel(i) { 1 } else { 2 }, Ordering::Relaxed);
    }

    let verdict = if info.is_none() {
        // NOT a failure of this rung — and it must not read as one. `orin_conwin` declines on a
        // non-conjunction image and says so on its own line; this rung then has no subject.
        "DECLINE reason=no-console-row"
    } else if disc.is_none() {
        // A console window with no minimise disc would make §6.1's whole ordering rule moot and this
        // rung unrunnable. `ctrls_for` decides the row's cluster, so this is the wire asking it.
        "DECLINE reason=no-minimise-disc"
    } else if strip.is_none() {
        // `dock::Layout::for_panel` refused the strip on this panel — the CONSOLEWIN law's geometry
        // half, which `orin_conwin` also tests before opening. Reaching here means the panel changed
        // under the boot; a park would be a one-way trip and the wire says so BEFORE one is taken.
        "DECLINE reason=no-dock-strip"
    } else {
        "ARMED"
    };
    let (dx, dy, dd) = disc.unwrap_or((0, 0, 0));
    let (sx, sy, sw, sh) = strip.unwrap_or((0, 0, 0, 0));
    serial_println!(
        "[orindock] arm panel={}x{} win={} disc=({},{},{}) strip=({},{},{}x{}) tiles={} glass={} orinconwin={} orinclick={} orindesk={} pidesk={} -> {}",
        pw, ph,
        info.as_ref().map(|i| i.id).unwrap_or(wm::WIN_NONE),
        dx, dy, dd, sx, sy, sw, sh, tiles, glass,
        cfg!(feature = "orinconwin") as u8, cfg!(feature = "orinclick") as u8,
        cfg!(feature = "orindesk") as u8, cfg!(feature = "desktop_firmware") as u8,
        verdict
    );
    LAD_ARMED.store(true, Ordering::Release);
}

/// **ORIN-LADDER — rung (b): the DOCK ROUND TRIP, sampled every tick and adjudicated every ~10 s.**
///
/// Called from `jd2_console_pump`'s phase-2 idle cadence (`main.rs`), on the same ~250 ms sweep tick
/// rung 3's and rung 6's censuses ride, so a stalled pump produces no line at all rather than a
/// stale reassuring one.
///
/// ⚠ WHY THIS SAMPLES EVERY TICK WHERE RUNGS 3 AND 6 SAMPLE ONLY ON THE PRINTING PASS. Those
/// censuses read COUNTERS, which are monotone: a 10 s cadence loses timing, never events. This one
/// reads a STATE — the console row's `z` — and the event it exists to witness is a park followed by
/// a restore, which an operator completes in a couple of seconds. A 10 s sampler would see the row
/// on the panel, then on the panel again, and report `IDLE-NEVER-PARKED` for a round trip that
/// actually happened. So the EDGE DETECTOR runs every tick (one `wm::info` walk, ~12 table lookups
/// under one lock, ~4/s) and only the CENSUS PRINT is at the 10 s period. That is a strictly smaller
/// footprint than the `wm::hit_test` rung 3 already takes per pointer event.
///
/// THE TWO HALVES, each with its own line, printed the tick they happen:
///
///   * `[orindock] park …` — the row's `z` dropped below the shell. That is `wm::minimise`'s park
///     and nothing else can produce it. The line carries whether the dock's tile model contains the
///     row, because THAT is the question §6.1 is about: a park with no tile is the one-way trip the
///     ordering rule exists to forbid, and it is named at the moment it is taken.
///   * `[orindock] restore …` — the row came back above the shell. `via=` is derived from
///     `dock::last_press_outcome()`: a `raise` is the dock's own tile press, anything else is a
///     restore by some other route (`<TAB>`, a focus change) and is NOT credited as the round trip.
///     `glass=` is rung (a)'s read-back re-fired on the restored window, which is what makes
///     "a restore that paints nothing" a DIFFERENT LINE from a restore that paints.
///
/// THE CENSUS VERDICT LADDER — first match wins, every arm reachable, none constant:
///
/// | verdict | what the wire is saying |
/// | --- | --- |
/// | `DECLINE reason=no-console-row` | there is no console window on this image. No subject; not a failure |
/// | `DECLINE reason=no-dock-strip` | the panel cannot host the strip. There is no way back, so the trip is not merely untaken, it is impossible |
/// | `FAIL reason=park-no-tile` | the row is parked NOW and the dock's tile model does not contain it. **The one-way trip, realised.** Structural, not timed — a slow operator never reads as this |
/// | `FAIL reason=restore-blank` | the row came back and every read-back said the content did not paint. "A restore that paints nothing" |
/// | `PARKED-AWAITING-DOCK` | parked, a tile names it, nobody has pressed it yet. The honest in-flight state |
/// | `DOCK-ROUNDTRIP` | a dock tile press brought it back AND the read-back found its glyphs on the glass. **This is rung (b) closed** |
/// | `RESTORED-NOT-VIA-DOCK` | it came back, but not through a dock tile. The round trip is NOT closed and the wire refuses to pretend it is |
/// | `IDLE-NEVER-PARKED` | the minimise disc has not been pressed. **UNRUN, never PASS** — boot7h's state, and it must stay distinguishable from a passing one |
#[cfg(feature = "orinladder")]
pub fn orin_ladder_census(tick: u64) {
    use crate::video::{dock, wm};
    use core::sync::atomic::Ordering;

    if !LAD_ARMED.load(Ordering::Acquire) {
        return;
    }

    // ── the EDGE DETECTOR, every tick ────────────────────────────────────────────────────────────
    let info = lad_console_info();
    let now_vis = match info {
        Some(ref i) => {
            if lad_on_panel(i) {
                1u32
            } else {
                2u32
            }
        }
        None => 0u32,
    };
    let was = LAD_LAST_VIS.swap(now_vis, Ordering::Relaxed);
    if let Some(ref i) = info {
        if was == 1 && now_vis == 2 {
            // PARK. Ask the dock's tile model for the way back AT THE MOMENT OF THE PARK — the
            // answer is what §6.1's rule is about, and asking later would let a table change hide it.
            let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
            let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
            let tiled = rows[..n].iter().any(|r| r.id == i.id);
            LAD_PARKS.fetch_add(1, Ordering::Relaxed);
            LAD_PARK_TICK.store(tick, Ordering::Relaxed);
            serial_println!(
                "[orindock] park win={} z={} shellz={} tiles={} tiled={} t={} -> {}",
                i.id, i.z, wm::shell_z(), n, tiled as u8, tick,
                if tiled { "PARKED" } else { "PARKED-NO-WAY-BACK" }
            );
        } else if was == 2 && now_vis == 1 {
            // RESTORE. `via` first (the dock's latch is read before anything below can disturb it),
            // then the read-back on the window as it now stands.
            let via_dock = dock::last_press_outcome() == "raise";
            LAD_RESTORES.fetch_add(1, Ordering::Relaxed);
            if via_dock {
                LAD_DOCK_RESTORES.fetch_add(1, Ordering::Relaxed);
            }
            let glass = lad_glass_budgeted("restore");
            // ORIN-GLASSINK — through `lad_glass_painted`, the ONE place "this console's text
            // reached the glass" is decided, so the anti-aliased verdicts cannot be read here as a
            // restore that painted nothing. `INK-OFF-COLOUR` deliberately stays OUTSIDE the painted
            // set: a foreign colour in the box is not a confirmation of this console's text, and the
            // `glass=` field on the line below names which of the two shapes it was.
            let painted = lad_glass_painted(glass);
            if painted {
                LAD_GOOD_RESTORES.fetch_add(1, Ordering::Relaxed);
            } else if glass != "BUDGET" && glass != "DECLINE" {
                LAD_BLANK_RESTORES.fetch_add(1, Ordering::Relaxed);
            }
            let parked_ticks = tick.wrapping_sub(LAD_PARK_TICK.load(Ordering::Relaxed));
            serial_println!(
                "[orindock] restore win={} z={} shellz={} via={} dockpress={} parked={}t glass={} t={} -> {}",
                i.id, i.z, wm::shell_z(),
                if via_dock { "dock" } else { "other" },
                dock::last_press_outcome(), parked_ticks, glass, tick,
                match (via_dock, painted) {
                    (true, true) => "RESTORED",
                    (true, false) => "RESTORED-BLANK",
                    (false, true) => "RESTORED-OFF-DOCK",
                    (false, false) => "RESTORED-OFF-DOCK-BLANK",
                }
            );
        }
    }

    // ── the CENSUS PRINT, every LAD_CENSUS_PERIOD ticks ──────────────────────────────────────────
    if tick.wrapping_sub(LAD_CENSUS_TICK.load(Ordering::Relaxed)) < LAD_CENSUS_PERIOD {
        return;
    }
    LAD_CENSUS_TICK.store(tick, Ordering::Relaxed);
    let seq = LAD_CENSUS_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let (now, freq) = lad_now_freq();
    let up = if freq == 0 {
        0
    } else {
        now.wrapping_sub(LAD_T0.load(Ordering::Relaxed)) / freq
    };

    let parks = LAD_PARKS.load(Ordering::Relaxed);
    let restores = LAD_RESTORES.load(Ordering::Relaxed);
    let dock_restores = LAD_DOCK_RESTORES.load(Ordering::Relaxed);
    let blanks = LAD_BLANK_RESTORES.load(Ordering::Relaxed);
    let good = LAD_GOOD_RESTORES.load(Ordering::Relaxed);
    let taken = LAD_GLASS_TAKEN.load(Ordering::Relaxed);
    let suppressed = taken.saturating_sub(LAD_GLASS_MAX.min(taken));

    let (pw, ph) = {
        let fb = *crate::video::WRITER.lock();
        if fb.is_ready() {
            (fb.width(), fb.height())
        } else {
            (0, 0)
        }
    };
    let strip = dock::strip_rect(pw, ph);
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let (tiles, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
    let tiled = info
        .as_ref()
        .map(|i| rows[..tiles].iter().any(|r| r.id == i.id))
        .unwrap_or(false);
    let parked_now = now_vis == 2;

    // Rung (a)'s own cadence, independent of any click — see `LAD_GLASS_PERIOD`. A parked row is
    // NOT probed: its content box holds whatever is behind it, so the read-back would answer a
    // question about the desktop and print `WIN2-NOT-ON-GLASS` for a window that is correctly hidden.
    let glass = if now_vis == 1 && (seq - 1) % LAD_GLASS_PERIOD == 0 {
        lad_glass_budgeted("census")
    } else if now_vis == 2 {
        "parked"
    } else {
        "skipped"
    };

    let verdict = if info.is_none() {
        "DECLINE reason=no-console-row"
    } else if strip.is_none() {
        "DECLINE reason=no-dock-strip"
    } else if parked_now && !tiled {
        "FAIL reason=park-no-tile"
    } else if restores != 0 && good == 0 && blanks != 0 {
        "FAIL reason=restore-blank"
    } else if parked_now {
        "PARKED-AWAITING-DOCK"
    } else if dock_restores != 0 && good != 0 {
        "DOCK-ROUNDTRIP"
    } else if restores != 0 {
        "RESTORED-NOT-VIA-DOCK"
    } else {
        "IDLE-NEVER-PARKED"
    };
    serial_println!(
        "[orindock] census seq={} t={} up={}s win={} vis={} tiles={} tiled={} parks={} restores={} viadock={} painted={} blank={} glass={} probes={} suppressed={} dockpress={} strip={} -> {}",
        seq, tick, up,
        info.as_ref().map(|i| i.id).unwrap_or(wm::WIN_NONE),
        match now_vis {
            1 => "panel",
            2 => "parked",
            _ => "norow",
        },
        tiles, tiled as u8, parks, restores, dock_restores, good, blanks, glass,
        taken.min(LAD_GLASS_MAX), suppressed,
        dock::last_press_outcome(),
        if strip.is_some() { "yes" } else { "REFUSED" },
        verdict
    );
}


// ── ORIN-SUPSOUND — the presenter's WIRE VOICE, and the seam counters behind it ───────────────────
//
// Appended at the END of this file, inside the ORIN-SUPSTATE tail block and gated on `supstate`
// alone: knob-off every item here is `#[cfg]`-erased, nothing below it exists to be shifted, and the
// jetson image's panic-`Location` records are untouched (the orinclick line-neutrality rule).
//
// THE DEFECT. `jd2_supstate_presenter` (main.rs) carried NOT ONE `serial_println!`. It is the only
// task that flushes to glass, so a presenter that dies is FROZEN GLASS AND A PERFECTLY HEALTHY
// SERIAL LOG: the input source keeps polling xHCI and emitting `:: tegra: JD20 —`, the dispatcher
// keeps echoing `:: tegra: JD2 — KEY`, the `[orinclick]` census keeps printing, and the screen never
// changes again. Every wire witness on the console path belongs to some OTHER role, so the flight
// reads green while the operator watches a still image. An instrument that only prints on the happy
// path cannot detect the failure it exists for.
//
// WHY THE CENSUS IS AUTHORED BY THE INPUT SOURCE AND NOT BY THE PRESENTER. This board's UART is a
// synchronous polled port SHARED WITH THE SPE's TCU and it drops bytes mid-line routinely, so **no
// verdict here may rest on a line's ABSENCE**. A census the presenter printed itself would report
// its own death by going silent, which on this wire is indistinguishable from a dropped line, a
// dropped ten seconds, or an operator who scrolled past it. So the census is emitted from the INPUT
// SOURCE's existing ~250 ms sweep (`jd2_supstate_phase2`, beside `orin_click_census`) and prints the
// presenter's counters as DELTAS. A dead presenter therefore produces a POSITIVE, REPEATING line —
// `pass=+0 ... -> DEAD` — every ~10 s, for as long as the operator cares to watch. That is the
// signal a lossy UART can carry: the operator confirms a verdict by seeing the SAME line twice, not
// by failing to see one once.
//
// The residual, stated rather than discovered: if the INPUT SOURCE dies, this census stops too. It
// is not silent about that either — `[orinclick] census` rides the same sweep and stops in the same
// breath, and the input source is also the xHCI poller, so `:: tegra: JD20 —` stops with it. Three
// correlated silences is a dead pump; one silent presenter is a `-> DEAD` line.
//
// FOOTPRINT. Rung 3's rule, verbatim: the cadence gate is decided FIRST off two system-register
// reads, and everything else happens on the ~1-in-40 pass that actually prints. The per-pass hooks
// the roles call (`sup_present_pass`, `sup_dispatch_pass`) are relaxed atomic adds and nothing else
// — no lock, no print, no allocation — so a per-frame path pays a `ldaddal` and no more.

/// ORIN-SUPSOUND — `CNTPCT_EL0` / `CNTFRQ_EL0` on the calling core. A private copy of
/// `clk_now_freq` because that one is `orinclick`-gated and this witness must stand alone (a
/// `supstate` boot with `UNAOS_ORINCLICK` unset is exactly the boot7k configuration).
#[cfg(feature = "supstate")]
fn sup_now_freq() -> (u64, u64) {
    let (now, freq): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, preserves_flags));
    }
    (now, freq)
}

/// ORIN-SUPSOUND — the presenter reached its first loop pass, i.e. `spawn` was followed by an actual
/// DISPATCH. `false` at census time is the one state the deltas cannot express: a role that was
/// announced (`[supstate] roles ... -> SPLIT`) but never ran is not the same defect as one that ran
/// and stopped, and `spawn`'s own `:: SCHED: task` witness is `#[cfg(feature = "pi")]` and therefore
/// silent on this board (a deliberate wire-cost re-gate; see `sched.rs`).
#[cfg(feature = "supstate")]
static SUP_PRES_UP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// ORIN-SUPSOUND — presenter loop passes (the liveness numerator).
#[cfg(feature = "supstate")]
static SUP_PRES_PASS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-SUPSOUND — presenter passes that took a NON-EMPTY frame board (there was work to present).
#[cfg(feature = "supstate")]
static SUP_PRES_WORK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-SUPSOUND — presenter passes that actually reached `pal.render()` (pixels went to glass).
/// `work` high with `flush` flat is the frozen-glass signature the census exists to name.
#[cfg(feature = "supstate")]
static SUP_PRES_FLUSH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-SUPSOUND — dispatcher loop passes (its own liveness, on the same terms).
#[cfg(feature = "supstate")]
static SUP_DISP_PASS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// ORIN-SUPSOUND — `sup_with_surface` calls that found NO surface installed. Structurally
/// unreachable from the roles (they are spawned after `sup_install`), so nonzero is a real defect.
#[cfg(feature = "supstate")]
static SUP_NOSURF: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-SUPSOUND — `SUP_SURFACE` acquisitions that had to give the core back (`try_lock` refused).
/// This is the CONTENTION meter for the arc's one long hold (the dispatcher across `handle_key`)
/// and, under ORIN-BSPRUN, for a holder caught off-CPU by a quantum expiry. Deliberately its own
/// counter: `[inwedge]` (input-path panel refusals) and `[wedge9]` (paint-path `owe_repaint`) are
/// separate populations by design and folding a third into either would make all three unreadable.
#[cfg(feature = "supstate")]
static SUP_SURF_WAIT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-SUPSOUND — the same, for the two LEAF seams (`SUP_KEYQ` / `SUP_FRAMES`) behind `sup_lock`.
/// The module header argues these are uncontended by construction on a cooperative core; a nonzero
/// reading is that argument being falsified, which is exactly what a preemptive terminus can do.
#[cfg(feature = "supstate")]
static SUP_SEAM_WAIT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// ORIN-SUPSOUND — keys pushed onto the key seam by the input source.
#[cfg(feature = "supstate")]
static SUP_KEY_PUSHED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-SUPSOUND — keys popped off the key seam by the dispatcher. `pushed - popped` is the seam's
/// standing depth, so a dispatcher that stopped draining is visible before the bound is reached.
#[cfg(feature = "supstate")]
static SUP_KEY_POPPED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-SUPSOUND — passes where `sup_key_full` answered TRUE: the 64-deep seam is at its bound and
/// the input source stopped draining the PAL ring. BACKPRESSURE, not loss — the keys are still in
/// the PAL ring — but it is the only observable that the dispatcher has fallen 64 keys behind.
#[cfg(feature = "supstate")]
static SUP_KEY_BACKPRESS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-SUPSOUND — the seam's DROP witness: `sup_key_push` refused a key because the bound would be
/// exceeded. Structurally unreachable while the caller honours `sup_key_full` before every pop, so
/// this is a hard-bound tripwire: NONZERO MEANS A KEY WAS LOST, and the census says so.
#[cfg(feature = "supstate")]
static SUP_KEY_DROPPED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// ORIN-SUPSOUND — quantum expiries observed by `timer_preempt` while `supstate` is armed. This is
/// the ONE number that says whether a SUPSTATE flight is testing the SUPSTATE x BSPRUN combination
/// at all: without ORIN-BSPRUN the tegra terminus never sets `SCHED_ACTIVE`, `timer_preempt` returns
/// at its first line, and this reads `+0` forever — a flight that reports `preempt=+0` has proven it
/// exercised the COOPERATIVE core only, and none of the preemption hazards were in play. It also
/// separates a quantum expiry from a voluntary `yield_now`, which the `ctx +N/win` rollup conflates.
#[cfg(feature = "supstate")]
static SUP_PREEMPT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// ORIN-SUPSOUND — census bookkeeping: last-printed sweep tick, sequence number, first-census
/// CNTPCT, and the previous readings the deltas are taken against.
#[cfg(feature = "supstate")]
static SUP_CENSUS_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "supstate")]
static SUP_CENSUS_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "supstate")]
static SUP_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "supstate")]
static SUP_T0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "supstate")]
static SUP_LAST: [core::sync::atomic::AtomicU32; 5] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 5];

/// ORIN-SUPSOUND — sweeps between census lines. The input source's sweep is ~250 ms, so 40 is the
/// ~10 s cadence `[orinclick]` and `[orintenant]` already use; one shared number would have coupled
/// three witnesses' cadences to one another, so this one is its own.
#[cfg(feature = "supstate")]
const SUP_CENSUS_PERIOD: u64 = 40;

/// ORIN-SUPSOUND — the presenter reached its first loop pass. Printed ONCE (an `AtomicBool` swap),
/// before the loop, so the line means "this task was DISPATCHED", not merely "spawn returned".
#[cfg(feature = "supstate")]
pub fn sup_present_up() {
    if SUP_PRES_UP.swap(true, core::sync::atomic::Ordering::Release) {
        return;
    }
    serial_println!(
        "[suppresent] up task=jd2-present core=0 — the presenter was DISPATCHED (spawn -> first loop pass), not merely announced; its liveness is now carried by the ~10 s [suppresent] census, which the INPUT SOURCE prints so that a dead presenter reads as a repeated line rather than as silence"
    );
}

/// ORIN-SUPSOUND — one presenter loop pass. `work` = the frame board was non-empty (or the auto-hide
/// edge fired); `flushed` = `pal.render()` actually ran. Relaxed adds only: this is a per-frame path.
#[cfg(feature = "supstate")]
pub fn sup_present_pass(work: bool, flushed: bool) {
    use core::sync::atomic::Ordering::Relaxed;
    SUP_PRES_PASS.fetch_add(1, Relaxed);
    if work {
        SUP_PRES_WORK.fetch_add(1, Relaxed);
    }
    if flushed {
        SUP_PRES_FLUSH.fetch_add(1, Relaxed);
    }
}

/// ORIN-SUPSOUND — one dispatcher loop pass.
#[cfg(feature = "supstate")]
pub fn sup_dispatch_pass() {
    SUP_DISP_PASS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// ORIN-SUPSOUND — a quantum expiry, from `timer_preempt` AFTER its `SCHED_ACTIVE` gate and its
/// `IN_RQ_SECTION` tripwire. Called from IRQ context on the ticking core, so it is one relaxed add
/// and NOTHING else — no lock, no print, no allocation.
#[cfg(feature = "supstate")]
pub fn sup_note_preempt() {
    SUP_PREEMPT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// **ORIN-SUPSOUND — the ~10 s presenter census, from the INPUT SOURCE's idle sweep** (appended
/// line-neutral beside `orin_click_census`'s call in `jd2_supstate_phase2`). See the block header
/// for why the presenter does not print its own liveness on this UART.
///
/// Verdict ladder (first match wins), each reachable and none constant:
///   * `UNSCHEDULED`  — `[supstate] roles ... -> SPLIT` was printed but the presenter never reached
///     its first loop pass: spawned onto core 0's queue and never dispatched.
///   * `DEAD`         — it ran and stopped. **THE FROZEN-GLASS VERDICT**: the panel is stuck on
///     whatever was last presented while every other console witness keeps printing.
///   * `FAIL reason=key-dropped`      — the 64-deep key seam refused a key (bound exceeded); a
///     keystroke was lost, which the seam's own contract says is unreachable.
///   * `FAIL reason=no-surface`       — `sup_with_surface` found no installed surface; the roles are
///     spawned after `sup_install`, so this too is meant to be unreachable.
///   * `STALLED`      — alive, taking work off the frame board, presenting nothing. The failure a
///     happy-path-only witness cannot see and the one that looks identical to health on the wire.
///   * `PRESENTING`   — flushes advanced in this window: pixels reached glass.
///   * `IDLE-NO-FRAMES` — alive, nothing posted to present. **UNRUN, never PASS**: a headless or
///     untouched console is idle by construction and this says so rather than claiming health.
#[cfg(feature = "supstate")]
pub fn sup_present_census(tick: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    // FOOTPRINT — cadence FIRST, off two system-register reads; nothing below runs on the other 39
    // sweeps of every 40. No lock is taken on any pass, printing or not.
    let (now, freq) = sup_now_freq();
    let armed = SUP_ARMED.swap(true, Relaxed);
    if armed && tick.wrapping_sub(SUP_CENSUS_TICK.load(Relaxed)) < SUP_CENSUS_PERIOD {
        return;
    }
    if !armed {
        SUP_T0.store(now, Relaxed);
    }
    SUP_CENSUS_TICK.store(tick, Relaxed);
    let seq = SUP_CENSUS_SEQ.fetch_add(1, Relaxed) + 1;
    let up = if freq == 0 { 0 } else { now.wrapping_sub(SUP_T0.load(Relaxed)) / freq };

    let pass = SUP_PRES_PASS.load(Relaxed);
    let work = SUP_PRES_WORK.load(Relaxed);
    let flush = SUP_PRES_FLUSH.load(Relaxed);
    let disp = SUP_DISP_PASS.load(Relaxed);
    let preempt = SUP_PREEMPT.load(Relaxed);
    let d_pass = pass.wrapping_sub(SUP_LAST[0].swap(pass, Relaxed));
    let d_work = work.wrapping_sub(SUP_LAST[1].swap(work, Relaxed));
    let d_flush = flush.wrapping_sub(SUP_LAST[2].swap(flush, Relaxed));
    let d_disp = disp.wrapping_sub(SUP_LAST[3].swap(disp, Relaxed));
    let d_preempt = preempt.wrapping_sub(SUP_LAST[4].swap(preempt, Relaxed));

    let dispatched = SUP_PRES_UP.load(core::sync::atomic::Ordering::Acquire);
    let nosurf = SUP_NOSURF.load(Relaxed);
    let dropped = SUP_KEY_DROPPED.load(Relaxed);
    let pushed = SUP_KEY_PUSHED.load(Relaxed);
    let popped = SUP_KEY_POPPED.load(Relaxed);

    let verdict = if !dispatched {
        "UNSCHEDULED reason=presenter-spawned-never-dispatched"
    } else if d_pass == 0 {
        "DEAD reason=presenter-not-advancing (FROZEN GLASS: the panel is stuck on the last presented frame while every other console witness keeps printing)"
    } else if dropped != 0 {
        "FAIL reason=key-dropped (the 64-deep seam exceeded its bound; a keystroke was lost)"
    } else if nosurf != 0 {
        "FAIL reason=no-surface (sup_with_surface found nothing installed)"
    } else if d_work != 0 && d_flush == 0 {
        "STALLED reason=work-taken-nothing-presented"
    } else if d_flush != 0 {
        "PRESENTING"
    } else {
        "IDLE-NO-FRAMES"
    };
    serial_println!(
        "[suppresent] census seq={} t={} up={}s dispatched={} pass=+{} work=+{} flush=+{} disp=+{} preempt=+{} nosurf={} surfwait={} seamwait={} keyq push={} pop={} depth={} full={} drop={} totals pass={} flush={} -> {}",
        seq, tick, up, dispatched as u8, d_pass, d_work, d_flush, d_disp, d_preempt,
        nosurf, SUP_SURF_WAIT.load(Relaxed), SUP_SEAM_WAIT.load(Relaxed),
        pushed, popped, pushed.wrapping_sub(popped), SUP_KEY_BACKPRESS.load(Relaxed), dropped,
        pass, flush, verdict
    );
}


// =================================================================================================
// ORIN-RASTGLASS — the RAST cube's glass read-back. `rast`-gated (no new knob), DEFAULT OFF with it.
// =================================================================================================
//
// THE DEFECT THIS ADDRESSES. `main.rs::tegra_rast_demo_maybe` is the only paint path on this arch
// with NO read-back. It prints `:: RAST: tegra — first 3D pixels on the Orin panel ::`, blits ~180
// frames through `Screen`, and returns — and every one of those statements is about what the code
// DID, not about what is on the panel. So when the cube did not appear on boot7j
// (flash/orin/boot7j-desktop-rast-ladder-20260826T0325Z-f0279b5), the wire could not distinguish
// "RAST painted and something repainted over it" from "RAST never put a pixel on the glass". Both
// produce exactly the same capture: the success line, the fps line, and no cube. ORIN-GLASSINK
// (`orin_glass_probe`) already established the shape for the console window; this is the same
// instrument pointed at the demo.
//
// WHY ONE SAMPLE CANNOT ANSWER IT, which is the whole design constraint. Overwritten and
// never-painted are the SAME glass state: no RAST pixel present. A probe fired once, whenever, can
// only report the state and would have to guess the history. So the read-back is a PAIR:
//
//   * `post`  — fired on the terminus line the instant `rast_demo::run` returns, on the same core,
//               with nothing dispatched in between. This establishes whether the blit ever reached
//               the scan-out at all. It is the "did we paint" half and nothing else can answer it.
//   * `late`  — fired from `jd2_console_pump`'s idle sweep (`orin_rast_census`), after the pump has
//               been running. This is the "did it survive" half.
//
// The three verdicts the brief asks for fall straight out of the pair, and only out of the pair:
// `post` painted + `late` painted = SURVIVED; `post` painted + `late` not = OVERWRITTEN; `post` not
// painted = NEVER-PAINTED, and no amount of `late` sampling can change that reading.
//
// THE DISCRIMINATOR is `rast_demo`'s own backdrop constant. `run` paints the WHOLE panel
// `0x0010_1018` before it draws (rast_demo.rs:96, `screen.fill_screen`), then blits a 320x240 render
// centred on it (`DEMO_W`/`DEMO_H`, rast_demo.rs:34-35, offsets rast_demo.rs:89-90). That constant
// appears nowhere else in this tree — it is not `video::PANEL_BG` (`0x001E_1E1E`), not the console's
// paper, not any theme colour — so an exact-equality test against it is a positive identification of
// RAST's own paint, and any repaint by the compositor, the console window or the desktop shows up as
// a foreign value rather than as a near-miss.
//
// ⚠ THE CONSTANTS ARE RESTATED, NOT IMPORTED, and that is the house convention rather than laziness:
// `src/rast_demo.rs` is a SHARED KERNEL-CORE file outside this track's lane, and `DEMO_W`/`DEMO_H`/
// the backdrop are private to it — importing them would mean a `pub` edit to a file this arc has no
// grant for. `orin_glass_probe` restates `video/fbcon.rs`'s theme constants for exactly this reason
// (see its own note at the ladder consts). The cost is a drift risk, and it is bounded in one
// direction only: if `rast_demo` changes its backdrop, this probe reports `NO-RAST-INK` — it goes
// BLIND, never falsely green. A restated constant that can only produce a false FAIL is an
// acceptable seam; one that could produce a false PASS would not be.
//
// ⚠ THE `blevels` LESSON (3b1c19c2, ORIN-GLASSINK), applied here in its general form. That commit
// found that a "fraction of samples matching the expected colour family" test PASSED on the desktop
// showing through the window, because `PANEL_BG` happened to sit on the paper->ink ramp. The lesson
// is not about ramps: it is that a single population can satisfy a coverage test for the wrong
// reason. This probe therefore samples TWO populations with OPPOSITE expectations and requires both:
//
//   * SURROUND (outside the 320x240 render box) — must be RAST's backdrop and nothing else. This is
//     the region `fill_screen` owns outright and the cube never touches, so any foreign pixel here
//     is a repaint, full stop.
//   * BOX (inside the render box) — must contain at least one NON-backdrop pixel, i.e. the cube
//     actually drew. Without this arm a surround-only test would report `CUBE-ON-GLASS` on a boot
//     where the fill landed and the rasteriser drew nothing at all — the identical class of
//     false-PASS `blevels` closed, arrived at from the other side.
//
// Neither population alone is evidence. `RAST-FILL-NO-CUBE` is the named verdict for surround-yes /
// box-no, so that state is REPORTED rather than folded into either neighbour.
//
// FOOTPRINT. Sampling is `LAD`-shaped: `RG_BANDS` rows x `RG_RUNS` contiguous runs of `RG_RUN`
// pixels, per region, so the reads are locality-friendly on a WC aperture where every unaligned
// volatile read is its own round trip (the GR17 cost model `read_pixel` documents). `WRITER` is
// copied out and the guard dropped in one statement — `FrameBuffer` is `Copy` — so no `wm` call can
// ever be made under it (ORIN-WM1's acyclic WRITER->TABLE rule). The `late` sample is budgeted and
// periodic like `lad_glass_budgeted`'s: a suppressed sample returns `BUDGET`, which is deliberately
// NOT in the passing set, because a sample that did not happen must never look like one that passed.

/// ORIN-RASTGLASS — `rast_demo`'s backdrop, restated (rast_demo.rs:96). See the ⚠ note above.
#[cfg(feature = "rast")]
const RG_PAPER: u32 = 0x0010_1018;
/// ORIN-RASTGLASS — `rast_demo`'s fixed render size, restated (rast_demo.rs:34-35). The blit is
/// centred, `off = (panel - demo) / 2` (rast_demo.rs:89-90), and `run` SKIPS entirely when the panel
/// is smaller than this — which is one of the never-painted histories this probe must be able to
/// report, so the same numbers have to be here to locate the box at all.
#[cfg(feature = "rast")]
const RG_DEMO_W: usize = 320;
#[cfg(feature = "rast")]
const RG_DEMO_H: usize = 240;
/// ORIN-RASTGLASS — the sampling grid, per region: `RG_BANDS` evenly spaced rows, `RG_RUNS`
/// contiguous runs of `RG_RUN` pixels on each. 8*4*32 = 1024 samples per region, the same budget
/// `orin_glass_probe` settled on.
#[cfg(feature = "rast")]
const RG_BANDS: usize = 8;
#[cfg(feature = "rast")]
const RG_RUNS: usize = 4;
#[cfg(feature = "rast")]
const RG_RUN: usize = 32;
/// ORIN-RASTGLASS — the latched `post` verdict, as an index into `RG_VERDICTS`. `u8::MAX` == the
/// probe has not run, which is itself a reportable state (`RAST-UNRUN`): `tegra_rast_demo_maybe`
/// returns before the paint site on a headless boot, so an absent `post` means RAST declined and
/// named its own reason on its own line.
#[cfg(feature = "rast")]
static RG_POST: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(u8::MAX);
/// ORIN-RASTGLASS — census bookkeeping: last-census tick, sequence, and the `late` sample budget.
#[cfg(feature = "rast")]
static RG_CENSUS_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "rast")]
static RG_CENSUS_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "rast")]
static RG_LATE_TAKEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// ORIN-RASTGLASS — ~2 s between census lines (the sweep cadence is ~250 ms; `TEN_CENSUS_PERIOD`'s
/// 40 is ~10 s). **The cadence is set by the window being measured, not by house habit, and copying
/// the tenant census's 40 here would have made this rung answer nothing.** `jd2_console_pump`'s
/// phase 1 — the interval between RAST returning and the console taking the panel — is bounded at 8
/// s (`CNTFRQ_EL0 * 8`, main.rs) and ends early on the first keystroke, i.e. AT MOST 32 sweeps. A
/// 40-sweep period cannot fire inside it, so every `late` sample would have been taken after the
/// console legitimately owned the panel and the verdict would have been a constant
/// `RAST-SUPERSEDED-BY-CONSOLE` — an instrument that reports the same answer on a healthy boot and
/// a broken one. 8 sweeps gives 3-4 samples inside the window where the cube is supposed to be
/// visible, which is the only window in which "something repainted over it" means anything.
#[cfg(feature = "rast")]
const RG_CENSUS_PERIOD: u64 = 8;
/// ORIN-RASTGLASS — set on `jd2_console_pump`'s phase-2 boundary, the statement at which the console
/// takes the panel for good. It is what separates the rung's two failure readings: the console
/// painting over the cube AFTER this point is the design working, and before it is the defect.
#[cfg(feature = "rast")]
static RG_CONSOLE_OWNS: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// ORIN-RASTGLASS — the census's terminal latch. This rung asks a question with a final answer
/// ("did the cube survive until the console took the panel"), unlike `orin_tenant_census`, whose
/// liveness IS its report. Once the answer cannot change, the census says so once and stops rather
/// than reprinting a settled verdict for the rest of the boot.
#[cfg(feature = "rast")]
static RG_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// ORIN-RASTGLASS — at most this many `late` read-backs for the whole boot. A read-back is ~2048
/// volatile VRAM reads, each its own non-posted round trip on a WC aperture (the GR17 cost model
/// `read_pixel` documents), so the budget is what keeps a periodic witness from becoming a periodic
/// cost. `RG_DONE` is what actually bounds this rung — the census stops the moment its answer is
/// settled — so the only path that spends budget is the unsettled one (cube still on the glass,
/// console not yet in possession), which lasts at most phase 1: 8 s at a 2 s cadence, ~4 samples.
/// 24 is therefore slack, not a working limit; it exists so a future caller on a different cadence
/// still cannot turn this into an unbounded per-sweep VRAM read. If it is ever reached the census
/// says `RAST-LATE-BUDGET` out loud rather than going quietly green.
#[cfg(feature = "rast")]
const RG_LATE_MAX: u32 = 24;

/// ORIN-RASTGLASS — the glass-state verdicts, indexed by the `u8` latched in `RG_POST`. A table
/// rather than `&'static str` plumbing because the `post` verdict has to survive in an atomic until
/// the census reads it, and an index that names a slot in this array cannot be a string that names
/// nothing.
#[cfg(feature = "rast")]
const RG_VERDICTS: [&str; 6] = [
    "CUBE-ON-GLASS",     // 0 — surround is all backdrop AND the box carries the cube's ink
    "RAST-FILL-NO-CUBE", // 1 — the fill landed, the rasteriser drew nothing into the box
    "RAST-PARTIAL",      // 2 — surround is part backdrop, part foreign: a partial repaint
    "NO-RAST-INK",       // 3 — not one backdrop pixel in the surround
    "UNREADABLE",        // 4 — read_pixel returned None everywhere (no panel / unmapped)
    "BUDGET",            // 5 — the sample was suppressed; NEVER in the passing set
];

/// ORIN-RASTGLASS — the passing set, the single adjudicator, listed explicitly. `orin_glass_probe`'s
/// `lad_glass_painted` states the rule this follows: *an adjudicator that admits verdicts it has
/// never seen is not an adjudicator* — so no prefix match, and a verdict added to `RG_VERDICTS`
/// without a decision made here is an omission a reader can see rather than a silent PASS.
///
/// `RAST-FILL-NO-CUBE` IS in the set, and the choice is load-bearing: this predicate answers *"did
/// RAST's paint reach the scan-out"*, which is the question the SURVIVED/OVERWRITTEN split turns on.
/// Whether the cube itself drew is a different defect, reported by the verdict string on its own
/// line and never averaged into this one.
#[cfg(feature = "rast")]
fn rg_painted(v: u8) -> bool {
    matches!(v, 0 | 1)
}

/// ORIN-RASTGLASS — sample one axis-aligned region and return `(read, paper, foreign)`.
///
/// `read` counts only samples `read_pixel` actually returned, so `read == 0` is a real, separable
/// answer ("nothing was readable") and never a silent zero folded in with the others. `paper` is
/// EXACT equality with [`RG_PAPER`] — no tolerance, because the backdrop is a flat fill written
/// through the same `put_pixel` encoding `read_pixel` inverts, so an exact round trip is the
/// specified behaviour and anything else is a different pixel, not a noisy one.
///
/// The `FrameBuffer` is borrowed from the caller's own copy: `WRITER` is already released, and
/// nothing here may take a lock.
#[cfg(feature = "rast")]
fn rg_sample(
    fb: &crate::video::FrameBuffer,
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
) -> (usize, usize, usize) {
    let (mut read, mut paper, mut foreign) = (0usize, 0usize, 0usize);
    if w == 0 || h == 0 {
        return (0, 0, 0);
    }
    let run = RG_RUN.min(w);
    for b in 0..RG_BANDS {
        // Band CENTRES, so a region shorter than RG_BANDS still spreads its samples instead of
        // stacking them all on row 0.
        let y = y0 + (2 * b + 1) * h / (2 * RG_BANDS);
        for r in 0..RG_RUNS {
            let centre = x0 + (2 * r + 1) * w / (2 * RG_RUNS);
            // Clamp the run inside the region: a run that walked out of the box would sample the
            // neighbouring population and quietly mix the two.
            let start = centre
                .saturating_sub(run / 2)
                .max(x0)
                .min((x0 + w).saturating_sub(run));
            for i in 0..run {
                if let Some(v) = fb.read_pixel(start + i, y) {
                    read += 1;
                    if v == RG_PAPER {
                        paper += 1;
                    } else {
                        foreign += 1;
                    }
                }
            }
        }
    }
    (read, paper, foreign)
}

/// **ORIN-RASTGLASS — read the panel back and say what is on it.** Returns the `RG_VERDICTS` index.
///
/// `phase` is the caller's label (`"post"` from the terminus line, `"late"` from the pump sweep) and
/// appears on the wire, so two lines with the same verdict are never confusable for one repeated
/// sample.
///
/// The verdict is DERIVED from the two populations, never asserted: the emitted line carries every
/// count it was computed from, and `paper + foreign == read` holds for each region on every line. A
/// line that does not balance is a defect in this instrument, not on the panel — `orin_glass_probe`'s
/// standing rule, restated because it is the only way a reader can audit the ladder from a capture.
#[cfg(feature = "rast")]
pub fn orin_rast_glass(phase: &str) -> u8 {
    // WRITER copied out and released in one statement (ORIN-WM1's rule); nothing below takes a lock.
    let fb = *crate::video::WRITER.lock();
    if !fb.is_ready() {
        serial_println!("[orinrast] phase={} -> NO-RAST-INK reason=no-panel (JD1 seeded no scanout; there is no glass to read, and tegra_rast_demo_maybe named the same condition on its own line). No panel is not painted — this must never adjudicate as painted", phase);
        return 3;
    }
    let i = fb.info();
    let (pw, ph) = (i.width, i.height);
    if pw < RG_DEMO_W || ph < RG_DEMO_H {
        // The panel is smaller than the render, which is exactly the arm `rast_demo::run` takes when
        // it prints "panel too small" and returns WITHOUT painting. There is no box to sample and no
        // paint to look for; say so rather than reporting an empty surround as a repaint.
        serial_println!("[orinrast] phase={} panel={}x{} demo={}x{} -> NO-RAST-INK reason=panel-too-small (rast_demo::run skips below this geometry and paints nothing at all — never-painted, not overwritten)", phase, pw, ph, RG_DEMO_W, RG_DEMO_H);
        return 3;
    }
    let (bx, by) = ((pw - RG_DEMO_W) / 2, (ph - RG_DEMO_H) / 2);
    // SURROUND: the full-width strip ABOVE the render box. It is entirely owned by `fill_screen` and
    // the cube can never reach it, so it is the cleanest available witness for the backdrop — and it
    // is one contiguous rectangle, which keeps the sampler simple enough to audit. (A panel exactly
    // 240 rows tall has no strip; `rg_sample` returns `(0,0,0)` for a zero-height region and the
    // `read == 0` arm below reports UNREADABLE rather than inventing a verdict.)
    let (s_read, s_paper, s_foreign) = rg_sample(&fb, 0, 0, pw, by);
    let (b_read, b_paper, b_foreign) = rg_sample(&fb, bx, by, RG_DEMO_W, RG_DEMO_H);
    // DERIVED, never asserted. Order matters: UNREADABLE outranks everything (an unread panel makes
    // no claim), then the surround decides whether RAST's fill is on the glass, and only inside the
    // "fill is intact" arm does the box get to say whether the cube drew. Reversing those two would
    // let a box full of foreign console pixels satisfy the "the cube drew" test.
    let v: u8 = if s_read == 0 && b_read == 0 {
        4 // UNREADABLE
    } else if s_paper == 0 {
        3 // NO-RAST-INK
    } else if s_foreign != 0 {
        2 // RAST-PARTIAL
    } else if b_foreign == 0 {
        1 // RAST-FILL-NO-CUBE
    } else {
        0 // CUBE-ON-GLASS
    };
    serial_println!(
        "[orinrast] phase={} panel={}x{} box={}x{} at ({},{}) paper={:#010x} surround read={} paper={} foreign={} box read={} paper={} foreign={} painted={} -> {}",
        phase, pw, ph, RG_DEMO_W, RG_DEMO_H, bx, by, RG_PAPER,
        s_read, s_paper, s_foreign, b_read, b_paper, b_foreign,
        rg_painted(v) as u8, RG_VERDICTS[v as usize]
    );
    v
}

/// ORIN-RASTGLASS — the `post` sample: fired from `tegra_rast_demo_maybe`'s terminus line the instant
/// `rast_demo::run` returns, and LATCHED. Nothing is dispatched on this core between the last blit
/// and this read, so it is the one measurement that can establish whether RAST's paint ever reached
/// the scan-out — every later sample can only report the state it finds.
#[cfg(feature = "rast")]
pub fn orin_rast_glass_post() {
    use core::sync::atomic::Ordering;
    let v = orin_rast_glass("post");
    RG_POST.store(v, Ordering::Release);
}

/// **ORIN-RASTGLASS — the ~10 s census, from `jd2_console_pump`'s idle sweep** (appended line-neutral
/// beside `orin_tenant_census`'s call). It takes the `late` sample and combines it with the latched
/// `post` into the lifecycle verdict — the answer to "why did the cube not appear".
///
/// Lifecycle ladder (first match wins), each reachable and none constant:
///   * `RAST-UNRUN`                — no `post` sample was ever latched, so `tegra_rast_demo_maybe`
///     returned before its paint site: a headless boot, or `rast` armed on an image with no scanout.
///     **UNRUN, never PASS** — `orin_tenant_census`'s `IDLE-NO-TENANTS` discipline.
///   * `RAST-NEVER-PAINTED`        — `post` says RAST's backdrop was not on the glass immediately
///     after `run` returned. Nothing had a chance to repaint in that window, so this is not a race:
///     the blit did not reach the scan-out. **No `late` sample can revise this**, which is why the
///     arm sits above both survival arms rather than being combined with them.
///   * `RAST-LATE-BUDGET`          — the `late` read-back was suppressed by the budget. Its own arm,
///     never a passing one, for the reason `lad_glass_budgeted` states: a sample that did not happen
///     must not look like a sample that passed.
///   * `RAST-LATE-UNREADABLE`      — `post` painted but the `late` read returned nothing at all. NOT
///     folded into OVERWRITTEN: an unreadable panel is a broken measurement, and reporting a broken
///     measurement as a detected overwrite would be the same overclaim this rung exists to remove.
///   * `RAST-PAINTED-SURVIVED`     — `post` painted and `late` still finds it, with the console not
///     yet in possession of the panel. The cube is on the glass; if it was not SEEN, the fault is
///     downstream of the scan-out (DCE, cable, panel), not in this kernel's paint path.
///   * `RAST-PAINTED-OVERWRITTEN`  — `post` painted, `late` does not, and **the console had not yet
///     taken the panel**. Something repainted the cube away inside the window where it was supposed
///     to be visible. This is the verdict that would have named boot7j's failure, and the only arm
///     that indicts a repainter.
///   * `RAST-SUPERSEDED-BY-CONSOLE` — `post` painted, `late` does not, and the console HAS taken the
///     panel (`jd2_console_pump` phase 2). The design working as specified, not a defect — and it is
///     a separate arm precisely because folding it into `RAST-PAINTED-OVERWRITTEN` would make that
///     verdict fire on every healthy boot and therefore mean nothing.
#[cfg(feature = "rast")]
pub fn orin_rast_census(tick: u64) {
    use core::sync::atomic::Ordering;
    // Terminal: the question has a final answer and it has been given. Checked before the cadence so
    // a settled rung costs one relaxed load per sweep and nothing else.
    if RG_DONE.load(Ordering::Acquire) {
        return;
    }
    // Cadence decided next, off the tick alone — rung 3's footprint rule. The panel is touched only
    // on the ~1-in-8 pass that prints, and even then only while budget remains.
    if tick.wrapping_sub(RG_CENSUS_TICK.load(Ordering::Relaxed)) < RG_CENSUS_PERIOD {
        return;
    }
    RG_CENSUS_TICK.store(tick, Ordering::Relaxed);
    let seq = RG_CENSUS_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let post = RG_POST.load(Ordering::Acquire);
    let owns = RG_CONSOLE_OWNS.load(Ordering::Acquire);
    // The `post` sample decides whether a `late` read is worth its ~2048 VRAM round trips at all: if
    // RAST never painted, no amount of later sampling changes the verdict. Index 5 stands for "not
    // taken" on those paths, and the ladder below never reads it there.
    let late = if post == u8::MAX || !rg_painted(post) {
        5
    } else if RG_LATE_TAKEN.fetch_add(1, Ordering::Relaxed) >= RG_LATE_MAX {
        5
    } else {
        orin_rast_glass(if owns { "late-console" } else { "late" })
    };
    let verdict = if post == u8::MAX {
        "RAST-UNRUN"
    } else if !rg_painted(post) {
        "RAST-NEVER-PAINTED"
    } else if late == 5 {
        "RAST-LATE-BUDGET"
    } else if late == 4 {
        "RAST-LATE-UNREADABLE"
    } else if rg_painted(late) {
        "RAST-PAINTED-SURVIVED"
    } else if owns {
        "RAST-SUPERSEDED-BY-CONSOLE"
    } else {
        "RAST-PAINTED-OVERWRITTEN"
    };
    // The ONE non-terminal state is "still on the glass, console not yet in possession" — the only
    // reading a later sample can still change. Everything else is settled: RAST never painted, the
    // cube is already gone, the console has taken over, or the budget is spent.
    if !(rg_painted(post) && !owns && rg_painted(late)) {
        RG_DONE.store(true, Ordering::Release);
    }
    serial_println!(
        "[orinrast] census seq={} t={} post={} late={} console-owns={} conwin={} pidesk={} final={} -> {}",
        seq, tick,
        if post == u8::MAX { "UNRUN" } else { RG_VERDICTS[post as usize] },
        RG_VERDICTS[late as usize],
        owns as u8,
        cfg!(feature = "orinconwin") as u8,
        cfg!(feature = "desktop_firmware") as u8,
        RG_DONE.load(Ordering::Acquire) as u8,
        verdict
    );
}

/// ORIN-RASTGLASS — the console has taken the panel. Called from `jd2_console_pump`'s phase-2
/// boundary (same-line append), which is where the console stops waiting and starts owning the
/// glass for the rest of the boot. Idempotent; one relaxed store.
///
/// This is the timestamp the whole rung's failure reading turns on: after it, the console painting
/// over the cube is the specified behaviour (`RAST-SUPERSEDED-BY-CONSOLE`); before it, the same
/// glass state is a defect (`RAST-PAINTED-OVERWRITTEN`). Without the latch the census could only
/// report that the cube was gone, which on a healthy boot is always true eventually.
#[cfg(feature = "rast")]
pub fn orin_rast_console_owns() {
    RG_CONSOLE_OWNS.store(true, core::sync::atomic::Ordering::Release);
}

// =================================================================================================
// DRAGDEAD (ledger A27) — **STEER the live title-bar grab from the Orin console pump.**
// =================================================================================================
//
// THE DEFECT, on the wire (docs/dev/evidence/orin15/render6-boot2.log, four times, windows 1/2/3):
//
//     [clickroute] press chrome win=3 owner=4294967042 at (1156,431) -> drag
//     [wm-act] drag-begin win=3 owner=0xffffff02 at (1156,431) -> grabbed
//     [wm-act] drag-end   win=3 owner=0xffffff02 at (520,441)  -> no-move
//
// The grab is minted and the grab is released and the window never moves. `drag_end`'s verb is
// decided by ONE counter — `DRAG_MOVES`, `video/wm.rs:16211` — and the only thing in the tree that
// increments it is `drag_motion`'s `Moved::Placed { changed: true }` arm (`video/wm.rs:16195`). So
// `no-move` is not a clamp, not a refusal and not a lost release: it is the arithmetic statement
// that `drag_motion` was NEVER CALLED for that gesture. The coordinates on the `drag-end` line say
// the same thing a second way — they are `DRAG_LAST_X/Y`, which `drag_begin` seeds with the ROW's
// origin (`video/wm.rs:15921-15922`) and only `drag_motion` ever re-seeds, so `(520,441)` is where
// the window already was, not where the hand went.
//
// WHY IT WAS NEVER CALLED, and the positive controls that make this a proof rather than a reading.
// `wm` publishes an arch-neutral steering tail for exactly this, `wm::drag_route_tail`
// (`video/wm.rs:16069`), whose own doc block names its two intended aarch64 call sites. Both exist
// and both are compiled into this image (`orinrender`/`deskcascade` imply `desktop_firmware`):
//
//   * x86 control:  `arch/x86_64/syscall.rs:6764 wc_route_tail` -> `wm::drag_motion`, driven from
//                   `main.rs:1991` and `main.rs:6725` — the two x86 drains.
//   * Pi control:   `main.rs:3879` (`route_input_to_active_el0`, the focused-app drain) and
//                   `main.rs:5442`/`:5452` (`render_service`'s `Mouse`/`MouseAbsolute` arms).
//
// The Orin runs NEITHER drain. Its pointer path is `jd2_console_pump` phase 2 (`main.rs:2801`),
// whose `Event::Mouse`/`Event::MouseAbsolute` arms only accumulate `pending_rel`/`pending_abs` and
// whose per-frame block (`main.rs:2987-2998`) applies them to `pal::cursor` and repaints the arrow.
// That block is the whole of what a pointer report does on this board, and no call to
// `drag_route_tail`, `drag_motion_paced` or `drag_motion` appears anywhere on it. Falsifier as run:
// `grep -rn 'drag_route_tail\|drag_motion' crates/kernel/src` returns the x86 and Pi sites above and
// nothing under `arch/aarch64/display_tegra.rs`, `arch/aarch64/syscall.rs` or the tegra region of
// `main.rs`. The tegra router (`arch/aarch64/syscall.rs:14400` press, `:14485` release) is complete:
// it BEGINS and it ENDS the drag. The gesture's middle had no owner on this board. This is it.
//
// WHY NOT SIMPLY CALL `wm::drag_route_tail` FROM THE PUMP. That function reads the panel geometry
// through `*super::WRITER.lock()` — a BLOCKING panel lock — before it reaches the pacer
// (`video/wm.rs:16081`). LOCKFIX (`7847ceea`) is standing law for this track's input path, and the
// adjacent `orinclick` router already obeys it by taking its geometry from
// `video::panel_info_nonblocking()` (`clk_pointer_pos`, this file). The pump moreover HAS the
// geometry already — it is holding the `Screen` it just drew the cursor on — so the lock would buy
// nothing. `orin_drag_steer` therefore takes the geometry from its caller and enters `wm` at
// `drag_motion_paced`, the same entry `drag_route_tail` uses one line later. Every lock below that
// point is `wm`'s own (`move_to_inner` takes `WRITER`, drops it, then takes `TABLE`) and is the
// identical sequence the x86 and Pi paths already drive; this seam adds none of its own.
//
// WHERE IT IS CALLED, and the placement is load-bearing in both directions. The call sits in
// `jd2_console_pump`'s per-frame pointer block IMMEDIATELY BEFORE `cursor::draw_over(&mut pal)`
// (`main.rs:2996`). AFTER `move_rel`/`set_abs`, because `pal::cursor::pos` must already carry this
// frame's travel or the window would trail the arrow by a frame — the same "call it where the
// cursor is FRESH" rule `drag_route_tail`'s doc states for the Pi. BEFORE `draw_over`, because
// `draw_over` saves the pixels under the arrow and `restore` writes them back: a compositor pass
// that repainted the panel between those two would make the next `restore` stamp a stale
// cursor-sized patch over the moved window. With the steer ahead of `draw_over` the arrow is off
// the glass while `wm` composites and goes back on top of the finished frame.
//
// RATE. `drag_motion_paced` is `wm`'s own coalescer (`video/wm.rs:16023`): it admits at most one
// reposition per `DRAG_MOTION_MS` and counts the rest as coalesced. The pump has already coalesced
// once on its own — every report in a drain frame collapses into a single `pending_rel`/
// `pending_abs` — so this seam is called once per FRAME, not once per report, and the pacer is the
// second gate behind it. Idle cost when nobody is dragging: one relaxed load of `DRAG_WIN`.
//
// SUPSTATE IS DELIBERATELY NOT WIRED INSIDE THE SURFACE LOCK, and the reason is named rather than
// left to be discovered. `jd2_supstate_phase2`'s twin block (`main.rs:7749-7767`) runs INSIDE
// `sup_with_surface`'s closure, which holds `SUP_SURFACE` across `f` (this file) under a stated
// LEAF-LOCK rule — "never held across a yield". `move_to_inner` composites and can yield, so
// steering from inside that closure would break the rule the supstate design is built on. The call
// therefore goes on the line AFTER the closure returns (`main.rs:7775`), where the cursor is already
// current and no supstate lock is held, and it passes `0,0` so the geometry is fetched non-blocking
// here.
//
// WITNESS. `wm` has no `drag-move` line of its own — it counts moves and reports them only at
// `drag_end` — so this seam prints its own, and NEVER per report:
//   `[dragroute] wired panel=WxH …`   once per boot, on the first pointer frame that reaches here.
//                                     Proves the CALL SITE is reached even on a boot where nobody
//                                     drags, which an absent `[dragroute] end` cannot.
//   `[dragroute] arm win=N …`         once per gesture, when a grab first becomes visible here.
//   `[dragroute] end win=N via=… fed=… applied=… at (x,y) -> VERDICT`   once per gesture.
//        `STEERED`     — the pacer admitted at least one reposition. Expect `drag-end … -> placed`.
//        `FED-NO-MOVE` — motion was fed and `wm` moved nothing: a clamp against a panel edge, or
//                        the cheap-skip on an unchanged origin. Distinguishes "the seam is dead"
//                        from "the window is already where it is being asked to go".
//        `NO-FEED`     — the grab began and ended without one pointer frame in between (a click on
//                        the title bar that never moved). The honest `no-move`.
// CONTROL: the token `[dragroute] control-absent` is written NOWHERE in this tree on purpose, so a
// `grep -a` for it returning zero proves the search can return zero and a zero for the tokens above
// is absence rather than a broken pattern.
//
// DEFAULT OFF. Every item is `#[cfg(feature = "orinclick")]` and this whole file is
// `#[cfg(feature = "tegra")]` (`arch/aarch64/mod.rs:105`), so it is absent from the Pi's
// `kernel8.img` by construction and vanishes from a knob-off jetson image. It is appended at the
// FILE TAIL and the three `main.rs`/`orin_click` sites are statements folded onto existing lines, so
// no line moves in either file and no panic `Location` is renumbered.
//
// A NOTE ON `RELEASE-DROPPED`, since the same captures raise it and it is NOT a defect.
// `[orinclick] edge=release … hit=no win=0 … -> RELEASE-DROPPED` reads as a lost release, and it is
// neither. `hit=no win=0` is `orin_click`'s own doing: it hit-tests only on a PRESS edge (`let
// target = if pressed != 0 { … } else { None }`), because the release belongs to whoever took the
// press, so `target` is `None` on every release by construction. `DROPPED` is `wc_click_route`
// returning `true` — the release was CONSUMED by the window layer rather than delivered to an EL0
// app — and that is the correct outcome for a chrome press, which stored `CLICK_TARGET_DROP`. The
// release does reach `drag_end`: `arch/aarch64/syscall.rs:14485` ends the grab at the TOP of the
// release arm, before the target is even read, which is why `[wm-act] drag-end` prints at all. The
// word is a misreport of what a reader expects it to mean, not of what happened. NOT renamed in
// this arc — it is `orinclick`'s wire vocabulary and four flights of scorers key on it.

/// DRAGDEAD — the window id whose gesture this seam is currently steering, `WIN_NONE` when idle.
/// Compared against `wm::drag_active()` every frame to find the gesture's edges: `wm` owns the
/// drag's truth, this cell only remembers what was last seen so a transition can be reported once.
#[cfg(feature = "orinclick")]
static DRG_WIN: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(crate::video::wm::WIN_NONE);
/// DRAGDEAD — pointer frames FED to `wm::drag_motion_paced` across the live gesture.
#[cfg(feature = "orinclick")]
static DRG_FED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// DRAGDEAD — repositions `wm` actually APPLIED across the live gesture (the pacer admitted them and
/// the origin changed). This is the quantity `drag_end`'s `placed`/`no-move` verb is decided by.
#[cfg(feature = "orinclick")]
static DRG_APPLIED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// DRAGDEAD — gestures this seam has seen, so the `arm` line carries an ordinal.
#[cfg(feature = "orinclick")]
static DRG_GEST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// DRAGDEAD — last panel point fed, reported at the end of the gesture. Unlike `drag-end`'s own
/// coordinates (the WINDOW's origin) this is where the HAND was, so the two lines together say
/// whether the window followed it.
#[cfg(feature = "orinclick")]
static DRG_X: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);
#[cfg(feature = "orinclick")]
static DRG_Y: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);
/// DRAGDEAD — the serial budget, the same discipline `CLK_LOG_MAX` applies to `[orinclick]`: a
/// gesture is an operator action and cannot storm, but a cancel loop could.
#[cfg(feature = "orinclick")]
static DRG_LOGGED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "orinclick")]
const DRG_LOG_MAX: u32 = 96;
/// DRAGDEAD — has the `wired` line been printed? One relaxed swap on the first pointer frame.
#[cfg(feature = "orinclick")]
static DRG_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// DRAGDEAD — report and clear the gesture just ended. `via` names WHICH edge observed the end:
/// `release` (the button came up and `wc_click_route` called `drag_end`) or `ended` (the drag was
/// gone by the next pointer frame without this seam seeing the release — `wm`'s own `drag_cancel`,
/// i.e. `barrier-stalled`, `row-gone` or `row-recycled`, whose reason is on the adjacent
/// `[wm-act] drag-cancel` line).
#[cfg(feature = "orinclick")]
fn drg_end_report(win: crate::video::wm::WinId, via: &str) {
    use core::sync::atomic::Ordering::Relaxed;
    let fed = DRG_FED.swap(0, Relaxed);
    let applied = DRG_APPLIED.swap(0, Relaxed);
    let (x, y) = (DRG_X.load(Relaxed), DRG_Y.load(Relaxed));
    let verdict = if fed == 0 {
        "NO-FEED"
    } else if applied == 0 {
        "FED-NO-MOVE"
    } else {
        "STEERED"
    };
    if DRG_LOGGED.fetch_add(1, Relaxed) < DRG_LOG_MAX {
        serial_println!(
            "[dragroute] end win={} via={} fed={} applied={} at ({},{}) -> {}",
            win, via, fed, applied, x, y, verdict
        );
    }
}

/// **DRAGDEAD — feed one pointer FRAME to the live title-bar drag.**
///
/// Called from `jd2_console_pump`'s per-frame pointer block (`main.rs`, same-line fold) after the
/// frame's travel has been applied to `pal::cursor` and before the arrow is drawn back on. Pass the
/// panel geometry the caller already holds; pass `0, 0` to have it read non-blocking from
/// `video::panel_info_nonblocking()` (the supstate site, which has no `Screen` in scope).
///
/// Total and cheap when nobody is dragging: one relaxed load and a return, which is what makes it
/// safe on every pointer frame of every boot.
#[cfg(feature = "orinclick")]
pub fn orin_drag_steer(pw: i32, ph: i32) {
    use core::sync::atomic::Ordering::Relaxed;
    use crate::video::wm;
    let live = wm::drag_active();
    let seen = DRG_WIN.load(Relaxed);
    if live != seen {
        // An edge. Report the gesture that ended (a superseding `drag_begin` ends one too), then
        // arm the new one. `wm` decided both; this only names them on the wire.
        if seen != wm::WIN_NONE {
            drg_end_report(seen, "ended");
        }
        DRG_WIN.store(live, Relaxed);
        if live != wm::WIN_NONE {
            DRG_FED.store(0, Relaxed);
            DRG_APPLIED.store(0, Relaxed);
            let n = DRG_GEST.fetch_add(1, Relaxed) + 1;
            if DRG_LOGGED.fetch_add(1, Relaxed) < DRG_LOG_MAX {
                serial_println!("[dragroute] arm win={} gesture={} -> STEERING", live, n);
            }
        }
    }
    let (pw, ph) = if pw > 0 && ph > 0 {
        (pw, ph)
    } else {
        match crate::video::panel_info_nonblocking() {
            Some(i) => (i.width as i32, i.height as i32),
            // No geometry means no cursor position worth reading; `[dragroute] end` will say
            // `NO-FEED` for the gesture, which is the honest report of a frame we could not steer.
            None => return,
        }
    };
    if !DRG_ARMED.swap(true, Relaxed) {
        serial_println!(
            "[dragroute] wired panel={}x{} desktop_firmware={} -> READY",
            pw,
            ph,
            cfg!(feature = "desktop_firmware") as u8
        );
    }
    if live == wm::WIN_NONE {
        return;
    }
    let (x, y) = crate::pal::cursor::pos(pw, ph);
    DRG_X.store(x, Relaxed);
    DRG_Y.store(y, Relaxed);
    DRG_FED.fetch_add(1, Relaxed);
    // `drag_motion_paced` coalesces to one reposition per frame period and re-tests the row's owner
    // under `TABLE` inside `move_to_inner`; a row that died under the hand cancels there, not here.
    if wm::drag_motion_paced(x, y) {
        DRG_APPLIED.fetch_add(1, Relaxed);
    }
}

/// **DRAGDEAD — the RELEASE edge's report.** Called at the tail of [`orin_click`] (same-line fold),
/// after `wc_click_route` has run: the release arm of the router calls `wm::drag_end()` before it
/// reads its target, so by the time control is back here the gesture is over. Reporting from the
/// button edge rather than waiting for the next pointer frame is what makes the `[dragroute] end`
/// line exist for a drag the operator ends without moving the mouse again.
#[cfg(feature = "orinclick")]
pub fn orin_drag_edge() {
    use core::sync::atomic::Ordering::Relaxed;
    use crate::video::wm;
    let seen = DRG_WIN.load(Relaxed);
    if seen != wm::WIN_NONE && wm::drag_active() == wm::WIN_NONE {
        DRG_WIN.store(wm::WIN_NONE, Relaxed);
        drg_end_report(seen, "release");
    }
}

// =================================================================================================
// CLICKDEAD — WHO RE-ARMS THE POINTER INTERRUPT-IN READ, AND DID IT? `orinclick`, FILE TAIL.
// =================================================================================================
//
// THE METAL FACT THIS ANSWERS. On render6 (`docs/dev/evidence/orin15/render6-boot1.log`) the
// `orinclick` census read `btn=0 press=0 rel=0 -> IDLE-NO-CLICKS` for 477 s with Peter's hand on
// the mouse, `[cursor3]` read `offers=0 taken=0`, and the shared driver's own pointer witness
// (`drivers/xhci/mod.rs:4774`, an UNGATED `serial_println!`) printed `:: MOUSE-1: 1 reports ::`
// ONCE, at enumeration, and never again. That witness fires at `n == 1 || n % 32 == 0` on a counter
// bumped at `mod.rs:4772-4773` — INSIDE the driver, BEFORE `pal::push_pointer_report` and therefore
// before every consumer this file, `wc_click_route` and the cursor path contain. So the whole
// consumer half is exonerated by that one line: fewer than 32 pointer reports were DECODED in ten
// minutes. The control, render4 (same devices, same hub, `orinclick` absent), decoded 1536+ and
// printed `JD20 — pointer live` and 24 `pointer BUTTON` lines. The pipeline, not the routing.
//
// WHAT THE WIRE COULD NOT SAY, AND WHY THIS EXISTS. The driver already accounts for every re-arm:
// `MOUSE_REARM_COUNT` (every `queue_mouse_read`), `MOUSE_DISCARD_REARM_COUNT` (the dup-Success
// guard's pipeline-preserving exit) and `MOUSE_ERROR_REARM_COUNT` (non-halting error re-arms) are
// `pub` and bumped UNCONDITIONALLY (`mod.rs:2373-2386`). Only their PRINT is knob-gated —
// `piusb39_witness` is `#[cfg(feature = "usbdebug")]` (`mod.rs:14730-14746`), and `usbdebug` is not
// in the jetson flight recipe. render6 therefore carried the answer in three atomics nothing read.
// This reads them. No new accounting, no change to the shared driver, no lock, no MMIO: three
// relaxed loads on the census pass that was already going to print.
//
// HOW TO SCORE THE NEXT BOOT.
//   * `reports=` on the census line is `rearm - discard - errrearm` = the arms that followed a
//     DECODED report, PLUS one enumeration arm per pointer that enumerated (`mod.rs:4310`). On this
//     board two pointers enumerate (the relative boot-mouse on slot 4 and the absolute pointer on
//     the keyboard composite, slot 5), so `reports=2` means ZERO decoded reports since boot and
//     `reports=2+k` means k of them. render6's value would have been 2.
//   * `[ptrpoll] ... -> ARMED-NO-COMPLETION` is the render6 shape: the read was armed and the
//     controller posted no further transfer event for the pointer DCI. That leaves exactly two
//     live mechanisms, and they are separated by `docs/dev/evidence/orin15/CLICKDEAD-xhci.patch`
//     (rmbp's lane, delivered as a patch, not committed): the dup-Success guard's `param ==
//     mouse_prev_phys` arm (`mod.rs:4594`) is the ONE branch in the driver that consumes a pointer
//     completion, does not re-arm, and — on a build without `usbdebug` — prints nothing. The patch
//     gives that arm a counter so `dup=` tells a discarded completion apart from no completion.
//   * `-> GUARD-REARM` / `-> ERROR-REARM` name the two churn shapes instead: completions ARE
//     arriving and the guard or the error path is re-arming them. `-> STREAMING` is health.
//
// WHY THE HALT PATH IS NOT THE ANSWER HERE, AND IS STILL A HOLE. A halting completion (codes
// 2/3/4/5/6) does not re-arm at dispatch; it is queued to `hid_halt_pending` (`mod.rs:4231-4234`)
// and drained only by `service_hid_halts`, reachable only through `service_hid_setproto`
// (`mod.rs:12874`). The tegra post-drop pumps call `poll_events()` and NOTHING else, deliberately —
// `xusb_tegra.rs:1924-1926` records why: the `service_*` pumps' bounded waits ride `crate::hlt()`,
// which after the drop has no wake source and parks the core. So on tegra a halted HID interrupt-IN
// endpoint is unrecoverable for the rest of the boot. It is NOT what killed render6's pointer:
// `hid_error_witness` (`mod.rs:14748-14768`) is explicitly UNGATED and the log carries zero
// `xHCI: pointer interrupt-IN error` lines. Recorded, not convicted — see CLICKDEAD.md.
//
// DEFAULT OFF. Every item below is `#[cfg(feature = "orinclick")]` and sits at the END of the file,
// after the last pre-existing line, so knob-off nothing here compiles and NO line in this file
// moves — panic `Location` records embed line numbers and the knob-off jetson image's
// byte-identity is this track's standing proof.

/// CLICKDEAD — the last `rearm + dup + nobuf` this witness printed, so a frozen pipeline costs one
/// line and a live one prints at the census cadence. `u64::MAX` = never printed.
#[cfg(feature = "orinclick")]
static PTRPOLL_LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// CLICKDEAD — the `rearm - discard - errrearm` balance the FIRST census pass saw, i.e. the
/// enumeration arms. `reports=` minus this is the decoded-report count since the census armed.
#[cfg(feature = "orinclick")]
static PTRPOLL_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// CLICKDEAD — read the shared driver's five ungated pointer-pipeline counters and, when they
/// moved (and once at the start, so a frozen pipeline is still timestamped), print `[ptrpoll]`.
///
/// Returns `rearm - discard - errrearm` for the caller's `reports=` census field, so the two lines
/// cannot disagree: both come from the same loads.
///
/// The token is `[ptrpoll]`, nine bytes — deliberately longer than eight so it cannot be folded
/// into an LLVM immediate and must appear in `.rodata`, which is what makes `grep -a` on the
/// artifact a reachability proof rather than a compile proof.
///
/// Runs on the census pass only (~1 in 40 sweeps, ~10 s), takes no lock and touches no MMIO — five
/// `Relaxed` loads and one `Relaxed` swap. The census comment's rule (never add lock traffic to
/// the path whose death this reports) holds by construction.
#[cfg(feature = "orinclick")]
fn ptrpoll_witness(tick: u64) -> u64 {
    use core::sync::atomic::Ordering;
    let rearm = crate::drivers::xhci::MOUSE_REARM_COUNT.load(Ordering::Relaxed);
    let disc = crate::drivers::xhci::MOUSE_DISCARD_REARM_COUNT.load(Ordering::Relaxed);
    let err = crate::drivers::xhci::MOUSE_ERROR_REARM_COUNT.load(Ordering::Relaxed); let dup = crate::drivers::xhci::MOUSE_DUP_DROP_COUNT.load(Ordering::Relaxed); let nobuf = crate::drivers::xhci::MOUSE_NOBUF_DROP_COUNT.load(Ordering::Relaxed); // CLICKDEAD-xhci.patch v2 — the two counters that separate (a1) from (a2), and (a1)'s two sub-causes from each other: `dup` = the guard ate a known duplicate, `nobuf` = the buffer/ring was gone. ⚠ folded, line-neutral.
    // Saturating: the three counters are read one at a time and a completion can land between the
    // loads, so the arithmetic is only ordered in the limit. A one-off underflow would print
    // `reports=0` and read as "worse than dead"; saturation makes it read as "not yet counted".
    let reports = rearm.saturating_sub(disc).saturating_sub(err);
    let base = PTRPOLL_BASE.load(Ordering::Relaxed);
    let first = base == u64::MAX;
    if first {
        PTRPOLL_BASE.store(reports, Ordering::Relaxed);
    }
    let moved = rearm.wrapping_add(dup).wrapping_add(nobuf); let last = PTRPOLL_LAST.swap(moved, Ordering::Relaxed); // CLICKDEAD-xhci.patch v2 — the DROPS join the movement test, or a pipeline being EATEN (rearm flat, dup/nobuf climbing) would be silently mistaken for one that is STARVED and print one line for the whole boot. ⚠ folded, line-neutral.
    if !first && last == moved {
        return reports; // nothing moved since the last census — one line already said so.
    }
    let decoded = reports.saturating_sub(if first { reports } else { base });
    let verdict = if decoded != 0 {
        "STREAMING (the pointer read is completing and re-arming; a dead click above this line is a ROUTING fault, not a pipeline one)"
    } else if disc != 0 {
        "GUARD-REARM (completions ARE arriving but their TRB does not match the armed read; the dup-Success guard is re-arming them and no report is decoding — mod.rs:4586)"
    } else if err != 0 {
        "ERROR-REARM (non-halting error completions on the pointer endpoint; the read is being re-armed off the error path — mod.rs:4238)"
    } else if first {
        "BASELINE (the enumeration arms; every later line is measured against this one)"
    } else if nobuf != 0 {
        "NOBUF-DROP (completions ARE arriving and the guard is dropping them because `mouse_data_buffer`/`mouse_ring` is GONE — mod.rs:4599's `!have_buf` arm. A teardown or allocation defect, NOT a duplicate: re-arming here would be wrong, there is nothing to arm. Look at the slot's soft state, not at the dup discrimination)"
    } else if dup != 0 {
        "DUP-DROP (completions ARE arriving and the dup-Success guard is consuming them WITHOUT re-arming — mod.rs:4599's `param == mouse_prev_phys` arm, buffer and ring still present. This is (a1): the pipeline is being eaten, not starved)"
    } else {
        "ARMED-NO-COMPLETION (the read is armed, dup=0 and nobuf=0, and the controller has posted NO transfer event for the pointer DCI since the last line. This is (a2): the endpoint went quiet — look at EP state, doorbell and periodic bandwidth, not at the guard)"
    };
    serial_println!(
        "[ptrpoll] t={} rearm={} discard={} errrearm={} dup={} nobuf={} reports={} base={} decoded={} -> {}",
        tick, rearm, disc, err, dup, nobuf, reports,
        if first { reports } else { base }, decoded, verdict
    );
    reports
}
