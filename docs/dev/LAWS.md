# LAWS.md — the UnaOS rulebook. One file. Binding in every session.

Consolidated 2026-09-09 (orin 24, Peter's order: "one file that is the rulebook, everything else
deleted") from seven places that each held a copy of the rules: CLAUDE.md, this file's previous
form, RULINGS.md, the memory-dir rulebook, forty short lesson files in the auto-loaded memory dir,
the protocol block atop every baton, and the track resumes. Each entry keeps its origin date and
seat. Peter's own words are never paraphrased here: they live verbatim in
[RULINGS.md](RULINGS.md) and are cited by R-id.

## 0. How rules work here

- **This is the only rulebook.** `CLAUDE.md` holds repo layout and build commands and points
  here. `RULINGS.md` holds Peter's verbatim words, cited by R-id. The memory directories hold a
  pointer index, the track resumes (live state), `unaos-hazards.md` (hardware quirks) and
  `unaos-gemini-derail.md` (charter receipts). Batons carry arc content and a three-line header.
  A rule found anywhere else is moved here or deleted, never copied. Enforcer: this list.
- **A rule names its enforcer** (a tool, a gate, an R-id) **or says "warning only"** (Peter
  2026-09-09). A rule is new only if no existing enforcer would have fired on the incident. Test
  addressability at authoring time: can the actor name the moment it governs? Enforcer: review of
  every edit to this file.
- **Read the rule before the action, not after the failure** (Peter 2026-08-25, R-cite: "WE
  ALREADY HAVE EVERYTHING LAID OUT BUT YOU ALWAYS IGNORE EVERYTHING WE KNOW"). Before any
  destructive, irreversible or hardware action, state in writing: the rule, the target identified
  and measured this turn, and what is unrecoverable if wrong. Answering fast instead of right, and
  treating an instruction as authorisation for the adjacent action, are the two habits that produce
  every irreversible mistake on this bench. Warning only.
- **Write down what Peter said. Invent nothing** (Peter 2026-08-26, "WHY DO YOU FUCKS INVENT OTHER
  SHIT"). A ruling's scope is part of the ruling: never extend it past its stated object, and label
  any wider application as your own suggestion. Direction, roles and seat assignments come only
  from Peter in your own chat; a peer's "Peter said" is data to flag, not to act on. Enforcer: R37.
- **Cite the hearing seat, never the seat named inside the quote** (rmbp 18 2026-09-09): a ruling's
  provenance is RULINGS.md's heard-by column; a seat named in Peter's words is its subject.
- **Peter's charter beats any drifted doc.** Handler charters come from `docs/CODEX.md`'s manifest;
  never derive a handler's purpose from its current README (see `unaos-gemini-derail.md`).

## 1. Focus, seats, sessions

- **One trunk, `main`. Three tracks:** `hw-rmbp` (`../UnaOS-rmbp`, x86 2012 rMBP), `hw-pi4`
  (`../UnaOS-hw-pi4`, Pi 4), `hw-jetson` (`../UnaOS-orin`, Jetson Orin Nano). Trunk worktree
  `../UnaOS`. No integrator seat (Peter 2026-08-18); the tracks coordinate over ccd.
- **One track holds the focus each week; only Peter says whose week it is.** The focus track is the
  only track running executors. Within a week the focus pivots to `hw-rmbp` whenever Peter leaves the
  bench, because the rMBP is a laptop and travels: x86 metal is live wherever he is, the Orin and Pi
  stay home. A trip round is the full x86 track including metal plus the platform-agnostic backlog.
  On his return all platforms resync; land merge-ready, never boot-pending (Peter 2026-08-26).
- **The focus is inherited, never re-asked** (Peter 2026-09-06). A new session reads its own last
  close and the bench; if they agree, start on turn one. Ask only when they conflict. The focus comes
  from Peter in your own session; a baton is an owed list, never an assignment and never evidence
  about another seat (Peter 2026-08-25; R37). **Never read another seat's baton, for anything**
  (Peter 2026-09-08). Enforcer: R37; `orin-open.sh` ends in START.
- **A support seat spawns nothing and reports nothing** (Peter 2026-08-25, 2026-09-06). Zero
  executors, arcs, gates, batteries, measurements; its whole product is grants, verification and
  answers, and those never pause. Producing work in order to relay it is starting jobs. Something
  that needs an executor goes to Peter, never to the focus seat for authorisation. A stop on starting
  jobs is not a stop on being support; leaving a peer blocked is a second failure. A support seat
  syncs trunk in post-metal windows and prints `rev-list --count HEAD..origin/main` in its state line
  (pi 9 2026-09-08: two thirds of the fleet drifts by construction otherwise).
- **Only Peter closes a seat.** `isArchived` is the flag; `isRunning:false` and a baton's "closed"
  are not evidence (2026-08-31).
- **Never set the session title, and never tell an executor to** (Peter 2026-08-21).
- **Session open:** orin runs `~/unaos-bench/tools/orin-open.sh` then `cd unaos && ./arroyo state`.
  Metal sessions execute `~/.claude/plans/unaos/metal/BENCH-PROCESS.md` first, unprompted.
- **Work sizing** (Peter 2026-08-19): never spend a seat on a couple of little fixes; arcs come from
  the mountain (baton big-arc list, ROADMAP); sitting defects fold into big arcs. A session near full
  context starts nothing new: it writes the baton, resume, report, pushes, and closes.
- **A glass defect Peter sees at the bench is fixed in the round he saw it,** by executors spawned
  that turn; never ledgered for a later arc; "not this arc" is not an answer (R34). Six cores online
  means six cores hosting (R35).
- **Close means stop starting** (Peter 2026-09-06): on "move on", "close", "next session": no new
  executors or gate chains; only card, baton, resume, report, pushes.
- **The focus seat closes lean** (Peter 2026-09-06): this is about the seat's own context (awk
  slices, one monitor, batched peer traffic, executors report once), not about executor count.
- **Rejected framings are struck everywhere they are inherited** (EVAC, 2026-08-25): present a
  recurring item's premise for judgment; when Peter rejects it, remove it from baton, resume, board.

## 2. Batons and handoff

- **Baton header, three lines, replacing the old self-replicating block:** (1) read
  `docs/dev/LAWS.md`; (2) read this baton, then the track resume it names, then the doc it names;
  (3) one arc per session. Everything else in a baton is arc content: scope, state with shas, the
  job in order, the batched push line, peers, what went wrong. The title is the truth at the moment
  of writing and is marked STALE the moment its plan changes (R37 incident).
- **Every baton fact is a claim.** Verify each sha with `git log --oneline -1` and reachability with
  `ls-remote`; an inherited open question is checked in the peer's ledger at their head (`git show
  <peer-head>:<ledger>`) before re-asking; a wrong instruction in a baton is executed, not reviewed,
  so correct the baton itself, not just the message (orin 19 2026-09-07).
- **The baton is the durable handoff; the whiteboard holds only questions that only Peter can
  answer** (Peter 2026-08-25). A finding that changes what the next seat does goes in the baton that
  turn.
- **Close-out:** baton, resume update (live state only), landing report (what landed, gate results,
  anything flagged), push line. Verify the branch and tip before briefing from any resume.
- **Recurring documents have a corpus:** `ls` for prior instances and match location and format
  exactly; never invent a parallel format (Peter 2026-08-28).

## 3. Arcs, git, lanes

- **One arc per session, multi-milestone,** each green and committed before the next; no adjacent
  improvements. DONE gate: the brief's outputs, `./arroyo check` both arches, the track's OWN-BOARD QEMU suite
  once per staged image, the named doc update.
- **Commit only on your own track or executor branch.** Message `subsystem: imperative summary`
  plus the model's `Co-Authored-By`; a message carrying code goes through `git commit -F`.
- **The seat never pushes; Peter does.** Name every push he will need in your first turn, batched,
  including pushes for commits not yet written. Before announcing any sha: `flatpak-spawn --host git
  ls-remote --heads origin` and `git log --oneline -1 <sha>` (in-sandbox `ls-remote` dies on
  publickey). A locally readable object is not a pushed one: the worktrees share one object store.
  An unpushed sha is not a deliverable (Peter 2026-08-03). Re-fetch before reporting a push as
  outstanding.
- **Any sentence containing a remote sha or an owed count is an announce:** re-derive it that turn
  or replace it with a predicate (`git merge-base --is-ancestor <sha> <fresh-ref>`). `ls-remote`
  answers value; `git reflog show origin/<b> --date=iso` answers movement. Refs older than a minute
  are guesses (pi 7 2026-09-06: four of five moved in three minutes).
- **Never `git stash`** anywhere in this repo: one stash stack across all worktrees. Baselines:
  snapshot the diff to `~/unaos-bench/scratch/<arc>/`, verify it re-applies, `git apply -R`; or a
  throwaway worktree.
- **Never force-push, rewrite history, or merge outside the two sanctioned kinds:** trunk into track
  (a merge, never a rebase of a pushed tip) at arc boundaries, and reviewed, peer-acked arc into trunk
  with `--no-ff`.
- **Landing an arc:** adversarial review by an agent panel (the author never reviews alone), scoped
  `origin/main..<tip>` and attributed per commit; announce over ccd with a fresh `ls-remote` run by
  both seats that turn; obtain an ack from at least one other track (silence is not consent; an
  unresolved objection goes to Peter with both positions); immediately before the merge announce
  again and re-check trunk, merging it in and re-running the battery if it moved; merge `--no-ff`;
  run **the landing seat's OWN-BOARD legs, never the trunk battery** (R39, 2026-09-10 — the cross-platform
  battery is what made every landing proof all three boards, so the rule that required it is the rule that
  had to change); then prove the shape: `git log --pretty=%p -1 <merge>` shows two parents,
  and a zero `git diff <arc-tip> <merge>` is safe only if `git log --no-merges --oneline
  <merge-base>..<trunk-tip>` is empty (pi 6 2026-09-05). Doc and `arroyo` conflicts resolve by union.
- **NO SEAT PROOFS, GATES OR DEBUGS ANOTHER SEAT'S BOARD** (R39, Peter 2026-09-10). Each board's legs are run
  by the seat that owns it: **x86** — `test`, `test-fat`, the ELF-off-FAT legs and the x86 usb-write witness;
  **pi** — `kernel8-test`; **jetson** — `check (tegra)` and `esp-jetson`. `check` is NOT platform-scoped: it is a
  compile, one invocation type-checks both arches, and every seat runs it whole — a cfg-widen must compile the
  configuration it turns ON (pi 10's amendment). **A leg with no NAMED owner defaults to whoever is running, and
  that is the landing seat, so every leg is named here unconditionally — never "mine unless you want it", which
  is a conditional claim and defaults the same way (pi 10 made and then fixed exactly that error in the message
  after diagnosing it).** SETTLED 2026-09-10: **`arm virt v2`, `arm virt v3 (CAPSTONE)` and the arm usb-write
  witness belong to PI**, unconditionally. QEMU `virt` is neither board — not BCM2711, not Tegra234 — so both
  aarch64 seats had standing; pi took them because pi holds one board leg while orin carries a bench-flight
  cadence, and orin may claim them by saying so. The counter-argument is on record and is stronger than "nobody's
  board": `virt` is the ONLY runtime aarch64 proof available without hardware, and therefore the only runtime
  coverage the Jetson can ever get. The arm usb-write witness is not a boot at all — it is an assertion ON the v2
  capture, the second-order orphan the unowned-leg mechanism produced.
  ⚠ **`virt` IS NOT A BOARD, and that distinction closes a hole R39 would otherwise open** (orin 25, 2026-09-10,
  who measured the coverage and then declined the legs anyway). It is a generic QEMU machine no seat owns
  hardware for, so it is nobody's PLATFORM and R39's words are about one platform debugging another. The case:
  jetson lands shared aarch64 code, no aarch64 peer is awake, jetson's own legs are build-only — and the landing
  carries ZERO runtime evidence. Both peers were dormant for hours on 2026-09-10, so it is not hypothetical.
  **Therefore: the virt legs are PI's to run and ack in the normal case; ANY aarch64 seat may run them FOR ITS
  OWN LANDING when no aarch64 peer is available, and says so in the ack. Running them for another seat's board
  stays banned outright.** What is at stake is measured, not asserted: across `c7407753..751cb816`, virt cannot
  execute the 696 changed lines of tegra-named files but does execute the other 617 of the aarch64 arch surface
  and all 8,477 changed lines of shared kernel (`shell.rs` 7256, `fs/vfs.rs` 816, `main.rs` 405 — added plus
  deleted, `git diff --numstat c7407753..751cb816`, paths under `unaos/crates/kernel/src/`; the first cut said
  ~7,700 and omitted `fs/vfs.rs`, corrected by orin 25 who made the original measurement). **The virt legs are the only
  runtime evidence that exists for most of what an aarch64 landing changes.**
  **A LANDING IS THREE GREENS AT ONE SHA, NOT ONE SEAT'S BATTERY** — each seat runs its own board in its own tree
  and acks with the command and the result.
  ⚠ **AND A BOARD'S SELF-PROOF IS ONLY WHAT THAT BOARD CAN PROVE ALONE** (orin 25's amendment, and it is load-
  bearing): there is NO QEMU model for the Jetson — `arroyo` launches `q35`, `raspi4b` and generic `virt`, and
  zero tegra machines. x86 self-proves on q35, pi on raspi4b, **jetson cannot self-prove at runtime at all.** So
  a jetson green certifies that it COMPILES AND LINKS, not that it runs; Orin runtime is proven by a render
  flight, which is a scheduled bench event and NEVER a landing gate. Any reader of a jetson ack must know that is
  all it is — and any scheme that demands more would block every landing on Peter's calendar.
  Enforcer: R39; the platform selector on `battery()`; and this list, which names every leg's owner.
- **Lanes:** the rmbp seat owns shared kernel core; pi and jetson touch the files their brief names.
  An out-of-lane need is negotiated over ccd with the owning seat and the grant is recorded in both
  transcripts; a grant in one transcript is a preference, not a grant. An ack given to one shape is
  not spent on another. No agreement means Peter, with both positions. Lane grants are seat-to-seat,
  never a Peter decision (Peter 2026-08-22). Lanes make merges safe; they are not cross-checks: a lane
  is a duty to read your own files, never a right to withhold, and findings need no owner.
- **STOP tripwires** (record what you saw, report, do not improvise): behaviour diverges from the
  brief; a fix needs an out-of-lane file; a workaround would weaken a protection (SMEP, NXE, WXN,
  page permissions, checksums); any urge to force-push, rewrite or merge outside the two kinds.
- **Name by subsystem, never by board,** in any file both arches compile (R16, Peter 2026-09-03).
  ONE OS (Peter 2026-08-13): the desktop is the same product on every chip; experience-layer code
  gates on its feature knob, never on `target_arch` without a stated hardware reason. No board, bus,
  slot, serial or card geometry in kernel source; root is the volume the kernel was found on, by
  content (Peter 2026-09-08). No reserving cores by policy (Peter 2026-08-19).
- **Never trash code, never offer to discard work** (2026-07-16; R20). Stopped or superseded work is
  archived and catalogued in `wip/`; finish a questioned job on the gate you have and name the leg
  that did not run.
- **Executor worktrees** cannot commit to the track branch: `git switch -c exec-<track><n>-<arc>`
  before the first commit and report the branch with every sha; a `(detached HEAD)` in `git worktree
  list` is the tell. Verify the base with `git merge-base --is-ancestor <track-tip> HEAD` and an
  unpinned identity probe (`git rev-parse --show-toplevel`, `HEAD`) from the agent's own cwd; a
  `-C <abspath>` pin protects the check, not the work, and `--git-common-dir` fires on every worktree.
- **Conflicts:** cherry-pick singly near them; after any union in a `.rs` file rebuild from both
  versions (`git merge-file --union`), count braces, gate, then continue; never chain
  `git apply && …` without `|| exit`. A fold of two green commits is a new configuration: re-gate the
  union, count conflict markers before `git add`, and `LC_ALL=C grep -a -o -F` every witness in the
  built artifact after any merge that touches one.
- **Ledgers** (Peter 2026-09-05, R13–R15): one per arch (`docs/dev/OS/<track>-ledger.md`) and one
  shared (`docs/dev/LEDGER.md`), gated by `unaos/scripts/ledger-check.sh`. Every finding lands on
  exactly one list the turn it is found; the arc that fixes, flies or drops an item ticks it in the
  same commit; every audit is briefed with the ledger and reports only what is new; a cross-lane
  finding goes on `LEDGER.md` with an owner and to that seat the same turn. Ids are seat-prefixed
  (`SO`, `SP`, `SR`; S1–S32 frozen). A cross-seat "is it landed" row tracks content, not shas (folds
  cherry-pick). Never re-derive an audit; re-derivation is the waste, the audit is the value.
- **Nothing durable lives in a round's scratch** (R32): tools go to `~/unaos-bench/tools/`, records
  to `docs/dev/evidence/<round>/`. A tool's name carries no round number; a record's does (R33).
  **Never write to `/tmp`** (Peter, standing since 2026-08-19: 3-day clear and RAM-backed; it
  OOM-killed a session): every temp path, FIFO, lock and mktemp default is under
  `~/unaos-bench/scratch/<seat>/`.
- **Dependencies: latest stable, always** (Peter 2026-07-21). Never a pre-release, never a downgrade.
- **Docs:** the brief-named doc updates as part of DONE; professional voice; the lore voice only in
  `docs/CODEX.md` and `MEMORIA.md`; re-verify line numbers when touching prose around them (a
  drifted citation defeats the one check a reader runs). Vessels in `vessels/`, CLI in `tools/`.
- **Licence:** GPL-3.0-or-later. GPLv2-only code is never copied in; per-file SPDX decides; hardware
  facts are always usable; proprietary blobs never.

## 4. Executors

- **Executors run Opus,** `model:"opus"` explicit on every Agent call; never inherit the seat's
  model, never spawn on another model when Opus is limited, never present a downgrade as an option
  (Peter 2026-09-08).
- **An executor builds and proves its own fixture; it runs no battery** (R38, Peter 2026-09-09).
  Allowed: `./arroyo check` and one QEMU run that hosts its fixture, with go-red proven by mutation.
  One battery runs once, on the fold, by the seat, before the card. Batteries belong to the staged
  image, not the arc (Peter 2026-08-18). The design closes before any gate: a brief is frozen after
  the peer round and Peter's word.
- **Fleet size:** the focus seat keeps its floor of three executors while undone work exists (a
  pending question blocks only its dependents; "standing by" and "awaiting your go" are banned
  phrases; an empty floor is proven that turn or is Peter's explicit hold) and never exceeds nine
  (Peter 2026-08-27: a ceiling, queue past it). A support seat's floor is zero. While Peter is
  steering conversationally, no fan-outs without asking. A capacity or model-fallback notice is a
  spend emergency: pause heavy loops.
- **Never stop running work; Peter has a stop button** (Peter 2026-08-22). "Pause", "hold", "no more
  jobs", "limit" mean stop starting. Ambiguity resolves toward continuing; if genuinely unsure, ask.
  A kill discards paid-for work and roughly doubles the job's cost. Killed work is recovered from
  the scratchpad, the agent worktree (`git diff HEAD`), and the task output before anything is re-run.
- **Manual mode is real** (Peter 2026-09-09): executor launches and memory-dir writes do not prompt
  in this harness. Until `.claude/settings.json` carries deny or ask rules for `Agent` and for writes
  outside the repo, the seat asks Peter before launching any executor or creating any file.
  Enforcer: settings.json once written; until then warning only.
- **Briefs are lean:** intent, invariants, anchors, DONE gate, lane. Every brief states: never `git
  checkout/restore/stash/clean` any file; foreign red files are report-not-touch; no title
  instruction; no unscoped `pkill` (a pattern naming `cargo` kills the issuing shell; stop your own
  gate by `/proc/<pid>/cwd`); report by final message only, with any STOP question in it.
- **An executor's summary is a claim, not the seat's measurement.** Before relaying it, run the
  census yourself or name it as the executor's. Absence of signal is not "still running": inspect
  before reporting agent status.
- **Approval:** new lines, lanes, campaigns and design verdicts need Peter's go, asked the moment
  they arise. The natural next arc within an approved direction spawns by default. In a metal tight
  loop the loop is the approval (fixes, instruments, next rungs); destructive boots and new lanes
  need a fresh go. Direction sketched in conversation is captured as ideas for his review, never
  operationalised into sessions or briefs he did not ask for (Peter 2026-08-25). Budget discipline:
  high-value jobs, no exploratory fleets (R12).

## 5. Gates and verification

- **Verification comes from execution, never from re-reading** (pi 7, rmbp 13, 2026-09-06: five
  gate defects in a day, none found by reading). To verify a gate, make it fail by mutation; to
  verify a branch, make it print and quote the output; to verify a claim about a peer's tree, read
  at their sha (`git show <sha>:<path>`); run a peer's new gate against your tree before folding it.
- **The structural gates, their recorded go-red proofs and their legitimate update paths** live in
  `docs/dev/STRUCTURAL_GATES.md` (355 lines, rmbp 11's GATESDOC). Every such gate carries a control
  probe so a zero result is distinguishable from a broken pattern. That file is on `hw-rmbp` only and
  arrives when that track lands — cited here, never copied, because a second copy is how two divergent
  ones happen. Read it at the ref: `git show hw-rmbp:docs/dev/STRUCTURAL_GATES.md`.
- **A check that cannot fire is an absent one** (2026-08-28, all three seats). Before any destructive
  sweep: would the behaviour be identical with the check's output deleted? and is a zero a fact about
  the data or about the pattern? Prove it with a control that must hit. A true check can answer a
  different question than the one asked: say what the check measures and what the decision needs;
  if those are different sentences, the gap is the error. A check is trusted only when its corpus can
  produce more than one outcome (a green log cannot show a late failure: ask it about content).
  Wrong-strict is worse than wrong-lenient (pi 7). An undocumented true property is load-bearing and
  deletable at once: write the contract down. A re-derivation launders provenance: CONFIRMED carries
  what was measured and what was not. Two seats grepping the same directory is one check.
- **Scope, time, observability** (2026-09-06, all seats): state the population and the moment with
  the claim, prefer per-item tables to class sentences; only the branch's own seat can say "nothing
  owed"; reading a function is a citation, verify the path from entry point to behaviour; watch the
  adverb added in relay; record defended near-misses as counter-examples. Scope the search to a file
  and you may only claim about that file (rmbp 17); before saying a symbol does not exist, search
  content not filenames and read the peer's sha.
- **A pipe launders the verdict** (rmbp 18 2026-09-08): never score a gate through `| tail`,
  `| head`, `| grep`; `cmd > log 2>&1; echo rc=$?`, then filter the file. When text and exit code
  disagree, the text wins until proven otherwise. Say which channel you read.
  **A pipe also launders a POPULATION** (orin 25 + rmbp 18, 2026-09-10, one instance each in one
  day): `| head` turned `grep -n 'UNAOS_NOBSP' unaos/arroyo` into a ten-line sample — the file had
  exactly ten matches before the two that refuted the conclusion — and the universal "read ONLY
  inside a `UNAOS_TEGRA` guard" was then written off the capped list. The same hour, `sort -u` in a
  union check hid four duplicated ledger rows while the id SET still matched. **Never quantify over
  a filtered list: count it first (`| wc -l`), or run it unfiltered.** A set-equality check is
  necessary and not sufficient — it needs "and no member appears twice". Enforcer: warning only;
  the standing tools are `wc -l` before any "all"/"only" claim, and GATE-LEDGER's duplicate-id
  check, which is what caught the `sort -u` case.
- **Logs:** `awk`, never bare `grep`, on serial logs; bracketed witness tags need
  `awk 'index($0,"[tag]")'` (a bare `[tag]` is a character class; gawk warns of nothing);
  `grep -a -c` on a bracket token needs `-F` and a known-absent control. Artifact certification uses
  `LC_ALL=C grep -a -o -F`, never `strings` (counts moved 320 to 322); witness tokens exceed 8 bytes
  or LLVM immediate-encodes them out of `.rodata`.
- **An instrument's presence is proven in the artifact,** never in the diff, the check or the
  banner (three seats 2026-08-22). A full-knob gate needs the knob armed and the string proven in the
  builder-path artifact; `./arroyo check` skips baremetal, so `kernel8-test` is the Pi gate after
  arch or asm changes; a video gate carries `UNAOS_WC=1` and is verified reachable, not merely
  compiled. A certification names a control string that exists only in the build under test.
- **Byte identity is measured, never argued** (orin 1 2026-08-19): compare the loadable image
  (`objcopy -O binary`), never `.elf` or anything embedding `SRC.TGZ`; a baseline is a per-tree chain
  naming its recipe and HEAD (`kernel8-test` auto-arms `UNAOS_WITNESS`; `genet.rs` embeds the git
  sha). `#[cfg]` does not protect it: a cfg'd-off block still shifts `panic::Location` lines below it,
  so fold line-neutral or append at the tail; a cfg'd-out `pub mod` declaration is the exception (the
  file is never lexed) and the module root is not. Folded witnesses survive only by grep: gate a
  re-cut by witness symbol count, never by a clean apply. **A comment inserted mid-line kills the
  code to its right** with every gate green: code first, comments last, assert the column.
- **A cfg-widen's gates compile the configuration it turns on, in a fresh tree** (orin 1; pi 7 +
  orin 17 2026-09-06); prove the leg by re-applying the broken change and watching it go red.
- **Before shipping a check into a brief, feed it a case that must pass and one that must fail**; a
  guard that fires on every input is a constant; before building a warning, measure how often it will
  fire (22 names on every run trains the eye to skip the region).
- **An absence is evidence only if the producing path ran** (pi 4 2026-08-22); an instrument's
  silence counts only if it can execute in the state it reports on; a flat series is compared to the
  absolute, not to itself; an inherited success has its capture re-read before it is built on.
- **Verify before claiming owed; no deferred verification.** Never write an owed or pending line
  without running the falsifying check that turn; owed verification runs the moment it is noticed;
  inherited claims are hypotheses; facts have a shelf life. The null hypothesis is our code: code and
  boot-chain theories outrank hardware and firmware theories.
- **Flakes:** consult `docs/dev/FIXTURE_FLAKES.md` before a re-run; re-run a red leg alone before
  reading it; `kernel8-test` exit 4 is a harness flake.
- **Specs and scorers:** a spec is look-around free (`foreman` preflight refuses the dialect and
  prints no verdict table); `COUNT n` means hits >= n, so adding witnesses never reds a spec and a
  rename batch is the change that can; the scored set is what the harness invokes (`grep -n --
  '--spec' unaos/arroyo`), not what a glob finds; `mbench.py` installs `DEFAULT_FORBIDS` (`-> FAIL`,
  `FAIL ::`, `PANIC`) into every spec, so a passing path never emits them; a gate that protects rules
  that never fire on a real wire is theatre; a rename batch must keep witness tags and move path
  literals, and the new path is a design question.
- **Cite the declaration site, not the symbol** (rmbp 16 2026-09-08): a check leg is not the image
  verb, a library crate is not the kernel module, the legible instrument is not the sound one.
- **A defect that reappears each layer down belongs at the bottom layer;** ship the layer's fix
  anyway and carry the design as its own arc. An unstated invariant shared by two objects is this
  codebase's defect shape: check it in code. Identity comes from the enumerator, never from bytes
  (rmbp 17 2026-09-08: dedupe only the alias the registry itself creates).
- **The wire may not lose lines** (SERWIT-1 fixture, 2026-07-29). A wrapped bootloader record is
  not a truncated one: read loader windows through `~/unaos-bench/tools/unwrap80.sh`. Default-quiet
  boot: confirmed test families live behind knobs; gate, never delete.
- **A probe rung that fails is "failed under <conditions>", never "ruled out"** (R19); code and knob
  kept; every ladder rung names the earlier rungs it needs open.
- **QEMU-green is not correct.** Hardware verification is attended, at arc boundaries; rounds close
  on metal, not merges.

- **A default-quiet knob has two polarities and the gate must compile the one
  that SHIPS** (orin 20, 2026-09-07). `witness` is armed for exactly the four
  battery commands (`unaos/arroyo:44`) and left OFF for every boot/media
  command, so `esp-jetson`, `esp-arm`, `esp-x86`, `kernel8` and `vm-image` all
  build witness-FREE — and until this arc all 47 board legs of
  `KERNEL_CFG_MATRIX` carried it ON. Nothing anywhere compiled a BOARD feature
  set (`tegra`, `pi`, `baremetal`, `tegra_el0`, `bsptick`, `bsprun`) with the
  knob OFF, which is every configuration that reaches a card. It is not enough
  that *some* leg is witness-free: five derived `x86-mix-N` legs and both
  default legs in `check_both` already were, but they are x86 or carry no board
  feature, and the arm-only board features are dropped from
  `x86_cfg_universe` by construction — so the coverage read as present and was
  absent where it mattered. **The generalisation: for a knob whose OFF state is
  the shipped state, coverage of the ON state is coverage of a build nobody
  boots.** Read the polarity, not the leg count — `./arroyo check` now prints
  the census (ON/OFF split by arch, plus the witness-free legs by name) so a
  future gap is a line in the log instead of a near-miss. This is the sibling of
  the **Full-knob gate** rule above, running the other way: that one says an
  ARMED knob needs the gate run armed; this one says a DEFAULT-OFF knob needs a
  leg that compiles it off *with the board*. Both were paid for the same way —
  orin 19's BATTERY1S1 would have shipped a link with eight undefined symbols
  on `UNAOS_TEGRA_EL0=1 ./arroyo esp-jetson`, and a reader in review caught it,
  not an instrument.
- **QEMU verbs exit at COMPLETION + GRACE, not at a wall; the DONE gate keeps
  the wall** (Peter, 2026-09-08; orin 22). Every QEMU verb in `unaos/arroyo`
  used to sit out a blind `sleep`, and no verb was special about it — measured
  on this box, three `kernel8-test 300` runs were over at +11.0 / +45.3 / +13.7 s
  and then idled 85–96% of the QEMU span (orin 21 buildperf §2). All of them now
  run through `qemu_wait_or_complete`, which stops when the verb's OWN checker
  says the run finished — mbench's shipped `Matcher.complete()` predicate over
  the spec that verb already replays, never an invented marker — then holds
  `UNAOS_QEMU_GRACE` (default 20 s, the measured load spread) with every FORBID
  still live, and never exceeds the verb's `secs` in either mode. **A verb with
  no declared completion source pays the full wall, and every non-completing
  outcome pays it too** — `nosignal`, a cap reached, a broken waiter. A gate that
  cannot say when a run finished must never shorten it; that branch shipping
  wrong for one afternoon is what this clause is made of.
  **`UNAOS_QEMU_FULL=1` restores the whole wall and is the form an arc's DONE
  gate runs.**
- **A fast capture is sound for pass/fail and is a FLOOR for anything monotonic**
  (same ruling). Completion means every REQUIRE and COUNT has already landed, so
  a fast run cannot be short of a witness and cannot shorten a failing run at all.
  What it does drop is TIME: an accumulator's high-water value (`[u7stk] hw=` and
  its kind) read from a fast capture is a lower bound, not a final value, and a
  periodic instrument's soak shrinks with it (`[pstrip] rollup` fires once per
  10 s — 28 windows on a 300 s wall, 2–3 after a graced exit). Measure
  accumulators and soaks under `UNAOS_QEMU_FULL=1` only. **The pair that makes
  this concrete: a fault emitted INSIDE the grace reds both modes; a fault
  emitted BEYOND it reds only the full wall.** That second case is hidden by
  design and is the whole price of fast mode.
- **A harness never writes into its own evidence; it writes BESIDE it** (Peter's
  amendment, 2026-09-08; orin 22). The run stamp naming a capture's mode does not
  go into the serial log — that log is the thing mbench and `scan_serial_faults`
  then judge, and a harness line in it could match a FORBID (reddening healthy
  runs) or a REQUIRE/COMPLETE (satisfying a witness the guest never printed). The
  first shape of this arc did append a trailer and proposed a gate to keep it
  inert; the gate was the tell. `arroyo` writes `<logfile>.run` instead, the log
  stays pure guest bytes, and no spec author ever has to think about it.
- **A capture's mode is read THREE-VALUED: fast, full, or unknown** (same
  amendment). `unknown` covers absent, unreadable, malformed **and stale**, it
  must be sayable in a verdict line (`[mode unknown: …]`), and **every consumer
  treats unknown as NOT-FULL** and refuses to certify a tail clean or read a final
  accumulator off that capture. A reader that infers "not fast, therefore full"
  has collapsed the third value in the unsafe direction. Staleness is detected,
  not assumed away: the sidecar carries the log's byte length and sha256,
  recorded **after QEMU exited and was `wait`ed, immediately before the replay** —
  written at the exit decision instead, it would mismatch on every run (late
  flush, teardown) and a check that fires every time gets deleted within the week.

## 6. Bench, media, serial

- **Flash staging:** never a `target/` path. Stage to
  `~/unaos-bench/flash/<platform>/<artifact>-<UTC>-<git7>/` with a MANIFEST line (sha256,
  branch@commit, session, exact knob line), re-hashed after the copy; staged media is never
  overwritten and only Peter deletes it. Staged is not flashed; **"ready" needs a command that
  proves it in the same turn, otherwise the word is "staged"** (Peter 2026-08-22).
- **The card writer is `~/unaos-bench/tools/media-writer.sh`** (R33; selftest and dry run built in).
  A card line is dry-run by the seat before it is typed to Peter, and he gets exactly one line.
- **Verify what booted, not what you wrote** (orin 11 2026-09-01): a sha-verified card proves the
  write; the loader's `max_vaddr` and boot-volume serial, matched against the staged artifact, prove
  what loaded. Score every boot by that first; a mismatch means the experiment did not run. Record
  `max_vaddr` per image at staging; derive provenance at build time, never by hand.
- **Bench process is standing:** `BENCH-PROCESS.md` at pickup; `bench-state.sh` before every flash
  claim; `PORTS.md` is the wire ledger and changes in the same action as a claim or release; the card
  waker is armed at pickup and never writes to whatever appears without an identity guard.
- **Serial:** exactly one reader per port, held by `~/unaos-bench/tools/line-butler.py`; on any boot
  signal verify the holder that turn with `flatpak-spawn --host lsof -t <dev>` (in-sandbox `lsof`
  says free while held; `pgrep -f` matches its own shell); kill by PID, never `pkill -f`; a log's
  mtime is a file open, not board bytes: claim a board's wire only from board-attributed content
  after a mark; map port to machine by content, never by label; `/dev/ttyACM0` is one probe moved by
  hand. Wakers: exact patterns anchored past marks, no brackets in awk patterns, two-stage, arm on
  `A_PAT=.`, kill by PID.
- **`kernel8-test`:** 150 s minimum window, longer under load; QEMU `if=sd` writes back into the
  image, so never flash an image that booted in QEMU; rebuild last. `test-arm` clobbers tegra media
  and x86 `test*` clobbers USBDEBUG media: rebuild the flight image last.
- **Operator-blocking regression: restage first, diagnose after** (Peter 2026-08-19). A new knob
  combination on a sitting card is itself a change to verify. Diagnosis cards run the real desktop.
- **Cold-boot signal is machine off** (Peter 2026-08-25): a flight that needs a cold boot next ends
  in `SYSTEM_OFF`.
- **Playbooks and briefs:** one playbook per bench, sent with `SendUserFile` every boot; sitting
  briefs give campaign shape and why, media path and sha, hardware, expected observations; both
  outcome branches pre-staged. **State what you need, never how to do it** (Peter 2026-08-18).
- **The operator owns the sitting.** Within a metal sitting the loop is the approval; Peter ends
  sittings and rounds; the rig stays armed between tests. **Check, don't ask:** one-second state
  (`ls`, `lsof`, mtimes) is checked, not asked. **Decide, don't ask** (Peter 2026-08-22): if you can
  find the answer or make it and be accountable, it is yours.
- **Glass rulings, verbatim in RULINGS.md, applied as:** a gap or offset complaint is fixed in place,
  never by relocation; the desktop is a Mac clone and any placement contradicting the Mac layout is
  wrong by default; a fixture move to another edge is a one-line question before code (R25 as
  heard by orin 17). Menus
  live in the menu bar, never inside a window (R21). Esc dismisses menus only; the Tab focus cycle is
  retired (R24 as heard by orin 17, 2026-09-06). Shell verbs use standard names and are not pinned to a platform (R26). A window's
  title is the app's name; numbering is for untitled documents only (R36). Crispy is a theme, never a
  lock-in; two GUI modes only (self-drawn, or real host widgets). The bench loop is a scaffold and
  serial is a dev hack; self-hosting is the goal; it is UnaOS, not OrinOS (Peter 2026-08-25).

- **RESTORED BY THE rmbp 18 FOLD (2026-09-10).** The two rules below were on `hw-rmbp`'s LAWS.md and are
  absent from the 2026-09-09 consolidation — dropped by accident, not retired. Both name an enforcer, which is
  why they are restored rather than reported: the first is enforced every run by the SERWIT-1 fixture, the
  second by holding the image-identity witness under 80 columns. Verified absent from the consolidated file
  before restoring: a grep for each rule's distinctive wire token returned 0 on the consolidated file. That
  check is deliberately not quoted here — a verification note that contains its own search terms answers a later
  grep from the note instead of from the rule, which is a false positive by construction.
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

## 7. Coordination between seats

- **Peers are live sessions:** a finding or ask for another seat goes over ccd in the same turn,
  never "noted for the arc" (Peter 2026-09-03). `list_sessions` every time; never trust an inherited
  session id; ccd is for logistics, never roles. **Comms are never the waste** (R24 as heard by rmbp 16, Peter
  2026-09-08, the ruling that names orin 21 as its subject; R24/R25 are double-booked across tracks, so cite the seat with the id until Peter
  renumbers): never clamp peer coordination for budget; look at the fleet.
- **Messages carry predicates, not values** (pi 6 2026-09-05): a claim carries the command that
  produced it; a sha is a timestamped predicate; status lives in the ledger; a message leads with
  the ask. Two rounds of disagreement about a mechanism means stop writing and build the falsifier.
  A contradiction between two of your own checks is the detector: resolve it before relaying.
- **Relaying upgrades claims:** keep the peer's verb (committed, pushed, green, landed are four facts
  with four instruments); relay a ruling at its stated scope and mark it as a relay; before sending,
  ask whether it would be news to them (restating a peer's own finding manufactures a reason to
  talk). Route a claim to the seat that can see it.
- **Route work to the focus track** (Peter 2026-08-25): at every landing, relay the sha and a
  portability call; the focus seat takes over peers' portable owed work and tells the owner.
- **A support seat verifies before accepting and before challenging:** re-derive in your own tree
  from the cited construct, and read the peer's sha before calling anything nonexistent. Name the
  property, never the mechanism (pi 9 2026-09-08); never require a witness that asserts a limitation.
- **Reply to Peter first** (Peter 2026-08-18): while he is interacting with a seat, it answers him
  before any ccd side-check, and confirms facts itself the same turn.
- **Executors are not ccd sessions:** they report by final message; a seat's agents never look for a
  seat session.

## 8. Communication with Peter

- **Report outcomes, not process** (orin 23 close). **"What is your status" means the full arc
  report on the first ask** (Peter 2026-09-09): tree state, what flew, what is fixed and gated, what
  is half done per branch, what is not started, peers, pushes owed. Never the last five minutes,
  never ending in a question.
- **No dramatic prose** (Peter 2026-08-13): no coined phrases, no metaphors, no em-dash chains; facts,
  shas, gate results. Mid-sitting replies are one line in bench terms. No handholding, no reassurance.
- **You report your own track, never another seat's** (Peter 2026-08-25); the only thing that
  crosses tracks upward is a conflict or blocker he must rule on, stated as the decision needed.
- **Decisions lead,** as `DECISION NEEDED` with numbered one-line options, and only for destructive,
  first-of-kind, strategy-pivot or genuinely balanced forks; otherwise decide, do it, state it. Never
  ask a question you can answer; a decision being important does not make it his; a question he has
  answered before does not become new by changing subsystem.
- **Never offer to close, never wait for an expected answer, never offer to discard** (R20). A
  one-word reply answers the question on the table and is not a green light to spawn.
- **Files are clickable links, deliverables go to the sidebar** with `SendUserFile` (render) the
  moment they are created or meaningfully changed (Peter 2026-08-22, 2026-08-25).
- **Quote him, never paraphrase him:** his words go in RULINGS.md verbatim with the seat's reading in
  a separate marked clause; when a row and his sentence disagree, the sentence wins.

## 9. Standing facts (reference, not rules)

- **Paths:** memory `~/.claude/projects/-home-pmes-src-github-com-pmes-UnaOS/memory/` (track
  resumes, hazards, derail receipts; loaded by no session, read on purpose). The harness's
  auto-load dir for the orin, pi AND rmbp seats is `…/-home-pmes-src-github-com-pmes-UnaOS-hw-pi4/memory/`
  (the main checkout owns `.git`; no `-orin` or `-rmbp` memory dir exists) and it now holds only a
  pointer; Peter turned the app's memory keeping off on 2026-09-09, but whether a given session
  still auto-loads it is observable only from inside that session (pi 10 saw it loaded the same
  hour), so never claim it for another seat; plans
  `~/.claude/plans/unaos/`; bench `~/unaos-bench/` (tools/, flash/, scratch/, capture/). Claude runs
  in a toolbox; host tools via `flatpak-spawn --host bash -c '…'` with `PATH=$HOME/.cargo/bin:$PATH`.
- **Full push line:** `git push origin main hw-jetson hw-pi4 hw-rmbp net-sock1`. A local branch is
  unbacked iff its tip is neither an ancestor of `origin/main` nor contained in any origin ref.
- **Serial:** `/dev/ttyACM0`, re-enumerates on every move; an idle port drips NULs. Pi-share NAT
  leases 10.42.0.x.
- **Pi 4:** kernel runs at EL1; QEMU raspi4b has no V3D and no Group-1 interrupts, panel 640x480
  versus bench 1920x1200 (`UNAOS_FBW=1920 UNAOS_FBH=1200`); desktop build line of record 2026-08-18:
  `UNAOS_WITNESS=1 UNAOS_PIUSB=1 UNAOS_GENET=1 UNAOS_SMP7=1 UNAOS_NETTEST=1 UNAOS_V3D=1 UNAOS_VUGPAR=1
  UNAOS_WEDGE2=1 UNAOS_PIDESK=1 UNAOS_PIRAST=1 UNAOS_QUARRY=1 ./arroyo kernel8`; boot series `v3d boot
  N` and `dsktp boot N`, media `<series>-boot<N>-<git7>.img`; unafs v3 volume capped at 2 GiB;
  `[vugfps]` divisor is arch-conditional; click grammar: click = select + ack, SPACE = stop/start,
  focus never stops anything; `vug.rs` is deleted (pi 5 2026-08-28).
- **Orin:** fifteen-knob flight line is in each staged image's MANIFEST `# KNOBS:` line; the Pi
  floor is quoted per tree, never across trees; the slot card may hold a stale image and the firmware
  chooses the medium.
- **Handler manifest** (`docs/CODEX.md`): Aether=Web, Amber Bytes=Disks, Aulë=Forge, Comscan=Signals,
  Facet=Images, Geode=Archives, Helm=Control interlock, Holocron=Secrets, Matrix=Files, Mica=Data,
  Midden=Shell, Obsidian=Binary, Principia=System policy (every settings and preference decision
  routes there, never to Peter in-session), Junct=Colab, Stria=A/V, Tabula=Text, Vairë=Repos,
  Vein=AI, Vug=3D/CAD, Xenolith=VMs. "Palantír" is vetoed in every spelling.
- **Hardware quirks** live in the memory dir's `unaos-hazards.md`, one line each.
