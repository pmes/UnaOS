# DROPTYPES (rmbp-ledger B477) — MACPARITY row 18, the drop-type declaration

Branch `exec-rmbp-droptypes`, cut from 5da57a2d (merge19). Knob: none — rides `wc` + `quarry` (x86) and
`desktop_firmware` + `quarry` (aarch64).

## Finding
DRAGDROP2 (B470) made ANY live ring-3 window a drop target (`dnd::resolve_glass`: `ring3_owner_live(owner)` is the
whole test), so a program that cannot open what it is handed is still handed it. The flights 24/25 wires carry no
`[dnd]` line (the arc was unflown), so this is built from the B470 seam. aarch64: `ring3_deliver` returns
`pushed=0` unconditionally, the bus dispatcher has no `BUS_VERB_DROP_GET` arm (it falls to EINVAL), and nothing
feeds `video::capture` there, so a drag armed by Quarry's press never sees a motion or a release.

## The seam (APPRES is the registrar; the kernel is the fulfiller of the drop verb)
* `droptypes` is a resource key beside `doctypes`: `una_abi::attr_keys::DROPTYPES = "una:droptypes"`,
  `midden_core::RES_KEY_DROPTYPES`, `tools/una-res` accepts `droptypes = a/b, c/*` and stamps it; `fs::appres::App`
  gains `droptypes` (read from the block, cached as an attribute and read back from the cache — the registry's
  facts, the way doctypes are). `type/*` and `*/*` are honoured by the dock's one rule (`dock::dnd_takes`).
  Absent = takes nothing.
* The owner's app: `wm::app_name_of(owner)` → `appres::key_of_title` → `appres::app` (the dock tile's own path from
  a name to the registrant). `wm::app_name_arm_launch` sights the path first, so a path launch (`lumen`) is a
  registrant.
* `video::dnd`: a ring-3 window is `Target::App` only when EVERY dragged file's type is declared; else it is
  `Target::Refused` — the ghost relabels to the refusal, the release delivers nothing and prints
  `[dnd] drop to=app:<owner> action=refused reason=type-undeclared types=<n>` (`n` = distinct undeclared types).
  The answer is memoised per (owner, first path, count) for the session.
* Lumen declares `droptypes = text/*, image/*` (what its `open` takes).
* aarch64: `ring3_owner_live` (the asid's live Proc row), the DROP_GET arm (the x86 body: owner = asid),
  `user_input_push_owner` for the event, and `capture::feed` from the focused-app drain and the shell path's
  `render_service` motion arms (each after the cursor moved), `capture::feed_release` from the shell drain.

## Milestones
* M1 — the key: una-abi DROPTYPES, midden_core RES_KEY_DROPTYPES, una-res parse/stamp (+ its test), appres App
  `droptypes` (block, cache write, cache read), Lumen's declaration, the launch sight.
* M2 — dnd: the type rule, `Target::Refused`, the ghost refusal, the refusal line, `types=declared` on the ring-3
  line; `tests dragdrop2` grows `types=ok`.
* M3 — aarch64: liveness, DROP_GET arm, the push, the capture feed.

## Witness (the wire a metal boot prints)
* `tests dragdrop2` → `:: DRAGDROP2: multi=ok dock=ok desktop=ok spring=ok ring3=ok types=ok -> PASS ::`
* by hand, a text file dragged from Quarry onto Lumen: `[dnd] ring3 owner=<o> token=<t> n=<n> pushed=1 types=declared`
  then `:: LUMEN: drop n=<n> first=<p> ::`; an audio file onto Lumen:
  `[dnd] drop to=app:<o> action=refused reason=type-undeclared types=1`.
* `[wm] launch-name … via=appres` for `lumen` (the sight landed).

## Owed
* The Pi's shell-drain release reads the cursor as `render_service` last left it (the press's own rule there).
* The ghost on aarch64 (the overlay row is x86 `wc` only); a per-window (not per-program) declaration; text drags.
