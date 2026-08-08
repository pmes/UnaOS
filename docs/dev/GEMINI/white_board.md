# WHITE BOARD — 2026-08-08 (GR22)

## Q1 — Should STAT.ELF keep auto-launching visible at boot?

You flagged it twice ("stat.elf still lingering at boot"). Background to answer with:
it is GR21's kernel-apps eviction move #1 — the desktop app auto-launches from the
device-service pass once storage is up (now deferred behind the DMG-REFUSE witness), sits
as a permanent 128x128 window, and holds 1 proc row + 1 job row + 1 user slot (which the
headroom raise has made cheap — 10 rows now). `kill` reaps it cleanly, and it does not
relaunch within a boot. Options, cheapest first:
  a) keep as is (it is the proof the desktop launches ring-3 apps from media every boot);
  b) launch it HIDDEN (parked via set_hidden — one line; it still proves the path, no
     window in your face; TAB brings it up);
  c) knob it (UNAOS_DESKAPP=0 skips the launch on boots where you don't want it);
  d) defer to a dock/launcher story (the PULSE-2 panel arc is queued anyway).
Answer with a letter; (b) is the seat's recommendation — proof kept, glass clean.

## Q2 — Flight 1b: fly it next?

Round 11 merged; the gate now accepts the Kepler-owned EXT state, the flight writes ONLY
the DDC register, and the unwind replays the exact pre-image. It needs its OWN boot
(gmux_igd switches persistent mux state — never regression media). The staged AO boot does
NOT carry it. Say the word and the gmux boot stages as AP with the igpu lane's capture
promised back to them for round 12.
