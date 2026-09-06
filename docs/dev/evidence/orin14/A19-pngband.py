#!/usr/bin/env python3
"""A19 band scorer: decode a UnaOS PRTSCR PNG (zlib + PNG filters, stdlib only) and count
non-DESKTOP_BG pixels in the top-left band (x 0-700, y 34-120) and two control bands.

Usage: python3 pngband.py SCREEN0.PNG
Verdict: PASS iff the band has 0 non-background pixels (tolerance 8/255 per channel).
DESKTOP_BG = wm::DESKTOP_BG = 0x002D2B55 (R=0x2D G=0x2B B=0x55)."""
import sys, zlib, struct

BG = (0x2D, 0x2B, 0x55)
TOL = 8

def decode(path):
    d = open(path, 'rb').read()
    assert d[:8] == b'\x89PNG\r\n\x1a\n', 'not a PNG'
    p = 8; idat = b''; w = h = bpp = None
    while p < len(d):
        ln, = struct.unpack('>I', d[p:p+4]); typ = d[p+4:p+8]; body = d[p+8:p+8+ln]; p += 12 + ln
        if typ == b'IHDR':
            w, h, bd, ct = struct.unpack('>IIBB', body[:10]); assert bd == 8, 'bit depth'
            bpp = {2: 3, 6: 4, 0: 1, 4: 2}[ct]
        elif typ == b'IDAT':
            idat += body
        elif typ == b'IEND':
            break
    raw = zlib.decompress(idat); stride = w * bpp; rows = []; prev = bytearray(stride); q = 0
    for _y in range(h):
        f = raw[q]; line = bytearray(raw[q+1:q+1+stride]); q += 1 + stride
        if f == 1:
            for i in range(bpp, stride): line[i] = (line[i] + line[i-bpp]) & 255
        elif f == 2:
            for i in range(stride): line[i] = (line[i] + prev[i]) & 255
        elif f == 3:
            for i in range(stride): line[i] = (line[i] + ((line[i-bpp] if i >= bpp else 0) + prev[i]) // 2) & 255
        elif f == 4:
            for i in range(stride):
                a = line[i-bpp] if i >= bpp else 0; b = prev[i]; c = prev[i-bpp] if i >= bpp else 0
                pa, pb, pc = abs(b-c), abs(a-c), abs(a+b-2*c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else (b if pb <= pc else c))) & 255
        rows.append(bytes(line)); prev = line
    return w, h, bpp, rows

def is_non_bg(r, x, bpp):
    px = r[x*bpp:x*bpp+3]
    return max(abs(px[0]-BG[0]), abs(px[1]-BG[1]), abs(px[2]-BG[2])) > TOL

def band(rows, bpp, x0, y0, x1, y1):
    non = tot = 0; rowhits = {}; xs = []
    for y in range(y0, y1):
        r = rows[y]
        for x in range(x0, x1):
            tot += 1
            if is_non_bg(r, x, bpp):
                non += 1; rowhits[y] = rowhits.get(y, 0) + 1; xs.append(x)
    return non, tot, rowhits, xs

if __name__ == '__main__':
    w, h, bpp, rows = decode(sys.argv[1])
    print(f"{sys.argv[1]}: {w}x{h} bpp={bpp}")
    # Controls: right of the band on the same rows, and below it LEFT of the console window (x < 307 on
    # the bench cascade — the window's box starts at (307,158), so x 0..300 is backdrop by construction).
    bands = {'band': (0, 34, 700, 120), 'ctrl-right': (700, 34, 1400, 120), 'ctrl-below': (0, 120, 300, 220)}
    verdict = None
    for name, (x0, y0, x1, y1) in bands.items():
        non, tot, rh, xs = band(rows, bpp, x0, y0, x1, y1)
        print(f"{name} x{x0}-{x1} y{y0}-{y1}: non-bg={non}/{tot} ({100*non/tot:.1f}%)")
        if rh:
            ys = sorted(rh)
            print(f"  rows with non-bg: y {ys[0]}..{ys[-1]} ({len(ys)} rows); x extent {min(xs)}..{max(xs)}")
            # row-run summary: contiguous row groups = text lines
            groups = []; start = ys[0]; last = ys[0]
            for y in ys[1:]:
                if y != last + 1: groups.append((start, last)); start = y
                last = y
            groups.append((start, last)); print("  row groups (text lines):", groups)
        if name == 'band':
            verdict = 'PASS' if non == 0 else 'FAIL'
    non, tot, _, _ = band(rows, bpp, 0, 0, 700, 34)
    print(f"bar x0-700 y0-34: non-bg={non}/{tot} (the menubar is not bg by design; informational)")
    print("A19 scorer verdict (band must be 0 non-bg):", verdict)
