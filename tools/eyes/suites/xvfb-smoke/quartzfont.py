#!/usr/bin/env python3
"""QUARTZFONT (LEDGER SR64) oracle: aether-shell's chrome text, as quartzite paints it with font_core, against
Chromium painting the SAME strings with the SAME face file at the SAME pen origins.

  python3 tools/eyes/suites/xvfb-smoke/quartzfont.py [--bin target/release/aether-shell] [--out DIR]

1. Runs the vessel on a private 640x400 Xvfb screen with QUARTZFONT_TRACE=1 and grabs the screen from Xvfb's
   own framebuffer (-fbdir, XWD), exactly as the EYES `xvfb` subject does. Each painted string arrives as a
   `[quartzfont]` stderr line: text, face file + index, px size, pen x / baseline in window px, ink, ascent,
   descent, and every glyph's pen and advance.
2. Writes one HTML page: per string, a box filled with the background the frame shows around that string
   (the mode of a 1 px ring around its glyph area) and the string in `@font-face { src: url(file://<face>) }`
   at the traced size, colour and pen origin (line-height = ascent + descent, so the baseline lands at
   top + ascent). A web font is rasterized unhinted by Chromium — the like-for-like path (AETHERFONT.md).
3. Screenshots it with tools/eyes/ref.mjs (the EYES Chromium flags: no hinting, grayscale AA, DSF 1).
4. Scores every inked glyph box (pen..pen+advance x ascent+descent) by the mean |luma difference|, after
   the best whole-pixel registration within +-2 px per string (reported; 0,0 is the expectation): the share
   within 8 levels is the result. Writes subject.png, ref.png and score.json in --out.
Exit 0 when at least 90 % of glyphs are within 8 levels, 1 below, 2 on a harness error (Chromium or Xvfb
absent).
"""
import json
import os
import signal
import subprocess
import sys
import time
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "../../../.."))
sys.path.insert(0, os.path.join(REPO, "tools/eyes"))
from metal import png_read, png_write  # noqa: E402  (stdlib PNG codec)

W, H = 640, 400


def xwd_rows(b):
    u = lambda i: int.from_bytes(b[i * 4:i * 4 + 4], "big")
    header, fmt, w, h = u(0), u(2), u(4), u(5)
    order, bpp, bpl = u(7), u(11), u(12)
    rm, gm, bm, ncolors = u(14), u(15), u(16), u(19)
    if fmt != 2 or bpp != 32:
        raise ValueError(f"unsupported xwd ({fmt}, {bpp})")
    data = header + ncolors * 12
    sh = lambda m: (m & -m).bit_length() - 1
    rows = []
    for y in range(h):
        row = bytearray()
        for x in range(w):
            o = data + y * bpl + x * 4
            v = int.from_bytes(b[o:o + 4], "little" if order == 0 else "big")
            row += bytes(((v & m) >> sh(m)) & 0xFF for m in (rm, gm, bm))
        rows.append(bytes(row))
    return w, h, rows


def grab(binary, out):
    disp = next(n for n in range(90, 200) if not os.path.exists(f"/tmp/.X11-unix/X{n}") and not os.path.exists(f"/tmp/.X{n}-lock"))
    fb = os.path.join(out, "fb")
    os.makedirs(fb, exist_ok=True)
    xvfb = subprocess.Popen(["Xvfb", f":{disp}", "-screen", "0", f"{W}x{H}x24", "-nolisten", "tcp", "-fbdir", fb],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        t0 = time.time()
        while not os.path.exists(f"/tmp/.X11-unix/X{disp}") and time.time() - t0 < 10:
            time.sleep(0.05)
        env = dict(os.environ, DISPLAY=f":{disp}", QUARTZFONT_TRACE="1")
        err = open(os.path.join(out, "stderr.txt"), "w")
        app = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=err)
        time.sleep(8)
        if app.poll() is not None:
            raise RuntimeError(f"aether-shell exited early ({app.returncode})")
        with open(os.path.join(fb, "Xvfb_screen0"), "rb") as f:
            raw = f.read()
        os.kill(app.pid, signal.SIGTERM)
        try:
            app.wait(5)
        except subprocess.TimeoutExpired:
            os.kill(app.pid, signal.SIGKILL)
        err.close()
    finally:
        os.kill(xvfb.pid, signal.SIGTERM)
        try:
            xvfb.wait(5)
        except subprocess.TimeoutExpired:
            os.kill(xvfb.pid, signal.SIGKILL)
    return xwd_rows(raw)


def traces(path):
    seen, out = set(), []
    for line in open(path, encoding="utf-8", errors="replace"):
        i = line.find("[quartzfont]\t")
        if i < 0:
            continue
        f = line[i:].rstrip("\n").split("\t")
        if len(f) < 12:
            continue
        t = dict(text=f[1].replace("\\t", "\t"), file=f[2], index=int(f[3] or 0), size=float(f[4]), x=float(f[5]),
                 base=float(f[6]), ink=f[7], alpha=int(f[8]), ascent=float(f[9]),
                 descent=float(f[10]), pens=[tuple(map(float, p.split(":"))) for p in f[11].split(",") if p])
        key = (t["text"], round(t["x"], 2), round(t["base"], 2))
        if key not in seen:  # the last paint of a string wins; repeats are the same frame
            seen.add(key)
            out.append(t)
    return out


def luma(rows, x, y):
    if not (0 <= x < W and 0 <= y < H):
        return 255
    r = rows[y]
    return (r[3 * x] * 299 + r[3 * x + 1] * 587 + r[3 * x + 2] * 114) // 1000


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def main(argv):
    binary = os.path.join(REPO, "target/release/aether-shell")
    out = os.path.join(REPO, "tools/eyes/out/quartzfont")
    if "--bin" in argv:
        binary = argv[argv.index("--bin") + 1]
    if "--out" in argv:
        out = argv[argv.index("--out") + 1]
    os.makedirs(out, exist_ok=True)
    try:
        w, h, subj = grab(binary, out)
    except Exception as e:  # noqa: BLE001
        print(f"quartzfont: harness: {e}")
        return 2
    png_write(os.path.join(out, "subject.png"), w, h, subj)
    ts = traces(os.path.join(out, "stderr.txt"))
    if not ts:
        print("quartzfont: no [quartzfont] trace lines (is the vessel built with --features gtk?)")
        return 2
    faces, boxes = {}, []
    for k, t in enumerate(ts):
        fam = faces.setdefault((t["file"], t["index"]), f"qf{len(faces)}")
        desc = t["descent"]
        x0 = int(t["x"] + min((p for p, _ in t["pens"]), default=0)) - 1
        x1 = int(t["x"] + max((p + a for p, a in t["pens"]), default=0)) + 2
        y0, y1 = int(t["base"] - t["ascent"]) - 1, int(t["base"] + desc) + 1
        ring = [subj[y][3 * x:3 * x + 3] for x in range(x0, x1) for y in (y0, y1) if 0 <= x < W and 0 <= y < H]
        ring += [subj[y][3 * x:3 * x + 3] for y in range(y0, y1) for x in (x0, x1) if 0 <= x < W and 0 <= y < H]
        bg = Counter(ring).most_common(1)[0][0] if ring else b"\xff\xff\xff"
        boxes.append((k, fam, x0, y0, x1, y1, "#%02x%02x%02x" % tuple(bg)))
    css = "".join(f"@font-face{{font-family:{f};src:url('file://{p}')}}\n" for (p, _), f in faces.items())
    body = []
    for (k, fam, x0, y0, x1, y1, bg), t in zip(boxes, ts):
        a = t["ascent"]
        lh = a + t["descent"]
        body.append(f'<div style="position:absolute;left:{x0}px;top:{y0}px;width:{x1 - x0}px;height:{y1 - y0}px;background:{bg}"></div>')
        rgba = f"rgba({int(t['ink'][1:3], 16)},{int(t['ink'][3:5], 16)},{int(t['ink'][5:7], 16)},{t['alpha'] / 255:.4f})"
        body.append(f'<div style="position:absolute;left:{t["x"]:.4f}px;top:{t["base"] - a:.4f}px;font:{t["size"]:.4f}px/{lh}px {fam};'
                    f'color:{rgba};white-space:pre">{esc(t["text"])}</div>')
    html = (f"<!DOCTYPE html><html><head><meta charset=utf-8><style>{css}html,body{{margin:0;background:#fff}}"
            f"div{{font-kerning:normal}}</style></head><body>{''.join(body)}</body></html>")
    page = os.path.join(out, "ref.html")
    open(page, "w").write(html)
    jobs = os.path.join(out, "jobs.tsv")
    open(jobs, "w").write(f"file://{page}\t{os.path.join(out, 'ref.png')}\t{W}\t{H}\n")
    env = dict(os.environ, PLAYWRIGHT_BROWSERS_PATH=os.environ.get("PLAYWRIGHT_BROWSERS_PATH", "/opt/pw-browsers"))
    if subprocess.run(["node", os.path.join(REPO, "tools/eyes/ref.mjs"), jobs], env=env).returncode != 0:
        print("quartzfont: harness: Chromium reference failed")
        return 2
    _, _, ref = png_read(os.path.join(out, "ref.png"))
    within = total = 0
    rows_out = []
    for (k, fam, x0, y0, x1, y1, bg), t in zip(boxes, ts):
        a, desc = t["ascent"], t["descent"]
        top, bot = int(t["base"] - a), int(t["base"] + desc)

        def err(dx, dy, gx0, gx1):
            n = s = 0
            for y in range(top, bot):
                for x in range(gx0, gx1):
                    s += abs(luma(subj, x, y) - luma(ref, x + dx, y + dy))
                    n += 1
            return s / max(n, 1)
        sx0, sx1 = x0 + 1, x1 - 1
        shift = min(((dx, dy) for dx in range(-2, 3) for dy in range(-2, 3)), key=lambda d: (err(d[0], d[1], sx0, sx1), abs(d[0]) + abs(d[1])))
        g_in = g_n = 0
        worst = 0.0
        for pen, adv in t["pens"]:
            gx0, gx1 = int(t["x"] + pen), int(t["x"] + pen + adv + 0.999)
            inked = any(abs(luma(subj, x, y) - luma(subj, sx0 - 1, top)) > 8 for y in range(top, bot) for x in range(gx0, gx1))
            if not inked:
                continue
            e = err(shift[0], shift[1], gx0, gx1)
            worst = max(worst, e)
            g_n += 1
            g_in += e <= 8
        within += g_in
        total += g_n
        rows_out.append(dict(text=t["text"], face=os.path.basename(t["file"]), size=round(t["size"], 4), x=t["x"], baseline=t["base"],
                             shift=shift, glyphs=g_n, within8=g_in, worst_mean_diff=round(worst, 2), background=bg))
        print(f"  {t['text']!r:24} {os.path.basename(t['file'])} {t['size']:.3f}px at ({t['x']:.2f},{t['base']:.2f}) "
              f"shift {shift}: {g_in}/{g_n} glyphs within 8 (worst mean |d| {worst:.2f})")
    share = within / max(total, 1)
    print(f"quartzfont: {within}/{total} glyphs within 8 levels of Chromium ({100 * share:.1f} %)")
    json.dump(dict(within8=within, glyphs=total, share=share, strings=rows_out), open(os.path.join(out, "score.json"), "w"), indent=1)
    return 0 if share >= 0.90 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
