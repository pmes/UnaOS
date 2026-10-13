# DHCPREPLUG (rmbp-ledger B632): the dongle's DHCP silence, read from our side

Source: `docs/dev/evidence/rmbp-0915/flight34/f34-boot1.log`, `f34-boot2.log` (flight 34, image 27b).

## 1. The re-plug: the back-off DID reset, and the B538 line misreported it

Boot 1: `[usbnet] link up` 15:01:21, discovers n=1,2,4,8 (B538 prints only the first two and the powers of two),
`:: USBNET: link down slot=2 (disconnect)` 15:04:05 (`disconnect` -> `ud_reset`, which zeroes `UD_RETX` and `UD_NEXT`),
`link up` 15:17:58, next printed `discover n=16` 15:20:35. n=9..15 were sent but not printed. Starting from the reset
(1 s settle, then 4, 8, 16, 32, 32, 32, 32 s): n=9 at 15:17:59, and n=16 at 15:17:59 + 4+8+16+4x32 = 156 s =
**15:20:35**, exactly the logged time. So the client restarted at the re-plug. The `retx_ms=16000` on n=8 and n=16 was
`UD_RETX/2`, i.e. half of the NEXT interval, which at the 32 s cap reads 16000 while the real interval is 32000.
DHCPREPLUG prints the interval the DISCOVER actually waits, and prints the first DISCOVER after every restart with
`after_link_up_ms=`.

What still needed fixing: a link that drops and returns WITHOUT a USB disconnect (a cable pull at the dongle) never
went through `ud_reset`, and even after one, the first DISCOVER waited the 1 s settle. Now `link_seen`'s up edge calls
`ud_link_up`: new xid, back-off 4000, DISCOVER 250 ms after link, both link probes armed again.

## 2. Our DISCOVER, as sent on flight 34 (the u9 dump, `tx dhcp discover n=1`)

`ffffffffffff 9c69d3286ef4 0800 | 45 00 0148 0000 4000 40 11 39a6 00000000 ffffffff | 0044 0043 0134 319d | 01 01 06 00 f0338380 0000 8000 …`, `opts=3501013703010306ff`

| field | sent | RFC 2131 §4.4.1 / 2132 |
|---|---|---|
| Ethernet dst / src / type | ff:ff:ff:ff:ff:ff / 9c:69:d3:28:6e:f4 / 0x0800 | ok |
| IPv4 ver/IHL, TOS, total length | 0x45, 0x00, 328 | ok |
| IP id, flags, TTL, protocol | 0, DF (0x4000), 64, 17 | ok (DF legal) |
| IP checksum | 0x39a6, verifies (`ipsum=ok`) | ok |
| IP src / dst | 0.0.0.0 / 255.255.255.255 | ok |
| UDP ports / length / checksum | 68 -> 67 / 308 / computed, verifies (`udpsum=ok`) | ok |
| op / htype / hlen / hops | 1 / 1 / 6 / 0 | ok |
| secs / flags | 0 always / 0x8000 BROADCAST | flag ok; secs now counts from the first DISCOVER |
| ciaddr / chaddr | 0.0.0.0 / our MAC (= Ethernet src) | ok |
| magic cookie | 63 82 53 63 | ok |
| options | 53=1, 55=[1,3,6], 255; BOOTP 300 octets | legal; no 61 / 12 |

Verdict: the flight-34 DISCOVER was RFC-valid. The BROADCAST flag was set, so (2d) of the brief (flag clear, the
OFFER unicast) is NOT the bug: a compliant server broadcasts its OFFER to this request. DHCPREPLUG adds 61 (type 1 +
MAC), 12 `unaos`, 55 = [1,3,6,15,28,51,58,59] and `secs`, the same in the REQUEST, so the frame now matches what
servers that key on client-id or host-name expect.

Differences from a Linux ISC dhclient 4.x DISCOVER (Debian default `dhclient.conf`, from knowledge, not a capture on
Peter's LAN): dhclient sends IP TOS 0x10, no DF, TTL 128 (ours 0x00, DF, 64); BOOTP flags 0x0000 (ours 0x8000:
dhclient's raw socket takes a unicast OFFER); `secs` = elapsed seconds (ours now the same); no option 61 (ours sends
it); 12 = the host name (ours `unaos`); 55 with 13 codes 1,28,2,3,15,6,119,12,44,47,26,121,42 (ours 8); option 50 when
it remembers a lease (ours never). None of these is a reason for a compliant server to stay silent.

## 3. What the LAN let through (the RX filter)

AX88179 RX_CTL as programmed `RX_CTL_OPEN` 0x028a: START 1, AB (broadcast) 1, PRO 0, AMALL 0. Boot 1 after the re-plug,
first 64 frames: `bcast=6 mcast=57 ours=1 foreign=0 udp67=0` — the broadcasts are ARP who-has; broadcast reception
works. Boot 1's FIRST link received nothing at all (`rx dead … reason=device-silent`, `link down … rx=0 tx=8`), so the
first eight DISCOVERs went out while the IN pipe was dead. The re-plugged link heard 64 frames in 2 minutes and no
UDP 67/68 frame of anyone's.

## 4. What the next flight reads

- `[dhcp] link-up -> restart xid=<x> backoff=4000 discover_in_ms=250 was=<state> old_xid=<x> restarts=<n>` and the
  `[dhcp] rx-filter at=link-up kind=ax88179 rxctl=0x028a start=1 ab=1 pro=0 amall=0 am=0 …` row after it;
- `[dhcp] discover n=<k> … secs=<s> … retx_ms=4000 … after_link_up_ms=<about 250>`;
- once per boot, two `[dhcp] tx-audit` rows: `eth dst=ff:ff:ff:ff:ff:ff … ip ver_ihl=0x45 tos=0x00 len=328 id=0
  frag=0x4000 ttl=64 proto=17 … csum_ok=1 src=0.0.0.0 dst=255.255.255.255 ; udp 68->67 len=308 … udpsum=ok` and
  `bootp op=1 htype=1 hlen=6 hops=0 … flags=0x8000 ciaddr=0.0.0.0 chaddr=<mac> chaddr_is_src=1 cookie=ok
  bootp_len=300 ; opts=53,61,12,55 type=discover client_id=ok host=unaos params=1,3,6,15,28,51,58,59 end=ok ->
  rfc2131 fail=none`;
- `[dhcp] rx n=<k> <ip>:<port> -> <ip>:<port> from=<mac> dst=<class> op= type= xid= ours= flags= yiaddr= chaddr=
  chaddr_is_us= server= lease_s= …` for EVERY UDP 67/68 frame heard (anyone's OFFER names the server and its lease);
- `[dhcp] tx-size probe len=342 who_has=<peer> reply_ms=<n> -> large-tx ok` or `reply_ms=none -> large-tx lost`:
  the 60-byte probe proved a minimum frame reaches the LAN; this tests a frame the DISCOVER's size.

Reading: `large-tx lost` puts the fault in our TX of a 342-byte frame (the AX TX header or the bulk-OUT for frames
over the minimum); `large-tx ok` with no `[dhcp] rx` row of any kind says the server got a valid broadcast DISCOVER
and stayed silent, or its OFFER did not pass our filter — a `[dhcp] rx` row of ANOTHER client's exchange separates
those two.
