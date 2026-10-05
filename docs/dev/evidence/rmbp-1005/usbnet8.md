# USBNET8 — rmbp-ledger B381 (branch exec-rmbp-usbnet8, cut from a60219de)

CHARTER: Kernel — driver (no new kernel file: `drivers/xhci/ring.rs`, `drivers/xhci/usbnet.rs`, the usbnet tail of
`drivers/xhci/mod.rs`); `unaos/libs/sys/usbnet_core` stays Kernel — shared-core (KATs only).

## Finding — read from the code and the wire (flight 23, `rmbp-0915/flight23/f23-boots.log`, awk), not guessed (R83)

**(1) "Every completion needs a kick" is NOT what the wire says.** The kick/reset lines are rate-limited (n = 1, 2, then
powers of two) but `rx resumed` prints EVERY time (it fires only on the first completion after a rung). Boot 1:
`rx_xfers=52` at the census, and exactly NINE `rx resumed` lines — so 43 of 52 completions arrived on their own with no
rung before them (xfers 0 -> 29 between kick 1 and kick 2 in 11 s; 29 -> 32 between kick 2 and reset 1). Boot 2 the same
(xfers 0 -> 30 before kick 2). The ladder fires on every 2 s of SILENCE (82 kicks, 74 resets in six minutes) and the
"resumed" completions arrive 2 s or more after their kick (09:09:31 kick -> 09:09:33 resumed; most after a reset), which
is a quiet LAN's next broadcast, not a doorbell answered at once. The only "kick then frame in the same second" pair in
each boot is the very first TD, armed at bring-up 3 s BEFORE link-up, kicked in the same pass the PHY reported link.
The row's arm-path hypotheses, checked against the code and xHCI 1.2:
- cycle bit last (§4.9.1): `TransferRing::produce` sets the cycle in `control`, writes the TRB, `fence(SeqCst)`, then the
  doorbell (`ring_doorbell_asm`: fence, MMIO write, fence). BUT the TRB is ONE `write_volatile` of a 16-byte
  `repr(packed)` struct: the order of its constituent stores is the compiler's, so the cycle-bearing dword is not
  PROVABLY last. Linux `queue_trb` writes dwords 0..2, `wmb()`, then dword 3. Harmless on a controller that only fetches
  after a doorbell, wrong against one that re-reads an idle ring slot. FIXED (M2), every ring (one producer path).
- Link TRB (§4.11.5.1): pre-placed at construction with TC=1, its cycle dword armed with the ending lap's colour AFTER
  the new lap's slot 0 is written (ONSET-3). Correct.
- 64 KiB boundary (§4.11.7.1): RX is `scsi_data_buffer` + 0, a 32 KiB buffer allocated 64 KiB-ALIGNED
  (`STORAGE_DATA_ALIGN`), 20480 B — it cannot cross. No rotating buffers (one TD outstanding). Correct; now on the wire
  (`cross64k=`).
- ERDP (§4.9.4): `drain_event_ring_once` advances ERDP with EHB after every event. Correct.

**(2) The delivery is NOT broken either.** `note_ethertype` (the census) runs only after `RXQ.push` succeeded, and
`census arp=2 ipv4=33 ipv6=24 rx_ok=59 rx_drop=0`: all 59 frames reached the ring the stack drains, with real
ethertypes — the 2-byte IPE pad IS applied (`split(buf, RX_PAD=2, ..)`; otherwise every ethertype would read the
source MAC's bytes). `frames=0` / `first_frame_ms=none` are the `tests usbnet` FIXTURE's own 3-5 s window seeing no frame
on a quiet wire (and that fixture pops frames itself, away from the stack). What the wire actually lacks is ONE frame
kind: `dhcp_replies=0` — not one UDP datagram from port 67 reached the ring in six minutes of DISCOVERs (tx=58
completions). Nothing on the wire says whether our TX ever reaches the LAN: no frame we sent has ever drawn an answer.

## Seam

None new. The framing stays `usbnet_core` (shared-core); the arm order is the one xHCI producer (`ring.rs`), fixed once
for every ring. smoltcp stays with the NetStack seam open (R85).

## Milestones

- **M1** this finding.
- **M2** `ring.rs::write_trb`: the parameter and status dwords first, `fence(SeqCst)`, then the control dword (the one
  carrying the cycle bit) alone, then the line clean (Linux `queue_trb` order). The arm prints
  `[usbnet] rx arm trb=<idx> cycle=<c> buf=<pa> len=<n> cross64k=<0|1>` at arms 1, 2 and every power of two.
- **M3** the ladder says what it found: a rung answered by a completion within 50 ms is a NEEDED rung (a missed
  doorbell's signature: the data was there); later than that it was silence. `rx resumed … after_ms=<n> needed=<0|1>`;
  the `tests usbnet` verdict becomes `:: USBNET7: rx_ok=<n> first_frame_ms=<n> kicks=<needed kicks>
  resets=<needed resets> idle_rungs=<n> -> PASS|FAIL ::` and PASS also needs `kicks=0 resets=0`.
- **M4** host KATs in `usbnet_core` with flight 23's two `rx0` captures, through the split to the Ethernet header
  (MLD report: dst 33:33:00:00:00:16, ethertype 0x86dd, 110 B; mDNS: dst 01:00:5e:00:00:fb, ethertype 0x0800).
- **M5** the TX question, under `tests usbnet7` only (R80): an ARP probe (RFC 5227, sender IP 0.0.0.0) for the last
  IPv4 source the census saw, and the answer: `[usbnet] txprobe arp who_has=<ip> sent=<0|1> reply_ms=<n|none>`.
  reply = our TX reaches the LAN and the DHCP silence is the DISCOVER itself; none = TX does not reach the wire
  (next arc: the OUT endpoint / MEDIUM EN_125MHZ whose readback drops it, `0x013f/0x0133`).

## Witness (metal; boot with the dongle in, then `tests usbnet` and `tests usbnet7`)

```
[usbnet] rx arm trb=0 cycle=1 buf=0x<pa> len=20480 cross64k=0
[usbnet] rx resumed len=<n> kicks=<n> resets=<n> after_ms=<n> needed=0
:: USBNET7: rx_ok=<n> first_frame_ms=<n> kicks=0 resets=0 idle_rungs=<n> -> PASS ::
[usbnet] txprobe arp who_has=10.0.1.<x> sent=1 reply_ms=<n>
:: NETCLOCK: … lease=ok … -> PASS ::
```

## Owed

The lease itself is not proven fixed — nothing in the arm path or the delivery was found broken by reading, so this arc
makes the next flight DECIDE: `needed=1` rungs = a real missed doorbell (then the M2 order is the first suspect, already
fixed); `txprobe … reply_ms=none` = TX never reaches the LAN (the DHCP silence explained, next arc on the OUT side).
