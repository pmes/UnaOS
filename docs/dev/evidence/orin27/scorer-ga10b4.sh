#!/usr/bin/env bash
# scorer-ga10b4.sh — score a GA10B rung-4 capture (brief docs/dev/OS/08_VIDEO/GA10B-RUNG4-BRIEF.md §3, §4, §10, §12).
#   usage: scorer-ga10b4.sh <log> [4a|4b|4c|4e|4f]  exit 0 = the rung's PASS arm present and STOP-check clean
#                                             exit 1 = not present / STOP-check red   exit 2 = no verdict
#          scorer-ga10b4.sh --selftest <real-4a+4b-log> [scratch-dir]
#                                             the 4c and 4e/4f go-red proofs, from files: builds the expected
#                                             wires from a copy of the real capture, then mutates them; prints
#                                             one line per case and exits 0 only if every case scored as
#                                             expected (fixtures left in the scratch dir).
# orin 27 (2026-09-12): copied from docs/dev/evidence/orin26/scorer-ga10b4.sh (left untouched) and extended
# with the `4c` rung. 4c keys on `[ga10bprobe4c]` lines only, never on their position in the capture, so the
# immediate (`=3`) and the deferred (`=4`, from the shutdown path) placements score identically.
# orin-0912b (2026-09-12, ledger A62): extended with the `4e` and `4f` rungs — the two one-boot arms of
# rung 4 (brief §12, `UNAOS_GA10B_PROBE4=5` / `=6`). They score the SAME oracles as 4a+4b and are told apart
# by the tag the rung prints on its own summary lines: `shift=8` (4e) and `brfetch=false` (4f). 4e also
# checks the ARITHMETIC — every address write carries `raw_pa=` and the value written must be that address
# shifted right by 8 — so the leg measures the encoding, not the label.
# Bounds are the §4 table's: full 10-char `br_retcode=0x########` literals (row A), substring-exclusive
# arm names (row B), case-sensitive `about-to-WRITE ` / `about-to-read ` with the trailing space (row C),
# and the `post-ignition ` phase token inside the bound (row D). awk only, LC_ALL=C: control bytes.
set -u
if [ "${1:-}" = "--selftest" ]; then
  REAL="${2:-}"; SCR="${3:-$HOME/unaos-bench/scratch/orin27/ga10b4c-logs/scorer-selftest}"
  [ -s "$REAL" ] || { echo "NO VERDICT: --selftest needs the real 4a+4b capture"; exit 2; }
  mkdir -p "$SCR"; export LC_ALL=C
  ME="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
  # The expected 4c wire (brief §10 shape), one line per announce/result pair, spliced BEFORE the
  # [pwrshutoff] lines of the real capture so the SYSTEM_OFF stays last — exactly where 4c prints on metal.
  W="$SCR/expected-4c.log"
  awk 'index($0,"[pwrshutoff]")==0' "$REAL" > "$W"
  {
    echo "[ga10bprobe4c] pass 1 — PRE-ignition census of 30 registers (after 4a's restore, before 4b; read-only)"
    echo "[ga10bprobe4c] about-to-read pre-ignition fuse_opt_sec_debug_en reg=0x17821040 — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it"
    echo "[ga10bprobe4c] pre-ignition fuse_opt_sec_debug_en @0x821040 = 0x00000000"
    echo "[ga10bprobe4c] about-to-read pre-ignition gsp_falcon_dmactl reg=0x1711010c — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it"
    echo "[ga10bprobe4c] pre-ignition gsp_falcon_dmactl @0x11010c = -UNREADABLE reason=pri-error val=0xbadf5620"
    echo "[ga10bprobe4c] pre-ignition census: readable=21/30 unreadable=9 -> PRECENSUS-DONE"
    echo "[ga10bprobe4c] pass 2 — POST-ignition: br_retcode series, then the 30-register census diffed against pass 1, then the DMA window (read-only; 4b's SYSTEM_OFF follows)"
    echo "[ga10bprobe4c] about-to-read series priscv_br_retcode reg=0x1711165c samples=20 settle_ms=10 — if this is the LAST line, THAT read was EL3-fatal"
    echo "[ga10bprobe4c] series sample=1/20 t_ms=0 br_retcode=0x00000002 br_result=0x2 reason_bits=0x00000000 (distinct #1)"
    echo "[ga10bprobe4c] series priscv_br_retcode @0x65c = distinct=1 first=0x00000002@1 last=0x00000002@1 reason_bits_or=0x00000000 samples=20 elapsed_ms=201 -> BRSERIES-STABLE"
    echo "[ga10bprobe4c] about-to-read post-ignition priscv_bcr_ctrl reg=0x17111668 — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it"
    echo "[ga10bprobe4c] post-ignition priscv_bcr_ctrl @0x111668 = 0x00000111 diff=written-by-4b pre=0x00000110 intact=1"
    echo "[ga10bprobe4c] post-ignition priscv_bcr_ctrl expected=0x00000111 intact=1"
    echo "[ga10bprobe4c] about-to-read post-ignition gsp_falcon_cpuctl_v1 reg=0x17110100 — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it"
    echo "[ga10bprobe4c] post-ignition gsp_falcon_cpuctl_v1 @0x110100 = -UNREADABLE reason=pri-error val=0xbadf5620 diff=same pre=0xbadf5620"
    echo "[ga10bprobe4c] bcr post-ignition: intact=8/8 altered=0 unreadable=0 -> POSTBCR-INTACT"
    echo "[ga10bprobe4c] post-ignition census: readable=21/30 unreadable=9 became_readable=0 became_unreadable=0 changed=1 changed_ex_bcr_retcode=0 pre_done=1 -> MAPDIFF-SAME"
    echo "[ga10bprobe4c] about-to-read post-ignition dmabuf-scan pa=0x80200000 size=0x200000 (every word of the window, CPU side, compared to the fill pattern) — if this is the LAST line, THAT read was fatal"
    echo "[ga10bprobe4c] post-ignition dmabuf-scan @0x80200000 = words_changed=0/524288 first_changed_off=none -> DMABUF-UNTOUCHED"
    echo "[ga10bprobe4c] rung 4c complete: reads_announced=6 reads_answered=6 (zero writes) -> CENSUS-COMPLETE"
  } >> "$W"
  awk 'index($0,"[pwrshutoff]")' "$REAL" >> "$W"
  # Mutations: (m1) an announced read with no result line; (m2) the SYSTEM_OFF never reached; (m3) the
  # capture truncated at a 4c announce (F1 hang); (m4) the CENSUS-COMPLETE line missing.
  awk 'index($0,"[ga10bprobe4c] post-ignition gsp_falcon_cpuctl_v1 @0x110100")==0' "$W" > "$SCR/m1-no-result.log"
  awk 'index($0,"PSCI SYSTEM_OFF")==0' "$W" > "$SCR/m2-no-off.log"
  awk '{print} index($0,"[ga10bprobe4c] about-to-read post-ignition dmabuf-scan"){exit}' "$W" > "$SCR/m3-hang.log"
  awk 'index($0,"-> CENSUS-COMPLETE")==0' "$W" > "$SCR/m4-no-complete.log"
  # ── The expected 4e and 4f wires (brief §12) ────────────────────────────────────────────────────────
  # Built by TRANSFORMING the real =2 capture, never by hand-typing a second wire: the transformation
  # cannot change the announce/result balance or the STOP-check, so what these fixtures test is the arm's
  # own delta and nothing else. The rung's own dmabuf_pa on that capture is 0x80200000, so the shifted
  # values are the ones a =5 boot of the same board must print.
  E4E="$SCR/expected-4e.log"; E4F="$SCR/expected-4f.log"
  awk -v BAN="[ga10bprobe4e] rung 4e ARMED (UNAOS_GA10B_PROBE4=5) — fixture wire: the flown =2 capture with the six BCR DMA addresses re-encoded pa >> 8 and the 4a/4b summary lines tagged shift=8" '
    BEGIN{ raw["fmccode"]="0x80200000"; sh["fmccode"]="0x00802000";
           raw["fmcdata"]="0x80280000"; sh["fmcdata"]="0x00802800";
           raw["pkcparam"]="0x80300000"; sh["pkcparam"]="0x00803000"; pend=0; ban=0 }
    {
      L=$0
      if (ban==0 && index(L,"[ga10bprobe4a]")) { print BAN; ban=1 }
      if (index(L,"about-to-WRITE ") && index(L,"BCR DMA address")) {
        k = index(L,"fmccode") ? "fmccode" : (index(L,"fmcdata") ? "fmcdata" : "pkcparam")
        nv = index(L,"_lo reg=") ? sh[k] : "0x00000000"
        sub(/val=0x[0-9a-f]+/, "val=" nv " raw_pa=" raw[k] " shift=8", L)
        pend=1; pr=raw[k]; pv=nv
      } else if (pend==1 && index(L," wrote=0x")) {
        sub(/wrote=0x[0-9a-f]+ read=0x[0-9a-f]+/, "wrote=" pv " read=" pv " raw_pa=" pr " shift=8", L)
        pend=0
      }
      if (index(L,"[ga10bprobe4a]") && index(L,"-> BCR-ALLHELD")) sub(/ lock_after=/, " shift=8 lock_after=", L)
      if (index(L,"[ga10bprobe4b]") && index(L,"v1_readable=") && index(L,"samples=")) sub(/ -> /, " shift=8 -> ", L)
      print L
    }' "$REAL" > "$E4E"
  awk -v BAN="[ga10bprobe4f] rung 4f ARMED (UNAOS_GA10B_PROBE4=6) — fixture wire: the flown =2 capture with bcr_ctrl written 0x00000011 and the 4a/4b summary lines tagged brfetch=false" '
    BEGIN{ ban=0 }
    {
      L=$0
      if (ban==0 && index(L,"[ga10bprobe4a]")) { print BAN; ban=1 }
      if (index(L,"about-to-WRITE priscv_bcr_ctrl ")) sub(/val=0x00000111/, "val=0x00000011 brfetch=false", L)
      else if (index(L,"[ga10bprobe4b] priscv_bcr_ctrl @")) gsub(/0x00000111/, "0x00000011", L)
      if (index(L,"[ga10bprobe4a]") && index(L,"-> BCR-ALLHELD")) sub(/ lock_after=/, " brfetch=false lock_after=", L)
      if (index(L,"[ga10bprobe4b]") && index(L,"v1_readable=") && index(L,"samples=")) sub(/ -> /, " brfetch=false -> ", L)
      print L
    }' "$REAL" > "$E4F"
  # (m5) the 4e wire with the 4b verdict's tag removed — the arm is not on the summary line;
  # (m6) the 4e wire with ONE written value off by one — the tag is there and the ARITHMETIC is wrong,
  #      which is the case a tag-only leg would pass and this one must not;
  # (m7) the 4f wire with bcr_ctrl back at 0x00000111 — the tag claims BRFETCH FALSE, the register says
  #      otherwise, and the leg must believe the register.
  awk '{ if (index($0,"[ga10bprobe4b]") && index($0,"v1_readable=") && index($0,"samples=")) sub(/ shift=8 -> /, " -> "); print }' "$E4E" > "$SCR/m5-4e-untagged.log"
  awk '{ if (index($0,"[ga10bprobe4b] priscv_bcr_fmccode_lo @")) sub(/wrote=0x00802000/, "wrote=0x00802001"); print }' "$E4E" > "$SCR/m6-4e-badshift.log"
  awk '{ if (index($0,"[ga10bprobe4b] priscv_bcr_ctrl @")) gsub(/0x00000011/, "0x00000111"); print }' "$E4F" > "$SCR/m7-4f-noctrl.log"
  bad=0
  run() { local name="$1" file="$2" rung="$3" want="$4" got; "$ME" "$file" "$rung" > "$SCR/$name.out" 2>&1; got=$?; echo "$name rung=$rung expect=$want got=$got $([ "$got" = "$want" ] && echo OK || echo MISMATCH)"; [ "$got" = "$want" ] || bad=1; }
  run real-4a4b-only "$REAL" 4c 2
  run expected-4c "$W" 4c 0
  run m1-no-result "$SCR/m1-no-result.log" 4c 1
  run m2-no-off "$SCR/m2-no-off.log" 4c 1
  run m3-hang "$SCR/m3-hang.log" 4c 1
  run m4-no-complete "$SCR/m4-no-complete.log" 4c 1
  # 4e / 4f: two wires each (brief §4) — the real one that must hit, and the sibling that must not.
  run expected-4e "$E4E" 4e 0
  run expected-4f "$E4F" 4f 0
  run real-untagged-as-4e "$REAL" 4e 1
  run real-untagged-as-4f "$REAL" 4f 1
  run expected-4e-as-4f "$E4E" 4f 1
  run expected-4f-as-4e "$E4F" 4e 1
  run m5-4e-untagged "$SCR/m5-4e-untagged.log" 4e 1
  run m6-4e-badshift "$SCR/m6-4e-badshift.log" 4e 1
  run m7-4f-noctrl "$SCR/m7-4f-noctrl.log" 4f 1
  # and the flown legs must still score the new wires: an arm is a tag on the SAME rung, not a new rung.
  run expected-4e-as-4a "$E4E" 4a 0
  run expected-4e-as-4b "$E4E" 4b 0
  echo "fixtures: $SCR"; [ $bad = 0 ] && echo "SELFTEST PASS" || echo "SELFTEST FAIL"; exit $bad
fi
LOG="${1:-}"; RUNG="${2:-4a}"
[ -s "$LOG" ] || { echo "NO VERDICT: no log"; exit 2; }
export LC_ALL=C
cnt() { awk -v p="$1" 'index($0,p){n++} END{print n+0}' "$LOG"; }
last_announce() { awk '/\[ga10bprobe4/{l=$0} END{ if (index(l,"about-to-WRITE ")||index(l,"about-to-read ")) print "RED last [ga10bprobe4*] line is an announce (F1 hang): " substr(l,1,160); else print "ok" }' "$LOG"; }
wa=$(cnt "about-to-WRITE "); ra=$(cnt "about-to-read "); wr=$(cnt " wrote=0x")
echo "write_announces=$wa read_announces=$ra write_results=$wr"
stop=$(last_announce); echo "STOP-CHECK: $stop"
red=0
case "$RUNG" in
  4a)
    for arm in "-> BCR-ALLHELD" "-> BCR-SOMEHELD" "-> BCR-NONEHELD" "-> BCR-SELFLOCKED" "-> BCR-STICKY" "-> REFUSED reason="; do
      n=$(awk -v p="$arm" 'index($0,"[ga10bprobe4a]") && index($0,p){n++} END{print n+0}' "$LOG"); echo "arm $arm = $n"; done
    all=$(awk 'index($0,"[ga10bprobe4a]") && index($0,"-> BCR-ALLHELD"){n++} END{print n+0}' "$LOG")
    [ "$all" = 1 ] || red=1 ;;
  4b)
    for arm in "-> BROM-VERDICT-FAIL" "-> BROM-VERDICT-PASS" "-> BROM-NOVERDICT" "-> BCR-CTRL-REFUSED" "-> IGNITION-SKIPPED"; do
      n=$(awk -v p="$arm" 'index($0,"[ga10bprobe4b]") && index($0,p){n++} END{print n+0}' "$LOG"); echo "arm $arm = $n"; done
    fail=$(awk 'index($0,"[ga10bprobe4b]") && index($0,"br_retcode=0x00000002") && index($0,"-> BROM-VERDICT-FAIL"){n++} END{print n+0}' "$LOG")
    post=$(awk 'index($0,"[ga10bprobe4b] post-ignition gsp_falcon_cpuctl_v1"){n++} END{print n+0}' "$LOG")
    echo "post-ignition v1 lines = $post (must be exactly 1)"
    [ "$fail" = 1 ] && [ "$post" = 1 ] || red=1 ;;
  4c)
    # 4c (brief §10): PASS = every announced 4c read has its result line, the census reached
    # CENSUS-COMPLETE exactly once, and the flight reached SYSTEM_OFF. No 4c lines at all = NO VERDICT
    # (a 4a+4b-only capture is not a 4c failure). Position-free: the deferred placement scores the same.
    n4c=$(cnt "[ga10bprobe4c]")
    [ "$n4c" -gt 0 ] || { echo "NO VERDICT: no [ga10bprobe4c] lines in the capture (4c not armed)"; exit 2; }
    ra4c=$(cnt "[ga10bprobe4c] about-to-read ")
    rr4c=$(awk 'index($0,"[ga10bprobe4c]") && index($0," @0x") && index($0," = "){n++} END{print n+0}' "$LOG")
    echo "4c read_announces=$ra4c read_results=$rr4c"
    for arm in "-> PRECENSUS-DONE" "-> PRECENSUS-SKIPPED" "-> BRSERIES-STABLE" "-> BRSERIES-CHANGED" "-> BRSERIES-UNREADABLE" "-> POSTBCR-INTACT" "-> POSTBCR-ALTERED" "-> POSTBCR-UNREADABLE" "-> MAPDIFF-SAME" "-> MAPDIFF-CHANGED" "-> DMABUF-UNTOUCHED" "-> DMABUF-ALTERED" "-> CENSUS-COMPLETE"; do
      n=$(awk -v p="$arm" 'index($0,"[ga10bprobe4c]") && index($0,p){n++} END{print n+0}' "$LOG"); echo "arm $arm = $n"; done
    done4c=$(awk 'index($0,"[ga10bprobe4c]") && index($0,"-> CENSUS-COMPLETE"){n++} END{print n+0}' "$LOG")
    off=$(cnt "PSCI SYSTEM_OFF (0x84000008) via SMC")
    echo "CENSUS-COMPLETE lines = $done4c (must be exactly 1); SYSTEM_OFF lines = $off (must be >= 1)"
    [ "$ra4c" -gt 0 ] && [ "$ra4c" = "$rr4c" ] || { echo "RED 4c read_announces != read_results (an announced read has no result line)"; red=1; }
    [ "$done4c" = 1 ] || { echo "RED 4c did not reach CENSUS-COMPLETE exactly once"; red=1; }
    [ "$off" -ge 1 ] || { echo "RED the flight did not reach PSCI SYSTEM_OFF"; red=1; } ;;
  4e|4f)
    # RUNGS 4e / 4f (brief §12): the two one-boot arms of rung 4. Same registers, same bounding rules,
    # same oracles as 4a+4b — what tells the arms apart is the TAG the rung prints on its OWN summary
    # lines, `shift=8` (4e) or `brfetch=false` (4f), plus the arm's ARMED banner.
    #
    # A capture without the tag is RED, never NO VERDICT. That is deliberate and it is the difference
    # between this leg and the 4c one: a 4a+4b (`=2`) wire is a wire this arm DID NOT FLY, and there are
    # two such wires already in evidence/. A leg that answered NO VERDICT on them could never come out
    # red from a real capture either, and a check that cannot fire is an absent one (LAWS §Gates).
    #
    # The rung's PASS here is NOT a particular verdict. `br_retcode` cannot be this arm's oracle —
    # a correctly encoded but unsigned payload returns the same 0x00000002 (rung-5 brief §2.5) — so what
    # this leg asserts is that the arm FLEW AND WAS MEASURED: the tagged 4a census reached BCR-ALLHELD,
    # the tagged 4b summary reached one of its five arms with post_lockdown= and v1_readable= beside it,
    # the post-ignition v1 read happened exactly once, and the machine went off. Which way the verdict
    # fell is printed, never required; brief §12 names the fail shapes.
    if [ "$RUNG" = 4e ]; then TAG="shift=8"; BAN="[ga10bprobe4e]"; else TAG="brfetch=false"; BAN="[ga10bprobe4f]"; fi
    nban=$(cnt "$BAN"); ntag=$(cnt "$TAG")
    echo "arm banner $BAN = $nban (must be >= 1); lines carrying '$TAG' = $ntag"
    a4=$(awk -v t="$TAG" 'index($0,"[ga10bprobe4a]") && index($0,"-> BCR-ALLHELD") && index($0,t){n++} END{print n+0}' "$LOG")
    echo "4a BCR-ALLHELD lines carrying '$TAG' = $a4 (must be exactly 1)"
    for arm in "-> BROM-VERDICT-FAIL" "-> BROM-VERDICT-PASS" "-> BROM-NOVERDICT" "-> BCR-CTRL-REFUSED" "-> IGNITION-SKIPPED"; do
      n=$(awk -v p="$arm" -v t="$TAG" 'index($0,"[ga10bprobe4b]") && index($0,p) && index($0,t){n++} END{print n+0}' "$LOG"); echo "arm $arm = $n"; done
    v4=$(awk -v t="$TAG" 'index($0,"[ga10bprobe4b]") && index($0,"br_retcode=0x") && index($0,"samples=") && index($0,"post_lockdown=") && index($0,"v1_readable=") && index($0,t){n++} END{print n+0}' "$LOG")
    echo "4b summary lines carrying '$TAG' with post_lockdown= and v1_readable= = $v4 (must be exactly 1)"
    post=$(awk 'index($0,"[ga10bprobe4b] post-ignition gsp_falcon_cpuctl_v1"){n++} END{print n+0}' "$LOG")
    lock=$(awk 'index($0,"[ga10bprobe4b] post-ignition hwcfg2 lockdown="){n++} END{print n+0}' "$LOG")
    off=$(cnt "PSCI SYSTEM_OFF (0x84000008) via SMC")
    echo "post-ignition v1 lines = $post (must be exactly 1); post-ignition lockdown lines = $lock (must be exactly 1); SYSTEM_OFF lines = $off (must be >= 1)"
    if [ "$RUNG" = 4e ]; then
      # THE ARM'S OWN ARITHMETIC, not its label. Every address write result carries `raw_pa=` beside
      # `wrote=`, and the value written must be that physical address >> 8, split lo/hi. Hex is parsed
      # in awk rather than by strtonum so the leg does not depend on a gawk extension.
      enc=$(awk 'function hx(s,  i,c,d,n){sub(/^0[xX]/,"",s); n=0; for(i=1;i<=length(s);i++){c=tolower(substr(s,i,1)); d=index("0123456789abcdef",c)-1; if(d<0) return -1; n=n*16+d} return n}
        index($0,"[ga10bprobe4")==0 { next }
        index($0," raw_pa=0x")==0 || index($0," wrote=0x")==0 { next }
        { w=-1; r=-1
          for(i=1;i<=NF;i++){ if(substr($i,1,8)=="wrote=0x") w=hx(substr($i,7)); if(substr($i,1,9)=="raw_pa=0x") r=hx(substr($i,8)) }
          ok++
          if(w<0||r<0){ bad++; next }
          s=int(r/256); lo=s%4294967296; hi=int(s/4294967296)
          if(index($0,"_lo @")){ if(w!=lo) bad++ }
          else if(index($0,"_hi @")){ if(w!=hi) bad++ }
          else { bad++ } }
        END{ printf "%d %d", ok+0, bad+0 }' "$LOG")
      echo "4e shift arithmetic: writes_with_raw_pa=${enc% *} wrong=${enc#* } (wrote must equal raw_pa >> 8, lo/hi split)"
      [ "${enc% *}" -ge 6 ] || { echo "RED 4e: fewer than 6 address writes carry raw_pa= — the arm did not announce what it shifted"; red=1; }
      [ "${enc#* }" = 0 ] || { echo "RED 4e: an address write does not equal raw_pa >> 8"; red=1; }
    else
      ctrl=$(awk 'index($0,"[ga10bprobe4b] priscv_bcr_ctrl ") && index($0,"wrote=0x00000011 "){n++} END{print n+0}' "$LOG")
      echo "4f bcr_ctrl writes of 0x00000011 (BRFETCH FALSE) = $ctrl (must be exactly 1)"
      [ "$ctrl" = 1 ] || { echo "RED 4f: bcr_ctrl was not written 0x00000011 exactly once — the tag claims BRFETCH FALSE and the register does not"; red=1; }
    fi
    # Rung 4c is NOT carried by =5/=6, so its three post-ignition oracles are REPORTED always and
    # REQUIRED only where the capture actually carries 4c — the same oracles, scored where they exist
    # instead of demanded where they cannot run.
    n4c=$(cnt "[ga10bprobe4c]")
    for arm in "-> POSTBCR-" "-> MAPDIFF-" "-> DMABUF-" "-> CENSUS-COMPLETE"; do
      n=$(awk -v p="$arm" 'index($0,"[ga10bprobe4c]") && index($0,p){n++} END{print n+0}' "$LOG"); echo "4c oracle $arm = $n"; done
    echo "4c lines in this capture = $n4c (0 is the expected =5/=6 shape)"
    if [ "$n4c" -gt 0 ]; then
      for arm in "-> POSTBCR-" "-> MAPDIFF-" "-> DMABUF-" "-> CENSUS-COMPLETE"; do
        n=$(awk -v p="$arm" 'index($0,"[ga10bprobe4c]") && index($0,p){n++} END{print n+0}' "$LOG")
        [ "$n" -ge 1 ] || { echo "RED 4c is armed in this capture but $arm never reached a verdict"; red=1; }
      done
    fi
    [ "$nban" -ge 1 ] || { echo "RED the $BAN ARMED banner is absent — this capture did not fly rung $RUNG"; red=1; }
    [ "$a4" = 1 ] || { echo "RED no single [ga10bprobe4a] BCR-ALLHELD line carrying '$TAG'"; red=1; }
    [ "$v4" = 1 ] || { echo "RED no single [ga10bprobe4b] summary line carrying '$TAG' with post_lockdown= and v1_readable="; red=1; }
    [ "$post" = 1 ] || { echo "RED post-ignition gsp_falcon_cpuctl_v1 is not present exactly once"; red=1; }
    [ "$lock" = 1 ] || { echo "RED post-ignition hwcfg2 lockdown is not present exactly once"; red=1; }
    [ "$off" -ge 1 ] || { echo "RED the flight did not reach PSCI SYSTEM_OFF"; red=1; } ;;
  *) echo "NO VERDICT: rung must be 4a, 4b, 4c, 4e or 4f"; exit 2 ;;
esac
[ "$wa" = "$wr" ] || { echo "RED write_announces != write_results"; red=1; }
[ "$stop" = ok ] || red=1
[ $red = 0 ] && echo "PASS $RUNG" || echo "FAIL $RUNG"
exit $red
