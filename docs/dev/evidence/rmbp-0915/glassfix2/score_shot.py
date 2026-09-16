#!/usr/bin/env python3
"""GLASSFIX2 PNG scorer — SO5 and SO12/S15 on the glass (rmbp-0915).

Usage:  python3 score_shot.py <desk.png> <serial.log>

Extends GLASSFIX's `score_shot.py` (SO2's typeface half, kept verbatim as leg A so one
script scores the whole desktop-glass arc) with the two rows GLASSFIX2 was cut for.
Every anchor is read from a kernel witness line or re-derived from the PNG's own
geometry; no coordinate in this file is a guess.

  leg A  SO2 (inherited) — the drop-down's left/top edge against the bar caption's glyph
         cell, and the two ink-coverage masks of the app name. Runs only when the
         `[winmenu] shotmenu` lines are on the wire; SKIPs, never FAILs, when they are not.

  leg B  SO12 / S15 on the PIXELS — the CLOSE-DISC CENSUS. SO12's operator-facing
         complaint was that a covered title bar makes "the pulse's close disc and drag
         handle unreachable", so the test is exactly that: find every macOS-hue close
         disc (`theme::CTRL_CLOSE` = #FF5F57, knurled and anti-aliased, so a tolerance
         band rather than an equality) as a 4-connected component, and walk its
         `theme::CONTROL_BOX` = 24 px span for the first pixel that is neither the disc
         nor title-bar chrome.

         **A DISC MAY BE COVERED BY A MENU AND MAY NOT BE COVERED BY A WINDOW**, and this
         leg distinguishes the two rather than counting clipped pixels. A Mac's open
         drop-down covers whatever is under it and the next click dismisses it; a window
         the desktop OPENED AT BOOT over another window's title bar is SO12, and nothing
         dismisses it. So a cover is scored against the open drop-down's own rect, read
         off `[winmenu] shotmenu drop=WxH+X+Y` — inside it the verdict is
         COVERED-BY-MENU and allowed, outside it the verdict is COVERED-BY-WINDOW and
         the leg FAILs. This distinction is not a refinement: `shotmenu_selftest` HOLDS a
         drop-down open across this very screendump (that is what GLASSFIX cut it for),
         and the first cut of this leg red-lined on it.

  leg C  SO5 re-derived — the kernel's `:: GLASSFIX2:` line states the sprite's extent
         over the backdrop and over a window. This leg recomputes both from the PNG's own
         height (`ui::Metrics::for_height`: scale = clamp(h/900, 1, 4); compositor side =
         (BASE_CELL + 1)*s; PAL back-buffer extent = 9*(s + 1)) and refuses the wire's
         numbers if they disagree. It is an INDEPENDENT arithmetic, not a re-read: a
         witness that printed its own constant would pass its own test.

         The sprite itself cannot be photographed at `UNAOS_QMP_WAIT`: `pal::cursor`
         auto-hides `HIDE_AFTER_MS` = 1500 ms after the last pointer report, and the shot
         fires ~85 s after the last fixture that moves the pointer. That is why SO5 is
         scored by the compositor's own per-surface arithmetic and not by a white blob —
         stated here rather than left for the next reader to rediscover.

  leg D  the two standing cascade witnesses must still be green on this boot:
         `[deskcascade] fit ... -> FIT` (CASCADEFIT, `9b1a0605`) and
         `[wm] tile-fit ... alias=none -> DISTINCT` (TILEFIT).

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



# ── GLASSFIX2 constants, lifted from the kernel so a theme change red-lines this file ─────────
CTRL_CLOSE = (0xFF, 0x5F, 0x57)   # video/theme.rs CTRL_CLOSE — macOS close hue, knurled
CONTROL_BOX = 24                  # video/theme.rs CONTROL_BOX
BASE_CELL = 8                     # ui.rs BASE_CELL
SCALE_STEP, SCALE_MAX = 900, 4    # ui.rs SCALE_STEP / SCALE_MAX


def metrics_scale(height):
    """`ui::Metrics::for_height(height).scale`, re-derived — leg C's independence."""
    return max(1, min(SCALE_MAX, height // SCALE_STEP))


def close_discs(px, w, h, tol=26):
    """Every close disc on the panel, as 4-connected components of the CTRL_CLOSE hue.

    `knurl` mills a diamond crosshatch into the disc at a 2 %-of-a-channel budget and the
    red channel is already saturated, so the crest CLIPS and the texture reads as its
    trough alone (theme.rs's own note). A tolerance band is therefore the only correct
    test; equality would find the crosshatch's troughs and miss its crests.
    """
    def near(p):
        return all(abs(a - b) <= tol for a, b in zip(p, CTRL_CLOSE))
    seen = [[False] * w for _ in range(h)]
    out = []
    for y in range(h):
        row = px[y]
        for x in range(w):
            if seen[y][x] or not near(row[x]):
                continue
            stack, comp = [(x, y)], []
            seen[y][x] = True
            while stack:
                cx, cy = stack.pop()
                comp.append((cx, cy))
                for nx, ny in ((cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)):
                    if 0 <= nx < w and 0 <= ny < h and not seen[ny][nx] and near(px[ny][nx]):
                        seen[ny][nx] = True
                        stack.append((nx, ny))
            if len(comp) < 32:          # knurl speckle / AA fringe of something else
                continue
            xs = [c[0] for c in comp]; ys = [c[1] for c in comp]
            out.append({
                'x': min(xs), 'y': min(ys),
                'w': max(xs) - min(xs) + 1, 'h': max(ys) - min(ys) + 1,
                'area': len(comp),
            })
    out.sort(key=lambda d: (d['y'], d['x']))
    return out


def cover_x(px, w, d, span=CONTROL_BOX):
    """The first x across this disc's control-box span that is neither the disc nor title
    chrome, or `None` when the whole span is the window's own title bar.

    The probe row is the disc's VERTICAL MIDDLE — the widest row of a circle, so it is the
    row a cover must cross. `theme::FRAME_LINE` (#B4B4B9) is deliberately NOT chrome here:
    a frame line inside a title bar's own span is the EDGE OF SOMETHING ELSE, which is the
    thing being looked for.
    """
    y = d['y'] + d['h'] // 2
    if y >= len(px):
        return None
    for x in range(d['x'], min(d['x'] + span, w)):
        p = px[y][x]
        near_disc = all(abs(a - b) <= 26 for a, b in zip(p, CTRL_CLOSE))
        chrome = min(p) >= 0xD8 and max(p) - min(p) <= 0x10
        if not (near_disc or chrome):
            return x
    return None


def main():
    png, serial = sys.argv[1], sys.argv[2]
    txt = open(serial, 'rb').read().decode('utf-8', 'replace')
    ms = re.search(r'\[winmenu\] shotmenu name=(\S+) cell=(\d+)x(\d+) '
                   r'bar_glyph=\((\d+),(\d+)\) menu_glyph=\((\d+),(\d+)\) '
                   r'drop=(\d+)x(\d+)\+(\d+)\+(\d+)', txt)
    md = re.search(r'\[winmenu\] drop x=(\d+) title_x=(\d+) y=(\d+) bar_h=(\d+) '
                   r'font=(\S+) -> (\S+)', txt)
    w, h, px = read_png(png)
    print(f'PNG {w}x{h}')
    rc = 0
    if not ms or not md:
        print('[A] SO2 leg SKIP — no `[winmenu] shotmenu` lines on this wire '
              '(this capture is not a menu-open shot)')
    else:
        rc |= so2_leg(px, w, h, ms, md)
    return rc | glassfix2_legs(px, w, h, txt)


def so2_leg(px, w, h, ms, md):
    name, cw, ch = ms.group(1), int(ms.group(2)), int(ms.group(3))
    bgx, bgy = int(ms.group(4)), int(ms.group(5))
    mgx, mgy = int(ms.group(6)), int(ms.group(7))
    mw, mh, mx, my = (int(ms.group(i)) for i in (8, 9, 10, 11))
    title_x, bar_h, font, verdict = (int(md.group(2)), int(md.group(4)),
                                     md.group(5), md.group(6))
    desk = px[400][900]
    print(f'[A] desktop bg {desk}')
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


def glassfix2_legs(px, w, h, txt):
    """Legs B, C and D — SO12/S15 on the pixels, SO5 re-derived, the standing witnesses."""
    ok = True

    # ---- leg B: the close-disc census -------------------------------------------------------
    drop = re.search(r'\[winmenu\] shotmenu .*?drop=(\d+)x(\d+)\+(\d+)\+(\d+)', txt)
    dr = tuple(int(drop.group(i)) for i in (1, 2, 3, 4)) if drop else None
    print(f'[B] open drop-down rect from the wire: '
          f'{"%dx%d+%d+%d" % dr if dr else "none (no menu held open on this boot)"}')
    discs = close_discs(px, w, h)
    covered_by_window = []
    for d in discs:
        cx = cover_x(px, w, d)
        if cx is None:
            verdict, by = 'WHOLE', '-'
        elif dr and dr[2] <= cx < dr[2] + dr[0] and dr[3] <= d['y'] < dr[3] + dr[1]:
            verdict, by = 'COVERED-BY-MENU', f'drop-down at x={dr[2]}'
        else:
            verdict, by = 'COVERED-BY-WINDOW', f'surface at x={cx} {px[d["y"] + 2][cx]}'
            covered_by_window.append(d)
        print(f"[B] close disc at ({d['x']},{d['y']}) painted {d['w']}x{d['h']} of "
              f"{CONTROL_BOX}x{CONTROL_BOX} area={d['area']} cover_x={cx} -> {verdict} {by}")
    print(f'[B] discs={len(discs)} covered_by_window={len(covered_by_window)}')
    if not discs:
        print('[B] FAIL — no close disc on the panel: no window title bar reached the glass')
        ok = False
    if covered_by_window:
        ok = False

    # ---- leg C: SO5, re-derived from the PNG's own geometry ---------------------------------
    line = re.search(r':: GLASSFIX2: (.*?) -> (\w+) ::', txt)
    if not line:
        print('[C] FAIL — no `:: GLASSFIX2:` line on the wire')
        return 1
    body, verdict = line.group(1), line.group(2)
    # Field-by-field, so a field ADDED to the witness does not silently stop this leg from
    # scoring the ones it cares about — a whole-line regex did exactly that on the first cut.
    def field(name, default=None):
        m = re.search(name + r'=(\d+)', body)
        if m is None and default is None:
            raise SystemExit(f'[C] FAIL — the GLASSFIX2 line carries no `{name}=` field')
        return int(m.group(1)) if m else default
    same = field('same')
    backdrop = field(r'backdrop')
    window = field(r'window')
    scale = field('scale')
    owns = field('owns_paint')
    comp_w = field('compositor')
    back_w = field('backbuffer')
    overlaps = field('overlaps')
    minted = field('minted', -1)
    nwin = field(r'\bn')
    control = field('control', -1)
    pw, ph = (int(v) for v in re.search(r'panel=(\d+)x(\d+)', body).groups())
    poc = field('pulse_over_console')
    if (pw, ph) != (w, h):
        print(f"[C] FAIL — the wire's panel {pw}x{ph} is not the PNG's {w}x{h}")
        ok = False
    s_px = metrics_scale(h)
    comp_px = (BASE_CELL + 1) * s_px
    back_px = 9 * (s_px + 1)
    exp_backdrop = comp_px if owns == 1 else back_px
    print(f'[C] re-derived from the PNG: h={h} -> scale={s_px} compositor={comp_px}x{comp_px} '
          f'backbuffer={back_px}x{back_px} owns_paint={owns} '
          f'=> over-backdrop {exp_backdrop}, over-window {comp_px}')
    print(f'[C] the wire said:           scale={scale} compositor={comp_w}x{comp_w} '
          f'backbuffer={back_w}x{back_w} '
          f'=> over-backdrop {backdrop}, over-window {window}, same={same}')
    agree = (scale == s_px and comp_w == comp_px and back_w == back_px
             and backdrop == exp_backdrop and window == comp_px
             and (same == 1) == (exp_backdrop == comp_px))
    print(f'[C] independent arithmetic {"AGREES" if agree else "DISAGREES"} with the witness')
    if not agree or same != 1:
        ok = False
    # The fixture's own preconditions, asserted here rather than trusted: a scan over no rows and
    # a scan by a blind detector both report `overlaps=0`.
    print(f'[C] fixture preconditions: minted={minted}/4 rows_scanned={nwin} '
          f'armed_control={control}')
    if minted != 4 or nwin < 2 or control != 1:
        print('[C] FAIL — the cascade scan had no rows to scan, or its armed control did not fire')
        ok = False

    # ---- leg D: the standing cascade witnesses ----------------------------------------------
    fit = re.findall(r'\[deskcascade\] fit .*?-> (\w+)', txt)
    tile = re.findall(r'\[wm\] tile-fit .*?alias=(\S+) -> (\w+)', txt)
    bad_fit = [v for v in fit if v != 'FIT']
    bad_tile = [(a, v) for a, v in tile if v != 'DISTINCT' or a != 'none']
    print(f'[D] [deskcascade] fit lines={len(fit)} verdicts={sorted(set(fit)) or "none"}'
          + ('' if fit else "  (this desktop opens no console window — `fbcon::CONSOLE_WIN` is "
                            "WIN_NONE, so the cap has no subject here; CASCADEFIT's rule is "
                            "measured on the aarch64 leg, see GLASSFIX2.md)"))
    print(f'[D] [wm] tile-fit lines={len(tile)} aliased={len(bad_tile)}')
    if bad_fit or bad_tile:
        ok = False

    print(f':: GLASSFIX2-PX: discs={len(discs)} covered_by_window={len(covered_by_window)} '
          f'sprite_backdrop={backdrop} sprite_window={window} same={same} '
          f'cascade_overlaps={overlaps} pulse_over_console={poc} windows={nwin} '
          f'minted={minted} control={control} '
          f'deskcascade={len(fit)}/{len(fit) - len(bad_fit)}FIT '
          f'tilefit={len(tile)}/{len(tile) - len(bad_tile)}DISTINCT wire={verdict} '
          f':: {"PASS" if ok and verdict == "PASS" else "FAIL"} ::')
    return 0 if (ok and verdict == 'PASS') else 1


sys.exit(main())
