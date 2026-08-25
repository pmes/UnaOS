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
    ); jd1_dc_model(base, size, have_cap, first); // JD1-DC-MODEL — the WHICH-CHIP discriminator, four more read-only reads, appended here because this is the first instruction at which an nvdisplay read is known non-fatal and the long window sweep has not yet risked the boot. Without it `VERDICT=DECODES-NOMATCH` is AMBIGUOUS between "the aperture is not live" and "our register map is Tegra194's and this silicon is Tegra234" — two answers that send the display arc in opposite directions. See the JD1-DC-MODEL block at this file's tail. APPENDED to this line, never a new one: knob-off it is cfg-erased and not one `core::panic::Location` below moves.

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
            ":: tegra: JD1-DC VERDICT=DECODES-NOMATCH — the aperture DECODES ({} window(s) at WIN_ENABLE=1 out of {} swept), but no START_ADDR equalled the JD1 scanout base {:#x}{}. Reachability is ESTABLISHED; the scanout is fed from a window this sweep's head model did not cover (swept=0 means the aperture held the FE capability word but no complete head-0 window bank) — compare the RAW lines above against DISPLAY_FE_SW_SYS_CAP={:#010x}, which is UEFI's own head/SOR enumeration. THIS VERDICT DOES NOT SAY WHY: on its own it cannot separate 'the aperture is live and the window is somewhere we did not look' from 'our window offsets are Tegra194's and this silicon is not Tegra194'. The JD1-DC MODEL-VERDICT line ABOVE is the one that separates them — read it first ::",
            enabled,
            swept,
            scanout,
            if scanout == 0 { " (JD1 resolved NO scanout this boot, so no comparison was possible — the match test is vacuous here, not failed)" } else { "" },
            first,
        );
    } else {
        serial_println!(
            ":: tegra: JD1-DC VERDICT=NOT-DECODING — {} of {} windows read all-ones and {} read all-zero, none enabled. READ THIS CAREFULLY: UEFI drives THESE registers on THIS board from THIS core by plain MmioRead32/MmioWrite32 (edk2-nvidia NvDisplayControllerDxe/NvDisplayHw.c), and the string 'dce' appears nowhere in that tree — so this result is NOT 'the DCE holds the block'. It is a finding about OUR access path: our aperture/mapping, the domain or clock state at our probe point, or an SCR narrower for us than for UEFI. FE_SW_SYS_CAP read {:#010x}. AND IT IS NOT A STATEMENT ABOUT THE WINDOW OFFSETS EITHER — a wrong register map produces DECODES-NOMATCH, not this; if the JD1-DC MODEL-VERDICT line above says the aperture answered anything at all, prefer that line's reading of it over this one ::",
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
