#!/usr/bin/env python3
"""panicloc-normalize.py — hash a kernel image WITHOUT its panic line numbers.

WHY THIS EXISTS.  This tree proves a change safe by BYTE-IDENTITY of the built
image: arm no knobs, rebuild, and show the loadable image is byte-for-byte the
baseline (`arroyo:790`, `:823`, `:866` — measured on `llvm-objcopy -O binary
kernel.elf`, never on `kernel.elf` itself).  That proof is why knob-gated
refactors land here without a boot.

But Rust bakes `file:line:col` into the image for every panic site, as DATA.
`core::panic::Location` is laid out

    { file_ptr: u64, file_len: u64, line: u32, col: u32 }        (24 B, align 8)

so INSERTING ONE LINE anywhere above a panic site moves that site's `line` u32
and the image is no longer byte-identical.  The orin 18 audit measured this
exactly: one blank line at `main.rs:3` moved EXACTLY ONE BYTE of a 1,303,832-byte
`kernel8.img`, `0xcd -> 0xce` at offset 1104296 — the low byte of the `line`
field of the Location for `main.rs:5069`.

The tree has been paying for that coupling in its SOURCE: statements folded onto
one line, comments appended to line ends instead of inserted, whole idioms
("zero net lines, `wc -l` both sides") invented to keep a u32 still.  That is the
test deforming the code it measures.

THE FIX IS THE COMPARISON, NOT THE SOURCE.  Peter's ruling (2026-09-07): KEEP THE
LINE NUMBERS.  Stripping them at build time (`-Z location-detail`) was considered
and REJECTED — the panic location is the operator's only tool on a metal panic,
and buying our proof by making them search a file for a message text is a trade
the wrong way round.  The panic locations are theirs; the byte comparison is
ours; ours is the one that bends.

So this tool zeroes the `line` field of every `panic::Location` record IN THE
BUILT IMAGE and hashes what is left.  Two images that differ only in where their
source lines sit normalize to the same hash.  Two images that differ in one
instruction do not.

HOW A RECORD IS IDENTIFIED — and why this does not over-match.  At every
8-byte-aligned offset, read the 24-byte window as the layout above and accept it
only when ALL of:

    1 <= file_len <= MAX_PATH                     plausible path length
    file_ptr .. file_ptr+file_len maps INTO this image
    those bytes are all printable ASCII
    that string ENDS IN `.rs`                     <-- the load-bearing filter
    line <= MAX_LINE  and  col <= MAX_COL         (0 allowed: idempotent re-runs)

The acceptance funnel, measured at orin 20 on both arches:

                                         aarch64 kernel8.img    x86_64 kernel ELF
    8-aligned 24-byte windows                    159,274              180,959
    ...1 <= file_len <= 512                        5,311               10,445
    ...pointer resolves into the image             4,445                9,585
    ...pointed-to bytes all printable                828                  903
    ...string ends in `.rs`         ACCEPTED          638                  730
    rejected by the `.rs` test ALONE                  99                  169

Every accepted record on both arches resolves to a source file that exists in the
tree with `line` inside that file's real line count — 638/638 and 730/730, zero
out of range.  Run `--srcroot` to re-take that measurement at any time.

The 99 and 169 rejected by the `.rs` test alone are exactly what the suffix
filter is for: they are real `{&str, u32, u32}` data — the shell's command table
reads `('clear', 0, 0)`, `('ls', 0, 0)`, `('panic', 0, 0)` — and zeroing "their"
third word would corrupt the image.  Dropping the `.rs` test is how this tool
would start lying.  (The numeric line/col bounds, by contrast, reject nothing the
`.rs` test has not already rejected on either arch: they are belt and braces.)

THE x86 CASE — why a naive scan finds ZERO records there.  `x86_64-unaos.json`
sets `position-independent-executables`, `relocation-model: pie` and
`relro-level: full`, so the x86 kernel is an ET_DYN PIE.  A PIE does not store
absolute pointers in the file: the `file_ptr` word of every Location reads
LITERALLY ZERO and the address rides the addend of an `R_X86_64_RELATIVE` entry
in `.rela.dyn` (7,499 of them, all type 8).  Measured:

    00 00 00 00 00 00 00 00  1e 00 00 00 00 00 00 00  4d 00 00 00  3e 00 00 00
    file_ptr = 0             file_len = 30            line = 77    col = 62

    ...with .rela.dyn carrying r_offset -> that word, addend 0x3ce4d, which is
    `crates/kernel/src/allocator.rs` (30 bytes) in .rodata => allocator.rs:77:62.

So this tool reads the relocation table and prefers a relocation's addend over
the file word wherever one applies — which is what the loader will do anyway.
The aarch64 kernel is a plain ET_EXEC with no such relocations, and the map is
simply empty there.  Without this join, x86 is invisible; with it, x86 is if
anything the CLEANER arch, because the relocation table states outright which
words are pointers instead of leaving it to be inferred.

USAGE

    panicloc-normalize.py IMAGE                      hash it (normalized)
    panicloc-normalize.py IMAGE --list               census every record found
    panicloc-normalize.py IMAGE --out NORM.img       write the normalized image
    panicloc-normalize.py A --compare B              is B a line-shift of A?
    panicloc-normalize.py IMAGE --srcroot DIR ...    self-check lines vs sources

`IMAGE` is a FLAT loadable image (`kernel8.img`) or an ELF.  For a flat image the
vaddr->offset map is `offset = vaddr - BASE`; BASE defaults to 0x80000, which is
`crates/kernel/pi-baremetal.ld`'s `. = DEFINED(KERNEL_LOAD) ? KERNEL_LOAD :
0x80000`.  For an ELF the map comes from the section headers, and both the scan
and the hash cover every SHF_ALLOC section that has bytes in the file — never
`.symtab`/`.strtab`/`.comment`, which are not allocated and which churn
`.llvm.<hash>` internal-linkage suffixes on unrelated rebuilds (orin 18 §3b
measured 1,859 of 8,751 symbols differing that way with every allocated section
identical).  That exclusion is not academic: at orin 20 a pure comment line
changed the x86 kernel ELF's FILE SIZE by 1,616 bytes while not moving a single
loadable byte outside a Location `line` field.  A raw sha of the ELF is not a
usable gate on either arch; the loadable view is.

EXIT STATUS.  0 normally.  With `--compare`, 0 when the two images are
EQUIVALENT after normalization and 1 when they genuinely differ — so it can be
used directly as a gate.
"""

import argparse
import hashlib
import os
import struct
import sys
from collections import Counter

LOC_SIZE = 24
LOC_ALIGN = 8
LINE_OFF = 16
COL_OFF = 20

MAX_PATH = 512
MAX_LINE = 1000000
MAX_COL = 10000

PI_FLAT_BASE = 0x80000

ELF_MAGIC = b"\x7fELF"
SHT_RELA = 4
SHT_NOBITS = 8
SHF_ALLOC = 0x2

EM_X86_64 = 62
EM_AARCH64 = 183

# The "relative" relocation each machine uses for a link-time-resolved absolute
# pointer inside a PIE.  Its ADDEND carries the address; the word in the file is 0.
R_RELATIVE = {EM_X86_64: 8, EM_AARCH64: 1027}


class Image:
    """A loadable image plus the vaddr -> file-offset map needed to chase pointers."""

    def __init__(self, path, base=PI_FLAT_BASE):
        self.path = path
        self.data = bytearray(open(path, "rb").read())
        self.is_elf = self.data[:4] == ELF_MAGIC
        # segments: list of (vaddr, size, file_offset, name)
        # relocs: vaddr of a relocated word -> the address its addend supplies
        self.relocs = {}
        if self.is_elf:
            self.segments = self._elf_sections()
        else:
            self.segments = [(base, len(self.data), 0, "<flat>")]

    def _elf_sections(self):
        d = self.data
        if d[4] != 2:
            sys.exit("panicloc-normalize: only ELF64 is supported")
        if d[5] != 1:
            sys.exit("panicloc-normalize: only little-endian ELF is supported")
        e_machine, = struct.unpack_from("<H", d, 0x12)
        e_shoff, = struct.unpack_from("<Q", d, 0x28)
        e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHH", d, 0x3A)
        if e_shoff == 0 or e_shnum == 0:
            sys.exit("panicloc-normalize: ELF has no section headers")
        strtab_off, = struct.unpack_from("<Q", d, e_shoff + e_shstrndx * e_shentsize + 0x18)

        def name_at(n):
            end = d.index(b"\0", strtab_off + n)
            return d[strtab_off + n:end].decode("utf-8", "replace")

        out = []
        relas = []
        for i in range(e_shnum):
            base = e_shoff + i * e_shentsize
            sh_name, sh_type, sh_flags = struct.unpack_from("<IIQ", d, base)
            sh_addr, sh_offset, sh_size = struct.unpack_from("<QQQ", d, base + 0x10)
            if sh_size == 0:
                continue
            if sh_type == SHT_RELA:
                relas.append((sh_offset, sh_size))
            # Everything the loader actually maps AND that has bytes in the file.
            # Excludes .bss/.relro_padding (NOBITS, no file bytes) and
            # .symtab/.strtab/.comment/.shstrtab (not SHF_ALLOC) — the latter churn
            # `.llvm.<hash>` suffixes on unrelated rebuilds and would poison the hash.
            if (sh_flags & SHF_ALLOC) and sh_type != SHT_NOBITS:
                out.append((sh_addr, sh_size, sh_offset, name_at(sh_name)))
        if not out:
            sys.exit("panicloc-normalize: ELF has no allocated sections with file content")
        out.sort()

        # A PIE resolves its absolute pointers with RELATIVE relocations: the word in the
        # file image is ZERO and the real address rides the relocation's addend.  The x86_64
        # kernel is exactly this (`x86_64-unaos.json`: position-independent-executables,
        # relocation-model pie, relro-level full), which is why a Location's `file_ptr`
        # reads 0 there and why chasing it needs this table.  The aarch64 kernel is a plain
        # ET_EXEC and has none of these, so the map is simply empty.
        want = R_RELATIVE.get(e_machine)
        if want is not None:
            for rela_off, rela_size in relas:
                for k in range(rela_size // 24):
                    o = rela_off + k * 24
                    r_offset, r_info, r_addend = struct.unpack_from("<QQq", d, o)
                    if (r_info & 0xFFFFFFFF) == want and r_addend > 0:
                        self.relocs[r_offset] = r_addend
        return out

    def to_offset(self, vaddr, length):
        """Map a vaddr span to a file offset, or None if it does not land in a segment."""
        for seg_addr, seg_size, seg_off, _ in self.segments:
            if seg_addr <= vaddr and vaddr + length <= seg_addr + seg_size:
                return seg_off + (vaddr - seg_addr)
        return None

    def scannable(self):
        """Byte ranges worth scanning, as (file_offset, size, vaddr_of_first_byte)."""
        return [(off, size, addr) for addr, size, off, _ in self.segments]

    def layout(self):
        """The identity of the loadable byte view: (name, addr, size) per segment.

        Two images with the same layout can be compared byte for byte even when
        their FILES differ in size — an ELF's `.symtab`/`.strtab` churn
        `.llvm.<hash>` suffixes on unrelated rebuilds and moves the file's total
        length without moving one loadable byte.
        """
        if not self.is_elf:
            return [("<flat>", self.segments[0][0], len(self.data))]
        return [(name, addr, size) for addr, size, _, name in self.segments]

    def view(self):
        """The loadable bytes, plus the file offset each one came from."""
        buf = bytearray()
        offs = []
        for _, size, off, _ in self.segments:
            buf += self.data[off:off + size]
            offs.extend(range(off, off + size))
        return buf, offs

    def digest(self):
        """The hash this tool gates on.

        A flat image hashes whole.  An ELF hashes its allocated PROGBITS sections
        in address order, each bound to its name and address so a section cannot
        silently move.
        """
        h = hashlib.sha256()
        if not self.is_elf:
            h.update(self.data)
            return h.hexdigest()
        for seg_addr, seg_size, seg_off, name in self.segments:
            h.update(("%s@%#x/%d\n" % (name, seg_addr, seg_size)).encode())
            h.update(self.data[seg_off:seg_off + seg_size])
        return h.hexdigest()


def find_records(img):
    """Every panic::Location record in the image, as dicts.

    `off` is the FILE offset of the record; `line_off`/`col_off` are the file
    offsets of the two u32s this tool is allowed to zero.
    """
    d = img.data
    recs = []
    seen = set()
    for scan_off, scan_size, scan_addr in img.scannable():
        start = scan_off + (-scan_off % LOC_ALIGN)
        for off in range(start, scan_off + scan_size - LOC_SIZE + 1, LOC_ALIGN):
            if off in seen:
                continue
            ptr, flen = struct.unpack_from("<QQ", d, off)
            if not (1 <= flen <= MAX_PATH):
                continue
            # In a PIE the pointer word is 0 and the address rides the relocation
            # that applies to it.  Prefer the relocation whenever one exists: it is
            # what the loader will actually write there.
            if img.relocs:
                ptr = img.relocs.get(scan_addr + (off - scan_off), ptr)
            line, col = struct.unpack_from("<II", d, off + LINE_OFF)
            if line > MAX_LINE or col > MAX_COL:
                continue
            fo = img.to_offset(ptr, flen)
            if fo is None:
                continue
            s = d[fo:fo + flen]
            if not all(0x20 <= b <= 0x7E for b in s):
                continue
            text = s.decode("ascii")
            if not text.endswith(".rs"):
                continue
            seen.add(off)
            recs.append({
                "off": off,
                "file": text,
                "line": line,
                "col": col,
                "line_off": off + LINE_OFF,
                "col_off": off + COL_OFF,
            })
    return recs


def normalized_offsets(recs, fields):
    """The exact set of file offsets this tool zeroes."""
    out = set()
    for r in recs:
        if "line" in fields:
            out.update(range(r["line_off"], r["line_off"] + 4))
        if "column" in fields:
            out.update(range(r["col_off"], r["col_off"] + 4))
    return out


def normalize(img, recs, fields):
    for r in recs:
        if "line" in fields:
            struct.pack_into("<I", img.data, r["line_off"], 0)
        if "column" in fields:
            struct.pack_into("<I", img.data, r["col_off"], 0)


def resolve_source(path, roots):
    if os.path.isabs(path):
        return path if os.path.exists(path) else None
    for r in roots:
        cand = os.path.join(r, path)
        if os.path.exists(cand):
            return cand
    return None


def line_count(path):
    with open(path, "rb") as fh:
        return sum(1 for _ in fh)


def self_check(recs, roots):
    """Every record's line must fit inside the file it names.  Prints the census."""
    cache = {}
    ok = out_of_range = unresolved = 0
    bad = []
    for r in recs:
        p = r["file"]
        if p not in cache:
            src = resolve_source(p, roots)
            cache[p] = line_count(src) if src else None
        n = cache[p]
        if n is None:
            unresolved += 1
        elif r["line"] <= n:
            ok += 1
        else:
            out_of_range += 1
            bad.append((r, n))
    print("SELF-CHECK  records=%d  line<=filelen=%d  OUT-OF-RANGE=%d  unresolved=%d"
          % (len(recs), ok, out_of_range, unresolved))
    for r, n in bad[:20]:
        print("   OUT OF RANGE  off=%d  %s:%d:%d  (file has %d lines)"
              % (r["off"], r["file"], r["line"], r["col"], n))
    return out_of_range


def cmd_list(img, recs):
    print("# image=%s  bytes=%d  elf=%s  records=%d"
          % (img.path, len(img.data), img.is_elf, len(recs)))
    print("# %-10s %-10s %s" % ("rec_off", "line_off", "file:line:col"))
    for r in sorted(recs, key=lambda x: (x["file"], x["line"], x["col"])):
        print("  %-10d %-10d %s:%d:%d" % (r["off"], r["line_off"], r["file"], r["line"], r["col"]))
    by_file = Counter(r["file"] for r in recs)
    print("# distinct files: %d" % len(by_file))
    for f, c in by_file.most_common():
        print("#   %5d  %s" % (c, f))


def cmd_compare(a_path, b_path, base, fields):
    """Raw vs normalized, plus how much of the raw delta is pure line renumbering."""
    a = Image(a_path, base)
    b = Image(b_path, base)
    raw_a, raw_b = a.digest(), b.digest()
    comparable = a.layout() == b.layout()

    recs_a = find_records(a)
    recs_b = find_records(b)
    loc_bytes = normalized_offsets(recs_a, fields) | normalized_offsets(recs_b, fields)

    stray = []
    n_diff = n_covered = 0
    if comparable:
        va, offs = a.view()
        vb, _ = b.view()
        for i in range(len(va)):
            if va[i] != vb[i]:
                n_diff += 1
                o = offs[i]
                if o in loc_bytes:
                    n_covered += 1
                else:
                    stray.append((o, va[i], vb[i]))

    normalize(a, recs_a, fields)
    normalize(b, recs_b, fields)
    norm_a, norm_b = a.digest(), b.digest()

    print("A  %s" % a_path)
    print("B  %s" % b_path)
    print("   file sizes      %d / %d%s"
          % (len(a.data), len(b.data),
             "" if len(a.data) == len(b.data) else "   (differ — non-loadable sections)"))
    print("   loadable layout %s" % ("IDENTICAL" if comparable else "DIFFERENT — byte census skipped"))
    print("   raw    A        %s" % raw_a)
    print("   raw    B        %s   %s" % (raw_b, "SAME" if raw_a == raw_b else "DIFFER"))
    print("   normalized A    %s" % norm_a)
    print("   normalized B    %s   %s" % (norm_b, "SAME" if norm_a == norm_b else "DIFFER"))
    print("   records         A=%d  B=%d" % (len(recs_a), len(recs_b)))
    if comparable:
        print("   loadable differing bytes       %d" % n_diff)
        print("   ...inside a Location %-9s %d" % ("/".join(sorted(fields)), n_covered))
        print("   ...ELSEWHERE (real change)     %d" % len(stray))
        for o, x, y in stray[:20]:
            print("        stray file offset %d   %02x -> %02x" % (o, x, y))
    if norm_a == norm_b:
        print("VERDICT: EQUIVALENT — the images differ only in panic line numbers.")
        return 0
    print("VERDICT: DIFFERENT — a real change survives normalization.")
    return 1


def main():
    ap = argparse.ArgumentParser(
        description="Zero panic::Location line fields in a kernel image and hash what is left.")
    ap.add_argument("image")
    ap.add_argument("--compare", metavar="OTHER",
                    help="compare against another image; exit 1 if they really differ")
    ap.add_argument("--out", metavar="FILE", help="write the normalized image here")
    ap.add_argument("--list", action="store_true", help="print every record found")
    ap.add_argument("--fields", default="line",
                    help="which fields to zero: line | line,column  (default: line)")
    ap.add_argument("--base", default=hex(PI_FLAT_BASE),
                    help="load base of a FLAT image (default 0x80000, pi-baremetal.ld)")
    ap.add_argument("--srcroot", action="append", default=[],
                    help="a source root; repeatable.  Enables the line<=filelen self-check")
    args = ap.parse_args()

    fields = set(f.strip() for f in args.fields.split(",") if f.strip())
    unknown = fields - {"line", "column"}
    if unknown:
        sys.exit("panicloc-normalize: unknown field(s): %s" % ", ".join(sorted(unknown)))
    base = int(args.base, 0)

    if args.compare:
        sys.exit(cmd_compare(args.image, args.compare, base, fields))

    img = Image(args.image, base)
    recs = find_records(img)

    if args.list:
        cmd_list(img, recs)

    if args.srcroot:
        if self_check(recs, args.srcroot):
            sys.exit("panicloc-normalize: self-check FAILED — a record's line exceeds its file")

    raw = img.digest()
    normalize(img, recs, fields)
    norm = img.digest()

    if args.out:
        with open(args.out, "wb") as fh:
            fh.write(img.data)

    print("image        %s  (%s, %d bytes)"
          % (img.path, "ELF" if img.is_elf else "flat", len(img.data)))
    print("records      %d" % len(recs))
    print("zeroed       %s" % "/".join(sorted(fields)))
    print("raw          %s" % raw)
    print("normalized   %s" % norm)


if __name__ == "__main__":
    main()
