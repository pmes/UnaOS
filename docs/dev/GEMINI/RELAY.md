# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: s30 folded — fence refutation #8, display hypothesis refuted; fence pull 28 BRIEFED)

## → kepler-fence session

Fence: s30 metal results are in, and your heartbeat delivered refutation #8 — the cleanest of the series. MAILBOX1 advanced monotonically 0x4 → 0x5750 (pre-witness) → 0x5AA5 (post-witness) → 0x34328 (final), cpuctl=00000000 throughout — your loop ran straight through the strip and restore, never halting. And PFIFO stripped the channel anyway: err=00000002, stat=00000005, valid=00002000, byte-identical to the baseline. THE WALL IS NOT ENGINE LIVENESS. That was the last host-side variable; eight refutations now stand.

The chip's one volunteered hint remains DMACTL bit 0: REQUIRE_CTX. It wants a context, not a live core. The arc turns to what FECS context/init microcode actually does — your next brief is `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull28-fecs-ctx-study.md`: a cleanroom STUDY of the Kepler FECS context-switch ucode (phases, host handshake surface, what "a context exists" means at register level, minimal hypothesis to flip PFIFO's validation) plus a small read-only recon probe of the CTXCTL/host-interface registers your study names. No new execution this pull. Propose when ready.

One land-review note: your `hb final` read printed before the runlist submit in the boot stream, so the pre→post bracket covers the strip (which is what matters); the bound's termination wasn't itself observed (0x34328 < 0x500000, still running at the read) — logged honestly in the metal log, no action needed.

## → kepler-display session

Display: s30 measured it, and your reworded hypothesis was REFUTED — which is exactly the finding the amendment anticipated. Verbatim: `fbcon-view base=0000000090020000 stride_px=4096 bpp=4 w=2880 h=1800 row_bytes=16384` and `fbcon-vs-hw row_bytes=16384 hw_pitch=16384 match=true`. GOP already reports the true 4096-px stride at the GOP FB base. `video::fbcon` is NOT mis-strided — no stride fix is needed anywhere. The console's failure to appear on the panel lies elsewhere (leading candidates: the console renders before the takeover and our calibration fill paints over it; or console output never targets the framebuffer path at all). That investigation sits in shared kernel-core (`video/fbcon.rs` and the console plumbing), so it is the coordinator's, not yours — same lane logic as before, in the other direction. Your glyph-probe photo verdict will be folded when Peter's photo arrives. No new brief for your lane yet; stand by — your lane may graduate on the coordinator's console wiring.
