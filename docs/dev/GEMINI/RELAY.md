# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pulls 20 + 27 APPROVED, one binding amendment each)

## → kepler-fence session

Fence: pull 27 proposal APPROVED with ONE BINDING AMENDMENT. First, the good part: I re-derived your byte stream independently and it packs to exactly the eight constants you listed (word 6 = f7 f8 02 + pad = 0002f8f7 — the "30 bytes" in your prose is 27, cosmetic only). mov $r3,0 + sethi $r3,0x50 gives 0x500000 iterations because sethi replaces the high half, and the branch target 0x0016 − 9 = 0x000d lands exactly on the `add`, so the loop closes correctly. Your MAILBOX1 port derivation is right too: (0x044 & 0xffc) << 6 = 0x1100.

THE AMENDMENT: the entire safety argument for this pull is that the loop terminates — so observe it. After the witness block and a short settle, add a third reading: `:: kepler: hb final mb1=XXXXXXXX cpuctl=XXXXXXXX ::`. cpuctl showing STOPPED with mb1 frozen means the bound held and the engine parked cleanly; still advancing means the loop outlasts the boot window — report that honestly rather than claiming a clean bound we never saw. Two reads.

Everything else stands: image A first as the known-good execution witness, HB started without polling, witness sequence byte-for-byte unchanged. Commit ALL docs+code, no push. Report "PUSH OWED: 13". (I run all builds and gates.)

## → kepler-display session

Display: pull 20 proposal APPROVED with ONE BINDING AMENDMENT — and first, the lane call was exactly right. The fbcon stride fix IS outside your lane, you stopped and routed it instead of reaching, and that's precisely what the brief asked for. I'll take that half.

THE AMENDMENT: don't presume the number you're about to measure. Your prose states as fact that GOP reports stride = 2880 px / 11520 B — we have never actually read it, and that's what `fbcon-view` is for. Reword it as a hypothesis and let the printed values decide. If GOP already reports 4096 px, then the console's problem lies elsewhere and THAT becomes this pull's finding, which is just as valuable.

Verified for you so you don't have to guess: `crate::video::fbcon::current_info()` exists (fbcon.rs:260, returns `unaos_boot_info::FrameBufferInfo`), so your read compiles as written. Everything else as proposed — measure, print the comparison, then the 8×8 glyph-shaped blocks at a console-like origin using the true 16384 pitch, calibration pattern and hold unchanged before it. Commit ALL docs+code, no push. Report "PUSH OWED: n". (I run all builds and gates.)
