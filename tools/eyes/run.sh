#!/usr/bin/env bash
# EYES — see what a program draws, score it against a reference, gate on regression.
#   tools/eyes/run.sh <suite>             # build, render subjects + oracles, score, gate vs baseline.json
#   tools/eyes/run.sh <suite> --accept    # same, then rewrite the suite's baseline.json from this run
#   extra flags pass to `eyes run`: --only SUBSTR, --all (include optional/network cases), --refresh-ref
#   tools/eyes/run.sh metal --from <dir-pulled-off-the-card> [--accept] [--only X]
#     (GLASSEYES, B343) the `metal` suite (kernel `shot <state>` PNGs vs golden/, masked by the kernel's .MSK)
#     is scored by metal.py (python3 stdlib only: zlib + PNG), not the eyes binary.
# Exit: 0 green, 1 regressed past baseline / drift past the gate, 2 harness error.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
suite="${1:?usage: run.sh <suite> [--from DIR] [--accept] [--only X] [--all]}"; shift
if [[ "$suite" == "metal" ]]; then
  exec python3 "$here/metal.py" "$here/suites/metal" "$here/out/metal" "$@"
fi
accept=""; pass=()
for a in "$@"; do if [[ "$a" == "--accept" ]]; then accept="--accept"; else pass+=("$a"); fi; done
export PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers}"
cd "$repo"
cargo build --release -q -p eyes
"$repo/target/release/eyes" run "$suite" ${pass[@]+"${pass[@]}"}
"$repo/target/release/eyes" gate "$suite" $accept
