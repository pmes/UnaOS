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
//! **Arc 2 (`hda-tone`) does:** program output stream 0 with a 16-bit stereo 48 kHz sine (440 Hz,
//! one second, low amplitude) through a two-entry BDL, bind the stream tag to EVERY converter of
//! the derived pin's ASSOCIATION (an internal speaker pair is two pins with two converters), power
//! and enable each pin, unmute the amplifier on every node of every member's path, drive the codec
//! function group's whole declared GPIO set (the CS4206 has no EAPD and its speaker amplifier is
//! not reachable any other way the specification defines), RUN the stream, and then PROVE it ran
//! without ears — `LPIB` advancing plus the BDL walked to its end, by a cyclic-buffer WRAP or a
//! polled `BCIS` — before stopping and restoring every register it changed.
//!
//! **Does NOT — 1: no interrupts.** `INTCTL` is never written, so `GIE`/`CIE`/`SIE` stay 0 and the
//! controller raises no message. `SDnCTL.IOCE` IS set, which is what makes `SDnSTS.BCIS` latch on a
//! buffer boundary, but with `INTCTL` at 0 that latch reaches no CPU: it is a polled flag. Every
//! wait in this file is a bounded TSC spin, the same discipline `drivers/ahci.rs` and
//! `drivers/sdhc.rs` use, because everything here happens inside `pci::init` where `arch::ms()`
//! does not advance.
//!
//! **Does NOT — 2: nothing runs on a codec whose walk found no output path.** Arc 2 prints
//! `[hda] tone REFUSED reason=no-speaker-path` and issues not one stream register write.
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
//! [hda] pair member=1/2 pin=0x.. dac=0x.. dev=speaker assoc=. seq=. chan=. stereo=. primary=.
//! [hda] bdl entry=0/1 addr=0x... len=... ioc=1 flags=0x00000001 (readback)
//! [hda] gpio afg=0x.. caps=0x........ gpios=N ... mask=0x.. enable=..->.. dir=..->.. data=..->.. -> set|none
//! [hda] bind member=. dac=0x.. want_tag=1 want_chan=. bound=0x.. tag=. chan=. ok=.
//! [hda] power member=. dac=0x.. set=D. actual=D. ... pin=0x.. set=D. actual=D. ...
//! [hda] amp member=. dac=0x.. gain=0x.... mute=. ... pinctl=0x.. out_en=. fmt_conv=0x.... fmt_match=.
//! [hda] tone stream=0 lpib=... -> ... bcis=... fifo_ready=... run_ms=... tag_ok=... wraps=... consumed=... rate_bps=...
//! :: HDA: codecs=<n> nodes=<n> path=<found|none> -> PASS|FAIL ::
//! :: HDA-TONE: lpib_advanced=<0|1> walked=<0|1> wraps=<n> bcis=<n> -> PASS|FAIL ::
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
    verbs_set: u32, #[cfg(feature = "hda-sie")] intctl: u32, // codec verbs that change codec state; HDASIE (B207): the INTCTL write count, a field only when the knob is on so the knob-off struct is the struct it always was
}

impl Audit {
    #[cfg(not(feature = "hda-sie"))] fn line(&self, stage: &str) { // HDASIE (B207): with the knob ON this body is replaced by the file-tail `impl Audit` whose `wrote-intctl=` is a COUNT, because an audited zero must never be printed over a register this driver has written
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
    // ⚠ HDAAMP — THE TIE-BREAK IS SEQUENCE, AND IT USED TO BE NID ORDER BY ACCIDENT. The tuple is
    // (rank, SEQUENCE, path, default-device). Rank alone left two pins of the SAME association at
    // the same score — the rMBP's two internal speaker pins, 0x0a (`assoc=1 seq=2`) and 0x0b
    // (`assoc=1 seq=0`), measured on flight 11 — and a strictly-smaller-rank replacement then hands
    // the tie to whichever pin the ascending-NID scan reached first. That is NID order dressed as a
    // preference. [HDA-SPEC §7.3.3.31] makes sequence the ordering within an association and
    // sequence 0 its PRIMARY member, so the specification supplies the tie-break the rank was
    // missing. (Arc 2 then drives every member of the association, not only this one.)
    let mut best: Option<(u8, u8, Path, u8)> = None; // (rank, sequence, path, default-device)

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
        let seq = (w.pincfg & 0x0F) as u8;
        if let Some(p) = find_path(ws, w.nid) {
            let better = match best {
                None => true,
                Some((r, sq, _, _)) => rank < r || (rank == r && seq < sq),
            };
            if better {
                best = Some((rank, seq, p, dev));
            }
        } else {
            serial_println!(
                "[hda] pin=0x{:02x} dev={} output-capable but NO converter reachable within {} hops — not a tone candidate",
                w.nid, pin_device_name(dev), MAX_PATH_DEPTH
            );
        }
    }
    if let Some((_, _, p, dev)) = best {
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
        ); #[cfg(feature = "hda-tone")] run_tone(base, &mut rings, iss as u8, chosen.as_ref(), &ws, &mut a); // ARC 2's ONE call site, and the ONLY `hda-tone` statement above this file's arc-2 banner. HERE because the tone must run on a walked controller and before the rings are stopped. FOLDED onto the arc-1 statement's closing line rather than given lines of its own so that arc 1's line numbering is identical with and without arc 2 — the same LINE-NEUTRAL discipline the hook in `drivers/pci.rs` uses, for the same `panic::Location` reason. The append goes BEFORE this line's first `//` (LEDGER P7 — after it the statement is a comment, compiles nothing, and the check stays green).

        rings.stop(&mut a);
        a.line("end");
        // One controller per boot; see the comment at the top of this loop.
        return;
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// ARC 2 — THE TONE. Everything below this line is `hda-tone`, and above it there is exactly ONE
// `hda-tone` statement: the call site, FOLDED onto an existing line inside `probe` so arc 1's line
// numbering does not move. Knob off, not one line below this banner is lexed — no stream-descriptor
// constant, no verb this driver needs only in order to make a noise, no sine generator, no BDL and
// no stream register write exists in the image, and no `core::panic::Location` above it can move.
// A file-tail split rather than a scatter of attributes, precisely so that claim is checkable by
// reading one line number instead of auditing thirty-odd cfgs.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "hda-tone")]
mod tone {
    use super::*;

    // ── The arc-2 half of the register and verb transcription. It lives INSIDE this module, not
    // beside arc 1's table at the top of the file, so that the WHOLE of arc 2 — every constant,
    // the 4-bit verb form, the sine generator and the stream itself — is one contiguous file
    // tail. Knob off, nothing below this banner is lexed and no `panic::Location` above it can
    // move. [HDA-SPEC §3.3.35 ff., §7.3.3]
    // ── Stream descriptors, at 0x80 + n * 0x20. [HDA-SPEC §3.3.35 ff.] ──────────────────────────────
    pub const SD_BASE: u64 = 0x80;
    pub const SD_STRIDE: u64 = 0x20;
    pub const SD_CTL: u64 = 0x00; // 24-bit, read/written here as 8-bit low + 8-bit high [HDA-SPEC §3.3.35]
    pub const SD_STS: u64 = 0x03; //  8-bit                                             [HDA-SPEC §3.3.36]
    pub const SD_LPIB: u64 = 0x04; // 32-bit — Link Position In Buffer                  [HDA-SPEC §3.3.37]
    pub const SD_CBL: u64 = 0x08; // 32-bit — Cyclic Buffer Length                      [HDA-SPEC §3.3.38]
    pub const SD_LVI: u64 = 0x0C; // 16-bit — Last Valid Index                          [HDA-SPEC §3.3.39]
    pub const SD_FMT: u64 = 0x12; // 16-bit — Format                                    [HDA-SPEC §3.3.41]
    pub const SD_BDPL: u64 = 0x18; // 32-bit — BDL Pointer Lower                        [HDA-SPEC §3.3.42]
    pub const SD_BDPU: u64 = 0x1C; // 32-bit — BDL Pointer Upper                        [HDA-SPEC §3.3.43]
    pub const SDCTL_SRST: u32 = 1 << 0; // Stream Reset
    pub const SDCTL_RUN: u32 = 1 << 1; // Stream Run
    pub const SDCTL_IOCE: u32 = 1 << 2; // Interrupt On Completion Enable
    pub const SDSTS_BCIS: u8 = 1 << 2; // Buffer Completion Interrupt Status (RW1C)
    pub const SDSTS_FIFOE: u8 = 1 << 3; // FIFO Error (RW1C)
    pub const SDSTS_DESE: u8 = 1 << 4; // Descriptor Error (RW1C)
    pub const SDSTS_FIFORDY: u8 = 1 << 5; // FIFO Ready
    pub const VERB_SET_CONNECTION_SELECT: u32 = 0x701; //                       [HDA-SPEC §7.3.3.2]
    pub const VERB_GET_STREAM_CHANNEL: u32 = 0xF06; //                          [HDA-SPEC §7.3.3.11]
    pub const VERB_SET_STREAM_CHANNEL: u32 = 0x706; //                          [HDA-SPEC §7.3.3.11]
    pub const VERB_GET_PIN_CONTROL: u32 = 0xF07; //                             [HDA-SPEC §7.3.3.13]
    pub const VERB_SET_PIN_CONTROL: u32 = 0x707; //                             [HDA-SPEC §7.3.3.13]
    pub const VERB_GET_EAPD: u32 = 0xF0C; //                                    [HDA-SPEC §7.3.3.16]
    pub const VERB_SET_EAPD: u32 = 0x70C; //                                    [HDA-SPEC §7.3.3.16]
    pub const VERB_GET_POWER_STATE: u32 = 0xF05; //                             [HDA-SPEC §7.3.3.10]
    pub const VERB_GET_AMP_GAIN_MUTE: u32 = 0xB00; //                           [HDA-SPEC §7.3.3.7]
    pub const VERB_SET_AMP_GAIN_MUTE: u32 = 0x300; // 4-bit verb, 16-bit payload[HDA-SPEC §7.3.3.7]
    pub const VERB_GET_CONVERTER_FORMAT: u32 = 0xA00; //                        [HDA-SPEC §7.3.3.8]
    pub const VERB_SET_CONVERTER_FORMAT: u32 = 0x200; // 4-bit verb             [HDA-SPEC §7.3.3.8]
    pub const PARAM_IN_AMP_CAPS: u32 = 0x0D; //                                 [HDA-SPEC §7.3.4.10]
    pub const PARAM_OUT_AMP_CAPS: u32 = 0x12; //                                [HDA-SPEC §7.3.4.12]
    pub const PINCTL_OUT_ENABLE: u8 = 1 << 6; // [HDA-SPEC §7.3.3.13]
    pub const PINCTL_HP_ENABLE: u8 = 1 << 7;
    pub const EAPD_ENABLE: u8 = 1 << 1; // [HDA-SPEC §7.3.3.16]

    // ── HDAAMP: the GPIO group. [HDA-SPEC §7.3.3, verb identifiers 0xF15/0x715, 0xF16/0x716,
    // 0xF17/0x717; §7.3.4.14 for the parameter] The VERB NUMBER is the citable constant and is
    // written beside each name, because a reader checking this file against the specification's
    // verb table looks the identifier up, not the prose.
    //
    // ⚠ WHY THIS EXISTS AT ALL. On the bench codec (Cirrus CS4206, `vid=1013:4206`) BOTH internal
    // speaker pins report `eapd=0` — `PIN_CAPS` bit 16 CLEAR, measured on flight 11 — so the
    // external-amplifier bit this driver already drives is ABSENT on this part and cannot be the
    // thing that unmutes the speakers. The function group's GPIO pins are the only other codec-side
    // output the specification defines, and the specification gives NO map from a GPIO to a board's
    // amplifier: that wiring is the machine's, not the standard's, and this tree has no legal
    // source for it. So the arc does the one spec-legal thing available: it reads how many GPIOs
    // the codec DECLARES, drives the whole declared set as one, PRINTS every word it read and
    // wrote, and restores all three registers at stream stop. The ear answers whether the set
    // matters; the next flight narrows it. A codec that declares none (QEMU's `hda-duplex`) prints
    // `gpio … -> none` and is not written at all.
    pub const PARAM_GPIO_COUNT: u32 = 0x11; //                                  [HDA-SPEC §7.3.4.14]
    pub const VERB_GET_GPIO_DATA: u32 = 0xF15; //                               [HDA-SPEC §7.3.3]
    pub const VERB_SET_GPIO_DATA: u32 = 0x715;
    pub const VERB_GET_GPIO_ENABLE: u32 = 0xF16; //                             [HDA-SPEC §7.3.3]
    pub const VERB_SET_GPIO_ENABLE: u32 = 0x716;
    pub const VERB_GET_GPIO_DIRECTION: u32 = 0xF17; // 1 = output               [HDA-SPEC §7.3.3]
    pub const VERB_SET_GPIO_DIRECTION: u32 = 0x717;

    /// `PARAM_WIDGET_CAPS` bit 0 — Stereo. A stereo converter consumes BOTH channels of a
    /// two-channel stream, so its `SET_CONVERTER_STREAM_CHANNEL` starting channel is 0; a mono
    /// converter takes one, and then the association's sequence order IS the channel order.
    /// [HDA-SPEC §7.3.4.6, §7.3.3.11]
    pub const WCAP_STEREO: u32 = 1 << 0;

    /// How many pins of one association this arc drives at once. An internal stereo speaker pair is
    /// two; the bound is what keeps a malformed default-configuration table from turning the tone
    /// into a walk of every pin on the codec. [HDA-SPEC §7.3.3.31]
    pub const PAIR_MAX: usize = 2;

    /// Elapsed milliseconds since `start` (a `now_cycles()` reading). Arc 2 only: arc 1's every wait
    /// is a predicate with a microsecond budget, and nothing in the walk reports a duration.
    pub fn elapsed_ms(start: u64) -> u64 {
        let hz = crate::arch::apic::tsc_hz();
        if hz == 0 {
            return 0;
        }
        crate::arch::now_cycles().wrapping_sub(start).saturating_mul(1000) / hz
    }

    /// 48 kHz, 16-bit, stereo. `SDnFMT`/converter-format encoding: BASE=0 (48 kHz family),
    /// MULT=000, DIV=000, BITS=001 (16 bit), CHAN=0001 (two channels, encoded as count-1).
    /// [HDA-SPEC §3.3.41 table 53]
    pub const FMT_48K_16_STEREO: u16 = 0x0011;
    pub const SAMPLE_RATE: usize = 48_000;
    pub const TONE_HZ: usize = 440;
    /// One second. 48000 frames x 4 bytes = 192000, which is 1500 x 128 — the cyclic buffer length
    /// is a multiple of 128 bytes, as the specification requires. [HDA-SPEC §3.3.38]
    pub const FRAMES: usize = SAMPLE_RATE;
    pub const PCM_BYTES: usize = FRAMES * 4;
    /// Low amplitude: about -18 dBFS. This is a diagnostic tone on a laptop speaker, not a test
    /// signal — a full-scale sine out of a cold boot is how you frighten an operator.
    pub const AMPLITUDE: i32 = 4096;
    /// Two BDL entries, which is the minimum the specification allows, each half the buffer and
    /// each with IOC set. [HDA-SPEC §3.6.2]
    pub const BDL_ENTRIES: usize = 2;
    pub const BDL_BYTES: usize = BDL_ENTRIES * 16;
    pub const BDL_ALIGN: usize = 128;
    /// The stream tag written into `SDnCTL[23:20]` and into the converter's
    /// `SET_CONVERTER_STREAM_CHANNEL`. Tag 0 means "unused" — a stream programmed with tag 0 is a
    /// stream the link will never carry, which is exactly the go-red mutation this arc names.
    /// [HDA-SPEC §3.3.35, §7.3.3.11]
    pub const STREAM_TAG: u32 = 1;
    /// How long the stream is allowed to run before it is stopped and scored.
    pub const RUN_MS: u64 = 1200;

    /// A fixed-point sine over one period, computed without `libm` (the kernel has no float
    /// runtime): a 5th-order Taylor polynomial on the first quadrant, mirrored into the other
    /// three. `x` is the phase in units of 1/1024 of a full turn; the result is Q15.
    fn sin_q15(phase: u32) -> i32 {
        const Q: u32 = 256; // quarter turn
        let p = phase % (4 * Q);
        let (quad, off) = (p / Q, p % Q);
        let x = match quad {
            0 | 2 => off,
            _ => Q - off,
        };
        // x in [0, 256] maps to [0, pi/2]. Work in Q15 on the normalised argument t = x / 256.
        let t = (x as i64 * 32768) / Q as i64; // Q15 in [0, 1]
        // sin(pi/2 * t) approximated by the odd polynomial 1.5708 t - 0.6459 t^3 + 0.0796 t^5,
        // which is the minimax-flavoured Taylor truncation and is within 1e-4 over [0,1] — far
        // inside the quantisation of a 16-bit sample.
        let t2 = (t * t) >> 15;
        let t3 = (t2 * t) >> 15;
        let t5 = (t3 * t2) >> 15;
        let s = (51472 * t - 21166 * t3 + 2608 * t5) >> 15; // coefficients in Q15
        let s = s.clamp(-32768, 32767) as i32;
        if quad >= 2 { -s } else { s }
    }

    /// Fill the PCM buffer with a 440 Hz sine, interleaved stereo, 16-bit little-endian.
    pub fn fill(buf: u64) {
        for f in 0..FRAMES {
            // Phase in 1/1024-turn units, accumulated in integers so there is no drift.
            let phase = ((f * TONE_HZ * 1024) / SAMPLE_RATE) as u32;
            let s = ((sin_q15(phase) * AMPLITUDE) >> 15) as i16;
            unsafe {
                core::ptr::write_volatile((buf + (f as u64) * 4) as *mut i16, s);
                core::ptr::write_volatile((buf + (f as u64) * 4 + 2) as *mut i16, s);
            }
        }
    }

    /// Every codec register this run changes, read before it is written and written back after.
    #[derive(Default, Clone, Copy)]
    pub struct Saved {
        pub pinctl: u8,
        pub pinctl_ok: bool,
        pub eapd: u8,
        pub eapd_ok: bool,
        pub fmt: u16,
        pub fmt_ok: bool,
        pub strm: u8,
        pub strm_ok: bool,
        pub power: [u8; MAX_PATH_DEPTH],
        pub power_ok: [bool; MAX_PATH_DEPTH],
        pub out_amp: [u16; MAX_PATH_DEPTH],
        pub out_amp_ok: [bool; MAX_PATH_DEPTH],
    }

    /// `SET_AMPLIFIER_GAIN_MUTE` payload. [HDA-SPEC §7.3.3.7 figure 74]
    /// bit15 set-output, bit14 set-input, bit13 left, bit12 right, bits11:8 index, bit7 mute,
    /// bits6:0 gain.
    pub fn amp_payload(output: bool, index: u8, mute: bool, gain: u8) -> u32 {
        let mut p: u32 = (1 << 13) | (1 << 12); // both channels
        if output { p |= 1 << 15 } else { p |= 1 << 14 }
        p |= ((index as u32) & 0xF) << 8;
        if mute {
            p |= 1 << 7;
        }
        p |= (gain as u32) & 0x7F;
        p
    }

    /// A moderate, unmuted gain from an amplifier-capability word: the 0 dB offset the codec
    /// itself declares (bits 6:0), clamped to the declared step count (bits 14:8). Never the
    /// maximum — this is a diagnostic tone. [HDA-SPEC §7.3.4.10, §7.3.4.12]
    pub fn moderate_gain(ampcaps: u32) -> u8 {
        let steps = ((ampcaps >> 8) & 0x7F) as u8;
        let offset = (ampcaps & 0x7F) as u8;
        if steps == 0 {
            return 0;
        }
        if offset == 0 || offset > steps { steps } else { offset }
    }
}

#[cfg(feature = "hda-tone")]
impl Rings {
    /// Issue a 4-bit-verb / 16-bit-payload command (`SET_CONVERTER_FORMAT`, `SET_AMP_GAIN_MUTE`).
    /// Arc 2 only: every verb the walk issues is the 12-bit form. [HDA-SPEC §7.3.1]
    fn cmd16(&mut self, cad: u8, nid: u8, verb: u32, payload: u32, a: &mut Audit) -> Option<u32> {
        let word = ((cad as u32 & 0xF) << 28) | ((nid as u32) << 20) | ((verb << 8) | (payload & 0xFFFF)) & 0x000F_FFFF;
        self.issue(word, a)
    }
}

/// Program output stream 0, run it, prove it ran, stop it and restore everything.
///
/// `iss` is `GCAP.ISS` — the input stream descriptors come FIRST in the descriptor array, so output
/// stream 0's descriptor is at `0x80 + ISS * 0x20`. A driver that assumed descriptor 0 was an
/// output stream would program an INPUT engine and then report a stuck LPIB as a broken codec.
/// [HDA-SPEC §3.3.35]
#[cfg(feature = "hda-tone")]
fn run_tone(base: u64, rings: &mut Rings, iss: u8, walk: Option<&CodecWalk>, ws: &[Widget; MAX_NODES], a: &mut Audit) {
    use tone::*;

    let (cad, path) = match walk.and_then(|c| c.path.as_ref().map(|p| (c.cad, *p))) {
        Some(v) => v,
        None => {
            serial_println!("[hda] tone REFUSED reason=no-speaker-path");
            serial_println!(":: HDA-TONE: reason=no-speaker-path -> REFUSED ::");
            return;
        }
    };

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // HDAAMP DEFECT 3 — AN ASSOCIATION IS A SET OF PINS, AND ARC 2 DROVE ONE OF THEM.
    //
    // Flight 11 measured two internal speaker pins on the CS4206, both `dev=speaker loc=internal`,
    // both with a converter of their own:
    //
    //     node=0x0a … pincfg=0x90100112 (… assoc=1 seq=2)  conns=[3]
    //     node=0x0b … pincfg=0x90100110 (… assoc=1 seq=0)  conns=[4]
    //
    // and the path line chose `dac=0x03 -> [10, 3] -> pin=0x0a`, i.e. the seq=2 member. WHY: the
    // rank in [`walk_codec`] is a function of (default device, gross location) ONLY. Both pins
    // score rank 0, the loop replaces `best` only on a STRICTLY smaller rank, so the first pin the
    // ascending-NID scan reaches wins — 0x0a, because 0x0a < 0x0b. Association and sequence were
    // decoded and PRINTED by arc 1 and then never consulted. [HDA-SPEC §7.3.3.31] makes sequence
    // the ordering WITHIN an association and sequence 0 its primary member, so the tie-break is not
    // a preference: it is the field the specification provides for exactly this question, and
    // `walk_codec` now breaks rank ties on it.
    //
    // But the deeper defect is that a tie-break still drives ONE pin. A two-member association IS
    // the stereo pair — that is what an association means — and a speaker pair is driven by binding
    // BOTH converters to the SAME stream tag. So arc 2 collects every output-capable pin of the
    // chosen pin's association that carries the same default device and has a converter of its
    // own, orders the members by sequence, and programs all of them.
    // ════════════════════════════════════════════════════════════════════════════════════════════
    let pin0 = ws[path.pin as usize];
    let assoc = ((pin0.pincfg >> 4) & 0x0F) as u8;
    let dev = ((pin0.pincfg >> 20) & 0x0F) as u8;
    let mut paths = [Path::default(); PAIR_MAX];
    let mut pseq = [0u8; PAIR_MAX];
    let mut pchan = [0u8; PAIR_MAX];
    paths[0] = path;
    pseq[0] = (pin0.pincfg & 0x0F) as u8;
    let mut np = 1usize;
    // Association 0 is "no association" and 15 is "not grouped"; neither is a pair, so neither is
    // walked for one. [HDA-SPEC §7.3.3.31]
    if assoc != 0x00 && assoc != 0x0F {
        for nid in 0..MAX_NODES {
            if np >= PAIR_MAX {
                break;
            }
            let w = ws[nid];
            if !w.present || w.kind != WT_PIN || w.nid == path.pin {
                continue;
            }
            if w.pincap & PINCAP_OUTPUT == 0 {
                continue;
            }
            // Port connectivity 1 is "no physical connection" — the same exclusion arc 1's rank
            // makes, restated here because this scan does not go through it.
            if ((w.pincfg >> 30) & 0x03) as u8 == 0x1 {
                continue;
            }
            if ((w.pincfg >> 4) & 0x0F) as u8 != assoc || ((w.pincfg >> 20) & 0x0F) as u8 != dev {
                continue;
            }
            let p = match find_path(ws, w.nid) {
                Some(p) => p,
                None => continue,
            };
            // One converter per member. Two pins fed by ONE converter are not a stereo pair and
            // binding that converter twice would be the same write issued twice.
            let mut dup = false;
            for i in 0..np {
                if paths[i].dac == p.dac {
                    dup = true;
                }
            }
            if dup {
                continue;
            }
            paths[np] = p;
            pseq[np] = (w.pincfg & 0x0F) as u8;
            np += 1;
        }
    }
    // Sequence order. Insertion sort over at most `PAIR_MAX` members. [HDA-SPEC §7.3.3.31]
    for i in 1..np {
        let mut j = i;
        while j > 0 && pseq[j] < pseq[j - 1] {
            paths.swap(j, j - 1);
            pseq.swap(j, j - 1);
            j -= 1;
        }
    }
    // The starting channel each converter takes out of the two-channel stream. A STEREO converter
    // consumes both, so it starts at 0; asking a stereo converter for channels 1..2 of a
    // two-channel stream asks for a channel the stream does not carry. A MONO converter takes one,
    // and then sequence order is channel order. [HDA-SPEC §7.3.3.11, §7.3.4.6]
    for i in 0..np {
        pchan[i] = if ws[paths[i].dac as usize].caps & WCAP_STEREO != 0 { 0 } else { i as u8 };
    }
    for i in 0..np {
        serial_println!(
            "[hda] pair member={}/{} pin=0x{:02x} dac=0x{:02x} dev={} assoc={} seq={} chan={} stereo={} primary={}",
            i + 1, np, paths[i].pin, paths[i].dac, pin_device_name(dev), assoc, pseq[i], pchan[i],
            (ws[paths[i].dac as usize].caps & WCAP_STEREO != 0) as u8, (i == 0) as u8
        );
    }

    let sd = SD_BASE + (iss as u64) * SD_STRIDE;
    let pcm = dma_alloc(PCM_BYTES, 128);
    let bdl = dma_alloc(BDL_BYTES, BDL_ALIGN);
    if pcm == 0 || bdl == 0 {
        serial_println!("[hda] tone REFUSED reason=dma-alloc pcm={:#x} bdl={:#x}", pcm, bdl);
        serial_println!(":: HDA-TONE: reason=dma-alloc -> REFUSED ::");
        return;
    }
    fill(pcm);

    // Two BDL entries, each half the cyclic buffer, each with IOC set so BCIS latches twice per
    // pass. [HDA-SPEC §3.6.2] Entry layout: address (64-bit), length (32-bit), flags (32-bit,
    // bit 0 = IOC).
    let half = (PCM_BYTES / BDL_ENTRIES) as u32;
    for i in 0..BDL_ENTRIES {
        let e = bdl + (i as u64) * 16;
        let addr = bus_addr(pcm) + (i as u64) * (half as u64);
        unsafe {
            core::ptr::write_volatile(e as *mut u32, (addr & 0xFFFF_FFFF) as u32);
            core::ptr::write_volatile((e + 4) as *mut u32, (addr >> 32) as u32);
            core::ptr::write_volatile((e + 8) as *mut u32, half);
            core::ptr::write_volatile((e + 12) as *mut u32, 1); // IOC
        }
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    // ⚠ HDAAMP — THE BDL IS READ BACK AND PRINTED, ENTRY BY ENTRY. Flight 11 scored `bcis=0` on a
    // stream whose LPIB walked the whole cyclic buffer and wrapped, and the three candidate causes
    // of a missing completion latch — a wrong entry length, a wrong entry address, an IOC flag that
    // was never actually stored — are all facts about THESE SIXTEEN BYTES PER ENTRY and about
    // nothing else. Printing the write does not settle it; printing what the memory holds after the
    // fence does. `ioc=` is the bit whose absence would explain `bcis=0` outright. [HDA-SPEC §3.6.2]
    for i in 0..BDL_ENTRIES {
        let e = bdl + (i as u64) * 16;
        let (lo, hi, len, flags) = unsafe {
            (
                core::ptr::read_volatile(e as *const u32),
                core::ptr::read_volatile((e + 4) as *const u32),
                core::ptr::read_volatile((e + 8) as *const u32),
                core::ptr::read_volatile((e + 12) as *const u32),
            )
        };
        serial_println!(
            "[hda] bdl entry={}/{} addr={:#x} len={} ioc={} flags={:#010x} (readback)",
            i, BDL_ENTRIES - 1, ((hi as u64) << 32) | (lo as u64), len, flags & 1, flags
        );
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // HDAAMP DEFECT 2 — THE SPEAKER AMPLIFIER IS NOT AN EAPD PIN ON THIS CODEC.
    //
    // The function group is found FROM THE WIRE here rather than carried down from the walk: node 0
    // is asked for its subordinate node count and each function group for its type, which is three
    // verbs and keeps every arc-2 statement below this file's arc-2 banner. See the `PARAM_GPIO_COUNT`
    // block in `mod tone` for why a GPIO set is the only spec-legal move available.
    // ════════════════════════════════════════════════════════════════════════════════════════════
    let mut afg: Option<u8> = None;
    if let Some(sub) = rings.cmd(cad, 0, VERB_GET_PARAMETER, PARAM_SUBNODE_COUNT, a) {
        a.verbs_get += 1;
        let start = ((sub >> 16) & 0xFF) as u8;
        let count = (sub & 0xFF) as u8;
        for i in 0..count {
            let fg = start.wrapping_add(i);
            if let Some(t) = rings.cmd(cad, fg, VERB_GET_PARAMETER, PARAM_FUNCTION_TYPE, a) {
                a.verbs_get += 1;
                if t & 0x7F == FG_TYPE_AUDIO {
                    afg = Some(fg);
                    break;
                }
            }
        }
    }
    // (function group, mask, data, direction, enable) as read BEFORE anything was driven.
    let mut gpio_saved: Option<(u8, u8, u8, u8, u8)> = None;
    match afg {
        None => serial_println!("[hda] gpio afg=none caps=0 -> none (no audio function group answered)"),
        Some(fg) => {
            let caps = rings.cmd(cad, fg, VERB_GET_PARAMETER, PARAM_GPIO_COUNT, a).unwrap_or(0);
            a.verbs_get += 1;
            // [HDA-SPEC §7.3.4.14] bits 7:0 GPIO count, 15:8 GPO count, 23:16 GPI count,
            // bit 30 GPI-unsolicited capable, bit 31 GPI-wake capable.
            let ngpio = (caps & 0xFF) as u8;
            let ngpo = ((caps >> 8) & 0xFF) as u8;
            let ngpi = ((caps >> 16) & 0xFF) as u8;
            if ngpio == 0 {
                serial_println!(
                    "[hda] gpio afg=0x{:02x} caps={:#010x} gpios=0 gpos={} gpis={} unsol={} wake={} -> none",
                    fg, caps, ngpo, ngpi, (caps >> 30) & 1, (caps >> 31) & 1
                );
            } else {
                let mask: u8 = if ngpio >= 8 { 0xFF } else { (1u8 << ngpio) - 1 };
                let d0 = rings.cmd(cad, fg, VERB_GET_GPIO_DATA, 0, a).unwrap_or(0) as u8;
                let dir0 = rings.cmd(cad, fg, VERB_GET_GPIO_DIRECTION, 0, a).unwrap_or(0) as u8;
                let en0 = rings.cmd(cad, fg, VERB_GET_GPIO_ENABLE, 0, a).unwrap_or(0) as u8;
                a.verbs_get += 3;
                gpio_saved = Some((fg, mask, d0, dir0, en0));
                // Enable, then direction, then data: a pin driven before it is an enabled output is
                // a write to a pin the codec is not yet driving. Restore runs in the mirror order.
                if rings.cmd(cad, fg, VERB_SET_GPIO_ENABLE, (en0 | mask) as u32, a).is_some() {
                    a.verbs_set += 1;
                }
                if rings.cmd(cad, fg, VERB_SET_GPIO_DIRECTION, (dir0 | mask) as u32, a).is_some() {
                    a.verbs_set += 1;
                }
                if rings.cmd(cad, fg, VERB_SET_GPIO_DATA, (d0 | mask) as u32, a).is_some() {
                    a.verbs_set += 1;
                }
                let d1 = rings.cmd(cad, fg, VERB_GET_GPIO_DATA, 0, a).unwrap_or(0) as u8;
                let dir1 = rings.cmd(cad, fg, VERB_GET_GPIO_DIRECTION, 0, a).unwrap_or(0) as u8;
                let en1 = rings.cmd(cad, fg, VERB_GET_GPIO_ENABLE, 0, a).unwrap_or(0) as u8;
                a.verbs_get += 3;
                serial_println!(
                    "[hda] gpio afg=0x{:02x} caps={:#010x} gpios={} gpos={} gpis={} unsol={} wake={} mask={:#04x} enable={:#04x}->{:#04x} dir={:#04x}->{:#04x} data={:#04x}->{:#04x} stuck={} -> set",
                    fg, caps, ngpio, ngpo, ngpi, (caps >> 30) & 1, (caps >> 31) & 1, mask,
                    en0, en1, dir0, dir1, d0, d1,
                    // A pin that will not read back as driven is a pin the codec refused, and that
                    // is a different answer from "driven and the speaker is still silent".
                    (d1 & mask != mask || dir1 & mask != mask || en1 & mask != mask) as u8
                );
            }
        }
    }

    // ── Save every register this run changes. ───────────────────────────────────────────────────
    let saved_ctl = (r8(base, sd + SD_CTL) as u32) | ((r8(base, sd + SD_CTL + 1) as u32) << 8)
        | ((r8(base, sd + SD_CTL + 2) as u32) << 16);
    let saved_fmt_reg = r16(base, sd + SD_FMT);
    let saved_bdpl = r32(base, sd + SD_BDPL);
    let saved_bdpu = r32(base, sd + SD_BDPU);
    let saved_cbl = r32(base, sd + SD_CBL);
    let saved_lvi = r16(base, sd + SD_LVI);

    let mut s = [Saved::default(); PAIR_MAX];
    for m in 0..np {
        let (mpin, mdac) = (paths[m].pin, paths[m].dac);
        if let Some(v) = rings.cmd(cad, mpin, VERB_GET_PIN_CONTROL, 0, a) {
            a.verbs_get += 1;
            s[m].pinctl = (v & 0xFF) as u8;
            s[m].pinctl_ok = true;
        }
        if ws[mpin as usize].pincap & PINCAP_EAPD != 0 {
            if let Some(v) = rings.cmd(cad, mpin, VERB_GET_EAPD, 0, a) {
                a.verbs_get += 1;
                s[m].eapd = (v & 0xFF) as u8;
                s[m].eapd_ok = true;
            }
        }
        if let Some(v) = rings.cmd(cad, mdac, VERB_GET_CONVERTER_FORMAT, 0, a) {
            a.verbs_get += 1;
            s[m].fmt = (v & 0xFFFF) as u16;
            s[m].fmt_ok = true;
        }
        if let Some(v) = rings.cmd(cad, mdac, VERB_GET_STREAM_CHANNEL, 0, a) {
            a.verbs_get += 1;
            s[m].strm = (v & 0xFF) as u8;
            s[m].strm_ok = true;
        }
        for i in 0..paths[m].len as usize {
            let nid = paths[m].nodes[i];
            if ws[nid as usize].caps & WCAP_POWER_CTL != 0 {
                if let Some(v) = rings.cmd(cad, nid, VERB_GET_POWER_STATE, 0, a) {
                    a.verbs_get += 1;
                    s[m].power[i] = (v & 0xFF) as u8;
                    s[m].power_ok[i] = true;
                }
            }
            if ws[nid as usize].out_amp() {
                // GET_AMP_GAIN_MUTE payload: bit15 output, bit13 left. Read the left channel; the
                // two are written back together, which is what this driver set them to.
                if let Some(v) = rings.cmd(cad, nid, VERB_GET_AMP_GAIN_MUTE, 0x80, a) {
                    a.verbs_get += 1;
                    s[m].out_amp[i] = (v & 0xFFFF) as u16;
                    s[m].out_amp_ok[i] = true;
                }
            }
        }
    }

    // ── Program the codec side of every member of the association. ──────────────────────────────
    for m in 0..np {
        let p = paths[m];
        for i in 0..p.len as usize {
            let nid = p.nodes[i];
            let w = ws[nid as usize];
            if w.caps & WCAP_POWER_CTL != 0 {
                if rings.cmd(cad, nid, VERB_SET_POWER_STATE, 0, a).is_some() {
                    a.verbs_set += 1;
                }
            }
            // A selector on the path is pointed at the input the path actually took; a mixer is
            // left alone (it sums) and gets its INPUT amp unmuted instead.
            if w.kind == WT_SELECTOR && w.nconns > 1 {
                if rings.cmd(cad, nid, VERB_SET_CONNECTION_SELECT, p.sel[i] as u32, a).is_some() {
                    a.verbs_set += 1;
                }
            }
            if w.out_amp() {
                let caps = rings.cmd(cad, nid, VERB_GET_PARAMETER, PARAM_OUT_AMP_CAPS, a).unwrap_or(0);
                a.verbs_get += 1;
                let g = moderate_gain(caps);
                if rings.cmd16(cad, nid, VERB_SET_AMP_GAIN_MUTE, amp_payload(true, 0, false, g), a).is_some() {
                    a.verbs_set += 1;
                }
            }
            if w.in_amp() && w.kind == WT_MIXER {
                let caps = rings.cmd(cad, nid, VERB_GET_PARAMETER, PARAM_IN_AMP_CAPS, a).unwrap_or(0);
                a.verbs_get += 1;
                let g = moderate_gain(caps);
                if rings.cmd16(cad, nid, VERB_SET_AMP_GAIN_MUTE, amp_payload(false, p.sel[i], false, g), a).is_some() {
                    a.verbs_set += 1;
                }
            }
        }

        // The pin: output enable, plus the headphone amplifier when the pin declares one, plus EAPD
        // WHEN THE PIN DECLARES IT — on the bench codec neither speaker pin does, which is defect 2.
        // [HDA-SPEC §7.3.3.13, §7.3.3.16]
        let pincap = ws[p.pin as usize].pincap;
        let mut pinctl = PINCTL_OUT_ENABLE;
        if pincap & PINCAP_HP_DRIVE != 0 {
            pinctl |= PINCTL_HP_ENABLE;
        }
        if rings.cmd(cad, p.pin, VERB_SET_PIN_CONTROL, pinctl as u32, a).is_some() {
            a.verbs_set += 1;
        }
        if pincap & PINCAP_EAPD != 0 {
            if rings.cmd(cad, p.pin, VERB_SET_EAPD, (s[m].eapd | EAPD_ENABLE) as u32, a).is_some() {
                a.verbs_set += 1;
            }
        }

        // The converter: format first, then the stream tag and this member's starting channel.
        // Order matters — a converter bound to a stream before its format is set can latch the old
        // format for the first buffer. [HDA-SPEC §7.3.3.8, §7.3.3.11]
        if rings.cmd16(cad, p.dac, VERB_SET_CONVERTER_FORMAT, FMT_48K_16_STEREO as u32, a).is_some() {
            a.verbs_set += 1;
        }
        if rings.cmd(cad, p.dac, VERB_SET_STREAM_CHANNEL, (STREAM_TAG << 4) | pchan[m] as u32, a).is_some() {
            a.verbs_set += 1;
        }
    }

    // ⚠ READ THE BINDING BACK, because LPIB AND BCIS CANNOT SEE IT. Both of those witnesses live on
    // the CONTROLLER side: they say the stream engine fetched sample data and walked the BDL. They
    // say nothing about whether the LINK carries those samples to a converter. MEASURED, on the
    // `intel-hda` fixture: with the stream tag deliberately set to 0 — the "unused" tag, which no
    // link ever carries [HDA-SPEC §3.3.35] — the run still read `lpib=0 -> 44160 (max 191960)
    // bcis=2 fifo_ready=1` and would have scored PASS. So the tag is checked where it is actually
    // observable: EVERY member's converter is asked what it was bound to, and one member that did
    // not take the binding fails the whole run. [HDA-SPEC §7.3.3.11]
    let mut tag_ok = STREAM_TAG != 0;
    let mut tag_bound0 = 0xFFFF_FFFFu32;
    for m in 0..np {
        let tb = rings.cmd(cad, paths[m].dac, VERB_GET_STREAM_CHANNEL, 0, a).unwrap_or(0xFFFF_FFFF);
        a.verbs_get += 1;
        if m == 0 {
            tag_bound0 = tb;
        }
        let this_ok = tb != 0xFFFF_FFFF && ((tb >> 4) & 0x0F) == STREAM_TAG && (tb & 0x0F) == pchan[m] as u32;
        if !this_ok {
            tag_ok = false;
        }
        serial_println!(
            "[hda] bind member={} dac=0x{:02x} want_tag={} want_chan={} bound={:#04x} tag={} chan={} ok={}",
            m, paths[m].dac, STREAM_TAG, pchan[m], tb & 0xFF, (tb >> 4) & 0x0F, tb & 0x0F, this_ok as u8
        );
    }

    // ── HDAAMP — the codec-side readbacks a silent run cannot be diagnosed without. ─────────────
    // Flight 11 proved the CONTROLLER side ran (see the wrap arithmetic at the verdict below) and
    // Peter heard nothing, so every remaining suspect is a codec register: a node still in D3, a
    // muted or zero-gain amplifier, a pin whose output enable did not stick, or a converter whose
    // own format disagrees with `SDnFMT`. All four are readable and none of them were printed.
    for m in 0..np {
        let (mpin, mdac) = (paths[m].pin, paths[m].dac);
        let pwr_dac = rings.cmd(cad, mdac, VERB_GET_POWER_STATE, 0, a).unwrap_or(0xFFFF_FFFF);
        let pwr_pin = rings.cmd(cad, mpin, VERB_GET_POWER_STATE, 0, a).unwrap_or(0xFFFF_FFFF);
        a.verbs_get += 2;
        // [HDA-SPEC §7.3.3.10] response bits 3:0 = the state SET, bits 7:4 = the state ACTUALLY
        // reached. They differ on a node that is still settling or that refused the request, and
        // that difference is the whole reason both are printed.
        serial_println!(
            "[hda] power member={} dac=0x{:02x} set=D{} actual=D{} raw={:#04x} pin=0x{:02x} set=D{} actual=D{} raw={:#04x} entry_dac={:#04x} entry_pin={:#04x}",
            m, mdac, pwr_dac & 0x0F, (pwr_dac >> 4) & 0x0F, pwr_dac & 0xFF,
            mpin, pwr_pin & 0x0F, (pwr_pin >> 4) & 0x0F, pwr_pin & 0xFF,
            s[m].power[(paths[m].len as usize).saturating_sub(1)], s[m].power[0]
        );
        let amp_dac = rings.cmd(cad, mdac, VERB_GET_AMP_GAIN_MUTE, 0x80, a).unwrap_or(0xFFFF_FFFF);
        let amp_pin = rings.cmd(cad, mpin, VERB_GET_AMP_GAIN_MUTE, 0x80, a).unwrap_or(0xFFFF_FFFF);
        let pinctl_back = rings.cmd(cad, mpin, VERB_GET_PIN_CONTROL, 0, a).unwrap_or(0xFFFF_FFFF);
        let fmt_conv = rings.cmd(cad, mdac, VERB_GET_CONVERTER_FORMAT, 0, a).unwrap_or(0xFFFF_FFFF);
        a.verbs_get += 4;
        serial_println!(
            "[hda] amp member={} dac=0x{:02x} raw={:#06x} mute={} gain={} pin=0x{:02x} raw={:#06x} mute={} out_amp={} pinctl={:#04x} out_en={} hp_en={} fmt_conv={:#06x} fmt_want={:#06x} fmt_match={}",
            m, mdac, amp_dac & 0xFFFF, (amp_dac & 0x80 != 0) as u8, amp_dac & 0x7F,
            mpin, amp_pin & 0xFFFF, (amp_pin & 0x80 != 0) as u8, ws[mpin as usize].out_amp() as u8,
            pinctl_back & 0xFF,
            (pinctl_back & PINCTL_OUT_ENABLE as u32 != 0) as u8,
            (pinctl_back & PINCTL_HP_ENABLE as u32 != 0) as u8,
            fmt_conv & 0xFFFF, FMT_48K_16_STEREO,
            ((fmt_conv & 0xFFFF) == FMT_48K_16_STEREO as u32) as u8
        );
    }

    // ── Program the stream descriptor. [HDA-SPEC §3.3.35 ff.] ───────────────────────────────────
    // Stream reset, which the specification requires before a descriptor is reprogrammed.
    w8(base, sd + SD_CTL, (saved_ctl as u8 & !(SDCTL_RUN as u8)) | SDCTL_SRST as u8);
    a.stream += 1;
    let srst_set = wait_us(10_000, || r8(base, sd + SD_CTL) & SDCTL_SRST as u8 != 0);
    w8(base, sd + SD_CTL, r8(base, sd + SD_CTL) & !(SDCTL_SRST as u8));
    a.stream += 1;
    let srst_clr = wait_us(10_000, || r8(base, sd + SD_CTL) & SDCTL_SRST as u8 == 0);

    w32(base, sd + SD_BDPL, (bus_addr(bdl) & 0xFFFF_FFFF) as u32);
    w32(base, sd + SD_BDPU, (bus_addr(bdl) >> 32) as u32);
    w32(base, sd + SD_CBL, PCM_BYTES as u32);
    w16(base, sd + SD_LVI, (BDL_ENTRIES - 1) as u16);
    w16(base, sd + SD_FMT, FMT_48K_16_STEREO);
    // Clear the sticky status bits so BCIS, FIFOE and DESE below are all facts about THIS run.
    w8(base, sd + SD_STS, SDSTS_BCIS | SDSTS_FIFOE | SDSTS_DESE);
    // Stream number into bits 23:20 of SDnCTL — the third byte of the 24-bit register.
    w8(base, sd + SD_CTL + 2, (STREAM_TAG << 4) as u8);
    w8(base, sd + SD_CTL, SDCTL_IOCE as u8);
    a.stream += 7;

    let fmt_back = r16(base, sd + SD_FMT);
    let cbl_back = r32(base, sd + SD_CBL);
    let lvi_back = r16(base, sd + SD_LVI);
    serial_println!(
        "[hda] tone arm sd={} (iss={} => descriptor {}) fmt={:#06x}(readback {:#06x}) cbl={}(readback {}) lvi={}(readback {}) bdl={:#x} pcm={:#x} bytes={} tag={} srst={}/{} ctl={:#010x} ioce={}",
        0, iss, iss, FMT_48K_16_STEREO, fmt_back, PCM_BYTES, cbl_back, BDL_ENTRIES - 1, lvi_back, bdl, pcm, PCM_BYTES,
        STREAM_TAG, srst_set as u8, srst_clr as u8,
        (r8(base, sd + SD_CTL) as u32) | ((r8(base, sd + SD_CTL + 1) as u32) << 8) | ((r8(base, sd + SD_CTL + 2) as u32) << 16),
        (r8(base, sd + SD_CTL) & SDCTL_IOCE as u8 != 0) as u8
    );

    // ── RUN. ────────────────────────────────────────────────────────────────────────────────────
    let lpib0 = r32(base, sd + SD_LPIB); #[cfg(feature = "hda-sie")] let sie_saved = sie_arm(base, iss, a); // HDASIE (B207): INTCTL.SIE[desc] set — and ONLY that bit; GIE/CIE stay as read — immediately before RUN, so the question B130 left (does SIE gate the SDnSTS.BCIS LATCH on the 7-series PCH the way RIRBCTL.RINTCTL gates RINTFL?) is answered by the `bcis=` this very run prints. Same-line, cfg-gated: knob-off bytes unchanged.
    w8(base, sd + SD_CTL, SDCTL_IOCE as u8 | SDCTL_RUN as u8);
    a.stream += 1;
    let ctl_running = (r8(base, sd + SD_CTL) as u32)
        | ((r8(base, sd + SD_CTL + 1) as u32) << 8)
        | ((r8(base, sd + SD_CTL + 2) as u32) << 16);
    let t0 = crate::arch::now_cycles();
    let mut bcis = 0u32;
    let mut lpib_max = lpib0;
    let mut fifo_ready = 0u8;
    // ⚠ HDAAMP — LPIB WRAPS, AND FLIGHT 11 IS THE PROOF THAT NOT COUNTING THE WRAPS MISREADS THE
    // RUN AS A STALL. `SDnLPIB` is a position INSIDE the cyclic buffer, so it returns to 0 every
    // `SDnCBL` bytes [HDA-SPEC §3.3.37]. Reporting only its final value turns a stream that
    // consumed more than one buffer into a number smaller than the one it started from, which is
    // exactly how flight 11's `lpib=0 -> 38396 (max 192000)` reads as "stopped at 38396 of 192000"
    // when it means "walked the whole 192000-byte buffer, wrapped, and was 38396 bytes into the
    // second pass". Counting the wraps here turns the position into BYTES CONSUMED, which is the
    // quantity the run actually claims and the one a rate can be computed from.
    let mut wraps = 0u32;
    let mut prev = lpib0;
    loop {
        let sts = r8(base, sd + SD_STS);
        if sts & SDSTS_FIFORDY != 0 {
            fifo_ready = 1;
        }
        if sts & SDSTS_BCIS != 0 {
            bcis += 1;
            w8(base, sd + SD_STS, SDSTS_BCIS);
            a.stream += 1;
        }
        let l = r32(base, sd + SD_LPIB);
        if l < prev {
            wraps += 1;
        }
        prev = l;
        if l > lpib_max {
            lpib_max = l;
        }
        if elapsed_ms(t0) >= RUN_MS {
            break;
        }
        core::hint::spin_loop();
    }
    let run_ms = elapsed_ms(t0);
    let lpib_end = r32(base, sd + SD_LPIB);
    let sts_end = r8(base, sd + SD_STS);

    // ── STOP, then restore every register this function changed. ────────────────────────────────
    w8(base, sd + SD_CTL, 0);
    a.stream += 1;
    wait_us(10_000, || r8(base, sd + SD_CTL) & SDCTL_RUN as u8 == 0);
    w8(base, sd + SD_CTL, SDCTL_SRST as u8);
    a.stream += 1;
    wait_us(10_000, || r8(base, sd + SD_CTL) & SDCTL_SRST as u8 != 0);
    w8(base, sd + SD_CTL, 0);
    a.stream += 1;
    wait_us(10_000, || r8(base, sd + SD_CTL) & SDCTL_SRST as u8 == 0);

    w32(base, sd + SD_BDPL, saved_bdpl);
    w32(base, sd + SD_BDPU, saved_bdpu);
    w32(base, sd + SD_CBL, saved_cbl);
    w16(base, sd + SD_LVI, saved_lvi);
    w16(base, sd + SD_FMT, saved_fmt_reg);
    w8(base, sd + SD_STS, SDSTS_BCIS | SDSTS_FIFOE | SDSTS_DESE);
    w8(base, sd + SD_CTL + 2, ((saved_ctl >> 16) & 0xFF) as u8);
    w8(base, sd + SD_CTL + 1, ((saved_ctl >> 8) & 0xFF) as u8);
    w8(base, sd + SD_CTL, (saved_ctl & 0xFF) as u8 & !(SDCTL_RUN as u8));
    a.stream += 9; #[cfg(feature = "hda-sie")] sie_restore(base, iss, sie_saved, bcis, a); // HDASIE (B207): INTCTL back to the value read before the arm, with a readback, after the stream is stopped and reset — the mirror-order restore every other register on these lines gets.

    for m in 0..np {
        let p = paths[m];
        if s[m].strm_ok && rings.cmd(cad, p.dac, VERB_SET_STREAM_CHANNEL, s[m].strm as u32, a).is_some() {
            a.verbs_set += 1;
        }
        if s[m].fmt_ok && rings.cmd16(cad, p.dac, VERB_SET_CONVERTER_FORMAT, s[m].fmt as u32, a).is_some() {
            a.verbs_set += 1;
        }
        if s[m].eapd_ok && rings.cmd(cad, p.pin, VERB_SET_EAPD, s[m].eapd as u32, a).is_some() {
            a.verbs_set += 1;
        }
        if s[m].pinctl_ok && rings.cmd(cad, p.pin, VERB_SET_PIN_CONTROL, s[m].pinctl as u32, a).is_some() {
            a.verbs_set += 1;
        }
        for i in 0..p.len as usize {
            let nid = p.nodes[i];
            if s[m].out_amp_ok[i] {
                let mute = s[m].out_amp[i] & 0x80 != 0;
                let gain = (s[m].out_amp[i] & 0x7F) as u8;
                if rings.cmd16(cad, nid, VERB_SET_AMP_GAIN_MUTE, amp_payload(true, 0, mute, gain), a).is_some() {
                    a.verbs_set += 1;
                }
            }
            if s[m].power_ok[i] {
                if rings.cmd(cad, nid, VERB_SET_POWER_STATE, s[m].power[i] as u32, a).is_some() {
                    a.verbs_set += 1;
                }
            }
        }
    }
    // The GPIO set goes back in the mirror order of the drive: data, then direction, then enable.
    if let Some((fg, mask, d0, dir0, en0)) = gpio_saved {
        if rings.cmd(cad, fg, VERB_SET_GPIO_DATA, d0 as u32, a).is_some() {
            a.verbs_set += 1;
        }
        if rings.cmd(cad, fg, VERB_SET_GPIO_DIRECTION, dir0 as u32, a).is_some() {
            a.verbs_set += 1;
        }
        if rings.cmd(cad, fg, VERB_SET_GPIO_ENABLE, en0 as u32, a).is_some() {
            a.verbs_set += 1;
        }
        let d1 = rings.cmd(cad, fg, VERB_GET_GPIO_DATA, 0, a).unwrap_or(0) as u8;
        a.verbs_get += 1;
        serial_println!(
            "[hda] gpio afg=0x{:02x} mask={:#04x} restored data={:#04x} dir={:#04x} enable={:#04x} readback={:#04x} match={}",
            fg, mask, d0, dir0, en0, d1, (d1 == d0) as u8
        );
    }

    // ── Score it. ───────────────────────────────────────────────────────────────────────────────
    // ⚠ HDAAMP — THE VERDICT'S SECOND WITNESS IS NOW "THE BDL WAS WALKED TO ITS END", NOT "BCIS
    // LATCHED", AND THAT IS A CORRECTION MEASURED ON METAL, NOT A WEAKENING. The claim has always
    // been "the stream ran", with two independent witnesses because either alone is weak: LPIB can
    // be read mid-fetch on a stream that stalls immediately after, so the second witness has to say
    // that the engine reached the END of a descriptor. `BCIS` said that on QEMU. On the bench
    // controller (Intel 7-series PCH, 8086:1e20) it did NOT: flight 11 read `bcis=0` with IOC set
    // in both BDL entries and `SDnCTL.IOCE` set — and on the same run LPIB walked the entire
    // 192000-byte cyclic buffer, wrapped, and reached 38396 of the next pass in 1200 ms. That is
    // 230396 bytes, 191997 B/s, against the 192000 B/s a 48 kHz 16-bit stereo stream must consume:
    // the engine was not merely past the end of a descriptor, it was past the end of the BUFFER, at
    // exactly the link rate, for the whole run. A WRAP IS A STRICTLY STRONGER WITNESS THAN A BCIS
    // LATCH — it is a full pass of every descriptor in the list — so the term is `wraps > 0 ||
    // bcis > 0`, and a controller whose completion latch does not work no longer votes a running
    // stream down. `bcis` stays on the wire, unscored on its own, because the next rung is why this
    // silicon does not latch it (hda.md §6, the INTCTL.SIE hypothesis).
    //
    // The RATE is printed and deliberately NOT a verdict term: the QEMU fixture's `audiodev none`
    // backend has no reason to consume at wall-clock rate and gating on a tolerance band there is
    // how a gate becomes a flake. On metal the number is the whole diagnosis.
    let consumed_i = (wraps as i64) * (PCM_BYTES as i64) + (lpib_end as i64) - (lpib0 as i64);
    let consumed = if consumed_i < 0 { 0u64 } else { consumed_i as u64 };
    let expect_bps = (SAMPLE_RATE * 4) as u64;
    let rate_bps = if run_ms > 0 { consumed.saturating_mul(1000) / run_ms } else { 0 };
    let advanced = wraps > 0 || lpib_max > lpib0 || lpib_end != lpib0;
    let walked = wraps > 0 || bcis > 0;
    let ok = advanced && walked && tag_ok && sts_end & SDSTS_FIFOE == 0 && sts_end & SDSTS_DESE == 0;
    serial_println!(
        "[hda] tone stream=0 lpib={} -> {} (max {}) bcis={} fifo_ready={} run_ms={} sts={:#04x} fifoe={} dese={} cbl={} tag={} tag_bound={:#04x} tag_ok={} wraps={} consumed={} rate_bps={} expect_bps={} members={} ctl_running={:#010x}",
        lpib0, lpib_end, lpib_max, bcis, fifo_ready, run_ms, sts_end,
        (sts_end & SDSTS_FIFOE != 0) as u8, (sts_end & SDSTS_DESE != 0) as u8, PCM_BYTES, STREAM_TAG,
        tag_bound0 & 0xFF, tag_ok as u8, wraps, consumed, rate_bps, expect_bps, np, ctl_running
    );
    serial_println!(
        ":: HDA-TONE: lpib_advanced={} walked={} wraps={} bcis={} tag_ok={} fifo_ready={} run_ms={} members={} -> {} ::",
        advanced as u8, walked as u8, wraps, bcis, tag_ok as u8, fifo_ready, run_ms, np,
        if ok { "PASS" } else { "FAIL" }
    );
    a.line("tone");
}

// ===================== BOOTSLOW (rmbp-ledger B201) — THE PROBE, AFTER THE ROOT =====================
//
// `probe()` used to run inside `arch::x86_64::pci::init`, on the boot core, after the internal SD
// card had registered and before the SCHED-X86 handoff started the service loop that binds the root.
// Flight 12 measured what that ordering cost: `[hda] tone arm` at 7116 ms, `:: HDA-TONE: … run_ms=1200
// … -> PASS ::` at 8325 ms, `BPACE: gui t=8344ms` — 1.2 s of audio fixture between a disk being
// present and anything being allowed to root the OS on it. The tone serves nothing the root needs.
//
// So the call moved to the device-service pass (`main.rs`, all three x86 loops) and runs ONCE, after
// the root pass has a verdict (`fs::bootdisk::root_pass_open`). Nothing in `probe` needed the boot
// core: it takes its own PCI inventory, maps its own BAR, allocates its own DMA, and every wait in
// this file is a bounded TSC spin (module docs), which runs the same in a scheduled kernel task as it
// did with interrupts masked. The content of `probe` — every register, every witness — is unchanged.

/// BOOTSLOW — `probe()` once, from a device-service pass, after the root pass's verdict.
pub fn probe_after_root() {
    static DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if DONE.load(core::sync::atomic::Ordering::Relaxed) || !crate::fs::bootdisk::root_pass_open("hda") {
        return;
    }
    if DONE.swap(true, core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    serial_println!(
        "[hda] probe deferred-start at={}ms :: BOOTSLOW: the HDA bring-up runs after the root pass, not on the boot core ::",
        crate::arch::ms()
    );
    probe();
}

// ===================== HDASIE (rmbp-ledger B207) — THE ONE-KNOB FLIGHT B130 LEFT OPEN =====================
//
// Flight 11 read `bcis=0` on a stream that ran at the link rate with IOC set in both BDL entries and
// `SDnCTL.IOCE` set: the 7-series PCH (8086:1e20) did not latch `SDnSTS.BCIS`. QEMU's controller does.
// This driver has one precedent for a latch gated by an interrupt-CONTROL bit: `RIRBCTL.RINTCTL`
// gates the `RIRBSTS.RINTFL` latch, not only the interrupt (hda.md §2.4, measured). The symmetric
// candidate is `INTCTL.SIE[n]` [HDA-SPEC §3.3.14] gating the `SDnSTS.BCIS` latch. `INTCTL` was this
// driver's audited never-written register, so the experiment is its own knob (`UNAOS_HDASIE=1`,
// feature `hda-sie`, implies `hda-tone`), default OFF, and it changes exactly one thing: the SIE bit
// of the ONE descriptor the tone runs on is set right before RUN and restored right after STOP.
// `GIE` (bit 31) and `CIE` (bit 30) are never touched, so no interrupt can reach the CPU either way —
// this is a latch experiment, not an interrupt arc. THE MEASUREMENT is the `bcis=` on the tone line
// of the same run; the verdict below is deliberately NOT about it (a latch that still does not fire
// is the finding, not a defect of this code): `:: HDA-SIE:` PASSes when the register was restored
// to the value read before the arm. `sie_set=` says whether the write stuck (a read-only bit on some
// part would read 0 and is then its own finding). Both functions are file-tail and cfg-gated, both
// call sites are same-line, so the knob-off image is byte-identical (`./arroyo knoboff hda-sie`).
#[cfg(feature = "hda-sie")]
fn sie_arm(base: u64, iss: u8, a: &mut Audit) -> u32 {
    let before = r32(base, REG_INTCTL);
    let bit = 1u32 << (iss as u32 & 31);
    w32(base, REG_INTCTL, before | bit);
    a.intctl += 1;
    let after = r32(base, REG_INTCTL);
    serial_println!(
        "[hda] intctl sie desc={} bit={:#010x} before={:#010x} want={:#010x} after={:#010x} set={} gie={} cie={}",
        iss, bit, before, before | bit, after, (after & bit != 0) as u8, (after >> 31) & 1, (after >> 30) & 1
    );
    before
}

#[cfg(feature = "hda-sie")]
fn sie_restore(base: u64, iss: u8, saved: u32, bcis: u32, a: &mut Audit) {
    let bit = 1u32 << (iss as u32 & 31);
    let armed = r32(base, REG_INTCTL);
    w32(base, REG_INTCTL, saved);
    a.intctl += 1;
    let after = r32(base, REG_INTCTL);
    let restored = after == saved;
    serial_println!(
        "[hda] intctl restore desc={} armed={:#010x} saved={:#010x} after={:#010x} restored={} bcis_with_sie={}",
        iss, armed, saved, after, restored as u8, bcis
    );
    serial_println!(
        ":: HDA-SIE: desc={} sie_set={} restored={} bcis={} -> {} ::",
        iss, (armed & bit != 0) as u8, restored as u8, bcis, if restored { "PASS" } else { "FAIL" }
    );
}

#[cfg(feature = "hda-sie")]
impl Audit {
    fn line(&self, stage: &str) {
        serial_println!(
            "[hda] audit stage={} wrote-cfg={} wrote-ctrl={} wrote-stream={} verbs-get={} verbs-set={} wrote-intctl={}(sie) wrote-wallclk=0(audited) wrote-dplbase=0(audited)",
            stage, self.cfg, self.ctrl, self.stream, self.verbs_get, self.verbs_set, self.intctl
        );
    }
}
