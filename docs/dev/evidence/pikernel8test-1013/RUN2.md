# PIKERNEL8TEST2 — `./arroyo kernel8-test` on the image-24 Pi line, with the three build fixes (2026-10-13)

Branch `exec-rmbp-local-pikernel8test2` off `35af76a3e` (carries f68f118be, the CURSORSHAPE registry gate, and
35af76a3e, banner-cert's `lumen` witness `:: LUMENAPP:` — PI11's fix) + cherry-pick of `f5dd50a63` (PI10's fix,
WIREDIET M1 gated to x86_64; applied clean as 6742b1fee) + cherry-pick of `f6b1424b3` (the PIKERNEL8TEST record:
RUN.md and the PI10/PI11 rows, docs only, so this record and its ledger updates sit beside them). Executor run,
LOCAL WAVE 2, QEMU raspi4b leg (Pi track). One run.

Command, from `unaos/` (in-tree build, no CARGO_TARGET_DIR, default 60 s window):

    set -a && . ./scripts/image-line-pi.env && set +a && ./arroyo kernel8-test

**rc=1, wall 568 s** (build + staging + banner-cert + 60.5 s of QEMU + mbench). Host load average 16-20 on 20
cores at launch (other worktrees building); 8.4 one minute after the run.

## Verdict: FAIL — 112/128 required, 27 forbidden hits. The build line is green; the boot is not.

Build-side, all three fixes held:

- VUG staging green: `✅ static-ELF EL0 kernel-vug program: VUGK.ELF (12648 B)` (PI10 flown).
- banner-cert, verbatim:

      ⚡ banner-cert: arch=aarch64  (FLAT image (no ELF header); the caller named this arch in argument 3)
      ⚡ banner-cert: ok=21 missing/leak=0 registered-divergences=0 unverifiable=4 nowitness=1 noverdict=0  (table: 92 measured, 3 unmeasured-here)
      ✅ banner-cert: every feature the banner named is in the artifact.

  (PI11 flown.)
- kernel8.img booted under QEMU raspi4b: 6500 serial lines, 500889 B (`serial-pi.log` beside this file, under the
  1 MB cap so not truncated). No `:: BOOT:` line is emitted on this path (0 hits, `LC_ALL=C awk`).

**How the run ended: BY WALL.** The runner printed
`⚡ kernel8-test: no completion inside the 60s cap — full wall paid (60.5s), saved 0s.` Yet the replay shows both
COMPLETE markers (`:: SCHED: task 'el0-midden' -> core`, `:: BANDY-RT:`) and `CAPSTONE COMPLETE`, and mbench says
"the end-of-run marker was seen — the run completed, so a missing witness here is a GENUINE regression". So the
completion markers landed but the live completion detector did not fire before the wall (harness disagreement,
noted here, not filed). The last serial lines are the idle `[click2] depth gui_chan=0 (sent=187 recv=187)
app_active=false` heartbeat: the guest sat at its steady state when killed. Under the rule a wall-ended run is
never a pass; this one is a FAIL on its own terms regardless.

Caution on reading the reds: the host was loaded, and several reds are timing-shaped (chrome-truth's defer budget
exhausted 33/32, dragwedge's stall, SERWIT-2's 10 dropped tste records, wc-g's BLIT verdicts). Others are
deterministic spec/emitter divergences (BGRUN-ST's capacity text, knurl's box, WMCTRL's floor) or codec failures
(K3-mount, BANDY-CODEC) that load does not produce. Each is filed as its own row, PI12-PI21; none was diagnosed
past the serial line (an executor builds no fixture here).

## Reds filed (pi-ledger)

- PI12 — SERWIT-2: `FAIL — balanced=true evidence_lost=10` (tste tap dropped=10).
- PI13 — K3-mount: `ls/cat byte-verified FAIL [w=0x1df]`.
- PI14 — BANDY-CODEC / BANDY-CODEC2: `FAIL — w=0x2f/0x3f`.
- PI15 — BGRUN-ST: emitter says `12 rows (... EL0 slots=heap, R90)`; spec `:789`/`:1983` still pin 6 rows / slots 8.
- PI16 — knurl: `box=12`, spec `:2180` pins `box=24` (hash identical).
- PI17 — WMCTRL: `floor=115`, spec `:2302` pins `floor=151` (line otherwise PASS).
- PI18 — focus / window-management cluster: wc-fv FAIL, TABRING SKIP ("an app holds the keyboard (focus=0xc0c)"),
  wc-c tab-cycle absent, wc-f twin DEFER only, wc-iso FAIL `owner_sibling_up=true`, clickroute FAIL
  `hidden_owner=0xffffff61` + close line absent, spinhunt orphan-reap line absent.
- PI19 — compositor paint cluster: chrome-truth `defers=33 budget=32 exhausted=true` + verdict `hits=0/10 -> FAIL`,
  8x `[wc-g] ... -> BLIT`, SHARD-PRESS `painted=false`.
- PI20 — dragperf `coalesced=0 -> FAIL`; dragwedge `furniture aimed=false ... -> FAIL`.
- PI21 — QUARRYVOL `mounts=6 ... rows=4 ... rows_ge_mounts=false ... match=false -> FAIL`.

PI10 and PI11 status cells updated: both fixes flew in this run (build line green, kernel8.img booted under QEMU).
The kernel8-test leg itself is not green, so they read "fixed, flown" rather than "kernel8-test green".

## Every spec verdict line, verbatim (ANSI stripped)

```
════════════ MBENCH VERDICT — pi4-regression.spec vs /home/pmes/src/github.com/pmes/UnaOS-hw-pi4/.claude/worktrees/agent-a755df87052f2e4cd/unaos/target/serial-pi.log ════════════
  ✅ COMPLETE   :: SCHED: task 'el0-midden' -> core
       end-of-run marker SEEN @ line 2762 — the run got past this point
  ✅ COMPLETE   :: BANDY-RT:
       end-of-run marker SEEN @ line 2863 — the run got past this point
  ❌ REQUIRE    :: SERWIT-2: mirror taps .*-> PASS ::
       0 hits — MISSING (pinned at pi4-regression.spec:265)
  ✅ REQUIRE    :: BOT-PARK: selftest .*-> PASS ::
       1 hit(s), first @ line 469
  ✅ REQUIRE    CAPSTONE COMPLETE
       1 hit(s), first @ line 208
  ✅ REQUIRE    M6b: EL0 fault isolation.*-> PASS
       1 hit(s), first @ line 443
  ✅ REQUIRE    M6g: disk-loaded EL0 program exited ok -> PASS
       1 hit(s), first @ line 515
  ✅ REQUIRE    :: ERET-SCRUB: first-entry GPR/FP/TPIDR residue = 0 .*-> PASS ::
       1 hit(s), first @ line 490
  ✅ REQUIRE    :: ERET-SCRUB: syscall-return preserved .*-> PASS ::
       1 hit(s), first @ line 493
  ✅ REQUIRE    U4: process model.*-> PASS
       1 hit(s), first @ line 528
  ✅ REQUIRE    U5: capabilities.*-> PASS
       1 hit(s), first @ line 534
  ✅ REQUIRE    U6: general object table.*-> PASS
       1 hit(s), first @ line 547
  ✅ REQUIRE    U6b: real File handles.*-> PASS
       1 hit(s), first @ line 552
  ✅ REQUIRE    U7: cross-process transfer.*-> PASS
       1 hit(s), first @ line 562
  ✅ REQUIRE    \[u7fix\] park margin — child parked [0-9]+ms before GO \(parked_out=0\), parent parked [0-9]+ms before GO \(parked_out=false\); park primitive=SYS_SLEEP_MS budget=0x8000
       1 hit(s), first @ line 559
  ✅ REQUIRE    U8: revocation trees.*-> PASS
       1 hit(s), first @ line 569
  ✅ REQUIRE    U9: real File writes.*-> PASS
       1 hit(s), first @ line 574
  ✅ REQUIRE    U10: file growth.*-> PASS
       1 hit(s), first @ line 579
  ✅ REQUIRE    U10-create: file create.*-> PASS
       1 hit(s), first @ line 584
  ✅ REQUIRE    U10-delete: file delete.*-> PASS
       1 hit(s), first @ line 589
  ✅ REQUIRE    U11: open-file lifecycle.*-> PASS
       1 hit(s), first @ line 594
  ✅ REQUIRE    U11-defer: cross-process unlink-defers-free.*-> PASS
       1 hit(s), first @ line 638
  ✅ REQUIRE    \[u11fix\] park margin — B parked [0-9]+ms before unlink-GO \(parked_out=0\), A parked [0-9]+ms before read-GO \(parked_out=false\) and [0-9]+ms before close-GO \(parked_out=false\); park primitive=SYS_SLEEP_MS budget=0x8000
       1 hit(s), first @ line 633
  ✅ REQUIRE    U11-reuse: sys_unlink slot-recycle.*-> PASS
       1 hit(s), first @ line 642
  ✅ REQUIRE    U11-reap: teardown-last-close reaper.*-> PASS
       1 hit(s), first @ line 718
  ✅ REQUIRE    U6-grants: owner/grants on open.*-> PASS
       1 hit(s), first @ line 725
  ✅ REQUIRE    K1-persist:.*SURVIVE REBOOT.*PASS
       1 hit(s), first @ line 744
  ✅ REQUIRE    K1-corrupt:.*fails closed to PUBLIC at boot PASS
       1 hit(s), first @ line 750
  ✅ REQUIRE    F2-witness:.*locked 240000/240000 intact
       1 hit(s), first @ line 731
  ✅ REQUIRE    F3-witness:.*locked 240000/240000 intact
       1 hit(s), first @ line 737
  ✅ REQUIRE    K2-liveenf:.*rebuild\+enforce PASS
       1 hit(s), first @ line 762
  ✅ REQUIRE    K3-revoke:.*durable-first PASS
       1 hit(s), first @ line 2276
  ✅ REQUIRE    K9-parity:.*discarded residue PASS
       1 hit(s), first @ line 2371
  ❌ REQUIRE    K3-mount:.*byte-verified PASS
       0 hits — MISSING (pinned at pi4-regression.spec:399)
  ✅ REQUIRE    K4-write:.*clean-tree PASS
       1 hit(s), first @ line 2420
  ✅ REQUIRE    K8a-cow:.*PASS
       1 hit(s), first @ line 2475
  ✅ REQUIRE    K8b-snap:.*PASS
       1 hit(s), first @ line 2547
  ✅ REQUIRE    K8c-snapread:.*PASS
       1 hit(s), first @ line 2595
  ✅ REQUIRE    K4-ready:.*prefix\) PASS
       1 hit(s), first @ line 2385
  ✅ REQUIRE    IMG-SIG:.*residual closed\) PASS
       1 hit(s), first @ line 2374
  ✅ REQUIRE    FATDIRS:.*delete_located\) PASS
       1 hit(s), first @ line 2382
  ✅ REQUIRE    FATMOVE:.*keep-chain\) PASS
       1 hit(s), first @ line 2391
  ✅ REQUIRE    K6-migrate:.*legacy PROGRAM_NAME stays\) PASS
       1 hit(s), first @ line 2666
  ❌ REQUIRE    BANDY-CODEC:.*decode fail-closed.*PASS
       0 hits — MISSING (pinned at pi4-regression.spec:470)
  ❌ REQUIRE    BANDY-CODEC2:.*decode fail-closed.*PASS
       0 hits — MISSING (pinned at pi4-regression.spec:477)
  ✅ REQUIRE    BANDY-STAMP:.*gen-fenced PASS
       1 hit(s), first @ line 2731
  ✅ REQUIRE    BANDY-RT:.*self-cleaned PASS
       1 hit(s), first @ line 2863
  ✅ REQUIRE    BANDY-EQ:.*both legs at EL0 through the production paths PASS
       1 hit(s), first @ line 2865
  ✅ REQUIRE    BANDY-WR:.*mv->cat\(new\) byte-exact.*PASS
       1 hit(s), first @ line 2866
  ✅ REQUIRE    BANDY-EQ2:.*byte-same -EACCES.*PASS
       1 hit(s), first @ line 2868
  ✅ REQUIRE    BANDY-ACL:.*foreign owner row intact.*PASS
       1 hit(s), first @ line 2869
  ✅ REQUIRE    BANDY-GRANT:.*grant re-persisted durable.*PASS
       1 hit(s), first @ line 2758
  ✅ REQUIRE    UVUG: frames=300 threads=2 checksum=0xf18f983557b87a55
       1 hit(s), first @ line 972
  ✅ REQUIRE    USER: input router — routed=2 .*GUI_CHANNEL bypassed :: PASS
       1 hit(s), first @ line 124
  ✅ REQUIRE    \[inroute\] router window — routed=2 stale_dropped=1 revokes=0
       1 hit(s), first @ line 123
  ✅ REQUIRE    EL0: window verbs.*witness=0x3ffff.*PASS
       1 hit(s), first @ line 1004
  ✅ REQUIRE    \[wc-c\] side-by-side windows=2 drawn=2
       1 hit(s), first @ line 864
  ✅ REQUIRE    \[wc-d\] verify win=.*bad_cache=0 bad_ram=0.*-> PASS
       25 hit(s), first @ line 636
  ✅ REQUIRE    \[wc-b\] rollup presents=[0-9]+ banded=[0-9]+ maxbands=[0-9]+ chrome_rows=[0-9]+ chrome_rows_used=[0-9]+ amp=1\.00x
       33 hit(s), first @ line 666
  ❌ REQUIRE    \[wc-fv\] focus-vis .*-> PASS
       0 hits — MISSING (pinned at pi4-regression.spec:675)
  ❌ REQUIRE    \[wc-c\] focus tab-cycle [0-9]+ -> [0-9]+ \(ring of [0-9]+ \+ shell\)
       0 hits — MISSING (pinned at pi4-regression.spec:724)
  ❌ REQUIRE    :: TABRING: TAB rotation — witness=0xff .*noweld=true shellpass=true :: PASS ::
       0 hits — MISSING (pinned at pi4-regression.spec:725)
  ✅ REQUIRE    \[wedge1\] dwell drains=.*spin_max=.*mvbound=[0-9]+ mvgiveup=[0-9]+ mvskip=[0-9]+ ywait=[0-9]+ ydrain=[0-9]+ scskip=[0-9]+ rescued=[0-9]+ grace=[0-9]+ latched=.*->
       5 hit(s), first @ line 945
  ✅ REQUIRE    \[storm\] boot-baseline \| busy
       1 hit(s), first @ line 111
  ✅ REQUIRE    BGRUN-ST: spawn->exit->reap PASS
       1 hit(s), first @ line 1879
  ❌ REQUIRE    BGRUN-ST: process table capacity = 6 rows \(bg programs alive at once; EL0 slots 8\)
       0 hits — MISSING (pinned at pi4-regression.spec:789)
  ✅ REQUIRE    BGRUN-ST: slot reclaim PASS
       1 hit(s), first @ line 1941
  ✅ REQUIRE    BGRUN-ST: kill mid-run PASS \(pid=[0-9]+, killed — row reaped
       1 hit(s), first @ line 1961
  ✅ REQUIRE    BGRUN-ST: persist\+kill PASS \(pid=[0-9]+,
       1 hit(s), first @ line 2245
  ✅ REQUIRE    KILLBOUND: 5/5 rounds .*PASS
       1 hit(s), first @ line 1706
  ✅ REQUIRE    \[wc-e\] fb-geometry .*row_ok=true fit_ok=true
       1 hit(s), first @ line 2
  ✅ REQUIRE    \[wc-f\] scanout .*-> PASS
       1 hit(s), first @ line 608
  ❌ REQUIRE    \[wc-f\] twin .*-> PASS
       0 hits — MISSING (pinned at pi4-regression.spec:861)
  ❌ REQUIRE    \[chrome-truth\] verdict wins=[0-9]+ hits=[0-9]+/[0-9]+ .*-> PASS
       0 hits — MISSING (pinned at pi4-regression.spec:903)
  ❌ REQUIRE    \[chrome-truth\] defers=[0-9]+ budget=[0-9]+ exhausted=false
       0 hits — MISSING (pinned at pi4-regression.spec:904)
  ✅ REQUIRE    \[wc-g\] win=.* fbbad=.* slow=.* ->
       24 hit(s), first @ line 624
  ✅ REQUIRE    \[wc-g\] rollup win=.* scope=window .*frame_us=.* ->
       3 hit(s), first @ line 670
  ✅ REQUIRE    \[wc-h\] win=.* compose_us=.* present_us=.* torn=.* -> BUFFERED
       40 hit(s), first @ line 626
  ✅ REQUIRE    \[wc-h\] win=.* staged=no reason=fixture -> DIRECT
       1 hit(s), first @ line 635
  ✅ REQUIRE    \[wc-h\] rollup win=.* scope=window .*declines=.* -> TEAR-FREE
       14 hit(s), first @ line 664
  ✅ REQUIRE    \[cursor3\] rollup scope=.* planned=.* offers=.* taken=.* adopt=.* repaint=.* ensure=.* stale=.* ->
       1 hit(s), first @ line 1179
  ✅ REQUIRE    \[cursor5\] rollup scope=.* stale_compose=.* adopt_incoh=.* selfsave=.* masked_nosession=.* drain_insession=.* ->
       1 hit(s), first @ line 1180
  ✅ REQUIRE    \[cursor6\] rollup scope=.* present_over=.* masked=.* desktop_over=.* mismatch=.* uncover_lost=.* ->
       1 hit(s), first @ line 1181
  ✅ REQUIRE    \[wc-j\] vacate close_painted=true close_desktop=true .* owner_painted=true owner_desktop=true .* -> PASS
       1 hit(s), first @ line 1093
  ✅ REQUIRE    \[wc-j\] retile survivor=.* moved=true painted=true live=true old_desktop=true .* -> PASS
       1 hit(s), first @ line 1104
  ✅ REQUIRE    \[wc-j\] move-once .* painted=true moved=true old_desktop=true .* new_window=true .* flash_px=0 exact=true -> PASS
       1 hit(s), first @ line 1174
  ✅ REQUIRE    \[wc-j\] vacate .*-> \w+ close_covered=[0-9]+ owner_covered=[0-9]+
       1 hit(s), first @ line 1093
  ✅ REQUIRE    \[wc-j\] retile .*-> \w+ covered=[0-9]+
       1 hit(s), first @ line 1104
  ✅ REQUIRE    \[wc-j\] move-once .*-> \w+ covered=[0-9]+
       1 hit(s), first @ line 1174
  ✅ REQUIRE    \[wc-k\] erase box=.* staged=yes .* contig=yes .* torn=.* -> BUFFERED
       4 hit(s), first @ line 710
  ✅ REQUIRE    \[wc-k\] rollup scope=fills samples=.* noncontig=.* declines=.* outside=0 .* -> TEAR-FREE
       1 hit(s), first @ line 860
  ✅ REQUIRE    \[wc-k\] erase box=.* staged=defer reason=route requeued=no -> DEFERRED
       4 hit(s), first @ line 709
  ✅ REQUIRE    \[pstrip\] armed cores=[0-9]+ panel=\([0-9]+,[0-9]+,[0-9]+x[0-9]+\) row_h=[0-9]+ bar=\(x=[0-9]+,w=[0-9]+\) leds=[0-9]+ led=[0-9]+x[0-9]+ gap=[0-9]+ bands=[0-9]+ full=[0-9]+ strip_h=[0-9]+ reserved=[0-9]+
       1 hit(s), first @ line 1406
  ✅ REQUIRE    \[pstrip\] armed .*full=1000
       1 hit(s), first @ line 1406
  ✅ REQUIRE    \[pstrip\] rollup samples=[0-9]+ redraws=[0-9]+ skipped=[1-9]
       4 hit(s), first @ line 2556
  ✅ REQUIRE    \[pstrip\] src live=[1-9][0-9]*/[0-9]+ quantum=[0-9]+ stepres=([0-9]+px|coarse) mono=(yes|no) (PASS|FAIL|SKIP-GEOM)
       2 hit(s), first @ line 1408
  ✅ REQUIRE    \[pstrip\] rollup .* srcdelta=[0-9]+ rate=
       4 hit(s), first @ line 2556
  ❌ REQUIRE    \[spinhunt\] SYS_EXIT asid=.* 2 sibling thread\(s\) left unjoined; orphan-reap armed
       0 hits — MISSING (pinned at pi4-regression.spec:1664)
  ✅ REQUIRE    \[spinhunt\] load orphan-window\(leader gone\)
       1 hit(s), first @ line 1789
  ✅ REQUIRE    \[spinhunt\] load settled
       1 hit(s), first @ line 1856
  ✅ REQUIRE    SPINHUNT: leader exited status=0 with 2 un-joined yield-polling workers .* drained to 0 live tasks PASS
       1 hit(s), first @ line 1858
  ✅ REQUIRE    BGSPREAD: 3 bg launches over [0-9]+ online cores -> cores [0-9]+,[0-9]+,[0-9]+ el0min [0-9]+/[0-9]+,[0-9]+/[0-9]+,[0-9]+/[0-9]+ inmin=3 \(want == 3\) PASS
       1 hit(s), first @ line 1873
  ✅ REQUIRE    VUGSPREAD: floor test — tasks=2 cores-used=[2-9] depth=1 tries=[1-9] :: PASS
       1 hit(s), first @ line 212
  ❌ REQUIRE    \[clickroute\] hit-test at .*corner=true close=true closereal=true -> PASS
       0 hits — MISSING (pinned at pi4-regression.spec:1821)
  ✅ REQUIRE    \[clickroute\] hit-test at .*-> \w+ hidden_owner=0x[0-9a-f]+
       1 hit(s), first @ line 1222
  ❌ REQUIRE    \[clickroute\] close=win[0-9]+ asid=[0-9]+ at .* settle=noproc-selftest
       0 hits — MISSING (pinned at pi4-regression.spec:1834)
  ✅ REQUIRE    :: MAILBOX: framebuffer [0-9]+x[0-9]+ pitch=[0-9]+B stride=[0-9]+px base=0x[0-9a-f]+ size=[0-9]+ ::
       1 hit(s), first @ line 1
  ✅ REQUIRE    :: TSTE: midden.dispatch -> PASS ::
       1 hit(s), first @ line 257
  ✅ REQUIRE    :: TSTE: midden.route -> PASS ::
       1 hit(s), first @ line 258
  ✅ REQUIRE    :: TSTE: midden.resolve -> PASS ::
       1 hit(s), first @ line 259
  ✅ REQUIRE    :: TSTE: midden.precedence -> PASS ::
       1 hit(s), first @ line 261
  ✅ REQUIRE    :: TSTE: shell\.relics\.mv -> PASS ::
       1 hit(s), first @ line 310
  ✅ REQUIRE    :: TSTE: layout\.mv -> PASS ::
       1 hit(s), first @ line 366
  ✅ REQUIRE    :: TSTE: vfsroute\.route -> PASS ::
       1 hit(s), first @ line 329
  ✅ REQUIRE    :: TSTE: shell\.relics\.mv\.xvol -> PASS ::
       1 hit(s), first @ line 314
  ✅ REQUIRE    :: TSTE: vfs.aclsym.dir -> PASS ::
       1 hit(s), first @ line 300
  ✅ REQUIRE    :: (?:TSTE: vfs.aclsym -> PASS|aclsym: .*vfs.aclsym skipped) ::
       1 hit(s), first @ line 367
  ✅ REQUIRE    \[paper\] kit=us-crispy-modern@0787ba9f algo=laid octaves=3 scale=4 amp_q16=1311 seed=0xfbb60e9f base=0xf5f2ea tile=352x64 hash=0x0df2b838251069dc
       1 hit(s), first @ line 839
  ✅ REQUIRE    :: PAPER: kit texture .* :: PASS ::
       1 hit(s), first @ line 1654
  ✅ REQUIRE    \[ceramic\] derived=peter-2026-08-09 algo=brushed-1d grain_oct=2 pitch=2 grain_amp_q16=786 curve_amp_q16=524 ctrl_gain_q16=32768 seed=0x75ae10b7 rows=128 hash=0x2c525bfdb49df67d
       1 hit(s), first @ line 621
  ✅ REQUIRE    :: CERAMIC: brushed material .* :: PASS ::
       1 hit(s), first @ line 1656
  ❌ REQUIRE    \[knurl\] derived=peter-2026-08-09 algo=crosshatch-2x45 pitch=4 amp_a_q16=656 amp_b_q16=655 budget_q16=1311 box=24 tile=4x4 hash=0x56957202a422b4b1
       0 hits — MISSING (pinned at pi4-regression.spec:2180)
  ✅ REQUIRE    :: KNURL: crosshatch material .* :: PASS ::
       1 hit(s), first @ line 1658
  ✅ REQUIRE    :: KNURL: .* paper=0x0df2b838251069dc ceramic=0x2c525bfdb49df67d unchanged
       1 hit(s), first @ line 1658
  ✅ REQUIRE    :: TERMRING: transport ring slots=64 len=240 .* :: PASS ::
       1 hit(s), first @ line 2378
  ❌ REQUIRE    :: WMCTRL: controls-declined — floor=151 .* furniture close=true minzoom=true packed=true/Some.* silent=true reaped=true :: PASS ::
       0 hits — MISSING (pinned at pi4-regression.spec:2302)
  ✅ REQUIRE    :: STOR2-MV: .* :: PASS \[w=0x3f/0x3f\] ::
       1 hit(s), first @ line 2887
  ✅ REQUIRE    :: LFNMV: sys_rename_long=-EINVAL shell_mv_long=ok alias_leak=false readback=ok \(aarch64: .* -> PASS ::
       1 hit(s), first @ line 2896
  ✅ COUNT>=26  :: (?:(?:[0-9A-DF-RT-Za-z_\-]|E[0-9A-KM-WYZa-z_\-]|EL[0-9A-EG-Za-z_\-]|ELF[02-9A-Za-z_\-]|ELF1[0-9A-Za-z_\-]|EX[0-9A-DF-Za-z_\-]|EXE[0-9ABD-Za-z_\-]|EXEC[02-9A-Za-z_]|EXEC\-[0-9A-TV-Za-z_\-]|EXEC\-U[0-9A-UW-Za-z_\-]|EXEC\-UV[0-9A-TV-Za-z_\-]|EXEC\-UVU[0-9A-FH-Za-z_\-]|EXEC\-UVUG[0-9A-Za-z_\-]|EXEC1[0-9A-Za-z_\-]|S[0-9A-DF-Za-z_\-]|SE[0-9A-QS-Za-z_\-]|SER[0-9A-VX-Za-z_\-]|SERW[0-9A-HJ-Za-z_\-]|SERWI[0-9A-SU-Za-z_\-]|SERWIT[0-9A-Za-z_]|SERWIT\-[013-9A-Za-z_\-]|SERWIT\-2[0-9A-Za-z_\-])[A-Za-z0-9_-]*|E|EL|ELF|EX|EXE|EXEC|EXEC\-|EXEC\-U|EXEC\-UV|EXEC\-UVU|S|SE|SER|SERW|SERWI|SERWIT|SERWIT\-): .*-> PASS ::
       129 hit(s), first @ line 46
  ✅ COUNT>=6   CAPSTONE \w+: PASS
       6 hit(s), first @ line 186
  ✅ PENDING    \[taskexit\] tid=\d+ name='[^']+' core=\d+ reason=(exit|killed)
       60 hit(s), first @ line 81 — MATCHED: consider promoting to REQUIRE
  ✅ OPTIONAL   K1-atr:.*codec PASS
       1 hit(s), first @ line 739
  ❌ FORBID*    -> FAIL
       7 hit(s), first @ line 840: [chrome-truth] verdict wins=2 hits=0/10 title_grad=209 ceramic_pp=8 knurl_pp=9 paper_pp=8 paper_on=no panel=640x480 -> FAIL
  ❌ FORBID*    FAIL ::
       2 hit(s), first @ line 683: :: SHARD-PRESS: menu=242x247+0+34 panel=640x480 routed=true(open) opened=true painted=false dismissed=true(dismiss) closed=true erased=true :: FAIL ::
  ✅ FORBID*    PANIC
       0 hits
  ✅ FORBID*    -> FLICKER
       0 hits
  ❌ FORBID     :: SERWIT-2: FAIL —
       1 hit(s), first @ line 1317: :: SERWIT-2: FAIL — balanced=true evidence_lost=10 ::
  ✅ FORBID     :: BOT-PARK: selftest .*-> FAIL ::
       0 hits
  ✅ FORBID     :: ERET-SCRUB: first-entry residue or=
       0 hits
  ✅ FORBID     :: ERET-SCRUB: syscall-return residue bitmap=
       0 hits
  ✅ FORBID     :: ERET-SCRUB: witness entries not latched
       0 hits
  ✅ FORBID     \[u7fix\] .*child parked [0-9]+ms before GO \(parked_out=[1-9]
       0 hits
  ✅ FORBID     \[u7fix\] .*parent parked [0-9]+ms before GO \(parked_out=true
       0 hits
  ✅ FORBID     \[u11fix\] .*B parked [0-9]+ms before unlink-GO \(parked_out=[1-9]
       0 hits
  ✅ FORBID     \[u11fix\] .*before read-GO \(parked_out=true
       0 hits
  ✅ FORBID     \[u11fix\] .*before close-GO \(parked_out=true
       0 hits
  ✅ FORBID     R1 error status
       0 hits
  ✅ FORBID     programming-busy timeout
       0 hits
  ✅ FORBID     AARCH64 EXCEPTION
       0 hits
  ◦ FORBID     K9-parity:.*discarded residue FAIL
       0 hits — UNREACHABLE: its literal "discarded residue FAIL" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     K4-write:.*FAIL
       0 hits
  ✅ FORBID     K8a-cow:.*FAIL
       0 hits
  ✅ FORBID     K8b-snap:.*FAIL
       0 hits
  ✅ FORBID     K8c-snapread:.*FAIL
       0 hits
  ✅ FORBID     IMG-SIG:.*FAIL
       0 hits
  ✅ FORBID     FATDIRS:.*FAIL
       0 hits
  ✅ FORBID     FATMOVE:.*FAIL
       0 hits
  ✅ FORBID     K6-migrate:.*FAIL
       0 hits
  ❌ FORBID     BANDY-CODEC:.*FAIL
       1 hit(s), first @ line 2721: :: BANDY-CODEC: FAIL — w=0x2f/0x3f ::
  ❌ FORBID     BANDY-CODEC2:.*FAIL
       1 hit(s), first @ line 2724: :: BANDY-CODEC2: FAIL — w=0x2f/0x3f ::
  ✅ FORBID     BANDY-STAMP:.*FAIL
       0 hits
  ✅ FORBID     BANDY-RT:.*FAIL
       0 hits
  ✅ FORBID     BANDY-EQ:.*FAIL
       0 hits
  ✅ FORBID     BANDY-WR:.*FAIL
       0 hits
  ✅ FORBID     BANDY-EQ2:.*FAIL
       0 hits
  ✅ FORBID     BANDY-ACL:.*FAIL
       0 hits
  ✅ FORBID     BANDY-GRANT:.*FAIL
       0 hits
  ✅ FORBID     USER: input router.*FAIL
       0 hits
  ✅ FORBID     \[wc-d\] verify .*-> FAIL
       0 hits
  ✅ FORBID     \[wc-b\] rollup .* amp=(?:$|[^1]|1(?:$|[^.]|\.(?:$|[^0]|0(?:$|[^0]|0(?:$|[^x])))))
       0 hits
  ✅ FORBID     :: \[wedge1\] DRAIN ABANDONED
       0 hits
  ✅ FORBID     \[wedge1\] dwell .*-> ABANDONED
       0 hits
  ✅ FORBID     \[wedge1\] dwell .*abandoned=[1-9]
       0 hits
  ❌ FORBID     \[wc-fv\] focus-vis .*-> FAIL
       1 hit(s), first @ line 1032: <F6>[wc-fv] focus-vis at (214,160) a=0xff2020 b=0x20ff20 stack=0x060608/false raise=0x060608/false shell=0x060608/false reraise=0x060608/false -> FAIL
  ✅ FORBID     BGRUN-ST: .*-> FAIL
       0 hits
  ✅ FORBID     KILLBOUND: .*-> FAIL
       0 hits
  ✅ FORBID     \[wc-e\] fb-geometry query FAILED
       0 hits
  ✅ FORBID     \[wc-f\] scanout .*-> FAIL
       0 hits
  ✅ FORBID     \[wc-f\] scanout -> SKIP
       0 hits
  ✅ FORBID     \[wc-f\] twin .*-> FAIL
       0 hits
  ✅ FORBID     \[wc-f\] twin -> SKIP
       0 hits
  ❌ FORBID     \[chrome-truth\] defers=.*exhausted=true
       1 hit(s), first @ line 825: [chrome-truth] defers=33 budget=32 exhausted=true (contested passes skipped before this verdict)
  ◦ FORBID     \[wc-g\] .*-> COHER
       0 hits — UNREACHABLE: its literal "-> COHER" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     \[wc-g\] .*-> RACE
       0 hits
  ❌ FORBID     \[wc-g\] .*-> BLIT
       8 hit(s), first @ line 909: <F7><F8>[wc-g] win=4 seq=0 own=no scale=1x app=0xcbf29ce484222325 blit=0x747c14df278b9c45 civac=0x747c14df278b9c45 after=0x747c14df278b9c45 fbbad=12852/108864 occluded=0 occ=0/0 us=3590 rectscan_us=10000 slow=no -> BLIT
  ✅ FORBID     \[wc-h\] .*presspread=[0-9] presspop=([2-9]|[0-9]{2,}) .*-> AT-RISK
       0 hits
  ◦ FORBID     \[wc-h\] .*-> UNSTAGED
       0 hits — UNREACHABLE: its literal "-> UNSTAGED" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ◦ FORBID     \[cursor5\] .*-> REGRESSED
       0 hits — UNREACHABLE: its literal "-> REGRESSED" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     \[cursor5\] .*drain_insession=[1-9]
       0 hits
  ✅ FORBID     \[wc-j\] .*-> FAIL
       0 hits
  ✅ FORBID     \[wc-k\] .*staged=no
       0 hits
  ✅ FORBID     \[wc-k\] .*-> DIRECT
       0 hits
  ✅ FORBID     \[wc-k\] .*contig=no
       0 hits
  ◦ FORBID     \[wc-k\] .*-> SPLIT
       0 hits — UNREACHABLE: its literal "-> SPLIT" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     \[wc-k\] .*-> AT-RISK
       0 hits
  ◦ FORBID     \[wc-k\] .*-> UNSTAGED
       0 hits — UNREACHABLE: its literal "-> UNSTAGED" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     \[wc-k\] .*-> LOST
       0 hits
  ✅ FORBID     \[wc-k\] .*-> STARVED
       0 hits
  ◦ FORBID     \[wc-k\] .*-> UNPUBLISHED
       0 hits — UNREACHABLE: its literal "-> UNPUBLISHED" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     \[wc-k\] .*outside=[1-9]
       0 hits
  ✅ FORBID     \[pstrip\] armed .*panel=\([0-9]+,[0-9]+,0x[0-9]+\)
       0 hits
  ✅ FORBID     \[pstrip\] armed .*row_h=0
       0 hits
  ✅ FORBID     \[pstrip\] armed .*leds=[0-9] led=
       0 hits
  ✅ FORBID     \[pstrip\] rollup .*\bsamples=[1-9][0-9]*\b.*\bskipped=0\b.*\bsrcdelta=0\b
       0 hits
  ✅ FORBID     \[pstrip\] src live=0/
       0 hits
  ✅ FORBID     \[pstrip\] src .*stepres=0px
       0 hits
  ✅ FORBID     \[pstrip\] src .*mono=no
       0 hits
  ✅ FORBID     \[pstrip\] src .*FAIL
       0 hits
  ✅ FORBID     \[pstrip\] src .*SKIP-ARCH
       0 hits
  ✅ FORBID     \[pstrip\] rollup .*rate=([6-9]|[1-9][0-9]+)\.[0-9]/s
       0 hits
  ✅ FORBID     SPINHUNT: .*-> FAIL
       0 hits
  ✅ FORBID     REFUSING corrupt switch-in: task=
       0 hits
  ✅ FORBID     BGSPREAD: .*-> FAIL
       0 hits
  ✅ FORBID     VUGSPREAD: floor test .*:: FAIL
       0 hits
  ✅ FORBID     \[spread2\] .* ratio ([5-9][0-9]{2}|[0-9]{4,})
       0 hits
  ✅ FORBID     \[clickroute\] close=.* settle=noproc$
       0 hits
  ✅ FORBID     \[clickroute\] hit-test teardown LEAK
       0 hits
  ◦ FORBID     :: MAILBOX: timeout waiting for write FIFO
       0 hits — UNREACHABLE: its literal ":: MAILBOX: timeout waiting for write FIFO" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ◦ FORBID     :: MAILBOX: timeout \(only other-channel replies\)
       0 hits — UNREACHABLE: its literal ":: MAILBOX: timeout (only other-channel replies)" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     :: MAILBOX: timeout .* drained [1-9]
       0 hits
  ✅ FORBID     :: MAILBOX: BUSY
       0 hits
  ✅ FORBID     M6b: EL0 fault isolation FAIL
       0 hits
  ✅ FORBID     M6g: disk-loaded EL0 program FAIL
       0 hits
  ✅ FORBID     U4: process model FAIL
       0 hits
  ✅ FORBID     U5: capabilities FAIL
       0 hits
  ✅ FORBID     U6: general object table FAIL
       0 hits
  ✅ FORBID     U6b: real File handles FAIL
       0 hits
  ✅ FORBID     U7: cross-process transfer FAIL
       0 hits
  ✅ FORBID     U8: revocation trees FAIL
       0 hits
  ✅ FORBID     U9: real File writes FAIL
       0 hits
  ✅ FORBID     U10: file growth FAIL
       0 hits
  ✅ FORBID     U10-create: file create FAIL
       0 hits
  ✅ FORBID     U10-delete: file delete FAIL
       0 hits
  ✅ FORBID     U11: open-file lifecycle FAIL
       0 hits
  ✅ FORBID     U11-defer: cross-process unlink-defers-free FAIL
       0 hits
  ✅ FORBID     U11-reuse: .*orphan-head sweep FAIL
       0 hits
  ✅ FORBID     U11-reap: teardown-last-close reaper FAIL
       0 hits
  ✅ FORBID     U6-grants: owner/grants FAIL
       0 hits
  ✅ FORBID     K1-persist:.*rebuild\+enforce FAIL
       0 hits
  ◦ FORBID     K1-corrupt:.*at boot FAIL
       0 hits — UNREACHABLE: its literal "at boot FAIL" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     F2-witness:.*-> SERIALIZATION REGRESSION
       0 hits
  ✅ FORBID     F3-witness:.*-> SERIALIZATION REGRESSION
       0 hits
  ✅ FORBID     K2-liveenf:.*rebuild\+enforce FAIL
       0 hits
  ◦ FORBID     K3-revoke:.*durable-first FAIL
       0 hits — UNREACHABLE: its literal "durable-first FAIL" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ❌ FORBID     K3-mount:.*byte-verified FAIL
       1 hit(s), first @ line 2399: :: K3-mount: native unafs volume located (base_lba=114688, 2048 blocks) + superblock v7 mounted + ls/cat byte-verified FAIL [w=0x1df] ::
  ✅ FORBID     K3-mount: located but mount FAILED
       0 hits
  ◦ FORBID     K4-ready:.*prefix\) FAIL
       0 hits — UNREACHABLE: its literal "prefix) FAIL" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     :: UVUG: frames=[0-9]+ threads=[0-9]+ checksum=(?:$|[^0]|0(?:$|[^x]|x(?:$|[^f]|f(?:$|[^1]|1(?:$|[^8]|8(?:$|[^f]|f(?:$|[^9]|9(?:$|[^8]|8(?:$|[^3]|3(?:$|[^5]|5(?:$|[^5]|5(?:$|[^7]|7(?:$|[^b]|b(?:$|[^8]|8(?:$|[^7]|7(?:$|[^a]|a(?:$|[^5]|5(?:$|[^5]))))))))))))))))))
       0 hits
  ✅ FORBID     \[inroute\] router window — (?:$|[^r]|r(?:$|[^o]|o(?:$|[^u]|u(?:$|[^t]|t(?:$|[^e]|e(?:$|[^d]|d(?:$|[^=]|=(?:$|[^2]|2(?:$|[^ ]| (?:$|[^s]|s(?:$|[^t]|t(?:$|[^a]|a(?:$|[^l]|l(?:$|[^e]|e(?:$|[^_]|_(?:$|[^d]|d(?:$|[^r]|r(?:$|[^o]|o(?:$|[^p]|p(?:$|[^p]|p(?:$|[^e]|e(?:$|[^d]|d(?:$|[^=]|=(?:$|[^1]|1(?:$|[^ ]| (?:$|[^r]|r(?:$|[^e]|e(?:$|[^v]|v(?:$|[^o]|o(?:$|[^k]|k(?:$|[^e]|e(?:$|[^s]|s(?:$|[^=]|=(?:$|[^0]))))))))))))))))))))))))))))))))))
       0 hits
  ✅ FORBID     \[wc-c\] side-by-side (?:$|[^w]|w(?:$|[^i]|i(?:$|[^n]|n(?:$|[^d]|d(?:$|[^o]|o(?:$|[^w]|w(?:$|[^s]|s(?:$|[^=]|=(?:$|[^2]|2(?:$|[^ ]| (?:$|[^d]|d(?:$|[^r]|r(?:$|[^a]|a(?:$|[^w]|w(?:$|[^n]|n(?:$|[^=]|=(?:$|[^2])))))))))))))))))
       0 hits
  ❌ FORBID     BGRUN-ST: process table capacity = (?:$|[^6]|6(?:$|[^ ]| (?:$|[^r]|r(?:$|[^o]|o(?:$|[^w]|w(?:$|[^s]))))))
       1 hit(s), first @ line 1881: :: BGRUN-ST: process table capacity = 12 rows (bg programs alive at once; EL0 slots=heap, R90) ::
  ✅ FORBID     \[wc-e\] fb-geometry .*row_ok=false
       0 hits
  ✅ FORBID     \[wc-e\] fb-geometry .*fit_ok=false
       0 hits
  ◦ FORBID     \[cursor3\] .*-> INCOHERENT
       0 hits — UNREACHABLE: its literal "-> INCOHERENT" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ◦ FORBID     \[cursor6\] .*-> OVERWRITTEN
       0 hits — UNREACHABLE: its literal "-> OVERWRITTEN" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     \[pstrip\] armed .*full=(?:[02-9]|1(?:$|[^0]|0(?:$|[^0]|0(?:$|[^0]|0(?:$|[^ ])))))
       0 hits
  ◦ FORBID     \[spinhunt\] SYS_EXIT .*orphan-reap NOT ARMED
       0 hits — UNREACHABLE: its literal "orphan-reap NOT ARMED" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ◦ FORBID     :: TSTE: midden\.\w+ -> FAIL
       0 hits — UNREACHABLE: its literal ":: TSTE: midden." has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ◦ FORBID     :: TSTE: vfs\.aclsym[\w.]* -> FAIL
       0 hits — UNREACHABLE: its literal ":: TSTE: vfs.aclsym" has 0 hits in pi_baremetal/ (a check that cannot fire is an absent one, LAWS §5)
  ✅ FORBID     aclsym: .*vfs\.aclsym skipped
       0 hits
  ✅ FORBID     :: PAPER: .* :: FAIL ::
       0 hits
  ✅ FORBID     :: CERAMIC: .* :: FAIL ::
       0 hits
  ✅ FORBID     :: KNURL: .* :: FAIL ::
       0 hits
  ✅ FORBID     :: WMCTRL: .* :: FAIL ::
       0 hits
  ❌ FORBID     \[dragperf\] .* -> FAIL
       1 hit(s), first @ line 1503: [dragperf] panel=640x480 box=330x60 extent_speedup=7.7x admitted=19 coalesced=0 -> FAIL
  ❌ FORBID     \[dragwedge\] .* -> FAIL
       1 hit(s), first @ line 1645: [dragwedge] furniture aimed=false chrome=false grab=false release=false | stall began=true gave_up=true cancelled=true ms=868/2500 | refuse=true ms=0/50 | recover=true bound=33554432 | arm_yield=true arm_skip=true skip_ms=38/400 tdbound=1073741824 | rescue=true rescued=1 unowned=1 -> FAIL
  ✅ FORBID     \[dragwedge\] .* arm_skip=false
       0 hits
  ✅ FORBID     \[dragwedge\] .* rescue=false
       0 hits
  ❌ FORBID     :: SHARD-PRESS: .* :: FAIL ::
       1 hit(s), first @ line 683: :: SHARD-PRESS: menu=242x247+0+34 panel=640x480 routed=true(open) opened=true painted=false dismissed=true(dismiss) closed=true erased=true :: FAIL ::
  ✅ FORBID     \[menubar\] .* press=inert
       0 hits
  ✅ FORBID     \[serfocus\] split .* :: FAIL ::
       0 hits
  ✅ FORBID     :: QUARRY: .* :: FAIL ::
       0 hits
  ✅ FORBID     :: STOR2-MV: aarch64 SYS_RENAME FAIL
       0 hits
  ✅ FORBID     :: LFNMV: SKIPPED
       0 hits
  ✅ FORBID     :: LFNMV: .* -> FAIL ::
       0 hits
  ─────
  ❌ MBENCH FAIL — 112/128 required witnesses, 27 forbidden hit(s), 6500 lines scanned, pending 1/1 matched, 17 unreachable FORBID(s) [full wall 60.5s]
       FIRST-SHORTFALL pi4-regression.spec:265 REQUIRE :: SERWIT-2: mirror taps .*-> PASS ::
       (15 further pinned line(s) also short — full table above)
       (the end-of-run marker was seen — the run completed, so a missing witness here is a GENUINE regression)
  (* = default FORBID, always on)
```
