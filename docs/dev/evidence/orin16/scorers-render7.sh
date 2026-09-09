#!/usr/bin/env bash
# scorers-render7.sh — score the render7 flight the minute the boot ends.
#
#   usage: scorers-render7.sh <boot-log-file> [burstA burstB pacedA pacedB]
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
# follow them, each with the source file:line of the format string it matches,
# read at hw-jetson 37c78ad7 (kernel source = 7b143041, the render7 image).
#
# THE QUESTIONS (render7-20260906T0445Z-7be8155/MANIFEST):
#   A15 A16/TCURX2 A27 A8 A25/R21 A26 A20 A17 A1

set -u
B="${1:?usage: scorers-render7.sh <boot-log-file> [burstA burstB pacedA pacedB]}"
L0="${2:-}"; L1="${3:-}"; L2="${4:-}"; L3="${5:-}"
[ -r "$B" ] || { echo "scorers-render7: cannot read $B" >&2; exit 2; }

echo "== render7 scorers over $B ($(wc -l < "$B") lines) =="

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
#   `[winmenu] publish owner={} titles={} items={} slot={} replaced={}` -- video/winmenu.rs:327
#   `[winmenu] publish REFUSE ...`                                     -- video/winmenu.rs:256,261,270,278,288,318
#   `[winmenu] open title={} items={} at ({},{}) owner={}`             -- video/winmenu.rs:634
#   `[winmenu] pick owner={} id={} label={}`                           -- video/winmenu.rs:728
#   `[winmenu] dismiss reason={} owner={}`  reason in {outside,esc,pick,title,clear,owner-change}
#                                                                      -- video/winmenu.rs:650
#   `[winmenu] clear owner={}`                                         -- video/winmenu.rs:364
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
#   OK:    `:: PRTSCR: {} {}x{} {} bytes -> OK ::` -- video/prtscr.rs:178 and prtscr.rs:546
#   THE NAMED IN-FLIGHT REFUSAL (the render6 gap — four fast presses printed nothing):
#          `:: PRTSCR: refused — capture in flight (another task holds the capture door; a key
#           request is re-armed and runs after it) ::` -- video/prtscr.rs:276
#           (Refusal::InFlight, minted at prtscr.rs:387, reported at prtscr.rs:191)
#   other refusals: `:: PRTSCR: ... capture skipped ::` / `capture INCOMPLETE ::` -- prtscr.rs:240-272
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
