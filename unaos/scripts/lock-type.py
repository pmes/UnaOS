#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# lock-type — GATE-LOCKTYPE (GATELOCKTYPE, LEDGER.md SR188; BP-cores-20, B414 LOCKREG, rmbp-ledger B823 LOCKREG2).
# Every kernel spin lock is `crate::sync::Mutex`: its construction site is its name, so the registry (sync.rs `reg`)
# can name a hold past 50 ms, a spin-wait and an inversion. A raw `spin::Mutex` is invisible to it. LOCKREG2 found 38
# raw statics in 15 files that had crept back after the B414 sweep because nothing stopped them; this gate stops them.
#
# SCOPE. Every `.rs` file under unaos/crates/kernel/src/ (arch/ included; sync.rs scanned like the rest).
#
# WHAT IS A FINDING (in CODE: comments and string literals stripped by tools/neutral-check.py's `lex`, imported, not
# copied). The spin crate's lock types, measured at 13ca547f against sync.rs's registry twins:
#   Mutex family   `spin::Mutex`, `spin::MutexGuard`, `spin::mutex::{Mutex,SpinMutex,TicketMutex,FairMutex}` —
#                  the twin is `crate::sync::Mutex` (sync.rs:21) and its guard (sync.rs:34);
#   RwLock family  `spin::RwLock*`, `spin::rwlock::…` — no twin, but a spin lock LOCKREG cannot see (BP-cores-20 says
#                  EVERY kernel spin lock): use `sync::Mutex`, or rule a sync.rs twin first;
#   imports        `use spin::{… Mutex …}` (and `spin::mutex::{…}` / `spin::rwlock::{…}`) naming a family member, and
#                  the glob `use spin::*`.
# NOT findings: `spin::Once` (a one-time init cell, no hold to name; no twin), `spin::Spin` / `spin::relax::Spin` (the
# relax parameter `crate::sync::MutexGuard` mirrors).
# ALLOW (below) is the registry's own inner lock, by exact path+line, each with its reason; SHRINK-ONLY: a finding with
# no row is RAW (exit 1); a row that matches nothing is STALE (exit 1), so an edit that moves one of those sync.rs lines
# re-rules it in the same commit.
#
# CONTROLS (exit 2, NO verdict): the scope enumerates >= 200 files and holds sync.rs; tools/neutral-check.py imports;
# the --selftest fixture goes red on a raw static, a brace import, an RwLock, a glob import and `spin::mutex::SpinMutex`,
# stays green on the token in a comment and a string, `spin::Once`, `crate::sync::MutexGuard<'_, T, spin::Spin>` and
# `use spin::relax::Spin`, green on an allowlisted line, red on a stale row. main() runs the selftest first. On the tree
# the six ALLOW rows matching are the proof the pattern fires.
#
#   unaos/scripts/lock-type.py [<unaos dir>]   check (exit 0 clean, 1 raw or stale, 2 no verdict)
#   unaos/scripts/lock-type.py --selftest      the fixture alone
import contextlib, importlib.util, io, os, re, sys, tempfile

SRC = os.path.join('crates', 'kernel', 'src')            # relative to the unaos dir
REG = 'unaos/crates/kernel/src/sync.rs'                   # the registry (repo-relative)
WHY = 'the registry\'s own inner lock (B414 LOCKREG): sync::Mutex is built on a raw spin::Mutex<()>'
ALLOW = {(REG, 22): WHY, (REG, 38): WHY, (REG, 49): WHY, (REG, 60): WHY, (REG, 67): WHY, (REG, 768): WHY}

FAM = r'(?:Mutex|MutexGuard|SpinMutex|TicketMutex|FairMutex|RwLock\w*)'
PATH = re.compile(r'\bspin\s*::\s*(?:(?:mutex|rwlock)\s*::\s*(?:spin\s*::\s*)?)?' + FAM + r'\b')
BRACE = re.compile(r'\buse\s+(?:::\s*)?spin\s*::\s*(?:(?:mutex|rwlock)\s*::\s*)?\{([^}]*)\}')
GLOB = re.compile(r'\buse\s+(?:::\s*)?spin\s*::\s*(?:(?:mutex|rwlock)\s*::\s*)?\*')
MEMBER = re.compile(r'(?<![\w:])' + FAM + r'\b')


def lexer(repo):
    p = os.path.join(repo, 'tools', 'neutral-check.py')
    spec = importlib.util.spec_from_file_location('neutral_check', p)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m.lex


def findings(text, lex):
    """Yield (line, token) for every raw lock-type use in the code of one file."""
    for ln, kind, body in lex(text):
        if kind != 'code':
            continue
        for m in PATH.finditer(body):
            yield ln + body.count('\n', 0, m.start()), re.sub(r'\s+', '', m.group(0))
        for m in BRACE.finditer(body):
            for t in MEMBER.findall(m.group(1)):
                yield ln + body.count('\n', 0, m.start()), 'use spin::{%s}' % t
        for m in GLOB.finditer(body):
            yield ln + body.count('\n', 0, m.start()), 'use spin::*'


def scan(unaos, lex):
    repo = os.path.dirname(os.path.abspath(unaos))
    src = os.path.join(unaos, SRC)
    found, n, reg = [], 0, False
    for d, dirs, files in os.walk(src):
        dirs.sort()
        for f in sorted(files):
            if not f.endswith('.rs'):
                continue
            p = os.path.join(d, f)
            rp = os.path.relpath(p, repo).replace(os.sep, '/')
            n += 1
            reg |= rp == REG
            try:
                text = open(p, encoding='utf-8', errors='replace').read()
            except OSError:
                continue
            for ln, tok in findings(text, lex):
                found.append((rp, ln, tok))
    return found, n, reg


def verdict(unaos, lex, allow, quiet=False, min_files=200):
    found, n, reg = scan(unaos, lex)
    if n < min_files:
        print('LOCKTYPE: NO VERDICT — the scope enumerated %d files (< %d) under %s' % (n, min_files, SRC))
        return 2
    if not reg:
        print('LOCKTYPE: NO VERDICT — %s is not in the scope (the registry the gate defends is missing)' % REG)
        return 2
    keys = {(f, ln) for f, ln, _ in found}
    raw = [x for x in found if (x[0], x[1]) not in allow]
    stale = sorted(k for k in allow if k not in keys)
    for f, ln, tok in raw:
        print('LOCKTYPE: RAW   %s:%d %s — use crate::sync::Mutex (BP-cores-20: LOCKREG names it by its site), or rule a sync.rs twin first' % (f, ln, tok))
    for f, ln in stale:
        print('LOCKTYPE: STALE %s:%d — the allow row (%s) matches no raw use: re-rule it at its new line or delete it (shrink-only)' % (f, ln, allow[(f, ln)]))
    allowed = len(found) - len(raw)
    if not quiet:
        print('LOCKTYPE: files=%d raw=%d allowed=%d stale=%d -> %s' % (n, len(raw), allowed, len(stale), 'FAIL' if (raw or stale) else 'PASS'))
    return 1 if (raw or stale) else 0


def selftest(lex):
    ok = True
    with tempfile.TemporaryDirectory() as t, contextlib.redirect_stdout(io.StringIO()):
        unaos = os.path.join(t, 'unaos')
        src = os.path.join(unaos, SRC)
        os.makedirs(src)
        clean = ('// a spin::Mutex in a comment is not a finding\n'
                 '/* nor spin::RwLock<u8> in a block */\n'
                 'static O: spin::Once<u8> = spin::Once::new();\n'
                 'fn g<T>(m: &crate::sync::Mutex<T>) -> crate::sync::MutexGuard<\'_, T, spin::Spin> { let _ = "spin::Mutex"; m.lock() }\n'
                 'use spin::relax::Spin as S;\n')
        open(os.path.join(src, 'sync.rs'), 'w').write('pub struct Mutex { raw: spin::Mutex<()> }\n')
        open(os.path.join(src, 'f0.rs'), 'w').write(clean)
        open(os.path.join(src, 'f1.rs'), 'w').write(clean)
        reg = {(REG, 1): 'fixture'}
        ok &= verdict(unaos, lex, reg, True, 3) == 0
        ok &= verdict(unaos, lex, {}, True, 3) == 1                       # the registry line itself, unallowed
        ok &= verdict(unaos, lex, {**reg, (REG, 9): 'gone'}, True, 3) == 1   # a stale row
        for plant in ('static X: spin::Mutex<u8> = spin::Mutex::new(0);\n', 'use spin::{Mutex, Once};\n',
                      'static R: spin::RwLock<u8> = spin::RwLock::new(0);\n', 'use spin::*;\n',
                      'static S: spin::mutex::SpinMutex<u8> = spin::mutex::SpinMutex::new(0);\n'):
            open(os.path.join(src, 'f1.rs'), 'w').write(clean + plant)
            ok &= verdict(unaos, lex, reg, True, 3) == 1
        open(os.path.join(src, 'f1.rs'), 'w').write(clean)
        ok &= verdict(unaos, lex, reg, True, 3) == 0
        os.remove(os.path.join(src, 'sync.rs'))
        ok &= verdict(unaos, lex, reg, True, 2) == 2                      # no registry: no verdict
        ok &= verdict(unaos, lex, reg, True, 99) == 2                     # scope too small: no verdict
    print('LOCKTYPE selftest: %s' % ('PASS' if ok else 'FAIL'))
    return 0 if ok else 2


def main(argv):
    args = [a for a in argv if not a.startswith('--')]
    unaos = os.path.abspath(args[0] if args else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
    try:
        lex = lexer(os.path.dirname(unaos))
    except Exception as e:
        print('LOCKTYPE: NO VERDICT — tools/neutral-check.py (the lexer) does not import: %s' % e)
        return 2
    try:
        st = selftest(lex)
    except Exception as e:
        print('LOCKTYPE: NO VERDICT — the selftest fixture raised: %r' % e)
        return 2
    if '--selftest' in argv:
        return st
    if st != 0:
        print('LOCKTYPE: NO VERDICT — the selftest fixture failed (the pattern is broken)')
        return 2
    return verdict(unaos, lex, ALLOW)


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
