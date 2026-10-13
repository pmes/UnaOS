#!/usr/bin/env python3
"""Generate handlers/aether/src/js/reflect_table.rs from WPT's reflection tables (AETHERREFLECT, SR81).

Source of truth: web-platform-tests `html/dom/reflection.js` (the per-element global attribute loop) and
`html/dom/elements-*.js` (the per-element attribute tables), pinned at the sha the WPT runner uses
(handlers/aether/tests/wpt_dom.rs). Fetch them sparse (`git sparse-checkout set html/dom`) and run:

    python3 handlers/aether/tools/gen_reflect_table.py <wpt-checkout>/html/dom > handlers/aether/src/js/reflect_table.rs

The script reads the tables as data (a small JS object-literal parser; nothing is executed), maps each
entry to one reflection kind of HTML §2.6.1, places it on its element (by tag; the binding resolves the
tag's interface with `html::interface_for_tag`) or on the interface the HTML Standard defines it on
(global attributes on HTMLElement, ARIAMixin on Element, the attributes common to audio and video on
HTMLMediaElement), and appends SPEC rows: reflected attributes the HTML Standard defines that the WPT
tables do not list. A SPEC row that collides with a WPT row is an error, so the supplement can only fill
gaps, never change what the tables say. Rows the binding owns elsewhere are skipped with the reason
written into the output.
"""
import os
import re
import sys

WPT_SHA = "da1f6d20caf40b4003ae288dda8c97f252dd9264"

# ------------------------------------------------------------------------------------------- JS literal parser

TOKEN = re.compile(
    r"""\s+|//[^\n]*|/\*.*?\*/|(?P<str>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')|(?P<num>-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?)|(?P<id>[A-Za-z_$][\w$]*)|(?P<p>[{}\[\]:,;()=.])|(?P<other>.)""",
    re.S,
)


def tokens(src):
    pos = 0
    out = []
    while pos < len(src):
        m = TOKEN.match(src, pos)
        pos = m.end()
        if m.group("str"):
            out.append(("str", unescape(m.group("str")[1:-1])))
        elif m.group("num"):
            out.append(("num", float(m.group("num"))))
        elif m.group("id"):
            out.append(("id", m.group("id")))
        elif m.group("p"):
            out.append(("p", m.group("p")))
        elif m.group("other"):
            out.append(("other", m.group("other")))  # operators of code around the tables; never inside one
    return out


def unescape(s):
    return re.sub(r"\\(u[0-9a-fA-F]{4}|x[0-9a-fA-F]{2}|.)", lambda m: esc(m.group(1)), s)


def esc(e):
    if e[0] in "ux":
        return chr(int(e[1:], 16))
    return {"n": "\n", "t": "\t", "r": "\r", "f": "\f", "0": "\0"}.get(e, e)


class Parser:
    def __init__(self, toks):
        self.t = toks
        self.i = 0

    def peek(self):
        return self.t[self.i] if self.i < len(self.t) else ("eof", None)

    def eat(self, kind, val=None):
        k, v = self.peek()
        if k != kind or (val is not None and v != val):
            raise SystemExit(f"gen_reflect_table: expected {kind} {val!r}, got {k} {v!r}")
        self.i += 1
        return v

    def value(self):
        k, v = self.peek()
        if k == "p" and v == "{":
            return self.obj()
        if k == "p" and v == "[":
            self.eat("p", "[")
            arr = []
            while self.peek() != ("p", "]"):
                arr.append(self.value())
                if self.peek() == ("p", ","):
                    self.eat("p", ",")
            self.eat("p", "]")
            return arr
        if k in ("str", "num"):
            self.i += 1
            return v
        if k == "id":
            self.i += 1
            return {"true": True, "false": False, "null": None}.get(v, ("ident", v))
        raise SystemExit(f"gen_reflect_table: unexpected {k} {v!r}")

    def obj(self):
        self.eat("p", "{")
        out = {}
        while self.peek() != ("p", "}"):
            k, v = self.peek()
            if k not in ("id", "str"):
                raise SystemExit(f"gen_reflect_table: bad key {v!r}")
            self.i += 1
            self.eat("p", ":")
            out[v] = self.value()
            if self.peek() == ("p", ","):
                self.eat("p", ",")
        self.eat("p", "}")
        return out


def element_tables(src):
    """Every `var NAME = { tag: { idl: type, … }, … };` in an elements-*.js file, in source order."""
    toks = tokens(src)
    out = []
    i = 0
    while i < len(toks):
        if toks[i] == ("id", "var") and i + 3 < len(toks) and toks[i + 2] == ("p", "=") and toks[i + 3] == ("p", "{"):
            p = Parser(toks)
            p.i = i + 3
            out.append(p.obj())
            i = p.i
        else:
            i += 1
    return out


def global_reflects(src):
    """The `ReflectionTests.reflects(type, "idl", element[, "dom"])` calls of reflection.js's per-element loop."""
    start = src.index("for (var element in elements) {")
    end = src.index("for (var idlAttrName in elements[element])", start)
    out = []
    for m in re.finditer(r"ReflectionTests\.reflects\((.*?)\);", src[start:end]):
        p = Parser(tokens(m.group(1)))
        ty = p.value()
        p.eat("p", ",")
        idl = p.value()
        p.eat("p", ",")
        p.value()  # element
        dom = idl
        if p.peek() == ("p", ","):
            p.eat("p", ",")
            dom = p.value()
        out.append((idl, ty, dom))
    return out


# ------------------------------------------------------------------------------------------- policy

# Rows the binding owns elsewhere: (tag or "*", idl) -> reason. Written into the output as comments.
SKIP = {
    ("*", "className"): "Element.className is Element's own (js/dom.rs), not an HTMLElement reflection",
    ("a", "href"): "HTMLHyperlinkElementUtils (js/html.rs) owns href and its URL decomposition",
    ("area", "href"): "HTMLHyperlinkElementUtils (js/html.rs) owns href and its URL decomposition",
    ("input", "type"): "js/controls.rs owns input.type (the type-change steps, HTML §4.10.5)",
}

# The HTML Standard's abstract element interface: a row identical on every listed tag lives on it.
ABSTRACT = {"HTMLMediaElement": ("audio", "video")}

# Where the HTML Standard leaves a default implementation-defined and WPT accepts a set
# (`defaultVal: [...]`), the Standard's suggested state.
SUGGESTED = {("*", "preload"): "metadata"}

# Where the HTML Standard is wider than a WPT table: (tag, idl) -> extra keywords.
EXTRA_KEYWORDS = {
    # §4.10.18.6: formmethod takes the method attribute's keywords, dialog included (WPT lists get, post).
    ("input", "formMethod"): ["dialog"],
}

# Reflected attributes the HTML Standard (and the ARIA reflection spec) defines that the WPT
# elements-*.js tables do not list. Grammar, one per line: `ON idl[=content] kind [args]`; ON is a tag
# (lowercase) or an interface (capitalised). Kinds: s, sn ([LegacyNullToEmptyString]), sq (nullable
# DOMString), u, b, l D, ll, ul D, lu D, luf D, cu MIN MAX D, d D, ld D,
# e KEYWORDS [m=MISSING] [i=INVALID] [a=FROM>TO …] (ne: nullable enum; `null` is the null state).
REFERRER = ",no-referrer,no-referrer-when-downgrade,same-origin,origin,strict-origin,origin-when-cross-origin,strict-origin-when-cross-origin,unsafe-url"
SPEC = f"""
img sizes s
img loading e lazy,eager m=eager i=eager
img fetchPriority=fetchpriority e high,low,auto m=auto i=auto
iframe loading e lazy,eager m=eager i=eager
link fetchPriority=fetchpriority e high,low,auto m=auto i=auto
link disabled b
link imageSrcset=imagesrcset s
link imageSizes=imagesizes s
script fetchPriority=fetchpriority e high,low,auto m=auto i=auto
script referrerPolicy=referrerpolicy e {REFERRER}
source width ul 0
source height ul 0
details name s
form rel s
iframe allow s
button popoverTargetAction=popovertargetaction e toggle,show,hide m=toggle i=toggle
input popoverTargetAction=popovertargetaction e toggle,show,hide m=toggle i=toggle
""" + "".join(
    f"Element {idl}={attr} sq\n"
    for idl, attr in [
        ("ariaColCount", "aria-colcount"),
        ("ariaColIndex", "aria-colindex"),
        ("ariaColIndexText", "aria-colindextext"),
        ("ariaColSpan", "aria-colspan"),
        ("ariaDescription", "aria-description"),
        ("ariaKeyShortcuts", "aria-keyshortcuts"),
        ("ariaLabel", "aria-label"),
        ("ariaLevel", "aria-level"),
        ("ariaPlaceholder", "aria-placeholder"),
        ("ariaPosInSet", "aria-posinset"),
        ("ariaRelevant", "aria-relevant"),
        ("ariaRoleDescription", "aria-roledescription"),
        ("ariaRowCount", "aria-rowcount"),
        ("ariaRowIndex", "aria-rowindex"),
        ("ariaRowIndexText", "aria-rowindextext"),
        ("ariaRowSpan", "aria-rowspan"),
        ("ariaSetSize", "aria-setsize"),
        ("ariaValueMax", "aria-valuemax"),
        ("ariaValueMin", "aria-valuemin"),
        ("ariaValueNow", "aria-valuenow"),
        ("ariaValueText", "aria-valuetext"),
        ("ariaBrailleLabel", "aria-braillelabel"),
        ("ariaBrailleRoleDescription", "aria-brailleroledescription"),
        ("role", "role"),
    ]
)


def lookup(table, tag, idl):
    return table.get((tag, idl), table.get(("*", idl)))


class Row:
    def __init__(self, on, idl, attr, kind, src):
        self.on, self.idl, self.attr, self.kind, self.src = on, idl, attr, kind, src

    def key(self):
        return (self.on, self.idl)


def num(v):
    return float(v)


def enum_kind(tag, idl, d):
    kws = list(d["keywords"]) + [k for k in EXTRA_KEYWORDS.get((tag, idl), []) if k not in d["keywords"]]
    nullable = bool(d.get("isNullable", False))
    missing = d.get("defaultVal", "")
    if isinstance(missing, list):
        missing = lookup(SUGGESTED, tag, idl) or missing[0]
    invalid = d.get("invalidVal", missing)
    aliases = [(k, v) for k, v in d.get("nonCanon", {}).items()]
    return ("Enum", kws, missing, invalid, nullable, aliases)


def kind_of(tag, idl, dom, d):
    """WPT type descriptor -> (kind tuple) or (None, reason)."""
    if isinstance(d, str):
        d = {"type": d}
    t = d["type"]
    if isinstance(t, dict):
        # elements-obsolete.js nests marquee's behavior/direction one level (`type: {type: "enum"…}`), so
        # reflection.js skips them as an unknown type; the Standard reflects them as a plain DOMString (an
        # enumerated attribute not limited to known values is transparent, HTML §2.6.1).
        return ("Str",), None
    if d.get("customGetter"):
        return None, "customGetter: the getter is the element's own behaviour (forms/media arcs), not reflection"
    if t in ("tokenlist", "settable tokenlist"):
        return None, "a DOMTokenList, not a scalar §2.6.1 reflection (reflection.js has no test for the type)"
    if (tag == "form" and idl == "action") or (tag in ("button", "input") and idl == "formAction"):
        return ("Action",), None
    if tag == "*" and idl == "hidden":
        return ("Hidden",), None
    if tag == "*" and idl == "tabIndex":
        return ("TabIndex",), None
    if t == "string":
        return (("StrNull",) if "treatNullAsEmptyString" in d else ("Str",)), None
    if t == "url":
        return ("Url",), None
    if t == "boolean":
        return ("Bool",), None
    if t == "enum":
        return enum_kind(tag, idl, d), None
    if t == "long":
        return ("Long", int(d.get("defaultVal", 0) or 0)), None
    if t == "limited long":
        return ("LimLong", int(d.get("defaultVal", -1))), None
    if t == "unsigned long":
        return ("ULong", int(d.get("defaultVal", 0))), None
    if t == "limited unsigned long":
        return ("LimULong", int(d.get("defaultVal", 1))), None
    if t == "limited unsigned long with fallback":
        return ("LimULongFb", int(d["defaultVal"])), None
    if t == "clamped unsigned long":
        return ("Clamped", int(d["min"]), int(d["max"]), int(d["defaultVal"])), None
    if t == "double":
        return ("Double", num(d.get("defaultVal", 0.0))), None
    if t == "limited double":
        return ("LimDouble", num(d.get("defaultVal", 0.0))), None
    raise SystemExit(f"gen_reflect_table: unknown reflection type {t!r} ({tag}.{idl})")


def spec_kind(kind, rest):
    n = lambda i, dflt: float(rest[i]) if i < len(rest) else dflt
    simple = {"s": ("Str",), "sn": ("StrNull",), "sq": ("NullableStr",), "u": ("Url",), "b": ("Bool",), "ll": ("LimLong", -1)}
    if kind in simple:
        return simple[kind]
    if kind == "l":
        return ("Long", int(n(0, 0)))
    if kind == "ul":
        return ("ULong", int(n(0, 0)))
    if kind == "lu":
        return ("LimULong", int(n(0, 1)))
    if kind == "luf":
        return ("LimULongFb", int(n(0, 1)))
    if kind == "cu":
        return ("Clamped", int(n(0, 0)), int(n(1, 0)), int(n(2, 0)))
    if kind == "d":
        return ("Double", n(0, 0.0))
    if kind == "ld":
        return ("LimDouble", n(0, 0.0))
    if kind in ("e", "ne"):
        kws = rest[0].split(",") if rest else []
        missing, invalid, aliases = "", None, []
        val = lambda v: None if v == "null" else v
        for opt in rest[1:]:
            if opt.startswith("m="):
                missing = val(opt[2:])
            elif opt.startswith("i="):
                invalid = ("set", val(opt[2:]))
            elif opt.startswith("a="):
                f, _, to = opt[2:].partition(">")
                aliases.append((f, val(to)))
        return ("Enum", kws, missing, invalid[1] if invalid else missing, kind == "ne", aliases)
    raise SystemExit(f"gen_reflect_table: unknown SPEC kind {kind!r}")


def build(wpt_dir):
    rows, skipped = [], []
    with open(os.path.join(wpt_dir, "reflection.js"), encoding="utf-8") as f:
        refl = f.read()
    for idl, ty, dom in global_reflects(refl):
        why = lookup(SKIP, "*", idl)
        k, r = (None, why) if why else kind_of("*", idl, dom, ty)
        if k is None:
            skipped.append(("HTMLElement", idl, r))
            continue
        rows.append(Row("HTMLElement", idl, dom.lower(), k, "reflection.js"))

    per_tag = {}  # tag -> {idl: (attr, kind, file)}
    done = set()
    order = []
    files = sorted(f for f in os.listdir(wpt_dir) if f.startswith("elements-") and f.endswith(".js"))
    for fn in files:
        with open(os.path.join(wpt_dir, fn), encoding="utf-8") as f:
            src = f.read()
        for table in element_tables(src):
            for tag, attrs in table.items():
                if tag not in per_tag:
                    per_tag[tag] = {}
                    order.append(tag)
                for idl, d in attrs.items():
                    if (tag, idl) in done:
                        continue  # the first file names it (elements-forms-weekmonth.js repeats input)
                    done.add((tag, idl))
                    dom = d.get("domAttrName", idl) if isinstance(d, dict) else idl
                    why = lookup(SKIP, tag, idl)
                    k, r = (None, why) if why else kind_of(tag, idl, dom, d)
                    if k is None:
                        skipped.append((tag, idl, r))
                        continue
                    per_tag[tag][idl] = (dom.lower(), k, fn)

    hoisted = set()
    for iface, tags in ABSTRACT.items():
        first = per_tag.get(tags[0], {})
        for idl, v in first.items():
            if all(per_tag.get(t, {}).get(idl, (None, None))[:2] == v[:2] for t in tags):
                rows.append(Row(iface, idl, v[0], v[1], v[2]))
                hoisted.update((t, idl) for t in tags)
    for tag in order:
        for idl, (attr, k, fn) in per_tag[tag].items():
            if (tag, idl) in hoisted:
                continue
            if tag == "undefinedelement":
                on = "HTMLElement"  # "Global attributes should exist even on unknown elements"
            elif idl.startswith("aria"):
                on = "Element"  # ARIAMixin
            else:
                on = tag
            rows.append(Row(on, idl, attr, k, fn))

    seen = {r.key() for r in rows}
    for line in SPEC.strip().splitlines():
        w = line.split()
        on, name, kind, rest = w[0], w[1], w[2], w[3:]
        idl, _, attr = name.partition("=")
        attr = attr or idl.lower()
        r = Row(on, idl, attr, spec_kind(kind, rest), "HTML Standard")
        if r.key() in seen:
            raise SystemExit(f"gen_reflect_table: SPEC row {on}.{idl} collides with a WPT row")
        seen.add(r.key())
        rows.append(r)
    return rows, skipped


# ------------------------------------------------------------------------------------------- emit

def rs_str(s):
    out = '"'
    for c in s:
        if c in '"\\':
            out += "\\" + c
        elif 0x20 <= ord(c) < 0x7F:
            out += c
        else:
            out += "\\u{%x}" % ord(c)
    return out + '"'


def rs_opt(v):
    return "None" if v is None else f"Some({rs_str(v)})"


def rs_f64(v):
    s = repr(float(v))
    return s if "." in s or "e" in s else s + ".0"


def rs_kind(k):
    tag = k[0]
    if tag in ("Str", "StrNull", "NullableStr", "Url", "Action", "Bool", "Hidden", "TabIndex"):
        return tag
    if tag in ("Long", "LimLong", "ULong", "LimULong", "LimULongFb"):
        return f"{tag}({k[1]})"
    if tag == "Clamped":
        return f"Clamped {{ min: {k[1]}, max: {k[2]}, default: {k[3]} }}"
    if tag in ("Double", "LimDouble"):
        return f"{tag}({rs_f64(k[1])})"
    if tag == "Enum":
        _, kws, missing, invalid, nullable, aliases = k
        kw = ", ".join(rs_str(x) for x in kws)
        al = ", ".join(f"({rs_str(a)}, {rs_opt(b)})" for a, b in aliases)
        return (f"Enum(&En {{ keywords: &[{kw}], missing: {rs_opt(missing)}, invalid: {rs_opt(invalid)}, "
                f"nullable: {'true' if nullable else 'false'}, aliases: &[{al}] }})")
    raise SystemExit(f"gen_reflect_table: cannot emit {k!r}")


def rs_on(on):
    return f"Iface({rs_str(on)})" if on[0].isupper() else f"Tag({rs_str(on)})"


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: gen_reflect_table.py <wpt>/html/dom")
    rows, skipped = build(sys.argv[1])
    o = []
    o.append("//! GENERATED by `handlers/aether/tools/gen_reflect_table.py` from web-platform-tests `html/dom/reflection.js`")
    o.append(f"//! + `html/dom/elements-*.js` @ {WPT_SHA} (AETHERREFLECT, SR81). Do not edit: change the")
    o.append("//! script (its SKIP / ABSTRACT / SUGGESTED / EXTRA_KEYWORDS / SPEC tables) and re-run it.")
    o.append("//!")
    o.append("//! One row per reflected IDL attribute: where it lives (a tag, resolved to its interface by")
    o.append("//! `html::interface_for_tag`, or an interface), the IDL name, the content attribute, and the HTML §2.6.1")
    o.append("//! reflection kind with its defaults, keywords and limits. The kinds are implemented once, in `reflect.rs`.")
    o.append("//!")
    o.append("//! Not reflected from here (owned elsewhere in the binding):")
    for on, idl, why in skipped:
        o.append(f"//!   {on}.{idl}: {why}")
    o.append("")
    o.append("use super::reflect::En;")
    o.append("use super::reflect::Kind::*;")
    o.append("use super::reflect::On::*;")
    o.append("use super::reflect::Row;")
    o.append("")
    o.append("#[rustfmt::skip]")
    o.append("pub const ROWS: &[Row] = &[")
    src = None
    for r in rows:
        if r.src != src:
            o.append(f"    // {r.src}")
            src = r.src
        o.append(f"    Row {{ on: {rs_on(r.on)}, idl: {rs_str(r.idl)}, attr: {rs_str(r.attr)}, kind: {rs_kind(r.kind)} }},")
    o.append("];")
    sys.stdout.write("\n".join(o) + "\n")


if __name__ == "__main__":
    main()
