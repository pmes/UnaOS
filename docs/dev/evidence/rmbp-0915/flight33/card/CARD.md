# CARDREAD — the flight-33 card (image 25, rmbp12flight33) read on the bench, 2026-10-13

Method: Peter's `sudo dd if=/dev/mmcblk0p2` (the UnaFS partition, 4 GiB; the boot FAT p1 untouched) to `~/unaos-bench/scratch/rmbp-0915/f33-card-p2.img`; `tools/unafs` (built from 441d7e45d) `ls`/`get` over the image. Seat-side (executors never touch the card).

## /var/log
`boot.1.witness` (172 B), `boot.2.witness` (172 B), `boot.last` (= `2`). Each witness file holds ONE line: the `:: BOOT:` summary (`lines=1 lost=0 at=desktop`). Boot 1: `total=22532ms lines=211`; boot 2: `total=23310ms lines=216`. **No `boot-<n>.log`** — BOOTLOGCARD never wrote the boot log on either card boot (the cloud's OPEN item "the boot log never written on card-only boots: `bootlog::divert` below the same early return" is confirmed from the card). The witness line's `at=desktop ms=6479` is written at the desktop — boot 1's witness was written BEFORE the SETTERWEDGE cut (the `:: BOOT:` summary prints at the form; the wedge came at ignition 12 s later), so the card carries no trace of the wedge.

## /home/una
`.Trash/`, `.holocron/`, `settings/` (one file: `aether`). **No `Desktop/` directory** — nothing created it at login, and no screenshot ever landed (SCREENSHOTFAT: PRTSCR refused on the boot FAT; it never wrote to the home either). `settings/` holds only Aether's store — the trackpad settings line on the wire (`[settings] trackpad loaded speed=5 …`) came from defaults, not a file.

## Root
`FIRMWARE/ apps/ home/ jobs/ lib/ system/ var/` (ROOTDISK2 layout); `/jobs`: `ledger/ owed queries/ queue/`; `/var`: `log/ tmp/`.

## Findings for the cloud
- BOOTLOGCARD does not write on card boots (confirmed on the card: only the one-line witness).
- `/home/una/Desktop` is never created; the screenshot has nowhere to land even once PRTSCR targets the home (SCREENSHOTFAT must create it or write to the home root).
