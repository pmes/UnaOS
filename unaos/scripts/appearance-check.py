#!/usr/bin/env python3
# appearance-check.py — APPEARANCE (rmbp-ledger B408, MACPARITY rows 21/22): every colour the kernel's
# glass paints comes from the system palette (`crates/kernel/src/video/theme.rs`), never a literal.
# APPEARANCE2 (B473, GATEREVIEW F18): the gate sees every spelling a colour takes in this tree.
#
# PAINTERSCOPE (B482): the scope is every PAINTER, not one directory. A painter outside video/ is named in
# `appearance.painters` (`<path under crates/kernel/src>  <what it paints>`); a file outside video/ that calls a
# colour SINK (SINKS below: the FrameBuffer put/fill/line/scroll methods, the text rasteriser, the Pal trait) and is
# not listed is a finding (`unlisted painter`), as is a listed file that is gone. A literal suffixed u64/usize/i64/
# isize, or held by a const/static/let typed so, is an address or offset — the glass's pixel is a u32.
#
# HOW IT DECIDES. Scope = crates/kernel/src/video/**/*.rs except theme.rs, plus the listed painters. Comments and string/char
# literals are stripped first. A COLOUR LITERAL is any of:
#   (a) a hex literal, underscores dropped and any integer suffix (`u32`, `u64`, …) ignored, whose digits
#       are six (0xRRGGBB) or eight (0xAARRGGBB, ANY alpha byte) — `0x12_34_56`, `0x0012_3456u32`,
#       `0x8012_3456` all count, one by one, so a `[u32; N]` palette counts N;
#   (b) `u32::from_be_bytes([a, b, c, d])` / `from_le_bytes` / `from_ne_bytes` over four integer literals,
#       read as the packed word it builds (then judged as (a));
#   (c) in a `const` / `static` item typed `(u8, u8, u8[, u8])` or `[u8; 3|4]` (or a table of them), each
#       3- or 4-tuple / array of integer literals — one RGB(A) colour per group.
# NOT a colour: a BIT MASK (an operand of `&` / `^`, or an all-0/F literal written as an operand of `|`); a
# sentinel 0xFFFF_xxxx; and a FORMAT constant (a hash prime, a magic, an owner tag, an address) named in
# `appearance.allow` (`<path under video/>  <literal as written>  <reason>`, one row per file+literal; a row
# that no longer matches is STALE and fails the gate, so the list cannot rot).
# The count must equal `theme::AUDIT_LITERALS_OUTSIDE` (0), which the kernel's `tests appearance`
# witness prints as `literals_outside_theme=` — the committed constant is certified here.
#
# CONTROL PROBES (exit 2, no verdict, if any fails) run on every invocation; `--selftest` prints each plant.
#
# usage: appearance-check.py [<unaos dir>] [--list] [--selftest]   (exit 0 clean; 1 literals found, a
# stale allow row, an unlisted or missing painter, or the constant disagrees; 2 a control probe failed)
import os, re, sys

SUFFIX = r'(?:[ui](?:8|16|32|64|128|size))?'
HEX = re.compile(r'\b0[xX]([0-9A-Fa-f_]+?)([ui](?:8|16|32|64|128|size))?\b')
WIDE = ('u64', 'i64', 'usize', 'isize', 'u128', 'i128')
WIDE_ITEM = re.compile(r'\b(?:const|static(?:\s+mut)?|let(?:\s+mut)?)\s+[A-Za-z_][A-Za-z0-9_]*\s*:\s*(?:u64|i64|usize|isize|u128|i128)\s*=([^;]*);', re.S)
# The colour SINKS: a call to one of these names (not its `fn` definition) makes a file a painter.
SINKS = ('put_pixel', 'put_raw4', 'fill_span4', 'fill_rows', 'fill_screen', 'fill_rect', 'draw_line', 'scroll_up',
         'draw_text', 'draw_row', 'draw_glyph_fb', 'draw_with', 'draw_cell',
         'draw_pixel', 'clear_screen', 'draw_rect', 'fill_triangle')
SINK_CALL = re.compile(r'(?<![A-Za-z0-9_])(' + '|'.join(SINKS) + r')\s*(?:::<[^>]*>)?\s*\(')
INT = r'(?:0[xX][0-9A-Fa-f_]+|0[bB][01_]+|[0-9][0-9_]*)' + SUFFIX
FROM_BYTES = re.compile(r'\bu32\s*::\s*from_(be|le|ne)_bytes\s*\(\s*\[\s*(' + INT + r')\s*,\s*(' + INT + r')\s*,\s*(' + INT + r')\s*,\s*(' + INT + r')\s*,?\s*\]\s*\)')
ITEM = re.compile(r'\b(?:const|static(?:\s+mut)?)\s+[A-Za-z_][A-Za-z0-9_]*\s*:\s*((?:[^=;\[]|\[[^\]]*\])*?)=([^;]*);', re.S)
RGB_TYPE = re.compile(r'\(\s*u8\s*,\s*u8\s*,\s*u8\s*(?:,\s*u8\s*)?\)|\[\s*u8\s*;\s*[34]\s*\]')
GROUP = re.compile(r'[\(\[]\s*(' + INT + r')\s*,\s*(' + INT + r')\s*,\s*(' + INT + r')\s*(?:,\s*(' + INT + r')\s*)?,?\s*[\)\]]')

def strip(src):
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith('//', i):
            j = src.find('\n', i)
            i = n if j < 0 else j
            continue
        if src.startswith('/*', i):
            j = src.find('*/', i + 2)
            seg = src[i:(n if j < 0 else j + 2)]
            out.append('\n' * seg.count('\n'))
            i = n if j < 0 else j + 2
            continue
        m = re.match(r'b?r(#*)"', src[i:i + 8])
        if m and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == '_')):
            close = '"' + m.group(1)
            j = src.find(close, i + len(m.group(0)))
            seg = src[i:(n if j < 0 else j + len(close))]
            out.append('""' + '\n' * seg.count('\n'))
            i = n if j < 0 else j + len(close)
            continue
        if c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == '\\' else 1
            seg = src[i:j + 1]
            out.append('""' + '\n' * seg.count('\n'))
            i = j + 1
            continue
        if c == "'":
            m = re.match(r"'(\\.[^']*|[^'\\])'", src[i:i + 12])
            if m:
                out.append("' '")
                i += len(m.group(0))
                continue
        out.append(c)
        i += 1
    return ''.join(out)

def int_of(tok):
    t = re.sub(r'[ui](?:8|16|32|64|128|size)$', '', tok).replace('_', '')
    return int(t, 0) if not (len(t) > 1 and t[0] == '0' and t[1].isdigit()) else int(t, 10)

def is_colour(digits):
    d = digits.replace('_', '').upper()
    if len(d) == 6:
        return True
    # 0xFFFF_xxxx is a sentinel / an all-ones word, never a packed pixel the glass paints.
    return len(d) == 8 and not d.startswith('FFFF')

def is_mask_use(text, s, e, digits):
    """An operand of `&` / `^` is a mask; an operand of `|` is one when every nibble is 0 or F (the alpha bit)."""
    d = digits.replace('_', '').upper()
    before = text[:s].rstrip()
    after = text[e:].lstrip()
    if before.endswith(('&', '^', '&=', '^=')) or after.startswith(('&', '^')):
        return not after.startswith('&&') and not before.endswith('&&')
    if all(ch in '0F' for ch in d):
        return before.endswith(('|', '|=')) or (after.startswith('|') and not after.startswith('||'))
    return False

def line_of(t, pos):
    return t.count('\n', 0, pos) + 1

def scan_text(text):
    """-> [(line, literal as written, form)] for every colour literal in one source file."""
    hits = []
    t = strip(text)
    taken = []
    for m in FROM_BYTES.finditer(t):
        try:
            b = [int_of(m.group(i)) for i in (2, 3, 4, 5)]
        except ValueError:
            continue
        if any(x > 255 for x in b):
            continue
        if m.group(1) == 'le':
            b = b[::-1]
        word = '%02X%02X%02X%02X' % tuple(b)
        taken.append((m.start(), m.end()))
        if is_colour(word):
            hits.append((line_of(t, m.start()), re.sub(r'\s+', '', m.group(0)), 'from_bytes'))
    for m in ITEM.finditer(t):
        if not RGB_TYPE.search(m.group(1)):
            continue
        for g in GROUP.finditer(m.group(2)):
            vals = [int_of(x) for x in g.groups() if x is not None]
            if all(v <= 255 for v in vals):
                s0 = m.start(2) + g.start()
                taken.append((s0, m.start(2) + g.end()))
                hits.append((line_of(t, s0), re.sub(r'\s+', '', g.group(0)), 'rgb_tuple'))
    for m in WIDE_ITEM.finditer(t):
        taken.append((m.start(1), m.end(1)))
    for m in HEX.finditer(t):
        if any(a <= m.start() < b for a, b in taken):
            continue
        if m.group(2) in WIDE:
            continue
        digits = m.group(1)
        if is_colour(digits) and not is_mask_use(t, m.start(), m.end(), digits):
            hits.append((line_of(t, m.start()), m.group(0), 'hex'))
    return sorted(hits)

# Each plant: (source, expected count). One per spelling the tree uses, plus the forms that must stay unseen.
PLANTS = [
    ('fn f() { fill(px, 0x0012_3456); }', 1, 'bare 0x00RRGGBB'),
    ('fn f() { fill(px, 0x12_34_56); }', 1, 'underscore-grouped 0xRR_GG_BB'),
    ('fn f() { fill(px, 0x123456); }', 1, 'ungrouped 0xRRGGBB'),
    ('let v = alloc::vec![0x0012_3456u32; 9];', 1, 'u32-suffixed'),
    ('let v = 0xFF00_0000u32;', 1, 'u32-suffixed opaque'),
    ('let v = 0x8012_3456;', 1, 'alpha byte 80'),
    ('let c = u32::from_be_bytes([0, 0x12, 0x34, 0x56]);', 1, 'from_be_bytes'),
    ('let c = u32::from_le_bytes([86, 52, 18, 0]);', 1, 'from_le_bytes (decimal)'),
    ('const PAL: [u32; 3] = [0x0012_3456, 0x0065_4321u32, 0x00AB_CDEF];', 3, '[u32; N] palette'),
    ('const BG: (u8, u8, u8) = (0x12, 0x34, 0x56);', 1, 'const RGB tuple'),
    ('const BG: [u8; 4] = [18, 52, 86, 255];', 1, 'const [u8; 4] RGBA'),
    ('static RAMP: [(u8, u8, u8); 2] = [(1, 2, 3), (4, 5, 6)];', 2, 'static table of RGB tuples'),
    ('let v = p & 0x00FF_FFFF;', 0, 'a & mask'),
    ('let p = 0xFF00_0000\n    | r;', 0, 'a | alpha bit'),
    ('let p = px.unwrap_or(0xFF00_0000);', 1, 'an opaque black value (not a mask)'),
    ('// 0x0012_3456\nlet s = "0x00AB_CDEF";', 0, 'comment and string'),
    ('let n = 0xFFFF_FFFFu32;', 0, 'sentinel'),
    ('let ip = u32::from_be_bytes([256, 0, 1, 2]);', 0, 'from_bytes not of bytes'),
    ('let h = 0xcbf2_9ce4_8422_2325u64;', 0, 'a 64-bit hash basis'),
    ('const K: [u8; 3] = [1, 2, 3]; fn g() -> u32 { 0x50u8 as u32 }', 1, 'a [u8;3] const counts, a short literal does not'),
    ('const KIND: (u16, u16, u16) = (1, 2, 3);', 0, 'a non-u8 tuple'),
    ('const OFF_CHNCTL: u64 = 0x0000_04E0;', 0, 'a u64-typed register offset'),
    ('let base = [0x0800_0000u64, 0x0700_0000usize];', 0, 'u64/usize-suffixed addresses'),
    ('let off: u64 = if cap { 0x0003_0000 } else { 0 };', 0, 'a u64-typed let'),
    ('const FG: u32 = 0x0012_3456;', 1, 'a u32-typed const still counts'),
]

def sink_calls(text):
    """The sink names a source CALLS (comments/strings stripped; a `fn <sink>(` definition is not a call)."""
    t = strip(text)
    return sorted({m.group(1) for m in SINK_CALL.finditer(t) if not re.search(r'\bfn\s+$', t[max(0, m.start() - 8):m.start()])})

def judge(tree, painters):
    """tree: {path under crates/kernel/src: source}; painters: listed paths. -> (hits{path: [...]}, unlisted, missing)."""
    hits, unlisted = {}, []
    for path in sorted(tree):
        vid = path.startswith('video/')
        if vid and path == 'video/theme.rs':
            continue
        if vid or path in painters:
            h = scan_text(tree[path])
            if h:
                hits[path] = h
        elif sink_calls(tree[path]):
            unlisted.append(path)
    return hits, unlisted, [p for p in painters if p not in tree]

# Each painter plant: (tree, painters, want literals, want unlisted, want missing, what).
PAINTER_PLANTS = [
    ({'video/a.rs': '', 'console.rs': 'fn f(fb: &Fb) { fb.fill_rect(0, 0, 1, 1, 0x0012_3456); }'}, ['console.rs'], 1, 0, 0,
     'A2: 0x0012_3456 in a listed painter outside video/'),
    ({'newpaint.rs': 'fn f(fb: &Fb) { fb.fill_screen(theme::desktop_bg()); }'}, [], 0, 1, 0, 'an unlisted painter'),
    ({'ui.rs': 'fn f() { crate::video::text::draw_text(px, s, w, h, 0, 0, b, ink, false, face); }'}, [], 0, 1, 0,
     'an unlisted text-rasteriser caller'),
    ({'gpu.rs': 'pub fn fill_rect(&self, c: u32) {}\n// fb.fill_rect(0, 0, 1, 1, c);\nlet s = "fill_screen(0)";'}, [], 0, 0, 0,
     'a sink definition, a comment, a string'),
    ({'regs.rs': 'const R: u32 = 0x0012_3456;'}, [], 0, 0, 0, 'a non-painter outside video/ is not scanned'),
    ({}, ['gone.rs'], 0, 0, 1, 'a listed painter that is gone'),
]

def probes(verbose=False):
    ok = True
    for src, want, what in PLANTS:
        got = len(scan_text(src))
        ok &= got == want
        if verbose:
            print(f'  plant {what:44} want={want} got={got} ' + ('ok' if got == want else 'MISS'))
    for tree, painters, wl, wu, wm, what in PAINTER_PLANTS:
        h, u, m = judge(tree, painters)
        got = (sum(len(v) for v in h.values()), len(u), len(m))
        ok &= got == (wl, wu, wm)
        if verbose:
            print(f'  plant {what:44} want=({wl},{wu},{wm}) got=({got[0]},{got[1]},{got[2]}) ' + ('ok' if got == (wl, wu, wm) else 'MISS'))
    return ok

def load_allow(path):
    rows = []
    if os.path.exists(path):
        for ln in open(path, encoding='utf-8'):
            ln = ln.rstrip('\n')
            if not ln.strip() or ln.lstrip().startswith('#'):
                continue
            parts = ln.split(None, 2)
            if len(parts) < 3:
                print(f'appearance-check: allow row without a reason: {ln!r}')
                rows.append((None, None, None))
                continue
            rows.append(tuple(parts))
    return rows

def load_painters(path):
    out = []
    if os.path.exists(path):
        for ln in open(path, encoding='utf-8'):
            if ln.strip() and not ln.lstrip().startswith('#'):
                out.append(ln.split()[0])
    return out

def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    lst = '--list' in sys.argv
    st = '--selftest' in sys.argv
    here = os.path.dirname(os.path.abspath(__file__))
    u = args[0] if args else os.path.join(here, '..')
    vid = os.path.join(u, 'crates/kernel/src/video')
    if not probes(verbose=st):
        print('appearance-check: CONTROL PROBE FAILED — no verdict')
        return 2
    if st:
        print(f'appearance-check --selftest: plants={len(PLANTS) + len(PAINTER_PLANTS)} -> ok')
        return 0
    allow = load_allow(os.path.join(u, 'scripts/appearance.allow'))
    bad_rows = sum(1 for r in allow if r[0] is None)
    painters = load_painters(os.path.join(u, 'scripts/appearance.painters'))
    src = os.path.join(u, 'crates/kernel/src')
    tree = {}
    for root, _, names in os.walk(src):
        for nm in names:
            if nm.endswith('.rs'):
                p = os.path.join(root, nm)
                tree[os.path.relpath(p, src).replace(os.sep, '/')] = open(p, encoding='utf-8', errors='replace').read()
    raw, unlisted, missing = judge(tree, painters)
    used = set()
    total, files, allowed = 0, {}, 0
    for path in sorted(raw):
        rel = os.path.relpath(os.path.join(src, path), vid).replace(os.sep, '/')
        hits = []
        for h in raw[path]:
            key = next((i for i, r in enumerate(allow) if r[0] == rel and r[1] == h[1]), None)
            if key is None:
                hits.append(h)
            else:
                used.add(key)
                allowed += 1
        if hits:
            files[rel] = hits
            total += len(hits)
    stale = [r for i, r in enumerate(allow) if r[0] is not None and i not in used]
    theme = open(os.path.join(vid, 'theme.rs'), encoding='utf-8').read()
    m = re.search(r'pub const AUDIT_LITERALS_OUTSIDE: usize = (\d+);', theme)
    want = int(m.group(1)) if m else None
    for f in sorted(files):
        print(f'  {f}: {len(files[f])}' + ('' if not lst else '  ' + ' '.join(f'{ln}:{lit}' for ln, lit, _ in files[f])))
    for r in stale:
        print(f'  STALE allow row: {r[0]} {r[1]}')
    for p in unlisted:
        print(f'  unlisted painter: {p} calls ' + ', '.join(sink_calls(tree[p])))
    for p in missing:
        print(f'  listed painter missing: {p}')
    print(f'appearance-check: literals_outside_theme={total} files={len(files)} certified={want} '
          f'allow_rows={len(allow)} allowed_hits={allowed} stale={len(stale) + bad_rows} '
          f'plants={len(PLANTS) + len(PAINTER_PLANTS)} painters={len(painters)} unlisted={len(unlisted) + len(missing)}')
    return 0 if (total == 0 and want == 0 and not stale and not bad_rows and not unlisted and not missing) else 1

if __name__ == '__main__':
    sys.exit(main())
