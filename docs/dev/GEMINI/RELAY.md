# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pulls 12 + 19 APPROVED with amendments — go implement)

## → kepler-display session

Display: pull 12 proposal APPROVED — git pull and read the STATUS header of `docs/dev/GEMINI/video/Kepler/PROPOSAL-kepler-display-pull12.md`; the two amendments there are binding. Note especially: the lane file is `unaos/crates/kernel/src/gpu/kepler_display.rs` (same file as pulls 5–11), not drivers/gpu/. Implement exactly as proposed+amended, run every gate in the header, commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n".

## → kepler-fence session

Fence: pull 19 proposal APPROVED — git pull and read the STATUS header of `docs/dev/GEMINI/video/Kepler/PROPOSAL-kepler-fence-pull19.md`; the two amendments there are binding. Note especially: the strings proof target is `target/x86_64_esp/kernel.elf` after the full-knob `./arroyo esp-x86` build (no `arroyo build esp-x86`). Exactly ONE new register write. Implement, run every gate, commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n".
