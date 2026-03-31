// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, ClassType};
use objc2_app_kit::{
    NSResponder, NSTextView, NSTextViewDelegate, NSTextDelegate,
    NSSplitView, NSSplitViewDelegate, NSScrollView, NSView,
    NSLayoutConstraint, NSLayoutAttribute, NSLayoutRelation
};
use objc2_foundation::{
    NSObjectProtocol, NSRect, NSPoint, NSSize, MainThreadMarker, NSArray
};

// -----------------------------------------------------------------------------
// COMMS DELEGATE (LUMEN REACTOR CHAT)
// -----------------------------------------------------------------------------
pub struct CommsDelegateIvars {}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaCommsDelegate"]
    #[ivars = CommsDelegateIvars]
    pub struct CommsDelegate;

    impl CommsDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(CommsDelegateIvars {});
            unsafe { msg_send![super(this), init] }
        }
    }

    // --- NSTextViewDelegate ---
    unsafe impl NSTextViewDelegate for CommsDelegate {
        #[unsafe(method(textView:doCommandBySelector:))]
        fn text_view_do_command_by_selector(
            &self,
            _text_view: &NSTextView,
            _command_selector: objc2::runtime::Sel,
        ) -> objc2::runtime::Bool {
            objc2::runtime::Bool::NO
        }
    }
);

unsafe impl NSObjectProtocol for CommsDelegate {}
unsafe impl NSTextDelegate for CommsDelegate {}
unsafe impl NSSplitViewDelegate for CommsDelegate {}

// -----------------------------------------------------------------------------
// ASSEMBLY
// -----------------------------------------------------------------------------
pub fn create_comms(_mtm: MainThreadMarker) -> (Retained<NSView>, Retained<CommsDelegate>) {
    // 1. Instantiate the delegate
    let delegate: Allocated<CommsDelegate> = unsafe { msg_send![CommsDelegate::class(), alloc] };
    let delegate: Retained<CommsDelegate> = unsafe { msg_send![delegate, init] };

    // 2. Main Vertical SplitView (The Slider)
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(774.0, 768.0));
    let split_view: Allocated<NSSplitView> = unsafe { msg_send![NSSplitView::class(), alloc] };
    let split_view: Retained<NSSplitView> = unsafe { msg_send![split_view, initWithFrame: frame] };
    split_view.setVertical(false); // Horizontal divider, stacking vertically
    split_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    // Turn off automatic constraints on the root container
    unsafe {
        let _: () = msg_send![&split_view, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }

    // 3. Top Split: Bubble Matrix Placeholder (NSScrollView)
    let matrix_scroll: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let matrix_scroll: Retained<NSScrollView> = unsafe { msg_send![matrix_scroll, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&matrix_scroll, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }
    matrix_scroll.setHasVerticalScroller(true);
    matrix_scroll.setHasHorizontalScroller(false);
    matrix_scroll.setAutohidesScrollers(true);

    // Add it to the split view
    split_view.addSubview(&matrix_scroll);

    // 4. Bottom Split: Input Buffer
    let input_scroll: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let input_scroll: Retained<NSScrollView> = unsafe { msg_send![input_scroll, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&input_scroll, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }
    input_scroll.setHasVerticalScroller(true);
    input_scroll.setHasHorizontalScroller(false);
    input_scroll.setAutohidesScrollers(true);

    let text_view: Allocated<NSTextView> = unsafe { msg_send![NSTextView::class(), alloc] };
    let text_view: Retained<NSTextView> = unsafe { msg_send![text_view, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&text_view, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }
    text_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    // Let the text view size to its content width, and grow vertically
    text_view.setVerticallyResizable(true);
    text_view.setHorizontallyResizable(false);

    // Anchor text view into its scroll view
    input_scroll.setDocumentView(Some(&text_view));

    // Anchor text view explicitly to the scroll view's content view
    if let content_view = input_scroll.contentView() {
        let cv = Retained::cast::<NSView>(content_view);
        let constraints = unsafe {
            NSArray::from_slice(&[
                NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                    &text_view, NSLayoutAttribute::Width, NSLayoutRelation::Equal,
                    Some(&cv), NSLayoutAttribute::Width, 1.0, 0.0
                )
            ])
        };
        unsafe {
            let _: () = msg_send![&cv, addConstraints: &*constraints];
        }
    }

    split_view.addSubview(&input_scroll);

    // The SplitView will manage sizing the two scroll views.
    // The user can drag the horizontal divider.
    // Ensure the input scroll view doesn't collapse to 0:
    let constraints = unsafe {
        NSArray::from_slice(&[
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_scroll, NSLayoutAttribute::Height, NSLayoutRelation::GreaterThanOrEqual,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 50.0 // Minimum 50px input height
            ),
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &matrix_scroll, NSLayoutAttribute::Height, NSLayoutRelation::GreaterThanOrEqual,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 150.0 // Minimum 150px chat height
            )
        ])
    };
    unsafe {
        let _: () = msg_send![&split_view, addConstraints: &*constraints];
    }

    (Retained::cast::<NSView>(split_view), delegate)
}
