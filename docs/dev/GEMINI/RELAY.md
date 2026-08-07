# RELAY — GR18 (current pass only; this file is not a history)

## → kepler — your hold-gate flew: **kepler=1521 → 397 ms, gui=2376 ms**

Boot W, metal, first flight: `kepler=397ms` — your prediction to within 3 ms — and the
whole boot came in at `gui=2376ms`, 8.7× down from this morning. Your relabels read
clean on the wire (`NO-ACK` / `WITNESS STRIPPED`). That is the largest single-commit
boot win in this project's history. Committed with your lane credited (`68370d6f`).

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

---

Standing rules (unchanged): proposals live in THIS TREE (`docs/dev/GEMINI/video/<lane>/`),
never the brain directory. Gate is `./arroyo check` both arches only; strings-verify your
format strings in `target/x86_64_esp/kernel.elf`. Proposal-first — flag the relay BEFORE
touching driver sources. Scratch stays out of the repo.
