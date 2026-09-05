# Orin ledger — every finding, ticked once

> Rule (SECURITY.md's, applied to this track): **an arc that fixes, flies, or drops a ledger item
> ticks it here in the same commit.** Every audit, inventory, or capture review folds its findings
> into this table the turn it lands. Before spawning any audit, brief it with this file: report
> only what is NEW or CHANGED. A finding re-derived because it was not here is the waste
> (Peter, 2026-09-05).

Status: **open** · **fixed-unflown** (in tree, not on metal) · **flown** (scored on the wire) ·
**dropped** (ruled out, with the ruling) · **relayed** (another lane owns it).
Sources: `od` = `docs/dev/OS/08_VIDEO/orin-desktop.md` · `AC` = `~/unaos-bench/scratch/orin12/ARCH-CONFORMANCE.md` ·
`OB` = `~/unaos-bench/scratch/orin13/broken/ORIN-BROKEN.md` · `RA` = `~/unaos-bench/scratch/orin13/audit/RENDER2-AUDIT.md` ·
`SD` = `~/unaos-bench/scratch/orin12/SERIAL-DEVLOOP.md` · `FR` = `~/unaos-bench/scratch/orin13/FLIGHT-RESULT.md`.

## A. What the operator sees broken (ranked by panel impact, OB table 1, 2026-09-05)

| id | item | status | evidence | closed by |
|---|---|---|---|---|
| A1 | No desktop furniture (bar, dock, crystal); clicks land on nothing. §5.2 stop-line refuses `desktop_firmware::activate()` until the boot-core stack through the cascade is measured | **open → in flight** | `od` §5.2, §3.12; OB#1 | CASCADE executor (knob `deskcascade` + boot-stack probe) |
| A2 | No EL0 program owns a window; `bg` refused; only the boot core drops to EL1 | open — Peter's SMP D1–D5 ruling | `sched.rs` EL0-EL1CORE; OB#2 | — |
| A3 | Keystrokes ignore focus; every key goes to the shell | open — design (desktop pseudo-owner) | `xusb_tegra.rs` header; `main.rs` ~:2916; OB#3 | — |
| A4 | Serial RX dead; every retry is a card write + power cycle | **fixed-unflown (pending fold)** | `SD`; FR q7 | ORINRX executor (knob `orinrx`, LSR witness) |
| A5 | Strip + pulse window paint once and freeze (`presents=2`); core 0 load a structural 100% because the capstone loop never folded idle | **fixed-unflown** | FR; OB#5 | LOADSAMPLER `341ca707` |
| A6 | One core, no preemption; five APs run zero work | open — `bsprun`/`bsptick` unflown | OB#6 | — |
| A7 | Dock minimise round-trip never proven; aarch64 shell tile dead (`SHELL_REOPEN` drained only on x86) | open | OB#7; AC | dock drain is video/ (rmbp lane) |
| A8 | No file browser (`quarry` compiles, cascade-gated) | open — follows A1 | OB#8 | — |
| A9 | Print Screen produces nothing: service call never reached the terminus | **fixed-unflown** | OB#9 | PRTSCR-ORIN `2000608a` (needs `UNAOS_HOLOCRON=1`) |
| A10 | `<Esc>` cannot dismiss the pulse window's menu | **dropped** — pulse window retired on the Orin (Peter 2026-09-05: it loads the old Pi background-pulse view) | OB#10; AC#20 | CASCADE (removes arm/service) |
| A11 | TAB mid-drag leaves the grab on the old window (x86 cancels) | open | OB#11; AC#7 | aarch64 `wc_focus_key` |
| A12 | No DHCP lease (NO-OFFER → static fallback); default jetson image carries no `net4` | open | OB#12; NET-4A | — |
| A13 | No `flight_recorder` on aarch64 (x86-only service) | open — FC-2 shape | OB#13; `main.rs` ~:1747 | shared file; rmbp's FC-2 gate |
| A14 | No Wi-Fi | open — out of the north star | OB#14 | — |

## B. Capture findings (RA, render2 boot 2026-09-05)

| id | item | status | evidence | closed by |
|---|---|---|---|---|
| B1 | Pulse window overlaps the console's bottom 4 rows (prompt row); `[wc-h] win=3 span=64 band=yes` | **dropped** — pulse window retired (A10) | RA N1 | CASCADE |
| B2 | Both USB hubs fail status-change endpoint configure (codes 17 / 8); hot-plug behind hubs dead all boot. rmbp's read: interval/ESIT math in the endpoint context | **relayed** to rmbp 11 (ledgered rmbp-12 P5) | RA N2; `xhci/mod.rs` ~:13465 | rmbp; fix flies on this bench |
| B3 | `MOUSE-1` prints `vid:pid=0000:0000` for hub-attached pointers | relayed to rmbp 11 | RA N3 | rmbp |
| B4 | Three power-ons that session, two dark boots of the foreign volume `0xabfbdefa` (old loader); the firmware can still pick it | open — bench: find and wipe the medium carrying the old loader | RA §8; FR | Peter |
| B5 | `PIUSB` witness family prints on the Orin (shared USB-storage driver, Pi-named) | open — GATE-NEUTRAL census item | RA §6 | rmbp's GATE-NEUTRAL |

## C. Architecture-conformance findings still open on the Orin (AC, 2026-09-02)

| id | item | status | closed by |
|---|---|---|---|
| C1 | AC#1 knob→leg coverage check cannot fail on this branch | open until trunk carries KNOBLEG `647f485a` | rmbp landing |
| C2 | AC#7 = A11 | open | — |
| C3 | AC#8/#9 layering vocabulary (`user-*` crates, "Ring 3" vs userspace) | open — doc | — |
| C4 | AC#12 `scan_serial_faults` passes on a missing log (negative-only suites) | open — arroyo (shared) | GATE-BLINDNESS (rmbp set) |
| C5 | AC#14 GATE-BOOTLOADER hole: `unaos_ivb` leg | **fixed** on hw-rmbp (GATE-ROOTS `e1bff790` leg 4) | rmbp landing |
| C6 | AC#20 = A10 | dropped | — |
| C7 | AC#21 (see AC) | open | — |
| C8 | The six render-pass defects (AC on ORINRENDER) | **flown** render2, all six scored | `a5a66fc1` `7ffd2122` `01739a93` `8085c9c8`; FR |
| C9 | `sys_cap_revoke` leaks the fd on aarch64 | **fixed-unflown** (QEMU kernel8-test 119/119 reaches it) | CAPREVOKE `06858185` |

## D. Decisions argued, awaiting Peter

| id | item | recommendation | source |
|---|---|---|---|
| D1 | Loader `SetMode` to the widest console mode: the 80-col wrap occurs ONLY on F11-menu boots (7/39); auto-boots inherit 240x56 | do it narrowly, knob-gated, before/after witness; one power cycle to test | `~/unaos-bench/scratch/orin13/consolemode/ARGUMENT.md` |
| D2 | `render_service` size-3 family: convergence arc (lift the waiting + input-ownership axes) | owed; ledger entry has an expiry | merge `be3b027e` body |
| D3 | GA10B: read-only probe rung exists, gates unverified; licensing/bunker ruling pending | Peter | `od` §4; resume |

## E. Landed but unflown (OB table 2) — each with the one question its flight answers
TABKEY `ac9c0701` · `orintenant` (rung 6) · `orinladder` (a) glyphs-on-glass, (b) minimise round-trip ·
`orinfurn` bar · `tegradesk` floors · the five §3.13 terminus instruments · `bsprun`/`bsptick` ·
REDZONE absorber (0 lines on render2 = never fired, not held) · `reboot` verb (PSCI, never fired) ·
NET-4 fix · window-body persistence after occluder subtraction · PRTSCR-ORIN (A9) · LOADSAMPLER (A5).

## F. Hardware support inventory
Pending: ORINHW executor output (`~/unaos-bench/scratch/orin13/hw/ORIN-HW.md`) folds in here as
table F when it lands — one row per subsystem, five states (works / compiled-unflown / stub /
absent / ruled-out).
