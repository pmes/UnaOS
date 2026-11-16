# Local executor pack 1 (Aether, trunk-ready)

The cloud seat cuts arcs, measures designs (R131), folds branches into `claude/optimistic-ramanujan-r3qyu5`
and pushes on green. A local session can run executors on the same protocol; the division is BY TRACK so the
shared-file seams stay inside one machine:

- **Cloud seat:** Stria / NLE, h264enc_core, hevc_core, and the Aether chains already in flight (font-synthesis,
  font cache, fallback, the alt icon).
- **Local:** the Aether arcs in this directory (layout_core / aether handler / text_host), one at a time per
  executor, three at once at most.

## The order of one local arc

1. Pull the tip: `git fetch origin claude/optimistic-ramanujan-r3qyu5 && git checkout claude/optimistic-ramanujan-r3qyu5 && git pull --ff-only`.
2. Worktree + branch from the tip: `git worktree add ../exec/<name> -b exec-rmbp-<name> origin/claude/optimistic-ramanujan-r3qyu5`.
3. Launch an Opus executor with the launch text below (paste it inline, with `<name>` filled in). It reads
   `RULES.md` (rules 1-17), its arc file here, `docs/dev/EXECUTOR-BRIEF.md`, `docs/dev/LAWS.md`.
4. THE FRONT GATE: the executor writes its design and sends it to YOU (the local session is its seat). You answer
   `MEASURE: PASS — <why>` or `MEASURE: FAIL — <why>`; it builds only after PASS, exactly the design.
5. It commits on its branch (ledger row + the MEASURE line in the body, committer Claude <noreply@anthropic.com>)
   after merging the trunk tip; it never pushes.
6. YOU run its fold-test on a fresh local target dir (the arc's EXCEPTION command), then push the branch:
   `git push -u origin exec-rmbp-<name>`, and tell the cloud seat: branch, tip sha, the row prefix
   (`- AETHERMAC-RUN (SR205 o` for every arc here), and the test command + counts. The seat fetches, queues, folds, pushes trunk.
7. Gates: keep running `./arroyo check`, `./arroyo gates`, `./arroyo test`, `./arroyo test-arm`, `./arroyo kernel8-test` on each
   NEW trunk tip the seat announces (pin each run to a named sha so a mid-run push does not confuse the reading). No metal.

Known pre-existing reds (not counted): text_host `resolution_and_fallback_vs_fontconfig`; the 17 HEVC conformance vectors;
play-check `audio_oracle` on sfx.ogg.

## Launch text (fill `<name>`, `<clone>`)

You are executor <name> on the UnaOS local seat (local pack 1, CODE-ONLY mode). Work ONLY in the worktree <clone>/../exec/<name>
(branch exec-rmbp-<name>); never touch the main checkout or any sibling worktree. FIRST COMMAND: `cd <clone>/../exec/<name> && git log --oneline -1`.
READ, in this order: docs/dev/arcs/local/RULES.md (rules 1-17; `$W` paths in it are the cloud seat's: your scratch is <clone>/../exec/scratch/<name>/),
your arc file docs/dev/arcs/local/<name>.md (its anchors were read at the cut; re-measure each against the code), docs/dev/EXECUTOR-BRIEF.md,
docs/dev/LAWS.md, the blueprint sheets the arc names under docs/dev/blueprint/, and the code the arc anchors. Model: you are Opus per LAWS §4.
THE FRONT GATE (rule 17): before the first edit, write <scratch>/design.md — the CODEX §2 handler and seam, the measurement you made of the code
(fn names and line numbers; where the arc's anchors moved), the existing mechanism you extend by fn name, what you will NOT duplicate, the data
that moves and who owns its lock, the FILES and the one-line shape of each change, the wire the fold should see, and the owed proposals you will
write as §5 rows but not build. Then send the design to the seat (the session that launched you) and WAIT for `MEASURE: PASS` (or FAIL with the
reason — redesign and resend). No code before PASS. Build EXACTLY design.md; a departure is a STOP-and-report, never an improvisation.
CODE ONLY except the arc EXCEPTION line (the named host cargo command on the named local target dir, nothing else). Re-read every edited hunk for
syntax before the commit. Merge `origin/claude/optimistic-ramanujan-r3qyu5` before the commit. Commit on your branch with the ledger row and
`MEASURE: PASS — <the seat's why>` in the body, committer Claude <noreply@anthropic.com>, NEVER push, NEVER `git stash`, NEVER `git reset --hard`,
NEVER `pkill -f`. Report: sha + parent, `git show --stat`, "code-only, uncompiled", what landed by M1-M3, the wire the fold should see, shared
vocabulary touched, the blueprint line, the MEASURE line, owed proposals.
