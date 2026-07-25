# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25, after s22boot1: pgraph enable TOOK; fence pull 20 briefed; display verdict awaits photos)

## → kepler-fence session

Fence: s22 verdict — YOUR ENABLE TOOK. pre=E011216D → rb=E011316D, bit 12 stuck, not refused. The all-BADF1200 wall is GONE: post-enable the block reads BADF1000 interleaved with REAL ZEROS (cpuctl=00000000 — Falcon present, halted, no ucode; pgraph stat off 050–064/074/078 read zero; imemc/dmemc still gated). Both passes identical and stable. The engine exists on the pri bus now. Pull 20 is briefed and it's the decisive one: re-run your s7–s10 channel witness sequence VERBATIM with the engine on — zero new register writes, resequence only, keep your original witness markers and add a framing pair `:: kepler: witness-rematch begin (pgraph on) ::` / `:: kepler: witness-rematch end err=X stat=X valid=X ::`. If PFIFO stops stripping VALID, the fence wall ends with no ucode work; if it still strips, that's the cleanest refutation yet and the ucode arc begins. git pull for the full brief (BRIEF-kepler-fence-pull20-witness-rematch.md). Proposal first. PUSH OWED reminder stands.

## → kepler-display session

Display: s22 verdict — your four pa-step cycles ran perfectly on metal (bytes per cycle exactly as computed), and the photos rule: PITCH ALIGNMENT IS REFUTED. At fixed bh, pg=192 and pg=256 gave IDENTICAL seam geometry — same count, same x positions (clearest at bh8: two seams at ~1/3 and ~2/3 in both). Seam count still scales with bh (~6–7 @bh4, 2–3 @bh8). GOB 64B×8 and block stacking stand; the surviving suspect is BLOCK WIDTH > 1 GOB. Pull 13 is briefed — git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-display-pull13-blockwidth.md`: four cycles, bw {2,4} × bh {4,8}, pitch back to natural 180 GOBs/row (pad to a multiple of bw with black), propose your exact within-block GOB ordering (x-fastest unless envytools says otherwise — cite if so). Proposal first. PUSH OWED reminder stands.
