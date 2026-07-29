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

## aarch64

The PL011/Tegra path did **not** share the drop defect: its `_print` used a blocking `SERIAL_PORT.lock()`,
which loses nothing. It carried the complementary defect instead — a panic or abort striking mid-print,
on the core already holding that lock, would spin on it forever and the machine would die with no
message at all. Silence by a different route.

Both arches now run the same staging discipline (`try_lock` + defer + shared ring) and the same panic
escape hatch, so there is one serial transport to reason about rather than two. This is shared
verification infrastructure; the Pi seat gates on it too.
