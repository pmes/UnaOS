#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# verb-alias-closure.sh [arroyo-path]  — GATE-VERBALIAS (orin session, 2026-09-13).
#
# A FEATURE GATE KEYS ON THE BUILD TARGET, NEVER ON ONE SPELLING OF ITS NAME. `arroyo`'s dispatcher
# accepts ALIASES — `esp-jetson|esp_jetson|jetson`, `test|test-x86|test_x86`, `kernel8|baremetal` —
# and every spelling in a group runs the SAME build. So any guard elsewhere in the script that keys
# on `$1` must name the WHOLE group, or it silently splits one build into two feature sets.
#
# THIS IS NOT HYPOTHETICAL AND IT COST A POWER CYCLE. `arroyo:1147` armed `ga10bprobe5a` with
# `[ "${1:-}" = "esp-jetson" ]` while SIX spellings reach that build; five of them dropped the
# ignition, kept `ga10bprobe5`, and printed a feature banner naming a rung they were not building.
# Fixed at 2e48d721 BY HAND, found by certifying the artifact, and caught by nobody's instrument —
# which is what this script is. Against the pre-fix tree it reds on exactly that line; against the
# fixed tree it passes. Both measured, in that order.
#
# WHAT IT DOES. Parse the dispatcher (the LAST top-level `case "${1:-}" in` in the file) into alias
# GROUPS, then scan every other `$1`-keyed guard — multi-line `case` arms, one-line inline `case …
# esac` guards, and `[ "$1" = "…" ]` equality tests alike — and report any guard that names SOME
# spellings of a group and not all of them.
#
# WHAT A ZERO MEANS HERE, and why the census line prints on every run (LAWS §5: "is a zero a fact
# about the data or about the pattern?"): a clean exit also prints the dispatcher's line, how many
# alias groups it parsed and how many guards it examined. A run that finds no dispatcher, no groups
# or no guards exits 2 — NO VERDICT — because that is the pattern breaking, not the file being clean.
#
# TRAILING COMMENTS ARE STRIPPED BEFORE ANY MATCH, and that is load-bearing rather than tidy:
# `arroyo:1147` quotes its own former `[ "${1:-}" = "esp-jetson" ]` in the comment that records the
# fix, so without the strip this script reds on its own documentation — a check firing on the note
# that says the defect is gone. Measured both ways while writing it.
#
# WHAT IT CANNOT SEE, stated so a green is read for what it is. It knows the dispatcher's GROUPS and
# nothing about what the arms DO, so it cannot tell that `esp_jetson_img()` is nothing but a call to
# `esp_jetson` — it reports `esp-jetson|esp_jetson|jetson` and `esp-jetson-img|esp_jetson_img|
# jetson-img` as two groups, which is why the pre-fix run below says "3 ways" where the commit
# message says six. Three is enough to red, and the six is a fact about the CALL GRAPH that belongs
# in the fix, not in this parse. It also sees only `$1`: a guard keyed on a variable COPIED from `$1`
# is outside its scope, and there is none in `arroyo` today (measured: the only top-level `${1`
# references are the four at :49, :1147, :2157 and the dispatcher).
#
# Exit 0 = every $1-keyed guard is alias-closed · 1 = at least one is not (each printed with
# file:line) · 2 = no verdict (file unreadable, dispatcher not found, or nothing to examine).
set -u
F="${1:-$(cd "$(dirname "$0")/.." && pwd)/arroyo}"
[ -r "$F" ] || { echo "VERBALIAS -> NO VERDICT: cannot read $F"; exit 2; }

awk -v file="$F" '
function isverb(s) { return (s ~ /^[A-Za-z0-9_-]+$/) }
# Record one guard arm: which case block it belongs to (0 = an inline one-liner), its line, its label.
function arm(cid, ln, label) { arm_case[++na] = cid; arm_line[na] = ln; arm_txt[na] = label }

/^[[:space:]]*#/ { next }
{
    line = $0
    sub(/[[:space:]]#.*$/, "", line)          # trailing comment — see the header note
    if (line ~ /^#/) next

    # ── a `case "$1" in` opening ───────────────────────────────────────────────────────────────
    if (line ~ /case[[:space:]]+"\$\{?1(:-)?\}?"[[:space:]]+in/) {
        nc++; c_line[nc] = FNR
        if (line ~ /^case[[:space:]]/) last_top = nc          # column 0 => a top-level case
        rest = line; sub(/^.*case[[:space:]]+"\$\{?1(:-)?\}?"[[:space:]]+in/, "", rest)
        if (line ~ /(^|[[:space:];])esac([[:space:];]|$)/) {
            # INLINE guard: `…; case "$1" in a|b) x ;; esac; …` — arms between `in` and `esac`.
            sub(/esac.*$/, "", rest)
            n = split(rest, S, ";;")
            for (j = 1; j <= n; j++) if (match(S[j], /[A-Za-z0-9_|*"-]+\)/)) arm(nc, FNR, substr(S[j], RSTART, RLENGTH - 1))
            inline_case[nc] = 1
        } else {
            in_case = nc
            if (match(rest, /[A-Za-z0-9_|*"-]+\)/)) arm(nc, FNR, substr(rest, RSTART, RLENGTH - 1))
        }
    } else if (in_case) {
        if (line ~ /(^|[[:space:];])esac([[:space:];]|$)/) { in_case = 0 }
        else if (match(line, /^[[:space:]]*[A-Za-z0-9_|*"-]+\)/)) {
            lbl = substr(line, RSTART, RLENGTH - 1); gsub(/^[[:space:]]+/, "", lbl); arm(in_case, FNR, lbl)
        }
    }

    # ── a `[ "$1" = "x" ]` / `[[ "$1" == x ]]` equality test ───────────────────────────────────
    l = line
    while (match(l, /"\$\{?1(:-)?\}?"[[:space:]]*==?[[:space:]]*"?[A-Za-z0-9_-]+"?/)) {
        m = substr(l, RSTART, RLENGTH); l = substr(l, RSTART + RLENGTH)
        v = m; sub(/^.*==?[[:space:]]*/, "", v); gsub(/"/, "", v)
        if (isverb(v)) { eq_line[++ne] = FNR; eq_verb[ne] = v }
    }
}

END {
    if (last_top == 0) { print "VERBALIAS -> NO VERDICT: no top-level `case \"${1:-}\" in` found in " file; exit 2 }
    for (i = 1; i <= na; i++) {
        if (arm_case[i] != last_top) continue
        t = arm_txt[i]; gsub(/"/, "", t)
        if (t == "*" || t == "") continue
        n = split(t, A, "|"); gid++
        for (j = 1; j <= n; j++) if (isverb(A[j])) { group[A[j]] = gid; members[gid] = members[gid] (members[gid] ? "|" : "") A[j]; gsize[gid]++ }
    }
    if (gid == 0) { print "VERBALIAS -> NO VERDICT: the dispatcher at line " c_line[last_top] " parsed to zero alias groups"; exit 2 }

    bad = 0; examined = 0
    for (i = 1; i <= na; i++) {
        if (arm_case[i] == last_top) continue
        t = arm_txt[i]; gsub(/"/, "", t)
        if (t == "*" || t == "") continue
        n = split(t, A, "|"); delete named; delete want; any = 0
        for (j = 1; j <= n; j++) { named[A[j]] = 1; if (A[j] in group) { want[group[A[j]]] = 1; any = 1 } }
        if (!any) continue
        examined++
        for (g in want) {
            n2 = split(members[g], M, "|"); miss = ""; have = 0
            for (k = 1; k <= n2; k++) { if (M[k] in named) have++; else miss = miss (miss ? "," : "") M[k] }
            if (miss != "") {
                printf("%s:%d: a $1-keyed `case` arm names %d of the %d spellings of the `%s` verb group — MISSING: %s\n", file, arm_line[i], have, n2, M[1], miss)
                bad++
            }
        }
    }
    for (i = 1; i <= ne; i++) {
        if (!(eq_verb[i] in group)) continue
        examined++
        g = group[eq_verb[i]]
        if (gsize[g] > 1) {
            n2 = split(members[g], M, "|"); miss = ""
            for (k = 1; k <= n2; k++) if (M[k] != eq_verb[i]) miss = miss (miss ? "," : "") M[k]
            printf("%s:%d: a $1-keyed STRING EQUALITY on \"%s\" gates a verb the dispatcher accepts %d ways — MISSING: %s. Enumerate the aliases in a `case`; a feature gate keys on the BUILD TARGET, never on one spelling of its name.\n", file, eq_line[i], eq_verb[i], n2, miss)
            bad++
        }
    }
    if (examined == 0) { printf("VERBALIAS -> NO VERDICT: %d alias group(s) parsed from the dispatcher at line %d, but NOT ONE $1-keyed guard was found to examine — the scan pattern is broken, not the file.\n", gid, c_line[last_top]); exit 2 }
    printf("VERBALIAS dispatcher=%d groups=%d guards=%d violations=%d -> %s\n", c_line[last_top], gid, examined, bad, (bad ? "FAIL" : "PASS"))
    exit (bad ? 1 : 0)
}
' "$F"
