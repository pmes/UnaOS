// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use core::cell::RefCell;

use objc2::runtime::ProtocolObject;
use objc2::{
    define_class, msg_send,
    ClassType,
    DefinedClass,
    MainThreadOnly,
    rc::{Allocated, Retained},
};
use objc2_foundation::{
    NSObject,
    NSObjectProtocol,
    NSRect, NSPoint, NSSize,
    NSArray,
};
use objc2_app_kit::{
    NSView,
    NSViewController,
    NSScrollView,
    NSTextView,
    NSTextDelegate,
    NSLayoutConstraint,
    NSLayoutAttribute,
    NSLayoutRelation,
    NSResponder,
};

pub struct CommsTextViewDelegateIvars {
    // In a full app, this would hold `tx_event` or some context for emitting user inputs.
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "LumenCommsTextViewDelegate"]
    #[ivars = CommsTextViewDelegateIvars]
    pub struct CommsTextViewDelegate;

    unsafe impl NSObjectProtocol for CommsTextViewDelegate {}

    unsafe impl NSTextDelegate for CommsTextViewDelegate {
        // Implement delegate methods when needed
    }
);

impl CommsTextViewDelegate {
    pub fn new() -> Retained<Self> {
        let alloc: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        let this = alloc.set_ivars(CommsTextViewDelegateIvars {});
        unsafe { msg_send![super(this), init] }
    }
}


pub struct CommsVCIvars {
    pub comms_delegate: RefCell<Option<Retained<CommsTextViewDelegate>>>,
}

define_class!(
    #[unsafe(super(NSViewController))]
    #[name = "LumenCommsViewController"]
    #[ivars = CommsVCIvars]
    pub struct CommsViewController;
);

impl CommsViewController {
    pub fn new() -> Retained<Self> {
        let alloc: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        let this = alloc.set_ivars(CommsVCIvars {
            comms_delegate: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub fn build_comms_pane() -> (Retained<NSView>, Retained<CommsViewController>) {
    // We create a root view to hold the scrollview
    let root_alloc: Allocated<NSView> = unsafe { msg_send![NSView::class(), alloc] };
    let root_view: Retained<NSView> = unsafe { msg_send![root_alloc, initWithFrame: NSRect::ZERO] };

    let scroll_alloc: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let scroll_view: Retained<NSScrollView> = unsafe { msg_send![scroll_alloc, initWithFrame: NSRect::ZERO] };

    let text_alloc: Allocated<NSTextView> = unsafe { msg_send![NSTextView::class(), alloc] };
    let text_view: Retained<NSTextView> = unsafe { msg_send![text_alloc, initWithFrame: NSRect::ZERO] };

    let delegate = CommsTextViewDelegate::new();

    unsafe {
        root_view.setTranslatesAutoresizingMaskIntoConstraints(false);
        scroll_view.setTranslatesAutoresizingMaskIntoConstraints(false);
        text_view.setTranslatesAutoresizingMaskIntoConstraints(false);

        scroll_view.setHasVerticalScroller(true);
        scroll_view.setDocumentView(Some(&text_view));

        text_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        root_view.addSubview(&scroll_view);

        // Manually build NSLayoutConstraint via strong bindings
        // Note: Using explicitly wired constraints with strong typed wrappers

        let c1: Retained<NSLayoutConstraint> = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &scroll_view,
            NSLayoutAttribute(3), // NSLayoutAttributeTop
            NSLayoutRelation(0),  // NSLayoutRelationEqual
            Some(&root_view),
            NSLayoutAttribute(3), // NSLayoutAttributeTop
            1.0,
            0.0
        );

        let c2: Retained<NSLayoutConstraint> = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &scroll_view,
            NSLayoutAttribute(4), // NSLayoutAttributeBottom
            NSLayoutRelation(0),
            Some(&root_view),
            NSLayoutAttribute(4),
            1.0,
            0.0
        );

        let c3: Retained<NSLayoutConstraint> = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &scroll_view,
            NSLayoutAttribute(1), // NSLayoutAttributeLeading
            NSLayoutRelation(0),
            Some(&root_view),
            NSLayoutAttribute(1),
            1.0,
            0.0
        );

        let c4: Retained<NSLayoutConstraint> = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &scroll_view,
            NSLayoutAttribute(2), // NSLayoutAttributeTrailing
            NSLayoutRelation(0),
            Some(&root_view),
            NSLayoutAttribute(2),
            1.0,
            0.0
        );

        let constraints = NSArray::from_slice(&[&*c1, &*c2, &*c3, &*c4]);
        NSLayoutConstraint::activateConstraints(&constraints);
    }

    let vc = CommsViewController::new();
    *vc.ivars().comms_delegate.borrow_mut() = Some(delegate);
    unsafe {
        vc.setView(&root_view);
    }

    (root_view, vc)
}
