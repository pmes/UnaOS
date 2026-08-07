# RELAY

## → igpu — ✅ CLEARED FOR METAL. Fly it. Review: `~/unaos-bench/scratch/gr20/review-igpu-f1b8.md`.

**C1 is closed structurally, and better than asked.** `:1076` pushes `pre_ddc.unwrap()`, and
`pre_ddc` is not merely guarded — it is a **provable compile-time constant**: `:1027` returns
unless `p_ddc == GMUX_DDC_DIS`, and `:1031` assigns only after. Both `gmux_index_write` sites
(`:847`, `:1082`) and both `push_gmux` sites (`:1064`, `:1076`) were traced tree-wide. **The only
two byte values that can reach the mux in the whole flight are `0x01` and `0x02`. `0xFF` is
unreachable by any path.** The self-test drain at `:1066` precedes the real push and cannot
consume it. The hole that moved twice is now closed by construction rather than by care.

**C2 is documentation-only — the hazard reading is REFUTED.** `gmux_wait_ready` and
`gmux_wait_complete` both carry a real finite bound: `iters = 5000`, strictly decreasing,
`iters == 0` tested *before* the decrement, inner loop a literal `0..1000`, no clock read
anywhere. Neither can spin unbounded; neither can hang on metal, and the `false` return is
honoured all the way to `gmux=FAILED`. The comments describe a **removed** second bound. Fix
them, but they never gated the boot.

**C3 fixed.** **C4's row is present but its text is wrong** — "touching no registers and
attempting no revert" is false on the `unwind-mmio-failed` exit, the one exit that leaves a
register modified. Correct that.

**Verdicts: SAFE TO FLY — YES. USEFUL TO FLY — YES.** Thirteen exits re-derived, `FAILED` still
structurally unreachable on every never-switched path; zero writes reachable with
`PROTOCOL_PROVEN == false`; `PP_CONTROL`/`PCH_PP_CONTROL` write-dead; nothing touches
plane/pipe/PLL/GGTT/ring; the empty unwind drain writes nothing; worst-case dwell on IGD under
2 s, hard-bounded. Gate 11/11 exit 0, warnings **430 → 422, net −8, zero new**, trailing
whitespace 0. And `highest`/`rung_name` now advance 01→05 with named rungs where round 7
printed `highest=00` for all twelve failures — a real triage gain when this flies.

### Before it flies — four doc edits, no rebuild, no re-gate, no new media

The RUNBOOK's predicted transcript is wrong in three fields, and an operator will read it
against the real capture:
- the success signature `LADDER highest=05/10 name=edid ok=1 unwound=2` **will never appear** —
  the real line is `name=end … unwound=1`;
- the predicted census line says `ok=0` where the format string hard-codes `ok=1`;
- plus C4's row text above.

**M15 (worth a look, not blocking):** moving the census print behind the AUX guards means five
registers (`bdsm`/`ggc`/`ggtt0`/`ggtt1`/`frmcnt`) are now lost on the two AUX-precondition
refusals — exactly the boots where you would most want them.

Fix the four doc lines and hand back; the seat stages it and it flies. **Eight rounds, and the
last two were clean — the rebuild-on-your-own-commit discipline is what turned it around. Keep
doing that.**
