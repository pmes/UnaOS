//! AETHERMACCHROME (SR205): the find bar on AppKit, the GTK page view's find overlay's counterpart: a hidden row over
//! the page — the query field (Enter: the next match), the count (`BrowserFindResult` through
//! [`crate::widgets::chrome_rows::find_count`]), the up / down arrows and the close box (Esc: hide, clear the count,
//! `BrowserFindClose`). The verbs are the GTK bar's: `BrowserFind { query, forward }` (`chrome_rows::find_msg`), which
//! the shell routes to the active tab's engine (`Coordinator::on_bus`). Cmd+F is the key equivalent of
//! [`find_toggle`], a button the side column carries (no event monitor).

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{msg_send, ClassType};
use objc2_app_kit::{NSStackView, NSTextField, NSView};
use objc2_foundation::{MainThreadMarker, NSString};

use bandy::{SMessage, Synapse};

use super::tab_strip::{hug, key_equivalent, stack, CMD};
use super::text_field::UnaTextField;
use crate::widgets::chrome_rows;

/// The bar's views: the row itself (hidden until [`show`]), the query field and the count.
#[derive(Clone)]
pub struct FindBar {
    pub root: Retained<NSStackView>,
    pub field: Retained<UnaTextField>,
    pub count: Retained<NSTextField>,
}

/// A non-editable, borderless label (`+[NSTextField labelWithString:]`).
pub(crate) fn label(text: &str) -> Retained<NSTextField> {
    unsafe {
        let s = NSString::from_str(text);
        msg_send![NSTextField::class(), labelWithString: &*s]
    }
}

/// The field's text now. Main thread only.
fn query(field: &UnaTextField) -> String {
    let s: Retained<NSString> = unsafe { msg_send![field, stringValue] };
    s.to_string()
}

/// Unhide the bar and put the caret in its field.
pub fn show(bar: &FindBar) {
    unsafe {
        let _: () = msg_send![&bar.root, setHidden: Bool::NO];
        let window: *mut AnyObject = msg_send![&bar.field, window];
        if !window.is_null() {
            let _: Bool = msg_send![window, makeFirstResponder: &*bar.field];
        }
    }
}

/// Hide the bar, clear the count and tell the engine (the highlight goes).
fn hide(bar: &FindBar, synapse: &Synapse) {
    unsafe {
        let _: () = msg_send![&bar.root, setHidden: Bool::YES];
        let empty = NSString::from_str("");
        let _: () = msg_send![&bar.count, setStringValue: &*empty];
    }
    synapse.fire(SMessage::BrowserFindClose);
}

/// The find bar, hidden; its count follows every `BrowserFindResult`.
pub fn bootstrap_find_bar(synapse: Synapse) -> FindBar {
    let mtm = MainThreadMarker::new().expect("bootstrap_find_bar must run on the main thread");
    let syn = synapse.clone();
    let field = super::text_field::bootstrap_text_field_with("Find in page", Box::new(move |q: String| syn.fire(chrome_rows::find_msg(&q, true))));
    let count = label("");
    let root = stack(false);
    let bar = FindBar { root: root.clone(), field: field.clone(), count: count.clone() };

    let arrow = |glyph: &str, forward: bool| {
        let (syn, f) = (synapse.clone(), field.clone());
        super::button::bootstrap_button_with(glyph, Box::new(move || syn.fire(chrome_rows::find_msg(&query(&f), forward))))
    };
    let prev = arrow("↑", false);
    let next = arrow("↓", true);
    let close = {
        let (b, syn) = (bar.clone(), synapse.clone());
        super::button::bootstrap_button_with("×", Box::new(move || hide(&b, &syn)))
    };
    // Esc closes it; a hidden button takes no key equivalent, so Esc is the bar's only while it shows.
    key_equivalent(&close, "\u{1b}", 0);
    unsafe {
        let _: () = msg_send![&root, addArrangedSubview: &*field];
        let _: () = msg_send![&root, addArrangedSubview: &*count];
        let _: () = msg_send![&root, addArrangedSubview: &*prev];
        let _: () = msg_send![&root, addArrangedSubview: &*next];
        let _: () = msg_send![&root, addArrangedSubview: &*close];
        let _: () = msg_send![&root, setHidden: Bool::YES];
    }
    hug(&root, 1);

    let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(count, mtm));
    let mut rx = synapse.subscribe();
    std::thread::Builder::new()
        .name("find-bar".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("find bar subscription runtime");
            rt.block_on(async move {
                loop {
                    match rx.recv().await {
                        Ok(SMessage::BrowserFindResult { matches, current, .. }) => {
                            let text = chrome_rows::find_count(matches, current);
                            let bound = bound.clone();
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                let mtm = MainThreadMarker::new().unwrap();
                                let s = NSString::from_str(&text);
                                let _: () = unsafe { msg_send![&**bound.get(mtm), setStringValue: &*s] };
                            });
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        })
        .expect("spawn find-bar thread");
    bar
}

/// The side column's Cmd+F button: shows `bar` and focuses its field.
pub fn find_toggle(bar: &FindBar) -> Retained<NSView> {
    let b = bar.clone();
    let button = super::button::bootstrap_button_with("⌕ Find", Box::new(move || show(&b)));
    key_equivalent(&button, "f", CMD);
    button
}
