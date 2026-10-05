#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# VP8CORE oracle input: the ALPH paths the public gallery files never use. Takes a lossy+alpha WebP
# and the alpha plane Chromium decoded from it (webp-raw.cjs .rgba output), and writes four variants
# whose ALPH chunk is RAW (compression 0) with filtering method 0, 1 (horizontal), 2 (vertical) and
# 3 (gradient), per RFC 9649 §2.7. Chromium then decodes the variants too; a decoder that gets any
# predictor rule wrong disagrees with Chromium on them.
#   alph-variants.py <in.webp> <chromium.rgba> <outdir>
import os, struct, sys

src, rgba_path, outdir = sys.argv[1:4]
b = open(src, 'rb').read()
rgba = open(rgba_path, 'rb').read()
chunks, p = [], 12
while p + 8 <= len(b):
    cc, n = b[p:p + 4], struct.unpack('<I', b[p + 4:p + 8])[0]
    chunks.append((cc, b[p + 8:p + 8 + n]))
    p += 8 + n + (n & 1)
vp8x = dict(chunks)[b'VP8X']
w = int.from_bytes(vp8x[4:7], 'little') + 1
h = int.from_bytes(vp8x[7:10], 'little') + 1
alpha = rgba[3::4]
assert len(alpha) == w * h

def pred(a, x, y, m):
    if x == 0 and y == 0: return 0
    if y == 0: return a[x - 1]
    if x == 0: return a[(y - 1) * w]
    l, t, tl = a[y * w + x - 1], a[(y - 1) * w + x], a[(y - 1) * w + x - 1]
    return {1: l, 2: t, 3: max(0, min(255, l + t - tl))}[m]

os.makedirs(outdir, exist_ok=True)
base = os.path.splitext(os.path.basename(src))[0]
for m in range(4):
    data = bytes(alpha) if m == 0 else bytes((alpha[y * w + x] - pred(alpha, x, y, m)) & 255 for y in range(h) for x in range(w))
    alph = bytes([(m << 2) | 0]) + data
    body = b''
    for cc, d in chunks:
        d = alph if cc == b'ALPH' else d
        body += cc + struct.pack('<I', len(d)) + d + (b'\0' if len(d) & 1 else b'')
    out = b'RIFF' + struct.pack('<I', 4 + len(body)) + b'WEBP' + body
    path = os.path.join(outdir, f'{base}-rawalpha-f{m}.webp')
    open(path, 'wb').write(out)
    print(path, len(out))
