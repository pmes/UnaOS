# Architecture Specification: AArch64 (ARM64)

## Status: PRE-ALPHA / PLANNING

## 1. Device Tree (DTB) vs ACPI
Unlike x86, ARM targets often rely on Device Trees.
* **Strategy:** unaOS for ARM will prioritize **ACPI** (ServerReady/SystemReady compliance) to unify the boot flow with x86.
* **Fallback:** For mobile devices (Pixel, etc.), a "Shim Loader" will translate the proprietary DTB into a standardized ACPI format for the kernel.

## 2. Page Size
* **Target:** 16KB Pages.
* **Reason:** Apple Silicon utilizes 16KB pages for performance. To emulate macOS apps efficiently on M-series chips later, our kernel memory manager must support non-4KB page alignment natively.

## 3. Interrupt Controller (GIC) — v2 / v3 parameterization

The AArch64 interrupt controller is the Arm Generic Interrupt Controller (GIC),
the analogue of the x86 local APIC + I/O APIC. A single driver
(`crates/kernel/src/arch/aarch64/gic.rs`) speaks **both** major GIC
architectures behind one public API; the version is chosen at runtime so the
same kernel image runs on either:

* **GICv2** — the QEMU `virt` board default (`gic-version=2`, a GIC-400-class
  model) and the real Raspberry Pi 4 (BCM2711 GIC-400). Two register banks: the
  global **Distributor (GICD)** and a per-core, memory-mapped **CPU interface
  (GICC)**.
* **GICv3** — QEMU `virt` with `gic-version=3`. The memory-mapped GICC is
  replaced by a **system-register CPU interface** (`ICC_*_EL1`), the banked
  SGI/PPI state moves out of the GICD into a per-core **Redistributor (GICR)**,
  and SPIs are routed by MPIDR affinity (ARE=1). The Jetson Orin Nano
  (Tegra234) also has a GICv3, which is why this path exists — but see
  "Orin metal pending" below.

### Version detection

At `gic::init` the driver reads **`ID_AA64PFR0_EL1.GIC` (bits [27:24])**: `0`
selects the GICv2 (memory-mapped) path, non-zero (`0b0001`/`0b0011`) selects the
GICv3 (system-register) path. This is a system-register probe on purpose — it
cannot fault, and a non-zero field is exactly the capability the v3 path depends
on (the `ICC_*_EL1` registers). We deliberately do **not** probe `GICD_PIDR2`:
its GICv3 offset (`0xFFE8`) lies inside the 64 KiB v3 distributor frame but
**outside** the 4 KiB GICv2 distributor, so an MMIO read there raises an
external abort on a v2 machine (confirmed on QEMU `virt` `gic-version=2`).

The **`pi` build hard-pins v2 at compile time** (the GIC-400 is never a GICv3):
detection and the entire v3 code path are `#[cfg(not(feature = "pi"))]`, so
`is_v3()` folds to a compile-time `false` and the Pi image is byte-identical to
the pre-v3 driver. Every v3 addition is additive and dispatch-gated; the shared
v2 code paths are unchanged.

### Register-model differences (what each public entry point dispatches)

| Operation                | GICv2                              | GICv3                                                        |
|--------------------------|------------------------------------|-------------------------------------------------------------|
| Distributor enable       | `GICD_CTLR` bit0                   | `GICD_CTLR` ARE (bit4) **then** EnableGrp1 (bit1), RWP-waited |
| CPU interface            | `GICC` MMIO (PMR/BPR/CTLR)         | `ICC_SRE_EL1` (+ `ICC_SRE_EL2` at EL2), `ICC_PMR_EL1`, `ICC_CTLR_EL1` (EOImode=0), `ICC_IGRPEN1_EL1` |
| Banked enable (SGI/PPI)  | `GICD_ISENABLER0` / `IGROUPR0`     | per-core Redistributor SGI frame (`GICR_ISENABLER0`, `GICR_IGROUPR0`) |
| Timer PPI 30 enable      | GICD                               | this core's Redistributor                                   |
| SPI routing              | `GICD_ITARGETSR` (CPU bitmask)     | `GICD_IROUTER<n>` (MPIDR affinity)                          |
| Acknowledge / EOI        | `GICC_IAR` / `GICC_EOIR`           | `ICC_IAR1_EL1` / `ICC_EOIR1_EL1`                            |
| Send SGI (IPI)           | `GICD_SGIR`                        | `ICC_SGI1R_EL1`                                             |

On the QEMU `virt`/UEFI path the kernel runs at **EL2** with IRQs routed there
(`HCR_EL2.IMO`); EL2 accesses to the `ICC_*_EL1` **physical** interface are
gated by `ICC_SRE_EL2.SRE`, which the v3 CPU-interface bring-up sets in addition
to `ICC_SRE_EL1`.

### Self-SGI delivery smoke

After the GIC is initialized (both versions, QEMU `virt` path only) the boot
core sends a free SGI (INTID 15) to itself and confirms it is delivered through
the IRQ vector, printing `:: GIC self-SGI delivered (v2) ::` /
`(v3) ::`. It runs before the timer is armed and before IRQs are globally
unmasked, so it briefly unmasks `PSTATE.I` itself, then restores `DAIF`. This is
the boot-core IPI smoke; cross-*core* SGI delivery is proven once the secondaries
are up (see SMP below).

### SMP (multi-core bring-up)

Two release mechanisms, selected by platform:

* **QEMU `virt` / UEFI — PSCI `CPU_ON` (Arc JC2).** UEFI starts only the boot
  core; the other three sit in PSCI-off state. The kernel starts them with the
  standardized **PSCI `CPU_ON`** call (function ID `0xC400_0003`) over the **SMC**
  conduit — the conduit that reaches QEMU's emulated PSCI from an EL2 guest
  (`virt,virtualization=on` advertises `method = "smc"` in the generated `/psci`
  node, confirmed via `qemu-system-aarch64 … -machine …,dumpdtb=…`; an `hvc` from
  EL2 would instead target our own EL2 vector). A woken core comes up through a
  CPU reset — MMU off, caches off, DAIF masked, at **EL2** — so it does not
  inherit the BSP's live registers. Rather than build fresh tables, the BSP
  **captures its live EL2 state** — `MAIR_EL2`/`TCR_EL2`/`TTBR0_EL2`/`SCTLR_EL2`
  plus `CPTR_EL2` (the FP-enable a PSCI-reset core does not inherit) — and each
  secondary **replays** it to join UEFI's identity map
  (`arch/aarch64/smp_virt.rs::enable_mmu_virt`, the EL2 analogue of the baremetal
  `boot::enable_mmu`). It then runs its own per-core GICv3 bring-up
  (`gic::init_secondary_v3` — redistributor wake + the system-register CPU
  interface, both banked) and **parks in WFI with IRQs unmasked**. The arc's
  verdict is cross-core SGI (`ICC_SGI1R_EL1`): BSP → each AP and AP → BSP, logged
  as `:: AARCH64 SMP: AP <n> online ::` plus the `BSP -> AP` / `AP -> BSP` SGI
  lines. GICv3 only, runtime-gated on `gic::is_v3()`; the GICv2 `virt` run stays
  single-core and byte-identical to baseline.
* **Raspberry Pi 4 bare-metal — spin-table (`arch/aarch64/smp.rs`).** The GPU
  firmware parks cores 1-3 in a spin-table; the kernel releases each by writing
  its entry into that core's release slot and `SEV`. Unchanged by JC2 — the whole
  PSCI path is `#[cfg(not(feature = "pi"))]`, so every Pi image compiles it out.

**Scheduler on the `virt` path — the boot core, since JC3.** The aarch64 scheduler
was `#[cfg(feature = "baremetal")]`-gated and coupled to EL1 (`ELR_EL1`/`SPSR_EL1`
eret paths), while the `virt` kernel runs at EL2. **Arc JC3** un-gates it: after the
JC2 SMP proof completes, the boot core drops **EL2 → EL1** (`arch/aarch64/boot_virt.rs`,
the `virt` analogue of the Pi `boot::drop_to_el1`/`enable_mmu`) and runs the full
6-primitive M4 CAPSTONE there — see the **JC3 result** section below. Only the boot
core drops; the JC2 secondaries stay parked at EL2 and still receive SGIs but run no
scheduled work (SMP scheduling on `virt` remains a later step). The per-core
generic-timer tick is likewise still deferred: arming it on a secondary would
double-count the shared `ticks()` clock the xHCI/e1000 timeout budgets read.

### Build knob

`UNAOS_GICV3=1 ./arroyo {arm,test-arm}` appends `-machine gic-version=3` to the
aarch64 `virt` QEMU invocation (QEMU takes the last value of the repeated
machine property). Default stays GICv2. The knob only affects the `virt` runs;
the Pi bare-metal paths (`kernel8*`, QEMU `raspi4b`) are always GICv2.

### Orin metal status

The boot-core GICv3 + generic timer are **METAL-CONFIRMED** on Orin (JM3 MMU + JM4
GIC/timer). **SMP** (all secondaries via PSCI `CPU_ON` + cross-core SGI) is written and
**QEMU-green** (Arc JM5: real Redistributor-frame enumeration for the fused core set,
multi-cluster affinity targeting `Aff2=cluster`/`Aff1=core`/`Aff0=0`, `HCR_EL2`/`CPTR_EL2`
carried in the AP entry stub, SMC conduit), but its **metal boot is ⛔ blocked**: on the real
Orin, PSCI **`CPU_ON` triggers a fatal Tegra RAS Uncorrectable Error** (CBB fabric "Error
response from slave") before any AP comes online, reproducibly across two attended attempts —
while the GICR enumeration, VLPIS stride, and the PSCI *query* path (`AFFINITY_INFO`) all work
on silicon. This is a Tegra-firmware (BL31/MCE) core-bring-up issue, not a JM5 code bug; it
needs a dedicated investigation (see the "JM5 result" section for the syndrome, the sharp
query-works/power-faults split, and the ranked hypotheses). QEMU models no Tegra machine, so
QEMU-green did not — and here could not — imply Orin-correct.

**Scheduler on Orin — the boot core, JM6 (⛔ metal FAILED) → JM6b (fixed; ✅ METAL-CONFIRMED
2026-07-06).** JM6 drops the Orin boot core **EL2 → EL1** (`arch/aarch64/boot_tegra.rs`) to run
the scheduler + CAPSTONE at EL1 — single-core, sidestepping the JM5 `CPU_ON` wall. The original arc
reused JM3's live EL2 identity `L1` as the EL1 table and **dark-hung at the drop on the real Orin**
(5 attended boots): EL2 leaf descriptors set AP[1] (RES1 at EL2), which the EL1&0 regime reads as
"EL0-writable", and the VMSA **forces PXN=1 on any EL0-writable region** — every RAM GiB was
privileged-execute-never, so the first EL1 fetch aborted and the armed `VBAR_EL1` vector could not even
fetch its handler. JM6b fixes it by building an **EL1-precise twin table** (`mmu_tegra`'s `L1_EL1`, RAM
AP[2:1]=0b00) and pointing `TTBR0_EL1` at that. **Metal-confirmed on the first attended boot: EL1
landing line + CAPSTONE 6/6 — the scheduler runs on Orin silicon.** Full story + the confirming
register/descriptor dump in the **JM6 result** section below.

### JC3 result — virt EL2 → EL1 drop + scheduler/CAPSTONE at EL1 (**QEMU-green**)

The `virt` boot core now runs the scheduler and the full six-primitive CAPSTONE at
**EL1**. The whole arc is QEMU-testable (the point is to prove the scheduler/EL1 path
on `virt gic-version=3`); a metal boot is a bonus, not the verdict.

**The drop (`arch/aarch64/boot_virt.rs`).** UEFI hands the `virt` kernel off at **EL2
with the MMU already on**. To run the EL1-coupled scheduler, the boot core builds a
fresh **EL1&0 identity map** — `L1[0]` = the low-1-GiB Device window (GICv3
`0x0800_0000`, PL011 `0x0900_0000`), each firmware-declared RAM GiB = a Normal-WB block
(QEMU `virt` = 1 GiB at `0x4000_0000`), plus a belt-and-braces re-mark of the executing
and stack GiBs — then **arms the EL1 regime with `M=1` while still at EL2** (dormant
there; `SCTLR_EL2` governs EL2 translation). The regime becomes live the instant the
`eret` lands at EL1, so **EL1 never runs a single instruction with its MMU off** — no
atomic ever executes on Device-typed memory (the hazard the Pi drop avoids by running no
atomics in its MMU-off window). `TCR_EL1`/`MAIR`/`SCTLR_EL1` are the proven `boot.rs`
EL1&0 recipe (`SCTLR_EL1` absolute with the A72 RES1 mask, since UEFI never initialised
it). The drop proper mirrors `boot::drop_to_el1` (HCR_EL2.RW, CPTR/CPACR FP-enable,
CNTHCTL, `VMPIDR`/`VPIDR` seed, `SPSR_EL2 = 0x3c5` = EL1h + DAIF masked, `eret` to `x30`),
with one `virt`-specific addition: it **disables the physical timer** (`CNTP_CTL_EL0 = 0`).

**Why the timer is disabled + cooperative CAPSTONE.** The shared IRQ vector stub
`exceptions::__vec_irq` banks `ELR_EL2`/`SPSR_EL2` on the not-baremetal build (compile-time
`irq_el!() == "2"`), which would fault if an IRQ were taken at EL1. So CAPSTONE on `virt`
runs **cooperatively** — exactly how the Pi CAPSTONE already runs under QEMU raspi4b (no
Group-1 delivery; every block/wake carried by the dispatch loop) — and the only periodic
IRQ source is killed before the drop. The JC2 SMP SGIs are quiescent by then (their proof
completed). `SCHED_ACTIVE` stays false.

**Per-CPU EL flip (`arch/aarch64/percpu.rs`).** Baremetal keeps a fixed `TPIDR_EL1`
(compile-time). The not-baremetal build (`virt` + tegra) now selects the thread-pointer
register at **runtime from `CurrentEL`** (`EL2 → TPIDR_EL2`, else `TPIDR_EL1`): the boot
core uses `TPIDR_EL2` for the EL2 bring-up + JC2 SMP proof, then `percpu::init(0)` is
re-called after the drop to seed `TPIDR_EL1`. tegra (always EL2) is behaviour-identical;
Pi is byte-identical (proven).

**Scheduler un-gate (`sched.rs` + `mod.rs`).** The scheduler module is now compiled for
`virt` too; all EL0/user machinery (`spawn_user*`, the user trampoline, the `exit()` slot
teardown, `poke_cpu`'s SGI id) stays `#[cfg(feature = "baremetal")]` (it reaches into the
Pi-only `super::boot`). CAPSTONE runs on the boot core **alone**
(`sched::run_capstone_boot_core`): coordinator and both "worker" cores are the boot core
(`CAP_CORES = [0, 0]`), so every cross-core wake degrades to a same-core cooperative
reschedule. This exercises the *semantics* of all six primitives (a real park + switch +
wake for each); true cross-core contention timing remains the Pi/metal proof.

**Evidence** (`UNAOS_GICV3=1 ./arroyo test-arm 45`):

```
:: AARCH64 SMP: 3/3 secondaries online via PSCI CPU_ON on the GICv3 path ::
:: JC3: SMP proof done; dropping the virt boot core EL2 -> EL1 for the scheduler + CAPSTONE ::
:: AARCH64 exception vectors installed (VBAR_EL1 = 0x7c38c000) ::      <- boot core now at EL1
:: AARCH64 SCHED (virt): boot core 0 at EL1 — running the full M4 CAPSTONE cooperatively ::
:: CAPSTONE Semaphore: PASS ::   :: CAPSTONE Mutex: PASS ::   :: CAPSTONE Channel: PASS ::
:: CAPSTONE Condvar: PASS ::     :: CAPSTONE RwLock: PASS ::  :: CAPSTONE join: PASS ::
:: CAPSTONE COMPLETE — all 6 sync primitives verified in one boot ::
```

The `VBAR_EL1` line (vs the pre-drop `VBAR_EL2`) is the crisp proof the boot core reached
EL1; CAPSTONE 6/6 then runs the scheduler + every sync primitive there. The **JC2 SMP proof
still passes 3/3** (it runs before the drop).

**Regression bar (all held).** `./arroyo check` both arches; `UNAOS_TEGRA=1 ./arroyo build`
both legs + `esp-jetson` link. GICv2 `virt` (`test-arm 25`): behaviour identical — the only
diff is **3 layout-only lines** (kernel ELF `192 → 198` pages, load address, `VBAR_EL2`, all
shifted by the added code). x86 (`test 25`): unaffected (MISSION SUCCESS + U1a/U1b/U2/U3
PASS). Pi (`kernel8-test 30`): **byte-identical mod interleave** (sorted-diff `0` vs a base
worktree at `31ff7a1`, over a log that runs the full Pi scheduler + CAPSTONE + M6g/EL0).

**Deferred (follow-on).** EL0 on `virt` (a `hello`-class program at EL1&0) is still later; the
Orin boot-core drop named here is now done — see the **JM6 result** below. SMP scheduling on
`virt` (dropping the APs too) is likewise later.

### JM6 result — Orin EL2 → EL1 drop + scheduler/CAPSTONE at EL1 (⛔ metal FAILED → **JM6b: fixed + ✅ METAL-CONFIRMED 2026-07-06**)

JM6 repeats the JC3 drop on the **Orin** (Tegra234, Cortex-A78AE) boot core: it drops
**EL2 → EL1** and runs the full six-primitive M4 CAPSTONE cooperatively at EL1 — the first time
the scheduler runs on Orin silicon. Single-core, so it needs no SMP and sidesteps the parked
JM5 `CPU_ON` wall entirely. The Orin path is **not** emulated in QEMU (QEMU models no Tegra
machine), so this lands **QEMU-green** — it compiles under `UNAOS_TEGRA` and the drop MECHANISM
is the JC3 one already proven on `virt` — and its **true verdict is a Peter-attended Orin serial
capture** (metal PENDING).

**The drop (`arch/aarch64/boot_tegra.rs`).** The tegra analogue of `boot_virt.rs`. As originally
landed, JM6 **reused the identity `L1` `mmu_tegra::init` already built** for the EL2 regime as the
EL1 table — the root cause of the metal FAIL (below). Since JM6b, `mmu_tegra::init` builds an
**EL1-precise twin** alongside the live EL2 table (`L1_EL1`, PA = `MmuInfo::ttbr0_el1`): the same
512-entry / RAM-GiB-mask identity shape, but with the EL1&0 leaf recipe — RAM AP[2:1]=`0b00`
(EL1 read-write, no EL0, EL1-executable), Device UXN|PXN — and cleans it to PoC next to the live
one. The EL1 arm points `TTBR0_EL1`/`TCR_EL1`/`MAIR_EL1`/`SCTLR_EL1` at the twin and arms it with
**`SCTLR_EL1.M=1` while still at EL2** (dormant there); it becomes live the instant the `eret`
lands at EL1, so **EL1 never runs a single instruction with its MMU off**. The map is identity,
so PC/SP do not move across the drop. `MAIR_EL1 = 0x04FF` matches the `L1`'s AttrIdx encoding
(Normal-WB / Device-**nGnRE** — Tegra's early-write-ack type, not the Pi/virt nGnRnE). The drop proper
mirrors `drop_el2_to_el1_virt` (VMPIDR/VPIDR seed, CPTR/CPACR FP-enable, CNTHCTL, HCR_EL2.RW,
`SPSR_EL2 = 0x3c5` = EL1h + DAIF masked, `eret` to `x30`, **CNTP disabled**), with one
tegra-specific addition: it **masks DAIF (`daifset #0xf`) up front**, because JM4 leaves IRQs
unmasked at EL2 (it proved the timer PPI delivering there) — CAPSTONE at EL1 needs none, and an
IRQ taken at EL1 would fault the EL2-banking `__vec_irq` stub. `SCTLR_EL1` is the absolute A72
RES1-mask value (`0x30D0_0800 | M|C|I`): every bit in that mask is, on A78AE, still RES1 or a
control whose 1-value is benign for a kernel-only core, and no A78AE RES1 bit lies outside it, so
the mask is a safe superset (Orin UEFI runs the kernel at EL2, so `SCTLR_EL1` was never
initialised → an absolute value, not an RMW).

**Call site (`main.rs::tegra_early_stop`).** After JM4 (GIC-600 + generic timer up, timer IRQ
proven at EL2), the terminus is now: `fbcon::detach()` → banner → `boot_tegra::drop_to_el1(
mmu.ttbr0)` → `percpu::init(0)` (now `TPIDR_EL1`) → `exceptions::install()` (now `VBAR_EL1`) →
`sched::run_capstone_boot_core(0)` (never returns). `percpu`/`exceptions` pick the EL from
`CurrentEL` at runtime (JC3), so they need no change. **JM5's `CPU_ON` SMP is deliberately not
invoked on this path**: on the real Orin the first `CPU_ON` RAS-faults (BL31/MCE, external
firmware) and powers the box off *before returning*, which would prevent ever reaching CAPSTONE;
JM6 is single-core by design. `smp_virt` stays compiled for tegra — it is simply not called.

**Metal verdict — ⛔ FAILED (dark hang at the drop; 5 Peter-attended boots, 2026-07-05).** On the
real Orin the boot core **dark-hangs at the EL2 → EL1 drop.** Every line up to and including
`:: tegra: JM6 — dropping … ::` prints cleanly — JM3 (EL2 MMU), JM4 (GIC/timer), and the heap init
(`:: KERNEL HEAP ALLOCATED ::`) all run on silicon — then nothing. Captures:
`target/serial-orin-jm6-FAIL{,2,3,4}-*.log`.

*Localization (what the 5 boots established):*
- **Monitor-independent** — headless reboots (matching the JM3/JM4 baseline) hang identically. (With a
  monitor the firmware publishes a GOP where headless publishes none, but the kernel boots `fb_addr=0`
  either way; the hang is unchanged.)
- **The `eret` reaches EL1** — an illegal exception return would have faulted to `VBAR_EL2` (which
  `mmu_tegra` set) and printed a syndrome; it does not. So the transition to EL1 completes.
- **The first EL1 instruction fetch aborts** — a diagnostic build armed `VBAR_EL1` with `mmu_tegra`'s
  bounded fault printer *before* the eret, and a second build `eret`ed into a naked EL1 stub that
  raw-writes `JM6` to UARTC as its first instructions (no stack, no calls). **Both stayed dark** — so
  `.text` is unfetchable/unexecutable at EL1 the instant the drop lands (the vector fetch aborts too).
- **`SCTLR_EL1` is not the sole cause** — switching from the absolute A72 RES1-mask value to
  `mmu_tegra`'s **metal-proven `SCTLR_EL2` RMW** (`read | M|C|I`) changed nothing.

*Root cause (JM6b, 2026-07-06) — the "only in AP[1]" dismissal was exactly backwards.* The reused
`L1`'s RAM leaves set descriptor bit 6 (`EL2_AP1_RES1` — AP[1] is RES1 in the single-privilege EL2
regime). Reinterpreted under EL1&0, bit 6 is AP[2:1]=`0b01` = **EL0 read-write** — and the VMSA
(Arm ARM DDI 0487, stage-1 instruction access permissions) **forces PXN=1 for any region writable at
EL0**, regardless of the descriptor's PXN bit and of `SCTLR_EL1.WXN`. "No EL0 exists yet" is
irrelevant; the rule is unconditional and base-Armv8 (Linux relies on it: the kernel never executes
user-writable memory). So every RAM GiB was privileged-execute-never at EL1: the first post-eret
fetch took a permission-fault instruction abort, and the armed `VBAR_EL1` vector — in the same
unexecutable RAM — could not fetch its handler either (recursive abort → dark). This explains **all**
the localization bullets above: eret completes, the very first EL1 fetch aborts, both instruments stay
dark, and no `SCTLR_EL1` variant matters. QEMU could not catch it (no Tegra machine model); the virt
twin was green because `boot_virt` builds its fresh table with the EL1 recipe (AP[2:1]=`0b00`).

*The fix (JM6b).* `mmu_tegra::init` now builds the **EL1-precise twin `L1_EL1`** (RAM AP[2:1]=`0b00`
EL1-executable, Device UXN|PXN, same RAM-GiB mask, cleaned to PoC) next to the live EL2 `L1` — which
is deliberately **not modified**: clearing AP[1] in a live single-privilege-regime table would be
writing a RES1 descriptor bit out from under the running EL2 walker. `boot_tegra::drop_to_el1` takes
`MmuInfo::ttbr0_el1`. Instrumentation retained for the metal boot (investigation-plan items 1–2,
executed): `tegra_early_stop` prints a **pre-drop line at EL2** — `HCR_EL2`, `ID_AA64MMFR1_EL1.VH`
(confirms non-VHE handoff, i.e. the `*_EL1` arms were not E2H-redirected), `TTBR0_EL1`, and the
read-back `L1_EL1[0]` / `L1_EL1[code-GiB]` descriptors — arms `VBAR_EL1` at `mmu_tegra`'s bounded
EL1 fault printer **before** the eret (under the fixed table the handler is fetchable, so a residual
landing fault prints a syndrome instead of hanging dark), and prints an **EL1 landing line**
(`CurrentEL` + live `SCTLR_EL1`) as the first EL1 serial output on Orin silicon.

**✅ Metal verdict — CONFIRMED (first boot, 2026-07-06, capture `target/serial-orin-jm6b.log`).** The
attended Orin boot ran the complete predicted chain: JM3 mmu live (EL2) → JM4 GIC/timer LIVE → heap →
`:: tegra: JM6b pre-drop — HCR_EL2=0x88000038 MMFR1.VH=1 TTBR0_EL1=0x25e5eb000
L1_EL1[0]=0x60000000000405 L1_EL1[9]=0x240000701 ::` → `:: tegra: JM6b — EL1 landing: CurrentEL=1
SCTLR_EL1=0x30d01805 ::` → `VBAR_EL1 = 0x25e5c3800` → **CAPSTONE 6/6 PASS at EL1** — the first time
the scheduler runs on Orin silicon. The pre-drop dump validates the diagnosis end to end: the twin
table is a distinct page from the EL2 `L1` (`0x25e5eb000` vs `0x25e5ea000`); the code-GiB leaf
`0x240000701` decodes to PA GiB 9 + AF + inner-shareable + Normal-WB + block with **AP[2:1]=0b00**
(EL1-executable — the fix); the device leaf carries UXN|PXN. Two facts worth keeping: `MMFR1.VH=1` —
VHE exists on the A78AE but the handoff regime was non-VHE (the `*_EL1` arms landed, as the JM3-based
exclusion argued); and the firmware+JM4 `HCR_EL2` was `0x88000038` — **TGE (bit 27) was set**, so the
drop's bare `HCR_EL2 = RW-only` write was load-bearing (an eret with TGE=1 would have been an illegal
return). The scheduler banner prints "(virt)" on Orin — a known cosmetic (hardcoded string in
`run_capstone_boot_core`), left for a follow-on.

**Regression bar (all held, QEMU).** `./arroyo check` both arches; `UNAOS_TEGRA=1 ./arroyo check`
both legs + `esp-jetson` link. virt (`UNAOS_GICV3=1 test-arm 45`): SMP 3/3 + the JC3 drop +
`VBAR_EL1 = 0x7c38c000` + CAPSTONE 6/6 — **byte-identical** to JC3 (same `VBAR_EL1` address, so
the virt binary layout is unshifted). Pi (`kernel8-test 30`): **sorted-diff 0** (the Pi *binary*
hash shifts only because a longer `main.rs` comment moves embedded panic/`Location` line-number
strings; the serial log — the behaviour — is unchanged). x86 (`test 25`): MISSION SUCCESS. All
JM6 code is `tegra`-gated (a new `boot_tegra` module, a tegra arm on the `sched` cfg gate, and the
tegra-only `tegra_early_stop` body), so every non-tegra build's cfg set — and thus its output — is
unchanged.

### JM7 — Orin video: the GOP framebuffer through the tegra tables (⛔ **blocked on firmware: the GOP is BltOnly**)

With a monitor connected at boot, NVIDIA's UEFI publishes a GOP and the bootloader hands its
framebuffer to the kernel (`BootInfo::framebuffer_*`); `fbcon` — the boot-log mirror every
`serial_println!` already writes through — has been drawing to it since `kernel_main` step 0 under
the UEFI map. JM7 makes that survive the kernel's own translation regimes, turning the monitor into
a live boot console on the Orin:

- **`mmu_tegra::build_l1` step 2b** force-marks the GOP's GiB range into the RAM-GiB mask (the GOP
  carveout can sit in a Reserved region the RAM scan skips), so both the live EL2 `L1` **and the EL1
  twin `L1_EL1`** map it — the mirror works before *and after* the JM6b drop. Mapped Normal-WB (a
  1 GiB block cannot carry a separate attribute without a new MAIR index); CPU-write → scanout
  coherency rides fbcon's existing damage-tracked `flush_range` → `dc cvac` (the Pi recipe;
  `arch::flush_framebuffer_range` is unconditional on aarch64). Headless boots (`fb addr=0`): the
  mask is untouched and fbcon stays inert — behavior identical to JM6b.
- **`tegra_early_stop`** prints the GOP handoff (`:: tegra: JM7 — GOP fb addr=… size=… WxH stride
  bpp ::`) and **no longer detaches fbcon** before the drop (contrast JC3/virt, whose EL1 map omits
  the fb) — the monitor shows the pre-drop dump, the EL1 landing line, and the CAPSTONE run live.
- **Metal verdict (2026-07-06, monitor on the DP→HDMI cable, capture `serial-orin-jm7.log`): the
  kernel side is confirmed safe and correct, but NO PIXELS — NVIDIA's UEFI GOP exposes no linear
  framebuffer.** The bootloader's GOP log shows 5 modes, EDID read fine (the monitor's native
  1920×1200 was detected and current), but every mode is BltOnly — `usable()` found no Rgb/Bgr
  mode to set, and the active mode's `frame_buffer()` path correctly reported *"active mode has no
  linear framebuffer; booting without a display"*. The kernel received
  `fb addr=0x0 size=0x0 1920x1200 stride=2048 bpp=4` (mode info without a surface), fbcon stayed
  inert, and the boot ran headless-identical: EL1 landing + CAPSTONE 6/6 again. Conclusion:
  **video-via-GOP is impossible on this firmware**; real Orin video means driving the Tegra234
  display engine (nvdisplay) directly — allocate a kernel surface, program scanout, then hand
  fbcon/`Screen` that address, at which point JM7's mapping + flush machinery is reused unchanged.
  That is its own major arc. The JM7 code stays: correct, inert when `fb addr=0`, and the landing
  pad for the nvdisplay arc's surface.
- Out of scope, documented for the follow-on: interactive console/GUI on Orin (input), and real USB
  keyboard/mouse — the Orin's built-in ports are **Tegra XUSB** (a platform controller needing
  firmware/phy bring-up, NOT the PCIe xHCI the kernel already drives); that is its own arc.

### JX1 result — XUSB first light (⛔ **the block is EL3-fatal to touch post-EBS; BPMP ungate required first**)

The one-boot probe (a guarded read of the xHCI capability block @ `0x0361_0000`, the Linux DT
`usb@3610000` base) answered the keyboard/mouse arc's gating question decisively, in the negative:
after `ExitBootServices`, the **first read fired an SError — ESR `0xbe000011` (EC=0x2F SError,
ISS=0x11) — fatal to EL3**: NVIDIA's BL31 printed "Unhandled Exception in EL3" and a crash dump
(capture `serial-orin-jx1.log`; the boot had run cleanly through JM3/JM4/heap first). UEFI tears
its USB stack down at handoff and the XUSB partition is clock-gated/powered off; a gated Tegra
block is a **CBB-fabric abort**, not an open-bus read — the same wall class as JM5's `CPU_ON` RAS
fault, and it means no guarded-read pattern can probe gated blocks safely. The probe was removed
after one boot (it would kill every boot); the result comment stays at the call site.

**Implication for the USB arc (the Opus brief):** bring up the **tegra-bpmp IVC channel first**
(HSP doorbell + shared-memory ring to the BPMP), then `MRQ_CLK`/`MRQ_RESET` to enable + de-reset
the XUSB host partition (and the padctl), THEN re-probe `0x0361_0000` — only after that does the
"platform-attach the existing xHCI stack" plan (and the XUSB-firmware-load question) become
testable. Linux's `xhci-tegra` follows exactly this order. The BPMP shared-memory/doorbell bases
and MRQ ABI are in the L4T sources (`tegra-bpmp` bindings); budget one arc for the IVC channel +
clock/reset MRQs alone.

### JB1 result — the BPMP channel + XUSB ungate (✅ **METAL-CONFIRMED**), and the EC=0 phantom (open)

**Landed and metal-proven (2026-07-06, captures in `serial-orin-jx1.log`):**
- **JB1a**: the firmware DTB (BootInfo::dtb_addr) parsed on silicon by the hand-rolled bounded FDT
  walker (`fdt_tegra.rs`; the root node's EMPTY name cost one boot — path matchers must not emit
  a leading double slash). Geometry: `/bpmp mboxes[hsp, DB, master 19]`, shmem TX `0x4007_0000` /
  RX `0x4007_1000` (SYSRAM `sram@40000000` children), HSP @ `0x03c0_0000`.
- **JB1b**: the IVC command channel (`bpmp_tegra.rs`): HSP doorbell derived on-board from
  HSP_INT_DIMENSIONING (`dim=0x8a228` → db_base `0x3c90000`, BPMP doorbell index 3), queue index
  3, stride 256; the SYNC→ACK→ESTABLISHED handshake (first metal run found the peer already in
  SYNC and exposed a missing final arm — the acked→peer-ESTABLISHED transition — fixed);
  **`MRQ_PING err=0 reply=0xab5466 (want 0xab5466) → PASS`, twice** — the first UnaOS↔BPMP
  exchange on silicon.
- **JB1c**: XUSB host partition ungated over that channel — ids read off the DTB's `usb@3610000`
  node (8 clocks, 2 power domains; NOTE: the firmware DTB exposes **no `resets` prop** on that
  node — JB2 must check the padctl node and Linux's reset ordering); `MRQ_PG` ON (domains 12, 10)
  and `MRQ_CLK` enable (267, 269, 268, 275, 14, 272, 103, 14) all `err=0`; then the exact read
  that was **EL3-fatal in JX1** returned: **xHCI v1.20, CAPLENGTH 0x20, HCSPARAMS1 0x08000524
  (8 ports), USBCMD 0, USBSTS 0x11 (halted-but-decoding) → PASS.** The JB2 keyboard arc starts
  from a live controller.
- **Bootloader I-cache fix** (`bootloader/main.rs`): the loader previously jumped into a freshly
  written+relocated image with NO cache maintenance — stale I-cache lines of the previous build's
  code at the recycled load base are a real ARMv8 hazard (invisible in QEMU). Now: `dc cvau` the
  image to PoU, `dsb ish; ic iallu; dsb ish; isb`. Correct regardless of the phantom below.

**✅ RESOLVED — the EC=0 phantom = Cortex-A78AE erratum 1941500, unmitigated in NVIDIA's firmware
(root-caused + healed 2026-07-06, same session).** The chain, each link metal-verified: (1) the
EC0-probe printed the D-side word at the faulting ELR = the valid encoding (`0xa9454ff4`) while
the I-side raised UNDEFINED — a proven I/D divergence (CTR_EL0: DIC=0, IDC=1); (2) MIDR
`0x410fd421` = **A78AE r0p1**, inside erratum 1941500's affected range (≤ r0p1); (3)
CPUECTLR_EL1 reads `0xa000000b40543000` — **the documented workaround bit [8] is CLEAR** (TF-A's
A78AE workaround was historically inverted — `bic` for `orr` — and NVIDIA's BL31 is TF-A
lineage); (4) the bit is **EL3-gated**: an EL2 `msr` traps to an unhandled EL3 exception (BL31
crash dump; the JB1d write was disarmed to report-only after two boot-loop boots). Only an NVIDIA
firmware update can set the bit. **The OS-side mitigation (JB1e, `exceptions.rs`)**: on a sync
EC=0 whose D-side word reads back valid, `ic iallu; dsb ish; isb` and RETRY the instruction —
`__vec_sync` gained SAVE_FP/RESTORE_FP + an eret path (the requirement the frame-macro comment
always documented), bounded at 64 heals/boot, one counted serial line per heal, all other faults
print-and-halt unchanged. Verification boot: the full chain (ping PASS, XUSB ALIVE, EL1 landing,
CAPSTONE 6/6) ran clean. The JB1d evidence line (r0p1 + bit8=0) prints every boot as the standing
record for the NVIDIA upstream report.

**Heal proven in live fire + JB2a port survey (the session's closing boot):** the phantom struck
mid-CAPSTONE and the heal caught it — `heal #1: EC=0 at ELR=0x25e544d30 (D-side 0xf81f0ffe valid)
— ic iallu + retry` — and the boot ran to CAPSTONE COMPLETE. Same boot, the JB2a survey (pure
PORTSC reads on the ungated controller, no writes/reset) reported the plugged keyboard: **ports 6
and 7 CONNECTED, PORTSC=0x000202e1 (CCS=1, PLS=7 Polling, USB2)** — the JB2 enumeration arc's
starting evidence, delivered before a line of driver porting.

*(Historical dossier, superseded by the above:)*
three occurrences of a fault with **ESR exactly `0x2000000` (EC=0x00 "unknown reason", IL=1,
ISS=0), FAR=0**, at INNOCENT instructions (a stack `str` before a `bl`; an epilogue
`ldp x29,x30,[sp],#0x60`), at BOTH ELs (twice at EL2 in the JB1b window on one specific binary,
deterministic for that binary; once at EL1 mid-CAPSTONE task-teardown on the JB1c boot — which
otherwise passed all JB1c lines first). Survived the I-cache fix. The Part-C vectors now print
the ENTRY INDEX + SPSR (an async entry printing stale ESR would masquerade as sync — the EL1 hit
came through exceptions.rs which attributes SYNCHRONOUS natively). Candidate directions for the
follow-on: full GPR/SPSR dump at the fault; A/B the JB1c ungate vs CAPSTONE (the EL1 hit was the
first boot combining both — though the EL2 hits predate any ungate); `timer::LIVE=true` steering
sched down metal-only paths at EL1; a CBB/implementation-defined report masquerading as EC=0.
Every occurrence is deterministic per binary+flow, so one instrumented boot per hypothesis
decides. The JB1 deliverables above all completed BEFORE the EL1 hit and stand independently.

### JB2b — the xHCI platform attach + polled USB keyboard (QEMU-green, ⏳ METAL-PENDING)

JB2b attaches the repo's **existing, x86-metal-proven xHCI driver** (`drivers/xhci/`) to the XUSB
block JB1c ungated, at its raw MMIO base `0x0361_0000` — no PCIe, no MSI-X, fully polled — and
brings a USB keyboard to first light: enumeration at EL2 pre-drop, keystrokes decoded and printed
at EL1 by a scheduled task. The driver needed **no platform constructor at all**: its controller
takes one raw base address, and the whole PCIe layer (`PciScanner`, bus-master enable, MSI-X) was
always caller-side. The tegra attach (`arch/aarch64/xusb_tegra.rs::jb2b_attach`) replays the exact
`arch/aarch64/pci.rs` sequence — `xhci::init` (halt + HCRST + CNR) → `XhciController::new` →
rings → `init_interrupter` → `init_pointers` → `start()` — then pumps the polled enumeration
(`poll_events` + `service_hubs`/`service_hid_setproto`/`service_slot_disposal`/`service_enum`)
bounded to 60 s of CNTPCT, exiting when a keyboard's interrupt-IN read is armed
(`keyboard_state == 3`). The window is sized to the worst case, not the happy path: on Orin's
31.25 MHz counter `hw_wait_budget()` is ~4.8 s per bounded stage (double its ~60 MHz design
note), and a stalled co-device ahead of the keyboard in the serialized queue can burn a
~22 s retry ladder before the keyboard's port is even tried; the happy path exits in seconds.
On success, a cooperative kernel task (`jb2-kbd`,
`xusb_tegra::kbd_pump_body`) is spawned onto the boot core **before** the JM6 drop
(`poke_cpu` self-skips, so nothing latches at the GIC to greet EL1); `run_capstone_boot_core`'s
EL1 dispatch loop co-runs it with the CAPSTONE tasks, and it keeps running after they drain.

Design facts (each verified against Linux `xhci-tegra.c` / `tegra234.dtsi` / edk2-nvidia before
a line was written):
- **Firmware**: on Tegra234 the xHCI Falcon firmware is UEFI-resident (Linux's tegra234 soc data
  loads none; its IFR path only reads the running firmware's header). `USBCMD.HCRST` resets the
  xHC state machine, not the Falcon (separate reset domain) — the driver's standard init is
  exactly what Linux runs on t234.
- **DMA coherence**: `tegra234.dtsi` marks `usb@3610000` `dma-coherent` — XUSB snoops the CPU
  caches, so the driver's Normal-WB heap rings (identity-mapped: VA==PA on both tables) need no
  cache maintenance. Verify-don't-assume: `fdt_tegra::xusb_dma_coherent` probes the **live**
  firmware DTB for the prop and the boot prints the verdict before the attach.
- **Ordering (the two shared-driver edits, both invisible on x86)**: `dsb st` (aarch64-only)
  before the doorbell write in `ring_doorbell_asm` and before the Run/Stop write in `start()` —
  the pre-existing `fence(SeqCst)` lowers to `dmb ish`, which does NOT order Normal-memory TRB
  writes against a Device-nGnRE (outer-shareable) doorbell; `dsb st` is Linux's `__iowmb`. And a
  `fence(Acquire)` (`dmb ishld`) in `EventRing::pop` between the cycle-bit check and the full TRB
  read — loads from different TRB words may otherwise satisfy out of order (Linux's `rmb()`
  twin). x86 codegen is unchanged (cfg'd out / compiler-only).
- **padctl/PHY**: left exactly as UEFI programmed it (separate block + reset domain, not in the
  JB1c PG toggle; the JB2a CCS=1/PLS=Polling reads ARE that state working). The BAR2 firmware
  mailbox (SS clock-scaling requests) is interrupt-delivered and irrelevant to polled HS work.
- **EL3-fatal discipline**: the attach is gated on `jb1c_ungate_xusb`'s new ALIVE verdict
  (`main.rs` threads it as `xusb_alive`) plus its own pre-flight cap0 read — `0x0361_0000` is
  never touched on a boot whose ungate failed. Every new register class prints before first
  touch; every wait is budget-bounded, so the worst case (SMMU not in bypass ⇒ DMA silently
  dropped; signature = healthy PORTSC + command timeouts) is a few bounded timeouts, an honest
  topology dump, and the **unchanged** JM6b drop + CAPSTONE chain.
- **EL1 liveness**: the EL1 pump calls ONLY `poll_events` (event drain → HID decode →
  interrupt-IN re-arm) — never the `service_*` sync pumps, whose `crate::hlt()` would WFI with no
  wake source at EL1 (the drop disables the timer but `timer::LIVE` reads stale-true). Busy-poll
  + `yield_now`, never `sleep_ticks` (the boot-core drive loop drains no sleepers).

Expected metal serial (the attended-boot checklist): the unchanged JB1/JB2a chain, then
`:: tegra: JB2b — usb@3610000 dma-coherent: YES … ::` → `:: tegra: JB2b — attaching the shared
xHCI driver @0x3610000 … ::` → the driver's own init lines (`Controller Reset Complete`,
`Supported Protocol`, `scratchpad`, `Controller Started!`) → per-port enumeration stages →
`SET_PROTOCOL(boot) OK` → `:: tegra: JB2b — keyboard ARMED (slot S, root port P) -> PASS ::` →
`:: tegra: JB2b — EL1 keyboard pump task spawned (boot core) ::` → the unchanged JM6b drop +
EL1 landing → `:: tegra: JB2b — EL1 keyboard pump live (xHCI polled at EL1) ::` → CAPSTONE 6/6 →
then, per keystroke, `xHCI: KEY: 'h' …` + `:: tegra: JB2b — KEY 'h' ::`. If no keyboard arms:
`keyboard NOT armed within the window` + a topology dump, and the boot completes as JB1e did.

Adversarial review (7-lens workflow + per-finding refutation pass): 0 must-fix; both should-fix
findings FIXED in-arc — (1) the pump window was 20 s but the worst-case stalled-co-device retry
ladder computes to ~22 s on Orin's clock → widened to 60 s; (2) the JB1e heal's serial print
hard-locked `SERIAL_PORT` from fault context — a phantom striking mid-`_print` would have
spin-deadlocked the very heal that was about to fix it → the heal line now `try_lock`s (skips
the line, never the heal, if the fault interrupted a print). Two accepted-and-noted nits:
the xHCI extended-capability walk trusts hardware-provided next-pointers (could in principle
chase garbage outside the ungated block — well-formed on this silicon, UEFI walks the same list
every boot; a range clamp is deliberate non-creep for this arc), and the new `dsb st` barriers
are aarch64-wide (virt/Pi metal included), a strictly conservative strengthening.

Gate results (2026-07-06, re-run after the review fixes): `./arroyo check` + `UNAOS_TEGRA=1
./arroyo check` green; x86 `test` MISSION SUCCESS; virt GICv2 `test-arm` full USB suite green;
virt GICv3 `test-arm` JC3 EL1-drop + CAPSTONE 6/6; Pi `kernel8` builds + `kernel8-test` full
battery (M6*, U4, U5, CAPSTONE) green; `esp-jetson` links. NOTE: virt `VBAR_EL1` moved from
round 10's `0x7c38c000` — the barrier instructions grew the aarch64 image and shifted the link
layout; the vector base is still 2 KiB-aligned and unshifted-in-the-bug-sense, and the whole
CAPSTONE chain is green at the new address (the current value prints in the run's own
`exception vectors installed` line). QEMU models no Tegra — **the verdict is the next
Peter-attended metal boot.**

### JB2b — the attended metal verdict (2026-07-06): driver attach CONFIRMED, enumeration blocked by a firmware regression

Two attended Orin boots (SD-in-USB-reader boot media; the same keyboard NVIDIA UEFI had just
enumerated as "Generic Usb Keyboard" on the shell `devices` list). **Both booted the JB2b binary
clean through the entire software chain:**

- **JB1 replays deterministically on silicon**: MRQ_PING PASS, XUSB ungate all `err=0`, XUSB
  ALIVE `xHCI v1.20`, zero erratum-1941500 heals fired.
- **JB2b's driver attach is metal-proven**: `dma-coherent: YES` from the live DTB, then the
  shared x86 driver ran on non-PCIe silicon for the first time — `Controller Reset Complete`,
  **both** protocol banks decoded (`USB 3.1 ports 1..4`, `USB 2.0 ports 5..8`), interrupter +
  rings programmed, scratchpad (3 buffers) allocated, `Controller Started!`.
- **The honest-failure path worked exactly as designed**: no keyboard armed in the 60 s window →
  full 8-port topology dump → the **unchanged** JM6b EL2→EL1 drop → EL1 landing → **CAPSTONE 6/6**.
  The boot completed as JB1e did, precisely as the JB2b brief anticipated for the no-keyboard case.

**But every one of the 8 root ports read `PORTSC=0x000002a0` (CCS=0, PP=1, PLS=RxDetect) from the
first pre-reset survey read, USBSTS.PCD never set, and the keyboard LED stayed dark** — physically
confirmed (no LED, and an unplug/replug into a *different* port during the live window still
produced zero port activity). The bus was electrically silent the instant our kernel took over.

**Root cause (4-agent research pass, high confidence — see the JB2c brief for the full dossier):
a NVIDIA firmware update between the JB2a test and this one.** JB2a saw ports 6 & 7 CONNECTED with
`USBSTS=0x11` (controller left live, port-change pending) on the older firmware. The updated
firmware (JetPack 6.0 GA / L4T r36.3+, edk2-nvidia "hide device resources at uefi exit") runs a
more aggressive `ExitBootServices` teardown on the Device-Tree boot path: it **power-gates the
XUSB partition and de-programs the padctl USB pads**. Our kernel's JB1c re-ungates the *controller*
(so the xHCI MMIO decodes and `Controller Started!` succeeds), but **nothing re-programs the
padctl pads** — they sit at reset defaults (powered down), so no root port ever leaves RxDetect,
CCS never asserts, and the downstream RTS5420 hub (which fans out the 4 physical Type-A ports)
never trains or powers its ports. `USBSTS=0x11`(old)→`0x01`(new) is the fingerprint.

This is **not** a JB2b bug and **not** a VBUS-GPIO problem — the P3768 devkit's 5V rail is
hardwired `regulator-always-on` with no GPIO enable, so the dark LED is a downstream *symptom* of
the un-trained pad, not a missing GPIO write. JB2b's design note "padctl/PHY: left exactly as UEFI
programmed it" was a correct bet on the old firmware and lost to the update. **The fix is a new arc,
JB2c** (below): re-program the padctl USB2 pad power-on sequence at `0x3520000` (pad n=1). padctl is
always-powered (outside the PG toggle) and already in the GiB-0 device map, so it is *not* a new
EL3-fatal MMIO class like JX1 — reads and writes there are safe on this ungated block.

Captured: `target/serial-orin-jb2b-1.log` (both boots + the UEFI shell `devices`/`map` that proves
the keyboard was live pre-handoff).

### JB2c brief — re-program the padctl USB2 pads (the enumeration fix)

Scoped from the JB2b metal verdict + the 4-agent research pass (2026-07-06). **One arc; do not
stack on unreviewed JB2b.** Lane: `arch/aarch64` tegra files (`xusb_tegra.rs` — revise the
"never touch padctl" directive at its head; `bpmp_tegra.rs` — add `TEGRA234_CLK_USB2_TRK` enable
+ the survey; a new padctl helper), this doc.

**Step 0 (this arc's first boot — read-only confirm, cheapest fork).** After JB1c ALIVE, dump
padctl read-only next to the PORTSC survey: `USB2_PAD_MUX` (0x3520004), `USB2_PORT_CAP` (0x3520008),
`OTG_PAD1_CTL0` (0x35200C8), `OTG_PAD1_CTL1` (0x35200CC). **Prediction that confirms the root
cause**: `OTG_PAD1_CTL0.PD` (bit26) and/or `PD_ZI` (bit29) *set*, and `PORT_CAP` for port 1 *not*
HOST(0x1) — i.e. pads at reset defaults. If instead the pads read already-programmed yet PORTSC is
still 0x2a0, the cause shifts to VBUS/rail or the NISO1 SMMU and this arc is the wrong fix — so this
one read-only boot forks the whole decision. (padctl is always-powered; these reads cannot EL3-fault.)

**Step 0 RESULT — CONFIRMED on silicon (2026-07-06, `target/serial-orin-jb2c-probe-CONFIRM.log`).**
A read-only probe (temporarily added at the end of `bpmp_tegra.rs::jb1c_ungate_xusb`, after the
JB2a survey; run on two power-cycles, captured, then **reverted to keep the JB2b arc clean at
`5fc51fe`** — JB2c re-adds it as its first writes, or skips it since the answer is now known) read
padctl cleanly — no EL3 fault, proving the block is safe to touch — and the values (identical across
both boots) nail the root cause and *refine* the fix:
```
USB2_PAD_MUX=0x00000055   USB2_PORT_CAP=0x00000111
OTG_PAD0 CTL0=0x26cc88d1  OTG_PAD1 CTL0=0x26cc88d0  OTG_PAD2 CTL0=0x26cc88d1  OTG_PAD3 CTL0=0x26cc88e0
   (all four: PD b26=1, PD_ZI b29=1, TERM_SEL b25=1;  CTL1 PD_DR b2=1)
BIAS_PAD_CTL0=0x060e0b38 (BIAS_PAD_PD b11=1)   BIAS_PAD_CTL1=0x0451e8df (PD_TRK b26=1)
```
- **Every pad power-down bit is SET**: all 4 USB2 OTG pads powered down (PD+PD_ZI+PD_DR) *and* the
  shared bias pad powered down (BIAS_PAD_PD + PD_TRK). This is the reset/torn-down state → no port
  trains → CCS never asserts. Root cause proven, not inferred.
- **Refinement vs the predicted sequence**: `PAD_MUX=0x55` = XUSB routing already set for all 4 ports
  (2-bit field per port, 0b01=XUSB); `PORT_CAP=0x111` = ports 0/1/2 already HOST (2-bit field at n*4,
  0b01=HOST), port 3 disabled. **The firmware left routing + capability intact and ONLY powered the
  pads down.** So Step-1 sub-steps 2 (`PAD_MUX`) and 3 (`PORT_CAP`) are idempotent no-ops on this
  silicon; the load-bearing fix is the **pad power-up**: clear PD/PD_ZI (CTL0) + PD_DR (CTL1) on the
  HOST pads, and the bias-pad power-up + tracking (steps E–H). Program the HOST-capable pads (0,1,2 —
  the hub upstream is pad 1 per Linux, but powering all three HOST pads covers whichever physical
  connector the device lands on). Leave pad 3 (disabled) alone.
- **De-risks the JB2c writes**: same always-powered block the probe read without fault, so the
  power-up writes are not a new EL3-fatal class.

**Step 1 (the fix).** Program pad n=1 (the hub upstream), all offsets from `P=0x3520000`, sequence
verbatim from Linux `drivers/phy/tegra/xusb-tegra186.c` (`tegra234_xusb_padctl_soc`):
1. Enable `TEGRA234_CLK_USB2_TRK` via BPMP MRQ_CLK (bias-pad tracking clock).
2. `PAD_MUX` (P+0x004): set port-1 field to XUSB (`0x1 << 2`).
3. `PORT_CAP` (P+0x008): set port-1 cap to HOST (`0x1 << 4`).
4. `OTG_PAD1_CTL0` (P+0x0C8): clear `PD_ZI` (bit29), set `TERM_SEL` (bit25), `HS_CURR_LEVEL`=0 ok
   for first light (fuse-cal is a later refinement).
5. `OTG_PAD1_CTL1` (P+0x0CC): clear `TERM_RANGE_ADJ` (bits[6:3]) + `RPD_CTRL` (bits[30:26]).
6. `BIAS_PAD_CTL1` (P+0x288): `TRK_START_TIMER`=0x1e (bits[18:12]), `TRK_DONE_RESET_TIMER`=0x0a
   (bits[25:19]).
7. `BIAS_PAD_CTL0` (P+0x284): clear `BIAS_PAD_PD` (bit11), `HS_DISCON_LEVEL`=0x7 (bits[5:3]);
   udelay(1).
8. Run tracking: clear `USB2_PD_TRK` (P+0x288 bit26); poll `TRK_COMPLETED` (bit31) with a ~100 µs
   budget (proceed on timeout); write `TRK_COMPLETED` back; clear `CYA_TRK_CODE_UPDATE_ON_IDLE`
   (P+0x28C bit31); udelay(2).
9. The two clears that light the port: `OTG_PAD1_CTL0` clear `USB2_OTG_PD` (bit26);
   `OTG_PAD1_CTL1` clear `USB2_OTG_PD_DR` (bit2).
10. **VBUS: do nothing** — `vdd_5v0_sys`/`vdd_1v1_hub` are always-on; **do NOT** set `VBUS_OVERRIDE`
    (P+0x360, a device-mode fake, wrong for a host port); **do NOT** assert `RESET_XUSB_PADCTL`
    (would wipe all padctl state).

Then re-run the JB2b survey: CCS should assert + PLS advance out of RxDetect on plug, and the
existing enumeration pump takes over → keyboard first light at EL1.

**STOP tripwire (unchanged from JB2b):** if, after the pads come up, PORTSC goes healthy (CCS=1)
but enumeration times out, that's the NISO1 SMMU `GBPA=ABORT` DMA-drop signature — an honest STOP
report and a *different* arc, not a padctl bug. **Zero-code alternative Peter can try meanwhile:**
roll back to the pre-update firmware slot (restores the old always-on handoff); no Jetson UEFI
menu USB/XHCI toggle exists.

### JB2c — landed + ✅ METAL: pads powered up (CCS=1), enumeration blocked by the predicted SMMU DMA-drop (2026-07-06)

Implemented 2026-07-06 as `xusb_tegra::jb2c_padctl_powerup(chan)` (the padctl MMIO) + a one-line
`bpmp_tegra::jb2c_usb2_trk_clk(chan)` (step 1, the tracking clock over the proven JB1 BPMP channel),
called from `main.rs` inside the `chan` scope right after `jb1c_ungate_xusb` sets `xusb_alive` —
**pre-drop at EL2, before the JB2b attach surveys the ports** (the JM6b drop terminus is untouched).
Every offset/bit was re-verified against live mainline Linux `drivers/phy/tegra/xusb-tegra186.c`
(`tegra186_utmi_{bias_,}pad_power_on`, tegra234 SoC data) 2026-07-06 and cross-checked bit-for-bit
against the Step-0 silicon readback — which confirmed *every* target value the firmware left
pre-power-down (`TRK_START_TIMER` already `0x1e`, `TRK_DONE_RESET_TIMER` already `0x0a`,
`HS_DISCON_LEVEL` already `0x7`, `PD2`=0, HS_CURR_LEVEL a real fuse value) — so the sequence is
idempotent on the config fields and load-bearing only on the power-down bits.

**★ One constant correction the verification caught:** `TEGRA234_CLK_USB2_TRK = 165`
(dt-bindings/clock/tegra234-clock.h line 323, `165U`) — an early draft eyeballed `349`, which is
wrong. Corroborated by the JB0 PWM3 clk=107 / reset=70 IDs reading cleanly out of the same headers.

Sequence (base `P = 0x3520000`; HOST pads 0/1/2, pad 3 disabled left alone; VBUS untouched):
1. BPMP `MRQ_CLK` enable `CLK_USB2_TRK` (165).
2–3. `PAD_MUX`(+0x004)→XUSB / `PORT_CAP`(+0x008)→HOST, RMW — idempotent no-ops (already `0x55`/`0x111`).
4–5. per-pad `OTG_PADx_CTL0`(0x88+x·0x40): clear `PD_ZI`(b29), set `TERM_SEL`(b25) [HS_CURR_LEVEL
   left = the firmware's resident fuse value, better than the dossier's "0 ok"]; `OTG_PADx_CTL1`
   (0x8C+x·0x40): clear `TERM_RANGE_ADJ`[6:3] + `RPD_CTRL`[30:26].
6–7. `BIAS_PAD_CTL1`(0x288): `TRK_START_TIMER`=0x1e, `TRK_DONE_RESET_TIMER`=0x0a; `BIAS_PAD_CTL0`
   (0x284): clear `BIAS_PAD_PD`(b11), `HS_DISCON_LEVEL`=0x7 [HS_SQUELCH left as firmware set it];
   `udelay(1)`.
8. `BIAS_PAD_CTL1` clear `USB2_PD_TRK`(b26) → start tracking; poll `TRK_COMPLETED`(b31) ~200 µs
   (warn-only, proceed on timeout); **W1C** it (write 1 to b31); `BIAS_PAD_CTL2`(0x28C) clear
   `CYA_TRK_CODE_UPDATE_ON_IDLE`(b31) [tegra234 `trk_hw_mode=false` → `USB2_TRK_HW_MODE` b0 stays 0];
   `udelay(2)`.
9. per-pad clear `USB2_OTG_PD`(b26, CTL0) + `USB2_OTG_PD_DR`(b2, CTL1) — the two clears that light
   the port.
10. VBUS: nothing (rails always-on; never `VBUS_OVERRIDE`, never `RESET_XUSB_PADCTL`).

All writes are **read-modify-write**, so the firmware's resident per-die fuse calibration survives in
the fields the sequence doesn't name. **Two deliberate parity deltas vs mainline, both non-blocking
for first light** (confirmed by the reconciliation pass): (a) Linux disables the trk clock after
tracking — we leave it on (harmless once `TRK_COMPLETED` is W1C'd; one fewer MRQ on the scarce
attended boot; avoids an unverified `CMD_CLK_DISABLE`); (b) Linux programs `HS_SQUELCH_LEVEL` from a
fuse read — we preserve the firmware's value (soft; affects HS receiver margin, not whether the pad
powers up). If first light fails *specifically* on signal integrity, folding in the fuse read (via
the FUSE `USB_CALIB` register) is the refinement — but the pads powering up does not depend on it.

**★ METAL RESULT (Peter-attended, 2026-07-06, capture `target/serial-orin-jb2c-metal.log`).** The
padctl bring-up ran clean on Orin silicon and **fixed the pad-teardown regression — the pads power up
and the ports train**, which is exactly what JB2c set out to do:
```
:: tegra: JB0 — fan ON (~40% duty): CSR<-0x80660000 (readback 0x80660000) -> PASS ::   (JB0 reconfirmed)
:: tegra: JB1c — XUSB ALIVE: xHCI v1.20 ... USBSTS=0x00000001 -> PASS ::
:: tegra: JB2c — USB2_TRK clk 165 enable -> err=0 ::                                    (the 349->165 fix, err=0)
:: tegra: JB2c — padctl @0x3520000 pad power-up (pads 0/1/2); pre PAD_MUX=0x55 PORT_CAP=0x111 ::
:: tegra: JB2c — bias pad up, tracking COMPLETED (BIAS_CTL0=0x060e0338 CTL1=0x0451e8df) :: (PD b11 cleared, tracking one-shot completed — no timeout)
:: tegra: JB2c — pad 0 up: CTL0=0x02cc88d1 CTL1=0x00101000 (PD/PD_ZI/PD_DR -> 0) ::      (0x26cc88d1 -> 0x02cc88d1: PD b26 + PD_ZI b29 cleared, TERM_SEL + HS_CURR_LEVEL preserved)
:: tegra: JB2c — pad 1 up: CTL0=0x02cc88d0 ... ::  pad 2 up: CTL0=0x02cc88d1 ...
:: tegra: JB2c — padctl USB2 pad power-up done -> PASS ::
```
**No EL3 fault** on any padctl write (Step-0's "always-powered, safe to write" held). Then the JB2b
survey showed the **dead ports come alive**: `port 6 [usb2] 0x00000e03 CCS=1 PED=1 PLS=0(U0) sp=3(HS)`
and `port 7 [usb2] 0x00000603 CCS=1 PED=1 PLS=0(U0) sp=1(FS)` — both were the dead `0x000002a0`
(CCS=0, RxDetect) before JB2c. Devices **connect, reset, and train to U0.** JB0 fan reconfirmed; boot
finished clean (JM6b drop → EL1 → CAPSTONE 6/6).

**★ …but enumeration is blocked one layer deeper — the predicted STOP tripwire fired.** With the pads
healthy (CCS=1), every port stalls at `ENABLE_SLOT` with **`watchdog-timeout, code 0`** (no completion
event), and the driver logs **`reset change latched but no event was delivered; polling fallback`** on
every port reset. Across the whole 60 s window: **0 command-completion events, 0 transfer events, 6
"no event delivered" fallbacks.** The controller runs and drives the ports (all MMIO / link-internal —
no system-memory DMA), but **cannot write a single event or completion into the command/event rings in
RAM.** That is a total controller→memory DMA-write failure = **exactly the NISO1 SMMU `GBPA=ABORT`
DMA-drop signature this dossier predicted** (UEFI left the XUSB stream-id's SMMU translation as
ABORT/stale, so the controller's ring DMA is dropped). It is **NOT a padctl bug and NOT a JB2c bug** —
JB2c is a metal success at the pad layer. The same shared xHCI driver enumerates fully on x86 + QEMU
virt, so the ring programming is correct; only the Tegra DMA path is dead.

**⇒ NEXT ARC (fresh session, don't stack): the XUSB SMMU/DMA path** — program the NISO1 SMMU (or
verify/relax `GBPA`, install a bypass/identity stream-table entry for the XUSB stream-id) so the
controller's ring DMA lands. The `dma-coherent` DTB prop was honored at attach; the failure is the
translate/abort stage, above coherency. Differential for that arc to rule out: (1) SMMU GBPA=ABORT /
missing STE for the XUSB SID (leading hypothesis); (2) an IOVA≠PA offset the SMMU imposes vs the raw
PAs we hand the controller. **Zero-code alternative Peter can try meanwhile:** roll back to the
pre-update firmware slot — the old always-on handoff left both the pads *and* the DMA path programmed,
which is how JB2a saw ports CONNECTED and (per the JB2b verdict) enumeration further along.

**Gate (2026-07-06):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; virt
GICv3 `test-arm` JC3 drop → CAPSTONE 6/6 (VBAR_EL1=0x7c389800, shifted by JB2b's binary growth, not
this arc); Pi `kernel8-test` all M6/U4/U5 verdicts + CAPSTONE 6/6; x86 `test` MISSION SUCCESS;
`esp-jetson` links (kernel.elf 220,584 B — healthy, not the ~355 KB corrupt-bloat signature); metal
boot as above. The whole diff is inside `#[cfg(feature = "tegra")]` code (`bpmp_tegra.rs`,
`xusb_tegra.rs`, and the `tegra_early_stop` hunk in `main.rs`), so **non-tegra binaries are
byte-identical by construction** (the QEMU logs are unchanged).

### JB3 (probe half) — the NISO1 SMMU is a dual MMU-500, and the probe reads how the XUSB stream dies (2026-07-07)

**Phase-0 research verdict (Campaign 2; every claim verify-on-device at the probe boot).** The
JB2c dossier's "GBPA/STE" language assumed SMMUv3 — the silicon says otherwise. Mainline
`tegra234.dtsi` (corroborated for L4T r36): `usb@3610000 { iommus = <&smmu_niso1
TEGRA234_SID_XUSB_HOST>; }` with `TEGRA234_SID_XUSB_HOST = 0x0e` (`dt-bindings/memory/
tegra234-mc.h`), and **`smmu_niso1` is `"nvidia,tegra234-smmu", "nvidia,smmu-500"` — a dual ARM
MMU-500 (SMMUv2)** at `0x0800_0000` + `0x0700_0000` (two fabric-interleaved instances, the
Tegra194 pattern; Linux broadcasts writes to both). Both bases sit in the GiB-0 Device block
`mmu_tegra` already maps — no new mapping. So the v3 plan translates to v2 reality:
`sCR0.CLIENTPD` (global bypass), `sCR0.USFCFG` (unmatched stream → fault vs bypass), and
`SMR[n]`/`S2CR[n]` stream matching. The board's own MB2 log (`serial-orin-jb2c-metal.log`)
shows `Program NV master stream id` → `SMMU external bypass disable` → `SMMU init` at t≈0.18 s —
the boot chain configures this block, UEFI's USB boot DMA then works through it, and the
ExitBootServices teardown strands us one layer below JB2c's pads.

**The probe (read-only, `smmu_tegra::jb3_probe` + `jb3_faults`, wired around the JB2b attach in
`tegra_early_stop`).** `fdt_tegra::xusb_iommu` resolves the LIVE firmware DTB's `iommus`
(phandle → SMMU node path, `reg` bases, bounded-ASCII `compatible`, the SID) with a
researched-values fallback that says so. Pre-attach, per instance: `sCR0` (CLIENTPD/USFCFG/
EXIDENABLE decoded), `IDR0/1/2/7`, the fault set, then every VALID `SMR[n]`+`S2CR[n]` with an
explicit `*MATCHES-XUSB*` verdict (`(sid ^ ID) & ~MASK == 0`). Post-attach (after the
ENABLE_SLOT watchdogs): `sGFSR`/`sGFAR`/`sGFSYNR0/1` — **the silicon names the faulting
StreamID itself.** No writes this boot (JX1 discipline: announce each instance before first
touch; GFSR is not even W1C'd — boot 2 owns clearing).

**The differential the dump settles → the boot-2 fix ladder:**
1. USFCFG=1 + no SMR matches 0x0e → unmatched-stream abort ⇒ claim a free SMR (VALID, ID=0x0e,
   MASK=0) + `S2CR.TYPE=bypass`, both instances (preferred over clearing USFCFG, which widens
   every unmatched stream).
2. An SMR matches with `S2CR.TYPE=translate` → UEFI's translation context, page tables dead
   since ExitBootServices ⇒ flip that S2CR to bypass (both instances).
3. Match with `TYPE=fault` → explicit kill ⇒ same flip.
4. CLIENTPD=1 or everything permissive → the drop is NOT this SMMU (differential moves to the
   MC-level bypass kill / IOVA≠PA) — an honest STOP, not a guess.
   Residual risk named up front: if the MB2 "external bypass disable" is an MC-side abort of
   ALL non-translated traffic, the bypass fixes (1–3) won't land and the arc needs a real
   identity translation context — that outcome would show as: probe fix applied cleanly, GFSR
   silent, yet ENABLE_SLOT still watchdogs.

Gate (probe milestone): `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches;
clean-build `esp-jetson` links `kernel.elf` = 227,752 B (healthy band; the growth is this
probe); all changes `tegra`-gated ⇒ non-tegra binaries byte-identical by construction. Metal
next: boot 1 = this read-only dump, Peter-attended.

### JB3 — LANDED 2026-07-07 (12-boot Peter-attended bench day): FIVE torn-down layers found + restored; root cause pinned = the XUSB Falcon is halted/locked by the EBS teardown

**The single biggest finding: the JB2c dossier's "one SMMU register" model was wrong — the
JetPack-6 ExitBootServices teardown tears down the ENTIRE controller-to-memory path, layer by
layer.** Each boot named the next layer (the probe design made the silicon self-diagnosing:
USFCFG=1 turned silent drops into logged fault syndromes with stream IDs). All restorations are
in `tegra_early_stop`, in order, `tegra`-gated. Capture: `~/serial-orin-jb3-probe.log`.

| Boot | Found | Restored |
|---|---|---|
| 1 | v2 SMMU pair (dual MMU-500 — NOT v3; DTB-confirmed sid=0xe → `iommu@8000000`): `CLIENTPD=1`, 0 SMRs, 0 faults — client port SHUT, silent swallow | read-only probe |
| 2 | NS writes take; USF fault names **sid `0x80e`** (event-ring writes; GFAR = our ring) | client port on + USFCFG=1 + SMR exact-0xe |
| 4 | cmd-ring reads emit **`0xc0e`**, inst1 sees **`0x100e`** — per-class SID decorations | SMR mask bit 11 |
| 5 | v2 pair fully clean, DMA still dead ⇒ killer downstream | SMR MASK=0x7f00 (low byte 0x0e = XUSB only) |
| 6 | census: NO SMMUv3 exists in the firmware DTB (3× MMU-500 only); MC err regs static | discrimination only |
| 7 | identity S1 translation clean (CB0: const 4K L1, 512×1GiB, WBWA+ISH, T0SZ=25) — still dead | S2CR translate → CB0 |
| 8 | `dc civac` experiment: **event ring truly empty in DRAM** — coherency ruled OUT | (experiment, shared driver, tegra-gated) |
| 9 | **MC stream-ID overrides ALL FOUR = 0** (EBS-cleared; Linux reprograms every boot) | HOSTR/HOSTW ← 0xe (rb ✓) |
| 10 | **FPCI wrapper wiped**: CFG_1 io/mem/busmaster = 0, BAR0 = 0xc | CFG_1 ← 0x7, BAR0 restored (field is 128K-granular) |
| 11 | **BAR2 wiped too**; ARU IFRDMA/STREAMID_FIELD = 0 (SID field refuses NS writes); mailbox HW alive (owner claim takes) but **firmware never ACKs** MSG_ENABLED | CFG_7 ← 0x3650000 |
| 12 | **Falcon CPUCTL/BOOTVEC read 0xffffffff via CSB** (dead window / lockdown) while the FW-header ioctl returns sane data (fw resident, codesize 0xc85f) — restart write has no effect | — |

**Verdict: every fabric layer (SMMU, MC SID, FPCI, ARU routing) is restored and verified
fault-free; the xHC command engine itself — the Falcon microcontroller — is halted and locked
against NS revival.** Ports train (link HW is autonomous); nothing DMAs because the engine that
would do it is not running. Honest STOP per the brief ("a probe day that pins the root cause is
a landable arc" — this pinned it five layers deeper than the dossier's hypothesis).

**Next (JB4, Fable/Peter-led):** (a) Peter's zero-code lever first — the pre-update firmware
slot (its gentler EBS exit leaves the Falcon running; all of JB3's restorations remain in the
boot chain and are still required/valid); (b) else Falcon revival: MRQ_RESET the XUSB
partition + falcon cold-start + firmware reload from the resident image (research arc: t234 fw
carveout, CSB lockdown semantics, possibly EL3/BPMP-mediated). The five restorations are
prerequisites either way — none of this work is wasted.

Gate (arc close): `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; every
QEMU suite unaffected (all changes `tegra`-gated ⇒ non-tegra byte-identical by construction);
clean `esp-jetson` healthy at each boot (final 243,248 B); 12 attended metal boots as above.

### JB4-prep — the Falcon revival dossier (OFFLINE: research + compile-gated code + the bench plan; the attended bench follows)

**Scope.** JB3 pinned the root cause: every fabric layer restored + fault-free, the XUSB **Falcon**
(the xHC command engine) halted/locked — CSB `CPUCTL`/`BOOTVEC` read `0xffffffff`, firmware resident
(`codesize 0xc85f`), the mailbox unACKed. This arc is **strictly offline** — research + dormant
revival code + a bench plan — no metal, no flashing, no serial. It answers the four unknowns from
primary sources, ships a compile-gated `jb4_falcon_revive()` behind `JB4_ENABLE = false`, and lays
out a boot-by-boot bench (cheapest fork first). The Peter-attended bench (JB4 proper) runs it later.

**The four unknowns — answered (per-claim confidence + sources).**

1. **Why is the CSB window dead (`0xffffffff`)?**
   - *The BAR2 CSB aperture UnaOS uses is CORRECT — not a wrong-register bug.* **[HIGH]** Mainline
     `drivers/usb/host/xhci-tegra.c` `tegra234_ops` routes CSB through **BAR2** (`.csb_reg_readl =
     bar2_csb_readl`): page-select `XUSB_BAR2_ARU_C11_CSBRANGE = 0x9c`, data base
     `XUSB_BAR2_CSB_BASE_ADDR = 0x2000` — byte-for-byte what `jb3_falcon`/`csb_r` use, and the same
     BAR2 the FW-header ioctl (`0x1000`→`0x1c`) and mailbox (`0x004..0x010`) read *successfully*. So
     the Falcon core behind the CSB bus is simply not answering; the wrapper/ARU around it is fine.
   - *A "missing Falcon clock/reset the DTB lists but JB1c skipped" is ruled out.* **[HIGH]**
     `tegra234.dtsi` `usb@3610000 { clocks = … }` already contains `TEGRA234_CLK_XUSB_FALCON` (269,
     "xusb_falcon_src") and `TEGRA234_CLK_XUSB_CORE_HOST` (267, "xusb_host"), both of which JB1c
     enables (they are early in the list, inside the 8-slot cap); the node has **no `resets`
     property** (reset is folded into `power-domains = XUSBC(12), XUSBA(10)`, which JB1c powers ON).
     So the Falcon is already clocked *and* powered when the CSB reads all-ones.
   - *Leading explanation (verify-confirmed): the Falcon CORE is halted / not executing, so its
     internal Config-Space-Bus slave floats all-ones — while the ARU wrapper (mailbox, FW-header
     ioctl) outside the core keeps answering.* **[HIGH]** The adversarial pass REFUTED both the
     "missing Falcon clock" and the "wrong CSB routing" hypotheses (clock 269 is already enabled by
     JB1c; the routing is byte-for-byte mainline). The EBS teardown left the core halted; whether a
     power-cycle re-runs its on-chip self-boot is the central bench question (unknowns 2–4).
   - *edk2-nvidia teardown mechanism: the device-discovery framework's ExitBootServices handler,
     NOT `PcieControllerDxe`.* **[MEDIUM]** A candidate — `PcieControllerDxe.OnExitBootServices`,
     which toggles PCIe `PERST#` (`XTL_RC_MGMT_PERST_CONTROL_PERST_O_N`) on nodes with the
     `nvidia,uefi-exit-reset` DTB property — was VERIFIED against source and **ruled out**: that
     driver "manages only PCIe root-complex controllers, not XUSB/USB" (`controller@141xxxxx`,
     NVMe/M.2). The Tegra234 XUSB host is a standalone BPMP-power-gated block whose "FPCI" is an
     *internal fake-PCI* space — not on the system PCIe root complex — so a root-port `PERST#` never
     reaches it (and the observed teardown is clocks/SMMU/MC-SID/FPCI/ARU/Falcon, not a `PERST#`
     pulse). The real teardown is the edk2-nvidia **device-discovery driver framework**: at
     ExitBootServices it undoes each managed non-discoverable device's `AutoEnableClocks` /
     `AutoResetModule` bring-up — the SAME mechanism this repo's JB0 (fan PWM clock+reset) and JB2c
     (USB2 pads) dossiers already pinned, here extended to the XUSB Falcon. This also explains why
     JB1c's clock+power re-enable does not revive it: re-applying a clock does not restart a Falcon
     CPU that stopped when its clock was pulled — only a power-domain cycle re-runs the boot-ROM/IFR
     self-boot (unknown 2). (Verified 2026-07-07 against `PcieControllerDxe.c`; the exact
     device-discovery source file was not pinned, but the mechanism is corroborated by JB0/JB2c.)

2. **The full reset path — would an `MRQ_RESET` assert/deassert clear the lockdown, and what does it
   wipe?**
   - *There is no separate XUSB-host/Falcon reset id on t234.* **[HIGH]**
     `dt-bindings/reset/tegra234-reset.h` exposes only `TEGRA234_RESET_XUSB_PADCTL` (114) and the
     `PEX_USB_UPHY_*` lane resets (146–162); `usb@3610000` carries no `resets`. On t234 the
     host/SS/Falcon reset is owned by the **power domain** (`MRQ_PG`) — Linux acquires the
     `xusb_host`/`xusb_ss` reset controls only when power-domains are *absent*.
   - *So the only Falcon-reset lever is a power-domain cycle (`MRQ_PG` OFF→ON) — the revival path.*
     **[HIGH mechanism / UNCERTAIN outcome]** A partition power-cycle resets the Falcon and clears
     its *volatile* IMEM, but the firmware **image** persists in the DRAM carveout — so on power-up
     the Falcon boot-ROM re-runs its carveout→IMEM self-boot (the Linux ELPG-resume mechanism:
     power-cycle, then wait `USBSTS.CNR`). It is NOT a one-way door — but whether that self-boot
     actually runs post-EBS on bare metal is the open question the verify pass rated **UNCERTAIN**
     (MB2 is gone at runtime, so no software reload; it rides entirely on the Falcon's boot-ROM).

3. **Firmware reload — where does the image live, and what is the t234 load path if IMEM is lost?**
   - *On t234 the OS does NOT — and cannot, non-secure — reload the xusb firmware.* **[HIGH]**
     `tegra234_soc.firmware = NULL`. `tegra_xusb_load_firmware()` branches
     `if (!soc->firmware) → tegra_xusb_init_ifr_firmware()`, which only **waits for the Falcon and
     reads the timestamp** — it loads no binary. The firmware is loaded by the bootloader (IFR,
     in-field-recovery firmware mode). The older ROM-loader DMA path (`tegra_xusb_load_firmware_rom`:
     `XUSB_CSB_MP_ILOAD_*` + `L2IMEMOP` DMA from a firmware buffer) is **not used on t234**.
   - *The Falcon AUTO-BOOTS its resident image; the OS just waits for `USBSTS.CNR` to clear.*
     **[HIGH]** `tegra_xusb_init_ifr_firmware()` polls xHCI `USBSTS.CNR` (up to 200 ms) for the
     Falcon to self-boot — it writes no CSB. The firmware image lives in the **CARVEOUT_XUSB DRAM**
     region MB2 authenticated at cold boot (persistent — do NOT read it out-of-band, it MC-RAS-
     faults); Falcon IMEM is volatile. So "no NS reload" does NOT mean "strand": the image survives
     in DRAM, and a clean power-up re-runs the Falcon boot-ROM's carveout→IMEM self-boot. The manual
     CSB `BOOTVEC`+`STARTCPU` restart is the WRONG path for t234; the real re-init lever is to make
     the Falcon re-run that self-boot — a power-domain cycle (unknown 2) or Peter's firmware-slot
     rollback (boot 0). Success is measured by `USBSTS.CNR` clearing, never by a CSB read.
   - *The old ROM-loader NS reload (`tegra_xusb_load_firmware_rom`: `ILOAD`/`L2IMEMOP` DMA) is
     unavailable on t234.* **[HIGH, verify-REFUTED as a lever]** It exists only for chips WITH
     `.firmware`, and every step rides the (dead) CSB window — so it can neither run nor is it the
     t234 mechanism. Not implemented.

4. **The BPMP angle — any MRQ that restarts/unlocks/reloads XUSB firmware?**
   - *No dedicated xusb/falcon MRQ.* **[MEDIUM]** The BPMP ABI (`soc/tegra/bpmp-abi.h`) carries no
     MRQ that reloads or unlocks the xusb Falcon; the xusb-relevant levers are the generic
     `MRQ_CLK` (enable), `MRQ_PG` (power-domain state), and `MRQ_RESET` (assert/deassert/module),
     plus `MRQ_UPHY` for the PHY lanes (not the Falcon). So the BPMP path reduces to: re-assert
     clocks/power (a no-op witness) or a `MRQ_PG` partition cycle (the revival lever) — no
     BPMP-mediated firmware reload for NS software to call (the verify pass confirmed MB2, which owns
     the reload, is a cold-boot applet absent at runtime).

**The verdict this dossier reaches.** The aperture is right; the Falcon is already clocked + powered;
the dead CSB is a halted CORE (not a clock/routing/reset the usual levers reach); and on t234 the
Falcon self-boots its resident carveout image (the OS just waits for `USBSTS.CNR`). So a non-secure
kernel cannot "restart" the Falcon by poking CSB. The one lever that re-runs its self-boot is an
`MRQ_PG` partition OFF→ON (the ELPG-resume mechanism) — the best code lever, but rated **UNCERTAIN**
by the verify pass post-EBS on bare metal (MB2 is gone at runtime; success rides on the Falcon's own
boot-ROM). The cheapest lever overall is Peter's zero-code firmware-slot rollback. The bench settles
it in order, cheapest first, with `USBSTS.CNR` clearing as the single success signal.

**The revival code (compile-gated + DORMANT).** In `xusb_tegra.rs` / `bpmp_tegra.rs`, all under
`feature = "tegra"` (⇒ out of every QEMU regression; non-tegra binaries byte-identical) AND a
run-time `const JB4_ENABLE: bool = false`; nothing is wired into `tegra_early_stop` this arc, so it
is dead-inert. The seat flips `JB4_ENABLE` and adds the calls at the bench.
- `xusb_tegra::jb4_falcon_revive(chan, ids)` — a NON-DESTRUCTIVE readiness check that makes **no CSB
  writes**: (1) baseline (FW header + mailbox owner + a read-only CSB `CPUCTL` witness +
  `USBSTS.CNR`); (2) a witness clock/power re-assert via `bpmp_tegra::jb4_reassert_falcon` (proves
  it is not a dropped clock); (3) a bounded `USBSTS.CNR` poll (~300 ms — the *true* signal); (4) if
  CNR clears → the `MSG_ENABLED` handshake + PASS; else an **honest STOP** naming the two real
  levers. The seat runs it standalone (boot 1, expected STOP) AND after the boot-2 partition cycle +
  JB3 re-run (expected PASS if the Falcon re-booted).
- `bpmp_tegra::jb4_powergate_cycle(chan, ids)` — the PARTITION-RESET revival lever (boot 2,
  double-guarded `JB4_ENABLE` + `JB4_ALLOW_PG_CYCLE`): `MRQ_PG` OFF→ON of XUSBC/XUSBA with a
  `GET_STATE` bracket (proving the rail actually dropped — an ON-only is a ref-counted no-op, which
  is why JB1c never re-booted the halted Falcon). It WIPES the partition, so the seat MUST re-run the
  JB3 chain (padctl + FPCI + ARU + SMMU + MC-SID) after it, then poll `USBSTS.CNR`.
- `bpmp_tegra::jb4_reassert_falcon(chan, ids)` — the IMEM-preserving witness re-assert (clocks
  269/267/270/271 + power ON); confirmed not a fix, kept for the "not a clock" proof + the
  `FALCON_HOST`/`FALCON_SS` leaf clocks absent from the DTB `clocks` list.
The CSB `BOOTVEC`/`STARTCPU` restart and the NS `ILOAD` firmware reload are DELIBERATELY NOT
implemented — the verify pass refuted both for t234 (wrong path / runs through the dead CSB window).

**Decision tree — the bench (cheapest fork first, one question per boot; `USBSTS.CNR` clear = win).**

| Boot | Flip / run | Expected serial | Abort criteria → next |
|---|---|---|---|
| 0 (Peter, ZERO code) | Roll back to the pre-JetPack-6 firmware slot; boot UnaOS unchanged (all JB3 restorations remain required + valid) | The JB2a `PORTSC` CONNECTED lines return with the Falcon running (the gentler EBS exit leaves it alive, `CNR` already clear) → enumeration proceeds through the JB3 fabric | Ports still dead / `CNR` still set → rollback is not the lever; go to boot 1 |
| 1 (non-destructive) | `JB4_ENABLE = true`; wire `jb4_falcon_revive(&chan, &ids)` after `jb3_falcon()` in `tegra_early_stop` | `JB4 — baseline … codesize=0xc85f … USBSTS.CNR=1`; `re-enable/​re-assert … err=0`; then **`CNR still set … STOP`** (expected — proves clocks/power are not the blocker) | `CNR still set` (expected) → the halted core will not self-restart from NS; go to boot 2 |
| 2 (partition cycle — the real lever) | `JB4_ALLOW_PG_CYCLE = true`; call `jb4_powergate_cycle(&chan, &ids)`, then RE-RUN the JB3 chain (jb2c padctl + jb3 fpci/aru/smmu/mc), then `jb4_falcon_revive` again | `PG … state pre=… post-OFF=<dropped>`; `post-ON=…`; `post-cycle XUSB cap0=<alive>`; JB3 chain re-applies; then **`CNR CLEARED (Falcon up)` → `MSG_ENABLED … PASS`** | `post-OFF` shows the rail did NOT drop (ref-counted) → domain pinned, cycle is a no-op → rollback. `CNR still set` after a real drop → the Falcon did not self-boot post-EBS → rollback. |
| 3 | — (no code) | — | NS `ILOAD` reload is refuted (dead CSB + IFR part) and runtime MB2 reload is absent → there is no boot 3; the firmware-slot rollback (boot 0) is the terminal lever. |

**Security posture (for the seat's ledger when this goes live).** All JB4 code is dormant this arc
(no live surface). When enabled it performs non-secure MMIO **reads** of the Falcon CSB + BPMP
`MRQ_CLK`/`MRQ_PG`, and — at boot 2 only — a guarded `MRQ_PG` partition cycle; it makes **no CSB
writes** and weakens no protection (SMEP/WXN/page-perms/checksums untouched). The honest STOP forks
mean a Falcon that will not self-boot ends in a clean report, never a blind escalation.

### JB0 brief — turn the cooling fan back on (safety hygiene; run FIRST on Orin)

Discovered 2026-07-06: when UnaOS takes over from UEFI the **fan stops and the Orin runs hot**.
Same mechanism as JB2c — NVIDIA's `ExitBootServices` teardown disables the fan PWM's clock + reset
(the device-discovery driver's `AutoEnableClocks`/`AutoResetModule` are undone). Scoped from a
3-agent research pass (fan-PWM / thermal-safety / teardown-scope), all cross-corroborated; see the
per-claim confidences below.

**Urgency — no die-DESTRUCTION risk, but genuinely hot (correction from a metal observation).** The
Orin has an OS-independent hardware thermal net armed by NVIDIA BL31/BPMP before UnaOS runs: **103 °C
Tj** = hardware clock-throttle (50/75/87.5 % caps, no OS notified), **105 °C Tj** = a die→PMIC
failsafe that power-cycles the board (cannot be altered in software). So a dead fan can never *cook*
the silicon — worst case is a clean PMIC reset. **BUT those trips sit far above touch-temperature:
in live fire (2026-07-06, pre-JB0 boots) Peter found the heatsink too hot to hold a finger on —
~55–65 °C surface ⇒ ~70–90 °C Tj — with the fan still off and NOTHING throttling.** The board will
happily sit fan-off in that 70–90 °C band indefinitely under light bring-up load and never trip, so
the failsafe does NOT keep it comfortable — it only stops destruction. **Corrected verdict: the fan
is a real need, not mere "hygiene"; do NOT run long fan-off boots.** JB0 runs ~1 s into every Orin
boot (right after the JB1b ping), so once it is in the image the fan comes on almost immediately —
verify it spins before leaning on any long (e.g. JB2c 60 s-window) boot. The hard damage rule still
stands: never run *sustained heavy GPU/CPU load with no heatsink*.

**The fix (confidence HIGH; register model corroborated by Linux `pwm-tegra.c` AND UEFI's own
`TegraPwmDxe`, which drives this same block during boot).** Controller = **PWM3 @ base
`0x032A0000`**, channel 0, one 32-bit CSR at base+0x0. **No fan MRQ exists** (MRQ_THERMAL only
reads temps / sets trips) — the host drives the PWM directly, but the clock/reset prerequisites ride
the same BPMP transport JB1 proved. CSR fields: bit[31]=ENABLE, bits[23:16]=DUTY (n/256), bits[12:0]
=SCALE (frequency only). Ordered:
1. BPMP `MRQ_CLK`/`CMD_CLK_ENABLE` on `TEGRA234_CLK_PWM3 = 107` (0x6B). *(verify-don't-assume the ID
   against `clk-t234.h`.)*
2. BPMP `MRQ_RESET`/`CMD_RESET_DEASSERT` on `TEGRA234_RESET_PWM3 = 70` (0x46). *(verify vs
   `reset-t234.h`.)*
3. **No `MRQ_PG`** — the `pwm3` DT node has no `power-domains`; always-on rail (both the DTS and the
   UEFI teardown confirm PWM gets clock+reset only, never a powergate). So — unlike XUSB — the CSR
   aperture is **not** a gated block: **no new EL3-fatal class** (the XUSB trap was touching a
   *power-gated* block; PWM has no power domain).
4. `w32(0x032A0000, <csr>)` where `csr = ENABLE | (duty<<16)`, `duty = round(pct/100 * 256)`.
   100% = `0x81000000` (count 256=0x100 overflows the nominal 8-bit field into bit24 — the exact
   value mainline emits). **Shipped value is regulated to ~40% = `0x80660000`** (100% was deafening
   on a few-watt board — see the JB0 landed note). (UEFI's own "medium" is `0x80800000` = 50%.)
5. Pinmux is normally already applied by MB1/UEFI on the devkit — only touch `pinmux@2430000` if the
   fan stays silent after 1–4 (MEDIUM confidence).

**Mapping — already covered (verified):** `0x032A0000` (53 MiB) is inside GiB 0, which
`mmu_tegra` maps as one Device-nGnRE block (same block that reaches XUSB `0x3610000` and the BPMP) —
no MMU change. Note: CSR reads/writes appear to work even with the clock off, but the output won't
toggle until steps 1–2 run; benign (no abort), just do the ungate first.

**Scope recommendation: a tiny standalone arc, run FIRST on Orin, before JB2c** — it's the cheapest
teardown-restore (no PG, no pad re-init), it's the #1 safety item, and it keeps the fan decoupled
from JB2c's heavier pad work. Lane: tegra `arch/aarch64` files + the JB1 BPMP MRQ transport (no
shared kernel-core); doc = this file + the jetson resume note. Alternatively fold it in as JB2c's
step 0 (same lane, same transport) if you'd rather one Orin session — integrator's call; the plan
file's follow-on-arc order (`~/.claude/plans/unaos-opus-jetson.md`) should get JB0 inserted ahead
of JB2c either way.

**What else the teardown kills (the "other things to turn on" list, confidence HIGH — from
edk2-nvidia's per-driver EBS config).** Ranked by urgency: **(1) fan PWM3** — JB0 (this);
**(2) USB pads / XUSB** — JB2c; **(3) nvdisplay** — JD1 (also a powergate, `SocDisplayHandoffMode=
NEVER`); (4) PCIe (powergate) and (5) Ethernet/EQOS (clk+reset+PG) — only if/when those subsystems
are needed. **Survives EBS, no action:** PMIC/regulator rails (RegulatorDxe registers no EBS event —
voltages persist), the BPMP itself, and GIC/timer/UART (not device-discovery drivers). So at handoff
the *voltages* are all up; only on-SoC clock/reset/powergate partitions get torn down, and only the
ones a given subsystem needs must be re-asserted.

### JB0 — landed + ✅ METAL-CONFIRMED (fan spins on Orin silicon, 2026-07-06)

Implemented as `bpmp_tegra::jb0_fan_on(chan)`, called from `main.rs` the instant the BPMP channel
is proven by the JB1b ping — **before** the JB1c XUSB ungate, so cooling is restored first. It runs
the three steps above over the just-proven channel: `MRQ_CLK` enable on `TEGRA234_CLK_PWM3` (107),
`MRQ_RESET` deassert on `TEGRA234_RESET_PWM3` (70), then a PWM3 CSR write. **All three constants
verified against mainline** (`tegra234-clock.h` / `tegra234-reset.h` / `pwm-tegra.c` +
`tegra234.dtsi pwm@32a0000`): clock/reset IDs correct; the duty count = `round(fraction * 256)` at
shift 16 (100% = 256 = `0x100`, which overflows the nominal 8-bit field into bit 24 — so full-on is
`ENABLE | (0x100<<16)` = `0x8100_0000`, not the `0x80FF_0000` first guessed). Best-effort: a failed
clock/reset MRQ prints and skips (no-fan is not fatal — the hardware thermal net still protects the
die); the CSR is always-mapped + always-powered, so the write cannot EL3-fault.

**Metal result (attended boot):** the whole chain ran clean on Orin silicon — `JB0 — fan PWM3 clk
107 enable -> err=0` → `reset 70 deassert -> err=0` → `fan ON … CSR<-… (readback matches) -> PASS`
→ JB1c XUSB ALIVE → Controller Started → **CAPSTONE 6/6**. **The fan physically spun.** First run at
100% (`0x8100_0000`) was deafening (a headless bring-up board draws only a few watts), so the
shipped value is **regulated to ~40% duty: `PWM_FAN_DUTY = 0x8066_0000`** (`ENABLE | 0x66<<16`,
102/256) — confirmed on metal (`readback 0x80660000`), much quieter, ample cooling, CAPSTONE 6/6.
Retune trivially: duty = `round(pct/100 * 256)`, CSR = `0x8000_0000 | (duty<<16)`.

**We do NOT need closed-loop thermal management.** The Orin's BL31/BPMP hardware thermal net
(103 °C throttle / 105 °C PMIC cutoff, OS-independent) guarantees safety regardless of fan value,
and UnaOS's light workload (CAPSTONE / polled xHCI / keyboard pump, a few watts) sits in a stable
cool band at a fixed 40%. Varying RPM by temperature is a pure acoustics/heavy-load nicety — if ever
wanted, the cheap form is a periodic `MRQ_THERMAL` Tj read stepping the fan through 2–3 duty bands
(Linux cooling-levels style), not a control loop. Filed as a future arc, not a JB0 blocker.

**⚠ Build hazard (cost several attended boots — recorded so it doesn't recur):** an *incremental*
`esp-jetson` build produced a **corrupt kernel.elf** (355 KB vs the correct ~221 KB — a ~57% `.text`
bloat, the signature of `overflow-checks`/`debug-assertions` sneaking on). It hash-flashed to the
card faithfully but **faulted on the first instruction at EL2, before any serial** — a dead hang
right after `Bootloader Started`, indistinguishable from a firmware EBS stall. A full `./arroyo
clean` + rebuild reproduced the correct 221 KB binary and booted. **Lesson: sanity-check the kernel
size before flashing** (a 60-line change must not balloon the binary), and prefer a clean build for
metal media. Also: `./arroyo clean` wipes `target/`, which will delete an in-progress serial capture
there — keep the bridge log outside `target/`.

Gate: `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green; x86 `test` MISSION SUCCESS; virt GICv3
`test-arm` CAPSTONE 6/6; Pi `kernel8` builds; `esp-jetson` links. Changes are entirely tegra-cfg-gated
(non-tegra binaries byte-identical).

### JB5 + JB6 — the Falcon "inherit, don't revive" pivot (attended bench, 2026-07-08)

**JB5 closed the revival question — negative.** Five attended probe boots plus a full read of NVIDIA's
edk2-nvidia source established that a non-secure kernel **cannot** re-boot the halted XUSB Falcon: MB2
boots it once per cold boot from its IFR image, and that self-boot is a **one-shot, un-re-armable from
non-secure** (five register / clock / power / PG-cycle / faithful-replay families tried on silicon —
every one left `CPUCTL=0xffffffff`). UEFI's own cold-boot D→A→D power-gate dance never re-boots it
either: `DeviceDiscoveryLib` **vote-refcounts** the power gate, so a Deassert on an already-ON domain is
a vote++ with **no MRQ** and the Falcon is never actually cycled. Revival is not the lever.

**JB6 is the lever: don't revive — *inherit*.** NVIDIA's `XhciControllerDxe.OnExitBootServices` tears
the XUSB block down on a Device-Tree boot (USBCMD←0, `UsbPadCtlDxe DeInitHw`, power-gate Assert), **but
self-skips the entire teardown when an ACPI table is present** — `EfiGetSystemConfigurationTable(
&gEfiAcpiTableGuid)` succeeds → immediate `return` (source-verified). So the **bootloader installs a
minimal, spec-correct dummy ACPI 2.0 RSDP+XSDT** into the UEFI config table immediately before
`ExitBootServices` (`crates/bootloader/src/main.rs::install_tegra_acpi_shim`), runtime-gated on a
`tegra234` substring in the firmware DTB so QEMU `esp-arm` stays byte-identical. The install's blast
radius is a single callback: adding a table under `gEfiAcpiTableGuid` signals that event group, firing
only EqosDeviceDxe's `UpdateACPIMacAddress`, which walks the ACPI **SDT protocol** (never our RSDP) and
early-returns when no SDT/DSDT is present — so a zero-entry XSDT is safe (both RSDP checksums and the
XSDT are made valid regardless).

**Metal result — the teardown-skip is PROVEN (attended, 2026-07-08).** A/B of the raw XUSB handoff
state, prior no-shim boot vs the JB6 shim boot:

| Handoff signal | Prior boot (no shim) | JB6 boot (shim) |
| --- | --- | --- |
| XUSB power gates (PG 12 / 10 `GET_STATE`) | `0x0` / `0x0` — torn OFF | **`0x1` / `0x1` — ON** |
| FPCI `CFG_1` | `mem=0 busmaster=0` | **`mem=1 busmaster=1`** |
| FPCI BAR4 / BAR7 | `0x0000000c` unprogrammed | **`0x0360000c` / `0x0365000c` programmed** |

UnaOS inherits a **powered, FPCI-configured** XUSB block instead of a torn-down dead one — the
"inherit, don't revive" the whole JB1–JB6 arc chased. The shim caused zero instability (the boot ran
clean through `CAPSTONE COMPLETE — all 6 sync primitives`), and the run-F kernel path is
non-destructive (`JB5_RUN_E_REPLAY=false` retires the run-E power-cycle replay that would kill an
inherited-live block; the read-only raw-handoff witness + a `jb1c_ungate_xusb` no-op run instead).

**Two clean next arcs remain — the inherited Falcon core is halted, and enumeration is blocked
downstream.**

- **(A) Halted Falcon core.** `CPUCTL` still reads `0xffffffff`, and this is **not a code bug**:
  UnaOS's CSB access (BAR2 `ARU_C11_CSBRANGE@0x9c` page-select + `CSB_BASE@0x2000` window, `CPUCTL@0x100`)
  is byte-identical to NVIDIA's own T234 path (`UsbFalconLib.c::FalconMapReg`, gated by the comment
  `/* Set Base 2 adress, only valid in T234 & T264 */`). The `jb6_csb_sweep` probe proved the
  page-select **sticks** (write `0x1234` → readback `0x1234`; `page_rb` tracks `page_want` for every
  page) yet the CSB data window returns all-ones for every address — the ARU wrapper is alive but the
  **Falcon core behind it is unresponsive (held in reset / clock-gated)**. The `fw_hdr` path reads the
  firmware *image* header fine (it doesn't need the core executing), confirming "fw loaded, core not
  running." UEFI idled the Falcon core before EBS, independent of the teardown we skipped. JB6 thus
  moves the problem from *"revive a power-gated dead block"* (proved impossible from NS) to *"start a
  powered, halted core"* — a reset-deassert lever, a different and more tractable arc.
- **(B) XUSB StreamID mismatch.** Enumeration reaches ENABLE_SLOT then stalls with
  `event ring … writes never reach DRAM`: the SMMU stream is opened for `SID=0xe` (the DTB's XUSB iommu
  id) but the MC `XUSB_HOSTR` StreamID override reads `0x0`, so XUSB DMA is tagged SID 0, never matches
  the opened stream, and event-TRB write-backs never land in DRAM. An MC-StreamID / SMMU arc,
  orthogonal to the Falcon.

**Committed state.** The JB6 bootloader shim ships live (tegra-gated). The kernel JB5/JB6 probe
instrumentation ships **active behind `JB5_PROBE=true`** (read-only witnesses + `jb6_csb_sweep`) as the
diagnostic base for arcs A/B; the JB4 revival levers stay dormant (`JB4_ENABLE=false`,
`JB4_ALLOW_PG_CYCLE=false`); the destructive run-E replay is retired (`JB5_RUN_E_REPLAY=false`). Also
kept: the `fdt_tegra` `XusbIds.clocks` cap 8→9 fix (the `usb@3610000` node lists 9 clocks; the 8-slot
cap silently dropped `TEGRA234_CLK_PLLE`).

**Gate:** `./arroyo check` both arches green; `UNAOS_TEGRA=1 ./arroyo esp-jetson` links
(`kernel.elf` 246 KB, healthy); virt `test-arm` green (storage_slot=1, and the shim is correctly a
**no-op** on the QEMU virt DTB — non-tegra path byte-identical). Metal: JB6 teardown-skip A/B confirmed
on Orin silicon, boot clean through CAPSTONE.

**⚠ Build hazard addendum (a new flavor of the JB0 355 KB trap).** `./arroyo test-arm` calls
`prepare_aarch64` **without** `UNAOS_TEGRA=1`, so it rebuilds `kernel.elf` as the **non-tegra
QEMU-virt** kernel (~355 KB — full generic xHCI, no Tegra UART / GICv3 / JB code) and repackages it
into the same `target/aarch64_esp`. Running `test-arm` after `esp-jetson` silently clobbers the tegra
media. **Always rebuild `UNAOS_TEGRA=1 ./arroyo esp-jetson` after any `test-arm`, and size-check
`kernel.elf` (~248 KB tegra vs ~355 KB virt/corrupt) before flashing.** The size-check caught it here
before a wasted attended boot.

### JB7 — arc B refuted, arc A closed at the non-secure wall (attended bench, 2026-07-08)

A close read of the JB6 run-F serial (`unaos-jetson-jb6-serial.log`) settles the two "next arcs" the
JB5+JB6 section left open: **arc A (the halted Falcon core) is the only real blocker, and arc B (the
"XUSB StreamID mismatch") is a misdiagnosis** — the event ring is empty because a halted command engine
issues no DMA, not because DMA is dropped.

**Arc B, retired.** The StreamID/SMMU path is already correct, and the fault census is clean:

| Evidence (run-F serial) | Reading |
| --- | --- |
| `MC SID HOSTR/HOSTW <- 0xe: rb=0x0000000e/0x0000000e` | The MC StreamID override **sticks**. The JB5+JB6 note's "reads `0x0`" is the *pre-fix first-touch* (`jb3_open_stream`), not the operative post-`jb3_mc_sid_fix` value. |
| `inst0/inst1 OPEN: SMR[0] rb=0xff00000e S2CR[0] rb=0x00000000`, `CB0 armed` | SMR matches the decorated SIDs (mask `0x7f00`); S2CR=translate through the identity CB0; USFCFG=1. Correctly configured. |
| `inst{0,1} {pre,post-attach} faults: sGFSR=0x00000000`; CB0 `FSR` unchanged; `MC INTSTATUS=0x00000000` | **Zero faults, before and after the attach.** With USFCFG=1 a SID-mismatched write would latch a USF fault naming the SID. Nothing latches ⇒ **no XUSB DMA is ever attempted.** |

The baton's proposed arc-B fix (S2CR bypass) was moreover **already refuted on metal** (boots 5/6: bypass
matched, fault-free, DMA still swallowed — the MB2 policy "SMMU external bypass disable" refuses
untranslated traffic; the code moved to identity-translate). Arc B is not independently testable until the
Falcon runs, and its config is already in place for when it does.

**Arc A, characterized.** `CPUCTL=0xffffffff` (halted=1 stopped=1) at *every* witness stage — raw-handoff
through post-attach — and nothing moves it (`jb3_falcon`'s CSB STARTCPU restart: `CPUCTL rb=0xffffffff
(spins 100000)`, the wrong path for t234). `jb6_csb_sweep` proved the BAR2 CSB **page-select sticks**
(`page_rb` tracks `page_want` for every page) while the data window reads `0xffffffff` throughout — the ARU
wrapper answers (`BAR2[0x000]=0x00140009`) but the Falcon core behind it does not. Ports train fine
(`port 1/6/7 CCS=1 … U0` — the port state machine is hardware, independent of the Falcon), the driver
reaches ENABLE_SLOT, and the watchdog fires because the halted command engine never DMA-reads the TRB nor
DMA-writes a completion. On Tegra XUSB **the Falcon *is* the xHC command engine**; a halted Falcon means no
DMA of any kind, which is exactly the zero-fault census above.

**Two read-only probes were added (compile-gated `feature="tegra"`, run-time `JB5_PROBE`-gated → QEMU
byte-identical), wired into the raw-handoff block before any XUSB-affecting MRQ:**

- **`bpmp_tegra::jb7_clocks_query`** — MRQ_CLK `CMD_CLK_IS_ENABLED` (a pure query) for all 9 DTB clocks +
  the 4 Falcon leaf clocks (267/269/270/271). `jb1c_ungate_xusb` only proves each ENABLE *acked* (err==0);
  this reports the clocks' *actual* state.
- **`xusb_tegra::jb7_csb_cfg_read`** — a BAR2-vs-alternate-CFG CSB cross-read of `CPUCTL`. Added, then
  **removed** after the metal boot proved it EL3-fatal (see below); the retirement note stands in
  `xusb_tegra.rs` where the function was.

**Attended bench (2026-07-08, native microSD) — three findings close arc A:**

- **The alternate CFG CSB aperture is EL3-fatal.** `jb7_csb_cfg_read`'s first access to the FPCI/CFG CSB
  window (`XUSB_FPCI+0x41c`/`+0x800`, the `UsbFalconLib.c::FalconMapReg` else-branch FalconUtil uses inside
  UEFI) trapped to BL31 — `Unhandled Exception in EL3`, `esr_el3=0xbe000011` (EC 0x2F = SError, an async
  CBB/fabric abort), `far_el3=0` — and killed the boot. Post-EBS on the inherited halted block that aperture
  is unrouted, so touching it is the JX1 EL3-fatal class (an SError cannot be guarded from EL2). The probe
  was **removed**; the BAR2 aperture (`jb6_csb_sweep`) is the only usable CSB path and cleanly reads
  `0xffffffff`. Two apertures behaving differently (BAR2 decodes-but-floats vs CFG unrouted) is itself
  confirmation the core is dead behind a partially-live ARU.
- **Boot-medium bisect (A2) — refuted.** Booted from the board's **native microSD slot (SDMMC)**, not a USB
  reader — the first Falcon-witness boot ever taken off a non-USB medium. `witness[raw-handoff]:
  CPUCTL=0xffffffff` all the same. The halt is **not** the USB-boot path; it is universal.
- **Clock census — core-clocked but reset-held.** All 9 DTB clocks read `=1`, including the Falcon **core**
  clock 269; the two Falcon **leaf** clocks absent from the DTB list read gated — `CLK 270 (FALCON_HOST)=0`,
  `CLK 271 (FALCON_SS)=0`. Not the lever: JB4 already enabled 270/271 on metal (`re-enable XUSB clk
  270/271 -> err=0`) and `CPUCTL` stayed `0xffffffff`. Core clock on + core halted = **held in reset**, not
  clock-gated. The clean boot ran through **CAPSTONE 6/6**.

**Verdict — arc A is at the non-secure wall.** Every non-destructive NS lever is tried or ruled out: CSB
STARTCPU (wrong path for t234), clock reassert (269 on, 270/271 enable no-ops), power/PG reassert (JB4/JB5,
retired), the boot-medium bisect (universal halt), the alt CFG aperture (EL3-fatal), and MRQ_RESET (no
`resets` on `usb@3610000`; the only BPMP reset is the retired MRQ_PG cycle). The core is held in reset by an
agent outside NS reach — UEFI's `XhciControllerDxe.Start` handling, or MB2/secure-world. Starting it needs
secure-world (BL31/MCE), a custom MB2, or a firmware-slot change — outside the executor's NS lane. This
mirrors JB5's revival verdict: **a non-secure kernel cannot start the halted XUSB Falcon.** The next jetson
XUSB swing is therefore a bootloader/UEFI-side question — can the JB6 shim also suppress
`XhciControllerDxe.Start` so UnaOS inherits MB2's still-running Falcon? — a fresh arc, or a pivot off USB.

**Gate:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` both arches green (probes compile clean, zero new
warnings); `UNAOS_TEGRA=1 ./arroyo esp-jetson` links, `kernel.elf` 254,536 B tegra (120 `tegra:` strings);
virt `test-arm` green (`storage_slot=1`, byte-identical — all JB7 code gated off). Metal: native-microSD
boot clean through CAPSTONE 6/6 with the clock census; the removed CFG probe's EL3 fault is recorded above.

### JB8 — pre-EBS Falcon witness + reconnect lever, and the IFR-autoboot discovery (loader-side arc)

JB7 closed arc A "at the non-secure wall" on the premise that starting the Falcon core needs a reset lever
NS can't reach. A source read of edk2-nvidia (r36.4.0-updates, the JetPack-6 branch this board runs, plus
main) overturns a load-bearing part of that premise:

- **Nothing in UEFI ever halts the Falcon core.** There is no STOPCPU/CPUCTL-halt anywhere in the tree. The
  only teardown levers are BPMP: the driver's `OnExitBootServices` asserts the XUSBA/XUSBC power gates
  (MRQ_PG — PG assert implies partition reset), and a *second*, framework-level EBS teardown
  (`DeviceDiscoveryOnExitBootServices`) gates clocks + asserts the DT resets (MRQ_RESET). **Both** carry the
  same ACPI-table skip, so the JB6 dummy-ACPI shim suppresses both — consistent with JB6's live-block
  inheritance.
- **T234 UEFI never starts the Falcon via CPUCTL.** In `XhciControllerDxe.Start` (after a PG
  deassert→assert→deassert cycle, padctl `InitHw`, and FPCI CFG_4/CFG_7/CFG_1 programming) it polls
  `USBSTS.CNR` for 200 ms: if CNR clears, firmware is already alive from the boot chain (MB2 — "Skipping
  powergate XUSB") and the load is **skipped**; else it runs `FalconFirmwareIfrLoad` — **IFR DMA autoboot**:
  copy the `xusb-fw` flash-partition blob (from `UsbFirmwareDxe`) into a DMA buffer of
  `EfiRuntimeServicesData` (survives EBS), then three writes to the **AO aperture** (padctl DT reg region 1):
  `AO+0x1bc IFRDMA_CFG0` ← buffer PA[31:0], `AO+0x1c0 IFRDMA_CFG1` ← PA[39:32], `AO+0x1c4
  IFRDMA_STREAMID` ← 0xE — and the Falcon's ROM engine fetches and boots the firmware itself. Plain NS MMIO
  + NS DMA; **no secure world involved**. The legacy CSB `BOOTVEC`+`CPUCTL_STARTCPU` path (what JB3 tried) is
  T186/T194-only. `Start` runs unconditionally at BDS connect (driver-binding on the DeviceDiscovery DT
  handle; DEPEX = padctl + xusb-fw protocols), not just when USB is used.

So the "reset-held core" may in fact be a **never-IFR-restarted core after the Start PG cycle** — and IFR is
an NS-reachable start lever the kernel (or loader) can pull, provided the AO aperture and a firmware image
are in hand (post-EBS, `IFRDMA_CFG0/1` should still hold the runtime-services buffer PA).

**JB8 ships the discriminating probe, loader-side** (`crates/bootloader`, runtime-gated on the tegra234 DTB
sniff — now factored into `dtb_is_tegra234`, shared with the JB6 shim; QEMU virt untouched at runtime):

- `jb8_falcon_witness("pre-EBS")` — runs immediately before the JB6 shim + `exit_boot_services`, while
  `XhciControllerDxe` still owns a live block: FPCI CFG_0/CFG_1/CFG_7, then (BAR2 self-located from CFG_7,
  never assumed) the kernel's exact JB3 CSB recipe — `CPUCTL`, `BOOTVEC`, fw codetag/codesize via the
  FW_SCRATCH ioctl — plus `USBSTS.CNR` via BAR0 (pre-EBS CNR discriminates "MB2 FW alive, load skipped"
  from "IFR load attempted"; post-halt CNR remains a stale latch, JB7). One bit decides the arc:
  `CPUCTL!=0xffffffff` pre-EBS ⇒ the kill is in the EBS window (huntable from the loader);
  `==0xffffffff` ⇒ `Start`'s own PG cycle (or earlier) left it dead and the lever is a forced re-`Start`.
- `jb8_reconnect_lever()` — **separate risk media**, bootloader feature `jb8lever`
  (`UNAOS_JB8_LEVER=1 ./arroyo esp-jetson`): `DisconnectController`+`ConnectController(recursive)` on every
  `Usb2Hc` handle, forcing a fresh `XhciControllerDxe.Start` — a fresh PG cycle + (if CNR stays set) a fresh
  IFR firmware load — at the last moment before handoff, then a `post-reconnect` witness re-read. Runs after
  the kernel/DTB are fully in memory, so tearing a USB boot volume's stack down is safe. Flash this only
  after the plain witness reads dead.

Bench matrix (attended): boot the plain media, read `JB8[pre-EBS]` off the UEFI console/serial, compare
with the kernel's raw-handoff witness. Dead pre-EBS → flash the `jb8lever` media. If the lever's
`post-reconnect` witness shows `CPUCTL` sane, the *kernel-side* follow-on is the IFR restart: read
`IFRDMA_CFG0/1` back from the AO aperture post-handoff and re-trigger autoboot (also re-examine the 0xE
IFRDMA StreamID against the retired arc-B census before writing it).

**Gate:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` both arches green; `UNAOS_TEGRA=1 ./arroyo
esp-jetson` links (`kernel.elf` 254,536 B tegra, 120 `tegra:` strings) with and without `UNAOS_JB8_LEVER=1`;
virt `test-arm` green (`storage_slot=1`, zero panics — JB8 is loader-side and DTB-gated, QEMU inert).

#### JB8 bench verdict (attended, 2026-07-08 pm, USB-reader boots — serial `unaos-jetson-jb8-serial.log`)

Five metal boots (`8513f0e` fixed a first-media crash loop: an unprogrammed CFG_7 read `0x1ff`, a sloppy
`& !0xf` mask dereferenced it — exact masks + mem-decode guard now; `504dc3c` re-based the witness on the
DT-fixed windows). Findings, in escalating order of importance:

1. **UEFI never programs the FPCI CFG BARs.** CFG_4/CFG_7 read raw garbage (`0x0180ff05`/`0x1ff`) on every
   boot, pre- and post-connect, while USB-reader boots demonstrably work — the NVIDIA driver drives the
   block purely via the DT-fixed addresses. FPCI CFG state is not a proxy for driver state; witness gating
   moved to "Usb2Hc handle exists".
2. **A plain auto-boot never connects XhciControllerDxe at all** (no Usb2Hc handle, block unpowered).
   Every previous "raw-handoff" reading is conditioned on how the boot was launched. The lever's
   connect-all handles this; `DisconnectController` on the Usb2Hc handle fails `INVALID_PARAMETER` on this
   firmware (twice, handle fresh from `locate_handle_buffer`) — unexplained, needs a follow-up (try the
   `driver_image` param, or disconnect the parent NonDiscoverable handle).
3. **⭐ `CPUCTL=0xffffffff` NEVER meant "halted core."** The decisive read: pre-EBS, driver live, xHC
   running (`USBSTS=0x10`, HCH=0, CNR=0), USB enumeration in progress — and CPUCTL/BOOTVEC *still* read
   `0xffffffff` while the fw-header ioctl answers (`codesize=0xc85f`). The Falcon is **alive and
   CSB-locked** (signed FW at raised priv level → external CSB reads float all-ones; the ARU mailbox
   services still respond). **Every "halted/reset-held Falcon" verdict from JB3→JB7 read a locked
   register, not a dead core.** JB7's "NS wall" dissolves: there was never a stopped core to start.
4. **The real failure is the DMA path, register path is fine.** At kernel time (JB6 shim active, generic
   XhciDxe's un-gated `XhcHaltHC` being the only EBS action): the kernel restarts the xHC (HCH 1→0), port
   resets complete, three ports link-train to U0 with `CCS=1 PED=1` — but the `enable-slot` command times
   out: command-ring fetches / event-ring writebacks never touch DRAM, with a **clean fault census** (JB7's
   zero-faults observation, now with the opposite meaning: DMA is *attempted* by a live engine and
   silently goes elsewhere/nowhere). The JB3 mailbox MSG_ENABLED no-ACK is the remaining witness that the
   FW may not be *servicing* requests post-handoff — discriminating "FW alive but DMA misrouted" from "FW
   idled by the EBS halt" is the next arc's first probe.

**Next-arc shape (fresh brief):** kernel-side, two probes — (a) a CPUCTL-free FW-liveness witness (mailbox
retry + FW scratch heartbeat), (b) DMA-path forensics under the live-Falcon premise: where do event-ring
writes go (SMMU S2CR/CB actually bound to stream 0xe at write time? stale UEFI SMMU context translating
IOVA≠PA? MC override vs the FW's own StreamID field) — arc B's question, reopened with arc A dead.

### JB9 — FW-liveness without CPUCTL + DMA-path forensics (kernel-side arc)

JB8's verdict reframed everything this arc stands on: the Falcon runs, CPUCTL/BOOTVEC are CSB
priv-locked reads (never a liveness witness on this firmware), and the real failure is the DMA
path — register ops healthy (ports link-train to U0, `CCS=1 PED=1`), but `enable-slot`
watchdog-times-out because command-ring fetches / event-ring writebacks never touch DRAM, with a
clean SMMU/MC fault census. JB9 ships two kernel-side probes (all `tegra`-feature +
`JB9_PROBE`-gated, QEMU byte-inert):

**A — the CPUCTL-free FW-liveness witness** (`xusb_tegra::jb9_fw_alive`), run at three points:
`raw-handoff` (after the JB5 minimal BAR2 route, before any XUSB-affecting MRQ),
`post-xhc-restart` (inside `jb2b_attach`, right after the shared driver's halt+HCRST+CNR init),
and `post-enum-attempt` (after the enumeration window closes). Each print is one verdict line
built from three CPUCTL-free signals:
1. *fw-header identity* through the ARU ioctl (the aperture JB8 proved answers on a locked core):
   codetag/codesize + `fwimg_checksum`@0x28 / `fwimg_created_time`@0x2c (mainline
   `tegra_xusb_fw_header` layout) — four coherent words a floating aperture cannot fake;
2. *scratch heartbeat*: the proven ARU range `[0x0,0x140)` swept twice ~10 ms apart — any word a
   live FW updates between sweeps is a heartbeat;
3. *MSG_ENABLED, patiently*: 5 spaced claim→send→~10 ms-ACK-poll→release attempts over ~100 ms
   (JB3's single ~200 µs try is the one datum suggesting the FW may not service requests
   post-handoff; a busy FW deserves patience before that verdict sticks).
Verdict line: `JB9-A [tag] — verdict: FW-ALIVE|FW-SILENT (hdr=… heartbeat=… mbox=… attempts=…)`.

**B — DMA forensics at enable-slot-pending time.** The pump loop inside `jb2b_attach` fires two
read-only captures at t≈200 ms and t≈5 s into the window — per the JB8 log, squarely inside an
enable-slot watchdog attempt (~340 ms each, 3 per port), i.e. while a live engine is actively
fetching a command ring that never lands. Each capture:
- `smmu_tegra::jb9_stream_dump` — the SMR matching SID 0xe and its S2CR routing on both NISO1
  instances, then the FULL context bank S2CR points at (SCTLR/TTBR0/TCR/TCR2/MAIR0 + FSR/FAR)
  with an explicit **"is TTBR0 OUR JB3 identity table?"** verdict — the prime suspect is a stale
  UEFI context translating IOVA≠PA (silent mis-landing explains the zero faults) — plus the MC
  HOSTR/HOSTW overrides + error log *at that instant*;
- `xusb_tegra::jb9_fw_sid_view` — the SID the firmware side is configured to tag: ARU
  `IFRDMA_CFG0/1`+`STREAMID_FIELD` (BAR2+0xe0/0xe4/0xe8) and the AO-side IFR-autoboot trio
  (`IFRDMA_CFG0`@AO+0x1bc, `CFG1`@+0x1c0, `IFRDMA_STREAMID`@+0x1c4 — JB8's edk2 source read),
  the AO base DTB-resolved from `padctl@3520000` reg region 1 (`fdt_tegra::xusb_padctl_ao`;
  absent ⇒ printed SKIP, never a guessed aperture);
- `xusb_tegra::jb9_ring_scan` — did the event-ring writeback land NEAR the target? The event
  ring's first four TRB slots (at-target), then a ±2 MiB RAM sweep around the ring for the
  command-completion fingerprint (TRB qword0 pointing into the command-ring page + type 33),
  clamped to the DRAM base. A hit at a wrong PA names the stale-translation delta directly.

Hazard posture: no CFG-path CSB touch (EL3-fatal, JB7), no FPCI BAR dereference (JB8 masks
lesson — the probes ride the already-routed windows), AO read only from a DTB-resolved base with
a JX1 first-touch announce line. The only writes are the mailbox handshake words `jb3_aru_probe`
already writes and the CSB page-select.

**Gate:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` both arches green; `UNAOS_TEGRA=1
./arroyo esp-jetson` links (kernel.elf 269,480 B tegra, 137 `tegra:` strings — grown from JB8's
254,536 B by the probe code, well under the ~355 KB corrupt-bloat signature); virt `test-arm`
green (`storage_slot=1`, zero panics — JB9 is `tegra`-gated, QEMU byte-inert).

#### JB9 bench verdict (attended, 2026-07-08 pm, ~10 boots — serial `unaos-jetson-jb9-serial.log`)

**Answer: OTHER — the fabric was never broken; UnaOS was breaking it.** The JB9 probes eliminated
every standing hypothesis, then a live-debug ladder (JB9b→JB9k, one lever per boot) found and fixed
the real, three-part cause. USB enumeration now works on Orin silicon: both halves of a VIA VL2109
hub enumerate (USB3 root port 1 + USB2 root port 6), device descriptors and hub bring-up complete
over real DMA into the >4 GiB event ring, and DISABLE_SLOT/ADDRESS_DEVICE/EP0 transfers all
round-trip. The chronicle:

- **JB9-A** (three points + JB9d loader-side pre-EBS): the ARU mailbox NEVER answers MSG_ENABLED
  from NS — *including pre-EBS while the FW is provably enumerating USB*. Mailbox silence means
  nothing on this firmware; the JB3 no-ACK was a red herring. The fw-header ioctl also answers
  across a true PG cycle (it reads the AO-configured DRAM buffer, not a running FW) — demoted as a
  liveness witness. The ARU scratch heartbeat found only one flaky bit (BAR2+0x18).
- **JB9-B**: SMMU bound to OUR identity CB (stale-UEFI-context hypothesis dead), MC overrides
  correct, zero faults, nothing near-target. AO IFR view: `IFRDMA_CFG0/1` hold the fw buffer PA;
  `IFRDMA_STREAMID=0x7f`. **JB9b**: the AO retag to 0xe is NS-REFUSED (even freshly post-PG-cycle);
  accepting the 0x7f class at the SMMU changed nothing — no XUSB DMA was reaching the SMMU at all.
- **JB9c**: a TRUE MRQ_PG rail drop (post-OFF=0x0 verified) neither restarts the FW nor unlocks AO —
  the t234 ROM does NOT re-run IFR autoboot on a bare PG-on; only `XhciControllerDxe.Start` ever
  loads this Falcon. ⚠ The PG cycle *destroys* the only firmware instance — never run it on an
  inherit-path boot.
- **JB9e**: a NOOP into an ALL-<4 GiB hand-programmed interrupter also lands nothing after HCRST
  (and UEFI's own event ring turns out to live >4 GiB anyway) — address theory dead.
- **⭐ JB9f (inherit-run)**: at raw handoff, bare `RS=1` on UEFI's own halted state — with NO reset —
  posts a Port-Status-Change TRB into UEFI's event ring within 200 ms. **The firmware was alive the
  whole time; the failure was in UnaOS's takeover.**
- **The three real bugs, fixed in order:** (1) **HCRST kills the inherited Falcon's service loop**
  (JB9g `JB9G_NO_HCRST`: halt-only takeover, reprogram while halted, RS=1 — xHCI-legal, no reset);
  (2) the JB3 fabric chain **mutates a working configuration** (JB9h `JB9H_SKIP_CHAIN`: skip SMMU
  re-arm/MC/FPCI/ARU/padctl/CSB-poke entirely on the inherit path — with it skipped, enable-slot
  COMPLETES: command completions land in the high event ring; the MC override reading 0x0 — arc B's
  founding "torn-down link" — is simply what the working config looks like); (3) **HCCPARAMS1.CSZ=1**:
  the Tegra xHC uses 64-byte contexts and the shared driver was hard-coded 32-byte stride — the
  cause of ADDRESS_DEVICE code-17 Parameter Error (fixed shared-driver-wide via
  `context::CTX_WORDS`, tegra=16/others=8, Peter-approved shared-file change; QEMU regression
  byte-identical). Plus two conventional driver gaps found immediately after: PORTSC speed ID 5
  (SuperSpeed+) had no MPS0 mapping (→ 512), and Full-Speed EP0 babble now learns MPS0=64 per port
  for the FSM's retry (`fs_ep0_mps64`). JB9i (DISABLE_SLOT 1..8 eviction of UEFI's stale slots) is
  retained as takeover hygiene.

**Remaining (next arc):** the hub-downstream walk only descends one level (Peter's keyboard/mouse/
storage sat behind nested VL2109 hub layers; `storage_slot=0`); port 7's FS device passes the
babble-learn then watchdogs at dev-desc (needs a fresh port reset in the retry flow); the JB4/JB5
chain code paths should be formally retired/gated for inherit-path boots; and a direct-to-root-port
keyboard boot is the quick win to demonstrate `keyboard ARMED` end-to-end.

## 4. Jetson Orin Nano headless bring-up (Arc JM2)

The Orin is brought up **headless over serial**. The only console that has ever
worked on the board is a Raspberry Pi Debug Probe on the carrier's TTL header
(pin 3 = RX, pin 4 = TX, 115200 8N1); the USB-C port never enumerated, and the
only display adapter on hand (a passive DP→mini-HDMI) drives nothing (the Orin's
DisplayPort needs a native or active sink). So the boot path must reach a live
serial console with **no framebuffer** — which is what JM2 makes true end to end.

### Boot flow (esp-jetson media)

`UNAOS_TEGRA=1 ./arroyo esp-jetson` builds a single-FAT32 USB stick
(`EFI/BOOT/BOOTAA64.EFI` + `kernel.elf`) that NVIDIA's on-board UEFI (JetPack 6 /
L4T r36) launches from the UEFI Shell (`connect -r`, `map -r`, then
`FSx:\EFI\BOOT\BOOTAA64.EFI`). From power-on:

1. **Bootloader** runs — confirmed on silicon at the JM1 bench (`UnaOS UEFI
   Bootloader Started`). With `UNAOS_BOOTDIAG=1` it first prints the `BOOTDIAG:`
   block (below).
2. **GOP lookup** now boots **headless** instead of returning `UNSUPPORTED` —
   JM1's dead end (`Failed to get GraphicsOutput handle: NOT_FOUND`). See
   "Headless bootloader" below.
3. Kernel load + relocation + `ExitBootServices`, then `_start(boot_info)` at
   whatever EL the firmware hands off (the kernel boot-diag line reports it).
4. **`tegra` kernel MMU + early platform stop** (`crates/kernel/src/main.rs::
   tegra_early_stop` → `arch::aarch64::mmu_tegra`): **JM3.** The UEFI-handoff tables
   map RAM but **not** the Tegra peripheral MMIO (JM2 R4: the kernel faulted on its
   first UARTC read), so the kernel FIRST installs its **own** translation regime via
   `mmu_tegra::init` — a single L1 of 1 GiB blocks mapping RAM Normal-WB (the GiBs
   derived from the firmware memory map, not hardcoded) + the low-1-GiB Tegra device
   window Device-nGnRE, plus a bounded fault vector — programmed for the handoff EL
   (EL2 primary / EL1 fallback, chosen from `CurrentEL`). Only once that switch lands
   (`SCTLR` M|C|I on) can the serial path reach UARTC. The kernel then prints the two
   `:: tegra: mmu … ::` lines, its banner + boot-diag line, `:: tegra: early platform
   stop (gic/timer discovery = JM4) ::`, and a serial-alive heartbeat loop
   (`:: tegra: heartbeat <n> ::`). It still does **not** bring up the GIC or generic
   timer — those walk QEMU-`virt` MMIO bases unmapped on Tegra234 (**JM4's** job).
   With no timer IRQ there is no `WFI`; the plain spin's climbing heartbeat is the
   liveness verdict. (Before JM3 the first `serial_println!` faulted on the UARTC read
   because the handoff tables did not map the Tegra peripherals — the R4 blocker this
   step removes.)

### Headless bootloader (`fb_addr == 0`)

`crates/bootloader/src/main.rs` used to `return Status::UNSUPPORTED` when the
firmware published no `GraphicsOutput` protocol — invisible on a serial-less box,
and the JM1 stop. Both GOP-acquisition `Err` arms now join the existing headless
path: `fb_addr = 0`, `fb_size = 0`, `FrameBufferInfo` zeroed with
`PixelFormat::Unknown`, `mode_action = 4`. The kernel already treats
`framebuffer_addr == 0` as serial-only (fbcon no-ops, the GUI is skipped), so a
GOP-less platform boots to a serial console instead of bouncing back to the
firmware boot picker. `mode_action = 4` reads out as `headless (no GOP protocol)`
in the boot-log mode-action match. This is the **shared** bootloader, so the x86
rMBP boots through it too — but in QEMU (x86 and aarch64 `virt`) a GOP is always
present, so the headless arms are never taken and boot logs stay byte-identical.

### `BOOTDIAG` block (`UNAOS_BOOTDIAG=1`)

An additive, knob-gated diagnostics block (`bootdiag` cargo feature) prints,
before the GOP lookup: firmware vendor + revision + UEFI spec revision; the
**GraphicsOutput handle count** via `locate_handle_buffer` (so a truly headless
SoC reads 0, rather than JM1's ambiguous `get_handle_for_protocol` NOT_FOUND);
**ConOut's device path**; and — the UART truth — the DTB `/chosen` `stdout-path`,
the node it resolves to, `bootargs`, and the `serial*`/`uart*` `/aliases` (whose
node names carry the physical MMIO base). OFF by default ⇒ byte-identical boot
logs. This is what names the Orin's real console UART for JM3. Only the aarch64
bootloader build reads the knob; the x86 bootloader (built by the `builder` crate)
is unaffected.

### UART / GOP truth table

The `tegra` serial driver's assumptions vs. what the real Orin Nano showed. R4
(2026-07-03) established GOP/firmware and the UARTC fault; **JM3 Part D
(2026-07-04) resolved the rest** once the kernel maps device memory. A claim only
where the serial capture shows it.

| Aspect            | Driver assumption (pre-metal)                            | Metal result (R4 2026-07-03 / **JM3 2026-07-04**)   |
|-------------------|----------------------------------------------------------|-----------------------------------------------------|
| GOP               | none published (headless)                                | **CONFIRMED (R4): `GraphicsOutput handles = 0 (NOT_FOUND)`** — genuinely headless |
| Firmware          | —                                                        | **CONFIRMED (R4): EDK II, fw-rev `0x00010000`, UEFI 2.7** |
| DTB config table  | firmware publishes its FDT                               | **CONFIRMED PRESENT (JM3):** with the corrected `EFI_DTB_TABLE_GUID`, `Found DTB at 0x2679df000, size: 997852` (bootloader), and BOOTDIAG read `model='NVIDIA Jetson Orin Nano Engineering Reference Developer Kit Super'`. R4's "no DTB configuration table" was the **wrong-GUID artifact** (now disproven). ⚠ BOOTDIAG's *full* parse of this 997 KB DTB panics inside `fdt-0.1.5` (`node.rs:472`) — a bootdiag-parser robustness bug the GUID fix exposed (see "JM3 result"); the header/model read is fine, and the kernel path (header-only) is unaffected. |
| Console UART base | UARTC `0x0C28_0000`, NS16550, reg-shift 2 (AON/SPE TCU)  | **CONFIRMED (JM3): UARTC `0x0C28_0000`.** Once the kernel maps it Device-nGnRE, the banner + 72 climbing heartbeats print cleanly over the header — the assumed base is the real console. |
| Handoff EL        | EL2 (QEMU `virt` / Pi UEFI)                              | **CONFIRMED (JM3): EL2** (`EL=2`); the EL2 non-VHE (`E2H=0`) MMU path worked, so the assumption held on A78AE silicon. |
| Generic-timer Hz  | unknown on silicon (62.5 MHz QEMU `virt`, 54 MHz Pi 4)  | **CONFIRMED (JM3): 31.25 MHz** (`CNTFRQ=31250000`) — a new fact, distinct from QEMU/Pi (JM4 derives the tick from it). |

### R4 metal result (2026-07-03, operator-attended)

The Orin booted the `UNAOS_BOOTDIAG=1 UNAOS_TEGRA=1 ./arroyo esp-jetson` media
over a Raspberry Pi Debug Probe on the board's TTL header (pin 3 = RX, pin 4 = TX,
115200; the USB-C port does not enumerate a console). Confirmed on silicon, in
order:

- **BOOTDIAG runs on metal:** `firmware vendor='EDK II' fw-revision=0x00010000
  uefi-revision=2.7`; `GraphicsOutput handles = 0 (NOT_FOUND)`; `ConOut has no
  DevicePath (UNSUPPORTED)`. The `no DTB configuration table published by firmware`
  line also printed but is **UNVERIFIED**: that scan used a wrong `EFI_DTB_TABLE_GUID`
  (fixed in JM3 Part 0), so it would report "no DTB table" whether or not the firmware
  actually published one — re-measured with the corrected GUID at the next attended
  boot (JM3 Part D).
- **R3 headless boot works on metal:** `No GraphicsOutput handle (…NOT_FOUND);
  booting headless (serial only)` — the bootloader proceeds PAST the old JM1 GOP
  stop, parses + loads the kernel (53 pages @ `0x25e5b4000`), and enters it.
  **R1–R3 are metal-confirmed.**
- **The kernel faults on its first Tegra UART access.** UEFI's still-resident
  handler catches it:
  ```
  Synchronous Exception at 0x25E5C52FC   (kernel_base 0x25e5b4000 + 0x112fc)
  ASSERT [ArmCpuDxe] …/DefaultExceptionHandler.c(345)  ->  Resetting
  ```
  Disassembly at that offset (in `serial::__print` → `tegra::write_byte`) shows
  the faulting instruction is `ldr w13, [x9]` with `x9 = 0x0C28_0014` — the
  `read_volatile(LSR)` of the THRE-wait, i.e. the **first MMIO read of UARTC**.
  **Root cause: the Tegra UARTC device region is not mapped in the page tables
  UEFI hands off.** The kernel runs on that map until it installs its own MMU
  (which the `tegra` early-stop build never does — it stops before `arch::init`),
  so any Tegra peripheral MMIO faults. This is an MMIO *translation* fault, not a
  wrong-base *silence*: even the correct UART base faults identically until the
  kernel maps device memory. (Full capture: `unaos/target/serial-orin-jm2.log`.)

**JM3 hand-off:** the Orin kernel needs its own MMU bring-up mapping the Tegra
peripherals (UARTC, then GIC/timer) as device memory before touching them — the
EL2 analogue of the Pi bare-metal `boot::mmu_init`. Until then `serial::tegra`
cannot drive UARTC, and the tegra early-stop's banner/heartbeats never reach the
console. This is the first item on the delta list below.

### JM3 result — kernel-owned Tegra MMU (**METAL-CONFIRMED 2026-07-04**)

JM3 builds and installs that regime. New module
`arch/aarch64/mmu_tegra.rs` (`#[cfg(feature = "tegra")]`): a single L1 translation
table (4 KiB granule, T0SZ=25 → 1 GiB blocks) where `L1[0]` is a **Device-nGnRE,
execute-never** block covering the low 1 GiB (UARTC `0x0C28_0000` and the Tegra234
GIC region `0x0F00_0000` for JM4), and every GiB the **firmware memory map** calls
RAM (`Usable`/`Bootloader`) is a **Normal-WB** block (belt-and-braces: the live
code and SP GiBs are marked too, so the first post-switch fetch/stack access cannot
fault). It reads `CurrentEL` and programs the regime for the EL it is actually at —
**EL2 primary** (`MAIR_EL2=0x04FF`, `TCR_EL2=0x8081_3519` non-VHE short format,
`SCTLR_EL2` read-modify-write M|C|I), **EL1 fallback**. The switch is one
cache-off `asm!` block (clean L1 to PoC → MMU off → reprogram MAIR/TCR/TTBR0 →
`tlbi alle2` → MMU+caches on), after which it installs a bounded 16-entry EL2/EL1
fault vector (Part C: prints `:: tegra: FAULT — ESR/FAR/ELR ::` then spins, turning
a dark post-switch hang into a recorded syndrome). `tegra_early_stop` calls this
FIRST, silently, so the first serial byte of the whole kernel is the post-switch
`:: tegra: mmu live (EL…) … ::` line.

**QEMU cannot model the Orin** (`tegra` is off in every QEMU build — it drives the
QEMU-`virt` PL011, not UARTC), so the QEMU gate here is **byte-stability + compile
only**, and the arc verdict is metal (below). Landed evidence:

- `./arroyo check` green both arches; `UNAOS_TEGRA=1 ./arroyo build` green both legs
  (full codegen — lowers all of `mmu_tegra`'s inline/global asm); `./arroyo
  esp-jetson` links `kernel.elf` (the vector tables land 2 KiB-aligned at the
  expected 0x80 entry stride; `tegra_fault_handler` resolves).
- Full QEMU regression battery byte-stable: `test` (x86 U1a/U1b PASS + MISSION
  SUCCESS), `test-arm` GICv2 (**byte-identical** to the pre-JM3 baseline — all JM3
  code is `tegra`-gated), `UNAOS_GICV3=1 test-arm` (JC2 SMP 3/3, modulo the known
  cross-core SGI-coalescing nondeterminism), `kernel8-test` (Pi M6b/M6d PASS + M6e
  verdict + CAPSTONE 6/6).

**★★ METAL-CONFIRMED (Part D, 2026-07-04, operator-attended).** The attended Orin
boot over the RPi Debug Probe is the verdict, and it passed clean — the first UnaOS
kernel to run its own MMU and drive UARTC on Orin silicon. Capture
`unaos/target/serial-orin-jm3.log` (73 heartbeats, **zero faults / panics /
exceptions**), verbatim:

```
[bootloader] ...booting headless (serial only)
[bootloader] Found DTB at 0x2679df000, size: 997852
:: tegra: mmu live (EL2) — RAM Normal-WB + Tegra Device-nGnRE mapped ::
:: tegra: mmu regs — SCTLR 0x30c5183d->0x30c5183d TCR=0x80813519 MAIR=0x4ff TTBR0=0x25e5eb000 RAM-GiB-mask=0x3fc ::
:: UnaOS aarch64 kernel — Jetson Orin Nano (Tegra234), headless serial console ::
:: AARCH64 boot diag: EL=2  CNTFRQ=31250000 Hz  MMU=on  DAIF(DAIF)=0b1111 ::
:: tegra: early platform stop (gic/timer discovery = JM4) ::
:: tegra: heartbeat 0..72 ::
```

Metal facts (feed JM4): **handoff EL = 2** (the EL2 non-VHE `E2H=0` assumption held
on A78AE); **CNTFRQ = 31.25 MHz** (new — ≠ QEMU 62.5 / Pi 54); firmware
**SCTLR_EL2 = `0x30c5183d`** (M|C|I already set, so our RMW was correctly
idempotent — `old == new`); **TTBR0 = `0x25e5eb000`** (our L1, in RAM GiB 9);
**RAM-GiB-mask = `0x3fc` = GiB 2..9** — the firmware-map-derived RAM span matched the
Orin's DRAM (`0x8000_0000..0x2_8000_0000`) exactly; **UARTC `0x0C28_0000` confirmed**
as the console base. The Part-C fault vector stayed inert (no fault). The DTB is
present (`0x2679df000`, 997 KB) — R4's "no DTB table" was the wrong-GUID artifact,
now disproven.

⚠ **Bootdiag caveat (this capture used non-BOOTDIAG media).** The first Part-D
attempt used `UNAOS_BOOTDIAG=1` media; with the corrected GUID, BOOTDIAG now reaches
the real 997 KB Orin DTB and its `fdt-0.1.5` traversal (`find_node("/chosen")` /
`aliases()`) **panics** (`node.rs:472` "bad node") → the bootloader shuts the board
down *before* the kernel loads. This is a latent JM2-bootdiag robustness bug the
GUID fix exposed (JM2 never parsed a real DTB — the wrong GUID always early-returned;
the crate's node *traversal* panics where its *helpers* were already guarded). It is
**outside the JM3 lane** (bootdiag parser / an `fdt` bump) and flagged as a follow-up.
The JM3 kernel never parses the DTB (`tegra_early_stop` diverges before pci/smp_virt),
so the header-only non-BOOTDIAG path boots cleanly and is what produced the capture
above; `serial-orin-jm3-r1-bootdiag-panic.log` holds the panic + the DTB/model proof.

### JM4 result — Orin GIC-600 + generic-timer interrupt (single core; **METAL-CONFIRMED 2026-07-04**)

JM3 left the Orin kernel spinning its heartbeat after the MMU came up. JM4 brings up
the **Tegra234 GIC-600 (GICv3) + the ARM generic timer on the boot core** so a timer
PPI (INTID 30) is delivered, turning the spin-heartbeat into an interrupt-driven one.
**SMP is deferred** (PSCI-on-Tegra, MPIDR affinity widening, per-AP SGI INTIDs) —
boot core only.

The whole interrupt path was already EL2-aware and is **reused unchanged**:
`exceptions::install` (VBAR_EL2 + `HCR_EL2.IMO` routing), `gic::init` (GICv3 detect +
distributor + this-core redistributor + ICC CPU interface + self-SGI smoke),
`timer::init` (CNTP timer, INTID 30, interval from the 31.25 MHz CNTFRQ),
`enable_irq`, `verify_live`. The **only** Orin-specific change was the GIC base
addresses in `gic.rs`: a third build-time base-set (mirroring `serial.rs`) pointing
the `tegra` build at the authoritative Tegra234 GIC-600 addresses from upstream
`tegra234.dtsi` — **GICD `0x0F40_0000`, GICR `0x0F44_0000`** — both inside the
low-1-GiB Device-nGnRE window `mmu_tegra` already maps. The per-core redistributor
stride is derived at runtime from `GICR_TYPER.VLPIS` (bit 1: a 4-frame GIC-600 is
`0x4_0000`, a 2-frame one `0x2_0000`); it is inert for the single-core boot (the boot
core matches the first redistributor frame before any stride advance) but
correct-by-construction for the deferred SMP arc. `tegra_early_stop` then enters an
interrupt-driven idle — `arch::hlt()` (WFI when the timer IRQ is confirmed delivering,
else poll-spin) with a heartbeat driven by `timer::ticks()` advancing, and a
CNTPCT-driven fallback beat that labels the degraded "GIC up, PPI-30 not delivering"
state rather than dark-hanging.

**QEMU cannot model the Orin** (`tegra` is off in every QEMU build — it drives the
QEMU-`virt` PL011/GIC, not the Tegra234 ones), so the QEMU gate is **byte-stability +
compile only**; the arc verdict is metal. Landed evidence: `check` both arches;
`UNAOS_TEGRA=1 build` both legs; `esp-jetson` links. The GIC change is tegra-gated or
virt-token-identical — the QEMU virt-**GICv2** serial log is **byte-identical**
clean-vs-JM4, virt-**GICv3** SMP stays **3/3** online, the Pi `kernel8-test` log is
identical modulo the known cross-core SMP interleave (proven by a same-binary re-run
showing the same interleave), and x86 stays U1a/U1b PASS + MISSION SUCCESS. (The ELF
hash differs only in LLVM `.llvm.<N>` symbol-mangling suffixes, which never reach
serial — so byte-identical *logs*, not byte-identical *binaries*, is the gate.)
Five-lens adversarial review → two issues, both fixed pre-commit (a wrong-bit VLPIS
test — bit 0 PLPIS vs bit 1 VLPIS; a stale JM3 call-site comment).

**★★ METAL-CONFIRMED (Part D, 2026-07-04, operator-attended).** Booted `./arroyo
esp-jetson` (non-BOOTDIAG) media over the RPi Debug Probe; passed clean on the first
boot — the first UnaOS kernel to take a hardware interrupt on Orin silicon. Capture
`unaos/target/serial-orin-jm4.log` (49 heartbeats, **zero faults / panics / degraded
fallback**), verbatim from the JM3 MMU lines onward:

```
:: tegra: mmu live (EL2) — RAM Normal-WB + Tegra Device-nGnRE mapped ::
:: AARCH64 boot diag: EL=2  CNTFRQ=31250000 Hz  MMU=on  DAIF(DAIF)=0b1111 ::
:: AARCH64 GICv3 init (GICD=0xf400000, GICR=0xf440000, 992 INTIDs) ::
:: GIC self-SGI delivered (v3) ::
:: AARCH64 generic timer armed (CNTFRQ=31250000 Hz, 250 Hz tick, INTID 30) ::
:: AARCH64 timer diag: CNTP_CTL=0x5 (ISTATUS=1)  GICD PPI30 pending=true ::
:: AARCH64 timer LIVE: IRQ delivery confirmed; idle = WFI ::
:: tegra: heartbeat 1..49 (ticks=250..12250, live) ::
```

Metal facts: the authoritative **GICD `0x0F40_0000` / GICR `0x0F44_0000`** addresses
are correct on silicon; the distributor reports **992 INTIDs** (`GICD_TYPER`
ITLinesNumber = 30 → (30+1)×32); the **boot-core redistributor walk found its frame**
(no panic — the VLPIS-derived stride was never needed, as expected for single core);
**self-SGI delivers (v3)** through the ICC CPU interface + the vector; the **generic
timer** at INTID 30 asserts (`CNTP_CTL=0x5`, ISTATUS=1) and the GIC latches the PPI
(`PPI30 pending=true`); `verify_live` confirmed **IRQ delivery**, so idle is **WFI**;
and the heartbeat is **timer-driven** — `ticks` climbing exactly 250/beat with
`live`, the whole timer→GIC→CPU-interface→vector→handler→EOI loop closed on Orin.
No degraded fallback path was taken. This is the interrupt-driven heartbeat verdict.

### JM5 result — Orin SMP via PSCI CPU_ON + cross-core SGI (QEMU-green 3/3; **metal ⛔ blocked on Tegra `CPU_ON`**)

JM4 ran the Orin on its boot core alone. JM5 brings the **secondary Cortex-A78AE cores
online via PSCI `CPU_ON`** and proves cross-core SGI in both directions, un-gating the
JC2 `smp_virt.rs` path for the `tegra` build and making its shared code correct on a
**multi-cluster** SoC. The Orin Nano is a 6-core part (BSP + 5 secondaries).

**The multi-cluster generalization (the crux).** Tegra234 encodes the topology as
**Aff2 = cluster (0–2), Aff1 = core-in-cluster (0–3), Aff0 = 0 always** (from upstream
`tegra234.dtsi`: `cpu@0…cpu@300`, `cpu@10000…`, `cpu@20000…`). So MPIDR Aff0 — which the
QEMU-`virt` single-cluster path used as the core id — is **not** a usable identifier on
Orin. JM5 splits the two roles the old path conflated:

* a **linear index** `0..N-1` (BSP = 0, secondaries 1..), dense and board-independent,
  selects a core's stack / per-CPU block / `CORE_READY` slot, and is handed to a woken
  core as the PSCI **context id** (delivered in `x0`); the AP entry stub uses `x0`
  directly instead of reading MPIDR;
* the **MPIDR affinity** (packed `{Aff3,Aff2,Aff1,Aff0}`) is a separate value used only
  as the `CPU_ON` target and the `send_sgi`/`IROUTER` target, so an SGI/SPI reaches the
  right core across clusters. On `virt` the two coincide (affinity = Aff0 = index), so
  the path stays byte-compatible there.

**Metal-truth core discovery.** Rather than parse the firmware DTB for the fused-core set
(the `fdt-0.1.5` parse of the real Orin DTB panics — task cde963a7), JM5 walks the GIC
**redistributor frames** (`gic::enumerate_redistributor_affinities`), reading each
frame's `GICR_TYPER` affinity — the core set and its real cluster affinities read straight
off the silicon, adapting to whatever the board is. This is the **first code to walk a
non-first redistributor frame on Orin**, i.e. the first metal exercise of JM4's
`GICR_TYPER.VLPIS`-derived stride.

**Delta-list items landed** (all in `smp_virt.rs`/`gic.rs`, shared with `virt`):

* **Part A — Tegra GICR + un-gate.** The tegra SMP kick-off runs in `tegra_early_stop`
  after the JM4 GIC/timer bring-up; APs resolve their own redistributor via the JM4
  Tegra234 GICR base + VLPIS stride (`gic::init_secondary_v3` — inherited, not
  re-hardcoded). `MAX_FRAMES = 8` covers the 6 cores.
* **Part B — `HCR_EL2` in `SEC_CTX`.** The capture/replay now carries `HCR_EL2`; a
  PSCI-reset AP forces `E2H`/`TGE` to the BSP's value (E2H=0) **before** replaying
  `TCR`/`TTBR`/`SCTLR_EL2`, so the translation regime is interpreted correctly regardless
  of the AP's UNKNOWN reset `E2H` (QEMU-invisible; JM3 confirmed the BSP is E2H=0).
* **Part C — `CPTR_EL2` + stack in the asm stub.** The `HCR_EL2` and `CPTR_EL2` replays
  and the stack setup are now in the AP entry asm stub, **before any compiler-generated
  code** (structural, not compiler fortune). A `CurrentEL` guard parks a core that comes
  up at an EL other than EL2 (belt-and-braces for a firmware that drops APs to EL1),
  which the BSP then observes as a clean `CORE_READY` timeout rather than a wedge.
* **Part D — affinity widening.** `send_sgi_v3` fills `ICC_SGI1R_EL1` `{Aff3,Aff2,Aff1}` +
  `TargetList = 1<<Aff0` from the target's real MPIDR (was Aff0-only); `enable_spi_v3`
  writes the full affinity into `GICD_IROUTER`. On `virt` both reduce to the pre-JM5 value
  (byte-identical), so one code path serves both boards.
* **Part E — per-AP distinct SGI INTIDs. DEFERRED** (delta-list follow-up). The BSP→AP
  direction is already individually attributable (each AP's own IPI counter); only the
  AP→BSP proof coalesces (one INTID). Deferred to keep the arc to one clean session.

The PSCI conduit is **SMC** (Orin's ATF/BL31 monitor at EL3, as on QEMU's emulated PSCI);
the tegra caller passes `dtb=0`, so `report_conduit` prints the assumed-SMC line without
touching the panicking DTB parse. `percpu::NUM_CPUS` was raised **4→8** (a Peter-approved
one-line lane extension: additive, inert for pi/virt/x86 — only the static block array
grows) to give the 6-core Orin enough per-CPU slots.

**QEMU gate (functional, not byte-identical — the shared SMP-correctness fixes legitimately
change what `virt` runs).** On QEMU-`virt gic-version=3` the enumerated bring-up is
**3/3 secondaries online + cross-core SGI**, verbatim:

```
:: AARCH64 SMP: redistributor walk found 4 core(s); BSP aff=0x00000000, 3 secondary(ies) to start ::
:: AARCH64 SMP: CPU_ON AP 1 (aff=0x00000001) -> SUCCESS (entry=0x…) ::
:: AARCH64 SMP: AP 1 online (aff=0x00000001) ::   (…AP 2, AP 3…)
:: AARCH64 SMP: BSP -> AP 1 SGI OK (count 0 -> 1) ::   (…AP 2, AP 3…)
:: AARCH64 SMP: AP -> BSP SGI OK (3 online APs pinged, 2 delivered; BSP ipi 1 -> 3) ::
:: AARCH64 SMP: 3/3 secondaries online via PSCI CPU_ON on the GICv3 path ::
```

Intended `virt` log deltas vs JC2: the new `redistributor walk …` discovery line, the
`aff=0x…` fields on the CPU_ON/online/warning lines (affinity-format change), and the
BSP→AP proof targeted by affinity. The rest of the battery:

* **virt-GICv2 — behavior-identical** (no SMP at runtime; `is_v3()` false). Not quite
  byte-identical: the GICv2 and GICv3 `virt` runs are the **same binary** (`gic-version` is
  a QEMU runtime flag, not a compile flag), so the shared v3/SMP code the arc necessarily
  changes (`send_sgi_v3`, `enumerate_redistributor_affinities`, …) is compiled into the v2
  binary too, even though v2 never calls it. The **only** diffs vs the pre-JM5 baseline are
  **two layout-dependent values** — the printed kernel-image size (`max_vaddr`, same page
  count) and `VBAR_EL2` — both shifted by that code growth. **No boot-sequence or behavior
  line differs.** (The per-CPU/stack array sizes are `tegra`-gated precisely so the *arrays*
  don't add to this — the residual shift is pure code size.)
* **Pi `kernel8-test` — byte-identical mod interleave.** The pi build compiles *none* of
  the JM5 code (`smp_virt` and every v3 helper are `cfg(not(pi))`; `percpu::NUM_CPUS` stays
  4). The `kernel8.img` is the **same size** as baseline (differing only in LLVM `.llvm.<N>`
  symbol-mangling suffixes, which never reach serial), and the serial log is the **same set
  of lines** as baseline — a **sorted diff is empty**; the raw diff is only the known
  cross-core scheduler interleave. So pi behaviour is provably unchanged.
* **x86** — U1a/U1b/U2 PASS + xHCI MISSION SUCCESS (arch/aarch64 not compiled).

`check` both arches; `UNAOS_TEGRA=1 build` both legs; `esp-jetson` links.

**Metal (Part F): ⛔ BLOCKED — PSCI `CPU_ON` triggers a fatal Tegra RAS fault (2 operator-attended
attempts, 2026-07-04).** QEMU cannot model the Orin, so the metal boot is the verdict — and it is a
**failure isolated to `CPU_ON`**. Both attempts booted clean through JM3/JM4 and *into* JM5, then the
**first** `CPU_ON` (to a real cluster-0 core, aff `0x00000100`) caused a fatal firmware **RAS
Uncorrectable Error** *before it returned* — no AP ever came online, and the BSP itself powered off
(0 post-fault heartbeats). Syndrome (verbatim):

```
:: AARCH64 SMP: enumerated core 0..7 aff = 0x0 / 0x100 / 0x200 / 0x300 / 0x10000 / 0x10100 / 0x10200 / 0x10300 ::
:: AARCH64 SMP: core 1..7 AFFINITY_INFO=1 -> present ::            (all 8 report present/OFF)
:: AARCH64 SMP: walk found 8 core(s); … 7 present -> starting ::
ERROR:  RAS Uncorrectable Error in IOB … SERR = Error response from slave … IERR = CBB Interface Error
ERROR:  RAS Uncorrectable Error in ACI … IERR = FillWrite Error … ADDR = 0x8000000000000b5c
ERROR:  Powering off core
```

**What metal *did* confirm** (bonus): JM3 MMU + JM4 GIC (`992 INTIDs`) + `timer LIVE` re-ran clean;
the **GICR enumeration works on silicon** — the walk crossed non-first frames (the first metal
exercise of JM4's VLPIS stride) and read all 8 frames' affinities: **two full clusters** (cluster0
`0x0/0x100/0x200/0x300`, cluster1 `0x10000…0x10300`); and the **SMC conduit works** — all 8
`AFFINITY_INFO` SMCs returned cleanly (`=1`, OFF). So the split is sharp: everything up to and
including PSCI *queries* works; only PSCI `CPU_ON`'s core-power action faults.

**Diagnosis.** The `CBB Interface Error` / `Error response from slave` on the Tegra Control Backbone
fabric, raised inside BL31 while servicing `CPU_ON`, is a bus error powering/resetting the target
core — not something our code touches directly. The `AFFINITY_INFO` gate (JM5 attempt-2 fix) did not
help because this firmware reports all 8 die slots present. **Ranked hypotheses for a dedicated
follow-up** (NOT for blind on-metal iteration — the STOP tripwire held):
1. Tegra `CPU_ON` needs **MCE / BPMP coordination** (Tegra's Multi-Core-Environment firmware) that a
   generic PSCI call from our minimal EL2 kernel does not provide → BL31's core-power path errors.
2. A **latent/poisoned RAS condition** surfaced by the first SMC→EL3 barrier (the fault lands exactly
   at the first `CPU_ON`; `ADDR` differs run-to-run: `0x…200`, `0x…b5c`).
3. **Entry point high in DRAM** (kernel at `0x25e52c000` ≈ 9.5 GiB) rejected by BL31's reset-vector
   programming — a low-PA trampoline would test this.
4. **Caller-EL** interaction: we run the whole path at NS-EL2; JetPack's OS calls PSCI from EL1/EL2
   with a fuller ATF handshake.

**Landing:** JM5 is **QEMU-green** (the SMP mechanism is correct for a compliant PSCI/GICv3 — proven
3/3 on `virt`) and **metal-blocked on Tegra `CPU_ON`**. This needs a dedicated "Orin PSCI/MCE core
bring-up" investigation (possibly the NVIDIA-collaboration angle), not a JM5 code fix. Captures:
`unaos/target/serial-orin-jm5-FAIL-rasfault.log` (attempt 1) and
`serial-orin-jm5-FAIL2-affinfo-allpresent.log` (attempt 2, with the enumerated affinities +
AFFINITY_INFO results).

### Orin-metal delta list (feeds JM3 + the cable-day graphical arc)

The `virt` GICv3 + PSCI SMP (JC2) is written to be a small delta to Orin silicon,
but the following must change and are **not** covered by QEMU:

* **Kernel MMU that maps the Tegra peripherals (the R4 blocker).** ✅✅ **DONE —
  METAL-CONFIRMED (JM3, 2026-07-04).** `arch/aarch64/mmu_tegra.rs` installs a
  kernel-owned regime (RAM Normal-WB from the firmware map + the Tegra device window
  Device-nGnRE, EL2 primary / EL1 fallback), the EL2 analogue of the Pi bare-metal
  `boot::mmu_init`/`enable_mmu`; wired into `tegra_early_stop` before the banner. On
  real Orin silicon the switch works at **EL2**, the banner + 72 heartbeats print over
  UARTC `0x0C28_0000`, `RAM-GiB-mask=0x3fc` (GiB 2..9) matches the DRAM span, and there
  are **zero faults**. See the "JM3 result" section for the full capture. The corrected
  `EFI_DTB_TABLE_GUID` also confirmed the Orin **does** publish a DTB config table
  (`0x2679df000`, model 'NVIDIA Jetson Orin Nano … Super') — R4's "no DTB table" was the
  wrong-GUID artifact. (Follow-up, out of the JM3 lane: BOOTDIAG's full `fdt` parse of
  the real DTB panics — see the JM3-result caveat.)
* **GICR base *and* frame stride.** ✅✅ **boot core DONE — METAL-CONFIRMED (JM4,
  2026-07-04)** — `gic.rs` selects the Tegra234 GIC-600 bases (GICD `0x0F40_0000`,
  GICR `0x0F44_0000`) for the `tegra` build and derives the redistributor stride from
  `GICR_TYPER.VLPIS` (bit 1); on real Orin the `GICv3 init (GICD=0xf400000,
  GICR=0xf440000, 992 INTIDs)` + `self-SGI delivered` + `timer LIVE` lines confirm the
  boot-core redistributor + CPU interface + timer PPI all work (see "JM4 result").
  **SMP-arc part ✅ QEMU-green; metal pending (JM5):** `smp_virt.rs` no longer hardcodes
  a virt base — its APs resolve their own redistributor through `gic::init_secondary_v3`
  (Tegra234 GICR base + VLPIS stride), the core set is discovered by walking the GICR
  frames (`gic::enumerate_redistributor_affinities` — the first non-first-frame walk, i.e.
  the first exercise of the VLPIS stride), and `send_sgi_v3`/`enable_spi_v3` are widened to
  the target's full MPIDR affinity (`ICC_SGI1R_EL1` `{Aff3,Aff2,Aff1}` + `1<<Aff0`;
  `GICD_IROUTER` full affinity). On QEMU-`virt` this reduces to the pre-JM5 value; on Orin
  it is the load-bearing multi-cluster path (Aff2=cluster, Aff1=core, Aff0=0). See "JM5
  result".
* **PSCI conduit from the real DTB.** ✅ QEMU-green; metal pending (JM5). JC2 confirmed
  QEMU's conduit is **SMC** via `dumpdtb`. On Orin the boot chain is ATF/BL31 + OP-TEE, so
  SMC is near-certain; JM5 uses **SMC** and — because the `fdt-0.1.5` parse of the real
  Orin DTB panics (task cde963a7) — the tegra caller passes `dtb=0`, so `report_conduit`
  prints the assumed-SMC line without parsing. If `CPU_ON` errors on metal, that is a clean
  diagnostic → STOP (do not blind-try `hvc`). A hand-rolled `/psci method` FDT walk (no
  `fdt` crate) remains a future option, out of the critical path.
* **`SEC_CTX` must add `HCR_EL2`.** ✅ QEMU-green; metal pending (JM5). The capture/replay
  now carries `HCR_EL2`; the AP forces `E2H`/`TGE` to the BSP's value **first** (in the
  entry stub) before the `TCR`/`TTBR`/`SCTLR_EL2` replay, so the regime is interpreted
  correctly regardless of the AP's UNKNOWN reset `E2H`. See "JM5 result".
* **`CPTR_EL2` replay + AP pre-MMU stack spill → asm stub.** ✅ QEMU-green; metal pending
  (JM5). The `HCR_EL2`/`CPTR_EL2` replays and the stack setup are now in the AP entry asm
  stub, **before any compiler-generated code** (structural, not compiler fortune). A
  `CurrentEL` guard parks an AP woken at EL≠2 without touching an EL2 register (→ a clean
  BSP-side timeout, not a wedge).
* **Per-AP SGI attribution via distinct INTIDs.** ⏸ **DEFERRED** (JM5 follow-up). The
  cross-core SGI proof stays "at least once" for the AP→BSP direction (GICv3 coalesces one
  INTID); the BSP→AP direction is already per-core attributable (each AP's own IPI counter).
  Deferred to keep JM5 to one clean session; give each AP a distinct SGI INTID in a
  follow-up so AP→BSP delivery is individually attributable too.
