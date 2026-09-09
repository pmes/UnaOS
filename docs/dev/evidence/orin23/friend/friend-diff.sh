#!/usr/bin/env bash
# friend-diff.sh — FRIEND-DIFF (FLIGHT-render11 §6, three-boot form). orin 23, 2026-09-09.
#   usage: friend-diff.sh calibrate <A1.log> <A2.log>     # same condition: every surviving line is a
#                                                          #   timing/counter field the normalizer missed —
#                                                          #   add it to friend-norm.sh, never to the allowed list
#          friend-diff.sh judge <A1.log> <B.log>           # A1 friend ABSENT vs B friend PRESENT: survivors are
#                                                          #   entanglements unless on the ALLOWED list below
# The ALLOWED list is FROZEN from FLIGHT §6 and is applied only in `judge`, never in `calibrate`.
set -uo pipefail
D=$(cd "$(dirname "$0")" && pwd)
mode="$1"; A="$2"; B="$3"
allowed() {  # lines that are permitted to differ between the two CONDITIONS (a friend disk exists or not)
  LC_ALL=C awk '!( index($0,"[vfs] volume mounted") || index($0,"[vfs] unafs volume") || index($0,"TEGRA-UNAFS") \
               || index($0,"[quarry] open volumes") || index($0,":: SDMMC:") || index($0,"[sdmmc]") \
               || (index($0,"[vfs] root") && (index($0,"matches=")||index($0,"home=")||index($0,"disks="))) \
               || index($0,"tegra-sd") || index($0,"[part]") || index($0,":: PART:") \
               || index($0,":: TEGRA-SD:") || index($0,":: PSRC:") || index($0,"=== butler") )'
}
case "$mode" in
  calibrate) diff <("$D/friend-norm.sh" "$A") <("$D/friend-norm.sh" "$B") ;;
  judge)     diff <("$D/friend-norm.sh" "$A" | allowed) <("$D/friend-norm.sh" "$B" | allowed) ;;
  *) echo "usage: friend-diff.sh calibrate|judge <a.log> <b.log>" >&2; exit 2 ;;
esac
rc=$?; echo "== friend-diff $mode: diff exit=$rc (0 = EMPTY) =="; exit $rc
