# QUEUE — the trunk queue: every job that does not need a specific board (R45)

One file, on `main`, kept current AS WORK HAPPENS, never at close. A job lives here if it can be
gated on QEMU or by compile alone; a job lives in a track queue (`docs/dev/OS/<track>-queue.md`)
only if it needs that board's metal or that board's own files. A job that has both halves is CUT:
the shared half is a row here, the metal half is a row there, each citing the other (R45: "isolate
down to the specific metal requirement as far as possible so as to maximize shared code").
Every row cites its ledger id — the ledger holds the finding, this file holds the ORDER. `✓` =
verified in this tree this session · `·` = inherited, not re-checked. Re-derive every sha.
Created 2026-09-12T00:3xZ (UTC; 2026-09-11 local) by orin 27 from the union of the four branches' ledgers (80 non-landed shared rows,
27 open orin rows, 93 open rmbp A/B rows, 8 open pi rows — counted by awk over the status column of each
branch's file at its tip that hour; the rmbp figure was first written as 88 from a narrower filter and
corrected by the landing review — the commands live in each track queue). rmbp 19's verdict stands: "88 open rows is a graveyard, not a queue" — so this
file lists JOBS, ranked, and a ledger row that is a record rather than a job stays in the ledger.

## STATE — 2026-09-12T03:5xZ (orin 27)
✓ main 0aa3afe6 = the jetson landing (parents d28e701e + 6ed2be1a; three review panels; landing legs on 5acf6d16: check tegra/plain strict rc=0, UNAOS_WC=1 test rc=0, test-arm rc=0; ledger-check rc=0 on the result; `UNAOS_LEDGER_STRICT=1 UNAOS_K8REACH_STRICT=1 ./arroyo check` on main 0aa3afe6 rc=0). Before it: rmbp 267bd49d and pi e10d9e4e landings.
✓ ALL THREE TRACKS LEVEL WITH MAIN after this commit (hw-jetson, hw-pi4, hw-rmbp fast-forwarded). R45's property holds: any track can merge to main at any time.
✓ Push line (Peter): `git push origin main hw-jetson hw-pi4 hw-rmbp exec-orin26-unafsroot exec-orin26-tear exec-orin27-ga10b4c exec-orin27-cmd8 exec-orin27-usblun exec-orin27-closefold exec-orin24-fold exec-orin25-shotverb`
✓ Orin bench: render13 (`render13-20260912T0259Z-f4b4bf3`, KELF max=0x34f820) written to the old 32 GB card; the UEFI does not list our MBR card in the native slot (GPT card image is the job, orin-queue) so it flies from the USB reader; waker armed on the loader line.

## 1. THE DESKTOP — glass defects Peter has seen, all in shared `video/` (fix once, every board)
· NEW 2026-09-12 (render13 on the Orin, Peter on the glass — shared code, every board): (a) launched programs are titled "Application" in the taskbar and menubar — `[winmenu] app-menu owner=N name=Application kind=default` for every QUARRY-LAUNCH'd ELF; only Shell is `from=declared`. R36: declared name, else the PROGRAM name; the launch path knows the ELF path and the app_name registry (wintitle) is not fed for EL0 launches. (b) R49: the console and the shell are APPS with pinned taskbar tiles — quit on close, relaunch from the tile through the same path any app takes; retire the console "route" (`[dock] console-reopen … route=declined -> DECLINE`) and the shell re-mint (`[realdesk] shell-remint … MINTED`). R50: Quarry is the Finder, always there, permanent tile (already in quarry.md since the day it was named). (c) after the shell re-mint the POINTER VANISHED — `[comp2] … sprite_us=1`, the sprite is no longer composed; goes with (b). (d) mousing/dragging not smooth: `[comp2] rollup pass_us≈2000 max_us=375000..588000` — half-second compositor passes; correlate the spike passes with `[wc-w]`, `[pstrip] gapmax`, `[wc-h] longpres`, the cross-core compositor gate (A46) before touching anything. Branches cut on hw-jetson at 2f6adcc4 for (a) exec-orin27-appname, (b)+(c) exec-orin27-conreopen, (d) exec-orin27-dragstall — briefs are the lines above plus the wire at ~/unaos-bench/scratch/orin27/render13-wire-partial.txt.
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
· USB mass storage holds ONE device: `drivers/xhci/mod.rs` keeps a single `storage_slot`, overwritten by whichever mass-storage device configures LAST (`Endpoints Configured … Storage ready`), and every render12 boot configured exactly one even with two readers attached (S1 class: hub enumeration). Two readers on the bench = a coin flip for which one is the disk. Fix shape: per-slot storage state and the registry array below; until then ONE reader per boot (Peter's bench, 2026-09-12)
· USB mass storage: the multi-LUN census and first-present selection are DONE (USBLUN, orin-ledger A56, on hw-jetson for the next landing). LEFT: the block registry holds ONE USB disk (`drivers/block.rs` `USB_BLOCK_DEVICE`/`BLOCK_DEVICE` are single slots; `fs/bootdisk.rs` walks them) so a reader with two cards publishes one — a registry design change: a small array with per-device handles, unpublish keyed on (slot, lun), the bootdisk walk iterating it
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
· NEW 2026-09-12: `make-pi-img.sh` (formats EVERY card image, Pi and Orin) — FATCLUST (`-s 1`, on hw-jetson 3225b5b0) fixed a FAT32 volume that was FAT16 by cluster count (orin-ledger A58), but its review OBJECTED: the warning guard greps dosfstools ≤4.1 wording (this host is 4.2: "Number of clusters for 32 bit FAT is less then suggested minimum") so it never fires; `2>&1 |` discards mkfs stderr; A58's numbers are wrong (256,030 clusters at 127 MiB, 110,874 at the Pi's 55 MiB); fs/fat.rs MISREADS such a volume as FAT16 (does not refuse). Fix shape: assert the cluster count off the written BPB (the C15 check), restore stderr, correct A58. Unflown on the Pi.
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
· `media-writer.sh --image`: no check that the image's FAT32 volume is FAT32 BY CLUSTER COUNT (≥ 65,525) — a 32,440-cluster "FAT32" passed C4/C5/C8 and the Orin UEFI refused to boot it (orin-ledger A58; the Linux vfat driver trusts the BPB, EDK2 and our kernel do not) — add C15 on the image and on `--src` targets
· `media-writer.sh --src`: no preflight that the staged tree FITS the target FAT, and a failed copy leaves the card mounted with a partial file (orin 27, 2026-09-12: a 640 MiB card image staged inside the tree filled the 127 MiB volume) — add C14 fits-the-volume and unmount-on-failure
· scorer STAMP-MATCH leg (wire `sha=` == card elf stamp); `media-writer.sh` is un-versioned (B97)
· SR5  R24/R25/R26 double-booked across seats and the gate cannot see it (seat-prefix RULINGS ids)
· Queues into `ledger-check.sh`: this file and the three track queues are gated by nothing yet (LAWS §3 Queues, warning only)

## 6. NEEDS PETER (not jobs until he rules)
· S27  138 prune-candidate origin refs from the 2026-07-25 triage, never OK'd (408 remote-tracking refs today)
· "one file is the rulebook, EVERYTHING ELSE DELETED": the deletion half never started (R20, destructive)
· D1 loader `SetMode` to the widest console mode (knob-gated, one power cycle)
· B7  vug arbiter placement (kernel-side recommended)
