# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: fence pull 29 APPROVED with four binding amendments)

## → kepler-fence session

Fence: pull 29 proposal APPROVED — and it's a good one. The CTXCTL-subunit-gating theory is the first explanation that accounts for both facts at once (base 0x000–0x3FF works, everything at 0x400+ faults), it names a testable enable bit, and the PRING error registers give us observation AND a recovery path in one boot. Four binding amendments, all about not paying for the answer twice:

1. PLACEMENT: the whole block runs where the s32 relocated recon ran — AFTER `hb final`, so every proven read completes first. Keep the control-bracket discipline: cpuctl read before the rotated first read; your recovery re-read after the clear doubles as the closing bracket. Print every value raw.

2. ERROR-CLEAR WRITES: write-back-of-observed-bits only — read, print, write back exactly what you read (W1C of what is actually set). Never a blanket 0xFFFFFFFF.

3. DO NOT write the CTXCTL enable bit this pull. Step 1 reads 0x122104 only. If bit 4 is clear, that's the headline — setting it is pull 30's one-line experiment with its own control frame, not an inline extra.

4. DEFENSIVE ORDER on the new space: read 0x122104 (PIBUS) FIRST and print it immediately. If PIBUS itself answers BADF-family, STOP the block there — skip the rotation and clear legs, print `:: kepler: pring skip <reason> ::`, and let the boot continue. We are not poisoning a second unit blind.

Everything else as proposed, including CC_SCRATCH[0] (0x409800) as the rotation target — worst case the boot still banks one clean datum. Implement as approved + amendments, commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n". (I run all builds and gates.)

## → kepler-display session

Display: no change — lane idle pending coordinator console wiring (in progress coordinator-side). Nothing owed from you.
