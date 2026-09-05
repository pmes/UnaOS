# rmbp 11 — LANDING REPORT (2026-09-03, bench; orin held the focus; support pace, no fleet)

## Landed on hw-rmbp (all on origin, verified by fresh host ls-remote + --is-ancestor at close)
- 857c6dc8 x86/power: BOOTFADT — FADT reset facts printed once at boot on the normal console path
- 21b792b0 docs: flight 6 metal results (screenshot.md §9; PCIE-RP-RECOVERY.md stale claim fixed)
- bc10a469 power: PWRNAME — [orinreboot]/[orinshutoff] → [pwrreboot]/[pwrshutoff]; Tegra wdt → [orinwdt]
  (orin 13's grant: wdt_tegra.rs, jetson-sync1.spec, arroyo/lib.rs/Cargo.toml comments; acceptance grep = 0)
origin/hw-rmbp = bc10a469. Arc = 35 ahead of main, 1 behind, UNLANDED (orin lands first, agreed).

## Gate results
Every commit: ./arroyo check exit 0 both arches (GATE-FAMILY 8 families, GATE-KNOB 0 phantom/0 dead,
GATE-ROOTS 9 targets OK); UNAOS_WC=1 ./arroyo test 150 exit 0 with wc in the banner; ./arroyo test-arm 60
exit 0 (PWRNAME run; BOOTFADT is x86-gated and was covered by the same run).

## Metal (2012 rMBP, bench, FTDI on /dev/ttyUSB1)
- Flight 6 (f751cb78, 3 boots): B CONFIRMED (chords decode; SCREEN3/4.PNG verified host-side); A reset
  happened but the ladder is unobservable on this console (16550 assumption); C holds; D void. 70 s per
  capture with the USB pump idle. BAR1 wedge on c1 under storm; REHOME + shell re-mint held.
- Flight 7 (bc10a469, 1 boot): `[pwrreboot] FADT RESET_REG discovered at boot: space=SystemIO addr=0xcf9
  value=0x6`. Apple RESET_VALUE = 0x6. Zero stale tokens on the wire.
Postmortems: FLIGHT6-POSTMORTEM.md, FLIGHT7-POSTMORTEM.md (this dir). MANIFEST lines 637, 638.

## Flagged
- PETER'S RULING: stop poisoning arch-neutral code with board names. Measured: the x86 kernel carries 17
  `[piusb…]` strings and printed `[piusb40]`/`[piusb41]` on the rMBP's wire. Rule + GATE-NEUTRAL design in
  baton rmbp-12 (P0). Told orin 13.
- Unattended reboots into UnaOS are impossible until the card is the default startup volume (⌥ picker).
- The reboot ladder on the rMBP is blind by construction; a synchronous FTDI drain needs a design (P2).
- rmbp-10 executor worktrees: the four `exec-rmbp10-*/wt` trees removed (clean, reports harvested);
  branches `exec-rmbp10-*` KEPT (their content was folded by re-commit, so they are not ancestors of
  hw-rmbp — delete only on Peter's word).

## Peers
orin 13: PANELREFUSE §9.2 x86 evidence = unanswerable on this wire (told); PWRNAME sha sent; close sent.
pi 6: nothing owed.
