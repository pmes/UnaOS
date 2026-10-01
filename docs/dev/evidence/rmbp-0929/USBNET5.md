# USBNET5 — the AX88179 chip emits DROP_ERR headers: post the real bulk-IN size, read the RX registers back, tolerate drop markers

## Finding (boot 18, flight18/f18-boot1.log)
`[usbnet] rx raw len=128 rx_hdr=0x00700002 pkt_cnt=2 hdr_off=112 first_pkt_hdr=0x80008000 reason=pkt-len-bad` on every transfer;
`rx_short=` all, no lease, `[sntp] target=0.0.0.0 from=none`. Geometry is consistent (112 B of packets, 2 headers at 112..120,
rx_hdr at 124); 0x80008000 = DROP_ERR (bit 31) with pkt_len 0: the chip itself marks the packets dropped, and the tree classified the whole transfer short.

## Mechanism
- Bulk-IN TD was `RX_CHUNK`=2048 (`usbnet.rs` RX_CHUNK; mod.rs `service_usbnet`), far below Linux's `rx_urb_size = 1024*(QCTRL[3]+2)` (SS 20 KiB). QCTRL tuple already written (`usbnet_bringup_ax`) promises aggregation up to that size.
- RX_CTL 0x03aa (= Linux DROPCRCERR|IPE|START|AP|AMALL|AB), RXCOE 0, MONITOR 0, PAUSE lo 0x34 / hi 0x52, MEDIUM 0x013f then rewritten per PHYSR at link-up (`link_seen`) — all already Linux-shaped; never read back before, so M2 makes them visible.

## Milestones
- M1 buffer: `rx_len()` = 1024*(q[3]+2): SS 20480, HS 24576, FS 26624 (max = `RX_CHUNK_MAX`, the 32 KiB 64K-aligned slot buffer; TX moved to +28672). QCTRL per USB speed (SS/HS/new FS tuple {07,cc,4c,18,08}). Prints `[usbnet] rx buf= qctrl=[..]`.
- M2 regs: `usbnet_ax_regs` prints `[usbnet] regs when=bringup|linkup rxctl= medium= rxcoe= monitor= pause=lo/hi qctrl= re= ok=` and keeps the MEDIUM readback for the witness `re=`. Medium write itself (RE|GM|FD|125|flow) was already Linux link_reset (`link_seen`).
- M3 tolerate: DROP_ERR checked BEFORE the length test, counts `rx_chip_drop=`, continues to the next header; first one dumps head/trailer bytes once.

## Written
Witness boot 19 should show: `:: USBNET: bus=xhci ... rx_ok=N>0 ... rx_short=0 rx_chip_drop=K buf=20480 re=1 -> PASS ::`, then `[dhcp] ... lease`.
No spec pin (metal-only; the only usbnet spec lane is QEMU ECM). No new knob.
