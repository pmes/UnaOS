# AETHERDIRPSEUDOAUTO (SR1176) — design (cut 414b466a5, measured on the worktree)

CHARTER: CODEX §2 handler Aether (Web). SEAM: shared-core — css_core (no_std, zero deps, the matcher over the abstract `Element` trait) owns `:dir()`; aether's dom/select.rs `El` is the one impl that has a DOM with text.

## Measured at the cut
- css_core/src/matching.rs `match_pseudo` :392; the `PseudoClass::Dir(rtl)` arm is :428-441 (anchor unmoved): an ancestor walk over `e.attr("dir")`; `rtl`/`ltr` (eq_ignore_ascii_case, no trim) decide; ANY other value — `auto`, invalid — falls through to `cur = e.parent_element()` (:439), i.e. dir=auto is treated as inherit; no ancestor: ltr (:441).
- `pub trait Element` matching.rs:16-87: local_name, namespace_url, each_attr, parent/prev/next/first_child_element, is_empty, is_root, state, attr, has_attr, id_is, has_class, lang. NO text access, no child-node (text) iteration.
- css_core Cargo.toml: `[dependencies]` EMPTY by charter (CSSCORE SR47: "NO dependencies") -> css_core CANNOT depend on font_core; the resolver takes a `fn(char) -> Option<bool>` strong-class callback (no table copied).
- `MatchContext` matching.rs:179 is built by struct literal at 6 sites (aether select.rs :99 :355 :378 :379, css/mod.rs :582, css_core tests/selectors.rs :25) -> a new context field is out; the trait is the seam.
- Element impls: handlers/aether/src/dom/select.rs:131 (`El<'_>`, the engine's), css_core tests/common/dom.rs:133, layout_core tests/common/mod.rs:141, unaos/crates/user-aether/src/style.rs:143 (metal). Only aether's gets the new method; the others keep the default (byte-identical behaviour: dir=auto inherits as today).
- font_core::bidi (pub mod, font_core/src/lib.rs:31) `bidi_class(c) -> BidiClass` bidi.rs:18; aether already depends on font_core (Cargo.toml:55) and html_core (`NodeData::Text(String)`, `Document::children`, dom.rs:82/:193).
- layout_core tree.rs `default_rtl` :1135 (anchor unmoved): dir rtl/ltr else inherited; dir=auto not resolved on trunk (SR1165 queued). Not edited here.
- INVALIDATION (M1): css_core invalidation.rs header: `:dir()` sets `InvalidationSet::whole`; aether css/mod.rs `restyle_plan` :1096 returns None on `set.whole` (:1108) -> the whole document re-cascades. A text mutation: js/dom.rs `replace_data` :1605 does `touch()` + `restyle_root(Change::Text{..})`; `set_text_content` :1099 on an element goes `replace_all` (insert path: `touch()` + Change::Children). So a text change inside a dir=auto element already reaches a full re-match on the next restyle: AETHER-DIR-PSEUDO-AUTO-INVALIDATION is NOT owed; the test pins it (ARMED).

## Existing mechanism extended (by fn name)
`match_pseudo`'s `PseudoClass::Dir` arm (its ancestor walk stays the one directionality walk) and the `Element` trait (one defaulted method, the shape of `state`/`lang`). NOT duplicated: no second ancestor walk, no bidi table in css_core (font_core's `bidi_class` is the one Bidi_Class), no copy of layout_core's direction code, no new MatchContext field, no invalidation change.

## Data / locks
None move. The walk reads the DOM under the cascade's existing shared `&Document` borrow (El borrows the arena); no new state, no lock.

## FILES and the one-line shape of each change
1. unaos/libs/web/css_core/src/matching.rs
   a. trait Element: ONE method `fn dir_auto_rtl(&self) -> Option<bool> { None }` (doc: Some(true) rtl, Some(false) ltr, None = no strong character / no text access; impls answer through `dir_auto_first_strong`).
   b. the `:dir` arm IN PLACE: ONE inserted line inside the `if let Some(d) = e.attr("dir")` block after the ltr check: `if d.eq_ignore_ascii_case("auto") && let Some(r) = e.dir_auto_rtl() { return r == *rtl; }` (None -> falls to the parent, the spec's "no strong character -> parent's directionality"; root -> ltr unchanged).
   c. ONE tail fn under `// ---- AETHERDIRPSEUDOAUTO (LEDGER SR1176): dir=auto's first strong character for :dir() (named tail block) ----` at the END of the file, top level:
      `pub fn dir_auto_first_strong<E: Element>(el: &E, children: &dyn Fn(&E, &mut dyn FnMut(Option<&str>, Option<&E>) -> bool) -> bool, strong: fn(char) -> Option<bool>) -> Option<bool>` — HTML §3.2.6.4 "auto directionality": child nodes in tree order; a text node -> its first char with a strong class decides; an element child -> recurse unless it is an HTML bdi / script / style / textarea or carries a dir attribute in a defined state (ltr / rtl / auto, ASCII case-insensitive); stops at the first decision. `children` is the impl's child-node visitor (text as `Some(&str), None`, element as `None, Some(&E)`), `strong` its Bidi_Class (L -> false, R / AL -> true). This is the ONE first-strong-over-a-tree walk; layout_core calls it later (owed AETHER-DIR-AUTO-ONE-WALK).
2. handlers/aether/src/dom/select.rs `impl Element for El`: ONE method `dir_auto_rtl` = `css_core::matching::dir_auto_first_strong(self, &|e, f| e.doc.children(e.id).any(|c| match e.doc.data(c) { NodeData::Text(t) => f(Some(t), None), NodeData::Element(_) => f(None, e.hop(Some(c)).as_ref()), _ => false }), |c| match font_core::bidi::bidi_class(c) { L => Some(false), R | AL => Some(true), _ => None })` (featureless scoped host: None).
3. Tests:
   - unaos/libs/web/css_core/tests/selectors.rs TAIL (named block): a small text-carrying test DOM implementing `Element` + `dir_auto_rtl` through `dir_auto_first_strong` with a test strong fn (Hebrew block U+0590-U+05FF rtl, ASCII letters ltr — a test stand-in, not a table copy). Tests `dir_auto_*`: hebrew-first -> :dir(rtl); latin-first -> :dir(ltr); neutrals only -> parent's (dir=rtl parent -> rtl; root -> ltr); child of an auto-rtl p matches :dir(rtl); bdi / script / style content skipped; a descendant with dir= skipped, one with an invalid dir value not skipped; explicit dir=rtl / ltr pins; an impl without the method (tests/common El) keeps dir=auto -> inherit.
   - handlers/aether/src/engine_tests.rs TAIL (named block, inside the file's single `mod tests` before its closing brace: every tail block in engine_tests.rs lives there because the whole file is that one test mod) `dir_pseudo_auto_*`: `<p dir=auto class=t id=a>שלום</p>` + `.t:dir(rtl){color:red}` -> getComputedStyle color rgb(255, 0, 0) (armed); Latin-first `:dir(ltr)` matches; neutrals -> the parent's; child of the auto-rtl p (no own dir) red (armed); `<bdi>` content skipped; explicit dir=rtl / ltr pins; `a.textContent = 'abc'` then render_frame -> not red (ARMED, the invalidation above).
4. docs/dev/blueprint/handlers.md L90 IN PLACE: SR1165's word is NOT on the cut's row, so the clause `AETHER-DIR-PSEUDO-AUTO BUILT by SR1176 (...)` goes inside the AETHER-CSSOM-UNICODE-BIDI / SR1008 clause (appended to its owed list after `AETHER-CSS-WIDE-NONINHERITED-INHERIT for unicode-bidi`), balanced parentheses, sheet stays 119 lines. BP-handlers-1..14 KEPT.
5. docs/dev/LEDGER.md: SR1176 row at the END of the table (after SR1125 at :1051), 7 cells, fixed-unflown, the EXCEPTION commands and counts in the last cell.

## Wire the fold should see
`cargo test -p css_core` gains the `dir_auto_*` tests (tests/selectors.rs); `cargo test -p aether --lib -- dir_pseudo` runs the `dir_pseudo_auto_*` engine tests. No report column / REPORT_COLS change; no kernel / metal file touched.

## Note on behaviour seam
Until SR1165 folds, layout_core's `default_rtl` still lays out dir=auto as inherited, so a hebrew dir=auto paragraph matches :dir(rtl) while its `direction` stays ltr; the selector is right per spec, the layout catches up with SR1165 and unifies with AETHER-DIR-AUTO-ONE-WALK.

## Owed (as §5 words inside the row, NOT built)
- AETHER-DIR-AUTO-ONE-WALK: after SR1165 folds, layout_core's `dir_auto_rtl` / `first_strong_rtl` calls css_core `dir_auto_first_strong` so the first-strong rule exists once.
- AETHER-DIR-AUTO-INPUT-VALUE (carried): input / textarea dir=auto resolve from the control's value, not its child text.
- AETHER-DIR-BDI-AUTO: a `<bdi>` with no defined dir is auto (HTML §4.5.24); the arm treats it as inherit.
- AETHER-DIR-PSEUDO-AUTO-METAL: user-aether's style.rs `El` keeps the default (no text access) -> dir=auto inherits on the metal build.
- AETHER-DIR-PSEUDO-AUTO-INVALIDATION: NOT owed (measured armed: `:dir` -> whole restyle, text mutation records Change::Text / Children).
