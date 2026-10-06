# WINMEMORY (rmbp-ledger B429, MACPARITY row 12) — design

**Finding.** An app window born without a requested origin (`wm::create`, `at = None`: every ring-3 program)
goes to the flow tiler (`wm::place`), so it lands wherever the live set puts it; nothing remembers where the
user left it. SETTINGSFILES (B407) gave the store its per-app stanza (`app.<name>.*` in `<home>/settings/<name>`,
PrefDeclare = 23) and Lumen declares `app.lumen.window.frame`, but nothing ever writes it.

**Seam (R79).** The WM is the frame's owner and its only writer; the store is Principia's (`crate::prefs`,
prefs_core's declared-stanza clamp). No second store. New child module `video/winmemory.rs` of `wm.rs`
(`//! CHARTER: Kernel — wm`), the twin of `winsnap.rs`; `wm.rs` carries line-neutral one-call seams only.

- Key: `app.<name>.window.frame` = `x,y,w,h` (the OUTER frame, panel px) — the key Lumen already declares, so
  the program's own stanza and the kernel's declaration are one row. The WM declares it on the app's behalf at
  its first window (`prefs::declare_app_key`, merge, never replaces a program's stanza).
- Name: the launcher-armed program name (`wm::app_name_of`). No armed name = no memory (fixtures, kernel rows,
  the WINTITLE-LATE race) — the row tiles as before.
- Placement (`create_inner`, only `at = None` app rows): a saved frame restores (clamped to the work area:
  below the menu bar, above DOCK2's reservation `ui_status::chrome_h`), none centres in the work area; the row
  is pinned and GLASSFIX3's cascade (one title band, `cascade_step`) offsets a second window of the same app.
- Quarry (FOLDERVIEW B424) creates with `create_at_native` (`at = Some`), so its per-folder frame wins by
  construction; `quarry` is also refused by name.
- Writes (R96): one store write per move-end (`drag_end`/`drag_cancel`, after WINSNAP's snap) or `close(id)`,
  and only when the frame differs from the last written; queued off the render task to a `winmem-flush` task
  on a worker core (INPUTSTALL M5's shape).

**Milestones.** M1 prefs tail `declare_app_key` + `winmemory.rs` (pure clamp/centre/codec, resolve, note,
queue) + the four wm.rs seams. M2 `tests winmemory`. M3 MACPARITY row 12 folded.

**Witness.** `[wm] place win=<id> app=<name> from=<saved|centre|cascade> frame=<x,y,w,h>` at each app window's
birth; `[winmem] save app=<name> frame=<x,y,w,h> on=<move-end|close> ok=<1|0>` per write;
`tests winmemory` → `:: WINMEMORY: saved=<n> restored=<ok> cascade=<ok> clamp=<ok> -> PASS ::`.

**Owed.** The program's surface size is the program's: `w,h` are recorded, only the origin is restored (a
program that reads its own key may size itself). A window opened before its launcher armed the name
(WINTITLE-LATE) is not remembered; `close_owner` (a program's exit) does not write — the move-end write
already holds the frame.
