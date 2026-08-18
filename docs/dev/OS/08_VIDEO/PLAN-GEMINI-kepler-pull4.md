# PLAN — Kepler pull 4 (for the Gemini session): head-scanout decode + PBDMA front-of-queue

**Context:** pull 3 passed its metal sitting (fox-metal-r23s1f #2, rMBP GT 650M,
2026-07-22): VRAM decode and GOP correlation both CONFIRMED FIXED on silicon
(512 MB; GOP base 0x90020000 = BAR1 + 0x20000). Two walls remain, both localized by
the pull-3 witnesses. Capture: `~/unaos-bench/capture/rmbp-r23s1f/` (marks
boot-P3K1..P3K4). This pull is diagnosis-driven: make the next sitting decisive.

**Process:** write `docs/dev/GEMINI/PROPOSALS/PROPOSAL-kepler-pull4.md`
(STATUS: PROPOSED) and push BEFORE implementing — review happens on the proposal
first. Standing gates unchanged (per-phase commits, all-knob + plain `./arroyo check`
green both arches, QEMU quiet baseline, envytools citation per register, out-of-lane
touches flagged).

## Wall 1 — EVO head-scanout decode (k2 aborted `no-match`, blind)

Metal facts: heads were scanned, none matched offset 0x20000, and the `no-match`
abort printed NO per-head raws (only the bounds-fail path does) — the next decode
iteration is blind without them.

1. **Instrument first:** the `no-match` path must dump per-head raws unconditionally:
   `:: kepler: head-raw head=N addr=<raw> size=<raw> storage=<raw> fmt=<raw> ::`
   for every head scanned, then the existing `:: kepler: takeover-abort no-match ::`.
2. **Fix the two candidate causes** (both, behind the same read path, decided by the
   raws):
   a. Address representation: match against BOTH the BAR1-relative offset (0x20000)
      AND the VRAM-physical base (0x90020000) — document which representation the
      scanout register actually holds per envytools disp facts (GF119+ ISO surface
      address is typically a VRAM offset >> 8; check the shift).
   b. Register/format: re-derive the per-head scanout-address register offset and
      field layout for GK107 (disp class GF119+/GK104 differences; cite the XML).
3. Flip logic unchanged otherwise: bounds discipline, latch readback, visual no-op
   rule. If the raws show scanout simply isn't representable as expected, abort with
   the raws — an honest wall report beats a guessed flip.

## Wall 2 — PBDMA never fetches GPFIFO entry 0 (k3 fence-timeout, raws in hand)

Metal facts: `:: kepler: fifo-layout userd=2002000 fence=2014000 gp=1/0 ::` then
`:: kepler: takeover-abort fence-timeout gp_get=0 ch_stat=11000001 ::`.
GP_PUT=1, GP_GET=0 → submission front stall: doorbell delivery, runlist scheduling,
or the USERD/instance base as the HARDWARE sees it.

4. **Decode ch_stat 0x11000001** against envytools GK104 channel-status facts; put
   the decode (bit meanings) in a comment and in REPORT. Add it decoded to the
   timeout witness (`ch_stat=11000001 (<flags>)`).
5. **Audit the submission-front chain, in hardware order, each with a witness:**
   a. Channel instance block contents: GPFIFO base/size and USERD address fields —
      byte layout per envytools (common bug: VRAM offset vs physical vs >>-shifted).
   b. Runlist entry format + submit register and length; verify the runlist was
      actually scheduled (read back the runlist-active/pending status register).
   c. PBDMA assignment/enable: GK107 PBDMA count, channel→PBDMA binding, PBDMA
      enable bits beyond PMC bit 8 (PFIFO has its own engine enables).
   d. Doorbell: confirm GK104-era submission is GP_PUT-in-USERD polled by PBDMA only
      when the channel is resident/scheduled — if a separate kick register exists on
      this generation, cite and use it.
   e. Add `:: kepler: fifo-front pbdma_stat=<raw> runlist_stat=<raw> ::` after the
      doorbell so the next timeout localizes to instance/runlist/pbdma.
6. Keep the bounded poll + honest timeout. Success criterion unchanged:
   `:: kepler: fence DEADBEEF ::` on metal.

## Out of scope
- K-GPU-4 Falcon/PGRAPH ucode.
- Any behavior change without a knob; baseline stays byte-quiet.

**Oracle:** QEMU = quiet baseline + clean refusals (gates green). Metal sitting #3:
k2 with per-head raws is DECISIVE either way (match+flip or raws for the next
derivation); k3 either fences or the new fifo-front witnesses pin the stall stage.
