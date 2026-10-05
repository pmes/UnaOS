#!/usr/bin/env bash
# EYES runner STUB (GLASSEYES, B343) — this tree predates SR23's `eyes` binary (tools/eyes/Cargo.toml).
#   tools/eyes/run.sh metal --from <dir-pulled-off-the-card> [--accept] [--only X]
# The `metal` suite (kernel `shot <state>` PNGs vs golden/, masked by the kernel's .MSK) is scored by metal.py
# (python3 stdlib only: zlib + PNG). Any other suite goes to the `eyes` binary when it is in the tree.
# Exit: 0 green, 1 drift past the gate, 2 harness error.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
suite="${1:?usage: run.sh <suite> [--from DIR] [--accept] [--only X]}"; shift
if [[ "$suite" == "metal" ]]; then
  exec python3 "$here/metal.py" "$here/suites/metal" "$here/out/metal" "$@"
fi
[[ -f "$here/Cargo.toml" ]] || { echo "eyes: suite '$suite' needs the eyes binary (SR23), not in this tree" >&2; exit 2; }
accept=""; pass=()
for a in "$@"; do if [[ "$a" == "--accept" ]]; then accept="--accept"; else pass+=("$a"); fi; done
cd "$repo" && cargo build --release -q -p eyes
"$repo/target/release/eyes" run "$suite" ${pass[@]+"${pass[@]}"}
"$repo/target/release/eyes" gate "$suite" $accept
