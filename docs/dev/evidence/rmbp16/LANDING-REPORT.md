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

## The orin exchange (four messages each way, same round)

6. **B85 — the x86 half of the junction-keyed-rule scan, run because orin held a fold gate on it.**
   Their question: can fitsland's emitter change reach an x86 spec keyed on adjacent tokens? **No** —
   `span_fit_report` is `fs/unafs.rs:2650` on their repaired tip `6cf9f13b`, inside the module gated
   aarch64 at `fs/mod.rs:35-36`, and no x86 spec references the tokens. **Checked at THEIR sha:
   grepping this tree returns nothing and would have read as ABSENT rather than NOT-LANDED-HERE** —
   one step from B82/B84's trap a third time in one round. The class transfers though: **six
   junction-keyed FORBIDs across seven x86 specs**, four verified still-fireable against
   `syscall.rs:7272` and `wm.rs:16342`/`:16350` (both arms of a two-arm emitter — exactly where the
   aarch64 defect lived), two unverified and queued.
7. **B82's third consequence corrected — the one part of it I had kept.** orin was right that "side
   of C15" is not observable by a scorer; their replacement is under-specified in one case,
   established at their fold base: the bind is `sdmmcroot`-gated (`sdmmc_tegra.rs:3790`) while
   `arroyo:1682` adds only `sdmmc`, so pre-C15 an image can be armed and legitimately carry no bind
   witnesses. **Score by `strings` for the BIND's own witnesses.** Three corrections deep, and the
   claim only became sound when derived from a third end.
8. **The TEARSCOPE arm is taken, and the condition that held it was mine and unsatisfiable.** I
   required per-call-site decline counts from a capture that closed 34 minutes before the emitter
   was authored. **Observability first: never condition an arm on a measurement whose instrument
   does not exist yet.** Identity and census delivered and pinned to boot 37 of 39.
9. **B71 ticked with orin's live reading**, and `strip.rs:820`'s false "torn=0 all boot" added to Q6
   as this lane's correction.
10. **B63 demonstrated itself in the writing.** B85's first draft carried a grep alternation whose
    escaped pipes split the row into 11 fields. GATE-LEDGER caught it **only because the debris
    landed in validated columns** — debris past the last validated column is precisely what B63 says
    passes silently, and three such rows are in this file today. The closest thing to a live fixture
    the unwritten field-count assert will get.

## The design round (Peter live, orin 22 holding the focus)

11. **R24 — the comms clamp's premise rejected.** orin 21 clamped inter-seat comms to asks and
    blockers on budget grounds; Peter rejected the premise and attributed the spend elsewhere (orin
    owned it: ten executors spawned on Fable against LAWS:207/:216). **B86 records the structural
    half: the most expensive rule in the corpus is REMEMBERED, not GATED** — the rule is written in
    three places across two corpora and a case-insensitive grep for `fable` over every `.sh`, `.py`
    and `.json` returns documentation only. A `PreToolUse` hook refusing a Fable spawn fails closed;
    proposed to Peter, not applied, because it edits harness configuration.
12. **B87/B88 — BOOTROOT granted, then the blocker, then the real price of J1.** Five shared
    kernel-core files granted on Peter's boot-cold direction, every symbol verified at orin's base
    rather than this one. **The grant's first form carried six conditions and Peter cut it to three
    in one sentence: condition 3 was backwards, protecting the very x86 special-casing the direction
    exists to delete.** Then the blocker — `BlockSource::TegraSd` exists only under `sdmmc`, which
    `esp_jetson()` never forces, so a boot-cold walk on a plain image cannot enumerate the medium it
    booted from. Accepted; `sdmmc` becomes default-on with an opt-out. **And pulling on orin's
    correction of a wrong claim of mine produced B88: seven gates exist only on `hw-rmbp`, and
    `ledger-check.sh` is +6/−190 against both `main` and `hw-jetson`. J1's price is not 166 commits
    of review — it is that this lane's gates are not protecting the fleet and its fixes are not
    reaching it.** A gates-only landing was proposed to Peter as the un-blockable subset.
13. **B89/B90/B91 — Peter's three disk clauses, turned into checks.** *"Not making assumptions and
    not tying them together possibly staining the testing of the newer version"* (isolation, not
    dumbness) · *"like seeing a friend — nobody wants another to wrap their life around them"*
    (**FRIEND-INVARIANCE**, B90: identical behaviour with and without another UnaOS disk, falsified
    by one unplug and a wire diff) · *"if UnaOS saw catalina and immediately formatted the disk as an
    alien enemy"* (**R25**, the stranger clause). **Two staining vectors found in this lane:**
    `prtscr`'s capture ladder writes to a foreign disk when the boot volume refuses, and a
    non-version-unique identity window lets root itself change — fixed by putting `UNAOS_GIT_SHA`,
    which ships in every image and has one dead reader, inside the comparison window. **And B91's
    ordering constraint on Peter's own goal: INSTALL-SELF guards HOME, not strangers, so the AHCI
    driver that would put UnaOS on the internal drive is the same driver that exposes Catalina to an
    engine whose only protection points the other way. The stranger guard lands first.**
14. **B92 — an 81× on `check`, handed over by orin's BUILDPERF arc and queued above this lane's own
    gate work.** 56 legs, 557 s cold / 155 s warm-serial / 1.9 s with per-slot target dirs, cause
    isolated rather than inferred. `arroyo` already documents the remedy in three other places and
    never applied it to the matrix.
15. **FRIEND-DIFF's normalizer was calibrated on the comparison it judges** — caught and fixed to a
    three-boot shape with a frozen same-condition normalizer and a durable positive control that must
    RED before any green is read.

## What this round got wrong, recorded because the round was about claims nothing re-checks

- **A cross-tree citation that does not name its tree is unfalsifiable.** This seat "corrected"
  orin's `arroyo:50` to `:54` using its own coordinates on a file that diverges 603/548 between the
  trees. Their number was right for their executor's tree. **The discipline was applied to symbols
  and dropped for line numbers, in the message whose subject was that line numbers are fragile** —
  and B87's opening paragraph, written four hours earlier, states the rule that was broken.
- **A scoring hazard built on a peer's characterisation of code held in this seat's own object
  store.** `bar_decline` emits nothing; it is two `fetch_add`s and a `false`. Verifying that a commit
  exists is not verifying what it does.
- **A control that tested nothing**, nearly shipped to a peer as evidence: an unquoted heredoc let
  the shell substitute the token out of the row before it was written. Caught only because both gates
  stayed silent where one had to fire.
- **A conclusion retracted correctly, then reached anyway by someone else changing the other half of
  the premise** (B82). A rejected conclusion deserves a watch, not a delete.

## Gates

`unaos/scripts/ledger-check.sh` — run on every ledger edit, verdict recorded below. Docs-only round;
no code touched, so no `./arroyo check` claim is made here.

## Flagged

- **The metal flight's inputs live on one machine.** Until push (1) lands, a `git worktree prune`
  plus a `gc` removes DUPGUARD, XHCINTD and PRTSCLOST from every seat at once — all four worktrees
  answer the same `git rev-parse --git-common-dir`, so there is no second copy anywhere.
- **`0019ec7a` touches `arch/aarch64/display_tegra.rs`** (14 lines) — out of lane. Read before the
  Q1 rebase; negotiate with orin if it survives.
