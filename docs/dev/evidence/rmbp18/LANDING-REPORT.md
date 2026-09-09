# rmbp 18 — landing report (OPEN; written early, per rmbp 17's convention)

Support round, zero executors (R22). J0 — the trunk sync — is DONE and committed;
J1 (the gates-only landing) remains with Peter and is not this seat's to decide.

## State at open — fresh host `ls-remote`, run that turn (2026-09-08 19:3xZ)

    main c7407753 · hw-pi4 247b1b95 · hw-jetson 0d83fd0d · hw-rmbp 0e993445

`hw-rmbp` was FULLY PUSHED at open — `origin/hw-rmbp == HEAD == 0e993445`, confirmed with
`flatpak-spawn --host git ls-remote origin hw-rmbp`. The baton's status line said
`hw-rmbp 36d82d73+`; Peter had pushed past it. Nothing was outstanding.

## Pushes named to Peter, batched in the first turn

  * At open: NONE. The branch tip was already on origin.
  * Owed now: ONE — `hw-rmbp`, for the sync merge `9b3506fd`.

## J0 — the trunk sync. DONE.

`9b3506fd`, parents `0e993445` + `c7407753`. 253 commits, 114 files, +47266/-591.
Verified by the predicate, not by comparing tips:
`git merge-base --is-ancestor origin/main HEAD` -> YES; `rev-list --left-right --count`
-> `0 254`, i.e. nothing of trunk's remains outside this branch.

rmbp 17's scout (B110) held exactly on the file map: ten of eleven `.rs` files auto-merged
clean, four files conflicted in five hunks. Each was resolved by measurement:

| file | resolution | the measurement that justified it |
|---|---|---|
| `prtscr.rs` | TRUNK verbatim | `cd533543` took this branch's PRTSCR-VOL byte-for-byte, then built PRTSCR-ASYNC (`d7eec583`) and pi 7's slot-id comment (`5f8f392f`) on it. Loss-tested line-by-line: the 68 lines unique to my side are the pre-rewrite FORM of work trunk kept — every behaviour survives in trunk's 1120-line file |
| `screenshot.md` | MINE | zero trunk lines lost (strict superset). §10/§11 document TRUNK's code: every symbol they name — `Job`, `Job::slice`, `Phase::Encode`, `Phase::Write`, `Refusal::Vanished`, `SLICE_ROWS`, `SLICE_WRITE`, `usb_publish_gen`, `write_grow`, `create_in_dir` — is present in trunk's `prtscr.rs`. Doc and implementation agree |
| `LEDGER.md` | UNION | pi 9's shape, both halves: resolved id set == exact UNION (ours 31, trunk 41, union 45), and 0 of 46 rows match NEITHER side |
| `RULINGS.md` | UNION, seat-qualified | NOT a plain union — see below |

## What the scout MISSED, and it is the round's finding: RULINGS R24/R25 are double-booked

Two seats allocated the same two ids for different rulings on unpushed branches:

  * orin 17, 2026-09-06 — now on `main`, `hw-pi4`, `hw-jetson`
  * rmbp 16 and orin 22, 2026-09-08 — `hw-rmbp` only

Both are Peter's verbatim words, so R20 forbids discarding either. RULINGS.md's own header
forbids renumbering a cited id, and BOTH are cited: orin's 54x on trunk and 72x on
hw-jetson, rmbp's 34x here (`git grep -ohE '\bR2[0-9]\b' <ref> -- docs/ unaos/`).
So nothing was renumbered. The id cell now carries the heard-by seat — `R24 (orin 17)` vs
`R24 (rmbp 16)` — which preserves every citation on every ref and makes a bare `R24` in
prose explicitly AMBIGUOUS, to be read by the branch its document came from. Filed as SR5.

A THIRD IS LATENT: orin 17's `R26` is on `hw-jetson` only and collides with orin 22's `R26`
the moment that track lands.

orin 23 agreed over ccd before archiving: orin 17's pair keeps the bare numbers (older, on
three refs), rmbp's pair moves in a SEPARATE scoped commit with its own citation gate —
never inside a sync, because this tree now holds BOTH meanings and a blind rewrite would
corrupt orin's citations too. The permanent collapse is Peter's record and is left with him.

This is the seat-prefix vote's exact failure mode, one file over. `ledger-check.sh:247`
records all three seats adopting SP/SR/SO on 2026-09-06 because "sequential allocation is
STRUCTURALLY broken across unpushed branches — a reserved gap only works if every seat can
see it, and none can; two collisions in one night." That fix was never extended to RULINGS.

## Three gates met trunk's content for the first time, and found three things

These gates exist only on `hw-rmbp` (J1's list), so trunk's 253 commits had never met them.
This is J1's argument as a number rather than a claim.

  * **S30** (LEDGER, owner rmbp) asserted an absence naming no enumeration. FIXED, and the
    claim re-measured on the post-sync tree: `git grep -c 'press seen=' --
    unaos/crates/kernel/src/drivers/` is 1 hit of 30 driver files, EHCI only.
  * **A27** (orin-ledger) 9 fields against a 7-column header — a literal `|` inside a wire
    quote splitting the row. Another seat's file and lane: REGISTERED, not fixed.
  * **GATE-FAMILY** caught `shell_reopen_drain` growing to `orin_shell_reopen_drain` +
    `pi_shell_reopen_drain`, both added to the SHARED core `main.rs` (`b19b2865`,
    `99c153ca`), and both carrying a BOARD name where R16 requires the owning subsystem.
    With `x86_render_service`'s inline drain that is three per-board implementations of one
    job. Not renamed here — renaming symbols inside two peers' in-flight arcs is not a
    sync's work. Recorded as SR6 with the extraction and the R16 rename named as ONE scoped
    arc. The gate's three required answers are in `9b3506fd`'s commit message.

## A registration that now carries its own trigger

orin had ALREADY fixed A27 and S30 on `hw-jetson` (`c386697b`), so both A27's and C10's
registrations in `ledger-check.sh` now carry an explicit DELETION TRIGGER **in the gate
itself** rather than in a baton: delete in the commit that folds `hw-jetson`, never before.
rmbp 17 knew this about C10 and kept it only in the baton; a baton is not where a gate's
own invariant survives.

**For whoever folds `hw-jetson`:** orin's S30 enumeration (`grep -rl 'recovered='
unaos/crates/kernel/src/drivers/`) and mine (`git grep -c 'press seen=' ...`) will conflict
on that one row. Either is a true enumeration — keep whichever resolves cleanly, do not
re-derive it.

## Gates, on the post-sync tree

  * `./unaos/arroyo check` — **GREEN, real exit 0**, default serial matrix, no
    `UNAOS_CHECK_P` (orin 23's binding condition). 66 green arch legs across both arches,
    0 failure lines in the whole log.
  * `GATE-LEDGER` OK — 253 rows, ids unique, field counts, absence forms, status enum,
    owners, 3 registered exceptions, 3 deferred cross-branch refs.
  * `GATE-FAMILY` OK — 9 families, none grown (baseline updated WITH its reason).
  * `GATE-APPEND`, `GATE-KNOB`, `GATE-ROOTS`, `k8-reach` — all OK.
  * x86 QEMU regression leg, `UNAOS_QMP_PORT=4481` pinned so it could not collide with a
    peer: **45 PASS, 0 FAIL**, real exit 0, log fresh (mtime 53 s before it was read),
    banner `⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet`.

## What this round got wrong

**A `| tail -40` on the first `arroyo check` reported `exited with code 0` while the check's
own text said `check FAILED`.** The exit code belonged to `tail`. Read as a green gate that
would have shipped a broken sync. Every gate result above was re-run writing to a FILE and
scored on the command's OWN exit code. This is `verification-comes-from-execution` with a
new edge: a pipeline launders the verdict, and the summary line is not the exit code.

## Flagged / owed

  * **J1, the gates-only landing** — still with Peter, unanswered, and now with a number
    behind it: two reds sitting green on trunk today, found the moment the gate met it.
  * **SR5** — the permanent R-id collapse. Cross-seat; orin 23 agreed the shape before
    archiving; Peter's record, Peter's call.
  * **SR6** — the `shell_reopen_drain` extraction and R16 rename, as one scoped arc.
  * orin 23 **archived mid-exchange**, so the family finding was relayed to pi 10 to carry
    to the next orin seat. The B90 positive control and the PRTSCR-VOL witness read owed
    to/from orin 23 are parked until an orin seat is open.
