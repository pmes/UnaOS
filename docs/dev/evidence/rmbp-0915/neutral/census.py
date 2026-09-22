#!/usr/bin/env python3
"""NEUTRAL census — NEUTRAL-TABLE.md §0 method with a real Rust lexer for comment stripping.
Handles: line comments, NESTED block comments, string/char/raw-string literals."""
import os, re, sys
from collections import defaultdict

ROOT = sys.argv[1] if len(sys.argv) > 1 else "unaos/crates/kernel/src"

def strip_comments(t):
    out = []; i = 0; n = len(t)
    while i < n:
        c = t[i]
        # raw string r"..." / r#"..."#
        if c == 'r' and i+1 < n and (t[i+1] == '"' or t[i+1] == '#'):
            j = i+1; h = 0
            while j < n and t[j] == '#': h += 1; j += 1
            if j < n and t[j] == '"':
                j += 1; close = '"' + '#'*h
                k = t.find(close, j)
                if k == -1: k = n
                out.append(t[i:k+len(close)]); i = k+len(close); continue
        if c == '"':
            j = i+1
            while j < n:
                if t[j] == '\\': j += 2; continue
                if t[j] == '"': j += 1; break
                j += 1
            out.append(t[i:j]); i = j; continue
        if c == "'":
            # char literal or lifetime; only treat as literal if it closes within 4 chars
            m = re.match(r"'(?:\\.|[^\\'])'", t[i:])
            if m: out.append(m.group(0)); i += len(m.group(0)); continue
            out.append(c); i += 1; continue
        if c == '/' and i+1 < n and t[i+1] == '/':
            j = t.find('\n', i)
            if j == -1: j = n
            i = j; continue                      # drop to end of line, keep the \n
        if c == '/' and i+1 < n and t[i+1] == '*':
            depth = 1; j = i+2
            while j < n and depth:
                if t.startswith('/*', j): depth += 1; j += 2
                elif t.startswith('*/', j): depth -= 1; j += 2
                else:
                    if t[j] == '\n': out.append('\n')   # keep line numbering
                    j += 1
            i = j; continue
        out.append(c); i += 1
    return ''.join(out)

shared, archf = [], []
for dp, dn, fn in os.walk(ROOT):
    for f in fn:
        if not f.endswith('.rs'): continue
        rel = os.path.relpath(os.path.join(dp, f), ROOT)
        (archf if rel.startswith('arch/') and rel != 'arch/mod.rs' else shared).append(rel)
shared.sort(); archf.sort()

PATS = {
 'bracket': re.compile(r"\[(?:orin|tegra|jetson|pi|rmbp|mbp|x86)[a-z0-9 -]*?(?=[\]:0-9])"),
 'colon'  : re.compile(r"::\s*(?:tegra|TEGRA-[A-Z0-9]+|PIUSB|PI-[A-Z]+|PIINSTALL|INSTALL-PI)\s*:"),
 'ident'  : re.compile(r"\b(?:orin|tegra|jetson|pi|rmbp|mbp)_[a-z0-9_]+\b"),
 'glued'  : re.compile(r"\b(?:piusb|orinusb)[0-9]+_[a-z0-9_]+\b"),
 'upper'  : re.compile(r"\b[A-Z][A-Z_0-9]*(?:ORIN|TEGRA|JETSON|RMBP|MBP|PIUSB|PIINSTALL|PIRAST|PIDESK)[A-Z_0-9]*\b|\b(?:ORIN|TEGRA|JETSON|RMBP|MBP|PIUSB|PIINSTALL)[A-Z_0-9]*\b|\bPI_[A-Z0-9_]+\b"),
 'camel'  : re.compile(r"\b(?:Orin|Tegra|Jetson)[A-Za-z0-9]*\b"),
}
BR_FP = re.compile(r"^\[pi$")          # plc[pi] index variable
UP_FP = {'PIN','PIO','PIPE','PITCH','PING','PID','PICKS','PINNED','ACPI','SPINS','EXPIRED','OCCUPIED'}

res = {k: defaultdict(lambda: defaultdict(int)) for k in PATS}
lines_of = defaultdict(lambda: defaultdict(list))

def scan(files):
    for rel in files:
        raw = open(os.path.join(ROOT, rel), encoding='utf-8', errors='replace').read()
        txt = strip_comments(raw)
        pre = [0]
        for ch in txt:
            pass
        starts = {}
        off = 0
        lineno = 1
        linestart = {}
        for idx, ch in enumerate(txt):
            linestart[idx] = lineno
            if ch == '\n': lineno += 1
        for kind, pat in PATS.items():
            for m in pat.finditer(txt):
                tok = m.group(0).strip() if kind == 'colon' else m.group(0)
                if kind == 'bracket' and BR_FP.match(tok): continue
                if kind == 'bracket' and tok in ('[pin','[pio','[pipe'): continue
                if kind == 'upper' and tok in UP_FP: continue
                res[kind][tok][rel] += 1
                lines_of[kind + '|' + tok][rel].append(linestart.get(m.start(), 0))

scan(shared)
print("== FILE COUNTS (shared = not under arch/, arch/mod.rs shared) ==")
print(f"shared .rs: {len(shared)}   arch/ .rs: {len(archf)}   total: {len(shared)+len(archf)}")
for kind in PATS:
    d = res[kind]
    tot = sum(sum(v.values()) for v in d.values())
    print(f"\n== {kind.upper()}: {len(d)} distinct / {tot} sites ==")
    for tok in sorted(d, key=lambda t: (-sum(d[t].values()), t)):
        fl = ", ".join(f"{f}:{c}" for f, c in sorted(d[tok].items(), key=lambda kv: -kv[1]))
        print(f"  {tok:32s} {sum(d[tok].values()):4d}   {fl}")
