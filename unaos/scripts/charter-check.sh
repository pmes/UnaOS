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
# HOW IT DECIDES. Scope = crates/kernel/src/{video,fs,install,selfhost}/**/*.rs plus help.rs,
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
# usage: charter-check.sh [<unaos dir>]   (exit 0 clean; 1 a file or dotfile is undeclared; 2 control failed)
set -u
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

norm() { printf '%s' "$1" | sed 's/ë/e/g;s/ï/i/g' | tr 'A-Z' 'a-z' | tr -d ' '; }
handler_ok() { local h; h=$(norm "$1"); [ "$h" = kernel ] || [ "$h" = pulse ] && return 0
  local x; for x in "${HANDLERS[@]}"; do [ "$(norm "$x")" = "$h" ] && return 0; done; return 1; }
seam_ok() { local s; for s in $SEAMS; do [ "$s" = "$1" ] && return 0; done; return 1; }
# header_ok <text>: a CHARTER line with a known handler and seam
header_ok() { local line h s
  line=$(printf '%s\n' "$1" | head -40 | sed -n 's/.*CHARTER:[[:space:]]*\(.*\)/\1/p' | head -1)
  [ -n "$line" ] || return 1
  h=$(printf '%s' "$line" | sed 's/[[:space:]]*\(—\|--\|-\||\).*//')
  s=$(printf '%s' "$line" | sed -n 's/^[^—|-]*\(—\|--\|-\|-\|-\|-\|-\|-\|-\|-\|-\|-\|-\|-\|-\|-\|-\|-\|-\||\)[[:space:]]*\([a-z-]*\).*/\2/p')
  handler_ok "$h" && seam_ok "$s"; }
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
scope() { ( cd "$K" && find video fs install selfhost -name '*.rs' 2>/dev/null; for f in help.rs termcolor.rs shellux.rs clipboard.rs shell.rs drivers/hda_play.rs; do [ -f "$f" ] && echo "$f"; done ) | sort -u; }
n=0
while IFS= read -r f; do
  n=$((n+1))
  if ! header_ok "$(head -40 "$K/$f")" && ! reg_ok "$f"; then
    echo "  ❌ GATE-CHARTER: $K/$f declares no owner — add a header line \`CHARTER: <CODEX handler|Kernel> — <seam>\` (seams: $SEAMS; a NEW file never gets a registry row)"
    rc=1
  fi
done < <(scope)
[ "$n" -ge 10 ] || { echo "charter-check: CONTROL FAILED — scope enumerated $n files"; exit 2; }

# dotfile literals: `{}.name"` (format of <home>/.name) and `".Name"` / `".name/..."` — a NAME has a
# path-join form (`{}.name`) or a Capitalised name (`.Trash`, `.Trash/.index`); a bare lowercase token
# (`.txt`, `.png`) is an extension test and an all-caps one (`.TRASHES`) a macOS leftover FAT skips
while IFS= read -r m; do
  name=$(printf '%s' "$m" | sed 's/^.*://; s/^{}//; s/"$//; s/^"//')
  case "$name" in .*) ;; *) continue;; esac
  dot_ok "$name" || { echo "  ❌ GATE-CHARTER: kernel writes a dotfile not on the allowlist: '$name' ($m) — a preference belongs in Principia's store; if it is not one, add a \`dotfile |\` row with its ledger id"; rc=1; }
done < <(grep -rhoE "$DOTRE" "$K" --include='*.rs' | sort -u)

[ "$rc" -eq 0 ] && echo "  ✅ charter (GATE-CHARTER: $n app-domain files declare an owner; every kernel dotfile is on the allowlist)"
exit $rc
