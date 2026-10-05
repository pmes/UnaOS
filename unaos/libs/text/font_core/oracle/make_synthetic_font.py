# FONTBIDI (SR56): a small synthetic test font exercising the lookup types the Noto fonts do not use under default
# features: GSUB 3 (alternate, first alternate chosen), GSUB 8 (reverse chained single), GPOS 3 (cursive, LTR),
# GPOS 1 (single) and contextual GPOS. Built from a DejaVu Sans subset with fontTools feaLib (test-time tooling
# only). Usage: python3 oracle/make_synthetic_font.py /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf tests/data/fontbidi_synth.ttf
import sys
from fontTools.ttLib import TTFont
from fontTools import subset
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString

src, dst = sys.argv[1], sys.argv[2]
opts = subset.Options()
opts.layout_features = []
opts.name_IDs = ['*']
opts.notdef_outline = True
opts.hinting = False
font = TTFont(src)
sub = subset.Subsetter(opts)
sub.populate(unicodes=list(range(0x20, 0x7F)))
sub.subset(font)
for t in ['GSUB', 'GPOS', 'GDEF', 'kern']:
    if t in font:
        del font[t]
fea = """
languagesystem DFLT dflt;
languagesystem latn dflt;
@LEFT = [a e];
@RIGHT = [b d];
feature calt {
    sub x from [y z];
    sub q by q u;
} calt;
feature liga {
    rsub @LEFT o' @RIGHT by u;
    rsub o' z by v;
    rsub i' [i j] by j;
} liga;
feature curs {
    pos cursive m <anchor NULL> <anchor 500 100>;
    pos cursive n <anchor 0 0> <anchor 600 -50>;
    pos cursive w <anchor 50 200> <anchor NULL>;
} curs;
feature kern {
    pos T 30;
    pos [A V] [V A]' -80 W;
    pos k l' 45;
} kern;
"""
addOpenTypeFeaturesFromString(font, fea)
font.save(dst)
