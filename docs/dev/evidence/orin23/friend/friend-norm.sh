#!/usr/bin/env bash
# friend-norm.sh — FRIEND-DIFF normalizer (FLIGHT-render11 §6, three-boot form). orin 23.
#   usage: friend-norm.sh <wire.log>  > normalized
# RULE: a field may be normalised ONLY if it varies between two boots in the SAME condition (A1 vs A2).
# The list below is the STARTING set from FLIGHT §6; the first A1-vs-A2 diff adds to THIS list, never to
# the allowed-diff list. Lines that are ALLOWED to differ between conditions (A vs B) are removed by the
# caller with friend-allowed.awk, not here.
bash "$HOME/unaos-bench/tools/unwrap80.sh" "$1" 2>/dev/null | LC_ALL=C tr -d '\000' \
| LC_ALL=C sed -E \
    -e 's/scan=[0-9]+cyc\/[0-9]+us/scan=N/g; s/paint=[0-9]+cyc\/[0-9]+us/paint=N/g; s/rate=[0-9]+\/1k/rate=N/g' \
    -e 's/\x1b\[[0-9;]*m//g' \
    -e 's/\[[0-9]{4}\.[0-9]{3}\]/[T]/g' \
    -e 's/(probe_us|list_us|us|rx|polls|budget|used|peak|hw|headroom|took|t|seq|passes|presents|parked|rearm|reports|decoded|folded|foldnew|up|samples|redraws|skipped|srcdelta|gapmax|lat_max_ms|census|dropped|cycles|last_disp|window_off|sp|low|top|at)=[-0-9a-fx]+/\1=N/g' \
    -e 's/\(\+[0-9]+\)/(+N)/g' \
    -e 's/rate=[0-9.]+\/s/rate=N/g; s/srate=[0-9.]+\/s/srate=N/g' \
    -e 's/tick [0-9]+/tick N/g' \
    -e 's/@ 0x[0-9a-f]+/@ 0xN/g; s/at 0x[0-9a-f]+/at 0xN/g' \
    -e 's/[a-z]+=[0-9]+ms/T=N/g' \
    -e '/JB6: dummy ACPI/d; /\[T\] I> /d' \
    -e 's/(sGFAR|SYNR0|SYNR1|sid|FAR|SCTLR|FSR|status|DAIF|HCR_EL2)=0x[0-9a-f_]+/\1=0xN/g' \
    -e 's/[0-9]+ (cyc|polls|us|ms)\b/N \1/g; s/\([0-9]+ polls\)/(N polls)/g; s/within [0-9]+ ms/within N ms/g' \
    -e 's/ Fail[^:]*rc=:0x[0-9a-f]+/ Fail rc=0xN/g' \
| LC_ALL=C grep -v $'\xef\xbf\xbd' \
| LC_ALL=C awk '!index($0,"[serialrx]") && !index($0,":: SCHED:") && !index($0,"[spread") && !index($0,"[pulse5]") && !index($0,"[wc-") && !index($0,"[wcn]") && !index($0,"[wcpar]") && !index($0,"[prio]") && !index($0,"[noatt]") && !index($0,"[fluid3]") && !index($0,"[comp2]") && !index($0,"[strip] ") && !index($0,"[dock] live") && !index($0,"[orinbsptick] tick") && !index($0,"[tcu] rx-mbox") && !index($0,"[rxmerge] census") && !index($0,"[menubar] menus") && !index($0,"[crystal] rollup") && !index($0,"[ptrpoll]") && !index($0,"[kbdpoll]") && !index($0,"[orinclick] census") && !index($0,"[orinrender] census") && !index($0,"[el0live]") && !index($0,"[pstrip] rollup")'
