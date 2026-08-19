// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// JB3 (probe half): read-only dump of the NISO1 SMMU pair — the layer the JB2c metal verdict
// indicted. With the USB2 pads powered (JB2c PASS) every port trains to U0, yet the controller
// cannot write ONE command-completion or event into its rings in RAM: a total controller→memory
// DMA-write failure, the predicted "UEFI left the XUSB stream's SMMU path as abort/stale"
// signature. This module answers, from silicon, exactly HOW the stream is being killed.
//
// What the research pass pinned (verify-on-device is this probe's whole job):
//   * tegra234.dtsi: `usb@3610000 { iommus = <&smmu_niso1 TEGRA234_SID_XUSB_HOST>; }` with
//     TEGRA234_SID_XUSB_HOST = 0x0e (dt-bindings/memory/tegra234-mc.h).
//   * `smmu_niso1` is NOT an SMMUv3: compatible = "nvidia,tegra234-smmu", "nvidia,smmu-500" —
//     a DUAL ARM MMU-500 (SMMUv2) at 0x0800_0000 + 0x0700_0000 (the Tegra194 pattern: two
//     mirrored instances interleaved by the fabric; Linux broadcasts writes to both). So the
//     JB3 brief's "GBPA/STE" v3 language maps to v2 reality: sCR0.CLIENTPD (global bypass),
//     sCR0.USFCFG (unmatched stream → fault vs bypass), and SMR[n]/S2CR[n] stream matching.
//   * The MB2 boot log on this very board shows "Task: Program NV master stream id" +
//     "Task: SMMU external bypass disable" + "Task: SMMU init" at t≈0.18 s — the boot chain
//     actively configures this block, then UEFI's own USB boot DMA works THROUGH it, and the
//     ExitBootServices teardown is what strands us (the JB2c pad lesson, one layer deeper).
//
// The differential this probe settles (each outcome names its own boot-2 fix):
//   (a) sCR0 has USFCFG=1 and NO valid SMR matches SID 0x0e  → unmatched-stream ABORT; fix =
//       S2CR bypass via a free SMR match for 0x0e (or clear USFCFG — one bit, but it widens
//       every unmatched stream, so the SMR route is preferred).
//   (b) a valid SMR MATCHES 0x0e with S2CR.TYPE=translate    → stale UEFI translation context
//       (its page tables died at ExitBootServices); fix = flip that S2CR to TYPE=bypass.
//   (c) a valid SMR matches with S2CR.TYPE=fault             → explicit kill; same fix as (b).
//   (d) CLIENTPD=1 (SMMU globally off) or everything reads sane-and-permissive → the drop is
//       NOT at this SMMU; the differential moves down (MC-level bypass kill / IOVA≠PA) — an
//       honest STOP, not a guess.
// The post-attach fault dump is the confirming witness either way: sGFSR latches the fault
// class (USF vs context) and sGFSYNR0/1 record the faulting StreamID — the silicon literally
// names its killer after the ENABLE_SLOT watchdogs.
//
// Safety: both instances live in GiB 0, Device-nGnRE in BOTH mmu_tegra tables (no new mapping);
// the SMMU is always-powered fabric infrastructure like padctl (not a JX1 gated-partition
// class), and this half is READ-ONLY — every register touched is a status/ID/config READ. Per
// the JX1 discipline each instance announces itself on one serial line before its first read,
// so a surprise dead address names itself.

/// MMU-500 global register file 0 (GR0) offsets — SMMUv2, not v3.
const SCR0: u64 = 0x000; // CLIENTPD b0, GFRE b1, GFIE b2, EXIDENABLE b3, USFCFG b10, SMCFCFG b21
const IDR0: u64 = 0x020; // NUMSMRG [7:0], EXIDS b8, SMS b27
const IDR1: u64 = 0x024; // NUMCB [7:0], NUMPAGENDXB [30:28], PAGESIZE b31
const IDR2: u64 = 0x028;
const IDR7: u64 = 0x03c; // MAJOR [7:4], MINOR [3:0]
const SGFAR_LO: u64 = 0x040;
const SGFAR_HI: u64 = 0x044;
const SGFSR: u64 = 0x048; // ICF b0, USF b1, SMCF b2, UCBF b3, UCIF b4, CAF b5, EF b6, PF b7, MULTI b31
const SGFSYNR0: u64 = 0x050;
const SGFSYNR1: u64 = 0x054; // StreamID [15:0] of the faulting transaction
const SMR_BASE: u64 = 0x800; // SMR[n] = 0x800 + 4n: VALID b31, MASK [30:16], ID [14:0]
const S2CR_BASE: u64 = 0xc00; // S2CR[n] = 0xc00 + 4n: TYPE [17:16] (0=translate 1=bypass 2=fault), CBNDX [7:0]

/// Cap the SMR scan: MMU-500 supports at most 128 stream-match groups; IDR0.NUMSMRG is the
/// per-instance truth and is min'd against this.
const MAX_SMRG: u32 = 128;

fn rd(base: u64, off: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}

/// The post-attach witness: after the xHCI attach window (and its ENABLE_SLOT watchdogs), the
/// global fault registers say whether THIS block recorded the kills — and sGFSYNR1 names the
/// faulting StreamID. Read-only (no W1C — boot 2 owns clearing).
pub fn jb3_faults(bases: &[u64]) {
    for (i, &base) in bases.iter().enumerate() {
        jb3_fault_line(i, base, "post-attach");
        // Boot-7: translation faults land in the context bank, not the global GFSR.
        // CB0 FSR @ +0x58 (TF b1, AFF b2, PF b3, EF b4, MULTI b31), FAR @ +0x60/0x64.
        let cb = base + CB0_OFF;
        serial_println!(
            ":: tegra: JB3 — inst{} CB0: SCTLR={:#010x} FSR={:#010x} FAR={:#x}_{:08x} ::",
            i,
            rd(cb, 0x0),
            rd(cb, 0x58),
            rd(cb, 0x64),
            rd(cb, 0x60)
        );
    }
}

/// The Tegra234 MC stream-id override block (always-on MC @0x02c0_0000, GiB-0 Device map);
/// XUSB_HOSTR's override register offset per tegra234-mc-sid.c. Read-only here: it tells us
/// which SID the controller actually EMITS (predicted 0x0e; 0x7f would be the legacy
/// "bypass" SID and would explain an SMR miss).
const MC_SID_BASE: u64 = 0x02c0_0000;

// ---- JB3 boot-7: a REAL identity translation context ------------------------------------
// Boot-6 verdict: no SMMUv3 exists in the firmware's world (census: only the three MMU-500
// pairs) and the v2 pair passes our matched stream fault-free — yet every DMA write still
// vanishes. The one agent left downstream is the fabric/MC, and the MB2 boot log names its
// policy outright: "SMMU external bypass disable" — UNTRANSLATED traffic is refused. Bypass
// output is untranslated by definition; UEFI's own SmmuDxe and Linux both run this fabric
// with real translation contexts. So: stop bypassing, translate identity.
//
// Geometry (from the boot-1 IDR dumps, both instances identical): IDR1=0xe0000080 →
// PAGESIZE=64 KiB (b31), NUMPAGENDXB=6 → NUMPAGE=128 → global half = 8 MiB; GR1 = base
// +0x10000 (page 1); CB n = base + 8 MiB + n·64 KiB; NUMCB=128 — CB0 is free (UEFI's
// contexts died with it; we own the block now).

/// One 4 KiB stage-1 L1 table: 512 × 1 GiB identity blocks covering IA[38:0] (T0SZ=25).
/// Const-built — no runtime init, no interior mutability. Descriptor: PA | AF | SH=inner |
/// AP[2:1]=01 (RW, any privilege) | AttrIndx=0 (MAIR0 idx0 = Normal-WB) | VALID (block).
#[repr(C, align(4096))]
struct IdMap([u64; 512]);
const fn idmap_build() -> IdMap {
    let mut t = [0u64; 512];
    let mut i = 0;
    while i < 512 {
        t[i] = ((i as u64) << 30) | (1 << 10) | (0b11 << 8) | (0b01 << 6) | 0b01;
        i += 1;
    }
    IdMap(t)
}
static JB3_IDMAP: IdMap = idmap_build();

/// MMU-500 layout constants for this silicon (verified against the boot-1 IDR dumps).
const GR1_OFF: u64 = 0x10000; // page 1 (64 KiB pages)
const CB0_OFF: u64 = 8 * 1024 * 1024; // global half = NUMPAGE(128) × 64 KiB
// GR1: CBAR[n] @ +4n (TYPE [17:16]: 0b01 = stage-1 with stage-2 bypass); CBA2R[n] @ +0x800+4n
// (VA64 b0). CB regs: SCTLR 0x0, TCR2 0x10, TTBR0 0x20/0x24, TCR 0x30, MAIR0 0x38.

/// JB3 boot-6: the MC error log — the always-on memory controller records aborted client
/// requests (status names the client + reason, ADR the address). Legacy layout at the
/// broadcast base (INTSTATUS 0x00, ERR_STATUS 0x08, ERR_ADR 0x0c); the same block whose SID
/// region we already read safely. Read-only.
pub fn jb3_mc_errs(tag: &str) {
    let ist = rd(MC_SID_BASE, 0x00);
    let est = rd(MC_SID_BASE, 0x08);
    let adr = rd(MC_SID_BASE, 0x0c);
    serial_println!(
        ":: tegra: JB3 — MC {} INTSTATUS={:#010x} ERR_STATUS={:#010x} ERR_ADR={:#010x} ::",
        tag,
        ist,
        est,
        adr
    );
}

/// JB3 boot-6: read-only dump of an SMMUv3 instance (base from the DTB census — never a
/// guessed address). v3 layout: IDR0 0x0, IDR1 0x4, CR0 0x20, CR0ACK 0x24, GBPA 0x44
/// (ABORT b20, UPDATE b31), STRTAB_BASE 0x80/0x84, STRTAB_BASE_CFG 0x88. If CR0.SMMUEN=0,
/// GBPA governs every incoming transaction — GBPA.ABORT=1 is the classic silent-drop config.
pub fn jb3_v3_dump(base: u64) {
    serial_println!(":: tegra: JB3 — SMMUv3 @{:#010x} first touch (read-only) ::", base);
    let idr0 = rd(base, 0x00);
    let idr1 = rd(base, 0x04);
    let cr0 = rd(base, 0x20);
    let cr0ack = rd(base, 0x24);
    let gbpa = rd(base, 0x44);
    let st_lo = rd(base, 0x80);
    let st_hi = rd(base, 0x84);
    let st_cfg = rd(base, 0x88);
    serial_println!(
        ":: tegra: JB3 — v3 IDR0={:#010x} IDR1={:#010x} CR0={:#010x} (SMMUEN={}) CR0ACK={:#010x} GBPA={:#010x} (ABORT={}) STRTAB={:#x}_{:08x} CFG={:#010x} ::",
        idr0,
        idr1,
        cr0,
        cr0 & 1,
        cr0ack,
        gbpa,
        (gbpa >> 20) & 1,
        st_hi,
        st_lo,
        st_cfg
    );
}

/// JB9-B: the DMA-forensics dump, taken WHILE enable-slot is pending (the JB8 verdict's live
/// window: a running Falcon fetches the command ring / writes the event ring and nothing lands,
/// zero faults). Per instance: the SMR that matches the XUSB SID and its S2CR routing, then the
/// FULL context bank that S2CR points at — SCTLR/TTBR0/TCR/TCR2/MAIR0 + FSR/FAR — with an
/// explicit "is TTBR0 OUR identity table?" verdict (a stale UEFI context translating IOVA≠PA
/// lands DMA at the wrong PA with zero faults — the prime suspect). Then the MC HOSTR/HOSTW
/// override + error log AT THIS INSTANT (not pre/post like JB3's brackets). Read-only.
pub fn jb9_stream_dump(bases: &[u64], xusb_sid: u32, tag: &str) {
    let ours = &JB3_IDMAP as *const _ as u64;
    serial_println!(
        ":: tegra: JB9-B [{}] — SMMU stream {:#x} binding at enable-slot-pending time ::",
        tag,
        xusb_sid
    );
    for (i, &base) in bases.iter().enumerate() {
        let scr0 = rd(base, SCR0);
        let n = (rd(base, IDR0) & 0xff).min(MAX_SMRG);
        let mut matched = false;
        for s in 0..n {
            let smr = rd(base, SMR_BASE + 4 * s as u64);
            if smr & (1 << 31) == 0 {
                continue;
            }
            let (id, mask) = (smr & 0x7fff, (smr >> 16) & 0x7fff);
            if (xusb_sid ^ id) & !mask & 0x7fff != 0 {
                continue;
            }
            matched = true;
            let s2cr = rd(base, S2CR_BASE + 4 * s as u64);
            let (s2type, cbndx) = ((s2cr >> 16) & 0b11, (s2cr & 0xff) as u64);
            serial_println!(
                ":: tegra: JB9-B [{}] — inst{} sCR0={:#010x} SMR[{}]={:#010x} S2CR={:#010x} (type={} cbndx={}) ::",
                tag, i, scr0, s, smr, s2cr, s2type, cbndx
            );
            // The context bank S2CR routes to (only meaningful for type=translate).
            let cb = base + CB0_OFF + cbndx * 0x10000;
            let (ttbr_lo, ttbr_hi) = (rd(cb, 0x20), rd(cb, 0x24));
            let ttbr = ((ttbr_hi as u64) << 32 | ttbr_lo as u64) & 0xffff_ffff_ffff;
            serial_println!(
                ":: tegra: JB9-B [{}] — inst{} CB{}: SCTLR={:#010x} TTBR0={:#x} ({}) TCR={:#010x} TCR2={:#010x} MAIR0={:#010x} CBAR={:#010x} CBA2R={:#010x} ::",
                tag, i, cbndx,
                rd(cb, 0x0),
                ttbr,
                if ttbr == ours { "OURS — JB3 identity map" } else { "NOT OURS — stale/foreign table" },
                rd(cb, 0x30),
                rd(cb, 0x10),
                rd(cb, 0x38),
                rd(base, GR1_OFF + 4 * cbndx),
                rd(base, GR1_OFF + 0x800 + 4 * cbndx)
            );
            serial_println!(
                ":: tegra: JB9-B [{}] — inst{} CB{} faults: FSR={:#010x} FAR={:#x}_{:08x} ::",
                tag, i, cbndx, rd(cb, 0x58), rd(cb, 0x64), rd(cb, 0x60)
            );
        }
        if !matched {
            serial_println!(
                ":: tegra: JB9-B [{}] — inst{} sCR0={:#010x}: NO valid SMR matches SID {:#x} (USFCFG={} governs) ::",
                tag, i, scr0, xusb_sid, (scr0 >> 10) & 1
            );
        }
        jb3_fault_line(i, base, tag);
    }
    // The MC's view at this instant: which SID do HOSTR/HOSTW transactions carry, and has the
    // always-on MC logged an aborted client request while the command engine was fetching?
    serial_println!(
        ":: tegra: JB9-B [{}] — MC HOSTR={:#010x}/sec={:#010x} HOSTW={:#010x}/sec={:#010x} INTSTATUS={:#010x} ERR_STATUS={:#010x} ERR_ADR={:#010x} ::",
        tag,
        rd(MC_SID_BASE, 0x250),
        rd(MC_SID_BASE, 0x254),
        rd(MC_SID_BASE, 0x258),
        rd(MC_SID_BASE, 0x25c),
        rd(MC_SID_BASE, 0x00),
        rd(MC_SID_BASE, 0x08),
        rd(MC_SID_BASE, 0x0c)
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════════
// NET-4i — the SMMU stream for PCIe controller-0 (the last suspect for the RX payload blackhole)
// ══════════════════════════════════════════════════════════════════════════════════════════════
//
// NET-4h armed the DWC INBOUND iATU (the controller-internal PCIe↔fabric translation) and STILL the
// RTL8168's descriptor writebacks + the FIRST RX payload land while every later payload write
// vanishes — the classic "some IOVAs translate, the rest abort silently" signature of a stale/partial
// firmware SMMU context. On Tegra234 an inbound PCIe write TLP, after the DWC iATU, is presented to
// the ARM MMU-500 (SMMUv2) carrying controller-0's stream id (from the DTB `iommu-map`); the SMMU is
// the layer BELOW the iATU and has never been examined for the PCIe stream (the NET-4b note "rings
// functioned dma-coherent" concluded nothing about it). This block: (1) reads the live SMMU state for
// that stream (read-only recon, `[net4i]` witnesses), then (2) applies the honest minimal fix —
// per-stream BYPASS, matching the driver's identity-DMA contract (PCIe addr == DRAM PA). Every write
// is announced before issue. Fail-closed: if the SMMU registers read poison, state is left untouched
// and the current NET-4h behaviour continues.
//
// Bypass-first is deliberate and testable: the XUSB-era verdict that this fabric refuses UNTRANSLATED
// traffic ("SMMU external bypass disable") was established for the NISO1 XUSB stream, not for the PCIe
// client — a different SMMU instance and fabric master. Arming per-stream bypass for PCIe C0 is the
// minimal, in-lane experiment; if the metal sitting shows the RX blackhole survives bypass (the XUSB
// signature repeating), THAT result is what promotes the fallback (a minimal identity translation
// context, the JB3_IDMAP machinery above) — the brief's ordered logic, gated on a metal result we do
// not yet have.

fn wr(base: u64, off: u64, val: u32) {
    unsafe {
        core::ptr::write_volatile((base + off) as *mut u32, val);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// A read that means "this SMMU register file is not answering": all-ones (unclaimed / powered-off
/// fabric decode) is the poison we fail-closed on. sCR0 is never legitimately 0xffffffff (reserved
/// bits read 0), so this is a safe liveness gate before any write.
#[inline]
fn smmu_poison(v: u32) -> bool {
    v == 0xffff_ffff
}

/// NET-4i recon: read-only dump of the PCIe-C0 stream's binding on every SMMU instance the DTB
/// named. Per instance: sCR0 (with CLIENTPD/USFCFG decoded), the SMR that matches the stream and its
/// S2CR routing (bypass/translate/fault), the context bank + "is TTBR0 our identity map?" verdict for
/// a translate route, and the global fault latch. Emits `[net4i]` witness lines sufficient to verdict
/// what the SMMU is doing to controller-0's inbound DMA. Touches no register with a write.
pub fn net4i_recon(bases: &[u64], sid: u32, tag: &str) {
    let ours = &JB3_IDMAP as *const _ as u64;
    serial_println!(
        ":: tegra: [net4i] {} — PCIe-C0 SMMU stream {:#x} binding across {} instance(s) ::",
        tag, sid, bases.len()
    );
    for (i, &base) in bases.iter().enumerate() {
        let scr0 = rd(base, SCR0);
        if smmu_poison(scr0) {
            serial_println!(
                ":: tegra: [net4i] {} — inst{} @ {:#010x}: sCR0=0xffffffff (POISON / not answering) — instance skipped ::",
                tag, i, base
            );
            continue;
        }
        let n = (rd(base, IDR0) & 0xff).min(MAX_SMRG);
        serial_println!(
            ":: tegra: [net4i] {} — inst{} @ {:#010x}: sCR0={:#010x} (CLIENTPD={} USFCFG={}) NUMSMRG={} ::",
            tag, i, base, scr0, scr0 & 1, (scr0 >> 10) & 1, n
        );
        let mut matched = false;
        for s in 0..n {
            let smr = rd(base, SMR_BASE + 4 * s as u64);
            if smr & (1 << 31) == 0 {
                continue;
            }
            let (id, mask) = (smr & 0x7fff, (smr >> 16) & 0x7fff);
            if (sid ^ id) & !mask & 0x7fff != 0 {
                continue;
            }
            matched = true;
            let s2cr = rd(base, S2CR_BASE + 4 * s as u64);
            let (s2type, cbndx) = ((s2cr >> 16) & 0b11, (s2cr & 0xff) as u64);
            let tname = match s2type {
                0 => "translate",
                1 => "bypass",
                _ => "fault",
            };
            serial_println!(
                ":: tegra: [net4i] {} — inst{} SMR[{}]={:#010x} matches sid {:#x} -> S2CR={:#010x} (type={} cbndx={}) ::",
                tag, i, s, smr, sid, s2cr, tname, cbndx
            );
            if s2type == 0 {
                let cb = base + CB0_OFF + cbndx * 0x10000;
                let (ttbr_lo, ttbr_hi) = (rd(cb, 0x20), rd(cb, 0x24));
                let ttbr = ((ttbr_hi as u64) << 32 | ttbr_lo as u64) & 0xffff_ffff_ffff;
                serial_println!(
                    ":: tegra: [net4i] {} — inst{} CB{}: SCTLR={:#010x} TTBR0={:#x} ({}) TCR={:#010x} FSR={:#010x} FAR={:#x}_{:08x} ::",
                    tag, i, cbndx,
                    rd(cb, 0x0),
                    ttbr,
                    if ttbr == ours { "OURS-identity" } else { "foreign/stale table (IOVA≠PA risk)" },
                    rd(cb, 0x30),
                    rd(cb, 0x58),
                    rd(cb, 0x64),
                    rd(cb, 0x60)
                );
            }
        }
        if !matched {
            serial_println!(
                ":: tegra: [net4i] {} — inst{}: NO valid SMR matches sid {:#x} (unmatched-stream {} governs) ::",
                tag, i, sid,
                if (scr0 >> 10) & 1 == 1 { "ABORT (USFCFG=1)" } else { "BYPASS (USFCFG=0)" }
            );
        }
        jb3_fault_line(i, base, tag);
    }
}

/// NET-4i fix: force PCIe-C0's stream to BYPASS on every named SMMU instance — the honest minimal
/// map matching the driver's identity-DMA design (an inbound PCIe write reaches the DRAM PA it
/// targets, untranslated). Per instance, in order of preference:
///   * sCR0 poison            → fail-closed, leave untouched (recon already logged it).
///   * CLIENTPD=1 (SMMU off)  → every stream already bypasses; nothing to arm.
///   * an SMR already matches  → flip that S2CR to type=bypass (unless it already is).
///   * no match               → claim a free SMR (VALID clear), set its S2CR=bypass, then VALIDate it.
///   * no free SMR            → fail-closed (cannot arm without clobbering another master's stream).
/// Every register write is announced before issue. Returns the number of instances armed.
pub fn net4i_bypass(bases: &[u64], sid: u32) -> usize {
    const S2CR_BYPASS: u32 = 0b01 << 16; // TYPE[17:16] = 0b01
    let mut armed = 0usize;
    serial_println!(
        ":: tegra: [net4i] FIX — arming per-stream BYPASS for PCIe-C0 sid {:#x} across {} instance(s) ::",
        sid, bases.len()
    );
    for (i, &base) in bases.iter().enumerate() {
        let scr0 = rd(base, SCR0);
        if smmu_poison(scr0) {
            serial_println!(
                ":: tegra: [net4i] FIX — inst{}: sCR0 POISON — FAIL-CLOSED, SMMU left untouched ::",
                i
            );
            continue;
        }
        if scr0 & 1 == 1 {
            serial_println!(
                ":: tegra: [net4i] FIX — inst{}: CLIENTPD=1 (SMMU globally bypassing) — stream already untranslated; nothing to arm ::",
                i
            );
            continue;
        }
        let n = (rd(base, IDR0) & 0xff).min(MAX_SMRG);
        // Pass 1: an SMR already matching the stream — reroute it to bypass in place.
        let mut done = false;
        for s in 0..n {
            let smr = rd(base, SMR_BASE + 4 * s as u64);
            if smr & (1 << 31) == 0 {
                continue;
            }
            let (id, mask) = (smr & 0x7fff, (smr >> 16) & 0x7fff);
            if (sid ^ id) & !mask & 0x7fff != 0 {
                continue;
            }
            let s2cr = rd(base, S2CR_BASE + 4 * s as u64);
            if (s2cr >> 16) & 0b11 == 0b01 {
                serial_println!(
                    ":: tegra: [net4i] FIX — inst{} SMR[{}] already S2CR=bypass — nothing to change ::",
                    i, s
                );
            } else {
                let new = (s2cr & !(0b11 << 16)) | S2CR_BYPASS;
                serial_println!(
                    ":: tegra: [net4i] FIX — inst{} >>> WRITE S2CR[{}] {:#010x} -> {:#010x} (type -> bypass) ::",
                    i, s, s2cr, new
                );
                wr(base, S2CR_BASE + 4 * s as u64, new);
                armed += 1;
            }
            done = true;
            break;
        }
        if done {
            continue;
        }
        // Pass 2: unmatched — claim a free SMR (VALID clear), bypass-route it, then validate.
        let mut placed = false;
        for s in 0..n {
            let smr = rd(base, SMR_BASE + 4 * s as u64);
            if smr & (1 << 31) != 0 {
                continue;
            }
            // Program S2CR to bypass BEFORE validating the SMR, so no transaction can transiently
            // hit a matched stream routed to a stale/zero context.
            let s2cr = rd(base, S2CR_BASE + 4 * s as u64);
            let new_s2cr = (s2cr & !(0b11 << 16)) | S2CR_BYPASS;
            serial_println!(
                ":: tegra: [net4i] FIX — inst{} >>> WRITE S2CR[{}] {:#010x} -> {:#010x} (free slot -> bypass) ::",
                i, s, s2cr, new_s2cr
            );
            wr(base, S2CR_BASE + 4 * s as u64, new_s2cr);
            let new_smr = (1u32 << 31) | (sid & 0x7fff); // VALID, mask=0 (exact match)
            serial_println!(
                ":: tegra: [net4i] FIX — inst{} >>> WRITE SMR[{}] {:#010x} -> {:#010x} (VALID, exact sid {:#x}) ::",
                i, s, smr, new_smr, sid
            );
            wr(base, SMR_BASE + 4 * s as u64, new_smr);
            armed += 1;
            placed = true;
            break;
        }
        if !placed {
            serial_println!(
                ":: tegra: [net4i] FIX — inst{}: no free SMR slot (all {} valid) — FAIL-CLOSED, cannot arm without clobbering another stream ::",
                i, n
            );
        }
    }
    serial_println!(":: tegra: [net4i] FIX — done: {} instance(s) armed to bypass ::", armed);
    armed
}

// ══════════════════════════════════════════════════════════════════════════════════════════════
// NET-4w — the inbound sink is BELOW the iATU: a REAL identity translation context for the PCIe
// stream (boot-30 + NET-4v audit verdict)
// ══════════════════════════════════════════════════════════════════════════════════════════════
//
// Evidence chain that lands here:
//   * NET-4v (boot-30): armed iATU region 1 covers ALL DMA objects (limit 0x6840ffff vs top need
//     0x68403fff) yet rx[1]'s sink at bus 0x683e0800 — INSIDE the window, 2 KiB past rx[0] which
//     translates — still vanishes. The iATU is exonerated; the sink is a stage below it.
//   * Surviving inbound set = ring page + first RX buffer only: the firmware-residual-mapping
//     signature. UEFI programmed inbound SMMU translations for its own DMA; we inherited them;
//     unmapped bus pages write-to-nowhere silently.
//   * NET-4i (boot-30) armed per-stream BYPASS for this very stream and the blackhole SURVIVED —
//     the same verdict the XUSB saga hit one fabric over: the MB2 boot log's "Task: SMMU external
//     bypass disable" means UNTRANSLATED (bypass-output) traffic is refused at the fabric/MC.
//     Bypass is a dead end on this silicon; the brief's option (b) is refuted by metal, which
//     promotes option (a): translate identity, honestly, through a context WE own.
//
// FACTS (L4T kernel + tegra234 DTB, GPLv2 = facts only; verify-on-device via the [net4w] lines):
//   * The devkit NIC enumerates in PCI domain 4: Tegra234 PCIe C4, `pcie@141a0000`. Its DT node
//     carries `iommu-map = <0x0 &smmu_niso1 TEGRA234_SID_PCIE_C4 0x1000>` with
//     `iommu-map-mask = <0x0>` — every RID behind the RC is squashed to the ONE stream id
//     (dt-bindings/memory/tegra234-mc.h). We never hard-code either: `fdt_tegra::pcie_iommu`
//     resolves phandle → SMMU bases + sid from the live DTB and the witness lines print them.
//   * That SMMU is NOT an SMMUv3 (no GBPA/STE): compatible "nvidia,tegra234-smmu",
//     "nvidia,smmu-500" — a dual ARM MMU-500 (SMMUv2), two mirrored instances interleaved by the
//     fabric (writes must go to BOTH; Linux broadcasts). The v3 "GBPA/STE" language of the brief
//     maps to v2 reality: sCR0.CLIENTPD (global bypass), sCR0.USFCFG (unmatched-stream policy),
//     SMR[n]/S2CR[n] stream matching, CBAR/CBA2R/CB[n] translation contexts.
//   * EL3 firewall: refuted by silicon — JB3/JB9 read this register file and NET-4i WROTE
//     SMR/S2CR on metal (boot-30) with no abort. Not a JX1 gated-partition class.
//
// The fix (option a — bounded, honest): build one stage-1 identity context (the JB3_IDMAP 1 GiB
// block table already proven const-correct above), program a context bank no live stream routes
// to, and re-route the PCIe stream's S2CR from bypass to translate-through-that-bank. Identity
// translation output is TRANSLATED traffic — it clears the fabric's bypass-disable policy while
// preserving the driver's identity-DMA contract (bus addr == DRAM PA). Options (b)/(c): (b) is
// the refuted NET-4i state (kept as the fail-closed fallback so a net4w abort never regresses
// boot-30 behaviour); (c) global bypass is NOT implemented — per-stream translate is possible,
// and (c) would lower inbound isolation for every master on the instance.
//
// Discipline: every register write is announced before issue and witnessed with a readback;
// any readback mismatch or geometry surprise fails CLOSED (S2CR untouched → the stream keeps
// its NET-4i bypass route). The page table is cache-cleaned to PoC before the walker can see it.

/// SMMUv2 CB_SCTLR fields (arm-smmu facts): M b0, TRE b1, AFE b2, CFRE b5, CFIE b6, ASIDPNE b12.
const CB_SCTLR_S1: u32 = (1 << 12) | (1 << 6) | (1 << 5) | (1 << 2) | (1 << 1) | 1; // 0x1067
/// CB_TCR for VA64 stage-1: T0SZ=25 (IA[38:0] — the JB3_IDMAP span), IRGN0/ORGN0=WBWA,
/// SH0=inner, TG0=4 KiB granule.
const CB_TCR_S1: u32 = 25 | (0b01 << 8) | (0b01 << 10) | (0b11 << 12);
/// CB_TCR2: PASIZE=0b010 (40-bit output — covers Orin DRAM), SEP/TBI zero.
const CB_TCR2_S1: u32 = 0b010;
/// MAIR0 attr0 = 0xff (Normal WB RWA) — AttrIndx 0, the index every JB3_IDMAP block names.
const CB_MAIR0_S1: u32 = 0x0000_00ff;
/// GR0 TLB maintenance: TLBIALLNSNH 0x68 (invalidate all non-secure non-hyp), sTLBGSYNC 0x70,
/// sTLBGSTATUS 0x74 (GSACTIVE b0).
const TLBIALLNSNH: u64 = 0x68;
const STLBGSYNC: u64 = 0x70;
const STLBGSTATUS: u64 = 0x74;

/// One announced+witnessed 32-bit register write. Returns false (and says so) on readback
/// mismatch — the caller fails closed.
fn wrw(what: &str, i: usize, base: u64, off: u64, val: u32) -> bool {
    serial_println!(
        ":: tegra: [net4w] FIX — inst{} >>> WRITE {} @+{:#x} = {:#010x} ::",
        i, what, off, val
    );
    wr(base, off, val);
    let rb = rd(base, off);
    if rb == val {
        true
    } else {
        serial_println!(
            ":: tegra: [net4w] FIX — inst{} !! {} readback {:#010x} != {:#010x} — FAIL-CLOSED ::",
            i, what, rb, val
        );
        false
    }
}

/// Per-instance MMU-500 geometry derived from IDR1 at runtime (the boot-1 dumps verified the
/// niso1 pair; the PCIe stream's instance is DTB-resolved and may differ, so never assume the
/// fixed XUSB constants): page size, global-half size, and CB count.
fn mmu500_geometry(base: u64) -> Option<(u64, u64, u32)> {
    let idr1 = rd(base, IDR1);
    if smmu_poison(idr1) {
        return None;
    }
    let pagesize: u64 = if idr1 & (1 << 31) != 0 { 0x10000 } else { 0x1000 };
    let numpage: u64 = 1u64 << (((idr1 >> 28) & 0b111) + 1);
    let numcb = idr1 & 0xff;
    if numcb == 0 {
        return None;
    }
    Some((pagesize, numpage * pagesize, numcb))
}

/// NET-4w step 2 verdict — the brief's one-liner, per instance: how is the PCIe stream routed
/// RIGHT NOW, and how many live (valid) stream mappings does the instance carry? Read-only.
pub fn net4w_verdict(bases: &[u64], sid: u32) {
    for (i, &base) in bases.iter().enumerate() {
        let scr0 = rd(base, SCR0);
        if smmu_poison(scr0) {
            serial_println!(":: tegra: [net4w] verdict inst{}: POISON (not answering) ::", i);
            continue;
        }
        if scr0 & 1 == 1 {
            serial_println!(":: tegra: [net4w] verdict inst{}: bypass (CLIENTPD=1, SMMU globally off) ::", i);
            continue;
        }
        let n = (rd(base, IDR0) & 0xff).min(MAX_SMRG);
        let mut live = 0u32;
        let mut route = "unmatched";
        for s in 0..n {
            let smr = rd(base, SMR_BASE + 4 * s as u64);
            if smr & (1 << 31) == 0 {
                continue;
            }
            live += 1;
            let (id, mask) = (smr & 0x7fff, (smr >> 16) & 0x7fff);
            if (sid ^ id) & !mask & 0x7fff == 0 {
                route = match (rd(base, S2CR_BASE + 4 * s as u64) >> 16) & 0b11 {
                    0 => "translate",
                    1 => "bypass",
                    _ => "fault",
                };
            }
        }
        serial_println!(
            ":: tegra: [net4w] verdict inst{}: translate-mode with {} live mappings; stream {:#x} routes {} (USFCFG={}) ::",
            i, live, sid, route, (scr0 >> 10) & 1
        );
    }
}

/// NET-4w fix (option a): route the PCIe stream through a stage-1 IDENTITY translation context on
/// every named instance. Per instance: derive geometry from IDR1; find the SMR matching the
/// stream (or claim a free one); pick the highest context bank no valid stream currently routes
/// to; program CBA2R(VA64) → CBAR(stage-1) → TCR2/TTBR0/TCR/MAIR0 → clear our CB's FSR → SCTLR.M;
/// TLB-invalidate + sync; only then flip S2CR to translate/cbndx. Every write witnessed by
/// readback; ANY mismatch or geometry surprise fails closed with the stream's prior route (the
/// NET-4i bypass) untouched. Returns instances armed.
pub fn net4w_translate(bases: &[u64], sid: u32) -> usize {
    // The walker reads the table through the coherent fabric; make the static table visible to
    // PoC before any instance can walk it.
    let tbl = &JB3_IDMAP as *const _ as u64;
    crate::arch::aarch64::cache::clean_range(tbl as usize, 4096);
    serial_println!(
        ":: tegra: [net4w] FIX — arming stage-1 IDENTITY translate for PCIe stream {:#x} across {} instance(s) (table @{:#x}) ::",
        sid, bases.len(), tbl
    );
    let mut armed = 0usize;
    'inst: for (i, &base) in bases.iter().enumerate() {
        let scr0 = rd(base, SCR0);
        if smmu_poison(scr0) {
            serial_println!(":: tegra: [net4w] FIX — inst{}: sCR0 POISON — FAIL-CLOSED ::", i);
            continue;
        }
        if scr0 & 1 == 1 {
            serial_println!(
                ":: tegra: [net4w] FIX — inst{}: CLIENTPD=1 (SMMU globally off) — no context to arm; leaving as-is ::",
                i
            );
            continue;
        }
        let Some((pagesize, global_half, numcb)) = mmu500_geometry(base) else {
            serial_println!(":: tegra: [net4w] FIX — inst{}: IDR1 unreadable/degenerate — FAIL-CLOSED ::", i);
            continue;
        };
        let n = (rd(base, IDR0) & 0xff).min(MAX_SMRG);
        // Which CBs do live streams route to (type=translate only)? Ours must collide with none.
        let mut cb_used = [false; 256];
        let mut our_smr: Option<u32> = None;
        let mut free_smr: Option<u32> = None;
        for s in 0..n {
            let smr = rd(base, SMR_BASE + 4 * s as u64);
            if smr & (1 << 31) == 0 {
                if free_smr.is_none() {
                    free_smr = Some(s);
                }
                continue;
            }
            let s2cr = rd(base, S2CR_BASE + 4 * s as u64);
            if (s2cr >> 16) & 0b11 == 0 {
                cb_used[(s2cr & 0xff) as usize] = true;
            }
            let (id, mask) = (smr & 0x7fff, (smr >> 16) & 0x7fff);
            if (sid ^ id) & !mask & 0x7fff == 0 && our_smr.is_none() {
                our_smr = Some(s);
            }
        }
        let Some(cbndx) = (0..numcb.min(256)).rev().find(|&c| !cb_used[c as usize]) else {
            serial_println!(
                ":: tegra: [net4w] FIX — inst{}: all {} context banks routed-to — FAIL-CLOSED ::",
                i, numcb
            );
            continue;
        };
        let gr1 = base + pagesize;
        let cb = base + global_half + cbndx as u64 * pagesize;
        serial_println!(
            ":: tegra: [net4w] FIX — inst{}: geometry page={:#x} global={:#x} numcb={} -> CB{} @+{:#x}, SMR slot {} ::",
            i, pagesize, global_half, numcb, cbndx,
            global_half + cbndx as u64 * pagesize,
            our_smr.map(|s| s as i64).unwrap_or(free_smr.map(|s| s as i64).unwrap_or(-1))
        );
        // Program the context BEFORE anything can route to it.
        let ok = wrw("CBA2R", i, gr1, 0x800 + 4 * cbndx as u64, 1) // VA64
            && wrw("CBAR", i, gr1, 4 * cbndx as u64, 0b01 << 16) // stage-1, stage-2 bypass
            && wrw("CB.TCR2", i, cb, 0x10, CB_TCR2_S1)
            && wrw("CB.TTBR0_LO", i, cb, 0x20, tbl as u32)
            && wrw("CB.TTBR0_HI", i, cb, 0x24, (tbl >> 32) as u32)
            && wrw("CB.TCR", i, cb, 0x30, CB_TCR_S1)
            && wrw("CB.MAIR0", i, cb, 0x38, CB_MAIR0_S1);
        if !ok {
            continue;
        }
        // Our bank's fault latch starts clean (W1C; we own this CB now) — then enable the MMU.
        wr(cb, 0x58, 0xffff_ffff);
        if !wrw("CB.SCTLR", i, cb, 0x0, CB_SCTLR_S1) {
            continue;
        }
        // Stale-TLB hygiene before routing: invalidate all NS-non-hyp entries and sync (bounded).
        wr(base, TLBIALLNSNH, 0);
        wr(base, STLBGSYNC, 0);
        let mut spins = 0u32;
        while rd(base, STLBGSTATUS) & 1 != 0 {
            spins += 1;
            if spins > 1_000_000 {
                serial_println!(
                    ":: tegra: [net4w] FIX — inst{}: sTLBGSYNC never drained — FAIL-CLOSED (S2CR untouched) ::",
                    i
                );
                continue 'inst;
            }
        }
        // Route the stream: flip (or place) its S2CR to translate-through-our-bank, LAST.
        let s2cr_val = cbndx; // TYPE[17:16]=0b00 translate, CBNDX[7:0]
        match (our_smr, free_smr) {
            (Some(s), _) => {
                if !wrw("S2CR(translate)", i, base, S2CR_BASE + 4 * s as u64, s2cr_val) {
                    continue;
                }
            }
            (None, Some(s)) => {
                // S2CR first, SMR VALID second — no transient match into a stale route.
                if !wrw("S2CR(translate)", i, base, S2CR_BASE + 4 * s as u64, s2cr_val)
                    || !wrw("SMR(valid)", i, base, SMR_BASE + 4 * s as u64, (1u32 << 31) | (sid & 0x7fff))
                {
                    continue;
                }
            }
            (None, None) => {
                serial_println!(
                    ":: tegra: [net4w] FIX — inst{}: no matching and no free SMR — FAIL-CLOSED ::",
                    i
                );
                continue;
            }
        }
        serial_println!(
            ":: tegra: [net4w] FIX — inst{} ARMED: stream {:#x} -> CB{} stage-1 identity (TTBR0={:#x}) ::",
            i, sid, cbndx, tbl
        );
        armed += 1;
    }
    serial_println!(
        ":: tegra: [net4w] FIX — done: {} instance(s) translating identity ::",
        armed
    );
    armed
}

fn jb3_fault_line(i: usize, base: u64, tag: &str) {
    let gfsr = rd(base, SGFSR);
    let far_lo = rd(base, SGFAR_LO);
    let far_hi = rd(base, SGFAR_HI);
    let syn0 = rd(base, SGFSYNR0);
    let syn1 = rd(base, SGFSYNR1);
    serial_println!(
        ":: tegra: JB3 — inst{} {} faults: sGFSR={:#010x} (ICF={} USF={} SMCF={} EF={} MULTI={}) sGFAR={:#x}_{:08x} SYNR0={:#010x} SYNR1={:#010x} (sid={:#x}) ::",
        i,
        tag,
        gfsr,
        gfsr & 1,
        (gfsr >> 1) & 1,
        (gfsr >> 2) & 1,
        (gfsr >> 6) & 1,
        (gfsr >> 31) & 1,
        far_hi,
        far_lo,
        syn0,
        syn1,
        syn1 & 0x7fff
    );
}
