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

5. **The VFSROUTE review** (B60) — orin 18 asked for it, so it was performed here: ACCEPT with one
   blocking condition. `MountTable::same_volume` compares `volume_name()` strings while
   `sdmmc_root_bind` mounts one card under two names, so the router reads one volume as two on the
   exact config the boot disk is being built for — and the transcript cannot convict it, because it
   uses `same_volume` as its own oracle. Three checks that could have failed came back green in the
   same pass: the census reproduces exactly, all 17 `HOST_VERBS` keep a dispatch arm across a
   −2497-line rewrite, and every mutating trait default is a refusal rather than a silent `Ok`.
   Acceptance criteria for SHELLRELICS' owed leg were issued in the same message so it is cut once.
6. **SR2 corrected** in `docs/dev/LEDGER.md` — the mid-capture volume guard is USB-scoped, not
   universal; a Pi capture (or any rMBP capture that lands on the internal Sdhc) has no guard at all.
   Raised by pi 7, re-raised by pi 8, relayed by orin 18, and **verified here at `98ffd63d` before
   being written down**. Lane rule B46 splits the fix: the SR2 text is on this branch, so this seat
   fixed it; orin's A36 and the `prtscr.rs` header prose are on hw-jetson and stay theirs — which is
   the rule pointing the opposite way from how the relay proposed it.

7. **SHELLRELICS ACCEPTED**, and the VFSROUTE condition carried onto the fold. The owed leg arrived
   as `shell.relics.write_raw` at fold-8 tip `f3e64daf`; it was read here, not accepted on report —
   the grant this seat once gave on a description is the one it now reads twice. C1's transfer was
   established by blob oid (`fs/vfs.rs` and `sdmmc_tegra.rs` byte-identical across the three shas,
   `shell.rs` not), so the shell.rs-resident checks were re-run at the fold tip rather than inherited.
8. **GATE-LEDGER caught this seat again** — four blob oids read as commit shas. The gate's own escape
   (`blob:<hash>`, already in use one row away) was the fix, not rewording the evidence. That is the
   fifth time this lane's gate has convicted its own author, and the reason the rule is to run it the
   moment the ledger is edited.

9. **SR3** — quarry's cache-invalidation stamp is USB-only: `volume_gen()` is `usb_publish_gen()`,
   whose only two bump sites sit inside `publish_usb_geometry`, and neither `register_sd` nor
   `register_tegra_sd` advances it, so no card event ever invalidates the listing cache. Raised by
   orin 18 while grepping their branch for text to fix, measured here — and the measurement narrowed
   the claim: a *stick* arrival does advance it on aarch64 (`xhci/mod.rs:11758` carries no arch
   gate), so "never fires" would itself have been a claim that cannot fire. Filed as one S-row with
   a C-section link rather than a second home on orin's branch (P14).
10. **B61 — this seat's own two failures**, recorded rather than absorbed: it routed a text fix to
    orin having verified only the code and never read the text (both texts were already correct, and
    `prtscr.rs` being unconditional meant the no-op edit would have spent pi's cleanest byte-identity
    control), and it told pi 8 a push count was "run this turn" when the check was two turns old —
    right value, wrong provenance. One class, and it is this seat's own standing rule arriving in its
    own outbound messages.

11. **B62/B63/B64 and the env-dep D-row**, all from the same evening's traffic: LAYOUT's spec blast
    radius is one line (`pi4-barename.spec:63`, and orin ruled its post-rename value `/apps/VUG.ELF`
    rather than the mechanical `/boot`); GATE-LEDGER reds a split row only when the debris lands in a
    column it validates, which this seat proved by walking into it twice in one commit; and
    **GATE-K8REACH cannot see a knob that arrives by `option_env!` instead of a cargo feature** —
    `UNAOS_DMAWIN` is read in source and armed by no `arroyo` command at all. The last is SR1's class
    through the other door and the most useful thing the round produced for the gate backlog.
12. **C1 gained a negative leg and orin adopted it as fold-blocking.** The five briefed values were
    all positive configurations, which cannot separate "identity implemented" from "identity present
    but bypassed". Two backends with one `volume_name` over two sources is the case that still means
    something after someone refactors the comparison.

## Round shape, so far
Zero executors. Every finding above came from reading a peer's work or from being read by a peer:
three seats, one evening, and **not one of the stale statements was found by re-reading the file it
lived in**. The three guards-that-cannot-fire (name-based `same_volume`, the USB-scoped prtscr probe,
quarry's card-blind stamp) were each found by someone looking for something else.

## Open at time of writing
C1 is with orin 18 (either the armed `UNAOS_SDMMCROOT=1` run, or the identity fix, and I review the
fix same-turn). SHELLRELICS closes when its leg's diff arrives. Everything else in the queue waits
for the focus.
