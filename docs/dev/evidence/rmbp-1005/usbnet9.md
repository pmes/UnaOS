# USBNET9 (rmbp-ledger B385) — the RX ladder stops eating the wire; the dongle behind a hub is claimed

## Finding (read from the wire and the code, not the row)
- **The ladder fires on an idle clock.** `rx_stall_action` (usbnet.rs) arms rung one 2 s after the IN TD's arm and rung
  two 2 s after that, with no other condition: flight 24 card 3 shows `rx kick` / `rx reset` every ~4 s and nearly every
  `rx resumed … needed=0` (the data came long after the rung — a quiet LAN, not a missed doorbell; 114 idle rungs at
  `USBNET7`). Rung two is a Stop Endpoint + Set TR Dequeue: `note_rx_reset` then `reset_arm()`s unconditionally, which
  (a) throws away a real completion (cc 1/13) that raced the Stop, and (b) throws away the bytes of a Stopped (cc 26)
  TD that was mid-burst — and the rest of that AX88179 aggregate then lands in the next TD without its head.
- **The wire never carried an OFFER.** Driver census across five minutes: `dhcp_replies=0` beside 16 IPv4 frames and an
  ARP reply to our MAC in 10 ms (`txprobe … reply_ms=10`): unicast RX to our MAC works, TX reaches the LAN. The resets
  cost at most the frames of a window; they cannot explain zero of ~45 OFFERs. So the DISCOVER itself goes on the wire
  (M3) and the next flight compares it with RFC 2131 §4.3.1 / §4.1 (checksums verified in-kernel).
- **NODONGLE is not the hub's TT.** `enumerate_downstream` (xhci/mod.rs) knows three kinds behind a hub — hub, mass
  storage, HID — and the USB-net hooks (`note_device`, the walk, `usbnet_after_walk`) exist only on the root-port async
  path. A dongle behind ANY hub is read, finds no HID endpoint, and is left unconfigured (`XHCIHUB … downstream=2`, one
  claim). Direct: the root path claims it. Flight 25 boot 2 "alone on the hub, enumerated" was `USBNET-EHCI` (the EHCI
  scout before the port flip), not xHCI; the tests then said `no-dongle`. The stick beside it is irrelevant.
- The AX88179 exposes no RX-FIFO-level register in Linux `ax88179_178a.c` (R83: nothing invented); "a frame is
  expected" is therefore an ANSWER OWED: a solicited TX (ARP request, DHCP to 67, DNS, ICMP echo, NTP, TCP SYN) issued
  after the last RX completion.

## The seam
Driver-internal (`drivers/xhci/usbnet.rs` + the USBNET tail of `drivers/xhci/mod.rs`); smoltcp stays the stack (R85).
No new file, no knob (the arc rides `usbnet`), no verb.

## Milestones
- **M1 — the ladder runs only when an answer is owed.** Rung one (doorbell kick, harmless on a Running endpoint,
  xHCI 1.2 §4.7) keeps its cadence for USBNET8's `needed=` measurement. Rung two fires only when: the IN TD is pending,
  an answer is owed for ≥ 2 s, a kick fired after the owe began and drew nothing, and the back-off since the last reset
  has passed (4 s doubling to 256 s). On a quiet link: `[usbnet] rx idle quiet_s=<s> pending=1 owed=0 kicks=<n>` at most
  once a minute, and never a reset.
- **M2 — a reset loses nothing.** After the Stop: a real completion that raced it is KEPT (delivered by the next pass);
  a Stopped TD's bytes (MPS multiple) are CARRIED — the fresh TD is posted at the buffer offset after them so the burst
  completes contiguously and is delivered whole. `[usbnet] rx reset n= … owed_ms= kept=<none|complete|carry:N|lost:N>`.
- **M3 — the DISCOVER on the wire.** `[usbnet] tx dhcp discover xid=<x> len=<n> ipsum=<ok|bad> udpsum=<ok|bad> bytes=<first 64> opts=<options>`
  (first two and every power of two), and the first server reply `[usbnet] rx dhcp from=<ip> op=<2> type=<2 offer|5 ack|6 nak> xid=<x>`.
- **M4 — NODONGLE.** `enumerate_downstream` hands a non-storage, non-hub device to `usbnet_downstream` (sync twin of the
  root walk: device descriptor → `note_device`, full config walk → `note_config_header`/`note_interface`/`note_bulk_ep`/
  `note_cs`, other configuration, `taken` + `configure_bulk_endpoints_sync` + `configured`); bring-up is the root
  path's. Witness per hub walk: `[xhci] hub slot=<n> ports=<n> enumerated=[<port>:<kind>@<slot>,…]`.

## Witness lines the next flight reads
`USBNET7 … dhcp=offer … resets=0 -> PASS`, `NETCLOCK … lease=ok -> PASS`; on a quiet link `[usbnet] rx idle …` and no
`rx reset`; if no OFFER, `[usbnet] tx dhcp discover …` (compare with RFC 2131 §4.3.1) and no `[usbnet] rx dhcp`; on
the hub `[xhci] hub slot=… enumerated=[…:net@…]` then `:: USBNET: candidate … ::` / `:: USBNET: up … -> PASS ::`.

## Owed
Hot-plug of a dongle behind a hub uses the same `enumerate_downstream` (covered); a dongle on the hub at the EHCI
side (pre-flip) stays the EHCI scout's. A chip-side RX-pending register, if ASIX documents one, would replace "owed".
