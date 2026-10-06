# INPUTSTALL2 (rmbp-ledger B388) — design

**Finding (the wire, flight 24 card 3, `f24-boots.log`).** The 16 s HID gap is the same 16 s on every boot
(16010 / 16016 / 16012) and it is not the device: `login ok` 12:38:46, `[net] tick task=net-tick cpu=7`
12:38:50, the mouse reports until 12:38:52, then NOTHING until 12:39:08 — and the window is exactly the
smoltcp boot witness ladder: `SOCK-1` (icmp, 12:38:49), `SOCK-2` (udp dns, 12:38:52), `SOCK-3` (tcp connect
`REFUSED`, 12:39:08), `SOCK-6/7` (12:39:08). The first report after the gap is `dx=-614` (the pad's own
accumulated motion: it was not polled). The ladder is called from `e1000::service_net()` INSIDE the
`usb-pump` task's loop (main.rs `usb_pump`), and each rung spins `smolnet::pump_until` for seconds; the
EHCI HID service (`service_ehci_hid`) is a step of the SAME loop, so `hid_gap_ms` = the loop's iteration
time = the ladder's wall time. The seat's candidates (prefs flush, deadman/census, wpace, UnaFS flush) are
not on that path. R80: a boot that TESTS (the SOCK ladder) took the input with it.

The 119 ms `render=handler` is a burst after login (4–5 lines at ~2.04 s, 31 lines in the capture), not an
idle period; the handler interval is everything between a route's exit and the next route/pass/park, and the
chart cannot say which step. The wm-stage clicks (`wm=92 … 276` in flight 25) are inside `wc_route_event`,
which is one guard over four doors — the chart cannot say which door either.

**The seam.** Kernel — wm (`video/lag.rs`, the instrument); the EHCI HID driver (its own pump task);
the network drive seam (`net_tick.rs`, which already owns the off-render net work since GLASSLAG).

**Milestones.**
- M1 — NAME it. `lag::seg(id)` marks on the render task (the four route doors in `wc_route_event`, the
  drain tail, the console/shell launch drains, instgui/login drain, the panel render, the shell present, the
  5 s rollups) and `lag::pump_seg(id)` marks on the `usb-pump` loop's steps; the per-second worst of each is
  carried on the stall line: `… strand=<s>/<f> handler=<step> handler_ms=<n> pump=<step> pump_ms=<n>`, and a
  pump step ≥ 50 ms makes the second a stall second.
- M2 — the HID pump never waits on the device-service pass: `service_ehci_hid` runs on its own task
  (`hid-pump`, the service core, one tick), started by `usb_pump`; `usb_pump` stops calling it once the task
  is live. `hid_gap_ms` then reads the HID pass's own cadence. Line: `[hid] pump task=hid-pump cpu=<c>
  (INPUTSTALL2: the HID pass never waits on the device-service loop)`.
- M3 — the SOCK ladder leaves the pump (R80 direction): on a boot with no e1000 (the metal: usbnet) the
  ladder runs on the `net-tick` task, never inside `usb_pump`; the e1000 (QEMU) path keeps its inline order
  (its hand-rolled `poll()` must not race the ladder). Line: `[net] ladder on=net-tick (INPUTSTALL2)`.

**Witness (the next flight).** Over the first minute after login with the pointer moving:
`:: INPUTSTALL: key_queue_max_ms=<50 comp_max_ms=<100 hid_gap_max_ms=<50 strand_pct=<1 bound=50 -> PASS ::`;
`[hid] pump task=hid-pump …` once at the desktop; any stall second names `handler=` and `pump=`.

**Owed.** The 119 ms handler and the wm-stage click are NAMED by M1, not yet cured (the next flight's
`handler=` / route door says which; one build per cause). The 2.4 s `click→shown comp=` on the console
press is the console window's open (prefill + composite), DESKTOPBUILT's furniture. VUGFITS strand_pct is
measured by the flight with M1–M3 in. The SOCK ladder belongs in `tests` (R80); M3 only takes it off the
input path.

**M4 (the seat's ruling, R80): the SOCK ladder is a test — `tests sock`.** The ladder (SOCK-1 icmp, SOCK-2 udp
dns, SOCK-3 tcp, SOCK-6/7 listener, SOCK-8 dns) no longer runs at boot on either path: `tests sock` arms it
(`[tests] sock armed path=<e1000-inline|net-tick> …`) and the next net pass runs it where M3 put it (e1000:
inline in `service_net`, its order unchanged; no e1000: `net-tick`, which then prints `[net] ladder
on=net-tick`). The lease (SOCK-5, `dhcp_link_tick`/`dhcp_acquire`) is the stack's service and is untouched.
Spec lines that change (R78: QEMU proves nothing; named so the bench's QEMU lanes are not surprised):
- `scripts/specs/x86-usbnet.spec` — the `REQUIRE :: SOCK-1: smoltcp icmp echo … 4/4 replies` and
  `REQUIRE :: SOCK-2: smoltcp udp dns query …` lines are commented out (moved under `tests sock`); a lane
  that wants them types `tests sock` and re-pins them.
- `scripts/banner-cert.sh:285` — `smolnet|:: SOCK-1: smoltcp icmp echo|-|measured(1)` seeds the smolnet banner
  cert on the boot SOCK-1 line, which no longer prints at boot: OWED to the seat (re-seed on SOCK-5 or
  `[tests] sock armed`). Not edited here.
- Not affected: `SOCK-2: ring-3 udp round-trip` and `SOCK-4` (u-probe fixtures in syscall.rs, not this
  ladder), `SOCK-5` (the lease).
