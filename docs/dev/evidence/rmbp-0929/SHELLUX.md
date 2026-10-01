# SHELLUX (R75) — history, completion, control keys in the desktop shell

## Design
Finding: `history` verb and the 64-line store already exist (BASICS: `shell.rs` `CMD_HISTORY`/`history_record`/`history_cmd`, `HISTORY_CAP=64`); `cd`/`pwd` exist (JD4: `CWD`, `vfs_path`). Missing: Up/Down recall, Tab, control keys — `main.rs::handle_key` took only printable, BS/DEL and CR/LF. The verb table is `midden_core::HOST_VERBS` (+`Avail::on`); a registry check that every `match` arm is listed already exists near `shell.rs` "Add a verb to `HOST_VERBS` and forget its arm" — no second `VERBS` list added.
Mechanism: bytes as the HID fold makes them (`drivers/xhci/mod.rs:135-138`): Up 0x1F, Down 0x1E, Tab 0x09, Ctrl-letter 0x01..0x1A. New `shellux.rs`: pure `key()` over (line, `LineSel` caret, history, verbs, dir lister); `console_key()` binds it to Console/`CMD_HISTORY`/`HOST_VERBS`/mount table. Hook: one same-line fold in `main.rs::handle_key` before the BS branch. Tail seams in `shell.rs` (`history_lines`, `verb_names`, `complete_ls`, `cwd_now`); `LineSel::end_selection` tail-appended in `video/termsel.rs`.
Milestones: M1 history walk (draft restored), M2 Tab (first word over verbs, later words over directory entries via `vfs_path`+mount table; one candidate completes, several print on one line and the common prefix is filled), M3 Ctrl-C (line only — the shell tracks no foreground child at this seam), Ctrl-L (`Console::clear`), Ctrl-A/E/U/W.
Witness: `:: SHELLUX: history=up-down completions=verbs+paths ctrl=[c,l,a,e,u,w] cwd=<pwd> -> PASS ::` from the `shellux` fixture (registered lazily by `tests::ensure_shellux`, x86 `witness`). Pin: `unaos/scripts/specs/x86-wc.spec`. No knob.

## Written
All three milestones in one pass. Boot 17 should show the witness line above after `tests` (or `tests shellux`), plus a `[shellux] c= l= a= e= u= w= comp=[..]` detail line.
