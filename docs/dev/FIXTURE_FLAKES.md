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

**Scope.** Classes 1-3 are x86 QEMU-gate observations from the `hw-rmbp` lane;
Class 4 is an aarch64 `virt` one from `hw-jetson` (added orin 23, 2026-09-08 —
the sentence that used to stand here said "everything here is x86", and adding an
aarch64 class without correcting it would have left a doc that reads false to the
next cold reader). **Classes 5-7 were added after that sentence and are named
here for exactly the reason it was corrected**: Class 5 is `hw-pi4`
(`kernel8-test`, raspi4b QEMU) with an x86 control of its own; Classes 6 and 7
are x86 `hw-rmbp` again.

**"None of these classes has been seen on metal" ALSO used to stand here, and it
is no longer true — corrected 2026-09-22 (FLAKEFIX) rather than left to read
false.** §1d has THREE metal sightings, on the rMBP's own flights 8 and 11
(`chop-logs/flight8.log` twice, `f11.log` once; all three are metal captures —
`uart16550=absent carrier=ftdi-mirror`, panel 2880x1800, SMC `OSK0` present).
That is not a wrinkle, it is corroboration: §1d's mechanism is a race against a
concurrent `compose`, and five real cores race it harder than TCG does. Class 7
is the one class that is emulator-only BY CONSTRUCTION — it is QEMU's own event
folding, and there is no such layer on metal. The rest remain unseen on metal and
none is metal-specific: they are launcher/observer races and transport-margin
effects, and load — host load under QEMU, real concurrency on metal — is what
makes them visible.

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

### 1b. SOCK-4 `cleared=false` / `kernel=false` — **variant 2 FIXED; variant 1 on watch**

The transferable-socket fixture has flaked **five times**, in two variants.
**Variant 2 (`cleared=true kernel=false`, `step=slot_reused`) is FIXED — see the
disposition at the end of this entry.** The live watch is variant 1
(`cleared=false`, 2 sightings, still teardown-race-SUSPECT), for which the
`all_clear` half of the `BREAKDOWN` line is now the armed instrument.

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
  `gen_advanced`, `b_stale_vs_freed`, `new_tenant_live`, `stale_gen_vs_tenant`,
  `b_stale_vs_new_tenant`, `handle_rows_clear`, `xfer_rows_clear`,
  `ledgers_free`). `step=not-run` means `cleared` was false and the check never
  ran — i.e. variant 1.

  **Tag change at the variant-2 fix.** `slot_reused` is GONE — the step it named
  (assert the reopen first-fit back onto the freed slot) no longer exists. It is
  replaced by the two tags of the reshaped step 5: `new_tenant_live` (the fresh
  socket's slot, packed at its CURRENT generation, resolves under its owner — the
  positive control) and `stale_gen_vs_tenant` (the SAME slot and SAME owner at the
  PRECEDING generation is `-EACCES` — the fence proper). A `step=slot_reused` on
  the wire from a build at or after the fix sha would mean a stale binary.

**Root cause, variant 2 (`cleared=true kernel=false`) — IDENTIFIED at sighting 5, FIXED.**
The false term was **`slot_reused`**: step 5 of `sock4_kernel_check` closes the
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

**Fix (variant 2).** The gen-rebind fence no longer needs the reopen to land on
any particular slot. Step 5 now stages the fence on **whatever slot the reopen
returned** (`sid2`), using that slot's own generation arithmetic:

| | before | after |
| --- | --- | --- |
| staging | `sid2 == sid` — the reopen first-fit back onto the freed slot (tag `slot_reused`) | *nothing* — no assertion about which slot the reopen returns |
| live-tenant control | implicit (the reused slot was known live) | `socket_id_of(A, (gen2, sid2)) == Ok(sid2)` — the tenant resolves under its OWNER at its CURRENT generation (tag `new_tenant_live`) |
| the fence proper | B's stale `(sid, gen0)` handle is `-EACCES` while `sid` holds a new tenant (tag `b_stale_vs_new_tenant`) | `socket_id_of(A, (gen2 - 1, sid2)) == Err(EACCES)` — SAME slot, SAME owner, SAME rights, one generation behind the live tenant (tag `stale_gen_vs_tenant`) — **plus** the unchanged `b_stale_vs_new_tenant` check, which holds whichever slot the reopen took |

The property is **not weakened — it is sharpened**. The old
`b_stale_vs_new_tenant` was over-determined: B's handle differs from the live
tenant in *both* generation and owner, so a rejection there did not isolate the
generation check. The new `stale_gen_vs_tenant` handle differs from the passing
control in exactly one field — the packed generation — so only `sock_valid`'s
generation comparison can reject it, and `new_tenant_live` rules out the boring
alternative that the slot was dead all along. `gen2 - 1` is a real historical
generation: a slot can only reach this open by having been closed, and
`stack_close` bumps the generation; in the degenerate never-closed case it wraps
to a generation the slot has never held, which is still not the live tenant's.

**No flake window remains.** Nothing in step 5 now reads, or asserts anything
about, the state of the global `reg` table beyond the slot the kernel itself
just handed this check. The DNS leg and the SOCK-6/7 listeners can open and
close freely inside the window with no effect on any term.

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
same table manifested as the far cheaper `slot_reused` mismatch (now fixed
away). Treat
`smolnet_init` / `proc_reserve_*` on the wire as evidence of a **real** defect,
not of this class.

**What to capture on recurrence.**

- Both lines — the FAIL line and the `BREAKDOWN` line beneath it.
- `killed` must be `0` (non-zero = a fixture was fault-killed = a real SOCK-4
  bug, **not** the class) and `done` must be `2` (lower = a fixture never
  reached its witness exit = a different failure).
- `step=not-run` = variant 1, the remaining watch: `cleared` was false and the
  check never ran, so read the five `all_clear` terms and treat the false one as
  the teardown-race candidate.
- **Any `step` value other than `not-run` is now new information.** The one
  observed non-`not-run` tag, `slot_reused`, no longer exists in the tree; every
  surviving tag names either a capability-path property (`b_resolves_moved`,
  `a_stale_eacces`, `c_dead_steal_fence`, `new_tenant_live`,
  `stale_gen_vs_tenant`, `b_stale_vs_*`) or a ledger-hygiene property, and each
  would be a genuine defect, not a flake.
- Whether a re-run at the same sha on an idle host is clean, and the host load
  at the time of the failure.

**Disposition — variant 2 FIXED at `5f243e19`; watch reduced to variant 1.**

- **Variant 2 (`cleared=true kernel=false`, `step=slot_reused`) — FIXED.**
  Fixture-only, x86 `arch/x86_64/syscall.rs` (`sock4_kernel_check` step 5); no
  smolnet/table change, no spec change (the PASS and FAIL line texts are
  byte-identical, tokens unchanged). Evidence: **three consecutive**
  `UNAOS_FATIMG=sf ./arroyo test 150` runs at the fix sha, each `SOCK-4 … ->
  PASS` and each `MBENCH PASS — 30/30`, the third under a deliberate 12-way host
  load; the `:: SMOLNET: [dns] … ::` neighbour printed inside the window in all
  three. The flake fired ~3 runs in 5 under load before the fix. A
  `step=slot_reused` after this sha means a stale binary, not a recurrence.
- **Variant 1 (`cleared=false`) — WATCH, cleared=false, 2 sightings (#1, #2),
  still teardown-race-SUSPECT.** Unreproduced under instrumentation. The
  `BREAKDOWN` line's `all_clear` half is armed for it and will name which of
  `grantor_row` / `grantee_row` / `xfer_grantor` / `xfer_grantee` / `recs_free`
  read false. The fix shape, if it recurs, remains the one proven in-tree for
  1a: a `SWEPT`-and-park handshake so the launcher's re-read happens inside a
  window where the state is provably live, instead of racing a 2000 ms deadline.

### 1c. `[dmgovlp]` `adopt_stretch=0/4` — **RE-CLASSED AND FIXED 2026-09-22 (DMGFLAKE, branch `exec-rmbp-dmgflake`, parent `6fab6b0d` — the tip sha at the fold; rmbp-ledger B158). NOT a wall-clock window, and NOT host starvation: the fixture was scoring composite passes that `COMP_GATE` DECLINED.** Rate before the fix: 6 in 47 boots (2026-09-22), the worst on the bench.

**Signature on the wire** (the verdict's own format, `video/wm.rs:24379`):

```
[dmgovlp] verdict passes=12/12 drained=12/12 drag_evt=… drag_px=… relay=… narrow=…/12 cur=12/12 adopt=… repaint=… max_ms=… adopt_stretch=0/4 -> FAIL
```

The fingerprint that identifies the flake rather than a real CURSTICK
regression is **every other counter green** — `passes=12/12`, `drained=12/12`,
`cur=12/12` — with only the stretch tally at zero. `x86-wc.spec` REQUIREs
`adopt_stretch >= 1`, and the doc at `wm.rs:24387` calls RED "structurally
0/4"; the one sighting shows host contention can produce structural-looking
0/4 with the fixture otherwise healthy.

**Seen:** once (2026-08-27, TOPPORT re-gate, 1 of 3 `UNAOS_WC=1` runs) on a
host running a nine-agent fleet. Patch ruled out as cause: the failing run's
diff adds shell-verb code no spec types, and both a reverse-applied base run
and a patched re-run on the same host pass `adopt_stretch=4/4`. Artifacts:
`~/unaos-bench/scratch/rmbp7/topport-regate.out` (the FAIL),
`topport-baseline-r1.out`, `topport-patched-r3.out`.

**Suspected mechanism (FALSIFIED 2026-09-22 — kept because it is what the
next reader would guess too):** the stretch passes assert inside a wall-clock
window (the CURSTICK stretch — present+drain must move `CUR3_TAKEN` within the
pass), and a QEMU starved by sibling load misses all four windows without any
kernel defect. Same shape as 1a's deadline race, one layer up.

**Why it is false, and the tell was on every capture the whole time:
`max_ms=0`.** Starvation makes a pass take LONGER. The failing captures read
`max_ms=0..3` against greens at `max_ms=2..25` — the failing runs are the FAST
ones, because nothing happened in them at all. See the real mechanism below.

**What to record on the next sighting:** the full verdict line; host load and
sibling QEMU count at the time; whether an idle-host re-run is clean; and
`max_ms=` from the failing line vs. a green one — if the failing run's
`max_ms` is an outlier, the starvation reading is confirmed without a new
experiment.

**Seen, second time:** 2026-09-12, LOGIN M4 re-gate (orin-0912b), 1 of 3
`UNAOS_WC=1` runs, host load 2.41 with 2 sibling QEMUs on 20 cores and six
executors building. The failing verdict:

```
[dmgovlp] verdict passes=12/12 drained=12/12 cur=12/12 drag_evt=0 drag_px=0 adopt=0 max_ms=0 adopt_stretch=0/4 -> FAIL
```

**This sighting was read as confirming starvation; it does the opposite, and
the correction is DMGFLAKE's (2026-09-22) rather than a rewrite of the
sighting.** `max_ms=0` beside greens at `max_ms=3` is not "an outlier" in the
direction starvation predicts — a starved pass is SLOWER, not instant. The
failing run is instant because its twelve passes were declined and never ran.
`drag_evt=0` is the same fact stated once more. The banked observation stands;
only its verdict is corrected. The original text follows. The
two green re-runs on the same tree carry `drag_evt=5 drag_px=38590 adopt=25
max_ms=3`; the failing run carries `drag_evt=0 drag_px=0 adopt=0 max_ms=0`.
`max_ms` is the outlier §1c asked for — and `drag_evt=0` says more than
starvation of the stretch windows alone: the synthetic drag got **no pass at
all**, so nothing could have adopted. Every other counter is green because
those tallies do not depend on a pass landing inside a window. An idle-host
re-run was clean twice.

A third sighting the same day, same cause, different arc: SDWRITE2's gate 4
(`UNAOS_QEMU_FULL=1 UNAOS_WC=1 ./arroyo test`) reported the same
`adopt_stretch=0/4 max_ms=0` line on its first attempt and `adopt_stretch=4/4
max_ms=2` when re-run alone. Six executors were on the box.

**Still WATCH, and now with a shape worth fixing:** a fixture whose verdict
cannot distinguish "the stimulus never ran" from "the stimulus ran and the
assert failed" will keep costing a re-run to read. The cheap fix is for the
drag leg to REFUSE (a distinct verdict word, not FAIL) when `drag_evt=0` —
it did not measure what it claims to measure. Owner: the next x86 compositor
arc.

**Two more sightings, 2026-09-22 — this is no longer "seen once".** GMUX8's
240 s battery at host load 23 (`gmux8-logs/test240.log:1944` —
`drag_evt=0 drag_px=0 relay=0 narrow=0/12 cur=11/12 adopt=0 repaint=1 max_ms=2
adopt_stretch=0/4 -> FAIL`) and FLAKEFIX's first `test-ptr` run at load 13
(`flakefix-logs/repro1.log`, `serial.log:2022`, the same fields with
`cur=10/12 adopt=0 repaint=0 max_ms=0`). Both `drag_evt=0`, i.e. both are the
"the stimulus never ran" half — exactly the reading the REFUSE verdict asked for
above would have made free. Rate: **2 reds in the 6 x86 battery runs read by hand that
day, and 6 reds in 47 boots (about 13%) across every QEMU boot on the bench
that day** — the worst rate of the three flakes measured, and one whole gate run
each time. TRACKPAD's two `test-ptr` captures from the same
box the same day both read
`drag_evt=5 drag_px=38590 relay=3 adopt_stretch=4/4 -> PASS`, so the fixture is
not broken — it is unmeasured under load.

### 1c — THE REAL MECHANISM, measured (DMGFLAKE, 2026-09-22)

**`composite()` on x86 is gated, and a declined pass is INDISTINGUISHABLE from
a drained one.** `wm::composite`'s x86 arm compare-exchanges [`COMP_GATE`]; a
second entrant takes the DECLINE arm, whose own comment states the property
exactly: *"This pass composites nothing and — crucially — CLEARS NOTHING"*. It
stores `COMP_PENDING`, ticks `WCSER_DECLINED`, calls `cursor::owe_repaint()`
and returns — in microseconds, having painted nothing and left every `damaged`
flag standing.

`dmgovlp_selftest` scored that as success, twice over:

* every counter it reads is a **delta over work that never happened**, so
  `drag_evt`, `drag_px`, `relay`, `narrow`, `adopt` and `adopt_stretch` all
  read zero; and
* its **drain check** — "one extra composite must add NOTHING" — is satisfied
  by the compositor doing nothing, so `drained=12/12` was scored BY the
  decline. Twelve green drains over twelve passes that never ran.

**The signature is bimodal, and that is the proof it is not a clock.** The
fixture's whole battery completes in microseconds when it is being declined,
so it fits inside ONE sibling composite pass (`[comp2] pass_us=4265` mean under
TCG, 170 ms tail). Either the gate is free when the battery starts and all
twelve passes run, or it is held and ALL of them fold. There is no middle: on
the bench every green reads `drag_evt=5 drag_px=38590 … adopt_stretch=4/4` and
every flake reads `drag_evt=0 drag_px=0 relay=0 narrow=0/12 … adopt_stretch=0/4`.
A per-pass wall-clock lottery would produce 1/4, 2/4 and 3/4. None exists.

**The discriminating field, free on every capture:** the `[wcser]` rollup.
Every `adopt_stretch=0/4` flake on the bench carries a `scope=fixture` rollup
with `declined_pct=23..31` (`gmux8-logs/test240.log` `entered=107 declined=48
declined_pct=30`; `flakefix-logs/repro1.log` `entered=202 declined=61
declined_pct=23`; `ptrinstall3-logs/serial-smp2-wc.log` `entered=165
declined=75 declined_pct=31`). Greens carry `declined_pct=0` or no
fixture-scope declines at all. `repaint=1` on the GMUX8 line is
`owe_repaint()`'s own fingerprint — the decline path leaking into the verdict.

**Fix — in the fixture's shape, not the compositor's.** The compositor is
correct: declining is what the gate is FOR. What was wrong is a witness that
could not tell "the pass ran and the assert failed" from "the pass never ran".
`dmgovlp_selftest` now routes every `composite()` through a bounded
`composite_live` helper which asks `WCSER_DECLINED` whether the call actually
took the gate, waits the holder out (250 ms budget — 1.5x the worst honest TCG
pass tail, 4x under `DMGOVLP_WEDGE_MS`, so it can never mask the boot-8 wedge)
and retries; the declined pass CLEARS NOTHING, so the damage it did not service
is still on the table and the next real pass carries it. Per LAWS §5 the wall
clock is **REPORTED and never gated** — a new `[dmgovlp] serialisation
folds=N fold_ms=N unran=N budget_ms=N` line — and the fold wait is kept out of
`max_ms` so a busy sibling can never forge a WEDGE. A drain now counts only if
its composite RAN, and the per-pass stretch line names the miss:
`why=adopted | missed:pass-declined-by-COMP_GATE | missed:unoffered |
missed:offered-but-no-carry`. Only the last convicts CURSTICK.

**The spec pin is unchanged in meaning and in grammar.** `x86-wc.spec` still
REQUIREs `adopt_stretch >= 1` via the same contiguous
`adopt_stretch=\d+/4 -> PASS` regex; the new fields ride their own line (no
`SKIP`, no `-> FAIL`, so they trip none of that file's FORBIDs), the same
argument DMGOVLP2's `stretch UNOFFERED` line makes. Proven by replay: the
three green gate runs below passed the spec unchanged.

**Evidence (all at host load 17–44 on 20 cores, sibling QEMUs live).**

| Run | Wire | Verdict |
| --- | --- | --- |
| three consecutive greens, loads 16.98 / 25.37 / 19.60 | `folds=0 fold_ms=0 unran=0` | `adopt_stretch=4/4 -> PASS`, `TEST_RC=0` x3 |
| **forced fold** (scratch probe holds `COMP_GATE` 80 ms across the battery) | `folds=2 fold_ms=110 unran=0`, `why=adopted` x4 | `adopt_stretch=4/4 -> PASS` — the fixture waits the holder out |
| **deliberate re-introduction** (same probe, the decline-blind drain restored) | `folds=13`, `pre=(true,0,0) dd=0 db=0`, `why=missed:pass-declined-by-COMP_GATE` x4 | `passes=12/12 drained=12/12 drag_evt=0 drag_px=0 relay=0 narrow=0/12 cur=12/12 adopt=0 repaint=0 max_ms=3 adopt_stretch=0/4 -> FAIL` |

The re-introduction line is the bench's §1c signature **character for character
apart from `max_ms`**, which is the confirmation that the forced probe and the
bench flake are the same event.

**Disposition — FIXED** (fixture-only, `video/wm.rs` `dmgovlp_selftest`; no
compositor change, no spec change, knob-off byte-identical on both arches).
The standing rule still holds for anything that reds here AFTER this sha: an
`adopt_stretch=0/4` with `why=missed:offered-but-no-carry` is a **real CURSTICK
regression** and must not be re-run away; one with
`why=missed:pass-declined-by-COMP_GATE` after this fix means the 250 ms budget
was exhausted, which is a new and reportable fact — read `unran=` and the
`[wcser]` `declined_pct=` beside it.

### 1d. DOCKID `order=false set=false` — **measured 2026-09-22 (FLAKEFIX, rmbp-ledger B150): NOT lost input. MECHANISM CORRECTED 2026-09-22 (FLAKEFIX2, rmbp-ledger B159) FROM THE WIRE: the stale snapshot is the WRITER'S, not the reader's, and the metal half is CLASS 6. BOTH HALVES FIXED 2026-09-22 (DOCKID2, rmbp-ledger B165) — see the SUPERSEDED disposition at the end of this entry; the writer scans at mutation time and the fixture SKIPs a composite that never reconciled; and the RESIDUAL DOCKID2 named is CLOSED 2026-09-22 (DOCKSTAMP, rmbp-ledger B175) — the window table carries an allocation stamp and `reconcile` admits in allocation order, so the rank is monotone in arrival BY CONSTRUCTION. THE ENTRY IS CLOSED; what is owed is the flight-12 reading.**

**Signature on the wire** (`video/dock.rs` `dockid_selftest`, the verdict's own
format):

```
:: DOCKID: tiles=5 closed=win2 reopened=win2 recycle=true order=false set=false furniture=false count=true/5 pins=true/2 press=yes :: FAIL ::
```

The green line from the same binary, same box, an hour apart:

```
:: DOCKID: tiles=5 closed=win2 reopened=win2 recycle=true order=true set=true furniture=true count=true/5 pins=true/2 press=yes :: PASS ::
```

**Read WHICH legs failed — that is the whole diagnosis, and it is already
printed.** The fixture has six legs and they split cleanly by what they read:

| leg | reads | 2026-09-22 |
| --- | --- | --- |
| `recycle` | `wm::create`'s returned slot id | `true` in 5/5 sightings |
| `order` | the SHARED tile registry (`TILE_ID`/`TILE_GEN`/rank) | **false** in 5/5 |
| `set` | the SHARED registry, both directions | **false** in 5/5 |
| `furniture` | `order_key` re-read over the already-sorted model | false in **1** of 5 |
| `count` / `pins` | LOCAL scratch only (`probe`, the pin chain) | `true` in 5/5 |

**The two legs that read only local scratch pass in every sighting; every leg
that reads the shared registry fails.** That is not an input-loss shape — no
injected event reaches this fixture at all — it is Class 1's shape exactly: a
ground-truth re-read racing a concurrent mutator.

**MECHANISM CORRECTED 2026-09-22 (FLAKEFIX2) — READ THIS BEFORE THE PARAGRAPH
BELOW IT, WHICH IS KEPT BECAUSE ITS END STATE IS RIGHT AND ITS WINDOW IS WRONG.**
Everything this entry says about the LEGS and about the END STATE survives: the
split by what each leg reads is exact, and `every app tile falls back to
`RANK_UNSEEN + id`, the strip returns to WINDOW-ID order` is precisely what the
captures show. What is wrong is WHOSE snapshot is stale. It is not the fixture
re-reading a registry that moved under it; **it is `reconcile` itself ranking
against a model it scanned BEFORE it mutated.** Four lines of one capture settle
it, and they are `[dock]` lines this corpus already asked for on recurrence:

```text
x86bind-logs/gored-b3-serial.log
1219: [wm]   alloc win=3 gen=4 owner=0xd1d3 title="idC"
1224: [dock] tile add win=1 gen=8 owner=0xd1d1 seq=15 label=idA
1225: [dock] tile add win=2 gen=5 owner=0xd1d2 seq=16 label=idB
1227: [dock] census tiles=4 win:gen=1:8,2:5,console:pin,shell:pin      <-- idC is LIVE and has NO TILE
1230: [wm]   alloc win=2 gen=6 owner=0xd1d4 title="idD"
1236: [dock] tile add win=2 gen=6 owner=0xd1d4 seq=17 label=idD        <-- the YOUNGER window ranks 17
1238: [dock] tile add win=3 gen=4 owner=0xd1d3 seq=18 label=idC        <-- the ELDER window ranks 18
1242: [dock] census tiles=6 win:gen=1:8,2:6,3:4,4:1,console:pin,shell:pin
1298: :: DOCKID: … order=false set=false furniture=true … :: FAIL ::
```

The same six windows on a GREEN boot, for the contrast that makes it conclusive:

```text
logs/foldgate/g2-test-x86-wc.log
2019: [wm]   alloc win=3 gen=4 owner=0xd1d3 title="idC"
2022: [dock] tile add win=3 gen=4 owner=0xd1d3 seq=17 label=idC        <-- elder ranks 17
2029: [wm]   alloc win=2 gen=6 owner=0xd1d4 title="idD"
2032: [dock] tile add win=2 gen=6 owner=0xd1d4 seq=18 label=idD        <-- younger ranks 18
2103: :: DOCKID: … order=true set=true furniture=true … :: PASS ::
```

**`idC` was allocated ELEVEN LINES BEFORE `idD` and was given the LATER arrival
rank.** `compose`'s `dock_scan` ran before `1219`, its `reconcile` ran after, so
that pass never saw `idC` and did not admit it; the next pass admitted `idD` and
`idC` together and walked the model in window-table order, where `idD`'s recycled
id 2 precedes `idC`'s id 3. `order_ok` asserts `a < c && c < d`, and `c < d` is
exactly what this reverses. **A tile's rank is therefore not its window's arrival
— it is the arrival of the first reconcile pass that happened to see it**, which
is the very thing `NEXT_SEQ`'s header says it exists to prevent.

**This is why a reader-side snapshot cannot fix it, and that has to be said
plainly because a generation/seqlock is the obvious cure and it is the wrong one
here.** At `1242` the census is complete, internally consistent and stable: the
registry is not TORN under the reader, it is stably and permanently WRONG. A
consistent snapshot hands the reader an unimpeachable view of an answer that was
already decided incorrectly one pass earlier. The tear is real and is worth
closing on its own (it is what `furniture=false`, 1 of 5, looks like), but it is
the smaller half and it is not what `order=false set=false` is.

**THE METAL HALF IS A DIFFERENT CLASS AGAIN — CLASS 6, not Class 1.** On the three
rMBP sightings the registry was not stale-by-one-pass, it was never written at all:

```text
gmux8-logs/f11.log   (identical in hdaamp-/gmux7-/camera1-/vugperf-logs/f11.log)
3257: [wm] alloc win=4 gen=8 owner=0xd1d1 title="idA"    … through …
3275: [wm] alloc win=8 gen=1 owner=0xd1d5 title="idF"
      <-- NOT ONE `[dock] tile add` OR `[dock] census` LINE BETWEEN 3256 AND 3298
3298: :: DOCKID: tiles=7 … order=false set=false furniture=true … :: FAIL ::
```

`dock::compose` reconciles UNCONDITIONALLY (it is the first statement of the
settle at `video/dock.rs:789`, ahead of the panel snapshot's early-out), and it
prints on every admit — so an absence of `tile add` over the whole fixture is not
a quiet reconcile, it is NO reconcile. The fixture's own `wm::composite()`
DECLINED to reach the dock (five real cores, another core holding the composite),
and `strip_model` then scored a registry that no pass had ever written. That is
Class 6's shape word for word — *the fixture drives a step that may DECLINE, then
scores the value that step was to publish* — and its cure is 6a's, not Class 1's:
give the fixture the declined step's own reading (a reconcile counter it can read
across its `wm::composite()`) and let it REFUSE rather than score. Scored as a
FAIL it convicts the kernel of a defect the capture does not show.

**Root cause — the registry's one writer is `compose`, and the fixture does not
exclude the render service's copy of it.** `strip_model` calls `wm::composite()`
(which reconciles the registry) and then `dock_scan` + the pin chain + `settle`,
and the fixture then re-reads `TILE_ID`/`TILE_GEN`/`order_key` for its three
assertions. Nothing parks the render core in between. Under load its own
`compose` lands inside that gap, and the two failure shapes follow from where it
lands:

- **between the reconcile and the reads** → the fixture's tiles are not the
  registry's any more, every app tile falls back to `RANK_UNSEEN + id`, the strip
  returns to WINDOW-ID order, and because the reopened window carries the closed
  one's recycled id it sorts BEFORE its elder sibling → `order=false`, and the
  registry/table cross-check → `set=false`. This is the majority shape
  (`furniture=true`, 4 of 5, and all three metal sightings) and it is the one `dockid_selftest`'s own doc
  comment already predicts in its "a dead fold empties the registry" paragraph —
  the same end state reached by load instead of by a commented-out `settle`.
- **between `settle`'s sort and the `furniture` walk** → `order_key` returns a
  different key for a row than the one it was sorted by, monotonicity breaks and
  `furniture=false` joins them (1 of 5, QEMU only so far).

**Trigger conditions — and this one IS on metal, which is why it is the most
convincing of the day's three.** On QEMU: **2 FAIL in 46 boots (about 4%)**
across every boot on the bench on 2026-09-22, de-duplicated by each boot's own
SERWIT-2 tap lines; FLAKEFIX's own five `test-ptr` runs at host load 9-41 were
all `PASS`, and the reds are load-correlated the way the rest of this corpus is.
On the rMBP's own metal, three further sightings, all
`order=false set=false furniture=true`:

| capture | when | verdict |
| --- | --- | --- |
| `f11.log` (flight 11) | `43391ms` | `tiles=7 closed=win5 reopened=win5 recycle=true order=false set=false furniture=true count=true/7 pins=true/2` |
| `chop-logs/flight8.log` | `36903ms` | `tiles=6 … order=false set=false furniture=true count=true/6 pins=true/1` |
| `chop-logs/flight8.log` | `38664ms` | the same line again, 1.8 s later in the same boot |

Both are metal captures (`uart16550=absent carrier=ftdi-mirror`, panel
2880x1800, SMC `OSK0` present), so this class is **not** a TCG artefact and the
scope note at the top of this file was corrected for it. Five real cores race a
`compose` harder than a loaded TCG host does, and the same two legs fail, with
the same `count`/`pins` passing beside them.

**It is NOT the typist class (7).** The lost-input mechanism below is loss
*outside* the guest — QEMU folding motion events the guest never polled — and
this fixture consumes no injected input: it mints its own six windows and reads
kernel state. The two classes share only "host load made it visible". Proved by
elimination as well as by mechanism: FLAKEFIX's `gored` run injected a
deliberately lossy 36-report stream that the guest counted as **8**, and DOCKID
in that same capture (`gored-serial.log:1740`) reads `order=true set=true
furniture=true :: PASS ::`.

**What to capture on recurrence.** The verdict line in full — the six legs ARE
the diagnosis. Then: whether `count`/`pins` also went false (they read local
scratch, so a false there is a DIFFERENT defect and not this class); whether
`recycle` held; and the host load, or on metal what else was compositing. A sighting with `order=false` and
`count=false` together is a regression, not this entry.

**Disposition — WATCH, fix STILL NOT MADE, and the cure is no longer the one this
entry used to name.** ⚠ **SUPERSEDED 2026-09-22 by DOCKID2 (B165) — BOTH PIECES ARE
NOW MADE; read the DISPOSITION SUPERSEDED block below before acting on this
paragraph, which is kept because it is the diagnosis that was executed.** FLAKEFIX2 held the file and did not write the change, which
is a decision and is recorded as one: the cure the brief carried (a reader-side
generation/seqlock) is refuted above by the capture, and the cure the wire asks
for is a WRITER-SHAPE change — `reconcile` must take its model at the moment it
mutates rather than be handed one scanned before, so an arrival cannot be re-ranked
behind a window created after it. That is a change to where a rank comes from, not
to how a reader reads one, and it touches the single-writer rule this block is
built on; it was not made unilaterally on a brief that specified the other fix.
The metal half additionally reclassifies to **Class 6** and its cure is 6a's
declined-step reading, above. Two separable pieces of work, then, and they should
be priced separately:

1. **The rank's provenance (Class 1, the operator-visible half).** `reconcile`
   ranks against its caller's snapshot. Cure: the writer scans at mutation time.
   Go-red must be DELIBERATE — at 2 FAIL in 46 boots the natural rate shows nothing
   in three runs — which means a widener that mints a window inside the
   scan→reconcile gap (a `witness`-gated hook in `settle`, driven by
   `dockid_selftest`). Without it neither a green nor a red means anything.
2. **The declined composite (Class 6, the metal half).** `strip_model` must read
   whether the `wm::composite()` it drove actually reconciled, and SKIP when it did
   not. Small, deterministic, and it removes all three metal sightings from the
   FAIL column without widening one assertion.

**Still not** a retry loop and **still not** a widened assertion: the legs are
correct about what they assert. What changed is that they are not asserting it
about a snapshot that moved — they are asserting it about a rank set that was
written wrong, or never written at all.

**DISPOSITION SUPERSEDED — BOTH PIECES MADE, 2026-09-22 (DOCKID2, rmbp-ledger
B165; branch `exec-rmbp-dockid2` off `9d190c4c`, the branch tip is the sha — the
seat fills it at the fold).** The two paragraphs above are kept because their
diagnosis is what was executed, verbatim, and the go-red each one asked for was
built and is quoted here.

**(1) THE RANK'S PROVENANCE — FIXED.** `reconcile` takes no model at all now: it
calls `wm::dock_scan` itself, immediately before it mutates, so the rows it ranks
are the rows as they stand AT ADMISSION. The single-writer rule did not move
(`compose`'s `settle(.., true)` is still the only reconciling caller) and neither
did LOCKFIX's rule for the router (`press_at` reaches `settle` with
`reconciling=false`). The cost that DID move is one extra `wm::dock_scan` per
composite pass — a second acquire of the window table on a path that already takes
it once, the same lock in the same masked context, no new wait CLASS.

*The widener is not the one suggested above and is better than it.* Rather than
mint a window inside the gap, the probe HIDES the newest non-furniture row from
`compose`'s own scan while leaving it in the window table — which is the same real
event (a window that exists but arrived after the model this pass is carrying was
taken), is deterministic, and above all is the SAME PROBE ON BOTH TREES, so the A/B
is one variable. Scratch, in `dock.rs`, reverted before the commit.

```text
UNFIXED + probe  (host load 9.52, `./arroyo test-ptr 150`, TEST_RC=1)
  r1-gored-serial.log
  1635: [wm]   alloc win=3 gen=4 owner=0xd1d3 title="idC"
  1638: [dock] tile add win=2 gen=5 owner=0xd1d2 seq=9  label=idB   <-- idC NOT admitted
  1639: [dock] census tiles=5 win:gen=quarry:pin,1:8,2:5,…          <-- idC live and TILELESS
  1645: [wm]   alloc win=2 gen=6 owner=0xd1d4 title="idD"
  1648: [dock] tile add win=2 gen=6 owner=0xd1d4 seq=10 label=idD   <-- the YOUNGER window ranks
  :: DOCKID: tiles=6 … recycle=true order=false set=false furniture=true … reconciled=3/3 folds=0 :: FAIL ::

FIXED + THE SAME PROBE  (host load 21.22, TEST_RC=0)
  r2-fixed-probe1-serial.log
  1714: [wm]   alloc win=3 gen=4 owner=0xd1d3 title="idC"
  1717: [dock] tile add win=3 gen=4 owner=0xd1d3 seq=17 label=idC   <-- elder ranks 17
  1724: [wm]   alloc win=2 gen=6 owner=0xd1d4 title="idD"
  1727: [dock] tile add win=2 gen=6 owner=0xd1d4 seq=18 label=idD   <-- younger ranks 18
  :: DOCKID: tiles=6 … order=true set=true furniture=true … reconciled=3/3 folds=0 :: PASS ::
```

`17`/`18` are `g2-test-x86-wc.log:2022/2032`'s green ranks character for character,
reached with the widener still armed. **Note what `reconciled=3/3` does in the RED
line**: it says the reconcile RAN, so that capture is defect (1) and provably not
the Class 6 decline — the new field separates the two defects on the wire, which no
capture in this entry's history could do.

**THE RESIDUAL, NAMED.** Two windows allocated between one reconcile and the next
are still admitted in ONE pass, and that pass walks them in window-table (id)
order, so a recycled low id can still take the lower of the two ranks. Closing that
needs an allocation stamp the window table does not carry — `wm::DockEntry` has
`id`, `owner_asid`, `title`, `title_len`, `visible`, `focused` and nothing monotone
— so it is a `video/wm.rs` change and was not made here. It is not the measured
defect: in `gored-b3-serial.log` the two windows are eleven lines and one whole
reconcile apart, and the fix collapses the miss window from a whole composite pass
to the few instructions between `dock_scan` returning and the admit loop reading it.

**(2) THE DECLINED COMPOSITE — FIXED, AND ALL THREE METAL SIGHTINGS ARE CLEARED.**
`strip_model` drives `composite_reconciled()` instead of a bare `wm::composite()`:
it asks the dock's own reconcile counter whether the pass reached `dock::compose`,
waits the holder out bounded (250 ms) and re-drives, and the verdict gains
`reconciled=<ran>/<drives> folds=<n>`. `ran < drives` is `:: SKIP ::` with its own
`reconciled=false` witness line — never a PASS (nothing was measured), never a FAIL
(a FAIL there convicts the kernel of a defect the capture does not show, which is
exactly what flights 8 and 11 did). DMGFLAKE's `composite_live` (B158) is the same
shape one layer up; it is a closure local to `wm::dmgovlp_selftest` and nothing
exports it, so this is the pattern reused and not the code. The witness is
deliberately the dock's reconcile counter and not `WCSER_DECLINED`: the latter
answers "did `COMP_GATE` turn a pass away", the former answers the question the
fixture actually has, and is also false when a pass is refused above the gate
(PANELREFUSE Tier 1).

```text
DECLINE PROBE + the decline-AWARE fixture  (host load 11.19, TEST_RC=1)
  r6-probe2-serial.log — ZERO `[dock] tile add`/`census` lines in the whole capture,
  f11's metal shape reached deterministically instead of by five racing cores
  :: DOCKID: declined drives=3 ran=0 folds=765 budget_ms=250 reconciled=false — … -> SKIP ::
  :: DOCKID: tiles=6 … order=false set=false furniture=true … reconciled=0/3 folds=765 :: SKIP ::

THE SAME PROBE + the decline-BLIND fixture restored  (host load 8.70, TEST_RC=1)
  r7-reintro-serial.log
  :: DOCKID: tiles=6 … recycle=true order=false set=false furniture=true … :: FAIL ::
  f11.log:3298  :: DOCKID: tiles=7 … recycle=true order=false set=false furniture=true … :: FAIL ::
```

The six legs read identically across the re-introduction and the metal sighting,
which is what makes the probe and the flake the same event.

**THE SPEC HOLE THIS ARC ALSO FOUND, and it is the larger finding of the two.**
`:: DOCKID:` was pinned by **no directive in any spec** — B159's and B150's reds,
and all three metal sightings, were convicted by `mbench`'s builtin
DEFAULT_FORBIDS seeing a bare `:: FAIL ::` and nothing else. So the fixture could
have stopped running entirely and no gate would have said a word. `x86-wc.spec`
now carries the pin, in pi4-regression.spec:2032/2052's REQUIRE-or-skip +
FORBID-the-skip shape: a REQUIRE that accepts PASS or SKIP (so absence reds, and
the honest `fixture — table full` arms stay legal) and two FORBIDs closing the
declined-composite arm on the lane where a reconcile must run. Replays quoted in
B165: green 15/15 required / 0 forbidden; the declined capture 15/15 required /
**2 forbidden hit** — both spellings fire.

**Disposition — CLOSED for both pieces, with the residual above named and owned by
`video/wm.rs`.** THREE consecutive greens under load on the fixed tree, no probe
(host 27.92 / 30.47 / 31.30 on 20 cores, sibling QEMUs live), all
`order=true set=true furniture=true reconciled=3/3 folds=0`, `TEST_RC=0` ×3. What
is still OWED is the METAL reading: flight 12 must print `reconciled=` on the
DOCKID line, and that field is the prediction — `reconciled=3/3` with a PASS, or
`reconciled=<r>/3` with a SKIP, but a FAIL carrying `reconciled=3/3` would mean the
metal half was never Class 6 and this entry is wrong about it. `folds=` on metal is
the first measurement anyone will have of how hard five real cores hold `COMP_GATE`
against a boot-task fixture.

**RESIDUAL CLOSED 2026-09-22 (DOCKSTAMP, rmbp-ledger B175; branch `exec-rmbp-dockstamp` off
`fe385712`, the branch tip is the sha — the seat fills it at the fold). THE ENTRY'S LAST OPEN
CLAUSE IS THE ONE THE BLOCK ABOVE NAMED IN ITS OWN WORDS**, and it is quoted here rather than
paraphrased because the fix is its answer word for word: *"two windows allocated between one
reconcile and the next are still admitted in ONE pass, and that pass walks them in window-table
(id) order, so a recycled low id can still take the lower of the two ranks. Closing that needs an
allocation stamp the window table does not carry."*

**THE STAMP.** `video/wm.rs` now mints a globally monotone ordinal for every window AT THE CLAIM
SITE — `create_inner`'s `t.rows[slot] = row` line, inside the table guard and beside
`winid_slot_bump`'s generation — and `wm::DockEntry` carries it through `dock_scan`. Under the
guard rather than after it for the reason this whole entry is about: a stamp taken after the
publish would order the DISCOVERY of rows, which is exactly the defect DOCKID2 fixed one layer up.
It is GLOBAL and not per-slot, because the question is "which of these two windows is older" and
two windows in different slots have no common ordering in `SLOT_GEN`; `0` means UNSTAMPED and is
reachable only from `DockEntry::empty`'s scratch.

**AND THE ADMIT ARM WALKS IT.** `dock::reconcile`'s admit loop sorts an index permutation by
`(stamp, id)` instead of consuming the scan in its own (id) order, so `NEXT_SEQ` hands ranks out in
ARRIVAL order by construction. The tie-break is `id`, so the degenerate all-unstamped input
degrades to exactly the old behaviour rather than to an arbitrary one. **What this removes is the
last dependence of tile order on TIMING**: DOCKID2 shrank the miss window from a whole composite
pass to a few instructions; this makes it not matter how many windows a pass admits at once.

*The probe is the one above, WIDENED FROM ONE HIDDEN ROW TO TWO, and it is again the SAME PROBE ON
BOTH TREES.* It withholds from every reconcile's scan both of the rows the fixture allocates in
REVERSE TABLE ORDER — `idC` takes the HIGH slot 3 FIRST, `idD` then recycles the freed LOW slot 2 —
and releases them together, so ONE pass admits both. That is the residual's shape, and it is the
only shape that can tell table order from allocation order. Scratch in `dock.rs`, reverted before
the commit; the script that applies it is in the evidence directory.

```text
PARENT + probe  (host load 12.89, `UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_SMC=1
                 UNAOS_QEMU_FULL=1 ./arroyo test 240`, TEST_RC=1)
  r1-parent-probe-serial.log
  1599: [wm] alloc win=3 gen=4 owner=0xd1d3 title="idC" from=declared     <-- allocated FIRST
  1611: [wm] alloc win=2 gen=6 owner=0xd1d4 title="idD" from=declared     <-- allocated SECOND
  1615: [dock] tile add win=2 gen=6 owner=0xd1d4 seq=18 label=idD         <-- the YOUNGER ranks 18
  1616: [dock] tile add win=3 gen=4 owner=0xd1d3 seq=19 label=idC         <-- the ELDER ranks 19
  :: DOCKID: tiles=6 … recycle=true order=false set=true furniture=true … reconciled=3/3 folds=0 :: FAIL ::

FIXED + THE SAME PROBE  (host load 28.62, TEST_RC=0)
  r2-fix-probe-serial.log
  1601: [wm] alloc win=3 gen=4 owner=0xd1d3 title="idC" from=declared
  1613: [wm] alloc win=2 gen=6 owner=0xd1d4 title="idD" from=declared
  1617: [dock] tile add win=3 gen=4 owner=0xd1d3 seq=18 label=idC         <-- FIRST-allocated ranks 18
  1618: [dock] tile add win=2 gen=6 owner=0xd1d4 seq=19 label=idD
  :: DOCKID: tiles=6 … order=true set=true furniture=true … reconciled=3/3 folds=0 :: PASS ::
```

Both captures admit the two rows in ONE pass — the `[dock] tile add` lines are adjacent and there
is no `census` between them — which is what makes this the residual and not the defect DOCKID2
already fixed. **`reconciled=3/3` on the RED line does the same work it did for DOCKID2**: it says
the reconcile RAN, so that capture is the ORDERING defect and provably not the Class 6 decline.

**THE ptr LANE IS PINNED NOW TOO, which was DOCKID2's other open clause** ("`x86-wc.spec` now
carries the pin" — and only that one did). `x86-ptr.spec` carries the same REQUIRE-or-skip and the
same two FORBID-the-skip lines. It is not a duplicate for tidiness: **this is the lane where the
fixture has actually gone red on this bench.** `logs/foldgate/g9r-test-ptr.log:2260`
(`serial.log:1702` in the capture's own numbering) is a real FAIL from gate 9's re-run —

```text
:: DOCKID: tiles=6 closed=win2 reopened=win2 recycle=true order=true set=false furniture=true
           count=true/6 pins=true/3 press=yes :: FAIL ::
```

— and the go-red is a REPLAY of the pin against that capture, not an argument about it. Parent
spec: `7/7 required witnesses, 2 forbidden hit(s)`, and the only rule that caught it is
`❌ FORBID* FAIL ::` — the `*` marks `mbench`'s BUILTIN default, i.e. **no directive in the file saw
it**, which is this block's whole thesis measured on the very lane it was missing from. With the
pin: `7/8 required witnesses, 4 forbidden hit(s)`,
`FIRST-SHORTFALL x86-ptr.spec:109 REQUIRE :: DOCKID: …` and `❌ FORBID :: DOCKID: .* :: FAIL :: —
2 hit(s), first @ line 2260`. Both replays are in the evidence directory.

**Disposition — §1d IS CLOSED, all three pieces, with nothing named and unowned.** Three
consecutive greens under load on the fixed tree with NO probe (host 14.80 / 13.27 / 7.87 on 20
cores, sibling QEMUs live), all `order=true set=true furniture=true reconciled=3/3 folds=0`,
`TEST_RC=0` ×3; and one green on the newly pinned typist lane
(`UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_QEMU_FULL=1 ./arroyo test-ptr 150`, host load
27.93, `MBENCH PASS — 8/8 required witnesses, 0 forbidden hit(s), 2871 lines scanned [full wall
152.1s]`). What is STILL OWED is unchanged and is the METAL reading: flight 12's prediction is
`order=true set=true … reconciled=3/3 folds=<n> :: PASS ::`. A flight-12 `order=false` carrying
`reconciled=3/3` would now mean a THIRD mechanism — both the ones this entry knows are closed — and
that is a sharper prediction than the paragraph above it could make.

**The natural rate of the residual is UNMEASURED, deliberately and by construction.** It needs two
allocations inside one reconcile window; the bench does not produce them on demand and no boot
count is claimed for it here. That is the same reason the probe exists, and the same reason the
paragraph above says a go-red had to be DELIBERATE.


---

## Class 2 — the evidence taps lose lines to a margin-tight serial ring

### 2a. SERWIT-2 `evidence_lost=N` — **ROOT CAUSE MEASURED 2026-09-22 (FLAKEFIX, rmbp-ledger B150); the original suspect REFUTED; FIX MADE 2026-09-22 (FLAKEFIX2, rmbp-ledger B159) and UNSCORED — the bench would not reach the ring's depth**

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

**Trigger conditions.** Seen **1 run in 12** when this entry was opened. Measured
properly on 2026-09-22 across every QEMU boot on the bench that day, de-duplicated
by each boot's own four tap lines: **2 FAIL in 65 boots, about 3%** — lower than
the opening estimate, and low enough that a seat meets it about once a week and
has no reason to remember it. Host load 9-41, up to nine concurrent `cargo`
builds. Load-correlated in the same way
as class 1: the drop path is reachable only while the sink lock is contended
*and* the 64-slot staging ring is already full, which needs several cores
printing at once.

**Root cause — MEASURED 2026-09-22 (FLAKEFIX, rmbp-ledger B150). The tap is
`ftdi`, the mechanism is DEPTH EXHAUSTION under lock contention, and the suspect
below is REFUTED.** Four captures from one day, three of them other executors'
and one reproduced by FLAKEFIX on its own tree, all on the same box:

| capture | ftdi tap at the verdict | tste tap | verdict |
| --- | --- | --- | --- |
| `crystalboot-logs/before-serial.log:588` | `staged=65 dropped=7 torn=0` | `dropped=0` | `FAIL — balanced=true evidence_lost=7` |
| `flakefix-logs/gored-serial.log:597` | `staged=64 dropped=13 torn=0` | `dropped=0` | `FAIL — balanced=true evidence_lost=13` |
| `crystalboot-logs/after-serial.log:587` | `staged=30 dropped=0 torn=0` | `dropped=0` | `PASS` |
| `gmux8-logs/test240.log:1290` | `staged=10 dropped=0 torn=0` | `dropped=0` | `PASS` |

Three things fall out of that table and each of them settles a question this
entry used to have to ask a future investigator for.

**(1) `torn=0` on every tap of every capture, FAIL and PASS alike.** That is
this entry's own discriminator #3 below, and it fires against the suspect: the
loss is the depth-exhaustion path, not the slot-width path, so **per-line growth
is exonerated** and experiment #6 below does not need running. The `stalls=`
suspect is retired.

**(2) `staged` reaches the ring's depth exactly on the FAIL runs (64 and 65
against `SLOTS = 64`) and sits at 10-30 on the PASS runs.** The ring is not
marginally sized for the ordinary boot; it is exactly sized for the burst, and
the burst that reaches it is SERWIT-1's own — `5 cores x 24 lines` plus 5
wide-line probes, 125 lines, with the same run's SERWIT-1 line reading `110-116
deferred to the staging ring` against `100-111` on the greens.

**(3) The sink is NOT a host device and there is no QEMU backpressure in this
path.** `drivers/xhci/ftdi.rs` `mirror()` loses a line only when, in order,
`RING.try_lock()` fails, the 64×240 `STAGE` ring is full, *and* the one free
retry `RING.try_lock()` fails again. `RING` is an in-kernel capture ring behind
a spinlock — the FTDI *device* (`-device usb-serial`) is attached only under
`UNAOS_USBSERIAL` (`builder/src/main.rs:1521`) and is absent from every default
`./arroyo test`. The host reaches this only through how long a core holds `RING`:
a vCPU descheduled by a loaded host mid-memcpy stretches the hold, the other four
cores all miss the `try_lock`, and `STAGE` fills behind them. "sink contended or
full" means **contended**, every time this has been measured.

**THE FIX IS NOW MADE — AND IT IS UNSCORED, WHICH IS SAID HERE FIRST BECAUSE IT
IS THE HALF A READER MUST NOT SKIM PAST** (FLAKEFIX2, rmbp-ledger B159, branch
`exec-rmbp-flakefix2`). `mirror()`'s third step was a single `try_lock`, so a
line was declared lost after losing ONE race, while `serial_ring`'s own primary
producer BACK-PRESSURES instead (`:: SERWIT-1B: … the contended producer
BACK-PRESSURES, it does not drop … one capped drain freed 3 slot(s), the next
turn DEFERRED the line intact, 0 dropped -> PASS ::`). The mirror now takes that
same policy, through the same spelling: `crate::serial_ring::defer_policy`, whose
six rows the compiler already checks on every `./arroyo check`, bounded by the
primary wire's own `BACKPRESSURE_SPINS` so the transport has ONE checkable
magnitude rather than a second one nobody can check. Each turn re-tries the SINK
first (winning it drains `STAGE` and writes the line intact), so the wait bears
progress and room can also arrive from another core's drain; nothing is held
across a turn; in panic mode the bound collapses to 1, which is the old single
free retry turn for turn, so a dying machine cannot spend a bounded wait per line.
`evidence_lost`'s threshold is UNCHANGED at zero — the 13 lines were really gone,
and on a 2012 rMBP with no 16550 the FTDI capture is not a mirror of the evidence,
it IS the evidence, so a tolerance here would be the wrong-lenient half of LAWS §5
applied to the one tap that cannot afford it.

**WHY IT IS UNSCORED, WITH THE NUMBERS, because an unproven fix recorded as a
proven one is the costlier error.** The proof this entry asks for is a run reading
`staged >= 64` with `dropped=0`, i.e. a run that REACHES the ring's depth. FLAKEFIX2
could not make the bench reach it. Four `./arroyo test` boots on 2026-09-22, all on
the loaded box this entry's own rates were measured on, read the `ftdi` tap at
**`staged=0`, `staged=1`, `staged=0`, `staged=0`** — below even the `staged=10..30`
this entry records for its PASS captures, and two orders of magnitude below the
`staged=64/65` the FAILs reached. Host load 29 → 57 across them (`uptime` quoted in
rmbp-ledger B159), six sibling `cargo` builds, and a run with all five vCPUs pinned
onto four already-spinning host CPUs — the deschedule-mid-memcpy shape this entry
names as the mechanism — moved the reading from 0 to 1 and no further. **The branch
that was changed was therefore never taken in any of the four runs**, so the four
greens are a NO-REGRESSION measurement (rc=0, `SERWIT-2 … -> PASS`, the four tap
lines' accounting identical, `balanced` intact) and nothing more. The go-red is
unreachable for the same reason: re-introducing the single-attempt path cannot
produce `dropped>0` on a boot that never fills the ring.

**What a future seat needs, stated as the specific missing instrument rather than
as more wall.** At the measured rate (2 FAIL in 65 boots) three CONSECUTIVE greens
at depth is not a thing wall-clock buys. What is needed is a DELIBERATE WIDENER that
drives `STAGE` to its 64 slots on demand — a SERWIT-1-shaped burst that holds `RING`
across a stretched `ring_write`, or an `arroyo` knob that does. Every place such a
widener can live is outside `drivers/xhci/ftdi.rs`: the SERWIT fixtures are in
`crate::serial_ring`, and a knob is `unaos/arroyo` plus `builder/src/main.rs`. That
is why it is not here, and it is the one thing to give whoever scores this.

The suspect that stood here before, kept because the arithmetic is still true and
the next reader should not re-derive it: **per-line growth on the rollup lines**
— concretely, the `stalls=` field, about **9 bytes per rollup** — against a
serial ring whose margin is real and finite. The arithmetic is in-tree and
verified:

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
first would produce `evidence_lost`.** That asymmetry was the cheapest available
discriminator; it has now been checked against four real captures and it reads
`torn=0` in all of them, which is why the width half is retired above.

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
6. ~~If it is reproducible under load, the discriminating experiment: re-run with
   the rollup lines shortened and see whether `evidence_lost` follows.~~ **Not
   needed — `torn=0` in four captures answers it. Do not spend a run on it.**

**Disposition — ROOT CAUSE KNOWN, FIX MADE AND UNSCORED (see above; it stays on
the owed list until a run at depth scores it), and CORRECT one sentence this entry
used to carry: it DOES turn the gate red.** The old text said SERWIT-2 "does not
turn a gate red on its own" because no `.spec` file carries a SERWIT token. That
is true of the spec replay and false of the run: `FAIL ::` is in arroyo's
`FAULT_PATTERNS`, so `scan_serial_faults` reds the leg on the verdict line and
the spec replay never runs. Measured twice on 2026-09-22 —
`✖ serial.log:591: :: SERWIT-2: FAIL — balanced=true evidence_lost=7 ::` ending
CRYSTALBOOT's baseline run, and `✖ serial.log:600: … evidence_lost=13 ::` ending
FLAKEFIX's. So a sighting is a **lost gate run**, not evidence to bank quietly,
and that is what puts the fix above on the owed list rather than the watch list.

**Companion sighting, same ring, same day, recorded so it is not diagnosed
separately.** FLAKEFIX's `green2` run at host load 33 red on four `serial_ring`
drain fixtures at once —
`:: PWRDRAIN: FAIL — filled=64 lines=61 bytes=4148 want_bytes=4352 …`,
`:: S5DRAIN: FAIL — filled=64 lines=25 bytes=1675 want_bytes=4288 …`,
`:: SINKDRAIN: FAIL — staged=8 drained=0 on_cable=0 …`,
`:: SERWIRE: FAIL — filled=8 capped_drains=1 capped_b=0 want_b=201 …` — every one
of them `filled=64`, i.e. the same ring at the same depth under the same
contention, and every one with `dropped=0` and `residue=0`. They are this class,
not four defects, and a fix that gives the mirror the primary wire's
back-pressure policy should be scored against them too.

### 2b. `[mirror] tste: N line(s) dropped` is **NOT** what `evidence_lost` counts — **measured 2026-09-22, recorded so the wrong inference is not drawn twice**

**The wrong reading, and it is the natural one.** A capture carries
`[mirror] tste: 13 line(s) dropped, 0 truncated since boot (sink contended or
full)` and a SERWIT-2 FAIL, and the two get joined. They are unrelated, and three
captures from one day say so:

| capture | `[mirror] tste` lines | tste tap AT the verdict | SERWIT-2 |
| --- | --- | --- | --- |
| `gmux8-logs/test240.log` | 12 of them, lines 2832-2939 | `:1291 dropped=0` | `:1293 PASS` |
| `crystalboot-logs/before-serial.log` | 11 of them, lines 2146-2257 | `:589 dropped=0` | `:591 FAIL, evidence_lost=7` (carried by **ftdi**, `:588`) |
| `flakefix-logs/gored-serial.log` | 11 of them, lines 2182+ | `:598 dropped=0` | `:600 FAIL, evidence_lost=13` (carried by **ftdi**, `:597`) |

**The mechanism of the confusion is ORDERING.** SERWIT-2 is a one-shot verdict
that prints at ~line 590-1290 of the boot; every `[mirror] tste:` line in all
three captures lands ~1500 lines LATER, in a different phase. The tste tap reads
`dropped=0` at the instant the verdict snapshots it, in every capture, green and
red alike. GMUX8's run is the clean falsification: 13 tste drops in the capture
and `SERWIT-2 … -> PASS` with `evidence_lost=0`.

**What the tste drops are.** A separate, later, still-real loss on the
boot-verdict replay ring, from the same contention mechanism as 2a but past the
verdict's window — so no fixture scores them and nothing reds on them. They are
worth a class entry of their own when someone holds `serial_ring.rs`; they are
not worth attributing to a SERWIT-2 number they cannot have contributed to.

**What to capture on recurrence.** The tap line at the verdict, with its serial
line number, beside the `[mirror]` line's. If the two are more than a handful of
lines apart, they are describing different moments and only the tap line is a
reading of what the verdict scored.

---

## Class 3 — PTRDEAD's backlog leg loses its coalesced entry to a FOREIGN consumer

**Signature on the wire.** The `-> FAIL` form of
`arch/x86_64/syscall.rs:6699`'s rollup, with `fpop12` NON-ZERO:

```
[ptrdead] backlog whole=false nodrop=true order=true pushed=192 entries=0 travel=(0,0) folded=191 dropped=0 fpop12=2 fpop3=0 quiesced=false -> FAIL
```

The PASS from the same build, same session, on an idle host:

```
[ptrdead] backlog whole=true nodrop=true order=true pushed=192 entries=1 travel=(192,-192) folded=192 dropped=0 fpop12=0 fpop3=0 quiesced=false -> PASS
```

**`fpop12` is the discriminator and it is already printed** — read it before
anything else. `foreign12 = (evq_pops() - pop0) - entries` (`arch/x86_64/syscall.rs`,
in the same fixture) counts pops made by a consumer *other than this leg* while the
leg held the queue. The leg pushes `QUEUE_SIZE_PUB * 3` relative reports expecting
them to coalesce into ONE entry carrying the whole travel; another drain popping two
of them mid-leg leaves `entries=0`, `travel=(0,0)` and `folded` one short of
`pushed`, and `whole` is false by arithmetic. Note `quiesced=false` on BOTH lines
above: it is *not* the discriminator, and reading it as one sends you after the
wrong mechanism.

**Trigger conditions.** Host CPU contention. Observed once (2026-09-06, orin 16
KEYDOORS-FIX) on the single `./arroyo test 60` run that was launched while
`./arroyo kernel8` was compiling in the same worktree — the two share `target/`
and the host's cores. Three runs of the **same kernel binary** with the host
otherwise idle were green, before and after. Same family as Class 1 — an observer
racing another consumer, made visible by load — but a different leg and a
different accounting field.

**Root cause — SUSPECT, mechanism clear, not confirmed.** `fpop12=2` proves a
foreign consumer ran; *which* one has not been established. The likely candidates
are the render-service drain and the drag belt, both of which pop `EVENT_QUEUE`
and neither of which is quiesced for this leg. Whether the correct cure is to
quiesce them (as the `order` leg alongside it already does via `SFQ_QUIESCE`) or
to make the leg tolerate a foreign pop is a design question this entry does not
answer.

**What to capture on recurrence.**

1. The full `[ptrdead] backlog` line — specifically `fpop12`, `fpop3`,
   `entries`, `folded` and `pushed`. `fpop12=0` with `whole=false` is a
   DIFFERENT failure and does not belong to this class: it means the coalescing
   itself broke, which is a regression.
2. The `[ptrdead] order detail: got=… fpop=… fpush=… quiesced=…` line, if the
   `order` leg also went red.
3. What else was running on the host — concurrent `arroyo` invocations above all.
   A red seen alongside a build is weak evidence; a red on a quiet host is strong
   evidence and moves this entry off SUSPECT.
4. Whether a re-run on an idle host is clean, and how many.

**Disposition — WATCH.** Not fixed, and it DOES turn the x86 gate red on its own:
`FAIL ::` / `-> FAIL` are in arroyo's `FAULT_PATTERNS` (`arroyo:2349`) and the
test leg's exit status is the fault scan's. So this is a real gate failure to
diagnose, not a line to skim past — but with `fpop12` non-zero and a build racing
it, it is this class rather than a regression in whatever arc is in flight.

**Do not run gates concurrently in one worktree.** The runs share `target/` and
the host's cores, and this class is the second-order cost — the same reason
`target/` is never a flash-staging handoff source.

**2026-09-22 reading, recorded so the field is not misread (FLAKEFIX, B150).**
Every `[ptrdead] backlog` line carrying a non-zero `fpop12`/`fpop3` in that day's
captures ends `-> PASS`, because the affected legs now read
`whole=skip nodrop=skip order=skip` — the leg DECLINES when a foreign pop is
seen instead of asserting through it, which is one of the two cures this entry
said it did not choose between. The line's grammar has also moved on from the
one quoted above: it ends `cpu=N svc=Some(N)` now, not `quiesced=`. So a
non-zero `fpop3` on a current capture is **not** by itself a red, and
rmbp-queue's `· B9` row (`[ptrdead] … fpop3=1 -> FAIL`, 2 reds in 5 WC runs,
both at load ≥ 24) is about the older grammar. A `-> FAIL` with `fpop12=0` is
still the regression this entry warns about.

---

## Class 4 — `test-arm` captures NO serial bytes at all (aarch64 virt, loaded host)

### 4a. `no serial bytes captured … QEMU produced nothing. Exit 4` — **suspect only, one occurrence**

**Signature on the wire.** There is none — that is the class. `target/serial-arm.log`
is zero bytes and the harness says so itself, on stdout:

```
✖ test-arm: no serial bytes captured at …/target/serial-arm.log — QEMU produced nothing.
  This run carries NO verdict: not a pass, not a regression. Exit 4.
```

**Trigger conditions.** Seen once, orin 23 (2026-09-08), on
`UNAOS_GICV3=1 UNAOS_NET4=1 UNAOS_NET5=1 ./arroyo test-arm`, with **twelve other
`./arroyo check` runs live on the same host** (a nine-executor fleet plus other
seats). The immediately preceding and following runs of the SAME command on the
SAME tree were both exit 0 and produced a full 454-line log, so the image and the
invocation are exonerated by differential.

**Root cause.** Unknown; host-load launcher race is the suspect. The harness
already handles it correctly and that is why this entry is short: exit 4 is a
distinct code and the text says in as many words that the run carries no verdict.
It is NOT a regression signal and must never be scored as one.

**What to capture on recurrence.** The host load at the time (`uptime`, count of
concurrent `arroyo`/`cargo`/`qemu-system-*`), whether the ESP was rebuilt that run,
and whether the log file was created-but-empty or absent. If it ever recurs on an
idle host, it stops being a load artifact and becomes a launcher defect.

**Disposition.** On watch. One occurrence, re-run green on the spot; recorded here
so the next seat spends the minute on the corpus and not on the driver.

---

## Class 5 — a COMPLETED run delivers an INCOMPLETE capture: witnesses go missing with no verdict at all

**The shape, and why it is its own class.** Classes 1-3 are races that make a
fixture print the WRONG verdict. This class is the one where the fixture prints
the RIGHT verdict and the line never reaches the capture. The failure therefore
arrives as a **MISSING REQUIRE on a run that reached its end-of-run marker** —
and the harness, seeing the marker, says in as many words that the absence must be
a regression:

```
  ❌ REQUIRE    :: <TAG>: … -> PASS ::
       0 hits — MISSING
  ❌ MBENCH FAIL — 125/126 required witnesses, 0 forbidden hit(s), 23846 lines scanned [full wall 300.1s]
       (the end-of-run marker was seen — the run completed, so a missing witness here is a GENUINE regression)
```

**That parenthesis is the trap.** A completed RUN is not a complete CAPTURE. The
marker proves the boot reached its end; it proves nothing about whether every line
in between survived the wire.

**The discriminator, and it is free.** Before treating a missing REQUIRE as a
regression, ask whether the producing path demonstrably ran:

1. **Is the fixture's own BANNER on the wire?** Many fixtures print a banner
   before their verdict. Banner present + verdict absent = the fixture ran and the
   line was lost.
2. **Is a SIBLING line from the same emitter present?** If one `serial_println!`
   from a statement sequence landed and its immediate neighbour did not, no code
   path explains it — read the emitter and check for an early return between them.
   If there is none, the line was lost, not skipped.
3. **Is there a TORN fragment?** Grep a distinctive interior token of the missing
   line (not its tag). Zero hits for the interior token = whole-line loss, which is
   `dropped`, not `torn` — Class 2's asymmetry, one layer out.
4. **Are there `[mirror] … line(s) dropped` lines in the capture?** They corroborate
   that loss happened on this boot, though they do NOT account for the primary wire
   (see the SERWIT-2 caveat below).

Only when all four say the fixture never ran is a missing REQUIRE a regression.

**The SERWIT-2 caveat, measured.** `:: SERWIT-2: … -> PASS ::` does NOT clear this
class. Both captures below carry SERWIT-2 PASS (`every line accounted for on all 4
taps, 0 lost on the 3 evidence taps`) *and* lost witnesses. SERWIT-2 conserves over
the four MIRROR taps (`fbcon`, `ftdi`, `tste`, `flightrec`); the primary PL011
capture the battery is scored from is not one of them. A green SERWIT-2 is not a
statement about the log MBENCH reads.

### 5a. `kernel8-test`: `[wc-g] -> COHER` / `-> RACE-BLIT` + `[wc-d] verify -> FAIL` — **known shape (SO7), rate now measured**

**Signature on the wire:**

```
[wc-g] win=1 seq=0 own=no scale=1x app=… blit=… civac=… after=… fbbad=1830/82944 occluded=0 occ=0/0 us=25054 rectscan_us=10000 slow=yes -> COHER
[wc-g] win=1 seq=9 own=yes scale=1x … fbbad=1647/82944 … us=9555 rectscan_us=10000 slow=no -> RACE-BLIT
[wc-g] rollup win=1 scope=window samples=4 coher=2 race=1 blit=0 clean=1 slow=1 maxus=25054 wit_us=48019 frame_us=16667 -> COHER
[wc-d] verify win=1 surf=288x288 band=none scale=1x at (17,51) panel=640x480 checked=82944 bad_cache=783 bad_ram=829 ram_indep=yes moved=1064 sprite_px=0 nonzero=82944 occluded=0 occ=0/0 cksum=0xd731c913edbb9654 first=(160,132) got=0xc9a6e8 want=0x1e1e1e fills=2->2 fact=0/0 desk=9->9 dact=0/0 -> FAIL
```

The green counterparts from the SAME tree, one run earlier:

```
[wc-g] rollup win=1 scope=window samples=4 coher=0 race=0 blit=0 clean=4 slow=1 maxus=17549 wit_us=31948 frame_us=16667 -> CLEAN+SLOW
[wc-d] verify win=1 … bad_cache=0 bad_ram=0 ram_indep=yes moved=0 … stable=yes -> PASS
```

**Read `moved=` FIRST, then `maxus=`.** `moved != 0` means the reference moved
UNDER the verifier, so the sample is invalid and the `bad_*` counters describe a
race, not content. `maxus` is the starvation tell (25054 µs failing vs 17549 µs
clean). Note the red line also LACKS the `stable=yes` field the green one carries.

**Rate — measured 2026-09-16 (FLAKERATE, tree `da2a9abc`, 20-core box):
1 red in 2** `UNAOS_QEMU_FULL=1 ./arroyo kernel8-test 300` runs. **The red was the
LOWER-load run:** 1-min load 13.35 red, 13.84 green. Prior tallies: orin 16's
baseline 3 FAIL/5 and patched 4 FAIL/7 (SO7/B26); SO7's stated load threshold was
~3.9.

**Root cause — attributed, not fixed.** SO7 names it at `wcg.rs:412`: a boot-seam
concurrent writer, fbcon's glyph raster from print context against the compositor's
checksum read. `own=no` on the first hit is the "repainted as COLLATERAL" tell.

**⚠ Divergence from SO7's stated shape, banked not folded.** SO7 records the
observed family as `bad_cache == bad_ram` (91/91, 867/867, 6816/6816) and calls the
one asymmetric run (145/83) *a SEPARATE observation, not this shape*. The sighting
above is `bad_cache=783 bad_ram=829` — the **second** asymmetric sighting. Do not
merge it into the symmetric family without a third.

**The rule for a reader.** Re-run the leg alone; a lone green is the verdict. But
**do not clear the gate and move on**: bank the line here, because the rate is what
closes SO7/B26 and a buried green is a lost sample.

**Disposition — WATCH, rate measured, fixture fix already named and not taken.**
SO7's cheap fix is a distinct `-> MOVED` / `-> RESAMPLE` verdict when `moved != 0`
— **never a relaxed FORBID**. This sighting is the argument for it: the instrument
printed `moved=1064` and the fixture rendered FAIL anyway.

### 5b. `kernel8-test`: `:: U5: capabilities … -> PASS ::` missing with its banner on the wire — **measured, 1 in 2**

**Signature on the wire.** The banner lands, the verdict does not:

```
:: U5: capabilities — rights + CHECK + grant/attenuate/revoke + routed sys_write ::        <- present
:: U5: capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded, revoke enforced, teardown-clear clean -> PASS ::   <- ABSENT
```

scored as `❌ REQUIRE U5: capabilities.*-> PASS  0 hits — MISSING`.

**Trigger conditions.** 1 run in 2, 1-min load 13.35, on a run that saw its
end-of-run marker (25961 lines, full 300.1 s wall). The other run at 13.84 printed
both lines.

**Root cause — SUSPECT: whole-line loss on the primary capture.** The banner proves
the fixture ran. No torn fragment exists. Three `[mirror] … line(s) dropped` lines
are in the same capture (`fbcon: 8`, `tste: 1`, `tste: 2`) while SERWIT-2 reports
PASS — see the class caveat above.

**What to capture on recurrence.** Whether the banner is present (it is the whole
diagnosis); the `[mirror]` lines and their positions; the four `SERWIT-2 tap …`
lines; the host load; and whether a lone re-run prints both.

**The rule for a reader.** Banner present + verdict absent is NOT a capabilities
regression. Re-run alone; a lone green is the verdict.

**Disposition — WATCH.** One occurrence at a measured rate of 1 in 2.

### 5c. `kernel8-test`: `:: ERET-SCRUB: first-entry …` absent in BOTH runs — **NOT a flake; recorded here so it is not mistaken for one**

**This entry exists to stop a reader filing it in this class.** `pi4-regression.spec:309`
requires:

```
REQUIRE :: ERET-SCRUB: first-entry GPR/FP/TPIDR residue = 0 .*-> PASS ::
```

and it was MISSING in **2 of 2** FLAKERATE runs (loads 13.84 and 13.35). Two in two
is a defect, not a rate.

**What is measured, and it is unusual enough to write down.** The emitter,
`arch/aarch64/syscall.rs:6541` (`eret_scrub_verdict`), prints TWO lines as
consecutive statements — first-entry (`if/else` at `:6550-:6559`) then
syscall-return (`if/else` at `:6560-:6569`) — and **all four arms print**, so no
code path skips line 1 and reaches line 2. In both captures:

- `LC_ALL=C grep -a -c -F 'ERET-SCRUB'` = **1**, and the hit is line 2:
  `:: ERET-SCRUB: syscall-return preserved x1-x30 + x8 + SP_EL0 + v0-v31 across SYS_YIELD (bitmap=0x0) -> PASS ::`
- `first-entry` = 0 hits, `TPIDR` = 0 hits, `FPSR` = 0 hits — absent in BOTH its
  PASS and its FAIL form, with no torn fragment.
- the producing path ran: `:: SCHED: task 'el0-eretentry' -> core 3 …` and
  `[el0stkhw] task=72:el0-eretentry …` land immediately before line 2.

**The reading a seat must not take.** `124/126` here is **not** a return-path scrub
regression — the scrub's sibling witness passed in both runs. Whether this is a
2-in-2 capture loss or something in the emitter is unresolved and needs a quiet box
to separate; FLAKERATE could not make the box quiet.

**Disposition — DEFECT, open, owner unassigned.** Filed as a defect, not a flake.

### 5d. Companion finding: a `FAIL` the DEFAULT FORBIDs cannot see

Not a flake, recorded because it will make a reader of this class mis-score a run.
In a `kernel8-test` run MBENCH scored **`0 forbidden hit(s)`**:

```
:: PWRDRAIN: FAIL — filled=64 lines=12 bytes=816 want_bytes=4352 residue=0 dropped=0 ::
```

`mbench.py:136`'s `DEFAULT_FORBIDS` are `-> FAIL`, `FAIL ::`, `PANIC`; this line's
form is `FAIL — `, with `::` seven fields later, so it matches none of them.
**Scan a capture for the bare token `FAIL`, not for the three shapes the harness
knows.** (Done over the four x86 captures of the same arc as a control: `FAIL`
count equals `-> FAIL` count in all four, so this is a Pi emitter's spelling and
not a tree-wide hole.)

### The x86 CONTROL for this whole file, measured the same day

FLAKERATE ran four `UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 90` runs on one tree
(`da2a9abc`) at 1-min load 7.90 / 15.55 / 15.18 / 15.09, peak 20.57 observed
in-run, and got **byte-identical verdict sets**: 119 distinct witness tags, 78
`-> PASS`, 22 `:: PASS ::`, 3 `-> FAIL`, every time, all four reaching the
completion marker. **Zero flakes in four** across DOCKID, `[dmgovlp]` (1c),
`[ptrdead]` (3), SOCK-4 (1b), DMG-REFUSE (1a), SERWIT-1, PWRDRAIN, S5DRAIN,
SINKDRAIN, DOCK and APPPIN — every one of them present in all four captures, so the
zero is a fact about the data and not about the pattern. **The practical
consequence for this corpus: none of Classes 1-3 reproduced at load ~20 on a
20-core box**, and a seat meeting one of them should not assume "the box was busy"
is a sufficient account. Full run table and quotations:
`docs/dev/evidence/rmbp-0915/flakerate/FLAKERATE.md`.

---

## Class 6 — the fixture drives a step that may DECLINE, then scores the value that step was to publish

**The shape, and why it is not Class 1 or Class 5.** Class 1 is a launcher racing a
ring-3 fixture's *teardown*; Class 5 is the right verdict *lost on the wire*. This
class is neither: everything is in-kernel, one task, and the line reaches the
capture intact — it is simply **wrong**, because the fixture treated a request as a
result. A kernel-side fixture calls a drive step whose signature is `-> ()` and
whose contract explicitly allows it to decline the whole pass and be re-asked
later, then reads the state that step was supposed to have published.

**The discriminator, and it is free.** Ask whether the PUBLISHER's own witness ran
between the drive and the read. If the publisher's line is absent — or present but
positioned *after* the verdict — the fixture scored a value that had not been
written yet. A verdict whose fields are all the *zeros of an empty snapshot* rather
than plausible-but-wrong numbers is the same tell one layer down.

**The rule for a reader.** A red whose numbers are structurally empty (`0x0+0`,
`passes=0`, `opens=0`) is a fixture that never got to ask its question. That is not
the subsystem's verdict and must never be re-run away silently — record it here.

### 6a. `:: WINMENU: … app_box=false … :: FAIL ::` — **known, fixed**

**Signature on the wire** (pre-fix; byte-identical across two sessions and two
trees, which is itself part of the diagnosis — a real red would vary):

```
:: WINMENU: win=1 name=gate box=0x0+0 title-x=6 drop=0x0+0+0 font=chrome20-bold panel=1280x800 app_box=false routed_open=false geometry=false escape=false quit_closes=false app_name_late=false :: FAIL ::
[winmenu] selftest passes=0 paints=0 rate=0/1k scan=0cyc/0us paint=0cyc/0us px/paint=0 live=0 bar_owner=0 open=0 publishes=0 clears=0 opens=0 dismisses=0 picks=0 refusals=0
```

The green counterparts, for contrast:

```
:: WINMENU: win=1 name=gate box=48x34+34 title-x=40 drop=134x65+40+34 font=chrome20-bold panel=1280x800 app_box=true routed_open=true geometry=true escape=true quit_closes=true app_name_late=true :: PASS ::
[winmenu] selftest passes=2 paints=2 rate=1000/1k scan=106060cyc/53us paint=1226180cyc/614us px/paint=8710 live=0 bar_owner=0 open=0 publishes=0 clears=0 opens=2 dismisses=2 picks=1 refusals=0
```

**Read `title-x=6` FIRST.** It is `x[0] + TPAD` (`winmenu.rs`'s `BarSnapshot::text_x`,
`TPAD == strip::PAD / 2 == 6`), so `title-x=6` means `x[0] == 0` — the snapshot is
`BarSnapshot::empty()`, handed back by `bar_boxes`'s no-publisher early return
(`if LIVE == 0 && app_owner == wm::WIN_NONE`). The six falses are one fact, not six.

**The collateral, and it is the loudest tell in the capture.** Leg 2 presses the app
box's CENTRE, computed off that empty snapshot — so it presses `(0, 0)`, which
`strip::press_route` falls through winmenu into `crystal::press_at`'s brand-mark
corner. Every red therefore walks the SHARD menu open and scores its state:

```
:: SHARD-MENU: crystal_press=open via=corner-zone menu=170x121+0+34 items=4 ::
```

**It is COUNTABLE, and it separates the two states on 10 captures out of 10.** The
crystal's own fixture opens the shard from the corner exactly once per boot; leg 2
and leg 5 of a red winmenu fixture add two more. Over both sessions' ten runs:

```
LC_ALL=C grep -a -c -F 'via=corner-zone'   # 1 in all eight greens, 3 in both reds
LC_ALL=C grep -a -c -F 'via=fixture-direct' # 5 in all ten — unchanged, so the delta is real
```

A seat that sees the crystal moving inside the winmenu fixture's window is looking
at this class, not at a crystal bug. **Count first: it is one command and it needs
no line numbers.**

**Trigger conditions — measured 2026-09-16 (WINMENUFLAKE) from the captures on
disk: 2 red in 10, i.e. 1 in 5 in each of two independent sessions.**

| Session | Command | Runs | Red |
| --- | --- | --- | --- |
| INSTALLVERB, 2026-09-15 | `UNAOS_INSTGUI=1` + `wc` | 5 (`baseline`, `run1`-`run4`) | 1 — `recon1.txt`, verdict at `serial.log:1642` |
| QUARRYX86-2, 2026-09-16 | `UNAOS_WC=1 UNAOS_QUARRY=1 ./arroyo test` | 5 (`recon`, `baseline`, `press`, `press2`, `gored`) | 1 — `test-recon.log:2283` |

The five QUARRYX86-2 runs are distinct runs and not re-readings of one: their
`[winmenu] selftest … scan=` counters are `0`, `137040`, `74580`, `72290`, `106060`
cycles. (`serial-press-AFTER.log` and `serial-gored-BEFORE.log` are the serial sides
of `test-press2` and `test-gored` — same counters, so they are **not** extra runs.)

**Both reds were that session's FIRST run** — the `recon` run in each case, against
a QEMU that had not run on that box yet. Neither session reproduced it again at any
load. That correlation is recorded as an observation and is not part of the
mechanism below; it is what to try first if it ever needs reproducing.

**Root cause — known.** `winmenu::selftest` drove `wm::focus_changed(OWNER)` +
`wm::composite()` and read `bar_boxes()` on the next line. Only
`menubar::compose` → `winmenu::set_app_window` ever stores `APP_OWNER`, and
`wm::composite()` returns `()` and has three documented arms that return early
without reaching it, every one of them correct and all three saying *"re-ask next
composite"*:

- x86 `COMP_GATE`'s second-entrant **FOLD** (`wm.rs`, COMPGATE);
- `menubar::compose`'s `panel_snapshot().filter(is_ready)` arm (LOCKFIX B1);
- `menubar::compose`'s `if model.menus.busy { return false; }` arm (PANEL V-3).

So one `wm::composite()` is a *request* to publish. The two reds caught both halves
of that: in QUARRYX86-2's the publication never landed at all before the verdict —
`[winmenu] app-menu owner=1 name=gate` is **absent**, and the next one
(`…name=VUG from=program`, line 2285) lands two lines **after** the FAIL at 2283. In
INSTALLVERB's the composite landed *late* — `[winmenu] app-menu owner=1 name=gate`
(2250) and `[menubar] menus cap_owner=1 cap=gate … boxes=1 items=app:gate@34+48`
(2253) are both present, but the fixture's own corner-zone presses (2245, 2251) are
already above them. Same defect, two interleavings.

**Fix.** `winmenu.rs`'s `await_app_publish(want, name)`: the fixture now parks until
the bar is holding the window *and the name* it is about to score, re-driving the
composite (the publisher is inside it, so asking again is the only thing that can
make the store land), bounded at 250 ms. A timeout is **`-> SKIP reason=menu-unpublished-after=<ms>ms`,
never a pass and never a FAIL** — the bar not having settled is a statement about
the compositor's luck this boot, not about whether R21 holds. Both publish points
carry it: leg 1 (`gate`) and leg 6 (`VUG`). The name is checked as well as the id
because the gate window and leg 6's program window are **both `win=1`** on every
capture in the corpus, so an id-only predicate would let leg 6 fire on leg 1's stale
publication. The verdict line now carries `owner=… published=y waited=<ms>ms
prog_waited=<ms>ms`, so a PASS that had to wait is visible instead of silent.

**Sibling instruments to check before filing anything here as this class.** Neither
`:: CRYSTAL-MENU: … :: PASS ::` nor `[menubar] menus …` is a clearance: both are
green in the reds.

**What to capture on recurrence.** The new fields make this cheap: `published=`,
`waited=` and `prog_waited=` off the verdict line, plus whether a
`-> SKIP reason=menu-unpublished-after=` line printed instead. A non-zero `waited=`
on a PASS is the *same* declining compositor caught and recovered — bank it, it is
the rate instrument for whether the 250 ms bound is right. Then: whether
`[winmenu] app-menu owner=… name=gate` is present and where it sits relative to the
verdict, and whether any `via=corner-zone` shard opens are in the region.

**Disposition — FIXED (fixture ordering only; no product change), on branch
`exec-rmbp-winmenuflake` off `11ca67f1`.** The menubar never published the wrong
owner's menu: it published nothing, by design, and the fixture scored the gap. No
`rmbp-ledger` row — there is no product defect here.

**The go-red, because it is what makes this entry's root cause a measurement
rather than a story.** Forcing the losing order deterministically — clear the
publication and answer `published` without waiting, i.e. exactly what the unfixed
fixture assumed — gives **rc=1 in 2 of 2 runs**, both
`box=0x0+0 title-x=6 drop=0x0+0+0` with all six legs false and `via=corner-zone`
at 3: **byte-identical to the two reds above**, from two sessions this seat did
not run. The new `owner=0` field convicts the mechanism outright — `APP_OWNER` was
`WIN_NONE` at the read — which is precisely what the old line could not say.

**One gap left open, and a reader of this class should know it.** NO spec REQUIREs
`:: WINMENU: … :: PASS ::` — `grep WINMENU unaos/scripts/specs/*.spec` is empty, so
this fixture was only ever scored by `mbench.py`'s `DEFAULT_FORBIDS`. A SKIP matches
none of them, so it is GREEN and silent: the right disposition for a flake and the
wrong one for a real R21 regression, which would now SKIP where it used to FAIL. One
`REQUIRE :: WINMENU: .*:: PASS ::` row in `x86-test.spec` closes it. Reported as a
STOP by WINMENUFLAKE and not taken — specs were outside that brief's named files.
CLOSED by WINMENUSPEC (branch `exec-rmbp-winmenuspec` off `fa4dcf0b`, same day): the pin now
exists — `REQUIRE :: WINMENU: … published=y … app_box=true … :: PASS ::` in `x86-ahci.spec`
(the `test` verb's WC-armed leg; `crystal::selftest` is `cfg(all(witness, wc))`, so the verdict
is absent from the knob-free default boot and a REQUIRE in `x86-default.spec` would have been the
APPPIN trap) and `FORBID :: WINMENU: .* -> SKIP reason=menu-unpublished-after=[0-9]+ms ::` in
both `x86-ahci.spec` and `x86-default.spec`, so a 250 ms miss on the fold gate's
`UNAOS_WC=1 ./arroyo test` is a red, not a silent green.

---

## Class 7 — the INJECTED-EVENT typist paces on the wall clock, and the emulator FOLDS what the guest did not poll

**The shape.** A fixture counts events a host-side typist injected over QMP, and
pins the count. The typist sends on a fixed cadence and never learns whether
QEMU's emulated device delivered anything. On an idle host the cadence is far
wider than the endpoint's polling interval and every event becomes its own HID
report; under load the guest's poll gap stretches past the cadence, QEMU folds
the events it has not yet handed over, and the report COUNT the fixture pins
comes back short — while nothing was dropped and no error was reported anywhere.
The fixture is then measuring the host's scheduler.

The trap is that the shortfall LOOKS like loss. It is not: the travel is
conserved, QMP acked every command, and the only casualty is the count.

### 7a. PTRLANE `:: PTRINSTALL: installs=27 reports=27` for 36 typed — **known, FIXED 2026-09-22 (FLAKEFIX, rmbp-ledger B150)**

**Signature on the wire.** The spec replay's first shortfall, against
`x86-ptr.spec:66/67`:

```
:: PTRINSTALL: installs=27 reports=27 folds=0 lag_max_ms=20 coalesced=0 drains=27 ::
  ❌ REQUIRE    \[ptrinstall\] installs=36 reports=36 lag_max_ms=\d+ coalesced=\d+ drains=\d+ folds=\d+
       FIRST-SHORTFALL x86-ptr.spec:66
```

**and the discriminator is one field away, on a line nobody was reading:**

```
:: MOUSE-1: 32 reports, last dx=0 dy=0 buttons=0x00 == witness ::
```

The green form of the same witness, same command, same box:

```
:: MOUSE-1: 32 reports, last dx=-24 dy=-24 buttons=0x00 == witness ::
:: PTRINSTALL: installs=36 reports=36 folds=0 lag_max_ms=27 coalesced=0 drains=36 ::
```

**`last dx=0 dy=0` IS THE ROOT CAUSE AND IT IS A DELTA THE TYPIST CANNOT SEND.**
`scripts/qmp_type.py --pointer-kind rel` alternates `+step` / `-step` on both
axes, so every event on the wire is ±24 and a zero delta can only be a SUM.
QEMU's `hid_pointer_event` (`hw/input/hid.c`) folds a new motion event into the
TAIL queue entry while that entry has not been polled — "we combine events where
possible to keep the queue small" — so two adjacent moves become ONE report of
(+24) + (−24) = 0 as soon as the guest's poll gap exceeds the injection gap. The
same capture's `drain_gap_max_ms=140` against a 50 ms cadence is that gap,
measured. The guest then discards the folded report before counting it:
`drivers/xhci/mod.rs` charges `mouse_report_count` (so MOUSE-1 keeps climbing)
but emits `Event::Mouse` only when `dx != 0 || dy != 0`, so `[ptrinstall]
reports=` never sees it. Hence `MOUSE-1` > `PTRINSTALL` > typed, all in one
capture.

**The three candidates, and how each was eliminated.**

| candidate | eliminated by |
| --- | --- |
| the QMP socket | every `input-send-event` returned `{}`; an `error` reply is already fatal in `qmp_type.py` |
| QEMU's 16-deep `QUEUE_LENGTH` drop path | needs >16 undelivered events; a 50 ms cadence against an 8 ms interval never builds that depth, and a drop loses travel — the travel is conserved |
| **QEMU's motion coalescing** | `last dx=0 dy=0`, impossible from the stream, plus `MOUSE-1 count > PTRINSTALL count` in the same capture |

**Trigger conditions.** Host CPU contention. TRACKPAD lost a `test-ptr` gate run
to it at host load 23 on 2026-09-22 (`27` of 36; its control run scored worse
still, 0 `[ptrinstall]` lines). It is **deterministically reproducible without
any load at all** by putting the cadence under the endpoint's 8 ms polling
interval — which is the same mechanism, driven rather than waited for:

```
UNAOS_PTRLANE_NOACK=1 UNAOS_PTRLANE_GAP=0.006 ./arroyo test-ptr 150
  -> :: PTRINSTALL: installs=8 reports=8 …   (36 typed, host load 11)
```

**Fix — the typist ACKS delivery instead of pacing on the clock.**
`scripts/qmp_type.py --pointer-ack N` (`PTRACK`; see that block in the script's
header). After the wall-clock first pass it reads the guest's own
`[ptrinstall] reports=` out of the serial log it already holds open for
`--marker`, and re-sends **exactly the shortfall** `--ack-gap` apart, up to
`--ack-rounds` (3 on this lane). Exactly the shortfall, so the loop can
undershoot and retry but can never overshoot a fixture that pins an equality;
`--ack-settle` (0.5 s) is the quiet the reading is taken after, so a rollup tick
that fires between the last send and the guest's 8 ms poll cannot be mistaken for
a shortfall. On cap exhaustion it prints
`[qmp] pointer ack: GAVE UP after N retry round(s) — guest counted C of T …` and
stops: **the shortfall stays real, the spec literal `installs=36 reports=36` is
untouched, and no tolerance is widened anywhere.** `arroyo`'s PTRLANE block arms
it; `UNAOS_PTRLANE_NOACK=1` and `UNAOS_PTRLANE_GAP=<secs>` are the two host-side
go-reds.

**Measured, at the cadence that fails deterministically (`UNAOS_PTRLANE_GAP=0.006`):**

| run | host load | typist | verdict |
| --- | --- | --- | --- |
| go-red, `NOACK=1` | 11.2 | `36 rel moves … 6 ms apart` | `installs=8 reports=8`, rc=1 |
| green0 | 10.3 | `round 1/3 — guest counted 10 of 36 … re-sending 26` → `36 of 36 after 1 round, 62 sent` | `installs=36 reports=36`, **rc=0** |
| green1 | 9.2 | `counted 6 of 36` → `36 of 36 after 1 round, 66 sent` | `installs=36 reports=36`, **rc=0** |
| green2 | 11.4 → 33.2 | `counted 4 of 36` → `36 of 36 after 1 round, 68 sent` | `installs=36 reports=36`; run rc=1 on an unrelated class-2 red |
| green3 | 33.2 → 41.2 | `counted 4 of 36` → `36 of 36 after 1 round, 68 sent` | `installs=36 reports=36`, **rc=0** |

Four consecutive runs, one retry round each, `installs=36 reports=36` every time
including at host load 41 — and never 37.

**What to capture on recurrence.** The typist transcript
(`target/ptrlane-typist.log`) and the `MOUSE-1` line together. `pointer ack:
GAVE UP` with a large shortfall and MOUSE-1 reading `dx=0 dy=0` is this class
with the retry budget too small; `GAVE UP` with MOUSE-1 NOT climbing at all is a
different failure (the device or the endpoint, not the fold) and does not belong
here.

**Do NOT apply the ack to the keyboard bursts.** `qmp_type.py --bursts` exists to
LOSE events — it measures how many the endpoint drops with no TRB armed — so
acking it would erase the thing it measures. `--pointer-ack` is pointer-only on
purpose.

---

## Adding an entry

An entry earns its place when a failure has been seen **more than once**, or once
with a mechanism worth writing down. Give it the five headings above, quote the
witness text **exactly** as the kernel formats it (copy it out of the source, not
out of memory), and state plainly whether the root cause is known or suspect. An
entry that overstates its confidence is worse than no entry: the whole purpose is
that a cold reader can trust the label and spend their minute accordingly.
