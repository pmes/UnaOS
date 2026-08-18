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

**Witness (M3, pay-as-you-go since GR18).** `syscall::clock_x1_witness()` runs once at boot (after
calibration) as a bounded, **uncounted** serial witness (`== witness ::`, never a `-> PASS` line, so it
shifts no fixture COUNT). It is **silent** where the TSC path is honestly frozen (`uptime_secs()` is
`None`), so a machine without an invariant TSC prints nothing. When live it proves `monotonic()` is
`Some`, that two `rdtsc` reads are monotone and advancing, and — the exact thing JD17 documented as
frozen on x86 — that the wall-second derivation `now()` uses to extend a seed **advances**. It never
seeds the global clock, leaving the operator's UNSET state untouched.

It emits **two** lines, and the split is the point. Waiting *here* for the second edge cost a uniform
draw over the 1 Hz period — 18 to 979 ms across eight metal boots, and the whole of `BPACE: sched d=`
(bootpace.md §8e). So the call site only SAMPLES (`uptime`, `rdtsc`, and the 1 kHz APIC tick), and
`syscall::clock_x1_poll()` — called from `bootpace::service_dump()`, the one call every x86 service loop
makes ungated — delivers the verdict from the first service pass, which is already seconds past the edge.
**Shape, not a capture quote** — the numbers below are illustrative (a real deferral runs ~2.2–3.6 s on
a default build, ~19.6 s on a compositor boot); the s73 table in bootpace.md §8e holds the measured ones:

```
:: CLOCK-X1: TSC invariant, ~2693 MHz; uptime 15 s SAMPLED — second-advance DEFERRED to the first
service pass (pay-as-you-go; a capture with no verdict line below never reached one) == witness ::
:: CLOCK-X1: TSC invariant, ~2693 MHz; monotone (rdtsc +9749000000); uptime 15->18 s (JD17 x86-frozen
clock now advances) [paygo: deferred 3620 ms TSC / 3608 ms APIC, uptime +3 s, core=7 — CONSISTENT]
== witness ::
```

The `[paygo: …]` clause is a cross-check the blocking form never had: the deferral is printed as the
TSC measured it AND as the APIC tick measured it, in milliseconds, and `SKEW` fires past
`200 ms + 5 % of the deferral`. Both figures are shown because `apic::ticks()` counts interrupts and so
undercounts by IF-masked time — that artefact must stay separable, on sight, from a real fault. And the
fault it convicts is a **differentially mis-armed heartbeat**, not a mis-calibrated `tsc_hz`: both arms
come off the same PM-timer denominator in `apic::calibrate`, so a bad PM reference scales them
identically and is undetectable here.

A second derivation that never moves prints `:: CLOCK-X1: FROZEN — uptime still N s after M ms APIC /
K ms TSC …` once 3000 ms of APIC ticks **or** `tsc_hz × 3` cycles have passed — armed on both counters,
since a deadline measured only by the tick cannot report a boot where the tick is dead too. It replaces
the old iteration-cap fallback line that reported a *pass* and so could not be told from a fast boot.
Either counter running backwards prints `:: CLOCK-X1: NON-MONOTONE …` carrying both `rdtsc` reads and
both uptime reads, tested ahead of the frozen branch so a backwards clock cannot hide behind a stale
`uptime still N s`. `core=N` on the verdict names the core it ran on: the sample is the BSP and the
verdict is the service core, so the subtraction is cross-core and rests on this kernel never writing
the TSC.

Read the pair by WHICH line is present, not by counting: the FTDI ring is drop-oldest, so a verdict
with no armed line above it is ring overflow and proves the witness fired, while an armed line with no
verdict below it means the boot never reached a service pass.

**QEMU/TCG note.** TCG cannot advertise the invariant-TSC feature —
`TCG doesn't support requested feature: CPUID[eax=80000007h].EDX.invtsc [bit 8]` — even under `-cpu max`
or an explicit `+invtsc`. So under `./arroyo test` the calibration measures a plausible rate (~2399 MHz
observed) but the CPUID bit is clear, `monotonic()` returns `None`, and the witness stays **silent** by
design (the boot trace reads `(NOT invariant — wall clock stays frozen)`). The witness-fires path is
reachable only on invariant-TSC silicon; the target 2012 rMBP (Ivy Bridge) advertises it, so the witness
is a **metal-bench** line. The firing behaviour above was reproduced under QEMU only with a throwaway,
uncommitted force-invariant probe to exercise the M3 logic.

**Metal verdict (2026-07-16, attended rMBP sitting; log
`rmbp-serial-2026-07-16-114357-boot2-knobon.log`): ✅ METAL-CONFIRMED.** The 2012 rMBP's
Ivy Bridge advertises the invariant-TSC bit as predicted: boot trace
`clock: TSC calibrated ~2693 MHz (invariant)`, then the witness fired live —
`:: CLOCK-X1: TSC invariant, ~2693 MHz; monotone (rdtsc +956725972); uptime 14->15 s
(JD17 x86-frozen clock now advances) == witness ::` — a genuine wall-second advance on
silicon. The x86 half of the JD17 wall clock is live: `uptime` counts, and a seeded
clock advances instead of freezing.
