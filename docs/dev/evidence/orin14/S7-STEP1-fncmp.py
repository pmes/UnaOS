#!/usr/bin/env python3
"""Per-function comparison of two ELFs' .text, modulo relocation.
Normalises: the address column; adr/adrp/literal-ldr targets and the page-offset `add` that follows an
adrp (all layout-dependent); `<sym+off>` targets keep the symbol name (call identity) but the anon.*/
.llvm.<hash> suffixes are stripped; branch targets within the function become SELF+off (layout-invariant).
Prints the functions whose normalised instruction stream differs, plus counts."""
import re, subprocess, sys
from collections import OrderedDict

def load(elf):
    out = subprocess.run(["llvm-objdump", "-d", "--no-show-raw-insn", elf], capture_output=True, text=True).stdout
    funcs = OrderedDict()
    cur = None
    for line in out.splitlines():
        m = re.match(r"^([0-9a-f]{16}) <(.+)>:$", line)
        if m:
            cur = re.sub(r"\.llvm\.\d+", "", m.group(2))
            funcs[cur] = []
            pages = set()
            continue
        if cur is None:
            continue
        m = re.match(r"^\s*[0-9a-f]+:\s+(.*)$", line)
        if not m:
            continue
        ins = m.group(1)
        ins = re.sub(r"\s+", " ", ins).strip()
        # <sym+off> / <sym> targets: strip absolute address, anon/llvm suffixes; self-branches -> SELF
        def tgt(mm):
            s = mm.group(1)
            s = re.sub(r"\.llvm\.\d+", "", s)
            base = s.split("+")[0]
            if base == cur:
                return "<SELF" + ("+" + s.split("+")[1] if "+" in s else "") + ">"
            if base.startswith("anon."):
                return "<ANON>"
            return "<" + s + ">"
        ins = re.sub(r"0x[0-9a-f]+ <([^>]*)>", tgt, ins)
        mn = ins.split(" ")[0]
        ops = ins[len(mn):].strip()
        dst = ops.split(",")[0].strip() if ops else ""
        if mn in ("adrp", "adr"):
            ins = re.sub(r"<[^>]*>", "<REL>", ins)
            if mn == "adrp":
                pages.add(dst)
            funcs[cur].append(ins)
            continue
        # any use of an adrp'd page register with an immediate is a layout fact
        for r in list(pages):
            if re.search(r"\[" + r + r", #0x[0-9a-f]+\]", ins):
                ins = re.sub(r"\[" + r + r", #0x[0-9a-f]+\]", "[" + r + ", #PGOFF]", ins)
            if mn == "add" and re.match(r"x\d+, " + r + r", #0x[0-9a-f]+$", ops):
                ins = "add " + dst + ", " + r + ", #PGOFF"
        if dst in pages and not (mn == "add" and ops.startswith(dst + ", " + dst + ", #PGOFF")):
            pages.discard(dst)
        if mn == "ldr" and "<" in ins:  # literal load
            ins = re.sub(r"<[^>]*>", "<REL>", ins)
        funcs[cur].append(ins)
    return funcs

def relax(ins_list):
    """LLD relaxes `adrp x, <REL>; add x, x, #PGOFF` to `nop; adr x, <REL>` when the target is within
    +-1 MiB — a layout fact. Canonicalise both to one ADDR pseudo-instruction."""
    out = []
    i = 0
    while i < len(ins_list):
        cur = ins_list[i]; nxt = ins_list[i + 1] if i + 1 < len(ins_list) else ""
        if cur.startswith("adrp ") and nxt.startswith("add ") and nxt.endswith("#PGOFF"):
            out.append("ADDR " + cur.split(" ", 1)[1].split(",")[0]); i += 2; continue
        if cur == "nop" and nxt.startswith("adr "):
            out.append("ADDR " + nxt.split(" ", 1)[1].split(",")[0]); i += 2; continue
        out.append(cur); i += 1
    return out

a, b = load(sys.argv[1]), load(sys.argv[2])
for d in (a, b):
    for k in d:
        d[k] = relax(d[k])
only_a = [k for k in a if k not in b]
only_b = [k for k in b if k not in a]
same = diff = 0
diffs = []
for k in a:
    if k in b:
        if a[k] == b[k]:
            same += 1
        else:
            diff += 1
            diffs.append((k, len(a[k]), len(b[k])))
print(f"functions: before={len(a)} after={len(b)} common={same+diff} identical(mod-reloc)={same} differing={diff}")
print("only-before:", only_a)
print("only-after:", only_b)
for k, la, lb in diffs:
    print(f"DIFF {k}: {la} -> {lb} insns")
