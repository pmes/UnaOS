# SMALLFIX4 (rmbp-ledger B466) — the fold's hygiene for merge18/19

**Finding.** Ten hand-back notes from this wave each left one small, named item for the integrator
(ARCHREVIEW F13/F14, TRASHCORE, PRINCIPIAFILES, LAUNCHERPREFS, FWPIN, KEPLERGR, COLUMNSVIEW,
STATUSTRAY, WIREDIET, SECREVIEW F6, knob-hygiene `wc_gpublit`). None is a feature; each is a seam
the arc that found it declined to churn in a shared file.

**Seam.** No new file. Each item lands on the file the hand-back names, at its tail or on the named
line (same-line, code before comment, on the line-sensitive files). F13 is the one clipboard
(`video/clipboard.rs`) gaining a typed file-ref fact; F14 moves the About box from the fs-core
registrar to the app menu (`video/winmenu.rs`), the registrar keeping the facts. TRASHCORE and
PRINCIPIAFILES are not on the cut tip: this branch MERGES them first (two merge commits, one trivial
conflict — the trash.rs CHARTER line takes trashcore's `shared-core`) so items 2 and 3 compile and
gate against the code that earns them; the seat's own later merges of those branches are then no-ops.

**Milestones** (one commit each, `smallfix4: <n> — <what>`): 1 F13+F14 · 2 TRASHCORE fold ·
3 PRINCIPIAFILES + LAUNCHERPREFS gate rows · 4 FWPIN retirements · 5 KEPLERGR doc line ·
6 COLUMNSVIEW hit-tests · 7 STATUSTRAY clock reach · 8 WIREDIET bench note · 9 SECREVIEW F6 ·
10 `wc_gpublit` knob + `tests smallfix4`.

**Witness.** `[clip] set-ref len=<n> ok=1` on a Quarry Copy, `[clip] get-ref len=<n> ref=1` on its
Paste; `[winmenu] about-box name=<n> shown=1` after `[appres] about ...`; `tests smallfix4` →
`:: SMALLFIX4: ... -> PASS ::`. The rest are named per item below.

**Owed.** Whatever an item below marks owed.

## Landed
