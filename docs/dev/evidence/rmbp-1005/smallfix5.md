# SMALLFIX5 (rmbp-ledger B480) — the merge19 wave's small owed items

Cut from 5da57a2d (merge19). No knob. x86_64 + host. Wire read first (flights 24 and 25, awk counts below).

## Design

| # | Finding (hand-back) | Fix, on the seam | Witness line |
|---|---|---|---|
| 1 | ASSOCSTAMP B460: a doc type only a ring-3 program declares is not in the stamp's hash; after a stamp match it never gets `una:preferred` | `fs/appres.rs` `sight_in` (both ROOT paths: first sight and attribute cache, i.e. every sight NOT in the per-boot memo) calls `assoc::stamp_invalidate_for(program, doctypes)`: for a declared type that is not compiled in (`facts` none, not a BUILTIN doc type) whose type object is missing or has no `una:preferred`, ONE attribute write replaces the stamp, so the next login build runs `seed_in` in full. A filled type does not invalidate, so the steady state stays one attribute read | `[filetypes] stamp invalidated by=<program> type=<mime>` |
| 2 | SVCLATCH B462: player's idle `STATE.try_lock()` while no window is open | `video/player.rs` `OPEN` flag (set beside the one `*STATE.lock() = Some`, cleared beside the one `take`); the WM-close check reads it before the lock | `tests smallfix5` `player_flag=ok` |
| 2b | F6 early return when nothing is damaged | LEFT, with the reason: a no-damage pass draws no pixel (the draw loop and the drag-out widening iterate dirty rows only, the cursor tail is `tail_of(disturbed, session, deferred)` either way), but it IS counted by the witness instruments the flights read (`wcn_note_pass(true)`: "drawn == 0 is still a pass"; `noatt_note_pass` bumps NOATT_PASSES and swaps every row's NOATT_SEEN; the C2 staging fold; the `<F8>` wedge mark) and the metal line carries `witness`, so the return changes wire output. COMPSCRATCH already took the allocations; what remains is the `RowsSnap` memcpy of at most `slots()` rows | — |
| 3 | SMALLFIX4 B466 item 11: x86 syscall.rs spawns that arm no name | the four windowed fixture spawns (WINX-2 STAT.ELF, WINX-3, SECLOGIN session-end STAT.ELF, LOGIN13 root-session STAT.ELF) arm through `wm::app_name_arm_launch` on their `let slot = slot as usize;` line. Not armed, with reasons: `sys_spawn` (HELLO.BIN, no window, IRQ-masked dispatch), SPAWNSTORM (14-byte image, no window); VUG/PULSE were already armed | `[wm] launch-name owner= path=/apps/STAT.ELF name=stat via=path armed=1` |
| 4 | REFUSALUI B468: system-files alert at owner 0 | `dialog::refused_by(what, why, owner)`; ROOTACL's `write_verdict` passes the writing slot's owner (x86: `current_slot` → `owner_of_launch`; aarch64 keeps 0, the rMBP is x86) so the alert is app-modal to the program that wrote | `[refusal] what=System files are protected why=… -> alert owner=<n>` |
| 5 | WIREDIET B461's six tags | counted (below); the one periodic line, `:: PWR:` (a 10 s rollup), becomes a 60 s window (a state change still flushes at once; the cumulative totals are the same sums). The rest are per-event and stay | `:: PWR: window_ms=~60000 …` |
| 6 | BOOT80 `types_blocks` walked `ls /system/filetypes` | the leg is ASSOCSTAMP's own read: resolve + the stamp attribute, inside the one mount lock; `types=` is the stamp's count, `types_from=stamp stamp=<match or miss or none>` | `:: BOOT80: … types_blocks=<n> … types_from=stamp stamp=match -> PASS ::` |
| 7 | SEEKTABLE2 B469: a hung `play-probe` | `table_probe_start` refuses while an earlier probe has no verdict (no second task stacked on a hung one); on a timeout `coded_tables` stops (no further VFS call or task), the rest read `:skip`, PLAYER prints `probe=timeout` and FAILS | `[player] coded seek path= probe=timeout ms=3000 -> rest skipped`, `:: PLAYER: … coded=ogg:none,opus:skip,adts:skip probe=timeout -> FAIL ::` |
| 8 | `tests smallfix5` | `smallfix5.rs` (CHARTER Kernel — kernel-by-ruling), model-only | `:: SMALLFIX5: stamp_rule=ok player_flag=ok pwr_window_ms=60000 probe_guard=ok refusal_owner=ok boot80_types=stamp -> PASS :: checked=… left=f6-early-return(witness-counted) ::` |

Milestones: M1 items 1 and 6 (fs) · M2 items 2, 7 (player) · M3 item 3 (syscall.rs) · M4 item 4 (dialog/rootacl) · M5 item 5 (PWR) · M6 item 8.

### Item 5 counts (awk `index($0,tag)`)

| tag | f24 (13536 lines) | f25 (4379 lines) | kind |
|---|---|---|---|
| `:: kepler:` | 18 | 17 | per-event (boot census, the vblank-intr window) |
| `[hda]` | 273 | 163 | per-event (census, codec walk, reset, per play) |
| `[serialdoor]` | 474 | 256 | per-event (one per typed key) |
| `:: gen7:` | 0 | 0 | absent |
| `:: SMC-SCOUT:` | 0 | 0 | absent |
| `:: PWR:` | 229 | 56 | PERIODIC (10 s rollup) → 60 s |

Owed: F6's early return (above); aarch64's refusal owner; the metal witness for every row.
