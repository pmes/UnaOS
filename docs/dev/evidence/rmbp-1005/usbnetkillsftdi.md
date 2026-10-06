# USBNETKILLSFTDI (rmbp-ledger B486) — the dongle's first burst must not kill the FTDI

**Finding (flight 26 boot 1, `f26-boot.log`, the last kernel lines).** After `[usbnet] rx arm trb=7 … n=8` and
`tx dhcp discover n=1`, `SOCK-2` ran a ring-3 `sendto` (`[net] poll enter op=sendto sid=0 len=24 masked=1`) and the
50 ms lock scan printed, in one pass, `[lock] spin-wait name=usbnet.rs:209 at=usbnet.rs:560 waiter=20@cpu2
on=untracked ms=27` and `[lock] held-too-long name=smolnet.rs:480 at=smolnet.rs:833 by=20@cpu2 ms=77`; `poll exit`
never printed and nothing else ever reached the wire. Both are reports of a hold IN PROGRESS: the masked syscall
(tid 20, holding STACK) spun on the usbnet RX ring (`RXQ`, a plain spin lock) on cpu2 — the cpu `net-tick` was
pinned to (`[net] tick task=net-tick cpu=2`). `net-tick` pumps DHCP through `pump_until -> rx_ready(true) -> drive`,
which holds the xHCI LOAN unmasked and takes `RXQ` inside it (`deliver`); preempted there, it can never run again
under a masked spinner on its own cpu (WEDGE-8's F3 shape: a masked wait on a preemptible holder). The loan never
returned, so the main loop's device-service pass — the FTDI drain — stopped: the wire died, the glass (EHCI HID,
GPU) lived, and the dongle's removal later found the controller wedged. Not an event-ring, interrupter or TRB-cycle
fault: the FTDI and the dongle share one interrupter correctly; they also shared a lock discipline that was not.

**Seam.** Kernel (the xHCI driver's own usbnet module); no handler domain, no store. No new knob (rides `usbnet`).

**Milestones.**
- M1 — every `RXQ`/`TXQ` hold is a masked micro-hold (`MaskedRing`: the guard takes `IrqMask`, drops the lock
  before the mask); `drive()` holds the loan masked (the same discipline as `claim`). Witnesses:
  `[ftdi] alive after usbnet arm rx=<n> beats=<n>` once (the first FTDI bulk-OUT completion after the eighth RX arm,
  flight 26's death depth) and `[usbnet] detach slot=<s> was_up=<0|1> rings=emptied loan=pass -> clean` on every
  detach.
- M2 — `tests usbnetftdi`: 5 s of the controller pass plus a MASKED stack-side drive (the syscall's shape) while
  reading the FTDI heartbeat → `:: USBNETFTDI: ftdi_alive=1 rx_armed=1 events=<n> beats=<n> drives=<n> -> PASS ::`
  (`SKIP reason=no-dongle` / `reason=no-ftdi`).

**The next flight reads** (dongle on the direct port at boot): `[ftdi] alive after usbnet arm rx=8 …`, the wire
continuing past `SOCK-2` (`[net] poll exit op=sendto`), `[usbnet] detach … -> clean` after the pull with the
glass still moving, and `tests usbnetftdi` → PASS.

**Owed.** The loan itself is still held unmasked by the main loop's own pass (a preempted main loop delays, but no
masked path waits on it: every masked taker is a try-claim). Flight 25's RX resets every ~2 s are NETFRAME's.
