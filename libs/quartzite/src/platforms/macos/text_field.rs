use std::cell::RefCell;
use objc2::{define_class, msg_send, ClassType, DeclaredClass};
use objc2::rc::{Retained, Allocated};
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSTextField, NSView};
use objc2_foundation::{MainThreadMarker, NSRect, NSPoint, NSSize, NSString};

use bandy::{SMessage, Synapse};
use crate::tetra::TextAction;

pub struct TextFieldIvars {
    action: RefCell<Option<Box<dyn Fn(String)>>>,
}

define_class!(
    #[unsafe(super(NSTextField))]
    #[name = "UnaTextField"]
    #[ivars = TextFieldIvars]
    pub struct UnaTextField;

    impl UnaTextField {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(TextFieldIvars {
                action: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(onAction:))]
        fn on_action(&self, sender: &AnyObject) {
            let text: Retained<NSString> = unsafe { msg_send![sender, stringValue] };
            if let Some(cb) = self.ivars().action.borrow().as_ref() {
                cb(text.to_string());
            }
        }
    }
);

pub fn bootstrap_text_field(
    placeholder: &str,
    text_action: TextAction,
    synapse: Synapse,
) -> Retained<NSView> {
    unsafe {
        let is_url_bar = matches!(text_action, TextAction::OpenDocument);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 24.0));
        let field: Allocated<UnaTextField> = msg_send![UnaTextField::class(), alloc];
        let field: Retained<UnaTextField> = msg_send![field, initWithFrame: frame];
        
        let placeholder_str = NSString::from_str(placeholder);
        let _: () = msg_send![&field, setPlaceholderString: &*placeholder_str];
        
        let _: () = msg_send![&field, setTarget: &*field];
        let _: () = msg_send![&field, setAction: objc2::sel!(onAction:)];

        let syn = synapse.clone();
        let action_closure = move |text: String| {
            let msg = match text_action {
                TextAction::OpenDocument => SMessage::OpenDocument { url: text },
                TextAction::ConsoleInput => SMessage::ConsoleInput(text),
                TextAction::BrowserText => SMessage::BrowserText(text),
            };
            syn.fire(msg);
        };
        *field.ivars().action.borrow_mut() = Some(Box::new(action_closure));

        // The address bar mirrors the engine: every navigation that changes
        // the current document url (link click, form submit, Back, Forward,
        // Reload, redirect) arrives as `BrowserUrlChanged` and is written into
        // the field. Writing `stringValue` does not make the field first
        // responder, so this never steals focus from the page surface; while
        // the user is actually typing in the bar (a live field editor) the
        // update is skipped rather than stomping the in-progress text.
        if is_url_bar {
            let mtm = MainThreadMarker::new()
                .expect("bootstrap_text_field must run on the main thread");
            let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(field.clone(), mtm));
            let mut rx = synapse.subscribe();
            // AppKit's main thread has no tokio reactor; the subscription gets
            // its own thread + current-thread runtime and hops back via GCD.
            std::thread::Builder::new()
                .name("url-bar".into())
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("url bar subscription runtime");
                    rt.block_on(async move {
                        loop {
                            match rx.recv().await {
                                Ok(SMessage::BrowserUrlChanged(url)) => {
                                    let bound = bound.clone();
                                    dispatch2::DispatchQueue::main().exec_async(move || {
                                        let mtm = MainThreadMarker::new().unwrap();
                                        let field = bound.get(mtm);
                                        let editor: *mut AnyObject =
                                            msg_send![&*field, currentEditor];
                                        if !editor.is_null() {
                                            // User is typing — leave it alone.
                                            return;
                                        }
                                        let s = NSString::from_str(&url);
                                        let _: () = msg_send![&*field, setStringValue: &*s];
                                    });
                                }
                                Ok(_) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    continue
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                })
                .expect("spawn url-bar thread");
        }

        Retained::cast_unchecked::<NSView>(field)
    }
}
