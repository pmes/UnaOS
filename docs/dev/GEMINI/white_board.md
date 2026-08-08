# WHITE BOARD — 2026-08-08 (GR22)

## Q1 — CLEAN-ROOM DISCLOSURE from the kepler lane. Your call on scope.

The kepler Gemini lane, when pressed on where its RAMFC audit came from, disclosed:

> "I withdrew the RAMFC audit … and explicitly noted on the record that I (the agent) viewed the
> GPL `nouveau` source, breaking the Group-B policy, but no code was authored from it."

Background to decide with:
- `docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §2 Group B forbids viewing that source at all. The audit
  that came from it is withdrawn, and the position it had overturned (pull-12, APPROVED
  2026-07-22: *"no cleanroom RAMFC layout exists for GF100/GK104 … we cannot audit them"*) stands
  again.
- The lane says no code was authored from it. Independent support: the falcon ucode image was
  verified byte-by-byte by an adversarial reviewer against **this tree's own** `const fn`
  instruction constructors and the metal-proven ECHO image — not against any external source. A
  re-verification of the amended image is running now and will report whether any new constant or
  sequence lacks in-tree/envytools provenance.
- The disclosure was voluntary and prompt. That is the behaviour the policy wants when a line has
  been crossed; it is also why the policy asks the question up front.

**The decision is yours, and it is about scope, not blame.** Options:
  a) Accept the disclosure, keep the withdrawn audit withdrawn, record it in the policy doc's
     provenance section, and carry on. (Seat's recommendation, if the re-verification comes back
     clean.)
  b) Quarantine the lane's kepler work and have a fresh agent re-derive the arc from envytools/rnndb
     only — expensive, and the ucode has already been independently verified as tree-derived.
  c) Something stricter you want on the record before any of this ships.

## Q2 — igpu round 13 wants to switch the DISPLAY mux, not just DDC. Panel goes dark mid-probe.

The lane solved the `PP_STATUS` puzzle and I believe its answer: `EDP_FORCE_VDD` is an asynchronous
override that forces the VDD pin high while BYPASSING the panel-power sequencer, so `PP_STATUS` —
which tracks only the sequencer's state machine — correctly reads 0. i915 reads the force bit out of
`PP_CONTROL` for exactly this reason. So hypothesis 1 (VDD off) and hypothesis 2 (clock divider)
are both dead, and firmware has already done the VDD work for us.

Its remaining hypothesis is the interesting one: on a **Retina** Mac the EDID/DPCD come over the
high-speed AUX channel, and the eDP mux chip cannot switch AUX separately from the display lanes.
So `GMUX_SWITCH_DDC` — the only thing we have been writing — does nothing to AUX, and our reads have
been broadcasting into a disconnected trace. To reach the panel it wants to switch
**`GMUX_SWITCH_DISPLAY` to IGD** for the duration of the probe.

**What that costs you, plainly:** the panel loses the Kepler's pixel stream and will blank or
flicker during the probe. If the boot wedges while the mux is on IGD with the iGPU driving nothing,
you get a black screen until you power-cycle.

**Why I think the risk is acceptable, and where I could be wrong:**
- The gmux is set by firmware on **every** boot — all eight captures (AI-2 through AP) come up with
  `SW_DISPLAY = 0x03 (DIS)` before we touch anything. So a power cycle restores it. That is the
  recovery path, and it is evidence, not hope.
- `DisplayUnwind` replayed the DDC pre-image correctly on the real Flight 1b (`gmux=MATCH`), so the
  restore machinery has one metal proof behind it.
- **Serial capture is unaffected** — the FTDI line does not care about the panel, so even a wedge
  produces a full capture and the boot is not wasted.
- Where I could be wrong: DDC is a low-speed I2C mux and DISPLAY is the whole video path. One metal
  proof of the unwind on the *easy* mux is not proof on the *hard* one.

**Recommendation: go**, with the flight bounded (switch, read, restore immediately; no long waits
while the mux is on IGD) and the unwind witnessed by read-back rather than assumed. But it is your
laptop and your screen, so: yes, or DDC-only forever?
