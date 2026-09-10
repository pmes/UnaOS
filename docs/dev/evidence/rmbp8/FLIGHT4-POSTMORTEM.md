# rmbp8-FLIGHT 4 — POSTMORTEM

**Capture** `~/unaos-bench/capture/rmbp8-flight4/ttyUSB0.log` — SEALED, 2 343 810 B,
15 910 lines, **TWO boots**. Boot 1 = L1–8871 (bytes 0–1 407 330), `[?ms]` →
`[420079ms]` (~7 min 00 s; last `[deadman] up=405`). Boot 2 begins at **L8872 /
byte 1 407 331** (timestamp reset to `[?ms]`; the brief's "~1.12 MB mark" is
corrected to 1 407 331), runs to `[364165ms]` (~6 min 04 s; last `[deadman]
up=349`). Both boots end at power-off mid-rollup — no shutdown marker.
**Image** `rmbp8flight4 @ bdddbdcd`, knob line = flight-3 line + `UNAOS_IVB3D=1`
(R7 armed; build banner `[16198ms] GPACE … build=kepler+takeover+fifo+ivb+wc+smc+`).
Panel 2880x1800, pitch 16384 B (`[?ms] video: WRITER seeded base=90020000
len=29491200 … stride=4096px pitch=16384B bpp=4`, L1).
**Predictions** `~/unaos-bench/PLAYBOOK-rmbp.md` (this flight).
**Flight-3 baseline** `~/unaos-bench/scratch/rmbp8/FLIGHT3-POSTMORTEM.md` (read in full).

All extraction with `awk`/`grep -a` (control bytes present). `L<n>` = line number
in the sealed file (boot-2 lines are global). `[Nms]` timestamps are boot-local.
§6 carries byte offsets.

---

## 1. Verdicts vs the playbook's seven predictions

| # | Prediction | Verdict |
|---|---|---|
| 1 | **R7 — the first IVB blit** | **MET VERBATIM, both boots.** `r7 verdict=r7-blit-verified … attempts=1/3 … best_dst_match=256/256` at `[15744ms]` boot 1 (L1565) and `[12199ms]` boot 2 (L10467). The pre-registered falsifier — wake held, `ctl_readback==0x1`, `head_moved=1`, `sentinel_hit=1`, `dst_match=256/256` — is satisfied field-for-field. None of the four failure tripwires fired (no drain-timeout, no head-stuck/sentinel-miss, `settled=1` both boots, no 16-px stripe garbling — `dst_crc==src_crc=D7994E3D`, so the bytes-not-DWords pitch ruling is now metal-proven). One residual: `reclaim=leaked` where the falsifier note expected `reclaim=freed` — structural, see §3.1. |
| 2 | **The wedge names its store** | **MET at the field level, sibling line UNFLOWN.** All 12 `PASS OVERDUE` ticks and all 3 `GATE STOLEN` lines carry `blits_retired= blit_aim= blit_inflight=`; every `blit_aim` decodes exactly to its own stated row (§3.2); frozen odometer + `blit_inflight=1` observed on the wire at T0 at all three strikes. The predicted `[wedge1] BLITAIM` sibling with per-core `inflight=[8] aim=[8] dead=[8]` **never printed** — zero matches in either boot (it evidently hangs off the `[wedge1]` tripwire latch, which stayed `tripwire=silent` all flight). Instrumentation gap for D-1. |
| 3 | **Debt forgiven, drains complete** | **MET.** `blit debt forgiven` + `blit debt heal: full present queued` at every steal; `abandoned=0` on every `[wedge1]` line of both boots; flight max `spin_max=1342654` cycles (boot 1) / `1537734` (boot 2) vs flight-3's three 1 073 741 824-spin unwinnable episodes — a ~780x reduction in worst-case spin, and zero `DRAIN STALLED`/`DRAIN ABANDONED` lines anywhere. §3.3. |
| 4 | **The shell comes back** | **MET.** `shell re-minted win=2 — corpse row adopted in place` after every rehome (+214 ms, +27 ms, +33 ms). Adoption, not stacking: exactly one `win=2` row in every `[wcn]` census, zero re-mint declines. The typing proof landed hard: after re-mint #2 Peter typed `pulse` (`[157013..159010ms]` keystrokes on the wire) and the pulse app window `win=10` was created at `[160393ms]` — 39 ms after the trailing space. Re-mint #3's typing proof went unexercised (no keystrokes after ~160 s). §3.4. |
| 5 | **PRTSCR gets a target** | **PARTIAL — arrival fired, capture FAILED.** Boot-1: healthy veto only (no stick offered). Boot-2: `writable volume arrived (source=global label=UNAOS-DATA)` at `[64125ms]`, then `write failed -EIO` at `[71996ms]` and `PRTSCR-ST: FAIL` — terminal. All three named defects confirmed on the wire, plus a fourth found by the full read: the stick **dropped off the bus mid-write** (hot-plug re-enumeration at `[69971ms]` while the write to LBA 5952 was in flight). No `SCREEN0.PNG`, no `IEND OK`. §3.6. |
| 6 | **BT intermittency discriminator** | **ANSWERED — branch 1: transient, intermittency CONFIRMED, in one capture.** Boot-1 inquiry stone deaf (`responses=0`, mode 0x01 latched, bits 1/33/46 all open) yet a **blind page linked 543 ms later** (`[13796ms]`, attempt 1/2, `aligned_by_inquiry=false`); boot-2 inquiry **heard the target in 338 ms** (`rssi=-89dBm`) and went on to the project's first BR/EDR pairing, encryption, and open L2CAP channel. LE control link healthy both boots (2400/2368 ms). `.hcd` not staged (operator's asset unplayed), so patchram itself remains open — but the "dominant deaf state" reading is dead. §3.5. |
| 7 | **Input survives the era** | **MET.** `[deadman] hid` resumed counting after every recovery (resumptions at 114.5 s and 123.4 s land full bursts, hid up to 84/s); no phantom-tid witness fired anywhere (SCHEDUAF silent = pass-by-silence). TOPPORT unexercised — the operator never typed `top`/`ps` (the complete boot-1 keystroke record is `storm`, 17x Tab, `jobs`, `pulse`). |

---

## 2. Two-boot timeline

### Boot 1 (L1–8871) — the working boot: 3 steals, 3 rehomes, 3 dead cores

| [ms] | L | event |
|---|---|---|
| 2400 | 413 | LE Connection Complete handle=0x0040 peer=88:c6:26:cc:2d:3c (healthy control, boot 1 of 2) |
| 3009–13253 | 431–443 | classic inquiry, full 10 240 ms, `responses=0 … HEARD NOTHING` (mode 0x01 latched, mask bits 1/33/46 open) — deaf-inquiry boot #3 |
| 13796 | 454 | **blind page LINKS on attempt 1/2** — `Connection Complete status=0x00 handle=0x000b`, `aligned_by_inquiry=false` |
| 13868/13869 | 466–467 | SSP fails: `Simple Pairing Complete status=0x18 -> NOT PAIRED`, `Authentication Complete status=0x05` — peer never answered IO-cap (`peer_io=none`) |
| 15378 | 475 | L2CAP AVDTP stuck at `PENDING`, round-2 window expires; clean teardown (`links_established=1 disconnections_confirmed=1`, L481) |
| 15744 | 1565 | **`r7 verdict=r7-blit-verified` — first IVB blit on metal** (r5 `enable-void` L1402, r6 `sentinel-hit` L1483 precede it, §3.1) |
| 15846 | 1722 | desktop-clear 2880x1800; shell win=2 asid=0xffffff02 created 16230 (L2242) |
| 16224 | 2223 | KBDWIT addr=5 kbd halted/retired again (xact-err-burn; addr=6 boot-mouse likewise) — DEADKBD5 unresolved |
| 16229 | 2240 | PRTSCR-ST veto (healthy; now says "a FAT USB volume plugged in NOW will be adopted on arrival") |
| 61052–64825 | 3426– | Peter types **`storm `**; six 288x288 vug windows win=4..9 created 64837–64906; last HID delivery ~65.0 s |
| 69074–72074 | 3693– | `PASS OVERDUE holder=c1 … win=8 phase=33 at=span-flush row=801 blits_retired=2147593 blit_aim=0xc84030 blit_inflight=1` x4 — fires **4.1 s after input STOPPED** |
| 72081 | 3719 | **STEAL 1** c1→c6 after 4006 ms; forgiven 72091; REHOME c1→c2 72096 (`rh=1` 72643); **re-mint 72295** |
| 89307–97195 | — | 17 Tab keystrokes into the re-minted shell; EHCIDARK kbd `max=117ms` first appears (L→§4c) |
| 114473 | — | input resumes after 15.9 s gap (`hid=20`, burst to 84/s) |
| 118495–121494 | 4620– | `PASS OVERDUE holder=c2 … win=7 phase=33 at=span-flush row=585 … blit_aim=0x925c38` x4 — **4.0 s after input resumed** |
| 121747 | 4661 | **STEAL 2** c2→c5 after 4252 ms; forgiven/REHOME c2→c3; **re-mint 121774** |
| 148978–160354 | — | Peter types **`jobs `** then **`pulse `** — keystrokes land in the twice-re-minted shell |
| 160393 | 5307 | pulse app window created: `create win=10 asid=0x8 surf=128x128 … z=109` + `PULSE-A: start pid=56` — **39 ms after the last keystroke: the re-mint proof** |
| 164668–167214 | 5450 | **the drag**: `drag-begin win=10 … (1729,693)` → `drag-end … (5,1027)`. Coincides with the flight's max `[wedge1]` dwell (`spun=6 spin_max=1342654` at 166780) and the onset of win=10's banding (4→246 in 6 s from 166333) — §4b |
| 204404/211820 | 6060/6206 | Peter closes the shell (`[wm-act] close … settle=closed`) and reopens it — user action, not a re-mint |
| ~264.8 s | — | last HID input of the boot (hid_ms=22753 at 287346) |
| 287286–290285 | 7254– | `PASS OVERDUE holder=c3 … win=6 phase=33 at=span-flush row=519 … blit_aim=0x81d2e0` x4 — fires **22.5 s INTO input silence** |
| 290538 | 7274 | **STEAL 3** c3→c6 after 4252 ms; forgiven/REHOME c3→c4; **re-mint 290571** |
| 420079 | 8871 | power-off. End state: `[wcpar] cores=5 … c1=c2=c3=0` (417534), `[wcser] steals=3`, amp 1.63x, presents flowing |

### Boot 2 (L8872–15910) — the quiet boot: 0 steals, BT triumph, PRTSCR failure

| [ms] | L | event |
|---|---|---|
| 2368 | 9291 | LE Connection Complete (control repeats) |
| **2869** | 9301 | **inquiry result — the target ANSWERS**: `addr=88:c6:26:cc:2d:3c psrm=0x01 clock_offset=0x7baa class_of_device=240418 (AUDIO/VIDEO Headphones audio_sink=true) rssi=-89dBm event=0x22` — 338 ms into the window |
| 7997 | 9318 | page attempt 1/2 (harvested offset) PAGE TIMEOUT, full 5123 ms |
| 9922 | 9330 | **attempt 2/2, phase-stepped 640 ms — BR/EDR LINK** (`aligned_by_inquiry=true`) |
| 10251–10322 | 9348– | **SSP: PAIRED status=0x00** (just-works numeric 534418 auto-accepted), link key type 0x04 stored (RAM), AUTHENTICATED, **ENCRYPTED (E0)** — all firsts |
| 10334–10339 | 9359– | **L2CAP AVDTP channel OPEN** (dcid=0x0480), configured both directions (peer MTU 0x037f, host MTU 48) — first open classic channel |
| 11839–11920 | 9373– | AVDTP_DISCOVER unanswered (1500 ms window); channel closed by agreement; link released reason=0x16. `C1 tally … inquiry_responses=1 page_aligned_by_inquiry=true pages_attempted=2 links_established=1` (L9382) |
| 12199 | 10467 | **`r7 verdict=r7-blit-verified`** — deterministic repeat, same fields |
| 12300/12616 | 10624/11137 | desktop-clear; PRTSCR veto |
| 63421 | 12254 | **stick hot-plug** enum Port 1 (`BPACE enum:p1 … d=50733ms`); STORAGE BRING-UP 63647; FAT32 `UNAOS-DATA` mounts 64120 (7 v3d-dump files); `PSRC: psrc=global … verdict=undecided … handles=global=present sdhc=present` 64123 |
| 64125 | 12377 | **PRTSCR-ST arrival** `(source=global label=UNAOS-DATA)` — deferred selftest runs |
| 69971 | 12432 | **the stick DROPS and re-enumerates mid-write** (`device connected (hot-plug); queuing for enumeration` while slot 2 mid-transfer) |
| 71946–71996 | 12456– | EP0 pump timeout; both CLEAR_FEATURE(HALT) fail (`no ep0_ring for slot 2`); `BOT: recover done reset=fail halts=fail ring=fail`; `BLK: io-cause op=write lba=5952 bot_err=TransferError(4)`; **`PRTSCR: write failed -EIO (Io; handles=global=absent sdhc=present)` → `PRTSCR-ST: FAIL`** |
| 72631 | 12636 | first FRGUARD SUBSTITUTION line (quoted in full §3.6); repeats 74899/205512/253659 |
| 123337–146.5 s | — | input burst (only real interaction of the boot); one 128x128 app window created 129876 |
| 204812 / 252836 | 14240/14812 | stick replug enumerations (Port 1, then **Port 5**) — mount + FRGUARD + PIUSB geometry probe each time, **ST never re-arms** |
| 364165 | 15910 | power-off. `steals=0`, `[wcpar] cores=2`, zero dead cores |

---

## 3. Per-lane findings, with flight-3 deltas

### 3.1 R7 / the gen7 ladder (GEN7-3D)

* **The ladder's own story, told identically twice.** R5 armed the ring with no
  forcewake hold: `r5 verdict=enable-void … ctl_wrote=00000001 ctl_readback=00000000`
  (L1402/L10304) and its own `next=` line named the suspect —
  `STOP-RING_CTL-enable-did-not-latch-likely-forcewake-released-R6-must-hold-forcewake-and-rearm`.
  R6 held the wake: `r6 verdict=r6-sentinel-hit by=mt … attempts=1/3` (L1483/L10385),
  head 0→0x20, sentinel 5EED1234 landed. R7 moved the same envelope to the BCS with a
  real `XY_SRC_COPY_BLT`: `ctl_readback=00000001 head_post=00000040 … sentinel_post=0B75C0DE
  sentinel_hit=1 dst_match=256/256 dst_crc=D7994E3D src_crc=D7994E3D … iters=3 cyc=7396`
  (boot 1 exec col, L1535; boot 2 `cyc=7112`, L10437). The drain column settled instantly:
  `ring_idle=1 drain_iters=0 drain_cyc=28 … settled=1` — `col=exec == col=drain = 256`,
  so the G1 STOP condition (drain below exec) is excluded by measurement, both boots.
  Enable-void → sentinel-hit → blit-verified is **deterministic on this part**, not marginal:
  same candidate (`mt`, class BDW-ONLY), same attempt count (1/3), 7.4k/7.1k cycle budgets
  of a 50M allowance.
* **The residuals, each read against gen7.md §2.7:**
  - `fw_evidence=blind` (r7 summary; the attempt line says `classification=fw-no-decode
    fw_evidence=0`, vs r6's `fw-req-decodes-no-ack fw_evidence=real`): the forcewake
    ack/request register never positively decodes on this silicon even while the BCS
    demonstrably executes. The hold works (the blit is the proof); the *evidence channel*
    doesn't. **R8 must not gate on a forcewake ack decode** — the hold-across-the-arm
    discipline plus behavioural witnesses is the only currency this part honors.
  - `battery_moved=0/17`: the 17-register GT observation battery stayed all-zero through a
    verified 1 KiB engine DMA. The battery is **not a liveness witness on IVB** — a rung
    that used "battery moved" as its wake proof would score this healthy boot dead. R3's
    `gt-live-already` by-none verdict (pre_live=1) is retroactively vindicated.
  - `tlb=tlb-flush-write-silent` + `reclaim=leaked pages=3 bytes=12288
    reason=no-invalidation-evidence`: `GFX_FLSH_CNTL` (0x101008) read 0 before and after
    the write — exactly §2.6's "self-clearing flush register and a non-decoding offset are
    indistinguishable" case. The reclaim invariant then refuses the free, correctly:
    `note=an-unpinned-register-is-never-the-sole-reason-a-page-goes-back-to-the-heap`.
    **This means the falsifier's watch-item 1 ("expect … reclaim=freed") is structurally
    unreachable on this part as coded**: the engine quiesced (`all_ring_idle=1
    engine_quiesced=1`), yet freed can never be reached because the only pinned
    invalidation register never yields read evidence. Cost: r5 (2) + r6 (2) + r7 (3) =
    **7 pages / 28 672 B leaked per armed boot**, bounded and one-shot. R8 decision:
    either pin an alternative invalidation witness or promote the bounded leak to
    documented-accepted and delete the `reclaim=freed` expectation from the falsifier.
* **What R8 inherits**, in the rung's own words: `r7 next=DONE-the-BCS-copies-pixels-under-a-
  held-wake-wire-bring_up_blt_ring-to-the-held-wake-and-fix-blitter_copy_rect-DW0-client-field`
  (L1566). The pitch-bytes ruling, the ordering barrier, and the teardown gate are all
  metal-proven; the path to a production blitter is engineering, not physics.

### 3.2 The wedge and WEDGESRC (D-1)

* **Three strikes, one signature.** All three holders died in `phase=33 at=span-flush`
  with `blit_inflight=1` and a frozen odometer across all four OVERDUE ticks:

  | steal | [ms] | holder→thief | win | row | blit_aim | decodes to (pitch 16384 B, 4 B/px) | blits_retired |
  |---|---|---|---|---|---|---|---|
  | 1 | 72081 | c1→c6 (4006 ms) | 8 | 801 | 0xc84030 | row **801** + x=12 px | 2 147 593 |
  | 2 | 121747 | c2→c5 (4252 ms) | 7 | 585 | 0x925c38 | row **585** + x=1806 px | 7 271 192 |
  | 3 | 290538 | c3→c6 (4252 ms) | 6 | 519 | 0x81d2e0 | row **519** + x=1208 px | 32 108 171 |

  Every aim decodes **exactly** to its own line's `row=` (aim = row x 16384 + 4x, x within
  the 2880-px visible width; all three inside the 29 491 200-B panel extent at 8.1–13.1 MiB).
  The D-1 deliverable is delivered: the stuck store's BAR1 byte offset, on the wire, at T0.
  The three aims are **scattered** (rows 519–801, x 12–1806) — the wedge is
  **path-pinned (span-flush row blit), not address-pinned**. Victim wins 8/7/6 are all
  storm-vug-era windows.
* **Odometer rates between strikes:** ~38 k blits/s to steal 1, then
  (7 271 192−2 147 593)/49.67 s = **103.2 k/s** in era 1, and
  (32 108 171−7 271 192)/168.79 s = **147.2 k/s** in era 2 — retired-blit throughput
  *rises* with each rehome (the D-2 free-run amplifying on fewer cores), then freezes dead
  at each strike with one blit in flight. Flight-3's conviction stands with better
  fingerprints: tens of millions of identical stores retire, then exactly one does not.
* **Boot 2: zero OVERDUE, zero steals in 364 s** — but under a near-idle desktop (no storm,
  no pulse, one 128x128 app at 129.9 s, `[wcser] … steals=0 declined_pct=0 -> SOLO`
  throughout). This is idle-doesn't-wedge again, not evidence of remission.
* Gap: the `[wedge1] BLITAIM` per-core sibling never printed (§1 row 2). Re-hang it off
  the `[wcser]` steal path where the per-steal fields already live.

### 3.3 DEBTCLEAR and the drains (flight-3 Q1 — the fix under test)

* At every steal, in order: `:: [wcser] blit debt forgiven core=N owed=1 active_now=1
  taint=armed == debt-clear ::` (72091, 121753, 290545) then `:: [wcser] blit debt heal:
  full present queued — every row re-damaged, whole-panel fill owed == debt-clear ::`
  (72109, 121835, 290627). Three for three.
* **No drain regressions anywhere.** `grep -a` for `DRAIN STALLED|DRAIN ABANDONED|STALLED|OVERDUE`
  yields only the 12 `[wcser] PASS OVERDUE` ticks — zero `[wedge1]` tripwire fires, zero
  abandons. Every `[wedge1] dwell` line in both boots reads `tripwire=silent … abandoned=0`.
  Worst spins: boot 1 `spin_max=1342654` (166780 ms, during the win=10 drag; the seat's
  mid-flight "~415 888 max" was one dwell line, not the flight max — 1 301 063 at 36 472 and
  1 342 654 at 166 780 exceed it), boot 2 `spin_max=1537734` (33 131 ms, boot churn). Against
  flight-3's three episodes at the 1 GiB bound: the unwinnable-spin disease is **cured**
  (~780x lower worst spin, all self-resolving DWELL/QUIET). Post-steal window operations
  completed normally — the 204 404 ms user `close win=2` settled in 43 ms.
* New family `[wedge11] overlay-claim` reports every scope ABSORBED (`abandoned=0 owed=0`)
  all flight — the overlay-claim arm of the fix also held.

### 3.4 WCSER-REMINT (flight-3 Q "corpse shell")

* Re-mints at +214 ms / +27 ms / +33 ms after steals 1/2/3 (L3761, L4675, L7288):
  `:: [wcser] shell re-minted win=2 — corpse row adopted in place (1440x883 surface
  repointed, fresh console bound, keyboard handed to the shell) == witness ::`.
* **Idempotent on the wire**: adoption produces no `[wc-a] create`/`close` events — win=2
  appears exactly once in every `[wcn]` census of the flight; zero decline lines. The only
  close/create pair (204404/211820) is a user action (`[wm-act] close … settle=closed`).
* **The keystroke proof**: era-1 shell absorbed 17 Tabs (89.3–97.2 s); era-2 shell received
  `jobs` and `pulse` and *executed* them — `create win=10` + `PULSE-A: start pid=56` at
  160393, 39 ms after the final keystroke. Era-3's re-mint was never typed at (input had
  ended 25 s before steal 3). Two of three eras proven; the mechanism has no counterexample.
* Flight-3 delta: flight-3's wedge closed the shell and left focus strandable on kernel
  furniture; this flight the shell was continuously available and used. FURNITUREFOCUS did
  not recur (no post-boot `focus asid=0xffffff02` hit-test raise in either boot).

### 3.5 Bluetooth — the sharpened deafness verdict

* **Stated precisely, with the controller's own state quoted:** boot 1 ran inquiry with
  `inquiry_mode=0x01` **LATCHED** (`HCI_Write_Inquiry_Mode (0x0C45) mode=0x01 status=0x00`
  + read-back `inquiry_mode=0x01 wanted=0x01`) and event mask `0x2000_5FFF_FFFF_FFFF` with
  "all THREE inquiry-result shapes (0x02 bit 1, 0x22 bit 33, 0x2F bit 46)" open, ran the
  full 10 240 ms, and heard **nothing** (`responses=0 target_found=false read_to_term=true`,
  L444). **543 ms later** the same radio's blind page — `psrm=0x02` fallback,
  `clock_offset=0x0000` bit-15 clear, "the controller is sweeping for the phase" — completed:
  `Connection Complete status=0x00 handle=0x000b link_type=0x01(ACL)` at 13796,
  `page summary attempts_run=1/2 … page_timeouts=0 aligned_by_inquiry=false` (L455).
  Paging requires the master to *receive* the slave's ID response on the page hop sequence —
  so **the receive chain worked, seconds after inquiry-RX heard nothing, in the same boot**.
* Boot 2, same silicon, same image, ~7 minutes later: `responses=1 target_found=true`
  at 338 ms with an RSSI reading (−89 dBm) — then a page-timeout on the *aligned* first
  attempt, and a link on the phase-stepped second (`phase_step=LANDED intended==measured`).
* **What this does to BTRX-PATCHRAM:** the hypothesis survives, but its shape is forced.
  It can no longer be "the unpatched ROM's RX path is dominantly dead" (flight-3's leading
  read). It must now explain a **boot-scoped, inquiry-scan-only deafness that spares
  page-response RX within the same boot** — e.g. per-boot inquiry-substate initialisation
  the missing patch would fix. Host event plumbing stays excluded (all shapes unmasked,
  read to termination, both boots). The staged-`.hcd` control (playbook step 5) was not
  played — `bt-c1` confirms "no such file is staged" and the FRGUARD/wifi lines show
  `/B43/` carrying only the three wifi blobs. **The patchram discriminator is still owed**;
  it is now the only clean experiment left on this question.
* **The road past the link is the boot-2 headline** (all firsts for the project):
  SSP just-works completes (`Simple Pairing Complete status=0x00 -> PAIRED`, numeric 534418
  auto-accepted, key type 0x04), **Authentication Complete status=0x00**, **Encryption
  Change … ENCRYPTED (E0)**; L2CAP AVDTP channel fully open and configured both directions
  (`L2CAP channel state — scid=0x0040 dcid=0x0480 … -> OPEN`); `AVDTP_DISCOVER` then
  goes unanswered for 1500 ms and the stage tears down by the book (`channel … CLOSED BY
  AGREEMENT`, link released reason=0x16). Boot-1's post-link road contrast: the same peer
  left SSP hanging (`peer_io=none`, `Simple Pairing Complete status=0x18`, Authentication
  Complete status=0x05) and L2CAP at `PENDING (result=0x0001 status=0x0002 — commonly an
  authorisation step on the device)` — consistent with a speaker that had not yet decided
  to authorise an unpaired host, i.e. peer-side state, not transport.
* LE residuals unchanged: OUT accepted 15/15, no ACL IN in 600 ms (LE receive still
  unproven); boot-1's early LE HCI_Disconnect refused status=0x12 with the disconnect
  already walked past — known shape.

### 3.6 PRTSCR — the three named defects, verified, plus a fourth

* **Boot 1**: exactly one PRTSCR line — the veto at 16229, now with the new arrival
  promise ("a FAT USB volume plugged in NOW will be adopted on arrival"). Healthy.
* **Boot 2, the full sequence:** arrival at `[64125ms]` after the 63 421 ms hot-plug;
  the volume is `UNAOS-DATA` (the Pi v3d-dump stick, 7 files, FAT32, 3 968 000 sectors).
  The selftest's write to **LBA 5952** was in flight when the stick **dropped off the bus**:
  `[69971ms] xHCI: [Port 1] device connected (hot-plug); queuing for enumeration` — then
  `EP0 sync pump TIMEOUT`, `CLEAR_FEATURE(HALT) ep 0x82/0x01 unexpected Err` with
  `why=nocompletion` and `push_ep0 failed, no ep0_ring for slot 2`, `BOT: recover done
  reset=fail halts=fail ring=fail`, `cause=TransferError(4)` →
  `:: BLK: io-cause op=write lba=5952 bot_err=TransferError(4) ::` →
  `:: PRTSCR: write failed -EIO (Io; handles=global=absent sdhc=present) — capture skipped ::`
  → `:: PRTSCR-ST: FAIL — the capture itself refused (line above) ::` (71996).
* **Defect 1 — ST does not re-arm: CONFIRMED.** The same stick re-mounted twice more
  (204 812 Port 1; 252 836 **Port 5**), each time producing a FAT mount, an FRGUARD line
  and a PIUSB geometry probe — and exactly **one** "writable volume arrived" line exists in
  the whole flight. FAIL is terminal by construction.
* **Defect 2 — arrival/write predicate mismatch: CONFIRMED, with the mechanism.** Arrival
  keyed on the *global* handle (`source=global`; `PSRC … handles=global=present
  sdhc=present` at 64123); by write time that handle was dead
  (`handles=global=absent sdhc=present`) because the device had left the bus. The two
  predicates sample the same handle at different times with no revalidation between.
* **Defect 3 — the rung-2 Usb handle never engaged: CONFIRMED.** No `usb` handle ever
  appears in any `handles=` census; the arrival rode the global slot. The Usb-side
  machinery only ever ran its read-only probe:
  `:: PIUSB: [usbw] scratch geometry: USB last_lba=3970047 (num_blocks=3970048), keep-out
  ceiling=3970048 [mbr-partition-table] ::` + `scratch skipped: on-disk container spans the
  medium … refusing to RMW inside a live volume ::` (205520, 253669).
* **Defect 4 (new from the full read) — the bus drop itself.** Port-1 re-enumerations at
  63 421 / 69 971 / 72 723 / 74 186 (PORTSC=0xe03) plus two `xHCI: >>> COMMAND FAILED
  (Code 4) <<<` during recovery: this stick/port connection is flaky enough that even a
  re-arming ST would have raced it. A retry-on-next-arrival design (defect 1's fix) also
  answers this; a different stick/port is the cheap bench control.
* **FRGUARD held, quoted in full** (fired at every mount — 72631, 74899, 205512, 253659):
  `:: FRGUARD: SUBSTITUTION — the boot volume serial 0xc27415fd is on the INTERNAL Sdhc
  card (blocks=124735488), not in the Default slot (blocks=3970048, 1 FAT volume(s), none
  of them ours); Default writes REFUSED ::` — the substitution guard correctly refused to
  let the foreign stick impersonate the boot volume, four times.

---

## 4. The felt symptoms, quantified

**(a) "vugs very rough, banded flicker."** The dominant bander is **win=10 — the pulse
window** (created by Peter's own `pulse` at 160.4 s; 128x128 @ 6x, `PULSE-A … frame_ms=50`),
not the storm vugs: final tally `torn=78 banded=12619` against `presspop=17114` —
**74 % of its presents were banded**, accruing at a steady ~44 banded/s from the moment of
the 164.7 s drag (banded 4→246 within 6 s of `[166333ms]`) to power-off. The seat's
mid-flight `win=10 torn=70 banded=9324` at 354 s sits exactly on this ramp. The six storm
vugs banded far less (win=9: 853; win=5: 153; win=6: 114; win=4: 28) but window-band pop
tells the same story panel-wide: `[wc-b]` final `presents=80208 banded=19446` (24 %).
Era-correlation: win=10 was born into era 2 (two dead cores) and banded from birth — the
banding is a property of the degraded era, matching flight-3's restated law (era first,
presspread second). Boot-2 contrast at zero dead cores: every window `torn=0`; its lone
oddity is the shell's recomposites being ~all banded (win=2 `banded=10970 whole=27`,
dragged in by win=3's 3-frame cadence) — visually invisible at 4/s, but worth one look.

**(b) "pulse window drag left a trail."** The drag is on the wire: `[wm-act] drag-begin
win=10 owner=0x8 at (1729,693)` (164668) → `drag-end … (5,1027)` (167214) — a 2.5 s
cross-panel drag of the pulse window, in era 2. Correction to the seat's attribution: the
cited `win=5 dkpx=146568 dout=104` belongs to a **storm vug**, not the dragged window, and
is its *steady state* for the whole flight (dkpx 136k–157k every 5 s rollup from 233 s to
417 s, drag or no drag). Per wm.rs (L13195–13202): `dout` counts drag-in edges caused by a
window's own damage, `dkpx` is the whole-box promotion bill in kilopixels. The dragged
win=10 shows `dout=0 dkpx=0` always — it is a pure *draggee* (`drg` 199–231 per 5 s).
So dkpx does not measure the trail; what coincides with the trail is:
(i) the flight's **largest wedge-drain dwell** exactly during the drag —
`[166780ms] [wedge1] dwell drains=16 spun=6 spin_max=1342654 … -> DWELL` (move-flush
drains spinning while the vacated rect repaints); (ii) banding onset on win=10 at the drag
(above); (iii) the era itself — two dead cores meant vacated-rect repaint lagged the
cursor. It does **not** correlate with a heal-present: the nearest steal is 123 s away.
The post-steal-3 heal shows separately as a 5 s att/comp collapse on win=5
(`att=132 comp=15` at 291140) — a different, later event. Lane conclusion: the trail is
STEALTAIL material (degraded-era repaint latency during moves), not DEBTCLEAR's.

**(c) "mouse cursor sluggish."** `EHCIDARK addr=8` dark-window maxima, both endpoints:

| era (dead cores) | vendor-mt (trackpad) max | kbd max |
|---|---|---|
| boot 1 pre-steal (0) | 34–37 ms | 11 ms |
| era 1 (1) | 69–72 ms | **117 ms** (first at 89307) |
| era 2–3 (2–3) | **114–117 ms pinned** (155296 on) | 117 ms |
| boot 2 whole boot (0) | ≤20 ms | ≤9 ms |

The degradation is monotone in dead-core count and saturates at 117 ms (~7 frames of
input-to-service dark) — worse than flight-1's 80–96 ms, and 3.2x this flight's own
healthy baseline. Boot 2's ≤20 ms at zero dead cores is the controlled contrast inside the
same capture: **the sluggishness is a casualty of core loss, not of the input stack.**
`[rtwit] in2present_max_us=--` all flight — the input-to-present ruler never engaged, so
EHCIDARK remains the only latency witness; INPUTKICK/STEALTAIL should wire in2present up.

**Input gaps, complete map** (`[deadman]` hid_ms, gaps >10 s):
Boot 1: 29.1–51.9 s (22.8), 66.4–85.0 (18.6), 98.6–114.5 (15.9), 120.8–147.3 (26.5),
169.9–201.1 (31.2), 214.9–260.3 (45.4), then **270.9 s → power-off** (148.8 s running,
`hid_ms=148753` at 419660 — the seat's `hid_ms=51753` at 317 s sits inside it). All but
the last end in resumption. Boot 2: 18.3–85.9 s (67.5), 92.9–123.4 (30.4), then
**146.5 s → power-off** (217.6 s, `hid_ms=217635` at 364165).
**Flight-3's law (steals fire 1–2.5 s after input resumes) tested against boot-1's three
steals: 1 of 3.** Steal 2 fits loosely (overdue 4.0 s after the 114.5 s resumption, during
an 84 reports/s burst). Steal 1 fired 4.1 s after input *stopped* (the storm vugs' creation
burst was the paint source). Steal 3 fired **22.5 s into input silence** (last delivery
~264.8 s, overdue 287.3 s) with only program-driven paint (six vugs + pulse at 20 fps) on
screen. Restated law: **the wedge follows sustained paint bursts; operator input is one
source of them, not the trigger.** Boot 2 corroborates from the other side: 364 steal-free
seconds with almost no paint load.

---

## 5. Lanes for the next arc, ranked

1. **WEDGEROOT (D-1, the disease).** Everything else is aftermath management that now
   works. New fingerprints to build on: three same-phase span-flush strikes, aims
   scattered across rows 519–801 (path-pinned, not address-pinned), retired-rate ramp
   38k→103k→147k blits/s across eras, freeze with `inflight=1` at T0. First patch:
   emit the unflown `[wedge1] BLITAIM` per-core sibling from the `[wcser]` steal path.
   Second: a bench repro recipe now exists without input — `storm` + `pulse`, walk away
   (steal 3 proved idle-with-paint wedges).
2. **PRTSCR-ARM (defects 1+2+3+4 together).** Make arrival a standing subscription:
   re-run the ST on every writable-volume arrival until PASS; revalidate the handle at
   write time; engage the Usb handle as rung 2 intended; survive a mid-write bus drop by
   retrying on the next mount. Bench: different stick/port to split device-flake from
   xHCI churn (four re-enumerations + 2 `COMMAND FAILED (Code 4)` this flight).
3. **BT-PATCHRAM discriminator flight.** The one clean experiment left: stage the `.hcd`
   on `/B43/` and re-run the inquiry ladder. The hypothesis to kill or crown is now
   sharp: *boot-scoped inquiry-substate deafness that spares page RX*. Secondary: rerun
   AVDTP_DISCOVER after pairing completes (boot-2's non-answer may be sequencing — the
   speaker answered everything up to the moment pairing state changed) and/or widen the
   1500 ms window.
4. **STEALTAIL/INPUTKICK, merged and re-aimed.** The post-steal era is now characterised
   by numbers a fix must move: EHCIDARK pinned at 117 ms, banded fraction 74 % on the
   busiest window, `[wcser] declined_pct` 76–77 %, amp 1.63x. Wire up `[rtwit]
   in2present` so cursor latency is measured, not inferred. The "1–2.5 s after input"
   trigger model is dead; drop it from the lane brief.
5. **R7→R8.** Proceed on the ladder's own `next=` (bring_up_blt_ring under the held wake,
   fix the DW0 client field). Carry three standing rules from the residuals: no gating on
   forcewake-ack decode, no gating on the GT battery, and settle the `reclaim=freed`
   unreachability (alternative invalidation witness, or document the bounded
   7-page/28 KiB per-boot leak and amend the falsifier).
6. **Ledger, not lanes:** DEADKBD5 persists (addr=5 kbd + addr=6 boot-mouse
   `xact-err-burn` halted at boot, both boots, L2223/L11119); `[clickroute] route …
   kernel=true desktop=false nofab=true -> FAIL` in the boot battery, deterministic both
   boots (31310/27692) — same standing as flight-3's `[dmgovlp] -> FAIL` (which also
   repeated, `adopt_stretch=0/4`, both boots); `[wc-x] move-vacate … -> FAIL` at boot,
   both boots; boot-2 `wc-w amp=6.69x` is an **idle artifact** — cumulative
   presented/requested with a static 4 687 024-px denominator and six full-panel presents
   dominating (the 5.70→6.69 step is exactly `full_presents` 5→6 after the 123.4 s input
   burst); boot-1's true end-state amp is 1.61–1.63x, *better* than flight-3's 2.08x
   (three dead cores vs five). TOPPORT flew unexercised — put `top` in the next flight's
   operator script.

---

## 6. Appendix — byte offsets for direct seeking

`grep -ab` offsets into the sealed 2 343 810-byte log. Boot 2 starts at L8872 /
**byte 1 407 331**.

| event | boot | [ms] | L | byte |
|---|---|---|---|---|
| inquiry summary responses=0 (deaf #3) | 1 | 13253 | 444 | 46 840 |
| blind-page BR/EDR link (attempt 1/2) | 1 | 13796 | 454 | 50 951 |
| SSP fails status=0x18 | 1 | 13868 | 466 | 53 801 |
| r7 verdict=r7-blit-verified | 1 | 15744 | 1565 | 150 945 |
| GATE STOLEN #1 c1→c6 (aim 0xc84030) | 1 | 72081 | 3719 | 410 928 |
| blit debt forgiven core=1 | 1 | 72091 | 3723 | 412 188 |
| shell re-minted #1 | 1 | 72295 | 3761 | 419 242 |
| GATE STOLEN #2 c2→c5 (aim 0x925c38) | 1 | 121747 | 4661 | 573 008 |
| shell re-minted #2 | 1 | 121774 | 4675 | 575 648 |
| create win=10 (pulse, typing proof) | 1 | 160393 | 5307 | 693 669 |
| drag-begin win=10 (the trail) | 1 | 164668 | 5450 | 716 596 |
| GATE STOLEN #3 c3→c6 (aim 0x81d2e0) | 1 | 290538 | 7274 | 1 079 419 |
| shell re-minted #3 | 1 | 290571 | 7288 | 1 082 361 |
| **boot 2 first line (timestamp reset)** | 2 | ? | 8872 | 1 407 331 |
| inquiry summary responses=1 (target heard) | 2 | 2871 | 9304 | 1 453 211 |
| BR/EDR link, phase-stepped attempt 2/2 | 2 | 9922 | 9330 | 1 463 747 |
| Simple Pairing Complete → PAIRED | 2 | 10251 | 9348 | 1 468 075 |
| L2CAP channel established (dcid 0x0480) | 2 | 10334 | 9359 | 1 471 715 |
| AVDTP_DISCOVER sent (goes unanswered) | 2 | 10339 | 9372 | 1 474 772 |
| r7 verdict=r7-blit-verified (repeat) | 2 | 12199 | 10467 | 1 570 608 |
| PRTSCR-ST writable volume arrived | 2 | 64125 | 12377 | 1 801 263 |
| PRTSCR write failed -EIO | 2 | 71996 | 12463 | 1 813 875 |
| PRTSCR-ST: FAIL (terminal) | 2 | 71996 | 12464 | 1 813 972 |
| FRGUARD SUBSTITUTION (first of four) | 2 | 72631 | 12636 | 1 825 465 |
| replug mount #1, no ST re-arm | 2 | 205512 | 14418 | 2 086 538 |
| replug mount #2 (Port 5), no ST re-arm | 2 | 253659 | 15002 | 2 172 184 |
| end of capture (power-off) | 2 | 364165 | 15910 | 2 343 810 (EOF) |

Extraction recipes: `grep -abn '<pat>' ttyUSB0.log`; boot split at L8872; deadman
gap map via `awk '/\[deadman\]/{…hid_ms reset detection…}'`; per-window tallies via
`awk` keyed on `win=`/`torn=`/`banded=` over `[wc-h] rollup`; aim decode =
`aim = row*16384 + 4*x` against the `[?ms]` WRITER line's pitch.
