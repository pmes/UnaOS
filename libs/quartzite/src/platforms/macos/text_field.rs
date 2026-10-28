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
    secret: bool,
    text_action: TextAction,
    synapse: Synapse,
) -> (Retained<NSView>, Option<tokio::task::AbortHandle>) {
    unsafe {
        let is_url_bar = matches!(text_action, TextAction::OpenDocument);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 24.0));
        let field: Allocated<UnaTextField> = msg_send![UnaTextField::class(), alloc];
        let field: Retained<UnaTextField> = msg_send![field, initWithFrame: frame];
        if secret { secure_cell(&field); } // LANDINGSECRETFIELD (SR432): before the placeholder, target and action land on the cell
        
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
        // FACELIVE-URLBAR (LEDGER SR443): the subscription runs through SR441's `tetra_eval::face_subscribe` (its own
        // named thread + current-thread runtime, hopping back via GCD), so its abort handle goes back to the tetra
        // evaluator, which records it with `face_held` and aborts it when a face Restack or Replace drops the field.
        let abort = is_url_bar.then(|| {
            let mtm = MainThreadMarker::new()
                .expect("bootstrap_text_field must run on the main thread");
            let bound = std::sync::Arc::new(dispatch2::MainThreadBound::new(field.clone(), mtm));
            let mut rx = synapse.subscribe();
            super::tetra_eval::face_subscribe("url-bar", async move {
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
            })
        });

        (Retained::cast_unchecked::<NSView>(field), abort)
    }
}

/// AETHERMACCHROME (SR205): [`bootstrap_text_field`] with Enter's closure given instead of a `TextAction` (the find
/// bar's field fires `BrowserFind`); the field is returned typed so its owner can read `stringValue` and focus it.
pub fn bootstrap_text_field_with(placeholder: &str, action: Box<dyn Fn(String)>) -> Retained<UnaTextField> {
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(220.0, 24.0));
        let field: Allocated<UnaTextField> = msg_send![UnaTextField::class(), alloc];
        let field: Retained<UnaTextField> = msg_send![field, initWithFrame: frame];
        let placeholder_str = NSString::from_str(placeholder);
        let _: () = msg_send![&field, setPlaceholderString: &*placeholder_str];
        let _: () = msg_send![&field, setTarget: &*field];
        let _: () = msg_send![&field, setAction: objc2::sel!(onAction:)];
        *field.ivars().action.borrow_mut() = Some(action);
        field
    }
}

/// LANDINGSECRETFIELD (SR432): a secret field's cell is AppKit's `NSSecureTextFieldCell` (bullets drawn, the field
/// editor secure, copy refused), swapped in before the caller sets the placeholder, target and action on it; the
/// field keeps its `UnaTextField` class and ivars.
unsafe fn secure_cell(field: &UnaTextField) {
    unsafe {
        let cell: Allocated<AnyObject> = msg_send![objc2::class!(NSSecureTextFieldCell), alloc];
        let empty = NSString::from_str("");
        let cell: Retained<AnyObject> = msg_send![cell, initTextCell: &*empty];
        let _: () = msg_send![&cell, setEditable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&cell, setSelectable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&cell, setBezeled: objc2::runtime::Bool::YES];
        let _: () = msg_send![&cell, setScrollable: objc2::runtime::Bool::YES];
        let _: () = msg_send![field, setCell: &*cell];
    }
}
