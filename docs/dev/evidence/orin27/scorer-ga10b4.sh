#!/usr/bin/env bash
# scorer-ga10b4.sh — score a GA10B rung-4 capture (brief docs/dev/OS/08_VIDEO/GA10B-RUNG4-BRIEF.md §3, §4, §10).
#   usage: scorer-ga10b4.sh <log> [4a|4b|4c]  exit 0 = the rung's PASS arm present and STOP-check clean
#                                             exit 1 = not present / STOP-check red   exit 2 = no verdict
#          scorer-ga10b4.sh --selftest <real-4a+4b-log> [scratch-dir]
#                                             the 4c go-red proof, from files: builds an expected-4c wire by
#                                             appending 4c lines to a copy of the real capture, then mutates
#                                             it four ways; prints one line per case and exits 0 only if every
#                                             case scored as expected (fixtures left in the scratch dir).
# orin 27 (2026-09-12): copied from docs/dev/evidence/orin26/scorer-ga10b4.sh (left untouched) and extended
# with the `4c` rung. 4c keys on `[ga10bprobe4c]` lines only, never on their position in the capture, so the
# immediate (`=3`) and the deferred (`=4`, from the shutdown path) placements score identically.
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
  bad=0
  run() { local name="$1" file="$2" want="$3" got; "$ME" "$file" 4c > "$SCR/$name.out" 2>&1; got=$?; echo "$name rung=4c expect=$want got=$got $([ "$got" = "$want" ] && echo OK || echo MISMATCH)"; [ "$got" = "$want" ] || bad=1; }
  run real-4a4b-only "$REAL" 2
  run expected-4c "$W" 0
  run m1-no-result "$SCR/m1-no-result.log" 1
  run m2-no-off "$SCR/m2-no-off.log" 1
  run m3-hang "$SCR/m3-hang.log" 1
  run m4-no-complete "$SCR/m4-no-complete.log" 1
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
  *) echo "NO VERDICT: rung must be 4a, 4b or 4c"; exit 2 ;;
esac
[ "$wa" = "$wr" ] || { echo "RED write_announces != write_results"; red=1; }
[ "$stop" = ok ] || red=1
[ $red = 0 ] && echo "PASS $RUNG" || echo "FAIL $RUNG"
exit $red
