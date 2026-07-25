# Video lanes — pull index (update on every status change)

Three pull sequences, numbered independently. Every pull = one BRIEF (coordinator)
+ one PROPOSAL (specialist) + implementation commits. Newest at the top of each lane.

## Lane 1: Kepler FENCE (PFIFO/scheduler — kepler.rs) — `video/Kepler/`, files `*kepler-fence-pull<N>*` (pulls ≤13: `*kepler-pull<N>*`)
| Pull | Files | Status | One-liner |
|---|---|---|---|
| 26 | BRIEF-kepler-fence-pull26-dmactl-require-ctx | BRIEFED — awaiting proposal | clear DMACTL bit0 REQUIRE_CTX (one write) then re-run image A — the named blocker |
| 25 | BRIEF-kepler-fence-pull25-first-ucode | LANDED; s28: upload+page PROVEN (tlb usable, verify exact) but core stays STOPPED (cpuctl 10→12); DMACTL REQUIRE_CTX=1 named | K-GPU-4 m2: first from-scratch ucode — magic to MAILBOX0 + EXIT, FECS only, full citations required |
| 24 | BRIEF-kepler-fence-pull24-falcon-port-probe-real-base | LANDED; s27: ⭐ **UPLOAD PATH PROVEN** — 16/16 sentinels back, both falcons, imem+dmem | sentinel probe at REAL bases (FECS/GPCCS), AINCW/AINCR discipline, zero execution |
| 23 | BRIEF-kepler-fence-pull23-falcon-base-recon | LANDED; s26: ⭐ **FALCONS FOUND** — FECS 0x409000 + GPCCS 0x41A000 real (cpuctl=10 both) | read-only recon of 0x409000 (FECS) + 0x41A000 (GPCCS) — find the real Falcon |
| 22 | BRIEF-kepler-fence-pull22-pgraph-reset-pulse | LANDED; s25: pulse clean, ports STILL dead — BADF1000 = nonexistent-reg signature; spec base wrong? | PMC bit-12 reset PULSE (off→settle→on) then identical port probe — reset-then-enable, not enable-alone |
| 21 | BRIEF-kepler-fence-pull21-falcon-memprobe | LANDED; s24: **PORTS GATED** — all IMEM/DMEM accesses BADF1000 with bit12 set; second gate exists | K-GPU-4 m1: IMEM/DMEM sentinel probe (zero execution) — are Falcon memory ports live post-enable? |
| 20 | BRIEF-kepler-fence-pull20-witness-rematch | LANDED; s23: **REFUTED #7** — strip signature identical with PGRAPH on (err=2, valid stripped) — K-GPU-4 arc begins | re-run s7–s10 witness sequence verbatim with PGRAPH on — zero new writes; either outcome decisive |
| 19 | BRIEF-kepler-fence-pull19-pgraph-enable | LANDED; s22: **ENABLE TOOK** (rb bit12 set) — BADF1200 wall gone, BADF1000+real zeros, Falcon halted/no-ucode | set PMC_ENABLE bit12 (one write) + re-run recon — BADF1200 should become real values |
| 18 | BRIEF-kepler-fence-pull18-falcon-recon / PROPOSAL-kepler-fence-pull18 | LANDED 9dacbd8c; s21: GROUND TRUTH — all-BADF1200, PMC_ENABLE bit12 CLEAR (PGRAPH never powered) | read-only PGRAPH Falcon ground-truth dump |
| 17 | BRIEF-kepler-fence-pull17-latch-correlation / PROPOSAL-kepler-fence-pull17 | LANDED 7ac404d8; s20: latch-delta NONE + pre all-zero — window DEAD ROAD, parked; fallback ladder EXHAUSTED → PGRAPH/ucode pivot is a Peter call | pre-takeover window dump + latch-delta diff |
| 16 | BRIEF-kepler-fence-pull16-beacon / PROPOSAL-kepler-fence-pull16 | LANDED 200be275; s19: NONE-SEEN (not our structures); window stable in-boot (p1→p2 zero deltas) | VRAM beacons in our channel structures |
| 15 | BRIEF-kepler-fence-pull15-mirror-recon / PROPOSAL-kepler-fence-pull15 | LANDED 51b98bab; s18: DELIVERED — window VOLATILE (fill grew 62→158 rows; memory-backed hypothesis) | read-only dense dump of method-mirror header 0x640000–0x6403FC |
| 14 | BRIEF-kepler-fence-pull14-ctrladdr / PROPOSAL-kepler-fence-pull14 | LANDED 384449d7; s13: REFUTED 12/12 (writes stick, witness never latched); recon: EVO 0x490=0D0500A9 live | PBDMA CTRL_ADDR TARGET audit; disp-era USERD = last in-family lead |
| 13 | BRIEF-kepler-pull13-visibility / PROPOSAL-kepler-pull13 | LANDED d63a1495; s12: REFUTED (flush executed, strip persists) | PFIFO_FLUSH before validate — flush is not the missing step; pull 14 = pre-committed fallbacks |
| 12 | BRIEF-kepler-pull12-poll-area-2 / PROPOSAL-kepler-pull12 | LANDED; s11: REFUTED (bit persists in inst, err=2) | USERD_HI bit31 test; SNOOP scrubbed |
| 11 | BRIEF-kepler-pull11-poll-area / PROPOSAL-kepler-pull11 | LANDED 7124e4e1; s10: candidate refuted | USERD_SNOOP test — inert on GK107 |
| 10 | BRIEF-kepler-pull10-sched-precondition / PROPOSAL-kepler-pull10 | LANDED 2650a38b; s9: CHIP NAMED NO_POLL | ask-the-chip error readback |
| 9 | BRIEF-kepler-pull9-runlist-entry / PROPOSAL-kepler-pull9 | LANDED 861b116c; s8: all encodings refuted | runlist entry fuzz + discriminators |
| 8 | BRIEF-kepler-pull8-runlist-bind / PROPOSAL-kepler-pull8 | LANDED 34aa12cd; s7: ORDER fixed, wall moved | ORDER 511→9 |
| 7 | BRIEF-kepler-pull7-pfifo / PROPOSAL-kepler-pull7 | LANDED 0715c94c; s6-7 | PBDMA base/clock instrumentation |
| 4–6 | PROPOSAL-kepler-pull4/5/6(+v2), WALKTHROUGHs, REVIEW-pull6-REJECTED | LANDED (pre-lane-split history) | VRAM/GOP/EVO era |

## Lane 2: Kepler DISPLAY (scanout — kepler_display.rs after split) — `video/Kepler/`, files `*kepler-display-pull<N>*`
| Pull | Files | Status | One-liner |
|---|---|---|---|
| 19 | BRIEF-kepler-display-pull19-relocate-decisive | BRIEFED (re-scoped) — awaiting proposal | full-panel draw at the GOP base, NO latch — make the working framebuffer first-class |
| 18 | BRIEF-kepler-display-pull18-placement-model | LANDED; s28: ⭐⭐ **EVO LATCH REFUTED / FRAMEBUFFER WON** — graphic drew before fill-done, armed=0200 (=GOP FB) never followed; we paint the fw FB directly and it works | specialist-designed probe discriminating scaling / scan-window / latch-granularity in one photo |
| 17 | BRIEF-kepler-display-pull17-row-offset | LANDED; s27: only WHITE visible ~2/3 down — **INCONCLUSIVE** (indistinguishable from fw console; my "impossible" arithmetic was wrong) | marker-row calibration — measure the scan-start row offset; the LAST display variable |
| 16 | BRIEF-kepler-display-pull16-linear-16k | LANDED; s26: ⭐⭐ **MAPPING SOLVED** — seam-free, solid white column (linear pitch 0x4000 confirmed on panel) | ONE linear fill, pitch 16384 — seam-free = MAPPING SOLVED |
| 15 | BRIEF-kepler-display-pull15-mirror-surface-params | LANDED; s25: ⭐ **FW SURFACE IS LINEAR pitch=0x4000** (storage 01004000, size 07080B40) — block-linear road retired | STOP GUESSING: read-only decode of 0x640400–0x6405FC — fw's own pitch/block-mode/size |
| 14 | BRIEF-kepler-display-pull14-bw-pitch | LANDED; s24: **PITCH×BW REFUTED** — all 4 combos cluster-seamed; ladder exhausted → read the mirror | bw {2,4} @ bh=4 × pg {192,256} — pitch padding × bw never tested together |
| 13 | BRIEF-kepler-display-pull13-blockwidth | LANDED; s23: **BW REAL** — periodic seams gone, clusters remain; cleanest (2,4); pitch×bw next | bw {2,4} × bh {4,8} — block wider than 1 GOB is the surviving suspect |
| 12 | BRIEF-kepler-display-pull12-pitchalign | LANDED; s22: **PITCH REFUTED** (identical seams at pg 192 vs 256; count still scales with bh) | bh {4,8} × pitch_gobs {192,256} mini-ladder — zero seams names the real pair |
| 11 | BRIEF-kepler-display-pull11-blockheight / PROPOSAL-kepler-display-pull11 | LANDED 410996eb/366e5b05; s21: no rung clean — monotonic seams → pitch-alignment is the second parameter | four-hold block-height ladder (bh 2/4/8/16) |
| 10 | BRIEF-kepler-display-pull10-swizzle / PROPOSAL-kepler-display-pull10 | LANDED fec4b73f; s20: BLOCK-LINEAR CONFIRMED (checkerboard gone; brick-seams = block-height wrong) | pre-swizzled ruler (GOB 64B×8) |
| 9 | BRIEF-kepler-display-pull9-ruler / PROPOSAL-kepler-display-pull9 | LANDED 3ce77eda; s19: DELIVERED — full cycle compressed ~8×, 16-px checkerboard, no white column → BLOCK-LINEAR hypothesis | ruler fill (64-row color cycle + wide left marker) |
| 8 | BRIEF-kepler-display-pull8-pattern / PROPOSAL-kepler-display-pull8 | LANDED 3bb0621c; s18: DELIVERED — early rows → bottom band; left-bar wraps as dashes (pitch≠11520 hypothesis); no blue/white | same latch, quarters+leftbar pattern fill |
| 7 | BRIEF-kepler-display-pull7-latch / PROPOSAL-kepler-display-pull7 | LANDED 9ff1a9c2; s17: ⭐ PROVEN — GREEN ON PANEL (first UnaOS pixels; latch works; armed-readout puzzle logged) | assembly write (0x640460) + UPDATE latch (0x640080), restore-paired |
| 6 | BRIEF-kepler-display-pull6-assembly / PROPOSAL-kepler-display-pull6 | LANDED 41d26552; s16: DELIVERED — assembly state found (0x640460=0x200 in method-mirror record; UPDATE slot 0x640080) | read-only assembly-state hunt: 0x6101E0 neighborhood + widened scan + armed-pair check |
| 5 | BRIEF-kepler-display-pull5-repoint / PROPOSAL-kepler-display-pull5 | LANDED 896faee0 (+2 inline fixes); s15: REFUTED — 0x6101E0 is read-only armed-state (write snaps back) | repoint 0x6101E0; arming evidently needs core-channel assembly + UPDATE latch |
| 4 | BRIEF-kepler-display-pull4-evo-core / PROPOSAL-kepler-display-pull4 | LANDED b025467b; s14: DELIVERED — single hit 0x6101E0=0x200 (surface pointer candidate); core window stable | EVO core-channel read-out: dense 0x610480 window + known-value scan of 0x610000–0x613FFC |
| 3 | BRIEF-kepler-display-pull3-decode / PROPOSAL-kepler-display-pull3 | LANDED 8ddc7fc3; s13: DELIVERED (0x604/0x614 = telemetry; timing block pinned; no surface addr in windows) | read-only candidate decode (dense windows, 3 passes, arithmetic cross-check) |
| 2 | BRIEF-kepler-display-pull2-head0-anchor / PROPOSAL-kepler-display-pull2 | LANDED f148d199; s12: DELIVERED (49/40 rows, diff decoded — no bare surface addr; 0x310 lead candidate) | capped head-0 dump (96) + head-1 baseline (64) |
| 1 | BRIEF-kepler-display-pull1-scanout-rederive / PROPOSAL-kepler-display-pull1 | LANDED; s11: HEAD 0 ALIVE (mirrors refuted, stat anchor proven) | module split + dual-mirror trace |

(Lane exists because sitting #10 proved the discrete GPU owns the panel; carried
by the ex-iGPU specialist. Sitting-5-era scanout guesses are refuted history.)

## Lane 3: iGPU (Intel HD 4000) — `video/iGUI/`, files `*igpu-pull<N>*` — **ARC CLOSED (s10)**
| Pull | Files | Status | One-liner |
|---|---|---|---|
| 5 | BRIEF-igpu-pull5-gmux-prove / PROPOSAL-igpu-pull5 | LANDED 9d904c96; s10: PROTOCOL PROVEN | 32-bit version read → panel owner = DISCRETE |
| 4 | BRIEF-igpu-pull4-gmux-protocol / PROPOSAL-igpu-pull4 | LANDED 6ee2eaf8; s9 | gmux handshake + version gate |
| 3 | BRIEF-igpu-pull3-point-zero / PROPOSAL-igpu-pull3 | LANDED 2a4a92ff; s8: Point-0 all-dead | Point-0 + gmux first contact |
| 2 | BRIEF-igpu-pull2-teardown-hunt / PROPOSAL-igpu-pull2 | LANDED df4fe972; s7: all-dead pre-EBS | 3-point teardown trace |
| 1 | BRIEF-igpu-display / PROPOSAL-igpu-pull1 (+REVIEW-igpu-pull1) | LANDED f73c85eb; s6 | read-only probe — iGPU dark |

Metal record: `docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md` (sittings, newest first).
