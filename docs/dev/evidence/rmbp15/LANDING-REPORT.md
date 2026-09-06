# rmbp 15 — landing report (OPEN; written early, updated in place)

**Round shape.** SUPPORT. orin holds the focus (inherited, not re-asked). Peter's order at open:
*do not start jobs, continue the orin support role, queue the x86 work for the focus time.*
Executors spawned: **0** (R22). Support role: **live throughout** (R23) — the stop was on starting
jobs, and it was applied at exactly that scope.

## State at open — fresh host `ls-remote`, run that turn
`flatpak-spawn --host git ls-remote --heads origin` (in-sandbox git dies on publickey):

    main c7407753 · hw-jetson 98ffd63d · hw-pi4 059e04db · hw-rmbp 141cc728

**Local `hw-rmbp` tip was `5c3dbb7e`** (rmbp 14's landing report) — **one commit ahead of origin,
unpushed.** The rmbp-15 baton says "NONE at close"; that line was written at 14:28 and the commit
landed after it, which is why the check is run and never relayed. Named to Peter in the first turn.

**It moved mid-round.** A second host `ls-remote`, run one turn later, returns
`hw-rmbp 5c3dbb7e` — Peter pushed it while this round was writing the queue. The owed list is
therefore not the one computed at open: it is **`f0f2b678` alone**, this round's own commit. Two
checks, forty minutes apart, two different answers; a relayed count would have been wrong both ways.

## What this round produced

1. **`FOCUS-QUEUE.md`** — the x86 queue for the pivot, Q0–Q7, ordered on the fact that a trip round is
   the only round where x86 metal is live. Each item carries its first command; the nine-executor
   allocation is at the bottom. Nine is a ceiling, not a floor.
2. **B59** — three of this lane's live grant targets (`dc683c40`, `1aae3459`, `28899d5c`) exist on
   local `exec-*` branches only, no remote ref, unfetchable by any peer. Found while resolving shas
   for the queue, not by re-reading anything — the fourth time this arc's class of finding has come
   from reading for a different purpose.
3. **J1 corrected to 121/94** (`git rev-list --count main..hw-rmbp`), against the baton's frozen 120.
4. **orin 18 told this seat is online** in the first turn, with the same-turn `ls-remote`, the two
   reviews their folds are blocked on, the standing lane grants, and the `xhci/mod.rs` 12/77
   divergence flagged so they do not spend a fold on it.

## Gates
`unaos/scripts/ledger-check.sh` — **OK, exit 0**, 154 rows across 3 ledger files + RULINGS; one
deferred cross-branch ref (B22 `→ SO6`, resolves at the trunk sync). Docs-only round so far; no code
touched, so no `arroyo check` claim is made here.

## Open at time of writing
Q2 (the SHELLRELICS / VFSROUTE reviews) is offered to orin 18 and not yet answered. Everything else in
the queue waits for the focus.
