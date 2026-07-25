# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25, after s28boot1: GOP overlap found — display results were confounded; ucode blocker named; pulls 19 + 26 briefed)

## → kepler-display session

Display: s28 verdict — your probe answered a bigger question than it was asked, and the answer is half bad news, half the thing we've been chasing since s17.

Peter watched the wire live: THE GRAPHIC CAME UP BEFORE `pm-step fill done` PRINTED — during the fill, pixels landing as we wrote them, a full cycle before the latch. And your reg-dump closes it independently: armed=00000200 and shadow=00000200 at both t=1 and t=5, and 0x200 << 8 = VRAM 0x20000 = the GOP framebuffer. The head was scanning the firmware's surface the whole time and never took our pointer — which is the s15 "0x6101E0 never follows" puzzle, finally explained.

So: the EVO arm+UPDATE path has NEVER repointed scanout. s17's first pixels were real pixels by the wrong mechanism — direct painting into the firmware framebuffer, which our scratch surface overlaps at row 1400. The block-linear era (s18–s26) was aliasing against the GOP's own linear/16384 layout; "mapping solved" was us matching that layout rather than decoding the hardware's. I'd rather say that plainly than let it stand.

The other half: your barcode, bands and diagonal all rendered with correct geometry on a real panel. THAT MEANS WE ALREADY HAVE A WORKING FRAMEBUFFER — linear, pitch 16384, into the GOP FB. Pull 19 is re-scoped to make it first-class (ignore the earlier "relocate to prove the latch" draft; that question is settled): git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-display-pull19-relocate-decisive.md` — base the fill at gop_vram_offset, all 1800 rows, full width, and REMOVE the latch from the path entirely (nothing to restore; we're drawing on the surface already being scanned). Keep your pattern as-is: it becomes a full-panel calibration target — fiducials top and bottom, band 0 at the top, diagonal corner to corner. One hold, one photo. After that lands, we wire fbcon to it and the kernel console renders on the panel. Commit ALL docs+code, no push. Report "PUSH OWED: n". (I run all builds and gates.)

## → kepler-fence session

Fence: s28 verdict — everything up to execution now works, and the blocker is named by data you already collected. Both images verified byte-exact in IMEM; `tlb page0=01000000` means the page-pad made page 0 usable; CPUCTL bit 6 is clear so writing 0x100 directly was right. But cpuctl went 00000010 → 00000012 and mailbox0 never left the A5A50000 seed: per rnndb that's START_TRIGGER latched (bit 1) with STOPPED still set (bit 4) — the trigger took and the core refused to run.

Your own post-sweep says why: **DMACTL (base+0x10C) = 0x00000001 — REQUIRE_CTX is SET.** The Falcon wants a bound context before it will execute; the scrub bits are clear, consistent with your IMEM writes landing. Nouveau clears exactly this bit on the no-context path. Pull 26 is briefed — git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull26-dmactl-require-ctx.md`: print DMACTL pre, mask-clear bit 0, print post, REFUSED marker + skip if it won't clear, then re-run image A exactly as landed. One new write. If mailbox0 moves off the seed, that is the first UnaOS-authored code ever executed on GPU silicon. If it still won't run with DMACTL clear, the next step is a READ-ONLY recon of the engine reset/context group (0x3C0, and the 0x048/0x054/0x480 aperture registers) — propose that, not blind writes. Commit ALL docs+code, no push. Report "PUSH OWED: n". (I run all builds and gates.)
