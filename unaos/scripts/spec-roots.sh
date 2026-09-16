#!/usr/bin/env bash
# spec-roots.sh — GATE-SPECROOTS: every replay spec in scripts/specs/ NAMES THE COMMAND THAT RUNS IT.
#
# WHY THIS EXISTS (SPECRUN, rmbp seat, 2026-09-15). This is GATE-ROOTS' shape — "a binary no check
# leg names is never type-checked" — applied to the other kind of root in this tree. A `.spec` file
# is nobody's dependency either: unless some command REPLAYS it, it is a file full of regexes that
# no run ever evaluates. `scripts/specs/x86-fat.spec` was exactly that for its whole life: 32 pinned
# witness lines, named in two COMMENTS in `arroyo` and in no line of code, so
#
#   * a kernel change that reddened those pins shipped GREEN (the pins could not fire), and
#   * an executor who RESPECTED the pins paid for them anyway — STORWAIT added a SECOND
#     `storage settle:` witness line rather than edit the one `x86-fat.spec:156` pinned verbatim.
#
# That is the whole cost profile of an unrun spec: it can never catch a regression, and it can still
# bend the kernel around itself. A replay spec no command runs is a silent landmine.
#
# HOW IT DECIDES. Two ways for a spec to have a runner, and exactly two:
#
#   GATED     `arroyo` names `specs/<file>` on a line that is not a full-line comment — i.e. a VERB
#             replays it and its rc is a gate's rc. Full-line comments are stripped first, which is
#             the whole point: the two prose mentions of x86-fat.spec were what made it LOOK run.
#
#   DECLARED  the spec's own header carries a `# RUN-BY: <who> — <command>` line naming who replays
#             it and how. This is for the BENCH specs, which assert METAL captures a QEMU verb
#             cannot produce (rmbp-boot, round6-rmbp, x86-witness, x86-wc, x86-holocron, x86-wifival,
#             jetson-jd5) and for specs a separate script consumes (jetson-sync1 -> orin-specscore.py).
#             `<who>` is `bench` (a human at a METAL bench runs the command), `knobleg` (a QEMU leg
#             behind a named knob that the default gate does not arm) or `script:<path>` — and a
#             `script:` path that does not exist is RED, so the declaration cannot rot into fiction.
#
#             `verb:<name>` is NOT accepted as a declaration: a spec that claims a verb runs it must
#             be findable in `arroyo`'s code, or the claim is false and this gate says so. That is
#             the teeth on the honour system — the one form of RUN-BY a file could use to lie about
#             being gated is the one form cross-checked against the script.
#
# Neither -> ORPHAN -> exit 1, by name. The fix is to wire it into a verb or to declare its bench
# command; deleting it is also a fix, and an honest one for a spec nobody has run in a year.
#
# CONTROL PROBES, the GATE-ROOTS idiom: a resolver that matched EVERYTHING would call every spec
# gated (silently green forever), and one that matched NOTHING would red every spec (loud, but it
# would also be "fixed" by declaring RUN-BY everywhere). Both are ruled out on synthetic input whose
# answer is known — one file mentioning a spec ONLY in a full-line comment MUST come back not-gated,
# one mentioning it in code MUST come back gated — plus one fact about this tree the parser has to
# rediscover: `pi4-regression.spec` is replayed by `kernel8-test` in code and must resolve GATED.
# Any control failure is exit 2 and NO verdict.
#
# usage: spec-roots.sh [workspace-dir]   (0 every spec has a runner; 1 an orphan; 2 control failed)
set -uo pipefail

WS="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
ARROYO="${WS}/arroyo"
SPECDIR="${WS}/scripts/specs"

[ -f "$ARROYO" ] || { echo "GATE-SPECROOTS: control FAILED — no arroyo at ${ARROYO}. No verdict." >&2; exit 2; }
[ -d "$SPECDIR" ] || { echo "GATE-SPECROOTS: control FAILED — no ${SPECDIR}. No verdict." >&2; exit 2; }

# ── 1. The resolver, as a function, so the controls exercise THE SAME CODE the verdict uses ───────
# `is_gated <arroyo-file> <spec-basename>` -> 0 when the file names specs/<basename> outside a
# full-line comment. awk, not grep: arroyo is UTF-8 prose and the match is a fixed substring.
is_gated() { # $1 = file to read, $2 = spec basename
  awk -v want="specs/$2" '
    { line = $0 }
    line ~ /^[ \t]*#/ { next }          # a full-line comment is PROSE, not a call
    index(line, want) { found = 1; exit }
    END { exit (found ? 0 : 1) }
  ' "$1"
}

# `run_by <spec>` -> echoes the `<who>` token of its `# RUN-BY:` header line, or nothing.
run_by() {
  awk '
    /^[ \t]*#[ \t]*RUN-BY:/ {
      sub(/^[ \t]*#[ \t]*RUN-BY:[ \t]*/, "")
      # <who> is the first whitespace-delimited token; the rest is the command, for the reader.
      split($0, a, /[ \t]/); print a[1]; exit
    }
  ' "$1"
}

# ── 2. Control probes ─────────────────────────────────────────────────────────────────────────────
_ctl="$(mktemp)"; trap 'rm -f "$_ctl"' EXIT
printf '%s\n' '# a comment that merely mentions scripts/specs/CONTROL-A.spec' > "$_ctl"
if is_gated "$_ctl" "CONTROL-A.spec"; then
  echo "GATE-SPECROOTS: control FAILED — a spec named ONLY in a full-line comment resolved as GATED; the comment stripper is broken. No verdict." >&2; exit 2
fi
printf '%s\n' 'FOO="${WORKSPACE_DIR}/scripts/specs/CONTROL-A.spec"' > "$_ctl"
if ! is_gated "$_ctl" "CONTROL-A.spec"; then
  echo "GATE-SPECROOTS: control FAILED — a spec named in CODE resolved as NOT gated; the resolver matches nothing. No verdict." >&2; exit 2
fi
if [ -f "${SPECDIR}/pi4-regression.spec" ] && ! is_gated "$ARROYO" "pi4-regression.spec"; then
  echo "GATE-SPECROOTS: control FAILED — pi4-regression.spec is replayed by kernel8-test in arroyo and did not resolve GATED; the arroyo reader is broken. No verdict." >&2; exit 2
fi

shopt -s nullglob
specs=("$SPECDIR"/*.spec)
shopt -u nullglob
[ "${#specs[@]}" -gt 0 ] || { echo "GATE-SPECROOTS: control FAILED — zero .spec files enumerated under ${SPECDIR}. No verdict." >&2; exit 2; }

# ── 3. Verdict ────────────────────────────────────────────────────────────────────────────────────
rc=0
orphans=""
echo "GATE-SPECROOTS: replay specs and the command that runs each —"
for f in "${specs[@]}"; do
  b="$(basename "$f")"
  who="$(run_by "$f")"
  if is_gated "$ARROYO" "$b"; then
    printf '  %-24s %s\n' "$b" "GATED (arroyo names it in code)"
    continue
  fi
  case "$who" in
    "")
      printf '  %-24s %s\n' "$b" "ORPHAN — no verb replays it and it declares no RUN-BY"
      orphans="${orphans} ${b}"; rc=1 ;;
    verb:*)
      printf '  %-24s %s\n' "$b" "FALSE CLAIM — header says '${who}' but arroyo names it nowhere in code"
      orphans="${orphans} ${b}"; rc=1 ;;
    script:*)
      sp="${who#script:}"
      if [ -f "${WS}/${sp}" ] || [ -f "$sp" ]; then
        printf '  %-24s %s\n' "$b" "DECLARED (${who})"
      else
        printf '  %-24s %s\n' "$b" "DEAD DECLARATION — ${who} does not exist"
        orphans="${orphans} ${b}"; rc=1
      fi ;;
    bench)
      printf '  %-24s %s\n' "$b" "DECLARED (bench — hand-run against a metal capture)" ;;
    knobleg)
      printf '  %-24s %s\n' "$b" "DECLARED (knobleg — hand-run QEMU leg behind a named knob)" ;;
    *)
      printf '  %-24s %s\n' "$b" "BAD RUN-BY '${who}' — want bench | knobleg | script:<path>"
      orphans="${orphans} ${b}"; rc=1 ;;
  esac
done

if [ "$rc" -ne 0 ]; then
  cat >&2 <<MSG
GATE-SPECROOTS FAILED — replay spec(s) that no command runs:${orphans}

A spec is its own root. Nothing depends on it, so unless a verb replays it or its header names the
bench command that does, no run ever evaluates its pinned lines — and it still costs, because the
next executor to touch the kernel line it pins will respect a pin that cannot fire (SPECRUN).
  FIX: replay it from the verb whose capture it describes (see \`x86_spec_replay\` in arroyo), or
       add a header line naming the command that does:
         # RUN-BY: bench — ./arroyo mbench --follow ~/<board>-serial.log --spec scripts/specs/<f> --timeout 300
         # RUN-BY: knobleg — <the knob-armed QEMU command, then the mbench --replay that scores it>
         # RUN-BY: script:scripts/<tool>.py — <the exact invocation>
       Deleting a spec nobody has run is also a fix, and an honest one.
MSG
fi
[ "$rc" -eq 0 ] && echo "GATE-SPECROOTS: OK — ${#specs[@]} replay specs, every one named by a verb or a declared command"
exit $rc
