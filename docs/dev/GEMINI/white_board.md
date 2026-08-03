# WHITE BOARD — GR14 close, 2026-08-03

**This is a whiteboard, not a record.** Wiped and rewritten whenever it changes. It carries one
thing: **what Peter needs to know or decide, right now.** Durable facts live in the baton
(`~/.claude/plans/unaos/active/unaos-gemini-coord-baton.md`); per-boot status lives in
`~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN — nothing

Trunk `f43b70ce`, everything pushed, both arches green under the full knob set. No blocked work,
no owed relays, no open tripwires. The arc closed on metal.

---

# → GR15 — THE PICKUP

## What is true right now

- **The x86 video re-land is DONE and metal-proven** (boot s69). M1 witness repair, M2 WC-BBSYNC,
  M3 damage-band substrate, M4 the banded present, plus the witness widening that made any of it
  observable on this arch.
- **U4y is fixed and confirmed** — the SYSCALL stub was parking the ring-3 stack pointer in a
  per-CPU slot, so two ring-3 tasks sharing a core ran on the same user stack whenever either
  blocked inside a syscall. Ledgered in `docs/SECURITY.md`.
- **Storage is real on metal for the first time.** S7 passes at all; U9x/U10 run their genuine FAT
  write-back leg instead of a silent in-memory fallback.
- The bench is armed and the card is a valid boot vehicle (`2bdae79b`, one docs commit behind).

## Where the next arc starts — and it is already measured

Boot s69, both windows in the same boot, so this is an internal control rather than a claim:

```
win=3 (banded console) bytes=20592    present_us=131   torn=no  -> TEAR-FREE
win=2 (UNBANDED)       bytes=1620080  present_us=9743  torn=yes -> AT-RISK
```

**`win=2` is the next arc.** 9.7 ms is 58 % of a frame budget and it tears on 4 of 4 samples. The
banding machinery already exists and is proven; this applies it to a second consumer, and the same
instrument can measure before and after.

Three smaller items behind it, in the baton's MENU: `comp2_emit`/WC-L still aarch64-narrow
(deliberate — widening `comp2_emit` drags COMPOSITE-2's whole ledger with it); `bg`-launched VUG
has never run its real path on x86 (the `[0x20]` flags word is aarch64-only, so `detached` always
reads false and every `bg` VUG takes the 300-frame cap); and `./arroyo check` never compiles the
`user-*` crates at all, so it is vacuous for any userspace change.

## How Peter wants to be worked with

- **He pushes. The seat never does** — and never reports a push as owed without a `git fetch` in
  that same turn. He pushes between turns; a stale list wastes a round-trip every time.
- **Coordinate, don't type.** Plan the work and run executors in parallel on non-overlapping files.
  Hand-editing code while he waits is the wrong shape.
- **Never instruct him on his own bench.** Report media state and what was verified; stop there.
- **Metal is the verdict**, `strings` is the artifact, and the banner is neither.
- Kepler and iGPU belong to Gemini's lane. Build knobs are fine; the driver files are not.

---

# THE PATTERN WORTH KEEPING

Seven instruments this session looked authoritative and could not fail: a waker reported armed that
was never started; one that fired on every idle line; a `strings` probe reading 0 because the tool
was absent; a count reading registers by line; a `pid=` echo for a command that failed with
permission denied; Fox's `cbw_fault=0` with `n=0`; and U9x/U10 "passing" in a fallback mode.

**A probe that cannot read zero proves nothing.** Every check now carries a positive and a negative
control, and the evidence ladder is explicit: a green banner says it compiled, `strings` says it is
present, an executed witness on metal says it ran — claim the highest rung you actually reached.

Two verification traps from today, both of which nearly inverted a verdict: two boots share one
capture file, so confirm a fix by "nothing after the boundary" and never by a whole-file grep; and
the saved `UNAOS.LOG` files are all 66048 bytes because the kernel reserves that size, so only the
hash tells them apart.
