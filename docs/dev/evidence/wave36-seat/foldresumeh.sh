#!/bin/bash
# foldresumeh <name> <SRid> "<title>" "<test command>" : like foldh but the merge is already staged and composed.
n=$1; id=$2; title=$3; tcmd=$4
W=/root/unaos-bench/scratch/cloud/wave36
cd /home/user/UnaOS || exit 9
source $W/foldlib.sh
uu=$(git status --short | grep '^UU' | awk '{print $2}')
for f in $uu; do c=$(grep -c '^<<<<<<<' "$f"); [ "$c" != 0 ] && { echo "STOP markers in $f"; exit 2; }; done
[ -z "$uu" ] || { echo "STOP unresolved: $uu"; exit 2; }
[ "$(grep -c "$id" docs/dev/LEDGER.md)" -ge 1 ] || { echo "STOP no $id ledger row"; exit 3; }
[ "$(grep -c "$id" docs/dev/blueprint/handlers.md)" -ge 1 ] || { echo "STOP no $id clause in handlers.md"; exit 3; }
python3 unaos/scripts/blueprint-check.py 2>&1 | tail -1 | grep -q PASS || { python3 unaos/scripts/blueprint-check.py 2>&1 | tail -3; echo "STOP blueprint"; exit 4; }
cargo metadata --locked --offline >/dev/null 2>&1 || { echo "STOP lockfile"; exit 5; }
export PKG_CONFIG_PATH=$W/nle1/alsa/pc CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 TMPDIR=$W/tmp
for c in $(git diff --cached --name-only | grep "/src/\|/tests/" | sed "s#/\(src\|tests\)/.*##" | sort -u); do [ -f "$c/src/lib.rs" ] && touch "$c/src/lib.rs"; [ -f "$c/src/main.rs" ] && touch "$c/src/main.rs"; [ -d "$c/tests" ] && find "$c/tests" -name "*.rs" -exec touch {} +; done # stale shared-target rlibs and test binaries
bash -c "$tcmd" > $W/$n-fold.log 2>&1; rc=$?
grep -E '^test result|^error|panicked|Finished' $W/$n-fold.log | sort | uniq -c | head -12
[ $rc = 0 ] || { echo "STOP red rc=$rc"; exit 6; }
foldcommit $n $id "$title" 2>&1 | tail -1
git worktree remove --force /home/user/exec/$n 2>/dev/null; git worktree prune
. /root/unaos-bench/scratch/cloud/wave36/proxy.env 2>/dev/null; git push -u origin claude/optimistic-ramanujan-r3qyu5 2>&1 | tail -1
git log --oneline -1 | cut -c1-60
