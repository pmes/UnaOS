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
