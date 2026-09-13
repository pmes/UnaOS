#!/usr/bin/env bash
# scorer-token-uniqueness.sh — WIREHYG (orin, 2026-09-13), LEDGER A72.
#
# THE RULE IT ENFORCES (LAWS §5): a scorer must key on a token UNIQUE to the verdict it is
# scoring, never on a fragment another emitter can produce.
#
# WHY IT EXISTS. `-> NO REPLY ::` was ARP's failure line. SO47 re-worded ARP's failure to
# `-> NO REPLY (cache miss, wire probe N ms, learned=N) ::` and the fragment did not disappear —
# it CHANGED VERBS. `ping` still emits those exact bytes, because its summary formats
# `" -> {} ::"` against `if received > 0 {"REPLY"} else {"NO REPLY"}` (net_phy.rs:1359). Nothing
# about that looks wrong from any ordinary angle: the count on the wire is unchanged, the string
# is still reachable, and only the EMITTER moved. A scorer keyed on it now goes green on exactly
# the failure it was written to catch, and an ARTIFACT scorer keyed on it reads 0 and reds a
# healthy build, because ping composes the bytes at runtime and never puts them in `.rodata`.
#
# WHAT IT MEASURES. For each token, every kernel print site that can PRODUCE it — including by
# COMPOSITION, where the token spans a `{}` hole filled from one of that site's own string
# literals. That composition case is the whole point: a `grep` of the sources finds ARP and
# misses ping, which is how this class hides.
#
# OUTPUT: one row per token — `count | verbs | file:line …`. A token is
#   DEAD      0 emitters              — the row cannot fire; it is scoring nothing.
#   SHARED    emitters name >1 VERB   — not unique to a verdict; another emitter reaches it.
#   PHRASE    only `Display`-spliced fragments — no verb of its own; fine as an ARTIFACT token.
#   FAMILY    a bracketed tag (`[ga10bprobe4a]`) — shared by design; the verb test does not apply.
#   OK        exactly one verb
# Each site is tagged `lit`, `arg` or `near`, and that tag IS the two-channel answer: `lit` means
# the bytes sit in a format string and will be in `.rodata`; `arg` means the emitter COMPOSES them
# at runtime from a literal in the SAME statement, so the wire carries them and the artifact reads
# 0 — `-> NO REPLY ::` is `ping/arg`; `near` is the same composition across STATEMENTS (`let arm =
# if … {"BCR-ALLHELD"} …` printed elsewhere), a weaker attribution that over-reports on purpose
# and must be read as "one of these", never as "all of these".
# `--verb V` asserts the token belongs to V and reds if the emitter set says otherwise: that is
# the MOVED-VERB check, and it is the one that would have caught SO47's shape.
#
# USAGE
#   scorer-token-uniqueness.sh --selftest                       # controls, three known answers
#   scorer-token-uniqueness.sh --verb arp 'NO REPLY (cache miss,'   # the gate form
#   scorer-token-uniqueness.sh 'TOKEN' ['TOKEN' …]              # ad-hoc census
#   scorer-token-uniqueness.sh --list FILE      # lines of `verb<TAB>token`, or a bare token
# Exit: 0 all OK · 1 a DEAD, SHARED or wrong-verb token · 2 usage/probe error (never a pass).
#
# LIMITS, AND A CORPUS MODE THAT WAS BUILT, MEASURED AND THEN REFUSED. A hole is expanded only
# from the SITE's own string literals. Letting a hole absorb ANY string is true — a hole really
# can hold anything — and useless: measured, it turned `-> NO REPLY ::` from 1 emitter into
# 5337. So a token that spans a COMPUTED value (`SCHED: task '{}' -> core`) reads 0 emitters here
# even though it is emitted on every boot. That is a WRONG-STRICT answer, and LAWS §5 says
# wrong-strict is worse than wrong-lenient — which is why the `--specs` sweep this script first
# carried IS NOT SHIPPED. It scored the whole `.spec` corpus by each directive's longest
# metacharacter-free run and returned 560 tokens: 329 OK, 151 DEAD, 80 SHARED, 37 unscoreable.
# A guard that fires on 41% of its corpus, most of them because the CHECK cannot see a runtime
# hole, trains the eye to skip the region. The question this script answers soundly is the
# NAMED one — "which emitters can produce THIS token, and what verb does each of them say" —
# so that is the only question it is allowed to ask. Point it at the tokens a scorer or a
# go-red cell actually asserts; do not point it at a corpus.
set -u
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
[ -d "$REPO/unaos/crates" ] || { echo "scorer-token-uniqueness: no unaos/crates under $REPO" >&2; exit 2; }
export WIREHYG_REPO="$REPO"
python3 - "$@" <<'PY'
import os, re, sys, itertools

REPO = os.environ["WIREHYG_REPO"]
SRC  = [os.path.join(REPO, "unaos", "crates"), os.path.join(REPO, "unaos", "libs")]
MACRO = re.compile(r'\b(?:serial_println|serial_print|println|print|write|writeln)\s*!\s*\(')
# A verdict PHRASE spliced into someone else's line through a `Display` impl. It has no verb of
# its own, and leaving it out was measurably wrong-strict: `NO ANSWER within budget` read DEAD
# while sitting in `.rodata`, because `DnsSay` writes it with `f.write_str`, not a macro.
PHRASE = re.compile(r'\b(?:write_str|push_str)\s*\(\s*"((?:[^"\\]|\\.)*)"')
LIT   = re.compile(r'"((?:[^"\\]|\\.)*)"')
HOLE  = re.compile(r'\{[^{}]*\}')
META  = set('.^$*+?()[]{}|\\')
CAP   = 20000

def unescape(s):
    return (s.replace('\\n','\n').replace('\\t','\t').replace('\\"','"')
             .replace("\\'","'").replace('\\\\','\\'))

def macro_bodies():
    """Every print-macro invocation, as (relpath, line, body) with balanced parens."""
    for base in SRC:
        for dp, _, fns in os.walk(base):
            if os.sep + 'target' + os.sep in dp + os.sep:
                continue
            for fn in fns:
                if not fn.endswith('.rs'):
                    continue
                p = os.path.join(dp, fn)
                try:
                    txt = open(p, encoding='utf-8', errors='replace').read()
                except OSError:
                    continue
                rel = os.path.relpath(p, REPO)
                for m in MACRO.finditer(txt):
                    i = m.end() - 1
                    j, n, depth, instr, esc = i, len(txt), 0, False, False
                    while j < n:
                        c = txt[j]
                        if instr:
                            if esc: esc = False
                            elif c == '\\': esc = True
                            elif c == '"': instr = False
                        elif c == '"': instr = True
                        elif c == '(': depth += 1
                        elif c == ')':
                            depth -= 1
                            if depth == 0: break
                        j += 1
                    yield rel, txt.count('\n', 0, m.start()) + 1, txt[i:j + 1]
                for m in PHRASE.finditer(txt):
                    yield rel, txt.count('\n', 0, m.start()) + 1, '"%s"\x01PHRASE' % m.group(1)

def verb_of(fmt):
    """The wire's SUBJECT word. `:: NET6:` is a `{}` hole (`P6`), so the verb is the first word
    after a leading hole, or after a leading `:: FAMILY:` written inline."""
    f = fmt.strip()
    f = re.sub(r'^\{[^{}]*\}\s*', '', f)
    f = re.sub(r'^::\s*[A-Za-z0-9_\-]+:\s*', '', f)
    f = re.sub(r'^\[[A-Za-z0-9_\-]+\]\s*', '', f)
    w = re.match(r'[A-Za-z0-9_\-]+', f)
    return w.group(0) if w else '<none>'

# A verdict word is often chosen in ONE statement (`let arm = if … {"BCR-ALLHELD"} else …`) and
# printed by ANOTHER through a `{}`. Harvesting only the macro's own literals misses that, and the
# miss is wrong-strict: `-> ACR-ACCEPTED` read 0 emitters while the wire carries it every boot. So
# a second, weaker fill pool is the literals NEAR the site in the same file; hits through it are
# tagged `near`, never `arg`, so a reader can see which answer they got.
NEAR = {}
for base in SRC:
    for dp, _, fns in os.walk(base):
        if os.sep + 'target' + os.sep in dp + os.sep:
            continue
        for fn in fns:
            if not fn.endswith('.rs'):
                continue
            fp = os.path.join(dp, fn)
            try:
                t = open(fp, encoding='utf-8', errors='replace').read()
            except OSError:
                continue
            idx = []
            for m in LIT.finditer(t):
                v = unescape(m.group(1))
                if v and len(v) <= 48 and '{' not in v:
                    idx.append((t.count('\n', 0, m.start()) + 1, v))
            NEAR[os.path.relpath(fp, REPO)] = idx

def near_pool(rel, line, span=80, cap=60):
    out = []
    for l, v in NEAR.get(rel, ()):
        if abs(l - line) <= span:
            out.append(v)
    return sorted(set(out))[:cap]

SITES = []
for rel, line, body in macro_bodies():
    lits = [unescape(l.group(1)) for l in LIT.finditer(body)]
    if not lits:
        continue
    fmt, args = lits[0], [a for a in lits[1:] if a and len(a) <= 64]
    if '{' not in fmt and len(fmt) < 4:
        continue
    verb = '<phrase>' if body.endswith('\x01PHRASE') else verb_of(fmt)
    SITES.append((rel, line, fmt, args, verb))

def produces(fmt, args, token, extra=()):
    """Can this site emit a line containing `token`?  Returns 'lit' | 'arg' | None.

    Alignment, not enumeration: the token is walked across the format's literal SEGMENTS, and
    where it runs off the end of one segment a `{}` hole must absorb the next piece. A hole
    absorbs one of the SITE's own string literals ('arg' — a NAMED answer, and the ping case).
    A hole holding a COMPUTED value is not expanded; see the LIMITS note at the head."""
    if token in fmt:
        return 'lit'
    segs = HOLE.split(fmt)
    if len(segs) == 1:
        return None
    fills = sorted(set(a for a in args if '{' not in a and a))
    tag = 'arg'
    if extra:
        fills = sorted(set(fills) | set(extra))
        tag = 'near'

    def walk(i, o, t, used_arg, budget):
        # consume `t` against segment i from offset o
        if budget[0] <= 0:
            return None
        budget[0] -= 1
        S = segs[i]
        while t and o < len(S):
            if S[o] != t[0]:
                return None
            t = t[1:]; o += 1
        if not t:
            return tag if used_arg else 'lit'
        if o < len(S) or i + 1 >= len(segs):
            return None
        best = None
        for f in fills:                      # hole absorbs one of the site's own literals
            k = 0
            while k < len(f) and k < len(t) and f[k] == t[k]:
                k += 1
            if k == len(t):
                return tag                   # token ends inside the fill
            if k == len(f) and k > 0:
                r = walk(i + 1, 0, t[k:], True, budget)
                if r: return tag
        # A hole carrying a RUNTIME value (an address, a count, a Display impl) is deliberately
        # NOT expanded: allowing a hole to absorb anything makes every format with a hole a
        # producer of every token, which is true and useless — measured, it turned 1 emitter
        # into 5337. The check is therefore sound for tokens composed from NAMED literals,
        # which is the class that moves verbs; a token that spans a computed value is not
        # scoreable from the sources at all and belongs to the artifact/wire channel instead.
        return best

    out = None
    for i, S in enumerate(segs):             # the token may start anywhere in any segment,
        for o in range(len(S) + 1):          # including at a segment boundary (i.e. in a hole)
            r = walk(i, o, token, False, [200000])
            if r in ('arg', 'near'):
                return r
            if r and out is None:
                out = r
    # …or INSIDE a fill: `REPLY ::` begins in ping's `"NO REPLY"` and ends in the next segment.
    for i in range(len(segs) - 1):
        for f in fills:
            for o in range(1, len(f)):
                k = 0
                while k < len(f) - o and k < len(token) and f[o + k] == token[k]:
                    k += 1
                if k == len(token):
                    return tag
                if k == len(f) - o and k > 0 and walk(i + 1, 0, token[k:], True, [200000]):
                    return tag
    return out

def score(token):
    hits = []
    for rel, line, fmt, args, verb in SITES:
        how = produces(fmt, args, token)
        if how:
            hits.append((rel, line, verb, how))
    if hits:
        return hits
    # Nothing from the macro-local pool: widen to the file-local one and say so with the tag.
    for rel, line, fmt, args, verb in SITES:
        how = produces(fmt, args, token, near_pool(rel, line))
        if how and (rel, line, verb, how) not in hits:
            hits.append((rel, line, verb, how))
    return hits

def render(token, hits, want_verb=None, label=''):
    verbs = sorted(set(h[2] for h in hits))
    sites = ', '.join('%s:%d[%s/%s]' % (r, l, v, h) for r, l, v, h in hits[:6])
    if len(hits) > 6:
        sites += ', +%d more' % (len(hits) - 6)
    # A `<phrase>` site is a verdict fragment spliced into somebody else's line through a
    # `Display` impl. It has no verb of its own, so it can neither conflict with one nor
    # satisfy a `--verb`: it is listed, and excluded from the conflict arithmetic.
    named = [v for v in verbs if v != '<phrase>']
    # A bracketed FAMILY tag (`[ga10bprobe4a]`) is shared by every line of its family BY DESIGN.
    # Running the verb test on one reports 84 emitters and says nothing; it is a family token, not
    # a verdict token, and the rule is about verdicts.
    if re.match(r'^\s*\[[A-Za-z0-9_\-]+\]\s*$', token):
        print('%-12s %2d emitter(s)  family tag — shared by design, verb test not applied  %s\n             token=%r'
              % ('FAMILY', len(hits), label, token))
        return 'FAMILY'
    if not hits:
        st = 'DEAD'
    elif named and want_verb is not None and want_verb not in named:
        st = 'WRONG-VERB(want %s)' % want_verb
    elif len(named) > 1:
        st = 'SHARED'
    elif not named:
        st = 'PHRASE'
    else:
        st = 'OK'
    print('%-12s %2d emitter(s)  verbs=%-24s %s%s\n             token=%r' %
          (st, len(hits), ','.join(verbs) or '-', label, sites, token))
    return st

argv = [a for a in sys.argv[1:]]
want = None
if '--verb' in argv:
    i = argv.index('--verb'); want = argv[i + 1]; del argv[i:i + 2]

print('# scorer-token-uniqueness: %d print sites harvested from unaos/{crates,libs}' % len(SITES))

if '--selftest' in argv:
    # Three controls with known answers. A probe that does not behave is exit 2 — no verdict.
    ok = True
    checks = [
        ('-> NO REPLY ::',        'ping', 1, 'composed: ping formats " -> {} ::" against "NO REPLY"'),
        ('NO REPLY (cache miss,', 'arp',  1, 'literal: the token SO47 prescribes for arp'),
        ('ZZ-NOT-AN-EMITTER-4242', None,  0, 'control: must be absent'),
    ]
    for tok, verb, n, why in checks:
        hits = score(tok)
        verbs = sorted(set(h[2] for h in hits))
        good = (len(hits) == n) and (verb is None or verbs == [verb])
        print('%-4s %-24r n=%d verbs=%s  (%s)' % ('PASS' if good else 'FAIL', tok, len(hits), verbs, why))
        ok = ok and good
    print('selftest', 'PASS' if ok else 'FAIL')
    sys.exit(0 if ok else 2)

tokens = []
if '--list' in argv:
    i = argv.index('--list'); path = argv[i + 1]; del argv[i:i + 2]
    for ln, raw in enumerate(open(path, encoding='utf-8'), 1):
        raw = raw.rstrip('\n')
        if not raw or raw.lstrip().startswith('#'):
            continue
        if '\t' in raw:
            v, t = raw.split('\t', 1)
            tokens.append((t, '[want %s] ' % v.strip(), v.strip()))
        else:
            tokens.append((raw, '', None))

tokens += [(t, '', want) for t in argv if not t.startswith('--')]
if not tokens:
    print('usage: scorer-token-uniqueness.sh [--selftest] [--list FILE] [--verb V] TOKEN...', file=sys.stderr)
    sys.exit(2)

bad = 0
for tok, label, w in tokens:
    st = render(tok, score(tok), w if w is not None else want, label)
    if st not in ('OK', 'PHRASE', 'FAMILY'):
        bad += 1
print('# %d token(s), %d not OK' % (len(tokens), bad))
sys.exit(1 if bad else 0)
PY
