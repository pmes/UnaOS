#!/usr/bin/env bash
# test-roots.sh — GATE-TESTROOTS: every TEST SUITE in this tree NAMES THE COMMAND THAT RUNS IT.
#
# WHY THIS EXISTS (TESTROOTS, rmbp seat, 2026-09-16). This is GATE-ROOTS' argument — "a binary is
# nobody's dependency, so a binary no leg names is never type-checked" — applied to the third kind
# of root, the one FOREMAN-ROOT found by hand and closed for exactly one file. A `#[test]` is
# nobody's dependency either: `cargo build` never compiles it, the kernel legs never link it, and
# unless some command RUNS it, it is a block of assertions that no run ever evaluates.
# `tools/foreman/tests/agreement.rs` was that file — checked in with the module it guards, named in
# no line of `arroyo`, and RED 21/21 for weeks while the tree read as "check green". FOREMAN-ROOT
# fixed the instance and wrote in its own QUEUE §5 row that "the sweep of the rest of that class
# (every #[test], tests/*.rs and script suite no verb names) is owed". This is that sweep, and it is
# the CLASS: a suite added tomorrow reds this gate the day it appears.
#
# A checked-in test that nothing runs is strictly worse than no test. It reads as coverage to every
# reader of the tree, it is quoted in reviews as if it had fired, and it can hold a kernel line
# hostage the way an unrun spec does (SPECRUN) — while catching nothing. The two failure modes are
# not symmetric: an absent test is visibly absent, an unrun one is invisibly absent.
#
# WHAT IT ENUMERATES, over the WHOLE repo (root workspace and `unaos/` alike), by DIRECTORY and not
# by workspace membership — a crate that is not even a member is the worse case, not an exemption:
#
#   <pkg>::unit        a crate whose `src/**` carries a `#[test]` or a `#[cfg(test)]` module. One
#                      suite per crate: that is the granularity `cargo test -p <pkg> --lib` has.
#   <pkg>::<name>      each `tests/<name>.rs` — cargo's integration targets, one binary each.
#   script:<path> …    a script suite: `*_test.sh` / `*-test.sh` / `test_*.sh` / `run_test.sh` /
#                      `*selftest*.sh`, and any `unaos/scripts/*.py` that declares `--self-test`.
#
# HOW IT DECIDES. Three ways for a suite to have a runner, and exactly three:
#
#   GATED      `arroyo` runs it in CODE — a `cargo test` line whose `-p`/`--workspace` reaches the
#              crate and whose target filter (`--test`, `--lib`, none) reaches the target; or, for a
#              script suite, a line naming both the script and its self-test flag. Full-line
#              comments are stripped first and backslash continuations are joined first, because the
#              two real legs in this tree need both: GATE-CORE's `cargo test -p midden_core` is a
#              one-liner and GATE-FOREMAN's `cargo test -p foreman --test agreement` sits on the
#              continuation of the `cd` that positions it.
#
#              THE TARGET FILTER IS HONOURED, and that is the half a coarser parser gets wrong.
#              `cargo test -p foreman --test agreement` runs `tests/agreement.rs` and NOT
#              `tests/preflight.rs`; a gate that read `-p foreman` alone would call both covered and
#              would have declared FOREMAN-ROOT's own class closed while `preflight.rs` stayed unrun.
#              That pair is a CONTROL below, not a comment.
#
#   DECLARED   the suite's own header carries a `# RUN-BY: <who> — <command>` line (the
#              GATE-SPECROOTS vocabulary, same three tokens): `bench` (a human at a metal bench),
#              `knobleg` (a leg behind a knob the default gate does not arm) or `script:<path>`,
#              whose path must EXIST or the declaration is dead. `verb:<name>` is refused outright —
#              a suite that claims a verb runs it must be findable in `arroyo`'s code, or the claim
#              is false and this gate says so. For a `<pkg>::unit` suite the header is read from the
#              crate's `src/lib.rs` or `src/main.rs`; for the rest, from the file itself.
#
#   REGISTERED a row in `scripts/tests.registry`: `<suite id>  <reason>`. That file is for the two
#              things a runner cannot fix — a suite that CANNOT run on this host (the `no_std`
#              kernel crates: their `#[cfg(test)]` bodies are never compiled by any build this tree
#              performs, on any target) and a suite that is RED today, which is reported by NAME
#              with the id of the cut that owns the fix rather than silently skipped. It is not an
#              allowlist to grow; read its header.
#
# Neither -> ORPHAN -> exit 1, by name.
#
# CONTROL PROBES, the GATE-ROOTS idiom — a resolver that matched EVERYTHING would call every suite
# gated and be silently green forever, and one that matched NOTHING would red every suite (loud, but
# "fixable" by registering everything). Both are ruled out before any verdict, on synthetic input
# whose answer is known, plus facts about THIS tree the parser has to rediscover:
#   (a) a `cargo test -p X` inside a full-line comment must NOT cover X; the same line in code MUST.
#   (b) `--test agreement` must cover `foreman::agreement` and must NOT cover `foreman::preflight`.
#   (c) `--workspace --exclude Y` must cover a member and must NOT cover Y.
#   (d) against the REAL arroyo: `midden_core::unit` must resolve GATED (GATE-CORE runs it) and
#       `foreman::agreement` must resolve GATED (GATE-FOREMAN runs it).
#   (e) the enumeration must be non-empty, and a registry file that exists must parse.
# Any control failure is exit 2 and NO verdict, because a gate that did not run is not a pass.
#
# usage: test-roots.sh [repo-root]   (0 every suite has a runner; 1 an orphan; 2 control failed)
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="${1:-$(cd "$HERE/../.." && pwd)}"
REGISTRY="${TESTROOTS_REGISTRY:-$HERE/tests.registry}"

[ -f "$REPO/unaos/arroyo" ] || { echo "GATE-TESTROOTS: control FAILED — no arroyo at $REPO/unaos/arroyo. No verdict." >&2; exit 2; }

python3 - "$REPO" "$REGISTRY" <<'PY'
import os, re, sys

REPO = os.path.abspath(sys.argv[1])
REGISTRY = sys.argv[2]
ARROYO = os.path.join(REPO, "unaos", "arroyo")

def rel(p):
    r = os.path.relpath(p, REPO)
    return p if r.startswith("..") else r

def die2(msg):
    print("GATE-TESTROOTS: control FAILED — %s No verdict." % msg, file=sys.stderr)
    sys.exit(2)

# ─────────────────────────────────────────────────────────────── 1. the crates and their suites ──
SKIP_DIRS = {"target", ".git", "node_modules"}

def walk_manifests(root):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and not d.startswith(".")]
        if "Cargo.toml" in filenames:
            yield os.path.join(dirpath, "Cargo.toml")

def pkg_name(manifest):
    inpkg = False
    try:
        with open(manifest, errors="replace") as fh:
            for line in fh:
                s = line.strip()
                if s.startswith("["):
                    inpkg = s.startswith("[package]")
                    continue
                if inpkg:
                    m = re.match(r'name\s*=\s*"([^"]+)"', s)
                    if m:
                        return m.group(1)
    except OSError:
        return None
    return None

def ws_members(manifest):
    """members = [...] of a [workspace] table, as absolute crate dirs."""
    out = []
    base = os.path.dirname(manifest)
    try:
        text = open(manifest, errors="replace").read()
    except OSError:
        return out
    m = re.search(r'^\[workspace\]\s*$(.*?)(?=^\[|\Z)', text, re.M | re.S)
    if not m:
        return out
    mm = re.search(r'^\s*members\s*=\s*\[(.*?)\]', m.group(1), re.M | re.S)
    if not mm:
        return out
    for line in mm.group(1).splitlines():
        line = line.split("#", 1)[0]
        for p in re.findall(r'"([^"]+)"', line):
            out.append(os.path.normpath(os.path.join(base, p)))
    return out

WS = {
    "root":  set(ws_members(os.path.join(REPO, "Cargo.toml"))),
    "unaos": set(ws_members(os.path.join(REPO, "unaos", "Cargo.toml"))),
}

# An attribute, not a mention of one. The anchor is what keeps PROSE out: `crates/kernel` carries
# five comment blocks explaining that its `#[cfg(test)] mod tests` were deleted deliberately
# (DECRUD-2 — "the four `#[test]` fns had never been compiled, let alone run, on either arch"), and
# an unanchored match reads every one of them as a suite. GATE-SPECROOTS' lesson, one layer down.
TEST_RE = re.compile(r'^[ \t]*#\[\s*(?:test\s*\]|cfg\s*\(\s*test\s*\)\s*\])', re.M)

def src_has_tests(cdir):
    src = os.path.join(cdir, "src")
    if not os.path.isdir(src):
        return None
    for dirpath, dirnames, filenames in os.walk(src):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for f in sorted(filenames):
            if not f.endswith(".rs"):
                continue
            p = os.path.join(dirpath, f)
            try:
                if TEST_RE.search(open(p, errors="replace").read()):
                    return p
            except OSError:
                pass
    return None

crates = {}       # dir -> {pkg, ws}
suites = []       # {id, kind, pkg, dir, target, file}
for manifest in sorted(walk_manifests(REPO)):
    cdir = os.path.dirname(manifest)
    name = pkg_name(manifest)
    if not name:
        continue                                   # a pure [workspace] manifest
    which = None
    for w, members in WS.items():
        if cdir in members:
            which = w
            break
    # Suite ids are `<pkg>::<target>`, so two crates sharing a package name would silently MERGE
    # into one row and one of them would be laundered by the other's runner. There are none today
    # (`handlers/helm` is `helm-handler`, not `helm`), and the day there is, this gate says it
    # cannot answer rather than answering wrongly.
    for other, meta in crates.items():
        if meta["pkg"] == name:
            die2("two crates share the package name %r (%s and %s), so the suite id space is "
                 "ambiguous and a runner for one would launder the other." %
                 (name, rel(other), rel(cdir)))
    crates[cdir] = {"pkg": name, "ws": which}
    hit = src_has_tests(cdir)
    if hit:
        suites.append({"id": "%s::unit" % name, "kind": "unit", "pkg": name,
                       "dir": cdir, "target": None, "file": hit})
    tdir = os.path.join(cdir, "tests")
    if os.path.isdir(tdir):
        for f in sorted(os.listdir(tdir)):
            if f.endswith(".rs"):
                t = f[:-3]
                suites.append({"id": "%s::%s" % (name, t), "kind": "integration", "pkg": name,
                               "dir": cdir, "target": t, "file": os.path.join(tdir, f)})

# Script suites. `test-roots.sh` and the other gate scripts are NOT suites — a gate is run by
# `check` by definition; the patterns below are the names a reader would read as "a test".
SCRIPT_PATTERNS = (
    re.compile(r'.*_test\.sh$'), re.compile(r'.*-test\.sh$'),
    re.compile(r'^test_.*\.sh$'), re.compile(r'^run_test\.sh$'),
    re.compile(r'.*selftest.*\.sh$'),
)
for dirpath, dirnames, filenames in os.walk(REPO):
    dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and not d.startswith(".")]
    for f in sorted(filenames):
        p = os.path.join(dirpath, f)
        if any(pat.match(f) for pat in SCRIPT_PATTERNS):
            suites.append({"id": "script:%s" % rel(p), "kind": "script", "pkg": None,
                           "dir": dirpath, "target": None, "file": p, "flag": None})
        elif f.endswith(".py") and os.path.dirname(p) == os.path.join(REPO, "unaos", "scripts"):
            try:
                body = open(p, errors="replace").read()
            except OSError:
                continue
            if '"--self-test"' in body or "'--self-test'" in body:
                suites.append({"id": "script:%s --self-test" % rel(p), "kind": "script", "pkg": None,
                               "dir": dirpath, "target": None, "file": p, "flag": "--self-test"})

suites.sort(key=lambda s: s["id"])

# ────────────────────────────────────────────────────── 2. the resolver: what does arroyo RUN? ──
def arroyo_code(text):
    """-> [(physical line number of the line that STARTED this logical line, text)]. Full-line
    comments dropped, backslash continuations joined. All three are load-bearing: the comment strip
    is what stops prose from reading as a leg (GATE-SPECROOTS' lesson), the join is what finds
    GATE-FOREMAN's leg, whose `cd` and `cargo test` sit on two physical lines, and the retained
    number is what lets the census name a `arroyo:<line>` a reader can open."""
    out, buf, start = [], "", 0
    for n, line in enumerate(text.splitlines(), 1):
        if re.match(r'^[ \t]*#', line):
            continue
        if buf == "":
            start = n
        if line.endswith("\\"):
            buf += line[:-1] + " "
            continue
        out.append((start, buf + line))
        buf = ""
    if buf:
        out.append((start, buf))
    return out

def cargo_test_coverage(lines, ws_pkgs):
    """-> {suite-id-prefix-key: [(kind, detail)]}. Returns a matcher closure instead of a set so
    the same code answers both the controls and the verdict.

    ws_pkgs: {workspace-name: [pkg, ...]} for expanding `--workspace`."""
    cov = []    # list of (pkgset, targetfilter, lineno, fn)
    fn = "(top level)"
    for i, line in lines:
        m = re.match(r'^([A-Za-z_][A-Za-z0-9_]*)\(\)[ \t]*\{', line)
        if m:
            fn = m.group(1)
        if "cargo test" not in line:
            continue
        seg = line.split("cargo test", 1)[1]
        if "/.." in line and re.search(r'cd\s+"\$\{?WORKSPACE_DIR\}?/\.\."', line):
            w = "root"
        elif re.search(r'cd\s+"\$\{?WORKSPACE_DIR\}?"', line):
            w = "unaos"
        else:
            w = "unaos"
        pkgs = set(re.findall(r'(?:^|\s)-p\s+([A-Za-z0-9_.-]+)', seg))
        pkgs |= set(re.findall(r'(?:^|\s)--package[= ]([A-Za-z0-9_.-]+)', seg))
        if re.search(r'(?:^|\s)--(workspace|all)(?:\s|$)', seg):
            excl = set(re.findall(r'(?:^|\s)--exclude[= ]([A-Za-z0-9_.-]+)', seg))
            pkgs |= set(ws_pkgs.get(w, [])) - excl
            pkgs -= excl
        # Target selection is ADDITIVE in cargo: `--lib --test preflight` selects both. So the
        # question is never "which one filter" but "was ANY target filter given" — with none, every
        # target of the package runs.
        tests = set(re.findall(r'(?:^|\s)--test[= ]([A-Za-z0-9_.-]+)', seg))
        unit = bool(re.search(r'(?:^|\s)--(lib|bins|all-targets)(?:\s|$)', seg)) or \
            bool(re.search(r'(?:^|\s)--bin[= ]', seg))
        if re.search(r'(?:^|\s)--tests(?:\s|$)', seg):
            tests = None            # every integration target, and only those
        cov.append((pkgs, tests, unit, i, fn))
    return cov

def covered_by(cov, suite):
    """-> list of 'arroyo:<line> (<fn>)' strings naming every leg that runs this suite."""
    hits = []
    for pkgs, tests, unit, ln, fn in cov:
        if suite["kind"] == "script":
            continue
        if suite["pkg"] not in pkgs:
            continue
        filtered = unit or tests is None or bool(tests)
        if not filtered:
            hits.append("arroyo:%d (%s)" % (ln, fn))
            continue
        if suite["kind"] == "unit" and unit:
            hits.append("arroyo:%d (%s)" % (ln, fn))
        elif suite["kind"] == "integration" and (tests is None or suite["target"] in tests):
            hits.append("arroyo:%d (%s)" % (ln, fn))
    return hits

def script_covered_by(lines, suite):
    """A script suite is GATED when a line of arroyo CODE names the script AND, where the suite is
    a flag on a larger tool, that flag. `./arroyo mbench "$@"` is a passthrough, not a leg: it lets
    an operator type the self-test, and no run of `check` ever does."""
    base = os.path.basename(suite["file"])
    hits = []
    fn = "(top level)"
    for i, line in lines:
        m = re.match(r'^([A-Za-z_][A-Za-z0-9_]*)\(\)[ \t]*\{', line)
        if m:
            fn = m.group(1)
        if base not in line:
            continue
        if suite.get("flag") and suite["flag"] not in line:
            continue
        hits.append("arroyo:%d (%s)" % (i, fn))
    return hits

RUNBY = re.compile(r'^[ \t]*(?://|#)[ \t]*RUN-BY:[ \t]*(\S+)')

def run_by(suite):
    files = []
    if suite["kind"] == "unit":
        for c in ("src/lib.rs", "src/main.rs"):
            p = os.path.join(suite["dir"], c)
            if os.path.isfile(p):
                files.append(p)
    else:
        files.append(suite["file"])
    for p in files:
        try:
            with open(p, errors="replace") as fh:
                for line in fh:
                    m = RUNBY.match(line)
                    if m:
                        return m.group(1)
        except OSError:
            pass
    return ""

# ───────────────────────────────────────────────────────────────────── 3. control probes ────────
CTL_WS = {"root": ["ctlpkg", "ctlother", "foreman", "midden_core"], "unaos": []}

def ctl_suite(pkg, kind, target=None):
    return {"id": "x", "kind": kind, "pkg": pkg, "dir": "/nonexistent", "target": target,
            "file": "/nonexistent"}

_a = arroyo_code('# a comment mentioning cargo test -p ctlpkg\n')
if covered_by(cargo_test_coverage(_a, CTL_WS), ctl_suite("ctlpkg", "unit")):
    die2("a `cargo test -p ctlpkg` inside a FULL-LINE COMMENT resolved as GATED; the comment "
         "stripper is broken, so prose reads as a leg.")
_a = arroyo_code('    if (cd "$WORKSPACE_DIR/.." && cargo test -p ctlpkg); then\n')
if not covered_by(cargo_test_coverage(_a, CTL_WS), ctl_suite("ctlpkg", "unit")):
    die2("a `cargo test -p ctlpkg` in CODE resolved as NOT gated; the resolver matches nothing.")
_a = arroyo_code('    if (cd "$WORKSPACE_DIR/.." \\\n        && cargo test -p foreman --test agreement); then\n')
_c = cargo_test_coverage(_a, CTL_WS)
if not covered_by(_c, ctl_suite("foreman", "integration", "agreement")):
    die2("`--test agreement` across a backslash continuation did not cover foreman::agreement; "
         "the continuation join or the target filter is broken.")
if covered_by(_c, ctl_suite("foreman", "integration", "preflight")):
    die2("`--test agreement` ALSO covered foreman::preflight; the target filter does not narrow, "
         "so a one-target leg would launder every other target of its crate.")
if covered_by(_c, ctl_suite("foreman", "unit")):
    die2("`--test agreement` ALSO covered foreman::unit; the target filter does not narrow.")
_a = arroyo_code('    (cd "$WORKSPACE_DIR/.." && cargo test -p foreman --lib --test preflight)\n')
_c = cargo_test_coverage(_a, CTL_WS)
if not covered_by(_c, ctl_suite("foreman", "unit")) or \
        not covered_by(_c, ctl_suite("foreman", "integration", "preflight")):
    die2("`--lib --test preflight` did not cover BOTH foreman::unit and foreman::preflight; cargo "
         "target selection is ADDITIVE and this reader treats it as exclusive.")
if covered_by(_c, ctl_suite("foreman", "integration", "agreement")):
    die2("`--lib --test preflight` ALSO covered foreman::agreement; the target filter does not narrow.")
_a = arroyo_code('    (cd "$WORKSPACE_DIR/.." && cargo test --workspace --exclude ctlother)\n')
_c = cargo_test_coverage(_a, CTL_WS)
if not covered_by(_c, ctl_suite("ctlpkg", "unit")):
    die2("`--workspace` did not cover a member; the workspace expansion is broken.")
if covered_by(_c, ctl_suite("ctlother", "unit")):
    die2("`--workspace --exclude ctlother` still covered ctlother; --exclude is not honoured.")

try:
    ARR = arroyo_code(open(ARROYO, errors="replace").read())
except OSError as e:
    die2("could not read %s (%s)." % (ARROYO, e))

ws_pkgs = {}
for w, members in WS.items():
    ws_pkgs[w] = [crates[d]["pkg"] for d in members if d in crates]
COV = cargo_test_coverage(ARR, ws_pkgs)

if not suites:
    die2("zero test suites enumerated over %s; the enumerator is broken." % REPO)

by_id = {s["id"]: s for s in suites}
for must in ("midden_core::unit", "foreman::agreement"):
    if must not in by_id:
        die2("%s was not enumerated; the suite enumerator does not see this tree." % must)
    if not covered_by(COV, by_id[must]):
        die2("%s is run by a leg of check in this tree and did not resolve GATED; the arroyo "
             "reader is broken." % must)

registered = {}
if os.path.exists(REGISTRY):
    try:
        with open(REGISTRY, errors="replace") as fh:
            for n, raw in enumerate(fh, 1):
                s = raw.strip()
                if not s or s.startswith("#"):
                    continue
                parts = s.split(None, 1)
                if len(parts) < 2 or not parts[1].strip():
                    die2("%s:%d has no reason — a registry row without a reason is an allowlist, "
                         "and this table is unreadable as written." % (rel(REGISTRY), n))
                registered[parts[0]] = parts[1].strip()
    except OSError as e:
        die2("could not read the registry %s (%s)." % (REGISTRY, e))

# ────────────────────────────────────────────────────────────────────────── 4. verdict ──────────
rc = 0
orphans = []
n_gated = n_declared = n_registered = 0
print("GATE-TESTROOTS: test suites and the command that runs each —")
for s in suites:
    if s["kind"] == "script":
        hits = script_covered_by(ARR, s)
    else:
        hits = covered_by(COV, s)
    if hits:
        n_gated += 1
        print("  %-38s GATED (%s)" % (s["id"], ", ".join(hits[:2])))
        continue
    if s["id"] in registered:
        n_registered += 1
        # The census is scrollback inside `check`: print the reason's FIRST SENTENCE-ish, not the
        # whole measurement. The file is the record; this line is the reminder that it exists.
        why = registered[s["id"]]
        print("  %-38s REGISTERED — %s" % (s["id"], why if len(why) <= 96 else why[:95] + "…"))
        continue
    who = run_by(s)
    if who == "":
        print("  %-38s ORPHAN — no verb runs it, no RUN-BY, no registry row  [%s]"
              % (s["id"], rel(s["file"])))
        orphans.append(s["id"])
        rc = 1
    elif who.startswith("verb:"):
        print("  %-38s FALSE CLAIM — header says '%s' but arroyo names it nowhere in code"
              % (s["id"], who))
        orphans.append(s["id"])
        rc = 1
    elif who.startswith("script:"):
        sp = who[len("script:"):]
        if os.path.isfile(os.path.join(REPO, sp)) or os.path.isfile(sp):
            n_declared += 1
            print("  %-38s DECLARED (%s)" % (s["id"], who))
        else:
            print("  %-38s DEAD DECLARATION — %s does not exist" % (s["id"], who))
            orphans.append(s["id"])
            rc = 1
    elif who in ("bench", "knobleg"):
        n_declared += 1
        print("  %-38s DECLARED (%s)" % (s["id"], who))
    else:
        print("  %-38s BAD RUN-BY '%s' — want bench | knobleg | script:<path>" % (s["id"], who))
        orphans.append(s["id"])
        rc = 1

print("GATE-TESTROOTS: census suites=%d gated=%d declared=%d registered=%d orphan=%d"
      % (len(suites), n_gated, n_declared, n_registered, len(orphans)))

if rc:
    print("""
GATE-TESTROOTS FAILED — test suite(s) that no command runs: %s

A `#[test]` is its own root. Nothing depends on it, so unless a leg of `check` runs it or its header
names the command that does, no run ever evaluates its assertions — and it still costs, because it
reads as coverage to every reader of the tree and can be red for weeks without anyone learning
(FOREMAN-ROOT: `tools/foreman/tests/agreement.rs`, red 21/21, named nowhere).
  FIX: run it from a leg of `check_both` (see GATE-TESTROOTS' runner leg in `arroyo`), or
       add a header line naming the command that does:
         # RUN-BY: bench — <the command a human runs at a metal bench>
         # RUN-BY: knobleg — <the knob-armed leg that runs it>
         # RUN-BY: script:scripts/<tool>.sh — <the exact invocation>
       A suite that CANNOT run here, or one that is RED today, goes in scripts/tests.registry with
       its reason and the id of the cut that owns the fix — reported by name every run, never
       silently skipped. Deleting a suite nobody has run is also a fix, and an honest one.""" % (
          " ".join(orphans)), file=sys.stderr)
sys.exit(rc)
PY
