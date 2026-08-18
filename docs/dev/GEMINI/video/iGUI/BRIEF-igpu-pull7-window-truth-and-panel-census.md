STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-igpu-pull7.md`, this directory)

# BRIEF — iGPU pull 7: give the power number a denominator, then census what the panel would cost

Coordinator-authored (2026-07-30, GR10). Predecessor: pull 6, APPROVED and LANDED in `46f8f37e`.

## Where it stands

M1 landed and is good. `:: PWR: ::` is in `drivers/smc.rs:966–1024`: raw counters instead of a
bare mean, the window flushed when `ac_derived` changes so charging and discharging never mix,
`Unknown` binned separately, `min`/`max` seeded from the first sample (checked — an all-negative
discharge window cannot report a `max` of 0 mW that never occurred), and `mW = mV × mA / 1000`,
which is the right unit arithmetic.

M2's verdict — *"the cheap win does not exist"* — is right and stands as the lane's answer.
`smc_pwr.patch` is out of the tree; that housekeeping item is closed.

**The metal baseline (s59) has not run.** So pull 7 is not written to respond to a number. It is
written to fix the one thing that would make that number weaker than it looks, and to do the
read-only groundwork the handover arc needs whatever the number turns out to be.

## M1 — the window has no denominator, and the sign is still an assumption

### 1. ⛔ MUST-FIX — print the elapsed window

`:: PWR: ::` carries `samples`, `sum`, `min`, `max` and no **time**. `PWR_ROLLUP_MS = 10_000`
(`smc.rs:643`) is a *minimum*, not the window: the flush happens on the next SMC sweep past 10 s,
and sweeps are throttled and can retry (`retries=SWEEP/TOTAL` exists precisely because the per-read
drop-out is real). Two consequences, both fatal to the brief's own stated purpose:

- **`sum` cannot become energy.** mW summed over an unknown interval is not mWh, or anything.
- **`sum / samples` is a per-sample mean, not a time-weighted mean**, and the samples are not
  uniformly spaced. Two boots that idle identically but retry differently produce different means
  from the same machine.

The whole point of M1 was *"a boot produces a number that can be compared against another boot"*.
Without the elapsed window it produces a number that can be compared against another boot **only
if the sample cadence was the same**, and nothing in the witness lets a reader check that. Add the
actual elapsed milliseconds of the window to the line, and state in the code comment what a
healthy window's `ms=` looks like next to `samples=` — so a reader can see the cadence and catch a
window where the SMC went quiet.

### 2. Add the boot-cumulative pair

One rollup window is a sample of a boot, not the boot. Carry a boot-total alongside — cumulative
elapsed and cumulative energy (or cumulative sum with cumulative samples, your call, argued) —
following the standing witness discipline: **a delta with the cumulative carried alongside**. Then
two boots compare on one line instead of by adding up windows out of a capture by hand.

### 3. Retire the word "assumption" from the sign convention — by experiment, at the bench

The line currently ends `(sign convention: inherited assumption)`. That label is honest and it was
the right thing to ship, but it is not a fact and this machine gives no independent witness for it:
the 2012 rMBP **lacks the AC key entirely** (`smc.rs:573`), which is why `ac_derived` exists at all.
So there is nothing in software to cross-check the sign against.

There is a free experiment, and the code already supports it: the rollup **flushes on an
`ac_derived` change**. So an attended plug/unplug during s59 produces two adjacent windows whose
signs must be opposite, with the operator's action as the ground truth. Pull 7 must:

- **make the witness say there is no independent witness** — print that the AC key is absent on
  this part, so a reader is never left to assume the state was measured when it was inferred;
- **specify the bench procedure in the proposal**, precisely enough for Peter to run it without
  asking a follow-up: boot on battery, let N rollups print, plug in, let N more print. Say what N
  is and why;
- **pre-declare both outcomes.** If the discharge windows are negative and the charging windows
  positive, the inherited convention is confirmed. If they are not, the convention is wrong and
  every power reading in this lane inverts. Write down what each case prints, in the proposal,
  before the boot.

The label itself changes in pull 8, once the boot has made it a fact. Do not pre-emptively relabel
it "derived" in code that has not yet been run.

## M2 — the panel-readiness census, READ-ONLY

M2 of pull 6 correctly enumerated in prose what would have to be brought up. Pull 7 turns that
prose into **probed values**, so the handover arc starts from readings rather than from a list.

Most of the reads already exist. `drivers/gpu/igpu.rs` dumps `PIPE{A,B,C}CONF`, `PIPE*SRC`,
`DSP*CNTR`, `DSPASURF`, `DP_A` (`0x64000`), `PP_STATUS` (`0x61200`), `PP_CONTROL` (`0x61204`) and
`DPLL_A` at its trace points (`igpu.rs:169–230`), and the canon is that the part reads **all-dead
at all four trace points, `DP_A=0x1C` constant**, pre- and post-ExitBootServices. **Do not
re-derive that.** What is missing is three things:

1. **The gaps in the census.** Name and read what the existing dump does not cover and the
   handover would need: the DDI / eDP link state and lane count, the DPLL/clock source feeding the
   pipe, panel power sequencer timing registers beyond `PP_STATUS`/`PP_CONTROL`, and the DDC/GMBUS
   path (which the gmux canon says is switched to DISCRETE — `SW_DDC=0x02`). Read before write,
   raw before decoded, every offset cited or labelled honestly as probed.
2. **Whether the iGPU is even reachable.** Is the HD 4000 present as a PCI device, is its BAR
   mapped, is the device powered? A register that reads `0x1C` because the engine is off and a
   register that reads `0x1C` because the read never reached the device are indistinguishable in
   the current capture, and that distinction decides whether this arc is possible at all.
3. **The verdict, as an ordered prerequisite list.** For each step that would have to be driven to
   light the panel from the iGPU: the register, the value it reads **now**, the value it would have
   to hold, and whether that step is reversible. A step whose restore path is unknown is called out
   as such. This list is the deliverable — it is what makes the eventual handover a plan instead of
   an attempt.

**Nothing in this pull writes.** Not the gmux, not the display engine, not the SMC, not
`PP_CONTROL`.

## ⛔ Hazards

- **A wrong write blanks the panel.** The Kepler owns scanout from the GOP framebuffer at phys
  `0x90020000` at every observed instant. Taking the mux or the discrete power away while it is
  scanning takes the display away from the only engine able to drive it.
- **Serial is the lifeline and it survives a black panel.** The rMBP console is FTDI,
  kernel-TX-only, on `/dev/ttyUSB0`, independent of the display path. Any experiment in this lane
  runs **with serial attached and captured**, or it does not run. A blank panel with serial is a
  finding; a blank panel without serial is an unrecoverable boot, which this bench treats as
  unaffordable.
- **Power measurements are only meaningful unplugged.** A terminal current on AC is dominated by
  charging and is not system draw. The witness must say which state it was taken in.
- **STOP tripwire.** If anything here would require disabling or weakening a protection, or would
  leave the machine unable to reach a lit panel with no proven restore path — stop, record exactly
  what you observed, and report. Do not improvise around it.

## Laws for this pull

- **Cleanroom.** Hardware facts with attribution are this lane's standing rule (it is how the gmux
  variant table was established in pull 5). **No GPL code bodies.**
- **Gate: `./arroyo check`, both arches, ONLY.** Do NOT run `./arroyo test` or `test-fat` — QEMU
  has no gmux, no SMC and no panel, so it can say nothing about any of this, and the runs cost
  money. Metal is the verdict.
- **Verify the symbols are IN the artifact** as `builder/` produces it (the `esp-x86` media
  artifact), not in a `.rlib` — the s42 INSTGUI lesson: a knob known to `arroyo` and unknown to
  `builder/` ships the feature DISABLED with every check green. `strings` it before staging.
- **Raw counters, never percentages; a delta with the cumulative alongside; an admissibility
  predicate stated in the witness itself.**
- **Keep scratch out of the source tree** — no patch files, extraction dirs or throwaway scripts
  under `unaos/` or at the repo root.
- **PROPOSAL FIRST.** Until this brief's proposal is ruled on, the only file you create is
  `PROPOSAL-igpu-pull7.md`. Do not modify anything under `unaos/crates/kernel/src/`.

## Owed

Metal: the next iGPU sitting (s59) — **unplugged**, with the plug/unplug transition of M1.3.
Report to the coordinator seat (x86/GR10) as `PROPOSAL-igpu-pull7.md` in this directory.
