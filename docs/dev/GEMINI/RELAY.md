# RELAY

## → kepler — FENCE brief. Your heartbeat is merged (`794cbccb` → trunk); DISPLAY is now Claude's.

**The standing problem is yours: PFIFO strips VALID from the channel write.** `kepler.rs:1512`
writes it, `:1525` reads back `0x00002000`, `err=0x2` — every boot, ~35 pulls of history.
This arc is RECONNAISSANCE FIRST:

1. **Read-only recon:** around the failing write, dump the PFIFO/channel state that decides
   validity (runlist status, channel instance/RAMFC pointers, engine status, intr/error
   registers) as classified witnesses — **raw hex on every line + a verdict with a stated
   refutation value** (the law your heartbeat round just learned; no bare `UNKNOWN`s).
   Deliverable: a table that says WHICH precondition the hardware considers unmet.
2. **At most ONE write experiment**, and only if the recon names a specific missing
   precondition: BCMA-S1 shape — record the pre-image, self-test the unwind, write, read
   back, restore, witness every step. No experiment without a falsifiable prediction in
   your PROPOSAL doc first (emitter-exact strings — the M12 law).
3. **Boundaries:** `kepler.rs` + `docs/dev/GEMINI/video/Kepler/` only. ⛔ `kepler_display.rs`
   is Claude's now — a diff touching it bounces whole. ⛔ Firmware blobs and blob-derived
   code go to the private `UnaOS-bunker` repo ONLY (with PROVENANCE.md entries), never here.
4. Gate: `./arroyo check` exit 0, zero new warnings, zero trailing whitespace. Hand back
   sha + the recon table; the seat reviews before merge.

## → igpu — while Flight 1b waits for its boot (staging NOW): the blitter learns to say how it died.

Round 9b is merged (`ae304d95`). F1b flies next; its capture writes your round 10. Until
then, one code round in your own file:

1. **Classify the blitter submit verdict.** `igpu.rs:600-664` spins on `HEAD==TAIL` with a
   1M-spin bound and one undifferentiated `blitter wedged` string. Split it:
   head-never-moved / head-stalled-mid-run / head-wrapped, and snapshot the ring registers
   (HEAD/TAIL/CTL/status) at death — raw hex + verdict, refutation value stated. ~60–100
   lines. This is the V3D lesson (classified death > bare death) landing on IVB.
2. Same commit, docs: the F4 residual (`LADDER-igpu-bringup.md:641` still says
   "doubly-bounded waits" — the C2 fossil's last home) + the 9b review nits N5 (reflow the
   ragged `:226` wrap), N6 (scope the banner sentence — `dp_aux_transfer`'s inner wait is
   rdtsc-deadline-only), N7 (note that EDID-failure rows can co-fire with `gmux=FAILED`,
   whose power-cycle advice outranks theirs).
3. Rules unchanged: build on trunk (`794cbccb`+), never regenerate, `igpu.rs` +
   `docs/dev/GEMINI/video/iGUI/` only, gate green + zero new warnings, hand back the sha.
