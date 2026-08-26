# CLAUDE.md — ground rules for Claude sessions in UnaOS

These rules apply in this repo and in every worktree of it. They exist so that
several sessions can work in parallel without stepping on each other.

## Layout & builds

- Monorepo, two layers: **Ring 0** kernel under `unaos/` (x86_64 + aarch64);
  **Ring 3** host-native userspace at the root (`libs/`, `handlers/`, `vessels/`, `tools/`).
- Kernel builds/runs via `unaos/arroyo`: `check` (type-check both arches),
  `test` / `test-arm` (headless QEMU, serial → `target/serial*.log`),
  `x86` / `arm` (QEMU GUI), `esp-x86` / `esp-arm` (metal boot media),
  `kernel8` / `kernel8-run` / `kernel8-test` (Pi 4 bare-metal image / QEMU raspi4b).
  Env knobs: `UNAOS_WC` (**arms the x86 window compositor — any gate touching
  the video stack MUST carry `UNAOS_WC=1`, and the run MUST show `wc` in the
  `⚡ kernel features:` banner. It gates `video/desktop_uefi.rs` — the x86 panel path —
  and the console-window routing, NOT the whole video stack: `video/mod.rs`
  declares `pub mod wm;` unconditionally, so a *type* gate on `wm.rs` is not
  vacuous without it. What IS vacuous without it is any gate that claims to
  exercise the compositor, because `desktop_uefi::activate()` has exactly one caller,
  `drivers/gpu/kepler_display.rs` — on x86 the compositor's ignition is the
  Kepler takeover, so a behavioural video gate needs `UNAOS_WC` AND the
  kepler knobs, and must be verified reachable (`strings`), not merely
  compiled (banner)**), `UNAOS_PI`, `UNAOS_BAREMETAL`, `UNAOS_SKIP_XHCI`, `UNAOS_BOOTLOG`,
  `UNAOS_SCHED_DEMO`, `UNAOS_USBDEBUG`, `UNAOS_FBW`/`UNAOS_FBH` (panel-geometry
  override — QEMU raspi4b is 640x480 while the bench Pi is 1920x1200, and the
  window compositor's upscale is a function of the panel, so
  `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test` is what reaches the
  bench's blit path; default unset = query the firmware).
- Inspect serial logs with `awk '/pattern/' <log>` — **not `grep`** (control
  bytes in the logs break it).
- **Dependencies: latest stable, always** (Peter, 2026-07-21). This is a brand-new
  OS — when a newer stable version of a crate exists, update to it; there is no
  legacy to protect. Never pin to a pre-release or downgrade below current stable.
- Direction: [`docs/ROADMAP.md`](docs/ROADMAP.md). Security model + hardening
  ledger: [`docs/SECURITY.md`](docs/SECURITY.md). Subsystem docs: `docs/dev/OS/`.
- Standing operational laws (verification, bench/media, code & history):
  [`docs/dev/LAWS.md`](docs/dev/LAWS.md) — binding in every session.

## Worktrees & lanes

- There is exactly ONE trunk branch; every rule below says "trunk" and means it
  name-agnostically. The trunk is **`main`** (Peter's ruling 2026-08-18: the
  `UnaOS-gemini` staging name is retired; until the fast-forward push of `main`
  to gemini's tip lands on origin, verify which ref is current with
  `git ls-remote origin main UnaOS-gemini` rather than trusting this line).
  Platform tracks: `hw-rmbp` (`../UnaOS-rmbp`, x86 2012 rMBP), `hw-pi4`
  (`../UnaOS-hw-pi4`, Pi 4 bare-metal), `hw-jetson` (`../UnaOS-orin`,
  Jetson Orin Nano). The shared trunk worktree is `../UnaOS` — landing merges
  and trunk batteries run there; don't create a duplicate.
- **Integrator-less coordination (Peter, 2026-08-18): there is no integrator
  seat.** The three track sessions coordinate their own integration over ccd
  session messages. Duties that belonged to the seat are reassigned, not
  dropped:
  - **Landing an arc to trunk**: the landing track runs the independent
    adversarial review itself (agent panel — the COI guard: the author seat
    never reviews alone), **announces the merge over ccd and obtains a peer
    ack from at least one other track seat** (the second pair of eyes the seat
    used to be), then merges its own reviewed arc to trunk with `--no-ff` and
    runs the trunk battery. Every merge announce, ack, and repeat of an ask
    carries a **fresh `git ls-remote` check run that same turn, both seats** —
    reachability claims are never relayed stale (the 2026-08-03 mirror
    failure). **Dispute path**: no ack, or an objection the seats cannot
    resolve over ccd → the merge does not happen and the disagreement goes to
    Peter with both positions. Silence is never consent; a 1-1 split never
    deadlocks unrecorded. **Landing race**: immediately before the `--no-ff`
    merge, announce over ccd and run a fresh `ls-remote`; if trunk moved since
    your review, merge the new trunk into your arc and re-run the trunk
    battery before landing — first announced merge wins, the other rebases its
    landing on the result.
  - **Sync**: each track picks up trunk at its own arc boundaries by MERGING
    trunk into its branch (never rebase a pushed tip; never force-push).
  - **Doc/`arroyo` conflicts** are reconciled by the landing seat (union: keep
    both tracks' additions) instead of deferred to a seat.
- Track sessions still commit **only to their own track branch** mid-arc; trunk
  is touched only in the landing step above, after review + peer ack.
- **The seat never runs `git push`. Peter does.** No inference overrides this.
- **Name every push Peter will need in your FIRST turn, batched** — including
  pushes for commits you have not written yet (if your arc will end on a
  branch, name that branch's push at the start). Discovering them one at a
  time costs him a full round-trip each, and the information to batch them
  is always available at minute one. Before announcing any sha to a peer,
  verify it with `git ls-remote --heads origin` **and**
  `git log --oneline -1 <sha>` — a sha the peer cannot fetch is not a
  deliverable. And re-run `git fetch` before ever reporting a push as still
  outstanding.
- **Tracks run independently, at their own pace.** No track waits for another.
  Standing cap unchanged: **one unmerged arc per track** — a fresh session per
  arc; don't stack a second arc on an unreviewed one within the same track.
- While parallel arcs are in flight: the rmbp session owns shared kernel-core
  files; the pi and jetson sessions touch only the files their brief names
  (pi: its `arch/aarch64` arc files; jetson: GIC/timer + `tegra`-feature
  files). If your arc needs a file outside your lane: **negotiate it over ccd
  with the owning seat before touching it** (and record the grant in both
  sessions); no agreement → stop and report to Peter. (Lanes are why
  independent merges stay conflict-free: x86 vs aarch64 rarely collide.)

- **Never `git stash` in this repo or any worktree of it.** The stash stack is ONE stack
  shared across all worktrees; with parallel sessions live, a stash is a race by
  construction (three cross-session pops in GR24 alone). To take a clean baseline:
  snapshot your diff to `~/unaos-bench/scratch/<arc>/`, verify the patch re-applies,
  then `git apply -R` — or `git worktree add` a throwaway tree at the baseline sha.

## Arc discipline

- **Work the jobs (2026-08-19): a brief/baton's named arcs spawn in the
  session's FIRST turn — the assignment is the go. At every turn end, running
  executors below the floor (3, up to 6 for Pi/Orin) while undone work exists
  = spawn before replying. "Standing by"/"awaiting your go" are banned; an
  empty floor must be proven that turn or be Peter's explicit hold. Full law:
  `docs/dev/LAWS.md` §Throughput.**
- One arc per session, defined by your brief
  (`~/.claude/plans/unaos-opus-<track>.md`). Do not exceed it, however
  tempting the adjacent improvement.
- **QEMU-green ≠ correct.** Hardware verification happens at arc boundaries
  and is not your job; your job ends at the DONE gate.
- DONE gate = the brief's exact expected outputs **+** `./arroyo check` green
  for both arches **+** the track's QEMU regression suite **+** the doc update
  your brief names.
- **STOP tripwires** — stop, record exactly what you observed, and report
  instead of improvising, whenever:
  - QEMU or hardware behavior diverges from the brief's stated expectations;
  - a fix would require touching a file outside your lane;
  - a workaround would disable or weaken a protection (SMEP, NXE, WXN,
    page permissions, checksums);
  - you are about to reach for a force-push, a history rewrite, or a merge
    outside the two sanctioned kinds (trunk→track sync; reviewed+peer-acked
    arc→trunk landing).

## Committing & handoff

- Commit on your track branch when the DONE gate passes. Message style follows
  `git log`: `subsystem: imperative summary`. End with your model's
  `Co-Authored-By` line.
- Before ending a session: update your track's resume/handoff notes and leave
  a short landing report (what landed, gate results, anything flagged).

## Docs & tone

- Update the doc named in your brief as part of DONE — docs stay current by
  construction.
- Technical docs are written in a professional voice; the lore/canon voice
  belongs only in `docs/CODEX.md` and `MEMORIA.md`.
