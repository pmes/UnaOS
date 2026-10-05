#!/usr/bin/env python3
"""FONTHINT (SR62) per-glyph oracle: FreeType itself (freetype-py, FreeType 2.13.2 with HarfBuzz) at the hinting
mode Chromium asks for on this host — fontconfig hintslight → Skia kSlight → FT_LOAD_TARGET_LIGHT (no autohint
override, no stem darkening). Reads jobs `path<TAB>face_index<TAB>size_px<TAB>gid,gid,...` on stdin; writes one line
per glyph:

    path size gid  ncontours  ends  points(x,y,tag;...)  left top w h coverage-hex

Points are FreeType's hinted outline in 26.6 (y up), the bitmap FT_RENDER_MODE_LIGHT 8-bit coverage. Prints
`NO_FREETYPE` and exits 0 when freetype-py is missing (the Rust test then skips).
"""
import sys
try:
    import freetype as ft
except Exception:
    print('NO_FREETYPE'); sys.exit(0)

mode = sys.argv[1] if len(sys.argv) > 1 else 'light'
flags = {'light': ft.FT_LOAD_TARGET_LIGHT, 'none': ft.FT_LOAD_NO_HINTING,
         'native': ft.FT_LOAD_TARGET_LIGHT | ft.FT_LOAD_NO_AUTOHINT}[mode]
faces = {}
out = sys.stdout
for line in sys.stdin:
    line = line.rstrip('\n')
    if not line:
        continue
    path, idx, size, gids = line.split('\t')
    key = (path, int(idx))
    if key not in faces:
        faces[key] = ft.Face(path, int(idx))
    f = faces[key]
    f.set_char_size(int(round(float(size) * 64)), 0, 72, 72)
    for g in gids.split(','):
        gid = int(g)
        try:
            f.load_glyph(gid, flags)
        except Exception:
            continue
        o = f.glyph.outline
        pts = ';'.join(f'{x},{y},{t & 3}' for (x, y), t in zip(o.points, o.tags))
        ends = ','.join(str(c) for c in o.contours)
        try:
            f.load_glyph(gid, flags | ft.FT_LOAD_RENDER)
            b = f.glyph.bitmap
            data = bytes(b.buffer[r * b.pitch + c] for r in range(b.rows) for c in range(b.width))
            bm = f'{f.glyph.bitmap_left}\t{f.glyph.bitmap_top}\t{b.width}\t{b.rows}\t{data.hex()}'
        except Exception:
            bm = '0\t0\t0\t0\t'
        out.write(f'{path}\t{size}\t{gid}\t{len(o.contours)}\t{ends}\t{pts}\t{bm}\n')
