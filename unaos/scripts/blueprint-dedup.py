#!/usr/bin/env python3
"""blueprint-dedup.py — drop fold-compose debris (a clause pasted more than once) from a blueprint §5 row.

CHARTER: tooling for the docs/dev/blueprint sheets (no CODEX §2 domain); blueprint-check.py stays THE gate, this only removes repeats.
WHY (BLUEPRINTROWDEDUP, LEDGER SR1006): sheet merges at the fold (rowappend / cellappend / compose) paste a branch's copy
of a clause beside HEAD's, so a row can carry the same `KEY BUILT by SRnnn (...)` clause two to five times, as exact
copies or as older and newer versions of it. blueprint-check counts anchors, not repeats, so the debris stays green.
WHAT: a COPY GROUP is two or more `BUILT|MEASURED-NOT-NEEDED by|as [LEDGER ]SRnnn` anchors with the same text, the same
enclosing paren and the same next 40 chars. For each consecutive pair (a, b) of a group: (a) drop the earlier copy, the
text a..b; else (b) drop the later copy: b's own paren group when one follows the anchor, else b up to the group's next
anchor or the enclosing paren's close, with the separator before it. A drop is taken only when THE PROOF holds on the
row; a group with no provable drop is REFUSED and left as it is. A literal `BUILT by SRn BUILT by SRn` stutter collapses.
THE PROOF (asserts, per row, before == after): the distinct capitalised KEY tokens; every SRnnn id; the ids written
`BUILT|MEASURED-NOT-NEEDED by|as SRnnn`; the keys written `KEY BUILT|MEASURED-NOT-NEEDED by|as`; the keys `owed` and
not built; the paren balance (net, unmatched `)`, unclosed `(`); the sheet's line count; every other line byte-identical.
usage: blueprint-dedup.py <sheet> "<row prefix>" [--dry-run]   rewrite (or only print) that row
       blueprint-dedup.py <sheet> --dry-run --all              print the drops every `- ` row would take
exit 0 (no refusal), 1 (a group refused), 2 (usage / the row prefix matches no line or more than one).
"""
import re, sys

ANCH = re.compile(r"\b(?:BUILT|MEASURED-NOT-NEEDED) (?:by|as) (?:LEDGER )?SR\d+\b")
STUTTER = re.compile(r"(\b(?:BUILT|MEASURED-NOT-NEEDED) (?:by|as) (?:LEDGER )?SR\d+) \1\b")
KEY = re.compile(r"\b[A-Z][A-Z0-9]*(?:-[A-Z0-9]+)*[a-z]?\b")
SRID = re.compile(r"\bSR\d+\b")
BUILTID = re.compile(r"\b(?:BUILT|MEASURED-NOT-NEEDED) (?:by|as) (?:LEDGER )?(?:[A-Z0-9]+ )?(SR\d+)\b")
BUILTKEY = re.compile(r"\b([A-Z][A-Z0-9]*(?:-[A-Z0-9]+)+[a-z]?) (?:BUILT|MEASURED-NOT-NEEDED) (?:by|as)\b")
OWEDKEY = re.compile(r"\bowed (?:from it:? )?([A-Z][A-Z0-9]*(?:-[A-Z0-9]+)+[a-z]?)\b")


def balance(r):
    """(net depth, unmatched `)` count, unclosed `(` count) of a row."""
    d = stray = 0
    for c in r:
        if c == "(":
            d += 1
        elif c == ")":
            if d == 0:
                stray += 1
            else:
                d -= 1
    return (r.count("(") - r.count(")"), stray, d)


def proof(r):
    keys = {k for k in KEY.findall(r) if len(k) > 1}
    built = set(BUILTKEY.findall(r))
    return {"keys": keys, "srids": set(SRID.findall(r)), "builtids": set(BUILTID.findall(r)),
            "builtkeys": built, "owedopen": set(OWEDKEY.findall(r)) - built, "balance": balance(r)}


def pairs(r):
    st, pr = [], {}
    for i, c in enumerate(r):
        if c == "(":
            st.append(i)
        elif c == ")" and st:
            pr[st.pop()] = i
    return pr


def enclosing(pr, p, n):
    best = (-1, n)
    for o, e in pr.items():
        if o < p < e and o > best[0]:
            best = (o, e)
    return best


def groups(r):
    pr, g = pairs(r), {}
    for m in ANCH.finditer(r):
        g.setdefault((enclosing(pr, m.start(), len(r)), m.group(0), r[m.end():m.end() + 40]), []).append(m.start())
    return pr, sorted((ps, k) for k, ps in g.items() if len(ps) > 1)


def step(r, base, refused):
    """One provable drop: (new row, log line) or None."""
    m = STUTTER.search(r)
    if m:
        new = r[:m.start()] + m.group(1) + r[m.end():]
        if proof(new) == base:
            return new, "stutter at %d: dropped %d chars %r" % (m.start(), m.end() - m.start() - len(m.group(1)), m.group(1))
    pr, gs = groups(r)
    for ps, (encl, an, _) in gs:
        if (an, r[ps[0]:ps[0] + 200]) in refused:
            continue
        for i in range(len(ps) - 1):
            a, b = ps[i], ps[i + 1]
            new = r[:a] + r[b:]
            if proof(new) == base:
                return new, "%s: copy %d of %d at %d dropped (earlier copy, %d chars)" % (an, i + 1, len(ps), a, b - a)
            g = b + len(an)
            if r[g:g + 2] == " (" and g + 1 in pr:
                e = pr[g + 1] + 1
            else:
                e = ps[i + 2] if i + 2 < len(ps) else encl[1]
            s = b
            while s > 0 and r[s - 1] in " :;,":
                s -= 1
            new = r[:s] + r[e:]
            if proof(new) == base:
                return new, "%s: copy %d of %d at %d dropped (later copy, %d chars)" % (an, i + 2, len(ps), b, e - s)
        refused.add((an, r[ps[0]:ps[0] + 200]))
        return r, "%s: REFUSED, %d copies at %s (no drop keeps the proof)" % (an, len(ps), ps)
    return None


def dedup(r):
    base, refused, log = proof(r), set(), []
    while True:
        s = step(r, base, refused)
        if s is None:
            break
        r, line = s
        log.append(line)
    assert proof(r) == base
    return r, log, len(refused)


def main(argv):
    args = [a for a in argv if not a.startswith("--")]
    dry, every = "--dry-run" in argv, "--all" in argv
    if not args or (every and not dry) or (not every and len(args) != 2):
        print(__doc__.strip().split("\n")[-3]); return 2
    path = args[0]
    lines = open(path, encoding="utf-8").read().split("\n")
    targets = [i for i, l in enumerate(lines) if l.startswith("- ")] if every else [i for i, l in enumerate(lines) if l.startswith(args[1])]
    if not every and len(targets) != 1:
        print("blueprint-dedup: row prefix %r matches %d lines" % (args[1], len(targets))); return 2
    out, refusals, dropped = list(lines), 0, 0
    for i in targets:
        new, log, nref = dedup(lines[i])
        refusals += nref
        if log:
            print("L%d %d -> %d chars" % (i + 1, len(lines[i]), len(new)))
            for l in log:
                print("  " + l)
        if new != lines[i]:
            dropped += 1
            out[i] = new
    assert len(out) == len(lines) and all(out[j] == lines[j] for j in range(len(lines)) if j not in targets)
    print("BLUEPRINT-DEDUP: rows=%d changed=%d refused-groups=%d lines=%d %s" % (len(targets), dropped, refusals, len(out), "(dry run)" if dry else "-> written"))
    if not dry and dropped:
        open(path, "w", encoding="utf-8").write("\n".join(out))
    return 1 if refusals else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
