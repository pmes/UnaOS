// SPDX-License-Identifier: GPL-3.0-or-later

//! The Workspace Right Pane (Reactor/Chat)
//!
//! Contains an `NSScrollView` for message history heavily anchored
//! above a responsive `NSTextView` input buffer with "Attach" and "Send" buttons.
//!
//! Follows Step 5 Blueprint exactly:
//! - History: NSScrollView anchored to top/leading/trailing edges.
//! - Input: NSScrollView housing NSTextView anchored below history, rigid height constraint (>= 40, <= 150).
//! - NSTextViewDelegate to intercept Enter.
//! - Attach/Send buttons constrained adjacent to the input buffer.

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2::runtime::{ProtocolObject, NSObjectProtocol, Sel};
use objc2_app_kit::{
    NSButton, NSControl, NSResponder, NSScrollView, NSTextView, NSTextViewDelegate,
    NSView, NSLayoutConstraint, NSStackView, NSUserInterfaceLayoutOrientation,
    NSTextDelegate
};
use objc2_foundation::{NSArray, NSString, NSObject, NSRect};

// In AppKit, we need to extract the string out of the NSTextView
// and then dispatch it.

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaCommsTextViewDelegate"]
    pub struct CommsTextViewDelegate;

    unsafe impl NSObjectProtocol for CommsTextViewDelegate {}
    unsafe impl NSTextDelegate for CommsTextViewDelegate {}

    unsafe impl NSTextViewDelegate for CommsTextViewDelegate {
        #[unsafe(method(textView:doCommandBySelector:))]
        fn do_command(&self, text_view: &NSTextView, command_selector: objc2::sel::Sel) -> bool {
            if command_selector == sel!(insertNewline:) {
                // To check for Shift, we'd query NSApp currentEvent modifierFlags

                unsafe {
                    // 1. Get string
                    let ns_string: Retained<NSString> = msg_send![text_view, string];
                    let message = ns_string.to_string();

                    if message.trim().is_empty() {
                        return true;
                    }

                    // 2. Dispatch
                    // In a full implementation we'd pass the Synapse through Ivars
                    // Here, we log the payload fully and clear the buffer to prove it fired correctly.
                    log::info!("Payload dispatched: {}", message);

                    // 3. Clear text view
                    let empty = NSString::from_str("");
                    let _: () = msg_send![text_view, setString: &*empty];
                }
                return true;
            }
            false
        }
    }
);

pub struct CommsRefs {
    pub container: Retained<NSView>,
    pub history_scroll: Retained<NSScrollView>,
    pub input_scroll: Retained<NSScrollView>,
    pub text_view: Retained<NSTextView>,
    pub send_btn: Retained<NSButton>,
    pub attach_btn: Retained<NSButton>,
    pub delegate: Retained<CommsTextViewDelegate>,
}

pub fn create_comms_pane() -> CommsRefs {
    let mtm = MainThreadOnly::new();

    let container: Retained<NSView> = unsafe { msg_send![NSView::class(), alloc] };
    let container: Retained<NSView> = unsafe { msg_send![container, initWithFrame: NSRect::ZERO] };
    unsafe {
        let _: () = msg_send![&container, setTranslatesAutoresizingMaskIntoConstraints: false];
    }

    // 1. Message History (Top)
    let history_scroll: Retained<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let history_scroll: Retained<NSScrollView> = unsafe { msg_send![history_scroll, initWithFrame: NSRect::ZERO] };
    unsafe {
        let _: () = msg_send![&history_scroll, setTranslatesAutoresizingMaskIntoConstraints: false];
        let _: () = msg_send![&history_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![&history_scroll, setAutohidesScrollers: true];
        let _: () = msg_send![&container, addSubview: &*history_scroll];
    }

    // 2. Input Box (Bottom Left)
    let input_scroll: Retained<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let input_scroll: Retained<NSScrollView> = unsafe { msg_send![input_scroll, initWithFrame: NSRect::ZERO] };
    unsafe {
        let _: () = msg_send![&input_scroll, setTranslatesAutoresizingMaskIntoConstraints: false];
        let _: () = msg_send![&input_scroll, setHasVerticalScroller: true];
    }

    let text_view: Retained<NSTextView> = unsafe { msg_send![NSTextView::class(), alloc] };
    let text_view: Retained<NSTextView> = unsafe { msg_send![text_view, initWithFrame: NSRect::ZERO] };
    unsafe {
        let _: () = msg_send![&text_view, setTranslatesAutoresizingMaskIntoConstraints: false];
        let _: () = msg_send![&text_view, setAllowsUndo: true];
        let _: () = msg_send![&text_view, setRichText: false];
        let _: () = msg_send![&input_scroll, setDocumentView: &*text_view];
    }

    // Setup the Delegate
    let delegate: Retained<CommsTextViewDelegate> = unsafe { msg_send![CommsTextViewDelegate::class(), alloc] };
    let delegate: Retained<CommsTextViewDelegate> = unsafe { msg_send![delegate, init] };
    let delegate_obj: &ProtocolObject<dyn NSTextViewDelegate> = ProtocolObject::from_ref(&*delegate);
    unsafe {
        let _: () = msg_send![&text_view, setDelegate: delegate_obj];
    }

    // 3. Buttons (Bottom Right)
    let attach_str = NSString::from_str("Attach");
    let attach_btn: Retained<NSButton> = unsafe { msg_send![NSButton::class(), buttonWithTitle: &*attach_str, target: None::<&objc2::runtime::AnyObject>, action: core::ptr::null_mut::<Sel>()] };
    unsafe {
        let _: () = msg_send![&attach_btn, setTranslatesAutoresizingMaskIntoConstraints: false];
        let _: () = msg_send![&attach_btn, setBezelStyle: 1_isize];
    }

    let send_str = NSString::from_str("Send");
    let send_btn: Retained<NSButton> = unsafe { msg_send![NSButton::class(), buttonWithTitle: &*send_str, target: None::<&objc2::runtime::AnyObject>, action: core::ptr::null_mut::<Sel>()] };
    unsafe {
        let _: () = msg_send![&send_btn, setTranslatesAutoresizingMaskIntoConstraints: false];
        let _: () = msg_send![&send_btn, setBezelStyle: 1_isize];
    }

    // Container for Bottom Bar (Input Scroll + Buttons)
    let bottom_stack: Retained<NSStackView> = unsafe { msg_send![NSStackView::class(), alloc] };
    let bottom_stack: Retained<NSStackView> = unsafe { msg_send![bottom_stack, init] };
    unsafe {
        let _: () = msg_send![&bottom_stack, setTranslatesAutoresizingMaskIntoConstraints: false];
        let _: () = msg_send![&bottom_stack, setOrientation: 0_isize]; // Horizontal
        let _: () = msg_send![&bottom_stack, setSpacing: 8.0_f64];

        let _: () = msg_send![&bottom_stack, addView: &*input_scroll, inGravity: 1_isize];
        let _: () = msg_send![&bottom_stack, addView: &*attach_btn, inGravity: 1_isize];
        let _: () = msg_send![&bottom_stack, addView: &*send_btn, inGravity: 1_isize];

        let _: () = msg_send![&container, addSubview: &*bottom_stack];
    }

    // Constraints! Billet-Aluminum Wiring!
    unsafe {
        let constraints = [
            // History Scroll to Top, Leading, Trailing of Container
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*history_scroll, 3, 0, Some(&*container), 3, 1.0, 0.0, // Top
            ),
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*history_scroll, 1, 0, Some(&*container), 1, 1.0, 0.0, // Leading
            ),
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*history_scroll, 2, 0, Some(&*container), 2, 1.0, 0.0, // Trailing
            ),

            // History Bottom to Bottom Stack Top (spacing)
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*history_scroll, 4, 0, Some(&*bottom_stack), 3, 1.0, -8.0, // Bottom -> Top
            ),

            // Bottom Stack to Bottom, Leading, Trailing of Container
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*bottom_stack, 4, 0, Some(&*container), 4, 1.0, -8.0, // Bottom (padding)
            ),
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*bottom_stack, 1, 0, Some(&*container), 1, 1.0, 8.0, // Leading (padding)
            ),
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*bottom_stack, 2, 0, Some(&*container), 2, 1.0, -8.0, // Trailing (padding)
            ),

            // Input Box Constraints: Height >= 40, Height <= 150
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*input_scroll, 8, 1, None, 0, 1.0, 40.0, // Height >= 40
            ),
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &*input_scroll, 8, -1, None, 0, 1.0, 150.0, // Height <= 150
            ),
        ];

        let array = NSArray::from_slice(&constraints);
        let _: () = msg_send![NSLayoutConstraint::class(), activateConstraints: &*array];
    }

    CommsRefs {
        container,
        history_scroll,
        input_scroll,
        text_view,
        send_btn,
        attach_btn,
        delegate,
    }
}
