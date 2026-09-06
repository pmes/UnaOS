// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// GA10B-PROBE1 — the FIRST read-only probe rung for the Orin Nano's Ampere GA10B iGPU. (GA10B-PROBE2, rung
// 2 — power + clocks + PMC_BOOT_0 — lives at the TAIL of this file under its SIBLING knob `ga10bprobe2`;
// GA10B-PROBE3, rungs 3 and 3b — the read-only pass over what the platform firmware left behind, and the
// ladder's first GA10B MMIO writes — lives after it under `ga10bprobe3` / `ga10bprobe3b`;
// see the ladder docs/dev/evidence/orin14/GA10B-LADDER.md and the as-built spec
// docs/dev/evidence/orin16/GA10B-RUNG3.md. Everything above the rung-2 marker is rung 1.)
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
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe3"))] const GSP_FALCON_BASE: u64 = 0x0011_0000;
// facts (Aperture framing): GSP falcon2 (RISC-V / priscv) base in BAR0 = 0x00111000.
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe3"))] const GSP_FALCON2_BASE: u64 = 0x0011_1000;

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
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe3"))] const PRISCV_CPUCTL_OFF: u64 = 0x388;
#[cfg(any(feature = "ga10bprobe1", feature = "ga10bprobe3"))] const PRISCV_CPUCTL_HALTED_BIT: u32 = 4;
// facts (b) Die-characterization: top_num_gpcs 0x022430 value bits[4:0] (GA10B Orin Nano = 2 GPC).
// BAR0-relative.
#[cfg(feature = "ga10bprobe1")] const TOP_NUM_GPCS: u64 = 0x0002_2430;
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

#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
const CMD_PG_SET_STATE: u32 = 1;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
const PG_STATE_OFF: u32 = 0;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
const MRQ_CLK: u32 = 22;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
const CMD_CLK_IS_ENABLED: u32 = 6;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
const CMD_CLK_ENABLE: u32 = 7;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
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
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
const PRI_ERROR_PATTERN: u32 = 0xBAD0_0000;
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
const PRI_ERROR_MASK: u32 = 0xFFF0_0000;

/// Bounded spin of ~`ms` milliseconds on CNTPCT (the bpmp_tegra `wait_ms` idiom, without a predicate):
/// the settle time between a power/clock MRQ and the BAR0 read. BPMP acks synchronously, so this is
/// margin, not a protocol requirement.
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
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
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
fn pg_state(chan: &super::bpmp_tegra::Chan, id: u32) -> Option<(i32, u32)> {
    chan.transfer(MRQ_PG, &[CMD_PG_GET_STATE, id]).map(|(err, out)| (err, out[0]))
}

/// MRQ_CLK with one subcommand for one clock id: `Some((err, payload[0]))`, `None` = timeout.
#[cfg(any(feature = "ga10bprobe2", feature = "ga10bprobe3"))]
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
#[cfg(feature = "ga10bprobe3")] const FUSE_OPT_SEC_DEBUG_EN: u64 = 0x0082_1040;
/// facts (b) Security-state fuses: opt_wpr_enabled (the ACR's write-protected region). BAR0-relative.
/// One of rung 4's two inputs — it gets its own summary line.
#[cfg(feature = "ga10bprobe3")] const FUSE_OPT_WPR_ENABLED: u64 = 0x0082_05ec;
/// facts (b) Security-state fuses: opt_vpr_enabled. BAR0-relative.
#[cfg(feature = "ga10bprobe3")] const FUSE_OPT_VPR_ENABLED: u64 = 0x0082_067c;
/// facts (b) Die-characterization: mc_enable. BAR0-relative. NEW ADDRESS CLASS this boot.
#[cfg(feature = "ga10bprobe3")] const MC_ENABLE: u64 = 0x0000_0200;
/// facts (b) Die-characterization: mc_elpg_enable — xbar 0x4, l2 0x8, hub 0x20000000. BAR0-relative.
#[cfg(feature = "ga10bprobe3")] const MC_ELPG_ENABLE: u64 = 0x0000_020c;
#[cfg(feature = "ga10bprobe3")] const MC_ELPG_XBAR: u32 = 0x4;
#[cfg(feature = "ga10bprobe3")] const MC_ELPG_L2: u32 = 0x8;
#[cfg(feature = "ga10bprobe3")] const MC_ELPG_HUB: u32 = 0x2000_0000;
/// facts (b) Die-characterization: top_device_info_cfg — version_init = 0x2; the device_info2 table
/// walk itself is rung 5's, not this rung's. BAR0-relative.
#[cfg(feature = "ga10bprobe3")] const TOP_DEVICE_INFO_CFG: u64 = 0x0002_24fc;
/// facts (b) Legacy Falcon regs, falcon-base-relative: irqmask 0x018, irqdest 0x01c, idlestate 0x04c,
/// cpuctl 0x100 (halt_intr bit4), hwcfg 0x108, dmactl 0x10c (require_ctx bit0).
#[cfg(feature = "ga10bprobe3")] const FALCON_IRQMASK_OFF: u64 = 0x018;
#[cfg(feature = "ga10bprobe3")] const FALCON_IRQDEST_OFF: u64 = 0x01c;
#[cfg(feature = "ga10bprobe3")] const FALCON_IDLESTATE_OFF: u64 = 0x04c;
#[cfg(feature = "ga10bprobe3")] const FALCON_CPUCTL_OFF: u64 = 0x100;
#[cfg(feature = "ga10bprobe3")] const FALCON_CPUCTL_HALT_INTR_BIT: u32 = 4;
#[cfg(feature = "ga10bprobe3")] const FALCON_HWCFG_OFF: u64 = 0x108;
#[cfg(feature = "ga10bprobe3")] const FALCON_DMACTL_OFF: u64 = 0x10c;
#[cfg(feature = "ga10bprobe3")] const FALCON_DMACTL_REQUIRE_CTX_BIT: u32 = 0;
/// facts (b) RISC-V boot-ROM interface, falcon2(priscv)-base-relative: bcr_ctrl 0x668,
/// bcr_dmacfg 0x66c (lock_locked 0x80000000), BCR DMA addrs 0x670..0x684, boot_vector 0x380/0x384,
/// riscv_irqmask 0x528, riscv_irqdest 0x52c.
#[cfg(feature = "ga10bprobe3")] const PRISCV_BCR_CTRL_OFF: u64 = 0x668;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BCR_DMACFG_OFF: u64 = 0x66c;
#[cfg(feature = "ga10bprobe3")] const BCR_DMACFG_LOCK_LOCKED: u32 = 0x8000_0000;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BCR_PKCPARAM_LO_OFF: u64 = 0x670;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BCR_PKCPARAM_HI_OFF: u64 = 0x674;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BCR_FMCCODE_LO_OFF: u64 = 0x678;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BCR_FMCCODE_HI_OFF: u64 = 0x67c;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BCR_FMCDATA_LO_OFF: u64 = 0x680;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BCR_FMCDATA_HI_OFF: u64 = 0x684;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BOOT_VECTOR_LO_OFF: u64 = 0x380;
#[cfg(feature = "ga10bprobe3")] const PRISCV_BOOT_VECTOR_HI_OFF: u64 = 0x384;
#[cfg(feature = "ga10bprobe3")] const PRISCV_RISCV_IRQMASK_OFF: u64 = 0x528;
#[cfg(feature = "ga10bprobe3")] const PRISCV_RISCV_IRQDEST_OFF: u64 = 0x52c;
/// facts (Aperture framing): PMU falcon2 base in BAR0 = 0x0010b000 — a DISTINCT engine from the GSP,
/// and the one NEW engine aperture this rung reads. Its cpuctl sits at the priscv-relative 0x388, the
/// same offset the facts file gives for the GSP's (facts (b) RISC-V boot-ROM interface).
#[cfg(feature = "ga10bprobe3")] const PMU_FALCON2_BASE: u64 = 0x0010_b000;

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
#[cfg(feature = "ga10bprobe3b")] const FALCON_MAILBOX0_OFF: u64 = 0x040;
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
