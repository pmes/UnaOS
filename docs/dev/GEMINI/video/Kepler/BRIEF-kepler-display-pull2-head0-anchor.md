STATUS: BRIEF — awaiting proposal (`PROPOSAL-kepler-display-pull2.md`, this directory)

# BRIEF — Kepler display pull 2: head 0 scans — find its surface registers from the proven anchor

Coordinator-authored (2026-07-22, post sitting #11 boot 1).

## The anchor (KEPLER-METAL-LOG.md #11)

- Head 0 is live: `HEAD_STAT` raster counters tick (`vert=0x0493048A
  horz=0x068C`), heads 1-3 dead, class 917D, GOP fb at VRAM +0x20000.
- Both EVO "MMIO mirror of method state" layouts refuted (all zeros).
- Sittings #5-#10's "engine idle" is retracted: wrong-address decode
  throughout. The 0x616000 HEAD_STAT block is the ONLY display block we have
  ever decoded correctly on this part — it is the anchor.

## What the proposal must derive

1. **The 917D-class armed/active state readback path** — where the hardware
   exposes the CURRENT scanout surface address and head config for
   GF119+/GK104 display, derived from the disp XMLs around the PROVEN
   HEAD_STAT offsets (same 0x800 head stride is likely; map the whole
   0x616000-era head block: what lives at each offset, cited row by row).
   Explicitly: does armed method state live in a PDISPLAY debug/armed window
   (cite it), or is the surface address only observable via the core-channel
   state in VRAM (then derive where the core channel's state table lives)?
2. **Raster-counter decode confirmation** — cite the register behind our
   stat rows (vert/horz counters) to lock the anchor's identity; the
   observed values must decode to the panel's mode as a cross-check.
3. **Trace extension (read-only)** — extend the kdisp trace to dump the full
   cited head-0 block (bounded row count, sentinels, `:: kdisp:` prefix) so
   one boot returns the entire head-0 state for offline decode. No writes.
4. **Success definition** — a named register whose value equals the GOP
   surface (VRAM +0x20000 / addr 0x200 in whatever shift the field uses).
   Finding it = scanout register PROVEN = the write-a-pixel takeover pull
   becomes possible next.

Standing rules unchanged (cleanroom, bounded, full-knob land-review with
strings-proof both artifacts, arch gate). Metal owed: sitting #12.
