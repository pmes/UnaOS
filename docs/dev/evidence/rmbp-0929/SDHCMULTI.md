# SDHCMULTI — the SD store write rate

## Finding (boot 17, f17-boot1.log)
`:: PRTSCR: SCREEN0.PNG ... capturing (15555053 bytes reserved ...)` at 143325 ms; slices at 143569, 143811, 144054, 144537 ... (one 32 KiB slice per ~244 ms); verdict `OK` at 187295 ms. 15.5 MB / 44 s = ~350 KB/s.

## Mechanism (what the code already does, what it does not)
- CMD25 WRITE_MULTIPLE_BLOCK with Auto-CMD12 ALREADY exists (SDHCPOST): `drivers/block.rs write_blocks_sdhc` -> `write_blocks_sdhc_mb` -> `drivers/sdhc.rs write_blocks_512` (64 blocks max, PIO FIFO, DAT0 busy wait, CMD13). M2 as briefed is therefore done; the posture gates (`sdhc_rw_gate`, `reason=`) are untouched. So the 350 KB/s is NOT single-sector writes of data.
- Per 32 KiB slice (= one cluster, spc=64) `fs/fat.rs write_grow` issued: `alloc_cluster` (claim EOC: 2x FAT RMW), `zero_cluster` (a full 32 KiB CMD25 of zeros, then overwritten at once), link tail (2x FAT RMW), the data CMD25, the dir-entry RMW — ~2 CMD25 + ~5 CMD24 (each with its own programming-busy + CMD13) + reads. Half the bytes are wasted zeros; each single-sector write pays a card programming window.
- Unknown without measurement: how much is PIO, how much DAT0 busy. M1 splits that.

## Milestones
- M1 (done): `[sdhc] write census blocks= calls= multi= ms= busy_ms= wall_ms= kbps=` per 2 s while writing; `:: SDHCWR: ... -> PASS|FAIL ::` once per burst (closed by the next write or read after a 3 s gap). Counters in `drivers/sdhc.rs` tail; hooks in `block.rs write_block_sdhc` / `write_blocks_sdhc_mb`.
- M2 (already present, see above): no change.
- M3 (done): `write_grow` skips the zero-fill for clusters its own data covers entirely (`alloc_cluster_z(false)`), halving the CMD25 traffic of sequential writers; `[fs] write run clusters= blocks= multi= zero_skipped=` per 64 appended clusters.

## Written
Boot 17+ should show, after a screenshot or fetch save: `[sdhc] write census ... kbps=` lines and `:: SDHCWR: blocks= calls= multi_calls= bytes= ms= busy_ms= wall_ms= errs=0 kbps= -> PASS ::`, plus `[fs] write run clusters=64 ... zero_skipped=64`. Read busy_ms vs ms: if busy dominates, the card is the bound and the next step is fewer FAT/dir single-sector writes per slice (batch FAT link + defer the dir entry to the end of the run).

## Spec pins
None added: x86-login.spec runs on QEMU, which has no 14e4:16bc SDHCI, so the store there is not `via=sdhc` and a REQUIRE on `:: SDHCWR:` would fail. Pin it in the metal spec once boot 17 shows the line.
