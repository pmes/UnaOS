# Zenbook ledger — every finding, ticked once

> Arch ledger for the ASUS Zenbook S 14 OLED UM5606W track (AMD Ryzen AI 9 HX 370 "Strix Point", Radeon 890M, MediaTek
> MT7925 Wi-Fi 7; runs on an external monitor). Cross-arch and shared-seam items live in `docs/dev/LEDGER.md` (ids `S<n>`,
> `P<n>`, and the seat-prefixed `SO`/`SP`/`SR`); rows below cross reference it by id. Ids here are `Z<n>` — orin's arch
> ledger uses A-D, rmbp's A, B, E and pi's PI, so a distinct prefix keeps the files mergeable without renumbering.

> Created 2026-10-27 by ZENBOOKMT7925 (wave 36) because ZENBOOKSURVEY, which owns this file, was in flight beside it; the
> seat unions the two at the fold (Peter 2026-10-27: "begin support for the um5606w hardware").

> Rule (Peter, 2026-09-05): **an arc that fixes, flies, or drops a ledger item ticks it here in the same commit.**

Status: **open** · **fixed-unflown** (in tree, not on metal) · **flown** (scored on the wire) ·
**landed** · **dropped** (ruled out, with the ruling).

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| Z5 | **ZENBOOKMT7925 — the MediaTek MT7925 (Wi-Fi 7, 14c3:7925 and 14c3:0717) S1 probe and its firmware pins.** `drivers::mt7925::service()` rides the three storage-ready passes (main.rs, beside the Broadcom service call; LINE-NEUTRAL) after `boot::services_gate("wifi")`: the PCI registry (no walk; absent = parked with no line), config COMMAND/REVISION/BAR0, one mapped page at BAR0+0x0e0000 with two reads (LPCTL ownership, CONN_ON_MISC firmware state) and no write; the linux-firmware pair hashed from the ESP FW/MT7925/ against `unaos/firmware/mt7925.pins` (arroyo MT7925-FW via the new `fw_stage_pinned`; pins from linux-firmware 5a3f625f). Wire: `[mt7925] probe at= id= rev= bar0= mem= lpctl= own= fwstate= fw= patch_sha= ram_sha= -> parked`. Survey, the SoftMAC seam and the ladder in `docs/dev/OS/06_NETWORK_STACK/mt7925.md`; net.md BP-net-8 and BP-net-9 changed. Owed: S2..S10 (driver-own and the chip id through the L1 remap, WFSYS reset, the WFDMA rings, the MCU patch, the RAM code and FW_START, the MAC, the radio backend trait, scan, auth/assoc, data, WPA2); FWPINONE (one pins reader for b43, kepler, mt7925); FWSTAGEONE (WIFI-FW and KEPLER-FW onto `fw_stage_pinned`); HWROW-MT7925 (`hw::pci_rows` names the radio); FWCENSUS-Z (firmware-census learns Z and PI ids); the Zenbook image line carries UNAOS_MT7925_FW_PATH. | zenbook | Zenbook bench | fixed-unflown — code-only, uncompiled at the commit; QEMU and the rMBP print no `[mt7925]` line | `unaos/crates/kernel/src/drivers/mt7925.rs`, `unaos/firmware/mt7925.pins`, `docs/dev/OS/06_NETWORK_STACK/mt7925.md` | — |
