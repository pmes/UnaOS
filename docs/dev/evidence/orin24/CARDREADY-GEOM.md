# CARDREADY-GEOM — the Orin boot card's partition geometry, as BUILT

Round: orin 24 (UNAFSROOT), 2026-09-09. Branch `exec-orin24-unafsroot`.

This file is a RECORD, not a note: `~/unaos-bench/tools/media-writer.sh`'s **C7-GEOMETRY** reads the
`CARDREADY-GEOM:` line below and refuses any medium whose MBR does not match it, field for field.
It lives in git rather than in a round's scratch because a check whose expectation can be deleted in
ten seconds is a check that stops firing (R32).

## The line

    CARDREADY-GEOM: 1:0c:2048:260096,2:7f:262144:1048576

Grammar (`media-writer.sh::probe_medium`, `"%d:%02x:%d:%d"`): `slot:type:start_lba:sector_count`,
comma-separated, partition type in lowercase hex, LBA and count in 512-byte sectors.

| slot | type | start LBA |   sectors | size    | what it is |
|-----:|:-----|----------:|----------:|--------:|:-----------|
| 1    | 0x0c |      2048 |   260 096 | 127 MiB | FAT32, label `UNAOS-ORIN` — the loader tree. Mounted at **`/boot`** (and `/apps` at its `APPS/`). |
| 2    | 0x7f |   262 144 | 1 048 576 | 512 MiB | native UnaFS volume, filling its partition exactly. Mounted at **`/`**. |

Image total 640 MiB (`128 + 512`); slot 1's count is `128 MiB − the 2048-sector MBR gap`.

## Where it comes from

`./arroyo esp-jetson` builds the tree, builds the card image beside it
(`target/UnaOS-orin-card.img`), then READS THE GEOMETRY BACK off the built image and prints both the
`:: ORIN-CARD: … ::` witness and the `CARDREADY-GEOM:` line above. The expectation therefore cannot
drift from the media: changing it requires building a different image.

Measured on this branch, 2026-09-09, from `unaos/target/UnaOS-orin-card.img`:

    :: ORIN-CARD: p1 type=0x0c part=[2048..262144) label=UNAOS-ORIN | p2 type=0x7f part=[262144..1310720) span_blocks=131072 vol_blocks=131072 v5 fits=yes magic=ok ::
    :: ORIN-CARD: unafs volume 512 MiB in a 512 MiB partition on a 640 MiB image ::
    CARDREADY-GEOM: 1:0c:2048:260096,2:7f:262144:1048576

## Two notes a reader will need

**The volume is UnaFS v5, not v3.** `libs/fs/unafs/src/superblock.rs` has `VERSION = 5`,
`MIN_SUPPORTED_VERSION = 3`, and the host `unafs init` CLI — the implementation of record, the same
one the Pi's `make-pi-img.sh` path uses — stamps the current version. A v5 volume of ≤ 2 GiB is
laid out identically to v4 and stays single-level (`VERSION_REFMAP_TREE` only changes the refmap
shape above `MAX_BLOCK_COUNT_ONE_LEVEL` = 524 288 blocks; this volume is 131 072). The kernel mounts
v3 through v5, so 512 MiB v5 is the same on-disk shape every existing volume in this tree uses.

**512 MiB is not chosen here.** It is `UNAFS_CAP_BLOCKS` from `arch/aarch64/sdmmc_tegra.rs`, derived
from the refmap's 8 B/block cost against a 48 MiB aarch64 heap, the per-boot refmap re-read, and the
single-level refmap bound. Matching it keeps the MBR staging path and the GPT installer path from
drifting. See the `UNAFSGROW` block in `unaos/arroyo` for the full derivation.
