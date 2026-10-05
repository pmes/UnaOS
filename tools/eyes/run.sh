#!/usr/bin/env bash
# EYES — see what a program draws, score it against a reference, gate on regression.
#   tools/eyes/run.sh <suite>             # build, render subjects + oracles, score, gate vs baseline.json
#   tools/eyes/run.sh <suite> --accept    # same, then rewrite the suite's baseline.json from this run
#   extra flags pass to `eyes run`: --only SUBSTR, --all (include optional/network cases), --refresh-ref
# Exit: 0 green, 1 regressed past baseline, 2 harness error.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
suite="${1:?usage: run.sh <suite> [--accept] [--only X] [--all]}"; shift
accept=""; pass=()
for a in "$@"; do if [[ "$a" == "--accept" ]]; then accept="--accept"; else pass+=("$a"); fi; done
export PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers}"
cd "$repo"
cargo build --release -q -p eyes
"$repo/target/release/eyes" run "$suite" ${pass[@]+"${pass[@]}"}
"$repo/target/release/eyes" gate "$suite" $accept
