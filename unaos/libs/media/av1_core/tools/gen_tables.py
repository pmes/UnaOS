#!/usr/bin/env python3
"""Generate src/tables.rs from the AV1 specification sources.

Input: a checkout of https://github.com/AOMediaCodec/av1-spec (the Markdown
sources of "AV1 Bitstream & Decoding Process Specification", v1.0.0 with
Errata 1). Every constant lookup table the spec defines inside a ```c code
block (`Name[ dims ] = { ... }`) is transcribed mechanically -- scan orders,
conversion tables, the default CDFs, quantizer lookups and matrices, filter
taps -- together with the symbolic constants of section 3 and the enumerated
values named in section 6's semantic tables.

Nothing is typed by hand: a value count that does not match the declared
dimensions aborts the generator. Run:

    python3 tools/gen_tables.py <av1-spec checkout> > src/tables.rs
"""
import re, sys, os, subprocess

spec = sys.argv[1]
files = ["03.symbols.md", "06.bitstream.syntax.md", "07.bitstream.semantics.md",
         "08.decoding.process.md", "09.parsing.process.md", "10.additional.tables.md"]
src = {f: open(os.path.join(spec, f)).read() for f in files}

def unesc(s):
    return s.replace("\\<", "<").replace("\\>", ">").replace("\\|", "|").replace("\\-", "-")

# ---- symbols (section 3) ----
syms = {}
order = []
def addsym(name, val, where):
    if name in syms:
        if syms[name] != val:
            # same spelling, different enumeration (e.g. table columns) -> keep first
            return
        return
    syms[name] = val
    order.append((name, where))

for line in src["03.symbols.md"].splitlines():
    m = re.match(r"^\|\s*`([A-Z_0-9]+)`\s*\|\s*([^|]+?)\s*\|", line)
    if m:
        expr = unesc(m.group(2))
        try:
            v = eval(expr, {}, dict(syms))
        except Exception:
            continue
        addsym(m.group(1), int(v), "3")
# ---- enumerations (section 6 semantic tables: | value | NAME ) ----
for line in src["07.bitstream.semantics.md"].splitlines():
    m = re.match(r"^\|\s*(\d+)\s*\|\s*([A-Z][A-Z_0-9]+)\.?\s*\|?\s*(\|.*)?$", line)
    if m:
        addsym(m.group(2), int(m.group(1)), "6")
# lr_type table has 3 columns: | lr_type | FrameRestorationType | Name
for line in src["07.bitstream.semantics.md"].splitlines():
    m = re.match(r"^\|\s*\d+\s*\|\s*(\d+)\s*\|\s*(RESTORE_[A-Z]+)", line)
    if m:
        addsym(m.group(2), int(m.group(1)), "6")
    m = re.match(r"^\|\s*(?:Don't care|\d+)\s*\|\s*(\d+)\s*\|\s*(TX_SET_[A-Z_0-9]+)", line)
    if m:
        addsym(m.group(2), int(m.group(1)), "6")
addsym("NONE", -1, "6")  # RefFrame[1] = NONE

# ---- tables ----
fence = re.compile(r"^(\s*)~~~~+\s*c\s*$")
blocks = []
for f in files[1:]:
    lines = src[f].splitlines()
    i = 0
    while i < len(lines):
        m = fence.match(lines[i])
        if m:
            j = i + 1
            body = []
            while j < len(lines) and not re.match(r"^\s*~~~~+\s*$", lines[j]):
                body.append(lines[j]); j += 1
            blocks.append((f, i + 1, "\n".join(body)))
            i = j + 1
        else:
            i += 1

decl = re.compile(r"([A-Z][A-Za-z0-9_]*)\s*((?:\[[^\]]*\]\s*)+)=\s*\{")
tables = {}
torder = []
for (f, ln, body) in blocks:
    body = re.sub(r"/\*.*?\*/", " ", body, flags=re.S)
    body = re.sub(r"//[^\n]*", " ", body)
    pos = 0
    while True:
        m = decl.search(body, pos)
        if not m:
            break
        name = m.group(1)
        dims_s = re.findall(r"\[([^\]]*)\]", m.group(2))
        # balanced braces
        k = m.end() - 1
        depth = 0
        e = k
        while True:
            c = body[e]
            if c == "{": depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0: break
            e += 1
        inner = body[k:e + 1]
        pos = e + 1
        try:
            dims = [int(eval(unesc(d), {}, dict(syms))) for d in dims_s]
        except Exception as ex:
            sys.stderr.write("skip %s (%s:%d): dims %s\n" % (name, f, ln, ex)); continue
        flat = inner.replace("{", ",").replace("}", ",")
        vals = []
        bad = False
        for tok in flat.split(","):
            tok = tok.strip()
            if not tok:
                continue
            parts = [tok]
            # erratum guard: two identifiers separated only by whitespace (missing comma)
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*(\s+[A-Za-z_][A-Za-z0-9_]*)+", tok):
                parts = tok.split()
                sys.stderr.write("note %s: missing comma between %s\n" % (name, parts))
            for p in parts:
                try:
                    vals.append(int(eval(unesc(p), {}, dict(syms))))
                except Exception:
                    bad = True
        if bad:
            sys.stderr.write("skip %s (%s:%d): not a constant table\n" % (name, f, ln))
            continue
        n = 1
        for d in dims: n *= d
        if len(vals) != n:
            sys.stderr.write("FATAL %s (%s:%d): %d values for dims %s\n" % (name, f, ln, len(vals), dims))
            sys.exit(1)
        if name in tables:
            if tables[name][1] != vals:
                sys.stderr.write("FATAL duplicate differing %s\n" % name); sys.exit(1)
            continue
        tables[name] = (dims, vals, f, ln)
        torder.append(name)

def rust_name(n):
    r = n.upper()
    if r in syms:  # e.g. the Max_Tx_Depth table vs the MAX_TX_DEPTH constant
        r += "_TABLE"
    return r

def rtype(vals, name):
    if name.endswith("_Cdf"):
        return "u16"
    lo, hi = min(vals), max(vals)
    if lo >= 0 and hi < 256: return "u8"
    if lo >= -128 and hi < 128: return "i8"
    if lo >= 0 and hi < 65536: return "u16"
    if lo >= -32768 and hi < 32768: return "i16"
    return "i32"

def ty(t, dims):
    s = t
    for d in reversed(dims):
        s = "[%s; %d]" % (s, d)
    return s

def emit(vals, dims):
    if len(dims) == 1:
        return "[" + ", ".join(str(v) for v in vals) + "]"
    step = len(vals) // dims[0]
    return "[" + ",\n".join(emit(vals[i * step:(i + 1) * step], dims[1:]) for i in range(dims[0])) + "]"

try:
    sha = subprocess.check_output(["git", "-C", spec, "rev-parse", "HEAD"]).decode().strip()
except Exception:
    sha = "unknown"
out = []
out.append("// GENERATED FILE -- do not edit. Regenerate with tools/gen_tables.py.")
out.append("//")
out.append("// Source: AV1 Bitstream & Decoding Process Specification (v1.0.0 + Errata 1),")
out.append("// Markdown sources https://github.com/AOMediaCodec/av1-spec at %s." % sha)
out.append("// Every table below is mechanically transcribed from a `Name[dims] = {...}`")
out.append("// declaration in the spec; the comment above each names the file and line.")
out.append("// Constants come from section 3 (symbols) and the enumerations of section 6.")
out.append("// CDF tables keep the spec layout: N cumulative values ending in 32768, then")
out.append("// one trailing adaptation counter (initially 0).")
out.append("#![allow(dead_code, clippy::all)]")
out.append("")
for (n, where) in order:
    v = syms[n]
    t = "isize" if v < 0 else "usize"
    out.append("pub const %s: %s = %d;" % (n, t, v))
out.append("")
for name in torder:
    dims, vals, f, ln = tables[name]
    t = rtype(vals, name)
    out.append("// %s line %d" % (f, ln))
    out.append("pub static %s: %s = %s;" % (rust_name(name), ty(t, dims), emit(vals, dims)))
    out.append("")
print("\n".join(out))
sys.stderr.write("%d constants, %d tables\n" % (len(order), len(torder)))
