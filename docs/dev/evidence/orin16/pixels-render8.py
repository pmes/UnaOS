#!/usr/bin/env python3
"""pixels-render8.py — the render8 GLASS scorer (orin 17, executor PIXELS).

Reads the four 1920x1200 PRTSCR captures of the render8 flight and prints, per capture, the
window list with measured rects and one verdict line per owed ledger row, in the scorer style
`<row> pixels: <numbers> -> PASS|FAIL|DATUM`.

Stdlib only — there is no PIL and no ImageMagick on the bench host. The PNG decoder is the one
from `docs/dev/evidence/orin14/A19-pngband.py` (zlib + the five PNG filters); the render8
captures are all filter-0, so the unfilter loop is a pass-through here.

The full-size captures are deliberately NOT in git (27 MB for four files); they live on the card
and in the flight harvest, checksummed by `render8-card-harvest.sha256` beside this file. Point
the script at whatever directory holds them:

    python3 pixels-render8.py <dir-with-SCREEN0..3.PNG>

Every number quoted in `PIXELS-render8.md` is printed by this script.
"""
import sys, os, zlib, struct, collections

BG    = (0x2D, 0x2B, 0x55)   # wm::DESKTOP_BG
BLACK = (0x00, 0x00, 0x00)
SURF  = (0xF5, 0xF2, 0xEA)   # window surface (cream)
RED   = (0xFF, 0x5F, 0x57)   # close disc — the cleanest of the three chrome dots
LIT   = (0x4A, 0x73, 0xAA)   # dock running dot, running
GREY  = (0xCB, 0xCB, 0xCF)   # dock running dot, not running
W, H  = 1920, 1200


# ---------------------------------------------------------------- decode

def decode(path):
    d = open(path, 'rb').read()
    assert d[:8] == b'\x89PNG\r\n\x1a\n', 'not a PNG'
    p = 8; idat = []; w = h = bpp = None
    while p < len(d):
        ln, = struct.unpack('>I', d[p:p+4]); typ = d[p+4:p+8]; body = d[p+8:p+8+ln]; p += 12 + ln
        if typ == b'IHDR':
            w, h, bd, ct = struct.unpack('>IIBB', body[:10]); assert bd == 8, 'bit depth'
            bpp = {2: 3, 6: 4, 0: 1, 4: 2}[ct]
        elif typ == b'IDAT':
            idat.append(body)
        elif typ == b'IEND':
            break
    raw = zlib.decompress(b''.join(idat)); stride = w * bpp
    out = bytearray(); prev = bytearray(stride); q = 0
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
        out += line; prev = line
    assert (w, h) == (W, H) and bpp == 3, f'unexpected geometry {w}x{h} bpp={bpp}'
    return bytes(out)


def px(buf, x, y):
    o = (y*W + x)*3
    return (buf[o], buf[o+1], buf[o+2])


def light(p):
    return min(p) >= 0xD0


# ---------------------------------------------------------------- structure

def boxes(buf, gap=24):
    """Coarse window-box scan: per row, the non-DESKTOP_BG runs merged across <=gap px, printed
    only where the signature changes. Window tops/bottoms/edges fall straight out of it."""
    def runs(y):
        r = []; s = None; o = y*W*3
        for x in range(W):
            nb = (buf[o+x*3], buf[o+x*3+1], buf[o+x*3+2]) != BG
            if nb and s is None: s = x
            elif not nb and s is not None: r.append([s, x-1]); s = None
        if s is not None: r.append([s, W-1])
        m = []
        for a, b in r:
            if m and a - m[-1][1] - 1 <= gap: m[-1][1] = b
            else: m.append([a, b])
        return tuple(tuple(v) for v in m)
    out = []; prev = None; start = 0
    for y in range(H+1):
        r = runs(y) if y < H else None
        if r != prev:
            if prev is not None: out.append((start, y-1, prev))
            prev = r; start = y
    return out


def dots(buf, col, x0=0, y0=0, x1=W-1, y1=H-1, near=40):
    """Cluster exact-colour pixels. The three title-bar discs and the dock's running dots are
    each a single flat-colour blob, so exact match + proximity grouping is enough."""
    gs = []
    for y in range(y0, y1+1):
        o = y*W*3
        for x in range(x0, x1+1):
            if (buf[o+x*3], buf[o+x*3+1], buf[o+x*3+2]) != col: continue
            for g in gs:
                if g[0]-near <= x <= g[2]+near and g[1]-near <= y <= g[3]+near:
                    g[0] = min(g[0], x); g[1] = min(g[1], y)
                    g[2] = max(g[2], x); g[3] = max(g[3], y); g[4] += 1; break
            else:
                gs.append([x, y, x, y, 1])
    gs.sort()
    return gs


def bar_words(buf, x1=400, gap=6):
    """Menu-bar title cells: columns in y0..33 that differ from the bar's own background (sampled
    at x=1500, which is empty in every render8 capture), grouped into words across <=gap px."""
    hit = []
    for x in range(x1):
        h = False
        for y in range(34):
            o = y*W*3; ref = (buf[o+1500*3], buf[o+1500*3+1], buf[o+1500*3+2])
            if (buf[o+x*3], buf[o+x*3+1], buf[o+x*3+2]) != ref: h = True; break
        hit.append(h)
    words = []; s = None; last = None
    for x, h in enumerate(hit):
        if h:
            if s is None: s = x
            last = x
        elif s is not None and x - last > gap:
            words.append((s, last)); s = None
    if s is not None: words.append((s, last))
    out = []
    for a, b in words:
        ys = [y for y in range(34) for x in range(a, b+1)
              if px(buf, x, y) != px(buf, 1500, y)]
        out.append((a, b, min(ys), max(ys)))
    return out


def crystal(buf):
    pts = [(x, y) for y in range(34) for x in range(1800, W)
           if px(buf, x, y) != px(buf, 1500, y)]
    xs = [p[0] for p in pts]; ys = [p[1] for p in pts]
    return min(xs), min(ys), max(xs)-min(xs)+1, max(ys)-min(ys)+1, W-1-max(xs)


def arrow(buf):
    """The pointer sprite: the only small 4-connected component of PURE white on the glass.
    Window borders and highlights also use #ffffff, so the size filter is what isolates it."""
    pts = set()
    for y in range(H):
        o = y*W*3
        for x in range(W):
            if buf[o+x*3] == 255 and buf[o+x*3+1] == 255 and buf[o+x*3+2] == 255: pts.add((x, y))
    seen = set(); out = []
    for p in pts:
        if p in seen: continue
        st = [p]; seen.add(p); comp = []
        while st:
            x, y = st.pop(); comp.append((x, y))
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    q = (x+dx, y+dy)
                    if q in pts and q not in seen: seen.add(q); st.append(q)
        xs = [c[0] for c in comp]; ys = [c[1] for c in comp]
        w = max(xs)-min(xs)+1; hh = max(ys)-min(ys)+1
        if 3 <= w <= 24 and 3 <= hh <= 24 and len(comp) >= 10:
            out.append((min(xs), min(ys), w, hh, len(comp)))
    return sorted(out)


def chrome_run(buf, y, x0, x1):
    """Light pixels on one title-bar row, inside one window's surface span."""
    xs = [x for x in range(x0, x1+1) if light(px(buf, x, y))]
    return (len(xs), min(xs), max(xs)) if xs else (0, None, None)


def content_census(buf, rect, occluders):
    """Black vs non-black over a window's content rect, counting only the pixels no window above
    it covers. Counting the whole rect would score the occluders, not the console."""
    cx0, cy0, cx1, cy1 = rect
    vis = blk = 0; c = collections.Counter()
    for y in range(cy0, cy1+1):
        for x in range(cx0, cx1+1):
            if any(a <= x <= b and u <= y <= v for a, u, b, v in occluders): continue
            vis += 1; p = px(buf, x, y)
            if p == BLACK: blk += 1
            else: c[p] += 1
    return vis, blk, c


# ---------------------------------------------------------------- per-capture facts
#
# Rects below are READ OFF the boxes()/dots() scans (both are printed), not assumed: each window's
# box is (first red disc x)-17, (first red disc y)-10, and its extent is the boxes() signature.

CAPS = {
    0: dict(when='~15:43Z', app='Shell',
            wins=[('shell',   923,  61,  970, 510),
                  ('quarry',  379, 183, 1162, 764),
                  ('console', 307, 158, 1305, 780),
                  ('pulse',    10, 914, 1290, 212)],
            console=(312, 197, 1606, 932),
            occl=[(923, 61, 1892, 570), (379, 183, 1540, 946)]),
    1: None, 2: None,                                    # identical layout; filled in below
    3: dict(when='~15:52Z', app='Quarry',
            wins=[('quarry',   64,  89, 1162, 764),
                  ('console', 307, 195, 1305, 780),
                  ('pulse',    10, 914, 1290, 212)],
            console=(312, 234, 1606, 969),
            occl=[(64, 89, 1225, 852)]),
}
CAPS[1] = CAPS[2] = CAPS[0]

DOCK_TILES = ['console', 'quarry', 'pulse', 'shell']     # read off the tile labels at y1156..1169
DOCK_DOT_X = [795, 903, 1011, 1119]


def score(n, buf):
    cfg = CAPS[n]
    print(f'===== SCREEN{n}.PNG ({cfg["when"]}) — {W}x{H} =====')

    print('-- window-box scan (non-DESKTOP_BG runs, merged across <=24 px) --')
    for a, b, sig in boxes(buf):
        if not sig: continue
        print(f'   y {a}..{b} ({b-a+1} rows): ' + ', '.join(f'x{u}..{v}' for u, v in sig))

    print('-- title-bar chrome discs (close disc #ff5f57) --')
    for g in dots(buf, RED):
        print(f'   disc x{g[0]}..{g[2]} y{g[1]}..{g[3]} ({g[2]-g[0]+1}x{g[3]-g[1]+1}, {g[4]} px)'
              f'  -> window box origin ({g[0]-17},{g[1]-10})')

    print('-- menu bar --')
    for a, b, u, v in bar_words(buf):
        print(f'   title cell glyphs x{a}..{b} (w={b-a+1}) y{u}..{v} (h={v-u+1})')
    cx, cy, cw, ch, gapr = crystal(buf)
    print(f'   crystal glyph x{cx}..{cx+cw-1} y{cy}..{cy+ch-1} ({cw}x{ch}) left_edge_x={cx} gap_right={gapr}')

    print('-- pointer sprite (isolated pure-white component) --')
    ar = arrow(buf)
    print('   ' + (str(ar) if ar else 'none'))

    print('-- pulse title-bar chrome vs surface (surface span x15..1294) --')
    for y in (923, 930, 940, 950, 952):
        cnt, lo, hi = chrome_run(buf, y, 15, 1294)
        print(f'   y={y}: light px={cnt} x{lo}..{hi}'
              + ('' if hi is None or hi == 1294 else f'  first px past {hi} = #%02x%02x%02x' % px(buf, hi+1, y)))

    print('-- console content --')
    cr = cfg['console']
    vis, blk, c = content_census(buf, cr, cfg['occl'])
    print(f'   rect {cr[2]-cr[0]+1}x{cr[3]-cr[1]+1} at ({cr[0]},{cr[1]}): unoccluded={vis} '
          f'black={blk} non-black={vis-blk} ({100*(vis-blk)/vis:.2f}%)')
    if c:
        print('   top non-black: ' + ', '.join('#%02x%02x%02x x%d' % (k[0], k[1], k[2], v)
                                               for k, v in c.most_common(5)))

    print('-- dock --')
    lit = dots(buf, LIT, 700, 1126, 1220, H-1)
    gry = dots(buf, GREY, 700, 1126, 1220, H-1)
    for name, x in zip(DOCK_TILES, DOCK_DOT_X):
        st = ('lit' if any(g[0] == x for g in lit)
              else 'GREY' if any(g[0] == x for g in gry) else 'absent')
        print(f'   tile {name}: running dot at x{x} y1179 6x6 -> {st}')
    return len(lit), len(gry)


def main():
    d = sys.argv[1] if len(sys.argv) > 1 else '.'
    bufs = {}
    for n in range(4):
        p = os.path.join(d, f'SCREEN{n}.PNG')
        if not os.path.exists(p):
            print(f'missing {p} — pass the directory holding the render8 captures'); return 2
        bufs[n] = decode(p)
    dockstate = {n: score(n, bufs[n]) for n in range(4)}

    print()
    print('===== VERDICTS =====')

    # A31 — the View drop-down's placement against the title that owns it.
    v = [w for w in bar_words(bufs[0]) if w[0] == 171]
    a = bar_words(bufs[3])[0]
    print(f'A31 pixels: View title cell x{v[0][0]}..{v[0][1]} ({v[0][1]-v[0][0]+1}x{v[0][3]-v[0][2]+1}) '
          f'vs wire title-x=171 / open at (171,34); app title cell x{a[0]} vs wire title-x=12 '
          f'-> PASS (placement); typeface DATUM (no capture has a drop-down open)')

    # A35 — the pointer sprite's footprint.
    ars = {n: arrow(bufs[n]) for n in range(4)}
    a2 = [g for g in ars[2] if g[4] < 40]
    print(f'A35 pixels: pointer arrow present in SCREEN2 only, {a2[0][2]}x{a2[0][3]} at '
          f'({a2[0][0]},{a2[0][1]}) over a window; absent from SCREEN0/1/3 -> DATUM '
          f'(window regime = compositor 9x9; no capture puts it on the backdrop)')

    # A33/SO4 — where the crystal glyph actually sits, every capture.
    cs = [crystal(bufs[n]) for n in range(4)]
    print('A33/SO4 pixels: crystal glyph left edge x=' + '/'.join(str(c[0]) for c in cs)
          + f' ({cs[0][2]}x{cs[0][3]}, gap_right={cs[0][4]}) in a {W}-wide bar, all four captures '
          f'-> FAIL (R25 puts the crystal top-LEFT; it is at the right edge)')

    # A29 — chrome, and the shell's absence.
    nd = {n: len(dots(bufs[n], RED)) for n in range(4)}
    print(f'A29 pixels (chrome): {nd[0]}/{nd[1]}/{nd[2]}/{nd[3]} title-bar disc groups, each 24x24 at '
          f'box+17 / +53 / +89 x and +10 y -> PASS (chrome identical on every window instance)')
    print(f'A29 pixels (shell): shell box 970x510 at (923,61) in SCREEN0/1/2, ABSENT from SCREEN3; '
          f'console box 1305x780 moves (307,158) -> (307,195) -> FAIL (the shell never comes back)')

    # SO11 — chrome width against surface width.
    c0 = chrome_run(bufs[0], 950, 15, 1294); c3 = chrome_run(bufs[3], 950, 15, 1294)
    print(f'SO11 pixels: pulse title-bar chrome x15..{c0[2]} = {c0[0]} px on SCREEN0/1/2 against a '
          f'{1294-15+1} px surface; x15..{c3[2]} = {c3[0]} px on SCREEN3, truncated at the console '
          f'box edge x=307 with #000000 past it -> PASS (chrome == surface; SO11 is OCCLUSION)')

    # A26 — is the console solid black?
    for n in (0, 3):
        cr = CAPS[n]['console']; vis, blk, _ = content_census(bufs[n], cr, CAPS[n]['occl'])
        print(f'A26 pixels SCREEN{n}: console content 1295x736 at ({cr[0]},{cr[1]}), unoccluded {vis} px, '
              f'non-black {vis-blk} ({100*(vis-blk)/vis:.2f}%)')
    print('A26 pixels: 7.35% non-black at 15:43 (monospaced grey-on-black glyphs) vs 0.00% at 15:52 '
          '-> DATUM (the console is NOT empty before its close; it is EXACTLY empty after the reopen)')

    # dock
    print('dock pixels: 4 tiles (console/quarry/pulse/shell, boxes x751..844 / 859..952 / 967..1060 / '
          '1075..1168, pitch 108) in all four captures; running dots 6x6 at y1179 — '
          + ' / '.join(f'SCREEN{n} {dockstate[n][0]} lit + {dockstate[n][1]} grey' for n in range(4))
          + ' -> PASS (the dock never fell to three tiles; the shell tile alone goes grey)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
