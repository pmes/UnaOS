import hashlib, os
from fontTools.ttLib import TTFont
root = os.path.abspath(os.path.join(os.getcwd(), '../../../..'))
IDS = {1, 2, 4, 6, 16, 17}
for rel in ["fonts/inter/InterVariable.ttf", "fonts/jetbrains-mono/JetBrainsMono-Variable.ttf", "fonts/noto-sans/NotoSans-Variable.ttf"]:
    p = os.path.join(root, rel)
    data = open(p, "rb").read()
    sha = hashlib.sha256(data).hexdigest()
    f = TTFont(p)
    for n in f["name"].names:
        if n.nameID not in IDS:
            continue
        ok = n.platformID == 0 or (n.platformID == 3 and n.platEncID in (0, 1, 10)) or (n.platformID == 1 and n.platEncID == 0)
        if not ok:
            continue
        try:
            v = n.toUnicode()
        except Exception:
            continue
        v = v.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n").replace("\r", "\\r")
        print("\t".join([rel, "0", sha, str(n.platformID), str(n.platEncID), str(n.langID), str(n.nameID), v]))
