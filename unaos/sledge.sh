#!/bin/bash
set -e

pre_flight_check() {
    echo "🛫 Pre-Flight Build Check..."
    if ! cargo build --package unaos-kernel; then
        echo "❌ FAILURE: Kernel does not compile."
        exit 1
    fi
}

if [[ "$1" == "--merge" ]]; then
    CURRENT=$(git rev-parse --abbrev-ref HEAD)
    pre_flight_check
    git add . && git commit -m "Pre-merge sync" || true
    git push origin "$CURRENT"
    git checkout main
    git pull origin main
    git merge "$CURRENT" --no-ff -m "Merge $CURRENT into main [via Sledge]"
    git push origin main
    git checkout "$CURRENT"
    echo "✅ unaOS is synchronized on main."
else
    MSG=${1:-"Kernel Strata update"}
    git add . && git commit -m "$MSG"
    git push origin $(git rev-parse --abbrev-ref HEAD)
fi
