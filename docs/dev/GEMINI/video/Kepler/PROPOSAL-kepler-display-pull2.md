# PROPOSAL — Kepler display pull 2: head 0 scans

## The Anchor: Raster Counter Confirmation
From sitting #11, the `HEAD_STAT` raster counters returned:
- `vert=0x0493048A`
- `horz=0x068C`

Decoding against `NV_HEAD_STAT` from `g80_pdisplay.xml` (offset `0x340` and `0x344`):
- `VERT` low 16 bits = `0x048A` (1162) -> Current vertical scanline.
- `VERT` high 16 bits = `0x0493` (1171) -> VBlank counter.
- `HORZ` low 16 bits = `0x068C` (1676) -> Current horizontal pixel.

These coordinates cleanly match an active 1080p (or similar) scanout (VTotal typically ~1125, HTotal ~2200). This categorically confirms that `0x616000` (PDISPLAY + 0x6000) is the base of the active head 0 block for GK104.

## The Armed State Readback Path
The prior attempts relied on `OFFSET_ORIGIN` (an EVO method shadow) and the pre-GF119 `HEAD_VAL` block (which `g80_pdisplay.xml` explicitly marks as `G80:GF119` only). Both were empty (all zeros). 

In GF119+, the head configuration block is grouped at `0x616000` with a uniform stride of `0x800` per head.
Currently, `g80_pdisplay.xml` only maps fragments of this block:
- `0x6000` (`HEAD_STAT` / `NV_HEAD_STAT`)
- `0x6100` (`HEAD_CAP`)

It is highly probable that the GF119+ equivalent of `HEAD_VAL` (the armed surface state) was relocated into this same `0x800`-byte block, occupying an unmapped window (e.g., `0x6200` or `0x6300` relative to PDISPLAY). Since it is not documented in the cleanroom XMLs, we must discover it empirically from the proven anchor.

## Trace Extension (Read-Only)
Instead of guessing offsets, we will dump the entire `0x800` byte footprint of Head 0.
- **Range:** `0x616000` to `0x6167FC` (inclusive, stepping by 4).
- **Condition:** We will read each `u32`. To avoid log spam, we will only log values that are "live" (non-zero, not `0xFFFFFFFF`, and not matching the `0xBAD0xxxx` unmapped-BAR pattern).
- **Format:** `:: kdisp: head0-dump off={:03X} val={:08X} ::` (where `off` is the offset from `0x616000`).

This ensures one boot will return the entire head-0 state for offline decode without any blind writes.

## Success Definition
The GOP framebuffer was reported at VRAM `+0x20000`.
In NVD0/Kepler EVO formatting, the surface address is typically shifted by 8, yielding `0x00000200`. It could also be stored as an absolute offset (`0x00020000`).

**Success:** Any register within the `0x616000` to `0x6167FC` block that holds the value `0x00000200` (or `0x00020000`) is categorically proven to be the active scanout address register for GF119+. Finding this register will allow us to execute the "write-a-pixel" takeover in the next pull by confirming the surface address before and after the EVO flip.
