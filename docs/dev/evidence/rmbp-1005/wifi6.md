# WIFI6 (B439) — S2r SPROM identity, S4i in-boot unwind (MMIO leg), C2's bound from the EROM

Branch `exec-rmbp-wifi6`, cut from 4ead840a (WIFI5 merged). DRIVERS-METHOD §6 shape. No PHY register
is written by anything this arc adds. No new boot asked for: every claim below is a line the next
STAGED armed boot (`UNAOS_WIFI=1 UNAOS_WIFI2=1 UNAOS_WIFI3=1 UNAOS_WIFI4=1`) prints.

## Finding (from the wire, f24/f25 read with `awk 'index($0,"wifi")'`)
- f25 08:23:22Z: wifi2 already moves cfg:0x80 onto ChipCommon (`WROTE cfg:0x80 … new=0x18000000 …
  took=1`) and reads `cc-raw chipid=0x13924331 … MATCH` there, every boot, flown since f13. The SPROM
  shadow is ChipCommon `+0x800` (bcm4331.md §S2r), inside that same 4 KiB window. So S2r needs NO new
  write: it is reads taken while the window already sits on ChipCommon. The only S2r code in the tree
  is `drivers/bcma.rs` under `bcmaS1`, which no flown image carried (§7 row S2r).
- f25: the EROM walk prints `base=` per core but DROPS the size field (`Erom::address` consumes the
  size descriptor and discards it). The d11 core's slave port is `0x18001000` and the next slave base
  is core[2] `0x18002000`: the wire's own gap is 0x1000.
- S4i: the only S5 write step in the tree is `apply_initvals` (S5i, §8). Its pre-image is C2 (WIFI5
  M3), captured BEFORE the upload on the same boot. No restore path exists: a failed initvals step
  today has the reboot as its only unwind.

## The seam (R79)
- The SPROM LAYOUT and its decode go into the shared no_std core `wifi_core::sprom` (the
  `wifi_core::fw` shape, host-tested with `cargo test -p wifi_core`); the kernel reads 220 words and
  hands the slice over. One decoder. `drivers/bcma.rs`'s Group-B S2 keeps its own constants (it is
  a different knob, `bcmaS1`, outside `src/wifi/`) — named for the seat as the fold to do next.
- The unwind is a child of `bringup` (`bringup/unwind.rs`) that uses C2's store (`capture.rs`) — no
  second capture, no second store.

## 1. Capture
C0 (every boot), C1/C2 (WIFI5 M3, staged boot) unchanged. NEW: **C-S2r**, the SPROM shadow (220
words at cc+0x800) dumped as `[wifi6] sprom-dump` rows while cfg:0x80 is on ChipCommon, every wifi2
boot (no firmware needed: the read precedes the upload gate).

## 2. Rung ledger (carried from bcm4331.md §7/§8; this arc changes no status — no rung settled by reading)
| rung | before | this arc | confirm / refute on the next boot |
|---|---|---|---|
| S2r | open | READ BUILT on the flown wifi2 path | confirm: `srom-rev` in 8..11 ∧ `board=` equals PCI ssid device 0x00ef ∧ MAC unicast, not all-0/FF. Refute: shadow all-FFFF/0000 (`-> BLOCKED`, the PA-line mux §S2) |
| S4i | open (MMIO + ucode legs) | MMIO leg BUILT behind `wifi5` | confirm: `[wifi6] unwind step=s5i restored=n/n verified=1` on a boot whose post-check FAILED; refute: verified=0 |
| C2 bound | 0x800 fixed | bound = min(EROM sp0 size, gap to next slave base, 0x1000 aperture) | confirm: `[wifi6] c2-bound … agree=1`; disagreement keeps 0x800 |

## 3. This boot's tree (a staged armed boot)
1. `[wifi6] sprom rev=… board=… mac=… phy=… radio=… -> PLAUSIBLE|SUSPECT|BLOCKED` (end of wifi2/wifi4).
2. `[wifi6] c2-bound d11 sp0-size=… gap=… aperture=0x1000 -> bound=…` before `capture begin`.
3. `[wifi6] s5i post-check psm-run=… mismatch=…/… -> PASS|FAIL unwind=…` after `initvals total`.
4. Only on FAIL with `wifi5` in the image: `[wifi6] unwind step=s5i restored=<n>/<n> verified=<0|1>`.

## 4. Walls known and unapplied
- S4i's UCODE leg (stream C1 back after a prologue, handshake must read 0x0288): needs C1 to read
  NON-DISTURBING on a flown boot first (§7 row S4i rung 0) — not written here.
- The SPROM CRC-8 (§S2): no in-tree transcription of the polynomial table — not computed.
- The PA-line mux write (§S2, ChipCommon chip-control): past the read-only ceiling — named on BLOCKED.
- SPROM rev 8 as cited carries NO radio id field: the line's `radio=` is the wifi4 V4 read, `phy=` the
  core-pre PHY_VER read; the SPROM-side comparison terms are `board=`↔ssid and the MAC.
- S5p (the PHY window), S5r, S5c: as §8.

## 5. Constants
| constant | value | citations |
|---|---|---|
| SPROM shadow offset | cc+0x800 | [ONE-SOURCE: bcm4331.md §S2r in-tree transcription] + `bcma.rs sprom_offset_for` (same tree, same source) — read path only |
| SPROM words / rev word | 220, last word low byte | [ONE-SOURCE: §S2r] |
| IL0MAC / SPID / BOARDREV / ANTAVAIL | +0x8C (3 BE words) / +0x04 / +0x82 / +0x9C | [ONE-SOURCE: §S2r]; SPID corroborated on metal iff it equals the PCI ssid device 0x00ef (f25 S0 MATCH) |
| EROM size field | `(ent & 0x30)`: 0→4K, 0x10→8K, 0x20→16K, 0x30→size descriptor `& 0xFFFFF000` | [LEDGER §S1b / scan.h, the mask already in-tree as AD_SZ_MASK] + the wire gap 0x18001000→0x18002000 (f25) + BCMA_CORE_SIZE 0x1000 in-tree |
| S5i post-check | psm-run=1 ∧ 2·mismatch < records | bcm4331.md §8 row S5i refute: "PSM stops … or mismatch ≈ records" |
| unwind exclusions | 0x3E0..0x3FF (PHY_VER .. radio/PHY data ports) | in-tree D11_PHY_VER 0x3E0, D11_RADIO_ADDR 0x3F6, data 0x3F8/0x3FA; "no PHY register written" |
| restored values | C2's captured words | capture (DRIVERS-METHOD §2: `observed=<efi> source=capture`) |

## 6. Unwind
S2r and the bound: read-only (no unwind owed). S4i MMIO leg: every unwind write puts back the EFI's own
C2 word at an offset the initvals step wrote; offsets C2 did not capture (read-side-effect skips) and the
PHY/radio port block are NOT written and are counted as unrestored. The reboot (S4u) stays the unwind
of the unwind.

## 7. What the next flight reads
`[wifi6] sprom …` (S2r), `[wifi6] c2-bound …`, `[wifi6] s5i post-check …`, and at `tests wifi`:
`:: WIFI6: rungs=27 confirmed=… refuted=… open=… parked=… sprom=<ok|suspect|blocked|unread> c2_bound=<n> unwind=<armed|off> ::`.
`UNAOS_WIFI5` stays OFF the metal line until the seat reads WIFI5's C1 line NON-DISTURBING (rung 0).

## Milestones
- M1: this design. M2: `wifi_core::sprom` + host tests. M3: S2r read on the wifi2 path + the identity
  line. M4: C2's bound from the EROM. M5: S5i post-check + `wifi5` knob + the MMIO unwind. M6: §9 of
  bcm4331.md (the S5 PHY pages owed, by register name) + the `tests wifi` line.
