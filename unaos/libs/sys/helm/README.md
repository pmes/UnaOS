# helm — the kernel control-authority core

The hard control interlock beneath the `helm` handler: `#![no_std]`-capable,
`#![forbid(unsafe_code)]`, embedded in Ring 0 so a kernel fault still parks the
machine safely. This is `unaos/libs/sys/` — a **system** core, not a device
class: it holds control authority, it does not decode a wire or drive a pin.

## Where it sits in the safety stack

The reconciliation ([`docs/dev/USERLAND/RECONCILIATION-2026-07.md`](../../../../docs/dev/USERLAND/RECONCILIATION-2026-07.md))
defines three layers — **law → authority → interlock**:

- **principia** (the policy engine) *states* the user-chosen safety levels per
  action domain.
- **the helm handler** *holds* control authority: every AI-initiated physical
  action passes through it, reads principia's levels, and decides pass/ask/refuse.
- **the helm core (this crate)** is the hard interlock beneath both — the layer
  that does not negotiate.

## The charter: DISARM / MANUAL / AUTO + a FAILSAFE latch

The core is a mode machine with a latch layered on top:

- **DISARM** — outputs neutral. The safe resting state and the only state you
  re-arm from. Selected by the transmitter, it wins immediately from every other
  state, in the same tick it is seen.
- **MANUAL** — outputs follow the transmitter's stick channels.
- **AUTO** — outputs follow a bounded, drop-oldest autonomy command channel.
- **FAILSAFE** — a *latch*, not a mode: loss of a fresh valid frame while armed
  (the deadman) forces neutral and latches. A re-arm requires an explicit DISARM
  first; nothing silently resumes. The transmitter is the human estop at every
  stage, and the mode channel is authoritative.

Arming is deliberate: DISARM→MANUAL/AUTO is refused unless the throttle is at
neutral in a *fresh* frame, so a boot-with-throttle-high or a stale receiver feed
cannot drive. A saturating throttle cap and steering clamp are applied last, at
the output boundary. On `Drop` — including an unwinding panic, and the kernel
panic handler's hook — the core forces the actuator neutral.

## Per-machine domains

Failsafes do not generalize: a rover's safe state is "stop"; a mill's is
"retract and stop the spindle". So each machine class is its own domain module.

- **`rover`** (first domain) — TALUS's arch-neutral receiver→actuator safety
  state machine. It takes decoded [`ibus`](../../../../libs/input/ibus) frames
  in and drives an injected `ActuatorSink` (steering + throttle in µs) that the
  [`pca9685`](../../../../libs/pwm/pca9685) codec later turns into I2C register
  writes. Its safety invariants **I1–I8** are proven by the named tests in
  `tests/rover_invariants.rs` and cross-referenced from
  [`docs/dev/USERLAND/TALUS_SAFETY.md`](../../../../docs/dev/USERLAND/TALUS_SAFETY.md).

## `no_std` and features

Default-on `std` adds the host-only `rover::sim` harness (a deterministic
`FakeClock` + a recording `MockSink`) used by the invariant tests. The kernel
embeds the crate with `default-features = false`; injected seams (a monotonic
`Clock` and the `ActuatorSink`) keep the core arch-neutral and its tests free of
real sleeps.
