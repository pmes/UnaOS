#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# CHARTER: Kernel — kernel-by-ruling (the build's static gate on the ring-3 images it stages)
#
# LUMENCRASH M4 (rmbp-ledger B326): refuse an x86 ring-3 ELF that cannot run at its entry.
#
#   scripts/elf-entry-check.sh <x86 static ELF>...
#
# Boot 20 killed `lumen` at entry+0x3f with a #GP: `movq -0x3b(%rip), %r13` was a GOTPCREL load whose GOT
# the link script had discarded, so lld resolved the slot to address 0 — the program's own first code bytes —
# and the next store went through the non-canonical pointer they spell. The x86 user target now relaxes GOT
# loads (`relax-elf-relocations`, M2); this gate makes the class impossible to stage again. Refused:
#
#   (a) dangling   — a `movq X(%rip), %r64` / `leaq X(%rip), %r64` whose target lies outside every PT_LOAD (a
#                    discarded section's address); a fixed-model image may reach its whole classic window and
#                    the +0x4000..+0x149000 landmarks above it.
#   (b) got-at-0   — a `movq X(%rip), %r64` that loads a pointer out of the first 64 bytes of the image base or
#                    of e_entry (a GOT slot resolved to the program's own first bytes: the signature that flew;
#                    `leaq _start(%rip)` — the fixed model's base derivation — stays legal).
#   (c) entry-insn — in the first 64 bytes at e_entry, an instruction ring 3 cannot execute: privileged,
#                    port I/O, descriptor-table / control / debug register access, a segment-register load,
#                    a far transfer, `int n`, `ud0/1/2`, `int3`, `hlt`.
#
# One line per image: `:: ELFENTRY: <name> entry=0x… insns=N rip_refs=N -> PASS|FAIL ::`, each refusal on its
# own `:: ELFENTRY: refused <rule> at entry+0x… — <insn> ::` line before it. Exit 1 if any image fails.
set -u
TOOLBIN="$(rustc +nightly --print sysroot)/lib/rustlib/$(rustc +nightly -vV | awk '/host/{print $2}')/bin"
OBJDUMP="${TOOLBIN}/llvm-objdump"
[ -x "$OBJDUMP" ] || OBJDUMP="$(command -v llvm-objdump || true)"
[ -n "$OBJDUMP" ] || { echo ":: ELFENTRY: no llvm-objdump (rustup component add llvm-tools) -> FAIL ::"; exit 1; }
[ "$#" -ge 1 ] || { echo "usage: $0 <x86 ELF>..."; exit 2; }
rc=0
for f in "$@"; do
    [ -s "$f" ] || { echo ":: ELFENTRY: $(basename "$f") missing -> FAIL ::"; rc=1; continue; }
    "$OBJDUMP" -d --no-show-raw-insn "$f" > "${f}.entrycheck.dis" 2>/dev/null || { echo ":: ELFENTRY: $(basename "$f") does not disassemble -> FAIL ::"; rc=1; rm -f "${f}.entrycheck.dis"; continue; }
    python3 - "$f" "${f}.entrycheck.dis" <<'PY' || rc=1
import os, re, struct, sys
f, dis = sys.argv[1], sys.argv[2]
b = open(f, 'rb').read()
name = os.path.basename(f)
if b[:4] != b'\x7fELF' or struct.unpack_from('<H', b, 18)[0] != 62:
    print(":: ELFENTRY: %s is not an x86_64 ELF -> FAIL ::" % name); sys.exit(1)
entry, = struct.unpack_from('<Q', b, 24)
phoff, = struct.unpack_from('<Q', b, 32)
phent, phnum = struct.unpack_from('<HH', b, 54)
loads = []
for i in range(phnum):
    t, fl, off, va, pa, fs, ms, al = struct.unpack_from('<IIQQQQQQ', b, phoff + i * phent)
    if t == 1:
        loads.append((va, va + ms))
# A one-past-the-end target is legal (a stack top: `addr_of!(STACK) + size`). A FIXED-model image (linked at 0,
# placed in the 16 KiB classic window) may reach the whole window (its stack is the window's top pages) and the
# window's ABI landmarks RIP-relatively: the info page at +0x4000 and the FB hole's surfaces up to +0x149000
# (RING3WIN M1). The elf model reaches its
# landmarks by absolute address, so it gets no such allowance.
XWIN = 0x10000200000
lo0 = min(lo for lo, _ in loads) if loads else 0
abi = [] if lo0 >= XWIN else [(lo0, lo0 + 0x149000)]
inside = lambda a: any(lo <= a <= hi for lo, hi in loads) or any(lo <= a < hi for lo, hi in abi)
SEG = ('%ds', '%es', '%ss', '%fs', '%gs', '%cs')
BAD = re.compile(r'^(hlt|cli|sti|in[bwl]?|out[bwl]?|ins[bwl]?|outs[bwl]?|lgdt\w*|lidt\w*|lldt\w*|ltr\w*|lmsw\w*|clts|'
                 r'invd|wbinvd|invlpg\w*|wrmsr|rdmsr|swapgs|sysret\w*|sysexit\w*|iret\w*|lret\w*|ljmp\w*|lcall\w*|'
                 r'lss\w*|lds\w*|les\w*|lfs\w*|lgs\w*|ud0\w*|ud1\w*|ud2\w*|int3|int|into|monitor|mwait|xsetbv)$')
LINE = re.compile(r'^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$')
bad, n, rip = [], 0, 0
for ln in open(dis):
    m = LINE.match(ln)
    if not m:
        continue
    a, mn, ops = int(m.group(1), 16), m.group(2), m.group(3)
    n += 1
    # Rules (a)/(b) read only the two shapes a GOT reference compiles to — `movq sym@GOT(%rip), %r64` (load the
    # slot) and `leaq sym(%rip), %r64` — because a flat-model `.text` also holds `.rodata`, and the bytes of a
    # table disassemble as arbitrary RIP-relative junk that is never executed.
    if '(%rip)' in ops:
        rip += 1
        t = re.search(r'#\s*0x([0-9a-f]+)', ops)
        shape = re.match(r'^-?(0x[0-9a-f]+)?\(%rip\),\s*%r[a-z0-9]+\b', ops)
        if t and shape and mn in ('movq', 'leaq'):
            tgt = int(t.group(1), 16)
            if not inside(tgt):
                bad.append(('dangling', a, '%s %s' % (mn, ops)))
            elif mn == 'movq' and (lo0 <= tgt < lo0 + 64 or entry <= tgt < entry + 64):
                bad.append(('got-at-0', a, '%s %s' % (mn, ops)))
    if entry <= a < entry + 64:
        o = ops.split('#')[0].strip()
        dst = o.split(',')[-1].strip() if o else ''
        if BAD.match(mn) or '%cr' in o or '%db' in o or '%dr' in o \
                or (mn.startswith(('mov', 'pop')) and dst in SEG):
            bad.append(('entry-insn', a, '%s %s' % (mn, ops)))
for rule, a, s in bad:
    print(":: ELFENTRY: refused %s at entry%+#x — %s ::" % (rule, a - entry, ' '.join(s.split())))
print(":: ELFENTRY: %s entry=%#x insns=%d rip_refs=%d -> %s ::" % (name, entry, n, rip, 'FAIL' if bad else 'PASS'))
sys.exit(1 if bad else 0)
PY
    rm -f "${f}.entrycheck.dis"
done
exit "$rc"
