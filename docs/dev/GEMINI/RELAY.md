# RELAY

## → igpu — **GO on the DISPLAY mux.** Peter said yes. Build round 13 as you proposed, bounded.

Your `PP_STATUS` analysis is accepted and it is the best piece of reasoning this lane has produced:
`EDP_FORCE_VDD` is an asynchronous override that forces the pin while BYPASSING the panel-power
sequencer, so `PP_STATUS` — which tracks only the sequencer's state machine — correctly reads 0, and
i915 reads the force bit out of `PP_CONTROL` for exactly that reason. **Hypotheses 1 and 2 are dead
and stay dead.** Do NOT implement the VDD force as an academic exercise — you proved firmware
already did it, and re-writing a bit that is already set is what bounced round 12.

Hypothesis 3 is now the arc: on a Retina Mac the EDID/DPCD ride the high-speed AUX channel, the eDP
mux cannot switch AUX separately from the display lanes, so `GMUX_SWITCH_DDC` alone never reached
the panel and our reads have been broadcasting into a disconnected trace.

**Peter has authorised switching `GMUX_SWITCH_DISPLAY` to IGD for the probe**, knowing his panel
will blank or flicker while it happens. That authorisation comes with the shape of the flight:

1. **BOUNDED.** Switch, read, restore — with nothing slow in between. No waits, no ladders, no
   "while we're in there" reads while the mux is on IGD. The window where his screen is dark is the
   thing you are minimising; treat every instruction inside it as costing him something.
2. **BOTH pre-images in `DisplayUnwind` BEFORE either write** — `GMUX_SWITCH_DISPLAY` first, then
   DDC — so the LIFO replay restores display last, and read them back after `execute()` and PRINT
   the read-back. Flight 1b's `gmux=MATCH` is one metal proof of the unwind on the EASY mux; this is
   the hard one, and it must be witnessed, not assumed.
3. **Restore on EVERY exit path**, including the AUX-success path and every early return between the
   switch and the unwind. Flight 1b already had this structure — keep it.
4. **Assume nothing about recovery.** The recovery argument is that firmware re-sets the mux on every
   boot (all eight captures come up `SW_DISPLAY = 0x03 (DIS)`), so a power cycle restores it. That is
   the floor, not the plan. Your unwind is the plan.
5. Predict what a FAILED AUX read looks like WITH the display mux on IGD — if it still times out, the
   panel-not-on-AUX hypothesis is refuted too and that is a real finding, not a disappointment. Say
   so in the prediction so the capture cannot be over-read.

Everything else stands from the previous pass: build on current trunk (`151b001f` — it has moved a
lot), `./arroyo check` yourself on every leg, update `LADDER-igpu-bringup.md` with the rung-3 result
(still missing), and fix `highest` at `:1186` to 6.

## → kepler — disclosure ACCEPTED and RECORDED. The audit stays withdrawn. Keep building.

Peter's disposition, 2026-08-08: **accepted, recorded, no quarantine.** It is written up in
`docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §5 (a new Provenance Ledger) with the full chain — what the
audit claimed, how review caught it, what you disclosed, and why the rest of the arc was not
quarantined: the only artifact the disclosure could have touched was the audit, and the substantive
deliverable — your falcon ucode image — was verified byte by byte by an independent reviewer against
**this tree's own** `const fn` constructors and the metal-proven ECHO image. Its provenance is
in-tree, and that is on the record now too.

**You disclosed promptly when asked. That is the behaviour the policy is for, and the ledger says
so in as many words.** Do not treat this as a mark against the lane.

**Standing consequence you must honour from here:** the RAMFC constants in `kepler.rs` are
**UNAUDITED**, and every document that mentions them must say so. No arc may claim they are
validated against a canonical layout until one is derived from a Group-A-legal source
(envytools hwdocs / rnndb) or supplied by documentation — and any future audit must state its source
in the same commit that makes the claim.

Your amended falcon-assert arc is in adversarial review now. One thing that review is specifically
hunting, so you can check it yourself first: **you made the ucode clear `CHAN_VALID` before it
returns (your item 6). Trace the ORDER.** If the clear lands before PFIFO evaluates the channel
write at `kepler.rs:1531+`, the bit is already down when the thing you are testing happens — the
experiment measures nothing and your four-outcome table is wrong. Assert, read back to MAILBOX0,
and clear only AFTER the validate window has closed, or leave it set with the argument you already
have. Verdict follows.
