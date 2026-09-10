#!/usr/bin/env bash
# check-roots.sh — GATE-ROOTS: every BINARY target in the tree is a named root of `./arroyo check`.
#
# WHY THIS EXISTS (2026-09-02). `check` type-checks the dependency graph reachable from the roots it
# NAMES: two default kernel legs, the KERNEL_CFG_MATRIX board legs, the USER_CHECK_MATRIX rows. The
# LIBRARIES come free — boot-info, net, una-abi, rast, xusb-fw are all compiled because the kernel
# depends on them. A BINARY is nobody's dependency. It is its own root, so a binary that no leg names
# is never type-checked by the gate, on any run, while the tree reads as "check green".
# `crates/bootloader` was that binary: the workspace's only leaf binary named by no check leg, built
# only by the media/launch commands (`esp-*`, `x86`, `arm`), which are not the gate. This script
# closes the CLASS, not the instance: a second unnamed binary reds it the day it appears.
#
# HOW IT DECIDES. It reads `arroyo` — the one that the gate runs — and walks the functions reachable
# from `check_both` (the `check` entry point), collecting every crate directory a `cargo check` or
# `cargo build` is run in: `cd "$WORKSPACE_DIR/<crate>"` followed by the cargo line, the subshell
# form `(cd ... && cargo ...)`, `-p <package>`, and `--manifest-path`. The one variable path in that
# closure, `crates/$_crate` in check_user_arch, is expanded from USER_CHECK_MATRIX exactly as the
# function does. `esp-arm`/`esp-x86`/`x86`/`arm` are outside the closure by construction: a build a
# media command runs is not a check the gate runs. Full-line comments are dropped first, so a
# commented-out leg is an absent leg (that is the go-red proof).
#
# CONTROL PROBE, the idiom GATE-KNOB and the knob->leg check use: a parser that matched NOTHING would
# report every binary unnamed (loud) — but a parser that matched only the direct `cd` form would
# report the matrix crates unnamed, and one that lost `check_both` would report zero roots. Both are
# distinguishable from a real hole only by asserting things that certainly exist: `crates/kernel`
# must be enumerated as a binary AND resolved as a root of check_both itself, and the matrix
# expansion must yield at least one crate. Otherwise exit 2 with NO verdict.
#
# usage: check-roots.sh          (exit 0 all binaries named; 1 a binary is unnamed; 2 control failed)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UNAOS="$(cd "$HERE/.." && pwd)"
ARROYO="$UNAOS/arroyo"
ENTRY="check_both"

[ -f "$ARROYO" ] || { echo "GATE-ROOTS: control FAILED — no arroyo at $ARROYO. No verdict." >&2; exit 2; }

# ── 1. The binaries: every crate under crates/*/ plus builder/ with a src/main.rs or a [[bin]] ─────
# By DIRECTORY, not by workspace membership: a binary crate that is not even a member is a binary
# nobody checks, which is the worse case, not an exemption.
bins=()
for d in "$UNAOS"/crates/*/ "$UNAOS"/builder/; do
  [ -f "$d/Cargo.toml" ] || continue
  rel="${d#"$UNAOS"/}"; rel="${rel%/}"
  if [ -f "$d/src/main.rs" ] || grep -qE '^\[\[bin\]\]' "$d/Cargo.toml"; then
    bins+=("$rel")
  fi
done

# package name -> directory, for `-p NAME` legs
pkg_dir() {
  local want="$1" m
  for m in "$UNAOS"/crates/*/Cargo.toml "$UNAOS"/builder/Cargo.toml; do
    if awk '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^name[ \t]*=/' "$m" | grep -qE "=[ \t]*\"$want\""; then
      m="${m#"$UNAOS"/}"; printf '%s' "${m%/Cargo.toml}"; return 0
    fi
  done
  return 1
}

# ── 2. arroyo, comments dropped; the function table; USER_CHECK_MATRIX crates ──────────────────────
src="$(grep -vE '^[[:space:]]*#' "$ARROYO")"
fn_names="$(printf '%s\n' "$src" | grep -oE '^[A-Za-z_][A-Za-z0-9_]*\(\)[[:space:]]*\{' | sed -E 's/\(\).*//' | sort -u)"
body_of() {  # $1 = function name; prints its body (first `^name() {` to the first `^}`)
  printf '%s\n' "$src" | awk -v n="$1" '
    $0 ~ "^"n"\\(\\)[ \t]*\\{" {f=1; next}
    f && /^\}/ {exit}
    f {print}'
}
matrix_crates="$(printf '%s\n' "$src" | awk '/^USER_CHECK_MATRIX=\(/{f=1;next} f && /^\)/{exit} f' \
                 | grep -oE '^[[:space:]]*"[A-Za-z0-9_-]+' | tr -d ' "' | sort -u)"

# ── 3. Walk the closure of functions reachable from check_both; collect (root, function) pairs ─────
declare -A seen=()
roots=""      # lines of "dir<TAB>function"
unresolved="" # cd paths with a $VAR this script does not know how to expand — reported, never counted
queue=("$ENTRY")
mark() { roots="${roots}$1	$2"$'\n'; }
while [ "${#queue[@]}" -gt 0 ]; do
  fn="${queue[0]}"; queue=("${queue[@]:1}")
  [ -n "${seen[$fn]:-}" ] && continue
  seen[$fn]=1
  body="$(body_of "$fn")"
  [ -n "$body" ] || continue
  cur=""
  while IFS= read -r line; do
    # subshell form on one line: (cd "$WORKSPACE_DIR/X" && cargo ... check|build ...)
    if [[ $line =~ \(cd\ \"\$\{?WORKSPACE_DIR\}?/([^\"]+)\"\ \&\&\ cargo ]]; then
      p="${BASH_REMATCH[1]}"
      if [[ $line =~ cargo(\ \+nightly)?\ (check|build)([[:space:]]|$) ]]; then
        case "$p" in ..*) ;; *) mark "$p" "$fn";; esac
      fi
      continue
    fi
    if [[ $line =~ (^|[[:space:];&|])cd\ \"\$\{?WORKSPACE_DIR\}?/([^\"]+)\" ]]; then
      cur="${BASH_REMATCH[2]}"
    elif [[ $line =~ (^|[[:space:];&|])cd\ \"\$\{?WORKSPACE_DIR\}?\" ]]; then
      cur=""
    fi
    if [[ $line =~ cargo(\ \+nightly)?\ (check|build)([[:space:]]|$) ]]; then
      if [[ $line =~ \ -p\ ([A-Za-z0-9_-]+) ]]; then
        d="$(pkg_dir "${BASH_REMATCH[1]}")" && mark "$d" "$fn"
      elif [[ $line =~ --manifest-path[=\ ]\"?\$\{?WORKSPACE_DIR\}?/([^\"[:space:]]+)/Cargo.toml ]]; then
        mark "${BASH_REMATCH[1]}" "$fn"
      elif [ -n "$cur" ]; then
        case "$cur" in
          'crates/$_crate') for c in $matrix_crates; do mark "crates/$c" "$fn"; done;;
          *'$'*) unresolved="${unresolved}  ${fn}: cd \"\$WORKSPACE_DIR/${cur}\""$'\n';;
          ..*) ;;
          *) mark "$cur" "$fn";;
        esac
      fi
    fi
  done <<<"$body"
  # every defined function this body names joins the walk (bounded by `seen`)
  for g in $fn_names; do
    [ -n "${seen[$g]:-}" ] && continue
    printf '%s\n' "$body" | grep -qE "(^|[^A-Za-z0-9_.])$g([^A-Za-z0-9_]|$)" && queue+=("$g")
  done
done
roots="$(printf '%s' "$roots" | sort -u)"

# ── 4. Control probes — no verdict unless the instrument demonstrably works ────────────────────────
printf '%s\n' "${bins[@]}" | grep -qx 'crates/kernel' \
  || { echo "GATE-ROOTS: control FAILED — crates/kernel not enumerated as a binary; the enumerator is broken. No verdict." >&2; exit 2; }
printf '%s\n' "$roots" | grep -qxF "crates/kernel	$ENTRY" \
  || { echo "GATE-ROOTS: control FAILED — crates/kernel not resolved as a root of $ENTRY; the arroyo parser is broken. No verdict." >&2; exit 2; }
[ -n "$matrix_crates" ] \
  || { echo "GATE-ROOTS: control FAILED — USER_CHECK_MATRIX parsed to zero crates; the matrix parser is broken. No verdict." >&2; exit 2; }

# ── 5. Verdict ─────────────────────────────────────────────────────────────────────────────────────
rc=0
missing=""
echo "GATE-ROOTS: binary targets and the check leg(s) naming each —"
for b in "${bins[@]}"; do
  by="$(printf '%s\n' "$roots" | awk -F'\t' -v b="$b" '$1==b{print $2}' | paste -sd, -)"
  if [ -n "$by" ]; then
    printf '  %-22s %s\n' "$b" "$by"
  else
    printf '  %-22s %s\n' "$b" "NAMED BY NO LEG"
    missing="${missing} ${b}"
    rc=1
  fi
done
if [ -n "$unresolved" ]; then
  echo "GATE-ROOTS: note — cd paths with a variable this script cannot expand (not counted as roots):" >&2
  printf '%s' "$unresolved" >&2
fi
if [ "$rc" -ne 0 ]; then
  cat >&2 <<MSG
GATE-ROOTS FAILED — binary target(s) that no leg of \`./arroyo check\` names:${missing}

A binary is its own root: nothing depends on it, so unless a check leg names it, it is never
type-checked and "check green" says nothing about it.
  FIX: add a \`cargo +nightly check\` leg for it inside check_both (or a function check_both calls,
       or a USER_CHECK_MATRIX row), for every target it ships on. If it is not a binary, drop its
       src/main.rs / [[bin]]. A media or launch command building it does not count — those are not the gate.
MSG
fi
[ "$rc" -eq 0 ] && echo "GATE-ROOTS: OK — ${#bins[@]} binary targets, every one named by a leg of $ENTRY"
exit $rc
