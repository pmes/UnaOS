# USBNETRX (rmbp-ledger B502) — the bulk-IN ring armed, rung, and silent

Branch `exec-rmbp-usbnetrx`, cut from 668cdd95 (the dev tip after the flight-27 merge). No new knob (`usbnet`).

## Finding (from the flight-27 wire, f27-boot1.log, awk `[usbnet]`)
- **First High-Speed attach ever.** `rx_arm n=1 ep=0x82 mps=512`, `qctrl=hs`, `stall … speed=3` (PORTSC 3 = HS). Every flight
  that saw frames (f22 rx_ok=35, f23 59, f24 44/45, f26 SS bursts at `rx arm trb=1..7`) attached at SuperSpeed (`mps=1024`).
  Flight 27 is the first HS bulk-IN on this xHCI, and it never completes: `xfers=0` from the first arm (trb=0, before any
  reset), while the OUT pipe on the same slot completes every DISCOVER (`tx done … cc=1`).
- **The kicks are doorbells.** `rx_stall_action` rung one = `ring_doorbell(slot, in_dci)`; rung two is the reset. The arm
  itself rings the IN doorbell after the push (`usbnet_data_pass`). Nothing is left un-rung.
- **in_state=3 is Stopped** (xHCI 1.2 §6.2.3 table: 0 Disabled, 1 Running, 2 Halted, 3 Stopped, 4 Error) — the state Set TR
  Dequeue leaves (§4.6.10) and the correct one; the next arm's doorbell restarts it (`stall … in_state=1` = Running, pending=1).
- **Cycle**: the arm line printed the ring's PCS after the push, not the TRB's own C bit; on a 16-TRB ring with ≤ 6 arms no
  wrap occurred, so the two agree on this wire — the new line prints both.
- **kept=none** on every reset: the Stop on a Running endpoint left no moved bytes. What the wire CANNOT say is whether the
  controller fetched the armed TRB (its TR Dequeue past it → the completion event was lost) or never did (dequeue == the
  armed TRB → the device NAKed every IN at HS). That is the one read this arc adds, and the fork decides the next fix.

## The seam
Kernel driver (`drivers/xhci/usbnet.rs`, the controller half in `drivers/xhci/mod.rs`); no new file, no handler domain.

## Milestones
- M1 — the read. One line per arm (arms 1..8, the first after each reset, powers of two):
  `[usbnet] rx arm trbs=<idx> cycle=<trb C> pcs=<ring> ep_state=<name> hw_deq=<idx> doorbell=rung usb=<hs|ss>`; and per
  reset, after the Stop and before the Set TR Dequeue: `[usbnet] rx reset read armed=<idx> hw_deq=<idx>
  td=<untouched|consumed> stopped_ev=<ccN|none> ep_state=<name>`.
- M2 — the bound. 32 s of back-off with zero completions is a FAIL the driver says once:
  `[usbnet] rx dead resets=<n> xfers=0 td=<…> usb=<hs|ss> -> FAIL reason=<device-silent|event-lost>`; while no completion
  has ever arrived the back-off stays at its 8 s floor (it does not double to 32/128 s).
- M3 — the witness. `[usbnet] rx frame n=1 len=<n>` on the first delivered frame; `tests usbnetrx` →
  `:: USBNETRX: armed=1 doorbell=1 ep_state=running frames=<n> usb=<hs|ss> -> PASS ::` (SKIP reason=no-dongle).

## Witness (next flight, dongle after login, direct port)
`[usbnet] rx arm trbs=0 cycle=1 pcs=1 ep_state=… doorbell=rung usb=hs`, then either `[usbnet] rx frame n=1 len=<n>` →
`tx dhcp discover` → `:: USBNET7: rx_ok>0 … -> PASS ::` → `NETCLOCK … lease=` — or the fork line
`[usbnet] rx reset read … td=untouched|consumed` + `[usbnet] rx dead … -> FAIL reason=…`.

## Owed
The HS-specific cause itself: `td=untouched` (device NAKs at HS → the AX88179 HS RX side: QCTRL/MEDIUM at HS, against Linux
`ax88179_link_reset`) or `td=consumed` (the event is lost → the event-ring path for this DCI). One flight decides; R83 forbids
guessing a register write before it.
