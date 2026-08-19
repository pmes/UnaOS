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

### 1b. SOCK-4 `cleared=false` / `kernel=false` — **instrumented; variant 2 mechanism IDENTIFIED**

The transferable-socket fixture has flaked **five times**, in two variants.

| # | When / where | Variant | Note |
| --- | --- | --- | --- |
| 1 | run report, no in-tree record | `cleared=false` | uncorroborated |
| 2 | 2026-08-19, executor gate at base `bcf56b68`, sibling-QEMU load, `serial.log:1029` | `cleared=false` | `killed=0 done=2`, clean on re-run |
| 3 | 2026-08-19, `UNAOS_WIFIVAL=1` gate at `26517e30`, sibling-QEMU load, `serial.log:1052` | `cleared=true kernel=false` | clean on an immediate re-run under the identical config |
| 4 | 2026-08-19, tip-battery run at `f4bd5a73`'s integration, `serial.log:1053` | `cleared=true kernel=false` | clean on re-run |
| 5 | the instrumenting arc's own first gate run, `exec/rmbp-s4` base `f4bd5a73` (host clock 2026-08-18), `serial.log:1054` | `cleared=true kernel=false` | **caught by the new breakdown line on its first flight** — see below |

Variant 2 (`cleared=true kernel=false`) is the majority: 3 of the 5 sightings,
and now all of the last three. It matters because it falsifies this entry's
original "`kernel=false` carries no independent information" note: that note
holds only when `cleared` is *false* (the `&&` short-circuits the check away).
With `cleared=true` the check really ran and really returned false.

**Signature on the wire:**

```
:: SOCK-4: transferable sockets FAIL — grantor=… grantee=… used=… snap=… cleared=… kernel=false killed=0 done=2 (want …/…/1/true/true/true/0/2) ::
:: SOCK-4 BREAKDOWN: all_clear grantor_row=… grantee_row=… xfer_grantor=… xfer_grantee=… recs_free=… | kernel_check step=… ::
```

The tell for either variant is **`killed=0` with `done=2`**: both fixtures ran to
their witness exits and nothing was fault-killed, yet a proof came back false.

**The recurrence ask is now ARMED IN-TREE.** The `BREAKDOWN` line above prints
on the FAIL path only (the PASS line and the FAIL line are both byte-identical
to before — this is an instrument, not new chatter) and answers, without a
one-off run, the two questions this entry used to have to ask a future
investigator for:

- the five `all_clear` terms individually — `grantor_row` / `grantee_row` /
  `xfer_grantor` / `xfer_grantee` / `recs_free`, all sampled at the same instant
  the launcher computes `cleared`;
- `kernel_check step=<tag>` — the FIRST term inside `sock4_kernel_check` that
  read false. The early-return acquisitions are named individually
  (`smolnet_init`, `proc_reserve_b`, `stack_open`, `proc_reserve_c`,
  `stack_open_reuse`); the `ok &=` chain latches a tag at its first failure
  (`a_resolves`, `xfer_a_to_b`, `recv_b`, `b_resolves_moved`, `a_stale_eacces`,
  `xfer_a_to_c`, `recv_c`, `c_dead_steal_fence`, `b_undisturbed`,
  `gen_advanced`, `b_stale_vs_freed`, `slot_reused`, `b_stale_vs_new_tenant`,
  `handle_rows_clear`, `xfer_rows_clear`, `ledgers_free`). `step=not-run` means
  `cleared` was false and the check never ran — i.e. variant 1.

**Root cause, variant 2 (`cleared=true kernel=false`) — IDENTIFIED at sighting 5.**
The false term is **`slot_reused`**: step 5 of `sock4_kernel_check` closes the
socket, reopens, and asserts `sid2 == sid` to stage the gen-rebind fence on a
*first-fit-reused* slot. That assertion is not a kernel invariant — it is an
assumption about the state of the **global** `smolnet` `reg` table. `stack_open`
takes `reg.iter().position(|s| s.is_none())`, so the reopen returns `sid` only
if no slot *below* `sid` is free at that moment. Any other socket in the kernel
closing inside that window frees a lower slot and the reopen first-fits into it
instead. Sighting 5's log shows exactly that neighbour: the SMOLNET DNS
resolver leg — which does `stack_open` / bind / sendto / recvfrom /
`stack_close` on the same `reg` table — printed its completion at
`serial.log:1053`, between SOCK-4's banner (1051) and its verdict (1054). That
is why the flake is load-dependent (host load moves the DNS leg's completion
into or out of the window) and why re-runs are clean. Nothing in the capability
path is wrong: the fence is simply not staged, and the check reports that as a
failure indistinguishable from a real one.

**Root cause, variant 1 (`cleared=false`) — SUSPECT, unchanged.** The launcher's
`all_clear` predicate (both handle rows clear, both inbox rows clear, the
transfer-record ledger fully free) is a ground-truth re-read of state that the
two fixtures' synchronous exits retire. It is **partially** mitigated relative to
pre-fix DMG-REFUSE — a bounded *poll*, not a single read — but the bound is
**2000 ms** with no park/release handshake fencing the observation. Under a lost
quantum that bound is a margin, not a guarantee. Still not reproduced under
instrumentation; the `BREAKDOWN` line will name the term when it recurs.

**Which early-return acquisition could plausibly fail — reasoning, not measurement.**
Read-only review of the three (`smolnet::init`, `proc_reserve` ×2,
`stack_open`) says none is a strong candidate for a load-induced transient, and
that this reasoning is now superseded for variant 2 by the measurement above.
`smolnet::init` is idempotent and has already succeeded by the time SOCK-4 runs
(SOCK-2/3/5 printed their PASS lines above it), so a false there would mean the
NIC went away mid-boot. `proc_reserve` runs after the launcher has `proc_free`d
its own planted entry and after every demo fixture has exited — the PULSE-W line
immediately above the SOCK-4 banner reports the Proc census back at `10/10`
free — so slot exhaustion would need ~ten concurrent live processes that this
boot does not have. `stack_open` is the one with a genuinely shared, contended
resource (the `NSOCK` `reg` table, which the DNS leg and the SOCK-6/7 persistent
listeners also draw from), so *if* an early return ever fires it is the one to
suspect — but exhaustion returns `None`, whereas the observed contention on that
same table manifests as the far cheaper `slot_reused` mismatch. Treat
`smolnet_init` / `proc_reserve_*` on the wire as evidence of a **real** defect,
not of this class.

**What to capture on recurrence.**

- Both lines — the FAIL line and the `BREAKDOWN` line beneath it.
- `killed` must be `0` (non-zero = a fixture was fault-killed = a real SOCK-4
  bug, **not** the class) and `done` must be `2` (lower = a fixture never
  reached its witness exit = a different failure).
- `step=slot_reused` with all five `all_clear` terms `true` = the identified
  variant-2 mechanism above; check the surrounding lines for a neighbouring
  socket close (the `:: SMOLNET: [dns] … ::` line is the known one).
- Any `step` value **other** than `slot_reused` or `not-run` is new information —
  it has never been observed, and the capability-path tags
  (`b_resolves_moved`, `a_stale_eacces`, `c_dead_steal_fence`,
  `b_stale_vs_*`) would each be a genuine defect, not a flake.
- Whether a re-run at the same sha on an idle host is clean, and the host load
  at the time of the failure.

**Disposition — WATCH, instrumented.** Variant 2's mechanism is identified but
**not fixed**: fixing it means changing `sock4_kernel_check`'s staging (making
the gen-rebind fence robust to a non-reusing reopen, or fencing the reopen
against concurrent `reg` traffic) — fixture logic, out of the instrumenting
arc's lane, and a decision an owner should take deliberately. Variant 1's fix
shape remains the one proven in-tree for 1a: a `SWEPT`-and-park handshake so the
launcher's re-read happens inside a window where the state is provably live,
instead of racing a 2000 ms deadline.

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
