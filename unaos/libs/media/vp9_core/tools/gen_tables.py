#!/usr/bin/env python3
"""Generate src/tables.rs from the VP9 reference decoder's constant tables.

The VP9 Bitstream & Decoding Process Specification (v0.6) prints its default
probabilities, scans, quantizer lookups and filter taps as tables whose values
are those of the reference decoder, libvpx (BSD-3-Clause, see
third_party/LICENSE-libvpx). This tool transcribes them mechanically from a
libvpx checkout so that nothing is typed by hand: every C initializer named
below is parsed (nested braces, brace elision, symbolic enumerators), padded
the way C pads a short initializer, and checked against its declared size --
a count that does not match aborts the generator.

    python3 tools/gen_tables.py <libvpx checkout> > src/tables.rs

Files read: vp9/common/{vp9_entropy.c, vp9_entropymode.c, vp9_entropymv.c,
vp9_scan.c, vp9_filter.c, vp9_quant_common.c, vp9_common_data.c}.
"""
import os, re, sys

root = sys.argv[1]

def read(rel):
    for cand in (os.path.join(root, rel), os.path.join(root, os.path.basename(rel))):
        if os.path.exists(cand):
            s = open(cand).read()
            s = re.sub(r"/\*.*?\*/", " ", s, flags=re.S)
            s = re.sub(r"//[^\n]*", " ", s)
            return s
    sys.exit("missing " + rel)

ENUM = {}
for i, n in enumerate("4X4 4X8 8X4 8X8 8X16 16X8 16X16 16X32 32X16 32X32 32X64 64X32 64X64".split()):
    ENUM["BLOCK_" + n] = i
ENUM["BLOCK_INVALID"] = 13
for i, n in enumerate("4X4 8X8 16X16 32X32".split()):
    ENUM["TX_" + n] = i

def body_of(src, name):
    m = re.search(r"\b" + re.escape(name) + r"\b[^=;{]*=\s*\{", src)
    if not m:
        sys.exit("table not found: " + name)
    i = m.end() - 1
    depth = 0
    for j in range(i, len(src)):
        if src[j] == "{":
            depth += 1
        elif src[j] == "}":
            depth -= 1
            if depth == 0:
                return src[i:j + 1]
    sys.exit("unterminated: " + name)

def parse(text):
    toks = re.findall(r"\{|\}|(?:-\s*)?[A-Za-z_][A-Za-z_0-9]*|(?:-\s*)?\d+", text)
    pos = 0
    def node():
        nonlocal pos
        t = toks[pos]
        if t == "{":
            pos += 1
            out = []
            while toks[pos] != "}":
                out.append(node())
            pos += 1
            return out
        pos += 1
        t = re.sub(r"\s", "", t)
        neg = t.startswith("-")
        t = t.lstrip("-")
        v = int(t) if t.isdigit() else ENUM[t]
        return -v if neg else v
    return node()

def leaves(n):
    if isinstance(n, list):
        for c in n:
            yield from leaves(c)
    else:
        yield n

def shaped(n, dims):
    """C aggregate initialisation: a braced list shorter than its dimension is zero-padded."""
    if not dims:
        if isinstance(n, list):
            raise ValueError
        return [n]
    if not isinstance(n, list) or len(n) > dims[0]:
        raise ValueError
    size = 1
    for d in dims[1:]:
        size *= d
    out = []
    for c in n:
        out += shaped(c, dims[1:])
    out += [0] * (size * (dims[0] - len(n)))
    return out

def table(src, name, dims):
    tree = parse(body_of(src, name))
    total = 1
    for d in dims:
        total *= d
    try:
        vals = shaped(tree, dims)
    except (ValueError, TypeError):
        vals = list(leaves(tree))
    if len(vals) != total:
        sys.exit(f"{name}: {len(vals)} values, declared {dims} = {total}")
    return vals

def rust_ty(base, dims):
    t = base
    for d in reversed(dims):
        t = f"[{t}; {d}]"
    return t

def rust_val(vals, dims):
    if len(dims) == 1:
        return "[" + ", ".join(str(v) for v in vals) + "]"
    step = len(vals) // dims[0]
    return "[" + ", ".join(rust_val(vals[i * step:(i + 1) * step], dims[1:]) for i in range(dims[0])) + "]"

out = []
def emit(rname, base, dims, vals, doc):
    out.append(f"/// {doc}")
    out.append(f"pub const {rname}: {rust_ty(base, dims)} = {rust_val(vals, dims)};")
    out.append("")

ent = read("vp9/common/vp9_entropy.c")
mode = read("vp9/common/vp9_entropymode.c")
mv = read("vp9/common/vp9_entropymv.c")
scan = read("vp9/common/vp9_scan.c")
filt = read("vp9/common/vp9_filter.c")
quant = read("vp9/common/vp9_quant_common.c")
common = read("vp9/common/vp9_common_data.c")

emit("CAT6_PROB", "u8", [18], table(ent, "vp9_cat6_prob_high12", [18]),
     "Category-6 extra-bit probabilities for 12-bit; 10-bit starts at [2], 8-bit at [4] (spec 9.3 cat6_prob).")
emit("COEFBAND_8X8PLUS", "u8", [1024], table(ent, "vp9_coefband_trans_8x8plus", [1024]), "Coefficient band by scan position, 8x8 and larger.")
emit("COEFBAND_4X4", "u8", [16], table(ent, "vp9_coefband_trans_4x4", [16]), "Coefficient band by scan position, 4x4.")
emit("ENERGY_CLASS", "u8", [12], table(ent, "vp9_pt_energy_class", [12]), "Token -> energy class for the token context.")
emit("PARETO8_FULL", "u8", [255, 8], table(ent, "vp9_pareto8_full", [255, 8]), "The Pareto model: the 8 tail node probabilities from the pivot node.")
coef = []
for n in ("4x4", "8x8", "16x16", "32x32"):
    coef += table(ent, "default_coef_probs_" + n, [2, 2, 6, 6, 3])
emit("DEFAULT_COEF_PROBS", "u8", [4, 2, 2, 6, 6, 3], coef, "Default coefficient probabilities [tx][plane][ref][band][ctx][node].")

emit("KF_Y_MODE_PROB", "u8", [10, 10, 9], table(mode, "vp9_kf_y_mode_prob", [10, 10, 9]), "Key-frame luma mode probabilities [above][left].")
emit("KF_UV_MODE_PROB", "u8", [10, 9], table(mode, "vp9_kf_uv_mode_prob", [10, 9]), "Key-frame chroma mode probabilities [y mode].")
emit("DEFAULT_IF_Y_PROBS", "u8", [4, 9], table(mode, "default_if_y_probs", [4, 9]), "Inter-frame luma mode probabilities [size group].")
emit("DEFAULT_IF_UV_PROBS", "u8", [10, 9], table(mode, "default_if_uv_probs", [10, 9]), "Inter-frame chroma mode probabilities [y mode].")
emit("KF_PARTITION_PROBS", "u8", [16, 3], table(mode, "vp9_kf_partition_probs", [16, 3]), "Intra-frame partition probabilities.")
emit("DEFAULT_PARTITION_PROBS", "u8", [16, 3], table(mode, "default_partition_probs", [16, 3]), "Inter-frame partition probabilities.")
emit("DEFAULT_INTER_MODE_PROBS", "u8", [7, 3], table(mode, "default_inter_mode_probs", [7, 3]), "Inter mode probabilities [mode context].")
emit("DEFAULT_INTRA_INTER_P", "u8", [4], table(mode, "default_intra_inter_p", [4]), "is_inter probabilities.")
emit("DEFAULT_COMP_INTER_P", "u8", [5], table(mode, "default_comp_inter_p", [5]), "comp_mode probabilities.")
emit("DEFAULT_COMP_REF_P", "u8", [5], table(mode, "default_comp_ref_p", [5]), "comp_ref probabilities.")
emit("DEFAULT_SINGLE_REF_P", "u8", [5, 2], table(mode, "default_single_ref_p", [5, 2]), "single_ref_p1/p2 probabilities.")
emit("DEFAULT_TX_PROBS", "u8", [12], table(mode, "default_tx_probs", [12]),
     "tx_size probabilities, flattened: p32x32[2][3], p16x16[2][2], p8x8[2][1].")
emit("DEFAULT_SKIP_PROBS", "u8", [3], table(mode, "default_skip_probs", [3]), "skip probabilities.")
emit("DEFAULT_SWITCHABLE_INTERP_PROB", "u8", [4, 2], table(mode, "default_switchable_interp_prob", [4, 2]), "interp_filter probabilities.")
emit("DEFAULT_NMV_CONTEXT", "u8", [69], table(mv, "default_nmv_context", [69]),
     "MV probabilities, flattened: joints[3], then per component (row, col): sign, classes[10], class0[1], bits[10], class0_fp[2][3], fp[3], class0_hp, hp.")

filters = []
for n in ("sub_pel_filters_8", "sub_pel_filters_8lp", "sub_pel_filters_8s", "bilinear_filters"):
    filters += table(filt, n, [16, 8])
emit("SUBPEL_FILTERS", "i16", [4, 16, 8], filters, "8-tap kernels by interp_filter: EIGHTTAP (regular), EIGHTTAP_SMOOTH, EIGHTTAP_SHARP, BILINEAR.")

for n, size in (("default_scan_4x4", 16), ("col_scan_4x4", 16), ("row_scan_4x4", 16),
                ("default_scan_8x8", 64), ("col_scan_8x8", 64), ("row_scan_8x8", 64),
                ("default_scan_16x16", 256), ("col_scan_16x16", 256), ("row_scan_16x16", 256),
                ("default_scan_32x32", 1024)):
    emit(n.upper(), "u16", [size], table(scan, n, [size]), f"Scan order {n}.")
    emit(n.upper() + "_NEIGHBORS", "u16", [2 * (size + 1)], table(scan, n + "_neighbors", [2 * (size + 1)]),
         f"Token-context neighbours (two positions per scan index) for {n}.")

dc = table(quant, "dc_qlookup", [256]) + table(quant, "dc_qlookup_10", [256]) + table(quant, "dc_qlookup_12", [256])
ac = table(quant, "ac_qlookup", [256]) + table(quant, "ac_qlookup_10", [256]) + table(quant, "ac_qlookup_12", [256])
emit("DC_QLOOKUP", "i32", [3, 256], dc, "DC quantizer step by bit depth (8, 10, 12) and qindex.")
emit("AC_QLOOKUP", "i32", [3, 256], ac, "AC quantizer step by bit depth (8, 10, 12) and qindex.")

emit("SUBSIZE_LOOKUP", "u8", [4, 13], table(common, "subsize_lookup", [4, 13]), "Block size after a partition [partition][block size]; 13 = invalid.")
emit("SS_SIZE_LOOKUP", "u8", [13, 2, 2], table(common, "ss_size_lookup", [13, 2, 2]), "Plane block size [block size][ss_x][ss_y]; 13 = invalid.")
emit("UV_TXSIZE_LOOKUP", "u8", [13, 4, 2, 2], table(common, "uv_txsize_lookup", [13, 4, 2, 2]), "Chroma transform size [block size][tx size][ss_x][ss_y].")

print("// SPDX-License-Identifier: LGPL-3.0-or-later")
print("// GENERATED by tools/gen_tables.py from the VP9 reference decoder's tables (libvpx, BSD-3-Clause,")
print("// Copyright (c) 2010 The WebM Project authors; see third_party/LICENSE-libvpx). Do not edit by hand.")
print("//! VP9 constant tables (default probabilities, scans, quantizers, filter taps, block lookups).")
print("#![allow(clippy::all)]")
print()
print("\n".join(out))
