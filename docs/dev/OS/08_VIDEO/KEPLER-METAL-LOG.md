# Kepler bring-up — metal facts of record (rMBP GT 650M / GK107)

Hard-won silicon facts from the fox-metal sitting series. Trust these over any
QEMU behavior. Newest sitting first.

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
