#!/usr/bin/env bash
# verb-roots.sh — GATE-VERBS: the shell's two verb tables are ONE set, checked in both directions.
#
# WHY THIS EXISTS (rmbp-ledger B47; built 2026-09-24, cloud session, after LOGIN14 crossed the seam by
# hand adding `passwd`). A shell verb lives in TWO files: `libs/sys/midden_core/src/lib.rs` `HOST_VERBS`
# (membership — `midden_core::plan` asks it first, and a word not in it is "Unknown command" before any
# arm is reached) and `crates/kernel/src/shell.rs` `dispatch_command`'s `match` (behaviour). Nothing
# asserted the two agree: `umv` and `urmattr` had full arms and no table entry for their whole life
# (B47), and a table entry with no arm is the mirror defect — a verb the table admits and the kernel
# answers with the `_ =>` fallthrough. `shell.rs`'s `tste` leg checks a HAND-PICKED subset at runtime;
# this gate checks the WHOLE set at check time, the way GATE-KNOB does over `[features]`.
#
# HOW IT DECIDES. Two sets, extracted structurally:
#   TABLE   every `("<word>", Avail::…)` tuple in HOST_VERBS (cfg-gated entries included: a spelling is
#           a spelling whatever knob arms it).
#   ARMS    every `"<word>" =>` and `"a" | "b" =>` pattern inside `dispatch_command`'s body, full-line
#           and trailing `//` comments stripped first, arms anywhere on a line (the file folds arms onto
#           shared lines for `panic::Location` neutrality — a line-anchored extractor missed `uptime`,
#           `dns` and `fdisk` on the first cut and read a cfg attribute's `installdemo` as an arm).
# Both differences must be EMPTY -> exit 0. Either non-empty -> exit 1, by name and direction. The fix
# is the missing entry or the missing arm; a `_ =>` fallthrough is not an arm.
#
# CONTROL PROBES (the GATE-ROOTS idiom): the extractor is run over SYNTHETIC copies — an arm added to
# dispatch_command must surface as ARMS-ONLY, a tuple added to HOST_VERBS must surface as TABLE-ONLY —
# and over one fact of this tree (`date` is in both). Any control not firing -> exit 2, NO VERDICT.
set -u
WS="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
SHELL_RS="$WS/crates/kernel/src/shell.rs"
MIDDEN_RS="$WS/libs/sys/midden_core/src/lib.rs"
for f in "$SHELL_RS" "$MIDDEN_RS"; do
    [ -r "$f" ] || { echo "GATE-VERBS: NO VERDICT — $f is not readable"; exit 2; }
done

table_of() { # $1 = a midden lib.rs
    grep -o -E '\("[a-z_0-9-]+", *Avail::' "$1" | grep -o -E '"[a-z_0-9-]+"' | tr -d '"' | sort -u
}
arms_of() { # $1 = a shell.rs; the body of dispatch_command, comments stripped
    local s e
    s=$(grep -n -E '^pub fn dispatch_command\(' "$1" | head -1 | cut -d: -f1)
    [ -n "$s" ] || return 3
    e=$(awk -v s="$s" 'NR>s && /^(pub |pub\(crate\) )?fn [a-z_]/ {print NR; exit}' "$1")
    [ -n "$e" ] || e=$(wc -l < "$1")
    awk -v s="$s" -v e="$e" 'NR>s && NR<e' "$1" | sed -E 's#//.*$##' \
        | grep -o -E '"[a-z_0-9-]+"([[:space:]]*\|[[:space:]]*"[a-z_0-9-]+")*[[:space:]]*=>' \
        | grep -o -E '"[a-z_0-9-]+"' | tr -d '"' | sort -u
}

# ---- controls first: a gate whose extractor cannot fire has no verdict --------------------------
tmp=$(mktemp -d "${TMPDIR:-${HOME}/unaos-bench/scratch}/verb-roots.XXXXXX" 2>/dev/null || mktemp -d)
trap 'rm -rf "$tmp"' EXIT
s=$(grep -n -E '^pub fn dispatch_command\(' "$SHELL_RS" | head -1 | cut -d: -f1)
[ -n "$s" ] || { echo "GATE-VERBS: NO VERDICT — dispatch_command not found in $SHELL_RS"; exit 2; }
awk -v s="$s" 'NR==s+1 {print "        \"zz-control-arm\" => {} // synthetic"} {print}' "$SHELL_RS" > "$tmp/shell.rs"
sed -E '0,/^pub const HOST_VERBS/s//    ("zz-control-word", Avail::Always),\n&/' "$MIDDEN_RS" > "$tmp/lib.rs"
# the sed above inserts the synthetic tuple BEFORE the declaration line; the extractor is not scoped to
# the array, so that is the point: a tuple anywhere in the file counts, and this one must be seen.
c_arms=$(arms_of "$tmp/shell.rs" | grep -c -x 'zz-control-arm')
c_table=$(table_of "$tmp/lib.rs" | grep -c -x 'zz-control-word')
c_fact=$(comm -12 <(table_of "$MIDDEN_RS") <(arms_of "$SHELL_RS") | grep -c -x 'date')
if [ "$c_arms" != 1 ] || [ "$c_table" != 1 ] || [ "$c_fact" != 1 ]; then
    echo "GATE-VERBS: NO VERDICT — control probes: synthetic arm seen=$c_arms (want 1), synthetic table word seen=$c_table (want 1), 'date' in both=$c_fact (want 1)"
    exit 2
fi

# ---- the verdict ----------------------------------------------------------------------------------
table_of "$MIDDEN_RS" > "$tmp/table.txt"
arms_of "$SHELL_RS" > "$tmp/arms.txt"
only_table=$(comm -23 "$tmp/table.txt" "$tmp/arms.txt" | tr '\n' ' ')
only_arms=$(comm -13 "$tmp/table.txt" "$tmp/arms.txt" | tr '\n' ' ')
nt=$(wc -l < "$tmp/table.txt"); na=$(wc -l < "$tmp/arms.txt")
echo "GATE-VERBS: HOST_VERBS=$nt dispatch arms=$na controls=3/3"
rc=0
if [ -n "$only_table" ]; then
    echo "GATE-VERBS: RED — in HOST_VERBS with NO dispatch arm (the table admits a word the kernel answers with the fallthrough): $only_table"
    rc=1
fi
if [ -n "$only_arms" ]; then
    echo "GATE-VERBS: RED — dispatch arm with NO HOST_VERBS entry (B47's shape: an arm that can never be reached): $only_arms"
    rc=1
fi
[ $rc -eq 0 ] && echo "GATE-VERBS: GREEN — both differences empty"
exit $rc
