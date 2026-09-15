#!/usr/bin/env bash
# fc2-check.sh — GATE-FC2: a module whose every CONSUMER is under one gate must not be declared
# wider than that gate.
#
# WHY THIS EXISTS (LEDGER S5, S3, S21). The knob/arch discipline in this tree is stated on CALL
# SITES: a site is `#[cfg]`-gated, the arm degrades to a shim, and knob-off is byte-identical. The
# DECLARATION is the half nobody checks. `pub mod foo;` with no cfg compiles `foo.rs` into every
# image — its `panic::Location` lines counted, its statics linked, its `fn`s type-checked — even
# when every path that can reach it is behind `target_arch = "x86_64"` or `feature = "wc"`. The
# result reads as live code on aarch64 and is unreachable there: the FC-2 shape. It costs image
# bytes on a board that can never call it, it breaks knob-off byte identity (LAWS §5 names the
# cfg'd-out `pub mod` DECLARATION as the one line that may move), and it hides the arch gap — S3's
# complaint is precisely that `flight_recorder` "reads as shared" while neither aarch64 board can
# leave an on-card record.
#
# WHAT IT ASSERTS. For every non-inline `mod`/`pub mod` declaration reachable from the two crate
# roots (`crates/kernel/src/lib.rs` for `unaos_kernel`, `crates/kernel/src/main.rs` for the binary):
# if the module is REFERENCED at least once, and EVERY reference site sits under some cfg predicate
# that the DECLARATION does not already carry, that is a FINDING. The declaration is wider than its
# consumers; the fix is to narrow the declaration to the union of its references' cfgs.
#
# THE CONTROL, AND WHY IT IS SYNTHETIC. A scan that matched nothing would report zero findings, and
# zero findings reads as a clean tree (STRUCTURAL_GATES.md §The control probe). Two controls run
# before any verdict:
#
#   1. FIXTURE (the positive control — it MUST fire). The analyser runs over an in-memory synthetic
#      crate carrying one deliberate FC-2 instance (`ctrl_fc2`: unconditional declaration, one
#      reference under `target_arch = "x86_64"`) and one deliberate non-instance (`ctrl_ok`:
#      declaration already gated, same reference shape). The fixture must produce EXACTLY the
#      finding `ctrl_fc2` and must NOT produce `ctrl_ok`. Either way round is a broken analyser and
#      exits 2 with NO verdict.
#   2. TREE PROBES (the parser controls). Three facts about THIS tree that the parser must
#      independently rediscover, each of which dies if one parsing stage breaks:
#        * `arch::x86_64` resolves to a declaration carrying `target_arch = "x86_64"` — that
#          declaration lives inside a `cfg_if::cfg_if!` arm, so this dies if the cfg_if handling
#          breaks, and with it every cfg inherited by the 40-odd files under `arch/x86_64/`.
#        * `flight_recorder` has at least one reference whose site cfg carries
#          `target_arch = "x86_64"` inherited from NOTHING ON ITS OWN LINE — `arch/x86_64/serial.rs`
#          gets it from the module tree. This dies if file-cfg inheritance breaks.
#        * `flight_recorder` has NO reference in `fs/fat.rs`, which mentions the name six times in
#          prose. This dies if comment stripping breaks — and comment stripping is the whole
#          difficulty here exactly as it is for GATE-KNOB: 21 of the 27 raw `flight_recorder` hits
#          in this tree are comments (`grep -rn flight_recorder crates/kernel/src | wc -l` = 27,
#          code sites = 6).
#      A control failure is a BROKEN GATE, not a clean tree, and is reported as such (exit 2).
#
# LIMITS OF THE APPROXIMATION, STATED. This is a line-and-path scan, not rustc's resolver.
#   * A cfg is reduced to its MUST-HOLD atom set: `all(A,B)` -> {A,B}, `any(A,B)` -> {A} INTERSECT
#     {B}, `not(A)` -> {}. So `any(all(x86,wc), all(aarch64,df))` (dock's gate) contributes NOTHING,
#     which is conservative: it can only suppress a finding, never invent one.
#   * A reference's cfg is the union of its file's inherited module cfg and the innermost enclosing
#     `#[cfg]`s on its LINE. A site that is reachable only through a cfg'd CALLER in another file is
#     seen as ungated. Conservative in the same direction: false negatives, never false positives.
#   * `else if` arms of a `cfg_if!` carry their own predicate only, not `not(previous)`.
#   * A module referenced ZERO times is NOT a finding — that is a different defect (a dead module)
#     and this gate does not rule on it; it is counted in the census so the number is visible.
#   * Findings are reported per ATOM, and only atoms the declaration does not already carry.
# Every one of these limits biases toward SILENCE, which is why the fixture control is mandatory:
# the gate's own failure mode is a quiet zero, and the fixture is what makes that zero readable.
#
# REGISTERED EXCEPTIONS live in `scripts/fc2.registry` (the k8-reach idiom), one module path per
# line with a reason. A registered finding is reported and does not fail the gate. The file carries
# the "must reach zero" note; it is not an allowlist to grow.
#
# EXIT: 0 clean (or only registered findings) · 1 an unregistered finding · 2 control failed / parse
# failure, NO VERDICT GIVEN.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="${1:-$HERE/../crates/kernel/src}"
REGISTRY="${FC2_REGISTRY:-$HERE/fc2.registry}"

if [ ! -d "$SRC" ]; then
    echo "fc2: NO VERDICT — source root not found: $SRC" >&2
    exit 2
fi

python3 - "$SRC" "$REGISTRY" <<'PY'
import os, re, sys

SRC = os.path.abspath(sys.argv[1])
REGISTRY = sys.argv[2]
CRATE_EXTERN = "unaos_kernel"

# ---------------------------------------------------------------- comment stripping
def strip_comments(text):
    """Replace comment bytes with spaces, preserving every newline and every column."""
    out = list(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == '"':
            i += 1
            while i < n:
                if text[i] == '\\':
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                i += 1
            continue
        if c == 'r' and i + 1 < n and text[i + 1] in '#"':
            j = i + 1
            hashes = 0
            while j < n and text[j] == '#':
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                close = '"' + '#' * hashes
                k = text.find(close, j + 1)
                i = n if k < 0 else k + len(close)
                continue
        if c == "'" and i + 2 < n:
            # char literal or lifetime; only skip the unambiguous char-literal forms
            if text[i + 1] == '\\':
                k = text.find("'", i + 2)
                if 0 <= k <= i + 6:
                    i = k + 1
                    continue
            elif i + 2 < n and text[i + 2] == "'":
                i += 3
                continue
        if c == '/' and i + 1 < n and text[i + 1] == '/':
            while i < n and text[i] != '\n':
                out[i] = ' '
                i += 1
            continue
        if c == '/' and i + 1 < n and text[i + 1] == '*':
            depth = 1
            out[i] = out[i + 1] = ' '
            i += 2
            while i < n and depth:
                if text[i] == '/' and i + 1 < n and text[i + 1] == '*':
                    depth += 1
                    out[i] = out[i + 1] = ' '
                    i += 2
                    continue
                if text[i] == '*' and i + 1 < n and text[i + 1] == '/':
                    depth -= 1
                    out[i] = out[i + 1] = ' '
                    i += 2
                    continue
                if text[i] != '\n':
                    out[i] = ' '
                i += 1
            continue
        i += 1
    return ''.join(out)

# ---------------------------------------------------------------- cfg expressions
ATOM_RE = re.compile(r'(target_arch|target_os|target_env|feature|target_pointer_width)\s*=\s*"([^"]*)"')

def _split_top(s):
    parts, depth, cur = [], 0, ''
    for ch in s:
        if ch == '(':
            depth += 1
        elif ch == ')':
            depth -= 1
        if ch == ',' and depth == 0:
            parts.append(cur)
            cur = ''
        else:
            cur += ch
    if cur.strip():
        parts.append(cur)
    return [p.strip() for p in parts]

def must_atoms(expr):
    """Atoms that MUST hold for `expr` to be true. not(..) contributes nothing (conservative)."""
    e = expr.strip()
    if not e:
        return frozenset()
    for head, comb in (("all", "and"), ("any", "or"), ("not", "no")):
        if e.startswith(head + "(") and e.endswith(")"):
            inner = e[len(head) + 1:-1]
            if comb == "no":
                return frozenset()
            subs = [must_atoms(p) for p in _split_top(inner)]
            if not subs:
                return frozenset()
            if comb == "and":
                return frozenset().union(*subs)
            r = subs[0]
            for s in subs[1:]:
                r = r & s
            return r
    m = ATOM_RE.fullmatch(e)
    if m:
        return frozenset(['%s = "%s"' % (m.group(1), m.group(2))])
    return frozenset()

# Cargo FEATURE IMPLICATION. `facet = ["quarry"]` means a cfg on `facet` is a cfg on `quarry` too.
# Without this closure the gate reds on every implied pair in the tree — `facet`/`quarry`,
# `gen7`/`intel-ivb`, `nvidia-kepler-ce`/`nvidia-kepler`, `piinstall_confirm`/`piinstall` — each of
# which is a declaration that ALREADY carries the atom its references are under, spelled through
# Cargo instead of through the cfg. Four of the first ten findings on this tree were exactly that.
FEATURE_IMPLIES = {}

def load_features(cargo):
    try:
        with open(cargo) as fh:
            text = fh.read()
    except OSError:
        return False
    m = re.search(r'^\[features\]\s*$(.*?)(?=^\[|\Z)', text, re.M | re.S)
    if not m:
        return False
    for line in m.group(1).split('\n'):
        line = line.split('#', 1)[0].strip()
        mm = re.match(r'^([A-Za-z0-9_-]+)\s*=\s*\[(.*)\]\s*$', line)
        if not mm:
            continue
        deps = []
        for d in re.findall(r'"([^"]*)"', mm.group(2)):
            if d.startswith('dep:') or '/' in d:
                continue
            deps.append(d)
        FEATURE_IMPLIES[mm.group(1)] = deps
    return bool(FEATURE_IMPLIES)

def close_features(atoms):
    out = set(atoms)
    work = [a for a in atoms if a.startswith('feature = ')]
    seen = set()
    while work:
        a = work.pop()
        if a in seen:
            continue
        seen.add(a)
        name = a[len('feature = "'):-1]
        for d in FEATURE_IMPLIES.get(name, ()):
            atom = 'feature = "%s"' % d
            if atom not in out:
                out.add(atom)
                work.append(atom)
    return frozenset(out)

def must_all(exprs):
    r = set()
    for e in exprs:
        r |= must_atoms(e)
    return close_features(r)

CFG_ATTR_RE = re.compile(r'#!?\[\s*cfg\s*\(')

def cfg_exprs_on(line):
    """Every `#[cfg(...)]` / `#![cfg(...)]` on the line, as balanced expression strings."""
    out = []
    for m in CFG_ATTR_RE.finditer(line):
        i = m.end() - 1
        depth, j = 0, i
        while j < len(line):
            if line[j] == '(':
                depth += 1
            elif line[j] == ')':
                depth -= 1
                if depth == 0:
                    break
            j += 1
        if depth == 0 and j < len(line):
            out.append((m.start(), j + 1, line[i + 1:j].strip()))
    return out

ATTR_ONLY_RE = re.compile(r'^\s*#!?\[.*\]\s*$')
CFGIF_OPEN_RE = re.compile(r'(?:\bcfg_if\s*::\s*)?\bcfg_if\s*!\s*\{')
CFGIF_ELSEIF_RE = re.compile(r'^\s*\}\s*else\s+if\s*#\[\s*cfg\s*\(')
CFGIF_ELSE_RE = re.compile(r'^\s*\}\s*else\s*\{')

def brace_delta(line):
    return line.count('{') - line.count('}')

def blank_strings(line):
    """Blank string-literal CONTENT so `{`/`}` inside a format string cannot move brace depth.
    (`serial_println!("{{")` is the case that unbalances a naive counter and makes every cfg
    below it stick, which is how a file's atoms silently accumulate.)"""
    out, i, n = [], 0, len(line)
    while i < n:
        c = line[i]
        if c == '"':
            out.append(' ')
            i += 1
            while i < n:
                if line[i] == '\\':
                    out.append('  ')
                    i += 2
                    continue
                if line[i] == '"':
                    out.append(' ')
                    i += 1
                    break
                out.append(' ')
                i += 1
            continue
        out.append(c)
        i += 1
    return ''.join(out)

ATTR_START_RE = re.compile(r'^\s*#!?\[')

def join_attrs(lines):
    """Collapse a multi-line attribute onto its FIRST line, blanking the continuations.

    `#[cfg(all(\n  feature = "facet",\n  any(...)\n))]` is the shape `video/mod.rs` uses for
    every compound gate. Scanned per physical line it parses as NOTHING — `cfg_exprs_on` needs
    balanced parens — so the module below it reads as `decl=none` and lands as a false FINDING.
    Line numbers are preserved: only the continuation lines are emptied."""
    out = list(lines)
    i = 0
    while i < len(out):
        ln = out[i]
        if ATTR_START_RE.match(ln) and ln.count('[') > ln.count(']'):
            j, buf = i, ln
            while j + 1 < len(out) and buf.count('[') > buf.count(']'):
                j += 1
                buf = buf.rstrip() + ' ' + out[j].strip()
            if buf.count('[') == buf.count(']'):
                out[i] = buf
                for k in range(i + 1, j + 1):
                    out[k] = ''
                i = j + 1
                continue
        i += 1
    return out

class SourceFile:
    """Comment-stripped lines plus the within-file cfg stack active at each line."""
    def __init__(self, path, text):
        self.path = path
        self.lines = join_attrs(strip_comments(text).split('\n'))
        self.brace_lines = [blank_strings(l) for l in self.lines]
        self.file_attrs = []          # #![cfg(..)] inner attributes
        self.line_cfgs = [frozenset()] * (len(self.lines) + 2)
        self._walk()

    def _walk(self):
        stack = []        # dicts: {kind: block|item, depth: int, cfgs: [expr]}
        pending = []
        depth = 0
        cfgif = []        # stack of {base_depth, arm_marker}
        for idx, raw in enumerate(self.lines):
            lineno = idx + 1
            line = raw
            stripped = line.strip()
            attrs = cfg_exprs_on(line)
            inner = [a for a in attrs if line[a[0]:a[0] + 3] == '#![']
            outer = [a for a in attrs if a not in inner]
            if inner:
                self.file_attrs.extend(e for _, _, e in inner)

            # cfg_if arm transitions, handled before the generic path
            if cfgif and (CFGIF_ELSEIF_RE.match(line) or CFGIF_ELSE_RE.match(line)):
                base = cfgif[-1]['base']
                # the PREVIOUS arm's block was pushed at exactly `base` by the generic path
                # (`if #[cfg(..)] {` is an attribute line that opens a block); pop it, or the
                # arms union instead of excluding each other and every file under arch/aarch64
                # inherits target_arch=x86_64 as well as aarch64.
                while stack and stack[-1]['kind'] == 'block' and stack[-1]['depth'] >= base:
                    stack.pop()
                armcfg = [e for _, _, e in outer]
                self.line_cfgs[lineno] = self._active(stack)
                stack.append({'kind': 'block', 'depth': base, 'cfgs': armcfg, 'arm': True})
                depth += brace_delta(self.brace_lines[idx])
                pending = []
                continue

            if ATTR_ONLY_RE.match(stripped) and stripped.startswith('#['):
                self.line_cfgs[lineno] = self._active(stack)
                pending.extend(e for _, _, e in outer)
                continue

            here = list(pending) + [e for _, _, e in outer]
            self.line_cfgs[lineno] = self._active(stack, extra=here)

            d = brace_delta(self.brace_lines[idx])
            if here:
                if d > 0:
                    stack.append({'kind': 'block', 'depth': depth, 'cfgs': here})
                elif not (stripped.endswith(';') or stripped.endswith(',') or stripped.endswith('}')):
                    stack.append({'kind': 'item', 'depth': depth, 'cfgs': here})
            pending = []

            if stripped and stack and stack[-1]['kind'] == 'item' and \
               (stripped.endswith(';') or stripped.endswith(',')) and not here:
                stack.pop()

            if CFGIF_OPEN_RE.search(line):
                cfgif.append({'base': depth + d})

            depth += d
            while stack and stack[-1]['kind'] == 'block' and depth <= stack[-1]['depth']:
                stack.pop()
            while cfgif and depth < cfgif[-1]['base']:
                cfgif.pop()

    @staticmethod
    def _active(stack, extra=None):
        out = []
        for e in stack:
            out.extend(e['cfgs'])
        if extra:
            out.extend(extra)
        return tuple(out)

# ---------------------------------------------------------------- module tree
# The FOLDED form is the one this gate PRESCRIBES — `#[cfg(...)] pub mod x;` on ONE line, so that
# no panic::Location below it moves (LAWS §5). A declaration scanner anchored at `pub mod`
# therefore stops seeing a module the moment someone applies the gate's own fix, and the gate
# goes quiet about that module forever. Measured: folding `splash` took the census from 162
# declarations to 161. Leading attributes are part of the declaration line and are matched here.
MOD_DECL_RE = re.compile(r'^\s*(?:#!?\[[^\]]*\]\s*)*(?:pub(?:\s*\([^)]*\))?\s+)?'
                         r'mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*([;{])')
PATH_ATTR_RE = re.compile(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]')

class Module:
    def __init__(self, name, path, decl_file, decl_line, decl_cfgs, mod_file, inline, moddir):
        self.name = name
        self.path = path              # tuple of segments
        self.decl_file = decl_file
        self.decl_line = decl_line
        self.decl_cfgs = decl_cfgs    # tuple of cfg exprs (file-inherited + own)
        self.own_cfgs = ()
        self.mod_file = mod_file
        self.inline = inline
        self.moddir = moddir
        self.refs = []                # (file, line, frozenset atoms)

files = {}     # abspath -> SourceFile
def sf(path):
    if path not in files:
        with open(path, 'r', encoding='utf-8', errors='replace') as fh:
            files[path] = SourceFile(path, fh.read())
    return files[path]

def moddir_of(path, is_root):
    d = os.path.dirname(path)
    base = os.path.basename(path)
    if is_root or base == 'mod.rs':
        return d
    return os.path.join(d, base[:-3])

modules = {}          # path tuple -> Module
file_cfgs = {}        # abspath -> tuple of cfg exprs inherited from the module tree
file_modpath = {}     # abspath -> tuple (module path of that file)
parse_errors = []

def walk_file(path, modpath, inherited, moddir):
    path = os.path.abspath(path)
    if path in file_cfgs and file_cfgs[path] != inherited:
        pass
    file_cfgs.setdefault(path, tuple(inherited))
    file_modpath.setdefault(path, tuple(modpath))
    try:
        f = sf(path)
    except OSError as exc:
        parse_errors.append("cannot read %s: %s" % (path, exc))
        return
    base_cfgs = tuple(inherited) + tuple(f.file_attrs)
    file_cfgs[path] = base_cfgs
    # inline module bodies shift the directory for their children
    inline_stack = []   # (brace_depth_at_open, dirname, modpath)
    depth = 0
    pending_path_attr = None
    for idx, line in enumerate(f.lines):
        lineno = idx + 1
        m = PATH_ATTR_RE.search(line)
        if m:
            pending_path_attr = m.group(1)
        d = brace_delta(f.brace_lines[idx])
        mm = MOD_DECL_RE.match(line)
        if mm:
            name = mm.group(1)
            kind = mm.group(2)
            curdir, curmp = (moddir, tuple(modpath))
            if inline_stack:
                curdir, curmp = inline_stack[-1][1], inline_stack[-1][2]
            mp = curmp + (name,)
            cfgs = base_cfgs + tuple(f.line_cfgs[lineno])
            if kind == '{':
                mod = Module(name, mp, path, lineno, cfgs, None, True,
                             os.path.join(curdir, name))
                mod.own_cfgs = tuple(f.line_cfgs[lineno])
                modules.setdefault(mp, mod)
                inline_stack.append((depth, os.path.join(curdir, name), mp))
                depth += d
                pending_path_attr = None
                continue
            # non-inline: resolve the file
            if pending_path_attr:
                # `#[path = "x.rs"]` on a non-inline module resolves beside the DECLARING FILE
                # (video/crystal.rs's `login` is video/login.rs, not video/crystal/login.rs);
                # inside an inline module body it resolves against that module's directory.
                cand = [os.path.join(os.path.dirname(path), pending_path_attr),
                        os.path.join(curdir, pending_path_attr)]
            else:
                cand = [os.path.join(curdir, name + '.rs'),
                        os.path.join(curdir, name, 'mod.rs')]
            target = next((c for c in cand if os.path.isfile(c)), None)
            mod = Module(name, mp, path, lineno, cfgs, target, False, None)
            mod.own_cfgs = tuple(f.line_cfgs[lineno])
            if mp in modules:
                pass
            else:
                modules[mp] = mod
            if target is None:
                parse_errors.append("unresolved module file for %s (%s:%d)" %
                                    ('::'.join(mp), os.path.relpath(path, SRC), lineno))
            else:
                mod.moddir = moddir_of(target, False)
                walk_file(target, mp, cfgs, moddir_of(target, False))
            pending_path_attr = None
            depth += d
            continue
        depth += d
        while inline_stack and depth <= inline_stack[-1][0]:
            inline_stack.pop()

# The feature graph is load-bearing (see FEATURE_IMPLIES): without it the gate reds on four
# declarations that already carry their consumers' atom through Cargo. If it will not parse there is
# no verdict to give.
CARGO = os.path.join(os.path.dirname(SRC), 'Cargo.toml')
FEATURES_OK = load_features(CARGO)

LIB = os.path.join(SRC, 'lib.rs')
MAIN = os.path.join(SRC, 'main.rs')
for root in (LIB, MAIN):
    if not os.path.isfile(root):
        parse_errors.append("crate root missing: %s" % root)
        continue
    walk_file(root, (), (), moddir_of(root, True))

# every .rs file under SRC that the tree never reached still needs a module path for ref scanning
for dirpath, _dirs, names in os.walk(SRC):
    for nm in names:
        if nm.endswith('.rs'):
            p = os.path.abspath(os.path.join(dirpath, nm))
            file_cfgs.setdefault(p, ())
            if p not in file_modpath:
                rel = os.path.relpath(p, SRC)[:-3].split(os.sep)
                if rel and rel[-1] == 'mod':
                    rel = rel[:-1]
                file_modpath[p] = tuple(rel)

# ---------------------------------------------------------------- reference scan
by_path = {mp: m for mp, m in modules.items()}
PATH_TOK_RE = re.compile(r'\b((?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)+[A-Za-z_][A-Za-z0-9_]*)')
USE_RE = re.compile(r'^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?use\s+(.*)$')

def expand_use(body):
    """Flatten `a::{b, c::{d, e}}` into ['a::b', 'a::c::d', 'a::c::e']."""
    body = body.strip().rstrip(';').strip()
    if not body:
        return []
    i = body.find('{')
    if i < 0:
        return [body]
    prefix = body[:i]
    depth, j = 0, i
    while j < len(body):
        if body[j] == '{':
            depth += 1
        elif body[j] == '}':
            depth -= 1
            if depth == 0:
                break
        j += 1
    inner = body[i + 1:j]
    tail = body[j + 1:]
    out = []
    for part in _split_top(inner):
        for e in expand_use(part):
            out.append(prefix + e + tail)
    return out

# GLOB RE-EXPORTS. `arch/mod.rs` does `pub use aarch64::*;` inside its `cfg_if!` arm, so the whole
# tree spells the Orin's drivers `arch::xusb_tegra`, never `arch::aarch64::xusb_tegra`. Without this
# rewrite those 17 call sites resolve to nothing, the module looks like it has 3 consumers instead of
# 20, and the gate reds on a module that is referenced from everywhere.
GLOB_ALIASES = []      # (from_modpath, to_modpath)

def collect_globs():
    for path in sorted(file_cfgs):
        try:
            f = sf(path)
        except OSError:
            continue
        modpath = file_modpath.get(path, ())
        for idx, line in enumerate(f.lines):
            m = re.match(r'^\s*(?:pub\s*(?:\([^)]*\)\s*)?)?use\s+([A-Za-z_][A-Za-z0-9_:\s]*)::\s*\*\s*;', line)
            if not m:
                continue
            toks = [t.strip() for t in m.group(1).split('::') if t.strip()]
            tgt = resolve(toks, modpath, {})
            if tgt and tgt != modpath:
                GLOB_ALIASES.append((tuple(modpath), tuple(tgt)))

def alias_candidates(ap):
    out = [tuple(ap)]
    for _ in range(2):
        grown = list(out)
        for cand in out:
            for src, tgt in GLOB_ALIASES:
                if len(cand) > len(src) and tuple(cand[:len(src)]) == src:
                    new = tuple(tgt) + tuple(cand[len(src):])
                    if new not in grown:
                        grown.append(new)
        if len(grown) == len(out):
            break
        out = grown
    return out

def resolve(tokens, modpath, imports):
    """Resolve a `::`-path's leading segments to an absolute module path, or None."""
    t = list(tokens)
    if not t:
        return None
    head = t[0]
    if head == 'crate' or head == CRATE_EXTERN:
        return tuple(t[1:])
    if head == 'self':
        return tuple(modpath) + tuple(t[1:])
    if head == 'super':
        up = 0
        while t and t[0] == 'super':
            up += 1
            t.pop(0)
        base = tuple(modpath)[:-up] if up <= len(modpath) else ()
        return base + tuple(t)
    if head in imports:
        return tuple(imports[head]) + tuple(t[1:])
    return tuple(modpath) + tuple(t)

def scan_refs():
    for path in sorted(file_cfgs):
        try:
            f = sf(path)
        except OSError:
            continue
        modpath = file_modpath.get(path, ())
        base = file_cfgs.get(path, ())
        imports = {}
        # pass 1: use statements (joined until the terminating `;`)
        buf, bufline = None, 0
        for idx, line in enumerate(f.lines):
            lineno = idx + 1
            if buf is None:
                m = USE_RE.match(line)
                if not m:
                    continue
                buf, bufline = m.group(1), lineno
            else:
                buf += ' ' + line.strip()
            if ';' not in buf:
                continue
            body = buf[:buf.index(';')]
            for flat in expand_use(body):
                toks = [s.strip() for s in flat.split('::')]
                toks = [s for s in toks if s]
                if not toks:
                    continue
                alias = None
                if len(toks) >= 2 and ' as ' in toks[-1]:
                    nm, alias = toks[-1].split(' as ', 1)
                    toks[-1] = nm.strip()
                    alias = alias.strip()
                ap = resolve(toks, modpath, imports)
                if ap:
                    yield_ref(path, bufline, base, f, ap)
                    leaf = alias or toks[-1]
                    if leaf not in ('*', ''):
                        imports[leaf] = ap
            buf = None
        # pass 2: every `::`-path occurrence
        for idx, line in enumerate(f.lines):
            lineno = idx + 1
            if USE_RE.match(line):
                continue
            if MOD_DECL_RE.match(line):
                continue
            for m in PATH_TOK_RE.finditer(line):
                toks = [s.strip() for s in m.group(1).split('::')]
                ap = resolve(toks, modpath, imports)
                if ap:
                    yield_ref(path, lineno, base, f, ap)

def yield_ref(path, lineno, base, f, abspath_tokens):
    """Credit a reference to every registered module that is a prefix of `abspath_tokens`."""
    filemp = file_modpath.get(path, ())
    seen_mp = set()
    for cand in alias_candidates(abspath_tokens):
        for n in range(1, len(cand) + 1):
            _credit(path, lineno, base, f, filemp, tuple(cand[:n]), seen_mp)

def _credit(path, lineno, base, f, filemp, mp, seen_mp):
    """One reference to module `mp`, counted at most once per path occurrence."""
    if mp in seen_mp:
        return
    mod = by_path.get(mp)
    if mod is None:
        return
    seen_mp.add(mp)
    # "outside the module itself": a file inside the module's own subtree does not count
    if tuple(filemp[:len(mp)]) == mp:
        return
    cfgs = tuple(base) + tuple(f.line_cfgs[lineno])
    mod.refs.append((path, lineno, must_all(cfgs)))

collect_globs()
scan_refs()

# ---------------------------------------------------------------- the FC-2 rule
def findings():
    out = []
    for mp, mod in sorted(modules.items()):
        if mod.inline:
            continue
        if not mod.refs:
            continue
        decl_must = must_all(mod.decl_cfgs)
        common = None
        for _p, _l, atoms in mod.refs:
            common = set(atoms) if common is None else (common & set(atoms))
            if not common:
                break
        extra = (common or set()) - set(decl_must)
        if extra:
            out.append((mod, sorted(extra), decl_must))
    return out

# ---------------------------------------------------------------- CONTROLS
FIXTURE = {
    'lib.rs': '\n'.join([
        'pub mod ctrl_fc2;',
        '#[cfg(target_arch = "x86_64")]',
        'pub mod ctrl_ok;',
        '#[allow(dead_code)] pub mod ctrl_fold;',
        'pub mod ctrl_user;',
    ]),
    'ctrl_fc2.rs': 'pub fn f() {}\n',
    'ctrl_ok.rs': 'pub fn f() {}\n',
    'ctrl_fold.rs': 'pub fn f() {}\n',
    'ctrl_user.rs': '\n'.join([
        '// prose mentioning ctrl_fc2::f and crate::ctrl_fc2::f must not count',
        '#[cfg(target_arch = "x86_64")]',
        'pub fn g() {',
        '    crate::ctrl_fc2::f();',
        '    crate::ctrl_ok::f();',
        '    crate::ctrl_fold::f();',
        '}',
    ]),
    'main.rs': 'fn main() {}\n',
}

def run_fixture():
    """The positive control: the analyser over a synthetic crate with one known FC-2 instance."""
    import tempfile, shutil
    global SRC, modules, by_path, files, file_cfgs, file_modpath, parse_errors
    saved = (SRC, modules, by_path, files, file_cfgs, file_modpath, parse_errors)
    saved_globs = list(GLOB_ALIASES)
    tmp = tempfile.mkdtemp(prefix='fc2ctrl.')
    try:
        for nm, body in FIXTURE.items():
            with open(os.path.join(tmp, nm), 'w') as fh:
                fh.write(body)
        SRC = tmp
        modules, by_path, files = {}, {}, {}
        file_cfgs, file_modpath, parse_errors = {}, {}, []
        walk_file(os.path.join(tmp, 'lib.rs'), (), (), tmp)
        walk_file(os.path.join(tmp, 'main.rs'), (), (), tmp)
        by_path = {mp: m for mp, m in modules.items()}
        del GLOB_ALIASES[:]
        collect_globs()
        scan_refs()
        got = {'::'.join(m.path) for m, _e, _d in findings()}
        return got, list(parse_errors)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        (SRC, modules, by_path, files, file_cfgs, file_modpath, parse_errors) = saved
        del GLOB_ALIASES[:]
        GLOB_ALIASES.extend(saved_globs)

fix_found, fix_errs = run_fixture()
if fix_errs:
    print("fc2: NO VERDICT — fixture did not parse: %s" % '; '.join(fix_errs), file=sys.stderr)
    sys.exit(2)
if fix_found != {'ctrl_fc2', 'ctrl_fold'}:
    print("fc2: NO VERDICT — CONTROL FAILED. The fixture carries TWO FC-2 instances —", file=sys.stderr)
    print("fc2:   `ctrl_fc2` (bare unconditional decl) and `ctrl_fold` (decl carrying a non-cfg", file=sys.stderr)
    print("fc2:   attribute FOLDED onto its line, the shape this gate itself prescribes) — each with", file=sys.stderr)
    print("fc2:   one x86-gated reference, plus one non-instance (`ctrl_ok`, already gated).", file=sys.stderr)
    print("fc2:   The analyser reported: %s" % (sorted(fix_found) or '[]'), file=sys.stderr)
    print("fc2:   A zero here would read as a clean tree. It is a broken gate.", file=sys.stderr)
    sys.exit(2)

# tree probes
probe_fail = []
if not FEATURES_OK:
    probe_fail.append("[features] in %s did not parse — the implication closure is off, and four "
                      "declarations in this tree carry their consumers' atom through Cargo "
                      "(`facet = [\"quarry\"]` and friends) rather than through the cfg" % CARGO)
elif 'quarry' not in FEATURE_IMPLIES.get('facet', []):
    probe_fail.append("`facet = [\"quarry\"]` was not read out of %s (feature-graph parse broken)"
                      % CARGO)
if not GLOB_ALIASES:
    probe_fail.append("no glob re-export found — `arch/mod.rs`'s `pub use aarch64::*;` is how the "
                      "whole tree spells `arch::xusb_tegra`, and without it 17 of that module's 20 "
                      "call sites resolve to nothing")
x86mod = by_path.get(('arch', 'x86_64'))
if x86mod is None:
    probe_fail.append("`arch::x86_64` was not resolved at all (module-tree walk broken)")
elif 'target_arch = "x86_64"' not in must_all(x86mod.decl_cfgs):
    probe_fail.append("`arch::x86_64` did not inherit target_arch=x86_64 from its `cfg_if!` arm "
                      "(cfg_if handling broken; ~40 files under arch/x86_64 lose their cfg)")
fr = by_path.get(('flight_recorder',))
if fr is None:
    probe_fail.append("`flight_recorder` was not resolved (module-tree walk broken)")
else:
    inherited = [r for r in fr.refs
                 if os.path.basename(os.path.dirname(r[0])) == 'x86_64'
                 and 'target_arch = "x86_64"' in r[2]]
    if not inherited:
        probe_fail.append("no `flight_recorder` reference under arch/x86_64/ carries an INHERITED "
                          "target_arch=x86_64 (file-cfg inheritance broken)")
    prose = [r for r in fr.refs if r[0].endswith(os.path.join('fs', 'fat.rs'))]
    if prose:
        probe_fail.append("`flight_recorder` counted %d reference(s) in fs/fat.rs, which mentions "
                          "it only in prose (comment stripping broken)" % len(prose))
if probe_fail:
    print("fc2: NO VERDICT — CONTROL FAILED (tree probes):", file=sys.stderr)
    for p in probe_fail:
        print("fc2:   * %s" % p, file=sys.stderr)
    sys.exit(2)

if parse_errors:
    print("fc2: NO VERDICT — parse failure:", file=sys.stderr)
    for e in parse_errors:
        print("fc2:   * %s" % e, file=sys.stderr)
    sys.exit(2)

# ---------------------------------------------------------------- registry + verdict
registered = {}
if os.path.isfile(REGISTRY):
    with open(REGISTRY) as fh:
        for raw in fh:
            s = raw.strip()
            if not s or s.startswith('#'):
                continue
            parts = s.split(None, 1)
            registered[parts[0]] = parts[1] if len(parts) > 1 else ''

found = findings()
rc = 0
n_reg = 0
for mod, extra, decl_must in found:
    key = '::'.join(mod.path)
    rel = os.path.relpath(mod.decl_file, SRC)
    decl = ' + '.join(sorted(decl_must)) if decl_must else 'none'
    verdict = 'FINDING'
    if key in registered:
        verdict = 'REGISTERED'
        n_reg += 1
    else:
        rc = 1
    print("fc2: %s:%d pub mod %s decl=%s refs=%d all-under=%s -> %s" %
          (rel, mod.decl_line, key, decl, len(mod.refs), ' + '.join(extra), verdict))
    if verdict == 'REGISTERED':
        print("fc2:     registered: %s" % registered[key])
    else:
        seen = []
        for p, l, _a in mod.refs[:6]:
            tag = "%s:%d" % (os.path.relpath(p, SRC), l)
            if tag not in seen:
                seen.append(tag)
        print("fc2:     sites: %s%s" % (', '.join(seen),
                                        ' ...' if len(mod.refs) > len(seen) else ''))

n_decl = sum(1 for m in modules.values() if not m.inline)
n_inline = sum(1 for m in modules.values() if m.inline)
n_refd = sum(1 for m in modules.values() if not m.inline and m.refs)
print("fc2: census declarations=%d (inline=%d) referenced=%d unreferenced=%d findings=%d "
      "registered=%d files=%d" %
      (n_decl, n_inline, n_refd, n_decl - n_refd, len(found) - n_reg, n_reg, len(file_cfgs)))

if rc:
    print("fc2: FAILED — a module is declared wider than every path that can reach it.",
          file=sys.stderr)
    print("fc2: Narrow the declaration to the union of its references' cfgs, ON ONE LINE "
          "(`#[cfg(...)] pub mod x;`)", file=sys.stderr)
    print("fc2: so no panic::Location below it moves (LAWS §5), or register it in "
          "scripts/fc2.registry with a reason.", file=sys.stderr)
sys.exit(rc)
PY
