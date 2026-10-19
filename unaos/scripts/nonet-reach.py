#!/usr/bin/env python3
"""nonet-reach.py — GATE-NONETREACH (rmbp-ledger B697 NOTELEMETRY, BP-bar-10, BP-gates-14): nothing under video/ reaches
an outbound network act.

WHY (docs/dev/blueprint/bar.md BP-bar-10: "No telemetry path exists" — bar (a) "no http_core call reachable from
`video/**`"). GATE-NONET (nonet-check.py, B672) walks from the FURNITURE files and reds any call into a NET MODULE by
name. That leaves two gaps this gate closes, by REACHABILITY rather than by names:
  * its roots are seven furniture stems; a remote feed can enter through any other painter under video/ (Quarry, Facet,
    the wallpaper, the player …) — here every fn under video/** is a root;
  * its sinks are four module names; the outbound act can sit outside them (net_phy's NET6 surface, a driver's own
    connect, a future http_core / tls_core call) and a read-only accessor inside them is not one — here a sink is the
    ACT itself, found in the fn bodies of the whole kernel tree.
It extends GATE-NONET and shares its machinery: the comment/string stripper, the module tree, the `use`-alias and path
resolver are nonet-check.py's, imported, not copied (one walker, two questions).

WHAT IT CHECKS (exit 1 on any finding; per reached sink one line, the first (shortest) path found):
  ROOTS  every `fn` with a body in crates/kernel/src/video/** (the furniture painters are a subset).
  WALK   from every root, every path call (`crate::a::b(`, `super::`, `self::`, a `use` alias, a same-file `f(`, a child
         module, `Type::f(`) resolved to (file, fn) through the module tree, breadth-first, any depth, THROUGH every
         module (net modules included: reaching `smolnet::lease_ip` is no finding, reaching `smolnet::stack_connect` is).
         Method calls (`.f(`) and trait dispatch are not resolvable statically and are not walked (`walk=paths`).
  SINK   a reached fn whose stripped body holds an outbound act (SINKS below): a smoltcp socket opened
         (`sockets.add(`), an active open (`.connect(`), the stack's outbound ledger (`note_connect(` / `note_send(` /
         `connect_said(`: every connect and send the stack counts, BP-net-11), a DNS query built (`build_query(`), or the
         HTTP / TLS cores (`http_core::` / `tls_core::`). A sink fn found on a reached path is a finding unless it is on
         BOUNDARY (a user verb's own entry, with its reason). No BOUNDARY row exists today.
CONTROL PROBES (exit 2, NO verdict): nonet-check.py importable; the tree has ≥ 1 root and ≥ 1 sink of each of the
connect, socket-open and ledger kinds; a synthetic tree (video::dock -> helper -> stack fn holding `sockets.add(`, and
a `use … as` alias into a DNS builder) is judged exactly: two findings, and a read-only accessor in the same net module
is not one.
usage: nonet-reach.py <unaos dir>   (exit 0 clean; 1 findings; 2 control failed / nothing to walk)
Wire: `NONET-REACH: roots=<n> edges=<n> reach=<n> -> PASS|FAIL`
"""
import importlib.util, os, re, sys
from collections import deque

HERE = os.path.dirname(os.path.abspath(__file__))
KSRC = os.path.join("crates", "kernel", "src")
# The outbound acts, by kind. A fn body holding any is a sink.
SINKS = (
    ("socket-open", re.compile(r'\bsockets\s*\.\s*add\s*\(')),
    ("connect", re.compile(r'\.\s*connect\s*\(')),
    ("ledger", re.compile(r'\b(?:note_connect|note_send|connect_said)\s*\(')),
    ("dns", re.compile(r'\bbuild_query\s*\(')),
    ("http", re.compile(r'\b(?:http_core|tls_core)\s*::')),
)
MUST_HAVE = ("socket-open", "connect", "ledger")   # kinds the real tree must hold (the sinks moved = no verdict)
# (file, fn) a walk may reach without a finding: a user verb's own entry (the user asked). Each row needs its reason.
BOUNDARY = {}


def load_nonet():
    spec = importlib.util.spec_from_file_location("nonet_check", os.path.join(HERE, "nonet-check.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def sink_kind(tree, rel, name):
    """The first outbound-act kind in any body of `rel::name`, or None."""
    t = tree.text(rel)
    for (_n, b, e, _l) in tree.fns(rel).get(name, ()):
        body = t[b:e]
        for kind, rx in SINKS:
            if rx.search(body):
                return kind
    return None


def walk(nn, tree):
    """(findings, roots, edges, sink kinds seen in the tree)."""
    roots = sorted((rel, n) for rel in tree.files if rel.startswith("video/") for n in tree.fns(rel))
    seen, parent, q, edges, out = set(roots), {}, deque(roots), 0, []
    hit = set()
    while q:
        rel, name = q.popleft()
        kind = sink_kind(tree, rel, name)
        if kind and (rel, name) not in BOUNDARY:
            if (rel, name) not in hit:
                hit.add((rel, name))
                chain, cur = [], (rel, name)
                while cur is not None:
                    chain.append(cur[0][:-3] + "::" + cur[1])
                    cur = parent.get(cur)
                out.append("nonet-reach: %s [%s]" % (" -> ".join(reversed(chain)), kind))
            continue   # the act is found; what it calls further is the stack's own
        for (_n, b, e, _l) in tree.fns(rel).get(name, ()):
            body = tree.text(rel)[b:e]
            for m in nn.CALL.finditer(body):
                call = m.group(1)
                if re.split(r'\s*::\s*', call)[0] in nn.KEYWORDS and '::' not in call:
                    continue
                tgt = tree.resolve(rel, call)
                if tgt is None or tgt[1] not in tree.fns(tgt[0]):
                    continue
                edges += 1
                if tgt not in seen:
                    seen.add(tgt)
                    parent[tgt] = (rel, name)
                    q.append(tgt)
    return out, len(roots), edges


def census(tree):
    """Sink kinds present anywhere in the tree (the control: the acts the gate names still exist)."""
    kinds = set()
    for rel in tree.files:
        t = tree.text(rel)
        for kind, rx in SINKS:
            if rx.search(t):
                kinds.add(kind)
    return kinds


def control(nn):
    import tempfile, shutil
    t = tempfile.mkdtemp(prefix="nonetreachctl.", dir=os.environ.get("TMPDIR") or None)
    try:
        os.makedirs(os.path.join(t, "video"))
        files = {
            "lib.rs": "pub mod video; pub mod stack; pub mod dnsq; pub mod helper;\n",
            "video/mod.rs": "pub mod dock;\n",
            "video/dock.rs": "use crate::helper as h;\npub fn paint() { h::refresh(); tray(); }\nfn tray() { let _ = crate::stack::addr(); }\n",
            "helper.rs": "use crate::dnsq as d;\npub fn refresh() { deeper(); }\nfn deeper() { crate::stack::open(1); d::ask(\"x\"); }\n",
            "stack.rs": "pub fn open(o: usize) { let h = S.sockets.add(o); }\npub fn addr() -> u8 { 0 }\n",
            "dnsq.rs": "pub fn ask(h: &str) { let mut b = [0u8; 64]; build_query(&mut b, 1, h); }\nfn build_query(o: &mut [u8], t: u16, h: &str) {}\n",
        }
        for k, v in files.items():
            open(os.path.join(t, k), "w").write(v)
        f, roots, _ = walk(nn, nn.Tree(t))
        return roots >= 2 and len(f) == 2 and any("stack::open" in x for x in f) and any("dnsq::ask" in x for x in f) \
            and not any("addr" in x for x in f)
    finally:
        shutil.rmtree(t, ignore_errors=True)


def main():
    unaos = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")
    try:
        nn = load_nonet()
    except Exception as e:  # noqa: BLE001 — any import failure is no verdict
        print("NONET-REACH: nonet-check.py not importable (%s) -> NO VERDICT" % e); return 2
    if not control(nn):
        print("NONET-REACH: control FAILED — the walk did not judge the synthetic dock -> helper -> socket-open tree -> NO VERDICT"); return 2
    ksrc = os.path.join(unaos, KSRC)
    if not os.path.isdir(ksrc):
        print("NONET-REACH: no %s -> NO VERDICT" % ksrc); return 2
    tree = nn.Tree(ksrc)
    kinds = census(tree)
    missing = [k for k in MUST_HAVE if k not in kinds]
    if missing:
        print("NONET-REACH: sink kind(s) %s found nowhere in the tree (moved? the gate names them) -> NO VERDICT" % ",".join(missing)); return 2
    findings, roots, edges = walk(nn, tree)
    if roots == 0:
        print("NONET-REACH: no fn under video/ -> NO VERDICT"); return 2
    for f in findings:
        print(f)
    print("nonet-reach: walk=paths sinks=%s boundary=%d" % ("+".join(k for k, _ in SINKS), len(BOUNDARY)))
    print("NONET-REACH: roots=%d edges=%d reach=%d -> %s" % (roots, edges, len(findings), "FAIL" if findings else "PASS"))
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
