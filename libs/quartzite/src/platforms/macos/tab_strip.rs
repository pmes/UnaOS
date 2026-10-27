//! AETHERMACCHROME (SR205): the browser's side tab column on AppKit. The strip's MODEL is the shell's one
//! `aether::chrome::tabs::TabStrip` (the `Coordinator`); this face paints what `SMessage::BrowserTabs` says through
//! [`crate::widgets::chrome_rows::side_rows`] (the model's `TabStrip::rows` over the wire) and fires the row's verb
//! ([`crate::widgets::chrome_rows::row_verb`]: `BrowserTabSelect` / `BrowserTabClose` / `BrowserTabGroupToggle` /
//! `BrowserTabNew`) through the existing [`super::button::bootstrap_button`]. A vertical `NSStackView`
//! `chrome_rows::SIDE_W` points wide: the find toggle on top, then one row per tab (the active one bordered), a header
//! per group, and `+` (Cmd+T) last. Every `BrowserTabs` rebuilds the rows (the url bar's subscription idiom: its own
//! thread, a current-thread runtime, the GCD hop to the main queue).

use objc2::rc::{Allocated, Retained};
use objc2::runtime::Bool;
use objc2::{msg_send, ClassType};
use objc2_app_kit::{NSStackView, NSView};
use objc2_foundation::{MainThreadMarker, NSArray, NSPoint, NSRect, NSSize, NSString};

use bandy::{SMessage, Synapse};

use crate::widgets::chrome_rows::{self, SideRow};

/// `NSEventModifierFlagCommand`.
pub(crate) const CMD: usize = 1 << 20;

/// A fresh `NSStackView`: `vertical` (else horizontal), Fill along the axis, children leading-aligned.
pub(crate) fn stack(vertical: bool) -> Retained<NSStackView> {
    unsafe {
        let s: Allocated<NSStackView> = msg_send![NSStackView::class(), alloc];
        let s: Retained<NSStackView> = msg_send![s, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))];
        let _: () = msg_send![&s, setOrientation: if vertical { 1isize } else { 0isize }];
        // NSStackViewDistributionFill
        let _: () = msg_send![&s, setDistribution: 0isize];
        if vertical {
            // NSLayoutAttributeLeading = 5
            let _: () = msg_send![&s, setAlignment: 5isize];
        }
        let _: () = msg_send![&s, setSpacing: 2.0f64];
        s
    }
}

/// `view` holds its size along `axis` (0 horizontal, 1 vertical): hugging and compression resistance high, so the
/// page beside it takes the slack.
pub(crate) fn hug(view: &NSView, axis: isize) {
    unsafe {
        let _: () = msg_send![view, setContentHuggingPriority: 750.0f32, forOrientation: axis];
        let _: () = msg_send![view, setContentCompressionResistancePriority: 750.0f32, forOrientation: axis];
    }
}

/// Give `button` a key equivalent (`key` with `mask`, e.g. "t" + [`CMD`]).
pub(crate) fn key_equivalent(button: &NSView, key: &str, mask: usize) {
    unsafe {
        let k = NSString::from_str(key);
        let _: () = msg_send![button, setKeyEquivalent: &*k];
        let _: () = msg_send![button, setKeyEquivalentModifierMask: mask];
    }
}

/// One side row's view.
fn row_view(row: &SideRow, synapse: &Synapse) -> Retained<NSView> {
    let button = |label: &str, msg: SMessage| super::button::bootstrap_button(label, msg, synapse.clone());
    match row {
        SideRow::Group { name, count, collapsed, .. } => {
            button(&chrome_rows::group_label(name, *count, *collapsed), chrome_rows::row_verb(row, false))
        }
        SideRow::Tab { label, active, grouped, .. } => {
            let indent = if *grouped { "  " } else { "" };
            let select = button(&format!("{indent}{}", chrome_rows::clip(label, chrome_rows::SIDE_LABEL_MAX)), chrome_rows::row_verb(row, false));
            unsafe {
                let _: () = msg_send![&select, setBordered: Bool::new(*active)];
                // NSLayoutConstraintOrientationHorizontal: the title takes the row's width, the close box holds.
                let _: () = msg_send![&select, setContentHuggingPriority: 1.0f32, forOrientation: 0isize];
            }
            let close = button("×", chrome_rows::row_verb(row, true));
            unsafe {
                let _: () = msg_send![&close, setBordered: Bool::NO];
            }
            hug(&close, 0);
            let line = stack(false);
            unsafe {
                let _: () = msg_send![&line, addArrangedSubview: &*select];
                let _: () = msg_send![&line, addArrangedSubview: &*close];
                Retained::cast_unchecked::<NSView>(line)
            }
        }
        SideRow::New => {
            let plus = button("+", chrome_rows::row_verb(row, false));
            key_equivalent(&plus, "t", CMD);
            plus
        }
    }
}

/// Replace `list`'s rows with `rows`. Main thread only.
fn fill(list: &NSStackView, rows: &[SideRow], synapse: &Synapse) {
    unsafe {
        let old: Retained<NSArray<NSView>> = msg_send![list, arrangedSubviews];
        for v in old.to_vec() {
            let _: () = msg_send![list, removeArrangedSubview: &*v];
            let _: () = msg_send![&v, removeFromSuperview];
        }
        for row in rows {
            let v = row_view(row, synapse);
            let _: () = msg_send![list, addArrangedSubview: &*v];
        }
    }
}

/// The side column: `find_toggle` (the find bar's Cmd+F button) on top, then the rows from every `BrowserTabs`.
pub fn bootstrap_side_tabs(synapse: Synapse, find_toggle: Retained<NSView>) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_side_tabs must run on the main thread");
    let column = stack(true);
    let list = stack(true);
    fill(&list, &chrome_rows::side_rows(0, &[], &[]), &synapse);
    unsafe {
        let _: () = msg_send![&column, addArrangedSubview: &*find_toggle];
        let _: () = msg_send![&column, addArrangedSubview: &*list];
        // A spacer under the rows takes the column's slack (the lowest hugging child under Fill), so the rows sit
        // at the top.
        let spacer: Allocated<NSView> = msg_send![NSView::class(), alloc];
        let spacer: Retained<NSView> = msg_send![spacer, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))];
        let _: () = msg_send![&spacer, setContentHuggingPriority: 1.0f32, forOrientation: 1isize];
        let _: () = msg_send![&column, addArrangedSubview: &*spacer];
        // The column's width: SIDE_W points, whatever the titles.
        let width: Retained<objc2_app_kit::NSLayoutDimension> = msg_send![&column, widthAnchor];
        let c: Retained<objc2_app_kit::NSLayoutConstraint> = msg_send![&width, constraintEqualToConstant: chrome_rows::SIDE_W as f64];
        let _: () = msg_send![&c, setActive: Bool::YES];
    }
    hug(&column, 0);

    let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(list.clone(), mtm));
    let mut rx = synapse.subscribe();
    std::thread::Builder::new()
        .name("side-tabs".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("side tabs subscription runtime");
            rt.block_on(async move {
                loop {
                    match rx.recv().await {
                        Ok(SMessage::BrowserTabs { active, tabs, groups }) => {
                            let rows = chrome_rows::side_rows(active, &tabs, &groups);
                            let (bound, syn) = (bound.clone(), synapse.clone());
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                let mtm = MainThreadMarker::new().unwrap();
                                fill(bound.get(mtm), &rows, &syn);
                            });
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        })
        .expect("spawn side-tabs thread");

    unsafe { Retained::cast_unchecked::<NSView>(column) }
}
