# USBNET6 — the AX88179 is not dropping: the "chip-drop" is the alignment dummy header, and DHCP never ran on the dongle

CHARTER: Kernel — driver (no new kernel file; edits `drivers/xhci/usbnet.rs`, `drivers/xhci/mod.rs`, `smolnet.rs`, `net_tick.rs`, `tests.rs`).

## Finding (flight 19, `rmbp-0915/flight19/f19-boots.log`, read with awk)
- Chip: `vidpid=0b95:1790` = **AX88179(B)** USB 3.0 gigabit (not a 772/772A/772B: no MFB bits, no 772B header mode;
  the aggregation size is `AX_RX_BULKIN_QCTRL`). Carried by **xHCI slot 2, SuperSpeed** (port 6 usb3 `sp=4(SS)`,
  `in_mps=1024`). EHCI saw it first behind the hub at HS (`EHCI-HID ... 0b95:1790 speed=HS`, `USBNET-EHCI datapath=stub`)
  until the port switch flipped the port to xHCI; the EHCI front has no bulk datapath, so no EHCI transfer exists to truncate.
- The transfer the tree called `chip-drop`: `len=128 rx_hdr=0x00700002` (pkt_cnt 2, hdr_off 112). Trailer bytes
  `[00 88 70 00][00 80 00 80][00 00 00 00][02 00 70 00]`: header 0 = `0x00708800` → **pkt_len 112, no error bit** (a real
  110-byte IPv6 MLD frame from `c8:a3:62:ec:63:b1` to `33:33:00:00:00:16`, ethertype 86dd, after the 2-byte IPE pad);
  header 1 = `0x80008000` → **pkt_len 0**: the AX88179's dummy header used for alignment, which Linux
  `ax88179_rx_fixup` skips (`if (pkt_len == 0) continue;`, commit "Fix packet receiving", 2022) and never counts as an error.
- The census agrees: every `:: USBNET:` line has `rx_ok == rx_chip_drop` (16/16, 48/48, ..., 64/64): one good frame and
  one dummy per transfer. **Every frame reached the RX ring.** `rx_drop=` was inflated by the dummies only.
- Why no lease: there is **no `SOCK-5` line in any of the four boots**. `smolnet::dhcp_acquire` runs only from
  `smolnet::init()`, and `init()` is called only from the ring-3 socket syscalls. On the metal the stack is built
  lazily by the SOCK-2 witness (`stack_open` → `ensure_stack`, static `10.0.2.15/24` slirp config) and the DHCP
  socket's `Configured` event is never polled; `LEASED` stays false, so `[sntp] target=0.0.0.0 from=none` forever.
- Register readbacks (USBNET5 M2) differ from what was written (rxctl 0x03aa/0x02aa, medium 0x013f/0x0133,
  pause 0x34,0x52/0x04,0x10, qctrl 4f,12/65,09). Receive works regardless (START, AB, AMALL, RE all read set), so the
  differing bits are the chip's read view (DROPCRCERR, EN_125MHZ/ALWAYS_ONE read 0; QCTRL/pause readback looks like
  the part's own defaults or a different read encoding) — M1 prints each one as `wrote/read` so the bench pins it.

## The AX88179 register sequence (Linux `ax88179_reset` + `ax88179_link_reset` + `set_multicast`), as now written
1. SET_CONFIGURATION 1. 2. `PHYPWR_RSTCTL`(0x26)=0 → 10 ms → =IPRL 0x0020 → 200 ms. 3. `CLK_SELECT`(0x33)=ACS|BCS 0x03 → 100 ms.
4. read `NODE_ID`(0x10) = the MAC. 5. `RX_BULKIN_QCTRL`(0x2e) = SS `{07,4f,00,12,ff}` (HS/FS rows), bulk-IN posted = 1024*(q3+2)
   = 20480 at SS — the buffer and the aggregation agree (the AX88179's "MFB"). 6. `PAUSE_WATERLVL_LOW/HIGH`(0x55/0x54)=0x34/0x52.
7. `RXCOE_CTL`/`TXCOE_CTL`(0x34/0x35)=0. 8. `MONITOR_MODE`(0x24)=0.
9. PHY up: MII ADVERTISE=0x05e1, CTRL1000=0x0200, BMCR=ANENABLE|ANRESTART 0x1200 (link polled, never blocked on).
10. `MEDIUM_STATUS_MODE`(0x22) = 0x013f provisional (RE|TXFC|RXFC|125|1|FD|GM), rewritten from PHYSR at link-up.
11. **write `NODE_ID`** = the MAC back (Linux `ax88179_get_mac_addr` writes it; the directed filter keys on it).
12. `RX_CTL`(0x0b) = 0x03aa START|AB|AMALL|AP|IPE|DROPCRCERR — last, after medium and node id.
Every write gets `[usbnet] reg <name>=<wrote>/<read>` (0x-prefixed hex in the log; no bare hex in a ledger cell).

## Milestones
- M1 doc (this) + bring-up order 9→10→11→12, NODE_ID write-back, per-register readback lines, `rx_ctl` readback kept.
- M2 parser: pkt_len==0 → dummy, counted `rx_pad=` and skipped (Linux); DROP_ERR/CRC_ERR with pkt_len>0 → the chip's
  verdict, first three flagged headers' raw bytes printed once per boot. R80: the doubling `:: USBNET: rx=` rollup and the
  +20/+60 s witness repeats go behind `tests usbnet`; the bring-up verdict
  `[usbnet] link=<up|down> speed=<n> mac=<..> rx_ctl=<0x..> rx_ok=<n> rx_drop=<n>` stays (the driver deciding, once at link resolution).
- M3 DHCP on the dongle: `smolnet::dhcp_link_tick()` from the 5 s `net_tick::service_tick`: once the link is up (PHY LINK for
  the dongle) and no lease, ensure the stack and run the chunked acquisition; up to 6 tries; prints the existing SOCK-5 line.
- M4 `tests usbnet`: link → pull frames for 5 s → PASS with the first ethertype; no dongle/no link → SKIP; zero frames
  with chip drops → FAIL naming rx_ctl. Witness:
  `:: USBNET6: chip=ax88179 rx_ctl=<r> mfb=qctrl-buf<n> frames=<n> dropped=<d> first_ethertype=<0x....> -> PASS|SKIP|FAIL ::`.

## What a metal boot (20) should print
`[usbnet] reg RX_CTL=0x03aa/0x....` (and the other writes), `[usbnet] link=up speed=1000 mac=9c:69:d3:28:6e:f4 rx_ctl=0x02aa ...`,
then `:: SOCK-5: smoltcp dhcpv4 lease a.b.c.d/n gw ... — witness OK ::`, `[sntp] target=... from=dns|lease`, and after `tests usbnet`
`:: USBNET6: chip=ax88179 ... frames=N>0 dropped=0 first_ethertype=0x.... -> PASS ::`.

## Owed
- smolnet's clock is a poll counter (`POLL_CLOCK`, 1 ms per poll), so smoltcp's retransmit/ARP timers run far faster than wall
  time on a USB link; flight 19 shows tx=65472 in four minutes. Not this arc; named for NETRING3.
- The QCTRL/pause readback differences are printed, not explained, until the bench reads them.
