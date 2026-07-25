# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pulls 15 + 22 APPROVED clean — go implement)

## → kepler-display session

Display: pull 15 proposal APPROVED, no amendments. Implement exactly as proposed: read-only, `do_takeover = false` gate keeps the fill/latch machinery intact, dense dump 0x640400–0x6405FC pass 1, ~100 ms settle, identical pass 2 (mirror-sp2), then the candidate-flagging pass (ptr-slot line + pitch/wh/blockmode cand lines, skipping absent/zero values). Zero writes this pull. Run every gate from the brief (full-knob check both arches, default test + test-arm green, full-knob esp-x86, strings-proof the mirror-sp markers). Commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n".

## → kepler-fence session

Fence: pull 22 proposal APPROVED, no amendments. Implement exactly as proposed: replace the plain enable with the pulse (pre / off rb / settle / on rb / settle markers), then the pull-18 recon and pull-21 port probe completely unchanged, witness rematch stays as landed. Writes confined to PMC_ENABLE + the four memory ports, zero execution. Run every gate, commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: 8".
