# WHITE BOARD — 2026-08-06 (GR17)

Two questions. Each carries only the background needed to answer it.

---

## 1. `read_pixel` wastes 2 of every 3 PCIe reads — who lands the fix?

`video/framebuffer.rs::read_pixel` fetches a pixel as three separate `read_volatile::<u8>`
calls. On cacheable RAM that's free; on the Kepler BAR (WC-mapped, uncached reads) each u8
read is its own non-posted PCIe transaction at ~976 ns. One `read_volatile::<u32>` fetches
all four bytes in one transaction — same bytes, same decode, alpha masked as today, coverage
identical.

- Effect: wc-d's verify drops 2.84 s → ~0.95 s per witness boot. Every future glass read
  gains the same 3×. (wc-g no longer cares — its own in-lane bulk path already landed.)
- It is shared kernel-core: the Pi framebuffer is cacheable, so behaviorally neutral there,
  but wc-d lines are matched by the pi4 gate — values shouldn't change, key order untouched.
- The question: me now (out-of-lane sanction), the integrator at merge, or Gemini?

## 2. Witness builds pay ~6 ms per kernel print once the console routes — investigate now, or park?

Measured on the existing s73 capture (no new boot needed to confirm it exists — only to
explain it): the same 229 bring-up lines cost 159 ms total witness-off vs 1438 ms
witness-armed. 0.69 vs 6.28 ms/line, starting exactly at `[wc-x] console-route first-paint`.

- 6.28 ms/line is what a ~70-byte line costs if something drains the UART synchronously;
  0.69 ms/line says witness-off prints don't wait. So the suspect is witness-build serial
  synchrony, or each print paying a console-window present — mechanism unconfirmed either
  way, and the code that decides it (fbcon/serial) is outside `wcg.rs`.
- Why it's worth your call: after the wc-g reshape this tax is the second-largest witness
  cost left (~1.29 s of the kepler block).
- The question: assign it (me, Gemini, next arc) or park it until Boots P and Q have flown?
