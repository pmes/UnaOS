#!/usr/bin/env python3
"""Regenerate tests/vectors.txt from a checkout of the resvg test suite (linebender/resvg, crates/resvg/tests).

    python3 oracle/make-vectors.py <resvg-checkout>/crates/resvg/tests <commit> > tests/vectors.txt

Each line: category  relpath  sha256  W  H   (W x H = the size of resvg's own reference PNG, the render size).
Font lines: font  fonts/<name>  sha256  0  0.
"""
import hashlib, os, struct, sys

CATS = [
    ("shapes", ["shapes/rect", "shapes/circle", "shapes/ellipse", "shapes/line", "shapes/polyline", "shapes/polygon"]),
    ("paths", ["shapes/path"]),
    ("painting", ["painting/fill", "painting/fill-opacity", "painting/fill-rule", "painting/stroke", "painting/stroke-dasharray",
                  "painting/stroke-dashoffset", "painting/stroke-linecap", "painting/stroke-linejoin", "painting/stroke-miterlimit",
                  "painting/stroke-opacity", "painting/stroke-width", "painting/opacity", "painting/color", "painting/paint-order",
                  "painting/visibility", "painting/display", "painting/shape-rendering", "painting/marker", "painting/context"]),
    ("gradients", ["paint-servers/linearGradient", "paint-servers/radialGradient", "paint-servers/stop", "paint-servers/stop-color",
                   "paint-servers/stop-opacity"]),
    ("patterns", ["paint-servers/pattern"]),
    ("text", ["text/text", "text/tspan", "text/text-anchor", "text/letter-spacing", "text/word-spacing", "text/font-size",
              "text/font-family", "text/font-weight", "text/font-style", "text/font", "text/text-decoration", "text/baseline-shift",
              "text/dominant-baseline", "text/font-kerning"]),
    ("clip-mask", ["masking/clip", "masking/clip-rule", "masking/clipPath", "masking/mask"]),
    ("use-symbol", ["structure/use", "structure/symbol", "structure/defs"]),
    ("structure", ["structure/g", "structure/svg", "structure/transform", "structure/style", "structure/style-attribute",
                   "structure/switch", "structure/systemLanguage", "structure/image", "structure/a"]),
] + [
    # SVGFILTERS (SR65): the filters/ category, one oracle category per primitive / property directory.
    (d, ["filters/" + d]) for d in [
        "filter", "filter-functions", "enable-background", "flood-color", "flood-opacity", "feBlend", "feColorMatrix",
        "feComponentTransfer", "feComposite", "feConvolveMatrix", "feDiffuseLighting", "feDisplacementMap",
        "feDistantLight", "feDropShadow", "feFlood", "feGaussianBlur", "feImage", "feMerge", "feMorphology", "feOffset",
        "fePointLight", "feSpecularLighting", "feSpotLight", "feTile", "feTurbulence"]
]

def png_size(p):
    with open(p, "rb") as f:
        h = f.read(24)
    return struct.unpack(">II", h[16:24])

root, commit = sys.argv[1], sys.argv[2]
print(f"# resvg test suite @ {commit}: https://raw.githubusercontent.com/linebender/resvg/{commit}/crates/resvg/tests/<relpath>")
for cat, dirs in CATS:
    for d in dirs:
        full = os.path.join(root, "tests", d)
        if not os.path.isdir(full):
            continue
        for dp, _, fns in sorted(os.walk(full)):
            for fn in sorted(fns):
                if not fn.endswith(".svg"):
                    continue
                p = os.path.join(dp, fn)
                ref = p[:-4] + ".png"
                if not os.path.exists(ref):
                    continue
                w, h = png_size(ref)
                rel = os.path.relpath(p, root)
                sha = hashlib.sha256(open(p, "rb").read()).hexdigest()
                print(f"{cat}\t{rel}\t{sha}\t{w}\t{h}")
for fn in sorted(os.listdir(os.path.join(root, "fonts"))):
    if fn.endswith((".ttf", ".otf")):
        p = os.path.join(root, "fonts", fn)
        print(f"font\tfonts/{fn}\t{hashlib.sha256(open(p, 'rb').read()).hexdigest()}\t0\t0")
