# Kepler bring-up — metal facts of record (rMBP GT 650M / GK107)

Hard-won silicon facts from the fox-metal sitting series. Trust these over any
QEMU behavior. Newest sitting first.

## Sitting #6 (igpu pull 1 + kepler pull 7, UnaOS-gemini@8105f73c, 2026-07-22, fox-metal-r23s1h) — IN PROGRESS

**Boot 1 (8f7aaa6e media) — WASTED, defect ours:** the staged kernel carried no
probe. Builder lacked the `UNAOS_IVB` env→feature mapping AND igpu.rs had never
compiled (3 errors) — the land-review "gates green" never armed the knob.
Fixed in d6efd093; false PASS corrected in 8105f73c. **Law adopted (both
sides): a knob-gated PASS requires the gate WITH knobs armed + strings-proof
of the probe in the builder-path kernel.elf.** (The builder's own env→feature
map can silently drop features — check it, not just arroyo's.)

**Boot 1b (8105f73c, strings-verified media) — iGPU probe CLEAN, and the
panel theory flips:** `[Intel iGPU]` through `:: igpu: probe-complete ::`.
- Pipes A/B/C `CONF=0` (ALL disabled). Planes A/B/C `CNTR=0 SURF=0 STRIDE=0
  LINOFF=0 TILEOFF=0` (all disabled, nothing mapped). `DP_A=0x1C` (port not
  enabled). FOX CROSS-CHECK line correctly did not fire.
- **Reading:** GOP left NO live iGPU scanout. Combined with sitting #5
  (Kepler evo/crtc=0 on all 4 heads): **NEITHER GPU has an enabled scanout at
  probe time** while the panel is black. The "gmux gave the panel to the iGPU"
  theory loses its iGPU half as-probed. Either firmware tears scanout down at
  ExitBootServices, or the 0x90020000 fb writes go to an aperture nobody
  scans. Milestone-2 framing (write-in-place vs GGTT remap) is MOOT as posed —
  there may be nothing to write into; the next question is "who can light a
  pipe from scratch" + what gmux muxes when both engines are off. Per
  null-hypothesis law, our bootchain's at/after-handoff behavior stays the
  prime suspect for the teardown. **Strategy call is Peter's, not an
  auto-continue.**
- Sitting-brief hygiene (recorded): boot-2 knob list must be
  `UNAOS_USBDEBUG+UNAOS_IVB+UNAOS_KEPLER+UNAOS_KEPLER_TAKEOVER+UNAOS_KEPLER_FIFO`
  — the original brief omitted the FIFO knob; Fox's strings check caught the
  under-build before it flew.

**Boot 2b (full pass) — SITTING COMPLETE, wall 2 relocated upstream:**
- igpu probe re-ran identically (all-dark confirmed twice). Nit: the probe
  prints twice — PCI walk revisits the device; harmless, dedupe whenever the
  file is next touched.
- Kepler: pbdma-count 3; **all three PBDMAs `ch=00000000 ACTIVE=0,
  ib_put=ib_get=0` — no PBDMA ever binds our channel.** Eng-masks are set
  (pbdma0=0x01, pbdma1=0x6E, pbdma2=0x10). Clocks proven fine:
  `PMC_ENABLE=0xE011216D` (PFIFO=1), `SUBFIFO_ENABLE=0x7` — the
  clock/enable theory is DEAD. Meanwhile `playlist_rd=0x2013 len=0x100001`
  (scheduler reads the runlist), `ch_stat=0x11000001` (ENABLED), `gp_get=0`,
  fence-timeout as before.
- **Synthesis:** the fetch never happens because the channel is never
  SCHEDULED onto a PBDMA. The wall moved upstream of PBDMA to the
  runlist-entry/channel-bind step. Pull-8 candidate list (rnndb facts only):
  (a) GK107 runlist entry format/ID vs our channel id; (b) RAMFC/instance
  fields the scheduler validates before binding (inst-raw
  08=0x02002000 0C=0 48=0x02001000 4C=0x01FF0000 is on serial to decode);
  (c) runlist submit/commit ordering vs channel-enable; (d) whether the
  channel must ride the ENGINE's runlist rather than runlist 0.

Capture: `~/unaos-bench/capture/rmbp-r23s6/` + MANIFEST rows.

## Sitting #5 (pull 6 v2, UnaOS-gemini@e49efbeb, 2026-07-22, fox-metal-r23s1g) — STRATEGIC REDIRECT

**Wall 1 — DOUBLE REFUTATION + gmux redirect.** Both candidates dead on all 4 heads:
`head-raw head=N evo=00000000 crtc=00000000` → `bad-read head N no valid candidates` ×4.
The whole PDISPLAY engine reads idle. NEW sitting fact: the internal panel went BLACK
almost instantly on EVERY boot (kbase and k1 too — usbdebug builds that should hold the
boot log on-panel; serial fully healthy throughout).
- **Reading (Fox):** the 2012 rMBP has a **gmux** — the internal panel is on the Intel
  HD 4000 **iGPU**, and the Kepler display engine is legitimately dark. The GOP fb at
  `0x90020000` (confirmed live in boot-2 fb-wc retype) belongs to the **iGPU**, not
  Kepler. So wall 1 was misframed: not "which Kepler register holds scanout" but
  "which GPU owns the panel" — and it is the iGPU.
- **Consequence:** Kepler *display takeover* (K-GPU-2) may be a dead end on this box.
  Kepler compute / PFIFO (K-GPU-3) is unaffected. Next display derivation, IF pursued:
  gmux state readback (ACPI GMUX / port 0x7xx IO) + Intel iGPU scanout regs — NOT more
  Kepler PDISPLAY decode. **This is a strategy decision for Peter, not an auto-continue.**
- Open question (bench): the black-panel-on-every-boot incl. kbase (no Kepler active) —
  environmental vs a broader fb-handoff regression. Per null-hypothesis-our-code, do not
  assume hardware; flag for a clean cross-check.

**Wall 2 — progress, still no fence.** `pbdma-count 3` (oddity persists) ·
`pbdma-eng-mask set` (took) · `inst-raw 08=02002000 0C=00000000 48=02001000 4C=01FF0000` ·
`fifo-layout userd=2002000 fence=2014000 gp=1/0` · `bad-read pbdma 40108 00000000` ·
`fifo-front pbdma_stat=00000000 playlist_rd=00002013 playlist_rd_len=00100001` ·
`ch_stat=11000001 (ENABLED UNK24_RO UNK28_RO)`.
- NEW vs #4: eng-mask took, channel ENABLED, playlist_rd nonzero (scheduler SEES the
  playlist) — yet `gp_get=0`: PBDMA still never fetches the GP entry. `pbdma_stat=0` +
  the `40108` bad-read are the fresh clues: we still cannot read PBDMA status at that
  offset → wrong PBDMA base, or the unit is unclocked/not started.
- NEXT: the PBDMA-count=3 decode and the `40108` bad-read together — derive the correct
  GK107 PBDMA register base + the unit start/clock so `40108` reads real status.

Capture: `~/unaos-bench/capture/rmbp-r23s1g/`. Staging note: a zsh env-split staging bug
first pass, caught by sha-compare + rebuilt (verify-strings law held).

## Sitting #4 (pull 5, UnaOS-gemini@44cf4387, 2026-07-22) — both walls: right block, wrong slice

**Wall 1 — head scanout (base 0x616100 ARMED block is LIVE, slicing wrong):**
All four heads read byte-identical:
`head-raw addr=00000001 size=078004FE storage=0A0006A8`
- Non-zero = the 0x616100 block is real display state (progress from #3's uniform zeros).
- All 4 heads identical → the `head*0x800` stride is collapsing (reading the same regs 4×) — stride wrong.
- `addr=0x00000001` is not address-shaped (flag/enable bit) → field offsets within the ARMED block are wrong.
- `size=0x078004FE`: 0x0780 = 1920 in the high half → this IS display geometry, just sliced wrong.
- NEXT: verify per-head stride AND field offsets in the GF119+ ARMED block; scanout addr likely at a different sub-offset; 0x…4FE/0x6A8 look like packed geometry/config, not a scanout address.

**Wall 2 — PBDMA (base 0x40000 reads clean-zero, unit never started):**
`pbdma-count 3` · `inst-raw 08=02002000 0C=00000000 48=02001000 4C=01FF0000` ·
`fifo-layout userd=2002000 fence=2014000 gp=1/0` · `bad-read pbdma 40108 00000000` ·
`fifo-front pbdma_stat=00000000 playlist_rd=00002013 playlist_rd_len=00100001` ·
`ch_stat=11000001 (ENABLED UNK24_RO UNK28_RO)`
- PSUBFIFO 0x40108 went poison(#4)→zero(#5): base likely right, but unit reads 0 = never started/clocked.
- `pbdma-count=3` vs expected 1 on GK107 → count register/decoding needs a second look; if 3 is real, per-PBDMA stride decides which unit serves our runlist.
- `gp_get=0` persists: PBDMA never fetches entry 0.
- `playlist_rd=0x2013` stable across #4 and #5 → consistent read, likely a real breadcrumb.
- NEXT: which PBDMA unit is bound to our channel's runlist + its start/enable/clock sequence.

## Open cleanroom debt
- `kepler.rs:~465` EVO core-channel offsets (0x490) carry a "derived from nouveau/gf119.c"
  comment (entered pull 3 / 1c9e2570). Must become an rnndb citation or an honest
  empirically-probed note before merge. Not a bench concern.

## Earlier sittings (summary)
- #3 (pull 4, d938dd00): head-raw uniform ZEROS (wrong block); pbdma_stat=0xBAD0011F poison
  at legacy 0x6c0 (wrong base). Both walls localized.
- #2 (pull 3, f9804ab6): BUG#1 (VRAM 2989MB) + BUG#2 (GOP no-gop) CONFIRMED FIXED on silicon
  (512MB; GOP 0x90020000 = BAR1+0x20000). Introduced EVO flip + PFIFO.
- #1 (pull 2, c84688f1): first silicon contact; found the two bugs #2 fixed; builder
  UNAOS_KEPLER→feature mapping catch.
