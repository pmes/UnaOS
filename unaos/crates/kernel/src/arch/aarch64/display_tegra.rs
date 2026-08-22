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
/// nvdisplay per-window aperture (Linux Tegra DRM `hub.c`, T186+/T234): window `i` registers live at
/// `head_base + 0x2800 + 0xC00*i`, with (byte offsets within the aperture) `WIN_OPTIONS`(WIN_ENABLE
/// bit30) `+0x600`, `WINDOWGROUP_SET_CONTROL`(OWNER low nibble) `+0x608`, `COLOR_DEPTH` `+0x60c`,
/// `SIZE`(output) `+0x614`, `CROPPED_SIZE`(source) `+0x618`, `PLANAR_STORAGE`(stride/64) `+0x624`,
/// `START_ADDR`(lo) `+0x700`, `SURFACE_KIND`(0=pitch) `+0x72c`, `START_ADDR_HI` `+0x734` — all plain
/// config registers (read-safe, not the read-to-clear status region). The per-head sub-aperture
/// stride inside the `display@13800000` wrapper is `0x10000` (4 heads; T194-derived, the one
/// bench-confirm number), so all four candidate heads are swept and the real one is the enabled
/// window with sane (≈1920×1200) geometry.
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
// call `pidesk::activate`, does not enable furniture, and touches no `dock`/`strip`/`menubar`/`crystal`
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
// clear is `pidesk`-gated on aarch64 (`wcf.rs` says so in its own NOCLEAR verdict) — so the composite
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
    id
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
/// * `VERDICT=DECODES-NOMATCH` — registers decode and windows are enabled, but no `START_ADDR`
///   matched. The pipe is reachable; the scanout is being fed from somewhere this sweep did not read
///   (a different head layout, or a window base the DTB handoff does not correspond to).
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
    );

    // 5. THE WINDOW SWEEP — read-only, every field, every window, EMPTY WINDOWS INCLUDED. The legacy
    //    `jd1_dc_survey` skips all-zero windows to keep its log short; this rung prints them, because
    //    "every window read zero" and "the sweep never ran" must not look alike in a capture that a
    //    decision rests on, and because a later save/restore rung needs the full field set from the
    //    same data. Offsets are the Linux Tegra DRM (`hub.c`, T186+/T234) window layout documented on
    //    `jd1_dc_survey`; per-head stride 0x10000, bounded by the DTB-declared aperture size so no
    //    read can leave the block's own decode window.
    let mut swept = 0u32;
    let mut all_ones = 0u32;
    let mut all_zero = 0u32;
    let mut enabled = 0u32;
    let mut matched = 0u32;
    let mut heads = 0u32;
    let mut match_head = 0u64;
    let mut match_win = 0u64;
    for &head_off in &[0u64, 0x10000, 0x20000, 0x30000] {
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
        swept * 9 + 1,
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
            ":: tegra: JD1-DC VERDICT=DECODES-NOMATCH — the aperture DECODES ({} window(s) at WIN_ENABLE=1 out of {} swept), but no START_ADDR equalled the JD1 scanout base {:#x}{}. Reachability is ESTABLISHED; the scanout is fed from a window this sweep's head model did not cover (swept=0 means the aperture held the FE capability word but no complete head-0 window bank) — compare the RAW lines above against DISPLAY_FE_SW_SYS_CAP={:#010x}, which is UEFI's own head/SOR enumeration ::",
            enabled,
            swept,
            scanout,
            if scanout == 0 { " (JD1 resolved NO scanout this boot, so no comparison was possible — the match test is vacuous here, not failed)" } else { "" },
            first,
        );
    } else {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=NOT-DECODING — {} of {} windows read all-ones and {} read all-zero, none enabled. READ THIS CAREFULLY: UEFI drives THESE registers on THIS board from THIS core by plain MmioRead32/MmioWrite32 (edk2-nvidia NvDisplayControllerDxe/NvDisplayHw.c), and the string 'dce' appears nowhere in that tree — so this result is NOT 'the DCE holds the block'. It is a finding about OUR access path: our aperture/mapping, the domain or clock state at our probe point, or an SCR narrower for us than for UEFI. FE_SW_SYS_CAP read {:#010x} ::",
            all_ones,
            swept,
            all_zero,
            first,
        );
    }
}
