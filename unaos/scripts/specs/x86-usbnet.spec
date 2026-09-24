# x86-usbnet.spec — USBNET (LEDGER SO56): the USB Ethernet link (CDC-ECM) as the ONLY NIC.
#   QEMU gate:  UNAOS_WC=1 UNAOS_QEMU_FULL=1 UNAOS_USBNET=1 UNAOS_NOE1000=1 ./arroyo test 120
#               (arroyo names this file in code as X86_USBNET_SPEC and replays it on exactly that knob
#               pair; with the e1000e present the smolnet pins below would pass over the wrong NIC, so
#               `x86_spec_replay` refuses the replay and says why.)
#   Measured 2026-09-24 on QEMU 8.2.2 (TCG, pc-q35-8.2) — every line below is a line that capture had.
#
# THE DEVICE. QEMU's `usb-net` presents TWO configurations, RNDIS first (index 0: 0x02/0x02/0xFF +
# 0x0A) and CDC-ECM second (index 1: 0x02/0x06 + 0x0A alt 0/1). The walk of index 0 finds no ECM pair
# and asks for index 1 — that request is the first pin. The station address the device reports is
# 40:54:00:12:34:57: the `mac=` the builder passes with the first octet's locally-administered bit set
# by the device, which is the address the guest is meant to use on its end of the link.
REQUIRE :: USBNET: configuration 0 holds no ECM pair \(rndis_seen=1\) — requesting configuration index 1 of 2 ::
REQUIRE :: USBNET: ecm candidate slot=\d+ cfg=1 ctrl=0 data=1 alt=1 imac=\d+ mss=\d+ in_mps=\d+ out_mps=\d+ ::
REQUIRE xHCI: USBNET Endpoints Configured \(Slot \d+\)\. Link bring-up pending\.
REQUIRE :: USBNET: up slot=\d+ cfg=1 ctrl=0 data=1 alt=1 mac=40:54:00:12:34:57 filter=ok -> PASS ::
FORBID :: USBNET: .* -> FAIL ::
FORBID :: USBNET: link down
#
# THE STACK OVER THE LINK. 10.0.2.30 is the `dhcpstart` of the usb-net's OWN user netdev (n1); the
# e1000e's netdev hands out 10.0.2.20 and is not attached on this leg at all (FORBID below), so the
# lease address is the proof that DHCP, and everything after it, went through the dongle.
REQUIRE :: SOCK-5: smoltcp dhcpv4 lease 10\.0\.2\.30/24 gw 10\.0\.2\.2 — witness OK ::
REQUIRE :: SOCK-1: smoltcp icmp echo 10\.0\.2\.2 4/4 replies — witness OK ::
REQUIRE :: SOCK-2: smoltcp udp dns query 10\.0\.2\.3:53 -> \d+ bytes back — witness OK ::
REQUIRE :: SOCK-2: ring-3 udp round-trip — socket/bind/sendto OK, recvfrom returned a datagram FROM 10\.0\.2\.3:53, socket teardown clean -> PASS ::
REQUIRE :: SOCK-4: transferable sockets — grantee received \+ round-tripped the moved socket, .* -> PASS ::
FORBID \[e1000\] up:
FORBID \[e1000\] BAR0
# NOT pinned: `:: SOCK-3:` (tcp connect 10.0.2.3:53). It is REFUSED in the cloud container with the
# e1000e too (LOGIN14.md, HDASIE.md) — environmental, not the link's — and the default FORBID on
# `-> FAIL` will red the run wherever it fails; a bench with a resolver on 10.0.2.3:53 scores it.
