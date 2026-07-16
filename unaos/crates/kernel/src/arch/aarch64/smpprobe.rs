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

use super::{cache, exceptions, gic, percpu, timer};

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

/// The tegra UART base (UARTC, mirrors `serial::tegra::BASE` — kept local so the SMP-5 leg-17 BSP line
/// can NAME the exact MMIO address the woken core's `serial_println!` will drive, BEFORE any `CPU_ON`,
/// without touching `serial.rs`). Documentation-only: the AP print itself goes through the real
/// `serial_println!` path (no new UART code); this const is never used to drive the UART.
const TEGRA_UART_BASE_DOC: u64 = 0x0C28_0000;

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

/// Context handed to each experiment (the firmware DTB span + the mapped-RAM mask). `ram_gib_mask` is
/// the ORIN-SMP-4 addition: `fdt_tegra::cpu_affinities` needs it to bound-check the DTB blob deref
/// before walking `/cpus` (the bisect legs source their single target from `/cpus`, RIDER 5).
pub struct ProbeCtx {
    pub dtb_addr: u64,
    pub dtb_size: usize,
    pub ram_gib_mask: u64,
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
    // ORIN-SMP-4 bisect leg family (values 10..16): the woken-core EXECUTION bisect that brackets the
    // SMP-3 RAS fault (see §ORIN-SMP-4). Dispatched separately from the SMP-2 experiment table because
    // each leg wakes ONE core into a MINIMAL entry adding exactly one variable over the previous leg.
    // ORIN-SMP-5 extends it with the RESIDUE legs (17..20) — what leg 16's replica omitted vs the real
    // flow: 17 = +AP serial print, 18 = +WFI tail, 19 = leg-16 shape on a cluster-1 core, 20 = the real
    // 5-core wake sequence (see §ORIN-SMP-5). Legs 17..19 reuse the single-target bisect driver; leg 20
    // has its own sequence driver.
    if (10..=19).contains(&sel) {
        run_bisect_leg(ctx, sel);
        return;
    }
    if sel == 20 {
        run_5core_sequence(ctx);
        return;
    }
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// ORIN-SMP-4 — the woken core's EXECUTION BISECT (UNAOS_SMPPROBE = 10..16)
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// SMP-3 proved firmware `CPU_ON` works on UEFI 39.2.0 (the SMP-2 exp5 park survived, ret=0) but waking
// the SAME core (aff 0x00000100) into the real `smp_virt::_secondary_start_virt` RAS-faults ×2 (IOB
// SERR=0x12 / CBB-0x6 / ADDR 0x8000000000000200) BEFORE the BSP prints the CPU_ON result — i.e. the
// fault is driven by the woken core's EARLY EXECUTION. This family brackets that: each leg wakes ONE
// `/cpus`-named core into a MINIMAL entry that adds exactly ONE variable over the previous leg, then
// parks. The woken core NEVER prints (its UART is unarbitrated — the pi core3probe lesson); instead it
// raises a per-leg CHECKPOINT flag (a plain MMU-off-safe store + `DC CVAC`, the spin-table-slot idiom)
// that the BSP polls, so a SILENT park is distinguishable from a faulted/reset core. All evidence is
// BSP-side serial. Probe-only (RIDER 4): the woken core touches ONLY its own stack, the SEC_CTX-named
// regime registers, its own GICR frame (leg 15, one read), and the checkpoint — no persistent state.
//
// The bisect is SELF-CONTAINED here (its own stack / captured regime / checkpoint / entry stubs), so
// the working `smp_virt.rs` SMP-3 path stays BYTE-UNTOUCHED — the diagnostic cannot perturb the code
// it is measuring. Leg 16 replicates the tail of `__secondary_rust_virt` (percpu + GICv3 secondary
// bring-up + the SGI ping) from the same public building blocks rather than calling the real entry, to
// keep that byte-identity; it runs LAST and only if 10..15 all survived (see the runbook).

/// One woken core's stack for the bisect legs (64 KiB, 16-aligned, BSS). Only one core is woken per
/// leg, so a single slot suffices — the context id is ignored for stack selection.
const PROBE_STACK_SIZE: usize = 0x1_0000; // 64 KiB
#[repr(C, align(16))]
struct ProbeStack([u8; PROBE_STACK_SIZE]);
static mut PROBE_STACK: ProbeStack = ProbeStack([0; PROBE_STACK_SIZE]);

/// The BSP's live EL2 regime the legs 12+ replay — a private mirror of `smp_virt::SecondaryCtx` (kept
/// local so `smp_virt.rs` is untouched). Read by the woken core with its MMU off (non-cacheable,
/// straight from PoC — hence the BSP cleans it), so plain `u64` fields, no locks. `align(64)` keeps it
/// in one cache line so a single `clean_range` publishes it.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct ProbeRegs {
    mair: u64,
    tcr: u64,
    ttbr0: u64,
    sctlr: u64,
    cptr: u64,
    hcr: u64,
}
static mut PROBE_REGS: ProbeRegs = ProbeRegs { mair: 0, tcr: 0, ttbr0: 0, sctlr: 0, cptr: 0, hcr: 0 };

/// The woken-core checkpoint flag (spin-table-slot idiom). The leg stub stores `CP_TAG | leg` with a
/// plain store + `DC CVAC` (MMU-off-safe: with the MMU off the store is non-cacheable straight to PoC,
/// with the MMU on the CVAC pushes the cacheable line to PoC — both leave it visible to the BSP after
/// an invalidate). `align(64)` isolates the line so the BSP's `DC IVAC` poll cannot disturb a neighbour.
#[repr(C, align(64))]
struct Checkpoint(u64);
static mut CHECKPOINT: Checkpoint = Checkpoint(0);

/// High half of the checkpoint value (`0x5304` = "SMP-4"); low half = the leg number. A distinct,
/// non-zero tag so a coincidental stale/zero line is never mistaken for a reached checkpoint.
const CP_TAG: u64 = 0x5304_0000;
#[inline]
fn cp_value(leg: u32) -> u64 {
    CP_TAG | leg as u64
}

/// The linear index / PSCI context id handed to the single woken core (advisory — the legs use one
/// stack and, for leg 16, one fixed per-CPU slot). Index 1 (never the BSP's 0).
const PROBE_CORE_INDEX: usize = 1;

/// SGI 0 — the leg-16 AP→BSP proof ping (the same INTID `smp_virt` reserves). Only leg 16 uses it.
const PROBE_IPI_SGI: u32 = 0;

/// The BSP's packed affinity, published before `CPU_ON` so leg 16's woken core can target it for the
/// AP→BSP ping. Written by the BSP (plain, then cleaned to PoC with the regime), read MMU-on by leg 16.
static mut PROBE_BSP_AFF: u32 = 0;

/// Capture the BSP's live EL2 regime into `PROBE_REGS` (the legs 12+ replay it). Read straight from the
/// running BSP's system registers — whatever the firmware/JM3 configured, carried faithfully.
fn capture_probe_regs() {
    let (mair, tcr, ttbr0, sctlr, cptr, hcr): (u64, u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, MAIR_EL2", out(reg) mair, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, TCR_EL2", out(reg) tcr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, TTBR0_EL2", out(reg) ttbr0, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, SCTLR_EL2", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CPTR_EL2", out(reg) cptr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, HCR_EL2", out(reg) hcr, options(nomem, nostack, preserves_flags));
        core::ptr::write_volatile(
            &raw mut PROBE_REGS,
            ProbeRegs { mair, tcr, ttbr0, sctlr, cptr, hcr },
        );
    }
}

/// Raise the checkpoint from Rust (legs 14..16, MMU on): plain store + clean-to-PoC. The store must be
/// visible to the BSP's invalidate-then-read poll, so `clean_range` (`DC CVAC` + `dsb sy`) follows it.
fn set_checkpoint(leg: u32) {
    unsafe {
        core::ptr::write_volatile(&raw mut CHECKPOINT as *mut u64, cp_value(leg));
    }
    cache::clean_range(&raw const CHECKPOINT as usize, core::mem::size_of::<Checkpoint>());
}

/// Rust tail for legs 14..16, entered from the asm stub with the MMU ON, HCR/CPTR/regime replayed, and
/// this core's stack set. `leg` selects how far up the real `__secondary_rust_virt` path to climb —
/// each higher leg adds exactly ONE step. Sets the checkpoint, then parks in WFE. Never prints (the
/// woken-core UART rule); never returns.
#[unsafe(no_mangle)]
extern "C" fn __smpprobe_leg_rust(leg: u64) -> ! {
    // leg 14: install this core's EL2 vectors (+ HCR IMO/FMO/AMO). The first step past the bare MMU.
    if leg >= 14 {
        exceptions::install();
    }
    // leg 15 (PRIME SUSPECT): resolve this core's GICR frame and read its GICR_WAKER ONCE. Read-only —
    // the redistributor is NOT woken; the leg isolates whether the mere MMIO access to the target's
    // GICR window is the CBB-rejected access (the fault ADDR smells like an MMIO window).
    if leg >= 15 {
        let _waker = gic::probe_read_this_gicr_waker();
    }
    // leg 16 (full-path replica): the remaining `__secondary_rust_virt` tail — per-CPU block, GICv3
    // redistributor wake + CPU interface, enable the IPI SGI + unmask IRQ, then ping the BSP. Built
    // from the same public building blocks the real path uses (so `smp_virt.rs` stays byte-untouched).
    // Legs 17..20 all do this same full bring-up (they are "leg 16 + one variable" / leg-16 shape on a
    // different target / the 5-core sequence).
    if leg >= 16 {
        percpu::init(PROBE_CORE_INDEX);
        gic::init_secondary_v3();
        gic::enable_sgi(PROBE_IPI_SGI);
        exceptions::enable_irq();
        let bsp = unsafe { core::ptr::read_volatile(&raw const PROBE_BSP_AFF) };
        gic::send_sgi(bsp as usize, PROBE_IPI_SGI);
    }
    // leg 17 (ORIN-SMP-5) — THE VARIABLE: ONE `serial_println!` from the WOKEN CORE, i.e. the UART MMIO
    // access + the `SERIAL_PORT` console spinlock taken BY A SECONDARY — exactly the "AP online" print the
    // real `__secondary_rust_virt` does and the one the bisect deliberately forbade (RIDER: probe-only AP
    // silence held for 10..16). Uses the SAME `serial_println!` path as the BSP (the bounded-THRE tegra
    // writer in `serial.rs`) — no new UART code. If this access is the CBB-rejected one, the box RAS-powers
    // off HERE, before the checkpoint store below → the BSP times out with the box down.
    if leg == 17 {
        serial_println!(
            ":: tegra: SMPPROBE-5 sel=17 [AP] woken core online — serial_println! from the SECONDARY \
             (UART MMIO + SERIAL_PORT spinlock; the residue variable) ::"
        );
    }
    set_checkpoint(leg as u32);
    // leg 18 (ORIN-SMP-5) — THE VARIABLE: the real WFI idle tail. The replica parks in WFE; the real
    // secondary path parks in WFI. The checkpoint is already raised (survival is recorded), so this only
    // swaps the park instruction; the core is NOT expected to return.
    if leg == 18 {
        loop {
            unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
        }
    }
    loop {
        unsafe { core::arch::asm!("wfe", options(nomem, nostack, preserves_flags)) };
    }
}

// The per-leg woken-core entry stubs. Each runs at EL2 with the MMU OFF (a PSCI-reset core), x0 = the
// PSCI context id. They add exactly one variable each; the checkpoint-store + WFE-park tail is shared
// (`_smpprobe_leg_cp_park`, x9 = the value). Legs 12..16 first check `CurrentEL == 2` and, if the
// firmware dropped this AP to EL1, park WITHOUT a checkpoint (`_smpprobe_leg_wrongel_park`) so the BSP
// times out and reads "wrong EL" — never `msr *_el2` at the wrong EL. Absolute symbol refs resolve
// because the tegra map is identity (VA == PA).
core::arch::global_asm!(
    r#"
    // ── leg 10 — CONTROL: the exp5 park shape + the evidence store (no regime, no SP) ──
    .globl _smpprobe_leg10
    _smpprobe_leg10:
        movz  x9, #10
        movk  x9, #0x5304, lsl #16
        b     _smpprobe_leg_cp_park

    // ── leg 11 — +SP: set SP into PROBE_STACK, push/pop one frame, then checkpoint ──
    .globl _smpprobe_leg11
    _smpprobe_leg11:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2                 // top of PROBE_STACK
        mov   sp, x3
        sub   sp, sp, #16
        str   x30, [sp]                  // push one frame (DRAM write, MMU off)
        ldr   x30, [sp]
        add   sp, sp, #16
        movz  x9, #11
        movk  x9, #0x5304, lsl #16
        b     _smpprobe_leg_cp_park

    // ── leg 12 — +regime replay: HCR/CPTR then MAIR/TCR/TTBR0_EL2 (SCTLR NOT written, MMU stays OFF) ──
    .globl _smpprobe_leg12
    _smpprobe_leg12:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        isb
        movz  x9, #12
        movk  x9, #0x5304, lsl #16
        b     _smpprobe_leg_cp_park

    // ── leg 13 — +MMU: leg 12 + tlbi/dsb/isb + SCTLR_EL2 (MMU ON) + isb ──
    .globl _smpprobe_leg13
    _smpprobe_leg13:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        tlbi  alle2
        dsb   sy
        isb
        ldr   x5, [x4, #{sctlr_off}]
        msr   sctlr_el2, x5             // captured M=1 → MMU ON
        isb
        movz  x9, #13
        movk  x9, #0x5304, lsl #16
        b     _smpprobe_leg_cp_park

    // ── leg 14 — +exceptions: MMU on (as leg 13), then Rust tail with leg=14 ──
    .globl _smpprobe_leg14
    _smpprobe_leg14:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        tlbi  alle2
        dsb   sy
        isb
        ldr   x5, [x4, #{sctlr_off}]
        msr   sctlr_el2, x5
        isb
        mov   x0, #14
        bl    {leg_rust}

    // ── leg 15 — +GICR: same MMU-on prologue, Rust tail with leg=15 (the prime suspect) ──
    .globl _smpprobe_leg15
    _smpprobe_leg15:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        tlbi  alle2
        dsb   sy
        isb
        ldr   x5, [x4, #{sctlr_off}]
        msr   sctlr_el2, x5
        isb
        mov   x0, #15
        bl    {leg_rust}

    // ── leg 16 — full path replica: same MMU-on prologue, Rust tail with leg=16 ──
    .globl _smpprobe_leg16
    _smpprobe_leg16:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        tlbi  alle2
        dsb   sy
        isb
        ldr   x5, [x4, #{sctlr_off}]
        msr   sctlr_el2, x5
        isb
        mov   x0, #16
        bl    {leg_rust}

    // ── leg 17 (ORIN-SMP-5) — leg-16 shape + the AP serial print: same MMU-on prologue, tail leg=17 ──
    .globl _smpprobe_leg17
    _smpprobe_leg17:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        tlbi  alle2
        dsb   sy
        isb
        ldr   x5, [x4, #{sctlr_off}]
        msr   sctlr_el2, x5
        isb
        mov   x0, #17
        bl    {leg_rust}

    // ── leg 18 (ORIN-SMP-5) — leg-16 shape + the WFI idle tail: same MMU-on prologue, tail leg=18 ──
    .globl _smpprobe_leg18
    _smpprobe_leg18:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        tlbi  alle2
        dsb   sy
        isb
        ldr   x5, [x4, #{sctlr_off}]
        msr   sctlr_el2, x5
        isb
        mov   x0, #18
        bl    {leg_rust}

    // ── leg 19 (ORIN-SMP-5) — leg-16 shape on a CLUSTER-1 core: same MMU-on prologue, tail leg=19 ──
    .globl _smpprobe_leg19
    _smpprobe_leg19:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        tlbi  alle2
        dsb   sy
        isb
        ldr   x5, [x4, #{sctlr_off}]
        msr   sctlr_el2, x5
        isb
        mov   x0, #19
        bl    {leg_rust}

    // ── leg 20 (ORIN-SMP-5) — leg-16 shape, reused per core by the 5-core sequence: tail leg=20 ──
    .globl _smpprobe_leg20
    _smpprobe_leg20:
        adrp  x1, {stack}
        add   x1, x1, #:lo12:{stack}
        mov   x2, #({stack_size} >> 12)
        lsl   x2, x2, #12
        add   x3, x1, x2
        mov   sp, x3
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  _smpprobe_leg_wrongel_park
        adrp  x4, {regs}
        add   x4, x4, #:lo12:{regs}
        ldr   x5, [x4, #{hcr_off}]
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]
        msr   cptr_el2, x6
        isb
        ldr   x5, [x4, #{mair_off}]
        msr   mair_el2, x5
        ldr   x5, [x4, #{tcr_off}]
        msr   tcr_el2, x5
        ldr   x5, [x4, #{ttbr0_off}]
        msr   ttbr0_el2, x5
        tlbi  alle2
        dsb   sy
        isb
        ldr   x5, [x4, #{sctlr_off}]
        msr   sctlr_el2, x5
        isb
        mov   x0, #20
        bl    {leg_rust}

    // ── shared tails ──
    .globl _smpprobe_leg_cp_park
    _smpprobe_leg_cp_park:                // x9 = CP_TAG | leg
        adrp  x10, {cp}
        add   x10, x10, #:lo12:{cp}
        str   x9, [x10]
        dc    cvac, x10
        dsb   sy
    1:  wfe
        b     1b

    .globl _smpprobe_leg_wrongel_park
    _smpprobe_leg_wrongel_park:           // dropped to EL1 by the monitor: park, no checkpoint
    1:  wfe
        b     1b
    "#,
    stack = sym PROBE_STACK,
    stack_size = const PROBE_STACK_SIZE,
    regs = sym PROBE_REGS,
    cp = sym CHECKPOINT,
    leg_rust = sym __smpprobe_leg_rust,
    mair_off = const core::mem::offset_of!(ProbeRegs, mair),
    tcr_off = const core::mem::offset_of!(ProbeRegs, tcr),
    ttbr0_off = const core::mem::offset_of!(ProbeRegs, ttbr0),
    sctlr_off = const core::mem::offset_of!(ProbeRegs, sctlr),
    cptr_off = const core::mem::offset_of!(ProbeRegs, cptr),
    hcr_off = const core::mem::offset_of!(ProbeRegs, hcr),
);

unsafe extern "C" {
    fn _smpprobe_leg10();
    fn _smpprobe_leg11();
    fn _smpprobe_leg12();
    fn _smpprobe_leg13();
    fn _smpprobe_leg14();
    fn _smpprobe_leg15();
    fn _smpprobe_leg16();
    fn _smpprobe_leg17();
    fn _smpprobe_leg18();
    fn _smpprobe_leg19();
    fn _smpprobe_leg20();
}

/// The bisect leg entry symbol + a one-line description of the ONE variable it adds. Every fn is
/// address-taken here so the linker retains all of them (and their strings) regardless of `SEL`.
static LEGS: &[(u32, unsafe extern "C" fn(), &str)] = &[
    (10, _smpprobe_leg10, "control: exp5 park shape + the checkpoint store (no SP, no regime, MMU off)"),
    (11, _smpprobe_leg11, "+SP into PROBE_STACK + push/pop one frame (MMU-off DRAM writes)"),
    (12, _smpprobe_leg12, "+regime replay: HCR/CPTR + MAIR/TCR/TTBR0_EL2 (SCTLR NOT written; MMU stays OFF)"),
    (13, _smpprobe_leg13, "+MMU: tlbi alle2 + SCTLR_EL2 write (MMU ON) + isb"),
    (14, _smpprobe_leg14, "+exceptions::install() (per-core EL2 vectors)"),
    (15, _smpprobe_leg15, "+GICR: this_cpu_redistributor() + ONE GICR_WAKER read (PRIME SUSPECT)"),
    (16, _smpprobe_leg16, "full: +percpu + GICv3 secondary bring-up + IPI SGI (real-path replica)"),
    (17, _smpprobe_leg17, "SMP-5: leg 16 + ONE serial_println! from the woken core (UART MMIO + SERIAL_PORT spinlock)"),
    (18, _smpprobe_leg18, "SMP-5: leg 16 + the real WFI idle tail (replica parks WFE; the core is not expected to return)"),
    (19, _smpprobe_leg19, "SMP-5: leg-16 shape on the CLUSTER-1 core (DTB aff 0x00010200)"),
    (20, _smpprobe_leg20, "SMP-5: leg-16 shape, reused per core by the real 5-core wake sequence"),
];

/// The cluster-1 target for leg 19 (RIDER: DTB-only presence — this must be present in `/cpus`). On
/// Tegra234 the CCPLEX is 3 clusters × 2 cores; cluster-1 core 0 packs as {Aff2=1, Aff1=2, Aff0=0} =>
/// `0x0001_0200` (the SMP-3 5-core sequence woke `0x10200`/`0x10300`). Leg 19 STOPs (no `CPU_ON`) if the
/// DTB `/cpus` list does not name it.
const CLUSTER1_AFF: u32 = 0x0001_0200;

/// Bounded wait (`deadline` = a CNTPCT value) for `cond`; returns whether it held. Mirrors the SMP
/// deadline pattern so a wedged core never hangs the boot.
fn wait_until(deadline: u64, mut cond: impl FnMut() -> bool) -> bool {
    loop {
        if cond() {
            return true;
        }
        if timer::cntpct() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Read the checkpoint with the invalidate-then-read idiom (the woken core cleaned its store to PoC;
/// the BSP must drop any stale cached copy before reading). The checkpoint line is `align(64)` and
/// BSP-write-once (zeroed before `CPU_ON`), so invalidating it in the poll is safe.
fn read_checkpoint() -> u64 {
    cache::invalidate_range(&raw const CHECKPOINT as usize, core::mem::size_of::<Checkpoint>());
    unsafe { core::ptr::read_volatile(&raw const CHECKPOINT as *const u64) }
}

/// Run one ORIN-SMP-4 bisect leg (`leg` in 10..16). Picks the single target from the DTB `/cpus` list
/// (RIDER 5), publishes the regime/stack/checkpoint the woken core needs, issues ONE `CPU_ON` into the
/// leg's stub, and polls the checkpoint under a bounded deadline. All evidence is printed here (the
/// woken core is silent). A RAS fault powers the box off before the poll times out — that reset IS the
/// verdict for a faulting leg (the pre-`CPU_ON` dump names the target); a clean checkpoint = survived.
fn run_bisect_leg(ctx: &ProbeCtx, leg: u32) {
    serial_println!(
        ":: tegra: SMPPROBE-4 BISECT sel={} — probe-only woken-core execution bisect (see arch_arm64 \
         §ORIN-SMP-4); a RAS power-off is DATA ::",
        leg
    );

    // The leg entry + its one-variable description.
    let mut entry_fn: Option<unsafe extern "C" fn()> = None;
    for &(v, f, desc) in LEGS {
        if v == leg {
            entry_fn = Some(f);
            serial_println!(":: tegra: SMPPROBE-4 sel={} leg-variable = {} ::", leg, desc);
        }
    }
    let entry_fn = match entry_fn {
        Some(f) => f,
        None => {
            serial_println!(":: tegra: SMPPROBE-4 sel={} UNKNOWN leg; boot continues ::", leg);
            return;
        }
    };

    // RIDER 5 — the single target comes from the DTB `/cpus` list (never AFFINITY_INFO / GICR walk).
    let bsp_aff = gic::this_affinity();
    let mut dtb_cores = [0u32; 8];
    let n_dtb =
        super::fdt_tegra::cpu_affinities(ctx.dtb_addr, ctx.dtb_size, ctx.ram_gib_mask, &mut dtb_cores);
    // Leg 19 (ORIN-SMP-5) targets the CLUSTER-1 core specifically (DTB-only presence); every other leg
    // takes the first non-BSP `/cpus` core (the SMP-2/3 target `0x00000100`).
    let mut target: Option<u32> = None;
    if leg == 19 {
        for &a in dtb_cores.iter().take(n_dtb) {
            if a == CLUSTER1_AFF {
                target = Some(a);
                break;
            }
        }
        if target.is_none() {
            serial_println!(
                ":: tegra: SMPPROBE-5 sel=19 — DTB /cpus does NOT name the cluster-1 core {:#010x} \
                 (n={}, BSP aff={:#010x}); STOP, no CPU_ON ::",
                CLUSTER1_AFF, n_dtb, bsp_aff
            );
            return;
        }
    } else {
        for &a in dtb_cores.iter().take(n_dtb) {
            if a != bsp_aff {
                target = Some(a);
                break;
            }
        }
    }
    let target = match target {
        Some(a) => a,
        None => {
            serial_println!(
                ":: tegra: SMPPROBE-4 sel={} — DTB /cpus named no non-BSP core (n={}, BSP aff={:#010x}); \
                 STOP, no CPU_ON ::",
                leg, n_dtb, bsp_aff
            );
            return;
        }
    };
    serial_println!(
        ":: tegra: SMPPROBE-4 sel={} BSP aff={:#010x}; target aff={:#010x} (first non-BSP /cpus core) ::",
        leg, bsp_aff, target
    );

    // Leg 15 (RIDER: prime suspect) — compute + PRINT the target's GICR frame + the exact GICR_WAKER
    // MMIO address the woken core will read, BEFORE any CPU_ON, so the prediction can name it.
    if leg == 15 {
        match gic::redistributor_frame_for_affinity(target) {
            Some(frame) => serial_println!(
                ":: tegra: SMPPROBE-4 sel=15 target GICR frame={:#x}; GICR_WAKER @ {:#x} (the read under test) ::",
                frame,
                frame + gic::GICR_WAKER_OFFSET
            ),
            None => serial_println!(
                ":: tegra: SMPPROBE-4 sel=15 target aff={:#010x} matched NO GICR frame (walk found none); \
                 the woken core's this_cpu_redistributor() will itself STOP ::",
                target
            ),
        }
    }

    // Leg 17 (ORIN-SMP-5 RIDER: name the address under test) — the woken core's `serial_println!` drives
    // the tegra UART; PRINT its MMIO base BSP-side before any CPU_ON so the prediction can name it.
    if leg == 17 {
        serial_println!(
            ":: tegra: SMPPROBE-5 sel=17 the woken core will serial_println! through tegra UART base {:#x} \
             (UARTC; the console MMIO + SERIAL_PORT spinlock under test) ::",
            TEGRA_UART_BASE_DOC
        );
    }

    // Leg 19 (ORIN-SMP-5 RIDER: name the address under test) — leg-16 shape on the cluster-1 core; PRINT
    // that core's resolved GICR frame BSP-side before any CPU_ON.
    if leg == 19 {
        match gic::redistributor_frame_for_affinity(target) {
            Some(frame) => serial_println!(
                ":: tegra: SMPPROBE-5 sel=19 cluster-1 target aff={:#010x} GICR frame={:#x}; GICR_WAKER @ {:#x} ::",
                target,
                frame,
                frame + gic::GICR_WAKER_OFFSET
            ),
            None => serial_println!(
                ":: tegra: SMPPROBE-5 sel=19 cluster-1 target aff={:#010x} matched NO GICR frame (walk found none) ::",
                target
            ),
        }
    }

    // Publish what the woken core reads: the BSP affinity (leg 16 ping), the EL2 regime (legs 12+), and
    // the stack (legs 11+). Clean the regime to PoC and clean+invalidate the stack (so the loader's
    // cacheable BSS-zero lines cannot later evict-clobber the woken core's MMU-off stack writes). Zero
    // + clean the checkpoint last so a stale value from a prior leg's boot can never read as reached.
    unsafe { core::ptr::write_volatile(&raw mut PROBE_BSP_AFF, bsp_aff) };
    gic::enable_sgi(PROBE_IPI_SGI); // so the BSP can receive leg 16's AP→BSP ping
    capture_probe_regs();
    cache::clean_range(&raw const PROBE_REGS as usize, core::mem::size_of::<ProbeRegs>());
    cache::clean_invalidate_range(
        &raw const PROBE_STACK as usize,
        core::mem::size_of::<ProbeStack>(),
    );
    unsafe { core::ptr::write_volatile(&raw mut CHECKPOINT as *mut u64, 0) };
    cache::clean_range(&raw const CHECKPOINT as usize, core::mem::size_of::<Checkpoint>());
    cache::clean_range(&raw const PROBE_BSP_AFF as usize, core::mem::size_of::<u32>());

    let freq = timer::cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let bsp_ipi_before = percpu::cpu(0).ipis.load(core::sync::atomic::Ordering::Acquire);

    let entry = entry_fn as usize as u64;
    serial_println!(
        ":: tegra: SMPPROBE-4 sel={} issuing CPU_ON target aff={:#010x} entry={:#x} ctxid={} — \
         (survives => checkpoint {:#x}; RAS power-off => the leg names the rejected access) ::",
        leg, target, entry, PROBE_CORE_INDEX, cp_value(leg)
    );

    let ret = smc(PSCI_CPU_ON, affinity_to_mpidr(target), entry, PROBE_CORE_INDEX as u64);
    if ret != 0 {
        serial_println!(
            ":: tegra: SMPPROBE-4 sel={} CPU_ON RETURNED ERROR ret={} — no core woken (unexpected on 39.2.0); \
             END ::",
            leg, ret
        );
        return;
    }
    serial_println!(":: tegra: SMPPROBE-4 sel={} CPU_ON ret=0 (survived the call) — polling checkpoint ::", leg);

    // Bounded ~500 ms wait for the woken core to raise its checkpoint.
    let want = cp_value(leg);
    let deadline = timer::cntpct() + freq / 2;
    let reached = wait_until(deadline, || read_checkpoint() == want);
    if reached {
        serial_println!(
            ":: tegra: SMPPROBE-4 sel={} CHECKPOINT REACHED (val={:#x}) — leg SURVIVED (woken core ran the \
             added variable + parked) ::",
            leg, want
        );
        // Legs 16..19 all do the full leg-16 bring-up including the AP→BSP SGI ping — observe it.
        if leg >= 16 {
            let deadline = timer::cntpct() + freq / 10;
            let ok = wait_until(deadline, || {
                percpu::cpu(0).ipis.load(core::sync::atomic::Ordering::Acquire) > bsp_ipi_before
            });
            let after = percpu::cpu(0).ipis.load(core::sync::atomic::Ordering::Acquire);
            serial_println!(
                ":: tegra: SMPPROBE-4 sel={} AP -> BSP SGI {} (BSP ipi {} -> {}) — full path online ::",
                leg,
                if ok { "OK" } else { "TIMEOUT" },
                bsp_ipi_before,
                after
            );
        }
    } else {
        let seen = read_checkpoint();
        serial_println!(
            ":: tegra: SMPPROBE-4 sel={} CHECKPOINT NOT reached in ~500ms (last read={:#x}); box still up => \
             woken core parked wrong-EL or hung (NOT the RAS reset — that would have powered the box off first) ::",
            leg, seen
        );
    }
    serial_println!(":: tegra: SMPPROBE-4 sel={} leg DONE ::", leg);
}

/// ORIN-SMP-5 leg 20 — the real 5-core wake SEQUENCE. Wakes every non-BSP `/cpus`-named core (up to 5
/// on Tegra234) into the leg-16-shape stub (`_smpprobe_leg20`), IN DTB ORDER, ONE AT A TIME: each core
/// is woken, its checkpoint (`0x53040014`) polled under the bounded deadline, then the next is woken.
/// The single `PROBE_STACK`/`CHECKPOINT`/percpu-slot are reused sequentially and safely (a woken core
/// parks in WFE — off the stack — before the next `CPU_ON`; the checkpoint is re-zeroed each round). If
/// the fault is driven by the multi-core sequence (SMP-3 woke five, tonight's single-core legs woke one)
/// a RAS power-off fires mid-sequence — the pre-`CPU_ON` line names which core was under the gun. Runs
/// LAST and only if 17..19 all survived (runbook rider). Probe-only; DTB-only presence.
fn run_5core_sequence(ctx: &ProbeCtx) {
    serial_println!(
        ":: tegra: SMPPROBE-5 sel=20 — the real 5-core wake SEQUENCE (leg-16 shape per core, DTB order); \
         a RAS power-off mid-sequence is DATA ::"
    );

    // The leg-20 stub (leg-16 shape; each core sets checkpoint 0x53040014).
    let mut entry_fn: Option<unsafe extern "C" fn()> = None;
    for &(v, f, _desc) in LEGS {
        if v == 20 {
            entry_fn = Some(f);
        }
    }
    let entry = match entry_fn {
        Some(f) => f as usize as u64,
        None => return,
    };

    // RIDER 5 — the targets come from the DTB `/cpus` list, in order, minus the BSP.
    let bsp_aff = gic::this_affinity();
    let mut dtb_cores = [0u32; 8];
    let n_dtb =
        super::fdt_tegra::cpu_affinities(ctx.dtb_addr, ctx.dtb_size, ctx.ram_gib_mask, &mut dtb_cores);
    let mut targets = [0u32; 8];
    let mut n_targets = 0usize;
    for &a in dtb_cores.iter().take(n_dtb) {
        if a != bsp_aff {
            targets[n_targets] = a;
            n_targets += 1;
        }
    }
    serial_println!(
        ":: tegra: SMPPROBE-5 sel=20 BSP aff={:#010x}; {} non-BSP /cpus core(s) to wake in order ::",
        bsp_aff, n_targets
    );
    if n_targets == 0 {
        serial_println!(":: tegra: SMPPROBE-5 sel=20 — DTB /cpus named no non-BSP core; STOP, no CPU_ON ::");
        return;
    }

    // Publish the regime/stack/BSP-affinity ONCE (reused by every woken core). Same publication the
    // single-leg driver does.
    unsafe { core::ptr::write_volatile(&raw mut PROBE_BSP_AFF, bsp_aff) };
    gic::enable_sgi(PROBE_IPI_SGI);
    capture_probe_regs();
    cache::clean_range(&raw const PROBE_REGS as usize, core::mem::size_of::<ProbeRegs>());
    cache::clean_invalidate_range(&raw const PROBE_STACK as usize, core::mem::size_of::<ProbeStack>());
    cache::clean_range(&raw const PROBE_BSP_AFF as usize, core::mem::size_of::<u32>());

    let freq = timer::cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let want = cp_value(20);

    let mut survived = 0usize;
    for i in 0..n_targets {
        let target = targets[i];

        // Re-zero + clean the checkpoint so a previous core's raise cannot read as this core's.
        unsafe { core::ptr::write_volatile(&raw mut CHECKPOINT as *mut u64, 0) };
        cache::clean_range(&raw const CHECKPOINT as usize, core::mem::size_of::<Checkpoint>());
        let bsp_ipi_before = percpu::cpu(0).ipis.load(core::sync::atomic::Ordering::Acquire);

        serial_println!(
            ":: tegra: SMPPROBE-5 sel=20 [{}/{}] issuing CPU_ON target aff={:#010x} entry={:#x} \
             (survives => checkpoint {:#x}; RAS power-off => this core in the sequence is the wall) ::",
            i + 1, n_targets, target, entry, want
        );

        let ret = smc(PSCI_CPU_ON, affinity_to_mpidr(target), entry, PROBE_CORE_INDEX as u64);
        if ret != 0 {
            serial_println!(
                ":: tegra: SMPPROBE-5 sel=20 [{}/{}] CPU_ON RETURNED ERROR ret={} target aff={:#010x} — \
                 not woken; continuing sequence ::",
                i + 1, n_targets, ret, target
            );
            continue;
        }

        let deadline = timer::cntpct() + freq / 2;
        let reached = wait_until(deadline, || read_checkpoint() == want);
        if reached {
            survived += 1;
            let d2 = timer::cntpct() + freq / 10;
            let ok = wait_until(d2, || {
                percpu::cpu(0).ipis.load(core::sync::atomic::Ordering::Acquire) > bsp_ipi_before
            });
            serial_println!(
                ":: tegra: SMPPROBE-5 sel=20 [{}/{}] target aff={:#010x} CHECKPOINT REACHED ({:#x}); \
                 AP -> BSP SGI {} — core online ::",
                i + 1, n_targets, target, want, if ok { "OK" } else { "TIMEOUT" }
            );
        } else {
            let seen = read_checkpoint();
            serial_println!(
                ":: tegra: SMPPROBE-5 sel=20 [{}/{}] target aff={:#010x} CHECKPOINT NOT reached in ~500ms \
                 (last read={:#x}); box up => wrong-EL park or hang (not a RAS reset); continuing ::",
                i + 1, n_targets, target, seen
            );
        }
    }
    serial_println!(
        ":: tegra: SMPPROBE-5 sel=20 SEQUENCE DONE — {}/{} cores reached checkpoint (box survived the \
         full 5-core wake) ::",
        survived, n_targets
    );
}
