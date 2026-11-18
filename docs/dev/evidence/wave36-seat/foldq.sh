#!/bin/bash
# foldq: a persistent fold queue. Jobs in $W/foldq.txt, one per line: name|SRid|title|test cmd|row prefix (fields separated by |).
# On a STOP the job line is written to $W/foldq.stopped and the runner waits; the seat composes, stages, then
# `mv foldq.stopped foldq.resume` (resume the same job via foldresumeh) or `rm foldq.stopped` (skip it).
W=/root/unaos-bench/scratch/cloud/wave36; cd /home/user/UnaOS
export PKG_CONFIG_PATH=$W/nle1/alsa/pc CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 TMPDIR=$W/tmp
M="CARGO_TARGET_DIR=$W/host-target-fold-media"; A="CARGO_TARGET_DIR=$W/host-target-fold-aether"
while true; do
  if [ -f $W/foldq.resume ]; then
    IFS='|' read -r n id t c r < $W/foldq.resume; rm -f $W/foldq.resume
    c=$(eval echo "\"$c\"")
    echo "== RESUME $id $(date -u +%H:%MZ)"; bash $W/foldresumeh.sh "$n" "$id" "$t" "$c"; rc=$?; echo "== $id rc=$rc"
    [ $rc = 0 ] || { echo "$n|$id|$t|$c|$r" > $W/foldq.stopped; echo "STOPPED $id"; }
    continue
  fi
  [ -f $W/foldq.stopped ] && { sleep 20; continue; }
  line=$(head -1 $W/foldq.txt 2>/dev/null); [ -z "$line" ] && { sleep 30; continue; }
  sed -i 1d $W/foldq.txt
  IFS='|' read -r n id t c r <<< "$line"; c=$(eval echo "\"$c\"")
  echo "== FOLD $id $(date -u +%H:%MZ)"; bash $W/foldh.sh "$n" "$id" "$t" "$c" "$r"; rc=$?; echo "== $id rc=$rc"
  [ $rc = 0 ] || { echo "$line" > $W/foldq.stopped; echo "STOPPED $id"; }
done
