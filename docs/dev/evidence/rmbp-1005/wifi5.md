# WIFI5 (B415) — the BCM4331 ladder rebuilt under DRIVERS-METHOD

Branch `exec-rmbp-wifi5`, cut from f1eea8d9. The ladder doc is `docs/dev/OS/06_NETWORK_STACK/bcm4331.md`
§7 (the rung ledger); this file is the arc's design, per DRIVERS-METHOD §6.

## Finding (from the wire already captured — no boot asked for)

The upload the ledger calls "never flew" flew and SUCCEEDED on every boot of flights 13, 14, 15, 17, 18,
19, 20 (f13 33884ms: `upload verify … => MATCH`, `ready=1 polls=10`, `ucode upload words=9938
crc=0xb2bd00d3 psm=1 rev=666 -> UPLOADED`). Every boot after one of those reads the platform firmware's
resident image again (`core-pre macctl=0xc0020403 psm-run=1`, `shm-probe … shared[+0x00]=0x0288`): the
reboot IS the unwind, confirmed nine-plus times. The wrapper reset alone is NOT an unwind (f13
`prologue post macctl=0x80000000 psm-run=0`). Flights 21–25 skipped the upload only because the UnaFS card
image carries no firmware set (`INCOMPLETE 0/3 … label='UNAOS'`). The wifi4 radio-id `verdict=INVALID` is
a gate fault, not a device fault: the V4 read order returns 0x0205917f = radio 0x2059, mfg 0x17F.

## §6 sections

1. **Capture** — C0 on the wire every boot; C1 (resident ucode memory via routing 0x0300 before the
   prologue) and C2 (d11 MMIO 0x000–0xFFF + shared 0x0000–0x0FFF, the EFI's working MAC/PHY state) planned,
   bcm4331.md §7 "Capture plan".
2. **Rung ledger** — bcm4331.md §7: 27 rungs, confirmed 13, refuted 2, parked 3, open 9.
3. **This boot's tree** — bcm4331.md §7 "decision tree".
4. **Walls known and unapplied** — bcm4331.md §7 last subsection.
5. **Constants** — every write-path constant this arc adds cites [SPEC-V3]/[SPEC-V4] + the capture line;
   the wrapper offsets (RESET_CTL, IOCTL) are `[ONE-SOURCE: capture f13 prologue took=1]`.
6. **Unwind** — reboot (confirmed, S4u); in-boot re-upload of the C1 pre-image (S4i, open).
7. **What the next flight reads** — `:: WIFI5: …` at `tests wifi`, then the decision tree's lines in order.

## Milestones (all committed on exec-rmbp-wifi5)

- **M1**: the rung ledger (bcm4331.md §7) and this design. No code.
- **M2**: `src/wifi/ladder.rs`, the ledger as one typed line at `tests wifi`:
  `:: WIFI5: rungs=27 confirmed=<13|14> refuted=2 open=<9|8> parked=3 upload_unwind=reboot-proven staged=<n>/3 uploaded_this_boot=<0|1> -> READY|NOT-READY reason=firmware-not-staged … ::`.
  Row F is measured live (`staged_count()==3`). The refusal names `UNAOS_WIFI_FW_PATH` as it will
  exist; the arroyo block is the seat's, re-added at merge17.
- **M3**: `src/wifi/bringup/capture.rs` (a child of `bringup`). It runs C2 (d11 MMIO 0x000-0x7fe x16,
  shared 0x0000-0x0fff), C1 (resident ucode, the words our stream overwrites) and its discriminator,
  and S5i rung 0 (initvals vs C2). It is called once, just before `upload BEGIN`; the upload sequence
  itself is unchanged.
- **M4**: the wifi4 radio-id gate decodes the V4-order word (R89). `v3-agree=` is printed and no
  longer gated. `unwind-selftest ADVISORY` replaces `PASS` when `discriminating=0`.
- **M5**: bcm4331.md §8, the S5 rung ledger (rung 0 S5-0, S5i-0, S5p) and the first S5 boot's tree.
  C3 is a post-upload+initvals MMIO diff against C2, called from `phy_once` after its gate.

## Notes for the fold
- The integration tip f1eea8d9 does NOT compile on the x86 seat line: `syscall.rs:30248 appquit_pid`
  iterates `0..MAX_PROCS`, which WINDOWCAP3 removed. The legs here were run with that one token
  locally replaced by `procs_rows()` (the file's own idiom) and then reverted. It is not in this branch.
- The initvals path (`apply_initvals`, B338 WIFI1) has never flown. On the next staged boot it is the
  first S5 write on metal (§8 S5i). A design question for the seat: gate it on `s5i-rung0` once that
  line has been read on one boot.
- The spec wiki was unreachable from this sandbox (egress blocked). S5p's PHY-window citation pass is owed.

Owed beyond this arc: S4i's restore boot, S5p, S5c from C2 + S5p, S2r on a flown image, S6-S8.
