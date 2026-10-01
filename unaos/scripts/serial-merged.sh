#!/usr/bin/env bash
# SERIALLOCK M3 — count wire lines that carry TWO `:: ` witness heads (two witnesses merged because
# the serial writer had no line lock). Usage: serial-merged.sh <log>. Prints the count; exit 0 when it
# is 0, exit 1 otherwise. Boot 17 read 197; boot 18 (with `serial_line::emit`) should read 0.
# A witness head is `:: NAME:` (NAME = upper-case letters, digits, `-`, `_`, `/`); the closing
# ` :: ` of a verdict does not match because no NAME: follows it.
set -u
log="${1:?usage: serial-merged.sh <log>}"
n=$(LC_ALL=C awk '{
  c = 0; s = $0
  while (match(s, /:: [A-Z][A-Z0-9_\/-]*:/)) { c++; s = substr(s, RSTART + RLENGTH) }
  if (c >= 2) { m++; if (m <= 5) printf("  e.g. line %d: %.140s\n", NR, $0) > "/dev/stderr" }
} END { print m + 0 }' "$log")
echo "$n"
[ "$n" -eq 0 ]
