# Local executor packs (pack 1 Aether; pack 2 Stria)

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

## Local pack 2 (Stria: the suite the local machine can carry)

Four Stria arcs, measure-first like pack 1, cut from the trunk tip (SR827-SR830). Same order of one arc as above; the
EXCEPTION target dir is `$HOME/unaos-local-target-media` (text_host wants alsa's `PKG_CONFIG_PATH` only if the system
lacks it). Row prefix for every pack-2 arc is `- NLE5b (SR204 owed)` (handlers.md L104, edited IN PLACE). The cloud seat
stays out of nle.rs `render`, nle_face.rs `paint`, timeline_core `lib.rs` / `view.rs` / `edl.rs` while pack 2 is open;
the export-message chain (nle_face.rs `job_line` / `export_eta`, stria-view `export_json`) stays the cloud seat's.

| arc | what | the seam to watch |
|---|---|---|
| `striawaveform` (SR827) | audio clip bodies paint their peaks (one reduction in timeline_core view.rs, scanned once per source) | view.rs `paint_in` gains ONE optional parameter: every caller edits one line |
| `striatitleclip` (SR828) | a title card as a generated Source (`* UNA TITLE` in the EDL; text_host renders the still once) | lib.rs `Source` gains ONE field: every literal gains `title: None` |
| `striaclipspeed` (SR829) | per-clip rate (`rate_q16`; `Clip::len` / `src_at` the one road; CMX `M2` line; `lay_edit_list` stops refusing) | every `src_out - src_in` reader in timeline_core becomes `len()`: count them in the design |
| `striaexportframe` (SR830) | `frame [<path>]`: the playhead frame as a PNG (preview road + the existing PNG encoder; a Frame button; `"frame"` on the witness) | stays out of `export_json`; `VERBS` 26 -> 27 |

Order: SR830 first (smallest, no model change), then SR827, then SR828 and SR829 (both touch lib.rs `Clip` / `Source`
literals: run them one after the other, not side by side, or the second one's merge re-touches every literal line).
