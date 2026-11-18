# AETHERSCROLLRESTOREPERDOCUMENT (SR1171) — design (measured on be52ba97e = 414b466a5 + merge of SR1163 90f14f913)

## Handler and seam (CODEX §2)
handlers/aether (Ring 3 browser engine), seam = layout/mod.rs "AETHERSTYLE3 (SR103): scroll containers' offsets across
relayouts" — the engine-side scroll memory between layout_core's per-tree `LayoutTree::scroll` and the cascade's end.
No layout_core / css_core / html_core change. BP-handlers-1..14 KEPT.

## Measurement (merged tree)
- handlers/aether/src/layout/mod.rs: `SCROLLED` :121 / `SCROLL_DRAG` :124 / `SNAPPED` :126 (thread_local, keyed by
  `html_core::NodeId` — an arena index; every document's arena starts at 0, so ids REPEAT across documents/engines);
  `remember_scroll(tree)` :141 does `m.clear()` on BOTH maps then refills from this tree (=> engine B's remember wipes
  engine A's record); `restore_scroll(tree)` :165 (applies every saved id to whatever box has that id; snap section
  :180-198 writes SCROLLED back); `forget_scroll()` :201 (clears everything, every engine); `element_scroll_offset(el)` :259
  (reads SCROLLED by bare NodeId).
- Call sites: remember_scroll — layout/mod.rs:228 (continue_scroll_drag), :315 (scroll_frame), lib.rs:458 (scrollbar
  arrow step), lib.rs:845 (wheel scroll_by). restore_scroll — css/mod.rs:904 (end of EVERY cascade: the page's, the JS
  flushed layout's `with_layout` cssom.rs:980-981, and the nested-frame cascade). forget_scroll — lib.rs:1993 (SR1163,
  before the first cascade; `self.document` is still the OLD document there) and lib.rs:2003 (SR103, after
  `self.document = Some(new)`). element_scroll_offset — js/cssom.rs:1443, :1486, :1712, :3311 (bare id, script side).
- Identity in hand: every call site holds a `&LayoutTree` whose `node_map` values are `crate::dom::NodeRef`, and
  `NodeRef::arena()` (dom/mod.rs:261) is the document's `Rc<RefCell<html_core::Document>>` — NodeRef's own Eq/Hash already
  use `Rc::ptr_eq` / `Rc::as_ptr` of it (dom/mod.rs:133-145). So the document identity = the arena pointer, reachable at
  remember/restore with ZERO new parameters. The script side has it as `crate::js::page(|p| p.doc)` (js/mod.rs:236,
  the same arena; layout already calls into crate::js at layout/mod.rs:410).
- Nested cascade lib.rs:240-262 (brief said :258): `frames::snapshot` (frames.rs:379) copies the nested document into a
  FRESH arena each paint, then `css::apply_stylesheets(&mut lt)` -> css/mod.rs:904 restore_scroll against the top
  document's ids: an iframe panel whose snapshot index equals a scrolled parent panel's index takes its offset.
  Per-arena keying makes the snapshot's lookup empty (starts at 0) with no lib.rs token there.
- PAGE_SCROLL css/mod.rs:470: one thread-local Cell set only by `AetherEngine::tick` lib.rs:634 and read by
  `container_sizes` css/mod.rs:500 for scroll-state(stuck) in EVERY cascade (engine A's cascade after B's tick reads B's
  viewport scroll; the nested-frame cascade reads the top page's) => LEAKS the same way: owed
  AETHER-PAGE-SCROLL-PER-DOCUMENT, not widened.
- Also measured, not widened: SCROLL_DRAG / SCROLL_REQUESTS / SMOOTH (layout/mod.rs:124, :247, :250) are bare-NodeId
  thread-locals and forget_scroll still clears them for every engine: owed AETHER-SCROLL-REQUESTS-PER-DOCUMENT.

## Choice (by counting call sites)
Key change: 0 new parameters at remember (4 sites) / restore (1 site) / element_scroll_offset (4 sites); forget_scroll
gains ONE arg at its 2 sites. Owner move (maps into AetherEngine): restore runs at the end of css apply_stylesheets
(no engine in hand; also the JS `with_layout` cascade and the nested cascade) and element_scroll_offset is read from
cssom getters — >3 fns of new plumbing. => KEY CHANGE.

## Existing mechanism extended (by fn name), NOT duplicated
`remember_scroll` / `restore_scroll` / `forget_scroll` / `element_scroll_offset` keep their bodies' shape; the two maps
keep their names. No second scroll table, no change to layout_core::scroll.

## Data that moves and who owns its lock
- `SCROLLED: RefCell<HashMap<(usize, NodeId), (f32, f32)>>`, `SNAPPED: RefCell<HashMap<(usize, NodeId), (Option<NodeId>,
  Option<NodeId>)>>` — the usize = `Rc::as_ptr(arena) as *const () as usize`.
- New `SCROLL_DOCS: RefCell<Vec<(usize, std::rc::Weak<RefCell<html_core::Document>>)>>` in the same thread_local! block:
  the Weak PINS the arena's allocation, so a key's address cannot be reused while any entry carries it; a doc whose
  `strong_count() == 0` is pruned (its entries + its Weak) on the next `scroll_doc` call, only then is the address free.
- Owner: the engine thread (thread-locals, RefCell borrows are fn-local, never held across a call out). No new lock.

## FILES and the one-line shape of each change
1. handlers/aether/src/layout/mod.rs (seam file, in place in the SR103 section):
   - SCROLLED / SNAPPED key type `(usize, html_core::NodeId)`; add SCROLL_DOCS beside them.
   - remember_scroll: `let Some(d) = scroll_doc(tree) else { return };` then `m.retain(|k, _| k.0 != d)` instead of
     `m.clear()` (both maps), inserts `(d, n.id())` — forgets/refills THIS document only.
   - restore_scroll: `let Some(d) = scroll_doc(tree) else { return };` saved / targets filtered to `k.0 == d`; the snap
     section's insert/remove keyed `(d, e)`.
   - forget_scroll(doc: Option<&crate::dom::NodeRef>): retains entries whose key != that doc's arena (ONE document),
     keeps clearing SCROLL_DRAG / SCROLL_REQUESTS / SMOOTH as today.
   - element_scroll_offset(el): key = `crate::js::page(|p| p.doc.as_ref().map(arena ptr))`; lookup `(key, el)`.
   - New fn `scroll_doc(tree: &LayoutTree) -> Option<usize>` at the FILE TAIL under
     `// ---- AETHERSCROLLRESTOREPERDOCUMENT (LEDGER SR1171): the scroll memory keyed by document (named tail block) ----`
     (first node_map value's arena; registers its Weak in SCROLL_DOCS; prunes dead docs from all three).
2. handlers/aether/src/lib.rs: ONE token each — :1993 `layout::forget_scroll(self.document.as_ref());` (the OLD
   document) and :2003 `layout::forget_scroll(self.document.as_ref());` (the new one: keeps SR103's wipe of the first
   cascade's snap writes). lib.rs:240-262 and css/mod.rs:904: NO change (identity comes from the tree).
3. handlers/aether/src/engine_tests.rs tail block `// ---- AETHERSCROLLRESTOREPERDOCUMENT (LEDGER SR1171) ... ----`,
   reusing SR1163's `nested_pixels_body` / `damage_rects_engine` / `nested_pixels_wheel_panel` / `nested_pixels_panel_off`:
   - `scroll_restore_per_document_other_engine_keeps`: A panel wheeled 60; B = a new engine loading a document; A.relayout()
     -> A's panel 60 (armed: 0, B's load cleared it).
   - `scroll_restore_per_document_iframe_starts_unscrolled`: parent `<div overflow:auto>` 20 coloured 20px rows +
     `<iframe srcdoc>` whose document has the same prefix markup so its snapshot panel lands on the parent panel's
     NodeId; parent panel wheeled 60; relayout + render_frame (layout_epoch moves, frames re-snapshot); the iframe's
     `frames::child_surface` top-left pixel is row 0's colour (armed: row 3's) and the parent panel keeps 60 across a
     second relayout. (The collision is checked in the first EXCEPTION run with an eprintln, removed after.)
   - `scroll_restore_per_document_same_document_restores`: wheel 60, relayout twice -> 60 (SR103 pin).
   - SR1163's `scroll_blit_nested_scroller_pixels_*` unchanged (pin).
4. docs/dev/blueprint/handlers.md L90 IN PLACE: SR1163's `owed AETHER-SCROLL-RESTORE-PER-DOCUMENT (...)` becomes
   `AETHER-SCROLL-RESTORE-PER-DOCUMENT BUILT by SR1171 (...; owed AETHER-PAGE-SCROLL-PER-DOCUMENT,
   AETHER-SCROLL-REQUESTS-PER-DOCUMENT)`; sheet stays 119 lines, paren delta unchanged.
5. docs/dev/LEDGER.md: SR1171 row at the END (7 cells, fixed-unflown, test command + counts).

## Order of the build
After PASS: tests first, EXCEPTION run #1 on the unchanged mechanism (the two armed tests RED, collision eprintln) =
the M1 reproduction; then the layout/lib.rs change; run #2 green (#3 spare). Then commit, then merge trunk.

## Wire the fold should see
`cargo test -p aether --lib -- scroll_restore scroll_blit nested_scroll scroll_top frames`: the 3 new
`scroll_restore_per_document_*` pass, every `scroll_blit_nested_scroller_pixels_*` and existing scroll_top / frames
tests unchanged.

## Owed (as §5 clauses inside the row, not built)
AETHER-PAGE-SCROLL-PER-DOCUMENT (measured leaking: PAGE_SCROLL css/mod.rs:470), AETHER-SCROLL-REQUESTS-PER-DOCUMENT
(SCROLL_DRAG / SCROLL_REQUESTS / SMOOTH bare-NodeId thread-wide; forget clears every engine's), AETHER-SCROLL-STATE-LOAD-SCROLL
carried (SR1166), AETHER-SCROLL-DAMAGE-FIXED-EXACT carried.
