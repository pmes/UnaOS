# USBNET — the USB Ethernet link, CDC-ECM front-end, QEMU proof (LEDGER SO56; SO55 found on the way; 2026-09-24)

Cloud session, QEMU 8.2.2 TCG (`UNAOS_QEMU_MACHINE=pc-q35-8.2`), no bench. Serial logs read with
`awk 'index($0,"<tag>")'`. Tree: `claude/optimistic-ramanujan-r3qyu5`. Type-check with the feature:
x86 (`wc,smolnet,usbnet,witness,login`) rc=0, aarch64 (`…,virt_el0,login,net6,usbnet`) rc=0, no
warning in the new code.

## The dongle-only leg (spec `x86-usbnet.spec`)

`UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_QEMU_FULL=1 UNAOS_USBNET=1 UNAOS_NOE1000=1 ./arroyo test 120`

### Run A — first build: the request loop (fixed)
The class-0x02 arm re-requested the configuration descriptor forever (988,143 serial lines, run
TRUNCATED): the dispatch chain runs on the configuration descriptor's completion too, where byte 4 is
`bNumInterfaces` (2 for this device), and the helper answered `true` for it. Gated on the descriptor
type (`desc_data[1] == 0x01`). The stock arms (0x08 storage, 0x09 hub) have the same exposure and are
noted in SO55, not changed.

### Run B — link up, stack starved (fixed)
```
:: USBNET: up slot=3 cfg=1 ctrl=0 data=1 alt=1 mac=40:54:00:12:34:57 filter=ok -> PASS ::
:: USBNET: rx=5974 tx=26794 rx_drop=0 tx_drop=45322 errors=0 ::
:: SOCK-1: smoltcp icmp echo 10.0.2.2 0/4 replies — witness INCOMPLETE ::
:: SOCK-5: smoltcp dhcpv4 no offer — static fallback stands 10.0.2.15/24 gw 10.0.2.2 — witness INCOMPLETE ::
```
The link was up and moving frames while every stack witness starved: smolnet's pumps spin on `raw_rx`
with the STACK lock held (the e1000's RX ring is readable from any context; a USB link's frames only
move on the xHCI pass). Fix: `usbnet::drive` — the accessors try-claim the controller loan and run
one `poll_events` + `service_usbnet` before answering (network_stack.md §10).

### Run C — the proof (rc=1 from the container's SOCK-3 only)
```
:: USBNET: configuration 0 holds no ECM pair (rndis_seen=1) — requesting configuration index 1 of 2 ::
:: USBNET: ecm candidate slot=3 cfg=1 ctrl=0 data=1 alt=1 imac=3 mss=0 in_mps=64 out_mps=64 ::
:: USBNET: up slot=3 cfg=1 ctrl=0 data=1 alt=1 mac=40:54:00:12:34:57 filter=ok -> PASS ::
:: USBNET: rx=174 tx=338 rx_drop=0 tx_drop=1 errors=0 ::
:: SOCK-1: smoltcp icmp echo 10.0.2.2 4/4 replies — witness OK ::
:: SOCK-2: smoltcp udp dns query 10.0.2.3:53 -> 64 bytes back — witness OK ::
:: SOCK-3: smoltcp tcp connect 10.0.2.3:53 REFUSED, 0 bytes back — witness INCOMPLETE ::
:: SOCK-6: smoltcp tcp listen :8080 armed — awaiting inbound connect (UNAOS_NET=socket injector) — witness PENDING ::
:: SOCK-7: persistent listener :8080 armed — awaiting a SECOND inbound connect (survives accept) — witness PENDING ::
:: SOCK-5: smoltcp dhcpv4 lease 10.0.2.30/24 gw 10.0.2.2 — witness OK ::
:: SOCK-2: ring-3 udp sockets — sys_socket(40)/bind(41)/sendto(42)/recvfrom(43), a datagram round-trip over the persistent smoltcp stack ::
:: SOCK-2: ring-3 udp round-trip — socket/bind/sendto OK, recvfrom returned a datagram FROM 10.0.2.3:53, socket teardown clean -> PASS ::
:: SOCK-3: ring-3 tcp sockets — sys_socket(SOCK_STREAM)/connect(44)/send(45)/recv(46), a byte-stream round-trip over the persistent smoltcp stack ::
:: SOCK-3: ring-3 tcp round-trip FAIL — witness=0x0 cleared=false killed=0 done=0 (want 0x1f/true/0/1) ::
:: SOCK-4: transferable sockets — SYS_XFER moves a KIND_SOCKET cap cross-row (owner migrates), the grantee round-trips it, the grantor's stale handle is rejected ::
:: SOCK-4: transferable sockets — grantee received + round-tripped the moved socket, grantor's migrated-away handle -EACCES, gen-rebind rejected, teardown clean -> PASS ::
```

Enumeration of the device (the two configurations, RNDIS then ECM):
```
xHCI: Device Found. Class=0x2 Sub=0x0 Proto=0x0
xHCI: Configuration Descriptor Total Length: 67
xHCI: Interface: Class=0x2 Sub=0x2 Proto=0xff
xHCI: Interface: Class=0xa Sub=0x0 Proto=0x0
xHCI: Configuration Descriptor Total Length: 80
xHCI: Interface: Class=0x2 Sub=0x6 Proto=0x0
xHCI: Interface: Class=0xa Sub=0x0 Proto=0x0
xHCI: Interface: Class=0xa Sub=0x0 Proto=0x0
xHCI: >>> BULK IN EP FOUND: 0x82, MPS: 64 <<<
xHCI: >>> BULK OUT EP FOUND: 0x2, MPS: 64 <<<
```

Spec replay: `python3 scripts/mbench.py --replay <serial> --spec scripts/specs/x86-usbnet.spec --platform x86`
    ════════════ MBENCH VERDICT — x86-usbnet.spec vs /tmp/claude-0/-home-user-UnaOS/823e4b66-c41d-5863-8c5e-22382d4804ac/scratchpad/usbnet
      ✅ MBENCH PASS — 9/9 required witnesses, 0 forbidden hit(s), 2764 lines scanned [mode unknown: no run sidecar]

Reading: `10.0.2.30` is the `dhcpstart` of the usb-net's own user netdev (n1); the e1000e's netdev
would have leased 10.0.2.20 and the e1000e is not attached (`[e1000] up:` FORBID, 0 hits). The MAC
`40:54:00:12:34:57` is the address the device reports in its iMACAddress string (the builder's
`52:54:00:12:34:57` with the locally-administered bit set by QEMU for the device's end of the link).
`SOCK-3` (tcp connect 10.0.2.3:53) is REFUSED in this container with the e1000e too (LOGIN14.md,
HDASIE.md) — environmental. Its ring-3 twin read `witness=0x0 done=0` here against `witness=0x1
done=1` with the e1000e: over the link the fixture did not reach `done=1` inside the 120 s wall. Owed
on a box where 10.0.2.3:53 answers; not claimed.

## Go-red by mutation
`set_mac_from_string_descriptor`: `d[1] != 0x03` → `d[1] != 0x04` (the string descriptor is refused),
same command:
```
:: USBNET: configuration 0 holds no ECM pair (rndis_seen=1) — requesting configuration index 1 of 2 ::
:: USBNET: ecm candidate slot=3 cfg=1 ctrl=0 data=1 alt=1 imac=3 mss=0 in_mps=64 out_mps=64 ::
:: USBNET: bring-up slot=3 refused at GET_DESCRIPTOR(string iMACAddress) -> FAIL ::
  ❌ FORBID hit @ line 1146: :: USBNET: bring-up slot=3 refused at GET_DESCRIPTOR(string iMACAddress) -> FAIL ::
  ❌ MBENCH FAIL — 3/9 required witnesses, 3 forbidden hit(s), 2784 lines scanned [mode unknown: no run sidecar]
  ❌ REQUIRE    :: SOCK-1: smoltcp icmp echo 10\.0\.2\.2 4/4 replies — witness OK ::
  ❌ REQUIRE    :: SOCK-2: ring-3 udp round-trip — socket/bind/sendto OK, recvfrom returned a datagram FROM 10\.0\.2\.3:53, socket teardown clean -> PASS ::
  ❌ REQUIRE    :: SOCK-2: smoltcp udp dns query 10\.0\.2\.3:53 -> \d+ bytes back — witness OK ::
  ❌ REQUIRE    :: SOCK-4: transferable sockets — grantee received \+ round-tripped the moved socket, .* -> PASS ::
  ❌ REQUIRE    :: SOCK-5: smoltcp dhcpv4 lease 10\.0\.2\.30/24 gw 10\.0\.2\.2 — witness OK ::
  ❌ REQUIRE    :: USBNET: up slot=\d+ cfg=1 ctrl=0 data=1 alt=1 mac=40:54:00:12:34:57 filter=ok -> PASS ::
════════════ MBENCH VERDICT — x86-usbnet.spec vs /tmp/claude-0/-home-user-UnaOS/823e4b66-c41d-5863-8c5e-22382d4804ac/scratchpad/usbnet
```
Source restored afterwards (`grep -c GO-RED usbnet.rs` = 0 before the commit).

## §SO55 — the configuration-descriptor read, default leg unchanged
`UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 120` on the tree with the
256-byte read (no USBNET knob): rc=1 from SOCK-3 only, and every stock enumeration line as before
(compare the HDASIE run, same box, 64-byte read):
```
      1 xHCI: >>> BULK IN EP FOUND: 0x81, MPS: 1024 <<<
      1 xHCI: >>> BULK OUT EP FOUND: 0x2, MPS: 1024 <<<
      1 xHCI: Configuration Descriptor Total Length: 34
      1 xHCI: Configuration Descriptor Total Length: 44
      1 xHCI: Endpoints Configured (Slot 1). Storage ready.
      1 xHCI: HID Endpoints Configured (Slot 2). Proceeding to Set Configuration...
```
On the dongle-only leg the same read produced `Total Length: 67` and `80` and walked both whole — with 64 the ECM data interface's alt-1 endpoints (bytes 64..80) were never seen.

## Run D — the two-front-end tree (AX88179 built beside ECM), ECM leg re-measured
Same command, after the AX88179 front-end landed on the same link (its lines never print here: QEMU
has no model of the part; `FORBID kind=ax88179` in the spec holds at 0 hits). rc=1 from the
container's SOCK-3 only.
```
:: USBNET: configuration 0 holds no ECM pair (rndis_seen=1) — requesting configuration index 1 of 2 ::
:: USBNET: candidate kind=ecm slot=3 vidpid=0525:a4a2 cfg=1 ctrl=0 data=1 alt=1 imac=3 mss=0 in_mps=64 out_mps=64 ::
:: USBNET: up kind=ecm slot=3 cfg=1 ctrl=0 data=1 alt=1 mac=40:54:00:12:34:57 filter=ok -> PASS ::
:: USBNET: rx=181 tx=331 rx_drop=0 tx_drop=136 errors=0 ::
:: SOCK-1: smoltcp icmp echo 10.0.2.2 4/4 replies — witness OK ::
:: SOCK-2: smoltcp udp dns query 10.0.2.3:53 -> 64 bytes back — witness OK ::
:: SOCK-5: smoltcp dhcpv4 lease 10.0.2.30/24 gw 10.0.2.2 — witness OK ::
:: SOCK-2: ring-3 udp sockets — sys_socket(40)/bind(41)/sendto(42)/recvfrom(43), a datagram round-trip over the persistent smoltcp stack ::
:: SOCK-2: ring-3 udp round-trip — socket/bind/sendto OK, recvfrom returned a datagram FROM 10.0.2.3:53, socket teardown clean -> PASS ::
:: SOCK-4: transferable sockets — SYS_XFER moves a KIND_SOCKET cap cross-row (owner migrates), the grantee round-trips it, the grantor's stale handle is rejected ::
:: SOCK-4: transferable sockets — grantee received + round-tripped the moved socket, grantor's migrated-away handle -EACCES, gen-rebind rejected, teardown clean -> PASS ::
════════════ MBENCH VERDICT — x86-usbnet.spec vs /tmp/claude-0/-home-user-UnaOS/823e4b66-c41d-5863-8c5e-22382d4804ac/scratchpad/usbnet
  ✅ MBENCH PASS — 9/9 required witnesses, 0 forbidden hit(s), 2779 lines scanned [mode unknown: no run sidecar]
```
`tx_drop=136`: frames smoltcp emitted while the controller loan was Busy (the main loop's pass held
it) and the 8-deep TX ring was full — counted, retransmitted by the stack, and the DHCP/ICMP/DNS
witnesses above are what came through. `vidpid=0525:a4a2` is QEMU's usb-net.

## The AX88179 front-end — NOT measured here
Built on this link (`usbnet::ax`, `usbnet_bringup_ax`, `deliver_ax`; network_stack.md §10). QEMU
8.2 has no model of the part, so nothing of it ran. Its first bench boot is scored on
`:: USBNET: candidate kind=ax88179 slot=… vidpid=0b95:1790 …`, `[usbnet] ax88179 link_status=…
medium(readback)=… qctrl=… phy_aneg=…`, then `:: USBNET: up kind=ax88179 … mac=<the dongle's> -> PASS ::`
or the named step that refused, and then smolnet's `:: SOCK-5: … lease` from the room's DHCP.
