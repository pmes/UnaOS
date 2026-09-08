# rmbp7-FLIGHT 3 — POSTMORTEM

**Capture** `~/unaos-bench/capture/rmbp7-flight3/ttyUSB0.log` — SEALED,
2 845 788 B, 17 438 lines, `[?ms]` → `[1144218ms]` (~19 min 04 s of uptime;
final `[deadman] up=1120`). Machine powered off at end of capture; no shutdown
marker — the log simply stops.
**Image** `hw-rmbp@9578cacd`, flight-1 knobs + `UNAOS_PRTSCRST=1`. 2880x1800
panel, Kepler takeover + window compositor confirmed live
(`[26032ms] [wc-x] desktop-clear panel=2880x1800`, L1334; menubar, crispy theme,
STAT.ELF desktop app armed at 26195 ms).
**Flight-1 baseline** `~/unaos-bench/scratch/rmbp7/postmortem/FLIGHT1-POSTMORTEM.md`
(read in full; comparisons below cite its sections).

All extraction done with `awk`/`grep -a`/`strings` (log carries control bytes).
Line references `L<n>` are `grep -n` line numbers in the sealed file; the
appendix carries byte offsets for direct seeking.

---

## 1. Flight summary and verdicts

The boot was healthy to desktop in ~26 s and the machine then ran for **19
minutes** under intermittent operator load. The flight's staged questions are
all answered. The dominant story of the tail is the same degradation staircase
as flight-1, but three more steps down it: **five render-gate steals, five
rehomes, five dead cores (c1–c5), and by end of capture the desktop was being
carried by c0/c6/c7 alone** (`[1141568ms] [wcpar] cores=3 … c0=77 c6=77 c7=5`).
Every post-boot collapse was triggered by operator input arriving into the
degraded compositor.

| # | Question the flight was staged to answer | Verdict |
|---|---|---|
| 1 | **PRTSCR-ST** (boot-time capture self-test) | **VETOED, never ran.** Exactly one PRTSCR line in the whole log: `[26414ms] :: PRTSCR-ST: program source is sdhc and vetoes writes (the internal SD reader is mounted READ-ONLY …) — still waiting for a writable volume ::` (L1852). It waits forever; no retry line, no capture, no FAIL/PASS. The self-test is unreachable on this bench medium as staged. |
| 2 | **Apple internal keyboard & HID usage 0x46** | **The keyboard does not emit 0x46 — confirmed at firmware level for the chords tried.** Zero `capture armed` lines in the sealed log (`grep -ac 'PRTSCR'` = 1 — the veto line only; the arm line would read `:: PRTSCR: PrintScreen (HID 0x46) down on EHCI -> capture armed ::`, ehci/mod.rs:16196). Decisive because the kbd endpoint was demonstrably delivering reports at exactly the time Peter tried the chords: `EHCIDARK addr=8 ep=IN3 kind=kbd` report totals 13→19→26→61→71→83→125 across 306 066–399 205 ms (L6181, L6349, L6827, L7041, L7311, L7632, L7882), 385 total by 956 218 ms. Reports flowed; the shared press-edge predicate never fired. Caveat in §5-Q4. |
| 3 | **Wedge behaviour ([wedge1] tripwire)** | **Fired once, three abandonments, and the drain was waiting on blits owed by cores that were already dead** — see §3 D-1. First fire 342 043 ms, `DRAIN ABANDONED` 363 075 ms, abandoned counter 1→2→3 at 363 088/504 041/529 073 ms. The abandonment closed the shell window (`[363075ms] [wc-a] close win=2`, L6821). |
| 4 | **BT / BTRX-ROOM decision rule** | **This boot's BR/EDR receiver was DEAF; across flights the receiver is INTERMITTENTLY deaf.** Inquiry 3 913–14 157 ms: `responses=0` with all three result shapes unmasked (L448–449); both blind page trains full-length timeouts (page summary 25 685 ms, L482). Peter confirms a phone sat on its Bluetooth settings screen the whole boot — per BTRX-ROOM's own pre-registered rule (L450: "NEITHER heard => the receiver is deaf"), the phone-not-heard control decides it. LE side healthy: connection to Megaboom at 2 416 ms. §4. |
| 5 | **Flight-1 Q5 (BT config re-read intermittent)** | **Did not recur.** The full-config walk succeeded this boot: complete `bt-l0` census of addr 7 (intf 0–3, all alts) at 1 792 ms, L358–366; zero `64-byte`/`wTotalLength` failure lines. Confirms flight-1's "PRE-EXISTING intermittent" verdict. |
| 6 | **D-lane incidentals** | D-1/D-2/D-5/D-6/D-7 all re-measured; two flight-1 conclusions are **refined** (valve DOES print reopens; tearing's presspread law only partially holds) — §3. |

Mid-flight facts verified against the sealed log — three corrections:

1. **`bracketq_met` froze at 63 110 ms, not "by 114 653 ms"** — last transition
   83→89 at `[63110ms]` (L3011), frozen at 89 through the final rollup
   (`[1143408ms]`, 5 964 presents later).
2. **Input did NOT die permanently at ~679 s.** The 679.9→789.1 s HID silence is
   one of three long idle gaps (569.9→671.4 s, 678.5→789.1 s, 799→936.8 s), each
   ending with real deliveries (`[deadman] hid>0`). Last HID activity:
   `[962018ms] [deadman] up=942 hid=2 … hid_ms=776` (L15749); silence for the
   final ~182 s.
3. **The WCD valve was not permanently stuck closed.** Longest closed stretch
   546 091→941 148 ms (395 s — the mid-flight `dwell_ms=242771` reading sits
   inside it), but it reopened 3 times mid-flight without a steal and cycles
   healthily after steal #5. §3 D-6.

---

## 2. Timeline

| [ms] | L | event |
|---|---|---|
| 1792 | 358 | bt-l0 full census addr 7 **succeeds** (flight-1's re-read failure did not recur) |
| 2416 | 412 | `LE Connection Complete status=0x00 handle=0x0040 role=MASTER peer=88:c6:26:cc:2d:3c` |
| 3913–14157 | 436–449 | classic inquiry, 10 240 ms, `responses=0`, all three result shapes unmasked |
| 14158–25685 | 452–482 | two blind page trains, both `status=0x04` PAGE TIMEOUT, full-length (`observed=5123/5124ms` of 5120) |
| 26032 | 1334 | desktop-clear; compositor live; win=1 (console, asid=0xffffff01) + win=2 (shell, asid=0xffffff02) created 26033/26416 |
| 26410 | 1834 | KBDWIT: addr=5 kbd endpoint HALTED (`xact-err-burn`), retired — dead for the boot |
| 26414 | 1852 | **PRTSCR-ST VETO** — SD reader read-only, "still waiting for a writable volume" (waits forever) |
| 41425–42267 | 2223– | window fleet; `[dmgovlp] verdict … -> FAIL` (41933, L2631); dock/badge churn |
| 63095/63110 | 3003/3011 | trackpad release; **bracketq_met freezes at 89** (flight-1: froze at 66 at 35.4 s) |
| 90412–90478 | 3346–3423 | load storm: 12 `[smpbal]` steals in 66 ms; six 288x288 vug windows created |
| 92279–95281 | 3489– | **STEAL 1**: c1 overdue `win=7 phase=33 at=span-flush row=652`; stolen by c3 at 95281; rehome c1→c2 (95285); valve reopens after 53 780 ms closed — at the steal |
| 96822–100082 | 3586– | **STEAL 2**: c2 overdue `win=8 phase=48 at=pw-content row=24`; stolen by c5 at 100082; rehome c2→c3 (100090) |
| 342043 | 6677 | **`[wedge1] DRAIN STALLED core=3 blit_active=2 pending=1`** — render core c3 spins in drain; `BLITWHO net=[0,1,1,0,0,0,0,0]` (363075, L6820): the two owed blits belong to **c1 and c2, dead for ~250 s** |
| 363075 | 6819–21 | `DRAIN ABANDONED … spins=1073741824` (abandoned=1); **`[wc-a] close win=2`** — the shell window closed by the abandonment |
| 374087 | 7192 | shell window re-minted: `create win=2 asid=0xffffff02` |
| 397533 | 7833 | valve reopens (97 ms) after 297 199 ms closed — **no steal nearby** |
| 479029–529073 | 9533–9923 | wedge spin episodes 2+3 on the render core under heavy input (`hid=119/s`, `in=6/469/4` GUI declines at 480975, L…): abandoned=2 (504041), abandoned=3 (529073) |
| 502909–529587 | 9718 | valve's one long spontaneous open (26.7 s) |
| 526249–529503 | 9906– | **STEAL 3**: c3 overdue `win=9 phase=33 at=span-flush row=577`; stolen by c5; rehome c3→c4 (529511); rh=3 (529977) |
| 546091 | 10419 | valve closes — stays closed 395 057 ms (the flight's longest stretch) |
| 673–679 s | | tearing resumes (win=10, win=12, win=6, win=3) after 100 s idle ends at 671.4 s |
| 789038/789060 | 13497 | trackpad release at EHCI + HID deliveries resume after 109 s idle… |
| 791481–794761 | 13629– | …and 2.4 s later **STEAL 4**: c4 overdue `win=11 phase=33 at=span-flush row=1050`; stolen by c5; rehome c4→c5 (794768); **rh=4 at 795080** — the fourth rehome's trigger is the input burst |
| 798449/798453 | 13897 | trackpad click → `[wc-fv] focus raise asid=0xffffff02` / `[vugmin] focus asid=0xffffff02` — focus lands on the **kernel shell-furniture window** (§3 D-7) |
| 790–940 s | | tear storm: win=2 21→336, win=3 8→75, win=10 3→45, win=6 2→39; `[comp2] util_pct=83-84` (L15xxx) |
| 936807–936889 | 15279 | Peter moves the mouse after 133 s idle (`hid=6` at 936889)… |
| 937898–941148 | 15309– | …compositor pass hangs immediately (`comp_ms=992→3992`, deadman L15318-); **STEAL 5**: c5 overdue `win=6 phase=41 at=pw-face row=432`; stolen by **c0** at 941148; rehome c5→c6 (941188); rh=5 (941939). **This is the "moved the mouse and the whole system stalled" event — a ~4.3 s full-desktop freeze.** |
| 941148 | 15347 | valve reopens after 395 057 ms closed — at the steal; thereafter cycles ~5 s open/closed to end |
| 961–962 s | 15749 | last HID deliveries (`up=942 hid=2`); operator stops interacting |
| 1144218 | 17438 | end of capture (`up=1120`, `wcpar cores=3`, presents still flowing at ~31/s, `amp=2.08x`, power off) |

Amplification (`[wc-w] rollup amp=`): 1.41x (67 s) → 1.81x (115–325 s) → 1.85 →
1.91 → 2.01–2.02 → 2.07–2.09x (821 s–end). Matches the mid-flight ~1.81–2.07x
observation; presents never stop (5 964 total).

---

## 3. D-lane findings, this flight vs flight-1

### D-1 — stall accounting (WEDGESRC)

* **One tripwire fire, three abandonments.** First fire
  `[342043ms] DRAIN STALLED core=3 blit_active=2 pending=1 spins=134217728`
  (L6677). spin_max evolution ep-1: 165 670 912 (342754) → 390 320 128 (347760)
  → 610 856 960 → 835 670 016 → 1 059 921 920 → bound 1 073 741 824;
  `DRAIN ABANDONED` at 363 075 (abandoned=1 at 363088). Episode 2: spin resumes
  479 029 (spin_max 74 784 768) → bound at 504 041, **abandoned=2**. Episode 3:
  509 063 → bound at 529 073, **abandoned=3** (no separate STALLED/ABANDONED
  banner for ep-2/3 — the tripwire latch prints once; only the counter moves).
* **The smoking gun:** `[363075ms] BLITWHO active=2 net=[0,1,1,0,0,0,0,0]
  last_enter=bg-user@c0 spin_core=3` (L6820). The drain on c3 (the render core
  since the 100 090 ms rehome) was waiting for blit-exits owed by **c1 and c2 —
  the cores killed by steals 1 and 2, 247 s and 242 s earlier.** A dead core's
  blit counter can never decrement; the 1 GiB spin bound was unwinnable by
  construction. This flight's wedge is therefore **not** flight-1's BAR1-store
  question (Q1/D-1): it is a bookkeeping wedge — steal/rehome does not clear the
  dead core's `blit_active` debt.
* Stall-site stamps this flight (all `PASS OVERDUE`/`GATE STOLEN`):
  `phase=33 at=span-flush row=652` (c1), `phase=48 at=pw-content row=24` (c2),
  `phase=33 at=span-flush row=577` (c3), `phase=33 at=span-flush row=1050` (c4),
  **`phase=41 at=pw-face row=432` (c5) — a NEW site**: 41 = PW_STAGED(40)+
  PW_FACE(1), the window-face fill (wm.rs:9388/9395/9440, gauge at 19104).
  Flight-1 saw only pw-content(48) row=150 and span-flush(33) row=822. Four of
  five stalls are span-flush (the BAR1 blit loop flight-1 convicted); the
  pw-face one says the stall family is not confined to the content path.

### D-2 — rehome behaviour (REHOMESPIN)

* **Five steals / five rehomes** (flight-1: two). Chain: c1→c2 (95285),
  c2→c3 (100090), c3→c4 (529511), c4→c5 (794768), c5→c6 (941188); `[deadman]`
  rh transitions 1/2/3/4/5 at 96195/100257/529977/795080/941939 (L3574, L3661,
  L10006, L13730, L15383). **The fourth rehome (between 780 s and 876 s) was
  triggered at 795 080 ms by steal 4**, which itself followed the 789 s input
  burst.
* The free-run persists: `[comp2] rate=152-175 passes/s` through the mid-flight
  (150218/280273/396530), `[noatt] rate=310-471/s taker=0` (UNATTRIBUTED) for
  most of the flight — flight-1's D-2 spin, unchanged. During wedge ep-2/3 the
  spinning render core produced `passes=841` of empty work
  (`[519065ms] [comp2] … pass_us=2 util_pct=0`).
* **New this flight — the trigger pattern:** every post-boot steal fired within
  1–2.5 s of operator input arriving after an idle gap (789.0 s input → 791.5 s
  overdue; 936.8 s input → 937.9 s overdue). The degraded compositor survives
  idle indefinitely and wedges on the first real paint burst.
* Steal 5 broke the monotone chain: acquirer was **c0** (pool-chosen), not c6;
  the rehome still went to c6. End state: 5 of 8 cores dead to the desktop
  (`wcpar c1..c5=0` from 941 s; already `c1=c2=c3=0` by 534614, matching the
  mid-flight "cores 5 alive" note, and `c4=0` after 794761).

### D-5 — bracketq death (BRACKETDEAD)

* Freeze moment: ramp 0→4→20→45→83→**89** between 59 100 and 63 110 ms
  (L2917–3011), then **frozen at 89 for the remaining 1 080 s** and ~5 460
  presents (`[1143408ms] … bracketq_met=89 bracketq_busy=0`). `bracketq_busy=0`
  throughout, `full_presents` keeps advancing 6→29 — same signature as
  flight-1's freeze at 66 @ 35.4 s. The freeze is again adjacent to the end of
  the first big interaction burst (trackpad release 63 095 ms, L3003), which
  supports flight-1's D-5 "stops being reached after the early desktop churn"
  reading. Confirmed: the bracket instrument reports nothing for 94 % of this
  boot. (Shared-root candidate with D-7's `[wc-k]` silence stands: zero
  `[wc-k] erase` lines appear after the boot phase this flight as well.)

### D-6 — valve (VALVELEVEL)

* Full open/close history (all `[wc-d]` OPEN/CLOSED lines, L1405–17064):
  closed 26232 (boot storm, util~98%) → open 29098; closed 41500 → **open 95281
  (after 53 780 ms, at STEAL 1)** → closed 95506; open 100082 (STEAL 2) →
  closed 100334; **open 397533 (after 297 199 ms — no steal)** → closed 397630;
  open 502909 (26.7 s, the longest mid-flight open) → closed 529587; open
  545752 → closed 546091; **open 941148 (after 395 057 ms, at STEAL 5)** →
  then ~30 open/close cycles of 2–7.5 s to end of log, final state **open**
  (`[1141643ms] valve READ util~3% wcd~0% state=open dwell_ms=0/2000`).
* **Flight-1's D-6 statement "reopen 8 %, never printed" is corrected:** the
  reopen DOES print (`valve OPEN resumed suspended=N after Xms closed`) and
  occurred 9+ times. What flight-1 observed was the level-latch: with `[comp2]`
  util pinned 15–35 % by the D-2 free-run, the sub-close-threshold dip needed to
  reopen almost never arrives — hence the 297 s and 395 s closed stretches
  (mid-flight reading `dwell_ms=242771/2000` at 783785, util 13–17 % vs
  `close_at=util12%`, sits inside the second). The three steal-coincident
  reopens are the compositor's pass rate collapsing to zero during the stolen
  gate. D-6's understanding (level gate + D-2 load = latched closed) is
  **confirmed**, its "never reopens" observation was flight-1 sample size.

### D-7 — corpse windows / shell furniture (CORPSEGLASS)

* Every steal prints the corpse policy explicitly:
  `REHOMED … c<n> and its in-flight window stay lost` and
  `SCHED-X86: render task is a REHOMED instance on core <m> … the previous
  instance's shell window is a corpse and is not re-minted` (L3544, L3650,
  L9954, L13681, L15356). Five stolen in-flight windows lost this flight
  (win=7, 8, 9, 11, 6 — the win named in each PASS OVERDUE). Late `[wcn]`
  rollups list win 11 absent from live sets after 794 s — "some windows gone"
  is this policy, working as printed.
* **`0xffffff02` is not a corpse sentinel.** It is
  `wm::KERNEL_OWNER_DESKTOP = KERNEL_OWNER_BASE(0xFFFF_FF00) + 2`
  (wm.rs:1020/1030) — the kernel-owner band's desktop-furniture ASID; win=2
  `surf=1440x883` is the kernel shell window created at boot (26416 ms,
  L1855). 0xffffff01 is KERNEL_OWNER_CONSOLE (win=1). The 798 453 ms
  `focus asid=0xffffff02` is a real trackpad click (798449, L13895) landing on
  that furniture and raising it (`wc-fv focus raise … top_win=2 z=232`).
  Kernel-band rows are deliberately "unreachable as a user focus target"
  (wm.rs:1017-1019 doc) — **focus parked on kernel furniture is why "input to
  the shell was impossible"**: keys went to a window that consumes nothing.
* The shell window WAS closed and re-minted once — but by the wedge, not the
  steal: `[363075ms] close win=2` (the DRAIN ABANDONED handler) and
  `[374087ms] create win=2 asid=0xffffff02` (L7192). Peter's
  "shell closed+reopened" observation is this pair.
* Tearing/presspread (flight-1 Q3 law test) — **the law survives only in
  weakened form.** Final per-window tallies (last `[wc-h] rollup` per window):

  | win | torn | presspread | presspop | note |
  |---|---|---|---|---|
  | 2 (shell 0xffffff02) | **336** | 671 | 16094 | presspread grew 19→252→537→671 as tears climbed |
  | 3 | **75** | 158 | 13733 | |
  | 10 | 45 | **13** | 10693 | counterexample: low spread, heavy tears |
  | 6 | 39 | **14** | 14819 | counterexample |
  | 5 | 5 | 232 | 8264 | counterexample the other way |
  | 12/9/11/4/7/8 | 5/4/1/0/0/0 | 16/62/61/5/1/5 | | |
  | 1 (console) | **0** | **5542** | 1364 | strongest counterexample |

  The two worst tearers do carry the two highest presspreads among
  actively-presenting windows (matches flight-1 §D), but win=10/6 tear heavily
  at presspread≈13 and win=1 never tears at presspread=5542. The stronger
  predictor this flight is **era**: ~90 % of all tear increments land in
  789–940 s — the four-dead-core, util 83–84 % regime (win=2 alone 21→336
  there). Restated law: presspread ranks tearers *within* a load era; the era
  itself (post-steal degraded parallelism) is the first-order cause. Flight-1's
  "banding is not the discriminator" holds (win=2: banded=1125 whole=9211 vs
  win=10: banded=240 whole=10453 — both tear).

---

## 4. Bluetooth

* **LE: healthy, first-window.** Passive scan enabled 1810 ms; Megaboom heard
  at 2310 ms (`ADV_IND rssi=-96dBm reports=2`, L402); address filter armed per
  Peter's Q14 ruling;
  `[2416ms] LE Connection Complete status=0x00 handle=0x0040 role=MASTER
  peer=88:c6:26:cc:2d:3c interval=48750us` (L412). ACL OUT accepted (15/15
  bytes, L415); no ACL IN within the 600 ms window (L416) — receive direction
  still unproven; disconnect needed a second lever (L427) — both known shapes.
* **BR/EDR: deaf this boot, by the flight's own pre-registered rule.** Inquiry
  with mode 0x01 (RSSI) latched and event-mask bits 1/33/46 all open
  (L431-433): `responses=0 … THE INQUIRY HEARD NOTHING AT ALL` (L449).
  BTRX-ROOM (L450) named the deciding control before the data existed: a phone
  held on its Bluetooth settings screen for the window. Peter ran that control
  this boot — the phone was inquiry-scanning the entire time and was not heard.
  Per the rule's own branch, **the receiver is deaf and BTRX-PATCHRAM is the
  surviving hypothesis.** Both blind pages full-length timed out
  (`observed=5123/5124ms` of 5120, status=0x04, L464-465/480-481; phase-step
  retry confirmed on the wire, L473) — consistent with, and subordinate to, the
  deaf-receiver verdict.
* **Cross-flight verdict: INTERMITTENTLY deaf.** Flight-1's doubled inquiry
  produced a response and the first BR/EDR link; this boot, same silicon, same
  image lineage, zero responses with a known-present inquiry-scanning device in
  the room.
* Surviving hypotheses, ranked: (1) **unpatched-ROM receiver defect
  (BTRX-PATCHRAM, L435)** — no vendor .hcd is ever uploaded (clean-room policy;
  zero OGF 0x3F commands), and an unpatched BCM20702 ROM with a marginal
  BR/EDR RX path is exactly the intermittent this pair of flights measures;
  (2) marginal RF/antenna path that some boots power up on the wrong side of —
  distinguishable from (1) only by a patched control, i.e. a user-staged .hcd
  via the WIFI-1 path; (3) host event plumbing — **excluded** this boot (all
  three result shapes unmasked and read to termination, L449).
* **Flight-1 Q5 config re-read: did not recur** (full census at 1792 ms,
  L358-366). PATCHRAM witness present at L435. BTRX-MODE/TXPWR sanity reads
  clean (mode 0x01 latched, tx_power=0 dBm in range, L433-434).

---

## 5. New questions this flight raises (ranked)

1. **Q1 — DEBTCLEAR: steal/rehome must retire the dead core's blit debt.** The
   363 s wedge spun 1 GiB of spins waiting on `net=[c1,c2]` blit-exits owned by
   cores dead since 95/100 s (§3 D-1). Either WCSER-REHOME clears the dead
   core's `blit_active`/net entries, or the drain must exclude dead cores from
   its predicate. This is now the proven mechanism of a full render-core loss
   (c3 spent 342–529 s wedged, then was itself stolen from while in span-flush)
   — the staircase's amplifying feedback: each steal plants the debt that
   wedges the next holder. Files: video/wm.rs (drain predicate + rehome).
2. **Q2 — INPUTKICK: why does the first input burst after idle wedge the
   holder within ~1–2.5 s?** Three-for-three this flight (789 s→steal 4,
   936.8 s→steal 5, and the 474–529 s interaction era→steal 3, with
   `in=6/469/4` GUI declines mid-wedge). The idle desktop never wedges. Suspect
   the input-driven damage burst (cursor + focus repaint) meeting the D-2
   free-running compositor. Reproducible on the bench: let it degrade to rh≥2,
   idle 2 min, move the mouse.
3. **Q3 — FURNITUREFOCUS: a click can park focus on kernel-band furniture
   (asid 0xffffff02) and strand the keyboard.** 798 453 ms, §3 D-7. The kernel
   band is documented "unreachable as a user focus target" (wm.rs:1017) but
   `wc-fv`/`vugmin` raised and focused it from a hit-test. One-line predicate
   check (`is_kernel_owner`) at the focus-raise seam. Explains "input to shell
   impossible" without any input-path defect.
4. **Q4 — DEADKBD5: what is the addr=5 kbd endpoint that died at boot?**
   `KBDWIT addr=5 ep=IN1 kind=kbd SILENCE-CUT-BY-HALT class=xact-err-burn …
   halted=1` retired at 26 410 ms (L1834-35), `dead=1` (L1939). The 0x46
   verdict in §1 rests on the addr=8 IN3 endpoint being *the* internal
   keyboard; if addr=5 is a second keyboard-class interface of the same
   hardware (boot-protocol path), the Fn-layer question re-opens on it. Next
   flight: identify addr=5 (VID/PID in the census) and, if keyboard, un-halt
   and watch it during one chord.
5. **Q5 — PRTSCRWRIT: give PRTSCR-ST a writable target on this bench.** The
   veto (L1852) makes the knob a no-op on the rMBP as staged (SD reader
   read-only; only the SDHC-4c flight-recorder extent is writable and no file
   verb can name it). Either stage a writable USB FAT volume next flight or
   teach the selftest to write the flight-recorder extent.
6. **Q6 — STEALTAIL: the post-steal-5 regime is new and stable — characterise
   it.** After 941 s: valve cycles ~5 s (util hovering exactly at the 12 %
   close threshold), `noatt` 450+/s with `taker=0`, three cores carrying a
   flowing desktop at amp 2.08x for 200 s. The machine's degraded-but-alive
   floor; worth one instrumented flight before any D-2 fix, as the fix's
   baseline.
7. Incidentals for the ledger, not lanes: `[dmgovlp] verdict … -> FAIL`
   (41933, L2631) and `[wc-x] move-vacate … -> FAIL` (26237, L1411) both fired
   this boot; `[wc-k] erase` silent post-boot again (D-5/D-7 shared-root
   candidate); steal-5's acquirer came from the pool as c0 while rehome went
   c6 (pick policy vs rehome policy divergence, benign but worth a comment).

---

## 6. Appendix — offsets for direct seeking

Byte offsets (`grep -ab`) into the sealed 2 845 788-byte log; `L` = line number.

| event | [ms] | L | byte offset |
|---|---|---|---|
| bt-l0 census addr 7 (re-read OK) | 1792 | 358 | — |
| LE Connection Complete | 2416 | 412 | — |
| inquiry summary responses=0 | 14157 | 449 | — |
| bt-c1 page summary (both trains dead) | 25685 | 482 | — |
| PRTSCR-ST veto | 26414 | 1852 | 167 640 |
| shell win=2 create (asid=0xffffff02) | 26416 | 1855 | — |
| bracketq_met freeze at 89 | 63110 | 3011 | 322 465 |
| first PASS OVERDUE (c1, span-flush row=652) | 92279 | 3489 | 388 910 |
| GATE STOLEN #1 c1→c3 | 95281 | 3536 | 393 868 |
| REHOME #1 c1→c2 | 95285 | 3541 | 394 906 |
| GATE STOLEN #2 c2→c5 / REHOME c2→c3 | 100082/100090 | 3642/3647 | 407 601 / 408 635 |
| wedge1 DRAIN STALLED core=3 | 342043 | 6677 | 1 028 314 |
| wedge1 DRAIN ABANDONED + BLITWHO + close win=2 | 363075 | 6819–21 | 1 048 302 |
| shell win=2 re-minted | 374087 | 7192 | 1 093 399 |
| valve reopen, no steal (after 297 s) | 397533 | 7833 | — |
| abandoned=2 / abandoned=3 | 504041/529073 | 9740/9923 | — |
| GATE STOLEN #3 c3→c5 / REHOME c3→c4 | 529503/529511 | 9947/9951 | 1 537 351 / 1 538 273 |
| valve close → 395 s stretch begins | 546091 | 10419 | — |
| trackpad release + HID resume | 789038/789060 | 13497/— | — |
| GATE STOLEN #4 c4→c5 / REHOME c4→c5 | 794761/794768 | 13668/13673 | 2 194 252 / 2 195 584 |
| shelldesk on core 5 (corpse notice) | 794820 | 13681 | 2 198 254 |
| deadman rh=4 | 795080 | 13730 | — |
| click → focus asid=0xffffff02 | 798449/798453 | 13895/13897 | — |
| mouse move after 133 s idle | 936889 | 15282 | — |
| PASS OVERDUE c5 (pw-face row=432) | 937898 | 15309 | — |
| GATE STOLEN #5 c5→c0 / REHOME c5→c6 | 941148/941188 | 15345/15354 | 2 506 791 / 2 508 975 |
| deadman rh=5 | 941939 | 15383 | — |
| last HID activity (hid=2) | 962018 | 15749 | — |
| end of capture | 1144218 | 17438 | 2 845 788 (EOF) |

Extraction recipes used (control-byte-safe): `grep -a -n '<pat>' ttyUSB0.log`;
deadman rh/hid transitions via
`awk '/\[deadman\]/{match($0,/rh=[0-9]+/);…}'`; per-window tear transitions via
`awk '/\[wc-h\] rollup/{…}'` keyed on `win=`/`torn=`.
