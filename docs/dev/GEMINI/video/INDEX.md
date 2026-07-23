# Video lanes — pull index (update on every status change)

Three pull sequences, numbered independently. Every pull = one BRIEF (coordinator)
+ one PROPOSAL (specialist) + implementation commits. Newest at the top of each lane.

## Lane 1: Kepler FENCE (PFIFO/scheduler — kepler.rs) — `video/Kepler/`, files `*kepler-fence-pull<N>*` (pulls ≤13: `*kepler-pull<N>*`)
| Pull | Files | Status | One-liner |
|---|---|---|---|
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
| 5 | BRIEF-kepler-display-pull5-repoint | BRIEFED (Peter-approved FIRST WRITE) — awaiting proposal | repoint 0x6101E0 to a prepared surface, panel is the verdict; restore |
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
