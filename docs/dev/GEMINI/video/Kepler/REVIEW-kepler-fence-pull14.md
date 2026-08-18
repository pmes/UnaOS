# REVIEW — kepler-fence pull 14 proposal: APPROVED WITH AMENDMENTS (2026-07-23)

Sound overall; the CTRL_ADDR audit is exactly the pre-committed fallback.
Implement per the proposal WITH these four amendments (binding):

1. **One variable at a time.** Never leave two PBDMAs modified simultaneously.
   Iterate PBDMA-by-PBDMA: modify one, run the ladder, restore it, then move
   to the next. "Write to all three to be certain" is rejected — a
   simultaneous multi-PBDMA change can't attribute a result and risks
   pointing more than one fetch unit at a bogus target at once.
2. **Stop on PASS.** If any step prints `WITNESS PASSED - bits stuck!`,
   freeze — no further TARGET steps, no restore of the passing word. Print
   the full ladder + discriminators and exit the experiment leaving the
   passing state intact for the capture.
3. **Readback discipline.** After each TARGET write, if `rb != wrote`, print
   `:: kepler: ctrladdr pbdma<N> RO? wrote=XXXXXXXX rb=XXXXXXXX ::` and skip
   the ladder for that step (a read-only or masked field refutes nothing).
4. **Milestone 2 is READ-ONLY this pull.** The PDISPLAY/EVO recon may read
   and log words only. Any disp-side WRITE crosses into the display engine
   (other lane's territory) and needs its own brief through the coordinator —
   delete the "if writes are attempted" branch from scope.

Also note in your report, not code: the TARGET bit positions (0:1) come from
rnndb marked XXX-unconfirmed — treat them as the hypothesis under test, and
say so in the capture-facing markers' surrounding report (absence-honesty
applies to semantics too).
