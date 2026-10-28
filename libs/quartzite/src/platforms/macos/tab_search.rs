//! CHARTER: Aether (CODEX §2 Web) — the AppKit tab search, the GTK face's Ctrl+Shift+A window's counterpart.
//!
//! STRIP2 (SR219): Cmd+Shift+A (the Tab menu, `super::install_tab_keys`) opens an `NSPopover` over the strip that
//! shows (`tab_strip::search_anchor`): a query field that refills on every keystroke (continuous) with the tabs
//! chrome_rows' `TabWire::search` finds over the strip as last sent (title or URL; every tab for an empty query), each
//! a button (favicon, clipped title, the URL as its tooltip) firing `BrowserTabSelect`; Return picks the first hit
//! (the Go button's key equivalent); Esc or a click outside closes it (a transient popover). Built once; its views and
//! hits live in main-thread thread-locals, so no closure holds the popover (no retain cycle).

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_app_kit::{NSStackView, NSView};
use objc2_foundation::{NSArray, NSRect, NSSize, NSString};

use bandy::{SMessage, Synapse};

use super::tab_strip::{key_equivalent, stack};
use super::text_field::UnaTextField;
use crate::widgets::chrome_rows;

/// The most hits the popover lists, and the title clip.
const MAX_HITS: usize = 12;
const HIT_LABEL_MAX: usize = 48;

struct Search {
    pop: Retained<AnyObject>,
    field: Retained<UnaTextField>,
    list: Retained<NSStackView>,
    synapse: Synapse,
}

thread_local! {
    static SEARCH: RefCell<Option<Search>> = const { RefCell::new(None) };
    /// The listed tabs' ids, in order (Return picks the first).
    static HITS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// List the tabs matching `query`. Main thread only.
fn refill(query: &str) {
    let found = super::tab_strip::wire().search(query);
    let active = super::tab_strip::wire().active;
    let list = SEARCH.with(|s| s.borrow().as_ref().map(|s| s.list.clone()));
    let Some(list) = list else { return };
    unsafe {
        let old: Retained<NSArray<NSView>> = msg_send![&list, arrangedSubviews];
        for v in old.to_vec() {
            let _: () = msg_send![&list, removeArrangedSubview: &*v];
            let _: () = msg_send![&v, removeFromSuperview];
        }
    }
    let mut ids = Vec::new();
    for t in found.iter().take(MAX_HITS) {
        let id = t.id;
        let b = super::button::bootstrap_button_with(&chrome_rows::clip(&t.label, HIT_LABEL_MAX), Box::new(move || pick(id)));
        unsafe {
            if let Some(img) = super::tab_strip::tile_image(id) {
                let _: () = msg_send![&b, setImage: &*img];
                // NSImageLeft
                let _: () = msg_send![&b, setImagePosition: 2isize];
            }
            let _: () = msg_send![&b, setBordered: Bool::new(id == active)];
            let tip = NSString::from_str(if t.url.is_empty() { &t.label } else { &t.url });
            let _: () = msg_send![&b, setToolTip: &*tip];
            let _: () = msg_send![&list, addArrangedSubview: &*b];
        }
        ids.push(id);
    }
    HITS.with(|h| *h.borrow_mut() = ids);
}

/// Select tab `id` and close the popover.
fn pick(id: u64) {
    let s = SEARCH.with(|s| s.borrow().as_ref().map(|s| (s.pop.clone(), s.synapse.clone())));
    if let Some((pop, syn)) = s {
        syn.fire(SMessage::BrowserTabSelect(id));
        unsafe {
            let _: () = msg_send![&*pop, performClose: None::<&AnyObject>];
        }
    }
}

/// Return: the first hit.
fn pick_first() {
    if let Some(id) = HITS.with(|h| h.borrow().first().copied()) {
        pick(id);
    }
}

/// Build the popover once. Main thread only.
fn build(synapse: &Synapse) {
    let field = super::text_field::bootstrap_text_field_with("Search tabs by title or address", Box::new(|q: String| refill(&q)));
    let go = super::button::bootstrap_button_with("Go", Box::new(pick_first));
    key_equivalent(&go, "\r", 0);
    let head = stack(false);
    let list = stack(true);
    let col = stack(true);
    unsafe {
        // Every keystroke sends the field's action (the refill), not only Return.
        let _: () = msg_send![&field, setContinuous: Bool::YES];
        let _: () = msg_send![&head, addArrangedSubview: &*field];
        let _: () = msg_send![&head, addArrangedSubview: &*go];
        let _: () = msg_send![&col, addArrangedSubview: &*head];
        let _: () = msg_send![&col, addArrangedSubview: &*list];
        // NSLayoutAttributeWidth = 7: the rows fill the popover's width.
        let _: () = msg_send![&col, setAlignment: 7isize];
        let _: () = msg_send![&list, setAlignment: 7isize];
        let _: () = msg_send![&col, setEdgeInsets: objc2_foundation::NSEdgeInsets { top: 8.0, left: 8.0, bottom: 8.0, right: 8.0 }];

        let vc: *mut AnyObject = msg_send![class!(NSViewController), alloc];
        let vc: *mut AnyObject = msg_send![vc, init];
        let Some(vc) = Retained::<AnyObject>::from_raw(vc) else { return };
        let _: () = msg_send![&*vc, setView: &*col];
        let pop: *mut AnyObject = msg_send![class!(NSPopover), alloc];
        let pop: *mut AnyObject = msg_send![pop, init];
        let Some(pop) = Retained::<AnyObject>::from_raw(pop) else { return };
        // NSPopoverBehaviorTransient: a click outside or Esc closes it.
        let _: () = msg_send![&*pop, setBehavior: 1isize];
        let _: () = msg_send![&*pop, setContentViewController: &*vc];
        let _: () = msg_send![&*pop, setContentSize: NSSize::new(440.0, 360.0)];
        SEARCH.with(|s| *s.borrow_mut() = Some(Search { pop, field, list, synapse: synapse.clone() }));
    }
}

/// Open the tab search over the strip that shows, with an empty query over the strip as it is now.
pub fn open(synapse: &Synapse) {
    let Some((anchor, side)) = super::tab_strip::search_anchor() else { return };
    if SEARCH.with(|s| s.borrow().is_none()) {
        build(synapse);
    }
    let parts = SEARCH.with(|s| s.borrow().as_ref().map(|s| (s.pop.clone(), s.field.clone())));
    let Some((pop, field)) = parts else { return };
    unsafe {
        let empty = NSString::from_str("");
        let _: () = msg_send![&field, setStringValue: &*empty];
    }
    refill("");
    unsafe {
        let bounds: NSRect = msg_send![&anchor, bounds];
        // NSRectEdge: beside the side column (NSMaxXEdge = 2), under the top strip (NSMinYEdge = 1).
        let edge: usize = if side { 2 } else { 1 };
        let _: () = msg_send![&*pop, showRelativeToRect: bounds, ofView: &*anchor, preferredEdge: edge];
        let window: *mut AnyObject = msg_send![&field, window];
        if !window.is_null() {
            let _: Bool = msg_send![window, makeFirstResponder: &*field];
        }
    }
}
