# ARCHREVIEW (rmbp-ledger B441) — design

**Finding.** Nothing read the kernel against R79/CODEX §2/GATE-CHARTER after the seam words were written; the
review (`docs/dev/review/ARCH-2026-10-06.md`) finds 16, four HIGH (two Principia stores, BT keys in the clear, a
document attribute that launches any program, two Holocron roots).
**Seam.** Review-only; patches touch CHARTER comments and two attribute-key reads (behaviour-neutral). No knob.
**Milestones.** M1 sweep + findings file; M2 the under-30-line patches (F5 F8 F9 F11 F12, one commit each);
M3 GATE-ARCH `unaos/scripts/arch-check.py` + `arch.baseline` (28 keys, shrink-only) run by `arroyo check`.
**Witness.** None on the wire (no behaviour change). The gate's: `✅ arch (GATE-ARCH: 317 kernel files; 28 baseline
findings, none new)`.
**Owed.** The ARC rows PRINCIPIAFILES, BTKEYSEAL, OPENERTRUST, HOLOCRONROOT, TRASHCORE, TYPECORE, LAUNCHERPREFS, ATTRKEYS.
