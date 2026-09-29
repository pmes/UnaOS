# FILEVIEW — opening a text file from the desktop

**Finding.** A double-click on `CONFIG.TXT` in Quarry ends at `Act::NoOpener` (`quarry/live.rs` `activate_row`, census in `run_act`): only `.ELF/.BIN` launch and `.PNG` (facet) views. No text viewer exists (grep of `unaos/crates docs` for `fileview` was empty).

**Mechanism.**
- `video/fileview.rs` (new): monospace grid painted with `font::draw_text` (console glyph path) into a `Vec<u32>` surface, window via `wm::create_at` (owner `KERNEL_OWNER_BASE+5`), title = file name, close box via `wm::close_box_hit`, read-only. `open(path)` reads through `shell::vfs_mount_table()` (cap 256 KB, `...truncated` row beyond), `layout()` wraps at the column count. Scroll: Up/Down (0x1F/0x1E), wheel (3 rows), space/`b` page, `g`/`G` ends.
- `quarry/live.rs`: new `Act::Text`; `activate_row` routes `is_text_name` (`.TXT .MD .LOG .SPEC`, extensionless) after the ELF and PNG tests; `run_act` latches via `fileview::request_open`, drained by `service()` (FACET's stack-depth reason). Key/wheel/press chained from `key_route`/`press_route` (router files stay untouched).
- `shell.rs`: `view <path>` verb, folded onto the `"reboot"` arm line, helper at the tail.
- `video/mod.rs`: `pub mod fileview;` folded onto the `quarry` line (same gate: x86 `wc`, aarch64 `desktop_firmware`).

**Milestones.** M1 viewer window; M2 Quarry routing; M3 `view` verb (+ fixture and spec pins).

**Witness.** `:: FILEVIEW: path=<p> bytes=<n> lines=<n> rows=<n> wrapped=<n> -> PASS ::`, chained from `quarry::live::selftest`/`door_selftest` (DONE-latched). Pins in `unaos/scripts/specs/x86-wc.spec`. No new knob.

## Written
All three milestones written, uncompiled (R76). Existing QUARRYOPEN legs updated for the new routing (`CONFIG.TXT`/`READ_ME` now `handler=fileview`; `.CAB` is the new no-handler case). PageUp/PageDown are not decoded to any byte on either HID path (`xhci/mod.rs` maps them to 0), so space/`b` page instead; `…` is drawn as `...` (the face is ASCII-only).
