# verdicts.awk — one "family<TAB>verdict" row per verdict line of a serial capture.
# family = the leading `:: NAME:` or `[tag]` (after an optional `[   t.ttt]` logts stamp);
# verdict = the LAST of PASS/FAIL/SKIP/SKIPPED/REFUSED/OK on the line, as a whole word.
{
  s = $0; sub(/\r$/, "", s)
  sub(/^\[ *[0-9]+\.[0-9]+\] */, "", s)
  if (s !~ /(^|[^A-Za-z])(PASS|FAIL|SKIP|SKIPPED)([^A-Za-z]|$)/) next
  fam = s
  if (match(s, /^:: *[A-Za-z0-9_.-]+[ A-Za-z0-9_-]*:/)) fam = substr(s, 1, RLENGTH)
  else if (match(s, /^\[[^]]+\]/)) fam = substr(s, 1, RLENGTH)
  else { split(s, w, " "); fam = w[1] }
  v = ""; t = s
  while (match(t, /(PASS|FAIL|SKIPPED|SKIP)/)) { v = substr(t, RSTART, RLENGTH); t = substr(t, RSTART + RLENGTH) }
  print fam "\t" v
}
