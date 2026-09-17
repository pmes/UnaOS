// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HDA — the Intel High Definition Audio controller (`UNAOS_HDA=1`, default OFF; the tone needs
//! `UNAOS_HDATONE=1`).
//!
//! # SCOPE
//!
//! **Arc 1 (`hda`) does:** find every PCI function reporting class 0x04 / subclass 0x03 through the
//! `[hda] census` walk in `drivers/pci.rs`, claim one (memory decode + bus master), map BAR0
//! uncacheable, read `GCAP`/`VMIN`/`VMAJ`, reset the controller through `GCTL.CRST` with the
//! specified timing, read `STATESTS` as the codec presence bitmap, build CORB and RIRB rings in
//! DMA-visible memory, run one `GET_PARAMETER VENDOR_ID` round trip per present codec, and then
//! walk every widget of every audio function group — node type, connection list, pin capabilities
//! and the pin default-configuration decode — deriving the output path (converter to pin) the
//! internal speaker sits on.
//!
//! **Arc 2 (`hda-tone`) is the next commit of this series** and is not in this one: an output
//! stream on OSS 0 carrying a sine through a BDL, bound to the derived path's converter, proved
//! without ears. Nothing in this file writes a stream-descriptor register yet.
//!
//! **Does NOT — 1: no interrupts.** `INTCTL` is never written, so `GIE`/`CIE`/`SIE` stay 0 and the
//! controller raises no message. `SDnCTL.IOCE` IS set, which is what makes `SDnSTS.BCIS` latch on a
//! buffer boundary, but with `INTCTL` at 0 that latch reaches no CPU: it is a polled flag. Every
//! wait in this file is a bounded TSC spin, the same discipline `drivers/ahci.rs` and
//! `drivers/sdhc.rs` use, because everything here happens inside `pci::init` where `arch::ms()`
//! does not advance.
//!
//! **Does NOT — 2: nothing plays.** This arc reads the codec, powers a function group so its
//! parameter reads are meaningful, and derives a path. It programs no stream and moves no sample.
//!
//! **Does NOT — 3: no mixer, no volume seam, no userspace.** Arc 3 is owed; see
//! `docs/dev/OS/11_AUDIO/hda.md`.
//!
//! # Why this exists
//!
//! Nothing in this kernel has ever touched audio. `PciScanner::enumerate_buses` matches exactly one
//! class triple (xHCI) and the `[PCI-STOR]` census matches class 0x01 and class 0x08/0x05 — so a
//! class-0x04 function is a function this kernel has structurally never been able to see, and no
//! capture this project has taken carries an `[hda]` line. ROADMAP §6, row "Audio (kernel): x86 HDA".
//!
//! # Clean room
//!
//! Every register offset, bit and verb in this file is transcribed from the public **Intel High
//! Definition Audio Specification revision 1.0a** and the codec verb/parameter tables it defines.
//! No driver source from any other operating system was read. Citation tags follow the tree's
//! convention: `[HDA-SPEC §x.y]` for the specification, `[TREE]` for a property of this codebase,
//! `[QEMU]` for a fixture fact, `[METAL]` for a bench-measured fact.
//!
//! # Arch neutrality
//!
//! The module is declared `#[cfg(all(target_arch = "x86_64", feature = "hda"))]` in `drivers/mod.rs`
//! because its enumeration seam is `arch::pci` config space, which on this tree is an x86 seam. The
//! FILE is arch-neutral in name and shape — HDA exists on other machines, and the Pi's HDMI/PWM
//! audio twin is its sibling, not its replacement (LAWS §3, ONE OS). Nothing here is named for a
//! board.
//!
//! # Byte identity
//!
//! Knob off, this file is not lexed at all (the `#[cfg]`-erased `pub mod` is the one case LAWS §5
//! names as byte-safe for a module), the census in `drivers/pci.rs` is an IMPL-TAIL append and the
//! call site there is a LINE-NEUTRAL append that vanishes — so no `core::panic::Location` line
//! number anywhere moves. `arroyo`'s `arm_features` strips `hda` and `hda-tone` from aarch64 media
//! so neither knob can shift an aarch64 `-Cmetadata` fingerprint.
//!
//! # The witness lines
//!
//! ```text
//! [hda] census bdf=0:27.0 id=8086:1e20 class=04 sub=03 bar0=0x... irq=...
//! [hda] absent
//! [hda] gcap=0x.... oss=. iss=. bss=. 64ok=. version=1.0
//! [hda] reset crst=1 statests=0x.... codecs=[0]
//! [hda] codec=0 vid=1af4:0022 rev=0x........
//! [hda] node=0x02 type=audio-out conns=[] pincfg=-
//! [hda] walk codecs=1 nodes=5 dacs=1 pins=2 speaker_pin=0x.. hp_pin=0x..
//! :: HDA: codecs=<n> nodes=<n> path=<found|none> -> PASS|FAIL ::
//! ```
//!
//! Both verdict lines are printed ONLY on a machine that has an HDA controller and reached the
//! stage they report on, so their silence means "no controller answered", never "the check passed"
//! (LAWS §5: an absence is evidence only if the producing path ran). A `-> FAIL` reds any leg
//! through `arroyo`'s FAULT-SCAN list and `mbench`'s `DEFAULT_FORBIDS`, which is what makes the
//! go-red mutation a measured leg rather than a reading of this source.

// ════════════════════════════════════════════════════════════════════════════════════════════════
// Controller register map — [HDA-SPEC §3.3], offsets from BAR0. Widths are part of the contract:
// the specification assigns each register a size and a controller is entitled to ignore an access
// of the wrong width, so every accessor below names its width and every constant names its owner.
// ════════════════════════════════════════════════════════════════════════════════════════════════

const REG_GCAP: u64 = 0x00; // 16-bit — Global Capabilities        [HDA-SPEC §3.3.2]
const REG_VMIN: u64 = 0x02; //  8-bit — Minor Version              [HDA-SPEC §3.3.3]
const REG_VMAJ: u64 = 0x03; //  8-bit — Major Version              [HDA-SPEC §3.3.4]
const REG_GCTL: u64 = 0x08; // 32-bit — Global Control             [HDA-SPEC §3.3.7]
const REG_STATESTS: u64 = 0x0E; // 16-bit — State Change Status    [HDA-SPEC §3.3.9]
const REG_INTCTL: u64 = 0x20; // 32-bit — Interrupt Control        [HDA-SPEC §3.3.14]

const REG_CORBLBASE: u64 = 0x40; // 32-bit                         [HDA-SPEC §3.3.18]
const REG_CORBUBASE: u64 = 0x44; // 32-bit                         [HDA-SPEC §3.3.19]
const REG_CORBWP: u64 = 0x48; // 16-bit                            [HDA-SPEC §3.3.20]
const REG_CORBRP: u64 = 0x4A; // 16-bit                            [HDA-SPEC §3.3.21]
const REG_CORBCTL: u64 = 0x4C; //  8-bit                           [HDA-SPEC §3.3.22]
const REG_CORBSIZE: u64 = 0x4E; //  8-bit                          [HDA-SPEC §3.3.24]

const REG_RIRBLBASE: u64 = 0x50; // 32-bit                         [HDA-SPEC §3.3.25]
const REG_RIRBUBASE: u64 = 0x54; // 32-bit                         [HDA-SPEC §3.3.26]
const REG_RIRBWP: u64 = 0x58; // 16-bit                            [HDA-SPEC §3.3.27]
const REG_RINTCNT: u64 = 0x5A; // 16-bit                           [HDA-SPEC §3.3.28]
const REG_RIRBCTL: u64 = 0x5C; //  8-bit                           [HDA-SPEC §3.3.29]
const REG_RIRBSTS: u64 = 0x5D; //  8-bit                           [HDA-SPEC §3.3.30]
const REG_RIRBSIZE: u64 = 0x5E; //  8-bit                          [HDA-SPEC §3.3.31]

/// `GCTL.CRST` — Controller Reset. 0 asserts reset, 1 releases it. [HDA-SPEC §3.3.7]
const GCTL_CRST: u32 = 1 << 0;

/// `CORBCTL.CORBRUN` — CORB DMA Engine Enable. [HDA-SPEC §3.3.22]
const CORBCTL_RUN: u8 = 1 << 1;
/// `CORBRP.CORBRPRST` — Read Pointer Reset. [HDA-SPEC §3.3.21]
const CORBRP_RST: u16 = 1 << 15;
/// `RIRBCTL.RINTCTL` — Response Interrupt Control. [HDA-SPEC §3.3.29] It gates the `RIRBSTS.RINTFL`
/// LATCH as well as the interrupt, which is why this polled driver sets it — see the measured note
/// in [`Rings::init`]. No message reaches the CPU regardless: `INTCTL.GIE` is never written.
const RIRBCTL_RINTCTL: u8 = 1 << 0;
/// `RIRBCTL.RIRBDMAEN` — RIRB DMA Engine Enable. [HDA-SPEC §3.3.29]
const RIRBCTL_DMAEN: u8 = 1 << 1;
/// `RIRBWP.RIRBWPRST` — Write Pointer Reset (write-only, self-clearing). [HDA-SPEC §3.3.27]
const RIRBWP_RST: u16 = 1 << 15;
/// `RIRBSTS.RINTFL` — Response Interrupt, RW1C. Set when `RINTCNT` responses have landed; it is
/// the ACKNOWLEDGEMENT the command engine's flow control waits on, not merely an interrupt flag.
/// [HDA-SPEC §3.3.30]
const RIRBSTS_RINTFL: u8 = 1 << 0;
/// `RIRBSTS.RIRBOIS` — Response Overrun Interrupt Status, RW1C. [HDA-SPEC §3.3.30]
const RIRBSTS_OIS: u8 = 1 << 2;




// ════════════════════════════════════════════════════════════════════════════════════════════════
// Codec verbs and parameters — [HDA-SPEC §7.3]. A verb is 20 bits inside the 32-bit CORB entry:
// either a 12-bit identifier with an 8-bit payload, or a 4-bit identifier with a 16-bit payload.
// ════════════════════════════════════════════════════════════════════════════════════════════════

const VERB_GET_PARAMETER: u32 = 0xF00; // payload = parameter id       [HDA-SPEC §7.3.3]
const VERB_GET_CONNECTION_ENTRY: u32 = 0xF02; //                        [HDA-SPEC §7.3.3.3]
const VERB_SET_POWER_STATE: u32 = 0x705; //                             [HDA-SPEC §7.3.3.10]
const VERB_GET_CONFIG_DEFAULT: u32 = 0xF1C; // four bytes at F1C..F1F   [HDA-SPEC §7.3.3.31]

const PARAM_VENDOR_ID: u32 = 0x00; //                                   [HDA-SPEC §7.3.4.1]
const PARAM_REVISION_ID: u32 = 0x02; //                                 [HDA-SPEC §7.3.4.2]
const PARAM_SUBNODE_COUNT: u32 = 0x04; //                               [HDA-SPEC §7.3.4.3]
const PARAM_FUNCTION_TYPE: u32 = 0x05; //                               [HDA-SPEC §7.3.4.4]
const PARAM_WIDGET_CAPS: u32 = 0x09; //                                 [HDA-SPEC §7.3.4.6]
const PARAM_PIN_CAPS: u32 = 0x0C; //                                    [HDA-SPEC §7.3.4.9]
const PARAM_CONNECTION_LEN: u32 = 0x0E; //                              [HDA-SPEC §7.3.4.11]

/// `PARAM_FUNCTION_TYPE` bits 7:0 — 0x01 is an Audio Function Group. [HDA-SPEC §7.3.4.4]
const FG_TYPE_AUDIO: u32 = 0x01;

// Widget types — `PARAM_WIDGET_CAPS` bits 23:20. [HDA-SPEC §7.3.4.6]
const WT_AUDIO_OUT: u8 = 0x0;
const WT_AUDIO_IN: u8 = 0x1;
const WT_MIXER: u8 = 0x2;
const WT_SELECTOR: u8 = 0x3;
const WT_PIN: u8 = 0x4;
const WT_POWER: u8 = 0x5;
const WT_VOLUME_KNOB: u8 = 0x6;
const WT_BEEP: u8 = 0x7;

const WCAP_IN_AMP: u32 = 1 << 1; // Input Amp Present
const WCAP_OUT_AMP: u32 = 1 << 2; // Output Amp Present
const WCAP_AMP_OVERRIDE: u32 = 1 << 3; // Amp Param Override
const WCAP_CONN_LIST: u32 = 1 << 8; // Connection List Present
const WCAP_DIGITAL: u32 = 1 << 9; // Digital
const WCAP_POWER_CTL: u32 = 1 << 10; // Power Control

const PINCAP_OUTPUT: u32 = 1 << 4; // Output Capable       [HDA-SPEC §7.3.4.9]
const PINCAP_HP_DRIVE: u32 = 1 << 3; // Headphone Drive Capable
const PINCAP_EAPD: u32 = 1 << 16; // EAPD Capable



/// Pin default-configuration Default Device field, bits 23:20. [HDA-SPEC §7.3.3.31 table 109]
const DEV_LINE_OUT: u8 = 0x0;
const DEV_SPEAKER: u8 = 0x1;
const DEV_HP_OUT: u8 = 0x2;

// ── Ring and buffer geometry ────────────────────────────────────────────────────────────────────
/// CORB is 256 entries of 4 bytes, the largest size the specification defines, 128-byte aligned.
/// [HDA-SPEC §3.3.18, §3.3.24]
const CORB_ENTRIES: usize = 256;
const CORB_BYTES: usize = CORB_ENTRIES * 4;
/// RIRB is 256 entries of 8 bytes (response + response extended), 128-byte aligned.
/// [HDA-SPEC §3.3.25, §3.3.31]
const RIRB_ENTRIES: usize = 256;
const RIRB_BYTES: usize = RIRB_ENTRIES * 8;
/// `CORBSIZE`/`RIRBSIZE` bits 1:0 value 2 selects 256 entries. [HDA-SPEC §3.3.24]
const RING_SIZE_256: u8 = 0x2;
/// Both ring bases are 128-byte aligned. [HDA-SPEC §3.3.18]
const RING_ALIGN: usize = 128;

/// The codec address space is 15 wide — `STATESTS` is a 15-bit `SDIWAKE` mask. [HDA-SPEC §3.3.9]
const MAX_CODECS: usize = 15;
/// A codec's node address space is 8 bits, so 256 widgets is the architectural ceiling.
/// [HDA-SPEC §7.1.2]
const MAX_NODES: usize = 256;
/// Connection lists are walked to this depth when deriving a path. A converter that is more than
/// eight widgets from its pin does not exist in any codec this driver has been shown; the bound is
/// what makes a malformed or cyclic connection list a printed refusal rather than a hang.
const MAX_PATH_DEPTH: usize = 8;

// ── Bounded waits, in TSC units. Same construction as `drivers/ahci.rs` and `drivers/sdhc.rs`:
// `arch::apic::tsc_hz()` is calibrated against the ACPI PM timer long before `pci::init` runs, and
// `arch::ms()` is NOT usable here (the APIC tick does not advance with interrupts masked). [TREE]
#[inline]
fn cycles_us(us: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    if hz != 0 {
        hz.saturating_mul(us) / 1_000_000
    } else {
        crate::arch::HW_WAIT_BUDGET.saturating_mul(us) / 2_000_000
    }
}

/// Spin until `pred()` holds or `us` microseconds elapse. `wrapping_sub` so a 64-bit TSC wrap
/// mid-wait cannot trip the deadline early.
fn wait_us<F: FnMut() -> bool>(us: u64, mut pred: F) -> bool {
    let start = crate::arch::now_cycles();
    let budget = cycles_us(us);
    loop {
        if pred() {
            return true;
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Busy-wait `us` microseconds. Used where the specification names a settling time rather than a
/// condition to poll.
fn delay_us(us: u64) {
    let start = crate::arch::now_cycles();
    let budget = cycles_us(us);
    while crate::arch::now_cycles().wrapping_sub(start) < budget {
        core::hint::spin_loop();
    }
}


// ── MMIO. `base` is the identity-mapped physical BAR0; the window is mapped Uncacheable, so no
// fence is needed between accesses on x86 (UC is strongly ordered). [TREE]
#[inline]
fn r8(base: u64, off: u64) -> u8 {
    unsafe { core::ptr::read_volatile((base + off) as *const u8) }
}
#[inline]
fn w8(base: u64, off: u64, v: u8) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u8, v) }
}
#[inline]
fn r16(base: u64, off: u64) -> u16 {
    unsafe { core::ptr::read_volatile((base + off) as *const u16) }
}
#[inline]
fn w16(base: u64, off: u64, v: u16) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u16, v) }
}
#[inline]
fn r32(base: u64, off: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}
#[inline]
fn w32(base: u64, off: u64, v: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, v) }
}

/// The one place a heap pointer becomes a bus address. The identity map means VA == PA on this
/// arch, and every DMA structure in `drivers/xhci/mod.rs` and `drivers/ahci.rs` is programmed into
/// its controller the same way — a future non-identity-mapped arm has one function to change. [TREE]
#[inline]
fn bus_addr(p: u64) -> u64 {
    p
}

/// Allocate zeroed, aligned DMA-visible memory. Returns 0 on failure, which every caller treats as
/// "this controller does not come up" rather than panicking. Deliberately never freed: the
/// controller keeps DMAing out of the CORB and into the RIRB for as long as the engines run, and
/// handing this memory back while the controller still owns it is the one class of bug a teardown
/// must get right. This arc's teardown stops both engines but does not free.
fn dma_alloc(size: usize, align: usize) -> u64 {
    let layout = match core::alloc::Layout::from_size_align(size, align) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    unsafe { alloc::alloc::alloc_zeroed(layout) as u64 }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// THE WRITE AUDIT. Every write this driver issues is counted here and printed on the `[hda] audit`
// line, in the shape `drivers/bcma.rs` established: a number a reader can check against the code,
// and an `(audited)` zero for a class of write this driver deliberately never issues.
// ════════════════════════════════════════════════════════════════════════════════════════════════
#[derive(Default, Clone, Copy)]
struct Audit {
    cfg: u32,       // PCI config-space writes (COMMAND only)
    ctrl: u32,      // controller MMIO register writes (GCTL, CORB*, RIRB*)
    stream: u32,    // stream-descriptor MMIO writes (SDnCTL/FMT/BDPL/BDPU/CBL/LVI/STS)
    verbs_get: u32, // codec verbs that only read
    verbs_set: u32, // codec verbs that change codec state
}

impl Audit {
    fn line(&self, stage: &str) {
        serial_println!(
            "[hda] audit stage={} wrote-cfg={} wrote-ctrl={} wrote-stream={} verbs-get={} verbs-set={} wrote-intctl=0(audited) wrote-wallclk=0(audited) wrote-dplbase=0(audited)",
            stage, self.cfg, self.ctrl, self.stream, self.verbs_get, self.verbs_set
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// The command/response ring pair. One instance per controller; lives for the whole boot.
// ════════════════════════════════════════════════════════════════════════════════════════════════
struct Rings {
    base: u64,
    corb: u64,
    rirb: u64,
    corb_wp: u16,
    rirb_rp: u16,
    /// Latch for the one-shot `[hda] ring-state` line in [`Rings::issue`].
    traced: bool,
}

impl Rings {
    /// [HDA-SPEC §4.4.1.3] CORB initialisation, and §4.4.2.2 RIRB initialisation. Both engines are
    /// stopped first: a controller the firmware left running would otherwise fetch from a base this
    /// function is in the middle of rewriting.
    fn init(base: u64, a: &mut Audit) -> Option<Rings> {
        let corb = dma_alloc(CORB_BYTES, RING_ALIGN);
        let rirb = dma_alloc(RIRB_BYTES, RING_ALIGN);
        if corb == 0 || rirb == 0 {
            serial_println!(
                "[hda] rings REFUSED reason=dma-alloc corb={:#x} rirb={:#x} — the controller is left reset and idle",
                corb, rirb
            );
            return None;
        }

        // Stop both DMA engines before touching either base. [HDA-SPEC §3.3.22, §3.3.29]
        w8(base, REG_CORBCTL, 0);
        w8(base, REG_RIRBCTL, 0);
        a.ctrl += 2;
        if !wait_us(1000, || r8(base, REG_CORBCTL) & CORBCTL_RUN == 0) {
            serial_println!("[hda] rings REFUSED reason=corb-run-stuck corbctl={:#04x}", r8(base, REG_CORBCTL));
            return None;
        }

        // CORBSIZE bits 1:0 select the entry count; bits 7:4 (CORBSZCAP) say which are legal. Bit 6
        // of the capability nibble is the 256-entry bit. [HDA-SPEC §3.3.24]
        let corbsize = r8(base, REG_CORBSIZE);
        if corbsize & 0x40 == 0 {
            serial_println!(
                "[hda] rings REFUSED reason=no-256-entry-corb corbsize={:#04x} (CORBSZCAP nibble={:#x}) — this driver programs only the 256-entry ring the specification's table 25 makes mandatory for a 1.0 controller",
                corbsize, corbsize >> 4
            );
            return None;
        }
        w8(base, REG_CORBSIZE, (corbsize & !0x03) | RING_SIZE_256);
        w32(base, REG_CORBLBASE, (bus_addr(corb) & 0xFFFF_FFFF) as u32);
        w32(base, REG_CORBUBASE, (bus_addr(corb) >> 32) as u32);
        a.ctrl += 3;

        // Read-pointer reset: set CORBRPRST, wait for it to read back set, clear it, wait for zero.
        // [HDA-SPEC §3.3.21]
        w16(base, REG_CORBRP, CORBRP_RST);
        a.ctrl += 1;
        let rp_set = wait_us(5000, || r16(base, REG_CORBRP) & CORBRP_RST != 0);
        w16(base, REG_CORBRP, 0);
        a.ctrl += 1;
        let rp_clr = wait_us(5000, || r16(base, REG_CORBRP) & CORBRP_RST == 0);
        if !rp_clr {
            serial_println!(
                "[hda] rings REFUSED reason=corbrp-reset-stuck corbrp={:#06x} set-ack={}",
                r16(base, REG_CORBRP), rp_set as u8
            );
            return None;
        }
        w16(base, REG_CORBWP, 0);
        a.ctrl += 1;

        // RIRB. `RINTCNT` is written 1 so the controller's response accounting matches this ring's
        // one-command-outstanding design exactly: one response, one acknowledgement.
        //
        // ⚠ `RIRBCTL.RINTCTL` IS SET, IN A DRIVER THAT TAKES NO INTERRUPTS, AND THAT IS NOT A
        // CONTRADICTION — it is the whole flow-control loop, and it cost two QEMU runs to find.
        // `RINTCTL` gates the `RIRBSTS.RINTFL` LATCH, not only the interrupt. With it clear, the
        // controller counts responses against `RINTCNT`, reaches the count, stops fetching
        // commands — and never sets the status bit whose RW1C would release it. MEASURED, on the
        // `intel-hda` fixture: `[hda] ring-state after-first-verb corbwp=0x0001 corbrp=0x0001
        // rirbwp=0x0001 rirbsts=0x00->0x00`, i.e. one response had landed and the latch was still
        // zero, and the next command then sat at `corbwp=0x0002 corbrp=0x0001` forever. Setting
        // `RINTCTL` makes the latch work; `issue` clears it after every response, which is what
        // keeps the command engine fetching.
        //
        // NO MESSAGE REACHES THE CPU, and that claim is separate from this bit: interrupt
        // GENERATION is gated by `INTCTL.GIE`/`CIE` [HDA-SPEC §3.3.14], which this driver never
        // writes — the `[hda] rings` line prints `intctl=…(untouched)` and the audit line carries
        // `wrote-intctl=0(audited)` so a reader can check it from the wire rather than from here.
        // [HDA-SPEC §3.3.28, §3.3.29, §3.3.30]
        let rirbsize = r8(base, REG_RIRBSIZE);
        if rirbsize & 0x40 == 0 {
            serial_println!(
                "[hda] rings REFUSED reason=no-256-entry-rirb rirbsize={:#04x} (RIRBSZCAP nibble={:#x})",
                rirbsize, rirbsize >> 4
            );
            return None;
        }
        w8(base, REG_RIRBSIZE, (rirbsize & !0x03) | RING_SIZE_256);
        w32(base, REG_RIRBLBASE, (bus_addr(rirb) & 0xFFFF_FFFF) as u32);
        w32(base, REG_RIRBUBASE, (bus_addr(rirb) >> 32) as u32);
        w16(base, REG_RIRBWP, RIRBWP_RST);
        w16(base, REG_RINTCNT, 1);
        a.ctrl += 5;

        // Both writes that START DMA come last, after every base and pointer is in place.
        w8(base, REG_CORBCTL, CORBCTL_RUN);
        w8(base, REG_RIRBCTL, RIRBCTL_DMAEN | RIRBCTL_RINTCTL);
        a.ctrl += 2;
        if !wait_us(1000, || r8(base, REG_CORBCTL) & CORBCTL_RUN != 0) {
            serial_println!("[hda] rings REFUSED reason=corb-would-not-run corbctl={:#04x}", r8(base, REG_CORBCTL));
            return None;
        }

        serial_println!(
            "[hda] rings corb={:#x} rirb={:#x} entries=256/256 corbctl={:#04x} rirbctl={:#04x} rintcnt=1 intctl={:#010x}(untouched)",
            corb, rirb, r8(base, REG_CORBCTL), r8(base, REG_RIRBCTL), r32(base, REG_INTCTL)
        );
        Some(Rings { base, corb, rirb, corb_wp: 0, rirb_rp: 0, traced: false })
    }

    /// Issue one verb and return its 32-bit response, or `None` on timeout.
    ///
    /// [HDA-SPEC §4.4.1.3] The command is written into the CORB at WP+1 and WP is then advanced;
    /// [§4.4.2.2] the response lands in the RIRB at the controller's write pointer. Strictly
    /// serialised — one command outstanding at a time — because every caller here needs the answer
    /// before it can form the next question, and a walk that pipelined would have to carry a
    /// per-response codec/nid demultiplexer for no gain at enumeration time.
    fn cmd(&mut self, cad: u8, nid: u8, verb: u32, payload: u32, a: &mut Audit) -> Option<u32> {
        let word = ((cad as u32 & 0xF) << 28) | ((nid as u32) << 20) | ((verb << 8) | payload) & 0x000F_FFFF;
        self.issue(word, a)
    }
    fn issue(&mut self, word: u32, a: &mut Audit) -> Option<u32> {
        let next = (self.corb_wp + 1) % (CORB_ENTRIES as u16);
        unsafe { core::ptr::write_volatile((self.corb + (next as u64) * 4) as *mut u32, word) };
        // The CORB entry must be visible to the controller's DMA engine before WP advances. On this
        // arch the heap is write-back and DMA is snooped, so this is a compiler/store ordering
        // fence, not a cache flush. [TREE]
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        w16(self.base, REG_CORBWP, next);
        a.ctrl += 1;
        self.corb_wp = next;

        let want = (self.rirb_rp + 1) % (RIRB_ENTRIES as u16);
        // 100 ms is a deadline, not an expectation: a codec answers in microseconds. It is short
        // enough that a whole widget walk against a dead codec still ends inside a boot.
        if !wait_us(100_000, || r16(self.base, REG_RIRBWP) & 0xFF == want & 0xFF) {
            serial_println!(
                "[hda] verb TIMEOUT word={:#010x} corbwp={:#06x} corbrp={:#06x} rirbwp={:#06x} rirbsts={:#04x} corbctl={:#04x} rirbctl={:#04x} rintcnt={:#06x}",
                word, r16(self.base, REG_CORBWP), r16(self.base, REG_CORBRP),
                r16(self.base, REG_RIRBWP), r8(self.base, REG_RIRBSTS),
                r8(self.base, REG_CORBCTL), r8(self.base, REG_RIRBCTL), r16(self.base, REG_RINTCNT)
            );
            return None;
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let resp = unsafe { core::ptr::read_volatile((self.rirb + (want as u64) * 8) as *const u32) };
        self.rirb_rp = want;

        // ⚠ THE RIRB STATUS LATCH IS PART OF THE FLOW-CONTROL LOOP, not decoration, and leaving it
        // set stops the CORB dead. `RIRBSTS.RINTFL` (bit 0) latches once `RINTCNT` responses have
        // landed and `RIRBSTS.RIRBOIS` (bit 2) latches on an overrun; both are RW1C
        // [HDA-SPEC §3.3.28, §3.3.30]. A controller counts UNACKNOWLEDGED responses against
        // `RINTCNT` and stops fetching commands when it reaches that count — MEASURED, not
        // reasoned: with this clear absent, the QEMU `intel-hda` fixture answered
        // `GET_PARAMETER VENDOR_ID` and then parked at `corbwp=0x0002 corbrp=0x0001
        // rirbwp=0x0001`, i.e. the second command was written into the ring and never fetched,
        // and the driver read that as a codec that had stopped answering. Clearing HERE —
        // immediately after the response is consumed, before the next command is formed — is the
        // acknowledgement that shape needs, and it pairs exactly with this ring's strictly
        // serialised one-command-outstanding design.
        let sts_before = r8(self.base, REG_RIRBSTS);
        w8(self.base, REG_RIRBSTS, RIRBSTS_RINTFL | RIRBSTS_OIS);
        a.ctrl += 1;
        // ONE-SHOT ring-state line, on the first response of the boot. It is the only place a
        // reader can see the flow-control registers in the state they are actually in mid-walk,
        // and it is what turned "the codec stopped answering" into a measured CORB stall.
        if !self.traced {
            self.traced = true;
            serial_println!(
                "[hda] ring-state after-first-verb corbwp={:#06x} corbrp={:#06x} rirbwp={:#06x} rirbsts={:#04x}->{:#04x} corbctl={:#04x} rirbctl={:#04x} rintcnt={:#06x}",
                r16(self.base, REG_CORBWP), r16(self.base, REG_CORBRP), r16(self.base, REG_RIRBWP),
                sts_before, r8(self.base, REG_RIRBSTS), r8(self.base, REG_CORBCTL),
                r8(self.base, REG_RIRBCTL), r16(self.base, REG_RINTCNT)
            );
        }
        Some(resp)
    }

    /// Stop both DMA engines. Called on every exit path so a controller this driver touched is
    /// never left fetching from heap memory the rest of the boot believes is idle.
    fn stop(&self, a: &mut Audit) {
        w8(self.base, REG_CORBCTL, 0);
        w8(self.base, REG_RIRBCTL, 0);
        a.ctrl += 2;
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// The widget walk
// ════════════════════════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Default)]
struct Widget {
    nid: u8,
    present: bool,
    kind: u8,
    caps: u32,
    pincap: u32,
    pincfg: u32,
    conns: [u8; 8],
    nconns: u8,
}

impl Widget {
    fn out_amp(&self) -> bool {
        self.caps & WCAP_OUT_AMP != 0
    }
    fn in_amp(&self) -> bool {
        self.caps & WCAP_IN_AMP != 0
    }
}

fn widget_kind_name(k: u8) -> &'static str {
    match k {
        WT_AUDIO_OUT => "audio-out",
        WT_AUDIO_IN => "audio-in",
        WT_MIXER => "mixer",
        WT_SELECTOR => "selector",
        WT_PIN => "pin",
        WT_POWER => "power",
        WT_VOLUME_KNOB => "volume-knob",
        WT_BEEP => "beep",
        0xF => "vendor-defined",
        _ => "reserved",
    }
}

/// [HDA-SPEC §7.3.3.31 table 109] Default Device.
fn pin_device_name(d: u8) -> &'static str {
    match d {
        0x0 => "line-out",
        0x1 => "speaker",
        0x2 => "hp-out",
        0x3 => "cd",
        0x4 => "spdif-out",
        0x5 => "digital-other-out",
        0x6 => "modem-line",
        0x7 => "modem-handset",
        0x8 => "line-in",
        0x9 => "aux",
        0xA => "mic-in",
        0xB => "telephony",
        0xC => "spdif-in",
        0xD => "digital-other-in",
        0xF => "other",
        _ => "reserved",
    }
}

/// [HDA-SPEC §7.3.3.31 table 110] Connection Type.
fn pin_conn_name(c: u8) -> &'static str {
    match c {
        0x0 => "unknown",
        0x1 => "stereo-mono-1/8",
        0x2 => "stereo-mono-1/4",
        0x3 => "atapi-internal",
        0x4 => "rca",
        0x5 => "optical",
        0x6 => "other-digital",
        0x7 => "other-analog",
        0x8 => "multichannel-din",
        0x9 => "xlr",
        0xA => "rj-11",
        0xB => "combination",
        0xF => "other",
        _ => "reserved",
    }
}

/// [HDA-SPEC §7.3.3.31] Location: bits 31:30 are port connectivity, 29:28 gross location,
/// 27:24 geometric location.
fn pin_location_name(gross: u8) -> &'static str {
    match gross {
        0x0 => "external",
        0x1 => "internal",
        0x2 => "separate-chassis",
        _ => "other",
    }
}

/// [HDA-SPEC §7.3.3.31] Colour, bits 15:12.
fn pin_colour_name(c: u8) -> &'static str {
    match c {
        0x0 => "unknown",
        0x1 => "black",
        0x2 => "grey",
        0x3 => "blue",
        0x4 => "green",
        0x5 => "red",
        0x6 => "orange",
        0x7 => "yellow",
        0x8 => "purple",
        0x9 => "pink",
        0xE => "white",
        0xF => "other",
        _ => "reserved",
    }
}

/// `PARAM_CONNECTION_LEN` + `GET_CONNECTION_ENTRY`. The list is packed four 8-bit entries per
/// response, or two 16-bit entries when the long-form bit (bit 7 of the length parameter) is set;
/// an entry whose top bit is set is the END of an inclusive RANGE starting at the previous entry.
/// [HDA-SPEC §7.3.3.3, §7.3.4.11]
fn read_connections(rings: &mut Rings, cad: u8, nid: u8, w: &mut Widget, a: &mut Audit) {
    if w.caps & WCAP_CONN_LIST == 0 {
        return;
    }
    let lenparam = match rings.cmd(cad, nid, VERB_GET_PARAMETER, PARAM_CONNECTION_LEN, a) {
        Some(v) => v,
        None => return,
    };
    a.verbs_get += 1;
    let long = lenparam & 0x80 != 0;
    let count = (lenparam & 0x7F) as usize;
    let per = if long { 2 } else { 4 };
    let mut prev: u16 = 0;
    let mut idx = 0usize;
    while idx < count && (w.nconns as usize) < w.conns.len() {
        let resp = match rings.cmd(cad, nid, VERB_GET_CONNECTION_ENTRY, (idx & 0xFF) as u32, a) {
            Some(v) => v,
            None => return,
        };
        a.verbs_get += 1;
        for slot in 0..per {
            if idx >= count || (w.nconns as usize) >= w.conns.len() {
                break;
            }
            let raw: u16 = if long {
                ((resp >> (slot * 16)) & 0xFFFF) as u16
            } else {
                ((resp >> (slot * 8)) & 0xFF) as u16
            };
            let range_bit: u16 = if long { 0x8000 } else { 0x80 };
            let mask: u16 = if long { 0x7FFF } else { 0x7F };
            let nid_val = raw & mask;
            if raw & range_bit != 0 && idx > 0 {
                // Range end: fill prev+1 ..= nid_val.
                let mut n = prev.saturating_add(1);
                while n <= nid_val && (w.nconns as usize) < w.conns.len() {
                    w.conns[w.nconns as usize] = n as u8;
                    w.nconns += 1;
                    n += 1;
                }
            } else {
                w.conns[w.nconns as usize] = nid_val as u8;
                w.nconns += 1;
            }
            prev = nid_val;
            idx += 1;
        }
    }
}

/// One derived output path: the converter, the widgets between it and the pin, and the pin.
#[derive(Clone, Copy, Default)]
struct Path {
    nodes: [u8; MAX_PATH_DEPTH], // pin first, converter last
    len: u8,
    /// For each hop, the index into THAT node's connection list that the path took. `sel[i]` is the
    /// index on `nodes[i]` that reaches `nodes[i + 1]`.
    sel: [u8; MAX_PATH_DEPTH],
    dac: u8,
    pin: u8,
}

/// Depth-first search from a pin back through connection lists to an Audio Output widget. Bounded
/// by `MAX_PATH_DEPTH` and by a visited set, so a cyclic or self-referential connection list ends
/// the search instead of the boot.
fn find_path(ws: &[Widget; MAX_NODES], pin: u8) -> Option<Path> {
    let mut stack: [(u8, u8); MAX_PATH_DEPTH] = [(0, 0); MAX_PATH_DEPTH];
    let mut visited = [false; MAX_NODES];
    let mut depth = 0usize;
    stack[0] = (pin, 0);
    visited[pin as usize] = true;
    loop {
        let (nid, next) = stack[depth];
        let w = &ws[nid as usize];
        if !w.present {
            if depth == 0 {
                return None;
            }
            depth -= 1;
            stack[depth].1 += 1;
            continue;
        }
        if w.kind == WT_AUDIO_OUT && depth > 0 {
            let mut p = Path::default();
            p.len = (depth + 1) as u8;
            for i in 0..=depth {
                p.nodes[i] = stack[i].0;
                p.sel[i] = stack[i].1;
            }
            p.pin = pin;
            p.dac = nid;
            return Some(p);
        }
        if (next as usize) < (w.nconns as usize) && depth + 1 < MAX_PATH_DEPTH {
            let child = w.conns[next as usize];
            if !visited[child as usize] {
                visited[child as usize] = true;
                depth += 1;
                stack[depth] = (child, 0);
                continue;
            }
            stack[depth].1 += 1;
            continue;
        }
        if depth == 0 {
            return None;
        }
        depth -= 1;
        stack[depth].1 += 1;
    }
}

/// Everything arc 2 needs about one codec, handed over by the walk.
struct CodecWalk {
    cad: u8,
    nodes: u32,
    dacs: u32,
    pins: u32,
    speaker_pin: Option<u8>,
    hp_pin: Option<u8>,
    path: Option<Path>,
    path_dev: u8,
}

/// Walk one codec: function groups, every widget under each audio function group, and the derived
/// output path.
fn walk_codec(rings: &mut Rings, cad: u8, ws: &mut [Widget; MAX_NODES], a: &mut Audit) -> Option<CodecWalk> {
    let vid = rings.cmd(cad, 0, VERB_GET_PARAMETER, PARAM_VENDOR_ID, a)?;
    a.verbs_get += 1;
    let rev = rings.cmd(cad, 0, VERB_GET_PARAMETER, PARAM_REVISION_ID, a)?;
    a.verbs_get += 1;
    serial_println!(
        "[hda] codec={} vid={:04x}:{:04x} rev={:#010x}",
        cad, (vid >> 16) & 0xFFFF, vid & 0xFFFF, rev
    );

    let sub = rings.cmd(cad, 0, VERB_GET_PARAMETER, PARAM_SUBNODE_COUNT, a)?;
    a.verbs_get += 1;
    let fg_start = ((sub >> 16) & 0xFF) as u16;
    let fg_count = (sub & 0xFF) as u16;

    let mut out = CodecWalk {
        cad,
        nodes: 0,
        dacs: 0,
        pins: 0,
        speaker_pin: None,
        hp_pin: None,
        path: None,
        path_dev: 0xFF,
    };
    // Ranked candidates: internal speaker beats headphone beats line out, and an analog pin beats a
    // digital one at every rank. `None` until a pin of that rank is found WITH a path behind it.
    let mut best: Option<(u8, Path, u8)> = None; // (rank, path, default-device)

    for fgi in 0..fg_count {
        let fg = (fg_start + fgi) as u8;
        let ftype = match rings.cmd(cad, fg, VERB_GET_PARAMETER, PARAM_FUNCTION_TYPE, a) {
            Some(v) => v,
            None => continue,
        };
        a.verbs_get += 1;
        serial_println!(
            "[hda] codec={} fg=0x{:02x} type={:#04x} ({})",
            cad, fg, ftype & 0xFF,
            if ftype & 0x7F == FG_TYPE_AUDIO { "audio" } else if ftype & 0x7F == 0x02 { "modem" } else { "other" }
        );
        if ftype & 0x7F != FG_TYPE_AUDIO {
            continue;
        }

        // The function group is powered up before its widgets are read: a codec in D3 is entitled
        // to answer a parameter read with stale or zeroed state. [HDA-SPEC §7.3.3.10]
        if rings.cmd(cad, fg, VERB_SET_POWER_STATE, 0, a).is_some() {
            a.verbs_set += 1;
        }

        let wsub = match rings.cmd(cad, fg, VERB_GET_PARAMETER, PARAM_SUBNODE_COUNT, a) {
            Some(v) => v,
            None => continue,
        };
        a.verbs_get += 1;
        let w_start = ((wsub >> 16) & 0xFF) as u16;
        let w_count = (wsub & 0xFF) as u16;

        for wi in 0..w_count {
            let nid_u16 = w_start + wi;
            if nid_u16 as usize >= MAX_NODES {
                break;
            }
            let nid = nid_u16 as u8;
            let caps = match rings.cmd(cad, nid, VERB_GET_PARAMETER, PARAM_WIDGET_CAPS, a) {
                Some(v) => v,
                None => continue,
            };
            a.verbs_get += 1;
            let kind = ((caps >> 20) & 0x0F) as u8;
            let mut w = Widget { nid, present: true, kind, caps, ..Default::default() };
            read_connections(rings, cad, nid, &mut w, a);

            if kind == WT_PIN {
                if let Some(pc) = rings.cmd(cad, nid, VERB_GET_PARAMETER, PARAM_PIN_CAPS, a) {
                    a.verbs_get += 1;
                    w.pincap = pc;
                }
                if let Some(cfg) = rings.cmd(cad, nid, VERB_GET_CONFIG_DEFAULT, 0, a) {
                    a.verbs_get += 1;
                    w.pincfg = cfg;
                }
                out.pins += 1;
            }
            if kind == WT_AUDIO_OUT {
                out.dacs += 1;
            }
            ws[nid as usize] = w;
            out.nodes += 1;

            // One line per node. The connection list is printed as the codec reports it, and the
            // pin decode is printed only for a pin — a `pincfg=-` on anything else is the honest
            // answer, not a zero.
            let mut conns = [0u8; 8];
            conns[..w.nconns as usize].copy_from_slice(&w.conns[..w.nconns as usize]);
            if kind == WT_PIN {
                let dev = ((w.pincfg >> 20) & 0x0F) as u8;
                let conn = ((w.pincfg >> 16) & 0x0F) as u8;
                let gross = ((w.pincfg >> 28) & 0x03) as u8;
                let colour = ((w.pincfg >> 12) & 0x0F) as u8;
                let portc = ((w.pincfg >> 30) & 0x03) as u8;
                serial_println!(
                    "[hda] node=0x{:02x} type={} conns={:?} pincap={:#010x} out={} hp={} eapd={} pincfg={:#010x} (dev={} loc={} conn={} colour={} portconn={} assoc={} seq={})",
                    nid, widget_kind_name(kind), &conns[..w.nconns as usize], w.pincap,
                    (w.pincap & PINCAP_OUTPUT != 0) as u8,
                    (w.pincap & PINCAP_HP_DRIVE != 0) as u8,
                    (w.pincap & PINCAP_EAPD != 0) as u8,
                    w.pincfg, pin_device_name(dev), pin_location_name(gross), pin_conn_name(conn),
                    pin_colour_name(colour), portc, (w.pincfg >> 4) & 0x0F, w.pincfg & 0x0F
                );
            } else {
                // `amp-override=0` matters and is why it is printed: with the override bit clear a
                // widget's amplifier capabilities are the FUNCTION GROUP's defaults rather than its
                // own, so a reader comparing two nodes' gain ranges needs to know which of the two
                // they are looking at. [HDA-SPEC §7.3.4.6]
                serial_println!(
                    "[hda] node=0x{:02x} type={} conns={:?} caps={:#010x} out-amp={} in-amp={} amp-override={} power-ctl={} digital={} pincfg=-",
                    nid, widget_kind_name(kind), &conns[..w.nconns as usize], caps,
                    w.out_amp() as u8, w.in_amp() as u8, (caps & WCAP_AMP_OVERRIDE != 0) as u8,
                    (caps & WCAP_POWER_CTL != 0) as u8, (caps & WCAP_DIGITAL != 0) as u8
                );
            }
        }
    }

    // Path derivation, after the whole graph is known — a pin's converter can be a node with a
    // higher NID than the pin, so this cannot be done inside the loop above.
    for nid in 0..MAX_NODES {
        let w = ws[nid];
        if !w.present || w.kind != WT_PIN || w.pincap & PINCAP_OUTPUT == 0 {
            continue;
        }
        let dev = ((w.pincfg >> 20) & 0x0F) as u8;
        let gross = ((w.pincfg >> 28) & 0x03) as u8;
        let portc = ((w.pincfg >> 30) & 0x03) as u8;
        // Port connectivity 1 is "no physical connection" — a pin the board does not wire to
        // anything. It is walked and printed, and it is never a tone candidate.
        if portc == 0x1 {
            continue;
        }
        if dev == DEV_SPEAKER && out.speaker_pin.is_none() {
            out.speaker_pin = Some(w.nid);
        }
        if dev == DEV_HP_OUT && out.hp_pin.is_none() {
            out.hp_pin = Some(w.nid);
        }
        // RANK. The brief's target is the INTERNAL SPEAKER; the QEMU fixture codec exposes a
        // line-out and no speaker at all, so the ranking is written as a preference over output
        // devices rather than as a speaker-only match — and the chosen device is PRINTED on the
        // walk line, so a reader always knows which of the three a given boot drove.
        let rank: u8 = match (dev, gross) {
            (DEV_SPEAKER, 0x1) => 0, // internal speaker — the rMBP's target
            (DEV_SPEAKER, _) => 1,
            (DEV_HP_OUT, _) => 2,
            (DEV_LINE_OUT, _) => 3, // the QEMU hda-duplex codec's only output
            _ => continue,
        };
        let rank = rank + if w.caps & WCAP_DIGITAL != 0 { 8 } else { 0 };
        if let Some(p) = find_path(ws, w.nid) {
            let better = match best {
                None => true,
                Some((r, _, _)) => rank < r,
            };
            if better {
                best = Some((rank, p, dev));
            }
        } else {
            serial_println!(
                "[hda] pin=0x{:02x} dev={} output-capable but NO converter reachable within {} hops — not a tone candidate",
                w.nid, pin_device_name(dev), MAX_PATH_DEPTH
            );
        }
    }
    if let Some((_, p, dev)) = best {
        out.path = Some(p);
        out.path_dev = dev;
    }
    Some(out)
}
// ════════════════════════════════════════════════════════════════════════════════════════════════
// Step 1 — claim the controller
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Report the function's config state raw, enable memory decode AND bus master, then map BAR0
/// uncacheable.
///
/// **Bus Master IS enabled**: HDA has no PIO data path at all — the controller masters the bus to
/// fetch the CORB, to write the RIRB, to fetch the BDL and to fetch sample data. A driver that
/// mapped BAR0 without bus master would program everything correctly and watch the RIRB write
/// pointer never move. [HDA-SPEC §1.2]
///
/// **BAR0 is the register block**, at config offset 0x10, and it may be a 64-bit BAR — unlike the
/// AHCI ABAR case, where the register block is the LAST BAR of the header and therefore cannot be.
/// [HDA-SPEC §2.1]
fn take(bus: u8, slot: u8, func: u8, vend: u16, devid: u16, a: &mut Audit) -> Option<u64> {
    let bar0_raw = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x10) };
    let command = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };

    if bar0_raw & 0x1 != 0 {
        serial_println!(
            "[hda] bdf {}:{}.{} {:04x}:{:04x} BAR0 is an I/O BAR (io={:#x}) — not an HDA register block, skipped",
            bus, slot, func, vend, devid, bar0_raw & 0xFFFF_FFFC
        );
        return None;
    }
    let base = if bar0_raw & 0x06 == 0x04 {
        let hi = unsafe { crate::arch::pci::read_config_32(bus, slot, func, 0x14) };
        ((bar0_raw & 0xFFFF_FFF0) as u64) | ((hi as u64) << 32)
    } else {
        (bar0_raw & 0xFFFF_FFF0) as u64
    };
    if base == 0 {
        serial_println!(
            "[hda] bdf {}:{}.{} {:04x}:{:04x} BAR0 unassigned by firmware — no MMIO probe",
            bus, slot, func, vend, devid
        );
        return None;
    }

    if command & 0x0006 != 0x0006 {
        unsafe { crate::arch::pci::write_config_16(bus, slot, func, 0x04, command | 0x0006) };
        a.cfg += 1;
        let after = unsafe { crate::arch::pci::read_config_16(bus, slot, func, 0x04) };
        serial_println!(
            "[hda] claim bdf {}:{}.{} cmd {:#06x} -> {:#06x} (mem-decode + bus-master; DMA is the only data path HDA has)",
            bus, slot, func, command, after
        );
        if after & 0x0006 != 0x0006 {
            serial_println!(
                "[hda] claim bdf {}:{}.{} decode/bus-master did not stick (cmd={:#06x}) — controller not claimable",
                bus, slot, func, after
            );
            return None;
        }
    }

    // Identity-map the register block Uncacheable — the same seam `drivers/ahci.rs`, `drivers/sdhc.rs`
    // and the GPU drivers use. Creating a mapping is a page-table edit, not a device access; the
    // controller sees nothing. 0x2000 covers the global registers (0x00..0x80) and 32 stream
    // descriptors at 0x20 each plus the alias window above them; the identity map's leaves are
    // 2 MiB, so this types the containing leaf UC either way. [TREE]
    crate::arch::memory::map_mmio_window(base, 0x2000);
    serial_println!("[hda] map bdf {}:{}.{} bar0={:#x} len={:#x} uncacheable", bus, slot, func, base, 0x2000);
    Some(base)
}

/// [HDA-SPEC §4.2.2 / §3.3.7] Controller reset. Both engines are stopped first (a controller the
/// firmware left running would keep DMAing across the reset window on some silicon), `CRST` is
/// driven low and observed low, then driven high and observed high, and the specification's 521 µs
/// codec-discovery window is waited out before `STATESTS` is read.
fn reset(base: u64, a: &mut Audit) -> Option<u16> {
    w8(base, REG_CORBCTL, 0);
    w8(base, REG_RIRBCTL, 0);
    a.ctrl += 2;

    let gctl = r32(base, REG_GCTL);
    w32(base, REG_GCTL, gctl & !GCTL_CRST);
    a.ctrl += 1;
    if !wait_us(1000, || r32(base, REG_GCTL) & GCTL_CRST == 0) {
        serial_println!("[hda] reset REFUSED reason=crst-would-not-assert gctl={:#010x}", r32(base, REG_GCTL));
        return None;
    }
    // The specification requires the reset be held; 100 µs is comfortably past the link's settling
    // requirement and is what every controller this file has been run against needs.
    delay_us(100);

    w32(base, REG_GCTL, r32(base, REG_GCTL) | GCTL_CRST);
    a.ctrl += 1;
    if !wait_us(1000, || r32(base, REG_GCTL) & GCTL_CRST != 0) {
        serial_println!("[hda] reset REFUSED reason=crst-would-not-deassert gctl={:#010x}", r32(base, REG_GCTL));
        return None;
    }

    // [HDA-SPEC §4.3] Codecs report their presence within 521 µs of CRST reading 1. Reading
    // STATESTS before that window closes is how a driver decides a codec is absent that is merely
    // slow; the wait is unconditional and is not a poll.
    delay_us(600);
    let statests = r16(base, REG_STATESTS);
    // STATESTS is RW1C: the presence bits are LATCHES, and leaving them set would make a later
    // reader see a state change that already happened. Read first, then clear what was read.
    w16(base, REG_STATESTS, statests);
    a.ctrl += 1;
    Some(statests)
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// Entry point — one pass, at enumeration time. Called from `drivers/pci.rs`.
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The whole driver, in one pass. Every exit path stops the CORB and RIRB DMA engines.
pub fn probe() {
    let mut a = Audit::default();
    let found = crate::drivers::pci::PciScanner::audio_inventory();
    if found.is_empty() {
        serial_println!("[hda] absent");
        return;
    }

    // ONE controller. A machine with two HDA functions (a discrete GPU's HDMI audio beside the
    // PCH's analog one) is real — the bench rMBP has exactly that shape — so the census prints
    // every one and this loop claims the FIRST that resets and answers, then stops. Driving two at
    // once is an arc 3 question, not a thing to do quietly here.
    for (bus, slot, func, vend, devid) in found.iter().copied() {
        let base = match take(bus, slot, func, vend, devid, &mut a) {
            Some(b) => b,
            None => continue,
        };

        let gcap = r16(base, REG_GCAP);
        let oss = (gcap >> 12) & 0x0F;
        let iss = (gcap >> 8) & 0x0F;
        let bss = (gcap >> 3) & 0x1F;
        let ok64 = gcap & 0x1;
        let vmaj = r8(base, REG_VMAJ);
        let vmin = r8(base, REG_VMIN);
        serial_println!(
            "[hda] gcap={:#06x} oss={} iss={} bss={} nsdo={} 64ok={} version={}.{}",
            gcap, oss, iss, bss, (gcap >> 1) & 0x3, ok64, vmaj, vmin
        );
        if oss == 0 {
            serial_println!("[hda] REFUSED reason=no-output-streams gcap={:#06x} — nothing to program", gcap);
            continue;
        }

        let statests = match reset(base, &mut a) {
            Some(s) => s,
            None => continue,
        };
        let mut codecs = [0u8; MAX_CODECS];
        let mut ncodecs = 0usize;
        for c in 0..MAX_CODECS {
            if statests & (1 << c) != 0 {
                codecs[ncodecs] = c as u8;
                ncodecs += 1;
            }
        }
        serial_println!(
            "[hda] reset crst={} statests={:#06x} codecs={:?}",
            r32(base, REG_GCTL) & GCTL_CRST, statests, &codecs[..ncodecs]
        );
        if ncodecs == 0 {
            serial_println!("[hda] REFUSED reason=no-codec statests={:#06x} — the link reported no codec after the 521us discovery window", statests);
            continue;
        }

        let mut rings = match Rings::init(base, &mut a) {
            Some(r) => r,
            None => continue,
        };

        // The widget table is one node-address space per codec, reused across codecs — 256 widgets
        // of a few dozen bytes is ~14 KiB of stack if it were a local, so it is boxed on the heap.
        let mut ws: alloc::boxed::Box<[Widget; MAX_NODES]> =
            alloc::boxed::Box::new([Widget::default(); MAX_NODES]);

        let mut total_nodes = 0u32;
        let mut total_dacs = 0u32;
        let mut total_pins = 0u32;
        let mut chosen: Option<CodecWalk> = None;
        let mut spk_line = 0xFFu16;
        let mut hp_line = 0xFFu16;

        for ci in 0..ncodecs {
            for w in ws.iter_mut() {
                *w = Widget::default();
            }
            let cw = match walk_codec(&mut rings, codecs[ci], &mut ws, &mut a) {
                Some(c) => c,
                None => {
                    serial_println!("[hda] codec={} walk INCOMPLETE — the codec stopped answering", codecs[ci]);
                    continue;
                }
            };
            total_nodes += cw.nodes;
            total_dacs += cw.dacs;
            total_pins += cw.pins;
            if let Some(p) = cw.speaker_pin {
                if spk_line == 0xFF {
                    spk_line = p as u16;
                }
            }
            if let Some(p) = cw.hp_pin {
                if hp_line == 0xFF {
                    hp_line = p as u16;
                }
            }
            if cw.path.is_some() && chosen.is_none() {
                chosen = Some(cw);
            }
        }

        // The summary. `speaker_pin`/`hp_pin` print as `none` rather than as a zero when the codec
        // has no pin of that device class: 0x00 is a legal NID and must never be confused with an
        // absence (LAWS §5, "a zero is a fact about the data or about the pattern").
        let path_desc = match chosen.as_ref().and_then(|c| c.path.as_ref()) {
            Some(p) => {
                let mut nodes = [0u8; MAX_PATH_DEPTH];
                nodes[..p.len as usize].copy_from_slice(&p.nodes[..p.len as usize]);
                serial_println!(
                    "[hda] path codec={} dac=0x{:02x} -> {:?} -> pin=0x{:02x} dev={} hops={}",
                    chosen.as_ref().map(|c| c.cad).unwrap_or(0xFF),
                    p.dac, &nodes[..p.len as usize], p.pin,
                    pin_device_name(chosen.as_ref().map(|c| c.path_dev).unwrap_or(0xFF)),
                    p.len
                );
                "found"
            }
            None => "none",
        };
        if spk_line == 0xFF {
            serial_println!(
                "[hda] walk codecs={} nodes={} dacs={} pins={} speaker_pin=none hp_pin={} path={}",
                ncodecs, total_nodes, total_dacs, total_pins,
                if hp_line == 0xFF { alloc::format!("none") } else { alloc::format!("0x{:02x}", hp_line) },
                path_desc
            );
        } else {
            serial_println!(
                "[hda] walk codecs={} nodes={} dacs={} pins={} speaker_pin=0x{:02x} hp_pin={} path={}",
                ncodecs, total_nodes, total_dacs, total_pins, spk_line,
                if hp_line == 0xFF { alloc::format!("none") } else { alloc::format!("0x{:02x}", hp_line) },
                path_desc
            );
        }
        a.line("walk");

        // The arc 1 verdict. Printed only here, on a controller that reset, answered and was
        // walked — so its silence is "no HDA controller answered", never "the walk passed".
        let arc1_ok = ncodecs > 0 && total_nodes > 0 && total_dacs > 0 && total_pins > 0;
        serial_println!(
            ":: HDA: codecs={} nodes={} dacs={} pins={} path={} -> {} ::",
            ncodecs, total_nodes, total_dacs, total_pins, path_desc,
            if arc1_ok { "PASS" } else { "FAIL" }
        );

        rings.stop(&mut a);
        a.line("end");
        // One controller per boot; see the comment at the top of this loop.
        return;
    }
}
