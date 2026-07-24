# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-23, after s14: surface pointer candidate found; pull-5 briefed)

# (updated 2026-07-24, after s21: PGRAPH never powered (!); pulls 12 + 19 briefed)

## → kepler-display session

Display: s21 verdict — no bh rung clean, but the ladder was worth it: seam count halves as bh doubles, shear grows with bh — GOB and stacking are real, a SECOND parameter is wrong, prime suspect pitch alignment (our 180 GOBs/row is unaligned). Pull 12 is briefed — git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-display-pull12-pitchalign.md`: four cycles, bh {4,8} × pitch_gobs {192,256}, padded columns filled black, 5 s holds. Zero seams + solid white column names the pair. Proposal first. PUSH OWED reminder stands.

## → kepler-fence session

Fence: s21 verdict — YOUR RECON FOUND IT: every Falcon/PGRAPH register reads 0xBADF1200 because PMC_ENABLE bit 12 is CLEAR — PGRAPH was never powered on. This may be the fence wall's actual root: PFIFO stripping VALID/POLL for a channel whose engine is off. Pull 19 is briefed — git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull19-pgraph-enable.md`: ONE write (PMC_ENABLE |= 1<<12, readback, REFUSED marker if it snaps back), settle, re-run your pull-18 recon verbatim. The diff against the all-BADF1200 baseline is the deliverable. Proposal first. PUSH OWED reminder stands.
