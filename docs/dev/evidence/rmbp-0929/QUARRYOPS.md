# QUARRYOPS (R75) — Quarry file operations

## Design
**Finding.** Quarry (`video/quarry/live.rs`) browses and opens only: `key_route` (live.rs ~2317) has Up/Down/Left/Right/Enter/Backspace/`r`; `press_route` selects and double-clicks. Nothing mutates.
**Mechanism.** New child module `video/quarry/ops.rs` (`#[path]`-declared in `live.rs`, so it reads the model's private fields).
- Right-click: x86 `wc_click_route_at` DOCKRUN arm (`arch/x86_64/syscall.rs`, the `let eat = if mask & 0x02 …` line) now also asks `quarry::right_press`, and a primary press asks `quarry::menu_press` while the menu is up. Knob-off stubs in `video/quarry.rs`.
- Menu (7 painted rows: Open, Rename, Delete, New Folder, Copy, Paste, Show Info) and the inline edit field are drawn by `ops::paint_overlay`, called at the end of `repaint_locked`.
- Keys: `ops::key_pre` is asked first in `key_route` (edit field takes everything; Enter commits, Esc cancels; Delete 0x7F deletes; `e` renames; `n` new folder). Cmd/Ctrl-C/V arrive as `Event::Action(Copy|Paste)` and are taken in `key_route` before the `Event::Key` test; the clipboard (`CLIP`) carries a path. F2 is BRIGHTKEYS' (`Action::BrightnessUp`, consumed at `pal::push_event`) and never reaches Quarry, so rename is `e` and the menu.
- Seams: `MountTable::{create,unlink,remove_dir,rename,read,write}` as the shell verbs call them (principal `KERNEL_PRINCIPAL`, FAT has no owners). Rename of a long name prints the LFNMV2 line `[fs] mv a -> b lfn=1 ok= sectors_written=`.
- DIRNS: `ns_check` — path must lie under `/home/<session user>` (`/home` with no session), no `..`, never the home itself; refusal logs `ok=0` and calls `login::notice_show` (B229).
- Every op prints `[quarry] op=<name> src= dst= ok= reason=` and `refresh()` re-lists.
**Witness.** `:: QUARRYOPS: ops=[mkdir,rename,copy,delete] ok=4 refused=2 -> PASS ::` from `tests` fixture `quarryops` (x86 ladder, `wc`+`quarry`).
**Spec pin.** `x86-wc.spec` REQUIRE that line, FORBID FAIL/SKIP.

## Written
M1 (all in one): ops.rs, hooks, fixture, pin. Boot 17 should show `:: QUARRYOPS: ops=[mkdir,rename,copy,delete] ok=4 refused=2 -> PASS ::` after `tests`, plus `[fs] mv ... lfn=1 ok=true` and `[quarry] op=` lines; right-click on a Quarry row shows the menu.
Not done: no QEMU/compile run (R76/R78). Show Info owner is the volume posture (FAT carries no owner).
