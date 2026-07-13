# ENDURO — drive-service safety interlocks

ENDURO (ROADMAP §4) makes UnaOS the vehicle computer between the RC receiver and
the actuators. This document is the **written safety-interlock checklist** that
gates every ENDURO metal step: no actuator is wired to a powered ESC until each
interlock below is proven in code and then witnessed on the bench.

The host-native foundation lands first, deliberately, the same way UNAFS-1 froze
its on-disk format before any silicon: the wire format and the safety state
machine are pinned by tests on the host, so the logic is settled before it is
ever near a 3S LiPo. Two crates hold it:

- [`libs/ibus`](../../../libs/ibus) — the FlySky i-BUS servo frame codec
  (`#![no_std]`, zero-dependency). The wire format is frozen by the
  known-answer tests in `libs/ibus/tests/kat_vectors.rs`.
- [`libs/drive`](../../../libs/drive) — the arch-neutral drive-service core: the
  DISARM / MANUAL / AUTO state machine with a FAILSAFE latch. The invariants
  below are proven by the named tests in `libs/drive/tests/invariants.rs`.

Both crates are `#![forbid(unsafe_code)]`. The `drive` core is `no_std` (verified
with `cargo check -p drive --no-default-features`) so the future kernel drive
service embeds it unchanged; the injected `Clock` and `ActuatorSink` seams let
the whole battery run on a simulated clock and a mock sink with zero real
sleeps.

The transmitter is the human override and estop at **every** stage of autonomy.
The mode channel is authoritative over everything the autonomy feed asks for
(that is interlock I4).

## The interlock checklist

Each row is one safety invariant, the test that proves it host-side, and the
bench precondition it maps to. The bench precondition is what a person verifies
with the transmitter and a servo/ESC (or a scope on the PWM line) before that
interlock is considered silicon-confirmed.

| # | Invariant | Proven by (`libs/drive/tests/invariants.rs`) | Bench precondition |
|---|-----------|-----------------------------------------------|--------------------|
| **I1** | **Neutral unless armed** — the sink receives exact neutral in DISARM and FAILSAFE, always, regardless of stick position. | `i1_neutral_unless_armed` | Mode switch low (DISARM) with the sticks deflected ⇒ steering centred, ESC at stop. |
| **I2** | **Deadman** — more than 500 ms without a fresh *valid* frame while armed forces neutral and latches FAILSAFE; a late frame does not silently resume — a re-arm is required. | `i2_deadman_latches_failsafe_and_requires_rearm` | Transmitter off (or out of range) ⇒ wheels neutral within 500 ms and stay neutral until the operator disarms and re-arms. |
| **I3** | **Throttle cap** — a saturating clamp applied at the output boundary, last, after everything else; no path emits throttle above the configured cap while armed. | `i3_throttle_cap_never_exceeded` | Throttle stick to full forward ⇒ the measured ESC pulse never exceeds the configured cap (scope the PWM line). |
| **I4** | **Disarm from every state** — a DISARM mode channel wins immediately, from MANUAL, AUTO, and FAILSAFE, in the same tick it is seen. | `i4_disarm_from_every_state` | Flip the mode switch to DISARM at any moment, in any mode ⇒ wheels go neutral immediately. This is the estop. |
| **I5** | **Deliberate arm** — DISARM→MANUAL/AUTO requires throttle at neutral in the same frame; a boot-with-throttle-high must not drive. | `i5_deliberate_arm_requires_throttle_neutral` | Power on / arm with the throttle stick not centred ⇒ the vehicle does not move; it arms only once the throttle is returned to neutral. |
| **I6** | **Invalid input never actuates** — an out-of-range frame (checksum failures never even reach the core) never touches the sink and does not refresh the deadman; it counts toward it. | `i6_invalid_input_never_actuates_counts_deadman` | A receiver glitch / corrupt frame ⇒ no twitch on the actuators; sustained glitching ⇒ the deadman trips exactly as a signal loss would. |
| **I7** | **Panic/drop ⇒ neutral** — the drive core's `Drop` forces the sink neutral on scope exit and on an unwinding panic; the kernel panic handler hooks the same `force_neutral` path. | `i7_drop_forces_neutral`, `i7_panic_unwind_forces_neutral` | A kernel panic or a killed drive service ⇒ the actuator is parked neutral, not left at its last commanded value. |
| **I8** | **Bounded command channel** — AUTO commands queue in a fixed-capacity, drop-oldest ring; overflow never blocks the control tick. | `i8_command_channel_bounded_drop_oldest`, `i8_auto_consumes_commands_and_caps` | Autonomy flooding commands ⇒ the 20 ms control loop stays real-time; only a bounded backlog is ever held, and stale commands are dropped in favour of fresh ones. |

A companion scenario test, `scenario_full_sink_trace`, drives the whole
sequence — arm → drive → signal loss → deadman → re-arm — and asserts the exact
ordered trace of setpoints the sink receives, so the interlocks are verified not
only in isolation but composed.

## The kernel seam (I7)

The `DriveCore` owns its `ActuatorSink`, so leaving scope always runs the
drop-to-neutral guard. The kernel drive service holds the drive instance and its
panic handler calls the identical `DriveCore::force_neutral` before halting, so a
kernel fault parks the wheels exactly as a host-side drop does. `force_neutral`
also latches FAILSAFE and disarms, so nothing resumes without a deliberate
re-arm.

## What is deferred to metal

The host foundation freezes the format and the logic; it does not touch
hardware. The remaining ENDURO legs, each behind this checklist, are:

- **i-BUS on a UART.** The FGr8B's i-BUS output option is verify-on-bench; the
  KATs are synthetic until real captures are appended to `CAPTURED_FRAMES` in
  `libs/ibus/tests/kat_vectors.rs` (the test harness is already wired for the
  upgrade). The i-BUS line must be a second Tegra UART, never the TCU-owned
  debug UART.
- **PWM out.** Tegra's native PWM is 8-bit at 50 Hz, insufficient for a servo,
  so actuation goes through a PCA9685 over I2C (12-bit duty; a later
  `libs/pca9685` arc). The `drive` sink speaks microseconds only — it makes no
  assumption about the PWM backend.
- **Power.** The Orin barrel input accepts 7–20 V, so a 3S LiPo drives it
  directly. No actuator is energised until every interlock above is bench-green.
