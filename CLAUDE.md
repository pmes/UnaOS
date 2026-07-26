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

- `main` is the integration trunk. Platform tracks: `hw-rmbp`
  (`../UnaOS-rmbp`, x86 2012 rMBP), `hw-pi4` (`../UnaOS-pi4`, Pi 4 bare-metal),
  `hw-jetson` (`../UnaOS-jetson`, Jetson Orin Nano).
- Track sessions commit **only to their own track branch**. Never merge or
  push to `main` — the integrator session does that after review.
- **Tracks run independently, at their own pace.** No track waits for another.
  When a track's arc lands and passes review, the integrator merges *that* arc
  to `main` and rebases *that* track for its next arc; the other tracks keep
  running on their current base and rebase at their own next landing. The only
  standing cap: **one unmerged arc per track** — a fresh session per arc; don't
  stack a second arc on an unreviewed one within the same track.
- While parallel arcs are in flight: the rmbp session owns shared kernel-core
  files; the pi and jetson sessions touch only the files their brief names
  (pi: its `arch/aarch64` arc files; jetson: GIC/timer + `tegra`-feature
  files). If your arc needs a file outside your lane: **stop and report** —
  the integrator updates the briefs. (Lanes are why independent merges stay
  conflict-free: x86 vs aarch64 rarely collide; docs/`arroyo` the integrator
  reconciles at merge.)

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
  - you are about to reach for a force-push, history rewrite, or merge.

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
