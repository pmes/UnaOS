# WIFI1 — the BCM4331 from firmware to a scan verb (B338)

Branch `exec-rmbp-wifi1`, cut from 53423b84. Licence rule for every file this arc touches: facts come
only from the b43 open-specification wikis (`[SPEC-V3]` bcm-specs.sipsolutions.net, `[SPEC-V4]`
bcm-v4.sipsolutions.net) as already recorded in `bcm4331.md` §S4-W5/§S5, from `[METAL]` captures, from
`[LEDGER]` (in-tree, metal-corroborated), and `[PUBLIC]` IEEE 802.11 frame formats. NO Linux driver
source was read. The firmware blob is never in the tree (R52 shape, CLEAN_ROOM_POLICY §4): the user
places it on the volume.

## Finding

Boot 20 (f20-boots.log) already flew the destructive S4-W5 upload at boot under `UNAOS_WIFI3=1`, and it
WORKED: `upload verify … => MATCH`, `ready=1 polls=10`, handshake `rev=666 patch=2 date=0xb217 time=0x09e7`,
`-> UPLOADED`, `phy-alive verdict=PHY-ALIVE`. It also settled the radio-id read order on metal: the
V3 order (hi then lo) read `0x0000917f`, the V4 order (lo then hi) read `0x0205917f` = rev 0, ver
`0x2059`, mfg `0x17f` — so the LOW read latches the HIGH half, the V4 order is the right one, and the
radio is the 2059 the ladder's prose named. What is missing is everything after: initvals were never
written, nothing is on-demand (R80: the ladder ran at boot), and there is no verb.

## The seam

`unaos/libs/sys/wifi_core` — a `no_std` shared core (the midden_core shape): the firmware container
codec (header + record walk, one implementation), and the IEEE 802.11 management-frame codec (beacon /
probe-response parse: SSID, BSSID, channel, RSSI carried in, capabilities, RSN; open-system auth and
association-request builders; data-frame <-> Ethernet LLC/SNAP). Host-tested (`cargo test -p
wifi_core`). The kernel's `wifi/d11.rs` is the driver that feeds it; `wifi/verb.rs` is the shell verb.

## Milestones

* **M1** — `wifi up`: load `ucode29_mimo.fw`, `ht0initvals29.fw`, `ht0bsinitvals29.fw` from
  `/system/firmware/bcm4331/` through the mount table, print name + bytes + sha256 for each, refuse by
  name when absent (with the bench command). Prologue + routing 0x0300 stream + readback + Ready +
  handshake (S4-W5 facts 1–5), then the initvals and bsinitvals as per-register MMIO writes after Ready
  (fact 6, [SPEC-V3 InitialValues]), every write read back. Witness `[wifi] ucode=ok rev= date=`.
* **M2** — d11 init: PHY id and radio id read-only (V4 order, metal-settled), MAC enable bit (fact 2),
  and the parts with no legal citation in the tree REFUSED by name (IRQ mask offset, DMA engine
  registers + descriptor format, RX header, promiscuous bit).
* **M3** — `wifi scan`: the beacon parser is real and tested; the device leg reports `scan=0` with the
  reason (no RX path and no HT-PHY channel tune). `tests wifi` prints
  `:: WIFI1: ucode=<ok|refused> d11=<up|down> scan=<n> -> PASS|FAIL ::`, SKIP without firmware.
* **M4** — `wifi join <ssid>`: open-system auth + association frames built and host-tested; the
  device leg refuses (no TX path). smoltcp device beside usbnet and DHCP: OWED.

## Witness

`[wifi] fw ucode=… sha256=…` ×3 · `[wifi] ucode=ok rev=666 date=2011-02-23 …` · `[wifi] initvals
wrote=… mismatch=…` · `[wifi] d11 radio-id=0x0205917f ver=0x2059 …` · `:: WIFI1: … ::`.

## Owed

DMA rings / RX / TX (needs [SPEC-V4 802.11/DMA] and the RX-header page, which the egress proxy blocked
this session), the HT-PHY channel tune (no legal source carries it, §S5(c)), association on air, smoltcp
device + DHCP, WPA2 (CRYPTOCORE: SHA-1, HMAC-SHA1, PBKDF2-HMAC-SHA1, the 802.11i PRF, AES key wrap
RFC 3394, AES-CCMP).
