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

## Result (M4)

- `5817bf49` M1 — `unaos/libs/sys/usbnet_core` (CHARTER: Kernel — shared-core), root-workspace member;
  `cargo test -p usbnet_core` exit 0, 9 KATs (AX88179: flight-22 one-frame-plus-dummy shape, 3-frame 8-byte
  stride, CRC/drop flags skip one packet only, no-IPE, refusals, trailer fields; RTL8153: 2 frames with
  descriptor + FCS stripped, zero tail and runts; and an AX completion read as RTL disagrees).
- `c90e37d8` M2 + M3 — `drivers/xhci/usbnet.rs` (`deliver_ax` on the core, `rx0` dump, `rxlog`, `rx_kick_due`,
  `usbnet7_selftest`), `drivers/xhci/mod.rs` (one same-line `else if` kick in `usbnet_data_pass`; tail
  `usbnet_stall_probe`), `smolnet.rs` tail `usbnet7_poll`, `tests.rs` same-line `usbnet7` registration, the
  kernel's `usbnet` feature now `["dep:usbnet_core"]`. No new knob, no new verb, no new dotfile.
- Legs: x86 metal shape + `smolnet,selfdiag,ahciroot,btc` `cargo check` exit 0; aarch64
  `login,loginst,virt_el0,usbnet` exit 0; charter-check exit 0.

What the next flight reads (boot with the dongle in, then `tests usbnet7`):

```
[usbnet] rx0 len=<n> hdr=0x<off><cnt> pkt_cnt=2 hdr_off=<n> bytes=<48 bytes hex> tail=<16 bytes hex>
[usbnet] rx kick n=1 pending_ms=<n> xfers=<n> rx_ok=<n>          (only if a TD stalls past 2 s)
:: USBNET7: rx_ok=<n> frames=<n> first_frame_ms=<n> ethertype=0x0800 dhcp=offer -> PASS ::
```

On FAIL: `[usbnet] rxlog n= lens=[…] ms=[…] last_ms= tx_last_ms= now_ms= kicks= tx_stuck=` and
`[usbnet] stall portsc= ccs= ped= pls= speed= in_state= out_state= pending=`. Reading them: kicks>0 and RX
resumed after (`n` grows past the stall) = a missed doorbell, the kick is the fix; `pls` not 0 (U0) = the USB3
link left U0 (power management) — next arc is the port's U1/U2 handling; `pls=0`, endpoint Running, kicks
without effect, `last_ms` near `tx_last_ms` = the AX88179 itself stopped delivering — next arc is the chip's RX
path (the QCTRL / PAUSE / CLK_SELECT readbacks that disagree with their writes).
