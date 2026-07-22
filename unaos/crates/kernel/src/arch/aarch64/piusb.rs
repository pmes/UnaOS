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
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Stable serial prefix so the operator (and `mbench`) can grep the whole bring-up as one block.
const P: &str = ":: PIUSB:";

// ─── PI-USB-2 handoff: the honesty line (`bringup`, pre-heap in `build_boot_info`) reaches "VL805
// xHCI halted-but-decoding + ports powered"; the DMA-side enumeration (`enumerate`, post-heap in
// kernel_main — rings/DCBAA/interrupter need the heap) picks up from there. These two statics carry
// the CPU-side xHCI base + a ready flag across that gap. Single-writer (the BSP, pre-SMP) so plain
// atomics suffice; knob-off the whole module (and both statics) vanish. ───
static XHCI_CPU_BASE: AtomicU64 = AtomicU64::new(0);
static XHCI_READY: AtomicBool = AtomicBool::new(false);

// ─── PIUSB-6: the adopt-don't-reset path is RETIRED. boot-P4 proved on metal that the VideoCore tears
// PCIe down before handoff (`PHYLINKUP=false DL_ACTIVE=false`, all windows zero, RC fully in reset), so
// there is never a firmware-left working decode to adopt on this platform — the firmware's config exists
// only during firmware runtime. The pre-reset dump is KEPT as a one-boot-proven read-only record (now
// hard-gated on link-up: a link-down RC must never be MMIO'd past its own register block), but the adopt
// branch, its `UNAOS_PIUSB_ADOPT` knob, and the WIN0 CPU-base decode are gone with it. ───

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
// MISC_CTRL SCB0 inbound-window SIZE field (bits [31:27]; SCB1 at [26:22], SCB2 at [21:17]). The RC will
// not claim an inbound region larger than its programmed SCB size — SCB0_SIZE must be sized to match
// RC_BAR2 BEFORE (or with) the RC_BAR2 write (documented BCM2711 order; pcie-brcmstb.c is GPL-2.0-only).
const PCIE_MISC_MISC_CTRL_SCB0_SIZE_SHIFT: u32 = 27;
const PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK: u32 = 0x1f << 27; // 0xf800_0000

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

/// Open-bus / firmware-fill poison signatures — NONE is ever live data (PI-V3D-1 false-PASS rule).
/// `0xdeaddead` joined the list after R22 sitting 2: it is the BCM2711 RC's abort-fill for a
/// downstream config read it could not forward (observed as `config[0x00] = 0xdeaddead` with the
/// link UP), and the old two-pattern gate FALSE-LIVED it as vendor 0xdead device 0xdead.
#[inline]
fn is_poison(v: u32) -> bool {
    v == 0xFFFF_FFFF || v == 0xDEAD_BEEF || v == 0xDEAD_DEAD
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

// ─── PIUSB-5/6: the pre-reset firmware reference dump (link-up-gated). ──────────────────────────────
//
// PIUSB-5 captured the firmware-left state before M1's reset destroys it. boot-P4 delivered the verdict:
// the firmware leaves the RC FULLY IN RESET — `RGR1_SW_INIT_1=0x3 PCIE_STATUS=0x0 (PHYLINKUP=false
// DL_ACTIVE=false)`, WIN0 all-zero, RC_BAR2 zero, no live child config. The VideoCore tears PCIe down
// before handoff; its working config exists only during firmware runtime. So there is NOTHING to adopt,
// and the adopt path (with its knob and the WIN0 CPU-base decode) is RETIRED in PIUSB-6.
//
// boot-P4 also exposed the MMIO HAZARD this dump now guards against: with the link down, the original
// dump proceeded to CAP-read the outbound window anyway — an MMIO into a link-down RC does NOT fault, it
// STALLS the CPU on the bus for a pathologically long time (the run looked frozen; a power-cycled rerun
// eventually returned 0x0). So the dump is now HARD-GATED on PHYLINKUP && DL_ACTIVE: it reads only the
// RC's own register block (always safe), and if the link is down it witnesses the RC's CPU-side claim
// regs (MISC_CTRL scb-size fields) against the documented BCM2711 order and returns — NO downstream
// MMIO. That witness is the boot-P5 evidence on the still-poison path. Nothing here writes a register.
//
// Breadcrumb for the doc: a pre-bring-up read at 0x6_0000_0000 returns 0x00000000 (the RC does not yet
// CLAIM that CPU address — unclaimed), whereas a post-bring-up read returns 0xdeaddead (the RC claims it
// but master-aborts forwarding it downstream). The unclaimed-vs-master-abort difference localises the
// wall: pre-reset it is the RC's address claim; post-reset it is downstream of the RC (the BAR).

/// Outbound-window packing constants (M1 step (f)): CPU base/limit are expressed in 1 MiB units, and the
/// upper MiB bits spill from the 12-bit BASE_LIMIT field into the separate BASE_HI/LIMIT_HI registers.
const WIN_MB_SHIFT: u64 = 20; // 1 MiB granularity
const WIN_HI_SHIFT: u32 = 12; // bits carried by the 12-bit base/limit field in BASE_LIMIT

/// Witness the RC's CPU-side claim registers against the documented BCM2711 bring-up order. Called only
/// when even the firmware-left state reads poison — at that point the offender is upstream of the BAR,
/// in the RC's own address-claim/scb sizing. Facts-only decode (pcie-brcmstb.c is GPL-2.0-only): in
/// PCIE_MISC_MISC_CTRL the three SCB (system-cache-bus) window SIZE fields sit in the top nibbles —
/// SCB0_SIZE at bits[31:27], SCB1_SIZE at [26:22], SCB2_SIZE at [21:17]; the RC will not claim an
/// inbound region larger than its programmed SCB size, and a zero SCB0_SIZE with a 4 GiB RC_BAR2 is a
/// documented bring-up-order fault (RC_BAR2 must agree with SCB0_SIZE). We only READ + report.
fn witness_rc_cpu_claim() {
    let misc = r(RC_BASE + PCIE_MISC_MISC_CTRL);
    let bar2_lo = r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_LO);
    let bar2_hi = r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_HI);
    if is_poison(misc) {
        serial_println!("{}   RC-claim witness: MISC_CTRL reads poison {:#010x} — RC register block itself not answering ::", P, misc);
        return;
    }
    let scb0 = (misc >> 27) & 0x1f;
    let scb1 = (misc >> 22) & 0x1f;
    let scb2 = (misc >> 17) & 0x1f;
    serial_println!(
        "{}   RC-claim witness: MISC_CTRL={:#010x} SCB0_SIZE={:#x} SCB1_SIZE={:#x} SCB2_SIZE={:#x} | RC_BAR2 LO={:#010x} HI={:#010x} ::",
        P, misc, scb0, scb1, scb2, bar2_lo, bar2_hi
    );
    // The documented order: SCB0_SIZE must be set (nonzero) before RC_BAR2 is programmed to a matching
    // inbound size; a nonzero RC_BAR2 size-code over a zero SCB0 is the classic misordering. This witness
    // runs on the PRE-RESET path (reading the state the FIRMWARE left) — so a zero SCB0 here reports the
    // FIRMWARE's teardown, not our programming. PIUSB-6's M1 now sizes SCB0 before RC_BAR2 (step (d));
    // that fix is proven by M1's own MISC_CTRL readback line, not by this pre-reset witness.
    let bar2_size_code = bar2_lo & 0x1f;
    if scb0 == 0 && bar2_size_code != 0 {
        serial_println!(
            "{}   RC-claim witness: SCB0_SIZE=0 but RC_BAR2 size-code={:#x} — inbound window sized AHEAD of its SCB window in the FIRMWARE-left state (RC won't claim it); PIUSB-6 M1 corrects this order on our own bring-up ::",
            P, bar2_size_code
        );
    }
}

/// The pre-reset firmware reference dump. Read-only. Runs after the DTB census (so only when an RC is
/// present) and before M1's first reset write. Reads the RC's OWN register block (bridge/window/status —
/// always safe, even with the link down) and prints it as the one-boot-proven record. Then a HARD
/// LINK-UP GATE: only if PCIE_STATUS shows PHYLINKUP && DL_ACTIVE does it go on to probe the VL805 child
/// config + the outbound CAP window; otherwise it witnesses the RC's CPU-side claim registers and
/// returns WITHOUT any downstream MMIO. boot-P4 proved why: the firmware leaves the RC fully in reset
/// (PHYLINKUP=false DL_ACTIVE=false, all windows zero), and an MMIO probe into a link-down RC does not
/// fault — it STALLS the CPU on the bus for a pathologically long time (the run appeared frozen; a
/// power-cycled rerun eventually returned 0). Adopt is retired: there is nothing live to adopt here.
fn dump_firmware_state() {
    serial_println!("{} PIUSB-5: pre-reset firmware reference dump (read-only; BEFORE any RC reset) ::", P);

    // (1) RC bridge + window registers, exactly as the firmware left them. These are the RC's OWN
    //     register block (the 0xFD50_0000 MMIO the CPU always claims) — safe to read link-down.
    let swinit = r(RC_BASE + PCIE_RGR1_SW_INIT_1);
    let status = r(RC_BASE + PCIE_MISC_PCIE_STATUS);
    let buses = r(RC_BASE + 0x18);
    let win_lo = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LO);
    let win_hi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_HI);
    let win_bl = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT);
    let win_bhi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI);
    let win_lhi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI);
    let bar2_lo = r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_LO);
    let bar2_hi = r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_HI);
    let phylinkup = status & PCIE_MISC_PCIE_STATUS_PHYLINKUP != 0;
    let dl_active = status & PCIE_MISC_PCIE_STATUS_DL_ACTIVE != 0;
    serial_println!(
        "{}   fw RC: RGR1_SW_INIT_1={:#010x} PCIE_STATUS={:#010x} (PHYLINKUP={} DL_ACTIVE={}) bus_window[0x18]={:#010x} ::",
        P, swinit, status, phylinkup, dl_active, buses
    );
    serial_println!(
        "{}   fw WIN0: LO={:#010x} HI={:#010x} BASE_LIMIT={:#010x} BASE_HI={:#010x} LIMIT_HI={:#010x} | RC_BAR2 LO={:#010x} HI={:#010x} ::",
        P, win_lo, win_hi, win_bl, win_bhi, win_lhi, bar2_lo, bar2_hi
    );

    // (2) LINK-DOWN GATE (the boot-P4 lesson, now law). PHYLINKUP && DL_ACTIVE are BOTH required before
    //     ANY probe past the RC register block — the child config goes through EXT_CFG forwarding to the
    //     (absent) downstream device, and the CAP read is a memory cycle into the outbound window; both
    //     stall the bus for a very long time when the link is down. If down, witness the RC's own
    //     claim/scb-size registers (safe, RC-register reads) and return — that witness is boot-P5's
    //     evidence line on the still-poison path.
    if is_poison(status) || !(phylinkup && dl_active) {
        serial_println!(
            "{}   fw left RC in reset — nothing to adopt/probe (PHYLINKUP={} DL_ACTIVE={}); skipping child-config + CAP probe (MMIO into a link-down RC stalls the bus — boot-P4 stalled here) ::",
            P, phylinkup, dl_active
        );
        witness_rc_cpu_claim();
        // Drain any latent async abort the RC-register reads could have set (the R22 sitting-2 class);
        // this dump runs BEFORE M1's reset, so nothing latent may survive into the reset sequence.
        super::exceptions::serror_drain_request("piusb: PIUSB-5 dump link-down");
        return;
    }
    serial_println!(
        "{}   fw RC link UP (PHYLINKUP && DL_ACTIVE) — firmware left the RC live; probing child config + CAP (read-only) ::",
        P
    );

    // (3) VL805 config, through whatever EXT_CFG forwarding the firmware left (link-up only; read-only).
    let id = vl805_cfg_read(0x00);
    let cmd = vl805_cfg_read(0x04);
    let bar0_lo = vl805_cfg_read(0x10);
    let bar0_hi = vl805_cfg_read(0x14);
    match live_vendor_device(id) {
        Some((v, d)) => serial_println!(
            "{}   fw VL805 cfg: id={:#010x} (vendor={:#06x} device={:#06x}) COMMAND={:#06x} (MEM={} BM={}) BAR0=[{:#010x} {:#010x}] ::",
            P, id, v, d, cmd & 0xffff, (cmd >> 1) & 1, (cmd >> 2) & 1, bar0_lo, bar0_hi
        ),
        None => serial_println!(
            "{}   fw VL805 cfg: id={:#010x} — no live child config (firmware left no EXT_CFG bus-window forwarding, or device silent) ::",
            P, id
        ),
    }

    // (4) CAP read at the canonical outbound window (link-up only). Read-only record: whether the
    //     firmware-left decode answers is the diff evidence, but with adopt retired we never ride it.
    let cap_cpu_base = OUTBOUND_CPU_BASE;
    unsafe { super::boot::map_device_1gib(cap_cpu_base) };
    let mut cap0 = r(cap_cpu_base);
    let mut tries = 1u32;
    while (is_poison(cap0) || cap0 == 0) && tries < 4 {
        settle_ms(2);
        cap0 = r(cap_cpu_base);
        tries += 1;
    }
    if is_poison(cap0) || cap0 == 0 {
        serial_println!(
            "{}   fw CAP probe @ {:#x} = {:#010x} after {} tries — the firmware-left state does not decode memory cycles here; the wall is upstream of the BAR ::",
            P, cap_cpu_base, cap0, tries
        );
        witness_rc_cpu_claim();
        super::exceptions::serror_drain_request("piusb: PIUSB-5 dump poison");
        return;
    }
    let cap_length = (cap0 & 0xff) as u8;
    let hci_version = (cap0 >> 16) as u16;
    let hcsparams1 = r(cap_cpu_base + 0x04);
    serial_println!(
        "{}   fw CAP probe @ {:#x} = {:#010x} LIVE — firmware-left xHC IS DECODING: CAPLENGTH={} HCIVERSION={:#06x} HCSPARAMS1={:#010x} (record only — adopt retired, we still reset) ::",
        P, cap_cpu_base, cap0, cap_length, hci_version, hcsparams1
    );
    super::exceptions::serror_drain_request("piusb: PIUSB-5 dump exit");
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

    // ── PIUSB-5/6: read-only reference dump FIRST (link-up-gated), reset SECOND. Capture the
    //    firmware-left state before M1 destroys it with the bridge/PERST reset. boot-P4 proved the
    //    firmware tears PCIe down before handoff (RC left fully in reset, no child config, no live
    //    decode), so the adopt path is retired — the dump is a record; we always take the reset path. ──
    dump_firmware_state();

    // ── M1: brcmstb RC bring-up. ─────────────────────────────────────────────────────────────────
    if !m1_rc_bringup() {
        serial_println!("{} M1 RC bring-up did not reach link-up — USB bring-up skipped (see lines above) ::", P);
        // SError-drain class rule (R22 sitting 2): the RC accesses above may have left a latent
        // async abort pending; a fail-closed exit must leave the machine clean.
        super::exceptions::serror_drain_request("piusb: M1 fail-closed");
        return;
    }

    // ── M2: VL805 enumeration + BAR sizing + firmware reset. ──────────────────────────────────────
    let Some(bar0_pcie) = m2_enumerate_vl805() else {
        serial_println!("{} M2 VL805 enumeration failed — USB bring-up skipped (see lines above) ::", P);
        // The poisoned downstream config read (`0xdeaddead`) is the R22 sitting-2 metal offender:
        // it left a pending async external abort that detonated at the first DAIF unmask. Drain.
        super::exceptions::serror_drain_request("piusb: M2 fail-closed (poisoned cfg read)");
        return;
    };

    // ── M3: map the outbound window, attach the shared xHCI to the honesty line. ──────────────────
    m3_attach_xhci(OUTBOUND_CPU_BASE, bar0_pcie);

    // Belt-and-suspenders: no latent async abort from any RC/BAR access may outlive the bring-up
    // (M3's fail-closed CAP-poison branch included — it returns into this drain).
    super::exceptions::serror_drain_request("piusb: bring-up exit");

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

    // (d) MISC_CTRL SCB0_SIZE: size the RC's inbound (system-cache-bus) window BEFORE programming
    //     RC_BAR2 — the documented BCM2711 bring-up order.
    //
    //     ── PIUSB-6 ADJUDICATION (the SCB/RC_BAR2 ordering audit — now THE live wall hypothesis) ─────
    //     PIUSB-5's `witness_rc_cpu_claim` flagged our prior code: it wrote RC_BAR2 (step (e)) with
    //     SCB0_SIZE left at the firmware/reset default, i.e. it programmed the inbound window AHEAD of
    //     sizing the SCB window that BACKS it. The RC will not claim an inbound region larger than its
    //     programmed SCB size, so a 4 GiB RC_BAR2 over an unsized (or mismatched) SCB0 is the documented
    //     ordering fault. `pcie-brcmstb.c` (GPL-2.0-only — facts only) programs the SCB0_SIZE field of
    //     MISC_CTRL to the inbound size in the SAME setup step, before/with the RC_BAR2 write. SCB0_SIZE
    //     uses the SAME encode as the ibar size-code (log2(size)-15 for 64 KiB..32 GiB), so a 4 GiB
    //     inbound window is 0x11 in BOTH fields. We set SCB0_SIZE here, read it back, THEN write
    //     RC_BAR2 (step (e)) — matching order restored. Read-modify-write: only the SCB0_SIZE field is
    //     touched; every other MISC_CTRL bit (SCB access enable, RC BAR sizing) is preserved.
    const RC_BAR2_SIZE_4G: u32 = 0x11; // encode_ibar_size(4 GiB) = log2(2^32) - 15 = 0x11 (shared: SCB0 + RC_BAR2)
    let misc_before = r(RC_BASE + PCIE_MISC_MISC_CTRL);
    let misc_after = (misc_before & !PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK)
        | ((RC_BAR2_SIZE_4G << PCIE_MISC_MISC_CTRL_SCB0_SIZE_SHIFT) & PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK);
    serial_println!(
        "{}   >>> WRITE: MISC_CTRL {:#010x} -> {:#010x} (SCB0_SIZE := {:#x} = 4 GiB, sized BEFORE RC_BAR2 — documented BCM2711 order; PIUSB-6 fix) ::",
        P, misc_before, misc_after, RC_BAR2_SIZE_4G
    );
    w(RC_BASE + PCIE_MISC_MISC_CTRL, misc_after);
    dsb();
    let misc_rb = r(RC_BASE + PCIE_MISC_MISC_CTRL);
    serial_println!(
        "{}   MISC_CTRL readback = {:#010x} (SCB0_SIZE={:#x}) -> {} ::",
        P, misc_rb, (misc_rb >> PCIE_MISC_MISC_CTRL_SCB0_SIZE_SHIFT) & 0x1f,
        if (misc_rb & PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK) == (misc_after & PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK) { "SIZED" } else { "DID NOT LATCH" }
    );

    // (e) Inbound DMA BAR (RC_BAR2): map the PCIe inbound window to system RAM base 0 so a bus-master
    //     device can DMA into RAM. Encoding (brcmstb): CONFIG_LO = (RAM base low 32b) | size-code in
    //     bits [4:0] (PCIE_MISC_RC_BAR2_CONFIG_LO_SIZE_MASK = 0x1f); CONFIG_HI = RAM base high 32b.
    //
    //     ── PI-USB-2 M1 ADJUDICATION (encode_ibar_size, the mandatory gate) ─────────────────────────
    //     The size-code is NOT log2(size). Linux `drivers/pci/controller/pcie-brcmstb.c`
    //     `brcm_pcie_encode_ibar_size(u64 size)` maps a byte size to the 5-bit field by branch on
    //     `ilog2(size)`:
    //         * log2 in [12,15]  (4 KiB .. 32 KiB)   -> (log2 - 12) + 0x1c
    //         * log2 in [16,35]  (64 KiB .. 32 GiB)  -> log2 - 15
    //         * otherwise                             -> 0 (disabled)
    //     A 4 GiB inbound window is size = 2^32, so log2 = 32, which lands in the [16,35] branch:
    //         code = 32 - 15 = 17 = 0x11.
    //     So 0x11 is CORRECT for 4 GiB; the alternative 0x20 (= 32 decimal, the raw log2) would be the
    //     32 EiB / out-of-range value and is WRONG. The lens caveat (0x11-vs-0x20) is hereby resolved to
    //     0x11 by the encode_ibar_size programming model — constant unchanged.
    //
    //     We stop at the honesty line (no DMA yet), but the inbound BAR is part of the RC sequence of
    //     record, so program + log it for correctness-by-construction. `RC_BAR2_SIZE_4G` is the shared
    //     size-code declared in step (d) (SCB0_SIZE and RC_BAR2 must agree — same 0x11 for 4 GiB).
    serial_println!("{}   >>> WRITE: RC_BAR2 inbound window = RAM@0 size=4GiB (DMA; SCB0_SIZE already set in (d)) ::", P);
    w(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_LO, RC_BAR2_SIZE_4G);
    w(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_HI, 0);
    dsb();
    serial_println!(
        "{}   RC_BAR2 readback: CONFIG_LO={:#010x} CONFIG_HI={:#010x} (size-code {:#x} = 4 GiB inbound) ::",
        P, r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_LO), r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_HI), RC_BAR2_SIZE_4G
    );

    // (f) Outbound MEM window: CPU 0x6_0000_0000 decodes PCIe 0xC000_0000, size 1 GiB.
    //
    //     ── PIUSB-3 ADJUDICATION (the boot-P1 0xdeaddead root cause) ─────────────────────────────────
    //     boot-P1 metal reached the honesty line's LAST step and read 0xdeaddead at CPU 0x6_0000_0000 —
    //     the RC never CLAIMED that CPU address, so the outbound window was misprogrammed. Audited against
    //     the BCM2711 brcmstb register model (`brcm_pcie_set_outbound_win`, facts only — pcie-brcmstb.c
    //     is GPL-2.0-only): the two register groups had their address spaces SWAPPED.
    //
    //       * WIN0_LO / WIN0_HI  hold the **PCIe-side** address the window translates TO (`pcie_addr`).
    //       * BASE_LIMIT + BASE_HI + LIMIT_HI hold the **CPU-side** address range the RC MATCHES against
    //         (`cpu_addr .. cpu_addr+size-1`), in 1 MiB units.
    //
    //     The prior code wrote the CPU base (0x6_0000_0000) into WIN0_LO/HI and the PCIe range
    //     (0xC000_0000..0xFFFF_FFFF) into BASE_LIMIT/BASE_HI/LIMIT_HI — inverted. The RC therefore matched
    //     CPU addresses in [0xC000_0000, 0xFFFF_FFFF] (which the CPU never issues for this BAR) and left
    //     0x6_0000_0000 (= 0x6000 MiB, bit 34 set) unclaimed → master-abort fill 0xdeaddead. Corrected
    //     below: PCIe addr -> WIN0_LO/HI; CPU addr range -> BASE_LIMIT/BASE_HI/LIMIT_HI.
    //
    //     Field layout (BCM2711): BASE_LIMIT packs base_mb[11:0] in bits [15:4] and limit_mb[11:0] in bits
    //     [31:20]; the upper MiB bits (a 12 MiB address needs >12 bits — 0x6000 MiB does) spill into the
    //     separate BASE_HI / LIMIT_HI registers, shifted right by 12 (HWEIGHT of the 12-bit base field).
    //     Every register is READ BACK and witnessed (the ORIN readback ritual) so a boot log proves the
    //     window is armed, not merely written.
    // WIN_MB_SHIFT / WIN_HI_SHIFT are the module-level packing constants.
    let cpu_base = OUTBOUND_CPU_BASE;
    let cpu_limit = OUTBOUND_CPU_BASE + OUTBOUND_SIZE - 1;
    let pcie_base = OUTBOUND_PCIE_BASE;
    let cpu_base_mb = cpu_base >> WIN_MB_SHIFT;
    let cpu_limit_mb = cpu_limit >> WIN_MB_SHIFT;
    serial_println!(
        "{}   >>> WRITE: outbound MEM WIN0 CPU [{:#x}, {:#x}] -> PCIe {:#x} (1 GiB) ::",
        P, cpu_base, cpu_limit, pcie_base
    );
    // WIN0_LO/HI = PCIe-side base (what the window translates the matched CPU address TO).
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LO, (pcie_base & 0xFFFF_FFFF) as u32);
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_HI, (pcie_base >> 32) as u32);
    // BASE_LIMIT = CPU-side base/limit low 12 MiB-bits: base in [15:4], limit in [31:20].
    let base_lo = (cpu_base_mb & 0xFFF) as u32;
    let limit_lo = (cpu_limit_mb & 0xFFF) as u32;
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT, (limit_lo << 20) | (base_lo << 4));
    // BASE_HI/LIMIT_HI = CPU-side base/limit high MiB-bits (0x6000 MiB >> 12 = 0x6 — carries bit 34).
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI, (cpu_base_mb >> WIN_HI_SHIFT) as u32);
    w(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI, (cpu_limit_mb >> WIN_HI_SHIFT) as u32);
    dsb();

    // Readback witnesses: every window register, so the boot log proves what the RC actually latched.
    // A LO/HI that reads back the CPU base, or a BASE_LIMIT that reads back the PCIe range, would flag a
    // regression to the pre-PIUSB-3 inverted programming.
    let rb_lo = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LO);
    let rb_hi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_HI);
    let rb_bl = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT);
    let rb_bhi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI);
    let rb_lhi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI);
    serial_println!(
        "{}   WIN0 readback: LO={:#010x} HI={:#010x} (PCIe base) | BASE_LIMIT={:#010x} BASE_HI={:#010x} LIMIT_HI={:#010x} (CPU range) ::",
        P, rb_lo, rb_hi, rb_bl, rb_bhi, rb_lhi
    );
    let win_expected = rb_lo == (pcie_base & 0xFFFF_FFFF) as u32
        && rb_hi == (pcie_base >> 32) as u32
        && rb_bl == ((limit_lo << 20) | (base_lo << 4))
        && rb_bhi == (cpu_base_mb >> WIN_HI_SHIFT) as u32
        && rb_lhi == (cpu_limit_mb >> WIN_HI_SHIFT) as u32;
    serial_println!(
        "{}   WIN0 armed: {} (RC now claims CPU {:#x}..{:#x}, translates to PCIe {:#x}) ::",
        P, if win_expected { "YES" } else { "NO — registers did not latch as programmed" },
        cpu_base, cpu_limit, pcie_base
    );

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
    // ── PI-USB-2 R22 sitting-2 fix: the RC never had its ROOT-PORT BRIDGE BUS WINDOW programmed.
    // The RP is a type-1 (bridge) function; its PRIMARY/SECONDARY/SUBORDINATE bus registers
    // (config offset 0x18, bytes 0/1/2) reset to 0/0/0, and a bridge whose secondary window does
    // not cover bus 1 does NOT forward type-1 config requests downstream — the EXT_CFG access to
    // bus1:dev0 master-aborts inside the RC and the DATA window returns the abort fill
    // (`0xdeaddead`, the exact word metal captured with the link UP). Linux never hits this in the
    // driver because the PCI core's pci_scan_bridge programs the window during enumeration; we ARE
    // the enumerator, so program primary=0 / secondary=1 / subordinate=1 here, before the first
    // downstream config read. (RP config lives directly at RC_BASE — no EXT_CFG indirection.)
    let buses = r(RC_BASE + 0x18);
    let newbuses = (buses & 0xFF00_0000) | 0x0001_0100; // pri=0, sec=1, sub=1; keep latency byte
    serial_println!(
        "{} M2: >>> WRITE: RP bus window {:#010x} -> {:#010x} (primary=0 secondary=1 subordinate=1 — forward type-1 cfg to bus 1) ::",
        P, buses, newbuses
    );
    w(RC_BASE + 0x18, newbuses);
    dsb();

    // ── PIUSB-7 ADJUDICATION (the boot-P3 CAP=0xdeaddead wall — memory forwarding, not config) ────────
    // boot-P1..P6 proved every CONFIG-path layer is GREEN: link UP, RP bus window latched, VL805
    // config[0x00]=0x34831106 answers, BAR0 latches to 0xc0000000, COMMAND MEM+BM read set — yet the
    // MEMORY read at CPU 0x6_0000_0000 (→ PCIe 0xc0000000) still returns 0xdeaddead. Config and memory
    // reach the VL805 by DIFFERENT gates in the root-port bridge: config forwarding is governed by the
    // bus-number window (0x18, fixed above), but a PCI-to-PCI bridge forwards MEMORY transactions from
    // its primary to its secondary bus ONLY when the target address falls inside its type-1 MEMORY
    // Base/Limit window (config 0x20) AND the bridge's own COMMAND Memory-Space-Enable is set. Both reset
    // to their power-on defaults: Memory Base/Limit = 0/0 (base 0 .. limit 0x000fffff — a base>limit
    // "forward nothing" window for our 0xc0000000 target), COMMAND MEM=0. So the CPU→PCIe memory TLP
    // reaches the root-port bridge and is DROPPED (not forwarded downstream) → master-abort fill
    // 0xdeaddead. This is the EXACT memory-forwarding analogue of the 0x18 bus-window fix that made
    // config forwarding work. Linux never hits it because pci_setup_bridge() programs the bridge memory
    // window from the child bus's assigned resources during enumeration; we ARE the enumerator, so we
    // program the RP bridge's memory window to cover the outbound region [0xc0000000, 0xffffffff] and
    // enable memory-space on the RP itself — BEFORE the first downstream memory access in M3.
    //
    // PCI-to-PCI bridge Memory Base/Limit (config 0x20, one dword): Memory Base in bits [15:0] holds
    // A[31:20] of the base in its top 12 bits [15:4]; Memory Limit in bits [31:16] holds A[31:20] of the
    // (inclusive) limit in its top 12 bits [31:20]. The low 20 bits of base are implied 0 and of limit
    // are implied 0xf_ffff, so a 1 MiB granularity covers our 1 GiB region exactly.
    const RP_MEM_BASE_A: u32 = (OUTBOUND_PCIE_BASE >> 20) as u32; // 0xc00
    const RP_MEM_LIMIT_A: u32 = ((OUTBOUND_PCIE_BASE + OUTBOUND_SIZE - 1) >> 20) as u32; // 0xfff
    const RP_MEM_BASE_LIMIT: u32 = ((RP_MEM_LIMIT_A & 0xfff) << 20) | ((RP_MEM_BASE_A & 0xfff) << 4);
    serial_println!(
        "{} M2: >>> WRITE: RP mem window [0x20] = {:#010x} (base {:#x} limit {:#x} — forward PCIe mem {:#x}..{:#x} to bus 1) ::",
        P, RP_MEM_BASE_LIMIT, OUTBOUND_PCIE_BASE, OUTBOUND_PCIE_BASE + OUTBOUND_SIZE - 1,
        OUTBOUND_PCIE_BASE, OUTBOUND_PCIE_BASE + OUTBOUND_SIZE - 1
    );
    w(RC_BASE + 0x20, RP_MEM_BASE_LIMIT);
    // Disable the prefetchable-memory window (0x24) explicitly: base>limit = forward nothing. Our BAR0 is
    // NON-prefetchable MMIO, so it must ride the (0x20) window above, never this one; a stale/enabled
    // prefetch window could otherwise shadow the region. Upper-32 base/limit (0x28/0x2c) → 0.
    w(RC_BASE + 0x24, 0x0000_fff0); // pref base A[31:20]=0xfff (0xfff00000), pref limit=0 → base>limit = off
    w(RC_BASE + 0x28, 0);
    w(RC_BASE + 0x2c, 0);
    dsb();
    let rb_memwin = r(RC_BASE + 0x20);
    serial_println!(
        "{} M2:   RP mem window readback [0x20] = {:#010x} -> {} ::",
        P, rb_memwin,
        if rb_memwin == RP_MEM_BASE_LIMIT { "LATCHED (bridge now forwards this mem range downstream)" } else { "DID NOT LATCH — RP mem window rejected" }
    );

    // Enable Memory-Space + Bus-Master on the ROOT PORT's own COMMAND (config 0x04, at RC_BASE directly).
    // A PCI-to-PCI bridge forwards downstream memory only while its COMMAND Memory-Space-Enable is set;
    // Bus-Master lets downstream DMA (upstream reads) traverse it. Read-modify-write; RP config is type-1
    // at RC_BASE (no EXT_CFG indirection).
    let rp_cmd = r(RC_BASE + 0x04);
    let rp_newcmd = (rp_cmd & 0xFFFF_0000) | ((rp_cmd & 0xFFFF) | 0b110);
    serial_println!(
        "{} M2: >>> WRITE: RP COMMAND {:#06x} -> {:#06x} (MEM+BusMaster enable on the root port — forward mem downstream) ::",
        P, rp_cmd & 0xffff, rp_newcmd & 0xffff
    );
    w(RC_BASE + 0x04, rp_newcmd);
    dsb();
    let rb_rp_cmd = r(RC_BASE + 0x04) & 0xffff;
    serial_println!(
        "{} M2:   RP COMMAND readback = {:#06x} (MEM={} BusMaster={}) -> {} ::",
        P, rb_rp_cmd, (rb_rp_cmd >> 1) & 1, (rb_rp_cmd >> 2) & 1,
        if (rb_rp_cmd & 0b110) == 0b110 { "RP MEM-FORWARDING ENABLED" } else { "NOT ENABLED — RP will still drop downstream mem" }
    );

    // PCIe spec: a device may take up to 100 ms after PERST# deassert before it must answer config
    // requests. Link training consumed part of that budget; burn the remainder (bounded) before the
    // first downstream read so a slow VL805 is not misread as absent.
    settle_ms(100);

    let mut idword = vl805_cfg_read(0x00);
    serial_println!("{} M2: VL805 config[0x00] = {:#010x} ::", P, idword);
    if is_poison(idword) {
        // One bounded retry after a further settle — distinguishes "still settling" from a hard
        // forwarding/absence failure before we diagnose.
        settle_ms(50);
        idword = vl805_cfg_read(0x00);
        serial_println!("{} M2: VL805 config[0x00] retry = {:#010x} ::", P, idword);
    }
    if is_poison(idword) {
        // Sharper honest diagnostic: did the RP bus window stick? A readback that lost our
        // secondary/subordinate programming points at RC/window misprogramming; a stuck window
        // with a still-poisoned child read points at the device (absent, or firmware not loaded).
        let rb = r(RC_BASE + 0x18);
        let window_ok = (rb & 0x00FF_FF00) == 0x0001_0100;
        serial_println!(
            "{}   child config POISON ({:#010x}); RP bus window readback {:#010x} — {} ::",
            P, idword, rb,
            if window_ok {
                "window PROGRAMMED (pri/sec/sub = 0/1/1) => forwarding is set up; the DEVICE did not answer (absent below the RP, or VL805 firmware not loaded)"
            } else {
                "window DID NOT STICK => RC config-forwarding misprogramming (index/data or offset math), NOT device-absent"
            }
        );
        return None;
    }
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

    // ── PIUSB-4 ADJUDICATION (the boot-P2 device-side 0xdeaddead root cause) ──────────────────────────
    // boot-P2 proved the outbound window armed CPU-side (every WIN0 readback correct, `WIN0 armed: YES`)
    // yet the CAP read at CPU 0x6_0000_0000 still returned 0xdeaddead — the abort moved DOWNSTREAM of the
    // RC. The offender is ORDERING: NOTIFY_XHCI_RESET makes the VideoCore firmware (re)load the VL805's
    // firmware, which RESETS the VL805's PCI config space — BAR0 and the COMMAND register revert to their
    // power-on defaults (BAR0 = 0, MEM decode off). The prior code assigned BAR0 + enabled COMMAND and
    // THEN issued the notify, so the firmware reload wiped both: the VL805 decoded its MMIO at BAR=0 (i.e.
    // NOT at PCIe 0xC000_0000), the CPU read of the outbound window hit nothing the device claimed, and
    // the RC returned its master-abort fill 0xdeaddead — device-side, exactly as captured.
    //
    // Linux never hits this because `quirk_vl805` is a DECLARE_PCI_FIXUP_HEADER: the firmware load/reset
    // runs during device scan, BEFORE the PCI core assigns BARs and enables the device (facts-only from
    // the fixup class; pcie-brcmstb.c / the firmware helper are GPL-2.0-only). We ARE the enumerator, so
    // we mirror that order here: notify FIRST (firmware settles the VL805 into a known reset state), THEN
    // size + assign BAR0 + enable COMMAND on top of that settled state, so nothing downstream reverts our
    // programming. Every layer is READ BACK and witnessed so boot-P3 names the failing layer if the wall
    // persists (mailbox SUCCESS is NOT proof the VL805 firmware is running; the config re-reads are).
    //
    // The dev_addr encoding (VL805_DEV_ADDR = (1<<20)|(0<<15)|(0<<12) = 0x0010_0000) matches the RPi
    // firmware property model — dev_id = (bus<<20)|(slot<<15)|(func<<12) for bus1/dev0/fn0. Verified
    // against the mailbox property-interface reference; constant unchanged.
    serial_println!("{}   NOTIFY_XHCI_RESET (mailbox 0x00030058, dev_addr={:#x}) — firmware VL805 reset/load (BEFORE BAR/COMMAND: the reload resets config space) ::", P, VL805_DEV_ADDR);
    let ok = mailbox::notify_xhci_reset(VL805_DEV_ADDR);
    serial_println!("{}   NOTIFY_XHCI_RESET reported {} ::", P, if ok { "SUCCESS (mailbox accepted — NOT proof the VL805 fw is running; see cfg re-read below)" } else { "FAILURE (firmware may already have loaded it at boot)" });
    // Let the VideoCore firmware finish loading/resetting the VL805 before we touch its config space.
    settle_ms(50);

    // Witness LAYER 2 (firmware running / device survival): re-read the identity word AFTER the notify.
    // If the VL805 vanished or the RP forwarding broke across the reset, this catches it before we
    // program a BAR into thin air. A live vendor:device here = the device answered post-reset.
    let post = vl805_cfg_read(0x00);
    match live_vendor_device(post) {
        Some((v, d)) => serial_println!("{}   post-NOTIFY cfg[0x00] = {:#010x} (vendor={:#06x} device={:#06x} — device survived the fw reset, answering config) ::", P, post, v, d),
        None => {
            serial_println!("{}   post-NOTIFY cfg[0x00] = {:#010x} — device did NOT answer after the firmware reset (absent/forwarding lost) — bring-up STOP ::", P, post);
            return None;
        }
    }

    // ── BAR0 sizing ritual (all-ones probe + immediate restore; the ORIN-NET-3 pattern). Now runs on the
    //    freshly-reset device, so `orig` reflects the post-reset default (typically 0) — that is fine: we
    //    reassign BAR0 explicitly below regardless. ──
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
    let is_64bit = mem_type == 0x2;
    let mask = readback & !0xf;
    let size = ((!(mask as u64)) & 0xFFFF_FFFF).wrapping_add(1);
    serial_println!(
        "{}   BAR0 = {} mem, 64bit={}, size={:#x} ::",
        P, if is_io { "I/O(!)" } else { "MMIO" }, is_64bit, size
    );

    // ── Assign BAR0 to the outbound window's PCIe base and enable decode. The VL805's xHCI MMIO will
    //    then be decoded at PCIe 0xC000_0000, which the CPU reaches at OUTBOUND_CPU_BASE (mapped in M3). ──
    let bar0_pcie = OUTBOUND_PCIE_BASE;
    serial_println!("{}   >>> WRITE: BAR0 := {:#x} (PCIe-side; CPU sees it at {:#x}) ::", P, bar0_pcie, OUTBOUND_CPU_BASE);
    let bar0_lo = (bar0_pcie & 0xFFFF_FFF0) as u32 | (orig & 0xf);
    vl805_cfg_write(0x10, bar0_lo);
    // Witness LAYER 1 (BAR upper dword): ALWAYS write BAR0[0x14] explicitly. For our sub-4-GiB PCIe base
    // (0xC000_0000) the high half is 0; leaving it stale/garbage on a 64-bit BAR would make the device
    // decode at <garbage>:0xC000_0000 — nowhere — which is one of the boot-P2 suspect classes. Write 0
    // unconditionally (harmless on a 32-bit BAR: [0x14] is then the next BAR, which we do not use and
    // which decode-enable does not arm until assigned), then read BOTH dwords back and witness them.
    let bar0_hi = (bar0_pcie >> 32) as u32; // = 0 for 0xC000_0000
    vl805_cfg_write(0x14, bar0_hi);
    let rb_bar_lo = vl805_cfg_read(0x10);
    let rb_bar_hi = vl805_cfg_read(0x14);
    let bar_ok = rb_bar_lo == bar0_lo && (!is_64bit || rb_bar_hi == bar0_hi);
    serial_println!(
        "{}   BAR0 readback: [0x10]={:#010x} [0x14]={:#010x} (wrote LO={:#010x} HI={:#010x}, 64bit={}) -> {} ::",
        P, rb_bar_lo, rb_bar_hi, bar0_lo, bar0_hi, is_64bit,
        if bar_ok { "LATCHED" } else { "DID NOT LATCH — BAR programming did not stick" }
    );

    // Command register (0x04): set MEM Space Enable (bit1) + Bus Master Enable (bit2). Enable decode LAST,
    // after the BAR is in place — a device must not decode before its BAR is assigned.
    let cmd = vl805_cfg_read(0x04);
    let newcmd = (cmd & 0xFFFF_0000) | ((cmd & 0xFFFF) | 0b110);
    serial_println!("{}   >>> WRITE: VL805 COMMAND {:#06x} -> {:#06x} (MEM+BusMaster enable) ::", P, cmd & 0xffff, newcmd & 0xffff);
    vl805_cfg_write(0x04, newcmd);
    // Witness LAYER 3 (enable/settle): read COMMAND back — MEM(bit1)+BM(bit2) must both read set, else the
    // device will not decode its BAR no matter how the window is armed.
    let rb_cmd = vl805_cfg_read(0x04) & 0xffff;
    let cmd_ok = (rb_cmd & 0b110) == 0b110;
    serial_println!(
        "{}   COMMAND readback = {:#06x} (MEM={} BusMaster={}) -> {} ::",
        P, rb_cmd, (rb_cmd >> 1) & 1, (rb_cmd >> 2) & 1,
        if cmd_ok { "DECODE ENABLED" } else { "DECODE NOT ENABLED — MEM/BM bits did not set" }
    );
    // Let decode settle before M3's first MMIO read at the CPU window (enable/settle ordering).
    settle_ms(10);

    Some(bar0_pcie)
}

/// M3: map the outbound window Device-nGnRnE, read the xHCI capability registers at the VL805 BAR
/// (poison-rejecting), attach the shared xHCI driver in polled mode (halt + reset = "halted-but-
/// decoding"), power the root ports, and STOP. Full device enumeration (rings, ADDRESS_DEVICE, HID/
/// storage) needs the heap + a live device and is the attended-metal follow-on — not this arc.
fn m3_attach_xhci(cpu_base: u64, _bar0_pcie: u64) {
    // BELT LINK-UP GATE (PIUSB-6): re-confirm PHYLINKUP && DL_ACTIVE before the CAP probe into the
    // outbound window. M3 is only reached after M1 declared link-up, but a link that dropped between M1
    // and here would make this CAP read an MMIO into a link-down RC — which stalls the bus for a
    // pathologically long time (boot-P4). Refuse the probe if the link is not up; RC-STATUS is a safe
    // RC-register read.
    let status = r(RC_BASE + PCIE_MISC_PCIE_STATUS);
    let up_mask = PCIE_MISC_PCIE_STATUS_PHYLINKUP | PCIE_MISC_PCIE_STATUS_DL_ACTIVE;
    if is_poison(status) || (status & up_mask) != up_mask {
        serial_println!(
            "{} M3: PCIE_STATUS={:#010x} link NOT up (PHYLINKUP={} DL_ACTIVE={}) — refusing the CAP probe into a link-down RC (link-down MMIO stalls the bus); attach SKIPPED, fail-closed ::",
            P, status,
            status & PCIE_MISC_PCIE_STATUS_PHYLINKUP != 0,
            status & PCIE_MISC_PCIE_STATUS_DL_ACTIVE != 0
        );
        return;
    }

    // Map the 1 GiB Device block that contains `cpu_base` into the live translation regime (the ONLY
    // new page-table write this arc makes; `boot::map_device_1gib`, piusb-gated; idempotent if the
    // reference dump already mapped this block). The RC forwards CPU reads of this window to the VL805's
    // BAR (programmed in M2 on the reset path).
    unsafe { super::boot::map_device_1gib(cpu_base) };
    serial_println!("{} M3: mapped outbound window CPU {:#x} (Device-nGnRnE, 1 GiB) — reading VL805 xHCI caps ::", P, cpu_base);
    let outbound_cpu_base = cpu_base;

    // Bounded settle+retry on the FIRST MMIO read (the ORIN readback-ritual idiom): a freshly-enabled
    // decode path can take a few cycles to answer. Try up to N times with a short settle between; a
    // still-poisoned read after the budget is an honest fail-closed, not a hang. Fail-closed after N.
    const CAP_TRIES: u32 = 8;
    let mut cap0 = r(outbound_cpu_base);
    let mut tries = 1u32;
    while (is_poison(cap0) || cap0 == 0) && tries < CAP_TRIES {
        settle_ms(5);
        cap0 = r(outbound_cpu_base);
        tries += 1;
    }
    if is_poison(cap0) || cap0 == 0 {
        serial_println!(
            "{}   xHCI CAP register reads {:#010x} @ {:#x} after {} tries — BAR not decoding (window/BAR mismatch or firmware not loaded); attach SKIPPED, fail-closed ::",
            P, cap0, outbound_cpu_base, tries
        );
        return;
    }
    if tries > 1 {
        serial_println!("{}   xHCI CAP settled after {} tries (first read was poison; decode came up on retry) ::", P, tries);
    }
    let cap_length = (cap0 & 0xff) as u8;
    let hci_version = (cap0 >> 16) as u16;
    let hcsparams1 = r(outbound_cpu_base + 0x04);
    let max_ports = ((hcsparams1 >> 24) & 0xff) as u8;
    serial_println!(
        "{}   xHCI DECODING: CAPLENGTH={} HCIVERSION={:#06x} HCSPARAMS1={:#010x} MaxPorts={} ::",
        P, cap_length, hci_version, hcsparams1, max_ports
    );

    // Attach the shared xHCI driver in polled mode: halt + HCRST + CNR wait (heap-free; no ring
    // allocation — that is the enumerating attach, deferred to metal). After this the controller is
    // HALTED-BUT-DECODING: its registers answer, it is reset to a clean state, ready for the metal
    // enumeration arc to program rings and pump ports.
    serial_println!("{}   attaching shared xHCI driver @ {:#x} (polled, halt+reset) ::", P, outbound_cpu_base);
    xhci::init(outbound_cpu_base);

    // Power the root ports: set PORTSC.PP (bit 9) on each port register (operational base +0x400 +
    // 0x10*port). A powered port can detect a device connect; the metal arc pumps the connect events.
    let op_base = outbound_cpu_base + cap_length as u64;
    let ports = max_ports.max(1).min(16); // clamp: a plausible VL805 has a small port count
    for port in 0..ports {
        let portsc_addr = op_base + 0x400 + (port as u64) * 0x10;
        let portsc = r(portsc_addr);
        if is_poison(portsc) {
            continue;
        }
        // Preserve everything except the RW1C change bits (do not clear/ack them here) and OR in PP.
        // Mask off the change/RW1C bits so we don't accidentally clear a latched change: PP is bit 9.
        //
        // PI-USB-2 M2 robustness nit: PED (Port Enabled/Disabled, bit 1) is RW1CS — writing it back as
        // 1 DISABLES the port. On a warm/inherited controller PED can read 1 for an already-enabled
        // port, so a naive RMW that only masked the RW1C change bits would write PED=1 and tear the
        // port's own enable down. Mask PED off alongside the change bits (hardware sets PED itself on a
        // successful reset; we never assert it): PP-set becomes a clean "power on, disturb nothing".
        const PORTSC_PP: u32 = 1 << 9;
        const PORTSC_PED: u32 = 1 << 1; // RW1CS: write-1 clears (disables) the port — must mask off
        const PORTSC_RW1C: u32 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);
        w(portsc_addr, (portsc & !(PORTSC_RW1C | PORTSC_PED)) | PORTSC_PP);
    }
    dsb();
    settle_ms(20); // let port power stabilize before the honesty read

    // PI-USB-2 handoff: record the decoding xHCI base + arm the DMA-side entry. `enumerate()` (called
    // post-heap from kernel_main) picks up here to program rings/interrupter and walk the ports. If we
    // never reach this line (QEMU census-skip, no link, BAR mismatch), XHCI_READY stays false and
    // `enumerate()` cleanly reports "honesty line not reached" and returns.
    XHCI_CPU_BASE.store(outbound_cpu_base, Ordering::Release);
    XHCI_READY.store(true, Ordering::Release);

    serial_println!("{} M3: {} root port(s) powered (PORTSC.PP set); controller halted-but-decoding — HONESTY LINE reached ::", P, ports);
    serial_println!(
        "{}   NEXT (post-heap, this arc): `piusb::enumerate()` programs rings + interrupter, RS=1, pumps port connects, ADDRESS_DEVICE, HID/storage enumeration ::",
        P
    );
}

/// Mirror of the shared driver's keyboard-armed predicate (the JB2b pattern, `xusb_tegra.rs`): a slot
/// is an armed HID keyboard once it is active, typed as a keyboard, and its interrupt-IN read FSM has
/// reached the armed state (3). Returns `(slot, root_port)` for the first such slot.
fn keyboard_armed(x: &xhci::XhciController) -> Option<(u8, u8)> {
    for (i, s) in x.slots.iter().enumerate() {
        if s.active && s.is_keyboard && s.keyboard_state == 3 {
            return Some((i as u8, s.port_id));
        }
    }
    None
}

/// PI-USB-2 M3 — the DMA-side bring-up + device enumeration on the VL805. Called ONCE on the BSP,
/// post-heap (kernel_main), after `bringup` reached the honesty line pre-heap. Reuses the shared xHCI
/// driver's polled-attach machinery verbatim (the JB2b pattern, `xusb_tegra::jb2b_attach`) with ZERO
/// xhci-core edits: `xhci::init` (halt+HCRST+CNR — this is OUR freshly-reset controller, so the plain
/// reset path, NOT the inherited-controller no-HCRST/CRCR takeover), then rings/DCBAA/interrupter via
/// `XhciController::new` + `COMMAND_RING`/`EVENT_RING`/`ERST_TABLE` + `init_interrupter`/`init_pointers`,
/// `start()` (RS=1), then a bounded polled-enumeration pump. Stops at keyboard-ARMED (or the bounded
/// window), dumping per-device identity lines. Heap-free paths only outside the rings.
///
/// Graceful degradation: if the honesty line was never reached this boot (QEMU raspi4b census-skip,
/// link-down, or a BAR/window mismatch), `XHCI_READY` is false — we say so and return. This is the
/// exact QEMU behaviour of rung 1 carried one rung forward: QEMU never builds a ring here.
pub fn enumerate() {
    if !XHCI_READY.load(Ordering::Acquire) {
        serial_println!(
            "{} enumerate: honesty line not reached this boot (no VL805 xHCI decoding — expected in QEMU raspi4b: models no PCIe RC/VL805) — DMA-side enumeration skipped, graceful degradation ::",
            P
        );
        return;
    }
    let base = XHCI_CPU_BASE.load(Ordering::Acquire);

    // Re-confirm the BAR still decodes before we build rings against it (poison-rejecting — the
    // PI-V3D-1 false-pass rule). A window/BAR regression since the honesty line = fail-closed here.
    let cap0 = r(base);
    if is_poison(cap0) || cap0 == 0 {
        serial_println!(
            "{} enumerate: xHCI CAP reads {:#010x} @ {:#x} — no longer decoding; enumeration SKIPPED, fail-closed ::",
            P, cap0, base
        );
        // Post-vector fail-closed probe: drain any latent async abort inline (SError-drain class).
        super::exceptions::serror_drain_request("piusb: enumerate CAP poison");
        return;
    }
    let cap_length = (cap0 & 0xff) as u64;
    serial_println!("{} enumerate: DMA-side bring-up @ {:#x} (CAPLENGTH={}) — OUR controller, fresh reset + RS=1 ::", P, base, cap_length);

    // (1) Halt + HCRST + CNR-wait: a clean freshly-reset controller immediately before we seat rings
    //     (the JB2b ordering; idempotent even though `bringup` reset it pre-heap — the intervening
    //     heap/AP/emmc bring-up leaves the halted controller untouched, but a defensive reset here
    //     guarantees the RS=1 below rides a known state).
    xhci::init(base);

    // (2) Rings / DCBAA / interrupter via the driver's existing init paths (heap-backed). Exactly the
    //     JB2b sequence: construct the controller, seat the command + event rings, point the
    //     interrupter's ERST at the event ring, seat the command-ring pointer, then RS=1.
    unsafe {
        let mut x = xhci::XhciController::new(base as usize);
        let (event_ring_phys, command_ring_phys) = {
            let mut cmd_ring_guard = xhci::COMMAND_RING.lock();
            let mut evt_ring_guard = xhci::EVENT_RING.lock();
            *cmd_ring_guard = Some(xhci::ring::TransferRing::new(256));
            *evt_ring_guard = Some(xhci::event::EventRing::new());
            (
                evt_ring_guard.as_mut().unwrap().get_ptr(),
                cmd_ring_guard.as_mut().unwrap().get_ptr(),
            )
        };
        let erst_table_phys = &raw mut xhci::ERST_TABLE as u64;
        serial_println!("{}   programming interrupter + rings (runtime regs), then RS=1 ::", P);
        x.init_interrupter(event_ring_phys, erst_table_phys);
        x.init_pointers(command_ring_phys);
        x.start();
        *xhci::XHCI_CONTROLLER.lock() = Some(x);
    }

    // (3) Bounded polled-enumeration pump (the JB2b pump, minus the Jetson SMMU/SID forensics). Only a
    //     metal boot with a live controller reaches here (QEMU returned at the READY gate), so a device
    //     that never enumerates pays the full window ONCE, honestly, with the driver's stage lines
    //     keeping the console alive; a keyboard exits early at ARMED, storage after a short settle.
    let pump_start = super::timer::cntpct();
    let freq = super::timer::cntfrq();
    let deadline = pump_start.wrapping_add(freq.saturating_mul(30)); // 30 s worst-case backstop
    let storage_settle = freq.saturating_mul(6); // keyboard-up: let a hubbed MSC settle, then stop
    let mut armed: Option<(u8, u8)> = None;
    let mut armed_at: u64 = 0;
    loop {
        let (armed_now, storage_slot, storage_ready) = {
            let mut guard = xhci::XHCI_CONTROLLER.lock();
            let x = guard.as_mut().unwrap();
            x.poll_events();
            x.service_hubs();
            x.service_hid_setproto();
            x.service_slot_disposal();
            x.service_enum();
            x.service_storage();
            (
                keyboard_armed(x),
                x.storage_slot,
                x.storage_slot != 0 && x.storage_note == "ready",
            )
        };
        if armed.is_none() {
            if let Some((slot, port)) = armed_now {
                serial_println!("{} enumerate: keyboard ARMED (slot {}, root port {}) -> PASS ::", P, slot, port);
                armed = Some((slot, port));
                armed_at = super::timer::cntpct();
            }
        }
        if armed.is_some() {
            if storage_ready {
                serial_println!("{} enumerate: mass storage ready (slot {}) ::", P, storage_slot);
                break;
            }
            if super::timer::cntpct().wrapping_sub(armed_at) >= storage_settle {
                break; // keyboard up, no disk within the settle window — stop
            }
        }
        // Wrap-safe deadline test (the ORIN pattern): elapsed past deadline once (now - deadline)
        // stays in the low half.
        if super::timer::cntpct().wrapping_sub(deadline) < (1u64 << 63) {
            break;
        }
        core::hint::spin_loop();
    }

    // (4) Per-device identity lines — whatever the polled walk enumerated (kbd/mouse/storage), one
    //     line each, so the boot's serial names the topology it reached (honest even on a no-device or
    //     deadline exit).
    serial_println!("{} enumerate: device topology after the polled walk: ::", P);
    if let Some(x) = xhci::XHCI_CONTROLLER.lock().as_ref() {
        for line in x.port_slot_summary() {
            serial_println!("{}   {} ::", P, line);
        }
    }
    for line in xhci::usb_summary() {
        serial_println!("{}   {} ::", P, line);
    }
    match armed {
        Some((slot, port)) => serial_println!("{} enumerate: DONE — keyboard armed (slot {}, port {}); enumeration + HID arming complete ::", P, slot, port),
        None => serial_println!("{} enumerate: DONE — no keyboard armed within the bounded window (honest result; see topology above) ::", P),
    }
}
