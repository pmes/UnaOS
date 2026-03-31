// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::Retained;
use objc2::{define_class, msg_send, ClassType, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSControlTextEditingDelegate, NSFont, NSScrollView, NSStackView, NSStackViewDistribution,
    NSTextDelegate, NSTextView, NSTextViewDelegate, NSView, NSColor
};
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSLayoutAttribute, NSLayoutConstraint, NSLayoutRelation, NSObjectProtocol,
    NSPoint, NSRect, NSSize, NSString
};
use std::cell::RefCell;

// -----------------------------------------------------------------------------
// COMMS TEXT DELEGATE (Input Buffer)
// -----------------------------------------------------------------------------

pub struct CommsTextViewDelegateIvars {
    pub text_view: RefCell<Option<Retained<NSTextView>>>,
}

define_class!(
    #[unsafe(super(objc2_app_kit::NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumenCommsTextViewDelegate"]
    #[ivars = CommsTextViewDelegateIvars]
    pub struct CommsTextViewDelegate;

    unsafe impl NSObjectProtocol for CommsTextViewDelegate {}

    // Required undocumented empty super-protocol for text views
    unsafe impl NSTextDelegate for CommsTextViewDelegate {}

    unsafe impl NSTextViewDelegate for CommsTextViewDelegate {
        #[unsafe(method(scrollCommsToBottomNotification:))]
        fn scrollCommsToBottomNotification(&self, _notification: &objc2_foundation::NSNotification) {
            // Trigger NSScrollView to jump to the bottom when new message arrives
            if let Some(view) = self.ivars().text_view.borrow().as_ref() {
                let range = objc2_foundation::NSRange::new(view.string().len(), 0);
                view.scrollRangeToVisible(range);
            }
        }

        #[unsafe(method(textView:doCommandBySelector:))]
        fn textView_doCommandBySelector(
            &self,
            _text_view: &NSTextView,
            command_selector: objc2::runtime::Sel,
        ) -> bool {
            let selector_name = command_selector.name();
            // Enter key triggers insertNewline:
            if selector_name == "insertNewline:" {
                // Here we would intercept to send the SMessage rather than newline,
                // but for now we let it pass. If we return true, it blocks the default action.
                return false;
            }
            false
        }
    }
);

impl CommsTextViewDelegate {
    pub fn new() -> Retained<Self> {
        let _mtm = MainThreadOnly::new().unwrap();
        let this = Self::alloc().set_ivars(CommsTextViewDelegateIvars {
            text_view: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

// -----------------------------------------------------------------------------
// UI BUILDER
// -----------------------------------------------------------------------------

pub fn build(_mtm: MainThreadOnly) -> (Retained<NSView>, Retained<CommsTextViewDelegate>) {
    unsafe {
        // Root Container
        let container: Retained<NSView> = msg_send![NSView::class(), alloc];
        let container: Retained<NSView> = msg_send![container, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 800.0))];
        container.setTranslatesAutoresizingMaskIntoConstraints(false);

        // History Scroll View (Upper area)
        let history_scroll: Retained<NSScrollView> = msg_send![NSScrollView::class(), alloc];
        let history_scroll: Retained<NSScrollView> = msg_send![history_scroll, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0))];
        history_scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        history_scroll.setHasVerticalScroller(true);
        history_scroll.setAutohidesScrollers(true);

        // Create a dummy document view for the history
        let history_content: Retained<NSView> = msg_send![NSView::class(), alloc];
        let history_content: Retained<NSView> = msg_send![history_content, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 1000.0))];
        history_scroll.setDocumentView(Some(&history_content));

        // 1. Attach Button
        let attach_btn: Retained<NSButton> = msg_send![NSButton::class(), alloc];
        let attach_btn: Retained<NSButton> = msg_send![attach_btn, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(32.0, 32.0))];
        attach_btn.setTranslatesAutoresizingMaskIntoConstraints(false);
        attach_btn.setTitle(&NSString::from_str("+"));
        attach_btn.setBezelStyle(objc2_app_kit::NSBezelStyle::Circular);

        // 2. Text Input Scroll View
        let input_scroll: Retained<NSScrollView> = msg_send![NSScrollView::class(), alloc];
        let input_scroll: Retained<NSScrollView> = msg_send![input_scroll, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(700.0, 44.0))];
        input_scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        input_scroll.setHasVerticalScroller(true);
        input_scroll.setAutohidesScrollers(true);

        // 2.1 The actual NSTextView
        let text_view: Retained<NSTextView> = msg_send![NSTextView::class(), alloc];
        let text_view: Retained<NSTextView> = msg_send![text_view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(700.0, 100.0))];
        text_view.setTranslatesAutoresizingMaskIntoConstraints(false);
        text_view.setAllowsUndo(true);
        text_view.setRichText(false);
        if let font = NSFont::systemFontOfSize(14.0) {
            text_view.setFont(Some(&font));
        }

        // Wire up the delegate
        let delegate = CommsTextViewDelegate::new();
        let delegate_obj = objc2::runtime::ProtocolObject::from_ref(&*delegate);
        text_view.setDelegate(Some(delegate_obj));
        *delegate.ivars().text_view.borrow_mut() = Some(text_view.clone());

        let center = objc2_foundation::NSNotificationCenter::defaultCenter();
        let name = NSString::from_str("org.unaos.lumen.ScrollCommsToBottom");
        let sel = objc2::sel!(scrollCommsToBottomNotification:);
        center.addObserver_selector_name_object(&*delegate, sel, Some(&name), None::<&AnyObject>);

        input_scroll.setDocumentView(Some(&text_view));

        // 3. Send Button
        let send_btn: Retained<NSButton> = msg_send![NSButton::class(), alloc];
        let send_btn: Retained<NSButton> = msg_send![send_btn, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(60.0, 32.0))];
        send_btn.setTranslatesAutoresizingMaskIntoConstraints(false);
        send_btn.setTitle(&NSString::from_str("Send"));
        send_btn.setBezelStyle(objc2_app_kit::NSBezelStyle::Rounded);

        // Add to main container
        container.addSubview(&history_scroll);
        container.addSubview(&attach_btn);
        container.addSubview(&input_scroll);
        container.addSubview(&send_btn);

        // ---------------------------------------------------------------------
        // CONSTRAINTS
        // ---------------------------------------------------------------------

        // History Scroll
        let c1 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &history_scroll,
            NSLayoutAttribute::Leading,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&container)),
            NSLayoutAttribute::Leading,
            1.0,
            0.0,
        );
        let c2 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &history_scroll,
            NSLayoutAttribute::Trailing,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&container)),
            NSLayoutAttribute::Trailing,
            1.0,
            0.0,
        );
        let c3 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &history_scroll,
            NSLayoutAttribute::Top,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&container)),
            NSLayoutAttribute::Top,
            1.0,
            0.0,
        );
        let c4 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &history_scroll,
            NSLayoutAttribute::Bottom,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&input_scroll)),
            NSLayoutAttribute::Top,
            1.0,
            -12.0, // Space between history and input
        );

        // Attach Button
        let c5 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &attach_btn,
            NSLayoutAttribute::Leading,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&container)),
            NSLayoutAttribute::Leading,
            1.0,
            12.0,
        );
        let c6 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &attach_btn,
            NSLayoutAttribute::Bottom,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&container)),
            NSLayoutAttribute::Bottom,
            1.0,
            -12.0,
        );
        let c7 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &attach_btn,
            NSLayoutAttribute::Width,
            NSLayoutRelation::Equal,
            None::<&AnyObject>,
            NSLayoutAttribute::NotAnAttribute,
            1.0,
            32.0,
        );
        let c8 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &attach_btn,
            NSLayoutAttribute::Height,
            NSLayoutRelation::Equal,
            None::<&AnyObject>,
            NSLayoutAttribute::NotAnAttribute,
            1.0,
            32.0,
        );

        // Input Scroll
        let c9 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &input_scroll,
            NSLayoutAttribute::Leading,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&attach_btn)),
            NSLayoutAttribute::Trailing,
            1.0,
            8.0,
        );
        let c10 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &input_scroll,
            NSLayoutAttribute::Trailing,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&send_btn)),
            NSLayoutAttribute::Leading,
            1.0,
            -8.0,
        );
        let c11 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &input_scroll,
            NSLayoutAttribute::Bottom,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&container)),
            NSLayoutAttribute::Bottom,
            1.0,
            -12.0,
        );
        let c12 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &input_scroll,
            NSLayoutAttribute::Height,
            NSLayoutRelation::GreaterThanOrEqual,
            None::<&AnyObject>,
            NSLayoutAttribute::NotAnAttribute,
            1.0,
            40.0,
        );
        let c13 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &input_scroll,
            NSLayoutAttribute::Height,
            NSLayoutRelation::LessThanOrEqual,
            None::<&AnyObject>,
            NSLayoutAttribute::NotAnAttribute,
            1.0,
            150.0,
        );

        // Inner NSTextView Match Width to ScrollView
        let c14 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &text_view,
            NSLayoutAttribute::Width,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&input_scroll)),
            NSLayoutAttribute::Width,
            1.0,
            0.0,
        );

        // Send Button
        let c15 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &send_btn,
            NSLayoutAttribute::Trailing,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&container)),
            NSLayoutAttribute::Trailing,
            1.0,
            -12.0,
        );
        let c16 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &send_btn,
            NSLayoutAttribute::Bottom,
            NSLayoutRelation::Equal,
            Some(objc2::rc::Retained::as_super(&container)),
            NSLayoutAttribute::Bottom,
            1.0,
            -12.0,
        );
        let c17 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &send_btn,
            NSLayoutAttribute::Width,
            NSLayoutRelation::Equal,
            None::<&AnyObject>,
            NSLayoutAttribute::NotAnAttribute,
            1.0,
            60.0,
        );
        let c18 = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &send_btn,
            NSLayoutAttribute::Height,
            NSLayoutRelation::Equal,
            None::<&AnyObject>,
            NSLayoutAttribute::NotAnAttribute,
            1.0,
            32.0,
        );

        let constraints = NSArray::from_slice(&[
            &*c1, &*c2, &*c3, &*c4, &*c5, &*c6, &*c7, &*c8,
            &*c9, &*c10, &*c11, &*c12, &*c13, &*c14, &*c15, &*c16, &*c17, &*c18
        ]);
        NSLayoutConstraint::activateConstraints(&constraints);

        (container, delegate)
    }
}
