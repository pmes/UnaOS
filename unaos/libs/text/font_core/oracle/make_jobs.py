# FONTCORE oracle job list: test strings x fonts x sizes, keeping only (string, font) pairs the font covers
# completely (otherwise Chromium would fall back to another font). Usage: python3 make_jobs.py > jobs.tsv
from fontTools.ttLib import TTFont
FONTS = {
    "sans": "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "sansb": "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "serif": "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
    "mono": "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "lib": "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "libs": "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
    "loma": "/usr/share/fonts/opentype/tlwg/Loma.otf",
}
STRINGS = {
    "fox": "The quick brown fox jumps over the lazy dog.",
    "kern": "AVATAR Wave Toyota LT Yo P. F, To We Ty",
    "liga": "office affluent flight fjord ffi ffl",
    "sphinx": "Sphinx of black quartz, judge my vow! 0123456789",
    "greek": "Ξεσκεπάζω την ψυχοφθόρα βδελυγμία.",
    "cyr": "Съешь же ещё этих мягких французских булок, да выпей чаю.",
    "accent": "Ångström façade naïve œuvre Ærø — “quotes” & (brackets) 50% $9.99",
}
SIZES = [12, 16, 24, 48]
for fk, fp in FONTS.items():
    cmap = TTFont(fp).getBestCmap()
    for sk, s in STRINGS.items():
        if any(ord(c) not in cmap for c in s):
            continue
        for z in SIZES:
            print(f"{fk}_{sk}_{z}\t{fp}\t{z}\t{s}")
