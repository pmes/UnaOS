#!/usr/bin/env python3
# arch-check.py — GATE-ARCH: the kernel against its own architecture rules (ARCHREVIEW, rmbp-ledger B441).
#
# WHY. GATE-CHARTER (B297) makes a file in the app-domain scope NAME an owner and a seam word; nothing
# reads the claim against the file, and nothing outside that scope is read at all. The 2026-10-06 review
# (docs/dev/review/ARCH-2026-10-06.md) found the holes this gate closes cheaply: a seam word with no
# ruling or audit row behind it, a `shared-core` claim on a file that links no core, a `<home>/.name`
# path in the `{}/.name` form the charter regex never sees (the Holocron ring, the BT link-key folder),
# a direct write into Principia's settings folder outside prefs.rs, a kernel file that keeps a store
# (a static holding user-visible state AND writes to a volume) with no CHARTER line, and a format magic
# or MIME table outside the shared cores (R79: a parser in the kernel is a shared core's or a duplicate).
#
# HOW. Six legs over unaos/crates/kernel/src/**/*.rs; each finding is a key. Today's keys are the
# BASELINE (scripts/arch.baseline): a key on the baseline passes, a NEW key fails, and a baseline key
# that no longer matches fails as STALE — the baseline only shrinks (the STATUS.baseline rule).
#   seamcite   kernel-by-ruling with no R<n>/B<n> on the CHARTER line; owed with no B<n>
#   sharedcore a `shared-core` CHARTER on a file that names no shared crate (`*_core::`, `unafs::`, `una_abi::`)
#   dotslash   a `"{}/.name…"` or `"/.name…"` literal
#   settings   a `/settings/` path literal outside prefs.rs (the store's one writer)
#   store      a file with a Mutex/RwLock static of Vec/BTreeMap/String AND a volume write, no CHARTER line,
#              no charter.registry row
#   parser     a MIME table or a format magic (PNG GIF ELF RIFF fLaC OggS ID3 ftyp) outside the shared cores
#
# CONTROLS (exit 2, NO verdict): the scope enumerates >= 200 files; the baseline parses; --selftest's
# fixture tree goes red on one planted finding per leg and green once each is baselined.
#
# usage: arch-check.py [--selftest] [--emit-baseline] [<unaos dir>]   exit 0 clean · 1 a new or stale key · 2 control failed
import os, re, sys, tempfile

SEAM_RE = re.compile(r'CHARTER:\s*([^—|-]+?)\s*(?:—|--|-|\|)\s*([a-z-]+)(.*)')
SHARED_RE = re.compile(r'\b[a-z0-9_]+_core::|\bunafs::|\buna_abi::|\b[a-z0-9_]+_core\b')
DOTSLASH_RE = re.compile(r'"(?:\{[A-Za-z_]*\})?/\.[A-Za-z][A-Za-z0-9_./-]*"')  # GATEREVIEW F2: `{home}/.x` too
SETTINGS_RE = re.compile(r'"\{\}/settings/[A-Za-z0-9_.-]*"')
STORE_STATIC_RE = re.compile(r'^\s*(?:pub(?:\([a-z]+\))?\s+)?static\s+[A-Z0-9_]+\s*:\s*(?:spin::)?(?:Mutex|RwLock)<[^;]*\b(?:Vec|BTreeMap|String)\b', re.M)
STORE_WRITE_RE = re.compile(r'\.(?:write|create|set_attr)\(\s*&?[A-Za-z_][A-Za-z0-9_.]*(?:\(\))?\s*,')  # GATEREVIEW F4: PATH consts, p.as_str()
PARSERS = [
    ('mime-table', re.compile(r'&\[\(&str,\s*&str\)\]\s*=\s*&\[[^\]]*"(?:image|audio|video)/', re.S)),
    ('png', re.compile(r'0x89,\s*b\'P\',\s*b\'N\',\s*b\'G\'|b"\\x89PNG')),
    ('gif', re.compile(r'b"GIF8')),
    ('elf', re.compile(r'0x7f,\s*b\'E\',\s*b\'L\',\s*b\'F\'|b"\\x7fELF')),
    ('riff', re.compile(r'b"RIFF"')),
    ('flac', re.compile(r'b"fLaC"')),
    ('ogg', re.compile(r'b"OggS"')),
    ('id3', re.compile(r'b"ID3"')),
    ('isobmff', re.compile(r'b"ftyp"')),
]


def scan(k):
    """All finding keys of the kernel tree at `k` (crates/kernel/src)."""
    keys, n = set(), 0
    reg = set()
    regf = os.path.join(k, '..', '..', '..', 'scripts', 'charter.registry')
    if os.path.isfile(regf):
        for line in open(regf, encoding='utf-8'):
            if '|' in line and not line.startswith('#'):
                reg.add(line.split('|')[0].strip())
    for root, _, files in os.walk(k):
        for f in files:
            if not f.endswith('.rs'):
                continue
            p = os.path.join(root, f)
            rel = os.path.relpath(p, k)
            n += 1
            try:
                text = open(p, encoding='utf-8', errors='replace').read()
            except OSError:
                continue
            head = '\n'.join(text.split('\n')[:40])
            m = None
            for line in head.split('\n'):
                if 'CHARTER:' in line:
                    m = SEAM_RE.search(line)
                    break
            if m:
                seam, rest = m.group(2), m.group(3)
                if seam == 'kernel-by-ruling' and not re.search(r'\b[RB]\d+', rest):
                    keys.add(f'seamcite|{rel}')
                if seam == 'owed' and not re.search(r'\bB\d+', rest):
                    keys.add(f'seamcite|{rel}')
                if seam == 'shared-core' and not SHARED_RE.search(re.sub(r'//[^\n]*', '', text)):  # GATEREVIEW F3: a core named in a comment links nothing
                    keys.add(f'sharedcore|{rel}')
            for lit in sorted(set(DOTSLASH_RE.findall(text))):
                keys.add(f'dotslash|{rel}|{lit.strip(chr(34))}')  # GATEREVIEW F1: keyed by FILE, so a baselined literal in a new file is new
            if rel != 'prefs.rs' and SETTINGS_RE.search(text):
                keys.add(f'settings|{rel}')
            if not m and rel not in reg and STORE_STATIC_RE.search(text) and STORE_WRITE_RE.search(text):
                keys.add(f'store|{rel}')
            for kind, rx in PARSERS:
                if rx.search(text):
                    keys.add(f'parser|{rel}|{kind}')
    return keys, n


def load_baseline(path):
    keys = set()
    if os.path.isfile(path):
        for line in open(path, encoding='utf-8'):
            line = line.rstrip('\n')
            if line and not line.startswith('#'):
                keys.add(line.split('  #')[0].strip())
    return keys


def verdict(k, base_path, quiet=False):
    keys, n = scan(k)
    if n < 200 and not quiet:
        print(f'arch-check: CONTROL FAILED — scope enumerated {n} files (< 200)')
        return 2
    base = load_baseline(base_path)
    new = sorted(keys - base)
    stale = sorted(base - keys)
    for x in new:
        if not quiet:
            print(f'  ❌ GATE-ARCH: new {x} — see docs/dev/STRUCTURAL_GATES.md §GATE-ARCH (fix it; a NEW key never gets a baseline row)')
    for x in stale:
        if not quiet:
            print(f'  ❌ GATE-ARCH: stale baseline row {x} — the finding is gone; delete the row (the baseline only shrinks)')
    if new or stale:
        return 1
    if not quiet:
        print(f'  ✅ arch (GATE-ARCH: {n} kernel files; {len(base)} baseline findings, none new)')
    return 0


def selftest():
    with tempfile.TemporaryDirectory() as d:
        k = os.path.join(d, 'crates', 'kernel', 'src')
        os.makedirs(os.path.join(d, 'scripts'))
        os.makedirs(k)
        open(os.path.join(d, 'scripts', 'charter.registry'), 'w').write('old.rs | Kernel | driver | x\n')
        plants = {
            'a.rs': '//! CHARTER: Kernel — kernel-by-ruling\n',
            'b.rs': '//! CHARTER: Kernel — shared-core\n// one day this links stria_core\nfn x() {}\n',
            'c.rs': 'fn p(h: &str) -> String { format!("{}/.secret", h) }\n',
            'd.rs': '//! CHARTER: Kernel — wm\nfn p(h: &str) -> String { format!("{}/settings/mine", h) }\n',
            'e.rs': 'static S: spin::Mutex<Vec<u8>> = spin::Mutex::new(Vec::new());\nfn w() { mt.write(p.as_str(), 0, b, k); }\n',
            'f.rs': '//! CHARTER: Kernel — driver\nfn s(b: &[u8]) -> bool { &b[..4] == b"OggS" }\n',
            'g.rs': '//! CHARTER: Kernel — owed B1\n//! CHARTER is fine\nfn x() { holo_core::y(); }\n',
            'i.rs': '//! CHARTER: Kernel — wm\nfn p(home: &str) -> String { format!("{home}/.recents") }\n',  # GATEREVIEW F2
            'h.rs': '//! CHARTER: Kernel — wm\nfn p(h: &str) -> String { format!("{}/.secret", h) }\n',  # GATEREVIEW F1: c.rs's literal reused
        }
        for f, t in plants.items():
            open(os.path.join(k, f), 'w').write(t)
        keys, _ = scan(k)
        want = {'seamcite|a.rs', 'sharedcore|b.rs', 'dotslash|c.rs|{}/.secret', 'dotslash|h.rs|{}/.secret', 'dotslash|i.rs|{home}/.recents', 'settings|d.rs', 'store|e.rs', 'parser|f.rs|ogg'}
        if keys != want:
            print(f'arch-check: SELFTEST FAILED — planted {sorted(want)}, scanned {sorted(keys)}')
            return 2
        base = os.path.join(d, 'base')
        open(base, 'w').write('\n'.join(sorted(want)) + '\n')
        keys2, _ = scan(k)
        if keys2 - load_baseline(base) or load_baseline(base) - keys2:
            print('arch-check: SELFTEST FAILED — a baselined tree is not clean')
            return 2
        open(base, 'a').write('store|gone.rs\n')
        if not (load_baseline(base) - keys2):
            print('arch-check: SELFTEST FAILED — a stale row was not seen')
            return 2
    print('  ✅ arch-check selftest: six planted findings caught, a baselined tree clean, a stale row seen')
    return 0


def main(argv):
    if '--selftest' in argv:
        return selftest()
    args = [a for a in argv if not a.startswith('--')]
    u = args[0] if args else os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
    k = os.path.join(u, 'crates', 'kernel', 'src')
    base = os.path.join(u, 'scripts', 'arch.baseline')
    if '--emit-baseline' in argv:
        keys, _ = scan(k)
        sys.stdout.write('\n'.join(sorted(keys)) + '\n')
        return 0
    rc = selftest()
    if rc:
        return rc
    return verdict(k, base)


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
