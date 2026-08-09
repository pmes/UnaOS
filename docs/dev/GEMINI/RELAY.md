# RELAY — GR23 (x86 seat → lanes). This file is a clipboard: each pass REPLACES it whole.

## kepler — FINISH THE FENCE FIX AND COMMIT CLEAN. You are not review-ready.

Your worktree sits at `c385bd19` with **uncommitted `kepler.rs` edits and four stray
`fix_*.py` scripts at the repo root** — the 5-hour cutoff left the fix half-applied and
dirty. Peter expects FENCE ready for review. Do, in order:

1. Apply the full bounce list — all six, they were one verdict: upload writes go to
   **IMEMD** (not IMEMC(1)) with **AINCW set**; write the **IMEMT tag**; **restore the
   readback verify** that was deleted (both legs currently run whatever was already in
   IMEM — the ucode is byte-correct and has NEVER been uploaded); fix the three host
   observables read from wrong registers; un-invert the poll gate; move the in-ucode
   CHAN_VALID clear so it lands AFTER PFIFO evaluates.
2. Delete the `fix_*.py` scripts — patch scripts do not ship in the tree.
3. ONE clean commit on your branch. No push. Report "FENCE ready for review" with the
   sha and the readback-verify's witness line quoted from a QEMU run (compile witness at
   minimum — QEMU has no Kepler, say so honestly).
4. Standing constraint unchanged: RAMFC constants are UNAUDITED (CLEAN_ROOM_POLICY §5) —
   every doc/comment touching them must say so.

## igpu — round 13 (`8b509d48`) BOUNCED. Do not fly. Fix, commit, report for re-review.

What checked out: gmux encodings vs apple-gmux exact; AUX port A justified from the PRM
cite; PCH PPS offsets; unwind LIFO order; mux readback before AUX; single funnel to
`unwind.execute()`. The bounce is narrow and every item is cheap:

**Blocking (all five before re-review):**
1. **F1 CRITICAL — nothing prints inside the blanked window.** The 137-invocation EDID
   dump (`igpu.rs:1159-1165`) and every error print at `:1125/:1133/:1142/:1149/:1155/
   :1177` run with DISPLAY on IGD; `_print`'s ring-full branch spins up to 1e6 turns with
   interrupts masked, per call. The `edid` buffer is complete at `:1137` — move EVERY
   print after `unwind.execute()` at `:1183`. On the failure path too.
2. **F2 CRITICAL — rewrite `RUNBOOK-gmux-igd.md` for a BLANKING flight.** It still says
   "THE PANEL SHOULD REMAIN ON" / "DDC ONLY" / `highest=05/10` — the operator's decision
   table routes a by-design blank toward a nonexistent fault.
3. **F3 — restore witness weakened:** post-restore reads only 0x10 (the latch), no longer
   0x11 (the mux). Read BOTH, print both; MATCH requires `0x11 == DIS`.
4. **F4 — validate `p_disp` before trusting it:** a `gmux_index_read` timeout stores
   `0xFFFFFFFF` and the unwind then WRITES 0xFF to GMUX_SWITCH_DISPLAY. Add
   `|| p_disp != GMUX_DISPLAY_DIS as u32` to the `:1056` gate.
5. **F5 — the prediction, written down, and the instrument to discriminate it:** sample
   `PCH_PP_STATUS`/`PP_CONTROL` immediately before AND after the AUX attempt (reads only),
   and state the table: mux-didn't-take vs AUX-dead vs VDD-off. Name F12's confound
   explicitly: EXTERNAL stays on DIS (upstream switches all three) — an accepted, named
   deviation, or switch it too and say why.

**Cheap, take in the same pass:** F6 print the two mux readbacks on `mux-switch-failed`;
F7 push GMUX_SWITCH_DISPLAY into the unwind self-test; F8 one window-scoped deadline over
the whole switched region (10 AUX transfers × 4 s each is a 40 s worst-case dark panel);
F9 print `p_disp` (0x10), the value actually restored; F10 `highest=06/10` vs the ladder's
five rungs; F11 delete the dead `GMUX_DWELL_MS`.
