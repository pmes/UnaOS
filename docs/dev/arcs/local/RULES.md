# WAVE 36 HEAD (re-written 2026-10-26 after the container reset; the rules are the same ones every wave-36 executor built under)
1. FIRST COMMAND: `git log --oneline -1` in your worktree; it must show the cut sha your arc file names.
2. NEVER `git stash`. NEVER `git push`. NEVER write under /tmp — scratch is /root/unaos-bench/scratch/cloud/wave36/<name>/.
3. Touch only the files the brief names. A fix that needs another file: STOP and report the exact line, or ask the seat (SendMessage `main`) for the grant.
4. Names in shared files are SUBSYSTEM-named, never board-named.
5. Knob-off byte-identity: new statements go inside an existing cfg region, or folded onto an existing line BEFORE its first `//` (LINE-NEUTRAL: the call on the existing line, the body in its own fn at the file tail).
6. CODE ONLY (Peter 2026-10-26): no cargo check, no gates, no QEMU on the executor side — the seat compiles the fold and the bench runs the gates. Re-read your hunks for syntax before the commit.
7. LEDGER: your commit appends ONE row at the END of the table in docs/dev/OS/rmbp-ledger.md (B-ids) or docs/dev/LEDGER.md (SR-ids): 7 cells, no `|` inside a cell, status `fixed-unflown`, no claim words.
8. COMMIT on your worktree branch: `<subsystem>: <NAME> — <imperative summary>`; body = the DESIGN + the ledger row + the MEASURE line.
9. REPORT (SubagentHandback): sha + parent, `git show --stat`, the compile command and rc with the log path and the Checking line, the gates line, what landed by M1-M3, the wire the fold should see, shared vocabulary touched, the blueprint line (`blueprint: <sheets> kept=<ids> changed=<ids>`), the MEASURE line, owed proposals.
10. NEVER kill by pattern (`pkill -f`, `killall`, `pgrep | xargs kill`).
11. CHARTER BEFORE CODE: name the docs/CODEX.md §2 handler and the seam that own the problem; a new module carries a one-line CHARTER doc.
12. THE PRE-COMMIT LEG IS THE PLAYBOOK'S (docs/dev/PLAYBOOK.md).
13. MODEL: executors run Opus (LAWS §4).
14. THE BLUEPRINT IS THE FRONT GATE: read the sheets the arc names under docs/dev/blueprint/ first; every change names the BP row it keeps or changes.
15. THE KERNEL'S TARGET SPEC is `unaos/x86_64-unaos.json` (soft-float, the real build's).
16. MEASURE TWICE, CUT ONCE (R131): before the first edit write the DESIGN — the handler and seam, the existing mechanism you extend (by fn name), what you will NOT duplicate, the data that moves and who owns its lock, the FILES and the one-line shape of each change. A copy of a mechanism that exists is refused at the fold; so is a module that reaches across a seam the sheet draws.
17. THE GATE IS AT THE FRONT DOOR: write design.md to /root/unaos-bench/scratch/cloud/wave36/<name>/design.md, SendMessage the seat (`main`) with its content, WAIT for `MEASURE: PASS` (or FAIL — redesign and resend). No code before PASS. Build exactly design.md; a departure is a STOP-and-report. Put `MEASURE: PASS — <the seat's why>` in the commit body; fold.sh refuses a commit without it.
18. (retired with rule 6: executors do not compile.)

## Seat notes, batch 2 (2026-10-27, fifteen folds at 2879aed46)
- FOLD ONE BRANCH AT A TIME. A loop over branches runs the next `git merge` while the first is unresolved (git refuses it, but the sheet merger then runs against the wrong branch). `foldone <n>` then resolve then `foldcommit <n>`, never two in one command.
- sheetmerge now DROPS a row the branch carries unchanged from the merge-base that HEAD no longer has (a §5 row retired at the seat); before that patch every later fold re-added the superseded rows and video.md crossed the 120-line cap twice.
- A row BOTH sides changed: `m3/rowmerge.py <file> <branch> "<row prefix>"` (word-level 3-way; clean when the edits are in different places) else `m3/rowappend.py` (HEAD's row + the branch's inserted chunks appended to the same cell; use it when both appended sentences at the same spot).
- Two tail blocks appended to the SAME impl/module (PORTRESET + HELDMAP in `pub mod suspend`): the union needs each HEAD block's impl closed before the next block's comment — `tailseam.py` does it for a file-tail seam; inside a module do it by hand and check with the lexed depth.
- Executor slips the compile catches every batch: `crate::serial_println!` (macro_export macros are not absolute-path addressable: write `serial_println!`), a struct literal missing a field another arc added, `#[cfg]` on an expression statement (wrap in a block), `crate::arch::sched::sleep_ms` in a desktop-family fn (x86-only: use the `arch::ms()` yield loop), `gen` as an identifier (edition 2024 keyword).
- The seat's four legs take ~25 min cold after a container restart (target-seat survives on disk; the restart only kills processes). Run them as one background command with a 3000 s timeout and never two cargo invocations on the same target dir at once.
- Push the tip as soon as the four legs are green; the bench's per-push checks start from the push, not from the seat's gates.
- DISK (the per-session allowance, not df's "Avail"): a host-test executor's cargo target is 3-6 GB. Host arcs use a SHARED target dir per crate family (`$W/host-target-aether`, `$W/host-target-unafs`), never their own; the seat deletes a finished arc's target at the fold. 14 executors + the seat's four legs fit only that way.

## Seat notes, batch 3-4 (2026-10-27)
- A tail that git splits into TWO conflict hunks (a shared `        }\n    }\n}` in the middle): do not tailseam each; rebuild the file as `git show HEAD:<f>` + "\n" + the branch's tail from its first new block comment to EOF (python; check `{`-`}` = 0 on head, tail and out).
- `git checkout --conflict=merge <sheet>` regenerates markers labelled `ours`/`theirs`: sed them to `HEAD`/`<branch>` before sheetmerge; then `rowmerge.py` the one BOTH-CHANGED key it prints.
- Two arcs cut from one tip can both add the same new BP id (BP-handlers-13 twice): renumber the LATER fold's row to the next id, and every reference to it (LEDGER row, the crate docs; `grep -rn`).
- blueprint-check REDS a §5 row for the claim word `proven` (say `exercised by`) and for a backticked path that does not exist in the tree (an external tool: unquote it, e.g. "libsbc's own esbc tool").
- A `markers 1` from sheetmerge on LEDGER.md is the P8 prose line that quotes `'^<<<<<<<'`; `grep -c '^<<<<<<<'` is the real count.
- fold.sh refuses a branch whose NEWEST commit lacks the MEASURE line: ask the executor for ONE follow-up commit carrying it (a real doc line), never an amend.
- The user's interrupt of the seat's turn KILLS every running executor: relaunch each as a RESUME prompt (worktree may be clean; design.md survives; put the seat's verdict in the prompt so a measured arc builds at once).
- Host checks of user-aether from the root need `-Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem` with the absolute `unaos/x86_64-unknown-none.json` / `aarch64-base.json`; `$W/hosttests.sh` has the eight host runs.
- A fold whose .rs conflicts span several files of a HOST crate (NLE4 vs NLE3: schedule.rs x3, lib.rs, edl.rs, nle.rs, tests): abort the merge and hand the seam to the arc's own executor: `git merge <seat tip>` in its worktree, resolve keeping both arcs' tests green, commit the MERGE commit with the MEASURE line; then foldone is clean. The seat keeps hand-merging only kernel tail seams and sheets.
- `rowmerge.py` rc=4 (token conflict) silently keeps HEAD's row — CHECK the row carries both arcs' ids afterwards; `m3/cellappend.py <sheet> <branch> "<row prefix>"` appends every span the branch inserted (vs merge-base) into the same cell of HEAD's row and is the right tool when two arcs each extend one BP clause.
- The 120-line sheet cap counts `wc -l` + 1 (the checker counts the final line): keep `wc -l` at 119 or under. When two or more host arcs each add §5 rows to handlers.md, retire before the fold: a row the BP clause already carries as BUILT, a pointer row ("is ONE owed row, X above"), or merge a one-sentence owed item into the sibling row that already names its subject (HEVCDTS into NLEPROBE-HI; the NLEPROBE swap into DEMUXSEEK-WIRE).
- Writing an arc file with a bash heredoc of double-quoted args EATS backticked `--flags` (command substitution): write arc files through python or a quoted heredoc ('EOF'), and grep the file for the flags afterwards.
- sheetmerge keys rows on their first 40 chars: an executor that REWRITES its owed row in place with a different opening (e.g. "- NLE5b (SR204 owed): BUILT by SR206") lands as a SECOND row beside HEAD's old one — drop the old one at the fold (python by content match, never by line number: `nl` skips blank lines).
- SEAT ERROR (AUTOLOCK3 fold 64b35e9ec): rebuilding a two-hunk tests.rs conflict as `git show HEAD:<f>` + the branch's tail DROPS the branch's IN-PLACE hunks elsewhere in the file (AUTOLOCK3's four folds at :2026-2073 were lost; AUTOLOCK4 restores them). The right rebuild is: take the MERGED file (markers and all), resolve ONLY the tail hunks as HEAD-block + branch-block, and leave every clean in-place hunk git already applied. Verify with `git diff <base> <branch> -- <f> | grep '^+' ` against `git diff <base> HEAD -- <f>` for the branch's non-tail lines.

## Seat notes, batch 5 (2026-10-27)
- SHARED HOST TARGET DIRS ARE POISON with executors on sibling worktrees: cargo served stale siblings' crates (Sinc / lay_edit_list / rate missing) even after touch. The seat compiles host suites in `host-target-seat` (private); every executor gets a FRESH private dir named after its arc and deletes it after the commit. host-target-media / host-target-aether were deleted (14 GB).
- sheetmerge keys rows on 40 chars: a row rewritten "BUILT by SRnnn" past char 27 lands as a NEW row and the old one stays -> delete the old row by content (python, startswith) at every fold; check `wc -l` = 119 after.
- When two branches rewrite the SAME §5 row (HEVCPAL / HEVCHEIC / HEVCINTER on HEVCPARSE), compose ONE row by hand from the pieces (python slicing on the clause names), never cellappend alone.
- Multi-file .rs conflicts (NLE2C vs NLE6's editor.rs move): abort, have the executor merge the seat tip into its branch on a private target dir, fold the merge commit.
- Pre-existing red at fd79422f2: `cargo test -p audio_core --test lossy_oracle` (chromium-sfx.ogg 15435 vs 15307 frames) -> VORBISTRIM SR307 cut.
- Cloud ledger ids now run from SR300 (local takes SR230+ / B900+); kernel arcs keep B896+.
- Disk: 12 worktrees ~ 4.5 GB; keep >= 5 GB free before starting the legs.

## Seat notes, batch 6-8 (2026-10-27 05:30-08:10Z)
- compose.py (<file> <base> <branch>) replays a branch's per-line diffs into the working sheet; it is SAFE only for pure inserts — a `delete applied` on a row sheetmerge already took from the branch corrupts it. Rule: after sheetmerge, check `cur[i]==branch[i]`; if HEAD's row == base's row, take the branch's row wholesale; only then compose by CLAUSE (clause() at paren depth 0, keyed on the item's opening words incl. the id, e.g. 'PDF-WRITE (SR325)'), never by character diff.
- clause() stops at the first depth-0 ';' — an item whose BUILT text carries a ';'-separated owed tail (PDF-WRITE SR325) LOSES the tail: take the span from the item's start to the NEXT item's opening words instead (PDF-JPX / CCITT, PIXEL-UNFILTER ...).
- `find()` returning -1 silently composes an EMPTY clause (the pdfsign BP-handlers-4 slip at eecd358de, restored at e7640ab07): assert the key is found in BOTH rows before replacing.
- A both-changed BP row (BP-handlers-4 gains a crate per arc) is best patched from the branch's phrase between two stable anchors ('the standard security handler, and, ' ... '), Holocron').
- Executor-reported order/timing flakes are real bugs: the aether IDB ordering KAT (red on the seat, green in the executor's loaded run) was the HTML §8.6 timer nesting clamp + a process-global test root (IDBKATORDER SR346).
- Kernel-linked host crates that gain a dep (audio_core→crypto_core, pixel_core/av1_core→yuv_core, kernel→yuv_core, vp8_core→yuv_core next) need: unaos/Cargo.lock + every unaos/crates/user-*/Cargo.lock that pins them (user-stria, user-aether, user-una) — `cargo metadata --locked` in each dir proves it; then the four legs.
- Three kernel arcs on vplay.rs in one batch (SWAPBACKVPLAY / NLE6MIXFACE / NLE6EXPORT) were cut with named-hunk seams; expect tail-section unions (HEAD block + "}\n\n" + branch block) and composed `ok` / `:: NLE:` lines in player.rs nle_fx (colour= env= xfades= tokens appended in fold order).
- Cloud ids used through SR358 / B959; next SR359 / B960.
