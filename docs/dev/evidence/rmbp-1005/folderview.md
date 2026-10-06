# FOLDERVIEW (rmbp-ledger B424) — a folder remembers its view, on the folder (MACPARITY §16 B6)

Branch `exec-rmbp-folderview`, cut from df15f19d; merged exec-rmbp-merge17 (QUARRY3) because df15f19d does not compile. Be's Tracker kept `_trk/pinfo` and `_trk/columns` ON the folder.

## Finding
Quarry's sort and column widths are ONE global view: QUARRY2's `columns.rs` loads them from Principia's `quarry`
namespace once per boot and every header press / `s` / `[` `]` re-saves the GLOBAL preference. ATTRCOLUMNS (B402)
put the folder's ATTRIBUTE column set on the folder as `una:view` (`fs::attrfacts::view_of/save_view`, written as
the kernel). There is no view mode on the folder, no per-folder sort or widths, no remembered frame, and Quarry
publishes no menu (no `View` title). The window is fixed-size (the panel's geometry), movable (`wm::move_to`,
clamped to the panel; `wm::frame_of` reads the outer box).

## Seam (R79)
The view state is ATTRIBUTES on the folder, through the one VFS attribute API ATTRCOLUMNS uses (`MountTable::
get/set/list_attrs`, `KERNEL_PRINCIPAL` — the system's bookkeeping about the folder, Tracker's `_trk`): no
dotfile, no second store. The DEFAULT stays Principia's (the `quarry` namespace QUARRY2 already owns, plus
`view.mode`): `Use as Default` writes it, nothing else does. Code: `video/quarry/folderview.rs`, a `#[path]` child
of `live` attached at live.rs's tail (one tail hunk, no existing line touched); hooks in `columns.rs` only.

- Keys on the folder: `una:view.mode` (`list`|`icons`), `una:view.columns` (`size=9,modified=16,type=12,origin=28`),
  `una:view.sort` (`name|size|mtime|type`, `-` prefix = descending, `@<attr key>` = an ATTRCOLUMNS column),
  `una:view.frame` (`x,y,w,h`, the outer frame in panel px). `una:view` (the attribute column set) stays ATTRCOLUMNS'.
- Resolve on entering a folder: its own keys, else the nearest ancestor that has any, else the default. A
  volume without attributes (the boot FAT, R99) answers `Unsupported` and is never written: `writable=0`.
- Writes: LATCHED (header press, `s`, `[`/`]`, a window move that settled, a mode change) and drained on
  Quarry's service pass, once per change, never per frame (R96); leaving the folder flushes its latch.
- Frame: polled from the service pass (every 250 ms, a read), written once it has settled 1 s, or at close;
  restored when Quarry's window appears (`wm::move_to`, which clamps to the panel).
- `View` menu (winmenu, the one bar framework): `Use as Default`, `Reset to Default` (removes the folder's
  `una:view*` keys; the folder then inherits). The pick latches; the service pass does the I/O.

## Milestones
- M1 `folderview.rs`: the codec, resolve-with-inheritance, latched save, the columns.rs hooks.
- M2 the frame (poll, settle, save, restore on reopen) and the `View` menu (Use as Default / Reset to Default).
- M3 `tests folderview` and the witness.

## Witness (what the next flight reads)
- Quarry's window appears: `:: FOLDERVIEW: folder=<path> mode=<m> cols=<n> sort=<k> frame=<w>x<h> restored=<1|0> ::`
- `[folderview] enter dir=<p> src=own|inherit:<dir>|default writable=<0|1>` on each new folder;
  `[folderview] save dir=<p> mode= cols= sort= frame= ok=<1|0>` once per change; `[folderview] menu use-as-default|reset dir=<p>`.
- `tests folderview` → `:: FOLDERVIEW: folder=<test dir> mode=list cols=<n> sort=<k> frame=<w>x<h> restored=<0|1> codec=ok inherit=ok reset=ok fat_writable=0 -> PASS ::`.

## Owed
- The mode is QUARRY3's switcher (`toolbar::View`, merged at merge17): `set_view` latches it, `enter` applies it (`toolbar::apply_mode`).
  A search's hit list (shown as `/`) is not a folder: nothing is resolved or saved for it.
- `tests quarry3`'s off-glass `Model::new` + `navigate` passes through `enter` (the view state is global, like ATTRCOLUMNS').
- Quarry's window is fixed-size: the frame's size is recorded, only its position is restored (clamped).
- One Quarry window: a frame is restored on REOPEN (Tracker's one-window-per-folder is not this shape).
