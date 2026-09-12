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

## STATE — 2026-09-12T01:0xZ
✓ origin/hw-rmbp 73d9d361; local hw-rmbp = this commit, clean. `git rev-list --left-right --count main...hw-rmbp` with main at 50d35cbf: main +3 (the 084b79ac merge, R45-R48, QUEUE.md) / rmbp +10 (docs: SR13, P14, the landing report, this file and its correction). Lands first.
✓ Open A/B rows in rmbp-ledger at 73d9d361 = 93, by `git show HEAD:docs/dev/OS/rmbp-ledger.md | awk -F'|' '/^\|/ && NF>=8 {id=$2;s=$6;gsub(/^[ *]+|[ *]+$/,"",id);gsub(/^[ *]+|[ *]+$/,"",s); if (tolower(s) ~ /^open/ && id ~ /^[AB][0-9]/) c++} END {print c+0}'` (the landing review's command; the seat's first figure, 88, came from a narrower filter stated without its command — SR12's class, corrected here).
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
