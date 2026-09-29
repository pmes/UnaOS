# USBNET3 — the AX88179 dongle on the xHCI path: witness + link poll

**Finding (boot 16, f16.log 4660-4700).** The dongle (0b95:1790) enumerated on EHCI at 2090 ms, then again on xHCI
port 5 at 35711 ms (PORTSC=0x1203 = SuperSpeed): `SLOT 2`, `VENDOR ID [0b95]`, `Configuration Descriptor Total
Length: 70`, `Interface: Class=0xff Sub=0xff Proto=0x0` — then nothing: `ENUM RECOVERY: port 5 failed at 'cfg-desc'
(watchdog-timeout)`. The image did not arm `UNAOS_USBNET=1`, so the walk had no consumer for a vendor interface (no
bulk pair collected, no Configure-Endpoint, stage never advanced) and the watchdog retried. Knob armed, the walk hands
the slot to `usbnet_after_walk` (xhci/mod.rs tail) and the enumeration proceeds.

**What already existed (USBNET/USBNET2, unflown).** xhci/usbnet.rs: AX88179 candidate detect (`note_interface`, class 0xFF +
`is_ax_part`), bring-up `usbnet_bringup_ax` (PHYPWR_RSTCTL, CLK_SELECT, NODE_ID mac read, RX_BULKIN_QCTRL, pause water,
COE off, RX_CTL, MEDIUM, PHY aneg), bulk-IN arm/claim + `deliver_ax` trailer parse, bulk-OUT with the 8-byte TX header
(`service_usbnet`), and the stack seam: `e1000::raw_rx/raw_tx/hw_addr` fall back to `usbnet::*` (e1000.rs:1039-1053), so
smolnet DHCP/ping/SNTP/NETFETCH run over the dongle when it is the only NIC. Knob: arroyo:2569, builder main.rs:336, Cargo feature.

**Added here.** (M1) `ax_phy_read`, PHY advertise (ADVERTISE 0x05e1, CTRL1000 1000FULL, BMCR restart) per Linux
`ax88179_reset`; non-blocking link poll `usbnet_ax_poll` (PHYSR 0x11: LINK 0x0400, speed 0xC000, duplex 0x2000) every 250 ms
until up, then MEDIUM_STATUS_MODE rewritten for the negotiated speed (Linux `ax88179_link_reset`: GIGAMODE|EN_125MHZ, PS
for 100M); `[usbnet] link up speed=…`. (M2) The witness `:: USBNET: bus=xhci slot= mac= link= speed= usb= rx= tx= … -> PASS|FAIL ::`
at link resolution (or 15 s without link: `link=down`, still PASS), again at +20 s and +60 s so rx/tx are the reading; a
FAIL twin on bring-up refusal. (M3) `deliver_ax` hands the stack pkt_len bytes after the 2-byte pad (safe if pkt_len does
or does not include the pad). banner-cert row `usbnet`; x86-witness.spec OPTIONAL row (QEMU has no AX88179 — no lane can
produce it, so no REQUIRE).

## Written
Boot 17 (armed `UNAOS_USBNET=1`, dongle on xHCI) should show: `:: USBNET: candidate kind=ax88179 …`, `xHCI: USBNET Endpoints
Configured`, `[usbnet] ax88179 link_status=…`, `:: USBNET: up kind=ax88179 … mac=… ::`, `[usbnet] link up speed=…M`, then
`:: USBNET: bus=xhci slot=N mac=… link=up speed=1000 usb=ss rx=… tx=… -> PASS ::`. Unflown: register values are Linux
ax88179_178a.c's; pkt_len pad semantics and the SS bulk endpoint burst (companion descriptor ignored) are bench-owed.
