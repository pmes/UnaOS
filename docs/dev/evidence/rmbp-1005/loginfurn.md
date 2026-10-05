# LOGINFURN (rmbp-ledger B374, R88) — nothing opens itself at login; the console shows the boot

Cut from hw-rmbp a60219de (flight 23 flown on image 16). Wire: `docs/dev/evidence/rmbp-0915/flight23/f23-boots.log`.

## Finding (from the wire, both boots)
- At `login ok` three things open themselves: `[dock] furniture relaunch posted console+shell` (login.rs
  `close_into_session` drains the R77/R86 `SWEPT` latch into `dock::relaunch_furniture`; boot 1 also has
  `installer_release` for the same latch), then `[wc-x] desktop-app HOLD-NONE name=/STAT.ELF` (desktop_uefi's
  DESKTOP_APP, held only until `desktop_allowed`).
- The console is BLANK for a second reason than "no history": the R86 bare activation calls `fbcon::detach()`
  (`GUI_ACTIVE = true`) and nothing clears it when the console window is minted later, so `fbcon::_print`
  returns at its first test for every line — the routed console takes no glyphs at all. And the window is
  minted fresh (`cells_remint from=none`), so even a live route would start empty.
- The boot's text already lives in one ring: `flight_recorder` (x86, no knob, 256 KiB static, fed from the one
  serial print seam `arch/x86_64/serial.rs::_print`; the ring `UNAOS.LOG` is flushed from). No second ring (R79).
- INSTALLBARE `windows=1` on boot 1 is NOT the setter's dialog (login's OWNER is 0 and `note_window` already
  skips owner 0). The wire names it: `[wc-fv] focus raise asid=0xffffff52` + `:: SHOTMENU: win=1 … :: FAIL ::` at
  09:09:00, under the setter — the `witness`-gated SHOTMENU fixture at the end of the demo chain mints a
  `KERNEL_OWNER_BASE + 0x52` "Glass" window at boot (an R80 test at boot), and it is the row the create-user sweep
  closed (`furniture swept n=1`).

## Seam
Kernel — kernel-by-ruling (R88). No new store: the prefill reads the flight recorder's ring; the console is
fbcon's existing window route; the census rides `boot::note_window`; the verdict is a `tests` fixture (R80).

## Milestones
- M1 — the bare login: `close_into_session` and `installer_release` post nothing; one line
  `[login] desktop bare: furniture=none (R88)`; STAT.ELF never self-launches on a `login` build; the services
  open at once (`:: BOOT: … why=furniture-none`) instead of waiting the 1.5 s furniture bound; the loginst
  fixtures (FIRSTBOOT-LOGIN, LOGOUT) read `desktop_bare=` (no launch posted) instead of `furniture_reignited=`.
- M2 — the console takes glyphs and is pre-filled: fbcon's first gate holds only while the bare-detached console
  is UNROUTED; on every console mint the flight recorder's tail (the grid's rows of lines) is replayed through
  `fbcon::_print` under one present, scrolled to the tail:
  `[console] prefill lines=<n> painted=<p> ring_bytes=<b> ring_full=<0|1> live=<0|1> (R88: …)`.
- M3 — INSTALLBARE's count: SHOTMENU is deferred behind `tests shotmenu` (R80), and the FAIL reason names the
  first pre-Desktop window (`first_window=owner:<asid>`), so a count is never again read as "the setter".
- M4 — `tests loginfurn`: `:: LOGINFURN: at_login windows=<w> services=<s> console_prefill_lines=<n> -> PASS ::`
  (`windows` = rows minted by a non-zero owner within 1500 ms of the login's ignition; `services` = kernel
  self-launches at login: a furniture post or a STAT launch; `console_prefill_lines` = the first prefill's
  painted lines, `none` while the console has not been opened).

## Witness (the next flight reads)
`[login] desktop bare: furniture=none (R88)` at `login ok`; no `[dock] furniture relaunch`, no
`desktop-app HOLD-NONE` after it; `[console] prefill lines=… live=1` when Peter opens the console;
`tests loginfurn` → `:: LOGINFURN: at_login windows=0 services=0 console_prefill_lines=<n> -> PASS ::`;
`tests installbare` → `windows=0 … -> PASS`.

## Owed
aarch64 has no flight recorder (the prefill reads `ring=none` there); the ring keeps the EARLIEST 256 KiB, so a
console opened after a long session shows the ring's end, not the wire's (`ring_full=1` says so); a
Desktop-from-the-first-instruction boot (no `login`) still mints its console at activation.
