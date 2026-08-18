# Goal

Gate the 1.12s `kdisp` frame-buffer draw hold behind an explicit environment feature knob (`UNAOS_KDISP_HOLD`) to reclaim ~74% of the Kepler boot block latency.

## Proposed Changes

I propose wrapping the 1.12s spin-loop inside `kepler_display.rs` (at lines 396-422) with a `#[cfg(feature = "nvidia-kepler-kdisp-hold")]` block, which is triggered by a new `UNAOS_KDISP_HOLD` environment variable in both `builder/src/main.rs` and `arroyo`. By defaulting this feature to OFF, the standard boot path skips the 1.12s spin, reducing the Kepler boot block from ~1521 ms to ~400 ms. For camera calibration boots, setting `UNAOS_KDISP_HOLD=1` will cleanly compile the hold back into the artifact.

### User Review Required

Does this one-paragraph proposal look good for the fast-ack? If so, I will proceed to add the knob to `builder`, `arroyo`, `Cargo.toml`, and `kepler_display.rs`.
