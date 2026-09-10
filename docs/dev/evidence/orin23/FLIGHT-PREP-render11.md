# FLIGHT-PREP render11 — orin 23, 2026-09-08 ~23:25Z (Peter back at the main bench; focus on orin)

Everything below was measured this session; nothing is relayed. The flight sequence itself is
`~/unaos-bench/scratch/orin22/stage11/FLIGHT-render11.md` (§3 card write, §4 serial, §5 score, §6 FRIEND-DIFF).
Two corrections to that file, found this session:
  * §2's census literal `[sdmmc]` is stale — the source, the elf and the baton all spell it `:: SDMMC:` (1 in both images).
    scorer11.sh keys only on the RETIRED `[sdmmc] root` (0 in both), so nothing scores wrong.
  * §4's butler pid 81753 is gone. Restarted this session: host pid 11670 holds /dev/ttyACM0
    (`flatpak-spawn --host lsof -t /dev/ttyACM0`), raw.log shows `line-butler holds /dev/ttyACM0 @ 2026-09-08T23:17:42Z`.

## Two staged images, SAME commit (exec-orin22-bootroot 600887c2, clean tree, stamp `UNAOS-BUILD-STAMP-1:600887c2`, no -d suffix)

| dir under ~/unaos-bench/flash/orin/ | build line | effective features | kernel.elf sha256 | max_vaddr |
|---|---|---|---|---|
| `render11-20260908T2321Z-600887c` (THE FLIGHT IMAGE, FLIGHT-render11 §1) | fifteen knobs = render10's line minus the deleted UNAOS_SDMMCROOT | render10 BANNER-BASELINE minus `sdmmcroot` exactly (diff = one line `< sdmmcroot`); no ga10bprobe1/2 | 57ae5ec38b933e8eaddf7d218e8f66e86b81b49f65b3ca4c9d5d187a7286d765 (3229696 B) | 0x34b748 |
| `render11plain-20260908T2321Z-600887c` (FALLBACK — orin 22's gate build) | plain `./arroyo esp-jetson`, no knobs | ehcihid,tegra,tegrasmp,bsptick,bsprun,sdmmc | 6dbeda7d1ae8e942fb764e13a175168e1d8a10e235abf0ef9ee5fd9d15c58c9d (2016152 B) | 0x249f38 |

Why two: orin 22 staged the PLAIN gate artifact as "the render11 candidate" (KNOBS-render11.env says so), but the
round's own flight doc §1 prescribes the fifteen-knob line. Plain boots no desktop, no witness, no net, no TCU RX:
it answers the root question only and every render10 carry leg scores NOT-EXERCISED, and FRIEND-DIFF's prtscr
control does not exist in it. The knob image answers root AND the carry legs. Recommendation: fly the knob image.

Both: `[vfs] root = boot volume` 1 · control `crates/kernel/src/` 54 (knob) / 36 (plain) · retired `[sdmmc] root` 0 ·
`:: SDMMC:` 1 · `volume mounted /volumes/` 2 · `multiple-kernels` 0 · `aliased=ambiguous` 0 (the known gap, below).

## KNOWN GAP flown scored-known (baton orin-23; integrate/BRIEF-AMENDMENT-01.md item 5)
Clone-vs-alias is NOT in: two cards sharing (num_blocks, BS_VolID) dedupe to ONE /volumes mount with `aliased=`
naming one device. Root is unaffected (first found). ON THIS BENCH IT SHOULD NOT FIRE: the render9 wire shows the
reader card's FAT serial 0xde001a13 and the slot card's BS_VolID 0xabfbdefa — different serials, so they are not
clones by DiskId. If the wire shows `aliased=` naming anything other than the same-card `usb->global` pair, that
is the gap firing and it is the finding. Fix stays landing-blocking (before exec-orin22-bootroot lands).

## Peter's steps (host, sudo) — card must be in the HOST reader (/dev/mmcblk0; lsblk on the host shows only nvme right now)
```bash
sudo ~/unaos-bench/tools/media-writer.sh --src render11-20260908T2321Z-600887c --target /dev/mmcblk0 --allow-geom-absent
```
(dry run: every read-only check, nothing written; expect C7-GEOMETRY announced-unchecked, everything else PASS) then the same line + `--write`.
Then: card back in the Orin's reader, power on. The butler is already capturing; the seat scopes the boot and scores.

## After the boot (seat)
1. Window ONE boot out of `~/unaos-bench/capture/line-acm0/orin.log` (FLIGHT §4 awk form) → `scratch/orin23/boot-render11-A1.log`.
2. `scorer11.sh <wire> ~/unaos-bench/flash/orin/render11-20260908T2321Z-600887c/kernel.elf` — the FLASHED elf.
3. render10's scorers alongside for the carry legs (read orin21 BULLETIN §31's two false reds first).
4. FRIEND-DIFF three-boot form (§6): A1 friend absent, A2 friend absent, B friend present; normalizer from A1 vs A2, frozen, applied to A1 vs B.

## Bench state at prep
- Card: still render9 (`render9-20260907T1157Z-a62188c`, written 2026-09-07T11:58Z) until the write above.
- Butler: host pid 11670. Peers: rmbp 17 (support, told this turn), pi 9 not running.
- Throwaway build tree: `~/unaos-bench/scratch/orin23/wt-render11` (detached 600887c2, clean) — remove after the flight.
- Build log: `~/unaos-bench/scratch/orin23/build-render11-knobs.log` (EXIT=0). Staging script: `scratch/orin23/stage-render11.sh`.

## Addendum ~23:55Z — MATRIXPAR push cannot execute as written
origin exec-orin22-matrixpar=ce0d992b is an orphan of the real tip 5c1f13d0 (not an ancestor; rejected P=6 default aboard). Plain push = non-FF; force forbidden. Proposal to Peter, rmbp-17-acked: `git push origin 5c1f13d0:refs/heads/exec-orin22-matrixpar2`, old ref retired by Peter only. Not boot-related.

## Addendum ~23:50Z — the writer moved and was repaired
Peter: "that really is a dumb file name" + "anything in ~/unaos-bench/scratch/ could be deleted in 10 seconds". The writer is now `~/unaos-bench/tools/media-writer.sh` (scratch/orin20/cardready/load-card10.sh left as the record, plus a .bak). Fixed there: sudo HOME (resolved via SUDO_USER), sandbox-only host-spawn prefix (under sudo there is no session bus — C6/C9 had passed VACUOUSLY on Peter's dry run), mount of partition p1 instead of the whole disk, harvest to flash/orin/harvest/, chown of harvest+FLIGHTID to the invoking user. Selftest 27/27, dry run of the staged dir PASS 8/9 (C7 announced unchecked).
