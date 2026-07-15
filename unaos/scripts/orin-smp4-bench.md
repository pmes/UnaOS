# ORIN-SMP-4 bench runbook — the woken core's EXECUTION BISECT (attended; one leg per boot)

This is a **bisect**, not a verification. The SMP-3 bench (§ORIN-SMP-3 STOP record) proved that on UEFI
39.2.0 firmware `CPU_ON` itself works (the SMP-2 exp5 park survived, ret=0), but waking the SAME core
(aff `0x00000100`) into the real `smp_virt::_secondary_start_virt` RAS-faults ×2 reproducibly (IOB
Status `0xe4000612`, SERR=0x12, IERR=CBB-0x6, ADDR `0x8000000000000200`, box reset) BEFORE the BSP
prints the `CPU_ON` result — i.e. the fault is the woken core's EARLY EXECUTION. This runbook drives the
`UNAOS_SMPPROBE=10..16` bisect (`arch/aarch64/smpprobe.rs`, §ORIN-SMP-4) to bracket which access is
rejected — one variable added per leg, one leg per boot.

## Hard rules for this bench

1. **RIDER 1 — leg 10 (park control) runs FIRST every sitting.** It proves the woken core executes at
   all on this firmware+kernel before any bisect variable is added.
2. **RIDER 2 — predictions are pre-registered (the table below, written BEFORE any boot) and exactly ONE
   variable is added per leg.** A leg whose outcome CONTRADICTS its prediction = **STOP THE SITTING
   THERE** (record it, run no further legs). Do not improvise a new leg on-metal.
3. **RIDER 3 — power-fault boots are DATA.** A RAS power-off IS the verdict for a faulting leg; recover
   with a full DC cut (per the SMP-2 runbook) and continue only per the pre-registered schedule.
4. **RIDER 4 — probe-only.** The woken core touches ONLY its own stack, the `SEC_CTX`-named regime
   registers, its own GICR frame (leg 15, one READ — never a write), and the checkpoint flag. No fuse or
   persistent-firmware writes.
5. **RIDER 5 — DTB-only presence.** The single target is the first non-BSP core from the DTB `/cpus`
   list; no `AFFINITY_INFO` / GICR-walk oracle. The leg-15 GICR frame address is COMPUTED + PRINTED
   BSP-side before the `CPU_ON`.

## Firmware precondition (assert BEFORE any leg)

The first serial lines must show UEFI `t23x_general 39.2.0-gcid-45755727` (or newer, Peter-acknowledged).
A downgraded/different firmware = **STOP** — the SMP-3 discrimination (CPU_ON-call works, execution
faults) was established on 39.2.0; on other firmware the JM5 CPU_ON-call wall may still stand and the
bisect's premise fails.

## The evidence channel (why the woken core is silent)

A secondary's PL011 writes race unarbitrated on metal (the pi core3probe lesson), so the woken core
NEVER prints. Instead each leg raises a **CHECKPOINT**: the woken core stores `0x5304_000<leg>` +
`DC CVAC` to PoC (the spin-table-slot idiom, MMU-off-safe), and the BSP polls it (invalidate-then-read)
under a bounded ~500 ms deadline. Read the BSP lines:

- `:: tegra: SMPPROBE-4 sel=<n> CHECKPOINT REACHED (val=0x5304000<n>) — leg SURVIVED …` → survived.
- box RAS power-off BEFORE that line → the leg FAULTED (its added variable is the rejected access).
- `:: tegra: SMPPROBE-4 sel=<n> CHECKPOINT NOT reached in ~500ms …; box still up …` → wrong-EL park or
  hang (NOT the RAS reset).

## Pre-registered prediction table (RIDER 2 — verbatim, matches §ORIN-SMP-4)

| leg | `UNAOS_SMPPROBE=` | variable added over the previous leg | predicted BSP serial | predicted box behavior |
|---|---|---|---|---|
| **10** | 10 | CONTROL — exp5 park shape + the checkpoint store (no SP, no regime; EL2, MMU off) | `sel=10 … CPU_ON ret=0 … CHECKPOINT REACHED (val=0x530400010… wait 0x5304000A)` | **SURVIVES** → checkpoint `0x5304000A`, box up |
| **11** | 11 | +SP into `PROBE_STACK` + push/pop one frame (MMU-off DRAM writes) | `sel=11 … CHECKPOINT REACHED (val=0x5304000B)` | **SURVIVES** → checkpoint `0x5304000B` |
| **12** | 12 | +regime replay `HCR/CPTR` + `MAIR/TCR/TTBR0_EL2` (SCTLR NOT written; MMU stays OFF) | `sel=12 … CHECKPOINT REACHED (val=0x5304000C)` | **SURVIVES** → checkpoint `0x5304000C` |
| **13** | 13 | +MMU: `tlbi alle2` + `SCTLR_EL2` write (MMU ON) + isb | `sel=13 … CHECKPOINT REACHED (val=0x5304000D)` | **SURVIVES** → checkpoint `0x5304000D` |
| **14** | 14 | +`exceptions::install()` (per-core EL2 vectors) | `sel=14 … CHECKPOINT REACHED (val=0x5304000E)` | **SURVIVES** → checkpoint `0x5304000E` |
| **15** | 15 | +GICR `this_cpu_redistributor()` + ONE `GICR_WAKER` read — **PRIME SUSPECT** | `sel=15 target GICR frame=0x0F44…; GICR_WAKER @ 0x0F44…14 …` then (if MMIO rejected) **no checkpoint** | **RAS power-OFF** expected (the GICR-MMIO access is the rejected one); SURVIVAL → checkpoint `0x5304000F` and the fault is elsewhere |
| **16** | 16 | full: +percpu + GICv3 secondary bring-up + IPI SGI (real-path replica) | `sel=16 … CHECKPOINT REACHED (val=0x530400010)` + `AP -> BSP SGI OK` | **RUNS LAST, only if 10..15 all survived**; predicted to **reproduce the SMP-3 fault** (RAS power-off), closing the bracket. SKIP if any earlier leg faulted |

**Leg-15 note.** The BSP prints `:: tegra: SMPPROBE-4 sel=15 target GICR frame=<addr>; GICR_WAKER @
<addr+0x14> (the read under test) ::` BEFORE the `CPU_ON`, so the exact MMIO address is on the
transcript even if the leg RAS-faults. Tegra GICR base `0x0F44_0000`, 4-frame stride `0x4_0000`; the
target aff `0x00000100` (cluster0 core1) resolves to whichever frame's `GICR_TYPER` matches — record the
printed value.

## Reading the results (decision table)

- **First leg that RAS-faults NAMES the rejected access.** Its added variable is the culprit; STOP the
  sitting there and report the leg + (for leg 15) the printed GICR address.
- Expected shape: **10..14 survive, 15 RAS-faults** → the fault is the GICR-frame MMIO access; the fix
  is a GICR-mapping / stride / frame-resolution question (a follow-up arc), not the regime replay.
- If **15 SURVIVES too** → the fault is in leg 16's remaining tail (redistributor WAKE write / SGI /
  percpu); run leg 16 to confirm the reproduction, then bisect that tail in a follow-up.
- If an EARLY leg (11/12/13) faults → the fault precedes GICR entirely (DRAM/SP or regime/MMU); a
  surprise that CONTRADICTS the prediction → STOP and report (RIDER 2).

## Schedule (one leg per boot; A-B-A bracketing optional)

1. Flash the leg-10 tar, boot, assert firmware precondition, confirm `CHECKPOINT REACHED (0x5304000A)`.
2. Ascend 11 → 12 → 13 → 14 → 15, reflashing per leg, recording each checkpoint / fault.
3. Run 16 LAST **only if** 10..15 all survived. If any leg faulted, STOP at it — 16 is spent evidence.
4. Optional A-B-A: re-flash the last surviving leg after a fault to confirm the boundary is stable (the
   CORE3 build-size discipline — record the git7 + tar sha of every boot).

## Recovery

A RAS power-off leaves the box off; do a full DC cut (unplug the barrel supply, wait, replug) before the
next boot — a warm reset can leave the CBB/MCE in a poisoned state that muddies the next leg. Recovery is
identical to the SMP-2 runbook.

## Staged media (flash ONLY from `~/unaos-bench/flash/orin/`, never `target/`)

Seven armed tars `UnaOS-orin-esp-smpprobe{10..16}-<UTCstamp>-<git7>.tar` (EFI + kernel.elf), plus the
knob-off DEFAULT tar for the byte-identity fallback. Shas in the MANIFEST + the ORIN-SMP-4 landing
report. Each armed image validates by its distinct ELF hash + `strings | grep SMPPROBE-4` present;
confirm the LIVE `sel=<n>` on the first `SMPPROBE-4` serial line matches the leg you flashed BEFORE
trusting the boot (`UNAOS_SMPPROBE` is compile-time — one image per leg).

---
## ⚡ SITTING VERDICT (2026-07-15 attended; serial `~/unaos-bench/jetson-serial-2026-07-15-smp4bisect.log`)

Legs 10–16, 7 boots, **0 faults** — firmware precondition matched every boot. 10–15 = predictions
EXACT (leg 15's `GICR_WAKER @ 0xf460014` read survived — prime suspect INNOCENT). **Leg 16 survived
AGAINST its prediction** — checkpoint `0x53040010`, `AP -> BSP SGI OK (BSP ipi 1 -> 2) — full path
online`, CAPSTONE: **the first live UnaOS AP on Orin silicon.** Sitting stopped at the contradiction
(final leg). Residue = what the replica deliberately omitted vs the real flow: (a) the AP's
`serial_println!` (UART MMIO + console spinlock from a secondary), (b) the WFI idle tail, (c) the
real 5-core sequence incl. cluster-1 (`0x10200/0x10300`) — tonight woke only `0x00000100`.
SMP-5 residue legs (17 +print · 18 +WFI · 19 cluster-1 · 20 five-core) pend ratification.
