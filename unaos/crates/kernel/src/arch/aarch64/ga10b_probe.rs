// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// GA10B-PROBE1 — the FIRST read-only probe rung for the Orin Nano's Ampere GA10B iGPU
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

use super::fdt_tegra::{Fdt, PropWords};

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
const GSP_FALCON_BASE: u64 = 0x0011_0000;
// facts (Aperture framing): GSP falcon2 (RISC-V / priscv) base in BAR0 = 0x00111000.
const GSP_FALCON2_BASE: u64 = 0x0011_1000;

// facts (b) Security-state fuses: opt_priv_sec_en 0x820434 (set => secure boot enforced). BAR0-rel.
const FUSE_OPT_PRIV_SEC_EN: u64 = 0x0082_0434;
// facts (b) Legacy Falcon regs: hwcfg2 0x0f4 — riscv_br_priv_lockdown bit13 (==1 => BR priv lockdown
// engaged). falcon-base-relative.
const FALCON_HWCFG2_OFF: u64 = 0x0f4;
const HWCFG2_PRIV_LOCKDOWN_BIT: u32 = 13;
// facts (b) RISC-V boot-ROM interface — priscv: br_retcode 0x65c — result bits[1:0]; FAIL=0x2,
// PASS=0x3; 0x0/0x1 = no verdict yet. falcon2-base-relative.
const PRISCV_BR_RETCODE_OFF: u64 = 0x65c;
const BR_RETCODE_FAIL: u32 = 0x2;
const BR_RETCODE_PASS: u32 = 0x3;
// facts (b) RISC-V boot-ROM interface — priscv: cpuctl 0x388 — halted bit4. falcon2-base-relative.
const PRISCV_CPUCTL_OFF: u64 = 0x388;
const PRISCV_CPUCTL_HALTED_BIT: u32 = 4;
// facts (b) Die-characterization: top_num_gpcs 0x022430 value bits[4:0] (GA10B Orin Nano = 2 GPC).
// BAR0-relative.
const TOP_NUM_GPCS: u64 = 0x0002_2430;
const TOP_NUM_GPCS_MASK: u32 = 0x1f;

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
    Some(GpuNode { bar0, pd_id })
}

/// GA10B-PROBE1 — the guarded, read-only probe. Runs from `tegra_early_stop`'s BPMP block (BPMP-first:
/// it borrows the `chan` `jb1b_ping` established). ENDS THE BOOT: on every reachable path it powers
/// the board OFF (`power::shutdown`, PSCI SYSTEM_OFF) rather than returning, because a probe flight's
/// next boot must be cold and a dark board is the ready-for-cold-boot signal.
pub fn ga10bprobe1_run(
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
fn finish() -> ! {
    serial_println!("[ga10bprobe1] flight done — powering OFF; the dark board is the ready-for-cold-boot signal");
    crate::power::shutdown()
}
