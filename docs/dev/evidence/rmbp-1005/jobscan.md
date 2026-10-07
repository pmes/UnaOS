# JOBSCAN (rmbp-ledger B497) — the jobs read off the render handler, after the bar, in chunks

Cut from 668cdd95 (the merge19 fold + flight 27). Flight 27 wire, `f27-boot1.log`.

## Finding (the wire)
- `[users] login ok` 08:25:52; `close_into_session` runs on the RENDER task (the submit's outcome is applied by
  `login::submit_drain`), and its tail called `fs::jobs::announce()` inline: `[jobs] volume=/jobs records=1675 …`
  lands at 08:26:14, 17 s after `[login] session open` (08:25:57). `announce` used `MountTable::read_dir`, whose
  UnaFS arm reads ONE INODE PER ROW (size, mtime) inside ONE IRQ-masked `with_unafs` transaction per directory —
  1675 inode reads to produce four counts that need only the directory blobs' kinds. That is the 10.9 s
  `masked_max_ms` and the first `[lag] stall … span_ms=16998 stage=hid stage_ms=11507 render=handler`.
- `bar_ms=31205` is NOT the jobs read: `[filetypes] built at=login … blocks_read=20776 cmds=3599 ms=31154` ends
  51 ms before `[desktop] built`. Both `desktopbuild::service` and `assoc::service` (FILETYPES B423, a first-boot
  full build) run on the x86 device-service pass; the bar's first pass waits for the battery source, returns, and
  the SAME pass then builds the registry for 31 s — the bar's next chance comes after it. The second stall
  (`span_ms=11653 stage=hid stage_ms=11187`, 08:26:16–28) is that build holding the pass that pumps hid.

## The seam
Kernel — fs-core (the kernel is a READER of `jobs_core`'s layout; Mica stays the only writer, no second store).
`VfsBackend::read_dir_kinds` (default = `read_dir`; the UnaFS arm overrides it with the directory blob alone — no
inode reads) is the one new VFS surface; `MountTable::read_dir_kinds` resolves to it.

## Milestones
- M1 — `read_dir_kinds` (vfs trait default + UnaFS override + MountTable wrapper). One transaction = one
  directory blob.
- M2 — `fs::jobs`: `owe()` at `login ok` (replaces the inline `announce`) prints `[jobs] scan pending`, spawns the
  `jobs-scan` task on `smp::worker_cpu(0)` (never the render core); the task waits (bounded 10 s) for
  `desktopbuild::settled()` (the bar painted), then walks the store one directory per chunk with a 1 ms sleep
  between chunks: `[jobs] scan chunk=<n>/<m> records=<r> ms=<ms>`; the landed line keeps its shape and adds
  `on=<worker|inline> chunks=<m> max_chunk_ms=<ms> ms=<total>`. `jobs::counts()` answers `None` (pending) until
  then. Off x86 (no worker) the same chunked scan runs inline (`on=inline`).
- M3 — the bar first: `assoc::service` (the device-service pass) leaves the owed registry to the jobs worker
  (`assoc::take_to_worker`), which builds it AFTER the scan — the 31 s first-boot build leaves the pass that
  pumps hid and paints the bar. `desktopbuild::settled()` + `last_bar_ms()` at the file's tail.
- M4 — `tests jobscan` (registered, never run at boot, R80):
  `:: JOBSCAN: on=worker chunks=<n> max_chunk_ms=<ms> bar_ms=<ms> -> PASS ::` (PASS iff on=worker, landed,
  max_chunk_ms ≤ 50, bar_ms ≤ 200).

## Witness (the next flight reads)
`[jobs] scan pending at=login on=worker cpu=<c>` → `[desktop] built at=login ok bar_ms=<200` →
`[jobs] scan chunk=1/<m> …` … → `[jobs] volume=/jobs records=1675 claims=382 ledger=774 queue=519 queries=1 on=worker
chunks=<m> max_chunk_ms=<ms> ms=<ms>` → `[filetypes] built at=login …` (now after the jobs line, on the worker).
HIDSTALL's `masked_max_ms` for the login minute < 50.

## Owed
- Which UI reads the counts: NONE today — the four numbers live only on the wire line (Quarry lists `/jobs/…`
  directly, as folders). `jobs::counts()` (None = pending) is the read a future surface (Mica's window, a status
  item) takes.
- A single directory blob is one transaction: a directory big enough to exceed 50 ms in one blob read would need
  a paged `ls` in the unafs crate (not done; the counts above need none — `status` is the largest at 382 rows).
