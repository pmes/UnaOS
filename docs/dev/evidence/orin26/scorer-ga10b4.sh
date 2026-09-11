#!/usr/bin/env bash
# scorer-ga10b4.sh — score a GA10B rung-4 capture (brief docs/dev/OS/08_VIDEO/GA10B-RUNG4-BRIEF.md §3, §4).
#   usage: scorer-ga10b4.sh <log> [4a|4b]     exit 0 = the rung's PASS arm present and STOP-check clean
#                                             exit 1 = not present / STOP-check red   exit 2 = no verdict
# Bounds are the §4 table's: full 10-char `br_retcode=0x########` literals (row A), substring-exclusive
# arm names (row B), case-sensitive `about-to-WRITE ` / `about-to-read ` with the trailing space (row C),
# and the `post-ignition ` phase token inside the bound (row D). awk only, LC_ALL=C: control bytes.
set -u
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
  *) echo "NO VERDICT: rung must be 4a or 4b"; exit 2 ;;
esac
[ "$wa" = "$wr" ] || { echo "RED write_announces != write_results"; red=1; }
[ "$stop" = ok ] || red=1
[ $red = 0 ] && echo "PASS $RUNG" || echo "FAIL $RUNG"
exit $red
