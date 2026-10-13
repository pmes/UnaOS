//! GA10B-PROBE5G — RUNG 5g of the GA10B ladder: THE GSP-ACR DOOR, STEP ONE — the IMEM/DMEM PORT CENSUS
//! (`ga10bprobe5g`, DEFAULT OFF; `UNAOS_GA10B_PROBE5=6`; implies `ga10bprobe5a` for the entry seam and
//! rung 4a's helpers, and REPLACES the ignition on that build — nothing is spent, the boot continues
//! into the desktop). Design: docs/dev/evidence/ga10bnext/GA10BNEXT.md; docs/dev/OS/08_VIDEO/
//! GA10B-RUNG6-BRIEF.md §8 (dated 2026-10-14); ledger A111.
//!
//! WHY THIS RUNG. Six ignitions through the BRFETCH-DMA door answered `br_retcode=0x00000002` (rung-5
//! brief §9, A95). The one door the ladder has NEVER taken is nvgpu's second route, the GSP-ACR door —
//! `MAILBOX0/1` descriptor + `bcr_ctrl 0x11` (BRFETCH FALSE) + IMEM/DMEM PORT LOADS before STARTCPU
//! (orin-queue 2026-09-12; rung-5 brief §9.2). Its GA10B write list is in no public source, so by the
//! ladder's own rule it was "not a candidate". The ladder's OTHER rule is older: a write class is
//! censused before it is used (3 -> 3b, 4a -> 4b). The port registers (`IMEMC/IMEMD/IMEMT/DMEMC/DMEMD`)
//! are the door's write class, and NOTHING has ever read them on this die. This rung is door 2's "4a":
//! are the ports readable, do their control registers hold a value, and does ONE WORD round-trip through
//! each data port — all under the priv-lockdown that gates `cpuctl`/`dmactl`/`irqmask`/`idlestate` in the
//! same block (render15: `0xbadf5620`) while `hwcfg`/`hwcfg2`/`mailbox0` read as values. Its answer is NOT
//! the one-bit `0x2` oracle: a `pri-error` on the port says the door is gated at this altitude (an
//! attributable FAIL, R53's condition), and a held round-trip says the door's write class is OPEN and the
//! next rung (the port-load of the acr-gsp triple, `bcr_ctrl 0x11`, STARTCPU) has a measured floor.
//!
//! THE RUNG, in order, on ONE boot, ZERO vendor bytes, NO BCR write, NO lock, NO STARTCPU:
//!   1. The bracket (rung 4a's, in this family): gpu@ from the DTB, BPMP MRQ_PG ON with readback, clocks.
//!   2. READ census of the GSP legacy-falcon block (BAR0 + 0x110000), every read announced: hwcfg (the
//!      IMEM/DMEM sizes), hwcfg2 (lockdown), mailbox0, cpuctl_v1 (the lockdown control: pri-error on this
//!      die), then the first-touch five — mailbox1, imemc0, imemd0, imemt0, dmemc0, dmemd0.
//!   3. CONTROL census: `imemc0` and `dmemc0` each written `BLK=1`, read back, RESTORED as found —
//!      rung 4a's write-readback-restore, on a register that was a value in step 2.
//!   4. DATA round-trip, one word per port, only where the control register HELD: point the port at word 0,
//!      read as-found, write the pattern, re-point, read, re-point, write as-found back, re-point, read.
//!   5. The verdict line and the `:: ga10b: 5g … ::` line; the bracket restored; RETURN into the boot.
//!
//! PROVENANCE. Offsets `hwcfg 0x108`, `hwcfg2 0x0f4` (lockdown bit 13), `cpuctl 0x100` are the ACKED facts
//! file's (`ga10b-probe-rung1.facts.md`). `mailbox0 0x040` is PUBLIC-RECALLED and metal-proven (rung 3b).
//! EVERYTHING ELSE HERE IS PUBLIC-RECALLED AND NOT IN THE ACKED FILE: the port registers and their fields
//! (nouveau `nvkm/falcon/v1.c`, MIT — `load_imem` writes `0x180 + port*16` with AINCW bit 24 and SECURE bit
//! 28, `load_dmem` writes `0x1c0 + port*8 | 1<<24`, `read_dmem` sets bit 25; open-gpu-kernel-modules
//! `dev_falcon_v4.h`, MIT), `imemt 0x188`, `mailbox1 0x044`, and HWCFG's size fields (nouveau
//! `nvkm/falcon/base.c`: IMEM_SIZE bits [8:0] and DMEM_SIZE bits [17:9], in 256-byte blocks). Every one is
//! announced before it is touched and printed raw, the ladder's discipline for a first touch.
//!
//! WITNESS FAMILY `[ga10bprobe5g]` — 15 bytes bracketed; `LC_ALL=C grep -a -o -F` certifies the artifact.

use super::super::ga10b_probe::{clk, pg_state, r32, resolve_gpu_node, settle_ms, unreadable_reason, w32_4};
use super::{
    CMD_CLK_DISABLE, CMD_CLK_ENABLE, CMD_CLK_IS_ENABLED, CMD_PG_SET_STATE, FALCON_CPUCTL_OFF, FALCON_HWCFG2_OFF,
    FALCON_HWCFG_OFF, FALCON_MAILBOX0_OFF, GSP_FALCON_BASE, HWCFG2_PRIV_LOCKDOWN_BIT, MRQ_PG, PG_STATE_OFF,
    PG_STATE_ON,
};

const FAM: &str = "ga10bprobe5g";

// ── PUBLIC-RECALLED, NOT ACKED (module doc, PROVENANCE) ─────────────────────────────────────────────────
const FALCON_MAILBOX1_OFF: u64 = 0x044;
const FALCON_IMEMC0_OFF: u64 = 0x180;
const FALCON_IMEMD0_OFF: u64 = 0x184;
const FALCON_IMEMT0_OFF: u64 = 0x188;
const FALCON_DMEMC0_OFF: u64 = 0x1c0;
const FALCON_DMEMD0_OFF: u64 = 0x1c4;
/// `IMEMC/DMEMC` field layout: OFFS [7:2], BLK [23:8], AINCW bit 24, AINCR bit 25, (IMEMC) SECURE bit 28.
/// The CONTROL probe value is BLK=1 — an address, every bit of it a plain field, inside a 64 KiB memory.
const PORT_CTRL_PROBE: u32 = 0x0000_0100;
/// Word 0, no auto-increment: the data-port round-trip re-points the port before EVERY access, so no
/// AINC bit is ever relied on.
const PORT_CTRL_WORD0: u32 = 0x0000_0000;
/// The one word written through each data port and read back; restored to the as-found value after.
const PORT_PATTERN: u32 = 0x4A10_B5D0;
/// HWCFG size fields (nouveau `nvkm/falcon/base.c`): 256-byte blocks.
const HWCFG_IMEM_SIZE_MASK: u32 = 0x1ff;
const HWCFG_DMEM_SIZE_SHIFT: u32 = 9;

/// One announced read, result printed raw by this fn. Returns the raw value.
fn read_g(name: &str, addr: u64, off: u64) -> u32 {
    serial_println!("[{}] about-to-read {} reg={:#x} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it", FAM, name, addr);
    let v = r32(addr);
    match unreadable_reason(v) {
        Some(why) => serial_println!("[{}] {} @{:#x} = -UNREADABLE reason={} val={:#010x}", FAM, name, off, why, v),
        None => serial_println!("[{}] {} @{:#x} = {:#010x}", FAM, name, off, v),
    }
    v
}

/// One announced write + immediate readback (rung 4a's `bcr_write_verify` shape, this family): exactly one
/// result line per announce, so the announce/result counter balances. `Ok(held)` or `Err(reason)`.
fn write_g(name: &str, addr: u64, off: u64, val: u32, why: &str) -> Result<bool, &'static str> {
    serial_println!("[{}] about-to-WRITE {} reg={:#x} val={:#010x} ({}) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", FAM, name, addr, val, why);
    w32_4(addr, val);
    let got = r32(addr);
    match unreadable_reason(got) {
        Some(r) => {
            serial_println!("[{}] {} @{:#x} wrote={:#010x} read=-UNREADABLE reason={} val={:#010x} — NOT folded into held or not-held (F4)", FAM, name, off, val, r, got);
            Err(r)
        }
        None => {
            let held = got == val;
            serial_println!("[{}] {} @{:#x} wrote={:#010x} read={:#010x} held={}", FAM, name, off, val, got, held as u32);
            Ok(held)
        }
    }
}

/// A data-port write: announced, written, and its result line is the value written — the readback of a
/// DATA port is the next announced read through the re-pointed port, never the register itself.
fn write_data_g(name: &str, addr: u64, off: u64, val: u32, why: &str) {
    serial_println!("[{}] about-to-WRITE {} reg={:#x} val={:#010x} ({}) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", FAM, name, addr, val, why);
    w32_4(addr, val);
    serial_println!("[{}] {} @{:#x} wrote={:#010x} (data port: the readback is the next announced read through the re-pointed port)", FAM, name, off, val);
}

/// Classify a step-2 value for the verdict line.
fn class(v: u32) -> &'static str {
    match unreadable_reason(v) {
        Some(r) => r,
        None => "value",
    }
}

/// Control-register census: write BLK=1, read back, restore as found. Returns the arm word.
fn ctrl_census(name: &str, base: u64, off: u64, as_found: u32) -> &'static str {
    if unreadable_reason(as_found).is_some() {
        serial_println!("[{}] {} census SKIPPED — the register read as -UNREADABLE in the read census; no write to a register that does not answer", FAM, name);
        return "unreadable";
    }
    let addr = base + off;
    let held = match write_g(name, addr, off, PORT_CTRL_PROBE, "port CONTROL census: BLK=1, an address and nothing else") {
        Ok(h) => h,
        Err(_) => {
            let _ = write_g(name, addr, off, as_found, "restore as found after an unreadable readback");
            return "unreadable-after-write";
        }
    };
    let restored = matches!(write_g(name, addr, off, as_found, "restore as found"), Ok(true));
    if !held {
        "not-held"
    } else if !restored {
        "held-sticky"
    } else {
        "held"
    }
}

/// One-word data round-trip through a port whose control register HELD. Returns the arm word.
fn data_roundtrip(cname: &str, dname: &str, base: u64, coff: u64, doff: u64) -> &'static str {
    let (ca, da) = (base + coff, base + doff);
    let point = |why: &str| matches!(write_g(cname, ca, coff, PORT_CTRL_WORD0, why), Ok(true));
    if !point("point the port at word 0 before the as-found read") {
        return "ctrl-lost";
    }
    let found = read_g(dname, da, doff);
    if unreadable_reason(found).is_some() {
        return "data-unreadable";
    }
    if !point("point the port at word 0 before the pattern write") {
        return "ctrl-lost";
    }
    write_data_g(dname, da, doff, PORT_PATTERN, "the ONE-WORD pattern; restored below");
    if !point("point the port at word 0 before the pattern read") {
        return "ctrl-lost";
    }
    let got = read_g(dname, da, doff);
    if !point("point the port at word 0 before the restore write") {
        return "ctrl-lost";
    }
    write_data_g(dname, da, doff, found, "restore the as-found word");
    if !point("point the port at word 0 before the restore read") {
        return "ctrl-lost";
    }
    let back = read_g(dname, da, doff);
    serial_println!("[{}] {} round-trip: as_found={:#010x} wrote={:#010x} read={:#010x} restored_read={:#010x} held={} restored={}", FAM, dname, found, PORT_PATTERN, got, back, (got == PORT_PATTERN) as u32, (back == found) as u32);
    if unreadable_reason(got).is_some() {
        "data-unreadable"
    } else if got != PORT_PATTERN {
        "not-held"
    } else if back != found {
        "held-sticky"
    } else {
        "held"
    }
}

/// The `:: ga10b: 5g … ::` line, one emission point. `DEFERRED` = not measured, nothing spent.
fn verdict_line(fields: &str, outcome: &str) {
    serial_println!(":: ga10b: 5g port-census {} -> {} ::", fields, outcome);
}

/// RUNG 5g — the ENTRY, called from `ga10bprobe5_run` in place of the loader and the ignition. RETURNS on
/// every path; nothing it does spends the power cycle.
pub(super) fn rung5g_run(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    serial_println!("[ga10bprobe5g] rung 5g ARMED (UNAOS_GA10B_PROBE5=6) — THE GSP-ACR DOOR, STEP ONE: the IMEM/DMEM PORT CENSUS. Six BRFETCH-DMA ignitions answered 0x00000002 (rung-5 brief §9); the door the ladder never took loads the image through the falcon IMEM/DMEM PORTS before STARTCPU, and those registers have never been read on this die. This rung reads them under the engaged priv-lockdown, censuses the two CONTROL registers (write BLK=1, readback, restore) and round-trips ONE WORD through each DATA port (as-found read, pattern write, read, restore). NO vendor byte is loaded, NO BCR register is written, NO lock, NO STARTCPU: nothing is spent and the boot CONTINUES into the desktop. Vocabulary: PORT-OPEN | PORT-GATED | PORT-PARTIAL | PORT-CTRL-NOT-HELD | PORT-DATA-NOT-HELD | PORT-UNREADABLE | DEFERRED reason=<no-gpu-node|no-power-domains|pg-timeout|pg-on-refused|pg-readback-not-on>. The :: ga10b: 5g line is the verdict; PASS = both data ports round-trip, FAIL reason= names the measured negative");

    // ── 1. The bracket (rung 4a's, in this family) ─────────────────────────────────────────────────────
    let Some(gpu) = resolve_gpu_node(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!("[ga10bprobe5g] -> DEFERRED reason=no-gpu-node — the firmware DTB carries no usable gpu@ node; zero MMIO; RETURNING");
        verdict_line("measured=0", "DEFERRED reason=no-gpu-node");
        return;
    };
    serial_println!("[ga10bprobe5g] gpu@ node: BAR0={:#x} power-domain-id={} clocks={}", gpu.bar0, match gpu.pd_id { Some(id) => id as i64, None => -1 }, gpu.n_clocks);
    let Some(pd_id) = gpu.pd_id else {
        serial_println!("[ga10bprobe5g] -> DEFERRED reason=no-power-domains — gpu@ lists no power-domains id; a gated access is EL3-fatal (JX1); zero MMIO; RETURNING");
        verdict_line("measured=0", "DEFERRED reason=no-power-domains");
        return;
    };
    let Some(g) = super::super::fdt_tegra::bpmp_geometry(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!("[ga10bprobe5g] -> DEFERRED reason=pg-timeout — no BPMP geometry in the DTB; zero MMIO; RETURNING");
        verdict_line("measured=0", "DEFERRED reason=pg-timeout");
        return;
    };
    let Some(chan) = super::super::bpmp_tegra::chan_reopen(&g) else {
        serial_println!("[ga10bprobe5g] -> DEFERRED reason=pg-timeout — the BPMP channel could not be reopened from the DTB geometry; zero MMIO; RETURNING");
        verdict_line("measured=0", "DEFERRED reason=pg-timeout");
        return;
    };
    let chan = &chan;
    let pg_before = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe5g] pg-before id={} err={} state={:#x}", pd_id, err, st);
            if err == 0 { Some(st) } else { None }
        }
        None => {
            serial_println!("[ga10bprobe5g] -> DEFERRED reason=pg-timeout — MRQ_PG GET_STATE got no frame; zero MMIO; RETURNING");
            verdict_line("measured=0", "DEFERRED reason=pg-timeout");
            return;
        }
    };
    let mut we_powered = false;
    if pg_before != Some(PG_STATE_ON) {
        serial_println!("[ga10bprobe5g] BPMP MRQ_PG SET_STATE id={} state=ON — a BPMP request, not an MMIO write", pd_id);
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_ON]) {
            Some((err, _)) => {
                serial_println!("[ga10bprobe5g] pg-set-on id={} err={}", pd_id, err);
                we_powered = err == 0;
            }
            None => serial_println!("[ga10bprobe5g] pg-set-on id={} TIMEOUT", pd_id),
        }
        settle_ms(2);
    }
    let pg_now = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe5g] pg-readback id={} err={} state={:#x}", pd_id, err, st);
            if err == 0 { st } else { 0xffff_ffff }
        }
        None => {
            serial_println!("[ga10bprobe5g] pg-readback id={} TIMEOUT", pd_id);
            0xffff_ffff
        }
    };
    let mut enabled_by_us = [false; 8];
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        let before = match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe5g] clk {} IS_ENABLED (before) err={} = {}", id, err, st);
                if err == 0 { Some(st) } else { None }
            }
            None => None,
        };
        if before == Some(0) {
            serial_println!("[ga10bprobe5g] clk {} ENABLE — BPMP request; if this is the LAST line the transaction hung the boot", id);
            match clk(chan, CMD_CLK_ENABLE, id) {
                Some((err, _)) => {
                    serial_println!("[ga10bprobe5g] clk {} ENABLE err={}", id, err);
                    enabled_by_us[i] = err == 0;
                }
                None => serial_println!("[ga10bprobe5g] clk {} ENABLE TIMEOUT", id),
            }
        }
    }
    settle_ms(2);
    let restore = |chan: &super::super::bpmp_tegra::Chan| {
        let mut n_disabled = 0usize;
        for i in (0..gpu.n_clocks).rev() {
            if enabled_by_us[i] {
                let id = gpu.clocks[i];
                match clk(chan, CMD_CLK_DISABLE, id) {
                    Some((err, _)) => {
                        serial_println!("[ga10bprobe5g] clk {} DISABLE (restore) err={}", id, err);
                        if err == 0 { n_disabled += 1; }
                    }
                    None => serial_println!("[ga10bprobe5g] clk {} DISABLE (restore) TIMEOUT", id),
                }
            }
        }
        let mut pg_final = pg_now;
        if we_powered {
            match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_OFF]) {
                Some((err, _)) => serial_println!("[ga10bprobe5g] pg-set-off (restore) id={} err={}", pd_id, err),
                None => serial_println!("[ga10bprobe5g] pg-set-off (restore) id={} TIMEOUT", pd_id),
            }
            if let Some((_, st)) = pg_state(chan, pd_id) { pg_final = st; }
        }
        serial_println!("[ga10bprobe5g] restored: pg={:#x} (was {}) clocks-disabled={} of {} enabled here — the board is left as found; the boot continues", pg_final, match pg_before { Some(s) => s as i64, None => -1 }, n_disabled, enabled_by_us.iter().filter(|b| **b).count());
    };
    if pg_now != PG_STATE_ON {
        let why = if we_powered { "pg-readback-not-on" } else { "pg-on-refused" };
        serial_println!("[ga10bprobe5g] -> DEFERRED reason={} — the explicit readback did not say ON; a gated access is EL3-fatal (JX1): NOT ONE BAR0 register was touched; RETURNING", why);
        verdict_line("measured=0", if we_powered { "DEFERRED reason=pg-readback-not-on" } else { "DEFERRED reason=pg-on-refused" });
        restore(chan);
        return;
    }

    // ── 2. READ census of the GSP legacy-falcon block ──────────────────────────────────────────────────
    let base = gpu.bar0 + GSP_FALCON_BASE;
    serial_println!("[ga10bprobe5g] read census — address class gsp-falcon-v1 (BAR0+{:#x}; hwcfg/hwcfg2/mailbox0/cpuctl read on this die by rungs 4c/5a without fault; mailbox1 and the five PORT registers are FIRST TOUCH, PUBLIC-RECALLED offsets, read-only here)", GSP_FALCON_BASE);
    let hwcfg = read_g("gsp_falcon_hwcfg", base + FALCON_HWCFG_OFF, FALCON_HWCFG_OFF);
    let (imem_kib, dmem_kib) = if unreadable_reason(hwcfg).is_none() {
        ((hwcfg & HWCFG_IMEM_SIZE_MASK) >> 2, ((hwcfg >> HWCFG_DMEM_SIZE_SHIFT) & HWCFG_IMEM_SIZE_MASK) >> 2)
    } else {
        (0, 0)
    };
    serial_println!("[ga10bprobe5g] hwcfg decode (PUBLIC-RECALLED field layout, 256-byte blocks): imem_kib={} dmem_kib={} raw={:#010x} — acr-gsp.text is 28,672 B and .data 9,472 B (A61), so the triple FITS a port-load if the ports are open", imem_kib, dmem_kib, hwcfg);
    let hwcfg2 = read_g("gsp_falcon_hwcfg2", base + FALCON_HWCFG2_OFF, FALCON_HWCFG2_OFF);
    let lockdown = match unreadable_reason(hwcfg2) { Some(_) => 1, None => (hwcfg2 >> HWCFG2_PRIV_LOCKDOWN_BIT) & 1 };
    serial_println!("[ga10bprobe5g] hwcfg2 lockdown={} (raw={:#010x}) — the priv-lockdown every flight has read engaged; this census asks what it REACHES", lockdown, hwcfg2);
    let mbox0 = read_g("gsp_falcon_mailbox0", base + FALCON_MAILBOX0_OFF, FALCON_MAILBOX0_OFF);
    let cpuctl = read_g("gsp_falcon_cpuctl_v1", base + FALCON_CPUCTL_OFF, FALCON_CPUCTL_OFF);
    serial_println!("[ga10bprobe5g] lockdown control: cpuctl_v1={} (render15 read pri-error here on every flight — a VALUE now would itself be a finding)", class(cpuctl));
    let mbox1 = read_g("gsp_falcon_mailbox1", base + FALCON_MAILBOX1_OFF, FALCON_MAILBOX1_OFF);
    let imemc = read_g("gsp_falcon_imemc0", base + FALCON_IMEMC0_OFF, FALCON_IMEMC0_OFF);
    let imemd = read_g("gsp_falcon_imemd0", base + FALCON_IMEMD0_OFF, FALCON_IMEMD0_OFF);
    let imemt = read_g("gsp_falcon_imemt0", base + FALCON_IMEMT0_OFF, FALCON_IMEMT0_OFF);
    let dmemc = read_g("gsp_falcon_dmemc0", base + FALCON_DMEMC0_OFF, FALCON_DMEMC0_OFF);
    let dmemd = read_g("gsp_falcon_dmemd0", base + FALCON_DMEMD0_OFF, FALCON_DMEMD0_OFF);
    serial_println!("[ga10bprobe5g] read census: hwcfg={} hwcfg2={} mailbox0={} mailbox1={} cpuctl_v1={} imemc0={} imemd0={} imemt0={} dmemc0={} dmemd0={} -> READCENSUS-DONE", class(hwcfg), class(hwcfg2), class(mbox0), class(mbox1), class(cpuctl), class(imemc), class(imemd), class(imemt), class(dmemc), class(dmemd));

    // ── 3. CONTROL census + 4. DATA round-trip ─────────────────────────────────────────────────────────
    let imemc_arm = ctrl_census("gsp_falcon_imemc0", base, FALCON_IMEMC0_OFF, imemc);
    let dmemc_arm = ctrl_census("gsp_falcon_dmemc0", base, FALCON_DMEMC0_OFF, dmemc);
    let imemd_arm = if imemc_arm == "held" {
        data_roundtrip("gsp_falcon_imemc0", "gsp_falcon_imemd0", base, FALCON_IMEMC0_OFF, FALCON_IMEMD0_OFF)
    } else {
        serial_println!("[ga10bprobe5g] imemd0 round-trip SKIPPED — imemc0 census {}; a data port whose control register does not hold is not driven", imemc_arm);
        "skipped"
    };
    let dmemd_arm = if dmemc_arm == "held" {
        data_roundtrip("gsp_falcon_dmemc0", "gsp_falcon_dmemd0", base, FALCON_DMEMC0_OFF, FALCON_DMEMD0_OFF)
    } else {
        serial_println!("[ga10bprobe5g] dmemd0 round-trip SKIPPED — dmemc0 census {}; a data port whose control register does not hold is not driven", dmemc_arm);
        "skipped"
    };

    // ── 5. The verdict ─────────────────────────────────────────────────────────────────────────────────
    let port_unread = |v: u32| unreadable_reason(v).is_some();
    let all_ports_unread = port_unread(imemc) && port_unread(imemd) && port_unread(imemt) && port_unread(dmemc) && port_unread(dmemd);
    let imem_open = imemd_arm == "held";
    let dmem_open = dmemd_arm == "held";
    let (arm, outcome) = if port_unread(hwcfg) {
        ("PORT-UNREADABLE", "FAIL reason=port-unreadable")
    } else if all_ports_unread {
        ("PORT-GATED", "FAIL reason=port-gated")
    } else if imem_open && dmem_open {
        ("PORT-OPEN", "PASS")
    } else if imem_open || dmem_open {
        ("PORT-PARTIAL", "FAIL reason=port-partial")
    } else if imemc_arm != "held" && dmemc_arm != "held" {
        ("PORT-CTRL-NOT-HELD", "FAIL reason=port-ctrl-not-held")
    } else {
        ("PORT-DATA-NOT-HELD", "FAIL reason=port-data-not-held")
    };
    serial_println!("[ga10bprobe5g] port census: lockdown={} hwcfg={:#010x} imem_kib={} dmem_kib={} cpuctl_v1={} mailbox1={} imemc={} dmemc={} imemd={} dmemd={} imemt={} -> {}", lockdown, hwcfg, imem_kib, dmem_kib, class(cpuctl), class(mbox1), imemc_arm, dmemc_arm, imemd_arm, dmemd_arm, class(imemt), arm);
    match arm {
        "PORT-OPEN" => serial_println!("[ga10bprobe5g] the door's write class is OPEN under lockdown={}: both ports accept and hold a word. The next rung is the GSP-ACR door itself — port-load the acr-gsp triple (text -> IMEM, data -> DMEM), the manifest where the write list says, bcr_ctrl 0x11, STARTCPU — and it is a candidate only once that write list is READ from nvgpu (rung-5 brief §9.5 item 1); build nothing past this line without it", lockdown),
        "PORT-GATED" => serial_println!("[ga10bprobe5g] every port register answers pri-error while hwcfg/hwcfg2 answer values: priv-lockdown reaches the IMEM/DMEM ports. The GSP-ACR door is CLOSED at this altitude by measurement — the same wall as cpuctl, read on the door's own write class, which no 0x2 could say (R19: failed under lockdown=1, EL2, this die)"),
        "PORT-UNREADABLE" => serial_println!("[ga10bprobe5g] hwcfg itself did not answer: the block is not the one rungs 4c/5a read (a bracket or aperture difference), not a port finding; re-fly from cold before reading anything into it"),
        _ => serial_println!("[ga10bprobe5g] a mixed answer: the per-register fields above are the finding; a control register that holds while its data port does not, or one port open and the other gated, each names the next read — record it, do not iterate"),
    }
    serial_println!(":: ga10b: 5g port-census lockdown={} imemc={} dmemc={} imemd={} dmemd={} imem_kib={} dmem_kib={} -> {} ::", lockdown, imemc_arm, dmemc_arm, imemd_arm, dmemd_arm, imem_kib, dmem_kib, outcome);
    restore(chan);
}
