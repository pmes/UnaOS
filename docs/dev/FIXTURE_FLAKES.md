# FIXTURE_FLAKES.md — the fixture-flake diagnosis corpus

A gate went red. You have one minute to decide whether you are looking at a
**regression** or at a **known flake class**, and the wrong answer costs either a
day of investigation or a shipped defect.

This file is the corpus that answers that question. Each entry gives, in order:

1. **Signature on the wire** — the exact witness text, so you can `awk` for it.
2. **Trigger conditions** — the load and timing regime that produces it.
3. **Root cause** — known, or honestly labelled as suspect.
4. **What to capture on recurrence** — the evidence that discriminates.
5. **Disposition** — fixed at a named sha, or on watch.

> Read serial logs with `awk '/pattern/' <log>`, **never plain `grep`** — control
> bytes in the logs break it. (`grep -a` also works.)

**The corpus is not a licence to re-run.** A flake that matches an entry is still
an observation: record the run, capture what the entry asks for, and only then
re-run. A class entry that never accumulates recurrence evidence is a class that
can never be closed.

**Scope.** Everything here is an x86 QEMU-gate observation from the `hw-rmbp`
lane. None of these classes has been seen on metal, and none is metal-specific:
they are all launcher/observer races or transport-margin effects that host load
makes visible.

---

## Class 1 — the ground-truth re-read races the fixture's teardown

**The shape.** A launcher spawns a ring-3 fixture, waits for the fixture's
witness (an exit status, a done-counter, a ledger word), and then re-reads
**kernel-side ground truth** — a window table, a handle row, a transfer-record
ledger — that the fixture's own exit is in the middle of retiring.

The trap is that the witness is published *before* the teardown completes.
`SYS_EXIT` publishes the status and *then* `sched::exit` runs the synchronous
release chain (`user_space_release` → `free_user_space_by_cr3` → `win_close_slot`
and friends). A launcher gated on the witness alone is therefore racing that
chain, and the race is decided by when the launcher's next `yield_now` round
comes back — microseconds on an idle host, **a whole quantum** under sibling-QEMU
load. That is why the class is a load-dependent flake rather than a constant
failure.

**The cure, and the tell for a correct fixture.** A witness word is a *point*
event; a ground-truth re-read needs an *interval*. The fixture must publish a
"swept, and still holding everything" flag and **park** (bounded) until the
launcher releases it, so the re-read happens strictly inside a window where the
fixture's state is provably live. If a launcher in this shape re-reads on a
done-counter with no park, it is a member of this class whether or not it has
flaked yet.

### 1a. DMG-REFUSE `NOT RUN` — **known, fixed**

**Signature on the wire** (pre-fix; a single message covered both conditions):

```
:: DMG-REFUSE: the window table moved under the probe (id_a=… id_free=… b0=… b1=… entry=0x01 recheck=0x01 owned=…) — refusal witness NOT RUN ::
```

The fingerprint that identifies the race rather than a real table change is
**`entry=recheck=0x01` with `b0`/`b1` still readable** — the prober's two window
rows are already gone from the table, but its slot backing has not been zeroed
yet, so its self-reported ids still read back fine.

The expected green line, for contrast:

```
:: DMG-REFUSE: SYS_WIN_PRESENT_ROWS(33) refused every malformed band — 19/19 probes from two ring-3 slots agree: … — witness OK ::
```

**Trigger conditions.** Roughly **2 runs in 5** under sibling-QEMU / heavy host
CPU load (the fix was validated against a 24-way load regime). Always clean on
re-run against an idle host. `x86-fat.spec` and `round6-rmbp.spec` both `FORBID
DMG-REFUSE:.*NOT RUN`, so the flake failed the spec rather than passing quietly —
which is the correct behaviour and the reason it got caught.

**Root cause — known.** The launcher gated its ground-truth re-read on `DMG_DONE`
alone. `DMG_DONE` is incremented by the `SYS_EXIT` arm **before** `sched::exit`'s
synchronous teardown retires the prober's two window rows, so under load the
launcher read the table after the rows had gone and reported "the table moved"
with nothing actually wrong. The same read also touched the prober's param block
after its slot could have been freed — a latent use-after-free the fix closes as
a by-product.

The old message text is itself part of the lesson: it asserted *table movement*,
which sent the first investigation at a bystander-ordering hypothesis that was
never the mechanism. A `NOT RUN` message must name the condition it actually
measured, not the cause it guesses at.

**Fix.** The prober publishes `SWEPT` at param-block `+0x28` after its 19 probes
and **parks** (bounded: 3000 × 20 ms ≈ 60 s) on the launcher's release word at
`+0x30`; the launcher takes the re-read strictly inside that park, releases on
every exit path so the prober can never be left holding a slot, and splits the
one message into three honest verdicts:

| Line | Means |
| --- | --- |
| `:: DMG-REFUSE: the prober's own window rows do not match the table (… probe_owned=…) — refusal witness NOT RUN ::` | ring 3's report disagrees with the kernel-side table |
| `:: DMG-REFUSE: the window table moved under the probe (… owned=…) — refusal witness NOT RUN ::` | the ground truth the expectations were built from really did shift |
| `:: DMG-REFUSE FAIL — the prober never published SWEPT within 10000ms (…) ::` | the prober never reached its park — a fixture failure, graded **FAIL**, not `NOT RUN` |

**What to capture if it recurs anyway.** The three lines above are now
discriminating, so start by recording *which one* printed. Then:

- the full field list from the line (`id_a`, `id_free`, `b0`, `b1`, `entry`,
  `recheck`, and `probe_owned` / `owned`);
- whether `entry` and `recheck` agree — if they differ, the table genuinely moved
  and this is **not** the class;
- the host load at the time (`uptime` / concurrent QEMU count), and whether a
  re-run on an idle host is clean;
- the preceding WINX-7 verdict and the desktop-app launch timing: the
  `DMG_REFUSE_SETTLED` flag exists because a desktop-app launch that beat the
  witness to window row 1 once produced a legitimate non-empty table at entry,
  which prints
  `:: DMG-REFUSE: the window table was not empty at entry (occupied=…) — refusal witness NOT RUN ::`
  — a different condition with the same `NOT RUN` verdict.

**Disposition — FIXED at `8fe65e1b`** (`fixtures: DMG-REFUSE — the ground-truth
re-read stops racing the prober's teardown`). Fixture-only, x86
`arch/x86_64/syscall.rs`; no spec change (tokens unchanged). A recurrence of the
`entry=recheck` fingerprint after this sha is a **new** defect, not this one.

### 1b. SOCK-4 `cleared=false` — **watch**

The transferable-socket fixture has flaked **three times**. The first
observation was a run report with no in-tree record; the second (2026-08-19,
an executor gate run at base `bcf56b68` under sibling-QEMU load:
`serial.log:1029`, `killed=0 done=2`, clean on re-run) corroborates it. The
third (2026-08-19, a `UNAOS_WIFIVAL=1` gate run at `26517e30` under
sibling-QEMU load, `serial.log:1052`) is a **distinct variant**:
`cleared=true kernel=false` — the teardown proof PASSED and
`sock4_kernel_check()` itself returned false. An immediate re-run under the
identical configuration passed clean, refuting a code-deterministic cause for
that boot's delta (the wifi replay leg). The variant matters because it
falsifies this entry's original "kernel=false carries no independent
information" note for that case: with `cleared=true`, the false term is inside
the kernel check's own resource acquisitions (`smolnet::init`,
`proc_reserve`, `stack_open` — each returns false/None on transient
exhaustion) or its `ok &=` chain, and the line does not say which. The flake
is observed-recurring with two distinct failing terms.

**Signature on the wire:**

```
:: SOCK-4: transferable sockets FAIL — grantor=… grantee=… used=… snap=… cleared=false kernel=false killed=0 done=2 (want …/…/1/true/true/true/0/2) ::
```

The tell is **`cleared=false` with `killed=0` and `done=2`**: both fixtures ran to
their witness exits and nothing was fault-killed, yet the launcher's teardown
proof came back false. `kernel=false` follows mechanically (`kernel_ok` is
`cleared && sock4_kernel_check()`), so it carries no independent information —
do not read it as a second failure.

**Trigger conditions.** Host load, same as 1a. Observed twice, both under
sibling-QEMU load; clean on re-run both times.

**Root cause — SUSPECT, not established.** The SOCK-4 launcher's `all_clear`
predicate (both handle rows clear, both inbox rows clear, the transfer-record
ledger fully free) is exactly a ground-truth re-read of state that the two
fixtures' synchronous exits retire. It is **partially** mitigated relative to
pre-fix DMG-REFUSE — it is a bounded *poll* rather than a single read, so it
tolerates ordinary teardown latency — but the bound is **2000 ms** and there is no
park/release handshake fencing the observation. Under a lost quantum that bound
is a margin, not a guarantee. This is a plausible mechanism, not a confirmed one:
nobody has reproduced it under load or instrumented which of the four `all_clear`
terms was false.

**What to capture on recurrence.**

- The full FAIL line, and specifically whether `killed` is `0` (if non-zero, a
  fixture was fault-killed and this is a real SOCK-4 bug, **not** the class).
- `done` — it must read `2`. A lower value means a fixture never reached its
  witness exit; that is a different failure.
- **Which `all_clear` term was false.** The line does not break it out, so this
  needs a one-off instrumented run splitting `handle_row_is_clear(grantor)` /
  `(grantee)` / `xfer_row_is_clear` ×2 / `xfer_recs_all_free()`. That single
  datum decides between "teardown still in flight" (the class) and "a record
  genuinely leaked" (a real defect).
- **For the `cleared=true kernel=false` variant: which term inside
  `sock4_kernel_check` failed.** The check returns one bool over ~a dozen
  acquisitions and assertions; the recurrence capture needs a per-term
  breakdown (the early-return resource acquisitions first — `smolnet::init`,
  `proc_reserve` ×2, `stack_open`) before the variant can be classified as
  transient exhaustion vs a real capability-path defect.
- Whether a re-run at the same sha on an idle host is clean, and the host load
  at the time of the failure.

**Disposition — WATCH.** One observation, mechanism unconfirmed. If it recurs,
the fix shape is known and already proven in-tree: give the fixtures a
`SWEPT`-and-park handshake in the DMG-REFUSE idiom so the launcher's re-read
happens inside a window where the state is provably live, instead of racing a
2000 ms deadline.

---

## Class 2 — the evidence taps lose lines to a margin-tight serial ring

### 2a. SERWIT-2 `evidence_lost=17` — **suspect only**

**Signature on the wire:**

```
:: SERWIT-2: FAIL — balanced=true evidence_lost=17 ::
```

immediately preceded by the four per-tap lines the verdict always prints first:

```
:: SERWIT-2 tap fbcon: submitted=… absorbed=… staged=… dropped=… suppressed=… torn=… inflight=… in_progress=… ::
:: SERWIT-2 tap ftdi: …
:: SERWIT-2 tap tste: …
:: SERWIT-2 tap flightrec: …
```

and, if the loss was not already announced, one or more:

```
[mirror] <tap>: N line(s) dropped, M truncated since boot (sink contended or full)
```

The green line, for contrast:

```
:: SERWIT-2: mirror taps — every line accounted for on all 4 taps, 0 lost on the 3 evidence taps (ftdi/tste/flightrec) -> PASS ::
```

**Read the verdict correctly — the two halves are different failures.**
`balanced` is the conservation law (submitted vs absorbed + dropped +
suppressed + in-flight, with a stated 64-line sampling window that is the core
ceiling, not a tolerance). `evidence_lost` is the **sum of `dropped` across the
three evidence taps only** — `ftdi`, `tste`, `flightrec`; `fbcon` is excluded on
purpose, because the panel is a view and its misses are reported without being
fatal. `balanced=true` with `evidence_lost=17` therefore says: *the accounting is
honest and 17 lines were genuinely, knowingly lost off evidence sinks.* There is
no tolerance on `evidence_lost` — the threshold is zero — which is why 17 is a
FAIL and not a warning.

Note also that `dropped` and `torn` are **different outcomes**: a tap charges
`torn` when a line was staged but did not fit its slot width (it is sealed with
`…⟨SERWIT-2W: line truncated here⟩` and still reaches the sink), and charges
`dropped` only on the exhaustion path — staging ring **full at depth**, *and* the
free retry at the sink also failed. `evidence_lost` counts only the latter.

**Trigger conditions.** Seen **1 run in 12**. Load-correlated in the same way as
class 1: the drop path is reachable only while the sink lock is contended *and*
the 64-slot staging ring is already full, which needs several cores printing at
once.

**Root cause — SUSPECT, explicitly not established.** No root-cause pass has been
done; nobody has reproduced it under controlled load or identified which tap
carried the 17.

The suspect named when this corpus was opened is **per-line growth on the
rollup lines** — concretely, the `stalls=` field, about **9 bytes per rollup** —
against a serial ring whose margin is real and finite. The arithmetic that
motivates the suspicion is in-tree and verified:

- the primary staging ring is `SLOTS = 64` × `SLOT_LEN = 1536` bytes; the
  measured worst-case line in the whole tree is 1291 chars + newline = **1292
  bytes**, leaving **244 bytes (19%)** of headroom, deliberately sized so the
  truncation counter reads exactly 0 and any non-zero reading is news;
- the FTDI mirror's own staging ring is **narrower**: 64 slots × **240 bytes**.

So the ring is 1536 B of *margin*, not slack, and the mirror's margin is
tighter still. Two ways growth could bite, and they are distinguishable on the
wire: extra bytes per line lengthen the sink-lock hold, which deepens staging and
makes the depth-exhaustion `dropped` path reachable (→ `dropped` climbs); or
extra bytes push a line past a slot width (→ `torn` climbs instead). **Only the
first would produce `evidence_lost`.** That asymmetry is the cheapest available
discriminator and it has not yet been checked against a real capture.

**What to capture on recurrence.** In priority order:

1. **The four `:: SERWIT-2 tap …:` lines.** They are printed immediately before
   the verdict and they are the whole diagnosis: they name which tap lost the
   lines and separate `dropped` from `torn`, `suppressed`, and `inflight`.
   Without them the FAIL line is a number with no referent.
2. Every `[mirror] …` line in the capture, with its position in the boot — they
   say *when* the loss burst happened, and the verdict's own summary cannot.
3. Whether `torn` is non-zero anywhere. Non-zero `torn` shifts weight toward the
   width/margin half of the suspect; `torn=0` with non-zero `dropped` points at
   depth exhaustion under contention and largely exonerates line width.
4. The SERWIT-1 verdict from the same boot, in full — it carries the primary
   wire's own backpressure margin for that run, which is the direct measurement
   of the pressure this suspect is about. On the PASS line the figures read
   `N back-pressured on a full ring (deepest X of Y turns)`; on the FAIL line
   they are the literal `stalls=N maxspin=M/…` fields.
5. Host load and core count, and whether a re-run on an idle host is clean.
6. If it is reproducible under load, the discriminating experiment: re-run with
   the rollup lines shortened and see whether `evidence_lost` follows. That is
   the check that would confirm or kill the suspect, and it has not been run.

**Disposition — WATCH, suspect unconfirmed.** SERWIT-2 is not asserted by any
`.spec` file (`x86-fat.spec`, `round6-rmbp.spec`, `x86-witness.spec`,
`rmbp-boot.spec` carry no SERWIT token), so this failure does **not** turn a gate
red on its own — it is caught by reading the log, and by
`tools/serial-analyzer.py`, which does carry a `SERWIT-2` witness family. Treat a
sighting as evidence to bank rather than as a gate failure to clear, and do not
let a green spec run bury it.

---

## Adding an entry

An entry earns its place when a failure has been seen **more than once**, or once
with a mechanism worth writing down. Give it the five headings above, quote the
witness text **exactly** as the kernel formats it (copy it out of the source, not
out of memory), and state plainly whether the root cause is known or suspect. An
entry that overstates its confidence is worse than no entry: the whole purpose is
that a cold reader can trust the label and spend their minute accordingly.
