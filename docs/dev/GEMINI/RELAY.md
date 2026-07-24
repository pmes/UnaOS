# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-23, after s14: surface pointer candidate found; pull-5 briefed)

# (updated 2026-07-24, after s18: pattern flew, panel photographed; pulls 9 + 16 briefed)

## → kepler-display session

Display: s18 verdict — your pattern flew and the photo decodes: only EARLY surface rows (red + part of green) reach a fixed bottom band; NO blue/white; and the 64-px black left bar showed as staggered drifting dashes, not a column → working hypothesis: hardware pitch ≠ w×4. Also your midhold caught 0x61634C mutating under the latch (0x07380BAF → 0x00050008) — logged. Pull 9 is briefed — git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-display-pull9-ruler.md`: identical writes, ruler fill (64-row color cycle + wide white left marker) so one photo yields pitch and row mapping as numbers. Proposal first. PUSH OWED reminder stands.

## → kepler-fence session

Fence: s18 verdict — your mirror-hdr window is VOLATILE (fill value 0xFF114D95 grew 62→158 non-zero rows between passes; entropy words at 0x16C–0x17C). Logged hypothesis: it's an aperture onto live memory, not a register file. Pull 16 is briefed — git pull, read `docs/dev/GEMINI/video/Kepler/BRIEF-kepler-fence-pull16-beacon.md`: plant BAR1 beacons in our own USERD/pushbuffer/runlist structures and see if the window mirrors them. Zero MMIO writes. Proposal first. PUSH OWED reminder stands.
