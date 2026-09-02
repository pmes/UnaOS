#!/usr/bin/env bash
# knob-hygiene.sh — GATE-KNOB: every `feature = "X"` in a cfg is declared, and every declared
# feature is used by one.
#
# A cfg on an UNDECLARED feature is always false. It does not fail to build and it does not fail
# `check` — rustc emits `unexpected cfg condition value` and the gate throws the warning away. So the
# code under it is dead while READING as live, on every board, and the `not` arm is taken
# unconditionally. pi 6 found the first instance (`pidesk`, 7 sites in main.rs and video/menubar.rs
# on hw-pi4 — arch-neutral files, so it reaches x86 the moment that lands).
#
# ⚠ COMMENTS ARE STRIPPED FIRST, AND THAT IS THE WHOLE DIFFICULTY.
# The naive form of this check — grep the feature names out of the sources and set-difference them
# against [features] — was proposed as "pure set-difference, cannot false-positive". It can. Run it
# on hw-rmbp and it reports `pidesk` PHANTOM, on the strength of a DOC COMMENT at
# video/menubar.rs:77 that merely quotes the cfg expression in prose. A gate that reds on a sentence
# teaches people to disable it. Hence the `sed 's@//.*@@'` below, and hence the false-positive
# fixture in the go-red proof: prose mentioning a feature must stay GREEN.
#
# usage: knob-hygiene.sh
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO="$HERE/../crates/kernel/Cargo.toml"
SRC="$HERE/../crates/kernel/src"

# `default` is Cargo's own and is correct while empty: it is declared by the manifest format, not by
# anyone intending a knob, so it can never have a cfg site.
DEAD_OK="default"

declared="$(awk '/^\[features\]/{f=1;next} /^\[/{f=0} f && /^[A-Za-z0-9_-]+[ \t]*=/{sub(/[ \t]*=.*/,"");print}' "$CARGO" | sort -u)"
used="$(grep -rh --include='*.rs' -E 'feature[[:space:]]*=' "$SRC" \
        | sed -E 's@//.*@@' \
        | grep -oE 'feature[[:space:]]*=[[:space:]]*"[A-Za-z0-9_-]+"' \
        | sed -E 's/.*"([^"]+)"/\1/' | sort -u)"

# CONTROL PROBE, the same idea the knob->leg check uses with `vugpar`, and for the same reason: a
# regex that matched NOTHING would report "0 phantoms" and read as a clean tree. A zero has to be
# distinguishable from a broken pattern, so both sides must contain a feature that certainly exists.
for _probe in wc witness; do
  printf '%s\n' "$declared" | grep -qx "$_probe" || { echo "GATE-KNOB: control FAILED — '$_probe' not parsed out of [features]; the manifest parser is broken. No verdict." >&2; exit 2; }
  printf '%s\n' "$used"     | grep -qx "$_probe" || { echo "GATE-KNOB: control FAILED — '$_probe' not found in any cfg; the source scan is broken. No verdict." >&2; exit 2; }
done

phantom="$(comm -13 <(printf '%s\n' "$declared") <(printf '%s\n' "$used"))"
dead="$(comm -23 <(printf '%s\n' "$declared") <(printf '%s\n' "$used") | grep -vxF "$DEAD_OK" || true)"

rc=0
if [ -n "$phantom" ]; then
  rc=1
  echo "GATE-KNOB FAILED — cfg on an UNDECLARED feature (always false; the code under it is dead and reads as live):" >&2
  for f in $phantom; do
    echo "  PHANTOM: $f" >&2
    grep -rn --include='*.rs' -E "feature[[:space:]]*=[[:space:]]*\"$f\"" "$SRC" | sed -E 's@^@    @' | head -8 >&2
  done
  echo "  FIX: declare it in crates/kernel/Cargo.toml [features], or delete the cfg and keep the arm that was actually running." >&2
fi
if [ -n "$dead" ]; then
  rc=1
  echo "GATE-KNOB FAILED — feature declared but named by no cfg (a knob wired to nothing):" >&2
  for f in $dead; do echo "  DEAD: $f" >&2; done
fi
[ "$rc" -eq 0 ] && echo "GATE-KNOB: OK — $(printf '%s\n' "$declared" | grep -c .) features declared, $(printf '%s\n' "$used" | grep -c .) named by a cfg, 0 phantom, 0 dead"
exit $rc
