#!/usr/bin/env bash
# AETHERSEE — build Aether + the scorer, render the corpus with both engines,
# score, and gate against baseline.json. Exit 0 green, 1 regressed, 2 harness error.
#   tools/aether-audit/run.sh            # corpus run + gate
#   tools/aether-audit/run.sh --live     # also the optional live URLs (never gated)
#   tools/aether-audit/run.sh --only 05  # one page (gate still checks the rest from the last run)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
export PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers}"
cd "$repo"
cargo build --release -q -p aether --bin aether
cargo build --release -q -p aether-audit
"$repo/target/release/aether-audit" run "$@"
"$repo/target/release/aether-audit" gate
