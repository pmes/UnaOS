# WCG-TRIAGE — every `target_arch` gate in `video/wcg.rs`, surveyed and classified

Arc rmbp-7, executor WCG. Tree: `~/unaos-bench/scratch/rmbp7/wcg`, branch `exec-r7-wcg`,
base `44c69738`. File under survey: `unaos/crates/kernel/src/video/wcg.rs` (3777 lines).

Milestone 1 is this document. Nothing is ported until it is committed.

---

## 1. The count, and how it was taken

Two instruments, deliberately, because they measure different things and the difference matters.

**`~/unaos-bench/tools/parity-arch-gates.sh` (the standing instrument).** Walks
`unaos/crates/kernel/src` and reports gates one arch has and the other cannot.

```
walk: unaos/crates/kernel/src (108 .rs files, arch/ excluded: per-arch by construction)
PAIRED   (both arches present -> per-arch dispatch, NOT drift): 618
UNPAIRED x86-only     (the Pi/Orin CANNOT have it)            : 471   (bare arch gate: 84 · arch+feature: 387)
UNPAIRED aarch64-only (x86 CANNOT have it)                    : 247   (bare arch gate: 102 · arch+feature: 145)
```

Its `wcg.rs` share: **52 x86-only, 1 aarch64-only** — 53 sites.

**Direct enumeration of the file (this survey).** `grep -n target_arch wcg.rs` = **95 lines**.
Broken down by gate shape:

| gate shape | count |
|---|---|
| `all(target_arch = "x86_64", feature = "wcg-paygo")` | 62 |
| `not(all(target_arch = "x86_64", feature = "wcg-paygo"))` | 17 |
| `target_arch = "x86_64"` (bare) | 8 |
| `not(target_arch = "x86_64")` | 2 |
| `target_arch = "aarch64"` (bare) | 3 |
| `not(target_arch = "aarch64")` | 1 |
| `all(target_arch = "aarch64", feature = "pidesk")` / its `not` | 2 |
| **total** | **95** |

**Why the two numbers differ, and which one this triage uses.** The parity script counts a
`#[cfg(all(x86_64, feat))]` / `#[cfg(not(all(x86_64, feat)))]` couple as ONE unpaired x86-only
site — the `not` arm is a stub, not the other chip's implementation, so it does not "pair".
This survey counts BOTH arms, because the brief's rule is the operative one:

> an `all(feature, target_arch)` gate is NOT a legitimate pair — the feature term does not
> excuse the arch term.

A `not(all(x86_64, "wcg-paygo"))` stub is precisely the drift wearing a fallback's clothes: it
says "aarch64 gets the degraded path EVEN WHEN THE FEATURE IS ARMED". So the drift population
this triage classifies is **95 gate sites**, of which **79 carry the `x86_64 ∧ wcg-paygo`
conjunction**.

The brief's "~107" is in the same neighbourhood and above the measured figure; the measured
figure is 95 and the command that produces it is `grep -c target_arch wcg.rs`.

---

## 2. Verdict summary

| verdict | sites | what it means |
|---|---:|---|
| **PORT** | **81** | the arch term is redundant with the feature term (or with nothing); every symbol the gated code touches exists on both arches; removing `target_arch` changes no build that exists today |
| **HARDWARE** | **12** | genuinely per-arch: different instructions, different tuned constants, or a device property only one chip has. All 12 already carry a justification comment AT the gate — none needed adding |
| **UNKNOWN** | **2** | the blocker is in another file and another lane; not ported, listed for the seat |
| total | 95 | |

---

## 3. PORT — 81 sites

### 3a. The `wcg-paygo` family — 79 sites

Every gate of the form `all(target_arch = "x86_64", feature = "wcg-paygo")` (62) and its
`not(...)` stub (17).

**The reason, stated once because it is the same reason 79 times: the paygo machinery is
arch-neutral by content, and every symbol it reaches is defined for both arches.** Traced:

| symbol paygo depends on | availability | evidence |
|---|---|---|
| `core::sync::atomic::{AtomicU32, AtomicU64}` | both | core |
| `crate::arch::now_cycles()` | both | `rdtsc` / `CNTVCT_EL0`, arch-neutral wrapper |
| `wcg::cycles_to_us()` | both | defined twice IN THIS FILE, `wcg.rs:1925` (aarch64) and `wcg.rs:1945` (x86) |
| `crate::bootpace::origin_cycles()` | both | `bootpace.rs:248`, no arch gate |
| `crate::bootpace::origin_hz()` | both | `bootpace.rs:239` → `counter_hz()`, `bootpace.rs:270/276` carries BOTH arms |
| `wcg::checksum()` | both | pure FNV over a byte range |
| `serial_println!` | both | arch-neutral |
| `super::wm::OccSnap` | both | `wm.rs:3203`, gated `feature = "witness"` ONLY |

Nothing in the family reads an x86 MSR, an x86 port, a PCIe aperture, or an x86-only driver.
The deferral clock, the lattice step, the chunk cursor, the banked sums, the census and its
cadence gate, the terminal latch — all of it is bookkeeping over atomics plus a monotonic
counter both chips have.

**What removing the arch term does to builds that exist today: nothing.** `wcg-paygo` is
named by exactly one leg in `arroyo` (`x86-all`, `arroyo:1949`) and by one env knob
(`UNAOS_WCG_PAYGO`, `arroyo:179`). No aarch64 check leg, no `K8_FEATS`, no Pi/Orin media set
carries it. So on every build that exists, the surviving `feature = "wcg-paygo"` term is
false on aarch64 and the code folds away exactly as the arch term made it fold away. The port
does not arm anything — it makes the capability *reachable* rather than *forbidden*.

The 79 sites, by line:

```
1978 1980 1996 2009 2042 2046 2048 2095 2130 2140 2322 2330 2350 2383 2411 2423
2430 2436 2457 2478 2485 2493 2517 2547 2568 2576 2584 2602 2604 2613 2615 2617
2619 2621 2623 2625 2627 2638 2725 2744 2773 2814 2830 2851 2870 2881 2889 2897
2905 2911 2929 2939 2986 2989 3006 3066 3099 3106 3142 3155 3197 3234 3247 3249
3275 3294 3296 3298 3340 3342 3377 3388 3407 3432 3512 3674 3687 3705 3707
```

Grouped by what they gate:

| lines | item | note |
|---|---|---|
| 2322, 2330 | `PAYGO_LATTICE_N` + its `const _: () = assert!` | a `usize` and a compile-time assert |
| 2350, 2383, 2411 | `PAYGO_DEFER_MS`, `since_entry_ms`, `paygo_clock` | reads `bootpace::origin_*`, both arches |
| 2423–2517 | `PAYGO_STEP/DEFERRED/SAID/PEND/EMIT/LASTROLL/LASTCENSUS/CLOSED` | atomic arrays |
| 2547, 2568 | `WCG_CHUNK_US`, `WCG_CHUNK_BYTES` | integer constants |
| 2576–2638 | `WCG_CUR/BUSY/CHUNKS/HOLD_MAX_US`, `WCG_ACC_*` ×8, `APP_OFF` | atomic arrays |
| 2725, 2744, 2851, 2870, 2881, 2889 | `paygo_recycle`, `paygo_seal_closed`, `PAYGO_FORCE`, `paygo_pending`, `paygo_ripe`, `paygo_force` | `pub(super)`; see §5 |
| 2773/2897, 2814/2830, 2905/2911, 2929/2939, 2986/2989 | `paygo_open`, `paygo_arm`, `probe_step`, `coverage_note`, `PAYGO_ROLLUP_NOTE` — real+stub couples | file-internal |
| 3006, 3066/3099, 3106/3155, 3142 | `paygo_note`, `paygo_flush`, `paygo_complete`, `paygo_closed` | `serial_println!` + atomics |
| 1978/1980, 1996, 2009 | `on_present`'s chunk-band checksum arm | |
| 2042, 2046, 2048 | `Probe::{chunk, band_off, band_len}` fields | |
| 2095, 2130, 2140 | `readback`'s time stop | uses `now_cycles`/`cycles_to_us`, both arches |
| 3197/3234, 3247/3249, 3275, 3294–3298, 3340/3342, 3377/3388, 3407, 3432/3512, 3674/3687, 3705/3707 | `begin`/`end`'s chunk arms | |

**Custody law, checked and preserved.** The brief's constraint — *wcg measurements travel with
the tenant; budgets and monotone wires are per-slot* — is enforced by `paygo_recycle`
(`wcg.rs:2725`) and `wch_recycle` (`wcg.rs:2679`), and by which cells each one clears. The
port removes an arch term from the `#[cfg]` line above `paygo_recycle`; it touches neither
function's body and moves no cell between the two. Custody is unchanged by construction.

### 3b. The occlusion attribution — 2 sites (`wcg.rs:2178`, `wcg.rs:2188`)

This one is not a redundancy. It is a live divergence between what the wire SAYS and what the
code DOES, on the Pi.

```rust
#[cfg(target_arch = "x86_64")]
{
    if occ_before.covers(x + col * scale, dy) || occ_after.covers(x + col * scale, dy) {
        occluded += 1;
    } else {
        bad += 1;
    }
}
#[cfg(not(target_arch = "x86_64"))]
{
    bad += 1;
}
```

Two comments in this same file already claim the behaviour is on both arches:

* `wcg.rs:2099-2101` — "PARITY §6.2 — counted on both arches now that `wm::occ_clip` withholds
  those probes' pixels on both."
* `wcg.rs:2948-2951` (`OccNote`) — "on BOTH arches since PARITY §6.2: the aarch64 blit now
  withholds occluded pixels too, so a wire that stayed silent about the excuse would be a wire
  that hid why a probe was not charged."

The claim is false as written. Verified:

* `wm::OccSnap` (`wm.rs:3203`) and `OccSnap::covers` (`wm.rs:3256`) are gated
  `#[cfg(feature = "witness")]` — **no arch gate**.
* The producers `occluders_above` (`wm.rs:3303`) and `occ_excuse` (`wm.rs:3368`) are gated
  `#[cfg(feature = "witness")]` — **no arch gate**.
* Their call sites that feed `wcg::begin`/`wcg::end` (`wm.rs:5570`, `wm.rs:5673`) are gated
  `#[cfg(feature = "witness")]` — **no arch gate**.

So on an aarch64 `witness` build the snapshots are populated with real occluder boxes, handed
to `readback`, and then **ignored** — every probe a higher window legitimately owns is charged
to `fbbad`, while `occluded=` prints 0. That is a false denominator and, at scale, a
manufactured `-> FAIL`: exactly the class of instrument this module's own commentary keeps
convicting.

Verdict **PORT**, and it is the highest-value finding of the survey.

**Flagged, because unlike §3a this one CHANGES THE aarch64 WIRE.** It is a behaviour change on
Pi `witness` builds with no feature knob in front of it: occluded probes stop being charged to
`bad`, and `occluded=` starts reporting non-zero. `./arroyo test-arm` is in this arc's DONE
gate for precisely this reason and is the check that decides it. If test-arm reddens, this is a
STOP and the port is reverted, not argued with.

---

## 4. HARDWARE — 12 sites

All twelve already carry a justification comment AT the gate. Nothing was added.

| lines | item | why it is genuinely per-arch | justification already present at |
|---|---|---|---|
| 1915, 1917 | `clean_invalidate_surface` | aarch64 needs `DC CIVAC`; on x86 compositor, owner and scan-out read one coherent view and the op is a no-op — same reason `FrameBuffer::flush_range` delegates to an arch hook | `wcg.rs:1901-1912` |
| 1925, 1945 | `cycles_to_us` ×2 | `CNTFRQ_EL0` vs `apic::tsc_hz()`; different registers, different uncalibrated fallbacks (54 MHz vs 1.25 GHz) | `wcg.rs:1921-1924` and `wcg.rs:1935-1944` |
| 541, 543 | `STALL_SPREAD` = 8 (aarch64) / 256 (x86) | a tuned constant, per-tree, and the doc says the ratio does not separate cleanly on both | `wcg.rs:534-540` |
| 298, 2230, 2248, 2158, 2167, 2169 | `PixelFormat` import, `struct GlassRow`, `impl GlassRow`, its construction and its two probe arms | the wide 64-bit glass read exists because the Kepler framebuffer is write-combining PCIe MMIO where `read_pixel`'s three byte reads are three uncached device round-trips. Not a portable optimisation — a property of THIS panel's bus | `wcg.rs:2200-2229` (30 lines of it) |

`GlassRow` is the one that could look like drift and is not: its `not(target_arch = "x86_64")`
arm at `wcg.rs:2169` is a real implementation (`fb.read_pixel`), not a stub — the aarch64
build gets the same measurement by the original path, which is what `GlassRow`'s own doc
argues for at length ("it DELEGATES to `read_pixel`, so the fallback is the original code").

---

## 5. UNKNOWN — 2 sites, not ported

| lines | gate | blocker |
|---|---|---|
| 3610, 3612 | `all(target_arch = "aarch64", feature = "pidesk")` / its `not` | the callee `fbcon::console_is_routed()` is itself gated `#[cfg(all(target_arch = "aarch64", feature = "pidesk"))]` at **`fbcon.rs:2161`** — a different file and outside this executor's lane. The x86 side of "is the console routed into a window" is the `wc` feature's console-window route, a different mechanism, not a rename. Removing the arch term here without an x86 counterpart in `fbcon.rs` would not compile |

Coordinates for the seat: `unaos/crates/kernel/src/video/fbcon.rs:2161` (`console_is_routed`),
consumer at `unaos/crates/kernel/src/video/wcg.rs:3610`. Note `fbcon.rs` carries an
append-only-tail law (`fbcon.rs:2166-2170`) — a line added above existing statements renumbers
panic `Location`s and breaks the Pi track's byte-identity proof. Any fix there must be
appended, not inserted.

---

## 6. Owed to another lane — coordination, not deliverables

These are consequences of §3a that this executor cannot take, because they are outside
`video/wcg.rs`.

1. **`unaos/arroyo`, `KERNEL_CFG_MATRIX`, `arm-pi` leg (`arroyo:1965`) — add `wcg-paygo`.**
   After the port, the paygo family's aarch64 half is compilable but type-checked by NO leg.
   `arroyo:1924` states the standing rule that this executor is invoking: a feature whose
   `#[cfg]` only one leg reaches is type-checked by that leg alone, and a feature no leg
   reaches is type-checked by nothing. This executor verified the aarch64+`wcg-paygo` build by
   hand instead (command in §7); the leg is what makes it stay verified.

2. **`unaos/arroyo`, `arm_features()` (`arroyo:983`) — decide whether `wcg-paygo` joins the
   media strip list.** Today it is NOT stripped (verified: 0 hits in `arroyo:983-1120`), which
   was harmless while the feature emitted no aarch64 code. After the port it emits real
   aarch64 code, so `UNAOS_WCG_PAYGO=1` + a Pi media build would change Pi behaviour. That may
   be exactly what a Pi paygo arc wants — hence a decision, not a defect. `sdw`, `deadman`,
   `wcdvalve`, `sdhcblk` and `pcicensus` are the precedents on both sides of the argument.

3. **`unaos/crates/kernel/src/video/wm.rs` — the consumer half of the seam.** Nine `pub(super)`
   paygo symbols are called from `wm.rs` at sites that carry the same `target_arch = "x86_64"`
   term (`wm.rs:3619`, `3681`, `6429`, `7775`, `7999`, `8084`, `8201`, `8219`, `8261`, `8484`).
   Porting `wcg.rs` alone leaves those symbols compiled-but-uncalled on aarch64 — `dead_code`
   warnings, not errors, and the workspace sets no `deny(warnings)`. The paygo *policy* becomes
   live on aarch64 only when `wm.rs`'s call sites drop their arch term too. `wm.rs` is a shared
   kernel-core file; the rmbp seat owns it, so this is a follow-on arc rather than a
   cross-track negotiation.

---

## 7. Verification commands

Baseline established before any edit:

```sh
~/unaos-bench/tools/parity-arch-gates.sh ~/unaos-bench/scratch/rmbp7/wcg
grep -c target_arch ~/unaos-bench/scratch/rmbp7/wcg/unaos/crates/kernel/src/video/wcg.rs
```

The aarch64 leg that `arroyo` does not yet run, run by hand to prove the ported code compiles
on the other chip with the feature ARMED:

```sh
cd ~/unaos-bench/scratch/rmbp7/wcg/unaos/crates/kernel
cargo +nightly check --release -Z json-target-spec \
  --target ../../aarch64-unaos.json \
  --features witness,logts,rast,pi,baremetal,pidesk,wcg-paygo
```

Confirmed green at base `44c69738` (19.69 s, 38 warnings, none from `wcg.rs`) — so the port has
a before-picture to be judged against and not merely an after one.
