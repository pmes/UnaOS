#!/usr/bin/env bash
# friend-norm-strict.sh — rmbp 17's PRE-REGISTERED set (B90): strip ONLY (a) monotonic counters and timestamps,
# (b) addresses and handles. Nothing else; NO line drops; order preserved. Mount witnesses are exempted by the
# caller (friend-diff.sh judge), not here. Unwrap of the 80-column loader console is a console-width fact.
bash "$HOME/unaos-bench/tools/unwrap80.sh" "$1" 2>/dev/null | LC_ALL=C tr -d '\000' \
| LC_ALL=C sed -E \
    -e 's/\x1b\[[0-9;]*m//g' \
    -e 's/\[[0-9]{4}\.[0-9]{3}\]/[T]/g' \
    -e 's/\[TS:[0-9]+\]/[TS]/g' \
    -e 's/0x[0-9a-f_]+/0xA/g' \
    -e 's/=-?[0-9]+(\.[0-9]+)?/=N/g' \
    -e 's/[0-9]+(ms|us|cyc|s|Hz|%)\b/N\1/g' \
    -e 's/\b[0-9]{4,}\b/N/g' \
    -e 's/ [0-9]+\/[0-9]+ / N\/N /g'
