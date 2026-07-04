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

**No scheduler on the `virt` path yet.** The aarch64 scheduler is
`#[cfg(feature = "baremetal")]`-gated and coupled to EL1 (`ELR_EL1`/`SPSR_EL1`
eret paths), while the `virt` kernel runs at EL2. Un-gating it needs a `virt`
**EL2 → EL1 drop** mirroring the Pi's `boot::drop_to_el1`, after which the
scheduler un-gates as-is and CAPSTONE can run there — the **JC3** candidate. So
the JC2 secondaries only receive SGIs; they run no scheduled work. The per-core
generic-timer tick is likewise deferred: arming it on a secondary would
double-count the shared `ticks()` clock the xHCI/e1000 timeout budgets read.

### Build knob

`UNAOS_GICV3=1 ./arroyo {arm,test-arm}` appends `-machine gic-version=3` to the
aarch64 `virt` QEMU invocation (QEMU takes the last value of the repeated
machine property). Default stays GICv2. The knob only affects the `virt` runs;
the Pi bare-metal paths (`kernel8*`, QEMU `raspi4b`) are always GICv2.

### Orin metal pending

All GICv3 **and PSCI SMP** work to date is **QEMU `virt` (`gic-version=3`)
only**. The Jetson Orin Nano's on-silicon GICv3 + PSCI — real Redistributor
frames at the Tegra234 base, cluster affinities in the target MPIDR, and the
board firmware's own PSCI conduit re-confirmed against its DTB — is **not yet
brought up**; that is a later, operator-attended arc. The `virt` PSCI `CPU_ON`
bring-up above is written to make it a small delta (swap the GICR base + confirm
the conduit; the capture/replay and per-core GICv3 init carry over). Nothing here
should be read as a metal claim: QEMU models no Tegra machine, and QEMU-green
does not imply Orin-correct.

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

### JM4 result — Orin GIC-600 + generic-timer interrupt (single core; QEMU-green, **metal PENDING**)

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

**Metal PENDING (Part D, operator-attended):** boot `./arroyo esp-jetson` (non-
BOOTDIAG — the bootdiag `fdt` panic is the separate flagged follow-up) media over the
RPi Debug Probe, capture → `unaos/target/serial-orin-jm4.log`. **Success sequence:**
the JM3 mmu lines → `boot diag EL=2 CNTFRQ=31250000` → `exception vectors installed
(VBAR_EL2=…)` → `GICv3 init (GICD=0xf400000, GICR=0xf440000, …)` (proves the Tegra234
bases resolved + the boot-core redistributor was found) → `GIC self-SGI delivered
(v3)` (SGI delivery on Orin) → `generic timer armed … INTID 30` → `timer diag …
ISTATUS=1 … PPI30 pending=1` → `timer LIVE: IRQ delivery confirmed; idle = WFI` →
`:: tegra: heartbeat N (ticks=…, live) ::` climbing. **Degraded-but-alive**
(acceptable, records the risk): up to `timer diag`, then `timer NOT live … poll-spin
fallback`, then the CNTPCT-driven fallback beat. **Failure:** the redistributor-walk
panic printing the MPIDR affinity (wrong base/stride) — a clean syndrome. No metal
claim is recorded until the capture shows it.

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
* **GICR base *and* frame stride.** ✅ **boot core done in JM4 (QEMU-green; metal
  pending)** — `gic.rs` now selects the Tegra234 GIC-600 bases (GICD `0x0F40_0000`,
  GICR `0x0F44_0000`) for the `tegra` build and derives the redistributor stride from
  `GICR_TYPER.VLPIS` (bit 1) instead of the QEMU-`virt` hardcodes, so the boot-core
  redistributor walk resolves on Tegra234. **Still deferred to the SMP arc:** the
  parallel `smp_virt.rs` walk still hardcodes the virt `GICR_BASE`/stride, and the
  target MPIDR must be widened to real cluster affinities (Aff1-3) for
  `IROUTER`/`ICC_SGI1R` — none of which JM4 touched (single core only).
* **PSCI conduit from the real DTB.** JC2 confirmed QEMU's conduit is **SMC** via
  `dumpdtb`, but the DTB **parse path is latent** — only the SMC-assumed fallback
  has ever run (the `virt` log prints `method not in FDT, assumed`). On the Orin,
  parse `/psci method` from the firmware DTB and honor it (`smc` vs `hvc`).
* **`SEC_CTX` must add `HCR_EL2`.** The capture/replay of the BSP's live EL2 state
  omits `HCR_EL2`; `E2H` is not guaranteed 0 on all silicon, and a reset AP coming
  up with a different `E2H`/`TGE` than the BSP would interpret the replayed
  `TCR`/`TTBR`/`SCTLR` under the wrong translation regime. Add `HCR_EL2` to the
  captured set.
* **`CPTR_EL2` replay + AP pre-MMU stack spill → asm stub.** Both currently rely on
  compiler fortune (the `CPTR_EL2` replay runs in Rust before the first FP/NEON
  instruction; the pre-MMU stack spill assumes the compiler doesn't touch the stack
  first). On real silicon make these **structural** — do the `CPTR_EL2` write and
  the stack setup in the AP entry asm stub, before any compiler-generated code runs.
* **Per-AP SGI attribution via distinct INTIDs.** The cross-core SGI proof is "at
  least once" (GICv3 coalesces) because every AP uses one INTID; on metal, give each
  AP a distinct SGI INTID so per-core delivery is individually attributable.
