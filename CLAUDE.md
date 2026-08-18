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
  Env knobs: `UNAOS_PI`, `UNAOS_BAREMETAL`, `UNAOS_SKIP_XHCI`, `UNAOS_BOOTLOG`,
  `UNAOS_SCHED_DEMO`, `UNAOS_USBDEBUG`.
- Inspect serial logs with `awk '/pattern/' <log>` — **not `grep`** (control
  bytes in the logs break it).
- Direction: [`docs/ROADMAP.md`](docs/ROADMAP.md). Security model + hardening
  ledger: [`docs/SECURITY.md`](docs/SECURITY.md). Subsystem docs: `docs/dev/OS/`.

## Worktrees & lanes

- The trunk is the integration branch (`UnaOS-gemini` today; `main` historically).
  Platform tracks: `hw-rmbp` (`../UnaOS-rmbp`, x86 2012 rMBP), `hw-pi4`
  (`../UnaOS-hw-pi4`, Pi 4 bare-metal), `hw-jetson` (`../UnaOS-orin`,
  Jetson Orin Nano).
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
    failure).
  - **Sync**: each track picks up trunk at its own arc boundaries by MERGING
    trunk into its branch (never rebase a pushed tip; never force-push).
  - **Doc/`arroyo` conflicts** are reconciled by the landing seat (union: keep
    both tracks' additions) instead of deferred to a seat.
- Track sessions still commit **only to their own track branch** mid-arc; trunk
  is touched only in the landing step above, after review + peer ack. Nobody
  pushes — Peter pushes; hand him the full push line at every landing.
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

## Arc discipline

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
