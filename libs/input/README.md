# libs/input — TALUS input device-class libraries

Kernel-lane input libraries for the TALUS rover lane: `no_std`, zero-dependency
codecs that decode a physical input wire into typed frames. Each subdirectory is
one device class; the kernel embeds these cores directly (`default-features =
false`) and the host lane exercises them with known-answer tests.

These crates are **deliberate kernel-lane work**, not drift — they are the input
half of the receiver→actuator control path described in
[`docs/ROADMAP.md`](../../docs/ROADMAP.md) §4 and
[`docs/dev/USERLAND/TALUS_SAFETY.md`](../../docs/dev/USERLAND/TALUS_SAFETY.md).

## Contents

- **`ibus/`** — the FlySky i-BUS servo-frame codec. A pure `#![no_std]`,
  `#![forbid(unsafe_code)]` byte parser: it validates the checksum, decodes the
  channels, and flags whether every channel lies within the sane servo band. The
  wire format is frozen by the known-answer tests in `ibus/tests/`. The future
  kernel UART driver feeds the same `Parser`; decoded `Frame`s are consumed by
  the helm rover core (`unaos/libs/sys/helm`, module `rover`), which refuses any
  out-of-range frame.

## Convention

Input codecs decode; they never actuate and never hold control authority. The
safety state machine that acts on a decoded frame lives in the helm core; the
actuator side lives in [`libs/pwm`](../pwm). Keeping decode, control, and
actuation in separate crates is what lets each be tested in isolation and lets
the kernel embed only what it needs.
