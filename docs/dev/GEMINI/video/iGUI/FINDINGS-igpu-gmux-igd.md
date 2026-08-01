# FINDINGS — igpu / GMUX-IGD: found, not fixed

Observed while doing the GMUX-to-IGD arc on `wt/gmux-igd-x86` and deliberately left
alone. Where a claim is unverified it says so.

---

## 1. The three seams — outside this lane, and they are what a deferred revert needs

`interrupts.rs`, `main.rs` and `shell.rs` are outside the igpu lane, so this arc did not
touch them. The consequence, stated so nobody has to rediscover it:

| Seam | File | What it would enable |
|---|---|---|
| 1 kHz `gmux_tick()` hook | `arch/x86_64/interrupts.rs`, in `timer_interrupt_handler` before `eoi()` | a deadline-driven revert independent of the arming call stack |
| pump executor call | `main.rs`, in `x86_usb_pump` | a revert that runs while boot continues, instead of stalling it |
| `gmux-revert` verb | `shell.rs`, `dispatch_command` | an operator-triggered revert |

The third is the cheapest and most useful: `gmux_revert_now()` is already public,
complete and idempotent, so the verb is two lines plus a help line. The RUNBOOK
currently has to tell the operator *"there is no verb, do not go looking for one"*.

**The first two should not be wired without redesigning the arc.** They would reintroduce
exactly the split this arc removed — an arming context whose executor may never run —
unless the arm is made conditional on the executor having demonstrably ticked at least
once. If a future round wants boot to continue during the dwell, that liveness beacon is
the piece to build, not just the hooks.

Reference for what those seams looked like: `46a17952` and `78b0ee3f` on
`battery-power-consumption-baseline`.

---

## 2. `gmux_igd` was missing from `crates/kernel/Cargo.toml`

`ce5c6f49` ("build: wire UNAOS_GMUX_IGD") wired `arroyo` and `builder/src/main.rs` on
the stated belief that Cargo.toml already carried the feature. On trunk it did not — the
entry only ever existed on the reference branch. `--features gmux_igd` could not have
resolved.

Fixed here (same crate as the code it gates), but flagged because it is the same failure
mode `ce5c6f49`'s own message describes: *"I checked Cargo.toml, saw the entry, and
stopped one file short"* — here the entry that was seen was on a different branch.

**Generalisation worth keeping:** the whiteboard asserted the knob was "already wired in
all three places". Two of three were true. A knob's wiring is a property of the tree you
are standing in, not of a sentence in a brief.

---

## 3. Kernel commits are sitting on a branch named for a battery experiment

`46a17952` (`gpu/igpu: Pull 9 - GMUX IGD Switch`) and `78b0ee3f` (`Pull 9 - Deferred
GMUX IGD Switch`) are on `battery-power-consumption-baseline`, the only branch containing
them. Nothing in that branch name suggests GPU driver work, and the round's framing
described this work as thrown away / uncommitted — it was not; it was committed
somewhere unexpected. A future session looking for prior art will not find it by name.

---

## 4. The dwell's iteration-cap backstop has an unknown wall-clock length

`GMUX_DWELL_ITER_CAP = 2_000_000`, each iteration 1000 `pause` instructions. On an Ivy
Bridge that is very roughly 20–30 s, but that is arithmetic on an assumed `pause`
latency, not a measurement — and `pause` latency changed by an order of magnitude between
microarchitecture generations, so the assumption is exactly the kind that should not be
trusted.

It only governs if `arch::ms()` has stopped advancing, so it is a backstop, not the
normal path. The dwell prints `iters=` on every run precisely so one metal boot converts
the guess into a number. **Until that boot happens, the RUNBOOK's "wait to 30 seconds"
is a guess and is labelled as one.**

---

## 5. The switch does not touch `DISCRETE_POWER`, and that is not obviously right

The mux is pointed at the integrated GPU while the discrete GPU is left powered
(`DISC_POWER=0x03 (ON)` as observed on metal, gmux version 3.2.19). Upstream's switching
flow has a power-off path for the inactive GPU.

Deliberately out of scope: powering the discrete GPU down while it owns the only working
display path is a much larger and much less reversible experiment than moving a mux.
Recorded so it is a decision rather than an oversight.

---

## 6. Nothing guards the knob across boots

`PROBED` is an `AtomicBool` — it prevents re-entry within one boot and nothing more.
Every boot from an armed stick switches the mux again and stalls again. There is no
persistent marker (no file, no NVRAM write, no boot-count) and none was added; anything
persistent would be a storage-layer change well outside this lane.

Consequence: **the media is single-use.** Stated at the top of the RUNBOOK rather than
only in a source comment, because the person it will bite is the operator.

---

## 7. Unverified, and therefore not claimed: the input chain under a switched mux

Whether EHCI-HID through `handle_key` stays alive with the display mux pointed at the
integrated GPU has **not been verified**, on metal or anywhere else. The brief asked for
that verification and for it to be stated in the RUNBOOK.

It could not be verified from this lane: it needs a bench boot, and the code has never
been near metal. Since this build ships no shell verb, no recovery path depends on the
answer, so the RUNBOOK states the non-verification plainly and tells the operator not to
treat the keyboard as a recovery path. **If a future round adds the `shell.rs` verb, this
becomes load-bearing and must be answered first** — a verb that cannot be typed is
another instrument that cannot execute in the state it reports on.

Related and also unverified: whether the serial console keeps emitting during the dwell.
The rig is kernel-TX-only, so serial cannot be a recovery path either way, but if the
capture goes quiet at the switch that is a finding about the UART, not about the mux.

---

## 8. Pre-existing warnings in neighbouring GPU files

Not introduced by this arc and not fixed by it, but they are noise that hides new
warnings in exactly the files this round works in:

- `kepler.rs` — several `unnecessary unsafe block` warnings from blocks nested under the
  large `unsafe { }` at `:334`; `bar1_base` assigned and never read (`:255`).
- `kepler_display.rs` — `matched_storage` assigned but never used (`:104`, `:152`);
  unused `fb_size` (`:253`) and `update_reg` (`:267`).
- `unused import: mmio_write` on the armed x86 build.

25 warnings on the armed `unaos-kernel` lib build. Confirming that a new file adds none
currently means reading past all of them.
