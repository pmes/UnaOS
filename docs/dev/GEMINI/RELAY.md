# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pulls 12 + 19 APPROVED — full approval text inline, post-ready)

## → kepler-display session

Display: pull 12 proposal APPROVED with two binding amendments:

(1) The lane file is `unaos/crates/kernel/src/gpu/kepler_display.rs` — NOT `drivers/gpu/kepler_display.rs` as your proposal wrote. It is the same file you modified for pulls 5–11; do not create a new path.

(2) The full gate set applies: full-knob `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo check` (both arches), default `./arroyo test` and `./arroyo test-arm` green, then `UNAOS_USBDEBUG=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86`, and strings-proof the new `pa-step` markers in `target/x86_64_esp/kernel.elf`.

Everything else exactly as you proposed: four cycles (bh=4,pg=192), (bh=4,pg=256), (bh=8,pg=192), (bh=8,pg=256); padded pixels x>=2880 filled black; `gobs_per_row = pg` in the index math; bytes value printed in the fill-done marker; 5 s holds, 1 s recovery gaps; writes remain exactly 0x640460 + 0x640080. Implement now, commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n".

## → kepler-fence session

Fence: pull 19 proposal APPROVED with two binding amendments:

(1) Your strings-proof command is wrong: there is no `arroyo build esp-x86` subcommand and no `target/x86_64-unaos` kernel path. The correct proof: build with `UNAOS_USBDEBUG=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86`, then verify the `pgraph-enable` markers appear in `strings target/x86_64_esp/kernel.elf`.

(2) Default `./arroyo test` and `./arroyo test-arm` green are also gates, alongside the full-knob check you listed.

Everything else exactly as you proposed: ONE new register write (PMC_ENABLE |= 1<<12), pre/wrote/rb markers, REFUSED marker + skip-recon branch if bit 12 doesn't stick, ~100 ms spin settle, re-run the ENTIRE pull-18 recon unchanged (both passes, same markers), leave PGRAPH enabled. Implement now, commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: 5".
