# Run oracle/gen_kat.py's logic on the three vendored faces only, with repo-relative paths.
import sys, re, runpy, io, os
src = open('oracle/gen_kat.py').read()
root = os.path.abspath(os.path.join(os.getcwd(), '../../../..'))
rel = ["fonts/inter/InterVariable.ttf", "fonts/jetbrains-mono/JetBrainsMono-Variable.ttf", "fonts/noto-sans/NotoSans-Variable.ttf"]
# path on disk differs from the path written into the row: patch q(p) for the vendored rows
src = re.sub(r'FONTS = \[.*?\]\n', 'FONTS = %r\n' % [os.path.join(root, r) for r in rel], src, flags=re.S)
src = src.replace('q(p)', 'q(os.path.relpath(p, %r))' % root).replace('import json', 'import json, os')
exec(compile(src, 'gen_kat_vendored', 'exec'), {'__name__': '__main__'})
