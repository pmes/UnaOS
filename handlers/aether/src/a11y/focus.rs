//! Sequential focus navigation (HTML §6.6.3) and the focus move itself (§6.6.4 "focusing steps").
//!
//! Focusable areas (§6.6.2): `a`/`area` with `href`, `button`, `input` (not hidden), `select`, `textarea`,
//! a details' first `summary`, `iframe`, `audio`/`video` with `controls`, editing hosts, and any element
//! whose `tabindex` parses (the rules for parsing integers, §2.3.4.1) — unless disabled (§4.10.18.5),
//! inert (§6.3) or not being rendered (no box, or `visibility: hidden`). The sequential order (§6.6.3
//! "sequential navigation search algorithm" as browsers run it): positive `tabindex` ascending (tree order
//! among equals), then `tabindex=0` and the default-focusable in tree order; a negative `tabindex` is
//! focusable but never a stop. A radio button group is ONE stop (§4.10.5.1.18): its checked radio, else
//! its first (forward) or last (backward).
//!
//! The move ([`focus`]) runs on both sides of the engine: `AetherEngine::focused_node` (what editing,
//! activation and the caret read) and the script's `document.activeElement` (what `:focus` /
//! `:focus-visible` match, `dom::select::focused_in`) with the focus update steps' events — `blur` /
//! `focusout` at the old element, `focus` / `focusin` at the new, each naming the other as
//! `relatedTarget` (§6.6.4). A keyboard move records keyboard modality (`css::note_focus_modality`) and
//! arms the painter's focus ring (`render::focusring`); the element is scrolled into view and the page
//! restyled (`:focus` rules).

use std::cell::RefCell;
use std::collections::HashMap;

use super::tree::Rendering;
use super::{attr, has_attr, tag};
use crate::dom::NodeRef;
use crate::AetherEngine;

/// HTML §2.3.4.1 "rules for parsing integers": optional ASCII whitespace, an optional sign, then digits
/// (trailing garbage ignored); None when no digit follows.
pub fn parse_integer(s: &str) -> Option<i32> {
    let s = s.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let v: i64 = digits.parse::<i64>().unwrap_or(i64::MAX).min(i32::MAX as i64 + 1);
    let v = if neg { -v } else { v };
    Some(v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

/// The element's `tabindex` value when the attribute parses.
pub fn tabindex(n: &NodeRef) -> Option<i32> {
    attr(n, "tabindex").and_then(|v| parse_integer(&v))
}

/// A details element's summary (§4.11.2: its first `summary` child).
fn is_details_summary(n: &NodeRef) -> bool {
    n.parent().is_some_and(|p| tag(&p) == "details" && p.children().find(|c| tag(c) == "summary").as_ref() == Some(n))
}

/// An editing host (`contenteditable` other than "false").
fn editing_host(n: &NodeRef) -> bool {
    attr(n, "contenteditable").is_some_and(|v| !v.trim().eq_ignore_ascii_case("false"))
}

/// Focusable by default, without a `tabindex` (HTML §6.6.2's table, as Chromium applies it).
pub fn focusable_by_default(n: &NodeRef) -> bool {
    match tag(n).as_str() {
        "a" | "area" => has_attr(n, "href"),
        "button" | "select" | "textarea" | "iframe" => true,
        "input" => crate::forms::input_type(n) != "hidden",
        "summary" => is_details_summary(n),
        "audio" | "video" => has_attr(n, "controls"),
        _ => editing_host(n),
    }
}

/// Disabled for focus: a disabled form control (`disabled`, or inside a disabled fieldset outside its
/// first legend).
fn disabled(n: &NodeRef) -> bool {
    matches!(tag(n).as_str(), "button" | "input" | "select" | "textarea" | "fieldset" | "option" | "optgroup") && crate::forms::is_disabled(n)
}

/// A focusable area under rendering `r`.
pub fn is_focusable(n: &NodeRef, r: &Rendering) -> bool {
    if !n.is_element() {
        return false;
    }
    if !(tabindex(n).is_some() || focusable_by_default(n)) {
        return false;
    }
    if disabled(n) || has_attr(n, "inert") || n.ancestors().any(|a| has_attr(&a, "inert")) {
        return false;
    }
    !r.not_rendered(n) && !r.visibility_hidden(n)
}

/// A sequential stop candidate: focusable with no negative `tabindex`.
pub fn in_sequence(n: &NodeRef, r: &Rendering) -> bool {
    is_focusable(n, r) && tabindex(n).is_none_or(|t| t >= 0)
}

/// The sequential focus order of `doc` (every candidate; radio groups are resolved by [`next`]).
pub fn order(doc: &NodeRef, r: &Rendering) -> Vec<NodeRef> {
    let all: Vec<NodeRef> = doc.descendants().filter(|n| in_sequence(n, r)).collect();
    let mut positive: Vec<(i32, usize, NodeRef)> = Vec::new();
    let mut zero = Vec::new();
    for (i, n) in all.into_iter().enumerate() {
        match tabindex(&n) {
            Some(t) if t > 0 => positive.push((t, i, n)),
            _ => zero.push(n),
        }
    }
    positive.sort_by_key(|(t, i, _)| (*t, *i));
    positive.into_iter().map(|(_, _, n)| n).chain(zero).collect()
}

/// The radio button group of `n` (§4.10.5.1.18): radios with the same non-empty `name` and the same
/// form owner in the same tree. None for anything else.
pub fn radio_group(n: &NodeRef) -> Option<Vec<NodeRef>> {
    if tag(n) != "input" || crate::forms::input_type(n) != "radio" {
        return None;
    }
    let name = attr(n, "name").filter(|v| !v.is_empty())?;
    let owner = crate::forms::form_owner(n);
    Some(
        n.document_node()
            .descendants()
            .filter(|o| {
                tag(o) == "input"
                    && crate::forms::input_type(o) == "radio"
                    && attr(o, "name").as_deref() == Some(name.as_str())
                    && crate::forms::form_owner(o) == owner
            })
            .collect(),
    )
}

/// Whether `n` is its radio group's stop when moving `backward` or forward.
fn radio_stop(n: &NodeRef, backward: bool, r: &Rendering) -> bool {
    let Some(group) = radio_group(n) else { return true };
    let live: Vec<&NodeRef> = group.iter().filter(|g| in_sequence(g, r)).collect();
    if let Some(c) = live.iter().find(|g| crate::forms::checkedness(g)) {
        return *c == n;
    }
    let edge = if backward { live.last() } else { live.first() };
    edge.is_some_and(|e| *e == n)
}

/// The next sequential stop after `current` (None: from the document's start / end), wrapping around
/// the document (a browser moves on into its own chrome; the engine has none to move into).
pub fn next(doc: &NodeRef, r: &Rendering, current: Option<&NodeRef>, backward: bool) -> Option<NodeRef> {
    let seq = order(doc, r);
    if seq.is_empty() {
        return None;
    }
    let len = seq.len();
    let same_group = |a: &NodeRef| match (current.and_then(radio_group), radio_group(a)) {
        (Some(g), Some(_)) => g.contains(a),
        _ => false,
    };
    // Where to start: the current element's place in the order, else (an element that is not a stop,
    // or none) the place its tree position falls at.
    let start: isize = match current {
        Some(c) => match seq.iter().position(|n| n == c) {
            Some(p) => p as isize,
            None => {
                let pos: HashMap<html_core::NodeId, usize> = doc.descendants().enumerate().map(|(i, n)| (n.id(), i)).collect();
                let here = pos.get(&c.id()).copied().unwrap_or(0);
                let first_zero = seq.iter().position(|n| tabindex(n).is_none_or(|t| t <= 0)).unwrap_or(len);
                if backward {
                    // the last zero-group stop before it, as if it sat between
                    (first_zero..len).rev().find(|&i| pos.get(&seq[i].id()).copied().unwrap_or(0) < here).map(|i| i as isize + 1).unwrap_or(first_zero as isize)
                } else {
                    (first_zero..len).find(|&i| pos.get(&seq[i].id()).copied().unwrap_or(0) > here).map(|i| i as isize - 1).unwrap_or(len as isize - 1)
                }
            }
        },
        None => {
            if backward {
                len as isize
            } else {
                -1
            }
        }
    };
    for step in 1..=len as isize {
        let i = if backward { start - step } else { start + step };
        let i = i.rem_euclid(len as isize) as usize;
        let cand = &seq[i];
        if Some(cand) == current {
            continue;
        }
        if same_group(cand) {
            continue;
        }
        if radio_stop(cand, backward, r) {
            return Some(cand.clone());
        }
    }
    None
}

thread_local! {
    /// HTML §6.6.3 "sequential focus navigation starting point": where the shell's heading / landmark /
    /// skip verbs left the reading position when the element there takes no focus.
    static START: RefCell<Option<NodeRef>> = const { RefCell::new(None) };
}

/// Sets the sequential focus navigation starting point.
pub fn set_starting_point(n: Option<NodeRef>) {
    START.with(|s| *s.borrow_mut() = n);
}

/// The starting point, when it is a node of the engine's current document.
pub fn starting_point(engine: &AetherEngine) -> Option<NodeRef> {
    let doc = engine.document.as_ref()?;
    START.with(|s| s.borrow().clone()).filter(|n| n.same_document(doc))
}

/// Where sequential navigation and the reading verbs start: the focused element, else the starting point.
pub fn position(engine: &AetherEngine) -> Option<NodeRef> {
    engine.focused_node.clone().or_else(|| starting_point(engine))
}

/// Tab (`backward`: Shift+Tab): focus moves to the next stop. Returns the element focused.
pub fn tab(engine: &mut AetherEngine, backward: bool) -> Option<NodeRef> {
    let doc = engine.document.as_ref()?.document_node();
    let r = Rendering::of(engine);
    let cur = position(engine);
    let to = next(&doc, &r, cur.as_ref(), backward)?;
    focus(engine, Some(to.clone()), true);
    Some(to)
}

/// Moves focus to `node` (None blurs) — the engine's focused element and the script's
/// `document.activeElement` with the focus events — then restyles and scrolls it into view. `keyboard`:
/// the move came from the keyboard (`:focus-visible` matches and the focus ring paints).
pub fn focus(engine: &mut AetherEngine, node: Option<NodeRef>, keyboard: bool) {
    crate::css::note_focus_modality(!keyboard);
    if engine.focused_node != node {
        if node.is_some() {
            // A newly focused text control starts with a collapsed caret at 0 (as `AetherEngine::set_focus`).
            engine.edit = crate::forms::edit::EditState::at(0);
        }
        engine.focused_node = node.clone();
    }
    if node.is_some() {
        set_starting_point(node.clone());
    }
    let new_id = node.as_ref().map(|n| n.id().0);
    let (w, h, scroll) = (engine.width as f32, engine.height as f32, (engine.scroll_x, engine.scroll_y));
    if let Some(js) = engine.js_engine.as_mut() {
        let old = crate::js::page(|p| p.focused);
        if old != new_id {
            js.set_viewport(w, h, scroll);
            if let Some(o) = old {
                crate::js::page(|p| p.focused = None);
                crate::js::events::fire_focus(&mut js.vm, o, "blur", new_id);
                crate::js::events::fire_focus(&mut js.vm, o, "focusout", new_id);
            }
            crate::js::page(|p| p.focused = new_id);
            if let Some(n) = new_id {
                crate::js::events::fire_focus(&mut js.vm, n, "focus", old);
                crate::js::events::fire_focus(&mut js.vm, n, "focusin", old);
            }
            let _ = crate::js::take_mutated();
        }
    }
    crate::render::focusring::set_target(node.clone().filter(|_| keyboard));
    // `:focus` / `:focus-visible` rules (and whatever the focus listeners changed) restyle the page.
    engine.relayout();
    if let Some(n) = &node {
        scroll_into_view(engine, n);
    }
}

/// Scrolls the viewport the least distance that shows `n`'s border box (CSSOM View "scroll an element
/// into view", block `nearest`).
pub fn scroll_into_view(engine: &mut AetherEngine, n: &NodeRef) {
    let Some((_, y, _, h)) = engine.box_of(n) else { return };
    let (y, h, vh) = (y as f64, h as f64, engine.height as f64);
    let top = engine.scroll_y;
    let to = if y < top {
        y
    } else if y + h > top + vh {
        (y + h - vh).min(y)
    } else {
        top
    };
    let to = to.max(0.0);
    if (to - top).abs() > 0.5 {
        engine.scroll_y = to;
        engine.damage_rects.push((0, 0, engine.width, engine.height));
        engine.needs_repaint = true;
    }
}
