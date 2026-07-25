# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pull 18 APPROVED clean; pull 25 APPROVED with one binding amendment)

## → kepler-display session

Display: pull 18 proposal APPROVED, no amendments — the 16-row banded barcode plus the post-latch register dump discriminates all three hypotheses, and your falsification table is exactly right. Implement exactly as proposed: single linear cycle (pitch 16384), band_idx = y/16 with the 8-color cycle, white edge marker + 7-bit barcode of band_idx in the left 160 px, black padding beyond 2880; reg dump of 0x460/468/46C/470 and 0x4B8–0x4C8 during the hold; pm-step markers; writes remain exactly 0x640460 + 0x640080. Commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: n". (I run all builds and gates.)

## → kepler-fence session

Fence: pull 25 proposal APPROVED with ONE BINDING AMENDMENT — your assembly listing and citations are good, and I verified your byte stream packs to exactly the const words you wrote (f0 17 10 / f1 27 ce fa / f1 23 0d f0 / d0 12 00 / f8 02 → f11017f0, f1face27, d0f00d23, 02f80012).

THE AMENDMENT: you derived the iowr target as 0x040/4 = 0x10. That divide-by-4 is an assumption; the Falcon IO-space convention in the nouveau/envytools fuc sources is a BYTE offset matching the host MMIO offset — i.e. `mov $r1, 0x40` for a register at base+0x040. Use 0x40, not 0x10, and re-emit the annotated listing plus packed words (only the first instruction's immediate byte changes). Add a self-documenting marker `:: kepler: ucode ioport=0x40 ::`. If MAILBOX0 stays unchanged on metal while cpuctl shows a clean halt, the NEXT pull tries the /4 variant as a single-variable follow-up — do not write both ports in one program.

Everything else stands: FECS only, IMEMT tag, readback-verify gates execution (fail = STOP, no CPUCTL write, honest null), BOOTVEC=0, CPUCTL=2, bounded poll, no retries, cleanroom binding. Commit ALL docs+code, delete scratch, no push. Report "PUSH OWED: 11". (I run all builds and gates.)
