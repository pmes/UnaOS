# CLAUDE.md — UnaOS

**The rules live in one file: [`docs/dev/LAWS.md`](docs/dev/LAWS.md). Read it before acting.
Binding in this repo and every worktree of it.** Peter's verbatim rulings are in
[`docs/dev/RULINGS.md`](docs/dev/RULINGS.md), cited by R-id. Nothing in this file is a rule.

## Layout

- Monorepo, two layers: **Ring 0** kernel under `unaos/` (x86_64 + aarch64); **Ring 3**
  host-native userspace at the root (`libs/`, `handlers/`, `vessels/`, `tools/`).
- Direction: [`docs/ROADMAP.md`](docs/ROADMAP.md). Security model and hardening ledger:
  [`docs/SECURITY.md`](docs/SECURITY.md). Subsystem docs: `docs/dev/OS/`. Ledgers:
  `docs/dev/LEDGER.md` and `docs/dev/OS/<track>-ledger.md`. Evidence: `docs/dev/evidence/<round>/`.
- Tracks and worktrees: `hw-rmbp` (`../UnaOS-rmbp`), `hw-pi4` (`../UnaOS-hw-pi4`), `hw-jetson`
  (`../UnaOS-orin`); trunk `main` in `../UnaOS`.

## Builds

`unaos/arroyo` verbs: `check` (type-check both arches) · `state` (branch@HEAD, trunk distance,
owed, dirty) · `test` / `test-arm` (headless QEMU, serial to `target/serial*.log`) · `x86` / `arm`
(QEMU GUI) · `esp-x86` / `esp-arm` / `esp-jetson` (metal boot media) · `kernel8` / `kernel8-run` /
`kernel8-test` (Pi 4 image / QEMU raspi4b).

Env knobs: `UNAOS_WC` (arms the x86 window compositor; any gate touching the video stack carries
`UNAOS_WC=1` and the run shows `wc` in the `⚡ kernel features:` banner; on x86 the compositor's
ignition is the Kepler takeover, so a behavioural video gate needs the kepler knobs too and is
verified reachable with `LC_ALL=C grep -a -o -F` on the artifact, not merely compiled), `UNAOS_PI`,
`UNAOS_BAREMETAL`, `UNAOS_SKIP_XHCI`, `UNAOS_BOOTLOG`, `UNAOS_SCHED_DEMO`, `UNAOS_USBDEBUG`,
`UNAOS_FBW`/`UNAOS_FBH` (panel geometry; QEMU raspi4b is 640x480, the bench Pi is 1920x1200).

Serial logs are read with `awk` (bracketed tags via `awk 'index($0,"[tag]")'`), never bare `grep`.
