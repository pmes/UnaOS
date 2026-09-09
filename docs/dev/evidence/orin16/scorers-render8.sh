#!/usr/bin/env bash
# scorers-render8.sh — score the render8 flight the minute the boot ends.
#
#   usage: scorers-render8.sh <boot-log-file> [burstA burstB pacedA pacedB]
#
# BUILT FROM docs/dev/evidence/orin16/scorers-render7.sh, verbatim through the A17 block,
# then the render8 additions: A21 tick (orin14/bsptick/score-tick1.sh, TICK1-FLIGHT.md §C),
# A21 run (BSPRUN, orin16/bsprun/PROGRESS.md §5), A12/NET-5 + the retained net4 readings
# (net4-20260906T0138Z MANIFEST Q0-Q3, orin16/net4b/PROGRESS.md §5), A24 GA10B rungs 3+3b
# (orin16/ga10b3/PROGRESS.md §7), A28 ROOTFS, and then a REAL scorer for every fold of the
# round: A37 rxmerge, A34/SO4 crystal, SR2/A36 prtscr3, SO1/A29 winid+winid2 (+ the S4
# drain), A10/SO2/SO3 menubar2, V-1..V-4 fixpanel, and a PENDING-FOLD block for KEYDOORS-FIX.
#
# NO TOKEN-TBD STUB REMAINS. Every fold landed on hw-jetson by 8b696271, so every pattern
# below was copied from SOURCE at that tip with `git grep -n` and carries its file:line.
# Each has a CAN-FIRE PROOF in stage8/scorers-render8-selftest.out — run against
# render7-boot1.log, where the token is absent and the scorer MUST say ABSENT/NOT-SCORED,
# and against synthetic lines carrying the token, because most of these have never flown.
# ("A check that cannot fire": printing is not gating, and a zero-hit result indicts the
# pattern before it indicts the boot.)
#
# <boot-log-file>  the flown bytes of THIS boot, NUL-stripped (see MAKE, below).
# burstA..pacedB   per-leg line windows into the SAME file for the A16 injection
#                  legs (burst = (burstA,burstB], paced = (pacedA,pacedB]).
#                  Omit them and the per-leg block says NOT-SCORED instead of
#                  inventing a window.
#
# MAKE the input (control bytes break grep AND awk's line handling):
#   tr -d '\000' < ~/unaos-bench/capture/line-acm0/orin.log | awk 'NR>$MARK' > boot.log
#
# Every scorer is ONE awk over the log and prints ONE line ending in `-> VERDICT`.
# The render6 scorers are carried over verbatim in shape; the render7 additions
# follow them, each with the source file:line of the format string it matches.
# CITATIONS: render7's numbers were read at 37c78ad7 and the round's folds moved many of
# them — every citation touched by a fold (A17/prtscr, A25/winmenu, and all the render8
# additions) has been RE-DERIVED at hw-jetson 8b696271 and says so inline.
#
# THE QUESTIONS (render8 MANIFEST, assembled from stage8/QUESTIONS-render8.md):
#   A15 A16/TCURX2 A27 A8 A25/R21 A26 A20 A17 A1   (render7 carry-overs)
#   A21-tick A21-run A12/NET-5 A28 A24-rung3 A24-rung3b   (the knobbed additions)
#   A37 A34/SO4 A30/SO5/A38 SR2/A36 SO1/A29 A10/SO2/SO3 V-1..V-4   (the knob-less folds)
#   PENDING-FOLD: KEYDOORS-FIX (not aboard; scored as a recorded baseline, never a failure)

set -u
B="${1:?usage: scorers-render7.sh <boot-log-file> [burstA burstB pacedA pacedB]}"
L0="${2:-}"; L1="${3:-}"; L2="${4:-}"; L3="${5:-}"
[ -r "$B" ] || { echo "scorers-render8: cannot read $B" >&2; exit 2; }

echo "== render8 scorers over $B ($(wc -l < "$B") lines) =="

# ---------------------------------------------------------------- A15 (SMP) --
# ORIN-SMP-3 CPU_ON AP 1..5 -> SUCCESS x5. Unchanged from render6.
awk '/ORIN-SMP-3 CPU_ON AP [0-9]+ .*-> SUCCESS/{s++} /CPU_ON AP [0-9]+ .*-> ERROR/{e++} /[0-9]+\/[0-9]+ secondaries online via PSCI CPU_ON/{o++} /Exception reason=1 syndrome=0x82000010/{x++} /Powering off core/{p++}
     END{printf "A15 smp: cpu_on_success=%d cpu_on_error=%d el3_abort=%d poweroff=%d online_line=%d -> %s\n",s,e,x,p,o,(s==5&&!e&&!x&&!p&&o)?"PASS (A15 pass 5)":(x||p)?"FAIL A15-signature (AP died in its MMU-off window)":"NO-VERDICT"}' "$B"

# ------------------------------------------------------------ A1 (u7stk hw) --
# `[u7stk] at={} task={}:{} sp={:#x} low={:#x} top={:#x} len={} used={} hw={} headroom={}`
#   -- arch/aarch64/sched.rs:10650 (boot-core arm; the task-N arm is sched.rs:134).
# render7 asks: activate() now opens the quarry, so compare hw= against render6's 15552.
awk '/\[u7stk\] at=boot-core:post-cascade/{n++; for(i=1;i<=NF;i++){if($i~/^hw=/){sub(/hw=/,"",$i);hw=$i+0} if($i~/^headroom=/){sub(/headroom=/,"",$i);hr=$i+0} if($i~/^len=/){sub(/len=/,"",$i);len=$i+0}}} /\[deskcascade\] arming/{a++}
     END{d=hw-15552; printf "A1 u7stk: arming=%d post_cascade=%d len=%d hw=%d (render6=15552 delta=%+d) headroom=%d -> %s\n",a,n,len,hw,d,hr,
       n?((hr>0)?((d>0)?"PASS-DEEPER (quarry cost "d" bytes; still unsaturated)":(d<0)?"PASS-SHALLOWER ("(-d)" bytes below render6 — check the quarry actually opened)":"PASS-EQUAL (hw unchanged at 15552 — did activate() reach the quarry?)"):"SATURATED — widen the boot-core window"):(a?"NO post-cascade probe after arming (overflow: the §5.2 stop-line case)":"NO CASCADE ARMED")}' "$B"
awk '/\[u7stk\] at=boot-core:(pre|post)-cascade/' "$B" | cut -c1-160

# --------------------------------------------- A18 carry-over (cascade/strip) --
awk '/\[deskcascade\] -> CASCADED/{c++} /\[deskcascade\] REFUSE/{r++; rr=$0} /\[pulsewin\] open win=/{pw++} /\[pulsewin\] open DECLINE/{pd++} /\[orinrender\] strip=kept/{sk++}
     /\[orinrender\] census/{if(match($0,/strip=[a-z]+/))st=substr($0,RSTART+6,RLENGTH-6); if(match($0,/pulsewin=[0-9]+/))pv=substr($0,RSTART+9,RLENGTH-9)}
     END{printf "A18 cascade: cascaded=%d refuse=%d pulsewin_open=%d pulsewin_decline=%d strip_kept=%d census_strip=%s census_pulsewin=%s -> %s\n",c,r,pw,pd,sk,(st?st:"n/a"),(pv?pv:"n/a"),
     (c&&pw&&!pd&&!sk&&st=="retired")?"PASS":(c&&pd)?"FAIL pulsewin DECLINE":(c&&!pw)?"FAIL no [pulsewin] open on the cascaded scene":(sk||st=="live")?"FAIL strip still live":(r?"NO-CASCADE: "substr(rr,1,80):"NO-VERDICT")}' "$B"
awk '/\[pulsewin\] open|\[orinrender\] strip=|\[deskcascade\] ->/' "$B" | cut -c1-160

# --------------------------------------------------------- A19 (band/shell) --
awk '/\[realdesk\] band-cleared/{b++; bl=$0} /\[realdesk\] shell-present/{s++} /\[u7stk\] at=jd2-console:shell-present/{u++}
     END{printf "A19 band: band_cleared=%d shell_present=%d jd2_probe=%d -> %s\n",b,s,u,(b&&s)?"WIRE PASS (now A19-pngband.py SCREEN0.PNG must read non-bg=0/60200)":"FAIL A19 wire"}' "$B"

# ------------------------------------------------------ A20 clicks (render6) --
awk '/\[orinclick\] arm .*-> ARMED/{a++} /\[orinrender\] arm .*click=1/{c1++} /\[clickroute\] press/{p++; pl=$0} /\[orinclick\] edge=press.*-> CONSUMED/{k++} /\[orinrender\] census.*-> ROUTING/{rt++}
     END{printf "A20 clicks: arm_click1=%d orinclick_armed=%d clickroute_press=%d consumed=%d routing_census=%d -> %s\n",c1,a,p,k,rt,(a&&p&&k)?"PASS (A20 flown)":(a&&!p)?"ARMED, NO CLICK SEEN (press not made or not routed)":(!a)?"NOT ARMED (knob missing?)":"NO-VERDICT"; if(pl) print "   " substr(pl,1,150)}' "$B"

# --------------------------------------------------- A22 TCU mailbox (rung 1) --
# `[tcu] rx-mbox raw={:#010x} full={} {} | census={} polls={} full-edges={} changes={} last-full={:#010x} -> {}`
#   -- arch/aarch64/hsp_tegra.rs:310   (arm sample: hsp_tegra.rs:234)
awk '/\[tcu\] hsp top0=/{arm++} /\[tcu\] STOP/{stop++} /\[tcu\] rx-mbox/{n++; for(i=1;i<=NF;i++){if($i~/^full=/){sub(/full=/,"",$i);f=$i+0} if($i~/^nbytes=/){sub(/nbytes=/,"",$i);nb=$i+0} if($i~/^full-edges=/){sub(/full-edges=/,"",$i);fe=$i+0} if($i~/^changes=/){sub(/changes=/,"",$i);ch=$i+0}} if(match($0,/data=\[[0-9a-f ]+\]/))dt=substr($0,RSTART,RLENGTH)}
     END{printf "A22 tcu: arm=%d stop=%d census=%d full_final=%d nbytes=%d full_edges=%d changes=%d %s -> %s\n",arm,stop,n,f,nb,fe,ch,dt,(stop)?"STOP at arm (DTB shape)":(f&&nb>=1)?"ROW1: SPE forwards RX into the mailbox and parks it — TCURX rung 2 not consuming":(fe>0&&!f)?"ROW2: FULL-SEEN then consumed":(!fe&&!ch)?"ROW3: FULL-NEVER — forwarding not on unprompted":"NO-VERDICT"}' "$B"

# -------------------------------------------------------- boot-health rollup --
awk '/AARCH64: timer heartbeat live/{h++} /JM6b — EL1 landing: CurrentEL=1/{e++} /\[orinrender\] arm conwin=/{t++} /RENDER-ARMED/{ra++} /RENDER-LIVE/{rl++} /\[redzone\] .*LOW-REDZONE/{rz++} /Exception reason=|panicked at|PANIC:/{x++}
     END{printf "HEALTH: heartbeat=%d el1=%d arm=%d armed=%d live=%d redzone=%d exceptions=%d -> %s\n",h,e,t,ra,rl,rz,x,(h&&e&&t&&ra&&rl&&!rz&&!x)?"PASS":"INCOMPLETE"}' "$B"

echo "-- render7 additions --"

# ============================================================ A16 / TCURX2 ====
# rung 2: the CONSUMER. Three tokens, all new to render7:
#   `[tcurx] took={:#04x} '{}' left={} word={:#010x} <- raw={:#010x} @ {:#x} n={} took-total={} tags={}`
#       -- arch/aarch64/hsp_tegra.rs:391   (per byte taken from the TCU RX mailbox)
#   `[serialrx] rx={} (+{}) polls={} refused={} ovrf={} lsr0={:#010x} mbox={} -> {}`
#       -- arch/aarch64/serial.rs:830      (the `mbox=` field exists ONLY under `tcurx`)
#   `[tcu] rx-mbox ... full={}`
#       -- arch/aarch64/hsp_tegra.rs:310   (must settle to full=0 once the consumer drains)
awk '/\[tcurx\] took=/{t++; if(match($0,/took-total=[0-9]+/))tt=substr($0,RSTART+11,RLENGTH-11)+0}
     /\[serialrx\] rx=/{n++; mbs="ABSENT"; for(i=1;i<=NF;i++){if($i~/^rx=/){sub(/rx=/,"",$i);rx=$i+0} if($i~/^ovrf=/){sub(/ovrf=/,"",$i);ov=$i+0;ovs=1} if($i~/^mbox=/){sub(/mbox=/,"",$i);mb=$i+0;mbs=mb}}}
     /\[tcu\] rx-mbox/{for(i=1;i<=NF;i++)if($i~/^full=/){sub(/full=/,"",$i);f=$i+0}}
     /:: tegra: JD2 — KEY /{k++}
     END{printf "A16 tcurx2: tcurx_took=%d took_total=%s serialrx_census=%d rx_final=%d mbox_final=%s ovrf=%s tcu_full_final=%d keys=%d -> %s\n",t,(tt?tt:0),n,rx,(mbs?mbs:"ABSENT"),(ovs?ov:"ABSENT"),f,k,
       (!n)?"NO [serialrx] CENSUS (orinrx knob absent)":(mbs=="ABSENT")?"mbox= FIELD ABSENT — the tcurx knob is NOT in this image (serial.rs:830 is the cfg-on arm)":(t>0&&mb>0&&f==0)?"PASS rung 2 (the consumer took "t" byte(s) and left the mailbox empty)":(t>0&&f!=0)?"PARTIAL: took="t" but [tcu] full="f" at the end — the SPE re-parked faster than the drain":(!t&&mb==0)?"CONSUMER NEVER FIRED (mbox=0, no [tcurx] took=) — no byte ever reached the mailbox on this boot":"NO-VERDICT"}' "$B"
awk '/\[tcurx\] took=|\[serialrx\] rx=/' "$B" | tail -4 | cut -c1-170

# A16 per-leg KEY count. Windows are (L0,L1] burst and (L2,L3] paced, line numbers
# into THIS file. `:: tegra: JD2 — KEY '{}' ::` -- main.rs:2943 (printable) / 2945 (hex).
if [ -n "$L0" ] && [ -n "$L1" ] && [ -n "$L2" ] && [ -n "$L3" ]; then
  for leg in "burst $L0 $L1" "paced $L2 $L3"; do set -- $leg
    awk -v a="$2" -v b="$3" -v leg="$1" 'NR>a && NR<=b {
         if(/:: tegra: JD2 — KEY /){k++; ks=ks" "$0}
         if(/\[tcurx\] took=/)t++
         if(/\[serialrx\] rx=/){for(i=1;i<=NF;i++){if($i~/^rx=/){sub(/rx=/,"",$i);rx=$i+0} if($i~/^mbox=/){sub(/mbox=/,"",$i);mb=$i+0;mbs=1} if($i~/^ovrf=/){sub(/ovrf=/,"",$i);ov=$i+0}}}}
       END{printf "A16 leg %s: keys=%d tcurx_took=%d rx_after=%d mbox_after=%s ovrf_after=%d -> %s\n",leg,k,t,rx,(mbs?mb:"ABSENT"),ov,
             (k==5)?"PASS 5/5":(k>0)?"PARTIAL "k"/5":"NO KEYS IN WINDOW"; gsub(/:: tegra: JD2 — /,"",ks); if(ks) print "   " ks}' "$B"
  done
else
  echo "A16 leg burst/paced: windows not supplied -> NOT-SCORED (pass burstA burstB pacedA pacedB)"
fi

# ================================================================= A27 DRAG ====
#   `[dragroute] wired panel={}x{} desktop_firmware={} -> READY`
#       -- arch/aarch64/display_tegra.rs:5246
#   `[dragroute] arm win={} gesture={} -> STEERING`
#       -- arch/aarch64/display_tegra.rs:5230
#   `[dragroute] end win={} via={} fed={} applied={} at ({},{}) -> {}`  verdict in
#       {NO-FEED, FED-NO-MOVE, STEERED} -- arch/aarch64/display_tegra.rs:5197 (verdicts 5188-5193)
#   `[wm-act] {} win={} owner={:#x} at ({},{}) -> {}`  -- video/wm.rs:15876,
#       action "drag-end", outcome "placed"/"no-move" -- video/wm.rs:16214/16216
awk '/\[dragroute\] wired /{w++; wl=$0} /\[dragroute\] arm win=/{a++} /\[dragroute\] end /{e++; if(/-> STEERED/)st++; else if(/-> FED-NO-MOVE/)fn_++; else if(/-> NO-FEED/)nf++; el=$0}
     /\[wm-act\] drag-end /{de++; if(/-> placed/)pl++; else if(/-> no-move/)nm++; dl=$0} /\[wm-act\] drag-begin /{db++}
     /\[dragroute\] control-absent/{ctl++}
     END{printf "A27 drag: wired=%d control_absent=%d arm=%d end=%d steered=%d fed-no-move=%d no-feed=%d | wm_drag_begin=%d wm_drag_end=%d placed=%d no-move=%d -> %s\n",w,ctl,a,e,st,fn_,nf,db,de,pl,nm,
       (ctl)?"CONTROL FIRED — the pattern matches a token written nowhere in the tree; the scorer is wrong":
       (!w)?"[dragroute] ABSENT — no pointer frame reached orin_drag_steer (orinclick off, or the pointer is dead: read A20)":
       (w&&!db)?"WIRED, NO GRAB (no [wm-act] drag-begin — the title bar was never pressed)":
       (st>0&&pl>0)?"PASS A27 (steered "st" gesture(s), wm placed "pl")":
       (de>0&&nm>0&&fn_==0&&st==0)?"FAIL A27 render6-SIGNATURE (drag-end -> no-move; DRAG_MOVES never left zero)":
       (fn_>0)?"FED-NO-MOVE: the pump fed motion and wm applied none — the pacer or the geometry, not the feed":
       "NO-VERDICT"; if(wl) print "   " substr(wl,1,140); if(el) print "   " substr(el,1,140); if(dl) print "   " substr(dl,1,140)}' "$B"

# =============================================================== A8 QUARRY ====
#   `[quarry] open win={} surf={}x{} ts={} box={}x{} at ({},{}) volumes={} tree-rows={} list-rows={} cwd={}`
#       -- video/quarry/live.rs:1806
#   `[quarry] DECLINE reason=...` -- video/quarry/live.rs:1706 etc; `[quarry] open SKIP reason=already-open win={}` -- live.rs:1670
#   `[pidesk] quarry open={} — the file manager is a desktop tenant ...`
#       -- video/desktop_firmware.rs:396
#   `[deskquarry] seat compiled={} open={} windows={} relatched={} (QUARRY-ORIN A8 ...)`
#       -- main.rs:8943
awk '/\[quarry\] open win=/{o++; ol=$0} /\[quarry\] open SKIP/{sk++} /\[quarry\] DECLINE reason=/{dc++; dl=$0}
     /\[pidesk\] quarry open=/{p++; if(/quarry open=true/)pt++; pl=$0}
     /\[deskquarry\] seat /{d++; dq=$0; if(match($0,/compiled=[0-9]+/))cp=substr($0,RSTART+9,RLENGTH-9)+0; if(match($0,/ open=[0-9]+/))op=substr($0,RSTART+6,RLENGTH-6)+0; if(match($0,/relatched=[0-9]+/))rl=substr($0,RSTART+10,RLENGTH-10)+0}
     END{printf "A8 quarry: quarry_open=%d skip=%d decline=%d pidesk_true=%d deskquarry_seat=%d compiled=%d open=%d relatched=%d -> %s\n",o,sk,dc,pt,d,cp,op,rl,
       (!d)?"[deskquarry] ABSENT — the A8 seat line is not in this image":
       (cp==0)?"NOT COMPILED (compiled=0 — the render6 defect stands: `quarry` feature off)":
       (cp==1&&op==1&&o>=1&&pt>=1)?"PASS A8 (compiled=1 open=1, the window is on the wire; now count 4 dock tiles on the glass)":
       (cp==1&&op==0&&dc>0)?"DECLINED and named: "substr(dl,1,80):
       (cp==1&&op==0)?"COMPILED, NOT OPEN and no DECLINE printed — silent":"NO-VERDICT";
       if(ol) print "   " substr(ol,1,150); if(pl) print "   " substr(pl,1,110); if(dq) print "   " substr(dq,1,110)}' "$B"

# ========================================================= A25 / R21 MENUBAR ==
# CITATIONS RE-DERIVED at hw-jetson 8b696271 — every one of these moved under the round's
# folds (b768331a MENUBAR2 + 42cf16f9 PANELFIX), so render7's numbers are all stale:
#   `[winmenu] publish owner={} titles={} items={} slot={} replaced={} app-menu={}`
#                                                                      -- video/winmenu.rs:435
#   `[winmenu] publish REFUSE ...`         -- video/winmenu.rs:358,363,372,380,390,420
#   `[winmenu] REFUSE site={} reason=registry-contended`                -- video/winmenu.rs:294
#   `[winmenu] open title={} items={} at ({},{}) title-x={} font={} kind={} owner={}`
#                                                                      -- video/winmenu.rs:1072
#   `[winmenu] pick owner={} id={} label={}`                           -- video/winmenu.rs:1227
#   `[winmenu] dismiss reason={} kind={} owner={}`  reason in {outside,esc,pick,title,clear,…}
#                                                                      -- video/winmenu.rs:1118
#   `[winmenu] clear owner={}`                                         -- video/winmenu.rs:585
awk '/\[winmenu\] publish owner=/{pb++} /\[winmenu\] publish REFUSE/{pr++; prl=$0} /\[winmenu\] REFUSE site=/{rs++}
     /\[winmenu\] open title=/{op++; ol=$0; if(match($0,/title=[^ ]+/))ti=substr($0,RSTART+6,RLENGTH-6)}
     /\[winmenu\] pick owner=/{pk++; pl=$0}
     /\[winmenu\] dismiss reason=/{ds++; if(/reason=esc/)esc++; if(/reason=outside/)outs++; if(/reason=pick/)dpk++; dl=$0}
     END{printf "A25 winmenu: publish=%d publish_refuse=%d contend_refuse=%d open=%d last_title=%s pick=%d dismiss=%d (esc=%d outside=%d pick=%d) -> %s\n",pb,pr,rs,op,(ti?ti:"n/a"),pk,ds,esc,outs,dpk,
       (!pb&&!op)?"[winmenu] ABSENT — MENUBAR not in this image (or no window published a tree)":
       (pr>0)?"PUBLISH REFUSED: "substr(prl,1,80):
       (pb>0&&op>0&&esc>0)?"PASS A25 (published, opened from the BAR, Esc dismissed — R21 satisfied on the wire)":
       (pb>0&&op>0&&!esc&&ds>0)?"OPENED and dismissed, but NOT by esc (reasons above) — press Esc with the menu down":
       (pb>0&&op>0&&!ds)?"OPENED, NEVER DISMISSED — the A10 shape, now in the bar":
       (pb>0&&!op)?"PUBLISHED, NEVER OPENED (the bar title was not clicked)":"NO-VERDICT";
       if(ol) print "   " substr(ol,1,130); if(pl) print "   " substr(pl,1,110); if(dl) print "   " substr(dl,1,110)}' "$B"

# A25 NEGATIVE — the render6 IN-WINDOW View strip must be GONE (R21: menus live in
# the bar, never in a window). The tokens the old in-window menu printed, at the
# render6 image f2eae02:
#   `:: PULSEWIN-MENU: title_press={} options={} live={} ::` -- pulsewin.rs@f2eae02:864
#   `[pulsewin] menu dismiss reason=outside|content|escape`  -- pulsewin.rs@f2eae02:834,892,907
# Both are written NOWHERE at 37c78ad7 (`grep -n 'pulsewin\] menu' video/pulsewin.rs`
# returns nothing; `:: PULSEWIN-MENU: pick id=` at pulsewin.rs:648 SURVIVES as the
# winmenu pick callback and is deliberately NOT counted here).
awk '/:: PULSEWIN-MENU: title_press=/{tp++; tl=$0} /\[pulsewin\] menu dismiss reason=/{md++; ml=$0} /:: PULSEWIN-MENU: pick /{pk++}
     END{printf "A25 negative: in-window title_press=%d in-window menu-dismiss=%d (winmenu pick callback=%d, expected>=0) -> %s\n",tp,md,pk,
       (tp==0&&md==0)?"PASS (no in-window View strip token — R21 holds)":"FAIL R21 (the in-window menu strip is STILL LIVE)";
       if(tl) print "   " substr(tl,1,120); if(ml) print "   " substr(ml,1,120)}' "$B"

# ========================================================= A26 CONSOLEQUIET ===
#   `[conquiet] mirror=off since=console-window-route win={} lines_dropped={} knob=bootlog (...)`
#       -- video/fbcon.rs:3018   (once per boot: CONQUIET_ANNOUNCED swap)
#   `[conquiet] census dropped={} win={} -> QUIET (...)`
#       -- video/fbcon.rs:3025   (once, at CONQUIET_CENSUS_AT=256 dropped lines, fbcon.rs:2991)
# The routed-console WRITER — the thing whose glyphs would have shown the census —
# announces itself with:
#   `[wc-x] console-route first-paint win={} (glyphs -> window surface, damage-limited)`
#       -- video/fbcon.rs:1227
# The route must be installed for the gate to be reachable at all; the wire proxy for
# "the console window shows no census" is CONQUIET_DROPPED climbing past the census mark.
awk '/\[wc-x\] console-route first-paint/{rt++; rl=$0} /\[wc-x\] console-window win=/{cw++}
     /\[conquiet\] mirror=off/{mo++; ml=$0; if(match($0,/lines_dropped=[0-9]+/))ld=substr($0,RSTART+14,RLENGTH-14)+0}
     /\[conquiet\] census dropped=/{cs++; cl=$0; if(match($0,/dropped=[0-9]+/))dr=substr($0,RSTART+8,RLENGTH-8)+0}
     END{printf "A26 conquiet: console_route=%d console_window=%d mirror_off=%d (at line %d) census=%d dropped=%d -> %s\n",rt,cw,mo,ld,cs,dr,
       (!rt)?"NO CONSOLE ROUTE — the gate is unreachable this boot; A26 NOT-SCORED":
       (mo==0)?"FAIL A26 — the route installed and [conquiet] mirror=off NEVER PRINTED (the census is still scrolling on the glass)":
       (mo>1)?"FAIL A26 — mirror=off printed "mo" times; the announce is one-shot by construction":
       (mo==1&&cs==1)?"PASS A26 (mirror=off once, then "dr" census lines dropped and named — the console window is quiet)":
       (mo==1&&cs==0)?"PASS-WEAK (mirror=off once; fewer than 256 lines dropped so no census line — read the glass)":"NO-VERDICT";
       if(rl) print "   " substr(rl,1,120); if(ml) print "   " substr(ml,1,120); if(cl) print "   " substr(cl,1,120)}' "$B"

# ================================================= A20 CLICKDEAD ptrpoll ======
#   `[ptrpoll] t={} rearm={} discard={} errrearm={} dup={} nobuf={} reports={} base={} decoded={} -> {}`
#       -- arch/aarch64/display_tegra.rs:5396 (called from the census pass, display_tegra.rs:1484)
#   verdicts: STREAMING / GUARD-REARM / ERROR-REARM / BASELINE
#             `NOBUF-DROP (...)` -- display_tegra.rs:5389
#             `DUP-DROP (...)`   -- display_tegra.rs:5391
#             `ARMED-NO-COMPLETION (...)` -- display_tegra.rs:5397 tail
#   dup=/nobuf= are MOUSE_DUP_DROP_COUNT / MOUSE_NOBUF_DROP_COUNT -- drivers/xhci/mod.rs:2385
# THE QUESTION: the FIRST [ptrpoll] line (census seq=1). rearm=2 -> reads stopped;
# rearm>2 -> the pipeline re-armed.
awk '/\[ptrpoll\] /{n++; if(n==1){first=$0; for(i=1;i<=NF;i++){if($i~/^rearm=/){sub(/rearm=/,"",$i);r1=$i+0} if($i~/^dup=/){sub(/dup=/,"",$i);d1=$i+0} if($i~/^nobuf=/){sub(/nobuf=/,"",$i);b1=$i+0}}}
       last=$0; for(i=1;i<=NF;i++){if($i~/^rearm=/){sub(/rearm=/,"",$i);rN=$i+0} if($i~/^dup=/){sub(/dup=/,"",$i);dN=$i+0} if($i~/^nobuf=/){sub(/nobuf=/,"",$i);bN=$i+0} if($i~/^reports=/){sub(/reports=/,"",$i);rp=$i+0}}
       if(/-> DUP-DROP/)vd++; if(/-> NOBUF-DROP/)vb++; if(/-> STREAMING/)vs++; if(/-> ARMED-NO-COMPLETION/)va++; if(/-> GUARD-REARM/)vg++; if(/-> BASELINE/)vbl++}
     END{printf "A20 ptrpoll: lines=%d first_rearm=%d first_dup=%d first_nobuf=%d | final rearm=%d dup=%d nobuf=%d reports=%d | verdicts STREAMING=%d BASELINE=%d GUARD-REARM=%d DUP-DROP=%d NOBUF-DROP=%d ARMED-NO-COMPLETION=%d -> %s\n",n,r1,d1,b1,rN,dN,bN,rp,vs,vbl,vg,vd,vb,va,
       (!n)?"[ptrpoll] ABSENT — the CLICKDEAD witness is not in this image (orinclick off?)":
       (r1==2)?"READS STOPPED at census 1 (rearm=2 — the enumeration armed twice and nothing re-armed since)":
       (r1>2)?"RE-ARMED (rearm="r1" > 2 at census 1 — the pipeline is moving; a dead click is a ROUTING fault)":
       "rearm="r1" at census 1 — below the enumeration floor of 2; read the line";
       if(first) print "   first:  " substr(first,1,150); if(last&&n>1) print "   last:   " substr(last,1,150)}' "$B"

# ================================================================= A17 PRTSCR =
#   armed: `:: PRTSCR: PrintScreen (HID 0x46) down on xHCI -> capture armed ::`
#       -- drivers/xhci/mod.rs:4929 (EHCI twin: drivers/ehci/mod.rs:16196)
#   CITATIONS RE-DERIVED at hw-jetson 8b696271 — the PRTSCR3 fold (cd533543/d7eec583/fc91eef9)
#   moved every one of these; render7's numbers are stale.
#   OK:    `:: PRTSCR: {} {}x{} {} bytes -> OK ::` -- video/prtscr.rs:387 and prtscr.rs:1069
#   THE NAMED IN-FLIGHT REFUSAL (the render6/render7 gap — fast presses printed nothing):
#          `:: PRTSCR: refused — capture in flight (another task holds the capture door; a key
#           request is re-armed and runs after it) ::` -- video/prtscr.rs:480
#           (Refusal::InFlight declared prtscr.rs:431, taken at prtscr.rs:661)
#   other refusals: `:: PRTSCR: ... capture skipped ::` -- prtscr.rs:444-469;
#           `capture INCOMPLETE ::` -- prtscr.rs:476; `volume vanished mid-capture` -- prtscr.rs:483
#   NOTE: the SR2/A36 block far below scores the async mechanism itself (slice/InFlight/Vanished);
#   this block stays as render7 wrote it so the two readings can be compared side by side.
awk '/PRTSCR: PrintScreen \(HID 0x46\) down/{armed++} /:: PRTSCR: SCREEN[0-9]+\.PNG [0-9]+x[0-9]+ [0-9]+ bytes -> OK ::/{ok++; match($0,/SCREEN[0-9]+\.PNG/); names=names" "substr($0,RSTART,RLENGTH)}
     /:: PRTSCR: refused/{inf++; il=$0} /:: PRTSCR: .*capture (skipped|INCOMPLETE)/{oth++; ol=$0} /:: PRTSCR: .* -> capturing /{cap++}
     END{printf "A17 prtscr: armed=%d capturing=%d ok=%d inflight_refusals=%d other_refusals=%d names=[%s ] -> %s\n",armed,cap,ok,inf,oth,names,
       (armed<4)?"INCOMPLETE ("armed"/4 presses seen — press four times, fast)":
       (ok>=2&&inf>=1)?"PASS A17 (two files and "inf" NAMED InFlight refusal(s) — the render6 gap is closed)":
       (ok>=2&&inf==0&&oth==0&&armed>ok)?"FAIL A17 GAP STANDS ("armed" presses, "ok" verdicts, "(armed-ok)" silent — no refusal named)":
       (ok>=2&&oth>0)?"PASS-BY-OTHER-REFUSAL (named, not InFlight): "substr(ol,1,80):
       (ok<2)?"FAIL A17 (<2 OK files from "armed" presses)":"NO-VERDICT";
       if(il) print "   " substr(il,1,140)}' "$B"
awk '/:: PRTSCR:/' "$B" | cut -c1-140

echo "-- render8 additions --"

# ================================================== A21 tick (UNAOS_BSPTICK) ==
# `:: [orinbsptick] arming PERIODIC CNTP at EL1 on cpu 0 (250 Hz, PPI30) — ... ::`
# `:: [orinbsptick] tick {N} taken at EL1 on cpu 0 — periodic CNTP live across the terminus ::`
#     (emitted at tick 1 and every 250th tick; the 250 Hz tick itself is silent)
# Verbatim from docs/dev/evidence/orin14/TICK1-FLIGHT.md §C, kept executable at
# ~/unaos-bench/scratch/orin14/bsptick/score-tick1.sh; it printed PASS on the 2026-09-06
# tick1 boot (arm=1 tick_lines=133 tmax=33000 census=140 exceptions=0).
awk '
/\[orinbsptick\] arming PERIODIC CNTP at EL1 on cpu 0/ { arm++ }
/\[orinbsptick\] tick [0-9]+ taken at EL/ { n=$0; sub(/.*\] tick /,"",n); sub(/ .*/,"",n); n+=0
  tl++; if (n>tmax) tmax=n; lastt=NR; if ($0 ~ /taken at EL2/) el2++ }
/\[orinrender\] census passes=/ { p=$0; sub(/.*passes=/,"",p); sub(/ .*/,"",p); p+=0
  if (arm) { cen++; if (p>cmax) cmax=p; lastc=NR; if (cen==1) cfirst=p } }
/=== AARCH64 EXCEPTION/ { exc++; if (!excline) excline=NR }
END { v="UNSCORED"
  if (arm==0) v="ARM-ABSENT (no banner — wrong image, or the boot died before the terminus; score A15 first)"
  else if (exc>0) v="EXCEPTION at line " excline
  else if (el2>0) v="FAIL-EL2 (HCR_EL2.IMO regressed)"
  else if (tl==0) v="ARM-MISS (banner, no tick 1)"
  else if (tmax==1) v="NO (tick 1 printed, the second never came)"
  else if (tmax>=2500 && cen>=10 && cmax>cfirst) v="PASS"
  else if (tmax>=2500) v="PUMP-STALL (ticks advance, census stopped)"
  else v=(lastc>lastt) ? "TICK-DIED at " tmax : "SHORT tmax=" tmax
  printf "A21 tick: arm=%d ticklines=%d tmax=%d census=%d passes=%d->%d exceptions=%d -> %s\n", arm,tl,tmax,cen,cfirst,cmax,exc,v }' "$B"

# =================================================== A21 run (UNAOS_BSPRUN) ===
# `:: [bsprun] host core=0 el=1 -> HOSTING (online=0x… el1cores=0x1; predicate = el0_placement_possible(CPU_AUTO) …) ::`
# `:: [bsprun] el0 first-run '<task>' on core 0 — CurrentEL EL1 checked, eret to EL0 ACCEPTED (n=1) ::`
# `:: [orinbsprun] boot core 0 joins run() … ::`
# `:: BGRUN: bg /fat/vug.elf — loaded N bytes, entry 0x…, pid=… slot=… DETACHED ::`
#   -- tokens quoted from ~/unaos-bench/scratch/orin16/bsprun/PROGRESS.md §5 (strings-proven
#      on the builder-path kernel.elf: `[bsprun] host core=`=1, `[bsprun] el0 first-run`=2).
awk '/\[bsprun\] host core=/{h++; if(/-> HOSTING/)ho++; if(/-> REFUSING/)rf++; hl=$0}
     /\[bsprun\] el0 first-run/{fr++; if(/CAPPED/)cap++; else fl=$0}
     /\[orinbsprun\]/{jr++; jl=$0}
     /BGRUN: bg /{bg++; if(/DETACHED/)det++; if(/rejected/){rej++; rl=$0}}
     END{printf "A21 run: host=%d hosting=%d refusing=%d bgrun=%d detached=%d rejected=%d el0_first_run=%d capped=%d orinbsprun_join=%d -> %s\n",h,ho,rf,bg,det,rej,fr,cap,jr,
       (!h)?"[bsprun] ABSENT — the BSPRUN witness is not in this image (knob missing, or the fold is not aboard)":
       (rf>0)?"FALSIFIED: -> REFUSING at the terminus — the arc'"'"'s premise does not hold on this board":
       (ho>0&&rej>0)?"SPLIT: HOSTING but the spawn-path re-check rejected (EL0-EL1CORE) — advisory and re-check disagree":
       (ho>0&&det>0&&fr>0)?"PASS A21-run (hosted, detached, and the eret to EL0 was ACCEPTED)":
       (ho>0&&det>0&&!fr)?"PLACED, NEVER DISPATCHED (no el0 first-run after a DETACHED spawn)":
       (ho>0&&!bg)?"HOSTING, NO SPAWN (no BGRUN line — nothing was launched to host)":"NO-VERDICT";
       if(hl) print "   " substr(hl,1,150); if(fl) print "   " substr(fl,1,150); if(rl) print "   " substr(rl,1,150); if(jl) print "   " substr(jl,1,120)}' "$B"

# ======================================================= A12 / NET-5 (Q0) =====
# `[net5R] ARMED …` / `[net5R] NOT ARMED …` / `… MISMATCH — probe VOID`
#   -- arch/aarch64/rtl8168_tegra.rs § NET-5; shapes from orin16/net4b/PROGRESS.md §5.
awk '/\[net5R\]/{n++; l=$0; if(/ARMED/&&!/NOT ARMED/)ar++; if(/NOT ARMED/)na++; if(/MATCH/&&!/MISMATCH/)mt++; if(/MISMATCH/)mm++}
     /net4F\] rx-ring phys=/{rp++}
     END{printf "A12 net5 Q0: net5R_lines=%d armed=%d not_armed=%d match=%d mismatch=%d (net4F rx-ring seen=%d) -> %s\n",n,ar,na,mt,mm,rp,
       (!n&&rp)?"BUILD FAULT — no [net5R] while [net4F] rx-ring phys= is present: net5 is NOT in this image. Check the effective-features banner. Never a hardware verdict.":
       (!n)?"[net5R] ABSENT and no net4F either — the net knobs are off, or the NIC never came up":
       (mm)?"VOID — the re-point never reached DRAM; no landing arm may be read off this boot":
       (na)?"UNDECIDED — the line names the failing precondition (below4g=0, or the shadow outside the NET-4h identity region); read the [net4A] census":
       (ar&&mt)?"ARMED + MATCH — live; read Q2":"NO-VERDICT"; if(l) print "   " substr(l,1,170)}' "$B"

# ------------------------------------------------------- A12 / NET-5 (Q1) ----
awk '/net4F\] rx-ring phys=/{n++; l=$0; if(/below4g=1/)b++}
     /net4s\]/{s++; if(/identity-covered, no alias/)ic++}
     /\[net4r\] alias region 1/{al++}
     END{printf "A12 net5 Q1: rx_ring_lines=%d below4g=%d net4s=%d identity_covered=%d alias_region1=%d -> %s\n",n,b,s,ic,al,
       (!n)?"NO rx-ring line — the ring never came up":(b&&ic>=5&&!al)?"PASS placement (sub-4GiB, identity-covered, no alias in the path)":
       (al)?"ALIAS PRESENT — [net4r] alias region 1 is on the wire; the no-alias premise does not hold":"CHECK: below4g="b" identity_covered="ic; if(l) print "   " substr(l,1,170)}' "$B"

# ------------------------------------------------- A12 / NET-5 (Q2 — THE Q) ---
# `[net5T] rx[…` per-pop labels and `[net5V] ring RE-FETCH verdict …` with the five arms
#   REFETCH-LIVE / REFETCH-WRONGSLOT / STALE-ORIG / NOWHERE / PREFETCHED, plus
#   `SINCE-LAST-POP` and `pops-scored=`.
# NOTE the arm counters read the VALUE of `NAME=<n>`, never the presence of the name: the
# [net5V] verdict line lists all five arms with their counts, so a substring test would score
# `REFETCH-WRONGSLOT=0` as a hit. (Caught by the synthetic can-fire proof in
# stage8/scorers-render8-selftest.out — the exact "check that cannot fire" family, inverted.)
awk 'function av(s,  n){ if(match($0,s"=[0-9]+")) {n=substr($0,RSTART+length(s)+1,RLENGTH-length(s)-1)+0; return n} return -1 }
     /\[net5T\]/{t++} /\[net5V\]/{v++; vl=$0
       x=av("REFETCH-LIVE"); if(x>0)live+=x; x=av("REFETCH-WRONGSLOT"); if(x>0)ws+=x
       x=av("STALE-ORIG"); if(x>0)st+=x; x=av("NOWHERE"); if(x>0)nw+=x; x=av("PREFETCHED"); if(x>0)pf+=x
       if(match($0,/pops-scored=[0-9]+/))ps=substr($0,RSTART+12,RLENGTH-12)+0
       if(/was NOT ARMED/)na++}
     END{printf "A12 net5 Q2: net5T=%d net5V=%d pops_scored=%d | REFETCH-LIVE=%d REFETCH-WRONGSLOT=%d STALE-ORIG=%d NOWHERE=%d PREFETCHED=%d -> %s\n",t,v,ps,live,ws,st,nw,pf,
       (!v)?"NO [net5V] VERDICT LINE — read Q0 first":
       (na||ps==0)?"UNDECIDED (pops-scored=0 or the probe was NOT ARMED) — no traffic / no link. NEVER FAIL.":
       (st>0)?"C2 PASS (defect located): STALE-ORIG — a payload landed in an ORIGINAL buffer no descriptor points at. Lane = NIC register/errata.":
       (ws>0)?"C2'"'"' PASS (defect located): REFETCH-WRONGSLOT — the address path is live; the reuse is an index defect.":
       (nw>0&&pf==0)?"C1 PASS (defect located): NOWHERE with PREFETCHED=0 — inbound delivery loss. Lane = the RC inbound write path.":
       (live>0)?"PASS (defect gone): REFETCH-LIVE — per-descriptor addressing works post-enable; the latch was the instrument. Read Q3 for the lease.":
       (pf>0)?"INCONCLUSIVE MIXED — the re-point lost the race with prefetch; prefetch-depth is the datum, re-fly arming the shadow BEFORE RxEnb.":"NO-VERDICT";
       if(vl) print "   " substr(vl,1,190)}' "$B"

# ------------------------------------------- A12 Q3 lease + Q4 net4 control ---
awk '/net4V no-lease verdict|DHCP lease|\[dhcp\]/{n++; l=$0; if(/DHCP lease/)ls++}
     END{printf "A12 net5 Q3: dhcp_lines=%d lease=%d -> %s\n",n,ls,(ls)?"LEASE ACQUIRED":(n)?"NO LEASE (meaningful only on a REFETCH-LIVE Q2)":"NO DHCP LINES"; if(l) print "   " substr(l,1,170)}' "$B"
awk '/net4F\] rx\[|net4F\] distinct buffers-written|RX ring pass verdict/' "$B" | tail -6 | cut -c1-170
awk '/\[net4G\]/{g++} END{printf "A12 net4 control: net4G_lines=%d -> %s\n",g,(g==0)?"EXPECTED (its self-gate is the [net4F] run of >=4 the honest per-pop cadence removes)":"UNEXPECTED — [net4G] armed under the NET-5 cadence; read why"}' "$B"

# =============================================== A24 GA10B rung 3 / 3b ========
# TOKENS READ FROM SOURCE at hw-jetson 8b696271 (fold 5fc5506a; knob UNAOS_GA10B_PROBE3=2):
#   `[ga10bprobe3] pg={:#x} clk={}/{} regs={} of {} readable, {} UNREADABLE -> COMPLETE`
#       -- arch/aarch64/ga10b_probe.rs:1049   (rung 3's ONE summary arm)
#   `[ga10bprobe3] pg=n/a clk=0/0 -> REFUSED reason=no-gpu-node`        -- ga10b_probe.rs:855
#   `[ga10bprobe3] pg=n/a clk=0/{} -> REFUSED reason=no-power-domains`  -- ga10b_probe.rs:867
#   `[ga10bprobe3] pg=timeout clk=0/{} -> REFUSED reason=pg-timeout`    -- ga10b_probe.rs:879
#   `[ga10bprobe3b] about-to-WRITE …` / `about-to-read …`
#       -- ga10b_probe.rs:1108, 1115, 1120, 1137, 1139  (announced BEFORE each access)
#   `[ga10bprobe3b] mailbox0 wrote={:#010x} read={:#010x}`  -- ga10b_probe.rs:1141
#   `[ga10bprobe3b] -> MAILBOX-HELD`                        -- ga10b_probe.rs:1143
#   `[ga10bprobe3b] -> MAILBOX-MISMATCH read={:#010x}`      -- ga10b_probe.rs:1145
#   `[ga10bprobe3b] -> MAILBOX-SKIPPED reason=cpuctl-all-ones|cpuctl-pri-error`
#       -- ga10b_probe.rs:1124 / :1130
#   `[ga10bprobe3b] rung 3b complete`                       -- ga10b_probe.rs:1147
# PASS / REFUSED / UNREADABLE / STOP shapes: orin16/ga10b3/PROGRESS.md §7.
# UNREADABLE IS A DATUM, NOT A FAILURE — a rung 3 with every priscv BCR register unreadable
# is a PASS WITH A FINDING (priv-lockdown covers the BCR block), which is what rung 4 needs.
awk '/\[ga10bprobe3\]/{n++; l=$0; if(/-> COMPLETE/){cmp++; cl=$0; cmpline=NR} if(/-> REFUSED/){r++; rl=$0} if(/rung 3 complete/)done3++}
     /-UNREADABLE/{unr++}
     /bcr_dmacfg lock_locked=/{bcr++; bl=$0} /opt_wpr_enabled=/{wpr++; wl=$0}
     /about-to-read|about-to-WRITE/{annline=NR}
     /\[deskcascade\] -> CASCADED/{casc++; cascline=NR}
     END{cascafter=(cmpline && casc && cascline>cmpline)?1:0
       printf "A24 rung3: lines=%d complete=%d refused=%d rung3_complete=%d unreadable=%d bcr_lock=%d opt_wpr=%d cascaded_after_probe=%d -> %s\n",n,cmp,r,done3,unr,bcr,wpr,cascafter,
       (!n)?"NOT-SCORED (no [ga10bprobe3] on the wire — the rung-3 fold or the knob is not aboard; score the effective-features banner, not the board)":
       (annline&&annline==NR)?"⚠ STOP RULE FIRED — THE LAST LINE ON THE WIRE IS AN ANNOUNCE. That access was fatal in this state. Record it verbatim and STOP; do NOT re-fly with the step removed (R19: failed under these conditions, never ruled out).":
       (r)?"REFUSED (a datum, not a defect) — the reason IS the answer: "substr(rl,1,110):
       (cmp&&done3&&bcr&&wpr&&cascafter)?("PASS A24-rung3 (COMPLETE, rung 3 complete, both rung-4 inputs present, and [deskcascade] -> CASCADED after it — the boot continued behind the rung)" (unr?" WITH A FINDING: "unr" register(s) UNREADABLE, which is itself the answer rung 4 needs":"")):
       (cmp&&!cascafter)?"COMPLETE but NO [deskcascade] -> CASCADED after it — the rung returned and the desktop did not come up; read the next lines":
       (cmp&&(!bcr||!wpr))?"COMPLETE but a rung-4 input is MISSING (bcr_dmacfg lock_locked="bcr+0" opt_wpr_enabled="wpr+0") — rung 4 cannot be planned off this boot":
       "NO-VERDICT"; if(cl) print "   " substr(cl,1,180); if(bl) print "   " substr(bl,1,150); if(wl) print "   " substr(wl,1,150); if(rl) print "   " substr(rl,1,180)}' "$B"
awk '/\[ga10bprobe3b\]/{n++
       if(/mailbox0 wrote=/){mb++; ml=$0; if(match($0,/wrote=0x[0-9a-f]+/))w=substr($0,RSTART+6,RLENGTH-6); if(match($0,/read=0x[0-9a-f]+/))rd=substr($0,RSTART+5,RLENGTH-5)}
       if(/-> MAILBOX-HELD/)held++; if(/-> MAILBOX-MISMATCH/){mis++; xl=$0}
       if(/-> MAILBOX-SKIPPED/){skip++; sl=$0} if(/rung 3b complete/)fin++
       if(/about-to-WRITE/)wr++; if(/about-to-read/)rdann++; if(/rung 3b ARMED/)armed++}
     END{printf "A24 rung3b: lines=%d armed=%d write_announces=%d read_announces=%d mailbox_lines=%d wrote=%s read=%s held=%d mismatch=%d skipped=%d terminus=%d -> %s\n",n,armed,wr,rdann,mb,(w?w:"n/a"),(rd?rd:"n/a"),held,mis,skip,fin,
       (!n)?"NOT-SCORED (no [ga10bprobe3b] — value 2 was not on the knob line, or the fold is not aboard)":
       (!fin)?"⚠ RUNG 3b DID NOT REACH ITS TERMINUS — no `rung 3b complete`. Read the last [ga10bprobe3b] line: if it is an about-to- announce, the STOP RULE applies.":
       (held)?"PASS A24-rung3b MAILBOX-HELD — a GA10B engine register accepted a write from this kernel and held it; the CCPLEX can drive this engine scratch state with the GSP halted":
       (mis)?"MAILBOX-MISMATCH (a RESULT, not a failure) — RE-VERIFY THE PUBLIC-RECALLED MAILBOX POINTER FIRST, before suspecting the board: "substr(xl,1,110):
       (skip)?"MAILBOX-SKIPPED BY DESIGN (a RESULT) — the reset did not leave the engine readable: "substr(sl,1,110):"NO-VERDICT";
       if(ml) print "   " substr(ml,1,150)}' "$B"
awk '/about-to-read|about-to-WRITE/{l=$0; n=NR} END{if(!n){print "A24 STOP-CHECK: no about-to- announce on the wire -> N/A (rung 3b not armed this boot)"} else {printf "A24 STOP-CHECK: last announce at line %d of %d -> %s\n", n, NR, (n==NR)?"⚠ THE ANNOUNCE IS THE LAST LINE — that access was fatal. STOP.":"OK (the boot continued past it)"; print "   " substr(l,1,180)}}' "$B"

# ====================================================== A28 ROOTFS ============
# `[sdmmc] root mount source=tegra-sd … -> OK …`
# `[sdmmc] root bound / = tegra-sd FAT read-only … entries=N …`
# `[sdmmc] root -> REFUSED reason=…`
#   -- tokens quoted from the ROOTFS executor's own arroyo block (UNAOS_SDMMCROOT, lines
#      1576-1593 of its worktree's unaos/arroyo). Baseline defect being fixed: `ls /` and
#      quarry answered "/: backend error: unafs-mount".
awk '/\[sdmmc\] root mount source=/{m++; if(/-> OK/)ok++; ml=$0}
     /\[sdmmc\] root bound \//{b++; bl=$0; if(match($0,/entries=[0-9]+/))en=substr($0,RSTART+8,RLENGTH-8)+0}
     /\[sdmmc\] root -> REFUSED/{r++; rl=$0}
     /backend error: unafs-mount/{um++}
     /volume \/fat not mounted/{fat++}
     END{printf "A28 rootfs: mount=%d ok=%d bound=%d entries=%d refused=%d unafs_mount_errors=%d fat_enodev=%d -> %s\n",m,ok,b,en,r,um,fat,
       (!m&&!r)?"NOT-SCORED (no [sdmmc] root line — the ROOTFS fold is not aboard, or UNAOS_SDMMCROOT is off)":
       (r)?"REFUSED and NAMED (a datum): read the reason":
       (ok&&b&&en>0&&!um)?"PASS A28 (/ bound to the card'"'"'s FAT through TegraSd, "en" entries, and NO unafs-mount error)":
       (ok&&b&&um)?"BOUND but an unafs-mount error is STILL on the wire — something else still mounts /":
       (ok&&!b)?"MOUNTED, NEVER BOUND (no root bound line)":"NO-VERDICT";
       if(ml) print "   " substr(ml,1,160); if(bl) print "   " substr(bl,1,160); if(rl) print "   " substr(rl,1,160)}' "$B"

# ================================================ A30 / SO5 / A38 — DESKFIX ===
# From ~/unaos-bench/scratch/orin16/deskfix/PROGRESS.md §5 (fold 0fced841). The A30 PASS
# shape is the one the render7 boot FAILED: three [pulsewin] open per boot became one.
awk '/\[pulsewin\] open win=/{o++} /\[pulsewin\] close win=/{c++; if(/-> CLOSED \(reopen only via dock\)/)cc++}
     /pulse=pin -> rearmed/{re++}
     END{printf "A30 deskfix: pulsewin_open=%d close=%d close_final=%d dock_rearm=%d -> %s\n",o,c,cc,re,
       (!o)?"NO [pulsewin] open — score A18 first":
       (o==1+re+0&&c==cc)?"PASS A30 (one open per boot plus "re+0" operator dock re-arm(s); every close is final)":
       (o>1+re+0)?"FAIL A30 ("o" opens for "re+0" dock re-arm(s) — the render7 shape was 3)":
       (c!=cc)?"FAIL A30 (a close did not carry -> CLOSED (reopen only via dock))":"NO-VERDICT"}' "$B"
awk '/\[sprite\] /{n++; l=$0; if(match($0,/same=[0-9]+/))sm=substr($0,RSTART+5,RLENGTH-5)+0; if(match($0,/n=[0-9]+\/8/))cap=substr($0,RSTART,RLENGTH)}
     END{printf "SO5 sprite: lines=%d same=%s cap=%s -> %s\n",n,(n?sm:"n/a"),(cap?cap:"n/a"),
       (!n)?"NOT-SCORED (no [sprite] witness — the deskfix fold is not aboard)":
       (sm==1)?"PASS SO5 (same=1 — the pal.rs grant landed and the two scales agree)":
       "EXPECTED-TODAY (same=0 — the pal.rs one-liner is FILED UNAPPLIED pending the rmbp grant; the divergence is now on the wire, which is the fix'"'"'s point)";
       if(l) print "   " substr(l,1,150)}' "$B"

# ═════════════════════════════════════════════════════════════════════════════
# THE FORMER STUB BLOCK — NO TOKEN-TBD REMAINS. Every fold of the round landed on hw-jetson
# by 8b696271, so each stub below has been replaced by a real awk whose pattern was copied
# from SOURCE at that tip (`git grep -n`), with the file:line printed above it. The can-fire
# proof for each is in stage8/scorers-render8-selftest.out: run against render7-boot1.log
# (where the token is absent -> must say ABSENT/NOT-SCORED) and against a synthetic line set
# carrying the token (where it must produce its PASS verdict), because these tokens have
# never flown. "A check that cannot fire": printing is not gating, and a zero-hit result
# indicts the pattern before it indicts the boot.
# ═════════════════════════════════════════════════════════════════════════════

# ───────────────────────────────────────────── A37 SERIALRX-DEDUP (rxmerge) ──
#   `[rxmerge] policy={} armed={} uartc-rbr={} -> A37: one owner, one ordered stream …`
#       -- arch/aarch64/serial.rs:1065   (once, at arm)
#   `[rxmerge] census policy={} seq={} uartc={} mbox={} dup={} reorder={} parked={} -> {}`
#       -- arch/aarch64/serial.rs:1145   verdict tail = SINGLE-SOURCE | SPLIT-SOURCE
#   `[rxmerge] src={} seq={} byte={:#04x} '{}' policy={} dup={} reorder={}`
#       -- arch/aarch64/serial.rs:1121   (per byte, for a suspected split)
# EXPECTED SIDE EFFECTS under policy=mbox-only, stated by serial.rs:1145's own verdict text:
# `[serialrx] polls=0` all boot is CORRECT (the RBR is never read; `parked=` is the
# liveness counter that replaces it) and `ovrf=` is at most 1. Do NOT score A16 as a
# regression on polls=0 — this block prints those two readings so the pair is visible.
awk '/\[rxmerge\] policy=/{arm++; al=$0}
     /\[rxmerge\] census /{n++; cl=$0
       if(match($0,/policy=[a-z-]+/))po=substr($0,RSTART+7,RLENGTH-7)
       if(match($0,/ dup=[0-9]+/))d=substr($0,RSTART+5,RLENGTH-5)+0
       if(match($0,/ reorder=[0-9]+/))ro=substr($0,RSTART+9,RLENGTH-9)+0
       if(match($0,/parked=[0-9]+/))pk=substr($0,RSTART+7,RLENGTH-7)+0
       if(/SINGLE-SOURCE/)ss++; if(/SPLIT-SOURCE/)sp++}
     /\[rxmerge\] src=/{pb++}
     /\[serialrx\] rx=/{sr++; if(match($0,/polls=[0-9]+/))pl=substr($0,RSTART+6,RLENGTH-6)+0; if(match($0,/ovrf=[0-9]+/))ov=substr($0,RSTART+5,RLENGTH-5)+0}
     END{printf "A37 rxmerge: arm=%d census=%d policy=%s dup=%d reorder=%d parked=%d per_byte=%d | side-effects [serialrx] polls=%s ovrf=%s -> %s\n",arm,n,(po?po:"n/a"),d,ro,pk,pb,(sr?pl:"n/a"),(sr?ov:"n/a"),
       (!n&&!arm)?"ABSENT — no [rxmerge] line at all; the A37 fold is not aboard this image (it is knob-less and rides tcurx, so check the banner for tcurx first)":
       (!n)?"ARMED but NO CENSUS — the policy line printed and the census never did; read the arm line":
       (sp||d>0||ro>0)?"FAIL A37 SPLIT-SOURCE (dup="d" reorder="ro") — two readers are STILL both delivering; awk /\\[rxmerge\\] src=/ for the per-byte trace":
       (ss&&d==0&&ro==0)?"PASS A37 SINGLE-SOURCE (exactly-once, in-order; parked="pk" is the drain-liveness counter — polls=0 beside it is CORRECT, not a dead drain)":
       "NO-VERDICT"; if(al) print "   " substr(al,1,150); if(cl) print "   " substr(cl,1,190)}' "$B"

# ────────────────────────────────────────────── A34 / SO4 CRYSTAL ────────────
#   `[crystal] verb=restart -> PSCI SYSTEM_RESET`                     -- src/power.rs:187
#   `[crystal] verb=restart -> PSCI SYSTEM_RESET RETURNED ret={} — a returning PSCI power
#    call is a REFUSAL; parking in hlt`                               -- src/power.rs:190
#   `[crystal] verb=shutdown -> PSCI SYSTEM_OFF`  (+ RETURNED arm)    -- src/power.rs:201/:204
#   `[menubar] crystal menu={}x{}+{}+{} anchor=right-flush-under-crystal glyph={}x{}+{}
#    bar_w={} gap_right={} ::`                                        -- video/crystal.rs:543
# A34 IS THE LAST ACTION OF THE SITTING: a successful restart REBOOTS THE BOARD, so on a
# PASS this log simply ENDS after the announce — absence of anything following it is the
# pass, and a RETURNED line is the only failure that can print.
awk '/\[crystal\] verb=restart/{rs++; rl=$0; rline=NR; if(/RETURNED/){rret++; rrl=$0}}
     /\[crystal\] verb=shutdown/{sd++; sl=$0; if(/RETURNED/)sret++}
     /\[menubar\] crystal menu=/{mb++; ml=$0; if(match($0,/gap_right=[0-9]+/))gr=substr($0,RSTART+10,RLENGTH-10)+0}
     END{printf "A34/SO4 crystal: restart_announce=%d restart_RETURNED=%d shutdown_announce=%d shutdown_RETURNED=%d | SO4 menubar_lines=%d gap_right=%s -> %s\n",rs,rret,sd,sret,mb,(mb?gr:"n/a"),
       (!rs&&!mb)?"ABSENT — neither the A34 verb nor the SO4 geometry line is on the wire; the crystal fold is not aboard":
       (!rs&&mb)?"SO4 ONLY (geometry present, gap_right="gr") — A34 NOT EXERCISED: the Restart verb was never picked. It is step 13 of the FLIGHT SEQUENCE and it reboots the board.":
       (rret)?"FAIL A34 — the PSCI call RETURNED: "substr(rrl,1,130)". A returning power call is a REFUSAL by BL31, not a bug in the verb.":
       (rs&&rline==NR)?"PASS A34 (the announce is the LAST line on the wire — the board took the reset; that silence IS the pass)":
       (rs)?"ANNOUNCED and the log CONTINUED past it with no RETURNED line — read the following lines before concluding; the board may still have been mid-reset":
       "NO-VERDICT"; if(ml) print "   " substr(ml,1,170); if(rl) print "   " substr(rl,1,150); if(sl) print "   " substr(sl,1,150)}' "$B"

# ────────────────────────────────────── SR2 / A36 PRTSCR3 (async capture) ────
#   `:: PRTSCR: slice n={} bytes={}/{} ::`                            -- video/prtscr.rs:852
#   `:: PRTSCR: refused — capture in flight (another task holds the capture door; a key
#    request is re-armed and runs after it) ::`                       -- video/prtscr.rs:480
#       (Refusal::InFlight declared prtscr.rs:431, taken at :661)
#   `:: PRTSCR: {} — volume vanished mid-capture at {}/{} bytes (usb geometry retracted or a
#    newer publish replaced it; handles={}) — capture ABANDONED, nothing written through the
#    stale handle ::`                                                 -- video/prtscr.rs:483
#       (Refusal::Vanished declared prtscr.rs:435, raised at :896 / :924)
#   `:: PRTSCR: {} {}x{} -> capturing ({} bytes reserved; …) ::`      -- video/prtscr.rs:784
#   `:: PRTSCR: {} {}x{} {} bytes -> OK ::`                           -- video/prtscr.rs:387/:1069
# Vanished APPEARING is a PASS for the invariant, never a failure: it means a mid-capture
# geometry change was caught instead of writing through a stale handle.
awk '/:: PRTSCR: slice n=/{sl++; if(match($0,/n=[0-9]+/))lastn=substr($0,RSTART+2,RLENGTH-2)+0; ll=$0}
     /:: PRTSCR: refused — capture in flight/{inf++; il=$0}
     /volume vanished mid-capture/{van++; vl=$0}
     /:: PRTSCR: .* -> capturing /{cap++}
     /:: PRTSCR: .* bytes -> OK ::/{ok++}
     /PRTSCR: PrintScreen \(HID 0x46\) down/{armed++}
     END{printf "SR2/A36 prtscr3: armed=%d capturing=%d ok=%d slices=%d (last n=%s) inflight_refusals=%d vanished=%d -> %s\n",armed,cap,ok,sl,(sl?lastn:"n/a"),inf,van,
       (!sl&&!inf&&!cap)?"ABSENT — no slice, no InFlight refusal and no capturing line; the PRTSCR3 fold is not aboard (holocron off?)":
       (armed<4)?"NOT-EXERCISED ("armed"/4 presses seen — the stimulus is FOUR FAST presses, as fast as the key repeats; spaced presses cannot produce an InFlight refusal)":
       (ok>=2&&sl>=1&&inf>=1)?"PASS SR2/A36 (sliced encode on the wire, "inf" NAMED InFlight refusal(s), "ok" files written — the render7 silent-drop gap is CLOSED)":
       (ok>=2&&sl>=1&&inf==0)?"SLICED but NO REFUSAL NAMED — either no press actually collided (press faster) or the door is not refusing; compare armed="armed" against ok="ok:
       (ok>=2&&!sl)?"FILES WRITTEN, NO SLICE LINES — the blocking path ran, not the job: the ASYNC half is not aboard":
       (armed>ok&&!inf)?"FAIL — THE GAP STANDS ("armed" presses, "ok" verdicts, "(armed-ok)" silent, no refusal named)":"NO-VERDICT";
       if(ll) print "   " substr(ll,1,140); if(il) print "   " substr(il,1,160);
       if(vl) print "   VANISHED (this is a PASS for the invariant, not a defect): " substr(vl,1,150)}' "$B"

# ──────────────────────────────────────── SO1 / A29 WINID + WINID2 ───────────
#   `[wm] alloc win={} gen={}`                                        -- video/wm.rs:25182
#   `[wm] close win={} gen={} route={} holders-cleared={} names={},{},{},{}`
#                                                                     -- video/wm.rs:25122
#   `[wm] winid-register REFUSED tag={} reason=registry-full max={} …` -- video/wm.rs:24959
#       MUST BE ZERO. The registry is at 6 of 8 after WINID2 registered wcg.rs:4050 SEAM_WIN.
#   `[wm-act] close-furniture win={} owner={:#x} closed={} route-dropped={} (id-scoped: …)`
#                                                       -- arch/aarch64/syscall.rs:14150
#   `[quarry] open win={} surf={}x{} …`                  -- video/quarry/live.rs:1806
#   `[dock] shell-reopen drained by=orin_render_service win={} gen={} route={} present={}
#    -> REOPEN`                                          -- main.rs:9049 (:9026 already-live,
#                                                           :9038 declined)
#   `[dock] press at ({},{}) tile={}/{} shell=pin -> reopen requested` -- video/dock.rs:1002
#   `[wcgseam] win={} seq={} … rb_delta={} refunded={}/{}`            -- video/wcg.rs:3677
# ⚠ `[winid] selftest` (wm.rs:25272) IS NOT IN THIS IMAGE — `u7_launcher` sits below the
# `-> !` terminus this board never returns from, so LLVM strips the fixture; winid's
# PROGRESS.md measured `strings` = 0 for it on the flight artifact against 1 for each
# witness above. A scorer that waited for it would be a check that cannot fire.
awk '/\[wm\] alloc win=/{al++}
     /\[wm\] close win=/{cl++; if(match($0,/holders-cleared=[0-9]+/)){hc=substr($0,RSTART+16,RLENGTH-16)+0; if(hc>0)hcn++} ; if(/route=dropped/)rd++; cll=$0}
     /\[wm\] winid-register REFUSED/{ref++; rfl=$0}
     /\[wm-act\] close-furniture win=/{cf++; if(/route-dropped=true/){cfd++; if(match($0,/win=[0-9]+/))dropid=substr($0,RSTART+4,RLENGTH-4)+0; dl=$0}}
     /\[quarry\] open win=/{qo++; if(match($0,/win=[0-9]+/))qid=substr($0,RSTART+4,RLENGTH-4)+0; ql=$0}
     /\[wcgseam\] /{seam++}
     END{printf "SO1/A29 winid: wm_alloc=%d wm_close=%d holders_cleared_closes=%d register_REFUSED=%d | close-furniture=%d route-dropped=true=%d (last dropped id=%s) quarry_open=%d (last id=%s) wcgseam=%d -> %s\n",al,cl,hcn,ref,cf,cfd,(cfd?dropid:"n/a"),qo,(qo?qid:"n/a"),seam,
       (!al&&!cl)?"ABSENT — no [wm] alloc/close witness; the WINID fold is not aboard (furniture predicate: witness x desktop_firmware)":
       (ref>0)?"HARD FAIL — [wm] winid-register REFUSED fired "ref"x: an id cache is UNREGISTERED and SO1(b) is re-entered. Raise WINID_HOLDER_MAX. "substr(rfl,1,110):
       (cfd>0&&hcn==0)?"FAIL SO1(b) — a route-dropped close happened and NO close cleared a holder; the id was freed behind a live cache":
       (cfd>0&&hcn>0)?"PASS SO1(b) ("cfd" route-dropped close(s), "hcn" close(s) cleared a holder — every reopen at a recycled id must carry a HIGHER gen; the two lines below are the pair to read)":
       (cf>0&&cfd==0)?"NOT-EXERCISED for SO1(b) — closes happened but none read route-dropped=true; close the CONSOLE window with its disc (FLIGHT SEQUENCE step 9)":
       "INSTRUMENT PRESENT, GESTURE NOT MADE (no close-furniture on this boot)";
       if(dl) print "   " substr(dl,1,150); if(ql) print "   " substr(ql,1,150); if(cll) print "   " substr(cll,1,150);
       printf "   A29 wcgseam: %s\n", seam?"present ("seam" line(s)) — the seam ran and is registered":"ABSENT — the seam never ran this boot; acceptable, NOT a failure (this board has never flown it)"}' "$B"
awk '/\[dock\] shell-reopen drained by=orin_render_service/{d++; dl=$0; if(/route=declined/)dec++}
     /shell=pin -> reopen requested/{req++; rl=$0}
     END{printf "SO1/S4 drain: shell_pin_presses=%d drains=%d declined=%d -> %s\n",req,d,dec,
       (!req)?"NOT-EXERCISED — the pinned SHELL tile was never pressed (FLIGHT SEQUENCE step 9)":
       (req>0&&d==0)?"FAIL S4 — "req" press(es) requested a reopen and the drain NEVER RAN: the pinned tile is still a dead button (render7 read exactly this, twice, with no window)":
       (d>0&&dec==d)?"DRAINED but every arm DECLINED — read the route= field":
       (d>0)?"PASS S4 ("d" drain(s) after "req" press(es) — the reopen was minted)":"NO-VERDICT";
       if(rl) print "   " substr(rl,1,140); if(dl) print "   " substr(dl,1,160)}' "$B"

# ──────────────────────────────── A10 / SO2 / SO3 MENUBAR2 (+ V-1..V-4) ──────
#   `[winmenu] dismiss reason={} kind={} owner={}`        -- video/winmenu.rs:1118
#   `[winmenu] open title={} items={} at ({},{}) title-x={} font={} kind={} owner={}`
#                                                         -- video/winmenu.rs:1072
#       (`title-x=` and `font=` are SO2's NEW fields — their presence proves the fold)
#   `[winmenu] publish owner={} titles={} items={} slot={} replaced={} app-menu={}`
#                                                         -- video/winmenu.rs:435 (:487 custom)
#   `[winmenu] pick owner={} id={} label={} -> {} win={}`  -- video/winmenu.rs:1236
#   `[winmenu] app-menu quit win={} closed={}`             -- video/winmenu.rs:1324
#   `[winmenu] REFUSE site={} reason=registry-contended (declined, retried next pass)`
#                                                         -- video/winmenu.rs:294  (V-3)
# The A25 block above scores the render7 carry-over; THIS block scores what menubar2 added.
awk '/\[winmenu\] dismiss reason=/{ds++; if(/reason=esc/){esc++; el=$0} if(/reason=outside/)outs++}
     /\[winmenu\] open title=/{op++; ol=$0; if(/title-x=/)tx++; if(/font=/)fo++}
     /\[winmenu\] publish owner=/{pb++; if(/app-menu=/){am++; pl=$0}}
     /\[winmenu\] pick owner=/{pk++; if(/label=Quit/){qk++; kl=$0}}
     /\[winmenu\] app-menu quit win=/{aq++; if(/closed=true/)aqc++; al2=$0}
     /:: tegra: JD2 — KEY 0x1b ::/{esckey++}
     END{printf "A10/SO2/SO3 menubar2: open=%d (title-x=%d font=%d) publish=%d with_app_menu=%d pick=%d quit_picks=%d app_menu_quit=%d closed=%d | dismiss=%d esc=%d outside=%d (KEY 0x1b seen=%d) -> %s\n",op,tx,fo,pb,am,pk,qk,aq,aqc,ds,esc,outs,esckey,
       (!pb&&!op)?"ABSENT — no [winmenu] traffic; the MENUBAR2 fold is not aboard (or no window published a tree)":
       (op>0&&tx==0)?"MENUBAR2 NOT ABOARD — [winmenu] open printed WITHOUT the new title-x=/font= fields, which is the PRE-fold line. A10, SO2 and SO3 all come from b768331a, so none of them is aboard; score the effective-features banner and the image max_vaddr before reading anything else in this block.":
       (esckey>0&&esc==0&&ds>0)?"FAIL A10 — the render7 shape EXACTLY: KEY 0x1b reached the wire, the menu stayed open, and it closed for another reason ("outs" outside). The shell door is still unwired.":
       (esckey==0)?"A10 NOT-EXERCISED — no KEY 0x1b on the wire; open a bar menu and press Esc (FLIGHT SEQUENCE step 7)":
       (esc>0&&am>0&&aqc>0)?"PASS A10+SO2+SO3 (Esc dismissed with reason=esc; the app menu published; a Quit pick actually closed its window)":
       (esc>0&&am>0&&qk==0)?"PASS A10+SO2, SO3 NOT-EXERCISED — the app menu published but Quit was never picked (FLIGHT SEQUENCE step 6)":
       (esc>0&&am==0)?"PASS A10, SO3 ABSENT — no publish line carries app-menu=; title box 0 is not building an app menu":
       (esc>0)?"PASS A10 (reason=esc) — read SO2/SO3 from the counts above":"NO-VERDICT";
       if(ol) print "   " substr(ol,1,170); if(pl) print "   " substr(pl,1,150); if(el) print "   " substr(el,1,130);
       if(kl) print "   " substr(kl,1,150); if(al2) print "   " substr(al2,1,130)}' "$B"
awk '/\[winmenu\] REFUSE site=.*registry-contended/{v3++; vl=$0; v3line=NR}
     /\[winmenu\] (open title=|publish owner=)/{ok++; if(v3line&&NR>v3line)after++}
     /\[menubar\]|\[winmenu\]/{any++}
     END{printf "V-1..V-4 fixpanel: v3_contended_declines=%d winmenu_successes=%d (after a decline=%d) winmenu_family_lines=%d -> %s\n",v3,ok,after,any,
       (!any)?"NOT-SCORED — no [winmenu]/[menubar] traffic at all; score A10/SO2/SO3 above first":
       (v3>0&&after>0)?"PASS V-3 (a transient lock refusal was DECLINED AND RETRIED — "after" later open/publish succeeded — instead of destroying operator state) and PASS V-1/V-2/V-4 by survival: the boot continued past the bar, which is V-1'"'"'s only falsifier (its failure mode is a HANG inside the composite pass)":
       (v3>0&&after==0)?"V-3 DECLINED and NOTHING SUCCEEDED AFTER IT — the retry never landed; read the decline: "substr(vl,1,120):
       (ok>0)?"PASS by survival (V-1/V-2/V-4): the bar published/opened "ok"x and the boot continued past the composite pass. V-3 UNEXERCISED — no lock contention occurred, which is the common case.":
       "NO-VERDICT"}' "$B"

# ──────────────────────────── A10 CLASS / KEYDOORS-FIX (hw-jetson 1686268e) ──
# ⚠ THIS FLIPPED MID-SESSION. Written first as PENDING-FOLD on a grep at 8b696271 that found
# ONE `quarry::key_route(` call site; the tip then advanced to c24d9517 and the same grep finds
# SIX — the EL0 ring door (arch/aarch64/syscall.rs:13211), x86's wc_route_event
# (arch/x86_64/syscall.rs:6740), and all three shell drains (main.rs:2948, :4578, :7619).
# The fold IS aboard, so the verdicts below are PASS/FAIL, not "expected defect".
# Re-derive before the flight:  git grep -n 'quarry::key_route(' -- '*.rs'   (1 hit = not
# aboard, read the EXPECTED-DEFECT arms as the answer; 6 = aboard, read PASS/FAIL).
#   `[wc-c] focus tab-cycle {} -> {} (ring of {} + shell)` -- arch/aarch64/syscall.rs:13435
#   `[quarry] closed win={} paints={}`                     -- video/quarry/live.rs:1874
#   `[wm-act] drag-cancel win={} owner={:#x} at ({},{}) -> focus-key`
#       -- video/wm.rs:15876 shared [wm-act] format, verb wm.rs:16256, armed at
#          arch/aarch64/syscall.rs:13415 (DRAGREL-A64)
# ⚠ THE PAIRING MUST BE ORDER-AWARE, or it lies. render7 carries ONE KEY 0x1b (line 2437) and
# TWO `[quarry] closed` (9544, 11333) — those closes were the MOUSE on the close disc, seven
# thousand lines later. A bare presence test scores that as "Esc closed Quarry" and would have
# reported the defect as already fixed. The window below (WIN lines after the key) is what
# makes the check able to be WRONG, which is the only thing that makes it a check.
awk -v WIN=60 '
     /:: tegra: JD2 — KEY 0x09 ::/{tab++; lasttab=NR}
     /\[wc-c\] focus tab-cycle/{cyc++; if(lasttab && NR-lasttab<=WIN)cycpair++}
     /:: tegra: JD2 — KEY 0x1b ::/{esc++; lastesc=NR}
     /\[quarry\] closed win=/{qc++; if(lastesc && NR-lastesc<=WIN)qcpair++}
     /\[quarry\] open win=/{qo++}
     /\[wm-act\] drag-cancel /{dc++; if(/-> focus-key/){dcf++; dl=$0}}
     END{printf "A10-class keydoors (fold ABOARD at c24d9517): KEY 0x09=%d tab-cycle=%d (paired within %d lines=%d) | KEY 0x1b=%d quarry_closed=%d (paired=%d) quarry_open=%d | drag-cancel=%d focus-key=%d -> %s\n",tab,cyc,WIN,cycpair,esc,qc,qcpair,qo,dc,dcf,
       (tab==0&&esc==0)?"NOT-EXERCISED (neither TAB nor Esc pressed — FLIGHT SEQUENCE steps 10/10b are the stimulus)":
       (tab>0&&cycpair>0&&esc>0&&qo>0&&qcpair>0)?"PASS F0+F1 (TAB paired with a focus tab-cycle, and Esc paired with a [quarry] closed — both shell doors are wired for the first time on this board)":
       (tab>0&&cycpair==0)?"FAIL F0 ("tab" x KEY 0x09, NO paired tab-cycle within "WIN" lines — the TAB shell door is still dead. This is the reading the previous arc had; if the fold IS aboard (6 key_route sites) the comment-swallow has recurred, so read main.rs:2948 BY COLUMN, not by grep.":
       (esc>0&&qo>0&&qcpair==0)?"FAIL F1 (Quarry on the glass, "esc" x KEY 0x1b, NO paired [quarry] closed — the file manager still cannot be closed from the keyboard)":
       (tab>0&&cycpair>0&&esc==0)?"PASS F0; F1 NOT-EXERCISED (no Esc pressed with Quarry open)":
       (esc>0&&qcpair>0&&tab==0)?"PASS F1; F0 NOT-EXERCISED (no TAB pressed)":
       "PARTIAL — read the paired counts above against the gesture actually made"; if(dl) print "   " substr(dl,1,150)
       printf "   DRAGREL-A64: %s\n", dcf?"PASS ("dcf" drag-cancel -> focus-key: a focus switch cancelled a live drag on aarch64, the arm x86 has had since DRAGREL and this arch never did)":dc?"drag-cancel(s) present but NONE reason=focus-key — read the reasons":"NOT-EXERCISED (grab a title bar and press TAB without releasing — FLIGHT SEQUENCE step 10b)"
       print "   ⚠ SO9 (known, open, NOT a flight defect — LEDGER.md:58): while Quarry is on the glass it takes the shell'"'"'s ENTER and BACKSPACE (key_route gates on on_glass(), quarry/live.rs:1887, = OPEN not FOCUSED). A shell Enter opens a FILE. Close Quarry with Esc and the shell behaves. Do NOT file it as a shell bug."}' "$B"

echo "-- end render8 scorers --"
