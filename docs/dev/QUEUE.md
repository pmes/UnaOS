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
· NEW 2026-09-12T15:1xZ (render13 boot 1 wire, orin-ledger A55/A54 context; shared `video/wm.rs` and `power.rs`): (e) `[wc-d] verify win=1 -> SKIP (no memory for WxH source snapshot)` (wm.rs:7877) fires once per compositor pass while a window whose snapshot cannot be allocated exists — 46,161 of 67,185 lines on one boot, and the serial staging ring dropped 5,331 lines in 192 `[serial] dropped` events under it. A per-pass witness must latch (print once, then count into a rollup). (f) the shutdown path (`power.rs` `platform_shutdown` / `crystal_shutdown` → `psci_call`) prints through the same staging ring and drains nothing before the SMC: on a flooded ring the last lines before SYSTEM_OFF are lost with the power. Drain the ring (bounded wait) before the SMC on every power verb. (g) the banner `crystal LIVE … Sleep/Restart/Shut Down print their honest unimplemented lines — no PSCI wiring on this track yet` (`video/desktop_firmware.rs:356`, `main.rs:8547`) is stale: `video/crystal.rs:693` calls `power::crystal_shutdown` → PSCI SYSTEM_OFF, and boot 2 of render13 proved the path (orin-queue STATE).
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
· FIXED ON hw-jetson, NOT YET LANDED 2026-09-12T2x:xxZ: both SERDRAIN findings are FIXED on `exec-orin-pwrdrain2` (S5DRAIN 30e736d6, WITNESS-GATING 3b784ed1), folded to hw-jetson daf38b01 as SO39 and SO40 — the x86 S5 flush went to the PORT (`poweroff()`'s own first statement) so no caller can skip it, and the three serial-ring fixtures are behind `witness`, returning 14,143 B ≈ 1.228 s of UART per boot. The two `· NEW` rows that stood here are REMOVED rather than left to duplicate at the land; hw-jetson carries them as ledger rows. **NEW AND OPEN — SO41, found while proving the second one:** `mirror_service` is NEVER CALLED on the Jetson (its two aarch64 call sites are `main.rs:4425`, `baremetal`-gated, and `main.rs:1779`, the shared BSP loop the Orin's `sched::run_bsp(0)` handoff at `main.rs:1494` diverges before). So `[mirror] N line(s) dropped` and `:: SERWIT-2 ::` cannot appear on a Jetson capture and a tap losing lines there is silent by construction. The fix is one call on the tegra/`bsprun` path. **Do not land it casually:** the Orin flight line carries `UNAOS_WITNESS=1`, so that call also arms the three ring fixtures on the card and spends the 1.228 s back — land it with that decided, and consider gating the fixtures on something narrower than `witness`. Owner: the next serial arc.
· FIXED ON hw-jetson, NOT YET LANDED 2026-09-12T2x:xxZ: both `knoboff` findings this session (the NET6 one — the default baseline `HEAD~1` is the FIRST parent, so a merge tree was compared against its own pre-merge tip; and the GA10B4E/SO34 one — one shared scratch directory, so concurrent runs corrupted each other's verdict) are FIXED on `exec-orin-knoboff` 1c8397fa, folded to hw-jetson a2f9f4ee: a bare merge HEAD now exits 2 and prints both parents with their re-run commands (refusal, not a guessed parent — the right parent depends on merge direction and LAWS §3 sanctions both), and every caller gets its own keyed scratch with an flock and a kept run directory on failure. The two `· NEW` rows that stood here are REMOVED rather than left to duplicate: hw-jetson carries them as `✓ DONE` and they arrive here when it lands.
✓ DONE 2026-09-12T16:xxZ (orin session, on hw-jetson): `make-pi-img.sh` FATCLUST review objections answered — the guard is now `unaos/scripts/fat-clusters.sh`, which parses the WRITTEN BPB and asserts FAT32 by cluster count (go-red proven on the refused render13 image: `clusters=32440 kind=FAT16 -> FAIL`; green on card-v2 `256030`; NOT-A-BPB control rc=2), mkfs stderr is no longer captured, the numbers are corrected (256,030 at 127 MiB; 110,874 at 55 MiB), and A58 says fs/fat.rs MISREADS (FAT16 by count, no refusal). Lands with hw-jetson.
· NEW 2026-09-12T18:xxZ: `ledger-check.sh` passed rc=0 on a LEDGER.md that carried three git conflict markers (`<<<<<<< HEAD`, `=======`, `>>>>>>> exec-…`) — a fold committed them (hw-jetson 4465eb20, fixed 7006857f). Add a refusal: any ledger, queue or RULINGS file containing a line that begins with `<<<<<<< `, `=======` alone, or `>>>>>>> ` reds the gate; go-red by mutation.
· NEW 2026-09-12 (DRAGSTALL, executor finding, measured): the x86 witness ladder's "no fixture between them may lose an event" rule extends to SERIAL PRINTS — a line-neutral fixture appended at the head of `dmgovlp_selftest` cost that fixture its drag leg (`drag_evt=0 … -> FAIL`; baseline `drag_evt=5 -> PASS`); moved to `physwit_once`'s site it is green. Write it in FIXTURE_FLAKES.md or LAWS §5. Also: `./arroyo test`'s 20 s default does not reach the x86 witness ladder on a loaded host — a brief that gates on a ladder fixture needs `./arroyo test 90`.
· NEW 2026-09-13 (orin session, seat finding; Peter asked "i'm curious why sntp doesn't 'just work'" and then "that sounds like a hack"): **SNTP IS DRIVEN FROM A NIC DRIVER, AND THERE ARE THREE CLIENTS.** The wire format was factored out correctly — `net_sntp.rs` is shared and arch-neutral, `clock.rs` is shared, and FAT mtimes already derive from the clock (CLOCK-3 `f98fbfa4`) — but the DRIVING of it was stapled to a driver: the ONLY caller of `smolnet::witness_tick_sntp` is a statement inside `drivers/e1000.rs:1184 service_net`, guarded `all(feature = "smolnet", target_arch = "x86_64")`, beside its DNS and v6 twins. The Pi therefore did not inherit a client, it got a SECOND copy on genet (`941db4b2` PI-NET-16, later adopting the shared parser at `69e4627e`). The Jetson has NONE — `lib.rs:46-48` says in as many words that aarch64 "has no consumer yet" — which is why the menu bar's clock (`video/menubar.rs:49-50`, `CLOCK_GLYPHS`, and the honesty rule at :131-137 that draws NO clock while `clock::try_unix_now()` is None) has never drawn a single time on that board. Normal SNTP is a userspace daemon on a socket; an in-kernel client is ordinary for this stage, but the per-driver coupling is not, and it has now produced exactly the failure it was always going to produce. **A fourth client is being written** (orin `exec-orin-orintime`, arch-neutral on the NET6 socket surface, explicitly NOT hung off rtl8168's tick, its driving seam named and justified in its commit). **THE ARC THIS ROW IS FOR:** ONE client, three drivers under it. What blocks it today is that `pub mod net6` is gated `#[cfg(all(feature = "net6", target_arch = "aarch64"))]` (`net_phy.rs:797`) and only `rtl8168_tegra.rs:5733` and `virtio_net.rs:643` call `register_nic`, while x86 talks to smolnet directly; nothing in net6 looks intrinsically arch-specific (smoltcp behind a `NicOps` trait), so bringing e1000 onto it and retiring the other two clients is plausible but touches x86 boot and is its own arc. ORINTIME is reporting the file:line cost of that move WITHOUT attempting it. Peter rules on whether it is worth doing. Owner: unassigned.
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
✓ DONE 2026-09-12T16:xxZ: `media-writer.sh` C15-FAT32-BY-COUNT on the `--image` p1 and on the `--src` target volume (probe_medium prints VOL_CLUSTERS/VOL_FATKIND; selftest fixture `spc8.img` = the A58 shape, red arms in both modes). Bench tool, un-versioned (B97 stands): backup `media-writer.sh.bak-c15-<UTC>` beside it.
· `media-writer.sh --src`: no preflight that the staged tree FITS the target FAT, and a failed copy leaves the card mounted with a partial file (orin 27, 2026-09-12: a 640 MiB card image staged inside the tree filled the 127 MiB volume) — add C14 fits-the-volume and unmount-on-failure
· scorer STAMP-MATCH leg (wire `sha=` == card elf stamp); `media-writer.sh` is un-versioned (B97)
· SR5  R24/R25/R26 double-booked across seats and the gate cannot see it (seat-prefix RULINGS ids)
· Queues into `ledger-check.sh`: this file and the three track queues are gated by nothing yet (LAWS §3 Queues, warning only)

## 6. NEEDS PETER (not jobs until he rules)
· S27  138 prune-candidate origin refs from the 2026-07-25 triage, never OK'd (408 remote-tracking refs today)
· "one file is the rulebook, EVERYTHING ELSE DELETED": the deletion half never started (R20, destructive)
· D1 loader `SetMode` to the widest console mode (knob-gated, one power cycle)
· B7  vug arbiter placement (kernel-side recommended)
