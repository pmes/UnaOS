#!/usr/bin/env python3
# attrkeys-check.py — GATE-ATTRKEYS: one attribute-key registry for both rings (ATTRKEYS, rmbp-ledger B452).
#
# WHY. ARCH-2026-10-06 F9: the attribute names (`una:*`, `media:*`, `doc:*`, `image:*`, `job:*`, `bt.*`) had six
# homes and raw literals besides; a respelled key is a silent on-disk format split (R79: one store, one name).
# `una_abi::attr_keys` (unaos/crates/una-abi/src/lib.rs) is now the ONE home; every other site names the constant.
#
# HOW. Every `.rs` file of both rings (unaos/crates, unaos/libs, libs, handlers, vessels, tools; `target/` skipped)
# is read; a string literal that STARTS with a registered namespace (`"una:` `"media:` `"doc:` `"image:` `"job:`
# `"bt.`) in CODE (line and trailing `//` comments skipped) outside the registry file is a finding
# `attrkey|<repo-relative file>|<literal>`. Today's non-key sites are the BASELINE (scripts/attrkeys.baseline): a
# baselined key passes, a NEW one fails, a baselined one that no longer matches fails as STALE (shrink-only).
# One leg, keyed the arch-check way, so GATE-ARCH (arch-check.py) can absorb it at the fold.
#
# CONTROLS (exit 2, NO verdict): the scope enumerates >= 200 files; the registry file exists and declares
# `pub mod attr_keys`; the baseline parses; --selftest's fixture goes red on a planted literal and on a stale row,
# green once baselined, and ignores a literal in a comment.
#
# usage: attrkeys-check.py [--selftest] [--emit-baseline] [<unaos dir>]  exit 0 clean · 1 new or stale · 2 control
import contextlib, io, os, re, sys, tempfile

REGISTRY = os.path.join('unaos', 'crates', 'una-abi', 'src', 'lib.rs')
SCOPE = [os.path.join('unaos', 'crates'), os.path.join('unaos', 'libs'), 'libs', 'handlers', 'vessels', 'tools']
LIT_RE = re.compile(r'"((?:una:|media:|doc:|image:|job:|bt\.)[^"\\]*)')


def code_part(line):
    """The line up to a `//` that is not inside a string literal."""
    ins, esc, i = False, False, 0
    while i < len(line):
        c = line[i]
        if ins:
            if esc:
                esc = False
            elif c == '\\':
                esc = True
            elif c == '"':
                ins = False
        else:
            if c == '"':
                ins = True
            elif line.startswith('//', i):
                return line[:i]
        i += 1
    return line


def scan(root):
    keys, n = set(), 0
    for top in SCOPE:
        base = os.path.join(root, top)
        for d, dirs, files in os.walk(base):
            dirs[:] = [x for x in dirs if x != 'target' and not x.startswith('.')]
            for f in files:
                if not f.endswith('.rs'):
                    continue
                p = os.path.join(d, f)
                rel = os.path.relpath(p, root)
                n += 1
                if rel == REGISTRY:
                    continue
                try:
                    lines = open(p, encoding='utf-8', errors='replace').read().split('\n')
                except OSError:
                    continue
                for line in lines:
                    for m in LIT_RE.finditer(code_part(line)):
                        keys.add(f'attrkey|{rel}|{m.group(1)}')
    return keys, n


def load_baseline(path):
    out = set()
    if not os.path.isfile(path):
        return out
    for line in open(path, encoding='utf-8'):
        line = line.rstrip('\n')
        if not line or line.startswith('#'):
            continue
        if not line.startswith('attrkey|') or line.count('|') < 2:
            raise ValueError(f'unparsable baseline row: {line!r}')
        out.add(line)
    return out


def verdict(root, baseline_path, quiet=False, min_files=200):
    reg = os.path.join(root, REGISTRY)
    if not os.path.isfile(reg) or 'pub mod attr_keys' not in open(reg, encoding='utf-8', errors='replace').read():
        print(f'GATE-ATTRKEYS: CONTROL — registry {REGISTRY} missing or has no `pub mod attr_keys`')
        return 2
    keys, n = scan(root)
    if n < min_files:
        print(f'GATE-ATTRKEYS: CONTROL — scope enumerated {n} files (< {min_files})')
        return 2
    try:
        base = load_baseline(baseline_path)
    except ValueError as e:
        print(f'GATE-ATTRKEYS: CONTROL — {e}')
        return 2
    new, stale = sorted(keys - base), sorted(base - keys)
    for k in new:
        print(f'GATE-ATTRKEYS: NEW   {k}  — name the una_abi::attr_keys constant (add the key there if it is new)')
    for k in stale:
        print(f'GATE-ATTRKEYS: STALE {k}  — the site is gone: delete the baseline row (shrink-only)')
    if not quiet:
        print(f'GATE-ATTRKEYS: files={n} findings={len(keys)} baseline={len(base)} new={len(new)} stale={len(stale)}')
    return 1 if (new or stale) else 0


def selftest():
    with tempfile.TemporaryDirectory() as t, contextlib.redirect_stdout(io.StringIO()):
        os.makedirs(os.path.join(t, os.path.dirname(REGISTRY)))
        open(os.path.join(t, REGISTRY), 'w').write('pub mod attr_keys { pub const TYPE: &str = "una:type"; }\n')
        src = os.path.join(t, 'handlers', 'x', 'src')
        os.makedirs(src)
        for i in range(3):
            open(os.path.join(src, f'f{i}.rs'), 'w').write('// a "una:type" in a comment is not a finding\n'
                                                             'let s = "http://x"; // "media:no"\n')
        bl = os.path.join(t, 'bl')
        ok = verdict(t, bl, True, 3) == 0
        open(os.path.join(src, 'f0.rs'), 'a').write('let k = "media:planted";\n')
        ok &= verdict(t, bl, True, 3) == 1
        open(bl, 'w').write('attrkey|handlers/x/src/f0.rs|media:planted\n')
        ok &= verdict(t, bl, True, 3) == 0
        open(bl, 'a').write('attrkey|handlers/x/src/f1.rs|una:gone\n')
        ok &= verdict(t, bl, True, 3) == 1
        open(bl, 'w').write('garbage\n')
        ok &= verdict(t, bl, True, 3) == 2
        ok &= verdict(t, bl, True, 99) == 2
    print(f'GATE-ATTRKEYS selftest: {"PASS" if ok else "FAIL"}')
    return 0 if ok else 2


def main(argv):
    args = [a for a in argv if not a.startswith('--')]
    unaos = os.path.abspath(args[0] if args else os.path.join(os.path.dirname(__file__), '..'))
    root = os.path.dirname(unaos)
    bl = os.path.join(unaos, 'scripts', 'attrkeys.baseline')
    if '--selftest' in argv:
        return selftest()
    if '--emit-baseline' in argv:
        keys, _ = scan(root)
        print('# GATE-ATTRKEYS baseline (B452): key-shaped literals outside una_abi::attr_keys. SHRINK-ONLY.')
        for k in sorted(keys):
            print(k)
        return 0
    rc = selftest()
    if rc != 0:
        return 2
    return verdict(root, bl)


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
