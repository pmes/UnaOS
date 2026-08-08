# PREDICTION — SMPBAL-X86 (`CPU_AUTO` placement + `try_steal` + the CR3-generation fix)

Written **before** the metal boot. Branch `wt/smpbalance`. Bench: rMBP 2012, 8 logical
cores. Arm the boot with `UNAOS_WEDGE2=1` — the arc adds a *remote* run-queue lock class
that has never existed on this arch, and `<W1>`/`<W2>` are the only things that can
convict it.

The baseline this is predicted against is **Boot AL**, six vug instances open:

```
[schedx86] load c0=0% c1=100% c2=0% c3=0% c4=0% c5=0% c6=0% c7=6% ...
```

One core (c1, the **render** core) pinned at 100 %; five workers flat at 0 %. Cause:
`bg_place_cpu()` returned the caller's core, the caller is the shell, and since SCHED-X86
the shell *is* `x86_render_service`. Every launch landed on c1 and could never leave it.

---

## 0. Which cores are which

On the bench the published split is `render=c1 svc=c7` (`SCHED-X86 PLACE`). SMPBAL-X86
excludes **both** from `CPU_AUTO` placement and from stealing (the service exclusion is a
deadlock rule, not a preference — `x86_usb_pump` holds the raw `XHCI_CONTROLLER` spinlock
there). So the candidate set for a ring-3 launch is exactly:

> **`{c0, c2, c3, c4, c5, c6}` — six cores, for six vugs.**

Read every number below against that set. Confirm the split from the boot's own
`SCHED-X86 PLACE` line before scoring anything; if it publishes different cores, shift the
set accordingly rather than assuming c1/c7.

---

## 1. `SCHEDPLACE-X86` — the placement lines (new)

Six launches produce six lines of the form:

```
:: SCHEDPLACE-X86: 'bg-user' -> c<N> (q=<depth> load=<pct>% from c1) ::
```

(`from c1` is the *caller's* core — the shell — and is expected to read c1 on every one
of them. That token is not the result; `-> c<N>` is.)

**P1 (headline, numeric).** The six `-> c<N>` values are **six distinct cores, all drawn
from `{c0, c2, c3, c4, c5, c6}`**. Zero of them read `-> c1` or `-> c7`.

*Why distinctness is predicted and not merely hoped for.* The key chain is (1) shallowest
ready queue, (2) lowest ~250 ms busy percent, (3) a rotating cursor. A running vug is in
`current`, not in the queue, so key (1) reads 0 for a core that already hosts one — key
(1) alone would NOT spread them. Key (2) separates them for hand-spaced launches (the
window has caught up). Key (3) separates them for script-fast launches: `AUTO_ROTATE`
advances one per call and the scan takes the first best-keyed core from a rotating start,
so consecutive fully-tied calls land on consecutive candidates. Both regimes spread; the
*mechanism* differs, which is why a partial result is diagnostic (see R2).

**P1-weak (accept floor).** If launches are scripted tighter than the rotation can
separate, ≥ 4 distinct cores. Fewer than 4 distinct is a refute.

---

## 2. `[schedx86] load` — the shape of the fix

Steady state, six vugs open, ~5 s cadence:

```
[schedx86] load c0=..% c1=..% c2=..% c3=..% c4=..% c5=..% c6=..% c7=..% sw=[..] q=[..] steal=M/P asgen=G/R
```

**P2 (headline, the falsifiable shape).** **No core reads ≥ 90 % while two or more cores
in `{c0, c2, c3, c4, c5, c6}` read exactly 0 %.** That single sentence is the arc's
headline claim and the one Peter's "core load not balancing" names.

**P3.** `c1` (render) drops from **100 % to below 50 %**. It keeps real load — it still
owns the panel, the shell and the compositor flush — so a low-but-nonzero c1 is correct
and 0 % would be suspicious, not better.

**P4.** **At least four** of `{c0, c2, c3, c4, c5, c6}` read non-zero on the same line.

**P5 (queues).** `q=[...]` reads **0 or 1 in every column**, with `max(q) ≤ 1` in steady
state. One vug per core means nothing queues behind a running task. A column at 2+ that
persists across consecutive lines means placement piled up and stealing did not drain it —
that is R3.

**P6 (`sw`).** The `sw=[...]` context-switch counters go from "one core climbing, the rest
flat" to **six columns climbing at comparable rates**. This is the independent
cross-check on P2: `sw` is an event count and the percent is a time measurement, so they
must agree about *which* cores are working while disagreeing about magnitude. If they
disagree about which, one of the two instruments is lying and the round stops there
(GR13/GR15 discipline).

---

## 3. `steal=M/P` — and why `M = 0` is a PASS

`M` = tasks migrated; `P` = idle passes that ran the steal attempt.

**P7.** `P` climbs steeply — **thousands within the first `load` line, tens of thousands
by the third**. QEMU already reads `steal=0/43935` on a 6-core box, so this is measured,
not guessed.

**P8.** `M` is expected to be **0, or a small number that goes flat**. Say this plainly
because the bench will otherwise read a zero as a dead instrument: with six vugs placed on
six distinct cores, *no queue ever reaches `STEAL_MIN_DEPTH = 2`*, so there is nothing to
steal. **Placement alone is predicted to solve the reported symptom; stealing is the
correction for when it does not.** `steal=0/<large>` means "placement got it right".

**P8b (how to make it fire, if the round wants the steal path exercised).** Launch **more
vugs than candidate cores** — 8 or more. Then `M` climbs to roughly `(launches − 6)` and
goes flat, and each migration prints
`:: [smpbal] steal 'bg-user' cA->cB ::` (first 24 only).

**Anti-witness: `P = 0` is the dead reading.** `try_steal` is called from `run()`'s
empty-queue arm on every idle pass of every non-render, non-service core. If `P` is 0 the
mechanism never executed and every conclusion in §3 is void.

---

## 4. `asgen=G/R` — the CR3-generation fix's only surface

`G` = live address-space generation (bumped by every user page-table leaf mutation: slot
build, teardown/recycle, ELF permission pass, every window map/unmap). `R` = dispatches
that had to re-validate CR3 against it.

This is here because the fix is otherwise **invisible**: its failure mode is a silent
cross-tenant stale-TLB read (stale W bits against a new ELF's W^X layout, plus reach into
another program's window-surface pages), which no witness can catch after the fact. The
only falsifiable property is whether the mechanism *fires*.

**P9.** Both terms are non-zero and both **climb monotonically** with launch/window
activity. QEMU measures `asgen=415/216` over a full fixture battery.

**P10.** `R` climbs when `G` does, and `R` is of the same order as `G` (each bump costs at
most one reload per dispatching core, and most cores are idle between bumps). `R ≫ G × 8`
would mean something is bumping the generation on a hot path.

**Refute R8:** `G` climbing while `R` stays at 0 — the dispatch site is not consulting the
generation and the fix is dead code. Equally: `G` stuck at 1 while vugs launch and open
windows — nothing is bumping, and the mutation sites were missed.

---

## 5. Anti-witnesses — what must NOT change

| must still read | why it is the right falsifier |
| --- | --- |
| `SCHED-X86 PLACE-CHECK: ... verdict=PASS` | render/input/usb-pump named explicit cores, so `steal_ok == false` and they are pinned. It was given **no exemption**, deliberately — a `PLACE-CHECK` taught to tolerate migration cannot falsify what it exists for. |
| `WXAUDIT-CORES: n=8 ... wp=0xFF nxe=0xFF` | per-core hardware facts with no task dimension. Untouched. |
| `[schedx86] load-prejoin c0=--` (with ≥1 other core carrying a percent) | `mark_online(0)` happens *inside* `run()`, after the prejoin emit. The SCHEDLOAD-X86 anti-witness is preserved exactly. |
| no `<W1>` / `<W2>` on a `UNAOS_WEDGE2=1` boot | the new remote run-queue acquisitions (peek + steal) both go through `wedge4::lock_or_squawk`. |
| no new `RING-3 FAULT`, no `<TRUNCATED>` / `<CAPPED>` on the load line | line bound re-derived: 637 bytes worst case against `LINEBUF_CAP = 768`. |

---

## 6. Refutes — any one of these means the arc did not do what it claims

* **R1** — a `SCHEDPLACE-X86` line names the render core or the service core while ≥ 2
  other cores are dispatching. The exclusion ladder is broken. *Security-adjacent:* a
  ring-3 program on the service core is the `xhci_worker_cpu` deadlock, not a slowdown.
* **R2** — two or more vugs placed on the same core while another candidate core reads
  `q=0` and `0%` on the following load line. Both tie-breaks failed; the key chain is not
  reading load.
* **R3** — the load line still shows one core ≥ 90 % while ≥ 3 candidate cores read
  exactly 0 %. **This is the headline refute:** the reported symptom is unchanged.
* **R4** — `steal=M/0`. `try_steal` never ran.
* **R5** — `M` climbing at *dispatch* rate rather than *idle-pass* rate (compare against
  the `sw` columns on the same line). That is churn, not balance — aarch64's own lesson,
  paid for over three arcs.
* **R6** — `PLACE-CHECK verdict=FAIL`, or `WXAUDIT-CORES n < 8`. Placement or stealing
  disturbed a pinned service.
* **R7** — any `<W1>` / `<W2>` token on a `UNAOS_WEDGE2=1` boot. The remote run-queue
  acquisition wedged.
* **R8** — `asgen=G/0` with `G` climbing, or `G` stuck at 1 across launches (§4).
* **R9** — the machine's ms-clock freezes (`BPACE`/timestamps stall) after a launch. A
  cooperative ring-3 task reached core 0 despite two independent exclusions.
* **R10** — repeated launch → exit → relaunch cycles produce a program reading another
  program's window pixels, or a ring-3 write succeeding to a page the new ELF marked RX.
  That is the exact defect the CR3-generation fix exists to prevent, and it is the only
  reason that fix ships in this commit rather than a follow-up.

---

## 7. What QEMU already proved, and what it cannot

**Proved** (`UNAOS_WEDGE2=1 ./arroyo test`, 6-core box, 48 PASS / 0 FAIL — byte-identical
PASS/FAIL count to the same battery on the unmodified base):

* `:: SCHEDPLACE-X86: 'bg-user' -> c0 (q=0 load=0% from c3) ::` — a program placed off the
  render core (c1) and off the service core (c5) for the first time on this arch.
* `PLACE-CHECK verdict=PASS`, `load-prejoin c0=--`, no `<W1>`/`<W2>`.
* `steal=0/43935` — the steal path executes tens of thousands of times and correctly
  declines (nothing eligible is ever queued behind a running task).
* `asgen=415/216` — the CR3-generation mechanism fires.

**Cannot prove:** an actual migration (`M > 0` needs a queue at depth ≥ 2 with an eligible
task, which the fixture battery never creates); the 8-core exclusion arithmetic of §0; and
anything about the real panel, since QEMU's timing does not reproduce the render core's
frame budget. **Every number in §1–§4 is a metal claim.**
