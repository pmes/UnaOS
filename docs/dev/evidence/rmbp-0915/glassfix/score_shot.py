#!/usr/bin/env python3
"""GLASSFIX PNG scorer — SO2 on the glass (rmbp-0915).

Usage:  python3 score_shot.py desktop-menu-open.png <serial.log>

Scores the QMP screendump taken by `winmenu::shotmenu_selftest` against the three
claims SO2 makes, reading its anchors from that fixture's own witness lines so the
coordinates are the kernel's, never this script's guesses.

Reads the QMP screendump and the `[winmenu] shotmenu ...` / `[winmenu] drop ...` witness
lines, then MEASURES on the pixels:

  1. the drop-down's painted left edge vs the bar caption's glyph cell origin (title_x),
  2. the drop-down's painted top edge vs the bar's bottom row,
  3. the ink-coverage mask of the app name `Glass` where the BAR draws it against the
     mask of the same string where the DROP-DOWN draws it. Same face + same weight =>
     identical masks, whatever the two backgrounds are.

Pure stdlib: zlib + struct, no PIL.
"""
import sys, zlib, struct, re


def read_png(path):
    d = open(path, 'rb').read()
    assert d[:8] == b'\x89PNG\r\n\x1a\n', 'not a PNG'
    pos, idat, w = 8, b'', None
    while pos < len(d):
        ln, typ = struct.unpack('>I4s', d[pos:pos + 8])
        body = d[pos + 8:pos + 8 + ln]
        if typ == b'IHDR':
            w, h, depth, ctype, comp, filt, inter = struct.unpack('>IIBBBBB', body)
            assert depth == 8 and ctype in (2, 6) and inter == 0, (depth, ctype, inter)
            nch = 3 if ctype == 2 else 4
        elif typ == b'IDAT':
            idat += body
        elif typ == b'IEND':
            break
        pos += 12 + ln
    raw = zlib.decompress(idat)
    stride = w * nch
    out, prev, p = [], bytearray(stride), 0
    for _ in range(h):
        f = raw[p]; p += 1
        line = bytearray(raw[p:p + stride]); p += stride
        if f == 1:
            for i in range(nch, stride):
                line[i] = (line[i] + line[i - nch]) & 0xFF
        elif f == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif f == 3:
            for i in range(stride):
                a = line[i - nch] if i >= nch else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif f == 4:
            for i in range(stride):
                a = line[i - nch] if i >= nch else 0
                b = prev[i]
                c = prev[i - nch] if i >= nch else 0
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        out.append(bytes(line)); prev = line
    px = [[tuple(row[x * nch:x * nch + 3]) for x in range(w)] for row in out]
    return w, h, px


def modal(vals):
    c = {}
    for v in vals:
        c[v] = c.get(v, 0) + 1
    return max(c.items(), key=lambda kv: kv[1])[0]


def mask(px, x0, y0, w, h):
    """Ink mask: a pixel is ink when it differs from the block's modal background."""
    block = [px[y0 + j][x0 + i] for j in range(h) for i in range(w)]
    bg = modal(block)
    def far(p):
        return sum(abs(a - b) for a, b in zip(p, bg)) > 24
    return [[1 if far(px[y0 + j][x0 + i]) else 0 for i in range(w)] for j in range(h)], bg


def render(m):
    return '\n'.join(''.join('#' if v else '.' for v in row) for row in m)




def probe_edges(px, w, h, desk, mx, mw, my, title_x, bar_h):
    """The drop-down's PAINTED left and top edge.

    The left-edge row must be taken from the menu's TOP BAND (`my + 2`): lower rows
    cross the fixture window's own chrome, which starts at x=12 and carries the red
    close disc at x=29..38 — a naive scan of the menu's mid-row reports x=12 and a
    spurious dx=-28. That was this scorer's own first reading, and it is the reason
    the probe row is pinned here rather than at mh/2.
    """
    out = {}
    row = my + 2
    xs = [x for x in range(w) if px[row][x] != desk]
    out['left'] = min(xs)
    out['left_row'] = row
    out['dx'] = out['left'] - title_x
    col = mx + mw // 2
    ys = [y for y in range(bar_h, h) if px[y][col] != desk]
    out['top'] = min(ys)
    out['top_col'] = col
    out['dy'] = out['top'] - bar_h
    return out


def alpha_map(px, x0, y0, gw, gh):
    """Per-pixel ink COVERAGE, recovered from the blend.

    A fixed RGB threshold cannot compare these two blocks: the bar draws white ink on
    the lit-title blue and the menu draws near-black on chrome face, so the same glyph
    at the same weight lands at different RGB distances and a threshold clips the two
    differently (it read 6 px of 900 "different", all of them on the `s` bowls, and all
    of them thresholding rather than type). Projecting each pixel onto the bg->ink axis
    recovers the alpha the rasteriser actually emitted, which IS the face+weight.
    """
    blk = [px[y0 + j][x0 + i] for j in range(gh) for i in range(gw)]
    bg = modal(blk)
    fg = max(blk, key=lambda p: sum(abs(a - b) for a, b in zip(p, bg)))
    den = sum((f - b) ** 2 for f, b in zip(fg, bg)) or 1
    m = [[max(0.0, min(1.0, sum((p - b) * (f - b)
                                for p, f, b in zip(px[y0 + j][x0 + i], fg, bg)) / den))
          for i in range(gw)] for j in range(gh)]
    return m, bg, fg


def main():
    png, serial = sys.argv[1], sys.argv[2]
    txt = open(serial, 'rb').read().decode('utf-8', 'replace')
    ms = re.search(r'\[winmenu\] shotmenu name=(\S+) cell=(\d+)x(\d+) '
                   r'bar_glyph=\((\d+),(\d+)\) menu_glyph=\((\d+),(\d+)\) '
                   r'drop=(\d+)x(\d+)\+(\d+)\+(\d+)', txt)
    md = re.search(r'\[winmenu\] drop x=(\d+) title_x=(\d+) y=(\d+) bar_h=(\d+) '
                   r'font=(\S+) -> (\S+)', txt)
    if not ms or not md:
        print('FAIL: shotmenu witness lines absent from', serial)
        return 2
    name, cw, ch = ms.group(1), int(ms.group(2)), int(ms.group(3))
    bgx, bgy = int(ms.group(4)), int(ms.group(5))
    mgx, mgy = int(ms.group(6)), int(ms.group(7))
    mw, mh, mx, my = (int(ms.group(i)) for i in (8, 9, 10, 11))
    title_x, bar_h, font, verdict = (int(md.group(2)), int(md.group(4)),
                                     md.group(5), md.group(6))
    w, h, px = read_png(png)
    desk = px[400][900]
    print(f'PNG {w}x{h}  desktop bg {desk}')
    print(f'wire: drop={mw}x{mh}+{mx}+{my} title_x={title_x} bar_h={bar_h} '
          f'font={font} -> {verdict}')

    e = probe_edges(px, w, h, desk, mx, mw, my, title_x, bar_h)
    print(f'[px] drop LEFT edge  x={e["left"]} (row y={e["left_row"]}; '
          f'x={e["left"]-1} is desktop)  title glyph cell x={title_x}  dx={e["dx"]}')
    print(f'[px] drop TOP  edge  y={e["top"]} (col x={e["top_col"]}; '
          f'y={bar_h-1} is bar)  bar bottom={bar_h}  dy={e["dy"]}')

    gw = len(name) * cw
    A, abg, afg = alpha_map(px, bgx, bgy, gw, ch)
    B, bbg, bfg = alpha_map(px, mgx, mgy, gw, ch)
    print(f'[px] "{name}" BAR  ({bgx},{bgy}) {gw}x{ch} bg={abg} ink={afg}')
    print(f'[px] "{name}" MENU ({mgx},{mgy}) {gw}x{ch} bg={bbg} ink={bfg}')
    d = [abs(A[j][i] - B[j][i]) for j in range(ch) for i in range(gw)]
    over = sum(1 for v in d if v > 0.25)
    inkA = sum(1 for j in range(ch) for i in range(gw) if A[j][i] > 0.5)
    inkB = sum(1 for j in range(ch) for i in range(gw) if B[j][i] > 0.5)
    print(f'[px] alpha delta max={max(d):.3f} mean={sum(d)/len(d):.4f} '
          f'>0.25: {over} of {len(d)}')
    print(f'[px] ink px (alpha>0.5) bar={inkA} menu={inkB} delta={inkB-inkA}')
    for j in range(ch):
        ra = ''.join(str(int(A[j][i] * 9.999)) for i in range(gw))
        rb = ''.join(str(int(B[j][i] * 9.999)) for i in range(gw))
        if ra.strip('0') or rb.strip('0'):
            print(f'  bar  {ra}')
            print(f'  menu {rb}')
    ok = e['dx'] == 0 and e['dy'] == 0 and over == 0 and inkA == inkB and inkA > 0
    print(f':: GLASSFIX-PX: dx={e["dx"]} dy={e["dy"]} alpha_over_0.25={over} '
          f'ink_delta={inkB-inkA} :: {"PASS" if ok else "FAIL"} ::')
    return 0 if ok else 1


sys.exit(main())
