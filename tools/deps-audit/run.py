#!/usr/bin/env python3
"""DEPS audit (SR31, R83; LAWS §Dependencies).

Walks every Cargo.toml in the tree, resolves each third-party dependency against
crates.io's sparse index (latest stable, latest pre-release when newer), reads the
version each workspace's Cargo.lock actually resolved, classifies the crate as
UTILITY or CHICKEN-WIRE (tools/deps-audit/classify.toml), and writes docs/dev/DEPS.md.

    run.py                 write docs/dev/DEPS.md
    run.py --check         gate: exit 1 when a dependency is behind latest stable
                           beyond --policy (default `any`), unless pinned in
                           tools/deps-audit/pins.toml (reason + ledger id)
    run.py --policy major  only a semver-incompatible lag fails the gate
    run.py --offline       use the index cache only (tools/deps-audit/.cache/)
    run.py --refresh       ignore the cache's age and refetch every crate
    run.py --stdout        print the report instead of writing DEPS.md

Policy `any` fails a row when EITHER the declared requirement does not admit the
latest stable (a manifest edit is owed) OR the workspace lockfile resolved an
older version than the latest stable (a `cargo update` is owed).
"""
from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import ssl
import subprocess
import sys
import time
import tomllib
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
CACHE = HERE / ".cache" / "index"
CACHE_TTL = 24 * 3600
OUT = ROOT / "docs" / "dev" / "DEPS.md"
SKIP_DIRS = {"target", ".git", ".claude", "node_modules", "envytools"}
SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")

# ----------------------------------------------------------------------------- semver


@dataclass(frozen=True, order=True)
class Ver:
    major: int
    minor: int
    patch: int
    pre: tuple = ()  # () sorts AFTER any pre-release: handled in key()
    raw: str = field(default="", compare=False)

    @staticmethod
    def parse(s: str) -> "Ver | None":
        m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?", s.strip())
        if not m:
            return None
        pre = ()
        if m.group(4):
            pre = tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in m.group(4).split("."))
        return Ver(int(m.group(1)), int(m.group(2)), int(m.group(3)), pre, s.strip())

    @property
    def is_pre(self) -> bool:
        return bool(self.pre)

    def key(self):
        # a release outranks its own pre-releases
        return (self.major, self.minor, self.patch, 0 if self.pre else 1, self.pre)

    def __str__(self):
        return self.raw or f"{self.major}.{self.minor}.{self.patch}"


def _partial(s: str):
    parts = s.strip().split(".")
    nums = []
    pre = ""
    for i, p in enumerate(parts):
        if p in ("*", "x", "X"):
            break
        if "-" in p and i == len(parts) - 1 or (i < 3 and "-" in p):
            p, pre = p.split("-", 1)
            pre = "-" + ".".join([pre] + parts[i + 1:])
            nums.append(int(p))
            break
        nums.append(int(p))
    return nums, pre


def _cmp_ver(nums, pre):
    full = nums + [0] * (3 - len(nums))
    return Ver.parse(".".join(map(str, full[:3])) + pre)


def compat_upper(nums):
    """exclusive upper bound of a caret requirement with these leading numbers"""
    n = nums + []
    if len(n) == 1:
        return Ver(n[0] + 1, 0, 0)
    if n[0] > 0:
        return Ver(n[0] + 1, 0, 0)
    if len(n) == 2:
        return Ver(0, n[1] + 1, 0)
    if n[1] > 0:
        return Ver(0, n[1] + 1, 0)
    return Ver(0, 0, n[2] + 1)


def req_matches(req: str, v: Ver) -> bool:
    """cargo's requirement semantics, enough for every form used in this tree"""
    if req.strip() in ("", "*"):
        return not v.is_pre
    for clause in req.split(","):
        c = clause.strip()
        m = re.match(r"(\^|~|=|>=|<=|>|<)?\s*(.*)", c)
        op, body = m.group(1) or "^", m.group(2)
        nums, pre = _partial(body)
        lo = _cmp_ver(nums, pre)
        k = v.key()
        if op in ("^", "~"):
            if op == "^":
                hi = compat_upper(nums)
            else:
                hi = Ver(nums[0] + 1, 0, 0) if len(nums) == 1 else Ver(nums[0], nums[1] + 1, 0)
            if not (lo.key() <= k < hi.key()):
                return False
        elif op == "=":
            if len(nums) == 3:
                if k != lo.key():
                    return False
            else:
                hi = compat_upper(nums) if len(nums) == 1 else Ver(nums[0], nums[1] + 1, 0)
                if not (lo.key() <= k < hi.key()):
                    return False
        elif op == ">=" and not k >= lo.key():
            return False
        elif op == ">" and not k > lo.key():
            return False
        elif op == "<" and not k < lo.key():
            return False
        elif op == "<=" and not k <= lo.key():
            return False
        # a pre-release only matches a requirement that names a pre-release of the same triple
        if v.is_pre and not (pre and (lo.major, lo.minor, lo.patch) == (v.major, v.minor, v.patch)):
            return False
    return True


def bump_kind(old: Ver, new: Ver) -> str:
    """'major' when the move is semver-incompatible (cargo's caret rule), else 'minor'/'patch'"""
    if old.key() >= new.key():
        return "none"
    if old.major != new.major or (old.major == 0 and old.minor != new.minor) or (
        old.major == 0 and old.minor == 0 and old.patch != new.patch
    ):
        return "major"
    return "minor" if old.minor != new.minor else "patch"


# ----------------------------------------------------------------------------- crates.io


def _ssl_ctx():
    for var in ("SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"):
        p = os.environ.get(var)
        if p and os.path.exists(p):
            return ssl.create_default_context(cafile=p)
    return ssl.create_default_context()


def index_path(name: str) -> str:
    n = name.lower()
    if len(n) <= 2:
        return f"{len(n)}/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[:2]}/{n[2:4]}/{n}"


def fetch_versions(name: str, offline: bool, refresh: bool) -> list[Ver] | None:
    CACHE.mkdir(parents=True, exist_ok=True)
    cp = CACHE / f"{name.lower()}.json"
    if cp.exists() and (offline or (not refresh and time.time() - cp.stat().st_mtime < CACHE_TTL)):
        data = json.loads(cp.read_text())
    elif offline:
        return None
    else:
        url = f"https://index.crates.io/{index_path(name)}"
        req = urllib.request.Request(url, headers={"User-Agent": "unaos-deps-audit (SR31)"})
        try:
            with urllib.request.urlopen(req, context=_ssl_ctx(), timeout=30) as r:
                body = r.read().decode()
        except Exception as e:  # noqa: BLE001
            if cp.exists():
                data = json.loads(cp.read_text())
            else:
                print(f"deps-audit: cannot fetch {name}: {e}", file=sys.stderr)
                return None
        else:
            data = []
            for line in body.splitlines():
                if line.strip():
                    j = json.loads(line)
                    data.append({"v": j["vers"], "y": j.get("yanked", False)})
            cp.write_text(json.dumps(data))
    out = []
    for d in data:
        if d["y"]:
            continue
        v = Ver.parse(d["v"])
        if v:
            out.append(v)
    return out


# ----------------------------------------------------------------------------- tree walk


@dataclass
class Use:
    crate: str          # crates.io name
    consumer: str       # package name of the manifest
    manifest: str       # repo-relative path
    section: str        # dependencies / dev-dependencies / build-dependencies (+ target cfg)
    req: str
    optional: bool
    workspace: str | None  # repo-relative workspace root dir, None = orphan manifest
    locked: list[Ver] = field(default_factory=list)


def manifests() -> list[Path]:
    out = []
    for dp, dns, fns in os.walk(ROOT):
        dns[:] = sorted(d for d in dns if d not in SKIP_DIRS)
        if "Cargo.toml" in fns:
            out.append(Path(dp) / "Cargo.toml")
    return sorted(out)


def _member(ws_dir: Path, ws: dict, crate_dir: Path) -> str:
    """'member', 'excluded' (cargo keeps looking upward) or 'outside'"""
    if crate_dir == ws_dir:
        return "member"
    rel = crate_dir.relative_to(ws_dir).as_posix()
    for ex in ws.get("exclude", []):
        if rel == ex or rel.startswith(ex.rstrip("/") + "/"):
            return "excluded"
    return "member" if any(fnmatch.fnmatch(rel, m) for m in ws.get("members", [])) else "outside"


def workspace_of(path: Path, tomls: dict[Path, dict]) -> Path | None:
    d = path.parent
    for anc in [d, *d.parents]:
        t = tomls.get(anc / "Cargo.toml")
        if t is not None and "workspace" in t:
            m = _member(anc, t["workspace"], d)
            if m == "member":
                return anc
            if m == "outside":
                return None  # cargo refuses to build it as is (e.g. a commented-out member)
        if anc == ROOT:
            break
    return None


def parse_lock(p: Path) -> dict[str, list[Ver]]:
    out: dict[str, list[Ver]] = {}
    if not p.exists():
        return out
    t = tomllib.loads(p.read_text())
    for pk in t.get("package", []):
        if str(pk.get("source", "")).startswith(("registry+", "sparse+")):
            v = Ver.parse(pk["version"])
            if v:
                out.setdefault(pk["name"], []).append(v)
    return out


UNSEEN: list[str] = []


def selftest() -> int:
    """GATEREVIEW F12: a git dependency and an unparsable manifest must each be named (no network needed)."""
    global ROOT
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        ROOT = Path(d)
        (ROOT / "a").mkdir(); (ROOT / "b").mkdir()
        (ROOT / "a/Cargo.toml").write_text('[package]\nname="a"\nversion="0.1.0"\n[dependencies]\nleft = { git = "https://x.invalid/l" }\n')
        (ROOT / "b/Cargo.toml").write_text('[package]\nname="b"\n[dependencies]\nserde = \n')
        collect()
    ok = any("git dependency" in u for u in UNSEEN) and any("unparsable" in u for u in UNSEEN)
    print(f"deps-audit selftest: {UNSEEN} -> {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def collect() -> list[Use]:
    tomls = {}
    for m in manifests():
        try:
            tomls[m] = tomllib.loads(m.read_text())
        except tomllib.TOMLDecodeError as e:
            print(f"deps-audit: {m}: {e}", file=sys.stderr)
            UNSEEN.append(f"{m.relative_to(ROOT).as_posix()}: unparsable manifest ({e})")  # GATEREVIEW F12
    locks: dict[Path, dict] = {}
    uses: list[Use] = []
    for m, t in tomls.items():
        rel = m.relative_to(ROOT).as_posix()
        pkg = t.get("package", {}).get("name") or f"[workspace {m.parent.relative_to(ROOT).as_posix() or '.'}]"
        ws = workspace_of(m, tomls)
        wsd = ws.relative_to(ROOT).as_posix() if ws else None
        if ws is not None and ws not in locks:
            locks[ws] = parse_lock(ws / "Cargo.lock")
        wsdeps = tomls.get(ws / "Cargo.toml", {}).get("workspace", {}).get("dependencies", {}) if ws else {}

        tables = [(s, t.get(s, {})) for s in SECTIONS]
        for cfg, tt in t.get("target", {}).items():
            tables += [(f"{s} [{cfg}]", tt.get(s, {})) for s in SECTIONS]
        tables.append(("workspace.dependencies", t.get("workspace", {}).get("dependencies", {})))
        for sec, table in tables:
            for key, spec in table.items():
                if isinstance(spec, str):
                    spec = {"version": spec}
                if spec.get("workspace"):
                    base = wsdeps.get(key, {})
                    base = {"version": base} if isinstance(base, str) else dict(base)
                    base.update({k: v for k, v in spec.items() if k != "workspace"})
                    spec = base
                if "git" in spec:  # GATEREVIEW F12: a git dependency has no index row to lag — it was invisible
                    UNSEEN.append(f"{rel}: `{key}` is a git dependency (no crates.io version to audit)")
                if "path" in spec or "git" in spec or "version" not in spec:
                    continue  # in-tree crate (or git pin: none in this tree)
                name = spec.get("package", key)
                u = Use(name, pkg, rel, sec, spec["version"], bool(spec.get("optional")), wsd)
                if ws is not None:
                    u.locked = sorted((v for v in locks[ws].get(name, []) if req_matches(u.req, v)), key=Ver.key)
                uses.append(u)
    return uses


# ----------------------------------------------------------------------------- classify / pins


def load_toml(p: Path) -> dict:
    return tomllib.loads(p.read_text()) if p.exists() else {}


@dataclass
class Row:
    crate: str
    uses: list[Use]
    stable: Ver | None
    pre: Ver | None
    kind: str
    capability: str
    behind: list[str] = field(default_factory=list)  # human reasons this row lags
    lagging: set = field(default_factory=set)        # consumers behind latest stable
    worst: str = "none"

    def evaluate(self, policy: str):
        if self.stable is None:
            return
        for u in self.uses:
            where = f"{u.consumer} ({u.manifest})"
            lo = _cmp_ver(*_partial(u.req.lstrip("^~=<>").split(",")[0])) if u.req.strip() not in ("", "*") else None
            if lo is not None and lo.key() > self.stable.key():
                continue  # declared AHEAD of latest stable: a pre-release on purpose (R83), not a lag
            if not req_matches(u.req, self.stable):
                k = bump_kind(lo, self.stable) if lo else "major"
                self.behind.append(f"{where}: requirement `{u.req}` does not admit {self.stable} ({k})")
                self.lagging.add(u.consumer)
                if k == "major" or self.worst != "major":
                    self.worst = k if k != "none" else "major"
            elif policy == "any" and u.locked and max(u.locked, key=Ver.key).key() < self.stable.key():
                self.behind.append(f"{where}: lock resolved {max(u.locked, key=Ver.key)} < {self.stable}")
                self.lagging.add(u.consumer)
                if self.worst == "none":
                    self.worst = "lock"


def build_rows(offline: bool, refresh: bool, policy: str) -> list[Row]:
    cls = load_toml(HERE / "classify.toml").get("chicken_wire", {})
    by: dict[str, list[Use]] = {}
    for u in collect():
        by.setdefault(u.crate, []).append(u)
    rows = []
    for name in sorted(by, key=str.lower):
        vs = fetch_versions(name, offline, refresh)
        stable = pre = None
        if vs:
            st = [v for v in vs if not v.is_pre]
            stable = max(st, key=Ver.key) if st else None
            pr = [v for v in vs if v.is_pre and (stable is None or v.key() > stable.key())]
            pre = max(pr, key=Ver.key) if pr else None
        c = cls.get(name)
        r = Row(name, by[name], stable, pre, "CHICKEN-WIRE" if c else "UTILITY", (c or {}).get("capability", ""))
        r.evaluate(policy)
        rows.append(r)
    return rows


def pinned(row: Row, pins: list[dict]) -> dict | None:
    for p in pins:
        if p.get("crate") == row.crate:
            if "until" in p and row.stable and Ver.parse(p["until"]) and row.stable.key() > Ver.parse(p["until"]).key():
                continue  # a newer release than the pin anticipated: re-review
            if "consumers" in p and not row.lagging <= set(p["consumers"]):
                continue  # the pin covers named consumers only; another one lags
            return p
    return None


# ----------------------------------------------------------------------------- report


def md_escape(s: str) -> str:
    return s.replace("|", "\\|")


def render(rows: list[Row]) -> str:
    cls = load_toml(HERE / "classify.toml").get("chicken_wire", {})
    pins = load_toml(HERE / "pins.toml").get("pin", [])
    try:
        sha = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    except OSError:
        sha = "?"
    nuses = sum(len(r.uses) for r in rows)
    nman = len({u.manifest for r in rows for u in r.uses})
    cw = [r for r in rows if r.kind == "CHICKEN-WIRE"]
    lag = [r for r in rows if r.behind]
    L = []
    L.append("# DEPS — every third-party dependency in the tree\n")
    L.append("Generated by `tools/deps-audit/run.py` (SR31; R83; LAWS §Dependencies). **Do not hand-edit**: change")
    L.append("`tools/deps-audit/classify.toml` (chicken-wire verdicts) or `tools/deps-audit/pins.toml` (rows that must")
    L.append("wait, each with a reason and a ledger id) and rerun. The gate is `tools/deps-audit/run.py --check`.\n")
    L.append(f"Tree at `{sha}`, index read {time.strftime('%Y-%m-%d', time.gmtime())}. {len(rows)} crates.io crates, "
             f"{nuses} declarations across {nman} manifests; {len(cw)} CHICKEN-WIRE, {len(rows) - len(cw)} UTILITY; "
             f"{len(lag)} behind latest stable ({sum(1 for r in lag if pinned(r, pins))} of them pinned).\n")
    L.append("Columns: **declared** is the manifest requirement (several when consumers differ); **locked** is what the")
    L.append("consumer's workspace `Cargo.lock` resolved (`—` = optional/target-gated dep not in the lock, or a manifest")
    L.append("outside any workspace); **stable** / **pre** are crates.io's latest non-yanked release and newer pre-release;")
    L.append("**delta** is `=` when every declaration admits and every lock holds the latest stable, `lock` when only a")
    L.append("`cargo update` is owed, `minor`/`major` when a manifest edit is owed (`major` = semver-incompatible).\n")
    L.append("## Inventory\n")
    L.append("| crate | kind | declared | locked | stable | pre | delta | consumers |")
    L.append("|---|---|---|---|---|---|---|---|")
    for r in rows:
        decl = sorted({u.req for u in r.uses})
        locked = sorted({str(max(u.locked, key=Ver.key)) for u in r.uses if u.locked})
        cons = sorted({u.consumer + (" (dev)" if u.section.startswith("dev") else " (build)" if u.section.startswith("build") else "") for u in r.uses})
        delta = {"none": "=", "lock": "lock"}.get(r.worst, r.worst)
        if pinned(r, pins) and r.behind:
            delta += " (pinned)"
        L.append(f"| `{r.crate}` | {r.kind} | {md_escape(', '.join(decl))} | {', '.join(locked) or '—'} | "
                 f"{r.stable or '?'} | {r.pre or ''} | {delta} | {', '.join(cons)} |")
    L.append("\n## Chicken-wire table\n")
    L.append("A crate is CHICKEN-WIRE when it does the work of a capability UnaOS claims (R83). Verdicts: **CUT** — the")
    L.append("UnaOS core that replaces it is an open arc; **OWED** — no arc yet, the proposed arc and crate are named;")
    L.append("**NOT WANTED** — UnaOS will not replace it, and why. Each row is meant to become a ledger row.\n")
    L.append("No crate in the tree demuxes MP4/WebM or decodes compressed audio or video today, so PLAYBACK SR26")
    L.append("(`demux_core`) and AUDIOCODEC SR30 (`audio_core`) replace no row here: they are greenfield. AVCODEC SR24")
    L.append("(`av1_core`) is named on the `image` row (AVIF), the one place an AV1 codec enters the lock (rav1e, an")
    L.append("encoder, through `image`'s default `avif` feature).\n")
    by_v = {}
    for r in cw:
        by_v[cls[r.crate]["verdict"]] = by_v.get(cls[r.crate]["verdict"], 0) + 1
    L.append("Counts: " + ", ".join(f"{k} {v}" for k, v in sorted(by_v.items())) + ".\n")
    L.append("| crate | capability | consumers | verdict | UnaOS replacement |")
    L.append("|---|---|---|---|---|")
    for r in cw:
        c = cls[r.crate]
        cons = ", ".join(sorted({u.consumer for u in r.uses}))
        L.append(f"| `{r.crate}` {r.stable or ''} | {md_escape(c['capability'])} | {cons} | **{c['verdict']}** | "
                 f"{md_escape(c.get('replacement', ''))} |")
    L.append("\n## Pins (`tools/deps-audit/pins.toml`)\n")
    if not pins:
        L.append("None.\n")
    else:
        L.append("| crate | consumers | until | ledger | reason |")
        L.append("|---|---|---|---|---|")
        for p in pins:
            L.append(f"| `{p.get('crate')}` | {', '.join(p.get('consumers', [])) or 'all'} | {p.get('until', '')} | "
                     f"{p.get('ledger', '')} | {md_escape(p.get('reason', ''))} |")
    L.append("\n## Rows behind latest stable\n")
    if not lag:
        L.append("None.\n")
    for r in lag:
        p = pinned(r, pins)
        tag = f" — PINNED ({p.get('ledger', '?')}): {p.get('reason', '')}" if p else ""
        L.append(f"- `{r.crate}` → {r.stable}{md_escape(tag)}")
        for b in r.behind:
            L.append(f"  - {md_escape(b)}")
    L.append("")
    return "\n".join(L)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--policy", choices=("any", "major"), default="any")
    ap.add_argument("--offline", action="store_true")
    ap.add_argument("--refresh", action="store_true")
    ap.add_argument("--stdout", action="store_true")
    ap.add_argument("--json", action="store_true", help="dump rows as JSON (for tooling)")
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()
    if a.selftest:
        return selftest()
    rows = build_rows(a.offline, a.refresh, a.policy)
    if a.json:
        print(json.dumps([{"crate": r.crate, "stable": str(r.stable) if r.stable else None,
                           "pre": str(r.pre) if r.pre else None, "kind": r.kind, "worst": r.worst,
                           "behind": r.behind,
                           "uses": [{"consumer": u.consumer, "manifest": u.manifest, "section": u.section,
                                     "req": u.req, "workspace": u.workspace,
                                     "locked": [str(v) for v in u.locked]} for u in r.uses]} for r in rows], indent=1))
        return 0
    if a.check:
        pins = load_toml(HERE / "pins.toml").get("pin", [])
        bad = 0
        unknown = [r.crate for r in rows if r.stable is None]
        for r in rows:
            if r.behind and not pinned(r, pins):
                bad += 1
                print(f"BEHIND {r.crate} (latest stable {r.stable}):")
                for b in r.behind:
                    print(f"    {b}")
        for p in pins:
            r = next((r for r in rows if r.crate == p.get("crate")), None)
            if r is None or not r.behind:
                print(f"note: pin for `{p.get('crate')}` ({p.get('ledger')}) is stale — the row is current or gone")
        for p in pins:
            if not p.get("reason") or not p.get("ledger"):
                print(f"PIN WITHOUT REASON/LEDGER: {p.get('crate')}")
                bad += 1
        for u in UNSEEN:
            print(f"UNAUDITED {u}")
            bad += 1
        if unknown:
            print(f"UNRESOLVED (index unreachable, no cache): {', '.join(unknown)}")
            bad += len(unknown)
        print(f"deps-audit --check (policy {a.policy}): {len(rows)} crates, {bad} failing")
        return 1 if bad else 0
    text = render(rows)
    if a.stdout:
        print(text)
    else:
        OUT.write_text(text)
        print(f"wrote {OUT.relative_to(ROOT)} ({len(rows)} crates)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
