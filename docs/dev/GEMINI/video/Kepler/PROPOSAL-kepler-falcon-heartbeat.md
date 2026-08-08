# PROPOSAL: Falcon Liveness Heartbeat and kdisp_takeover Inner Bounds

## Goal
Resolve Pull-35's blocking question by distinguishing a Falcon bus-error reading from an unreadable host aperture, and separate the `kdisp_takeover` (328 ms) phase into inner bounds.

## Job 1: Falcon Liveness and PRI Fault Discrimination

Establish independently whether the falcon executed the POKE image, separate the failure modes of the poison read, and define the exact scope of the PRI fault.

### 1. The Temporal Separation (Amendment 1)
`MAILBOX1` (`0x409044`) is inside the same `0x409xxx` unit that faults, and Boot Z confirmed it returns `BADF1000` after the poison read. The separation here is **temporal**, not spatial: the falcon cannot reach `iord I[$r8]` until it observes `cmd=1`. A host read between `START_TRIGGER` and `cmd=1` is guaranteed to happen before any poison read is attempted, measuring the falcon's liveness on an unpoisoned aperture.

### 2. The Heartbeat Poll (Amendments 2 & 3)
Between `CPUCTL <= 2` and `cmd=1`, the host polls `MAILBOX1`.
- `PHASE_A_PRELOOP = 0x01` is overwritten in ~3 falcon instructions. The host will almost always read `0x02`.
- The test accepts `hb != MB_SEED && classify_fecs_word(hb) == "VALUE"`.
- The poll is bounded to ~1000 host reads (~1 ms), a fraction of `ECHO_BOUND` (~1,048,576 falcon iterations), ensuring we do not delay `cmd=1` so long that the falcon bounds-exits.

### 3. Aperture Witnesses (Amendments 5, 6, & 9)
Inside the post-poke poll, we read three additional registers:
1. **`CC_SCRATCH[0]` (`0x409800`)**: Host-write-only. The host wrote `1` to it to unblock the falcon; the POKE image *never writes it*. If it reads `00000001` alongside `phase=BADF1000`, only the data-return muxes are dead and `BADF1000` is a real value. If it reads `BADF1000`, the whole unit is dead.
2. **`GPCCS` (`0x41A100`)**: Cross-unit control. Boot Z established its healthy value as `00000010`. If `CC_SCRATCH[0]` is `BADF1000` but `GPCCS` is `00000010`, the sever is unit-scoped (FECS only). If both are `BADF1000`, the entire PRI ring went dead.
3. **`FALCON_CPUCTL` (`0x409100`)**: Read to bound the scope of the sever. The expected readback has bit 4 (0x10) set (the STOPPED bit, per Pull-25/34). The verdict alphabet evaluated is `alive` if `(cpuctl & 0x10) != 0`, `value-mismatch` if the bit is clear, and `severed` (for `BADFxxxx` POISON wildcards).
- *Law preserved*: No host read of `0x409504` is ever performed. `504_read_idx=none` remains intact.

### 4. The Outcome Space (Amendments 4, 7 & 8)
1. **W1 vs W2 Undecidable**: From the host, a completed `iord` that severs the bus (W1) is byte-identical to a fault *at* the `iord` (W2). This instrument establishes liveness and sever scope, but cannot discriminate W1 from W2. **Pull-35's class question cannot be settled from the host on a severed boot.**
2. **Sign-Extension Arms**: `FFFFFFBD` (already live via `PHASE_A_BOUND`) confirms sign-extension. `000000BD` refutes it. (These require `CC_SCRATCH[0] == 00000001` to be trusted).
3. **Heartbeat Arms**:
   - `hb != MB_SEED && classify == VALUE`: Falcon reached the poll loop.
   - `hb == A5A50000` at bound: Falcon never executed; refutes the execution premise.
   - `hb == BADFxxxx`: Unit severed before `cmd=1`; refutes the `0x409504`-trigger model.
   - `hb == 00000000`: Bleed-over from `MAILBOX0`, not a heartbeat.
4. **Sever Arms** (if `phase == BADF1000`):
   - `CC_SCRATCH[0] == 00000001`: Register-scoped sever; `phase` is a real value.
   - `CC_SCRATCH[0] == BADFxxxx` AND `GPCCS == 00000010`: Unit-wide FECS sever.
   - `GPCCS == BADFxxxx`: Ring-wide PRI sever; all FECS conclusions suspect.

### 5. Job 1 Emitter Strings
The instrument will emit:
- `:: kepler: ucode-poke heartbeat hb={:08X} hb_iters={} ::`
- `:: kepler: ctx-poke img=POKE ack={:08X} mb0={:08X} phase={:08X} scratch0={:08X}({}) cpuctl={:08X}({}) gpccs={:08X}({}) iters={} class={} ::`


## Job 2: `kdisp_takeover` Inner Bounds

The `takeover_display` call took 328 ms. We will instrument `kepler_display.rs` to break this down.

**Proposed Changes (`unaos/crates/kernel/src/drivers/gpu/kepler_display.rs`):**
Add a local `kdisp_phase!` macro:
```rust
macro_rules! kdisp_phase {
    ($name:expr) => {
        let t_now = crate::arch::ms();
        serial_println!(":: kdisp: inner phase={} d={} ::", $name, t_now.wrapping_sub(t_last));
        t_last = t_now;
    }
}
let mut t_last = crate::arch::ms();
```
And insert 8 boundaries (including the glyph_draw correction):
1. `evo_core_passes`
2. `evo_scan`
3. `pre_blit_recon`
4. `blit`
5. `nvidia_kepler_kdisp_hold`
6. `glyph_draw`
7. `panel_console_resume`
8. `wcx_activate`

Plus two additional lines in a third shape:
- `:: kdisp: inner phase kdisp_hold cfg_hold={} ::`
- `:: kdisp: inner phase wcx_activate cfg_wc={} ::`

This will pinpoint exactly which operation accounts for the 328 ms without altering any display driver behavior. The 13 ms APIC-vs-TSC deficit is NOT accounted for in the `kdisp_phase!` ledger, as `arch::ms()` on x86 uses the lossy APIC clock.
