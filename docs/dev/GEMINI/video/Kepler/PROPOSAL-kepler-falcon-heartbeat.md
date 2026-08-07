# PROPOSAL: Falcon Liveness Heartbeat and kdisp_takeover Inner Bounds

## Goal
Resolve Pull-35's blocking question by distinguishing a Falcon bus-error reading from an unreadable host aperture, and separate the `kdisp_takeover` (328 ms) phase into inner bounds.

## Job 1: Falcon Liveness and PRI Fault Discrimination

Currently, `phase=BADF1000` is ambiguous because we do not know if the Falcon never started, if only the `CC_SCRATCH` / `MAILBOX` registers were poisoned, or if the entire FECS unit was severed from the host bus by a PRI fault.

**Proposed Changes (`unaos/crates/kernel/src/drivers/gpu/kepler.rs`):**
1. **Liveness Heartbeat:** After writing `START_TRIGGER` (2) to `CPUCTL`, but BEFORE writing `cmd=1` to `CC_SCRATCH[0]`, the host will poll `MAILBOX1` (0x409044). The Falcon image `POKE` writes `1` to `MAILBOX1` before its polling loop. Reading `1` here proves the Falcon booted and reached the wait loop without faulting.
2. **Aperture Witness:** Inside the host's wait loop (after `cmd=1` is written and the Falcon attempts the `0x409504` read), the host will read `FALCON_CPUCTL` (0x409100) alongside the mailboxes. `CPUCTL` is a control register outside the scratch/mailbox data path.
3. **The Third Arm:**
    - If `phase == FFFFFFBD`: Sign-extension is confirmed.
    - If `phase == 000000BD`: Sign-extension is refuted.
    - If `phase == BADF1000` AND `cpuctl == BADF1000` (with a successful heartbeat = 1): The entire FECS unit was severed from the host bus by a fatal PRI fault resulting from the read of `0x409504`.

## Job 2: `kdisp_takeover` Inner Bounds

The `takeover_display` call took 328 ms. We will instrument `kepler_display.rs` to break this down.

**Proposed Changes (`unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`):**
Add a local `kdisp_phase!` macro:
```rust
macro_rules! kdisp_phase {
    ($label:expr) => {
        let t_now = crate::arch::ms();
        serial_println!(":: kdisp: phase={} d={} ::", $label, t_now - t_last);
        t_last = t_now;
    };
}
let mut t_last = crate::arch::ms();
```
And insert 4 boundaries:
1. `pre_blit_recon`: After the EVO-core passes and known-value scans (lines 183-225).
2. `blit`: After the `for y in 0..expected_height` linear writes to BAR1 (line 383).
3. `panel_console_resume`: After `fbcon::panel_console_resume()`.
4. `wcx_activate`: After `wcx::activate()`.

This will pinpoint exactly which operation accounts for the 328 ms without altering any display driver behavior.
