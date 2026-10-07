# WIFI7 (rmbp-ledger B505) — flight 27's next rungs on the BCM4331

Cut from 668cdd95 (the merge19 fold). Read from `flight27/f27-boot1.log` (awk on `wifi`):
`:: WIFI5: … uploaded_this_boot=1 -> READY ::`, the handshake `shm[0x0,0x2,0x4,0x6]=[0x029a,0x0002,0xb217,0x09e7]`
after a pre-upload `shared[+0x00]=0x0288`, `phy-alive … verdict=PHY-ALIVE`, `phy-reset REFUSED`,
`channel-tune REFUSED`, `rx-chain NOT ATTEMPTED`. MACCTL after the start is `0x80020402`: PSM_RUN set,
MAC-enable (bit 0) CLEAR.

## Findings

1. **PHY reset (S5r): DECLINED, by name.** The brief's premise that `drivers/bcma.rs` carries the AI
   wrapper layout "from the SPEC-V4 agent pages" does not hold: its `WRAP_IOCTL`/`RESET_CTL` offsets and
   `IOCTL_CLK/FGC` cite Linux `include/linux/bcma/bcma.h`, and the core bits cite b43's
   `B43_BCMA_IOCTL_*` — Group B, which CLEAN_ROOM §2 keeps out of a write path in `src/wifi/`. No
   Group-A page for the AI IOCTL exists in the tree. What the metal DOES say, printed as an ADVISORY
   decode: `[SPEC-V4 802.11/CoreFlags]` puts PHY Clock Enable / PHY Reset / MAC-PHY Clock at TMSLOW bits
   18/19/20, i.e. 16 above the generic CLK/FGC at 16/17; the measured AI IOCTL `0x2055` carries CLK at
   bit 0, so under the same 16-bit offset it reads PHY-clock=1, PHY-reset=0, MAC-PHY-clock=1 — the
   shape of a running PHY, which PHY-ALIVE corroborates. That is coherence, not a pin. A second reason
   stands on its own: S5-0 (bcm4331.md §8) is the premise that the EFI's PHY/radio tune survives our S4
   sequence; a PHY reset is the one write that would destroy it, and with S5c parked nothing could
   rebuild it. So the reset is declined even if the bit were pinned, until S5c has a table.
2. **The ucode's own revision from SHM (S5u).** Re-read after wifi4 through routing 0x0001 (the read
   path, one `SHM_CONTROL` select): `[wifi] ucode rev=666 patch=2 from SHM pre=0x0288 host-wrote-shared=0`.
   The host never writes shared memory (the stream goes to routing 0x0300), so a word that moved from
   the EFI's 0x0288 to our image's revision was written by the uploaded microcode: it executes.
3. **Channel tune (S5c): DECLINE, once, with the tables owed by name**: radio-2059 init table,
   radio-2059 per-channel tune table (2.4 GHz and 5 GHz), HT-PHY (type 7) init tables, HT-PHY table-RAM
   ADDRESS/DATA port map, HT-PHY per-channel baseband coefficients. None is in [SPEC-V3]/[SPEC-V4].
4. **The RX rung without a tune (S5d on the EFI channel).** No legal source pins an SHM offset for an
   RX-frame counter, so `rx_frames_5s` cannot be a decoded count (R83: no chicken wire). The rung that
   IS takeable is the measurement that finds the counter: at `tests wifi`, the whole shared segment
   (0x0000–0x0fff) is read at t0 and t0+5 s (TSF-timed), read-only, with MAC-enable printed (it is 0:
   the ucode has not been told to receive, which is itself the discriminator for a zero). Movers are
   printed by offset; a word that advances only with the antenna in air is the counter S6 cites.
   Also the boot snapshot (taken at the end of wifi4) against `tests wifi` time: movers since boot.

## Seam

Kernel — driver (the d11 is the kernel's device; no handler owns a radio register). New file
`unaos/crates/kernel/src/wifi/bringup/live.rs`, a child of `bringup` using its covenanted accessors;
its ONE write is `SHM_CONTROL` (the read-path selector, the shm-probe's class), counted.

## Milestones

- M1 this design.
- M2 `live.rs`: boot-side `rev_line` + `decline_lines` (S5r advisory decode, S5c tables owed) at the
  tail of `phy_once`, the shared-segment snapshot; capture.rs notes the pre-upload word.
- M3 `tests wifi`: the 5 s watch + since-boot movers; the WIFI5 line gains `ucode_rev= rx_frames_5s=`.

## Witness (what the next metal boot prints)

```text
[wifi] ucode rev=666 patch=2 from SHM pre=0x0288 changed=1 host-wrote-shared=0 psm-run=1 -> EXECUTING
[wifi] phy-reset DECLINED reason=ai-ioctl-core-bits-no-groupA-source+s5-0-efi-tune-would-be-destroyed ioctl=0x00002055 advisory(CoreFlags>>16) phyclk=1 phyreset=0 macphyclk=1
[wifi] channel-tune DECLINED reason=htphy-2059-tables-UNPINNED owed=radio2059-init,radio2059-chan-2g,radio2059-chan-5g,htphy-init,htphy-tableram-ports,htphy-chan-bb
[wifi] rx-watch window=5000ms tsf-delta=<us> mac-enabled=0 psm-run=1 movers=<n> up=<n>
[wifi] rx-watch mover shm+0x<off> t0=0x<..> t5=0x<..>          (up to 16)
[wifi] rx-watch since-boot elapsed=<s> movers=<n>
:: WIFI5: rungs=27 … uploaded_this_boot=1 ucode_rev=666 rx_frames_5s=unpinned(movers=<n>) -> READY ::
```

## Owed

The SHM RX-counter offset (a Group-A page, or the movers above with the antenna in air vs shielded);
MAC enable (`[SPEC-V3 ChipInit]` steps 9+); S5r (a Group-A AI IOCTL map); S5c (the tables above); S6.
No ledger row moves on this arc: S5r and S5c stay parked (declined by name), S6 stays open.
