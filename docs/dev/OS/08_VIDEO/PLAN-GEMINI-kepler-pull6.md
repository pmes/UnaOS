# PLAN — Kepler pull 6 (for the Gemini session): finish both derivations + cleanroom debt

**Context:** sitting #4 (fox-metal-r23s1f #4, 2026-07-22, UnaOS-gemini@44cf4387) moved
BOTH walls from "wrong register block" to "right block, wrong slice." The bases are
now believed correct; this pull settles the strides/field-offsets/enable-sequence and
clears one cleanroom item. Metal facts of record:
[`KEPLER-METAL-LOG.md`](KEPLER-METAL-LOG.md) (sitting #4 section) — trust it over QEMU.

Still a root-cause/derive brief. **Proposal first** (`PROPOSALS/PROPOSAL-kepler-pull6.md`,
STATUS: PROPOSED) and — as with pull 5 — the proposal must contain the derivations
themselves (offsets/strides + the exact envytools XML each comes from), reviewed against
the facts before code. Cleanroom rules stand: rnndb/envytools facts ONLY; nouveau code
and function names are off-limits (this pull also REMOVES an existing violation, see §3).

## Derivation 1 — head ARMED-state stride + field offsets (wall 1)
Facts: base `0x616100` reads LIVE state, but all 4 heads are byte-identical
(`addr=00000001 size=078004FE storage=0A0006A8`) → stride collapsing, and `addr` is a
flag not an address → field offsets wrong. `size=0x078004FE` high half = 1920 (0x780) =
real geometry, confirming the block is display state sliced wrong.
1. Re-derive the GF119+ per-head ARMED-state STRIDE (the `head*0x800` guess makes 4
   reads land on one head — the real stride differs; cite it).
2. Re-derive the field layout within the ARMED block: which sub-offset holds the
   SCANOUT ADDRESS (VRAM offset, likely >>8), which holds width/height, which holds
   pitch/storage. Map the observed `078004FE`/`0A0006A8` to named fields to prove the
   layout (a correct layout must explain those exact values).
3. Match uses the scanout-address field only, against `expected_addr`/`expected_phys`
   (both >>8). Keep the bad-read guard on every read. Flip logic unchanged (bounds,
   latch, visual no-op, STOP discipline).

## Derivation 2 — start the PBDMA that serves our runlist (wall 2)
Facts: PSUBFIFO base `0x40000` reads clean (0x40108 = 0, not poison → base OK, unit not
started); `pbdma-count=3` (expected 1 on GK107 — decode suspect); `gp_get=0` (never
fetches); `playlist_rd=0x2013` stable (real breadcrumb); channel ENABLED.
4. Re-derive the PBDMA COUNT register/decoding for GK107 — is 3 real (3 PBDMAs) or a
   misread? Cite the count register; witness the decoded value.
5. Derive which PBDMA unit is bound to OUR channel's runlist, and the START/ENABLE/
   clock sequence a PBDMA needs to begin fetching (beyond PMC bit 8 + SUBFIFO_ENABLE):
   PBDMA-local enable/start register(s), and the runlist SUBMIT/COMMIT that makes the
   channel resident. `playlist_rd=0x2013` — identify that register from the XML and use
   it to confirm the runlist was actually committed (not just written).
6. Keep the full witness chain + bad-read guard on every PBDMA read. After the start
   sequence, re-read pbdma_stat and gp_get into a witness so a still-`gp_get=0` result
   localizes to residency vs fetch. Success unchanged: `:: kepler: fence DEADBEEF ::`.

## §3 — cleanroom debt (must clear this pull)
7. `kepler.rs:~465`, EVO core-channel control at `0x490`, carries the comment
   "Offsets derived from nouveau/gf119.c as envytools is sparse here" (entered pull 3,
   commit 1c9e2570). This is a GPLv2-source derivation — forbidden.
   - If the offset/bits trace to rnndb/envytools facts, re-cite from there.
   - If they genuinely aren't documented, replace with an HONEST comment
     ("empirically probed on GK107, unverified against public docs") and put the reads
     behind the bad-read guard so a wrong value self-identifies.
   - Either way the nouveau-source citation must be gone.

## Out of scope
Falcon/PGRAPH ucode; any un-gated behavior change; aarch64.

**Oracle:** QEMU = quiet baseline, clean refusals, gates green (with + without every
knob). Metal sitting #5: k2 either matches a real scanout address (then flips under the
no-op rule) or the named-field head-raws show the last slice error; k3 either fences or
the post-start witnesses pin residency-vs-fetch. Update KEPLER-METAL-LOG.md as part of
DONE.
