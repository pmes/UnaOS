# ZENBOOK FLIGHT 1 — the bench brief (Peter's go, 2026-10-27)

Peter 2026-10-27: "we can test the zenbook". The recipe is `docs/dev/OS/01_BOOT_HAL/um5606w.md` §6 (ZENBOOK-FIRSTBOOT,
zenbook-queue.md row 1); this page is the flight's checklist against the tip. Every Z row is `fixed-unflown` (Z2..Z69): this
boot is the first witness for all of them.

## 1. Build and write the card

1. `cd unaos && UNAOS_MT7925_FW_PATH=<bunker dir> ./arroyo esp-zenbook` (Z6). Expected: `⚡ esp-zenbook: line=scripts/image-line-zenbook.env …`,
   then esp-x86's `⚡ kernel features:` naming nvme, ioapic, wc, login, unafs, hda, wifi (none of smc, nvidia-kepler*, intel-ivb, hda-tone,
   wifi2..wifi10, bt*, ahci*). The card is `target/unaos-x86-card.img`; write it to a USB stick (the `UnaOS_Installer` card face is in
   flight as SR414 — this flight still uses `dd`).
2. External monitor on USB-C (DP alt mode) or HDMI; the FTDI FT232 on a USB port; the stick on another.
3. F2/Esc into the firmware: Secure Boot OFF; boot the stick.

## 2. What the card does by itself

The card runs the ONE flight list (`unaos/fixtures/TESTS.LIST`, Z33/Z37): the generic fixtures plus the `@zenbook` verbs
`platform ec ecbattery nvme i8042 dwi2c dwi2ctp dma hdacensus gopshot`. Expect their `:: <VERB>: … -> PASS|FAIL ::` lines on
serial once the xHCI is up (everything before that is glass only: photograph it; um5606w.md §6 step 5).

## 3. What to type by hand (the QEMU lane types these; on metal it is you)

In this order, each a `tests <verb>` at the shell:

| verb | witnesses (ledger) | the line to read |
|---|---|---|
| `platform`, `ec`, `ecbattery`, `eclid` | Z8/Z20/Z27 | `[ec] …`, the lid and battery words |
| `ecqnames` | Z51 | the EC query names |
| `nvme` | Z2/Z9/Z22/Z24/Z31/Z45/Z49/Z52 | `[nvme] ctrl … -> ready`, MSI-X, the IRQ wait, preempt-off |
| `preemptaudit` | Z57/Z61/Z62/Z65/Z66/Z69, B1035 | `:: PREEMPTAUDIT: sites=176 … converted_unafs=1 … loan_wait= ptp_path= loan_shed= -> PASS ::` |
| `dwi2c`, `dwi2ctp`, `dwi2cacpi` | Z14/Z18/Z26/Z32/Z42/Z50 | the touchpad over DesignWare I2C, the fast-mode timings |
| `hdacensus`, `acp` (play) | Z25/Z43/Z47/Z60 | the ACP codec, a tone out, the depop |
| `gopshot` | Z4/Z10/Z28 | the GOP frame |
| `suspend` | Z48 | the S3 round trip (expect a refusal by name if the EC path is not there yet) |
| `wifiscan` | Z34 | `[mt7925] s7 scan …` the SSIDs seen |
| `wpa2` | Z55 | the WPA2 known-answer legs, no radio needed |
| `wifijoin <ssid> <pass>` | Z38/Z46/Z55/Z56/Z59/Z63/Z64 | `s8 join`, `s9 dhcp`, `s10 wpa2 … -> SECURED`, then the watch; power-cycle the AP once: `s9 rejoin … secured=1`; wait for a group rekey: `rekeys=` |
| `wifileave` | Z68 | `[mt7925] s9 left reason=operator keys=cleared pmk=zeroized deauth=sent …` |
| `lockreg`, `hidstall`, `memline` | B1034/B1036, B1028/B1037, B1032/B1041 | the LOCKREG tail (`release_stale=0 forceunlock_kat=ok`), MASKSCOPE `wrapsite_ok=`, the MEMLINE logout leg |
| `unafs` | B1058/B1061/B1063 | `[unafs] bind_now kat=ok r=ok bound=1 masked=0 …` then `:: UNAFSX86-T: root=unafs … -> PASS ::` (also the card list's last light line since B1063: type it only when the list's line is missing) |

## 4. What to bring back

- The whole serial capture as `docs/dev/evidence/zenbook-1/boot1.log` (and `boot2.log` for a second power cycle), plus the glass
  photos for the pre-xHCI lines. The seat lands them, scores every Z row flown, and writes the landing report.
- Three numbers off the glass if the serial door never opens: `[xhci] controllers=<n> picked=<i> bdf=`, and whether the FTDI's
  port is on that controller (rebuild with `UNAOS_XHCI_PICK=<i>` for the next i, Z7).
