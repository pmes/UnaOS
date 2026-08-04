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
| Ring full (depth 64) | The line is lost — but COUNTED, and the next drain puts `[serial] dropped N lines, truncated M (staging ring full, depth 64)` on the wire. Loss is never silent again. |
| Line longer than 240 bytes | Truncated at a UTF-8 char boundary, counted, and reported by the same marker. A shortened line never masquerades as a whole one. |
| Panic | The Mutex is bypassed entirely: staged backlog then panic text, raw and synchronous, through the bounded lock-free UART primitive. |

Implementation: [`crates/kernel/src/serial_ring.rs`](../../../../unaos/crates/kernel/src/serial_ring.rs),
shared by both arches' `_print`.

## Why nothing here can deadlock

This is the constraint that shapes the whole design, because the alternative fixes (a bounded spin, a
blocking acquire) all trade silence for a hang, and a hang in the console is worse than a drop.

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
- **Every wait is bounded.** The only spin in the path is the arch's pre-existing bounded TX-ready
  poll, so a machine with no UART still degrades instead of hanging.

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
:: SERWIT-1: contended serial [uart16550=present carrier=16550@0x3F8 law=declined==0] — 72 lines sent,
   48 deferred to the staging ring, 0 dropped, accounting balanced
   (submitted=73 emitted=73 declined=0 inflight=0) -> PASS ::
```

On a UART-bearing machine the `deferred` figure is the load-bearing one: it says two thirds of the
fixture's lines took the `try_lock`-failure branch, i.e. the branch that used to discard them. A run
reporting `deferred=0` there would mean the fixture failed to contend and proved nothing. On a machine
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

## aarch64

The PL011/Tegra path did **not** share the drop defect: its `_print` used a blocking `SERIAL_PORT.lock()`,
which loses nothing. It carried the complementary defect instead — a panic or abort striking mid-print,
on the core already holding that lock, would spin on it forever and the machine would die with no
message at all. Silence by a different route.

Both arches now run the same staging discipline (`try_lock` + defer + shared ring) and the same panic
escape hatch, so there is one serial transport to reason about rather than two. This is shared
verification infrastructure; the Pi seat gates on it too.
