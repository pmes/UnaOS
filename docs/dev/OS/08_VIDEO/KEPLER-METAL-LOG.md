# Kepler bring-up — metal facts of record (rMBP GT 650M / GK107)

Hard-won silicon facts from the fox-metal sitting series. Trust these over any
QEMU behavior. Newest sitting first.

## Sitting #19 (display pull 9 + fence pull 16, UnaOS-gemini@ae5ce2b2, 2026-07-24, fox-metal-r23s1j)

**Display — ruler flew; ONE hypothesis now explains every panel fact:
the scanout window reads the surface BLOCK-LINEAR (GOB 64 B × 8 rows) while
we fill linear.** (Coordinator decode from Peter's photo + notes at
`capture/rmbp-s18/s19boot1-panel-observation.md`.)
- Full 8-color cycle visible in-order inside the same bottom ~1/8 band,
  whole cycle compressed — each 64-row block only a few panel rows tall →
  ~8× vertical compression = GOB height 8.
- Red stripes dashed/checkerboarded at short regular periods → 64-byte
  (16-px) chunks of our linear rows stacking vertically under the swizzle.
- White 256-px left column invisible → 1 KB of white per row shatters into
  scattered 64 B blocks. Per-row notch unresolvable, same reason.
- Retro-consistency: s17 solid green (swizzle-invariant) showed a clean
  band; s18 quarters kept coarse order. Latch ladder identical
  (asm-stuck=y, armed-followed=n, all boots).
- Bottom-band placement REMAINS a separate unknown (viewport/window offset).
- Pull 10 = pre-swizzled ruler (linear→GOB transform in the fill): clean
  stripes + solid white column on the panel would PROVE tiling + params.

**Fence — beacon verdict: NONE-SEEN; window is NOT a mirror of our channel
structures.** Beacons planted at userd 0x2002000 / pb 0x2003000 / runlist
0x2013000 (BAR1); pass1 clean of beacons (158 nonzero rows); **pass1→pass2:
ZERO words changed** — the window is stable within a boot; s18's
"volatility" was across boots/boot-phase, not continuous churn. Standing
read: engine-private memory aperture, contents boot-dependent. Pull 17 =
latch-correlation probe (dump the window BEFORE takeover_display and after,
same boot, read-only — does the display UPDATE perturb it?).

**s18 completion note (fox-metal-r23s1j):** three s18 boots total, ladders
identical; bench corrected its own count — mirror-hdr pass1 nonzero rows =
158 (matches the coordinator's fold; the 159 in the first relay counted the
done line).

## Sitting #18 (display pull 8 + fence pull 15, UnaOS-gemini@c4dbbbb6, 2026-07-24, fox-metal-r23s1j)

**Single all-knob boot, both lanes served. Serial side capture-verified;
panel photographed (Peter).**

**Panel facts (photo, pattern boot):**
1. Visible band sits at the BOTTOM ~fifth of the panel (same region as
   s17's green): RED above GREEN, red roughly twice the green's height. No
   blue, no white anywhere → only EARLY surface rows (red quarter + part of
   the green quarter) ever reach the panel.
2. **No continuous black left column** — the 64-px left bar appears instead
   as periodic dark DASHES drifting across the band. Deduction: the
   hardware scan pitch ≠ our assumed 11520 (w×4); the left-bar marker wraps
   to drifting x positions row by row. Pitch is the primary unknown.
3. Band interior shows staggered brick-patterned dark dashes through red
   and green (bench read: "tears"; coordinator read: the black left-bar
   fragments wrapping at drifting x — a spatial pitch artifact, not
   temporal tearing, since the pattern is stable in a still frame). The
   dash stagger is itself pitch data; pull 9's ruler resolves which read
   is right and yields the number.

Mapping verdict: the latch scans a SUB-RANGE of our surface (early rows)
into a fixed bottom band, at a pitch we have wrong. Pull 9 = ruler pattern
(row-coded color cycling + thin black row-markers + wide white left column)
to solve pitch and row-mapping arithmetically from the next photo.

**Display pull 8 (serial side):** geom w=2880 h=1800 pitch=11520; full latch
ladder identical to s17 (asm-stuck=y, armed never followed, raster ticking
t=1..8). **Mid-hold fact: 0x61634C read 0x00050008 while the pattern surface
was latched — s13/s16 read 0x07380BAF (raster totals) at that same offset
pre-latch.** The timing-cluster word CHANGES under an active latch (it took
the value shape head 1 shows at reset). 0x616340 stayed raster-consistent;
0x6101E0/0x61D1E0/0x61D014 all unmoved. Interpretation open until the panel
report lands.

**Fence pull 15 (method-mirror header 0x640000–0x6403FC, read-only):**
- Structure (pass 0): zeros 0x000–0x088; lone 0x08C=0x2CB23507; solid
  0xFF114D95 fill 0x090–0x168; five high-entropy words 0x16C–0x17C
  (F3EEF6EE/8FD5136D/EE76BF7D/3642C748/CD3A5D9D); 0x240=0x00000801;
  zeros elsewhere.
- **The region is VOLATILE: pass 1 has 158 non-zero rows vs pass 0's ~62**
  (the 0xFF114D95 fill GREW between passes; 302 fill-rows total across both).
  This does not read like a stable register file — hypothesis (labeled as
  such): the window is an aperture onto live memory (core-channel
  pushbuffer/USERD territory), not config MMIO. Fence pull 16 design should
  treat it as memory-backed and correlate against the display lane's latch
  activity.
- Coordinator row-count note: my capture count says pass1=158 non-zero
  (bench said 159); rows=256 both passes confirmed.

Head-scan preamble: all four heads evo=0 skip (expected, refuted mirrors);
evo-core 32-row dumps present both passes.

## Sitting #17 (display pull 7, UnaOS-gemini@11f06ded, 2026-07-23, fox-metal-r23s1i) — ⭐ MILESTONE

**FIRST DELIBERATE UNAOS PIXELS ON THE rMBP INTERNAL PANEL. The EVO
arm-and-latch mechanism WORKS.** (Coordinator capture-verified, all lines.)
- Ladder: pre asm=armed=shadow=0x200 → `asm-wrote=00016000 rb=00016000`
  (assembly slot 0x640460 is WRITABLE and holds) → selfcheck ×2: armed
  unchanged (no premature latch — assembly and armed states are properly
  distinct) → UPDATE write 0x640080=0 (rb 0) → **panel showed a GREEN BAR at
  the BOTTOM of the screen during the 5 s hold (Peter's eyes)** → restore:
  asm back to 0x200, armed/shadow 0x200, screen recovered.
- `verdict asm-stuck=y armed-followed=n`: the 0x6101E0 "armed" readout NEVER
  left 0x200 even while green was on the panel — so 0x6101E0 is NOT the live
  scanout tracker (it reports some other/base state, or latches at a
  boundary we didn't cross). Known-unknown, logged as such.
- Green as a bottom BAND (not full screen) — the 0x640460 offset evidently
  maps a sub-region of the raster. Facts in hand cannot yet say which
  mapping (stride/tiling/multi-window split); pull 8 discriminates with a
  patterned fill instead of solid green. HEAD_STAT vert ticked throughout
  (raster never stalled); vblank_count high-halves advanced ~13-14/s.
- Fence-lane consequence: the EVO method-mirror write + UPDATE path is
  PROVEN LIVE — the disp-era-USERD fallback now has a working mechanism to
  ride; fence pull 15 = read-only recon of the method-mirror header region
  (0x640000–0x6403FC, never yet dumped) to locate channel-control/USERD
  slots.

## Sitting #16 (display pull 6, UnaOS-gemini@939ba952, 2026-07-23, fox-metal-r23s1i)

**Single read-only boot — ASSEMBLY STATE FOUND. Coordinator decode:**
- evo-scan2: 18 hits, uncapped. Pair table: no first≠second anywhere
  (nothing latched during the window — expected; nothing was arming).
- **The find: 0x640460 = 0x00000200** — a third 0x200-holder, sitting inside
  a coherent record in the 0x640000 (DISP_USER) region: 0x640420 holds
  0x07380BAF (the SAME raster-totals value proven at 0x61634C/s13), with the
  w2880/h1800 cluster at 0x640468–0x6404C8. Decode: the 0x640000 region is
  the EVO core-channel METHOD MIRROR — core-channel method layout puts head 0
  at +0x400 with the surface OFFSET slot at +0x60 → 0x640460. The record
  shape matches method semantics exactly (offset + raster + geometry).
  **0x640460 is the assembly-side surface pointer; the UPDATE method slot is
  +0x80 → 0x640080 is the latch-trigger candidate.**
- 0x61D1E0 = 0x200 as well: armed-shadow at +0xD000 from the s15 readout
  (second armed-side mirror, read-only presumed). Same block also holds
  0x61D014 = 0x00020000 — the GOP vram offset UN-shifted (coordinator
  capture-verified; 18/18 hits confirmed, 19 pair lines, zero diverging).
- Repeating 0x90000000-shaped words at 0x61C/0x61D x128-stride and the
  full gap-window rows (256×2) are in the capture for later decode.
- Next: pull 7 = assembly-write + UPDATE-latch experiment (0x640460 then
  0x640080), fully restore-paired. Display-write class already approved (s15).

## Sitting #15 (display pull 5, UnaOS-gemini@5686f417/e9b1e89f, 2026-07-23, fox-metal-r23s1i)

**Boot 1 (5686f417) — no write occurred:** the LEGACY EVO-mirror head-match
gate (refuted decode, s11) sat upstream of the repoint code and aborted
(`takeover-abort no-match`). Land-review miss (control flow not verified to
reach the new code); fixed inline at e9b1e89f — gate defaults to head 0
(HEAD_STAT canon) with an honest marker; refuted bounds check neutralized.

**Boot 2 (e9b1e89f) — repoint hypothesis REFUTED, cleanly:**
- Full ladder ran twice (known double-invocation), both passes identical:
  surf2 filled (0x1600000, 0x13C6800 bytes green) →
  `repoint wrote=00016000 rb=00000200` — **the write does not take; readback
  is the original value immediately** → raster ticked through all 5 hold
  seconds (vert 0x11C3→0x1206) → restore rb=00000200 → `verdict rb-stuck=no`.
  Panel never changed (nothing was armed). Boot continued normally.
- **Verdict: 0x6101E0 is a READ-ONLY armed-state readout, not a writable
  pointer.** Consistent with EVO armed-vs-assembly semantics: the armed
  surface value is reported here, but arming goes through the core channel's
  assembly state + an UPDATE/latch step (s13's 0x494=1 companion and the
  descriptor table at 0x6104A0+ are the standing leads).
- Bench note: the "stray no-match line" reported for boot 2 was a cross-mark
  grep artifact (it was boot 1's abort in the same accumulating log); the
  boot-2 segment is clean.

## Sitting #14 (display pull 4, UnaOS-gemini@114fad64, 2026-07-23, fox-metal-r23s1i)

**Single read-only boot — EVO core-channel read-out. THE SURFACE REGISTER
CANDIDATE FELL OUT.**
- Bench "divergence" (missing pass2) was a miscount — the brief's two passes
  are pass0+pass1, both complete in the capture (33 lines each). No rerun
  needed.
- **Known-value scan: exactly ONE hit in the full 16 KB sweep —
  `0x6101E0 = 0x00000200`** = the GOP surface address in >>8 form
  (fb at VRAM +0x20000; 0x20000>>8 = 0x200 — the `expected_addr` shape the
  s12 differential hunted for). hits=1 capped=false: the predicate produced
  zero false positives; this is the armed scanout surface pointer candidate,
  and it lives in the PDISPLAY armed-state region (0x6101E0), NOT the
  per-head 0x616000 block — exactly why s12/s13 couldn't find it there.
- **Core window (0x610480–0x6104FC) fully stable across passes** (zero
  varying words). Structure: +0x10=0x0D0500A9 +0x14=1 (the s13 recon words),
  then a repeating 3-word record {0x40000088, 0x00000001, 0x80010000} at
  stride 0x10 from +0x20 — a channel descriptor table (per-EVO-channel
  ctrl/flag/base records), uniform and quiescent.
- Verdict: display pull 5 = repoint-the-surface experiment (write 0x6101E0
  to a second prepared surface, watch panel, restore). FIRST display-register
  write — Peter decision required. Fence's disp-era-USERD input: the core
  channel is configured and idle; the descriptor table above is the map for
  any USERD-linkage probe.

## Sitting #13 (display pull 3 + kepler pull 14, UnaOS-gemini@f4a7ef6e, 2026-07-23, fox-metal-r23s1i)

**Boot 1 — display pull 3 candidate decode DELIVERED. Coordinator decode:**
- Dense windows (0x300–0x35C, 0x3F0–0x40C, 0x5F0–0x61C), both heads, 3 passes,
  all rows captured (`~/unaos-bench/capture/rmbp-s13/`, mark s13boot1).
- **Telemetry (varies across passes, disqualified as config):** 0x314 is a
  frame counter (0xACE→0xACF, monotone; the value s12 saw mirrored at
  0x118/0x53C), 0x340/0x344 track raster position, 0x3F4 toggles, and — new —
  **both 0x604 (0x0078→0x007A high-half) and 0x614 (0x22500→0x22900) move**:
  the in-kernel `stable=yes` for 0x614 was a per-pass sampling artifact; the
  window rows refute it. Both former candidates are counters, not config.
- **Mode-timing block identified:** 0x34C=0x07380BAF decodes as
  vtotal=0x738 (1848) | htotal=0xBAF (2991) — exactly raster totals for the
  2880×1800 panel with blanking; 0x348=0x00310070 is sync/porch-shaped
  (49/112); head 1 holds near-reset 0x00050008/0x00060009. The head block's
  0x340-region is the live timing/raster cluster, matching HEAD_STAT.
- **Surviving stable head-0-only config:** 0x310=0x008959E6 (same value
  across s12 AND s13 boots — config, not a counter; magnitude ~9.0M fits no
  obvious address/pitch/size against fb=+0x20000, pitch 0x2D00, fbsize
  0x13C6800 — PLL/link-coefficient-shaped, unresolved), 0x30C=0x58008000
  (flag word vs head 1's 0x01220000), 0x520=0x00000600, 0x600=0x000F4101
  vs 0x00000100 (enable cluster), 0x610=0x08000014 vs 0x08000000.
- **Verdict: the scanout surface ADDRESS is not exposed anywhere in these
  head-block windows.** 0x408=0x21EC4000 is address-shaped but identical on
  the dead head — a shared default, not the surface. Conclusion for pull 4
  planning: on 917D the armed surface likely lives in EVO core-channel state
  (reachable via the core channel, not as a bare per-head MMIO word), or in a
  head-block region outside the three windows. Peter decision required either
  way (first write vs wider read).

**Boot 2 — kepler pull 14: CTRL_ADDR TARGET hypothesis REFUTED (12/12).**
- All three PBDMAs read `pre=00000000 hi=00000000`; every TARGET value 0..3
  on every PBDMA wrote and READ BACK exactly (`wrote=rb`, register writable,
  never ABSENT/RO) — and the s10 witness ladder never latched once:
  `WITNESS FAILED - bits stripped` on all 12 steps, err=2 throughout,
  fence-timeout each iteration, clean evidenced restores (`restored
  rb=00000000`) between steps. Amendment discipline held (one PBDMA at a
  time; no freeze since no PASS).
- **M2 disp-era recon (read-only) found live EVO core-channel state:**
  `disp-userd-recon pdisplay_0=917D0210 +40=0000000A evo_0x490=0D0500A9
  evo_0x494=00000001`. 0x610490 holds a rich value and 0x610494 reads 1 —
  the disp-era USERD/core-channel enablement path (the last pre-committed
  fallback) has a live anchor to probe.
- Fence-wall ledger after s13: refuted = 3 runlist encodings (s8),
  USERD_SNOOP (s10), USERD_HI bit31 (s11), PFIFO_FLUSH (s12), CTRL_ADDR
  TARGET ×12 (s13). Remaining in-family lead: disp-era USERD enablement
  (write phase — needs its own brief). Beyond that the lane pivots to
  PGRAPH/ucode (K-GPU-4) — Peter strategy call.

## Sitting #12 (display pull 2 + kepler pull 13, UnaOS-gemini@9d22d263, 2026-07-23, fox-metal-r23s1i)

Captures: `~/.claude/plans/unaos/review/rmbp-s12boot1-headdumps.md` (boot 1,
full rows) and `rmbp-s12boot2-capture.md` (boot 2, raw post-mark). Bench note:
port labels were crossed — the rMBP serial for boot 1 landed in the
`pi4-r23s1i/cu.usbserial-ABAFUJCO.log` capture; content-verified rMBP output.

**Boot 1 — head0/head1 differential dump DELIVERED (display pull 2 complete):**
- Two full trace passes; head 0 dump 49/46 live rows, head 1 dump 40/38 rows,
  neither capped (96/64 caps were sufficient). HEAD_STAT confirms head 0 alive
  again (vert/horz counters tick across passes), heads 1–3 stat zero.
- Offline diff (coordinator decode): 19 offsets differ. Head-0-ONLY rows split
  into (a) frame-varying values — 0x118/0x314/0x53C all hold the same value
  (0x0E35 pass 1 → 0x0D9E pass 2) and 0x340/0x344 track the HEAD_STAT raster —
  i.e. live-scan telemetry, and (b) stable config-shaped rows: 0x310=0x008959E6,
  0x520=0x00000600, 0x604=0x00780000, 0x614=0x00022500. Rich DIFF rows where
  head 1 holds near-reset defaults: 0x348/0x34C (0x00310070/0x07380BAF vs
  0x00060009/0x00050008 — timing-shaped), 0x600 (0x000F4101 vs 0x00000100),
  0x538 (0x80001200 vs 0x80000000), 0x30C (0x58008000 vs 0x01220000).
- **The hoped-for clean signature did NOT fall out**: no offset on either pass
  holds a 0x200/0x20000/0x90020000-shaped value (the GOP surface address in
  any obvious shift). The scanout surface pointer is not a bare address in the
  0x616000 head block, or it is encoded (0x310's stable 0x008959E6 is the one
  address-shaped head-0-only candidate). Write-a-pixel cannot claim a proven
  target register yet; a decode step stands between us and it.

**Boot 2 — pull-13 flush hypothesis REFUTED (correcting the bench summary):**
- The first bench read ("aborted before PFLUSH ever printed") was wrong — the
  grep missed the success-branch marker. The ladder RAN:
  `flush-executed 0x70000 pre=00000000 post=00000000 iters=1` (register
  present, not ABSENT/POISON, drained in one iteration) →
  `WITNESS FAILED - bits stripped` → err=2 unchanged post-restore →
  post-submit stat=0x5, playlist_rd advances (0x2013/len 0x00100003) →
  all three PBDMA discriminators CHID=0 ACTIVE=0 → fence timeout at
  0x2014000, `takeover-abort fence-timeout gp_get=0 ch_stat=11000001`.
- Verdict: **a PFIFO_FLUSH between instance writes and validate does not stop
  the VALID/POLL strip.** Engine-side stale-view-of-BAR1-writes, in its
  flushable form, is refuted. inst-raw confirms our bytes persist
  (0C=80000000) exactly as s11 found.
- New evidence for the fallback audit: full RAMFC post-submit dump (16 rows;
  +08=02002000 userd, +10=0000FACE, +30=FFFFF902, +48/+4C=02001000/00090000)
  and per-PBDMA eng_mask readbacks (0x01/0x6E/0x10) with ib_put=ib_get=0 —
  no PBDMA was ever bound to the channel.
- Boot-2 kdisp side printed `takeover-abort no-match` only (expected — display
  read-only this sitting).

Sitting #12 complete, both rungs. Fence lane → pull 14 per the pre-committed
fallbacks (PBDMA CTRL_ADDR TARGET audit; disp-era USERD enablement). Display
lane → pull 3 shape is a Peter decision (decode-first vs write-and-watch).

## Sitting #11 (display pull 1 + kepler pull 12, UnaOS-gemini@9eab5823, 2026-07-22, fox-metal-r23s1h)

**Boot 1 — HEAD 0 IS ALIVE AND SCANNING (major canon correction):**
- `caps version=0210 class=917D` (GK107 display class live);
  `gop phys=0x90020000 vram_off=0x20000`.
- Both candidate EVO mirror layouts REFUTED: evo rows AND hv rows all zero on
  all 4 heads.
- **`head[0] stat underflow=0 vert=0x0493048A horz=0x0000068C` — nonzero
  raster counters, head 0 ONLY** (heads 1-3 stat all zero). The display
  engine was NEVER torn down and NEVER idle; every "evo=crtc=0 all heads →
  engine idle" reading (sittings #5-#10) was wrong-address decode — exactly
  what the panel-owner proof demanded. vert 0x0493/0x048A and horz 0x068C
  decode as plausible raster line/column pairs for the panel timing.
- Sentinel discipline worked: trace cells beyond caps+head0-stat carry the
  DEAD sentinel, so real-vs-absent is unambiguous per cell.
- Display pull 2 target: HEAD_STAT (0x616000-block) is the one genuinely
  decoded block — derive the armed/surface registers outward from ITS
  offsets for the 917D class.

**Boot 2 — USERD_HI bit31 refuted as the poll enable, with a new precision:**
- Witness FAILED, err=2 unchanged, discriminators zero, clean evidenced
  restore (post-restore err=2 stat=0).
- NEW: `inst-raw 0C=80000000` — **the bit31 write PERSISTS in instance
  memory** (read 0 in s9/s10). Coordinator's read, sharper than the bench
  framing: the strip has always been on the PFIFO_CHAN MMIO word (documented
  NO_POLL refusal), NOT on instance bytes — inst writes are visible to US
  via BAR1. The genuinely open question this creates: does the ENGINE see
  our instance bytes at validate time? BAR1 readback only proves BAR1
  self-coherence, not scheduler-side visibility (WC/L2 flush between inst
  writes and validate is now a live hypothesis).
- Fence poll ran bounded, failed as expected.

Sitting #11 complete, both rungs: head-0-alive (boot 1) + inst-writes-persist
/ USERD_HI-refuted (boot 2). Pull-13 targets: (1) VRAM-write→validate
visibility (a cited flush/serialization step), (2) if flushing changes
nothing, the poll area is still elsewhere — widen the derivation. Capture +
MANIFEST are the record.

## Sitting #10 (igpu pull 5 + kepler pull 11, UnaOS-gemini@dffa7816, 2026-07-22, fox-metal-r23s1h)

**Boot 1 — GMUX PROTOCOL PROVEN, PANEL OWNER NAMED (canon reversal):**
- Version 3.2.19 via the 32-bit indexed read (#9's failure was the 3×8-bit
  variant fact, confirmed); MAX_BRIGHTNESS=0x3FF second proof. Gate PASSED.
- Decoded, stable at Boot AND Kernel: **SW_DISPLAY=0x03 (DISCRETE),
  SW_DDC=0x02 (DISCRETE), DISC_POWER=0x03 (ON). The Kepler dGPU owns the
  panel at every observed instant.**
- Canon updates:
  1. iGPU-all-dead (all 4 trace points, reconfirmed this boot) is the
     EXPECTED state — the iGPU paradox is CLOSED, not mysterious.
  2. **Sitting #5's gmux/iGPU redirect is formally REVERSED.** The GOP
     console at 0x90020000 is scanned by the KEPLER; sitting #5's
     "evo=crtc=0 on all heads" is now the anomaly to re-derive: either the
     GK107 PDISPLAY head/scanout decode is wrong for this part, or firmware
     tears the Kepler display engine down at EBS while the gmux keeps the
     panel wired to it (which would produce exactly the black panel).
  3. Display line pivots back to Kepler-side scanout derivation
     (BRIEF-kepler-display-pull1-scanout-rederive in video/Kepler/ — the
     display specialist carries it; module split first so the two Kepler
     lanes don't collide in kepler.rs).

**Boot 2 — Candidate A cleanly refuted:** `USERD_SNOOP orig=0` → write 1 →
witness FAILED (bits stripped), snoop restored; err stays 2, stat 5,
discriminators 0, RAMFC untouched. No residue.
- **Write-behavior pattern (coordinator-refined from Fox's read):** it is NOT
  "PFIFO config writes don't stick" — SUBFIFO_ENG_MASK (0x2390+) and
  PLAYLIST_WR/LEN demonstrably stick. The true split:
  (i) PFIFO_CHAN VALID/POLL — SEMANTIC refusal (chip sets err=2 NO_POLL and
      strips by design; this is the documented CHAN_TABLE_ERROR behavior);
  (ii) USERD_SNOOP 0x2a1c — writes-read-as-zero, UNEXPLAINED: either absent
      on GK107 (rnndb has no variants tag either way), write-gated, or not a
      simple boolean on this part.
- Open hypotheses for pull 12, strongest first: (b) the "poll area" on GK104+
  is per-channel state in INSTANCE memory (USERD pointer/enable in the inst
  block or channel-table INST word), not a global MMIO knob — we may be
  missing an inst-block field, not an MMIO write; (a) a PFIFO
  reset/priv-unlock handshake gating config writes; (c) a sched-block clock
  domain. envytools hwdocs (allowed source) likely documents GK104 fifo
  channel setup.

Sitting #10 complete, both rungs: panel owner PROVEN (discrete), candidate A
refuted, write-behavior pattern named and refined. Capture + MANIFEST are
the record.

## Sitting #9 (igpu pull 4 + kepler pull 10, UnaOS-gemini@785a8795, 2026-07-22, fox-metal-r23s1h)

**Boot 1 — gmux handshake: protocol UNPROVEN, but the picture sharpened:**
- Version self-test FAILED (implausible tuples) → gate held, raw bytes only.
- With the real handshake the values are STABLE boot→kernel (sitting #8's
  0x39→0x03 "movement" is RETRACTED as a handshake artifact — the canon guard
  was right) and the three registers now answer DISTINCT bytes:
  SW_DISP=0x03, SW_DDC=0x02, POWER=0x03 (both points).
- UNPROVEN-decode note (not canon): 0x03 in SW_DISPLAY would read "discrete
  owns the panel" in the classic decode — which would put the GOP console on
  the Kepler side and reopen the wrong-registers question on the KEPLER
  display engine, not Intel. Proving the protocol is now the whole game:
  pull 5 = variant version-reg offsets / gmux revision protocol variants.
- iGPU teardown rows unchanged (all-dead, DP_A=0x1C constant).

**Boot 2 — THE CHIP SPOKE. The wall's name is NO_POLL:**
- `sched-status`: pre-init `err=0 stat=0` → post-init `err=0x00000002` →
  post-submit `err=0x00000002 stat=0x00000005`. CHAN_TABLE_ERROR EXISTS on
  GK107 and names the reject: **code 2 = NO_POLL ("validated a channel with
  POLL_ENABLE, but poll area is disabled")**, fired at channel-VALIDATE time,
  before any runlist submit.
- Sharper coordinator read: the hardware REFUSES the validate — sitting #8's
  dump already showed it (`PFIFO_CHAN[1] 00=0x00002000` after we wrote
  0x80002000: bit31 was CLEARED on readback). The chip rejects and strips
  VALID(/POLL) when the poll area isn't configured. Bit30 "not sticking" is
  the SYMPTOM; the missing "poll area" configuration is the cause.
- `stat=0x00000005` (SCHED_STATUS, RO) post-submit — undecoded; rnndb gives
  no bit meanings.
- rnndb dead-ends on "poll area": the only mentions in gf100_pfifo.xml are
  the NO_POLL code and the POLL_ENABLE bit. Pull-11 derivation must find the
  poll-area config (USERD/BAR1 poll machinery or PFIFO config) elsewhere in
  envytools or empirically.
- Everything else unchanged (discriminators 0, RAMFC untouched, gmux rows
  as boot 1).

Sitting #9 complete, both rungs. **First silicon-named root cause of the
fence wall.** Capture + MANIFEST are the record.

## Sitting #8 (igpu pull 3 + kepler pull 9, UnaOS-gemini@b3ec47d1, 2026-07-22, fox-metal-r23s1h)

**Boot 1 — display paradox, two hard facts:**
- **Point-0 is ALL-DEAD too.** Pipes/planes/PP_STATUS/PP_CONTROL/DPLL_A read
  0x00000000 at all four points (first-instruction-adjacent bootloader entry
  included); DP_A=0x1C constant. Panel power and the PLL were NEVER on at any
  observable instant → the "firmware tears down during our bootloader window"
  theory is DEAD. What remains: wrong-registers or the mux points elsewhere.
- **The gmux answers on the INDEXED protocol and its state MOVES:** indexed
  reads return real bytes (classic PIO = sentinel; absent on this rig);
  values 0x39 at Point-0 → 0x03 at kernel probe. First register on this
  machine that answers differently at boot vs kernel.
- **CAVEAT on the decode (not yet canon):** idx_SWITCH and idx_POWER returned
  IDENTICAL bytes at each point (0x39/0x39, 0x03/0x03). Two distinct
  registers agreeing twice is the signature of an incomplete indexed
  handshake (missing ready-wait between index write and value read) — we may
  be reading a status/stale byte, not per-register data. The boot→kernel
  CHANGE is a real observable; the 0x39/0x03 meanings are NOT decodable yet.
  Pull 4 = full indexed protocol with ready-wait + a version-register
  self-test (known-shape value proves the protocol before trusting
  switch/power).

**Boot 2 — fuzz answered, negatively but sharply:**
- `playlist_rd=0x2013 len=0x00100003` — scheduler READ and COUNTED all three
  entries (len 1→3 vs #7).
- DISCRIMINATOR pbdma0/1/2 all `ch=0 ACTIVE=0` — raw, bit31-valid, and
  bit0-valid entry encodings ALL REFUTED as sufficient.
- `PFIFO_CHAN[1]` pre==post (`00=00002000 04=11000001`); RAMFC untouched by
  hw (0xFACE sentinel intact). The scheduler never writes back anything.
- Synthesis: runlist parse is fine; the gate is a per-channel scheduling
  PRECONDITION. Pull-10 leads (rnndb, cited): GF100's CHAN_TABLE decode has
  bit30 `POLL_ENABLE` + bit31 `VALID` in the CHAN word and bit0 `RUNNABLE` in
  STATE — GK104's "UNK31" is almost certainly VALID, and we never set the
  bit30 analog. And **`CHAN_TABLE_ERROR` (PFIFO+0x52c) is a readable reject
  reason** (codes incl. NO_POLL "validated a channel with POLL_ENABLE, but
  poll area is disabled", NO_ENGINE, INVALID_TARGET) plus `SCHED_STATUS`
  (+0x63c RO) — never read them; the chip may have been naming the reason
  every boot.

Sitting #8 complete, both rungs. Capture + MANIFEST are the record.

## Sitting #7 (igpu pull 2 + kepler pull 8, UnaOS-gemini@7014b022→94b0ed0c, 2026-07-22, fox-metal-r23s1h) — COMPLETE

**Boot 1 (teardown hunt):** three-point trace ran; verdict **ALL THREE POINTS
DEAD** — iGPU pipes/planes/DP_A read disabled even PRE-ExitBootServices.
Firmware never lights iGPU scanout at any stage we can see. (Filter lesson:
the rows carried no "igpu" substring and were initially reported missing;
rows now carry the `:: igpu:` prefix. Residual caveat: an all-zero trace was
also the failed-BAR0-read signature; the bootloader helper now returns the
`0xBAD0BA20` sentinel on a failed read, so the ambiguity dies with the next
boot.)
- Combined with #5 (Kepler heads dead) and #6 (iGPU dark at kernel time):
  **no scanout engine on either GPU is lit at ANY observed point**, while the
  GOP fb at 0x90020000 accepts writes. Display strategy remains open — next
  question is who CAN light a pipe, and what the gmux muxes.
- **CF8-failed-read caveat REFUTED (canon upgrade):** the DP_A row reads
  `0x0000001C | 0x0000001C | 0x0000001C` — nonzero and stable across all
  three points while every pipe/plane row is zero. The bootloader's reads are
  live (a failed read would have zeroed DP_A too). All-three-points-dead is
  REAL. Since the GOP text console is visibly scanned during Option-boot,
  either (a) firmware tears scanout down BEFORE our Point-1 read (Point-1 is
  later in the boot than assumed), or (b) scanout state on these parts lives
  in registers other than the ones decoded. Pull-3 brief targets the split:
  Point-0 at bootloader entry + gmux status readback.

**Boot 2 (all five knobs) — "hard hang" RETRACTED, was SLOW:** full output
landed ~6-7 min in; culprit = unbounded instrumentation polls (10M-read fence
poll through BAR1 + takeover retries), not a wedge. Fixed post-sitting: fence
poll bounded 500k, acceptance poll 100k, and GPU init moved AFTER xHCI so any
future GPU wedge prints breadcrumbs instead of pre-serial silence (bench
serial is the usbdebug FTDI behind xHCI — structural blind spot removed).

**Kepler wall-2:** the ORDER defect was REAL and is FIXED on silicon —
`inst-raw 4C=0x00090000` (was 0x01FF0000; log2(512)=9 took). But **REFUTED as
the bind wall**: post-bind `playlist_rd=0x2013 len=0x100001` (runlist read),
yet all three PBDMAs still `ch=0 ACTIVE=0 ib=0/0`, `gp_get=0`,
fence-timeout, `ch_stat=0x11000001`. The channel is never scheduled onto any
PBDMA. Pull-9 target: runlist entry format/ID encoding, RAMFC fields the
scheduler validates, submit-vs-enable ordering, which-runlist.

**Boot 2r (94b0ed0c, all five knobs) — all post-sitting fixes PROVEN on
metal:** serial-first GPU init (PDISPLAY breadcrumb on the wire), takeover
aborts in seconds (bounded polls), prefixed rows pass the filter. Content
pure confirmation of the above canon (iGPU all-dead, DP_A=0x1C ×3, ORDER
0x00090000, playlist read, PBDMAs unbound). Wall 2 = pull-9's
runlist-entry/channel-bind, nothing else remains.

Capture: `~/unaos-bench/capture/rmbp-r23s6/` + MANIFEST rows.

## Sitting #6 (igpu pull 1 + kepler pull 7, UnaOS-gemini@8105f73c, 2026-07-22, fox-metal-r23s1h) — superseded notes below (boots 1/1b/2b)

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
