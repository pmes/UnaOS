# orin 15 — CLOSE REPORT (2026-09-06, the render6 flight and its consequences)

Not a trunk landing: this arc's code stays on `hw-jetson` for orin 16 to fold, fly (render7) and land.

## Flown
render6 (`render6-20260906T0049Z-f2eae02`, kernel `bec22fd0`, ELF max `0x2db188`) booted twice; both boots pinned by
anchor and PURE. Scores: `FLIGHT-RESULT-render6.md`. Headlines: A15 pass 4 · A18 pass 3 · A19 FLOWN (the
pre-cascade band is gone, `A19-pngband.py` 0/60200) · A22 FLOWN row 1 (the SPE forwards console RX into the HSP
mailbox and parks it: word `0x82006574` = `t`,`e`, the two bytes UARTC loses) · A20 clicks land on boot 2, the
pointer was DEAD on boot 1 (intermittent, same image) · A10 confirmed (Esc does not dismiss) · A27 NEW (drag
grabs, never moves) · A17 gap (no `InFlight` refusal for presses during a capture) · A8 (quarry never opens:
the only opener is inside `activate()`, which the cascade skips).

## Ruled
R21 (Peter): menus belong in the menu bar, never inside a window (`RULINGS.md`; row A25). Focus is inherited
(UNAOS-LAWS §ROLES amendment; `~/unaos-bench/tools/orin-open.sh`).

## Seen in the screenshots (`render6-SCREEN0-small.png`, `render6-boot2-SCREEN2-small.png`)
The menu bar is empty except the crystal (the focused window's name appears once something has focus); the
console window is a kernel log (row A26, QUIET-PANEL never ported to aarch64); core 0 at 94 %, five cores at 0 %.

## Committed on hw-jetson (all local; Peter's `git push origin hw-jetson`)
`8ab82761` NEUTRAL-TABLE row 47 · `191823c2` flight result boot 1 + R21 + A25 · `bb3294af` boot 2 excerpt ·
`66515b59` ledger ticks + S29 + S15 re-measure · `a560d185` + `8a9a231c` S7 re-cut v2 (rmbp GRANTED main.rs,
pi 7 ACKED) · `fb5d0d8a` TCURX2 (A16 rung 2, knob `tcurx`; loadable image byte-identical knob-off; union gate log
`~/unaos-bench/scratch/orin15/check-fb5d0d8a.log`).

## Executor branches still to fold (see the orin-16 baton table)
`exec-orin15-clickdead`, `-menubar`, `-consolequiet`, `-dragdead`, `-quarry`. Grants: menubar (rmbp A–J),
fbcon (standing parity grant + K–N), main.rs S7 (rmbp, pi-acked). Probe images staged: sdmmcwrite, net4,
ga10bprobe2 (+ tick1).

## Process
The session opened by asking who holds the focus and idled an hour; the rule that produced that is amended and
tooled (`orin-open.sh`). The session's Fable spend was its context size re-billed per turn — orin 16 opens lean.

## Union gate on the TCURX2 fold (fb5d0d8a)
`./arroyo check` exit 0 — 69 ✅ / 0 ❌, GATE-KNOB OK, GATE-LEDGER OK (`~/unaos-bench/scratch/orin15/check-fb5d0d8a.log`);
`./arroyo test-arm 60` exit 0 (`testarm-fb5d0d8a.log`).

## Folds after the executor kill (2026-09-06 03:2x–04:4xZ)
DRAGDEAD `4dc88314` (+ A27 tick `7580018e`) · QUARRY `f36cb82f` (+ A8 tick in `092c85fb`) · CLICKDEAD `40a11ca9` + docs
`092c85fb` (`CLICKDEAD-xhci.patch`, rmbp GRANTED on the split-counter condition; v2 in flight) · FOLDFIX `80ed35a4`
(the display_tegra.rs tail union rebuilt 3-way after the seat's hand splice cut a function open) · MENUBAR docs
`563a5717` (`MENUBAR-WINMENU.patch` to rmbp 12; pi 7 asked for the C ack; LEDGER S32). CONSOLEQUIET still running.
MENUBAR video commit `adb3b1cd` (the granted patch applied; ten files, 1403+/266−; union gate check + test-arm +
`UNAOS_WC=1 test 150` running at write time — logs `~/unaos-bench/scratch/orin15/{check,testarm,testwc}-adb3b1cd.log`).
CONSOLEQUIET docs `9e030696` (`CONSOLEQUIET.patch` for fbcon.rs to rmbp 12; row A26; the x86 image byte-identical, the
Pi panic-on-glass leg demonstrated). fbcon.rs commit waits on rmbp's read.
MENUBAR union gate on `adb3b1cd`: check exit 0 (both arches), test-arm exit 0, `UNAOS_WC=1 test 150` exit 0 (wc in the
banner, 69 PASS, 0 panics). Pi image: `kernel8.img` before = after (apply -R method; the value TCURX2.md records).
CONSOLEQUIET fbcon commit `3329eec6` (approved by rmbp 12; union gate check + test-arm + wc + kernel8 sha running at write
time — logs `~/unaos-bench/scratch/orin15/*-3329eec6.log`).
CLICKDEAD v2 docs `c769e697` (split counters per rmbp; `CLICKDEAD-xhci.patch` v2; Pi image moves once, both shas in the
doc; pi 7's re-accept asked; xhci/mod.rs applied on hw-jetson only after it).
CONSOLEQUIET union gate on `3329eec6`: check 0, test-arm 0, `UNAOS_WC=1 test 150` 0, `kernel8.img` unchanged (the Pi
knob-off image carries no `desktop_firmware`). Peter pushed hw-jetson to `12bbc0ed` (ls-remote 04:36Z); owed from there.
