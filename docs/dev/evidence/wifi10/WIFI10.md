# WIFI10 (B646) — rung S7's probe-request half: the design, the b43 citations, the wire shapes

Peter, 2026-10-14: "what about the GPUs wifi and there must be more!!!". After WIFI9 (B625: S5i, S5c, S6 — the
receive-only rungs, `tests wifi` under `UNAOS_WIFI9=1`), the next rung is S7, and its first observable
(bcm4331.md §S7: "Each of scan / probe / auth / assoc has its own observable") is the probe exchange: ONE frame
out of the chip, and the responses the S6 receiver then hears.

R110 (RULINGS.md) and R95 §2: the Linux b43 driver (GPL-2.0, `drivers/net/wireless/broadcom/b43/`, torvalds/linux
master fetched 2026-10-13 into the executor's scratch, never into the tree) read for HARDWARE FACTS only — register
numbers, bits, the header layout, sequences. The frame is IEEE Std 802.11 (`[PUBLIC]`). No text copied, no
function transcribed. The one fact outside the arc's source list is named below and the half that needs it is
designed-not-built.

Knob: `UNAOS_WIFI10=1` → `wifi10` (implies `wifi9`). **The first knob that lets the d11 transmit.** Default OFF.

## Design (R106, written before the code)

What S6 proved (WIFI9's `-> SEEN`): the PHY is tuned to channel 11 and decodes beacons, the 64-bit RX DMA engine on
controller 0 delivers frames into host memory with the FW598 frame offset, the MAC runs in station opmode with
promiscuous receive. What S7 adds, in order, all from `tests wifi` once per boot after `wifi9()`:

1. **Gates.** `status::wifi9().tune == 1` (the channel-11 tune read back MATCH this boot) and `ident::mac()` (the
   SPROM `il0macaddr` S2r judged plausible). Either missing → `:: wifi10: tx NOT ATTEMPTED reason=…` and no write.
2. **MAC suspended** (re-asserted; `main.c:2987-3017`) and the **PHY's TX side**: under the PA override
   (`phy_ht.c:196-213`: `RF_CTL_INT_C1..C3` saved, 0x0400 written, restored after) the RF sequencer is forced
   RX2TX then RST2RX (`phy_ht.c:983-986`) — the TX path sequenced once, the radio left in RX — then the TX power
   fix and power control OFF (`:999-1000`); `TXPCTL_CMD_C1..C3` (phy_ht.h:39, :62, :100) are read and printed:
   that is the fixed power index the frame leaves at. (`htphyinit::tx_prep`.)
3. **The channel cookie** SHM 0x00A0 = 11 (`phy_common.c:425-432`): the ucode compares the header's
   `chan_radio_code` against it (`xmit.c:360-364`) and suppresses on mismatch (`B43_TXST_SUPP_CHAN`).
4. **`tx::prep`** — the rate memory (`main.c:3162-3199`: shared `0x480 + (ofdm code & 0xF)*2` and
   `0x4C0 + (cck code & 0xF)*2`, each copied to +0x20; 12 entries), the PHY-TX-control words the ucode's own frames
   use (`main.c:3206-3216`: BEACPHYCTL 0x54, ACKCTSPHYCTL 0x22, PRPHYCTL 0x188 ← CCK|ANT01AUTO|TXPWR), the
   station MAC into `MACFILTER_SELF` (`main.c:657-677`, `:705-709`: control 0x420 ← 0x0020, three halfwords
   through 0x422), the TX engine reset (`dma.c:478-530`) and ONE 64-bit TX ring on controller 1 — `DMA64_BASE1`
   0x240 (b43.h:70), b43's non-QoS `AC_BE` ring (`dma.c:1089`, `:1330`) — programmed per `dma.c:681-694`
   (TXCTL enable + parity off, RINGLO, RINGHI with the PCI-host translation S6 proved). 64 slots x 16 bytes in
   8 KiB-aligned memory (`dma.h:166`, `dma.c:407-428`).
5. **S6's receiver armed** (`rx::arm`: the MAC enabled, the ucode awake) with `Seen.me` = the station MAC.
6. **`tx::fire`** — the frame (IEEE 802.11 §9.3.3.10: FC 0x0040, DA ff:ff:ff:ff:ff:ff, SA = the station MAC, BSSID
   wildcard, seq 0; SSID IE wildcard, Supported Rates 1/2/5.5/11 basic, DS Parameter Set 11 — 35 bytes) and
   the 118-byte FW598 TX header (`xmit.h:25-65`; size `xmit.h:192-196`) built as `xmit.c:235-567` builds it for a
   management frame at CCK 1 Mbit/s with no key, no RTS/CTS: `mac_ctl` = HWSEQ|STMSDU|LONGFRAME (no ACK on a
   broadcast), `phy_ctl` = CCK|ANT01AUTO, `phy_ctl1`/`_fb` = BW_20 (`xmit.c:167-181`: CCK on an HT-PHY), `phy_rate`
   0x0A, `extra_ft` CCK fallback, `chan_radio_code` 11, `tx_receiver` = DA, `dur_fb` = the frame's duration (0),
   both PLCPs (`xmit.c:136-165`: 0x0A, 0x04, length = octets*16/2 µs with the FCS counted), the cookie 0x2000
   (`dma.c:1133-1150`: (controller+1)<<12 | slot 0) at `format_598.cookie`. Two descriptors (`dma.c:1186-1286`,
   `:35`): the header on slot 0 FRAMESTART, the frame on slot 1 FRAMEEND|IRQ; `TXINDEX` ← 2 x 16 (`dma.c:213-217`).
   Then the TX-status FIFO (`XMITSTAT_0/1` 0x170/0x174, b43.h:53-54) is drained for up to 200 ms and decoded per
   `main.c:1341-1356` (cookie = v0>>16; frame_count, rts_count, supp_reason, acked, intermediate, for_ampdu
   from v0's low half; seq and phy_stat from v1; an `intermediate` entry is not final, `xmit.c:823`).
7. **The response window**: `rx::poll` 2 s; `take` now decodes subtype 5 (probe response) through
   `wifi_core::ieee80211::parse_beacon` and counts those whose addr1 is our MAC.
8. **Stop**: `rx::finish` (MAC suspended, RX engine reset), `tx::stop` (TX engine reset, ring registers cleared,
   `dma.c:753-760`), SHM_CONTROL restored.

Verdicts: **SENT** ⇔ the FIFO carried OUR cookie with `frame_count > 0` and `supp_reason == 0`; otherwise
`NOT-SENT reason=` no-txstatus(dptr before→after, TXSTATUS, TXERROR) | foreign-cookie | frame_count=0 supp=… |
supp=…. **HEARD** ⇔ at least one probe response addressed to our MAC; otherwise `SILENT reason=` tx:<the TX
reason> | responses-not-to-us(n) | no-probe-resp(frames=n beacons=n) | nothing-received. WIFI5's S7 row goes
Confirmed only on HEARD.

### Designed, not built — and why

* **HT-PHY TX power control** (`phy_ht.c:589-640` `b43_phy_ht_tx_power_ctl_setup`, `:572-587` idle TSSI via the
  TX tone, `:589-594` TSSI setup, `:533-570` enable): the setup takes `sprom->core_pwr_info[c].maxpwr_2g` and
  `pa_2g[0..3]` per core — the rev-8 SPROM offsets of those fields are in `drivers/bcma/sprom.c`, NOT in the
  b43 files the arc names. The frame therefore leaves with power control OFF at the index the init tables left in
  `TXPCTL_CMD_C1..C3` — printed on the wire (`txpctl_cmd=[…]`) — exactly the state b43 holds between
  `phy_ht.c:999` and `:1004`. If the bench reads `SENT` and `SILENT reason=no-probe-resp` with beacons present,
  TX power is the first suspect and the owed half is this one (the register/field: `TXPCTL_CMD_C1` 0x1E7 EN bits
  0xE000, `TXPCTL_TARG_PWR` 0x1EA, `TXPCTL_IDLE_TSSI` 0x1E9; the inputs: SPROM PA coefficients).
* No template RAM, no beacon, no probe RESPONSE from us (PRMAXTIME stays 1), no RTS/CTS, no encryption, no QoS
  ring, no interrupts (the IRQ descriptor flag is set as b43 sets it; nothing is unmasked).

### Residual, accepted by design

`MACFILTER_SELF` now carries the station MAC, so the running microcode ACKs unicast frames addressed to it — the
probe responses asked for. S7 is the transmit rung; the end line counts every write class.

## Citations — every b43 hardware fact, to its line in our code

`wifi/` = `unaos/crates/kernel/src/wifi/`. WIFI9's 108 rows (docs/dev/evidence/wifi9/CITATIONS.md) stand for the
receiver this rung re-uses; rows below are the new ones.

| # | fact | source (file:line) | our code (file:line) |
|---|---|---|---|
| 1 | DMA64 controller 1 base 0x240 | `b43.h:70` | `wifi/bringup/tx.rs` `DMA64_BASE1` |
| 2 | controller 1 = AC_BE ring; the non-QoS ring for every frame | `dma.c:1089, :1307-1335` | `tx.rs` header |
| 3 | TXCTL/TXINDEX/TXRINGLO/TXRINGHI/TXSTATUS/TXERROR offsets | `dma.h:85-104` | `tx.rs` `TXCTL..TXERROR` |
| 4 | TXENABLE 1, TXPARITYDISABLE 0x800, TXADDREXT mask/shift | `dma.h:86-92` | `tx.rs` |
| 5 | TXSTATDPTR 0x1FFF, TXSTAT states 0..4 << 28, TXERR mask | `dma.h:97-106` | `tx.rs` `txstat_name`, `Report` |
| 6 | descriptor ctl0 DTABLEEND/IRQ/FRAMEEND/FRAMESTART; ctl1 BYTECNT/ADDREXT | `dma.h:149-155` | `tx.rs` `fill` |
| 7 | 8 KiB ring memory, 8 KiB aligned | `dma.h:166, dma.c:407-428` | `tx.rs` `RINGMEM` |
| 8 | two slots per frame: header then body | `dma.c:35, :1186-1286` | `tx.rs` `fire` |
| 9 | TX controller setup: TXCTL, RINGLO, RINGHI | `dma.c:681-694` | `tx.rs` `prep` |
| 10 | TX engine reset: quiescent wait, TXCTL 0, wait DISABLED, 1 ms | `dma.c:478-530` | `tx.rs` `tx_reset` |
| 11 | controller cleanup clears RINGLO/RINGHI | `dma.c:753-760` | `tx.rs` `stop` |
| 12 | poke: TXINDEX = next slot x 16 after a wmb | `dma.c:213-217, :1284-1285` | `tx.rs` `fire` |
| 13 | cookie = (index+1)<<12 \| slot | `dma.c:1133-1150` | `tx.rs` `COOKIE_RING1` |
| 14 | XMITSTAT_0 0x170, XMITSTAT_1 0x174; bit 0 = entry present; field decode | `b43.h:53-54, main.c:1334-1360` | `tx.rs` `fire` |
| 15 | supp_reason values | `xmit.h:226-235` | `tx.rs` `SUPP_NAMES` |
| 16 | intermediate status is not final | `xmit.c:823` | `tx.rs` `fire` |
| 17 | MACFILTER_CONTROL 0x420, DATA 0x422, SELF 0, select \|= 0x0020, three halfwords | `b43.h:106-107, :399, main.c:657-677` | `tx.rs` `macfilter_self` |
| 18 | the card MAC upload | `main.c:705-709` | `tx.rs` `prep` |
| 19 | rate memory: 0x480/0x4C0 + (code & 0xF)*2, copied to +0x20, 8 OFDM + 4 CCK | `main.c:3162-3199` | `tx.rs` `rate_memory_init` |
| 20 | PHY TX control defaults: BEACPHYCTL 0x54, ACKCTSPHYCTL 0x22, PRPHYCTL 0x188 ← CCK\|ANT01AUTO\|TXPWR | `main.c:3206-3216, b43.h:302, :306, :312` | `tx.rs` `phytxctl_defaults` |
| 21 | FW598 TX header layout and size 112+6 | `xmit.h:25-65, :192-196` | `tx.rs` `TXH_*` |
| 22 | mac_ctl HWSEQ 0x10, STMSDU 0x8, LONGFRAME 0x2, ACK 0x1 | `xmit.h:124-128` | `tx.rs` `txhdr` |
| 23 | phy_ctl ENC_CCK, ANT01AUTO 0xC0, TXPWR 0xFC00 | `xmit.h:149, :157, :160` | `tx.rs` |
| 24 | phy_ctl1 for CCK on HT = BW_20 | `xmit.h:167, xmit.c:167-181, :554-558` | `tx.rs` |
| 25 | extra_ft fallback CCK | `xmit.h:132` | `tx.rs` |
| 26 | CCK rate codes 0x0A/0x14/0x37/0x6E; OFDM 0xB/0xF/0xA/0xE/0x9/0xD/0x8/0xC; CCK 1 Mbit/s = 2 | `xmit.c:96-131, b43.h:593` | `tx.rs` |
| 27 | CCK PLCP: rate code, service 0x04, length octets*16/bitrate (+FCS) | `xmit.c:136-165, :688` | `tx.rs` `plcp_cck` |
| 28 | txhdr fields for a plain mgmt frame: FC copy, receiver, dur_fb, chan_radio_code, cookie | `xmit.c:263-286, :360-364, :541-567` | `tx.rs` `txhdr` |
| 29 | the ucode compares chan_radio_code to SHM | `xmit.c:360-364` | `bringup.rs` `wifi10` (chan cookie) |
| 30 | PA override: RF_CTL_INT_C1..C3 saved / 0x0400 / restored | `phy_ht.c:196-213` | `htphyinit.rs` `pa_override` |
| 31 | op_init TX side: RX2TX then RST2RX under PA override | `phy_ht.c:983-986, phy_ht.h:76` | `htphyinit.rs` `tx_prep` |
| 32 | TXPCTL_CMD_C2 0x222, C3 EXTG(0x164) | `phy_ht.h:62, :100` | `htphyinit.rs` |
| 33 | tx power fix + power control off before/after the TX-power setup | `phy_ht.c:999-1004` | `htphyinit.rs` `tx_prep` |
| 34 | power control setup needs SPROM core_pwr_info (maxpwr_2g, pa_2g) | `phy_ht.c:589-640` | designed-not-built, above |

Rows: 34.

## The wire shapes the bench should expect (one `tests wifi`, the image line + UNAOS_WIFI9=1 UNAOS_WIFI10=1)

After WIFI9's three lines:

```
[wifi10] mac-suspend macctl pre=… post=… ucodestat=… suspended=1 polls=… — main.c:2987-3017
[wifi10] htphy-tx-prep rx2tx=done rst2rx=done txpctl_cmd=[0x…,0x…,0x…] tx_pwr_ctl=off idle_tssi=not-run txpctl_setup=not-run(…) tssi_setup=not-run …
[wifi10] tx-setup ctl=1 ring=0x… buf=0x… slots=64 txctl=0x00000801 ringhi=0x80000000 txstatus=0x…(disabled) reset=1 rate_mem=12 phytxctl=3 macfilter_self=<mac> — …
[wifi10] rx-setup ring=… (S6's line under the wifi10 tag)
[wifi10] tx-poke kind=probe-req len=35 plcp_octets=39 cookie=0x2000 txindex=32 hdr[0..24]=1a000000400000000c000200020000000000000000000a0000 — …
[wifi10] tx-status seen=1 cookie=0x2000 match=1 frame_count=1 rts_count=0 supp=none acked=0 … dptr=0->32 txstatus=0x…(idle-wait|stopped) txerror=0x00000000(err=0) waited=<n>ms — …
:: wifi10: tx frames=1 kind=probe-req ch=11 rate=1M(cck) len=35 from=<mac> cookie=0x2000 status=seen frame_count=1 supp=0 acked=0 seq=<n> txpctl_cmd=[…] -> SENT ::
[wifi10] rx-raw … (up to three)
[wifi10] rx-window tsf-delta=2000…us slots-consumed=<n> … stop: mac-suspended=1 engine-disabled=1 — …
:: wifi10: rx probe-resp=<n> to_us=<n> from=[<bssid>,…] ssids=[<ssid>,…] beacons=<n> frames=<n> fcs_err=<n> other_chan=<n> ch=11 window=2000ms -> HEARD ::
:: wifi10: end mac-suspended-first=1 cookie_pre=0x000b wifi9_beacons=<n> rx2tx=1 rst2rx=1 tx_pwr_ctl=off tx-reset=1 rx-reset=1 mac-suspended=1 wrote-macctl=… wrote-shm=… wrote-dma=… wrote-tx-desc=8 wrote-macfilter=4 … shm-ctl-restored=MATCH transmit=one(probe-req; …) ::
```

The failure shapes, each a distinct line: `-> NOT-SENT reason=no-txstatus(dptr 0->0 …)` (the engine never read the
descriptors — a ring defect); `reason=no-txstatus(dptr 0->32 …)` (the engine consumed them, the ucode reported
nothing — the FIFO or the MAC); `reason=supp=chan-mismatch` (the SHM channel cookie); `reason=frame_count=0 …`
(the ucode declined the frame); `-> SILENT reason=no-probe-resp(frames=n beacons=n)` with SENT (the air heard
nothing of ours — TX power, the designed-not-built half, first). QEMU models no BCM4331: WIFI1 SKIPs before any
of this and the lines are absent by design.

Then WIFI5's row S7 reads Confirmed only on HEARD; WIFI1's `scan=` stays WIFI9's beacon count.
