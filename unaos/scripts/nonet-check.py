#!/usr/bin/env python3
"""nonet-check.py — GATE-NONET (rmbp-ledger B672, BP-bar-10, BP-gates-13): the desktop's furniture never reaches the network.

WHY (docs/dev/blueprint/bar.md BP-bar-10: "No telemetry path exists. Furniture draws only the user's objects; no remote
content feed reaches `video/`" — held by "a grep gate: no http_core call reachable from `video/**`", owed). A grep of the
furniture files alone is the gate that cannot see a helper: a dock that calls `quarry::refresh()` which calls
`smolnet::resolve()` would read clean. So this is a CALL WALK, not a grep.

WHAT IT CHECKS (exit 1 on any finding; one line each, `nonet: <furniture fn> -> … -> <net fn> (<file>:<line>)`):
  ROOTS  every `fn` in the furniture files under crates/kernel/src/video/ (FURNITURE below: the menubar, the dock,
         Crystal, the notifications, the strip, Settings, the login screen; a stem matches `<stem>.rs`, `<stem><x>.rs`
         and `<stem>/**.rs`).
  WALK   from each root, every path call in its body (`crate::a::b(`, `super::f(`, `self::f(`, a `use` alias, a bare
         `f(` defined in the same file, a child-module path) is resolved to (file, fn) through the module tree
         (`a/b.rs`, `a/b/mod.rs`) and walked, breadth-first, to any depth. Method calls (`.f(`) and trait dispatch are
         not resolvable statically and are not walked (said on the PASS line as `walk=paths`).
  NET    a call that resolves into a NET module (NET below: smolnet, net_fetch, net_dns, netring3) is a finding unless
         the fn is on ALLOW (a local, read-only accessor of the stack's own state — no frame leaves the machine — with
         its reason). A furniture FILE that names a NET module at all (a `use` or a path) is a finding too.
CONTROL PROBES (exit 2, NO verdict): the tree has every NET module and ≥ 1 furniture root; a synthetic tree (a dock fn
-> a helper in another module -> `crate::smolnet::stack_connect`, and a `use crate::net_dns as d;` alias call) is
caught by the same walk, and its allowed accessor is not.
usage: nonet-check.py <unaos dir>   (exit 0 clean; 1 findings; 2 control failed / nothing to walk)
"""
import os, re, sys
from collections import deque

KSRC = os.path.join("crates", "kernel", "src")
FURNITURE = ("menubar", "dock", "crystal", "notify", "strip", "settings", "login")
NET = ("smolnet", "net_fetch", "net_dns", "netring3")
# Local accessors of the stack's own state: they read an atomic / a lease the stack already holds; nothing is sent.
ALLOW = {
    ("smolnet", "lease_ip"): "the tray's address (video/status.rs): the lease the stack holds, read; no frame",
}
KEYWORDS = {"if", "while", "for", "match", "return", "loop", "fn", "let", "in", "as", "move", "unsafe", "else",
            "Some", "Ok", "Err", "None", "Box", "Vec", "String", "where", "impl", "pub", "mut", "ref", "break",
            "continue", "struct", "enum", "type", "const", "static", "use", "mod", "dyn", "async", "await"}


TOKEN = re.compile(r'//[^\n]*|/\*.*?\*/|(?<![\w])b?r(#*)".*?"\1|"(?:\\.|[^"\\])*"|\'(?:\\.[^\']*|[^\'\\\n])\'', re.S)


def strip(src):
    """Comments and string/char literals blanked; newlines kept (line numbers hold). One regex pass."""
    def blank(m):
        t = m.group(0)
        if t.startswith('//'):
            return ''
        if t.startswith('/*'):
            return '\n' * t.count('\n')
        if t.startswith("'"):
            return "' '"
        return '""' + '\n' * t.count('\n')
    return TOKEN.sub(blank, src)


FN_DEF = re.compile(r'\bfn\s+([A-Za-z_]\w*)')
CALL = re.compile(r'(?<![\w.!])((?:(?:crate|super|self|[A-Za-z_]\w*)\s*::\s*)*[A-Za-z_]\w*)\s*(?:::\s*<[^<>()]*>)?\s*\(')
USE = re.compile(r'\buse\s+([^;]+);')


def modpath(rel):
    """`video/menubar.rs` -> ('video','menubar'); `video/quarry/mod.rs` -> ('video','quarry'); lib.rs -> ()."""
    p = rel[:-3].split('/')
    if p[-1] == 'mod' or p == ['lib']:
        p = p[:-1]
    return tuple(p)


def fns_of(text):
    """[(name, body_start, body_end, line)] — a fn with a `{ … }` body (a `;`-terminated signature has none)."""
    out = []
    for m in FN_DEF.finditer(text):
        i, depth = m.end(), 0
        while i < len(text):
            ch = text[i]
            if ch in '([':
                depth += 1
            elif ch in ')]':
                depth -= 1
            elif ch == ';' and depth == 0:
                i = -1
                break
            elif ch == '{' and depth == 0:
                break
            i += 1
        if i < 0 or i >= len(text):
            continue
        b, d = i, 0
        while i < len(text):
            if text[i] == '{':
                d += 1
            elif text[i] == '}':
                d -= 1
                if d == 0:
                    break
            i += 1
        out.append((m.group(1), b, i, text.count('\n', 0, m.start()) + 1))
    return out


def uses_of(text, mp):
    """alias -> absolute path tuple, from every `use` in the file (groups, `self`, `as`)."""
    out = {}

    def expand(prefix, s):
        s = s.strip()
        if not s:
            return
        if '{' in s:
            k = s.index('{')
            head = [x for x in re.split(r'\s*::\s*', s[:k].strip()) if x]
            inner, d, cur, parts = s[k + 1:s.rindex('}')], 0, '', []
            for ch in inner:
                if ch == ',' and d == 0:
                    parts.append(cur); cur = ''
                    continue
                d += ch == '{'
                d -= ch == '}'
                cur += ch
            parts.append(cur)
            for p in parts:
                expand(prefix + head, p)
            return
        alias = None
        m = re.match(r'(.*?)\s+as\s+([A-Za-z_]\w*)$', s)
        if m:
            s, alias = m.group(1), m.group(2)
        segs = prefix + [x for x in re.split(r'\s*::\s*', s) if x]
        if segs and segs[-1] == 'self':
            segs = segs[:-1]
        if not segs or segs[-1] == '*':
            return
        out[alias or segs[-1]] = absolute(segs, mp)

    for m in USE.finditer(text):
        expand([], m.group(1))
    return {k: v for k, v in out.items() if v is not None}


def absolute(segs, mp):
    """A path's segments made crate-absolute (None for an extern crate path)."""
    segs = list(segs)
    if not segs:
        return None
    if segs[0] == 'crate':
        return tuple(segs[1:])
    if segs[0] in ('self', 'super'):
        base = list(mp)
        while segs and segs[0] in ('self', 'super'):
            if segs.pop(0) == 'super' and base:
                base.pop()
        return tuple(base + segs)
    return None


class Tree:
    """The module tree of every .rs file; a file is read and parsed only when the walk reaches it."""
    def __init__(self, ksrc):
        self.ksrc, self.files, self.mods, self._t, self._f, self._u = ksrc, {}, {}, {}, {}, {}
        for d, _, fs in os.walk(ksrc):
            for f in fs:
                rel = os.path.relpath(os.path.join(d, f), ksrc).replace(os.sep, '/')
                if f.endswith('.rs') and rel != 'main.rs':   # main.rs is the binary's root, not `crate::` of the lib
                    mp = modpath(rel)
                    self.files[rel], self.mods[mp] = mp, rel

    def _load(self, rel):
        if rel not in self._t:
            try:
                t = strip(open(os.path.join(self.ksrc, rel), encoding='utf-8', errors='replace').read())
            except OSError:
                t = ''
            fns = {}
            for fn in fns_of(t):
                fns.setdefault(fn[0], []).append(fn)
            self._t[rel], self._f[rel], self._u[rel] = t, fns, uses_of(t, self.files[rel])

    def text(self, rel):
        self._load(rel)
        return self._t[rel]

    def fns(self, rel):
        self._load(rel)
        return self._f[rel]

    def uses(self, rel):
        self._load(rel)
        return self._u[rel]

    def file_of(self, segs):
        """Longest module prefix of `segs` that is a file: (file, rest)."""
        for k in range(len(segs), -1, -1):
            if tuple(segs[:k]) in self.mods:
                return self.mods[tuple(segs[:k])], list(segs[k:])
        return None, None

    def resolve(self, rel, call):
        """A call path in `rel` -> (file, fn) or None."""
        segs = [x for x in re.split(r'\s*::\s*', call) if x]
        mp = self.files[rel]
        if len(segs) == 1:
            if segs[0] in self.fns(rel):
                return rel, segs[0]
            u = self.uses(rel).get(segs[0])
            if u:
                f, rest = self.file_of(list(u))
                if f and len(rest) == 1:
                    return f, rest[0]
            return None
        a = absolute(segs, mp)
        if a is None:
            u = self.uses(rel).get(segs[0])
            if u:
                a = tuple(u) + tuple(segs[1:])
            elif tuple(mp) + (segs[0],) in self.mods:
                a = tuple(mp) + tuple(segs)   # a child module named from its parent
            elif (segs[0],) in self.mods:
                a = tuple(segs)               # a crate-root module named bare (2015-style / the root file)
            else:
                return None
        f, rest = self.file_of(list(a))
        if f is None or not rest:
            return None
        return f, rest[-1]   # `Type::method` / an inline `mod`: the last segment, looked up in that file


def net_mod(rel):
    stem = rel[:-3].split('/')[0]
    return stem if stem in NET and rel.count('/') == 0 else None


def furniture(rel):
    if not rel.startswith('video/'):
        return False
    tail = rel[len('video/'):]
    head = tail.split('/')[0]
    return any(head == s + '.rs' or (head.startswith(s) and head.endswith('.rs')) or head == s for s in FURNITURE)


def walk(tree):
    """(findings, roots, edges walked)."""
    out, seen, q, edges = [], set(), deque(), 0
    roots = [(rel, n) for rel in tree.files if furniture(rel) for n in tree.fns(rel)]
    for rel in sorted(tree.files):
        if not furniture(rel):
            continue
        for i, l in enumerate(tree.text(rel).split('\n'), 1):
            for n in NET:
                if re.search(r'(?<![\w])(?:crate\s*::\s*)?%s\s*::' % n, l) or re.search(r'\buse\s+crate\s*::\s*%s\b' % n, l):
                    out.append("nonet: %s:%d names the net module `%s` (furniture file)" % (rel, i, n))
    parent = {}
    for r in roots:
        seen.add(r)
        q.append(r)
    while q:
        rel, name = q.popleft()
        for (_n, b, e, _line) in tree.fns(rel).get(name, ()):
            body = tree.text(rel)[b:e]
            for m in CALL.finditer(body):
                call = m.group(1)
                first = re.split(r'\s*::\s*', call)[0]
                if first in KEYWORDS and '::' not in call:
                    continue
                tgt = tree.resolve(rel, call)
                if tgt is None:
                    continue
                edges += 1
                nm = net_mod(tgt[0])
                if nm:
                    if (nm, tgt[1]) in ALLOW:
                        continue
                    line = tree.text(rel).count('\n', 0, b + m.start()) + 1
                    chain, cur = [tgt[0][:-3] + "::" + tgt[1]], (rel, name)
                    while cur is not None:
                        chain.append(cur[0] + ":" + cur[1])
                        cur = parent.get(cur)
                    out.append("nonet: %s (%s:%d)" % (" -> ".join(reversed(chain)), rel, line))
                    continue
                if tgt not in seen:
                    seen.add(tgt)
                    parent[tgt] = (rel, name)
                    q.append(tgt)
    return sorted(set(out)), len(roots), edges


def control():
    """A synthetic tree the walk must judge exactly: two findings, the allowed accessor clean."""
    import tempfile, shutil
    base = os.environ.get("TMPDIR") or None
    t = tempfile.mkdtemp(prefix="nonetctl.", dir=base)
    try:
        os.makedirs(os.path.join(t, "video"))
        files = {
            "lib.rs": "pub mod video; pub mod smolnet; pub mod net_dns; pub mod helper;\n",
            "video/mod.rs": "pub mod dock;\n",
            "video/dock.rs": "use crate::helper as h;\npub fn paint() { h::refresh(); tray(); }\nfn tray() { let _ = crate::helper::addr(); }\n",
            "helper.rs": "use crate::net_dns as d;\npub fn refresh() { deeper(); }\nfn deeper() { crate::smolnet::stack_connect(0, [0; 4], 80); d::build_query(&mut [], 0, \"x\"); }\npub fn addr() -> u8 { crate::smolnet::lease_ip(); 0 }\n",
            "smolnet.rs": "pub fn stack_connect(a: usize, b: [u8; 4], c: u16) {}\npub fn lease_ip() {}\n",
            "net_dns.rs": "pub fn build_query(o: &mut [u8], t: u16, h: &str) {}\n",
        }
        for k, v in files.items():
            open(os.path.join(t, k), "w").write(v)
        f, roots, _ = walk(Tree(t))
        want = ("smolnet::stack_connect", "net_dns::build_query")
        return roots >= 2 and len(f) == 2 and all(any(w in x for x in f) for w in want) and not any("lease_ip" in x for x in f)
    finally:
        shutil.rmtree(t, ignore_errors=True)


def main():
    unaos = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")
    ksrc = os.path.join(unaos, KSRC)
    if not control():
        print("NONET: control FAILED — the walk did not catch the synthetic dock -> helper -> smolnet chain -> NO VERDICT"); return 2
    if not os.path.isdir(ksrc):
        print("NONET: no %s -> NO VERDICT" % ksrc); return 2
    tree = Tree(ksrc)
    missing = [n for n in NET if n + ".rs" not in tree.files]
    if missing:
        print("NONET: net module(s) %s not in the tree (moved? the gate names them) -> NO VERDICT" % ",".join(missing)); return 2
    findings, roots, edges = walk(tree)
    if roots == 0:
        print("NONET: no furniture fn under video/ (%s) -> NO VERDICT" % ",".join(FURNITURE)); return 2
    for f in findings:
        print(f)
    print("NONET: furniture_fns=%d edges=%d walk=paths net=%s allow=%d findings=%d -> %s"
          % (roots, edges, "+".join(NET), len(ALLOW), len(findings), "FAIL" if findings else "PASS"))
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
