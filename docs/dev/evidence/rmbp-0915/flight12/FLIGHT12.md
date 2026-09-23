# rmbp12 flight 12 — image 4 (hw-rmbp@6d8d3d2d), 2026-09-23 18:26Z–18:33Z

Capture: `~/unaos-bench/capture/rmbp12-flight8/ttyUSB0.log` from byte 5398457 (session-start MARK 17:55:40Z); the seat's slice
`~/unaos-bench/scratch/rmbp-0915/logs/foldgate/f12-boot1.log` (6899 lines). Staged tree `UnaOS-rmbp-esp-rmbp12flight12-20260923T1820Z-6d8d3d2/`
(kernel.elf sha256 2a3b42fe…). The wire ended at 411583 ms with NO `SHARD: shut down` line and no `[pwrshutoff]`: the machine was powered
off by hand. Peter's glass report is R63 (RULINGS.md), verbatim. Every line below is quoted from the capture; the loader window before
the first kernel line was not captured (the FTDI is kernel TX only).

## What booted
- `[vfs] root -> NONE reason=no-disk-enumerated sha=6d8d3d2d …` at 177 ms — the image's own sha on the wire; `:: KEYMAP: table=crispy …
  rows=8/6 -> PASS ::` (a string only image 4 carries). `:: X86BIND: root=sdhc:/kernel.elf serial=0x00000000 by=content … -> PASS ::` at
  32220 ms. `FRGUARD: boot volume serial ABSENT (0)`.

## Pre-registered rows (playbook order)
| # | row | reading |
|---|---|---|
| 1 | BOOTCLOCK | `firmware->loader=13311ms loader-read=741ms loader-jump=20ms` (flight 10: 12992). `:: BPACE: pci-probes t=5939ms d=105ms`, **`pci-scan t=5939ms d=0ms`** — B152's 1315 ms is gone. `BPACE: total gui=8344ms`. |
| 2 | VECTORS | `[vectors] allocated=3 free=172 table=timer:0x20,xhci:0x40,nic:0x41,ipi:0x42,ehci:0x43,spurious:0xff == witness ::` — B168 first metal reading, as predicted. |
| 3 | IOAPIC | `[ioapic] census ioapics=1 isos=2 gsis=24 nmis=8 dropped=0 madt_entries=11`, `[ioapic] id=2 addr=0xfec00000 gsi_base=0 entries=24 version=0x20`. BUT `[ioapic] route bdf=0:29.0 pin=INTA line=0 -> REFUSED reason=no-firmware-line` at 5833 ms and the ISRARM line STILL reads the flight-11 text `there is no IOAPIC in this kernel to route INTx to` — the text is stale (an IOAPIC is in this kernel; the route was refused for no-firmware-line), a wire-beats-comment defect in the message itself. EHCI-HID stays POLLED. `ehci-hid-done t=5833ms d=5538ms`. |
| 4 | SDHC rw (R59) | first `:: SDHCPOST: posture … write-path=ABSENT -> sdhc=ro reason=no-write-path` is the pre-registration self-test (leg 8, says so); then at 32154 ms **`:: SDHCPOST: posture sdw-ro=0 wp-pin=enabled write-path=live -> sdhc=rw reason=none`** and **`:: SDHCBLK: FAT mounted READ-WRITE on the internal SD card (29721 MiB)`**. R59 on the metal. |
| 5 | sertx / PASSPERIOD / tp | `PASSPERIOD self-test … pass_period_us_max=40000 … -> PASS`; `[sertx] … taps_us_max=18 tap_max=fbcon:0,ftdi:11,tste:12,rec:4`; `[tp] mode … widx=1 try=hid1.11-intf … latched=no` then `widx=0 try=legacy-index0 … latched=yes`. |
| 6 | menubar | `[menubar] first-paint at=6699 after_enable_ms=0 model=partial:caption+clock+batt crystal=drawn rect=2880x34+0+0` once; `[wc-x] desktop-app HOLD-NONE name=/STAT.ELF held_ms=0` once, `HOLD-EXPIRED` 0 (B167); `[status] poll n=1 answered=1 src=smc took_us=1317`, `[menubar] battery pct=84 charging=n … src=smc mv=11640 ma=-2410 polls=21/21` (B170 — the pack answers). BUT `:: MENUFIRST: … crystal=absent … painted=false :: FAIL ::` at 48497 ms — see §Fixtures. |
| 7 | DMG-REFUSE | `19/19 probes from two ring-3 slots agree` at 55191 ms (B172 metal shape, read). |
| 8 | KEYMAP | PASS (above). |
| 9 | LOGIN | `[login] screen open window=2 box=1330x764 at (775,345)` at 8358 ms — a WINDOW over the live desktop (the desktop app launched at 32152 ms and the dock, menubar and STAT ran under it). NO `[users]` line all boot. One `[login] press at=(1158,491) control=none answered=0 swallowed=1`. Keys (EHCI-HID KEY lines at 260–268 s: Tab, Tab, s,t,o,r,m, Enter) reached the DESKTOP: `[wc-c] focus tab-cycle`, `[winmenu] app-menu owner=4 name=STAT`, then `storm` to USB-DEBUG — the screen never had focus. |

## Input
- Keyboard: NOT dead — `KBDWIT … SILENCE-BROKE … quiet_ms=238451` then keys, routed past the login window (above).
- Trackpad: DEAD — `:: EHCI-HID: [1] STOP-NOTE interrupt endpoint halted addr=6 ep=IN1 kind=boot-mouse mps=4 class=xact-err-burn tok=0x00048141 cerr=0 halted=1 … reports=0` at 32422 ms (`drivers/ehci/mod.rs:13863`; the mod.rs:14457 table names token 0x00048141 as the 05ac:820b BT-HID-proxy mouse). `[deadman] … hid=0 … in=0/0/0` from then on. The ONE `[cursor] armed x=961 y=617` at 50266 ms follows `[ptrdead] backlog … pushed=192 … travel=(192,-192)` — PTRDEAD's synthetic motion, B186's Class 3 leak, on metal.

## Fixtures at 48–55 s (the desktop battery ran UNDER the login window)
`[clickroute] … deliver=false -> FAIL`, `:: DOCK: strip … vacate=false … :: FAIL ::` (the §1e shape; DOCKVAC in flight), `:: MENUFIRST: … crystal=absent :: FAIL`, `:: APPQUIT: … :: FAIL`, `:: CLICK-BAND: … :: FAIL`, `:: MENUDROP: … open=false :: FAIL`, `:: SERIALDOOR: … control=false wire=false :: FAIL`, `:: APPPIN: … :: FAIL`, `:: SHOTMENU: … :: FAIL` — nine reds within seven seconds, all press-driven, all while `[login] press … swallowed=1` states that every press is the screen's. Under R63 the screen is not open at boot 13; the battery's metal reading is owed from that flight, not from this one. PASS in the same window: `:: DOCKID: … reconciled=3/3 folds=16 :: PASS`, `[dmgovlp] verdict passes=12/12 … adopt_stretch=0/4 -> PASS` (the flight-11 FAIL is gone), `:: GLASSFIX2:`, `:: PRTSCR-DIR-FIX: no session -> REFUSED reason=no-session … -> PASS` (R54), `:: HDA-TONE: … run_ms=1200 members=2 -> PASS` with `[hda] codec=0 vid=1013:4206` (first read of the Cirrus codec; whether it was audible is not on the wire and Peter did not say). Not run: storm (`wedge-sample` 1 = the fixture), VUGART (0 frames), ⌘⇧3, `[clip]` chords (9 lines = fixture). `GATE STOLEN` 0. PANIC 0.

## KVBLANK2 (B179) first metal reading
`:: kepler: vblank bdf-hunt … found=1 bdf=1:0.0`; `pmc-arm … deliver=none reason=no-vector-helper`; `vblank arm head=0 vt=1852 mode=poll src=HEAD_STAT.VERT[31:16]`; selftests PASS and GO-RED-OK; then `vblank head=0 count=15 period_us=127022 jitter_us=1214654` at 8319 ms and `count=10430 period_us=38241 jitter_us=23605994 … mode=poll vbwaits=316 vbwait_us=3689731 vbgaveup=0` at 405 s. The interrupt path did not arm (no vector helper); the poll path ran; the period is 38 ms, not 16.7 — a 26 Hz vblank count on a 60 Hz panel.

## Decisions taken from this flight
R63: boot 13 boots to the root desktop; `adduser` from the session; Log Out returns the login screen over nothing. Two executors named (LOGIN13, MOUSEHALT) — Peter's go pending at the time of this record.

## Correction (CRYSTAL2, rmbp-ledger B194, 2026-09-23)
§Fixtures files `:: MENUFIRST:` under "all press-driven". It is not: MENUFIRST presses nothing, and its `painted=false recorded=false` is eight declined paints (menubar `decl_lock` 0 -> 16 across the fixture, `paints=3` before and after) — FIXTURE_FLAKES §6b, Class 6. `:: MENUBATT: … change_paint=false :: FAIL ::` in the same second is the same swallow; `:: DOCK: … vacate=false` is B187's COMP_GATE decline. The one `[login] press … swallowed=1` is `clickroute_selftest`'s.
