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
- **M3** the ladder says what it found (and a TD armed before link-up starts its stall clock at link-up, so the bring-up TD is not kicked at the instant frames begin): a rung answered by a completion within 50 ms is a NEEDED rung (a missed
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
[usbnet] rx arm trb=0 cycle=1 buf=0x<pa> len=20480 cross64k=0 n=1            (arms 1, 2 and every power of two)
[usbnet] rx resumed len=<n> kicks=<n> resets=<n> after_ms=<n> needed=0        (needed=1 = a missed doorbell, the data was waiting)
:: USBNET7: rx_ok=<n> first_frame_ms=<n> kicks=0 resets=0 idle_rungs=<n> ethertype=0x.... window_ms=<n|none> -> PASS ::   (tests usbnet)
:: USBNET7: rx_ok=<n> frames=<n> first_frame_ms=<n> ethertype=0x0800 dhcp=offer kicks=0 resets=0 idle_rungs=<n> -> PASS ::  (tests usbnet7)
[usbnet] txprobe arp who_has=10.0.1.<x> sent=1 reply_ms=<n> tx=<n>            (tests usbnet7)
:: NETCLOCK: … lease=ok … -> PASS ::
```

`kicks=`/`resets=` on the USBNET7 lines count NEEDED rungs only; `idle_rungs=` the rungs that fired on a quiet wire
(the NETFRAME `rx kick`/`rx reset` lines keep their totals). `first_frame_ms` on the `tests usbnet` line is now the
link's (first arm to the first frame of the boot), `window_ms` the fixture's own.

## Owed

The lease itself is not proven fixed — nothing in the arm path or the delivery was found broken by reading, so this arc
makes the next flight DECIDE: `needed=1` rungs = a real missed doorbell (then the M2 order is the first suspect, already
fixed); `txprobe … reply_ms=none` = TX never reaches the LAN (the DHCP silence explained, next arc on the OUT side).

## Result

- M1 `7472e6d8` this finding. M2 `ring.rs::write_trb` writes parameter + status, `fence(SeqCst)`, then the control dword
  (every ring: the one producer path); the arm line. M3 needed/idle rung split, `after_ms= needed=` on `rx resumed`,
  the stall clock of a TD armed before link starts at link-up, both USBNET7 lines carry `kicks= resets= idle_rungs=`.
  M4 two flight-23 KATs (`cargo test -p usbnet_core` exit 0, 11 tests). M5 the ARP txprobe under `tests usbnet7`.
- No new knob, verb, file or dotfile. Legs: x86 metal shape exit 0; aarch64 `login,loginst,virt_el0,lumen,
  desktop_firmware,quarry,facet,usbnet` exit 0; aarch64 `tegra,login,loginst,virt_el0` exit 0; charter-check exit 0.

## M7 / M8 — the seat's second dispatch (TX by reading; the fixture taps)

Seat rulings: (a) rung two stays this flight, `needed=` judges it; (b) 50 ms stands, and `rx resumed` now carries
`pass_ms=<longest data-pass interval since the rung>` when the answer came later than 50 ms and a pass took longer than
that (an undercount is visible); (c) the fixture taps.

**M7 — the TX side against Linux `drivers/net/usb/ax88179_178a.c` + `usbnet.c`:**
1. *TX header.* `ax88179_tx_fixup`: `tx_hdr1 = skb->len`, `tx_hdr2 = gso_size` (0) `| 0x80008000` when `(len + 8) %
   maxpacket == 0`, both `put_unaligned_le32`, 8 bytes pushed ahead. Ours matched. `usbnet_start_xmit`: the AX88179
   `driver_info` has no `FLAG_SEND_ZLP`, so an exact multiple of maxpacket gets ONE extra zero byte (a short packet ends
   the transfer); ours sent no byte and no ZLP — an exact-multiple frame (1016 B, 2040 B) would have left the OUT TD's
   transfer unterminated. Not the DISCOVER's case (342 + 8 = 350). Now the shared core's `usbnet_core::ax88179_tx::header`
   (KATs: 342 -> `56 01 00 00 00 00 00 00`, 350 bytes; 1016 -> pad flag, 1025 bytes), which the data pass calls.
2. *MEDIUM_STATUS_MODE* 0x013f written, 0x0133 read: the bits that do not stick are 0x0008 = `AX_MEDIUM_EN_125MHZ` and
   0x0004, which mainline names nothing (the `ALWAYS_ONE` name is not in mainline; I cannot verify the vendor driver
   here). 0x0133 is EXACTLY what Linux `ax88179_reset` writes before any link (RECEIVE_EN | TXFLOW_CTRLEN |
   RXFLOW_CTRLEN | FULL_DUPLEX | GIGAMODE). `ax88179_link_reset` (after link, from PHYSR) writes RECEIVE_EN | TXFLOW |
   RXFLOW, + GIGAMODE | EN_125MHZ for 1000, + FULL_DUPLEX = 0x013b. Our bring-up wrote 0x013f before link (now 0x0133);
   our link-up wrote PHYSR-derived 0x013f (now 0x013b, bit 2 dropped). Whether EN_125MHZ sticks after the Linux-order
   link-up is on the next wire (`reg MEDIUM_STATUS_MODE=0x013b/…`); at gigabit the MAC sources the TX clock (GTX_CLK), so
   a missing EN_125MHZ is a TX-only fault that fits "RX works, nothing we send is answered".
3. *ORDER after link-up.* Linux `ax88179_link_reset`: loop (<= HZ/10) { RX_CTL = 0; RX_CTL = rxctl; read 4 bytes,
   vendor request 0x81, wValue 0x8c — "check the usb device control TX FIFO full or empty" } while bit 30 is set; then
   PHYSR; QCTRL; MEDIUM. Ours: MEDIUM, QCTRL, RX_CTL — and the TX FIFO was never looked at. FIXED to Linux's order
   (`usbnet_ax_link_reset`), one line: `[usbnet] link reset order=linux rxctl=0x028a/<rb> txfifo=<hex> tries=<n> ms=<n>
   medium=0x013b ok=1`.
4. *RX_CTL* 0x03aa written, 0x02aa read: the bit that does not stick is 0x0100 = `AX_RX_CTL_DROPCRCERR`. 0x03aa
   (DROPCRCERR | IPE | START | AP | AB | AMALL) is Linux `ax88179_reset`'s own value; after open and on every link reset
   Linux writes `rxctl` = START | AB | IPE (+ AM/AMALL/PRO from `set_rx_mode`). Link-up now writes 0x028a (AMALL in place
   of AM + the hash filter this driver does not program). (`AP` is 0x0020, not promiscuous; `PRO` 0x0001 is.)
5. *TX on the wire:* `[usbnet] tx done n= len= cc= residual=` for the first three OUT TDs and every non-success
   (cc 0 = abandoned by the stuck valve), plus M5's ARP probe.

**M8 — the fixture taps.** `tests usbnet` drove `raw_rx` itself and kept up to 4 frames for up to 5 s. It now runs the
main pass + the stack poll and counts frames as they are pushed. Flight 23 check: the fixture ran 09:15:09–09:15:14
(`USBNET6` at 09:15:14); the DHCP tries (2 s each, `DHCP_WAIT_MS`, on the 5 s net tick) ended 09:14:16 (link tries
exhausted) and the next started on the 09:15:26 re-arm — NO overlap, and the shell holds the main loop during `tests`, so
no try could run inside the window. The fixture did not steal an offer on flight 23; there was none to steal
(`dhcp_replies=0` counts at the push, before any pop).

Wire added for flight 24:
```
[usbnet] reg MEDIUM_STATUS_MODE=0x0133/<rb>                                   (bring-up, Linux reset value)
[usbnet] reg MEDIUM_STATUS_MODE=0x013b/<rb>                                   (link-up, from PHYSR)
[usbnet] link reset order=linux rxctl=0x028a/0x028a txfifo=0x........ tries=1 ms=<n> medium=0x013b ok=1
[usbnet] tx done n=1 len=<n> cc=1 residual=0
[usbnet] rx resumed len=<n> kicks=<n> resets=<n> after_ms=<n> needed=0[ pass_ms=<n>]
```
Reading: `txfifo` bit 30 set after 100 ms = the dongle's TX FIFO is stuck (TX never leaves the chip); `tx done cc=1
residual=0` + `txprobe reply_ms=none` = the controller delivered every byte and the dongle did not put it on the LAN
(MEDIUM readback without 0x0008 points at EN_125MHZ); a reply = TX works and the DHCP silence is the DISCOVER itself.
