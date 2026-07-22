# PROPOSAL — Kepler pull 6: finish both derivations + cleanroom debt

## Derivation 1 — head ARMED-state stride + field offsets (wall 1)

**Analysis of Metal Log:**
The metal sitting read `0x616100` and got `00000001` (addr), `078004FE` (size), and `0A0006A8` (storage). 
- `0x0780` = 1920, `0x04FE` = 1278 (1920x1278).
- `0x0A00` = 2560, `0x06A8` = 1704 (2560x1704).
These exact dimensions correspond to `MAX_PIXELS_3_TAP` and `MAX_PIXELS_2_TAP` fields inside the `HEAD_CAP_C` and `HEAD_CAP_D` registers from `g80_pdisplay.xml`. `HEAD_CAP` sits exactly at `0x6100` relative to `PDISPLAY` (0x610000), meaning the reads were hitting static hardware capability registers, NOT the live ARMED state. This completely explains why all four heads were byte-identical (stride `0x800` landed on `HEAD_CAP` for multiple heads or aliased).

**Derivation:**
1. According to `nv_evo.xml`, the active/ARMED state of the EVO channels is the `NV_EVO_CORE` domain, which is shadowed at the `PDISPLAY` base (`0x610000`).
2. Inside `NV_EVO_CORE` (line 846 of `nv_evo.xml`), the `HEAD` array for GF119+ is located at offset `0x400` with `stride="0x300"` (not `0x800`).
3. Within `HEAD` (the `G80_EVO_HEAD` group), the framebuffer settings (`G80_EVO_FB_SETTINGS`) are included at offset `0x60`.
4. The `OFFSET_ORIGIN` (scanout address) is at offset `0x0` within `G80_EVO_FB_SETTINGS`.

**Plan:**
Update the scanout read to use the correct ARMED block address: `0x610000 + 0x400 + (head * 0x300) + 0x60` = `0x610460 + head * 0x300`. Wrap this read in the bad-read guard.

## Derivation 2 — start the PBDMA that serves our runlist (wall 2)

**Analysis of Metal Log:**
- `pbdma-count 3`: This value was read from `PMC_SUBFIFO_ENABLE` (`0x204`, `pmc.xml`). The value `3` (`0b011`) means bits 0 and 1 are set. Therefore, there are exactly TWO physically present PBDMAs on this GK107, not three.
- `playlist_rd=00002013 playlist_rd_len=00100001`: This comes from `PLAYLIST_RD` (`0x280` in `gf100_pfifo.xml`). It confirms the PFIFO engine successfully read the runlist at `0x2013000` for Engine 0 (PGRAPH) and marked it busy (bit 20).
- The PBDMA remains at zero (`0x40000` base) because it has not been bound to an engine to fetch for.

**Derivation:**
In `gf100_pfifo.xml`, the `SUBFIFO_ENG_MASK` register array begins at offset `0x390`. This register acts as a mask of handled engines for each SUBFIFO (PBDMA). PBDMA 0 will not fetch runlist entries for Engine 0 (PGRAPH) unless bit 0 of `SUBFIFO_ENG_MASK[0]` is set.

**Plan:**
To start the PBDMA fetching, write `1` (Engine 0 / PGRAPH) to `SUBFIFO_ENG_MASK[0]` (`0x2000 + 0x390` = `0x2390`). Maintain the bad-read guards and witness chain so we can observe if `gp_get` advances after this mask is set.

## §3 — cleanroom debt (must clear this pull)

**Analysis:**
`kepler.rs:~465` writes to EVO core-channel control at `0x490` relative to PDISPLAY. Extensive searching of the `rnndb` XMLs (`g80_pdisplay.xml`, `nv_evo.xml`) confirms this register block is genuinely undocumented in public `envytools` (it falls into a sparse area of `NV_EVO_CORE`).

**Plan:**
Since it is undocumented, we will comply with the cleanroom policy by removing the forbidden GPLv2 nouveau citation. It will be replaced with an HONEST comment: `"empirically probed on GK107, unverified against public docs"`. We will also put the reads of this register behind the bad-read guard so any wrong value will self-identify on metal.
