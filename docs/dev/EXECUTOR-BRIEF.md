# EXECUTOR-BRIEF — the mandatory head of every executor brief

Paste the block below VERBATIM at the top of every executor prompt, filling `<track>`, `<tip>`,
`<seat>`, `<ledger-id>`. It exists because five briefs in one session re-typed the same rules and
three executors still hit the worktree trap (LEDGER.md P1). A rule that lives in the seat's memory
is re-derived per executor; a rule that lives here is pasted.

```
MANDATORY HEAD (docs/dev/EXECUTOR-BRIEF.md):
1. FIRST COMMAND: `git log --oneline -1`. Agent worktrees seed at main's tip, not the track tip. If
   you are not on <tip>, `git reset --hard <tip>` on your private worktree branch before editing.
2. NEVER `git stash`. NEVER `git push`. NEVER write under /tmp — scratch is
   ~/unaos-bench/scratch/<seat>/<name>/ (build logs only; anything a row will cite goes in
   docs/dev/evidence/<arc>/).
3. Touch only the files the brief names. A fix that needs another file: STOP and report the exact
   change; do not make it.
4. Names in shared files are SUBSYSTEM-named, never board-named (`[pwrreboot]`, not `[orinreboot]`).
5. Knob-off byte-identity: new statements go inside an existing cfg region, or folded onto an
   existing line where the file carries a LINE-NEUTRAL rule (read the nearest "LINE-NEUTRAL append"
   comment and copy its shape). AN APPEND GOES BEFORE THE LINE'S FIRST `//` — after it, the statement is
   a comment, compiles nothing, and the check stays green (LEDGER P7). Prove the position, then
   MEASURE it: `./arroyo knoboff <feature>` builds the knob-off loadable image at your baseline and
   at your tree IN ONE DIRECTORY and compares them, with an armed control probe. Quote its EXIT
   STATUS: 0 = byte-identical, 1 = the knob-off image MOVED, 2 = no verdict. A 2 is never a pass.
6. GATE before commit: `cd unaos && ./arroyo check` exit 0 both arches; run the QEMU suite named by
   the brief. Quote exit codes and leg counts, never "green".
7. LEDGER: your commit ticks <ledger-id> in docs/dev/LEDGER.md or docs/dev/OS/<track>-ledger.md
   (status begins with open|fixed-unflown|flown|landed|dropped). The seat folds the COMMIT, never
   re-types your report.
8. COMMIT on your worktree branch: `<subsystem>: <NAME> — <imperative summary>`; body = mechanism
   with file:line evidence and the command that measured each claim; end with
   `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`. If the seat folds your diff by
   hand, the seat adds `Folded-by: <seat>` above the Co-Authored-By line.
9. REPORT (short): sha and its parent, `git show --stat`, gate exit codes, the knob line to build
   with, and the exact wire shape the next boot should show. Do not end your turn waiting on your
   own background check — poll it, then commit.
```
