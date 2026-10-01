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
  if (c >= 2) {
    m++
    if (m <= 10) {
      # name the two heads and where the first one was cut (SERIAL2 M1)
      t = $0; h1 = ""; h2 = ""; p2 = 0
      if (match(t, /:: [A-Z][A-Z0-9_\/-]*:/)) { h1 = substr(t, RSTART, RLENGTH); r1 = RSTART; t2 = substr(t, RSTART + RLENGTH); off = RSTART + RLENGTH - 1
        if (match(t2, /:: [A-Z][A-Z0-9_\/-]*:/)) { h2 = substr(t2, RSTART, RLENGTH); p2 = off + RSTART } }
      printf("  merged #%d line %d: head1=%s head2=%s head1_cut_at=%d len=%d :: %.120s\n", m, NR, h1, h2, p2 - 1, length($0), $0) > "/dev/stderr"
    }
  }
} END { print m + 0 }' "$log")
echo "$n"
[ "$n" -eq 0 ]
