STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-igpu-pull6.md`, this directory)

# BRIEF — iGPU pull 6: power consumption on battery

Coordinator-authored (2026-07-30, GR9), at Peter's direction: *"we need the iGPU engineer
back so we can control power consumption on battery."*

## Why this track is being reopened

The iGPU lane went dormant after sitting #10 (2026-07-22) because the question it was
chartered to answer got answered. The gmux protocol was proven and the panel owner was
named, which closed the "dead iGPU paradox" and pivoted display work to the Kepler side.

That closed the **display** question. It did not close the **power** question — and the
very fact that closed it is the power problem, stated exactly:

> **Sitting #10, canon:** version 3.2.19 via the 32-bit indexed read; MAX_BRIGHTNESS=0x3FF
> second proof; gate PASSED. Decoded and stable at Boot **and** Kernel:
> **`SW_DISPLAY=0x03` (DISCRETE), `SW_DDC=0x02` (DISCRETE), `DISC_POWER=0x03` (ON).**
> The Kepler dGPU owns the panel at every observed instant.

On a 2012 rMBP the discrete GK107 is the largest single consumer in the machine, and today
UnaOS **never** changes any of that: the discrete GPU is powered and driving the panel for
100% of every boot, on AC and on battery alike. Whatever the ceiling on this machine's
battery life is, that is what sets it.

## The instrument already exists — and it makes this measurable

`drivers/smc.rs` already reads and prints:

```
:: SMC-BATT: present= soc=% volt=mV amp=mA full=mAh rem=mAh ac= retries=/ == witness ::
```

`ac=` distinguishes battery from wall, and **`amp=` is a real current measurement at the
battery terminal**. This lane therefore does not have to argue about power from first
principles or datasheet numbers — it can weigh the machine before and after. That is the
difference between a power claim and a power *finding*, and it is why M1 below comes first
and is non-negotiable.

## Milestones

### M1 — power accounting baseline (READ-ONLY, zero hazard). Do this first.

Turn the SMC reading into an instrument that can falsify something:

1. **Derive the sign convention of `amp` empirically** — which sign is discharge — and say
   how you determined it. Do not assume it.
2. **State what `amp` reads in the healthy-but-idle case**, and what it reads while charging.
   This is a standing project law: a counter whose healthy-but-idle reading is
   indistinguishable from its interesting reading cannot falsify anything. A terminal current
   on AC is dominated by charging and is **not** a measure of system draw — so every power
   measurement in this lane is only meaningful **unplugged**, and the witness must say which
   state it was taken in rather than leaving a reader to infer it.
3. Add a `:: PWR: ::` rollup over a window — mean and spread of draw, sample count, and the
   `ac=` state — so a boot produces a number that can be compared against another boot.
   Follow the existing witness discipline: raw counters, never percentages; a delta with the
   cumulative carried alongside; and an admissibility predicate stated in the witness itself.
4. Establish the **baseline draw** of a normal boot at idle, on battery. Everything this lane
   proposes afterwards is measured against that number.

M1 alone is worth landing even if nothing else in this brief ever ships, because without it
no power change in UnaOS — this one or any future one — can be shown to have worked.

### M2 — the control surface, as facts (NO WRITES)

Establish, with citation, what can actually be controlled and what each control costs:

1. **gmux**: the display-switch and discrete-power registers, their documented semantics,
   which are reversible, and what each does to a **live scanout**. The indexed protocol,
   the 32-bit read, the version gate and MAX_BRIGHTNESS are already proven on this machine —
   build on that, do not re-derive it.
2. **The iGPU-side prerequisites.** This is the load-bearing part of the brief. The panel is
   scanned by the **Kepler**, from the GOP framebuffer at phys `0x90020000`. The Intel HD 4000
   has been observed **all-dead at all four trace points** — pipes, planes and `DP_A=0x1C`
   constant — at every stage we can see, pre- and post-ExitBootServices. So switching the mux
   to the iGPU today would hand the panel to an engine with no live pipe, and the panel would
   go **black**. Enumerate honestly what would have to be brought up first: pipe
   configuration, plane + surface + stride, `DP_A`/eDP, and panel power (`PP_CONTROL` /
   `PP_STATUS` — `igpu.rs` already carries the register definitions and has never driven them).
3. **Assess a cheaper partial win, honestly.** The full switch is a multi-pull arc. Ask
   whether a smaller, reversible reduction exists that does *not* require handing over the
   panel — and if the answer is no, say so plainly rather than inventing one. A brief that
   returns "the cheap win does not exist, here is what the real one costs" is a successful
   brief.

### M3 — experiment plan (PROPOSAL ONLY — do not execute)

A staged, reversible experiment with an explicit hazard analysis and a restore path for each
step. **No gmux write is to be performed without Peter's explicit go**, given separately from
approval of this brief.

## ⛔ Hazards — read before proposing anything that writes

- **A wrong write blanks the panel.** Switching the mux, or powering down the discrete GPU,
  while the Kepler is scanning the GOP surface takes the display away from the only engine
  currently able to drive it.
- **Serial is the lifeline, and it survives a black panel.** The rMBP console is FTDI,
  kernel-TX-only, on the machine's own USB port — independent of the display path entirely.
  Any experiment in this lane runs **with serial attached and captured**, or it does not run.
  A blank panel with serial is a finding; a blank panel without serial is an unrecoverable
  boot, which is the one thing this bench treats as unaffordable.
- **STOP tripwire.** If a power reduction would require disabling or weakening a protection,
  or would leave the machine unable to reach a lit panel with no proven restore path — stop,
  record exactly what you observed, and report. Do not improvise around it.
- Nothing in M1 or M2 writes to the gmux, the display engine, or the SMC.

## Laws for this pull

- **Cleanroom.** Hardware facts with attribution are this lane's standing rule (it is how the
  gmux variant table was established in pull 5). **No GPL code bodies.**
- **Gate: `./arroyo check`, both arches, only.** Per Peter this sitting the QEMU suites are
  dropped — do NOT run `test` or `test-fat`. QEMU has no gmux, no SMC and no panel; it can
  say nothing about any of this. Metal is the verdict.
- **Verify the symbols are IN the artifact** before staging (the s42 INSTGUI lesson: a knob
  added only to `arroyo` is invisible to `builder/`, so the feature ships disabled while every
  check stays green).
- **Read before write; raw before decoded; decode stays gated on a passed self-test** — the
  discipline from pulls 4 and 5 that produced the proven gmux protocol. It is why this lane's
  canon is trustworthy; keep it.

## Owed

Metal: the next iGPU sitting (the lane's last was #10; the log is now at #42). Report to the
coordinator seat (x86/GR9) as `PROPOSAL-igpu-pull6.md` in this directory.
