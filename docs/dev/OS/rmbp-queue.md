# RMBP — WHAT IS IN FLIGHT AND WHAT IS OWED (the x86 rMBP track queue, R45)

One file, fixed path, not per-round. Kept current AS WORK HAPPENS, never at close. Only jobs that need
the rMBP's metal or x86-only files live here (`arch/x86_64`, Kepler/gmux/EHCI/FTDI, the x86 specs,
`stage-x86.sh`); everything else is a row in the trunk queue `docs/dev/QUEUE.md` on `main`. Ledger:
`docs/dev/OS/rmbp-ledger.md` (A/B/E) and `docs/dev/LEDGER.md` (SR<n>). `✓` = verified in this tree ·
`·` = inherited, not re-checked. Re-derive every sha before acting.
Created 2026-09-12T00:35Z (2026-09-11 local; dates here are UTC) by orin 27 (all lanes, R46). rmbp 19's own measurement, quoted by Peter the same day:
"88 open rows is a graveyard, not a queue" and "16 items … exist only in a plans directory outside the
repo" — this file is the queue those two sentences asked for; the 16 baton items were not read (R37),
their three named subjects are rows here or in the trunk queue.

## STATE — 2026-09-15T18:3xZ (rmbp focus session)
✓ hw-rmbp 160176d2 = main bd17f887 + hw-jetson fd440602 + hw-pi4 e6e71c9f, all folded here on Peter's order ("sync with both leaving main"); code is hw-jetson's exactly (`git diff --stat hw-jetson hw-rmbp` = QUEUE.md + pi-queue.md only). One conflict, QUEUE.md §5, by union; ledger-check rc=0. UNPUSHED: 00d32fc3, 5c4aad2d, 160176d2 (+ the executor branches below at close). main untouched.
✓ Bench: rMBP on the bench, NOT booted, FTDI on /dev/ttyUSB0 held by line-butler pid 10585 → `~/unaos-bench/capture/line-usb0/raw.log` (first bytes of the next boot land there). A new-to-us 29 GB SD in the reader (`/dev/mmcblk0p1`, vfat, NO label; Peter: contents disposable) — it becomes the UNAOS-X86 card at the card step. No flight image built yet (Peter: no compile before development).
✓ Executors cut from 160176d2 this session (branches `exec-rmbp-*`, worktrees `~/unaos-bench/scratch/rmbp-0915/<name>`): FTDIRX (A9, the x86 half of S29), XHCIKBD (B45 + B44/A41, the key task), TESTTRUNC (trunk §5 truncated-run row + B95 + S8), BEAMX86 (A5). Fold gate, esp-x86 LAST, stage, card, playbook, mark, waker — after the fold.
· x86 legs (R39): `test`, `test-fat`, the ELF-off-FAT legs, the x86 usb-write witness — rmbp's.

## METAL — needs the rMBP on the bench
· A9  serial console is TX-only (FTDI bulk IN 0x81 never driven) — no typing over the wire (the x86 half of S29)
· A1  BAR1 wedge under paint bursts (`storm`): a core dies mid-blit
· A5  shell window tears under storm (`torn=111`) — the x86 twin of the Orin BEAM fix: Kepler's VERT/rgpos register as the beam source
· A6  `[clickroute] … -> FAIL` deterministic on metal, green in QEMU
· A3 / A4  reboot verb's witnesses never reach the wire; unattended reboots need the card as default startup volume (⌥ picker)
· A7  gmux switch to the iGPU does not persist — ⚠ RESTATED by the register: it does not switch AT ALL (GMUX-1 below)
· A8  Bluetooth inquiry deafness is boot-scoped
· B8  Broadcom 0x14e4 NIC has no driver (e1000 is QEMU-only)
✓ B10  shut-out register for the rMBP GPU ladders (R19) — COMPILED: `docs/dev/OS/08_VIDEO/SHUTOUT-REGISTER.md`; the jobs it found are the GPU LADDERS section below
· B89 / B91  the "boot dumb" falsifier the tree fails; the installer guards HOME, not the stranger (Catalina)
· FLIGHT 8+ on the current tree — the x86 track's last metal flight predates its own landing

## GPU LADDERS — the B10 shut-out register's job list (R19)
One row per rung whose failure conditions a next flight (or, for the first three, no flight at all) can change.
Every row cites `docs/dev/OS/08_VIDEO/SHUTOUT-REGISTER.md` by section; read the row there before acting.
· NEW  GMUX-1  read the flight-5 capture's `:: igpu-dpy: pre-switch state DDC=… SW_DISP=… SW_EXT=… DISP=… EXT=…` line (`igpu.rs:1214`, printed BEFORE the refusal at `:1245`). It names which register put the gmux ladder at `highest=00/10`, and it is already on disk — **no boot, no build**. Blocks A7, G5, G7, G9. Register §5
· NEW  SHUTRESTORE  restore the seven refuted rungs whose code R19 says must be KEPT and is gone from the tree (`USERD_SNOOP`, `PFIFO_FLUSH`/`flush-executed`, `CTRL_ADDR`, the 0x6101E0 `repoint`, the EVO latch arm+UPDATE — `update_reg` at `kepler_display.rs:280` is declared and unused — `lin-step`/`bwpg`, `gop-overlap`), each behind its own knob. Also give `run_recon` (`kepler_display.rs:288`) and `do_takeover` (`:330`) knobs instead of literals. Register §7
· NEW  GEN7DOC  correct `gen7.md` §2's table, §2.6, §2.7 and §4 — they record R6/R7 as "pending metal"/"held dark" and both PASSED on flight 4 (`r6-sentinel-hit`, `r7-blit-verified … best_dst_match=256/256`). One pointer line landed with the register; the body is still wrong. Register §4
· NEW  CEFLY  first CE-LADDER flight: `UNAOS_KEPLER_CE=1` decides R1/R2/R2b/R3/R5 in one boot (`CE-LADDER end r1_ptop=…`). Confirm `nvidia-kepler-ce` in the `⚡ kernel features:` banner AND by `LC_ALL=C grep -a -o -F` on the artifact (s42's INSTGUI lesson). Falsification story pre-written. Register §3
· NEW  BAR1UC  fly `UNAOS_BAR1EXP=uc` (`:: x86 bar1exp: UC arm ARMED`, `arch/x86_64/memory.rs:3729`) on a boot scored **wedge/no-wedge**, not throughput — the M1-vs-{M2,M3} discriminator for ledger A1. The ~6.8x UC slowdown that disqualified it from flight 5's power numbers is irrelevant to this question. The only coded, never-flown power experiment. Register §6
· NEW  GEN7R2  re-score gen7 R2 (`gt-still-dark`): it failed on an instrument flight 4 proved blind — `battery_moved=0/17` through a **verified** 1 KiB engine DMA — and on a poll that passed at iteration zero on a gated window. Re-run with a behavioural witness (ring arm / sentinel), not the 17-register battery. Register §4
· NEW  GEN7R8  R7's own `next=`: wire `bring_up_blt_ring` to the held wake and fix `blitter_copy_rect`'s DW0 client field. The BCS copies pixels on metal; this is engineering, not physics. Must NOT gate on a forcewake ack decode (`fw_evidence=blind` while the BCS executed). Register §4
· NEW  GEN7TLB  decide the R6/R7 reclaim: pin an alternative GGTT-invalidation witness, or promote the bounded 7-page / 28 672 B per-armed-boot leak to documented-accepted and delete `reclaim=freed` from the falsifier — it is structurally unreachable as coded (`tlb-flush-write-silent`). Register §4
· NEW  KDHEAD  re-run the per-head EVO/CRTC decode (KD3) with KD4's `head[0] stat underflow=` as the control bracket and the per-head stride re-derived for the 917D class. It failed with no control read, and KD4 proves head 0 scans. Register §1
· NEW  KFBIND  re-run the CHAN_CUR/CHAN_NEXT bind (KF18) and the post-bind strip (KF19) **with FECS context microcode resident and running** — the one condition never varied across ten eliminations. Carry KF6/KF8/KF9 (once SHUTRESTORE lands) as first steps of the same boot: each was refuted only under a host that had never run FECS ucode. Register §2
· NEW  KFUNWEDGE  the un-wedge experiment, still UNEXERCISED after ten sittings: a boot where the `0x409504` poison **deliberately** fires, then a PRING observe/clear and a `cpuctl` re-read in the same boot. KF21 proved the write is harmless; it is the read that poisons. Register §2

## X86-ONLY FILES — QEMU q35 gates them
· B1  LOCKFIX gap at `arch/x86_64/syscall.rs` `click_pointer_pos` (`WRITER.lock()` in an input band)
· B3  five x86 features uncovered by any board leg (`nvidia-kepler-kdisp-hold rtpi rtwit selfhost vugras`)
· B4  two `unsafe` warnings in the bootloader under `unaos_ivb`
· B45  the x86 gate does not exercise the xHCI keyboard path at all (decides where the N-TD fix is proven)
· B85  six FORBIDs keyed across a token junction in the x86 specs
· B95  x86 spec files run by no `arroyo` verb (trunk-queue row 5; the x86 verb wiring is here)

## THE GRAVEYARD SWEEP (an rmbp job, one sitting)
· 60-odd B-rows (B18-B88) are GRANT and REVIEW records of orin arcs that have since landed or been folded; each is closed-by-landing or re-homed as a job, never deleted. The sweep is briefed with this file and reports only what is still a job.

## SHARED, ROUTED TO THE TRUNK QUEUE (listed so the cut is visible)
· SR13 strict trigger, R39 battery selector, S6 board names in shared files (50), SR1/SR9/SR10/SR11/SR12 gate classes, SR2 print-screen wedge, orin-ledger A41 / B44 keyboard report loss — all trunk-queue rows
