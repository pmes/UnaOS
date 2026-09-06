// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// GA10B-PROBE1 — the FIRST read-only probe rung for the Orin Nano's Ampere GA10B iGPU. (GA10B-PROBE2, rung
// 2 — power + clocks + PMC_BOOT_0 — lives at the TAIL of this file under its SIBLING knob `ga10bprobe2`;
// see the ladder docs/dev/evidence/orin14/GA10B-LADDER.md. Everything above the rung-2 marker is rung 1.)
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
//   * ZERO MMIO WRITES. Every GPU access here is `core::ptr::read_volatile`. There is no
//     `write_volatile` in this module and there must not be — the block is left exactly as MB2
//     handed it over.
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
#[cfg(feature = "ga10bprobe1")] const GSP_FALCON_BASE: u64 = 0x0011_0000;
// facts (Aperture framing): GSP falcon2 (RISC-V / priscv) base in BAR0 = 0x00111000.
#[cfg(feature = "ga10bprobe1")] const GSP_FALCON2_BASE: u64 = 0x0011_1000;

// facts (b) Security-state fuses: opt_priv_sec_en 0x820434 (set => secure boot enforced). BAR0-rel.
#[cfg(feature = "ga10bprobe1")] const FUSE_OPT_PRIV_SEC_EN: u64 = 0x0082_0434;
// facts (b) Legacy Falcon regs: hwcfg2 0x0f4 — riscv_br_priv_lockdown bit13 (==1 => BR priv lockdown
// engaged). falcon-base-relative.
#[cfg(feature = "ga10bprobe1")] const FALCON_HWCFG2_OFF: u64 = 0x0f4;
#[cfg(feature = "ga10bprobe1")] const HWCFG2_PRIV_LOCKDOWN_BIT: u32 = 13;
// facts (b) RISC-V boot-ROM interface — priscv: br_retcode 0x65c — result bits[1:0]; FAIL=0x2,
// PASS=0x3; 0x0/0x1 = no verdict yet. falcon2-base-relative.
#[cfg(feature = "ga10bprobe1")] const PRISCV_BR_RETCODE_OFF: u64 = 0x65c;
#[cfg(feature = "ga10bprobe1")] const BR_RETCODE_FAIL: u32 = 0x2;
#[cfg(feature = "ga10bprobe1")] const BR_RETCODE_PASS: u32 = 0x3;
// facts (b) RISC-V boot-ROM interface — priscv: cpuctl 0x388 — halted bit4. falcon2-base-relative.
#[cfg(feature = "ga10bprobe1")] const PRISCV_CPUCTL_OFF: u64 = 0x388;
#[cfg(feature = "ga10bprobe1")] const PRISCV_CPUCTL_HALTED_BIT: u32 = 4;
// facts (b) Die-characterization: top_num_gpcs 0x022430 value bits[4:0] (GA10B Orin Nano = 2 GPC).
// BAR0-relative.
#[cfg(feature = "ga10bprobe1")] const TOP_NUM_GPCS: u64 = 0x0002_2430;
#[cfg(feature = "ga10bprobe1")] const TOP_NUM_GPCS_MASK: u32 = 0x1f;

/// One read-only 32-bit BAR0 access. There is NO write counterpart in this module by construction.
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

#[cfg(feature = "ga10bprobe2")]
const CMD_PG_SET_STATE: u32 = 1;
#[cfg(feature = "ga10bprobe2")]
const PG_STATE_OFF: u32 = 0;
#[cfg(feature = "ga10bprobe2")]
const MRQ_CLK: u32 = 22;
#[cfg(feature = "ga10bprobe2")]
const CMD_CLK_IS_ENABLED: u32 = 6;
#[cfg(feature = "ga10bprobe2")]
const CMD_CLK_ENABLE: u32 = 7;
#[cfg(feature = "ga10bprobe2")]
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
#[cfg(feature = "ga10bprobe2")]
const PRI_ERROR_PATTERN: u32 = 0xBAD0_0000;
#[cfg(feature = "ga10bprobe2")]
const PRI_ERROR_MASK: u32 = 0xFFF0_0000;

/// Bounded spin of ~`ms` milliseconds on CNTPCT (the bpmp_tegra `wait_ms` idiom, without a predicate):
/// the settle time between a power/clock MRQ and the BAR0 read. BPMP acks synchronously, so this is
/// margin, not a protocol requirement.
#[cfg(feature = "ga10bprobe2")]
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
#[cfg(feature = "ga10bprobe2")]
fn pg_state(chan: &super::bpmp_tegra::Chan, id: u32) -> Option<(i32, u32)> {
    chan.transfer(MRQ_PG, &[CMD_PG_GET_STATE, id]).map(|(err, out)| (err, out[0]))
}

/// MRQ_CLK with one subcommand for one clock id: `Some((err, payload[0]))`, `None` = timeout.
#[cfg(feature = "ga10bprobe2")]
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
