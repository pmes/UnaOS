# PHASE31ROOT — the BAR1 store wedge: mechanism candidates and the first experiment arm

Status: design + one knob-gated experiment arm (`UNAOS_BAR1EXP=uc`), default-off.
Scope of any change described here: **memory TYPE only** — no page-permission bit
(PRESENT/WRITABLE/USER/NX), no MTRR, no SMEP/NXE/WXN interaction. The mechanism is the
same PAT-selector machinery `set_framebuffer_wc` already owns.

## 1. The disease, as the evidence chain states it

Three independent legs, in order of acquisition:

1. **rmbp-5, boot 17 (engine discriminator).** A tree byte-identical to the wedging
   boot with the Kepler FIFO and CE engines dropped wedged anyway. Verdict: the defect
   is the bare write-combining store into BAR1; the engines are exonerated.
2. **FLIGHT1-POSTMORTEM Q1** (`~/unaos-bench/scratch/rmbp7/postmortem/FLIGHT1-POSTMORTEM.md`).
   Refined verdict **STORE-BUFFER BACKPRESSURE**: wedge 1's marked store was a WB *heap*
   store observed one instruction downstream of a stuck BAR1 store from an earlier band
   of the same present — a full WC/store buffer blocks the *next* store of any memory
   type at retirement. The chrome walk was exonerated; the direct painter (`phase 5x`)
   never appears. The postmortem's §F found **no address, alignment or seam property**
   on the wedged rows and read the fault as *time/pressure*-triggered, not
   *address*-triggered.
3. **Flight-4** (2026-08-28, `~/unaos-bench/capture/rmbp8-flight4/ttyUSB0.log`), the
   flight that carried the D-1 WEDGESRC instrument (`blits_retired` / `blit_aim` /
   `blit_inflight`). Three strikes, and the instrument settled the postmortem's open
   question in favour of reading 1:

   | strike | [ms] | holder | win | phase | row | `blit_aim` | `blits_retired` | `blit_inflight` |
   |---|---|---|---|---|---|---|---|---|
   | 1 | 69074–72081 | c1 | 8 | 33 span-flush | 801 | `0xc84030` | 2 147 593 (frozen) | 1 |
   | 2 | 118495–121747 | c2 | 7 | 33 span-flush | 585 | `0x925c38` | 7 271 192 (frozen) | 1 |
   | 3 | 287286–290538 | c3 | 6 | 33 span-flush | 519 | `0x81d2e0` | 32 108 171 (frozen) | 1 |

   `blit_inflight=1` with a frozen odometer across the whole 4 s tripwire window means
   the holder was **inside `fb.blit`'s `copy_nonoverlapping` into the aperture** — the
   store issued and never retired. All three strikes are the same shape as flight-1's
   wedge 2 and none is the downstream-WB shape, which is exactly what the postmortem's
   reading 1 predicted.

   The flight-4 postmortem (`~/unaos-bench/scratch/rmbp8/FLIGHT4-POSTMORTEM.md` §3.2)
   confirms the decode below field-for-field and adds three facts this design carries:

   * **Odometer rates ramp with era**: ~38 k blits/s to steal 1, 103.2 k/s in era 1,
     147.2 k/s in era 2 — retired-blit throughput *rises* after each rehome (the D-2
     free-run amplifying on fewer cores), then freezes dead with one blit in flight.
     Pressure grows monotonically toward each strike.
   * **The trigger law is restated**: flight-3's "steals fire 1–2.5 s after input
     resumes" tested 1-of-3 against these strikes (steal 1 fired 4.1 s after input
     *stopped*; steal 3 fired 22.5 s into input silence under program-driven paint
     alone). The wedge follows **sustained paint bursts**; operator input is one source
     of them, not the trigger. Boot 2 is the control: 364 steal-free seconds under a
     near-idle desktop — idle does not wedge.
   * **Instrument gap**: the `[wedge1] BLITAIM` per-core sibling never printed (it
     hangs off the `[wedge1]` tripwire latch, silent all flight); the per-steal fields
     on the `[wcser]` lines are what delivered D-1.

   Additional exoneration from the same capture: ASPM was cleared at init
   (`[pcih] aspm cleared rp 0043->0040 ep 0043->0040`) and the strikes happened anyway
   — ASPM L0s/L1 is off the table. And at every strike the wedge sampler printed
   `[pcih] rp-at-wedge lnksta=d081 devsta=0000 secsta=2000 aer=n`: the root port's
   config space answers, the link is up, no AER, no new secondary-status error since
   the boot latch. Whatever is stuck, it is stuck *silently* — no error signalling.

## 2. Aim-pattern analysis (candidate (d)'s funeral)

`blit_aim` is the BAR1-relative byte offset `py * pitch + bx * bpp` of the stuck row
copy (pitch = 16384 B, bpp = 4, fb base = `0x90020000`, 2880x1800 panel). Decoding:

| strike | aim | = row · 16384 + col·4 | window at wedge (last `[wc-a] create`) | matches |
|---|---|---|---|---|
| 1 | `0xc84030` | row **801**, bx = 12 | win=8 `288x288 scale=2x at (17,717)` → bx = 17−5 = 12, box rows 678..1298 | ✓ row 801 = band-local 123 |
| 2 | `0x925c38` | row **585**, bx = 1806 | win=7 `288x288 scale=2x at (1811,85)` → bx = 1806, box rows 46..666 | ✓ row 585 = local 539 |
| 3 | `0x81d2e0` | row **519**, bx = 1208 | win=6 `288x288 scale=2x at (1213,85)` → bx = 1208, box rows 46..666 | ✓ row 519 = local 473 |

Every aim is exactly the first span of an ordinary interior row of the window the pass
was flushing. What the three aims do **not** share:

* **Not one address**: aims span `0x81d2e0`..`0xc84030` (≈4.6 MB apart).
* **Not one page/leaf**: absolute addresses `0x9083d2e0`, `0x90945c38`, `0x90ca4030`
  fall in 2 MiB leaves 4, 4, and 6 of the aperture — two different huge leaves, three
  different 4 KiB pages.
* **Not one alignment**: aim mod 64 = 32, 56, 48 — three different phases within a
  64-byte WC line (all are mid-line starts, but that is true of nearly every window
  flush, since `bx·4 % 64 == 0` only when `bx % 16 == 0`).
* **Not one row class**: rows 519/585/801, three different windows, three different
  cores, three different holders.
* **Not one count**: strikes at 2.1 M / 7.3 M / 32.1 M retired blits — inter-arrival
  2.1 M, 5.1 M, 24.8 M. No periodicity.

What they **do** share: the same window class (288x288 @ 2x → `row_bytes` = 2344, the
same 2344-byte `copy_nonoverlapping` shape as flight-1 wedge 2), phase 33, and
sustained composite load. The flight-4 postmortem's phrasing is the right one: the
wedge is **path-pinned (the span-flush row blit), not address-pinned** — three
scattered offsets, each exactly where the blit said it was writing. This confirms
flight-1 §F at n=6 total wedges across two flights: the trigger is **pressure over
time, not a poisoned address** — and the era-ramped odometer rates (§1) plus the
paint-burst trigger law sharpen "pressure" into *sustained aperture write throughput*,
which is precisely the quantity a memory-type experiment perturbs. Candidate (d),
aperture-region avoidance, is refuted and not implemented; the discrimination budget
goes to the mechanism fork instead.

## 3. Today's mapping, exactly (the baseline the experiment perturbs)

As found at `bdddbdcd`, all in `unaos/crates/kernel/src/arch/x86_64/memory.rs`:

* **PAT programming** (`ensure_pat_wc`, memory.rs): IA32_PAT (MSR 0x277) slot **PA4**
  is set to WC (encoding 0x01); slots 0..3 keep the power-on `[WB, WT, UC-, UC]`.
  Programmed on the BSP by `set_framebuffer_wc` and on every AP by `smp::ap_entry`.
* **Panel leaves** (`set_framebuffer_wc`, called from `fbcon::init` /
  `main.rs`): every identity-map leaf covering the firmware framebuffer range
  (`0x90020000..0x91c40000` on the bench machine — 15 × 2 MiB leaves, flight log line
  `:: x86 fb-wc: retyped 15 leaf(s) WC (PAT PA4) ... ::`) gets the leaf-level PAT bit
  **set** and PCD/PWT **cleared** → PAT index 4 = **WC**. Only the selector bits are
  written; permissions carried through. The retyped leaf hull is latched in
  `FB_WC_LO/HI`.
* **Firmware MTRR context**: the aperture sits under a UC variable-range MTRR
  (metal-confirmed, per the module's own comments). Effective type = combine(MTRR UC,
  PAT): **WC** for PA4 (the one PAT/MTRR combination that overrides UC), **UC** for
  everything else. This fact kills candidate (b) below.
* **All other MMIO** (`map_mmio_window`): PAT index 3 (PAT=0, PCD=1, PWT=1) = **UC**,
  with `leaf_is_fb_wc` skipping the deliberately-WC panel leaves when BAR1 is mapped
  as a window.

**Store paths into the aperture** (the complete census of writers):

1. `wm.rs stage_window` **span-flush** — `blit_traced` → `fb.blit` →
   `copy_nonoverlapping` of a row (`clip.n == 0`) or its unoccluded sub-spans. THE
   convicted site: all three flight-4 strikes and flight-1 wedge 2.
2. `wm.rs` erase path — the same `fb.blit` primitive for uncovered rows.
3. `paint_window` **direct** painter (`phase 5x`, dst = panel) — scalar `p.write`
   runs / `fill_span4` / `put_pixel`. Never observed wedged in any flight.
4. `video/cursor.rs` — bounded `read_pixel`/`put_pixel` pairs (≤ MAX_PIX per pass)
   against the panel: volatile 4-byte stores plus **non-posted volatile reads**
   (~976 ns each, GR17 cost model).
5. `video/screen.rs` — `front.blit` damage-rect flush (same primitive as 1).
6. WC-D/WC-G verify — volatile **read-back** from the aperture (reads, not stores;
   throttled by the wcdvalve).
7. `fbcon` early console, before the compositor owns the panel.

Chrome and menubar compose into the RAM stage and reach the panel only through path 1.

## 4. Candidate mechanisms

All three below are consistent with all three evidence legs (engines exonerated;
stall observable downstream through a full store buffer; stuck store confirmed
in-flight at scattered aims under load; link error-free at the wedge).

### M1 — stuck WC-buffer drain (core/uncore side)

An Ivy Bridge WC fill buffer, holding a partial line from a mid-line row start,
fails to complete its eviction. Store buffer backs up behind it; the core stalls at
retirement forever.

Predicts: memory type is **causal** — removing write-combining removes the wedge.
Partial-line evictions (every wedged row starts mid-line) are the natural aggravator;
load correlation follows from more concurrent fill buffers in flight. Weakness: a
fill-buffer eviction that hangs with a healthy downstream would be a CPU erratum —
possible on this silicon, but the least externally-supported of the three.

### M2 — PCIe posted-write credit starvation

The GK107 endpoint stops returning posted-header/data flow-control credits for the
BAR1 traffic class. The root port's buffers fill, the uncore cannot evict WC buffers,
the store buffer backs up — same CPU-side signature. No error is signalled because
credit starvation is not an error: `rp-at-wedge`'s clean `devsta/secsta/aer` is
exactly what this predicts.

Predicts: memory type is **not causal** — a UC store is a posted write too (one
outstanding at a time), so the first unaccepted write stalls the UC build the same
way, minus the WC buffering.

### M3 — GPU-side aperture stall

The BAR1 window path inside the GPU (host interface / VRAM arbiter) stops servicing
incoming window writes. Externally indistinguishable from M2 at the root port — the
device simply stops accepting — and boot-17 already proved this path wedges with
FIFO/CE dropped, so it would be a property of the bare window logic, not of any
engine.

Predicts: same as M2 from the CPU side. Discriminating M2 from M3 needs a
device-side observation **during** the wedge (an EP config read or a BAR0 register
read from the stealing core: non-posted requests use a different credit class, so
"non-posted completes while posted starves" separates link-credit starvation from a
device that answers nothing). That is a *future* arm; it is deliberately not this
session's, because the M1-vs-{M2,M3} fork comes first and is cheaper.

## 5. Candidate fixes, costs, and verdicts

### (a) UC mapping for the panel — **the implemented experiment arm**

Retype the panel leaves to PAT index 3 (UC) instead of index 4 (WC) at the same
`set_framebuffer_wc` site. Correct by construction (UC is the strictest type; it was
the pre-WC state of this machine) and the cleanest possible discriminator for
M1-vs-{M2,M3}.

**Cost, quantified from flight-4's own numbers.** The terminal `[comp2]` rollup shows
`bytes_pp=3 657 944` against `blit_us=3 302` → the WC flush sustains **≈1.11 GB/s**
into the aperture. The metal-measured UC rate is **≈162 MB/s, size-invariant**
(memory.rs, the pre-WC measurement; 7.6 fps vs 53.8 fps end-to-end). The same pass
therefore takes ≈6.8× longer to flush (≈22.6 ms instead of 3.3 ms), and flight-4's
sustained demand — 32.1 M blits retired by 290 s, at the 2344-byte row shape of the
wedged windows, order 150–260 MB/s of aperture writes — sits at or above the UC
ceiling. **An armed flight will be visibly slow and must be judged per aperture
byte, not per wall-clock second** (see §6). UC is the experiment, not the shipping
fix.

### (b) WT via PAT — **rejected: architecturally unreachable in scope**

The aperture is under a UC variable-range MTRR. The SDM's effective-type combination
(Vol 3A, table 11-7) gives UC for PAT = WB/WT/WP when the MTRR says UC; **only PAT WC
overrides a UC MTRR**. A "WT via PAT" arm would therefore produce a second UC arm
wearing a WT label — no new information — and reaching real WT would require MTRR
edits, which are explicitly outside this seat's memory-type-only scope (and outside
the STOP tripwires). Not implemented, and should not be revisited without an MTRR
mandate.

### (c) bounded-cadence `sfence`/drain on the blit path

An `sfence` per row (or per N rows) bounds the number of un-evicted WC lines a pass
can accumulate. Cheap: tens of ns per fence against a 3.3 ms flush budget. But
flight-4 weakens it as a *discriminator*: `blit_inflight=1` shows the stall biting
**inside a single 2344-byte copy**, so unless the mechanism is cross-blit
accumulation, a fence between blits moves the stall site without curing it — and if
eviction itself hangs (M1), the fence *is* the stall site. Retained as the leading
mitigation candidate **if the UC arm convicts M1**; not this session's arm.

### (d) aperture-region avoidance — **rejected by the data**

§2: three aims, two huge leaves, three line phases, no shared property, plus
flight-1 §F's explicit no-pattern finding. There is no region to avoid.

## 6. The implemented arm: `UNAOS_BAR1EXP=uc` (feature `bar1exp-uc`)

**What it does.** At the one site that types the panel (`set_framebuffer_wc`), the
armed build writes PAT index **3** (PAT=0, PCD=1, PWT=1 → UC) to the panel leaves
instead of index 4, and does not latch the `FB_WC_LO/HI` hull (nothing is WC, so
there is nothing for `map_mmio_window` to protect; a later BAR1 window map re-types
the same leaves to the same UC — idempotent). `ensure_pat_wc` still runs on every
core: PA4 is still programmed WC, but no PTE selects it — one fewer divergence from
the baseline, and the slot stays ready for future arms. Everything else — walk,
leaf granularity, invlpg sweep, permission preservation — is byte-for-byte the
baseline code.

**Armed witness** (strings-verifiable in the ELF):

```
:: x86 bar1exp: UC arm ARMED — retyped N leaf(s) UC (PAT PA3) over 0x..0x; panel write-combining SUPPRESSED (PHASE31ROOT) ::
```

replaces the baseline `:: x86 fb-wc: retyped ... WC (PAT PA4) ... ::` line. The
WXPROBE fb line independently corroborates: armed boots read `pat=0 pcd=1 pwt=1`
where the baseline reads `pat=1 pcd=0 pwt=0`.

**Knob wiring** (both places — the two-place trap has an executable CHECK now):
`arroyo`'s knob map (`UNAOS_BAR1EXP` → `bar1exp-uc`, with an unknown-mode refusal
above the map line so `UNAOS_BAR1EXP=wt` fails loudly instead of silently arming the
UC arm) and `builder/src/main.rs` (same mapping, same refusal), so metal media built
by the builder carries the arm when — and only when — the banner says so. The
feature is named on the `x86-all` check leg, so both polarities are type-checked and
the KNOB→BUILDER WIRING CHECK covers the knob.

**Default build unchanged.** Every kernel-side delta is behind
`#[cfg(feature = "bar1exp-uc")]` / `#[cfg(not(feature = "bar1exp-uc"))]` in
`arch/x86_64/memory.rs`; the feature defaults off and is pushed by nothing except
the knob. Knob off ⇒ the armed tokens are not compiled, the baseline tokens are the
pre-arc bytes ⇒ today's mapping, provably (measured at the gate: the default
`UNAOS_WC=1` QEMU leg is green with an unchanged banner, and the armed witness
string is absent from the default ELF by `strings`).

### Falsifiable predictions

Flight-4's null rate is 3 wedges in 32.1 M retired blits (≈1 per 10.7 M). The armed
flight is judged on the same odometer, because UC throughput makes wall-clock
incomparable:

* **P1 (M1 true — WC convicted):** an armed metal flight that retires ≥ 32 M blits
  (the `blits_retired` field on the tripwire/BLITAIM lines is the meter) produces
  **zero** `[wcser] PASS OVERDUE` strikes. Under the null rate, zero wedges in 32 M
  blits has probability ≈ e⁻³ ≈ 5 % — a clean armed flight is significant, not
  suggestive. Expected side effect, which is itself a check that the arm is real:
  `[comp2] blit_us` per pass rises ≈7× and the desktop visibly drops toward the
  7.6 fps regime. Budget note: at the UC ceiling the armed flight cannot approach
  flight-4's era-2 rate of 147 k blits/s (postmortem §3.2), so reaching the 32 M-blit
  meter takes on the order of 8–15 minutes of *sustained paint* — the flight must run
  the same storm workload (vugs + pulse), since boot 2 proved an idle desktop retires
  almost nothing and wedges never.
* **P2 (M2/M3 true — memory type exonerated):** the armed flight still strikes, with
  the same signature (phase 33, `blit_inflight=1`, frozen odometer, scattered aim).
  The next arm is then the device-side wedge probe of §4 (EP config/BAR0 read from
  the stealing core), and mitigation work moves to the link/GPU side
  (`PCIE-RP-RECOVERY.md` becomes the adjacent document).
* **P3 (either way, boot-time):** the armed boot prints the `bar1exp` ARMED line and
  WXPROBE reads `pat=0 pcd=1 pwt=1` on the fb leaf; the baseline lines are absent.
  If the armed boot ever prints the baseline `fb-wc ... WC (PAT PA4)` line, the arm
  did not deploy and the flight is void.

A flight that wedges *earlier* per-blit under UC would additionally favour M2/M3
with back-to-back non-combined stress, and is worth recording but not predicted.

## 7. Out of scope, recorded so nobody re-litigates silently

* MTRR edits (blocks candidate (b) — see §5b).
* Page permissions, WXN/NXE/SMEP — untouched by design and by the seat's scope line.
* The scheduler and the WCSER steal/rehome machinery (flight-1 lanes D-2/D-6/D-7 own
  those).
* QEMU reproduction: QEMU does not model PAT/WC timing or PCIe flow control; the
  wedge has never reproduced there. The QEMU legs gate *regression*, not the disease.
