// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// GA10B-PROBE1 — the FIRST read-only probe rung for the Orin Nano's Ampere GA10B iGPU. (GA10B-PROBE2, rung
// 2 — power + clocks + PMC_BOOT_0 — lives at the TAIL of this file under its SIBLING knob `ga10bprobe2`;
// GA10B-PROBE3, rungs 3 and 3b — the read-only pass over what the platform firmware left behind, and the
// ladder's first GA10B MMIO writes — lives after it under `ga10bprobe3` / `ga10bprobe3b`;
// see the ladder docs/dev/evidence/orin14/GA10B-LADDER.md and the as-built spec
// docs/dev/evidence/orin16/GA10B-RUNG3.md. Everything above the rung-2 marker is rung 1. GA10B-PROBE4, rungs 4a and 4b — the BCR writability census and the blob-free boot-ROM ignition — is the LAST block of this file under `ga10bprobe4a` / `ga10bprobe4b`; brief docs/dev/OS/08_VIDEO/GA10B-RUNG4-BRIEF.md. GA10B-PROBE4C, rung 4c — the read-only post-ignition CENSUS — and GA10B-PROBE4D, the DEFERRED arm that runs 4a+4b+4c from the shutdown path after a full desktop session, are the tail after it under `ga10bprobe4c` / `ga10bprobe4d`; brief §10. GA10B-PROBE4E and GA10B-PROBE4F, rungs 4e and 4f — the SHIFT arm and the BRFETCH arm, each the flown rung with exactly ONE value changed — are the VERY LAST block of this file under `ga10bprobe4e` / `ga10bprobe4f`; brief §12.)
// (`ga10bprobe1`, DEFAULT OFF; implies `tegra`). One attended cold-boot flight that answers, without
// booting one byte of GPU firmware or writing one GPU register: is the GA10B power rail on, has its
// GSP RISC-V boot ROM ever reached a verdict, and is the block priv-locked? See the design note
// docs/dev/OS/09_PLATFORM/ga10b-clean-room.md §3 and the register fact base
// docs/dev/OS/09_PLATFORM/ga10b-facts/ga10b-probe-rung1.facts.md (ACKED under §6). Every offset and
// bit below cites that FACTS FILE — never nvgpu, which no seat on this side of the clean-room wall
// reads.
//
// THE DISCIPLINE (inherited from JX1/JX2/JD1-DC, spelled out in the design note §2):
//   * BPMP POWER GATE FIRST. A read of a POWER-GATED Tegra block is EL3-FATAL (JX1: SError
//     ESR 0xbe000011, EC=0x2F, BL31 "Unhandled Exception in EL3"). So this rung asks the only
//     authority that knows — BPMP, over the HSP+IVC channel JB1b proved — for the GA10B power
//     domain's state (MRQ_PG GET_STATE) and touches NOT ONE byte of BAR0 unless the domain answers
//     ON. The domain id is a DTB fact (EXT: the `gpu@` node's `power-domains` phandle/id), never a
//     guess.
//   * ANNOUNCE-BEFORE-READ. Every new register is named on the wire BEFORE it is touched (the JX2/
//     jd1dc idiom), so if a read is fatal the last line on the wire names the killer exactly. The
//     reads ride ONE flight because the one-register-step-per-boot law is about WRITES — this rung
//     writes nothing — but the RISK ORDER (safest fuse first, priscv last) and announce-first are
//     mandatory.
//   * ZERO MMIO WRITES ON THIS RUNG. Every GPU access on rung 1 (and rungs 2 and 3) is
//     `core::ptr::read_volatile` — the block is left exactly as MB2 handed it over. The absolute
//     "there is no `write_volatile` in this module" form of this rule held until rung 3b
//     (`ga10bprobe3b`, at the very tail of this file), which is the ladder's designated FIRST-WRITE
//     rung: it owns the module's ONLY `w32` and that helper is `#[cfg(feature = "ga10bprobe3b")]`,
//     so every other configuration — rung 1, rung 2, rung 3 alone — still compiles with no write
//     path to a GA10B register at all. Read that as the invariant: writes exist only where a rung
//     was explicitly authorised to write, and they are announced one line before they happen.
//   * COLD-BOOT ENDING = MACHINE OFF. Per the 2026-08-25 bench law, a probe flight is its own media
//     and its next boot must be cold, so the flight ends in PSCI `SYSTEM_OFF` (`power::shutdown`, in
//     tree since 38d95900) — a dark board is the "ready for cold boot" signal. The shutdown is
//     reached only on the `ga10bprobe1` path, so no other configuration inherits it.
//
// WITNESS FAMILY `[ga10bprobe1]` (13 bytes with the brackets — well over the 8-byte LLVM
// immediate-encode floor that made shorter tokens invisible to `strings` on the artifact while fully
// working, orin-6 §7). Every verdict is one distinct line; the vocabulary is announced up front,
// the arms are mutually exclusive, and each arm is honest (a not-fused / not-locked / not-halted
// datum reads as such, never silently as its expected twin).

use super::fdt_tegra::Fdt;

// ── BPMP-ABI power-domain query (facts: bpmp_tegra.rs, the in-tree BPMP transport this rung reuses) ─
// MRQ_PG { cmd, id } with CMD_PG_GET_STATE = 2 is a PURE QUERY (zero mutation) — the same wire shape
// `jb5_pg_on` / `pg_get_state` already prove on metal, restated here so the probe borrows only the
// transport (`Chan::transfer`), not a jd1dc-gated helper. Response payload[0] = 1 (ON) / 0 (off).
// SET_STATE is deliberately NOT issued: powering or cycling the GPU domain is out of scope and the
// probe reads or refuses, it never powers anything.
const MRQ_PG: u32 = 66;
const CMD_PG_GET_STATE: u32 = 2;
const PG_STATE_ON: u32 = 1;

// ── GA10B BAR0-relative register facts — EVERY constant cites ga10b-probe-rung1.facts.md ────────────
// The GA10B GSP falcon/falcon2 engines live at fixed BAR0 offsets; priscv (RISC-V boot ROM) regs are
// falcon2-base-relative, so absolute = BAR0 + GSP_FALCON2_BASE + off.
//
// facts (Aperture framing): GSP falcon (v1) base in BAR0 = 0x00110000.
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const GSP_FALCON_BASE: u64 = 0x0011_0000;
// facts (Aperture framing): GSP falcon2 (RISC-V / priscv) base in BAR0 = 0x00111000.
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const GSP_FALCON2_BASE: u64 = 0x0011_1000;

// facts (b) Security-state fuses: opt_priv_sec_en 0x820434 (set => secure boot enforced). BAR0-rel.
#[cfg(feature = "ga10bprobe1")] const FUSE_OPT_PRIV_SEC_EN: u64 = 0x0082_0434;
// facts (b) Legacy Falcon regs: hwcfg2 0x0f4 — riscv_br_priv_lockdown bit13 (==1 => BR priv lockdown
// engaged). falcon-base-relative.
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe4a"))] const FALCON_HWCFG2_OFF: u64 = 0x0f4;
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe4a"))] const HWCFG2_PRIV_LOCKDOWN_BIT: u32 = 13;
// facts (b) RISC-V boot-ROM interface — priscv: br_retcode 0x65c — result bits[1:0]; FAIL=0x2,
// PASS=0x3; 0x0/0x1 = no verdict yet. falcon2-base-relative.
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe4a"))] const PRISCV_BR_RETCODE_OFF: u64 = 0x65c;
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe4a"))] const BR_RETCODE_FAIL: u32 = 0x2;
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe4a"))] const BR_RETCODE_PASS: u32 = 0x3;
// facts (b) RISC-V boot-ROM interface — priscv: cpuctl 0x388 — halted bit4. falcon2-base-relative.
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_CPUCTL_OFF: u64 = 0x388;
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_CPUCTL_HALTED_BIT: u32 = 4;
// facts (b) Die-characterization: top_num_gpcs 0x022430 value bits[4:0] (GA10B Orin Nano = 2 GPC).
// BAR0-relative.
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe4c"))] const TOP_NUM_GPCS: u64 = 0x0002_2430;
#[cfg(feature = "ga10bprobe1")] const TOP_NUM_GPCS_MASK: u32 = 0x1f;

/// One read-only 32-bit BAR0 access. Its write counterpart (`w32`) exists ONLY under
/// `ga10bprobe3b`, the ladder's designated first-write rung; in every other configuration this is
/// the module's only GA10B MMIO primitive.
#[inline]
fn r32(pa: u64) -> u32 {
    unsafe { core::ptr::read_volatile(pa as *const u32) }
}

/// The `gpu@` node's two DTB facts this rung needs (EXT — resolved from the Orin FDT, never guessed):
/// BAR0 physical base (`reg` entry[0]) and the BPMP power-domain id (`power-domains` [phandle, id]).
struct GpuNode {
    bar0: u64,
    /// The power-domain id (odd word of the [phandle, id] pair), or `None` if the node lists none —
    /// in which case the rail cannot be proven ON and NO BAR0 register may be read.
    pd_id: Option<u32>,
    /// The BPMP clock ids of the node's `clocks` = <&bpmp ID>... pairs (odd words), in DTB order; rung 2's
    /// MRQ_CLK list. Rung 1 ignores them.
    clocks: [u32; 8],
    n_clocks: usize,
}

/// Resolve the `gpu@` node from the live firmware DTB — a pure RAM walk, ZERO MMIO. Matches the node
/// name component `gpu@` (the Tegra234 iGPU wrapper), reads `reg` entry[0] as the BAR0 aperture
/// (addr:2, size:2 cells) and the odd word of `power-domains` as the domain id. `None` = no usable
/// `gpu@` node (the rung refuses rather than guessing an aperture — verify-don't-assume, the JX1
/// rule that a wrong aperture is fatal).
fn resolve_gpu_node(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<GpuNode> {
    if dtb_addr == 0 || dtb_size == 0 {
        return None;
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return None;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let fdt = Fdt::new(blob)?;
    // Find the FIRST `gpu@` node (the leaf name component, not a `gpus`/`gpu-something` sibling: the
    // '@' is what pins it to a unit-addressed device node).
    let mut path = [0u8; super::fdt_tegra::MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        if plen == 0 && e.path.windows(4).any(|q| q == b"gpu@") {
            let l = e.path.len().min(super::fdt_tegra::MAX_PATH);
            path[..l].copy_from_slice(&e.path[..l]);
            plen = l;
        }
    });
    if plen == 0 {
        return None;
    }
    let node = &path[..plen];
    let reg = fdt.prop_at(node, b"reg");
    if reg.n < 4 {
        return None;
    }
    let bar0 = ((reg.words[0] as u64) << 32) | reg.words[1] as u64;
    if bar0 == 0 {
        return None;
    }
    // power-domains = <&bpmp DOMAIN_ID> — [phandle, id] pair; the id is the odd (index 1) word.
    let pds = fdt.prop_at(node, b"power-domains");
    let pd_id = if pds.found && pds.n >= 2 { Some(pds.words[1]) } else { None };
    // clocks = <&bpmp ID>, ... — [phandle, id] pairs (bpmp #clock-cells = 1, the same shape xusb_ids reads);
    // keep the odd words, up to 8.
    let cks = fdt.prop_at(node, b"clocks");
    let mut clocks = [0u32; 8];
    let mut n_clocks = 0usize;
    let mut i = 1;
    while cks.found && i < cks.n && n_clocks < clocks.len() {
        clocks[n_clocks] = cks.words[i];
        n_clocks += 1;
        i += 2;
    }
    Some(GpuNode { bar0, pd_id, clocks, n_clocks })
}

/// GA10B-PROBE1 — the guarded, read-only probe. Runs from `tegra_early_stop`'s BPMP block (BPMP-first:
/// it borrows the `chan` `jb1b_ping` established). ENDS THE BOOT: on every reachable path it powers
/// the board OFF (`power::shutdown`, PSCI SYSTEM_OFF) rather than returning, because a probe flight's
/// next boot must be cold and a dark board is the ready-for-cold-boot signal.
#[cfg(feature = "ga10bprobe1")] pub fn ga10bprobe1_run(
    chan: &super::bpmp_tegra::Chan,
    dtb_addr: u64,
    dtb_size: usize,
    ram_gib_mask: u64,
) -> ! {
    // Verdict vocabulary announced UP FRONT (design note §3): mutually exclusive, honest arms.
    serial_println!(
        "[ga10bprobe1] rung 1 (READ-ONLY, zero MMIO writes) — verdict vocabulary: GA10B-RAIL-GATED | GA10B-RAIL-POWERED ; GA10B-SECURE-FUSED ; GA10B-PRIVLOCK-ENGAGED ; GA10B-BROM-NEVERRAN | GA10B-BROM-PASSED | GA10B-BROM-FAILED ; GA10B-CORE-HALTED ; GA10B-GPC-CENSUS=<n> ; per-register GA10B-*-UNREADABLE (all-ones)"
    );

    // 1. THE APERTURE + THE DOMAIN — pure DTB RAM walk, no MMIO. Both come from the SAME `gpu@` node
    //    so "the domain we prove ON owns the aperture we read" is true by construction (the JD1-DC
    //    same-node rule), not by assumption.
    let Some(gpu) = resolve_gpu_node(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!(
            "[ga10bprobe1] REFUSED reason=no-gpu-node — the firmware DTB carries no usable gpu@ node (reg/power-domains); NOT ONE BAR0 register was read"
        );
        finish()
    };
    serial_println!(
        "[ga10bprobe1] gpu@ node: BAR0={:#x} (DTB reg[0], EXT) power-domain-id={} (DTB power-domains, EXT)",
        gpu.bar0,
        match gpu.pd_id {
            Some(id) => id as i64,
            None => -1,
        },
    );
    let Some(pd_id) = gpu.pd_id else {
        serial_println!(
            "[ga10bprobe1] GA10B-RAIL-GATED verdict=indeterminate reason=no-power-domains — gpu@ lists no power-domains id, so MRQ_PG has nothing to ask and the rail state cannot be proven; a read of a gated block is EL3-FATAL (JX1), so NOT ONE BAR0 register was read"
        );
        finish()
    };

    // 2. THE POWER GATE — MRQ_PG GET_STATE, read-only, over the JB1b channel. Announce the transaction
    //    BEFORE issuing it (announce-first extends to BPMP MRQs per the design note). If the domain is
    //    not provably ON, the flight STOPS here and never touches BAR0.
    serial_println!(
        "[ga10bprobe1] BPMP MRQ_PG GET_STATE (read-only) for GA10B power-domain id={} — proving the rail BEFORE any BAR0 touch (JX1: gated read is EL3-fatal)",
        pd_id,
    );
    match chan.transfer(MRQ_PG, &[CMD_PG_GET_STATE, pd_id]) {
        Some((err, out)) if err == 0 && out[0] == PG_STATE_ON => {
            serial_println!(
                "[ga10bprobe1] GA10B-RAIL-POWERED reg=bpmp-mrq-pg val={:#010x} (err={} state=0x1) — the domain is ON; BAR0 reads may proceed in risk order",
                out[0],
                err,
            );
        }
        Some((err, out)) => {
            serial_println!(
                "[ga10bprobe1] GA10B-RAIL-GATED reg=bpmp-mrq-pg val={:#010x} (err={} state need 0x1) — the GA10B rail is NOT provably ON; a read of a gated block is EL3-FATAL (JX1), so NOT ONE BAR0 register was read",
                out[0],
                err,
            );
            finish();
        }
        None => {
            serial_println!(
                "[ga10bprobe1] GA10B-RAIL-GATED reg=bpmp-mrq-pg val=timeout — MRQ_PG GET_STATE got no response frame in 100 ms; rail state UNKNOWN and unknown is not ON, so NOT ONE BAR0 register was read"
            );
            finish();
        }
    }

    // ── past this point, and only past this point, GA10B BAR0 MMIO is touched — READ-ONLY, in risk
    //    order (safest fuse first, priscv boot-ROM state last), each read ANNOUNCED before it. GiB 0
    //    is already mapped Device-nGnRE by mmu_tegra and every offset below lands inside it. ──
    let base = gpu.bar0;
    // An all-ones datum on a POWERED rail is an UNREADABLE register (priv-locked / not decoding), not
    // rail-gated — a first-class datum, reported per register, never silently folded into a value.
    let unreadable = |v: u32| v == 0xFFFF_FFFF;

    // 2a. RISK STEP 1 — fuse opt_priv_sec_en (BAR0 0x820434, facts (b)). Expect 1 (secure boot fused).
    //     The fuse block is the safest aperture, so it is read first.
    let addr = base + FUSE_OPT_PRIV_SEC_EN;
    serial_println!(
        "[ga10bprobe1] about-to-read fuse_opt_priv_sec_en reg={:#x} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it",
        addr,
    );
    let sec = r32(addr);
    if unreadable(sec) {
        serial_println!("[ga10bprobe1] GA10B-SECURE-UNREADABLE reg={:#x} val={:#010x} — fuse read all-ones on a powered rail", addr, sec);
    } else if sec & 1 != 0 {
        serial_println!("[ga10bprobe1] GA10B-SECURE-FUSED reg={:#x} val={:#010x} (bit0=1) — production secure boot is fused (expected)", addr, sec);
    } else {
        serial_println!("[ga10bprobe1] GA10B-SECURE-NOTFUSED reg={:#x} val={:#010x} (bit0=0) — secure boot NOT fused (unexpected on this silicon; first-class finding)", addr, sec);
    }

    // 2b. RISK STEP 2 — falcon_hwcfg2 bit13 (GSP falcon base 0x110000 + 0x0f4, facts (b)). Expect
    //     engaged (BR priv lockdown) on secure silicon.
    let addr = base + GSP_FALCON_BASE + FALCON_HWCFG2_OFF;
    serial_println!(
        "[ga10bprobe1] about-to-read falcon_hwcfg2 reg={:#x} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it",
        addr,
    );
    let hwcfg2 = r32(addr);
    if unreadable(hwcfg2) {
        serial_println!("[ga10bprobe1] GA10B-PRIVLOCK-UNREADABLE reg={:#x} val={:#010x} — hwcfg2 read all-ones on a powered rail", addr, hwcfg2);
    } else if hwcfg2 & (1 << HWCFG2_PRIV_LOCKDOWN_BIT) != 0 {
        serial_println!("[ga10bprobe1] GA10B-PRIVLOCK-ENGAGED reg={:#x} val={:#010x} (bit13=1) — GSP BR priv-lockdown engaged (expected); priscv reads below may return locked values, itself the datum", addr, hwcfg2);
    } else {
        serial_println!("[ga10bprobe1] GA10B-PRIVLOCK-OPEN reg={:#x} val={:#010x} (bit13=0) — BR priv-lockdown NOT engaged (unexpected on secure silicon; first-class finding)", addr, hwcfg2);
    }

    // 2c. RISK STEP 3 — priscv br_retcode result bits[1:0] (GSP falcon2 base 0x111000 + 0x65c,
    //     facts (b)). Expect 0x0 (BR never reached a verdict — MB2 loads no GPU firmware). 0x2/0x3 is
    //     a first-class finding (contradicts "no GPU fw loaded", design note §3).
    let addr = base + GSP_FALCON2_BASE + PRISCV_BR_RETCODE_OFF;
    serial_println!(
        "[ga10bprobe1] about-to-read priscv_br_retcode reg={:#x} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it",
        addr,
    );
    let retcode = r32(addr);
    if unreadable(retcode) {
        serial_println!("[ga10bprobe1] GA10B-BROM-UNREADABLE reg={:#x} val={:#010x} — br_retcode read all-ones on a powered rail", addr, retcode);
    } else {
        let result = retcode & 0b11;
        match result {
            BR_RETCODE_PASS => serial_println!("[ga10bprobe1] GA10B-BROM-PASSED reg={:#x} val={:#010x} (result[1:0]=0x3) — GSP boot ROM reported PASS (FIRST-CLASS FINDING: contradicts 'MB2 loads no GPU firmware')", addr, retcode),
            BR_RETCODE_FAIL => serial_println!("[ga10bprobe1] GA10B-BROM-FAILED reg={:#x} val={:#010x} (result[1:0]=0x2) — GSP boot ROM reported FAIL (FIRST-CLASS FINDING: the BR ran and rejected its payload)", addr, retcode),
            _ => serial_println!("[ga10bprobe1] GA10B-BROM-NEVERRAN reg={:#x} val={:#010x} (result[1:0]={:#x}, 0x0/0x1 = no verdict) — GSP boot ROM never reached a verdict (expected: no GPU fw is loaded by MB2)", addr, retcode, result),
        }
    }

    // 2d. RISK STEP 4 — priscv cpuctl halted bit4 (GSP falcon2 base 0x111000 + 0x388, facts (b)).
    //     Expect 1 (RISC-V core halted).
    let addr = base + GSP_FALCON2_BASE + PRISCV_CPUCTL_OFF;
    serial_println!(
        "[ga10bprobe1] about-to-read priscv_cpuctl reg={:#x} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it",
        addr,
    );
    let cpuctl = r32(addr);
    if unreadable(cpuctl) {
        serial_println!("[ga10bprobe1] GA10B-CORE-UNREADABLE reg={:#x} val={:#010x} — priscv cpuctl read all-ones on a powered rail", addr, cpuctl);
    } else if cpuctl & (1 << PRISCV_CPUCTL_HALTED_BIT) != 0 {
        serial_println!("[ga10bprobe1] GA10B-CORE-HALTED reg={:#x} val={:#010x} (bit4=1) — GSP RISC-V core is halted (expected)", addr, cpuctl);
    } else {
        serial_println!("[ga10bprobe1] GA10B-CORE-RUNNING reg={:#x} val={:#010x} (bit4=0) — GSP RISC-V core NOT halted (unexpected; a running core with no fw loaded is a first-class finding)", addr, cpuctl);
    }

    // 2e. RISK STEP 5 — top_num_gpcs bits[4:0] (BAR0 0x022430, facts (b)). Expect 2 (GA10B Orin Nano
    //     = 2 GPC). The die-identity cross-check; last because the top block is the most likely to be
    //     an unmapped aperture class if our BAR0 base is wrong.
    let addr = base + TOP_NUM_GPCS;
    serial_println!(
        "[ga10bprobe1] about-to-read top_num_gpcs reg={:#x} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it",
        addr,
    );
    let top = r32(addr);
    if unreadable(top) {
        serial_println!("[ga10bprobe1] GA10B-GPC-UNREADABLE reg={:#x} val={:#010x} — top_num_gpcs read all-ones on a powered rail", addr, top);
    } else {
        let gpcs = top & TOP_NUM_GPCS_MASK;
        serial_println!(
            "[ga10bprobe1] GA10B-GPC-CENSUS={} reg={:#x} val={:#010x} (bits[4:0]) — {} GPC(s) present ({})",
            gpcs,
            addr,
            top,
            gpcs,
            if gpcs == 2 { "expected: GA10B Orin Nano = 2 GPC" } else { "unexpected count — cross-check the resolved BAR0 base" },
        );
    }

    serial_println!("[ga10bprobe1] rung 1 complete — read list exhausted, zero MMIO writes performed; ending the flight in SYSTEM_OFF (cold-boot bench law)");
    finish();
}

/// End the probe flight the cold-boot way: power the board OFF (PSCI SYSTEM_OFF via `power::shutdown`,
/// in tree since 38d95900). Never returns. The shutdown is reachable ONLY down the `ga10bprobe1`
/// path, so no other configuration inherits it.
#[cfg(feature = "ga10bprobe1")] fn finish() -> ! {
    serial_println!("[ga10bprobe1] flight done — powering OFF; the dark board is the ready-for-cold-boot signal");
    crate::power::shutdown()
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// GA10B-PROBE2 — RUNG 2: power + clocks + one PMC_BOOT_0 read (`ga10bprobe2`, DEFAULT OFF; implies
// `tegra`; a SIBLING of `ga10bprobe1`, never its dependent — rung 1's `ga10bprobe1_run` is `-> !` and
// ends the boot in SYSTEM_OFF unconditionally, and rung 2 must RETURN so the desktop boot continues
// behind it: the flight is a full boot). Peter's ruling 2026-09-06 (RULINGS R18): probe the hardware
// boot by boot. The ladder, one rung per boot from where rung 1 stopped, is
// docs/dev/evidence/orin14/GA10B-LADDER.md; this is its rung 2 as code.
//
// THE QUESTION: with the GPU power partition ON and the `gpu@` node's DTB clocks ENABLED — both asked of
// BPMP, the only authority over Tegra234 power/clock state — does the GA10B's PMC block answer at BAR0+0
// with an Ampere chip id? Rung 1 (flight o3d, capture line-acm0/orin.log) found the rail ALREADY ON at
// the raw handoff (MRQ_PG GET_STATE id=35 -> 0x1) and read fuse/falcon/priscv/top registers without an
// SError, so on that board the power-on below is expected to be a no-op and the new facts this rung
// buys are (1) which GPU clocks UEFI left running, (2) whether BPMP lets us drive the GPU domain and
// clocks at all (every err on the wire), and (3) the PMC_BOOT_0 datum — the ONE new BAR0 address class
// this boot touches (one-new-aperture-per-boot, the JX3 model).
//
// WRITES: BPMP MRQs only (MRQ_PG SET_STATE, MRQ_CLK ENABLE/DISABLE) — there is STILL no MMIO write
// to any GA10B register in this module (`write_volatile` does not appear here). Every mutation is
// SYMMETRIC: the domain and each clock are read BEFORE they are driven, only what was OFF is turned ON,
// and everything this rung turned on is turned back OFF before it returns — the board is left as the
// rung found it, whatever it found.
//
// SError bound: the BAR0 read is guarded behind an EXPLICIT MRQ_PG GET_STATE readback of ON taken AFTER
// the power-on (never the pre-state, never the SET_STATE err alone). A public fact bounding the risk of
// a PRI read with the GPU sys clock gated does NOT exist on this side of the clean-room wall; what
// bounds it empirically is rung 1: five PRI reads on this rail with UEFI's clock state answered sanely.
// That is why the clock ENABLEs come BEFORE the read (they can only add clocks, never remove one) and
// why the clock pre-state is printed before anything is driven.
//
// PROVENANCE (never nvgpu — this executor read none; the rung-1 facts file is ACKED, the rest is public):
//   * MRQ_PG 66 / CMD_PG_SET_STATE 1 / CMD_PG_GET_STATE 2 / PG_STATE_OFF 0 / PG_STATE_ON 1, request
//     {cmd, id[, state]}, GET_STATE response payload[0] = state; MRQ_CLK 22, request word =
//     subcommand[31:24] | clk_id[23:0], CMD_CLK_IS_ENABLED 6 (response payload[0] = 0/1),
//     CMD_CLK_ENABLE 7, CMD_CLK_DISABLE 8 — Linux include/soc/tegra/bpmp-abi.h (SPDX GPL-2.0 OR MIT),
//     the same header bpmp_tegra.rs already cites; the wire shapes are the ones jb1c/jb5/jb7/clk_enable
//     prove on metal.
//   * The GPU power-domain id and clock ids are DTB facts (EXT) resolved from the live `gpu@` node —
//     rung 1 read power-domain-id=35 there, which is TEGRA234_POWER_DOMAIN_GPU = 35 in Linux
//     include/dt-bindings/power/tegra234-powergate.h; the clock ids are printed as found (expected
//     among TEGRA234_CLK_GPC0CLK 41 / GPC1CLK 236 / GPUSYS 304 / FUSE 40 / GPU_PWR 42 per
//     include/dt-bindings/clock/tegra234-clock.h — UNVERIFIED until the flight prints them).
//   * NV_PMC_BOOT_0 = BAR0 + 0x00000000, read-only — NVIDIA open-gpu-kernel-modules
//     src/common/inc/swref/published/ampere/ga100/dev_boot.h (MIT). Chipset id = bits[28:20] — envytools
//     rnndb/bus/pmc.xml (NV10+ ID form: CHIPSET bits 20-28) and nouveau nvkm (MIT). Ampere =
//     architecture 0x17 in that field's upper bits (GPU_ARCHITECTURE_AMPERE 0x0170, GPU_IMPLEMENTATION_
//     GA102 0x02 … — open-gpu-kernel-modules published/nv_arch.h, MIT); GA10B's implementation nibble
//     0xB is INFERRED from the GA10x naming rule those defines follow (GA102 -> 0x02, GA107 -> 0x07), so
//     the PASS predicate is the ARCHITECTURE match (0x17), and the implementation is a datum.
//   * A read returning 0xBADxxxxx-class values is the PRI fabric's error pattern (public, nouveau /
//     open-gpu-kernel-modules) — reported as its own arm, never folded into a value.
//
// WITNESS FAMILY `[ga10bprobe2]` (13 bytes bracketed — over the 8-byte LLVM immediate-encode floor).
// The summary line is exactly one of:
//   [ga10bprobe2] pg=<state> clk=<n>/<t> boot0=0x… -> POWERED …      (BAR0 read, Ampere id)
//   [ga10bprobe2] pg=<state> clk=<n>/<t> boot0=0x… -> UNPOWERED …    (BAR0 read, zero / all-ones / PRI-error)
//   [ga10bprobe2] pg=<state> clk=<n>/<t> boot0=n/a -> REFUSED reason=…  (no BAR0 read at all)

#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
const CMD_PG_SET_STATE: u32 = 1;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
const PG_STATE_OFF: u32 = 0;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
const MRQ_CLK: u32 = 22;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
const CMD_CLK_IS_ENABLED: u32 = 6;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
const CMD_CLK_ENABLE: u32 = 7;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
const CMD_CLK_DISABLE: u32 = 8;
/// NV_PMC_BOOT_0, BAR0-relative (open-gpu-kernel-modules ga100 dev_boot.h, MIT).
#[cfg(feature = "ga10bprobe2")]
const PMC_BOOT_0: u64 = 0x0;
/// PMC_BOOT_0 chipset field bits[28:20] (envytools pmc.xml / nouveau); Ampere = 0x17x.
#[cfg(feature = "ga10bprobe2")]
const BOOT0_CHIPSET_SHIFT: u32 = 20;
#[cfg(feature = "ga10bprobe2")]
const BOOT0_CHIPSET_MASK: u32 = 0x1ff;
#[cfg(feature = "ga10bprobe2")]
const BOOT0_ARCH_AMPERE: u32 = 0x17;
/// The expected full chipset id (arch 0x17, impl 0xB — the impl nibble INFERRED from the GA10x naming rule).
#[cfg(feature = "ga10bprobe2")]
const BOOT0_CHIPSET_GA10B_EXPECTED: u32 = 0x17b;
/// The PRI fabric's error-return pattern: bits[31:20] == 0xBAD (public: nouveau / open-gpu-kernel-modules).
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
const PRI_ERROR_PATTERN: u32 = 0xBAD0_0000;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
const PRI_ERROR_MASK: u32 = 0xFFF0_0000;

/// Bounded spin of ~`ms` milliseconds on CNTPCT (the bpmp_tegra `wait_ms` idiom, without a predicate):
/// the settle time between a power/clock MRQ and the BAR0 read. BPMP acks synchronously, so this is
/// margin, not a protocol requirement.
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
fn settle_ms(ms: u64) {
    let freq: u64;
    let start: u64;
    unsafe {
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) start, options(nomem, nostack, preserves_flags));
    }
    let budget = freq / 1000 * ms;
    loop {
        let now: u64;
        unsafe {
            core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) now, options(nomem, nostack, preserves_flags));
        }
        if now.wrapping_sub(start) > budget {
            return;
        }
        core::hint::spin_loop();
    }
}

/// MRQ_PG GET_STATE for one domain: `Some((err, state))`, `None` = 100 ms timeout. Pure query.
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
fn pg_state(chan: &super::bpmp_tegra::Chan, id: u32) -> Option<(i32, u32)> {
    chan.transfer(MRQ_PG, &[CMD_PG_GET_STATE, id]).map(|(err, out)| (err, out[0]))
}

/// MRQ_CLK with one subcommand for one clock id: `Some((err, payload[0]))`, `None` = timeout.
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3", feature = "ga10bprobe4a"))]
fn clk(chan: &super::bpmp_tegra::Chan, cmd: u32, id: u32) -> Option<(i32, u32)> {
    chan.transfer(MRQ_CLK, &[(cmd << 24) | (id & 0x00ff_ffff)]).map(|(err, out)| (err, out[0]))
}

/// GA10B-PROBE2 — rung 2. Runs from `tegra_early_stop`'s BPMP block BEFORE rung 1's call (so a co-armed
/// image runs rung 2, then rung 1's SYSTEM_OFF; the rung-2 flight arms rung 2 ALONE). RETURNS on every
/// path — the boot continues into the desktop behind it.
#[cfg(feature = "ga10bprobe2")]
pub fn ga10bprobe2_run(
    chan: &super::bpmp_tegra::Chan,
    dtb_addr: u64,
    dtb_size: usize,
    ram_gib_mask: u64,
) {
    serial_println!(
        "[ga10bprobe2] rung 2 (POWER + CLOCKS via BPMP, then ONE BAR0 read; zero GA10B MMIO writes; symmetric restore; RETURNS) — summary vocabulary: POWERED | UNPOWERED | REFUSED reason=<no-gpu-node|no-power-domains|pg-timeout|pg-on-refused|pg-readback-not-on>"
    );

    // 1. THE APERTURE, THE DOMAIN, THE CLOCKS — pure DTB RAM walk, no MMIO (rung 1's resolver + clocks).
    let Some(gpu) = resolve_gpu_node(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!("[ga10bprobe2] pg=n/a clk=0/0 boot0=n/a -> REFUSED reason=no-gpu-node — the firmware DTB carries no usable gpu@ node; nothing driven, nothing read; RETURNING");
        return;
    };
    serial_println!(
        "[ga10bprobe2] gpu@ node: BAR0={:#x} (DTB reg[0], EXT) power-domain-id={} (DTB power-domains, EXT) clocks={} (DTB clocks, EXT): {} {} {} {} {} {} {} {}",
        gpu.bar0,
        match gpu.pd_id { Some(id) => id as i64, None => -1 },
        gpu.n_clocks,
        gpu.clocks[0], gpu.clocks[1], gpu.clocks[2], gpu.clocks[3],
        gpu.clocks[4], gpu.clocks[5], gpu.clocks[6], gpu.clocks[7],
    );
    let Some(pd_id) = gpu.pd_id else {
        serial_println!("[ga10bprobe2] pg=n/a clk=0/{} boot0=n/a -> REFUSED reason=no-power-domains — gpu@ lists no power-domains id; a gated read is EL3-fatal (JX1), so nothing was driven and nothing read; RETURNING", gpu.n_clocks);
        return;
    };

    // 2. POWER — read the domain BEFORE driving it; drive it only if OFF; readback after.
    serial_println!("[ga10bprobe2] BPMP MRQ_PG GET_STATE (read-only) id={} — the pre-state, before anything is driven", pd_id);
    let pg_before = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe2] pg-before id={} err={} state={:#x} ({})", pd_id, err, st, if err == 0 && st == PG_STATE_ON { "ON as found — power-on below is a no-op by design" } else { "not ON — this rung will drive it ON" });
            if err == 0 { Some(st) } else { None }
        }
        None => {
            serial_println!("[ga10bprobe2] pg=timeout clk=0/{} boot0=n/a -> REFUSED reason=pg-timeout — MRQ_PG GET_STATE got no frame in 100 ms; nothing driven, nothing read; RETURNING", gpu.n_clocks);
            return;
        }
    };
    let mut we_powered = false;
    if pg_before != Some(PG_STATE_ON) {
        serial_println!("[ga10bprobe2] BPMP MRQ_PG SET_STATE id={} state=ON — the rung's first WRITE (a BPMP request, not an MMIO write); if this is the LAST line the BPMP transaction itself hung the boot", pd_id);
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_ON]) {
            Some((err, _)) => {
                serial_println!("[ga10bprobe2] pg-set-on id={} err={} ({})", pd_id, err, if err == 0 { "acked" } else { "REFUSED by BPMP — negative = -errno" });
                we_powered = err == 0;
            }
            None => serial_println!("[ga10bprobe2] pg-set-on id={} TIMEOUT (no frame in 100 ms)", pd_id),
        }
        settle_ms(2);
    }
    // The EXPLICIT readback — the only thing that earns the BAR0 read below.
    let pg_now = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe2] pg-readback id={} err={} state={:#x}", pd_id, err, st);
            if err == 0 { st } else { 0xffff_ffff }
        }
        None => {
            serial_println!("[ga10bprobe2] pg-readback id={} TIMEOUT", pd_id);
            0xffff_ffff
        }
    };

    // 3. CLOCKS — IS_ENABLED before, ENABLE only what is off, IS_ENABLED after. Every err on the wire.
    let mut enabled_by_us = [false; 8];
    let mut n_on_before = 0usize;
    let mut n_on_after = 0usize;
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        let before = match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe2] clk {} IS_ENABLED (before) err={} = {}", id, err, st);
                if err == 0 && st == 1 { n_on_before += 1; }
                if err == 0 { Some(st) } else { None }
            }
            None => {
                serial_println!("[ga10bprobe2] clk {} IS_ENABLED (before) TIMEOUT", id);
                None
            }
        };
        if before == Some(0) {
            serial_println!("[ga10bprobe2] clk {} ENABLE — BPMP request; if this is the LAST line the transaction hung the boot", id);
            match clk(chan, CMD_CLK_ENABLE, id) {
                Some((err, _)) => {
                    serial_println!("[ga10bprobe2] clk {} ENABLE err={}", id, err);
                    enabled_by_us[i] = err == 0;
                }
                None => serial_println!("[ga10bprobe2] clk {} ENABLE TIMEOUT", id),
            }
        }
    }
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe2] clk {} IS_ENABLED (after) err={} = {}", id, err, st);
                if err == 0 && st == 1 { n_on_after += 1; }
            }
            None => serial_println!("[ga10bprobe2] clk {} IS_ENABLED (after) TIMEOUT", id),
        }
    }
    serial_println!("[ga10bprobe2] clocks: {} of {} running before, {} of {} after this rung's enables", n_on_before, gpu.n_clocks, n_on_after, gpu.n_clocks);
    settle_ms(2);

    // 4. THE ONE BAR0 READ — only behind the explicit pg readback of ON.
    if pg_now == PG_STATE_ON {
        let addr = gpu.bar0 + PMC_BOOT_0;
        serial_println!("[ga10bprobe2] about-to-read pmc_boot_0 reg={:#x} — the ONE new BAR0 address class this boot; if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it", addr);
        let boot0 = r32(addr);
        let chipset = (boot0 >> BOOT0_CHIPSET_SHIFT) & BOOT0_CHIPSET_MASK;
        if boot0 == 0xFFFF_FFFF {
            serial_println!("[ga10bprobe2] pg={:#x} clk={}/{} boot0={:#010x} -> UNPOWERED reason=all-ones — the PMC block did not decode on a rail BPMP reports ON (priv-locked or unclocked; the clock census above is the next datum)", pg_now, n_on_after, gpu.n_clocks, boot0);
        } else if boot0 == 0 {
            serial_println!("[ga10bprobe2] pg={:#x} clk={}/{} boot0={:#010x} -> UNPOWERED reason=zero-id — PMC_BOOT_0 read zero (no chip id) on a rail BPMP reports ON", pg_now, n_on_after, gpu.n_clocks, boot0);
        } else if boot0 & PRI_ERROR_MASK == PRI_ERROR_PATTERN {
            serial_println!("[ga10bprobe2] pg={:#x} clk={}/{} boot0={:#010x} -> UNPOWERED reason=pri-error — the PRI fabric returned its error pattern (0xBADxxxxx): the target block is not reachable in this power/clock state", pg_now, n_on_after, gpu.n_clocks, boot0);
        } else if (chipset >> 4) == BOOT0_ARCH_AMPERE {
            serial_println!("[ga10bprobe2] pg={:#x} clk={}/{} boot0={:#010x} -> POWERED chipset={:#x} arch=0x17 (Ampere) impl={:#x} ({}) rev={:#x}", pg_now, n_on_after, gpu.n_clocks, boot0, chipset, chipset & 0xf, if chipset == BOOT0_CHIPSET_GA10B_EXPECTED { "GA10B as inferred" } else { "NOT the inferred 0xB — first-class datum" }, boot0 & 0xff);
        } else {
            serial_println!("[ga10bprobe2] pg={:#x} clk={}/{} boot0={:#010x} -> POWERED chipset={:#x} arch={:#x} (NOT Ampere 0x17 — first-class datum: cross-check the resolved BAR0 base)", pg_now, n_on_after, gpu.n_clocks, boot0, chipset, chipset >> 4);
        }
    } else {
        serial_println!("[ga10bprobe2] pg={:#x} clk={}/{} boot0=n/a -> REFUSED reason={} — the explicit readback did not say ON, and a read of a gated block is EL3-fatal (JX1): NOT ONE BAR0 register was read", pg_now, n_on_after, gpu.n_clocks, if we_powered { "pg-readback-not-on" } else { "pg-on-refused" });
    }

    // 5. SYMMETRIC RESTORE — undo exactly what this rung turned on, in reverse order; every err on the wire.
    let mut n_disabled = 0usize;
    for i in (0..gpu.n_clocks).rev() {
        if enabled_by_us[i] {
            let id = gpu.clocks[i];
            match clk(chan, CMD_CLK_DISABLE, id) {
                Some((err, _)) => {
                    serial_println!("[ga10bprobe2] clk {} DISABLE (restore) err={}", id, err);
                    if err == 0 { n_disabled += 1; }
                }
                None => serial_println!("[ga10bprobe2] clk {} DISABLE (restore) TIMEOUT", id),
            }
        }
    }
    let mut pg_final = pg_now;
    if we_powered {
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_OFF]) {
            Some((err, _)) => serial_println!("[ga10bprobe2] pg-set-off (restore) id={} err={}", pd_id, err),
            None => serial_println!("[ga10bprobe2] pg-set-off (restore) id={} TIMEOUT", pd_id),
        }
        match pg_state(chan, pd_id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe2] pg-final id={} err={} state={:#x}", pd_id, err, st);
                pg_final = st;
            }
            None => serial_println!("[ga10bprobe2] pg-final id={} TIMEOUT", pd_id),
        }
    }
    serial_println!(
        "[ga10bprobe2] restored: pg={:#x} (was {}) clocks-disabled={} of {} enabled here — board left as found",
        pg_final,
        match pg_before { Some(s) => s as i64, None => -1 },
        n_disabled,
        enabled_by_us.iter().filter(|b| **b).count(),
    );
    serial_println!("[ga10bprobe2] rung 2 complete — RETURNING to the boot (no SYSTEM_OFF; the flight is a full boot)");
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// GA10B-PROBE3 — RUNG 3 (read-only) and RUNG 3b (the ladder's FIRST GA10B MMIO writes).
// Knobs: `ga10bprobe3` (implies `tegra`) = rung 3 alone; `ga10bprobe3b` (implies `ga10bprobe3`) adds
// rung 3b. ONE env knob drives both: `UNAOS_GA10B_PROBE3=1` -> rung 3, `UNAOS_GA10B_PROBE3=2` -> rung
// 3 + rung 3b, so the read-only rung can be flown alone first. A THIRD SIBLING of
// `ga10bprobe1`/`ga10bprobe2`, never their dependent: like rung 2 this RETURNS, so the flight is a
// full boot and the desktop comes up behind it.
//
// THE QUESTIONS.
//   Rung 3  — did MB2/UEFI stage anything for the GPU's secure boot (a BCR DMA descriptor, a WPR/VPR
//             region, a PMU image), and what do the remaining security fuses, the MC engine enables
//             and the TOP device-info config say? This decides whether rung 4 starts from "nothing
//             staged" (the expected case — rung 1 measured `br_retcode = 0`) or from a
//             partially-configured boot ROM. Its TWO rung-4 inputs get their own summary lines:
//             `bcr_dmacfg lock_locked=<0|1>` and `opt_wpr_enabled=<v>`.
//   Rung 3's FIRST question — the one rung 2 left on the table: DTB clock id 236 answered `err=-22`
//             to `MRQ_CLK CMD_CLK_IS_ENABLED` on the rung-2 flight while 304 and 41 answered `err=0`.
//             Rung 3 asks BPMP directly, with PURE QUERIES: `CMD_CLK_GET_MAX_CLK_ID` once, then
//             `CMD_CLK_GET_ALL_INFO` and `CMD_CLK_GET_RATE` per clock. That discriminates the three
//             hypotheses on the wire — the id is outside BPMP's table (out of range, or in range with
//             no entry: the dt-bindings number exists but this firmware does not export it), or it IS
//             in the table and only the enable-state subcommand is refused for it. It is NOT a "wrong
//             id" question in the DTB sense: the DTB is what named 236, and rung 2 printed it as read.
//   Rung 3b — does a GSP engine reset (assert -> hold -> deassert) leave the falcon halted and
//             readable, and does a Falcon MAILBOX scratch register hold a written value? The smallest
//             possible write that proves this kernel can drive a GA10B engine register.
//
// THE BRACKET. Rung 3 reuses rung 2's PROVEN power+clock bracket verbatim in shape — MRQ_PG GET_STATE
// pre-state, SET_STATE ON only if it was off, an EXPLICIT GET_STATE readback (the only thing that
// earns a BAR0 touch: a read of a power-gated Tegra block is EL3-FATAL, JX1), MRQ_CLK IS_ENABLED /
// ENABLE / IS_ENABLED per DTB clock, then the reads, then a SYMMETRIC restore of exactly what this
// rung turned on, then RETURN. It shares rung 2's helpers (`pg_state`, `clk`, `settle_ms`) rather than
// re-deriving them, so the two rungs cannot drift.
//
// SCOPE FENCE. NOTHING here touches the display engine, the memory fabric, or any vendor pad block:
// the FWALL/nvdisplay SError convictions (GA10B-HISTORY.md S2/S4/S5 — the boot7e window sweep, ESR
// 0xbe000011) stand untouched, and every address below is inside the `gpu@` BAR0 aperture the DTB
// declares. Two BAR0 address classes are NEW this boot and each is announced before its first touch:
// MC (BAR0 + 0x200 block) and the PMU falcon2 aperture (BAR0 + 0x10b000) — the PMU goes LAST, per the
// ladder's risk order (fuses -> MC -> TOP -> GSP falcon v1 -> GSP priscv BCR -> PMU falcon2).
//
// PROVENANCE. Every offset/bit below is from the ACKED facts file
// docs/dev/OS/09_PLATFORM/ga10b-facts/ga10b-probe-rung1.facts.md (§6, ACK-WITH-EDITS 2026-08-25) —
// never nvgpu, which this executor did not read. The one exception is flagged where it is used:
// Falcon MAILBOX0 at falcon-base + 0x040 is PUBLIC-RECALLED (nouveau `nvkm/falcon`, MIT;
// open-gpu-kernel-modules `dev_falcon_v4.h`, MIT) and is corroborated only by the facts file's
// matching v1 offsets (irqmask 0x018, irqdest 0x01c, cpuctl 0x100, bootvec 0x104, hwcfg 0x108,
// dmactl 0x10c). It is a WRITE target, so its recalled status is printed on the wire beside it.
// The MRQ_CLK subcommand numbers CMD_CLK_GET_RATE = 1, CMD_CLK_GET_ALL_INFO = 14 and
// CMD_CLK_GET_MAX_CLK_ID = 15 come from the same Linux `include/soc/tegra/bpmp-abi.h`
// (SPDX GPL-2.0 OR MIT) the rest of this file cites; `-22` on the wire is `-BPMP_EINVAL` from that
// header's error table.
//
// WITNESS FAMILIES `[ga10bprobe3]` (14 bytes bracketed) and `[ga10bprobe3b]` (15) — both well over the
// 8-byte LLVM immediate-encode floor that makes a token findable with `strings` on the artifact.

/// facts (b) Security-state fuses: opt_sec_debug_en. BAR0-relative.
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const FUSE_OPT_SEC_DEBUG_EN: u64 = 0x0082_1040;
/// facts (b) Security-state fuses: opt_wpr_enabled (the ACR's write-protected region). BAR0-relative.
/// One of rung 4's two inputs — it gets its own summary line.
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const FUSE_OPT_WPR_ENABLED: u64 = 0x0082_05ec;
/// facts (b) Security-state fuses: opt_vpr_enabled. BAR0-relative.
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const FUSE_OPT_VPR_ENABLED: u64 = 0x0082_067c;
/// facts (b) Die-characterization: mc_enable. BAR0-relative. NEW ADDRESS CLASS this boot.
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const MC_ENABLE: u64 = 0x0000_0200;
/// facts (b) Die-characterization: mc_elpg_enable — xbar 0x4, l2 0x8, hub 0x20000000. BAR0-relative.
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const MC_ELPG_ENABLE: u64 = 0x0000_020c;
#[cfg(feature = "ga10bprobe3")] const MC_ELPG_XBAR: u32 = 0x4;
#[cfg(feature = "ga10bprobe3")] const MC_ELPG_L2: u32 = 0x8;
#[cfg(feature = "ga10bprobe3")] const MC_ELPG_HUB: u32 = 0x2000_0000;
/// facts (b) Die-characterization: top_device_info_cfg — version_init = 0x2; the device_info2 table
/// walk itself is rung 5's, not this rung's. BAR0-relative.
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const TOP_DEVICE_INFO_CFG: u64 = 0x0002_24fc;
/// facts (b) Legacy Falcon regs, falcon-base-relative: irqmask 0x018, irqdest 0x01c, idlestate 0x04c,
/// cpuctl 0x100 (halt_intr bit4), hwcfg 0x108, dmactl 0x10c (require_ctx bit0).
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const FALCON_IRQMASK_OFF: u64 = 0x018;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const FALCON_IRQDEST_OFF: u64 = 0x01c;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const FALCON_IDLESTATE_OFF: u64 = 0x04c;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const FALCON_CPUCTL_OFF: u64 = 0x100;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const FALCON_CPUCTL_HALT_INTR_BIT: u32 = 4;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const FALCON_HWCFG_OFF: u64 = 0x108;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const FALCON_DMACTL_OFF: u64 = 0x10c;
#[cfg(feature = "ga10bprobe3")] const FALCON_DMACTL_REQUIRE_CTX_BIT: u32 = 0;
/// facts (b) RISC-V boot-ROM interface, falcon2(priscv)-base-relative: bcr_ctrl 0x668,
/// bcr_dmacfg 0x66c (lock_locked 0x80000000), BCR DMA addrs 0x670..0x684, boot_vector 0x380/0x384,
/// riscv_irqmask 0x528, riscv_irqdest 0x52c.
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_BCR_CTRL_OFF: u64 = 0x668;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_BCR_DMACFG_OFF: u64 = 0x66c;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const BCR_DMACFG_LOCK_LOCKED: u32 = 0x8000_0000;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_BCR_PKCPARAM_LO_OFF: u64 = 0x670;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_BCR_PKCPARAM_HI_OFF: u64 = 0x674;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_BCR_FMCCODE_LO_OFF: u64 = 0x678;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_BCR_FMCCODE_HI_OFF: u64 = 0x67c;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_BCR_FMCDATA_LO_OFF: u64 = 0x680;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4a"))] const PRISCV_BCR_FMCDATA_HI_OFF: u64 = 0x684;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const PRISCV_BOOT_VECTOR_LO_OFF: u64 = 0x380;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const PRISCV_BOOT_VECTOR_HI_OFF: u64 = 0x384;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const PRISCV_RISCV_IRQMASK_OFF: u64 = 0x528;
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const PRISCV_RISCV_IRQDEST_OFF: u64 = 0x52c;
/// facts (Aperture framing): PMU falcon2 base in BAR0 = 0x0010b000 — a DISTINCT engine from the GSP,
/// and the one NEW engine aperture this rung reads. Its cpuctl sits at the priscv-relative 0x388, the
/// same offset the facts file gives for the GSP's (facts (b) RISC-V boot-ROM interface).
#[cfg(any(feature = "ga10bprobe3", feature = "ga10bprobe4c"))] const PMU_FALCON2_BASE: u64 = 0x0010_b000;

/// BPMP MRQ_CLK subcommands used ONLY as pure queries by rung 3's clock-identity block
/// (Linux include/soc/tegra/bpmp-abi.h, SPDX GPL-2.0 OR MIT — the header the rest of this file cites).
/// GET_RATE's response payload words [0],[1] are the rate lo/hi; GET_ALL_INFO's are `flags` and
/// `parent` (the name string and parent list that follow sit past the two words `Chan::transfer`
/// returns, and rung 3 deliberately does NOT widen the shared transport to reach them — a probe rung
/// does not get to edit the channel every other Tegra subsystem uses).
#[cfg(feature = "ga10bprobe3")] const CMD_CLK_GET_RATE: u32 = 1;
#[cfg(feature = "ga10bprobe3")] const CMD_CLK_GET_ALL_INFO: u32 = 14;
#[cfg(feature = "ga10bprobe3")] const CMD_CLK_GET_MAX_CLK_ID: u32 = 15;
/// `-BPMP_EINVAL` — the error rung 2 saw on clock id 236 (bpmp-abi.h error table).
#[cfg(feature = "ga10bprobe3")] const BPMP_EINVAL: i32 = 22;

/// facts (b): GSP engine reset — `pgsp_falcon_engine`, BAR0 0x001103c0, assert bit0 = 1, deassert = 0,
/// with a 10 us assert-to-deassert delay REQUIRED. Marked `[WRITE — probe omits]` for rung 1; rung 3b
/// is the rung that stops omitting it.
#[cfg(feature = "ga10bprobe3b")] const PGSP_FALCON_ENGINE: u64 = 0x0011_03c0;
#[cfg(feature = "ga10bprobe3b")] const PGSP_FALCON_ENGINE_RESET_BIT: u32 = 0x1;
/// Falcon MAILBOX0, falcon-base-relative. **PUBLIC-RECALLED, NOT FROM THE ACKED FACTS FILE** — see the
/// block comment above. Printed as recalled on the wire beside the write.
#[cfg(any(feature = "ga10bprobe3b", feature = "ga10bprobe4c"))] const FALCON_MAILBOX0_OFF: u64 = 0x040;
/// The scratch pattern: 0x5A5AA5A5 — neither all-zero nor all-ones, so a stuck bus is distinguishable
/// from a register that really holds it.
#[cfg(feature = "ga10bprobe3b")] const MAILBOX_PATTERN: u32 = 0x5A5A_A5A5;

/// The module's ONLY 32-bit MMIO WRITE, and it exists only under `ga10bprobe3b` — the ladder's
/// designated first-write rung. Every call site announces the write on its own line BEFORE issuing it.
#[cfg(feature = "ga10bprobe3b")]
#[inline]
fn w32(pa: u64, v: u32) {
    unsafe { core::ptr::write_volatile(pa as *mut u32, v) }
}

/// One rung-3 register: wire name, BAR0-relative offset, address-class label (announced on the first
/// touch of each class), and the expectation the ladder's §Rung 3 table records for it.
#[cfg(feature = "ga10bprobe3")]
struct R3 {
    name: &'static str,
    off: u64,
    class: &'static str,
    expect: &'static str,
}

/// The §Rung 3 read list, IN RISK ORDER: fuses (the safest aperture, and one rung 1 already read at
/// 0x820434) -> MC (NEW class) -> TOP (read at 0x022430 by rung 1) -> GSP falcon v1 (read at 0x1100f4
/// by rung 1) -> GSP priscv BCR (read at 0x11165c/0x111388 by rung 1) -> PMU falcon2 (NEW class, LAST).
#[cfg(feature = "ga10bprobe3")]
const RUNG3_REGS: &[R3] = &[
    R3 { name: "fuse_opt_sec_debug_en", off: FUSE_OPT_SEC_DEBUG_EN, class: "fuse", expect: "datum" },
    R3 { name: "fuse_opt_wpr_enabled", off: FUSE_OPT_WPR_ENABLED, class: "fuse", expect: "datum (rung-4 input: does the ACR's write-protected region exist?)" },
    R3 { name: "fuse_opt_vpr_enabled", off: FUSE_OPT_VPR_ENABLED, class: "fuse", expect: "datum" },
    R3 { name: "mc_enable", off: MC_ENABLE, class: "mc", expect: "datum: which engines UEFI left enabled" },
    R3 { name: "mc_elpg_enable", off: MC_ELPG_ENABLE, class: "mc", expect: "datum: xbar 0x4 / l2 0x8 / hub 0x20000000 bits" },
    R3 { name: "top_device_info_cfg", off: TOP_DEVICE_INFO_CFG, class: "top", expect: "version_init=0x2 (the device_info2 walk is rung 5's)" },
    R3 { name: "gsp_falcon_hwcfg", off: GSP_FALCON_BASE + FALCON_HWCFG_OFF, class: "gsp-falcon-v1", expect: "datum: IMEM/DMEM sizes, rung 4's load input" },
    R3 { name: "gsp_falcon_dmactl", off: GSP_FALCON_BASE + FALCON_DMACTL_OFF, class: "gsp-falcon-v1", expect: "datum: require_ctx bit0" },
    R3 { name: "gsp_falcon_idlestate", off: GSP_FALCON_BASE + FALCON_IDLESTATE_OFF, class: "gsp-falcon-v1", expect: "datum" },
    R3 { name: "gsp_falcon_irqmask", off: GSP_FALCON_BASE + FALCON_IRQMASK_OFF, class: "gsp-falcon-v1", expect: "datum" },
    R3 { name: "gsp_falcon_irqdest", off: GSP_FALCON_BASE + FALCON_IRQDEST_OFF, class: "gsp-falcon-v1", expect: "datum" },
    R3 { name: "gsp_falcon_cpuctl_v1", off: GSP_FALCON_BASE + FALCON_CPUCTL_OFF, class: "gsp-falcon-v1", expect: "halt_intr bit4 (the v1 view of halted; rung 1 read the priscv view as 0x10)" },
    R3 { name: "priscv_bcr_ctrl", off: GSP_FALCON2_BASE + PRISCV_BCR_CTRL_OFF, class: "gsp-priscv-bcr", expect: "0 expected — no BCR programmed (rung 1: br_retcode=0)" },
    R3 { name: "priscv_bcr_dmacfg", off: GSP_FALCON2_BASE + PRISCV_BCR_DMACFG_OFF, class: "gsp-priscv-bcr", expect: "lock_locked bit31 — rung-4 input: if set, the BCR is locked for this power cycle" },
    R3 { name: "priscv_bcr_pkcparam_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_PKCPARAM_LO_OFF, class: "gsp-priscv-bcr", expect: "0 expected" },
    R3 { name: "priscv_bcr_pkcparam_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_PKCPARAM_HI_OFF, class: "gsp-priscv-bcr", expect: "0 expected" },
    R3 { name: "priscv_bcr_fmccode_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCCODE_LO_OFF, class: "gsp-priscv-bcr", expect: "0 expected" },
    R3 { name: "priscv_bcr_fmccode_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCCODE_HI_OFF, class: "gsp-priscv-bcr", expect: "0 expected" },
    R3 { name: "priscv_bcr_fmcdata_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCDATA_LO_OFF, class: "gsp-priscv-bcr", expect: "0 expected" },
    R3 { name: "priscv_bcr_fmcdata_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCDATA_HI_OFF, class: "gsp-priscv-bcr", expect: "0 expected" },
    R3 { name: "priscv_boot_vector_lo", off: GSP_FALCON2_BASE + PRISCV_BOOT_VECTOR_LO_OFF, class: "gsp-priscv-bcr", expect: "datum" },
    R3 { name: "priscv_boot_vector_hi", off: GSP_FALCON2_BASE + PRISCV_BOOT_VECTOR_HI_OFF, class: "gsp-priscv-bcr", expect: "datum" },
    R3 { name: "priscv_riscv_irqmask", off: GSP_FALCON2_BASE + PRISCV_RISCV_IRQMASK_OFF, class: "gsp-priscv-bcr", expect: "datum" },
    R3 { name: "priscv_riscv_irqdest", off: GSP_FALCON2_BASE + PRISCV_RISCV_IRQDEST_OFF, class: "gsp-priscv-bcr", expect: "datum" },
    R3 { name: "pmu_falcon2_cpuctl", off: PMU_FALCON2_BASE + PRISCV_CPUCTL_OFF, class: "pmu-falcon2", expect: "halted bit4 — the PMU is a SECOND engine; NEW aperture, read LAST by design" },
];

/// Announce a BAR0 address class before its first touch, saying honestly whether ANY rung has read
/// inside it before — "new aperture" is the risk the one-class-per-boot (JX3) model is built around.
#[cfg(feature = "ga10bprobe3")]
fn announce_class(class: &str, base: u64) {
    let (newness, note) = match class {
        "fuse" => ("KNOWN", "rung 1 read 0x820434 in this class without fault"),
        "mc" => ("NEW", "no rung has touched the MC block on this die — announced before first touch"),
        "top" => ("KNOWN", "rung 1 read top_num_gpcs 0x022430 in this class without fault"),
        "gsp-falcon-v1" => ("KNOWN", "rung 1 read falcon hwcfg2 0x1100f4 in this class without fault"),
        "gsp-priscv-bcr" => ("KNOWN", "rung 1 read priscv br_retcode 0x11165c and cpuctl 0x111388 in this class without fault"),
        "pmu-falcon2" => ("NEW", "a SECOND engine's aperture at BAR0+0x10b000 — deliberately the LAST class this rung touches"),
        _ => ("UNKNOWN", "unclassified"),
    };
    serial_println!("[ga10bprobe3] address class {} ({} this boot, BAR0={:#x}) — {}", class, newness, base, note);
}

/// GA10B-PROBE3 — rungs 3 and (under `ga10bprobe3b`) 3b. Runs from `tegra_early_stop`'s BPMP block
/// between rung 2's call and rung 1's, borrowing the `chan` `jb1b_ping` established. RETURNS on every
/// path — the boot continues into the desktop behind it.
#[cfg(feature = "ga10bprobe3")]
pub fn ga10bprobe3_run(
    chan: &super::bpmp_tegra::Chan,
    dtb_addr: u64,
    dtb_size: usize,
    ram_gib_mask: u64,
) {
    serial_println!(
        "[ga10bprobe3] rung 3 (READ-ONLY register pass inside rung 2's PROVEN power+clock bracket; symmetric restore; RETURNS) — risk order: fuse -> mc -> top -> gsp-falcon-v1 -> gsp-priscv-bcr -> pmu-falcon2 (LAST). Two NEW BAR0 address classes this boot (mc, pmu-falcon2), each announced before first touch. No display, no fabric, no vendor pad block: the FWALL/nvdisplay SError convictions stand. Summary vocabulary: COMPLETE | REFUSED reason=<no-gpu-node|no-power-domains|pg-timeout|pg-on-refused|pg-readback-not-on>"
    );
    #[cfg(feature = "ga10bprobe3b")]
    serial_println!(
        "[ga10bprobe3b] rung 3b ARMED (UNAOS_GA10B_PROBE3=2) — after rung 3's reads this boot performs the ladder's FIRST GA10B MMIO WRITES: pgsp_falcon_engine reset ASSERT (bit0=1) -> hold -> DEASSERT (0x0) -> priscv cpuctl readback -> and ONLY if that read is sane, one MAILBOX0 scratch write + readback. EVERY write is announced on its own line BEFORE it happens, so if a write is fatal the last line on the wire names it exactly."
    );

    // 1. APERTURE + DOMAIN + CLOCKS — pure DTB RAM walk, zero MMIO (rung 1's resolver, rung 2's use).
    let Some(gpu) = resolve_gpu_node(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!("[ga10bprobe3] pg=n/a clk=0/0 -> REFUSED reason=no-gpu-node — the firmware DTB carries no usable gpu@ node; nothing driven, nothing read; RETURNING");
        return;
    };
    serial_println!(
        "[ga10bprobe3] gpu@ node: BAR0={:#x} (DTB reg[0], EXT) power-domain-id={} (DTB power-domains, EXT) clocks={} (DTB clocks, EXT): {} {} {} {} {} {} {} {}",
        gpu.bar0,
        match gpu.pd_id { Some(id) => id as i64, None => -1 },
        gpu.n_clocks,
        gpu.clocks[0], gpu.clocks[1], gpu.clocks[2], gpu.clocks[3],
        gpu.clocks[4], gpu.clocks[5], gpu.clocks[6], gpu.clocks[7],
    );
    let Some(pd_id) = gpu.pd_id else {
        serial_println!("[ga10bprobe3] pg=n/a clk=0/{} -> REFUSED reason=no-power-domains — gpu@ lists no power-domains id; a gated read is EL3-fatal (JX1), so nothing was driven and nothing read; RETURNING", gpu.n_clocks);
        return;
    };

    // 2. POWER — rung 2's bracket, unchanged in shape: pre-state, drive only if off, explicit readback.
    serial_println!("[ga10bprobe3] BPMP MRQ_PG GET_STATE (read-only) id={} — the pre-state, before anything is driven", pd_id);
    let pg_before = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe3] pg-before id={} err={} state={:#x} ({})", pd_id, err, st, if err == 0 && st == PG_STATE_ON { "ON as found — rung 2 measured the same; the power-on below is a no-op by design" } else { "not ON — this rung will drive it ON" });
            if err == 0 { Some(st) } else { None }
        }
        None => {
            serial_println!("[ga10bprobe3] pg=timeout clk=0/{} -> REFUSED reason=pg-timeout — MRQ_PG GET_STATE got no frame in 100 ms; nothing driven, nothing read; RETURNING", gpu.n_clocks);
            return;
        }
    };
    let mut we_powered = false;
    if pg_before != Some(PG_STATE_ON) {
        serial_println!("[ga10bprobe3] BPMP MRQ_PG SET_STATE id={} state=ON — a BPMP request, not an MMIO write; if this is the LAST line the BPMP transaction itself hung the boot", pd_id);
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_ON]) {
            Some((err, _)) => {
                serial_println!("[ga10bprobe3] pg-set-on id={} err={} ({})", pd_id, err, if err == 0 { "acked" } else { "REFUSED by BPMP — negative = -errno" });
                we_powered = err == 0;
            }
            None => serial_println!("[ga10bprobe3] pg-set-on id={} TIMEOUT (no frame in 100 ms)", pd_id),
        }
        settle_ms(2);
    }
    let pg_now = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe3] pg-readback id={} err={} state={:#x}", pd_id, err, st);
            if err == 0 { st } else { 0xffff_ffff }
        }
        None => {
            serial_println!("[ga10bprobe3] pg-readback id={} TIMEOUT", pd_id);
            0xffff_ffff
        }
    };

    // 3. CLOCKS — the same IS_ENABLED / ENABLE / IS_ENABLED census rung 2 ran, then the CLOCK-IDENTITY
    //    block that answers rung 2's leftover: why did BPMP answer err=-22 to IS_ENABLED on id 236?
    let mut enabled_by_us = [false; 8];
    let mut n_on_before = 0usize;
    let mut n_on_after = 0usize;
    let mut is_enabled_err = [0i32; 8];
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        let before = match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe3] clk {} IS_ENABLED (before) err={} = {}", id, err, st);
                is_enabled_err[i] = err;
                if err == 0 && st == 1 { n_on_before += 1; }
                if err == 0 { Some(st) } else { None }
            }
            None => {
                serial_println!("[ga10bprobe3] clk {} IS_ENABLED (before) TIMEOUT", id);
                is_enabled_err[i] = -1;
                None
            }
        };
        if before == Some(0) {
            serial_println!("[ga10bprobe3] clk {} ENABLE — BPMP request; if this is the LAST line the transaction hung the boot", id);
            match clk(chan, CMD_CLK_ENABLE, id) {
                Some((err, _)) => {
                    serial_println!("[ga10bprobe3] clk {} ENABLE err={}", id, err);
                    enabled_by_us[i] = err == 0;
                }
                None => serial_println!("[ga10bprobe3] clk {} ENABLE TIMEOUT", id),
            }
        }
    }
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe3] clk {} IS_ENABLED (after) err={} = {}", id, err, st);
                if err == 0 && st == 1 { n_on_after += 1; }
            }
            None => serial_println!("[ga10bprobe3] clk {} IS_ENABLED (after) TIMEOUT", id),
        }
    }
    serial_println!("[ga10bprobe3] clocks: {} of {} running before, {} of {} after this rung's enables", n_on_before, gpu.n_clocks, n_on_after, gpu.n_clocks);

    // 3a. CLOCK IDENTITY — pure MRQ_CLK QUERIES (zero mutation), the discriminator for rung 2's err=-22.
    serial_println!("[ga10bprobe3] clock-identity block (READ-ONLY MRQ_CLK queries; rung 3's FIRST question — why did id 236 answer err=-22 to IS_ENABLED on the rung-2 flight?): GET_MAX_CLK_ID once, then GET_ALL_INFO + GET_RATE per DTB clock, with a per-clock verdict so 236 is read against two same-boot controls (304, 41) and not alone");
    let max_clk_id = match chan.transfer(MRQ_CLK, &[CMD_CLK_GET_MAX_CLK_ID << 24]) {
        Some((err, out)) => {
            serial_println!("[ga10bprobe3] clk GET_MAX_CLK_ID err={} max_id={}", err, out[0]);
            if err == 0 { Some(out[0]) } else { None }
        }
        None => {
            serial_println!("[ga10bprobe3] clk GET_MAX_CLK_ID TIMEOUT");
            None
        }
    };
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        // These two want the response's FIRST TWO payload words, so they go through `chan.transfer`
        // directly rather than the `clk` helper (which keeps only payload[0]). GET_ALL_INFO: word 0
        // = flags, word 1 = parent. GET_RATE: words 0/1 = the rate lo/hi halves.
        let info_err = match chan.transfer(MRQ_CLK, &[(CMD_CLK_GET_ALL_INFO << 24) | (id & 0x00ff_ffff)]) {
            Some((err, out)) => {
                serial_println!("[ga10bprobe3] clk {} GET_ALL_INFO err={} flags={:#010x} parent={}", id, err, out[0], out[1]);
                err
            }
            None => {
                serial_println!("[ga10bprobe3] clk {} GET_ALL_INFO TIMEOUT", id);
                -1
            }
        };
        match chan.transfer(MRQ_CLK, &[(CMD_CLK_GET_RATE << 24) | (id & 0x00ff_ffff)]) {
            Some((err, out)) => serial_println!("[ga10bprobe3] clk {} GET_RATE err={} rate_lo={} rate_hi={}", id, err, out[0], out[1]),
            None => serial_println!("[ga10bprobe3] clk {} GET_RATE TIMEOUT", id),
        }
        let in_range = match max_clk_id { Some(m) => id <= m, None => true };
        let verdict = if is_enabled_err[i] == 0 {
            "BPMP-MANAGED — IS_ENABLED answered err=0; the id is in this firmware's clock table and its enable state is queryable"
        } else if info_err != 0 && !in_range {
            "NOT-IN-BPMP-TABLE (out of range) — the id is above GET_MAX_CLK_ID and GET_ALL_INFO refused it too: the dt-bindings number exists, this BPMP firmware's table does not carry it"
        } else if info_err != 0 {
            "NOT-IN-BPMP-TABLE (in range, no entry) — GET_ALL_INFO refused it too, so the id is not an entry this BPMP firmware exports to the CCPLEX; NOT a wrong id in our DTB read, because the DTB is what named it"
        } else {
            "IN-TABLE-BUT-ENABLE-STATE-REFUSED — GET_ALL_INFO answered err=0, so BPMP knows this clock; only the IS_ENABLED/ENABLE subcommands are refused for it (a per-clock capability, not a missing id and not the wrong MRQ)"
        };
        serial_println!("[ga10bprobe3] clk {} identity: is_enabled_err={} info_err={} in_range={} -> {}", id, is_enabled_err[i], info_err, in_range as u32, verdict);
        if is_enabled_err[i] == -BPMP_EINVAL {
            serial_println!("[ga10bprobe3] clk {} note: err=-22 is -BPMP_EINVAL (bpmp-abi.h error table) — an ARGUMENT rejection, never an -EACCES-class policy refusal; rung 2 saw exactly this on id 236", id);
        }
    }
    settle_ms(2);

    // 4. THE READS — only behind the explicit pg readback of ON, in the ladder's risk order, each
    //    register announced before it is touched and each address class announced before its first.
    let mut wpr_val: Option<u32> = None;
    let mut dmacfg_val: Option<u32> = None;
    let mut n_read = 0usize;
    let mut n_unreadable = 0usize;
    if pg_now == PG_STATE_ON {
        let base = gpu.bar0;
        let mut cur_class = "";
        for r in RUNG3_REGS {
            if r.class != cur_class {
                cur_class = r.class;
                announce_class(r.class, base);
            }
            let addr = base + r.off;
            serial_println!("[ga10bprobe3] about-to-read {} reg={:#x} (class={}) — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it", r.name, addr, r.class);
            let v = r32(addr);
            if v == 0xFFFF_FFFF {
                n_unreadable += 1;
                serial_println!("[ga10bprobe3] {} @{:#x} = -UNREADABLE reason=all-ones (on a rail BPMP reports ON: priv-locked or not decoding — a first-class datum, never folded into a value) expect={}", r.name, r.off, r.expect);
            } else if v & PRI_ERROR_MASK == PRI_ERROR_PATTERN {
                n_unreadable += 1;
                serial_println!("[ga10bprobe3] {} @{:#x} = -UNREADABLE reason=pri-error val={:#010x} (the PRI fabric's 0xBADxxxxx pattern) expect={}", r.name, r.off, v, r.expect);
            } else {
                n_read += 1;
                serial_println!("[ga10bprobe3] {} @{:#x} = {:#010x} expect={}", r.name, r.off, v, r.expect);
                if r.off == MC_ELPG_ENABLE {
                    serial_println!("[ga10bprobe3] mc_elpg_enable decode: xbar={} l2={} hub={}", (v & MC_ELPG_XBAR != 0) as u32, (v & MC_ELPG_L2 != 0) as u32, (v & MC_ELPG_HUB != 0) as u32);
                }
                if r.off == GSP_FALCON_BASE + FALCON_DMACTL_OFF {
                    serial_println!("[ga10bprobe3] gsp_falcon_dmactl decode: require_ctx={}", (v >> FALCON_DMACTL_REQUIRE_CTX_BIT) & 1);
                }
                if r.off == GSP_FALCON_BASE + FALCON_CPUCTL_OFF {
                    serial_println!("[ga10bprobe3] gsp_falcon_cpuctl_v1 decode: halt_intr(bit4)={}", (v >> FALCON_CPUCTL_HALT_INTR_BIT) & 1);
                }
                if r.off == PMU_FALCON2_BASE + PRISCV_CPUCTL_OFF {
                    serial_println!("[ga10bprobe3] pmu_falcon2_cpuctl decode: halted(bit4)={} — the PMU engine's own halt state", (v >> PRISCV_CPUCTL_HALTED_BIT) & 1);
                }
            }
            if r.off == FUSE_OPT_WPR_ENABLED && v != 0xFFFF_FFFF { wpr_val = Some(v); }
            if r.off == GSP_FALCON2_BASE + PRISCV_BCR_DMACFG_OFF && v != 0xFFFF_FFFF { dmacfg_val = Some(v); }
        }
        // THE TWO RUNG-4 INPUTS, each on its own summary line, in the shape the scorer greps for.
        match dmacfg_val {
            Some(v) => serial_println!("[ga10bprobe3] bcr_dmacfg lock_locked={} (raw={:#010x}) — 1 means the BCR is locked for this power cycle and rung 4 CANNOT reprogram it without a cold boot", ((v & BCR_DMACFG_LOCK_LOCKED) != 0) as u32, v),
            None => serial_println!("[ga10bprobe3] bcr_dmacfg lock_locked=-UNREADABLE — the register did not answer; rung 4's precondition is UNKNOWN, not clear"),
        }
        match wpr_val {
            Some(v) => serial_println!("[ga10bprobe3] opt_wpr_enabled={:#010x}", v),
            None => serial_println!("[ga10bprobe3] opt_wpr_enabled=-UNREADABLE"),
        }
        serial_println!("[ga10bprobe3] pg={:#x} clk={}/{} regs={} of {} readable, {} UNREADABLE -> COMPLETE", pg_now, n_on_after, gpu.n_clocks, n_read, RUNG3_REGS.len(), n_unreadable);

        // 5. RUNG 3b — the first GA10B MMIO writes, inside the same bracket, only under its own knob.
        #[cfg(feature = "ga10bprobe3b")]
        rung3b(base);
    } else {
        serial_println!("[ga10bprobe3] pg={:#x} clk={}/{} -> REFUSED reason={} — the explicit readback did not say ON, and a read of a gated block is EL3-fatal (JX1): NOT ONE BAR0 register was read and NO write was attempted", pg_now, n_on_after, gpu.n_clocks, if we_powered { "pg-readback-not-on" } else { "pg-on-refused" });
    }

    // 6. SYMMETRIC RESTORE — rung 2's, verbatim in shape: undo exactly what THIS rung turned on.
    let mut n_disabled = 0usize;
    for i in (0..gpu.n_clocks).rev() {
        if enabled_by_us[i] {
            let id = gpu.clocks[i];
            match clk(chan, CMD_CLK_DISABLE, id) {
                Some((err, _)) => {
                    serial_println!("[ga10bprobe3] clk {} DISABLE (restore) err={}", id, err);
                    if err == 0 { n_disabled += 1; }
                }
                None => serial_println!("[ga10bprobe3] clk {} DISABLE (restore) TIMEOUT", id),
            }
        }
    }
    let mut pg_final = pg_now;
    if we_powered {
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_OFF]) {
            Some((err, _)) => serial_println!("[ga10bprobe3] pg-set-off (restore) id={} err={}", pd_id, err),
            None => serial_println!("[ga10bprobe3] pg-set-off (restore) id={} TIMEOUT", pd_id),
        }
        match pg_state(chan, pd_id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe3] pg-final id={} err={} state={:#x}", pd_id, err, st);
                pg_final = st;
            }
            None => serial_println!("[ga10bprobe3] pg-final id={} TIMEOUT", pd_id),
        }
    }
    serial_println!(
        "[ga10bprobe3] restored: pg={:#x} (was {}) clocks-disabled={} of {} enabled here — board left as found (a GSP engine reset, if rung 3b ran, has no restore and needs none: the engine was halted and never-booted before and after)",
        pg_final,
        match pg_before { Some(s) => s as i64, None => -1 },
        n_disabled,
        enabled_by_us.iter().filter(|b| **b).count(),
    );
    serial_println!("[ga10bprobe3] rung 3 complete — RETURNING to the boot (no SYSTEM_OFF; the flight is a full boot)");
}

/// RUNG 3b — the ladder's FIRST GA10B MMIO writes. Called from `ga10bprobe3_run` ONLY after the
/// explicit `pg=ON` readback earned the reads and the read list is exhausted, so a fatal write can
/// never be confused with a fatal read. Three writes at most, each announced on its own line BEFORE
/// it happens; the MAILBOX write is SKIPPED unless the post-reset cpuctl read is sane. Restore: none
/// is possible for a reset, and none is needed — the engine was halted and never-booted before
/// (rung 1: `br_retcode=0`, `cpuctl=0x10`) and is halted and never-booted after.
#[cfg(feature = "ga10bprobe3b")]
fn rung3b(base: u64) {
    serial_println!("[ga10bprobe3b] rung 3b — GSP engine reset (assert -> hold -> deassert) then, only if the readback is sane, ONE MAILBOX0 scratch write. Verdict vocabulary: MAILBOX-HELD | MAILBOX-MISMATCH | MAILBOX-SKIPPED reason=<cpuctl-all-ones|cpuctl-pri-error>");
    let engine = base + PGSP_FALCON_ENGINE;
    serial_println!("[ga10bprobe3b] address class gsp-falcon-engine (a KNOWN aperture, but a NEW ACCESS KIND: BAR0={:#x}, pgsp_falcon_engine at {:#x}) — the module's first write_volatile to a GA10B register", base, engine);

    serial_println!("[ga10bprobe3b] about-to-WRITE pgsp_falcon_engine reg={:#x} val={:#010x} (engine reset ASSERT, bit0=1) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", engine, PGSP_FALCON_ENGINE_RESET_BIT);
    w32(engine, PGSP_FALCON_ENGINE_RESET_BIT);
    // facts (b) require >= 10 us assert-to-deassert. 1 ms is the coarsest bounded wait this module
    // has and it is ~100x the requirement — margin, never a shorter hold.
    settle_ms(1);
    serial_println!("[ga10bprobe3b] assert held >= 1 ms (facts require >= 10 us)");

    serial_println!("[ga10bprobe3b] about-to-WRITE pgsp_falcon_engine reg={:#x} val=0x00000000 (engine reset DEASSERT) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", engine);
    w32(engine, 0);
    settle_ms(1);

    let cpuctl_addr = base + GSP_FALCON2_BASE + PRISCV_CPUCTL_OFF;
    serial_println!("[ga10bprobe3b] about-to-read priscv_cpuctl reg={:#x} (post-reset readback; rung 1 measured 0x10 = halted before any reset ever ran) — if this is the LAST line, THAT read was EL3-fatal", cpuctl_addr);
    let cpuctl = r32(cpuctl_addr);
    if cpuctl == 0xFFFF_FFFF {
        serial_println!("[ga10bprobe3b] priscv_cpuctl @{:#x} = -UNREADABLE reason=all-ones after the reset — the engine did not come back readable; the MAILBOX write is SKIPPED by design", GSP_FALCON2_BASE + PRISCV_CPUCTL_OFF);
        serial_println!("[ga10bprobe3b] -> MAILBOX-SKIPPED reason=cpuctl-all-ones");
        serial_println!("[ga10bprobe3b] rung 3b complete");
        return;
    }
    if cpuctl & PRI_ERROR_MASK == PRI_ERROR_PATTERN {
        serial_println!("[ga10bprobe3b] priscv_cpuctl @{:#x} = -UNREADABLE reason=pri-error val={:#010x} — the PRI fabric refused the target after the reset; the MAILBOX write is SKIPPED by design", GSP_FALCON2_BASE + PRISCV_CPUCTL_OFF, cpuctl);
        serial_println!("[ga10bprobe3b] -> MAILBOX-SKIPPED reason=cpuctl-pri-error");
        serial_println!("[ga10bprobe3b] rung 3b complete");
        return;
    }
    serial_println!("[ga10bprobe3b] priscv_cpuctl @{:#x} = {:#010x} halted(bit4)={} — sane, so the MAILBOX write is authorised", GSP_FALCON2_BASE + PRISCV_CPUCTL_OFF, cpuctl, (cpuctl >> PRISCV_CPUCTL_HALTED_BIT) & 1);

    let mbox = base + GSP_FALCON_BASE + FALCON_MAILBOX0_OFF;
    serial_println!("[ga10bprobe3b] about-to-WRITE gsp_falcon_mailbox0 reg={:#x} val={:#010x} — POINTER IS PUBLIC-RECALLED (nouveau nvkm/falcon; open-gpu-kernel-modules dev_falcon_v4.h; both MIT), NOT from the ACKED facts file: falcon-base+0x040, corroborated only by that file's matching v1 offsets. If this is the LAST line, THAT WRITE was fatal and the boot ended inside it", mbox, MAILBOX_PATTERN);
    w32(mbox, MAILBOX_PATTERN);
    serial_println!("[ga10bprobe3b] about-to-read gsp_falcon_mailbox0 reg={:#x} (the scratch readback) — if this is the LAST line, THAT read was EL3-fatal", mbox);
    let got = r32(mbox);
    serial_println!("[ga10bprobe3b] mailbox0 wrote={:#010x} read={:#010x}", MAILBOX_PATTERN, got);
    if got == MAILBOX_PATTERN {
        serial_println!("[ga10bprobe3b] -> MAILBOX-HELD — a GA10B engine register accepted a write from this kernel and held it; the CCPLEX can drive this engine's scratch state with the GSP halted");
    } else {
        serial_println!("[ga10bprobe3b] -> MAILBOX-MISMATCH read={:#010x} — the write did not stick (priv-locked scratch, a wrong pointer, or a register that is not a plain scratch); a first-class datum, and the RECALLED pointer is the first thing to re-verify", got);
    }
    serial_println!("[ga10bprobe3b] rung 3b complete");
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// GA10B-PROBE4 — RUNGS 4a and 4b: the BCR writability census and the BLOB-FREE boot-ROM ignition
// (`ga10bprobe4a` / `ga10bprobe4b`, DEFAULT OFF; `ga10bprobe4a` implies `tegra`, `ga10bprobe4b` implies
// `ga10bprobe4a`; a FOURTH sibling of the probe knobs, never their dependent). Implemented by orin 26 from
// the FROZEN design docs/dev/OS/08_VIDEO/GA10B-RUNG4-BRIEF.md (e7c8eb24); every register, bit, constant
// and ordering below is the ACKED facts file's §(b) "RISC-V boot-ROM interface" and "Boot-ROM handshake
// ordering (SEQ)" — nothing here was extracted from nvgpu, which this seat has not read.
//
// THE ONE FACT THE RUNG TURNS ON: `br_retcode` (priscv 0x65c) has never left 0x0 on this die (rung 1,
// render8, render11). Moving it to 0x2 (FAIL) is the first observable execution of GA10B silicon under
// UnaOS, and it needs NO correct image: the PASS predicate is "the ROM reached a verdict", not "the ROM
// accepted our payload". A FAIL verdict is this rung's success.
//
// 4a (free, RETURNS): inside rung 3's proven power+clock bracket, re-prove the two rung-4 inputs THIS
// boot (`bcr_dmacfg` lock bit == 0, `bcr_ctrl` readable), seat the rung's OWN 2 MiB Normal-NC DMA window
// (never the NIC's), fill it with a non-signature pattern, then write the six BCR DMA address registers
// and `bcr_dmacfg` WITHOUT the lock bit, reading each back, stop at the first mismatch, restore all seven
// to zero and verify. `bcr_ctrl` is NOT written — that bit is 4b's, and withholding it is what makes 4a
// free. Two 4a arms spend the power cycle anyway (BCR-SELFLOCKED, BCR-STICKY) and end in SYSTEM_OFF.
// 4b (ENDS THE MACHINE): only under `=2` AND a same-boot BCR-ALLHELD. Re-write the addresses, set
// `bcr_dmacfg` = noncoherent | lock_locked (spends the cycle), `bcr_ctrl` = 0x111 (SEQ brom_config), then
// the ignition `priscv_cpuctl` = startcpu; poll `br_retcode` (bounded, every sample printed); read the
// post-ignition state block; SYSTEM_OFF on every reachable path.
//
// Announce discipline inherited verbatim from 3b: `about-to-WRITE ` (upper) before every write,
// `about-to-read ` (lower) before every read — case is load-bearing for the scorer (brief §4 row C).
// Every value is printed `{:#010x}` so no value is a prefix of another (§4 row A). Summary arms are
// chosen so none is a prefix or substring of another (§4 row B).

// facts (b): bcr_dmacfg target_noncoherent_system = 0x2 (lock_locked = 0x80000000 is BCR_DMACFG_LOCK_LOCKED above).
#[cfg(feature = "ga10bprobe4a")] const BCR_DMACFG_TARGET_NONCOHERENT_SYSTEM: u32 = 0x2;
/// Sub-offsets INSIDE the rung's own 2 MiB window for the fmcdata and pkcparam "images". Ours to choose —
/// nothing on the die constrains them — and choosing them inside one owned window keeps every address the
/// ROM could fetch inside memory this kernel controls (brief §3.2).
#[cfg(feature = "ga10bprobe4a")] const DMABUF_FMCDATA_OFF: u64 = 512 * 1024;
#[cfg(feature = "ga10bprobe4a")] const DMABUF_PKC_OFF: u64 = 1024 * 1024;
/// The non-signature fill pattern: neither all-zero nor all-ones, not a plausible header.
#[cfg(feature = "ga10bprobe4a")] const DMABUF_PATTERN: u32 = 0x4A10_B4A5;
// facts (b) SEQ step 1: brom_config bcr_ctrl = 0x111 (rung 3 read the baseline as 0x110 — the delta is bit 0).
#[cfg(all(feature = "ga10bprobe4b", not(feature = "ga10bprobe4f")))] const BCR_CTRL_BROM_CONFIG: u32 = 0x111; #[cfg(feature = "ga10bprobe4f")] const BCR_CTRL_BROM_CONFIG: u32 = 0x011; // RUNG 4f folds its value onto THIS line (line-neutral: =1/=2/=3/=4 keep 0x111 and their bytes). 0x011 is the ACKED SEQ's ALTERNATE set_bcr value — BRFETCH FALSE, CORE_SELECT RISCV, VALID TRUE — decoded exactly in GA10B-RUNG5-BRIEF.md §2.2 against the MIT GA102 dev_riscv_pri.h field map.
// facts (b): priscv cpuctl 0x388 startcpu_true = 0x1 — THE IGNITION.
#[cfg(feature = "ga10bprobe4b")] const PRISCV_CPUCTL_STARTCPU: u32 = 0x1;
/// Bounded br_retcode poll: N samples, fixed settle between them, every sample printed with its index.
#[cfg(feature = "ga10bprobe4b")] const BR_POLL_SAMPLES: u32 = 16;
#[cfg(feature = "ga10bprobe4b")] const BR_POLL_SETTLE_MS: u64 = 10;

/// Rung 4a's OWN write helper (the BCR census writes). `#[cfg(feature = "ga10bprobe4a")]`, so no
/// configuration without rung 4 compiles a BCR write path; rung 3b's `w32` stays 3b's.
#[cfg(feature = "ga10bprobe4a")]
#[inline]
fn w32_4(pa: u64, v: u32) {
    unsafe { core::ptr::write_volatile(pa as *mut u32, v) }
}

/// Rung 4b's OWN write helper — the lock, the trigger and the ignition. `#[cfg(feature = "ga10bprobe4b")]`:
/// rung 4a alone compiles NO path that can set the lock bit, write `bcr_ctrl`, or start the core.
#[cfg(feature = "ga10bprobe4b")]
#[inline]
fn ignite_w32(pa: u64, v: u32) {
    unsafe { core::ptr::write_volatile(pa as *mut u32, v) }
}

/// End a rung-4 flight the cold-boot way (PSCI SYSTEM_OFF via `power::shutdown`). Never returns.
#[cfg(feature = "ga10bprobe4a")]
fn finish4(fam: &str) -> ! {
    serial_println!("[{}] flight done — powering OFF; the dark board is the ready-for-cold-boot signal", fam);
    crate::power::shutdown()
}

/// `Some(reason)` when a readback is not a value: all-ones or the PRI fabric's 0xBADxxxxx pattern.
/// Such a readback is reported `-UNREADABLE reason=…` and NEVER folded into held or not-held (brief F4).
#[cfg(feature = "ga10bprobe4a")]
fn unreadable_reason(v: u32) -> Option<&'static str> {
    if v == 0xFFFF_FFFF {
        Some("all-ones")
    } else if v & PRI_ERROR_MASK == PRI_ERROR_PATTERN {
        Some("pri-error")
    } else {
        None
    }
}

/// The six BCR DMA address registers, in the brief's A1..A6 order (facts (b) BCR DMA addrs).
#[cfg(feature = "ga10bprobe4a")]
const BCR_ADDR_REGS: [(&str, u64); 6] = [
    ("priscv_bcr_fmccode_lo", PRISCV_BCR_FMCCODE_LO_OFF),
    ("priscv_bcr_fmccode_hi", PRISCV_BCR_FMCCODE_HI_OFF),
    ("priscv_bcr_fmcdata_lo", PRISCV_BCR_FMCDATA_LO_OFF),
    ("priscv_bcr_fmcdata_hi", PRISCV_BCR_FMCDATA_HI_OFF),
    ("priscv_bcr_pkcparam_lo", PRISCV_BCR_PKCPARAM_LO_OFF),
    ("priscv_bcr_pkcparam_hi", PRISCV_BCR_PKCPARAM_HI_OFF),
];

/// The six address VALUES for a window at `pa`: fmccode at +0, fmcdata at +DMABUF_FMCDATA_OFF, pkcparam
/// at +DMABUF_PKC_OFF, lo/hi halves in BCR_ADDR_REGS order, >> `R4_ADDR_SHIFT` (0 flown, 8 on rung 4e).
#[cfg(feature = "ga10bprobe4a")]
fn bcr_addr_values(pa: u64) -> [u32; 6] {
    let fc = pa >> R4_ADDR_SHIFT;
    let fd = (pa + DMABUF_FMCDATA_OFF) >> R4_ADDR_SHIFT;
    let pk = (pa + DMABUF_PKC_OFF) >> R4_ADDR_SHIFT;
    [fc as u32, (fc >> 32) as u32, fd as u32, (fd >> 32) as u32, pk as u32, (pk >> 32) as u32]
}

/// One announced write + immediate readback of a BCR register at falcon2-relative `off`. Returns
/// `Ok(held)` (readback == value) or `Err(reason)` when the readback was not a value at all. Prints the
/// announce line, then exactly one result line — so `write_announces == write_results` (brief §4 row C).
#[cfg(feature = "ga10bprobe4a")]
fn bcr_write_verify(fam: &str, name: &str, f2: u64, off: u64, val: u32, why: &str) -> Result<bool, &'static str> {
    let addr = f2 + off;
    serial_println!("[{}] about-to-WRITE {} reg={:#x} val={:#010x} ({}) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", fam, name, addr, val, why);
    w32_4(addr, val);
    let got = r32(addr);
    match unreadable_reason(got) {
        Some(r) => {
            serial_println!("[{}] {} @{:#x} wrote={:#010x} read=-UNREADABLE reason={} val={:#010x} — the register stopped answering after the write; NOT folded into held or not-held (F4)", fam, name, off, val, r, got);
            Err(r)
        }
        None => {
            let held = got == val;
            serial_println!("[{}] {} @{:#x} wrote={:#010x} read={:#010x} held={}", fam, name, off, val, got, held as u32);
            Ok(held)
        }
    }
}

/// GA10B-PROBE4 — the ENTRY the post-heap-init line in `tegra_early_stop` calls. Under `ga10bprobe4d`
/// (`UNAOS_GA10B_PROBE4=4`) it does NOT run the rung: it stashes the DTB coordinates, arms the deferred
/// flag and RETURNS so the desktop comes up; `ga10bprobe4_deferred_run` (called by `power::psci_call` on a
/// SYSTEM_OFF request) runs the rung later. Every other configuration runs `ga10bprobe4_body` at once.
#[cfg(feature = "ga10bprobe4a")]
#[allow(unreachable_code)]
pub fn ga10bprobe4_run(
    chan: &super::bpmp_tegra::Chan,
    dtb_addr: u64,
    dtb_size: usize,
    ram_gib_mask: u64,
) {
    #[cfg(feature = "ga10bprobe4d")] { let _ = chan; arm_deferred(dtb_addr, dtb_size, ram_gib_mask); return; }
    ga10bprobe4_body(chan, dtb_addr, dtb_size, ram_gib_mask)
}

/// GA10B-PROBE4 — rung 4a and (under `ga10bprobe4b`) rung 4b, and (under `ga10bprobe4c`) rung 4c's two
/// census passes around 4b. Runs from `tegra_early_stop`'s post-heap-init line (or, under `ga10bprobe4d`,
/// from the shutdown path) on a channel re-derived from the DTB geometry. RETURNS on every 4a path except
/// BCR-SELFLOCKED / BCR-STICKY; every 4b path ends in SYSTEM_OFF.
#[cfg(feature = "ga10bprobe4a")]
fn ga10bprobe4_body(
    chan: &super::bpmp_tegra::Chan,
    dtb_addr: u64,
    dtb_size: usize,
    ram_gib_mask: u64,
) {
    const FAM: &str = "ga10bprobe4a";
    serial_println!(
        "[ga10bprobe4a] rung 4a (BCR WRITABILITY CENSUS inside rung 3's PROVEN power+clock bracket, re-proven THIS boot; no ignition; symmetric restore; RETURNS) — six BCR DMA address writes + one dmacfg write WITHOUT the lock bit, each announced and read back, stop at first mismatch, restore all seven to zero. bcr_ctrl is NOT written by 4a. Summary vocabulary: BCR-ALLHELD | BCR-SOMEHELD | BCR-NONEHELD | BCR-SELFLOCKED | BCR-STICKY | REFUSED reason=<no-gpu-node|no-power-domains|pg-timeout|pg-on-refused|pg-readback-not-on|bcr-locked|bcr-dmacfg-unreadable|bcr-ctrl-unreadable|no-dma-window>. SELFLOCKED and STICKY spend the power cycle and end in SYSTEM_OFF."
    ); #[cfg(feature = "ga10bprobe4e")] r4e_banner(); #[cfg(feature = "ga10bprobe4f")] r4f_banner(); // RUNGS 4e/4f announce their ONE delta HERE, folded onto this line so the flown arms keep their bytes.
    #[cfg(feature = "ga10bprobe4b")]
    serial_println!(
        "[ga10bprobe4b] rung 4b ARMED (UNAOS_GA10B_PROBE4=2) — after a same-boot BCR-ALLHELD this boot performs the blob-free boot-ROM IGNITION: addresses re-written -> bcr_dmacfg = noncoherent|lock_locked (SPENDS THE POWER CYCLE) -> bcr_ctrl = 0x111 -> priscv_cpuctl = startcpu -> bounded br_retcode poll -> post-ignition state block -> SYSTEM_OFF on EVERY path. The rung's PASS is a FAIL verdict (br_result=0x2): the ROM executed and rejected an unsigned pattern. F2 warning: a GPU-side fabric RAS after the ignition may need a manual power cut."
    );
    #[cfg(feature = "ga10bprobe4c")]
    serial_println!(
        "[ga10bprobe4c] rung 4c ARMED (UNAOS_GA10B_PROBE4=3 immediate, =4 deferred to the shutdown path) — READ-ONLY census, ZERO new write classes: 30 registers PRE-ignition (after 4a's restore, before 4b: rung 3's 25 + top_num_gpcs, gsp_falcon_hwcfg2, gsp_falcon_mailbox0, priscv_cpuctl, priscv_br_retcode), then after 4b's verdict a 20-sample br_retcode SERIES, the same 30 POST-ignition diffed register-by-register against PRE (the 8 BCR registers against what 4b WROTE), and a CPU read of the DMA window; then 4b's SYSTEM_OFF. Vocabulary: PRECENSUS-DONE | PRECENSUS-SKIPPED ; BRSERIES-STABLE | BRSERIES-CHANGED | BRSERIES-UNREADABLE ; POSTBCR-INTACT | POSTBCR-ALTERED | POSTBCR-UNREADABLE ; MAPDIFF-SAME | MAPDIFF-CHANGED ; DMABUF-UNTOUCHED | DMABUF-ALTERED ; CENSUS-COMPLETE"
    );

    // P0 — APERTURE + DOMAIN + CLOCKS: pure DTB RAM walk, zero MMIO.
    let Some(gpu) = resolve_gpu_node(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa=0x00000000 lock_after=0 -> REFUSED reason=no-gpu-node — the firmware DTB carries no usable gpu@ node; nothing driven, nothing read; RETURNING");
        return;
    };
    serial_println!(
        "[ga10bprobe4a] gpu@ node: BAR0={:#x} (DTB reg[0], EXT) power-domain-id={} (DTB power-domains, EXT) clocks={} (DTB clocks, EXT): {} {} {} {} {} {} {} {}",
        gpu.bar0,
        match gpu.pd_id { Some(id) => id as i64, None => -1 },
        gpu.n_clocks,
        gpu.clocks[0], gpu.clocks[1], gpu.clocks[2], gpu.clocks[3],
        gpu.clocks[4], gpu.clocks[5], gpu.clocks[6], gpu.clocks[7],
    );
    let Some(pd_id) = gpu.pd_id else {
        serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa=0x00000000 lock_after=0 -> REFUSED reason=no-power-domains — gpu@ lists no power-domains id; a gated access is EL3-fatal (JX1); RETURNING");
        return;
    };

    // P0 — POWER: rung 3's bracket, unchanged in shape: pre-state, drive only if off, explicit readback.
    serial_println!("[ga10bprobe4a] BPMP MRQ_PG GET_STATE (read-only) id={} — the pre-state, before anything is driven", pd_id);
    let pg_before = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe4a] pg-before id={} err={} state={:#x}", pd_id, err, st);
            if err == 0 { Some(st) } else { None }
        }
        None => {
            serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa=0x00000000 lock_after=0 -> REFUSED reason=pg-timeout — MRQ_PG GET_STATE got no frame in 100 ms; nothing driven, nothing read; RETURNING");
            return;
        }
    };
    let mut we_powered = false;
    if pg_before != Some(PG_STATE_ON) {
        serial_println!("[ga10bprobe4a] BPMP MRQ_PG SET_STATE id={} state=ON — a BPMP request, not an MMIO write", pd_id);
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_ON]) {
            Some((err, _)) => {
                serial_println!("[ga10bprobe4a] pg-set-on id={} err={}", pd_id, err);
                we_powered = err == 0;
            }
            None => serial_println!("[ga10bprobe4a] pg-set-on id={} TIMEOUT", pd_id),
        }
        settle_ms(2);
    }
    let pg_now = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe4a] pg-readback id={} err={} state={:#x}", pd_id, err, st);
            if err == 0 { st } else { 0xffff_ffff }
        }
        None => {
            serial_println!("[ga10bprobe4a] pg-readback id={} TIMEOUT", pd_id);
            0xffff_ffff
        }
    };

    // P0 — CLOCKS: the IS_ENABLED / ENABLE / IS_ENABLED census, rung 2's and rung 3's shape.
    let mut enabled_by_us = [false; 8];
    let mut n_on_after = 0usize;
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        let before = match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe4a] clk {} IS_ENABLED (before) err={} = {}", id, err, st);
                if err == 0 { Some(st) } else { None }
            }
            None => {
                serial_println!("[ga10bprobe4a] clk {} IS_ENABLED (before) TIMEOUT", id);
                None
            }
        };
        if before == Some(0) {
            serial_println!("[ga10bprobe4a] clk {} ENABLE — BPMP request; if this is the LAST line the transaction hung the boot", id);
            match clk(chan, CMD_CLK_ENABLE, id) {
                Some((err, _)) => {
                    serial_println!("[ga10bprobe4a] clk {} ENABLE err={}", id, err);
                    enabled_by_us[i] = err == 0;
                }
                None => serial_println!("[ga10bprobe4a] clk {} ENABLE TIMEOUT", id),
            }
        }
    }
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe4a] clk {} IS_ENABLED (after) err={} = {}", id, err, st);
                if err == 0 && st == 1 { n_on_after += 1; }
            }
            None => serial_println!("[ga10bprobe4a] clk {} IS_ENABLED (after) TIMEOUT", id),
        }
    }
    serial_println!("[ga10bprobe4a] clocks: {} of {} running after this rung's enables (236 answering err=-22 is the rung-3 datum, expected)", n_on_after, gpu.n_clocks);
    settle_ms(2);

    // The census body. `Some(true)` = 4b may arm (BCR-ALLHELD this boot); `Some(false)` = 4a finished
    // without ALLHELD; the SELFLOCKED/STICKY arms never return from inside.
    let base = gpu.bar0;
    let f2 = base + GSP_FALCON2_BASE;
    let mut allheld = false;
    let mut dmabuf_pa: u64 = 0;
    let mut ctrl_baseline: u32 = 0;
    if pg_now == PG_STATE_ON {
        'census: {
            // P1 — the lock bit, re-read THIS boot.
            let a = f2 + PRISCV_BCR_DMACFG_OFF;
            serial_println!("[ga10bprobe4a] about-to-read priscv_bcr_dmacfg reg={:#x} (P1: lock_locked bit31 must be 0) — if this is the LAST line, THAT read was EL3-fatal", a);
            let v = r32(a);
            if let Some(r) = unreadable_reason(v) {
                serial_println!("[ga10bprobe4a] priscv_bcr_dmacfg @{:#x} = -UNREADABLE reason={} val={:#010x}", PRISCV_BCR_DMACFG_OFF, r, v);
                serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa=0x00000000 lock_after=0 -> REFUSED reason=bcr-dmacfg-unreadable — zero writes");
                break 'census;
            }
            serial_println!("[ga10bprobe4a] priscv_bcr_dmacfg @{:#x} = {:#010x} lock_locked={}", PRISCV_BCR_DMACFG_OFF, v, (v & BCR_DMACFG_LOCK_LOCKED != 0) as u32);
            if v & BCR_DMACFG_LOCK_LOCKED != 0 {
                serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa=0x00000000 lock_after=1 -> REFUSED reason=bcr-locked — the BCR is spent for this power cycle; zero writes; the operator's next boot must be COLD");
                break 'census;
            }
            // P2 — bcr_ctrl baseline.
            let a = f2 + PRISCV_BCR_CTRL_OFF;
            serial_println!("[ga10bprobe4a] about-to-read priscv_bcr_ctrl reg={:#x} (P2: baseline; rung 3 read 0x00000110) — if this is the LAST line, THAT read was EL3-fatal", a);
            let v = r32(a);
            if let Some(r) = unreadable_reason(v) {
                serial_println!("[ga10bprobe4a] priscv_bcr_ctrl @{:#x} = -UNREADABLE reason={} val={:#010x}", PRISCV_BCR_CTRL_OFF, r, v);
                serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa=0x00000000 lock_after=0 -> REFUSED reason=bcr-ctrl-unreadable — zero writes");
                break 'census;
            }
            ctrl_baseline = v;
            serial_println!("[ga10bprobe4a] bcr_ctrl_before={:#010x} bcr_ctrl_baseline_changed={} (a value other than 0x00000110 is a datum, not a stop)", v, (v != 0x0000_0110) as u32);
            // P3 — the rung's OWN DMA window, seated at heap-guard by the NET4A law, mapped Normal-NC now.
            let (wb, ws) = super::mmu_tegra::ga10b4_nc_window();
            if wb == 0 || ws == 0 {
                serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa=0x00000000 lock_after=0 -> REFUSED reason=no-dma-window — no second clean L2-split 2 MiB block was seated below 4 GiB (see the [ga10b4nc] census at heap-guard); zero writes");
                break 'census;
            }
            if wb + ws > 0x1_0000_0000 || !super::mmu_tegra::install_nc_window(wb, ws) {
                serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa={:#010x} lock_after=0 -> REFUSED reason=no-dma-window — the seated block could not be mapped Normal-NC (or is not below 4 GiB); zero writes", wb);
                break 'census;
            }
            dmabuf_pa = wb;
            serial_println!("[ga10bprobe4a] dmabuf_pa={:#010x} dmabuf_size={:#x} (this kernel's OWN block, Normal-NC, below 4 GiB, clear of every carveout the heap dodges and of the NIC's window) fmcdata_off={:#x} pkc_off={:#x}", wb, ws, DMABUF_FMCDATA_OFF, DMABUF_PKC_OFF);
            // P4 — fill with the non-signature pattern; NC mapping means no cache to clean, dsb orders it.
            {
                let mut off = 0u64;
                while off < ws {
                    unsafe { core::ptr::write_volatile((wb + off) as *mut u32, DMABUF_PATTERN) };
                    off += 4;
                }
                unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
            }
            serial_println!("[ga10bprobe4a] dmabuf_pattern={:#010x} filled {} KiB, dsb sy", DMABUF_PATTERN, ws >> 10);

            // A1..A6 — the six address writes, stop at the first mismatch or unreadable.
            let vals = bcr_addr_values(wb);
            let mut held = 0u32;
            let mut n_unreadable = 0u32;
            let mut written = [false; 7];
            let mut stopped = false;
            for i in 0..6 {
                let (name, off) = BCR_ADDR_REGS[i];
                written[i] = true;
                #[cfg(feature = "ga10bprobe4e")] match bcr_write_verify_shift(FAM, name, f2, off, vals[i], bcr_addr_raw(wb, i), "BCR DMA address, A-step") { Ok(true) => held += 1, Ok(false) => { stopped = true; } Err(_) => { n_unreadable += 1; stopped = true; } } #[cfg(not(feature = "ga10bprobe4e"))] match bcr_write_verify(FAM, name, f2, off, vals[i], "BCR DMA address, A-step") {
                    Ok(true) => held += 1,
                    Ok(false) => { stopped = true; }
                    Err(_) => { n_unreadable += 1; stopped = true; }
                }
                if stopped {
                    serial_println!("[ga10bprobe4a] write list STOPPED at {} — the remaining address writes are not attempted (F3/F4)", name);
                    break;
                }
            }
            // A7 — dmacfg WITHOUT the lock bit.
            let mut selflocked = false;
            if !stopped {
                written[6] = true;
                match bcr_write_verify(FAM, "priscv_bcr_dmacfg", f2, PRISCV_BCR_DMACFG_OFF, BCR_DMACFG_TARGET_NONCOHERENT_SYSTEM, "target_noncoherent_system ONLY; the lock_locked bit is DELIBERATELY NOT SET") {
                    Ok(true) => held += 1,
                    Ok(false) => {
                        let got = r32(f2 + PRISCV_BCR_DMACFG_OFF);
                        if got & BCR_DMACFG_LOCK_LOCKED != 0 {
                            selflocked = true;
                            serial_println!("[ga10bprobe4a] dmacfg readback {:#010x} has lock_locked SET without being asked (F6)", got);
                        }
                    }
                    Err(_) => { n_unreadable += 1; }
                }
            }
            // A8 — restore: every register this rung wrote back to zero, verified.
            let mut sticky: Option<&str> = None;
            for i in 0..7 {
                if !written[i] { continue; }
                let (name, off) = if i < 6 { BCR_ADDR_REGS[i] } else { ("priscv_bcr_dmacfg", PRISCV_BCR_DMACFG_OFF) };
                match bcr_write_verify(FAM, name, f2, off, 0, "restore to zero, as found") {
                    Ok(true) => {}
                    Ok(false) => { if sticky.is_none() { sticky = Some(name); } }
                    Err(_) => { n_unreadable += 1; }
                }
            }
            let a = f2 + PRISCV_BCR_DMACFG_OFF;
            serial_println!("[ga10bprobe4a] about-to-read priscv_bcr_dmacfg reg={:#x} (lock_after) — if this is the LAST line, THAT read was EL3-fatal", a);
            let after = r32(a);
            let lock_after = (unreadable_reason(after).is_none() && after & BCR_DMACFG_LOCK_LOCKED != 0) as u32;
            let arm = if selflocked || lock_after == 1 {
                "BCR-SELFLOCKED"
            } else if let Some(_) = sticky {
                "BCR-STICKY"
            } else if held == 7 {
                "BCR-ALLHELD"
            } else if held == 0 {
                "BCR-NONEHELD"
            } else {
                "BCR-SOMEHELD"
            };
            #[cfg(not(any(feature = "ga10bprobe4e", feature = "ga10bprobe4f")))] serial_println!("[ga10bprobe4a] bcrheld={}/7 dmabuf_pa={:#010x} lock_after={} unreadable={} -> {}{}", held, wb, lock_after, n_unreadable, arm, match sticky { Some(n) => { let _ = n; " (a register did not clear to zero — see its restore line; the board is NOT left as found)" } None => "" }); #[cfg(feature = "ga10bprobe4e")] serial_println!("[ga10bprobe4a] bcrheld={}/7 dmabuf_pa={:#010x} shift=8 lock_after={} unreadable={} -> {}{}", held, wb, lock_after, n_unreadable, arm, match sticky { Some(n) => { let _ = n; " (a register did not clear to zero — see its restore line; the board is NOT left as found)" } None => "" }); #[cfg(feature = "ga10bprobe4f")] serial_println!("[ga10bprobe4a] bcrheld={}/7 dmabuf_pa={:#010x} brfetch=false lock_after={} unreadable={} -> {}{}", held, wb, lock_after, n_unreadable, arm, match sticky { Some(n) => { let _ = n; " (a register did not clear to zero — see its restore line; the board is NOT left as found)" } None => "" });
            if arm == "BCR-SELFLOCKED" {
                serial_println!("[ga10bprobe4a] the lock latched on a config write that did not ask for it: 4a is NOT free on this die — the power cycle is spent and the next boot must be COLD (F6)");
                finish4(FAM);
            }
            if arm == "BCR-STICKY" {
                serial_println!("[ga10bprobe4a] the restore did not hold — never read 'restored' over this; the next boot must be COLD (F7)");
                finish4(FAM);
            }
            if arm == "BCR-NONEHELD" {
                serial_println!("[ga10bprobe4a] the gate is NAMED: priv-lockdown gates BCR writes from the CCPLEX while MAILBOX0 (rung 3b) accepts them — a complete, publishable answer; rung 4 ends here");
            }
            allheld = arm == "BCR-ALLHELD";
        }
        // RUNG 4c, pass 1 — the PRE-ignition census, on the rail the readback just proved ON, after 4a's
        // restore (so the BCR reads as rung 3 found it) and before 4b can spend anything.
        #[cfg(feature = "ga10bprobe4c")]
        rung4c_pre(base);
    } else {
        serial_println!("[ga10bprobe4a] bcrheld=0/7 dmabuf_pa=0x00000000 lock_after=0 -> REFUSED reason={} — the explicit readback did not say ON; a gated access is EL3-fatal (JX1): NOT ONE BAR0 register was touched", if we_powered { "pg-readback-not-on" } else { "pg-on-refused" });
        #[cfg(feature = "ga10bprobe4c")]
        serial_println!("[ga10bprobe4c] pre-ignition census: readable=0/{} unreadable=0 -> PRECENSUS-SKIPPED reason=pg-not-on — the rail was not proven ON, so not one census register was read", R4C_N);
    }

    // 4b — the ignition. Only under its own knob; it decides on the same-boot ALLHELD and never returns.
    let _ = (dmabuf_pa, allheld, ctrl_baseline);
    #[cfg(feature = "ga10bprobe4b")]
    rung4b(base, f2, dmabuf_pa, allheld, ctrl_baseline);

    // SYMMETRIC RESTORE of the bracket — rung 3's, verbatim in shape.
    let mut n_disabled = 0usize;
    for i in (0..gpu.n_clocks).rev() {
        if enabled_by_us[i] {
            let id = gpu.clocks[i];
            match clk(chan, CMD_CLK_DISABLE, id) {
                Some((err, _)) => {
                    serial_println!("[ga10bprobe4a] clk {} DISABLE (restore) err={}", id, err);
                    if err == 0 { n_disabled += 1; }
                }
                None => serial_println!("[ga10bprobe4a] clk {} DISABLE (restore) TIMEOUT", id),
            }
        }
    }
    let mut pg_final = pg_now;
    if we_powered {
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_OFF]) {
            Some((err, _)) => serial_println!("[ga10bprobe4a] pg-set-off (restore) id={} err={}", pd_id, err),
            None => serial_println!("[ga10bprobe4a] pg-set-off (restore) id={} TIMEOUT", pd_id),
        }
        match pg_state(chan, pd_id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe4a] pg-final id={} err={} state={:#x}", pd_id, err, st);
                pg_final = st;
            }
            None => serial_println!("[ga10bprobe4a] pg-final id={} TIMEOUT", pd_id),
        }
    }
    serial_println!(
        "[ga10bprobe4a] restored: pg={:#x} (was {}) clocks-disabled={} of {} enabled here — BCR registers verified back to zero above",
        pg_final,
        match pg_before { Some(s) => s as i64, None => -1 },
        n_disabled,
        enabled_by_us.iter().filter(|b| **b).count(),
    );
    serial_println!("[ga10bprobe4a] rung 4a complete — RETURNING to the boot (no SYSTEM_OFF; the flight is a full boot)");
}

/// RUNG 4b — the blob-free boot-ROM ignition. Called from `ga10bprobe4_run` only under `ga10bprobe4b`;
/// arms ONLY on a same-boot BCR-ALLHELD; ENDS THE MACHINE in SYSTEM_OFF on every reachable path (the
/// lock bit it sets makes the BCR final for this power cycle — facts §(b) — so the flight cannot repeat
/// warm, and a dark board is the bench's "ready for cold boot" signal).
#[cfg(feature = "ga10bprobe4b")]
fn rung4b(base: u64, f2: u64, dmabuf_pa: u64, allheld: bool, ctrl_baseline: u32) {
    const FAM: &str = "ga10bprobe4b";
    let n = BR_POLL_SAMPLES;
    serial_println!("[ga10bprobe4b] rung 4b — the IGNITION. Verdict vocabulary: BROM-VERDICT-FAIL (the rung's PASS) | BROM-VERDICT-PASS | BROM-NOVERDICT | BCR-CTRL-REFUSED | IGNITION-SKIPPED reason=<bcr-not-allheld|bcr-addr-refused>");
    if !allheld {
        serial_println!("[ga10bprobe4b] br_retcode=0x00000000 br_result=0x0 samples=0/{} lock_latched=0 post_lockdown=0 v1_readable=0 -> IGNITION-SKIPPED reason=bcr-not-allheld — 4a did not read BCR-ALLHELD this boot, so no lock, no trigger and no ignition were written; nothing was spent", n);
        finish4(FAM);
    }
    // B0 — the addresses, re-written (4a restored them to zero).
    let vals = bcr_addr_values(dmabuf_pa);
    for i in 0..6 {
        let (name, off) = BCR_ADDR_REGS[i];
        #[cfg(not(feature = "ga10bprobe4e"))] let ok = matches!(bcr_write_verify(FAM, name, f2, off, vals[i], "BCR DMA address, B0 re-write"), Ok(true)); #[cfg(feature = "ga10bprobe4e")] let ok = matches!(bcr_write_verify_shift(FAM, name, f2, off, vals[i], bcr_addr_raw(dmabuf_pa, i), "BCR DMA address, B0 re-write"), Ok(true));
        if !ok {
            serial_println!("[ga10bprobe4b] B0 mismatch at {} — restoring the addresses to zero and skipping the ignition", name);
            for j in 0..=i {
                let (nm, of) = BCR_ADDR_REGS[j];
                let _ = bcr_write_verify(FAM, nm, f2, of, 0, "restore to zero after B0 mismatch");
            }
            serial_println!("[ga10bprobe4b] br_retcode=0x00000000 br_result=0x0 samples=0/{} lock_latched=0 post_lockdown=0 v1_readable=0 -> IGNITION-SKIPPED reason=bcr-addr-refused", n);
            finish4(FAM);
        }
    }
    // B1 — dmacfg WITH the lock. THIS SPENDS THE POWER CYCLE.
    let a = f2 + PRISCV_BCR_DMACFG_OFF;
    let lockval = BCR_DMACFG_TARGET_NONCOHERENT_SYSTEM | BCR_DMACFG_LOCK_LOCKED;
    serial_println!("[ga10bprobe4b] about-to-WRITE priscv_bcr_dmacfg reg={:#x} val={:#010x} (target_noncoherent_system | lock_locked — THIS SPENDS THE POWER CYCLE: the BCR cannot be reprogrammed again until a cold boot) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", a, lockval);
    ignite_w32(a, lockval);
    let got = r32(a);
    let lock_latched = (unreadable_reason(got).is_none() && got & BCR_DMACFG_LOCK_LOCKED != 0) as u32;
    serial_println!("[ga10bprobe4b] priscv_bcr_dmacfg @{:#x} wrote={:#010x} read={:#010x} lock_latched={} ({})", PRISCV_BCR_DMACFG_OFF, lockval, got, lock_latched, if lock_latched == 1 { "the lock took" } else { "the lock did NOT latch — a datum; the SEQ asks for it, whether the ROM requires it is UNKNOWN; continuing" });
    // B2 — bcr_ctrl = brom_config.
    let a = f2 + PRISCV_BCR_CTRL_OFF;
    #[cfg(not(feature = "ga10bprobe4f"))] serial_println!("[ga10bprobe4b] about-to-WRITE priscv_bcr_ctrl reg={:#x} val={:#010x} (the ACKED SEQ brom_config value; baseline was {:#010x}) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", a, BCR_CTRL_BROM_CONFIG, ctrl_baseline); #[cfg(feature = "ga10bprobe4f")] serial_println!("[ga10bprobe4b] about-to-WRITE priscv_bcr_ctrl reg={:#x} val={:#010x} brfetch=false (RUNG 4f: the ACKED SEQ's ALTERNATE set_bcr value — BRFETCH FALSE, CORE_SELECT RISCV, VALID TRUE. The flown arm wrote 0x00000111, BRFETCH TRUE; baseline was {:#010x}) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", a, BCR_CTRL_BROM_CONFIG, ctrl_baseline);
    ignite_w32(a, BCR_CTRL_BROM_CONFIG);
    let ctrl = r32(a);
    serial_println!("[ga10bprobe4b] priscv_bcr_ctrl @{:#x} wrote={:#010x} read={:#010x} held={}", PRISCV_BCR_CTRL_OFF, BCR_CTRL_BROM_CONFIG, ctrl, (ctrl == BCR_CTRL_BROM_CONFIG) as u32);
    if ctrl != BCR_CTRL_BROM_CONFIG {
        let (halted, lockdown, v1r) = post_ignition_block(base, f2);
        serial_println!("[ga10bprobe4b] br_retcode=0x00000000 br_result=0x0 samples=0/{} lock_latched={} post_lockdown={} v1_readable={} -> BCR-CTRL-REFUSED read={:#010x} — the trigger register did not take brom_config; the ignition write was NOT issued (halted={})", n, lock_latched, lockdown, v1r, ctrl, halted);
        finish4(FAM);
    }
    // B3 — step 3 of the SEQ (riscv_boot_vector lo/hi) is DELIBERATELY OMITTED.
    serial_println!("[ga10bprobe4b] SEQ step 3 (priscv_boot_vector lo/hi @0x111380/0x111384) DELIBERATELY OMITTED — both are in rung 3's 9-UNREADABLE set (0xbadf5620): a write there could not be read back, an unverifiable mutation this ladder forbids; the SEQ marks the step optional and we take the option");
    // B4 — THE IGNITION.
    let a = f2 + PRISCV_CPUCTL_OFF;
    serial_println!("[ga10bprobe4b] about-to-WRITE priscv_cpuctl reg={:#x} val={:#010x} (startcpu_true) — THE IGNITION. if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", a, PRISCV_CPUCTL_STARTCPU);
    ignite_w32(a, PRISCV_CPUCTL_STARTCPU);
    // B5 — the bounded poll, every sample printed with its index.
    let rc = f2 + PRISCV_BR_RETCODE_OFF;
    let mut retcode: u32 = 0;
    let mut samples: u32 = 0;
    for i in 1..=n {
        samples = i;
        serial_println!("[ga10bprobe4b] about-to-read priscv_br_retcode reg={:#x} sample={}/{} — if this is the LAST line, THAT read was EL3-fatal", rc, i, n);
        retcode = r32(rc);
        serial_println!("[ga10bprobe4b] br_retcode={:#010x} br_result={:#x} sample={}/{}", retcode, retcode & 0x3, i, n);
        if unreadable_reason(retcode).is_none() && (retcode & 0x3 == BR_RETCODE_FAIL || retcode & 0x3 == BR_RETCODE_PASS) {
            break;
        }
        settle_ms(BR_POLL_SETTLE_MS);
    }
    // B6 — the post-state block, each line phase-tagged `post-ignition ` (§4 row D).
    let (halted, lockdown, v1r) = post_ignition_block(base, f2);
    // B7 — the summary and the end of the machine.
    let result = if unreadable_reason(retcode).is_some() { 0xf } else { retcode & 0x3 };
    let arm = if result == BR_RETCODE_FAIL {
        "BROM-VERDICT-FAIL"
    } else if result == BR_RETCODE_PASS {
        "BROM-VERDICT-PASS"
    } else {
        "BROM-NOVERDICT"
    };
    #[cfg(not(any(feature = "ga10bprobe4e", feature = "ga10bprobe4f")))] serial_println!("[ga10bprobe4b] br_retcode={:#010x} br_result={:#x} samples={}/{} lock_latched={} post_lockdown={} v1_readable={} -> {}", retcode, result, samples, n, lock_latched, lockdown, v1r, arm); #[cfg(feature = "ga10bprobe4e")] serial_println!("[ga10bprobe4b] br_retcode={:#010x} br_result={:#x} samples={}/{} lock_latched={} post_lockdown={} v1_readable={} shift=8 -> {}", retcode, result, samples, n, lock_latched, lockdown, v1r, arm); #[cfg(feature = "ga10bprobe4f")] serial_println!("[ga10bprobe4b] br_retcode={:#010x} br_result={:#x} samples={}/{} lock_latched={} post_lockdown={} v1_readable={} brfetch=false -> {}", retcode, result, samples, n, lock_latched, lockdown, v1r, arm);
    match arm {
        "BROM-VERDICT-FAIL" => serial_println!("[ga10bprobe4b] THE RUNG'S PASS: the GSP boot ROM executed, read our payload and rejected it — the first execution of GA10B silicon under UnaOS, with no vendor blob. AMBIGUITY, inline by design (brief §2.3): FAIL proves EXECUTION, not the cause — unsigned payload, malformed manifest, NSDRAM-encryption confound (the GPU may have read different bytes than the CPU wrote), or a DMA timeout into FAIL are indistinguishable here; no later rung may read this as a statement about signatures"),
        "BROM-VERDICT-PASS" => serial_println!("[ga10bprobe4b] EXTRAORDINARY: a pattern we authored verified against the vendor key — treat as a MEASUREMENT ERROR until independently re-flown from a cold boot; build nothing on it this session (F8)"),
        _ => serial_println!("[ga10bprobe4b] no verdict in {} samples: post-ignition halted={} — halted=1 means the core never started or halted again (the ignition write was masked, or bcr_ctrl bit0 is not the trigger); halted=0 means it is RUNNING and the poll was short — a running RISC-V core is itself execution (F5)", n, halted),
    }
    // RUNG 4c, pass 2 — the POST-ignition census, the br_retcode series and the DMA-window readback, all
    // read-only, between 4b's verdict and the SYSTEM_OFF it already owes the bench.
    #[cfg(feature = "ga10bprobe4c")]
    rung4c_post(base, f2, dmabuf_pa, lockval);
    finish4(FAM);
}

/// B6 — three reads after the ignition, each tagged `post-ignition ` so the scorer cannot confuse them
/// with rung 3's pre-ignition reads of the same registers. Returns (halted, lockdown, v1_readable).
#[cfg(feature = "ga10bprobe4b")]
fn post_ignition_block(base: u64, f2: u64) -> (u32, u32, u32) {
    let a = f2 + PRISCV_CPUCTL_OFF;
    serial_println!("[ga10bprobe4b] about-to-read post-ignition priscv_cpuctl reg={:#x} — if this is the LAST line, THAT read was EL3-fatal", a);
    let c = r32(a);
    let halted = match unreadable_reason(c) { Some(_) => 1, None => (c >> PRISCV_CPUCTL_HALTED_BIT) & 1 };
    serial_println!("[ga10bprobe4b] post-ignition priscv_cpuctl halted={} (raw={:#010x})", halted, c);
    let a = base + GSP_FALCON_BASE + FALCON_HWCFG2_OFF;
    serial_println!("[ga10bprobe4b] about-to-read post-ignition falcon_hwcfg2 reg={:#x} — if this is the LAST line, THAT read was EL3-fatal", a);
    let h = r32(a);
    let lockdown = match unreadable_reason(h) { Some(_) => 1, None => (h >> HWCFG2_PRIV_LOCKDOWN_BIT) & 1 };
    serial_println!("[ga10bprobe4b] post-ignition hwcfg2 lockdown={} (raw={:#010x}; rung 1 measured bit13=1 engaged — a drop is a datum of the first order)", lockdown, h);
    let a = base + GSP_FALCON_BASE + FALCON_CPUCTL_OFF;
    serial_println!("[ga10bprobe4b] about-to-read post-ignition gsp_falcon_cpuctl_v1 reg={:#x} — if this is the LAST line, THAT read was EL3-fatal", a);
    let v = r32(a);
    let v1r = unreadable_reason(v).is_none() as u32;
    serial_println!("[ga10bprobe4b] post-ignition gsp_falcon_cpuctl_v1 readable={} (raw={:#010x}; three flights read 0xbadf5620 — readable now means GPU-side state changed under our direction, a second witness of execution independent of br_retcode)", v1r, v);
    (halted, lockdown, v1r)
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// GA10B-PROBE4C — RUNG 4c: the POST-IGNITION CENSUS (`ga10bprobe4c`, implies `ga10bprobe4b`; DEFAULT OFF;
// `UNAOS_GA10B_PROBE4=3`), and GA10B-PROBE4D — the DEFERRED arm (`ga10bprobe4d`, implies `ga10bprobe4c`;
// `UNAOS_GA10B_PROBE4=4`). Design: docs/dev/OS/08_VIDEO/GA10B-RUNG4-BRIEF.md §10 (orin 27, 2026-09-12, from
// Peter's "can you add more GPU probes to the next boot" and "the next boot must run the full desktop first
// and the GPU probe last").
//
// 4c adds ZERO write classes: 4b's seven BCR writes, the lock, the trigger and the ignition stay the only
// GA10B writes in the boot. Everything below is `r32` (or a CPU read of this kernel's OWN DRAM window), each
// announced on its own line first. Two passes over ONE register list — 30 registers: rung 3's 25 plus
// top_num_gpcs, gsp_falcon_hwcfg2, gsp_falcon_mailbox0, priscv_cpuctl and priscv_br_retcode; every offset
// is the ACKED facts file's except MAILBOX0, whose pointer is PUBLIC-RECALLED and metal-proven on this die by
// rung 3b (render11 MAILBOX-HELD) — in rung 3's risk order. Pass 1 is PRE-ignition (after 4a's restore,
// before 4b, inside the same bracket); pass 2 is POST-ignition (after 4b's summary, before the SYSTEM_OFF).
// The register-by-register diff between them is the "lockdown drop" oracle generalised to every register
// the ladder has ever read. Around pass 2: a bounded br_retcode SERIES (does the verdict move, do the upper
// bits ever carry a reason code — brief §7 UNKNOWN), the 8 BCR registers checked against what 4b WROTE (did
// the ROM consume or alter its descriptor), and a CPU read of the DMA window (did the ROM write anything
// INTO our buffer). Every summary line ends in `-> ARM` from the vocabulary in the brief §10; no arm is a
// prefix or substring of another or of a 4a/4b arm, and no 4c line carries ` wrote=0x` or `about-to-WRITE `
// (the 4a/4b scorer's write accounting tokens).
//
// 4d: with `=4` the post-heap-init call does NOT run the rung — it stashes (dtb_addr, dtb_size, ram_gib_mask,
// a DTB checksum), arms `DEFERRED_ARMED` and returns, so the desktop comes up and Peter's glass checks run
// first. `power::psci_call` calls `ga10bprobe4_deferred_run()` on EVERY PSCI SYSTEM_OFF request (shell
// `shutdown`/`off`, crystal Shut Down, and a probe's own finish); the flag is CONSUMED on entry, so the rung's
// own finish4 -> shutdown -> psci_call re-entry is a no-op and the OFF proceeds. The deferred run masks DAIF
// on the calling core, re-verifies the DTB checksum, re-derives the BPMP channel with `chan_reopen`, then runs
// 4a -> 4b -> 4c exactly as at boot (the bracket is re-proven from BPMP; nothing is inherited but the DTB
// coordinates) and 4b's finish4 ends the machine. Anything that refuses prints why and RETURNS so the
// SYSTEM_OFF it interrupted proceeds. A SYSTEM_RESET (restart) does NOT trigger it: after 4b's lock the next
// boot must be cold, and a warm reset would hand the next boot a locked BCR.
//
// WITNESS FAMILIES `[ga10bprobe4c]` / `[ga10bprobe4d]` (15 bytes bracketed, over the 8-byte floor).

/// The bounded br_retcode series after 4b's verdict: N samples, fixed settle, every DISTINCT value printed
/// with the sample index and the time it first appeared.
#[cfg(feature = "ga10bprobe4c")] const BR_SERIES_SAMPLES: u32 = 20;
#[cfg(feature = "ga10bprobe4c")] const BR_SERIES_SETTLE_MS: u64 = 10;
/// The census list length (rung 3's 25 + 5).
#[cfg(feature = "ga10bprobe4c")] const R4C_N: usize = 30;
/// Indices into `RUNG4C_REGS` of the eight registers 4b writes (bcr_ctrl .. fmcdata_hi, contiguous) — their
/// post value is judged against what 4b WROTE, never against the pre pass — and of br_retcode, whose change
/// IS 4b's verdict and is therefore excluded from the "unexpected change" count.
#[cfg(feature = "ga10bprobe4c")] const R4C_BCR_FIRST: usize = 15;
#[cfg(feature = "ga10bprobe4c")] const R4C_BCR_LAST: usize = 22;
#[cfg(feature = "ga10bprobe4c")] const R4C_BR_RETCODE: usize = 28;
#[cfg(feature = "ga10bprobe4c")] const R4C_MAILBOX0: usize = 14;

/// One census register: wire name, BAR0-relative offset, address-class label (rung 3's classes).
#[cfg(feature = "ga10bprobe4c")]
struct R4C {
    name: &'static str,
    off: u64,
    class: &'static str,
}

/// The rung-4c census list, IN RUNG 3's RISK ORDER (fuse -> mc -> top -> gsp-falcon-v1 -> gsp-priscv-bcr ->
/// pmu-falcon2 LAST), built from the SAME offset constants rung 3 / rung 1 / rung 3b read on this die, so
/// "same offsets" holds by construction. Facts-file citation per entry (§ = ga10b-probe-rung1.facts.md):
///   fuse_*            §(b) Security-state fuses        mc_*, top_*     §(b) Die-characterization
///   gsp_falcon_* (v1) §(b) Legacy Falcon regs          priscv_*        §(b) RISC-V boot-ROM interface
///   pmu_falcon2_cpuctl §Aperture framing (PMU base) + §(b) priscv cpuctl
///   gsp_falcon_mailbox0  NOT in the facts file: PUBLIC-RECALLED (nouveau nvkm/falcon; open-gpu-kernel-modules
///                        dev_falcon_v4.h; MIT), metal-proven by rung 3b on this die (render11 MAILBOX-HELD).
#[cfg(feature = "ga10bprobe4c")]
const RUNG4C_REGS: [R4C; R4C_N] = [
    R4C { name: "fuse_opt_sec_debug_en", off: FUSE_OPT_SEC_DEBUG_EN, class: "fuse" },
    R4C { name: "fuse_opt_wpr_enabled", off: FUSE_OPT_WPR_ENABLED, class: "fuse" },
    R4C { name: "fuse_opt_vpr_enabled", off: FUSE_OPT_VPR_ENABLED, class: "fuse" },
    R4C { name: "mc_enable", off: MC_ENABLE, class: "mc" },
    R4C { name: "mc_elpg_enable", off: MC_ELPG_ENABLE, class: "mc" },
    R4C { name: "top_device_info_cfg", off: TOP_DEVICE_INFO_CFG, class: "top" },
    R4C { name: "top_num_gpcs", off: TOP_NUM_GPCS, class: "top" },
    R4C { name: "gsp_falcon_hwcfg", off: GSP_FALCON_BASE + FALCON_HWCFG_OFF, class: "gsp-falcon-v1" },
    R4C { name: "gsp_falcon_dmactl", off: GSP_FALCON_BASE + FALCON_DMACTL_OFF, class: "gsp-falcon-v1" },
    R4C { name: "gsp_falcon_idlestate", off: GSP_FALCON_BASE + FALCON_IDLESTATE_OFF, class: "gsp-falcon-v1" },
    R4C { name: "gsp_falcon_irqmask", off: GSP_FALCON_BASE + FALCON_IRQMASK_OFF, class: "gsp-falcon-v1" },
    R4C { name: "gsp_falcon_irqdest", off: GSP_FALCON_BASE + FALCON_IRQDEST_OFF, class: "gsp-falcon-v1" },
    R4C { name: "gsp_falcon_cpuctl_v1", off: GSP_FALCON_BASE + FALCON_CPUCTL_OFF, class: "gsp-falcon-v1" },
    R4C { name: "gsp_falcon_hwcfg2", off: GSP_FALCON_BASE + FALCON_HWCFG2_OFF, class: "gsp-falcon-v1" },
    R4C { name: "gsp_falcon_mailbox0", off: GSP_FALCON_BASE + FALCON_MAILBOX0_OFF, class: "gsp-falcon-v1" },
    R4C { name: "priscv_bcr_ctrl", off: GSP_FALCON2_BASE + PRISCV_BCR_CTRL_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_bcr_dmacfg", off: GSP_FALCON2_BASE + PRISCV_BCR_DMACFG_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_bcr_pkcparam_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_PKCPARAM_LO_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_bcr_pkcparam_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_PKCPARAM_HI_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_bcr_fmccode_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCCODE_LO_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_bcr_fmccode_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCCODE_HI_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_bcr_fmcdata_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCDATA_LO_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_bcr_fmcdata_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCDATA_HI_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_boot_vector_lo", off: GSP_FALCON2_BASE + PRISCV_BOOT_VECTOR_LO_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_boot_vector_hi", off: GSP_FALCON2_BASE + PRISCV_BOOT_VECTOR_HI_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_riscv_irqmask", off: GSP_FALCON2_BASE + PRISCV_RISCV_IRQMASK_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_riscv_irqdest", off: GSP_FALCON2_BASE + PRISCV_RISCV_IRQDEST_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_cpuctl", off: GSP_FALCON2_BASE + PRISCV_CPUCTL_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "priscv_br_retcode", off: GSP_FALCON2_BASE + PRISCV_BR_RETCODE_OFF, class: "gsp-priscv-bcr" },
    R4C { name: "pmu_falcon2_cpuctl", off: PMU_FALCON2_BASE + PRISCV_CPUCTL_OFF, class: "pmu-falcon2" },
];

/// Pass 1's raw readings, kept for pass 2's diff (readability is derived from the raw value, so only the
/// raw word is stored). `PRE_DONE` says pass 1 ran; without it pass 2 prints `diff=no-pre`.
#[cfg(feature = "ga10bprobe4c")]
static PRE_RAW: [core::sync::atomic::AtomicU32; R4C_N] = [const { core::sync::atomic::AtomicU32::new(0) }; R4C_N];
#[cfg(feature = "ga10bprobe4c")]
static PRE_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The rung's own read accounting, printed on the CENSUS-COMPLETE line: every `about-to-read` 4c prints
/// bumps ANN, every result line it prints bumps ANS. The scorer counts the wire independently.
#[cfg(feature = "ga10bprobe4c")]
static ANN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "ga10bprobe4c")]
static ANS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Milliseconds on CNTPCT (CNTFRQ is 31.25 MHz on this SoC; the division is exact enough for a series).
#[cfg(feature = "ga10bprobe4c")]
fn now_ms() -> u64 {
    let freq: u64;
    let now: u64;
    unsafe {
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) now, options(nomem, nostack, preserves_flags));
    }
    now / (freq / 1000).max(1)
}

/// One announced, counted 4c read. Prints the announce (bumps ANN), reads, returns the raw word; the CALLER
/// prints exactly one result line and bumps ANS, so the two counters can be compared on the wire.
#[cfg(feature = "ga10bprobe4c")]
fn read4c(phase: &str, name: &str, addr: u64, note: &str) -> u32 {
    serial_println!("[ga10bprobe4c] about-to-read {} {} reg={:#x}{} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it", phase, name, addr, note);
    ANN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    r32(addr)
}

#[cfg(feature = "ga10bprobe4c")]
fn answered() {
    ANS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// The census pass. `post == false`: read the 30, store them for the diff. `post == true`: read the 30,
/// print each beside its pre value with a `diff=` verdict, judge the 8 BCR registers against `bcr_expect`
/// (what 4b wrote, in RUNG4C_REGS order 15..=22) and print the POSTBCR and MAPDIFF summaries.
#[cfg(feature = "ga10bprobe4c")]
fn census_4c(post: bool, base: u64, bcr_expect: &[u32; 8]) {
    use core::sync::atomic::Ordering;
    let phase = if post { "post-ignition" } else { "pre-ignition" };
    let pre_done = PRE_DONE.load(Ordering::Relaxed);
    let mut cur_class = "";
    let mut n_read = 0u32;
    let mut n_unread = 0u32;
    let mut became_readable = 0u32;
    let mut became_unreadable = 0u32;
    let mut changed = 0u32;
    let mut changed_ex = 0u32;
    let mut bcr_intact = 0u32;
    let mut bcr_altered = 0u32;
    let mut bcr_unread = 0u32;
    for (i, r) in RUNG4C_REGS.iter().enumerate() {
        if r.class != cur_class {
            cur_class = r.class;
            serial_println!("[ga10bprobe4c] {} address class {} (KNOWN — read on this die by rungs 1-3 without fault; BAR0={:#x})", phase, r.class, base);
        }
        let note = if i == R4C_MAILBOX0 { " (class=gsp-falcon-v1; pointer PUBLIC-RECALLED, NOT the facts file — metal-proven by rung 3b MAILBOX-HELD)" } else { "" };
        let v = read4c(phase, r.name, base + r.off, note);
        let unread = unreadable_reason(v);
        if unread.is_some() { n_unread += 1; } else { n_read += 1; }
        if !post {
            PRE_RAW[i].store(v, Ordering::Relaxed);
            match unread {
                Some(why) => serial_println!("[ga10bprobe4c] pre-ignition {} @{:#x} = -UNREADABLE reason={} val={:#010x}", r.name, r.off, why, v),
                None => serial_println!("[ga10bprobe4c] pre-ignition {} @{:#x} = {:#010x}", r.name, r.off, v),
            }
            answered();
            continue;
        }
        let pre = PRE_RAW[i].load(Ordering::Relaxed);
        let pre_unread = unreadable_reason(pre).is_some();
        let is_bcr = (R4C_BCR_FIRST..=R4C_BCR_LAST).contains(&i);
        let diff = if !pre_done {
            "no-pre"
        } else if is_bcr {
            "written-by-4b"
        } else if pre_unread && unread.is_none() {
            became_readable += 1;
            "became-readable"
        } else if !pre_unread && unread.is_some() {
            became_unreadable += 1;
            "became-unreadable"
        } else if pre != v {
            changed += 1;
            if i != R4C_BR_RETCODE { changed_ex += 1; }
            "changed"
        } else {
            "same"
        };
        // The BCR eight: intact means the ROM left 4b's descriptor exactly as written.
        let mut bcr_note = "";
        let mut expect = 0u32;
        let mut intact = 0u32;
        if is_bcr {
            expect = bcr_expect[i - R4C_BCR_FIRST];
            if unread.is_some() { bcr_unread += 1; bcr_note = " intact=-UNREADABLE"; }
            else if v == expect { bcr_intact += 1; intact = 1; bcr_note = " intact=1"; }
            else { bcr_altered += 1; bcr_note = " intact=0"; }
        }
        match unread {
            Some(why) => serial_println!("[ga10bprobe4c] post-ignition {} @{:#x} = -UNREADABLE reason={} val={:#010x} diff={} pre={:#010x}{}", r.name, r.off, why, v, diff, pre, bcr_note),
            None => serial_println!("[ga10bprobe4c] post-ignition {} @{:#x} = {:#010x} diff={} pre={:#010x}{}", r.name, r.off, v, diff, pre, bcr_note),
        }
        if is_bcr {
            serial_println!("[ga10bprobe4c] post-ignition {} expected={:#010x} intact={}{}", r.name, expect, intact, if unread.is_some() { " (unreadable — folded into neither intact nor altered, F4)" } else { "" });
        }
        answered();
    }
    if !post {
        PRE_DONE.store(true, Ordering::Relaxed);
        serial_println!("[ga10bprobe4c] pre-ignition census: readable={}/{} unreadable={} (rung 3 on render11 read 16/25 readable, 9 unreadable; the 5 added registers were read by rungs 1, 3b and 4b) -> PRECENSUS-DONE", n_read, R4C_N, n_unread);
        return;
    }
    let bcr_arm = if bcr_unread > 0 { "POSTBCR-UNREADABLE" } else if bcr_altered == 0 { "POSTBCR-INTACT" } else { "POSTBCR-ALTERED" };
    serial_println!("[ga10bprobe4c] bcr post-ignition: intact={}/8 altered={} unreadable={} (the 8 registers 4b wrote, read back after the ROM's verdict: INTACT means the ROM consumed the descriptor without altering it) -> {}", bcr_intact, bcr_altered, bcr_unread, bcr_arm);
    let map_arm = if pre_done && became_readable == 0 && became_unreadable == 0 && changed_ex == 0 { "MAPDIFF-SAME" } else { "MAPDIFF-CHANGED" };
    serial_println!("[ga10bprobe4c] post-ignition census: readable={}/{} unreadable={} became_readable={} became_unreadable={} changed={} changed_ex_bcr_retcode={} pre_done={} (the lockdown-drop oracle over every register the ladder has read: SAME means the ignition changed nothing but what 4b wrote and the verdict register) -> {}", n_read, R4C_N, n_unread, became_readable, became_unreadable, changed, changed_ex, pre_done as u32, map_arm);
}

/// RUNG 4c pass 1 — called from `ga10bprobe4_body` after 4a's census block, on the rail proven ON this boot.
#[cfg(feature = "ga10bprobe4c")]
fn rung4c_pre(base: u64) {
    serial_println!("[ga10bprobe4c] pass 1 — PRE-ignition census of {} registers (after 4a's restore, before 4b; read-only)", R4C_N);
    census_4c(false, base, &[0u32; 8]);
}

/// The br_retcode series: N announced-once samples after 4b's verdict, every DISTINCT value printed with the
/// sample index and elapsed ms at which it first appeared; the summary is the announce's one result line.
#[cfg(feature = "ga10bprobe4c")]
fn br_series(f2: u64) {
    let rc = f2 + PRISCV_BR_RETCODE_OFF;
    let n = BR_SERIES_SAMPLES;
    serial_println!("[ga10bprobe4c] about-to-read series priscv_br_retcode reg={:#x} samples={} settle_ms={} (a repeat read of the register 4b just polled; every DISTINCT value is printed with the sample index and time it first appeared) — if this is the LAST line, THAT read was EL3-fatal", rc, n, BR_SERIES_SETTLE_MS);
    ANN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let t0 = now_ms();
    let mut have = false;
    let mut last: u32 = 0;
    let mut distinct = 0u32;
    let mut first_v = 0u32;
    let mut first_i = 0u32;
    let mut last_i = 0u32;
    let mut any_unread = false;
    let mut reason_or = 0u32;
    for i in 1..=n {
        let v = r32(rc);
        let t = now_ms().wrapping_sub(t0);
        if unreadable_reason(v).is_some() { any_unread = true; } else { reason_or |= v >> 2; }
        if !have || v != last {
            distinct += 1;
            if !have { first_v = v; first_i = i; }
            last_i = i;
            serial_println!("[ga10bprobe4c] series sample={}/{} t_ms={} br_retcode={:#010x} br_result={:#x} reason_bits={:#010x} (distinct #{})", i, n, t, v, v & 0x3, v >> 2, distinct);
        }
        have = true;
        last = v;
        if i < n { settle_ms(BR_SERIES_SETTLE_MS); }
    }
    let elapsed = now_ms().wrapping_sub(t0);
    let arm = if any_unread { "BRSERIES-UNREADABLE" } else if distinct == 1 { "BRSERIES-STABLE" } else { "BRSERIES-CHANGED" };
    serial_println!("[ga10bprobe4c] series priscv_br_retcode @{:#x} = distinct={} first={:#010x}@{} last={:#010x}@{} reason_bits_or={:#010x} samples={} elapsed_ms={} (reason_bits = bits[31:2]; a non-zero OR means the ROM published something above the verdict — brief §7's UNKNOWN answered either way) -> {}", PRISCV_BR_RETCODE_OFF, distinct, first_v, first_i, last, last_i, reason_or, n, elapsed, arm);
    answered();
}

/// The DMA window after the ROM: the first four words at each descriptor target, then a full scan for any
/// word that is not the fill pattern. A CPU read of this kernel's own Normal-NC DRAM — not a GPU register.
#[cfg(feature = "ga10bprobe4c")]
fn dmabuf_post(pa: u64, size: u64) {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    for (label, off) in [("fmccode", 0u64), ("fmcdata", DMABUF_FMCDATA_OFF), ("pkcparam", DMABUF_PKC_OFF)] {
        let a = pa + off;
        serial_println!("[ga10bprobe4c] about-to-read post-ignition dmabuf {} pa={:#x} (a CPU read of this kernel's OWN Normal-NC window — DRAM the boot ROM's descriptor pointed at, not a GPU register) — if this is the LAST line, THAT read was fatal", label, a);
        ANN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let w = [r32(a), r32(a + 4), r32(a + 8), r32(a + 12)];
        serial_println!("[ga10bprobe4c] post-ignition dmabuf {} @{:#x} = {:#010x} {:#010x} {:#010x} {:#010x} (first 4 words; fill pattern={:#010x})", label, a, w[0], w[1], w[2], w[3], DMABUF_PATTERN);
        answered();
    }
    serial_println!("[ga10bprobe4c] about-to-read post-ignition dmabuf-scan pa={:#x} size={:#x} (every word of the window, CPU side, compared to the fill pattern) — if this is the LAST line, THAT read was fatal", pa, size);
    ANN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let total = size / 4;
    let mut changed = 0u64;
    let mut first_off: Option<u64> = None;
    let mut first_val = 0u32;
    let mut off = 0u64;
    while off < size {
        let v = r32(pa + off);
        if v != DMABUF_PATTERN {
            changed += 1;
            if first_off.is_none() { first_off = Some(off); first_val = v; }
        }
        off += 4;
    }
    let arm = if changed == 0 { "DMABUF-UNTOUCHED" } else { "DMABUF-ALTERED" };
    match first_off {
        Some(o) => serial_println!("[ga10bprobe4c] post-ignition dmabuf-scan @{:#x} = words_changed={}/{} first_changed_off={:#x} first_changed_val={:#010x} -> {}", pa, changed, total, o, first_val, arm),
        None => serial_println!("[ga10bprobe4c] post-ignition dmabuf-scan @{:#x} = words_changed={}/{} first_changed_off=none -> {}", pa, changed, total, arm),
    }
    answered();
}

/// RUNG 4c pass 2 — called from `rung4b` after its summary, before its SYSTEM_OFF: the series, the post
/// census (with the BCR eight judged against what 4b wrote), the DMA-window readback, the complete line.
#[cfg(feature = "ga10bprobe4c")]
fn rung4c_post(base: u64, f2: u64, dmabuf_pa: u64, lockval: u32) {
    use core::sync::atomic::Ordering;
    serial_println!("[ga10bprobe4c] pass 2 — POST-ignition: br_retcode series, then the {}-register census diffed against pass 1, then the DMA window (read-only; 4b's SYSTEM_OFF follows)", R4C_N);
    br_series(f2);
    let vals = bcr_addr_values(dmabuf_pa);
    // RUNG4C_REGS order 15..=22: bcr_ctrl, bcr_dmacfg, pkcparam lo/hi, fmccode lo/hi, fmcdata lo/hi;
    // bcr_addr_values order: fmccode lo/hi, fmcdata lo/hi, pkcparam lo/hi.
    let expect = [BCR_CTRL_BROM_CONFIG, lockval, vals[4], vals[5], vals[0], vals[1], vals[2], vals[3]];
    census_4c(true, base, &expect);
    let (_, ws) = super::mmu_tegra::ga10b4_nc_window();
    if dmabuf_pa != 0 && ws != 0 {
        dmabuf_post(dmabuf_pa, ws);
    } else {
        serial_println!("[ga10bprobe4c] post-ignition dmabuf-scan skipped pa=0x00000000 words_changed=0/0 (no window seated this boot, so nothing to read) -> DMABUF-UNTOUCHED");
    }
    serial_println!("[ga10bprobe4c] rung 4c complete: reads_announced={} reads_answered={} (zero writes) -> CENSUS-COMPLETE", ANN.load(Ordering::Relaxed), ANS.load(Ordering::Relaxed));
}

// ── GA10B-PROBE4D — the DEFERRED arm ─────────────────────────────────────────────────────────────

#[cfg(feature = "ga10bprobe4d")]
static DEFERRED_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "ga10bprobe4d")]
static DEF_DTB_ADDR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "ga10bprobe4d")]
static DEF_DTB_SIZE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(feature = "ga10bprobe4d")]
static DEF_RAM_MASK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "ga10bprobe4d")]
static DEF_DTB_SUM: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// A cheap whole-blob checksum of the firmware DTB, taken when the rung is armed and re-taken before the
/// deferred run walks it: the DTB lives in UEFI-allocated RAM the kernel never recycles (no frame allocator
/// on this path; the heap is a fixed window), and this check is what makes that a measurement.
#[cfg(feature = "ga10bprobe4d")]
fn dtb_sum(addr: u64, size: usize) -> u32 {
    if addr == 0 || size == 0 {
        return 0;
    }
    let b = unsafe { core::slice::from_raw_parts(addr as *const u8, size) };
    b.iter().fold(0u32, |s, &x| s.wrapping_mul(31).wrapping_add(x as u32))
}

/// Arm the deferred run (the `=4` boot-time entry): stash the DTB coordinates, print the arm, RETURN.
#[cfg(feature = "ga10bprobe4d")]
fn arm_deferred(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    use core::sync::atomic::Ordering;
    let sum = dtb_sum(dtb_addr, dtb_size);
    DEF_DTB_ADDR.store(dtb_addr, Ordering::Relaxed);
    DEF_DTB_SIZE.store(dtb_size, Ordering::Relaxed);
    DEF_RAM_MASK.store(ram_gib_mask, Ordering::Relaxed);
    DEF_DTB_SUM.store(sum, Ordering::Relaxed);
    DEFERRED_ARMED.store(true, Ordering::SeqCst);
    serial_println!("[ga10bprobe4d] DEFERRED ARM (UNAOS_GA10B_PROBE4=4): rungs 4a+4b+4c are ARMED for the shutdown path and do NOT run now — the boot continues into the desktop. The rung runs when a PSCI SYSTEM_OFF is requested (shell `shutdown`/`off`, crystal Shut Down), on the requesting core, before the OFF. Stashed dtb={:#x} size={:#x} ram_gib_mask={:#x} dtb_sum={:#010x} (re-verified at flight time; a mismatch REFUSES with zero MMIO). A SYSTEM_RESET (restart) does NOT trigger it; nothing else in the boot touches the GPU aperture or the BPMP channel after this line", dtb_addr, dtb_size, ram_gib_mask, sum);
}

/// The deferred run. Called by `power::psci_call` on every PSCI SYSTEM_OFF request; a no-op unless armed,
/// and the flag is consumed on entry so the rung's own finish4 -> shutdown -> psci_call is a no-op too.
/// Masks DAIF on the calling core, re-verifies the DTB, re-derives the BPMP channel, runs 4a -> 4b -> 4c.
/// Returns (falling through to the SYSTEM_OFF it interrupted) only on a refusal or a 4a path that did not
/// reach 4b.
#[cfg(feature = "ga10bprobe4d")]
pub fn ga10bprobe4_deferred_run() {
    use core::sync::atomic::Ordering;
    if !DEFERRED_ARMED.swap(false, Ordering::SeqCst) {
        return;
    }
    unsafe { core::arch::asm!("msr daifset, #0xf", options(nomem, nostack, preserves_flags)) };
    let dtb_addr = DEF_DTB_ADDR.load(Ordering::Relaxed);
    let dtb_size = DEF_DTB_SIZE.load(Ordering::Relaxed);
    let ram_gib_mask = DEF_RAM_MASK.load(Ordering::Relaxed);
    let sum0 = DEF_DTB_SUM.load(Ordering::Relaxed);
    serial_println!("[ga10bprobe4d] DEFERRED RUN — triggered by a PSCI SYSTEM_OFF request reaching power::psci_call (the shutdown verb; the [pwrshutoff] or [crystal] line above names the route). DAIF masked on this core; the other cores keep hosting and, by the stated assumption (brief §10), touch neither the BPMP channel nor the GPU aperture. Running 4a -> 4b -> 4c NOW, the bracket re-proven from BPMP, nothing inherited from boot but the stashed DTB coordinates — then the SYSTEM_OFF this interrupted");
    let sum = dtb_sum(dtb_addr, dtb_size);
    if sum != sum0 {
        serial_println!("[ga10bprobe4d] REFUSED reason=dtb-changed stashed_sum={:#010x} now={:#010x} — the firmware DTB is not the blob the arm read; zero MMIO; falling through to SYSTEM_OFF", sum0, sum);
        return;
    }
    serial_println!("[ga10bprobe4d] dtb re-verified sum={:#010x} dtb={:#x} size={:#x} — re-deriving the BPMP channel from the same geometry", sum, dtb_addr, dtb_size);
    let Some(g) = super::fdt_tegra::bpmp_geometry(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!("[ga10bprobe4d] REFUSED reason=no-bpmp-geometry — zero MMIO; falling through to SYSTEM_OFF");
        return;
    };
    let Some(c) = super::bpmp_tegra::chan_reopen(&g) else {
        serial_println!("[ga10bprobe4d] REFUSED reason=no-doorbell — zero MMIO; falling through to SYSTEM_OFF");
        return;
    };
    ga10bprobe4_body(&c, dtb_addr, dtb_size, ram_gib_mask);
    serial_println!("[ga10bprobe4d] deferred rung RETURNED (a 4a path that did not reach 4b's SYSTEM_OFF) — falling through to the SYSTEM_OFF it interrupted");
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// GA10B-PROBE4E / GA10B-PROBE4F — RUNGS 4e and 4f: two ONE-BOOT arms of the SAME rung 4
// (`ga10bprobe4e` / `ga10bprobe4f`, DEFAULT OFF, each IMPLIES `ga10bprobe4b`; `UNAOS_GA10B_PROBE4=5`
// and `=6`). Design: docs/dev/OS/08_VIDEO/GA10B-RUNG4-BRIEF.md §12, from GA10B-RUNG5-BRIEF.md §1.2
// (the encoding finding), §2.3 cause 5, §2.6 (the experiment) and §6 Q4/Q5. Ledger A62.
//
// Each arm is the FLOWN `=2` rung with exactly ONE value changed, and nothing else:
//   * 4e SHIFT   — the six BCR DMA address registers are written `pa >> 8` (the register holds the
//     physical address in 256-byte units per NVIDIA's published MIT Hopper bootstrap) instead of raw.
//     The `lo` half is the low 32 bits of `pa >> 8`, the `hi` half the rest (the HI registers are 12
//     bits wide — rung-5 brief §1.1 — so a sub-4 GiB window puts zero there either way, and the delta
//     is entirely in `lo`). Every announce carries BOTH the raw address and the value written.
//   * 4f BRFETCH — `bcr_ctrl` is written 0x011 instead of 0x111: same register, same address class,
//     one bit of the decoded field map (BRFETCH TRUE -> FALSE), the SEQ's alternate `set_bcr` value.
//
// NO NEW WRITE CLASS, NO NEW REGISTER, NO NEW ADDRESS CLASS. 4b's seven BCR writes, the lock, the
// trigger and the ignition remain the only GA10B writes in the boot; the flight ends in `SYSTEM_OFF`
// on every path exactly as `=2` does; rung 4c is NOT carried by either arm.
//
// WITNESS FAMILIES `[ga10bprobe4e]` / `[ga10bprobe4f]` (15 bytes bracketed, over the 8-byte LLVM
// immediate-encode floor). The arm ALSO tags the 4a and 4b SUMMARY lines — `shift=8` on 4e,
// `brfetch=false` on 4f — so a capture says which arm flew without the scorer reading placement, and
// so `docs/dev/evidence/orin27/scorer-ga10b4.sh 4e|4f` can tell one wire from the other (that scorer's
// 4e/4f legs go RED on a capture without the tag, which is how the `=2` wires already on disk score).
//
// BYTE IDENTITY OF THE FLOWN ARMS. Every site these two features touch above is an EXISTING line
// rewritten in place — the numstat for this change is 11 added / 11 deleted in this file, no hunk
// changing a line count — and everything new lives in THIS block, which is the last thing in the
// file, so nothing compiled sits below it and no `panic::Location` moves. Measured, not argued: the
// `=2` and `=4` loadable images (`llvm-objcopy -O binary`, one directory, one pinned `UNAOS_GIT_SHA`)
// are byte-identical across this change, and `./arroyo knoboff ga10bprobe4a` covers the default one.

/// The right shift applied to a BCR DMA address before it is split into its LO/HI halves.
/// 0 on every arm that has flown; 8 under rung 4e, which is the whole of that arm.
#[cfg(all(feature = "ga10bprobe4a", not(feature = "ga10bprobe4e")))] const R4_ADDR_SHIFT: u32 = 0;
#[cfg(feature = "ga10bprobe4e")] const R4_ADDR_SHIFT: u32 = 8;

/// The RAW physical address behind `BCR_ADDR_REGS[i]` — the thing 4e's announce prints beside the
/// shifted value it writes. A `match`, never an index: an index would add a bounds check, and a
/// bounds check is a `panic::Location` this file must not grow.
#[cfg(feature = "ga10bprobe4e")]
fn bcr_addr_raw(pa: u64, i: usize) -> u64 {
    match i {
        0 | 1 => pa,
        2 | 3 => pa + DMABUF_FMCDATA_OFF,
        _ => pa + DMABUF_PKC_OFF,
    }
}

/// 4e's announced write + readback for the six ADDRESS registers. `bcr_write_verify`'s shape exactly —
/// one announce line, then exactly one result line, so `write_announces == write_results` still holds
/// (brief §4 row C) — with `raw_pa=` and `shift=8` added to both, so the wire carries the address the
/// rung MEANT and the number it actually put in the register, and a scorer can check the arithmetic
/// instead of trusting a tag. `bcr_write_verify` itself is untouched: the flown arms call it, and the
/// restore writes (value 0, no address behind them) call it here too.
#[cfg(feature = "ga10bprobe4e")]
fn bcr_write_verify_shift(fam: &str, name: &str, f2: u64, off: u64, val: u32, raw: u64, why: &str) -> Result<bool, &'static str> {
    let addr = f2 + off;
    serial_println!("[{}] about-to-WRITE {} reg={:#x} val={:#010x} raw_pa={:#010x} shift=8 ({}) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", fam, name, addr, val, raw, why);
    w32_4(addr, val);
    let got = r32(addr);
    match unreadable_reason(got) {
        Some(r) => {
            serial_println!("[{}] {} @{:#x} wrote={:#010x} read=-UNREADABLE reason={} val={:#010x} raw_pa={:#010x} shift=8 — the register stopped answering after the write; NOT folded into held or not-held (F4)", fam, name, off, val, r, got, raw);
            Err(r)
        }
        None => {
            let held = got == val;
            serial_println!("[{}] {} @{:#x} wrote={:#010x} read={:#010x} raw_pa={:#010x} shift=8 held={}", fam, name, off, val, got, raw, held as u32);
            Ok(held)
        }
    }
}

/// 4e's ARMED banner, printed from the fold on rung 4a's opening line.
#[cfg(feature = "ga10bprobe4e")]
fn r4e_banner() {
    serial_println!("[ga10bprobe4e] rung 4e ARMED (UNAOS_GA10B_PROBE4=5) — the FLOWN 4a+4b rung with ONE delta: the six BCR DMA address registers are written shift=8, i.e. pa >> 8, because NVIDIA's published MIT Hopper GSP-FMC bootstrap writes them in 256-byte units. The flown arms wrote them RAW, so if this die's boot ROM shares that encoding the ignition pointed it about 512 GiB above DRAM and the signature wall was never reached (rung-5 brief §1.2, §2.3 cause 5). NOTHING else changes: same registers, same bcr_dmacfg, same bcr_ctrl=0x00000111, same ignition, same bounded poll, same SYSTEM_OFF on every path, no rung 4c. Every address announce carries raw_pa= beside the value written, and the 4a/4b summary lines carry shift=8. WHAT THIS CANNOT DECIDE: br_retcode is not an oracle — a correctly encoded but UNSIGNED payload returns the same 0x00000002 (rung-5 brief §2.5). The oracles that discriminate are post_lockdown, v1_readable and the DMA-window scan, and all three read the negative side on the flown wire");
}

/// 4f's ARMED banner, printed from the same fold.
#[cfg(feature = "ga10bprobe4f")]
fn r4f_banner() {
    serial_println!("[ga10bprobe4f] rung 4f ARMED (UNAOS_GA10B_PROBE4=6) — the FLOWN 4a+4b rung with ONE delta: bcr_ctrl is written 0x00000011 instead of 0x00000111, brfetch=false. The MIT GA102 dev_riscv_pri.h decomposes that register exactly — BRFETCH bit 8, CORE_SELECT bit 4, VALID bit 0 — so the platform firmware's leftover 0x00000110 is BRFETCH TRUE + RISCV + NOT VALID, 4b's flown 0x00000111 marked it valid, and 0x00000011 is the ACKED SEQ's alternate set_bcr value: configure the BCR but do not have the boot ROM fetch through it (rung-5 brief §2.2, §6 Q5 — a door named by that brief and taken by nobody). Same register, same address class, no new write. NOTHING else changes: addresses raw as flown, same bcr_dmacfg, same ignition, same bounded poll, same SYSTEM_OFF on every path, no rung 4c. The 4a/4b summary lines carry brfetch=false. NO THEORY IS OFFERED about what the ROM does with BRFETCH FALSE: this arm spends one power cycle to find out, and BCR-CTRL-REFUSED is a real outcome — the register may simply not take the value");
}
