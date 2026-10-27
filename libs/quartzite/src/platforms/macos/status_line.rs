//! AETHERMACCHROME (SR205): the status line at the browser window's foot on AppKit: a label driven by the messages
//! the GTK status overlay and toolbar read — the hovered link (`BrowserStatus`) and the active tab's load
//! (`BrowserNavState.loading`), worded by [`crate::widgets::chrome_rows::status_text`].

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{MainThreadMarker, NSString};

use bandy::{SMessage, Synapse};

use super::find_bar::label;
use super::tab_strip::hug;
use crate::widgets::chrome_rows;

/// The status line; its text follows every `BrowserStatus` and `BrowserNavState`.
pub fn bootstrap_status_line(synapse: Synapse) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_status_line must run on the main thread");
    let line = label("");
    hug(&line, 1);
    let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(line.clone(), mtm));
    let mut rx = synapse.subscribe();
    std::thread::Builder::new()
        .name("status-line".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("status line subscription runtime");
            rt.block_on(async move {
                let (mut hover, mut loading, mut shown) = (String::new(), false, String::new());
                loop {
                    match rx.recv().await {
                        Ok(SMessage::BrowserStatus(text)) => hover = text,
                        Ok(SMessage::BrowserNavState { loading: l, .. }) => loading = l,
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                    let text = chrome_rows::status_text(&hover, loading);
                    if text == shown {
                        continue;
                    }
                    shown = text.clone();
                    let bound = bound.clone();
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        let mtm = MainThreadMarker::new().unwrap();
                        let s = NSString::from_str(&text);
                        let _: () = unsafe { objc2::msg_send![&**bound.get(mtm), setStringValue: &*s] };
                    });
                }
            });
        })
        .expect("spawn status-line thread");
    unsafe { Retained::cast_unchecked::<NSView>(line) }
}
