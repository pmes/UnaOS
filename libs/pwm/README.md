# libs/pwm — TALUS actuation device-class libraries

Kernel-lane PWM/actuation libraries for the TALUS rover lane: `no_std`,
zero-dependency codecs that translate an actuator setpoint into the exact bytes
a PWM controller consumes. Each subdirectory is one device class; the kernel
embeds these cores directly (`default-features = false`) and the host lane
exercises them with datasheet known-answer tests.

These crates are **deliberate kernel-lane work**, not drift — they are the
actuation half of the receiver→actuator control path described in
[`docs/ROADMAP.md`](../../docs/ROADMAP.md) §4 and
[`docs/dev/USERLAND/TALUS_SAFETY.md`](../../docs/dev/USERLAND/TALUS_SAFETY.md).

## Contents

- **`pca9685/`** — the PCA9685 16-channel 12-bit PWM register/prescale codec. A
  pure `#![no_std]`, `#![forbid(unsafe_code)]` codec: it computes the prescale
  and the per-channel duty register writes; the future kernel I2C driver moves
  the bytes. The prescale math and register map are frozen by datasheet KATs. A
  dev-only seam test proves a `helm::rover::Actuation` (steering + throttle in
  µs) composes cleanly through the codec's µs→counts helper — the shape a real
  kernel actuation service takes: the safety core hands it µs, it hands the I2C
  driver register writes.

## Convention

Actuation codecs encode; they hold no control authority and make no safety
decision. The setpoints they encode come only from the helm rover core
(`unaos/libs/sys/helm`, module `rover`), which applies the full safety envelope
(arm state, deadman, throttle cap) before any setpoint is produced. The input
side lives in [`libs/input`](../input).
