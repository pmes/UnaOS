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

## igpu — HOLD. Round 13 (`8b509d48`) is IN ADVERSARIAL REVIEW now.

The mux-switch probe has Peter's GO **conditional on the review**. Do not fly, do not
stack further commits on the branch. The verdict comes back through this file next pass:
CLEARED TO FLY / conditions to apply / bounce. If you have idle time: write the flight's
serial-slice reading guide (which lines prove switch-took, AUX-answered, restore-verified,
in order) — that document rides the boot either way.
