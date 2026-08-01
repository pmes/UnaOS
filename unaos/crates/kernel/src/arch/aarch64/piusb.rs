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

// ─── PIUSB-6/16: the adopt-don't-reset path. boot-P4 proved that with the OLD boot firmware (pftf UEFI
// v1.52) the VideoCore tore PCIe down before handoff (`PHYLINKUP=false DL_ACTIVE=false`, all windows
// zero, RC fully in reset), so PIUSB-6 RETIRED adopt: there was never a firmware-left decode to adopt.
// PIUSB-16 changes the lever UPSTREAM of this file — the image build now ships the CURRENT official
// raspberrypi/firmware start4.elf/fixup4.dat/DTB (arroyo FW_TAG), which does the stock power-on
// VL805/PCIe init + firmware push before handoff. So adopt is RE-ENABLED, but STRICTLY GATED on the
// ENTRY link state read in `bringup`: only when the firmware handed us `PHYLINKUP && DL_ACTIVE` (RC live)
// do we take the adopt branch — skip M1's RGR1/PERST reset (which would tear the VC's trained link +
// loaded VL805 fw back down), keep the VC's outbound WIN0, and go straight through M2 (config/NOTIFY —
// non-destructive to the link) into M3. A cold entry (RC still in reset) falls through to the unchanged
// COLD-BUILD path (M1 reset → M2 → M3). The pre-reset dump stays a one-boot-proven read-only record,
// hard-gated on link-up (a link-down RC must never be MMIO'd past its own register block). ───

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
// ── PIUSB-18: the MISC_CTRL access-control bits Linux `brcm_pcie_setup` programs and our step (d) has
// been OMITTING. SCB_ACCESS_EN gates the RC's system-cache-bus (SCB) master path — the inbound route by
// which a downstream master (or the VideoCore's VL805 firmware-load engine) reaches host SDRAM. Sizing
// SCB0 (the window) without enabling SCB_ACCESS (the master) leaves the inbound path CLOSED: the RC will
// not issue SCB cycles at all. P28 metal proved our bring-up left MISC_CTRL=0x88000000 — SCB0_SIZE=0x11
// set, bit12 (SCB_ACCESS_EN) CLEAR. Facts-only from pcie-brcmstb.c (GPL-2.0-only): bit masks + the fact
// that brcm_pcie_setup sets SCB_ACCESS_EN=1, CFG_READ_UR_MODE=1, MAX_BURST_SIZE=128B (=0 on BCM2711).
const PCIE_MISC_MISC_CTRL_SCB_ACCESS_EN: u32 = 1 << 12; // 0x0000_1000 — enable RC SCB (inbound) master
const PCIE_MISC_MISC_CTRL_CFG_READ_UR_MODE: u32 = 1 << 13; // 0x0000_2000 — return UR (not fault) on bad cfg read
const PCIE_MISC_MISC_CTRL_MAX_BURST_SIZE_MASK: u32 = 0x3 << 20; // bits[21:20]; 0b00 = 128B (BCM2711)

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

// ─── PIUSB-8 → XHCI-COHERENCE: the external non-coherent-DMA bridge is RETIRED. ─────────────────────
// The VL805 reaches host RAM through the BCM2711 RC's inbound window (RC_BAR2, RAM@0 4 GiB), and that
// PCIe path is NOT I/O-coherent — a PCIe master read/write does NOT snoop the A72 data caches. PIUSB-8
// bridged this from the attach side over the three pub structures the driver exposed (command/event
// rings + ERST), which was partial by construction — the per-device Input/Device contexts, DCBAA,
// scratchpad and transfer rings the driver allocates internally were unreachable from here. The durable
// cure now lives INSIDE the shared driver: the `drivers/xhci::dma_coherency` seam (gated
// `target_arch = "aarch64"`, no-op on x86) cleans/invalidates every DMA structure at its
// producer/consumer boundary, so the Pi 4 (and the Jetson) are coherent by construction with NO cache
// maintenance in this file. The old `dcache_*` helpers + ring-span constants are gone with the bridge.

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

// ── PIUSB-40: stage timing. Metal P59b timed a ~3-minute boot pause on the COLD-BUILD path with a
//    stopwatch; an operator stopwatch cannot name WHICH stage spent it. These two helpers bracket every
//    bring-up stage off CNTVCT (the same free-running counter `settle_ms`/`ms_now` use — always live, no
//    init, no interrupt dependency) and print ONE line per stage:
//        :: PIUSB: [piusb40] stage=<name> took=<ms>ms (t=<ms>ms) ::
//    Cost is one counter read per stage plus one serial line — always-on within the `piusb` knob, and
//    (deliberately) emitted only on paths the DTB census already admitted to metal, so a QEMU raspi4b
//    boot (no `pcie@` node → `bringup_inner` returns before the first stage; `XHCI_READY` false →
//    `enumerate` returns before the first stage) produces ZERO `[piusb40]` lines and a byte-identical log.
//    No stage helper performs MMIO — they are safe wherever they are placed.
#[inline]
fn stage_begin() -> u64 {
    super::timer::cntpct()
}

/// Close a stage opened by `stage_begin` and print its wall-clock cost. Returns the closing timestamp so
/// a caller can chain stages back-to-back without re-reading the counter.
fn stage_end(name: &str, t0: u64) -> u64 {
    let now = super::timer::cntpct();
    let freq = super::timer::cntfrq().max(1);
    let took = now.wrapping_sub(t0).saturating_mul(1000) / freq;
    serial_println!("{} [piusb40] stage={} took={}ms (t={}ms) ::", P, name, took, now.saturating_mul(1000) / freq);
    now
}

/// PIUSB-40: how long ONE MMIO read into the outbound window may take before we treat the RC as eating a
/// completion timeout rather than answering. A healthy Device-nGnRnE read off a decoding BAR is sub-
/// microsecond; a read the RC cannot forward is absorbed by the bridge's completion-timeout machinery and
/// costs orders of magnitude more. 20 ms is far above any honest read and far below the multi-second
/// stalls metal has shown, so it separates the two without ever cutting a real answer short.
const MMIO_ABORT_COST_MS: u64 = 20;

/// PIUSB-40: total wall-clock a poison-retry ladder over the outbound window may spend. The ladders this
/// caps are pure "has the freshly-enabled decode woken up yet" polls — the PCIe-mandated settles
/// (T_PVPERL, the 100 ms config-request window) are SEPARATE and untouched. 60 ms is ~12x the 5 ms
/// inter-try settle these ladders were written around, so on a link that answers at all the ladder still
/// completes exactly as before; it only bites when every try is being absorbed.
const MMIO_LADDER_BUDGET_MS: u64 = 60;

/// PIUSB-40: read `addr` until it returns a live (non-poison, non-zero) word, bounded BOTH by a total
/// wall-clock budget and by a per-read cost check. Returns `(value, tries, elapsed_ms, bailed_on_cost)`.
///
/// Why this replaced the fixed `while tries < N { settle_ms(5); }` ladders: those ladders were budgeted as
/// if a try cost `5 ms` (the settle) — but on the COLD-BUILD path a try that the RC cannot forward costs
/// the RC's own completion timeout, which metal has shown reaching seconds. A ladder that "obviously"
/// spends 40 ms can therefore spend minutes, serially, with no output between tries. So: measure each
/// read; the moment ONE read costs more than `MMIO_ABORT_COST_MS` the ladder stops — a read that
/// expensive is a master-abort being absorbed, and every further try buys the same abort at the same
/// price. That is fail-fast to the honest no-USB path, not a weakened wait: nothing here is a settle the
/// PCIe/Linux sequence mandates, and the caller's fail-closed branch is unchanged.
fn mmio_settle_read(addr: u64, max_tries: u32) -> (u32, u32, u64, bool) {
    let freq = super::timer::cntfrq().max(1);
    let budget = (freq * MMIO_LADDER_BUDGET_MS) / 1000;
    let abort_cost = (freq * MMIO_ABORT_COST_MS) / 1000;
    let start = super::timer::cntpct();
    let mut tries = 0u32;
    let mut v;
    loop {
        // LENS FIX (1): `MRS CNTPCT_EL0` is NOT ordered against a Device load, so without a barrier the
        // closing counter read may retire BEFORE the load's completion and the measured cost
        // UNDER-REPORTS an absorbed read — silently disarming the abort diagnostic, which is this arc's
        // whole point. `dsb sy` on both sides pins the load inside the measured interval. (The 60 ms wall
        // budget below is unaffected either way: it is the real safety bound. The barrier is what makes
        // the DIAGNOSTIC trustworthy.)
        dsb();
        let read_at = super::timer::cntpct();
        v = r(addr);
        dsb();
        let cost = super::timer::cntpct().wrapping_sub(read_at);
        tries += 1;
        let elapsed_ms = || super::timer::cntpct().wrapping_sub(start).saturating_mul(1000) / freq;
        if !is_poison(v) && v != 0 {
            return (v, tries, elapsed_ms(), false);
        }
        if cost >= abort_cost {
            let el = elapsed_ms();
            serial_println!(
                "{} [piusb40] mmio-ladder @ {:#x}: try {} took {}ms (>= {}ms) — consistent with the RC absorbing this read as a completion timeout rather than answering it; further tries would buy the same abort at the same price. Ladder STOPPED at {}ms (fail-fast to the honest no-USB path) ::",
                P, addr, tries, cost.saturating_mul(1000) / freq, MMIO_ABORT_COST_MS, el
            );
            return (v, tries, el, true);
        }
        let out_of_tries = tries >= max_tries;
        let out_of_budget = super::timer::cntpct().wrapping_sub(start) >= budget;
        if out_of_tries || out_of_budget {
            // LENS FIX (2): this arm used to return UNLOGGED, so a ladder that hit the wall budget — the
            // arm that actually bounds the cold-build path — left no trace at all. Say which wall was hit.
            let el = elapsed_ms();
            serial_println!(
                "{} [piusb40] mmio-ladder @ {:#x}: exhausted after {} tries ({}ms), last read {:#010x} — {} ::",
                P, addr, tries, el, v,
                if out_of_budget {
                    "WALL BUDGET reached (the ladder's true safety bound); no single read was slow enough to trip the abort diagnostic"
                } else {
                    "try count reached within budget (reads are answering promptly — the BAR simply is not decoding yet)"
                }
            );
            return (v, tries, el, false);
        }
        settle_ms(5);
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

// ─── PIUSB-32: the power/clock CENSUS — the deferred-RC-stall diagnostic. ────────────────────────────
//
// Metal P39/P40/P41: with V3D + GENET + the panel live, the FIRST RC APB register read
// (RGR1_SW_INIT_1 @ 0xfd509210) HARD-STALLS the CPU — the `piusb31` enter line prints, the exit line
// never does, with no fault and no timeout. The SAME read is fine in P38's early (pre-V3D/pre-SMP/
// pre-panel) single-threaded context. The question this census answers on the next metal boot: did any
// firmware-managed power/clock domain that could feed the RC's register interface CHANGE state between
// P38's early context and the deferred context (e.g. V3D bring-up's mailbox SET_POWER/SET_CLOCK
// reassigning a shared PLL parent, or a firmware runtime-PM powering an unclaimed domain down)?
//
// ── AUTHORITATIVE DTB FACT (bcm2711.dtsi, `pcie@7d500000`) ──────────────────────────────────────────
// The BCM2711 PCIe RC node carries NO `clocks`, `power-domains`, `resets` or `reset-names` property
// (GENET @7d580000 likewise). So in the Linux/firmware model the RC's APB register block is NOT behind a
// firmware-managed power/clock/reset domain the OS is expected to claim — it is expected to be
// always-clocked. That makes the "firmware gated the RC APB clock" and "V3D toggled a PLL feeding the RC
// register clock" theories UNSUPPORTED by the device tree; this census is here to PROVE OR REFUTE them
// empirically at the next boot (there is no DTB node to consult on real silicon for these state words).
//
// Candidate firmware ids (RPi firmware property interface; two id namespaces):
//   * V3D power DOMAIN 10 + V3D clock 5 — the ONLY power/clock domains our boot now writes before the
//     deferred RC read (via `v3d::bringup` inside `init_framebuffer`); GENET and the framebuffer alloc
//     issue no SET_POWER/SET_CLOCK. If V3D bring-up perturbed a shared parent, these are where it shows.
//   * USB HCD DEVICE 3 (the GET/SET_POWER_STATE device-id namespace: 3 = USB HCD) — the closest
//     firmware "USB power" handle, witnessed as a control.
// The census is READ-ONLY by default; per the PIUSB-32 brief step 4 it will ENABLE a candidate ONLY IF
// the firmware reports it PRESENT-BUT-OFF (a genuine off-when-it-should-be-on), witnessing the flip. It
// is mailbox-only (timeout-bounded — never the source of a hard stall) so it is safe on BOTH the early
// boot-critical path and the deferred path.
const CENSUS_DOMAIN_V3D: u32 = 10;
const CENSUS_CLOCK_V3D: u32 = 5;
const CENSUS_DEVICE_USB_HCD: u32 = 3;

/// Decode a firmware state reply word into a short verdict. bit1 = does-not-exist, bit0 = on/active.
fn census_state(word: Option<u32>) -> (&'static str, u32) {
    match word {
        None => ("mbox-fail", 0xffff_ffff),
        Some(w) if w & 0x2 != 0 => ("ABSENT", w),
        Some(w) if w & 0x1 != 0 => ("ON", w),
        Some(w) => ("off", w),
    }
}

/// PIUSB-32 census: witness (and, if genuinely off, enable) the candidate firmware power/clock domains at
/// `ctx`. Prints one `[piusb32]` line per candidate plus a verdict, all mailbox-only (no RC MMIO). Called
/// in BOTH the early P38-equivalent context (`bringup`) and the deferred context (`bringup_inner`) so the
/// two boots' lines can be diffed: a candidate whose state DIFFERS across the wall is the smoking gun; all
/// three IDENTICAL and ON exonerates the firmware-power/clock theory and localizes the stall to the
/// deferred execution context itself (the split-design signal — see the arc report).
fn power_clock_census(ctx: &str) {
    let dom = mailbox::get_domain_state(CENSUS_DOMAIN_V3D);
    let clk = mailbox::get_clock_state(CENSUS_CLOCK_V3D);
    let usb = mailbox::get_power_state(CENSUS_DEVICE_USB_HCD);
    let (dom_s, dom_w) = census_state(dom);
    let (clk_s, clk_w) = census_state(clk);
    let (usb_s, usb_w) = census_state(usb);
    serial_println!(
        "{}   [piusb32] census({}): V3D-domain({}) power={} ({:#010x}) | V3D-clock({}) clock={} ({:#010x}) | USB-HCD-dev({}) power={} ({:#010x}) ::",
        P, ctx, CENSUS_DOMAIN_V3D, dom_s, dom_w, CENSUS_CLOCK_V3D, clk_s, clk_w, CENSUS_DEVICE_USB_HCD, usb_s, usb_w
    );

    // Step 4 (conditional enable): only a candidate the firmware reports PRESENT-BUT-OFF gets flipped on,
    // and the flip is re-witnessed. On metal V3D is freshly powered/clocked (expected ON), so this is a
    // no-op unless something genuinely powered a candidate down — in which case the re-witness proves it.
    if dom_s == "off" {
        let got = mailbox::set_power_domain(CENSUS_DOMAIN_V3D, 1);
        let re = census_state(mailbox::get_domain_state(CENSUS_DOMAIN_V3D));
        serial_println!(
            "{}   [piusb32] census({}): V3D-domain({}) was OFF — SET on -> {:?}, re-read {} ({:#010x}) ::",
            P, ctx, CENSUS_DOMAIN_V3D, got, re.0, re.1
        );
    }
    if clk_s == "off" {
        let got = mailbox::set_clock_state(CENSUS_CLOCK_V3D, true);
        let re = census_state(mailbox::get_clock_state(CENSUS_CLOCK_V3D));
        serial_println!(
            "{}   [piusb32] census({}): V3D-clock({}) was OFF — SET on -> {:?}, re-read {} ({:#010x}) ::",
            P, ctx, CENSUS_CLOCK_V3D, got, re.0, re.1
        );
    }
}

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
    // PIUSB-18: also witness SCB_ACCESS_EN (bit12) — a sized SCB0 with SCB_ACCESS_EN=0 is the "inbound
    // window armed but master disabled" state that closes the RC's route to SDRAM (the P28 wall).
    let scb_access = misc & PCIE_MISC_MISC_CTRL_SCB_ACCESS_EN != 0;
    serial_println!(
        "{}   RC-claim witness: MISC_CTRL={:#010x} SCB0_SIZE={:#x} SCB1_SIZE={:#x} SCB2_SIZE={:#x} SCB_ACCESS_EN={} | RC_BAR2 LO={:#010x} HI={:#010x} ::",
        P, misc, scb0, scb1, scb2, scb_access as u8, bar2_lo, bar2_hi
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

/// PIUSB-14: the NOTIFY-boundary witness. Prints the exact downstream state the VideoCore's
/// `NOTIFY_XHCI_RESET` handler depends on to reach the VL805 and (re)load its firmware over PCIe —
/// captured immediately BEFORE and immediately AFTER a NOTIFY so a single boot shows what the fw-load
/// touched. On this EEPROM-less rev-1.5 board the VC MUST push the VL805 firmware across the link at
/// NOTIFY time (there is no SPI EEPROM), so the link must be trained (DL_ACTIVE) AND the RC's config-
/// forwarding path to bus1/dev0 must be live AT THE MOMENT the handler runs. The discriminators this
/// captures across the boundary:
///   * PCIE link (PHYLINKUP/DL_ACTIVE) — was the link up when the VC ran the handler, and did the VC
///     DROP it (its handler may PERST the VL805 to "reset" it, which would re-train the link and could
///     tear our forwarding down)?
///   * RGR1_SW_INIT_1 + RP bus window [0x18] + RP mem window [0x20] + RP COMMAND — did the VC CLOBBER
///     our RC programming (a changed value POST-NOTIFY = the VC re-ran its own RC bring-up and does not
///     use our windows; an unchanged value = it rides ours, so ours must be correct)?
///   * VL805 config id[0x00]/COMMAND[0x04]/BAR0[0x10..0x14] — did the device change state (a fw load
///     resets the VL805's config space; id going live, or BAR0/COMMAND reverting to power-on defaults,
///     witnesses that the reset/load actually reached the silicon vs. a handler that ran into a dead
///     downstream path and loaded nothing).
/// Read-only. Child-config reads are HARD-GATED on PHYLINKUP && DL_ACTIVE (the boot-P4 link-down-MMIO-
/// stalls-the-bus law); with the link down it prints the RC register block only and returns.
fn witness_notify_boundary(ctx: &str, label: &str) {
    let swinit = r(RC_BASE + PCIE_RGR1_SW_INIT_1);
    let status = r(RC_BASE + PCIE_MISC_PCIE_STATUS);
    let phylinkup = status & PCIE_MISC_PCIE_STATUS_PHYLINKUP != 0;
    let dl_active = status & PCIE_MISC_PCIE_STATUS_DL_ACTIVE != 0;
    let buses = r(RC_BASE + 0x18);
    let memwin = r(RC_BASE + 0x20);
    let rp_cmd = r(RC_BASE + 0x04) & 0xffff;
    serial_println!(
        "{}   NOTIFY-{} ({}): RGR1={:#010x} PCIE_STATUS={:#010x} (PHYLINKUP={} DL_ACTIVE={}) | RP bus[0x18]={:#010x} mem[0x20]={:#010x} CMD[0x04]={:#06x} (MEM={} BM={}) ::",
        P, label, ctx, swinit, status, phylinkup, dl_active, buses, memwin, rp_cmd, (rp_cmd >> 1) & 1, (rp_cmd >> 2) & 1
    );
    if is_poison(status) || !(phylinkup && dl_active) {
        serial_println!(
            "{}   NOTIFY-{} ({}): link NOT up (PHYLINKUP={} DL_ACTIVE={}) — skipping child-config read (link-down MMIO stalls the bus, boot-P4) ::",
            P, label, ctx, phylinkup, dl_active
        );
        return;
    }
    let id = vl805_cfg_read(0x00);
    let cmd = vl805_cfg_read(0x04) & 0xffff;
    let bar0_lo = vl805_cfg_read(0x10);
    let bar0_hi = vl805_cfg_read(0x14);
    match live_vendor_device(id) {
        Some((v, d)) => serial_println!(
            "{}   NOTIFY-{} ({}): VL805 cfg id={:#010x} (vendor={:#06x} device={:#06x}) COMMAND={:#06x} (MEM={} BM={}) BAR0=[{:#010x} {:#010x}] ::",
            P, label, ctx, id, v, d, cmd, (cmd >> 1) & 1, (cmd >> 2) & 1, bar0_lo, bar0_hi
        ),
        None => serial_println!(
            "{}   NOTIFY-{} ({}): VL805 cfg id={:#010x} — child config not answering (device silent / forwarding down) BAR0=[{:#010x} {:#010x}] ::",
            P, label, ctx, id, bar0_lo, bar0_hi
        ),
    }
}

// ─── PIUSB-19: link speed/width witness (candidate-(a) pre-arm). ────────────────────────────────────
//
// The surviving suspect after PIUSB-18 (which enabled the RC inbound SCB master — candidate (b)) is
// candidate (a): the PCIe link geometry at NOTIFY time. Linux's brcmstb reads the STANDARD PCI Express
// capability's Link Status register (LNKSTA) and logs negotiated speed/width; some VL805 units need the
// link at a particular Gen (or a retrain) before their internal 8051 firmware engine behaves and clears
// USBSTS.CNR. This witness READS the Express capability (LNKCAP / LNKSTA / LNKCTL2) on BOTH the BCM2711
// root complex (bus0/dev0, config at RC_BASE) and the VL805 endpoint (bus1/dev0, via the EXT_CFG child
// window), decodes negotiated speed (Gen1/Gen2) + width (x1), prints the `link Gen<N> x<W> (rc) /
// Gen<N> x<W> (ep)` verdict template, and FLAGS any side whose negotiated geometry falls short of its
// own LNKCAP maximum (the candidate-(a) signature). It is strictly READ-ONLY — no retrain / LNKCTL2 /
// speed writes this arc. The EP side is HARD-GATED on PHYLINKUP && DL_ACTIVE (a link-down child-config
// MMIO stalls the bus — boot-P4); the RC-side Express cap is in the RC's own register block (always
// safe to read even link-down).

/// PCI Express capability id (standard PCI capability list).
const PCI_CAP_ID_EXPRESS: u32 = 0x10;
/// Offsets WITHIN the PCI Express capability structure (from the capability's base offset).
const EXP_LNKCAP: u64 = 0x0C; // Link Capabilities (32-bit): [3:0] max speed, [9:4] max width
const EXP_LNKCTL_STA: u64 = 0x10; // Link Control [15:0] + Link Status (LNKSTA) [31:16]
const EXP_LNKCTL2_STA2: u64 = 0x30; // Link Control 2 (LNKCTL2) [15:0] + Link Status 2 [31:16]

/// Walk a config space's PCI capability list to the PCI Express capability; return its byte offset.
/// `rd` reads a 32-bit dword at a byte offset into the target's config space (dword-aligned). Bounded
/// (<=48 links) and poison-guarded — a non-answering config space (poison id) returns None without a
/// walk. RC config uses `|off| r(RC_BASE+off)`; the EP uses `vl805_cfg_read` (EXT_CFG child window).
fn find_express_cap(rd: &dyn Fn(u64) -> u32) -> Option<u64> {
    // A config space that does not answer (poison vendor:device) has no walkable cap list.
    if is_poison(rd(0x00)) {
        return None;
    }
    // Status register (bits [31:16] of the 0x04 dword): bit 4 = Capabilities List present.
    let status = rd(0x04) >> 16;
    if status & (1 << 4) == 0 {
        return None;
    }
    // Capabilities Pointer: low byte of the 0x34 dword.
    let mut ptr = (rd(0x34) & 0xff) as u64 & !0x3;
    let mut guard = 0u32;
    while ptr >= 0x40 && ptr != 0xfc && guard < 48 {
        guard += 1;
        let hdr = rd(ptr); // cap-id = byte0, next-ptr = byte1 (ptr is dword-aligned)
        if is_poison(hdr) {
            return None;
        }
        if (hdr & 0xff) == PCI_CAP_ID_EXPRESS {
            return Some(ptr);
        }
        ptr = ((hdr >> 8) & 0xff) as u64 & !0x3;
    }
    None
}

/// Decode + print one side's Express-capability link geometry. Returns
/// `(cur_speed_gen, cur_width, cap_speed_gen, cap_width)` so the caller can build the verdict and flag a
/// short-of-LNKCAP negotiation. Speed codes are the PCIe encoding (1=Gen1/2.5GT/s, 2=Gen2/5GT/s,
/// 3=Gen3/8GT/s, ...), so the Gen number equals the code. Read-only.
fn decode_link_geometry(rd: &dyn Fn(u64) -> u32, cap: u64, side: &str) -> Option<(u32, u32, u32, u32)> {
    let lnkcap = rd(cap + EXP_LNKCAP);
    let lnkctl_sta = rd(cap + EXP_LNKCTL_STA);
    let lnkctl2 = rd(cap + EXP_LNKCTL2_STA2) & 0xffff;
    if is_poison(lnkcap) || is_poison(lnkctl_sta) {
        serial_println!("{}   PIUSB-19 {}: Express-cap link regs read poison (LNKCAP={:#010x} LNKCTL/STA={:#010x}) — not answering ::", P, side, lnkcap, lnkctl_sta);
        return None;
    }
    let cap_speed = lnkcap & 0xf; // max link speed (Gen N)
    let cap_width = (lnkcap >> 4) & 0x3f; // max link width
    let lnksta = (lnkctl_sta >> 16) & 0xffff;
    let cur_speed = lnksta & 0xf; // negotiated (current) link speed
    let cur_width = (lnksta >> 4) & 0x3f; // negotiated link width
    let target_speed = lnkctl2 & 0xf; // LNKCTL2 target link speed (retrain target)
    serial_println!(
        "{}   PIUSB-19 {}: cap@{:#x} LNKCAP={:#010x} (maxGen{} x{}) LNKSTA={:#06x} (curGen{} x{}) LNKCTL2={:#06x} (targetGen{}) ::",
        P, side, cap, lnkcap, cap_speed, cap_width, lnksta, cur_speed, cur_width, lnkctl2, target_speed
    );
    Some((cur_speed, cur_width, cap_speed, cap_width))
}

/// The PIUSB-19 witness. Prints the RC + VL805 link geometry + the EP identity snapshot at `ctx`, then a
/// single `PIUSB-19 (<ctx>) VERDICT: link Gen<N> x<W> (rc) / Gen<N> x<W> (ep) — <flag>` line. READ-ONLY:
/// no retrain, no LNKCTL2/speed writes. EP-side reads are gated on link-up (boot-P4); any latent async
/// abort from a child-config read into an unforwarded/absent EP is drained before return.
fn witness_link_speed_width(ctx: &str) {
    let status = r(RC_BASE + PCIE_MISC_PCIE_STATUS);
    let phylinkup = status & PCIE_MISC_PCIE_STATUS_PHYLINKUP != 0;
    let dl_active = status & PCIE_MISC_PCIE_STATUS_DL_ACTIVE != 0;

    // ── RC side (bus0/dev0): config at RC_BASE, the RC's own always-safe register block. ──
    let rc_rd = |off: u64| r(RC_BASE + off);
    let rc = find_express_cap(&rc_rd).and_then(|cap| decode_link_geometry(&rc_rd, cap, "rc(bus0)"));
    if rc.is_none() {
        serial_println!("{}   PIUSB-19 ({}): RC Express capability not found (no cap list / cap absent) ::", P, ctx);
    }

    // ── EP side (bus1/dev0): only when the link is up (link-down child MMIO stalls the bus). ──
    let ep = if is_poison(status) || !(phylinkup && dl_active) {
        serial_println!(
            "{}   PIUSB-19 ({}): link NOT up (PHYLINKUP={} DL_ACTIVE={}) — RC side only; skipping EP child-config (boot-P4 link-down-MMIO law) ::",
            P, ctx, phylinkup, dl_active
        );
        None
    } else {
        let ep_rd = |off: u64| vl805_cfg_read(off);
        // EP identity + decode-state snapshot at this exact moment (VID/DID, COMMAND, BAR0, class/rev).
        let id = ep_rd(0x00);
        let cmd = ep_rd(0x04) & 0xffff;
        let classrev = ep_rd(0x08);
        let bar0_lo = ep_rd(0x10);
        let bar0_hi = ep_rd(0x14);
        match live_vendor_device(id) {
            Some((v, d)) => serial_println!(
                "{}   PIUSB-19 ({}) EP VL805 cfg: id={:#010x} (vendor={:#06x} device={:#06x}) COMMAND={:#06x} (MEM={} BM={}) BAR0=[{:#010x} {:#010x}] class={:#04x}/{:#04x}/{:#04x} rev={:#04x} ::",
                P, ctx, id, v, d, cmd, (cmd >> 1) & 1, (cmd >> 2) & 1, bar0_lo, bar0_hi,
                (classrev >> 24) & 0xff, (classrev >> 16) & 0xff, (classrev >> 8) & 0xff, classrev & 0xff
            ),
            None => serial_println!(
                "{}   PIUSB-19 ({}) EP VL805 cfg: id={:#010x} — child config not answering (forwarding not yet set / device silent) BAR0=[{:#010x} {:#010x}] ::",
                P, ctx, id, bar0_lo, bar0_hi
            ),
        }
        let ep = find_express_cap(&ep_rd).and_then(|cap| decode_link_geometry(&ep_rd, cap, "ep(bus1)"));
        if ep.is_none() {
            serial_println!("{}   PIUSB-19 ({}): EP Express capability not found (config not forwarded / cap absent) ::", P, ctx);
        }
        // A child-config read into an unforwarded/absent EP can leave a latent async external abort (the
        // 0xdeaddead class); drain it here so it never outlives this read-only witness.
        super::exceptions::serror_drain_request("piusb: PIUSB-19 EP witness");
        ep
    };

    // ── Verdict template + short-of-LNKCAP flag (the candidate-(a) signature). ──
    let (rgen, rwid, rcapgen, rcapwid) = rc.unwrap_or((0, 0, 0, 0));
    let (egen, ewid, ecapgen, ecapwid) = ep.unwrap_or((0, 0, 0, 0));
    let rc_short = rc.is_some() && (rgen < rcapgen || rwid < rcapwid);
    let ep_short = ep.is_some() && (egen < ecapgen || ewid < ecapwid);
    serial_println!(
        "{}   PIUSB-19 ({}) VERDICT: link Gen{} x{} (rc) / Gen{} x{} (ep) — {} ::",
        P, ctx, rgen, rwid, egen, ewid,
        if rc_short || ep_short {
            "NEGOTIATED BELOW LNKCAP (candidate-(a) INDICATED: link short of its max Gen/width — a retrain to the LNKCAP geometry may be needed before the VL805 8051 behaves)"
        } else if rc.is_some() && ep.is_some() {
            "negotiated at LNKCAP max on both sides (candidate-(a) NOT indicated by speed/width — the link geometry is not the CNR wall)"
        } else if rc.is_some() {
            "RC side only (EP Express cap unread — link down or forwarding not set at this ctx); RC geometry is the record"
        } else {
            "incomplete (neither side's Express cap read)"
        }
    );
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
///
/// PIUSB-43 — WHY THIS DUMP IS NOW KNOB-GATED. P60 is the first real cold-boot measurement of the
/// bracket chain, and it indicts this function, not the bring-up: total PCIe/VL805 bringup 141.8s, of
/// which `stage=fw-state-dump took=129688ms` and (nested inside it) `stage=dump-linkdown-rc-claim
/// took=32410ms`, while every FUNCTIONAL stage is fast (m1-rc-bringup 340ms, m2-enumerate-vl805 564ms,
/// m3-attach-xhci 332ms, xhci handoffs 18/21ms — no 2.8s multiple, so the budget-escalation theory is
/// refuted). Arithmetic names the mechanism: this path performs 13 bare RC-APB reads (10 here + 3 in
/// `witness_rc_cpu_claim`) and 129.7s/13 ≈ 10s PER READ. When the firmware hands us the RC still held
/// in reset (RGR1_SW_INIT_1=0x3 = INIT_GENERIC|PERST, PHYLINKUP=false) the RC's OWN APB block does not
/// answer promptly either — each 32-bit load is absorbed by the bridge's completion-timeout machinery
/// and stalls the CPU for ~10s. These are bare `r()` loads, so `MMIO_LADDER_BUDGET_MS` (60 ms) does not
/// apply to them: that wall only caps `mmio_settle_read` ladders. The cost is therefore LINEAR IN THE
/// NUMBER OF REGISTERS WE CHOOSE TO DUMP — and a dump of a confirmed family is exactly what the
/// default-quiet-boot law says must not run by default. So: the full register dump and the RC-claim
/// witness now require the `usbdebug` feature (`UNAOS_USBDEBUG=1`); the default path reads only the TWO
/// registers the link-down gate genuinely needs (RGR1_SW_INIT_1 + PCIE_STATUS — the gate decision and
/// PIUSB-16's discriminator are both driven off them) and prints ONE summary line. Nothing functional
/// changed: no bring-up step, wait, ordering or protection is touched, the gate is the same gate, and
/// every `[piusb40]` bracket still emits `stage=… took=…` so the chain stays gap-free.
///
/// PIUSB-42 — RETURNS `(RGR1_SW_INIT_1, PCIE_MISC_PCIE_STATUS)`, the two gate registers this function
/// reads unconditionally on EVERY path (including both early returns, and in BOTH modes — the pair is
/// on the always-read path above the `deep` gate, so the knob cannot change what is threaded out). It
/// returns them because the caller's PIUSB-16 entry-link discriminator needs exactly these two values
/// and used to re-read them microseconds later. On the cold-boot link-down path each bare RC-APB read
/// costs ~10s (PIUSB-43's measurement: the RC is held in reset and the load is absorbed by the bridge's
/// completion-timeout machinery), so that duplicate pair was pure waste — worth AT MOST ~10.9s, which is
/// all P60's total leaves for it once the dump and the functional stages are subtracted (a uniform
/// 2 × ~10s extrapolation would say ~21s, but that exceeds the residual; see 5h). Threading them out is
/// behaviour-preserving by construction — see the discriminator's own note on why the staleness is
/// sound; the longer DEEP-mode dwell between the read and the discriminator does not weaken that
/// argument, which turns on there being no WRITER in between, not on how little time passes.
fn dump_firmware_state() -> (u32, u32) {
    // The knob. `usbdebug` is the established USB-diagnostics feature (`UNAOS_USBDEBUG=1 ./arroyo …`).
    let deep = cfg!(feature = "usbdebug");
    serial_println!(
        "{} PIUSB-5: pre-reset firmware reference dump (read-only; BEFORE any RC reset) — mode={} ::",
        P,
        if deep { "DEEP (usbdebug: full RC register dump + RC-claim witness)" } else { "SUMMARY (default-quiet: 2 gate registers only; build UNAOS_USBDEBUG=1 for the full dump — PIUSB-43: each RC-APB read costs ~10s while the fw holds the RC in reset)" }
    );

    // (1) The TWO registers the gate needs, always. These are the RC's OWN register block (the
    //     0xFD50_0000 MMIO the CPU always claims) — safe to read link-down, just slow when the RC is
    //     held in reset.
    // PIUSB-31 pinpoint witness: metal P40 printed the PIUSB-5 header (above) then wedged silently
    // with no further line and no MAILBOX-timeout — an UNBOUNDED CPU bus stall, not the (timeout-
    // guarded) mailbox. The very first RC-register MMIO read is the prime suspect: safe in P38's
    // pre-SMP pre-V3D single-threaded boot context, wedging in this deferred post-GUI context. This
    // enter/exit pair makes P41 say precisely whether the wall is the first RC read.
    serial_println!("{} piusb31: RC-reg-read enter (RGR1_SW_INIT_1 @ {:#x}) ::", P, RC_BASE + PCIE_RGR1_SW_INIT_1);
    let swinit = r(RC_BASE + PCIE_RGR1_SW_INIT_1);
    let status = r(RC_BASE + PCIE_MISC_PCIE_STATUS);
    serial_println!("{} piusb31: RC-reg-read exit (2 gate registers read OK) ::", P);
    let phylinkup = status & PCIE_MISC_PCIE_STATUS_PHYLINKUP != 0;
    let dl_active = status & PCIE_MISC_PCIE_STATUS_DL_ACTIVE != 0;
    serial_println!(
        "{}   fw RC: RGR1_SW_INIT_1={:#010x} PCIE_STATUS={:#010x} (PHYLINKUP={} DL_ACTIVE={}) ::",
        P, swinit, status, phylinkup, dl_active
    );

    // (1b) The other EIGHT RC-own registers — the bus/window/BAR2 record. Diagnostic only: nothing
    //      below reads these, and M1 reprograms every one of them from scratch moments later. At
    //      ~10s per absorbed read on the cold-boot link-down path they are ~80s of the P60 pause,
    //      so they run only under the knob.
    if deep {
        let buses = r(RC_BASE + 0x18);
        let win_lo = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LO);
        let win_hi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_HI);
        let win_bl = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT);
        let win_bhi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI);
        let win_lhi = r(RC_BASE + PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI);
        let bar2_lo = r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_LO);
        let bar2_hi = r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_HI);
        serial_println!("{}   fw RC (deep): bus_window[0x18]={:#010x} ::", P, buses);
        serial_println!(
            "{}   fw WIN0: LO={:#010x} HI={:#010x} BASE_LIMIT={:#010x} BASE_HI={:#010x} LIMIT_HI={:#010x} | RC_BAR2 LO={:#010x} HI={:#010x} ::",
            P, win_lo, win_hi, win_bl, win_bhi, win_lhi, bar2_lo, bar2_hi
        );
    }

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
        // ── PIUSB-40 WEDGE AUDIT (the P59a full wedge). Metal P59a died with the wire silent immediately
        //    after the line above and was power-cycled; the brief asked whether this fall-through performs
        //    an MMIO the skip was supposed to avoid. AUDIT RESULT: it does not. Everything below this
        //    point touches only the RC's OWN register block (`witness_rc_cpu_claim` reads MISC_CTRL +
        //    RC_BAR2_CONFIG_LO/HI) — in DEEP mode the same MISC block the ten reads above already answered
        //    (2 piusb31 gate reads + 8 deep diagnostic reads); in SUMMARY mode only 2 gate registers, and
        //    witness_rc_cpu_claim is skipped. The reads are bracketed by the `piusb31` enter/exit pair that
        //    printed. No child config, no outbound-window memory cycle, no mailbox. So the wedge is NOT a
        //    mis-skipped downstream MMIO on this path.
        //    What is left unproven is WHICH of the three remaining candidates ate the boot: the
        //    MISC_CTRL/RC_BAR2 reads, the SError drain window, or the caller's next stage (M1). A stopwatch
        //    cannot separate them and neither could P59a's log, because none of them printed anything.
        //    These three brackets make the next boot name it: whichever `[piusb40]` line is MISSING is the
        //    stage that wedged. They are pure counter reads + serial — they add no bus traffic of their own.
        //    PIUSB-43: P60 finally timed this witness — `stage=dump-linkdown-rc-claim took=32410ms`
        //    for its THREE RC-APB reads (MISC_CTRL + RC_BAR2_CONFIG_LO/HI), ~10.8s each, matching the
        //    per-read cost the enclosing dump shows. It is a pure diagnostic (read-only; M1 reprograms
        //    MISC_CTRL/RC_BAR2 itself and proves the result with its own readback line), and its family
        //    is confirmed — on the fw-left-RC-in-reset path it has only ever reported the firmware's
        //    teardown. So it runs under the knob only. The BRACKET stays unconditional: the chain must
        //    remain gap-free, and a near-zero `took=` here is itself the evidence that the stage was
        //    skipped rather than wedged.
        let t_w = stage_begin();
        if deep {
            witness_rc_cpu_claim();
        } else {
            serial_println!(
                "{}   RC-claim witness SKIPPED (default-quiet): 3 RC-APB reads cost ~32s on this path (P60) and only re-report the firmware teardown — build UNAOS_USBDEBUG=1 to take it ::",
                P
            );
        }
        let t_w = stage_end("dump-linkdown-rc-claim", t_w);
        // Drain any latent async abort the RC-register reads could have set (the R22 sitting-2 class);
        // this dump runs BEFORE M1's reset, so nothing latent may survive into the reset sequence.
        super::exceptions::serror_drain_request("piusb: PIUSB-5 dump link-down");
        stage_end("dump-linkdown-serror-drain", t_w);
        return (swinit, status);
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
    // PIUSB-31 pinpoint witness: map_device_1gib documents pre-SMP single-threaded use only (its live
    // page-table block install is break-before-make-hazardous once the APs are translating). This
    // dump now runs post-SMP, so bracket the map + the first CAP read (a memory cycle into the
    // outbound window) — the second-stage stall candidate if the RC-reg reads above pass.
    serial_println!("{} piusb31: map_device_1gib enter (@ {:#x}, post-SMP) ::", P, cap_cpu_base);
    unsafe { super::boot::map_device_1gib(cap_cpu_base) };
    serial_println!("{} piusb31: map_device_1gib exit; CAP-read enter ::", P);
    // PIUSB-40: cost-aware ladder (was a fixed 4-try / 2 ms-settle loop). See `mmio_settle_read`.
    // NOTE: folding this site into the shared helper changed its inter-try settle 2 ms -> 5 ms (the
    // helper's single value, matching the other two ladders). Harmless and deliberate: this is a
    // "has the decode woken up" poll, not a mandated settle, its worst case is 4 tries, and the whole
    // ladder is capped by MMIO_LADDER_BUDGET_MS regardless. Recorded so the change is not silent.
    let (cap0, tries, ladder_ms, _bailed) = mmio_settle_read(cap_cpu_base, 4);
    if is_poison(cap0) || cap0 == 0 {
        serial_println!(
            "{}   fw CAP probe @ {:#x} = {:#010x} after {} tries ({}ms) — the firmware-left state does not decode memory cycles here; the wall is upstream of the BAR ::",
            P, cap_cpu_base, cap0, tries, ladder_ms
        );
        witness_rc_cpu_claim();
        super::exceptions::serror_drain_request("piusb: PIUSB-5 dump poison");
        return (swinit, status);
    }
    let cap_length = (cap0 & 0xff) as u8;
    let hci_version = (cap0 >> 16) as u16;
    let hcsparams1 = r(cap_cpu_base + 0x04);
    serial_println!(
        "{}   fw CAP probe @ {:#x} = {:#010x} LIVE — firmware-left xHC IS DECODING: CAPLENGTH={} HCIVERSION={:#06x} HCSPARAMS1={:#010x} (record only — adopt retired, we still reset) ::",
        P, cap_cpu_base, cap0, cap_length, hci_version, hcsparams1
    );
    super::exceptions::serror_drain_request("piusb: PIUSB-5 dump exit");
    (swinit, status)
}

/// PIUSB-29: firmware DTB pointer stashed by `bringup` (the pre-heap `build_boot_info` call) for the
/// async bring-up task to read. The RC bring-up + enumeration no longer run on the boot-critical path;
/// `bringup` now only records the DTB and returns immediately so `kernel_main` reaches the GUI/panel
/// without waiting on the multi-second brcmstb RC reset/PERST/CNR settle + the bounded enumerate pump.
static PIUSB_DTB: AtomicU64 = AtomicU64::new(0);

/// PIUSB-29: entry point kept for `build_boot_info` (boot.rs) — now a zero-cost DTB stash. The real RC
/// bring-up + enumeration is deferred to `bringup_task`, spawned from `kernel_main` onto a secondary
/// core so the boot core proceeds straight to the GUI/panel. Called once on the BSP, pre-heap/pre-SMP;
/// stashing an AtomicU64 is heap-free and safe there. Off the boot path = the panel unblocks at once.
pub fn bringup(dtb: u64) {
    PIUSB_DTB.store(dtb, Ordering::Release);
    // PIUSB-33: the SPLIT bring-up — run the RC + xHCI HARDWARE bring-up HERE, in the early P38-proven
    // single-threaded pre-V3D/pre-GENET/pre-panel context (tail of `build_boot_info`: pre-heap, pre-SMP,
    // IRQs masked, panel not yet live). Metal evidence (this sitting): P39/P40/P41 — the very first RC APB
    // read (RGR1_SW_INIT_1 @ 0xfd509210) HARD-STALLS on ANY core in the deferred post-panel context; P43 —
    // the early-vs-deferred power/clock census is IDENTICAL (firmware power/clock exonerated; the DTB
    // carries no clocks/power-domains on `pcie@`); P38 — the FULL bring-up succeeds in exactly this early
    // context. Conclusion: the RC APB + xHCI controller bring-up must run in the early proven context;
    // only the heap-backed device enumeration / HID-and-storage walk stays deferred (`bringup_task` →
    // `enumerate`), because the heap is not up yet at this point in `build_boot_info` and enumeration
    // touches the xHCI BAR MMIO (not the stalling RC APB). `bringup_inner` is heap-free (it stops at the
    // "controller halted-but-decoding + ports powered" honesty line, allocating no rings) and CENSUS-
    // BEFORE-TOUCH: it reads the firmware DTB for a `pcie@` node FIRST and returns before ANY RC MMIO if
    // none is present, so QEMU raspi4b (models no PCIe RC, no `pcie@` node) skips cleanly. The early
    // power/clock census (kept — cheap, proven harmless) fires inside `bringup_inner`, right before the
    // first RC read. Cost is measured + witnessed so the honest panel-delay impact of putting the hardware
    // bring-up back on the early path is serial-visible.
    let t0 = ms_now();
    bringup_inner(dtb);
    if dtb_has_pcie(dtb) {
        let now = ms_now();
        serial_println!(
            ":: piusb33: early hw bring-up done (dur={}ms, t={}ms; RC reset + link train + xHCI init to ports-powered, on the early P38 path) ::",
            now.saturating_sub(t0), now
        );
    }
}

/// PIUSB-30: the deferred USB bring-up, run ONCE on the BOOT CORE (the BSP) from `kernel_main` AFTER the
/// GUI/input/render tasks are spawned onto the APs — so the panel is already live and painting while this
/// runs, but the RC/VL805 + VideoCore-mailbox sequence stays in its metal-proven single-threaded boot-core
/// context (P38). PIUSB-29 deferred this onto a SECONDARY core via `spawn_auto`; metal P39 proved that
/// wedges (the task stalled right after the DTB census, controller never published) — running RC MMIO +
/// the mailbox singleton concurrently with the GUI on an AP is not the proven context, so the AP placement
/// is retired. Runs ONCE: RC bring-up (`bringup_inner`, the brcmstb RC reset/PERST/CNR settle) → `enumerate`
/// (rings + RS=1 + the bounded polled walk) → returns, leaving steady-state servicing to `usb_pump`. A
/// keyboard/mouse plugged at power-on arms the same way, just slightly later (the devices do not vanish).
/// Brackets the work with a CNTPCT-derived millisecond witness so the panel-unblock win is serial-visible.
pub fn bringup_task(_arg: usize) {
    // PIUSB-33: the DEFERRED half of the split — enumeration ONLY. The RC + xHCI hardware bring-up
    // (`bringup_inner`) already ran EARLY, in `build_boot_info`'s P38-proven context (the RC APB read
    // that P39/P40/P41 proved hard-stalls in this deferred post-panel context never happens here — the
    // controller is already halted-but-decoding with its ports powered, `XHCI_READY` set). All that
    // remains is the heap-backed DMA-side walk: rings/interrupter, RS=1, port-connect pump, ADDRESS_DEVICE
    // and the HID/mass-storage enumeration — which touches the xHCI BAR MMIO (not the stalling RC APB) and
    // needs the heap, so it stays here on the BSP after the GUI/panel tasks are spawned. `enumerate`
    // self-gates on `XHCI_READY`: if the early bring-up did not reach the honesty line (QEMU raspi4b
    // census-skip, link-down, BAR mismatch) it says so and returns.
    let t0 = ms_now();
    serial_println!(
        ":: piusb33: deferred enumerate start (early hw bring-up already reached the honesty line; panel already live on the APs at t={}ms) ::",
        t0
    );
    enumerate();
    let now = ms_now();
    serial_println!(
        ":: piusb33: deferred enumerate done (dur={}ms, t={}ms; DMA-side walk ran on the BSP, off the panel path) ::",
        now.saturating_sub(t0), now
    );
}

/// PIUSB-29: milliseconds since power-on off the always-live generic counter (CNTPCT/CNTFRQ). Used only
/// to timestamp the bring-up witness; a zero frequency (never on real silicon) degrades to 0 rather
/// than dividing by zero.
fn ms_now() -> u64 {
    let f = super::timer::cntfrq();
    if f == 0 {
        return 0;
    }
    super::timer::cntpct().saturating_mul(1000) / f
}

/// Bring the BCM2711 PCIe RC + VL805 xHCI up to the honesty line. PIUSB-33: runs EARLY, from `bringup`
/// in `build_boot_info` (pre-heap, pre-SMP, single-threaded — the P38-proven context in which the RC APB
/// read succeeds; the deferred post-panel context hard-stalls it, P39/P40/P41). Heap-free itself: the
/// attach stops at "halted-but-decoding + ports powered" and never allocates rings (`enumerate`, deferred
/// to `bringup_task`, does that next once the heap is up). `dtb` is the firmware device tree stashed by
/// `bringup` — censused for a `pcie@` node BEFORE any RC MMIO (QEMU raspi4b has none → clean skip). Every
/// wait is a FINITE CNTPCT backstop.
fn bringup_inner(dtb: u64) {
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

    // ── PIUSB-32/33: the power/clock census, RIGHT BEFORE the first RC MMIO (`dump_firmware_state`'s
    //    `piusb31` enter/read). As of PIUSB-33 `bringup_inner` runs EARLY (from `bringup`, in
    //    `build_boot_info`), so this census now fires in the P38-proven pre-V3D/pre-GENET/pre-panel
    //    context — the same context the (former) early census captured. It is kept because it is cheap and
    //    proven harmless (mailbox-only, timeout-bounded; it cannot be the source of the deferred hard
    //    stall — that stall is what PIUSB-33 designed out by moving the RC read here). P43 already showed
    //    the early-vs-deferred census IDENTICAL (firmware power/clock exonerated: the DTB carries no
    //    clocks/power-domains on `pcie@`), so this line is now steady-state instrumentation, not a
    //    diagnostic diff-target.
    // PIUSB-40: from here on every stage is bracketed. This point is PAST the DTB census, so a QEMU
    // raspi4b boot has already returned and emits no `[piusb40]` line at all.
    let t_total = stage_begin();
    let t = stage_begin();
    power_clock_census("early/bringup_inner (P38 context, right before the first RC read)");
    let t = stage_end("census-power-clock", t);

    // ── PIUSB-5/6: read-only reference dump FIRST (link-up-gated), reset SECOND. Capture the
    //    firmware-left state before M1 destroys it with the bridge/PERST reset. boot-P4 proved the
    //    firmware tears PCIe down before handoff (RC left fully in reset, no child config, no live
    //    decode), so the adopt path is retired — the dump is a record; we always take the reset path. ──
    //    PIUSB-42: the dump also HANDS BACK the two gate registers it reads unconditionally
    //    (RGR1_SW_INIT_1 + PCIE_STATUS) so the discriminator below does not pay for them twice.
    let (dump_swinit, dump_status) = dump_firmware_state();
    let t = stage_end("fw-state-dump", t);

    // ── PIUSB-16: the ENTRY link-state discriminator — the one-boot lever verdict. ────────────────
    //    Read the RC's own reset + status registers exactly as the firmware left them (both are safe
    //    RC-register reads even link-down). This is where the FW_TAG lever proves out in a single boot:
    //      * PHYLINKUP && DL_ACTIVE here  => the VideoCore ran its stock power-on PCIe/VL805 init and
    //        handed us a LIVE root complex (with the fw pushed on this EEPROM-less rev-1.5 board). Take
    //        the ADOPT path: NO M1 reset (it would tear the trained link + loaded fw down); keep the VC's
    //        outbound WIN0; run M2 (config/NOTIFY only — non-destructive to the link) then M3, and CNR is
    //        expected to be 0 at M3 because the controller was already loaded.
    //      * RC still in reset (RGR1 INIT_GENERIC|PERST set, PCIE_STATUS=0) => the new firmware still did
    //        NOT init PCIe before handoff — the FW_TAG swap was not the lever; take the COLD-BUILD path
    //        (M1 reset → M2 → M3) exactly as before, and the next lever is up for evaluation.
    //
    //    LENS FIX (3): these two RC reads sat in the BRACKET GAP between `fw-state-dump` and
    //    `m1-rc-bringup`. They are RC-own APB reads on a path we have just proven can wedge silently, so
    //    an unbracketed gap here would misattribute a wedge to M1 — exactly the misreading the P59a audit
    //    is trying to prevent. Bracketed, the chain is now gap-free from the census to the end of M3.
    //    LENS FIX (PIUSB-42): the two reads are GONE. They are now the values `dump_firmware_state`
    //    already read, handed back from the site it reads them at unconditionally. These values are
    //    STALE by however long the dump took — on the cold-boot link-down path that is seconds, not
    //    microseconds. That is sound, and provably so rather than by hope: RGR1_SW_INIT_1 and
    //    PCIE_STATUS are written by NOTHING between the two points. The dump is read-only by
    //    construction (its whole contract is "BEFORE any RC reset"), the only writer of
    //    RGR1_SW_INIT_1 in this file is M1, which runs strictly AFTER this discriminator and is
    //    gated ON its verdict, and PCIE_STATUS is read-only status the RC updates only in response
    //    to a link event M1 has not yet triggered. So the re-read could only ever have returned the
    //    same bits — while the firmware holds the RC in reset each absorbed RC-APB read runs ~10s
    //    (PIUSB-43), so this bought AT MOST ~10.9s of nothing: that is the whole residual P60's
    //    141.8s total leaves after the 129.688s dump and the 1.236s of m1/m2/m3, and it is SHARED
    //    with `census-power-clock`, which P60 never separated from it. Do not quote ~21s (the naive
    //    2 × ~10s) as measured — it exceeds that residual. This stage's own `took=`, which the
    //    bracket still emits, is what settles the split. The witness line below is unchanged and prints the
    //    same values it always did; only their provenance moved. The BRACKET stays: the chain must
    //    remain gap-free, and this stage's `took=` collapsing to ~0 is itself the evidence the
    //    duplication is gone.
    let entry_status = dump_status;
    let entry_swinit = dump_swinit;
    stage_end("entry-link-discriminator", t);
    let up_mask = PCIE_MISC_PCIE_STATUS_PHYLINKUP | PCIE_MISC_PCIE_STATUS_DL_ACTIVE;
    let link_up_at_entry = !is_poison(entry_status) && (entry_status & up_mask) == up_mask;
    serial_println!(
        "{} PIUSB-16: ENTRY link state — RGR1_SW_INIT_1={:#010x} PCIE_STATUS={:#010x} (PHYLINKUP={} DL_ACTIVE={}) -> {} ::",
        P, entry_swinit, entry_status,
        entry_status & PCIE_MISC_PCIE_STATUS_PHYLINKUP != 0,
        entry_status & PCIE_MISC_PCIE_STATUS_DL_ACTIVE != 0,
        if link_up_at_entry {
            "VC LEFT PCIe LIVE — ADOPT path (skip RC reset; keep VC windows + loaded VL805 fw; M2 config/NOTIFY → M3)"
        } else {
            "VC left RC in reset — COLD-BUILD path (M1 reset → M2 → M3); FW_TAG lever did not move VC init"
        }
    );

    // ── M1: brcmstb RC bring-up — COLD-BUILD path ONLY. On the ADOPT path we deliberately SKIP M1: its
    //    RGR1 INIT_GENERIC|PERST assert would reset the bridge and PERST the downstream link, tearing
    //    down the very link the VC trained and forcing a VL805 fw reload from cold (the wall this arc
    //    exists to avoid). ─────────────────────────────────────────────────────────────────────────────
    if !link_up_at_entry {
        let t_m1 = stage_begin();
        let m1_ok = m1_rc_bringup();
        stage_end("m1-rc-bringup", t_m1);
        if !m1_ok {
            serial_println!("{} M1 RC bring-up did not reach link-up — USB bring-up skipped (see lines above) ::", P);
            // SError-drain class rule (R22 sitting 2): the RC accesses above may have left a latent
            // async abort pending; a fail-closed exit must leave the machine clean.
            super::exceptions::serror_drain_request("piusb: M1 fail-closed");
            stage_end("bringup-total (M1 fail-closed)", t_total);
            return;
        }
    } else {
        serial_println!("{} PIUSB-16 ADOPT: skipping M1 RC reset — riding the VideoCore's live link + outbound window ::", P);
    }

    // ── M2: VL805 enumeration + BAR sizing + firmware reset. Config-space + mailbox only — it does NOT
    //    reset the PCIe link, so it is safe on BOTH paths (cold-built or adopted). ────────────────────
    let t_m2 = stage_begin();
    let m2 = m2_enumerate_vl805();
    stage_end("m2-enumerate-vl805", t_m2);
    let Some(bar0_pcie) = m2 else {
        serial_println!("{} M2 VL805 enumeration failed — USB bring-up skipped (see lines above) ::", P);
        // The poisoned downstream config read (`0xdeaddead`) is the R22 sitting-2 metal offender:
        // it left a pending async external abort that detonated at the first DAIF unmask. Drain.
        super::exceptions::serror_drain_request("piusb: M2 fail-closed (poisoned cfg read)");
        stage_end("bringup-total (M2 fail-closed)", t_total);
        return;
    };

    // ── M3: map the outbound window, attach the shared xHCI to the honesty line. ──────────────────
    let t_m3 = stage_begin();
    m3_attach_xhci(OUTBOUND_CPU_BASE, bar0_pcie);
    stage_end("m3-attach-xhci", t_m3);
    stage_end("bringup-total", t_total);

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

    // (a) Assert bridge core reset FIRST, then PERST — the exact Linux `brcm_pcie_setup` order
    //     (`brcm_pcie_bridge_sw_init_set(pcie, 1)` then `perst_set(pcie, 1)`), each a DISTINCT write, so
    //     the bridge core is quiesced before the downstream link is forced into fundamental reset.
    //     PIUSB-17: matched to the brcmstb reset structure exactly (was one combined write). PERST is
    //     held asserted from here through step (g) while we configure the windows — Linux does the same.
    let mut v = r(RC_BASE + PCIE_RGR1_SW_INIT_1) | PCIE_RGR1_SW_INIT_1_INIT_GENERIC;
    serial_println!("{}   >>> WRITE: RGR1_SW_INIT_1 |= INIT_GENERIC ({:#010x}) — assert bridge core reset (Linux step 1) ::", P, v);
    w(RC_BASE + PCIE_RGR1_SW_INIT_1, v);
    dsb();
    v = r(RC_BASE + PCIE_RGR1_SW_INIT_1) | PCIE_RGR1_SW_INIT_1_PERST;
    serial_println!("{}   >>> WRITE: RGR1_SW_INIT_1 |= PERST ({:#010x}) — assert downstream fundamental reset (Linux step 2) ::", P, v);
    w(RC_BASE + PCIE_RGR1_SW_INIT_1, v);
    dsb();
    settle_us(200); // Linux `usleep_range(100, 200)` while asserted; we hold a conservative 200 us

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
    // ── PIUSB-18: program the SCB0 window size AND the access-control bits Linux sets in the SAME
    //    MISC_CTRL write. Sizing SCB0 alone (our pre-18 code) armed the inbound WINDOW but never enabled
    //    the RC's SCB MASTER (bit12) — so the RC issued no inbound SCB cycles at all, and the VideoCore's
    //    VL805 firmware-load path (which P26/P28 proved is NOT our outbound BAR — the load must reach the
    //    device/its blob by the RC's own inbound/SCB route) had a closed door to SDRAM: CNR wedged. Set:
    //      * SCB0_SIZE   := 0x11 (4 GiB, unchanged — the window)
    //      * SCB_ACCESS_EN := 1  (the master enable — THE PIUSB-18 delta; bit12 was 0 on P28 metal)
    //      * CFG_READ_UR_MODE := 1 (bad cfg reads return UR, not an abort — Linux default; hardens the
    //                               downstream config path the notify handler walks)
    //      * MAX_BURST_SIZE := 0b00 (128B, the BCM2711 value)
    //    RMW: preserve every other MISC_CTRL bit (SCB1/2 sizes etc.).
    let misc_after = (misc_before
        & !(PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK | PCIE_MISC_MISC_CTRL_MAX_BURST_SIZE_MASK))
        | ((RC_BAR2_SIZE_4G << PCIE_MISC_MISC_CTRL_SCB0_SIZE_SHIFT) & PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK)
        | PCIE_MISC_MISC_CTRL_SCB_ACCESS_EN
        | PCIE_MISC_MISC_CTRL_CFG_READ_UR_MODE; // MAX_BURST_SIZE field left 0 (128B) by the clear above
    serial_println!(
        "{}   >>> WRITE: MISC_CTRL {:#010x} -> {:#010x} (SCB0_SIZE := {:#x} = 4 GiB + SCB_ACCESS_EN + CFG_READ_UR_MODE + burst=128B — PIUSB-18: enable the inbound SCB master, not just size the window) ::",
        P, misc_before, misc_after, RC_BAR2_SIZE_4G
    );
    w(RC_BASE + PCIE_MISC_MISC_CTRL, misc_after);
    dsb();
    let misc_rb = r(RC_BASE + PCIE_MISC_MISC_CTRL);
    let scb_access = misc_rb & PCIE_MISC_MISC_CTRL_SCB_ACCESS_EN != 0;
    serial_println!(
        "{}   MISC_CTRL readback = {:#010x} (SCB0_SIZE={:#x} SCB_ACCESS_EN={} CFG_READ_UR_MODE={}) -> {} ::",
        P, misc_rb, (misc_rb >> PCIE_MISC_MISC_CTRL_SCB0_SIZE_SHIFT) & 0x1f,
        scb_access as u8, (misc_rb & PCIE_MISC_MISC_CTRL_CFG_READ_UR_MODE != 0) as u8,
        if (misc_rb & (PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK | PCIE_MISC_MISC_CTRL_SCB_ACCESS_EN))
            == (misc_after & (PCIE_MISC_MISC_CTRL_SCB0_SIZE_MASK | PCIE_MISC_MISC_CTRL_SCB_ACCESS_EN))
        { "SIZED + SCB MASTER ENABLED" } else { "DID NOT LATCH" }
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
    // (e′) PIUSB-20 DECODE WITNESS — spell out the inbound match window + the applied cpu→pci offset.
    //     RC_BAR2 encoding (brcmstb `brcm_pcie_get_rc_bar2_size_and_offset`): the inbound MATCH ADDRESS is
    //     the LOWEST PCIe address in the DTB `dma-ranges`, split across CONFIG_LO[31:5] (low, size-code in
    //     [4:0]) and CONFIG_HI[31:0] (high). The device-visible cpu→pci translation offset is
    //     `pci_addr - cpu_addr` from that same `dma-ranges` entry.
    //
    //     AUTHORITATIVE dma-ranges (BCM2711, `bcm2711.dtsi` `pcie@7d500000`; QEMU raspi4b models no RC so
    //     there is no firmware DTB node to parse — the values are the fixed BCM2711 facts):
    //         dma-ranges = <0x02000000 0x0 0x00000000   0x0 0x00000000   0x4 0x00000000>;
    //                        \_flags_/ \__PCIe addr__/   \__CPU addr__/   \___size 16G__/
    //     PCIe base 0x0 == CPU base 0x0  ⇒  match address = 0, and the cpu→pci offset is 0 (1:1). So a host
    //     physical address is handed to the VL805 UNCHANGED as its bus address; DCBAAP/CRCR/ERST/TRB
    //     pointers need NO translation on BCM2711 (unlike the pre-C0 `0xc0000000` era or a Tegra-style
    //     offset). Any nonzero offset here would MIS-match the window and break the (already-working)
    //     inbound command fetch — so match-base 0 / offset 0 is the correct programming, confirmed by the
    //     live command-ring READ landing at boot P29. This witness makes offset==0 explicit per boot.
    let bar2_lo = r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_LO);
    let bar2_hi = r(RC_BASE + PCIE_MISC_RC_BAR2_CONFIG_HI);
    let match_base: u64 = ((bar2_hi as u64) << 32) | ((bar2_lo & !0x1f) as u64);
    let size_code = bar2_lo & 0x1f;
    // Decode size-code back to bytes (inverse of brcm_pcie_encode_ibar_size): [0x1c,0x1f]→4K..32K,
    // [0x01,0x14]→64K..32G, 0→disabled.
    let size_bytes: u64 = match size_code {
        0x1c..=0x1f => 1u64 << ((size_code - 0x1c) + 12),
        0x01..=0x14 => 1u64 << (size_code + 15),
        _ => 0,
    };
    const CPU_TO_PCI_OFFSET: u64 = 0; // BCM2711 dma-ranges: pci_addr(0) - cpu_addr(0) = 0 (1:1)
    serial_println!(
        "{}   PIUSB-20 RC_BAR2 DECODE: inbound match_base={:#x} size={:#x}B (code {:#x}) | dma-ranges cpu→pci offset={:#x} (BCM2711 1:1) — bus addrs handed to VL805 UNCHANGED ::",
        P, match_base, size_bytes, size_code, CPU_TO_PCI_OFFSET
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
    let perst_deassert_at = super::timer::cntpct();

    // (h) PIUSB-17 — THE DELTA. Mandatory fixed post-PERST-deassert settle BEFORE any link poll or
    //     downstream access. Linux `brcm_pcie_start_link()` does `msleep(PCIE_RESET_CONFIG_WAIT_MS)`
    //     (= 100 ms) UNCONDITIONALLY right after `perst_set(pcie, 0)`, and only THEN begins polling
    //     PHYLINKUP/DL_ACTIVE. This is the PCIe CEM T_PVPERL / device-ready window: the VL805's PCIe
    //     CORE trains the link within a few ms (so DL_ACTIVE flips early), but its internal 8051
    //     firmware engine needs the FULL fundamental-reset recovery before it is ready to be reset/loaded
    //     and to clear USBSTS.CNR. The prior M1 polled link-up the instant PERST deasserted and pressed
    //     straight into M2 (config → NOTIFY) as soon as DL_ACTIVE flipped — i.e. it touched the
    //     controller while the 8051 was still coming out of reset. That is the "MMIO live (PCIe core),
    //     state machine dead (CNR wedged)" signature this arc chases: link-trained ≠ device-ready.
    //     Wait the spec window first, exactly as Linux does. Bounded, finite (the anti-hang rule).
    //
    //     PIUSB-40 NOTE — this settle is NOT trimmable and was not trimmed. It is a Linux/PCIe-CEM
    //     mandated window (`PCIE_RESET_CONFIG_WAIT_MS`), and PIUSB-17 exists precisely because skipping it
    //     produced the CNR wall. It is bracketed so the metal log accounts for its 100 ms honestly rather
    //     than leaving it inside an unattributed gap.
    const PERST_DEASSERT_SETTLE_MS: u64 = 100; // Linux PCIE_RESET_CONFIG_WAIT_MS
    let t_perst = stage_begin();
    settle_ms(PERST_DEASSERT_SETTLE_MS);
    stage_end("m1-perst-settle", t_perst);
    serial_println!(
        "{}   PIUSB-17: waited {} ms post-PERST-deassert BEFORE link poll (Linux PCIE_RESET_CONFIG_WAIT_MS; device-ready/T_PVPERL window — the 8051 fw engine needs this even though the link trains sooner) ::",
        P, PERST_DEASSERT_SETTLE_MS
    );

    // (i) Poll link-up (PHYLINKUP + DL_ACTIVE) with a finite ~100 ms backstop (brcmstb polls every 5 ms
    //     for up to ~500 ms; we keep a bounded 100 ms budget ON TOP of the fixed settle above). An honest
    //     DOWN after the budget is a real result, not a hang.
    let up_mask = PCIE_MISC_PCIE_STATUS_PHYLINKUP | PCIE_MISC_PCIE_STATUS_DL_ACTIVE;
    let t_poll = stage_begin();
    let deadline = super::timer::cntpct() + super::timer::cntfrq() / 10; // ~100 ms
    let mut status = 0u32;
    let mut up = false;
    let mut link_up_at = 0u64;
    while super::timer::cntpct() < deadline {
        status = r(RC_BASE + PCIE_MISC_PCIE_STATUS);
        if !is_poison(status) && (status & up_mask) == up_mask {
            up = true;
            link_up_at = super::timer::cntpct();
            break;
        }
        core::hint::spin_loop();
    }
    stage_end("m1-link-train-poll", t_poll);
    if !up {
        serial_println!(
            "{} M1: link DOWN after training budget (PCIE_STATUS = {:#010x}; PHYLINKUP={} DL_ACTIVE={}) — honest hardware result, no device below the RC ::",
            P, status,
            status & PCIE_MISC_PCIE_STATUS_PHYLINKUP != 0,
            status & PCIE_MISC_PCIE_STATUS_DL_ACTIVE != 0
        );
        return false;
    }
    // PIUSB-17 discriminator: how long AFTER PERST-deassert did the link actually report up? This tells
    // one boot whether the link was already up when the fixed 100 ms settle expired (elapsed ~= settle,
    // link trained during the wait — the expected, healthy case) or only came up well after (a slow
    // train). `settle_ms` above already burned PERST_DEASSERT_SETTLE_MS, so `poll_elapsed` here is the
    // EXTRA time the poll loop spent beyond the fixed settle.
    let freq = super::timer::cntfrq().max(1);
    let total_since_perst_ms = link_up_at.wrapping_sub(perst_deassert_at) * 1000 / freq;
    let poll_elapsed_ms = link_up_at.wrapping_sub(perst_deassert_at + freq / 10).saturating_mul(1000) / freq;
    serial_println!(
        "{} M1: LINK UP (PCIE_STATUS = {:#010x}) — {} ms after PERST-deassert (fixed settle {} ms + {} ms of polling) ::",
        P, status, total_since_perst_ms, 100u64, poll_elapsed_ms
    );

    // PIUSB-19: witness the negotiated link speed/width AT M1 POST-LINK-UP (candidate-(a) pre-arm). The
    // RP bus window is not programmed until M2, so the EP child config is not yet forwarded here — this
    // captures the RC side's LNKSTA (the trained link geometry) as the earliest record; the EP side fills
    // in at the M2/M3 pre-NOTIFY witnesses once forwarding is up. Read-only.
    witness_link_speed_width("M1-post-linkup");

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
/// PCIe base, enable MEM decode + bus-master on the VL805, VERIFY an MMIO CAP[0] read decodes through
/// the new BAR, and ONLY THEN issue the NOTIFY_XHCI_RESET mailbox so the VideoCore firmware (re)loads
/// the VL805 firmware over that live BAR (PIUSB-15: boot-P25 showed the load needs an assigned BAR to
/// push through). Witnesses BAR0 retention across the notify and re-programs if the load reverts config
/// space. Returns the PCIe-side BAR0 base on success.
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

    // ── PIUSB-15 ADJUDICATION (boot-P25's boundary witness REVERSES the PIUSB-4 order) ─────────────────
    // PIUSB-4 concluded "NOTIFY first, THEN assign BAR — the fw reload wipes config space, so assigning
    // before is pointless." boot-P25 (PIUSB-14's real capture) CONFOUNDS that verdict, and it predates the
    // PIUSB-7 RP mem window + this boundary data so it is NOT binding:
    //   * At NOTIFY time the VL805 BAR0 read [0x00000004 0x00000000] — UNASSIGNED — because the prior code
    //     deferred BAR assignment until AFTER the notify.
    //   * The VC nonetheless REACHED the device and set VL805 COMMAND MEM+BM ITSELF (0x0000 -> 0x0146) —
    //     so the config-forwarding path was live at notify time — yet BAR0 stayed unassigned and CNR
    //     stayed 1 through the full wait. The controller was never loaded.
    // Derived conclusion (this arc acts on it): the VC's fw-load path writes the VL805 fw image THROUGH
    // the device's BAR0 (an MMIO push). With BAR0 unassigned there is no address to push through, so the
    // handler enables the COMMAND bits and EXITS WITHOUT LOADING. In Linux, PCI enumeration assigns the
    // VL805's BAR0 BEFORE `rpi_firmware_init_vl805`'s NOTIFY fires — we ARE the enumerator, so we mirror
    // that: SIZE + ASSIGN BAR0 into the RP mem window, ENABLE MEM+BM, VERIFY an MMIO CAP[0] read decodes
    // through the new BAR, and ONLY THEN issue NOTIFY. After NOTIFY we witness BAR0 RETENTION (the old
    // PIUSB-4 concern, now a witness rather than a reason to defer) and re-program if the load reverted
    // config space. The CNR wait (M3 / enumerate) remains the verdict gate.
    //
    // The dev_addr encoding (VL805_DEV_ADDR = (1<<20)|(0<<15)|(0<<12) = 0x0010_0000) matches the RPi
    // firmware property model — dev_id = (bus<<20)|(slot<<15)|(func<<12) for bus1/dev0/fn0.

    // ── BAR0 sizing ritual (all-ones probe + immediate restore; the ORIN-NET-3 pattern), BEFORE NOTIFY. ──
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

    // ── Assign BAR0 to the outbound window's PCIe base and enable decode — BEFORE NOTIFY (PIUSB-15). The
    //    VL805's xHCI MMIO decodes at PCIe 0xC000_0000, reachable by the CPU at OUTBOUND_CPU_BASE, which
    //    gives the VC's fw-load path a live MMIO target to push the firmware image through. ──
    let bar0_pcie = OUTBOUND_PCIE_BASE;
    serial_println!("{}   >>> WRITE: BAR0 := {:#x} (PCIe-side; CPU sees it at {:#x}) — assigned BEFORE NOTIFY (PIUSB-15) ::", P, bar0_pcie, OUTBOUND_CPU_BASE);
    let bar0_lo = (bar0_pcie & 0xFFFF_FFF0) as u32 | (orig & 0xf);
    vl805_cfg_write(0x10, bar0_lo);
    // ALWAYS write BAR0[0x14] explicitly. For our sub-4-GiB PCIe base (0xC000_0000) the high half is 0;
    // leaving it stale on a 64-bit BAR would make the device decode at <garbage>:0xC000_0000 — nowhere.
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

    // Command register (0x04): set MEM Space Enable (bit1) + Bus Master Enable (bit2). Enable decode after
    // the BAR is in place — a device must not decode before its BAR is assigned.
    let cmd = vl805_cfg_read(0x04);
    let newcmd = (cmd & 0xFFFF_0000) | ((cmd & 0xFFFF) | 0b110);
    serial_println!("{}   >>> WRITE: VL805 COMMAND {:#06x} -> {:#06x} (MEM+BusMaster enable, BEFORE NOTIFY) ::", P, cmd & 0xffff, newcmd & 0xffff);
    vl805_cfg_write(0x04, newcmd);
    let rb_cmd = vl805_cfg_read(0x04) & 0xffff;
    let cmd_ok = (rb_cmd & 0b110) == 0b110;
    serial_println!(
        "{}   COMMAND readback = {:#06x} (MEM={} BusMaster={}) -> {} ::",
        P, rb_cmd, (rb_cmd >> 1) & 1, (rb_cmd >> 2) & 1,
        if cmd_ok { "DECODE ENABLED" } else { "DECODE NOT ENABLED — MEM/BM bits did not set" }
    );
    settle_ms(10); // let decode settle before the MMIO verify read

    // ── PIUSB-15: VERIFY an MMIO CAP[0] read decodes through the freshly-assigned BAR, BEFORE NOTIFY.
    //    This is the arc's core precondition: the VC fw-load push needs a live MMIO address. Map the
    //    outbound window (idempotent with the reference dump / M3's map) and read CAP[0]. The xHCI
    //    capability registers are hardware-fixed and decode as soon as BAR0+MEM are set — they do NOT
    //    require the fw to be loaded (USBSTS.CNR tracks fw readiness separately), so a live CAP[0] here
    //    confirms the BAR is a real MMIO target. A poison read is recorded LOUDLY but does NOT abort:
    //    issuing the NOTIFY anyway is what captures the CNR verdict that discriminates the theories. ──
    unsafe { super::boot::map_device_1gib(OUTBOUND_CPU_BASE) };
    // PIUSB-40: cost-aware ladder (was a fixed 8-try / 5 ms-settle loop, i.e. a "40 ms" budget that on the
    // COLD-BUILD path could be eight serial RC completion timeouts instead). See `mmio_settle_read`.
    let t_verify = stage_begin();
    let (precap, ptries, ladder_ms, bailed) = mmio_settle_read(OUTBOUND_CPU_BASE, 8);
    stage_end("m2-mmio-verify", t_verify);
    if is_poison(precap) || precap == 0 {
        serial_println!(
            "{}   PIUSB-15 pre-NOTIFY MMIO verify: CAP[0]={:#010x} @ {:#x} after {} tries ({}ms{}) — BAR does NOT decode MMIO yet (theory (c): the load path may not be BAR-MMIO; proceeding to NOTIFY to capture the CNR verdict) ::",
            P, precap, OUTBOUND_CPU_BASE, ptries, ladder_ms,
            if bailed { ", ladder cut on per-read cost" } else { "" }
        );
    } else {
        serial_println!(
            "{}   PIUSB-15 pre-NOTIFY MMIO verify: CAP[0]={:#010x} @ {:#x} LIVE (CAPLENGTH={} HCIVERSION={:#06x}) — BAR decodes MMIO; the VC fw-load now has an address to push through ::",
            P, precap, OUTBOUND_CPU_BASE, precap & 0xff, (precap >> 16) as u16
        );
    }

    // PIUSB-14 boundary witness, immediately BEFORE the NOTIFY — now BAR0 is ASSIGNED at notify time
    // (the PIUSB-15 reversal). BAR0 in this line vs. the POST line below shows whether the fw load
    // reverts config space (theory (b)); the CNR wait shows whether the load actually ran (a)/(c).
    witness_notify_boundary("M2", "PRE");

    // PIUSB-19: witness link speed/width IMMEDIATELY BEFORE the M2 NOTIFY (candidate-(a)). Forwarding is
    // now set up (RP bus + mem windows programmed above), so both RC and EP Express caps read here.
    witness_link_speed_width("M2-pre-NOTIFY");

    serial_println!("{}   NOTIFY_XHCI_RESET (mailbox 0x00030058, dev_addr={:#x}) — VC (re)loads VL805 fw; BAR0 ASSIGNED+MMIO-verified FIRST (PIUSB-15: the fw push needs a live BAR) ::", P, VL805_DEV_ADDR);
    let n = mailbox::notify_xhci_reset(VL805_DEV_ADDR);
    witness_notify("M2", &n);
    // Let the VideoCore firmware finish loading/resetting the VL805 before we re-read its config space.
    settle_ms(50);

    // PIUSB-14: re-witness the SAME state AFTER the NOTIFY. RC/RP registers changed => the VC re-ran its
    // own bring-up (does not ride our windows); VL805 BAR0/COMMAND changed => the load reverted config
    // space (theory (b)).
    witness_notify_boundary("M2", "POST");

    // Device survival: a live vendor:device here = the device answered post-reset.
    let post = vl805_cfg_read(0x00);
    match live_vendor_device(post) {
        Some((v, d)) => serial_println!("{}   post-NOTIFY cfg[0x00] = {:#010x} (vendor={:#06x} device={:#06x} — device survived the fw reset, answering config) ::", P, post, v, d),
        None => {
            serial_println!("{}   post-NOTIFY cfg[0x00] = {:#010x} — device did NOT answer after the firmware reset (absent/forwarding lost) — bring-up STOP ::", P, post);
            return None;
        }
    }

    // PIUSB-15: BAR0 RETENTION across the fw load (the old PIUSB-4 concern, now a witness). If the load
    // reverted BAR0 to a power-on default, re-program BAR0+COMMAND so M3's CAP read + HCRST land on a
    // decoding controller (discrimination (b)).
    let post_bar = vl805_cfg_read(0x10);
    if post_bar != bar0_lo {
        serial_println!(
            "{}   PIUSB-15: BAR0 changed across NOTIFY ({:#010x} -> {:#010x}) — fw load reverted config space; re-programming BAR0+COMMAND (theory (b)) ::",
            P, bar0_lo, post_bar
        );
        reassert_vl805_decode("M2 post-notify");
    } else {
        serial_println!(
            "{}   PIUSB-15: BAR0 RETAINED across NOTIFY ({:#010x}) — fw load did not revert config space ::",
            P, post_bar
        );
    }

    Some(bar0_pcie)
}

/// PIUSB-12: decode + print one NOTIFY_XHCI_RESET response into the three status words that
/// discriminate the CNR-never-clears theories in a SINGLE boot. The prior witness (`rc={} echo={}`)
/// conflated "mailbox message processed" with "VL805 reset handler ran" and read the value echo as if
/// it were a load proof — but boot-P21 (rc=true echo=0x0) is fully consistent with the tag being
/// honoured (this command tag returns 0, it does NOT echo dev_addr). The load-vs-noop verdict lives in
/// the TAG response word, not the echo. This prints:
///   * `overall` (buffer word 1): 0x80000000 = message OK, 0x80000001 = malformed buffer.
///   * `tag_code` (buffer word 4): bit31 SET (e.g. 0x80000004) = firmware RAN the tag handler;
///     bit31 CLEAR = firmware IGNORED the tag (unknown/unsupported in this fw config) — a genuinely
///     different root cause than a handler that ran but the VL805 fw blob failed to load.
///   * `echo` (buffer word 5): we POST dev_addr (0x100000) here, so echo==0 also proves the reply
///     was re-fetched from RAM (cache-invalidate working) rather than served from our stale write.
/// One boot now separates: (T1) tag honoured, load ran → CNR wall is downstream (VL805 blob / link);
/// (T2) tag_code bit31 clear → firmware never ran the handler (tag/dev_addr/fw-config problem);
/// (T3) overall==0x80000001 → our message buffer is malformed.
fn witness_notify(ctx: &str, n: &mailbox::NotifyResp) {
    let honoured = n.tag_code & 0x8000_0000 != 0;
    let resp_len = n.tag_code & 0x7fff_ffff;
    serial_println!(
        "{}   NOTIFY ({}) rc={} overall={:#010x} tag_code={:#010x} (honoured={} resp_len={}) echo={:#010x} buf_pa={:#x} ::",
        P, ctx, n.ok, n.overall, n.tag_code, honoured, resp_len, n.echo, n.buf_pa
    );
    serial_println!(
        "{}   NOTIFY ({}) verdict: {} ::",
        P, ctx,
        if !n.ok {
            "MAILBOX FAILED — overall code not 0x80000000 (malformed buffer / no reply); NOTIFY did NOT run"
        } else if honoured {
            "tag HONOURED (firmware ran the VL805 reset/load handler) — if CNR still wedges the wall is DOWNSTREAM (VL805 fw blob absent in bootloader EEPROM, or link/BAR), NOT the NOTIFY"
        } else {
            "tag IGNORED (bit31 clear) — firmware did NOT run the handler: tag unsupported in this fw config, or dev_addr/topology mismatch — NOTIFY is the wall"
        }
    );
}

/// PIUSB-11: re-issue NOTIFY_XHCI_RESET to the VideoCore firmware IMMEDIATELY before a controller
/// halt+HCRST, then let the firmware settle. This mirrors Linux verbatim: `xhci_pci_common_probe`
/// (drivers/usb/host/xhci-pci.c) calls `reset_control_reset(reset)` — which on the Pi 4 lands in
/// `rpi_reset_reset` (drivers/reset/reset-raspberrypi.c), issuing this exact mailbox with
/// dev_addr=0x100000 then `usleep_range(200,1000)` — as the FIRST act of probe, right before
/// `usb_hcd_pci_probe` → `xhci_gen_setup` runs the xHCI HCRST. Linux therefore reloads the VL805
/// firmware on EVERY probe, immediately before every HCRST.
///
/// Our board is a Pi 4B rev 1.5 (boardrev d03115: 8 GB, Sony UK, BCM2711) — rev >= 1.4, so the VL805
/// has NO dedicated SPI EEPROM; its firmware is loaded from the main bootloader EEPROM by the
/// VideoCore, and this mailbox is the ONLY way to (re)load it. M1's PERST cycle (a PCIe fundamental
/// reset) is exactly the "PCI reset [that] resets the VL805 requiring the firmware to be reloaded"
/// the Linux reset driver documents — so after M1 the fw is DOWN and only NOTIFY brings it back.
///
/// The bug this fixes (boot-P18..P21): M2 issued a single early NOTIFY, then the controller was
/// HCRST'd TWICE with no intervening NOTIFY — once in M3 (pre-heap) and again in `enumerate` (post-
/// heap, SECONDS later after heap/AP/eMMC bring-up). A controller whose firmware is not running when
/// HCRST completes holds USBSTS.CNR=1 forever and silently drops every op/runtime write (P20:
/// USBSTS=0x811). Re-issuing NOTIFY right before each HCRST — Linux's placement — gives the firmware a
/// fresh, in-sequence reload so CNR can clear. PIUSB-10's CNR wait remains the verdict gate.
fn notify_before_reset(ctx: &str) {
    serial_println!("{}   NOTIFY (pre-reset, {}): mailbox 0x00030058 dev_addr={:#x} — reload VL805 fw immediately before halt+HCRST (Linux reset_control_reset placement) ::", P, ctx, VL805_DEV_ADDR);
    let n = mailbox::notify_xhci_reset(VL805_DEV_ADDR);
    witness_notify(ctx, &n);
    // Linux waits usleep_range(200, 1000) for VL805 startup after the reset mailbox; give it a
    // comfortably wider bounded settle so the fw is running before HCRST samples CNR.
    settle_ms(5);
    // The firmware (re)load resets the VL805's PCI config space (PIUSB-4: BAR0 -> 0, COMMAND MEM/BM
    // off) — so re-assert the decode path AFTER the reload, or the outbound-window CAP read that
    // follows would hit an unclaimed address (0xdeaddead) and HCRST would sample a non-decoding
    // controller. Idempotent: if this firmware revision preserves config across the reload, these
    // writes read back identical; if it clears it, they restore BAR0=OUTBOUND_PCIE_BASE + MEM/BM.
    reassert_vl805_decode(ctx);
}

/// Re-program the VL805's BAR0 to the outbound-window PCIe base and re-enable MEM-space + Bus-Master
/// decode, then witness the readback. Called after every pre-reset NOTIFY (the fw reload may revert
/// config space). Uses the EXT_CFG child window (same path M2 assigns the BAR through). The low BAR
/// flag bits are hardware-fixed type indicators (RO) — we read them and preserve them.
fn reassert_vl805_decode(ctx: &str) {
    let orig = vl805_cfg_read(0x10);
    if is_poison(orig) {
        serial_println!("{}   re-assert decode ({}): BAR0 reads poison {:#010x} — VL805 not answering config after reload; leaving as-is ::", P, ctx, orig);
        return;
    }
    let bar0_lo = (OUTBOUND_PCIE_BASE & 0xFFFF_FFF0) as u32 | (orig & 0xf);
    vl805_cfg_write(0x10, bar0_lo);
    vl805_cfg_write(0x14, (OUTBOUND_PCIE_BASE >> 32) as u32); // = 0 for 0xC000_0000
    let cmd = vl805_cfg_read(0x04);
    let newcmd = (cmd & 0xFFFF_0000) | ((cmd & 0xFFFF) | 0b110);
    vl805_cfg_write(0x04, newcmd);
    let rb_bar = vl805_cfg_read(0x10);
    let rb_cmd = vl805_cfg_read(0x04) & 0xffff;
    serial_println!(
        "{}   re-assert decode ({}): BAR0={:#010x} (wrote {:#010x}) COMMAND={:#06x} (MEM={} BM={}) -> {} ::",
        P, ctx, rb_bar, bar0_lo, rb_cmd, (rb_cmd >> 1) & 1, (rb_cmd >> 2) & 1,
        if rb_bar == bar0_lo && (rb_cmd & 0b110) == 0b110 { "DECODE RE-ARMED" } else { "MISMATCH — config did not latch post-reload" }
    );
    settle_ms(5);
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
    // PIUSB-40: cost-aware ladder (was a fixed 8-try / 5 ms-settle loop). See `mmio_settle_read`.
    const CAP_TRIES: u32 = 8;
    let t_cap = stage_begin();
    let (cap0, tries, ladder_ms, bailed) = mmio_settle_read(outbound_cpu_base, CAP_TRIES);
    stage_end("m3-cap-probe", t_cap);
    if is_poison(cap0) || cap0 == 0 {
        serial_println!(
            "{}   xHCI CAP register reads {:#010x} @ {:#x} after {} tries ({}ms{}) — BAR not decoding (window/BAR mismatch or firmware not loaded); attach SKIPPED, fail-closed ::",
            P, cap0, outbound_cpu_base, tries, ladder_ms,
            if bailed { ", ladder cut on per-read cost" } else { "" }
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
    // PIUSB-11: reload the VL805 firmware immediately before halt+HCRST (Linux places its
    // reset_control_reset() right before probe/HCRST). Without a fresh fw the HCRST completes into a
    // firmwareless controller and CNR wedges at 1.
    //
    // PIUSB-14: MMIO BAR0 sanity read across this pre-HCRST NOTIFY. Unlike the M2 boundary witness
    // (config-space only — the window is not yet mapped there), the outbound window IS mapped here, so
    // we read the controller's OWN xHCI registers through the BAR: CAP[0] (is it decoding?) and USBSTS
    // (CNR bit 11 — Not-Ready). If the fw load reaches the silicon, the post-NOTIFY USBSTS should show
    // CNR behaviour change (the fw drives CNR); an identical CAP/USBSTS across the NOTIFY = the load did
    // not alter the controller through this BAR path. `op_base` = CAP base + CAPLENGTH.
    let op_base_probe = outbound_cpu_base + cap_length as u64;
    let cap_pre = r(outbound_cpu_base);
    let usbsts_pre = r(op_base_probe + 0x04);
    serial_println!(
        "{}   NOTIFY-PRE (M3 MMIO): CAP[0]={:#010x} USBSTS={:#010x} (CNR={} HCH={}) @ op_base {:#x} ::",
        P, cap_pre, usbsts_pre, (usbsts_pre >> 11) & 1, usbsts_pre & 1, op_base_probe
    );
    // PIUSB-19: witness link speed/width IMMEDIATELY BEFORE the M3 pre-HCRST NOTIFY (candidate-(a)) — the
    // last read of the link geometry the VC's fw (re)load engine sees before the controller is HCRST'd.
    witness_link_speed_width("M3-pre-NOTIFY");
    let t_notify = stage_begin();
    notify_before_reset("M3 attach");
    stage_end("m3-notify", t_notify);
    let cap_post = r(outbound_cpu_base);
    let usbsts_post = r(op_base_probe + 0x04);
    serial_println!(
        "{}   NOTIFY-POST (M3 MMIO): CAP[0]={:#010x} USBSTS={:#010x} (CNR={} HCH={}) -> {} ::",
        P, cap_post, usbsts_post, (usbsts_post >> 11) & 1, usbsts_post & 1,
        if usbsts_post != usbsts_pre || cap_post != cap_pre {
            "controller state CHANGED across NOTIFY (fw load reached the silicon through the BAR)"
        } else {
            "controller state UNCHANGED across NOTIFY (fw load did NOT alter the controller via this BAR path)"
        }
    );
    // PIUSB-40: the xHCI handoff is the single biggest UNTRIMMABLE-FROM-HERE budget on this path —
    // `xhci::init` runs three `wait_until` waits (HCH=1, HCRST=0, CNR=0), each bounded by
    // `arch::hw_wait_budget()` = 150e6 CNTVCT ticks ≈ 2.8 s at the Pi's 54 MHz, so a controller whose
    // firmware never loaded pays ~8.3 s here. Those budgets live in `drivers/xhci` + `arch/aarch64/mod.rs`
    // (shared kernel-core, NOT this track's lane), so this arc MEASURES the stage rather than re-cutting
    // it: the metal line below says whether the handoff is a material share of the pause, which is the
    // evidence the owning lane would need before touching a shared timeout.
    let t_xhci = stage_begin();
    xhci::init(outbound_cpu_base);
    stage_end("m3-xhci-handoff", t_xhci);

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

/// PIUSB-13: the post-CNR enumeration observer. The full enumeration FSM (enable-slot →
/// ADDRESS_DEVICE → GET_DESCRIPTOR → SET_CONFIGURATION → SET_PROTOCOL(boot) → arm interrupt-IN →
/// boot-report decode → ASCII) lives in the shared `drivers/xhci` driver and is driven verbatim by
/// the pump below (`service_enum`/`service_hid_setproto`/`poll_events`). This struct adds no control
/// flow — it is a pure *observer* that snapshots the driver's read-only state each pump tick and
/// emits one `:: PIUSB: [enum] ... ::` milestone line per stage transition, so a single metal boot
/// localizes exactly how far a keyboard got and — on failure — the stage + completion code that
/// stopped it.
///
/// GATING (inert while CNR is stuck): `enumerate()` returns at the `XHCI_READY` gate in QEMU (no
/// controller built), and on metal with the VL805 CNR wall still up, RS=1 is dropped, so NO port
/// ever connects, NO enum stage ever advances, and NO slot is ever assigned — every edge test below
/// is a no-op and the observer stays silent. It only speaks once real enumeration makes progress.
struct EnumWitness {
    // Root-port edge state, indexed by xHCI port id (1..=max_ports; VL805 = 8). Index 0 unused.
    port_connected: [bool; 17],
    port_speed: [u32; 17],
    // Per-slot once-only milestone bitmasks (slot id 0..63).
    slot_addressed: u64,
    slot_hid_armed: u64,
    slot_first_report: u64,
    // Root-enum FSM stage edge state.
    last_stage: &'static str,
    last_stage_port: u8,
    last_stall_count: u32,
    // cntpct throttle: the pump spins hot, but enumeration milestones are ms-scale, so the observer
    // samples on a ~2 ms cadence (bounds the per-tick snapshot cost without missing a stage).
    next_poll: u64,
    poll_period: u64,
}

impl EnumWitness {
    fn new() -> Self {
        let freq = super::timer::cntfrq();
        EnumWitness {
            port_connected: [false; 17],
            port_speed: [0; 17],
            slot_addressed: 0,
            slot_hid_armed: 0,
            slot_first_report: 0,
            last_stage: "idle",
            last_stage_port: 0,
            last_stall_count: 0,
            next_poll: 0,
            poll_period: (freq / 500).max(1), // ~2 ms
        }
    }

    fn speed_name(id: u32) -> &'static str {
        match id { 1 => "Full-Speed", 2 => "Low-Speed", 3 => "High-Speed", 4 => "SuperSpeed", _ => "untrained" }
    }

    /// Snapshot the driver's read-only state and emit any newly-crossed milestones. Never mutates
    /// the controller.
    fn tick(&mut self, x: &xhci::XhciController) {
        let now = super::timer::cntpct();
        if now.wrapping_sub(self.next_poll) >= (1u64 << 63) {
            return; // not yet due (wrap-safe)
        }
        self.next_poll = now.wrapping_add(self.poll_period);

        // (1) Root-port connect / speed edges.
        for (port, connected, speed) in x.root_ports_now() {
            let p = port as usize;
            if p >= self.port_connected.len() { continue; }
            if connected && !self.port_connected[p] {
                serial_println!("{} [enum] port {} connect (device attached) ::", P, port);
            } else if !connected && self.port_connected[p] {
                serial_println!("{} [enum] port {} disconnect (device removed) ::", P, port);
                self.port_speed[p] = 0; // re-arm the speed edge for a re-plug
            }
            self.port_connected[p] = connected;
            if connected && speed != 0 && self.port_speed[p] == 0 {
                serial_println!(
                    "{} [enum] port {} trained speed {} (xhci speed id {}) ::",
                    P, port, Self::speed_name(speed), speed
                );
                self.port_speed[p] = speed;
            }
        }

        // (2) Root-enum FSM stage transitions — surfaces enable-slot (slot assigned),
        //     address-device (addressed), dev-desc/cfg-desc (descriptors), set-config (configured).
        let stage = x.enum_stage_now();
        let stage_port = x.enumerating_port_now();
        if stage != self.last_stage || stage_port != self.last_stage_port {
            if stage != "idle" {
                serial_println!("{} [enum] port {} stage -> {} ::", P, stage_port, stage);
            }
            self.last_stage = stage;
            self.last_stage_port = stage_port;
        }

        // (3) Per-slot milestones (addressed / HID armed / first report), once each.
        for (i, s) in x.slots.iter().enumerate() {
            if i >= 64 { break; }
            let bit = 1u64 << i;
            if s.active && (self.slot_addressed & bit) == 0 {
                serial_println!("{} [enum] slot {} addressed (root port {}) ::", P, i, s.port_id);
                self.slot_addressed |= bit;
            }
            if s.active && s.is_keyboard && s.keyboard_state == 3 && (self.slot_hid_armed & bit) == 0 {
                serial_println!(
                    "{} [enum] slot {} HID boot-keyboard armed (interrupt-IN ep {:#04x} mps {}, root port {}) ::",
                    P, i, s.keyboard_ep, s.keyboard_mps, s.port_id
                );
                self.slot_hid_armed |= bit;
            }
            if s.is_keyboard && s.keyboard_report_count > 0 && (self.slot_first_report & bit) == 0 {
                serial_println!(
                    "{} [enum] slot {} first keyboard report received — HID pipe live (keycodes decode to `xHCI: KEY` lines) ::",
                    P, i
                );
                self.slot_first_report |= bit;
            }
        }

        // (4) Stall localizer — on a NEW stall, name the stage + completion code that stopped it.
        let sc = x.stall_count_now();
        if sc != self.last_stall_count {
            if let Some((port, st, why, code, portsc)) = x.last_stall_now() {
                serial_println!(
                    "{} [enum] STALL port {} @ stage {} ({}, completion code {}) PORTSC={:#010x} — enumeration halted here ::",
                    P, port, st, why, code, portsc
                );
            }
            self.last_stall_count = sc;
        }
    }
}

// ── PIUSB-43: the enum-portsc witness. PA5c metal ran the FULL 30 s enum-pump budget with ZERO
//    port-connect events, and the wire could not say WHICH link died: no device electrically
//    present (CCS never set)? CCS set but no Port Status Change Event ever generated? events
//    written but never consumed off our ring? or PP/link state regressed after the "M3: root
//    port(s) powered" line? This witness samples every root port's RAW PORTSC word — raw printed
//    beside every decode, the frozen-register discipline — plus the event-ring consumer state
//    (dequeue index/cycle, total TRBs consumed, a fresh-TRB peek, IMAN) at pump START, on a ~2 s
//    cadence through the pump, and once at pump END, then closes with ONE verdict line naming the
//    branch the sample series actually supports. Read-only on the sample cadence: PORTSC reads
//    latch nothing (all change bits are RW1C and we never write), IMAN is read not acknowledged,
//    and the ring peek is `has_event` (cache invalidate + volatile read — no state change). No
//    pump logic, budget, or write is touched.
//
//    Default-ON, deliberately: the instrument's whole reason to exist is the failing path, and a
//    knob-off default would be the classic unreachable-instrument mistake. QEMU raspi4b never
//    reaches the pump (`XHCI_READY` gate — no PCIe RC/VL805 modeled), so the witness is
//    hardware-gated quiet there and the QEMU log stays byte-identical.
//
//    Line budget: 1 HCSPARAMS1 + 1 start + <=13 periodic + 1 end + 1 verdict = <=17 lines per boot; an
//    early-exiting healthy pump emits as few as 3.
const PIUSB43_MAX_PORTS: usize = 8; // VL805 exposes 5 root ports; clamp for the fixed arrays
const PIUSB43_PERIODIC_CAP: u32 = 13; // 30 s / 2 s = 15 interior ticks; cap keeps the budget honest

struct PortscWitness {
    /// xHCI operational base (CAP base + CAPLENGTH); PORTSC(n) at +0x400 + 0x10*(n-1).
    op_base: u64,
    /// Interrupter 0 register base (IMAN at +0). 0 = not published; IMAN then prints as absent.
    ir0_base: u64,
    ports: usize,
    /// Pump-start raw PORTSC words — the REGRESSED reference. M3's claim (PP set on all root
    /// ports) is checked against these, and these against every later sample.
    baseline: [u32; PIUSB43_MAX_PORTS],
    have_baseline: bool,
    saw_ccs: u32, // bitmask: port ever sampled CCS=1
    saw_csc: u32, // bitmask: port ever sampled CSC=1 (a connect toggled between samples)
    /// First PP 1->0 observed vs baseline: (port, baseline raw, regressed raw).
    pp_drop: Option<(usize, u32, u32)>,
    /// First PLS change vs baseline on a port that never showed CCS: (port, baseline raw, raw).
    pls_change: Option<(usize, u32, u32)>,
    end_pend: bool,  // has_event() at the most recent sample (the END sample decides)
    end_popped: u64, // total event TRBs consumed as of the most recent sample
    periodic: u32,
    /// First poison PORTSC read: (port, raw). A non-decoding BAR reads 0xffffffff, which DECODES as
    /// CCS=1 CSC=1 PED=1 PP=1 — indistinguishable from a live connect on the decode alone. Latched
    /// here so the sample series is disqualified rather than believed (PI-V3D-1 false-PASS rule).
    poison: Option<(usize, u32)>,
}

impl PortscWitness {
    fn new(op_base: u64, ir0_base: u64, ports: usize) -> Self {
        Self {
            op_base,
            ir0_base,
            ports,
            baseline: [0; PIUSB43_MAX_PORTS],
            have_baseline: false,
            saw_ccs: 0,
            saw_csc: 0,
            pp_drop: None,
            pls_change: None,
            end_pend: false,
            end_popped: 0,
            periodic: 0,
            poison: None,
        }
    }

    /// One sample = ONE serial line: every root port's raw PORTSC beside its decode, then the
    /// event-ring consumer state. Read-only throughout (see the block comment above).
    fn sample(&mut self, tag: &str) {
        use core::fmt::Write as _;
        let mut line = alloc::string::String::new();
        let freq = super::timer::cntfrq().max(1);
        let t_ms = super::timer::cntpct().saturating_mul(1000) / freq;
        let _ = write!(line, "{} [piusb43] portsc {} t={}ms", P, tag, t_ms);
        for i in 0..self.ports {
            let raw = r(self.op_base + 0x400 + (i as u64) * 0x10);
            // A poison word is NOT live data. Print it with an explicit tell and accumulate NOTHING
            // from it: 0xffffffff would otherwise set saw_ccs/saw_csc and drive verdict() into
            // CCS-NO-EVENT ("a device was seen electrically") off a BAR that stopped decoding.
            if is_poison(raw) {
                let _ = write!(line, " p{}={:#010x}(POISON — not decoded)", i + 1, raw);
                if self.poison.is_none() {
                    self.poison = Some((i + 1, raw));
                }
                continue;
            }
            let ccs = raw & 1;
            let csc = (raw >> 17) & 1;
            let ped = (raw >> 1) & 1;
            let pp = (raw >> 9) & 1;
            let pls = (raw >> 5) & 0xF;
            let spd = (raw >> 10) & 0xF;
            let _ = write!(
                line,
                " p{}={:#010x}(CCS={} CSC={} PED={} PP={} PLS={} spd={})",
                i + 1, raw, ccs, csc, ped, pp, pls, spd
            );
            if ccs == 1 {
                self.saw_ccs |= 1 << i;
            }
            if csc == 1 {
                self.saw_csc |= 1 << i;
            }
            if self.have_baseline {
                let b = self.baseline[i];
                if self.pp_drop.is_none() && (b >> 9) & 1 == 1 && pp == 0 {
                    self.pp_drop = Some((i + 1, b, raw));
                }
                if self.pls_change.is_none() && ccs == 0 && (b & 1) == 0 && ((b >> 5) & 0xF) != pls {
                    self.pls_change = Some((i + 1, b, raw));
                }
            } else {
                self.baseline[i] = raw;
            }
        }
        // Event-ring consumer state. The EVENT_RING lock is taken alone here — the caller does NOT
        // hold the XHCI_CONTROLLER guard at sample points, matching poll_events' take-then-release
        // ordering, and the IRQ path never takes these locks (raw-MMIO-only by contract).
        let (deq, cyc, pend, popped) = {
            let g = xhci::EVENT_RING.lock();
            match g.as_ref() {
                Some(er) => (er.dequeue_index, er.cycle_bit as u32, er.has_event(), er.popped),
                None => (0, 0, false, 0),
            }
        };
        self.end_pend = pend;
        self.end_popped = popped;
        if self.ir0_base != 0 {
            let iman = r(self.ir0_base);
            let _ = write!(
                line,
                " | evt deq={} cyc={} popped={} pend={} IMAN={:#010x}(IP={} IE={}) ::",
                deq, cyc, popped, pend as u32, iman, iman & 1, (iman >> 1) & 1
            );
        } else {
            let _ = write!(
                line,
                " | evt deq={} cyc={} popped={} pend={} IMAN=unpublished ::",
                deq, cyc, popped, pend as u32
            );
        }
        serial_println!("{}", line);
        self.have_baseline = true;
    }

    /// ONE verdict line at pump end, naming the branch the sample series supports — and ONLY that.
    /// Branch priority: an unconsumed ring first (it invalidates any silence claim downstream of
    /// it), then powered-state regressions, then the device-evidence branches. A state the samples
    /// cannot distinguish prints UNRESOLVED naming the read that would decide it.
    fn verdict(&self, advanced: Option<&str>) {
        // The pump reached a terminal SUCCESS state. Every branch below names a way the connect
        // path died; none of them may speak here. PA6 metal printed a confident
        // "yet enumeration did not advance" while the hub was walked, slots addressed and the
        // keyboard armed — the verdict was asserting a state it had never checked.
        if let Some(how) = advanced {
            serial_println!(
                "{} [piusb43] verdict=ADVANCED — the pump exited on a terminal success ({}); the connect path did NOT die and no death branch is claimed. Ring at exit: popped={} pend={} ::",
                P, how, self.end_popped, self.end_pend as u32
            );
            return;
        }
        if !self.have_baseline {
            serial_println!(
                "{} [piusb43] verdict=UNRESOLVED — no sample was ever taken (pump exited before the start sample); this instrument did not execute in the state it reports on, so its silence is not evidence ::",
                P
            );
            return;
        }
        // Ring holds a fresh TRB at exit AND nothing was ever consumed: events reached the ring
        // but the consumer never took one. (pend=1 with popped>0 is NOT this branch — the pump
        // was draining and one more event arrived after the last poll; the end sample line shows
        // it either way.)
        if self.end_pend && self.end_popped == 0 {
            // The ring finding is DRAM-derived and survives a dark BAR — but if PORTSC read poison
            // this line must carry that tell too, or it is the same confident-verdict-from-a-dark-
            // window defect the poison branch below exists to prevent, just pointing the other way.
            match self.poison {
                Some((port, raw)) => serial_println!(
                    "{} [piusb43] verdict=EVENTS-UNCONSUMED — the event ring holds a fresh TRB at pump exit (pend=1) and popped=0 events were consumed all pump: events reached the ring, the consumer never took one. CAVEAT: port {} PORTSC read back {:#010x} (poison), so this ring finding stands but NO PORTSC-derived reading does ::",
                    P, port, raw
                ),
                None => serial_println!(
                    "{} [piusb43] verdict=EVENTS-UNCONSUMED — the event ring holds a fresh TRB at pump exit (pend=1) and popped=0 events were consumed all pump: events reached the ring, the consumer never took one ::",
                    P
                ),
            }
            return;
        }
        // A poison PORTSC read disqualifies every PORTSC-DERIVED branch below (REGRESSED,
        // CCS-NO-EVENT, NO-CCS-EVER all rest on those words). It is checked AFTER the ring branch
        // above because that one is derived from the event ring in DRAM, not from this MMIO window,
        // and stays sound while the BAR is dark. The raw ring words ride this line so nothing the
        // series did observe is lost.
        if let Some((port, raw)) = self.poison {
            serial_println!(
                "{} [piusb43] verdict=UNRESOLVED — port {} PORTSC read back {:#010x} (poison): the xHC register window stopped decoding, so the PORTSC-derived branches cannot be judged (that word decodes as CCS=1 CSC=1 PED=1 PP=1 and would otherwise be read as a device). Ring state at exit: popped={} pend={}. Deciding read: re-probe the VL805's PCIe config vendor:device and BAR decode enable — a live 1106:3483 means the window is back and the pump should be re-run; poison there means the RC link or the BAR window dropped ::",
                P, port, raw, self.end_popped, self.end_pend as u32
            );
            return;
        }
        // M3 said all root ports powered; a baseline PP=0 means the regression happened BETWEEN
        // the M3 line and the pump start.
        for i in 0..self.ports {
            if (self.baseline[i] >> 9) & 1 == 0 {
                serial_println!(
                    "{} [piusb43] verdict=REGRESSED — port {} PORTSC={:#010x} already shows PP=0 at pump start, but M3 reported PORTSC.PP set on all root ports: the powered state regressed between M3 and the pump ::",
                    P, i + 1, self.baseline[i]
                );
                return;
            }
        }
        if let Some((port, before, after)) = self.pp_drop {
            serial_println!(
                "{} [piusb43] verdict=REGRESSED — port {} PP dropped during the pump: PORTSC {:#010x} (pump start) -> {:#010x}; the M3 powered state did not hold ::",
                P, port, before, after
            );
            return;
        }
        let dev = self.saw_ccs | self.saw_csc;
        if dev != 0 {
            if self.end_popped == 0 {
                serial_println!(
                    "{} [piusb43] verdict=CCS-NO-EVENT — port mask {:#x} showed CCS/CSC set in the samples, yet popped=0 event TRBs were consumed and none is pending at exit: a device was seen electrically but no Port Status Change Event reached the ring — event path suspect ::",
                    P, dev
                );
            } else {
                serial_println!(
                    "{} [piusb43] verdict=UNRESOLVED — port mask {:#x} showed CCS/CSC and popped={} event TRBs were consumed, yet enumeration did not advance; deciding read: a TRB-type census of the consumed events (was any a Port Status Change Event, and for which port) — this witness does not retain popped TRB types ::",
                    P, dev, self.end_popped
                );
            }
            return;
        }
        if let Some((port, before, after)) = self.pls_change {
            serial_println!(
                "{} [piusb43] verdict=REGRESSED — port {} link state moved without a connect: PORTSC {:#010x} (pump start) -> {:#010x} (PLS changed, CCS never set); the link regressed vs the M3 state ::",
                P, port, before, after
            );
            return;
        }
        serial_println!(
            "{} [piusb43] verdict=NO-CCS-EVER — all {} root ports sampled CCS=0 CSC=0 at every sample across the pump (PP held, PLS stable, popped={}): no device was electrically visible to the VL805 ::",
            P, self.ports, self.end_popped
        );
    }
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
    //
    // PIUSB-11: this is the load-bearing NOTIFY. `bringup` issued its NOTIFY seconds ago (pre-heap);
    // the intervening heap/AP/eMMC bring-up means the VL805 firmware may no longer be running when
    // this HCRST completes — and a firmwareless controller wedges USBSTS.CNR=1 forever, dropping every
    // CRCR/DCBAAP/ERST/RS write (boot-P18..P21). Linux reloads the fw on EVERY probe immediately before
    // the HCRST (reset_control_reset → rpi_reset_reset, then xhci_gen_setup's reset); we mirror that
    // here so the CNR wait (PIUSB-10) has a running firmware to clear against.
    // PIUSB-40: the deferred half's stages. Reached only past the `XHCI_READY` gate, so QEMU raspi4b
    // emits none of these.
    let t_enum_total = stage_begin();
    let t = stage_begin();
    notify_before_reset("enumerate");
    let t = stage_end("enum-notify", t);
    xhci::init(base);
    stage_end("enum-xhci-handoff", t);

    // (2) Rings / DCBAA / interrupter via the driver's existing init paths (heap-backed). Exactly the
    //     JB2b sequence: construct the controller, seat the command + event rings, point the
    //     interrupter's ERST at the event ring, seat the command-ring pointer, then RS=1.
    let t_rings = stage_begin();
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
        // XHCI-COHERENCE: the shared `drivers/xhci` driver is now SELF-COHERENT — it cleans the ERST
        // (init_interrupter), the zeroed event ring (init_interrupter clean+invalidate), and every
        // ring's zeroed handoff (TransferRing::new) via its internal `dma_coherency` seam. PIUSB-8's
        // external pre-RS flush of these same structures is therefore redundant and has been removed;
        // maintaining it here would double-apply the same `dc c*vac` for no effect.
        serial_println!("{}   programming interrupter + rings (runtime regs), then RS=1 ::", P);
        x.init_interrupter(event_ring_phys, erst_table_phys);
        x.init_pointers(command_ring_phys);
        x.start();
        *xhci::XHCI_CONTROLLER.lock() = Some(x);
    }
    stage_end("enum-rings-rs1", t_rings);

    // XHCI-COHERENCE witness: the shared driver now self-maintains every DMA structure through its
    // internal `dma_coherency` seam (command/transfer TRBs cleaned at push, the event ring
    // invalidated at each dequeue, contexts/DCBAA/ERST/scratchpad cleaned before their command / the
    // Run bit, transfer buffers invalidated before the CPU reads them). The external PIUSB-8 bridge
    // that used to clean/invalidate these from here is retired — the Pi 4 (non-coherent PCIe → VL805)
    // is covered by the same `target_arch = "aarch64"` path as the Jetson.
    let cmd_ring_base = xhci::COMMAND_RING.lock().as_ref().map(|r| r.get_ptr()).unwrap_or(0);
    let evt_ring_base = xhci::EVENT_RING.lock().as_ref().map(|r| r.get_ptr()).unwrap_or(0);
    serial_println!(
        "{} enumerate: xHCI self-coherent (cmd ring @ {:#x}, event ring @ {:#x}) — drivers/xhci dma_coherency seam maintains all DMA memory on aarch64; no external bridge ::",
        P, cmd_ring_base, evt_ring_base
    );

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
    // PIUSB-13: the post-CNR enumeration observer. Inert until real enumeration makes progress (see
    // EnumWitness gating note) — silent on a CNR-stuck boot, one `[enum]` line per milestone on a
    // live one.
    let mut witness = EnumWitness::new();
    serial_println!(
        "{} [enum] observer armed — watching root-port connect/speed, slot assign/address, HID arm, first report (silent unless enumeration advances) ::",
        P
    );
    // PIUSB-43: arm the enum-portsc witness — start sample now, ~2 s cadence inside the loop,
    // end sample + verdict after it. All reads, no pump-logic change (see the witness block doc).
    // HCSPARAMS1 is read the same way and gets the same poison discipline: a poison word yields
    // MaxPorts=0xff, which the clamp would silently turn into 8 — three ports past the VL805's five,
    // sampled as if they existed. Poison here means the window is dark, so fall back to the known
    // VL805 port count and let the per-port poison tell in `sample()` name the state on the wire.
    // The port count is a DERIVED value, so its raw word rides the wire UNCONDITIONALLY — otherwise
    // NO-CCS-EVER quotes "all N root ports" with nothing letting a reader check where N came from.
    // `0` is poison here too (this file's own idiom at the CAP[0] probe): a zero read would clamp to
    // 1 and the witness would report NO-CCS-EVER across "all 1 root ports".
    let hcs1 = r(base + 0x04);
    let ports = if is_poison(hcs1) || hcs1 == 0 {
        serial_println!(
            "{} [piusb43] HCSPARAMS1={:#010x} (poison/zero) — MaxPorts unreadable; sampling the VL805's 5 root ports and expecting the per-port poison tell below ::",
            P, hcs1
        );
        5
    } else {
        let n = (((hcs1 >> 24) & 0xff) as usize).clamp(1, PIUSB43_MAX_PORTS);
        serial_println!(
            "{} [piusb43] HCSPARAMS1={:#010x} MaxPorts={} — sampling {} root port(s) ::",
            P, hcs1, (hcs1 >> 24) & 0xff, n
        );
        n
    };
    let mut pw = PortscWitness::new(
        base + cap_length,
        xhci::XHCI_IR0_BASE.load(Ordering::Acquire) as u64,
        ports,
    );
    pw.sample("start");
    let mut pw_last = super::timer::cntpct();
    // PIUSB-43r3: what the pump ACHIEVED, for the verdict. PA6 metal printed
    // "verdict=UNRESOLVED — … yet enumeration did not advance" on a boot that enumerated a hub,
    // addressed slots and armed a keyboard: the verdict had no success branch and asserted a
    // failure it never checked. The witness must know how the pump exited before it names a death.
    let mut enum_advanced: Option<&'static str> = None;
    loop {
        // XHCI-COHERENCE: no external cache maintenance here anymore. The driver's `poll_events`
        // invalidates the event ring at each dequeue (`EventRing::has_event`) and cleans every command
        // TRB it pushes (`ring::write_trb`), so the enable-slot handshake is coherent by construction
        // on aarch64 without this loop touching the caches.
        let (armed_now, storage_slot, storage_ready) = {
            let mut guard = xhci::XHCI_CONTROLLER.lock();
            let x = guard.as_mut().unwrap();
            x.poll_events();
            x.service_hubs();
            x.service_hid_setproto();
            x.service_slot_disposal();
            x.service_enum();
            // service_storage() carries the PIUSB-25 `[piusb25]` enumeration + LBA0 read-proof
            // witness (shared xhci, aarch64-gated) — it fires here on Pi metal and on the QEMU virt
            // main-loop path alike, the instant the shared bring-up publishes geometry.
            x.service_storage();
            witness.tick(x);
            let ready = x.storage_slot != 0 && x.storage_note == "ready";
            (
                keyboard_armed(x),
                x.storage_slot,
                ready,
            )
        };
        // PIUSB-43: ~2 s sample cadence, bounded, taken with the controller guard already
        // dropped (the block above closed) so the witness's EVENT_RING lock is taken alone.
        if pw.periodic < PIUSB43_PERIODIC_CAP {
            let now = super::timer::cntpct();
            if now.wrapping_sub(pw_last) >= freq.saturating_mul(2) {
                pw.periodic += 1;
                pw_last = now;
                pw.sample("pump");
            }
        }
        if armed.is_none() {
            if let Some((slot, port)) = armed_now {
                serial_println!("{} enumerate: keyboard ARMED (slot {}, root port {}) -> PASS ::", P, slot, port);
                armed = Some((slot, port));
                armed_at = super::timer::cntpct();
            }
        }
        // Storage-ready is a terminal state on its own — a stick present at boot with no keyboard
        // must still report and exit (the old gate only broke once a keyboard had armed, an x86
        // keyboard-first carryover that left a disk-only Pi boot spinning to the 30 s deadline).
        if storage_ready {
            serial_println!("{} enumerate: mass storage ready (slot {}) ::", P, storage_slot);
            enum_advanced = Some("mass storage ready");
            break;
        }
        if armed.is_some() {
            if super::timer::cntpct().wrapping_sub(armed_at) >= storage_settle {
                enum_advanced = Some("keyboard armed, no mass storage within the settle window");
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

    // PIUSB-43: end sample + the one verdict line, before the stage bracket closes.
    pw.sample("end");
    pw.verdict(enum_advanced);

    stage_end("enum-pump", pump_start);
    stage_end("enum-total", t_enum_total);

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
