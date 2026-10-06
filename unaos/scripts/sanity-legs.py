#!/usr/bin/env python3
# sanity-legs.py — GATE-SANITY (SANITYLEGS, rmbp-ledger B488): every compile-time-sanity assert gets a leg.
#
# WHY. Flight 26 (image 19) rebooted on EVERY boot at `video/winmenu.rs:2269 assertion failed: BAR_BOXES_MAX ==
# MENU_TITLES_MAX + 2`: an assert over CONSTANTS that ran at boot (UIMETRICS' `metrics::ignite`) and that no compile
# leg ever evaluated, so APPMENU2's `+ 5` and the stale `+ 2` met for the first time on the glass. An assert over
# consts belongs in a `const _: () = assert!(…)` (every compile leg proves it), and one over runtime metrics in a
# `tests` leg (R80: never at boot).
#
# WHAT IT REFUSES (exit 1, each finding `sanity|<file>|<fn>|<expr>`):
#   S1 a runtime `assert!` / `assert_eq!` / `assert_ne!` in the kernel (unaos/crates/kernel/src) whose operands are
#      all CONST-SHAPED — UPPER_SNAKE items, UPPER() metric readers, literals, `a::b::` paths, `.len()` — outside a
#      `const _` / `const NAME:` / `static` initialiser, a `const fn`, a `#[cfg(test)]` / `#[test]` item, or a tests
#      fixture fn (name contains `selftest`, `fixture` or `sanity`: reached only from the `tests` verb).
#   S2 `video::metrics::ignite` (the boot's first desktop pass) calling a `uimetrics_*` relation fn or `panic!`.
#   S3 `SANITY_CONST_ASSERTS` in video/metrics.rs absent or disagreeing with the number of `const _: () = assert!` in the
#      `uimetrics_sanity*` fns (the number `tests sanity` prints as `const_asserts=`).
# CONTROLS (exit 2, no verdict): the scope enumerates >= 200 files; metrics.rs declares `fn ignite`; --selftest's fixture goes red on each plant and green on each allowed shape.
#
# usage: sanity-legs.py [--selftest] [<unaos dir>]      exit 0 clean · 1 findings · 2 control
import os, re, sys, tempfile

KSRC = os.path.join('crates', 'kernel', 'src')
METRICS = os.path.join(KSRC, 'video', 'metrics.rs')
MAC = re.compile(r'\b(assert|assert_eq|assert_ne)!\s*\(')
IDENT = re.compile(r'[A-Za-z_][A-Za-z0-9_]*')
FIXTURE_FN = re.compile(r'selftest|fixture|sanity')
NEUTRAL = {'as', 'usize', 'u8', 'u16', 'u32', 'u64', 'u128', 'i8', 'i16', 'i32', 'i64', 'isize', 'crate',
           'super', 'self', 'px', 'true', 'false'}


def strip(t):
    """Comments blanked (newlines kept), string and char literals kept / blanked; offsets preserved."""
    out, i, n = [], 0, len(t)
    while i < n:
        c = t[i]
        if c == '"':
            j = i + 1
            while j < n and t[j] != '"':
                j += 2 if t[j] == '\\' else 1
            out.append(t[i:j + 1]); i = j + 1; continue
        if t.startswith('//', i):
            j = t.find('\n', i); j = n if j < 0 else j
            out.append(' ' * (j - i)); i = j; continue
        if t.startswith('/*', i):
            j = t.find('*/', i); j = n if j < 0 else j + 2
            out.append(re.sub(r'[^\n]', ' ', t[i:j])); i = j; continue
        if c == "'":
            m = re.match(r"'(\\.[^']*|[^'\\])'", t[i:])
            if m:
                out.append(' ' * len(m.group(0))); i += len(m.group(0)); continue
        out.append(c); i += 1
    return ''.join(out)


def blocks(t):
    """[(open, close, kind, name)] for every brace block; kind fn|cfn|const|test|blk."""
    res, st, last = [], [], 0
    for m in re.finditer(r'(?s)[{};]|"(?:[^"\\]|\\.)*"', t):
        c = m.group(0)
        if c == '{':
            head = t[last:m.start()]
            f = re.search(r'\bfn\s+([A-Za-z0-9_]+)', head)
            if re.search(r'cfg\(test\)|#\[test\]', head):
                k = ('test', '')
            elif re.search(r'\bconst\s+(_|[A-Z][A-Z0-9_]*)\s*:|\bstatic\s+(mut\s+)?[A-Z]', head) and not f:
                k = ('const', '')
            elif f:
                k = ('cfn' if re.search(r'\bconst\s+(unsafe\s+)?fn\b', head) else 'fn', f.group(1))
            else:
                k = ('blk', '')
            st.append((m.start(), k)); last = m.end()
        elif c == '}':
            if st:
                o, k = st.pop(); res.append((o, m.start(), k[0], k[1]))
            last = m.end()
        elif c == ';':
            last = m.end()
    return res


def call_args(t, i):
    d, j, s, parts = 0, i, i + 1, []
    while j < len(t):
        c = t[j]
        if c == '"':
            k = j + 1
            while k < len(t) and t[k] != '"':
                k += 2 if t[k] == '\\' else 1
            j = k + 1; continue
        if c in '([{':
            d += 1
        elif c in ')]}':
            d -= 1
            if d == 0:
                parts.append(t[s:j]); return parts
        elif c == ',' and d == 1:
            parts.append(t[s:j]); s = j + 1
        j += 1
    return parts


def constish(e):
    e = re.sub(r'(?s)"(?:[^"\\]|\\.)*"', '', e)
    e = re.sub(r'\.len\(\)', '', e)
    toks = IDENT.findall(re.sub(r'\b\d\w*', '', e))
    if not toks:
        return False
    for k in toks:
        if k in NEUTRAL or re.fullmatch(r'[A-Z][A-Z0-9_]*', k):
            continue
        if re.fullmatch(r'[a-z][a-z0-9_]*', k) and re.search(r'\b' + k + r'\s*::', e):
            continue
        return False
    return True


def scan_file(rel, raw):
    """(findings, const_in_sanity_fns, runtime_const_asserts_allowed)"""
    t = strip(raw)
    bl = blocks(t)
    finds, allowed = [], 0
    for m in MAC.finditer(t):
        if t[max(0, m.start() - 6):m.start()].endswith('debug_'):
            continue
        parts = call_args(t, m.end() - 1)
        e = ' && '.join(parts[:1] if m.group(1) == 'assert' else parts[:2])
        if not constish(e):
            continue
        enc = sorted([b for b in bl if b[0] < m.start() < b[1]], key=lambda b: b[0])
        line_start = t.rfind('\n', 0, m.start()) + 1
        stmt_head = t[max(t.rfind(';', 0, m.start()), t.rfind('{', 0, m.start()), t.rfind('}', 0, m.start())) + 1:m.start()]
        fn = next((b[3] for b in reversed(enc) if b[2] in ('fn', 'cfn')), '-')
        if re.search(r'\bconst\s+(_|[A-Z][A-Z0-9_]*)\s*:', stmt_head) or any(b[2] in ('const', 'cfn', 'test') for b in enc):
            continue
        if FIXTURE_FN.search(fn):
            allowed += 1
            continue
        finds.append(f"sanity|{rel}|{fn}|{' '.join(e.split())[:120]}")
    csan = 0
    for b in bl:
        if b[2] == 'fn' and b[3].startswith('uimetrics_sanity'):
            csan += len(re.findall(r'\bconst\s+_\s*:\s*\(\)\s*=\s*assert!', t[b[0]:b[1]]))
    return finds, csan, allowed


def check(unaos):
    root = os.path.join(unaos, KSRC)
    files, finds, csan, allowed = 0, [], 0, 0
    for d, dirs, fs in os.walk(root):
        dirs[:] = [x for x in dirs if x != 'target']
        for f in fs:
            if not f.endswith('.rs'):
                continue
            p = os.path.join(d, f)
            files += 1
            a, b, c = scan_file(os.path.relpath(p, unaos), open(p, encoding='utf-8', errors='replace').read())
            finds += a; csan += b; allowed += c
    mp = os.path.join(unaos, METRICS)
    if files < 200 or not os.path.isfile(mp):
        print(f'GATE-SANITY: CONTROL — scope files={files} metrics.rs={os.path.isfile(mp)} -> NO VERDICT'); return 2
    ms = strip(open(mp, encoding='utf-8').read())
    k = re.search(r'\bSANITY_CONST_ASSERTS\s*:\s*usize\s*=\s*(\d+)', ms)
    ig = [b for b in blocks(ms) if b[2] == 'fn' and b[3] == 'ignite']
    if not ig:
        print('GATE-SANITY: CONTROL — metrics.rs lacks fn ignite -> NO VERDICT'); return 2
    body = ms[ig[0][0]:ig[0][1]]
    for bad in re.findall(r'\buimetrics_\w+|\bpanic!', body):
        finds.append(f'sanity|{METRICS}|ignite|boot calls {bad} (R80: relations run under `tests sanity`)')
    if not k:
        finds.append(f'sanity|{METRICS}|SANITY_CONST_ASSERTS|not declared (`tests sanity` cannot print const_asserts=)')
    elif int(k.group(1)) != csan:
        finds.append(f'sanity|{METRICS}|SANITY_CONST_ASSERTS|declares {k.group(1)} but the uimetrics_sanity fns hold {csan}')
    for f in finds:
        print(f'  ❌ {f}')
    print(f'GATE-SANITY: files={files} const_asserts={csan} fixture_asserts={allowed} findings={len(finds)} -> '
          f'{"PASS" if not finds else "FAIL"}')
    return 1 if finds else 0


def selftest():
    import contextlib, io
    ok = True
    with tempfile.TemporaryDirectory() as td:
        src = os.path.join(td, KSRC)
        os.makedirs(os.path.join(src, 'video'))
        for i in range(200):
            open(os.path.join(src, f'f{i}.rs'), 'w').write('pub fn f() {}\n')
        met = os.path.join(td, METRICS)
        good_m = ('pub const SANITY_CONST_ASSERTS: usize = 1;\npub fn ignite() { let x = 1; }\n'
                  'pub fn uimetrics_sanity(ck: &mut u8) { const _: () = assert!(A >= 1); }\n')
        cases = [
            ('clean', good_m, '', 0),
            ('runtime-const', good_m, 'fn layout() { assert!(BAR_BOXES_MAX == MENU_TITLES_MAX + 2); }\n', 1),
            ('runtime-metric', good_m, 'pub fn lay() {\n    assert!(CELL_H() <= BAR_H(), "m");\n}\n', 1),
            ('path-const', good_m, 'fn a() { assert_eq!(wm::MAX_TITLE, 16); }\n', 1),
            ('const-block', good_m, 'const _: () = assert!(A == B + 5);\nfn a() { const _: () = assert!(A <= 4); }\n', 0),
            ('const-fn', good_m, 'const fn a() -> u8 { assert!(A > 0); 1 }\n', 0),
            ('cfg-test', good_m, '#[cfg(test)]\nmod tests { fn t() { assert!(A == 2); } }\n', 0),
            ('fixture-fn', good_m, 'pub fn dock_selftest() { assert!(A == 2); }\n', 0),
            ('runtime-value', good_m, 'fn a(x: u8) { assert!(x > 0); assert!(len > A); }\n', 0),
            ('continued-str', good_m, 'fn a() { if x { p!("a \\\n b {}", 1); } }\nfn b() { assert!(A == 2); }\n', 1),
            ('comment', good_m, 'fn a() { // assert!(A == 2);\n}\n', 0),
            ('ignite-call', good_m.replace('let x = 1;', 'crate::video::wm::uimetrics_sanity_title_cell(&mut c);'), '', 1),
            ('ignite-panic', good_m.replace('let x = 1;', 'panic!("x");'), '', 1),
            ('count-drift', good_m.replace('= 1;', '= 2;'), '', 1),
        ]
        for name, mtext, plant, want in cases:
            open(met, 'w').write(mtext)
            open(os.path.join(src, 'video', 'zz_plant.rs'), 'w').write(plant)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = check(td)
            good = rc == want
            ok &= good
            print(f'  case={name} rc={rc} want={want} -> {"ok" if good else "WRONG"}')
            if not good:
                print(buf.getvalue())
    print(f'GATE-SANITY selftest: {"PASS" if ok else "FAIL"}')
    return 0 if ok else 2


def main(argv):
    if '--selftest' in argv:
        return selftest()
    rest = [a for a in argv if not a.startswith('--')]
    unaos = rest[0] if rest else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
    rc = selftest_quiet()
    if rc:
        print('GATE-SANITY: CONTROL — selftest failed -> NO VERDICT'); return 2
    return check(unaos)


def selftest_quiet():
    import contextlib, io
    with contextlib.redirect_stdout(io.StringIO()):
        return selftest()


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
