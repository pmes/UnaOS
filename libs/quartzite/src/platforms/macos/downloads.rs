//! AETHERMACCHROME (SR205): the downloads list on AppKit, the GTK page view's download rows' counterpart: a vertical
//! stack above the status line, hidden while empty, one row per `BrowserDownload` id — the text
//! ([`crate::widgets::chrome_rows::download_text`]), an `NSProgressIndicator` bar (indeterminate while the size is
//! unknown: [`crate::widgets::chrome_rows::download_fraction`]) and a dismiss box that removes the row. The list is
//! the shell's (`aether::chrome::downloads::DownloadList`); the face keeps only its row views, on the main thread.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, Message};
use objc2_app_kit::{NSStackView, NSTextField, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use bandy::{SMessage, Synapse};

use super::find_bar::label;
use super::tab_strip::{hug, stack};
use crate::widgets::chrome_rows;

/// One download's row: the row view, its text and its bar.
struct Row {
    view: Retained<NSStackView>,
    text: Retained<NSTextField>,
    bar: Retained<AnyObject>,
}

thread_local! {
    /// The rows on view, by download id (main thread only).
    static ROWS: RefCell<HashMap<u64, Row>> = RefCell::new(HashMap::new());
}

/// A bar-style `NSProgressIndicator` over 0..1.
fn progress_bar() -> Option<Retained<AnyObject>> {
    unsafe {
        let p: *mut AnyObject = msg_send![class!(NSProgressIndicator), alloc];
        let p: *mut AnyObject = msg_send![p, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(160.0, 12.0))];
        let p = Retained::from_raw(p)?;
        // NSProgressIndicatorStyleBar
        let _: () = msg_send![&*p, setStyle: 0usize];
        let _: () = msg_send![&*p, setMinValue: 0.0f64];
        let _: () = msg_send![&*p, setMaxValue: 1.0f64];
        Some(p)
    }
}

/// Take one `BrowserDownload` into `list`. Main thread only.
fn upsert(list: &NSStackView, id: u64, text: &str, fraction: Option<f64>) {
    ROWS.with(|rows| {
        let mut rows = rows.borrow_mut();
        if !rows.contains_key(&id) {
            let Some(bar) = progress_bar() else { return };
            let view = stack(false);
            let t = label("");
            let list_r = list.retain();
            let close = super::button::bootstrap_button_with("×", Box::new(move || dismiss(&list_r, id)));
            unsafe {
                let _: () = msg_send![&close, setBordered: Bool::NO];
                let _: () = msg_send![&view, addArrangedSubview: &*t];
                let _: () = msg_send![&view, addArrangedSubview: &*bar];
                let _: () = msg_send![&view, addArrangedSubview: &*close];
                let _: () = msg_send![list, addArrangedSubview: &*view];
                let _: () = msg_send![list, setHidden: Bool::NO];
            }
            rows.insert(id, Row { view, text: t, bar });
        }
        let Some(row) = rows.get(&id) else { return };
        unsafe {
            let s = NSString::from_str(text);
            let _: () = msg_send![&row.text, setStringValue: &*s];
            match fraction {
                Some(f) => {
                    let _: () = msg_send![&*row.bar, stopAnimation: std::ptr::null::<AnyObject>()];
                    let _: () = msg_send![&*row.bar, setIndeterminate: Bool::NO];
                    let _: () = msg_send![&*row.bar, setDoubleValue: f];
                }
                None => {
                    let _: () = msg_send![&*row.bar, setIndeterminate: Bool::YES];
                    let _: () = msg_send![&*row.bar, startAnimation: std::ptr::null::<AnyObject>()];
                }
            }
        }
    });
}

/// The dismiss box: the row goes; the list hides when it is empty. Main thread only.
fn dismiss(list: &NSStackView, id: u64) {
    ROWS.with(|rows| {
        let mut rows = rows.borrow_mut();
        if let Some(row) = rows.remove(&id) {
            unsafe {
                let _: () = msg_send![list, removeArrangedSubview: &*row.view];
                let _: () = msg_send![&row.view, removeFromSuperview];
            }
        }
        if rows.is_empty() {
            unsafe {
                let _: () = msg_send![list, setHidden: Bool::YES];
            }
        }
    });
}

/// The downloads list, hidden until the first `BrowserDownload`.
pub fn bootstrap_downloads(synapse: Synapse) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_downloads must run on the main thread");
    let list = stack(true);
    unsafe {
        let _: () = msg_send![&list, setHidden: Bool::YES];
    }
    hug(&list, 1);
    let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(list.clone(), mtm));
    let mut rx = synapse.subscribe();
    std::thread::Builder::new()
        .name("downloads".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("downloads subscription runtime");
            rt.block_on(async move {
                loop {
                    match rx.recv().await {
                        Ok(SMessage::BrowserDownload { id, name, received, total, done, error }) => {
                            let text = chrome_rows::download_text(&name, received, total, done, error.as_deref());
                            let fraction = chrome_rows::download_fraction(received, total, done, error.is_some());
                            let bound = bound.clone();
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                let mtm = MainThreadMarker::new().unwrap();
                                upsert(bound.get(mtm), id, &text, fraction);
                            });
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        })
        .expect("spawn downloads thread");
    unsafe { Retained::cast_unchecked::<NSView>(list) }
}
