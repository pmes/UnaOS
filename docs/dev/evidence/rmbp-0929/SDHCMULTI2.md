# SDHCMULTI2 — the FAT layer between card calls

## Finding (boot 18, f18-boot1.log)
`:: SDHCWR: blocks=309 calls=26 multi_calls=5 bytes=158208 ms=70 busy_ms=0 wall_ms=1063 ... kbps=148` and `blocks=678 calls=52 multi_calls=10 ... ms=201 wall_ms=3428 kbps=101`.
The controller spent 70 ms of 1063 ms. 26 calls for 5 data CMD25s: 21 single-block calls = FAT claim + link + dirent per cluster.

## Mechanism (fs/fat.rs)
- `write_grow` appended one cluster per loop turn: `alloc_cluster_z` (FAT sector read, locked re-read, `set_fat_entry_inner` = RMW x num_fats) then `set_fat_entry(tail, n)` (another RMW x2), then `write_dir_entry_fields_mtime` (RMW) at step 4 on every call.
- `write_span` already issues the largest contiguous run to the block layer (`wr_sectors`); the card driver bounds a CMD25 at `MB_MAX_BLOCKS`=64 (32 KiB DMA bounce).

## Milestones
- M1 `claim_run`: contiguous free run (<=128 clusters, from the hint), compare-and-claim under `FAT_MUTATION`, ONE multi-block write of the touched FAT sectors per FAT copy; the tail link rides the same write when the run is fully overwritten by the data (no zero-fill needed); a partially covered cluster still goes zero-then-link.
- M2 deferred dirent (x86 internal SD only, `sdhcblk`+`sdw`): `publish_dirent` records (lba, off, first, size) in `PEND`; flushed (RMW incl. mtime) before any `FatFs::rd_sector/rd_sectors` covering that sector, when another dirent is deferred, by the card read path's idle hook (>1 s after the last write, before the burst closes) and by `flush_pending_dirent()`. Other sources keep the immediate publish.
- M3 witness only: `max_run_blocks` = longest contiguous request `write_span` handed down. NOT done: raising the card driver's 64-block CMD25 bound to 512 (ADMA2 bounce + descriptor chain in drivers/sdhc.rs) — a driver change.

## Witness
`:: SDHCWR: ... kbps=N fat_writes=N dirent_writes=N max_run_blocks=N -> PASS ::`. Target boot 19: `wall_ms` within 2x of `ms`. NB `wall_ms` also includes caller time between writes (encoder); if it stays high with `calls` small the remainder is not the FAT layer.

## Spec pins
None (QEMU has no SDHCI on the x86 lane).

## Written
M1+M2+M3-witness written in one pass, uncompiled (R76). Boot 19 should show `fat_writes` ~ 2 per appended run, `dirent_writes` 1 per file, `max_run_blocks` >= 64.
