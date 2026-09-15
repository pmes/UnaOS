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
✓ COUNT COMMAND for the rmbp-ledger A/B population (this block carried none before the GRAVEYARD sweep; LAWS §5 wants the unfiltered `| wc -l` first):
  `awk -F'|' 'NR>=17 && NR<142 && /^\| [AB][0-9]+ \|/' docs/dev/OS/rmbp-ledger.md | wc -l` = 115 rows (the whole population, sections A and B, B24's two-rows-on-one-line counted once)
  then by class: `awk -F'|' 'NR>=17 && NR<142 && /^\| [AB][0-9]+ \|/{s=$6; sub(/^ */,"",s); sub(/ —.*/,"",s); sub(/,.*/,"",s); sub(/ *$/,"",s); print s}' docs/dev/OS/rmbp-ledger.md | sort | uniq -c`
  GRAVEYARD sweep 2026-09-15: open 93 -> 46, landed 1 -> 42, dropped 0 -> 6, fixed-unflown 20 and flown 1 unchanged. Per-row table and proof commands: `docs/dev/evidence/rmbp-0915/GRAVEYARD-SWEEP.md`
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
· A7  gmux switch to the iGPU does not persist
· A8  Bluetooth inquiry deafness is boot-scoped
· B8  Broadcom 0x14e4 NIC has no driver (e1000 is QEMU-only)
· B10  shut-out register for the rMBP GPU ladders (R19)
· B89 / B91  the "boot dumb" falsifier the tree fails; the installer guards HOME, not the stranger (Catalina)
· FLIGHT 8+ on the current tree — the x86 track's last metal flight predates its own landing
· B51 / B56  XHCINTD + DUPGUARD are REVIEWED and ACCEPTED on `origin/exec-orin17-dupguard` (`0019ec7a`, `28899d5c`, `e390721f` — all fetchable since Peter's push) and FOLD NOWHERE until the rMBP flies the completion path: `KBD_INFLIGHT`, `kbd_retire` and `kbd_guard_verdict` are 0 hits in this tree. B45 established the rMBP is the only place in the fleet that can score it. Shared half: trunk queue §2 A41/B44
· B90  the FRIEND-INVARIANCE positive control, and it is one unplug plus a diff: boot the same image twice, once with a friend UnaOS disk present and once without, and diff the wire modulo the mount witnesses (normalization PRE-REGISTERED: counters, timestamps, addresses and mount witnesses only; no line drops, order kept). The Orin flew the falsifier and it was NOT FALSIFIED — but vector 1 declined to trigger, so the leg that matters is FRIEND PRESENT **and** ROOT REFUSING WRITES, the only configuration in which the capture ladder's rung 2 can stain. B109's witness field (`Shot.vol`, `source`, `serial`) is landed, so the result is now readable

## X86-ONLY FILES — QEMU q35 gates them
· B1  LOCKFIX gap at `arch/x86_64/syscall.rs` `click_pointer_pos` (`WRITER.lock()` in an input band)
· B3  five x86 features uncovered by any board leg (`nvidia-kepler-kdisp-hold rtpi rtwit selfhost vugras`)
· B4  two `unsafe` warnings in the bootloader under `unaos_ivb`
· B45  the x86 gate does not exercise the xHCI keyboard path at all (decides where the N-TD fix is proven)
· B85  six FORBIDs keyed across a token junction in the x86 specs
· B95  x86 spec files run by no `arroyo` verb (trunk-queue row 5; the x86 verb wiring is here)
· B9  `[ptrdead] … fpop3=1 -> FAIL` in `UNAOS_WC=1 ./arroyo test-fat sf 200` — the foreign-drain flake in `arch/x86_64/syscall.rs`, 2 reds in 5 WC runs, both at load >= 24 (load-correlated, not random); `0d509431` / `badc8732` did not close it. Costs a re-run per proof on an unrelated leg
· B83  the CITATION half: rmbp-ledger B58 and B60 cite `dc683c40` / `1aae3459`, which resolve on local `exec-*` branches only, while their CONTENT is on `main` as `18af05ab` / `c8153b4e`. A re-cite against the landed shas, not a rescue. (The metal-flight half closed when Peter pushed `exec-orin17-dupguard`.) Lane rule adopted: every cross-lane cherry-pick uses `-x`, so a dangling citation resolves by `git log --grep=<old sha>`

## THE GRAVEYARD SWEEP (an rmbp job, one sitting)
· 60-odd B-rows (B18-B88) are GRANT and REVIEW records of orin arcs that have since landed or been folded; each is closed-by-landing or re-homed as a job, never deleted. The sweep is briefed with this file and reports only what is still a job.

## SHARED, ROUTED TO THE TRUNK QUEUE (listed so the cut is visible)
· SR13 strict trigger, R39 battery selector, S6 board names in shared files (50), SR1/SR9/SR10/SR11/SR12 gate classes, SR2 print-screen wedge, orin-ledger A41 / B44 keyboard report loss — all trunk-queue rows
