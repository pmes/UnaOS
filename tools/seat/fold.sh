#!/bin/bash
# fold.sh <branch>: merge an executor branch into the main checkout without committing (R131 / head rule 17: refuse a branch whose newest commit body lacks "MEASURE: PASS"). Ledger/registry conflicts are left for the seat's hand (sheetmerge.py / tailseam.py).
B="$1"; cd /home/user/UnaOS || exit 1
if ! git log -1 --format=%B "$B" | grep -q "MEASURE: PASS"; then echo "REFUSED: $B has no MEASURE: PASS line in its commit body (head rule 17)"; exit 3; fi
git merge --no-commit --no-ff "$B" 2>&1 | grep -E "CONFLICT|Automatic merge" | sed 's/^/merge: /'
git diff --name-only --diff-filter=U
