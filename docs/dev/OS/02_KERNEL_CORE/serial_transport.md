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

Four counters, all `Relaxed` (they gate diagnostics and order nothing):

```
SUBMITTED == EMITTED + DROPPED + in_flight()        and, on a healthy transport, DROPPED == 0
```

`SUBMITTED` counts every `_print`; `EMITTED` counts every submitted line that reached the UART;
`DROPPED` counts ring-full losses; `STAGED` counts deferrals (a deferred line is *not* a lost one, and
the two must never be conflated). The `[serial] dropped …` marker is deliberately **not** counted in
`EMITTED` — it was never submitted by anyone, and counting it would corrupt the law in exactly the runs
where the law matters.

## The SERWIT-1 witness

`crates/kernel/src/serial_ring.rs` + the `serwit1_run` driver in `crates/kernel/src/main.rs`, in the
tree's existing U\*x/witness idiom and gated behind the `witness` battery like every other fixture.

One kernel worker per online AP, all parked on a release gate so their bursts genuinely overlap, each
printing `[serwit] c=<core> n=<seq>` back to back with no yield. The BSP waits bounded on `ticks()`,
then asserts the conservation law across the stress window.

Two independent proofs come out of one run, which is the point:

1. **In-kernel** — the law balances with `dropped == 0`. This is what the `-> PASS` is made of.
2. **On the wire** — every line is sequence-numbered, so the log falsifies the counter from outside:
   ```
   awk '/\[serwit\]/' target/serial.log | sed 's/.*\[serwit\]/[serwit]/' | sort -u | wc -l
   ```
   must equal cores × burst. A counter that only ever agreed with itself would prove nothing.

Verdict line:

```
:: SERWIT-1: contended serial — 72 lines sent, 48 deferred to the staging ring, 0 dropped,
   accounting balanced (submitted=73 emitted=73 inflight=0) -> PASS ::
```

The `deferred` figure is the load-bearing one: it says two thirds of the fixture's lines took the
`try_lock`-failure branch, i.e. the branch that used to discard them. A run reporting `deferred=0`
would mean the fixture failed to contend and proved nothing.

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

## aarch64

The PL011/Tegra path did **not** share the drop defect: its `_print` used a blocking `SERIAL_PORT.lock()`,
which loses nothing. It carried the complementary defect instead — a panic or abort striking mid-print,
on the core already holding that lock, would spin on it forever and the machine would die with no
message at all. Silence by a different route.

Both arches now run the same staging discipline (`try_lock` + defer + shared ring) and the same panic
escape hatch, so there is one serial transport to reason about rather than two. This is shared
verification infrastructure; the Pi seat gates on it too.
