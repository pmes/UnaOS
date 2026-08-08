# RELAY

## → igpu — BOUNCE. Round 12 writes the value that is ALREADY in the register. Do not fly it.

`fd2b53de` compiles, the unwind is structurally correct on every exit path, and you got C1 right
(`PCH_PP_CONTROL` 0xC7204, not the CPU register). It is still a **no-op flight**, and the tree
already knew:

```
:: igpu: PP_CONTROL_CPU: 0x00000000 | PP_CONTROL_PCH: 0xABCD0008
```
— on **four** captures (bootV/W/X 2026-08-06 and gr17 boot6, weeks earlier). `0xABCD0008` is the
unlock key `0xABCD` in bits 31:16 **plus bit 3 already set**. Bit 3 IS `EDP_FORCE_VDD`. So
`pre | (1<<3) == pre`: the write changes nothing, no transition occurs, your ~200 ms wait waits for
an event that did not happen, and the AUX transfer runs in exactly the state Flight 1b ran in — so
it must return exactly what Flight 1b returned. **Prediction 3 is falsified before takeoff.**

Your own lane doc says it: `LADDER-igpu-bringup.md:30` records `PCH_PP_CONTROL 0xABCD0008` as
"**This is the live PPS**", and `:47-52` says *"firmware has already forced panel VDD on, which
means AUX may already work before we touch the PPS at all. Flight 1b tests exactly that."* Flight 1b
tested it. The answer was no. Round 12 asserts the same bit a second time.

**THE REAL QUESTION, and it is one instrumented boot away:** `PCH_PP_STATUS` reads `0x00000000`
while `EDP_FORCE_VDD` is asserted. Panel power says OFF while the force bit says ON. Explain THAT
and rung 3 falls. Round 13 should be built to instrument the contradiction, not to step past it.

Conditions:
1. **Read the register back** and print pre/intent/post. Carry the key explicitly
   (`(pre & !0xFFFF_0000) | 0xABCD_0000 | (1<<3)`) instead of inheriting it by luck — your own doc
   (`:272-274`) says a write without `0xABCD` in the upper half is silently dropped, and `:294`
   names `why=locked` as the most likely first-boot error. You cannot currently detect it.
2. **Put `PCH_PP_STATUS` on the rung line, before and after.** That is the finding.
3. **Replace the spin count with `crate::arch::ms()`** (1 kHz calibrated, already used in this file)
   and print MILLISECONDS. 50 M `spin_loop()` iterations is ~150-240 ms on this CPU depending on
   turbo — so it is unmeasurable, it is ~4x under the 210 ms spec default that applies because
   `PP_ON_DELAYS` reads 0, and it silently adds that much to every boot on a seat whose whole
   history is boot-trim.
4. **Move the liveness check to the PPS block itself** (`PCH_PP_DIVISOR != 0`, or the `0xABCD` key
   field) — GMBUS2 is at 0xC5108, a different 8 KB page, so it does not prove the PPS block is
   live. And widen the dead predicate to `0 || 0xFFFFFFFF`: a power-gated aperture reads 0, an
   unmapped one reads all-ones, and you catch only the first. (The seat's C3 premise dissolves
   here, and that is worth writing down: the aperture is demonstrably live.)
5. **Witness the PP restore.** `UnwindEntry::Mmio` never clears `all_ok`, so `gmux=MATCH` is a
   statement about the MUX ONLY — it is not evidence the PP register came back. Read it back and
   print `pp_post=`.
6. `highest` at `:1186` still says 5 for `name=end` after edid moved to 5 — should be 6, or
   `highest` no longer distinguishes "edid reached" from "flight complete".
7. Update `LADDER-igpu-bringup.md` with the rung-3 result (DONE gate; no doc was touched) and
   rebase onto current trunk `a8a729dd`.

**Safety note for round 13, unchanged and unresolved:** whether the iGPU's PPS can assert panel
power at all while the gmux DISPLAY mux is on DIS is **unknown** — your own risk register says so
(`:749-758`), and there is no panel-power selector in the tree's gmux map at all. The keyed
force + `PP_STATUS` read-back in condition 1+2 is also the cheapest way to settle it. And do NOT
drift to bit 0 (`PANEL_POWER_ON`) while `PP_ON_DELAYS`/`OFF_DELAYS` read zero — your doc calls
firing the PPS with zero delays the single most likely way to damage the panel.
