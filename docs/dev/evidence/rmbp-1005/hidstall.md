# HIDSTALL (B485) — flight 26, R103: the pointer fixed once and for all

## Finding (read on the f26 wire, awk)
1. **The halt at session open is the BT HID PROXY, not the keyboard.** Boot 2: `BTHID: bring-up at 49632 ms on controller [1] addr=7` (HCI Reset, the radio goes to HCI) → the same second `STOP-NOTE … addr=5 kind=kbd mps=8 class=xact-err-burn reports=0` and `addr=6 kind=boot-mouse mps=4 … reports=0`. Those are `05ac:820a`/`05ac:820b` on the Broadcom hub beside the radio (BTPROXY, DEADKBD5 in `drivers/ehci/mod.rs`): with an HCI host up they never carry a byte and their TT answers ERR. The internal keyboard/trackpad (`05ac:0262`, addr 8, mps 10) never halted on any f26 boot. The xHCI `ENUM RECOVERY` lines are pre-login (13:31:37, 14:30:49) on the xHCI ports, not the EHCI HID path.
2. **The one-second `stage=hid` stall is the Dock's Trash poll.** Every stall line on boots 2/3 reads `pump=desktop-app pump_ms=189–232` at an exact 2 s cadence (at_ms 37212, 39219, 41219 …) and `hid_gap_ms=49`. `dock2_store_service` re-reads the Trash every `TRASH_POLL_MS = 2_000` (`fs::trash::count()` → a `TRASH_QUERY` over the volume), and on ROOTDISK2 `/home` is UnaFS, whose every transaction runs IRQ-MASKED (`with_unafs_attempt`). A masked span on the service core is not preemptible, so the `hid-pump` task on that core (cpu 7) cannot run for its length: 49 ms of no HID pass every 2 s.
3. **The 3–4 s key queue** (`INPUTSTALL key_queue_max_ms=3143`, 3716) is `stage=queue handler=-` during `tests openers` / `lumencrash` (UnaFS-heavy fixtures): the render task routed nothing for 3 s. The wire does not name its wait; this arc puts the masked span on the stall line so the next flight does.
4. **DOCKRELEASE**: `[dock] press … app=console -> armed (launches on release)` then 3 s later `pin drag app=console -> none launch=0` — the release came late and as a drag; nothing bounded the arm.

## Seam
Kernel — the EHCI HID driver (`drivers/ehci`), the Dock (`video/dock.rs`, the WM's furniture), the Trash's kernel I/O (`fs/trash.rs`, the fulfiller over `trash_core`), the lag instrument. One new file `src/hidstall.rs` (CHARTER: Kernel — kernel-by-ruling): the arc's counters and `tests hidstall`. No new knob, no new store.

## Milestones
- **M1** halt: a halted INPUT endpoint (xact-err-burn with reports>0, or stall) is cleared and re-armed — `[hid] recovered addr= ep= after_ms=`; the BT proxy (xact-err-burn, reports=0, mps<=8, a radio on the controller) retires as `[hid] proxy-retired addr= ep= kind= why=bt-hci-owns-radio input=internal` and is not a login halt.
- **M2** stall: the Trash state is read on a change (`fs::trash` generation: trash/restore/empty, the session user), never on a 2 s clock — `[dock] trash state why=<first|change> full= took_ms=`; the stall line names the masked UnaFS span: `masked_ms=`.
- **M3** DOCKRELEASE: a pin press with no release and no travel within 400 ms launches — `[dock] press at (x,y) app=<a> release=timeout after_ms=<n> -> launched`.
- **M4** `tests hidstall` → `:: HIDSTALL: halted_at_login=<n> recovered=<n> proxy_retired=<n> hid_stall_s=<n> key_queue_max_ms=<n> dock_timeouts=<n> masked_max_ms=<n> -> PASS|FAIL ::` (PASS iff halted_at_login=0, hid_stall_s=0, key_queue_max_ms<=50; counted from `phase=desktop`).

## Witness (the next flight reads)
No `[lag] stall … pump=desktop-app pump_ms=2xx` 2 s cadence; `[dock] trash state why=first …` once at login; `[hid] proxy-retired addr=5 …`/`addr=6 …` in place of a login halt; `:: HIDSTALL: halted_at_login=0 … hid_stall_s=0 … -> PASS ::`.

## Owed
- The 3–4 s key queue during UnaFS fixtures is NAMED (masked_ms on the stall line), not fixed: every UnaFS transaction is IRQ-masked for its whole length (UNAFSTXN); shortening that is the UnaFS arc's.
- A Trash change made outside `fs::trash` (a shell `mv` into `.Trash`) shows on the tile at the next trash op, not at once.
- The bcm5974 index-1 GET/SET_REPORT EP0 timeouts (2 s each, pre-login, every flight since f20) are TPMODE's (B459).
