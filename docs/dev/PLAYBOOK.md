# PLAYBOOK — one session, start to close, with the commands (written 2026-09-12 from the session that ran every step)

There is ONE session at a time (Peter 2026-09-12: "there will only be the one session"). No seats, no peers, no
acks over ccd, no batons, no numbered sessions. The focus is what Peter says at the start.

The rules are in [LAWS.md](LAWS.md). The bench ritual is in `~/.claude/plans/unaos/metal/BENCH-PROCESS.md`.
The executor head is in [EXECUTOR-BRIEF.md](EXECUTOR-BRIEF.md). This file is the ORDER: what a session does,
in sequence, and the exact command at each step. Read it top to bottom on every session. Paths: trunk
worktree `../UnaOS` (main), tracks `../UnaOS-orin` (hw-jetson), `../UnaOS-hw-pi4` (hw-pi4), `../UnaOS-rmbp`
(hw-rmbp). Scratch is `~/unaos-bench/scratch/<something>/`, never `/tmp`.

## 0. Open (every session, ~5 minutes)
1. Peter names the focus (a platform, or trunk). Nothing else names it.
2. Read `docs/dev/LAWS.md`, `docs/dev/QUEUE.md` (main), `docs/dev/OS/<platform>-queue.md` (that track).
3. State, re-derived, never from memory:
   `for b in main hw-pi4 hw-jetson hw-rmbp; do echo "$b $(git rev-parse --short $b) origin=$(git rev-parse --short origin/$b) main...b=$(git rev-list --left-right --count main...$b)"; done`
   and `git status --short` in every worktree. Pushed = `git merge-base --is-ancestor <sha> origin/<b>`.
4. Metal session: `~/unaos-bench/tools/bench-state.sh` (ports, holders, staged media); the butler must hold the
   port (`flatpak-spawn --host lsof -t /dev/ttyACM0`); `cat ~/unaos-bench/capture/line-acm0/marks.txt | tail -3`.
5. Write the STATE block of the platform queue before doing anything else; commit it.

## 1. Work a queue item with an executor
1. Cut a branch and worktree from the track tip:
   `git -C ../UnaOS-orin worktree add ~/unaos-bench/scratch/<name> -b exec-<platform>-<name> <track-tip>`
2. Brief = the EXECUTOR-BRIEF head verbatim (fill track, tip, who, ledger id) + intent, invariants, anchors
   (file:line), the DONE gate as a numbered command list with `cmd > log 2>&1; echo rc=$?`, the files it may
   touch, the ledger row it opens (next free `A<n>` in the arch ledger or `SO<n>` in LEDGER.md — grep both
   branches first), and "report by final message: sha, parent, `git show --stat`, rc list, wire shape".
   Opus. Fable only for the one key task of the round. Never `pkill -f`.
3. An executor that ends its turn "waiting on its gates" is resumed with: "poll them yourself, commit, report".
4. Its report is a claim: read its logs (`~/unaos-bench/scratch/<name>-logs/`), not its summary.

## 2. Fold an executor branch into the track
1. `git merge --no-ff --no-commit exec-<...>`; conflicts are almost always ledger rows. Resolve by UNION:
   keep every id once; when both sides carry the same id, keep the newer text (the executor's for its own
   row). Count markers = 0. Never resolve a `.rs` conflict blind: rebuild from both versions and gate.
2. `bash unaos/scripts/ledger-check.sh` rc=0 (an unpushed sha in a `fixed-unflown` row reds it: cite by name).
3. Commit the merge with a message that carries the executor's rc list. One fold gate on the WHOLE fold
   before any card (R38), not one per branch.

## 3. The fold gate (from `unaos/`, each into its own log; artifact build LAST — test-arm rebuilds it)
```
UNAOS_TEGRA=1 UNAOS_LEDGER_STRICT=1 UNAOS_K8REACH_STRICT=1 ./arroyo check   # tegra polarity
UNAOS_LEDGER_STRICT=1 UNAOS_K8REACH_STRICT=1 ./arroyo check                 # plain
UNAOS_WC=1 ./arroyo test                                                     # x86 compositor (shared video/)
./arroyo test-arm                                                            # virt
<knob line> ./arroyo esp-jetson                                              # the flight image, LAST
<knob line> ./arroyo esp-jetson-img                                          # the whole-card image
```
Then certify the artifact, never the diff: `LC_ALL=C grep -a -o -F '<witness>' target/aarch64_esp/kernel.elf | wc -l`
for every family the flight needs, and a control string that must be 0.

## 4. Stage and card (Orin)
1. Stage the tree (never a `target/` path to Peter):
   `UNAOS_SEAT="<who — the tool still calls it a seat>" ~/unaos-bench/tools/stage-orin.sh unaos/target/aarch64_esp <prefix> <sha40> <branch> "<KNOBS>" "<effective-features from the banner>" "<one-line header>"`
   → `~/unaos-bench/flash/orin/<prefix>-<UTC>-<sha7>/` + MANIFEST + `max_vaddr` (the boot's identity).
2. The whole-card image goes BESIDE the staged dir as `<name>.card.img` (NEVER inside it — `--src` copies the
   whole tree onto a 127 MiB FAT), with its sha appended to `~/unaos-bench/flash/orin/MANIFEST`;
   `python3 ~/unaos-bench/tools/validate-manifest.py ~/unaos-bench/flash/orin/MANIFEST --quiet`.
3. Before it is a card: parse the image's FAT boot sector and assert clusters ≥ 65,525 (A58). Dry-run both
   writer modes (`media-writer.sh --image <img> --target /dev/mmcblk0` and `--src <name> --target /dev/mmcblk0`);
   readability needs sudo, everything else must PASS.
4. Playbook: overwrite `~/unaos-bench/PLAYBOOK-orin.md` (watch list FIRST, then the card lines, then
   known/expected), `SendUserFile` it. Mark: append to `~/unaos-bench/capture/line-acm0/marks.txt`
   (`MARK <name> <sha7> boot1 staged-armed card=<name> elfmax=<max_vaddr> … raw=<bytes> orin=<bytes>`).
5. Waker: edit `~/unaos-bench/tools/waker.conf` (OWNER, `A_ANCHOR`/`B_ANCHOR` = current byte sizes of
   orin.log/raw.log, `A_PAT=` empty for growth first), start `sh ~/unaos-bench/tools/waker.sh` in the
   background. It exits on the first event; re-anchor and restart it after every fire. The idle port drips
   single NUL bytes, so a growth wake can be one byte.
6. Two lines to Peter, in order, and nothing else:
   `sudo ~/unaos-bench/tools/media-writer.sh --image <path>.card.img --target /dev/mmcblk0 --write --erase-target`
   `sudo ~/unaos-bench/tools/media-writer.sh --src <name> --target /dev/mmcblk0 --write`
   The second writes the FLIGHTID; score every boot by `KELF min=0x0 max=<max_vaddr>` and the boot volume
   serial FIRST. The Orin boots our MBR card from the native microSD slot only when the FAT32 is FAT32 by
   cluster count; from a USB reader it always did.

## 5. Read the wire
`LC_ALL=C tail -c +<mark byte+1> ~/unaos-bench/capture/line-acm0/raw.log | tr -d '\000' > <scratch>/wire.txt`,
then `awk 'index($0,"<token>")'` — never bare grep. Faults: awk for `PANIC|[Pp]anic|SError|EXCEPTION|RAS Uncorrectable|-> FAIL`
and READ the hits (register names contain EXC; instruments say "panic-fallback"). GPU rung verdicts:
`docs/dev/evidence/orin27/scorer-ga10b4.sh <log> 4a|4b|4c`. The capture goes into
`docs/dev/evidence/<round>/` with its KELF line in it (the ledger gate refuses an unanchored excerpt).

## 6. Ledger and queues, the turn it happens
Every finding: one row (arch ledger `A<n>` / shared `SO|SP|SR<n>`), 7 cells, no literal `|`, no scratch
paths, no unpushed shas. Every job or fact: the platform queue if it needs that metal, `docs/dev/QUEUE.md`
otherwise, and the OTHER platform's queue when it touches it. Commit as you go; nothing waits for close.

## 7. Land a track into main
1. Fold trunk into the track first: `git -C <track> merge --no-ff main` (conflicts by union; arroyo cfg
   lists by union of features — check no feature vanished from any leg).
2. Review panel: one Opus agent, READ-ONLY, scoped `main..<track>`, told NOT to run check/test if your gate
   already is; it reports ACK/OBJECT with findings per commit. Every OBJECT item is answered in code or
   ledgered before the merge; the merge message lists them. The panel IS the review — there is no peer.
3. `git merge-tree --write-tree main <track>` rc=0, then in `../UnaOS`: `git merge --no-ff <track> -F <msg>`;
   shape: `git log --pretty=%p -1` shows two parents; `bash unaos/scripts/ledger-check.sh` rc=0;
   `UNAOS_LEDGER_STRICT=1 UNAOS_K8REACH_STRICT=1 ./arroyo check` rc=0 (and `test`/`test-arm` when code moved).
4. Level the others: `git -C <other track> merge --ff-only main`. Record in QUEUE.md STATE.

## 8. Close
The queues say everything (state, what flew, what is open, what may be wrong, the push line). No file
outside git. Never push: Peter pushes. Say the push line once.
