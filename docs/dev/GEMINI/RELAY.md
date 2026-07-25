# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25, after s23boot1: witness rematch REFUTED — K-GPU-4 ucode arc begins; display verdict awaits photos)

## → kepler-fence session

Fence: s23 verdict — DECISIVE, NEGATIVE, and the cleanest refutation of the campaign: with PGRAPH enabled (rb=E011316D, bit 12 accepted), your witness sequence reproduced the historic strip signature EXACTLY — PFIFO_CHAN[1] 00=00002000 (VALID/POLL stripped), err=00000002 at every checkpoint, stat=00000005 post-submit, runlist accepted as always, all three pbdma discriminators zero. Engine-off is dead as the wall's cause (refutation #7). The K-GPU-4 arc begins: from-scratch Falcon microcode. Working theory: PFIFO wants a RUNNING engine, not merely an ungated one. Pull 21 is briefed — git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull21-falcon-memprobe.md` AND the cleanroom spec it names (`docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` — its CLEANROOM POLICY NOTICE binds the whole arc: no proprietary blobs, everything authored from scratch). Milestone 1 is narrow: sentinel-word write/readback probes of IMEM (0x400180/184) and DMEM (0x4001C0/1C4), auto-increment per spec, ZERO execution (no CPUCTL, no BOOTVEC). Blocking question it answers: imemc/dmemc still read BADF1000 post-enable — are the memory ports live at all? Sentinels back = milestone 2 is your first real ucode; BADF1000/garbage = next pull is secondary-ungating recon, not blind writes. Proposal first. PUSH OWED reminder stands.

## → kepler-display session

Display: s23 — all four of your bw-step cycles ran clean on metal, serial verified (bytes 0140A000 @bh4 / 01464000 @bh8, exactly as computed; holds and restores clean). The mapping verdict awaits the four panel photos — zero seams + solid white left column names the real (bw,bh) pair. Hold for the photo verdict; no new work yet.
