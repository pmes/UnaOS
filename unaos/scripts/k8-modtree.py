#!/usr/bin/env python3
"""Per-FILE `target_arch` context imposed by the kernel's module tree.

Answers "does the Pi image ever LEX this file", which no path or line grep can answer.
`drivers/gpu/mod.rs` is declared `#[cfg(all(target_arch = "x86_64", ...))] pub mod gpu;`
in `drivers/mod.rs`, so every `feature = "nvidia-kepler"` cfg inside it is x86-only
although neither the line nor the path says so. A cfg'd-out `pub mod` is never lexed
(pi 7, 2026-09-05).

Three things this parser must get right, each of which it got wrong on the first cut and
each of which is silent when wrong:

  * SEVERAL `mod` decls folded onto ONE line, each with its own `#[cfg]` — this repo's
    byte-identity idiom (LEDGER P7). `arch/aarch64/mod.rs:110` carries three, and a
    line-anchored regex sees only the first, leaving two files unclassified.
  * a non-`mod.rs` parent's children live in `<stem>/`, not in the parent's own
    directory: `video/quarry.rs` declares `pub mod live;` = `video/quarry/live.rs`.
  * a `//` before a `mod` disowns it (LEDGER P7 / GATE-APPEND), so comments go first.

Every file the walk cannot account for is PRINTED (`UNREACHED`/`UNRESOLVED`/`INLINE`),
never defaulted to a context. A silent default is how a check stops being able to fire.

usage: k8-modtree.py <kernel-src-dir>
output: "<relpath> <both|x86_64|aarch64|none>", then the UNREACHED/UNRESOLVED/INLINE rows.
"""
import os, re, sys

ARCHS = frozenset(("x86_64", "aarch64"))
ROOTS = ("lib.rs", "main.rs")

def strip_comments(text):
    text = re.sub(r'/\*.*?\*/', ' ', text, flags=re.S)
    return "\n".join(l.split("//", 1)[0] for l in text.split("\n"))

def cfg_archs(expr):
    """The archs a cfg attribute admits, or None when it constrains no arch."""
    pos = set(re.findall(r'target_arch\s*=\s*"([a-z0-9_]+)"', expr))
    if not pos:
        return None
    neg = set(re.findall(r'not\s*\(\s*target_arch\s*=\s*"([a-z0-9_]+)"\s*\)', expr))
    if neg:
        return ARCHS - neg
    return pos & ARCHS

MOD_RE = re.compile(r'(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z0-9_]+)\s*$')
INLINE_RE = re.compile(r'(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z0-9_]+)\s*\{')

def decls(path):
    """(name, archs|None) per `mod X;` in this file, plus the inline `mod X { }` names.

    Statements are split on `;` so that N folded decls on one line are N statements, each
    carrying only the attributes that precede it.
    """
    text = strip_comments(open(path, encoding="utf8", errors="replace").read())
    out, inline = [], []
    for chunk in text.split(";"):
        m = MOD_RE.search(chunk)
        if m:
            a = None
            for attr in re.findall(r'#\[[^\]]*\]', chunk):
                c = cfg_archs(attr)
                if c is not None:
                    a = c if a is None else (a & c)
            out.append((m.group(1), a))
        inline.extend(im.group(1) for im in INLINE_RE.finditer(chunk))
    return out, inline

def child_dir(path):
    d, base = os.path.dirname(path), os.path.basename(path)
    return d if base in ("mod.rs",) + ROOTS else os.path.join(d, base[:-3])

def resolve(base, name):
    for cand in (os.path.join(base, name + ".rs"), os.path.join(base, name, "mod.rs")):
        if os.path.isfile(cand):
            return cand
    return None

def walk(src):
    """{relpath: set(archs)}, plus the unresolved decls and inline modules."""
    ctx, inline_mods, unresolved = {}, [], []
    work = [(os.path.join(src, r), set(ARCHS)) for r in ROOTS
            if os.path.isfile(os.path.join(src, r))]
    while work:
        path, archs = work.pop()
        rel = os.path.relpath(path, src)
        prev = ctx.get(rel)
        merged = archs if prev is None else (prev | archs)
        if prev is not None and merged == prev:
            continue
        ctx[rel] = merged
        kids, inl = decls(path)
        inline_mods.extend((rel, n) for n in inl)
        for name, a in kids:
            child = resolve(child_dir(path), name)
            if child is None:
                unresolved.append((rel, name))
                continue
            work.append((child, merged if a is None else (merged & a)))
    return ctx, unresolved, inline_mods

def tag(archs):
    return "both" if archs == ARCHS else (",".join(sorted(archs)) if archs else "none")

if __name__ == "__main__":
    src = sys.argv[1]
    ctx, unresolved, inline_mods = walk(src)
    for rel in sorted(ctx):
        print("%s %s" % (rel, tag(ctx[rel])))
    for dirpath, _, files in os.walk(src):
        for f in files:
            if f.endswith(".rs"):
                rel = os.path.relpath(os.path.join(dirpath, f), src)
                if rel not in ctx:
                    print("UNREACHED %s" % rel)
    for rel, n in unresolved:
        print("UNRESOLVED %s declares mod %s (no file)" % (rel, n))
    for rel, n in inline_mods:
        print("INLINE %s mod %s" % (rel, n))
