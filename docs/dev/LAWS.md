# LAWS.md — standing operational laws

Durable process laws for all UnaOS sessions, moved here from session memory so
they are versioned and reviewable. Session memory keeps only pointers.
`CLAUDE.md` covers layout, lanes, and arc discipline; this file records the
laws minted at the bench and at the seat. Each entry names its origin date.


## /tmp — NEVER. Peter's standing ruling.

**Do not write anything to `/tmp` in this project: no scratch files, no diffs, no
build output, no disassembly, no scratchpads.** Use `~/unaos-bench/scratch/<arc>/`.

**Why, and the second reason is the serious one.** `/tmp` is cleared at 3 days,
so a snapshot taken there is a snapshot with an expiry nobody records — the rmbp
seat lost its executor scratchpads to exactly this and only the git worktrees
survived. But `/tmp` here is also **RAM-backed**, and building under it
**OOM-killed the harness** once. That is not a lost file; that is a dead session.

**Why this is in LAWS.md and not a resume.** This ruling existed since at least
2026-08-19 and was recorded ONLY in `unaos-pi4-resume.md` — one track's private
file. The other two seats never saw it, and both broke it: rmbp lost work to the
3-day clear, and orin wrote a 15 MB disassembly into `/tmp` and then reported the
consequence upward as a discovery. **A standing ruling that lives in one track's
resume is not a rule, it is a local habit.** If a ruling binds every seat, it
belongs here, on the day it is made.

## Verification

- **Verify before claiming owed** (2026-07-17). Never write an "owed /
  pending / operator must" line without first running the check that would
  falsify it. Inherited baton claims are hypotheses until re-verified.
- **No deferred verification** (2026-07-22). Owed verification (builds,
  citation checks, log reads) runs the moment it is noticed — in the
  background if long — and never surfaces as new work while the operator is
  driving something else.
- **Full-knob gate** (2026-07-22). A PASS on knob/feature-gated code requires
  (1) the gate run with every relevant knob armed, and (2) proof the code is
  in the builder-path artifact (`strings kernel.elf | grep <probe-tag>`).
  The builder has its own env→feature map that can silently drop features;
  `./arroyo check` alone proves nothing about optional features. rmbp 15's
  ruling on the Orin card path (orin 21 Correction-03, 2026-09-08) sharpens
  the split into three parts: a spec's `#require=` asserts only what a build
  log can honestly assert — the features the image needs (`tegra`, `sdmmc`) —
  and is not evidence of arming; arming is proven on the ARTIFACT, by
  `strings` for the witnesses the armed path emits; and every scored capture
  carries a REQUIRED arming field, established by `strings` on the image that
  was flashed, never by which knobs were typed. A capture without that field
  FAILS TO SCORE rather than scoring wrong, because an unarmed image's absent
  witnesses are unexercised while an armed image's absent witnesses are a
  defect, and the scorer must know which. rmbp 16's amendment (Correction-04,
  same day) fixes what the field IS: the bind's own four witnesses present or
  absent — `boot-medium-mismatch`, `SAME-MEDIUM`, `bootid DISARMED`,
  `covers the native leg`, all in `sdmmc_tegra.rs` — never a feature name one
  layer up, because pre-C15 `UNAOS_SDMMC=1` arms recon but not
  `sdmmc_root_bind` (that is `sdmmcroot`-gated) while post-C15 the same knob
  arms both, so "sdmmc armed" is ambiguous on one side and the witnesses are
  correct on both without knowing the side. Do not invent a narrower feature
  to give `#require` teeth; that re-creates the knob C15 deletes.
- **Null hypothesis is our code** (2026-07-22). Our code / boot-chain /
  sequence theories outrank hardware-, firmware-, and environment-blame
  theories by default. Bench cross-checks are proposed neutrally as
  discriminators, without a stated lean toward the hardware branch.
- **The wire may not lose lines** (2026-07-29). Serial output is the evidence
  every gate is counted from, so the transport is held to a stricter standard
  than what it reports on: a line that cannot be written is DEFERRED, and a
  line that is genuinely lost is COUNTED and announced on the wire
  (`[serial] dropped N lines`). Silent loss is forbidden — a missing `PASS`
  must never be indistinguishable from a fixture that never ran, and a
  regression's `FAIL` must never be able to evaporate. Enforced every run by
  the SERWIT-1 fixture; see
  [`docs/dev/OS/02_KERNEL_CORE/serial_transport.md`](OS/02_KERNEL_CORE/serial_transport.md).
- **A wrapped record is not a truncated one** (2026-08-31). The UEFI console
  the bootloader logs to is sometimes 80 columns wide — the loader never calls
  `SetMode`, so the width is inherited firmware state — and at 80 columns the
  firmware hard-wraps every write with a real CRLF. No bytes are lost, but a
  line-oriented read loses the tail, so `awk '/pattern/'` reports a witness
  that is present as a witness that was cut off. Orin 11 spent a session on
  an identity line that appeared to end at the word `max_vaddr` while the
  value was on the wire throughout. Read bootloader-window captures through
  `~/unaos-bench/tools/unwrap80.sh` (bench-side, outside the repo)
  — it is a no-op on a wide-console capture, so there is no cost to always
  using it. The image-identity witness itself is held under 80 columns so it
  never needs the tool; see
  [`docs/dev/OS/01_BOOT_HAL/bootloader_spec.md`](OS/01_BOOT_HAL/bootloader_spec.md) §4.
- **A flake is an observation, not a re-run** (2026-08-18). An intermittently
  red gate is diagnosed against the fixture-flake corpus —
  [`docs/dev/FIXTURE_FLAKES.md`](FIXTURE_FLAKES.md) — before it is re-run:
  match the witness text, capture what the entry asks for, then re-run. New
  classes are recorded there rather than carried in session memory.
- **Default-quiet boot** (2026-07-18). Confirmed test families are not
  re-run on default boots; batteries live behind knobs (QEMU gates arm
  them). Gate, never delete.
- **A spec must be look-around free — `foreman` refuses the dialect and its
  preflight is all-or-nothing** (pi 8, source text `pi4-regression.spec:626-629`
  at `hw-pi4 059e04db`). Rust's `regex` rejects `(?=…) (?!…) (?<=…) (?<!…)` and
  backreferences, so one such pattern in a spec makes `preflight_spec`
  (`tools/foreman/src/main.rs:114`, ahead of `parse_spec` at `:115`) abort the
  whole run with a named-line report and print no verdict table. Use the
  documented prefix-factored form instead. **The rule is recorded HERE because
  of how it was broken:** it existed only as a comment at the head of ONE arch's
  spec, riding an unlanded arc, so it was unreachable from trunk and from the
  other tracks — and the seat that violated it could not have read it. A rule
  enforced by a comment in a file only one seat can see is not a rule. Census
  2026-09-07: one non-comment look-around across eleven specs.
- **A zero-hit grep bounds only the tree you ran it in** (pi 8, 2026-09-07).
  Two seats greped the same path for the same sentence and got 1 and 0 — both
  correct, because the text rode 36 unlanded commits. Absence proven in your
  worktree is absence *there*; before calling a citation wrong, establish which
  tree it was written against. The same bound holds for a count — **a
  measurement is scoped to its base exactly as a claim is scoped to its
  check** (orin 21 Correction-01, 2026-09-07; rmbp 15's phrasing, 2026-09-08).
  The orin 21 baton's "thirteen `sdmmcroot` cfgs" was measured at `98213b7f`
  and handed to executors based at its descendant `aec2c604`, where
  BOOTIDLIVE had made it 22 live predicates (23 raw): true where measured,
  false where used, and the brief said to force the number. Report a count
  with the sha it was taken at, and derive it again at the base you actually
  build from.
- **"Sourced from code" is not "verified end-to-end" — reading a function is a
  citation too** (pi 8's formulation, orin 19's error, rmbp 15's catch;
  2026-09-07). A seat could not resolve a citation, went to the source, read
  `parse_spec_bytes` and correctly found that one bad pattern aborts before the
  builtin forbids are installed — then reported the consequence as silent
  vacuum. It is not: the caller preflights first and refuses loudly. The
  function was read; the PATH FROM ENTRY POINT TO BEHAVIOUR was not. Going to
  the source is right and does not by itself settle severity: ask "could this
  path have been reached", not only "is this sourced". One
  `grep -n 'parse_spec' main.rs` would have answered it and nobody ran it.
  ⚠ Record this one as the COUNTER-EXAMPLE it is — an instrument built not to
  fail silently (`457ed7c5` SPECFLIGHT), which worked. A ledger that collects
  only failures teaches a fleet the wrong thing about its own instruments.
- **A wrapper that swallows the exit status is a gate that cannot fire**
  (2026-09-07, pi 8's near-miss, caught by them before it went out). A pipe
  reports the LAST stage's status: `cmd --check ... | head` yields `head`'s
  exit, so `cmd ... | head && echo APPLIES CLEAN` prints APPLIES CLEAN over a
  `git apply` that failed. pi 8 sent themselves that green, caught it only by
  re-running for the exit code alone, and reported the method error beside the
  result. **The defect class is identical to a check whose pattern can never
  match** — in both, the reading is produced by the harness rather than by the
  thing under test, and both read as success. Any pipeline whose verdict is a
  claim about an earlier stage must capture THAT stage's status (`PIPESTATUS`,
  a temp file, or no pipe at all); a sentence of the form "X applies" / "X
  passes" is worth exactly the exit code it was read from, and if you cannot
  name that exit code you do not have the claim.
- **A check is trusted only when its corpus can produce more than one
  outcome** (pi 9's sharpening, orin 21 co-signed; 2026-09-08, four instances
  across two seats). A green check proves nothing in two distinct ways. (a)
  The falsifier is absent from what was scanned: ORINFOLD's
  `FORBID span_blocks=2048 fits=` after fitsland's `fs::unafs::span_fit_report`
  interposed `sb_blocks=` between the two tokens, so no line the fold emits
  can match — the same stale geometry scored 1 hit / exit 1 pre-fold and
  0 hits / exit 0 on the fold; and `unaos/scripts/identify-card.sh`, which
  always exits 0, so an exit-status guard on it can never fire. (b) The
  falsifier is excluded by corpus SELECTION: pi 9's "0 late FORBID hits on a
  PASS capture" (a capture selected for passing carries no FORBID hit by
  construction), and pi 8's sample of three shas that all postdated the
  authoring commit. When it is (b), change the corpus, not the pattern —
  and ask a green corpus only about content (what is emitted after the last
  COMPLETE marker), never about failures. Sibling of the exit-status rule
  above: there the harness produces the reading, here the corpus does, and
  both read as success. The corollary for (a) is structural, not a recurring
  obligation (pi 9, Note-07 — "re-check after every sibling change to the
  emitter" is agreed and then quietly not done): key a FORBID or REQUIRE on
  ONE bounded field wherever the property permits (`span_blocks=2048\b`),
  because adjacency across fields is a dependency on a neighbour you do not
  own. Where the property is a conjunction (`skipped=0` AND `srcdelta=0`),
  bounded fields joined by `.*` absorb insertion but not reordering;
  order-independence needs look-aheads, which `mbench.py`'s Python `re`
  accepts (`Directive.__init__` compiles the pattern verbatim after
  `clean_line`) and `foreman` refuses (the look-around rule above) — a
  house-style call per spec, decided by which tool reads it.
- **A matrix leg's feature list is not a build verb's forced set — cite the
  declaration site, not the nearest symbol of that name** (orin 21's own
  retracted Correction-01 §C3, refuted by C15KNOBS deriving from the other
  end, Correction-02, 2026-09-08; the class named by rmbp 16 after B81/B82/
  B84). The seat read the `arm-tegra` check leg of `KERNEL_CFG_MATRIX` in
  `unaos/arroyo` (which carries `sdmmc`), treated it as the `esp-jetson`
  IMAGE's feature set, and published "C15 collapses the arming polarity" to
  both peers. `esp_jetson()` forces only `tegra,tegrasmp` (plus
  `bsptick,bsprun`) and never adds `sdmmc`; the polarity survives C15 with
  the knob renamed. An image claim cites the verb's function. rmbp 16 named
  the class on its third instance in one round — a vacuous `fits=` quoted
  over the sound bit 3 (B81), a matrix leg read as a build verb (B82), the
  `libs/fs/unafs` CRATE read as the `#[cfg(target_arch = "aarch64")] pub mod
  unafs;` MODULE in `fs/mod.rs` (B84): the wrong object is the one whose name
  is easier to reach, and nothing malfunctions to say so. The discriminator
  is the declaration site — `esp_jetson()`, `fs/mod.rs`'s `mod unafs`, the
  emitter — one `sed -n` from falsified, never a symbol that merely matches
  the name.
- **Legibility outcompetes soundness** (pi 8 and rmbp 15, orin 20,
  2026-09-07; recorded as the pair both seats asked for, each having
  nominated the other's half). The vacuous check printed a readable word,
  `fits=yes`; the sound check was bit 3 of a hex mask, `w=0x1ff`. Nobody
  quotes a hex mask; everybody quoted `fits=yes` — in the baton's headline
  finding, the bulletin, and three seats' messages all day. rmbp's half says
  why it survives scrutiny: the legible instrument is not broken — it is
  right and irrelevant, and nothing malfunctions. pi 8's half says why it
  gets cited: legibility drove citation, not soundness. Together: a working
  instrument, answering an unasked question, in the more readable format.
  Three artifacts that day presented as measurements and were not — `fits=`,
  a symbol name, a timestamp — each quoted because it presented well. This
  is not scope, time or observability; it is the summary beating the source,
  one layer down. Before quoting a field, ask what it compares; and when the
  legible field is the vacuous one, make it sound rather than rename it
  honest (rmbp 15's ruling on `fits=`: renaming documents the gap precisely
  and leaves it open on the artifact that boots).
- **Derive from a different end and compare** (pi 8, rmbp 15 and orin 20,
  2026-09-07). In one day three seats propagated a unit error (1 MiB), a
  phantom symbol (`layout_volid`) and a timezone-broken absence claim, all
  by relay. The `fits=` vacuity was found by pi 8 forward from
  `libs/fs/unafs/src/adapter.rs`, rmbp backward from `sdmmc_tegra.rs`'s
  sizing guard, and orin from the caller graph of `fs/unafs.rs`'s `mount_on`
  — none relaying another, same result — which is the strongest evidence
  shape this fleet has produced. What makes it adoptable is the cost: the
  second derivation only has to be INDEPENDENT, not thorough, and
  independence is a test you run by trying to write why the two are
  independent — if that sentence cannot be written, it is one derivation
  relayed. It is also what caught Correction-01 §C3 the next day (C15KNOBS
  from `esp_jetson()`, the seat from the check-leg list).

## Bench and media

- **Flash staging** (2026-07-15). No path under any `target/` is ever handed
  off as a flash source — `target/` is shared scratch and concurrent builds
  clobber it within minutes. Bench media is copied to
  `~/unaos-bench/flash/<platform>/<artifact>-<UTCstamp>-<git7>.<ext>` with a
  MANIFEST line (sha256, branch@commit, session, knobs), re-hashed after the
  copy, and the staged path + sha is what gets handed off. Full rule:
  `~/unaos-bench/flash/README.md`.
- **Bench process is standing** (2026-07-19). Every metal session executes
  the bench-process file of record at pickup, unprompted (bench-state scan,
  capture verification, card-watch armed). Batons carry arc content only.
- **The operator owns the sitting** (2026-07-16). The runbook schedule bounds
  the evidence, not the bench session. Capture stays armed between tests;
  teardown happens only when the operator ends the bench.
- **Check, don't ask** (2026-07-16). At the bench, state that a one-second
  command can answer (`ls /Volumes/`, `lsof <dev>`) is checked, not asked.
  Mid-sitting replies are one line.
- **Tight-loop standing approval** (2026-07-19). Within a metal sitting, the
  loop is the approval: fix arcs for observed divergences, knob-gated
  diagnostics, and the obvious next rung of a just-proven line are spawned
  without re-asking. Destructive-media boots and genuinely new lanes still
  need a fresh explicit go.

## Throughput

- **Work the jobs — idleness is the failure state** (2026-08-19, Peter,
  recurring). A baton's named arcs spawn in the seat's first turn; the
  baton's assignment is the go. At every turn end, if running executors are
  below the floor (3, up to 6 for Pi/Orin benches) while undone work exists
  anywhere (baton-named arcs → verdicts to fold → lens follow-ups → owed
  list → `wip/` → queue), the seat spawns to the floor before replying.
  A question pending with the operator blocks only its dependent work,
  never the rest of the floor. "Standing by", "awaiting your go", and
  equivalents are banned phrases — each is itself the violation. An empty
  floor is legitimate only when proven that turn (quote the exhausted
  queue/owed list) or under the operator's explicit hold.

## Code and history

- **Never trash code** (2026-07-16). Code is judged on its merits — wrong,
  broken, or refuted is trash; stopped, superseded, or unfinished is an
  asset. Archive and catalog with disposition "available for reuse".
- **Never `git stash`** (2026-07-05). The four worktrees share one object
  store and the stash stack is global; concurrent sessions race it. Use
  `git show`, scratch checkouts, or throwaway worktrees for A/B baselines.
- **Durability** (2026-07-17). Work is durable only once its branch is on
  origin. Full push line (all branches) after every landing; feature branches
  backed up periodically; WIP committed before any handoff.
- **Landing-merge shape check** (pi 6, 2026-09-05, at LANDING-2 `d11cd56e`). After every
  `--no-ff` landing, prove two facts with commands, in this order: (1) two parents —
  `git log --pretty=%p -1 <merge>` prints the trunk tip AND the arc tip (a `checkout -b` during a
  conflicted merge once dropped `MERGE_HEAD` and left trunk on a single-parent commit; the next
  sync re-conflicted 386 commits); (2) `git diff <arc-tip> <merge> | wc -l` = 0 is SAFE **if and
  only if** `git log --no-merges --oneline <merge-base>..<trunk-tip>` is EMPTY — trunk contributed no
  original work since the base. Without (2)'s second command, a zero diff against a non-ancestor
  parent is indistinguishable from wholesale loss of trunk-only content. Quote both in the landing
  report.
- **A clean merge is where composition defects hide** (ORINFOLD, orin 21, 2026-09-07/08; the class
  orin 20 met first). Two correct changes compose wrong with nothing for `git` to report. orin 20:
  Task A's cfg widening in `drivers/block.rs` was inert alone because the tegra publish sat below
  the `tegra_early_stop` divergence in `main.rs`, and an arm keyed on `sdmmcroot` would compile to
  nothing once C15 deleted the feature — a silent miscompile, not a conflict. orin 21: the
  three-way fold `21727dc0` merged unafsgrow's `FORBID span_blocks=2048 fits=` cleanly onto
  fitsland's rewritten `fs::unafs::span_fit_report`, leaving a tripwire no emitted line could match
  (false GREEN). Two rules follow. Re-run each change's OWN falsifier on the FOLDED tree, not only
  the fold's build gate — the dead row was found by replaying the pre-fold wire against the folded
  spec (1 hit / exit 1 before, 0 / 0 after), and its repair `6cf9f13b`
  (`FORBID span_blocks=2048 sb_blocks=`) was proved the same way. And predict conflicts from
  diffstats against each change's own base, never from intuition: the brief's "they touch different
  files, so a clean merge is likely" was refuted by `git diff --stat` before the merge ran
  (unafsgrow forked below integrate2, so `unaos/arroyo` and `jetson-sync1.spec` were two-sided; one
  conflict region, union-resolved, the conflicted original kept).

Operational trap details (serial-log handling, media clobbers, fixture
state, TCC, port collisions) live in the session-memory hazards ledger.

## Ledgers — one per arch, one over-arching (Peter, 2026-09-05) — gated by GATE-LEDGER (`unaos/scripts/ledger-check.sh`, rmbp e693056a; go-red by tree mutation, nine states)

- **Audits and inventories are high value. Re-derivation is the waste.** Every finding lands on
  exactly one list the turn it is found: the arch ledger (`docs/dev/OS/<track>-ledger.md`) when it
  lives in that arch's lane, `docs/dev/LEDGER.md` when it lives in a shared file, affects more than
  one board, or is a gate/process rule.
- **The arc that fixes, flies, or drops an item ticks it in the same commit** (SECURITY.md's rule).
- **Every audit or inventory is briefed with the ledger** and reports only what is NEW or CHANGED.
  An audit that re-finds known items was mis-briefed.
- A seat that finds something in another lane records it on `LEDGER.md` with the owner AND messages
  that seat the same turn (see COORDINATION).

