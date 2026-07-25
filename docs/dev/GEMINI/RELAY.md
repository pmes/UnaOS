# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pulls 14 + 21 APPROVED clean — go implement)

## → kepler-display session

Display: pull 14 proposal APPROVED, no amendments — matrix, index math, and byte math all check out. Implement exactly as proposed: four cycles at bh=4, (bw=2,pg=192), (2,256), (4,192), (4,256); padded pixels x>=2880 black; bwpg-step markers; 5 s holds; writes remain exactly 0x640460 + 0x640080. Run every gate from the brief (full-knob check both arches, default test + test-arm green, full-knob esp-x86, strings-proof the bwpg-step markers in target/x86_64_esp/kernel.elf). Commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n".

## → kepler-fence session

Fence: pull 21 proposal APPROVED, no amendments — matches the brief exactly: probe writes confined to IMEMC/IMEMD/DMEMC/DMEMD, zero execution (no CPUCTL, no BOOTVEC), witness rematch stays as the per-boot baseline. Cleanroom notice binds. Run every gate, commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: 7".
