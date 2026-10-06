# SMALLFIX4 (rmbp-ledger B466) — the fold's hygiene for merge18/19

**Finding.** Ten hand-back notes from this wave each left one small, named item for the integrator
(ARCHREVIEW F13/F14, TRASHCORE, PRINCIPIAFILES, LAUNCHERPREFS, FWPIN, KEPLERGR, COLUMNSVIEW,
STATUSTRAY, WIREDIET, SECREVIEW F6, knob-hygiene `wc_gpublit`). None is a feature; each is a seam
the arc that found it declined to churn in a shared file.

**Seam.** No new file. Each item lands on the file the hand-back names, at its tail or on the named
line (same-line, code before comment, on the line-sensitive files). F13 is the one clipboard
(`video/clipboard.rs`) gaining a typed file-ref fact; F14 moves the About box from the fs-core
registrar to the app menu (`video/winmenu.rs`), the registrar keeping the facts. TRASHCORE and
PRINCIPIAFILES are not on the cut tip: this branch MERGES them first (two merge commits, one trivial
conflict — the trash.rs CHARTER line takes trashcore's `shared-core`) so items 2 and 3 compile and
gate against the code that earns them; the seat's own later merges of those branches are then no-ops.

**Milestones** (one commit each, `smallfix4: <n> — <what>`): 1 F13+F14 · 2 TRASHCORE fold ·
3 PRINCIPIAFILES + LAUNCHERPREFS gate rows · 4 FWPIN retirements · 5 KEPLERGR doc line ·
6 COLUMNSVIEW hit-tests · 7 STATUSTRAY clock reach · 8 WIREDIET bench note · 9 SECREVIEW F6 ·
10 `wc_gpublit` knob + `tests smallfix4`.

**Witness.** `[clip] set-ref len=<n> ok=1` on a Quarry Copy, `[clip] get-ref len=<n> ref=1` on its
Paste; `[winmenu] about-box name=<n> shown=1` after `[appres] about ...`; `tests smallfix4` →
`:: SMALLFIX4: ... -> PASS ::`. The rest are named per item below.

**Owed.** Whatever an item below marks owed.

## Landed
- 1 F13: Quarry's Copy is `clipboard::set_file_ref` (a typed file-ref on the one buffer; the editor's ⌘V pastes the
  path), its Paste `get_file_ref`; Quarry's own CLIP is gone. F14: `appres::about` returns the facts, the box is
  `winmenu::about_box` (`[winmenu] about-box name=<n> shown=1`).
- 2 TRASHCORE (merged first): columns' `is_trash` is `trash_core::is_trash_folder`; Matrix's `.una-trash` workspace
  bin retired — a confirmed Delete is the one Trash's verb, refused with no vault (`cargo test -p matrix`: lib 22
  ok; finder 17 ok + the pre-existing uid-0 `write_to_readonly_dir_surfaces_loud_denial`); registry row B449.
- 3 PRINCIPIAFILES (merged first) + LAUNCHERPREFS: four arch.baseline rows go (launcher's key is
  `prefs::domain_path`), the stale HOLOCRONROOT `.ring` row too; three charter.registry rows go. arch-check exit 0.
- 4 FWPIN: `S_WAIT_ALT`, `Pending`, `stage_attempt`'s `commit`, `block::alternate_program_source` removed.
- 5 gpublit3.md §7 step 4 = `[kfifo] decode chid=2 …`. 6 Columns: right press `[quarry] columns right-press
  pane=<i> kind=<k> row=<r>`; the wheel steps the focus pane. 7 `[notify] switch from=center to=clock` /
  `from=<item> to=center`. 8 queue build line: drop `UNAOS_USBDEBUG`. 9 Quarry `SURF` is `Vec<u32>`.
- 10 `wc_gpublit` retired (Cargo, arroyo, builder, k8-reach); knob-hygiene/knob-parity exit 0. `tests smallfix4`:
  `:: SMALLFIX4: event_codes=unique bus_verbs=unique registrable=unique host_verbs=unique action_codes=unique
  launch_name=lumen -> PASS ::`.
- 11 `wm::app_name_arm_launch` (APPRES name for a path launch): `[wm] launch-name owner=<o> path=/apps/LUMEN.ELF
  name=lumen via=<appres|path> armed=1`.
- 12 ROOTACL + TYPECORE merged first: attrsys `system_tree` gone; five filetype parser rows leave the baseline;
  assoc's extensions walk type_core. RAWCORE's arw/tiff carried into type_core's table at the merge.
