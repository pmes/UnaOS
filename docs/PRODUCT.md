# PRODUCT.md — what the OS actually does, per chip

One page. Rows are what a user experiences; cells are verified state, not claims.
LAW: any arc that changes a row updates this table in the same commit. A feature is DONE
when its row says yes on every chip that has the hardware for it — not when it lands once.
Cell values: **yes** (verified on that platform) · **no** (verified absent) · **partial** ·
**?** (claimed in a landing report, not re-verified) · **n/a** (hardware absent).

| Experience | x86 (rMBP) | Pi 4 | Notes / evidence |
|---|---|---|---|
| Boot to graphical desktop | ? | yes | Pi: PA38 metal session 2026-08-13 |
| Crispy theme paints (chrome/materials) | ? | yes | Pi: chrome-truth glass probes 5/5 HIT; materials 5–9/255 amplitude |
| Paper texture on content surfaces | ? | no | only call site is x86-gated instgui; Pi has no kernel content surface yet |
| App windows (create/present/composite) | ? | yes | Pi: 3 live windows @~100 comps/s on metal |
| Window drag by title bar | ? | **no** | Pi arm never wired (drag_begin has 1 caller, x86); port in flight |
| Window close (app windows) | ? | yes | Pi: close grammar exercised on metal |
| Close refusal on kernel furniture (no freeze) | ? | yes | drain-stall fix 49b3a5fe; wedge1 tripwire verified |
| Dock (window switcher) | ? | yes | Pi: [dock] census on metal; press verdict awaits a bench click |
| Menu bar visible live | no | no | ENABLED=false everywhere outside fixtures; port in flight |
| Crystal (SHARD) menu reachable live | no | no | same gate as menu bar; About works in fixtures only |
| Shell as typeable window | yes? | no | x86: trunk commits 3c182692/8564ae0d/ce5bf4d7 (unverified this seat); Pi port in flight |
| Boot-log console window | yes? | no | x86: panel_console_window_open (unverified this seat); Pi port in flight |
| Mouse pointer (move/click) | ? | yes | Pi: MOUSE-1 + piusb24 on metal |
| Cursor never vanishes | ? | yes | fix 2977899c; awaits next metal session to confirm |
| Keyboard to shell | ? | yes | Pi: typematic + midden verbs on metal |
| USB storage disk mint (incl. stuck-reader cure) | ? | partial | Pi: mint works after replug; hub-cycle rung refused port-shared once (ghost fix 7b87e045 unflown) |
| Boots + runs from ONE card (no data card) | ? | yes | Pi: ONECARD witness + ELF1/EXEC1/K2/K3/K4 all off the internal card, no USB attached (QEMU raspi4b default) |
| FAT read/write | ? | yes | Pi: midden write/rm/mv byte-exact on metal |
| UnaFS mount + verbs (incl. umv/urmattr, snapshots) | ? | yes | Pi: fixtures green on metal boots; F2/F3 landed |
| One path namespace (ls/cat/run agree) | ? | yes | VFS seam 4cd87124; QEMU witnesses |
| Wired network (DHCP/ping) | n/a | yes | Pi GENET on metal |
| WiFi / Bluetooth | partial? | no | x86: GR27 b43 work in flight; Pi: needs bunker firmware, unstarted |
| 3D: software rasterizer demo | ? | partial | Pi: 21.6fps in QEMU; metal eyeball unflown; pidesk+pirast join fix d84bd04f |
| 3D: GPU accel (V3D) | n/a | no | wall downstream of every ARM-readable register; start4.elf prong ruled 2-then-1 |
| Audio | ? | no | no Pi audio path exists |
| Multi-core scheduling (SMP) | ? | yes | Pi: 4 cores on metal |
| EL0 process isolation + ACL persistence | ? | yes | Pi: K2 metal-confirmed 2026-07-11 |
| Reboot-surviving filesystem (CoW, power-cut safe) | ? | yes | Pi: K8a/K8b metal-confirmed |
| File manager (Quarry: volume tree + detailed list) | no | partial | `UNAOS_QUARRY=1`. Module type-checks on BOTH arches, but its data source is the VFS mount table, which `fs/vfs.rs` gates to aarch64 — x86 opens on an empty volume list until the x86 VFS adoption lands (vfs.md §12.4). Pi: opens and reads `/` at bench geometry in QEMU; declines at 640x480 by the CONSOLEWIN law; metal eyeball unflown. docs/dev/OS/05_USER_EXPERIENCE/quarry.md |
| Scrolling (any list, anywhere) | no | partial | Quarry only, and app-owned: no wheel (the xHCI HID decoder drops the byte), no content offset in `wm`, no scrollbar widget, no rect-scoped blit. quarry.md §4 |

x86 column is mostly **?** because this seat has never verified that bench; the x86 seat's
landing reports claim more. First x86 session under this law: verify and fill the column.
Desktop-layer gate-by-gate detail: docs/dev/OS/08_VIDEO/PARITY.md (in flight).
