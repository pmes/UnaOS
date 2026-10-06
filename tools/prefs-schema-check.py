#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# prefs-schema-check — every preference key referenced in the tree must be declared in the ONE schema
# (PRINCIPIA2, LEDGER SR32; AUDIT B287). The schema is `prefs_core::schema::SCHEMA`; its generated form is
# docs/dev/PREFS-SCHEMA.md (the gate `cargo test -p prefs_core --test schema_gate` proves the document IS
# the table, then runs this script). Exit 0 = every referenced key is declared; exit 1 = the undeclared
# keys are printed, one per line, with where they were found.
#
#   tools/prefs-schema-check.py              scan the tree
#   tools/prefs-schema-check.py --list       also print every reference found (declared or not)
#   tools/prefs-schema-check.py --selftest   prove the scanner goes red on an undeclared key
#
# What counts as a reference (non-test code only: a `#[cfg(test)]` item's braces are skipped, and
# `tests/` directories are not scanned):
#   R1 a `pub const X: &str = "<key>"` inside `mod key { ... }` of a file that declares
#      `pub const NS: &str = "<ns>"` (the kernel's `prefs::key`)                       -> <ns>.<key>
#   R2 `.get(<NS>, "<key>")` where <NS> is a string literal or a `*NS` constant      -> <ns>.<key>
#   R3 a bare `get("<key>")` call in a file that defines `fn from_prefs(` (the consumers' lookup
#      closure; namespace = that file's or the tree's `PREF_NS`)                       -> <ns>.<key>
#   R4 a PREF_GET / PREF_SET bus body literal `b"<ns>.<key>..."`                      -> <ns>.<key>
#   R5 `PrefGet|PrefSet { ns: "<ns>", key: "<key>"` and `set("<ns>", "<key>"`        -> <ns>.<key>
#   R6 a `"display.<...>"` literal in prefs_core's `display` module                   -> system.<key>
# The kernel's own shapes (PREFSKERNEL, rmbp-ledger B345 — the gate covers unaos/crates/kernel/src):
#   R7 `prefs::get|set|set_applied("<ns>", "<key>"`                                    -> <ns>.<key>
#   R8 a closure `let f = |k: &str| …prefs::get("<ns>", k)…` and every `f("<key>")` call in that file
#                                                                                      -> <ns>.<key>
#   R9 `prefs::int|peek_int|flag|text(` or `sys_int|sys_flag|sys_text|sys_get|sys_set(` (prefs_client)
#      on a `"<key>"` literal, or on a `CONST` the same file declares `const CONST: &str = "<key>"`
#                                                                                      -> system.<key>
#   R4 also reads Principia's ring-3 tags (`BUS_VERB_R3PREF_GET|SET`).
#   R10 the user-prefs demo TABLE rows `(b"<ns>.<key>", b"<value>")` (crates/user-prefs): its keys are
#      schema keys since PREFSKERNEL (its five namespace-less demo keys drifted from the schema).
# SETTINGSFILES (rmbp-ledger B407, R98): the store is split by DOMAIN (`<home>/settings/<domain>`,
# `prefs_core::files::domain_of`, mirrored by `domain_of` below) and the report says declared=referenced per
# domain. A program's own keys (`app.<name>.<key>`) are declared by its PrefDeclare stanza, not the table:
#   R12 a stanza byte literal `b"<name>\0<key>\t<spec>\t<default>\t<doc>\n…"` DECLARES app.<name>.<key>.
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCAN = ["unaos/crates", "unaos/libs", "libs", "handlers", "vessels", "tools"]
SKIP_DIRS = {"target", "tests", ".git", "node_modules", "build"}
SKIP_FILES = {os.path.join("unaos", "libs", "sys", "prefs_core", "src", "schema.rs")}
DOC = os.path.join(ROOT, "docs", "dev", "PREFS-SCHEMA.md")

SEG = r"[A-Za-z0-9_-]+"
KEY = r"[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*"


def declared(doc_text):
    keys = set()
    for line in doc_text.splitlines():
        m = re.match(r"\| `(" + SEG + r")\.(" + KEY + r")` \|", line)
        if m:
            keys.add(m.group(1) + "." + m.group(2))
    return keys


def strip_tests(src):
    """Blank every `#[cfg(test)]` item (its braces), keeping line numbers."""
    out = list(src)
    i = 0
    while True:
        j = src.find("#[cfg(test)]", i)
        if j < 0:
            break
        k = src.find("{", j)
        semi = src.find(";", j)
        if k < 0 or (0 <= semi < k):  # `#[cfg(test)] use ...;` / `extern crate std;`
            end = semi + 1 if semi >= 0 else len(src)
        else:
            depth, p = 0, k
            while p < len(src):
                c = src[p]
                if c == "{":
                    depth += 1
                elif c == "}":
                    depth -= 1
                    if depth == 0:
                        break
                p += 1
            end = p + 1
        for q in range(j, min(end, len(src))):
            if out[q] != "\n":
                out[q] = " "
        i = end
    return "".join(out)


def ns_consts(files):
    consts = {}
    for _, src in files:
        for m in re.finditer(r"const (\w*NS): &str = \"(" + SEG + r")\"", src):
            consts.setdefault(m.group(1), m.group(2))
    return consts


def refs_in(path, src, consts):
    """Yield (ns.key, line, rule) for one file's non-test source."""
    line_of = lambda pos: src.count("\n", 0, pos) + 1
    file_ns = None
    m = re.search(r"pub const NS: &str = \"(" + SEG + r")\"", src)
    if m:
        file_ns = m.group(1)
    # R1
    if file_ns:
        for mk in re.finditer(r"mod key \{(.*?)\n\}", src, re.S):
            for c in re.finditer(r"pub const \w+: &str = \"(" + KEY + r")\"", mk.group(1)):
                yield file_ns + "." + c.group(1), line_of(mk.start(1) + c.start()), "R1"
    # R2
    for g in re.finditer(r"\.get\(\s*(\"" + SEG + r"\"|[\w:]*\bNS|[\w:]*_NS)\s*,\s*\"(" + KEY + r")\"\s*\)", src):
        tok = g.group(1)
        ns = tok.strip('"') if tok.startswith('"') else consts.get(tok.split("::")[-1])
        if ns:
            yield ns + "." + g.group(2), line_of(g.start()), "R2"
    # R3
    if "fn from_prefs(" in src:
        ns = consts.get("PREF_NS")
        for g in re.finditer(r"(?<![\w.])get\(\"(" + KEY + r")\"\)", src):
            if ns:
                yield ns + "." + g.group(1), line_of(g.start()), "R3"
    # R4
    for g in re.finditer(r"BUS_VERB_(?:R3)?PREF_(?:GET|SET)\s*,\s*\w+\s*,\s*b\"(" + SEG + r")\.(" + KEY + r")", src):
        yield g.group(1) + "." + g.group(2), line_of(g.start()), "R4"
    # R5
    for g in re.finditer(r"Pref(?:Get|Set)\s*\{\s*ns:\s*\"(" + SEG + r")\"[^}]*?key:\s*\"(" + KEY + r")\"", src):
        yield g.group(1) + "." + g.group(2), line_of(g.start()), "R5"
    for g in re.finditer(r"(?<![\w])set\(\s*\"(" + SEG + r")\"\s*,\s*\"(" + KEY + r")\"", src):
        yield g.group(1) + "." + g.group(2), line_of(g.start()), "R5"
    # R7
    for g in re.finditer(r"prefs::(?:get|set|set_applied)\(\s*\"(" + SEG + r")\"\s*,\s*\"(" + KEY + r")\"", src):
        yield g.group(1) + "." + g.group(2), line_of(g.start()), "R7"
    # R8
    for c in re.finditer(r"let (\w+) = \|(\w+): &str\|[^;\n]*?prefs::get\(\s*\"(" + SEG + r")\"\s*,\s*\2\s*\)", src):
        name, ns = c.group(1), c.group(3)
        for g in re.finditer(r"(?<![\w.])" + re.escape(name) + r"\(\s*\"(" + KEY + r")\"\s*\)", src):
            yield ns + "." + g.group(1), line_of(g.start()), "R8"
    # R9
    local = {m.group(1): m.group(2) for m in re.finditer(r"const (\w+): &str = \"(" + KEY + r")\"", src)}
    for g in re.finditer(r"(?:prefs::(?:int|peek_int|flag|text|set_sys)|(?<![\w])(?:sys_(?:int|flag|text|get|set)|set_sys))\(\s*(\"(" + KEY + r")\"|[A-Z][A-Z0-9_]*)\s*[,)]", src):
        key = g.group(2) if g.group(2) else local.get(g.group(1))
        if key:
            yield "system." + key, line_of(g.start()), "R9"
    # R10
    if path.replace(os.sep, "/").endswith("crates/user-prefs/src/main.rs"):
        mt = re.search(r"const TABLE: [^=]*= &\[(.*?)\];", src, re.S)
        if mt:
            for g in re.finditer(r"\(b\"(" + SEG + r")\.(" + KEY + r")\"\s*,", mt.group(1)):
                yield g.group(1) + "." + g.group(2), line_of(mt.start(1) + g.start()), "R10"
    # R11 (VEINTLS, SR36): a ring-3 PREF_GET client asking by the dotted literal — vein_ring3's
    # `get("vein.endpoint", …)` / `one("vein.key_file", …)` in a file that speaks BUS_VERB_PREF_GET.
    if "BUS_VERB_PREF_GET" in src:
        for g in re.finditer(r"(?<![\w.])(?:get|one)\(\s*\"(" + SEG + r")\.(" + KEY + r")\"\s*,", src):
            yield g.group(1) + "." + g.group(2), line_of(g.start()), "R11"
    # R6
    if path.endswith(os.path.join("prefs_core", "src", "lib.rs")):
        md = re.search(r"pub mod display \{", src)
        if md:
            for g in re.finditer(r"\"(display\." + KEY + r")\"", src[md.start():]):
                yield "system." + g.group(1), line_of(md.start() + g.start()), "R6"


def load_tree():
    files = []
    for top in SCAN:
        base = os.path.join(ROOT, top)
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for f in filenames:
                if not f.endswith(".rs"):
                    continue
                full = os.path.join(dirpath, f)
                rel = os.path.relpath(full, ROOT)
                if rel in SKIP_FILES:
                    continue
                try:
                    with open(full, encoding="utf-8", errors="replace") as fh:
                        files.append((rel, strip_tests(fh.read())))
                except OSError:
                    pass
    return files


SYSTEM_DOMAINS = ("display", "login", "desktop", "sound", "trackpad", "notify", "general")


def domain_of(full):
    """`prefs_core::files::domain_of` for `<ns>.<key>` (the Rust test pins the Rust side to the schema)."""
    ns, _, key = full.partition(".")
    first = key.split(".")[0]
    if ns == "system":
        if key == "display.wallpaper":
            return "desktop"
        return {"display": "display", "dock": "desktop", "login": "login", "audio": "sound",
                "pointer": "trackpad", "trackpad": "trackpad", "notify": "notify"}.get(first, "general")
    if ns == "app":
        return first
    return ns


STANZA = re.compile(r'b"(' + SEG + r')\\0((?:[^"\\]|\\.)*)"')


def stanzas(files):
    """R12: every app.<name>.<key> a PrefDeclare stanza literal declares."""
    keys = set()
    for _rel, src in files:
        for m in STANZA.finditer(src):
            for line in m.group(2).split("\\n"):
                f = line.split("\\t")
                k = f[0]
                if len(f) >= 3 and re.fullmatch(KEY, k) and re.match(r"(int|float|bool|str|enum)\b", f[1]):
                    keys.add("app.%s.%s" % (m.group(1), k))
    return keys


def per_domain(found, keys):
    doms = {}
    for k in keys:
        doms.setdefault(domain_of(k), [0, 0])[0] += 1
    for k in found:
        doms.setdefault(domain_of(k), [0, 0])[1] += 1
    return " ".join("%s=%d/%d" % (d, v[0], v[1]) for d, v in sorted(doms.items()))


def scan(files):
    consts = ns_consts(files)
    found = {}
    for rel, src in files:
        for key, line, rule in refs_in(rel, src, consts):
            found.setdefault(key, []).append("%s:%d (%s)" % (rel, line, rule))
    return found


def report(found, keys, show_all):
    missing = sorted(k for k in found if k not in keys)
    if show_all:
        for k in sorted(found):
            print("%s %s  %s" % ("ok     " if k in keys else "MISSING", k, found[k][0]))
    for k in missing:
        print("undeclared %s  referenced at %s" % (k, ", ".join(found[k])))
    print("prefs-schema-check: domains (declared/referenced) %s" % per_domain(found, keys))
    print("prefs-schema-check: declared=%d referenced=%d undeclared=%d -> %s"
          % (len(keys), len(found), len(missing), "PASS" if not missing else "FAIL"))
    return 0 if not missing else 1


def selftest(keys):
    fake = [
        ("unaos/crates/kernel/src/prefs.rs",
         'pub const NS: &str = "system";\npub mod key {\n    pub const BRIGHTNESS: &str = "display.brightness";\n'
         '    pub const GHOST: &str = "display.ghost_key";\n}\n'),
        ("libs/x/src/a.rs", 'pub const PREF_NS: &str = "vein";\nfn from_prefs() { get("provider"); get("phantom.knob"); }\n'
         '#[cfg(test)]\nmod tests { fn t() { get("only.in_tests"); } }\n'),
        ("unaos/crates/user-x/src/main.rs", 'ask(BUS_VERB_PREF_GET, 1, b"vein.provider");\nask(BUS_VERB_PREF_SET, 2, b"quarry.view\\x00\\"list\\"");\n'),
        # PREFSKERNEL: the kernel shapes R7..R9 and the user-prefs table R10.
        ("unaos/crates/kernel/src/drivers/x.rs",
         'pub const PREF_KEY: &str = "audio.ghost_ms";\nfn a() { crate::prefs::peek_int(PREF_KEY, 0, 9); crate::prefs::int("audio.volume", 0, 16); }\n'
         'fn b() { crate::prefs::set_sys("display.ghost_set", v); crate::prefs::get("vein", "ghost_url"); let s = |k: &str| crate::prefs::get("vein", k).map(|v| v); s("phantom_tls"); s("provider"); }\n'),
        ("unaos/crates/user-prefs/src/main.rs", 'const TABLE: &[(&[u8], &[u8])] = &[\n    (b"system.audio.volume", b"12"),\n    (b"ui.theme", b"dark"),\n];\n'
         'fn c() { ask(BUS_VERB_R3PREF_GET, 1, b"ui.font_scale"); }\n'),
        # R11: the deleted `vein.tls` (VEINTLS) must go red if a ring-3 PREF_GET client asks for it again.
        ("unaos/libs/sys/x_ring3/src/prefs.rs", 'use una_abi::BUS_VERB_PREF_GET;\nlet a = one("vein.model", &mut v, &mut e);\nlet b = one("vein.tls", &mut v, &mut e);\n'),
        # SETTINGSFILES R12: a stanza declares app.demo.zoom; app.demo.ghost is referenced, never declared.
        ("unaos/crates/kernel/src/y.rs", 'const S: &[u8] = b"demo\\0zoom\\tint:1:4\\t1\\tthe zoom\\n";\n'
         'fn d() { crate::prefs::get("app", "demo.zoom"); crate::prefs::get("app", "demo.ghost"); }\n'),
    ]
    files = [(p, strip_tests(s)) for p, s in fake]
    found = scan(files)
    keys = keys | stanzas(files)
    want_missing = {"system.display.ghost_set", "app.demo.ghost", "system.display.ghost_key", "vein.phantom.knob", "quarry.view", "vein.tls",
                    "system.audio.ghost_ms", "vein.ghost_url", "vein.phantom_tls", "ui.theme", "ui.font_scale"}
    missing = {k for k in found if k not in keys}
    ok = (missing == want_missing and "vein.only.in_tests" not in found and "system.display.brightness" in found
          and "vein.model" in found and "system.audio.volume" in found and "vein.provider" in found
          and "app.demo.zoom" in found and domain_of("app.demo.zoom") == "demo" and domain_of("system.dock.pins") == "desktop")
    print("prefs-schema-check selftest: found=%s missing=%s -> %s" % (sorted(found), sorted(missing), "PASS" if ok else "FAIL"))
    return 0 if ok else 1


def main(argv):
    try:
        with open(DOC, encoding="utf-8") as fh:
            keys = declared(fh.read())
    except OSError:
        print("prefs-schema-check: %s missing — generate it (PREFS_SCHEMA_BLESS=1 cargo test -p prefs_core --test schema_gate)" % DOC)
        return 2
    if not keys:
        print("prefs-schema-check: no rows parsed from %s" % DOC)
        return 2
    if "--selftest" in argv:
        return selftest(keys)
    tree = load_tree()
    return report(scan(tree), keys | stanzas(tree), "--list" in argv)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
