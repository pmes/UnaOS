#!/usr/bin/env python3
"""FONTHINT (SR62): generate src/hint/autofit_tables.rs — the script / style / blue-string / Unicode-range DATA
the light autohinter needs, read from a FreeType 2.13.2 source tree (src/autofit/{afblue.dat,afscript.h,
afranges.c,afstyles.h}). Only data crosses over (character strings, code-point ranges, style order); the algorithm
is font_core's own Rust (src/hint/autofit.rs). Usage:

    gen_autofit.py <freetype2-source-root> > src/hint/autofit_tables.rs

The source used is the `freetype2/` tree of the freetype-sys 0.20.1 crate (FreeType 2.13.2); see oracle/vectors.txt.
"""
import re, sys, os

root = sys.argv[1]
af = os.path.join(root, 'src', 'autofit')

def strip_c_comments(s):
    return re.sub(r'/\*.*?\*/', '', s, flags=re.S)

# ---- afblue.dat: strings and stringsets
dat = open(os.path.join(af, 'afblue.dat'), encoding='utf-8').read().split('\n')
strings, stringsets = {}, {}
section = None; cur = None
for line in dat:
    t = line.strip()
    if t.startswith('//') or t.startswith('#') or not t:
        continue
    if t.endswith(':') and len(t.split()) == 3:
        section = t.split()[0]; cur = None; continue
    if section == 'AF_BLUE_STRING_ENUM':
        if t.startswith('"'):
            strings[cur] += t[1:-1]
        else:
            cur = t; strings[cur] = ''
    elif section == 'AF_BLUE_STRINGSET_ENUM':
        if t.startswith('{'):
            stringsets[cur].append(t)
        elif t.startswith('AF_BLUE_PROPERTY') or t.startswith('|'):
            stringsets[cur][-1] += ' ' + t
        else:
            cur = t; stringsets[cur] = []
props = {'AF_BLUE_PROPERTY_LATIN_TOP': 1, 'AF_BLUE_PROPERTY_LATIN_SUB_TOP': 2, 'AF_BLUE_PROPERTY_LATIN_NEUTRAL': 4,
         'AF_BLUE_PROPERTY_LATIN_X_HEIGHT': 8, 'AF_BLUE_PROPERTY_LATIN_LONG': 16,
         'AF_BLUE_PROPERTY_CJK_TOP': 1, 'AF_BLUE_PROPERTY_CJK_HORIZ': 2, 'AF_BLUE_PROPERTY_CJK_RIGHT': 1}
sets = {}
for name, recs in stringsets.items():
    out = []
    for r in recs:
        body = ' '.join(r.split())
        m = re.match(r'\{\s*(\w+)\s*,\s*(.*?)\s*\}', body)
        sname, p = m.group(1), m.group(2)
        if sname == 'AF_BLUE_STRING_MAX':
            break
        v = 0
        for tok in re.findall(r'\w+', p):
            if tok != '0':
                v |= props[tok]
        out.append((strings[sname], v))
    sets[name] = out

# ---- afscript.h
scr = strip_c_comments(open(os.path.join(af, 'afscript.h'), encoding='latin-1').read())
scripts = []
for m in re.finditer(r'SCRIPT\(\s*(\w+)\s*,\s*(\w+)\s*,\s*"[^"]*"\s*,\s*(\w+)\s*,\s*(\w+)\s*,\s*((?:"[^"]*"\s*)+)\)', scr):
    s, S, hb, h = m.group(1), m.group(2), m.group(3), m.group(4)
    raw = ''.join(re.findall(r'"([^"]*)"', m.group(5)))
    std = bytes(raw, 'latin-1').decode('unicode_escape').encode('latin-1').decode('utf-8')
    scripts.append((s, S, hb, h == 'HINTING_TOP_TO_BOTTOM', std))
script_index = {S: i for i, (s, S, hb, t, std) in enumerate(scripts)}

# ---- afranges.c
rng = strip_c_comments(open(os.path.join(af, 'afranges.c'), encoding='latin-1').read())
ranges = {}
for m in re.finditer(r'af_(\w+?)_(nonbase_)?uniranges\[\]\s*=\s*\{(.*?)\};', rng, flags=re.S):
    key = (m.group(1), bool(m.group(2)))
    ranges[key] = [(int(a, 16), int(b, 16)) for a, b in re.findall(r'AF_UNIRANGE_REC\(\s*(0x[0-9A-Fa-f]+|0)\s*,\s*(0x[0-9A-Fa-f]+|0)\s*\)', m.group(3).replace('(       0', '(0x0').replace('(      0', '(0x0'))
                   if int(a, 16) != 0]

# ---- afstyles.h (expanded in order)
sty = strip_c_comments(open(os.path.join(af, 'afstyles.h'), encoding='latin-1').read())
sty = re.sub(r'#define(?:[^\n]*\\\n)*[^\n]*', '', sty)
latin_cov = ['PETITE_CAPITALS_FROM_CAPITALS', 'SMALL_CAPITALS_FROM_CAPITALS', 'ORDINALS', 'PETITE_CAPITALS',
             'SCIENTIFIC_INFERIORS', 'SMALL_CAPITALS', 'SUBSCRIPT', 'SUPERSCRIPT', 'TITLING', 'DEFAULT']
cov_feat = {'PETITE_CAPITALS_FROM_CAPITALS': ['c2pc'], 'SMALL_CAPITALS_FROM_CAPITALS': ['c2sc'],
            'ORDINALS': ['ordn'], 'PETITE_CAPITALS': ['pcap'], 'SCIENTIFIC_INFERIORS': ['sinf'],
            'SMALL_CAPITALS': ['smcp'], 'SUBSCRIPT': ['subs'], 'SUPERSCRIPT': ['sups'], 'TITLING': ['titl'], 'DEFAULT': []}
styles = []
tok = re.finditer(r'META_STYLE_LATIN\(\s*(\w+)\s*,\s*(\w+)\s*,[^)]*\)|STYLE_DEFAULT_INDIC\(\s*(\w+)\s*,\s*(\w+)\s*,[^)]*\)|'
                  r'STYLE\(\s*(\w+)\s*,\s*\w+\s*,\s*"[^"]*"\s*,\s*AF_WRITING_SYSTEM_(\w+)\s*,\s*AF_SCRIPT_(\w+)\s*,\s*'
                  r'(?:AF_BLUE_STRINGSET_(\w+)|\(AF_Blue_Stringset\)0)\s*,\s*AF_COVERAGE_(\w+)\s*\)', sty)
for m in tok:
    if m.group(1):
        for c in latin_cov:
            styles.append((m.group(1) + '_' + {'PETITE_CAPITALS_FROM_CAPITALS': 'c2cp', 'SMALL_CAPITALS_FROM_CAPITALS': 'c2sc', 'ORDINALS': 'ordn', 'PETITE_CAPITALS': 'pcap', 'SCIENTIFIC_INFERIORS': 'sinf', 'SMALL_CAPITALS': 'smcp', 'SUBSCRIPT': 'subs', 'SUPERSCRIPT': 'sups', 'TITLING': 'titl', 'DEFAULT': 'dflt'}[c], 'LATIN', m.group(2), m.group(2), c))
    elif m.group(3):
        styles.append((m.group(3) + '_dflt', 'INDIC', m.group(4), None, 'DEFAULT'))
    else:
        styles.append((m.group(5), m.group(6), m.group(7), m.group(8), m.group(9)))

# HarfBuzz script -> OpenType script tags (hb_ot_tags_from_script_and_language, the cases the table needs)
hb2ot = {'HB_SCRIPT_LATIN': ['latn'], 'HB_SCRIPT_CYRILLIC': ['cyrl'], 'HB_SCRIPT_GREEK': ['grek'],
         'HB_SCRIPT_HEBREW': ['hebr'], 'HB_SCRIPT_ARABIC': ['arab'], 'HB_SCRIPT_THAI': ['thai'],
         'HB_SCRIPT_ARMENIAN': ['armn'], 'HB_SCRIPT_GEORGIAN': ['geor'], 'HB_SCRIPT_DEVANAGARI': ['dev2', 'deva'],
         'HB_SCRIPT_BENGALI': ['bng2', 'beng'], 'HB_SCRIPT_GUJARATI': ['gjr2', 'gujr'], 'HB_SCRIPT_GURMUKHI': ['gur2', 'guru'],
         'HB_SCRIPT_KANNADA': ['knd2', 'knda'], 'HB_SCRIPT_MALAYALAM': ['mlm2', 'mlym'], 'HB_SCRIPT_TAMIL': ['tml2', 'taml'],
         'HB_SCRIPT_TELUGU': ['tel2', 'telu'], 'HB_SCRIPT_ORIYA': ['ory2', 'orya'], 'HB_SCRIPT_MYANMAR': ['mym2', 'mymr'],
         'HB_SCRIPT_KHMER': ['khmr'], 'HB_SCRIPT_LAO': ['lao '], 'HB_SCRIPT_HAN': ['hani'], 'HB_SCRIPT_ETHIOPIC': ['ethi'],
         'HB_SCRIPT_SINHALA': ['sinh'], 'HB_SCRIPT_TIBETAN': ['tibt'], 'HB_SCRIPT_MONGOLIAN': ['mong']}

def rs(s):
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"') + '"'

o = []
o.append('// @generated by oracle/gen_autofit.py from FreeType 2.13.2 src/autofit (afblue.dat, afscript.h, afranges.c,')
o.append('// afstyles.h): DATA only — blue strings, standard characters, Unicode ranges, style order. Do not edit.')
o.append('#![allow(clippy::all)]')
o.append('use super::autofit::{Coverage, ScriptRec, StyleRec, WritingSystem};')
o.append('')
o.append('pub static SCRIPTS: &[ScriptRec] = &[')
for s, S, hb, ttb, std in scripts:
    rb = ranges.get((s, False), []); nb = ranges.get((s, True), [])
    tags = hb2ot.get(hb, [])
    o.append(f'    ScriptRec {{ name: {rs(s)}, top_to_bottom: {str(ttb).lower()}, standard: {rs(std)}, ot_tags: &[{", ".join(rs(t) for t in tags)}],')
    o.append(f'        ranges: &[{", ".join(f"(0x{a:04X}, 0x{b:04X})" for a, b in rb)}],')
    o.append(f'        nonbase: &[{", ".join(f"(0x{a:04X}, 0x{b:04X})" for a, b in nb)}] }},')
o.append('];')
o.append('')
o.append('pub static STYLES: &[StyleRec] = &[')
for name, ws, S, bss, cov in styles:
    blues = sets.get('AF_BLUE_STRINGSET_' + bss, []) if bss else []
    feats = cov_feat.get(cov, [])
    covv = 'Coverage::Default' if cov == 'DEFAULT' else 'Coverage::Features(&[' + ', '.join(rs(f) for f in feats) + '])'
    ws_v = {'LATIN': 'WritingSystem::Latin', 'CJK': 'WritingSystem::Cjk', 'INDIC': 'WritingSystem::Indic', 'DUMMY': 'WritingSystem::Dummy'}[ws]
    o.append(f'    StyleRec {{ name: {rs(name)}, ws: {ws_v}, script: {script_index[S]}, coverage: {covv},')
    o.append(f'        blues: &[{", ".join(f"({rs(st)}, {p})" for st, p in blues)}] }},')
o.append('];')
idx = {n: i for i, (n, *_r) in enumerate(styles)}
o.append(f'pub const STYLE_NONE_DFLT: usize = {idx["none_dflt"]};')
o.append(f'pub const SCRIPT_LATN: usize = {script_index["LATN"]};')
print('\n'.join(o))
