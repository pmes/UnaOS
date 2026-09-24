# FLIGHT 15 — image 8 (rmbp12flight15, hw-rmbp@d2d7a0a8 = image 7 + LOGIN16), 2026-09-24, main bench

Capture: `f15-boots.log` (34498 lines, FOUR boots on one card; the fresh session log from byte 110).
Image: gate15s (artifact leg + certs on the bench, the QEMU proof the cloud's), banner-cert ok.

## Peter, verbatim (the glass)

- Boot 1: "the pw dialog got covered up and the mouse cannot click and drag anymore. i tried the 2 finger drag and the single finger click and drag but it did not work. i cannot select a window or open the crystal menu. also the mouse takes a lot of finger work to get across the screen."
- "that test sound is fucking frightening because it is so abrupt. it sounds like an old dial up modem in the middle of the crazy connection sounds it would make."
- Boot 2-3: "pw is set but i can't log out shut down and restarted it goes straight to root desktop. still can't log out"
- Boot 3-4: "it works! i've run a vug storm and it is running very better. smp is still weird though. seems like it should spread the load better. i tried plugging in the net dongle and did not get any activity lights and rebooted to the same results"

## 1. THE SITTING, END TO END, FOR THE FIRST TIME (R62/R63/R64/R65 on the metal)

- **LOGIN16**: `[7122ms] :: USERSMOUNT: rmbp-shape=sdhc qemu-shape=global none=none old-mount-on-rmbp=none this-boot via=sdhc -> PASS ::`, `[7127ms] [users] load volume=el0-fat(rw) via=sdhc src=none users=0 (fresh store)` — the store mounts through the card, read-write, where flights 12-14 read nothing (13, 14) or `NoDisk` × 4096 (14). `[rand] source=rdrand probe=cpuid.01h.ecx.30=1 bits=256` (multiuser.md §8.3 line 2's Ivy Bridge prediction, confirmed).
- **LOGIN14 root**: `[login] root password unset row=created -> set-password screen`, `[login] set-password screen open user=root` at 7204 ms; boot 1 ended in `[login] set-password user=root retype mismatch (nothing written; the form stays)`; boot 2 `row=present` (the row persisted on the card without a credential), `[51936ms] [users] password set user=root first=true`; boot 3 `[users] load … src=dat users=1 seq=2 ver=2`, `[login] root password set row=present (LOGIN14: nothing to ask)` — no alert, straight to the root desktop, as ruled.
- **R63 Log Out gate**: `[users] logout REFUSED session=root reason=no-users (R63: adduser <name> first …)` six times over boots 2-3 — correct, and SILENT on the glass ("still can't log out"). LOGOUTUI is proven necessary by this boot.
- **adduser una → Log Out → the screen → first login**: `[239279ms] [login] screen open window=3 box=1330x764 at (775,345)`, `[298214ms] [login] first login user=una -> set password`, `[login] set-password screen open user=una login_after=true in_place=true`, `[310335ms] [users] password set user=una first=true`, `[users] home=/home/una exists volume=00000000`, `[users] login ok user=una id=8784219 principal=user:una#8784219`, `[login] session open user=una (first login: the password was chosen here)`; then `[322575ms] [users] logout epoch=3 ended=0 windows=0`, `[login] logged out — screen returns`, `[login] screen open …`, `[330792ms] [users] login ok user=una …`, `[login] session open user=una` — a second login with the chosen password. On boot 4, `PRTSCR-DIR-FIX: … dir=/home/una/Desktop home=/home/una`. "it works!"

## 2. Findings

- **LOGINZ (new, the boot-1 blocker)**: the set-password alert takes every press (SO44, by design) but is an ORDINARY window in z-order: created `win=3 … z=4` at 7177 ms, then `win=4 … z=5` at 7217 ms and, at 23703-23726 ms, `win=5/6 … z=6..10` with `[wc-fv] focus raise` — the launcher/fixture windows landed ABOVE the input-taker. Peter could not see it or click anything else ("covered up … cannot select a window or open the crystal menu"). The keyboard still reached it (his mismatch, then his password). Fix: the screen and the set-password alert are pinned topmost while open (the compositor's z is theirs; a later create or focus-raise cannot pass them); go-red = a window created after the alert opens with a higher z.
- **LOGOUTUI**: §1 — six silent refusals; the cloud's question to Peter stands (a small alert or a status-bar line).
- **TPSPEED**: "the mouse takes a lot of finger work to get across the screen" at `div=8` (flight 14: "mousing is great" at the same divisor, different bench/table). A reading, not a failure: 8 is at the slow edge; 6, or 8 with a velocity term, is the next try. The click-and-drag failure on boot 1 was the alert swallowing presses (§2 LOGINZ), NOT the pad — on boots 3-4 Peter ran a vug storm with the pad. TPDRAG (two-finger) stands, unread here.
- **HDATONE3**: "frightening because it is so abrupt … like an old dial up modem" at 4096 with `HDA-PCM … -> PASS` — a modem squeal is PITCH: the converter rate (the format the codec accepted, `0x0011`, vs what its clock runs) is the first read, as the cloud's row says. Also the ONSET: the stream starts at full amplitude with no ramp ("so abrupt") — a 20 ms fade-in is cheap and is owed with the rate read.
- **SMP** (Peter): "smp is still weird though. seems like it should spread the load better" — a reading for the SMP arcs (SMPBALK): no per-CPU load line was pinned for this flight; the next image carries one so the glass and the wire can be compared.
- **The dongle**: `[1297ms] :: EHCI-HID: [1] M1 hub-downstream device addr=3 0b95:1790 class=0x00 speed=HS depth=1 (parent hub 1 port 2)` — the AX88179 enumerated on the EHCI [1] hub, NOT on the xHCI. USBNET is an xHCI-path driver (usb_xhci.md / network_stack.md) and its knob `UNAOS_USBNET=1` was not armed on this image (by the plan: the dongle test was deferred). So no activity light is EXPECTED twice over; the reading that matters is the port: on this rMBP the port Peter used is behind the EHCI hub, so USBNET needs an EHCI front-end (the `ftdirx`/`ehcihid` pattern) or Peter uses the other port. DECISION for Peter: which port did he use, and try the other one next flight with the knob armed.
- **VUGART2**: the fixture red 993 lines and green 75 across four boots (the PASS run is the `frames=N coherent=N torn_rows=0` shape at 342-350 s); the glass: "running very better". The threshold question stands.

## 3. Flown green (rows to flown)

LOGIN16 (the mount ladder), LOGIN14/B198 (root's password on the glass, the first-login password), LOGIN13/B189 (root at boot, adduser, Log Out ends root, the screen takes every key), LOGIN15/B213 (predicate), the R63 no-users gate (correct), the persisted store across four boots (`src=dat users=1 seq=2`). IOAPIC3/B216 armed again (`ISRARM armed via=ioapic-intx gsi=22`). TPSCALE/B214 usable (slow edge).

## 4. Owed to the queue

LOGINZ first (it blocks the sitting on any fresh card), LOGOUTUI (Peter's answer), HDATONE3 (+ fade-in), TPSPEED (div 6 or velocity), the dongle port decision (Peter) + USBNET's EHCI question, an SMP load line pinned for the next flight, VUGART2, TPDRAG.
