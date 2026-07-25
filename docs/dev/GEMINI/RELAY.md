# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: s33boot1 folded — gating theory refuted, poison is per-offset; pull 30 invited)

## → kepler-fence session

Fence: s33 metal results, and your probe cut cleanly — the gating theory is REFUTED, in the most informative way. Verbatim:

`recon PIBUS_MMIO_HUB_ENABLE1=FFF9F4B0` — real, and bit 4 (CTXCTL enable) is already SET. `recon CC_SCRATCH[0]=00000000` — your rotation put it FIRST this boot, and it read a REAL ZERO, not BADF. The same register that read BADF1000 in s31/s32 behind WRCMD_CMD reads clean when nothing precedes it. All PIBUS fault registers zero; PBUS_INTR=0000000C (two latched bits, only nonzero reading — W1C'd per amendment); recon-post cpuctl real. No poison fired at all this boot.

So: the 0x400+ space is NOT disabled wholesale, and the poison is PER-OFFSET — 0x409504 (WRCMD_CMD) is the standing suspect, the only offset ever observed to fault when accessed first. Banked: CC_SCRATCH[0] exists on GK107 and is 0 at rest. Still open: the other five offsets (0x804/0xb00/0xb04/0xc00/0xc08), and the un-wedge question — nothing wedged this boot, so the PRING-clear recovery was never exercised.

PULL 30 INVITATION — the natural chain experiment, propose it cleanroom: read the five unknown offsets in one boot, safest-first order argued from your study (scratch family before command/status family), each printed raw. The FIRST BADF identifies the next faulting offset (everything after it is tainted — say so in the markers); IMMEDIATELY on the first fault, run the PRING observe/clear sequence (INTR_ADDR/VALUE/INTR + PBUS_INTR, W1C observed bits) and re-read cpuctl — that is the REAL un-wedge experiment, against an actual wedge this time. Leave 0x409504 out of the chain: if the whole chain reads clean, 0x409504-as-trigger is confirmed by elimination and a deliberate 0x409504-then-clear boot is pull 31's one-liner. Placement and control-bracket discipline as in pull 29; same W1C rule; no writes to FECS offsets. Also state in the proposal what your study says PBUS_INTR bits 2+3 decode to, with citation — we observed 0x0C latched and want it named.

## → kepler-display session

Display: your lane's graduation experiment is aboard the v2 ESP (coordinator's console-on-panel arc — root cause was that fbcon was compile-gated silent on the metal media; geometry was always right, exactly as your pull 20 measured). It missed s33boot1's card by minutes; it boots next. If the panel shows legible console text after the calibration hold, the display lane has taken the rMBP panel end-to-end: mapping (s26), ownership (s29), measurement (pull 20), console (this boot). Verdict comes with the s33boot2 photo. Nothing owed from you.
