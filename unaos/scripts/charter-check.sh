#!/usr/bin/env bash
# charter-check.sh — GATE-CHARTER: a kernel file in the app-domain scope names the CODEX handler that
# owns its domain, and a kernel-written dotfile under <home> is on the allowlist.
#
# WHY THIS EXISTS (AUDIT 2026-10-01, docs/dev/evidence/rmbp-0929/AUDIT-HANDLERS-UNAFS.md; Peter:
# "is there a pre-code audit to make sure these side steps go away" — there was not). Three waves of
# desktop features were built inside the kernel beside live Ring-3 handlers that own the domain
# (Settings beside Principia, Trash beside Matrix, an editor beside Tabula, a player beside Stria),
# each with its own persistence. LAWS line 33 says charters come from docs/CODEX.md's manifest; no
# brief step, fold step or check ever read it. This gate makes the question mechanical: every file
# in scope DECLARES its owner and the seam it sits on, so "who owns this" is answered before the
# code exists, and a preference that is not Principia's shows up as a dotfile literal with no row.
#
# HOW IT DECIDES. Scope (CHARTERSCOPE, B476) = EVERY crates/kernel/src/**/*.rs; the first cut read only {video,fs,install,selfhost}/ plus help.rs,
# termcolor.rs, shellux.rs, clipboard.rs, shell.rs, drivers/hda_play.rs. A file passes if it has a
# header line `CHARTER: <Handler> — <seam>` (any of the first 40 lines; handler ∈ the CODEX §2
# manifest ∪ {Kernel, pulse}; seam ∈ wm driver fs-core shared-core fulfiller kernel-by-ruling owed)
# OR a row in scripts/charter.registry (the grandfather table — files that predate the gate; a
# NEW file never gets a row). Dotfiles: every `{}.name"` / `".Name"` literal in the kernel whose
# name begins with a dot must be a `dotfile |` row.
#
# CONTROL PROBES (exit 2, NO verdict, if any fails): the CODEX manifest parses ≥ 20 handlers; a
# synthetic header `CHARTER: Principia — owed B0` is accepted; a synthetic file with no header and
# no row is caught; a synthetic `.nowhere` literal is caught.
#
# usage: charter-check.sh [<unaos dir> [--emit-baseline]]   (exit 0 clean; 1 a file or dotfile is undeclared; 2 control failed)
set -u
if [ "${1:-}" = "--selftest" ]; then   # CHARTERSCOPE (B476): GATEREVIEW plant C1 in a fixture tree, plus the stale rule
  me="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"; T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
  mkdir -p "$T/unaos/scripts" "$T/unaos/crates/kernel/src/video" "$T/unaos/crates/kernel/src/desktop" "$T/docs"
  cp "$me" "$T/unaos/scripts/"; cp "$(dirname "$me")/../../docs/CODEX.md" "$T/docs/"
  printf 'video/old.rs | Kernel | wm | fixture\n' > "$T/unaos/scripts/charter.registry"
  printf '# fixture\nlegacy.rs\n' > "$T/unaos/scripts/charter-scope.baseline"
  printf 'fn a() {}\n' > "$T/unaos/crates/kernel/src/video/old.rs"; printf 'fn l() {}\n' > "$T/unaos/crates/kernel/src/legacy.rs"
  printf '//! CHARTER: Kernel — driver\nfn d() {}\n' > "$T/unaos/crates/kernel/src/net.rs"
  printf 'fn notes() {}\n' > "$T/unaos/crates/kernel/src/desktop/notes.rs"
  o=$(CHARTER_FLOOR=1 bash "$T/unaos/scripts/charter-check.sh" "$T/unaos"); r1=$?
  printf '//! CHARTER: Tabula — owed B1\nfn notes() {}\n' > "$T/unaos/crates/kernel/src/desktop/notes.rs"
  CHARTER_FLOOR=1 bash "$T/unaos/scripts/charter-check.sh" "$T/unaos" >/dev/null; r2=$?
  printf 'gone.rs\n' >> "$T/unaos/scripts/charter-scope.baseline"
  o3=$(CHARTER_FLOOR=1 bash "$T/unaos/scripts/charter-check.sh" "$T/unaos"); r3=$?
  if [ $r1 = 1 ] && printf '%s' "$o" | grep -q 'desktop/notes.rs declares no owner' && ! printf '%s' "$o" | grep -q legacy.rs \
     && [ $r2 = 0 ] && [ $r3 = 1 ] && printf '%s' "$o3" | grep -q 'stale charter-scope.baseline row gone.rs'; then
    echo "  ✅ charter-check selftest: a headerless file in a NEW directory is refused (C1), a baselined one passes, a declared one passes, a stale row is seen"; exit 0
  fi
  echo "charter-check: SELFTEST FAILED — C1 rc=$r1 (want 1), declared rc=$r2 (want 0), stale rc=$r3 (want 1)"; printf '%s\n%s\n' "$o" "$o3"; exit 2
fi
U="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
K="$U/crates/kernel/src"
SD="$(cd "$(dirname "$0")" && pwd)"
REG="$SD/charter.registry"
CODEX="$SD/../../docs/CODEX.md"
[ -f "$REG" ] || { echo "charter-check: no registry at $REG"; exit 2; }
[ -f "$CODEX" ] || { echo "charter-check: no CODEX at $CODEX"; exit 2; }

# handler names from the manifest table: rows `| **Name** | Domain | ...`
mapfile -t HANDLERS < <(sed -n 's/^| \*\*\([^*]*\)\*\* |.*/\1/p' "$CODEX" | sed 's/ë/e/g;s/ï/i/g;s/[[:space:]]*$//')
[ "${#HANDLERS[@]}" -ge 20 ] || { echo "charter-check: CONTROL FAILED — CODEX manifest parsed ${#HANDLERS[@]} handlers (< 20)"; exit 2; }
SEAMS="wm driver fs-core shared-core fulfiller kernel-by-ruling owed"

# CHARTERSCOPE (B476): the scope grew from ~80 files to every kernel file, so the header test is fork-free
# bash (the sed/tr pipeline per file took 10 s at 80 files); the semantics are the first cut's.
norm() { local x="$1"; x=${x//ë/e}; x=${x//ï/i}; x=${x,,}; printf '%s' "${x// /}"; }
declare -A HSET=([kernel]=1 [pulse]=1)
for _h in "${HANDLERS[@]}"; do HSET[$(norm "$_h")]=1; done
handler_ok() { [ -n "${HSET[$(norm "$1")]+x}" ]; }
seam_ok() { local s; for s in $SEAMS; do [ "$s" = "$1" ] && return 0; done; return 1; }
# line_ok <the text after CHARTER:>: a known handler, then — / -- / - / |, then a known seam word
line_ok() { local l h r; l=${1//—/|}; l="${l#"${l%%[![:space:]]*}"}"
  h=${l%%[|-]*}; r=${l:${#h}}; h="${h%"${h##*[![:space:]]}"}"
  r=${r#|}; r=${r#-}; r=${r#-}; [[ $r =~ ^[[:space:]]*([a-z-]*) ]] || return 1
  handler_ok "$h" && seam_ok "${BASH_REMATCH[1]}"; }
# header_ok <text>: the first CHARTER line in its first 40 lines names a known handler and seam
header_ok() { local n=0 ln; while IFS= read -r ln && [ $n -lt 40 ]; do n=$((n+1))
    case "$ln" in *CHARTER:*) line_ok "${ln##*CHARTER:}"; return;; esac; done <<< "$1"; return 1; }
reg_ok() { grep -q "^$1 |" "$REG"; }
dot_ok() { grep -q "^dotfile | $1 |" "$REG"; }

# controls
header_ok $'// x\n//! CHARTER: Principia — owed B0\n' || { echo "charter-check: CONTROL FAILED — synthetic header not accepted"; exit 2; }
header_ok $'// no charter here\nfn x() {}\n' && { echo "charter-check: CONTROL FAILED — headerless text accepted"; exit 2; }
reg_ok "video/__synthetic__.rs" && { echo "charter-check: CONTROL FAILED — synthetic file has a row"; exit 2; }
dot_ok ".nowhere" && { echo "charter-check: CONTROL FAILED — synthetic dotfile allowed"; exit 2; }

# GATEREVIEW F5: the extraction regex is itself probed (the `.nowhere` control above only tested the row lookup), and it
# reads the Capitalised path-join form `{}.Recents` that `{}\.[a-z]` missed.
DOTRE='\{\}\.[a-z][A-Za-z_./]*"|\{\}\.[A-Z][a-z][A-Za-z_]*"|"\.[A-Z][a-z][A-Za-z_]*(/[A-Za-z_.]*)?"'
[ "$(printf '%s\n' 'format!("{}.nowhere", h); format!("{}.Recents", h); x(".Trash/.index"); y("{}.PNG")' | grep -oE "$DOTRE" | wc -l)" -eq 3 ] || { echo "charter-check: CONTROL FAILED — the dotfile regex did not extract its three synthetic literals"; exit 2; }
rc=0
# CHARTERSCOPE (B476, GATEREVIEW C1): the scope was a directory list, so a NEW kernel directory (a headerless
# `desktop/notes.rs`) was unseen. Scope = EVERY crates/kernel/src/**/*.rs. A file passes with a CHARTER header,
# a charter.registry row (the app-domain grandfathers), or a row in charter-scope.baseline (the files outside the
# first cut's directories that predate this widening). The baseline only shrinks: a row whose file is gone or now
# declares an owner is STALE and fails. A NEW file anywhere declares in its header; it never gets a row.
SB="$SD/charter-scope.baseline"
[ -f "$SB" ] || { echo "charter-check: CONTROL FAILED — no scope baseline at $SB"; exit 2; }
declare -A BASE=()
while IFS= read -r ln; do case "$ln" in ''|'#'*) continue;; esac; BASE[${ln%%[[:space:]]*}]=1; done < "$SB"
[ -z "${BASE[desktop/__synthetic__.rs]+x}" ] || { echo "charter-check: CONTROL FAILED — synthetic file has a scope-baseline row"; exit 2; }
declare -A REGROW=()
while IFS= read -r ln; do case "$ln" in *' | '*) REGROW[${ln%% | *}]=1;; esac; done < <(grep -v '^#' "$REG")
declare -A FIRST=()   # file -> the text after its first CHARTER: within 40 lines (one awk over the tree)
mapfile -t FILES < <(cd "$K" && find . -name '*.rs' | sed 's#^\./##' | sort)
while IFS=$'\t' read -r f t; do FIRST[$f]=$t; done < <(cd "$K" && awk 'FNR==1{d=0} !d && FNR<=40 && index($0,"CHARTER:") {sub(/.*CHARTER:/,""); print FILENAME "\t" $0; d=1}' "${FILES[@]}")
n=0; nb=0; emit=0; [ "${2:-}" = "--emit-baseline" ] && emit=1
for f in "${FILES[@]}"; do
  n=$((n+1))
  if [ -n "${FIRST[$f]+x}" ] && line_ok "${FIRST[$f]}"; then
    [ -n "${BASE[$f]+x}" ] && { echo "  ❌ GATE-CHARTER: stale charter-scope.baseline row $f — the file declares its owner now; delete the row (the baseline only shrinks)"; rc=1; }
    continue
  fi
  [ -n "${REGROW[$f]+x}" ] && continue
  if [ $emit = 1 ]; then echo "$f"; continue; fi
  if [ -n "${BASE[$f]+x}" ]; then nb=$((nb+1)); continue; fi
  echo "  ❌ GATE-CHARTER: $K/$f declares no owner — add a header line \`CHARTER: <CODEX handler|Kernel> — <seam>\` (seams: $SEAMS; a NEW file never gets a registry or baseline row)"
  rc=1
done
[ $emit = 1 ] && exit 0
[ "$n" -ge "${CHARTER_FLOOR:-300}" ] || { echo "charter-check: CONTROL FAILED — scope enumerated $n files (< 300: every kernel file)"; exit 2; }
for f in "${!BASE[@]}"; do [ -f "$K/$f" ] || { echo "  ❌ GATE-CHARTER: stale charter-scope.baseline row $f — no such file; delete the row"; rc=1; }; done

# dotfile literals: `{}.name"` (format of <home>/.name) and `".Name"` / `".name/..."` — a NAME has a
# path-join form (`{}.name`) or a Capitalised name (`.Trash`, `.Trash/.index`); a bare lowercase token
# (`.txt`, `.png`) is an extension test and an all-caps one (`.TRASHES`) a macOS leftover FAT skips
while IFS= read -r m; do
  name=$(printf '%s' "$m" | sed 's/^.*://; s/^{}//; s/"$//; s/^"//')
  case "$name" in .*) ;; *) continue;; esac
  dot_ok "$name" || { echo "  ❌ GATE-CHARTER: kernel writes a dotfile not on the allowlist: '$name' ($m) — a preference belongs in Principia's store; if it is not one, add a \`dotfile |\` row with its ledger id"; rc=1; }
done < <(grep -rhoE "$DOTRE" "$K" --include='*.rs' | sort -u)

[ "$rc" -eq 0 ] && echo "  ✅ charter (GATE-CHARTER: $n kernel files, $((n-nb)) declare an owner or are registered, $nb grandfathered by charter-scope.baseline; every kernel dotfile is on the allowlist)"
exit $rc
