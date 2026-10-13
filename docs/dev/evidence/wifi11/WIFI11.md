# WIFI11 (B653) — S7's TX power control from the SPROM, and the S8 design

Peter, 2026-10-14: "what about the GPUs wifi and there must be more!!!". WIFI10 (B646, `docs/dev/evidence/wifi10/WIFI10.md`)
built S7's probe-request half and STOPPED at HT-PHY TX power control: its setup needs the SPROM's per-core PA
coefficients, whose offsets live in `drivers/bcma/sprom.c`, outside WIFI10's b43 list. This arc reads them, sets the
control up, turns it ON, and re-sends S7's probe request under it. S8 (open-network authentication + association) is
DESIGNED, not built: `docs/dev/OS/06_NETWORK_STACK/wifi_bcma.md` §S8 (dated 2026-10-14).

R110 (RULINGS.md) and R95 §2: Linux b43 (GPL-2.0, `drivers/net/wireless/broadcom/b43/`) and Linux bcma
(`drivers/bcma/sprom.c`, "Licensed under the GNU/GPL"; `include/linux/ssb/ssb_regs.h`, `include/linux/ssb/ssb.h`),
torvalds/linux master fetched 2026-10-13 into the executor's scratch (`~/unaos-bench/scratch/cloud/wave34/wifi11/linux/`),
never into the tree, read for HARDWARE FACTS only: SPROM byte offsets and field masks, PHY register numbers and bits,
the write order, the estimate-table arithmetic. No text copied, no function transcribed. Line numbers below are the
fetched copies' (phy_ht.c 1137 lines; WIFI10 cited an earlier fetch, so its `:589-640` is this copy's `:621-729`).

Knob: none new. Everything is under the `wifi10` cfg region (the arc's rule: no new knob unless the five-place rule
forces one; it does not — the lines are `wifi11`-tagged, the feature is `wifi10`). `image-line.env` untouched.

## Design (R106, written before the code)

1. **The SPROM read** (`wifi/bringup/ident.rs`, the same 220-word shadow S2r reads through ChipCommon — no new
   register, no new window). bcma accepts SPROM revisions 8..=11 (`sprom.c:167-173`) and extracts EVERY accepted
   revision through `bcma_sprom_extract_r8` (`sprom.c:643`), so the per-core block offsets are one layout for the
   card's revision whatever it reads in 8..11; the wire prints the revision beside the fields. Per core c in 0..2
   (the HT-PHY's three cores, `phy_ht.c:632-637`), block base 0x00C0/0x00E0/0x0100 (`ssb_regs.h:537-539`): byte +0 low
   = `maxpwr_2g`, high = `itssi_2g` (`ssb_regs.h:542-545`, `sprom.c:287-290`), +2/+4/+6 = `pa_2g[0..2]`
   (`ssb_regs.h:546-548`, `sprom.c:292-294`; the fourth coefficient in Linux's struct, `ssb.h:25`, is never read for
   this layout and never used by the HT setup). Word index = byte / 2 (`sprom.c:183`). The last byte read (0x107)
   is inside the 440-byte shadow. Gate: the S2r verdict PLAUSIBLE (`sprom=ok`, the gate `ident::mac()` shares).
   Our own refusal, not b43's: a block an unprogrammed SPROM would carry (PA words uniform 0xFFFF/0x0000, max power
   0 or 0xFF) is `sprom=blank(coreN)` and the control is NOT enabled — b43 would drive it from garbage.
2. **TX power control** (`htphyinit::tx_pwr_ctl`), b43's `op_init` tail in its order (`phy_ht.c:998-1004`), after
   `tx_prep`'s tx-power-fix + control OFF (`:999-1000`), the MAC suspended throughout as b43 runs it
   (`phy_common.c:455`):
   * **idle TSSI** (`:573-607`): per core (bases 0x840/0x860/0x880 = EXTG 0x40/0x60/0x80) save +6, +7, +0; +6 <- 0,
     +7 &= ~0xF, +0 |= 0x400, +0 |= 0x1000; the TX TONE (`:415-421`): the sample buffer, table 0x11, 160 x 32-bit zero
     (`:359-372`; b43 sets the cursor once, this tree per element), run endless (`:374-413`: DEP_CNT 159, LOOP_CNT
     0xFFFF, WAIT_CNT 0, RF_SEQ_MODE.CA_OVER around SAMP_CMD bit 0, RF_SEQ_STATUS bit 0 polled 100 x 10 us); 20 us;
     one RSSI sample under the TSSI-2G selection (`:467-509`, `:427-465`: AFE_Cx |= 0x300, |= 0xC00; AFE_Cx_OVER |=
     0x200, |= 0x400; radio C3|0xBF |= 1; radio Cx|0x159 <- 0x11; the twelve registers saved/restored; RSSI_C1..C3
     0x219..0x21B, 6-bit fields sign-extended); playback stopped (`:335-357`); CCA reset; the nine saved registers
     restored. idle[c] = the core's low field & 0xFF. **The tone is the first signal this tree puts through the PA.**
   * **setup** (`:621-729`, 2.4 GHz arm `:632-637`): target = maxpwr_2g, a1/b0/b1 = pa_2g[0..2] as s16; TSSIMODE.EN;
     CMD_C1 &= ~PCTLEN; IDLE_TSSI |= 0x4000; CMD_C1..C3 INIT field <- 0x19; IDLE_TSSI |= BINF; idle[0..1] into
     IDLE_TSSI C1/C2, idle[2] into IDLE_TSSI2 C3; TXPCTL_N TSSID 0xF0, NPTIL2 3; targets into TARG_PWR C1/C2 and
     TARG_PWR2 C3; per core the 64-entry estimate table HTTAB16(26+c, i): num = 8*(16*b0 + b1*i), den = 32768 + a1*i,
     pwr = max((4*num + den/2)/den, -8) (`:719-729`; C's truncating division is Rust's).
   * **TSSI mux** (`:609-619`): radio 0x8BF |= 1, radio Cx|0x159 <- 0x11, per core.
   * **control ON** (`:533-571`, enable arm `:556-568`): CMD_C1 |= COEFF|HWPCTLEN|PCTLEN. The 5 GHz 0x32 writes do not
     apply; b43's saved indices start at INIT+1 = 0x80 (`:862-864`), above INIT, so no CMD index is written and the
     hardware starts from the INIT field the setup wrote (0x19).
   * **readback**: CMD_C1..C3, TX_PCTL_STATUS_C1..C3 (0x1ED/0x1EE/EXTG 0x169 — the hardware's current index per core,
     the `target_idx` on the wire), TARG_PWR/TARG_PWR2, IDLE_TSSI/IDLE_TSSI2. ON <=> CMD_C1 carries all three EN bits.
3. **S7's probe request re-sent under control**: `bringup::wifi10()` runs step 2 between `tx_prep` and the channel
   cookie, so WIFI10's frame leaves with the control ON; WIFI10's lines follow unchanged except the end line's
   `tx_pwr_ctl=on|off`. On any refusal the control stays OFF and the frame leaves at the fixed index, as WIFI10 did,
   with the reason on the wire.
4. **S8 designed** — `wifi_bcma.md` §S8.

## Citations — every b43/bcma hardware fact, to its line in our code

`wifi/` = `unaos/crates/kernel/src/wifi/`. WIFI10's 34 rows stand for the ring and the header.

| # | fact | source (file:line) | our code |
|---|---|---|---|
| 1 | SPROM revisions 8..11 accepted; the revision word is the last word | `sprom.c:167-173` | `ident.rs` `CorePwr` doc, `pwr_line` |
| 2 | every accepted revision is extracted by `extract_r8` | `sprom.c:643` | `ident.rs` `CorePwr` doc |
| 3 | word index = byte offset / 2 | `sprom.c:183` | `ident.rs` `pwr_decode` |
| 4 | per-core power-info block bases 0xC0/0xE0/0x100/0x120 | `ssb_regs.h:537-540`, `sprom.c:225-228` | `ident.rs` `PWR_INFO_CORE` |
| 5 | +0x00: maxpwr_2g bits 7:0, itssi_2g bits 15:8 | `ssb_regs.h:542-545`, `sprom.c:287-290` | `ident.rs` `pwr_decode` |
| 6 | +0x02/+0x04/+0x06: pa_2g[0..2] | `ssb_regs.h:546-548`, `sprom.c:292-294` | `ident.rs` `pwr_decode` |
| 7 | the HT setup reads cores 0..2: target = maxpwr_2g, a1/b0/b1 = pa_2g[0..2] | `phy_ht.c:632-637` | `bringup.rs` `pin`, `htphyinit.rs` `PwrIn` |
| 8 | IQLOCAL_CMDGCTL 0x0C2; SAMP_CMD 0x0C3 (STOP 0x2), LOOP_CNT 0x0C4, WAIT_CNT 0x0C5, DEP_CNT 0x0C6, STAT 0x0C7 | `phy_ht.h:20-26` | `htphyinit.rs` `HT_IQLOCAL_CMDGCTL..HT_SAMP_STAT` |
| 9 | TSSIMODE 0x122, EN 0x1 | `phy_ht.h:30-31` | `htphyinit.rs` `HT_TSSIMODE` |
| 10 | TXPCTL_CMD INIT field 0x7F (C1, C2, C3), PCTLEN 0x8000 | `phy_ht.h:40, :43, :63, :101` | `htphyinit.rs` `HT_TXPCTL_CMD_INIT`, `_PCTLEN` |
| 11 | TXPCTL_N 0x1E8: TSSID 0xFF, NPTIL2 0x700 shift 8 | `phy_ht.h:44-48` | `htphyinit.rs` `HT_TXPCTL_N*` |
| 12 | TXPCTL_IDLE_TSSI 0x1E9: C1 0x3F, C2 0x3F00, BINF 0x8000; IDLE_TSSI2 EXTG(0x165) C3 0x3F | `phy_ht.h:49-54, :102-104` | `htphyinit.rs` `HT_TXPCTL_IDLE_TSSI*` |
| 13 | TXPCTL_TARG_PWR 0x1EA: C1 0xFF, C2 0xFF00; TARG_PWR2 EXTG(0x166) C3 0xFF | `phy_ht.h:55-59, :105-107` | `htphyinit.rs` `HT_TXPCTL_TARG_PWR*` |
| 14 | TX_PCTL_STATUS_C1..C3 0x1ED/0x1EE/EXTG(0x169) | `phy_ht.h:60-61, :108` | `htphyinit.rs` `HT_TX_PCTL_STATUS` |
| 15 | RSSI_C1..C3 0x219..0x21B | `phy_ht.h:64-66` | `htphyinit.rs` `HT_RSSI` |
| 16 | RF_SEQ_MODE.CA_OVER 0x1 | `phy_ht.h:73` | `htphyinit.rs` `HT_RF_SEQ_MODE_CA_OVER` |
| 17 | stop playback: STAT bit 0 -> CMD.STOP, bit 1 -> CMDGCTL &= 0x7FFF; CMD &= ~4; bb_mult restored to HTTAB16(13, 0x63/0x67 + i*4) | `phy_ht.c:335-357` | `htphyinit.rs` `stop_playback` |
| 18 | sample buffer: cursor 0x4400 (table 0x11), 20<<3 entries of DATAHI 0 / DATALO 0 | `phy_ht.c:359-372` | `htphyinit.rs` `load_samples` |
| 19 | run samples: bb_mult saved from HTTAB16(13, 0x63+i*4); DEP_CNT samps-1; loops 0xFFFF kept; WAIT 0; CA_OVER around CMD bit 0; the ~0 masks; STATUS bit 0 polled 100 x 10 us | `phy_ht.c:374-413` | `htphyinit.rs` `run_samples` |
| 20 | the TX tone = load + run(endless) | `phy_ht.c:415-421` | `htphyinit.rs` `idle_tssi` |
| 21 | RSSI select, TSSI 2G: AFE_Cx 0x300 / 0xC00, AFE_Cx_OVER 0x200 / 0x400, radio C3 0xBF bit 0, radio Cx 0x159 <- 0x11 | `phy_ht.c:427-465` | `htphyinit.rs` `rssi_select_tssi_2g` |
| 22 | poll RSSI: the twelve saved registers; two 6-bit fields per word, sign-extended | `phy_ht.c:467-509` | `htphyinit.rs` `HT_POLL_RSSI_SAVE`, `poll_rssi_tssi_2g` |
| 23 | idle TSSI: bases 0x840/0x860/0x880, +6/+7/+0 saved and driven, tone, 20 us, one poll, stop, CCA reset; idle = low field & 0xFF | `phy_ht.c:573-607` | `htphyinit.rs` `idle_tssi` |
| 24 | TSSI setup: radio 0x8BF bit 0, radio Cx 0x159 <- 0x11 | `phy_ht.c:609-619` | `htphyinit.rs` `tssi_setup` |
| 25 | power-control setup write order and values (TSSIMODE, PCTLEN off, 0x4000, INIT 0x19, BINF, idle fields, N 0xF0 / 3, targets) | `phy_ht.c:671-717` | `htphyinit.rs` `tx_power_ctl_setup` |
| 26 | the per-core 64-entry estimate table, HTTAB16(26+c, 0..64), its arithmetic | `phy_ht.c:719-729` | `htphyinit.rs` `tx_power_ctl_setup` |
| 27 | control ON: CMD_C1 |= EN bits; 5 GHz-only 0x32 writes; CMD index written only when the saved index <= INIT | `phy_ht.c:533-571` | `htphyinit.rs` `tx_power_ctl_on` |
| 28 | the saved indices start at INIT + 1; bb_mult_save at -1 | `phy_ht.c:855-867` | `htphyinit.rs` `tx_power_ctl_on` doc, `run_samples` |
| 29 | op_init's tail order: fix, OFF, idle TSSI, setup, TSSI setup, ON | `phy_ht.c:998-1004` | `htphyinit.rs` `tx_pwr_ctl`, `bringup.rs` `wifi10` |
| 30 | b43's fallback coefficients (52, -424, 5612, -1393) are the no-band arm only; NOT used | `phy_ht.c:660-665` | `ident.rs` `pwr_blank` (refusal instead) |

S8 design citations (`wifi_bcma.md` §S8): `b43.h:399-400` (MACFILTER_SELF 0, BSSID 3), `main.c:657-677` (the filter
write), `main.c:679-690` (the BSSID template written through the filter), `xmit.c:399-400` (mac_ctl ACK for a unicast
frame), `xmit.h:128`, `main.c:3972-3985` + `b43.h:608-609` (short/long retry limits 7/4 into SHM scratch), `b43.h:460`
(MACCTL INFRA), `main.c:3117-3123`.

Rows: 30.

## The wire shapes the bench should expect (one `tests wifi`, the image line with UNAOS_WIFI10 armed)

After WIFI9's three lines and WIFI10's `mac-suspend` and `htphy-tx-prep` lines:

```
[wifi11] sprom rev=8 layout=rev8..11(bcma extract_r8 for every accepted revision) core0@0x0c0 maxpwr_2g=<n>(<n>dBm/4) itssi_2g=<n> pa_2g=[0x…,0x…,0x…] core1@0x0e0 … core2@0x100 … — sprom.c:167-173, :221-232, :285-294; ssb_regs.h:537-548
[wifi11] idle-tssi tone=ran samp_stat=0x… idle=[a,b,c] wrote-phy-data=<n> wrote-radio-data=<n> timeouts=0 — phy_ht.c:573-607 (…)
[wifi11] txpctl-setup target=[t,t,t]dBm/4 a1=[…] b0=[…] b1=[…] init_idx=0x19 cmd=[0xe…,0x…,0x…] en_bits=0xe000 targ_pwr=0x…/0x… idle_tssi=0x…/0x… status=[0x…,0x…,0x…] est_tables=3x64(HTTAB16 26..28) tssi_mux=set … -> ON — phy_ht.c:621-729, :609-619, :533-571
:: wifi11: txpctl on=1 target_idx=0x… status=[0x…,0x…,0x…] targ_pwr=[t,t,t]dBm/4 idle_tssi=[a,b,c] tone=ran sprom=ok(rev=8) -> SET ::
[wifi10] tx-setup … (WIFI10's lines, unchanged)
:: wifi10: tx frames=1 kind=probe-req … -> SENT ::
:: wifi10: rx probe-resp=… -> HEARD | SILENT reason=… ::
:: wifi10: end … tx_pwr_ctl=on … ::
```

Refusals, each its own line, the control left OFF and WIFI10 continuing at the fixed index:
`:: wifi11: txpctl on=0 target_idx=- sprom=unread|SUSPECT|BLOCKED|SHORT -> NOT-SET reason=sprom-… ::`;
`:: wifi11: txpctl on=0 target_idx=- sprom=blank(coreN rev=R) -> NOT-SET reason=unprogrammed-pa-block … ::`;
`-> NOT-SET reason=cmd-en-bits-unset` (the enable did not read back). `tone=TIMEOUT` is b43's own tolerated case
(`:409-410`): the setup proceeds, the idle values are whatever the poll read. QEMU models no BCM4331: WIFI1 SKIPs first
and every line above is absent by design.

What a bench read then says: SENT + HEARD under `on=1` closes WIFI10's first suspect. SENT + SILENT with beacons
present and `on=1` moves the suspect off TX power (the b43 power path is now in place) to the frame or the AP's
side (S8's design names the next observable: a unicast exchange with an ACK).
