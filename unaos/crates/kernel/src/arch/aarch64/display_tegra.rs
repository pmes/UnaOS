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
// WHY IT IS RUNG 3 AND NOT RUNG 4. `video/pidesk.rs:39-44` states the CONSOLEWIN law: the console
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
// this call stack. This rung therefore does NOT arm `pidesk`: `orinclick` implies `tegra_el0` and
// NOTHING ELSE, so every furniture arm inside `wc_click_route` (`strip::press_route`,
// `quarry::service`, `pulsewin::press_route`, `quarry::press_route`, the DRAG-PI chrome arm and the
// SHELLWIN-PI furniture arm) is `#[cfg(feature = "pidesk")]` and COMPILED OUT. What is left is the
// window half: `wm::hit_test`, `wm::close_box_hit`/`minimise_hit`/`zoom_hit`, `focus_changed`, and
// `user_input_set_active` — none of which opens a file, and none of which is on either recorded
// overflow's path. No dock, no strip, no menubar, no crystal, no `render_service`.
//
// WHY `orinclick` IMPLIES `tegra_el0`, and why that is the SHAPE OF THE CONFIGURATION rather than a
// wider net. `wc_click_route` lives in `arch/aarch64/syscall.rs`, and `arch/aarch64/mod.rs:46` gates
// `pub mod syscall;` on `any(feature = "baremetal", feature = "tegra_el0")`. `baremetal` implies `pi`
// and `pi` + `tegra` is a hard `compile_error!` (`arch/aarch64/serial.rs:22`), so on the Orin the ONLY
// satisfiable term is `tegra_el0`. A standalone `orinclick = []` in the `orindesk`/`jd1dc`/`smpmark`
// mould would have been a knob that compiles NOTHING unless the operator happens to also set
// `UNAOS_TEGRA_EL0=1` — a vacuous gate wearing a green verdict, the defect class `arroyo`'s own
// KERNEL_CFG_MATRIX preamble is written against. `tegra_el0` implies `tegra`, so
// `UNAOS_ORINCLICK=1 ./arroyo check` and `UNAOS_ORINCLICK=1 ./arroyo esp-jetson` are both
// self-sufficient. The ARMED polarity is type-checked by the `arm-tegra-orinclick` leg of
// KERNEL_CFG_MATRIX, and the `pidesk` CROSS by `arm-tegra-desk` — never by the knob mapping.
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
/// desktop/console; `CONSUMED` is a furniture or control arm (only reachable on a `pidesk` build).
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
/// | `CONSUMED` | press was taken by a control or furniture arm (close/minimise/zoom, or `pidesk` chrome) |
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
/// `pidesk` OFF the SHELLWIN-PI arm (`is_kernel_owner` -> hand the keyboard to asid 0) is compiled
/// out, so a press on `orin_wm1`'s row — owner `wm::KERNEL_OWNER_DESKTOP` — takes the ordinary
/// `owner != cur` arm and leaves `USER_INPUT_ACTIVE` holding that kernel pseudo-ASID. On the Orin
/// that is INERT for the keyboard and it was verified rather than assumed: the only consumer of
/// `USER_INPUT_ACTIVE` for keystrokes is `pump_usb_into_gui`'s `user_input_active() != 0` branch
/// (`main.rs:3887`), which is `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]` and does
/// not exist on tegra; `jd2_console_pump` feeds every `Event::Key` straight through `handle_key`
/// regardless of focus. The focus either side of the call is printed on every line so the operator
/// can see the pseudo-ASID land rather than having to take this paragraph on trust. When rung 5 arms
/// `pidesk`, the SHELLWIN-PI arm compiles in and takes over — no change is owed here.
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
    }
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
            cfg!(feature = "pidesk") as u8, tick, verdict
        );
        return;
    }

    if tick.wrapping_sub(CLK_CENSUS_TICK.load(Ordering::Relaxed)) < CLK_CENSUS_PERIOD {
        return;
    }
    CLK_CENSUS_TICK.store(tick, Ordering::Relaxed);
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
        "[orinclick] census seq={} t={} up={}s btn={} press={} rel={} noedge={} raised={} same={} miss={} consumed={} stuck={} nogeom={} dropped={} rows={} compat={} focus={:#x} -> {}",
        seq, tick, up, btn, press, rel, noedge, raised, same, miss, consumed, stuck,
        nogeom, dropped, rows, compat as u8, sc::user_input_active(), verdict
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
// `video/pidesk.rs:39-44` states the CONSOLEWIN law, inherited from `wcx`: the console window carries
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
// `pidesk::activate()` is NOT called. §5.2 blocks the desktop-ARMING CASCADE — the Pi overflowed a
// 16 KiB kernel stack in it on two consecutive metal boots and no QEMU gate in this tree can stack the
// preemption frame that does it. This rung takes exactly the two steps of `activate`'s sequence the
// console window needs (2a FONT-PI, 2-3 CONSOLEWIN) and none of the rest: no PIDESK DESKTOP-CLEAR
// (which would paint over ORIN-WM1's row and whose soundness argument is an empty window table — the
// floor `main.rs`'s DESKSEAM refuses on), no `menubar::set_enabled`, no crystal, no `render_service`,
// no window population. `quarry` is not implied, so `quarry::open()` — boot 11's ACTUAL overflow, at
// click-router depth — is the `#[cfg(not(feature = "quarry"))]` `false` stub in this build.
//
// WHAT `pidesk` DOES BRING INTO THE ROUTER, stated because §3.7 promised the opposite for `orinclick`
// alone and the difference must not pass unnoticed: `wc_click_route`'s furniture arms
// (`strip::press_route` -> `crystal::press_at` + `dock::press_at`, `pulsewin::press_route`, the DRAG-PI
// chrome arm, the SHELLWIN-PI arm) are compiled IN on an `orinconwin` image. That is not a tolerated
// widening — it is the rung's precondition. `dock::press_at` IS §6.1's route back; without `pidesk`
// there is no dock in the image at all (`video/mod.rs` gates the whole furniture family on it), so a
// minimise disc really would be one-way. `pulsewin::press_route` returns on a NONE window id and
// `quarry::press_route` is the stub, so the two deep arms are unreachable on this build.
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
// `Cargo.toml`): it implies `pidesk` + `tegra_el0`, and `tegra_el0` implies `tegra`, so
// `UNAOS_ORINCONWIN=1 ./arroyo esp-jetson` builds the armed configuration with no second knob — and
// prints the ordering-rule DECLINE, because neither `orindesk` nor `orinclick` came with it.

/// ORIN-CONWIN — one-shot latch. `tegra_early_stop` runs once per boot on the boot core, so this
/// cannot fire today; it is here for `pidesk::activate`'s own reason — `panel_console_window_open` is
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
/// rather than inferred from this function's own control flow — `pidesk::activate`'s discipline, for
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

    // 3. THE CONSOLEWIN LAW'S GEOMETRY HALF, evaluated with the SAME call `pidesk` makes —
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

    // 5. THE CONSOLEWIN LAW'S REFUSAL. Narrowed exactly as `pidesk` narrows it: this guards the console
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
        // LIVE vs FROZEN is the rung's second half and it is read from the BUILD, not from this
        // function: `jd2_console_pump`'s phase-2 detach is guarded by `tegra_conwin_live()`, which is
        // this same route. On this build the guard exists, so a routed console stays live — every
        // kernel line printed after the handoff lands in the window and is composited, damage-limited
        // and paced, by the machinery `fbcon` already carries.
        "LIVE",
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
// ungated, so a tenant CAN be parked on any image. The routes back are the dock (`pidesk` aboard —
// the conjunction image) or kill/exit; the dock round-trip is the ladder's next attended item
// (§3.9.1) and is NOT claimed here. The census prints `pidesk=` so a capture names which image shape
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
        cfg!(feature = "orinconwin") as u8, cfg!(feature = "pidesk") as u8
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
    let n = TEN_REAP_PENDING[asid as usize].swap(0, Ordering::Relaxed);
    if n == 0 {
        return;
    }
    TEN_REAPED.fetch_add(n, Ordering::Relaxed);
    if TEN_LOGGED.fetch_add(1, Ordering::Relaxed) < TEN_LOG_MAX {
        serial_println!("[orintenant] reap asid={} rows={} -> TENANT-REAPED (exit-path funnel: win_close_asid unmapped + freed, wm::close_owner retires the compositor rows next)", asid, n);
    }
}

/// **ORIN-TENANT — the ~10 s census, from `jd2_console_pump`'s idle sweep** (appended line-neutral
/// beside `orin_click_census`'s call). Rung 3's whole argument applies verbatim: this seam's death
/// is a dead pump, nothing re-homes a dead pump's roles, and a census that stops IS the report. It
/// prints on its own core off its own CNTPCT, so it cannot report liveness it does not have;
/// `seq=` increments by one per line so a serial gap names itself.
///
/// Verdict ladder (first match wins), each reachable and none constant:
///   * `FAIL reason=geometry-refused`  — a create was refused over the cap: the pre-parity defect
///     observed live; must never print on a post-parity image running the shipped fixtures.
///   * `DECLINE reason=headless-rows`  — creates succeeded but the compositor refused rows (no
///     panel, or `wm` table full): verbs green, glass empty, said out loud.
///   * `TENANT-LIVE`                   — at least one EL0-owned row is in the table NOW.
///   * `TENANT-EXITED-CLEAN`           — tenants existed and every one left through close/reap.
///   * `IDLE-NO-TENANTS`               — nobody ran an EL0 window program. **UNRUN, never PASS.**
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

    let verdict = if refused != 0 {
        "FAIL reason=geometry-refused"
    } else if creates != 0 && headless != 0 {
        "DECLINE reason=headless-rows"
    } else if rows != 0 {
        "TENANT-LIVE"
    } else if creates != 0 {
        "TENANT-EXITED-CLEAN"
    } else {
        "IDLE-NO-TENANTS"
    };
    serial_println!(
        "[orintenant] census seq={} t={} up={}s rows={} bound={} creates={} headless={} refused={} closes={} reaped={} presents={} suppressed={} focus={:#x} pidesk={} -> {}",
        seq, tick, up, rows, bound, creates, headless, refused, closes, reaped, presents,
        suppressed, sc::user_input_active(), cfg!(feature = "pidesk") as u8, verdict
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
                None => None,
            };
        }
        crate::arch::sched::yield_now();
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
        crate::arch::sched::yield_now();
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
    sup_lock(&SUP_KEYQ).len() >= SUP_KEYQ_CAP
}

/// ORIN-SUPSTATE — push one key for the dispatcher. Returns false (and drops nothing the caller
/// did not already hold) iff the bound would be exceeded — unreachable when the caller honours
/// `sup_key_full`, kept as a hard bound rather than an assumption.
#[cfg(feature = "supstate")]
pub fn sup_key_push(c: u8) -> bool {
    let mut q = sup_lock(&SUP_KEYQ);
    if q.len() >= SUP_KEYQ_CAP {
        return false;
    }
    q.push_back(c);
    true
}

/// ORIN-SUPSTATE — pop one key as the dispatcher. FIFO; `None` when idle.
#[cfg(feature = "supstate")]
pub fn sup_key_pop() -> Option<u8> {
    sup_lock(&SUP_KEYQ).pop_front()
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
// `orinconwin` transitively supplies `pidesk` (the `dock`/`strip` modules) and `tegra_el0` ->
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
/// | `INK-NO-STEM` | non-paper pixels inside the box, but not one fully covered stroke of the console's own ink: something is over the content that is not this console's text |
/// | `GLYPHS-NO-CHROME` | the console's paper and its ink strokes are both on the glass, and the FRAME is not. Text landed, the window's own chrome was overdrawn — §3.8.1's measured JD2-blit overdraw, caught in the act |
/// | `GLYPHS-ON-GLASS` | frame and glyphs both read back at panel coordinates. **This is rung (a) closed** |
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
                    if v == LAD_PAPER {
                        paper += 1;
                    } else {
                        ink += 1;
                    }
                    if v == LAD_INK {
                        stem += 1;
                    }
                }
            }
        }
    }

    // DERIVED, never asserted — the whole point of the line.
    let verdict = if read == 0 {
        "UNREADABLE"
    } else if paper == 0 {
        "WIN2-NOT-ON-GLASS"
    } else if paper == read {
        "BLANK-NO-GLYPHS"
    } else if stem == 0 {
        "INK-NO-STEM"
    } else if fread != 0 && fhit == fread {
        "GLYPHS-ON-GLASS"
    } else {
        "GLYPHS-NO-CHROME"
    };
    serial_println!(
        "[oringlass] phase={} win={} box={}x{} at ({},{}) content={}x{} at ({},{}) scale={} onpanel={} frame={}/{} samples={} read={} paper={} ink={} stem={} first={:#08x} uniform={} -> {}",
        phase, i.id, ow, oh, ox, oy, cwp, chp, i.x, i.y, i.scale,
        if on_panel { "yes" } else { "PARKED" },
        fhit, fread,
        LAD_BANDS * LAD_RUNS * LAD_RUN,
        read, paper, ink, stem, first,
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
        cfg!(feature = "orindesk") as u8, cfg!(feature = "pidesk") as u8,
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
            let painted = glass == "GLYPHS-ON-GLASS" || glass == "GLYPHS-NO-CHROME";
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
