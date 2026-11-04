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
    bootstrap_console_held(console, synapse).0
}

/// FACELIVE-SUBSCRIPTION-DROP (SR441): [`bootstrap_console`] with its bus subscriber's abort handle (the loop runs through
/// `tetra_eval::face_subscribe`), which the tetra evaluator keeps and aborts when the Console leaves the tree.
pub fn bootstrap_console_held(console: &ConsoleTetra, synapse: Synapse) -> (Retained<NSView>, tokio::task::AbortHandle) {
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
    let pad = jog_pad(&root, synapse.clone(), mtm); // VUGCOMSCANJOGFACEMACOS (SR622): the jog pad under the scrollback, painted from widgets::jog_rows

    let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(state, mtm));
    let mut rx = synapse.subscribe();
    let abort = super::tetra_eval::face_subscribe("console-view", async move {
        loop {
            match rx.recv().await {
                Ok(msg @ SMessage::Pump(_)) => pad.hear(&msg), // VUGCOMSCANJOGFACEMACOS (SR622): the pad rides this one subscriber, so the evaluator's abort (SR441) ends both
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
    (unsafe { Retained::cast_unchecked::<NSView>(root) }, abort)
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

// --- VUGCOMSCANJOGFACEMACOS (LEDGER SR622): the Console's jog pad on AppKit, painted from widgets::jog_rows (named tail hunk) ---

/// The jog pad's card and widgets, on the main thread only (the GTK `JogPad`'s counterpart): comscan's `PumpCard` (the
/// pad's state, folded from the bus) and the views painting `widgets::jog_rows` over its snapshot.
struct PadState {
    card: RefCell<comscan::PumpCard>,
    link: Retained<NSTextField>,
    travel: Retained<NSTextField>,
    position: Retained<NSTextField>,
    squawk: Retained<NSTextField>,
    why: Retained<NSTextField>,
    steps: RefCell<Vec<(f64, Retained<NSView>)>>,
    keys: RefCell<Vec<Retained<NSView>>>,
    declare: std::cell::OnceCell<Retained<super::text_field::UnaTextField>>, // VUGCOMSCANDECLARECONSOLEFACE (SR633): the declare entry, set once built (its Enter clears it)
}

/// The pad's handle the subscriber loop holds: Send (the loop runs on its own thread); every touch hops to the main
/// thread, as the LogTail arm does.
struct JogPad {
    bound: std::sync::Arc<dispatch2::MainThreadBound<Rc<PadState>>>,
}

/// Build the jog pad, add it under `root`'s scrollback and paint its first rows. Keys fire on `synapse` as the local
/// user; the step picker is the filter row's ✓ convention (no segmented control in the house). Main thread only.
fn jog_pad(root: &objc2_app_kit::NSStackView, synapse: Synapse, mtm: MainThreadMarker) -> JogPad {
    use super::meter::{ns_color, LABEL};
    use crate::widgets::jog_rows::{declare_verb, key_verb, DECLARE_PLACEHOLDER, KEYS, STEPS_MM}; // VUGCOMSCANDECLARECONSOLEFACE (SR633): declare_verb, DECLARE_PLACEHOLDER
    let who = bandy::Origin::LocalUser(std::env::var("USER").unwrap_or_else(|_| "una".into()));
    let st = Rc::new(PadState {
        card: RefCell::new(comscan::PumpCard::new(who)),
        link: label(""),
        travel: label(""),
        position: label(""),
        squawk: label(""),
        why: label(""),
        steps: RefCell::new(Vec::with_capacity(STEPS_MM.len())),
        keys: RefCell::new(Vec::with_capacity(KEYS.len())),
        declare: std::cell::OnceCell::new(), // VUGCOMSCANDECLARECONSOLEFACE (SR633)
    });
    // dim-label's twin: the palette's legend colour.
    let _: () = unsafe { msg_send![&st.why, setTextColor: &*ns_color(LABEL)] };

    let step_row = stack(false);
    let step_name = label("step");
    let _: () = unsafe { msg_send![&step_row, addArrangedSubview: &*step_name] };
    for mm in STEPS_MM {
        let w = Rc::downgrade(&st);
        let b = super::button::bootstrap_button_with(&format!("{mm} mm"), Box::new(move || {
            if let Some(st) = w.upgrade() {
                st.card.borrow_mut().set_step(mm);
                render_pad(&st);
            }
        }));
        let _: () = unsafe { msg_send![&step_row, addArrangedSubview: &*b] };
        st.steps.borrow_mut().push((mm, b));
    }
    hug(&step_row, 1);

    let key_row = stack(false);
    for (k, name) in KEYS {
        let (w, syn) = (Rc::downgrade(&st), synapse.clone());
        let b = super::button::bootstrap_button_with(name, Box::new(move || {
            if let Some(st) = w.upgrade() {
                syn.fire(key_verb(&st.card.borrow(), k));
            }
        }));
        let _: () = unsafe { msg_send![&key_row, addArrangedSubview: &*b] };
        st.keys.borrow_mut().push(b);
    }
    hug(&key_row, 1);

    // VUGCOMSCANDECLARECONSOLEFACE (SR633): the house text field under the squawk row. Enter fires the card's declare when
    // the parser takes the line and clears the field; a refused line stays for mending (the card says why in the squawk
    // row); the pad repaints either way. The closure holds the state weakly, so the field and the state hold no cycle.
    let declare = {
        let (w, syn) = (Rc::downgrade(&st), synapse.clone());
        super::text_field::bootstrap_text_field_with(DECLARE_PLACEHOLDER, Box::new(move |line: String| {
            if let Some(st) = w.upgrade() {
                let sent = declare_verb(&mut st.card.borrow_mut(), &line);
                if let Some(m) = sent {
                    syn.fire(m);
                    if let Some(f) = st.declare.get() {
                        set_field(f, "");
                    }
                }
                render_pad(&st);
            }
        }))
    };
    let _ = st.declare.set(declare.clone());
    hug(&declare, 1);

    for l in [&st.link, &st.travel, &st.position, &st.squawk, &st.why] {
        hug(l, 1);
    }
    let pad = stack(true);
    unsafe {
        // NSLayoutAttributeWidth = 7: every row fills the width, as in the root.
        let _: () = msg_send![&pad, setAlignment: 7isize];
        let _: () = msg_send![&pad, addArrangedSubview: &*st.link];
        let _: () = msg_send![&pad, addArrangedSubview: &*st.travel];
        let _: () = msg_send![&pad, addArrangedSubview: &*st.position];
        let _: () = msg_send![&pad, addArrangedSubview: &*st.squawk];
        let _: () = msg_send![&pad, addArrangedSubview: &*declare]; // VUGCOMSCANDECLARECONSOLEFACE (SR633)
        let _: () = msg_send![&pad, addArrangedSubview: &*step_row];
        let _: () = msg_send![&pad, addArrangedSubview: &*key_row];
        let _: () = msg_send![&pad, addArrangedSubview: &*st.why];
    }
    hug(&pad, 1);
    let _: () = unsafe { msg_send![root, addArrangedSubview: &*pad] };

    render_pad(&st);
    JogPad { bound: std::sync::Arc::new(dispatch2::MainThreadBound::new(st, mtm)) }
}

impl JogPad {
    /// Fold a bus message into the card (pump answers only) and repaint, on the main thread.
    fn hear(&self, msg: &SMessage) {
        if let SMessage::Pump(ev) = msg {
            let (ev, bound) = (ev.clone(), self.bound.clone());
            dispatch2::DispatchQueue::main().exec_async(move || {
                let mtm = MainThreadMarker::new().unwrap();
                let st = bound.get(mtm);
                st.card.borrow_mut().hear(&ev);
                render_pad(st);
            });
        }
    }
}

fn set_field(field: &NSTextField, text: &str) {
    let s = NSString::from_str(text);
    let _: () = unsafe { msg_send![field, setStringValue: &*s] };
}

fn set_hidden(view: &NSView, hidden: bool) {
    let _: () = unsafe { msg_send![view, setHidden: Bool::new(hidden)] };
}

/// Paint `jog_rows` over the card's snapshot: the optional rows shown only when present, the step titles ticked on the
/// card's step, the picker and keys disabled while no link is up. The pump's words are sanitized like every Console
/// line. Main thread only.
fn render_pad(st: &PadState) {
    use crate::widgets::jog_rows::{jog_rows, JogRow};
    let rows = jog_rows(&st.card.borrow().snapshot());
    set_hidden(&st.position, true);
    set_hidden(&st.squawk, true);
    set_hidden(&st.why, true);
    for row in rows {
        match row {
            JogRow::Link(t) => set_field(&st.link, &t),
            JogRow::Travel(t) => set_field(&st.travel, &t),
            JogRow::Position(t) => {
                set_field(&st.position, &t);
                set_hidden(&st.position, false);
            }
            JogRow::Squawk(t) => {
                set_field(&st.squawk, &crate::tetra::sanitize_line(&t));
                set_hidden(&st.squawk, false);
            }
            JogRow::Why(t) => {
                set_field(&st.why, &t);
                set_hidden(&st.why, false);
            }
            JogRow::Steps { chosen, enabled, .. } => {
                for (mm, b) in st.steps.borrow().iter() {
                    let t = if *mm == chosen { format!("✓ {mm} mm") } else { format!("{mm} mm") };
                    let s = NSString::from_str(&t);
                    unsafe {
                        let _: () = msg_send![&**b, setTitle: &*s];
                        let _: () = msg_send![&**b, setEnabled: Bool::new(enabled)];
                    }
                }
            }
            JogRow::Pad { enabled, .. } => {
                for b in st.keys.borrow().iter() {
                    let _: () = unsafe { msg_send![&**b, setEnabled: Bool::new(enabled)] };
                }
            }
        }
    }
}
