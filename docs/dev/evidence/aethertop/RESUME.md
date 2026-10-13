# AETHERTOP (SR146) — RESUME (stopped by the seat 2026-10-14: wrong model; resumes on Opus)

Branch `exec-host-aethertop`, worktree `/home/user/exec/aethertop`, cut from 8da376cb. WIP commit f63350e5
(parent 8da376cb). NOTHING COMPILED YET: no `cargo check` was run (the shared wave34 target dir is empty, 0 bytes;
`df` showed 3.0 GB free on `/`). No gates, no linters run. Arc file: `/root/unaos-bench/scratch/cloud/wave34/aethertop.md`;
head correction from the seat: the pre-commit leg is `cd unaos && ./arroyo gates > gates.log 2>&1` (paste its
`GATES: N run N green -> PASS` line), not the three linters alone.

## DONE (uncompiled; the design, with file:line)

Step (1) the pseudo-classes:
- `unaos/libs/web/css_core/src/selectors.rs`: `ElementState::{PopoverOpen, Modal, Fullscreen}` (after `Autofill`,
  ~l.110) and the three arms in `pseudo_class()` next to `"open"` (~l.646). css_core's `html_state` falls to
  `_ => false` for them; `invalidation.rs::state_attributes` returns None for them (host-driven states) — untouched.
- `handlers/aether/src/dom/select.rs` `state()`: arms `PopoverOpen` / `Modal` read `crate::js::toplayer::is_popover_showing`
  / `is_modal`; `Fullscreen` reads `crate::js::fullscreen::element()`; all guarded by the new `is_page_doc(doc)` (same
  arena-identity check `focused_in` uses).
- `handlers/aether/src/js/toplayer.rs`: a Rust MIRROR of the prelude's top layer — thread-locals `TOP: Vec<(usize node,
  u8 kind)>` (KIND_POPOVER=1 showing popover, KIND_MODAL=2 modal dialog, KIND_FULLSCREEN=3 appended by `top_layer()`
  from `fullscreen::element()`) and `GENERATION: u64`; pub fns `bump()` (generation+1 and `super::touch()` so the engine
  relayouts/repaints), `generation()`, `is_popover_showing`, `is_modal`, `top_layer()`, `in_top_layer`, `modal_dialog()`,
  `is_inert(id)` (HTML §4.11.4 "blocked by a modal dialog": inert unless under the topmost modal dialog or a
  top-layer entry after it; walks `super::dom::parent_of`). Native `__tl('track', n, elements, kinds)` fills the
  mirror (reads the JS arrays via `PropertyKey::from_str("length")` / `PropertyKey::Index(i)`) and bumps when it changed;
  the prelude's `track()` now passes `topLayer` and a kinds array (`st(el).vis==='showing'?1:(s.modal?2:0)`).
  `reset()` clears the mirror and bumps.
- `handlers/aether/src/js/fullscreen.rs`: `__fs('set')` calls `toplayer::bump()` when the element changed.
- `handlers/aether/src/css/mod.rs`: `UA_SHEET` extended (hidden popover `display:none`, `dialog:popover-open {display:block}`,
  `[popover]` and `dialog:modal` as `position: fixed; top: 0; left: 0` boxes — NOT the spec's `inset:0; margin:auto;
  width: fit-content`, because layout_core's `css.rs` "width" arm has no `fit-content` keyword (grep: none in
  layout_core) and an auto-width absolute box with both insets would stretch to the viewport; `:not(:root):fullscreen`
  per Fullscreen §7; `::backdrop { position: fixed; inset: 0 }`; `dialog:modal::backdrop { background-color:
  rgba(0,0,0,0.1) }`). `ledger_rules`: `PE::Backdrop => continue` (consumed now). `StyleCache.toplayer_gen` recorded;
  `restyle_plan` arm `S::PopoverOpen | S::Modal | S::Fullscreen if prev.toplayer_gen != toplayer::generation() => return None`
  (placed BEFORE the `_ => Vec::new()` arm). After `cascade_document` (when `hover.is_some()` and the tree is the page's
  document) `top_layer_backdrops(doc, &rules)` scans `rules.entries` for `PE::Backdrop` selectors matching each
  top-layer element (`matches_for_pseudo`), picks the winning `background-color` / `background` by (important,
  origin rank, specificity, order, decl index) and parses it with `AetherHost::parse_color_alpha`; stored in
  thread-local `TOP_LAYER_PAINT`, read by `pub(crate) fn top_layer_paint()`.

Step (2) painting + ::backdrop + inertness (painting done, inertness hooks NOT yet wired):
- `handlers/aether/src/render/mod.rs`: `FrameSnap.top_gen`; thread-locals `TOP_IDS` (taffy ids of the top-layer
  elements this frame — the FIRST box of each DOM node, chosen by the smallest taffy id) and `TOP_PASS` (the element the
  top pass is painting); `hoisted_to_top_layer(id)`; `draw_node_inner` returns early for a hoisted box (after the
  display:none check); nested fn `paint_top_layer(...)` in `frame_walk`: per element, blend the backdrop over the
  damage rects with `blend_px` (not on the index walk, mode 1), then `draw_node(tid, parent-origin-sum, inherited along
  the taffy ancestor chain via `inherit_element`, …, clip = viewport)`; called right after BOTH root `draw_node` calls
  (index walk and paint walk). `relayout_damage` returns None (whole frame) when `before.snap.top_gen !=
  toplayer::generation()`.
- Trusted events (step 3) DONE in toplayer.rs/fullscreen.rs: native `__tl('fire', target, event)` →
  `events::dispatch_trusted`; every `dispatchEvent` in the toplayer prelude (ToggleEvents, `cancel`, `close`) and the
  fullscreen prelude's `fullscreenchange` / `fullscreenerror` go through it. closewatcher.rs (NOT in FILES) still
  dispatches the CloseWatcher interface's own events untrusted — gap list.

## NOT DONE (exact next steps, in order)

1. `handlers/aether/src/js/events.rs`: inertness hooks — in `click_with_activation` (l.~1602) when `trusted &&
   super::toplayer::is_inert(n)` return `(true, false)` BEFORE the CLICKING insert; in `user_key` (l.~1557) after `let t = …`:
   `let t = if super::toplayer::is_inert(t) { super::toplayer::modal_dialog().unwrap_or(t) } else { t };`.
2. `handlers/aether/src/js/observers.rs` (step 4): in `updateIntersections` after `targetRect = rectOf(target)`, clip by
   every ancestor between the target and the root whose computed `overflow-x`/`overflow-y` is not `visible`: intersect
   with the ancestor's padding box (border box less `edges()` borders), expanding a scroll container's clip by
   `s.scrollMargin` (`expand`); an element root with non-visible overflow uses its padding box as `rootRect`.
   Update the module doc line "a root's clipping by its own overflow ancestors is not modelled".
3. Compile ONCE: `cd handlers/aether && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target cargo check --tests`
   and `cd unaos/libs/web/css_core && CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target cargo check --tests`.
   Likely compile risks: `vm.get(obj, &PropertyKey)` signature in toplayer `track` (copied from observers.rs `update`,
   which calls `vm.get(g, &PropertyKey::from_str(..))`); `Value::as_object()` exists (used in dom.rs `node_of`);
   `Specificity` tuple ordering (it impls Ord); `crate::js::page` is pub(crate) (ok from css/mod.rs);
   `<AetherHost as layout_core::Host>::parse_color_alpha` path; edition 2024 `is_none_or` fine.
4. `docs/dev/evidence/aetherwpt/WPT.md`: new section `## AETHERTOP (SR146)` after SR145's (ends ~l.640): what landed per
   spec section (HTML §4.16.3, §4.11.4, §6.12, §15.5.x; CSS Position 4 §top-layer; Fullscreen §6/§7; Intersection
   Observer §3.2.5), "What the fold should expect" (html/semantics/popovers pages using popover-utils.js visibility
   asserts move; the-dialog-element `dialog-showModal`/`inert-*` partly; the `centering` / `top-layer-position` layout
   pages stay red: no fit-content), the dated gap list (fit-content → layout_core css.rs change; lib.rs `hit_origin` /
   hit test does not hoist the top layer nor skip inert nodes; the focusing steps do not refuse inert elements; the
   CloseWatcher interface events untrusted; `@media`-conditioned `::backdrop` rules not evaluated; CSSColorValue —
   typed_om.rs is not in FILES).
5. `docs/dev/LEDGER.md`: SR146 row inserted after SR145 (l.264), 7 cells, owner `shared (handlers/aether, libs/web/css_core)`,
   platform `rmbp`, witness `host`, status `fixed-unflown — AETHERTOP on exec-host-aethertop @the commit carrying this row
   (parent 8da376cb): compiled (<command> rc=0), proof owed to the fold` + one sentence; no `|` in a cell, no claim word.
6. Pre-commit leg: `cd unaos && ./arroyo gates > gates.log 2>&1; echo rc=$?` (paste the GATES line); then
   `python3 tools/status-check.py`, `python3 unaos/scripts/appearance-check.py unaos`, `bash unaos/scripts/charter-check.sh`
   as the head said. Final commit `aether: AETHERTOP — …` with the design in the body (this file's DONE section is it),
   squashing or following the wip commit f63350e5 as the seat prefers. Do not push.

## Learned (not in the arc file)
- The prelude's top layer never touches the DOM when a popover shows: without `toplayer::bump()` → `touch()` the
  engine would neither restyle nor repaint; and `restyle_plan`'s scoped cascade would never re-read the three states
  (its `_ => Vec::new()` arm) — hence the generation counter in `StyleCache`.
- `relayout_damage` damages by box snapshots only; a backdrop is not a box, hence the whole-frame rule on a generation change.
- The hit test (`lib.rs` 2085-2310, `hit_origin`/`hit_context`) mirrors render's paint order but knows nothing of the
  top layer or inertness; lib.rs is outside FILES — name the change for the seat.
- Shared wave34 target dir was EMPTY at start; a full `cargo check --tests` of aether builds every dependency; disk
  was 3.0 GB free.
