# VOLUMES — rmbp-ledger B366 (branch exec-rmbp-volumes)

Peter at the glass, flight 22 (items 4/5): "a duplicate home dir is created on the fat boot partition, along with dup
apps and system dirs. the boot partition should appear under volumes in place of efi and only efi under volumes/boot.
the mounted UnaFS root volume should also appear in volumes. maybe you can find free samples for all the various file
format we need tested? there are missing file formats." — "we need test-f under system".

## Finding (from the wire, f22-boots.log)

- The user store was NOT the FAT `home` creator: `[users] home=/home/una created volume=unafs` (`ensure_home_native`).
- The creator was the **screenshot writer**: `:: PRTSCR-DIR: … dir=/home/una/Desktop path=home/una/Desktop created=3`
  then `:: SHOTMOUNT: via=fat path=/home/una/Desktop/SCREEN0.PNG … -> FAIL` (x3). `video/prtscr.rs::ensure_capture_dir`
  walks the FAT capture target (the boot partition) and mints `HOME/<u>/Desktop` there; the mount-table create of
  `/home/<u>/Desktop/<name>` on the UnaFS root then had no parent, and the bytes fell back to the FAT copy. That is
  also why Peter "thought screenshots were broken": they were on the FAT, under a `home` Quarry showed under /boot.
- The "dup apps and system" are the FAT's own `APPS/` and `SYSTEM/` (the loader's staging, the builder's data tree),
  listed under `/boot` beside the root's `/apps` (the FAT `APPS/` rooted) and `/system` (UnaFS, `assoc::seed_in`).
- `/volumes` held only `/volumes/EFI` (`source=ahci0 rw=no`, the internal SSD's ESP); the boot partition and the
  UnaFS root were not volumes.
- `tests play flac|opus|mp3 -> SKIP reason=no-file`: no sample of those formats was staged anywhere.

## The seam

- M1 (fs-core, the mount table decides the medium): on a native root the capture folder is created THROUGH THE MOUNT
  TABLE and the name chosen against that directory (`native_capture_dir` / `native_pick`, prtscr tail); the FAT walk
  remains only for a FAT-only card. Same rule `users::ensure_home_native` already keeps.
- M2 (fs-core): the Volumes entries are MOUNTS (`fs/volumes.rs::bind_aliases`, called from `bootdisk::bind`):
  `/volumes/boot` = the FAT boot volume (same volume name `boot`, so `same_volume("/boot", "/volumes/boot")`) and,
  on a native root, `/volumes/UnaOS` = the UnaFS root (the superblock carries no label). The shell's `ls /volumes`
  and Quarry agree. Quarry's presentation (`volumes::view`, applied in `quarry/live.rs::collect`): no `boot` row at
  `/` (it is under Volumes), only `EFI` at the boot partition's root. `/boot` stays bound for every verb and spec.
- M3 (builder): `unaos/builder/testf.list` — 18 fetched samples (Chromium test data, BSD-3-Clause; the bmpsuite BMP,
  public domain; the audio + AV1 rows are files the oracles' `vectors.txt` already pin) + 6 generated (QOI, SVG, TXT,
  MD, JSON, CSV; CC0-1.0). The builder fetches at image time into `target/testf-cache/`, stages only on a sha256
  match, writes `system/test-f/MANIFEST.txt` (name bytes sha256 licence source); 812 KB total (ceiling 20 MB).
  `arroyo esp-x86` (UNAOS_UNAFS card) copies it onto the UnaFS root at `/system/test-f` (`tools/unafs mkdir`, new).
  Kernel: `volumes::testf_find` (`/system/test-f`, then `/boot/system/test-f`); `tests play` asks it after `/home`
  (and now plays `wav` too); `UNAOS_TESTF=0` stages nothing.

## Milestones

- M1 — prtscr: no `home` on the FAT on a native root (`PRTSCR-DIR … volume=unafs … -> RESOLVED`).
- M2 — `/volumes/boot`, `/volumes/UnaOS`, Quarry's Volumes view, `tests volumes`.
- M3 — `system/test-f` (builder + arroyo + tools/unafs mkdir), `tests testf`, `tests play` lookup.
- M4 — this doc.

## Witness (what a metal boot should print)

    [vfs] volume alias /volumes/boot = fat boot volume source=sdhc (Quarry shows EFI only) ::
    [vfs] volume alias /volumes/UnaOS = native unafs root source=sdhc ::
    :: PRTSCR-DIR: theme=crispy user=una home=/home/una dir=/home/una/Desktop path=/home/una/Desktop created=1 volume=unafs reason=session -> RESOLVED ::
    :: SHOTMOUNT: via=vfs path=/home/una/Desktop/SCREEN0.PNG bytes=… -> PASS ::
    tests volumes -> :: VOLUMES: boot=efi-only root=unafs home_on_fat=0 shown=EFI,UnaOS,boot -> PASS :: root_view=… ::
    tests testf   -> :: TESTF: staged=24/24 missing=- -> PASS :: dir=/system/test-f manifest=yes ::
    tests play    -> :: PLAYCODEC: path=/system/test-f/TEST.FLAC … -> PASS :: (and opus, vorbis, mp3, aac, m4a, aiff)

## Owed / not done

- The internal SSD's ESP still shows as `/volumes/EFI` (Peter's "in place of efi" may mean it should go; the row says
  "every other mounted volume by label", so it stays — a question for the seat).
- A card already carrying flight 22's FAT `HOME/` reads `home_on_fat=1 -> FAIL` until re-imaged; nothing deletes it.
- `/volumes/UnaOS` is a second path to the root tree (so `/system` is reachable twice, once per path) — by request.
- The fonts (`system/fonts`) still ride only the FAT data tree (`/boot/system/fonts` on a UnaFS root); not in scope.
- The image viewer finds the samples by browsing `/system/test-f` in Quarry; no viewer fixture opens them yet.
- The Chromium URLs track `main`; a changed upstream file is refused by its pin and reported missing (re-pin then).
