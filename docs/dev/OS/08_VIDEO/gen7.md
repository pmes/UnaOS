# GEN7 — Ivy Bridge GT2 render-engine ladder (x86 / MacBookPro10,1)

Subsystem: `unaos/crates/kernel/src/drivers/gpu/gen7.rs`.
Call site: `drivers/gpu/igpu.rs`, inside `init`, above `bring_up_blt_ring`.
Feature: `gen7` (implies `intel-ivb`). Env knob: `UNAOS_IVB3D=1`, wired in `unaos/arroyo`
and `unaos/builder/src/main.rs`. Default OFF; with the knob unset the module is not compiled
and the image is byte-identical to baseline.

Design of record: `~/unaos-bench/scratch/gr25/GEN7-3D-draft.md` (GR25). Security and
reversibility rules: [`docs/dev/LAWS.md`](../../LAWS.md).

---

## 1. What this module is

A ladder of investigative rungs against the Ivy Bridge integrated GPU's render engine. Each
rung is decisive in one boot, names every outcome it can reach *before* it runs, and prints a
single classified verdict on the serial wire. The module exists to answer one question in
stages — *can the x86 track offload compositing work to the IGD's engines?* — and it is built
so that a negative answer is a finding rather than a failure.

The IGD is not the panel's owner on this machine. The panel belongs to the Kepler
(`igpu.rs` ~778); every iGPU pipe and plane reads `0x00000000`. That fact is what makes this
ladder able to move quickly, and it is one of the two independent reasons no rung here can
black the display.

### 1.1 The standing safety properties

These hold in every rung and are verified rather than asserted:

- **No display register is written.** The module touches exactly one display-block offset —
  `PCH_PP_CONTROL` (`0xC7204`), the PCH Panel Power Sequencer — and it is **read only**, as a
  control-frame witness: the PPS sits outside the GT power well, so reading it separates
  "BAR0 is dead" from "the GT is dead".
- **Every write is captured, restored and re-read on every exit path**, including the refusal
  and error paths.
- **GGTT entries are claimed only into a window first proven unowned** — every slot and both
  bracketing neighbours reading all-zero, or reading the four-leg-confirmed firmware
  scratch-fill (R4b) — with neighbour-smear checks at claim and at restore.
- **Scratch pages are never handed back to the allocator while a GT translation to them might
  survive.** Before R6 they were leaked unconditionally; R6 replaces that with a gated
  reclaim (§2.6).
- **Every poll is bounded on the cycle counter** (`now_cycles()`), never on `arch::ms()`.
- **A zero-compare is never a verdict.** Every read is classified three ways
  (`structured` / `zero` / `allones`), every register is read twice so a read-twice
  difference is positive proof of life, and the control frame carries generator-checked exact
  values from outside the GT power well.

### 1.2 Citation classes

Every offset and encoding in the module carries its provenance on the wire:

| Class | Meaning |
| --- | --- |
| `[PINNED]` | Verified against a public Intel Ivy Bridge PRM; document, volume, section and page in the source comment. |
| `[BDW-ONLY]` / `[CHV-ONLY]` | Pinned on **later** silicon (Broadwell, Cherryview/Braswell) and carried here as a hypothesis to be tested on our own part — never implemented as though it were Gen7 spec. |
| `[EXT-UNPINNED]` | Recollection, unverified. May be tested under capture/restore; may never be load-bearing. |
| `[METAL]` | Observed on this MacBookPro10,1 in a named boot capture. |

Clean-room line: Intel PRMs are the only pinning target. Linux `i915` and Mesa `i965` source
are off-limits and are not a source for anything in this ladder.

---

## 2. Ladder state, R1 through R6

| Rung | Name | Writes | Metal verdict |
| --- | --- | --- | --- |
| R1 | `recon` | none | GT block dark; `GTFIFOCTL` the only structured read |
| R2 | `wake` | 3 GT power-management regs, all reversed | `gt-still-dark` |
| R3 | `forcewake` | 1 forcewake request reg per candidate, released in-rung | acquires fly; the well does not open on the battery |
| R4 / R4b | `claim` | ≤3 GGTT PTEs, all restored | GGTT PTE round-trip **proven** |
| R5 | `execute` | 2 GGTT PTEs + 4 RCS ring regs, all restored | `enable-void` — `RING_CTL` write did not latch |
| R6 | `rearm` | as R5, **under a held wake**, + 1 GTT-flush reg | *pending metal* |

### 2.1 R1 — `recon` (read-only)

A census that answers: is the GT window alive, is there a forcewake block where the draft
guessed, what is in the GGTT, and where is stolen memory. Zero MMIO writes, zero config
writes.

**[METAL, Boot D]** the single most important reading R1 produced: `HYP_GTFIFOCTL`
(`0x120008`) read `0x0000003F`, stable — **the only structured register in the whole
25-register probe**. Every ring-block register, every `0xA18x` offset and every other
`0x13xxxx` offset read `0x00000000`. Two consequences, and neither is a decode of the value:
the BAR0 window reaches the `0x12xxxx` block (so a zero at `0x1300xx` is a statement about
that register, not about the mapping), and there is a live GT-wrapper block **outside**
whatever gates the ring registers. Every rung since reads `GTFIFOCTL` as its delta control.

### 2.2 R2 — `wake` (the ladder's first write)

Drives the IVB Sync-Flush workaround (`IVB-V1P3 §1.1.10.9`, pp.70-71): `INSTPM 0x2050 =
0x00010001`, `RCS_WAKE 0x2700 = 0`, poll `0x22AC[3:0] == 0`, re-park `INSTPM` on every exit
path.

**Verdict on metal: `gt-still-dark`** — `trans_untouched=0/14`. Two lessons the later rungs
are built on. First, `poll_ack=1 poll_iters=0` was **not** an ack: `0x22AC` read zero on the
first look and `== 0` was the pass condition, so a power-gated window that returns zero for
everything passed the poll on iteration zero. Every ack test from R3 onward is a
**transition** test with a stable-zero precondition. Second, draining a command streamer is
not the same act as powering one — the Sync-Flush sequence is a VT-d workaround, not a
general forcewake protocol, and the module never pretended otherwise.

### 2.3 R3 — `forcewake`

Goes at the documented mechanism: a forcewake **request** register and its **ack** partner,
one candidate at a time, each acquire released in-rung with the release verified against the
entry value, and the 17-register GT battery read under each hold.

Intel never published the Gen7 GT power/forcewake register block — the complete sixteen-volume
IVB PRM set was searched and the only hits are register-less "Force Wakeup bit" prose. So
both candidates are pinned on later silicon and flown as hypotheses:
`FORCE_WAKE 0x0A188` / `GTSP1 0x130044[15:0]` `[BDW-ONLY]`, and `RENFW 0x1300B0` / `0x1300B4`
`[CHV-ONLY]`.

R3 also carries an honesty bound the module reuses everywhere: `restored=` is
`req_post == req_pre && ack_post == ack_pre`, and on this part all four of those dwords read
`0x00000000` — a `0 == 0` compare cannot fail. So the rung measures whether the check *had*
any discriminating power and prints `evidence=real|blind` beside it. `restore=clean
restore_evidence=blind` is the honest form of "as far as anything readable on this part can
tell".

### 2.4 R4 / R4b — `claim`

Reads R3's verdict and branches. The read-only census runs on any reachable wake; the single
reversible PTE round-trip runs only on a **confirmed** wake. "Unowned" has two proven shapes
(R4b, Boot Ab): every pre-image zero, **or** every pre-image the firmware **scratch-fill** —
one identical valid PTE whose frame is the BDSM stolen-memory base read from the host bridge
*this boot*, uniform across the window, both neighbours, and six distant probe slots.

**The GGTT PTE round-trip is proven on metal.** The GT fabric answers; the engine block does
not. That split is what the rest of the ladder is about.

### 2.5 R5 — `execute`

Claims two GGTT slots (a ring page and a target page), maps a minimal RCS ring, writes one
`MI_STORE_DATA_IMM` into it that stores a sentinel to the target page's GGTT address, programs
`RING_START` / `HEAD` / `TAIL` / `CTL`, advances the tail, and polls for the sentinel.

**Verdict on metal, three boot legs: `enable-void`.** The PTEs landed. The four submission
registers were programmed. `RING_CTL` was written `0x00000001` and read back `0x00000000`.
The enable did not latch. R5's own `next=` named the suspect:
`R6-must-hold-forcewake-and-rearm`.

### 2.6 R6 — `rearm` (the wake that makes RING_CTL latch)

R3 releases its forcewake acquire *inside its own rung*, by design — R3's job was to measure
the acquire, not to keep it. So by the time R5 wrote `RING_CTL`, no hold was in force. R6 is
the experiment that follows: acquire a candidate, **keep the hold across the whole
arm / submit / drain / disable / restore**, and only then release.

#### The preheld guard, retired

R3 as first written refused any candidate whose request register read non-zero at entry. On
metal that **skipped the only [PINNED]-adjacent candidate the ladder has**: `0x0A188` read
`0x00010000` at candidate entry while reading `0x00000000` in the frame census milliseconds
earlier, and the boot was spent.

The defect was the inference, not the threshold. A non-zero read at a request register cannot
distinguish "another owner holds forcewake" from "this offset is not a forcewake register on
this part" (on Cherryview the same offset is `SCRATCH1`, an unrelated ECO scratch register)
from a gated-window decode artefact. And the value actually seen makes the alarming reading
the *least* likely of the three: `0x00010000` is the **mask-form release pattern the rung
itself writes**, and under the documented mask semantics (*"Reads to this field returns
zero"*) a healthy MT register cannot read its mask field back as set at all.

So the guard is **retired**, and the discrimination is the handshake itself: attempt the
documented set-and-verify sequence regardless of the entry value, under full
capture/restore/re-read, and classify by what the silicon answers.

| `classification=` | Meaning |
| --- | --- |
| `fw-ack-transition` | The ack field left a stable-zero entry column. The pair is real. |
| `fw-req-decodes-no-ack` | No ack, but the request register read back a change from our write. Something decodes here; the ack is not where we looked. |
| `fw-no-decode` | No ack, and the request register did not read back our write. |
| `fw-ack-unreadable` | The ack's entry column was non-zero or unstable — a transition is not readable, so no ack may be claimed either way. |

What the guard actually protected is preserved by two stronger properties. The acquire is
**additive, never clearing**: the mask form writes `0x00010001`, whose mask bit arms *only*
data bit 0, so bits [15:1] — any other thread's request — are untouchable by it; the plain
form writes `req_pre | 1`, so no bit set at entry is ever cleared. And the release restores
**the captured entry dword**, whatever it was, re-read and reported with its own `evidence=`
bound. The entry reading survives as a witness (`pre_nonzero=`, `pre_stable=`) on every
candidate line — data, not a skipped rung.

`Acq::SkippedPreheld` and R3's `both-req-preheld` verdict arm are retired with the guard.

#### Candidate order

One variable per attempt, in a stated order, stopping at the first candidate whose enable
latches. Each is labelled with its citation class on the wire.

| # | `cand=` | request / ack | `class=` | Write form |
| --- | --- | --- | --- | --- |
| 1 | `mt` | `0x0A188` / `0x130044[15:0]` | `BDW-ONLY` (BDW-V2C pp.493/703) | mask `0x00010001` |
| 2 | `renfw` | `0x1300B0` / `0x1300B4` | `CHV-ONLY` (CHV-V2C pp.1078/1077) | plain `req_pre \| 1` |
| 3 | `gtforceawake` | `0x130090` / `0x130044` | `BDW-ONLY-reg+EXT-UNPINNED-ack` (BDW-V2C p.656) | plain `req_pre \| 1` |

`mt` is first because it is the only [PINNED]-adjacent candidate — Intel documents the
request register, the mask-write form, the ack register *and* the poll procedure, on silicon
two generations later — and because it is the one the retired guard skipped. `gtforceawake`
is last and its **ack pairing is [EXT-UNPINNED]**: Broadwell says of that register only that
it is no longer used and refers the reader to `0xA188`, and names no ack partner; R6 watches
`GTSP1` because that is the ack of the mechanism which replaced it. That is an inference, it
is labelled as one, and it is worth flying because a *legacy* wake register is exactly the
kind of thing that would still be the live mechanism one generation earlier.

`MISC_CTRL0 0x0A180` is deliberately **not** a candidate. It is GPM *control*, not forcewake,
even on the silicon where it is pinned — writing it would be poking a power controller on a
hypothesis, a different and worse class of act than testing a documented handshake.

#### Wall D for R6

- **`RING_CTL` readback is generator-checked by construction.** We write a specific bit and
  demand that bit back; `0x00000000` is R5's finding and `0x00000001` is the falsifier. The
  pass condition is our own value returning, not the absence of a zero.
- **`HEAD == TAIL` is never the success test.** That is the dead-GT trap: on a powered-down
  part MMIO reads may return zero, and `HEAD == 0` equals `TAIL == 0` before anything runs.
  The execution witness is the **sentinel** — a value only the GT's store could place in a
  page pre-seeded with a *different* generated pattern, read back through that page's own CPU
  mapping after a `clflush`. `head_moved` is corroboration, never proof.
- **The battery is read under each hold before any ring write.** R6 writes four of the
  seventeen battery rows, and a register we caused to change is not a witness, so three counts
  go on the wire: `live_all17`, `live_r2untouched14`, `live_r6untouched13`. A reader can
  discount every row this rung ever writes and still have a witness.
- **`GTFIFOCTL` is the delta control** — the machine's only always-on GT witness, written by
  no rung, read at entry, under every hold, and at exit.

#### The GGTT TLB-invalidation rung, and the reclaim

R5 leaked two pages per run for a stated reason: with no invalidation, a translation the GT
cached during the hold could outlive a free and DMA into reused kernel heap. R6 adds the rung.

The flush register is `0x101008` and it is **[EXT-UNPINNED]** — the R0.1 PRM search never
looked for a GTT-flush register, and re-searching sixteen volumes is outside this rung. It is
written under full capture / restore / re-read and gets its own three-way verdict:

| `r6 tlb verdict=` | Meaning |
| --- | --- |
| `tlb-flush-decodes` | The readback changed. Something is there. |
| `tlb-flush-write-silent` | The readback did not change. **A self-clearing flush register and a non-decoding offset are indistinguishable here** — the rung says so instead of scoring a silent write as a success. |
| `tlb-flush-allones` | The offset read `0xFFFFFFFF` — the documented dead-well signature. |

**The reclaim is not gated on that register alone.** It fires on `reversal_clean` **AND**
(`never-fetched` **OR** `flush-verdict`), where `never-fetched` is an independent structural
proof: no candidate's `RING_CTL` enable ever read back set, the head never moved, and no
sentinel ever landed — so no engine access was ever issued through either GGTT address and
there is no cached translation to invalidate. On the machine R5 flew on, `never-fetched` is
the leg that fires, and it is the stronger of the two. **An [EXT-UNPINNED] register is never
the sole reason a page goes back to the heap.**

#### Cycle bounds

`poll_cycles` bounds every poll on `now_cycles()` — rdtsc, invariant on this part, and it
advances regardless of `EFLAGS.IF`. `iters=` stays on the wire beside `cyc=` as the rate
datum, but it no longer decides when to stop: an iteration count is not a time, and R3's
`200_000`-iteration ack budget was an unknown number of milliseconds. Budgets:
`FW_ACK_BUDGET_CYC` ≈ 8 ms, `EXEC_BUDGET_CYC` ≈ 20 ms, `DRAIN_BUDGET_CYC` ≈ 8 ms.
`arch::ms()` is used nowhere in this module.

#### Verdicts

| `r6 verdict=` | Meaning |
| --- | --- |
| `r6-gated-on-wake` | R3 did not confirm a wake. **Nothing written.** |
| `r6-range-owned-refused` / `r6-fill-hypothesis-refuted` | The GGTT window is not provably unowned. Nothing written. |
| `r6-claim-write-void` | A PTE did not land, or a neighbour smeared. The ring was never armed. |
| `r6-enable-void-under-every-hold` | **The decisive negative.** Every candidate hold was taken and `RING_CTL` still read back `0`. |
| `r6-head-stuck` / `r6-head-stuck-partial` | The enable latched but the CS did not parse the bare ring — the `§1.1.11.4 p.79` default-context caveat coming true, and a strictly better place to stand than R5 ended. |
| `r6-sentinel-miss` | The head retired without the store taking effect. |
| `r6-sentinel-hit-head-stuck` | The store landed but the head did not retire the ring. |
| `r6-sentinel-hit` | **The win.** The enable latched under a held wake and the GT executed the command. |
| `r6-ring-would-not-disable` | **Safety override.** The PTEs are left claimed under a possibly-live engine; it dominates every exec reading. |

#### The metal falsifier

Stated before the boot: **with a wake held, `ctl_readback==0x1`, `head_moved=1`,
`sentinel_hit=1`.**

If `ctl_readback` stays `0x00000000` under **every** candidate hold, the engine register
domain is not writable on this part on any documented register, and the x86 engine-offload
programme is dead on documented registers. That is a finding worth the boot, and the rung
states it in those words on its `next=` line.

---

## 3. Verification

**QEMU has no Ivy Bridge IGD.** `./arroyo check` proves types and the cfg lattice and
**nothing about any rung**; the feature banner proves a feature is compiled, not that a
witness is reachable. The honest gates are:

1. `./arroyo check` for both arches, plus `UNAOS_WC=1 ./arroyo check` and
   `UNAOS_WC=1 UNAOS_IVB=1 UNAOS_IVB3D=1 ./arroyo check` with `gen7` in the feature banner.
2. The x86-fat knob-off battery, proving the disarmed image is unchanged.
3. **`strings` on the armed `esp-x86` artifact**, proving every verdict token is present in
   the shipped ELF — not merely compiled behind a `cfg`.
4. **Metal.** The falsifier above.

One deliberate `strings` exception: `r6-ring-addr-illegal` (and R5's identical
`ring-addr-illegal`) does not appear in the artifact, because `ring_gtt_addr` is a
compile-time constant and rustc proves the branch dead. That is the `RING_BUFFER_START`
bits[31:29] invariant being discharged **at compile time** — stronger than a runtime check,
not absent. If the ring slot is ever made non-constant, the branch and its token return.

---

## 4. Where the ladder stands

The GT **fabric** is alive: `GTFIFOCTL` is structured and moves, and the GGTT PTE round-trip
is proven on metal at R4/R4b. The **engine register block** has so far refused every write —
R2's `INSTPM` did not latch, R5's `RING_CTL` did not latch. R6 is the rung that decides
whether that refusal is the absence of a held wake or a property of the part. Either answer
moves the GEN7-vs-Kepler decision; only one of them keeps the ladder alive.
