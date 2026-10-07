# WIFI7 (rmbp-ledger B505) — flight 27's next rungs on the BCM4331

Cut from 668cdd95 (the merge19 fold). Read from `flight27/f27-boot1.log` (awk on `wifi`):
`:: WIFI5: … uploaded_this_boot=1 -> READY ::`; the handshake `shm[0x0,0x2,0x4,0x6]=[0x029a,0x0002,0xb217,0x09e7]`
after the shm-probe's pre-upload `shared[+0x00]=0x0288`; `phy-alive … verdict=PHY-ALIVE`, `phy-reset REFUSED`,
`channel-tune REFUSED`, `rx-chain NOT ATTEMPTED`. MACCTL after the start is `0x80020402`: PSM_RUN set,
MAC Enabled (bit 0, [SPEC-V4 802.11/Registers]) CLEAR.

## Findings

1. **PHY reset (S5r): DECLINED, by name.** The brief's premise that `drivers/bcma.rs` carries the AI
   wrapper layout "from the SPEC-V4 agent pages" does not hold: `drivers/bcma.rs:200-211` cites Linux
   `include/linux/bcma/bcma.h` (`BCMA_IOCTL`, `BCMA_RESET_CTL`, `BCMA_IOCTL_CLK/FGC`) and carries no
   `[SPEC-V3]`/`[SPEC-V4]` tag at all; `bringup.rs` tags the same block `[EXT-CORROBORATION-WEAK]` and its
   `IOCTL_PHY_RESET` "Group-B, recorded" (b43's `B43_BCMA_IOCTL_PHY_RESET`). bcm4331.md §S4-W5 fact 7:
   the AI wrapper is in NEITHER spec generation. There is no page to cite, so no pin. What the metal
   says, printed as an ADVISORY decode on wifi4's own line: `[SPEC-V4 802.11/CoreFlags]` puts PHY Clock
   Enable / PHY Reset / MAC-PHY Clock at TMSLOW bits 18/19/20, i.e. 16 above the generic CLK at 16; the
   measured AI IOCTL `0x2055` carries CLK at bit 0, so under the same offset it reads phyclk=1
   phyreset=0 macphyclk=1 — the shape of a running PHY, which PHY-ALIVE corroborates. Coherence, not a
   pin. Second, independent reason: S5-0 (bcm4331.md §8) is the premise that the EFI's PHY/radio tune
   survives S4; a PHY reset is the one write that would destroy it, and with S5c parked nothing could
   rebuild it. Declined even if the bit were pinned, until S5c has a table.
2. **The ucode's own revision from SHM (S5u).** Re-read after the initvals through routing 0x0001 (two
   selects; `SHM_CONTROL`'s pre-image read first and restored last). **Correction to M1 of this arc**:
   M1's line claimed `host-wrote-shared=0`; the f27 wire refutes that — the staged initvals and
   bsinitvals WRITE `SHM_CONTROL` with shared routing (`s5i-delta bsinitvals 160=10003/10000 …`,
   `initvals 160=3010005/… 160=301000d/…`), so the host does write shared memory. What holds: the
   lowest dword those selects name is 3 (byte 0x000c) and the window only auto-increments upward, so
   word 0x0000 is not a host write. The line now COMPUTES that floor from the staged records
   (`host-shared-selects=<n> host-shared-low=<byte>`) and says EXECUTING only when the floor is above
   word 0, the word moved from C2's pre-image, and psm-run=1.
3. **Channel tune (S5c): DECLINED once, with the tables owed by name**, folded onto wifi4's existing
   line (its `REFUSED reason=htphy-2059-tables-UNPINNED` token kept for the f13–f27 history greps):
   radio-2059 init table, radio-2059 per-channel tune tables (2.4 GHz and 5 GHz), HT-PHY (type 7)
   init tables, the HT-PHY table-RAM ADDRESS/DATA port map, HT-PHY per-channel baseband coefficients.
4. **The RX rung without a tune (S5d on the EFI channel).** No page the tree carries pins an SHM offset
   for an RX-frame counter, so `rx_frames_5s` cannot be a decoded count (R83). The rung that IS
   takeable is the measurement that finds the counter: at `tests wifi`, the whole shared segment
   (0x0000–0x0fff) is read at t0 and t0+5 s (TSF-timed, the wait sleeps inside a task), read-only bar
   the select, with MAC Enabled printed (it is 0 on f27: the ucode has not been told to receive — the
   discriminator for an all-quiet answer). Movers are printed by offset; a word that advances with the
   antenna in air and not shielded is the counter S6 cites. The boot snapshot gives since-boot movers.
   The watch re-reads `cfg:0x80` live first and refuses if the window is not on the d11 core.

## Seam

CHARTER: Kernel — driver (the d11 is the kernel's device; no CODEX §2 handler owns a radio register).
New file `unaos/crates/kernel/src/wifi/bringup/live.rs`, a child of `bringup` on its covenanted
accessors; its ONE written register is `SHM_CONTROL` (the read-path selector, the shm-probe's class),
counted in wifi2's `Writes::core_regs` (so the audited `wrote-core-regs` on the wifi2 end line includes
it) and called AFTER wifi4's end line, whose audit says wifi4 writes no SHM_CONTROL.

## Witness (what the next metal boot prints, same knob line as flight 27)

```text
:: wifi4: phy-reset REFUSED reason=ai-wrapper-phy-reset-bit-UNPINNED -> DECLINED(S5r) also=s5-0-efi-tune-would-be-destroyed ioctl=0x00002055 advisory(CoreFlags>>16) phyclk=1 phyreset=0 macphyclk=1 — …
:: wifi4: channel-tune REFUSED reason=htphy-2059-tables-UNPINNED -> DECLINED(S5c) owed=radio2059-init,radio2059-chan-2g,radio2059-chan-5g,htphy-init,htphy-tableram-ports,htphy-chan-bb — …
:: wifi4: rx-chain NOT ATTEMPTED reason=depends-on-channel-tune instead=s5d-on-efi-channel at=tests-wifi — …
:: wifi4: end ok=1 stage=phy … wrote-core-regs=0(audited …) …
[wifi] ucode rev=666 patch=2 from SHM pre=0x0288 changed=1 host-shared-selects=<n> host-shared-low=0x000c psm-run=1 macctl=0x80020402 shm-ctl-restored=MATCH -> EXECUTING — …
:: wifi2: … end ok=1 … wrote-core-regs=<f27's 10463 + 4>(audited … WIFI7's live.rs SHM_CONTROL selects + restore are counted here) …
--- tests wifi ---
[wifi] rx-watch window=5000ms tsf-delta=<us> full=1 mac-enabled=0 psm-run=1 movers=<n> up=<n> selects=3 shm-ctl-restored=MATCH — …
[wifi] rx-watch mover shm+0x<off> t0=0x<..> t5=0x<..> d=<signed>     (up to 16)
[wifi] rx-watch since-boot tsf-delta=<us> movers=<n> up=<n>
:: WIFI5: rungs=27 confirmed=14 refuted=2 open=8 parked=3 … uploaded_this_boot=1 ucode_rev=666 rx_frames_5s=unpinned(movers=<n>) -> READY ::
```

Refusal shapes: `[wifi] ucode rev REFUSED reason=wifi3-upload-not-proven` (no upload this boot);
`-> UNPROVEN(host-may-have-written)` (a staged initvals select at dword 0, or an undecoded 16-bit
write to the control word); `[wifi] rx-watch REFUSED reason=no-executing-ucode-this-boot` /
`reason=window-not-on-d11 cfg80=… d11=…`; `ucode_rev=none rx_frames_5s=not-watched` on a build
without wifi4.

## open=, honestly

`open=` does NOT drop on this arc: no §7 row lands. S5r and S5c stay parked (declined by name, with
the owed pin and tables on the wire); S5u (the ucode executes, read back from SHM) and S5d-on-EFI are
not §7 rows, and `ladder.rs`'s table is §7 row for row. Owed to the seat (another file, head §3): a
bcm4331.md §7 row `S5u | ucode executes | shared word 0x0000 moves from C2's pre-image to the image's
revision with psm-run=1 and no host select at dword 0 | confirm: [wifi] ucode … -> EXECUTING; refute:
NOT-EXECUTING`, and with it a live S5u row in `ladder.rs` (confirmed when `live::watch` returns a rev) —
then `rungs=28 confirmed=15`.

## Owed

The SHM RX-counter offset (a Group-A page, or the movers above, antenna in air vs shielded); MAC
enable (`[SPEC-V3 ChipInit]` steps 9 onward); S5r (a Group-A AI IOCTL map); S5c (the tables above); S6.
