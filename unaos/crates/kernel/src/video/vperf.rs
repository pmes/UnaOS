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

//! VPERF — video-path performance instrumentation (x86_64, `videobench` feature only).
//!
//! Root cause under measurement: on the 2012 rMBP the EFI GOP framebuffer (2880x1800,
//! stride 4096 px, 4 bpp ≈ 28.1 MiB) is scanned by the discrete GPU; every CPU *read* of it is
//! an uncached PCIe round trip. `FrameBuffer::scroll_up` is a full-surface `core::ptr::copy`
//! whose SOURCE is that framebuffer — ~28.0 MiB read + ~28.0 MiB written per 8-px text line.
//! This module counts those bytes so the M3 shadow fix ("scroll in cached RAM, blit write-only")
//! is provable: after M3 the VRAM-read counter must be 0 across the scripted scroll scenario.
//!
//! Everything here is bench-only plumbing:
//! - relaxed atomic counters bumped from `FrameBuffer::scroll_up` / `put_pixel` (cfg-gated hooks);
//! - a RAW SERIAL emitter — vperf lines must NOT mirror onto fbcon (a per-scroll line printed
//!   through fbcon would itself scroll and recurse, and counter values on screen would break the
//!   pre/post-M3 screendump pixel-compare);
//! - the effective-framebuffer-memory-type readout (MTRR + live PTE + PAT — the WC assumption at
//!   `arch/x86_64/mod.rs` is inherited from firmware and UNVERIFIED; this prints the truth, which
//!   on metal is bench data and under QEMU/TCG is synthetic);
//! - a PCI class-0x03 display probe (which device's BAR owns the fb address);
//! - the deterministic scripted scroll scenario the DONE gates screendump-compare against.
//!
//! Compiled only under `cfg(all(target_arch = "x86_64", feature = "videobench"))` (see
//! `video/mod.rs`), so the aarch64 artifact is byte-identical with the knob on OR off.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Bytes moved by `FrameBuffer::scroll_up` (source read + destination write counted once, as the
/// memmove payload size), across ALL surfaces (VRAM and RAM shadows alike).
static SCROLL_BYTES: AtomicU64 = AtomicU64::new(0);
/// Bytes `scroll_up` READ from the real framebuffer (the uncached-PCIe cost being eliminated).
static VREAD_BYTES: AtomicU64 = AtomicU64::new(0);
/// `put_pixel` invocations (per-pixel glyph pokes + per-pixel fills).
static PX_CALLS: AtomicU64 = AtomicU64::new(0);
/// `scroll_up` invocations (the cadence base for the emitted line).
static SCROLLS: AtomicU64 = AtomicU64::new(0);

/// The real framebuffer range, registered by `fbcon::init` from BootInfo. `scroll_up` classifies
/// its source against this to attribute VRAM reads (a heap shadow/back buffer never matches).
static VRAM_BASE: AtomicUsize = AtomicUsize::new(0);
static VRAM_LEN: AtomicUsize = AtomicUsize::new(0);

/// Record the real framebuffer's identity-mapped range (called once from `fbcon::init`).
pub fn set_vram_range(base: usize, len: usize) {
    VRAM_BASE.store(base, Ordering::Relaxed);
    VRAM_LEN.store(len, Ordering::Relaxed);
}

/// True iff `base` lies inside the registered real-framebuffer range.
#[inline]
pub fn in_vram(base: usize) -> bool {
    let vb = VRAM_BASE.load(Ordering::Relaxed);
    let vl = VRAM_LEN.load(Ordering::Relaxed);
    vb != 0 && base >= vb && base < vb + vl
}

/// Hook: one `put_pixel` call (hot; a single relaxed add).
#[inline]
pub fn on_put_pixel() {
    PX_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// Hook: one `scroll_up` memmove of `bytes`, whose source surface base is `src_base`.
/// Emits the counter line on a cadence (first scroll + every 128th) — RAW SERIAL ONLY.
pub fn on_scroll(bytes: u64, src_base: usize) {
    SCROLL_BYTES.fetch_add(bytes, Ordering::Relaxed);
    if in_vram(src_base) {
        VREAD_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }
    let n = SCROLLS.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n % 128 == 0 {
        raw_print(format_args!(
            ":: vperf: scroll={} vread={} px={} ::\n",
            SCROLL_BYTES.load(Ordering::Relaxed),
            VREAD_BYTES.load(Ordering::Relaxed),
            PX_CALLS.load(Ordering::Relaxed),
        ));
    }
}

/// Serial-only print: the UART leg AND the FTDI boot-capture-ring leg of `arch::serial::_print`,
/// but NEVER the fbcon or selftest mirrors. Two reasons this must not touch fbcon: (1) `on_scroll`
/// fires while the FBCON lock is held — a mirrored line would try to draw (and possibly scroll)
/// recursively; (2) counter values on screen would differ between the pre- and post-M3 runs and
/// break the screendump compare. `try_lock` + skip, IRQ-masked — safe from any print context (the
/// serial lock is NOT held when fbcon draws: `serial::_print` writes the UART and releases before
/// mirroring). VPERF-OBS: the FTDI ring leg is what makes these readout lines (fbmem/display/
/// scenario) VISIBLE on a metal rMBP, where `SERIAL1` is `None` (no 16550 — the FTDI console is why)
/// so the UART leg alone drops them silently; `ftdi::mirror` is `try_lock`-only + never takes the
/// FBCON lock, so both constraints above still hold. In QEMU (`SERIAL1` present) the UART leg already
/// reaches `serial.log` and the ring is not replayed there, so QEMU logs are byte-unchanged.
fn raw_print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    crate::arch::without_interrupts(|| {
        if let Some(mut guard) = crate::arch::serial::SERIAL1.try_lock() {
            if let Some(uart) = guard.as_mut() {
                let _ = uart.write_fmt(args);
            }
        }
        // The SAME leg 3 `serial::_print` uses (never legs 2/4 = fbcon/selftest) — metal visibility.
        crate::drivers::xhci::ftdi::mirror(args);
    });
}

// -------------------------------------------------------------------------------------------
// Effective framebuffer memory type — MTRR + live PTE + PAT readout (read-only).
//
// The kernel programs zero PAT/MTRR state and runs on the firmware identity map, so the
// framebuffer's effective memory type is whatever firmware left. QEMU/TCG's picture is synthetic
// (fb = host RAM; MTRRs typically zeroed); the readout that matters is the attended metal bench.
// -------------------------------------------------------------------------------------------

const IA32_MTRRCAP: u32 = 0xFE;
const IA32_MTRR_DEF_TYPE: u32 = 0x2FF;
const IA32_MTRR_PHYSBASE0: u32 = 0x200;
const IA32_PAT: u32 = 0x277;

fn rdmsr(msr: u32) -> u64 {
    unsafe { x86_64::registers::model_specific::Msr::new(msr).read() }
}

/// Decode an architectural memory-type encoding (MTRR type field / PAT entry).
fn type_name(t: u8) -> &'static str {
    match t {
        0 => "UC",
        1 => "WC",
        4 => "WT",
        5 => "WP",
        6 => "WB",
        7 => "UC-", // valid only as a PAT entry
        _ => "??",
    }
}

/// Walk the live CR3 tables (identity map, physical_memory_offset == 0) and return the LEAF
/// entry mapping `va` plus its level (1 = 4 KiB, 2 = 2 MiB, 3 = 1 GiB). Read-only; the local twin
/// of `arch::x86_64::memory::translate` that keeps the raw entry (we need PWT/PCD/PAT bits).
fn walk_leaf(va: u64) -> Option<(u64, u8)> {
    const PRESENT: u64 = 1;
    const HUGE: u64 = 1 << 7;
    const ADDR: u64 = 0x000F_FFFF_FFFF_F000;
    let idx = |shift: u64| ((va >> shift) & 0x1FF) as usize;
    unsafe {
        let pml4 = x86_64::registers::control::Cr3::read().0.start_address().as_u64() as *const u64;
        let pml4e = *pml4.add(idx(39));
        if pml4e & PRESENT == 0 {
            return None;
        }
        let pdpte = *((pml4e & ADDR) as *const u64).add(idx(30));
        if pdpte & PRESENT == 0 {
            return None;
        }
        if pdpte & HUGE != 0 {
            return Some((pdpte, 3));
        }
        let pde = *((pdpte & ADDR) as *const u64).add(idx(21));
        if pde & PRESENT == 0 {
            return None;
        }
        if pde & HUGE != 0 {
            return Some((pde, 2));
        }
        let pte = *((pde & ADDR) as *const u64).add(idx(12));
        if pte & PRESENT == 0 {
            return None;
        }
        Some((pte, 1))
    }
}

/// The MTRR memory type covering physical address `pa`, or a note when MTRRs are absent/disabled.
/// Returns (type, "how"): variable-range hit, default type, or disabled=UC.
fn mtrr_type_for(pa: u64) -> (u8, &'static str) {
    let def = rdmsr(IA32_MTRR_DEF_TYPE);
    if def & (1 << 11) == 0 {
        return (0, "disabled"); // MTRRs disabled => all UC
    }
    let vcnt = (rdmsr(IA32_MTRRCAP) & 0xFF) as u32;
    // Overlap resolution (SDM: UC wins; WT beats WB). Track the strongest match.
    let mut hit: Option<u8> = None;
    for i in 0..vcnt {
        let base = rdmsr(IA32_MTRR_PHYSBASE0 + 2 * i);
        let mask = rdmsr(IA32_MTRR_PHYSBASE0 + 2 * i + 1);
        if mask & (1 << 11) == 0 {
            continue; // range not valid
        }
        let m = mask & 0x000F_FFFF_FFFF_F000;
        if (pa & m) == (base & m) {
            let t = (base & 0xFF) as u8;
            hit = Some(match hit {
                None => t,
                Some(prev) => {
                    if prev == 0 || t == 0 {
                        0 // UC dominates
                    } else if prev == 4 || t == 4 {
                        4 // WT beats WB in overlaps
                    } else {
                        t
                    }
                }
            });
        }
    }
    match hit {
        Some(t) => (t, "var-range"),
        None => (((def & 0xFF) as u8), "default"),
    }
}

/// Combine MTRR and PAT types per Intel SDM Vol. 3A Table 11-7 (the effective memory type).
fn effective_type(mtrr: u8, pat: u8) -> u8 {
    match pat {
        0 => 0,                       // PAT UC: UC regardless of MTRR
        7 => {
            if mtrr == 1 { 1 } else { 0 } // PAT UC-: UC, except WC MTRR -> WC
        }
        1 => 1,                       // PAT WC: WC regardless of MTRR
        4 => match mtrr {
            6 | 4 => 4,               // WB/WT -> WT
            5 => 4,                   // WP -> WT (SDM: implementation may map to UC; report WT)
            _ => 0,                   // UC/WC -> UC
        },
        5 => match mtrr {
            6 | 5 => 5,               // WB/WP -> WP
            _ => 0,
        },
        6 => mtrr,                    // PAT WB: the MTRR type rules
        _ => 0xFF,
    }
}

/// Print the effective framebuffer memory type: MTRR coverage of the fb physical base, the live
/// leaf PTE (raw, so PWT/PCD/PAT bits are visible), the IA32_PAT entry it selects, and the
/// combined effective type. Read-only (RDMSR + page-walk loads). Guarded by CPUID feature bits so
/// a model without MTRR/PAT never RDMSRs an unimplemented register.
pub fn report_fbmem() {
    let fb = VRAM_BASE.load(Ordering::Relaxed) as u64;
    if fb == 0 {
        raw_print(format_args!(":: vperf: fbmem no framebuffer registered ::\n"));
        return;
    }
    let cpuid1 = unsafe { core::arch::x86_64::__cpuid(1) };
    let has_mtrr = cpuid1.edx & (1 << 12) != 0;
    let has_pat = cpuid1.edx & (1 << 16) != 0;

    let (mtrr_t, how) = if has_mtrr { mtrr_type_for(fb) } else { (0xFF, "no-mtrr") };

    // Identity map: the fb's virtual address IS its physical address.
    let (pte, level, pat_t) = match walk_leaf(fb) {
        Some((e, lvl)) => {
            let pwt = (e >> 3) & 1;
            let pcd = (e >> 4) & 1;
            // The PAT bit sits at bit 7 in a 4 KiB PTE but bit 12 in a 2 MiB/1 GiB leaf.
            let pat_bit = if lvl == 1 { (e >> 7) & 1 } else { (e >> 12) & 1 };
            let idx = (pat_bit << 2 | pcd << 1 | pwt) as u32;
            let pat_entry = if has_pat {
                ((rdmsr(IA32_PAT) >> (8 * idx)) & 0x7) as u8
            } else {
                // No PAT: the legacy PCD/PWT table (WB / WT / UC- / UC).
                match idx & 3 {
                    0 => 6,
                    1 => 4,
                    2 => 7,
                    _ => 0,
                }
            };
            (e, lvl, pat_entry)
        }
        None => (0, 0, 0xFF),
    };

    let eff = if mtrr_t <= 7 && pat_t <= 7 { effective_type(mtrr_t, pat_t) } else { 0xFF };
    raw_print(format_args!(
        ":: vperf: fbmem mtrr={}({}) pte={:#x}(l{}) pat={} eff={} fb={:#x} ::\n",
        type_name(mtrr_t),
        how,
        pte,
        level,
        type_name(pat_t),
        type_name(eff),
        fb,
    ));
}

// -------------------------------------------------------------------------------------------
// PCI display-class probe: which class-0x03 device's BAR range contains the framebuffer.
// -------------------------------------------------------------------------------------------

/// Log every PCI class-0x03 (display) function and which one's memory BAR owns the framebuffer
/// address. READ-ONLY: no BAR sizing writes (the display is live-scanned-out on metal — toggling
/// decode mid-scanout is not worth a bench line), so ownership is a heuristic: the display BAR
/// with the highest base at/below the fb address within a 512 MiB window (the rMBP's GT 650M
/// exposes a 256 MiB prefetchable aperture; QEMU's std VGA a 16 MiB one).
pub fn pci_display_probe() {
    let fb = VRAM_BASE.load(Ordering::Relaxed) as u64;
    let mut owner: Option<(u16, u16, usize, u64)> = None; // (vid, did, bar index, base)
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let id = unsafe { crate::arch::pci::read_config_32(bus, dev, func, 0x00) };
                if id == 0xFFFF_FFFF {
                    if func == 0 {
                        break; // no device; skip the other functions
                    }
                    continue;
                }
                let class_reg = unsafe { crate::arch::pci::read_config_32(bus, dev, func, 0x08) };
                if ((class_reg >> 24) & 0xFF) as u8 != 0x03 {
                    continue;
                }
                let vid = (id & 0xFFFF) as u16;
                let did = (id >> 16) as u16;
                raw_print(format_args!(
                    ":: vperf: display {:04x}:{:04x} class {:06x} @ {}:{}.{} ::\n",
                    vid,
                    did,
                    class_reg >> 8,
                    bus,
                    dev,
                    func
                ));
                // Type-0 header: six 32-bit BARs at 0x10..0x28. Decode memory BARs (bit 0 = 0);
                // a 64-bit BAR (type field 0b10) consumes the next slot as the high dword.
                let mut bar = 0usize;
                while bar < 6 {
                    let off = 0x10 + (bar as u8) * 4;
                    let lo = unsafe { crate::arch::pci::read_config_32(bus, dev, func, off) };
                    let is_64 = lo & 0x1 == 0 && (lo >> 1) & 0x3 == 0x2;
                    if lo & 0x1 == 0 {
                        let mut base = (lo & 0xFFFF_FFF0) as u64;
                        if is_64 {
                            let hi = unsafe {
                                crate::arch::pci::read_config_32(bus, dev, func, off + 4)
                            };
                            base |= (hi as u64) << 32;
                        }
                        if fb != 0 && base != 0 && fb >= base && fb - base < 0x2000_0000 {
                            let better = match owner {
                                None => true,
                                Some((_, _, _, b)) => base > b,
                            };
                            if better {
                                owner = Some((vid, did, bar, base));
                            }
                        }
                    }
                    bar += if is_64 { 2 } else { 1 };
                }
            }
        }
    }
    match owner {
        Some((vid, did, bar, base)) => raw_print(format_args!(
            ":: vperf: display {:04x}:{:04x} bar{} owns fb (base={:#x}) ::\n",
            vid, did, bar, base
        )),
        None => raw_print(format_args!(":: vperf: display — no BAR claims fb ::\n")),
    }
}

// -------------------------------------------------------------------------------------------
// The scripted scroll scenario — the deterministic workload every DONE gate measures/compares.
// -------------------------------------------------------------------------------------------

/// Lines the scenario prints. Enough to scroll the frame several times over at any plausible
/// mode (2880x1800 -> 225 text rows; QEMU GOP modes are far smaller).
const SCENARIO_LINES: usize = 400;
/// Boot-relative start time: late enough that every one-shot storage/syscall fixture in the
/// usbdebug loop has fired and gone quiet, so the post-scenario screen content is exactly the
/// scenario's own tail — deterministic across runs, which is what the screendump compare needs.
const SCENARIO_START_MS: u64 = 15_000;

static SCENARIO_DONE: AtomicBool = AtomicBool::new(false);

/// One-shot scripted scroll scenario, pumped from the usbdebug main loop. Clears the console,
/// prints `SCENARIO_LINES` fixed-width numbered lines through the normal serial->fbcon mirror
/// (forcing scrolls), then emits the counter DELTAS raw-serial-only. Nothing else prints after
/// it in a headless usbdebug run, so the framebuffer settles on the scenario tail.
pub fn scenario_tick() {
    if SCENARIO_DONE.load(Ordering::Relaxed) {
        return;
    }
    if crate::arch::ms() < SCENARIO_START_MS {
        return;
    }
    if SCENARIO_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    raw_print(format_args!(":: vperf: scenario start ::\n"));
    let s0 = SCROLL_BYTES.load(Ordering::Relaxed);
    let v0 = VREAD_BYTES.load(Ordering::Relaxed);
    let p0 = PX_CALLS.load(Ordering::Relaxed);
    let n0 = SCROLLS.load(Ordering::Relaxed);
    crate::video::fbcon::clear();
    for i in 0..SCENARIO_LINES {
        serial_println!("vperf-scroll line {:04} abcdefghijklmnopqrstuvwxyz0123456789", i);
    }
    raw_print(format_args!(
        ":: vperf: scenario scroll={} vread={} px={} scrolls={} lines={} ::\n",
        SCROLL_BYTES.load(Ordering::Relaxed) - s0,
        VREAD_BYTES.load(Ordering::Relaxed) - v0,
        PX_CALLS.load(Ordering::Relaxed) - p0,
        SCROLLS.load(Ordering::Relaxed) - n0,
        SCENARIO_LINES,
    ));
    raw_print(format_args!(":: vperf: scenario done ::\n"));
}
