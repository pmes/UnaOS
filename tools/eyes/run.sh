#!/usr/bin/env bash
# EYES runner STUB (tools/eyes's documented shape: `run.sh <suite> [--accept]`; exit 0 green, 1 regressed,
# 2 harness error). The full harness (chromium/xvfb oracles, SSIM, baselines) lives on the AETHERSEE
# branch; this stub runs `cmd` subjects against `golden` oracles with `facet diff`, and is replaced by it.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"; suite="${1:?usage: run.sh <suite> [--accept]}"; accept="${2:-}"
cd "$repo" && exec python3 - "$repo" "$suite" "$accept" <<'PY'
import sys, tomllib, subprocess, shutil, pathlib
repo, name, accept = sys.argv[1], sys.argv[2], sys.argv[3] == "--accept"
sd = pathlib.Path(repo, "tools/eyes/suites", name); od = pathlib.Path(repo, "tools/eyes/out", name); od.mkdir(parents=True, exist_ok=True)
s = tomllib.loads((sd / "suite.toml").read_text()); fails = 0
for b in s.get("build", []): subprocess.run(b, shell=True, check=True)
for c in s["case"]:
    v = {"repo": repo, "suite": str(sd), "outdir": str(od), "case": c["name"], "out": str(od / f"{c['name']}.subject.png")}
    v["ref"] = str(sd / s["defaults"]["oracle"]["path"].format(**v)); run = c["subject"]["run"].format(**v)
    if subprocess.run(run, shell=True).returncode: print(f"FAIL {c['name']}: subject failed"); fails += 1; continue
    if accept: shutil.copy(v["out"], v["ref"])
    d = subprocess.run([f"{repo}/target/release/facet", "diff", v["out"], v["ref"], "--max-delta", str(c.get("max_delta", 0))], capture_output=True, text=True)
    print(("ok  " if d.returncode == 0 else "FAIL") + f" {c['name']:<14} " + d.stdout.replace("\n", "  ").strip()); fails += d.returncode != 0
sys.exit(1 if fails else 0)
PY
