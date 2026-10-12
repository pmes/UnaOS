# VFSTXCOST (rmbp-ledger B522) — where the fixed 76–103 ms of one UnaFS transaction went

Inputs: SMALLFIX8's table (`git show exec-rmbp-smallfix8:docs/dev/evidence/smallfix8/smallfix8.md`) and
flight 28's wire (`docs/dev/evidence/rmbp-0915/flight28/f28-boot1.log`, read with `awk 'index($0,"<tag>")'`).
Code read at b14be387. Derived from the code and the wire; no new flight.

## 1. What one transaction reads

`read_dir_kinds` (`fs/vfs.rs:1787`) = one `with_unafs` hold running `resolve_path` + `ls`.

- The mount is cached across transactions (`fs/unafs.rs` `MOUNT`, bound once): superblock, root slot,
  inode map (`UnaFS::imap`, a RAM `Vec`) and refcount map are NOT re-read per transaction. Not the cost.
- `resolve_path` (`unafs/src/fs.rs:1796` -> `resolve_path_via`) runs one `ls_via` per path component,
  starting at the root. `ls_via` (`fs.rs:1603`) = `read_inode_via` (one 4 KiB inode block,
  `fs.rs:1017`) + `read_data_via` over the directory blob (`fs.rs:1616`; skipped when size is 0).
- So `/jobs/ledger/<track>` touches: root inode, root data, `/jobs` inode, `/jobs` data, `ledger`
  inode, `ledger` data, `<track>` inode, `<track>` data (none if empty) = 7–8 distinct blocks.
  `/jobs/status` touches 6. Copy-on-write scatters them (each rewrite lands on a fresh block).

## 2. What one block read costs on the rMBP

- Flight 28 mounted on the SD card: `:: [unafsbind] mount=native handle=sdhc ::`.
- `SdSectorDevice` routes every block through BOOT80's read-ahead cache (`fs/unafs.rs:346`):
  `ra_window(Sdhc)` = 64 sectors, so a MISS is one 32 KiB CMD18 (`unafs/src/readahead.rs` `read`).
- The cache held ONE window (`ReadAheadCache`: one `start/len/buf`). Every scattered block is a miss,
  and each miss evicts the last — nothing survives to the next block, let alone the next transaction.
- The bus: `drivers/sdhc.rs` §"1-bit bus, no high-speed" — DAT0 only at 25 MHz = 3.125 MB/s, so 32 KiB
  is 10.5 ms of wire before command overhead. The card's cost is per BYTE at this width, not per command.
- Measured on the same boot: `:: BOOT80: mount_ms=120 mount_blocks=88 mount_cmds=11 ...` =
  **10.9 ms per window fill**.

## 3. The arithmetic

| chunk shape | distinct blocks | fills × 10.9 ms | measured (SMALLFIX8) |
|---|---|---|---|
| `/jobs/<top>` (status, queries) | 6 | 65 | 92, 92 |
| `/jobs/<top>` nested, non-empty blob (ledger, queue) | 6–7 | 65–76 | 90, 103 |
| `/jobs/<top>/<sub>` | 7–8 | 76–87 | 76, 76, 76, 80, 87, 87, 98, 119 |

The per-record slope (≈ 0.015 ms) is the deserialize of a blob that is already in the window; the
fixed term is 7–9 window fills of ~11 ms. Twelve chunks re-read the same root and `/jobs` blocks from
the card twelve times.

## 4. The fix and what it predicts

`fs/unafs.rs` `RaSet`: sixteen read-ahead windows, LRU, write-through (the BOOT80 coherence rule:
every device write is mirrored into every window it overlaps; a failed write drops all; a failed fetch
caches nothing; a remount builds a fresh device). The shared ancestors stay resident, so:

- `stat /jobs` (scan's first step) leaves root inode + data and `/jobs` inode resident;
- chunk 1 (`/jobs/status`) pays `/jobs` data + `status` inode + `status` data: ≤ 3 fills ≈ ≤ 33 ms;
- every later chunk pays its own inode + data: ≤ 2 fills ≈ ≤ 22 ms (1 when the builder laid them in one window).

Memory: at most 16 × 32 KiB = 512 KiB of heap, grown only as touched.

## 5. The instrument

`[vfs] tx what=<list|resolve|read> path=<p> resolve_ms=<n> list_ms=<n> blocks=<n> hits=<n> gen=<n>`,
printed after the hold for any native read transaction over 20 ms (`fs/vfs.rs` tail, `TxCost`).
`blocks` = medium fetches (window fills), `hits` = block reads served from a resident window.
If the fix holds, JOBSCAN's chunks print few or none of these; a chunk that still costs > 20 ms names
its split.
