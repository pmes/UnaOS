#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# firmware-census — GATE-FIRMWARE (GATENEUTRAL, LEDGER.md SR166; BP-shape-11, R52, R127; LAWS.md:292-299 "no binary
# firmware under `unaos/` (a `git ls-files` census in ledger-check.sh, owed)"). Vendor firmware is STAGED on media and
# loaded as data; the tree holds only facts ABOUT it: `unaos/firmware/<set>.pins`, one `<role|file> <64 lowercase hex>`
# row per file, written from the bench's measured digest, with the ledger row that ruled the set.
#
# THE CENSUS. Every file under unaos/firmware/ that git knows (`git ls-files --cached --others --exclude-standard`, so
# an uncommitted addition is counted before its commit; a tree that is not a git checkout — the GATEPATH scratch copy —
# is walked instead, and the summary says `census=walk`). Each file must be one of:
#   F1 a `<set>.pins` file: UTF-8 text with no NUL; every row that is not blank or `#` is `<name> <64 lowercase hex>`
#      (a malformed row fails); at least one such row (a pins file with no digest fails); its text cites at least one
#      ledger id (`B<n>`, `S<n>`, `SR<n>`, `SO<n>`, `SP<n>`) that resolves to a row of docs/dev/LEDGER.md or
#      docs/dev/OS/*-ledger.md (`| <id> |`) — a set nobody ruled fails.
#   F2 any other file: it fails if it holds a NUL byte (vendor bytes in the tree, R52), and otherwise needs its sha256
#      as a digest row of some .pins file AND its file name in a ledger row (an unpinned or unruled file fails).
#
# CONTROLS (exit 2, NO verdict): unaos/firmware/ exists and the census holds >= 1 file; at least one ledger file reads;
# --selftest's fixture goes red on a binary blob, an unpinned text file, a pins file with no digest, a malformed row
# and a pins file citing no resolving ledger id, and green on a clean set. main() runs the selftest first.
#
#   tools/firmware-census.py [<repo root>]   exit 0 clean · 1 findings · 2 control
#   tools/firmware-census.py --selftest
import contextlib, glob, hashlib, io, os, re, subprocess, sys, tempfile

FW = os.path.join('unaos', 'firmware')
PIN_ROW = re.compile(r'^(\S+)\s+([0-9a-f]{64})\s*$')
LEDGER_ID = re.compile(r'\b(SR|SO|SP|S|B)(\d+)\b')


def census(root):
    """Return (repo-relative file list, 'git'|'walk')."""
    top = None
    try:
        top = subprocess.run(['git', '-C', root, 'rev-parse', '--show-toplevel'], capture_output=True, text=True,
                             timeout=30).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        top = None
    if top and os.path.realpath(top) == os.path.realpath(root):
        out = subprocess.run(['git', '-C', root, 'ls-files', '-z', '--cached', '--others', '--exclude-standard', '--', FW],
                             capture_output=True, timeout=60).stdout.decode('utf-8', 'replace')
        files = sorted({f for f in out.split('\0') if f and os.path.isfile(os.path.join(root, f))})
        return files, 'git'
    files = []
    for d, dirs, fs in os.walk(os.path.join(root, FW)):
        dirs[:] = [x for x in dirs if not x.startswith('.')]
        files += [os.path.relpath(os.path.join(d, f), root).replace(os.sep, '/') for f in fs]
    return sorted(files), 'walk'


def ledger_text(root):
    paths = [os.path.join(root, 'docs', 'dev', 'LEDGER.md')] + sorted(glob.glob(os.path.join(root, 'docs', 'dev', 'OS', '*-ledger.md')))
    texts = []
    for p in paths:
        try:
            texts.append(open(p, encoding='utf-8', errors='replace').read())
        except OSError:
            pass
    return '\n'.join(texts), len(texts)


def check(root, quiet=False):
    files, how = census(root)
    if not os.path.isdir(os.path.join(root, FW)) or not files:
        print('FIRMWARE: CONTROL — %s missing or the census is empty (%s)' % (FW, how))
        return 2
    ltext, nled = ledger_text(root)
    if not nled:
        print('FIRMWARE: CONTROL — no ledger file read (docs/dev/LEDGER.md, docs/dev/OS/*-ledger.md)')
        return 2
    rows = set(m.group(1) for m in re.finditer(r'^\|\s*([A-Z]+\d+)\s*\|', ltext, re.M))
    errs, pins, digests, others = [], 0, set(), []
    for f in files:
        data = open(os.path.join(root, f), 'rb').read()
        if not f.endswith('.pins'):
            others.append((f, data))
            continue
        pins += 1
        if b'\0' in data:
            errs.append('%s: a .pins file holds a NUL byte (binary, not pins text)' % f)
            continue
        text = data.decode('utf-8', 'replace')
        n = 0
        for i, line in enumerate(text.split('\n'), 1):
            s = line.strip()
            if not s or s.startswith('#'):
                continue
            m = PIN_ROW.match(s)
            if not m:
                errs.append('%s:%d malformed pin row %r (want `<name> <64 lowercase hex>`)' % (f, i, s[:60]))
                continue
            n += 1
            digests.add(m.group(2))
        if not n:
            errs.append('%s: no pins digest (no `<name> <64 lowercase hex>` row)' % f)
        ids = sorted({a + b for a, b in LEDGER_ID.findall(text)})
        if not any(x in rows for x in ids):
            errs.append('%s: no ledger row — cites %s, none of which is a row of LEDGER.md or OS/*-ledger.md' % (f, ', '.join(ids) or 'no ledger id'))
    for f, data in others:
        if b'\0' in data:
            errs.append('%s: binary file under %s — vendor firmware is staged on media, never committed (R52)' % (f, FW))
            continue
        if hashlib.sha256(data).hexdigest() not in digests:
            errs.append('%s: no pins digest — its sha256 is no row of any %s/*.pins' % (f, FW))
        if os.path.basename(f) not in ltext:
            errs.append('%s: no ledger row names it' % f)
    for e in errs:
        print('FIRMWARE: ' + e)
    if not quiet:
        print('FIRMWARE: census=%s files=%d pins=%d digests=%d other=%d findings=%d -> %s' % (
            how, len(files), pins, len(digests), len(others), len(errs), 'FAIL' if errs else 'PASS'))
    return 1 if errs else 0


def selftest():
    ok = True
    with tempfile.TemporaryDirectory() as t, contextlib.redirect_stdout(io.StringIO()):
        os.makedirs(os.path.join(t, FW))
        os.makedirs(os.path.join(t, 'docs', 'dev', 'OS'))
        open(os.path.join(t, 'docs', 'dev', 'OS', 'x-ledger.md'), 'w').write('| B7 | a set | x |\n')
        ok &= check(t, True) == 2                                       # empty census: no verdict
        pin = os.path.join(t, FW, 'a.pins')
        open(pin, 'w').write('# FWPIN (B7) facts\nucode %s\n' % ('ab' * 32))
        ok &= check(t, True) == 0                                       # a clean set
        blob = os.path.join(t, FW, 'ucode.bin')
        open(blob, 'wb').write(b'\x7fELF\0\0vendor')
        ok &= check(t, True) == 1                                       # binary in the tree
        os.remove(blob)
        txt = os.path.join(t, FW, 'notes.txt')
        open(txt, 'w').write('unpinned\n')
        ok &= check(t, True) == 1                                       # an unpinned text file
        open(pin, 'a').write('notes %s\n' % hashlib.sha256(b'unpinned\n').hexdigest())
        ok &= check(t, True) == 1                                       # pinned, but no ledger row names it
        open(os.path.join(t, 'docs', 'dev', 'OS', 'x-ledger.md'), 'a').write('| B8 | notes.txt staged | x |\n')
        ok &= check(t, True) == 0                                       # pinned and ruled
        os.remove(txt)
        open(pin, 'a').write('ucode NOTAHEX\n')
        ok &= check(t, True) == 1                                       # a malformed row
        open(pin, 'w').write('# FWPIN (B7) facts, no rows\n')
        ok &= check(t, True) == 1                                       # no digest
        open(pin, 'w').write('# FWPIN (B999) facts\nucode %s\n' % ('ab' * 32))
        ok &= check(t, True) == 1                                       # a ledger id that resolves to no row
    print('FIRMWARE selftest: %s' % ('PASS' if ok else 'FAIL'))
    return 0 if ok else 2


def main(argv):
    args = [a for a in argv if not a.startswith('--')]
    root = os.path.abspath(args[0] if args else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
    if '--selftest' in argv:
        return selftest()
    if selftest() != 0:
        return 2
    return check(root)


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
