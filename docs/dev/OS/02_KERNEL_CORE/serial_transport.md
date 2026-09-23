# Serial transport — the wire is an instrument, and it must not lie

The serial log is this project's primary evidence. Gates are counted by tallying `PASS` lines in
`target/serial*.log`; bench sittings are read from an attended capture; the wedge and cursor
investigations are conducted entirely through it. Everything downstream of the UART inherits that
log's honesty, so the transport itself has to be held to a stricter standard than the subsystems it
reports on.

This document describes what the transport guarantees, why the guarantee is shaped the way it is, and
how it is proven every run.

## The defect this replaced (SERWIT-1)

`arch::serial::_print` acquired the UART with `try_lock()` and, when the lock was already held, took
the `else` branch — which did nothing at all. The line was discarded.

The `try_lock` was correct and remains correct: a print from an IRQ-masked, fault, or panic context
must never be able to block on a console lock another core owns, and the original `.lock()` shape
self-deadlocked when a panic struck mid-print. The defect was the *failure branch*. There was no
counter, no marker, and no sequence number anywhere in the serial path, on either arch — so a dropped
line was, by construction, undetectable after the fact. The only way to notice one was to already know
which line should have been there.

Consequences, in order of severity:

1. A real regression's `FAIL` line can vanish and the broken build reads green.
2. A gate goes red for no reason: a lost `PASS` is indistinguishable from a fixture that never ran.
3. An attended metal capture loses evidence exactly when the machine is busiest — which is the only
   moment the lockup investigations care about.

**Measured scope.** With the pre-fix failure branch restored under the SERWIT-1 stress fixture
(3 cores × 24 lines, released together so the bursts overlap), **9 of 72 lines reached the wire**. 63
lines — 87.5% — evaporated with no trace, and the accounting read `submitted=73 emitted=10`. That is
the honest magnitude of the loss under contention; the intermittently-missing verdict lines seen in
ordinary runs were the tail of the same distribution.

## What the transport guarantees now

| Situation | Behaviour |
| --- | --- |
| Uncontended | Straight at the UART, as before — plus a drain of anything other cores staged. |
| Contended (`try_lock` fails) | The whole formatted line is deferred into a lock-free staging ring. No spin, no block, no lock. The next core to hold the UART emits it **intact and in order**. |
| Ring full (depth 64) | SERWIT-1B — the producer does **not** discard the line. It goes round again, up to `BACKPRESSURE_SPINS` (1,000,000) turns, and every turn re-tries the UART itself (winning it drains the whole ring and writes the line intact) before re-trying the stage. Only when the bound expires is the line lost — and it is still COUNTED, and the next drain still puts `[serial] dropped N lines, truncated M (staging ring full, depth 64)` on the wire. Loss is never silent again. |
| Line longer than 240 bytes | Truncated at a UTF-8 char boundary, counted, and reported by the same marker. A shortened line never masquerades as a whole one. |
| Panic | The Mutex is bypassed entirely: staged backlog then panic text, raw and synchronous, through the bounded lock-free UART primitive. |

Implementation: [`crates/kernel/src/serial_ring.rs`](../../../../unaos/crates/kernel/src/serial_ring.rs),
shared by both arches' `_print`.

## Why nothing here can deadlock

This is the constraint that shapes the whole design, because the obvious alternative fixes trade
silence for a hang, and a hang in the console is worse than a drop. Note the word that does the work:
an *unbounded* wait is what is forbidden. SERWIT-1B's backpressure is a bounded one, on the one branch
that would otherwise lose the line outright, and it degrades to the old drop-and-count when its bound
expires — see that section for why the bound is not optional.

- **No lock is introduced.** The ring is a few atomics plus per-slot atomics. No `Mutex`, no `RwLock`,
  no allocation, and no reentrancy — `stage` and `drain` never call `serial_println!`.
- **WEDGE-2 / WEDGE-4 breadcrumbs are untouched.** Those primitives write single bytes through
  `arch::serial::wedge2_raw_byte` and deliberately acquire *nothing* — that is the entire reason they
  exist, since every console/video/allocator lock is reachable from the chain they instrument. They do
  not enter `serial_ring` at all, and `serial_ring` adds no lock they could contend for. The x86
  breadcrumb body moved verbatim into the shared `raw_byte` primitive; same bounded LSR poll, same
  single `out`.
- **The panic path never touches the Mutex.** The `#[panic_handler]` calls
  `serial_ring::enter_panic_mode()` before its first print. This is strictly better than the old
  behaviour, where a panic striking mid-print lost the `try_lock` *to its own core* and dropped the
  entire panic message — a red screen and silence.
- **Every wait is bounded.** Two spins exist in the path and both carry a hard ceiling: the arch's
  TX-ready poll, so a machine with no UART degrades instead of hanging, and SERWIT-1B's ring-full
  backpressure, so a print from a context that interrupted the UART holder degrades to the old
  drop-and-count instead of spinning on a lock that can never be released.

## Ordering

The drain runs *before* the holder writes its own line, so a line staged at t0 is always emitted ahead
of a line submitted directly at t1 > t0. The drain stops at the first claimed-but-not-yet-published
slot rather than skipping it — skipping would reorder the wire. The one unordered window is a few
instructions wide and involves two genuinely concurrent lines; nothing is lost either way.

## Accounting and the conservation law

Five counters, all `Relaxed` (they gate diagnostics and order nothing):

```
SUBMITTED == EMITTED + DECLINED + DROPPED + in_flight()      and, always, DROPPED == 0
```

`SUBMITTED` counts every `_print`; `EMITTED` counts every submitted line that reached the UART;
`DECLINED` counts lines the 16550 path refused because this machine has no 16550 (see SERWIT-1D below);
`DROPPED` counts ring-full losses; `STAGED` counts deferrals (a deferred line is *not* a lost one, and
the two must never be conflated). The `[serial] dropped …` marker is deliberately **not** counted in
`EMITTED` — it was never submitted by anyone, and counting it would corrupt the law in exactly the runs
where the law matters.

## SERWIT-1B — the ring-full branch was reachable on every single run

The `Ring full` row above used to end at "the line is lost". That branch was believed unreachable in
practice — the module said so: *"a ring that is drained on every uncontended print is a ring that
essentially never reaches the full-and-must-drop state"* — and on the x86 bench nothing could ever
prove otherwise, because that machine has no 16550 and never stages a line at all (SERWIT-1D). It was
`./arroyo test`'s x86 leg learning to read its own log (`arroyo/test`, 2026-08-06) that exposed it. The
very first honest run:

```
:: SERWIT-1: FAIL — uart16550=present carrier=16550@0x3F8 sent=125 (want 125) dropped=19 truncated=0
   submitted=126 emitted=107 declined=0 inflight=0 balanced=true law(declined==0)=true ::
```

Read which clauses held. `sent == want`. `truncated == 0`. The conservation law **balanced**. The
configuration clause held. The accounting was perfect and nineteen lines were simply gone. Across four
runs the figure was 19–36 and tracked host load.

### It is a rate problem, not a size problem

A producer formats a whole line into a slot with one `memcpy`. The consumer pushes that line through a
16550 a byte at a time — under TCG, some thousands of times the cost. Five cores in the SERWIT-1 burst
therefore fill *any* finite ring and then overflow it by exactly `produced − consumed` for the window.

Sizing the ring to `(cores − 1) × burst` would make this fixture fit, and would turn the ring into a
measurement of the fixture rather than a transport: the next burst one line longer drops again. **A
producer that outruns its consumer without bound must be slowed, or it must lose data.** There is no
third option and no depth that buys one.

### What the fix is

Backpressure at the producer, with three properties that keep it inside the rules the rest of the
transport is built on:

- **Bounded, always.** `BACKPRESSURE_SPINS` turns, then the line is dropped and counted exactly as
  before. The bound is a correctness requirement, not a tuning knob: a print arriving from an exception
  handler that interrupted *this* core inside its own UART-locked region would otherwise spin on a
  holder that can never release, and an unbounded wait there is a hang. `dropped > 0` remains a FAIL.
- **It waits by working.** Each turn re-tries the UART, so winning the lock means draining the whole
  ring and writing the line intact — the stalled producer becomes the consumer. That is also what makes
  it livelock-free at the tail of a burst, where a pure *wait-for-room* would spin out its bound and
  drop every queued line because the last holder has stopped printing for good.
- **It costs nothing when the ring is not full.** The loop's first turn is the pre-existing
  uncontended/contended path verbatim. `STALLED` and `STALL_SPINS_MAX` count the exceptions, and the
  verdict prints both, so a green run states on the wire whether the backpressure was *exercised* or
  merely present — a fix that cannot be seen working is indistinguishable from a run that got lucky.

The panic path returns before reaching any of this, so the deadlock analysis below is unchanged.

### The falsification

| Build | Ring | Backpressure | Result |
| --- | --- | --- | --- |
| pre-fix | 64 | none | `dropped=19` … `-> FAIL`, `./arroyo test` exit 1 |
| fix | 64 | 1,000,000 | `dropped=0`, `stalls=16..31`, deepest `72..304` turns, `-> PASS` ×4 |
| scratch A | 64 | **0** | `dropped=20 … stalls=0 maxspin=0/0` → `-> FAIL`, exit 1 |
| scratch B | **4** | 1,000,000 | `dropped=0`, `stalls=111`, deepest `721` turns, `-> PASS` |

Scratch A proves the gate still reds on loss and that the backpressure — not a witness edit — is what
turned it green. Scratch B is the one that proves the diagnosis: with the ring cut by 16×, to a depth
that could not hold even one core's worth of the burst, the run is **still** green. Ring depth is no
longer load-bearing for correctness; it only sets how often a producer has to push back. Both scratch
edits were reverted and the two source files byte-verified against their pre-falsification hashes.

Boot timing is unaffected: `BPACE total gui=1834ms` before, `1754ms` after.

## SERWIT-1D — the law counted a transport this machine does not have

SERWIT-1 printed `FAIL` in **every capture ever taken on the x86 bench**, going back to gr7. PASS = 0,
everywhere:

```
:: SERWIT-1: FAIL — sent=150 (want 150) dropped=0 truncated=0 submitted=151 emitted=0 inflight=0
   balanced=false ::
```

Read the numbers: `sent` matched `want`, `dropped` was 0, `truncated` was 0. **Nothing was lost.** The
150 lines went out perfectly well — over the FTDI cable, as SERWIT-2's own tap confirms
(`tap ftdi: submitted=1024 absorbed=1024 … dropped=0`, `-> PASS`). The witness was structurally
unpassable on that machine, and the reason is mechanical:

* a 2012 rMBP has no 16550 at 0x3F8, so `SERIAL1` holds `None`;
* `note_emitted` sits inside `if let Some(uart) = guard.as_mut()`, so it is unreachable;
* the staging branch is guarded by `UART_STATE != 2`, so with the port known-absent nothing is staged
  either — correctly, since nobody would ever drain that ring;
* the ring therefore stays empty, `drain` counts nothing, and `EMITTED` never leaves 0.

The old three-state law convicted the machine for lacking hardware it does not use. **That is a worse
defect than the one the witness guards against**, because an instrument red in every boot forever trains
every reader to skip `FAIL` lines — and this tree has already lost a genuinely broken `[wc-d]` verdict
(two boots, unexamined) and a two-week panel regression (`AT-RISK` printing every boot) to exactly that
habit. Permanently-red instruments are camouflage for real failures.

### The fix is a fourth terminal state, not a relaxed law

A line handed to a transport that does not exist on this machine ended somewhere that is none of the
three existing outcomes. It is not `EMITTED` (no byte reached a 16550, because there is none); it is not
`DROPPED` (nothing was lost — the line is on the wire via the FTDI mirror, whose own conservation law
SERWIT-2's `ftdi` tap asserts independently); it was never staged, so it is not in flight. It is
`DECLINED`: refused by policy, not lost — the same distinction `TapCounters` already draws with
`suppressed`, for the same reason.

Two `_print` sites charge it, and one of them had **no counter at all** before this change — the
contended-and-no-UART branch, a line leaving `SUBMITTED` with no matching term, which is precisely the
shape of hole this module exists to make impossible. The other site replaced `drain(|_| {})` with
`discard_staged()`, fixing a second latent lie: `drain` charges `EMITTED` for every line it consumes, so
lines thrown into a `|_| {}` sink were being counted as having reached the wire.

### Why this is not a weakening

On a machine that HAS a 16550, `UART_STATE == 1` and both declining branches are unreachable — they are
guarded by the same single fact, `guard.is_some() == false`. `DECLINED` is provably 0 there, **the
verdict asserts it is 0 there**, and the equation reduces term for term to the law it replaced. No
configuration that could fail before can pass now.

What is new is a *configuration clause*, asserted on top of conservation, which gives each machine the
term the other one cannot check:

| configuration          | clause asserted | what it catches                                        |
|------------------------|-----------------|--------------------------------------------------------|
| 16550 present          | `declined == 0` | lines silently withheld from a UART that works         |
| 16550 absent           | `emitted == 0`  | a line charged to "reached the wire" that reached nothing |

Both verdict lines now name the transport and the clause, because "balanced over a real UART" and
"balanced over the FTDI cable because there is no UART" are different facts and a reader holding only
the log must be able to tell them apart:

```
:: SERWIT-1: contended serial [uart16550=absent carrier=ftdi-mirror law=emitted==0] — 150 lines sent
   (incl. 6 wide-line probes at ~1287B), 0 deferred to the staging ring, 0 dropped, 0 truncated,
   accounting balanced (submitted=151 emitted=0 declined=151 inflight=0) -> PASS ::
```

### The go-red paths are checked by the compiler

The law is pure integer arithmetic over a `SerwitTally`, so it is `const`-evaluable, so its truth table
is pinned in the build as `const _: () = assert!(…)` — fifteen rows, evaluated on every `./arroyo check`
on both arches, emitting not one byte of code. A witness that cannot go red is strictly worse than the
permanently-red one it replaces, because it *looks* like evidence; the truth table is what stops the
second failure from quietly replacing the first. Mutation-checked, by extracting the marked block and
compiling it with `--emit=metadata`:

* `passes() := true` → rejected (`uart present, 7 lines vanished uncounted: must FAIL`)
* `balanced()` with the `DECLINED` term removed → rejected (the no-16550 PASS row stops holding)
* `config_law() := true` → rejected (both configuration rows fire)

## The SERWIT-1 witness

`crates/kernel/src/serial_ring.rs` + the `serwit1_run` driver in `crates/kernel/src/main.rs`, in the
tree's existing U\*x/witness idiom and gated behind the `witness` battery like every other fixture.

One kernel worker per online AP, all parked on a release gate so their bursts genuinely overlap, each
printing `[serwit] c=<core> n=<seq>` back to back with no yield. The BSP waits bounded on `ticks()`,
then asserts the conservation law across the stress window.

Two independent proofs come out of one run, which is the point:

1. **In-kernel** — the law balances with `dropped == 0`, and this machine's configuration clause holds
   (SERWIT-1D). This is what the `-> PASS` is made of.
2. **On the wire** — every line is sequence-numbered, so the log falsifies the counter from outside:
   ```
   awk '/\[serwit\]/' target/serial.log | sed 's/.*\[serwit\]/[serwit]/' | sort -u | wc -l
   ```
   must equal cores × burst. A counter that only ever agreed with itself would prove nothing.

Verdict line:

```
:: SERWIT-1: contended serial [uart16550=present carrier=16550@0x3F8 law=declined==0] — 125 lines sent
   (incl. 5 wide-line probes at ~1287B), 96 deferred to the staging ring, 22 back-pressured on a full
   ring (deepest 72 of 1000000 turns), 0 dropped, 0 truncated, accounting balanced
   (submitted=126 emitted=126 declined=0 inflight=0) -> PASS ::
```

On a UART-bearing machine the `deferred` figure is the load-bearing one: it says three quarters of the
fixture's lines took the `try_lock`-failure branch, i.e. the branch that used to discard them. A run
reporting `deferred=0` there would mean the fixture failed to contend and proved nothing. The
`back-pressured` figure is SERWIT-1B's: non-zero means the ring genuinely filled and the bounded retry
is what kept `dropped` at 0, so the mechanism is being *exercised* by this run and not merely present
in it; `deepest N of 1000000` is the margin left under the bound. On a machine
with **no** 16550 the deferral branch is correctly disabled (nobody would drain the ring), so
`deferred=0` is the expected reading and `declined` is the figure that carries the traffic — see
SERWIT-1D.

**Acceptance criterion.** The gate's `PASS` tally must be *identical* across consecutive runs — that
stability, not the absolute number, is the property being defended. Six consecutive
`UNAOS_WC=1 ./arroyo test 45` runs on this change: 37 PASS / 0 FAIL every run, 72/72 distinct
`[serwit]` lines every run, zero drop markers.

## SERWIT-2 — the four mirror taps

`_print` does not write to one sink, it writes to five. SERWIT-1 fixed the primary wire; hanging off
the same seam are four **mirrors**, every one of which had the identical `try_lock`-and-discard shape
with no counter anywhere:

| Tap | File | Sink | Verdict |
|---|---|---|---|
| `fbcon` | `video/fbcon.rs` | the on-screen console | **legitimately lossy — counted and announced** |
| `ftdi` | `drivers/xhci/ftdi.rs` | the FTDI cable (the bench's own capture) | **fixed** — staged, no loss |
| `tste` | `selftest.rs` | the boot-verdict replay ring | **fixed** — lock removed entirely |
| `flightrec` | `flight_recorder.rs` | `UNAOS.LOG` | **fixed** — staged, no loss |

The aggravating fact: all four run **outside `SERIAL1` and outside the interrupt mask** (the arch
`_print` calls them after the locked region), so they are contended by every core at once — including
on the very lines the primary wire is busy deferring. They were not a rarer instance of the defect;
under a multi-core burst they were a worse one.

### Not every tap owes the same thing

`try_lock`-and-never-block stays the rule everywhere: a mirror that could block would be able to stall
the primary wire, and that inversion is worse than the drop it would replace. What changed is the
failure branch, and the right answer differs by tap.

* **Sinks whose content is evidence** (`ftdi`, `flightrec`, `tste`) must not lose lines at all. Their
  sinks are cheap — a memcpy into a byte ring, a 42-byte record — so the SERWIT-1 discipline
  transplants directly: defer into a lock-free `LineRing` (a generic form of the staging ring) and let
  the next holder drain it, in order, before writing its own line. `tste` goes further and drops its
  `Mutex` outright: a fixed array of fixed-size records needs only an atomic index claim, so there is
  no lock left to lose and no contention-loss path at all.
* **Sinks that are a view** (`fbcon`) stay lossy on purpose. Painting a deferred backlog would put
  glyph work inside the masked, locked critical section PANEL-DEFER exists to keep short, and the line
  is on the wire regardless — the panel is the one sink whose loss costs no evidence. Its obligation is
  the weaker half of the law: every miss is counted and announced, on the panel (`[fbcon] N line(s)
  missed the panel`) and on the wire (`[mirror] fbcon: …`).

fbcon also has a second, subtler loss the split paint path introduced: if the **first** chunk of a line
wins the console lock and a **later** one loses it, the panel shows a line that stops mid-word with no
indication that it does. That is counted separately as a `torn` line and announced like a drop.

### Accounting

Each tap keeps a `TapCounters` ledger and satisfies

```
submitted == absorbed + dropped + suppressed + in_flight   (± the sampling window, see below)
```

`suppressed` is a line the tap **declined by policy** — the GUI owns the panel, quiet-panel is in
force, the line is not a verdict line. Separating "declined on purpose" from "lost" is the whole point:
lumping them would make a silently-lossy tap indistinguishable from a correctly-quiet one.

The six counters cannot be read as one instant without a lock, and a lock on the print path is the one
thing this work forbids. So a snapshot taken while another core sits between its `submit()` and its
outcome shows that line as unaccounted. **At most one such line can exist per core**, so the tolerance
is not a fudge factor — it is the core-count ceiling (`MIRROR_WINDOW = 64`). A sampling artefact is
bounded and vanishes on the next sample; a genuine accounting hole grows without bound with traffic.

### Announcement channels

Each tap announces through **its own** channel, because the reader who needs to know a tap lied is the
reader of that tap — and they are different people:

* `ftdi` injects `[ftdi] N console line(s) lost to contention` **into the capture stream**. On the 2012
  rMBP there is no 16550, so on an attended metal sitting the cable is not a mirror of the evidence, it
  *is* the evidence; a counter the sitting never sees would be worthless.
* `fbcon` paints its marker on the glass.
* `flightrec`'s byte-drop note already goes into `UNAOS.LOG`.
* `tste`'s ring-full count is already printed by `run()`.
* All four are *also* announced on the wire by `serial_ring::mirror_service()`, polled from the x86
  main loop. It is self-rate-limiting (the pending counter is swapped to zero by the announcement), so
  a healthy tap prints nothing at all.

### The SERWIT-2 verdict

Emitted once, on entry to the main loop, after the boot fixtures — SERWIT-1's own multi-core burst
included — have run through all four taps under real contention. PASS requires the conservation law to
balance on **all four** taps and zero loss on the **three evidence** taps. `fbcon`'s misses are
reported but not fatal: the property being proven for a view is that a miss is visible, not that it
never happens.

## SERWIT-2W — the slot width was a guess, and it was wrong

The aarch64 seat gated SERWIT-1 into their tree and immediately saw `truncated 1`: their F3/K1 witness
lines run past 340 characters and the 240-byte slot clipped them. **A truncated verdict line breaks an
`awk` tally exactly as badly as a lost one**, so on that tree the law was not actually held — the
transport was still corrupting evidence under contention, just more quietly than before.

So the width was measured. All 2786 `serial_print!`/`serial_println!` format strings in the kernel were
reconstructed (`\`-continuations joined, placeholders charged a pessimistic 20 bytes each):

```
  > 240 chars: 264 format strings   ← 9.5% of the tree truncated at the old width
  > 340 chars:  97                  ← the aarch64 seat's floor is NOT the maximum
  > 512 chars:  21
  > 768 chars:   6
  > 896 chars:   1
  measured maximum: 1291 chars      arch/aarch64/v3d.rs, the v3d59 audit note — a pure
                                    literal, so this is exact, not an estimate
```

1291 + newline = **1292 bytes of true worst case**. The chosen width is **1536**: 244 bytes (19%) of
headroom over it, 655 over the entire rest of the tree.

* **Not 1024** (which would cover 2785 of 2786): it would leave exactly one line truncating on every
  boot that enables it, and a truncation counter that is permanently non-zero for a known-benign reason
  is a counter people learn to ignore. At 1536 the counter reads 0, so any non-zero reading is news.
* **One width, not per-arch**: the >340 population spans both arches — `v3d.rs` and `rtl8168_tegra.rs`
  on aarch64, `video/wcf.rs`'s `[wc-f]` scanout rollups and `arch/x86_64/syscall.rs`'s S9 verdicts on
  x86. Two per-arch numbers would each be sized against one seat's lines and silently wrong for the
  other's, which is the precise failure being closed. Both seats read one number.
* **Cost**: 64 × 1536 = 96 KiB of `.bss` per staging ring × 3 rings (wire, FTDI, recorder) = 288 KiB,
  uniform on both arches, present on metal.
* **The headroom is margin, not slack — do not shrink it.** Line lengths grow with the tree: the
  415-byte uvug6 verdict that made 1024 untenable was extended by an arc landed the SAME DAY this
  width was chosen (PAL-TYPEMATIC), and the 200-byte tste scan window it also broke had hidden five
  verdict families from every replay in this tree's history — no drop marker, no truncation, correct
  on the wire, absent from the record. A future pass that sees 288 KiB of `.bss` and trims the width
  re-opens exactly that failure mode, one witness extension at a time.

Truncation is still counted, still announced, and now **self-evident on the wire**: a clipped line has
its tail overwritten in place with `…⟨SERWIT-2W: line truncated here⟩`, so a human reading a capture
cannot mistake a cut line for a complete one. Overwriting rather than appending is deliberate — a
marker that only appeared when there happened to be room would be absent exactly on the longest lines.

Two silent-truncation bugs of the same family were found and closed while sizing this:

* `flight_recorder::capture` formatted each line into a **256-byte stack buffer** before copying it in,
  so `UNAOS.LOG` was quietly clipping the widest diagnostics it exists to preserve. The buffer is gone;
  `LogRing` is the `fmt::Write` sink now, so lines are formatted straight into the ring, whole.
* `selftest::capture` formatted into a **200-byte** buffer and then searched it for `-> PASS`, with the
  comment "the verdict marker is early in the line". The marker is at the **end** of the line, by the
  tree's own convention. Any verdict line over 200 bytes therefore had its marker chopped off before
  the search ran, and the fixture was **not recorded at all** — not dropped-and-counted, not truncated,
  simply absent from `tste`'s table as if it had never executed. Replaced with a streaming scanner with
  no width limit.

### The SERWIT-3 leg

Every SERWIT worker now also emits one line at the widest realistic size (1287 bytes — within five
bytes of the measured worst case) **through the contended path**, end-sentinelled:

```
awk '/SERWIT3-END/' target/serial.log | wc -l     # must equal the worker count
```

Truncation takes the tail, so a clipped probe loses its sentinel by construction. The in-kernel
assertion (`TRUNCATED` delta == 0 across the stress window) and the on-the-wire sentinel count are
independent, for the same reason SERWIT-1 sequence-numbers its burst.

## `UNAOS.LOG` — the complementary capture channel

The `flightrec` tap in the SERWIT-2 table is not just a fourth mirror to keep honest. It is a **second
capture channel**, and on this bench it is the only one that holds the head of a boot. This section is
how to read it, and the one check that must be run before any of it can be believed.

### It keeps the earliest bytes, on purpose

`crates/kernel/src/flight_recorder.rs` — `RING_CAP = 64 KiB` (`:86`). `LogRing::append` (`:97-110`)
computes `room = RING_CAP - self.len` and, when `room == 0`, adds to `dropped` and **returns without
writing**. There is no head eviction anywhere in the type.

```rust
let room = RING_CAP - self.len;
if room == 0 {
    self.dropped = self.dropped.saturating_add(bytes.len());
    return;
}
```

That is the exact opposite of the FTDI mirror (`drivers/xhci/ftdi.rs`, `Ring::push_byte` — "drop-oldest
on overflow"), and the opposition is the point:

| channel | on overflow | what survives | what is lost |
|---|---|---|---|
| FTDI mirror → the capture file | drop-**oldest** | the tail, up to console-up and everything after | the pre-console head |
| flight recorder → `UNAOS.LOG` | drop-**newest** (stop and count) | `t=0` forward, until the ring fills | everything after the ring is full |

Neither channel alone covers a boot on this machine. Together they overlap heavily, and the overlap is
what lets a reader stitch them with confidence rather than by hope.

**The file is self-describing about its own truncation.** `capture` (`:218-224`) appends the drop note
only when `dropped > 0`, and always closes with the end-of-log marker (`:229`):

```
:: FLIGHTREC: 9646 byte(s) dropped (ring full / contended) ::
:: FLIGHTREC: end of log (65536 captured byte(s); the remainder of this 66048-byte file is reserved padding) ::
```

`RESERVE_BYTES = RING_CAP + 512 = 66048` (`:247`) is why every saved copy on the bench is exactly that
size, and why everything past the end marker is NUL padding rather than log.

### Proven recovery — s67 and s68

Both boots' serial capture (`capture/rmbp-s66-cand444/ttyUSB0.log`, four boots in one file) contains
**zero** `x86 fb-wc` and **zero** `X86_64 Memory Init`. Their `UNAOS.LOG` copies contain both, plus
`SMEP on`, `KERNEL HEAP ALLOCATED`, the `DMAR: IOMMU present …` line, `clock: TSC calibrated`, and
`SMP: starting APs`. The head was on the card the whole time.

Aligning each copy against its own boot's segment of the serial file, line for line:

| | s67 | s68 |
|---|---|---|
| `UNAOS.LOG` log content runs to | line 1044 | line 1042 |
| serial replay's first line corresponds to `UNAOS.LOG` line | 53 | 73 (mid-line — the replay starts inside it) |
| lines recovered that the serial never had | **~52** | **~72** |
| overlapping lines available to stitch on | **~992** | **~970** |
| bytes the recorder itself dropped, at the tail | 9646 | 10983 |

The two loss figures are the model working: the mirror lost tens of lines off the head, the recorder lost
~10 KB off the tail, and roughly 970–990 lines are present in both.

> ⚠ **Do not use a line's position in `UNAOS.LOG` as the replay boundary.** `RWLOCK: [cpu7] done 5/5,
> torn=false, max_concurrent_readers=3 => PASS` sits at line **299** in both copies and is a good
> alignment anchor once you have found the boundary — it is a single, distinctive, once-per-boot line —
> but it is nowhere near where the replay begins. Find the boundary by walking back from the anchor:
> establish the constant offset between the two files at the anchor, then find the first serial line of
> that boot's segment. The offset drifts by a few lines across a boot (the channels do not carry an
> identical line set), so re-check it near the boundary rather than extrapolating from one point.

### The cross-match is mandatory, and this is the trap that makes it so

`reserve_log` (`:285`) has three cases, and the first one is the hazard (`:290-295`):

```rust
Ok((de, _dl, _doff))
    if de.size as usize >= RESERVE_BYTES && de.first_cluster() >= 2 =>
{
    // Big enough already: reuse the existing chain in place. NO FAT/dir mutation whatsoever.
    return Ok((de.first_cluster(), de.size, true));
}
```

An already-large-enough `UNAOS.LOG` is **reused untouched**, and `PAD_NEXT` only clears the stale tail on
the first *successful* flush. So a boot that never reaches storage — it panicked early, it wedged before
the main loop, the card was pulled — leaves the **previous** boot's log on the card, at the right size,
with a well-formed header and a well-formed end marker. **It is structurally indistinguishable from a
fresh one.** Nothing in the file says which boot wrote it.

This is not hypothetical. The bench archive already carries the failure:

* `capture/s62-s65-UNAOS.LOG.saved` — one file named for four sessions. Its `hz=2693855145` matches the
  **sixth and last** of the six boots in `capture/rmbp-s62-probe/ttyUSB0.log`. The other five have no
  saved copy at all.
* `capture/rmbp-s62-probe/UNAOS.LOG.s62` — same session, `hz=2693856980`, which is the **first** of those
  six boots. Two copies from one session, each a different boot, and only the cross-match says so.
* `capture/s71-UNAOS.LOG.saved` — `hz=2693851785`, which is the **second** of the three boots in
  `capture/rmbp-gr15-s70/ttyUSB0.log`, not the third (`hz=2693849494`).

**The rule, non-negotiable: cross-match `hz=` between the `UNAOS.LOG` copy and the serial capture before
attributing a single line of it.** The raw TSC calibration figure is unique per boot and is printed into
both channels, in the `EPACE`/`GPACE`/`BPACE` ledger lines:

```
awk '/hz=[0-9]/' <UNAOS.LOG copy>                 # one value; that is the boot the file is
awk '/hz=[0-9]/{print NR": "$0}' <serial log>     # segments the serial file by boot
```

Observed values, all distinct, all on the same machine: `2693845865` (s61), `2693846860` (s66),
`2693849020`, `2693849494`, `2693849905`, `2693851785`, `2693853305` (s67), `2693853945` (s68),
`2693855145`, `2693855465`, `2693856025`, `2693856745`, `2693856980`, `2693857105`.

**`clock: TSC calibrated ~2693 MHz (invariant)` is NOT the discriminator.** It is rounded to the MHz and
is byte-identical in every capture on this bench. Only the ten-digit `hz=` separates boots. A filename is
not evidence either — every example above is a file whose name disagrees with its contents.

### Honest scope — what this channel is still for

`0b66d9cd` raised the FTDI mirror's `CAP` from 64 KiB to 256 KiB (`drivers/xhci/ftdi.rs:93`), so the
serial capture should now carry the head itself. **On the evidence available, that is compiled and not
yet metal-proven** — every boot in the archive predates the change. Until a capture shows otherwise,
`UNAOS.LOG` remains the recovery path for a boot-blind head.

Two claims about it are commonly overstated and are wrong:

* **It is not "the only record for boots where FTDI never comes up".** `ftdi=none` in a `UNAOS.LOG` copy
  is a *snapshot*, not a verdict. The `BPACE` ledger prints repeatedly, and the early prints (`n=22`,
  `n=24`) are emitted before the console opens, so they read `ftdi=none` by definition. The same boots'
  serial shows the later prints carrying the real figure — s67 `ftdi=21743ms`, s68 `ftdi=21425ms`, s70's
  three boots `29472ms` / `22126ms` / `20390ms`. FTDI came up in **every** boot in the archive; it came up
  *late*. The recorder's ring is simply full before the ledger line that would have said so. The honest
  basis for keeping this channel is narrower and sufficient: *it is the only record of the pre-console
  head on boots whose pre-console volume overflowed the mirror.*
* **Its coverage is not the whole boot.** Every reserve-era copy reports exactly `65536 captured byte(s)`
  — i.e. the ring is **always** full, on every boot, and always stops. It covers `t=0` forward to roughly
  **8–22 s** depending on how long that boot took to reach the GUI (the `BPACE total gui=` figures inside
  the rings run 7828 ms to 21575 ms), and nothing after. It is a boot-head channel, not a session log.

Finally: **the archive offers no before/after on a pre-2026-07-21 state.** The earliest saved copy is
`capture/rmbp-r23s6/UNAOS.LOG.prior-boot` (2026-07-22, 29,025 bytes — pre-reservation, with no end-of-log
marker); every other copy is from 2026-08-02 or later. Any question of the form "what did this line read
before the regression landed?" cannot be answered from this channel.

## DRAINCAP — one drain has a BYTE budget, because the UART charges in bytes

This section is SO29's, and SO29 is the one finding in this file that was measured on the GLASS rather
than on the wire. Peter, render13: *"mousing/dragging not smooth"*.

### The defect

The holder of the UART drains the staging ring **before** writing its own line. That ordering is
correct and stays (see *Ordering* above). What was wrong is that the drain was bounded by SLOT COUNT
only — `guard > SLOTS` in `drain_into` — and a slot count is not a cost.

The cost is bytes, because the consumer moves one byte at a time. 115200 8N1 is ten bits per byte:

```
    11 520 B/s  ->  86.8 us per byte
    64 slots x 68 B (the measured width of a witness line) = 4 352 B  ->  377.8 ms
```

377.8 ms of **IRQ-masked, UART-locked, inline** work, charged to whichever core happened to print
next. On render13 boot 1 that core was the compositing core, and the drag-stall band reads

```
    [comp2] rollup pass_us≈1835 max_us=375061..617554      (121 spike rollups of 166)
```

`375 061 us` is 4 321 bytes and `617 554 us` is 7 114 bytes: the whole spike band is one ring-drain
wide, to 0.7 %. The compositor was not slow. It was paying for the wire. Four other suspects were
excluded arithmetically first — see `docs/dev/LEDGER.md` SO29 and commit `0fc32a4c`.

### The fix, and why it is a byte budget and not a smaller ring

`DRAIN_BYTE_BUDGET = 192` bytes, in `serial_ring.rs`, applied by `drain_capped` — the spelling both
arches' `_print` now use.

**Why bytes.** Shrinking `SLOTS` would trade the stall for loss, and SERWIT-1B already settled that
depth is not load-bearing for correctness (scratch B: ring cut to 4, still `dropped=0`). It would not
bound the cost anyway: one 1536-byte line costs more than twenty 68-byte ones, so a slot bound says
nothing about time. The budget is the honest unit.

**Why 192.** One frame of UART at the cadence the compositor is trying to hold: a 60 Hz frame is
16.667 ms, and 11 520 B/s x 0.016667 s = **192.0 B**. That is the *whole* frame, so it is a ceiling
and not a target — a pass that spends its entire frame on the wire has composited nothing.

> ⚠ The SO29 row's parenthetical *"~16 ms = ~1,900 bytes"* is an arithmetic slip: 1 900 B at
> 86.8 us/B is 164.9 ms, ten frames, not one. 192 B is the figure the row's own rate produces. Both
> numbers are stated here so the next reader does not have to re-derive which is right.

**The bound it actually gives.** The loop emits while `bytes_so_far < DRAIN_BYTE_BUDGET`, so the last
line taken may straddle the budget: one drain pays at most `budget - 1 + <widest line drained>`. It
has to be that way round. A drain that refused to start a line it could not finish inside the budget
would never emit a line wider than the budget at all, and `SLOT_LEN` is 1536 — the widest evidence
lines in the tree would sit in the ring forever. **At least one line always leaves.** In render13's
shape that is `192 + 68 = 260 B = 22.6 ms` against `377.8 ms`: a **16.7x** cut, and what is left is a
property of the line width rather than of the ring depth.

### Nothing is lost by capping

The remainder stays in the ring, in order, and rides the next print — and the ring is drained on
every print, by every core, so "next" is soon and is conditional on nothing. **A capped drain changes
WHEN a staged line reaches the wire, never WHETHER.** That is the SERWIT-1 law restated, not an
exception to it, and it is checked two independent ways:

* the DRAINCAP fixture accounts for every line its fill staged across the capped drain plus the next
  one, with `DROPPED` unmoved;
* SERWIT-1's conservation law is the outside check — a byte cap that lost a line would break
  `SUBMITTED == EMITTED + DECLINED + DROPPED + in_flight()` and turn that verdict red on its own.

**Only the two `_print` hot paths take the cap.** The panic path and the power verbs call the uncapped
`drain`, and must: a dying machine owes its reader every staged byte and there is no next print to
ride. `arch/x86_64/acpi_power.rs`'s reboot ladder likewise keeps the uncapped spelling it already had.

### The fixture and its go-red

`serial_ring::draincap_selftest`, one-shot, riding `mirror_service` — the same call site the SERWIT-2
verdict uses, for the same stated reason (IRQs unmasked, no locks held, not a print context) and
reached on **both** arches. It empties the ring, fills it with 68-byte lines, takes one capped drain
and one uncapped drain, and asserts: bounded, capped, and lossless.

```
:: DRAINCAP: one drain is bounded by BYTES — staged 64 x 68 B, the capped drain paid 204 B in 3
   line(s) (budget 192 B, ceiling 260 B = budget + one line), the remaining 61 line(s) / 4148 B rode
   the next drain, 0 lost. Uncapped this ring is 4352 B = 377753 us of IRQ-masked UART at 86.8 us/B
   (SO29) -> PASS ::
```

Go-red: give `drain_capped` a `usize::MAX` budget and the fill drains whole in one pass —
`capped_paid=4352B (ceiling 260B) capped_lines=64` — and both the byte clause and the
cap-took-effect clause fire.

**Every PASS clause is one-sided on purpose.** The fixture uses the LIVE ring, because the live ring
is the thing under test; a private `LineRing` would test a copy of the code. The price is that another
core can win the UART between the two drains and take lines the fixture staged. So interference can
only make the measured byte count *smaller* and the capped line count *fewer* — it can hide a
regression on an unlucky run, and can never invent one. That direction is deliberate: a gate that can
false-fail gets deleted the week it flakes.

The budget predicate itself is `const`-evaluable, so its go-red rows are pinned in the build like
SERWIT-1's law — five `const _: () = assert!(…)` rows, both arches, every `./arroyo check`, emitting
not one byte of code. The first row is the load-bearing one: *an empty drain must always take its
first line, whatever its width.*

## SERWIT-1B PARITY — the backpressure was x86-only, and aarch64 dropped on the first turn

SERWIT-1B (above) installed the bounded, progress-bearing retry that stopped a full ring from being
terminal. It was installed in **x86's `_print` only**. `arch/aarch64/serial.rs`'s `_print` called
`serial_ring::stage`, which tried the ring once and, on a full ring, wrote the line off on the spot.

That asymmetry is visible on the wire, and it went unread for weeks because the accounting was
*correct*. render13 boot 1, on the Orin:

```
    [serial] dropped 5331 lines in 192 events          — counted, announced, law balanced
    stall count: 0                                     — the mechanism that would have saved them
```

Five thousand three hundred and thirty-one lines that the other arch would have kept. Nothing lied;
the transport simply had two different contracts wearing one name, which is the failure this whole
document exists to close. (The producer that filled the ring is a separate finding, SO30, and is
fixed — but a transport is not allowed to depend on its producers being polite.)

### One policy, and the old spelling is deleted

Both arches' `_print` now call `serial_ring::defer_contended`, and behind it sits one pure decision:

```rust
pub const fn defer_policy(staged: bool, spins: u32, limit: u32) -> Defer {
    if staged { Defer::Staged } else if spins >= limit { Defer::Lost } else { Defer::Retry }
}
```

`Staged` / `Retry` / `Lost` are the three outcomes, and the distinction between the first two *is* the
law: **a deferred line is not a lost one.** `stage()` is **deleted**, not deprecated. The go-red for
"an arch regresses to drop-instantly" must not be a reviewer noticing — it is the compiler refusing to
resolve the name. There is one spelling of the contended path left in the tree, so aarch64's `_print`
either calls it or does not compile.

Six `const _: () = assert!(…)` rows pin the policy in the build, both arches, every `./arroyo check`.
Row 2 is the defect, stated as an assertion: *a full ring on the first turn must back-pressure, never
drop.*

### The fixture

`serial_ring::backpressure_selftest`, one-shot on `mirror_service`, fills the ring and takes three
turns of the real `defer_contended` with one capped drain in the middle — the drain standing in for
SERWIT-1B's *it waits by working*, where the stalled producer wins the UART and becomes the consumer:

```
    turn 1   ring full, bound not reached   ->  Retry   (spins 1)
    turn 2   ring full, bound not reached   ->  Retry   (spins 2)
    [one capped drain: room appears]
    turn 3   room                           ->  Staged
```

```
:: SERWIT-1B: the contended producer BACK-PRESSURES, it does not drop — ring filled to 64 line(s),
   2 turns on a full ring returned Retry (spins=2 of 1000000), one capped drain freed 3 slot(s), the
   next turn DEFERRED the line intact, 65 line(s) out, 0 dropped. One policy (`defer_policy`) on both
   arches; `stage()` is deleted, so drop-instantly cannot be written -> PASS ::
```

**The `Lost` leg is not exercised at runtime, on purpose.** Firing it would leave `DROPPED` non-zero
and `[serial] dropped 1 lines` on the wire of a *healthy* boot, and this tree has already lost a real
`[wc-d]` verdict and a two-week panel regression to readers trained by a permanently-noisy instrument
(`docs/dev/LAWS.md` §5). The `Lost` rows are the compiler's.

Two go-reds, failing at different stages:

* **compile** — flip the `Retry` arm to `Defer::Lost`: truth-table row 2 refuses to compile and
  `./arroyo check` reds on both arches before anything boots;
* **runtime** — add `note_dropped()` to `defer_contended`'s `Retry` arm (a retried line counted as a
  lost one). It compiles; the fixture reads `dropped=2` and prints `-> FAIL`, which `FAULT_PATTERNS`
  (`FAIL — `) turns into a non-zero exit from `./arroyo test-arm`.

## The power verbs drain first — a deferred line needs a NEXT print, and a power verb has none

Trunk queue §1 (f); `LEDGER.md` SO31. Every guarantee in this document rests on one sentence: *a
contended line is deferred, and the next holder of the UART emits it.* That sentence has a
precondition nobody had written down — **there has to be a next holder.**

`power.rs`'s verbs announce through `_print` and then hand the machine to the firmware:

| verb | mechanism |
| --- | --- |
| `platform_shutdown` / `crystal_shutdown` (aarch64, non-Pi) | PSCI `SYSTEM_OFF` via `smc #0` |
| `platform_reboot` / `crystal_restart` (aarch64, non-Pi) | PSCI `SYSTEM_RESET` via `smc #0` |
| `platform_shutdown` (x86) | ACPI S5, `acpi_power::poweroff` |
| `platform_reboot` (x86) | FADT RESET_REG ladder, `acpi_power::reboot` |
| both (Pi 4) | honest witness, then `hlt_loop` |

The announce is the line most likely to be staged — a desktop Shut Down press happens with the
compositor printing — and the firmware call is microseconds behind it. On a flooded ring (render13
boot 1 was dropping 5 331 lines) everything still staged when `SYSTEM_OFF` lands dies with the power,
**including the verb's own witness**, and the operator who pressed the button is left with a dark
board and a capture that never says the verb ran.

### The fix

One uncapped full drain through the arch's raw lock-free writer, immediately before the firmware call,
then a witness. `serial_ring::power_drain(tag) -> (lines, bytes)`:

```
[pwrshutoff] ring drained lines=61 bytes=4148
```

Three properties, each load-bearing:

* **Uncapped.** `DRAIN_BYTE_BUDGET` (192 B) exists so a *composite pass* cannot be stretched by the
  wire. A machine that is powering off has no frame to protect and no next print to defer to; it owes
  its reader every staged byte. Same reason the panic path keeps the uncapped `drain`.
* **Bounded anyway.** `drain_into`'s slot guard bounds the loop at `SLOTS` + 1 iterations, so "every
  staged line" is a finite statement, and `raw_write_str`'s TX-ready poll is itself bounded — a machine
  whose UART never drains degrades rather than hanging the shutdown.
* **Raw, but the whole SINK SET** (SINKDRAIN, 2026-09-15). The witness must not be able to take the
  very branch it reports on, so this never re-enters `_print` and takes none of `_print`'s locks —
  but see the subsection below: until SINKDRAIN "raw" also meant *one arch port*, which is a far
  narrower claim than "everywhere `_print` goes", and on one board it meant nowhere at all.

`lines=0` and a missing line are different facts, which is the point of printing the count: an empty
ring says so, and a capture with no `ring drained` line at all says the machine died before this point.

The Pi arms get the drain too. They do not power anything off — they print an honest refusal and park
in `hlt_loop` — and a park is likewise a context with no next print.

> **On the 2012 rMBP this drain is the first of TWO buffers, and only the first.** `power_drain` (as RBTDRAIN read it — SINKDRAIN below corrects this: it wrote the raw port ONLY, never `_print`) ends
> in `serial::_print`, whose x86 sinks are the 16550 at 0x3F8 — a port that laptop does not have — and
> the FTDI MIRROR RING, which reaches the cable only when the xHCI device-service pass runs. So on
> that machine the flush above moved every staged line one buffer closer to a human and no further,
> and the reboot ladder still died in the ring. *RBTDRAIN*, at the end of this document, is the last
> leg.

### SINKDRAIN — the drain's sink is `_print`'s SINK SET, not one arch port

Trunk queue §5 (2026-09-15, RBTDRAIN's finding, measured on the cable); `rmbp-ledger` A3.

`power_drain` emptied the ring into `arch::serial::raw_write_str` — on x86 the 16550 at 0x3F8 and
nothing else — and wrote its own tally through that same single port. **`_print` does not write to
one sink.** On x86 it writes to the UART *and* to the FTDI mirror ring (`arch/x86_64/serial.rs`'s
`ftdi::mirror` tap), and the 2012 rMBP has no 16550, so that cable is the machine's only console. On
that board a line DEFERRED into the staging ring under contention was therefore CONSUMED by the power
verb into a port that does not exist — before any FTDI flush could carry it — and
`[pwrreboot] ring drained lines=N` never reached a human either. PWRDRAIN and S5DRAIN are both correct
and both a no-op for that reader: they prove the ring is EMPTIED and say nothing about WHICH SINKS
received it, and that second half had never been asserted anywhere.

RBTDRAIN measured the two facts one line apart in one run: the cable's tail carried
`[pwrreboot] reboot verb invoked …` and `[pwrreboot] ftdi flushed bytes=210 transfers=4 exhausted=0`
(both through `_print`), while `[pwrreboot] ring drained lines=0 bytes=0` appeared **only** in
`target/serial.log`. This is not rMBP-only in principle: any board whose console is not the arch's
raw port has it.

The fix is `serial_ring::sink_write(s)` — one already-formatted line to every sink `_print` reaches on
this arch, taking none of `_print`'s locks:

| arch | the sink set `sink_write` writes | why that set |
| --- | --- | --- |
| x86_64 | `raw_write_str` (the 16550, where one exists) **and** `xhci::ftdi::mirror` | both are in `arch/x86_64/serial.rs`'s `_print` |
| aarch64 | `raw_write_str` (PL011 / Tegra 16550) only | that arch's `_print` mirrors to fbcon and the selftest ring, never to this cable |

```
[pwrshutoff] ring drained lines=61 bytes=4148 mirror=ok
```

The ` mirror=` field is x86-only, and so is the leg that produces it; the aarch64 tally is the
pre-SINKDRAIN literal with the pre-SINKDRAIN argument count. `ok` means the mirror took every line of
this drain, `SKIPPED` means at least one could not be placed — a delta across the drain, never a boot
total, because a tally reading `SKIPPED` for a line lost an hour earlier is not actionable.

Three things make this safe on a path whose next statement is the SMC:

* **`ftdi::mirror`, not a private copy of its body.** That function already carries, audited, every
  discipline this call site needs: `try_lock` ONLY (a power path can never block on it, and the mirror
  can never invert against the primary wire), its own bounded fallback (stage into the tap's
  `LineRing`, then one free retry at the ring), and its own tap accounting, so a line it cannot place
  is COUNTED rather than lost in silence — which is what lets the tally say `SKIPPED` as a measurement
  instead of an assumption. A second spelling of a contended-sink policy is how two divergent policies
  happen; SERWIT-1B PARITY above is this module's own scar from exactly that.
* **It cannot loop.** `mirror` never calls `_print` and never touches the staging ring. A drained line
  re-submitted to `_print` would be re-staged into the ring it was just drained out of.
* **The raw UART still runs first, unconditionally.** Nothing that used to reach the wire stops
  reaching it; where there is no 16550 that leg is the bounded TX-ready poll it always was.

The mechanism is UNGATED — transport correctness, not an instrument, in every build of both arches
(the LOCKFIX / S5DRAIN / RBTDRAIN precedent). Only the witness is behind `witness`.

#### The fixture, and why PWRDRAIN could not have caught this

`serial_ring::sinkdrain_selftest`, x86, one-shot on `mirror_service` and last of the four. It stages
`SINKDRAIN_FILL` lines each carrying the token `SINKDRAIN-CABLE`, calls
`power_drain("sinkdrain-test")` with the firmware call left off, and then **asks the cable's ring what
it received** — `ftdi::peek_recent`, the capture ring's existing `try_lock`-only, non-consuming read
path, which returns the newest bytes. The fixture runs on the x86 BSP main loop, so no `drain_ftdi`
pass can intervene between the drain and the peek, and `None` from `peek_recent` is read as "busy"
and retried under a bound, never as "empty".

```
:: SINKDRAIN: staged=8 drained=8 on_cable=8 — a power verb's drain reaches `_print`'s SINK SET … -> PASS ::
```

It deliberately has **no `uart_absent()` SKIP**, unlike DRAINCAP, SERWIT-1B and PWRDRAIN. Those skip
because a machine with no 16550 never stages (SERWIT-1D) and they have nothing to exercise; this one
is the opposite case by construction, because the board with no 16550 is the board the defect is
about. It stages directly rather than through `_print`, so it runs identically in both configurations,
and on the rMBP it is the only fixture in this file that can go red for the right reason. The cost is
stated rather than discovered: a staged-and-drained line charges `EMITTED`, which SERWIT-1D's
`emitted == 0` clause forbids on that board — harmless, because `serwit_verdict`'s window closes during
the boot fixtures while this runs on entry to the main loop. PWRDRAIN and S5DRAIN charge `EMITTED` the
same way, and have since SO31.

**The go-red** is to delete the `ftdi::mirror` leg from `sink_write`, i.e. the tree exactly as it stood
before this change. It compiles; the lines and the tally still leave through the 16550 and still empty
the ring, so **PWRDRAIN and S5DRAIN stay green** — that they cannot see the mutation is the whole
reason this fixture exists — while SINKDRAIN reads `on_cable=0 tally_on_cable=false` and prints
`-> FAIL`, which mbench's `DEFAULT_FORBIDS` turns into a non-zero exit from
`UNAOS_USBSERIAL=1 UNAOS_WC=1 ./arroyo test`.

### The fixture

`serial_ring::pwrdrain_selftest`, one-shot on `mirror_service`, so it runs on both arches at the same
call site as DRAINCAP's and SERWIT-1B's. It runs everything a power verb runs **except the SMC** —
which is the one line of a shutdown a fixture may not execute, since the next instruction would take
the machine and leave no verdict to read:

```
ring filled to SLOTS x PWRDRAIN_LINE_LEN = 64 x 68 B = 4 352 B     (SO29's whole ring)
power_drain("pwrshutoff")  ->  lines = 64, bytes = 4 352           (22x DRAIN_BYTE_BUDGET)
a following drain          ->  residue = 0
:: PWRDRAIN: … -> PASS ::
```

4 352 B is 22x the byte budget on purpose: the gap between *uncapped* and *capped* has to be wide
enough that the fixture cannot pass by accident on a tree where the two spellings were swapped. The
`lines` and `bytes` comparisons are `>=` and `residue == 0` is strict — a live kernel can stage a
foreign line between the fill loop and the drain, and foreign traffic can only ADD to what a full
drain emits, never subtract, so `>=` is the direction the property actually points.

**Go-red, both at runtime**, because `drain` and `drain_capped` share a signature and no type can
separate them:

* the realistic one — swap `power_drain`'s `drain(...)` for `drain_capped(...)`: the budget stops it
  after 3 lines of 68 B (68, 136, 204; `drain_may_continue` is strictly `<`), the fixture reads
  `lines=3 residue=61` and prints `-> FAIL`, and `FAULT_PATTERNS` turns that into a non-zero
  `./arroyo test-arm`;
* the blunt one — delete the `drain(...)` call: `lines=0 bytes=0 residue=64`, same `-> FAIL`.

What the fixture does **not** prove, stated rather than implied: that the firmware call is reached,
that `raw_write_str` outruns the SMC on real silicon, or that the Jetson's UART has flushed its own
FIFO when power drops. None of those is observable from inside the machine. It proves the ring is
empty and the bytes were handed to the port before the verb continues.

### Certification

The witness tokens are `[pwrreboot]` / `[pwrshutoff]`, `power.rs`'s own families: subsystem-named,
never board-named, and 11+ bytes with their brackets, so LLVM cannot immediate-encode them out of
`.rodata`. `UNAOS_TEGRA=1 ./arroyo esp-jetson` followed by

```
LC_ALL=C grep -a -o -F 'ring drained' target/aarch64_esp/kernel.elf | wc -l
```

is the artifact proof — the presence of an instrument is proven in the artifact, never in the diff.

> ⚠ **SCOPE, as it stood.** On x86 SO31 covered the `power::shutdown` route only. `video/crystal.rs`'s
> Shut Down and `video/instgui.rs` call `arch::acpi_power::poweroff()` **directly** and reached S5 with
> the ring unflushed. That box is **closed by S5DRAIN below** (trunk queue §5, 2026-09-12);
> `acpi_power::reboot` already drained (since LOCKFIX) and carries the witness too.

## S5DRAIN — the drain belongs at the PORT, not at each route into it

Trunk queue §5 (2026-09-12, SERDRAIN's finding). SO31 part 3 put the flush in `power.rs`, once per
verb. On aarch64 that is complete: every route to an SMC — the shell's `power::shutdown`, the
desktop's `power::crystal_shutdown` — passes through that file. On **x86 it was half a fix**, and the
missing half was the half an operator uses.

`grep -rn 'acpi_power::poweroff' unaos/crates/kernel/src/` finds three call sites:

| call site | route | before S5DRAIN |
| --- | --- | --- |
| `power.rs:190` | shell `power::shutdown` | drained (SO31 part 3) |
| `video/crystal.rs:686` | the desktop's **Shut Down** menu item | **undrained** |
| `video/instgui.rs:578` | the installer's halt | **undrained** |

A desktop Shut Down is the press most likely to land while the compositor is printing, which is
exactly when the ring is deepest — so the two routes that bypassed `power.rs` were the two whose
evidence was worth most. What died with the power was everything `_print` had deferred, the verb's own
`:: SHARD-MENU: crystal_pick verb=ShutDown ::` announce included.

### The fix

One statement, at the top of `poweroff()` — the single point every x86 S5 route passes through —
calling the **same** `serial_ring::power_drain(tag)` entry point on the **same** `[pwrshutoff]` witness
family `power.rs` uses. Not a second copy of the policy: a second spelling is how two divergent
policies happen, and this document exists because the transport once had two contracts wearing one
name (see *SERWIT-1B PARITY* above).

It is the **first** statement because `poweroff()`'s `discover()` failure arm prints and then parks in
`hlt_loop`, and a park is a context with no next print just as S5 is. **No caller is exempt**, and
that is checked rather than assumed: all three end in S5 or in `hlt_loop`, both terminal. The
`power.rs` route now drains twice — its own verb-order count on the wire, then `lines=0` at the port —
which is the shape the reboot ladder has had since SO31 and reads as an empty ring, not as a failure.

The statement is folded **line-neutral onto `poweroff()`'s signature line**: `gas_space_name`,
`table_checksum_ok`, `discover_reset`, `reset_settle`, `raw_witness`, `reset_report` and `reboot` all
sit below it in that file, and a new source line would move every `panic::Location` record in them
(`docs/dev/LEDGER.md` P7). Its body, `s5_ring_flush()`, is a file-tail append.

### The fixture

`serial_ring::s5drain_selftest`, one-shot on `mirror_service`, `witness`-gated and
`target_arch = "x86_64"`-gated — the defect is x86's, because on aarch64 the desktop's Shut Down is
`power::crystal_shutdown` and there is no caller that reaches a firmware power call without passing
`power.rs`.

It exists because PWRDRAIN cannot cover this. PWRDRAIN proves the *policy* — a full drain empties the
ring and counts the bytes — and says nothing about which routes call it. S5DRAIN proves the other half
of the sentence: it calls `arch::acpi_power::s5_ring_flush`, **the symbol `poweroff()`'s first
statement calls**, `#[inline(never)]` so it is one call site and not two inlined copies, and asserts
the ring is empty when it returns.

```
ring filled to SLOTS x S5DRAIN_LINE_LEN = 64 x 67 B = 4 288 B
acpi_power::s5_ring_flush()  ->  lines = 64, bytes = 4 288        (22x DRAIN_BYTE_BUDGET)
a following drain            ->  residue = 0
:: S5DRAIN: … -> PASS ::
```

`S5DRAIN_LINE_LEN = 18 + 48 + 1 = 67` B: `"[s5drain] fill NN "` is 18 bytes, `DRAINCAP_PAD` is 48, the
newline is 1 — one byte narrower than `PWRDRAIN_LINE_LEN` only because the tag is one character
shorter. Two `const _: () = assert!(…)` rows pin it: the line must not truncate against `SLOT_LEN`,
and the filled ring must be more than eight budgets wide or capped and uncapped are
indistinguishable. The `lines`/`bytes` comparisons are `>=` and `residue == 0` is strict, for
PWRDRAIN's reason: foreign traffic on a live kernel can only ADD to what a full drain emits.

**The go-red**, both at runtime, because `drain` and `drain_capped` share a signature and no type can
separate them:

* the realistic one — swap `s5_ring_flush`'s `power_drain(...)` for `drain_capped(...)`: the 192 B
  budget stops it after 3 lines of 67 B (67, 134, 201; `drain_may_continue` is strictly `<`), the
  fixture reads `lines=3 residue=61` and prints `-> FAIL`, which `FAULT_PATTERNS` turns into a
  non-zero exit from `UNAOS_WC=1 ./arroyo test 90`;
* the blunt one — delete the `power_drain(...)` call: `lines=0 bytes=0 residue=64`, same `-> FAIL`.

**What it does not prove, stated rather than implied:** that `poweroff()`'s *first statement* is that
call. Deleting the fold on the signature line and leaving `s5_ring_flush` intact leaves this fixture
green. That is one line of reading, it is the same gap PWRDRAIN names for the SMC, and the regression
that is actually plausible is someone changing the drain rather than deleting the call — which is the
whole reason `s5_ring_flush` is a named symbol instead of an open-coded `power_drain`.

## WITNESS-GATING — the three ring fixtures are instruments, and instruments cost UART

Trunk queue §5 (2026-09-12, SERDRAIN). `draincap_selftest`, `backpressure_selftest` and
`pwrdrain_selftest` each fill the live ring to `SLOTS` once per boot from `mirror_service`, and none
of them was gated on anything. `mirror_service` runs on every image that reaches it, so a
witness-FREE flight image — the polarity every media command ships (`docs/dev/LAWS.md` §5, orin 20) —
paid for three instruments it carried no other witness for. SO30 is this same defect one layer up:
one witness line spent ~36 % of a boot's whole UART budget.

The cost, measured with `awk` over the x86 witness capture at 115200 8N1 = 11 520 B/s = 86.8 µs/B:

| fixture | on the wire | bytes |
| --- | --- | --- |
| `draincap_selftest` | 64 x 68 B fill + 310 B verdict | 4 662 |
| `backpressure_selftest` | 64 x 66 B fill + 68 B probe + 374 B verdict | 4 666 |
| `pwrdrain_selftest` | 64 x 68 B fill + 46 B `ring drained` + 417 B verdict | 4 815 |
| | **per boot** | **14 143 B = 1.228 s of wire** |

(The fills alone are 12 928 B, which is the figure the queue row carries.)

All three are now `#[cfg(feature = "witness")]`, on the function **and** on the `mirror_service` call
site, the way every other fixture in this tree is gated — and so is everything that exists only to
serve them (`DRAINCAP_PAD`, `DRAINCAP_LINE_B`, `DRAINCAP_BOUND_B`, `BPRESS_PROBE`, `draincap_wire`,
the three one-shot `…_DONE` statics). `s5drain_selftest` was born gated.

**What is NOT gated, and must not be.** The `const _: () = assert!(…)` truth tables —
`drain_may_continue`'s five rows, `defer_policy`'s six, `PWRDRAIN_LINE_LEN`'s two and
`S5DRAIN_LINE_LEN`'s two — stay in every build of both arches. They emit not one byte of code, they
are the go-red that fires before anything boots, and a compile-time proof that only runs in the
configuration nobody ships is the polarity trap `docs/dev/LAWS.md` §5 names. The transport itself is
untouched: `power_drain`, `drain_capped`, `defer_contended` and the ring are not instruments, they
are the wire.

### Proven in the artifact, and the artifact is not the one the finding named

`LC_ALL=C grep -a -o -F` on the built images, never on the diff. `ring drained` is the control — it
is `power_drain`'s own witness, transport and not fixture, and it must SURVIVE the gating; a run
where it went to zero as well would be a broken build, not a saving.

| image (before → after) | `[draincap] fill` | `[bpress] fill` | `[pwrdrain] fill` | `ring drained` | `sink contended` |
| --- | --- | --- | --- | --- | --- |
| `esp-x86` (`ehcihid,kbdwit,sdhcblk,smolnet,sdwrite`) | 1 → **0** | 1 → **0** | 1 → **0** | 2 → 1 | 1 → 1 |
| `kernel8` (`baremetal,skip_xhci`) | 1 → **0** | 1 → **0** | 1 → **0** | 2 → 1 | 1 → 1 |
| `esp-jetson` (`…,tegra,bsptick,bsprun,tegrasmp,apsrun,…`) | **0 → 0** | **0 → 0** | **0 → 0** | 1 → 1 | **0 → 0** |

`ring drained` goes 2 → 1 rather than 2 → 2 because one of the two hits was never the witness: it was
the PWRDRAIN *verdict* string, which quotes the witness back at the reader. The surviving hit is
`power_drain`'s own format string — the transport's, and the one that had to stay. `sink contended` is
SERWIT-2's `[mirror]` announcement, law rather than knob, untouched on both images where
`mirror_service` is reachable at all.

**The saving that is real is the one on the WIRE: 14 143 B per boot, 1.228 s at 115200 8N1.** The
image-size half is reported with a caveat, because on aarch64 the flat image is not a code-size
measurement:

| loadable image (`objcopy -O binary`, never the `.elf`) | before | after | delta |
| --- | --- | --- | --- |
| x86 default knob-off (`esp-x86`'s own knob line) | 1 571 164 | 1 529 300 | **−41 864** |
| `kernel8.img` (Pi 4 bare-metal) | 1 448 252 | 1 315 080 | **−133 172** |
| aarch64-`virt` knob-off | 1 483 102 | 1 595 624 | +112 522 |
| `esp-jetson` | 1 572 056 | 1 692 504 | +120 448 |

> ⚠ The two aarch64 rows GREW while the code shrank, and that is a LINK-LAYOUT artefact, not a
> regression. The aarch64 flat image spans VMA 0 upward, so it contains `.rela.dyn` (≈115 kB) and
> `.rodata` before `.text`, and `.text`'s start address is padded to a coarse boundary above them —
> measured at `0x5c000` in one build of this pair and `0x64000` in the other, a 32 KiB jump for a
> few-kB code delta. So an aarch64 flat size moves in quanta and cannot be read as bytes of code.
> `kernel8.img` does not have that problem (fixed load address, no `.rela.dyn`) and is the aarch64
> number to quote. This is the same trap `docs/dev/LAWS.md` §5 names one step further in: byte
> identity is a yes/no question about the image, and a size DELTA is a different question that the
> same artifact cannot always answer.

> ⚠ **The `esp-jetson` row is a finding, not a pass.** The queue row expected `> 0` there and
> measured `0` — on the tree *before* this gating. The three fixtures never reached the Jetson image
> at all, and neither does SERWIT-2's `[mirror]` announcement or its one-shot verdict: on aarch64 the
> only non-`baremetal` `mirror_service` call site is the shared BSP main loop (`main.rs:1779`), and
> the Jetson's `bsprun` scheduler handoff (`sched::run_bsp(0)`, `main.rs:1494`) diverges before that
> loop is ever entered, so the whole chain is eliminated. The evidence is two-sided: none of
> `[draincap] fill`, `[bpress] fill`, `[pwrdrain] fill`, `bounded by BYTES`, `BACK-PRESSURES`,
> `drains the WHOLE ring` or `sink contended` appears in `target/aarch64_esp/kernel.elf`, while the
> controls `ring drained`, `staging ring full` and `:: ` (499 hits) do; and the symbol table carries
> the `DRAINCAP_DONE` / `PWRDRAIN_DONE` / `BACKPRESSURE_DONE` / `MIRROR_VERDICT_DONE` `.bss` statics
> with **no function symbol** for `mirror_service`, any `…_selftest`, or `mirror_verdict_once`. That
> is `LEDGER.md` SO41 and it is a reachability defect in `main.rs`, which this arc's brief does not
> name — reported, not made.

So the saving is real on every image that can reach `mirror_service` and has no witness to spend on
it — `esp-x86` and `kernel8` measured above, and `esp-arm` / `vm-image` by the same construction —
and on the Jetson the honest statement is that there was nothing to save because there was nothing
running.

## aarch64

The PL011/Tegra path did **not** share the drop defect: its `_print` used a blocking `SERIAL_PORT.lock()`,
which loses nothing. It carried the complementary defect instead — a panic or abort striking mid-print,
on the core already holding that lock, would spin on it forever and the machine would die with no
message at all. Silence by a different route.

Both arches now run the same staging discipline (`try_lock` + defer + shared ring) and the same panic
escape hatch, so there is one serial transport to reason about rather than two. This is shared
verification infrastructure; the Pi seat gates on it too.

## FTDIRX — the FTDI console learns to RECEIVE (x86, `ftdirx`, default OFF)

Everything above is about bytes leaving the machine. On the 2012 rMBP there was no other kind: the
board has no 16550, its only console is an FTDI FT232R cable on xHCI, and that driver was TX-only by
its own admission — `drivers/xhci/ftdi.rs`'s header said bulk-IN RX was "a STUB deferred to a future
arc". So a bench operator could read a boot and could not answer it, and every interactive verdict
on that machine was a photograph of the panel plus a USB keyboard. `ftdirx` is the other direction.

**It is `orinrx` for the other board, and deliberately the same shape.** The Orin's UART RX drain
(`arch/aarch64/serial.rs`'s `serialrx::drain`) polls the port once per console-pump pass and pushes
each byte as `pal::Event::Key`. This does exactly that from a USB endpoint: `service_ftdi` keeps ONE
Normal TRB outstanding on bulk-IN `0x81`, and on each completion pushes the packet's data bytes onto
the same `pal::EVENT_QUEUE` the xHCI HID decoder feeds — so `x86_input_service`'s `next_event` drain
sees a cable byte exactly as it sees a keystroke, and nothing downstream of the queue knows or cares
which it was. The feature name is the TRANSPORT, not the board, for the same reason `serialrx` is.

### The two bytes that are not data

**Every FTDI bulk-IN packet is prefixed by TWO modem-status bytes** (Linux
`drivers/usb/serial/ftdi_sio.h`: byte 0 is the modem status register, byte 1 the line status
register). They are not payload and must never reach the key path: pushed through, a typed `help\n`
arrives at the shell as `\x01\x60help\n`. This is the one thing the arc could get silently wrong,
which is why the fixture's go-red is exactly "delete the strip" rather than something contrived.

**The go-red is the RX witness, not the shell's answer, and that is a measured correction.** Running
the fixture with the strip deleted (2026-09-15) turned the wire red exactly where it should — the
first byte received read `byte=0xb1 '.'` instead of `byte=0x68 'h'`, and the rollup read `rx=24` for
`packets=8` instead of `rx=8` for `packets=8`, i.e. three bytes delivered per packet typed. But the
shell still ran the typed command, because QEMU's two status bytes happen to be `0xb1` and `0x00`,
and `handle_key` discards both as non-printable. That is luck, not design: an FT232 whose modem
status lands in the printable range would type junk into the shell, and a fixture that asserted only
"the shell answered" would have passed the broken build. So the assertion is the conservation law —
`rx` equals the bytes typed and `packets` equals the packets sent — with the shell's answer as the
downstream confirmation, not as the discriminator.

A packet of two bytes or fewer carries no data at all — it is the chip's idle poll, emitted once per
latency-timer tick (16 ms by default; `FTDI_SIO_SET_LATENCY_TIMER` is declared beside the other
vendor requests but this arc does not issue it). Those are counted as `idle=` and dropped. (QEMU 10.2's FT232 model NAKs an IN token when it has no
data rather than answering with a bare status packet, so `idle=0` on every QEMU capture; the counter
is there for the cable, where the latency-timer poll is real.) Counting
them rather than ignoring them is what separates a cable that is quiet from a cable that is dead —
the two look identical from a `rx=0` alone.

### What is where, and why

| Half | Lives in | Why there |
|---|---|---|
| Status-byte strip, counters, the single `push_event` intake | `drivers/xhci/ftdi.rs`, tail module `ftdirx` | FT232 protocol; arch-neutral, beside the rest of the chip's constants |
| Arm the TRB, claim its completion, re-arm | `drivers/xhci/mod.rs`, tail `impl XhciController` | it needs a transfer ring, and that is where transfer rings are |

**The event-ring dispatch does not deliver bytes.** It stores four relaxed words and returns; the
packet is handed to `push_event` from the main-loop service pass instead. That is LOCKFIX: the
event-queue lock must never be taken from inside `drain_event_ring_once`, which runs from every
synchronous pump in the driver.

**Exactly one TRB is outstanding, ever** — the completion is consumed before a new one is pushed, so
the IN ring cannot over-arm. The unarmed window between a retired TD and the re-arm is the same one
PRTSCLOST measures on the HID endpoints, and here it is harmless in a way it is not there: the FT232
buffers received bytes in its own 256-byte RX FIFO and NAKs nothing away, so a byte typed while the
endpoint is dark is still waiting for the next IN token. A HID state change inside that window is
gone forever; a typed character is not.

### Byte-identity, and why the code is in tail blocks

`#[cfg]` does not buy byte-identity on its own: `panic::Location` embeds the source LINE, so a
cfg-erased block inserted ABOVE existing code still moves every panic site below it, in an image
that contains none of the feature (LEDGER P7). Both files here are compiled into images that are
supposed to be unmoved — `drivers/xhci/mod.rs` reaches the Pi's `kernel8.img` — so the new code is
appended past the last statement of each file, and the four sites inside the files proper (the
completion claim, the service call, and three teardown drops) are LINE-NEUTRAL appends folded onto
existing lines, before those lines' first `//`. Measured, not reasoned about:
`./arroyo knoboff ftdirx <baseline>` builds the knob-off image at both trees in one directory and
compares them, with an armed control probe.

### Typing at QEMU

The U2.5 gate attaches QEMU's `-device usb-serial` with a **file** chardev, and a file cannot be
written INTO — there was no way to send bytes toward the kernel. The builder knob
`UNAOS_FTDIRX_INJECT=<unix socket path>` swaps that file for a listening socket chardev, which is
bidirectional; `scripts/ftdi_inject.py` connects to it and writes. Unset, the file chardev is
byte-for-byte what it was, so every existing U2.5 run and its `target/ftdi.log` capture are
untouched. On metal none of this exists: the cable is the chardev and the operator is the injector.

The injector waits for `:: U2.5: FTDI console up` in the KERNEL'S SERIAL log — a different channel
from the cable — before it writes. Not because early bytes would be lost (the FT232 holds them), but
because a run that typed into a console which never came up would otherwise be indistinguishable
from a run whose RX path is broken.

Witnesses on the wire: `:: FTDIRX: first byte rx=<n> byte=… ::` once, and
`:: FTDIRX: rx=… packets=… idle=… errors=… result=OK ::` on the log-scale doubling throttle
`note_ftdi_pump` already uses — so a session of typing costs O(log n) lines for n bytes, which is
what makes a rollup safe on a console whose own output shares the cable. It does not promise a final
total: the last line is the last power-of-two milestone (a 10-byte session ends at `rx=8`).

### FTDICR — the wire's Enter is not the defect; a FOCUSED QUARRY EATS IT

Flight 10 (2026-09-17) typed `help\r`, then `date\r`, then `help\r` at the cable, and for a long
time nothing ran. The RX path was blameless and said so: `rx=5`, `rx=10`, `errors=0`, and the
per-byte doors printed their own verdicts. **The bytes reached the shell's line buffer and the CRs
were eaten by the Quarry window**, measured on
`~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log` after `=== SQUAWK MARK flight10` (read-only,
`awk index()`):

```
[ 252771ms] [quarry] key_route key=0x68 focus=1 took=0     <- 'h' declined, falls to the shell
[ 252771ms] [quarry] key_route key=0x65 focus=1 took=0     <- 'e'
[ 252771ms] [quarry] key_route key=0x6c focus=1 took=0     <- 'l'
[ 252771ms] [quarry] key_route key=0x70 focus=1 took=0     <- 'p'
[ 252772ms] [quarry] key_route key=0x0d focus=1 took=1     <- CR CONSUMED
[ 256841ms] [quarry] key_route key=0x0d focus=1 took=1     <- and again
[ 375912ms] [quarry] key_route key=0x0d focus=0 took=0     <- focus gone: the SAME byte passes
[ 375931ms] :: [midden] cmd="helpdatehelp" -> TerminalError len=44 ::
```

Read the last two lines together: the run that "proved" CR was not Enter is the run in which **CR
submitted the line**, 19 ms after Quarry declined it. Nothing about the byte changed between
252772 ms and 375912 ms; only `focus` did. `handle_key` (`main.rs`) has always taken
`c == b'\n' || c == b'\r'`, so the transport never needed a translation.

**The door, not the transport.** `wc_route_event` (`arch/x86_64/syscall.rs`) offers every key to
`video::strip::key_escape` and then `video::quarry::key_route` BEFORE the shell sees it, and
Quarry's `b'\r' | b'\n'` arm (`video/quarry/live.rs`) consumes Enter whenever Quarry is focused and
on glass — **for both spellings and from every transport**. A keyboard Enter typed at the same
moment is eaten identically; the keyboard only looked privileged because every keyboard Enter in
that flight (`key=0x0a`, at 143793 ms and 396387 ms) happened to arrive with `focus=0`. So an
operator at the serial console cannot reach the shell at all while Quarry holds focus, and has no
way to see why — which is the defect, and it is not in this file's subsystem.

**A normalisation at the FTDI intake would not have fixed it** and is deliberately not shipped: CR
and LF hit the same Quarry arm, every Enter consumer in the tree already matches both, and the
Pi/Orin UART path delivers its bytes raw (`arch/aarch64/serial.rs`'s `serialrx::deliver`), so
translating here would make the two consoles differ for no measured gain. The attempted patch is
kept out of the tree at `~/unaos-bench/scratch/rmbp-0915/ftdicr-logs/ftdicr-crlf-normalisation.patch`.

**The QEMU lane cannot score this, and that is stated rather than assumed.** `UNAOS_USBSERIAL=1
UNAOS_FTDIRX=1 UNAOS_FTDIRX_INJECT=<sock> UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 90` with
`scripts/ftdi_inject.py --text 'help\r'` answers `:: [midden] cmd="help" -> TerminalOutput
len=3793 ::` — and answers it IDENTICALLY with a CR-to-LF arm compiled in and with it removed, so
that fixture has no power over this question. The reason is in the two captures: the flight-10
image is `build=kepler+takeover+fifo+ivb+wc+smc+` with a focused Quarry window, while the QEMU boot
takes no Kepler takeover and its Quarry is never focused. **Any future fixture for this defect must
give Quarry focus first, or it is scoring a state the defect cannot occur in.**

### SERIALDOOR — the wire is a CONSOLE, not a keyboard, and the door is told which it is

Peter's ruling, 2026-09-17, on the §FTDICR reading above: *the serial console is a console, not a
keyboard.* A byte typed at the cable reaches the **shell** whatever holds window focus; a byte typed
on the keyboard keeps today's behaviour, so a focused Quarry still opens its selection with Enter.
§FTDICR closes by saying the defect "is not in this file's subsystem" — it is in the key DOOR — but
the mechanism that separates the two transports is, because the two producers share one queue and
the tag is minted at the intake this document owns.

**Why a tag at all.** `ftdirx::deliver` pushes `pal::Event::Key(b)` into the SAME `pal::EVENT_QUEUE`
the HID decoders push into. By the time a key reaches `wc_route_event` there is nothing left in the
event that says where it came from — which is why `[quarry] key_route key=0x0d focus=1 took=1` was
the correct behaviour of a door that had no way to know better. Any fix must therefore either split
the queue (two producers, two FIFOs, two drains, and every ordering guarantee re-derived) or carry
the origin alongside it. The second is what shipped.

**The origin FIFO** (`drivers/xhci/ftdi.rs`, the `ftdirx` module's tail):

| | |
| --- | --- |
| `note_origin(b)` | producer side. Called from `deliver` **immediately before** `push_event`, never after — `push_event` takes the queue lock and the drain can be running on another core the instant it is released, so a tag written after the push is a tag the door may look for and not find. Tagging early is harmless (see the claim rule); tagging late loses one byte per boot, silently, which is the class of defect nobody reproduces. |
| `claim_origin(b)` | consumer side, the door's question. Pops and answers `true` only when the ring is non-empty **and its head is this exact byte**. |
| `ORIGIN_RING` / `ORIGIN_W` / `ORIGIN_R` | 128 bytes — four full FT232 data packets (`CHUNK` 64 less `STATUS_BYTES`, 62 each), i.e. sized to hold a burst the console can produce between two service passes, not a session. SPSC and lock-free by construction: `deliver`'s caller is the xHCI main-loop service pass and the consumer is the key drain, one of each, so the cursors need no CAS. |
| `ORIGIN_CLAIMED` / `ORIGIN_OVERRUN` | the census. An overrun **is not a lost byte**: the byte still reaches the door, it is simply judged as a keystroke — exactly the pre-arc behaviour — and it is counted rather than swallowed so a capture can say whether the ring was ever the limit. |
| `origin_census() -> (claimed, outstanding, overrun)` | read by the fixture and by the rollup, so a capture can be asked whether every tagged byte was claimed and not merely whether the door fired. |

**A FIFO of BYTES and not a counter**, and the difference is the first interleave. A credit ("the
next N keys are serial") is spent on whichever key comes out of the shared queue next, so one
keyboard report arriving mid-burst hands a keystroke to the shell and a wire byte to the window —
both wrong, in the same pass. The ring records *what* was pushed, in order. The one state it cannot
separate is stated rather than hidden: the same byte value typed on the keyboard while a serial byte
of that value is outstanding is claimed as serial, so that keystroke reaches the shell instead of the
focused window. It needs two people typing the same character into two devices inside one drain
pass, it costs one keystroke, and it fails **toward the shell** — the safe direction, because the
operator can always get back out.

**The branch at the top of `wc_route_event`** (`arch/x86_64/syscall.rs`), and its position is the
whole fix. It sits FIRST, ahead of `strip::key_escape`, `quarry::key_route`, `wc_focus_key` and
`user_input_route`, because every one of those is a question about WINDOW FOCUS and a serial byte is
not addressed to a window. It answers `return raw` — **not** `user_input_route(raw)`: the ruling says
the SHELL, so the byte skips the focused ring-3 input ring too. A program that wants the wire asks
for it; it does not inherit it by being frontmost. The whole branch is `#[cfg(feature = "ftdirx")]`,
matching the module that mints the tag, so a build without the FTDI console compiles nothing here.

**The witness**, bounded to 256 lines because a console being typed into must not spend its own
bandwidth narrating itself:

```
[serialdoor] key=0x0d win_focus=0xffffff03 ring=0x0 -> shell (the wire is a console)
```

`key=` is the byte the door just claimed. **There are TWO focus numbers and a reader of flight 10
will otherwise pair the wrong one**: `win_focus=` is `wm::focus_asid()`, the WINDOW focus
`quarry::key_route` gates on and the one `[quarry] key_route … focus=` reports, while `ring=` is
`USER_INPUT_ACTIVE`, the EL0 input ring. A kernel-owned window holds the first and not the second, so
`win_focus=0xffffff03 ring=0x0` is the NORMAL shape of this line and is not "nothing was focused".

**The fixture builds the losing state rather than hoping for it**, which is the contract §FTDICR's
last sentence set: `serialdoor_selftest` mints its own bar-published window, focuses it, and scores
four legs — (1) CONTROL, an UNTAGGED `Esc` must be eaten by the live door and close the menu, without
which a green "the wire got through" is indistinguishable from a dead door; (2) THE FIX, the SAME
byte tagged through `tag_serial_byte` and driven through the SAME function, handed back as
`Event::Key(0x1b)` with the menu still down; (3) QUARRY's `\r`, flight 10's exact shape, reported
`skip-unopened`/`skip-knoboff` and NOT failed where the window cannot be opened; (4) end to end and
INFORMATIONAL only — `help\r` through `inject_serial_byte`, the producer seam `deliver` itself uses,
with the window still focused, so a capture can be asked for `[midden] cmd=` with a window focused.
Leg 4 is outside the verdict on purpose: the shell drain is another task, this fixture does not own
its scheduling, and a leg whose green depends on another task's luck is the WINMENUFLAKE lesson. The
verdict line is `:: SERIALDOOR: win=… control=… wire=… quarry=…(…) claimed=… outstanding=…
overrun=… e2e=pushed :: PASS ::`, and it is pinned in `unaos/scripts/specs/x86-wc.spec`.

**Knob-off byte identity.** The `ftdirx` module is `#[cfg]`-erased knob-off and this block is
appended at its tail, so no panic `Location` in `ftdi.rs` moves; the door's branch is folded onto
`wc_route_event`'s own signature line, before that line's first `//`, so `syscall.rs` keeps its line
count. `inject_serial_byte` and `tag_serial_byte` are additionally `witness`-gated: **no shipped
image carries a way to synthesise wire bytes.**

## RBTDRAIN — the reboot ladder was flushed into a ring the reset then killed

`docs/dev/OS/rmbp-ledger.md` A3. On the 2012 rMBP the `reboot` verb **worked** — it resets the
machine, FADT `RESET_REG` at 0xcf9 ← 0x6 — and **not one of its witnesses ever reached a human.**

Follow a line from `power::reboot` to the cable and the reason is structural, not a bug in any one
function:

| stage | where it lands | on a machine with a 16550 | on the rMBP |
| --- | --- | --- | --- |
| `serial_println!` | `serial_ring`, maybe deferred | fine | fine |
| `power_drain("pwrreboot")` (SO31) | `serial::_print`'s x86 sinks | **out the port** | 16550 absent; into the FTDI mirror ring |
| `acpi_power::reboot`'s `raw_witness` | `raw_write_str` = the 16550 at 0x3F8 | **out the port** | **written into a port that is not there** |
| the mirror ring | `drivers/xhci/ftdi.rs` `RING`, 256 KiB | n/a | reaches the wire only on the next `service_ftdi` |
| the reset | `out 0xcf9, 6` | — | lands microseconds later; **there is no next pass** |

PWRDRAIN (SO31) and S5DRAIN (SO39) fixed the *staging* half of exactly this problem, and both are
correct. What they could not know is that on this laptop `_print` is not the wire: it is a second
buffer, and the thing that empties it is the xHCI device-service pass — the one context a power verb
has just guaranteed will never run again.

### The fix — a synchronous, bounded flush, called at the PORT and at the verb

`drivers::xhci::ftdi_flush_sync(budget_cycles) -> (bytes, transfers, exhausted)`. Ungated, arch-neutral,
and a no-op that touches no controller state where `ftdi::is_live()` is false — which is every board
that has a real UART. It pumps `XhciController::drain_ftdi` in a loop until the mirror ring is empty
or the budget is spent.

**It never blocks, and three separate bounds hold that:**

* **The controller is CLAIMED, never locked.** `xhci::claim()` is the WEDGE-8 loan — a masked O(1)
  take that answers `Busy` rather than waiting. That is the LOCKFIX discipline: the only correct
  answer to a held lock on a path that cannot wait is to decline it. `NotReady` (no controller at
  all) returns at once.
* **A `Busy` claim is retried only inside the same budget.** The retry exists because the verb is
  typed at the shell while `x86_usb_pump` holds the loan on another core — microseconds of ordinary
  overlap, not a wedge — and a single try would lose the tail to a race. A pass that never gives the
  loan back costs this call its budget and never the machine: the loop's exit is the clock, not the
  lock.
* **Each pump is bounded twice over.** `drain_ftdi` spends at most one PTBURST slice (4 ms) per call,
  and each of its bulk-OUT transfers waits at most one `hw_wait_budget()`, after which the sink is
  turned off permanently rather than retried.

`drain_ftdi` gained a return value for this and nothing else: **true when it left nothing further to
push** (ring empty, or the sink is unusable), **false on its one PTBURST slice-yield exit**. A service
pass may ignore that distinction — its next pass is 4 ms away — but a caller pumping the function in a
loop cannot. The flag already existed in the body as `ring_emptied`; this only lets a caller read it.

### Two call sites, and neither is redundant

```
power::reboot                     [pwrreboot] reboot verb invoked …            -> _print -> mirror ring
power::platform_reboot            [pwrreboot] x86 mechanism: FADT RESET_REG …  -> _print -> mirror ring
  power_drain("pwrreboot")        [pwrreboot] ring drained lines=N bytes=M     -> raw_write_str: 16550 ONLY
  ftdi_flush_witness("pwrreboot") …pumps the mirror ring out the cable…
                                  [pwrreboot] ftdi flushed bytes=B transfers=T exhausted=0
                                                                               -> _print -> mirror ring
acpi_power::reboot                ftdi_flush_sync(...)  <- FIRST STATEMENT: carries the line above out
  interrupts::disable()
  … the FADT ladder …               raw_witness(…)      -> raw_write_str: 16550 ONLY
```

Measured on the QEMU fixture, the cable's last three lines are the three marked `mirror ring`, in that
order, and the two marked `16550 ONLY` are on `serial.log` and **not** on the cable. On the rMBP there
is no 16550 at all, so those two are simply gone — which for the FADT ladder's `raw_witness` lines is
ledger A3 restated (`reset_report` already prints those facts at boot, BOOTFADT, flown F7), and for
`ring drained` is a **residual gap this fixture found and RBTDRAIN does not close**: `power_drain`
writes both the drained lines *and* its own tally through `arch::serial::raw_write_str`, never through
`_print`, so on this laptop PWRDRAIN empties the staging ring into a port that does not exist. A line
DEFERRED under contention is therefore consumed there before the FTDI flush can ever see it. What
RBTDRAIN does carry is everything `_print` reached the mirror ring with — which is every line on a
quiet path, and the verbs' own announces on any path. Closing the rest means giving `power_drain` a
sink that is `_print`'s SET and not one arch port — a change to the shared `serial_ring.rs`, carried
as a `· NEW` row in trunk `docs/dev/QUEUE.md` §5 rather than taken here. It is not rMBP-only in
principle: any board whose console is not the arch's raw port has it.

**Why the witness is printed after the flush it reports and before the flush that carries it.** Print
first and the counts do not exist yet — a line claiming a flush it has not performed is the precise
lie this arc removes. Flush first and print after, and the line itself is left in the ring when the
reset lands. So the verb and the port compose: `power.rs` flushes and then writes its tally into the
ring, and `acpi_power::reboot`'s first statement — the same call — is what puts that tally on the
wire. The port copy is the one **no caller can skip**, which is S5DRAIN's argument at `poweroff()`
repeated at `reboot()`; the verb copy is the one that **reports**, in the verb's own announce order.

**It is the first statement of `reboot()` for a second, independent reason:** the next line disables
interrupts, and the FTDI TX pump awaits completions through `crate::hlt()`. A `hlt` with `IF` clear
and no timer never wakes, so a flush placed after the mask would hang the very verb it makes honest.
The statement is folded **line-neutral onto `reboot()`'s signature**, exactly as `s5_ring_flush` is on
`poweroff()`'s, because `s5_ring_flush` sits below it in that file (`docs/dev/LEDGER.md` P7).

**`exhausted` is a real outcome and is printed as one.** It is `1` when the budget ran out — or the
loan never came back inside it — with bytes still in the ring: *the tail did not all reach the wire.*
A flush that reported success it had not achieved would be worse than no flush at all, because the
reader would stop looking for the missing lines.

### Ungated, on purpose

The mechanism carries no knob. It is transport correctness — `docs/dev/LAWS.md`'s *the wire may not
lose lines* — and it has the same standing as `power_drain` and `s5_ring_flush`, which are likewise
ungated. A witness that exists only under a knob is not the witness a bench sitting in front of an
unknobbed flight image needs. The cost to a boot that has no FTDI console is one relaxed atomic load.

`ftdi_flush_sync` is placed **above** the `ftdirx` tail block in `drivers/xhci/mod.rs` and not below
it: that block is `#[cfg]`-erased knob-off and is appended past the last statement in the file so
nothing under it can move. Ungated source added *below* it would sit at a different line with the knob
on than with it off, moving its own `panic::Location` records between the two images. Above it, these
lines are at the same place in both and the tail block still has nothing beneath it.

### The fixture

No new knob, and no kernel fixture code — the verb IS the fixture. One QEMU run with the emulated
FT232 (`UNAOS_USBSERIAL=1`) and the RX console (`UNAOS_FTDIRX=1` + `UNAOS_FTDIRX_INJECT=<socket>`):
`scripts/ftdi_inject.py` types `reboot\n` at the cable and the cable-side capture is then asserted to
END with the ladder's last witnesses.

`scripts/ftdi_inject.py` gained one additive flag, `--wait-for-text`, for a reason worth stating:
console-up is the right moment to type for an RX gate, and the wrong one for a gate whose verb ENDS
THE RUN. `reboot` at console-up resets the machine long before the boot reaches the `COMPLETE` marker
`./arroyo test` scores, and the harness would call that run TRUNCATED — correctly, and for a reason
that has nothing to do with what the fixture measures. The flag names a second string to wait for in
the same log, so the fixture says *once the boot has finished, type this*. `-no-reboot` goes on via
`UNAOS_QEMU_EXTRA` (the x86 builder's QEMU line does not carry it; the aarch64 ones do), so the reset
EXITS QEMU instead of restarting it and the capture's tail is the reboot ladder rather than a second
boot.

The go-red is the call, not a mutation of it: delete `ftdi_flush_witness` from `platform_reboot` and
the flush from `reboot()`'s signature line, and the same run's capture ends mid-desktop — no
`ring drained`, no `ftdi flushed`, nothing. That is the defect this section describes, reproduced on
demand.

### The S5 port gets the same fold, for S5DRAIN's own reason

`poweroff()`'s signature line now reads `s5_ring_flush(); ftdi_flush_sync(…)` — the staging drain
S5DRAIN put there, then the mirror drain, in that order so S5DRAIN's "first statement" property is
unmoved. Both run before the `discover()` failure arm's `hlt_loop` park and before the
`interrupts::disable()` deeper in the body, which the FTDI pump requires.

It is at the PORT and not only at the verb because that is S5DRAIN's argument one buffer further
along. `power::shutdown` flushes in its own announce order, but `video/crystal.rs`'s **Shut Down**
and `video/instgui.rs` call `acpi_power::poweroff` **directly**, and before this fold those two
routes — the desktop press most likely to land while the compositor is printing — reached S5 with the
mirror ring unflushed altogether. It also carries `platform_shutdown`'s own
`[pwrshutoff] ftdi flushed …` tally, which is written into the ring one call earlier and would
otherwise have nothing left to take it out.

So both x86 power ports now drain both buffers, and the table at the top of this section has no row
left where a route reaches firmware with the cable's tail unsent.
## SERWIRE — the cap is on the metal path, so 375 ms is not 192 bytes of UART

SO29 concluded that `[comp2] max_us` measures this ring's drain, and SO31/DRAINCAP capped one drain at
[`DRAIN_BYTE_BUDGET`] = 192 B and predicted the drag-stall band would fall from 377.8 ms to 22.6 ms.
On render14 metal it did not move: `[comp2] max_us` reads 361 130 / 359 500 / 348 593 us across boots
1/2/5 and 367 166 us on boot 3. SO45 is that disagreement, and this section is the derivation that
settles which half of it is wrong.

### The path, with a line at every hop

The compositor's rollup reaches the UART on the Jetson through exactly this chain. No hop carries a
`cfg` that a tegra flight build turns off, and the last one is the cap:

| hop | file:line | what |
| --- | --- | --- |
| 1 | `video/wm.rs:14451` | `comp2_emit(span)` from the `[wcn]` rollup cadence — `#[cfg(feature = "witness")]`, and the flight line arms `UNAOS_WITNESS=1` |
| 2 | `video/wm.rs:13462` | `serial_println!("[comp2] rollup …")` |
| 3 | `arch/aarch64/serial.rs:284` | the macro expands to `arch::aarch64::serial::_print` |
| 4 | `arch/aarch64/serial.rs:177` | `_print` — `note_submitted`, then the panic escape hatch (`:199`, uncapped, not the flight path) |
| 5 | `arch/aarch64/serial.rs:203` | `arch::without_interrupts(|| { … })` — the whole body runs IRQ-masked |
| 6 | `arch/aarch64/serial.rs:224` | `SERIAL_PORT.try_lock()` |
| 7 | `arch/aarch64/serial.rs:238` | **`serial_ring::drain_capped(&mut sink)` — THE CAP, with no `cfg` on it** |
| 8 | `serial_ring.rs:709/710` | `drain_capped` → `drain_into(.., DRAIN_BYTE_BUDGET)` |
| 9 | `serial_ring.rs:251` | `pub const DRAIN_BYTE_BUDGET: usize = 192` — ungated, both arches, every image |
| 10 | `serial_ring.rs:842` | `drain_may_continue(paid, budget)`, i.e. `paid < 192`, tested BEFORE each line |
| 11 | `arch/aarch64/serial.rs:141` | the sink → `SerialPort::write_str` → `write_byte` |
| 12 | `arch/aarch64/serial.rs:64` | `tegra::write_byte` — bounded THRE poll, one 32-bit store per byte |

`DRAINCAP_PAD` (`serial_ring.rs:1839`) is a different object and is **not** on that path at all. It is
`#[cfg(feature = "witness")]` fixture padding, read only by `draincap_selftest` (`:1892`),
`backpressure_selftest` (`:2029`), `pwrdrain_selftest` (`:2172`) and `s5drain_selftest` (`:2316`) — all
of which are reached only from `mirror_service`, which LEDGER SO41 proved is unreachable on the Jetson
flight image. The pad has never executed on that board. The CAP CONSTANT is not gated on anything.

### The arithmetic

115200 8N1 is 10 bits per byte = 11 520 B/s = **86.805 us/byte**. Step 10 tests the budget before it
takes a line and the test is strictly `<`, so one capped drain pays at most
`DRAIN_BYTE_BUDGET - 1 + <widest line it took>`:

```text
  SO31's stated 68 B line       191 +   68 =   259 B  =  22.5 ms      <- SERDRAIN's prediction
  render14 boot 1 mean 143.1 B  191 +  143 =   334 B  =  29.0 ms
  render14 boot 1 max   1135 B  191 + 1135 = 1 326 B  = 115.1 ms      <- absolute worst single drain
  the uncapped pre-SO31 ring    64 x   68 = 4 352 B  = 377.8 ms      <- SO29's mechanism
```

(line widths measured over the 5 926 lines of `docs/dev/evidence/orin28/render14-boot1-desktop-menubar.log`.)

Against that, `max_us = 361 130 us` is `361130 / 86.805` = **4 161 bytes** of UART — 21.7 budgets, and
1.05 whole staging rings of the 68 B shape. **One capped drain cannot produce it.** So either SO29's
mechanism is wrong, or a single composite pass makes many prints. Two readings off the flight capture
point at the first, and neither is conclusive alone:

* render14 boot 1 has **zero** `[serial] dropped` lines, where the pre-SERDRAIN render13 boot 1 had
  `dropped 5331 lines in 192 events`. The ring never reached `SLOTS` on the flight at all, and a ring
  that never fills has no 64 lines to drain.
* render14 boot 3's power verb printed `[pwrshutoff] ring drained lines=1 bytes=163` — the whole
  staging ring held **one** line at shutdown — on the same boot that read `max_us=367166`.

### The instrument: a SPAN TOTAL is an upper bound on any single pass

`drain_capped` is the one spelling both arches' `_print` use (`arch/aarch64/serial.rs:238`,
`arch/x86_64/serial.rs:132`) and is exactly the object SO31 capped. SERWIRE charges every call in
cycles and bytes, and `comp2_emit` drains that odometer on the same rollup and against the same span
as `max_cyc`. Because the span total bounds any single pass inside the span:

```text
  drain_us  <  max_us   =>  the pass that produced max_us did NOT spend it in the ring drain
  drain_us >=  max_us   =>  the drain could still account for it; per-pass attribution is next
```

That is what makes the adjudication possible without a new bracket in the compositor — this arc is
allowed to touch exactly one site in `video/wm.rs`, the `[comp2]` rollup emit, and that is enough.

The uncapped spellings (`drain`, `power_drain`, `discard_staged`) are deliberately **not** charged:
they run in panic and power contexts, never inside a composite pass, and counting them would put a
shutdown flush into a drag-stall number. `serwire_selftest` asserts both directions.

### The constants, each with its reason

| constant | value | why |
| --- | --- | --- |
| `SERWIRE_ARM_US` (`video/wm.rs`) | `100_000` us | the threshold `max_us` must cross before the line speaks. render14 boot 1's healthy rollups read `pass_us` 8 227..17 679 and `max_us` 43 218..85 871, so 100 ms is above every healthy pass that boot recorded and is 6x the 16.667 ms frame; the stall band it exists for (348 593..367 166 us) clears it by 3.5x. A lower threshold would latch on the first rollup of every boot and report a span with no stall in it. |
| `SERWIRE_LINE_B` | `18 + 48 + 1 = 67` | width of one fixture fill line, `"[serwire] fill NN " + DRAINCAP_PAD + "\n"`, so the byte assertions are arithmetic the compiler checks rather than magic numbers |
| `SERWIRE_BOUND_B` | `DRAIN_BYTE_BUDGET + SERWIRE_LINE_B = 259` | the cap's own stated bound instantiated for that width |
| `SERWIRE_FILL` | `8` | more than one budget and less than `SLOTS`, so one capped drain and one uncapped drain each have work to do and `filled` is not clipped by back-pressure |
| `SERWIRE_WANT_B` | `3 * 67 = 201` | what one capped drain must emit: 0, 67 and 134 are under 192; 201 is not. Three lines. |

The four rows are pinned by `const _: () = assert!(...)` in `serial_ring.rs` and, like every other truth
table in that file, they are **not** `witness`-gated: they emit no code, and a compile-time go-red that
only runs in the configuration nobody ships is the polarity trap LAWS §5 names.

### What it costs a flight boot

The line prints **at most once per boot** — `SERWIRE_SAID` is a one-shot latch — and is bounded at
**324 B**: 113 B of format literal and newline, plus ten fields that cannot exceed 20 decimal digits
each, plus the 11 B verdict word. Measured at 163 B on the render14 numbers it was shaped against. A per-rollup field was rejected outright: at ~140 rollups on a 700 s boot
that is SO30 one layer up, and the brief forbids it. `[comp2]` itself is not widened by one byte.

`wire_take()` is nevertheless called on **every** rollup, latched or not, so the odometer stays a SPAN
and can never silently become a boot total. That ordering is load-bearing in one direction only: an
inflated `drain_us` can produce a false `DRAIN-BOUND`, never a false `NOT-DRAIN`, so the verdict errs
toward keeping SO29 alive rather than toward acquitting the drain.

The hot path pays two `now_cycles()` reads (one `mrs cntvct_el0` / one `rdtsc`) and five relaxed
atomics per print, in a witness build only, and zero bytes of UART.

### Reading the next Orin boot

```text
[comp2] rollup passes=N pass_us=… max_us=361130 … blit_us=… span=…ms
[serwire] arm max_us=361130 span_ms=34258 passes=4 drains=D drain_us=U drain_b=B maxdrain_us=X maxdrain_b=Y cap_b=192 share_pct=P -> NOT-DRAIN
```

`-> NOT-DRAIN` with a small `share_pct` closes SO29: the drag stall is not this ring, and the next
suspect is whatever `[comp2] blit_us` is measuring (on the 361 130 rollup it is 94 713 us mean against a
119 516 us mean pass, i.e. 79 % of the pass, at 32 B/us against the 109 B/us the same boot's healthy
rollups sustain). `-> DRAIN-BOUND` keeps SO29 alive and makes per-pass attribution the next arc.
`maxdrain_b` above `DRAIN_BYTE_BUDGET - 1 + SLOT_LEN` would convict the cap itself.

No `[serwire]` line on a capture that HAS `[comp2]` rollups means no pass crossed 100 ms that boot —
the "the stall did not happen" reading, not "the instrument did not run".

## SERIALTX — a print does not emit its own line any more, and the census says what one costs

*(rmbp-ledger B154, 2026-09-22; commissioned off EHCIDARK B146 and `LEDGER.md` SO29/SO45.)*

### The four terms, and how many of them had ever been measured

`_print` has always paid four things. Until this arc, instruments existed for exactly one:

| # | term | measured before this arc? |
|---|---|---|
| 1 | the **ring drain** — other cores' staged lines | yes: SO29 capped it, SO45's `[serwire]` odometer measures it |
| 2 | the **own-line emission** — this print's line, byte by byte at the UART | **no** |
| 3 | the **contended-lock spin** — SERWIT-1B's bounded backpressure | no |
| 4 | the **four post-mask taps** — fbcon, the FTDI mirror, `tste`, the flight recorder | no |

Terms 1–3 ran inside one `interrupts::without_interrupts`; term 4 ran after it. SO45 answered the
question it was asked — *is `[comp2] max_us` the ring drain?* — and its answer is unaffected here.
What it could not answer is what the OTHER three cost, and term 2 is the large one: at 115200 8N1 a
byte is **86.8 µs**, so a 100-byte line is **~8.7 ms with interrupts masked on that core**, on top of
up to 22.6 ms of capped drain. A core inside that span cannot be preempted and runs no service pass.

### What EHCIDARK saw, and the part of its sentence that does not hold on this board

`usb_xhci.md` §33h (B146) partitioned flight 11's 928 post-scheduler seconds by the capture's own line
rate and measured the EHCI HID pass period collapsing from **1.08 ms** across 840 quiet seconds to
**6.46 ms** across the 17 burst seconds, with `max=108ms` inside that band and the deadman's 1 Hz line
on a *different* core stretching to 2041 ms. That correlation is real, and re-derived independently
here from the same capture by bucketing timestamps into whole seconds: **24 seconds at ≥ 150 lines/s
against 957 below 50 lines/s**, over 999 seconds.

Its *mechanism* sentence — "`_print` … drains the whole ring through a 16550 a byte at a time" —
**cannot be what cost those milliseconds on the rMBP**, and the capture says so in three places:

```text
[    244ms] :: SERWIT-1: contended serial [uart16550=absent carrier=ftdi-mirror law=emitted==0] …
             (submitted=151 emitted=0 declined=151 inflight=0) -> PASS ::
[  28280ms] :: DRAINCAP: SKIP — no 16550 on this machine, nothing is ever staged (SERWIT-1D) …
[ 993490ms] [comp2] rollup … wcd_us=0 wcd_skips=0 …
```

There is no 16550 on that laptop (SERWIT-1D), so every masked branch of `_print` there is the O(1)
`DECLINED` one: nothing is staged, nothing is drained, and not one byte is written behind the mask.
`wcd_us=0` on every rollup of the flight is the same fact from the compositor's end. **The console is
still the right suspect; the UART is not the site.** On that board the only term that can be large is
term 4, the post-mask taps — which is why the census below reports `taps_us` beside `masked_us` rather
than folding the two together, and why a census that measured only the mask would have gone green on
the one machine the arc exists for.

### The fix — `_print` enqueues, an unmasked owner emits

`arch/x86_64/serial.rs`:

* **(a) masked, byte-free.** Resolve the 16550 tri-state if it is still unknown, and enqueue the line
  into the staging ring. A compare-exchange and a memcpy: no `out`, no LSR poll, no UART lock held
  across a byte. `SERIAL1` is taken here only to *ask* whether a 16550 exists (once per boot) and, on
  the machine where the answer is no, to hold the drainer's uniqueness across `discard_staged` — the
  SERWIT-1D accounting is unchanged, branch for branch.
* **(b) the drain owner, with interrupts ENABLED.** `drain_owner()` takes `SERIAL1.try_lock()` and
  drains up to `DRAIN_BYTE_BUDGET` (192 B). Whichever core printed next has always been the drain
  owner; what changed is that it drains unmasked, so the cost is charged to throughput instead of to
  latency and the core stays preemptible at every byte.
* **(c) a print that arrived ALREADY masked** — from an interrupt handler, an exception, or inside
  another subsystem's `without_interrupts` — cannot unmask, because re-enabling interrupts is not the
  console's to do. It still owes the ring forward progress, so it takes exactly one 16550 transmit
  FIFO, `serial_ring::MASKED_BYTE_BUDGET` = **16 B = 1.39 ms worst case**, and leaves the rest.

The three candidate owners the brief named were the 1 kHz timer tick, the 1 ms device-service pump
(`x86_usb_pump`), and the next printer. The next printer is the one that needs no line in a file this
arc may not touch, and it is not a concession: it is self-clocking (a console under load has, by
definition, a next print arriving), it reserves no core, and it leaves the drain on the core already
paying for the console instead of moving that cost onto the service core c7 — which is the core
EHCIDARK is trying to protect.

**The residual flush when printing STOPS is `serial_ring::residual_drain`, called last of all from
`mirror_service`.** That is not decoration. "A deferred line is not a lost one only for as long as
there is a NEXT PRINT" is the sentence PWRDRAIN is built on, and this arc made it load-bearing one
layer earlier: before it, a `_print` wrote its own line directly, so the last line of a burst always
reached the wire on the print that produced it; now every line leaves on a capped drain and the tail
of a burst can outlive its print by one. On a machine that then goes quiet — the end of a boot ladder,
a capture's last rungs, a fixture's final `-> PASS` — a verdict sitting in a ring is
indistinguishable from a fixture that never ran, which is precisely what *the wire may not lose lines*
forbids. `mirror_service` already runs IF=1, lock-free, non-print, on the main loop of both arches, so
the flush costs one `try_lock` and at most `DRAIN_BYTE_BUDGET` bytes per poll and needs no line in the
timer ISR or in `main.rs`. It is a no-op on aarch64 by design: that arch still emits its own line
synchronously, so a line that reached `_print` there has already reached the wire. `power_drain` and
the panic path keep their uncapped synchronous drains exactly as before.

**Ordering is unchanged and is now structural.** The old code drained other cores' staged lines before
writing its own directly, so that a line staged at t0 preceded a line written at t1 > t0. Now every
line goes through the ring, so wire order *is* submission order by construction and there is no
"direct" line left to reorder against.

**Loss is unchanged.** `defer_contended` is still the single shared policy, a full ring still
back-pressures for `BACKPRESSURE_SPINS` bounded turns, and a line that outlives the bound is still
counted in `DROPPED` and announced on the wire. What changed inside the retry turn is *where* it makes
progress: the old turn re-tried the UART inside the mask and, winning it, paid for a whole capped
drain there; the new turn makes room by running the drain owner outside it.

### The census — `[sertx]`, and why it is UNCONDITIONAL

```text
[sertx] prints=N masked_us_max=A masked_us_mean=B drain_us=C emit_us=D spin_us=E bytes=F
        masked_b=G fifo_b=16 taps_us=H taps_us_max=I
        tap_max=fbcon:a,ftdi:b,tste:c,rec:d tap_sum=fbcon:e,ftdi:f,tste:g,rec:h
        sink=uart|ftdi|both|none hz=Z masked_cy_max=J masked_cy_sum=K
```

| field | reading |
|---|---|
| `masked_us_max` | the longest single interrupt-masked `_print` region of the span. **This is the term B146's `pass_period_us_max=` is downstream of.** |
| `masked_us_mean` | a mean near the max is a steady cost; a max orders above it is a tail, which is the shape a console burst makes. |
| `drain_us` | of that, the ring drain — comparable with `[serwire] drain_us`, which measures the same drains from the other end. |
| `emit_us` | of that, the own-line synchronous emission. **Zero is the fixed shape.** Non-zero says that arch still writes its own line under the mask. |
| `spin_us` | of that, SERWIT-1B's backpressure turns; includes those turns' drains, and is 0 on an uncontended print. |
| `bytes` / `masked_b` | bytes put at a 16550, and how many of them behind a mask — the currency of the dark window, at 86.8 µs each. |
| `taps_us` / `taps_us_max` | the four post-mask mirrors. On a board with no 16550 this is the **only** term that can be large. |
| `tap_max` / `tap_sum` | TAPSMAX — the same two quantities **by tap**, `fbcon:…,ftdi:…,tste:…,rec:…` in µs (`rec` is `flightrec`, `UNAOS.LOG`). `taps_us_max` is one number over four sinks, so it can say the taps are the cost but never *which* tap; this pair names one. Read `tap_max` first — a dark window is a **maximum** — and read `tap_sum` beside it exactly as `masked_us_mean` is read against `masked_us_max`. |
| `sink` | `ftdi` on the bench rMBP, `uart` under QEMU, `both` on a machine carrying both. |
| `hz` | the rate the microseconds were derived at. **`hz=0` means UNKNOWN**, every `_us` field reads 0 for that reason alone, and only the `_cy` pair is evidence. |

Every other instrument in this file is `witness`-gated, which is right for a fixture. This one is not,
because the thing it measures only exists on the images that are **not** witness builds — `esp-x86`
media, the card a flight boots (LAWS §5's default-quiet polarity rule: coverage of the ON state is
coverage of a build nobody boots). A `witness`-gated `[sertx]` would be absent from every image that
has ever produced a dark window. The price is two `now_cycles()` reads and a handful of relaxed
atomics per print, against a print that already costs microseconds, plus one rollup line per
`SERTX_PERIOD_MS` (10 s — `[pstrip]`'s cadence, and deliberately not per-window: EHCIDARK's own rollup
constant exists because a census that printed per dark window would print hardest exactly when the
console is already the problem, and a transmit-cost census has that failure mode twice over).

**It therefore MOVES the default image, and `./arroyo knoboff` says so.** That is the honest outcome
for a change to unconditional console code, not a defect to be gated away — see the arc's report for
what the knob-offs can and cannot certify here.

### TAPSMAX — the census BY TAP, and why the seam is the tap ledger

`taps_us_max=` is one number over four sinks. On the bench rMBP it is the **only** term that can be
large (no 16550, so every masked branch is the O(1) `DECLINED` one), which makes it the whole verdict
— and a whole verdict that cannot name a suspect is a number a seat cannot act on. `tap_max=` splits
it four ways and `tap_sum=` gives the same split for the total.

**The stopwatch is the tap ledger itself, and that is why this needed no foreign file.** Every tap
opens with `TapCounters::submit()` and leaves through exactly one terminal outcome — `absorb`,
`suppress`, `drop_line` or `note_staged` — because that is the SERWIT-2 conservation law
(`submitted == absorbed + dropped + suppressed + in_flight`). `submit` stamps `now_cycles()` into the
tap's own `span_t0`; each closer swaps it back to zero and charges the delta to that tap's
`cost_max`/`cost_sum`. Two ledger methods are deliberately **not** closers. `absorb_n` accounts a
batch drained in a *later* print's context, whose own span the `note_staged` that deferred it already
closed — while the drain a tap performs on its *own* print path (`drain_staged_into`, called between
`submit` and `absorb`) is inside the span and correctly charged to it. And `tear` is never a line's
terminal outcome: on `ftdi`/`flightrec` it follows `note_staged`, and on `fbcon` it is charged from
inside `PanelSink::flush` with the rest of the line still to paint, so closing there would end
fbcon's span mid-line and hand its `absorb` an empty cell.

**Every per-tap number is a LOWER bound, one-sidedly.** There is one `span_t0` cell per tap, not one
per core, so two cores inside the same tap make the later `submit` overwrite the earlier stamp: the
first closer then measures from the *later* stamp (short), and the second finds zero and charges
nothing. Contention can only shrink this reading — it can hide a long tap on an unlucky sample and
can never invent one, the same polarity DRAINCAP's clauses are built on. A per-core cell would buy
a CPU-id read on the print path for a number that is a high-water mark over thousands of prints.

`tap_sum` and `taps_us` are measured at different seams and will not add up: `taps_us` is the arch's
single bracket around the whole tap block (call overhead included), `tap_sum` is the four taps' own
submit-to-outcome spans. A large `taps_us - tap_sum` says *the block*, not *a tap*.

**WHAT QEMU CAN AND CANNOT SAY HERE.** QEMU has no Kepler, so `fbcon::panel_console_resume` is never
reached (`splash.rs`'s own note says so) and `PANEL_CONSOLE` stays clear for the whole boot. The
`fbcon` tap therefore takes its O(1) `suppress()` branch on **every** line of a wc-lane run — the
SERWIT-2 tap line reads `fbcon: … absorbed=0 … suppressed=N` and says it out loud. So a wc-lane
`tap_max=fbcon:` is a measurement of a gate returning `false`, **not** of the panel paint, and the
panel geometry cannot be forced either: `UNAOS_FBW`/`UNAOS_FBH` are read by `arch/aarch64/mailbox.rs`
with `option_env!` and have no x86 reader at all. To exercise the fbcon paint half under QEMU the
build must carry `bootlog` (`UNAOS_BOOTLOG=1`), which compiles the QUIET-PANEL gates out and sends
every line through PANEL-DEFER. The three evidence taps (`ftdi` with `UNAOS_FTDIRX=1`, `tste`,
`flightrec`) are live on the ordinary wc lane and their `tap_max=` is a real measurement of them.

### The fixture — the proof is in BYTES, not in cycles

`:: SERIALTX:` (x86_64, `witness`, last of all in `mirror_service`) stages 8 × 65 B, makes **one real
`serial_println!` through the live `_print`**, and asserts on the census counters that the drain owner
emitted `3 × 65 = 195 B` **unmasked** and `0 B` masked, with `emit_us` cycles at zero and nothing
dropped. 195 B is the same arithmetic DRAINCAP and SERWIRE assert, read through a third set of
counters, so all three convict each other if any drifts.

Bytes rather than cycles because a cycle bound would be a flake (host TSC rate, TCG timing, whatever
else the box is compiling) while bytes are exact and are the currency the UART charges.
`tx_note_bytes` classifies each write by the interrupt flag **as it actually stands at the write**,
never by the call site — which is what makes the instrument sound (a print arriving from an interrupt
handler is masked without `_print` having masked anything, and a census keyed on the call site would
file its bytes under "unmasked" and read clean while the pass period collapsed).

**Go-red, and it is one edit:** wrap `drain_owner`'s `drain_capped` arm in
`interrupts::without_interrupts(…)`. The same sink's bytes re-file themselves as masked,
`:: SERIALTX:` reads `masked_b=195` and FAILs, and `[sertx] masked_us_max=` returns to the line cost
in the same run. The second, weaker mutation is deleting the `tx_note_bytes` call: `unmasked_b=0`
where the fill provably went out, and the verdict FAILs for the other clause — which is what stops a
zero from acquitting on no evidence.

### The aarch64 twin, measured

**AND THE aarch64 CENSUS HAS ALREADY ANSWERED THE SECOND MILESTONE.** `./arroyo test-arm` on this same tree prints `[sertx] prints=342 masked_us_max=5029 masked_us_mean=165 drain_us=274 emit_us=55960 spin_us=0 bytes=0 masked_b=0 fifo_b=16 taps_us=2010 taps_us_max=109 sink=uart hz=62 500 000 (CNTFRQ_EL0)`. Read it: **`emit_us=55960` against `drain_us=274` — the own-line emission is 204x the ring drain**, and `masked_us_mean=165` is 55960/342 = 163.6, i.e. the masked region on that arch IS the own-line emission to within 1 %. The term SO29 capped was never the cost on that path; the term nobody had measured is all of it. The port is warranted and the number that warrants it is on the wire. (`bytes=0` there is correct and is a stated limitation, not a hole: aarch64 charges `tx_note_bytes` from the DRAIN sink only — its own-line write goes through `write_fmt` and is timed, not counted — and `drain_us=274` says there was nothing to drain.)

**x86_64 only, and that is a statement not an omission.** `arch/aarch64/serial.rs` still drains and
then writes its own line synchronously inside the mask; this arc MEASURES that arch rather than
changing it, so the fix there is decided by numbers and not by symmetry. A fixture running there would
be REQUIRING a limitation — the shape LAWS §5 calls upside down — and its green would certify that the
second milestone has not been done. That arch's evidence is the `[sertx]` line itself, where `emit_us`
is non-zero and `masked_b` tracks the line width.

### Reading the next flight

```text
[sertx] prints=N masked_us_max=<small> masked_us_mean=<small> drain_us=0 emit_us=0 spin_us=0
        bytes=0 masked_b=0 fifo_b=16 taps_us=<H> taps_us_max=<I>
        tap_max=fbcon:<a>,ftdi:<b>,tste:<c>,rec:<d> … sink=ftdi hz=<tsc>
:: EHCI-HID: [1] EHCIDARK … max=… pass_period_us_max=… pass_period_us_mean=… == witness ::
```

On the bench rMBP `sink=ftdi` and every UART term reads 0 — correctly, because there is no 16550 —
so the flight's whole `[sertx]` verdict rests on `taps_us_max`, and **`tap_max=` is what turns that
verdict into a name.** If `taps_us_max` is in the tens of thousands of microseconds, the dark window
is the POST-MASK TAPS and the largest term of `tap_max=` says which one owns it; `pass_period_us_max=`
will then NOT fall to the tick, and that pair of readings is the measurement B146's prediction is now
conditional on. If `taps_us_max` is small too, the console is acquitted outright on that board and
EHCIDARK's remaining term is elsewhere. **Either way the answer is a number in the capture and not an
inference**, which is the whole of what this arc buys on metal.

**But `fbcon` is NOT the first suspect for the STEADY-STATE window, and flight 11's own wire is why.**
The panel mirror is live for 881 ms of that 999-second capture and no longer: armed at
`[27181ms] :: fbcon: glyphs-active …` by the Kepler takeover, and handed away at
`[28062ms] [panel-owner] panel-ownership-handover from=owner-console-window to=owner-gui-screen
site=fbcon::detach`. `detach()` stores `GUI_ACTIVE`, which is `fbcon::_print`'s FIRST test, so from
28 062 ms the tap is an atomic load and a `suppress()` for the remaining 96.7 % of the boot — and the
`max=108ms` EHCIDARK reports was not set during those 881 ms at all: its census does not begin until
`[112445ms] … max=5ms`, reaches `max=106ms` at 557 953 ms and `max=108ms` at 582 559 ms. The 813
painted lines the SERWIT-2 snapshot records are real and they are all inside the boot window.

So on the rMBP the taps that are live when the dark window happens are `ftdi` and `flightrec` (both
absorb every line) and `tste` (a prefix test), and **`fbcon` is the only one of the four that masks
interrupts at all** — `drivers/xhci/ftdi.rs`, `flight_recorder.rs` and `selftest.rs` contain no
`without_interrupts` call between them, while `video/fbcon.rs` has sixteen. A reader of flight 12
should therefore expect a large `tap_max=fbcon:` in the one rollup covering ~27–28 s and ~0 in every
later one; a large `fbcon:` in a LATE rollup would mean the detach did not hold, and that is itself
the finding.

The same capture also bounds the taps' MEAN, tightly and from the steady regime: 100 prefixed lines
share the single stamp `[ 43375ms]` (and 110 share `[ 27066ms]`), so a whole `_print` — mask, drain,
formatting and all four taps — averaged **under 10 µs of wall clock** there, and under 80 µs per
print even on the worst assumption that all eight cores were printing in lockstep through that
millisecond. A 108 ms window is 1 350 of those back to back on ONE core, against a densest observed
*whole second* of 1 337 lines across every core. The mean is acquitted by three orders of magnitude;
only a per-span TAIL can produce that window, and `tap_max=` is the only field in this census that
can see one. (The bound counts prefixed lines, which is what `logts` stamps at print time; a line
built from several `serial_print!` fragments carries one prefix, so the per-`_print` figure is
smaller still, never larger.)

Under QEMU, where a 16550 does exist, `sink=uart`, `emit_us=0` and `masked_b=0` are the fix itself
reported from the inside, and `masked_us_max` against the pre-arc build is the before/after.
EHCIDARK's prediction for `pass_period_us_max=` — that it falls toward the tick once the console stops
masking — is testable on x86 QEMU and, on the rMBP, is testable only once `taps_us_max` has named
which sink was actually holding the core.
