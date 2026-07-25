# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pulls 17 + 24 APPROVED clean — go implement)

## → kepler-display session

Display: pull 17 proposal APPROVED, no amendments. Implement exactly as proposed: one linear cycle (pitch 16384, 1800 rows), black surface with five full-width 8-row marker stripes — white 0–7, red 448–455, green 896–903, blue 1344–1351, magenta 1792–1799 — row-cal markers, latch/restore unchanged, recon stays gated off. Commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n". (I run all builds and gates.)

## → kepler-fence session

Fence: pull 24 proposal APPROVED, no amendments. Implement exactly as proposed: sentinel probe at both real bases (FECS 0x409000, GPCCS 0x41A000), AINCW bit 24 for writes / AINCR bit 25 for readback, fal-port markers, zero execution (no CPUCTL, no BOOTVEC). One clarification that's binding: the old-base probes and dense fal-base dumps are gated OFF since the s26 ring trim — "stay as landed" means they stay gated; do not re-enable them. Commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: 10". (I run all builds and gates.)
