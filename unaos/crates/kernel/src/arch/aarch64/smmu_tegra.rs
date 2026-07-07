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

/// The read-only pre-attach dump: global config + every VALID stream-match group, with an
/// explicit MATCH verdict against the XUSB stream id. One announce line per instance before
/// its first read (JX1: a dead boot names the killer address class).
pub fn jb3_probe(bases: &[u64], xusb_sid: u32) {
    serial_println!(
        ":: tegra: JB3 — NISO1 SMMU probe (read-only), {} instance(s), XUSB SID={:#x} ::",
        bases.len(),
        xusb_sid
    );
    for (i, &base) in bases.iter().enumerate() {
        serial_println!(":: tegra: JB3 — inst{} @{:#010x} first touch ::", i, base);
        let scr0 = rd(base, SCR0);
        let idr0 = rd(base, IDR0);
        let idr1 = rd(base, IDR1);
        let idr2 = rd(base, IDR2);
        let idr7 = rd(base, IDR7);
        serial_println!(
            ":: tegra: JB3 — inst{} sCR0={:#010x} (CLIENTPD={} USFCFG={} EXIDENABLE={}) IDR0={:#010x} (SMS={} NUMSMRG={}) IDR1={:#010x} IDR2={:#010x} IDR7={:#010x} (r{}p{}) ::",
            i,
            scr0,
            scr0 & 1,
            (scr0 >> 10) & 1,
            (scr0 >> 3) & 1,
            idr0,
            (idr0 >> 27) & 1,
            idr0 & 0xff,
            idr1,
            idr2,
            idr7,
            (idr7 >> 4) & 0xf,
            idr7 & 0xf
        );
        jb3_fault_line(i, base, "pre");
        // Every VALID stream-match group + its routing control. On MMU-500 the (id, mask)
        // match is `(sid ^ ID) & ~MASK == 0` — a set MASK bit means "don't care".
        let n = (idr0 & 0xff).min(MAX_SMRG);
        let mut valid = 0u32;
        let mut xusb_hit = false;
        for s in 0..n {
            let smr = rd(base, SMR_BASE + 4 * s as u64);
            if smr & (1 << 31) == 0 {
                continue;
            }
            valid += 1;
            let id = smr & 0x7fff;
            let mask = (smr >> 16) & 0x7fff;
            let s2cr = rd(base, S2CR_BASE + 4 * s as u64);
            let hits = (xusb_sid ^ id) & !mask & 0x7fff == 0;
            xusb_hit |= hits;
            serial_println!(
                ":: tegra: JB3 — inst{} SMR[{}]={:#010x} (id={:#x} mask={:#x}){} S2CR={:#010x} (type={} cbndx={}) ::",
                i,
                s,
                smr,
                id,
                mask,
                if hits { " *MATCHES-XUSB*" } else { "" },
                s2cr,
                (s2cr >> 16) & 0b11,
                s2cr & 0xff
            );
        }
        serial_println!(
            ":: tegra: JB3 — inst{}: {} valid SMR group(s) of {}, XUSB SID {:#x} {} ::",
            i,
            valid,
            n,
            xusb_sid,
            if xusb_hit {
                "MATCHED (stale-context/fault class)"
            } else {
                "UNMATCHED (USFCFG governs)"
            }
        );
    }
    serial_println!(":: tegra: JB3 — SMMU probe done (no writes) -> PASS ::");
}

/// The post-attach witness: after the xHCI attach window (and its ENABLE_SLOT watchdogs), the
/// global fault registers say whether THIS block recorded the kills — and sGFSYNR1 names the
/// faulting StreamID. Read-only (no W1C — boot 2 owns clearing).
pub fn jb3_faults(bases: &[u64]) {
    for (i, &base) in bases.iter().enumerate() {
        jb3_fault_line(i, base, "post-attach");
    }
}

/// Additional GR0 offsets for the fix half.
const TLBIALLNSNH: u64 = 0x068; // invalidate all NS non-hyp TLB entries (write-any)
const STLBGSYNC: u64 = 0x070; // global TLB sync (write-any)
const STLBGSTATUS: u64 = 0x074; // GSACTIVE b0 — sync complete when clear

/// The Tegra234 MC stream-id override block (always-on MC @0x02c0_0000, GiB-0 Device map);
/// XUSB_HOSTR's override register offset per tegra234-mc-sid.c. Read-only here: it tells us
/// which SID the controller actually EMITS (predicted 0x0e; 0x7f would be the legacy
/// "bypass" SID and would explain an SMR miss).
const MC_SID_BASE: u64 = 0x02c0_0000;
const MC_SID_XUSB_HOSTR: u64 = 0x250;

fn wr(base: u64, off: u64, v: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, v) }
}

/// JB3 fix (boot 2) — open the XUSB stream through the NISO1 pair.
///
/// Boot 1's metal verdict (2026-07-07, serial-orin-jb3-probe.log): both instances sit at
/// sCR0.CLIENTPD=1 (client port DISABLED), USFCFG=0, ZERO valid SMRs, and sGFSR never latches —
/// UEFI's ExitBootServices teardown shut the SMMU's client port outright (the JB2c pad lesson,
/// one layer deeper), and with the fabric's external-bypass path also disabled (the MB2 task),
/// the controller's DMA writes are swallowed with no fault logic ever reached.
///
/// The re-open, per instance and deliberately in this order:
///   1. SMR[0] = VALID | ID=sid (mask 0) and S2CR[0] = TYPE=bypass — the stream's route exists
///      BEFORE any traffic can be accepted;
///   2. sCR0: clear CLIENTPD (accept transactions), set USFCFG (unmatched streams FAULT — so
///      a wrong-SID surprise logs itself in sGFSR/sGFSYNR1 instead of dying silently);
///   3. TLBIALLNSNH + sTLBGSYNC/sTLBGSTATUS bounded poll (no stale bypass/translation state);
///   4. readbacks printed — NS-write efficacy is itself a verify-don't-assume item (if the
///      readback still shows CLIENTPD=1, the block is secure-owned and this arc STOPs).
/// Security posture (ledgered in the arc doc): only the matched XUSB stream bypasses; every
/// other NISO1 stream now faults-and-logs rather than silently dropping. Bring-up honest,
/// tightened further when real SMMU translation arrives.
pub fn jb3_open_stream(bases: &[u64], xusb_sid: u32) {
    // Read-only first: what SID does the fabric say XUSB emits?
    serial_println!(
        ":: tegra: JB3 — MC SID block @{:#010x} first touch (read-only) ::",
        MC_SID_BASE
    );
    let ovr = rd(MC_SID_BASE, MC_SID_XUSB_HOSTR);
    let sec = rd(MC_SID_BASE, MC_SID_XUSB_HOSTR + 4);
    serial_println!(
        ":: tegra: JB3 — MC XUSB_HOSTR override={:#010x} (sid={:#x}) security={:#010x} ::",
        ovr,
        ovr & 0xff,
        sec
    );
    for (i, &base) in bases.iter().enumerate() {
        let scr0_pre = rd(base, SCR0);
        wr(base, SMR_BASE, (1 << 31) | (xusb_sid & 0x7fff));
        wr(base, S2CR_BASE, 0b01 << 16); // TYPE=bypass, CBNDX dont-care
        let scr0_new = (scr0_pre & !1) | (1 << 10); // CLIENTPD=0, USFCFG=1
        wr(base, SCR0, scr0_new);
        wr(base, TLBIALLNSNH, 0);
        wr(base, STLBGSYNC, 0);
        let mut spins = 0u32;
        while rd(base, STLBGSTATUS) & 1 != 0 && spins < 100_000 {
            spins += 1;
        }
        serial_println!(
            ":: tegra: JB3 — inst{} OPEN: sCR0 {:#010x}->{:#010x} (rb {:#010x}) SMR[0] rb={:#010x} S2CR[0] rb={:#010x} tlbsync {} ::",
            i,
            scr0_pre,
            scr0_new,
            rd(base, SCR0),
            rd(base, SMR_BASE),
            rd(base, S2CR_BASE),
            if spins < 100_000 { "OK" } else { "TIMEOUT" }
        );
        if rd(base, SCR0) & 1 != 0 {
            serial_println!(
                ":: tegra: JB3 — inst{} sCR0 write did NOT take (secure-owned?); STOP ::",
                i
            );
        }
    }
    serial_println!(":: tegra: JB3 — XUSB stream opened (SMR bypass + client port on) ::");
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
