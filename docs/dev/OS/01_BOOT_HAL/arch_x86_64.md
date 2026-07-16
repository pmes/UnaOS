# Architecture Specification: x86_64 (Ivy Bridge +)

## 1. CPU Feature Requirements
unaOS targets the `x86_64-v2` microarchitecture level or higher.
* **Required Flags:** `SSE4.2`, `AVX`, `POPCNT`.
* **Rationale:** The target hardware (Ivy Bridge i7-3720QM) supports AVX. Using these instructions allows for vectorized memory copies, significantly speeding up the "Clean Room" emulation layer.

## 2. Interrupt Handling (APIC)
* We bypass the legacy 8259 PIC entirely.
* **x2APIC** mode is enabled immediately to handle high-frequency inter-processor interrupts (IPIs).
* This is critical for the "Pervasive Multithreading" model (BeOS style).

## 3. The "No-SMM" Policy
System Management Mode (SMM) is a security risk (ring -2).
* The unaOS kernel attempts to lock down SMM configuration registers at boot.
* We do not allow ACPI calls to enter SMM if an alternative hardware interface exists.

## 4. CLOCK-X1 — the x86 wall-clock timebase (JD17 twin)

JD17 (`crates/kernel/src/clock.rs`) built the kernel wall clock: operator-seeded once per boot with
the `setdate` verb, then extended forward by the architecture's free-running counter and stamped onto
FAT mtimes. On aarch64 that counter is `CNTPCT_EL0`/`CNTFRQ_EL0`; on x86_64 `monotonic()` originally
returned `None`, so a set clock stayed **frozen** at its seeded second and `uptime_secs()` was `None`.
CLOCK-X1 supplies the x86 half.

**Counter — the invariant TSC.** `clock::monotonic()` on x86 returns `Some((rdtsc, tsc_hz))`, gated on
two conditions, each of which returns `None` (honest frozen clock) when unmet:

* **Invariant-TSC bit** — CPUID leaf `0x8000_0007`, EDX bit 8 (`apic::tsc_invariant()`, SDM Vol. 3A
  §17.17). A TSC that is *not* invariant would drift with core frequency across P-/C-/T-states and make
  `now()` lie; the kernel never serves it as a clock. The maximum extended leaf is confirmed first so a
  CPU without the leaf reports "not invariant" honestly rather than reading an undefined EDX.
* **Calibrated frequency** — `apic::tsc_hz() != 0`. Ivy Bridge predates CPUID leaf `0x15`/`0x16`, so the
  TSC rate is not enumerable; `apic::calibrate` (already run at boot, step 4b''') measures it against the
  fixed-frequency **ACPI PM timer** — firmware-independent, bounded (a `CALIB_MAX_TSC` cycle ceiling so a
  wedged/absent PM timer aborts instead of hanging the serial-less boot), no floating point, no new
  interrupt handler. The result is stored once in a static; `monotonic()` itself is one CPUID + one
  atomic load + one `rdtsc` — cheap and lock-free, safe on the shell-verb and FAT-stamp hot paths.

With the counter live the existing JD17 machinery lights up on x86 unchanged: `uptime_secs()` returns
`Some`, `setdate`+`date` advance, `fat_stamp()` stamps real times once seeded. The aarch64 branch, the
`fat.rs` stamp logic and the verbs are untouched.

**Boot trace (M1).** `apic::calibrate` emits, after the timer re-arm:
`clock: TSC calibrated ~NNNN MHz (invariant)` — or `(NOT invariant — wall clock stays frozen)` where the
CPU does not advertise the bit.

**Witness (M3).** `syscall::clock_x1_witness()` runs once at boot (after calibration) as a bounded,
**uncounted** serial witness (`== witness ::`, never a `-> PASS` line, so it shifts no fixture COUNT). It
is **silent** where the TSC path is honestly frozen (`uptime_secs()` is `None`), so a machine without an
invariant TSC prints nothing. When live it proves `monotonic()` is `Some`, that two `rdtsc` reads are
monotone and advancing, and — the exact thing JD17 documented as frozen on x86 — that the wall-second
derivation `now()` uses to extend a seed **advances**, observed as `uptime` crossing a second within a
bounded budget (or, if the run is too fast to cross one, the raw cycle advance with the current uptime —
the tick-monotonicity + nonzero-freq fallback). It never seeds the global clock, leaving the operator's
UNSET state untouched:

`:: CLOCK-X1: TSC invariant, ~2399 MHz; monotone (rdtsc +1896226904); uptime 6->7 s (JD17 x86-frozen clock now advances) == witness ::`

**QEMU/TCG note.** TCG cannot advertise the invariant-TSC feature —
`TCG doesn't support requested feature: CPUID[eax=80000007h].EDX.invtsc [bit 8]` — even under `-cpu max`
or an explicit `+invtsc`. So under `./arroyo test` the calibration measures a plausible rate (~2399 MHz
observed) but the CPUID bit is clear, `monotonic()` returns `None`, and the witness stays **silent** by
design (the boot trace reads `(NOT invariant — wall clock stays frozen)`). The witness-fires path is
reachable only on invariant-TSC silicon; the target 2012 rMBP (Ivy Bridge) advertises it, so the witness
is a **metal-bench** line. The firing behaviour above was reproduced under QEMU only with a throwaway,
uncommitted force-invariant probe to exercise the M3 logic.
