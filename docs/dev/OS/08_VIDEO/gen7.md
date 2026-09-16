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
  reclaim (§2.6). From R7 the gate additionally requires the engine to be **provably idle**,
  not merely disabled (§2.7) — a cleared enable bit is not a statement about a DMA already in
  flight.
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

## 2. Ladder state, R1 through R8

> **The metal record for every rung is [`SHUTOUT-REGISTER.md`](SHUTOUT-REGISTER.md) §4
> (R19 / ledger B10), and where this document and that register disagree, the register's
> quoted capture is the record.** The cells and prose below were corrected on 2026-09-15
> (GEN7DOC) against **flight 4, 2026-08-28, both boots** — in-tree reading
> `docs/dev/evidence/rmbp8/FLIGHT4-POSTMORTEM.md` §3.1, capture
> `~/unaos-bench/capture/rmbp8-flight4/ttyUSB0.log` (sealed, 2 343 810 B, 15 910 lines;
> boot 1 = L1–8871, boot 2 = L8872–15910). Read the register for the conditions each
> failure is recorded under; this file keeps the design.

| Rung | Name | Writes | Metal verdict |
| --- | --- | --- | --- |
| R1 | `recon` | none | GT block dark; `GTFIFOCTL` the only structured read |
| R2 | `wake` | 3 GT power-management regs, all reversed | `r2-unscorable-until-behavioural-witness battery=gt-still-dark` — GEN7R2, 2026-09-15: the battery reading is kept verbatim, the VERDICT now names the epistemic state (§2.2) |
| R3 | `forcewake` | 1 forcewake request reg per candidate, released in-rung | acquires fly; flight 4 read `gt-live-already by=none` with no ack decoded, and the battery that scored "does not open" is the instrument §2.2 records as blind — SHUTOUT-REGISTER §4 carries the reading |
| R4 / R4b | `claim` | ≤3 GGTT PTEs, all restored | GGTT PTE round-trip **proven** |
| R5 | `execute` | 2 GGTT PTEs + 4 RCS ring regs, all restored | `enable-void` — **failed under *no hold in force*, and R6 re-opened it by changing that one condition** (§2.5) |
| R6 | `rearm` | as R5, **under a held wake**, + 1 GTT-flush reg | **`r6-sentinel-hit by=mt … attempts=1/3`** — flight 4, both boots |
| R7 | `blit` | as R6 on the **BCS**, 3 GGTT PTEs + 4 BCS ring regs | **`r7-blit-verified … best_dst_match=256/256`, `dst_crc==src_crc`** — flight 4, both boots |
| R8 | `fb_blit` | as R7, **at the panel's pitch**: `1 + 4 + ceil((64+1)·pitch/4096)` GGTT PTEs (265 on the bench panel) + 4 BCS ring regs + 1 GTT-flush reg, all restored | **flew flights 8/9 2026-09-16 and refused on its own ceiling (R8CAP, §2.8); pending a metal run that reaches the blit.** Verdicts: `r8-fb-blit-verified` (the win) · `r8-fb-blit-verified-spill` · `r8-fb-blit-verified-head-stuck` · `r8-fb-blit-full-sentinel-miss` · `r8-fb-blit-partial` · `r8-sentinel-miss` · `r8-head-stuck`/`-partial` · `r8-enable-void-under-every-hold` · `r8-seed-collision` · `r8-ring-would-not-disable` · `r8-ring-drain-timeout` · the refusals `r8-gated-on-r7`, `r8-gated-on-wake`, `r8-refused-no-bar0`, `r8-refused-bpp-not-32`, `r8-refused-pitch-too-wide`, `r8-refused-panel-too-small`, `r8-refused-surface-too-large`, `r8-refused-bar0-too-small`, `r8-range-owned-refused`, `r8-fill-hypothesis-refuted`, `r8-alloc-failed`, `r8-virt-unmapped`, `r8-phys-above-4g`, `r8-pte-indistinct`, `r8-entry-drifted`, `r8-claim-write-void` (§2.8) |

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

**Verdict on metal: `gt-still-dark`** — `trans_untouched=0/14`
(flight 4, both boots: `r2 verdict=gt-still-dark trans_all=0/17 trans_untouched=0/14 struct=0
varies=0 poll_ack=1 poll_iters=0 rung=R2 wrote=3 reparked=1`). Three lessons the later rungs
are built on. First, `poll_ack=1 poll_iters=0` was **not** an ack: `0x22AC` read zero on the
first look and `== 0` was the pass condition, so a power-gated window that returns zero for
everything passed the poll on iteration zero. Every ack test from R3 onward is a
**transition** test with a stable-zero precondition. Second, draining a command streamer is
not the same act as powering one — the Sync-Flush sequence is a VT-d workaround, not a
general forcewake protocol, and the module never pretended otherwise.

**Third, and it is a correction to this rung's own verdict: the 17-register GT battery R2 was
scored on is not a liveness witness on this part.** The same flight-4 boots that produced
`gt-still-dark` also read `battery_moved=0/17` on R6 and R7 — through a *verified* 1 KiB
engine-side DMA (§2.7). A rung that takes "the battery moved" as its wake proof scores a
demonstrably working boot dead, which is what happened here. `gt-still-dark` therefore records
**"the battery did not move"**, not "the GT is dark".

#### GEN7R2 — the re-score, decided 2026-09-15

The queue row offered two ways to make this rung honest: score it through a behavioural witness,
or mark the verdict `unscorable-until-<battery>` on the wire. **The second was taken, and the
reason is structural rather than a deferral.** A behavioural witness — a ring arm, a sentinel —
needs two inputs R2 does not have at its call site: a GGTT window *proven unowned* (R4b) and a
*held* forcewake acquire (R6's discipline). R2 runs before R3 has returned a `GtWake` at all and
before R4 has read the GGTT. There is nothing to arm and nowhere to arm it. Moving the experiment
to where those inputs exist is not a change to R2; it is R6 and R7, which **have already run it**:
`r6-sentinel-hit` and `r7-blit-verified` are the behavioural answer to R2's question, on this
ladder, on this part, on flight 4.

So what R2 owed the wire was a label, and it now prints one:

```
:: gen7: r2 verdict=r2-unscorable-until-behavioural-witness battery=gt-still-dark
         instrument=battery17 instrument_blind=1
         blind_cite=flight4-2026-08-28-both-boots-battery_moved=0/17-through-a-verified-1KiB-engine-DMA
         trans_all=0/17 trans_untouched=0/14 struct=0 varies=0 poll_ack=1 poll_iters=0
         rung=R2 wrote=3 reparked=1 ::
```

Three properties of that line are deliberate. **`battery=` keeps the old token verbatim**, so every
flight-4-era capture and every `awk` written against it still compares field-for-field — the same
discipline GMUX-2 used when it left `why=pre-switch-not-accepted` unchanged and added a new
discriminating field beside it. **`instrument_blind=` is on the wire as a datum**, not implied by
the verdict string. And **the blindness reaches the NEGATIVE arm only**: a register that was dark
and came back structured, or that varies across a read-twice, *moved*, and nothing about a blind
instrument can manufacture that — so `gt-woke` is still a verdict and is still the STOP-HERE arm.
Collapsing both arms would have been the mirror of the original error.

The `next=` line now points the reader at the rungs that answer the question instead of at a
re-run of the same blind battery. Conditions and the queue row: `SHUTOUT-REGISTER.md` §4 (R2),
`docs/dev/OS/rmbp-queue.md` GEN7R2.

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
`STOP-RING_CTL-enable-did-not-latch-likely-forcewake-released-R6-must-hold-forcewake-and-rearm`.

**It was right, and R5's failure is now a "failed under", not a property of the part.** On
flight 4 R5 printed `r5 verdict=enable-void … ctl_wrote=00000001 ctl_readback=00000000` on both
boots, and minutes later in the same boots R6 wrote the identical enable bit with the acquire
still held and read back `ctl_readback=00000001`. One condition changed — the hold — and the
write latched. R5's recorded condition is **no forcewake hold in force**; the rung's code and
knob stay (R19).

### 2.6 R6 — `rearm` (the wake that makes RING_CTL latch)

R3 releases its forcewake acquire *inside its own rung*, by design — R3's job was to measure
the acquire, not to keep it. So by the time R5 wrote `RING_CTL`, no hold was in force. R6 is
the experiment that follows: acquire a candidate, **keep the hold across the whole
arm / submit / drain / disable / restore**, and only then release.

**Metal verdict — flight 4, 2026-08-28, both boots: `r6-sentinel-hit`.** The rung flew and
passed; this section's design is confirmed, not pending.

```
[  15736ms] :: gen7: r6 verdict=r6-sentinel-hit by=mt mode=scratch-fill wake=gt-live-already
            attempts=1/3 any_ctl_enabled=1 best_ctl_readback=00000001 any_head_moved=1
            any_sentinel=1 battery_moved=0/17 fw_restored=1 fw_evidence=real
            ring_regs_restored=1 ptes_restored=1 smear_post=0 reclaim=leaked
            tlb=tlb-flush-write-silent rung=R6
            note=hold-was-kept-ACROSS-the-arm-and-the-teardown-no-display-register-touched ::
```

(boot 1, L1483; boot 2 prints the same fields at `[12191ms]`, L10385.) Head advanced
`0 → 0x20` and sentinel `5EED1234` landed. The first candidate in the order below —
`mt`, `class=BDW-ONLY` — took it on `attempts=1/3`; `renfw` and `gtforceawake` were never
reached, so neither has a metal reading. The pre-registered falsifier (`ctl_readback==0x1`,
`head_moved=1`, `sentinel_hit=1`) is satisfied field-for-field, and
`r6-enable-void-under-every-hold` — the decisive negative that would have ended the x86
engine-offload programme — did **not** fire.

Two residuals from that boot, both carried forward rather than closed here: `fw_evidence=real`
on R6 against `fw_evidence=blind` on R7 (§2.7), and `battery_moved=0/17` while the enable
latched and the sentinel landed — the reading that retroactively blinds R2's instrument
(§2.2).

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

The flush register is `0x101008`, and as of 2026-08-27 it is **[PINNED]** — IVB PRM Vol1
Part3 (IHD-OS-V1 Pt 3 – 05 12) §1.2.21 `MI_UPDATE_GTT`, p.191: after CPU-side GTT updates
the driver must "write any value to MMIO address 0x101008" so system-agent TLBs are
invalidated before the pages are used. That pins the address and the write-any-value
semantic (so the capture/restore write-back is itself a second flush, not an un-flush). The
PRM gives **no** register name, format, or read decode — `GFX_FLSH_CNTL` is our label — so
the write stays under full capture / restore / re-read and keeps its own three-way verdict:

| `r6 tlb verdict=` | Meaning |
| --- | --- |
| `tlb-flush-decodes` | The readback changed. Something is there. |
| `tlb-flush-write-silent` | The readback did not change. **A self-clearing flush register and a non-decoding offset are indistinguishable here** — the rung says so instead of scoring a silent write as a success. |
| `tlb-flush-allones` | The offset read `0xFFFFFFFF` — the documented dead-well signature. |

**The reclaim is not gated on that register alone.** It fires on `reversal_clean` **AND**
(`never-fetched` **OR** `flush-verdict`), where `never-fetched` is an independent structural
proof: no candidate's `RING_CTL` enable ever read back set, the head never moved, and no
sentinel ever landed — so no engine access was ever issued through either GGTT address and
there is no cached translation to invalidate. **A register whose read decode is
unspecified — pinned write semantic or not — is never the sole reason a page goes back to
the heap.**

**On metal both legs are shut, and that is structural rather than a boot's luck.** On the
machine R5 flew on, `never-fetched` was the leg that fired. The moment R6 and R7 succeeded it
stopped firing — an enable that reads back set, a head that moves and a sentinel that lands are
exactly the three things `never-fetched` denies — while `0x101008` read `0` both before and
after the write on flight 4, both boots (`tlb=tlb-flush-write-silent`), so `flush-verdict` did
not fire either. The result on the wire is `reclaim=leaked pages=3 bytes=12288
reason=no-invalidation-evidence` on R7, and the same refusal on R5 and R6:
**r5 (2) + r6 (2) + r7 (3) = 7 pages / 28 672 B per armed boot, bounded and one-shot.** The
invariant behaved exactly as written. What flight 4 made unreachable is the *expectation* of
`reclaim=freed` carried in the falsifiers below.

#### GEN7TLB — the reclaim decision, 2026-09-15: HOLD

The queue row offered three moves: pin an alternative GGTT-invalidation witness, free the pages
under R7's idle proof, or promote the bounded leak to documented-accepted. **The decision is to
hold, and `reclaim=freed` is removed from the falsifier.**

*Why not an alternative witness.* There is none to pin. Intel's IVB PRM names exactly one
invalidation mechanism (Vol1 Part3 §1.2.21, "write any value to MMIO address 0x101008") and gives
it no name, no format and no read decode. On this part it reads `0` before and after the write, and
§2.6 above states why that is undecidable rather than negative: a self-clearing flush register and a
non-decoding offset are indistinguishable here.

*Why not the idle proof.* This is the tempting one and it is wrong. `engine_quiesced` is
`RING_CTL & 1 == 0` **and** `HEAD == TAIL` — two statements about the **command streamer** and about
the transfer it was handed. A GGTT translation is cached in the **system agent's** TLB. An idle
engine does not evict one; nothing in the architecture says it does, and we have no reading that
says it does either. Substituting idle for invalidation would be scoring a different question than
the one the free needs — LAWS §5's error shape, stated in its own words: *"say what the check
measures and what the decision needs; if those are different sentences, the gap is the error."* It
would also hollow out §2.6's own invariant: "an unpinned register is never the sole reason a page
goes back to the heap" would become "a register we did not even read is".

*What is accepted.* **r5 (2) + r6 (2) + r7 (3) = 7 pages / 28 672 B per ARMED boot**, plus R8's
`1 + 4 + dst_pages` (128 pages / 524 288 B on the bench panel) when `gen7r8` is armed too. One-shot,
bounded, on a 256 MiB x86 heap, behind a diagnostic knob that never reaches shipping media. That is
0.011 % of the heap for the gen7-only line and 0.21 % with R8. The cost is real and it is the
cheaper of the two failures available: a 16 KiB DMA into re-issued kernel heap is not.

*What changed on the wire.* Until this arc a healthy armed flight and a drain timeout both printed
`reclaim=leaked`, and only `reason=` told them apart — a decision and an alarm wearing the same
word. The state is now three-valued in R6, R7 and R8:

| `reclaim=` | Meaning |
| --- | --- |
| `freed` | a reclaim leg fired (`never-fetched` or `flush-verdict`); the pages are back on the heap. |
| `held` | **DECIDED RETENTION.** Every write restored, the engine quiesced, and no GGTT invalidation witness exists on this part. This is the EXPECTED state of a healthy armed flight, and it is what flight 4 would have printed. |
| `leaked` | **SAFETY REFUSAL.** The reversal did not verify — a ring that would not disable, a drain that timed out, a smear. Something is wrong, and it must not read the same as the healthy case. |

`reason=` is deliberately unchanged (`no-invalidation-evidence` on the held path), so flight-4-era
captures still compare field-for-field. The queue row `GEN7TLB` closes on this decision; the falsifier
in §2.7's watch line 1 is corrected to match.

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

#### The metal falsifier, and how it was met

Stated before the boot: **with a wake held, `ctl_readback==0x1`, `head_moved=1`,
`sentinel_hit=1`.**

If `ctl_readback` stayed `0x00000000` under **every** candidate hold, the engine register
domain was not writable on this part on any documented register, and the x86 engine-offload
programme was dead on documented registers. That was a finding worth the boot, and the rung
states it in those words on its `next=` line.

**Flight 4 met the falsifier, both boots**, on the first candidate and the first attempt:
`best_ctl_readback=00000001 any_head_moved=1 any_sentinel=1 attempts=1/3 by=mt`. The
alternative — `r6-enable-void-under-every-hold` — did not print on either boot. The engine
register domain **is** writable on this part under a kept hold.

### 2.7 R7 — `blit` (the BCS moves pixels, or says why not)

R6 asked whether **any** ring's `RING_CTL` latches under a held wake, and whether the CS
retires a bare `MI_STORE_DATA_IMM` on the RCS. R7 keeps that entire envelope — the same
`R6_CANDS` order, the same acquire/hold-across-the-arm/release discipline, the same
proven-unowned GGTT window, full capture/restore/re-read on every write — and moves it to the
**BCS** with a real 2D command: `XY_SRC_COPY_BLT` copying a 16x16x32bpp rectangle (1 KiB) from
a seeded source page into a destination page.

Three pages instead of two: `ring`, `src`, `dst`. Three GGTT slots (`0x10000`, `0x10001`,
`0x10002`), the R5/R6 window extended by one.

#### R7 flew, and `write_ok()` was never a launch switch

**Metal verdict — flight 4, 2026-08-28, both boots: `r7-blit-verified`. The BCS copied
pixels.**

```
[  15744ms] :: gen7: r7 cand=mt class=BDW-ONLY col=exec ctl_wrote=00000001
            ctl_readback=00000001 ctl_enabled=1 head_at_arm=00000000 head_post=00000040
            head_moved=1 tail=00000040 sentinel_seed=DA7A5EED sentinel_post=0B75C0DE
            sentinel_hit=1 dst_match=256/256 dst_crc=D7994E3D src_crc=D7994E3D armed=1
            iters=3 cyc=7396 budget=50000000 ::
[  15744ms] :: gen7: r7 cand=mt col=drain ring_idle=1 drain_iters=0 drain_cyc=28
            head_drain=00000040 tail=00000040 sentinel_drain=0B75C0DE sentinel_hit=1
            dst_match=256/256 dst_crc=D7994E3D exec_sentinel_hit=1 exec_dst_match=256/256
            settled=1 ::
[  15744ms] :: gen7: r7 verdict=r7-blit-verified by=mt mode=scratch-fill wake=gt-live-already
            engine=BCS attempts=1/3 … any_copy=1 best_dst_match=256/256 battery_moved=0/17
            fw_restored=1 fw_evidence=blind ring_regs_restored=1 all_disabled=1
            all_ring_idle=1 ptes_restored=1 smear_post=0 reclaim=leaked
            tlb=tlb-flush-write-silent rung=R7 ::
```

Boot 1 at `[15744ms]` (L1565); boot 2 repeats every field at `[12199ms]` (L10467), differing
only in `cyc=7112`. `dst_crc == src_crc` on both boots, so the bytes-not-DWords pitch ruling
at the end of this section is metal-proven.

**Correction to this section's premise, and it was a misreading of our own code.** `write_ok()`
is not a launch gate and never was: it is `GtWake::write_ok()` (`gen7.rs:1388`), which returns
true for `GtWake::Woke | GtWake::LiveAlready` — R3's wake verdict, nothing else. There is no
seat-held switch behind it to open. On flight 4 R3 returned `gt-live-already`, so every rung
printed `write_ok=1` on its own `begin` line — `r5 begin … write_ok=1`, `r6 begin … write_ok=1`,
`r7 begin … write_ok=1` — and R7 wrote. The sentence that said R7 was "held dark by the same
`write_ok()` gate" described a gate that does not exist; what actually holds R7 dark is a boot
whose R3 returns `Dark` or `WokeNoAck`.

On such a boot R7 still performs its full read-only census before the gate: ~130 MMIO reads
(the entry battery, the BCS ring-register images, the GGTT window and its neighbours, the six
far probes, `GTFIFOCTL` twice) plus one PCI config read of the host bridge's `BDSM`. That
census is the rung's value on an unconfirmed wake, and it must stay reachable.

The 2D encodings were **[EXT-UNPINNED]** when R7 was built; as of 2026-08-27 every one of them
is **[PINNED]** against the IVB PRM (the table at the end of this section), and flight 4 then
executed them.

#### The two witnesses, and the rule that each speaks alone

R7 is the first rung with two independent success witnesses, and the first where they can
disagree:

| Witness | Column | What it proves |
| --- | --- | --- |
| **copy** | `dst_match=N/256` | the CS parsed `XY_SRC_COPY_BLT` and moved pixels |
| **retire** | `sentinel_hit` | `MI_STORE_DATA_IMM` took effect |
| **head** | `head_post == tail` | the ring retired to its tail |

`dst_crc` / `src_crc` cross-check the copy (a perfect copy makes them equal).

**Each witness speaks without permission from the others.** As shipped in `f0daf518` every
copy arm was conditioned on `sentinel_hit` first, so `copy_full && !sentinel_hit` fell through
to `sentinel-miss` and was reported as `r7-sentinel-miss` — *a blit failure* — while
`best_dst_match=256/256` sat on the same summary line. A full copy is a 2D positive whether or
not the store lands. The classification chain now gives every combination its own name.

#### The ordering barrier

`XY_SRC_COPY_BLT` (DW0-7) is followed by `MI_FLUSH_DW` (DW8-11) and only then
`MI_STORE_DATA_IMM` (DW12-15). Without the flush nothing stops the sentinel store retiring
while the 1 KiB copy is still in the engine's write pipeline — which would make a short
`dst_match` indistinguishable from a wrong `BR13` field, destroying the one discrimination the
rung exists to make. The ring tail is unchanged at `0x40`: the flush occupies exactly the four
`MI_NOOP` dwords the previous layout used as padding, so the tail stays QWord-aligned
(§1.1.11.1 p.75).

The flush's encoding was [EXT-UNPINNED] when this barrier was designed and is now pinned
(Vol1 Part4 §2.2.5, pp.137–139), but the rung still does not rest the discrimination on any
single encoding. The pixel witness is sampled **twice** and both readings go on the wire:

| Column | When | Reading |
| --- | --- | --- |
| `col=exec` | the instant the exec poll exited | earliest, may be short because we looked early |
| `col=drain` | after the ring drained to `HEAD==TAIL` | settled; `settled=1` when the two agree |

`col=exec` short with `col=drain` at 256 means *we looked too early*. Both short on a drained
ring means the copy genuinely did not land, and `BR00`/`BR13` are the suspect. That leg depends
on no encoding at all — only on `HEAD==TAIL` — so it survives a wrong `MI_FLUSH_DW`.

#### The teardown gate — disabled is not idle

R6's ring held one 4-byte `MI_STORE_DATA_IMM`, so "the enable bit read back clear" was a
serviceable stand-in for "the engine is done". **R7's ring holds a 1 KiB engine-side DMA write
into `dst_page`, and the stand-in stops holding.** Clearing `RING_CTL` bit 0 stops the command
streamer *fetching*; it retires nothing already handed to the engine. Between "stop fetching"
and "the last byte has landed" there is a window, and unmapping the destination PTE inside it
aims that write at whatever the GGTT slot resolves to next.

`f0daf518` computed the drain witness and then used it only in a log line. It is now
load-bearing:

```
engine_quiesced = all_disabled && all_ring_idle
```

`all_disabled` = `RING_CTL & 1 == 0`, read back. `all_ring_idle` = every armed candidate's ring
reached `HEAD==TAIL` (§1.1.11.4 p.78's clean point) inside `DRAIN_BUDGET_CYC`. **Both, or the
pages do not move.** `engine_quiesced` gates the GGTT unmap *and*, through `reversal_clean`,
the `dealloc` — two independent barriers, because a skipped unmap also forces `ptes_restored`
false. On the un-armed path — the expected outcome on this part — `all_ring_idle` is vacuously
true and nothing that used to reclaim stops reclaiming.

The three predicates live between `R7-TEARDOWN-GATE-BEGIN` / `-END` markers in `gen7.rs` as
pure `const fn`s, for one reason: they are the only thing standing between a live DMA engine
and reused kernel heap, and pure functions can be driven with a **forced drain timeout outside
the kernel**. See §3.1.

#### Verdicts

| `r7 verdict=` | Meaning |
| --- | --- |
| `r7-gated-on-wake` | R3 did not confirm a wake. **Nothing written.** |
| `r7-range-owned-refused` / `r7-fill-hypothesis-refuted` | The GGTT window is not provably unowned. Nothing written. |
| `r7-claim-write-void` | A PTE did not land, or a neighbour smeared. The ring was never armed. |
| `r7-enable-void-under-every-hold` | Every candidate hold was taken and BCS `RING_CTL` still read back `0`. |
| `r7-blit-verified` | **The win.** Enable latched, 2D parsed, all 256 dwords copied, store retired, head at tail. |
| `r7-blit-verified-head-stuck` | Copy **and** store landed; only the head never reached tail. The 2D block is proven by 256 dwords — suspect retirement, not the encoding. |
| `r7-blit-full-sentinel-miss` | All 256 dwords copied, store did **not** land. Also a 2D positive; suspect `MI_STORE_DATA_IMM` or the flush ahead of it. |
| `r7-blit-partial` | The copy ran but not every dword landed. Compare `col=exec` against `col=drain` before pinning `BR00`/`BR13`. |
| `r7-sentinel-miss` | The head retired, no pixel moved, no store landed. |
| `r7-head-stuck` / `-partial` | The enable latched but the CS did not parse the ring. |
| `r7-ring-would-not-disable` | **Safety override.** `RING_CTL` bit 0 would not clear. PTEs left claimed, pages leaked. |
| `r7-ring-drain-timeout` | **Safety override.** The enable cleared but `HEAD` never reached `TAIL` — the engine may be mid-DMA into `dst_page`. PTEs left claimed, pages leaked. |

The last two dominate every exec reading. `r7-blit-verified-head-stuck`,
`r7-blit-full-sentinel-miss` and `r7-ring-drain-timeout` are new in this arc; the first two
were previously collapsed into `r7-blit-partial` and `r7-sentinel-miss`, and the third was
invisible — a drain timeout freed the pages.

#### What was unpinned, and is now pinned

All six R7 hypotheses were verified 2026-08-27 against Intel's official IVB PRM PDFs
(intel.com `cdrdv2-public` / the x.org `docs/intel/IVB` mirror of the same documents). The
clean-room line stood throughout: `i915` and `i965` were not opened. **Every constant was
CONFIRMED as coded — no value changed.** Two of the old table's *volume pointers* were wrong
and are corrected below: in the IVB PRM set the blitter is Vol1 **Part4** (Part5 is the
video-codec engine CS — the Part5 recollection was SNB's numbering), and `MI_FLUSH_DW` for
the BCS lives in Part4 with the blitter, not Part3 (Part3's render-engine `MI_FLUSH` section
itself notes the other engines use `MI_FLUSH_DW`).

| Constant | Value | Verdict | Pin |
| --- | --- | --- | --- |
| `XY_SRC_COPY_BLT_DW0` (BR00) | `0x54F00006` | **CONFIRMED**, all 32 bits | Vol1 Part4 §1.9.14 pp.62–63: Client[31:29]=02h, Opcode[28:22]=53h, 32bpp Byte Mask[21:20]=11b (alpha+RGB), [19:16]/[14:12]/[10:8] MBZ, [15]=0 src linear, [11]=0 dst linear, DWord Length[7:0]=06h (bias 2, 8 DW total) |
| `BR13` | `0x03CC0040` | **CONFIRMED** | Vol1 Part4 §1.9.14 p.63: [31] MBZ, [30] clip=0, [29:26] MBZ, Color Depth[25:24]=11b 32-bit, ROP[23:16]=CCh — code CC = "S" in the §1.2.1.3 table (p.11), the source-copy op (§1.2.2 example, p.28) — pitch[15:0]=64 bytes (see note) |
| `SRC_PITCH` / `RECT_WH` / `RECT_00` | `64` / `16x16` / `0` | **CONFIRMED** | Vol1 Part4 §1.9.14 p.63: DW2=dstY1\|X1, DW3=dstY2\|X2 (Y2=Bottom[31:16], X2=Right[15:0]), DW4=dst base, DW5=srcY1\|X1, DW6=src pitch[15:0] ([31:16] MBZ), DW7=src base — exactly the ring's `add(0..7)` order |
| `MI_FLUSH_DW_OPCODE` | `0x26` | **CONFIRMED** | Vol1 Part4 §2.2.5 p.137 ("Source: BlitterCS"): DW0[28:23] default 26h `MI_FLUSH_DW`; Command Type[31:29]=0h |
| `MI_FLUSH_DW_TOTAL_DW` | `4` | **CONFIRMED** | Vol1 Part4 §2.2.5 pp.138–139: DWord Length[5:0] = Total−2, listed default 2h ("2 for QWord"); layout = DW0 header, DW1 address, DW2..3 imm QWord = 4 DW. DW1..3=0 valid — address/imm ignored when post-sync[15:14]=0h "No write" |
| `HYP_GFX_FLSH_CNTL` | `0x101008` | **PINNED** (address + write semantic) | Vol1 Part3 §1.2.21 `MI_UPDATE_GTT` p.191: "write any value to MMIO address 0x101008" to invalidate system-agent TLBs after CPU-side GTT updates. No name/format/read-decode in the PRM — the reclaim invariant (§2.6) stands |

**The pitch-unit note.** §1.9.14's DW1 row header reads "Destination Pitch in DWords" —
taken alone it would make `64` wrong (16 px × 4 B = 16 DWords). The PRM's own register pages
contradict that phrase for the linear case, and *bytes* wins on three independent legs:
§1.10.7 `BR11` (p.91) says the linear XY-blit pitch field is a byte specification; §1.10.9
`BR13` (pp.92–93) defines the field as the memory-address offset added per scan line; and the
§1.2.2 worked examples (pp.27–28) program `400h` = 1024 **bytes** for a 1024-px 8bpp surface.
The "in DWords" phrase belongs to the tiled encodings (BR11's granularity note). `64` stands.
If metal ever returns `r7-blit-partial` with row-wrapped `dst_match` geometry, re-open this
note before touching the constant.

**Residuals, named so they are not mistaken for pins left undone:**

- The PRM does not state whether X2/Y2 are inclusive or exclusive; the `dst_match` witness
  discriminates that on metal. Not an encoding pin — both readings are built from the same
  pinned DW layout.
- The on-wire provenance labels (`witnessed_write(..., "EXT-UNPINNED")` for `GFX_FLSH_CNTL`,
  and the one `next=` STOP string that cites "Vol1-Part5") are **serial-output strings**, out
  of scope for a doc-and-comment pin pass; relabelling them is the seat's one-line follow-up.

The MI **header format** used to build `MI_FLUSH_DW_DW0` was already pinned (§1.2.17 p.186:
Type[31:29], Opcode[28:23], DWord Length = *Total Length − 2*); the §2.2.5 layout confirms
the same arithmetic with the length field at [5:0] and [7:6] MBZ — identical bits for the
value 2.

**Nothing remained unpinned, R7 flew, and every constant above was exercised on silicon.**

#### The metal falsifier, and what the watch lines read

Stated before the boot: **with a wake held, `ctl_readback==0x1`, `head_moved=1`,
`sentinel_hit=1`, `dst_match=256/256`.** **Flight 4 satisfied it field-for-field on both
boots.** Three things silicon had to confirm that no host gate can, and what it answered:

1. **The drain gate never fires spuriously.** On a healthy boot expect
   `all_ring_idle=1 engine_quiesced=1` and `reclaim=freed`. An `r7-ring-drain-timeout` on a run
   that otherwise looks clean means `DRAIN_BUDGET_CYC` (≈8 ms) is too short for a 1 KiB blit on
   this part — raise the budget, do **not** relax the gate.
   **Read on metal: half met.** `all_ring_idle=1 all_disabled=1` and no drain timeout on either
   boot, in `drain_iters=0 drain_cyc=28` of a 20 M-cycle allowance. But `reclaim=freed` did not
   appear and **cannot**: the flush register is silent on this part and the engine did fetch, so
   neither reclaim leg can fire (§2.6). This expectation is the one part of the falsifier that
   was wrong about the machine rather than about the rung.
   ⚠ **CORRECTED by GEN7TLB, 2026-09-15 (§2.6).** `reclaim=freed` is struck from this expectation
   and replaced: on a healthy boot expect `all_ring_idle=1 engine_quiesced=1` and **`reclaim=held
   reason=no-invalidation-evidence`**. `reclaim=leaked` is now reserved for the safety refusals, so
   it reappearing on an otherwise-clean run IS a finding again instead of the normal case.
2. **The flush framing.** If R7 returns `r7-head-stuck` or `r7-sentinel-miss` where R6 — same
   hold, same ring, one command — returned `r6-sentinel-hit`, the delta is the `MI_FLUSH_DW`
   block mis-framing the store that follows it, **not** the 2D block. The encoding is now
   pinned (Vol1 Part4 §2.2.5), so that signature would mean the store's privilege/address or
   a misread of the pin — re-derive from the Part4 PDF before touching code.
   **Read on metal: not exercised.** R6 and R7 both passed, so the signature never arose. The
   tripwire stays armed for R8.
3. **`settled=`.** `settled=0` with `col=drain` at 256 is the barrier doing its job late and is
   informative, not a failure. `settled=0` with `col=drain` *below* `col=exec` would mean the
   engine wrote `dst_page` after we thought it was idle — that is the G1 hazard observed live,
   and it is a STOP.
   **Read on metal: `settled=1`, both boots**, with `col=exec` and `col=drain` both at
   `dst_match=256/256`. The G1 STOP condition is excluded **by measurement**, not by argument.

**One residual the flight added.** R7 reports `fw_evidence=blind` and
`classification=fw-no-decode` on the same line as a verified 1 KiB engine DMA (R6, one rung
earlier, read `fw-req-decodes-no-ack fw_evidence=real`). The hold works — the blit is the proof
— and the *evidence channel* does not. **R8 must not gate on a forcewake ack decode**; the
hold-across-the-arm discipline plus behavioural witnesses is the only currency this part
honours. `SHUTOUT-REGISTER.md` §4 (R3) records the conditions.

### 2.8 R8 — `fb_blit` (the same blit at the panel's own geometry)

Knob: `UNAOS_IVB3D_R8=1` → features `gen7,gen7r8`. **Default OFF**; `gen7r8 = ["gen7"]`, and the
rung is `mod r8` inside `gen7.rs` with its single call site at the tail of `blit()`. Status:
**flown and refused on flights 8 and 9 (2026-09-16); the refusal was the rung's own ceiling, fixed
below; still pending a metal run that reaches the blit.**

R7 proved the BCS parses `XY_SRC_COPY_BLT` and moves pixels on this part. It proved it in the
smallest possible case, and that case was degenerate in four ways at once: a **16×16** rectangle,
at destination origin **(0,0)**, across a **64-byte pitch**, inside a **single GGTT page** whose
source was one page too. None of those is what a compositor asks a blitter for, and each is a real
failure mode this part could still have. R8 is the identical command with all four removed:

| Degeneracy R7 left | What R8 exercises | What a failure would mean |
| --- | --- | --- |
| pitch = 64 B | the panel's own pitch (`stride × bpp`; **16 384 B** on the bench rMBP) | the pitch-unit question §2.7 settled *by reading* — at 16×16-in-one-page a wrong unit still lands inside the page, so the read was never tested |
| origin (0,0) | the **top-right** corner, `X1 = width − 64` (2 816 on the bench panel) | DW2/DW3's X/Y fields were exercised only at zero; an **inclusive** X2 spills into the next row's first pixel, which is the §2.7 residual |
| one destination page | **260 pages**, one PTE each | the engine's per-page GGTT walk — the premise of a GGTT, and untested until now |
| one source page | 4 pages at a 256-byte source pitch | the same, on the read side |

**It does not touch the panel, and that is a finding rather than a caution.**
`SHUTOUT-REGISTER.md` §5 G4 is `proven`: `SW_DISPLAY=0x03 (DISCRETE)`, `SW_DDC=0x02 (DISCRETE)`,
stable at Boot **and** at Kernel — the Kepler dGPU owns the panel at every observed instant on this
machine — and G7, the gmux switch to IGD, is `never-run`. A gen7 blit aimed at the live scanout
would therefore be two bad things at once: it would prove nothing visible, because nothing the IGD
writes is being scanned out; and it would be a write into **another device's VRAM aperture**, which
is the class of act this ladder's safety argument is built on never doing. So R8's destination is a
CPU-readable heap surface **carrying the panel's geometry**, the visible copy is deferred to the
rung after a gmux switch persists (queue rows `GMUX-2`, `A7`), and this paragraph is the reason,
written down before the flight rather than after it.

#### The three witnesses

| Witness | Column | What it proves |
| --- | --- | --- |
| **copy** | `rect_match=N/4096` | the CS parsed the command and moved the rectangle's pixels |
| **geometry** | `spill=N` | dwords **outside** the rectangle that no longer carry their seed |
| **retire** | `sentinel_hit` | `MI_STORE_DATA_IMM` took effect, from the slack row |

`spill` is the witness R7 could not have had: at 16×16 into a 4 KiB page there was nowhere for a
mis-pitched or edge-inclusive blit to land that the rung was looking at. Here there is, and it is
counted — which is why the classification puts `copy_full && spill > 0` **ahead** of the sentinel
and the head. A full copy that also painted somewhere else is a geometry finding and must never be
reported as a clean win because the store happened to retire.

Reading a non-zero `spill` is arithmetic, and the two answers are far apart:

- `spill ≈ pitch/4` dwords (**4 096** on the bench panel — the stride in pixels, not the width) — one
  whole extra **row** moved: a pitch-unit error, the §2.7 note coming true.
- `spill ≈ R8_RECT_H` dwords (64 — one per row) — one extra **column**: `X2` is **inclusive**. That
  answers the §2.7 residual the PRM does not state, and it answers it in our own surface, which is
  why the destination carries **one slack row past the rectangle**: an inclusive edge lands inside
  the allocation and is counted instead of running off the end.

#### The pre-blit control

A match count is worthless unless it started at zero, so every attempt runs the full scan **before**
the arm and prints `preblit rect_match=0/4096 spill=0 seed_ok=1`. The two seed generators are
different multiplicative hashes; if they ever collide the control says so and
`r8-seed-collision` **dominates the copy verdict** — an instrument that did not start clean cannot
report a transition. That is R2's lesson (§2.2) applied prospectively instead of retroactively.

#### What R8 inherits unchanged, and what it must not do

- **The wake discipline is R6's**: one `fw_acquire` per candidate over `R6_CANDS` in the same order,
  **held across the arm, submit, drain, disable and restore**, released last.
- ⚠ **R8 MUST NOT GATE ON A FORCEWAKE ACK DECODE** (`SHUTOUT-REGISTER.md` §4, R3). R7 read
  `fw_evidence=blind classification=fw-no-decode` on the same line as a verified 1 KiB engine DMA.
  R8 arms under each hold regardless of `acked()`, exactly as R6 and R7 do, and every arm of its
  verdict rests on behaviour.
- **The teardown gate is R7's, reused and not copied**: `r7_engine_quiesced`, `r7_reversal_clean`
  and `r7_reclaim`, the three `const fn`s between the `R7-TEARDOWN-GATE-BEGIN`/`-END` markers. The
  go-red harness of §3.1 therefore proves R8's gate as well as R7's, and there is no second copy to
  drift.
- **Every window PTE is read before any is written.** R4–R7 read three slots and two neighbours;
  R8's window is `1 + 4 + dst_pages` wide (**265** on the bench panel) and **every** slot is censused,
  because "no rung writes a GGTT entry it did not first prove unowned" is a claim about each entry
  and a sampled proof would be a weaker, different claim.
- **Every write captured, restored and re-read**: the forcewake request register, the four BCS ring
  registers (restored only once the ring is confirmed disabled, §1.1.11.2 p.76), every slot of the
  window plus both bracketing neighbours, and the one GTT-flush register. **No display register is
  touched at all**, and the framebuffer is never written.

#### The rung dependency, and where it is wired

R8 runs **only** on a boot whose R7 verdict is `r7-blit-verified` (R19: "every ladder rung names the
earlier rungs it needs open"). On any other boot it prints `r8-gated-on-r7` and writes nothing,
because a boot on which the blitter did not blit cannot distinguish "R8's geometry is wrong" from
"the engine did not run today".

That dependency is wired by calling R8 **from the tail of `blit()`** rather than from `igpu::init`,
for two reasons that are both invariants. The verdict is a local of `blit()`: handing it out through
the call site would mean changing `blit`'s signature and `igpu.rs`, and would let a future caller
pass a verdict R7 never reached. And the call is the last statement in the file's last function, so
with `gen7r8` off there is nothing below it to shift and no `panic::Location` line moves.

#### Refusals — stated, never trimmed

`r8-refused-bpp-not-32` · `r8-refused-pitch-too-wide` (BR13's pitch field is 16 bits; a wider panel
needs an encoding this rung does not carry, so it is refused rather than truncated) ·
`r8-refused-panel-too-small` (which also refuses `stride < width`, a malformed surface description
rather than a small one) · `r8-refused-surface-too-large` (the window reservation, below). **A rung
that silently shrinks its own experiment reports a verdict about a different experiment**, so each of
these parks and says so.

#### R8CAP — the ceiling was a constant, and it refused the experiment (flights 8 and 9, 2026-09-16)

Both flights reached R8 with R7 green on the same boot and neither ran the blit:

```
:: gen7: r7 verdict=r7-blit-verified by=mt mode=scratch-fill … best_dst_match=256/256 …
:: gen7: r8 verdict=r8-refused-surface-too-large dst_bytes=1064960 max=1048576 writes=0
   note=refused-not-trimmed-a-rung-that-shrinks-its-own-experiment-reports-a-verdict-about-a-different-experiment ::
```

**Where the pitch comes from, measured rather than inferred.** It is the firmware's, and the takeover
already publishes it: `:: KDHEAD: gop w=2880 h=1800 vram_off=00020000 pitch=16384`. The rung reads the
same number through `video::WRITER`, which prints it on its own line —
`fb=panel fb_w=2880 fb_h=1800 stride_px=4096 bpp=4 fb_len=29491200 dst_pitch_bytes=16384 dst_rows=65
dst_span_bytes=1064960 dst_bytes=1064960 dst_pages=260`. So **16 384 B/row is the real scanout pitch,
not an over-estimate**: the GOP and the kernel's framebuffer agree, and `fb_len` confirms it a third
way independently of both (`29 491 200 = 1800 × 16 384`). The rMBP's firmware pads a 2 880-pixel panel
to a **4 096-pixel stride** — 11 520 B of pixels in a 16 384 B row — and every consumer sees the padded
stride, which is correct and is what a blit must carry.

Two arithmetic corrections that a reader of the refusal line alone would get wrong, and which are the
reason (b) below exists: the destination is **65 rows**, not 64 (the rectangle plus the deliberate
slack row of §2.8's spill argument), so `1064960 / 64 = 16640` is not the pitch — `1064960 / 65 =
16384` is. And the refusal named a byte count, so the resource that actually ran out was invisible.

**The mechanism.** `R8_DST_MAX_BYTES` was the literal `1024 * 1024` with a comment asserting "the
bench rMBP's 1920×1200 panel asks for 123" destination pages. The bench rMBP is 2 880×1 800 and asks
for 260. But the ceiling's real job was never memory at all: it was the sole term sizing the
fixed PTE table, `let mut ptes = [0u32; 1 + 4 + (R8_DST_MAX_BYTES / 4096)]` — 261 entries — while the
claim loop walks `win_slots = 1 + src_pages + dst_pages` = **265**. The table was four entries short,
the ceiling was the thing standing between that and an out-of-range index, and nothing in the file
said so. **A number that protects an array should be written in the array's terms**; this one was
written in megabytes, and so it was tuned against a panel the machine does not have.

**The new bound is derived, and the derivation is the window.** `R8_WIN_SLOTS_MAX = 1 + R8_SRC_PAGES +
R8_DST_MAX_PAGES` is stated once and sizes `ptes`; `R8_DST_MAX_BYTES = R8_DST_MAX_PAGES × 4096` falls
out of it. `R8_SRC_PAGES` and `R8_DST_ROWS` are derived from the rectangle (replacing a bare `4` and a
second spelling of `R8_RECT_H + 1`), with `const _: () = assert!(…)` holding the table's size and its
consumer in the same terms at compile time. The one remaining choice is `R8_MAX_STRIDE_PX = 4096`,
taken from the `KDHEAD` line above, giving 65 × 4 096 × 4 = 1 064 960 B = **260 destination pages and
a 265-slot window** — exactly the bench panel, and every narrower one including 3 840-wide UHD at the
same padded stride.

**What the reservation costs, and why it stops there.** The table is this function's stack frame, at
4 bytes per slot. The old 261-entry table was 1 044 B inside `fb_blit`'s measured `sub $0xca8,%rsp` =
**3 240 B** frame; 265 entries is 1 060 B, so the new frame is **≈3 256 B, +16 B**. Reserving instead
for BR13's widest *encodable* pitch (65 535 B × 65 rows = 1 041 pages, a 1 046-slot window) would cost
4 184 B of table and put `fb_blit` at a ~6.4 KiB frame — and **no function in this kernel's x86
artifact has a frame above 4 096 B** (`objdump -d` over every `sub $imm,%rsp`; `fb_blit` at 3 240 B is
already the second largest). Making the boot path carry the largest stack frame in the kernel, by
1.6× the established maximum, to cover panels no bench machine has, is a worse trade than a refusal
that names its own shortfall. So the refusal stays, and it stays **reachable**: a stride between 4 097
and 16 383 pixels clears `r8-refused-pitch-too-wide` and lands here.

**(b) The refusal now names the shortfall**, scored on slots rather than bytes, so the next flight
does not have to re-derive it (and get 16 640):

```
:: gen7: r8 verdict=r8-refused-surface-too-large dst_bytes= max= rows= pitch= stride_px=
   max_stride_px= slots_have= slots_need= short_by= writes=0 …
:: gen7: r8 next=STOP-window-reservation-short-raise-R8_MAX_STRIDE_PX-to-cover-stride_px-above-the-cost-is-4-bytes-of-stack-per-slot …
```

Had that line existed for flight 8 it would have read `rows=65 pitch=16384 stride_px=4096
slots_have=261 slots_need=265 short_by=4`, and the fix would have been legible from the wire.

#### The metal falsifier, stated before the boot

**With a wake held and R7 verified on the same boot: `ctl_readback==0x1`, `head_moved=1`,
`rect_match=4096/4096`, `spill=0`, `sentinel_hit=1`, `settled=1`** → `r8-fb-blit-verified`.

Three named alternatives and what each would mean:

1. **`r8-fb-blit-verified-spill`** — the engine works and the geometry does not. Read `spill`
   against `pitch/4` and against `R8_RECT_H` per the arithmetic above; the fix is a **field**, never
   an opcode, and §2.7's pin table is not re-opened by it.
2. **`r8-fb-blit-partial` with `col=exec` short and `col=drain` full** — we looked too early, and
   `EXEC_BUDGET_CYC` needs raising for a 16 KiB transfer (R7 moved 1 KiB in 7 396 cycles of a 50 M
   allowance; 16× the bytes is still three orders of magnitude inside it, so this would itself be a
   finding). **Both short on a drained ring** is a short DMA or a GGTT page that did not translate —
   read the `claim` line's `ptes_landed` first.
3. **`r8-enable-void-under-every-hold` after R7 latched on the same boot** — the engine register
   domain is demonstrably writable this boot, so the delta is R8's own ring programming. Diff the
   two ring images DW0–DW7; do not re-open the forcewake question.

Expected on a healthy armed boot and **not** a defect: `battery_moved=0/17` (§2.2's blind
instrument, kept on the wire precisely so a reader sees it stay at zero while the engine works),
`fw_evidence=blind` (§2.7's residual), and `reclaim=held reason=no-invalidation-evidence` (§2.6's
GEN7TLB decision).

---

## 3. Verification

**QEMU has no Ivy Bridge IGD.** `./arroyo check` proves types and the cfg lattice and
**nothing about any rung**; the feature banner proves a feature is compiled, not that a
witness is reachable. The honest gates are:

1. `./arroyo check` for both arches, plus `UNAOS_WC=1 ./arroyo check` and
   `UNAOS_WC=1 UNAOS_IVB=1 UNAOS_IVB3D=1 ./arroyo check` with `gen7` in the feature banner.
   A knob-gated change type-checked only knob-off is **not** gated: the armed run is required,
   and the banner must actually read `...,intel-ivb,unaos_ivb,gen7`.
   **R8's armed polarity is a named leg, not an inference** (GEN7NEXT, 2026-09-15): `gen7r8` is the
   last entry of `arroyo`'s `x86-all` row, which already carries `gen7`, so every `./arroyo check`
   compiles `gen7 + gen7r8` ON. Proven by mutation rather than by reading — a deliberate type error
   planted inside `mod r8` reds the armed leg (`rc=101`, `error[E0308]` naming the line) and leaves
   the `gen7`-only leg green (`rc=0`), which is also the proof that the module is genuinely absent
   when the sub-knob is off.
2. The x86-fat knob-off battery, proving the disarmed image is unchanged.
3. **`LC_ALL=C grep -a -o -F` on the armed `esp-x86` artifact (never `strings` — LAWS §5, counts moved 320→322)**, proving every verdict token is present in
   the shipped ELF — not merely compiled behind a `cfg`.
4. **The R7 teardown-gate go-red** (§3.1) — for any change to the unmap/free gate, a
   demonstration that a non-idle engine cannot reach the page free, not an argument that it
   cannot.
5. **Metal.** The falsifiers above.

One deliberate certification exception: `r6-ring-addr-illegal` (and R5's identical
`ring-addr-illegal`) does not appear in the artifact, because `ring_gtt_addr` is a
compile-time constant and rustc proves the branch dead. That is the `RING_BUFFER_START`
bits[31:29] invariant being discharged **at compile time** — stronger than a runtime check,
not absent. If the ring slot is ever made non-constant, the branch and its token return.

R7 carries the same exception for `r7-ring-addr-illegal`, and R8 for `r8-ring-addr-illegal`, for
the same reason: `R8_BASE_SLOT` is a constant, so rustc proves the branch dead. If that slot is ever
made non-constant, the branch and its token return.

### 3.1 The R7 teardown-gate go-red

`./arroyo check` proves the gate compiles. It cannot prove the gate *refuses*, and neither can
metal: QEMU has no Ivy Bridge IGD, and on the one flight that armed R7 the engine quiesced
cleanly (`all_ring_idle=1 engine_quiesced=1`, `drain_iters=0`, both boots), so the drain
timeout the gate exists to catch did not occur. It is unreachable in every environment
available to a session and was not produced by the rung passing either.

So the gate is proven where it can be: the three `const fn`s between the
`R7-TEARDOWN-GATE-BEGIN` / `-END` markers in `gen7.rs` are **extracted verbatim** by a host
harness (`awk` between the markers, `include!`d, compiled with `rustc`) and driven with
`all_ring_idle` forced false. The harness also carries the pre-fix predicate as its red
baseline, and models the teardown's control flow exactly — the GGTT-unmap block runs only when
the gate passes, so `ptes_restored` cannot become true if the unmap was skipped.

Three legs, all passing on this arc:

| Leg | What it drives | Result |
| --- | --- | --- |
| 1 | forced drain timeout, everything else green | **pre-fix**: `ggtt_unmapped=true pages_freed=true` (RED). **post-fix**: both `false`. |
| 2 | exhaustive over the other six witnesses, `all_ring_idle` pinned false (64 combinations) | pre-fix reached `dealloc` in **3**; post-fix in **0**, and the unmap in **0**. |
| 3 | the un-armed path, plus all 64 idle-path combinations | still reclaims; **0 divergences** from the pre-fix gate whenever the engine is idle. |

Leg 3 is the one that keeps the fix honest: it narrows the gate **only** where the engine is
not provably idle, and changes nothing on the path this part actually takes.

Because the extraction is by marker and the harness asserts it read a non-trivial region, the
thing proven is the shipped source, not a paraphrase of it.

---

## 4. Where the ladder stands

The GT **fabric** is alive: `GTFIFOCTL` is structured and moves, and the GGTT PTE round-trip
is proven on metal at R4/R4b. The **engine register block** refused every write up to R5 —
R2's `INSTPM` did not latch, R5's `RING_CTL` did not latch — and R6 was the rung that decided
whether that refusal was the absence of a held wake or a property of the part.

**It was the absence of a held wake.** On flight 4, 2026-08-28, both boots, R6 kept R3's
acquire across the whole arm and the identical `RING_CTL` write latched
(`r6-sentinel-hit by=mt … attempts=1/3`), and R7 then moved the same envelope to the **BCS**
with a real `XY_SRC_COPY_BLT` and copied 1 KiB of pixels
(`r7-blit-verified … best_dst_match=256/256`, `dst_crc == src_crc = D7994E3D`). Enable-void →
sentinel-hit → blit-verified is **deterministic on this part**, not marginal: same candidate
(`mt`, `class=BDW-ONLY`), same `attempts=1/3`, 7 396 and 7 112 cycles against a 50 M allowance.
All six R7 constants pinned in §2.7 were exercised on silicon and none was contradicted.

What the ladder hands its successor is engineering, not physics, and R7 says it in its own
words on the wire:
`r7 next=DONE-the-BCS-copies-pixels-under-a-held-wake-wire-bring_up_blt_ring-to-the-held-wake-and-fix-blitter_copy_rect-DW0-client-field`
(boot 1, L1566).

**All three queued decisions are taken, 2026-09-15 (GEN7NEXT).**

- **GEN7R8 — flown 2026-09-16, refused on its own ceiling, ceiling fixed (R8CAP, §2.8).** The rung is `fb_blit`, behind `UNAOS_IVB3D_R8`, and it
  is R7's command at the panel's own geometry: a 64×64×32bpp rectangle at the **top-right corner**
  of a **framebuffer-pitch**, **multi-page** GGTT surface, with a third witness — `spill`, the bytes
  outside the rectangle — that R7's single-page destination could not carry. It blits into
  CPU-readable scratch and **not** into the scanout, because G4 proves the Kepler owns the panel;
  the visible copy is the rung after the gmux switch persists, and the reasoning is written down in
  §2.8 rather than assumed. It does not gate on a forcewake ack decode, as §4/R3 requires.
- **GEN7TLB — decided: HOLD (§2.6).** `reclaim=freed` is struck from the falsifier. An idle engine
  is a statement about the command streamer, not about a system-agent TLB entry, so it may not stand
  in for an invalidation witness; the bounded retention is documented-accepted and now prints as
  `reclaim=held`, distinct from the safety refusal `reclaim=leaked`.
- **GEN7R2 — decided: unscorable, and it says so (§2.2).** The behavioural witness the row asked for
  needs a proven-unowned GGTT window and a held wake, neither of which exists at R2's call site —
  and the experiment has already been run where they do, by R6 and R7. R2 prints
  `r2-unscorable-until-behavioural-witness` with the old `gt-still-dark` reading kept verbatim in
  `battery=`.

**What is owed is a boot.** Nothing above is a fact about silicon until the rMBP flies it; each
verdict string and its falsifier are stated here first, which is the only order in which a flight can
settle anything.
