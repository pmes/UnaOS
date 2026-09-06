#!/usr/bin/env python3
"""GATE-K8REACH — a knob with no `K8_FEATS` arm is UNREACHABLE in every Pi image, silently.

LEDGER SR1's CLASS. `kernel8()` builds from a CURATED `K8_FEATS` list that deliberately
does not draw from the general `_feats` map, so a knob added to `_feats` and never given
an arm is not an error and not a warning: the operator sets `UNAOS_X=1`, flashes, and the
image is byte-identical to the one without it. Two instances a week apart, different
seats, and each cost days before anyone suspected the knob rather than the code:
`UNAOS_PRTSCRST` (pi 7 — the Print Screen gate greened about nothing) and `UNAOS_BOOTLOG`
(orin 15 — a `UNAOS_PIDESK=1` image with no way back to the serial mirror).

This is NOT S9/KNOBLEG. That gate asks whether every aarch64-qualified feature is COMPILED
by some check leg; a knob can be fully leg-covered and still absent from the image an
operator boots. Build coverage vs operator reachability.

WHAT THIS GATE DOES NOT DO, and why. It does not decide which knobs BELONG in the Pi image.
That judgment is not mechanical: `nvidia-kepler`'s sites are x86 hardware but one of them
is in arch-neutral `video/wm.rs`, and `rastmc`'s single Pi-live site is a call whose callee
is x86-gated. pi 7's objection stands -- by inspection, a Pi-meaningful knob that was never
given an arm is indistinguishable from one deliberately omitted. So the gate asserts the
weaker thing that IS mechanical and that both instances would have failed: every knob is
ACCOUNTED FOR -- armed, or written down as deliberately unarmed. A knob added tomorrow
cannot be silently unreachable; it fails this gate until someone rules on it.

  RED  UNREGISTERED   a knob in `_feats` with no `K8_FEATS` arm and no registry row.
  RED  STALE          a registry row for a knob that is not in `_feats` -- DEFERRED on a track
                      branch, red on the trunk (see below).
  RED  CONTRADICTION  a knob that is both armed and registered as unarmed.

STALE IS DEFERRED ON A TRACK BRANCH, and the reason is a real ordering constraint rather than
leniency. The registry lives on `hw-rmbp` and knobs are added on every branch: orin 17's seven
NA rows are correct, evidenced and ready, and they name knobs that exist only on `hw-jetson`
until the merge. Landing them here strictly would red this branch for being early; holding them
until the merge means the gate's answer arrives after the commit that needed it. So a row whose
knob this tree does not have is DEFERRED and LISTED on a track branch, and becomes a red
automatically on the trunk, where every branch's `_feats` is present and a row nothing matches
really is dead. Same mechanism, same trunk trigger and the same override knobs as GATE-LEDGER's
DEFERRED/STRICT split (rmbp-ledger B21) -- wired, not remembered.

`--evidence` re-runs the site classification behind a row (module-tree context, prose
stripped, per-arm `target_arch`) so that ruling on a TODO row is a command, not a squint.
`--seed` prints a registry for the current tree.

CONTROL, checked before any verdict (exit 2, no verdict, if it fails): the `kernel8()`
bounds must resolve, the `_feats` parse must find >= 50 knobs INCLUDING both SR1 instances
(`UNAOS_PRTSCRST`, `UNAOS_BOOTLOG`), and the arm parse must find >= 20. A parse that
silently found no knobs would report a clean tree, which is the failure this whole file
exists to prevent. The canaries are checked for PRESENCE, not for being armed, so that
de-arming one is caught as a RED rather than swallowed as "no verdict" -- a control must
not blind the gate to the very instances that created the class.

usage: k8-reach.py [--root DIR] [--seed | --evidence KNOB|FEATURE]
"""
import os, re, subprocess, sys

# The evidence mode loads a sibling script by path; a gate that leaves __pycache__ behind
# dirties `git status` in every tree it runs in.
sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
CANARIES = ("UNAOS_PRTSCRST", "UNAOS_BOOTLOG")
TRUNK = os.environ.get("UNAOS_K8REACH_TRUNK", "main")
STATUSES = ("NA", "TODO")

RED, GREEN, YELLOW, OFF = "\033[91m", "\033[92m", "\033[93m", "\033[0m"


def parse_arroyo(path):
    """(knob -> features from the general `_feats` map, knobs named inside `kernel8()`)."""
    lines = open(path, encoding="utf8", errors="replace").read().split("\n")
    starts = [i for i, l in enumerate(lines) if l.startswith("kernel8() {")]
    if not starts:
        return None, None
    k8s = starts[0]
    ends = [i for i in range(k8s + 1, len(lines)) if re.match(r'^[A-Za-z_0-9]+\(\) *\{', lines[i])]
    if not ends:
        return None, None
    knob_feats = {}
    for l in lines[:k8s]:
        m = re.search(r'_feats="\$\{_feats\}([A-Za-z0-9_,\-]*),?"', l)
        if not m:
            continue
        for k in re.findall(r'UNAOS_[A-Z0-9_]+', l):
            knob_feats.setdefault(k, set()).update(f for f in m.group(1).split(",") if f)
    armed = {k for l in lines[k8s:ends[0]] for k in re.findall(r'UNAOS_[A-Z0-9_]+', l)}
    return knob_feats, armed


def parse_registry(path):
    """knob -> (status, reason). Blank lines and `#` comments are skipped."""
    rows = {}
    if not os.path.isfile(path):
        return rows
    for line in open(path, encoding="utf8", errors="replace"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 2)
        if len(parts) < 2 or parts[1] not in STATUSES:
            continue
        rows[parts[0]] = (parts[1], parts[2] if len(parts) > 2 else "")
    return rows


# --- evidence: which cfg sites of a feature the Pi image can actually reach -------------

def site_verdicts(feat, src, ctx):
    """(relpath, line, PI-LIVE|X86|PROSE|UNREACHED-FILE, text) per `feature = "feat"` site."""
    out = subprocess.run(["grep", "-rn", 'feature = "%s"' % feat, src],
                         capture_output=True, text=True).stdout
    res = []
    for ln in out.split("\n"):
        if not ln.strip():
            continue
        path, lno, text = ln.split(":", 2)
        rel = os.path.relpath(path, src)
        pos = text.find('feature = "%s"' % feat)
        cpos = text.find("//")
        if text.lstrip().startswith("//") or (cpos != -1 and pos > cpos):
            res.append((rel, lno, "PROSE", text)); continue
        arch = ctx.get(rel)
        if arch is None:
            res.append((rel, lno, "UNREACHED-FILE", text)); continue
        if arch == {"x86_64"}:
            res.append((rel, lno, "X86", text)); continue
        near = enclosing_arch(text, pos)
        res.append((rel, lno, "X86" if (near and "aarch64" not in near) else "PI-LIVE", text))
    return res


def enclosing_arch(line, pos):
    """target_arch names on the innermost bracket group around `pos`, else the whole line.

    `any(all(feature = "tegra", target_arch = "aarch64"), all(feature = "rastmc",
    target_arch = "x86_64"))` is aarch64-live for one feature and x86-only for the other,
    on one line; "does x86_64 appear here" answers both wrong.
    """
    opens = []
    for i, ch in enumerate(line):
        if ch == "(":
            opens.append(i)
        elif ch == ")" and opens:
            o = opens.pop()
            if o < pos < i:
                ar = re.findall(r'target_arch\s*=\s*"([a-z0-9_]+)"', line[o:i])
                if ar:
                    return set(ar)
    ar = re.findall(r'target_arch\s*=\s*"([a-z0-9_]+)"', line)
    return set(ar) if ar else None


def load_ctx(src):
    """The module-tree arch context, loaded out of the sibling script by path (its name is
    not an importable identifier)."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("k8modtree", os.path.join(HERE, "k8-modtree.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    ctx, _, _ = mod.walk(src)
    return {rel: set(a) for rel, a in ctx.items()}


SITE_CONTROL = "wc"


def site_scan_control(src):
    """Refuse to report sites at all unless a feature that certainly exists returns some.

    A wrong `--root` makes every `grep -rn` come back empty, and "0 site(s), 0 Pi-live" then
    reads as a clean answer about the tree instead of no answer at all -- the exact failure
    this file's own control exists to prevent, which it walked into once (rmbp 14: a staged
    tree one directory deep produced seven confident zeroes).
    """
    if not os.path.isdir(src):
        return "the kernel source directory %s does not exist" % src
    out = subprocess.run(["grep", "-rn", 'feature = "%s"' % SITE_CONTROL, src],
                         capture_output=True, text=True).stdout
    if not out.strip():
        return ("the site scan found no `feature = \"%s\"` anywhere under %s; a scan that "
                "matches nothing reports every knob as siteless" % (SITE_CONTROL, src))
    return None


def cargo_implications(cargo):
    """feature -> the features it pulls in transitively, out of `[features]`.

    arroyo's `_feats` line shows only what the KNOB names; Cargo pulls the rest. orin 17 found
    the gap the hard way: `UNAOS_GA10B_PROBE2` arms `ga10bprobe2`, and `ga10bprobe2 = ["tegra"]`
    makes it a tegra knob that no arroyo line says is one. A `--evidence` run that does not print
    the closure invites exactly the ruling that misses it.
    """
    deps, inf = {}, False
    for line in open(cargo, encoding="utf8", errors="replace"):
        if line.startswith("[features]"):
            inf = True; continue
        if line.startswith("["):
            inf = False; continue
        if not inf:
            continue
        m = re.match(r'^([A-Za-z0-9_-]+)[ \t]*=[ \t]*\[(.*)\]', line)
        if m:
            deps[m.group(1)] = [d.strip().strip('"') for d in m.group(2).split(",")
                                if d.strip() and "dep:" not in d and "/" not in d]

    def closure(f, seen=None):
        seen = seen if seen is not None else set()
        for d in deps.get(f, []):
            if d not in seen:
                seen.add(d)
                closure(d, seen)
        return seen
    return closure


def evidence(name, knob_feats, src):
    ctx = load_ctx(src)
    closure = cargo_implications(os.path.join(os.path.dirname(src), "Cargo.toml"))
    feats = sorted(knob_feats.get(name, {name}))
    for f in feats:
        rows = site_verdicts(f, src, ctx)
        live = sum(1 for r in rows if r[2] == "PI-LIVE")
        imp = sorted(closure(f))
        print("%s: %d site(s), %d Pi-live%s"
              % (f, len(rows), live, ("  [implies: %s]" % ", ".join(imp)) if imp else ""))
        for rel, lno, v, text in rows:
            print("  %-14s %s:%s  %s" % (v, rel, lno, text.strip()[:100]))


def main():
    argv = sys.argv[1:]
    root = ROOT
    if "--root" in argv:
        i = argv.index("--root"); root = os.path.abspath(argv[i + 1]); del argv[i:i + 2]
    arroyo = os.path.join(root, "arroyo")
    registry = os.path.join(root, "scripts", "k8-reach.registry")
    src = os.path.join(root, "crates", "kernel", "src")

    knob_feats, armed = parse_arroyo(arroyo)
    if knob_feats is None:
        print("%s⚠ k8-reach: kernel8() bounds did not resolve in %s — NO VERDICT%s" % (YELLOW, arroyo, OFF))
        return 2
    if len(knob_feats) < 50:
        print("%s⚠ k8-reach: the _feats parse found %d knobs (expected >= 50) — NO VERDICT%s"
              % (YELLOW, len(knob_feats), OFF))
        return 2
    for c in CANARIES:
        if c not in knob_feats:
            print("%s⚠ k8-reach: control failed — %s is not in the parsed _feats map; both SR1 "
                  "instances must parse or this gate is reporting about nothing — NO VERDICT%s"
                  % (YELLOW, c, OFF))
            return 2
    if len(armed) < 20:
        print("%s⚠ k8-reach: the kernel8() arm parse found %d knobs (expected >= 20); every knob "
              "would read as unarmed — NO VERDICT%s" % (YELLOW, len(armed), OFF))
        return 2

    if argv and argv[0] == "--evidence":
        why = site_scan_control(src)
        if why:
            print("%s⚠ k8-reach --evidence: %s — NO VERDICT%s" % (YELLOW, why, OFF))
            return 2
        evidence(argv[1], knob_feats, src)
        return 0

    unarmed = sorted(k for k in knob_feats if k not in armed)
    if argv and argv[0] == "--seed":
        why = site_scan_control(src)
        if why:
            print("%s⚠ k8-reach --seed: %s — NO VERDICT%s" % (YELLOW, why, OFF))
            return 2
        ctx = load_ctx(src)
        print("# GATE-K8REACH registry — every UNAOS_* knob with no K8_FEATS arm, and why.")
        print("# NA <reason> = deliberately absent from the Pi bare-metal image.")
        print("# TODO <owner/note> = grandfathered at seeding, nobody has ruled yet.")
        print("# `scripts/k8-reach.py --evidence <KNOB>` prints the sites behind a row.")
        for k in unarmed:
            live = 0
            for f in sorted(knob_feats[k]):
                live += sum(1 for r in site_verdicts(f, src, ctx) if r[2] == "PI-LIVE")
            print("%-26s TODO  seeded 2026-09-06; %s"
                  % (k, "no Pi-live cfg site" if live == 0 else "%d Pi-live cfg site(s)" % live))
        return 0

    rows = parse_registry(registry)
    if not rows:
        print("%s⚠ k8-reach: %s is missing or has no rows — NO VERDICT%s" % (YELLOW, registry, OFF))
        return 2

    unregistered = [k for k in unarmed if k not in rows]
    stale = [k for k in rows if k not in knob_feats]
    contradiction = [k for k in rows if k in armed]

    branch = subprocess.run(["git", "-C", root, "rev-parse", "--abbrev-ref", "HEAD"],
                            capture_output=True, text=True).stdout.strip()
    env_strict = os.environ.get("UNAOS_K8REACH_STRICT")
    if env_strict == "0":
        strict, why = False, "suppressed by UNAOS_K8REACH_STRICT=0"
    elif env_strict == "1":
        strict, why = True, "forced by UNAOS_K8REACH_STRICT=1"
    elif branch == TRUNK:
        strict, why = True, "automatic: on the trunk branch `%s`, where every branch's knobs are present" % TRUNK
    else:
        strict, why = False, "off: branch `%s` is a track branch, cross-branch rows deferred" % (branch or "<unknown>")
    deferred = []
    if not strict and stale:
        deferred, stale = stale, []

    if unregistered:
        print("%s❌ k8-reach UNREGISTERED: %s — knob(s) with no K8_FEATS arm and no registry row. "
              "Setting one and flashing kernel8 changes nothing, silently (LEDGER SR1). Add the "
              "arm in kernel8(), or a row in scripts/k8-reach.registry saying why not "
              "(`--evidence <KNOB>` prints the sites).%s" % (RED, " ".join(unregistered), OFF))
    if stale:
        print("%s❌ k8-reach STALE: %s — registry row(s) for knob(s) that are not in the _feats "
              "map (%s). On the trunk every branch's knobs are present, so a row nothing matches "
              "is dead: delete it.%s" % (RED, " ".join(stale), why, OFF))
    if contradiction:
        print("%s❌ k8-reach CONTRADICTION: %s — armed in kernel8() AND registered as unarmed. "
              "The row is wrong; delete it.%s" % (RED, " ".join(contradiction), OFF))
    if unregistered or stale or contradiction:
        return 1

    if deferred:
        print("  k8-reach DEFERRED — %d row(s) for knob(s) this tree does not have; NOT findings. "
              "Strict is %s; these become reds automatically on the trunk: %s"
              % (len(deferred), why, " ".join(sorted(deferred))))
    todo = sum(1 for k, (s, _) in rows.items() if s == "TODO")
    print("%s  ✅ k8 reachability (%d knobs: %d armed, %d registered unarmed — %d still TODO)%s"
          % (GREEN, len(knob_feats), len(armed & set(knob_feats)), len(rows), todo, OFF))
    return 0


if __name__ == "__main__":
    sys.exit(main())
