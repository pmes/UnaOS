# UnaOS milestones

A running, quick-to-digest log of what landed each integration round — one
entry per arc, newest first. Each entry: **what it does**, **how it was tested**
(QEMU + metal), and the commit. Deep detail lives in the per-subsystem docs
under [`dev/OS/`](dev/OS); the ledger of hardening state is in
[`SECURITY.md`](SECURITY.md); direction is [`ROADMAP.md`](ROADMAP.md).

Legend: **✅ metal-confirmed** · **🔬 QEMU-green, metal pending** · dates ISO.

---

## Round 6 — 2026-07-04 (landed on track branches; awaiting integration)

### U3 — per-process address spaces (CR3) (x86) 🔬 `hw-rmbp`
- **What:** each ring-3 process now runs in its OWN top-level page table (its own
  CR3) instead of sharing one user window — the x86 mirror of aarch64 M6d. A static
  8-slot pool of page tables each SHARES the kernel half (copies every kernel PML4
  entry except the user-window slot, so the identity map / MMIO / heap / kernel
  stacks are shared) and owns a PRIVATE user window at USER_BASE. The scheduler
  installs a task's CR3 before dropping to ring 3 and restores the kernel CR3 +
  frees the slot on exit. Two processes can map the same address to different
  memory. Plain `mov cr3` (full TLB flush) — PCID (the x86 ASID analogue) deferred.
- **Tested — QEMU:** `./arroyo test 25` → a deterministic kernel isolation probe
  (two spaces, same VA, distinct sentinels, swap CR3 and read → distinct) PASS, and
  two ring-3 tasks each in their own CR3 each read their own private sentinel PASS;
  U1a/U1b/U2/U2.5 byte-identical, no reboot loop; coexists with the U2 disk loader
  (`UNAOS_FATIMG=1`), the U2.5 FTDI console (`UNAOS_USBSERIAL=1`), and the FAT
  regression (`test-fat part`/`sf`). `./arroyo check` both arches; 0 aarch64 files.
  4-lens adversarial review → 0 confirmed findings.
- **Metal — PENDING:** rides the next rMBP reflash (FTDI cable day, ~2026-07-08).
- **Commits:** on `hw-rmbp` (see landing report); unmerged (Fable credits out).

## Round 5 — 2026-07-03

### M6g — load a program from storage (aarch64) ✅ `hw-pi4`
- **What:** the Pi twin of x86 U2 — the first *program loaded from the microSD the
  Pi booted from* into the EL0 boundary. A block-layer backend seam lets the
  read-only path dispatch to a new BCM2711 EMMC2/SDHCI microSD driver (PIO,
  single-block CMD17, polled, no writes) beside the untouched xHCI path; the driver
  probes **EMMC2 first, legacy Arasan second** (the reverse of QEMU, so the metal
  base is the first tried). The loader mounts the card's FAT volume, reads
  `HELLO.BIN`, size-checks it, copies it into a fresh per-task M6d slot (EL0-RX/EL1-RO
  before the task exists), and runs it at EL0 (`hello from EL0`). The loaded bytes are
  untrusted — bounded only by size, contained by EL0 + per-page perms + the M6b
  fault-kill net.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → `SD card @0xfe300000 identified —
  131072 blocks (64 MiB, CSD v1)`, `FAT mounted from SD (Fat32)`, `HELLO.BIN loaded
  from SD (51 bytes) -> EL0`, second `hello from EL0`, `disk-loaded EL0 program exited
  ok -> PASS`, with every prior milestone byte-identical and 0 unexpected faults; the
  `UNAOS_SDIMG=0` no-SD control adds exactly the two no-card lines + the loader-skipped
  line. x86 (`test` + `UNAOS_FATIMG=1 test`) byte-identical (seam inert); `check` both
  arches; aarch64 virt v2 + GICv3 JC2 SMP 3/3 unchanged.
- **Tested — metal (real Pi 4, 2026-07-04):** ★ the driver's EMMC2-first success leg —
  the one QEMU physically cannot exercise — ran on silicon: **no fallback line**, `SD card
  @0xfe340000 identified — 31116288 blocks (15193 MiB, CSD v2)` (the real ~16 GB microSD,
  SDHC/block-addressed — vs QEMU's 64 MiB/CSD v1 legacy fallback), then `FAT mounted from
  SD (Fat32)`, `HELLO.BIN loaded from SD (51 bytes) -> EL0`, second `hello from EL0`,
  `disk-loaded EL0 program exited ok -> PASS`. This reflash also carried M6f's pending
  metal: all three M6f verdicts PASS and the per-task preempt rider went > 0 on silicon
  (`spsentinel=3`); M6b `exited=1 killed=3 PASS`, M6d ×3 PASS, M6e `IRQs-taken-at-EL0=26`,
  CAPSTONE 6/6, 0 unexpected faults. EL=1, CNTFRQ=54 MHz.
- **Commit:** `faad571` (merge) · arcs `11b8191` `a072a48` `683d48c`.

### U2.5 — FTDI USB-serial console (x86) 🔬 `hw-rmbp`
- **What:** a captured console for the serial-less 2012 rMBP. The kernel
  enumerates an FTDI FT232 (0403:6001) on the xHCI bus, configures it
  (115200 8N1), and drains a 64 KiB boot-capture ring — fed by every
  `serial_print!` since the first — out its bulk-OUT endpoint, so the whole early
  boot log replays out the cable. Also folds three U2-review hardening items
  (first-entry x87/MMX scrub, per-CPU `DR7` clear, whole-ring-3-window zero before
  a load) and fixes the APIC ms-clock: the BSP heartbeat now re-arms *after* the
  calibrated rate is stored (it was pinned to the fixed fallback → metal read
  ~119 Hz instead of ~1000).
- **Tested — QEMU:** `UNAOS_USBSERIAL=1 ./arroyo test 25` → `FTDI USB-SERIAL
  DETECTED (0403:6001)`, `FTDI console up`, `FTDI TX mirror -> PASS (~17 KB
  replayed)`, `target/ftdi.log` carries the boot log; coexists with the U2 disk
  loader (`UNAOS_USBSERIAL=1 UNAOS_FATIMG=1` → both PASS) and the usbdebug view.
  No-knob boot log byte-identical but for the `DR7 cleared` line and the APIC
  re-arm. FAT regression (`test-fat part`/`sf`) green; `./arroyo check` both arches.
- **Metal — PENDING (~2026-07-08):** rides the physical FTDI cable (B0CJVC19CF).
  QEMU-only until then; the APIC ~1000 Hz truth and the FTDI console verify on the
  real rMBP on cable day (`UNAOS_USBDEBUG=1` boot, FTDI in a root USB-A port).
- **Commits:** `229d675` (Part 0 folds) · `ab0f975` (APIC re-arm) · `f7c929e` (FTDI console).

### U2 — loadable ring-3 programs from FAT (x86) ✅ `hw-rmbp`
- **What:** the first *real program loaded from disk* into the x86 privilege
  boundary. A flat ring-3 binary (`HELLO.BIN`) is read off a FAT volume,
  validated, copied read-only-from-start into the user code page, and run in
  ring 3 (`hello from disk`). Plus the boundary preconditions that make loading
  untrusted code safe: #DB and #MC on dedicated interrupt stacks (closing a
  user-triggerable CPU-halt), a register scrub at first entry, and self-test
  fixtures for the NMI stack and the CVE-2012-0217 guard.
- **Tested — QEMU:** `UNAOS_FATIMG=1 ./arroyo test 25` across all four FAT
  layouts → the three U2 lines + the Part-0 fixtures + U1a/U1b still PASS, full
  USB boot, 0 unexpected faults. `./arroyo check` both arches.
- **Tested — metal (real 2012 MacBook Pro, 2026-07-03):** Realtek USB3 SD reader
  → FAT16 card → `HELLO.BIN` (72 B) → `hello from disk` PASS. *Metal-pending:*
  the #DB-resume path (TCG can't model the single-step-on-SYSCALL trap) and the
  #MC fire path.
- **Commit:** `9cdf397` (merge) · arc `7d8a6bb`.

### M6f — validated user pointers + wider syscalls (aarch64) ✅ `hw-pi4`
- **What:** `copy_from_user`/`copy_to_user` (bounds- and overflow-checked; a bad
  pointer is an error return, not a task kill) plus the first "real" syscalls
  (`yield`, `sleep_ms`, `getpid`, `getinfo`). Also folds five hardening items
  from the M6d review, incl. scrubbing the FP/SIMD registers at first entry.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → the M6f fixtures PASS
  (getinfo round-trip; four hostile pointers all refused with no kills;
  yield/sleep interleave) with every prior milestone still green.
- **Tested — metal (real Pi 4, 2026-07-04, on the M6g reflash):** all three M6f
  verdicts PASS on silicon (getinfo/copy_to_user round-trip, 4 hostile pointers
  refused with 0 kills, yield/sleep interleave) and the per-task EL0 preempt rider
  went > 0 (`spsentinel=3`, QEMU shows all 0) — the timer preempted that slot task
  and it resumed correctly under its own ASID.
- **Commit:** `ee21e30` (merge) · arcs `71ed153` + `e65ffc0`.

### JM2 — Orin headless first light (aarch64/Jetson) 🔬 `hw-jetson`
- **What:** makes the Jetson build boot headless and safe. Gates the QEMU-virt
  SMP path off the Tegra build (it would otherwise touch Tegra memory that
  isn't there), makes the bootloader boot without a display (the shared
  bootloader — the MacBook boots through it too), and adds a boot-diagnostics
  knob that reports the firmware's real serial/display configuration.
- **Tested — QEMU:** full battery byte-stable — `./arroyo check` both arches;
  x86 U1a/U1b PASS; aarch64 virt v2 + GICv3 SMP lines intact; Pi `kernel8-test`
  unchanged. The Tegra feature is off in all of these, so nothing regresses.
- **Tested — metal (real Orin Nano, 2026-07-03):** the boot diagnostics ran on
  silicon (genuinely headless firmware: 0 display handles) and the headless
  path **entered the kernel for the first time on Orin** — which then faulted
  on its first Tegra UART register read because the firmware-handoff page
  tables don't map Tegra device memory. That diagnosis is the next arc (JM3:
  kernel-owned MMU).
- **Held at integration review:** the merge is waiting on a small must-fix —
  the boot-diagnostics DTB table scan compares against a wrong GUID constant
  (it can never match), so the captured "firmware publishes no DTB" line is
  withdrawn as unverified until a re-run with the corrected constant. Fix
  rides at the head of the JM3 arc.
- **Commit:** *(merge pending the fix)* · arcs `811259c` `d382677` `0bd0dae`
  `d0835c0` `27bf835`.

---

## Round 4 — 2026-07-02

### M6d — per-task address spaces + ASIDs (aarch64) ✅ `hw-pi4`
- **What:** every user task gets its own isolated page tables and its own stack,
  ASID-tagged so task switches need no TLB flush. This is what lets two programs
  use the same virtual address for different data — real process isolation.
- **Tested — metal (real Pi 4):** same-VA isolation proven distinct on real A72
  TLBs (QEMU can't test this — it has no TLB model), stack write/readback PASS,
  all under live timer preemption. `4a06a8c`.

### JC2 — PSCI SMP on GICv3 (aarch64/Jetson) 🔬 `hw-jetson`
- **What:** brings up secondary CPU cores on the QEMU-virt GICv3 path via PSCI,
  proven by cross-core interrupts (each core pinged, both directions).
- **Tested — QEMU:** `UNAOS_GICV3=1 ./arroyo test-arm 30` → 3 cores online +
  cross-core SGI both ways; v2 single-core and Pi SMP unchanged. Metal (Orin)
  pending. `18be259`.

## Round 3 — 2026-07-02

### U1b — ring-3 fault isolation + boundary hardening (x86) ✅ `hw-rmbp`
- **What:** a faulting ring-3 program is killed (kernel survives); plus register
  scrubbing, the CVE-2012-0217 guard, an NMI stack, and cross-core W^X.
- **Tested — metal (real 2012 MacBook Pro):** SMEP active, 3 fault-kills with
  correct syndromes, kernel alive past all three. `37d2af8`.

## Round 2 — 2026-07-02

### M6e — preemptible EL0 (aarch64) ✅ `hw-pi4`
- **What:** the timer can preempt a running user task and resume it correctly.
- **Tested — metal (real Pi 4):** 18 preemptions of a spinning EL0 task, all
  resumed correctly. `e62fd4c`.

## Round 1 — 2026-07-02

First rotation of the three-track machine: **U1a** (x86 ring-3 round-trip),
**M6c** (aarch64 loadable blob), **JC1** (GICv3 beside GICv2) — all landed,
reviewed, merged. `637ee5c`.
