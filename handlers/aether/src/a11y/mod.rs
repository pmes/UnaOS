//! CHARTER: Aether (docs/CODEX.md §2, Web) — seam: handler (host-native Ring 3, handlers/aether), the
//! browser's accessibility layer; a reader of the engine's DOM and computed style, never a second store
//! of either (the one thing it keeps of its own is the sequential-navigation starting point, HTML §6.6.3).
//!
//! AETHERA11Y (LEDGER SR119): the accessibility half of the browser.
//!
//! | piece                     | module        | what it reads / writes                                                       |
//! |---------------------------|---------------|-------------------------------------------------------------------------------|
//! | roles                     | [`role`]      | the element and its `role` attribute: HTML-AAM implicit roles, WAI-ARIA 1.2 explicit roles (first valid token, abstract roles refused, the presentational-role conflict rule), landmarks |
//! | names and descriptions    | [`name`]      | accname 1.2 §4.3 steps 2A–2I over the DOM: `aria-labelledby`, `aria-label`, the host language's labels (`<label>`, `alt`, `<legend>`, `<caption>`, `<figcaption>`, button values), embedded controls, name from content, `title`, `placeholder` |
//! | the tree                  | [`tree`]      | `AetherEngine::document` + `css::computed_report` over `AetherEngine::layout_tree` (display and visibility as layout consumed them): hidden subtrees pruned, generic wrappers flattened, presentational children folded, states and properties |
//! | sequential navigation     | [`focus`]     | HTML §6.6.3: focusable areas, `tabindex` order, radio-group stops; moves focus on `AetherEngine::focused_node` AND the script's `document.activeElement` with the focus events, scrolls into view, arms the focus ring |
//! | keys                      | [`keyboard`]  | Tab / Shift+Tab, Enter on links, Enter/Space on `<summary>`, arrows in radio groups and ARIA listboxes |
//! | shell verbs               | [`nav`]       | skip to content, next/previous heading (any level or one), next/previous landmark; A11YFACE (SR124): bandy's `A11yVerb` ([`nav::run`]) and the keys Alt+1 / H / D ([`keyboard::reading_key`]) |
//! | preferences               | [`prefs`]     | Principia's `system.accessibility.*` rows → css_core's `prefers-reduced-motion` / `prefers-contrast` / `forced-colors` and the focus-ring theme |
//!
//! | AT-SPI cache              | [`atspi`]     | A11YFACE (SR124): the tree as AT-SPI2's `GetItems` objects with Component extents and Text; the D-Bus side is refused (no D-Bus crate, R83) — the shell serves the shape on its own socket |
//!
//! THE ONE READ API is [`tree`] (`a11y::tree(&engine) -> AxTree`): the GTK face (AT-SPI later) and the
//! devtools Accessibility panel ([`crate::devtools::accessibility`]) both read it; nothing caches it, so a
//! script's mutation shows on the next read.

pub mod atspi;
pub mod focus;
pub mod keyboard;
pub mod name;
pub mod nav;
pub mod prefs;
pub mod role;
pub mod tree;

pub use tree::{AxNode, AxTree, States, Tristate};

use crate::dom::NodeRef;
use crate::AetherEngine;

/// The accessibility tree of the engine's current document (empty before the first load).
pub fn tree(engine: &AetherEngine) -> AxTree {
    tree::build(engine)
}

/// An element's lowercased local name ("" for a non-element).
pub(crate) fn tag(n: &NodeRef) -> String {
    n.as_element().map(|e| e.name.local.0.to_ascii_lowercase()).unwrap_or_default()
}

/// A no-namespace attribute's value.
pub(crate) fn attr(n: &NodeRef, name: &str) -> Option<String> {
    n.as_element().and_then(|e| e.attributes.borrow().get(name).map(str::to_string))
}

/// Whether the element carries attribute `name`.
pub(crate) fn has_attr(n: &NodeRef, name: &str) -> bool {
    n.as_element().is_some_and(|e| e.attributes.borrow().contains(name))
}

/// An ARIA true/false attribute: `Some(true)` for "true", `Some(false)` for "false", None when absent or
/// any other value (WAI-ARIA 1.2 §6.2.1 true/false: "undefined" is the default).
pub(crate) fn aria_bool(n: &NodeRef, name: &str) -> Option<bool> {
    match attr(n, name)?.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Whitespace collapsed to single spaces and trimmed (accname 1.2 §4.3 step 3, "flat string").
pub(crate) fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The element with `id` in `n`'s document (the first in tree order, as `getElementById`).
pub(crate) fn by_id(n: &NodeRef, id: &str) -> Option<NodeRef> {
    if id.is_empty() {
        return None;
    }
    n.document_node().descendants().find(|d| d.as_element().is_some_and(|e| e.attributes.borrow().get("id") == Some(id)))
}

/// The ids an IDREF-list attribute names, resolved in order (missing ids skipped).
pub(crate) fn id_refs(n: &NodeRef, name: &str) -> Vec<NodeRef> {
    attr(n, name).map(|v| v.split_whitespace().filter_map(|id| by_id(n, id)).collect()).unwrap_or_default()
}
