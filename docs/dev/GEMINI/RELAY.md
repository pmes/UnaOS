# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: s31+s32 folded — pull 28 CLOSED, PRI-poison law found and double-confirmed; pull 29 invited)

## → kepler-fence session

Fence: pull 28 is closed, and your probe found something sharper than what it went looking for. Two sittings of results:

s31: the very first recon read (WRCMD_CMD, 0x409504) returned BADF1000 — and then EVERY subsequent read of the 0x409xxx unit returned BADF1000 for the rest of the boot, including registers that read real values seconds earlier (cpuctl was 00000010 right before the block) and everything s30 had proven (mailboxes, IMEM readback). Your verify-gates did exactly their job: ucode A and HB both ABORTED cleanly on readback mismatch rather than starting blind. PFIFO was untouched.

s32 (coordinator relocated the block after `hb final` and bracketed it with cpuctl control reads): s30 behavior FULLY RESTORED — ucode A executed again (mailbox0=F00DFACE), heartbeat mb1 0x4 → 0x57C9 → 0x5B2E → 0x343B4 across the witness, signature unchanged. Then the control frame: `recon-pre cpuctl=00000000` (real — your HB was still running) … all seven recon offsets BADF1000 … `recon-post cpuctl=BADF1000`, the SAME register, microseconds apart.

THE NEW SILICON LAW: on this GK107, the first access to a bad 0x409xxx offset faults immediately and poisons all subsequent reads of the FECS unit for the rest of the boot. Consequences: (1) the only clean per-offset datum is 0x409504 = absent-or-faulting; the other six gf100-era ctxctl offsets are CONFOUNDED, not disproven; (2) the s24/s25 all-BADF1000 sweeps are retroactively suspect for the same reason; (3) the pull-28 amendment stands — no hypothesis writes against any offset not proven readable.

PULL 29 INVITATION — propose a per-offset truth strategy, cleanroom as always. Candidate directions (pick, combine, or better them): (a) rotate which offset is read FIRST across boots — one clean datum per boot, slow but certain; (b) cleanroom study of the PRI fault/error mechanism on Kepler — is there a host-visible error-clear (PPRI/ringmaster family) that un-wedges the unit so several offsets can be probed per boot? cite sections; (c) re-derive where the GK107 FECS host-interface actually lives — nouveau drives 0x409504 on gk104, so a fault HERE is surprising; is there a Kepler-specific layout difference, an enable prerequisite (clock/priv gating), or an access-width requirement? A proposal that explains WHY 0x409504 faults on a part whose siblings use it may be worth more than the six remaining values.

## → kepler-display session

Display: no change since the last relay — s31/s32 were fence sittings; the panel showed the calibration draw unchanged, as expected. Your lane remains idle pending the coordinator's console wiring. Nothing owed from you.
