//! CHARTER: Aether (CODEX §2 Web) — the system-log Console on AppKit, the GTK `console_view`'s counterpart.
//!
//! AETHERMACDEVTOOLS (SR312): a read-only, live, monospace view of `comscan`'s scrollback (`TetraNode::Console`): the
//! header ([`console_rows::render_header`]: the ring's evictions and the pause), the level filter (one button per
//! [`LevelFilter::ALL`], the chosen one ticked), and an `NSScrollView` over a non-editable `NSTextView` holding
//! [`console_rows::render_body`] of the filtered rows. It seeds from the snapshot and follows every
//! `SMessage::Logs(LogEvent::LogTail)` through [`console_rows::rows_from_tail`] (Tabula's sanitizer: a control byte is
//! shown, never obeyed). Read-only by ownership, as on GTK: the log is root-owned on the shard; the page's REPL is the
//! developer-tools pane's eval field (`BrowserDevtoolsEval`), not this view. [`open_console_window`] is the summon
//! (the Develop menu's System Log, Ctrl+`, `super::install_develop_menu`). Every line here is read-checked only
//! (AETHERMAC-RUN).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{msg_send, ClassType};
use objc2_app_kit::{NSFont, NSScrollView, NSTextField, NSTextView, NSView, NSWindow};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use bandy::{LogEvent, SMessage, Synapse};

use super::find_bar::label;
use super::tab_strip::{hug, stack};
use crate::tetra::{ConsoleLine, ConsoleTetra};
use crate::widgets::console_rows::{self, LevelFilter};

/// One console's state, on the main thread only.
struct ConsoleState {
    lines: RefCell<Vec<ConsoleLine>>,
    filter: Cell<LevelFilter>,
    text: Retained<NSTextView>,
    header: Retained<NSTextField>,
    buttons: RefCell<Vec<(LevelFilter, &'static str, Retained<NSView>)>>,
}

thread_local! {
    /// The summoned Console windows, kept alive while open (main thread).
    static WINDOWS: RefCell<Vec<Retained<NSWindow>>> = const { RefCell::new(Vec::new()) };
}

/// Set an `NSTextView`'s whole text and scroll to its end. Main thread only.
pub(crate) fn set_text(view: &NSTextView, text: &str, to_end: bool) {
    let s = NSString::from_str(text);
    unsafe {
        let _: () = msg_send![view, setString: &*s];
        if to_end {
            let end = objc2_foundation::NSRange { location: s.length(), length: 0 };
            let _: () = msg_send![view, scrollRangeToVisible: end];
        }
    }
}

/// A read-only, selectable, monospace `NSTextView` inside a vertically scrolling `NSScrollView` that yields along both
/// axes. Main thread only.
pub(crate) fn text_scroller() -> (Retained<NSView>, Retained<NSTextView>) {
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 200.0));
        let sc: Allocated<NSScrollView> = msg_send![NSScrollView::class(), alloc];
        let sc: Retained<NSScrollView> = msg_send![sc, initWithFrame: frame];
        let _: () = msg_send![&sc, setHasVerticalScroller: Bool::YES];
        let _: () = msg_send![&sc, setHasHorizontalScroller: Bool::NO];
        let _: () = msg_send![&sc, setAutohidesScrollers: Bool::YES];
        let tv: Allocated<NSTextView> = msg_send![NSTextView::class(), alloc];
        let tv: Retained<NSTextView> = msg_send![tv, initWithFrame: frame];
        let _: () = msg_send![&tv, setEditable: Bool::NO];
        let _: () = msg_send![&tv, setSelectable: Bool::YES];
        let _: () = msg_send![&tv, setRichText: Bool::NO];
        if let Some(font) = NSFont::userFixedPitchFontOfSize(12.0) {
            let _: () = msg_send![&tv, setFont: &*font];
        }
        let _: () = msg_send![&tv, setVerticallyResizable: Bool::YES];
        let _: () = msg_send![&tv, setHorizontallyResizable: Bool::NO];
        // NSViewWidthSizable = 2: the text follows the scroller's width.
        let _: () = msg_send![&tv, setAutoresizingMask: 2usize];
        let _: () = msg_send![&tv, setMaxSize: NSSize::new(1.0e7, 1.0e7)];
        let _: () = msg_send![&sc, setDocumentView: &*tv];
        for axis in [0isize, 1isize] {
            let _: () = msg_send![&sc, setContentHuggingPriority: 1.0f32, forOrientation: axis];
            let _: () = msg_send![&sc, setContentCompressionResistancePriority: 1.0f32, forOrientation: axis];
        }
        (Retained::cast_unchecked::<NSView>(sc), tv)
    }
}

/// Redraw the body and the filter buttons from the state. Main thread only.
fn show(state: &ConsoleState) {
    let f = state.filter.get();
    let body = console_rows::render_body(&console_rows::filtered(&state.lines.borrow(), f));
    set_text(&state.text, &body, true);
    for (lf, name, b) in state.buttons.borrow().iter() {
        let t = if *lf == f { format!("✓ {name}") } else { name.to_string() };
        let s = NSString::from_str(&t);
        let _: () = unsafe { msg_send![&**b, setTitle: &*s] };
    }
}

fn set_header(state: &ConsoleState, dropped: u64, paused: bool) {
    let s = NSString::from_str(&console_rows::render_header(dropped, paused));
    let _: () = unsafe { msg_send![&state.header, setStringValue: &*s] };
}

/// The read-only, live Console view from a `ConsoleTetra` snapshot (the GTK `bootstrap_console`'s counterpart).
pub fn bootstrap_console(console: &ConsoleTetra, synapse: Synapse) -> Retained<NSView> {
    let mtm = MainThreadMarker::new().expect("bootstrap_console must run on the main thread");
    let header = label("");
    hug(&header, 1);
    let (scroll, text) = text_scroller();
    let state = Rc::new(ConsoleState {
        lines: RefCell::new(console.lines.clone()),
        filter: Cell::new(LevelFilter::All),
        text,
        header,
        buttons: RefCell::new(Vec::new()),
    });
    let filters = stack(false);
    for (lf, name) in LevelFilter::ALL {
        let st = state.clone();
        let b = super::button::bootstrap_button_with(name, Box::new(move || {
            st.filter.set(lf);
            show(&st);
        }));
        let _: () = unsafe { msg_send![&filters, addArrangedSubview: &*b] };
        state.buttons.borrow_mut().push((lf, name, b));
    }
    hug(&filters, 1);
    set_header(&state, console.dropped, console.paused);
    show(&state);

    let root = stack(true);
    unsafe {
        // NSLayoutAttributeWidth = 7: every row fills the width.
        let _: () = msg_send![&root, setAlignment: 7isize];
        let _: () = msg_send![&root, addArrangedSubview: &*state.header];
        let _: () = msg_send![&root, addArrangedSubview: &*filters];
        let _: () = msg_send![&root, addArrangedSubview: &*scroll];
    }

    let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(state, mtm));
    let mut rx = synapse.subscribe();
    std::thread::Builder::new()
        .name("console-view".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("console subscription runtime");
            rt.block_on(async move {
                loop {
                    match rx.recv().await {
                        Ok(SMessage::Logs(LogEvent::LogTail { lines, dropped, paused })) => {
                            let rows = console_rows::rows_from_tail(&lines);
                            let bound = bound.clone();
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                let mtm = MainThreadMarker::new().unwrap();
                                let st = bound.get(mtm);
                                *st.lines.borrow_mut() = rows;
                                set_header(st, dropped, paused);
                                show(st);
                            });
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        })
        .expect("spawn console-view thread");
    unsafe { Retained::cast_unchecked::<NSView>(root) }
}

/// Summon the Console as its own window over the process bus (the GTK `open_console_window`'s counterpart); it seeds
/// empty and fills on the next `LogTail`.
pub fn open_console_window(synapse: Synapse) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let view = bootstrap_console(&ConsoleTetra::default(), synapse);
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 600.0));
        let w: Allocated<NSWindow> = msg_send![NSWindow::class(), alloc];
        // NSWindowStyleMask Titled | Closable | Miniaturizable | Resizable = 15; NSBackingStoreBuffered = 2.
        let w: Retained<NSWindow> = msg_send![w, initWithContentRect: frame, styleMask: 15usize, backing: 2usize, defer: Bool::NO];
        let _: () = msg_send![&w, setReleasedWhenClosed: Bool::NO];
        let t = NSString::from_str("Console — UnaOS system log");
        let _: () = msg_send![&w, setTitle: &*t];
        let _: () = msg_send![&w, setContentView: &*view];
        let _: () = msg_send![&w, center];
        let _: () = msg_send![&w, makeKeyAndOrderFront: None::<&AnyObject>];
        WINDOWS.with(|ws| ws.borrow_mut().push(w));
    }
}
