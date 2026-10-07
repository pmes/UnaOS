# JOBSUI (rmbp-ledger B511): the jobs queue on the glass

Branch `exec-rmbp-jobsui`. Its base is **JOBSCAN's tip `8d177783`**, fast-forwarded from 668cdd95. This arc reads
`fs::jobs::counts()` / `state()` / `owe()`, and those exist only on `exec-rmbp-jobscan` (B497). Fold JOBSCAN
first; this commit then cherry-picks cleanly on top of it.

Peter (2026-10-07): "what happened to the new jobs queue?" The queue on the volume has to be visible on the
glass, and the wire alone is not enough.

## Charter / seam
Owner: **Mica** (`handlers/mica`, CODEX §2's Ledger). Seam: **`shared-core`** (`jobs_core`, linked by both
rings) plus the kernel as the **fulfiller** of the reads (R79). The view's shape (`counts_line`, `item_row`,
`shown_open`, `last_flight`, `queue_name`, `rank_order`, `PENDING`, `glass`) lives in `jobs_core`. Mica
renders it on the host (`mica jobs view`), and the kernel renders it on the glass. No second store, no new
knob and no new verb word.

## What was built
1. **jobs_core**: the Jobs view section, between the queues section and the verbs' pure halves (placed away
   from JOBSNEXT's tail append).
2. **Mica**: `mica::view_lines` and `mica jobs view --img I [track] [--n N]`, which print the counts and then
   the open queue items by rank (name, track, status, the flight that last touched the item).
   *Premise mismatch:* at 668cdd95 Mica has **no window and no bus verbs** (a host CLI with no gtk feature).
   The view is therefore Mica's text view. A gtk window is owed (see Owed below).
3. **Kernel `fs/jobs.rs`** (appended at the tail; the `tests jobsui` registration is folded onto the
   `jobscan` register line):
   - `view_counts()` and `status_text()`: the counts line, or `jobs scanning...` while the scan is pending.
   - `open_items(n, track)`: one `read_dir_kinds` per track, then a `list_attrs` and a 4 KiB head read per
     candidate, capped at 160 files.
   - `shell_verb()`.
   - `tests jobsui`.
4. **Shell**: the existing `jobs` arm (BGRUN-1's reaper) changed on its own line (shell.rs:6364, code before
   `//`, `#[cfg(feature = "unafs")]`). The forms are:
   - `jobs` prints the counts, the top ten open items, then the background programs. Nothing retires (R90).
   - `jobs <rmbp|orin|pi|trunk>` prints one queue.
   - `jobs bg` prints the reaper alone.
   - `jobs next` names JOBSNEXT (B506).
   - The word `jobs` is already in HOST_VERBS (`Avail::Proc`) and shelltask ROUTED, so midden_core is
     untouched and the read runs on the shell task, not the render task.
   - When the scan was never asked (`idle`), `jobs` calls `owe()` (asked, R80).
5. **Quarry** (`video/quarry/live.rs`, QUARRY3's status line): under `/jobs`, the counts line is drawn
   right-aligned on the path-bar row, only where it does not cover the path or the activation status.
   `jobs_status_service()` repaints once when the scan lands, so the counts replace `scanning...`.

## Rank
The queue's own `job:seq` within each track, tracks in `jobs_core::QUEUES` order (rmbp first). An item is
shown when `job:status == open` and its line does not open with `✓`. JOBSNEXT's `rank_next` (B506, not landed)
replaces `rank_order` with one call when it folds.

## Wire (the next boot)
- `jobs` typed after login prints `[jobsui] verb=jobs counts=landed items_shown=10 read=<n> ms=<ms>`.
  The console shows `jobs records=1675 claims=382 ledger=774 queue=519`, then ` 1. <NAME> rmbp open f<n>` and
  so on.
- Quarry opened on `/jobs` prints `[jobsui] quarry status=landed text=jobs records=1675 claims=382 ledger=774 queue=519`.
  If the window was opened before the scan landed, it prints `status=pending text=jobs scanning...` first.
- `tests jobsui` prints `:: JOBSUI: counts=ok pending_said=1 items_shown=10 -> PASS :: state=landed live_pending=<n> verb_items=<n> read=<n> first=<NAME>`.
  - PASS requires all of: counts landed and consistent (records = claims + ledger + queue), the pending line
    in both spellings carries the word and no digit (`pending_said`), and at least one item shown when the
    queue is non-empty.
  - `live_pending` counts the times a surface actually drew the pending line.

## Owed
- Mica GUI window (gtk) for the Jobs view: Mica has none at this base.
- `help.rs`'s `jobs` doc line still reads "list background programs and reap the exited ones". It should
  name the queue read (`help.rs:128`, outside this arc's file list).
- JOBSNEXT will want the `jobs next` arm. It is the same shell line (6364), so expect a same-line merge.
