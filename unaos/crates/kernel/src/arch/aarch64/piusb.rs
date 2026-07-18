// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PI-USB-1 — BCM2711 PCIe root complex + VIA VL805 xHCI attach on the Raspberry Pi 4.
//
// Every USB-A port on the Pi 4 hangs off ONE endpoint: the VIA VL805 xHCI (PCI 1106:3483), which sits
// behind the BCM2711's single PCIe root complex (`pcie@7d500000`, ARM-physical `0xFD50_0000` in
// low-peripheral mode). Bringing USB up therefore means bringing PCIe up first: reset the brcmstb
// bridge, wait for the link, program the outbound MEM window + inbound DMA BAR, enumerate the VL805,
// size its BAR, tell the VideoCore firmware to (re)load the VL805 firmware over the mailbox, then
// attach the shared xHCI driver to the "controller halted-but-decoding + ports powered" honesty line.
//
// ── ATTENDED-METAL-UNVERIFIED ──────────────────────────────────────────────────────────────────────
// QEMU's `raspi4b` machine does NOT model the PCIe root complex. Everything past the RC identity read
// (`rc_alive`) therefore runs ONLY on real silicon at an attended sitting; in QEMU the RC register
// block reads open-bus (0 / all-ones), the identity gate says "absent", and the bring-up returns after
// a single graceful-degradation line. So M1..M3 below are correct-by-construction against the Linux
// references cited inline, NOT QEMU-exercised. Do NOT treat "no RC in QEMU" as a divergence. This is
// the same discipline as PI-V3D-1 (arch/aarch64/v3d.rs) and ORIN-NET-3 (arch/aarch64/pcie_probe.rs).
//
// References of record (fold the key facts into arch_arm64.md §PI-USB):
//   * brcmstb PCIe RC: Linux `drivers/pci/controller/pcie-brcmstb.c` (bridge sw-init/reset, HARD_DEBUG
//     SERDES power-up, PCIE_MISC_PCIE_STATUS link bits, CPU_2_PCIE_MEM_WIN0 outbound window, RC_BAR2
//     inbound window, EXT_CFG_INDEX/DATA child config access). Offsets below are the BCM2711 values.
//   * VL805 firmware reset: the RPi firmware `NOTIFY_XHCI_RESET` mailbox tag (0x00030058) — normally
//     issued by the RPi bootloader/EEPROM at power-on; an OS bringing the controller up re-issues it.
//   * xHCI attach: the shared `drivers/xhci` driver, in the polled-aarch64 mode (the JB2b platform-
//     attach pattern, arch/aarch64/xusb_tegra.rs — adapted to a PCIe-BAR base instead of a fixed
//     platform MMIO base).
//
// ## The poison-rejection rule (PI-V3D-1, the cautionary tale)
//
// PI-V3D-1's attended sitting found a block that never decoded yet returned the firmware's `0xdeadbeef`
// fill, which a zero-only liveness gate FALSE-PASSED as "present". Every liveness read here rejects BOTH
// `0xffffffff` (PCIe master-abort / open-bus) AND `0xdeadbeef` (firmware fill) as ABSENT DECODE.
//
// ## Write discipline (the arc's review lens)
//
// This arc performs deliberate MMIO/config writes (bridge reset, window config, BAR sizing, decode
// enable, port power) — but ONLY to the BCM2711 RC register block and the VL805's own config/BAR, and
// every BAR-sizing probe restores the original immediately (the ORIN-NET-3 ritual). No other device is
// touched. The whole module is `#[cfg(all(feature = "baremetal", feature = "piusb"))]`; knob-off it and
// its single call site (mailbox.rs tail) vanish and the kernel8 image is byte-identical to baseline.

use super::mailbox;
use crate::drivers::xhci;

/// Stable serial prefix so the operator (and `mbench`) can grep the whole bring-up as one block.
const P: &str = ":: PIUSB:";

// ─── BCM2711 PCIe RC register block (ARM-physical; inside the 0xC000_0000–0xFFFF_FFFF Device-nGnRnE
// window `boot.rs` L1[3] already maps — no new mapping needed to reach ANY of the RC registers or the
// EXT_CFG child-config window). ───
const RC_BASE: u64 = 0xFD50_0000;

// brcmstb RC register offsets (Linux pcie-brcmstb.c, BCM2711 values).
const PCIE_RC_CFG_VENDOR_SPECIFIC: u64 = 0x0188; // (unused here; kept as a documented landmark)
const PCIE_MISC_MISC_CTRL: u64 = 0x4008;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LO: u64 = 0x400C;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_HI: u64 = 0x4010;
const PCIE_MISC_RC_BAR2_CONFIG_LO: u64 = 0x4034; // inbound (PCIe->RAM DMA) BAR, low
const PCIE_MISC_RC_BAR2_CONFIG_HI: u64 = 0x4038;
const PCIE_MISC_PCIE_STATUS: u64 = 0x4068;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT: u64 = 0x4070;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI: u64 = 0x4080;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI: u64 = 0x4084;
const PCIE_MISC_HARD_PCIE_HARD_DEBUG: u64 = 0x4204;
const PCIE_EXT_CFG_DATA: u64 = 0x8000; // 4 KiB child-config data window
const PCIE_EXT_CFG_INDEX: u64 = 0x9000; // child-config BDF index register
const PCIE_RGR1_SW_INIT_1: u64 = 0x9210; // bridge sw-init + PERST

// Field bits.
const PCIE_MISC_PCIE_STATUS_PHYLINKUP: u32 = 1 << 4;
const PCIE_MISC_PCIE_STATUS_DL_ACTIVE: u32 = 1 << 5;
const PCIE_RGR1_SW_INIT_1_PERST: u32 = 1 << 0; // PERST# assert (1 = in reset)
const PCIE_RGR1_SW_INIT_1_INIT_GENERIC: u32 = 1 << 1; // bridge core reset (1 = in reset)
const PCIE_MISC_HARD_DEBUG_SERDES_IDDQ: u32 = 1 << 27; // 1 = serdes powered DOWN; clear to power up

// ─── The VL805 endpoint. It enumerates as bus 1, dev 0, fn 0 behind the RC's single downstream port. ───
const VL805_VENDOR: u16 = 0x1106; // VIA Technologies
const VL805_DEVICE: u16 = 0x3483; // VL805 USB 3.0 xHCI
/// The VL805's PCI device address for the `NOTIFY_XHCI_RESET` mailbox: (bus<<20)|(dev<<15)|(fn<<12).
const VL805_DEV_ADDR: u32 = (1 << 20) | (0 << 15) | (0 << 12); // 0x0010_0000

// ─── The PCIe outbound MEM window. Firmware/DT place it at CPU-physical 0x6_0000_0000 (24 GiB), where
// the PCIe-side address 0xC000_0000 is decoded — the canonical Pi 4 `ranges` mapping
// (`0x02000000 0 0xc0000000  0x6 0x00000000  0 0x40000000`). This is OUTSIDE `boot.rs`'s fixed 0–4 GiB
// map, so M3 installs one 1 GiB Device block for it (`boot::map_device_1gib`) before reading the BAR. ───
const OUTBOUND_CPU_BASE: u64 = 0x6_0000_0000; // ARM-physical (where the CPU reads the VL805 BAR)
const OUTBOUND_PCIE_BASE: u64 = 0xC000_0000; // PCIe-side (what the VL805 BAR is programmed to)
const OUTBOUND_SIZE: u64 = 0x4000_0000; // 1 GiB

#[inline]
fn r(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
#[inline]
fn w(addr: u64, v: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}
#[inline]
fn dsb() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Open-bus / firmware-fill poison signatures — NEITHER is ever live data (PI-V3D-1 false-PASS rule).
#[inline]
fn is_poison(v: u32) -> bool {
    v == 0xFFFF_FFFF || v == 0xDEAD_BEEF
}

/// A config-space `vendor:device` word is LIVE only if it is neither poison nor an absent-decode
/// boundary (vendor 0x0000 = no responder, 0xffff = unclaimed). Returns `Some((vendor, device))`.
#[inline]
fn live_vendor_device(word: u32) -> Option<(u16, u16)> {
    if is_poison(word) {
        return None;
    }
    let vendor = (word & 0xffff) as u16;
    let device = (word >> 16) as u16;
    if vendor == 0x0000 || vendor == 0xffff {
        return None;
    }
    Some((vendor, device))
}

/// Busy-wait a bounded ~`ms` milliseconds off the free-running CNTPCT — a settle delay for a resetting
/// block. Finite by construction (the anti-hang rule; the ORIN-SMP determinism lesson).
fn settle_ms(ms: u64) {
    let deadline = super::timer::cntpct() + (super::timer::cntfrq() * ms) / 1000;
    while super::timer::cntpct() < deadline {
        core::hint::spin_loop();
    }
}

/// Bounded microsecond settle (for the short brcmstb reset pulses).
fn settle_us(us: u64) {
    let deadline = super::timer::cntpct() + (super::timer::cntfrq() * us) / 1_000_000 + 1;
    while super::timer::cntpct() < deadline {
        core::hint::spin_loop();
    }
}

// ── Child (VL805) config access via the brcmstb EXT_CFG window. Root-port (bus 0) config is at the RC
// base directly; a downstream device (bus 1) is reached by writing its BDF to EXT_CFG_INDEX then
// reading/writing at EXT_CFG_DATA + offset. ──

/// Point the EXT_CFG window at bus 1 / dev 0 / fn 0 (the VL805). Idempotent; call before each cfg op.
fn vl805_cfg_select() {
    // Linux encodes the child config index as (bus<<20)|(dev<<15)|(fn<<12); offset comes from the DATA
    // window. We select once per access to be robust against any intervening RC register touch.
    w(RC_BASE + PCIE_EXT_CFG_INDEX, VL805_DEV_ADDR);
    dsb();
}
#[inline]
fn vl805_cfg_read(off: u64) -> u32 {
    vl805_cfg_select();
    r(RC_BASE + PCIE_EXT_CFG_DATA + off)
}
#[inline]
fn vl805_cfg_write(off: u64, v: u32) {
    vl805_cfg_select();
    w(RC_BASE + PCIE_EXT_CFG_DATA + off, v);
    dsb();
}

/// Minimal, self-bounded flat-device-tree scan: does the firmware DTB at `dtb` contain a node whose
/// name begins `pcie@`? This is the CENSUS-BEFORE-TOUCH gate — the sole thing that runs in QEMU
/// raspi4b, and it touches only RAM (the DTB blob), never the RC MMIO. QEMU's DTB has no `pcie@` node,
/// so this returns false and the caller skips before the unbacked RC read (which would external-abort
/// with no vectors installed at this pre-`kernel_main` point). A malformed/absent blob returns false.
///
/// FDT format (devicetree.org spec): big-endian header {magic 0xd00dfeed, totalsize, off_dt_struct,
/// off_dt_strings, ...}; the structure block is a token stream — FDT_BEGIN_NODE(0x1) followed by a
/// NUL-padded node name, FDT_END_NODE(0x2), FDT_PROP(0x3){len,nameoff}+padded value, FDT_NOP(0x4),
/// FDT_END(0x9). We only need FDT_BEGIN_NODE names, bounded strictly by totalsize.
fn dtb_has_pcie(dtb: u64) -> bool {
    if dtb == 0 {
        return false;
    }
    let be32 = |off: usize| -> u32 {
        unsafe {
            let p = (dtb as usize + off) as *const u8;
            u32::from_be_bytes([
                p.read_volatile(),
                p.add(1).read_volatile(),
                p.add(2).read_volatile(),
                p.add(3).read_volatile(),
            ])
        }
    };
    if be32(0) != 0xd00d_feed {
        return false; // not an FDT
    }
    let totalsize = be32(4) as usize;
    let off_struct = be32(8) as usize;
    if totalsize < 40 || totalsize > 4 * 1024 * 1024 || off_struct >= totalsize {
        return false; // implausible header — refuse
    }
    let mut pos = off_struct;
    let mut guard = 0usize; // hard bound on token count (never an unbounded walk)
    while pos + 4 <= totalsize && guard < 200_000 {
        guard += 1;
        let tok = be32(pos);
        pos += 4;
        match tok {
            0x1 => {
                // FDT_BEGIN_NODE: read the NUL-terminated name and test its leaf for "pcie@".
                let name_start = pos;
                let mut end = name_start;
                while end < totalsize {
                    let b = unsafe { ((dtb as usize + end) as *const u8).read_volatile() };
                    if b == 0 {
                        break;
                    }
                    end += 1;
                }
                // The node name may be a leaf ("pcie@7d500000") — check its start after any '@'-free
                // '/'; DTB node names carry no '/', so just test the raw name prefix.
                let name_len = end - name_start;
                if name_len >= 5 {
                    let mut matches = true;
                    for (i, &c) in b"pcie@".iter().enumerate() {
                        let b = unsafe { ((dtb as usize + name_start + i) as *const u8).read_volatile() };
                        if b != c {
                            matches = false;
                            break;
                        }
                    }
                    if matches {
                        return true;
                    }
                }
                // Advance past the name + its NUL, rounded up to a 4-byte boundary.
                pos = (end + 1 + 3) & !3;
            }
            0x3 => {
                // FDT_PROP: {len(4), nameoff(4)} then `len` value bytes, padded to 4.
                if pos + 8 > totalsize {
                    break;
                }
                let len = be32(pos) as usize;
                pos += 8;
                pos = (pos + len + 3) & !3;
            }
            0x2 | 0x4 => {} // END_NODE / NOP: no payload
            0x9 => break, // FDT_END
            _ => break,   // unknown token — stop (malformed)
        }
    }
    false
}

/// Entry point: bring the BCM2711 PCIe RC + VL805 xHCI up to the honesty line. Called once on the BSP,
/// single-threaded, from `build_boot_info` (pre-SMP, pre-heap, mailbox idle). Heap-free: the attach
/// stops at "halted-but-decoding + ports powered" and never allocates rings (full enumeration is the
/// metal follow-on). `dtb` is the firmware device tree from x0 — censused for a `pcie@` node BEFORE any
/// RC MMIO (QEMU raspi4b has none → clean skip). Every wait is a FINITE wall-clock backstop off CNTPCT.
pub fn bringup(dtb: u64) {
    serial_println!("{} PI-USB-1 bring-up starting (BCM2711 PCIe RC @ {:#x} + VL805 xHCI) ::", P, RC_BASE);

    // CENSUS-BEFORE-TOUCH: only proceed to RC MMIO if the firmware DTB describes a `pcie@` controller.
    // QEMU raspi4b models no PCIe RC and its DTB has no such node — so this returns before the unbacked
    // RC read that would external-abort pre-vectors. On metal the Pi firmware DTB has `pcie@7d500000`.
    if !dtb_has_pcie(dtb) {
        serial_println!(
            "{} no `pcie@` node in the firmware DTB (@{:#x}) — no BCM2711 PCIe RC (expected in QEMU raspi4b; models no RC) — USB bring-up skipped, graceful degradation ::",
            P, dtb
        );
        return;
    }
    serial_println!("{} DTB census: `pcie@` controller present — proceeding to RC bring-up ::", P);

    // ── M1: brcmstb RC bring-up. ─────────────────────────────────────────────────────────────────
    if !m1_rc_bringup() {
        serial_println!("{} M1 RC bring-up did not reach link-up — USB bring-up skipped (see lines above) ::", P);
        return;
    }

    // ── M2: VL805 enumeration + BAR sizing + firmware reset. ──────────────────────────────────────
    let Some(bar0_pcie) = m2_enumerate_vl805() else {
        serial_println!("{} M2 VL805 enumeration failed — USB bring-up skipped (see lines above) ::", P);
        return;
    };

    // ── M3: map the outbound window, attach the shared xHCI to the honesty line. ──────────────────
    m3_attach_xhci(bar0_pcie);

    serial_println!("{} PI-USB-1 bring-up DONE (honesty line; device enumeration is attended metal) ::", P);
}

/// M1: reset the brcmstb bridge, power up the serdes, program the outbound MEM window + inbound DMA BAR,
/// deassert PERST, and poll the link up with a FINITE backstop. Returns whether the RC is alive AND the
/// link came up. An honest link-DOWN (or an absent RC in QEMU) returns false after saying so — never a
/// hang, never a fault.
fn m1_rc_bringup() -> bool {
    // Absent-RC gate FIRST (the sole thing QEMU exercises; it MUST NOT fault). The BCM2711 RC block is
    // in the mapped Device window, so an absent read is open-bus (0 / all-ones), not a translation
    // fault. RGR1_SW_INIT_1 reads a plausible non-poison value on real silicon.
    let swinit = r(RC_BASE + PCIE_RGR1_SW_INIT_1);
    if swinit == 0x0000_0000 || is_poison(swinit) {
        serial_println!(
            "{} M1: RC register block reads {:#010x} @ {:#x} — absent/open-bus (expected in QEMU raspi4b; no PCIe RC modeled) — graceful skip ::",
            P, swinit, RC_BASE + PCIE_RGR1_SW_INIT_1
        );
        return false;
    }
    serial_println!("{} M1: RC alive (RGR1_SW_INIT_1 = {:#010x}) — bridge reset sequence ::", P, swinit);

    // (a) Assert bridge core reset + PERST (put the bridge and the downstream link in reset).
    let mut v = r(RC_BASE + PCIE_RGR1_SW_INIT_1);
    v |= PCIE_RGR1_SW_INIT_1_INIT_GENERIC | PCIE_RGR1_SW_INIT_1_PERST;
    serial_println!("{}   >>> WRITE: RGR1_SW_INIT_1 |= INIT_GENERIC|PERST ({:#010x}) — assert reset ::", P, v);
    w(RC_BASE + PCIE_RGR1_SW_INIT_1, v);
    dsb();
    settle_us(200); // brcmstb: >= 100 us in reset

    // (b) Deassert the bridge core reset (INIT_GENERIC), KEEPING PERST asserted while we configure.
    v = r(RC_BASE + PCIE_RGR1_SW_INIT_1) & !PCIE_RGR1_SW_INIT_1_INIT_GENERIC;
    serial_println!("{}   >>> WRITE: RGR1_SW_INIT_1 &= ~INIT_GENERIC ({:#010x}) — release bridge core ::", P, v);
    w(RC_BASE + PCIE_RGR1_SW_INIT_1, v);
    dsb();
    settle_us(200);

    // (c) Power up the serdes: clear SERDES_IDDQ in HARD_DEBUG.
    v = r(RC_BASE + PCIE_MISC_HARD_PCIE_HARD_DEBUG) & !PCIE_MISC_HARD_DEBUG_SERDES_IDDQ;
    serial_println!("{}   >>> WRITE: HARD_DEBUG &= ~SERDES_IDDQ ({:#010x}) — power up serdes ::", P, v);
    w(RC_BASE + PCIE_MISC_HARD_PCIE_HARD_DEBUG, v);
    dsb();
    settle_ms(1);

    // (d) MISC_CTRL: leave the firmware/reset default (SCB access, 64-bit RC BAR sizing) in place — a
    //     read-modify-touch is unnecessary for our scoped bring-up; log it for the metal record.
    serial_println!("{}   MISC_CTRL = {:#010x} (firmware default retained) ::", P, r(RC_BASE + PCIE_MISC_MISC_CTRL));

    // (e) Inbound DMA BAR (RC_BAR2): map the PCIe inbound window to system RAM base 0 so a bus-master
    //     device can DMA into RAM. Encoding (brcmstb): [LO] = RAM base | size-code in low bits; we set
    //     base 0 with the 4 GiB size code (0x11 in the low field per pcie-brcmstb `encode_ibar_size`).
    //     We stop at the honesty line (no DMA yet), but the inbound BAR is part of the RC sequence of
    //     record, so program + log it for correctness-by-construction.
    const RC_BAR2_SIZE_4G: u32 = 0x11; // brcmstb size code for a 4 GiB inbound window
    serial_println!("{}   >>> WRITE: RC_BAR2 inbound window = RAM@0 size=4GiB (DMA) ::", P);
    w(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_LO, RC_BAR2_SIZE_4G);
    w(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_HI, 0);
    dsb();

    // (f) Outbound MEM window: CPU 0x6_0000_0000 decodes PCIe 0xC000_0000, size 1 GiB. WIN0_LO/HI hold
    //     the CPU-side base; BASE_LIMIT + BASE_HI/LIMIT_HI hold the PCIe-side base..limit (in 1 MiB
    //     units, per brcmstb). Program all four so the RC forwards CPU reads of the window to the fabric.
    let pcie_base = OUTBOUND_PCIE_BASE;
    let pcie_limit = OUTBOUND_PCIE_BASE + OUTBOUND_SIZE - 1;
    serial_println!(
        "{}   >>> WRITE: outbound MEM WIN0 CPU {:#x} -> PCIe [{:#x}, {:#x}] (1 GiB) ::",
        P, OUTBOUND_CPU_BASE, pcie_base, pcie_limit
    );
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LO, (OUTBOUND_CPU_BASE & 0xFFFF_FFFF) as u32);
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_HI, (OUTBOUND_CPU_BASE >> 32) as u32);
    // BASE_LIMIT: base[31:20] in bits [15:4], limit[31:20] in bits [31:20] (brcmstb 1 MiB granularity).
    let base_mb = ((pcie_base >> 20) & 0xFFF) as u32;
    let limit_mb = ((pcie_limit >> 20) & 0xFFF) as u32;
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT, (limit_mb << 20) | (base_mb << 4));
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI, (pcie_base >> 32) as u32);
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI, (pcie_limit >> 32) as u32);
    dsb();

    // (g) Deassert PERST — release the downstream link to train.
    v = r(RC_BASE + PCIE_RGR1_SW_INIT_1) & !PCIE_RGR1_SW_INIT_1_PERST;
    serial_println!("{}   >>> WRITE: RGR1_SW_INIT_1 &= ~PERST ({:#010x}) — release link, training ::", P, v);
    w(RC_BASE + PCIE_RGR1_SW_INIT_1, v);
    dsb();

    // (h) Poll link-up (PHYLINKUP + DL_ACTIVE) with a finite ~100 ms backstop (brcmstb allows up to
    //     ~100 ms for link training). An honest DOWN after the budget is a real result, not a hang.
    let up_mask = PCIE_MISC_PCIE_STATUS_PHYLINKUP | PCIE_MISC_PCIE_STATUS_DL_ACTIVE;
    let deadline = super::timer::cntpct() + super::timer::cntfrq() / 10; // ~100 ms
    let mut status = 0u32;
    let mut up = false;
    while super::timer::cntpct() < deadline {
        status = r(RC_BASE + PCIE_MISC_PCIE_STATUS);
        if !is_poison(status) && (status & up_mask) == up_mask {
            up = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !up {
        serial_println!(
            "{} M1: link DOWN after training budget (PCIE_STATUS = {:#010x}; PHYLINKUP={} DL_ACTIVE={}) — honest hardware result, no device below the RC ::",
            P, status,
            status & PCIE_MISC_PCIE_STATUS_PHYLINKUP != 0,
            status & PCIE_MISC_PCIE_STATUS_DL_ACTIVE != 0
        );
        return false;
    }
    serial_println!("{} M1: LINK UP (PCIE_STATUS = {:#010x}) ::", P, status);

    // (i) Read the root-port identity (bus 0, dev 0) directly from the RC config space — expect a
    //     Broadcom vendor id (0x14e4). Poison-rejecting.
    let rp = r(RC_BASE + 0x00);
    match live_vendor_device(rp) {
        Some((ven, dev)) => serial_println!("{}   root port: vendor={:#06x} device={:#06x} ::", P, ven, dev),
        None => serial_println!("{}   root port identity ABSENT DECODE ({:#010x}) — proceeding to child config ::", P, rp),
    }
    let _ = PCIE_RC_CFG_VENDOR_SPECIFIC; // documented landmark; not read on the scoped path
    true
}

/// M2: enumerate the VL805 through the EXT_CFG child window, verify its identity (poison-rejecting),
/// run the standard BAR-sizing ritual on BAR0 (restore-immediate), assign BAR0 to the outbound window's
/// PCIe base, enable MEM decode + bus-master on the VL805, and issue the NOTIFY_XHCI_RESET mailbox so
/// the VideoCore firmware (re)loads the VL805 firmware. Returns the PCIe-side BAR0 base on success.
fn m2_enumerate_vl805() -> Option<u64> {
    let idword = vl805_cfg_read(0x00);
    serial_println!("{} M2: VL805 config[0x00] = {:#010x} ::", P, idword);
    let (vendor, device) = live_vendor_device(idword)?;
    if vendor != VL805_VENDOR || device != VL805_DEVICE {
        serial_println!(
            "{}   device below RP is {:#06x}:{:#06x}, NOT the expected VL805 {:#06x}:{:#06x} — bring-up STOP (unexpected topology) ::",
            P, vendor, device, VL805_VENDOR, VL805_DEVICE
        );
        return None;
    }
    serial_println!("{}   VL805 FOUND: vendor={:#06x} device={:#06x} (VIA VL805 xHCI) ::", P, vendor, device);

    let classrev = vl805_cfg_read(0x08);
    if !is_poison(classrev) {
        serial_println!(
            "{}   class={:#04x} subclass={:#04x} progif={:#04x} rev={:#04x} (expect 0c/03/30 = USB xHCI) ::",
            P, (classrev >> 24) & 0xff, (classrev >> 16) & 0xff, (classrev >> 8) & 0xff, classrev & 0xff
        );
    }

    // ── BAR0 sizing ritual (all-ones probe + immediate restore; the ORIN-NET-3 pattern) ──
    let orig = vl805_cfg_read(0x10);
    serial_println!("{}   >>> WRITE: BAR0[0x10] all-ones probe (orig={:#010x}) — write 0xffffffff, read size, RESTORE ::", P, orig);
    vl805_cfg_write(0x10, 0xFFFF_FFFF);
    let readback = vl805_cfg_read(0x10);
    vl805_cfg_write(0x10, orig); // restore IMMEDIATELY
    if readback == 0 || is_poison(readback) {
        serial_println!("{}   BAR0 unimplemented/absent (readback {:#010x}) — cannot attach ::", P, readback);
        return None;
    }
    let is_io = orig & 1 == 1;
    let mem_type = (orig >> 1) & 0x3; // 0 = 32-bit, 2 = 64-bit
    let mask = readback & !0xf;
    let size = ((!(mask as u64)) & 0xFFFF_FFFF).wrapping_add(1);
    serial_println!(
        "{}   BAR0 = {} mem, 64bit={}, size={:#x} ::",
        P, if is_io { "I/O(!)" } else { "MMIO" }, mem_type == 0x2, size
    );

    // ── Assign BAR0 to the outbound window's PCIe base and enable decode. The VL805's xHCI MMIO will
    //    then be decoded at PCIe 0xC000_0000, which the CPU reaches at OUTBOUND_CPU_BASE (mapped in M3). ──
    let bar0_pcie = OUTBOUND_PCIE_BASE;
    serial_println!("{}   >>> WRITE: BAR0 := {:#x} (PCIe-side; CPU sees it at {:#x}) ::", P, bar0_pcie, OUTBOUND_CPU_BASE);
    vl805_cfg_write(0x10, (bar0_pcie & 0xFFFF_FFF0) as u32 | (orig & 0xf));
    if mem_type == 0x2 {
        vl805_cfg_write(0x14, (bar0_pcie >> 32) as u32); // 64-bit BAR high half
    }
    // Command register (0x04): set MEM Space Enable (bit1) + Bus Master Enable (bit2).
    let cmd = vl805_cfg_read(0x04);
    let newcmd = (cmd & 0xFFFF_0000) | ((cmd & 0xFFFF) | 0b110);
    serial_println!("{}   >>> WRITE: VL805 COMMAND {:#06x} -> {:#06x} (MEM+BusMaster enable) ::", P, cmd & 0xffff, newcmd & 0xffff);
    vl805_cfg_write(0x04, newcmd);

    // ── NOTIFY_XHCI_RESET: have the VideoCore firmware (re)load + reset the VL805 firmware. Normally
    //    the RPi bootloader does this at power-on from the SPI EEPROM; an OS bringing the controller up
    //    itself re-issues it so the xHCI comes up in a known state before attach. ──
    serial_println!("{}   NOTIFY_XHCI_RESET (mailbox 0x00030058, dev_addr={:#x}) — firmware VL805 reset/load ::", P, VL805_DEV_ADDR);
    let ok = mailbox::notify_xhci_reset(VL805_DEV_ADDR);
    serial_println!("{}   NOTIFY_XHCI_RESET reported {} ::", P, if ok { "SUCCESS" } else { "FAILURE (firmware may already have loaded it at boot)" });
    // A NOTIFY failure is not fatal to the honesty line — the bootloader normally loaded the firmware
    // already; log and proceed.

    Some(bar0_pcie)
}

/// M3: map the outbound window Device-nGnRnE, read the xHCI capability registers at the VL805 BAR
/// (poison-rejecting), attach the shared xHCI driver in polled mode (halt + reset = "halted-but-
/// decoding"), power the root ports, and STOP. Full device enumeration (rings, ADDRESS_DEVICE, HID/
/// storage) needs the heap + a live device and is the attended-metal follow-on — not this arc.
fn m3_attach_xhci(_bar0_pcie: u64) {
    // Map the 1 GiB Device block that contains OUTBOUND_CPU_BASE into the live translation regime (the
    // ONLY new page-table write this arc makes; `boot::map_device_1gib`, piusb-gated). The RC forwards
    // CPU reads of this window to the VL805's BAR (programmed in M2).
    unsafe { super::boot::map_device_1gib(OUTBOUND_CPU_BASE) };
    serial_println!("{} M3: mapped outbound window CPU {:#x} (Device-nGnRnE, 1 GiB) — reading VL805 xHCI caps ::", P, OUTBOUND_CPU_BASE);

    let cap0 = r(OUTBOUND_CPU_BASE);
    if is_poison(cap0) || cap0 == 0 {
        serial_println!(
            "{}   xHCI CAP register reads {:#010x} @ {:#x} — BAR not decoding (window/BAR mismatch or firmware not loaded); attach SKIPPED, fail-closed ::",
            P, cap0, OUTBOUND_CPU_BASE
        );
        return;
    }
    let cap_length = (cap0 & 0xff) as u8;
    let hci_version = (cap0 >> 16) as u16;
    let hcsparams1 = r(OUTBOUND_CPU_BASE + 0x04);
    let max_ports = ((hcsparams1 >> 24) & 0xff) as u8;
    serial_println!(
        "{}   xHCI DECODING: CAPLENGTH={} HCIVERSION={:#06x} HCSPARAMS1={:#010x} MaxPorts={} ::",
        P, cap_length, hci_version, hcsparams1, max_ports
    );

    // Attach the shared xHCI driver in polled mode: halt + HCRST + CNR wait (heap-free; no ring
    // allocation — that is the enumerating attach, deferred to metal). After this the controller is
    // HALTED-BUT-DECODING: its registers answer, it is reset to a clean state, ready for the metal
    // enumeration arc to program rings and pump ports.
    serial_println!("{}   attaching shared xHCI driver @ {:#x} (polled, halt+reset) ::", P, OUTBOUND_CPU_BASE);
    xhci::init(OUTBOUND_CPU_BASE);

    // Power the root ports: set PORTSC.PP (bit 9) on each port register (operational base +0x400 +
    // 0x10*port). A powered port can detect a device connect; the metal arc pumps the connect events.
    let op_base = OUTBOUND_CPU_BASE + cap_length as u64;
    let ports = max_ports.max(1).min(16); // clamp: a plausible VL805 has a small port count
    for port in 0..ports {
        let portsc_addr = op_base + 0x400 + (port as u64) * 0x10;
        let portsc = r(portsc_addr);
        if is_poison(portsc) {
            continue;
        }
        // Preserve everything except the RW1C change bits (do not clear/ack them here) and OR in PP.
        // Mask off the change/RW1C bits so we don't accidentally clear a latched change: PP is bit 9.
        const PORTSC_PP: u32 = 1 << 9;
        const PORTSC_RW1C: u32 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);
        w(portsc_addr, (portsc & !PORTSC_RW1C) | PORTSC_PP);
    }
    dsb();
    settle_ms(20); // let port power stabilize before the honesty read
    serial_println!("{} M3: {} root port(s) powered (PORTSC.PP set); controller halted-but-decoding — HONESTY LINE reached ::", P, ports);
    serial_println!(
        "{}   NEXT (attended metal): program rings + interrupter (needs heap), pump port connects, ADDRESS_DEVICE, HID/storage enumeration ::",
        P
    );
}
