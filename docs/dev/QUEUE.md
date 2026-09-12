# QUEUE — the trunk queue: every job that does not need a specific board (R45)

One file, on `main`, kept current AS WORK HAPPENS, never at close. A job lives here if it can be
gated on QEMU or by compile alone; a job lives in a track queue (`docs/dev/OS/<track>-queue.md`)
only if it needs that board's metal or that board's own files. A job that has both halves is CUT:
the shared half is a row here, the metal half is a row there, each citing the other (R45: "isolate
down to the specific metal requirement as far as possible so as to maximize shared code").
Every row cites its ledger id — the ledger holds the finding, this file holds the ORDER. `✓` =
verified in this tree this session · `·` = inherited, not re-checked. Re-derive every sha.
Created 2026-09-12 by orin 27 from the union of the four branches' ledgers (80 open shared rows,
27 open orin rows, 88 open rmbp rows, 8 open pi rows — counted by awk over each branch's file at
its tip that hour). rmbp 19's verdict stands: "88 open rows is a graveyard, not a queue" — so this
file lists JOBS, ranked, and a ledger row that is a record rather than a job stays in the ledger.

## STATE — 2026-09-12 (orin 27)
✓ main 95888db4 (R45-R48 + LAWS) · hw-jetson f2794266 (rung 4 flown) · hw-pi4 153c78dd · hw-rmbp 73d9d361
✓ Distances to main: rmbp +8/−1 (docs), pi +46/−0, jetson +67/−0 (+1 local). Landing order: rmbp, pi, jetson, then fold main into all three.
✓ In flight: exec-orin26-tear (BEAM, Opus finishing), exec-orin27-ga10b4c (rung 4c, Fable), exec-orin26-unafsroot 5984c69c (gated, folding).

## 1. THE DESKTOP — glass defects Peter has seen, all in shared `video/` (fix once, every board)
· SO3  no application menu, no Quit anywhere (menubar press map + winmenu app tree)
· SO2 / SO4  window-menu drop-down misplaced + wrong typeface; crystal drop-down 12 px inset
· SO5  pointer sprite changes size over the desktop backdrop
· SO1 / SO10 / SO13 / SO17  console route lost on close; shell re-open resolves to the console; no `pin_console`; re-minted console comes back empty
· SO9 / SO14  a shell Enter opens a file when Quarry is open-unfocused; boot-opened Quarry holds the keyboard
· SO11  chrome drawn at a different width from its surface
· SO12 / S15  boot cascade overlaps the pulse title bar; pulse overlaps the console on both aarch64 boards
· SR2 (=A36)  Print Screen wedges the machine for the whole capture, every board — the write is inside the device-service pump
· SO21  dock tiles and windows "all crazy mixed up" — four pins, one count, five readers (exec-orin23-dockid WIP e86d3485, ungated)
· S32  four furniture `rollup` functions have no callers
· x86 `[wm] close-scope win=0` on the close-box path; `screenshot` verb prints OK with no device (shell.rs, two-line fix via Shot::report_ok; exec-orin25-shotverb is a non-compiling preserved edit)

## 2. INPUT AND USB — shared xHCI/EHCI
· A41 / B44  keyboard report loss: SET_IDLE 0 + one outstanding interrupt-IN TRB (both arches; the N-TD fix is the real one)
· S1 / S2  hubs fail status-change endpoint configure; hub-attached pointers print vid:pid 0000:0000
· S30  pointer path has no press-recovery accounting
· S29  no board owns a serial RECEIVE path (Orin: SPE/TCU mailbox; x86: FTDI bulk IN never driven — the x86 half is rmbp's)

## 3. FILESYSTEM AND NAMESPACE
· SO20  `SYS_OPEN` has no directory namespace (every EL0 file is pinned to the volume root) — the shared EL0 ABI
· SO18  no volume but FAT can remove a directory (`remove_dir` on `VfsBackend`, then UnaFS)
· SO19  `screenshot` still writes FAT-direct, bypassing the mount table
· SR3  quarry's cache-invalidation stamp advances only for USB arrivals
· `read_block_at` `(lba*512) as u32` truncation above 4 GiB (flagged by sdv1, unledgered — ledger it first)

## 4. ONE OS — platform-split families and board names in shared code (R16, R4, S6)
· S7  `render_service` ×3 (`render_service` Pi / `x86_render_service` / `orin_render_service`) — convergence arc
· SR6 / S4  `SHELL_REOPEN` drained three ways, two names break R16
· `release`/`pi_release`, `run_bsp`/`run_bsp_tegra`, `shell_remint`/`tegra_shell_remint` (R16 AND named after a function it does not call), `input_service`/`x86_input_service`, `on_block`/`pi_on_block`, `rast_demo_maybe` ×2 — the GATE-FAMILY table (`unaos/scripts/arch-families.sh`), one family per arc
· S6  the 50 board-named symbols/tokens in shared files (rmbp 19's count; scour per R4), and the 28 board-named shared scripts
· S3 / S5 / S21  `flight_recorder` x86-only; unconditional `pub mod` with single-arch consumers (FC-2 shape, fourth instance)
· SR8  `BLIT_NET_CORE` is `[AtomicU64; 8]` at module scope with no cfg

## 5. GATES AND TOOLING — shared `arroyo`, scripts, specs
· R39 battery selector: `battery()` runs the landing seat's OWN-BOARD legs — the selector is not written
· SR13  GATE-LEDGER's strict trigger cannot fire in a detached worktree (the shape every executor gates from)
· SR1  a `UNAOS_*` knob with no `K8_FEATS` arm is unreachable for every Pi image, silently — the CLASS needs a gate
· SR9  GATE-FAMILY groups by NAME and cannot tell a per-platform COPY from a CALLER
· SR10  the battery preserves the log it prints and destroys the log it judges
· SR11 / SR12  ledger-check: notation that collides with its subject; deferrals nobody checks are keepable
· S8  `scan_serial_faults` passes on a MISSING log; `test`/`test-arm` are negative-only
· SO6  `arroyo check` from the repo root reds the knob→builder probe (a red with no defect)
· SO7 / B26  `kernel8-test` (MBENCH) reds intermittently under host load — quiet-box obligation
· SR7  three QEMU verbs have no completion signal (QEMU-FAST cannot shorten them)
· B95  the x86 spec files are run by no `arroyo` verb; `orin-specscore.py` and `mbench --self-test` likewise
· S16  106 ordering invariants, 99 in comments enforced by nothing — check them in code, one file per arc
· S13  `[u7stk]` probe has no reachable caller outside `u7_launcher`
· scorer STAMP-MATCH leg (wire `sha=` == card elf stamp); `media-writer.sh` is un-versioned (B97)
· SR5  R24/R25/R26 double-booked across seats and the gate cannot see it (seat-prefix RULINGS ids)
· Queues into `ledger-check.sh`: this file and the three track queues are gated by nothing yet (LAWS §3 Queues, warning only)

## 6. NEEDS PETER (not jobs until he rules)
· S27  138 prune-candidate origin refs from the 2026-07-25 triage, never OK'd (408 remote-tracking refs today)
· "one file is the rulebook, EVERYTHING ELSE DELETED": the deletion half never started (R20, destructive)
· D1 loader `SetMode` to the widest console mode (knob-gated, one power cycle)
· B7  vug arbiter placement (kernel-side recommended)
