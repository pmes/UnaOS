# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25, after s24boot1: falcon ports still gated → pull 22 reset pulse briefed; display verdict awaits photos)

## → kepler-fence session

Fence: s24 verdict — FALCON MEMORY PORTS STILL GATED. With PMC bit 12 set, every access to IMEMC/IMEMD/DMEMC/DMEMD returned BADF1000 — control readbacks included, no sentinel came back. (Note: the readback path in your pull-21 code was corrected at land-review to use bit 25 AINCR for reads — bit 24 only auto-increments writes; the spec doc now says so. The verdict stands regardless: even the control-register readbacks are dead.) The witness baseline printed unchanged (err=2). Conclusion: enable-alone doesn't open the Falcon sub-block; there is a second gate. Pull 22 is briefed — git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull22-pgraph-reset-pulse.md`: replace the plain enable with a RESET PULSE of the same PMC bit (clear bit 12, ~100 ms settle, set bit 12, settle; markers pgraph-pulse pre/off rb/on rb), then re-run your pull-18 recon AND pull-21 port probe unchanged. Reset-then-enable is standard init discipline for this hardware class; the diff against s24's all-BADF1000 baseline is the deliverable. Writes stay confined to PMC_ENABLE + the four ports. Proposal first. PUSH OWED reminder stands.

## → kepler-display session

Display: s24 — all four bwpg-step cycles ran clean on metal, serial verified (bytes 01560000 @pg192 / 01C80000 @pg256, both bw values, exactly as computed; restores clean). The mapping verdict awaits the four panel photos — zero seams + solid white left column completes the mapping. Hold for the photo verdict; no new work yet.
