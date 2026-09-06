# PRODUCT.md — what the OS actually does, per chip

One page. Rows are what a user experiences; cells are verified state, not claims.
LAW: any arc that changes a row updates this table in the same commit. A feature is DONE
when its row says yes on every chip that has the hardware for it — not when it lands once.
Cell values: **yes** (verified on that platform) · **no** (verified absent) · **partial** ·
**?** (claimed in a landing report, not re-verified) · **n/a** (hardware absent).

| Experience | x86 (rMBP) | Pi 4 | Notes / evidence |
|---|---|---|---|
| Boot to graphical desktop | ? | yes | Pi: PA44 metal 2026-08-18 — desktop-clear 1920x1200 bg 0x2d2b55 HIT, DESKHOLD holds the mirror |
| Crispy theme paints (chrome/materials) | ? | yes | Pi: PA44 metal chrome-truth 5/5 HIT with textured chrome (ceramic_pp=8, knurl_pp=9) |
| AA font rendering (Noto Sans Mono) | ? | yes | x86 landed it first (fa193e6f, unverified this seat); Pi: PA44 metal faces noto20-aa (chrome) + noto16-aa (console) |
| Paper texture on content surfaces | ? | no | only call site is x86-gated instgui; Pi has no kernel content surface yet |
| App windows (create/present/composite) | ? | yes | Pi: 3 live windows @~100 comps/s on metal |
| Window drag by title bar | ? | partial | Pi: DRAG-PI wired + coalesced; metal refusals convicted by BLITWHO (same-core drain self-starvation, PA50 2026-08-18) and FIXED in-tree by DRAGFIX (yield/skip arms + latch self-heal at drain exit) — metal drag verdict = next boot |
| Window close (app windows) | ? | yes | Pi: close grammar exercised on metal |
| Close refusal on kernel furniture (no freeze) | ? | yes | drain-stall fix 49b3a5fe; wedge1 tripwire verified |
| Dock (window switcher) | ? | yes | Pi: [dock] census on metal; press verdict awaits a bench click |
| Menu bar visible live | no | yes | Pi: PA44 metal menubar ENABLED + PAINTED owns_pixels=true; x86 row awaits its seat's verify |
| Crystal (SHARD) menu reachable live | no | yes | Pi: PA42 boot5 press=crystal PASS (bar completes without mouse motion); PA44 crystal LIVE |
| Shell as typeable window | yes? | yes | Pi: SHELLWIN landed; PA42 boot5 SHARD-PRESS full chain PASS on metal |
| Boot-log console window | yes? | partial | Pi: console window exists + routed=true (PA44), but content is a frozen snapshot — live console (render core consumes console_service) still owed, M3 |
| Pulse instrument window (dual-view) | ? | yes | Pi: PULSEWIN; PA42 boot5 window + View toggle verified on metal |
| Mouse pointer (move/click) | ? | yes | Pi: MOUSE-1 + piusb24 on metal |
| Cursor never vanishes | ? | yes | fix 2977899c; awaits next metal session to confirm |
| Keyboard to shell | ? | yes | Pi: typematic + midden verbs on metal; serial shell drive with a GUI window focused = SERIAL-FOCUS PASS, PA42 boot5 (storm bounded, GUI untouched) |
| USB storage disk mint (incl. stuck-reader cure) | ? | partial | Pi: mint works after replug; hub-cycle rung refused port-shared once (ghost fix 7b87e045 unflown) |
| Boots + runs from ONE card (no data card) | ? | yes | Pi: ONECARD witness + ELF1/EXEC1/K2/K3/K4 all off the internal card, no USB attached (QEMU raspi4b default) |
| FAT read/write | ? | yes | Pi: midden write/rm/mv byte-exact on metal |
| UnaFS mount + verbs (the plain file verbs reach it; `setfattr -x`, `snap`) | ? | yes | Pi: fixtures green on metal boots; F2/F3 landed |
| One path namespace (ls/cat/run agree) | ? | yes | VFS seam 4cd87124; QEMU witnesses |
| Wired network (DHCP/ping) | n/a | yes | Pi GENET on metal |
| WiFi / Bluetooth | partial? | no | x86: GR27 b43 work in flight; Pi: needs bunker firmware, unstarted |
| 3D: software rasterizer demo | ? | partial | Pi: 21.6fps in QEMU; metal eyeball unflown; pidesk+pirast join fix d84bd04f |
| 3D: GPU accel (V3D) | n/a | no | wall downstream of every ARM-readable register; start4.elf prong ruled 2-then-1 |
| Audio | ? | no | no Pi audio path exists |
| Multi-core scheduling (SMP) | ? | yes | Pi: 4 cores on metal |
| EL0 process isolation + ACL persistence | ? | yes | Pi: K2 metal-confirmed 2026-07-11 |
| Reboot-surviving filesystem (CoW, power-cut safe) | ? | yes | Pi: K8a/K8b metal-confirmed |
| File manager (Quarry: volume tree + detailed list) | no | partial | `UNAOS_QUARRY=1`. x86 empty until VFS adoption (vfs.md §12.4). Pi: PA42 boot5 metal — opens 1152x720, 2 volumes, dock-pinned, FAT contents load; open defects: listing VERY slow (linear FAT walk), /fat listed twice, hover flashing, no launch verb (v2 headline), /usb rows absent. docs/dev/OS/05_USER_EXPERIENCE/quarry.md |
| Scrolling (any list, anywhere) | no | partial | Quarry only, and app-owned. The WHEEL now exists as a sensory channel: the xHCI HID decoder reads byte 3 of the 4-byte relative boot report (length taken from the Transfer Event residual, so a 3-byte no-wheel mouse is never read past), `pal::Event::Wheel(i8)` carries it, `INPUT_EV_WHEEL` packs it, and the router delivers it to the focused EL0 window — WHEEL arc, `[wheel1]` census. What is still missing is every CONSUMER: no app scrolls on it, no content offset in `wm`, no scrollbar widget, no rect-scoped blit. Reachability is strings-proven in the image; the delivery verdict is metal's, because QEMU raspi4b attaches no USB at all. quarry.md §4 |

x86 column is mostly **?** because this seat has never verified that bench; the x86 seat's
landing reports claim more. First x86 session under this law: verify and fill the column.
Desktop-layer gate-by-gate detail: docs/dev/OS/08_VIDEO/PARITY.md (in flight).
