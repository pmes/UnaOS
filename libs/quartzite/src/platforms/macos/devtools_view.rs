//! CHARTER: Aether (CODEX §2 Web) — the developer-tools pane on AppKit, the GTK `devtools_view`'s counterpart.
//!
//! AETHERMACDEVTOOLS (SR312): R104, the face paints and the model decides. The model is the shell's
//! `aether::chrome::DevtoolsChrome`; everything crosses bandy, and every rule and verb is
//! `crate::widgets::devtools_rows`' (the one copy both faces read):
//!
//! - the page surface keeps the WHOLE content area: AppKit's surface reports its own frame as `BrowserResize`
//!   (`image_view::fire_resize`) and the shell splits that area, so the pane never shrinks the surface. It is an
//!   OVERLAY at `BrowserDevtoolsLayout`'s pane rect over the page, which draws the engine's frame 1:1 pinned top-left
//!   (`image_view::image_dest`), and the split's page rect is always `(0, 0, w, h)`. That is the GTK face's sizer +
//!   `Overlay` shape, in a flipped host view ([`UnaDevtoolsHost`]);
//! - the head row: the panel buttons (`PANELS` -> `BrowserDevtoolsPanel`), the dock button (`store_dock`, Principia
//!   `aether.devtools.dock`, then `BrowserDevtoolsDock`), the close button (`toggle_key`, the F12 toggle);
//! - the text: `BrowserDevtoolsLines` in a read-only monospace `NSTextView`. A click on a frame line
//!   (`line_at` -> `goto_target`) fires `BrowserDevtoolsGoto` ([`UnaDevtoolsText`], the view's delegate);
//! - the eval field: `eval_msg` -> `BrowserDevtoolsEval`, cleared on send;
//! - the grip on the pane's leading (right dock) or top (bottom dock) edge: a drag fires
//!   `BrowserDevtoolsShare(share_from_position(..))` ([`UnaDevtoolsGrip`]).
//!
//! The dock side Principia holds is sent once at build. The keys (Cmd+Option+I, Cmd+Option+J, F12) are the Develop
//! menu's (`super::install_develop_menu`), which reads [`is_open`]. Every line here is read-checked only
//! (AETHERMAC-RUN).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{NSResponder, NSTextView, NSView};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString};

use bandy::{SMessage, Synapse};

use super::console_view::{set_text, text_scroller};
use super::tab_strip::{hug, stack};
use super::text_field::UnaTextField;
use crate::widgets::devtools_rows::{self, PANELS};

/// The grip's thickness across the dock axis, in points.
const GRIP: f64 = 6.0;

thread_local! {
    /// The pane as the last layout left it (main thread): open, docked under the page.
    static OPEN: Cell<bool> = const { Cell::new(false) };
    static BOTTOM: Cell<bool> = const { Cell::new(false) };
    /// True while the face itself writes the text (its selection change is not a click).
    static SETTING: Cell<bool> = const { Cell::new(false) };
    static HOST: RefCell<Option<Retained<UnaDevtoolsHost>>> = const { RefCell::new(None) };
    static TEXT: RefCell<Option<Retained<NSTextView>>> = const { RefCell::new(None) };
}

/// Whether the last layout showed the pane (the Develop menu's console key reads it). Main thread only.
pub fn is_open() -> bool {
    OPEN.with(|o| o.get())
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

// ---- UnaDevtoolsHost: the page with the pane over it ---------------------------------------------------------------

pub struct DevtoolsHostIvars {
    page: RefCell<Option<Retained<NSView>>>,
    pane: RefCell<Option<Retained<NSView>>>,
    grip: RefCell<Option<Retained<UnaDevtoolsGrip>>>,
    /// The layout's pane rect `(x, y, w, h)` in the host's flipped points; None: closed.
    pane_rect: Cell<Option<(f64, f64, f64, f64)>>,
    /// The text view's delegate (an `NSTextView` holds its delegate weakly).
    text_delegate: RefCell<Option<Retained<UnaDevtoolsText>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "UnaDevtoolsHost"]
    #[ivars = DevtoolsHostIvars]
    pub struct UnaDevtoolsHost;

    impl UnaDevtoolsHost {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(DevtoolsHostIvars {
                page: RefCell::new(None),
                pane: RefCell::new(None),
                grip: RefCell::new(None),
                pane_rect: Cell::new(None),
                text_delegate: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        /// Flipped: the layout's rects are top-left `(x, y, w, h)`.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> Bool {
            Bool::YES
        }

        /// The host's own layout replaces autoresizing: the page fills it, the pane and grip sit at the layout's rect.
        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews(&self, _old: NSSize) {
            self.lay();
        }
    }
);

impl UnaDevtoolsHost {
    /// Place the page (the whole bounds) and the pane and its grip (the layout's rect, hidden when closed).
    fn lay(&self) {
        let b: NSRect = unsafe { msg_send![self, bounds] };
        let iv = self.ivars();
        if let Some(page) = iv.page.borrow().as_ref() {
            let _: () = unsafe { msg_send![&**page, setFrame: b] };
        }
        let r = iv.pane_rect.get();
        let bottom = BOTTOM.with(|x| x.get());
        if let Some(pane) = iv.pane.borrow().as_ref() {
            unsafe {
                if let Some((x, y, w, h)) = r {
                    let _: () = msg_send![&**pane, setFrame: rect(x, y, w, h)];
                }
                let _: () = msg_send![&**pane, setHidden: Bool::new(r.is_none())];
            }
        }
        if let Some(grip) = iv.grip.borrow().as_ref() {
            grip.ivars().bottom.set(bottom);
            unsafe {
                if let Some((x, y, w, h)) = r {
                    let f = if bottom { rect(x, y - GRIP / 2.0, w, GRIP) } else { rect(x - GRIP / 2.0, y, GRIP, h) };
                    let _: () = msg_send![&**grip, setFrame: f];
                }
                let _: () = msg_send![&**grip, setHidden: Bool::new(r.is_none())];
                let window: *mut AnyObject = msg_send![&**grip, window];
                if !window.is_null() {
                    let _: () = msg_send![window, invalidateCursorRectsForView: &**grip];
                }
            }
        }
    }
}

// ---- UnaDevtoolsGrip: the splitter ----------------------------------------------------------------------------------

pub struct DevtoolsGripIvars {
    synapse: RefCell<Option<Synapse>>,
    /// The dock side the last layout gave (the drag's axis).
    bottom: Cell<bool>,
    /// The last share this drag sent.
    last: Cell<u32>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "UnaDevtoolsGrip"]
    #[ivars = DevtoolsGripIvars]
    pub struct UnaDevtoolsGrip;

    impl UnaDevtoolsGrip {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(DevtoolsGripIvars { synapse: RefCell::new(None), bottom: Cell::new(false), last: Cell::new(0) });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        /// The press is the grip's (the drag that follows comes here).
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &AnyObject) {
            self.ivars().last.set(0);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &AnyObject) {
            self.drag_to(event);
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            unsafe {
                let b: NSRect = msg_send![self, bounds];
                let cursor: *mut AnyObject = if self.ivars().bottom.get() {
                    msg_send![objc2::class!(NSCursor), resizeUpDownCursor]
                } else {
                    msg_send![objc2::class!(NSCursor), resizeLeftRightCursor]
                };
                if !cursor.is_null() {
                    let _: () = msg_send![self, addCursorRect: b, cursor: cursor];
                }
            }
        }
    }
);

impl UnaDevtoolsGrip {
    /// The drag's place along the dock axis of the host as the pane's share; sent when it changes (the model clamps).
    fn drag_to(&self, event: &AnyObject) {
        unsafe {
            let host: *mut NSView = msg_send![self, superview];
            if host.is_null() {
                return;
            }
            let win: NSPoint = msg_send![event, locationInWindow];
            let p: NSPoint = msg_send![host, convertPoint: win, fromView: None::<&NSView>];
            let b: NSRect = msg_send![host, bounds];
            let (pos, total) = if self.ivars().bottom.get() { (p.y, b.size.height) } else { (p.x, b.size.width) };
            let share = devtools_rows::share_from_position(pos as i32, total as i32);
            if total > 0.0 && share != self.ivars().last.get() {
                self.ivars().last.set(share);
                if let Some(s) = self.ivars().synapse.borrow().as_ref() {
                    s.fire(SMessage::BrowserDevtoolsShare(share));
                }
            }
        }
    }
}

// ---- UnaDevtoolsText: a click on a frame line ------------------------------------------------------------------------

pub struct DevtoolsTextIvars {
    synapse: RefCell<Option<Synapse>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaDevtoolsText"]
    #[ivars = DevtoolsTextIvars]
    pub struct UnaDevtoolsText;

    impl UnaDevtoolsText {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(DevtoolsTextIvars { synapse: RefCell::new(None) });
            unsafe { msg_send![super(this), init] }
        }

        /// NSTextViewDelegate: a click leaves an insertion point; the line under it, when it is a frame, opens Sources.
        #[unsafe(method(textViewDidChangeSelection:))]
        fn did_change_selection(&self, notification: &AnyObject) {
            if SETTING.with(|s| s.get()) {
                return;
            }
            unsafe {
                let tv: *mut AnyObject = msg_send![notification, object];
                if tv.is_null() {
                    return;
                }
                let r: NSRange = msg_send![tv, selectedRange];
                if r.length != 0 {
                    return;
                }
                let s: Retained<NSString> = msg_send![tv, string];
                let text = s.to_string();
                if let Some((url, line)) = devtools_rows::goto_target(devtools_rows::line_at(&text, r.location)) {
                    if let Some(syn) = self.ivars().synapse.borrow().as_ref() {
                        syn.fire(SMessage::BrowserDevtoolsGoto { url, line });
                    }
                }
            }
        }
    }
);

unsafe impl NSObjectProtocol for UnaDevtoolsText {}

// ---- assembly -------------------------------------------------------------------------------------------------------

/// The pane: the head row, the text, the eval field.
fn pane(synapse: &Synapse) -> (Retained<NSView>, Retained<NSTextView>) {
    let head = stack(false);
    for label in PANELS {
        let b = super::button::bootstrap_button(label, SMessage::BrowserDevtoolsPanel(label.to_string()), synapse.clone());
        let _: () = unsafe { msg_send![&head, addArrangedSubview: &*b] };
    }
    let dock = {
        let syn = synapse.clone();
        super::button::bootstrap_button_with("⇵", Box::new(move || {
            let want = !BOTTOM.with(|b| b.get());
            devtools_rows::store_dock(want);
            syn.fire(SMessage::BrowserDevtoolsDock { bottom: want });
        }))
    };
    let close = super::button::bootstrap_button("×", devtools_rows::toggle_key(), synapse.clone());
    unsafe {
        let _: () = msg_send![&dock, setToolTip: &*NSString::from_str("Dock to the other side")];
        let _: () = msg_send![&close, setToolTip: &*NSString::from_str("Close (F12)")];
        let _: () = msg_send![&head, addArrangedSubview: &*dock];
        let _: () = msg_send![&head, addArrangedSubview: &*close];
    }
    hug(&head, 1);

    let (scroll, text) = text_scroller();

    let holder: Rc<RefCell<Option<Retained<UnaTextField>>>> = Rc::new(RefCell::new(None));
    let eval = {
        let (syn, holder) = (synapse.clone(), holder.clone());
        super::text_field::bootstrap_text_field_with("Evaluate in the page (Enter)", Box::new(move |line: String| {
            if let Some(m) = devtools_rows::eval_msg(&line) {
                syn.fire(m);
                if let Some(f) = holder.borrow().as_ref() {
                    let empty = NSString::from_str("");
                    let _: () = unsafe { msg_send![&**f, setStringValue: &*empty] };
                }
            }
        }))
    };
    *holder.borrow_mut() = Some(eval.clone());
    hug(&eval, 1);

    let col = stack(true);
    unsafe {
        // NSLayoutAttributeWidth = 7: every row fills the pane's width.
        let _: () = msg_send![&col, setAlignment: 7isize];
        let _: () = msg_send![&col, addArrangedSubview: &*head];
        let _: () = msg_send![&col, addArrangedSubview: &*scroll];
        let _: () = msg_send![&col, addArrangedSubview: &*eval];
        // The pane is opaque over the page: an `NSBox` (NSBoxCustom = 4, no title, no border) filled with the window
        // background, the column its content.
        let bx: Allocated<NSView> = msg_send![objc2::class!(NSBox), alloc];
        let bx: Retained<NSView> = msg_send![bx, initWithFrame: rect(0.0, 0.0, 400.0, 300.0)];
        let _: () = msg_send![&bx, setBoxType: 4usize];
        let _: () = msg_send![&bx, setTitlePosition: 0usize];
        let _: () = msg_send![&bx, setBorderWidth: 0.0f64];
        let _: () = msg_send![&bx, setContentViewMargins: NSSize::new(0.0, 0.0)];
        let bg: Retained<AnyObject> = msg_send![objc2::class!(NSColor), windowBackgroundColor];
        let _: () = msg_send![&bx, setFillColor: &*bg];
        let _: () = msg_send![&bx, setContentView: &*col];
        (bx, text)
    }
}

/// The page with the developer-tools pane over it, wired to the bus (the GTK `bootstrap_devtools`'s counterpart). The
/// returned view takes the page's place in the window; the page keeps reporting the whole area.
pub fn bootstrap_devtools(synapse: Synapse, page: Retained<NSView>) -> Retained<NSView> {
    let _mtm = MainThreadMarker::new().expect("bootstrap_devtools must run on the main thread");
    let (pane_view, text) = pane(&synapse);
    let host: Retained<UnaDevtoolsHost> = unsafe {
        let h: Allocated<UnaDevtoolsHost> = msg_send![UnaDevtoolsHost::class(), alloc];
        msg_send![h, initWithFrame: rect(0.0, 0.0, 800.0, 600.0)]
    };
    let grip: Retained<UnaDevtoolsGrip> = unsafe {
        let g: Allocated<UnaDevtoolsGrip> = msg_send![UnaDevtoolsGrip::class(), alloc];
        msg_send![g, initWithFrame: rect(0.0, 0.0, GRIP, GRIP)]
    };
    *grip.ivars().synapse.borrow_mut() = Some(synapse.clone());
    let delegate: Retained<UnaDevtoolsText> = unsafe {
        let d: Allocated<UnaDevtoolsText> = msg_send![UnaDevtoolsText::class(), alloc];
        msg_send![d, init]
    };
    *delegate.ivars().synapse.borrow_mut() = Some(synapse.clone());
    unsafe {
        let _: () = msg_send![&text, setDelegate: &*delegate];
        let _: () = msg_send![&*page, setTranslatesAutoresizingMaskIntoConstraints: Bool::YES];
        let _: () = msg_send![&*pane_view, setTranslatesAutoresizingMaskIntoConstraints: Bool::YES];
        let _: () = msg_send![&*pane_view, setHidden: Bool::YES];
        let _: () = msg_send![&grip, setHidden: Bool::YES];
        let _: () = msg_send![&host, addSubview: &*page];
        let _: () = msg_send![&host, addSubview: &*pane_view];
        let _: () = msg_send![&host, addSubview: &*grip];
        // The host takes the window's slack, as the bare surface did (`tetra_eval::stretch`).
        for axis in [0isize, 1isize] {
            let _: () = msg_send![&host, setContentHuggingPriority: 1.0f32, forOrientation: axis];
            let _: () = msg_send![&host, setContentCompressionResistancePriority: 1.0f32, forOrientation: axis];
        }
    }
    {
        let iv = host.ivars();
        *iv.page.borrow_mut() = Some(page);
        *iv.pane.borrow_mut() = Some(pane_view);
        *iv.grip.borrow_mut() = Some(grip);
        *iv.text_delegate.borrow_mut() = Some(delegate);
    }
    host.lay();
    HOST.with(|h| *h.borrow_mut() = Some(host.clone()));
    TEXT.with(|t| *t.borrow_mut() = Some(text));

    let bottom = devtools_rows::stored_dock();
    BOTTOM.with(|b| b.set(bottom));
    synapse.fire(SMessage::BrowserDevtoolsDock { bottom });

    let mut rx = synapse.subscribe();
    std::thread::Builder::new()
        .name("devtools-pane".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("devtools pane subscription runtime");
            rt.block_on(async move {
                loop {
                    match rx.recv().await {
                        Ok(m @ SMessage::BrowserDevtoolsLayout { .. }) => {
                            let open = devtools_rows::pane_open(&m).unwrap_or(false);
                            let SMessage::BrowserDevtoolsLayout { bottom, pane, .. } = m else { continue };
                            dispatch2::DispatchQueue::main().exec_async(move || apply_layout(open, bottom, pane));
                        }
                        Ok(SMessage::BrowserDevtoolsLines(lines)) => {
                            let body = lines.join("\n");
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                TEXT.with(|t| {
                                    if let Some(t) = t.borrow().as_ref() {
                                        SETTING.with(|s| s.set(true));
                                        set_text(t, &body, false);
                                        SETTING.with(|s| s.set(false));
                                    }
                                });
                            });
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        })
        .expect("spawn devtools-pane thread");
    unsafe { Retained::cast_unchecked::<NSView>(host) }
}

/// A layout from the shell: the pane shows at its rect (or hides) on its dock side. Main thread only.
fn apply_layout(open: bool, bottom: bool, pane: Option<(u32, u32, u32, u32)>) {
    OPEN.with(|o| o.set(open));
    BOTTOM.with(|b| b.set(bottom));
    let r = pane.map(|(x, y, w, h)| (x as f64, y as f64, w as f64, h as f64));
    HOST.with(|cell| {
        if let Some(host) = cell.borrow().as_ref() {
            host.ivars().pane_rect.set(r);
            host.lay();
        }
    });
}
