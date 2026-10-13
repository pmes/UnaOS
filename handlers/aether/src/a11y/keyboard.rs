//! The keys keyboard users navigate a page with (HTML §6.6.3 and the WAI-ARIA Authoring Practices'
//! keyboard interaction for radio groups and listboxes):
//!
//! | key                         | on                                                     | does |
//! |-----------------------------|--------------------------------------------------------|------|
//! | Tab / Shift+Tab (`ISO_Left_Tab`) | anywhere (a text field too: Tab is never an edit) | focus moves to the next / previous sequential stop ([`super::focus::tab`]) |
//! | Enter                       | a link (`a`/`area` with `href`)                        | the link's activation: a trusted `click`, then (not canceled) the hyperlink is followed |
//! | Enter / Space               | a details' `summary`                                   | the summary's activation: the details toggles |
//! | Up/Left, Down/Right         | a radio button in a group                              | the previous / next enabled radio of the group is focused AND checked (its click: `input` / `change` fire) |
//! | Up/Down, Home/End           | an ARIA `listbox` option (or the listbox itself)       | the previous / next / first / last option is focused (roving focus) or made `aria-activedescendant`, and selected (`aria-selected`) when the listbox is single-select |
//!
//! | Alt+1                       | anywhere                                               | A11YFACE (SR124): skip to content ([`super::nav::skip_to_content`]) |
//! | H / Shift+H                 | no text field, editing host or typeahead widget focused | the next / previous heading ([`super::nav::next_heading`]) |
//! | D / Shift+D                 | likewise                                               | the next / previous landmark ([`super::nav::next_landmark`]) |
//!
//! The reading keys sit here, in the keydown's DEFAULT action (`AetherEngine::key_default`), so a page whose own
//! `keydown` listener cancels the key (a page shortcut on `h`) keeps it — the same seam Tab uses.
//!
//! Native buttons, checkboxes and radios keep the engine's own activation (`AetherEngine::key_default`:
//! Space, and Return on a button — form submission lives there), so [`handle_key`] answers false for
//! them and the engine's default runs.

use super::focus::{self, radio_group};
use super::tree::Rendering;
use super::{attr, has_attr, role, tag};
use crate::dom::NodeRef;
use crate::forms::edit::{parse_key, Key};
use crate::AetherEngine;

/// Handles a navigation key for the engine's page. Returns true when the key was used (the caller runs no
/// other default for it). `key` is the shell's key string (`Tab`, `Shift+Tab`, `ISO_Left_Tab`, `Return`,
/// `space`, `Up`, …).
pub fn handle_key(engine: &mut AetherEngine, key: &str) -> bool {
    // A11YFACE (SR124): the reading keys.
    if let Some(verb) = reading_key(engine, key) {
        super::nav::run(engine, &verb);
        return true;
    }
    let (mods, k) = parse_key(key);
    let Some(k) = k else { return false };
    if mods.ctrl || mods.alt || mods.meta {
        return false;
    }
    if k == Key::Tab {
        let backward = mods.shift || key.ends_with("ISO_Left_Tab");
        return focus::tab(engine, backward).is_some();
    }
    let Some(node) = engine.focused_node.clone() else { return false };
    // A text field keeps its editing keys.
    if crate::forms::is_text_editable(&node) {
        return false;
    }
    let t = tag(&node);
    match k {
        Key::Return if matches!(t.as_str(), "a" | "area") && has_attr(&node, "href") => follow_link(engine, &node),
        Key::Return | Key::Space if t == "summary" => click(engine, &node),
        Key::Up | Key::Left | Key::Down | Key::Right if radio_group(&node).is_some() => {
            radio_arrow(engine, &node, matches!(k, Key::Down | Key::Right))
        }
        Key::Up | Key::Down | Key::Home | Key::End => listbox_key(engine, &node, k),
        _ => false,
    }
}

/// Roles whose letter keys are typeahead (WAI-ARIA APG: listbox, menu, tree, grid type-to-select).
const TYPEAHEAD_ROLES: [&str; 10] = ["listbox", "option", "combobox", "menu", "menubar", "menuitem", "tree", "treeitem", "grid", "treegrid"];

/// A11YFACE (SR124): the reading verb a key asks for — Alt+1 skip to content; H / Shift+H the next / previous
/// heading and D / Shift+D the next / previous landmark while no text field, editing host, `<select>` or
/// typeahead widget holds the keys (those keep their letters).
pub fn reading_key(engine: &AetherEngine, key: &str) -> Option<bandy::A11yVerb> {
    use bandy::A11yVerb as V;
    let (mods, k) = parse_key(key);
    let Some(Key::Char(c)) = k else { return None };
    if mods.ctrl || mods.meta {
        return None;
    }
    if mods.alt {
        return (c == '1' && !mods.shift).then_some(V::Skip);
    }
    if let Some(n) = engine.focused_node.as_ref() {
        let editing_host = attr(n, "contenteditable").is_some_and(|v| !v.trim().eq_ignore_ascii_case("false"))
            || n.ancestors().any(|a| attr(&a, "contenteditable").is_some_and(|v| !v.trim().eq_ignore_ascii_case("false")));
        let typeahead = tag(n) == "select" || role::explicit_role(n).is_some_and(|r| TYPEAHEAD_ROLES.contains(&r));
        if crate::forms::is_text_editable(n) || editing_host || typeahead {
            return None;
        }
    }
    match c.to_ascii_lowercase() {
        'h' => Some(V::NextHeading { backward: mods.shift, level: None }),
        'd' => Some(V::NextLandmark { backward: mods.shift }),
        _ => None,
    }
}

/// Dispatches the trusted click (with its activation behavior) at `node`; the page restyles when the
/// click mutated the DOM. False when no script engine runs the page.
pub fn click(engine: &mut AetherEngine, node: &NodeRef) -> bool {
    let (w, h, scroll) = (engine.width as f32, engine.height as f32, (engine.scroll_x, engine.scroll_y));
    let Some(js) = engine.js_engine.as_mut() else { return false };
    js.set_viewport(w, h, scroll);
    js.click(node);
    if crate::js::take_mutated() {
        engine.relayout();
    }
    engine.damage_rects.push((0, 0, engine.width, engine.height));
    engine.needs_repaint = true;
    true
}

/// Enter on a link: the trusted click, then — unless a listener canceled it — the hyperlink is followed
/// (HTML §4.6.4 "follow the hyperlink"). The binding's trusted-click activation leaves an ordinary link to
/// the shell's pointer path (js/events.rs `"a" | "area"`), so the keyboard path follows it here through the
/// same navigation a script-dispatched click takes (`js::window::navigate_from_link`: a same-document
/// fragment scrolls, anything else is staged for the shell and picked up by `AetherEngine::tick`). A
/// `javascript:` URL was already queued by the click itself.
fn follow_link(engine: &mut AetherEngine, node: &NodeRef) -> bool {
    if !click(engine, node) {
        return false;
    }
    if crate::js::last_click_canceled() {
        return true;
    }
    let Some(href) = attr(node, "href") else { return true };
    let t = href.trim_start_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control());
    if t.len() >= 11 && t.is_char_boundary(11) && t[..11].eq_ignore_ascii_case("javascript:") {
        return true;
    }
    if let Some(js) = engine.js_engine.as_mut() {
        let _ = crate::js::window::navigate_from_link(&mut js.vm, &href);
        if crate::js::take_mutated() {
            engine.relayout();
        }
    }
    true
}

/// An arrow key on a radio in a group: the next (`forward`) or previous enabled, rendered radio of the
/// group, wrapping (HTML §4.10.5.1.18 as browsers implement it), is focused and checked.
fn radio_arrow(engine: &mut AetherEngine, node: &NodeRef, forward: bool) -> bool {
    let Some(group) = radio_group(node) else { return false };
    let r = Rendering::of(engine);
    let live: Vec<NodeRef> = group.into_iter().filter(|g| focus::is_focusable(g, &r)).collect();
    let Some(at) = live.iter().position(|g| g == node) else { return false };
    if live.len() < 2 {
        return true;
    }
    let to = if forward { (at + 1) % live.len() } else { (at + live.len() - 1) % live.len() };
    let target = live[to].clone();
    focus::focus(engine, Some(target.clone()), true);
    click(engine, &target);
    true
}

/// The listbox a focused node drives: the node itself (role `listbox`, `aria-activedescendant` pattern) or
/// the listbox owning a focused option (roving focus).
fn listbox_of(node: &NodeRef) -> Option<NodeRef> {
    if role::explicit_role(node) == Some("listbox") {
        return Some(node.clone());
    }
    if role::explicit_role(node) == Some("option") {
        return node.ancestors().find(|a| role::explicit_role(a) == Some("listbox"));
    }
    None
}

/// The options of an ARIA listbox (role `option` descendants not inside a nested listbox), minus the
/// `aria-disabled` ones.
fn options(listbox: &NodeRef) -> Vec<NodeRef> {
    listbox
        .descendants()
        .filter(|d| role::explicit_role(d) == Some("option"))
        .filter(|d| d.ancestors().find(|a| role::explicit_role(a) == Some("listbox")).as_ref() == Some(listbox))
        .filter(|d| super::aria_bool(d, "aria-disabled") != Some(true))
        .collect()
}

fn set_attr(n: &NodeRef, name: &str, value: &str) {
    if let Some(e) = n.as_element() {
        e.attributes.borrow_mut().insert(name, value);
    }
}

/// Up / Down / Home / End in an ARIA listbox.
fn listbox_key(engine: &mut AetherEngine, node: &NodeRef, k: Key) -> bool {
    let Some(lb) = listbox_of(node) else { return false };
    let opts = options(&lb);
    if opts.is_empty() {
        return false;
    }
    let active_desc = attr(&lb, "aria-activedescendant").and_then(|id| super::by_id(&lb, &id));
    let current = if node != &lb {
        Some(node.clone())
    } else {
        active_desc.or_else(|| opts.iter().find(|o| super::aria_bool(o, "aria-selected") == Some(true)).cloned())
    };
    let at = current.as_ref().and_then(|c| opts.iter().position(|o| o == c));
    let to = match (k, at) {
        (Key::Home, _) | (Key::Down, None) => 0,
        (Key::End, _) | (Key::Up, None) => opts.len() - 1,
        (Key::Down, Some(i)) => (i + 1).min(opts.len() - 1),
        (Key::Up, Some(i)) => i.saturating_sub(1),
        _ => return false,
    };
    let target = opts[to].clone();
    // Selection follows focus in a single-select listbox (APG Listbox, "recommended selection model").
    if super::aria_bool(&lb, "aria-multiselectable") != Some(true) {
        for o in &opts {
            set_attr(o, "aria-selected", if *o == target { "true" } else { "false" });
        }
    }
    if node == &lb && attr(&target, "id").is_some_and(|i| !i.is_empty()) {
        // The listbox keeps focus; the active option moves.
        set_attr(&lb, "aria-activedescendant", &attr(&target, "id").unwrap_or_default());
        crate::js::touch();
        engine.relayout();
    } else {
        crate::js::touch();
        focus::focus(engine, Some(target), true);
    }
    true
}
