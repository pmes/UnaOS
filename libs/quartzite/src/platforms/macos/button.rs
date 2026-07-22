use std::cell::RefCell;
use objc2::{define_class, msg_send, ClassType, Allocated, DeclaredClass};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSButton, NSView, NSBezelStyle};
use objc2_foundation::{NSRect, NSPoint, NSSize, NSString};

use bandy::{SMessage, Synapse};

pub struct ButtonIvars {
    action: RefCell<Option<Box<dyn Fn()>>>,
}

define_class!(
    #[unsafe(super(NSButton))]
    #[name = "UnaButton"]
    #[ivars = ButtonIvars]
    pub struct UnaButton;

    impl UnaButton {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(ButtonIvars {
                action: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(onAction:))]
        fn on_action(&self, _sender: &AnyObject) {
            if let Some(cb) = self.ivars().action.borrow().as_ref() {
                cb();
            }
        }
    }
);

pub fn bootstrap_button(
    label: &str,
    action_msg: SMessage,
    synapse: Synapse,
) -> Retained<NSView> {
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(40.0, 24.0));
        let btn: Allocated<UnaButton> = msg_send![UnaButton::class(), alloc];
        let btn: Retained<UnaButton> = msg_send![btn, initWithFrame: frame];
        
        let title = NSString::from_str(label);
        let _: () = msg_send![&btn, setTitle: &*title];
        
        // 1isize = NSRoundedBezelStyle
        let _: () = msg_send![&btn, setBezelStyle: 1isize];

        let _: () = msg_send![&btn, setTarget: &*btn];
        let _: () = msg_send![&btn, setAction: objc2::sel!(onAction:)];

        let syn = synapse.clone();
        let action_closure = move || {
            syn.fire(action_msg.clone());
        };
        *btn.ivars().action.borrow_mut() = Some(Box::new(action_closure));

        Retained::cast_unchecked::<NSView>(btn)
    }
}
