#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# spec-coverage.sh — GATE-SPECCOV (zenbook-ledger Z58, SPECCOVERAGE; owed by ZENBOOKSPECPINS Z54).
# CHARTER: Aulë — a host gate of `unaos/arroyo` (GATES_SET row `speccov`, BP-gates-15); no kernel line.
#
# EVERY VERDICT A LANE'S TYPED VERBS PRINT IS SCORED BY THAT LANE'S SPEC. A fixture added to a typed verb prints its
# `:: TAG: … -> PASS|FAIL ::` on the lane's wire, and unless the lane's spec names TAG the line is scored by nothing but
# mbench's DEFAULT_FORBIDS (a FAIL reds; a vanished verdict, or a PASS with the wrong values, stays green). ALCPLAY-SPEC,
# GLYPHS-SPEC-QEMU and the Z54 trio (SUSPEND, ECQNAMES, DWI2CACPI) all landed that way, unscored.
#
# THE LANES are the `SPEC-COVERAGE:` markers in arroyo's COMMENTS, one per typed lane, each on the line that carries
# the lane's own verb list, as `SPEC-COVERAGE: <spec file> <verb> <verb> …` (the spec under scripts/specs/). The
# marker's verbs must EQUAL that line's own list — a `for v in <verb>:<TAG> …; do` loop's verbs (test-zenbook's
# typist), or the `tests <verb>` lines of the `scripts/specs/<f>.inject` the line names (kernel8-test --desk) — else
# `marker-drift` (rc 1): a verb typed but not in the marker would be a lane this gate silently stopped reading.
#
# A VERB'S TAGS, the heuristic measured on the zenbook lane at 611cf90b9: the verb's `register("<v>", <path>)` /
# `defer*("<v>", <path>)` sites anywhere under crates/kernel/src; the path resolved (absolute `crate::…`, `self::` /
# `super::`, relative to the registering module, inline `mod x {` blocks honoured) to ONE fn; that fn plus its
# SAME-FILE callee closure (the body alone missed DWI2CGPIO/DWI2CFAST and HDAPATH; the whole file over-collected
# ECBATTERY, DWI2C-TP, HDA-*); every string literal in those bodies that matches witness-owners.py's SITE (the tag
# universe, imported) AND carries `->` (the verdict word is often composed: `-> {} ::`). Cross-file callees are not
# followed (SPEC-COVERAGE-XFILE, owed); `[tag] … -> …` lines are outside the universe (SPEC-COVERAGE-BRACKET, owed).
# The lexer is tools/neutral-check.py's `lex` (imported, lock-type.py's idiom): brace depth over code only.
#
# A TAG IS SCORED when a REQUIRE / OPTIONAL / FORBID / PENDING / COMPLETE / COUNT line of the spec (mbench.py's six
# directive words) carries `:: TAG:` or an alternation `:: (A|TAG|…):`, or the spec carries `# COVERAGE-SKIP TAG <why>`
# (design-unscored; the why is mandatory). A skip for a tag the lane no longer prints is `stale-skip` (rc 1).
#
# usage: spec-coverage.sh                       every marked lane (the gate)
#        spec-coverage.sh <spec> <verb>…        one lane, no drift check
# out:   `  <spec> verb=<v> fn=<file>:<line> tags=<…>` per verb, then per lane
#        `✔ SPEC-COVERAGE: <spec> tags=<n> scored=<n> skip=<n>` or `✖ SPEC-COVERAGE: <spec> unscored=<TAG,…>`,
#        last `SPEC-COVERAGE: lanes=<n> unscored=<n> -> PASS|FAIL`.
# rc:    0 every tag scored · 1 unscored / stale-skip / marker-drift · 2 NO VERDICT (an import failed, the verb table is
#        under 200, no marker, a spec missing, a verb unregistered or unresolved, a skip with no why, or the CONTROL —
#        a synthetic module whose one verdict sits in a callee beside a `"{"` literal, a non-verdict and an uncalled
#        verdict beside it — did not yield exactly {ZZCTL}). A NO VERDICT is never a pass (BP-gates-2).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
command -v python3 >/dev/null 2>&1 || { echo "SPEC-COVERAGE: NO VERDICT — no python3"; exit 2; }
exec python3 - "$REPO" "$@" <<'PY'
import bisect, importlib.util, os, re, sys

REPO = sys.argv[1]
ARGS = sys.argv[2:]
UNAOS = os.path.join(REPO, 'unaos')
SRC = os.path.join(UNAOS, 'crates', 'kernel', 'src')
SPECS = os.path.join(UNAOS, 'scripts', 'specs')
ARROYO = os.path.join(UNAOS, 'arroyo')
MIN_VERBS = 200

DIRECTIVE = re.compile(r'^\s*(REQUIRE|OPTIONAL|FORBID|PENDING|COMPLETE|COUNT)\b')
NAMED = re.compile(r'::\s*\((?:\?:)?([A-Za-z0-9_|-]+)\)\s*:|::\s*([A-Za-z0-9_-]+):')
SKIP = re.compile(r'^\s*#\s*COVERAGE-SKIP\b[ \t]*(\S*)[ \t]*(.*?)\s*$')
MARKER = re.compile(r'SPEC-COVERAGE:\s+([\w.-]+\.spec)((?:[ \t]+[a-z0-9_-]+(?=\s|$))*)')
FORLIST = re.compile(r'\bfor\s+\w+\s+in\s+([^;]*);\s*do\b')
INJECT = re.compile(r'scripts/specs/([\w.-]+\.inject)')
REGTAIL = re.compile(r'\b(?:register|defer|defer_fast|defer_polled)\s*\(\s*$')
REGPATH = re.compile(r'^\s*,\s*((?:[A-Za-z_]\w*::)*[A-Za-z_]\w*)')
SCAN = re.compile(r'\bfn\s+([A-Za-z_]\w*)|\bmod\s+([A-Za-z_]\w*)|([A-Za-z_]\w*)(?=\s*\()|[{}()\[\];]')


class NoVerdict(Exception):
    pass


def load(rel, name):
    p = os.path.join(REPO, rel)
    spec = importlib.util.spec_from_file_location(name, p)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


try:
    LEX = load(os.path.join('tools', 'neutral-check.py'), 'neutral_check').lex
    SITE = load(os.path.join('unaos', 'scripts', 'witness-owners.py'), 'witness_owners').SITE
except Exception as e:   # the lexer or the tag universe is gone: no verdict, never a pass
    print('SPEC-COVERAGE: NO VERDICT — an import failed (tools/neutral-check.py lex, scripts/witness-owners.py SITE): %s' % e)
    sys.exit(2)

CACHE = {}


class Parsed:
    def __init__(self, fns, strs, calls, regs):
        self.fns, self.strs, self.calls, self.regs = fns, strs, calls, regs
        self.call_seq = [c[0] for c in calls]
        self.byname = {}
        for f in fns:
            self.byname.setdefault(f['name'], []).append(f)


def parse(path, text=None):
    """One file: its fns (name, line, seq span, inline-mod path, depth), string literals, calls, register sites."""
    if path in CACHE:
        return CACHE[path]
    if text is None:
        with open(path, encoding='utf-8', errors='replace') as fh:
            text = fh.read()
    fns, strs, calls, regs = [], [], [], []
    seq = depth = pb = 0
    stack, pending, prev, preg = [], None, '', None
    mods = lambda: tuple(e[1] for e in stack if e[0] == 'mod')
    for line0, kind, body in LEX(text):
        if kind != 'code':
            seq += 1
            strs.append((seq, line0, body))
            preg = (body, line0, mods()) if kind == 'str' and REGTAIL.search(prev) else None
            prev = ''
            continue
        if preg is not None:
            m = REGPATH.match(body)
            if m:
                regs.append((preg[0], m.group(1), preg[1], preg[2]))
            if body.strip():
                preg = None
        prev = prev + body if not body.strip() else body
        for m in SCAN.finditer(body):
            seq += 1
            ln = line0 + body.count('\n', 0, m.start())
            if m.group(1) or m.group(2):
                pending = ('fn' if m.group(1) else 'mod', m.group(1) or m.group(2), depth, pb, ln)
                continue
            if m.group(3):
                calls.append((seq, m.group(3)))
                continue
            c = m.group(0)
            if c in '([':
                pb += 1
            elif c in ')]':
                pb -= 1
            elif c == ';':
                if pending and pb == pending[3]:
                    pending = None   # `mod x;`, a trait method's declaration
            elif c == '{':
                if pending and pending[2] == depth and pending[3] == pb:
                    stack.append((pending[0], pending[1], depth, seq, pending[4]))
                    pending = None
                depth += 1
            elif c == '}':
                depth -= 1
                if stack and stack[-1][2] == depth:
                    k, n, d, a, l = stack.pop()
                    if k == 'fn':
                        fns.append({'name': n, 'line': l, 'a': a, 'b': seq, 'mods': mods(), 'depth': d})
    p = Parsed(fns, strs, calls, regs)
    CACHE[path] = p
    return p


def filemod(path):
    parts = os.path.relpath(path, SRC).replace(os.sep, '/').split('/')
    if parts[-1] == 'mod.rs':
        return parts[:-1]
    if len(parts) == 1 and parts[0] in ('main.rs', 'lib.rs'):
        return []
    parts[-1] = parts[-1][:-3]
    return parts


def modfile(segs):
    cands = [os.path.join(SRC, 'main.rs'), os.path.join(SRC, 'lib.rs')] if not segs else [
        os.path.join(SRC, *segs) + '.rs', os.path.join(SRC, *segs, 'mod.rs')]
    for c in cands:
        if c in CACHE or os.path.isfile(c):
            return c
    return None


def locate(segs, name):
    for i in range(len(segs), -1, -1):
        f = modfile(segs[:i])
        if not f:
            continue
        inner = tuple(segs[i:])
        hits = [fn for fn in parse(f).byname.get(name, []) if fn['mods'] == inner]
        if hits:
            return f, min(hits, key=lambda fn: (fn['depth'], fn['a']))
    return None


def resolve(pathstr, regfile, regmods):
    segs = pathstr.split('::')
    name, segs = segs[-1], segs[:-1]
    base = filemod(regfile) + list(regmods)
    if segs and segs[0] == 'crate':
        cands = [segs[1:]]
    elif segs and segs[0] == 'self':
        cands = [base + segs[1:]]
    elif segs and segs[0] == 'super':
        b = list(base)
        while segs and segs[0] == 'super':
            b, segs = b[:-1], segs[1:]
        cands = [b + segs]
    else:
        cands = [base + segs, segs]   # relative to the registering module, then from the crate root
    for c in cands:
        r = locate(c, name)
        if r:
            return r
    return None


def closure_tags(path, fn):
    p = parse(path)
    seen, work, spans = set(), [fn], []
    while work:
        f = work.pop()
        if f['a'] in seen:
            continue
        seen.add(f['a'])
        spans.append((f['a'], f['b']))
        lo, hi = bisect.bisect_left(p.call_seq, f['a']), bisect.bisect_right(p.call_seq, f['b'])
        for _, n in p.calls[lo:hi]:
            work.extend(p.byname.get(n, ()))
    tags = set()
    for s, _, body in p.strs:
        if '->' in body and any(a <= s <= b for a, b in spans):
            m = SITE.search('"' + body)
            if m:
                tags.add(m.group(1))
    return tags


def verb_table():
    table = {}
    for d, dirs, files in os.walk(SRC):
        dirs.sort()
        for f in sorted(files):
            if f.endswith('.rs'):
                p = os.path.join(d, f)
                for verb, ps, ln, mods in parse(p).regs:
                    table.setdefault(verb, []).append((p, ps, ln, mods))
    return table


def rel(p):
    return os.path.relpath(p, REPO).replace(os.sep, '/')


def read_spec(spec):
    sp = os.path.join(SPECS, spec)
    if not os.path.isfile(sp):
        raise NoVerdict('spec %s is not under unaos/scripts/specs/' % spec)
    named, skips = set(), {}
    with open(sp, encoding='utf-8', errors='replace') as fh:
        for i, l in enumerate(fh, 1):
            m = SKIP.match(l)
            if m:
                if not m.group(1) or not m.group(2):
                    raise NoVerdict('%s:%d COVERAGE-SKIP needs a TAG and its why' % (spec, i))
                skips[m.group(1)] = m.group(2)
                continue
            if DIRECTIVE.match(l):
                for a, b in NAMED.findall(l):
                    named.update(t for t in (a or b).split('|') if t)
    return named, skips


def lane(spec, verbs, table):
    """-> (rc, n unscored); prints the per-verb detail and the lane's line."""
    named, skips = read_spec(spec)
    tags = {}
    for v in verbs:
        if v not in table:
            raise NoVerdict('%s: verb %r has no register/defer site under crates/kernel/src' % (spec, v))
        for regfile, ps, ln, mods in table[v]:
            r = resolve(ps, regfile, mods)
            if not r:
                raise NoVerdict('%s: verb %r -> %s (%s:%d) resolves to no fn' % (spec, v, ps, rel(regfile), ln))
            ts = closure_tags(*r)
            print('  %s verb=%s fn=%s:%d tags=%s' % (spec, v, rel(r[0]), r[1]['line'], ','.join(sorted(ts)) or '-'))
            for t in ts:
                tags.setdefault(t, set()).add(v)
    scored = sorted(t for t in tags if t in named)
    skipped = sorted(t for t in tags if t not in named and t in skips)
    unscored = sorted(t for t in tags if t not in named and t not in skips)
    stale = sorted(t for t in skips if t not in tags)
    rc = 0
    if unscored:
        print('✖ SPEC-COVERAGE: %s unscored=%s' % (spec, ','.join(unscored)))
        rc = 1
    if stale:
        print('✖ SPEC-COVERAGE: %s stale-skip=%s' % (spec, ','.join(stale)))
        rc = 1
    if rc == 0:
        print('✔ SPEC-COVERAGE: %s tags=%d scored=%d skip=%d' % (spec, len(tags), len(scored), len(skipped)))
    return rc, len(unscored)


def markers():
    out = []
    with open(ARROYO, encoding='utf-8', errors='replace') as fh:
        for i, l in enumerate(fh, 1):
            for m in MARKER.finditer(l):
                if '#' in l[:m.start()]:
                    out.append((i, l, m.group(1), m.group(2).split()))
    return out


def drift(i, line, verbs):
    """The marked line's own verb list, compared with the marker's; None when they agree."""
    code = line.split(' # ', 1)[0]
    m = FORLIST.search(code)
    if m:
        listed = [t.split(':', 1)[0] for t in m.group(1).split()]
    else:
        j = INJECT.search(code)
        if not j:
            raise NoVerdict('arroyo:%d carries a SPEC-COVERAGE marker but no verb list (a `for v in` loop or a .inject)' % i)
        ip = os.path.join(SPECS, j.group(1))
        if not os.path.isfile(ip):
            raise NoVerdict('arroyo:%d names %s, which is not in scripts/specs/' % (i, j.group(1)))
        with open(ip, encoding='utf-8', errors='replace') as fh:
            listed = [l.split()[1] for l in fh if re.match(r'^tests\s+\S+', l)]
    if set(listed) != set(verbs):
        return 'line=%s marker=%s' % (','.join(listed) or '-', ','.join(verbs) or '-')
    return None


CONTROL = ('pub mod m {\n'
           '    pub fn f() { let _ = "{"; g(); }\n'
           '    fn g() { serial_println!(":: ZZCTL: n={} -> {} ::", 1, "PASS"); }\n'
           '    fn h() { serial_println!(":: ZZNO: info ::"); serial_println!(":: ZZOFF: x -> PASS ::"); }\n'
           '}\n'
           'pub fn ensure() { crate::tests::register("zzctl", m::f); }\n')


def control():
    path = os.path.join(SRC, 'zz_speccov_control.rs')
    p = parse(path, CONTROL)
    try:
        regs = [r for r in p.regs if r[0] == 'zzctl']
        r = resolve(regs[0][1], path, regs[0][3]) if regs else None
        got = closure_tags(*r) if r else None
    finally:
        del CACHE[path]
    if got != {'ZZCTL'}:
        raise NoVerdict('the CONTROL yielded %s, not {ZZCTL}: the resolver or the closure is broken' % (sorted(got) if got is not None else 'no fn'))


def main():
    control()
    table = verb_table()
    if len(table) < MIN_VERBS:
        raise NoVerdict('the verb table holds %d verbs (< %d): the register scan is broken' % (len(table), MIN_VERBS))
    worst, lanes, total = 0, 0, 0
    if ARGS:
        rc, n = lane(ARGS[0], ARGS[1:], table)
        worst, lanes, total = rc, 1, n
    else:
        ms = markers()
        if not ms:
            raise NoVerdict('no SPEC-COVERAGE marker in unaos/arroyo')
        for i, line, spec, verbs in ms:
            if not verbs:
                raise NoVerdict('arroyo:%d marker for %s names no verb' % (i, spec))
            d = drift(i, line, verbs)
            if d:
                print('✖ SPEC-COVERAGE: %s marker-drift arroyo:%d %s' % (spec, i, d))
                worst = max(worst, 1)
            rc, n = lane(spec, verbs, table)
            worst, lanes, total = max(worst, rc), lanes + 1, total + n
    print('SPEC-COVERAGE: lanes=%d unscored=%d -> %s' % (lanes, total, 'PASS' if worst == 0 else 'FAIL'))
    return worst


try:
    sys.exit(main())
except NoVerdict as e:
    print('SPEC-COVERAGE: NO VERDICT — %s' % e)
    sys.exit(2)
PY
