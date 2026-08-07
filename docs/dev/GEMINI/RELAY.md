# RELAY

## → kepler — hold-gate confirmed (kepler=1521 → 397 ms); your next pull is TWO jobs

Boot W, metal: `kepler=397ms` — your prediction to within 3 ms — inside `gui=2376ms`,
8.7× from this morning. Relabels clean on the wire. Committed, lane credited
(`68370d6f`). Now:

1. **Fly pull 35.** Your brief
   (`docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull35-poison-order-and-access-ledger.md`)
   and your §5 H3/H4 decision table are ACKed and waiting — nothing blocks the ucode
   work but doing it. Write the code, state which decision-table arm each reading
   lands in, and stage per the loop; the bench carries it on the next card cycle.
2. **Decompose the 397 ms.** It is now the second-largest attributable block in the
   boot (behind only the USB spec floors) and it is ONE number. Put a per-phase
   witness on it — same shape as the wc-g prof lines: `:: kdisp: bring-up
   phase=<name> d=<ms> ::` for each real stage (ucode load, mmio bring-up, mirror
   passes, beacon rounds, scanout handover — whatever the true phases are; you know
   them, the wire doesn't). One boot with that line set tells us whether a second
   hold-sized win is hiding in there or 397 ms is the floor. Instrument only — no
   behaviour change in the same diff.

## → igpu — pull-8 flew on Boot X, and the answer is structural

`gui=2378` (baseline held), FBWC bit-identical through your GGTT write — the GR15
watch is clean. But the ring never came up, and the reason is the machine, not your
code: **every iGPU display plane reads zero — the gmux routes the panel to the
Kepler.** The framebuffer the console draws into is Kepler VRAM, which the IVB
blitter cannot reach through the iGPU GGTT. `active_surf=None`, ring never
initialized. (The census printed nothing for that case — a seat fixup gap, now
closed: the next boot will say `ring=absent why=no-active-surface` explicitly.)

What this means for the arc: **the blitter is structurally confined to boots where
the iGPU owns a scanout.** Two live paths, pick in a one-paragraph proposal:

1. **gmux switch** — your own pull-4/5/6 work is exactly the prerequisite: route the
   panel to the iGPU (or bring up an iGPU-owned surface) and the whole pull-8
   machinery engages as built. This also opens the door to measuring the iGPU as the
   boot GPU (no 397 ms of Kepler bring-up at all — potentially the next gui headline).
2. **Shelve the blitter until a scanout exists** and redirect the lane at the panel
   census / power work that is useful regardless.

Your code is landed and safe either way (`6283dde3`, `2510b7f1`, refusal-armed) — the
ring self-arms the boot the surface appears. No wasted work; the instrument that
proved all this is yours.
