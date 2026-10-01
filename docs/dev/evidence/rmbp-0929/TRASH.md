# TRASH (R75) — a desktop Trash

## Design
Finding: QUARRYOPS `op_delete` (`video/quarry/ops.rs`) unlinks/rmdirs permanently.
Mechanism: new `fs/trash.rs` (`pub mod trash` in `fs/mod.rs`). `/home/<user>/.Trash/` is created on first use via the
mount table; trash = `MountTable::rename` into it (the `[fs] mv ... lfn=1` line is printed like ops' rename); a collision
appends `~1`, `~2`; `.Trash/.index` holds `original-path<TAB>trashed-name<TAB>unix-time` (append on trash, rewrite on
restore/empty). Restore refuses (NOTICE) if the original dir is gone or the path exists. Empty unlinks the tree and rewrites the index.
Milestones: M1 trash.rs + Quarry Delete = Move to Trash; M2 Quarry menu rows Show Trash / Restore / Empty Trash (n items);
M3 shell verb `trash <path>|list|restore <name>|empty`, permanent delete row + Shift+Delete hook.
Witness: `[trash] op=trash|restore|empty path= ok= reason=` and `:: TRASH: trashed= restored= emptied= index_ok= -> PASS ::`
(`tests trash`, registered on the `quarryops` line in `arch/x86_64/syscall.rs`, same cfg `wc`+`quarry`).
Pin: `x86-wc.spec` REQUIRE `:: TRASH:`. `:: QUARRYOPS:` untouched (its fixture calls `op_delete`, still permanent).

## What I did about the alert / Shift
No yes/no alert exists (`login::open_alert` is OK-only): Empty Trash is a TWO-STEP. The menu row reads "Empty Trash (n items)";
the first press arms it and raises a NOTICE; the row then reads "Click again: Empty" and a second press within 5 s (uptime) empties.
Quarry's key stream is bare bytes with no modifier state, so Shift+Delete cannot be seen: `ops::set_shift(bool)` is the hook (unwired),
and meanwhile the menu row "Delete Permanently" and the `D` key (Shift+d) do the old permanent path. Plain Delete (0x7F) trashes.

## Written
Boot 17 (after `tests trash`): `:: TRASH: trashed=2 restored=1 emptied=1 index_ok=3 -> PASS ::` with `[trash] op=trash/restore/empty` lines.
