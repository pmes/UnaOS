# AETHEROUTLINESTICKYHELD (SR1174) — DESIGN (measured on 1fa32db4a = 414b466a5 + merge of exec-rmbp-aetherstickyouterhold cda78be59)

CHARTER: docs/CODEX.md §2 handler = Aether (handlers/aether); seam = shared-core layout_core (unaos/libs/web/layout_core, no_std, the ONE sticky rule SR1167 put in scroll.rs) read by Aether's painter (render/focusring.rs) and its shell input (lib.rs). No new module.

## M1 measurement (merged tree)
- layout_core/src/scroll.rs: `box_origin` :652 (UNHELD flow origin; handles the fixed pin via `fixed_at`: yes), `sticky_hold` :789 (a box's OWN hold), `box_origin_held` :834 (box_origin + every STRICT ancestor's hold; stops per axis past a pinning fixed box; does NOT include the box's own hold), `sticky_shift` :866 (held + own - flow), `scrollers_at` :606 (its own placement walk: content shifts, IFC atomics, fixed against vs — NO sticky holds), `hit_scrollbar` :576 (local coords).
- MEASURED SURPRISE 1: `box_origin_held` excludes the box's own hold. A focused element that is itself sticky, or a sticky scroller (a sticky TOC sidebar with overflow:auto: common), is painted at box_origin + sticky_shift(id). The painted place of `id` = `box_origin(id) + sticky_shift(id)` (= box_origin_held + sticky_hold(id)).
- MEASURED SURPRISE 2: lib.rs `scrollbar_press` :434 gets its candidates from `scrollers_at(tree, px, py, vs)` :438, which places no sticky hold: at the PAINTED point of a list inside a stuck header the list is not returned at all, so changing only the :439 origin cannot fix the press. The same walk feeds the wheel chain (lib.rs :839): a wheel over the stuck list scrolls the page, not the list.
- MEASURED SURPRISE 3: the UA focus ring (focusring.rs `paint` :312) is not placed by `box_origin` but by focusring's own `box_of` :171 — a naive `location` sum that ignores content_shift (inner scroller offsets), IFC atomics, the fixed pin and sticky holds. The arc's ":270 box_origin" is `paint_outlines` :264 (author outlines, incl. every site's `:focus-visible { outline: ... }`). Both draw "the focus ring"; both are painted-wanted.
- Fixed ancestors (SR1135/SR1139): `box_origin` handles the pin (yes) for focusring :264, lib.rs :439, lib.rs :716, render :4956; focusring `box_of`: NO (it ignores `fixed_at`: a UA ring on a link in a fixed header at page scroll s is drawn s px off). `box_origin_held` / `sticky_shift` stop at a pinning fixed box: yes.
- (vs, viewport) in hand: focusring paint / paint_outlines: (sx, sy) and (width, height) as u32 — the same pair render's `sticky_scrollport` adapter (:4960) passes to `sticky_shift`. lib.rs scrollbar_press: vs = (scroll_x as i32, scroll_y as i32), viewport (self.width, self.height) as the SR1167 probe uses. scrollers_at: only `vs`; the tree carries `viewport` (LayoutTree::viewport, the (vw, vh) the tree laid out against = the engine's CSS viewport).

## Classification of every box_origin reader (and the two placement walks)
| reader | wants | change |
|---|---|---|
| layout_core scroll.rs :750 sticky_scrollport parent / :764 port | flow (SR1167's rule builds holds ON the flow place) | keep |
| layout_core scroll.rs :795 / :835 / :867 inside sticky_hold / box_origin_held / sticky_shift | flow (the rule's base) | keep |
| render/mod.rs :4956 sticky_scrollport adapter `d = abs - shift - box_origin(P)` | flow (it measures painted minus flow) | keep |
| lib.rs :716 top-layer hit, parent origin | flow, mirroring render `paint_top_layer` (parent's border-box origin; the UA rule makes the box fixed, so placed against the viewport) | keep |
| render/focusring.rs :264 paint_outlines (author outline) | PAINTED | ONE call |
| render/focusring.rs :312 paint, the UA ring via `box_of` | PAINTED | ONE call (box_of retired: its only caller) |
| lib.rs :439 scrollbar_press origin | PAINTED | ONE call |
| layout_core scroll.rs :634 scrollers_at child origin `ko` (feeds scrollbar_press :438 and the wheel :839) | PAINTED (doc: "Positions follow the painter") | ONE token |
| lib.rs AetherEngine::box_of :587 (scroll_into_view, a11y, devtools) | its own naive walk; not this arc | owed |

## Existing mechanism I extend (not duplicate)
SR1167's `sticky_shift` / `box_origin_held` / `sticky_hold` are the ONE rule. I add NO hold math. ONE 3-line composition at the scroll.rs tail: `pub fn box_origin_painted(tree, id, vs, viewport) -> Option<(f32,f32)>` = `box_origin(id)` + `sticky_shift(id)` (where the painter draws `id`'s own border box), so the three painted readers call one fn instead of each composing box_origin+sticky_shift. scrollers_at's walk adds `sticky_hold(tree, k, vs, tree.viewport)` to the child's origin (each level adds its own hold, so the walk's origin accumulates exactly box_origin_painted). NOT duplicated: no second hold rule, no change to placed_origin (AETHER-STICKY-ONE-RULE-PAINTER stays owed), no change to box_origin's flow semantics.

## Data / locks
Read-only geometry over `&LayoutTree` (taffy layout, paint_map, fixed_at) at the caller's vs; no new state, no lock (scrollbar_press holds `&mut` tree as today; the call is a read before `begin_scroll_drag`).

## FILES and the one-line shape of each change
1. unaos/libs/web/layout_core/src/scroll.rs (SEAT GRANT NEEDED: not in the arc's FILES; SR1167's file): (a) the `ko` else-arm line in scrollers_at's walk gains ` + sticky hold` in place (one line: `let (hx, hy) = sticky_hold(tree, k, vs, tree.viewport);` folded on the same line as the `ko` let... concretely the line `(shifted.0 + kl.location.x + cs.0, shifted.1 + kl.location.y + cs.1)` becomes the same tuple plus `sticky_hold(tree, k, vs, tree.viewport)` components via a let on the existing line); (b) tail block `// ---- AETHEROUTLINESTICKYHELD (LEDGER SR1174): ... ----` with `box_origin_painted`.
2. handlers/aether/src/render/focusring.rs: :264 `box_origin(layout, id, vs)` -> `box_origin_painted(layout, id, vs, (width as f32, height as f32))`; :312 `box_of(layout, &n)` -> `id_of(layout, &n).and_then(|id| Some((box_origin_painted(...)?, layout.taffy.layout(id).ok()?.size))).map(|((x,y),s)| (x,y,s.width,s.height))` on the one line; the now-callerless `box_of` fn (:170-187) deleted (it would be dead code).
3. handlers/aether/src/lib.rs: :439 `box_origin(tree, id, vs)` -> `box_origin_painted(tree, id, vs, (self.width as f32, self.height as f32))` (self.width/height read into a local before the `&mut` borrow if the borrow checker needs it: then on the existing `vs` line :436).
4. handlers/aether/src/engine_tests.rs: tail block `sticky_held_readers_*` (below).
5. docs/dev/blueprint/handlers.md L90: AETHER-OUTLINE-STICKY-HELD and AETHER-SCROLLBAR-PRESS-STICKY-HELD rewritten IN PLACE inside SR1167's owed list as `... BUILT by SR1174 (...)`; paren balance kept; 119 lines.
6. docs/dev/LEDGER.md: SR1174 row at the END.

## M3 tests (engine_tests tail, page 400x300, body margin 0, 100px spacer, sticky top:0 header 160px blue, 2000px spacer)
- `sticky_held_readers_ua_ring_on_stuck_link` (armed): link `#lk` (block, 20px, at 20 inside the header) set as the ring target (`focusring::set_target`, full damage, render); scrolled 300: ring ink (16,16,16) in the band around the link's painted rect at viewport y 18..42 (today: the flow place is viewport -180, no ring there); reset target after.
- `sticky_held_readers_author_outline_on_stuck_link` (armed): `#lo` with `outline: 2px solid red` in the header: red outline ink at its painted rect at scroll 300.
- `sticky_held_readers_scrollbar_press_stuck_list` (armed): a 100px `overflow:auto` list (400px content) in the header; scrolled 300; the vertical thumb (from `scroll::scrollbars`) pressed at its painted viewport place (flow y - header flow y), moved 30px, released: the list's offset > 0 (today: the press misses, offset 0).
- `sticky_held_readers_wheel_stuck_list` (armed): the wheel over the stuck list scrolls the list, not the page.
- `sticky_held_readers_pins_at_scroll_zero` (pin): the ring, the outline and the thumb drag at scroll 0 (flow == painted) all work as before.
- `sticky_held_readers_flow_link_pin` (pin): a focused link in normal flow below the header at scroll 300: ring at its flow place minus the scroll.
- SR1131 outline tests, SR1119/SR1124 outline tests and SR1167's `sticky_outer_hold_*` unchanged.
EXCEPTION run: `cargo test -p aether --lib -- sticky focus outline scrollbar` per the brief's env (touch list incl. scroll.rs).

## Wire the fold should see
No serial wire (host arc). `cargo test -p aether --lib -- sticky focus outline scrollbar`: the new `sticky_held_readers_` tests among the passed; `layout_core --test scroll` unchanged (scrollers_at with no sticky box is byte-identical: sticky_hold returns (0,0) for a non-sticky box).

## Owed (§5 inside my clause, not built)
AETHER-STICKY-ONE-RULE-PAINTER carried; AETHER-CSSOM-BOUNDING-STICKY-NESTED carried (SR1173 beside me); AETHER-ENGINE-BOXOF-PAINTED (AetherEngine::box_of :587 is the same naive walk: scroll_into_view, a11y bounds and devtools read flow-without-scroller-offset/fixed/sticky); AETHER-SCROLLERS-AT-ATOMIC-STICKY (scrollers_at's IFC-atomic arm adds no hold: a sticky atomic is rare). AETHER-FOCUSRING-FIXED-HELD: the UA ring's fixed-ancestor fault is FIXED by change 2 (box_of retired), so it is closed in the row, not owed.

## Asks to the seat
(1) GRANT unaos/libs/web/layout_core/src/scroll.rs (one in-place line in scrollers_at + a tail fn). Without it scrollbar_press cannot be fixed by its origin call alone (Surprise 2); fallback if refused: build focusring only and owe AETHER-SCROLLBAR-PRESS-STICKY-HELD with this measurement. (2) Accept retiring focusring's `box_of` (its only caller moves to the painted origin).
