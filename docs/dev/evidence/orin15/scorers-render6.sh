awk '/ORIN-SMP-3 CPU_ON AP [0-9]+ .*-> SUCCESS/{s++} /CPU_ON AP [0-9]+ .*-> ERROR/{e++} /[0-9]+\/[0-9]+ secondaries online via PSCI CPU_ON/{o++} /Exception reason=1 syndrome=0x82000010/{x++} /Powering off core/{p++}
     END{printf "cpu_on_success=%d cpu_on_error=%d el3_abort=%d poweroff=%d online_line=%d -> %s\n",s,e,x,p,o,(s==5&&!e&&!x&&!p&&o)?"PASS (A15 pass +1)":(x||p)?"FAIL A15-signature (AP died in its MMU-off window)":"NO-VERDICT"}' "$B"
awk '/\[u7stk\] at=boot-core:post-cascade/{n++; for(i=1;i<=NF;i++){if($i~/^hw=/){sub(/hw=/,"",$i);hw=$i+0} if($i~/^headroom=/){sub(/headroom=/,"",$i);hr=$i+0} if($i~/^len=/){sub(/len=/,"",$i);len=$i+0}}} /\[deskcascade\] arming/{a++}
     END{printf "arming=%d post_cascade=%d len=%d hw=%d headroom=%d -> %s\n",a,n,len,hw,hr,n?(hr>0?"PASS (unsaturated)":"SATURATED — widen the boot-core window"):(a?"NO post-cascade probe after arming (overflow: the §5.2 stop-line case)":"NO CASCADE ARMED")}' "$B"
awk '/\[u7stk\] at=boot-core:(pre|post)-cascade/' "$B" | cut -c1-160
awk '/\[deskcascade\] -> CASCADED/{c++} /\[deskcascade\] REFUSE/{r++; rr=$0} /\[pulsewin\] open win=/{pw++; pl=$0} /\[pulsewin\] open DECLINE/{pd++; pl=$0} /\[orinrender\] strip=kept/{sk++}
     /\[orinrender\] census/{if(match($0,/strip=[a-z]+/))st=substr($0,RSTART+6,RLENGTH-6); if(match($0,/pulsewin=[0-9]+/))pv=substr($0,RSTART+9,RLENGTH-9)}   # <-- A18 CENSUS TOKEN SLOT: `strip={retired|live} pulsewin=<win>` as written by executor A18 at 13:17Z; re-check the token before scoring
     END{printf "cascaded=%d refuse=%d pulsewin_open=%d pulsewin_decline=%d strip_kept=%d census_strip=%s census_pulsewin=%s -> %s\n",c,r,pw,pd,sk,(st?st:"n/a"),(pv?pv:"n/a"),
     (c&&pw&&!pd&&!sk&&st=="retired")?"PASS":(c&&pd)?"FAIL pulsewin DECLINE":(c&&!pw)?"FAIL no [pulsewin] open on the cascaded scene":(sk||st=="live")?"FAIL strip still live":(r?"NO-CASCADE: "substr(rr,1,80):"NO-VERDICT")}' "$B"
awk '/\[pulsewin\] open|\[orinrender\] strip=|\[deskcascade\] ->/' "$B" | cut -c1-160
awk '/PRTSCR: PrintScreen \(HID 0x46\) down/{armed++} /:: PRTSCR: SCREEN[0-9]+\.PNG [0-9]+x[0-9]+ [0-9]+ bytes -> OK ::/{ok++; match($0,/SCREEN[0-9]+\.PNG/); names=names" "substr($0,RSTART,RLENGTH)} /:: PRTSCR: .*capture (skipped|INCOMPLETE)/{ref++; refs=refs" | "substr($0,1,90)}
     END{printf "armed=%d ok=%d refusals=%d names=[%s ] -> %s\n",armed,ok,ref,names,(armed>=2&&ok==2)?"PASS (two files; now verify the second on the card)":(armed>=2&&ok==1&&ref>=1)?"PASS-BY-REFUSAL (named):"refs:(armed>=2&&ok==1&&!ref)?"FAIL A17 (second press: no verdict — the render3b signature)":(armed<2)?"INCOMPLETE (<2 presses seen)":"NO-VERDICT"}' "$B"
awk '/:: PRTSCR:/' "$B" | cut -c1-140
# the card half (card back in the reader, mounted at $MP): every name the wire printed `-> OK` for
MP=/run/host/run/media/pmes/UNAOS-ORIN
python3 - "$MP"/SCREEN*.PNG <<'EOF'
import os,struct,sys
for p in sys.argv[1:]:
    b=open(p,'rb').read(33); n=os.path.getsize(p)
    ok=len(b)>=33 and b[:8]==b'\x89PNG\r\n\x1a\n' and b[12:16]==b'IHDR'
    w,h=struct.unpack('>II',b[16:24]) if ok else (0,0)
    print("%s size=%d sig+IHDR=%s %dx%d -> %s"%(p,n,ok,w,h,"VALID" if ok and n>0 else "INVALID"))
EOF
cp "$MP"/SCREEN*.PNG "$SCR"/ 2>&1; ls -la "$SCR"/SCREEN*.PNG
awk '/\[serialrx\] lsr=/{l++} /\[serialrx\] .*iir=/{ii++; il=$0} /\[serialrx\] rx=/{n++; for(i=1;i<=NF;i++){if($i~/^rx=/){sub(/rx=/,"",$i);rx=$i+0} if($i~/^ovrf=/){sub(/ovrf=/,"",$i);ov=$i+0;ovs=1}}} /:: tegra: JD2 — KEY /{k++}   # <-- A16 TOKEN SLOT: `iir=`/`fifo=` once, `ovrf=N` on the census, per the A16 brief; re-check against serial.rs at $SHA
     END{printf "lsr_lines=%d iir_lines=%d census=%d rx_final=%d ovrf_final=%s keys=%d -> %s\n",l,ii,n,rx,(ovs?ov:"ABSENT"),k,(ii==1&&ovs)?"DISCRIMINATORS PRESENT — verdict per A16-SCORE.md":"DISCRIMINATORS ABSENT (A16 bytes not aboard, or the token changed — fix the slot)"; if(il) print substr(il,1,160)}' "$B"
# per-leg windows (L0 = orin.log lines before the burst, L1 = before the paced leg; from B.5/B.6 or marks.txt):
for leg in "burst $L0 $L1" "paced $L1 999999999"; do set -- $leg
  tr -d "\000" < "$CAP/orin.log" | awk -v a="$2" -v b="$3" -v leg="$1" 'NR>a && NR<=b && /:: tegra: JD2 — KEY /{k++; ks=ks" "$0} NR>a && NR<=b && /\[serialrx\] rx=/{for(i=1;i<=NF;i++){if($i~/^rx=/){sub(/rx=/,"",$i);rx=$i+0} if($i~/^ovrf=/){sub(/ovrf=/,"",$i);ov=$i+0}}}
       END{printf "%s: keys=%d rx_after=%d ovrf_after=%d\n",leg,k,rx,ov; gsub(/:: tegra: JD2 — /,"",ks); print "   " ks}'
done
awk '/AARCH64: timer heartbeat live/{h++} /JM6b — EL1 landing: CurrentEL=1/{e++} /\[orinrender\] arm conwin=/{t++} /RENDER-ARMED/{ra++} /RENDER-LIVE/{rl++} /\[redzone\] .*LOW-REDZONE/{rz++} /Exception reason=|panicked at|PANIC:/{x++}
     END{printf "heartbeat=%d el1=%d arm=%d armed=%d live=%d redzone=%d exceptions=%d -> %s\n",h,e,t,ra,rl,rz,x,(h&&e&&t&&ra&&rl&&!rz&&!x)?"PASS":"INCOMPLETE"}' "$B"
# ---- render6 additions (orin 15) ----
# A20 clicks: arm -> ARMED, and a press routed -> CONSUMED
awk '/\[orinclick\] arm .*-> ARMED/{a++} /\[orinrender\] arm .*click=1/{c1++} /\[clickroute\] press/{p++; pl=$0} /\[orinclick\] edge=press.*-> CONSUMED/{k++} /\[orinrender\] census.*-> ROUTING/{rt++}
     END{printf "arm_click1=%d orinclick_armed=%d clickroute_press=%d consumed=%d routing_census=%d -> %s\n",c1,a,p,k,rt,(a&&p&&k)?"PASS (A20 flown)":(a&&!p)?"ARMED, NO CLICK SEEN (press not made or not routed)":(!a)?"NOT ARMED (knob missing?)":"NO-VERDICT"; if(pl) print "   " substr(pl,1,150)}' "$B"
# A22 TCU RX mailbox: arm line + census FULL state after the burst (TCURX-DESIGN.md §7 rows)
awk '/\[tcu\] hsp top0=/{arm++} /\[tcu\] STOP/{stop++} /\[tcu\] rx-mbox/{n++; l=$0; for(i=1;i<=NF;i++){if($i~/^full=/){sub(/full=/,"",$i);f=$i+0} if($i~/^nbytes=/){sub(/nbytes=/,"",$i);nb=$i+0} if($i~/^full-edges=/){sub(/full-edges=/,"",$i);fe=$i+0} if($i~/^changes=/){sub(/changes=/,"",$i);ch=$i+0}} if(match($0,/data=\[[0-9a-f ]+\]/))dt=substr($0,RSTART,RLENGTH)}
     END{printf "arm=%d stop=%d census=%d full_final=%d nbytes=%d full_edges=%d changes=%d %s -> %s\n",arm,stop,n,f,nb,fe,ch,dt,(stop)?"STOP at arm (DTB shape)":(f&&nb>=1)?"ROW1: SPE forwards RX into the mailbox and parks it — TCURX rung 2":(fe>0&&!f)?"ROW2: FULL-SEEN then consumed — find the other consumer":(!fe&&!ch)?"ROW3: FULL-NEVER — forwarding not on unprompted":"NO-VERDICT"}' "$B"
# A19 wire half (the PNG half is A19-pngband.py on SCREEN0.PNG from the card)
awk '/\[realdesk\] band-cleared/{b++; bl=$0} /\[realdesk\] shell-present/{s++} /\[u7stk\] at=jd2-console:shell-present/{u++}
     END{printf "band_cleared=%d shell_present=%d jd2_probe=%d -> %s\n",b,s,u,(b&&s)?"WIRE PASS (now A19-pngband.py SCREEN0.PNG must read non-bg=0/60200)":"FAIL A19 wire"; if(bl) print "   " substr(bl,1,140)}' "$B"
# A10 Esc on the pulse window menu: a menu open then a dismiss by Esc
awk '/SHARD-MENU.*open|pulsewin.*menu.*open|\[pulsewin\] menu/{o++; ol=$0} /dismiss reason=esc|dismiss reason=escape|KEY 0x1b|Esc/{e++; el=$0} /dismiss reason=/{d++; dl=$0}
     END{printf "menu_open=%d esc_seen=%d dismiss=%d -> %s\n",o,e,d,(o&&e&&d)?"CANDIDATE PASS — read the dismiss line":(o&&!d)?"MENU OPEN, NO DISMISS (A10 stands)":(!o)?"NO MENU OPENED (press not made)":"NO-VERDICT"; if(ol) print "   open: " substr(ol,1,120); if(dl) print "   dismiss: " substr(dl,1,120)}' "$B"
