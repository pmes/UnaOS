# BP-input — HID to focus to ring 3 (read at 1719fe54)

## 1. What it is
- Decoders: xHCI HID (`unaos/crates/kernel/src/drivers/xhci/mod.rs`) and EHCI HID incl. Bluetooth HID and the rMBP Wellspring trackpad (`drivers/ehci/{mod,bthid,finemotion,tpgest}.rs`).
- Every decoder emits `pal::Event` through ONE seam, `pal::push_event` (`unaos/crates/kernel/src/pal.rs:1341`): `Key`, `KeyUp`, `Mouse`, `MouseAbsolute`, `Button`, `Wheel`, `Action`, plus internal `Timer/None/Unknown` (`unaos/crates/kernel/src/pal.rs:22-80`).
- Chords are a TABLE (`unaos/crates/kernel/src/video/keymap.rs`): role mask + HID usage → `Action`; `keymap::resolve` is the one judge (`unaos/crates/kernel/src/video/keymap.rs:316`).
- The drain offers each event in order: the login screen (`login::consume_key`), then the focused ring-3 slot (`user_input_route`), else the shell/kernel window.
- Ring 3 reads packed `INPUT_EV_*` words from a per-slot SPSC ring (`SYS_INPUT_POLL` 27 / `SYS_INPUT_WAIT` 28).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-input-1 | All decoders reach the event queue through `pal::push_event`; brightness Actions are consumed there, never queued. | `unaos/crates/kernel/src/pal.rs:1341` | unheld |
| BP-input-2 | No chord is a literal in a decoder: bindings live in `keymap` tables; a second keymap (Softy's `una-pc`) is a table, not a driver edit (R60). | `unaos/crates/kernel/src/video/keymap.rs:17-40` `unaos/crates/kernel/src/video/keymap.rs:90` `unaos/crates/kernel/src/video/keymap.rs:316` | `keymap::selftest` (runs where reached) |
| BP-input-3 | Edit chords are COMMAND-key rows on every window including the terminal; `Ctrl-C` is not a row and reaches the shell as 0x03 (R61). | `unaos/crates/kernel/src/video/keymap.rs:27-33`; `unaos/crates/kernel/src/drivers/xhci/mod.rs:222` | `keymap::selftest` |
| BP-input-4 | While the login/set-password screen is up it takes every key BEFORE serial echo and the console; no typed byte reaches the wire. | `unaos/crates/kernel/src/video/login.rs:17-27` `unaos/crates/kernel/src/video/login.rs:954` | `[login] key taken by the screen …` line |
| BP-input-5 | Input to the setter/login screen is never blocked by anything running; key latency is measured per key. | `unaos/crates/kernel/src/boot.rs:23-27` `unaos/crates/kernel/src/boot.rs:284-330`; R86, R88 | `[login] key latency ms=` line |
| BP-input-6 | Focus is ONE word (`USER_INPUT_ACTIVE`, slot+1; 0 = the shell owns the keyboard); a bad slot is refused, never published; a fresh focus clears the ring. | `unaos/crates/kernel/src/arch/x86_64/syscall.rs:5661-5672` `unaos/crates/kernel/src/arch/x86_64/syscall.rs:5965-5972` | unheld |
| BP-input-7 | The ring is SPSC with a single producer (the router), DROP-NEWEST when full, tail published after the slot store, wake after the tail. | `unaos/crates/kernel/src/arch/x86_64/syscall.rs:5636-5658` | `user_input_stats` counters (delivered/dropped) |
| BP-input-8 | A routed event comes back to the drain as `Event::Unknown` (consumed), an unrouted one unchanged; the shell never also handles a routed event. | `unaos/crates/kernel/src/arch/x86_64/syscall.rs:5703-5710` | unheld |
| BP-input-9 | The packed event has bit 63 clear (type at [55:48]); `INPUT_EV_*` numbers are una-abi's and never renumbered silently (renumbers are recorded at the const). | `unaos/crates/una-abi/src/lib.rs:394-451` `unaos/crates/una-abi/src/lib.rs:752-762` `unaos/crates/una-abi/src/lib.rs:1847` `unaos/crates/una-abi/src/lib.rs:1949` `unaos/crates/una-abi/src/lib.rs:2381` | compile |
| BP-input-10 | The wheel is read from byte 3 of the relative boot report only when the transfer length says it exists; delivery is to the focused window (no consumer scrolls yet). | `unaos/crates/kernel/src/drivers/xhci/mod.rs:5084`; `docs/PRODUCT.md` (Scrolling row) | `[wheel1]` census |
| BP-input-11 | Pointer and keyboard never falter; a faltering pointer is a fault named on the wire, not a load (R88, R103). | `unaos/crates/kernel/src/hidstall.rs:1-8`; R88 | `[lag] stall … stage=hid …`; `[hid] stall census` |
| BP-input-12 | Trackpad gain is Principia's `system.trackpad.speed`, applied after the sub-pixel curve; x86 only (the pad hangs off EHCI). | `docs/dev/OS/05_USER_EXPERIENCE/trackpad.md:1-10` | `tests pointerfeel` |

## 3. Seams
- Decoders may call: `pal::push_event` only. They may not call `wm`, `login` or ring-3 code.
- The router may call: `login::consume_key`, `user_input_enqueue`, `wm` hit-test/focus, `keymap::resolve`.
- aarch64 carries the same router seam (`unaos/crates/kernel/src/arch/aarch64/syscall.rs:13188`); the arch `syscall.rs` owns the ring, una-abi owns the encoding.
- Services that hold a lock the input lanes need run bounded (cores.md BP-cores-9, -15): input rides the service core's cadence.

## 4. Wire
`[login] key latency ms=` · `[login] key echo comp_ms=` (`unaos/crates/kernel/src/video/login.rs:2951`) · `[lag] click→shown ms= … worst=` · `[lag] stall … stage=hid hid_gap_ms=` · `[hid] stall census … pump_cpu= -> CLEAR` · `[wheel1]` census.

## 5. Ruled, unbuilt
- Scroll consumers: no content offset in `wm`, no scrollbar widget, no rect-scoped blit (`docs/PRODUCT.md`, Scrolling row).
- R103's pointer order is standing; POINTERSCALE2 (B554) is fixed-unflown at its newest rung per `docs/dev/OS/05_USER_EXPERIENCE/trackpad.md:3` (status words there are ledger-owned).

## 6. Read further
- `docs/dev/OS/08_VIDEO/keymap.md`, `docs/dev/OS/05_USER_EXPERIENCE/trackpad.md`, `docs/dev/OS/05_USER_EXPERIENCE/cursor.md`.
- `docs/dev/OS/07_USB_STORAGE/xhci_hid_enumeration.md` — HID bring-up.
