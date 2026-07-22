use std::cell::RefCell;
use objc2::{define_class, msg_send, ClassType, DeclaredClass};
use objc2::rc::{Retained, Allocated};
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSTextField, NSView};
use objc2_foundation::{NSRect, NSPoint, NSSize, NSString};

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

        Retained::cast_unchecked::<NSView>(field)
    }
}
