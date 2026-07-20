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

#### Open investigation: core-3 bring-up vs image size (CORE3-SMP)

**Status: ROOT CAUSE FOUND + FIX SHIPPED (QEMU-green; metal 4/4 confirmation pending the
attended bench).** The 2026-07-15 probe bench proved core-3 delivery correct and the id-0
corruption kernel-side; disassembly of the >1 MiB build then pinned the exact mechanism (an
MMU-off stack spill of the core-id argument reloaded stale-cacheable after `enable_mmu`), and
the fix re-derives the id from `MPIDR_EL1` with the MMU on. See **⚡ PROBE VERDICT** and
**✅ FIX** below. The static-analysis narrative that follows is preserved as the record of what
was ruled out — note hypothesis 5 ("`core_raw` is preserved") is the one the probe + codegen
overturned.

**Symptom (attended A-B-A bench, real Pi 4 / BCM2711, 2026-07-14).** With the same
silicon, card, and power supply minutes apart (thermal/environment excluded), the
spin-table release of **core 3** fails **deterministically as a function of `kernel8.img`
size**:

| `kernel8.img` size | hex | cores online |
|---|---|---|
| 524,232 B | `0x7ffc8` | 1, 2, 3 — **4/4** |
| 588,936 / 589,960 B | `0x8fc48` / `0x8fc88` | 1, 2 up; **core 3 timeout** |
| 704,080 B | `0xabdd0` | 1, 2 up; **core 3 timeout** |

Cores 1 and 2 always come up; everything else is healthy on 3 cores. QEMU `raspi4b`
models the same spin-table and brings up **4/4 at every size** — it never reproduces,
so the proof burden here is static analysis, not a repro.

**The tell.** Every failing boot (5 logged, across the `0x8fc88` and `0xabdd0` sizes) is
byte-consistent: cores 1 and 2 log their own ids, then a **phantom `:: AARCH64 SMP:
core 0 online ::`** appears where core 3's line should be, and `CORE_READY[3]` never
sets → the wait loop times out. Since only cores {1,2,3} are released and 1/2 report
correctly, the phantom "core 0" **is** physical core 3: it runs `__secondary_rust(0)`
end-to-end (per-CPU init, GIC, timer, `CORE_READY[0].store`) with `core_raw == 0`.

**The arithmetic — the boundary is exactly 1 MiB.** The GPU firmware loads `kernel8.img`
to `0x80000`; `.bss` is `NOLOAD`, so the image file is exactly the loaded sections and
`img_size == __bss_start - 0x80000`. Image end (`= __bss_start`):

* passing `0x7ffc8`  → `0x80000 + 0x7ffc8 = 0xFFFC8`  (56 B **below** 1 MiB)
* failing `0x8fc48`  → `0x80000 + 0x8fc48 = 0x10FC48` (**above** 1 MiB)

So the loaded image (`.text`/`.rodata`/`.data`) crossing **`0x100000` (1 MiB)** is the
threshold. Note the BCM2711 **L2 cache is exactly 1 MiB** — i.e. the image stops fitting
in L2 at the same point.

**What is ruled out (kernel side):**

1. *No firmware-memory overlap.* The base DTB `/memreserve/` reserves only page 0
   (`0x0..0x1000`); the firmware relocates the **final** DTB high (observed
   `0x2eff2700`) and CMA sits at `0x30000000`. All firmware placements are logged
   **identically in passing and failing boots** — nothing firmware-placed sits at or
   near 1 MiB, and the layout does not move with image size.
2. *Release addresses are correct.* The DTB `cpu-release-addr` values
   (`0xd8/0xe0/0xe8/0xf0` for cores 0-3) match `smp.rs::RELEASE_ADDR`, and all sit in
   page 0 — never overlapped by an image at `0x80000+`.
3. *Mailbox cache-clean is correct and size-invariant.* `clean_range(0xe0, 0x18)`
   covers `0xe0..0xf8`, all within the single 64 B line at `0xc0`, so the one `DC CVAC`
   flushes core 3's slot (`0xf0`) to PoC. The addresses are fixed constants — identical
   at every image size.
4. *Mailbox mapping is identical across sizes.* `0xf0` lies in the L3-paged first 2 MiB
   block (`USER_REGION` keeps `l2_idx == 0` in both builds); page 0 is `ram_page`
   (Normal cacheable, EL1) in both — same attributes regardless of size.
5. *`core_raw` is preserved.* `_secondary_start` reads `MPIDR_EL1` at **EL2** (physical
   affinity) into `x0` and tail-calls `__secondary_rust(x0)`; `drop_to_el1` clobbers
   only `x0` and the compiler spills the argument to a callee-saved register across the
   call. If core 3 entered with `x0 == 3` it would print `3`.

**The paradox that forces the STOP.** The BSP writes one identical entry value to all
three release slots in a uniform loop and flushes them with one size-invariant sequence;
cores 1 and 2 read their affinity correctly at EL2 (where `MPIDR_EL1` cannot read 0 for
core 3). Yet core 3 alone runs as id 0, deterministically, and only once the image
exceeds 1 MiB. Nothing in the kernel path differs per-core for core 3 or per-size for the
mailbox. The divergence therefore lies **below the kernel** — in the BCM2711 GPU-firmware
/ armstub delivery of the last core, or a micro-architectural effect keyed to the 1 MiB =
L2-size boundary (e.g. the branch-target/mailbox line's visibility to a core that fetches
MMU-off / cache-off once the image no longer fits L2). This is not resolvable by static
inspection of `smp.rs`/`boot.rs`, so per the arc's STOP tripwire **no speculative linker
reservation or stack relocation was shipped** — there is no identified structure at the
boundary for a reservation to protect.

**⚡ PROBE VERDICT (attended Pi bench, 2026-07-15 — INVERTS the conclusion above).** The
`core3probe` instrumentation (merged `da47846`; raw PL011 dump as the first instructions of
`_secondary_start`, MMU-off, no stack) captured core 3's record clean on a real BCM2711 boot
where core 3 subsequently failed: **`[03E2X0]` — MPIDR Aff0 = 3, CurrentEL = EL2, arrival
x0 = 0 — on the same boot as the phantom "core 0 online" + "core 3 did not come online"**
(log `~/unaos-bench/core3probe-boot3-2026-07-15.log`; build 712656 B, the >1 MiB regime).
Decision-table row 1: firmware/armstub delivery of core 3 is CORRECT and `MPIDR_EL1` does
NOT read 0. Both surviving below-kernel mechanisms are REFUTED — **the id-0 corruption is
KERNEL-SIDE, after the asm stub**: somewhere in `__secondary_rust`/`drop_to_el1`/the
`core_raw` argument path, a correctly-read id 3 becomes 0, only on ≥1 MiB images. (Probe-idiom
note for any multi-core rerun: the three secondaries write the PL011 DR unarbitrated, so racing
records drop or corrupt — QEMU's modeled FIFO hid this, only one clean record survives per metal
boot; add a UART mutex or MPIDR-staggered delay if a future probe needs all three in one boot.)

**✅ FIX (CORE3-FIX arc, 2026-07-15 — mechanism proven by disassembly, structural fix shipped).**
Disassembling `<__secondary_rust>` from the >1 MiB build
(`llvm-objdump -d target/aarch64-base/release/unaos-kernel`) proved the mechanism the probe
predicted, and overturned hypothesis 5 above. The pre-fix codegen spilled the `core_raw`
argument to the **stack with the MMU off**, then reloaded it **cacheable after `enable_mmu`**:

```
d95e4: stp x30, x0, [sp,#0x10]   ; core_raw (x0) spilled to sp+0x18 — MMU OFF (Device/non-cacheable → DRAM)
d95f0: bl  drop_to_el1
d95f4–d9634: (enable_mmu inlined) ; MMU + D-cache turn ON here
d96b4: add x9, sp, #0x18          ; serial_println! passes &core = pointer to the spill slot
d96d8: ldr x0, [sp,#0x18]         ; RELOAD (cacheable) feeds CORE_READY[core] index + wait_and_run(core)
d96f4: stlrb w19, [x8]            ; CORE_READY[core].store(true), x8 indexed by the reloaded value
```

The MMU-off store writes DRAM directly; the post-`enable_mmu` cacheable reloads can hit a stale
(zero) L2 line for that stack address — the BSP runs cacheable over all RAM and can speculatively
allocate lines in `SECONDARY_STACKS` (which `_start` zeroed). That reproduces the phantom exactly:
percpu/GIC/timer init use a callee-saved copy (`x19` = correct id 3), but the print, `CORE_READY`
index, and `wait_and_run` use the stale-reloaded spill (id 0). It is QEMU-invisible (TCG models no
caches) and image-size-deterministic because stale-line residency in the 1 MiB L2 is layout-dependent.
This is the same mismatched-attributes coherency class as the spin-table slot's `DC CVAC` note.
Hypothesis 5's "compiler spills to a callee-saved register" was half-right — it *also* spills to the
stack, and `serial_println!`'s Display-by-reference forces the pointer-to-slot that keeps the stack
copy load-bearing.

**The fix (`smp.rs::__secondary_rust`):** ignore the incoming argument (now `_advisory`) and
re-derive the id from `MPIDR_EL1` **after** `drop_to_el1()` + `enable_mmu()` (`mrs`, `& 0xff`).
`drop_to_el1` seeds `VMPIDR_EL2` with the real MPIDR, so the EL1 read returns the physical Aff0;
and because the `mrs` and every spill of the derived value now execute with the MMU on, each
store/load pair is cacheable-coherent — the stale-line window is deleted, not patched (no cache
maintenance on the stack, no reliance on registers surviving `drop_to_el1`). The derived id is
bounds-checked (`< NUM_CORES`); on garbage the core parks in a `wfe` loop (the pre-fix failure
mode, never worse) rather than panicking through a possibly-unsound path. Post-fix disassembly
confirms it: `d9634: mrs x8, MPIDR_EL1` sits **after** the `SCTLR_EL1` write (MMU on), the id
spill `d9640: str x8,[sp,#0x18]` and its reload `d96ec: ldr x0,[sp,#0x18]` are both MMU-on, and
the advisory x0 is no longer spilled at all. The asm stubs are unchanged (x0 became advisory);
`smp_virt.rs`/`boot_virt.rs`/`boot_tegra.rs` are out of lane — the analogous virt/Tegra pattern is
flagged upward separately.

**Gate:** `./arroyo check` green both arches; `kernel8` builds clean; `kernel8-test` byte-equivalent
(41 PASS, CAPSTONE 6/6 on APs [1,2,3], K3-mount `[w=0x1ff]` + K4-write `[w=0x7f]` + F2/F3 locked
240000/240000, 0 forbidden); `test-arm` MISSION SUCCESS (virt untouched). QEMU brings up 4/4 at
every size and can never reproduce the regression — the real verdict was always metal.

**✅✅ METAL-CONFIRMED (2026-07-15 attended Pi bench, Peter physical, boot 1 — the verdict).**
A **>1 MiB build (`kernel8.img` 712,464 B loaded to `0x80000` — squarely the failing regime)
brought ALL FOUR cores online**: cores 1/2/3 each logged their own correct id, **no phantom
"core 0"**, no bring-up timeout, all 3 IPIs OK, and **CAPSTONE ran 6/6 COMPLETE** (workers on
cores 2 + 3) — **the first full-core boot in the >1 MiB regime since the regression.** Same boot:
F2/F3 witnesses both `locked 240000/240000 intact` (unlocked lost exactly 50%) under true 4-core
parallelism, K3-mount `[w=0x1ff]`, K4-write `[w=0x7f]`, and **two rider captures** — **K3-revoke
`[w=0x7f]`** (the SYS_FGRANT two-phase durable-first revoke ordering exercised against the REAL
card — previously QEMU-only) and **K5-lockspan `[w=0x3f]`** (revoke/re-persist ns-span + create
gate on silicon). **Zero forbidden lines** (no PANIC/EXCEPTION/CMD13/R1/heal). Caveat, verbatim
from the bench: the boot showed the documented stale-fixture signature (card not re-prepped since
the probe bench's 3 boots) — U9/U10 false-FAIL-looking lines (`sector_changed=false` /
`size_grew=false`) + U10-create/U11/U6-grants `(stale image) — demo skipped`. Documented
signature, NOT regressions (0 forbidden, `cleared=true killed=0`). **The strict line closed the
same day: boot 2 (pristine card, fresh flash of the staged `b9bcf53` image, 718,160 B — still
>1 MiB) = `MBENCH PASS 32/32 required witnesses, 0 forbidden` in a single boot — CAPSTONE 6/6
again (second consecutive full-core >1 MiB boot), the whole created-file family real-PASS
(U9/U10/U10-create/U10-delete/U11+defer/reuse/reap/U6-grants), F2/F3 locked 240000/240000, both
riders re-captured.** Serial: `~/unaos-bench/pi-serial-2026-07-15-core3fix-32of32.log`.
Core-3-down on >1 MiB images is no longer an expected signature.

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

### Fixed-priority multilevel run queues + anti-starvation aging (AARCH64-PRIO)

The run queue is no longer flat round-robin. Each CPU's `RUN_QUEUES` entry is a
`RunQueue` of `NUM_PRIORITIES = 4` FIFO levels (`PRIO_LOW`=0, `PRIO_NORMAL`=1,
`PRIO_HIGH`=2, `PRIO_RT`=3). A dispatch always pops the front of the **highest
non-empty** level (strict priority; round-robin within a level). This ports the
proven x86 design (`arch/x86_64/sched.rs`) to this module's own structures.

- **Spawn API.** `spawn`/`spawn_user`/`spawn_joinable` are unchanged and land every
  task at the default `PRIO_NORMAL` — a single level, so those (many) call sites stay
  behaviourally identical to the pre-priority flat round-robin. A new
  `spawn_prio(name, entry, arg, cpu, priority)` picks a level explicitly.
- **Aging (anti-starvation).** A ready task carries a lock-protected, owning-CPU-only
  `wait_ticks`. `RunQueue::push` (ENQUEUE) re-bases a task to its base-priority level
  and zeroes the clock; a periodic sweep (`RunQueue::age`, run under the same run-queue
  lock as the pop, HIGH→LOW so a promotion is visited at most once) RELOCATES any task
  that has waited `AGE_TICKS` one level UP via a raw `VecDeque` move — its base
  `priority` is untouched, so a promoted-then-dispatched task re-bases on its next
  enqueue. A low task under continuous higher-priority load thus climbs to parity, runs,
  and drops back — starvation is bounded to ~`AGE_TICKS` per level climbed.
- **Aging clock = dispatch passes, not timer ticks (the aarch64 adaptation).** x86 ages
  in its always-live LVT `percpu.ticks`. The aarch64 *cooperative* dispatch paths (the
  BSP demo, the `virt` secondaries, the `virt` CAPSTONE driver, and QEMU raspi4b) have
  **no live periodic tick** — QEMU delivers no Group-1 timer IRQ, so `percpu.ticks` is
  frozen at 0. So aging advances one unit per `dispatch_next` pass on the owning CPU
  (`SchedCpu::age_passes` / `age_last_sweep`), which ticks on *every* path (cooperative
  and preemptive, QEMU and metal). A pass **is** the starvation measure — it counts each
  time the core dispatched someone else while a waiter sat.
- **Contract preservation.** The per-CPU run-queue spinlock ownership, the CPU pulse
  telemetry (`CPU_BUSY`/`CPU_IDLE`, still bumped only on real dispatch/idle in
  `dispatch_next`), and the `virt` busy/idle-heartbeat witness are unchanged: the
  secondary probe tasks are all `PRIO_NORMAL`, so the busy count stays `busy=8`. The
  aging relocate's `push_back` may allocate under the run-queue lock; that is benign
  exactly as at `spawn` — the heap lock is always innermost (run-queue → heap, never
  inverted).

**M3 witness (`priority_aging_witness`).** A self-checking, bounded cooperative pass run
on the `virt` GICv3 boot core (inside `run_capstone_boot_core`, before the CAPSTONE):
`PRIO_HIGH` loaders keep the top level continuously non-empty while one `PRIO_LOW`
candidate must be aged up to run. It asserts the low task completed **while high load was
still active** (only possible via aging) and prints
`:: AARCH64 SCHED: priority+aging PASS ::`. It never hangs — every task does finite work
and the low task never yields, so a broken aging path FAILs loudly instead of wedging the
core. Captured by `UNAOS_GICV3=1 ./arroyo test-arm 40`.

**PRIO-MIX witness (`prio_mix_witness`).** The dedicated priority-mix stress witness the
AARCH64-PRIO landing deferred (the Pi metal ledger recorded it as "mix witness deferred").
Under a genuine mixed-priority load it proves **both** halves of the multilevel scheduler on
one core, back to back, and reports each independently on one line:

- **strict** — from a *drained* queue seeded with `PM_STRICT_HIGH`=3 `PRIO_HIGH` short tasks
  (each runs to completion in one dispatch, no yield) + 1 `PRIO_LOW` short task, a monotonic
  completion-order counter proves the whole high level finished before the low task (its
  finish index is last). This is an **ordering** claim — valid only on a cooperative drained
  start, which is how the witness always runs (both call sites run it before preemption is
  enabled), so it is deliberately *not* asserted under preemption.
- **aged-rescue** — from a drained queue seeded with `PM_AGE_HIGH`=2 `PRIO_HIGH` loaders that
  each yield 40 times (keeping the top level continuously ready) + 1 `PRIO_LOW` no-yield
  canary, the canary is rescued by aging and completes **while high load is still active** —
  the anti-starvation proof. This is a **bounded-rescue** claim (completion before the finite
  load drains), *not* an ordering claim, so it stays honest under real preemption on Pi metal:
  the aging clock is dispatch passes (`SchedCpu::age_passes`), which advance on cooperative and
  preemptive dispatch alike, so a rescued-before-drain low is bounded in either regime.

It emits `:: AARCH64 SCHED: prio-mix witness (strict=..., aged-rescue=...) => PASS/FAIL ::`.
Bounded and never hangs a battery — every task does finite work and neither low task yields,
so `run_until_empty` always drains and a broken scheduler FAILs loudly. It is wired into
**both** witness paths: alongside `priority_aging_witness` on the `virt` GICv3 boot core
(`run_capstone_boot_core`, captured by `UNAOS_GICV3=1 ./arroyo test-arm 40`) and into the Pi
`kernel8` battery (end of `demo_cooperative`, before `start_aps` enables preemption — the
deferred Pi accrual, captured by `./arroyo kernel8-test 35`). On the `virt`/Orin paths the
boot core diverges into `run_capstone_boot_core` before reaching `demo_cooperative`, so each
platform runs the witness exactly once.

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
  **video-via-GOP is impossible on this firmware**; real Orin video means going around the GOP to the
  live scanout. **Resolved in §JD1** (below): the firmware does not withhold the framebuffer, it hands
  it off through the DTB (`simple-framebuffer`, the SIMPLEFB handoff), so UnaOS **inherits** the live
  scanout base+geometry from the device tree — no need to allocate a surface or re-program the DC. The
  JM7 code stays: correct, inert when `fb addr=0`, and the mapping/flush machinery JD1 reuses unchanged.
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
keyboard boot is the quick win to demonstrate `keyboard ARMED` end-to-end. **These four are JB10,
below.**

### JB10 — nested-hub descent, FS Evaluate-Context, root-keyboard readiness, inherit-path housekeeping

JB10 follows the JB9 verdict (USB enumerates on the Orin). It is **QEMU-green + code-review clean;
the USB-behaviour halves are metal-pending — QEMU cannot exercise them** (its regression attaches
devices only on root ports, no `usb-hub`, and is lenient about EP0 MPS), so items 1–3 below are
verified only by construction and an adversarial 5-lens review, not on silicon. They land as levers
for the next attended bench. Item 4 is pure hygiene and needs no bench.

> **✅ JB10 METAL VERDICT (attended bench, 2026-07-08 — all four confirmed in one boot).** Serial
> `~/unaos-bench/jetson-serial-2026-07-08-180338.log`. **Item 1 (nested-hub):** the VIA VL2109 chain
> enumerated two tiers deep — `HUB DETECTED (slot 4)` → `HUB-BEHIND-HUB DETECTED (slot 5, 2109:2813,
> route 0x2 tier 1)` → descended → tier-2 devices at route `0x22`/`0x32`/`0x42` (route-string math
> exact on silicon) → a **Low-Speed keyboard (slot 8, route 0x42, tier 2) `keyboard ARMED (root port
> 6) -> PASS`**, and typing came through end-to-end (`KEY 'h' 'e' 'l' 'l' 'o'` at EL1) — so the LS-child
> TT (DW2) programming is correct on hardware too. **Item 2 (FS Evaluate-Context):** the JB9 port-7
> blocker cracked — `[tegra fs-mps] slot 10 (FS port 7) … bMaxPacketSize0 = 64 … EP0 MPS0 -> 64
> (Evaluate Context OK)`: the strict Tegra FW **accepted** the in-place Evaluate Context (the
> review-caught EP0-offset fix was right), so the FS device learns MPS0 and reads its descriptor
> instead of going silent after the old teardown churn. **Item 4:** JB9 diagnostic suite silent
> (0 `FW-ALIVE`/`jb9f`/`ring_scan` lines). CAPSTONE 6/6, clean EL2→EL1 drop. **Still open (documented
> follow-up):** storage-behind-hub — slot 6 (Alcor `058f:6362`, tier 2) enumerated but "no HID
> interrupt endpoint"; downstream storage is not yet claimed, so `storage_slot` stays 0.

> **Display (why the panel is dark — precise finding, 2026-07-08 bench).** The JetPack UEFI publishes
> its GraphicsOutput in **`BltOnly` mode**: 5 modes, current 1920×1200, every one `fmt=BltOnly` — i.e.
> a `Blt()` boot-service with **no linear framebuffer** (and `Blt()` is gone after ExitBootServices).
> The bootloader logs `GOP: active mode has no linear framebuffer; booting without a display`
> (`crates/bootloader/src/main.rs@450`) and the kernel takes the headless path (`JM7 — GOP fb
> addr=0x0`, banner `headless serial console`). So all boot/CAPSTONE output goes to UARTC serial, never
> the panel — even though the firmware itself IS driving the monitor at 1920×1200 (its display
> controller is live and scanning out a framebuffer from DRAM; the panel holds the firmware's last
> frame). The UEFI-GOP route to a framebuffer is a **dead end** on this board. Getting pixels needs
> UnaOS to **inherit the firmware's live scanout FB** (read the Tegra234 nvdisplay scanout base, map
> it, blit — the JB6→JB9 "inherit, don't re-init" pattern) or drive the Tegra DC natively. That is the
> next arc, **JD1** (baton `~/.claude/plans/unaos-jetson-display-BATON.md`).

1. **Nested-hub descent** (shared `drivers/xhci/mod.rs`, additive; the hub FSM is dead code under
   the QEMU suites, so this cannot regress x86/Pi4). `enumerate_downstream` now detects a downstream
   device that is itself a hub (device class `0x09`) and pushes its slot into `hubs_pending`, so the
   next `service_hubs` pass brings it up and descends another tier — mirroring the root-port `HUB
   DETECTED` push. `DeviceSlot` gains `route_string`/`route_depth`; `bring_up_hub` extends the route
   for each child (`route | (port << (4·depth))`, `depth+1`) with a 5-tier cap (the xHCI Route String
   is 20 bits = 5 nibbles). `address_downstream` programs the Slot Context DW2 Transaction Translator
   (TT Hub Slot ID / TT Port Number) for Low/Full-Speed children — the immediate parent hub is the TT
   for the common single-level-HS-hub topology; HS/SS children keep DW2 = 0, so the working VIA-hub
   HS path is byte-unchanged. Root devices leave `route_*` at 0 (addressed by the root FSM), cleared
   in `reset_soft_state`/`new` for recycled-slot safety.
2. **Full-Speed EP0 Evaluate-Context** (`drivers/xhci/mod.rs`, `#[cfg(feature="tegra")]`,
   const-toggle `JB10_FS_EVAL_CTX`, default on; **HYPOTHESIS — verify on the bench**). The JB9 baton
   framed port 7 as "needs a fresh port reset", but the serial refutes that: the babble→recover path
   *already* does DISABLE_SLOT + fresh port reset + re-ADDRESS at the correct MPS0=64 — and the FS
   device then goes **silent** (dev-desc watchdog-times-out, PORTSC still `0x603` connected). The
   device responds at MPS0=8 (it babbles = it sent data) but not after the tear-down churn. So JB10
   adopts Linux `xhci_check_maxpacket`: for a Tegra FS root device, read the first 8 bytes (one
   packet, no babble), learn `bMaxPacketSize0`, patch EP0 MPS0 **in place** via an Evaluate Context
   command (TRB type 13, Add-flag A1, EP0 copied from the *output* context), then read the full
   descriptor — all on the same slot, no DISABLE_SLOT, no reset. Deferred to `service_enum`
   (`fs-mps-learn` stage, main-loop context where `sync_control`/`run_command_sync` are safe, never
   inside `poll_events`). On any failure it falls back to the shared babble→recover path. Non-tegra
   builds are byte-identical (the whole path is `cfg(tegra)`; `begin_device_descriptor` collapses to
   the prior `request_device_descriptor` call). ⚠ A review pass caught the Evaluate-Context source
   offset (the output DeviceContext has no Input-Control prefix, so EP0 is at `1·CTX_WORDS`, not
   `2·CTX_WORDS`) — fixed before commit; the wrong offset would have submitted a null-ring EP0 that
   the strict Tegra FW rejects (code 17), silently defeating the lever.
3. **Direct-root keyboard demo** — no code change. The path already exists end to end (root device →
   `poll_events` class-detect → `configure_hid_endpoints` → `keyboard_state` 1→2→3 → `keyboard_armed`
   → `:: tegra: JB2b — keyboard ARMED (slot, root port) -> PASS ::`, then `kbd_pump_body` prints
   `:: tegra: JB2b — KEY '<c>' ::`). Item 2 improves the odds for a Full-Speed root keyboard on the
   Tegra FW. Bench procedure: plug a USB keyboard directly into an Orin root port (no hub) and boot;
   watch for `keyboard ARMED -> PASS` then a `KEY` line per keystroke.
4. **Inherit-path housekeeping** (`xusb_tegra.rs` + `main.rs`, tegra-only, zero x86/Pi4 blast radius).
   `JB9_PROBE` defaults **false** now the verdict is in — it silences the diagnostic suite
   (`jb9_fw_alive`, `jb9f_inherit_run_probe`, the `jb9_fw_sid_view`/`jb9_ring_scan`/`jb9_stream_dump`
   pump-window captures). It does **not** touch the working recipe: the JB9g no-HCRST takeover and
   JB9i slot eviction gate on `JB9G_NO_HCRST`, and `jb5_bar2_route` (load-bearing) gates on
   `JB5_PROBE` — both stay true. Two compile-time tripwires
   (`const _: () = assert!(!(JB4_ALLOW_PG_CYCLE && JB9G_NO_HCRST))` and the `JB5_RUN_E_REPLAY` twin)
   make the two firmware-destroying levers un-co-enable-able with the inherit recipe: a wrong pairing
   now fails the build, not a boot. The `main.rs` JB4 revival block is wrapped in `if !jb9h_skip` for
   symmetry with the JB3 chain. The forensic kit is **kept, not deleted** — flip `JB9_PROBE` back to
   `true` at the next bench when a failed downstream enumeration needs the telemetry; retiring the
   dead JB3/JB4/JB5 chain code is a separate scoped cleanup arc (it spans `bpmp_tegra`/`smmu_tegra`).

**Build note:** with `JB9_PROBE=false` the optimizer prunes the dead `if JB9_PROBE` diagnostic call
sites, so `esp-jetson` `kernel.elf` is **~242 KB / ~90 `tegra:` strings** (down from ~257 KB / ~100–140
with the probes on) — this is the intended shrink, **not** a broken/virt-clobbered build; the corrupt
RED LINE remains ~355 KB.

**Known limitations (follow-ups, not regressions):** (a) a *normal* hot-unplug of a root port hosting
a nested hub tree tears nothing down (the disconnect handler only logs) so the subtree's slots/contexts
leak — pre-existing behaviour for tier-1 too, bounded, no UAF, matching the codebase's forget-don't-free
model; the *recovery* path does cascade-dispose correctly. (b) TT attribution is correct only when every
intermediate hub above a LS/FS device is High-Speed (single-level is the target); a LS/FS device below a
non-HS intermediate hub is not handled. (c) a SuperSpeed device *behind* a USB3 hub is mis-speeded
(`reset_downstream_port` decodes LS/HS/FS only, no SS) — not needed for the keyboard/mouse demo.

### JD1 — first pixels: inherit the firmware's live scanout framebuffer (QEMU-inert, ⏳ METAL-PENDING)

JM7 established the block: the panel is dark not because the display is off but because NVIDIA's UEFI
publishes its GOP in **`BltOnly`** mode — no CPU-linear framebuffer, so `BootInfo::framebuffer_addr == 0`
and fbcon stays inert — even though the firmware's display engine is very much alive and scanning out a
framebuffer from DRAM (the panel holds the last frame at 1920×1200). JD1 lights UnaOS's own boot console on
that panel by **inheriting** the firmware's live scanout rather than re-initialising the pipeline (the
JB6→JB9 "inherit, don't re-init" lesson, applied to display).

**The decisive finding (edk2-nvidia source, cross-checked against mainline Linux `drm/tegra`):** the GOP is
`BltOnly` *on purpose*. The Orin's `SocDisplayHandoffMethod` defaults to **SIMPLEFB**, under which the
firmware deliberately withholds a linear GOP surface and instead hands the framebuffer off **through the
device tree**. MB2/TegraBL allocates the scanout as a fixed DRAM carveout (`CARVEOUT_DISP_EARLY_BOOT_FB`)
that the DCE (the RISC-V ucontroller owning nvdisplay) scans out; at ReadyToBoot the UEFI display driver
writes a `compatible = "simple-framebuffer"` node into the FDT it exposes — geometry (`width`/`height`/
`stride`/`format`) on the node, and the **physical** base/size in the node's `memory-region` reserved-memory
`reg`, with `iommu-addresses = <&dcN base size>` declaring the display-IOMMU **identity** map (so the reg PA
equals the DC's scanout IOVA). The bootloader already captures that FDT into `BootInfo::dtb_addr`, so the
live scanout PA is sitting in a DTB the kernel can walk. This is a strictly better inherit path than reading
the nvdisplay window register (which would be an SMMU IOVA, double-buffered, and behind a possibly-gated
block): **a pure RAM walk — no display MMIO, no SMMU translation, no active/assembly hazard, no
EL3-fatal-touch risk.**

**Implementation (all `cfg(feature = "tegra")` → non-tegra x86/`virt` builds are byte-identical; QEMU never
compiles `tegra`, so this is inert in every regression):**
- **`fdt_tegra::nvdisplay_simplefb`** resolves the handoff: finds the first non-`disabled`
  `simple-framebuffer` node, reads `width`/`height`/`stride`(bytes)/`format`(string), follows
  `memory-region` → reserved-memory `reg` (2 addr + 2 size cells) for the physical base+size. `jd1_dump` is
  the human-readable twin (prints every `simple-framebuffer` node and every `fb`/`framebuffer`/`display`
  reserved-memory carveout), so a bench boot is self-diagnosing even when the strict resolver rejects a node.
- **`display_tegra::jd1_survey`** maps the `format` string to a `FrameBuffer` layout (`"x8r8g8b8"` →
  in-memory `[B,G,R,X]` = UnaOS `Bgr`; `"x8b8g8r8"` → `Rgb`), converts the byte `stride` to the pixel
  `FrameBufferInfo::stride`, sanity-gates the result (DRAM base ≥ `0x8000_0000`, sane geometry, carveout
  covers the visible image), and prints the `:: tegra: JD1 — scanout: base=… size=… WxH stride=…B fmt=… ::`
  verdict. `None` (no handoff / bad geometry) → the boot stays headless, byte-identical to pre-JD1.
- **`mmu_tegra::map_fb_region`** maps the carveout's GiB span Normal-WB into **both** the live EL2 `L1` and
  the EL1 twin `L1_EL1` (cleans the edited descriptors to PoC; `tlbi alle2` for the active EL2 regime; the
  EL1 twin is picked up by the JM6 drop's own `tlbi vmalle1`), so the panel keeps working across the
  EL2→EL1 drop. Idempotent — a GiB already mapped by `build_l1` is confirmed, not re-written; it refuses
  GiB 0/1 (Device/SYSRAM) and out-of-range bases.
- **`main.rs::tegra_early_stop`** (right after the JM7 report): survey → map → `jd1_test_pattern` (eight
  colour bars + a bright frame, so a wrong stride shears the bars and a wrong format swaps blue↔red) →
  `fbcon::init` on the inherited scanout. fbcon is *not* detached on the tegra path, so from here every
  `serial_println!` (JB1a … JM4 … and the EL1 CAPSTONE, across the JM6 drop) also paints onto the panel.
  CPU-write → scanout coherency rides fbcon's existing damage-tracked `flush_range` → `dc cvac` (the
  Pi-HVS recipe — the DCE does not snoop the CPU cache), valid on Normal-WB. The shared renderer
  (`video/framebuffer.rs`/`fbcon.rs`/`screen.rs`) is **unchanged** — JD1 only feeds it an address+geometry.

**DC-register fallback (default OFF).** For the case where the firmware published no simple-framebuffer node
into the DTB we received, `display_tegra::jd1_dc_survey` (behind `const JD1_DC_PROBE = false`) is a read-only
sweep of the nvdisplay window registers. Register map, cross-checked against mainline `drm/tegra`
`dc.h`/`hub.c` (register numbers are **dword** indices, byte = index≪2): the display block is
`display@13800000` (compatible `nvidia,tegra234-display`, ~0xEF000, inside the GiB-0 device window), heads at
`0x13800000 + n·0x10000` (n=0..3; the one bench-confirm number, T194-derived), per-window aperture at
`head + 0x2800 + 0xC00·i` with `WIN_OPTIONS`(WIN_ENABLE bit30) `+0x600`, `WINDOWGROUP_SET_CONTROL`(OWNER)
`+0x608`, `COLOR_DEPTH` `+0x60c`, `SIZE` `+0x614`, `CROPPED_SIZE` `+0x618`, `PLANAR_STORAGE`(stride/64)
`+0x624`, `START_ADDR` `+0x700`, `SURFACE_KIND`(0=pitch) `+0x72c`, `START_ADDR_HI` `+0x734`. The read base is
an SMMU IOVA with **bit 39 a GPU sector-swizzle flag to mask**. It is default-off because the DTB path is
safe + primary and touching a *powergated* display block would be EL3-fatal (the JX1 lesson) — the panel is
lit so it is *believed* powered, but the DTB path proves pixels without betting on that. It touches only
plain config registers within the block's own decoded aperture (never the read-to-clear status region).

**Status — ✅ METAL-CONFIRMED (2026-07-08, attended bench; Peter at the Orin, DP→HDMI monitor).** The
firmware DID publish the SIMPLEFB handoff into the FDT we captured: `JD1 — simple-fb /chosen/framebuffer:
1920x1200 stride=8192 fmt='x8r8g8b8'` → `resv /reserved-memory/framebuffer@2,79E00000 reg[0x2 0x79e00000 0x0
0x960000]` → `scanout: base=0x279e00000 size=0x960000 1920x1200 stride=8192B fmt=x8r8g8b8 (Bgr) sane=true` →
`panel LIVE`. The scanout base is in GiB 9 (already RAM-mapped — `map_fb_region` confirmed it) and the
carveout size `0x960000` = exactly `8192×1200`. **On the panel:** the colour-bar test pattern rendered
pixel-correct — clean vertical bars (black·blue·green·cyan·red·magenta·yellow·white), a crisp full-frame
border, correct colours (blue 2nd / red 5th → the `x8r8g8b8 → Bgr` decode is right; a swap would trade
blue↔red and cyan↔yellow), no shear (stride right), full-screen (base/geometry right) — then fbcon painted
the whole boot log + `CAPSTONE COMPLETE` across the EL2→EL1 drop. UnaOS's first deliberate, pixel-correct
frame on the Orin. A 3 s hold (`JD1_TEST_PATTERN_HOLD_SECS`, a `CNTPCT` busy-wait) keeps the pattern legible
before the console takes the screen; set it to 0 to skip. `UNAOS_TEGRA=1 ./arroyo check` green both arches;
`./arroyo test` (x86) + `test-arm` (aarch64 virt) byte-green; `esp-jetson` `kernel.elf` = **250,416 B / 101
`tegra:` strings** (up from JB10's 241,936 B / 90; RED LINE ~355 KB). The DC-register fallback (`JD1_DC_PROBE`)
was never needed — the DTB path carried it. NEXT (JD2): route the inherited USB keyboard to a live shell on
the panel. A blank panel after a correct verdict = wrong base/stride/format/memory-type, **not** "re-init
needed" (do not reset the DC/SOR/DP).

### JD2 — interactive shell on the panel: keyboard → console → shell over the inherited scanout (✅ METAL-CONFIRMED 2026-07-10)

JD1 put pixels on the panel; JB10 armed a USB keyboard whose HID reports land in the shared `pal`
event queue. JD2 joins the two: the Orin's first interactive session, drawn on the inherited scanout
and typed on the inherited keyboard, with **no new hardware touched** — pure software routing over
the JD1/JB10 state.

**Design (all `cfg(tegra)`, all in `main.rs`'s tegra blocks — the shared renderer/console/shell are
called, never edited):**

- `tegra_early_stop` now also seeds `video::WRITER` with the JD1 scanout (the same `{base, len,
  info}` fbcon got) — the front-buffer handle the GUI `Screen` builds over, the x86/pi pattern.
- The JB2b `jb2-kbd` spawn is replaced by **`jd2-console`** → `main.rs::jd2_console_pump`, a
  cooperative EL1 task on the boot-core run queue (spawned pre-drop, dispatched by
  `run_capstone_boot_core` alongside CAPSTONE). It keeps the JB2b discipline: `poll_events` only
  (never the `service_*` pumps — their bounded waits WFI, and the post-drop core has no timer
  wake), busy-poll + `yield_now`, never `sleep_ticks`.
- **Phase 1:** the JD1 boot log holds the panel; the pump polls the xHCI until the **first
  keystroke**. **Phase 2:** `fbcon::detach()` (serial mirror off the panel; serial itself
  unaffected), build the double-buffered `video::Screen` over `WRITER` (~9 MiB back buffer off the
  48 MiB heap), draw the `Console`, and feed every key — including the wake-up key — through the
  shared `handle_key` → `shell::dispatch_command`. Each keystroke repaints only the input line;
  `Screen::flush` cleans the damaged span to PoC (`dc cvac`) once per present — the DCE scans the
  carveout from DRAM and does not snoop, the recipe fbcon already proved on this panel. Every key
  is also echoed to serial (`:: tegra: JD2 — KEY … ::`) so the bench proof rides both channels.
- **Headless boot** (no JD1 handoff → `WRITER` never seeded): the task delegates to the JB2b
  `kbd_pump_body`, so a serial-only bench keeps the exact pre-JD2 `KEY` evidence lines.

**Gate:** `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test` (x86) + `test-arm`
(aarch64 virt) green — all JD2 code is `cfg(tegra)`, off in every QEMU build, so non-tegra images
are byte-identical by construction. ⚠ **New size datum:** `esp-jetson` `kernel.elf` = **378,728 B /
105 `tegra:` strings** (up from JD1's 250,416 B / 101). The +128 KB is the console/shell/FAT/font
machinery the linker previously dead-code-eliminated from the tegra image — `dispatch_command` now
being reachable pulls it all in. This is **past the old ~355 KB "red line"**, which was only ever a
clobbered-virt-build heuristic: from JD2 on, validate tegra media by the `tegra:`-string count
(≈105; a virt clobber has ~0), not by size.

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-10, attended bench; Peter at the Orin, this session
flashed the card itself).** Serial (`~/unaos-bench/jetson-serial-2026-07-10-090000.log`): JD1
unchanged (`scanout … sane=true` → `panel LIVE`), keyboard ARMED on **direct root port 6 (slot 4)**,
`JD2 — EL1 console pump task spawned` → post-drop `console pump live`, `CAPSTONE COMPLETE`, then
the first keystroke (0x0a) → `JD2 — console OWNS the panel (Screen back buffer live)`. Peter typed
`help` ⏎ (each key echoed as a serial `KEY` line; the command list painted on the panel — verdict
"it works!"), then `gneiss` ⏎ and more input: **the pump survived vug** — keystrokes kept flowing
after it, no panic/wedge (vug-on-tegra behaviour beyond survival not formally assessed this bench).
The first interactive UnaOS session on the Orin: typed on the inherited keyboard, drawn on the
inherited scanout, dispatched by the shared shell at EL1.

### JD3 — storage behind the hub: real `ls`/`cat` on the panel shell (✅ METAL-CONFIRMED 2026-07-10)

JD2 gave the Orin an interactive shell, but its `ls`/`cat` had no disk behind them (`storage_slot`
stayed 0 — JB10 saw the Alcor reader enumerate behind the hub but never claimed it). JD3 brings the
mass-storage device up, mounts FAT off it, and routes the panel shell's `ls`/`cat` to real files.
The shell↔FAT↔block wiring is already architecture-neutral (`shell::dispatch_command` → `fs::fat::mount`
→ `drivers::block::read_block` → the xHCI BOT pump), so **no per-arch shell code was needed** — the
work is entirely in *when* and *how* the tegra path drives the block device.

**M1 — MSC bring-up in the pre-drop attach window.** `enumerate_downstream` already detects a
hubbed Mass-Storage interface (`parse_msc_config`, added with the rMBP hub-downstream fix `3bee9d6`)
and defers its SCSI bring-up to `service_storage`. JD2's `jb2b_attach` pump ran `service_hubs` /
`service_enum` but **deliberately not** `service_storage`; JD3 adds it. This must happen **pre-drop,
at EL2, while the JM4 timer is live** — `service_storage → bring_up_storage` runs the driver's
heaviest synchronous path (SET_CONFIGURATION, TEST-UNIT-READY, INQUIRY, READ CAPACITY, a sanity
READ(10)), and every stage's BOT/control pump rides `crate::hlt()`, which the *post*-drop timerless
core cannot wake (the JD2/JB2b rule). Once the disk is up it publishes `drivers::block::BLOCK_DEVICE`
and the pump returns. Because a hubbed device enumerates *after* the root ports, the pump now keeps
running for a bounded **storage-settle window** (8 s) once the keyboard is armed — returning the
instant the disk reports ready, or when the window closes (a keyboard-only boot pays the wait once;
`service_storage` is a no-op with nothing pending, so JD2 keyboard-only boots are otherwise
unchanged). Serial evidence: `>>> HUB DOWNSTREAM MASS STORAGE (slot N …) <<<` → `Endpoints Configured
(Slot N). Storage ready.` → `Disk '…' block_size=512 …` → `JD3 — mass storage ready (slot N); panel
shell ls/cat live`.

**M2 — post-drop reads on a timerless core (the crux).** The shell runs at EL1 *after* the JM6
EL2→EL1 drop, and the drop disables the physical timer (`CNTP_CTL=0`) so CAPSTONE can run
cooperatively. But `boot_tegra::drop_to_el1` left `timer::LIVE` reading **stale-true** (`verify_live`
set it at EL2), so `arch::hlt()` — which does `if is_live() { wfi } else { spin_loop() }` — would
**WFI-park the core forever** the first time `ls`/`cat` → `block::read_block` → the BOT pump yielded.
Two changes fix this:

- **`timer::set_not_live()` after the drop** (`main.rs::tegra_early_stop`, tegra-lane): the timer is
  genuinely off, so `hlt()` correctly falls back to a busy spin instead of a wake-less park. Verified
  safe — nothing on the post-drop tegra path relies on `hlt()==WFI` or `is_live()==true` (the boot
  core's drive loop `spin_loop()`s directly; `yield_now` never `hlt()`s; every other `is_live()`
  reader is `baremetal`-gated and not compiled for tegra).
- **`pump_until_bot_done` → wall-clock budget** (shared `drivers/xhci/mod.rs`): a busy-spinning
  `hlt()` makes the old 2000-**iteration** budget expire in microseconds — before a real DMA
  completion. The pump now bounds itself with a `now_cycles`/`hw_wait_budget` deadline (the idiom the
  enumeration FSM already uses), so it is correct whether each `hlt()` waits a tick (x86/Pi/virt,
  pre-drop tegra) or busy-spins (post-drop tegra). This is the exact pattern the Pi's polled EMMC2
  driver already relies on — a free-running counter (`CNTVCT`/`CNTPCT`) keeps advancing with the
  timer *interrupt* off. The change is **arch-neutral** (both primitives are monotonic and
  IRQ-independent on x86 and aarch64) and strictly more robust for every target; it is guarded by the
  x86 `UNAOS_HUBSTORAGE=1 test` and aarch64 `test-arm 22` MISSION regressions (both exercise the BOT
  pump). *This edits the shared xHCI driver — the "xHCI seam" the JD3 brief pre-authorised the jetson
  track to request from the integrator; it is `cfg`-neutral and benefits all arches.*

**M3 — retire the dead JB3/JB4/JB5 revival machinery.** With storage working through the inherit
path, the "revive the halted Falcon" code (JB3 fabric chain, JB4 partition-cycle levers, JB5
UEFI-replay) — dead since the JB9 inherit pivot, gated off by `JB9H_SKIP_CHAIN`/`JB4_ENABLE`/
`JB5_RUN_E_REPLAY`/`JB4_ALLOW_PG_CYCLE` (all const-false on the working boot, so already
optimizer-pruned from the binary) — was removed from source: the `!jb9h_skip` / `JB5_RUN_E_REPLAY`
call blocks in `main.rs` (the live JB1c ungate + JB2c pad power-up kept), and the now-orphaned
functions in `bpmp_tegra.rs` (`jb5_linux_order_ungate`, `jb5_clocks_on`, `jb5_uefi_pg_cycle`,
`jb4_reassert_falcon`, `jb4_powergate_cycle`), `xusb_tegra.rs` (`jb3_fpci_enable`, `jb3_aru_probe`,
`jb3_falcon`, `jb4_falcon_revive`, `xusb_cnr_set`, `jb5_elpg_release`, `jb5_fpci_uefi_and_poll`,
`jb5_settle_witness`, `JB4_ENABLE`, `FALC_CPUCTL`), and `smmu_tegra.rs` (`jb3_probe`,
`jb3_open_stream`, `jb3_install_identity_cb0`, `jb3_mc_sid_fix`, `MC_SID_XUSB_HOSTR`). **Explicitly
KEPT** (still load-bearing on the inherit path): the ⭐ JB9 recipe (`JB9G_NO_HCRST`,
`JB9H_SKIP_CHAIN`, `context::CTX_WORDS`, `JB5_PROBE` + `jb5_bar2_route`); **both** compile-asserts
that make the firmware-destroying levers un-co-enable-able (`JB4_ALLOW_PG_CYCLE`/`JB5_RUN_E_REPLAY`
consts survive to feed them); the shared XUSB register consts (`XUSB_FPCI`, `XUSB_BAR2`, MMU-500
`GR1_OFF`/`CB0_OFF`/`MC_SID_BASE`, `JB3_IDMAP`); the read-only post-attach diagnostics
(`jb3_faults`, `jb3_mc_errs`, `jb3_v3_dump`); the JB5/JB6/JB7 witnesses; and the JB9 forensic kit.
Nothing touching CPUCTL/BOOTVEC or MRQ_PG can be re-enabled without failing the build. (The now-dead
JB2c re-power-up and the JB9b SID levers are out of the JB3/JB4/JB5 remit and left for the seat.)
Net: `bpmp_tegra.rs` −217, `xusb_tegra.rs` −405, `smmu_tegra.rs` −220 lines; behaviour-neutral (the
deleted code was already gated off, and the `tegra:`-string count is unchanged at 107).

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches;
`UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (`storage_slot=2 note='ready'`, no BOT
timeout); `./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` →
`CAPSTONE COMPLETE` 6/6; `esp-jetson kernel.elf` links, **107 `tegra:` strings** (JD2 was 105;
validate tegra media by `tegra:`-string count, not size — the JD2 rule; the elf is ~388 KB, the
handful of extra KB vs M1+M2 is optimizer inlining re-balancing after the dead code left, not bloat).

**⚠ Metal is the real verdict — QEMU has no Alcor.** The tegra path never compiles under any QEMU
gate, so the post-drop timerless BOT busy-poll is **metal-unexercised**; QEMU proves only the
arch-neutral BOT-pump change (on x86/virt, where `hlt()` still waits on an interrupt) and that the
tegra image links. The attended-bench risks to watch: (a) hub-MSC **power/timing** on the real Alcor
reader (does its SCSI bring-up complete inside the 8 s settle window?); (b) the wall-clock budget
sizing for a real USB-MSC read latency (bump `hw_wait_budget()`'s BOT multiple if a real read
marginally times out); (c) `set_not_live()` + the busy-poll pump actually completing a read on the
timerless EL1 core. Bench proof = flash, then on the panel shell: `diskinfo` (geometry), `ls` (the
card's root), `cat <known file>` (its bytes).

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-10, attended bench; Peter at the Orin).** Serial
`~/unaos-bench/jetson-serial-2026-07-10-104357.log`. An SD card in the **Alcor USB reader behind the
hub** enumerated on the retry boot — `HUB downstream slot 5 device … vid=058f pid=6362 (route 0x4 tier
1)` → `>>> HUB DOWNSTREAM MASS STORAGE (slot 5, bulk in 0x82/512 out 0x1/512) <<<` → M1's
`service_storage` ran the SCSI bring-up **in the pre-drop pump**: `Disk 'Generic' 'USB SD Reader'
block_size=512 num_blocks=60800 (29 MiB)` → `READ(10) LBA0 CSW status=Passed residue=0` →
`JD3 — mass storage ready (slot 5); panel shell ls/cat live` (the disk was up well inside the 8 s
settle window, so the pump returned keyboard-armed + storage-ready). Then post-drop, on the panel
shell, `diskinfo` / `ls` / `cat` read the FAT card — the M2 crux: **synchronous BOT reads completing on
the timerless EL1 core** via `set_not_live()` + the wall-clock `pump_until_bot_done`. Peter's verdict:
**PASS**, and **zero `BOT pump TIMEOUT` lines** across the whole session — the ×3 `hw_wait_budget`
budget was ample for the real USB-MSC read latency. The first real filesystem content on the Orin
panel. ⚠ Bench note confirming the flagged risk: the hub-downstream enumeration of the reader is
**intermittent** — the *first* boot the hubbed LS/FS devices failed (`ADDRESS_DEVICE code 4`, a
`vid=0000` descriptor read, an FS `dev-desc` recovery loop) and the settle window correctly fell
through to `no mass storage within the settle window; proceeding` (graceful, no wedge); a re-seat +
power-cycle brought the Alcor up cleanly. So the JD3 code path is solid; the residual variability is
hub-MSC power/timing on the real reader (a direct-root USB stick sidesteps it entirely).

### JD4 — read-side FAT navigation on the panel shell + the last dead-lever retirement + screen-on-boot (✅ METAL-CONFIRMED 2026-07-10)

JD3 gave the panel shell a real disk but only a flat root: `ls` listed the root, `cat` took a bare
root filename. JD4 is the READ-side navigation arc (the write path is deliberately deferred to a
future JD5 — pi4's F3 namespace-lock arc is about to churn `fat.rs`, and a jetson write path would
both collide with and want those locks), plus two leftovers the seat blessed: the dead JB2c/JB9b
levers and JD2's screen-on-boot polish.

**M1 — `ls <dir>` / `cd` / `pwd` / `cat <path>` (`5ca6e28`, arch-neutral).** One seat-granted,
purely additive read helper in `fat.rs`: `pub fn read_dir(first_cluster)` — the public face of the
existing `read_root`/`read_dir_chain` walkers; cluster `0` means the root (the value a
subdirectory's `..` entry stores when its parent is the root, and the FAT16 fixed root's
convention). Read-only, takes NO lock (F3 may revisit read-side locking); placed in the read
section away from the mutation code F3 will churn; no existing line touched. Everything else is
`shell.rs`: the cwd lives as a **normalized, canonical absolute path string** (`/DIR/SUB` in the
on-disk 8.3 spelling; `None` = root, no heap until the first `cd`), **re-resolved from the root on
every command** — a swapped or remounted card can never leave the shell holding a stale chain head;
the worst case is an honest `-ENOENT`. `normalize_path` folds `.`/`..`/`//`/absolute joins
lexically (`..` never climbs above root); `resolve_path` walks components via `read_dir` with
case-insensitive 8.3 matching and returns errno-tagged errors that are always printed, never
swallowed (`-ENOENT`, `-ENOTDIR`, `-EISDIR`, `-EIO`). `ls [dir]` lists the cwd or any
absolute/relative path (an `ls` of a plain file prints its one table line — the DOS idiom);
`cd [dir]` canonicalizes, verifies it is a directory, stores + echoes the canonical path (no arg =
root); `pwd` prints it; `cat <path>` resolves a full path (`cat DOCS/README.TXT`) then reads via
the unchanged bounded `read_file`. The shell stays arch-neutral — the same commands work on x86 and
both aarch64 targets; LFN entries are skipped (short names only), exactly as before.

**M2 — the dead JB2c/JB9b levers retired (`436d7ef`, behaviour-neutral).** The two lever groups
JD3's M3 explicitly left ("JB9/JB2c-named, not JB3/4/5"): `jb2c_padctl_powerup` +
`jb2c_usb2_trk_clk` (the pre-inherit pad re-power-up — on the JB9g/h inherit path the JB6 shim
keeps UEFI from tearing the pads down, and the `main.rs` call was gated on `!JB9H_SKIP_CHAIN` =
never) and `jb9b_ao_sid_fix` + `jb9b_accept_bypass_sid` (the SID-mismatch levers, dead behind the
same inherit gate AND `JB9_PROBE=false`; JB9f proved the inherited fabric passes the FW's DMA
as-is). Their orphaned private helpers went too: the padctl register consts + `pr32`/`pw32` +
`poll_trk_completed` (`xusb_tegra.rs`), `CLK_USB2_TRK` (`bpmp_tegra.rs`), and the smmu `wr` +
TLB-sync consts (`smmu_tegra.rs`). **KEPT, untouched:** the ⭐ JB9 recipe, BOTH
firmware-destroying-lever compile-asserts (their guard consts survive to feed them), the read-only
diagnostics, and the JB9 forensic kit. Net −313 lines; `JB2c`/`JB9b` strings verified absent from
the linked elf. The pre-inherit pad-power-up recipe (Linux `tegra186_utmi_*_power_on` for tegra234)
lives in the git record at JD3 and earlier if a non-inherit boot path ever returns.

**M3 — screen-on-boot (`195ab88`, tegra-only).** JD2 held the fbcon boot log on the panel until a
blind first keystroke. `jd2_console_pump`'s phase 1 is now bounded: the console takes the panel at
the **first keystroke OR a ~8 s CNTPCT wall-clock deadline**, whichever comes first (free-running
counter — the JD3 timerless mechanism; the post-drop EL1 core has no timer IRQ, so no tick-based
wait is possible). 8 s is long past the CAPSTONE stragglers, so the takeover cannot race a late
fbcon paint — the reason JD2 could not simply draw at task start (that would also have erased the
JD1 boot-log demo). A keystroke inside the window keeps the exact JD2 behaviour (fed through, not
swallowed); the timeout path draws the banner + prompt and logs
`:: tegra: JD4 — console OWNS the panel … screen-on-boot (no key, ~8 s) ::`. Headless boots
(WRITER unseeded) still delegate to `kbd_pump_body` — the JB2b serial evidence contract holds.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches;
`UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (run because M1 touches the shared
`shell.rs`/`fat.rs`); `./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm
40` → `CAPSTONE COMPLETE` 6/6; `esp-jetson kernel.elf` links, **108 `tegra:` strings** (the JD4
count: −9 retired JB2c/JB9b lines +… net of the POLISH-1/2 merges the branch rebased onto; validate
by count, not size — the elf is ~452 KB with the polish-era console/vug machinery linked).

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-10, attended bench; Peter at the Orin; serial
`~/unaos-bench/jetson-serial-2026-07-10-135751.log`).** Peter's verdict: **PASS**, all three
milestones in one session across three boots:

- **M3 — 3/3 boots**: the panel console took over ON ITS OWN each boot (`:: tegra: JD4 — console
  OWNS the panel (Screen back buffer live); screen-on-boot (no key, ~8 s) ::`), after CAPSTONE 6/6,
  zero fbcon races.
- **M2 — every boot**: zero `JB2c`/`JB9b` serial lines; keyboard + storage enumeration unaffected.
- **M1 — boot 3** (a **FAT16** card — `UNAOSRW`, the 29 MiB pi4 fixture card with a fresh
  `DOCS/README.TXT`, in the Alcor reader behind the hub, slot 5): the full navigation sequence on
  the panel — `diskinfo` → `ls` (root: `<DIR> DOCS` + the pi4 fixtures) → `cd docs` → `ls` →
  `cat readme.txt` → `pwd` → `cd ..` → `cat /docs/readme.txt` → honest-error probes `cd nosuch`
  (`-ENOENT`) and `cat docs` (`-EISDIR`). Bonus proofs beyond the QEMU gates: the whole sequence
  was typed **lowercase** (case-insensitive 8.3 matching on silicon) and the card is FAT16 (the
  fixed-root → subdirectory-cluster-chain leg of `read_dir`, which the FAT32 QEMU images don't
  exercise on tegra).

⚠ Bench reconfirmed the JD3 intermittency datum, twice: boots 1–2 had the storage device on hub
route 0x4 read `vid=0000` (failed descriptor) and the settle window fell through GRACEFULLY each
time (`no mass storage within the settle window; proceeding` — the shell still came up, reporting
no disk honestly). Boot 3 with the reader re-seated came up clean. Also learned: the Orin boot
stick itself did not surface as the block device on this bench — plan on a SEPARATE data
card/reader for tegra storage benches (the pi4-style boot-disk read is not the tegra pattern).

### JD5 — the write path: create / edit / delete files from the panel shell (✅ METAL-CONFIRMED 2026-07-10)

JD4 made the panel navigable read-only. JD5 makes the Orin a machine you can DO WORK ON: `touch`,
`write`, `append`, and `rm` from the shell, through the SAME F3-locked FAT stack the U9/U10/U11
syscalls and the QEMU fixtures prove. The arc completes the panel story: first pixels (JD1) → first
keystroke (JD2) → first disk read (JD3) → first navigation (JD4) → first write (JD5).

**Design — why the write path rides `fat.rs` directly, not the `SYS_OPEN`/`SYS_WRITE` syscall layer.**
The brief allowed either the syscall path (preferred where reachable) or the `fat.rs` public mutation
API (where not). The syscall path is NOT reachable from the kernel shell: `SYS_OPEN`/`SYS_WRITE` are
EL0/ASID-keyed and dispatched from the SVC handler in the out-of-lane `arch/aarch64/syscall.rs`, and
the kernel shell runs at **EL1 as ASID 0** — on the tegra post-drop core `TTBR0_EL1[63:48] = 0` (the
drop loads a bare table PA with no ASID bits; the shell never switches TTBR0 to a user slot). So the
shell rides the same `fat.rs` PUBLIC entry points the U9/U10/U11 syscalls call — `create_in_root`,
`find_located`, `write_grow`, `delete_located` — and **`fat.rs` stays call-never-edit this arc**
(pi4's K1 arc owns its mutation concurrently; JD5 only CALLS the existing public API under its
existing lock contracts).

**Principal — the kernel shell IS ASID 0, the public principal.** By U6's existing rule an ASID-0
`O_CREAT` is always PUBLIC (no owner row — ASID 0 is the shared/boot window, never gen-fenced, never
torn down, so it cannot be a private owner). Shell-created files are therefore plain public FAT
files. The shell does not — and cannot without an out-of-lane `pub` accessor — consult the U6
`OWNED_FILES` ACL, which lives entirely on the SVC `sys_open` path; that is correct, because the
panel shell is the local trusted operator console, the same trust level as the shared boot window.
(A future arc that runs EL0 tasks and returns to the shell must re-establish ASID 0 before shell FAT
ops; today the shell is cooperative EL1 and never installs a user-slot TTBR0.)

**Scope — root-directory only.** `create_in_root`/`find_located` operate on the root directory only,
and `fat.rs` is call-never-edit, so a target whose parent is a subdirectory is an honest `-ENOTSUP`
(a subdirectory create needs a future `create_in_dir`). Bare names resolve against the cwd; `.`/`..`
normalize lexically first; only a root parent is writable. This is the one capability JD4's read-side
navigation has that JD5's write side does not — deliberately, to stay within the call-never-edit
contract while pi4's F3/K1 churns the mutation code.

**M1 — `touch` + `write` (`3a143f5`, arch-neutral).** `touch <path>` creates a 0-length root file if
absent (idempotent). `write <path> <text>` is create-or-TRUNCATE + store the exact bytes: a truncate
of an existing file is `delete_located` (free chain + `0xE5`) then `create_in_root` then `write_grow`
from offset 0 — the only create-or-truncate reachable through the PUBLIC API (there is no in-place
shrink primitive, and the directory-field publisher is private). The existing `write <lba> <byte>`
raw-block command is preserved byte-identically for its exact 2-numeric-arg shape; any other shape is
the file write (text = rest of line, whitespace-collapsed like `echo`); a numeric filename stays
reachable as `/NAME`.

**M2 — `rm` + `append` (`dfaf180`, arch-neutral).** `append <path> <text>` opens, seeks to EOF, and
grows via `write_grow` (allocate + zero-fill + chain, directory `size` published LAST), creating the
file if absent (like `>>`). `rm <path>` (alias `del`) is `delete_located` — mark the dir slot `0xE5`
FIRST, then free the chain (all FAT copies), the crash-safe order `fat.rs` guarantees; directory →
`-EISDIR` (removal out of scope this arc), absent → `-ENOENT`.

**M3 — safety rails + `sync` (`2531209`).** Three properties, made explicit:
- **Bounded.** `block::write_block` rides the SAME JD3 wall-clock BOT pump as reads (`write_block` →
  `storage_write10` → `scsi_write10` → `bot_transfer(Out)` → `run_bot_stage` → `pump_until_bot_done`,
  a `now_cycles`/`hw_wait_budget×3` deadline). A stalled USB write times out to `BlockError::Io` →
  `FatError::Io` → an honest console error; it never WFI-parks the timerless EL1 core. This is the
  load-bearing check for the Orin's storage-is-a-USB-reader-that-stalls reality (JD3/JD4 benches) —
  verified in the driver, not assumed.
- **Consistent.** Each `fat.rs` step is atomic under F3's `FAT_MUTATION`/`DIR_MUTATION` locks, so a
  mid-sequence failure leaves the volume consistent: a failed grow keeps the OLD (smaller) size (size
  published last); a failed `rm`/truncate leaves lost clusters (benign, chkdsk-reclaimable), never an
  aliasing or torn volume.
- **Write-through.** `write_block` is synchronous (BOT WRITE(10) / polled SD CMD24 complete before the
  command returns — no write-back cache), so every command is already durable when it returns; `sync`
  is the honest no-op confirmation.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches;
`UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (shared `shell.rs` guard); `./arroyo
test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → `CAPSTONE COMPLETE` 6/6;
`esp-jetson kernel.elf` links, **108 `tegra:` strings** (unchanged from JD4 — the shell-write strings
carry no `tegra:` token; validate media by count, not size). The write PRIMITIVES themselves already
run headless — the `el0-u10create`/`el0-u10delete`/`el0-u11close` fixtures exercise the identical
`create_in_root`/`write_grow`/`delete_located` on a FAT image; JD5's shell arms are thin glue over
them, invoked only on interactive input (the boot/test path is unperturbed). A headless demo of the
SHELL write path is not cleanly reachable in-lane (the shell dispatches only on a keystroke, and tegra
never runs in QEMU), so the shell-level verdict is attended-pending, exactly as JD2/JD3/JD4.

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-10, attended bench; Peter at the Orin; serial
`~/unaos-bench/jetson-serial-2026-07-10-165211.log`). Verdict: PASS** — the whole write battery on
real silicon, across two boots, on the FAT16 `UNAOSRW` card (29 MiB / 60800 blocks, in the Alcor
`USB SD Reader` behind the hub, slot 5; enumerated cleanly on BOTH boots — no `vid=0000` intermittency
this bench):

- **The money shot — write-through durability across a power cycle.** Boot 1: `write hello.txt hello
  from orin` (create), `cat hello.txt` (read back), `append hello.txt and from peter` (grow),
  `cat hello.txt` (both). **Power-cycle.** Boot 2: `cat hello.txt` → the content **survived** — the
  first file written on the Orin panel to persist across a reboot. Write-through durability proven on
  the real xHCI USB write path.
- **Delete path:** `rm hello.txt` → freed; `cat hello.txt` → `-ENOENT`.
- **Root-only guard:** `write docs/x.txt hi` and `write docs/y.txt …` were both REFUSED with
  `-ENOTSUP`; a follow-up `cat docs/x.txt` confirmed the file was never created, and `ls docs` still
  lists the subdirectory — JD4's read-side navigation is untouched by the root-only write scope.
- **Case-insensitive 8.3 on silicon:** every name was typed lowercase (`hello.txt` created/matched
  `HELLO.TXT`), exactly as JD4.
- **Health:** both boots reached CAPSTONE 6/6 + screen-on-boot; **zero `BOT pump TIMEOUT`, zero BOT
  transfer errors, zero panics/aborts** — the bounded write pump and F3's locks held on real hardware.

⚠ Bench logistics: no dedicated jetson boot stick was on hand, so the rMBP `UNAOS` card was repurposed
as the Orin boot stick (its x86 ESP overwritten with the aarch64 JD5 ESP) — the rMBP track must
re-flash its own boot media before its next bench. The FAT16 `UNAOSRW` (the pi4 fixture card) served
as the tegra data card, as in JD4.

### JD6 — the write path reaches the whole tree: subdirectory writes (✅ METAL-CONFIRMED 2026-07-11)

JD4 made the whole tree *navigable* (read); JD5 made the *root* writable; JD6 closes the gap:
`touch` / `write` / `append` / `rm` in ANY directory the shell can `cd` into. The panel becomes a
workstation you can *organize*, not just a flat scratchpad. The read side already resolved subdir
paths (`resolve_path`); JD6 gives the write side the same reach.

**The seam — a seat-granted narrow additive `fat.rs` exception.** JD5 was root-only *because*
`fat.rs`'s public mutation API (`create_in_root` / `find_located`) hard-codes the root directory, and
`fat.rs` mutation is the pi4-K1 lane (call-never-edit for this track). JD6 needs a dir-aware entry
point, so — per the round-6 seat coordination (ccd, GATE-0) — the seat granted a narrow ADDITIVE
exception, the same shape as JD4's `read_dir` grant: **two new public wrappers, zero edits to any
existing function, placed adjacent to their root twins:**
- `locate_in_dir(first_cluster, name)` — `first_cluster == 0` ⇒ `find_located` (root), else the
  existing private `locate_in_dir_chain` (a read-only bounded directory walk, no lock).
- `create_in_dir(first_cluster, name, attr)` — `0` ⇒ `create_in_root`, else the existing private
  `free_slot_in_dir_chain` + a **verbatim** copy of `create_in_root`'s `with_dir_lock` slot-write
  RMW (both sites cross-referenced "twin — keep in sync"). It rides `DIR_MUTATION`/`FAT_MUTATION`
  exactly as the root twin: the free-slot SCAN stays outside the lock, only the sector RMW inside;
  it allocates no clusters and touches no FAT. Nothing else in the write path needed a `fat.rs`
  change — `delete_located`/`write_grow` already take `(dir_lba, dir_off, first_cluster)` and were
  parent-agnostic.

**Principal — unchanged.** Subdirectories don't change the principal: the shell is still EL1 ASID 0,
the PUBLIC principal, and subdir creates are plain public FAT files (the U6 owner ACL still lives
only on the out-of-lane SVC `sys_open` path; the trusted local console does not consult it — §JD5).

**Scope — the whole tree, with honest edges.** `resolve_write_target` normalizes the path against
the cwd (`.`/`..` collapse lexically), then walks to the PARENT directory via the read-only
`resolve_path` and returns `(parent_first_cluster, leaf, parent_canon)` (root ⇒ cluster 0). The
error map is honest and non-hanging:
- parent is a plain file → `-ENOTDIR`; a missing parent → `-ENOENT` (both surface from `resolve_path`);
- the root itself as a target → `-EISDIR`;
- a FULL directory (no free slot) → `-ENOSPC` — **extending a subdirectory's cluster chain is out of
  scope this arc** (the twins add a slot but never grow the directory chain), exactly as the root has
  always been;
- a directory target for `rm` → `-EISDIR`: **directory removal (`rmdir`) stays out of scope** (it needs
  emptiness + `.`/`..` handling and a `fat.rs` primitive this track does not have).

**M1 — subdir write plumbing + `touch` (`446b986`).** The two `fat.rs` wrappers + `resolve_write_target`
+ `fs_touch` rewired through `locate_in_dir`/`create_in_dir`. `touch DOCS/NOTE.TXT` creates in a
subdirectory; success echoes the canonical absolute path (`/DOCS/NOTE.TXT`).

**M2 — `write` (create-or-truncate) in subdirs (`a3bc06a`).** `fs_write` routes through the dir-aware
twins; create-or-truncate semantics unchanged (truncate = delete chain + fresh 0-length entry + grow).
The raw `write <lba> <byte>` block command is untouched (dispatched separately, before `fs_write`).

**M3 — `rm` + `append` in subdirs; retire `resolve_root_name` (`2e9ca1b`).** `fs_append`/`fs_rm` routed
the same way, completing the set. With the last consumers converted, the JD5 root-only resolver
`resolve_root_name` is removed; the module DESIGN NOTE's scope paragraph is updated to the whole-tree
reach and the honest error map.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches;
`./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → `CAPSTONE COMPLETE`
6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (shared `shell.rs` guard); `esp-jetson
kernel.elf` links, **108 `tegra:` strings** (unchanged — the subdir-write strings carry no `tegra:`
token; validate media by count, not size). As in JD2–JD5 the SHELL write path is not headless-reachable
in-lane (the shell dispatches only on a keystroke, and tegra never runs in QEMU), so the shell-level
verdict is **attended-pending**; the write PRIMITIVES the twins call are the same F3-locked
`create_in_*`/`write_grow`/`delete_located` the U9/U10/U11 fixtures already exercise headless.

**Metal verdict — ✅ CONFIRMED (attended bench, 2026-07-11 — PASS, panel-observed).** The subdir
money-shot ran on the round-6 bench, on the JB1f-fixed kernel (`e074518` tip; the JD6 code itself
was blocked from benching by the §JB1f crash until that fix landed): the bench card
[`jd6-bench.md`](../../../../unaos/scripts/jd6-bench.md) ran to completion on the FAT16 `UNAOSRW`
card's pre-existing `DOCS/` subdirectory, including the write → power-cycle → `cat` durability leg.
Attending-operator verdict: **pass 100%**. ⚠ Same evidence caveat as the §JB1f verdict: the
host-side serial capture failed mid-bench, so the verdict is the attended panel observation (the
JD6 card's checks are all panel-visible); no replay log exists for an mbench assert.

### FATDIRS — the `fat.rs` directory-mutation seam: `create_dir` / `remove_dir` (pi4-lane, `cdfe25b`)

JD6 left `mkdir`/`rmdir` explicitly out of scope — its subdir writes reused the existing
`create_in_dir`/`locate_in_dir`/`free_slot_in_dir_chain` walkers, but creating or removing a *directory*
needs new `fat.rs` logic (allocate + initialize a directory cluster with `.`/`..`; verify emptiness
before removal). Because that logic is arch-shared and lock-sensitive, it landed as a **dedicated
pi4-lane arc** (FATDIRS, 2026-07-12) rather than inside a jetson write arc; **JD7's panel
`mkdir`/`rmdir` consumes it call-never-edit**, exactly as JD6's write path rides `create_in_dir`.

**The seam (two public methods + one private helper, ZERO edits to any existing `fat.rs` fn — the JD6
additive-exception pattern):**
- `pub fn create_dir(parent_first_cluster: u32, name: &str) -> Result<(DirEntry, u64, usize), FatError>`
  — `alloc_cluster` (compare-and-claim under `FAT_MUTATION`) a child cluster, `init_subdir_cluster`
  writes `.` (self) and `..` (parent; `0` when the parent is root) into it, then `create_in_dir` links a
  0-cluster DIR-attr (`0x10`) entry in the parent and `write_dir_entry_fields` publishes the child
  cluster (the last write). Returns the parent's entry + its on-disk `(lba, off)` — the `create_in_dir`
  shape, so JD7 can hang a K-lineage ACL row on it. `parent_first_cluster == 0` ⇒ the root, dispatching
  through the same JD6 twins, so it works on the FAT16 fixed-root and FAT32 images alike.
- `pub fn remove_dir(parent_first_cluster: u32, name: &str) -> Result<Vec<u32>, FatError>` — locate,
  refuse a non-directory target and a `first_cluster == 0` root-like entry, verify the target holds
  ONLY `.`/`..` (the `read_dir` walk), then `delete_located` (mark `0xE5` first, then `free_chain`).
  Returns the freed clusters.

**Crash order (fail-safe):** the child is fully initialized before the parent link; a crash leaks a
cluster or leaves a `FstClus==0` entry (the known JD6-ledgered corner), never a live entry over a
cluster that gets freed/aliased. `remove_dir` inherits `delete_located`'s `0xE5`-then-free order.

**Locking:** every sector RMW is SMP-atomic via the existing `FAT_MUTATION`/`DIR_MUTATION` locks, held
only over single-sector RMWs (never widened across a scan or `free_chain` — the F3 span rule). Sound
WITHOUT the syscall `NAMESPACE` lock (the EL1 panel reaches `fat.rs` directly). One honest residual —
`remove_dir`'s emptiness-scan → delete is not atomic vs a concurrent `create_in_dir` into the same
target — is EXCLUDED_BY_SEQUENCING today (no concurrent EL1 FS mutators) and ledgered in
[`SECURITY.md`](../../SECURITY.md) with F3's. **Errno fidelity:** reuses existing `FatError` variants
(a new one would break shell.rs's exhaustive `fat_errno` match) — `-ENOTDIR`/root map to `Unsupported`,
`-ENOTEMPTY` to `IsDirectory`; enriching `FatError` is a JD7-side (jetson-lane) follow-up.

**Tested (QEMU):** `check` both arches + `UNAOS_TEGRA=1 check` green; `kernel8-test 30` = 23 PASS
byte-identical + CAPSTONE 6/6 + all prior witnesses + an uncounted `:: FATDIRS: … PASS [w=0xff] ::`
(the `k1_atr` disk-selftest idiom, fully self-cleaning), zero FAIL; `test-arm 22` MISSION. Zero x86
behavioural change. **Metal:** the attended money-shot rides JD7's Orin panel `mkdir`/`rmdir` bench.

### JD7 — shaping the tree: panel `mkdir` / `rmdir` (✅ METAL-CONFIRMED 2026-07-12)

JD4 made the tree *navigable*, JD5/JD6 made it *writable*; JD7 lets you *shape* it — `mkdir DOCS/DRAFTS`,
`rmdir DOCS/OLD` from the panel. Together they close the loop: a directory tree you can organize end to
end from the console. JD7 is **thin panel glue** — it adds NO `fat.rs` logic. The directory-mutation
seam already landed as the pi4-lane FATDIRS arc (§FATDIRS above); JD7 *consumes* it call-never-edit,
exactly as JD6's write path rides `create_in_dir`. The whole arc is `shell.rs`-only (two new command
handlers `fs_mkdir`/`fs_rmdir`, the `mkdir`/`rmdir` dispatch arms with DOS `md`/`rd` aliases, and the
help/scope-comment refresh).

**`mkdir` — walk, de-dup, create.** `fs_mkdir` reuses JD6's `resolve_write_target` to get
`(parent_first_cluster, leaf, parent_canon)`, then — because `fat::create_dir` inherits `create_in_dir`'s
**no-de-dup** contract — `locate_in_dir`s the leaf FIRST: a name already taken (file *or* directory) is an
honest `-EEXIST`, never a duplicate slot. Absent ⇒ `fat::create_dir(parent, leaf)` allocates + `.`/`..`-
initializes the child cluster and links the parent DIR entry; success echoes `created directory
/DOCS/DRAFTS` (the canonical on-disk spelling).

**`rmdir` — walk, pre-check, remove-if-empty.** `fs_rmdir` refuses the root LOCALLY first (`-EBUSY` — the
volume root is unnameable and cluster 0 is not freeable; this also catches `rmdir .` at root and `rmdir ..`
that pops to it), then walks to the parent and `locate_in_dir`s the target. A FILE target is rejected with
`-ENOTDIR` from the shell's own pre-check (so the seam's `Unsupported`-for-non-dir never has to surface);
otherwise `fat::remove_dir(parent, leaf)` verifies emptiness (only `.`/`..`) and frees the one cluster. The
seam maps a NON-EMPTY directory to `IsDirectory`, which the shell renders as `-ENOTEMPTY`. `rm` stays
file-only (a directory is still `-EISDIR`).

**Errno fidelity is shell-side** (the FATDIRS seam reuses existing `FatError` variants — a new variant
would break `shell.rs`'s exhaustive `fat_errno` match, kernel-core). The shell resolves file-vs-dir-vs-root
from the parent walk BEFORE calling, so it emits the POSIX-shaped tags itself:

| condition | who decides | tag |
|---|---|---|
| name already taken | shell `locate_in_dir` before `create_dir` | `-EEXIST` |
| parent missing | `resolve_write_target` (`resolve_path`) | `-ENOENT` |
| parent is a plain file | `resolve_write_target` | `-ENOTDIR` |
| volume / parent-dir full | `create_dir` → `NoSpace` | `-ENOSPC` |
| non-8.3 name | `create_dir` → `Unsupported` | `-EINVAL` |
| `rmdir` a FILE | shell `is_dir` pre-check | `-ENOTDIR` |
| `rmdir` a NON-EMPTY dir | `remove_dir` → `IsDirectory` | `-ENOTEMPTY` |
| `rmdir` the root | shell path pre-check | `-EBUSY` |
| absent name | shell `locate_in_dir` → `NotFound` | `-ENOENT` |

**Principal — unchanged.** The shell is still EL1 ASID 0, the PUBLIC principal; directories don't change
that. The FATDIRS block's locking is sound for these EL1 callers *without* the syscall `NAMESPACE` lock
(they reach `fat.rs` directly). JD7 adds one caller-side note to the ledger: now that the EL1 panel can
`create_dir` (via `mkdir`) as well as remove, the honest residual is symmetric — an EL1-panel-vs-EL0
create/create into the SAME directory is the same EXCLUDED_BY_SEQUENCING class as FATDIRS's ledgered
`remove_dir` TOCTOU (no concurrent EL1 FS mutators run today; the fix is the same future `fat.rs` namespace
lock). Recorded in [`SECURITY.md`](../../SECURITY.md).

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test-arm 22`
→ `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test
25` → `MISSION SUCCESS` (shared `shell.rs` guard); `esp-jetson kernel.elf` links, **108 `tegra:` strings**
(unchanged — the `mkdir`/`rmdir` strings carry no `tegra:` token; validate media by count, not size). As in
JD2–JD6, the SHELL command path is not headless-reachable in-lane (the shell dispatches only on a keystroke,
and tegra never runs in QEMU), so the shell-level verdict is **attended-pending**; the directory PRIMITIVES
that `fs_mkdir`/`fs_rmdir` call — the FATDIRS `create_dir`/`remove_dir` — are already exercised headless by
their own `fatdirs_check` selftest.

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-12 attended bench; Peter at the Orin; serial
`~/unaos-bench/jetson-serial-2026-07-12-180110.log`).** The directory tree created on the panel
(`/DOCS/DRAFTS` + `NOTE.TXT`) SURVIVED a power cycle — `cd` back in and `cat` returned the content
byte-intact; an empty `rmdir` freed its cluster; the `-ENOTEMPTY` (non-empty) and `-EBUSY` (root) probes
tagged honestly; zero BOT-timeout/fault/heal on serial. This also flips **FATDIRS**'s `create_dir`/`remove_dir`
first-silicon verdict — they ran end-to-end on real hardware here for the first time. The money-shot is the bench card
[`jd7-bench.md`](../../../../unaos/scripts/jd7-bench.md): `mkdir DOCS/DRAFTS`, `cd`, `write NOTE.TXT …`,
power-cycle, `cd DOCS/DRAFTS`, `cat` (the created tree survives), then `rm NOTE.TXT`, `cd ..`, `rmdir DRAFTS`.
⚠ Verify the serial bridge captures a full boot BEFORE burning bench time — the round-6 host capture failed
mid-bench (see §JB1f). Repeated `mkdir`/`rmdir` runs accumulate `0xE5` tombstone slots in the parent across
boots (FATDIRS cleanup leaves them; harmless — don't misread as corruption; a card re-prep clears them).

### JD8 — copying files: panel `cp` (✅ METAL-CONFIRMED 2026-07-12)

JD4 navigates, JD5/JD6 write, JD7 shapes; JD8 lets you *duplicate* — `cp README.TXT DOCS/`,
`cp DOCS/A.TXT B.TXT` from the panel. Together they close the file-manager verb set: create, edit,
delete, organize, and now **copy**. (`mv` — the last verb — waits on a future pi4-lane FATMOVE seam
[an "unlink-entry-keep-chain" `fat.rs` op with no existing public twin]; the round-9 seat banked it and
picked `cp`, which is fully in-lane.) Like JD7, JD8 is **`shell.rs`-only and adds NO `fat.rs` logic**: it
composes primitives that already exist — the offset-aware read `read_at` and the JD6 create-or-truncate
write path (`create_in_dir` + `write_grow`) — all **call-never-edit**. One new command handler `fs_cp`
plus the `cp`/`copy` dispatch arm.

**Source, destination, and the `cp FILE DIR/` idiom.** `fs_cp` resolves `src` via the read-only
`resolve_path` and requires a FILE — a plain-`cp` directory source is `-EISDIR` (recursive `cp -r` is
JD9, §JD9 below). The destination is decided by resolving `dst`: an existing DIRECTORY (or the root) receives
the copy under the source's canonical leaf name (`cp A.TXT DOCS` → `/DOCS/A.TXT`); anything else — an
existing file or a not-yet-existing name — is the destination file itself, validated through JD6's
`resolve_write_target` (so a missing dst parent is `-ENOENT`, a dst parent that is a plain file is
`-ENOTDIR`). An existing destination file is truncated in place (delete chain + fresh 0-length entry),
exactly the JD6 `write` create-or-truncate prologue. Copying a file onto itself is refused with `-EINVAL`
— canonical paths are unique per file, so a case-insensitive full-path compare is complete (FAT 8.3 names
are case-insensitive, so `cp A.TXT a.txt` in one directory is honestly the same file).

**Size handling — the M2 decision: streaming, no ceiling.** The copy STREAMS the source in fixed 32 KiB
windows: `read_at(src_fc, src_size, off, buf, 32K)` fills a window, `write_grow` appends it at `off` to
the growing destination, and `(first_cluster, size, off)` advance across windows (the fresh entry begins
at cluster 0 / size 0, and `write_grow` allocates + publishes as it grows). A file of ANY size therefore
copies with a bounded (window-sized) heap footprint and **no truncation** — deliberately no size cap,
unlike `cat`'s 8 KiB display bound. `read_at` is existing public `fat.rs` API (the U9/read-path
offset-aware twin of `read_file`), so streaming reached for no new primitive. The trade-off logged for a
JD9 follow-up: the per-window `write_grow` re-walks the destination cluster chain from its head, so a very
large copy is O(windows²) FAT reads — bounded, and every access rides the JD3 wall-clock BOT pump (a
stalled transfer is `-EIO`, never a hang on the timerless EL1 core), but a future single-pass copy
primitive could tighten it. An empty source copies as 0 windows → a fresh 0-length destination.

**Errno fidelity is shell-side** (the JD6/JD7 pattern, reusing `fat_errno` + shell-owned tags): src
missing → `-ENOENT`; src is a directory → `-EISDIR`; dst parent missing → `-ENOENT`; dst parent is a
file → `-ENOTDIR`; the volume or directory full → `-ENOSPC`; a non-8.3 destination name → `-EINVAL`;
copy-onto-self → `-EINVAL`. On success it echoes `copied /DOCS/A.TXT -> /DOCS/B.TXT (N bytes)` in the
canonical on-disk spelling.

**Principal — unchanged.** The shell is still EL1 ASID 0, the PUBLIC principal; `cp` reads a public
source and creates a public destination — no U6 owner ACL is consulted, correct for the trusted local
console (§JD5). JD8 adds no new lock or namespace surface: it composes the same F3-locked
`create_in_dir`/`write_grow`/`delete_located`/`read_at` primitives JD6/JD7 already exercise and ledgered,
so it inherits their locking analysis unchanged.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test-arm 22`
→ `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test
25` → `MISSION SUCCESS` (shared `shell.rs` guard); `esp-jetson kernel.elf` links, **108 `tegra:` strings**
(unchanged — the `cp` strings carry no `tegra:` token; validate media by count, not size). As in JD2–JD7
the SHELL command path is not headless-reachable in-lane (the shell dispatches only on a keystroke, and
tegra never runs in QEMU), so the shell-level verdict is **attended-pending**; the read/write PRIMITIVES
`fs_cp` calls are already exercised headless by the U9/read-path and the U10/U11 write fixtures.

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-12 attended bench; Peter at the Orin; same serial log as
§JD7).** A file copied on the panel (`README.TXT` → `/COPIES/README.TXT` via the `DIR/` idiom, plus an
explicit-name `/COPIES/BACKUP.TXT`) SURVIVED a power cycle — both copies `cat`'d intact after re-boot with
the source left byte-for-byte untouched; zero BOT-timeout/fault on serial. The money-shot is the bench card
[`jd8-bench.md`](../../../../unaos/scripts/jd8-bench.md): `cp` a file into a subdirectory, `cat` the copy,
power-cycle, `cat` the copy again (it survives) with the source left untouched, then the error probes
(dir source, self-copy, missing parent, file-parent). ⚠ Verify the serial bridge captures a full boot
BEFORE burning bench time — the round-6/8 host capture failed mid-bench (see §JB1f).

### JD9 — copying trees: panel `cp -r` (✅ METAL-CONFIRMED 2026-07-12)

JD8 duplicates a file; JD9 duplicates a whole subtree — `cp -r DOCS BACKUP` from the panel. It closes the
copy half of the file-manager verb set (navigate, write, shape, copy; only `mv` remains, gated on a future
pi4-lane FATMOVE seam). Like JD7/JD8, JD9 is **`shell.rs`-only and adds NO `fat.rs` logic**: it composes
primitives that all already exist — `read_dir` (JD4) walks the source, the FATDIRS `create_dir` seam (the
JD7 `mkdir` idiom) rebuilds the destination tree, and the JD8 per-file streaming copy (refactored into a
shared `copy_file_into` helper) copies each file — all **call-never-edit**. The `cp`/`copy` dispatch arm
gained a `-r`/`-R` flag; `fs_cp_recursive` is the new handler, `cp_tree` the recursion.

**Source, destination, and the `cp DIR DEST` idiom.** `fs_cp_recursive` resolves `src`:
- a **ROOT** source (`cp -r /`) is refused `-EINVAL` — the volume root has no leaf name to copy *as*, and
  every in-volume destination is a descendant of the root anyway (the self/descendant guard would refuse it);
- a **FILE** source degrades to a plain file copy (`fs_cp`) — POSIX-friendly and honest (`cp -r FILE DST` ==
  `cp FILE DST`);
- a **DIRECTORY** source is the recursive case. The destination follows the same idiom as JD8's file copy:
  if `dst` resolves to an existing **directory** (or the root) the tree lands AS `dst/<src-leaf>`
  (`cp -r DOCS BACKUP` where `BACKUP/` exists → `/BACKUP/DOCS`); a not-yet-existing `dst` **becomes** the new
  tree (`cp -r DOCS NEWNAME` → `/NEWNAME`); an existing **file** at `dst` is `-ENOTDIR`.

**The four guards (M1/M2).**
1. **Self / descendant.** Copying a directory into itself or one of its own descendants is refused `-EINVAL`
   via a case-insensitive canonical-path prefix compare (`is_descendant`: `path` is inside `ancestor` iff it
   is strictly longer, shares the prefix, and the next byte is `/` — so `/DOCSX` is *not* inside `/DOCS`).
   This is what stops an infinite `cp -r DOCS DOCS/SUB` (the destination would be created *inside* the source
   being walked).
2. **Fresh-tree `-EEXIST`.** The top-level target directory must NOT already exist; if it does, refuse
   `-EEXIST`. This is the **simple, safe rule** chosen for the merge-into-vs-refuse decision: `cp -r` always
   creates a brand-new tree, never silently merging into or overwriting an existing one (FAT + our
   truncate-overwrite file copy make a silent merge surprising). A useful consequence: because the top-level
   target is fresh, *every* directory `cp_tree` creates below it sits inside a freshly-created, therefore
   empty, parent — so no child name can ever collide (`create_dir`/`create_in_dir` do not de-dup, but they
   never have to here) and no pre-existing file is ever clobbered. The recursion needs no per-node existence
   check.
3. **Depth bound.** Recursion is capped at `CP_MAX_DEPTH = 32`; exceeding it is an honest `-ELOOP`, never a
   stack blow-out. (`read_dir`'s own chain-loop guard already backstops a malformed self-referential volume;
   the depth cap is the shell-side belt-and-braces.)
4. **Honest partial failure.** A mid-tree error (e.g. `-ENOSPC` part-way through) stops immediately and
   reports the running tally — dirs/files/bytes copied *before* the error — plus the failing path and errno.
   No silent truncation. Nothing is rolled back: the partial tree is left on disk (crash-safe per the
   FATDIRS/JD6 `0xE5`-then-free and child-before-parent ordering), and the operator can `rmdir`/`rm` it. Every
   op rides the JD3 wall-clock BOT pump, so a stalled transfer is `-EIO`, never a hang on the timerless EL1
   core.

`cp_tree` filters the `.`/`..` self/parent links a subdirectory cluster carries (the root has none) at every
level, `create_dir`s each child directory and recurses, and streams each child file through `copy_file_into`
(the JD8 streaming core, now shared by both `fs_cp` and `cp_tree` so the create-or-truncate + windowed
`read_at`→`write_grow` logic lives in exactly one place). On success it echoes
`copied /DOCS/ -> /BACKUP/DOCS/ (N dir(s), M file(s), K bytes)`.

**Errno fidelity is shell-side** (the JD6/JD7/JD8 pattern, reusing `fat_errno` + shell-owned tags): src
missing → `-ENOENT`; ROOT src → `-EINVAL`; dst is a plain file → `-ENOTDIR`; self / descendant → `-EINVAL`;
top-level target already exists → `-EEXIST`; recursion too deep → `-ELOOP`; the volume/dir full mid-tree →
`-ENOSPC` (partial-reported); a non-8.3 child name in the source (should not occur — `read_dir` only surfaces
representable names) → `-EINVAL` from `create_dir`.

**Principal — unchanged.** The shell is still EL1 ASID 0, the PUBLIC principal; `cp -r` reads public sources
and creates public destinations — no U6 owner ACL is consulted, correct for the trusted local console (§JD5).
JD9 adds no new lock or namespace surface: it composes the same F3-locked
`read_dir`/`create_dir`/`create_in_dir`/`write_grow`/`delete_located`/`read_at` primitives JD6/JD7/JD8
already exercise and ledgered, so it inherits their locking analysis unchanged (including the FATDIRS
`create_dir`-into-the-same-directory TOCTOU, EXCLUDED_BY_SEQUENCING today — no concurrent EL1 FS mutators).

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test-arm 22`
→ `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test
25` → `MISSION SUCCESS` (shared `shell.rs` guard); `esp-jetson kernel.elf` links, **108 `tegra:` strings**
(unchanged — the `cp -r` strings carry no `tegra:` token; validate media by count, not size). As in JD2–JD8
the SHELL command path is not headless-reachable in-lane (the shell dispatches only on a keystroke, and tegra
never runs in QEMU), so the shell-level verdict is **attended-pending**; the read/write/dir PRIMITIVES `cp -r`
composes are already exercised headless by the U9 read path, the U10/U11 write fixtures, and FATDIRS's own
`fatdirs_check` selftest.

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-12 attended bench; Peter at the Orin; same serial log as
§JD7).** A directory tree copied recursively on the panel (`SRC` → `DST` and into-dir `SRC` → `/BACKUP/SRC`,
each `(2 dir(s), 2 file(s), 20 bytes)`) SURVIVED a power cycle — the deep file `DST/SUB/B.TXT` `cat`'d
`deep beta` after re-boot, source tree untouched; the guards fired honestly (self-into-descendant `-EINVAL`,
pre-existing target `-EEXIST`, volume-root `-EINVAL`). The money-shot is the bench card
[`jd9-bench.md`](../../../../unaos/scripts/jd9-bench.md): `mkdir` a small source tree with files, `cp -r` it
into a new destination, `ls`/`cat` to verify the tree structure and file contents match, power-cycle, verify
the copy survives, then the guards (self-into-descendant `-EINVAL`, missing source `-ENOENT`, pre-existing
target `-EEXIST`). ⚠ Verify the serial bridge captures a full boot BEFORE burning bench time — the round-6/8
host capture failed mid-bench (see §JB1f). Repeated `cp -r`/`rmdir` cycles accumulate `0xE5` tombstone slots
across boots (the FATDIRS cleanup leaves them; harmless — a card re-prep clears them).

### JD10 — moving & renaming: panel `mv` (✅ METAL-CONFIRMED 2026-07-12)

`mv` is the last classic file-manager verb, and JD10 closes the set: navigate (JD4), write (JD5/JD6),
shape (JD7), copy (JD8/JD9), **move/rename** (JD10). Unlike `cp -r`, a move is **O(1) by reference** — the
file's data never moves; only its directory entry is relinked. JD10 consumes the pi4-lane **FATMOVE** seam
(`rename_entry`/`move_entry`, landed and merged with the FATDIRS/JD7 split) **call-never-edit**: `shell.rs`
is the only file with logic, composing the seam with the JD6 path-resolution idioms
(`resolve_path`/`resolve_write_target`/`normalize_path`) and the JD9 `is_descendant` guard. The
`mv`/`move`/`ren`/`rename` dispatch arm routes to the new `fs_mv` handler.

**Two dispatches, decided by parent.** `fs_mv` resolves `src` to a concrete entry (a ROOT source is refused
`-EBUSY` — the volume root has no leaf to move *as*), takes its parent's first-cluster id via
`resolve_write_target`, and decides the destination with the same `mv SRC DIR/` idiom JD8/JD9 use: an
existing **directory** (or the root) receives the entry under the source's own leaf; anything else names the
destination directly (rename / move-with-new-name). Then it dispatches on whether source and destination
share a parent directory:
- **SAME parent → `rename_entry`** (rewrites the 8.3 name in the existing directory entry in place — a single
  dir-sector RMW). It works on **files AND directories**: an in-place rename leaves `first_cluster` untouched,
  so a renamed directory's own `.`/`..` and its children's `..` links stay correct. **`mv DIR NEWNAME` is
  O(1)** — one entry relink moves the whole subtree, so no `mv -r` is needed (the contrast with `cp -r`).
- **DIFFERENT parents → `move_entry`** (publishes the destination entry over the SAME `first_cluster`, then
  `0xE5`s the source name WITHOUT freeing the chain — the data moves by reference, no copy). **FILES only:**
  a directory across parents would need its `..` rewritten to the new parent (out of the seam's scope), so
  the seam returns `IsDirectory` → the shell surfaces `-EISDIR` with the honest remedy (rename it in place,
  or `cp -r` + `rm -r`).

**The guards (in order).**
1. **Self / descendant.** If the source is a **directory**, moving it onto itself or into its own subtree is
   refused `-EINVAL` via the JD9 `is_descendant` case-insensitive canonical-path prefix compare — the right
   message even though `move_entry` would independently refuse a cross-parent directory move. (`mv DOCS
   DOCS/SUB` is stopped here before any mutation.)
2. **No-clobber `-EEXIST`.** The destination must not already exist. Rather than POSIX's silent overwrite,
   the panel default is no-clobber (`-EEXIST`), mirroring the FATMOVE seam's own dest-exists refusal
   shell-side. The pre-check is **skipped only when the destination IS the source** (same parent + same
   canonical leaf, e.g. `mv FOO.TXT foo.txt`) — a rename to the source's own name, which `rename_entry`
   treats as a no-op success (its documented same-slot contract).

**Errno fidelity is shell-side** (the JD6–JD9 pattern, reusing `fat_errno` + shell-owned tags): src missing
→ `-ENOENT`; ROOT src → `-EBUSY`; dst parent missing → `-ENOENT`; dst parent is a file → `-ENOTDIR`; dst dir
full → `-ENOSPC`; a non-8.3 dst name → `-EINVAL`; a directory across parents → `-EISDIR`. On success the
shell echoes `renamed /OLD.TXT -> /NEW.TXT` (same parent) or `moved /A.TXT -> /DOCS/A.TXT` (across parents),
using the seam's returned canonical destination name.

**Principal + ACL — unchanged, ACL-neutral by construction.** The shell is still EL1 ASID 0, the PUBLIC
principal, and consults no U6/K-line `OWNED_FILES` ACL (§JD5). This matters more for `mv` than for the other
verbs: the pi4 `OWNED_FILES` ACL keys a private file by `(dir_lba, dir_off)`, and `move_entry` writes a NEW
slot — so an *EL0-owned* file moved from a user path would strand its owner row. But a **panel** `mv` runs as
the PUBLIC principal (no ACL row is ever consulted or created), so it is ACL-neutral. The owner-row re-key on
a moved owned file is a future K-line seam, ledgered in the pi4 FATMOVE `SECURITY.md` note; JD10 adds no EL0
ACL plumbing. **Crash safety is the seam's job:** `move_entry` publishes the destination BEFORE `0xE5`ing the
source, so a power-cut mid-move leaves a benign duplicate (two names, one chain), never a lost chain. JD10
adds no new lock or namespace surface — it composes the F3-locked `rename_entry`/`move_entry`/`locate_in_dir`
/`resolve_path` primitives and inherits their locking analysis unchanged.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test-arm 22`
→ `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test
25` → `MISSION SUCCESS` (shared `shell.rs` guard); `esp-jetson kernel.elf` links, **108 `tegra:` strings**
(unchanged — the `mv` strings carry no `tegra:` token; validate media by count, not size). As in JD2–JD9 the
SHELL command path is not headless-reachable in-lane (the shell dispatches only on a keystroke, and tegra
never runs in QEMU), so the shell-level verdict is **attended-pending**; the FATMOVE primitives `mv` composes
are already gated headless on the pi4 side (`kernel8-test`'s `FATMOVE` witness).

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-12 attended bench; Peter at the Orin; same serial log as
§JD7).** A file moved on the panel (`A.TXT` →rename→ `B.TXT` →move→ `/DOCS/B.TXT`) SURVIVED a power cycle —
`cat /DOCS/B.TXT` returned `hello alpha` intact after re-boot; an in-place **directory rename**
(`mv DOCS NOTES`) carried the whole subtree with one O(1) relink (no copy — `ls NOTES` showed the former
`DOCS` children); the guards fired honestly (dir self-into-descendant `-EINVAL`, cross-parent dir move
`-EISDIR`, missing source `-ENOENT`, no-clobber `-EEXIST`, root `-EBUSY`). **This also flips FATMOVE's own
metal verdict** — its `move_entry` crash-ordering (destination published before the source `0xE5`) ran on
real silicon for the first time, with zero BOT-timeout/fault on serial. The money-shot is the bench card
[`jd10-bench.md`](../../../../unaos/scripts/jd10-bench.md): rename a file in place (`mv A.TXT B.TXT`), move a
file into a directory (`mv B.TXT DOCS/`), power-cycle, and `cat` the moved file to prove it read intact
across the cycle (this also flips FATMOVE's own metal verdict — its `move_entry` crash-ordering runs on
silicon for the first time), then rename a directory in place (`mv DOCS NOTES`, O(1) subtree move) and the
guard/error probes (self-into-descendant `-EINVAL`, cross-parent dir move `-EISDIR`, missing source
`-ENOENT`, pre-existing target `-EEXIST`). ⚠ Verify the serial bridge captures a full boot BEFORE burning
bench time — the round-6/8 host capture failed mid-bench (see §JB1f).

### JD11 — mirroring shell command output to serial (bench-transcript infrastructure)

The round-9 Orin bench (§JB1f heal-tally, 2026-07-12) surfaced a bench-methodology gap: the panel console
has **no scrollback**, and shell command **output** (`ls`/`cat`/`pwd`/verb results) is drawn only to the
panel — only *keystrokes* were echoed to serial (`:: tegra: JD2 — KEY … ::`). So the durable bench record
was keystrokes + driver markers + heal tally, and verbatim command output could not be captured over the
serial bridge or replayed by mbench; card readout was the four-verb bench's bottleneck. JD11 closes the gap
by mirroring command-output lines to serial too, making every future Orin bench self-documenting — a
bench-infrastructure multiplier for the whole metal program, not just the panel.

**Where the output already converges.** Every shell command result reaches the panel through exactly one
sink: `Console::println` (shell.rs calls it for all output — `touch`/`write`/`ls`/`cat`/errno lines/etc.).
JD11 mirrors *there*, so no per-command plumbing is needed and the mirror is complete by construction. A
command that takes the whole screen (`gneiss`/vug) paints the framebuffer directly rather than via
`println`, so it is honestly **not** mirrored — the mirror carries text command output only, not graphics.

**Design — an inert, opt-in output sink (platform-neutral).** `Console` gains an
`out_sink: Option<fn(&str)>` field (`None` on `new()`). `println` pushes the line to the panel history as
before, then — *after* the history push, so a fault in the sink cannot lose the panel line — calls the sink
if one is installed. On every non-tegra surface (x86 GUI, pi `render_service`, headless) the sink is unset,
so `println` is byte-for-byte unchanged and no serial noise appears: **zero off-tegra behavioural change.**
The serial-line FORMAT lives in the tegra caller, not in the shared `console.rs`: the tegra
`jd2_console_pump` calls `console.set_output_sink(jd2_out_sink)` right after building the `Console` (before
the shell-entry banner, so those lines head the transcript), and `jd2_out_sink` — a `cfg(feature = "tegra")`
`fn(&str)` in `main.rs` — emits `:: tegra: JD2 — OUT | <line> ::`. Keeping the `tegra:` marker string in the
tegra-gated caller (not the shared crate) means it compiles into the tegra kernel **alone**; the shared
`console.rs` carries no `tegra:` literal.

**Marker format + why.** The output marker deliberately shares the `:: tegra: JD2 — …` family with the
keystroke marker (`… — KEY …`), so a single `awk '/:: tegra: JD2 —/' <log>` reconstructs the whole
interleaved session — keys typed *and* output produced, in order. `OUT | <line>` carries the verbatim
console line after the pipe. Ordering/locking: `jd2_out_sink` runs synchronously from `println` (called from
`shell::dispatch_command` → `handle_key`), *after* the per-keystroke `KEY` line has already printed and
released the UART; it only touches the serial UART (no re-entrancy into `Console`, no lock the caller holds),
so there is no new lock ordering and no deadlock — output lines simply follow their triggering `KEY` line.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test-arm 22`
→ `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test
25` → `MISSION SUCCESS` (shared `shell.rs`/`console.rs` guard); `esp-jetson kernel.elf` links
(`540,184 B`), **109 `tegra:` strings** — up 1 from the JD10 baseline of 108. The single new occurrence is
the `:: tegra: JD2 — OUT | {} ::` marker literal (`strings` splits it on the UTF-8 em-dash, so its
`:: tegra: JD2 ` prefix is the counted `tegra:` fragment); the shared `console.rs` adds none. This is the
first `tegra:`-count change since JD2 — validate media by count as before (109 tegra vs virt-clobber ≈ 0/1),
not size. Zero x86 behavioural change (the sink is `None` off-tegra). As in JD2–JD10 the shell command path
is not headless-reachable in-lane (dispatch is keystroke-driven; tegra never runs in QEMU), so the verdict
is **attended-pending**.

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-13 attended bench; Peter at the Orin; serial
`~/unaos-bench/jetson-serial-2026-07-13-133055.log`).** One card session, 4 clean boots / 4 power
cycles, 0 heals / 0 fatals / 0 BOT-timeouts, erratum-1941500 bit8=0 every boot. The whole session
is durable on serial — 397 `JD2 — KEY` + 77 `JD2 — OUT` lines; the round-9 "output vanishes, only
keys echo" gap is CLOSED and every verb is replayable from the capture. (Original expectation kept
below for the record.) The payoff is itself a metal artifact: at the next attended Orin
bench, every `ls`/`cat`/verb result appears on the serial log as `:: tegra: JD2 — OUT | … ::`, so the bench
produces a durable, mbench-able output transcript for the first time (the round-9 bench could not). Bench
card [`jd11-bench.md`](../../../../unaos/scripts/jd11-bench.md): run any output-producing command
(`help`, `ls`, `cat <file>`) and confirm the panel text is reproduced verbatim on the serial capture,
paired with its `KEY` lines. ⚠ Verify the serial bridge captures a full boot BEFORE burning bench time
(§JB1f) — with JD11 the bridge is now the *primary* evidence channel for output, so a mid-bench freeze
costs the transcript.

### JD12 — paging (`head`/`tail`) and wildcard globbing on the panel shell

The classic file-manager verb set closed at JD10 (`mv`) and JD11 made benches self-documenting; JD12 is the
polish pass over that set — two user-facing conveniences, both **`shell.rs`-only and call-never-edit** (they
add NO `fat.rs` logic, riding the existing public read/dir API), so the lane is exactly JD7–JD11's.

**`head <path> [n]` / `tail <path> [n]` — paging (M1).** Print the first / last `n` lines of a file
(default 10). `head` STREAMS from offset 0 through the offset-aware `read_at` in 4 KiB windows and stops the
moment it has seen `n` newlines — so `head 10` of a huge file reads only the first window(s), never the whole
file; a 64 KiB byte ceiling backstops a file with too few newlines so an unterminated giant line still bounds
the read and the heap. `tail` reads a bounded 64 KiB window ending at EOF, renders it, and prints the last
`n` lines; if that window began mid-file it probes the byte just before it to decide precisely whether the
first line is a cut partial (dropped) or a boundary-aligned complete line (kept), and notes the bound. A
directory or the root is `-EISDIR`; an empty file prints nothing. The three viewers `cat`/`head`/`tail` share
one `render_text` (LF splits, CR dropped, other non-printing bytes → `.`), and `cat`'s body was refactored
onto a `cat_render` helper (byte-identical output) that the wildcard `cat` reuses — so a file reads
identically however it is viewed, and mirrors identically into the JD11 serial transcript.

**Wildcard globbing — `*` / `?` (M2–M3).** A single TRAILING glob in a path's LAST component expands against
its parent directory via the read-only `read_dir` (the JD4 case-insensitive 8.3 match, now with `*` = any
run and `?` = one char, via a small iterative star-backtrack matcher — no recursion, no allocation). Matches
are `.`/`..`-filtered and sorted for a deterministic listing / serial transcript. The engine is invoked ONLY
inside the fs-verb arms — the shared arg-split at the top of `dispatch_command` is UNCHANGED, and the NET
command region (`netinfo`/`ping`/`arp`/`connect`/`udpsend`/`get` — a sockets-arc lane) never sees a glob. A
leaf with no metacharacter, or a glob confined to a non-trailing component, passes through literally
(byte-identical to pre-JD12; a mid-path wildcard resolves to an honest `-ENOENT`). The verbs it lifts:

- `ls *.EXT` lists the matches (one table line each + the file/dir tally); `cat *.EXT` cats each matching
  file in order (reusing `cat_render`; a directory match is the classic `-EISDIR`).
- `rm <path...>` takes multiple targets and expands wildcards (`rm *.TMP`); `cp [-r] <src...> <dst>` and
  `mv <src...> <dst>` take multiple sources with the LAST path the destination — with more than one source
  the destination MUST be an existing directory (`-ENOTDIR` otherwise, since several files can only land
  INTO a directory), and each source rides the existing `fs_cp`/`fs_cp_recursive`/`fs_mv` via the
  `SRC DIR/ → DIR/<leaf>` idiom. The single-source / no-wildcard case is a fast path, byte-identical to
  pre-JD12. Expansion is **SNAPSHOT-then-act**: the match list is captured before any mutation, so a
  `rm *.TXT` / `mv *.LOG ARCHIVE/` that deletes or moves as it goes never invalidates its own list; each
  concrete target is then re-resolved by its per-file helper (stale-proof, the JD4 cwd model). A wildcard
  that matches nothing is an honest per-pattern `no match` note.

**PRINCIPAL / ACL unchanged.** The shell is still EL1 ASID 0 = the PUBLIC principal; paging is read-only and
globbing only multiplies the same per-file operations, so JD12 adds no new `fat.rs` surface, no new lock, and
no new ACL interaction — it is a pure `shell.rs` composition of primitives JD4–JD11 already ledgered.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches (no new warnings);
`./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6;
`UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (the shared `shell.rs` guard); `esp-jetson
kernel.elf` links, **109 `tegra:` strings** — UNCHANGED from JD11 (the new verbs carry no `tegra:` token;
validate media by count, not size — the ELF grew to ~725 KB purely from the base's merged SOCK-3/UNAFS-K3
work, not from JD12). Zero x86 behavioural change (the shell command path is keystroke-driven and not
headless-reachable in-lane; tegra never runs in QEMU). A 1-lens adversarial review (refute-mode) found no
data-correctness bug — `glob_match`'s backtrack, `cat_render`/`ls_resolved` byte-identity, and the
snapshot-then-mutate safety all cleared; two low-severity truncation-note edges it raised were folded in
(`head` notes when lines remain, not merely when it stopped short of EOF; `tail` keeps a boundary-aligned
first line).

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-13 attended bench; same card session + serial log as
the JD11 confirm above).** `head`/`tail`: content-read, `-EISDIR`, `-ENOENT`, no-hang all correct on
silicon. Glob: `ls`/`cat`/`cp`/`mv`/`rm` all expand+sort correctly; the `-ENOTDIR` guard fires on
multi-source→non-dir; a glob-copied `DOCS/A.TXT` and glob-moved `ARCHIVE/C.LOG` survived a REAL
power cycle. Coverage note (not a defect): the panel's write/append verbs insert no newline, so
panel-authored files are single-line and last-N tail truncation was never distinguished from head on
metal — a true `tail N` witness needs a pre-seeded multi-line file (future bench prep item). ⚠ Bench
lesson (hazard, cross-track): macOS AppleDouble `._*` sidecars are glob-visible on FAT via 8.3 short
names (`_~8.TXT` matches `*.TXT`) — `dot_clean` the DATA card too, not just the boot stick.
(Original expectation kept below for the record.) As in JD2–JD11 the interactive path can only be exercised on
silicon. Bench card [`jd12-bench.md`](../../../../unaos/scripts/jd12-bench.md): page a file with
`head`/`tail`, then glob `ls *.TXT` / `cat *.TXT` / `cp *.TXT DOCS/` / `rm *.TMP` / `mv *.LOG ARCHIVE/`, and
confirm on the JD11 serial transcript that the right files are paged / listed / copied / removed / moved,
that a no-match pattern reports honestly, and that a multi-source copy onto a non-directory is `-ENOTDIR`.
This bench needs a card with several 8.3-named files sharing an extension (create them on the panel first).

### JD13 — recursive delete: panel `rm -r <dir>`

The classic file-manager verb set closed at JD10 (`mv`) with a create/copy/move quadrant, but `rm` stayed
file-only — a directory was `-EISDIR` (use `rmdir`), and `rmdir` removes only an EMPTY directory. JD13 closes
the **destructive** side: `rm -r DOCS` deletes a whole subtree in one command, and it multiplies with the JD12
glob (`rm -r OLD*/`). Like JD7–JD12 it is **`shell.rs`-only and adds NO `fat.rs` logic** — it composes
primitives that all already exist: `read_dir` (JD4) walks each directory, the `fs_rm` file-delete pair
(`locate_in_dir` + `delete_located`, JD6) unlinks each file, and the `rmdir` primitive (`remove_dir`, FATDIRS)
removes each emptied directory — all **call-never-edit**. The `rm`/`del` dispatch arm gained a `-r`/`-R` flag;
`fs_rm_recursive` is the new handler, `rm_tree` the recursion. It is the delete twin of JD9's `cp -r`, inverted:
where `cp_tree` creates top-down, `rm_tree` deletes bottom-up (a directory is emptied before it is removed).

**`-r` is required — the no-`-r` default is unchanged.** Without `-r`, `rm DIR` is still `-EISDIR` (the JD6
behaviour, byte-identical); a recursive delete is a footgun, so it must be asked for explicitly. Flags (leading
`-`) are filtered out of the path list exactly as `cp`/`mv` already do, so `-r`/`-R` may appear anywhere among
the args; a file literally named `-FOO` is reachable as `./-FOO` (the established convention).

**Source, and the two degrade/refuse cases.** `fs_rm_recursive` refuses the **ROOT** first, LOCALLY, before
any walk (`-EBUSY` — a recursive delete of the whole volume is a footgun, and the root is never a removable
directory: cluster 0 is not freeable). This mirrors `fs_rmdir`'s root refusal and also folds `rm -r .` at the
root and `rm -r ..` that pops to it into the same `-EBUSY` (both normalize to `/`). A **FILE** target degrades
to a plain `rm` (`rm -r FILE` == `rm FILE`, POSIX-friendly and honest). A **DIRECTORY** target is the recursive
case: `rm_tree` empties it depth-first, then the now-empty top directory itself is removed via `remove_dir`.

**The recursion (`rm_tree`) — bottom-up, snapshot-safe.** At each level `read_dir` SNAPSHOTS the directory's
entries before any deletion; `.`/`..` are filtered. A child **file** is unlinked by re-locating its slot by
name (`locate_in_dir` → `delete_located`) — run quiet (no per-file console line, so a whole tree yields ONE
summary like `cp -r`, not a flood). A child **directory** is recursed into FIRST (emptied), then `remove_dir`'d
(it now holds only `.`/`..`, so the seam's emptiness check passes). Deleting one entry never invalidates the
walk: a `0xE5` mark on one slot does not move another entry's slot, and each child is addressed by name — so
the SNAPSHOT-then-act property JD12's glob established is carried intact into the recursion (`rm -r *` and
`rm -r OLD*/` never invalidate their own match list). Recursion is depth-capped at `CP_MAX_DEPTH = 32` (the JD9
constant, reused) — an over-deep or malformed self-referential tree is an honest `-ELOOP`, never a stack
blow-out (`read_dir`'s own chain-loop guard is the first line of defence; the cap is belt-and-braces).

**Honest partial failure.** A mid-tree error (e.g. `-EIO` part-way through, or a `-ENOTEMPTY` if a concurrent
mutator raced a directory non-empty — excluded today, see below) stops immediately and reports the running
tally — dirs/files removed *before* the error — plus the failing path and errno, mirroring `fs_cp_recursive`.
Nothing is rolled back: the partial deletion is left on disk (crash-safe per the U10 `0xE5`-then-free ordering
— a name is unreachable before its chain is freed, so a power-cut mid-delete leaves at worst lost clusters,
never an aliased chain), and the operator can simply re-run `rm -r` to clear the remainder. Every op rides the
JD3 wall-clock BOT pump, so a stalled transfer is `-EIO`, never a hang on the timerless EL1 core. On success it
echoes `removed /DOCS/ (N dir(s), M file(s))` — the tally counts the top directory itself (as `cp -r` counts
its top dir).

**Note — no `is_descendant` guard.** `cp -r`/`mv` use the JD9 `is_descendant` prefix compare to refuse copying/
moving a directory into its own subtree (which would otherwise recurse forever, since the destination is created
*inside* the source being walked). `rm -r` has no destination and creates nothing, so that hazard does not
exist — termination is bounded by the finite snapshot at each level plus `CP_MAX_DEPTH`. Consistent with
`rmdir`, `rm -r` also does NOT special-case the current working directory: `rm -r` of the cwd (or an ancestor of
it) succeeds and leaves the JD4 cwd stale, and the very next cwd-relative command re-resolves it to an honest
`-ENOENT` — the documented JD4 stale-cwd model, not corruption.

**Errno fidelity is shell-side** (the JD6–JD12 pattern, reusing `fat_errno` + shell-owned tags):

| condition | who decides | tag |
|---|---|---|
| `rm -r /` (or `rm -r .`/`..` at the root) | shell path pre-check | `-EBUSY` |
| source missing | `resolve_path` | `-ENOENT` |
| `rm DIR` without `-r` | `fs_rm` (`is_dir`) | `-EISDIR` |
| a mid-tree read/free failure | `read_dir`/`delete_located`/`remove_dir` | `-EIO` (partial-reported) |
| recursion too deep / malformed | shell `CP_MAX_DEPTH` cap | `-ELOOP` (partial-reported) |

**Principal — unchanged.** The shell is still EL1 ASID 0, the PUBLIC principal; `rm -r` deletes public
entries and consults no U6/K-line `OWNED_FILES` ACL — correct for the trusted local console (§JD5). JD13 adds
no new lock or namespace surface: it composes the same F3-locked
`read_dir`/`locate_in_dir`/`delete_located`/`remove_dir` primitives JD6/JD7/JD9 already exercise and ledger,
so it inherits their locking analysis unchanged (including the FATDIRS
`remove_dir`-in-the-same-directory TOCTOU, EXCLUDED_BY_SEQUENCING today — no concurrent EL1 FS mutators run).

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches (no new warnings — only
the pre-existing `shutdown` double-`hlt_loop`); `./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1
./arroyo test-arm 40` → CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (the shared
`shell.rs` guard); `esp-jetson kernel.elf` links, **109 `tegra:` strings** — UNCHANGED from JD11/JD12 (the
`rm -r` strings carry no `tegra:` token; validate media by count, not size). Zero x86 behavioural change. As in
JD2–JD12 the SHELL command path is not headless-reachable in-lane (the shell dispatches only on a keystroke,
and tegra never runs in QEMU), so the shell-level verdict is **attended-pending**; the delete PRIMITIVES
`rm -r` composes are already exercised headless by the U10/U11 write/delete fixtures and FATDIRS's own
`fatdirs_check` selftest.

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-14 attended Orin bench, one card session with JD14; kernel =
`hw-jetson` tip `57ae4b2`, 109 `tegra:` strings; serial `~/unaos-bench/jetson-serial-2026-07-14-101517.log`,
1059 KEY / 149 OUT lines, 5 clean boots, 0 heals / 0 fatals / 0 BOT-timeouts, erratum-1941500 bit8=0 +
CAPSTONE all-6 every boot).** All bench-card sections passed: the tree built, `rm -r` removed it with an
honest summary; every guard fired (`rm DIR` no-`-r` → `-EISDIR`, `rm -r /` → `-EBUSY`, `rm -r NOSUCH` →
`-ENOENT`, `rm -r FILE` → plain delete); the glob form removed several trees; and the power-cycle capstone
held — a re-created same-named tree read back fresh content (`cat` returned the new bytes, not the deleted
tree's), proving the freed clusters were genuinely released and reused across a real power cut.

### JD14 — the `-f`/force + `-n`/no-clobber flag family for `cp`/`mv`/`rm`

The verb set closed at JD13; JD14 adds the POSIX-style flag family that completes the everyday ergonomics —
`cp -f`/`mv -f` to overwrite, `rm -f`/`rm -rf` to delete quietly, `-n` to make the no-clobber default
explicit. Like JD6–JD13 it is **`shell.rs`-only and adds NO `fat.rs` logic** — it composes the existing
public primitives (`locate_in_dir`/`delete_located`/`rename_entry`/`move_entry`, all call-never-edit). A new
`split_flags(argv)` helper parses **bundled short flags** (`-rf` == `-r -f`), which also fixes a latent
pre-JD14 gap: the old exact-token match (`*a == "-r"`) never recognized `rm -rf DIR`, so `rm -rf` silently
fell through to `-EISDIR`.

**No-clobber is now the panel DEFAULT for `cp` AND `mv`.** `mv` already refused a pre-existing destination
(`-EEXIST`); JD14 brings `cp` into line — an existing destination FILE is `-EEXIST` unless `-f`. This is a
deliberate divergence from POSIX `cp` (which overwrites silently): the panel favours safety and cp/mv
symmetry over strict POSIX. It is a behaviour change to plain `cp` onto an existing file (previously a silent
truncate-in-place overwrite, now `-EEXIST`) — no automated gate exercises the panel `cp` path (the shell is
keystroke-driven and tegra-only), so the change is gate-neutral and surfaces only at the attended bench.

**`-f`/force — opt into overwrite:**
- `cp -f SRC DST` overwrites an existing destination FILE via the JD8 truncate-in-place path
  (`copy_file_into` gains a `force` param; the `cp -r` recursion always writes into a freshly-created,
  empty tree, so it passes `force = true` and the guard never trips there).
- `mv -f SRC DST` overwrites an existing destination FILE by **delete-dst-first**: the existing file is
  removed (`delete_located`), then the entry is relinked (`rename_entry`/`move_entry`) into the freed slot.
- A **DIRECTORY** destination is never clobbered even with `-f` (overwriting a whole subtree would need a
  recursive delete) — `mv -f` onto a directory is refused `-EISDIR` ("remove it first (rm -r)"), and
  `cp -r`'s fresh-tree `-EEXIST` rule stands regardless of `-f` (a directory-tree merge/replace is out of
  scope — remove the target with `rm -r` first). This keeps `-f`'s destructive surface bounded to a single
  file, never a subtree.

**`-n`/no-clobber — reassert the default.** `-n` makes the (now default) no-clobber behaviour explicit and,
for safety, **overrides `-f`** if both are given (`force = has(-f) && !has(-n)`).

**`rm -f` / `rm -rf` — quiet on a missing target.** POSIX `rm -f NOSUCH` exits quietly; JD14 suppresses the
`-ENOENT` (a missing leaf, a missing parent component, and a no-match wildcard all go quiet under `-f`), so
`rm -rf OLD*` is the natural idiom that pairs with JD13's recursive delete. Two guards are NOT relaxed by
`-f`: `rm -rf /` is still refused `-EBUSY` (the whole-volume footgun), and a wrong-usage `-EISDIR` (a
directory without `-r`) is still reported — `-f` suppresses only the *missing-target* error, exactly as
POSIX `rm -f` still complains about a directory.

**Flag filtering.** Flags (a `-` followed by one or more ASCII letters) are filtered out of the positional
paths for all three verbs (`mv` gains this; `cp`/`rm` already did). A file literally named `-x` is still
reachable as `./-x` (its letters parse as unknown/ignored flags — the established convention); an arg with a
non-letter after the dash (`-2`) or `-` alone is treated as a path.

**Errno additions** (shell-side, the JD6–JD13 pattern):

| condition | tag |
|---|---|
| `cp`/`mv` onto an existing FILE without `-f` | `-EEXIST` |
| `mv -f` onto an existing DIRECTORY | `-EISDIR` (remove it first with `rm -r`) |
| `cp -r` onto an existing directory tree (even with `-f`) | `-EEXIST` (remove it first with `rm -r`) |
| `rm -f`/`rm -rf` on a missing target or no-match wildcard | (quiet — no output) |
| `rm -rf /` | `-EBUSY` (unchanged — not relaxed by `-f`) |

**Principal — unchanged.** The shell is still EL1 ASID 0, the PUBLIC principal; the flags add no new fat.rs
surface, no new lock, and no ACL interaction — they only gate which existing primitive runs.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches (no new warnings — only
the pre-existing `shutdown` double-`hlt_loop`); `./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1
./arroyo test-arm 40` → CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS`;
`esp-jetson kernel.elf` links, **109 `tegra:` strings** — UNCHANGED from JD11–JD13 (the flag strings carry
no `tegra:` token; validate media by count, not size). Zero x86 behavioural change. As in JD2–JD13 the shell
command path is not headless-reachable in-lane, so the shell-level verdict is **attended-pending**.

**Metal verdict — ✅ METAL-CONFIRMED (2026-07-14 attended Orin bench, same card session as JD13; evidence
line above in §JD13).** All bench-card sections passed: no-clobber default (`cp`/`mv` onto an existing file
`-EEXIST`), `-f` overwrite for both, `mv -f` onto a directory refused, `rm -f NOSUCH`/`rm -rf NOSUCH*` quiet,
bundled `rm -rf DIR` removed a tree, `rm -rf /` stayed `-EBUSY`, and a forced `cp -f` overwrite survived a
real power-cycle (boot-2 `cat` read the overwritten bytes). Two bench findings folded: (1) `cp -r`'s
fresh-tree `-EEXIST` fires on the COMPUTED target (`cp -r SRC EXISTING_DIR` correctly takes the JD9
copy-INTO idiom; the refusal needs `EXISTING_DIR/SRC` to pre-exist — confirmed both ways on silicon; the
bench card's original §4 criterion was wrong and is corrected in-repo, `823b5ba`); (2) `mv -f` in the same
directory prints `renamed` not `moved` — correct JD10 wording (same-parent = `rename_entry`).

### JD15 — `-f` tree-replace for `cp -r`/`mv`: forced replace of an existing directory-TREE destination

JD14 bounded `-f` to a single FILE destination: an existing directory tree stayed `-EEXIST` (for `cp -r`) or
`-EISDIR` (for `mv -f` onto a directory), and the operator had to `rm -r` it first. JD15 closes the last gap
in the flag family — **`cp -rf` and `mv -f` now REPLACE an existing directory-tree destination**, so the
forced verbs behave uniformly whether the destination is a file or a whole subtree. Still **`shell.rs`-only,
zero `fat.rs` mutation** (call-never-edit): it composes the JD13 `rm_tree` + `remove_dir` delete primitives
with the existing JD9 copy / JD10 relink paths.

**The mechanism — delete-dst-first, then a fresh copy/move.** A new `force_remove_existing(fs, de, parent,
leaf, canon)` helper removes whatever occupies the destination (a FILE via `locate_in_dir` +
`delete_located`; a DIRECTORY via `rm_tree` to empty it, then `remove_dir`), leaving the destination absent.
The caller then proceeds down its normal fresh-destination path:
- **`cp -rf SRC DST`** — where JD14 returned `-EEXIST` on an existing target, JD15 (under `-f`) calls the
  helper to delete the target first, then builds the FRESH tree exactly as before. Without `-f` the target
  still stays `-EEXIST` (no-clobber remains the panel default).
- **`mv -f SRC DST`** — the JD14 dir-dest refusal (`-EISDIR`) is replaced: under `-f` the existing directory
  destination is tree-deleted, then the entry is relinked (`rename_entry`/`move_entry`) into the freed slot.
  The JD14 file-overwrite path is unchanged.

**⚠ Crash-safe-PARTIAL — honest, bounded, no rollback (the JD13 discipline).** Because the destination is
deleted BEFORE the fresh copy/move, a power cut in the delete→recreate window leaves the destination
**ABSENT** — never a half-overwritten or silently-merged tree. Nothing is rolled back; the operator re-runs
the `cp -rf`/`mv -f` to complete it. This is the deliberate trade `-f` tree-replace makes: it exchanges the
plain `-EEXIST`/`-EISDIR` refusal for a bounded, honest destructive window. no-clobber stays the DEFAULT —
only `-f` opts in; `-n` is unchanged; plain `-f` on a FILE destination is unchanged; a directory destination
WITHOUT `-f` still returns `-EEXIST` (`cp -r`) / lands the source INSIDE it (`mv` copy-into idiom).

**Guards preserved.** The JD9 self/subtree refusal (`-EINVAL`), the `mv` directory-across-parents refusal
(`-EISDIR`, surfaced BEFORE any delete-dst-first so `-f` never destroys a destination for a doomed move),
and the `cp -rf /` / `rm -rf /` volume-root footgun refusals all stand. The volume root is never a
replace target (a computed target is always a leaf under some parent; a defensive `-EBUSY` covers the
unreachable root case).

**Errno additions** (shell-side, the JD6–JD14 pattern):

| condition | tag |
|---|---|
| `cp -r` onto an existing directory tree WITHOUT `-f` | `-EEXIST` (use `cp -rf`, or `rm -r` it first) |
| `cp -rf` onto an existing file OR directory tree | (replaced — delete-dst-first, then fresh copy) |
| `mv -f` onto an existing directory tree | (replaced — delete-dst-first, then relink) |
| power cut mid-replace | destination ABSENT (crash-safe-partial; re-run to complete) |

**Principal — unchanged.** Still EL1 ASID 0, the PUBLIC principal; JD15 adds no new fat.rs surface, no new
lock, and no ACL interaction — it only sequences existing call-never-edit primitives.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches (no new warnings — only
the pre-existing `shutdown` double-`hlt_loop`); `./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1
./arroyo test-arm 40` → CAPSTONE 6/6; `UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS`. Zero x86
behavioural change. As in JD2–JD14 the shell command path is not headless-reachable in-lane (keystroke-driven,
tegra-only), so the shell-level verdict rode the bench card — and is now **✅ METAL-CONFIRMED (2026-07-15
attended Orin bench, serial `jetson-serial-2026-07-15-092500.log`)**: default `-EEXIST`, `-rf` differing-tree
replace (delete-then-rebuild, prior nested content gone), `mv -f` file-over-dir, `-EISDIR`-before-delete,
both `-EINVAL` self-guards and the `-EBUSY` root refusal under `-f`, and power-cut durability of a completed
replace. Card §3 errata: `cp -rf NEW OLD/NEW` onto an existing dir follows the copy-INTO idiom (nests) —
consistent with §2/POSIX; the true replace form is `cp -rf NEW OLD`.

### JB1f — the unhealed early-vector window (the round-6 boot crash), closed (`85f74f8`)

**The evidence (2026-07-11 attended bench, real Orin).** Kernel `446abd3` (the JD6 tip): 2/2 boots
FATAL within ms of `panel LIVE`, inside `fbcon::Sink::write_str`'s per-char glyph loop (nm-confirmed).
Boot A: `ESR=0x2000000` (EC=0 UNDEF-class) at a font-table `ldrb`, EC0-probe D-side readback == the
ELF word exactly — the proven I/D divergence. Boot B: `ESR=0x96000004` (data abort) with
`FAR=0xfffffffffffffed0` — x20 (the sink pointer) reloaded as `-0x130`, corrupted silently upstream;
its ELR sits exactly one A78 I-cache line (0x40) past Boot A's. Kernel `7a126f5` (the JD5-metal
lineage): 2/2 boots CLEAN — and boot 2 carried a HEALED phantom strike, proving the erratum active on
the board that day. Both fatal dumps printed SPSR — a format only `mmu_tegra`'s Part-C
`tegra_fault_handler` emits — pinning both crashes inside the **unhealed window**: the stretch between
`mmu_tegra::init` (which installs the divergent probe-and-spin Part-C vectors at the MMU switch) and
`exceptions::install` at JM4, a stretch that mirrors the whole early boot log glyph-by-glyph once
`fbcon::init` brings the panel up — the boot's heaviest ifetch+store workload, with no heal armed.

**Diagnosis (2-lens + adjudicator panel, unanimous).** One silicon defect, two flavors, plus one
kernel-side latent gap. The bullet is the documented **A78AE erratum 1941500** (arch_arm64.md "JB1
result": r0p1 in range, CPUECTLR_EL1[8] ships clear and is EL3-gated): the UNDEF flavor (Boot A) is
exactly what the JB1e heal retries through; the valid-but-wrong-decode flavor (Boot B — the
historically proven victim word `0xa9454ff4` decodes to `ldp x20, x19, [sp,#0x50]`, an epilogue-shaped
stale fetch that loads x20 with stack garbage) corrupts silently and no OS-side heal can catch it.
**The VPERF fbcon rewrite was exonerated hunk-by-hunk**: every new arithmetic site is compile-time
`cfg(target_arch = "x86_64")`, the aarch64-visible edits resolve to `&self.fb` (semantically identical
to the metal-proven lineage), and every reachable span already used the byte-stride with clamp-to-len
— the Orin's padded stride (2048 px vs 1920 width) was a red herring. What the 7a126f5→446abd3 window
DID change is ~2,400 aarch64 text lines of **binary layout** (K1 syscall, shell, fat — not VPERF
semantics), re-rolling which hot PCs sit on vulnerable I-cache lines. **Ledger note (pattern
sensitivity):** erratum-1941500 strikes are deterministic per binary+flow — treat per-binary layout
luck as expected variance; a layout shift can move the strike site into ANY hot loop, which is why the
whole boot must run under healed vectors rather than chasing the loop of the week. No speculative
video-code change was made (M2 = this note).

**The fix (JB1f, `85f74f8`) — three parts:**
1. **Install the healed `exceptions.rs` vectors EARLY**: `tegra_early_stop` now calls
   `exceptions::install()` right after the mmu-regs banner, BEFORE fbcon starts mirroring. Chosen over
   arming a heal inside the Part-C vectors: one heal implementation (metal-proven, full GPR+FP frame,
   per-strike serial line, naturally shared budget) beats duplicating frame/heal asm that `global_asm!`
   macro scoping cannot share. Part C keeps its probe-and-spin role for the (now three-serial-line)
   switch window itself. Audited safe that early: `BOOT_EL` latches 2, serial is live, the fatal path
   busy-spins (`timer::LIVE` false), and the IRQ entries stay dormant — the HCR_EL2 routing bits move
   earlier but DAIF stays fully masked until JM4's `enable_irq`.
2. **Nest-safety** (panel fold-in #1): `__vec_sync` banks ELR/SPSR/SP_EL0 to its frame and restores
   them before the heal `eret` — a sync fault nesting inside the handler re-banks the per-core
   registers and previously retargeted the outer eret (silent PC corruption of exactly the Boot-B
   shape). The EL bank is a **runtime** `CurrentEL` check, not `__vec_irq`'s compile-time suffix:
   on tegra this vector genuinely serves EL2 (pre-drop) and EL1 (post-drop CAPSTONE, where a heal
   has fired on metal), and an `ELR_EL2` access at EL1 itself UNDEFs.
3. **Heal-storm hardening** (fold-in #2): global budget 64 → 1024 (64 was sized to the
   isolated-strike era; a hot-loop site can legitimately re-strike across iterations), plus a
   consecutive-same-PC cap (32) preserving the wedged-core stop — a different healed PC proves
   progress and resets the streak. The counter is `fetch_add` (the old load-then-store pair was
   non-atomic); same-PC repeats print every 8th line (~8 ms of 115200-baud UART per line at loop
   frequency); the fatal path and `install()` print the heal tally when nonzero, so a
   `try_lock`-skipped heal line can never masquerade as a clean boot.

**Gate:** QEMU byte-equivalent everywhere (the heal never fires in QEMU): `check` +
`UNAOS_TEGRA=1 check` both arches; `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6;
`kernel8` + `kernel8-test` 23 PASS 0 FAIL + CAPSTONE 6/6 + K1 witnesses; `esp-jetson` links,
108 `tegra:` strings. Zero x86 delta. **Metal expectation:** survives `panel LIVE` ×3 boots minimum;
a phantom strike prints a heal line (or a later tally) and CONTINUES; then the JD6 bench card runs to
completion on the same kernel. A silent hang is a FAIL — screen state + serial silence together decide.

> **✅ JB1f METAL VERDICT (attended bench, 2026-07-11 — PASS, panel-observed).** The JB1f kernel
> (`e074518` tip, sha-verified media) ran the full bench on the crash-day board: boots survived
> `panel LIVE` and ran through the boot mirror — the exact stretch that killed `446abd3` 2/2 that
> morning — to the interactive shell, and the JD6 bench card completed on the same kernel (§JD6
> verdict below). Attending-operator verdict: **pass 100%**. ⚠ Evidence caveat, recorded honestly:
> the HOST-side serial capture failed mid-bench (the dated log froze at boot 1's early-UEFI bytes;
> the bridge stdout caught a later boot's MB2 fragment) — so there is NO replay log for an mbench
> assert and no per-strike heal-line tally; the verdict rests on the attended panel observation,
> which is sufficient for the two criteria that matter (boots visibly completed = not a silent
> hang; the JD6 card visibly completed). Whether any strike was silently healed is unknowable from
> this capture — irrelevant to the survival criterion, but the NEXT bench should fix the bridge
> (probe re-enumeration mid-bench is the suspect) and re-capture a serial-asserted boot for the
> tally record.

> **✅ JB1f HEAL-TALLY CLOSED (round-9 attended bench, 2026-07-12 — the honest gap above is filled).**
> The JD7–JD10 kernel (`a834b8f`, `540,064 B`, 108 `tegra:` strings) ran on the Orin with the serial
> bridge verified capturing a FULL boot from byte 0 BEFORE bench time (STEP-0 discipline — the round-6/8
> fix). Across **4 clean boots / 4 power cycles** (one per JD money-shot), the capture stayed live the whole
> session (`~/unaos-bench/jetson-serial-2026-07-12-180110.log`, 318 KB, 1365 keystrokes echoed): every boot
> reached `panel LIVE` → CAPSTONE 6/6 → the interactive shell, and there were **ZERO heal lines and ZERO
> fatal/panic/BOT-timeout across all four boots** — the tally machinery is silent, which (since the fatal
> path and `install()` print the tally when nonzero) means the boots took **0 erratum-1941500 strikes** in
> the healed window, not that strikes were silently absorbed. `JB1d — CPUECTLR_EL1=…(bit8=0)` read cleanly
> each boot. This is the serial-asserted record the round-6 verdict lacked: the JB1f window is healthy on
> this binary, and per-binary layout luck (a strike site landing in the window) is the documented variance —
> none landed here. The bridge survived every board power-cycle without re-enumerating (no re-run needed).

### JD16 — `ls -l`: long listing with REAL FAT timestamps

JD1–JD15 kept `fat.rs` **call-never-edit** for the shell verbs. JD16 is the first arc to touch it, and the
edit is deliberately narrow: a **read-side-only** addition to the parsed directory entry. The `-f`/`-n` flag
family and the file/dir verbs never needed a timestamp; `ls -l` does, and FAT already carries one in every
short directory entry — JD16 simply surfaces it.

**The `fat.rs` read-side grant (bounded).** `DirEntry` gains two fields, `mtime_time` and `mtime_date`, filled
by `classify_dir_slot` from the standard FAT short-entry offsets **0x16 (last-write time)** and **0x18
(last-write date)** — the same 32-byte slot the existing walkers already parse, so there is **zero extra I/O**
and every pre-JD16 caller is byte-identical (the two new fields are simply ignored by code that does not read
them). Nothing else in `fat.rs` changes: no write primitive, no entry-layout serialization, no lock. This is
strictly the DirEntry struct + parse path, so the concurrent x86 write-side arc (STOR-S8) reconciles cleanly.
Creation time (0x0E/0x10) is intentionally **not** read — mtime is what `ls -l` shows, and a second timestamp
would only widen the reconciliation surface.

**The FAT timestamp format — documented honestly.** FAT packs the moment into two 16-bit words:

| word | offset | bit layout |
|---|---|---|
| DATE | 0x18 | bits 15..9 = `year − 1980`, bits 8..5 = month (1..12), bits 4..0 = day (1..31) |
| TIME | 0x16 | bits 15..11 = hour (0..23), bits 10..5 = minute (0..59), bits 4..0 = **seconds/2** |

Consequences, stated plainly: the **epoch is 1980-01-01**; the resolution is **2 seconds** (the low bit of
real seconds is unrepresentable); and there is **no timezone** — the on-disk value is wall-clock local time as
whatever tool wrote it saw, with no stored UTC offset, so `DirEntry::mtime()` presents the packed fields
verbatim (a new `FatTimestamp { year, month, day, hour, min, sec }`). An **all-zero pair** decodes to the
`is_zero()` sentinel (month/day 0) — a value a real stamp never produces — so the display renders it honestly
rather than as a bogus 1980 date.

**The shell (`shell.rs` FAT-verb region).** The `ls` arm parses an `-l`/`-L` flag (the JD14 convention: a
`-`+letters arg is a flag and is filtered out of the positional path, so a file literally named `-l` is still
reachable as `./-l`; unknown flag letters are ignored). Plain `ls` is **unchanged** — byte-identical short
table. `ls -l` adds the FAT last-write timestamp column between size and name (a directory shows `<DIR>` and a
trailing `/` marker):

```
ls -l
       42  2026-07-14 11:37:22  README.TXT
    <DIR>        2026-07-14 11:38:04  DOCS/
       17         -            K4TEST.TXT
2 file(s), 1 dir(s)
```

`fmt_mtime` renders a zeroed stamp as a dashed placeholder of the same width — the honest display for entries
a host tool wrote with a 0 field, **and for every kernel-written entry** (see below). The long format threads
through the same `print_dir_listing` used by both the single-path `ls` and the JD12 wildcard `ls *.EXT`, so
`ls -l *.TXT` gets the timestamp column too.

**What a KERNEL-written file's timestamp actually contains — observed, not invented.** The kernel has **no
RTC**. The `fat.rs` create path (`create_in_root`/`create_in_dir`) zeroes the entire 32-byte entry except
name/attr/first_cluster/size — so the time and date words are written as **0**, and the JD5/JD6 write/append
paths only republish size + chain-head (a U10 read-modify-write), never the timestamp. Therefore **a file the
OS itself creates or writes carries an all-zero mtime**, which `ls -l` renders as the dashed placeholder. That
is the correct, honest verdict for this arc: the OS does not fabricate a clock reading. Files written on the
host (by `mkfs`/copy tools) carry whatever real timestamp that host stamped, and those show through faithfully.
**A real on-write clock (RTC read on Pi/Jetson, or a monotonic-since-boot stamp) is a named FUTURE arc**, not
JD16 — JD16's contract is to display truthfully whatever the on-disk field holds.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches (no new warnings beyond
the pre-existing set); `./arroyo test-arm 22` → `MISSION SUCCESS`; `./arroyo kernel8-test` → **40 PASS / 0
FAIL** (this battery protects `fat.rs`, shared with the Pi image); `./arroyo test 25` → `MISSION SUCCESS`
(x86, `fat.rs` shared there too). As in JD2–JD15 the shell command path is not headless-reachable in-lane, so
the shell-level `ls -l` verdict is **✅ METAL-CONFIRMED (2026-07-15 attended Orin bench)**: a host-written
file's stamp showed through to the exact second (09:20:34), every kernel-written entry showed the honest
dash (bench card `unaos/scripts/jd16-bench.md`).

### JD17 — the KERNEL CLOCK: `setdate`-seeded wall time that stamps FAT mtime

§JD16 exposed the honest gap: with no RTC the kernel reads, every kernel-written FAT entry carried an all-zero
mtime and `ls -l` showed a dashed placeholder. JD17 closes that gap **without inventing a clock**: the operator
seeds a wall-clock once per boot, the free-running architectural counter extends it forward, and the FAT
create/write **publication** paths stamp mtime from it — but only when the operator has actually set it. UNSET
stays first-class and honest.

**The clock service (`clock.rs`, new).** A `WallTime { year, month, day, hour, min, sec }` (calendar-validated
to the FAT-representable span **1980..=2107**) plus:
- `set(t)` plants an **anchor**: `base_secs` (whole seconds since the 1980-01-01 epoch) paired with the
  architectural counter reading at the moment of setting, under a small `spin::Mutex`. Re-seeding replaces the
  anchor — the operator's correction wins. Out-of-range input returns `Err(())` (the shell shows a usage line).
- `now()` = `base_secs + (CNTPCT_now − anchor_ticks) / CNTFRQ`, or `None` while never set this boot. The
  counter is the **same JD3 timerless mechanism** (`CNTPCT_EL0`/`CNTFRQ_EL0`, EL-independent, never stops) the
  BOT pump and JD4 screen-on-boot deadline already ride.
- `fat_stamp()` packs `now()` into the two on-disk words `(time @0x16, date @0x18)` — bit-for-bit the inverse
  of §JD16's `DirEntry::mtime()` decode (DATE: `year−1980`/month/day; TIME: hour/min/`sec÷2`) — and returns
  **`(0, 0)` while unset**, byte-identical to the pre-JD17 zeroed field that `ls -l` renders as the dash.

`from_secs` **saturates at end-2107** (the last FAT-representable moment) rather than wrapping or panicking, so
a clock left running for 128 years degrades to a pinned honest maximum. Resolution is **2 seconds** (the FAT
packing truncates the low second bit) and there is **no timezone** — FAT stores local wall time with no offset,
exactly as §JD16 documented on the read side.

**The frozen-x86 note — stated honestly.** No calibrated invariant-frequency counter is plumbed on x86_64 in
this kernel (the TSC frequency is measured nowhere), so `monotonic()` returns `None` there and a set clock is
**frozen at its seeded second** (elapsed = 0). No x86 caller sets the clock today; the `date`/`setdate` verbs
merely compile. x86 monotonic calibration is explicitly **out of scope** (a named future arc).

**The shell (`shell.rs`).** Two additive arms: `date` prints the current wall clock or `date: clock not set`
when unset; `setdate YYYY-MM-DD HH:MM[:SS]` (seconds optional, default 0) seeds it. `parse_setdate` enforces
the strict field shapes (dash/colon-separated decimals) and hands the numbers to `clock::set`, which owns the
range validation. A `CLOCK:` help line was added.

**The FAT write-side (`fat.rs`) — publication paths only, RMW-riding, no new lock.** The stamp lands in the
**existing** `with_dir_lock` sector RMWs — no extra I/O, no second crash window:
- **Both create twins** (`create_in_root`/`create_in_dir` — the VERBATIM-TWINNED slot write, kept in sync)
  stamp the two mtime words in the same slot write. The slot is pre-zeroed, so `(0,0)`-when-unset is
  byte-identical to the pre-JD17 create.
- **`write_grow` step-4 publish** calls a new sibling `write_dir_entry_fields_mtime` — identical to
  `write_dir_entry_fields` (the same `first_cluster`+`size` single-sector RMW) but additionally refreshing the
  mtime words. Crucially, **when the clock is UNSET it leaves the existing on-disk words UNTOUCHED** — a
  host-stamped file rewritten by a clockless kernel **keeps its old stamp** rather than being zeroed (strictly
  less destructive than fabricating or erasing).

**What does NOT refresh mtime this arc — the honest gap.** The stamp lands **only on entry-publication paths**
(the two creates and `write_grow`'s step-4 publish). `fat.write_at` — the strictly-bounded **in-place
overwrite** — stays **completely untouched**: it is guaranteed dir-untouched / never-grows / never-allocs, and
the x86 S8 witness and the S3 write-through path lean on exactly that contract. Consequence, stated plainly: a
**pure in-place overwrite does not refresh mtime this arc**. This is not user-visible from the panel shell —
its `write` verb is truncate-recreate (a create, stamped) and its append is `write_grow` (stamped), so **every
shell mutation still stamps**; the only unstamped path is the EL0 in-place `sys_write` syscall, and refreshing
it would widen the S8 reconciliation surface for no shell-visible gain. `rename_entry`/`move_entry` keep the
plain (non-mtime) sibling on purpose — **a rename/move preserves mtime**, and a fresh directory entry was
already stamped at create.

**The unset-honesty contract, in one line.** The kernel never fabricates a reading: unset ⇒ `now()` is `None`,
`fat_stamp()` is `(0,0)`, creates write zero (the dash), and rewrites preserve whatever the on-disk stamp was.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches (no new warnings);
`./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6;
`./arroyo kernel8-test` → **0 FAIL** (this battery protects `fat.rs`, shared with the Pi image);
`UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (x86, shared shell/fat guard). The wall-clock stamp
is not headless-reachable in-lane, so the shell-level verdict rode the card — now **✅ METAL-CONFIRMED
(2026-07-15 attended Orin bench)**: unset-honest both boots, seed counter-extended across live `date` reads,
out-of-range `setdate` rejected without disturbing the set clock, post-seed files stamped at the FAT
2-second resolution while a pre-seed file kept its dash, the stamp byte-identical across a genuine power
cut, and the next boot up unset again (bench card `unaos/scripts/jd17-bench.md`).

### JD18 — read-only TREE TOOLS: `find` (recursive glob search) + `du` (subtree size tally) + `uptime`

Three read-only panel additions built entirely from primitives the file-manager verb set already ships — **`shell.rs`-only, zero mutation, `fat.rs` call-never-edit, NET arms + unafs verbs untouched.** They compose the JD9 `cp_tree`/JD13 `rm_tree` `read_dir` SNAPSHOT walk, the JD12 `glob_match`, and the JD17 clock's additive `uptime_secs()` helper; `.`/`..` are filtered at every level and recursion is bounded by the shared `CP_MAX_DEPTH` (=32, honest `-ELOOP`). A mid-walk read error stops with an honest `path: reason (-EIO)` and the partial results already printed — nothing is invented.

**`find <root> <pattern>`.** Recursively walks the tree under `<root>` (a directory path; one argument = the pattern with root defaulting to `.`, two = explicit root + pattern) and matches each entry's on-disk 8.3 name against `<pattern>` with the **existing JD12 `glob_match`** (case-insensitive; `*`/`?`; a literal pattern is an exact-name match). Each hit prints as its full canonical path — a directory with a trailing `/` — followed by an honest `N match(es), M dir(s) scanned` tally, where *dirs scanned* counts every `read_dir` level (the root included). A subdirectory is always recursed into whether or not its own name matched. Errors are honest: a missing root is `-ENOENT`; a **FILE root degrades to a single self-match test** (the POSIX shape — `find` on a file tests that file, `0 dir(s) scanned`); a mid-walk I/O error reports the path + errno with the partial hits standing.

**`du <dir>`.** Same read walk. For each **direct child** of `<dir>` it prints the child's total bytes — a file is its own size, a directory is the recursive sum of its subtree — then a `total: N byte(s) in M file(s), K dir(s)` line. **FAT directory ENTRIES themselves report size 0** (the on-disk size field is zero for directories — only file sizes are real bytes), so a directory contributes only the sum of its descendant files. `du FILE` is that file's single line. A missing path is `-ENOENT`; a mid-walk read error reports the path + errno and the `total:` line stays honest for what was scanned before the stop.

**`uptime`.** Seconds since boot from the architectural counter, via a small **additive** `clock::uptime_secs() -> Option<u64>` = `CNTPCT_EL0 / CNTFRQ_EL0` on aarch64 (the counter resets to 0 at boot and never stops — the same JD3 mechanism `now()` extends from), `None` on x86_64 (no calibrated invariant counter is plumbed). The helper reads the same `monotonic()` source `now()` uses but touches **neither the seed anchor nor the `now()`/`fat_stamp()` logic**. The verb renders `up HH:MM:SS`; when the JD17 wall clock has been seeded it appends the current time — `up 00:12:34 (clock: 2026-07-15 14:32:10)`. x86 prints an honest `uptime: no calibrated counter on this arch`.

**Not in scope:** any mutation, `find -exec`/`-type` flags, mid-path globs (the JD12 trailing-leaf rule stands), `du -h` human units, and sorting beyond the natural walk order.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches (no new warnings); `./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6; `./arroyo kernel8-test` → **0 FAIL** (this battery protects `fat.rs`, shared with the Pi image; these verbs add no `fat.rs` surface); `UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (x86, shared shell guard); `UNAOS_TEGRA=1 ./arroyo esp-jetson` links, validate by `tegra:` COUNT (unchanged — these verbs add no `tegra:` token) never size, built LAST after `test-arm`. The tools are exercised interactively, so the shell-level verdict rode the card — now **✅ METAL-CONFIRMED
(2026-07-15 attended Orin bench)**: `find` recursive glob correct at scale (29 matches / 19 dirs scanned on
the full tree, scoped and no-match forms honest), `du` tallied a seeded tree exactly (12 bytes / 3 files /
1 dir), `uptime` counter-derived both without and with the seeded-clock parenthetical (bench card
`unaos/scripts/jd18-bench.md`).

### JD19 — read-only FORENSIC verbs: `stat` (full on-disk detail) + `xd` (bounded hexdump)

Two read-only inspection verbs that expose the on-disk truth of a FAT entry — **`shell.rs`-only, zero mutation, `fat.rs` call-never-edit, NET arms + unafs verbs untouched.** Both ride primitives the read path already ships (`resolve_path`/`locate_in_dir`/`read_at`) plus, for `stat`'s attr byte, one raw `block::read_block` of the on-disk directory sector — the same raw block path the `read <lba>` verb uses. Neither is glob-wired: a metacharacter in the path resolves literally, an honest `-ENOENT`, exactly as a mid-path glob does today.

**`stat <path>`.** Prints one directory entry's full detail: the canonical absolute path, kind (file/dir), size in bytes, the raw FAT **attr byte** (hex + decoded `RO`/`HIDDEN`/`SYS`/`DIR`/`ARCHIVE` flags, `-` when none set), first cluster (hex; `0x0` honest for a 0-length file), the FAT last-write stamp (a bare `-` when the on-disk pair is zeroed, via the §JD16 `fmt_mtime`/`FatTimestamp::is_zero`), and the **on-disk location** — the directory-entry LBA + 32-byte slot offset. The parsed `DirEntry` keeps only `is_dir` (not the whole attr byte), so `stat` reads the true byte back from slot offset `+11` of the on-disk directory sector returned by `locate_in_dir` (which yields `(DirEntry, dir_lba, dir_off)`). **`stat /` reports the root honestly** — a directory with **no directory entry of its own** (the FAT root has no parent slot), so the `entry:` line says so rather than inventing an LBA. A missing path is `-ENOENT`.

**`xd <path> [off] [len]`.** A bounded hexdump of a file's bytes via the offset-aware `read_at`: default `off=0`, `len=256`, with `len` **hard-capped at 4096**. Rows are the canonical `OFFSET: <16 hex bytes> | <ascii> |` layout, labelled with the **absolute file offset** (starting at `off`, not from 0 like the raw `read`-verb dump) and non-printables rendered as `.`; a short final row is padded so the ASCII gutter stays aligned. When the file holds more bytes past the dumped window — a cap hit, a short `len`, or both — an honest `[... n more byte(s)]` tail note is printed. An `off` at or past EOF is an honest `offset N at/past EOF` note (no rows); a directory target (and the root) is `-EISDIR`. `off`/`len` accept decimal or `0x`-hex.

**Not in scope:** any mutation, glob expansion for these two verbs, raw-LBA dumps (the `read <lba>` verb already covers that), and any FAT-internal walk beyond the public API.

**Gate (QEMU):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches (no new warnings); `./arroyo test-arm 22` → `MISSION SUCCESS`; `UNAOS_GICV3=1 ./arroyo test-arm 40` → CAPSTONE 6/6; `./arroyo kernel8-test` → **0 FAIL** (protects `fat.rs`, shared with the Pi image; these verbs add no `fat.rs` surface); `UNAOS_HUBSTORAGE=1 ./arroyo test 25` → `MISSION SUCCESS` (x86, shared shell guard); `UNAOS_TEGRA=1 ./arroyo esp-jetson` links, validate by `tegra:` COUNT (unchanged — these verbs add no `tegra:` token) never size, built LAST after `test-arm`. Exercised interactively; the metal verdict is **✅ METAL-CONFIRMED (2026-07-15 attended bench)** — the
forensic view exact on silicon: attr/cluster/entry-LBA+slot surfaced, host mtime to the second, root/
missing/dir/past-EOF all honest (bench card `unaos/scripts/jd19-bench.md`).

### RAST-TEGRA — first 3D pixels on the Orin panel (`UNAOS_RAST=1`, ⏳ METAL-PENDING)

The platform-neutral software rasterizer (RAST-1) wired to the JD1 panel. With the `rast` knob
armed, `tegra_early_stop` runs the spinning flat-shaded z-buffered cube demo through the **inherited
scanout** — no mode-set, no scanout reprogramming — as the last panel content before CAPSTONE. The
call is `tegra_rast_demo_maybe()` at the EL1 tail (post-drop, right before `run_capstone_boot_core`):
it builds a `Screen` over `video::WRITER` (seeded by JD1, mapped into both translation tables so the
carveout is reachable at EL1), detaches fbcon's mirror, then calls the shared, arch-neutral
`rast_demo::run()` — **call-never-edit** on the panel surface (the same present path RAST-1 proved on
x86). `crate::arch::ms()` reads `CNTVCT` on the timerless post-drop core (the VUGFIX fallback), so the
honest fps line still ticks. Design + full detail: [rasterizer.md §4](../08_VIDEO/rasterizer.md).

**Byte-identity.** The wire-in adds zero source lines ahead of any panic `Location` literal — the
runner sits at the file tail and is called on the *same source line* as the terminus, with an
`#[inline(always)]` empty knob-off twin (the PI-V3D-1 panic-line constraint). Verified: the tegra
knob-off kernel is byte-identical to base — `.text a2ce1599…`, `.rodata 5d1f7604…`, `.data 4f1fe11e…`,
`.data.rel.ro e17e3b13…`, 0 `rast` symbols.

**Witness.** QEMU never builds `tegra`, so the on-panel cube rides the attended Orin bench; the honest
QEMU proof of the identical arch-neutral render is the aarch64/**virt** path (GICv2 + `ramfb`):
`UNAOS_RAST=1 ./arroyo test-arm` prints `:: RAST: … spinning cube … ::` + the fps line, and the boot
still reaches `MISSION SUCCESS`.

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

### ORIN-SMP — the CORE3-class audit of the PSCI bring-up + the born-fixed re-derive (phase 1)

The Pi CORE3-SMP fix (§CORE3-SMP: an MMU-off stack spill of the secondary core-id reloaded
stale-cacheable after the MMU turns on) flagged three analogous aarch64 files as out of the Pi
lane — `smp_virt.rs`, `boot_virt.rs`, `boot_tegra.rs`. This arc audits all three for the same
mismatched-attributes coherency hazard and fixes the one that carries it, so the Orin PSCI path
is **born fixed** rather than patched after a (currently metal-blocked) bench regression. The
hazard is **QEMU-invisible** (TCG models no caches) and **image-layout-deterministic**, so the
verdict is the disassembly, not the QEMU battery.

**Audit verdict.**

* **`boot_virt.rs` — SOUND (BSP-only drop, no incoming-id spill).** `drop_to_el1(ram_gib_mask)`
  runs on the boot core alone; there is no PSCI-delivered core-id argument to spill. The one
  MPIDR concern — an EL1 `mrs MPIDR_EL1` reading `VMPIDR_EL2` — is handled: the naked
  `drop_el2_to_el1_virt` seeds `VMPIDR_EL2` from the real MPIDR (`mrs x0, mpidr_el1; msr
  vmpidr_el2, x0`) as its first pair, strictly before the `eret`, and `enable_el1_regime`
  contains no MPIDR read. The EL1 MMU is armed dormant at EL2 (`SCTLR_EL1.M=1` before the eret),
  so EL1 never runs MMU-off. Nothing to fix.
* **`boot_tegra.rs` — SOUND (same shape, `l1_pa` signature audited).** `drop_to_el1(l1_pa)` is
  likewise boot-core-only. The differing signature (`l1_pa` = `MmuInfo::ttbr0_el1`) only flows to
  `TTBR0_EL1` inside `enable_el1_regime` — no MPIDR interaction. The VMPIDR seed order is correct:
  `drop_el2_to_el1_tegra` seeds `VMPIDR_EL2`/`VPIDR_EL2` (after `daifset` masking) before the
  `eret`, and neither `enable_el1_regime` nor the mask touch MPIDR. Nothing to fix.
* **`smp_virt.rs` — HAZARD PRESENT (fixed).** `__secondary_rust_virt` receives the PSCI context
  id (the **linear core index** — deliberately not MPIDR Aff0, which is 0 on every Tegra234
  cluster) in x0 with the MMU off, calls `enable_mmu_virt()` (MMU on), then uses the id for
  `percpu::init`, the `CORE_READY` index, and `serial_println!`. Pre-fix codegen (llvm-objdump of
  the retained `virt` build) showed the exact CORE3 forcing shape:
  `5e740: str x0,[sp]` (MMU-off spill of the id) then `5e780: msr SCTLR_EL2,x8` (MMU on) then
  `5e8dc: bl __print` reading `&core = sp+0` by reference then `5e8e0: ldr x0,[sp]` (cacheable
  reload) feeding the `CORE_READY[core]` Release store. Identical to the Pi: `serial_println!`'s
  Display-by-reference makes the MMU-off stack copy load-bearing. The existing
  `clean_invalidate_range(SECONDARY_STACKS)` before `CPU_ON` narrows but does not delete the
  window (it clears BSP-seeded lines once; it does not make the spill/reload structurally MMU-on).

**The fix (`smp_virt.rs`, born on the idiom, with the Orin correction).** The Pi's literal
"re-derive from `MPIDR_EL1 & 0xff`" cannot apply — the linear index is not recoverable from Aff0
on a multi-cluster part. Instead: the BSP publishes the linear-index to packed-affinity table
(`AFF_BY_INDEX` + an `N_CORES_PUB` Release) before the first `CPU_ON`; the secondary ignores the
now-advisory context id and, **after the MMU is on**, reads its live `MPIDR_EL1` affinity
(`gic::this_affinity()`, full packed `{Aff3,Aff2,Aff1,Aff0}` — never a bare Aff0 mask) and matches
it against the table to recover its own index, parking in `wfe` on no match (the graceful
BSP-timeout failure mode). Post-fix codegen confirms the deletion: `5b5e4: msr SCTLR_EL2` (MMU on)
then `5b5f4: mrs MPIDR_EL1` (id derived MMU-on); the only `[sp]` stores of the derived id
(`5b620: str x19,[sp]`) and its reload (`5b79c: ldr x0,[sp]`) are both MMU-on and
cacheable-coherent; the advisory x0 is never spilled, and the only MMU-off stores are the
non-reloaded callee-saved saves (the function is `-> !`, no epilogue). The stale-line window is
structurally deleted, not patched — mirroring §CORE3-SMP with the multi-cluster affinity decode.

**Gate + status.** `./arroyo check` green (both arches, with and without `tegra`); `test-arm`
GICv3 SMP path brings all 3 secondaries online (each re-derives its correct index), BSP/AP SGIs
deliver 3/3, CAPSTONE 6/6; plain `test-arm` (GICv2 single-core) byte-unaffected (smp_virt
runtime-gated on `is_v3`). QEMU proves only non-regression — the hazard is QEMU-invisible. There
is **no metal leg for this arc**: the tegra binary DCEs `smp_virt` (JM5 Orin PSCI is parked on the
external Tegra BL31/MCE `CPU_ON` RAS fault — see "JM5 result"), so the tegra image is byte-unchanged
by this fix, and the disassembly is the proof of record. When the Orin `CPU_ON` firmware wall is
cleared, the bring-up is already born fixed.

### ORIN-SMP-2 — the JM5 `CPU_ON` firmware-wall INVESTIGATION probe (`UNAOS_SMPPROBE`, code-only; bench Peter-attended)

JM5's metal verdict isolated the failure to the *first* PSCI `CPU_ON`: every PSCI query
(`AFFINITY_INFO`) returns cleanly on silicon, but the first `CPU_ON` raises a fatal Tegra CBB-fabric
RAS Uncorrectable Error inside BL31/MCE and powers the box off. This arc ships **instrumentation to
discriminate §JM5-result's four ranked hypotheses** — not a fix and not a bench. The deliverable is a
default-OFF probe knob plus a pre-registered A-B-A runbook (`unaos/scripts/orin-smp2-bench.md`); the
bench is Peter-attended and power-fault boots are DATA.

**The knob (`arch/aarch64/smpprobe.rs`, `tegra`+`smpprobe` gated).** `UNAOS_SMPPROBE=<n>` selects ONE
experiment per boot, recorded to serial as grammar-checkable single-line records
(`:: tegra: SMPPROBE sel=<n> … ::`, the pi `core3probe` idiom — bounded, expected-vs-got). The whole
module and its one call site in `tegra_early_stop` (after JM4 GIC/timer/heap, before the JM6 drop) are
`#[cfg(all(feature = "tegra", feature = "smpprobe"))]`; with `smpprobe` OFF (the default) they vanish
and the tegra image is **byte-identical to baseline** (proven: two default `esp-jetson` builds hash
identical, `tegra:` count 109). The experiment is a compile-time const from
`option_env!("UNAOS_SMPPROBE")`, so each armed value is a **distinct image** (distinct hashes; cargo
rebuilds on the env change) — the operator rebuilds+reflashes per boot for the A-B-A schedule, and
every record echoes the LIVE `SEL` so the operator VERIFIES which experiment ran before trusting a
boot. A `static` fn-pointer table + `black_box(SEL)` keep every experiment's strings linked regardless
of `SEL`, so any armed image has the SAME **`tegra:` count = 142** (109 baseline + 33 probe strings).

**Safety (RIDER (b) — probe-only).** No experiment writes fuses, BCT/EEPROM, UEFI variables, MB1/MB2
storage, or persistent MCE/firmware config. Queries are read-only SMCs / system-register reads; the
`CPU_ON`-issuing experiments (3, 5) command a *volatile* core-power action (what JetPack's OS does
every boot) and write no persistent state. H4 (caller-EL) is recorded **BLOCKED-BY-DESIGN** — its
discrimination cannot be reproduced from our minimal EL2 kernel (see the table).

**Pre-registered prediction table (RIDER (a)).** Written BEFORE any bench; the runbook carries it
verbatim. `knob → hypothesis → predicted serial record → predicted box behavior`:

| knob | hypothesis / role | issues `CPU_ON`? | predicted serial record | predicted box behavior |
|---|---|---|---|---|
| **0** | CONTROL — `AFFINITY_INFO` topology sweep (Aff2 0–3 × Aff1 0–3) + redistributor walk | no | `sel=0 slot aff=… AFFINITY_INFO=<0/1/2 or −>` for each slot; the fused cores report valid (0/1/2), unpopulated slots return −INVALID_PARAMS; `present=<k>` | clean boot → CAPSTONE (the control) |
| **1** | **H1** MCE/BPMP coordination — census what BL31 advertises | no | `PSCI_VERSION=…`; `FEATURES(CPU_ON)=<r>`; `FEATURES(AFFINITY_INFO)`; `MIGRATE_INFO_TYPE=<r>` | clean boot → CAPSTONE. **Read:** if `CPU_ON` is advertised (`≥0`) yet still faults, the failure is inside Tegra's `CPU_ON` impl (consistent with the MCE-coordination story), not an unrecognized call |
| **2** | **H2** latent/poisoned RAS surfaced by the first EL3 barrier — read RAS error records BEFORE any `CPU_ON` | no | `ID_AA64PFR0.RAS=<f>`; then per record `ERXSTATUS=… V=<b> UE=<b> ERXADDR=… ERXMISC0=…` | clean boot → CAPSTONE. **Read:** a pre-existing `V=1`/`UE=1` record supports H2; all-clean weakens it |
| **3** | **H3** entry-point-high — `CPU_ON` to the first present secondary with a LOW (2 GiB sentinel) entry PA | **yes** | `sel=3 target aff=… entry=0x80000000 … issuing CPU_ON`; then either a RAS fault (no further record) or `CPU_ON RETURNED ret=<r> — SURVIVED` | **RAS fault + power-OFF** if H3 false (fault precedes the fetch); SURVIVAL ⇒ H3 candidate |
| **4** | **H4** caller-EL — **BLOCKED-BY-DESIGN** | no | `sel=4 exp=el1-caller BLOCKED-BY-DESIGN`; `reason=SMC from NS-EL1 vs NS-EL2 hits the same BL31 handler; JetPack's difference is its boot-time ATF handshake, not the runtime caller EL — not reproducible from our EL2 kernel` | clean boot → CAPSTONE (records the block) |
| **5** | **H3 reference / baseline reproduction** — `CPU_ON` to the first present secondary at the HIGH (~9.5 GiB kernel) entry PA of `_smpprobe_park` | **yes** | `sel=5 target aff=… entry=0x25e……(HIGH) … issuing CPU_ON` then a RAS fault (no further record) or `CPU_ON RETURNED ret=<r> — SURVIVED` | **RAS fault + power-OFF** (the JM5 wall, isolated to one core). exp3 vs exp5 differ ONLY in the entry PA: same fault ⇒ H3 refuted |

exp3/exp5 both gate the `CPU_ON` behind an `AFFINITY_INFO` presence check (the JM5 attempt-1 lesson:
a `CPU_ON` to a fuse-disabled phantom is itself a fatal RAS) and target exactly ONE secondary (minimal
blast radius), with the full enumeration dumped before the call so the record survives the power-off.

**Gate (this executor).** `./arroyo check` green both arches ±`tegra`; `test-arm 22` MISSION SUCCESS;
GICv3 `test-arm 40` CAPSTONE 6/6 + 3/3 secondaries (smp_virt untouched); `kernel8-test` 0-FAIL;
`UNAOS_HUBSTORAGE` x86 MISSION SUCCESS; `esp-jetson` links, `tegra:` count 142 armed / 109 off
(byte-identical off). The bench closes at Peter's window with LC-orin per the runbook.

### ORIN-SMP-3 — the real 6-core Orin bring-up (`UNAOS_TEGRASMP`, knob-gated; bench Peter-attended)

ORIN-SMP-2's attended bench proved the wall of §JM5-result is GONE on current firmware — `CPU_ON`
returns `ret=0` and the box stays healthy on UEFI `t23x_general 39.2.0-gcid-45755727` (2026-06-01).
This arc wires the tegra SMP kick-off that the born-fixed §ORIN-SMP bring-up was waiting on: a
default-OFF `UNAOS_TEGRASMP=1` build-time knob that, when armed, brings every real Orin core online
via PSCI `CPU_ON` after JM4 and before the JM6 drop. Default OFF, the whole kick-off and its `/cpus`
enumerator vanish and the tegra image is **byte-identical to baseline** (two default `esp-jetson`
builds hash identical, `tegra:` count 109 unchanged). The METAL verdict is the attended Orin bench
(`unaos/scripts/orin-smp3-bench.md`); QEMU cannot model the Tegra machine, so QEMU here is
regression-only (the shared `smp_virt` GICv3 path stays 3/3 + CAPSTONE 6/6).

**Presence + table (M1).** The target list is produced by `fdt_tegra::cpu_affinities` from the
firmware DTB `/cpus` walk ALONE (a direct `cpu@…` child's `reg` = its MPIDR affinity, cell count from
`/cpus/#address-cells`; converted to the packed GICR-contiguous form). `start_secondaries_tegra`
builds the dense linear-index → affinity table (BSP = `gic::this_affinity()` at index 0, each other
`/cpus` core at 1..N in DTB order), publishes it via the existing `AFF_BY_INDEX`/`N_CORES_PUB` Release
protocol, and prints each enumerated core's affinity to serial (the bench evidence). If `/cpus` names
nothing (unmapped/malformed DTB, or a headless handoff without one) it STOPs single-core — never a
phantom start.

**Kick-off (M2).** `start_secondaries_tegra` runs from `tegra_early_stop` after JM4 (GIC-600 +
generic timer + heap + SMC all live) and **before** the JM6 EL2→EL1 drop, so the BSP is still at EL2.
It `CPU_ON`s each `/cpus`-named secondary at the `_secondary_start_virt` stub with the linear index as
context id; each secondary runs the **born-fixed** `__secondary_rust_virt` path (re-derive the index
from live `MPIDR_EL1` full affinity post-MMU, never the MMU-off-spilled context id — §ORIN-SMP). A
bounded ~500 ms per-core wait gates readiness; a core that misses = WARNING + continue (the graceful
pre-fix mode, never a hang). Expected serial: `AARCH64 SMP: AP <n> online (aff=…)` ×5 + the BSP↔AP SGI
proofs, then the boot core proceeds to the JM6 EL1 drop + CAPSTONE.

**EL regime — no divergence needed (brief point 4).** PSCI wakes a secondary at the *caller's* EL. The
kick-off precedes the JM6b drop, so the BSP is at EL2 and the secondaries wake at EL2 and replay the
BSP's live EL2 regime through `SEC_CTX`/`enable_mmu_virt` — exactly the state the shared `smp_virt`
path already captures. The JM6b EL1-precise twin table (`L1_EL1`) is the boot core's *post-drop*
regime; it is not on the secondaries' path, and no tegra `SEC_CTX` variant is required. (The stub's
`CurrentEL != 2` guard still parks any AP a firmware monitor unexpectedly dropped to EL1, which the
BSP observes as a clean `CORE_READY` timeout.)

**RIDER 3 — the oracle discrepancy (hard-won, silicon-measured; do not re-litigate).** On the 6-core
Orin Nano three "presence" sources DISAGREE, and only one is trustworthy:

* **`AFFINITY_INFO`** answers a *valid* state (0/1/2) for **12** affinity slots — the die's full
  12-core layout — even though only 6 are fused in. It is a firmware topology table, NOT a fused-SKU
  presence oracle. Trusting it re-opens the JM5 attempt-1 wall: a `CPU_ON` to a fuse-disabled phantom
  is a fatal Tegra CBB-fabric RAS Uncorrectable Error that powers the box off.
* **The GIC-600 redistributor walk** exposes **8** frames (the die's core slots) — also more than the
  6 real cores.
* **The DTB `/cpus` node**, which NVIDIA's UEFI populates from the *fused* SKU, names exactly the **6**
  present cores.

So ORIN-SMP-3's target list is sourced from `/cpus` ALONE (RIDER 1): no code path issues `CPU_ON` to
an affinity absent from that enumeration — not from `AFFINITY_INFO`, not from the GICR walk, not from
a hardcoded list. `cpu_affinities` is the single provable producer (FDT walk only; the review lens
verifies). The fuse-disabled-`CPU_ON` question stays UNTESTED and is not retested by accident — only a
pre-registered leg (Peter) may probe it later.

**RIDER 2 — firmware precondition.** The bench card asserts the UEFI build line
(`t23x_general 39.2.0-gcid-45755727`, or newer Peter-acknowledged) as a precondition; a
downgraded/different firmware = STOP before any `CPU_ON` (the wall may still stand there). The
kick-off's first serial line restates it so the transcript self-documents.

**String-count note.** Unlike ORIN-SMP-2's probe (whose records are `:: tegra: …` → count 142), the
kick-off's records use the `:: AARCH64 SMP: ORIN-SMP-3 …` family (the `smp_virt` convention), so the
armed image's `tegra:` count is **109 — unchanged from baseline**. Validate the armed image by the
distinct ELF hash + the presence of `ORIN-SMP-3` strings, NOT by the `tegra:` count (which is
identical off and on).

**Gate (this executor).** `./arroyo check` green (both arches) + `UNAOS_TEGRA=1` + `UNAOS_TEGRA=1
UNAOS_TEGRASMP=1`; `test-arm 22` MISSION SUCCESS; GICv3 `test-arm 40` CAPSTONE 6/6 + 3/3 secondaries
(the shared `smp_virt` path is byte-untouched); `kernel8-test` 0-FAIL; `UNAOS_HUBSTORAGE` x86 MISSION
SUCCESS; `esp-jetson` links (built LAST), knob-off byte-identity proven (two default builds hash
identical, `tegra:` 109). The metal verdict is the attended bench with LC-orin + Peter.

### ORIN-SMP-4 — the woken core's EXECUTION BISECT (`UNAOS_SMPPROBE=10..16`, knob-gated; bench Peter-attended)

The SMP-3 attended bench (§ORIN-SMP-3 STOP record) DISCRIMINATED the wall: on UEFI 39.2.0 firmware
`CPU_ON` itself works (the SMP-2 exp5 park survived, ret=0), but waking the SAME core (aff
`0x00000100`) into the real `smp_virt::_secondary_start_virt` RAS-faults ×2 reproducibly (IOB Status
`0xe4000612`, SERR=0x12 slave-error, IERR=CBB-0x6, ADDR `0x8000000000000200`, + ACI, box reset) BEFORE
the BSP prints the `CPU_ON` result. So the fault is driven by the woken core's EARLY EXECUTION — some
access in our secondary path is rejected by the Tegra CBB fabric. ORIN-SMP-4 is the pre-registered
**execution bisect** that brackets which access, one leg per boot.

**Mechanism (`arch/aarch64/smpprobe.rs`, extends the `UNAOS_SMPPROBE` probe).** Each leg 10..16 wakes
ONE `/cpus`-named core into a MINIMAL entry stub that adds exactly ONE variable over the previous leg,
then parks in WFE. Because a secondary's UART is unarbitrated on metal (the pi core3probe lesson), the
woken core **never prints**; it raises a per-leg **CHECKPOINT** flag — a plain store of `0x5304_000<leg>`
+ `DC CVAC` to the Point of Coherency (the spin-table-slot idiom, MMU-off-safe) — that the BSP polls
under a bounded ~500 ms deadline (invalidate-then-read). A raised checkpoint = the leg SURVIVED; a RAS
power-off before the poll completes = the leg faulted and NAMES the rejected access; a bounded timeout
with the box still up = a wrong-EL park or hang. All evidence is BSP-side serial (`:: tegra: SMPPROBE-4
… ::`).

The bisect is **self-contained** in `smpprobe.rs` (its own single 64 KiB stack, its own captured EL2
regime `ProbeRegs`, its own checkpoint, its own entry stubs) so the working SMP-3 path in `smp_virt.rs`
stays **byte-untouched** — the diagnostic cannot perturb the code it measures. Leg 16 replicates the
tail of `__secondary_rust_virt` (percpu + GICv3 secondary bring-up + the SGI ping) from the same public
building blocks (`exceptions::install`, `percpu::init`, `gic::init_secondary_v3`/`enable_sgi`/`send_sgi`)
rather than calling the real entry, preserving that byte-identity.

**The legs (one variable each, measured relative to leg 10):**

| knob | variable added over the previous leg | woken-core EL/MMU | evidence |
|---|---|---|---|
| **10** | CONTROL — the exp5 park shape + the checkpoint store (no SP, no regime) | EL2, MMU off | checkpoint `0x53040000A` |
| **11** | +SP into `PROBE_STACK` + push/pop one frame (MMU-off DRAM writes) | EL2, MMU off | checkpoint `0x53040000B` |
| **12** | +regime replay: `HCR/CPTR` then `MAIR/TCR/TTBR0_EL2` (SCTLR NOT written) | EL2, MMU off | checkpoint `0x53040000C` |
| **13** | +MMU: `tlbi alle2` + `SCTLR_EL2` write (MMU ON) + isb | EL2, MMU **on** | checkpoint `0x53040000D` |
| **14** | +`exceptions::install()` (per-core EL2 vectors) | EL2, MMU on | checkpoint `0x53040000E` |
| **15** | +GICR: `this_cpu_redistributor()` + ONE `GICR_WAKER` read — **PRIME SUSPECT** | EL2, MMU on | checkpoint `0x53040000F` |
| **16** | full: +percpu + GICv3 secondary bring-up + IPI SGI (real-path replica) | EL2, MMU on | checkpoint `0x530400010` + AP→BSP SGI |

**Leg 15 is the prime suspect** (RIDER 5): the GIC-600 exposes 8 redistributor frames on a 6-core part,
and the SMP-3 fault ADDR `0x8000000000000200` smells like an MMIO window. Leg 15 is the first leg to
touch the target's GICR frame. The read is bounded to ONE `GICR_WAKER` load (the redistributor is NOT
woken — `GICR_WAKER` is never written) and the BSP **computes + prints the exact frame + `GICR_WAKER`
MMIO address BEFORE any `CPU_ON`** (via `gic::redistributor_frame_for_affinity` + `GICR_WAKER_OFFSET`),
so the prediction names the address under test. Tegra GICR base `0x0F44_0000`, 4-frame stride
`0x4_0000`; `GICR_WAKER` at `frame + 0x14` (the BSP line reports the resolved value for the target).

**Predictions (RIDER 2 — one variable per leg, written BEFORE any boot; a contradicted prediction
STOPs the sitting):** legs 10..14 all SURVIVE (their checkpoints raise, box stays up) — they replay only
per-core CPU state the SMP-2/JC2 path already exercised. Leg 15 is the expected fault (RAS power-off
before its checkpoint) if the rejected access is the GICR MMIO. Leg 16 runs LAST and only if 10..15 all
survived (otherwise the first faulting leg already named the access and 16 is SKIPPED); it is predicted
to reproduce the SMP-3 fault, closing the bracket. Leg 10 runs FIRST every sitting (RIDER 1).

**Probe-only (RIDER 4)** and **DTB-only presence (RIDER 5):** the woken core touches ONLY its own stack,
the `SEC_CTX`-named regime registers, its own GICR frame (leg 15, one read), and the checkpoint — no
fuse/persistent-state writes; the single target is the first non-BSP core from the DTB `/cpus` list
(`fdt_tegra::cpu_affinities`, cfg-widened to `any(tegrasmp, smpprobe)`), never `AFFINITY_INFO`/GICR walk.

**String-count / byte-identity note.** Like ORIN-SMP-3, armed images differ from baseline; the default
(knob-off) `esp-jetson` is byte-identical across rebuilds (`tegra:` 109; the new `gic.rs` probe helpers
are dead-code-eliminated from the default image — verified absent by `nm`). Armed values 10..16 are
distinct kernels (`UNAOS_SMPPROBE` is a compile-time const); validate an armed image by its distinct ELF
hash + the presence of `SMPPROBE-4` strings.

**Gate (this executor).** `./arroyo check` green (both arches) + `UNAOS_TEGRA=1` + `UNAOS_TEGRA=1
UNAOS_SMPPROBE=<n>` + `UNAOS_TEGRA=1 UNAOS_TEGRASMP=1`; `test-arm 22` MISSION SUCCESS; GICv3 `test-arm 40`
CAPSTONE 6/6 + 3/3 secondaries (the shared `smp_virt` path is byte-untouched); `kernel8-test` 0-FAIL
(34 PASS); `UNAOS_HUBSTORAGE` x86 MISSION SUCCESS; knob-off byte-identity proven (two default builds hash
identical, `tegra:` 109); 7 armed leg tars staged. The metal verdict is the attended bench with LC-orin
+ Peter (runbook `scripts/orin-smp4-bench.md`).

### ORIN-SMP-5 — the RESIDUE legs (`UNAOS_SMPPROBE=17..20`, knob-gated; bench Peter-attended)

The ORIN-SMP-4 sitting (2026-07-15 attended) came back **7/7 legs survived**: legs 10..15 matched
their predictions EXACTLY (leg 15's `GICR_WAKER @ 0xf460014` read survived — the prime suspect was
INNOCENT), and **leg 16's full-path replica survived AGAINST its prediction** — checkpoint
`0x53040010`, `AP -> BSP SGI OK (BSP ipi 1 -> 2)`: the first live UnaOS AP on Orin silicon. The
SMP-3 fault was therefore **NOT reproduced** by the replica. So the SMP-3 trigger lives in what leg 16
deliberately **omitted** vs the real `__secondary_rust_virt` flow. ORIN-SMP-5 adds four RESIDUE legs,
each still "leg-16 shape + exactly one restored real-path element," in the same self-contained
`smpprobe.rs` machinery (same stack / regime / checkpoint / entry-stub idiom; `smp_virt.rs` stays
byte-untouched).

**The residue legs (checkpoint tag continues the `0x5304_00<leg>` family — leg 17 = `0x53040011`):**

| knob | residue element restored over leg 16 | checkpoint | notes |
|---|---|---|---|
| **17** | +ONE `serial_println!` from the WOKEN CORE (UART MMIO + `SERIAL_PORT` console spinlock from a secondary) — the "AP online" print the bisect deliberately forbade | `0x53040011` | PRIME residue suspect. The print runs BEFORE the checkpoint store, so a rejected UART/spinlock access RAS-powers-off before `0x53040011`. The AP uses the SAME bounded-TXFF `serial_println!` path as the BSP (no new UART code); the BSP names the UARTC base `0x0C28_0000` before `CPU_ON`. |
| **18** | +the real **WFI** idle tail (the replica parks in WFE; the real secondary parks in WFI) | `0x53040012` | Checkpoint is raised BEFORE the WFI, so survival is recorded regardless; the core is not expected to return. |
| **19** | leg-16 shape on a **CLUSTER-1** core (DTB aff `0x0001_0200`) instead of `0x00000100` | `0x53040013` | Crosses a CCPLEX cluster boundary (per-cluster MCE/BPMP coordination). STOPs with no `CPU_ON` if `/cpus` does not name `0x00010200` (RIDER 5 — DTB-only presence). BSP prints the resolved cluster-1 GICR frame before `CPU_ON`. |
| **20** | the real **5-core wake SEQUENCE** — every non-BSP `/cpus` core woken leg-16-shape, IN DTB ORDER, one at a time | `0x53040014` (per core) | Runs LAST and only if 17..19 all survived. Tests whether the fault is driven by multi-core concurrency (SMP-3 woke five; the single-core legs woke one). A RAS power-off mid-sequence — the pre-`CPU_ON` line names which core was under the gun — is the verdict. |

**Predictions (RIDER 2 — written BEFORE any boot; a contradicted prediction STOPs the sitting).** The
first residue leg to RAS-power-off NAMES the residual trigger and STOPs the sitting there (that fault
IS the located wall). If all four survive, the SMP-3 fault is not reproduced by any single restored
element — pointing to timing/ordering/concurrency, a follow-up arc. Leg 17 is the leading fault
candidate (the secondary's console access). Legs 18 (WFI) is expected benign. Leg 19 (cluster-1) and
leg 20 (5-core sequence) are the two remaining fault candidates if 17 survives. Full pre-registered
prediction table + the schedule live in `scripts/orin-smp5-bench.md`.

**Gate (this executor).** `./arroyo check` green (both arches, knob-off) + `UNAOS_TEGRA=1
UNAOS_SMPPROBE=17..20` all compile; knob-off byte-identity proven (two default `esp-jetson` builds hash
identical `17bc4e7a…`, `tegra:` 109, zero `SMPPROBE-5` strings); armed images 17..20 are distinct
kernels carrying `SMPPROBE-5` strings (validate by ELF hash + `strings | grep SMPPROBE-5` + the LIVE
`sel=<n>` on the first serial line); `test-arm 22` MISSION SUCCESS; GICv3 `test-arm 40` CAPSTONE 6/6 +
3/3 secondaries (shared `smp_virt` byte-untouched); `kernel8-test` 0-FAIL (34 PASS); `UNAOS_HUBSTORAGE`
x86 MISSION SUCCESS; 4 armed leg tars + the knob-off DEFAULT staged. The metal verdict is the attended
bench with LC-orin + Peter (runbook `scripts/orin-smp5-bench.md`).

**⚡ SITTING VERDICT (2026-07-16 attended, Peter + LC-orin; serial
`~/unaos-bench/jetson-serial-2026-07-16-smp5sitting.log`; firmware precondition `t23x_general
39.2.0-gcid-45755727` asserted on every boot; all boots flashed from the staged tars, on-stick ELF
hash verified against the MANIFEST before each boot): ALL FOUR RESIDUE LEGS SURVIVED — the sitting
ran the full schedule with ZERO RAS faults.**

- **RIDER-1 re-confirm (leg 16):** survived byte-perfect — `CPU_ON ret=0` on `0x00000100`,
  `CHECKPOINT REACHED (0x53040010)`, `AP -> BSP SGI OK (BSP ipi 1 -> 2)`.
- **Leg 17 (AP print — the PRIME suspect): SURVIVED, suspect INNOCENT.** The woken core's own
  `:: … sel=17 [AP] woken core online … ::` line arrived intact on the wire, then checkpoint
  `0x53040011` + SGI OK. The secondary's console access (UART MMIO + `SERIAL_PORT` spinlock) is not
  the wall.
- **Leg 18 (WFI tail): SURVIVED as predicted** — checkpoint `0x53040012`, SGI OK, box up.
- **Leg 19 (cluster-1): SURVIVED — the first cluster-1 core ever online.** BSP named GICR frame
  `0xf500000` (`GICR_WAKER @ 0xf500014`) pre-`CPU_ON`; `ret=0` on `0x00010200`, checkpoint
  `0x53040013`, SGI OK across the cluster boundary.
- **Leg 20 (the real 5-core sequence): SEQUENCE DONE — 5/5.** All five DTB secondaries
  (`0x100`, `0x200`, `0x300`, `0x10200`, `0x10300`) each reached checkpoint `0x53040014` + SGI OK,
  in order, one boot. **Every core on the part has now run UnaOS code.**

**Verdict per the pre-registered decision table: the SMP-3 fault is reproduced by NO restored
element, singly or (leg 20) sequentially combined. The residual trigger is
timing/ordering/concurrency — or the real `_secondary_start_virt` entry shape itself — and that is
the follow-up arc's bisect target.** This is the table's "all four survive" branch: a real,
informative outcome, not a null. Bench-rig note for the record: two early boots were discarded for
capture faults on the HOST side (a baud-reset garble, then two processes splitting the serial byte
stream — kill stale readers BY DEVICE before trusting a capture); evidence starts at the clean
RIDER-1 boot. Default image restored to the boot stick at close (`17bc4e7a…`, `tegra:` 109).

### VUGFIX — reviving vug's meters on the timerless tegra EL1 (`arch::ms()` fallback + honest meter count; bench Peter-attended)

Peter's post-SMP-5 shell poke (2026-07-16) showed the `vug` demo on the Orin with two defects: blank
render **ms/fps**, flat CPU-load bars, an unthrottled spinner, and a CPU meter claiming **8 cores** on
the 6-core part. Two independent root causes, both tegra-only.

**Root cause A — timerless EL1 freezes `arch::ms()`.** By design the Orin's EL2→EL1 drop disables the
physical timer (`CNTP_CTL=0`, JD3; `timer::set_not_live`), so the timer IRQ never fires at EL1 and
`timer::ticks()` is frozen at 0. `arch::ms()` was `ticks()*4` → stuck at 0, so `vug`'s 200 ms readout
window (`dt >= 200`) never elapsed (the fps/ms readout, the load fraction, AND the demo-core pulse
fallback all sit behind that window) and the render loop had no pacing. `setdate`/JD17 is a *separate*
clock service and was correctly disproven as the culprit.

*Fix:* a `tegra`-gated `ms()` fallback. When the timer is not live (`!timer::is_live()`, always the
case on the current tegra EL1 drop), derive ms from the free-running virtual counter,
`CNTVCT_EL0 / (CNTFRQ_EL0/1000)` — the SAME interrupt-flag-independent timebase `now_cycles()` already
uses to bound hardware busy-waits, so **no new hardware is touched**. `CNTFRQ==0` guards to 0. When the
timer *is* live the heartbeat form (`ticks()*4`) is kept so the two paths agree. With `ms()` alive the
existing 200 ms window, fps/ms readout, load fraction and demo-core fallback all revive on their own —
no `vug` render-loop rework.

**Root cause B — the meter DISPLAYED the array-headroom bound, not the real core count.**
`sched::meter_cpu_count()` returned `percpu::NUM_CPUS`, which is `8` under `tegra` (JM5 sized the
per-CPU array to the part's 8 GICR frames + headroom). The Orin Nano's DTB `/cpus` names **6**
Cortex-A78AE cores (the hard-won SMP-2/SMP-3 oracle: GICR frames and `AFFINITY_INFO` both over-count;
only `/cpus` is truthful).

*Fix:* a `tegra`-gated `percpu::METER_CPU_COUNT = 6` that `meter_cpu_count()` DISPLAYS on tegra;
`NUM_CPUS` stays the array bound everywhere (`meter_cpu_ticks` still caps at it; 6 ≤ 8 keeps every read
in range). It is a **compile-time** count, not a runtime `/cpus` walk, because the default tegra boot is
single-core and performs no runtime enumeration (that lives only on the `tegrasmp` probe path;
`smp_virt::N_CORES_PUB` is 0 on the default boot), and threading the DTB pointer to the meter read path
would need an out-of-lane boot-path change (`main.rs`) — outside this arc's lane.

**Byte-identity rule (the load-bearing constraint).** The whole change is `tegra`-gated AND
line-count-preserving in the shared files (both `cfg` folds live on a single source line; the new const
is appended at EOF), specifically so the pi/virt binaries — and their serial-log addresses — stay
byte-identical. Proven: the pi `kernel8.img` (objcopy flat) hashes **identical** base-vs-HEAD; the virt
`test-arm` kernel's raw `.text`/`.rodata`/`.data` and its full objcopy `-O binary` loadable image hash
**identical** base-vs-HEAD (the full ELF differs only in non-loaded DWARF `.debug_*`, which records the
changed source text and shifts later section file offsets — it does not reach the running kernel).

**Gate (this executor).** `./arroyo check` green (both arches, knob-off AND `UNAOS_TEGRA=1`);
`test-arm 22` MISSION SUCCESS; GICv3 `test-arm 40` CAPSTONE 6/6 + 3/3 secondaries; `kernel8-test`
0-FAIL (34 PASS); `UNAOS_HUBSTORAGE` x86 MISSION SUCCESS; `esp-jetson` links, `tegra:` 109 (unchanged —
no new serial prints). QEMU cannot model the tegra timerless drop, so the verdict is the attended Orin
bench (LC-orin + Peter): expected metal outcome = `vug` shows live fps/ms/load, the CPU meter reads
**6** cores, and the bars move.
### ORIN-SMP-6 — the LAST-DIFFERENCES legs (`UNAOS_SMPPROBE=21..23`, knob-gated; bench Peter-attended)

The ORIN-SMP-5 sitting (2026-07-16 attended) acquitted every RESIDUE element — AP serial print (17),
WFI tail (18), cluster-1 bring-up (19), and the serialized 5-core sequence (20; 5/5 online, every
core on the part has run UnaOS) — while SMP-3 (the real `tegrasmp` bring-up) still RAS-faulted ×2
(IOB SERR=0x12 / CBB-0x6 / ADDR `0x8000000000000200`) BEFORE the first `CPU_ON` result printed.
Exactly **two differences** remain between everything acquitted and the faulting SMP-3 flow, and
ORIN-SMP-6 takes them one variable per leg:

1. the **REAL `_secondary_start_virt` entry** — the real stub code + the real per-CPU
   `SECONDARY_STACKS` slot + the real `__secondary_rust_virt` (MMU replay, MPIDR-derived index,
   AP-online print, `CORE_READY`, AP→BSP ping, WFI park) — vs the probe's replica stub;
2. **RAPID-FIRE wake concurrency** — SMP-3 issues all five `CPU_ON`s in a tight loop — vs leg-20's
   park-before-next serialization.

**Lane amendment (Maestro-granted 2026-07-16).** Feeding the real entry requires the real path's
private publication state, so `smp_virt.rs` gained EXACTLY one `smpprobe`-gated (plus `tegra`)
publish-only API + one read accessor — `probe_publish_real_path(aff_by_index) -> entry_pa`
(the exact `start_secondaries_tegra` pre-`CPU_ON` publication: BSP affinity + SGI-0 enable, EL2 ctx
capture + clean-to-PoC, `SECONDARY_STACKS` clean+invalidate, `AFF_BY_INDEX`/`N_CORES_PUB` Release;
**no `CPU_ON` inside**) and `probe_core_online(idx)` (the `CORE_READY` Acquire read). Both are
compiled out knob-off — the default image stays byte-identical to baseline (re-proven this arc).

**The legs (leg-22 slots carry the core index in byte 1 — `0x5304_0116..0x5304_0516`):**

| knob | the ONE variable | evidence channel | notes |
|---|---|---|---|
| **21** | REAL entry × ONE core (`0x00000100`, ctxid 1) | the real path's own signals: the AP's `:: AARCH64 SMP: AP 1 online … ::` print (EXPECTED — this leg is deliberately not probe-silent), `CORE_READY[1]`, AP→BSP SGI | SMP-3 died before core 1's `CPU_ON` result printed — this leg alone may name the wall. Publication via the granted API; every address printed BSP-side pre-`CPU_ON`. |
| **22** | RAPID 5-core burst × the known-safe replica stub (`_smpprobe_leg22`: leg-16 shape + per-core stacks `PROBE_STACKS6` + per-core checkpoint slots) | per-core slots `0x5304_01xx16` polled AFTER the burst | The full plan (every target/entry/slot value) prints BEFORE the first `CPU_ON`; the burst itself is print-free (back-to-back, faster than SMP-3's own print-per-call loop). Pure concurrency on acquitted code. |
| **23** | REAL entry × RAPID 5-core — SMP-3 replayed under instrumentation | `CORE_READY[1..5]` polled after the print-free burst; AP-online prints expected ×5 | Runs LAST, only if 21 AND 22 both survived on the bench (runbook-gated; the image is always staged). |

**Predictions (RIDER 2 — pre-registered in `scripts/orin-smp6-bench.md` BEFORE any boot).** The 2×2
of {entry shape} × {concurrency} is fully covered by legs 20 (serialized/stub, survived), 21, 22, 23:
whichever leg faults FIRST names the trigger axis; if 21 and 22 both survive and 23 faults, the wall
is the *conjunction*; if all three survive, SMP-3's fault is not reproduced under instrumentation at
all (a boot-state/ordering delta vs the `tegrasmp` flow — e.g. the kick-off's surrounding context —
becomes the target). A leg that faults where survival was predicted = STOP the sitting.

**Gate (this executor).** `./arroyo check` green (both arches, knob-off) + `UNAOS_TEGRA=1
UNAOS_SMPPROBE=21/22/23` all compile; knob-off byte-identity re-proven post-amendment (two default
`esp-jetson` builds hash identical, zero `SMPPROBE-6` strings knob-off); `test-arm 22` MISSION
SUCCESS; GICv3 `test-arm 40` CAPSTONE 6/6 + 3/3 secondaries (the no-behavior-change proof for the
`smp_virt.rs` amendment); `kernel8-test` 0-FAIL; `UNAOS_HUBSTORAGE` x86 MISSION SUCCESS; 3 armed leg
tars + the knob-off DEFAULT staged. The metal verdict is the attended bench with LC-orin + Peter
(runbook `scripts/orin-smp6-bench.md`).

**⚡ SITTING VERDICT (2026-07-16 attended, Peter + LC-orin; serial
`~/unaos-bench/jetson-serial-2026-07-16-smp6sitting.log`, 11 boots; firmware
`39.2.0-gcid-45755727` asserted per boot; every image hash-verified on-stick pre-boot):**

- **RIDER-1 leg-16 re-confirm: PASS** (checkpoint `0x53040010`, SGI OK).
- **Leg 21 (REAL entry × one core): SURVIVED — the real entry shape is INNOCENT.** First run of the
  production secondary path on Orin silicon: `CPU_ON ret=0` → the genuine `_secondary_start_virt` →
  full `__secondary_rust_virt` → the real path's own `:: AARCH64 SMP: AP 1 online (aff=0x00000100) ::`
  → `CORE_READY[1]` → AP→BSP SGI OK.
- **Leg 22 (rapid 5-core burst × stub): SURVIVED — pure wake concurrency is INNOCENT.** Five
  back-to-back `CPU_ON`s (all ret=0, box up through the print-free burst), all five per-core slots
  reached (`0x53040116..0x53040516`), `RAPID SEQUENCE DONE — 5/5`.
- **Leg 23 (REAL entry × rapid 5-core — the SMP-3 replay): UNANSWERED — blocked by a NEW, distinct
  wall.** The leg-23 IMAGE faulted **4-for-4** in ordinary EARLY BOOT, before the probe ever
  dispatched (no `sel=23` line on any attempt).
- **NEW WALL, characterized (its own arc, not SMP):** RAS SNOC `SERR=0xd` "Illegal address
  (software fault)" + IERR Carveout Uncorrectable `0x3`, paired ACI `SERR=0x4` IERR FillWrite `0x9`;
  **fixed fault ADDR `0x800000027767dc80`** (once `+0x200`) — beyond the 8 GiB DRAM top — fires at
  the xHCI `JB9i` inherited-slot-eviction step (`DISABLE_SLOT 1..8` drained is the last line).
  **Image-layout-correlated:** leg-23 image 4/4 faults; leg-21 and leg-22 images 1/2 each (clean on
  retry); leg-16 image 0/1; 0/19 boots across all prior sittings. **Keyboard EXONERATED** (fault
  reproduced with nothing on the bus but the boot stick). Echoes the Pi CORE3 build-layout
  correlation, in xHCI clothing: something layout-dependent decides whether the takeover-phase DMA
  hits the carveout, with a deterministic bad address when it does.

**SMP conclusion: the SMP-3 discrimination space is down to (a) the CONJUNCTION (real entry × rapid
5-core — leg 23's still-open question) or (b) boot-state context around the real `tegrasmp`
kick-off. Every single-variable suspect is now acquitted on silicon. The immediate next experiment
is the XHCI-carveout arc's relink test (a rebuilt leg-23 layout likely boots — then leg 23 answers
in one boot).** Power-fault boots were pre-registered data; five DC-cut recoveries; nothing
improvised.

### JETSON-XCARVE — the xHCI-takeover carveout wall (diagnosis arc; `UNAOS_XCARVE` / `UNAOS_XCARVE_RELINK`)

The ORIN-SMP-6 sitting surfaced a wall UNRELATED to SMP: on the no-HCRST inherit path some kernel
images RAS-power-off the box in ordinary early boot, during the xHCI inherited-state takeover, and
WHETHER an image faults is decided by its build LAYOUT, not by what it does. This arc is **diagnosis
only** — it adds instrumentation and a relink experiment; no fix ships here.

**The wall (do not re-derive — 11 attended boots):** RAS SNOC `SERR=0xd` "Illegal address (software
fault)" + IERR Carveout Uncorrectable `0x3`, paired ACI `SERR=0x4` IERR FillWrite `0x9`; **FIXED fault
ADDR `0x800000027767dc80`** (once `+0x200`); fires right after `JB9i — inherited-slot eviction:
DISABLE_SLOT 1..8 issued + drained`. Image-layout-correlated: leg-23 image 4/4, leg-21/22 ~50%, 0/19
across all prior sittings. Keyboard EXONERATED (boot stick alone on the bus).

**What the address's shape says.** `0x800000027767dc80` decomposes as a 64-bit pointer whose HIGH dword
is `0x80000000` and LOW dword is `0x7767dc80`. The low part alone (`0x27767dc80`, ~9.86 GiB) sits INSIDE
the Orin DRAM window `[0x8000_0000, 0x2_8000_0000)` (2–10 GiB PA), near its top — i.e. a plausible-looking
firmware DRAM address with bit 63 (the hi-dword's bit 31) SPURIOUSLY set. That is the fingerprint of a
64-bit xHCI pointer assembled from a poisoned/flagged high half: the controller issues a FillWrite to
what it thinks is a real structure, the top bit shoves it past the 40-bit PA space, and the platform's
carveout guard kills the box as an illegal address. Because the no-HCRST takeover (JB9g) preserves the
firmware's live xHCI state, the eviction's `DISABLE_SLOT` drain rides one of these inherited pointers.

> **⚠ SUPERSESSION (2026-07-16, R19 erratum research — the paragraph above is retained as the
> sitting-era reading; do not cite it as current):** external Tegra234 RAS records refute the
> bit-63-poisoned-pointer premise — **every published Tegra234 RAS ADDR carries the
> `0x80000000_xxxxxxxx` prefix** (NVIDIA's own XUSB-carveout bug records, edk2-nvidia issue #111:
> `0x8000000472f02820`/`0x8000000272f02820`; IOB records `0x8000000003270000`,
> `0x8000000003f300c0`). Bit 63 is a **record-format artifact** (address-valid flag class), not a
> pointer high half. Current reading: the fault ADDR low bits `0x2_7767_dc80/…dc40` sit in
> carveout territory near the 8 GiB DRAM top, adjacent to the Orin NX XUSB-FW carveout in the
> #111 record, and the signature pair (SNOC Illegal-address `0xd` + Carveout `0x3` + ACI
> FillWrite `0x9`) is the platform's canonical "protected carveout was touched" record. The
> in-DRAM census/scrub null results stand unchanged; the poisoned state remains
> controller/firmware-internal. Full evidence + decode:
> `~/.claude/plans/unaos/review/unaos-orin-erratum-DRAFT.md` §3–§4.5; reported upstream as
> NVIDIA forum topic 377113.

**M2 — the inherited-pointer instrumentation (`UNAOS_XCARVE=1`).** `jb2b_attach` gained a
`#[cfg(feature = "xcarve")]` census, `jbxc_inherited_dump`, fired at attach entry BEFORE the takeover
reprograms DCBAAP/CRCR/ERST and BEFORE JB9i — so it snapshots every pointer the firmware left, and
prints even on a boot that then faults. It dumps, with a stable `JBXC:` prefix and raw 64-bit hex: the
op regs (USBCMD/USBSTS/CONFIG), DCBAAP/CRCR/ERSTBA/ERDP, every ERST entry (ring base + size), the DCBAA
base, DCBAA[0] = the scratchpad-buffer-array pointer + each scratchpad entry, and each nonzero DCBAA
per-slot device-context pointer + that slot's context state. Reads are pure CPU loads of identity-mapped,
dead-but-intact post-EBS RAM (the class JB9f already dereferences); the fault is a controller DMA WRITE,
never a CPU load, so reading is safe. Every dereference is plausibility-guarded (nonzero, `< 1<<40`);
a value that FAILS the guard — exactly the bit-63 pointer we hunt — is still PRINTED, never chased.
**Each print discriminates:** whichever line carries hi-half `0x80000000` / lo-half `0x7767dc80` names
the FillWrite's pointer class — an inherited per-slot device-context pointer (the eviction's context
write rides a poisoned firmware slot pointer), a scratchpad array/entry pointer, the inherited event
ring (ERSTBA/ring base/ERDP — a `DISABLE_SLOT` completion writeback the controller latched pre-takeover),
or CRCR (command ring). **Two census-coverage caveats (review lens, fold before the sitting so a null
Boot-1 result is read correctly):** (1) per xHCI 5.4.5 a CRCR *read* returns the Command Ring Pointer
field as ZEROS (only status bits read back) — the `JBXC: CRCR=` line cannot surface an inherited
command-ring pointer, so a command-ring culprit falls into the controller-internal bucket below, not a
named `JBXC:` line; (2) the census reads each device context's SLOT context only — the endpoint
contexts that follow it (and the transfer-ring dequeue pointers inside them) are NOT walked, so a
poisoned TR/endpoint pointer would also surface only as a null census. If NO dumped pointer carries
the hi-half, the target is a controller-INTERNAL latched pointer, an inherited command-ring pointer
(unreadable via CRCR), or an unwalked endpoint-context/TR pointer — still a finding that steers the
fix (the fix step should also widen the plausibility guard's lower bound to the DRAM base
`0x8000_0000`, lens note 2a, so a sub-DRAM inherited value is never CPU-dereferenced into the MMIO
aperture). Knob-off the whole census +
its call site vanish (byte-identical; zero `JBXC` strings; proven by two default `esp-jetson` builds
hashing equal).

**M1 — the relink experiment (`UNAOS_XCARVE_RELINK=1`).** A `#[used]` (never GC'd) inert read-only pad
static in its own `.xcarve_relink_pad` section (16 KiB of `0xA5`, never read) shifts every downstream
section, symbol, and the image's own load extent with ZERO semantic change — the takeover code is byte
identical. Measured deltas (default vs relinked-default): `.text` VMA `0x2c000 → 0x30000`, `.data`
`0xcbc80 → 0xcfc80`, `.bss` `0xd3000 → 0xd7000` — the whole image moves by `0x4000`; the pad appears
between `.rodata` and `.text` (leg-23 image shifts identically: `.text 0x2f000 → 0x33000`, `.bss
0xde000 → 0xe2000`). `.text` also grew 12 bytes on the shift — a linker veneer/`--fix-cortex-a53-843419`
artifact of moved addresses, not a code change, and immaterial to the experiment (which needs only a
layout move). Rebuilding the 4/4-faulting leg-23 image with this on tests the correlation decisively: if
the fault VANISHES or MOVES, layout correlation is proven on one image AND leg 23 unblocks (its SMP-3
replay answers in the same clean boot). Knob-off the pad vanishes (byte-identical to baseline).

**M3 — proposed fix direction (NOT implemented; its own reviewed step).** Ranked by what the diagnosis
would license: **(a)** once M2 names the pointer class, SCRUB that specific inherited structure before
JB9i — e.g. zero DCBAA[0] / re-point the scratchpad array to a valid heap page, or zero the inherited
DCBAA slot entries for 1..8 so the `DISABLE_SLOT` drain has no poisoned context pointer to write through,
or normalize the latched ERST/ERDP (clear the bit-63 hi-half) — the least-mutating write that removes the
carveout target while leaving the live Falcon untouched. **(b)** A full HCRST would clear all inherited
pointers but JB9f/JB9e proved it kills the Falcon's service loop on this firmware — NOT viable on the
inherit path. **(c)** If the relink proves pure layout correlation, a layout-pin is a fragile last
resort (it hides the poisoned pointer rather than neutralizing it); (a) is preferred. The DONE gate for
THIS arc is the diagnosis (M1 + M2) + this direction; the fix lands separately once a sitting names the
pointer. Bench runbook: `scripts/orin-xcarve-bench.md` (predictions pre-registered).

**⚡ SITTING VERDICT (2026-07-16 attended, Peter + LC-orin; serial
`~/unaos-bench/jetson-serial-2026-07-16-xcarvesitting.log`; 4 boots, media hash-verified on-stick
per boot; firmware `39.2.0-gcid-45755727`):**

- **Boot 1 (instrumented, faulted = full-value):** the `JBXC:` census printed completely
  pre-eviction, then the boot died at the wall. **NULL CENSUS** — no inherited pointer carries the
  poisoned hi-half; every dumped value sane in-DRAM (~`0x2_5f2a_x000` region): DCBAA, 3 scratchpads,
  ERST/event ring, 8 firmware slots all slot_state=3 (Configured). CRCR read `0x0` exactly per the
  5.4.5 caveat. Per the folded coverage caveats the target is THREE-WAY AMBIGUOUS: controller-internal
  latched pointer, inherited command-ring pointer (unreadable via CRCR), or an unwalked
  endpoint-context/transfer-ring pointer. **NEW FACT: the fault ADDR MOVED — `0x800000027767dc40`
  (−0x40, one context stride) on this new-layout image vs the SMP-6 era's fixed `…dc80`.**
- **Boot 2 (relinked leg-23): CLEAN through JB9i where the old layout faulted 4/4** — then the full
  leg-23 conjunction ran: 5 back-to-back real-entry `CPU_ON`s, **5/5 cores online via the REAL path**
  (both clusters, `CORE_READY[1..5]`, AP→BSP SGI OK, `RAPID REAL-ENTRY SEQUENCE DONE`), EL1 drop,
  CAPSTONE 6/6 PASS, panel live (~145–160 fps observed).
- **Boot 3 (same stick, repro sample): FAULTED at JB9i, ADDR `…dc40`** — the relink turned the
  4/4-faulter into a **~50%er**, the sibling-image pattern. **LAYOUT CORRELATION PROVEN IN BOTH
  DIMENSIONS: layout modulates the fault PROBABILITY (4/4 → ~50%) and the fault TARGET ADDRESS
  (`dc80` on all old-layout images, `dc40` on both new-layout images).** The relink does NOT
  eliminate the wall — layout-pin (M3c) is confirmed non-viable; the fix must neutralize the pointer.
- **Boot 4 (restored knob-off default `83035a8f…`): clean boot, CAPSTONE 6/6; VUG METAL WITNESS
  PASS** — live climbing fps (145→160+), meter reads 6 cores, `VUG: crystal live — 24 faces`.
  ⚠ Recorded artifact: ALL 6 load bars read pinned — parked cores (2..5, never scheduled on the
  default image) accrue no idle ticks so a busy/idle meter reads them 100%. Display-honesty
  follow-up candidate (show parked cores idle/absent), not a VUGFIX regression (fps/ms chain is the
  witness and it is live). End-restore debt paid within the sitting.

**LEG 23 VERDICT (SMP-3 close-out): the conjunction is INNOCENT.** Every code-shape suspect —
entry shape (leg 21), wake concurrency (leg 22), their conjunction (leg 23) — is now acquitted on
silicon. **SMP-3's residual trigger = boot-state context** (whatever differed on the original
SMP-3 boots: e.g. the tegrasmp kick-off's position in the boot sequence, xHCI/eviction state at
wake time — note SMP-3's fault signature was IOB `ADDR 0x8000000000000200`, ALSO a bit-63 address,
now suggestive in the carveout wall's light). Next SMP arc = a boot-state-context bisect
(proposal to Peter; do not spawn).

---

#### FIX arc — census v2 (endpoint contexts) + aimed neutralization (`UNAOS_XCARVE_SCRUB`)

The sitting's NULL census left the FillWrite target three-way ambiguous: controller-internal latched
pointer, inherited command-ring pointer (unreadable via CRCR per xHCI 5.4.5), or an **unwalked
endpoint-context / transfer-ring pointer** (the diagnosis census read slot contexts only). The FIX arc
attacks the two DRAM-visible arms of that ambiguity and disambiguates the third.

**Census v2 (`UNAOS_XCARVE=1`, unchanged feature).** `jbxc_inherited_dump` now walks each Configured
slot's ENDPOINT CONTEXTS — the class the sitting left dark. For a slot whose device-context pointer is
plausible it reads the slot context's Context Entries (dword0[31:27] = last valid endpoint DCI), then
for each DCI 1..=ctx_entries prints the endpoint context's EP State (dword0[2:0]), EP Type (dword1[5:3]),
and the raw 64-bit **TR Dequeue Pointer** (ep-ctx offset 0x08), flagging any TRDeq that fails the DRAM
window as `IMPLAUSIBLE`. The endpoint-context stride is the controller's real context size — **CSZ
(HCCPARAMS1 bit 2): 64 bytes when set, else 32** (`jbxc_ctx_size`) — a wrong stride would mis-address
every endpoint context. The plausibility guard's **lower bound is widened to the DRAM base
`0x8000_0000`** (lens note 2a; window now `[0x8000_0000, 0x2_8000_0000)`), so a sub-DRAM inherited value
is never CPU-dereferenced into the MMIO aperture — printed raw, never chased. A faulting census-v2 boot
now NAMES or exonerates the TR class on the same log.

**The aimed neutralization (`UNAOS_XCARVE_SCRUB=1`, new feature `xcarve_scrub ⇒ tegra`, default OFF).**
`jbxc_scrub_inherited` runs at the top of `jb2b_attach` right after the census — pre-takeover (so it
reads the firmware's live inherited DCBAAP/structures, not our replacements) and pre-JB9i (so the
`DISABLE_SLOT` drain finds no poisoned pointer to ride). It walks the same inherited DRAM structures and
rewrites **ONLY** the values that fail the plausibility test — bit-63 set, or outside the DRAM window:
- a poisoned DCBAA[0] scratchpad-array pointer → zero the DCBAA[0] entry; poisoned scratchpad page
  pointers → zeroed in place;
- a poisoned per-slot device-context pointer → zero that DCBAA slot entry (the `DISABLE_SLOT` drain then
  has no poisoned context to write through; that slot's endpoint contexts are not walked off a bad base);
- for a slot with a SANE device-context base, a poisoned endpoint-context TR dequeue pointer → zero the
  whole TRDeq qword (pointer + DCS).

Rules held exactly: **(a)** only values that fail the plausibility test are ever rewritten — a sane
pointer is NEVER touched (a poisoned base is recorded and left unwalked, never dereferenced); **(b)**
every rewrite prints a `JBXC-SCRUB:` line naming what, where (address), and the old raw value; **(c)** if
nothing is poisoned the scrub is a one-line no-op; **(d)** the Falcon stays untouched — the scrub writes
DRAM structures ONLY, never HCRST, never CSB, never any controller-control register (weakening the
takeover would be a STOP).

**The no-op discriminates the last bucket.** Because the scrub fires only on provably-invalid DRAM
values, if the poison is controller-INTERNAL (a latched pointer or the CRCR-unreadable command ring —
nothing visible in DRAM) the scrub **no-ops and the wall persists**. That is not a failed arc: it is the
result that CONFIRMS the controller-internal / command-ring bucket and steers the follow-up (which would
need a different lever than a DRAM scrub — e.g. a controlled command-ring reset that does not kill the
Falcon). Conversely, a `JBXC-SCRUB:` line followed by a clean boot through JB9i is the FIXED verdict and
names the exact structure that carried the poison.

Knob-off both features vanish (byte-identical to baseline; zero `JBXC` / `JBXC-SCRUB` strings; proven by
two default `esp-jetson` builds hashing equal). Bench legs pre-registered in `scripts/orin-xcarve-bench.md`.

**⚡ FIX-SITTING VERDICT (2026-07-16 attended, Peter + LC-orin; serial
`~/unaos-bench/jetson-serial-2026-07-16-xcarvefix-sitting.log`; 5 boots — census v2 boot A + 4
boots of the census+scrub-leg23 image; media hash-verified on-stick per boot; 0 RAS faults in the
whole sitting):**

- **Boot A (census v2, default layout): the TRANSFER-RING ARM IS EXONERATED.** All ~30 nonzero
  TR dequeue pointers across the 8 inherited Configured slots are sane in-DRAM (`0x2_5f19–5f2f`
  region); the only `IMPLAUSIBLE`-flagged entries are NULL (unused endpoints — a census
  display-noise nit, see note below). Clean boot, CAPSTONE 6/6.
- **Boots B1/B2 + 2 extra cuts (census+scrub on the historically-4/4 leg-23 knobs): scrub NO-OP
  4/4** ("no poisoned inherited DRAM value found"), then a clean JB9i passage EVERY boot — **0
  faults in 4 boots on this (third distinct) layout.** The confirming no-op-then-fault sample
  never arrived, so the verdict is stated at its honest strength: **every DRAM-visible inherited
  class is eliminated by direct observation (exhaustive census + 4× corroborating no-ops); the
  FillWrite target is controller-INTERNAL or the CRCR-unreadable command ring, BY ELIMINATION.**
  The DRAM-scrub lever, though verified safe on silicon, cannot reach this wall — the next fix
  proposal must target controller-side state (e.g. quiesce/normalize the inherited command ring
  by programming CRCR before the eviction, or a controller-internal-state investigation), and
  goes to Peter as a proposal.
- **Layout correlation, third angle:** three distinct layouts of the same leg-23 knobs now
  sample 4/4 (original), ~50% (relink), and 0/4 (census+scrub build) fault rates — reinforcing
  that layout decides exposure while the poisoned value itself lives outside DRAM view.
- **Incidental SMP replication:** leg 23 (real entry × rapid 5-core) ran on all 4 scrub-image
  boots — 5/5 cores online via the real path + CAPSTONE 6/6 every time. With the prior sitting,
  the SMP-3 conjunction's innocence is now replicated ×5 on silicon.
- **Census firmware-determinism confirmed:** the inherited-pointer census is bit-identical
  across all 5 boots except ERDP (the firmware event-ring dequeue advances a few entries per
  boot — expected).
- **Nit for the next probe touch (non-blocking):** the census's `IMPLAUSIBLE` flag does not
  special-case a NULL TRDeq (unused endpoint) and so flags 13 zeros per dump; the scrub
  correctly treats NULL as not-poisoned (`raw != 0` short-circuit). Cosmetic only.

---

#### CRCR-QUIESCE arc — the command-ring re-seat (`UNAOS_XCARVE_CRCRQ`)

The FIX-sitting eliminated every DRAM-visible inherited class by direct observation (exhaustive
census + 4× corroborating scrub no-ops): the FillWrite target is controller-INTERNAL or the
CRCR-unreadable command ring, by elimination. This arc attacks that last bucket with a
controller-side lever that stays inside the Falcon-safety invariant.

**The mechanism (xHCI 1.2 §5.4.5 / §4.6.1.2).** The no-HCRST takeover (JB9g) preserves the
firmware's live xHCI state. Among that state is the command ring's INTERNAL fetch pointer / dequeue
latch — the one xHCI structure whose pointer field a CRCR *read* returns as zeros (§5.4.5), so no
`JBXC:` census line can name it. The trap is a specific spec rule: **the controller loads the
Command Ring Pointer field of CRCR ONLY when the ring is stopped (CRR=0); while CRR=1 a CRCR write
moves only the CA/CS status bits and the pointer field is ignored.** **REVIEW-LENS CORRECTION
(spec + census, folded before any bench):** per §5.4.5 CRR is CLEARED whenever the controller
halts (HCH=1) — a halted controller CANNOT read CRR=1 — and both prior sittings' censuses read raw
`CRCR=0x0` at HCH=1, which (CRR = bit 3, readable) directly observes **CRR=0** on this exact
inherited state. So `init_pointers`' halted-time CRCR write DOES latch, the dropped-write theory is
REFUTED a priori, and the CA branch below is expected DEAD on this path (`CRR-before=0` every
boot). **The lever's honest value is as a CONFIRMING PROBE:** it closes the command-ring bucket by
silicon observation instead of spec argument (zero cost knob-off), and its re-seat guarantees no
un-re-seated-ring window regardless. It is NOT the likely fix; if the wall persists past it, the
target is controller-internal beyond the command ring (final bucket).

**M1 — the quiesce + re-seat (`jbxc_crcrq_quiesce`, feature `xcarve_crcrq ⇒ tegra`, default OFF).**
In `jb2b_attach`, placed AFTER the takeover programs the rings and `start()`s the controller but
BEFORE the JB9i `DISABLE_SLOT` loop — provably before the first command doorbell (`start()` rings no
doorbell; JB9i is the first). Sequence, in spec order, under a stable `JBXC-CRCRQ:` prefix:
1. Read `CRCR.CRR` (bit 3). Print `CRR-before`.
2. If `CRR=1` (ring inherited-live): issue Command Abort as ONE 64-bit write carrying OUR ring
   pointer + RCS + `CA` (bit 2) — the valid-pointer compose avoids the pre-2021 Linux null-dequeue
   abort bug (`ff0e50d3564f`) if the ring stops mid-write. Poll `CRR → 0` bounded (~200 ms); print
   the outcome (`CRR-after`, or a flagged `CRR STILL 1` timeout).
3. WHETHER `CRR` was 1 or already 0, program `CRCR = our_ring | RCS` — now that `CRR=0` the pointer
   field is actually loaded. If `init_pointers`' write already took (CRR was 0), this is an
   idempotent re-write. This leaves **NO window** where a command is issued on an un-re-seated ring.

**Falcon-safety argument (the invariant is absolute).** CA (Command Abort) is a command-ring
control op ONLY — it aborts the command ring and touches nothing else. The lever issues **no HCRST,
no CSB write, nothing that stops the Falcon service loop** (the class JB9f/JB9e proved fatal on this
firmware). The controller's own enumeration recovery (`abort_enum_command`, drivers/xhci/mod.rs)
already issues this exact CA handshake on this silicon, so CA is proven non-fatal here. The only
registers written are `CRCR` (op + 0x18) — the command-ring control register — never a Falcon /
CSB / reset register.

**M2 — event-ring re-seat ordering audit (no window; documented, no code).** The census showed the
firmware ERDP advancing per boot, raising the question of whether the controller could write back a
stale event to the FIRMWARE event ring after takeover. Traced ordering in `jb2b_attach`: JB9g halts
the controller (`RS=0`); THEN the unsafe block runs `init_interrupter` (ERSTBA/ERDP/ERSTSZ ← OUR
event ring), `init_pointers` (CRCR), `start()` (`RS=1`) — **all event-ring reprogramming happens
while the controller is halted (RS=0)**. A halted controller generates no events and performs no
event writeback; ERSTBA is freely writable (no CRR-style stop-gate). By the time `RS=1`, ERSTBA/ERDP
already point at our ring, so the controller never runs with the firmware's event ring. **No window
exists — the event-ring re-seat provably precedes the first RS=1.** No code change; if a future
reorder ever moved `RS=1` ahead of `init_interrupter`, closing the window would be its own flagged,
knob-gated change (not folded into M1).

**Knob-off** the quiesce + its call site vanish (byte-identical to baseline; zero `JBXC-CRCRQ`
strings; proven by two default `esp-jetson` builds hashing equal). Bench legs pre-registered in
`scripts/orin-xcarve-bench.md`.

**Bench pre-registration (the fork).** The decisive testbed is the historically-4/4-faulting leg-23
knobs (`UNAOS_SMPPROBE=23` era) PLUS `UNAOS_XCARVE_CRCRQ=1` (+ census `UNAOS_XCARVE=1` so the boot
documents itself). ⚠ **Disclosure:** adding the quiesce code CHANGES the image layout — the exact
4/4 leg-23 layout cannot be byte-preserved (this is the fourth distinct layout of those knobs; the
prior three sampled 4/4, ~50%, 0/4). The fault-rate comparison is therefore statistical; the
`JBXC-CRCRQ:` lines are the mechanism witness either way. Pre-registered outcomes (ordered by
PREDICTED likelihood after the review-lens correction above):
- **`CRR-before=0` every boot (THE PREDICTED RESULT)** ⇒ confirms on silicon what spec + census
  already say: the inherited ring was not running, init_pointers' CRCR write already took, the
  command-ring bucket CLOSES by observation. The re-seat still leaves no window; if the wall
  persists on other boots, the target is controller-internal beyond the command ring (final
  bucket).
- **quiesce lines + fault persists at JB9i** ⇒ same conclusion with the fault sampled in-window —
  command ring exonerated, controller-internal confirmed.
- **`CRR-before=1` + CA + clean boots ×3+** ⇒ would CONTRADICT §5.4.5 on this silicon (halted
  controller reading CRR=1) — treat as a silicon-erratum finding AND a strong FIXED signal;
  record exactly, both halves matter.

---

### ORIN-SMP-7 — the BOOT-STATE-CONTEXT bisect (`UNAOS_SMPPROBE=24..27`, knob-gated; bench Peter-attended)

§ORIN-SMP-6 + the XCARVE leg-23 close-out acquitted every code-shape suspect for the SMP-3 wall on
silicon — entry shape (leg 21), wake concurrency (leg 22), their conjunction (leg 23, replicated ×5).
What remains is BOOT-STATE CONTEXT: the original SMP-3 fault (IOB `SERR=0x12` / CBB-`0x6`, ADDR
`0x8000000000000200` — a bit-63 address, suggestive next to the XCARVE carveout wall's bit-63
`0x800000027767dc80`) fired from the real `UNAOS_TEGRASMP=1` kick-off at its production position,
while every surviving probe ran from the `smpprobe` dispatch point. This arc bisects WHERE in the boot
the wake happens — specifically, relative to the JB2b `jb2b_attach` xHCI takeover / JB9i
inherited-slot eviction (the XCARVE-suspect step, and the interplay the bit-63 echo suggests).

**The boot-ordering audit (the leg→position mapping, with code evidence).** In `tegra_early_stop`
(`main.rs`) the relevant order is: JB1c BPMP ungate + JB5/JB9f raw-handoff witnesses → JM4 (GIC-600 +
generic timer, `percpu::init(0)`) → global heap → **JB2b `jb2b_attach` xHCI takeover (incl. JB9i
eviction)** → the `smpprobe::run` dispatch (`main.rs`) → the `tegrasmp` `start_secondaries_tegra`
kick-off → JM6. Two facts fall out of that audit and shape the legs:

1. **The `smpprobe::run` dispatch (leg 23's site) and the `start_secondaries_tegra` kick-off are
   adjacent and BOTH post-xHCI-takeover** — nothing executable sits between them. So position is NOT
   what differed between leg 23 and the real SMP-3 run; and the code delta is also ~nil (leg 23's
   `run_real_entry_rapid` publishes via `smp_virt::probe_publish_real_path`, the line-for-line twin of
   `start_secondaries_tegra`'s pre-`CPU_ON` publication, then bursts the same `CPU_ON`s into the same
   real `_secondary_start_virt`; the only difference — a print-free burst vs SMP-3's print-per-call
   loop — was itself acquitted by legs 20/22). The residual enumerable delta reduces to the build
   FEATURE / image LAYOUT (`tegrasmp` vs `smpprobe`), which the XCARVE arc proved decides fabric
   exposure — the through-line to pursue.
2. **The one boot-state variable that is both in-lane and cleanly togglable is the wake's position
   relative to the xHCI takeover.** The earliest point at which the real-entry rapid wake can run
   (GIC + timer + SMC + serial all live, `percpu::init(0)` done) is immediately after JM4 + the heap,
   which is BEFORE `jb2b_attach`. So a second, PRE-takeover dispatch site (`smpprobe::run_pre_xhci`,
   placed right after `:: KERNEL HEAP ALLOCATED ::`) brackets the takeover against the existing
   post-takeover site, with ZERO `xusb_tegra.rs` touch.

**The legs (one variable per leg; pre-registered predictions; probe-only).** Legs 24 and 25 run the
IDENTICAL wake code (`run_real_entry_rapid`, the leg-23 real-entry × rapid 5-core path — publication
via the granted `probe_publish_real_path`, real `_secondary_start_virt`, `CORE_READY` online signal);
they differ ONLY in dispatch POSITION, so the pair isolates exactly one variable — the
takeover/eviction fabric state at wake time.

| knob | the ONE variable (position) | dispatch site | prediction |
|---|---|---|---|
| **24** | REAL entry × rapid 5-core at the **POST-xHCI-takeover** position (leg-23's site) — the REPRO CONTROL | `smpprobe::run`, after `jb2b_attach` | Per the boot-state hypothesis: FAULT (IOB `…0200`). Given leg 23's ×5 innocence at this exact position, the likely actual is **SURVIVE** — then the bisect INVERTS and the finding is the leg-24-vs-real-SMP-3 delta (audit point 1: image layout / build feature). |
| **25** | the SAME wake at the **PRE-xHCI-takeover** position — before `jb2b_attach`/JB9i eviction | `smpprobe::run_pre_xhci`, after JM4+heap | If leg 24 FAULTS and 25 SURVIVES → the takeover/eviction fabric state IS the trigger (the wall is created by the xHCI takeover, and a wake into the pre-takeover fabric is clean). If BOTH survive → the takeover-state axis is also acquitted; the residual is the build-layout delta (audit point 1), a non-probe follow-up. |

**Legs 26/27 — FOLDED (documented, not built).** Leg 26 ("wake immediately AFTER JB9i eviction, before
the rest of `jb2b_attach`") would need an instrumentation hook INSIDE `jb2b_attach` (`xusb_tegra.rs` —
another executor's file) → **OUT OF LANE, flagged to LC-orin**; and leg 24 (post-FULL-takeover) already
brackets the post-eviction fabric, so the coarse before/after bracket (25 vs 24) captures the
takeover-state question without it. Leg 27 ("full production post-wake path") is **DEGENERATE**: after
any probe leg returns, the BSP already proceeds to the JM6 EL1 drop + CAPSTONE with the woken APs parked
in WFI — exactly the real SMP-3 post-wake path (leg 23/24's APs run the real `__secondary_rust_virt` to
its WFI park). Both fold-reasons print a self-documenting line if 26/27 are ever armed; three legs (24,
25, and the acquitted-baseline leg 23 re-run) suffice. See `scripts/orin-smp7-bench.md`.

**Byte-identity note.** Knob-off, the whole `smpprobe` module and BOTH `main.rs` dispatch calls
(`run` and the new `run_pre_xhci`) are `#[cfg(feature = "smpprobe")]`-compiled-out; zero `SMPPROBE-7`
strings, `tegra:` count 109. The default tegra kernel's `.text` / `.rodata` / `.data` are BIT-identical
base-vs-HEAD; the loadable image differs by exactly **one byte** — a panic `Location` line-number
constant in `.data.rel.ro`, shifted by the new (compiled-out) `run_pre_xhci` dispatch site in `main.rs`
(the unavoidable source-line-number consequence of any new main.rs call site, cf. the SMP-3 kick-off
addition; no behavioral effect, running code unchanged). The armed images 24/25 are distinct kernels
carrying `SMPPROBE-7` strings; validate by ELF hash + `strings | grep SMPPROBE-7` + the live `sel=<n>`
on the first serial line.

**Gate (this executor).** `./arroyo check` green (both arches, knob-off) + `UNAOS_TEGRA=1
UNAOS_SMPPROBE=24/25` compile; `test-arm 22` MISSION SUCCESS; GICv3 `test-arm 40` CAPSTONE 6/6 + 3/3
secondaries (the shared `smp_virt.rs` path is byte-untouched — the arc only READS via the SMP-6-granted
`probe_publish_real_path`/`probe_core_online`); `kernel8-test` CAPSTONE COMPLETE (0 FAIL); x86 `test 25`
MISSION SUCCESS; knob-off `.text/.rodata/.data` bit-identical + the 1-byte `.data.rel.ro` note above; 2
armed leg tars + the knob-off DEFAULT staged. The metal verdict is the attended Orin bench with LC-orin
+ Peter (runbook `scripts/orin-smp7-bench.md`).

**⚡ CRCR+SMP-7 SITTING VERDICT (2026-07-16 attended, Peter + LC-orin; serial
`~/unaos-bench/jetson-serial-2026-07-16-crcr-smp7-sitting.log`; 6 boots / DC-cut recoveries;
media hash-verified on-stick per boot):**

- **CRCR probe (crcrq-leg23, 2 boots): `CRR-before=0` both boots — the predicted value — AND the
  boot FAULTED IN THE QUIESCE WINDOW BOTH TIMES, deterministically:** the fault (`…dc40`, same
  SNOC/ACI signature) landed exactly between the `CRR-before` print and the re-seat confirmation
  print, i.e. at the **CRCR write issued while RS=1** — the FIRST pre-JB9i fault ever observed,
  2/2 reproducible. **MECHANISM NAMED: engaging the command-ring machinery on the running
  inherited controller is the trigger** — a CRCR write (no command, no doorbell) suffices; the
  historical JB9i-time faults were the DOORBELL engaging the same machinery two steps later.
  Model: firmware leaves a controller-internal latch poisoned (or not) per boot; the first
  command-machinery engagement fires the FillWrite to the fixed address. **The quiesce lever is
  REFUTED as a fix in the strongest form — it fires the wall EARLIER.** Boot rule of thumb until
  fixed: do not write CRCR at RS=1 on the inherit path. The command-ring bucket is CLOSED
  (CRR-before=0 observed ×2); the target is controller-internal state engaged by command-ring
  operations. Next step (proposal, not spawned): firmware/NVIDIA-erratum investigation of the
  XUSB context-save/internal-latch behavior, and/or ordering levers that avoid RS=1 CRCR writes.
- **SMP-7 leg 24 (post-takeover control): SURVIVED** (5/5 real-path cores + CAPSTONE; first boot
  of the image was taken by the carveout wall pre-probe — retry cleared, wall data not SMP data).
- **SMP-7 leg 25 (PRE-takeover wake): SURVIVED** — 5/5 real-path cores online BEFORE
  `jb2b_attach` ever ran. **The position axis is exonerated end-to-end**; with legs 21-25 all
  innocent, **SMP-3's residual trigger = IMAGE LAYOUT** (the XCARVE through-line). Next
  experiment (proposal): relink the real `tegrasmp` image (the SMP-3 fault build) and re-bench.
  Bonus datum: the wall then took that same boot at JB9i with 5 secondaries online — signature
  unchanged (`…dc40`), the wall and live APs are independent.
- Wall fault-rate tally on the new-layout images this sitting: crcrq-leg23 2/2 (both in-window at
  the CRCR write), smpprobe24 1/2 (JB9i-time), smpprobe25 1/1 (JB9i-time, post-verdict).

**Post-sitting addendum (same evening):** the freshly-staged folded-default layout (`63e90f8`
knob-off) proved to be a WALL-FAULTER on metal — 2/2 boots RAS'd at JB9i (`…dc40`), presenting on
the panel as a "lockup" (the RAS power-off freezes the scanout). Wall data, not a regression: the
fourth-through-sixth new layouts built today have now ALL sampled the wall. Stick default
therefore restored to the metal-validated `cad623af…` (`d3ecf48` era, 20+ clean boots), verified
to shell on metal; the folded-default tar is flagged in the bench MANIFEST as a faulter, unfit
for stick-default duty. Standing rule until the wall is fixed: a stick DEFAULT must be a layout
with a clean multi-boot metal record — QEMU-green + fresh layout is not sufficient for the
default slot.

**⚡ NVIDIA-facing reproducer (R20, 2026-07-17, Peter's direct ask):** NVIDIA asked, on the
upstream forum report (topic 377113), for a way to reproduce this fault on their own bench.
`tools/orin-xhci-repro/` is a standalone `#![no_std]` UEFI Rust app (own Cargo workspace, no
UnaOS tooling required to build or run) that reads USBCMD/USBSTS/CONFIG/CRCR, confirms RS=1,
then — gated behind an operator keypress and a loud warning banner — issues exactly one
content-free echo write to CRCR (read the register, write the same value back), tracing the
exact trigger this sitting named above: a CRCR write while RS=1. Builds clean for
`aarch64-unknown-uefi`; QEMU cannot model the fault, so the actual reproduction is an
attended metal run, not exercised in this repo's CI. Draft forum reply:
`~/.claude/plans/unaos/review/unaos-orin-repro-REPLY.md` (Peter's call whether/when to post
and whether to offer the reproducer source). Landing checklist:
`~/.claude/plans/unaos/review/unaos-orin-repro-LANDING.md`. (Correction of record: the
"echo write" described above was the original design; the independent review lens changed the
one gated write to a synthetic, non-null, 64-byte-aligned pointer with RCS=1 —
`0x2_0000_0000 | 1` — matching the shape of the kernel's validated trigger write, since an
echo would load a NULL ring pointer per xHCI §5.4.5, an untested variant. `a412320`.)

**⚡ ATTENDED RESULT (2026-07-17, Peter, 26+ runs): NO REPRODUCTION.** The standalone
reproducer was run 26+ times across BOTH boot sticks (new official + old) and never fired the
fault. This is the pre-registered non-repro branch of the landing report, and it is a finding,
not a tool failure. Two hypotheses remain, unseparated:
1. **The bare CRCR write on the untouched UEFI-inherited controller is NOT the sufficient
   trigger** — the kernel's fuller takeover sequence (halt, reprogram DCBAAP/ERST/CONFIG,
   restart RS, THEN the quiesce write) does something necessary to reach the poisoned
   condition that a single direct register write does not.
2. **No boot in the window was poisoned** (the box-state-over-time axis) — the session had no
   positive control: a known wall-faulter kernel layout was not booted in the same window to
   confirm the box was in a faulting state at all. Precedent: the July-15 2/2-deterministic
   IOB faulter binary went 3/3 clean the next day unchanged.
Discriminating experiment (when a bench window opens): pair boots in ONE session — the known
wall-faulter layout vs. the reproducer, interleaved. Faulter fires + reproducer doesn't ⇒
hypothesis 1 confirmed (revise the reproducer toward the full takeover sequence). Neither
fires ⇒ hypothesis 2 (box not poisoned; result inconclusive, retry another day). **The
staged forum reply (`unaos-orin-repro-REPLY.md`) must NOT be posted as-is** — its reproducer
section describes a tool now known not to reproduce at n=26+; per the landing report's own
pre-registration, the reply's trigger framing needs revision first. The repo copy of the
tool's README was removed from the working tree (Peter, 2026-07-17) to avoid automated-scanner
friction; the tool itself stays cataloged here.

**⚡⚡ SAME-WINDOW DISCRIMINATION (2026-07-17 R21 attended sitting — the hypothesis-1 answer):**
serial of record `~/unaos-bench/jetson-serial-2026-07-17-r21-sitting.log`. The fresh merged-main
default candidate (`af1af39`, kernel `80475a57…`, tegra:109) went 2 clean boots (boot A fully
captured: CAPSTONE 6/6, zero RAS, **VUG-HONESTY parked-core witness PASS on silicon**; boot B
attested-unlogged) then **wall-faulted on boot 3** at USB enumeration — the exact carveout
signature (SNOC `0xec00030d`/SERR `0xd`/Carveout `0x3` + ACI FillWrite `0x9`, ADDR `…dc40`),
panel "lockup" = the RAS power-off freezing scanout. Candidate therefore **DISQUALIFIED for
default duty** (defaults rule); stick restored to validated `cad623af…` and the restore **proven
to shell** (third captured boot, CAPSTONE 6/6, clean). Then the decisive leg: **the standalone
reproducer ran 13-for-13 SILENT minutes after the kernel fault, same box, same window** (28
pre/post write lines on serial), on top of the morning's 26-run silence. **Verdict: hypothesis 1
CONFIRMED — the bare register write is NOT the sufficient trigger; the kernel's fuller takeover
sequence is required to reach the poisoned condition.** Hypothesis 2 (box not poisoned) is dead
for this window — the kernel path faulted minutes earlier. Consequences: (a) the NVIDIA reply
revision is UNBLOCKED with this framing (tool = honest negative + the takeover sequence as the
real trigger; Peter posts); (b) the wall remains live on current firmware; (c) the af1af39-era
layout joins the faulter ledger (~1-in-3 observed, small sample).

**⚡⚡ R21 CONSOLIDATED SITTING (2026-07-17 night; serial
`~/unaos-bench/jetson-serial-2026-07-17-r21b-sitting.log`; merged main `3338d55` media):**
- **ORIN-SMP-DEFAULT candidate (pure default build, kernel `922ab1ce…`): 1 fault / 2 clean.**
  Boot 1 fired the SMP-3-class IOB record (`0x…0200`, CBB `0x6` + ACI FillWrite) at the
  tegrasmp kick-off — the historical SMP-3 signature re-sampled by the new layout,
  probabilistic not deterministic here. Boots 2 and 3: **5/5 secondaries online on BOTH
  clusters, SGIs clean, VUG-HONESTY witness PASS, CAPSTONE COMPLETE, interactive shell — the
  first default-path 6-core UnaOS shells on Orin silicon.** Candidate NOT promoted (defaults
  rule: the record must be fault-free); stick default remains `cad623af…` (restored at close,
  content-verified). The SMP code itself is exonerated again — the kick-off fault is the
  known layout/box-state residual, now observed probabilistic on this layout.
- **RAST-TEGRA: FIRST 3D PIXELS ON THE ORIN PANEL** — witness build (kernel `b41989d0…`)
  rendered the cube on the real 1920×1200 inherited scanout: `90 frames in 91 ms — 989.010
  fps` (attended visual confirm: the animation completes in ~0.1 s and presents as a blue
  flash/square — frame PACING is a follow-up nit, the render itself is proven; contrast x86's
  21.9 fps per-pixel present). CAPSTONE COMPLETE same boot, zero faults.
- **ORIN-NET-1 census on real firmware: 8 Tegra234 PCIe controllers found and walked
  read-only** (controller 0 `pcie@140a0000` domain 8 status=okay with full
  appl/config/atu_dma/dbi/ecam reg map + ranges captured; controllers 1–7 enumerated; full
  detail in the serial log). The NET-2 scoping data now exists on the record; fold into
  §ORIN-NET-1's metal columns at the next docs pass on the merged tree.

### ORIN-SMP-8 — the tegrasmp RELINK (the layout-axis close-out; BUILD-ONLY, `UNAOS_TEGRASMP` + `UNAOS_XCARVE_RELINK`)

ORIN-SMP-7's attended sitting exonerated the wake POSITION axis end-to-end (legs 24/25 both put 5/5
real-path cores online, pre- and post-xHCI-takeover). With legs 21–25 all innocent on silicon —
entry shape (21), wake concurrency (22), their conjunction (23, replicated ×5), post-takeover
position (24), pre-takeover position (25) — **the SMP-3 discrimination space is EMPTY except one
axis: IMAGE LAYOUT.** That is the XCARVE through-line: the same knob-set rebuilt at three distinct
layouts sampled 4/4, ~50%, and 0/4 carveout-fault rates, with the fault ADDRESS itself moving with
layout (`…dc80` on old-layout images, `…dc40` on new-layout images). The only enumerable variable
left standing between the surviving leg images and the original `tegrasmp` image (which RAS-faulted
2/2 on 2026-07-15) is the build itself — feature set → image layout.

**The experiment (BUILD-ONLY — no new kernel surface).** The XCARVE relink pad
(`XCARVE_RELINK_PAD`, a `#[used]` 16 KiB inert `0xA5` static in its own `.xcarve_relink_pad` section,
`arch/aarch64/xusb_tegra.rs`) is composed onto the REAL `UNAOS_TEGRASMP=1` image — the exact SMP-3
kick-off (`smp_virt::start_secondaries_tegra`, the real 6-core Orin bring-up) at a shifted layout.
Both features imply `tegra`; `arroyo` composes `tegrasmp,tegra,xcarve_relink,tegra` and cargo
de-dups (compose verified: `./arroyo check` green both arches for `UNAOS_TEGRASMP=1` and
`UNAOS_TEGRASMP=1 UNAOS_XCARVE_RELINK=1`; feature-echo confirms both active). No code changed — the
pad and both features already exist from prior arcs; this arc is docs + runbook + staging only.

**Compose + layout evidence (`llvm-objdump -h`, tegrasmp-original vs tegrasmp-relinked; both built
this arc, the original is the bench control).** The pad shifts the whole image by exactly +0x4000
with zero semantic change (the takeover + SMP code is byte-for-byte identical):

| section | tegrasmp-original VMA | tegrasmp-relinked VMA | delta |
|---|---|---|---|
| `.rodata` | `0x3850` | `0x3850` | (unchanged — precedes the pad) |
| `.xcarve_relink_pad` | (absent) | `0x11e40` (size `0x4000`) | new, between `.rodata` and `.text` |
| `.text` | `0x2c000` | `0x30000` | +0x4000 (size `0x7addc` IDENTICAL — no veneer growth this build) |
| `.data.rel.ro` | `0xb6de0` | `0xbade0` | +0x4000 |
| `.data` | `0xca040` | `0xce040` | +0x4000 |
| `.bss` | `0xd1000` | `0xd5000` | +0x4000 |

Strings witnesses: both images carry the `AARCH64 SMP: ORIN-SMP-3` markers (11 each) and `tegra:`
count 109; zero `JBXC` strings (no census on either); `xcarve_relink_pad` section present only in the
relinked image. Both builds are deterministic (rebuild reproduced the ELF hashes exactly). ELF SHAs:
tegrasmp-original `510869fd…`, tegrasmp-relinked `095b9251…` (full SHAs in the bench MANIFEST).

**The two-signature discrimination (the crux of the sitting).** Two independent walls can take a
boot on these images; the runbook demands the operator read the RAS ADDR to tell them apart — only
one answers SMP-8:

| signature | RAS ADDR | class / register set | where it fires | means |
|---|---|---|---|---|
| **SMP-3 fault** (the axis under test) | ends `…0200` (`0x8000000000000200`) | IOB `SERR=0x12` / CBB-`0x6` | at `start_secondaries_tegra`, BEFORE the first `CPU_ON` result prints | the SMP-3 wall — the ONLY signature that answers this arc |
| **SNOC-Carveout / xHCI wall** (unrelated to SMP) | ends `…7767dcXX` (`…dc40`/`…dc80`) | SNOC `SERR=0xd` Illegal-address + Carveout `0x3`, ACI `SERR=0x4` FillWrite `0x9` | at the JB9i inherited-slot eviction (`DISABLE_SLOT 1..8 … drained`) | the xHCI-takeover carveout wall — WALL DATA, retry (may take any boot pre-probe) |

**Pre-registered predictions (verbatim in `scripts/orin-smp8-bench.md`, written BEFORE any boot).**
Boot A = tegrasmp-relinked ×2–3: a **clean boot to 5 real APs online** (`:: AARCH64 SMP: AP <n>
online … ::` ×5 + CAPSTONE 6/6, panel live) ⇒ **SMP-3 trigger = LAYOUT, CONFIRMED** — the production
6-core SMP path is CODE-COMPLETE and the SMP arc closes pending the carveout wall's real fix. A
`…0200` fault at the kick-off instead ⇒ **layout REFUTED for this image** — the first-ever refutation
of the layout axis (equally decisive: the SMP-3 trigger survives a +0x4000 relink, so it is not
purely layout — record exactly, re-open the residual, STOP and report). Boot B (optional) =
tegrasmp-original ×1: expect the historical SMP-3 fault (`…0200`) as the control that the wall still
reproduces at the original layout (a SURVIVE is also data — it would recast the 2026-07-15 2/2 as a
sample of a probabilistic layout-modulated wall, the XCARVE pattern). Either image may ALSO sample
the carveout wall (`…7767dcXX` at JB9i) pre-probe on any boot — wall data, retry.

**Staged media (three tars to `~/unaos-bench/flash/orin/`, MANIFEST re-hashed).**
`tegrasmp-relinked` (the layout-axis test image), `tegrasmp-original` (the 2026-07-15 fault-repro
control — flagged in the MANIFEST as EXPECTED to RAS at the SMP kick-off), and a `knoboff-default`
reference (byte-identity fallback — two default `esp-jetson` builds hash equal; **NOT a stick-default
candidate** per the ORIN-SMP-7-addendum standing rule: no clean multi-boot metal record, the stick
default stays `cad623af…`/`d3ecf48`). Gates green: `./arroyo check` both arches × {knob-off,
TEGRASMP, TEGRASMP+RELINK}; knob-off byte-identity re-proven; full `arroyo battery` GREEN (x86 `test
25` MISSION, arm virt v2 MISSION, GICv3 CAPSTONE 6/6 + 3/3 secondaries, `kernel8-test` 0-FAIL,
esp-jetson links); `UNAOS_HUBSTORAGE` x86 MISSION SUCCESS. Bench runbook: `scripts/orin-smp8-bench.md`.

**⚡⚡ SMP-8 SITTING VERDICT (2026-07-16 attended, Peter + LC-orin; serial
`~/unaos-bench/jetson-serial-2026-07-16-smp8-sitting.log`; 8 boots, 0 faults, both walls silent):**

- **Relinked tegrasmp (2/2 CLEAN): the production SMP path is CODE-COMPLETE on Orin silicon** —
  full boots to 5 real APs online + CAPSTONE 6/6 + live 6-core shell, twice.
- **Today-original-layout tegrasmp (1/1 clean):** the pre-registered fault prediction VIOLATED
  (STOP recorded) — but today's "original knobs" is itself a new layout at today's base.
- **THE TRUE CONTROL — the LITERAL July-15 2/2-faulting binary (`915249a6…`): 3/3 CLEAN today**,
  on the new stick ×2 AND the original shared stick ×1. **The layout axis is REFUTED as the
  sufficient explanation, and the stick/device axis is REFUTED too: the fault-determining state
  is BOX-STATE OVER TIME** (persistent firmware/fabric state that changed between 2026-07-15 and
  tonight — candidates: cumulative DC-cut history, firmware-internal latches; NOT image, NOT
  device, NOT code). This also retro-reads the carveout wall's "layout correlation": layout
  modulated the odds within a session, but the underlying poisoned-latch state is temporal.
- **SMP-3 CLOSES:** the fault was never the SMP code (legs 21–25 + tonight's 8 = every axis
  acquitted); the production 6-core bring-up runs end-to-end. The residual mystery (what box
  state loaded the dice on July-15) merges into the xHCI-wall/firmware-erratum investigation —
  one combined proposal to Peter, not two.
- Sitting-4 media provenance: relinked `095b9251…` / today-original `510869fd…` / July-15 binary
  `915249a6…` all hash-verified on-stick per boot; new official Orin stick commissioned mid-
  sitting (full ESP prep from staged tar; no more rMBP sharing). ⚠ Process note (cost one wasted
  boot): a mid-sitting "old stick" control ran on the NEW stick — caught by CONTENT-signature
  identity check (old stick carries HELLO.BIN/hello.txt), re-run correctly; media identity
  checks by content, not by operator memory, are now part of the flash discipline.
- **Bolt-1 attended window: SATISFIED, GREEN** (same evening, host-side): STATUS honest-AMBER →
  dry-run reviewed → attended `--apply` (SNAP-first) → all 5 mirrors coherent, penumbra 235/0/0,
  narino untouched. The reframed UnaFS-NATIVE vaire arc-2 proposal is UNBLOCKED (per the R18
  baton: UnaFS objects on the K-line machinery + NSSPAN-pattern per-phase ticks so the first
  native sync IS the baseline FS benchmark).

### ORIN-NET-1 — read-only PCIe root-complex + NIC recon (`UNAOS_PCIEPROBE`, knob-gated; census-before-touch)

Orin has no network path. The Jetson Orin Nano devkit's NIC sits behind the Tegra234 PCIe root
complex, so networking begins with knowing **exactly what the firmware (NVIDIA UEFI / L4T 39.2.0)
left us** at ExitBootServices — the SMP-2-style read-only census that scopes the real bring-up chain
(PCIe RC → NIC → smoltcp, already in-tree). This arc writes NOTHING to fabric or config space beyond
what reading requires: no BAR writes, no bus-master/command-register writes, no link retraining, no
power-domain state changes, and **no new page-table mapping** (a mapping is a write — the STOP
tripwire). The wall (JETSON-XCARVE) taught this track to census before it touches.

All code lives in `arch/aarch64/pcie_probe.rs` behind the `pcieprobe` cargo feature
(`UNAOS_PCIEPROBE=1`). The feature is **standalone** (it does NOT imply `tegra`, mirroring the
`smpprobe` pattern) so the same census can run on BOTH the metal tegra path and the QEMU `virt` GICv3
path — the metal build combines it with `UNAOS_TEGRA=1`. Knob-off, the module and its two call sites
vanish and every image (tegra AND virt) is **byte-identical to baseline** (verified: knob-off tegra
`esp-jetson` kernel ELF has zero `PCIE:` strings, `tegra:` count unchanged at 109, and the same-path
rebuild against base `45a06b2` hashes equal).

**Two layers, in strict order of trust.**

1. **DTB census (ALWAYS).** Walk the firmware's own device tree READ-ONLY (the same bounded
   big-endian token scan `fdt_tegra::Fdt`/`for_each_prop` already uses — a malformed blob degrades to
   a printed "not found", never a fault) and dump every `pcie@` controller: `compatible`,
   `device_type`, `status`, `reg`/`reg-names`, `ranges`, `interrupts`/`interrupt-map` (presence +
   size), `num-lanes`/`phy-names`, `power-domains`, `linux,pci-domain`. This alone names which
   controllers exist, which the firmware left **ENABLED** (`status = "okay"`), and — from the RC's
   `ranges`/child nodes — where the NIC lives. It is the deliverable's spine and is zero-MMIO-risk.

2. **Config-space liveness read (GATED, conservative).** ONLY for a controller the firmware left
   ENABLED (`status = "okay"`) AND whose config/appl aperture (`reg-names` = `"config"`, else
   `"dbi"`/`"appl"`) resolves **inside the already-mapped GiB-0 Device-nGnRE window** (`mmu_tegra`
   maps GiB 0 + RAM; it does NOT map the high Tegra234 PCIe config apertures). No new mapping is made.
   The read decodes vendor/device, class/subclass/prog-if/rev, header type, and BAR0..5 — all
   read-only, no config write. If the aperture is out of the mapped window (the expected Tegra234
   case — its `config` regions live high, e.g. the C-controller `0x2e…` range), the probe **records
   the blocker and leaves that controller un-walked**: NET-2 must map it Device-nGnRE first. A partial
   map with honest gaps beats a touched fabric.

**The poison-rejection rule (PI-V3D-1, cited in the probe's liveness comments).** PI-V3D-1's attended
Pi sitting found the V3D core block never decoded — every read returned the firmware's `0xdeadbeef`
fill — yet the probe's liveness gate FALSE-PASSED it (it treated the non-zero word as "present"). A
gate that only rejects zero is not a liveness gate. Here `is_poison()` rejects BOTH `0xffffffff` (the
PCIe master-abort / unclaimed-config return) AND `0xdeadbeef` (firmware fill): either is **ABSENT
DECODE, never "present"**. A live config space returns a plausible vendor id (not `0x0000`, not
`0xffff`) whose word is not a poison fill.

**Read-only invariant (the arc's review lens).** Every access is a `read_volatile` or a DTB byte
read — no `write_volatile`, no config/BAR/command write, no `SET_*` MRQ, no `CPU_ON`, no link
retrain. The DTB `status` is the enable oracle, gating the only MMIO the arc would ever touch to a
block the firmware itself declares enabled (the JX1 lesson: a gated Tegra block is an EL3-fatal touch,
unguardable — so we touch only `status = "okay"` apertures that are already mapped).

**Call sites (two, both `pcieprobe`-gated).**
- `tegra_early_stop` (main.rs), just after `:: KERNEL HEAP ALLOCATED ::` and before any JB2b xHCI work
  (the census is PCIe-only, independent of XUSB) — the **metal Orin census**.
- The virt GICv3 `is_v3()` block (main.rs), after `smp_virt::start_secondaries` and before the EL2→EL1
  drop — the **QEMU graceful-skip witness**.

**QEMU vs metal — the honesty boundary.** QEMU `virt` has no Tegra234 root complex (a generic
`pci-host-ecam-generic` at most), and on the GICv3 divergence path the UEFI handoff leaves
`boot_info.dtb_addr = 0`. So the census's only observable QEMU effect is the graceful-skip line, then
CAPSTONE completes unchanged:

```
:: PCIE: ORIN-NET-1 read-only PCIe/NIC census (DTB @0x0 size=0x0) ::
:: PCIE: no DTB handed off — census SKIPPED (graceful) ::
```

Everything past the DTB census (the config-space liveness read, the NIC identity/BAR/link-state map)
is **ATTENDED-METAL-PENDING**: the Orin devkit's live DTB is only present on real silicon. The census
is staged as a tar for an attended sitting (`scripts/orin-net1-bench.md`); the map's device columns
(which controller hosts the NIC, its vendor/device/class, link state as-left-by-firmware, the SCOPED
NET-2 chain) fill in from that boot.

**Gates green.** `./arroyo check` both arches × {knob-off, `PCIEPROBE`, `PCIEPROBE`+`TEGRA`}; knob-off
byte-identity re-proven (same-path rebuild vs base `45a06b2` hashes equal; zero `PCIE:` strings;
`tegra:` 109 unchanged); `test-arm 22` MISSION SUCCESS; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6 (both
knob-off AND knob-on — the knob-on run witnesses the graceful skip); `kernel8-test 35` 0-FAIL (29
PASS); `esp-jetson` links. Bench runbook: `scripts/orin-net1-bench.md`.

### ORIN-NET-2 — controller-0 link + device enumeration (`UNAOS_PCIE2`, knob-gated; the last recon before a driver)

NET-1's census named **controller 0** (`/bus@0/pcie@140a0000`, domain 8) firmware-ENABLED with a full
`appl|config|atu_dma|dbi|ecam` reg map, then read the downstream `config` window (`0x2a00_0000`) and got
`0xffffffff` — an ABSENT DECODE. NET-2 answers the two questions that scope the real driver arc (NET-3):
**is the link up, and WHAT DEVICE is behind it** (the NIC hypothesis). It is read-mostly recon: the ONLY
writes it performs are kernel page-table mappings; no fabric/config/BAR writes, no link retraining, no
BAR sizing, no device programming — with poison-rejecting liveness on every read.

All code extends `arch/aarch64/pcie_probe.rs` behind the `pcie2` cargo feature (`UNAOS_PCIE2=1`),
standalone (does NOT imply `tegra`, mirroring `pcieprobe`) so the recon runs on BOTH the metal tegra path
(`+UNAOS_TEGRA=1`) and the QEMU `virt` GICv3 witness. Two knob-gated call sites (`census2`) sit exactly
where NET-1's do: `tegra_early_stop` (metal, after the heap + MMU, before the JB2b xHCI work) and the
virt GICv3 block (witness). Knob-off, the module + both sites vanish (see the byte-identity note below).

**The NET-1 lesson corrected — read link state from DBI, not the downstream window.** On the DesignWare
Tegra234 root complex the downstream `config` window routes to the first downstream bus via the
controller's internal ATU and returns all-Fs when the link is down (or the CFG ATU region is unset) —
which is exactly what NET-1 saw. The **root port's own identity and link state do not live there**; they
live in the **`dbi`** aperture (`0x2a08_0000`), the RP's local config space, valid regardless of link
state. So NET-2 reads link state from DBI: it walks the RP capability list to the **PCIe capability**
(id `0x10`) and decodes **Link Status** — negotiated speed/width and the **Data-Link-Layer-Link-Active**
bit — the definitive "is the link up" answer, plus Link Capabilities (max speed/width) and the RP's
vendor/device/class/header-type. If DLL-active (link up), it then reads one level below (bus1:dev0:fn0
through the already-mapped `config` window), poison-rejecting, decoding vendor/device/class/header-type
and BAR0..5 **raw** (sizes reported UNKNOWN — the BAR-sizing write ritual is NET-3 territory).

**M1 — the Device-nGnRE MMIO mapper, and the PS-ceiling finding.** `mmu_tegra::map_mmio_window(pa, size)`
reaches an aperture for a read-only walk via the EXISTING kernel page-table path — the same L1-block
mechanism `map_fb_region` uses, but Device-nGnRE and idempotent against the already-mapped peripheral
windows, patching BOTH the live EL2 `L1` and the EL1-precise twin so the window survives a JM6 drop. It
returns `AlreadyMapped` / `Mapped` / `BeyondPsCeiling`. Applied to controller 0's apertures (metal reg
values from the r21b sitting):

| region | base | in GiB-0 device window? | reach |
|---|---|---|---|
| `appl` | `0x140a_0000` | yes | AlreadyMapped |
| `config` | `0x2a00_0000` | yes | AlreadyMapped |
| `atu_dma` | `0x2a04_0000` | yes | AlreadyMapped |
| `dbi` | `0x2a08_0000` | yes | AlreadyMapped |
| `ecam` | `0x2e_2000_0000` (Tegra234, 256 MiB) | **no — ~184 GiB** | **BeyondPsCeiling** |

The scoped NET-2 walk (RP link state via `dbi` + one level below via `config`) needs **no new mapping** —
all four low apertures fall inside the GiB-0 Device window `L1[0]` already maps. The **ECAM** whole-domain
enumeration window, and the MMIO `ranges` (`0x32_/0x35_…`, ~200–213 GiB), live **above the tegra regime's
36-bit PS output ceiling** (`TCR_EL2_VAL` PS = `0b001` = 64 GiB). A block descriptor there raises an
address-size fault, and reaching it needs a **TCR_EL2.PS widen to 40-bit** — a translation-regime change
**beyond a page-table write**. Per the arc's STOP tripwire, `map_mmio_window` **refuses** that (returns
`BeyondPsCeiling`, no descriptor written) and records it as the concrete NET-3 prerequisite rather than
performing it. So the mapper's writes-permitted budget is a ceiling, not a requirement: on controller 0
it installs **zero** new descriptors (low apertures already mapped, ECAM refused) — an honest, in-scope
outcome.

**The poison-rejection rule (PI-V3D-1)** carries over unchanged: `is_poison()` rejects both `0xffffffff`
and `0xdeadbeef` on every DBI/config read; an absent RP decode is a STOP-record (RAS-safe: no further
touch), never a false "present".

**Read-only invariant (the arc's review lens).** Every access is a `read_volatile` (DBI/config space) or
a DTB byte read; the only writes are the Device-nGnRE page-table descriptors `map_mmio_window` would
install (and on controller 0 it installs none). No config/BAR/command write, no `SET_*` MRQ, no link
retrain, no BAR sizing. Any RAS/SError, or any step that would need a write beyond kernel page tables
(the PS widen), is a STOP-record + leave-un-walked, not a workaround.

**QEMU vs metal — the honesty boundary.** On the `virt` GICv3 path the UEFI handoff leaves
`boot_info.dtb_addr = 0`, so `census2`'s only observable QEMU effect is the graceful-skip line, then
CAPSTONE completes unchanged (witnessed):

```
:: PCIE2: ORIN-NET-2 controller-0 link + device recon (DTB @0x0 size=0x0) ::
:: PCIE2: no DTB handed off — recon SKIPPED (graceful) ::
```

Everything past the DTB scope (the DBI link-status read, the RP identity, the device below) is
**ATTENDED-METAL-PENDING** — controller 0's live registers exist only on real Orin silicon, and NET-1's
metal evidence (`config` = all-Fs) strongly predicts the link is **DOWN** as-left-by-firmware, i.e. the
expected metal verdict is "link down, RP = ⟨vendor/device⟩, no device enumerable below." The recon is
staged as a tar for an attended sitting (`scripts/orin-net2-bench.md`).

**The NET-3 scope this implies.** (1) **Widen the tegra translation regime's PS to 40-bit** so the ECAM
(`0x2e_2000_0000`) and the MMIO `ranges` (~200 GiB) become mappable — the prerequisite for any
multi-bus enumeration or BAR assignment. (2) If the link is down, **bring it up / retrain** (appl + PHY
programming, LTSSM) — the first *write* to the fabric, which this read-only arc does not do. (3) Then
enumerate, size BARs, and bind a driver.

**Byte-identity (knob-off) — the ratified 1-byte Location class, objcopy-verified + disclosed.** Knob-off
the `pcie2` module + both call sites are compiled out, but the two gated `#[cfg]` blocks in `main.rs`
occupy source lines, shifting the `#[track_caller]` Location line number of the `jb2b_attach` call below
them by +24 (the count of inserted lines). Verified per-section against baseline `922ab1ce` (knob-off
`esp-jetson kernel.elf`): `.text`, `.rodata`, `.data`, `.got` are **byte-identical**; `.data.rel.ro`
differs by **exactly one byte** — a single `core::panic::Location` `line` field, `1378 → 1402`
(low byte `0x62 → 0x7a`). The rest of the ELF delta is non-loaded DWARF line info. The **loadable,
executable image is unchanged** but for that one Location literal; `tegra:` count 109, zero `PCIE2`
strings knob-off.

**Gates green.** `./arroyo check` both arches × {knob-off, `PCIE2`, `PCIE2`+`TEGRA`} (zero net-new
warnings on every combo); `test-arm 22` MISSION SUCCESS; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6 + 3/3
secondaries + idle/busy heartbeat + VUG-HONESTY PASS (both knob-off AND `PCIE2`-on — the knob-on run
witnesses the graceful skip); `kernel8-test` 0-FAIL; `esp-jetson` (+`UNAOS_PCIE2=1`) links. Bench
runbook: `scripts/orin-net2-bench.md`.

### ORIN-NET-3 — PS widen + controller-0 link bring-up + device enumeration (`UNAOS_PCIE3`, knob-gated; the lane's first fabric writes)

NET-2 named the exact blockers: controller-0's ECAM (`0x2e_2000_0000`, ~184 GiB) and MMIO `ranges`
(~200–212 GiB) sit **above** the tegra regime's 36-bit PS output ceiling, and the link is expected DOWN
as firmware leaves it. NET-3 removes the ceiling, brings the link up, and identifies what is behind it —
the last step before a NIC driver arc. It is the lane's **first deliberate fabric-write arc**, and it
performs writes in **exactly three classes**, each announced on serial *before* it is issued:

| # | write class | where | what |
|---|---|---|---|
| M1 | **TCR PS/IPS widen 36→40-bit** | `mmu_tegra` (EL2 + EL1 fallback) + `boot_tegra` (post-drop EL1) | one system-register field (PS `0b001→0b010`) programmed at MMU-enable, knob-gated; lets `map_mmio_window` reach the ECAM |
| M2 | **appl LTSSM enable** | controller 0 `appl` block, `APPL_CTRL` bit 7 | one read-modify-write to set `LTSSM_EN`, then poll DLL-active (finite backstop) |
| M3 | **BAR sizing ritual** | the enumerated device's BARs (via ECAM) | per BAR: all-ones probe → readback → **restore original immediately** |

Everything else stays read-only: **no** driver bind, **no** bus-master/MEM decode enable, **no** MSI, **no**
DMA, **no** writes to any other controller, **no** PERST/PHY reprogramming beyond the LTSSM enable. Recon
stops at "device identified, BARs sized, link state recorded." Poison-rejection (PI-V3D-1) guards every
identity read. Any RAS/SError raised by a write lands in the `mmu_tegra` Part-C / healed `exceptions.rs`
vectors (recorded syndrome + spin) — that IS the STOP-record for an unexpected fabric fault. All code is
behind the `pcie3` cargo feature (`UNAOS_PCIE3=1`), which **implies `pcie2`** (it builds on NET-2's
`census2` / `map_mmio_window` machinery); the metal M2/M3 writes are additionally `tegra`-gated.

**M1 — the PS widen, and the two ceilings.** The tegra regime maps PA via 1 GiB L1 blocks. NET-2's
`TCR_EL2` PS field was `0b001` = 36-bit = **64 GiB output ceiling**, so a block descriptor for the ECAM
(~184 GiB output) raised an address-size fault and `map_mmio_window` **refused** it (`BeyondPsCeiling`).
NET-3 widens PS to `0b010` = 40-bit = **1 TiB output ceiling** (`TCR_EL2_ACTIVE` / `TCR_EL1_ACTIVE`,
knob-gated), so the MMU may *emit* the 38-bit output addresses. Crucially the widen flips **only the
output field, not `T0SZ`** — the L1 table still spans **512 GiB** (512 entries × 1 GiB, 39-bit VA), and
that is enough VA to identity-map every controller-0 aperture (max ~212 GiB). So after the widen there are
**two** reachability limits, and `map_mmio_window` enforces both:

- **PS output ceiling** — `PS_OUTPUT_CEILING_GIB` (64 knob-off, **1024** under `pcie3`).
- **L1 table VA extent** — `L1_GIB_EXTENT = 512` (the array-safety guard: `l1.add(gi)` is only in
  bounds for `gi < 512`, regardless of the wider PS ceiling).

The tighter of the two binds: after the widen it is the **512-GiB table extent**, comfortably above every
controller-0 aperture. The audit of the old 64-GiB constant (`grep PS_GIB_CEILING`) confirmed the only
MMIO-ceiling site was `map_mmio_window`; the remaining `< 64` bounds (`build_l1`'s `ram_gib_mask`,
`map_fb_region`'s DRAM bound, `gib_mapped`) are RAM/framebuffer data-structure widths (Orin RAM/scanout ≤
~10 GiB), independent of the MMIO output ceiling — deliberately unchanged.

Applied to controller 0 under `pcie3`:

| region | base | NET-2 reach (36-bit) | NET-3 reach (40-bit) |
|---|---|---|---|
| `appl` / `config` / `dbi` | `0x140a_0000` / `0x2a00_0000` / `0x2a08_0000` | AlreadyMapped (GiB-0) | AlreadyMapped |
| `ecam` | `0x2e_2000_0000` (256 MiB, ~184 GiB) | **BeyondPsCeiling** | **Mapped** (new Device-nGnRE block) |

**M2 — link bring-up (controller 0 only).** With the link left down by firmware, NET-3 runs the `appl`
LTSSM-enable sequence — **Linux `drivers/pci/controller/dwc/pcie-tegra194.c` is the documentation of
record**. The single write sets `APPL_CTRL.LTSSM_EN` (bit 7); we then poll **DLL-active** via the RP's DBI
PCIe-capability Link Status (the NET-2 read path) *and* the appl-side `APPL_LINK_STATUS.RDLH_LINK_UP`
mirror, with a bounded spin backstop, and record the `APPL_DEBUG` LTSSM state (`0x11` = L0) either way. A
still-down link after a correct enable is an **honest hardware result** — recorded, not improvised;
further bring-up (PERST deassert / PHY retrain) is beyond the M2 enable sequence and this arc's three write
classes.

**M3 — enumerate + BAR sizing.** With the link up, NET-3 enumerates the downstream device through the
**now-mapped ECAM** (`ecam_base + (1<<20)` = bus1:dev0:fn0 — the direct hardware config window M1
unlocked, so **no iATU CFG-region fabric write** is needed, the blocker NET-2 flagged), poison-rejecting
the identity read. It then runs the standard **all-ones/readback BAR-sizing ritual** on that device's
BARs — writing `0xffffffff`, reading the size mask, and **restoring the original immediately** — handling
32- and 64-bit memory BARs, per-BAR write announced. No decode-enable, no driver bind.

**QEMU vs metal — the honesty boundary.** QEMU `virt` models **no Tegra234 RC**, so — exactly as NET-2 —
all link/device answers are **attended-metal**. The tegra TCR widen is only programmed on the metal boot;
QEMU's gates are two: (1) `census2`'s **graceful skip** (`dtb_addr = 0` on the GICv3 handoff), and (2) a
dedicated **PS-widen mapping witness** (`ps_widen_witness`, on the GICv3 virt path) that exercises the real
`map_mmio_window` reach ceiling and **inverts NET-2's regression** — the ECAM that NET-2 refused is now
reachable, and refusal persists above the reachable range:

```
:: PCIE3: ORIN-NET-3 PS-widen mapping witness (QEMU virt; the tegra TCR widen itself is metal-only) ::
:: PCIE3:   ECAM 0x2e20000000 (+0x10000000, GiB 184): REACHABLE (NET-2 BeyondPsCeiling refusal INVERTED by the 40-bit widen) ::
:: PCIE3:   refusal preserved: @512GiB(table-extent)=true @1TiB(>40-bit)=true ::
:: PCIE3: ORIN-NET-3 PS-widen witness: PASS ::
```

On `virt` the `mmu_tegra` L1 statics are **not** the active regime (the boot core translates through
`boot_virt`'s table), so the descriptor the witness writes into the inert static is functionally invisible
— it observes only the returned reach classification. Everything past that (the appl LTSSM enable, the
downstream device identity, the BAR sizes) is **ATTENDED-METAL-PENDING**; the recon is staged as a tar for
a consolidated sitting (`scripts/orin-net3-bench.md`).

**The fabric-write ledger (every deliberate write this arc adds).** (M1) `TCR_EL2`/`TCR_EL1` PS/IPS
`0b001→0b010` at MMU-enable (system-register, knob-gated) + the Device-nGnRE **page-table descriptor** for
the ECAM GiB that `map_mmio_window` now installs. (M2) one `APPL_CTRL |= LTSSM_EN` read-modify-write on
controller 0. (M3) per enumerated BAR, an all-ones probe write **and** an immediate restore write (≤ 2 per
32-bit BAR, ≤ 4 for a 64-bit pair). That is the complete set; nothing else touches fabric, config, or
system registers.

**Byte-identity (knob-off) — the ratified 1-byte Location class.** Knob-off (`pcie3` and `pcie2` both off)
the module blocks + all call sites are compiled out. The `TCR_*_ACTIVE` constants fold to the exact NET-2
literals (so `enable_el2`/`enable_el1`/the drop program identical values and the `mmu-regs` banner is
unchanged), and the added `#[cfg]` source lines shift the `#[track_caller]` Location `line` field of the
`jb2b_attach` call below them — the same ratified 1-byte class NET-1/NET-2 disclosed. Verified per-section
against baseline `3fe218a` (knob-off `esp-jetson kernel.elf`): **`.text` (538896 B), `.rodata`, `.data`,
`.got` byte-identical**; **`.data.rel.ro` differs by exactly one byte** — a single `core::panic::Location`
`line` low byte `0x7a → 0x87` (the `#[cfg(pcie3)]`/comment lines added before the `jb2b_attach` site shift
its `#[track_caller]` line number). The loadable image is unchanged but for that one Location literal.

**Gates green.** `./arroyo check` both arches × {knob-off, `PCIE3`, `PCIE3`+`TEGRA`} (zero net-new
warnings on every combo); `test-arm 22` MISSION SUCCESS; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6 +
VUG-HONESTY PASS (knob-off **and** `PCIE3`-on — the on run witnesses the census2 graceful skip **and** the
PS-widen witness PASS); `kernel8-test` 0-FAIL. Bench runbook: `scripts/orin-net3-bench.md`.

### ORIN-NET-4 — RTL8168/8111 GbE driver + smoltcp bind (`UNAOS_NET4`, knob-gated; the Orin's first network path)

**The metal facts NET-4 stands on (the NET-1/2/3 recon ground truth, not re-litigated).** The consolidated
NET-3 sitting resolved the two questions the recon existed to answer, and they are **load-bearing** for the
driver:

- **The device is a Realtek RTL8168/8111 GbE controller** — `vendor 0x10ec`, `device 0x8168`, at
  controller-0 bus1:dev0:fn0 (reached through the PS-widened ECAM). This fixes the *programming model*:
  the datasheet-standard **C+ command / descriptor-ring** interface (Realtek + Linux `r8169`).
- **The link was observed UP (gen1 x1) — the `LINK-UP-pre-LTSSM` observation.** The RP's DBI PCIe-capability
  Link Status read DLL-active before the M2 `APPL_CTRL.LTSSM_EN` write even landed (firmware left the link
  trained), so a device answered below the root port and the ECAM enumeration succeeded. NET-4 therefore
  does not re-fight link bring-up (NET-3 owns that); it *claims the device NET-3 found*.
- **The BARs sized:** BAR0 I/O `0x100`, **BAR2 mem `0x1000`** (the 4 KiB register window — the driver's
  MMIO), BAR4 mem `0x4000` (MSI-X). NET-4 drives **BAR2**.

**What NET-4 is.** The Orin's first *network path*: the driver that turns "device identified" into "packets
move." It builds directly on NET-3 (`net4` **implies `pcie3`**) and runs on the metal Orin **after** the
NET-3 `census2` has widened the regime, enabled the LTSSM, and enumerated bus1:dev0:fn0. In one bring-up it:

| step | what |
|---|---|
| **claim** | resolve controller-0's `ecam` from the live DTB (tegra-RC + firmware-`okay` gated), map it via the PS-widened `map_mmio_window`, read bus1:dev0:fn0, poison-reject the identity, and **confirm it is the RTL8168** (`0x10ec:0x8168`) before touching anything |
| **decode-enable** | set the device's `COMMAND` register **MEM-space + bus-master** — the config write NET-3 deliberately *refused* (a driver's job), so the BARs decode and the NIC can master DMA |
| **BAR map** | resolve BAR2 (64-bit-BAR aware) and map its 4 KiB register window Device-nGnRE via the same PS-widened path |
| **reset + MAC** | soft-reset the MAC (`CR.RST`, finite backstop), read the station MAC from `IDR0..5` |
| **rings** | allocate the C+ **RX (32) / TX (8) descriptor rings** + DMA buffers, program `RDSAR`/`TNPDS`, `RMS`/`MTPS`/`TCR`/`RCR`, enable `CR = RxEnb\|TxEnb` (RTL8168 programming-guide / `r8169` `rtl_hw_start` order) |
| **bind** | build a **smoltcp 0.13 `phy::Device`** over the rings (the x86 e1000/`smolnet` seam, now literally shared) + an `Interface` (MAC, static bring-up CIDR, default route) and poll it. The `phy::Device`/`RxToken`/`TxToken` boilerplate is the **shared `crate::net_phy`** adapter (`SmoltcpPhy<N, O>` over the small `RawNic` trait — `transmit`/`rx_frame_raw`/`mac`); NET-4 implements `RawNic` for its `NET4_DEVICE` registry. See **§NET-PHY**. |

**DMA / identity-map invariant (and its one metal risk).** `mmu_tegra` builds an **identity map (VA==PA)**
for RAM, so — exactly as the x86 e1000 relies on UEFI's 1:1 tables — a heap allocation's virtual pointer
*is* the physical address the NIC DMAs against. The rings are `alloc_zeroed` (256-byte-aligned bases), the
pointer used verbatim as the descriptor/buffer physical address. The one unknown QEMU cannot settle:
whether the **SMMU** (`smmu_tegra`) is translating or bypassing controller-0's PCIe stream IDs. NET-4
programs the identity-physical addresses and documents the SMMU-bypass assumption; an attended sitting
confirms it. This is why the arc is **code-complete-prior-to-metal by design**. The **second** unknown
(review-lens fold): **cache coherency** — rings/buffers are Normal cacheable RAM handed over with
`dsb sy` only (ordering, not clean/invalidate); correctness assumes Tegra234 controller-0 PCIe is
I/O-coherent toward DRAM. Metal signature if it isn't: rings never advance / torn or zero frames on a
live link; the fix is clean-before-OWN + invalidate-before-read, never a weakened OWN protocol.

**Poison-honesty (the PI-V3D-1 lane law) throughout.** The device-identity read rejects `0xffffffff` /
`0xdeadbeef` as absent decode; the ring init does a **poison-honest `TCR` readback** and *fails the
bring-up* (rather than trusting a dead controller) if the register space returns open-bus.

**Write discipline.** NET-4, being a driver, does the writes NET-3 refused — the `COMMAND` decode-enable and
the RTL8168 control/ring register program — each **announced on serial before issue**. It touches only
controller-0's downstream device and that device's own BAR2: no other controller, no MSI/MSI-X, no PERST/PHY.

**QEMU vs metal — the honesty boundary.** QEMU `virt` models **no Tegra234 RC**, so the whole MMIO/DMA +
smoltcp layer is additionally **`tegra`-gated**. A `net4`-standalone (virt) build prints **one honest
witness line** and returns before any MMIO — the GICv3 regression run is unperturbed:

```
:: PCIE4: ORIN-NET-4 RTL8168 driver compiled; no Tegra234 RC on this build (QEMU virt) — bring-up is metal-only (UNAOS_NET4=1 UNAOS_TEGRA=1) ::
```

On metal (`UNAOS_NET4=1 UNAOS_TEGRA=1`) the full claim → rings → bind sequence runs; live ICMP/ARP over the
bound interface is **attended-metal** (the devkit link has no DHCP lease pre-config, so a static bring-up
address lets the interface bind and exercise ARP once the link's real subnet is known).

**Byte-identity (knob-off).** `net4` is default-OFF and armed only by `UNAOS_NET4=1`; with it off the module
+ both call sites are compiled out **and the smoltcp dep is not pulled** (it is declared optional under
`net4`), so the default tegra/virt media are byte-identical to baseline. `net4` is *not* stripped by
`arm_features` (unlike the x86-only `smolnet`) — it is a real aarch64 feature.

**Gates green (pre-metal).** `UNAOS_NET4=1 UNAOS_TEGRA=1 ./arroyo check` both arches (zero net-new
warnings), `net4`/virt + default-off both arches; `test-arm 22` MISSION SUCCESS; `UNAOS_GICV3=1 test-arm 40`
CAPSTONE 6/6 + VUG-HONESTY PASS (knob-off **and** `net4`-on — the on run fires the `PCIE4` witness line and
still completes CAPSTONE 6/6, i.e. the net4 virt build does not perturb the GICv3 run). Metal verification is
deferred to an attended sitting. Bench runbook: `scripts/orin-net4-bench.md`.

### ORIN-NET-4b — the outbound-iATU fix-forward (FAULT-AT-M1: raw PCIe BAR deref'd as a CPU address)

**The fault of record (verbatim brief; the silicon record, NOT re-litigated).** The NET-4 driver reached
the RTL8168 (config reads/writes via ECAM fine, the NET-3 recon preamble twice-confirmed), then the
**FIRST BAR-register write** (CR soft reset at the BAR2-mapped address) raised a **RAS Uncorrectable** —
SNOC *"Illegal address (software fault)"* / Carveout, ADDR body `a5a5a5a5a5a5` poison fill; recovery needed
a **DC cut**. Bench observation to adjudicate: **BAR2 read back `0x4000_4000`** — a PCIe BUS address; with
the iATU unprogrammed there is no outbound CPU→PCIe MEM translation, and the driver mapped the raw BAR value
as a CPU physical address and dereferenced it.

**Adjudication — the bench observation was RIGHT.** Firmware assigns the device's BARs inside controller-0's
PCIe MEM window (PCI base `0x4000_0000`), so `0x4000_4000` is a *PCIe bus* address. The old path
(`bar_base = bar2 & !0xf`) treated it as a CPU PA and called `map_mmio_window(0x4000_4000, …)`. `0x4000_4000`
falls in **GiB 1** — the SYSRAM/BPMP carveout window `mmu_tegra::fill_table` maps Device-nGnRE — so
`map_mmio_window` even returned `AlreadyMapped` without complaint. The first register write
(`0x4000_4000 + CR`) then hit a protected Tegra carveout → the SNOC illegal-address RAS fault + `a5a5…`
poison. The shape fully explains the fault; nothing else needed.

**The ATU design (DWC / `pcie-tegra194` sequence-of-record).** The DesignWare host model reaches a PCIe MEM
address from the CPU through an **outbound iATU region**: the DT `ranges` property gives the CPU
aperture ↔ PCIe-address windows, and each outbound region maps `[cpu_base, cpu_base+size) → PCIe target`.
The fix:

1. **Resolve** controller-0's `ranges` from the live DTB and pick the MEM window (space code 2/3) whose
   `[pci_base, pci_base+size)` **contains** the firmware BAR2 value; resolve the `atu_dma` reg region as the
   ATU base (DWC-core `dbi + 0x30_0000` fallback documented).
2. **Program** DWC outbound iATU **region 0** for that whole window (unrolled registers at
   `atu_base + N*0x200`: LOWER/UPPER BASE, LIMIT, TARGET, CTRL1 = TYPE_MEM, CTRL2 =
   `ENABLE|INCREASE_REGION_SIZE`) — every write announced before issue, region enabled last.
3. **Translate, do not reassign.** Keep firmware's BAR assignment (it already sits inside the
   ranges-described window NET-3 sized it in) and compute the **CPU-side aperture** address
   `cpu_addr = cpu_base + (bar_pci − pci_base)` (~200 GiB up, inside the PS-widened 40-bit / 512-GiB-table
   reach). Map **that** Device-nGnRE and drive the registers there — never the raw BAR value. Reassigning
   the BAR would mean more fabric writes for no gain and diverge from the Linux DWC model (which programs
   outbound ATU from `ranges` and leaves enumerated BARs in place). **Choice: keep + translate.**

**Why reads through the CPU aperture are safe where the raw BAR was fatal.** `cpu_addr` targets the RC's own
outbound MEM aperture (RC-owned MMIO); a mistranslation or a down link returns **UR / all-ones**, never a
carveout. The raw BAR value aliased DRAM/SYSRAM — a *write* there is fatal. So the fix both (a) routes
accesses through the aperture and (b) earns a pre-write probe.

**The M2/M3 guard — poison-honest readback before the first write (V3D-2 lesson, made law).** After mapping
the CPU aperture and **before any register write**, the driver reads **TCR** (`0x40`, whose chip-version bits
a live RTL8168 always returns — `r8169` reads exactly this to identify the MAC) and rejects the poison fills
— open-bus `0xffffffff`, firmware `0xdeadbeef`, and now the **carveout `0xa5a5a5a5`** the M1 fault left
(added to `is_poison`). A poison readback ⇒ the register window is not live ⇒ the bring-up is **REFUSED**
cleanly, before any write, so the first-write fault can never recur. This guard is the general rule for the
driver flow: **every new MMIO window earns a probe read before its first write.**

**Write discipline unchanged, plus the ATU class.** The iATU writes target the controller's own internal
register block (GiB-0 device window, always decoding on a powered RC — NET-2/3 read `dbi`/`appl`/`ecam`
there), not a carveout, so they carry none of the M1 fault's risk. Each is announced `>>> ATU WRITE
(M1-fix): …`. The COMMAND decode-enable and RTL8168 control/ring program are unchanged.

**Gates green (pre-metal; QEMU models no Tegra234 RC, so the metal path is unexercised by construction).**
`./arroyo check` default + `UNAOS_NET4=1 UNAOS_TEGRA=1` + `UNAOS_VNET=1` both arches (zero net-new
warnings); `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6; `UNAOS_NET4=1 UNAOS_GICV3=1 test-arm 40` `PCIE4`
witness + CAPSTONE 6/6; `test-arm 22` MISSION SUCCESS; `test 22` unregressed; `kernel8-test` 0 FAIL. Metal
verification (the real first-write, now guarded) is the next attended sitting. Bench:
`scripts/orin-net4-bench.md`.

### ORIN-SMP idle-heartbeat — the parked-core pinned-bar fix (per-core telemetry honesty)

> Numbering note: the spawning round labelled this arc "SMP-6", but SMP-6/7/8 are already merged
> (SMP-8 closed the SMP-3 bring-up mystery — the production 6-core path is code-complete). By the
> real sequence this is the SMP-**9** step: bring-up is done, so the next in-lane SMP work is making
> the now-complete multi-core telemetry honest. Filed under the spawn's label for continuity.

**The gap.** The VUG-1 CPU-pulse meter reads `sched::meter_cpu_ticks(cpu) -> (CPU_BUSY, CPU_IDLE)`
and shows `busy/(busy+idle)` per core. Those counters are bumped **only inside `dispatch_next`**
(idle on an empty run queue, busy on a dispatch). A secondary brought up on the GICv3 path
(`smp_virt::__secondary_rust_virt`) comes online, publishes `CORE_READY`, then **parks in a bare
`loop { wfi }`** — it never enters `sched::run()`, so it never calls `dispatch_next`, so its
`(CPU_BUSY, CPU_IDLE)` stay `(0, 0)` forever. `(0,0)` renders a **pinned/undefined** meter bar for a
demonstrably-online-idle core: online-idle is indistinguishable from wedged/never-ran. This is the
follow-up flagged at the SMP-6 vug metal witness (*"parked cores' bars read PINNED"*).

**The fix (additive, in-lane).** `sched::note_core_idle(cpu)` — a bounds-checked, lock-free-relaxed
seam (same introspection-only contract as the existing pulse counters) that bumps `CPU_IDLE[cpu]`.
`__secondary_rust_virt` calls it once **before** the first `WFI` (so the BSP's bring-up summary can
witness `idle > 0` deterministically) and again on **every wake**, so the bar tracks the core staying
parked-idle. No scheduling-path change; no counter removed; a core that later runs the scheduler still
accrues busy/idle normally through `dispatch_next`.

**Determinism.** The park-entry bump happens strictly after the core publishes `CORE_READY`, and the
BSP formats its summary only after a full BSP→AP ping round trip + the AP→BSP verdict — a multi-ms
wall-clock gap. Note the precise ordering: IRQs are unmasked (`exceptions::enable_irq`) *before* both
`CORE_READY` (Release) and the park-entry `note_core_idle`, so the AP→BSP synchronization edge (the
SGI the BSP waits on) can in principle be established at a program point *before* the first `CPU_IDLE`
bump; the witness's `Relaxed` load is therefore not *memory-model-guaranteed* to observe that first
bump. The load-bearing guarantee is instead the **in-loop re-park bump** — each WFI wake (the BSP→AP
ping is exactly such a wake) bumps `CPU_IDLE` again inside the loop, and every real Orin/vug window
sees many such wakes. So `busy + idle > 0` is what the witness asserts, not an exact count. A `(0,0)`
read would be an ordering finding — **fail-loud (`FAIL`) + STOP + report**, never papered over with a
sleep; the fail-loud stance is what keeps the (theoretical) relaxed-load race safe.

**The QEMU gate (not metal-only).** The `virt` GICv3 path runs the *same* `__secondary_rust_virt`
park, so `UNAOS_GICV3=1 ./arroyo test-arm` proves the fix: after `3/3 secondaries online`, the BSP
reads each secondary's pulse counters back and emits
`:: AARCH64 SMP: per-core idle heartbeat PASS — 3 online APs report idle (not pinned) ::` with a
per-AP `busy=0, idle=N` breakdown (observed `idle=2`: park-entry bump + one BSP→AP-SGI wake). CAPSTONE
6/6 and the 3/3 secondary bring-up are unchanged. The real Orin `start_secondaries_tegra` 6-core path
parks through the identical `__secondary_rust_virt`, so the same honesty holds on metal — the live vug
pixels (parked APs' bars reading idle/0% busy instead of pinned) are the **accruing metal witness**,
not this arc's gate.

**Gates green:** `./arroyo check` both arches × {knob-off, `UNAOS_TEGRA=1`}; `UNAOS_GICV3=1
./arroyo test-arm` CAPSTONE 6/6 + 3/3 secondaries + heartbeat PASS; `./arroyo test-arm` virt v2
MISSION; `./arroyo kernel8-test` 0-FAIL (the shared-`sched.rs` regression gate; `check` skips
baremetal); `esp-jetson` links, `tegra:` count 109.

### ORIN-SMP busy-heartbeat — cooperative scheduled work on the virt secondaries (the idle-heartbeat's other half)

The idle-heartbeat above proved only that a *parked* online secondary reads honest **idle**
(`busy=0, idle>0`, not the pinned `(0,0)`). Its complement — that an online secondary can actually
**run scheduled work and read busy** — is the QEMU-testable slice of *"SMP scheduling on `virt`"* (long
named a later step: the secondaries are brought up but otherwise only park). This arc lands that slice.

**Cooperative only — and why that is the honest QEMU half.** A `virt` secondary runs at **EL2** and has
no per-core generic-timer tick (arming one would double-count the shared `TICKS` clock — deferred, see
above). So preemptive multi-core scheduling stays the **metal-only** proof. But *cooperative*
scheduling needs no timer and no IRQ: `switch_context` is EL-neutral (callee-saved + SP, no `eret`),
and a run-to-completion task (yield/exit only) never blocks or sleeps. This is exactly how the boot-core
CAPSTONE and the Pi CAPSTONE already run cooperatively under QEMU. So each secondary runs **one bounded
cooperative pass** over a finite, BSP-pre-staged queue, then parks idle as before.

**Mechanism (additive, in-lane; `sched.rs` + `smp_virt.rs`).**
* BSP, after the ping proofs: `sched::stage_secondary_work(cpu, n)` spawns `n` cooperative probe tasks
  pinned to each online secondary's run queue, then `sched::secondary_work_go()` releases them.
* Secondary, right after its AP→BSP ping and **before** the idle park: `sched::run_secondary_work(core)`
  spins (IRQ unmasked — the BSP→AP ping still lands) until released, then `run_until_empty(core)` drains
  its queue (real `dispatch_next` dispatches, each bumping `CPU_BUSY[core]`), then sets a per-core
  `SECWORK_DONE` flag.
* BSP waits on the completion **count** (`secondary_work_done == expected`) with a generous finite
  backstop (~2 s) before reading the meter — so the busy witness never races an un-run core, and never
  flakes when the host is loaded (a fixed short ceiling did; see Shared-tail safety below).
* Run queues are per-CPU; a secondary drains **its own** queue and never contends the boot core's
  (which runs the JC3 CAPSTONE). No scheduling-path change; no counter altered; the idle-heartbeat
  seam and the deferred timer-stretch invariant are untouched.

**The QEMU gate.** `UNAOS_GICV3=1 ./arroyo test-arm` now emits, per online AP,
`AP <n> pulse (busy=8, idle=2) ran+idle` (`busy=8` = 2 probe tasks × 4 dispatches each: 1 initial +
3 yields) and both witness lines:
`:: AARCH64 SMP: per-core idle heartbeat PASS ...` **and**
`:: AARCH64 SMP: per-core busy heartbeat PASS — 3 online APs ran cooperative scheduled work ::`.
The idle-heartbeat asserts `idle > 0`; the busy-heartbeat asserts `busy > 0`; a `(0,0)` or bare
`busy=0` read fails **loud** (`FAIL`), never papered over. CAPSTONE 6/6 and the 3/3 bring-up are
unchanged.

**Shared-tail safety (metal + probe).** `__secondary_rust_virt` is the *shared* real-entry tail for
the `virt` `start_secondaries`, the real Orin `start_secondaries_tegra`, AND the SMP-probe legs — but
only the `virt` BSP arms the work (`SECWORK_ARMED`, set once before any `CPU_ON`) and calls
`secondary_work_go`. So `run_secondary_work` returns immediately on the tegra/probe paths (not armed →
**no wait at all**), while an armed `virt` secondary waits on `secondary_work_go` under a generous
finite backstop (~1 s). Every wait is a finite safety backstop, not the normal-case timing (which
completes in microseconds when the host is idle) — so the witness stays **deterministic even under
heavy host load** (proven at 8× CPU oversaturation), where the earlier fixed ~20 ms one-shot ceiling
flaked (slow-to-arrive secondaries missed the window and parked idle → spurious `busy=0` FAIL). No
path can hang a core, the failure mode an unbounded spin would introduce on every non-`virt` caller.
**Consequence:** on real Orin today
the secondaries still park *idle* (the tegra path stages no work), so metal shows the idle bar, not a
busy one. A **live vug busy bar on a metal secondary** requires the tegra bring-up to also stage +
release cooperative work — a small, attended follow-up (staging cooperative EL2 work on real Orin
secondaries wants a metal sitting to confirm), NOT claimed done here.

**Deferred (unchanged by this arc):** preemptive multi-core scheduling on the secondaries (per-core
timer tick + IRQ-driven `timer_preempt`) remains the metal-only step; it lands with a per-core-only
tick path in `timer.rs` (the shared-`TICKS` double-count containment named above), outside this lane.

**Gates green:** `./arroyo check` both arches; `UNAOS_GICV3=1 ./arroyo test-arm 40` = 3/3 secondaries
+ idle-heartbeat PASS + **busy-heartbeat PASS** + CAPSTONE 6/6; `./arroyo test-arm 22` MISSION;
`./arroyo kernel8-test` 44 PASS / 0 FAIL (shared-`sched.rs` unregressed on the Pi — its APs run the
full `run()` loop via `start_aps`, an untouched path).

### VUG-HONESTY — the parked-core *display* completion (the heartbeats' third leg)

The two heartbeats above made the *counters* honest: a parked online secondary reads `idle > 0`, and
one that ran cooperative work reads `busy > 0`, so the BSP's one-shot boot witness never sees the
pinned `(0, 0)`. But that is not the whole story the **live** meter tells. `vug`'s CPU-pulse meter
(`CpuPulse::refresh` in `vug.rs`) does not read the cumulative counters — it samples their **per-window
deltas** (~5×/sec) and shows `db/(db+di)` per core. And there sat a residual display-honesty defect the
counter fixes did not reach:

**The residual.** `refresh`'s fallback branch read: *"`db + di == 0` this window ⇒ this is the demo
core executing outside the scheduler ⇒ credit it the render loop's own busy%."* That is right for **one**
core — the core actually running the render loop, whose own counters freeze while it draws. It is wrong
for **every other** core with frozen counters. A parked EL2 `virt`/Orin secondary gets **no periodic
wake** (no per-core timer; `note_core_idle` bumps only at park-entry and on the rare BSP→AP SGI), so
between two 200 ms windows its counters do not move — `db + di == 0` — and the old code credited it
`own_load` too. Result: while the crystal spins at a high render busy%, *all* parked cores mirrored the
busy demo core and read **PINNED** — fabricated load on cores doing nothing. A never-online core `(0,0)`
read the same. This is the exact defect the R18 XCARVE metal witness flagged (*"parked cores' bars read
PINNED"*), surviving *below* the counter layer the heartbeats fixed.

**The fix (display-layer only; `vug.rs` + one additive accessor).** The pure decision now lives in
`vug::classify_load(db, di, is_demo, own_load)`:
* `db + di > 0` → honest busy fraction (unchanged — the scheduled path, incl. every x86 AP; never
  regressed).
* frozen **and** the demo core → its measured render load (the one legitimate `own_load` case).
* frozen **and not** the demo core → **`PARKED`** — a load-array sentinel, never a fabricated number.

The demo core is identified live via a new introspection accessor `sched::meter_current_cpu()` (a
`TPIDR_EL1/EL2` self-index on aarch64, the mirror `gs:[0]` self-index on x86 — same additive,
lock-free, no-scheduling-effect contract as `meter_cpu_count`/`meter_cpu_ticks`). A `PARKED` core draws
a **dashed, cooler track** (`draw_pulse_bar`) — deliberately distinct from an idle core's solid-dim
track, so *"idle 0%"* and *"never woken"* never read alike (the JD16/JD17 unset-≠-invent doctrine); the
`pulse` full-screen view prints `park` in place of a percent. No scheduler logic, no counter, and no
`note_core_idle` seam changed — this builds strictly **on** the merged heartbeats.

**The QEMU witness (deterministic, framebuffer-free).** `vug::parked_display_witness()` exercises
`classify_load` over the separating cases (busy, idle→0%, half, frozen-demo→render load,
frozen-non-demo→`PARKED`) and emits one line; it is wired into the `virt` CAPSTONE boot
(`run_capstone_boot_core`), so `test-arm` / the GICv3 suite print
`:: VUG-HONESTY: parked-core display witness PASS ... a frozen non-demo core reads PARKED (never the
demo core's load) ::`. The live parked-bar pixels on a real multi-core Orin panel are the accruing
**metal** witness, as before.

**Gates green:** `./arroyo check` both arches × {knob-off, `UNAOS_TEGRA=1`}; `UNAOS_GICV3=1 ./arroyo
test-arm 40` = 3/3 secondaries + idle + busy heartbeat PASS + **VUG-HONESTY witness PASS** + CAPSTONE
6/6; `./arroyo test-arm 22` MISSION; `./arroyo kernel8-test` CAPSTONE COMPLETE / 0 FAIL (shared
`sched.rs` unregressed on the Pi); `./arroyo test` (x86) MISSION, no behavioral change (the accessor
compiles the shared `refresh`; `vug` runs only on the GUI, so headless x86 is untouched).
---

### ORIN-SMP-DEFAULT — the 6-core bring-up is now the tegra DEFAULT (`UNAOS_NOTEGRASMP` opt-out)

Three rounds of SMP work — §ORIN-SMP-1..8 (entry shape, concurrency, wake position, image layout,
idle/busy heartbeats, count-based determinism) — are all proven, yet the shipping Orin image ran
workers on the boot core only because the real kick-off was gated behind the opt-*in*
`UNAOS_TEGRASMP=1`. This arc promotes the kick-off to the tegra **default** and turns the knob into an
opt-*out*, mirroring the PORTSW-1/SMOLNET default-ON/negative-knob policy.

**What changed — build scripts only; no kernel source, no scheduler logic.** The `tegrasmp` cfg
already fully gates the kick-off (`smp_virt.rs::start_secondaries_tegra` + the `fdt_tegra` `/cpus`
enumerator, §ORIN-SMP-3), so the promotion is entirely a matter of *which features the build pushes*:

- **`unaos/arroyo`.** Any tegra build now arms `tegrasmp` by default: the `esp-jetson` target (which
  force-adds `tegra`) also force-adds `tegrasmp` unless opted out, and a `UNAOS_TEGRA=1` build adds
  `tegrasmp` unless opted out. `UNAOS_NOTEGRASMP=1` suppresses the push → the tegra image runs
  boot-core-only, byte-identical to the pre-flip baseline. `UNAOS_TEGRASMP=1` still arms it explicitly
  (back-compat; byte-identical to the new default).
- **`unaos/builder/src/main.rs`.** Parity mapping only — the x86_64 builder never produces aarch64
  tegra media (arroyo's `esp-jetson` does), so it maps the explicit `UNAOS_TEGRASMP=1` knob for
  sync-completeness; `UNAOS_NOTEGRASMP` is a no-op there.
- **`unaos/crates/kernel/Cargo.toml`.** `tegrasmp = ["tegra"]` unchanged; the doc comment records the
  default-on policy.

**Byte-identity (proven, not claimed).** Because the change only remaps env→features and the kernel
source is untouched, the new default's ELF is byte-identical to a pre-arc `UNAOS_TEGRASMP=1` build, and
the `UNAOS_NOTEGRASMP=1` opt-out is byte-identical to the pre-arc default. Verified in-tree at this
arc: default `esp-jetson` and explicit `UNAOS_TEGRASMP=1 esp-jetson` produce the same `kernel.elf`
(`sha256 bde59b4f…`); the `UNAOS_NOTEGRASMP=1` opt-out is the distinct boot-core-only baseline
(`sha256 cb7df0a4…`), `tegra:` count 109, zero `ORIN-SMP-3` strings.

**Gates green:** `./arroyo check` both arches × {default, `UNAOS_NOTEGRASMP=1`, `UNAOS_TEGRA=1`};
`./arroyo test-arm` MISSION SUCCESS (virt unaffected — tegrasmp is tegra-only, never on the virt
board); `UNAOS_GICV3=1 ./arroyo test-arm` CAPSTONE 6/6 + idle + busy heartbeat PASS + VUG-HONESTY
witness PASS (workers on the real virt secondaries by default, independent of the tegra flip);
`./arroyo kernel8-test` 43 PASS / 0 FAIL (Pi unaffected — a tegra-only default flip).

**Not the stick default yet.** Per the §ORIN-SMP-7-addendum standing rule, a tegra image becomes the
metal boot-stick default only after a clean multi-boot metal record. The staged candidate is a
`DEFAULT-CANDIDATE pending metal record`; this arc ends at staging + the sitting note, and does **not**
declare it the stick default. (The xHCI-takeover wall of §JETSON-XCARVE is image-layout-sensitive; a
new default layout may sample it — that is expected wall data, not an arc failure. QEMU cannot fire it.)

### ORIN-SDMMC — Tegra234 microSD-slot SDMMC controller READ-ONLY recon (`UNAOS_SDMMC`, knob-gated; the installer line's first rung)

**Context — the installer line.** The Orin devkit is the "mule" for a UnaOS-native installer: it has the
microSD slot, and the goal (a later arc) is to write the card **from a booted UnaOS** (boot the validated
USB stick → flash the card in place → verify by content → reboot). An installer is storage-target bring-up
+ partition/format + payload write + verify. ORIN-SDMMC-1 is the **first rung**: the NET-1 house pattern of
**read-only census before any touch**. It resolves the SDMMC controller, brings the SDHCI engine up to
card-identification, reads CID/CSD/capacity, and reads sector 0 — and **writes nothing to the card**. The
write path (a scratch-region → readback → real-write paranoia ladder) is a separate arc (SDMMC-2) behind
its own arm flag; the seated card is sacred until then.

**Driver:** `arch/aarch64/sdmmc_tegra.rs`, `sdmmc`-feature-gated, tegra-gated at the MMIO layer (the net4
witness pattern). Mirrors the proven Pi 4 `drivers::emmc2` SDHCI register/bit model — the BCM2711 "32-bit
view" register names ARE the standard SDHCI block (BLKSIZECNT 0x04, ARG1 0x08, CMDTM 0x0C, RESP 0x10.., DATA
0x20, PRESENT-STATE 0x24, CONTROL0 0x28, CONTROL1 0x2C, INTERRUPT 0x30, CAPS 0x40), which Tegra234 serves
identically. Mirrored, not copied: the base is DTB-resolved (no hardcode), and the Tegra vendor quirks are
documented rather than assumed away.

**The census design (M1–M3).**
- **M1 — FDT census + poison-honest probe.** A read-only DTB walk enumerates every SDMMC/SDHCI-compatible
  node (compatible `nvidia,tegra234-sdhci` / a `mmc@`/`sdhci@` node name), logging each candidate's `reg`
  base/size, `status`, `non-removable`/`cd-gpios` flags, and a bounded `compatible` ASCII view. The picked
  instance is the **enabled removable** one (the microSD slot; the on-module eMMC is `non-removable`), with
  the first-enabled instance as a documented fallback. No hardcoded base — the DTB decides. The controller
  window sits in the GiB-0 device window `mmu_tegra` already maps Device-nGnRE at boot (sdmmc1 @
  `0x0340_0000` « `0x4000_0000`), so it is reached without the pcie2 `map_mmio_window` path — guarded by a
  mapped-GiB check (never deref an unmapped address). Then, **before any write** (the NET-4b law), the
  `CAPABILITIES`/Host-Version registers are read and poison-checked (`0xffffffff`/`0xdeadbeef`/`0xa5a5a5a5`
  ⇒ absent decode ⇒ **honest refusal**, no reset, no writes).
- **M2 — SDHCI identification (READ-ONLY).** Reset, status-latch, 3.3 V bus power, card-detect (Present
  State bit 16; absent ⇒ an honest "no card seated" line, never a hang), 400 kHz identification clock, then
  the ladder CMD0 → CMD8 → CMD55/ACMD41 → CMD2 (CID) → CMD3 (RCA) → CMD9 (CSD) → CMD7 → CMD16, raising to
  the 25 MHz default-speed transfer clock. Prints the **CID** (manufacturer/OEM/product/revision/serial/
  date), the **CSD-derived capacity** (blocks + MiB, CSD v1/v2), and the negotiated **bus width/speed**
  (1-bit, default-speed). Every wait is CNTPCT-bounded.
- **M3 — sector-0 read census.** A single-block CMD17 READ into a 512-byte stack buffer, then the first 16
  bytes hex + a signature classification: **GPT-protective MBR** (0x55AA + first-partition type 0xEE),
  **FAT boot sector** (jump opcode + "FAT" type string), **MBR** (0x55AA), or **unknown**.

**Read-only by construction.** The module issues ONLY the identification ladder + CMD17 single-block READ.
There is no `cmd(24)`/WRITE_SINGLE_BLOCK, no CMD25, no ACMD6 bus-width write, no erase, no CMD6 switch — a
`grep` of the source for `WRITE`/`write_block`/`cmd(24)` finds nothing targeting card storage. The
controller-register writes it does make (SRST, clock, power, command issue) are the SDHCI machinery every
read needs; none is a write to the card's storage.

**Tegra vendor-quirk assumptions (documented; metal-pending).** (1) The firmware/BPMP already enabled the
sdmmc1 module clock + pad power (the bootloader read the card to boot); we drive only the standard SDHCI
internal-clock divider, never the CAR/BPMP clock or the Tegra vendor pad registers (≥ 0x100). If the
internal clock never stabilises, the diagnosis is "input clock gated" (a BPMP-clock MRQ, a later arc) —
surfaced, never worked around. (2) If `CAPABILITIES[15:8]` reads 0, a documented 200 MHz base is assumed
(logged); identification runs at 400 kHz then 25 MHz, so an inexact base only changes the divider. (3)
4-bit / high-speed negotiation is deferred (not needed to census the card).

**QEMU vs metal.** QEMU models no Tegra234 SDMMC controller, so the whole MMIO path is `tegra`-gated: a
`sdmmc`-standalone (virt) build does **zero MMIO** and prints one honest compiled-present witness line; only
`UNAOS_SDMMC=1 UNAOS_TEGRA=1` on real Orin silicon touches the controller. Correctness off-metal comes from
`arroyo check`, the QEMU regression non-regression (the tegra code is compiled out on virt), and faithful
adherence to the SD Physical Layer / SDHCI spec. Metal leg: census whatever card is seated (bench runbook
`scripts/orin-sdmmc1-bench.md`).

**Expected metal serial chain** (grep `SDMMC`):
```
:: SDMMC: ORIN-SDMMC-1 Tegra234 microSD READ-ONLY recon (DTB @0x… size=0x…) ::
:: SDMMC:   M1: candidate /bus@0/mmc@3400000 reg=0x03400000(size 0x10000) status=okay removable cd-gpios compat='nvidia,tegra234-sdhci|' ::
:: SDMMC:   M1: picked /bus@0/mmc@3400000 @ 0x03400000 (size 0x10000) as the microSD slot ::
:: SDMMC:   M1: controller window 0x03400000(+0x10000) is in the GiB-0 device window (already Device-nGnRE) ::
:: SDMMC:   M1: live SDHCI — CAPABILITIES=0x……… (base-clk … MHz, 8-bit=…, ADMA2=…), spec-version reg=0x… (SDHCI 4.0) ::
:: SDMMC:   M2: card detected (Present State 0x………) ::
:: SDMMC:   M2: CID manufacturer(MID)=0x… OEM(OID)='..' product(PNM)='.....' rev=0x. serial(PSN)=0x……… date=M/YYYY ::
:: SDMMC:   M2: capacity … blocks (… MiB, CSD v2), addressing block (SDHC/SDXC), v2 (CMD8 ok) ::
:: SDMMC:   M2: identified — RCA 0x…, bus 1-bit, default-speed (<=25 MHz) [4-bit/HS negotiation deferred] ::
:: SDMMC:   M3: sector 0 first 16 bytes = xx xx … ::
:: SDMMC:   M3: sector-0 signature = GPT-protective MBR (…) ::
:: SDMMC: ORIN-SDMMC-1 DONE — microSD censused: … blocks (… MiB, CSD v2), sector-0 … (READ-ONLY; no card write) ::
```

**Refusal / no-card signatures (all honest, all bounded — none is a bug to work around):**
- `M1: CAPABILITIES[…] = 0x… — POISON … recon REFUSED (no reset, no writes)` — the window is not a live
  SDHCI (open bus / carveout / firmware fill); the NET-4b read-before-write guard doing its job.
- `M2: no card seated (Present State …, Card-Inserted clear) — census done, nothing to identify` — an empty
  slot, the honest terminal state.
- `M2: internal clock never stabilised … the input clock is gated (BPMP-clock diagnosis)` — the vendor-quirk
  (1) fallback; scope a BPMP-clock arc, do not touch the CAR.
- `M1: no SDMMC/SDHCI-compatible node found` / `M1: controller window … outside the already-mapped GiB
  windows` — DTB resolution or reach refusal.

**STOP for the bench:** any RAS/SError line, or any `>>> … WRITE` announcing a card-storage write (there
must be **none** — this rung is read-only). See `scripts/orin-sdmmc1-bench.md`.

**Byte-identity.** `sdmmc`-off, the module + both call sites vanish; the tegra image is byte-identical to
baseline (zero `SDMMC` strings knob-off). Standalone feature — does not imply `tegra`/`pcie2`, so it runs on
both the metal tegra build (`UNAOS_SDMMC=1 UNAOS_TEGRA=1`) and the virt witness build (`UNAOS_SDMMC=1`).

**Gates green.** `./arroyo check` default + `UNAOS_SDMMC=1 UNAOS_TEGRA=1` + `UNAOS_SDMMC=1` (virt) all both
arches; knob-off `UNAOS_GICV3=1 ./arroyo test-arm 40` CAPSTONE 6/6, `./arroyo test-arm 22` + `./arroyo test
22` + `./arroyo kernel8-test` 0 FAIL; knob-on `UNAOS_SDMMC=1 UNAOS_GICV3=1 ./arroyo test-arm 40` prints the
witness line + CAPSTONE 6/6 intact. Landing: `review/unaos-orin-sdmmc1-LANDING.md`.

#### ORIN-SDMMC-2 — the write path behind the paranoia ladder (`UNAOS_SDMMC_ARM`, a SEPARATE arm on top of `UNAOS_SDMMC`)

**Double-gating (the seated card is sacred).** The write path is gated on a **second, separate** explicit arm
on top of the `sdmmc` recon feature: a `sdmmc_arm` cargo feature (which `requires` `sdmmc`), wired to
`UNAOS_SDMMC_ARM=1`. A card write happens only when BOTH are present — the recon knob (`UNAOS_SDMMC=1`) alone
never writes the card. **Every line of the write path is `sdmmc_arm`-gated**, so a plain `UNAOS_SDMMC=1` build
is byte-identical in behavior to the merged ORIN-SDMMC-1 recon: the unarmed kernel contains **zero** SDMMC-2 /
`ladder` strings (the whole-file binary hash differs only in Cargo's feature-fingerprint metadata, which the
compiler embeds in symbol hashes once the feature exists — the active code is unchanged).

**The paranoia ladder** (announced-before-issue, bounded, RESTORE-by-construction). After the read census
(M1–M3), when armed, the driver runs seven verified steps against the SEATED card:
1. **re-run the rung-1 read census** (sector 0) and confirm it is byte-stable since the census read;
2. **pick the SCRATCH REGION** — the card's **last block** (LBA `capacity-1`), **only if sector 0 shows no
   GPT**. This is the scratch-region rule: a GPT **backup** header lives in the card's last LBA, exactly where
   the scratch region sits, so **with GPT present the ladder REFUSES all scratch writes this arc** and says so
   honestly (`write ladder REFUSED (GPT present …)`). Without GPT there is no end-of-device structure to
   endanger, and the write is stashed-then-restored regardless, so a power loss mid-ladder can only corrupt an
   end-of-device block that held no partition/backup structure — the justification for the rule;
3. **read + stash** the scratch block's current contents;
4. **single-block CMD24 WRITE** of a stamped pattern (ASCII marker + LBA + a byte-position sweep);
5. **read-back + byte-compare** against the pattern (the write verified);
6. **RESTORE** the stashed original;
7. **read-back + byte-compare** the restoration.

`:: SDMMC: write ladder — write/verify/restore/verify => PASS ::` is emitted **only if every step verified**.
Any mismatch is a **distinct `ladder FAIL step N (…)` line naming the step**. If a write may have landed
(steps 4/5), an **emergency restore** of the stash runs; and if a restore step (6/7) or the emergency restore
**cannot be verified**, the driver **dumps the stashed original as 32 `stash[0xNN]:` hex rows** so the data is
never silently lost. The CMD24 primitive is the driver's **only** card-write, reachable only from the ladder,
only under `sdmmc_arm`, only against a stashed scratch block; it waits for transfer-complete **and** DAT0-busy
release so a following read never races the card's internal programming. Multi-block (CMD25/CMD18) is not used
this arc — the witness is single-block only.

**Virt witness (armed):** on a `sdmmc_arm` build without `tegra`, one extra honest metal-only line (zero MMIO)
records the ladder is compiled-present but has no Tegra234 SDMMC to write. Bench runbook (both rungs):
`scripts/orin-sdmmc1-bench.md`. Landing: `review/unaos-orin-sdmmc2-LANDING.md`.

#### ORIN-INSTALL-1 — the first real installer flow (`UNAOS_INSTALL_TARGET_SD`, the THIRD destructive gate)

**What it is.** The installer line's rung-3 wired to silicon: boot from the USB stick and install UnaOS onto
the SEATED microSD *from inside UnaOS*. It drives the arch-neutral installer **engine** (`crate::install` —
the GPT writer + FAT32 formatter + sha extent-verify proven pre-silicon by `UNAOS_INSTALLDEMO`, see
`docs/dev/OS/10_INSTALL/installer_engine.md`) over a new `InstallTarget` that speaks the rung-2 armed SD write
path. The SD-specific glue lives in `arch/aarch64/sdmmc_tegra.rs` (`SdInstallTarget` + `install_to_sd`); the
engine's verified write/verify semantics are **untouched** — this arc only supplies the block target, a
metadata-zeroing pass, and the destructive announcement.

**The three-gate escalation ladder.** A real install is the most destructive act in the tree, so it stands
behind three independent gates (each a separate cargo feature / knob), and even fully gated it announces
before it writes:

| gate | feature / knob | meaning |
|---|---|---|
| 1 | `sdmmc` / `UNAOS_SDMMC` | the controller is up and the card **census succeeded** (we hold a `Card`) |
| 2 | `sdmmc_arm` / `UNAOS_SDMMC_ARM` | the rung-2 armed CMD24 write path is compiled in |
| 3 | `install_target` / `UNAOS_INSTALL_TARGET_SD` | the explicit **destructive-confirmation** gate (this arc a knob; on metal the future UX asks the operator) |

Under gate 3 the flow prints **exactly what it is about to destroy** — the sector-0 classification, the card's
capacity, and its **CID identity** (re-decoded from the retained CMD2 response) — *before* the first write.

**Non-blank cards (do-it-right beyond the demo's blank-only law).** The engine's `blank_check` refuses a
non-blank scratch disk; an *installer* must instead handle a card that already carries a partition table. It
does **not** refuse — it announces, then re-establishes the FAT formatter's blank-precondition by zeroing
**exactly** the ESP metadata region (`fat32::blank_region_sectors` = reserved + both FAT copies; no more — the
data area's free clusters may hold stale bytes harmlessly — and no less — a stale FAT entry would forge an
allocation). The GPT writer overwrites its own structures, so it needs no pre-zero.

**The flow** (each step a serial line; any engine error is a single named `FAIL`):
GPT (protective MBR + primary/backup + ESP + data, parse-back verified) → zero the ESP metadata region →
FAT32 format the ESP → copy the payload → **sha extent-verify** (re-read every written extent off the card) →
`ORIN-INSTALL-1 SD install — gpt+zero+fat32+copy verify => PASS`.

**The payload (M2), answered honestly.** The installer's intended v1 payload is the *running system's own boot
volume* (the seed's clone-thyself design). But `install_to_sd` runs at the **pre-JB2b-takeover EL2 census
site** (`sdmmc_census`, `main.rs` before the xHCI takeover), where the USB boot stick is **not yet enumerated
as a block device** (`drivers::block::info()` is `None`), so self-read is unreachable this arc. Rather than
fake a clone, v1 writes a small **generated marker payload** (`UNAOS.IMG`) that the in-tree FAT reader can
find, and the **self-clone is flagged as the named follow-up (INSTALL-2)**. (The in-tree `fs::fat::mount()`
interop self-check the x86 witness runs is likewise skipped here — `mount()` reads the USB block layer, not
this armed SD target; the by-content SD extent-verify is the proof.)

**Transfer primitives.** INSTALL-1/2 looped the proven rung-2 single-block CMD24/CMD17 primitives; ORIN-SDMMC-3
(below) adds bounded multi-block CMD25/CMD18 that `SdInstallTarget` now uses for whole-file writes, with the
single-block path retained as the 1-block/metadata fallback. The bounded metadata-zero pass (~2064 sectors for
a 64 MiB ESP) rides the same multi-sector path.

**Byte-identity.** Every installer line — including the `cid` field added to `Card` — is `install_target`-gated,
so an `sdmmc_arm`-without-`install_target` build is byte-identical in behavior to the merged rung-2 ladder
(**string-identity evidence: the tegra `sdmmc_arm` binary contains zero `ORIN-INSTALL` / `ABOUT TO DESTROY`
strings; the `install_target` binary contains them**).

**Virt witness (third gate):** on an `install_target` build without `tegra`, one honest metal-only line (zero
MMIO) records the flow is compiled-present but has no Tegra234 SDMMC to install onto. The full flow's **first
execution is the attended Orin sitting** (runbook `scripts/orin-sdmmc1-bench.md`, install leg). Landing:
`review/unaos-install1-LANDING.md`.

#### ORIN-INSTALL-2 — the self-clone: the installer copies the running system's real boot payload (`UNAOS_INSTALL_TARGET_SD`)

**What it is.** INSTALL-1's payload was honest but synthetic — a generated `UNAOS.IMG` marker, because at its
pre-JB2b census site the USB boot stick was not yet a block device. INSTALL-2 completes the clone-thyself
design: the installer reads the **running system's own boot payload off the USB stick's ESP** and mirrors it
onto the freshly-formatted microSD ESP, every copied file sha-extent-verified. Same three-gate ladder, same
about-to-destroy announcement, same engine — this arc adds the reposition, the real payload read, and the
engine's multi-file/multi-cluster/subdirectory writer.

**The position adjudication (the named blocker, resolved).** The install act is **split in two**. The
read-only `sdmmc_census` still runs at the pre-JB2b EL2 site (`main.rs` ~1396) and now **stashes** the card
identity (`base` + `Card` + `sector 0`) into `PENDING_INSTALL` instead of installing. The **destructive
install is deferred** to `sdmmc_install_from_usb`, called from the boot sequence **immediately after the JB2b
pump window** (`main.rs`, just after the `if xusb_alive { … }` block, ~line 1522) — the earliest position
where all three constraints hold:
- **(a) the stick is readable** — the JB2b pump's `service_storage` publishes `drivers::block::BLOCK_DEVICE`
  in its settle window, so `drivers::block::info()` is now `Some` (the tegra build is not `baremetal`, so
  `drivers::block` routes to the xHCI USB-MSC path, never the Pi `emmc2` SD backend — no backend conflict with
  the directly-driven `SdInstallTarget`);
- **(b) the SDMMC MMIO is still usable** — the census mapped the controller window (GiB-0 Device-nGnRE), which
  persists; and the core is **still at EL2** here (the JM6 drop is further down), so the SD path's bounded
  `hlt()` waits still have the JM4 timer as their wake source;
- **(c) nothing later is perturbed** — the JD2 console shell reads `drivers::block` (the USB stick), not the SD;
  the SMP wake touches neither. The microSD is repartitioned in isolation.

If the census never identified a card, or the stick did not enumerate (`drivers::block::info()` is `None` —
no self to clone), the deferred site prints an honest SKIP and does nothing destructive.

**The payload = the real thing.** `install_flow` mounts the USB stick through the in-tree FAT reader
(`fs::fat::mount()`, which reads `drivers::block`) and **walks its whole boot tree**, mirroring it onto the SD
ESP: the esp-jetson layout is `/EFI/BOOT/BOOTAA64.EFI` + `/kernel.elf`, so the copy recreates the `EFI/` and
`EFI/BOOT/` subdirectories (with well-formed `.`/`..`) and both files — nothing hardcoded; whatever the stick
carries is enumerated and cloned. Each file is read whole (bounded by a 32 MiB per-file cap against the 48 MiB
heap), its clusters allocated and written, and its record kept for verify.

**The engine extension (the flagged single-FAT-sector bound, lifted).** INSTALL-1's `write_payload_file`
required the whole file chain to fit in **FAT sector 0** (≤125 clusters ≈ 64 KiB) — far too small for a real
`kernel.elf`. INSTALL-2 adds, **additively**, a `TreeWriter` to `install/fat32.rs` (the x86 witness's
`write_payload_file` is untouched): a running free-cluster cursor, a `set_fat_run` that RMWs **every FAT
sector a run touches, in both FAT copies** (multi-MB files link correctly), and directory clusters built
**wholly in memory** then written once (so a stale data cluster on a non-blank card never leaks bytes into a
directory). INSTALL-2 assumed each directory fit one cluster (the boot tree does); ORIN-SDMMC-3 (below) lifts
that to multi-cluster directories. Same verify discipline downstream: every file is re-read off the card and
SHA-checked through the engine's own `verify_extents`.

**The flow** (each step a serial line; any engine/read error is a single named `FAIL`):
GPT (parse-back verified) → zero the ESP metadata region → FAT32 format → **mount the USB stick + clone its
boot tree file-by-file** → **per-file `sha256=… VERIFIED`** manifest (the real manifest replacing INSTALL-1's
single `UNAOS.IMG`) → `ORIN-INSTALL-2 SD install — gpt+zero+fat32+clone(N files) verify => PASS`.

**Byte-identity.** Every INSTALL-2 line — including the `Card` `Clone`/`Copy` derive and the stash — is
`install_target`-gated (`fat32.rs`'s additions live in the `install` module, itself compiled only under
`installdemo`/`install_target`), so an `sdmmc_arm`-without-`install_target` build is byte-identical to the
merged rung-2 ladder (**string-identity: the `sdmmc_arm` jetson binary contains zero `ORIN-INSTALL` strings
and rebuilds byte-for-byte; the `install_target` binary contains them**).

**Virt witness:** unchanged in shape — one honest metal-only line (no Tegra234 SDMMC in QEMU). The full
self-clone's **first execution is the attended Orin sitting** (runbook `scripts/orin-sdmmc1-bench.md`, install
leg). Landing: `review/unaos-install2-LANDING.md`.

#### ORIN-SDMMC-3 — multi-block SD transfers + multi-cluster directories (the INSTALL-2 perf/size follow-ups)

**What it is.** INSTALL-2 flagged two follow-ups: single-block-only SD transfers (a multi-MB `kernel.elf` is
thousands of CMD24s) and single-cluster directories (a directory of >16 entries hit an honest `NoSpace`).
ORIN-SDMMC-3 closes both. Both are `install_target`-gated (⇒ `sdmmc_arm` ⇒ `sdmmc`), so a plain
`sdmmc`/`sdmmc_arm` build is byte-for-byte identical to the merged recon/ladder.

**Multi-block CMD18/CMD25 (the SDHCI multi-block model).** Two new primitives in `sdmmc_tegra.rs` move a run
of contiguous blocks in one command: `BLKSIZECNT` carries the block count in `[31:16]`, the Transfer-Mode
field (`CMDTM[15:0]`) sets Block-Count-Enable + Multi-Block-Select, and **completion uses auto-CMD12** — the
host controller issues `STOP_TRANSMISSION` itself at the counted transfer's end. Auto-CMD12 was chosen over an
explicit CMD12 so there is no second command round-trip and no separate CMD12 error path; normal
transfer-complete (`INT_DATA_DONE`) still fires. `read_blocks_at` (CMD18 READ_MULTIPLE) is available on the
read side; `write_blocks_at` (CMD25 WRITE_MULTIPLE) is the armed multi-block card-write. `SdInstallTarget`'s
`read_sectors`/`write_sectors` loop a **bounded 64-block (32 KiB) chunk** and drop to the retained single-block
CMD17/CMD24 primitive for a 1-block tail (so the 512-byte FAT-metadata path stays single-block, a whole-file
write rides multi-block). The **rung-2 witness ladder is untouched — still single-block CMD24** — so its
metal-verified semantics do not shift.

`TreeWriter::write_file` now writes each file's contiguous data chain in **one multi-sector `write_sectors`
call** (recording per-cluster extents unchanged for verify granularity), so a real `kernel.elf` copies as a
handful of CMD25 bursts instead of one CMD24 per 512 bytes.

**Multi-cluster directories (the >16-entry bound, lifted).** `TreeWriter` gains `alloc_dir_clusters`,
`write_dir_image`, and `reserve_root`; a new `dir_clusters_for_slots` sizes a directory's cluster chain from
its entry count. A directory's image is built **wholly in memory across its whole (contiguous) cluster chain**
then written once — the same no-stale-byte discipline, now spanning >1 cluster. `copy_dir` sizes each
directory (root included, via `reserve_root` before any file allocation so cluster 2's chain stays contiguous)
up front from the source entry count. `NoSpace` now means the *volume* is genuinely full, not a 16-entry
directory. `put_dir_entry` operates on a `&mut [u8]` slice (the whole image) rather than a single 512-byte
cluster.

**Witness (x86 `installdemo`).** `run_demo_inner` gains a final step: it re-establishes the blank-precondition,
re-formats, and builds a tree whose `SUB/` directory holds **20 files (22 slots → 2 clusters)** through the
`TreeWriter`, then the in-tree FAT reader mounts the volume, follows `SUB/`'s cluster chain, and re-reads +
SHA-verifies every file — a genuine multi-cluster directory read on the same engine the SD installer uses:
`:: INSTALL: multi-cluster dir — SUB/ 20 entries across 2 clusters, all re-read + sha-verified (dirs=1) => PASS ::`.

**Byte-identity.** The multi-block primitives and their strings are `install_target`-gated; string-identity
evidence: the tegra `sdmmc_arm` binary contains **zero** `ORIN-SDMMC-3` / `mb: CMD18` / `mb: CMD25` strings and
**rebuilds byte-for-byte identical**; the `install_target` binary contains them. Metal exercise of the
multi-block SD path is part of the attended Orin sitting (QEMU models no Tegra234 SDMMC). Landing:
`review/unaos-orin-sdmmc3-LANDING.md`.

## AARCH64-VNET — virtio-net-mmio driver + smoltcp bind on QEMU `virt` (`UNAOS_VNET`, knob-gated)

**Purpose: a pre-metal, QEMU-testable proof of the aarch64 smoltcp seam that ORIN-NET-4 built.** NET-4's
RTL8168 driver (`arch/aarch64/rtl8168_tegra.rs`) is `tegra`-gated because QEMU models no Tegra234 root
complex, so its ring → `smoltcp::phy::Device` → `Interface` → ICMP path was only ever *compile*-tested off
metal (a `net4`/virt build prints one honest witness line and does no MMIO). AARCH64-VNET exercises the
**identical seam shape** against a device QEMU *does* model — a `virtio-net-device` on the `virt` machine's
virtio-mmio bus — driven end-to-end with **real packets** over QEMU user-mode networking (slirp). It is the
confidence, before the Orin sitting, that the ring mechanics and the smoltcp adapter are correct. It does
**not** touch NET-4's code or behaviour; it is a parallel, self-contained aarch64 net module
(`arch/aarch64/virtio_net.rs`).

**Relationship to NET-4.** Same invariants, different transport:

| | ORIN-NET-4 (`rtl8168_tegra.rs`) | AARCH64-VNET (`virtio_net.rs`) |
|---|---|---|
| device | Realtek RTL8168/8111 (PCIe, Tegra234 RC) | virtio-net (virtio-mmio, QEMU `virt`) |
| discovery | DTB walk → PS-widened ECAM → config claim | fixed virtio-mmio window scan (`0x0a00_0000`, ×32) |
| rings | RTL8168 C+ descriptor rings | legacy split virtqueues (RX q0 / TX q1) |
| DMA | identity map (`mmu_tegra`), SMMU-bypass *unknown* | identity map (virt EL2/EL1), **no SMMU** (unmediated) |
| smoltcp | `phy::Device` + `Interface` + ICMP, poll | *same shape*, + a live ICMP-echo witness |
| QEMU | metal-only (no RC modelled) | **runs, end-to-end, with slirp packets** |

The "same shape" is now literally shared code: every driver — both aarch64 nets and the x86 e1000e — binds over **`crate::net_phy`** — see **§NET-PHY**. VNET implements `RawNic` for its `VNET_DEVICE` registry.

**Transport: LEGACY virtio-mmio (QEMU's default).** QEMU's `virtio-mmio` bus defaults to
`force-legacy=true`, so the 32 transports (base `0x0a00_0000`, stride `0x200`, in the low-1-GiB Device
window both the EL2 firmware map and the JC3 EL1 drop cover) present the **version-1 / legacy** interface: a
single `QueuePFN` register + `GuestPageSize`, the legacy split-virtqueue layout, and a fixed 10-byte
`virtio_net_hdr` (no `num_buffers` — `MRG_RXBUF` is *not* negotiated). The driver reads the `Version`
register and reports it; a version-2 (modern) transport is reported and skipped honestly rather than
mis-driven.

**Feature negotiation (minimal).** Of the offered device features (`0x39bf8064` on QEMU 11) the driver
accepts **only `VIRTIO_NET_F_MAC` (bit 5, `0x20`)** — enough to read the station MAC from config space — and
nothing else: no checksum/GSO offload, no mergeable RX buffers. That keeps the header 10 bytes and the
datapath a plain copy.

**Bring-up sequence.** Scan for a live virtio-net transport (magic `virt`, device-id 1); status handshake
reset → ACKNOWLEDGE → DRIVER; negotiate features; set `GuestPageSize = 4096`; set up the RX/TX virtqueues
(`QueueSel`/`QueueNum`/`QueueAlign`/`QueuePFN = region >> 12`, each ring a page-aligned `alloc_zeroed`
region whose identity-physical base is the device's DMA target); pre-post every RX descriptor
(device-writable, full buffer); read the MAC; set DRIVER_OK. Then bind a `smoltcp::phy::Device` over the
rings (the NET-4 / e1000 adapter shape) and drive an ICMP echo to the slirp gateway.

**Where it runs.** On the QEMU `virt` **GICv3** path, at EL2 *before* the JC3 EL2→EL1 drop — the heap is up
and the virtio-mmio window is mapped. The bounded ICMP-ping witness (≤ `PUMP_ITERS` poll iterations,
non-hanging) completes synchronously, then CAPSTONE runs unchanged. Static bring-up addressing is the slirp
subnet: guest `10.0.2.15/24`, gateway `10.0.2.2` (slirp answers ARP + ICMP for it; no DHCP needed).

**Witness (self-checking):**

```
:: AARCH64 VNET: virtio-net-mmio bring-up (QEMU virt, legacy transport) ::
:: AARCH64 VNET:   found virtio-net at slot 31 (0xa003e00) version 1 ::
:: AARCH64 VNET:   device features 0x39bf8064; accepted 0x00000020 (F_MAC=yes) ::
:: AARCH64 VNET:   virtio-net up: station MAC 52:54:00:12:34:56, RX/TX virtqueues armed (16 desc each) ::
:: AARCH64 VNET: ping 10.0.2.2 RTT 4374 us (4/4 sent, 4/4 replies) => PASS ::
:: AARCH64 VNET: AARCH64-VNET DONE — virtio-net driver up + smoltcp bound ::
```

**How to run.** `UNAOS_VNET=1 UNAOS_GICV3=1 ./arroyo test-arm`. The knob adds both the `vnet` kernel feature
and the QEMU `-netdev user,id=unet -device virtio-net-device,netdev=unet` args (arroyo `VNET_ARG`).

**Gates green:** `./arroyo check` both arches × {default, `UNAOS_VNET=1`}; knob-off `./arroyo test-arm`
MISSION SUCCESS + `UNAOS_GICV3=1 ./arroyo test-arm` CAPSTONE 6/6 + priority+aging PASS + VUG-HONESTY PASS
(unregressed); knob-on `UNAOS_VNET=1 UNAOS_GICV3=1 ./arroyo test-arm` fires the `AARCH64 VNET … => PASS`
witness (4/4 slirp replies) **and** CAPSTONE still 6/6; `./arroyo kernel8-test` 0 FAIL; `./arroyo test`
MISSION SUCCESS (x86 unaffected — `vnet` is aarch64-virt-only). Default OFF ⇒ the module + call site vanish
and the smoltcp dep is not pulled ⇒ byte-identical to baseline, and the QEMU invocation is byte-identical
(empty `VNET_ARG`).

### NET-PHY — the shared, arch-neutral smoltcp `phy::Device` adapter (`crate::net_phy`, `crates/kernel/src/net_phy.rs`)

Every NIC seam binds smoltcp the same way — a `phy::Device` whose `receive`/`transmit` shuttle raw L2
frames to/from the driver's rings, with a `RxToken` that hands smoltcp the received buffer and a `TxToken`
that copies the built reply back out. The two aarch64 net drivers (NET-4, VNET) **and** the x86 default net
stack (`smolnet.rs`, the e1000e — SOCK-1..7) each carried a near-identical copy of that boilerplate.
**NET-PHY** hosts it **once**: `SmoltcpPhy<N, O>` — owning the struct-local RX/TX scratch (`FRAME_CAP =
1536`, no heap) — is generic over a small trait

```rust
pub trait RawNic {
    fn rx_frame_raw(out: &mut [u8]) -> Option<usize>; // pop one raw RX frame (recycle the descriptor)
    fn transmit(frame: &[u8]);                          // send one raw L2 frame
    fn mac() -> Option<[u8; 6]>;                         // station MAC, or None if unregistered
}
```

The three methods are **associated functions** (no `self`): each driver reaches its one registered NIC
through a module-static registry (`NET_DEVICE` / `NET4_DEVICE` / `VNET_DEVICE`) behind a short-held lock —
the e1000 `raw_rx`/`raw_tx` discipline (never hold the registry lock across a smoltcp poll).
`rtl8168_tegra.rs` implements `RawNic` for `Rtl8168Nic`, `virtio_net.rs` for `VnetNic`, and `smolnet.rs`
for `E1000Nic`; each bind constructs its phy and drives its own `Interface`/socket loop (NET-4 a bounded
bind-witness poll, VNET a live ICMP echo, x86 the full SOCK-1..7 stack — those stay per-driver). `fmt_mac`
(the boot-log MAC formatter) is shared here too.

**The RX observer seam (`O`).** x86 `smolnet` additionally snoops inbound ARP replies as they cross
`receive()` (smoltcp hides the resolved neighbor MAC, which the `arp`/`ping` shell commands recover by
watching the wire); the aarch64 drivers do not. `SmoltcpPhy<N, O: RxObserver = ()>` is therefore generic
over an RX observer whose `observe` runs on every received frame before the tokens are minted. aarch64 uses
`O = ()` (a zero-cost no-op via `SmoltcpPhy::<_>::new()`, compiling to the exact pre-share datapath); x86
supplies an ARP-snooping observer via `SmoltcpPhy::with_observer(..)`, reproducing its old `receive()`
byte-for-byte.

**Home.** The module lives at the **crate root** (`crates/kernel/src/net_phy.rs`), not under `arch/`,
because it is shared across arches. It cannot live in a module named `net`: the kernel depends on an
**extern crate** `net` (`net::ethernet` / `net::arp`), which a `crate::net` module would shadow inside this
crate. It is gated `#[cfg(any(feature = "net4", feature = "vnet", feature = "smolnet"))]` — each pulls the
optional `smoltcp` dep — so it compiles under any combination and vanishes (with `smoltcp`) when all three
are off. **Zero behavior change**: the factoring is a pure code move — witness lines, feature gates, lock
discipline, and the no-alloc poll paths are the exact code the drivers carried. Gate-confirmed:
`UNAOS_VNET=1 UNAOS_GICV3=1 ./arroyo test-arm` still fires `AARCH64 VNET … => PASS` (4/4 slirp replies) +
CAPSTONE 6/6; `./arroyo test 22` still fires all x86 `SOCK-1..7`/`smolnet` witnesses (SOCK-1 ICMP 4/4 —
the ARP-snoop path); `check` green across the full net4/tegra/vnet/smolnet feature matrix on both arches.

### NET-DHCP — a shared DHCPv4 bring-up helper on the smoltcp seam (`crate::net_phy::dhcp_or_static`)

Both aarch64 NIC seams originally bound their `Interface` to a **hard-coded static address** (VNET the
slirp `10.0.2.15/24`; NET-4 the `192.168.1.2/24` placeholder flagged at the NET-4 landing as wrong,
because "the link's real subnet is a metal input"). **NET-DHCP** is the do-it-right fix: a shared,
arch-neutral helper — `net_phy::dhcp_or_static(prefix, iface, dev, now_ms, timeout_ms, static_ip,
static_prefix, static_gw) -> NetConfig` — that runs smoltcp's `dhcpv4::Socket` over an already-built
`Interface` until a lease is acquired or a bounded timeout elapses, then configures the interface **in
place** and returns the settled `NetConfig { leased, ip, prefix_len, gw }` for the caller's witness.

* **On lease** it applies the leased address + default route and emits
  `NET: DHCP lease ip=<ip>/<prefix> gw=<gw> (server <srv>) => PASS`.
* **On timeout** it emits an honest `NET: no lease within <n> ms — falling back to static <ip>/…` line
  and applies the caller's static values — **the fallback is preserved**, so a link with no DHCP server
  still comes up (the pre-DHCP behaviour is the honest last resort, not deleted).

**Arch-neutral by construction.** The caller supplies a monotonic **millisecond clock** (`now_ms`) that
drives *both* the smoltcp `Instant` fed to `poll` *and* the wall-clock timeout — so the bound is real
time, not iteration count (VNET reads `CNTPCT` via its `now_us()/1000`; NET-4 metal reads `CNTPCT`
directly in a module-local `now_ms()`, both at EL2 before the JC3 drop). The DHCP socket's storage is a
single-slot `SocketSet` scoped to the call — entirely stack-local (no heap growth), dropped on return
before the caller builds its own ICMP socket set. The socket-buffer-less `dhcpv4::Socket::new()` needs
`socket-dhcpv4`, already in every smoltcp feature set the kernel declares. **x86 `smolnet` is untouched
this arc** — the helper is trivially reusable there (a future fold: `smolnet` would call the same
`dhcp_or_static` in place of its static bind).

**VNET (M2, QEMU-proven).** `bind_and_ping` now calls `dhcp_or_static` first (3 s bound), then pings the
gateway of whichever config it settled on. slirp's built-in DHCP server hands out the exact static
values, so a healthy run **leases** them rather than falling back:

```text
:: AARCH64 VNET: NET: DHCP discover (timeout 3000 ms) ::
:: AARCH64 VNET: NET: DHCP lease ip=10.0.2.15/24 gw=10.0.2.2 (server 10.0.2.2) => PASS ::
:: AARCH64 VNET: ping 10.0.2.2 RTT 4248 us (4/4 sent, 4/4 replies) [dhcp] => PASS ::
```

Gate of record: `UNAOS_VNET=1 UNAOS_GICV3=1 ./arroyo test-arm 40` — DHCP lease PASS + ping 4/4 `[dhcp]`
PASS + CAPSTONE 6/6, no `FAIL`.

**NET-4 metal (M3, compile-tested off metal).** `bind_smoltcp` runs `dhcp_or_static` first (5 s bound)
before the bounded bind-witness poll; the static `192.168.1.2/24` is now the fallback, not the primary.
QEMU models no Tegra234 RC so this metal path never runs on virt — correctness is `check` across the
net4/tegra matrix + the RTL8168/slirp seam equivalence VNET proves. The bench runbook's expected serial
chain (`scripts/orin-net4-bench.md`) now carries the DHCP lines before the ring/ping expectations. On a
devkit link with a DHCP server the witness line reports `[dhcp]`; on a DHCP-less link it reports
`[static]` after the bounded fallback.

## PI-V3D — VideoCore VI (V3D 4.2) GPU foundation on the Pi 4 (Arc PI-V3D-1)

The first GPU silicon UnaOS touches. The target is **not** a triangle: it proves the full
non-graphics chain — firmware power domain, clock, MMIO register access, the V3D-private MMU, a
control-list fetch, and a tile store — with the smallest job that exercises all of it: **the GPU
clears a buffer to a known colour and the CPU verifies the bytes**. A triangle (binner control list
+ shader record) is the explicit *next* arc; nothing here starts it. All code lives in
`arch/aarch64/v3d.rs` behind the `v3d` cargo feature (`UNAOS_V3D=1`, implies `baremetal` ⇒ `pi`),
plus two `v3d`-gated firmware helpers in `arch/aarch64/mailbox.rs`. Knob-off, the module and its call
site vanish and the `kernel8` image is **byte-identical to baseline** (verified: `4337453747e7…`).

**Call site (a byte-identity note).** The bring-up is triggered from the tail of
`mailbox::init_framebuffer` (the VideoCore framebuffer path — exactly the surface M3 blits into),
*not* from the middle of `kernel_main`. Inserting a gated block into `kernel_main` shifts the embedded
panic-location line numbers of every aarch64 statement below it (a positional artifact — a single
blank line at that point changes the image hash), which would break the knob-off byte-identity gate.
A gated call at the end of the last function in `mailbox.rs` shifts nothing, so knob-off is bit-exact
to baseline while knob-on runs the full chain. The site is single-threaded (boot, pre-SMP, mailbox
idle) and has the framebuffer in hand — the same preconditions the BSP-post-`emmc2` spot would give,
minus the line-shift.

### QEMU vs metal — the honesty boundary

**QEMU `raspi4b` does not model V3D.** The V3D hub base `0xFEC00000` happens to be backed (reads
return 0), but the V3D **core** block at `+0x4000` is unmapped — a read there raises a *synchronous
external abort* (`ESR=0x96000010`, EC=0x25, IFSC=0x10 "external abort, not on a translation walk";
the Device window itself is MMU-mapped by `boot.rs` L1[3], so this is a bus abort from an unbacked
address, not a translation fault). Because `AARCH64 EXCEPTION` is a forbidden regression pattern, the
probe reads **hub `IDENT0` first and decides on it alone**: live (non-zero, non-all-ones) ⇒ real
silicon, proceed to the core registers; not-live (QEMU's 0) ⇒ print the graceful-degradation line and
return *before touching any core register*. So in QEMU the arc's only observable effect is:

```
:: V3D: PI-V3D-1 bring-up starting (VideoCore VI / V3D 4.2) ::
:: V3D: power domain 10 ON ::                 (QEMU models the firmware power/clock/clock-state tags)
:: V3D: clock id 5 rate set to 500000000 Hz ::
:: V3D: clock id 5 gate ENABLED (active) ::   (PI-V3D-2: SET_CLOCK_STATE opens the gate)
:: V3D: probe verdict BLOCK-DOWN — hub IDENT0 = 0x00000000 (block absent/unpowered; expected in QEMU raspi4b) — GPU bring-up skipped, graceful degradation ::
```

On metal, the corrected enable sequence is expected to yield `BLOCK-UP` with a live identity word; the
PI-V3D-1 false-pass value (`0xdeadbeef`) now yields the distinct `BUS-POISON` verdict, fail-closed.

Everything past the presence gate (MMU program, clear job) is **ATTENDED-METAL-UNVERIFIED**: written
correct-by-construction against the references below, exercised only at an attended Pi sitting. Do
**not** treat "no V3D in QEMU" as a divergence.

### The chain (M1–M4)

- **M1 — power, clock, probe.** Firmware mailbox `SET_DOMAIN_STATE` (tag `0x00038030`, domain 10 =
  V3D, state 1) **then** `SET_CLOCK_RATE` (tag `0x00038002`, clock id 5 = V3D, 500 MHz) **then**
  `SET_CLOCK_STATE` (tag `0x00038001`, clock id 5, on) — in that order (a powered-but-unclocked block
  reads garbage registers). The `SET_CLOCK_STATE` gate-enable is the **PI-V3D-2** addition: the RPi
  firmware treats a clock's rate and its enable gate *independently*, so `SET_CLOCK_RATE` alone
  programs the frequency but leaves the gate closed — the block stays powered-but-unclocked and its
  registers read open-bus poison. After a bounded settle, map the hub (`0xFEC00000`) + core 0
  (`+0x4000`), read `HUB_IDENT0..3` / `CTL_IDENT0..2`, decode the tech version (expect V3D 4.2 on the
  Pi 4). The legacy VC4 `Enable_QPU`/set-power tags are **not** the Pi-4 path.
- **M2 — the V3D MMU.** The big VC4→VC6 structural change: CLE fetches and tile stores go through a
  V3D-private page table. We build a **flat, confined identity** table — one `u32` PTE per 4 KiB of
  iova, `pte = VALID | WRITEABLE | (phys>>12)`, mapping **only** the buffer arena's own pages
  (`iova == phys`) and leaving every other PTE invalid. Programmed via hub `V3D_MMU_PT_PA_BASE` (base
  in pages), `V3D_MMU_CTL` (enable + PT-invalid/write-violation **abort** policy), the illegal-address
  catcher, then an MMUC flush + a TLB clear polled to completion with a finite backstop. *Confinement
  is the review-lens property:* the GPU can reach the arena and nothing else — a stray address faults
  in the V3D MMU rather than scribbling kernel RAM.
- **M3 — the clear job.** Build a render-only control list (no binner, no shaders) in the arena
  (`TILE_RENDERING_MODE_CFG` + `CLEAR_COLORS` + per-tile `TILE_COORDINATES` +
  `STORE_TILE_BUFFER_GENERAL` + `END_OF_TILE_MARKER` + `END_OF_RENDERING`), pre-seed the target with a
  sentinel, `clean_range` the CL + target to RAM, kick **CT1** (the render queue) via `CT1QBA`/`CT1QEA`,
  poll `CT1CS.CTRUN` to idle with a **finite ~500 ms backstop** (never an unbounded spin — the
  ORIN-SMP anti-hang discipline), `clean_invalidate_range` the target, and have the CPU byte-verify it
  equals the clear colour. On success the 64×64 result is blitted into the panel framebuffer (a
  metal visible witness). The RCL packet framing, target pointer, clear value, and tile loop are all
  present and arena-bounds-checked; the exact 4.2 per-packet field packing is the attended-metal
  refinement.
- **M4 — cache-maintenance audit.** V3D is **not** coherent with the A72 data cache. Every buffer the
  CPU writes for the GPU (page table, control list, target sentinel) is `cache::clean_range`d before
  the kick; every buffer the GPU writes for the CPU (the target) is `cache::clean_invalidate_range`d
  before the readback. No-ops in QEMU, load-bearing on metal — exactly the "works in QEMU, black
  screen on metal" class this kernel has been bitten by. The MMIO window needs no new mapping: the V3D
  hub/core sit in the `0xC0000000–0xFFFFFFFF` Device-nGnRnE GiB already mapped by `boot.rs` L1[3].

### Memory-safety invariant (the arc's review lens)

The GPU reaches RAM only through PTEs we mark VALID, and we mark valid **only** the arena's own
pages. Every V3D-visible address written into a control list (the store target, the CL begin/end
addresses handed to CT1) is bounds-checked to lie inside the arena before the kick (`arena_contains`);
the CL writer (`RclWriter`) saturates at the arena end and can never append past it; the page-table
fill is bounded by `PT_CAP` and refuses (fail-closed) if the arena would not fit. A control list that
referenced any address outside the arena would fault in the V3D MMU, not corrupt kernel memory.

### PI-V3D-2 — the poison-honest probe + the enable-sequence fix (fix-forward)

PI-V3D-1's probe **false-PASSED on metal.** The attended Pi sitting (2026-07-17) found every V3D
IDENT register reading `0xdeadbeef` — open-bus firmware fill, the block never decoded — yet the
liveness gate (`ident_looks_live`: reject only `0` and `0xffffffff`) treated the non-zero word as
"present" and proceeded, until the V3D MMU backstop caught reality and fail-closed cleanly. QEMU's
`raspi4b` does not model V3D, so nothing past the probe had ever run anywhere; the gate's blind spot
was invisible until silicon. Peter's ruling: **leave PI-V3D-1 merged, fix-forward as V3D-2.** Two
legs, both landed here:

**Leg 1 — poison-honest probe (`v3d.rs`).** The gate now discriminates **three** verdicts, each with
a distinct serial line, and only one proceeds:
  * **BLOCK-UP** — a live, non-zero, non-poison identity word → proceed to the core registers.
  * **BLOCK-DOWN** — `HUB_IDENT0 == 0x00000000` (absent / unpowered; QEMU raspi4b's hub-base read) →
    skip cleanly, graceful degradation.
  * **BUS-POISON** — `HUB_IDENT0` matches an open-bus / firmware-fill signature (`0xffffffff` or
    `0xdeadbeef`) → skip, **fail-closed**. This is the exact PI-V3D-1 metal value; it is now an
    ABSENT-DECODE verdict, never "present."
`is_poison()` mirrors `pcie_probe::is_poison` (the poison-rejection rule, cited above). Both BLOCK-DOWN
and BUS-POISON return **before any core-register access**, so neither can raise the forbidden
`AARCH64 EXCEPTION`. The probe retries a poison read within a short bounded settle window (finite off
CNTPCT) before declaring BUS-POISON, to allow a freshly powered block a moment to answer.

**Leg 2 — the enable-sequence gap (`mailbox.rs` + `v3d.rs`).** Root cause of the metal false-pass:
power (`SET_DOMAIN_STATE`) and rate (`SET_CLOCK_RATE`) both ACKed, but the V3D clock **gate was never
opened** — the RPi firmware treats rate and enable-state as independent, and `SET_CLOCK_RATE` does not
enable a gated clock. A new `mailbox::set_clock_state` (tag `SET_CLOCK_STATE`, `0x00038001`) opens the
gate explicitly and requires the firmware to confirm the clock **present AND active** before the probe
runs; a bounded settle then precedes the first register read. On metal the corrected sequence is
expected to read a real V3D identity (BLOCK-UP) instead of poison — **metal verification is deferred to
the next attended Pi sitting** (this is a QEMU/metal divergence class by construction; QEMU models the
tags and reports BLOCK-DOWN). Knob-off byte-identity to baseline is preserved: every change is inside
`#[cfg(feature = "v3d")]`-gated code.

### PI-V3D-3 — the PM / ASB enable step (the enable-sequence refinement after V3D-2's metal refutation)

**The V3D-2 metal verdict (2026-07-18, LC-metal R22, non-relitigable ground truth).** One boot,
`~/unaos-bench/pi-serial-2026-07-18-r22-v3d2.log`:
```
:: V3D: power domain 10 ON ::
:: V3D: clock id 5 rate set to 500000000 Hz ::
:: V3D: clock id 5 gate ENABLED (active) ::
:: V3D: probe verdict BUS-POISON — hub IDENT0 = 0xdeadbeef (open-bus/firmware fill, NOT a live
   register — the powered+clocked path did not bring the block up) — GPU bring-up skipped, fail-closed ::
```
Leg 1 (the poison-honest probe) **CONFIRMED** — `0xdeadbeef` is now correctly named open-bus and
fail-closes cleanly (no MMU-backstop halt, no exception, boot continues). Leg 2 (the clock-gate as the
enable fix) **REFUTED**: `set_clock_state` demonstrably worked (gate ENABLED/active) and the block still
did not decode. **Conclusion of record: the RPi firmware property-channel power+clock path is NOT
sufficient to decode the V3D block on BCM2711.**

**The ASB adjudication (Linux `drivers/soc/bcm/bcm2835-power.c` + `bcm2711.dtsi`, rpi-6.1.y).** On
BCM2711 the V3D power domain (`BCM2835_POWER_DOMAIN_GRAFX_V3D`) is brought up by
`bcm2835_asb_power_on(PM_GRAFX, ASB_V3D_M_CTRL, ASB_V3D_S_CTRL, PM_V3DRSTN)`. The PM_POWUP / inrush /
memory-repair core sequence (`bcm2835_power_power_on`) is **skipped on BCM2711** (`if (power->rpivid_asb)
return 0`) — the firmware already does it, which is why our mailbox `SET_DOMAIN_STATE` domain 10 ACKs.
What the firmware property path does **not** do, and what `bcm2835_asb_power_on` still runs on BCM2711,
is the missing piece:
  1. **Deassert the V3D reset** — set `PM_V3DRSTN` (bit 6) in `PM_GRAFX` (offset `0x10c` in the PM block
     at ARM PA `0xFE10_0000`), written with the **PM password** `0x5A000000` in the top byte.
  2. **Release the two async AXI bridges** — clear `ASB_REQ_STOP` (bit 0) in `ASB_V3D_M_CTRL` (offset
     `0x0c`) then `ASB_V3D_S_CTRL` (offset `0x08`), each written with the PM password, and wait for
     `ASB_ACK` (bit 1) to clear. **The V3D ASB registers live in the `rpivid_asb` block, not the legacy
     `asb` block** — in the DT the `pm` node's third reg range is `<0x7ec11000 0x20>` "rpivid_asb", and
     `bcm2835_asb_control` routes `ASB_V3D_{S,M}_CTRL` to `power->rpivid_asb` on BCM2711 (ARM PA
     `0xFEC1_1000`).

Both bases are inside the `boot.rs` L1[3] Device-nGnRnE window (`0xC000_0000–0xFFFF_FFFF`) — no new MMU
mapping. **The firmware power/rate/gate steps are KEPT** (ACKed-working, still necessary — they stand in
for the skipped PM_POWUP sequence); the PM/ASB step is **added after them**, before the probe.

**Implementation (`v3d.rs::enable_pm_asb`).** Announced-before-issue writes, PM-password discipline,
poison-honest readbacks at each stage, a finite CNTPCT backstop on each ASB `ACK`-clear wait. Best-effort
by design: a bridge that never ACKs (or reads poison) is logged and bring-up proceeds — the `IDENT0`
probe that follows is the real verdict gate (it BUS-POISONs honestly if the block still did not decode).
Nothing here can fault or hang, so QEMU (which models neither `rpivid_asb` nor V3D) stays on the honest
**BLOCK-DOWN**: the ASB reads return 0, `ACK` is already clear, no wait fires. **On metal the
discriminating expectation becomes BLOCK-UP** (a live V3D identity); if it still reads poison the probe
fail-closes with `BUS-POISON` and the raw IDENT word feeds the next refinement — that is honest data, not
a STOP.

The new QEMU chain (verbatim), between the gate-enable and the probe:
```
:: V3D: PM/ASB deassert V3D reset — PM_GRAFX 0x00000000 -> set PM_V3DRSTN (pw) ::
:: V3D: PM_GRAFX readback 0x00000000 ::
:: V3D: PM/ASB release V3D master (ASB_V3D_M_CTRL) — cur 0x00000000 -> clear ASB_REQ_STOP (pw) ::
:: V3D: PM/ASB V3D master (ASB_V3D_M_CTRL) readback 0x00000000 — ACK clear (bridge released) ::
:: V3D: PM/ASB release V3D slave  (ASB_V3D_S_CTRL) — cur 0x00000000 -> clear ASB_REQ_STOP (pw) ::
:: V3D: PM/ASB V3D slave  (ASB_V3D_S_CTRL) readback 0x00000000 — ACK clear (bridge released) ::
:: V3D: probe verdict BLOCK-DOWN — hub IDENT0 = 0x00000000 (block absent/unpowered; expected in QEMU raspi4b) ...
```

### PI-V3D-4 — the M2 MMU program that read back zero (fabricated register constants)

**The V3D-3 metal verdict (2026-07-18, LC-metal R22 sitting-2, non-relitigable ground truth).** The
PM/ASB step landed: the probe now reaches **BLOCK-UP** on silicon with a live V3D identity, and the
hub + core IDENT windows all read real values:
```
:: V3D: probe verdict BLOCK-UP — hub IDENT0 = 0x42554856 (live V3D identity) ::
:: V3D: HUB_IDENT1..3 = 0x000e1124 0x00000100 0x00000e00 ::
:: V3D: CTL_IDENT0..2 = 0x04443356 0x81001422 0x40078121 ::
:: V3D: MMU CTL=0x00000000 VIO_ADDR=0x00000000 DEBUG=0x00000000 (mapped 64 arena pages @ 0x154000) ::
:: V3D: M2 MMU program FAILED — halting bring-up (fail-closed) ::
```
So the block is powered, clocked, out of reset, bridged, and decoding — every IDENT is live — yet the
MMU register window at hub `+0x1200` reads all-zero after programming, and the enable-verify fails
closed.

**Root cause: the `V3D_MMU_CTL_*` bit constants and two MMU offsets in `v3d.rs` were fabricated, not
transcribed from `v3d_regs.h`.** The old constants placed the control bits at the *top* of the word
(`ENABLE=1<<31`, `PT_INVALID_ENABLE=1<<30`, `PT_INVALID_ABORT=1<<29`, `WRITE_VIOLATION_ABORT=1<<21`,
`TLB_CLEAR=1<<3`, `TLB_CLEARING=1<<2`). The real hardware layout is at the *bottom*:
`ENABLE=BIT(0)`, `PT_INVALID_ENABLE=BIT(16)`, `PT_INVALID_ABORT=BIT(19)`,
`WRITE_VIOLATION_ABORT=BIT(11)`, `TLB_CLEAR=BIT(2)`, `TLB_CLEARING=BIT(7)`. Consequences, exactly
matching the capture:
  1. The "enable" write (`0xE0200000`) set only **reserved** bits — real `ENABLE` (bit 0) stayed
     clear, so the MMU was **never enabled**.
  2. Reserved/undefined bits do not latch, so the `V3D_MMU_CTL` readback returns `0x00000000`.
  3. The verify `ctl & ENABLE(=1<<31)` reads zero → `program_mmu` fail-closes and halts M2. This is the
     verbatim `MMU CTL=0x00000000 … M2 MMU program FAILED` line — a pure software constants bug, not a
     silicon or bring-up defect.

Two register **offsets** were also off by a slot: `VIO_ADDR` pointed at `V3D_MMU_HIT` (`0x1208`) and
`DEBUG_INFO` at `V3D_MMU_VIO_ADDR` (`0x1234`). Corrected to the `v3d_regs.h` map
(`0x1230 ILLEGAL_ADDR · 0x1234 VIO_ADDR · 0x1238 DEBUG_INFO`). (The PTE bits `VALID=BIT(28)` /
`WRITEABLE=BIT(29)`, the `MMUC_CONTROL` bits, and `ILLEGAL_ADDR_ENABLE=BIT(31)` were already correct
and are unchanged.)

**Fix (`v3d.rs`).** The constants are now transcribed verbatim from `torvalds/linux`
`drivers/gpu/drm/v3d/v3d_regs.h` (with a comment recording the corrected slot map). `program_mmu` now
(a) prints the values it is about to program (`PT_PA_BASE`, `CTL`, `ILLEGAL_ADDR`), (b) issues a
`dsb sy` between the programming writes and the readback so the program→verify handoff across the async
AXI bridge is explicit, and (c) prints a single richer readback line — `CTL`, decoded `ENABLE=`,
`PT_PA_BASE`, `VIO_ADDR`, `DEBUG` — so the next metal verdict is one line. Fail-closed posture is
unchanged: if `CTL.ENABLE` still does not latch, M2 halts exactly as before and the SError-drain runs.

New witness lines (metal — QEMU stays BLOCK-DOWN and never reaches M2):
```
:: V3D: MMU program — PT_PA_BASE<=0x000000NN (pt@0xNNNNN) CTL<=0x00090801 ILLEGAL_ADDR<=0x800000NN ::
:: V3D: MMU readback CTL=0x000NNNNN (ENABLE=1) PT_PA_BASE=0x000000NN VIO_ADDR=0x00000000 DEBUG=0x00NNNNNN (mapped 64 arena pages @ 0x154000) ::
```
The programmed `CTL` value is now `0x00090801` = `ENABLE(0) | WRITE_VIOLATION_ABORT(11) |
PT_INVALID_ENABLE(16) | PT_INVALID_ABORT(19)`. **Metal-owed:** confirm `ENABLE=1` on the readback and
that `DEBUG_INFO` decodes a plausible VA/PA width + MMU version (M2 PASS), then M3.

### PI-V3D-5 — the M3 clear-job wrote nothing (two-class instrumentation)

**The V3D-4 metal verdict (boot-P1, 2026-07-20, LC-metal R23s1).** With PI-V3D-4's corrected MMU
constants the block now programs its MMU and M2 passes; M3 then fails:
```
:: V3D: verify mismatch at word 0 — got 0xdeadbeef expect 0x00a68cff ::
:: V3D: M3 clear-job did not verify ::
```
`0xdeadbeef` is the CPU-side sentinel `fill_target` pre-seeds before the kick, so the verify proves
the **GPU wrote nothing to the target** (or wrote elsewhere and DRAM at the target still holds the
sentinel). Two failure classes fit that single symptom and cannot be told apart off-metal:
  - **Class A — job never ran.** The CLE never accepted/executed the render list (a `CT1QBA/QEA`
    kick that the engine ignored, or `CTRUN` that never latched). The store never issued.
  - **Class B — job ran but the store landed off-target.** Either the store address faulted in the
    V3D MMU (wrote nowhere), or the placeholder RCL packet encoding stored to the wrong address (the
    packet field layout in `build_rcl` is an admitted attended-metal refinement, the *next* arc), or
    a stale CPU cache line masked a real GPU write on the verify read.

**Off-metal audit of the PI-V3D-4 MMU constants (no provable defect found).** Against BCM2711 V3D 4.2
(`v3d_regs.h` / `v3d_mmu.c`): the `PT_PA_BASE` load is `pt_paddr >> V3D_MMU_PAGE_SHIFT(12)` ✓; the
PTE encoding is `VALID(28) | WRITEABLE(29) | (phys>>12)` with an identity map (iova==phys) ✓; the
programmed `CTL` (`ENABLE(0) | PT_INVALID_ENABLE(16) | PT_INVALID_ABORT(19) | WRITE_VIOLATION_ABORT(11)`)
matches the register's bottom-of-word layout ✓; the CL/job addresses are V3D **IOVAs** that, under the
identity map, equal the ARM PA the arena lives at, and the arena's PTEs are the ones marked valid ✓;
the verify's `clean_invalidate_range` is safe because the target line was published *clean* by the
pre-kick `clean_range` (the clean half writes nothing back, the invalidate forces a DRAM re-load) ✓.
**No load-bearing MMU constant is provably wrong** — so this arc adds the discriminating
instrumentation rather than a speculative constant change. (The witness-only offsets `VIO_ADDR`/
`VIO_ID`/`DEBUG_INFO` are as PI-V3D-4 transcribed; they gate nothing.)

**Instrumentation (`v3d.rs::clear_job`, all reads — programs nothing new).** Around the CT1 kick:
  1. **Class-A discriminator.** Snapshot `CT1CS` immediately *before* the kick (`pre`), *after*
     writing `CT1QBA/QEA` with a `dsb` (`kicked`), and after the poll (`done`); read `CT1CA` (the
     CLE's current execution address). `CTRUN` latched-then-cleared **or** `CT1CA != BA` ⇒ the CLE
     executed; `CTRUN` never latched **and** `CT1CA == BA` ⇒ **CLASS-A JOB-NEVER-RAN**.
  2. **Class-B discriminator.** Read `V3D_MMU_CTL` and decode its hardware-set fault bits
     `PT_INVALID(20)`, `WRITE_VIOLATION(12)`, `CAP_EXCEEDED(27)`, plus `VIO_ADDR`/`VIO_ID`. Any fault
     bit ⇒ **CLASS-B MMU-FAULT** (the store faulted, wrote nowhere) and `VIO_ADDR` names where. No
     fault but the CLE ran and the target is still the sentinel ⇒ **CLASS-B RAN-NO-FAULT** (store
     landed off-target — the RCL-encoding case, the next arc). The cache sub-case is already defeated
     by the invalidated verify read.
  3. **SError-drain correlation.** A faulting V3D store can leave a latent async external abort that
     the global SError-drain would otherwise consume unlabelled at bring-up exit (or fire at the
     first timer tick). A `serror_drain_request("v3d: M3 clear-job kick window")` is issued right
     after the poll, so a `consumed N latent async abort(s) … [v3d: M3 clear-job kick window]` line —
     if any — is unambiguously correlated with the M3 store, not with M1/M2. Zero drained ⇒ the store
     raised no bus fault.

QEMU `raspi4b` models no V3D, so it stays on **BLOCK-DOWN** and never reaches M3 — the new lines are
metal-only by construction (the module is entirely `v3d`-feature-gated, so knob-off images are
byte-identical to baseline).

**Expected boot-P2 witness lines (metal).** Between the `M2 MMU PASS` line and the verify result:
```
:: V3D: M3 clue — CT1CS pre=0x……… kicked=0x……… done=0x……… CT0CS=0x……… CT1CA=0x……… (BA=0x……… EA=0x………) — <CLASS-A JOB-NEVER-RAN | CLASS-B MMU-FAULT | CLASS-B RAN-NO-FAULT | INDETERMINATE> ::
:: V3D: M3 clue — MMU_CTL=0x……… (PT_INVALID=n WRITE_VIOLATION=n CAP_EXCEEDED=n) VIO_ADDR=0x……… VIO_ID=0x……… ::
```
plus, iff the store faulted, one `:: SERROR-DRAIN: consumed N latent async abort(s) … [v3d: M3 clear-job kick window] … ::`. The class label + `VIO_ADDR`/drain trio route the next arc: Class A → CLE kick/ring shape; Class-B MMU-FAULT → the store address vs the arena map; Class-B RAN-NO-FAULT → the `build_rcl` packet encoding.

### PI-V3D-6 — the real render control list (the placeholder `build_rcl` convicted at boot-P2)

**The boot-P2 verdict (2026-07-20, LC-metal R23s1).** PI-V3D-5's discriminator returned **CLASS-B
RAN-NO-FAULT**: `CT1CS pre=0 kicked=0 done=0 CT0CS=0 CT1CA=0 (BA=…, EA=BA+0x1a)`, `MMU_CTL=0x00090801`
with every fault bit `0`. The CLE consumed the `0x1a`-byte list to completion with no MMU fault, but
the store never targeted the buffer. PI-V3D-4's MMU constants are exonerated (audited + metal-clean);
the admitted-placeholder `build_rcl` was the convicted culprit.

**What the placeholder got wrong, at the packet level.** It wrote a stream of bare opcode *bytes* with
**no field bit-packing at all**, plus several wrong opcodes and a structurally impossible render:
- **No field packing.** Each packet was `w.u8(opcode)` followed by a couple of raw `u16`/`u32`s. V3D
  packets pack named fields at specific bit offsets after the opcode byte; a bare stream sets none of
  them (sub-ids, BPP, format, stride, buffer-select all read as 0/garbage).
- **Wrong opcodes.** `114` was used for "clear colors" — `114` is *Blend Enables*; the clear color is
  sub-id 3 of `TILE_RENDERING_MODE_CFG` (`121`). `125` was used for "end-of-tile marker" — `125` is
  *Tile Coordinates Implicit*; the real End-of-Tile Marker is `27`.
- **Malformed STORE.** `STORE_TILE_BUFFER_GENERAL` (`29`, correct opcode) had the target address at
  the wrong byte offset (the address field is a full 32-bit slot at packet byte 9, XML `start=64`) and
  none of the Output-Image-Format / Memory-Format / stride / Buffer-to-Store fields — so even had a
  tile been rendered, the store had no valid destination format.
- **No supertile execution.** There was no `MULTICORE_RENDERING_SUPERTILE_CFG` and, fatally, no
  `SUPERTILE_COORDINATES` — nothing ever triggered a tile to render/store. The `0x1a` bytes ran to
  the `Halt` and wrote nowhere: exactly CLASS-B RAN-NO-FAULT.

**The fix — a correct V3D 4.2 two-level render list.** `build_rcl` now emits the real render-only
clear+store as Mesa builds it (`v3dX(emit_rcl)` + `emit_render_layer` +
`v3d_rcl_emit_generic_per_tile_list`), for a single 64×64 tile = single supertile, no binned geometry:
- **Main list** (`OFF_RCL`, what CT1 executes): `TILE_RENDERING_MODE_CFG` **Common** (64×64, 1 RT,
  32-bit BPP) → **Clear Colors Part1** (RT0 low-32 = `0x00A68CFF`) → **Color** (RT0 32-bit BPP,
  internal type `8` = rgba8 unorm, clamp none) → **ZS Clear Values** (ends config) →
  `TILE_LIST_INITIAL_BLOCK_SIZE` → `MULTICORE_RENDERING_TILE_LIST_SET_BASE` →
  `MULTICORE_RENDERING_SUPERTILE_CFG` (1×1) → the initial tile-buffer clear (the GFXH-1742 double
  dummy-store + `CLEAR_TILE_BUFFERS`) → `FLUSH_VCD_CACHE` → `START_ADDRESS_OF_GENERIC_TILE_LIST`
  (pointing at the sub-list) → `SUPERTILE_COORDINATES(0,0)` → `END_OF_RENDERING` (Halt).
- **Generic per-tile sub-list** (`OFF_SUBLIST`, branched to per supertile): `TILE_COORDINATES_IMPLICIT`
  → `END_OF_LOADS` → `PRIM_LIST_FORMAT` (triangles) → `SET_INSTANCEID(0)` → **`STORE_TILE_BUFFER_GENERAL`
  (RT0 → target, raster, rgba8, 256-byte stride, address at byte 9)** → `CLEAR_TILE_BUFFERS` →
  `END_OF_TILE_MARKER` → `RETURN_FROM_SUB_LIST`. `BRANCH_TO_IMPLICIT_TILE_LIST` is deliberately omitted
  (no binned geometry, so the tile-alloc base is never dereferenced).

Every opcode, field bit-offset, size, enum value and packet length is transcribed verbatim from Mesa
`src/broadcom/cle/v3d_packet_v33.xml` (`gen="3.3" max_ver="42"`); the packing convention (opcode byte
0, XML `start` bits relative to the bit after the opcode, length = `max(field_end)/8 + 1`) is from
Mesa `gen_pack_header.py`; the packet ordering is from `src/gallium/drivers/v3d/v3dx_rcl.c`. **Mesa is
MIT-licensed — verbatim-liftable with attribution** (contrast the Linux-kernel `v3d` GPL-2.0-only
sources, which remain facts-only; none are used here). The sub-list is `cache::clean_range`d for the
non-coherent GPU alongside the main list + target.

**CT1CA-reads-0 nuance (carried forward).** boot-P2 showed `CT1CA=0` *after* the malformed run. The
PI-V3D-5 clue decode treats `CT1CA != BA` as one "ran" witness; if `CT1CA` still latches `0` after this
*correct* list executes on metal, extend the clue-line decode (CT1CA may latch differently than
assumed) rather than reading it as job-never-ran — the CLASS label and `SUPERTILE`/store evidence lead.

**Expected boot-P3 lines (metal).** With the block **BLOCK-UP** on silicon: the M3 clue line should now
report **CLASS-B RAN-NO-FAULT retired** — `CTRUN` latched-then-cleared, no MMU fault — and the verify:
```
:: V3D: M3 clear-job PASS (GPU cleared buffer; CPU byte-verified) ::
```
i.e. every 32-bit word of the target reads `0x00A68CFF` (the first correct V3D-rendered pixels on the
Pi), with the panel blit as the visible witness. QEMU `raspi4b` still stops at BLOCK-DOWN (no V3D
modelled), so this list is correct-by-construction against the cited Mesa sources and refined at the
attended sitting.

### References of record

- Register layout: Linux `drivers/gpu/drm/v3d/v3d_regs.h` (hub + core + MMU offsets, field bits).
- V3D MMU: Linux `drivers/gpu/drm/v3d/v3d_mmu.c` (flat page table, PTE bits, flush sequence).
- Render-control-list packets: Mesa `src/broadcom/cle/v3d_packet_v33.xml` (4.2 encodings — the
  VC4-era packet numbers/sizes do **not** transfer) + `gen_pack_header.py` (the opcode-byte / bit-offset
  packing convention). **MIT — liftable with attribution** (PI-V3D-6).
- Render-control-list ordering: Mesa `src/gallium/drivers/v3d/v3dx_rcl.c` (`v3dX(emit_rcl)`,
  `emit_render_layer`, `v3d_rcl_emit_generic_per_tile_list`) — the packet sequence PI-V3D-6 follows.
- Structure reference: librerpi/lk-overlay `v3d.c`.
- PM / ASB power sequence (PI-V3D-3): Linux `drivers/soc/bcm/bcm2835-power.c` (`bcm2835_asb_power_on`,
  `bcm2835_asb_control`; `PM_GRAFX`/`PM_V3DRSTN`/`PM_PASSWORD`, `ASB_V3D_{S,M}_CTRL`/`ASB_REQ_STOP`/
  `ASB_ACK`) + `arch/arm/boot/dts/bcm2711.dtsi` (the `pm` node's `pm`/`asb`/`rpivid_asb` reg ranges).

### Gates green

`./arroyo check` both arches (knob on + off); `UNAOS_V3D=1 ./arroyo kernel8-test 43` = **MBENCH PASS
46/46, 0 forbidden, 0 AARCH64 EXCEPTION** (the probe degrades gracefully in QEMU); knob-off
`./arroyo kernel8-test 43` = 46/46; **knob-off `kernel8.img` byte-identical to baseline `03105f0`**
(sha256 recorded in the landing report). Positive V3D verification (power/clock/IDENT live, MMU
program, GPU clear, panel blit) is the **attended Pi sitting** — see `scripts/pi-v3d-bench.md`.

## PI-USB — BCM2711 PCIe root complex + VL805 xHCI attach on the Pi 4 (Arc PI-USB-1)

Every USB-A port on the Raspberry Pi 4 hangs off **one** endpoint: the VIA **VL805** xHCI (PCI
`1106:3483`), which sits behind the BCM2711's single PCIe root complex (`pcie@7d500000`,
ARM-physical `0xFD50_0000` in low-peripheral mode). Bringing USB up therefore means bringing PCIe
up first. `arch/aarch64/piusb.rs` (feature `piusb`, implies `baremetal`) does the whole chain to a
**halted-but-decoding + ports-powered honesty line**; full device enumeration (rings, `ADDRESS_DEVICE`,
HID/storage) is the attended-metal follow-on.

### QEMU can't model this (the by-construction caveat)

QEMU's `raspi4b` machine models **no PCIe root complex**. Everything past the RC identity read runs
**only on real silicon**. The bring-up is therefore correct-**by-construction** against the Linux
references below (the same discipline as **PI-V3D-1** and **ORIN-NET-3**), not QEMU-exercised. Do **not**
treat "no RC in QEMU" as a divergence.

**Census-before-touch (the anti-abort gate).** `build_boot_info` runs in `__rust_boot`, *before*
`kernel_main` installs the exception vectors. The BCM2711 RC aperture (`0xFD50_0000`) is inside the
`boot.rs` L1[3] Device window, so an **absent** read there is an *external abort*, not a translation
fault — and with no vectors installed that abort would kill the boot. (This is why the V3D probe, which
reads `0xFEC0_0000` — a *modeled* container that returns `0` in QEMU — survives, but a bare RC read does
not.) So `piusb::bringup` first does a minimal, self-bounded **flat-device-tree scan** for a `pcie@`
node and returns *before any RC MMIO* if none is present. QEMU raspi4b's DTB has no `pcie@` node → clean
skip; the Pi firmware DTB has `pcie@7d500000` → proceed. The scan touches only the DTB blob (RAM), never
the RC.

### The bring-up chain (`piusb.rs`)

- **M1 — brcmstb RC bring-up.** Absent-RC gate (`RGR1_SW_INIT_1` reads `0`/all-ones ⇒ skip), then the
  Linux `pcie-brcmstb.c` sequence: assert bridge core reset + PERST (`PCIE_RGR1_SW_INIT_1` bits
  INIT_GENERIC|PERST), release the bridge core, power up the serdes (clear `HARD_DEBUG.SERDES_IDDQ`),
  program the **inbound DMA BAR** (`RC_BAR2` → RAM base 0, 4 GiB) and the **outbound MEM window**
  (`CPU_2_PCIE_MEM_WIN0_*`: CPU `0x6_0000_0000` decodes PCIe `0xC000_0000`, 1 GiB — the canonical Pi 4
  `ranges`), deassert PERST, and **poll link-up** (`PCIE_MISC_PCIE_STATUS` PHYLINKUP|DL_ACTIVE) with a
  finite ~100 ms backstop. *(PIUSB-3, boot-P1 fix)* The two outbound-window register groups map **opposite**
  address spaces and must not be crossed: `WIN0_LO`/`WIN0_HI` hold the **PCIe-side** target address the
  window translates TO (`0xC000_0000`), while `BASE_LIMIT` + `BASE_HI` + `LIMIT_HI` hold the **CPU-side**
  address range the RC MATCHES against (`0x6_0000_0000 .. 0x6_3FFF_FFFF`, in 1 MiB units — `0x6000` MiB, so
  the high MiB-bits carry bit 34 into `BASE_HI`/`LIMIT_HI`, shifted right 12). boot-P1 had these swapped
  (CPU base in `WIN0_LO/HI`, PCIe range in `BASE_LIMIT`), so the RC never claimed `0x6_0000_0000` and the
  CAP read returned the master-abort fill `0xdeaddead`. Every window register (outbound five + `RC_BAR2`
  pair) is now **read back and witnessed** in the boot log (the ORIN readback ritual) with a `WIN0 armed:
  YES/NO` verdict line.
  An honest link-DOWN says so and returns — never a hang. Reads the root-port
  identity (expect Broadcom `0x14e4`) from RC config space.
- **M2 — VL805 enumeration.** Child config via the brcmstb `EXT_CFG_INDEX`/`EXT_CFG_DATA` window (bus 1,
  dev 0, fn 0): verify identity `1106:3483` (poison-rejecting), read class (expect `0c/03/30` USB xHCI).
  *(PIUSB-4, boot-P2 fix — device-side `0xdeaddead`)* **Order matters:** issue the **`NOTIFY_XHCI_RESET`**
  mailbox (tag `0x00030058`, `dev_addr = 0x0010_0000` = `(bus1<<20)|(dev0<<15)|(fn0<<12)`) **FIRST**, then
  size + assign BAR0 + enable decode. The notify makes the VideoCore firmware (re)load the VL805 firmware,
  which **resets the VL805's PCI config space** — BAR0 and COMMAND revert to power-on defaults. boot-P2 had
  the arm CPU-side window correct (`WIN0 armed: YES`) yet still read `0xdeaddead`, because the old order
  assigned BAR0 + enabled COMMAND and only *then* issued the notify, so the firmware reload wiped both and
  the VL805 decoded at BAR=0 (not PCIe `0xC000_0000`) → master-abort fill device-side. Linux avoids this by
  running the VL805 firmware load as a `DECLARE_PCI_FIXUP_HEADER` — *before* PCI-core resource assignment;
  we mirror that order. After the notify (+settle) the device identity is **re-read** (mailbox SUCCESS ≠
  firmware running — the config re-read is the proof), then the **BAR-sizing ritual** on BAR0 (all-ones
  probe + **immediate restore** — the ORIN-NET-3 pattern), BAR0 assigned to the outbound window's PCIe base
  with **`[0x14]` written explicitly (=0)** and **both dwords read back + witnessed**, MEM decode +
  bus-master enabled with **COMMAND read back + witnessed**. Three witness layers (BAR dwords / post-notify
  survival / COMMAND) name the failing layer if the wall persists.
- **M3 — xHCI attach.** Map the outbound window Device-nGnRnE via `boot::map_device_1gib` (one L1 block
  for CPU `0x6_0000_0000`; the **only** new page-table write this arc makes — outside `build_l1`'s fixed
  0–4 GiB map, reachable under the 36-bit IPS / 39-bit VA). Read `CAPLENGTH`/`HCIVERSION`/`HCSPARAMS1`
  (poison-rejecting), attach the shared `drivers/xhci` in **polled** mode (`xhci::init` = halt + HCRST +
  CNR wait — heap-free, no ring allocation), set `PORTSC.PP` on each root port, and **stop** at the
  honesty line. This is the JB2b platform-attach pattern (`xusb_tegra.rs`) adapted to a PCIe-BAR base.
  *(PIUSB-4)* The first CAP read is a **bounded settle+retry** (up to 8 tries, 5 ms apart — the ORIN
  readback-ritual idiom) so a just-enabled decode path that answers a few cycles late is not misread as
  poison; a still-poisoned read after the budget is an honest fail-closed, never a hang.

**Write discipline (the review lens).** The arc's writes are confined to the BCM2711 RC register block
and the VL805's own config/BAR; every BAR-sizing probe restores its original immediately; no other
device is touched. Every liveness read rejects both `0xffffffff` (PCIe master-abort / open-bus) and
`0xdeadbeef` (firmware fill) as ABSENT DECODE — the **PI-V3D-1 poison-rejection rule**.

### Byte-identity call sites

`piusb::bringup(dtb)` is called at the **end of `build_boot_info`** (with the DTB in hand); the map
helper is at **end-of-`boot.rs`**; the mailbox `NOTIFY_XHCI_RESET` tag/fn are gated additions. Each
sits where its gated insertion shifts **no** panic-location line numbers in baseline code — the knob-off
byte-identity guarantee (the V3D-call-site lesson).

### References of record

- brcmstb PCIe RC: Linux `drivers/pci/controller/pcie-brcmstb.c` (bridge sw-init/reset, `HARD_DEBUG`
  serdes power-up, `PCIE_MISC_PCIE_STATUS` link bits, `CPU_2_PCIE_MEM_WIN0` outbound window, `RC_BAR2`
  inbound window, `EXT_CFG_INDEX`/`EXT_CFG_DATA` child config). BCM2711 register offsets.
- VL805 firmware reset: the RPi firmware `NOTIFY_XHCI_RESET` mailbox tag (`0x00030058`).
- xHCI attach: the shared `drivers/xhci`, polled-aarch64 (the JB2b pattern, `arch/aarch64/xusb_tegra.rs`).

### Gates green

`./arroyo check` both arches; knob-off `./arroyo kernel8-test` = 0 FAIL and **`kernel8.img`
byte-identical to baseline** (sha256 in the landing report); knob-on `UNAOS_PIUSB=1 ./arroyo kernel8-test`
= 0 FAIL, DTB-census graceful skip, full suite reached (no pre-vector abort); `./arroyo test-arm 22`,
`UNAOS_GICV3=1 ./arroyo test-arm 40`, `./arroyo test 22` all unregressed. Positive verification (RC link
up, VL805 `1106:3483` found, BAR sized, xHCI decoding, ports powered) is the **attended Pi sitting** —
see `scripts/pi-usb1-bench.md`.

## PI-USB-2 — from the honesty line to device enumeration on the VL805 (Arc PI-USB-2)

Rung 1 stopped the VL805 xHCI at "halted-but-decoding + ports powered". Rung 2 adds the **DMA-side
bring-up + polled device enumeration**: it programs the controller's rings/DCBAA/interrupter (which need
the heap), runs it (RS=1), and lets the shared `drivers/xhci` driver walk whatever is plugged
(keyboard/mouse/storage) to per-device identity lines, arming a HID keyboard where present (the JB2b
keyboard pattern). Reuses the shared driver's polled-attach machinery verbatim — **zero `drivers/xhci`
core edits**.

### M1 encoding adjudication (`encode_ibar_size`) — the mandatory gate, resolved

The rung-1 lens flagged the `RC_BAR2` inbound-window size code as **0x11-vs-0x20 unresolved**. Rung 2
resolves it against the Linux programming model. `drivers/pci/controller/pcie-brcmstb.c`'s
`brcm_pcie_encode_ibar_size(u64 size)` maps a **byte size** to the 5-bit size field
(`PCIE_MISC_RC_BAR2_CONFIG_LO_SIZE_MASK = 0x1f`, bits `[4:0]` of `CONFIG_LO`) by branch on `ilog2(size)`:

```
log2 in [12,15]  (4 KiB .. 32 KiB)   -> (log2 - 12) + 0x1c
log2 in [16,35]  (64 KiB .. 32 GiB)  -> log2 - 15
otherwise                             -> 0 (disabled)
```

A 4 GiB inbound window is `size = 2^32`, so `log2 = 32`, which lands in the **[16,35]** branch:
`code = 32 - 15 = 17 = 0x11`. **So `0x11` is CORRECT for a 4 GiB window**; the alternative `0x20`
(`= 32` decimal, the raw `log2`) is out of the field's meaning and **WRONG**. The rung-1 constant
`RC_BAR2_SIZE_4G = 0x11` is right and is **unchanged** — the comment at `piusb.rs` step (e) now states this
source-derived rule verbatim. (The full `CONFIG_LO` value written is `(RAM base low 32b) | 0x11`; with RAM
base 0 that is `0x11`, and `CONFIG_HI = 0`.)

### The whole-RAM inbound window — the no-IOMMU DMA threat (carried forward)

`RC_BAR2` maps the PCIe inbound window to **system RAM base 0, 4 GiB** — the entire address space a
bus-master device can DMA into, with the Pi 4 having **no IOMMU** in this path. Once bus-master is enabled
(M2) and the controller is running (rung-2 M3), the VL805 (and anything behind its USB ports, via the
xHCI's scatter-gather) can read/write **any** physical RAM the rings point it at. This is the standing
threat of record for the Pi USB path: the safety here is that the driver programs only its own
heap-allocated, identity-mapped ring/DCBAA/buffer structures as DMA targets, and every liveness read is
poison-rejecting — but there is **no hardware translation/containment** backstopping a driver bug. A
future IOMMU/least-privilege inbound window is the hardening item (tracked in the landing report).

### The DMA-side chain (`piusb::enumerate`, post-heap)

- **Handoff.** Rung-1 `bringup` (pre-heap, in `build_boot_info`) reaches the honesty line and stashes the
  decoding CPU-side xHCI base + a ready flag in two module statics. `enumerate()` (post-heap, on the BSP in
  `kernel_main`) reads them; if the honesty line was never reached (QEMU census-skip, link-down, BAR
  mismatch) it says so and returns — the **exact rung-1 graceful-degradation carried one rung forward**
  (QEMU never builds a ring).
- **PORTSC PED-mask robustness nit (M2).** The rung-1 port-power RMW masked off the RW1C change bits before
  OR-ing in `PP`. Rung 2 also masks **PED** (Port Enabled/Disabled, bit 1): PED is RW1CS — writing it back
  as 1 *disables* the port. On a warm/already-enabled port a naive RMW would tear the port's own enable
  down. Masking PED off makes "power on" disturb nothing (hardware sets PED itself on a successful reset).
- **Rings + RS=1 (M3).** `xhci::init` (halt + HCRST + CNR wait — this is **OUR** freshly-reset controller,
  so the plain reset path, **not** the inherited-controller no-HCRST/CRCR takeover the Orin uses), then
  `XhciController::new` + seat `COMMAND_RING`/`EVENT_RING`/`ERST_TABLE` + `init_interrupter` /
  `init_pointers` / `start()`. Then a **bounded** polled-enumeration pump (`poll_events` + `service_hubs` +
  `service_hid_setproto` + `service_slot_disposal` + `service_enum` + `service_storage`), exiting early at
  keyboard-ARMED (plus a short storage settle) or a ~30 s worst-case backstop. Per-device identity lines
  (`port_slot_summary` + `usb_summary`) are printed so the boot's serial names the topology it reached.

### Byte-identity — the rung-2 call-site reality

Rung 1's call sites were chosen to shift **no** panic-location line numbers (end-of-function / end-of-file),
giving strict byte-identity. Rung 2's `enumerate()` **must** be called post-heap from inside `kernel_main`
(the heap + BSP context live only there), and **any** insertion into `kernel_main` shifts the source lines
of every item below it. The knob-off `kernel8.img` therefore differs from baseline by **exactly one byte**:
the `core::panic::Location.line` u32 of an *unrelated* `assert!` in `input_service` (`1840 -> 1848`, +8 for
the 8-line gated insertion). **All machine code and data are identical** — the delta is a single embedded
source-line number, not any code or behavior. Knob-off, `piusb::enumerate` compiles out entirely and the
full kernel8 battery is 0 FAIL. (Verified: `cmp -l baseline mine` = 1 byte at the `Location.line` field.)

### Expected metal chain (attended Pi sitting)

```
RC link up -> VL805 1106:3483 found -> BAR0 sized + assigned -> xHCI DECODING (CAPLENGTH/HCIVERSION) ->
ports powered -> [rung 2] rings+interrupter, RS=1 -> port connect(s) -> ADDRESS_DEVICE -> device identity
line(s) -> keyboard ARMED (if a HID keyboard is plugged)
```

### Gates green

`./arroyo check` both arches, knob on **and** off; knob-off `./arroyo kernel8-test` = 0 FAIL (full 35 s
battery: K3-mount `w=0x1ff`, CAPSTONE 6/6) with the 1-byte panic-`Location` delta above (functional
byte-identity); knob-on `UNAOS_PIUSB=1 ./arroyo kernel8-test` = 0 FAIL with **both** graceful census-skip
lines (`bringup` DTB-skip + `enumerate` honesty-line-not-reached skip — the DMA-side path census-skips in
QEMU exactly as rung 1 does); `./arroyo test-arm 22`, `UNAOS_GICV3=1 ./arroyo test-arm 40`, `./arroyo test
22` all unregressed. Positive verification (rings/RS=1, live device enumeration, keyboard armed) is the
attended Pi sitting — see the rung-2 runbook in `scripts/pi-usb1-bench.md`.

## PI-GENET — BCM2711 on-board Gigabit Ethernet (Broadcom GENET v5) + smoltcp bind (Arc PI-GENET)

The Pi's **first network path.** The BCM2711 integrates a Broadcom "GENET" v5 unimac Ethernet
controller (`ethernet@7d580000`, `brcm,bcm2711-genet-v5`) driving an external BCM54213PE RGMII PHY.
`UNAOS_GENET=1` (the `genet` cargo feature, default OFF) arms `arch/aarch64/genet.rs`: it DTB-resolves
the register base, poison-honest classifies the platform, brings up the UMAC + PHY + TDMA/RDMA
descriptor rings per the Linux `bcmgenet` v5 programming model, reads the station MAC, and binds a
`smoltcp::phy::Device` over the rings through the shared `net_phy` seam (the third rider, after
AARCH64-VNET and ORIN-NET-4). It is **code-complete-prior-to-metal** (like ORIN-NET-4): positive
link/DHCP verification is the attended Pi-4 sitting.

### The load-bearing finding — QEMU 11.0.1 raspi4b does NOT model GENET

The M1 empirical question ("does QEMU model GENET?") is settled on the bench, not assumed. QEMU
`raspi4b` (bcm2838 SoC) does **not** model the GENET block, and — unlike an x86 open-bus read — an
access to the unmodeled register window at ARM-physical `0xFD58_0000` raises a **synchronous external
Data Abort** (`ESR=0x96000010`, EC=0x25, DFSC=0x10, `FAR=0xfd580000`), NOT a poison read. QEMU also
hands `-kernel` boots **no usable DTB** (x0=`0x100`, size 0), so there is no `ethernet@7d580000` node
to resolve either.

Because an unmodeled read *faults* (it does not return `0xffffffff`), the classification is **DTB-gated
before any MMIO**, the exact discipline **PI-USB** uses (piusb's `dtb_has_pcie` guard — QEMU raspi4b
models no PCIe RC either, and touching `RC_BASE` blind would fault). M1 resolves the GENET node from
the live firmware DTB and touches the register window **only** if the DTB actually describes one; a
poison-honest `SYS_REV_CTRL` read then guards against a link-down/absent decode on real metal (the
standing "read before the first write" law — the PI-V3D-1 / FAULT-AT-M1 lesson). On QEMU (no DTB node)
the driver records an honest compiled-present line and returns **before any MMIO** — it never
dereferences an unmodeled window. First metal run of the fault-forward path (the original
documented-fallback probe *did* fault the BSP at `0xFD58_0000`; the DTB-gate fixed it in-arc).

### MAC source

The station MAC comes from the DTB `local-mac-address` property of the GENET node (the RPi firmware
fills it) when present; otherwise the driver falls back to reading the UMAC `MAC0`/`MAC1` registers
(the firmware programs them at boot). The boot log states which source it used.

### Register / datapath model (Linux `bcmgenet` v5)

Sub-blocks in the 64 KiB window: SYS (`0x0000`), EXT (`0x0080`), RBUF (`0x0300`), UMAC (`0x0800`),
RDMA descriptors (`0x2000`) + ring/global regs (`0x2C00`), TDMA descriptors (`0x4000`) + ring/global
regs (`0x4C00`). The driver uses the default descriptor ring index 16 (`DESC_INDEX`) for both RX and
TX, exactly as `bcmgenet_init_dma`. The datapath is a **producer/consumer-index** ring (not the RTL8168
per-descriptor OWN handoff): the driver advances the TX producer index after posting and the RX
consumer index after draining; hardware advances the mirror index. Bring-up order follows
`bcmgenet_open` / `init_umac` / `init_dma`: SYS port mode → UMAC soft reset + RBUF/TBUF flush → MAC +
max-frame-len → MIB reset → RBUF 64B/align → RGMII OOB (mode-enable only) → RX/TX rings → `UMAC_CMD`
TX/RX enable + promiscuous bring-up filter. Interrupts masked (polled). Every register write is
announced on serial before issue. The GENET register window lands in the `0xC000_0000..0xFFFF_FFFF`
Device GiB `boot::build_l1` already maps, so — unlike piusb's outbound window / NET-4's iATU — no new
page-table write is needed once resolved.

### Autoneg-honest link (PI-GENET-2) — speed/duplex/link taken FROM the PHY, never forced

The M2 bring-up **does not** hand-assert `SPEED_1000` in `UMAC_CMD` or `RGMII_LINK` in
`EXT_RGMII_OOB_CTRL`. Instead, after `find_phy`, `phy_resolve` reads the external BCM54213PE's
negotiated result over MDIO — all IEEE 802.3 Clause-22 standard registers: `BMSR` (link + autoneg-
complete), then the highest-common-denominator technology from our advertisement (`ANAR` 0x04 /
1000BASE-T control 0x09) intersected with the link partner's ability (`ANLPAR` 0x05 / 1000BASE-T
status 0x0a). `mac_set_from_link` then programs the `UMAC_CMD` speed bits + `CMD_HD_EN` from that
resolution and sets `RGMII_LINK` **only** when the PHY reports link (the Linux `bcmgenet_mii_setup`
discipline). Rationale: forcing `SPEED_1000` while the PHY negotiated 100M mis-clocks the RGMII pins,
so every TX frame is garbage on the wire — the MAC transmits but no peer parses a valid frame (the
observed "no DHCP OFFER, solid-yellow transport LED, steady beat on the Mac end" signature). Until the
PHY resolves, `UMAC_CMD` speed is left at the 10M floor rather than a fast lie.

### TX evidence (storm / LED-red-herring classes)

After the DHCP + ping exchange, `tx_evidence` logs the software frames-enqueued count against the
hardware TDMA producer/consumer indices (`RING_TDMA_PROD_INDEX` / `RING_TDMA_CONS_INDEX`, hardware-
advanced as it drains descriptors). `cons == prod` with a small count means the ring fully drained
with no runaway re-post — a steady activity LED under those readings is a benign gigabit-link
indication, not a TX storm. The driver's `transmit` bumps the producer index exactly once per frame
(no retransmit loop); DHCP retransmits originate in smoltcp and are bounded by the 5 s lease timeout.

### DMA / identity-map + coherency

Rings + buffers are heap-allocated; the Pi bare-metal MMU maps RAM identity (VA==PA), so the pointer
doubles as the DMA physical address (the x86 e1000 / NET-4 / VNET invariant). Published with `dsb sy`.
The BCM2711 GENET is I/O-coherent toward DRAM; if attended metal ever shows stale descriptors on a live
link, the fix is clean-before-own / invalidate-before-read on the rings + buffers — **not** a weakening
of the index protocol.

### Serial witness

```
:: PI-GENET: BCM GENET v5 GbE bring-up (DTB @<x0> size=<sz>) ::
# QEMU raspi4b (the classification, fault-free):
:: PI-GENET:   no DTB handed off (x0=0x100, size=0x0) — no GENET node to resolve ::
:: PI-GENET:   GENET driver compiled-present; no GENET node in the DTB (QEMU raspi4b models no GENET,
               or a DTB-less boot) — bring-up SKIPPED before any MMIO ... ::
# Real Pi-4 metal (attended, expected chain):
::   DTB GENET node reg child base 0x7d580000 -> ARM-physical 0xfd580000 (SoC ranges +0x80000000) ::
::   SYS_REV_CTRL = 0x.......6 — LIVE GENET v5 ...; this build MODELS the block ::
::   station MAC = <mac> (source: dtb local-mac-address | umac-reg readback) ::
::   M2 bring-up ... rings up: RX/TX ring 16 (32 desc each); UMAC_CMD readback ... (live) ::
::   PHY autoneg resolved (MDIO addr 1): link UP · aneg COMPLETE · speed 1000M · full-duplex ::
::   >>> REG WRITE (M2): UMAC_CMD speed<-1000M full-duplex (from PHY autoneg) ::
::   >>> REG WRITE (M2): EXT_RGMII_OOB_CTRL RGMII_LINK SET (honoring PHY link) ::
:: PI-GENET ping <gw> (4/4 sent, N/4 replies) [dhcp] link UP => PASS ::
::   TX evidence [post-DHCP+ping]: sw frames-enqueued=N (tx_prod=N) | HW TDMA prod_index=N cons_index=N (drained; no storm) ::
```

If the PHY instead resolves `speed 100M`, the witness makes the previous forced-1000 mismatch visible
directly, and the MAC is programmed for 100M so the DISCOVER goes out valid — the class-1 fix. A
`link DOWN` resolution clears `RGMII_LINK` and the bring-up is an honest bounded no-op.

### Gates green

`./arroyo check` both arches, knob on **and** off; knob-off `./arroyo kernel8-test` = 0 FAIL with
functional byte-identity to baseline (text/data/bss sizes identical; symbol tables identical after
stripping build-path-dependent `.llvm.<hash>` suffixes — the PI-USB-2 build-path-metadata precedent);
knob-on `UNAOS_GENET=1 ./arroyo kernel8-test` = 0 FAIL with the honest DTB-gated skip (BSP survives to
the shell, CAPSTONE 6/6, no exception); `./arroyo test-arm 22`, `UNAOS_GICV3=1 ./arroyo test-arm 40`,
`./arroyo test 22` all unregressed. Positive verification (live link autoneg → MAC → DHCP lease on the
bench LAN → ping) is the attended Pi sitting — see `scripts/pi-genet-bench.md`.
## INSTALL-PI — the installer engine's first LIVE install, on the Pi 4 emmc2 microSD (Arc INSTALL-PI)

`UNAOS_PIINSTALL_CONFIRM=1` (feature `piinstall_confirm` ⇒ `piinstall_arm` ⇒ `piinstall` ⇒ `baremetal`),
default OFF. Glue: `crates/kernel/src/install/pi.rs` (`EmmcInstallTarget` + the three-gate flow), called from
the Pi BSP boot path (`main.rs`, immediately after `emmc2::probe`). This is rung 4's Pi target from the
installer line — and the installer engine's **first full end-to-end execution that needs no bench**.

### Why the Pi is the first LIVE install

QEMU's `raspi4b` **models the BCM2711 SD controller and attaches a real emulated card image** (the in-tree
`drivers::emmc2` SDHCI driver — the same one M6g census + U9/U10 write — drives it, and QEMU exercises the
LEGACY-Arasan leg where the `if=sd` card sits). So unlike ORIN-INSTALL-1 (metal-only — QEMU models no
Tegra234 SDMMC), the whole engine flow (GPT → FAT32 → payload → sha extent-verify) actually runs and PASSes
under CI-grade QEMU against a real (emulated) card.

### `EmmcInstallTarget`

An `InstallTarget` (the arch-neutral engine's seam) over `drivers::emmc2`: `read_sectors` / `write_sectors`
loop the proven single-block `read_block_512` / `write_block_512` (CMD17/CMD24) primitives — bounded
multi-sector looping, exactly as the Orin `SdInstallTarget` loops its single-block path. The engine
(`write_gpt` / `format_esp` / `write_payload_file` / `verify_extents`, `blank_region_sectors`) runs verbatim;
no engine change was needed.

### The three-gate escalation (the seated card is sacred)

On metal the Pi's seated card holds the running system, so the destructive install stands behind three gates
mirroring `sdmmc / sdmmc_arm / install_target`, plus an ABOUT-TO-DESTROY announcement:

- **Gate 1 `piinstall`** — census the card (read-only) + announce identity/capacity/sector-0 class. No write.
- **Gate 2 `piinstall_arm`** — arm the write path: a NON-destructive scratch write/verify/restore ladder on
  the card's LAST block (stashed + restored, every step verified; REFUSED if a GPT is present, whose backup
  header lives in that block).
- **Gate 3 `piinstall_confirm`** — the destructive-confirm gate: the about-to-destroy line, then
  GPT → zero-ESP-metadata → FAT32 → payload → sha extent-verify → `:: INSTALL: pi emmc2 gpt+fat32+copy verify => PASS ::`.

Each feature implies the previous; a plain build compiles NONE of it (module + call site + engine all vanish),
so all machine code + data are unchanged — the only possible delta from baseline is embedded
panic-`Location.line` u32s shifted by the 8-line gated insertion in `kernel_main` (the PI-USB precedent), never
code or behavior.

### QEMU-live witness + host-side verification

`raspi4b` has one SD slot, so the witness run is its OWN QEMU invocation with a **dedicated BLANK scratch
image** in the slot — NEVER the `kernel8-test` battery fixture (which carries HELLO.BIN / the unafs volume the
battery reads back). `./arroyo kernel8-install [secs]` arms all three gates, generates a blank 128 MiB scratch
image, boots against it, and then **re-reads the scratch image ON THE HOST** — protective-MBR (0x55AA + 0xEE),
primary GPT `EFI PART` @LBA1, and the FAT32 boot sector at the ESP's first LBA (`FAT32` fs-type + `UNAOS`
volume label + 0x55AA) — the installer's claim verified from outside the kernel.

Witness of record (2026-07-18, `raspi4b`, blank 128 MiB scratch):

```
:: PIINSTALL: Gate 1 census — target = Pi emmc2 microSD (262144 x 512B sectors), capacity 262144 blocks (128 MiB), sector-0 = unknown (no recognised signature) ::
:: PIINSTALL: Gate 2 scratch ladder — write/verify/restore/verify at LBA 262143 => PASS ::
:: PIINSTALL: ABOUT TO DESTROY: microSD sector-0 = unknown … — the entire card is about to be repartitioned ::
:: PIINSTALL:   GPT written + parse-back verified — ESP LBA 2048..133119, data LBA 133120..262110 of 262144 sectors ::
:: PIINSTALL:   ESP formatted FAT32 — fat_sz=1016sec clusters=129008 data@vol+2064 ::
:: PIINSTALL:   copied UNAOS.IMG (4096 bytes, 8 extents) ::
:: PIINSTALL:   extent sha-verify (re-read every written extent off the card) => PASS ::
:: INSTALL: pi emmc2 gpt+fat32+copy verify => PASS ::
── host-side ──  PASS protective MBR / EFI PART @LBA1 / FAT32 fs-type / UNAOS label / 0x55AA  → HOST-VERIFY: PASS
```

### Payload adjudication (M2) + metal follow-up

A Pi "install" payload is ultimately the boot volume's FAT files (kernel8.img / start4.elf / config.txt — what
the GPU ROM loads). At the pre-shell BSP call site those are not reachable as a readable clone source, so v1
writes a generated `UNAOS.IMG` marker (honest-and-sufficient for the QEMU witness) and the **self-clone of the
boot FAT files is the named metal follow-up** (the Pi analogue of INSTALL-2). On real hardware the seated card
IS the running system's card: the three gates + about-to-destroy line are exactly that guard, and a metal
install leg wants a dedicated erasable card, never the boot card.

### Gates green

`./arroyo check` both arches; the `piinstall` / `piinstall_arm` / `piinstall_confirm` knob matrix all compile;
knob-off `./arroyo kernel8-test 35` = 0 FAIL (CAPSTONE 6/6 — functionally byte-identical, module compiled
out); the `./arroyo kernel8-install` live witness = in-kernel PASS + host-side `HOST-VERIFY: PASS`;
`./arroyo test-arm 22`, `UNAOS_GICV3=1 ./arroyo test-arm 40`, `./arroyo test 22`, and the `UNAOS_INSTALLDEMO`
engine witness all unregressed. Landing: `review/unaos-install-pi-LANDING.md`.
