# FLIGHT-RESULT — render8 (orin 17, 2026-09-06T15:42–15:57Z)

Image `render8-20260906T1532Z-c24d951` (hw-jetson `c24d9517`; branch tip at flight time `963516c8`, which differs
only by the orin 16 close report), kernel.elf sha256 `4297df1f07f86dbd…`, **ELF max_vaddr `0x3395c8`** — matched on
the wire by the loader anchor `KELF min=0x0 max=0x3395c8 pg=826` (excerpt :904), so the image that booted is the
image that was staged. Sixteen knobs:

```
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1
UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 UNAOS_TCURX=1 UNAOS_BSPTICK=1 UNAOS_BSPRUN=1
UNAOS_NET4=1 UNAOS_NET5=1 UNAOS_SDMMCROOT=1 UNAOS_GA10B_PROBE3=2
```

effective features `witness,ehcihid,holocron,tegra,bsptick,bsprun,ga10bprobe3,ga10bprobe3b,orinclick,tegra_el0,`
`tegrasmp,orinrender,desktop_firmware,orinrx,tcuprobe,tcurx,deskcascade,net4,pcie3,pcie2,net5,sdmmcroot,sdmmc`.

One boot, one power cycle — up at ~15:42Z, Peter powered the board off at 15:57:37Z, ~15 minutes on the glass.
Port `/dev/ttyACM1`, butler pid 756188 (unchanged from render7). Scored per `FLIGHT.md` §C with
[`scorers-render8.sh`](scorers-render8.sh) (committed beside this file; exit 0), verdicts in
[`render8-scores.txt`](render8-scores.txt); excerpt [`render8-boot1.log`](render8-boot1.log) (18767 lines,
sha256 `323dfb4a71dd90b5…`); marks in [`render8-marks.txt`](render8-marks.txt); card checksums in
[`render8-card-harvest.sha256`](render8-card-harvest.sha256); the glass in
[`render8-SCREEN3-small.png`](render8-SCREEN3-small.png) (a 25% reduction of the full-resolution capture).

**Every excerpt line number in this report refers to the committed `render8-boot1.log`.**

## Pre-flight (§A)
- **Card**: 10/10 files sha-matched against the staged `MANIFEST` at 15:39Z; **no `SCREEN*.PNG` present** (the
  card was tidied per FLIGHT.md §A.4, so this flight's captures name themselves `SCREEN0`…); unmounted before
  power-on.
- **Wire**: `/dev/ttyACM1` held by butler pid 756188 (`lsof`), capture armed before power-on.
- **Mark**: `MARK render8 c24d951 pre-boot card=render8-20260906T1532Z-c24d951 elfmax=0x3395c8 kernel=4297df1f
  butler=756188 port=/dev/ttyACM1 seat=orin17 at 2026-09-06T15:40:39Z raw=145097 orin=87996`.

## Pin and purity (§C.1–C.2)
- Boot slice = `orin.log` lines 87997–106763 (18767 lines). Burst injection at `orin=91229..91468`
  (15:44:17Z, slice-relative 3233–3472); paced at `orin=92082..92198` (15:44:57Z, slice-relative 4086–4202);
  power-off mark at `orin=106763` (15:57:37Z).
- **Anchor present in-slice**: `KELF min=0x0 max=0x3395c8 pg=826` at :904 — unlike render7, this boot's loader
  window is inside the `orin.log` slice itself, so no `unknown.log` splice was needed.
- Purity: `pi_marks=0` over the excerpt (no `RPI`/`BCM2711`/`raspi` token); 2084 lines carry a tegra/orin token;
  zero `=== AARCH64 EXCEPTION`. **PURE.**

## Flight sequence — as flown, with the deviations named
Peter drove the glass himself and did not run the staged sequence in order. What that costs is recorded here so
no row is read as a clean negative when the stimulus never happened.

| step | flown? | note |
|---|---|---|
| 1–5 (boot, SMP, GA10B rungs, mounts) | yes | unattended; A15/A24/A28 all scored |
| 6 burst + paced `tste\r` | yes | 15:44:17Z / 15:44:57Z, injector marks in `render8-marks.txt` |
| 7 window drags | yes | 4 gestures, A27 PASS |
| 8 quarry open/close, app menus | yes | out of order — quarry Quit was picked BEFORE the pulse Quit legs |
| 9 pulse close-box + Quit | yes | out of order, and repeated (Peter: "i have to hit x twice") |
| 10 Esc on a bar menu | yes | once, :5775 |
| 10b DRAGREL-A64 (Tab mid-drag, no release) | **NO** | never exercised; `DRAGREL-A64: NOT-EXERCISED` on the wire |
| 11 Tab focus cycle | yes | 7 presses; see the RULING section — the leg is now RETIRED, not failed |
| 12 Print Screen ×4, then ×3 fast | yes | both bursts flown |
| — Quarry close before the shell legs | **NO** | **Quarry was open across the whole Tab burst and both shell-pin bursts** (last open :6965, no close after), so SO9's key theft was live for every one of them |
| 13 crystal **Restart** | **NO** | Peter powered the board off at the wall instead — **A34 NOT EXERCISED** |
| — crystal Shut Down | **NO** | same |

Consequences carried into the rows below: **A34 is unflown, not failed** (`restart_announce=0 shutdown_announce=0`);
DRAGREL-A64 is unflown; and every shell/Tab reading was taken with Quarry on the glass.

## Scores
Every verdict line `scorers-render8.sh` printed, verbatim, with the source that mints its token.

| q | scorer line (verbatim) | token source | verdict | ledger |
|---|---|---|---|---|
| A15 | `A15 smp: cpu_on_success=5 cpu_on_error=0 el3_abort=0 poweroff=0 online_line=1 -> PASS (A15 pass 5)` | `ORIN-SMP-3 CPU_ON AP n -> SUCCESS` | **PASS — pass 6 of the APTEXT layout** (the scorer's literal "pass 5" is its own hard-coded label carried over from render7's script; the count of passes is now six: render3b, render4, render6 ×2, render7, render8). 0 deaths since APTEXT | A15 |
| A1 | `A1 u7stk: arming=1 post_cascade=1 len=32768 hw=14744 (render6=15552 delta=-808) headroom=18024 -> PASS-SHALLOWER (808 bytes below render6 — check the quarry actually opened)` | `arch/aarch64/sched.rs:10650` | PASS, unsaturated. The scorer's caution is DISCHARGED — the quarry opened (A8). Five samples: 15472 / 15584 / 15552 / **14400** (render7) / **14744** (render8). The +344 B against render7 is the round's folds inside `activate()`; headroom 18024 of 32768 | A1 |
| A18 | `A18 cascade: cascaded=1 refuse=0 pulsewin_open=8 pulsewin_decline=0 strip_kept=0 census_strip=retired census_pulsewin=3 -> PASS` | `[deskcascade]` / `[orinrender] census` | PASS, fifth pass. `pulsewin_open=8` is ONE window opened eight times — see A30, which is where the eight come from | A18 |
| A19 (wire) | `A19 band: band_cleared=1 shell_present=1 jd2_probe=1 -> WIRE PASS (now A19-pngband.py SCREEN0.PNG must read non-bg=0/60200)` (`[realdesk] band-cleared x=0 y=34 w=1920 h=1166 bg=2d2b55 shell=win=4 surf=960x466 box=970x510 at (515,402)` :2002; `[realdesk] shell-present win=4 outcome=Composited` :2016) | `[realdesk]` | WIRE PASS, third pass | A19 |
| **A19 (pixels)** | `A19-pngband.py render8-card-harvest/SCREEN0.PNG` → `band x0-700 y34-120: non-bg=0/60200 (0.0%)` … `A19 scorer verdict (band must be 0 non-bg): PASS` | `docs/dev/evidence/orin14/A19-pngband.py` | **PIXEL PASS — the leg render7 left owed is now read.** The control bands prove the check is not vacuous: `ctrl-right x700-1400 y34-120: non-bg=19299/60200 (32.1%)`, rows y 61..119, x 923..1399 — that is the **shell window's chrome and its three text lines** (verified by decoding the region), i.e. the scorer sees real content 223 px to the right of a band it reads as perfectly clean. `ctrl-below x0-300 y120-220: non-bg=0/30000`. Bar row informational: `bar x0-700 y0-34: non-bg=23800/23800` | A19 |
| A20 (routing) | `A20 clicks: arm_click1=1 orinclick_armed=1 clickroute_press=10 consumed=20 routing_census=0 -> PASS (A20 flown)` (`[clickroute] press chrome win=2 owner=4294967043 at (1053,110) -> drag`) | `[orinclick]`/`[clickroute]` | PASS — 10 presses routed, 20 consumed | A20 |
| A20 (pointer) | `A20 ptrpoll: lines=25 first_rearm=3 first_dup=0 first_nobuf=0 \| final rearm=5178 dup=0 nobuf=0 reports=5178 \| verdicts STREAMING=24 BASELINE=1 GUARD-REARM=0 DUP-DROP=0 NOBUF-DROP=0 ARMED-NO-COMPLETION=0 -> RE-ARMED (rearm=3 > 2 at census 1 — the pipeline is moving; a dead click is a ROUTING fault)` | `arch/aarch64/display_tegra.rs:5389…`, `drivers/xhci/mod.rs:2385` | **Instrument aboard, UNTRIGGERED again** — the pointer was alive all boot (5178 re-arms, 5175 decoded, `ARMED-NO-COMPLETION=0`). Second consecutive fault-free boot; the CLICKDEAD instrument is still unvalidated, exactly as rmbp 12 worded it for render7. NOT a pass for the instrument | A20 |
| A22 | `A22 tcu: arm=1 stop=0 census=857 full_final=0 nbytes=0 full_edges=3 changes=7 data=[00 00 00] -> ROW2: FULL-SEEN then consumed` | `arch/aarch64/hsp_tegra.rs:310` | **ROW 2 held**, three full-edges and seven changes, mailbox empty at the end | A22 |
| liveness | `HEALTH: heartbeat=1 el1=1 arm=1 armed=1 live=853 redzone=0 exceptions=0 -> PASS` | — | PASS | — |
| A16 | `A16 tcurx2: tcurx_took=8 took_total=8 serialrx_census=842 rx_final=8 mbox_final=8 ovrf=0 tcu_full_final=0 keys=41 -> PASS rung 2 (the consumer took 8 byte(s) and left the mailbox empty)` | `hsp_tegra.rs:391`, `serial.rs:830` | PASS rung 2, second pass | A16 |
| A16 burst | `A16 leg burst: keys=3 tcurx_took=3 rx_after=3 mbox_after=3 ovrf_after=0 -> PARTIAL 3/5` · keys in window `KEY 't' :: KEY 's' :: KEY 't' ::` | `main.rs:2943/2945` | **PARTIAL 3/5 — and this is A37's fix showing its cost, not a regression.** Under `policy=mbox-only` the UARTC direct read is parked, so the burst is delivered ONLY through the mailbox; the mailbox word holds three bytes and `full-edges=1` over the burst window, so `e` and CR were overwritten before the drain ran. render7 delivered 5/5 by using BOTH readers — and paid for it with reordering (A37). The remedy is mailbox depth/backpressure, not un-parking the second reader | A16, A37 |
| A16 paced | `A16 leg paced: keys=5 tcurx_took=5 rx_after=8 mbox_after=8 ovrf_after=0 -> PASS 5/5` · `KEY 't' :: KEY 's' :: KEY 't' :: KEY 'e' :: KEY 0x0d ::` | `main.rs:2943/2945` | **PASS 5/5 IN ORDER** — render7's paced leg delivered six keys for five bytes (the CR twice). A37's single-source policy fixed it | A16, A37 |
| A37 | `A37 rxmerge: arm=1 census=842 policy=mbox-only dup=0 reorder=0 parked=164572494 per_byte=8 \| side-effects [serialrx] polls=0 ovrf=0 -> PASS A37 SINGLE-SOURCE (exactly-once, in-order; parked=164572494 is the drain-liveness counter — polls=0 beside it is CORRECT, not a dead drain)` (`[rxmerge] policy=mbox-only armed=1 uartc-rbr=parked`) | `arch/aarch64/serial.rs:1065/1145` | **PASS — A37 fixed and flown.** `dup=0 reorder=0`, `seq=8 uartc=0 mbox=8`; both declared side effects present and correctly scored | A37 |
| A27 | `A27 drag: wired=1 control_absent=0 arm=4 end=4 steered=4 fed-no-move=0 no-feed=0 \| wm_drag_begin=7 wm_drag_end=7 placed=4 no-move=3 -> PASS A27 (steered 4 gesture(s), wm placed 4)` (`[dragroute] end win=2 via=release fed=41 applied=24 at (1233,99) -> STEERED`; `[wm-act] drag-end win=2 owner=0xffffff03 at (247,117) -> placed`) | `display_tegra.rs:5197/5246`, `wm.rs:16214/16216` | PASS, second pass — 4 of 4 steered gestures placed | A27 |
| A8 | `A8 quarry: quarry_open=3 skip=0 decline=0 pidesk_true=1 deskquarry_seat=1 compiled=1 open=1 relatched=0 -> PASS A8 (compiled=1 open=1, the window is on the wire; now count 4 dock tiles on the glass)` (`[quarry] open win=2 surf=1152x720 ts=2 box=1162x764 at (379,203) volumes=1 tree-rows=8 list-rows=24 cwd=/usb`) | `video/quarry/live.rs:1806`, `desktop_firmware.rs:396` | **PASS — the render7 defect is GONE.** Three opens, each listing real files: `[quarry] open census cwd=/usb entries=21 dirs=4 files=17` at :1811, then `entries=24 dirs=4 files=20` at :5367 and :6968 once the captures existed. `[quarry] open volumes mounts=["/", "/fat", "/usb"] roots=["/"]`. Four dock tiles on the glass. No `unafs-mount` error anywhere | A8, A28 |
| A28 | `A28 rootfs: mount=1 ok=1 bound=1 entries=4 refused=0 unafs_mount_errors=0 fat_enodev=0 -> PASS A28 (/ bound to the card's FAT through TegraSd, 4 entries, and NO unafs-mount error)` (`[sdmmc] root mount source=tegra-sd card_blocks=62333952 -> OK label="UNAOS-PI" vol_id=0xabfbdefa clusters=110874 cluster_bytes=512 probe_us=89111` :1430; `[sdmmc] root bound / and /fat = tegra-sd FAT read-only … entries=4 dirs=1 files=3` :1782) | `arch/aarch64/sdmmc_tegra.rs:3373` | **PASS — the VFS root has a backend for the first time.** Datum, not a defect: the FAT **boot-sector** label reads `UNAOS-PI` while `lsblk` reports `UNAOS-ORIN` — the two fields are the boot-sector volume label and the root-directory volume-label entry, and only one of them was rewritten when the card was relabelled. Worth a one-line fix in the card tooling; nothing on this board reads it | A28 |
| A21 tick | `A21 tick: arm=1 ticklines=719 tmax=179500 census=853 passes=1->164613074 exceptions=0 -> PASS` | `arch/aarch64/timer` / `[orinbsptick]` | **PASS on the full desktop.** 179 500 ticks (718 s at 250 Hz) with the compositor, quarry, pulse, net and GA10B rungs all live — the tick1 probe boot proved 33 000 on a bare image; this proves the tick survives the whole desktop | A21 |
| A21 run | `A21 run: host=1 hosting=1 refusing=0 bgrun=0 detached=0 rejected=0 el0_first_run=1 capped=0 orinbsprun_join=1 -> HOSTING, NO SPAWN (no BGRUN line — nothing was launched to host)` (`:: [bsprun] host core=0 el=1 -> HOSTING (online=0x3f el1cores=0x1…)` :1830; `:: [bsprun] el0 first-run 'el0-hello' on core 0 — CurrentEL EL1 checked, eret to EL0 ACCEPTED (n=1) ::` :1834; `[orinbsprun] boot core 0 joins run() — SCHED_ACTIVE=true, mark_online(0)` :1826) | `arch/aarch64/sched.rs:10718/10748` | **PASS on both halves that were exercisable.** The A2 predicate flipped: `mark_online(0)` ran, `el0_placement_possible` is satisfied, the terminus HOSTS rather than REFUSES, and an EL0 image was actually dispatched (`eret ACCEPTED n=1`). `bgrun=0` is **NOT a failure**: nothing was ever launched, because the shell was never usable this boot (SO9 + SO10). The `bg` half of A2 stays unflown | A21, A2 |
| A12 net5 Q0 | `A12 net5 Q0: net5R_lines=1 armed=1 not_armed=0 match=1 mismatch=0 (net4F rx-ring seen=1) -> ARMED + MATCH — live; read Q2` | `rtl8168_tegra.rs:3525` | PASS — the shadow re-point reached DRAM, so the landing arm is readable | A12 |
| A12 net5 Q1 | `A12 net5 Q1: rx_ring_lines=1 below4g=1 net4s=15 identity_covered=5 alias_region1=0 -> PASS placement (sub-4GiB, identity-covered, no alias in the path)` | `rtl8168_tegra.rs` / `[net4F]` | PASS placement | A12 |
| A12 net5 Q2 | `A12 net5 Q2: net5T=5 net5V=1 pops_scored=5 \| REFETCH-LIVE=0 REFETCH-WRONGSLOT=3 STALE-ORIG=0 NOWHERE=1 PREFETCHED=1 -> C2' PASS (defect located): REFETCH-WRONGSLOT — the address path is live; the reuse is an index defect.` | `rtl8168_tegra.rs:3646` | **THE ANSWER THE ARC WAS BUILT TO GET.** `STALE-ORIG=0` acquits the NIC-errata branch outright; the NIC re-fetches the descriptor address after the re-point (address path live) but writes into the WRONG SLOT — a post-enable **index** defect in our own ring bookkeeping, three of five pops. That is a lane we own, not silicon | A12 |
| A12 net5 Q3 | `A12 net5 Q3: dhcp_lines=1 lease=0 -> NO LEASE (meaningful only on a REFETCH-LIVE Q2)` (`[net4V no-lease verdict] dhcp-tx: discover=1 request=0 \| dhcp-rx: offer=0 ack=0 nak=0 \| ring: popped=5/32 wraps=0 zero-payload=4 [ring died MID-PASS …]`) | `[net4V]` | **NO LEASE, correctly UNSCORED as a lease question** — Q2 is not `REFETCH-LIVE`, so the lease result carries no information about DHCP; it is downstream of the index defect | A12 |
| A12 net4 control | `A12 net4 control: net4G_lines=1 -> UNEXPECTED — [net4G] armed under the NET-5 cadence; read why` | `[net4G]` | **READ, AND IT IS NOT A DEFECT — the scorer's pattern is too loose.** The one `[net4G]` line is `[net4G] latch-site status: never ARMED — no [net4F] single-address-latch verdict fired this window (the experiment self-gates on a tag-proven latch) (victim=-1 latched=-1 interim-pops=0 interim-L-hits=0)` (:1366). The MANIFEST's expectation ("`[net4G]` is EXPECTED never to arm") is **satisfied**; the scorer counted a line that says `never ARMED` as an arming. Same family as A38 — a token counted without reading its verdict. Fix the pattern in render9's scorer | A12, A38 |
| A24 rung 3 | `A24 rung3: lines=93 complete=1 refused=0 rung3_complete=1 unreadable=9 bcr_lock=1 opt_wpr=1 cascaded_after_probe=1 -> PASS A24-rung3 (COMPLETE, rung 3 complete, both rung-4 inputs present, and [deskcascade] -> CASCADED after it — the boot continued behind the rung) WITH A FINDING: 9 register(s) UNREADABLE, which is itself the answer rung 4 needs` (`[ga10bprobe3] pg=0x1 clk=2/3 regs=16 of 25 readable, 9 UNREADABLE -> COMPLETE`) | `arch/aarch64/ga10b_probe.rs:1049` | **PASS WITH A FINDING, and both rung-4 inputs are in hand.** `[ga10bprobe3] opt_wpr_enabled=0x00000001` — **WPR is enabled**, so rung 4 must place its carveout accordingly. `[ga10bprobe3] bcr_dmacfg lock_locked=0 (raw=0x00000000)` — **the BCR is NOT locked for this power cycle, so rung 4 CAN reprogram it without a cold boot.** The 9 UNREADABLE are `pri-error 0xbadf5620` in the `gsp-falcon-v1` / `gsp-priscv-bcr` classes = priv-lockdown over the GSP block, which is the answer rung 4 needed. Third datum: DTB `gpu@` `clocks=3: 304 41 236`, and **`[ga10bprobe3] clk 236 identity: is_enabled_err=-22 info_err=-22 in_range=1 -> NOT-IN-BPMP-TABLE (in range, no entry)`** — `GET_ALL_INFO` refused it too, `err=-22` is `-BPMP_EINVAL`, an ARGUMENT rejection and never an `-EACCES` policy refusal, so **rung 2's mystery on id 236 is closed: the DTB names a clock this BPMP firmware does not export.** Not our DTB read (the DTB is what named it) | A24, A39 |
| A24 rung 3b | `A24 rung3b: lines=13 armed=1 write_announces=3 read_announces=2 mailbox_lines=1 wrote=0x5a5aa5a5 read=0x5a5aa5a5 held=1 mismatch=0 skipped=0 terminus=1 -> PASS A24-rung3b MAILBOX-HELD — a GA10B engine register accepted a write from this kernel and held it; the CCPLEX can drive this engine scratch state with the GSP halted` | `ga10b_probe.rs:1141` | **PASS — the ladder's first GA10B MMIO WRITES, and they stuck.** Falcon reset assert/hold/deassert, then `mailbox0 wrote=0x5a5aa5a5 read=0x5a5aa5a5`. Board restored, boot continued | A24, A39 |
| A24 stop-check | `A24 STOP-CHECK: last announce at line 1123 of 18767 -> OK (the boot continued past it)` | `ga10b_probe.rs` | OK — the binding STOP RULE is satisfied; no access was fatal | A24 |
| A25 | `A25 winmenu: publish=8 publish_refuse=0 contend_refuse=1 open=7 last_title=View pick=5 dismiss=7 (esc=1 outside=0 pick=5) -> PASS A25 (published, opened from the BAR, Esc dismissed — R21 satisfied on the wire)` | `video/winmenu.rs:327/634/650/728` | **PASS, and this is the flight where `esc=1` finally appears.** Seven bar opens, five picks, seven dismisses | A25, A10 |
| A25 negative | `A25 negative: in-window title_press=0 in-window menu-dismiss=0 (winmenu pick callback=0, expected>=0) -> PASS (no in-window View strip token — R21 holds)` | `pulsewin.rs` | PASS — R21 holds, second flight | A25 |
| A10 / SO2 / SO3 | `A10/SO2/SO3 menubar2: open=7 (title-x=7 font=7) publish=8 with_app_menu=8 pick=5 quit_picks=5 app_menu_quit=5 closed=5 \| dismiss=7 esc=1 outside=0 (KEY 0x1b seen=1) -> PASS A10+SO2+SO3 (Esc dismissed with reason=esc; the app menu published; a Quit pick actually closed its window)` (`:: tegra: JD2 — KEY 0x1b ::` :5775 → `[winmenu] dismiss reason=esc kind=title owner=3` :5776, ONE line apart) | `winmenu.rs`, `menubar.rs` | **PASS ×3, and A10 closes on metal.** A10: the single Esc on the wire dismissed the bar menu on the very next line — render7's failure (`state=open` printed twice after the Esc, dismissal by `reason=outside`) does not recur. Peter at the glass: "Esc closes a menu". SO2: every open carries `title-x=` and `font=chrome20-bold kind=title`, drop-down under its own title in the bar's type. SO3: **an application menu exists and its Quit works** — `[winmenu] pick owner=2 id=161 label=Quit -> close win=2` (:5236) → `[winmenu] app-menu quit win=2 closed=true` (:5244), and the quarry window **stayed closed** until Peter pressed its dock pin at :5355. That is SO3's proof, and it is the control that convicts A30 | A10, SO2, SO3 |
| SO4 | `A34/SO4 crystal: … \| SO4 menubar_lines=1 gap_right=0 -> SO4 ONLY (geometry present, gap_right=0)` (`[menubar] crystal menu=170x121+1750+34 anchor=right-flush-under-crystal glyph=16x22+1904 bar_w=1920 gap_right=0` :1774) | `video/crystal.rs`, `video/menubar.rs` | **GEOMETRY FLOWN AS LEDGERED.** The drop-down is at x=1750 (right-flush under a glyph at x=1904 in a 1920-wide bar) with `gap_right=0` — render7's `+12+34` left-inset is gone. On the glass Peter saw the crystal at the far right and said "wtf?" — the fix did exactly what SO4 asked for and he is not sure he wants it. **Re-ruling pending; do not touch the code on this report** | A33, SO4 |
| A26 | `A26 conquiet: console_route=1 console_window=2 mirror_off=1 (at line 1) census=0 dropped=0 -> PASS-WEAK (mirror=off once; fewer than 256 lines dropped so no census line — read the glass)` (`[conquiet] mirror=off since=console-window-route win=1 lines_dropped=1 knob=bootlog` :1745; `[wc-x] console-route first-paint win=1` :1757) | `video/fbcon.rs:1227/3018/3025` | **PASS-WEAK on the wire — and the GLASS HALF IS NOW READ.** In `SCREEN3.PNG` the console window (1295×736 at 312,197) is a **solid black surface with no text at all**. That is the strong statement A26 wanted (no kernel log scrolling in the console window) arriving as its opposite problem: the window is not quiet, it is *empty*. Still `PASS-WEAK` for A26's own question; the emptiness is a separate observation for render9 to chase | A26 |
| A17 | `A17 prtscr: armed=5 capturing=4 ok=4 inflight_refusals=1 other_refusals=0 names=[ SCREEN0.PNG SCREEN1.PNG SCREEN2.PNG SCREEN3.PNG ] -> PASS A17 (two files and 1 NAMED InFlight refusal(s) — the render6 gap is closed)` | `drivers/xhci/mod.rs:4929`, `video/prtscr.rs` | **PASS — the silent-swallow gap is CLOSED.** Five arms, four files, and the fifth press printed `:: PRTSCR: refused — capture in flight …`. Every press is now accounted for by a verdict or a named refusal | A17 |
| SR2 / A36 | `SR2/A36 prtscr3: armed=5 capturing=4 ok=4 slices=32 (last n=8) inflight_refusals=1 vanished=0 -> PASS SR2/A36 (sliced encode on the wire, 1 NAMED InFlight refusal(s), 4 files written — the render7 silent-drop gap is CLOSED)` | `video/prtscr.rs` | **PASS — sliced encode flown, `vanished=0`.** But see the NEW findings: Peter's SECOND stimulus (three fast presses at ~15:52Z) produced **exactly one** `PrintScreen (HID 0x46) down` line (:11373), one file (`SCREEN3`) and **zero refusals** — two presses never reached the key path at all. That is a *different* loss than the InFlight refusal SR2 fixed, and it is upstream of the capture door | A36, SR2 |
| SO1(b) / A29 | `SO1/A29 winid: wm_alloc=14 wm_close=11 holders_cleared_closes=6 register_REFUSED=0 \| close-furniture=2 route-dropped=true=1 (last dropped id=1) quarry_open=3 (last id=2) wcgseam=0 -> PASS SO1(b) (1 route-dropped close(s), 6 close(s) cleared a holder — every reopen at a recycled id must carry a HIGHER gen; the two lines below are the pair to read)` (`[wm-act] close-furniture win=1 owner=0xffffff01 closed=true route-dropped=true` :7950; `[wm-act] close-furniture win=4 owner=0xffffff02 closed=true route-dropped=false` :8144) | `video/wm.rs` holder registry (`b19b2865`) | **PASS SO1(b) — the id-reuse half is fixed and proven.** Six closes cleared a holder; ids recycled repeatedly this boot (`win=3` through `gen=7`, `win=5 gen=1`, `win=2 gen=2`) and **not one stale-id reopen occurred**; `register_REFUSED=0` so the 6/8 registry never overflowed. The console close still reports `route-dropped=true` while every other window reports `false` — that half of SO1 is the console-route drop, and it is now scored separately as **SO10** | A29, SO1 |
| A29 wcgseam | `A29 wcgseam: ABSENT — zero `[wcgseam]` lines — NOT evidence the seam never ran: both seam prints lead with `verdict != CLEAN`, and this boot had zero convictions, so the absence carries no information (corrected by orin 17 after WCG; LEDGER SO16 adds the census denominator); acceptable, NOT a failure (this board has never flown it)` | `video/wcg.rs:410` | ABSENT, as on every Orin boot to date | A29, SO7 |
| **SO1 / S4 drain** | `SO1/S4 drain: shell_pin_presses=3 drains=3 declined=0 -> PASS S4 (3 drain(s) after 3 press(es) — the reopen was minted)` | `video/dock.rs` `SHELL_REOPEN` | **WIRE PASS CONTRADICTED BY THE GLASS — the scorer counts drains, not windows. New row SO10.** The drain runs, but it resolves to the CONSOLE's id: `[dock] press at (1065,1162) tile=2/3 shell=pin -> reopen requested` (:8235) → `[wc-fv] focus raise asid=0xffffff01 windows=1 top_win=1` (:8255) → `[dock] shell-reopen drained by=orin_render_service win=1 gen=2 route=routed present=Composited -> REOPEN` (:8258); presses 2 and 3 (:8388, :10775) drain identically with `route=already-live`. **`win=1` / `asid=0xffffff01` is the console, not the shell** (the shell is `win=4` / `asid=0xffffff02`, closed at :8144). No `[wm] alloc` and no `[wc-a] create` for a shell surface follows any of the three, and the dock's shell tile carries a grey (not-running) dot in `SCREEN3.PNG`. Peter: the shell does not reopen. **The scorer's PASS is wrong and this row is a FAIL** | A7, S4, **SO10 (new)** |
| A30 | `A30 deskfix: pulsewin_open=8 close=4 close_final=4 dock_rearm=2 -> FAIL A30 (8 opens for 2 dock re-arm(s) — the render7 shape was 3)` | `video/pulsewin.rs:508/527` | **FAIL, with the mechanism fully resolved — see the A30 section below.** Eight opens = 1 boot + 2 dock re-arms + **5 spurious re-mints**. The `-> CLOSED (reopen only via dock) … the ARMED latch is cleared` line is present on every close, so the fix's own witness fires; the latch is simply cleared **after** `wm::close`, and any render pass landing in that gap re-mints the window | A30 |
| SO5 | `SO5 sprite: lines=8 same=0 cap=n=8/8 -> EXPECTED-TODAY (same=0 — the pal.rs one-liner is FILED UNAPPLIED pending the rmbp grant; the divergence is now on the wire, which is the fix's point)` (`[sprite] size=18x18 scale=2 over=desktop at (440,85) panel=1920x1200 compositor=9x9 backbuffer=18x18 same=0 n=8/8`) | `video/cursor.rs`, `video/pal.rs` | **WITNESS FLOWN, DIVERGENCE MEASURED — exactly as designed.** `compositor=9x9` vs `backbuffer=18x18`, `same=0` on all eight samples: the two-sprite-scale root cause orin 16 derived from the source is now a *measurement*. The fix is rmbp's and is filed unapplied | A35, SO5 |
| V-1…V-4 | `V-1..V-4 fixpanel: v3_contended_declines=1 winmenu_successes=15 (after a decline=11) winmenu_family_lines=224 -> PASS V-3 (a transient lock refusal was DECLINED AND RETRIED — 11 later open/publish succeeded — instead of destroying operator state) and PASS V-1/V-2/V-4 by survival` | `video/winmenu.rs` lock path | PASS V-3 explicitly (one decline, eleven later successes) and V-1/V-2/V-4 by survival — the boot continued past the bar, which is V-1's only falsifier | V-1…V-4 |
| KEYDOORS F0 | `A10-class keydoors (fold ABOARD at c24d9517): KEY 0x09=7 tab-cycle=0 (paired within 60 lines=0) … -> FAIL F0 (7 x KEY 0x09, NO paired tab-cycle within 60 lines — the TAB shell door is still dead…` | `main.rs:2948`, `arch/aarch64/syscall.rs:13381` | **RETIRED BY RULING R24, not a defect** — and the mechanism was found anyway (executor KEYDIAG, this session, read-only). Peter (2026-09-06 ~16:20Z): "tab is not meant to permanently switch between windows — that was a thing we did early for when mouse did not work." Wire datum only: 7 × `KEY 0x09` (:10520–:10608, :12958), `tab-cycle=0`. **Three sub-findings worth keeping** — (i) the SO8 column check is CLEAN at this sha: `main.rs:2948` is 7171 chars with its first `//` at **column 345**, and the three door calls sit at columns 104 / 201 / 308, all ahead of it, so the order-only fix held; (ii) **Quarry does NOT swallow Tab** — `0x09` is unbound in `quarry::key_route`, falling to `_ => acted = false` (`video/quarry/live.rs:2005`) and returning false, so Quarry being open all boot is irrelevant; (iii) the Tab died one level deeper, in `wc_focus_key`'s guard `if cur == 0 && n == 0 { return false; }` (`syscall.rs:13381`), because `focus_ring_apps` filters the focus ring through `key_sink_drains` (`asid <= USER_SLOTS`, :13571) and **every window this boot was kernel-band furniture** (`asid` `0xffffff01`/`02`/`03`/`0xffffff60`), so `n = 0` all boot with `USER_INPUT_ACTIVE = 0` beside it. Zero `[wc-c] focus tab-cycle` lines is therefore a *correct* decline, not a dead door. No fix is owed; retire the scorer leg in render9 | A10, A11, **R24** |
| KEYDOORS F1 | `KEY 0x1b=1 quarry_closed=0 (paired=0) quarry_open=3` | `video/quarry/live.rs:1906` | **RETRACTED BY RULING R24, not FAIL.** The single Esc was pressed on a bar menu and dismissed it (`reason=esc`, :5775–5776), never with Quarry focused, so F1's stimulus never occurred — and Peter then ruled that Esc must never close an app window, which retracts F1's design outright. `quarry::key_route` still carries `if c == 0x1B { close(); return true; }` at `live.rs:1906`; **that line is what R24 retracts**. **Correction to the scorer's reading:** `quarry_closed=0` is that scorer's own Esc-close token, NOT evidence that Quarry never closed — Quarry closed twice this boot by app-menu Quit (`[wm] close win=2 gen=1` :5237, `gen=2` :6682) and reopened only on an explicit dock pin | A10, SO9, **R24** |
| DRAGREL-A64 | `DRAGREL-A64: NOT-EXERCISED (grab a title bar and press TAB without releasing — FLIGHT SEQUENCE step 10b)` | — | NOT EXERCISED — step 10b was not flown. Under R24 the Tab *focus cycle* is retired, but **A11 (Tab mid-drag leaves the grab) is a different question and stays open** | A11 |
| A34 | `A34/SO4 crystal: restart_announce=0 restart_RETURNED=0 shutdown_announce=0 shutdown_RETURNED=0 … A34 NOT EXERCISED: the Restart verb was never picked.` | `[SHARD-MENU] crystal_pick` | **NOT EXERCISED — unflown, not failed.** Peter powered the board off at the wall; step 13 was skipped. `A34` keeps its `fixed-unflown` status and is render9's first owed leg | A34 |

## A30 — TWO defects, not one (code reading by executor KEYDIAG, this session, read-only)

Four close-box closes and three pulse app-menu Quit picks. Every one prints the fix's own witness
(`[pulsewin] close win=3 -> CLOSED (reopen only via dock) ... A30 - the ARMED latch is cleared`), and five of the
seven reopened anyway. Two different faults produce that, and the wire separates them by one field:

| # | stimulus | `[wm] close` line | `holders-cleared` | alloc between? | outcome |
|---|---|---|---|---|---|
| 1 | close-box :6175 | `win=3 gen=1` :6177 | `0 names=,,,` | **yes** - `[wm] alloc win=3 gen=2` :6180 | **reopened** :6187 |
| 2 | close-box :6218 | `gen=2` :6220 | `0` | no | **stuck** (dock pin :6369 reopened it) |
| 3 | close-box :6465 | `gen=3` :6467 | `0` | **yes** - `alloc win=3 gen=4` :6470 | **reopened** :6477 |
| 4 | close-box :6492 | `gen=4` :6494 | `0` | no | **stuck** (dock pin :6602) |
| 5 | Quit pick :7294 | `gen=5` :7296 | **`1 names=pulsewin`** | **yes** - `alloc win=3 gen=6` :7299 | **reopened** :7305 |
| 6 | Quit pick :7403 | `gen=6` :7405 | **`1 names=pulsewin`** | **yes** - `alloc win=5 gen=1` :7406 | **reopened as `win=5`** :7418 |
| 7 | Quit pick :7497 | `win=5 gen=1` :7499 | **`1 names=pulsewin`** | **yes** - `alloc win=3 gen=7` :7500 | **reopened at recycled `win=3`** :7511 |

*(Correction to the flight brief, which recorded "both app-menu Quit picks": there were **three**, and all three
reopened. `holders-cleared` is the tell - see below.)*

### Defect 1 - the close-box RACE (`video/pulsewin.rs:510-526`)

```rust
510  let id = WIN.swap(wm::WIN_NONE, Ordering::AcqRel);   // the WIN cell is released FIRST
518  winmenu::clear(id);                                   // composites
521  wm::close(id);                                        // prints "[wm] close win=..."
522  ARMED.store(false, Release); SURF.store(0, Release);  // the latch clear - LAST
526  serial_println!("[pulsewin] close win={} -> CLOSED ...")
```

`pulsewin::service()` (`:594`/`:611`), driven from `orin_render_service` (`main.rs:8358`) on a different task and
core, mints a window whenever it samples `WIN == WIN_NONE && ARMED == true`. **That state exists from :510 to
:522** - and :518's composite plus :521's drain barrier and serial line are hundreds of microseconds inside the
hole. So the exposed interval is wider than "after `wm::close`": it opens at the `WIN.swap`. The wire shows the
interleave verbatim at :6175-:6187, with `[wm] alloc win=3 gen=2` (:6180) landing *before* the `CLOSED` print
(:6185). The two closes that stuck are the two with no render pass in the gap; their dock re-arms at :6369/:6602
are genuine (`[dock] press ... pulse=pin -> rearmed`), so those closes really did hold.

**Fix shape (NOT APPLIED): make `ARMED.store(false, Release)` the FIRST statement of `close()`, ahead of the
`WIN.swap` at :510.** Moving it merely above `wm::close` is not enough - the hole starts at the swap.

### Defect 2 - the Quit BYPASS (`video/winmenu.rs:1319-1324`), a guarantee rather than a race

```rust
1321  APP_ITEM_QUIT => {
1322      clear(win);
1323      let closed = wm::close(win);      // raw wm::close - pulsewin::close() is NEVER called
1324      serial_println!("[winmenu] app-menu quit win={} closed={}", win, closed);
```

`ARMED` is therefore **never cleared on a Quit**, so the next render pass reopens the window with certainty. The
wire proves the bypass without reading the source: the Quit closes print `holders-cleared=1 names=pulsewin` (the
WINID registry doing the clearing, because `WIN` still held the id) while the close-box closes print
`holders-cleared=0 names=,,,` (`pulsewin::close()` had already emptied `WIN` at :510). Three Quits, three reopens,
no exceptions.

**The control that convicts both**: the same app-menu Quit on the **quarry** window closes it and it stays closed
(`[wm] close win=2 gen=1` :5237 -> `[wc-a] close win=2` :5239 -> nothing until the dock pin at :5355). Quarry has
no `ARMED` latch, so the raw `wm::close` is harmless there. The bar's Quit path and `wm::close` are both correct;
the defect is `pulsewin`'s latch - its ordering, and the fact that nothing in the Quit path touches it.

**Fix shape (NOT APPLIED):** dispatch the QUIT arm to the row's owning module close instead of raw `wm::close`
(the accessor `pulsewin::win()` already exists at `pulsewin.rs:243`). **UNDECIDED and deliberately left open:**
whether that is a per-owner special case in `winmenu.rs` or a generic close hook in `wm::close` - that is a
`video/` design call and `video/` is rmbp's lane. Peter's "hit x twice" is defect 1; his "menu quit does not quit
pulse" is defect 2.

## Glass — Peter at the bench (~15:5xZ)

> console and shell are separate windows … i closed console and shell; console disappears from the dock; pressing
> the SHELL tile does not reopen the shell / esc closes a menu / pulse's "View" title stays in the bar / crystal is
> at the far right — wtf? / menu quit does not quit pulse and i have to hit x twice to close it / i did the tab
> thing … it did nothing / i hit prt sc three times fast [→ one capture]

*(Recorded as substance, not keystroke-verbatim, except the fragments in quotes inside the table below.)*

| Peter's line | Wire / pixels | Row |
|---|---|---|
| "console and shell are SEPARATE windows"; console disappears from the dock when closed | Correct and by design: `win=1` console `asid=0xffffff01`, `win=4` shell `asid=0xffffff02`. `[wm-act] close-furniture win=1 … route-dropped=true` (:7950) then the dock falls to three tiles (`tile=2/3` at :8235) | A29, SO1 |
| "pressing the SHELL tile does NOT reopen the shell" | Three presses, three drains, **zero shell windows** — the drain resolves to `win=1` (console) and reports `route=already-live`. See the SO1/S4 row and **SO10** | S4, **SO10** |
| "esc closes a menu" | `KEY 0x1b` :5775 → `[winmenu] dismiss reason=esc` :5776. **A10 PASS on the glass and on the wire** | A10 |
| "pulse's View menu title stays in the bar" (after pulse close) | This is A30's re-mint seen from the bar: the window is re-created before the bar's menu clear runs, so the title never goes. Not a separate defect — it disappears when A30 is fixed | A30 |
| "crystal is at the far right — wtf?" | `[menubar] crystal menu=170x121+1750+34 anchor=right-flush-under-crystal glyph=16x22+1904 bar_w=1920 gap_right=0`. This is **SO4 exactly as he specified it on render7** ("it should be all the way to the right edge"). **Peter's re-ruling, if any, is pending — the code is not to be touched on this report** | A33, SO4 |
| "menu quit does not quit pulse and i have to hit x twice" | A30, resolved to the line above | A30 |
| "i did the tab thing … it did nothing" | 7 × `KEY 0x09`, `tab-cycle=0`. **Retired by R24** — the feature is withdrawn, not broken | R24 |
| "i hit prt sc three times fast" → one capture | ONE `PrintScreen (HID 0x46) down` on the wire (:11373), one file, **zero refusals**. Two presses produced no key-down at all — lost upstream of the capture door, in xHCI/HID. NEW, and distinct from the InFlight refusal SR2 closed | A36, SR2 |

### What `SCREEN3.PNG` shows (~15:52Z; committed as a 25% reduction)
- **Menu bar**: `quarry` (the app title) then `View` — SO3's app menu and SO2's bar-hosted window menu, both present.
- **Crystal**: glyph hard against the right edge of the 1920-wide bar (SO4 geometry, `gap_right=0`).
- **Quarry window**: listing `/usb` with `SCREEN0.PNG`/`SCREEN1.PNG`/`SCREEN2.PNG` at 6751K each, and a `MODIFIED`
  column that is blank for every row — **FAT timestamps are not being read or not being rendered**. New, minor,
  filed below.
- **Console window**: a solid **black** 1295×736 surface, no text (the A26 glass half).
- **Pulse window**, bottom-left: its **title bar / chrome is ~300 px wide while the LED band it belongs to extends
  ~1290 px to the right** — the chrome and the surface disagree. `[pulsewin] open … surf=1280x168 box=1290x212`
  says the surface is 1290; the chrome is drawn at some other width. New row **SO11**.
- **Dock**: four tiles — console, quarry, pulse, shell — with **shell's running-dot grey** while console's,
  quarry's and pulse's are lit blue (decoded from the full-resolution capture, x740–1300, y1130–1200). That is the
  dock agreeing with SO10: no shell window exists.
- **And the console tile is BACK, lit.** Peter closed the console at :7950 and the dock fell to three tiles
  (`tile=2/3` on the first shell-pin press, :8235). By the second press it reads `tile=3/4` again (:8388). The
  reason is SO10 itself: the first shell-pin drain took the mint arm and minted **the console**
  (`[wm] alloc win=1 gen=2`, `route=routed present=Composited`). So the dock's own tile count is a third,
  independent witness that pressing SHELL brought back CONSOLE.

## New findings this flight

- **SO10 — the shell-reopen drain resolves to the console's window id and reports success.** `SHELL_REOPEN` is
  drained on aarch64 now (the S4 fix `b19b2865` is aboard and runs), but what it drains to is `win=1`,
  `asid=0xffffff01` — the console — and it reports `route=routed present=Composited` on the first press and
  `route=already-live` on the next two. No shell surface is ever minted. **S4 is therefore only half-fixed: the
  latch is drained, the target is wrong.** The scorer says PASS because it counts drains; the glass says FAIL
  because it counts windows.
  **Root cause, read from the source (executor KEYDIAG, this session, read-only) — it is not a stale cache.**
  `orin_shell_reopen_drain` (`main.rs:9015`) keys BOTH arms on the console's global cell: the predicate is
  `fbcon::console_is_routed()` (:9022, reading `CONSOLE_WIN`, `video/fbcon.rs:2242`), the already-live arm returns
  `fbcon::console_win()` (:9023), the mint arm calls `fbcon::panel_console_window_open()` (:9032), and both
  `focus_changed` calls name `wm::KERNEL_OWNER_CONSOLE`. **There is no shell arm at all.** Why `win=1 gen=2`
  specifically: Peter closed the console FIRST (:7950, `route-dropped=true`), so `console_is_routed()` was false on
  press 1 and the drain took the mint arm — which minted the *console* (`[wm] alloc win=1 gen=2`, `[wc-a] create
  win=1 asid=0xffffff01`); presses 2 and 3 then took the already-live arm. No `[wc-a] create` with
  `asid=0xffffff02` appears anywhere after :8144.
  **Why the Orin drain is the only one of three that does this:** the Orin's shell row is minted by
  `tegra_shell_window_open` (`main.rs:8691`) and its id lives ONLY in a local inside `jd2_console_pump`
  (`main.rs:2874`) — a different task from `orin_render_service` — with no static cell (`TEGRA_SHELL_PRESENTED`,
  `main.rs:8789`, is a bool). The Pi's `pi_shell_reopen_drain` (`main.rs:9124`) and x86's inline drain
  (`main.rs:6765`) both test `shell_id != WIN_NONE && wm::owner_of(shell_id) == Some(KERNEL_OWNER_DESKTOP)` because
  the id is in scope there. **Fix shape (NOT APPLIED):** publish the Orin shell's id in a static registered with
  `wm::winid_register_holder` (the pattern `pulsewin.rs:478` already uses), then give :9022 the Pi's predicate and
  the shell's own open path as the mint arm. **UNDECIDED:** whether the console-reopen at :9032 is meant to remain
  as a second, console-tile behaviour — `main.rs:8358`'s comment says the substitution was deliberate but does not
  give a reason a reader can check. Filed on the shared ledger (`video/dock.rs` + `main.rs` drain, rmbp's lane).
- **SO11 — a window's chrome is drawn at a different width from its surface.** The pulse window's title bar is
  ~300 px wide against a ~1290 px surface (`SCREEN3.PNG`). Glass-only today: no token carries the chrome rect, so
  nothing on the wire can score it — the same witness gap SO2 had before MENUBAR2. Filed on the shared ledger
  (`video/wm.rs` chrome layout).
- **Print Screen loses presses upstream of the capture door.** Distinct from A17/SR2's InFlight refusal, which is
  now correctly named on every occurrence. Peter's three fast presses produced ONE `PrintScreen (HID 0x46) down`
  line. The loss is in xHCI/HID key delivery, before `prtscr` sees anything — so no refusal can be printed for it,
  and the fix is not in `prtscr.rs`. Recorded against A36/SR2 rather than opening a row: the next flight must
  press slowly enough to separate this from the capture-door path, and instrument the HID queue if it recurs.
- **A scorer that counts a token without reading its verdict, twice.** `[net4G] latch-site status: never ARMED`
  was scored as "armed — UNEXPECTED", and `SO1/S4 drain` scored three drains as a reopen. Both are A38's family
  and both are in `scorers-render8.sh`. render9's scorer must match verdict tails, not family tokens.
- **Quarry shows no modification times.** `MODIFIED` column blank for every entry in `SCREEN3.PNG` while `SIZE` is
  correct. Minor; no row opened — noted here so the next quarry arc has it.
- **The card's FAT boot-sector label and its root-directory label disagree** (`UNAOS-PI` vs `UNAOS-ORIN`). A card
  tooling datum, recorded on A28.

## RULING — R24 (Peter, 2026-09-06)

Numbered **R24**, and here is why: the `RULINGS.md` sequence is shared across seats and merged by union. The orin
tree carries R8–R21; the `hw-rmbp` tree carries R20, **R22 and R23** (`git -C ../UnaOS-rmbp grep -n 'R2[0-9]' --
docs/dev/RULINGS.md`, run this turn). The brief said "if R22 exists there use R23" — **R23 exists there too**, so
the next free id in the union is **R24**. (The `R22`/`R23`/`R24` tokens elsewhere in `docs/dev/OS/` are round and
sitting labels — `LC-metal R22 sitting-2`, `R23s1f`, `R24 boot7` — a different namespace that has coexisted with
the rulings sequence since it was seeded; they do not consume ruling ids.)

**Two clauses, heard the same afternoon:**

1. **(~15:55Z)** "esc should not close any app windows" — **Esc dismisses menus only.** It retracts KEYDOORS F1's
   design (Esc closes Quarry) outright. F1 is recorded as RETRACTED BY RULING, never as a FAIL. Note the second-order
   effect: SO9's stated workaround ("close Quarry with Esc and the shell behaves") is withdrawn with it — SO9 now
   has **no** operator workaround, which raises its priority.
2. **(~16:20Z)** "tab is not meant to permanently switch between windows — that was a thing we did early for when
   mouse did not work" — **the Tab focus-cycle is RETIRED.** KEYDOORS F0 is RETIRED, not a defect; its wire fact
   (7 × `KEY 0x09`, `tab-cycle=0`) is a datum only. A11 (Tab mid-drag leaves the grab) is a **separate** question
   and is not retired by this.

## Not scored
- **A34** — crystal Restart / Shut Down. Step 13 was not flown; the board was powered off at the wall. Unflown,
  not failed. **First owed leg of render9.**
- **DRAGREL-A64 / A11** — Tab mid-drag, step 10b, not flown.
- **A2's `bg` half** — `bgrun=0`: nothing was ever launched, because the shell was unusable all boot (SO9 + SO10).
  The HOSTING predicate flipped, which is the half that was askable.
- **A26 strong leg** — still needs a boot that drops more than 256 console lines so `[conquiet] census dropped=`
  prints.
- **A20's instrument** — untriggered for the second consecutive boot.
- **SCREEN1 / SCREEN2 pixel legs** — harvested and checksummed, not read. Only `SCREEN0` (A19 band) and `SCREEN3`
  (the glass reading above) were decoded.

## Card after the flight
`SCREEN0.PNG` … `SCREEN3.PNG`, four valid 1920×1200 captures of 6 913 793 B each; sha256 of each in
[`render8-card-harvest.sha256`](render8-card-harvest.sha256). All four harvested to the bench, the card tidied of
them and unmounted, so render9's first capture will name itself `SCREEN0.PNG`. The full-size PNGs are deliberately
NOT committed (27 MB); the 25% reduction of `SCREEN3` is.

*Tooling note for whoever repeats this: this bench host has **neither PIL nor ImageMagick** (`convert`, `magick`,
`gm`, `ffmpeg` all absent). The reduction and every region decode in this report were done with a stdlib-only
box-downscaler built on `A19-pngband.py`'s PNG decoder (zlib + the five PNG filters), kept at
`~/unaos-bench/scratch/orin17/png-downscale.py`. Do not plan a pixel leg around `convert -resize` here.*

## What render9 must carry
1. **A34** — crystal Restart, flown as the last action of the sitting (it reboots the board). This is the one
   question render8 was staged to answer and did not.
2. **A30's TWO fixes** — (a) move `ARMED.store(false, Release)` to be the first statement of
   `video/pulsewin.rs::close()`, ahead of the `WIN.swap` at :510 (moving it merely above `wm::close` leaves the
   hole open); (b) stop `video/winmenu.rs:1323`'s QUIT arm bypassing the owner's close. The wire already carries
   its own falsifier for both (`[wm] alloc` between close and CLOSED; `holders-cleared=1 names=pulsewin`).
3. **SO10's fix** — publish the Orin shell's window id in a static (`wm::winid_register_holder`) and give
   `main.rs:9022` the Pi's predicate plus the shell's own open path as the mint arm; and its scorer leg must count
   an `[wc-a] create … asid=0xffffff02`, not `[dock] shell-reopen drained` lines.
4. **SO11** — a witness that prints the chrome rect beside the surface rect, so the divergence can be scored from
   the wire rather than from a screenshot.
5. **SO9 with Esc withdrawn** — under R24 there is no operator workaround left for Quarry stealing Enter and
   Backspace. Focus-gated key routing for open-but-unfocused tenants moves up the list; until it lands, the shell
   cannot be exercised at all on a desktop that boots with Quarry open, and A2's `bg` half stays unflown behind it.
6. **A12's index defect** — `REFETCH-WRONGSLOT` ×3 names our own post-enable ring bookkeeping. The NIC-errata
   branch is acquitted (`STALE-ORIG=0`); this is a lane we own.
7. **A16's burst leg** — `PARTIAL 3/5` is now a mailbox-depth question, not a two-reader question. Either deepen
   the drain cadence or give the mailbox backpressure; do NOT un-park the UARTC reader (that is what A37 fixed).
8. **Scorer repairs** — retire the F0 leg (R24), fix the `[net4G]` pattern to read `never ARMED`, and re-write the
   S4 drain leg to count minted windows.
9. **A24 rung 4** — both inputs are in hand: `opt_wpr_enabled=1` and `bcr lock_locked=0` (the BCR can be
   reprogrammed without a cold boot), with priv-lockdown over the GSP falcon/priscv block as the wall.

## Ledger

Ticked in `docs/dev/OS/orin-ledger.md` this flight: **A1** (fifth sample, `hw=14744`), **A8** (PASS, render7's
defect gone), **A10** (PASS on wire and glass; F1 retracted, F0 retired by R24), **A12** (net5 flown,
`REFETCH-WRONGSLOT` located), **A15** (pass 6), **A16** (rung 2 second pass; burst PARTIAL under `mbox-only`),
**A17** (PASS, gap closed), **A19** (wire PASS + **pixel PASS**), **A20** (routing PASS; instrument untriggered
again), **A21** (tick PASS on the full desktop, `tmax=179500`; run HOSTING + first EL0 `eret` ACCEPTED), **A24**
(rungs 3 and 3b PASS with the datums), **A25** (PASS, second flight), **A26** (PASS-WEAK + the glass half),
**A27** (PASS, second flight), **A28** (PASS — the VFS root has a backend), **A29** (SO1(b) PASS), **A30**
(FAIL with the mechanism resolved), **A33** (SO4 geometry flown as ledgered; Peter's re-ruling pending), **A34**
(**NOT EXERCISED**), **A35** (SO5 witness flown, divergence measured), **A36** (SR2 PASS + the lost presses),
**A37** (PASS, SINGLE-SOURCE), **A39** (rungs 3/3b flown).

Opened this flight, on `docs/dev/LEDGER.md`: **SO10** the shell-reopen drain resolves to the console's window id ·
**SO11** window chrome drawn at a different width from its surface.

Ruling recorded in `docs/dev/RULINGS.md`: **R24** (two clauses — Esc dismisses menus only; the Tab focus-cycle is
retired).
