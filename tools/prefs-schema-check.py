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
# The user-prefs fixture TABLE (BANDY3's R3PREF demo verbs 128/129) is not Principia's store and is not
# scanned.
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
    for g in re.finditer(r"BUS_VERB_PREF_(?:GET|SET)\s*,\s*\w+\s*,\s*b\"(" + SEG + r")\.(" + KEY + r")", src):
        yield g.group(1) + "." + g.group(2), line_of(g.start()), "R4"
    # R5
    for g in re.finditer(r"Pref(?:Get|Set)\s*\{\s*ns:\s*\"(" + SEG + r")\"[^}]*?key:\s*\"(" + KEY + r")\"", src):
        yield g.group(1) + "." + g.group(2), line_of(g.start()), "R5"
    for g in re.finditer(r"(?<![\w])set\(\s*\"(" + SEG + r")\"\s*,\s*\"(" + KEY + r")\"", src):
        yield g.group(1) + "." + g.group(2), line_of(g.start()), "R5"
    # R7 (VEINTLS, SR36): a ring-3 PREF_GET client asking by the dotted literal — vein_ring3's
    # `get("vein.endpoint", …)` / `one("vein.key_file", …)` in a file that speaks BUS_VERB_PREF_GET.
    if "BUS_VERB_PREF_GET" in src:
        for g in re.finditer(r"(?<![\w.])(?:get|one)\(\s*\"(" + SEG + r")\.(" + KEY + r")\"\s*,", src):
            yield g.group(1) + "." + g.group(2), line_of(g.start()), "R7"
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
        # R7: the deleted `vein.tls` (VEINTLS) must go red if a ring-3 PREF_GET client asks for it again.
        ("unaos/libs/sys/x_ring3/src/prefs.rs", 'use una_abi::BUS_VERB_PREF_GET;\nlet a = one("vein.model", &mut v, &mut e);\nlet b = one("vein.tls", &mut v, &mut e);\n'),
    ]
    files = [(p, strip_tests(s)) for p, s in fake]
    found = scan(files)
    want_missing = {"system.display.ghost_key", "vein.phantom.knob", "quarry.view", "vein.tls"}
    missing = {k for k in found if k not in keys}
    ok = missing == want_missing and "vein.only.in_tests" not in found and "system.display.brightness" in found and "vein.model" in found
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
    return report(scan(load_tree()), keys, "--list" in argv)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
