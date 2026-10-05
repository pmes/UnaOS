# NETFRAME — rmbp-ledger B368 (branch exec-rmbp-netframe)

## Finding (the wire, flight 22, `docs/dev/evidence/rmbp-0915/flight22/f22-boots.log`)

- The dongle is `vidpid=0b95:1790` = **ASIX AX88179** (`kind=ax88179`, SuperSpeed, bulk IN 0x82 mps 1024).
  The RTL8153 leading-descriptor framing does NOT apply to this part.
- Bulk-IN buffer: `rx buf=20480` = 1024 x (QCTRL[3]+2) with QCTRL[3]=0x12, the SuperSpeed size the chip
  wants (>= 20 KiB). Written `RX_BULKIN_QCTRL=07 4f 00 12 ff`, read back `07 65 00 09 ff` (a readback that
  disagrees with the write; so do CLK_SELECT, PAUSE_WATERLVL_*, PHYPWR_RSTCTL — the "UNFLOWN register map"
  note stands; the 20 KiB buffer is >= either reading's size, so the buffer is not short).
- **The framing parse is NOT the fault.** The census at the `tests usbnet` run reads
  `[usbnet] census arp=0 ipv4=25 ipv6=10 dhcp_replies=0 rx_pad=35 rx_ok=35`: each of the 35 completions held
  exactly one real packet header plus one alignment dummy header (rx_pad=35), and every delivered frame carried
  a valid ethertype (25 IPv4, 10 IPv6). A wrong framing would have produced `rx raw … reason=` / random
  ethertypes; none printed.
- **The fault is that RX STOPS.** `rx_xfers=35 arms=36 pending=1 ep_state=1` ten minutes after link-up: the 36th
  bulk-IN TD is posted, the endpoint is Running, no error event ever came (`errors=0`), and the 5 s fixture —
  which drives the controller itself (its own `ring` dump claimed the loan, so `drive()` was not starved) —
  saw no completion. On a home LAN (ARP, mDNS, the router's DHCP replies to our 6 DISCOVERs) 35 frames in
  ten minutes is a stall, not a quiet wire. Also at 04:55:01 the stack's `sendto` started returning `ok=0`
  (TX ring full / OUT TD stuck) — both directions stalled near the same moment.
- The earlier flight (20) received 0; USBNET7 (first-bulk endpoint pick) + NETCLOCK made RX move at all.
- No DHCP offer (`SOCK-5 … no offer` x6 then the link tries are exhausted, `DHCP_LINK_MAX=6`) and therefore
  `[sntp] target=0.0.0.0` follow from the stall, not from the parse.

## Seam

`unaos/libs/sys/usbnet_core` — CHARTER: Kernel — shared-core. A `no_std`, `forbid(unsafe)`, zero-dependency
framing core: the AX88179 trailing header block and the RTL8153 leading 24-byte rx descriptor, split into
`(offset, length)` frame views with per-packet verdicts. The kernel's `drivers/xhci/usbnet.rs` calls it
(behind `usbnet`, a `dep:`), the host runs the KATs (`cargo test -p usbnet_core`). One implementation.

Datasheet facts used (data only): AX88179 — the last 4 bytes of a bulk-IN transfer are `pkt_cnt` (low 16) and
`hdr_off` (high 16); at `hdr_off` sit `pkt_cnt` little-endian u32 packet headers, length in bits 16..28,
CRC error bit 29, drop bit 31; packets start at 0, each padded to 8 bytes; with RX_CTL.IPE every packet
begins with 2 alignment bytes; a zero-length header is the alignment dummy. RTL8153 — each packet is a 24-byte
descriptor (opts1 bits 0..14 = length INCLUDING the 4-byte FCS) then the frame, the next descriptor at the
8-byte-aligned offset. (The row said "8-byte descriptor"; the RTL8153's rx descriptor is 24 bytes.)

## Milestones

- **M1** — this finding; `usbnet_core` with both framings and host KATs (constructed buffers incl. the flight-19
  dummy header `0x80008000`, a flight-22-shaped one-packet completion, CRC/drop flags, bad geometry, RTL
  multi-packet with FCS).
- **M2** — the wire shows the framing and the stall regardless: `[usbnet] rx0 len= hdr= pkt_cnt= hdr_off=
  bytes=… tail=…` (first completion, once); a completion log (`[usbnet] rxlog n= lens=[…] ms=[…] last_ms=
  now_ms= tx_last_ms= kicks=`), and on a stall the dongle's PORTSC (link state) and both endpoint states.
- **M3** — `deliver_ax` on `usbnet_core` (same verdicts, same counters); an RX stall kick — a TD pending
  > 2 s with link up re-rings the IN doorbell (xHCI: harmless on a Running endpoint, recovers a missed
  doorbell), counted `kicks=`; `tests usbnet7`: drives the main pass + the stack for up to 10 s (and re-arms
  one DHCP try) and prints `:: USBNET7: rx_ok=<n> frames=<n> first_frame_ms=<n> ethertype=0x0800 dhcp=offer
  -> PASS ::` (PASS = frames>0, an IPv4 frame seen, and a DHCP server reply or a lease).
- **M4** — this doc's result section; the ledger status text for the seat.

## Witness (metal)

`tests usbnet7` →
`:: USBNET7: rx_ok=<n> frames=<n> first_frame_ms=<n> ethertype=0x0800 dhcp=offer -> PASS ::`; on FAIL the next
lines are `[usbnet] rxlog …` and `[usbnet] stall portsc= pls= in_state= out_state= kicks= …`, which say
whether the device stopped (port U0, endpoint Running, kicks did nothing) or the controller did.

## Owed

The stall's root cause is unproven (R78: metal only). The kick is a recovery candidate, not a proven fix. The
RTL8153 parser is linked but not wired: this driver reaches an RTL8153 only through its CDC-ECM
configuration, which has no descriptor framing.
