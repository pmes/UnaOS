# TEXTEDIT — a text editor window (R75)

## Design
FILEVIEW (`video/fileview.rs`, B238) is read-only. `video/textedit.rs` reuses its layout (`fileview::layout`), row painter (`font::draw_text`), window recipe (`wm::create_at`, latch + `service`, `key_route`/`press_route` chained from `quarry/live.rs`).
- Open: `edit <path>` (`shell.rs` `edit_verb`) and Quarry double-click (`quarry/live.rs` `Act::Text`: `textedit::may_edit` -> editor, else viewer). Ownership = path under `/home/<whoami>/` (FAT has no per-file owners; DIRNS rule), no `..`.
- Input: Key bytes type/backspace/enter/Up/Down; Left/Right/Home/End and Shift-selection come from `pal::Event::Action` (the arrow byte is typed alongside, so it is ignored); Copy/Cut/Paste/SelectAll via `clipboard::{set,get}`; Ctrl-S = `0x13`. Click places the caret (`press_route`).
- Save: mount table, unlink + create + write as `fs_write` does (`KERNEL_PRINCIPAL`). Dirty mark in title via new `wm::retitle` (tail of wm.rs); `*` because the face has no `•`.
- Close: prints `[edit] closed dirty=<0|1>`; no modal.

## Written
Witness boot 17 should show: `:: TEXTEDIT: path=<scratch> bytes=40 lines=1 edits=40 saved=40 -> PASS ::` (fixture, registered `textedit` in `arch/x86_64/syscall.rs`), pinned in `scripts/specs/x86-wc.spec`.
Not done: PgUp/PgDn (no decoded byte on either HID path), Cmd-S (no keymap row; Ctrl-S works), drag-select, save-on-close confirm, aarch64 fixture registration.
