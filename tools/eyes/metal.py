#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
"""EYES `metal` suite scorer (GLASSEYES, rmbp-ledger B343) — python3 stdlib only.

    metal.py <suite-dir> <out-dir> --from <dir> [--accept] [--only SUBSTR]

For every case in <suite-dir>/cases/*.toml: SUBJECT = the kernel's `shot` PNG under --from, ORACLE = the golden
under <suite-dir>/golden/, MASK = the kernel's `<STEM>.MSK` PNG (white = masked) plus any `mask` rectangles.
Scores `mismatch` = % of UNMASKED pixels whose largest channel differs by > 40 (EYES's PIXEL_TOL), writes
<out>/<case>.diff.png (golden faded grey, red where they differ, blue where masked), <out>/scores.json and
<out>/SCORE.md (the drift report). --accept first copies each subject into golden/ (the first flown boot).
Exit 0 green, 1 a required case past the gate (suite.toml [gate] max_mismatch), 2 harness error.
"""
import json
import os
import shutil
import struct
import sys
import tomllib
import zlib

PIXEL_TOL = 40
SIG = b"\x89PNG\r\n\x1a\n"


def png_read(path):
    """-> (w, h, rows) with rows a list of bytes objects, RGB8. Depth 8, colour 0/2/4/6, non-interlaced."""
    with open(path, "rb") as f:
        b = f.read()
    if b[:8] != SIG:
        raise ValueError("not a PNG")
    at, idat, w = 8, [], None
    while at + 8 <= len(b):
        n, kind = struct.unpack(">I4s", b[at:at + 8])
        data = b[at + 8:at + 8 + n]
        if kind == b"IHDR":
            w, h, depth, colour, _, _, inter = struct.unpack(">IIBBBBB", data)
            if depth != 8 or colour not in (0, 2, 4, 6) or inter:
                raise ValueError(f"unsupported PNG (depth {depth} colour {colour} interlace {inter})")
        elif kind == b"IDAT":
            idat.append(data)
        elif kind == b"IEND":
            break
        at += 12 + n
    if w is None:
        raise ValueError("no IHDR")
    if not idat:
        raise ValueError("no IDAT (a truncated capture: the boot was cut before IEND)")
    raw = zlib.decompress(b"".join(idat))
    bpp = {0: 1, 2: 3, 4: 2, 6: 4}[colour]
    stride = w * bpp
    rows, prev = [], bytearray(stride)
    for y in range(h):
        o = y * (stride + 1)
        ft, cur = raw[o], bytearray(raw[o + 1:o + 1 + stride])
        if ft == 1:
            for i in range(bpp, stride):
                cur[i] = (cur[i] + cur[i - bpp]) & 0xFF
        elif ft == 2:
            cur = bytearray((c + p) & 0xFF for c, p in zip(cur, prev))
        elif ft == 3:
            for i in range(stride):
                left = cur[i - bpp] if i >= bpp else 0
                cur[i] = (cur[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ft == 4:
            for i in range(stride):
                a = cur[i - bpp] if i >= bpp else 0
                bb, c = prev[i], (prev[i - bpp] if i >= bpp else 0)
                p = a + bb - c
                pa, pb, pc = abs(p - a), abs(p - bb), abs(p - c)
                cur[i] = (cur[i] + (a if pa <= pb and pa <= pc else bb if pb <= pc else c)) & 0xFF
        elif ft != 0:
            raise ValueError(f"bad filter {ft} on row {y}")
        prev = cur
        if colour == 2:
            rows.append(bytes(cur))
        elif colour == 6:
            rows.append(bytes(v for i, v in enumerate(cur) if i % 4 != 3))
        elif colour == 0:
            rows.append(bytes(v for v in cur for _ in range(3)))
        else:
            rows.append(bytes(cur[i] for i in range(0, stride, 2) for _ in range(3)))
    return w, h, rows


def png_write(path, w, h, rows):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + r for r in rows)
    with open(path, "wb") as f:
        f.write(SIG + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def find(path):
    """The file, or its other spelling: STEM.PNG <-> stem.png, STEM.MSK <-> state.mask.png; any case."""
    if os.path.exists(path):
        return path
    d, base = os.path.split(path)
    if os.path.isdir(d):
        for f in os.listdir(d):
            if f.lower() == base.lower():
                return os.path.join(d, f)
    return None


def score(case, suite, outdir, frm):
    name = case["name"]
    fill = lambda s: s.replace("{from}", frm).replace("{suite}", suite)
    subj = find(fill(case["subject"]["path"]))
    ref = os.path.join(suite, fill(case["oracle"]["path"]))
    mpath = case.get("mask_png") and find(fill(case["mask_png"]))
    if subj is None:
        return {"note": "no subject frame (state not shot this boot)", "mismatch": 100.0, "masked": 0.0}
    if not os.path.exists(ref):
        return {"note": "no golden (run with --accept to bless)", "mismatch": 100.0, "masked": 0.0}
    gw, gh, g = png_read(ref)
    sw, sh, s = png_read(subj)
    note = "" if (sw, sh) == (gw, gh) else f"subject {sw}x{sh} != golden {gw}x{gh}"
    mask = None
    if mpath:
        mw, mh, mrows = png_read(mpath)
        mask = [bytes(1 if (y < mh and x < mw and mrows[y][3 * x] >= 0x80) else 0 for x in range(gw)) for y in range(gh)]
    elif case.get("mask_png"):
        note = (note + "; " if note else "") + "no mask file (scored unmasked)"
    rects = case.get("mask", [])
    bad = masked_n = 0
    diff = []
    for y in range(gh):
        gr = g[y]
        sr = s[y] if y < sh else b""
        mrow = mask[y] if mask else None
        rect_x = [(r[0], r[0] + r[2]) for r in rects if r[1] <= y < r[1] + r[3]]
        out = bytearray(gw * 3)
        same_row = sr[:gw * 3] == gr and not rect_x and (mrow is None or not any(mrow))
        for x in range(gw):
            i = 3 * x
            lum = (299 * gr[i] + 587 * gr[i + 1] + 114 * gr[i + 2]) // 1000
            base = 200 + lum // 5
            m = (mrow is not None and mrow[x]) or any(a <= x < b for a, b in rect_x)
            if m:
                masked_n += 1
                out[i:i + 3] = b"\xb4\xc8\xff"
                continue
            if same_row:
                out[i:i + 3] = bytes((base, base, base))
                continue
            sp = sr[i:i + 3] if i + 3 <= len(sr) else b"\xff\xff\xff"
            d = max(abs(sp[0] - gr[i]), abs(sp[1] - gr[i + 1]), abs(sp[2] - gr[i + 2]))
            if d > PIXEL_TOL:
                bad += 1
            t = min(d / 128.0, 1.0)
            v = int(base * (1 - t))
            out[i:i + 3] = bytes((int(base * (1 - t) + 255 * t), v, v))
        diff.append(bytes(out))
    png_write(os.path.join(outdir, f"{name}.diff.png"), gw, gh, diff)
    scored = gw * gh - masked_n
    return {"mismatch": round(100.0 * bad / scored, 3) if scored else 0.0,
            "masked": round(100.0 * masked_n / (gw * gh), 3), "subject": subj, "note": note}


def main(argv):
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    suite, outdir, rest = argv[0], argv[1], argv[2:]
    frm = rest[rest.index("--from") + 1] if "--from" in rest else None
    only = rest[rest.index("--only") + 1] if "--only" in rest else None
    if not frm or not os.path.isdir(frm):
        print("metal: --from <dir> (the Shots/ folder pulled off the card) is required", file=sys.stderr)
        return 2
    frm = os.path.abspath(frm)
    with open(os.path.join(suite, "suite.toml"), "rb") as f:
        st = tomllib.load(f)
    gate = float(st.get("gate", {}).get("max_mismatch", 0.5))
    cases = []
    for fn in sorted(os.listdir(os.path.join(suite, "cases"))):
        if fn.endswith(".toml"):
            with open(os.path.join(suite, "cases", fn), "rb") as f:
                cases += tomllib.load(f).get("case", [])
    if only:
        cases = [c for c in cases if only in c["name"]]
    os.makedirs(outdir, exist_ok=True)
    if "--accept" in rest:
        for c in cases:
            fill = lambda s: s.replace("{from}", frm).replace("{suite}", suite)
            src = find(fill(c["subject"]["path"]))
            if src:
                dst = os.path.join(suite, fill(c["oracle"]["path"]))
                shutil.copyfile(src, dst)
                m = c.get("mask_png") and find(fill(c["mask_png"]))
                if m:
                    shutil.copyfile(m, dst[:-4] + ".mask.png")
                print(f"accept: {c['name']:<18} <- {src}")
    scores, red = {}, []
    for c in cases:
        try:
            r = score(c, suite, outdir, frm)
        except (OSError, ValueError, zlib.error) as e:
            r = {"mismatch": 100.0, "masked": 0.0, "note": f"harness: {e}"}
        scores[c["name"]] = r
        opt = c.get("optional", False)
        if not opt and r["mismatch"] > gate:
            red.append(c["name"])
        print(f"{c['name']:<18} mismatch {r['mismatch']:7.3f}%  masked {r['masked']:6.2f}%"
              f"{'  (optional)' if opt else ''}{'  [' + r['note'] + ']' if r.get('note') else ''}")
    with open(os.path.join(outdir, "scores.json"), "w") as f:
        json.dump({"from": frm, "gate_max_mismatch": gate, "cases": scores}, f, indent=2, sort_keys=True)
    md = [f"# EYES drift report — suite `metal` (from `{frm}`)", "",
          f"Gate: a required state fails above {gate}% mismatch OUTSIDE its mask (channel delta > {PIXEL_TOL}).", "",
          "| state | mismatch % (unmasked) | masked % | diff | note |", "|---|---|---|---|---|"]
    for c in sorted(cases, key=lambda c: -scores[c["name"]]["mismatch"]):
        r = scores[c["name"]]
        md.append(f"| {c['name']}{' (optional)' if c.get('optional') else ''} | {r['mismatch']:.3f} | {r['masked']:.2f} | "
                  f"`{c['name']}.diff.png` | {r.get('note', '')} |")
    with open(os.path.join(outdir, "SCORE.md"), "w") as f:
        f.write("\n".join(md) + "\n")
    print(f"metal: wrote {os.path.join(outdir, 'SCORE.md')}")
    if red:
        print(f"gate: RED ({', '.join(red)} past {gate}%)")
        return 1
    print("gate: GREEN (metal)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
