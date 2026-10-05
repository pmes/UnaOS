#!/usr/bin/env python3
"""FONTHINT (SR62): freeze a slice of the FreeType per-glyph oracle into tests/data/hint_kat.tsv so the hinting KAT
runs on a host without freetype-py (tests/hint_kat.rs). Each line: file, sha256 of the file, size px, gid, the
FT_LOAD_TARGET_LIGHT outline points in 26.6 as x,y;x,y;... (FreeType 2.13.2 via freetype-py 2.5.1)."""
import hashlib, sys
import freetype as ft

CASES = [
    ('/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf', 0, 'Hamburgefonstiv AVWxyz0123 →∑∞', [12, 16, 24]),
    ('/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf', 0, 'Hamburgefonstiv ЖΩ', [12, 16]),
    ('/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf', 0, 'Hamburgefonstiv', [12, 16]),
    ('/usr/share/fonts/truetype/freefont/FreeSans.ttf', 0, 'AHxfi', [16]),
    ('/usr/share/fonts/opentype/tlwg/Loma.otf', 0, 'กขคสวัด Ham', [12, 16, 24]),
    ('/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc', 0, 'の世界你', [12, 16]),
]

out = sys.stdout
for path, idx, text, sizes in CASES:
    sha = hashlib.sha256(open(path, 'rb').read()).hexdigest()
    f = ft.Face(path, idx)
    gids = []
    for ch in text:
        g = f.get_char_index(ch)
        if g and g not in gids:
            gids.append(g)
    for s in sizes:
        f.set_char_size(s * 64, 0, 72, 72)
        for g in gids:
            f.load_glyph(g, ft.FT_LOAD_TARGET_LIGHT)
            pts = ';'.join(f'{x},{y}' for x, y in f.glyph.outline.points)
            out.write(f'{path}\t{sha}\t{s}\t{g}\t{pts}\n')
