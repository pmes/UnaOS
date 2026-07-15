// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-SMP-2 — the JM5 `CPU_ON` firmware-wall INVESTIGATION probe (tegra + `smpprobe` gated).
//
// JM5 proved the PSCI/GICv3 SMP mechanism correct on QEMU `virt` (3/3 secondaries), but the FIRST
// `CPU_ON` on real Orin silicon triggers a fatal Tegra CBB-fabric RAS Uncorrectable Error inside
// BL31/MCE and powers the box off, while every PSCI *query* (`AFFINITY_INFO`) works. See
// `arch_arm64.md` §JM5-result — its ranked hypotheses are this module's charter.
//
// This module adds a boot-time, serial-recorded probe selected by `UNAOS_SMPPROBE=<n>` (one
// experiment per boot), wired into `tegra_early_stop` after the JM4 GIC/timer/heap bring-up and
// BEFORE the JM6 EL2->EL1 drop. It is the MINIMAL delta that discriminates each hypothesis.
//
// ## Safety model (RIDER (b) — probe-only)
//
// Every experiment is READ / QUERY / one-shot-VOLATILE only. NONE writes fuses, BCT/EEPROM, UEFI
// variables, MB1/MB2 storage, or any persistent MCE/firmware config. The `CPU_ON`-issuing
// experiments (3, 5) command a volatile core-power action — exactly what JetPack's OS does every
// boot — and write no persistent state; a power-fault boot is DATA, not a failure. Hypothesis H4
// (caller-EL) is recorded BLOCKED (see `exp4`): its discrimination cannot be reproduced from our
// minimal EL2 kernel without JetPack's boot-time ATF handshake.
//
// ## Selection & the string-count invariant
//
// `SEL` is a compile-time const parsed from `option_env!("UNAOS_SMPPROBE")` (default 0). Each armed
// value is therefore a distinct kernel image (the operator rebuilds+reflashes per boot — the
// runbook's A-B-A schedule). Every record echoes the LIVE `SEL` + experiment name so the operator
// can VERIFY on serial which experiment actually ran before trusting the boot. The dispatch keeps
// EVERY experiment fn address-taken (a `static` table + `black_box`), so the compiled `tegra:`
// string set — and thus the `strings | grep -c 'tegra:'` count — is the SAME for any armed value.
//
// The whole module is `#[cfg(all(feature = "tegra", feature = "smpprobe"))]`. With `smpprobe` OFF
// (the default), nothing here is compiled and the tegra image is byte-identical to baseline.

use super::gic;

/// PSCI (Arm DEN0022) SMCCC fast-call function IDs used by the probes. All 64-bit (`0xC4…`) except
/// the 32-bit query calls (`0x84…`) which take/return only 32-bit values.
const PSCI_VERSION: u64 = 0x8400_0000;
const PSCI_CPU_ON: u64 = 0xC400_0003;
const PSCI_AFFINITY_INFO: u64 = 0xC400_0004;
const PSCI_MIGRATE_INFO_TYPE: u64 = 0x8400_0006;
const PSCI_FEATURES: u64 = 0x8400_000A;
const PSCI_SYSTEM_OFF: u64 = 0x8400_0008; // queried by FEATURES only — never invoked

/// A LOW in-DRAM sentinel entry PA for the H3 (entry-point-high) discriminator: the base of the
/// lowest firmware-mapped DRAM GiB (2 GiB), far below the kernel's ~9.5 GiB load PA. No stub is
/// written there (RIDER (b)); the hypothesis says BL31 faults in the core-power path BEFORE the
/// woken core fetches, so the entry PA should be irrelevant. Same RAS ⇒ H3 refuted; any divergence
/// ⇒ H3 warrants a proper low-trampoline follow-up.
const LOW_ENTRY_SENTINEL: u64 = 0x8000_0000;

/// The experiment selected at compile time from `UNAOS_SMPPROBE` (default 0 = the safe control).
pub const SEL: u32 = match option_env!("UNAOS_SMPPROBE") {
    Some(s) => parse_u32(s),
    None => 0,
};

/// `const fn` decimal parse (no `str::parse` in const context). Unparseable → 0 (the safe control).
const fn parse_u32(s: &str) -> u32 {
    let b = s.as_bytes();
    let mut i = 0;
    let mut n: u32 = 0;
    while i < b.len() {
        let c = b[i];
        if c < b'0' || c > b'9' {
            return 0;
        }
        n = n * 10 + (c - b'0') as u32;
        i += 1;
    }
    n
}

/// A parking loop a woken core lands on so a SUCCESSFUL `CPU_ON` (H3 candidate on metal, or any
/// non-fault path) does not run off into undefined code. Identity-mapped on tegra (VA==PA), so its
/// symbol address IS the high (~9.5 GiB) entry PA the H3 hypothesis is about. MMU off on entry;
/// `wfe` needs no translation. Self-contained — touches no captured context.
core::arch::global_asm!(
    r#"
    .globl _smpprobe_park
    _smpprobe_park:
    1:  wfe
        b   1b
    "#
);
unsafe extern "C" {
    fn _smpprobe_park();
}

/// One PSCI/SMCCC fast call via the SMC conduit (Orin's ATF/BL31 monitor at EL3 services it). x0-x17
/// are volatile per SMCCC; we clobber x1-x17 and read x0. No `nomem`: a `CPU_ON` has global side
/// effects that must not be reordered around the record prints.
fn smc(func: u64, x1: u64, x2: u64, x3: u64) -> i64 {
    let mut x0 = func;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") x0,
            inout("x1") x1 => _,
            inout("x2") x2 => _,
            inout("x3") x3 => _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nostack),
        );
    }
    x0 as i64
}

/// Packed GICR/MPIDR-contiguous affinity {Aff3[31:24],Aff2[23:16],Aff1[15:8],Aff0[7:0]} -> the
/// MPIDR/PSCI target layout (Aff3 -> bits[39:32]). Identity for Aff3=0 (all of Tegra234). Mirrors
/// `smp_virt::affinity_to_mpidr` (kept local — that one is private).
#[inline]
fn affinity_to_mpidr(packed: u32) -> u64 {
    let p = packed as u64;
    (p & 0x00FF_FFFF) | (((p >> 24) & 0xFF) << 32)
}

/// Decode an `AFFINITY_INFO` return into a short tag for the record grammar.
fn info_tag(info: i64) -> &'static str {
    match info {
        0 => "ON",
        1 => "OFF",
        2 => "ON_PENDING",
        _ => "absent(-INVALID_PARAMS)",
    }
}

/// The BSP's own affinity + the first present, non-BSP secondary affinity (via the redistributor
/// walk + an `AFFINITY_INFO` presence gate), or `None` if none is present. Used by the `CPU_ON`
/// experiments so they never target a fuse-disabled phantom (JM5 attempt-1 RAS lesson).
fn first_present_secondary() -> (u32, Option<u32>) {
    let bsp = gic::this_affinity();
    let mut frames = [0u32; 8];
    let n = gic::enumerate_redistributor_affinities(&mut frames);
    for &a in frames.iter().take(n) {
        if a == bsp {
            continue;
        }
        let info = smc(PSCI_AFFINITY_INFO, affinity_to_mpidr(a), 0, 0);
        serial_println!(
            ":: tegra: SMPPROBE sel={} enum aff={:#010x} AFFINITY_INFO={} -> {} ::",
            SEL,
            a,
            info,
            info_tag(info)
        );
        if info >= 0 {
            return (bsp, Some(a));
        }
    }
    (bsp, None)
}

// ── Experiment 0 — CONTROL: AFFINITY_INFO topology sweep (no CPU_ON) ─────────────────────────────
// Sweeps the Tegra234 affinity space (Aff2 = cluster 0..3, Aff1 = core-in-cluster 0..3, Aff0 = 0)
// with the pure-query `AFFINITY_INFO`, and dumps the redistributor walk. Never touches core power,
// so it is the clean-boot control that falls through to CAPSTONE.
fn exp0_affinity_sweep(_: &ProbeCtx) {
    serial_println!(":: tegra: SMPPROBE sel=0 exp=affinity-sweep BEGIN (control; no CPU_ON) ::");
    let bsp = gic::this_affinity();
    let mut frames = [0u32; 8];
    let n = gic::enumerate_redistributor_affinities(&mut frames);
    serial_println!(":: tegra: SMPPROBE sel=0 redistributor-walk found {} frame(s) BSP={:#010x} ::", n, bsp);
    for i in 0..n {
        serial_println!(":: tegra: SMPPROBE sel=0 frame[{}] aff={:#010x} ::", i, frames[i]);
    }
    let mut present = 0u32;
    for aff2 in 0u32..4 {
        for aff1 in 0u32..4 {
            let packed = (aff2 << 16) | (aff1 << 8);
            let info = smc(PSCI_AFFINITY_INFO, affinity_to_mpidr(packed), 0, 0);
            if info >= 0 {
                present += 1;
            }
            serial_println!(
                ":: tegra: SMPPROBE sel=0 slot aff={:#010x} (Aff2={} Aff1={}) AFFINITY_INFO={} -> {} ::",
                packed, aff2, aff1, info, info_tag(info)
            );
        }
    }
    serial_println!(":: tegra: SMPPROBE sel=0 exp=affinity-sweep END present={} (no CPU_ON; boot continues) ::", present);
}

// ── Experiment 1 — H1: PSCI capability census (no CPU_ON) ────────────────────────────────────────
// H1: Tegra `CPU_ON` needs MCE/BPMP coordination a generic PSCI call doesn't provide. Discriminator:
// ask BL31 what it advertises. If CPU_ON is advertised supported (FEATURES >= 0) yet still faults,
// the failure is inside Tegra's CPU_ON implementation (consistent with the MCE-coordination story),
// not an unrecognized call. MIGRATE_INFO_TYPE reveals the OP-TEE TOS presence (the SMC handshake
// context). All pure queries — clean boot.
fn exp1_psci_census(_: &ProbeCtx) {
    serial_println!(":: tegra: SMPPROBE sel=1 exp=psci-census BEGIN ::");
    let v = smc(PSCI_VERSION, 0, 0, 0);
    serial_println!(
        ":: tegra: SMPPROBE sel=1 PSCI_VERSION={} (maj={} min={}) ::",
        v, (v >> 16) & 0xFFFF, v & 0xFFFF
    );
    let f_cpu_on = smc(PSCI_FEATURES, PSCI_CPU_ON, 0, 0);
    serial_println!(":: tegra: SMPPROBE sel=1 FEATURES(CPU_ON)={} ({}) ::", f_cpu_on, if f_cpu_on >= 0 { "advertised" } else { "NOT-supported" });
    let f_aff = smc(PSCI_FEATURES, PSCI_AFFINITY_INFO, 0, 0);
    serial_println!(":: tegra: SMPPROBE sel=1 FEATURES(AFFINITY_INFO)={} ({}) ::", f_aff, if f_aff >= 0 { "advertised" } else { "NOT-supported" });
    let f_off = smc(PSCI_FEATURES, PSCI_SYSTEM_OFF, 0, 0);
    serial_println!(":: tegra: SMPPROBE sel=1 FEATURES(SYSTEM_OFF)={} ({}) ::", f_off, if f_off >= 0 { "advertised" } else { "NOT-supported" });
    let mit = smc(PSCI_MIGRATE_INFO_TYPE, 0, 0, 0);
    serial_println!(
        ":: tegra: SMPPROBE sel=1 MIGRATE_INFO_TYPE={} ({}) ::",
        mit,
        match mit { 0 => "TOS-migrate-capable", 1 => "TOS-not-migrate-capable", 2 => "no-TOS", _ => "err" }
    );
    serial_println!(":: tegra: SMPPROBE sel=1 exp=psci-census END (no CPU_ON; boot continues) ::");
}

// ── Experiment 2 — H2: latent RAS error-record read (no CPU_ON) ──────────────────────────────────
// H2: a latent/poisoned RAS condition is surfaced by the first SMC->EL3 barrier (the fault lands
// exactly at the first CPU_ON; ADDR differs run-to-run). Discriminator: read the Armv8 RAS error
// records BEFORE any CPU_ON. A pre-existing valid/uncorrectable record supports H2; all-clean
// weakens it. ID-gated on `ID_AA64PFR0_EL1.RAS` (a non-trapping ID read) so we never touch ERR*
// registers on a part that doesn't implement them. At EL2 the ERR* accesses are not trapped by
// `HCR_EL2.TERR` (which only traps EL1). Pure reads — clean boot.
fn exp2_ras_latent(_: &ProbeCtx) {
    serial_println!(":: tegra: SMPPROBE sel=2 exp=ras-latent BEGIN ::");
    let pfr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ID_AA64PFR0_EL1", out(reg) pfr0, options(nomem, nostack, preserves_flags));
    }
    let ras = (pfr0 >> 28) & 0xF;
    serial_println!(":: tegra: SMPPROBE sel=2 ID_AA64PFR0.RAS={} ({}) ::", ras, if ras == 0 { "not-implemented" } else { "implemented" });
    if ras == 0 {
        serial_println!(":: tegra: SMPPROBE sel=2 exp=ras-latent END (RAS ext absent; ERR* skipped; boot continues) ::");
        return;
    }
    let erridr: u64;
    unsafe {
        core::arch::asm!("mrs {}, S3_0_C5_C3_0", out(reg) erridr, options(nomem, nostack, preserves_flags)); // ERRIDR_EL1
    }
    let num = (erridr & 0xFFFF) as u64;
    serial_println!(":: tegra: SMPPROBE sel=2 ERRIDR.NUM={} error-record(s) ::", num);
    let cap = if num > 16 { 16 } else { num };
    let mut r = 0u64;
    while r < cap {
        let status: u64;
        let addr: u64;
        let misc0: u64;
        unsafe {
            core::arch::asm!("msr S3_0_C5_C3_1, {}", in(reg) r, options(nomem, nostack, preserves_flags)); // ERRSELR_EL1
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, S3_0_C5_C4_2", out(reg) status, options(nomem, nostack, preserves_flags)); // ERXSTATUS_EL1
            core::arch::asm!("mrs {}, S3_0_C5_C4_3", out(reg) addr, options(nomem, nostack, preserves_flags)); // ERXADDR_EL1
            core::arch::asm!("mrs {}, S3_0_C5_C5_0", out(reg) misc0, options(nomem, nostack, preserves_flags)); // ERXMISC0_EL1
        }
        let v = (status >> 30) & 1; // Valid
        let ue = (status >> 29) & 1; // Uncorrectable Error
        serial_println!(
            ":: tegra: SMPPROBE sel=2 record={} ERXSTATUS={:#018x} V={} UE={} ERXADDR={:#018x} ERXMISC0={:#018x} ::",
            r, status, v, ue, addr, misc0
        );
        r += 1;
    }
    serial_println!(":: tegra: SMPPROBE sel=2 exp=ras-latent END (no CPU_ON; boot continues) ::");
}

// ── Experiment 3 — H3: entry-point-high discriminator (CPU_ON, LOW sentinel entry) ───────────────
// H3: the kernel's ~9.5 GiB entry PA is rejected by BL31's reset-vector programming. Discriminator:
// issue ONE CPU_ON to the first present secondary with a LOW entry PA (2 GiB sentinel). Everything
// else is identical to exp5 (the high-entry reproduction) — the entry PA is the ONLY delta. Same RAS
// fault + power-off as exp5 ⇒ the fault precedes the fetch (H1/H2), H3 refuted. A returned ret /
// survival ⇒ H3 candidate. Writes nothing to the sentinel PA (RIDER (b)).
fn exp3_entry_pa_low(_: &ProbeCtx) {
    serial_println!(":: tegra: SMPPROBE sel=3 exp=entry-pa-low BEGIN ::");
    let (_bsp, target) = first_present_secondary();
    match target {
        None => serial_println!(":: tegra: SMPPROBE sel=3 no present secondary; SKIP (no CPU_ON) END ::"),
        Some(aff) => {
            serial_println!(
                ":: tegra: SMPPROBE sel=3 target aff={:#010x} entry={:#x} (LOW sentinel) — issuing CPU_ON (expect RAS+power-off if H3 false) ::",
                aff, LOW_ENTRY_SENTINEL
            );
            let ret = smc(PSCI_CPU_ON, affinity_to_mpidr(aff), LOW_ENTRY_SENTINEL, 0xB3);
            serial_println!(
                ":: tegra: SMPPROBE sel=3 CPU_ON RETURNED ret={} — SURVIVED (H3 candidate: low entry avoided the fault) END ::",
                ret
            );
        }
    }
}

// ── Experiment 4 — H4: caller-EL — BLOCKED-BY-DESIGN ─────────────────────────────────────────────
// H4: JetPack calls PSCI from EL1 with a fuller ATF handshake; we run at NS-EL2. This cannot be
// discriminated from our minimal kernel: an SMC from NS-EL1 vs NS-EL2 reaches the SAME BL31 SMC
// handler identically (the conduit is SMC regardless of caller EL), and JetPack's difference is its
// BOOT-TIME ATF/BL31/OP-TEE handshake — set up while JetPack boots, not reproducible by flipping our
// runtime caller EL. Recorded BLOCKED; no CPU_ON; clean boot.
fn exp4_caller_el_blocked(_: &ProbeCtx) {
    serial_println!(":: tegra: SMPPROBE sel=4 exp=el1-caller BLOCKED-BY-DESIGN ::");
    serial_println!(
        ":: tegra: SMPPROBE sel=4 reason=SMC from NS-EL1 vs NS-EL2 hits the same BL31 handler; JetPack's difference is its boot-time ATF handshake, not the runtime caller EL — not reproducible from our EL2 kernel END ::"
    );
}

// ── Experiment 5 — H3 reference / baseline reproduction (CPU_ON, HIGH kernel entry) ──────────────
// The isolated re-confirmation of the JM5 wall: ONE CPU_ON to the first present secondary at the
// HIGH (~9.5 GiB, identity-mapped kernel) entry PA of `_smpprobe_park`. Unlike JM5 (which looped all
// cores) this targets exactly one, with a full pre-dump, so the record survives even as the box
// powers off. Predicted: the RAS Uncorrectable Error + power-off. This is the entry-PA-high partner
// of exp3 (same code, high entry) and the A-leg reference the A-B-A schedule brackets.
fn exp5_entry_pa_high(_: &ProbeCtx) {
    serial_println!(":: tegra: SMPPROBE sel=5 exp=entry-pa-high BEGIN (baseline JM5-wall reproduction) ::");
    let entry = _smpprobe_park as *const () as usize as u64;
    let (_bsp, target) = first_present_secondary();
    match target {
        None => serial_println!(":: tegra: SMPPROBE sel=5 no present secondary; SKIP (no CPU_ON) END ::"),
        Some(aff) => {
            serial_println!(
                ":: tegra: SMPPROBE sel=5 target aff={:#010x} entry={:#x} (HIGH kernel PA) — issuing CPU_ON (expect RAS+power-off) ::",
                aff, entry
            );
            let ret = smc(PSCI_CPU_ON, affinity_to_mpidr(aff), entry, 0xB5);
            serial_println!(
                ":: tegra: SMPPROBE sel=5 CPU_ON RETURNED ret={} — SURVIVED (wall NOT reproduced this boot) END ::",
                ret
            );
        }
    }
}

/// Context handed to each experiment (the firmware DTB span, for any future DTB-reading probe).
pub struct ProbeCtx {
    pub dtb_addr: u64,
    pub dtb_size: usize,
}

type ProbeFn = fn(&ProbeCtx);

/// The experiment table. Every fn is address-taken here, so the linker retains ALL of them (and
/// their `tegra:` record strings) regardless of the compile-time `SEL` — the string-count invariant
/// that keeps `strings | grep -c 'tegra:'` stable across armed values.
static EXPERIMENTS: &[(u32, &str, ProbeFn)] = &[
    (0, "affinity-sweep", exp0_affinity_sweep),
    (1, "psci-census", exp1_psci_census),
    (2, "ras-latent", exp2_ras_latent),
    (3, "entry-pa-low", exp3_entry_pa_low),
    (4, "el1-caller", exp4_caller_el_blocked),
    (5, "entry-pa-high", exp5_entry_pa_high),
];

/// Entry point, called from `tegra_early_stop` after JM4 (GIC/timer/heap up) and before the JM6
/// drop. Dispatches the compile-time-selected experiment; clean (non-`CPU_ON`) experiments return
/// and the boot proceeds to CAPSTONE, so `smpprobe`-on boots for sel 0/1/2/4 are still full boots.
pub fn run(ctx: &ProbeCtx) {
    // `black_box(SEL)` blocks the optimizer from proving only one table entry is reachable and
    // pruning the others (which would drop their strings and perturb the count).
    let sel = core::hint::black_box(SEL);
    serial_println!(
        ":: tegra: SMPPROBE ARMED sel={} — probe-only investigation (see arch_arm64 §ORIN-SMP-2); power-fault boots are DATA ::",
        sel
    );
    for &(v, name, f) in EXPERIMENTS {
        if v == sel {
            serial_println!(":: tegra: SMPPROBE dispatch sel={} exp={} ::", sel, name);
            f(ctx);
            serial_println!(":: tegra: SMPPROBE sel={} exp={} DONE ::", sel, name);
            return;
        }
    }
    serial_println!(":: tegra: SMPPROBE sel={} UNKNOWN (no experiment); boot continues ::", sel);
}
