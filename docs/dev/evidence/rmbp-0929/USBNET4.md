# USBNET4 — AX88179 RX parse + SNTP target

## Design
Finding (boot 17): `:: USBNET: bus=xhci slot=2 ... link=up speed=1000 usb=ss rx=46 tx=10358 rx_drop=46 ... errors=46` — every RX transfer dropped; SNTP then fell back to `10.0.2.2 no reply`.
Mechanism: `drivers/xhci/usbnet.rs` `deliver_ax` (RX trailer rx_hdr: pkt_cnt low16, hdr_off high16; per-packet u32 headers at hdr_off; frame at pkt_start+2, len pkt_len-2); armed from `drivers/xhci/mod.rs` ~17240 (RX_CHUNK 2048). QCTRL SS table unchanged (matches Linux first entry).
Milestones: M1 `[usbnet] rx raw ...` once; M2 parse + counters rx_ok/rx_crc/rx_drop_err/rx_short on both `:: USBNET:` lines; M3 `smolnet.rs` `witness_tick_sntp` target: DNS pool then leased router, none without lease (`[sntp] target=… from=lease|dns|none`).

## Written
Boot 18 should show `:: USBNET: ... rx_ok=N>0 ...`, a `[dhcp]`/SOCK-5 lease, `[sntp] target=… from=dns|lease`, and if parse still wrong the `[usbnet] rx raw len= rx_hdr= pkt_cnt= hdr_off= first_pkt_hdr= reason=` line.
