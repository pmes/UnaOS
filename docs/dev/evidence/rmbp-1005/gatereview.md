# GATEREVIEW (rmbp-ledger B457) — design

**Finding.** Eleven gates run on this tree, and none had been checked for what it misses. Every gate was given a
planted defect (fixture or temporary tree edit, reverted), and the exit code was recorded. 24 plants: 17 pass a gate
that should refuse them. Three gates are red on the clean tip (knob-hygiene `DEAD: wc_gpublit`, deps-audit `objc2`
lock lag, GATE-LEDGER 218 of 384 findings from a shallow clone), so a new red there looks the same as the old one.
Appearance-check, prefs-schema-check and deps-audit run in no `check` path. Before the first `tests` fires, flight 25
boot 1 prints 26 verdict tags at boot (R80). Findings: `docs/dev/review/GATES-2026-10-06.md`.
**Seam.** Review-only, on host gates. The one kernel edit fixes a vacuous PASS: `tests keplerlog` passed
unconditionally. It sits at the tail of `serial_line.rs`. No new file and no knob.
**Milestones.** M1 this design, the findings file and the plant table. M2.. one commit per patch finding
(`gatereview: F<n> — …`), each under 30 lines, each with a plant added to its gate's selftest/control.
**Witness.** The host gates are their own witness: each selftest exits 0 and catches its new plant. On metal, the
next flight reads `:: KEPLERLOG: lines=<n> kept=<k> dropped=<d> shown=<s> -> PASS ::` from `tests keplerlog`. A boot
with no fold reads `-> SKIP reason=no-fold`.
**Owed.** The arcs named in the findings file. The standing reds (KNOBRETIRE, DEPSLAG) need the seat, because
the seat's metal line names `wc_gpublit`.
