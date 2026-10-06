#!/usr/bin/env python3
# appearance-check.py — APPEARANCE (rmbp-ledger B408, MACPARITY rows 21/22): every colour the kernel's
# glass paints comes from the system palette (`crates/kernel/src/video/theme.rs`), never a literal.
#
# HOW IT DECIDES. Scope = crates/kernel/src/video/**/*.rs except theme.rs. Comments and string/char
# literals are stripped first. A COLOUR LITERAL is a hex literal whose digits (underscores dropped) are
# six, or eight with a top byte of 00 or FF (0x00RRGGBB / 0xFFRRGGBB — the packed pixel forms the
# compositor and the surfaces carry). A BIT MASK is not a colour: a literal whose every nibble is 0 or F
# written as an operand of `|` (the alpha bit), or any operand of `&` / `^` (a channel mask). A literal
# 0xFFFF_xxxx is a sentinel, not a pixel. Everything else counts.
# The count must equal `theme::AUDIT_LITERALS_OUTSIDE` (0), which the kernel's `tests appearance`
# witness prints as `literals_outside_theme=` — the committed constant is certified here, the way
# PREFS-SCHEMA.md is certified by prefs_core's schema gate.
#
# CONTROL PROBES (exit 2, no verdict, if any fails): a synthetic `fill(.., 0x0012_3456)` is counted; a
# synthetic `& 0x00FF_FFFF` mask is not; a literal inside a comment or a string is not.
#
# usage: appearance-check.py [<unaos dir>] [--list]   (exit 0 clean; 1 literals found or the constant
# disagrees; 2 a control probe failed)
import os, re, sys

HEX = re.compile(r'\b0[xX][0-9A-Fa-f_]+\b')

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

def is_colour(lit):
    d = lit[2:].replace('_', '').upper()
    if len(d) == 6:
        return True
    # 0xFFFF_xxxx is a sentinel / an all-ones word, never a packed pixel the glass paints.
    return len(d) == 8 and d[:2] in ('00', 'FF') and not d.startswith('FFFF')

def is_mask_use(text, s, e, lit):
    """An operand of `&` / `^` is a mask; an operand of `|` is one when every nibble is 0 or F (the alpha bit)."""
    d = lit[2:].replace('_', '').upper()
    before = text[:s].rstrip()
    after = text[e:].lstrip()
    if before.endswith(('&', '^', '&=', '^=')) or after.startswith(('&', '^')):
        return not after.startswith('&&') and not before.endswith('&&')
    if all(ch in '0F' for ch in d):
        return before.endswith(('|', '|=')) or (after.startswith('|') and not after.startswith('||'))
    return False

def scan_text(text):
    hits = []
    t = strip(text)
    for m in HEX.finditer(t):
        lit = m.group(0)
        if is_colour(lit) and not is_mask_use(t, m.start(), m.end(), lit):
            hits.append((t.count('\n', 0, m.start()) + 1, lit))
    return hits

def probes():
    ok = len(scan_text('fn f() { fill(px, 0x0012_3456); }')) == 1
    ok &= len(scan_text('let v = p & 0x00FF_FFFF;')) == 0
    ok &= len(scan_text('let p = 0xFF00_0000\n    | r;')) == 0
    ok &= len(scan_text('let p = px.unwrap_or(0xFF00_0000);')) == 1
    ok &= len(scan_text('// 0x0012_3456\nlet s = "0x00AB_CDEF";')) == 0
    return ok

def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    lst = '--list' in sys.argv
    u = args[0] if args else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
    vid = os.path.join(u, 'crates/kernel/src/video')
    if not probes():
        print('appearance-check: CONTROL PROBE FAILED — no verdict')
        return 2
    total, files = 0, {}
    for root, _, names in os.walk(vid):
        for nm in sorted(names):
            p = os.path.join(root, nm)
            if not nm.endswith('.rs') or os.path.relpath(p, vid) == 'theme.rs':
                continue
            hits = scan_text(open(p, encoding='utf-8', errors='replace').read())
            if hits:
                files[os.path.relpath(p, vid)] = hits
                total += len(hits)
    theme = open(os.path.join(vid, 'theme.rs'), encoding='utf-8').read()
    m = re.search(r'pub const AUDIT_LITERALS_OUTSIDE: usize = (\d+);', theme)
    want = int(m.group(1)) if m else None
    for f in sorted(files):
        print(f'  {f}: {len(files[f])}' + ('' if not lst else '  ' + ' '.join(f'{ln}:{lit}' for ln, lit in files[f])))
    print(f'appearance-check: literals_outside_theme={total} files={len(files)} certified={want}')
    return 0 if (total == 0 and want == 0) else 1

if __name__ == '__main__':
    sys.exit(main())
