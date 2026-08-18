# PLAN — Kepler pull 5 (for the Gemini session): two register-base derivations

**Context:** sitting #3 (fox-metal-r23s1f #3, 2026-07-22, UnaOS-gemini@d938dd00) turned
both walls into single-question derivation problems. This pull is a root-cause/derive
brief — the register facts are the work; the code changes are small.

**Metal facts of record (verbatim from capture, marks P4K3/P4K4):**
- Wall 1: `head-raw` uniform ZEROS — all 4 heads, all 4 fields. The panel IS scanned
  out of NVIDIA VRAM (GOP base 0x90020000 = BAR1+0x20000; the dGPU owns the display).
  → The head-state read targets the wrong register block on GK107. The
  address-representation theory is DEAD — do not revisit it.
- Wall 2: `pbdma_stat=BAD0011F` — poison-family read (cf. PGRAPH 0xBADF1200): the
  PBDMA is unclocked/not-enabled at that offset, or the base is wrong for GK107.
  Channel ENABLED, gp=1/0, runlist scheduled (`playlist_rd=2013 len=100001`).
  Instance words on record: `08=02002000 0C=00000000 48=02001000 4C=01FF0000`.

**Process:** proposal first (`PROPOSALS/PROPOSAL-kepler-pull5.md`, STATUS: PROPOSED)
— and for THIS pull the proposal must contain the derivations themselves (register
offsets + the envytools XML/file each comes from), not just intentions. Review will
check the offsets against the facts before any code lands. Cleanroom rules stand:
rnndb/envytools facts only; nouveau code and function names are off-limits.

## Derivation 1 — GK107 head scanout state (wall 1)
1. Re-derive where GK107 exposes ACTIVE head scanout state. Candidates to settle
   from the disp XMLs (cite precisely):
   a. The GF119+ display has a separate "armed vs assembly" state model — reading
      the assembly (pending) side of a head that firmware configured would read
      zeros. Find the ARMED-state mirror offsets.
   b. The core-channel state DMA: firmware-programmed EVO state is reflected in a
      state buffer readable via the core channel's NV_DMA object rather than bare
      PDISPLAY MMIO. If that is the documented mechanism, read it there.
   c. If (a)/(b) both dead-end in the docs, fall back: derive the scanout base from
      the ISO hub/window registers (the unit that actually fetches pixels must hold
      a live address somewhere readable).
2. Guard generalization: any candidate read returning 0x00000000 across a whole
   block OR a 0xBADxxxxx-family value must witness
   `:: kepler: bad-read unit=<name> off=<off> val=<raw> ::` and disqualify that
   base — wrong-base reads must self-identify in one boot, never masquerade as data.
3. Code change: point the existing head-raw dump + match at the derived offsets.
   Flip logic unchanged (bounds, latch, visual no-op, STOP discipline).

## Derivation 2 — GK107 PBDMA base + unit enable (wall 2)
4. Derive from the fifo XMLs (cite precisely):
   a. The GK104-family PBDMA register base and per-PBDMA stride, and the GK107
      PBDMA unit COUNT (read the documented PBDMA-count register and witness it:
      `:: kepler: pbdma-count <n> ::`).
   b. Every enable the PBDMA needs beyond PMC bit 8 and SUBFIFO_ENABLE — PFIFO
      subunit enables, clock gating, and any INTR/init sequencing the docs name.
5. Apply the same bad-read guard to every PBDMA register read.
6. Re-check the instance-block encoding (words 0x08/0x0C/0x48/0x4C on record)
   against the derived facts — if the encoding was wrong, fix it and note the
   before/after interpretation in REPORT.
7. Keep the full witness chain (inst-raw, fifo-layout, fifo-front, decoded ch_stat,
   bounded timeout). Success criterion unchanged: `:: kepler: fence DEADBEEF ::`.

## Out of scope
Falcon/PGRAPH ucode; any un-gated behavior change; aarch64.

**Oracle:** QEMU = quiet baseline, clean refusals, gates green. Metal sitting #4:
k2 either decodes real head state (then flips under the no-op rule) or the bad-read
guards name the dead bases; k3 either fences or `pbdma-count` + guarded reads pin
the enable that's missing. Either way: no blind boots remain after this pull.
