# USBNET7 — the AX88179 receives nothing after USBNET6 (B328)

CHARTER: Kernel — driver (no new kernel file; edits `drivers/xhci/usbnet.rs` and the usbnet tail of `drivers/xhci/mod.rs`, plus two same-line appends in its descriptor walk).

## Finding — boot 20 (`rmbp-0915/flight20/f20-boots.log`, read with awk), the bring-up in order
1. Walk of slot 2 (`0b95:1790`, SS): `BULK IN EP FOUND: 0x82 MPS 1024`, `BULK OUT EP FOUND: 0x3 MPS 1024`, `BULK OUT EP FOUND: 0x5 MPS 1024`
   → `Configuring Bulk Endpoints for Slot 2 (IN 0x82 dci5 mps1024, OUT 0x5 dci10 mps1024)`; `X200: slot2 bulk-in TRdeq=0x2033e800 (RS=1 HCH=0 CRR=1)`.
   IN is right (0x82, dci 5, mps 1024). **OUT is 0x05, not 0x03**: the walk keeps the LAST bulk endpoint of each direction;
   Linux `usbnet_get_endpoints` takes the FIRST (0x03). Same in flight 19.
2. `[usbnet] reg PHYPWR_RSTCTL=0x0020/0x0000`, `CLK_SELECT=0x03/0xd9`, `rx buf=20480 qctrl=[07,4f,00,12,ff]`, `RX_BULKIN_QCTRL=0x074f0012ff/0x07650009ff`,
   `PAUSE 0x34/0x04 0x52/0x10`, `RXCOE 0/0`, `TXCOE 0/0`, `MONITOR 0/0`, then (USBNET6's new order) PHY ADVERTISE/CTRL1000/BMCR-restart (no readback),
   `MEDIUM_STATUS_MODE=0x013f/0x0133` (RE bit 8 reads SET), `NODE_ID=0x9c69d3286ef4/0x9c69d3286ef4`, `RX_CTL=0x03aa/0x02aa` (START, AB, AMALL, AP, IPE read set).
3. `:: USBNET: up ... -> PASS`; the service pass arms ONE bulk-IN Normal TRB (20480 B, IOC) on dci 5 and rings the doorbell — no line says so (the gap M2 closes).
4. `[usbnet] link up speed=1000M ... medium=0x013f` at +4 s → `reg MEDIUM_STATUS_MODE=0x013f/0x0133` → `regs when=linkup rxctl=0x02aa medium=0x0133 ... re=1 ok=1`
   → `[usbnet] link=up ... rx_ok=0 rx_drop=0 rx_pad=0`. Six `SOCK-5 ... no offer`. At +3.5 min `:: USBNET: ... rx=0 tx=58380 errors=0 rx_short=0 ...`
   and `:: USBNET6: ... frames=0 ... reason=no-frame-5s -> FAIL`.
5. No `:: USBNET: IN completion code=` line in the boot: the armed TD never completed with an error (a halted / stalled pipe would have printed).
   Poll-driven (service pass + `raw_rx` drive), no interrupter of its own.

## Cause (M1)
Not (a): the re-arm is unconditional on `take_done` and flew 64 times in boot 19 with this exact code. Not (c) for RX: IN 0x82 dci 5 is the part's bulk-IN.
Not (d): `TransferRing::produce` pre-places the Link TRB and toggles the cycle at the wrap, unchanged since boot 19. Not (e): the buffer is 20480 and the
part's QCTRL readback (`[3]=0x09`) aggregates at most 11 KiB. The host-side RX code is byte-identical between flight 19 (receives, 64 transfers) and
flight 20 (receives nothing) — `git diff e708ffe1 26c35637` touches only the bring-up. **(b): USBNET6 M1 reordered the AX88179 bring-up** — PHY
autoneg restart moved AHEAD of MEDIUM_STATUS_MODE, MEDIUM (RE) ahead of RX_CTL START, and a NODE_ID write between them — against Linux
`ax88179_reset` (NODE_ID, QCTRL, PAUSE, COE, **RX_CTL START, MONITOR, MEDIUM**, then `mii_nway_restart` LAST) and against what boot 19 flew. The
readbacks are identical in both boots, so the part holds the same values but its receive MAC was started in the wrong state (RE + clock before START,
mid-negotiation); Linux additionally rewrites QCTRL and MEDIUM at link (`ax88179_link_reset`) and RX_CTL at open (`set_rx_mode`), which we never did.
Separately, TX goes to EP 0x05 (step 1), which is why even boot 19's receiving link never drew a DHCP offer.

## Milestones
- M1 this doc (the ordered trace, the cause).
- M2 FIX: bring-up in Linux order (NODE_ID read+write-back, QCTRL, PAUSE, COE, RX_CTL START, MONITOR, MEDIUM, PHY advertise + aneg restart last);
  at link-up (Linux `link_reset` + open): MEDIUM, QCTRL, RX_CTL rewritten, each read back (MEDIUM on its existing `reg` line; QCTRL and RX_CTL on the
  existing `regs when=linkup` line — no new boot line); the PHY MII writes read back onto the existing `ax88179 link_status=` line; the AX88179 bulk pair
  is the FIRST of each direction (OUT 0x03, Linux). Once at the first arm: `[usbnet] rx_arm n=1 ep=0x82 mps=1024`. Counters `rx_xfers=` (IN completions),
  `rx_zlp=` (completions under 4 bytes — previously silent) and `rx_arms=` on the existing census/rollup line.
- M3 WITNESS: `tests usbnet` waits up to 3 s for any frame:
  `:: USBNET7: rx_ok=<n> first_frame_ms=<n> ethertype=<0x....> -> PASS|FAIL ::`; on FAIL once: `[usbnet] ring deq=<0x..> enq=<i> cycle=<0|1> pending=<0|1> ep_state=<n> xfers=<n> zlp=<n> arms=<n>`.

## What boot 21 should print
Bring-up: the `reg` lines in the new order ending `reg MEDIUM_STATUS_MODE=0x013f/0x0133`, `ax88179 link_status=0x04 ... phy_aneg=1 adv=0x05e1/... `,
`:: USBNET: up ... -> PASS`, `[usbnet] rx_arm n=1 ep=0x82 mps=1024`; at link `[usbnet] link=up ... rx_ok=N` and `:: SOCK-5: ... lease ...`;
`tests usbnet` → `:: USBNET7: rx_ok=N first_frame_ms=M ethertype=0x0806|0x0800|0x86dd -> PASS ::`.
If it still FAILs, the ring line splits the cases: `xfers=0 pending=1 ep_state=1` = the part never answered the IN token (chip RX path);
`zlp>0` = the part answers with empty transfers; `ep_state=2|4` = halted pipe.

## Owed
- The chip-side mechanism of the order sensitivity is inferred from the boot 19/20 bisect, not read off the part; boot 21 confirms it.
- One outstanding bulk-IN TD (the 32 KiB slot buffer holds one 20 KiB RX + TX); a second TD needs a second buffer.
- smolnet's poll clock (USBNET6 owed) still drives the TX flood (tx=58380).
