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

# TRAILING-COMMENT PHANTOM (orin 13, 2026-09-05, LEDGER P7): a `#[cfg(...)]` appended AFTER a line's
# trailing `//` comment is prose. It compiles nothing and `check` stays green — PRTSCR-ORIN shipped that
# way for two hours; a union merge did it again to ORINRX's census fold. Red when CODE precedes the `//`
# on the line (so a pure comment or doc-comment line that quotes a cfg expression stays GREEN — the
# prose fixture again), and `#[cfg(` sits after it.
# Shape of the hazard, not the word: CODE before the `//`, then `#[cfg(...)]`, then a STATEMENT — an
# identifier with a call and a `;` (or a block `{`) — still on the same line. A `} // end #[cfg(...)] block`
# note or a comment that merely mentions a cfg has no statement after the attribute and stays green.
#
# ⚠ THE END-OF-LINE ANCHOR WAS REMOVED (KEYDOORS F0, orin 16, 2026-09-06), AND THAT REMOVAL IS THE
# WHOLE POINT OF THIS PARAGRAPH. Until then the pattern ended `...(\(.*\).*;|\{)[[:space:]]*$`, i.e.
# the buried statement had to be the LAST thing on the line. main.rs:2948 was a fold carrying TWO
# comments: A10's prose was written at column 139, ahead of TABKEY's `#[cfg(feature = "tegra_el0")]
# if …wc_shell_focus_key(ev) { continue; }` at column ~1600, and TABKEY's own prose ran on after it.
# The buried call therefore had a second `//` behind it, the `$` did not match, and this gate — the
# one check in the tree aimed squarely at this hazard — reported OK on a LIVE regression that had
# silently disabled <TAB> on the Orin for an entire arc. Nothing else could see it either: the line
# count never moved (so the kernel8.img byte-identity proof is silent), `check` is green (the callee
# is a `pub fn` in a lib crate, so no dead-code warning fires), and `git grep` still printed the line.
# One anchor, one missed regression. Do not put it back: a buried call is buried whether or not more
# prose follows it.
#
# GO-RED, MEASURED (2026-09-06): the pre-fix `39e4b6c7:main.rs` scanned with the OLD pattern -> 0
# hits; with the pattern below -> 1 hit, main.rs:2948, the real defect. The fixed tree -> 0 hits with
# both. Reproduce: ~/unaos-bench/scratch/orin16/keydoors-fix/regex-gored.sh
#
# ONE pattern, held in a variable, used by BOTH the tree scan and the control probe — so the probe
# can never certify a pattern the scan is not actually running.
TRAILING_RE='^[^/]*[^[:space:]/][^/]*//.*#\[cfg\([^]]*\)\][[:space:]]*[A-Za-z_:][A-Za-z0-9_:]*.*(\(.*\).*;|\{)'
# CONTROL PROBE, the discipline the phantom/dead checks above already follow: a pattern that matched
# NOTHING would report "0 trailing-comment cfg" and read as a clean tree, so a zero has to be
# distinguishable from a broken regex. Two fixtures, both required, because this check has two ways
# to rot — it can stop catching the hazard, or start catching prose:
#   HAZARD fixture   must MATCH (the F0 shape: code, comment, buried cfg+call, MORE comment)
#   PROSE  fixture   must NOT match (a pure comment line quoting a cfg expression — the false
#                    positive that made the naive form of this whole gate unusable, see the top note)
_hz='    foo(); // prose #[cfg(feature = "wc")] bar::baz(ev); // and still more prose'
_pr='    // see the #[cfg(feature = "wc")] bar::baz(ev); fold above'
printf '%s\n' "$_hz" | grep -qE "$TRAILING_RE" || { echo "GATE-KNOB: control FAILED — the TRAILING-COMMENT pattern no longer matches its own hazard fixture; it would report 0 on any tree. No verdict." >&2; exit 2; }
printf '%s\n' "$_pr" | grep -vE '^[[:space:]]*//' | grep -qE "$TRAILING_RE" && { echo "GATE-KNOB: control FAILED — the TRAILING-COMMENT pattern now matches a PROSE fixture; it would red on a sentence, which teaches people to disable it. No verdict." >&2; exit 2; }
trailing="$(grep -rn --include='*.rs' -E "$TRAILING_RE" "$SRC" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$trailing" ]; then
  rc=1
  echo "GATE-KNOB FAILED — a #[cfg(...)] and the statement it guards sit AFTER a trailing // comment on a code line (they are PROSE: the call does not happen, and neither the line count, nor \`check\`, nor \`git grep\` can tell you so):" >&2
  printf '%s\n' "$trailing" | sed -E 's@^@  TRAILING: @' | cut -c1-160 >&2
  echo "  FIX: move the attribute AND its statement ahead of every comment on the line. The rule for folds in this tree is CODE FIRST, ALL OF IT, THEN THE COMMENTS." >&2
fi
[ "$rc" -eq 0 ] && echo "GATE-KNOB: OK — $(printf '%s\n' "$declared" | grep -c .) features declared, $(printf '%s\n' "$used" | grep -c .) named by a cfg, 0 phantom, 0 dead, 0 trailing-comment cfg"
exit $rc
