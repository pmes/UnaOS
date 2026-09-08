# rmbp 16 — landing report (OPEN; written early, updated in place)

**Round shape.** SUPPORT. orin holds the focus (inherited, not re-asked — the seat did not spend a
turn asking). Peter's order at open: *no new jobs, support orin, and queue your work for your focus
time.* Executors spawned: **0** (R22). Support role: **live throughout** (R23) — the stop is on
starting jobs and is applied at exactly that scope, so grants, verification and answers to the focus
seat continue without permission.

## State at open — fresh host `ls-remote`, run that turn

`flatpak-spawn --host git ls-remote --heads origin` (in-sandbox git dies on publickey), UNFILTERED —
orin 21's baton inherited a false "nothing was pushed" from a filtered check:

    main c7407753 · hw-jetson 98213b7f · hw-pi4 059e04db · hw-rmbp 1b24cb8a
    exec-orin21-fold 21727dc0  (ON ORIGIN — the fold is fetchable; that push is not owed)
    exec-orin20-bootidlive / exec-orin21-c15knobs / exec-orin21-claimcheck  aec2c604
    exec-orin21-buildperf / exec-orin21-ledger 98213b7f · exec-orin20-fitsland 086ec38d
    exec-orin20-unafsgrow 74b7f21b · exec-orin20-integrate2 0f06d60c · UnaOS-gemini 122ed63e

Local `hw-rmbp` = origin, `git log origin/hw-rmbp..hw-rmbp` **EMPTY**, tree clean. **Ancestry, not
value:** only this seat can say the list is empty; a peer can only confirm that a commit landed.

## Pushes named to Peter in the first turn (batched)

1. `git push origin exec-orin17-dupguard` — **the one that matters.** Carries `28899d5c` (DUPGUARD),
   `0019ec7a` (XHCINTD) and `e390721f` (PRTSCLOST). All three are on no remote ref and their content
   is on no remote ref either, verified with both controls. They are the inputs to the metal flight.
2. `git push origin hw-rmbp` at close — this round's ledger rows, queue and report.
3. Optional, ranked below the other two: `exec-orin17-shellrelics` / `exec-orin17-vfsroute`, whose
   content is already safe on `origin/hw-jetson`; pushing them only repairs dangling citations, and
   a re-cite against the landed shas does the same for free.

## What this round produced

1. **B82 — a wrong premise in this seat's own baton, retracted after independent verification.**
   The rmbp-16 baton states that C15 collapses the arming polarity, that `#require=sdmmc` stops
   discriminating, and that this seat therefore owes a new staging predicate. orin 21 derived the
   opposite from the C15KNOBS end; this seat verified it at its OWN tip rather than accepting the
   relay — `esp_jetson()` forces `tegra` (+`tegrasmp`) and never adds `sdmmc`, which enters only via
   `arroyo:1682` (`UNAOS_SDMMC=1`) or `:1738` (`UNAOS_SDMMCROOT=1`). **B77's condition stands with
   its teeth intact, the next Orin flight does not refuse the bind by default, and a queued work
   item is DELETED rather than scheduled.** The general form is worth more than the instance: a
   matrix leg's feature list and a build verb's forced set are different objects, and the wrong one
   is the legible one.
2. **B83 — B59's census was built with an instrument that cannot answer the question it was asked.**
   It enumerated named grant targets; the question is reachability. The sweep
   (`git rev-list <all local heads> --not <all remote refs>`) returns **414 stranded commits over
   ~270 branch tips**, and it immediately surfaced `e390721f` (PRTSCLOST), a fifth commit in the
   same family that four rounds of tracking never named. **The row also splits B59 into two
   close-outs that were being tracked as one:** SHELLRELICS/VFSROUTE have a CONTENT close-out on
   `origin/hw-jetson` and no CITATION close-out (B58's and B60's shas dangle); DUPGUARD/XHCINTD/
   PRTSCLOST have neither. rmbp 15's queue predicted fold 8 would close all of it. Fold 8 landed and
   closed half.
3. **`FOCUS-QUEUE.md` for rmbp 16** — the x86 queue for the pivot, Q0–Q7, metal first, every number
   re-derived at `1b24cb8a` rather than inherited. **Three inherited numbers were stale:** J1 is
   **166 ahead / 94 behind**, not 121/94; `drivers/xhci/mod.rs` diverges **21/86 over 22 hunks**,
   not 12/77; and Q2 is closed. The J1 growth has a measured rate — **+45 commits in one round in
   which no kernel code was written** — so the landing's price is a function of rounds waited.
4. **B84 — a queued doc "fix" that would have introduced the falsehood it was sent to remove.**
   The baton carries, from orin, that `partitions.md`'s *"aarch64 only, because `fs::unafs` is"* is
   false. It is TRUE: the sentence names the kernel module, declared once at `fs/mod.rs:36` under
   `#[cfg(target_arch = "aarch64")]` — present on all seven refs checked, including the fold tip, so
   it is not branch skew. What is unconditional is a **different object with the same name**, the
   arch-neutral library crate at `unaos/libs/fs/unafs/src/lib.rs` (zero `target_arch` gates); the
   doc's own paragraph contrasts them two sentences earlier. Retraction, not an edit.
   **Three findings this round, one class** — B82's matrix leg read as a build verb, B84's crate read
   as a module, and B81's legible instrument cited over the sound one. **Two queued work items
   deleted by verification, zero scheduled.**

5. **orin told, first turn**, with the same-turn `ls-remote`: their bounced Correction-02 delivered
   and independently verified here; the standing grants re-stated in a form they can hold this seat
   to (`fs/unafs.rs` size 8 MiB granted / posture stopped; `fits=` make-it-sound-do-not-rename; the
   `fs/vfs.rs` `NativeBackend` `cfg` STOP tripwire); exactly what CLAIMCHECK-2 must carry for the
   TEARSCOPE fix arm to be taken; and a request for the current B71 reading rather than the old one.

## Gates

`unaos/scripts/ledger-check.sh` — run on every ledger edit, verdict recorded below. Docs-only round;
no code touched, so no `./arroyo check` claim is made here.

## Flagged

- **The metal flight's inputs live on one machine.** Until push (1) lands, a `git worktree prune`
  plus a `gc` removes DUPGUARD, XHCINTD and PRTSCLOST from every seat at once — all four worktrees
  answer the same `git rev-parse --git-common-dir`, so there is no second copy anywhere.
- **`0019ec7a` touches `arch/aarch64/display_tegra.rs`** (14 lines) — out of lane. Read before the
  Q1 rebase; negotiate with orin if it survives.
