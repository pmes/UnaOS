# AETHERA11Y (SR119) — role and name coverage

What `handlers/aether/src/dom/a11y/` (source: `handlers/aether/src/a11y/`) computes, and the test in
`handlers/aether/tests/a11y.rs` that pins each row. Tests are written, not run (R107): every row is
"fixed-unflown" until the seat's fold runs `cargo test -p aether --test a11y`.

## Implicit roles (HTML-AAM 1.0 §3.4) — `role.rs` `implicit_role`

| element | role | condition | test |
| :-- | :-- | :-- | :-- |
| `a`, `area` | link / generic | `href` present / absent | implicit_roles_follow_html_aam |
| `article` | article | | implicit_roles_follow_html_aam |
| `aside` | complementary | | implicit_roles_follow_html_aam |
| `blockquote` | blockquote | | implicit_roles_follow_html_aam |
| `button` | button | | implicit_roles_follow_html_aam |
| `caption` | caption | | — |
| `code` | code | | — |
| `datalist` | listbox | (never rendered: pruned) | — |
| `dd` / `dt`, `dfn` | definition / term | | — |
| `del`, `s` / `ins` | deletion / insertion | | — |
| `details` | group | | implicit_roles_follow_html_aam |
| `dialog` | dialog | | implicit_roles_follow_html_aam |
| `em` / `strong` | emphasis / strong | | — |
| `fieldset`, `optgroup`, `address`, `hgroup` | group | | implicit_roles_follow_html_aam (fieldset) |
| `figure` | figure | | implicit_roles_follow_html_aam |
| `footer` | contentinfo / generic | outside / inside article, aside, main, nav, section | implicit_roles_follow_html_aam |
| `form` | form | a landmark only when named | implicit_roles_follow_html_aam, landmarks_are_listed_in_order |
| `h1`–`h6` | heading, level N | | implicit_roles_follow_html_aam |
| `header` | banner / generic | outside / inside sectioning | implicit_roles_follow_html_aam |
| `hr` | separator | | implicit_roles_follow_html_aam |
| `img` | img / none | `alt=""` with no title or aria-label → none | implicit_roles_follow_html_aam, name_from_host_language |
| `input` button, image, reset, submit | button | | implicit_roles_follow_html_aam |
| `input` checkbox / radio | checkbox / radio | | implicit_roles_follow_html_aam |
| `input` range / number | slider / spinbutton | | implicit_roles_follow_html_aam |
| `input` search | searchbox / combobox | `list` absent / present | implicit_roles_follow_html_aam |
| `input` text, email, tel, url, password, date… | textbox / combobox | `list` absent / present | implicit_roles_follow_html_aam |
| `input` color, file | button | (Chromium's mapping; HTML-AAM: none) | — |
| `li` | listitem / generic | parent `ul`/`ol`/`menu`/role list / otherwise | implicit_roles_follow_html_aam |
| `main` | main | | implicit_roles_follow_html_aam |
| `math` | math | | — |
| `menu`, `ol`, `ul` | list | | implicit_roles_follow_html_aam |
| `meter` | meter | | implicit_roles_follow_html_aam |
| `nav` | navigation | | implicit_roles_follow_html_aam |
| `option` | option | | implicit_roles_follow_html_aam |
| `output` | status | | implicit_roles_follow_html_aam |
| `p` | paragraph | | implicit_roles_follow_html_aam |
| `progress` | progressbar | | implicit_roles_follow_html_aam |
| `search` | search | | landmarks_are_listed_in_order |
| `section` | region / generic | named (aria-label, aria-labelledby, title) / not | implicit_roles_follow_html_aam |
| `select` | combobox / listbox | single, size ≤ 1 / `multiple` or size > 1 | implicit_roles_follow_html_aam |
| `sub` / `sup` | subscript / superscript | | — |
| `summary` | button / generic | a details' summary (Chromium's disclosure button) / otherwise | implicit_roles_follow_html_aam |
| `svg` | img / generic | named (incl. a `<title>` child) / not | — |
| `table` | table | | implicit_roles_follow_html_aam |
| `tbody`, `thead`, `tfoot` | rowgroup | | — |
| `td` | cell | | implicit_roles_follow_html_aam |
| `textarea` | textbox (multiline) | | implicit_roles_follow_html_aam |
| `th` | columnheader / rowheader | `scope=row`/`rowgroup` → rowheader | implicit_roles_follow_html_aam |
| `time` | time | | — |
| `tr` | row | | implicit_roles_follow_html_aam |
| everything else (`div`, `span`, `label`, `body`, …) | generic | flattened when unnamed and unfocusable | implicit_roles_follow_html_aam |

## Explicit roles (WAI-ARIA 1.2 §5.4, §9.1) — `role.rs` `explicit_role`, `computed_role`

| rule | test |
| :-- | :-- |
| the 82 concrete roles; the first valid token wins | explicit_roles_and_presentation_conflict (`bogus switch checkbox` → switch) |
| the 12 abstract roles are never valid | explicit_roles_and_presentation_conflict (`widget button` → button, `landmark` → generic) |
| `presentation` = `none`: the element's semantics removed | explicit_roles_and_presentation_conflict |
| presentation ignored on a focusable element or one with a global aria-* | explicit_roles_and_presentation_conflict |
| presentation inherited by required owned elements (li in ul, tr/td in table, dt/dd) | explicit_roles_and_presentation_conflict |
| landmarks: banner, complementary, contentinfo, form, main, navigation, region, search | landmarks_are_listed_in_order |

## Accessible name (accname 1.2 §4.3) — `name.rs` `alternative`, answered with `NameFrom`

| step | source | NameFrom | test |
| :-- | :-- | :-- | :-- |
| 2A | hidden nodes contribute nothing (unless the labelledby/describedby root was hidden) | — | name_precedence_labelledby_label_native, name_from_content_and_embedded_control |
| 2B | `aria-labelledby` (once per traversal; a self-reference names by the node's other steps) | LabelledBy | name_precedence_labelledby_label_native |
| 2C | `aria-label` (an embedded control inside another name gives its value instead) | AriaLabel | name_precedence_labelledby_label_native |
| 2D | `<label for>` and the wrapping `<label>` of a labelable element | Label | name_precedence_labelledby_label_native |
| 2D | `alt` of `img`, `area`, `input type=image` | Alt | name_from_host_language |
| 2D | first `<legend>` of a fieldset | Legend | name_from_host_language |
| 2D | a table's `<caption>` | Caption | name_from_host_language |
| 2D | a figure's `<figcaption>` | Figcaption | name_from_host_language |
| 2D | button input `value`, else "Submit" / "Reset"; image input "Submit" | Value | name_from_host_language |
| 2D | `option` / `optgroup` / `track` `label` | Value | name_from_host_language |
| 2D | an svg's `<title>` child; the document's `<title>` (root) | TitleElement | implicit_roles_follow_html_aam (root) |
| 2E | embedded control value: textbox value, select's selected option(s), range valuetext/valuenow/value | Value | name_from_content_and_embedded_control |
| 2F | name from content for the 18 roles ARIA 1.2 allows (and any node inside another name), block children spaced, `<br>` a space | Contents | name_from_content_and_embedded_control |
| 2I | `title` | Title | title_placeholder_and_description |
| HTML-AAM | `placeholder` / `aria-placeholder` of a text field, after title | Placeholder | title_placeholder_and_description |
| prohibited | caption, code, deletion, emphasis, generic, insertion, paragraph, strong, sub/superscript, none: no name | None | explicit_roles_and_presentation_conflict |
| description | `aria-describedby`, else `aria-description`, else an unused `title` | — | title_placeholder_and_description |

## Tree, states, navigation, preferences

| rule | test |
| :-- | :-- |
| pruned: not rendered (display:none box or no box), hidden attribute, aria-hidden=true, inert, metadata | hidden_subtrees_are_pruned |
| visibility:hidden left out, a visibility:visible descendant kept | hidden_subtrees_are_pruned |
| states: checked (native + aria, mixed), pressed, expanded (aria + summary/details), selected (option, tab), disabled (+ fieldset), required, readonly, multiline, multiselectable, invalid, haspopup, current, live (+ implicit), value min/max/now (+ range defaults) | states_and_properties |
| Tab order: positive tabindex ascending, then 0/default in tree order; -1, disabled, hidden, inert skipped; wrap | tab_order_follows_tabindex |
| Shift+Tab / ISO_Left_Tab; Tab from a non-stop continues from its tree position; Ctrl+Tab not the page's | shift_tab_goes_backwards |
| a radio group is one stop (checked, else first/last); arrows focus + check, wrapping | radio_group_is_one_stop_and_arrows_check |
| ARIA listbox: Up/Down/Home/End, roving focus and aria-activedescendant, aria-selected follows focus, disabled options skipped | listbox_arrows_move_selection |
| Enter on a link follows it; Enter/Space on a summary toggles its details | enter_activates_links_and_summaries |
| Space/Return on a Tab-focused native button: the engine's own activation | space_activates_a_tabbed_button |
| focus events (blur/focusout/focus/focusin with relatedTarget), activeElement, :focus-visible | focus_moves_fire_events_and_match_focus_visible |
| the focus ring (2px #101010 + 1px white halo; none after pointer focus; 3px black under more contrast) | focus_ring_paints_from_the_theme |
| skip to content (main), heading navigation (any level / one level, wrap, backwards), landmark navigation | skip_to_content_and_heading_navigation |
| skip to content through the page's own skip link | skip_link_target_without_main |
| Principia `system.accessibility.*` → prefers-reduced-motion, prefers-contrast, forced-colors | accessibility_media_features_follow_principia |
| devtools Accessibility panel: tree rows, selected node's role / name (from) / states / tab order | devtools_accessibility_panel |

## Owed outside this arc's files (not made; the exact change)

1. `handlers/aether/src/lib.rs` `key_default` (lib.rs:996), first statement:
   `if crate::dom::a11y::keyboard::handle_key(self, &key) { return; }` — the shell's `KeyDown` then
   reaches Tab / Enter-on-link / arrows (today only a caller of `handle_key` does). Optionally move the
   module declaration from `dom/mod.rs` (`#[path = "../a11y/mod.rs"]`) to lib.rs as `pub mod a11y;`.
2. `handlers/aether/src/lib.rs` MouseUp (lib.rs:869): `crate::dom::a11y::focus::set_starting_point(Some(node.clone()))`
   (HTML §6.6.3: a click moves the sequential navigation starting point) and keep `js` PageState.focused in step
   with `set_focus` (today a click focuses the engine's control but not `document.activeElement`).
3. `libs/quartzite/src/platforms/gtk/devtools_view.rs` `PANELS`: append `"Accessibility"` (6 entries).
4. The GTK face: read Principia `system.accessibility.*` and call
   `aether::dom::a11y::prefs::A11yPrefs::from_prefs(|k| …).apply(Some(&mut engine))` at start and on change;
   bind the shell verbs (`nav::skip_to_content`, `nav::next_heading`, `nav::next_landmark`) to keys / bandy verbs
   (`vessels/aether-shell/src/main.rs`, `libs/bandy/src/signals.rs`).
5. AT-SPI: a bridge over `a11y::tree` (the read API is in place; nothing exports it yet).
6. `outline: none` is not honored by the ring: layout_core carries no `outline` value to paint or read.
