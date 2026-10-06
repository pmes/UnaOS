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
arms_of() { # $1 = a shell.rs; the DEPTH-1 arms of dispatch_command's `match command {` (VERBDEPTH)
    # GATEREVIEW V1 / arc VERBDEPTH (B476): a `"w" =>` in an INNER match (a sub-verb, a flag, a path
    # case) is not a verb arm, so only patterns at brace depth 1 of the dispatch match count. A char
    # scanner skips strings, raw strings, char literals and (nested) comments, keeps the depth-1 text
    # and blanks everything deeper; the arm regex reads only what is left. No dispatch match: exit 3.
    python3 - "$1" <<'PY'
import re, sys
t = open(sys.argv[1], encoding="utf-8", errors="replace").read()
f = re.search(r"^pub fn dispatch_command\(", t, re.M)
m = f and re.compile(r"^[ \t]*match command \{", re.M).search(t, f.end())
if not m:
    sys.exit(3)
i, n, d, out = m.end(), len(t), 1, []
while i < n and d > 0:
    c = t[i]
    if t.startswith("//", i):
        j = t.find("\n", i); i = n if j < 0 else j; continue
    if t.startswith("/*", i):
        k, i = 1, i + 2
        while i < n and k:
            if t.startswith("/*", i): k += 1; i += 2
            elif t.startswith("*/", i): k -= 1; i += 2
            else: i += 1
        out.append(" "); continue
    r = re.compile(r'b?r(#*)"').match(t, i)
    if r and (i == 0 or not (t[i-1].isalnum() or t[i-1] == "_")):
        j = t.find('"' + r.group(1), r.end()); j = n if j < 0 else j + 1 + len(r.group(1))
        out.append(t[i:j] if d == 1 else " "); i = j; continue
    if c == '"':
        j = i + 1
        while j < n and t[j] != '"':
            j += 2 if t[j] == "\\" else 1
        out.append(t[i:j+1] if d == 1 else " "); i = j + 1; continue
    if c == "'":
        q = re.compile(r"'(?:\\[^']{1,10}|[^'\\])'").match(t, i)
        if q: out.append(" "); i = q.end(); continue
    if c == "{": d += 1; out.append(" "); i += 1; continue
    if c == "}": d -= 1; out.append(" "); i += 1; continue
    out.append(c if d == 1 else (" " if c != "\n" else "\n")); i += 1
body = "".join(out)
words = set()
for a in re.findall(r'"[a-z_0-9-]+"(?:\s*\|\s*"[a-z_0-9-]+")*\s*=>', body):
    words.update(re.findall(r'"([a-z_0-9-]+)"', a))
print("\n".join(sorted(words)))
PY
}

# ---- controls first: a gate whose extractor cannot fire has no verdict --------------------------
tmp=$(mktemp -d "${TMPDIR:-${HOME}/unaos-bench/scratch}/verb-roots.XXXXXX" 2>/dev/null || mktemp -d)
trap 'rm -rf "$tmp"' EXIT
s=$(grep -n -E '^pub fn dispatch_command\(' "$SHELL_RS" | head -1 | cut -d: -f1)
[ -n "$s" ] || { echo "GATE-VERBS: NO VERDICT — dispatch_command not found in $SHELL_RS"; exit 2; }
m=$(awk -v s="$s" 'NR>s && /^[ \t]*match command \{/ {print NR; exit}' "$SHELL_RS")
[ -n "$m" ] || { echo "GATE-VERBS: NO VERDICT — dispatch_command has no \`match command {\`"; exit 2; }
awk -v s="$m" 'NR==s+1 {print "        \"zz-control-arm\" => {} // synthetic"; print "        /* \"zz-comment-arm\" => {} */ // GATEREVIEW F8"; print "        \"zz-outer-arm\" => match x { \"zz-inner-arm\" => {} _ => {} }, // VERBDEPTH: an inner arm is no verb arm"} {print}' "$SHELL_RS" > "$tmp/shell.rs"
sed -E '0,/^pub const HOST_VERBS/s//    ("zz-control-word", Avail::Always),\n&/' "$MIDDEN_RS" > "$tmp/lib.rs"
# the sed above inserts the synthetic tuple BEFORE the declaration line; the extractor is not scoped to
# the array, so that is the point: a tuple anywhere in the file counts, and this one must be seen.
c_arms=$(arms_of "$tmp/shell.rs" | grep -c -x 'zz-control-arm')
[ "$(arms_of "$tmp/shell.rs" | grep -c -x 'zz-comment-arm')" = 0 ] || c_arms=commented   # GATEREVIEW F8: an arm inside /* */ is no arm
[ "$(arms_of "$tmp/shell.rs" | grep -c -x -e 'zz-inner-arm' -e 'zz-outer-arm')" = 1 ] || c_arms=depth   # VERBDEPTH: the outer arm is seen, its inner arm is not
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
