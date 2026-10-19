#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# neutral-check — GATE-NEUTRAL (GATENEUTRAL, LEDGER.md SR166; BP-shape-4, R16). Peter, 2026-09-03: names in any file
# both arches compile are SUBSYSTEM names, never board names (`[pwrreboot]`, not `[orinreboot]`; LAWS.md:185).
#
# SCOPE. Every `.rs` file under unaos/crates/kernel/src/ except arch/ (arch/<isa>/ compiles on one arch and holds the
# board-specific files, e.g. arch/aarch64/*_tegra.rs). No drivers/ directory is board-specific at fb8ad2f4
# (BOARD_DIRS below is empty; a dir that becomes one is named there, with its reason, and in docs/dev/blueprint/gates.md).
#
# WHAT IS A FINDING. A board name — a word part that starts with `orin`, `rmbp`, `pi4`, `jetson` or `tegra`, or ends
# with `rmbp`, `pi4`, `jetson` or `tegra` (an optional trailing digit run and letter: `tegra234`, `raspi4b`) — in
#   ident  an identifier in CODE (comments are stripped: `//`, `/* */`, doc comments), or
#   str    an identifier-shaped word inside a string literal (a serial tag `[orinreboot]`, a wire line, a knob name).
# Word parts split on `_` and camelCase, so `storing`/`restoring`/`integrated` are not findings and `orinreboot` is.
# NOT findings: a citation inside a string (`rmbp-ledger B188`, the round `orin 17`, the capture `rmbp1-boot1`: the
# ledger or evidence file is named, not code — the string-side twin of a comment citing a ledger id); the string after `feature =` (a `cfg(feature = "tegra")` knob gate, the knob registry's business);
# an identifier that IS the stem of a module under arch/ (`display_tegra`, `xusb_tegra`: a path to the board-specific
# file, which lives where R16 lets it live).
# A finding's key is `<repo-relative file>  <token>` (no line number, so a line drift is not churn); the first line it
# occurs on is printed. tools/neutral.allow holds the ruled exceptions, one row per key with its REASON; a key with
# no row is NEW (exit 1); a row that matches nothing is STALE (exit 1: the allowlist only shrinks).
#
# CONTROLS (exit 2, NO verdict): the scope enumerates >= 200 files and arch/ holds >= 1 board-named module; every
# allowlist row parses (file, token, a non-empty reason); --selftest's fixture goes red on a planted identifier and a
# planted serial tag, stays green on a comment, a `feature = "tegra"` gate, an arch module path and `restoring`,
# green once allowlisted, red on a stale row, and 2 on a row without a reason. main() runs the selftest first.
#
#   tools/neutral-check.py [<repo root>]          check (exit 0 clean, 1 new or stale, 2 control)
#   tools/neutral-check.py --selftest             the fixture alone
#   tools/neutral-check.py --emit [<repo root>]   print every finding as `<file>  <token>` (an allowlist row without its reason, which refuses to parse)
import contextlib, io, os, re, sys, tempfile

SRC = os.path.join('unaos', 'crates', 'kernel', 'src')
ARCH = 'arch'
BOARD_DIRS = ()   # kernel-src-relative dirs outside arch/ that are board-specific (none at fb8ad2f4)
PART = re.compile(r'^(?:orin|rmbp|pi4|jetson|tegra)|(?:rmbp|pi4|jetson|tegra)\d*[a-z]?$')
WORD = re.compile(r'[A-Za-z_][A-Za-z0-9_]*')
FEATURE_TAIL = re.compile(r'\bfeature\s*=\s*$')
# a citation inside a string: a ledger or queue FILE (`rmbp-ledger B188`), a round (`orin 17`), a capture (`rmbp1-boot1`)
CITE = re.compile(r'\b(?:rmbp|orin|pi|pi4|jetson)-(?:ledger|queue)\b|\b(?:rmbp|orin|pi)\s+\d+\b|\b(?:rmbp|orin|pi)\d+-boot\w*', re.I)


def parts(tok):
    out = []
    for p in tok.split('_'):
        out += re.findall(r'[A-Z]+(?![a-z])\d*|[A-Z]?[a-z]+\d*|\d+', p)
    return out


def is_board(tok):
    return any(PART.search(p.lower()) for p in parts(tok))


def lex(text):
    """Yield (line, kind, text) for code runs ('code') and string literal bodies ('str'); comments dropped. A string
    that directly follows `feature =` in code is yielded as kind 'feat'."""
    i, n, line, code, start = 0, len(text), 1, [], 1
    def flush():
        nonlocal code
        s = ''.join(code)
        code = []
        return s
    tail = ''
    while i < n:
        c = text[i]
        if text.startswith('//', i):
            j = text.find('\n', i)
            i = n if j < 0 else j
            continue
        if text.startswith('/*', i):
            d, i = 1, i + 2
            while i < n and d:
                if text.startswith('/*', i):
                    d, i = d + 1, i + 2
                elif text.startswith('*/', i):
                    d, i = d - 1, i + 2
                else:
                    line += text[i] == '\n'
                    i += 1
            code.append(' ')
            continue
        raw = None
        if c in 'rb' and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_')):
            raw = re.match(r'b?r(#*)"', text[i:i + 12])
        if raw or c == '"' or (c == 'b' and text.startswith('b"', i) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_'))):
            s = flush()
            tail = (tail + s)[-64:]
            if s:
                yield start, 'code', s
            sl = line
            if raw:
                end = '"' + raw.group(1)
                j = text.find(end, i + len(raw.group(0)))
                j = n if j < 0 else j
                body = text[i + len(raw.group(0)):j]
                i = j + len(end)
            else:
                j = i + (2 if c == 'b' else 1)
                buf = []
                while j < n and text[j] != '"':
                    if text[j] == '\\':
                        buf.append(text[j:j + 2]); j += 2; continue
                    buf.append(text[j]); j += 1
                body = ''.join(buf)
                i = j + 1
            line += body.count('\n')
            yield sl, ('feat' if FEATURE_TAIL.search(tail) else 'str'), body
            tail = ''
            start = line
            continue
        if c == "'":
            m = re.match(r"'(?:\\u\{[0-9a-fA-F]+\}|\\x[0-9a-fA-F]{2}|\\.|[^\\'\n])'", text[i:i + 14])
            if m:
                i += len(m.group(0)); code.append(' ')
                continue
        if c == '\n':
            s = flush()
            tail = (tail + s)[-64:]
            if s:
                yield start, 'code', s
            line += 1
            start = line
            i += 1
            continue
        code.append(c)
        i += 1
    s = flush()
    if s:
        yield start, 'code', s


def arch_modules(src):
    mods = set()
    for d, dirs, files in os.walk(os.path.join(src, ARCH)):
        for x in dirs + files:
            stem = x[:-3] if x.endswith('.rs') else x
            if is_board(stem):
                mods.add(stem)
    return mods


def scan(root):
    """Return ({key: (first line, kind)}, files scanned, arch module set)."""
    src = os.path.join(root, SRC)
    mods = arch_modules(src)
    found, nfiles = {}, 0
    for d, dirs, files in os.walk(src):
        rel = os.path.relpath(d, src)
        if rel == ARCH or rel.startswith(ARCH + os.sep) or any(rel == b or rel.startswith(b + os.sep) for b in BOARD_DIRS):
            dirs[:] = []
            continue
        dirs.sort()
        for f in sorted(files):
            if not f.endswith('.rs'):
                continue
            p = os.path.join(d, f)
            rp = os.path.relpath(p, root).replace(os.sep, '/')
            nfiles += 1
            try:
                text = open(p, encoding='utf-8', errors='replace').read()
            except OSError:
                continue
            for ln, kind, body in lex(text):
                if kind == 'feat':
                    continue
                if kind == 'str':
                    body = CITE.sub(' ', body)
                for t in WORD.findall(body):
                    if t in mods and kind == 'code':
                        continue
                    if is_board(t):
                        found.setdefault((rp, t), (ln, 'ident' if kind == 'code' else 'str'))
    return found, nfiles, mods


def load_allow(path):
    rows = {}
    if not os.path.isfile(path):
        return rows
    for i, line in enumerate(open(path, encoding='utf-8'), 1):
        line = line.rstrip('\n')
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        f = line.split(None, 2)
        if len(f) < 3 or not f[2].strip():
            raise ValueError('allowlist row %d has no reason: %r' % (i, line))
        rows[(f[0], f[1])] = f[2].strip()
    return rows


def verdict(root, allow_path, quiet=False, min_files=200):
    found, n, mods = scan(root)
    if n < min_files:
        print('NEUTRAL: CONTROL — scope enumerated %d files (< %d) under %s' % (n, min_files, SRC))
        return 2
    if not mods:
        print('NEUTRAL: CONTROL — no board-named module under %s/%s (the exemption set is empty: the pattern is broken)' % (SRC, ARCH))
        return 2
    try:
        allow = load_allow(allow_path)
    except ValueError as e:
        print('NEUTRAL: CONTROL — %s' % e)
        return 2
    new = sorted(k for k in found if k not in allow)
    stale = sorted(k for k in allow if k not in found)
    for k in new:
        ln, kind = found[k]
        print('NEUTRAL: NEW   %s:%d %s %s — name it for the subsystem (R16), or add a tools/neutral.allow row with the reason' % (k[0], ln, kind, k[1]))
    for k in stale:
        print('NEUTRAL: STALE %s %s — no longer in the file: delete the allowlist row (shrink-only)' % k)
    if not quiet:
        print('NEUTRAL: files=%d findings=%d allowed=%d new=%d stale=%d -> %s' % (
            n, len(found), len(allow), len(new), len(stale), 'FAIL' if (new or stale) else 'PASS'))
    return 1 if (new or stale) else 0


def selftest():
    ok = True
    with tempfile.TemporaryDirectory() as t, contextlib.redirect_stdout(io.StringIO()):
        src = os.path.join(t, SRC)
        os.makedirs(os.path.join(src, ARCH, 'aarch64'))
        open(os.path.join(src, ARCH, 'aarch64', 'display_tegra.rs'), 'w').write('pub fn orin_only() {}\n')
        for i in range(3):
            open(os.path.join(src, 'f%d.rs' % i), 'w').write(
                '// orinreboot in a comment is not a finding\n'
                '/* nor [orinreboot] in a block */\n'
                '#[cfg(feature = "tegra")]\n'
                'fn restoring() { crate::arch::aarch64::display_tegra::up(); let s = "storing"; }\n')
        al = os.path.join(t, 'allow')
        ok &= verdict(t, al, True, 3) == 0
        open(os.path.join(src, 'f0.rs'), 'a').write('pub fn orinreboot() {}\n')
        ok &= verdict(t, al, True, 3) == 1
        open(al, 'w').write('%s/f0.rs orinreboot fixture\n' % SRC.replace(os.sep, '/'))
        ok &= verdict(t, al, True, 3) == 0
        open(os.path.join(src, 'f2.rs'), 'a').write('fn q() { log("[pwr] up (rmbp-ledger B188, orin 17, rmbp1-boot1)"); }\n')
        ok &= verdict(t, al, True, 3) == 0
        open(os.path.join(src, 'f1.rs'), 'a').write('fn p() { log("[pwrtegra] up"); }\n')
        ok &= verdict(t, al, True, 3) == 1
        open(al, 'a').write('%s/f1.rs pwrtegra fixture\n%s/f2.rs rmbpgone fixture\n' % ((SRC.replace(os.sep, '/'),) * 2))
        ok &= verdict(t, al, True, 3) == 1
        open(al, 'w').write('%s/f0.rs orinreboot\n' % SRC.replace(os.sep, '/'))
        ok &= verdict(t, al, True, 3) == 2
        ok &= verdict(t, os.path.join(t, 'none'), True, 99) == 2
    print('NEUTRAL selftest: %s' % ('PASS' if ok else 'FAIL'))
    return 0 if ok else 2


def main(argv):
    args = [a for a in argv if not a.startswith('--')]
    root = os.path.abspath(args[0] if args else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
    allow = os.path.join(root, 'tools', 'neutral.allow')
    if '--selftest' in argv:
        return selftest()
    if '--emit' in argv:
        found, _, _ = scan(root)
        for (f, t) in sorted(found):
            print('%s  %s' % (f, t))   # no reason column: a row is not valid until its reason is written
        return 0
    if selftest() != 0:
        return 2
    return verdict(root, allow)


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
