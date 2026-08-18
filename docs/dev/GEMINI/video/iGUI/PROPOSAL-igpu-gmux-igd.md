# PROPOSAL — igpu: a GMUX switch to IGD that cannot strand the panel

Branch `wt/gmux-igd-x86`. File in lane: `unaos/crates/kernel/src/drivers/gpu/igpu.rs`.
Knob: `UNAOS_GMUX_IGD=1` → Cargo feature `gmux_igd`.

## What this arc delivers, and what it explicitly does not

**Deliverable:** a serial read-back proving that a write to the gmux `SWITCH_DISPLAY`
register landed, and a revert that provably executes on every path that can reach the
switch.

**The panel WILL go black between the switch and the revert. That is the expected
result, not a failure.** The Pull-7 census on this machine reads every iGPU pipe
(`PIPEACONF`/`PIPEBCONF`/`PIPECCONF`), every plane (`DSPACNTR`/`DSPBCNTR`/`DSPCCNTR`)
and `DPLL_A` as zero; the only live display register is `DP_A = 0x0000001C`. Nothing on
the integrated side is driving the panel, so pointing the mux at IGD points it at an
unconfigured display engine. A future round configures pipes; it needs to know the mux
write lands *before* it spends a bench sitting on pipe programming. That is what this
arc buys.

Anything that reads a black panel as "the experiment failed" has misread it. The
falsifiable claim here is exactly one sentence: **the read-back of `SWITCH_DDC`,
`SWITCH_DISPLAY` and `SWITCH_EXTERNAL` after the write equals the values written.**

## Starting point

Trunk (`0913b91e`) contains **no** gmux switch code. The previous session's work exists
as `46a17952` / `78b0ee3f` on the `battery-power-consumption-baseline` branch. That work
is the reference for what was hard-won, and it is also where the six defects live. This
arc re-derives the switch inside `igpu.rs` from that reference.

## What is kept from the reference, unchanged in intent

- **`RevertState` pack/unpack** — one encode point, one decode point. Every mutation of
  the saved pre-switch bytes routes through it, so no saved byte can be lost to a mask.
- **The `0xFFFFFFFF` timeout sentinel refuses to arm.** A pre-switch read that timed out
  means there is no known state to return to, so there is nothing safe to switch away
  from.
- **Port constants and write order match upstream `drivers/platform/x86/apple-gmux.c`**:
  `0x7C2` value, `0x7D0` read-index, `0x7D4` write-index/status; register order
  DDC `0x28` → DISPLAY `0x10` → EXTERNAL `0x40`. This **includes the `wait_ready()`
  between the value write and the index write** in `gmux_index_write`. An earlier review
  twice asked for that wait to be removed; that instruction was wrong and is retracted.
  Upstream has it exactly there. A citation comment goes in the source so it is not
  re-raised a third time.
- **An ISR-safe tick stays trivial** — load, unpack, compare a deadline, store. No port
  I/O, no loop, no blocking wait. See "lane boundary" below for why no such hook is
  wired in this arc.

## The six required changes, and how each is met

**1. The switch must not arm where its revert cannot run.**

The reference armed from `igpu::init()` (inside `pci::init`) while the only executor,
`gmux_task_tick()`, lived in `x86_usb_pump` — spawned ~350 lines of boot later, and only
on a build that is not `rast`, with a non-zero framebuffer, and two distinct APs online.
Three live paths therefore ended with the mux switched and no revert until power cycle.

**This arc makes the arming context and the revert executor the same instruction
stream.** `igpu::init()` arms, writes, verifies, dwells, and reverts on one call stack,
synchronously, before it returns. There is no deferred executor to be absent, no task to
fail to spawn, and no window in which a wedge elsewhere in boot can strand the mux — the
only code that can strand the panel is code between the switch and the revert, and that
is a bounded spin in this function with no calls out.

This is the strongest available reading of "arm from a context whose revert driver is
provably live". The driver is not merely live; it is the next statement.

Cost, stated plainly: boot stalls for the dwell (10 s) inside `pci::init`, before xHCI
enumeration. That is accepted for a one-shot experiment build behind a knob.

**2. The manual trigger must complete the revert itself.**

`gmux_revert_now()` is public and performs the entire port sequence itself — writes,
read-back, comparison, verdict — and returns `bool`. It does not set a flag and hope.
A caller can therefore report truthfully. It is idempotent: it claims the armed state
with a compare-exchange, so a second call after a completed revert returns `false`
rather than re-running the sequence.

**3. Refuse to arm on an unproven protocol.**

In the reference the `boot_ver_ok`/`kern_ver_ok` if/else closed and the arm block opened
*after* it, so the switch fired whether the driver printed `PROTOCOL PROVEN` or
`PROTOCOL UNPROVEN`. Here the arm block sits **inside the `PROTOCOL PROVEN` arm**. A
gmux that answers the handshake but reports an implausible version tuple gets no write.

**4. The knob-off build stays identical to trunk.**

The reference replaced trunk's inline, iteration-bounded `read_gmux_trace()` closures
with `arch::ms()`-deadline helpers that were gated on `target_arch` only — so every
`unaos_ivb` build, armed or not, picked up a wait whose bound depends on the BSP timer
ISR still running. The old bound could not hang; the new one could.

Here **`read_gmux_trace()` is left exactly as trunk has it** — inline closures, hard
iteration cap, no `ms()` anywhere. The entire new helper set lives under
`#[cfg(feature = "gmux_igd")]` and is compiled out when the knob is off. The armed
helpers carry **both** an unconditional iteration cap and an `ms()` deadline, so even on
the armed build a stopped clock cannot hang them.

**5. A failed write must not be treated as a switch.**

Every `index_write` returns `bool`. After the write triple, all three registers are read
back and compared field-by-field against the values intended. The wire carries an
explicit `MATCH` / `MISMATCH` verdict naming which register disagreed and with what,
for both the switch and the revert. Without this a black screen cannot be distinguished
from a write that never happened, and the experiment is unfalsifiable in both directions.

**6. `REVERT_STATE` read-modify-write is made atomic.**

`SeqCst` on an independent load and an independent store does not make the pair atomic.
All mutations use a `compare_exchange_weak` loop over the packed `u64`, so two contexts
cannot interleave and re-run a revert.

## Lane boundary — what this arc does NOT do, and why

The reference commits touched `interrupts.rs` (the 1 kHz `gmux_tick()` hook),
`main.rs` (the `x86_usb_pump` executor call) and `shell.rs` (the `gmux-revert` verb).
**All three are outside this lane and none of them exist on trunk.** They are not added
here.

The consequence is deliberate and must not be papered over: **there is no deferred
revert path and no shell verb in this build.** Shipping `gmux_tick()` and
`gmux_task_tick()` with no caller would be an instrument that cannot execute in the
state it reports on — precisely the failure mode this round exists to stop. So they are
not shipped. The synchronous design needs neither.

`gmux_revert_now()` is public so that a one-line seam in `shell.rs` would wire a working
verb later, but this arc does not claim that verb exists.

## Gate

```
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 UNAOS_GMUX_IGD=1 ./arroyo check
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check
```

Both green on both arches; the armed run's `⚡ kernel features:` banner must end
`…,unaos_ivb,gmux_igd`. No QEMU suite is run — metal is the verdict, and emulation
cannot reach a gmux at all.

## Expected serial witness

```
:: igpu: PROTOCOL PROVEN (version plausible)
:: igpu: [GMUX] pre-switch state: DDC=0x02 DISP=0x03 EXT=0x03
:: igpu: [GMUX] ARMED synchronous revert, dwell=10000ms
:: igpu: [GMUX] switch write: ddc=ok disp=ok ext=ok
:: igpu: [GMUX] switch read-back: DDC=0x01 DISP=0x02 EXT=0x02
:: igpu: [GMUX] switch verdict: MATCH (mux is on IGD)
:: igpu: [GMUX] dwell ended by=deadline elapsed=10001ms iters=...
:: igpu: [GMUX] revert write: ddc=ok disp=ok ext=ok
:: igpu: [GMUX] revert read-back: DDC=0x02 DISP=0x03 EXT=0x03
:: igpu: [GMUX] revert verdict: MATCH (mux is back on DIS)
```

The falsifying observations, each of which must be distinguishable on the wire:
`REFUSED` (unproven protocol, or a `0xFFFFFFFF` pre-read), `MISMATCH` on either verdict,
and `dwell ended by=itercap` (the ms clock stopped).

## Single-use media

Nothing guards the knob across boots — `PROBED` only prevents re-entry within one boot.
Every subsequent boot from an armed stick switches the mux again. The stick must be
re-flashed after the sitting. This goes in `RUNBOOK-gmux-igd.md` where the operator
will read it, not only in a source comment.
