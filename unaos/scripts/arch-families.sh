#!/usr/bin/env bash
# arch-families.sh — GATE-FAMILY: a ratchet on platform-split symbol families.
#
# WHY THIS EXISTS (2026-08-31). UnaOS grows per-platform twins of shared jobs: `render_service` /
# `x86_render_service`, `input_service` / `x86_input_service`, `usb_pump` / `x86_usb_pump`. Each one
# was individually defensible when written. The result is that one job has N implementations and the
# shared 40-60% is maintained N ways.
#
# The lane rule is NOT the cause — crossing a lane already works, by grant. What is missing is a
# PRICE ON NOT SHARING: crossing a lane costs a negotiation, a recorded grant and a review, while
# duplicating costs nothing and appears in no measurement. So the cheapest correct move is the
# duplicating one. This gate is the bill.
#
# It fires at the moment a NAME is chosen — before the body is written — which is the only point
# where the fix is still a rename rather than an extraction.
#
# NOT a style check: growing a family is allowed. It just cannot be SILENT. Update the ledger with a
# one-line reason in the same commit and the gate goes green.
#
# usage: arch-families.sh [--update]      (--update rewrites the ledger from the tree)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$HERE/../crates/kernel/src"
LEDGER="$HERE/../arch-families.ledger"

# AFFIXES — deliberately CONSERVATIVE, and `arm` is deliberately ABSENT from both lists.
# `arm` collides with the English verb: `orin_ladder_arm` means "arm the ladder", not "the ARM
# ladder", and an earlier draft of this gate paired it with `ladder` and produced a false family.
# A gate with false positives teaches people to scroll past the region a real one appears in, so the
# affix set stays narrow and grows only on evidence.
PREFIXES="x86_ orin_ pi_ tegra_ aarch64_"
SUFFIXES="_x86 _orin _pi _tegra _aarch64"

scan() {
  # every fn definition in the kernel, name only
  grep -rhoE '^[[:space:]]*(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?(unsafe[[:space:]]+)?(extern[[:space:]]+"[^"]*"[[:space:]]+)?fn[[:space:]]+[A-Za-z_][A-Za-z0-9_]*' \
    --include='*.rs' "$SRC" 2>/dev/null | sed -E 's/.*fn[[:space:]]+//' | sort -u
}

# ONE affix is stripped, never two: stripping both would fold `orin_ladder_arm` onto `ladder`.
base_of() {
  local n="$1" p s
  for p in $PREFIXES; do case "$n" in "$p"*) printf '%s' "${n#"$p"}"; return;; esac; done
  for s in $SUFFIXES; do case "$n" in *"$s") printf '%s' "${n%"$s"}"; return;; esac; done
  printf '%s' "$n"
}

measure() {
  local names n b
  names="$(scan)"
  # map every platform-marked symbol to its base; a plain sibling joins the family if it exists
  {
    while IFS= read -r n; do
      b="$(base_of "$n")"
      [ "$b" = "$n" ] && continue
      printf '%s\t%s\n' "$b" "$n"
      grep -qxF "$b" <<<"$names" && printf '%s\t%s\n' "$b" "$b"
    done <<<"$names"
  } | sort -u | awk -F'\t' '{fam[$1]=fam[$1]" "$2; n[$1]++} END{for(f in fam) if(n[f]>=2) printf "%s\t%d\t%s\n", f, n[f], substr(fam[f],2)}' | sort
}

NOW="$(measure)"

if [ "${1:-}" = "--update" ]; then
  printf '%s\n' "$NOW" > "$LEDGER"
  echo "GATE-FAMILY: ledger rewritten — $(wc -l < "$LEDGER") families"
  exit 0
fi

if [ ! -f "$LEDGER" ]; then
  echo "GATE-FAMILY: no ledger at $LEDGER — run with --update to seed it" >&2
  exit 1
fi

if diff -u "$LEDGER" <(printf '%s\n' "$NOW") > /tmp/.gatefam.$$ 2>&1; then
  echo "GATE-FAMILY: OK — $(printf '%s\n' "$NOW" | grep -c . ) platform-split families, none grown"
  rm -f /tmp/.gatefam.$$
  exit 0
fi

echo "=============================================================================" >&2
echo "GATE-FAMILY FAILED — a platform-split symbol family changed." >&2
echo >&2
sed -n '3,$p' /tmp/.gatefam.$$ >&2
rm -f /tmp/.gatefam.$$
cat >&2 <<'MSG'

A "+" line is a family that GREW or APPEARED: one more per-platform copy of a job that
already had one. That is allowed, and it is not free.

Before updating the ledger, answer in the commit message:
  * what is the SHARED part of these N implementations, and why is it not extracted?
  * which axis genuinely differs (waiting? instance identity? focus?) — name it.
  * would a caller of the existing member have worked, with a parameter?

Then: unaos/scripts/arch-families.sh --update, and commit the ledger WITH that reason.
MSG
exit 1
