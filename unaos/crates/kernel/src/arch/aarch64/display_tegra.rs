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

    // 3. Resolve the simple-framebuffer handoff (the safe primary path).
    let sfb = fdt_tegra::nvdisplay_simplefb(dtb_addr, dtb_size, ram_gib_mask)?;

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
    }
    Some(ScanoutFb { base: sfb.base, len, info })
}

/// Paint a distinctive test pattern into the inherited scanout so a bench boot can confirm — before
/// fbcon takes the screen for the boot log — that the inherited `{base, stride, format}` actually
/// land on the panel: a wrong stride shears the bars, a wrong format swaps blue↔red, a wrong base
/// shows nothing. Eight colour bars + a bright frame. Flushed to RAM for the (non-coherent) DCE.
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
