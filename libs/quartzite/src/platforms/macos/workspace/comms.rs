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
    NSLayoutConstraint, NSLayoutAttribute, NSLayoutRelation,
    NSTextField, NSColor, NSStackView, NSStackViewGravity
};
use objc2_foundation::{
    NSObjectProtocol, NSRect, NSPoint, NSSize, MainThreadMarker, NSArray,
    NSString
};
use std::cell::{Cell, RefCell};

// -----------------------------------------------------------------------------
// BUBBLE LAYOUT STATE (MATRIX WEAVER)
// -----------------------------------------------------------------------------

pub struct BubbleLayoutState {
    pub is_user: bool,
    pub staggered_constraints: Retained<NSArray<NSLayoutConstraint>>,
    pub single_column_constraints: Retained<NSArray<NSLayoutConstraint>>,
}

// -----------------------------------------------------------------------------
// FLIPPED DOCUMENT IVARS
// -----------------------------------------------------------------------------

pub struct FlippedDocumentIvars {
    pub is_single_column: Cell<bool>,
    pub bubbles: RefCell<Vec<BubbleLayoutState>>,
}

// -----------------------------------------------------------------------------
// THE MATRIX WEAVER: FLIPPED DOCUMENT VIEW
// -----------------------------------------------------------------------------

define_class!(
    #[unsafe(super(NSView))]
    #[name = "UnaFlippedDocumentView"]
    #[ivars = FlippedDocumentIvars]
    pub struct FlippedDocumentView;

    impl FlippedDocumentView {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(FlippedDocumentIvars {
                is_single_column: Cell::new(false),
                bubbles: RefCell::new(Vec::new()),
            });
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> objc2::runtime::Bool {
            objc2::runtime::Bool::YES // Ensure top-down coordinate space for scrolling
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: NSSize) {
            // 1. MUST route the super message first to apply the true geometry bounds
            let _: () = unsafe { msg_send![super(self, NSView::class()), setFrameSize: new_size] };

            // 2. Intercept bounds, process threshold toggle logic (Can-Am Rules: 450pt threshold)
            let threshold = 450.0;
            let currently_single_column = self.ivars().is_single_column.get();
            let should_be_single_column = new_size.width < threshold;

            // Short circuit if no state transition is required
            if currently_single_column == should_be_single_column {
                return;
            }

            self.ivars().is_single_column.set(should_be_single_column);

            // 3. Perform dynamic constraint hot-swapping
            let bubbles = self.ivars().bubbles.borrow();

            if should_be_single_column {
                for state in bubbles.iter() {
                    unsafe {
                        NSLayoutConstraint::deactivateConstraints(&state.staggered_constraints);
                        NSLayoutConstraint::activateConstraints(&state.single_column_constraints);
                    }
                }
            } else {
                for state in bubbles.iter() {
                    unsafe {
                        NSLayoutConstraint::deactivateConstraints(&state.single_column_constraints);
                        NSLayoutConstraint::activateConstraints(&state.staggered_constraints);
                    }
                }
            }
        }
    }
);

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
// THE BUILDER: APPEND BUBBLE
// -----------------------------------------------------------------------------
pub fn append_bubble(
    doc_view: &Retained<FlippedDocumentView>,
    stack_view: &Retained<NSStackView>,
    text: &str,
    is_user: bool,
) -> Retained<NSView> {
    unsafe {
        // 1. Create the Bubble Container
        let bubble: Allocated<NSView> = msg_send![NSView::class(), alloc];
        let bubble: Retained<NSView> = msg_send![bubble, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(100., 30.))];
        let _: () = msg_send![&bubble, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];

        // Ensure the bubble background is drawn natively (Core Animation backed, or just generic AppKit styling)
        let _: () = msg_send![&bubble, setWantsLayer: objc2::runtime::Bool::YES];
        let layer: *mut objc2::runtime::AnyObject = msg_send![&bubble, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: 10.0f64];
            let color = if is_user { NSColor::systemBlueColor() } else { NSColor::systemGrayColor() };
            let cg_color: *mut objc2::runtime::AnyObject = msg_send![&color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
        }

        // 2. Create the NSTextField
        let text_field: Allocated<NSTextField> = msg_send![NSTextField::class(), alloc];
        let text_field: Retained<NSTextField> = msg_send![text_field, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(100., 30.))];
        let _: () = msg_send![&text_field, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];

        text_field.setStringValue(&NSString::from_str(text));
        let _: () = msg_send![&text_field, setEditable: objc2::runtime::Bool::NO];
        let _: () = msg_send![&text_field, setSelectable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&text_field, setBordered: objc2::runtime::Bool::NO];
        let _: () = msg_send![&text_field, setDrawsBackground: objc2::runtime::Bool::NO];
        let text_color = if is_user { NSColor::whiteColor() } else { NSColor::labelColor() };
        text_field.setTextColor(Some(&text_color));

        // Enable word wrapping
        let cell: *mut objc2::runtime::AnyObject = msg_send![&text_field, cell];
        if !cell.is_null() {
            let _: () = msg_send![cell, setWraps: objc2::runtime::Bool::YES];
        }

        bubble.addSubview(&text_field);

        // 3. Anchor Text Field to Bubble (8pt Padding)
        let internal_constraints = NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &text_field, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                Some(&bubble), NSLayoutAttribute::Top, 1.0, 8.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &text_field, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                Some(&bubble), NSLayoutAttribute::Bottom, 1.0, -8.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &text_field, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&bubble), NSLayoutAttribute::Leading, 1.0, 8.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &text_field, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                Some(&bubble), NSLayoutAttribute::Trailing, 1.0, -8.0
            ),
        ]);
        let _: () = msg_send![&bubble, addConstraints: &*internal_constraints];

        // 4. Inject Bubble into StackView
        stack_view.addView_inGravity(&bubble, NSStackViewGravity::Top);

        // 5. Build X-Axis Constraints (Matrix Staggered)
        // 75% Max Width Constraint (relative to Document View width minus padding)
        let doc_view_nsview = Retained::cast_unchecked::<NSView>(doc_view.clone());
        let max_width = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &bubble, NSLayoutAttribute::Width, NSLayoutRelation::LessThanOrEqual,
            Some(&doc_view_nsview), NSLayoutAttribute::Width, 0.75, -32.0 // minus 16pt left + 16pt right theoretical padding
        );

        let stagger_x = if is_user {
            // User: Anchor Trailing (Right side)
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &bubble, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                Some(&doc_view_nsview), NSLayoutAttribute::Trailing, 1.0, -16.0
            )
        } else {
            // AI: Anchor Leading (Left side)
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &bubble, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&doc_view_nsview), NSLayoutAttribute::Leading, 1.0, 16.0
            )
        };

        let staggered_constraints = NSArray::from_slice(&[&*max_width, &*stagger_x]);

        // 6. Build X-Axis Constraints (Single-Column)
        // Unified Leading Anchor for both User and AI
        let single_x = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &bubble, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
            Some(&doc_view_nsview), NSLayoutAttribute::Leading, 1.0, 16.0
        );
        let single_column_constraints = NSArray::from_slice(&[&*max_width, &*single_x]);

        // 7. Inject state into Document Ivars
        let state = BubbleLayoutState {
            is_user,
            staggered_constraints,
            single_column_constraints,
        };
        doc_view.ivars().bubbles.borrow_mut().push(state);

        // 8. Activate Initial Geometry Set
        let currently_single_column = doc_view.ivars().is_single_column.get();
        if currently_single_column {
            NSLayoutConstraint::activateConstraints(&doc_view.ivars().bubbles.borrow().last().unwrap().single_column_constraints);
        } else {
            NSLayoutConstraint::activateConstraints(&doc_view.ivars().bubbles.borrow().last().unwrap().staggered_constraints);
        }

        bubble
    }
}

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

    // Transparent Backgrounds for Comms
    unsafe {
        let clear_color = NSColor::clearColor();
        let _: () = msg_send![&matrix_scroll, setBackgroundColor: &*clear_color];
        let _: () = msg_send![&matrix_scroll, setDrawsBackground: objc2::runtime::Bool::NO];
    }

    // Initialize FlippedDocumentView
    let doc_view: Allocated<FlippedDocumentView> = unsafe { msg_send![FlippedDocumentView::class(), alloc] };
    let doc_view: Retained<FlippedDocumentView> = unsafe { msg_send![doc_view, init] };
    unsafe {
        let _: () = msg_send![&doc_view, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }

    // Initialize NSStackView for Y-Axis
    let stack_view: Allocated<NSStackView> = unsafe { msg_send![NSStackView::class(), alloc] };
    let stack_view: Retained<NSStackView> = unsafe { msg_send![stack_view, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&stack_view, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }
    stack_view.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
    stack_view.setSpacing(16.0);
    // Explicitly disable orthogonal alignment (X-axis) so our custom constraints don't collide
    unsafe {
        let _: () = msg_send![&stack_view, setAlignment: NSLayoutAttribute::NotAnAttribute];
    }

    doc_view.addSubview(&stack_view);

    // Anchor NSStackView to FlippedDocumentView
    unsafe {
        let stack_constraints = NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &stack_view, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                Some(&doc_view), NSLayoutAttribute::Top, 1.0, 16.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &stack_view, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                Some(&doc_view), NSLayoutAttribute::Bottom, 1.0, -16.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &stack_view, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&doc_view), NSLayoutAttribute::Leading, 1.0, 0.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &stack_view, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                Some(&doc_view), NSLayoutAttribute::Trailing, 1.0, 0.0
            ),
        ]);
        let _: () = msg_send![&doc_view, addConstraints: &*stack_constraints];
    }

    matrix_scroll.setDocumentView(Some(&doc_view));

    // Anchor DocumentView width to the scroll view content view
    let m_content_view = matrix_scroll.contentView();
    let m_cv = unsafe { Retained::cast_unchecked::<NSView>(m_content_view) };
    unsafe {
        let doc_constraints = NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &doc_view, NSLayoutAttribute::Width, NSLayoutRelation::Equal,
                Some(&m_cv), NSLayoutAttribute::Width, 1.0, 0.0
            )
        ]);
        let _: () = msg_send![&m_cv, addConstraints: &*doc_constraints];
    }

    // Inject Test Bubbles to prove the Matrix
    append_bubble(&doc_view, &stack_view, "Wake up, Neo...", false);
    append_bubble(&doc_view, &stack_view, "Who are you?", true);
    append_bubble(&doc_view, &stack_view, "The Matrix has you.", false);
    append_bubble(&doc_view, &stack_view, "Follow the white rabbit.", false);

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
        text_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    }

    // Let the text view size to its content width, and grow vertically
    text_view.setVerticallyResizable(true);
    text_view.setHorizontallyResizable(false);

    // Anchor text view into its scroll view
    input_scroll.setDocumentView(Some(&text_view));

    // Anchor text view explicitly to the scroll view's content view
    let content_view = input_scroll.contentView();
    let cv = unsafe { Retained::cast_unchecked::<NSView>(content_view) };
    let constraints = unsafe {
        NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &text_view, NSLayoutAttribute::Width, NSLayoutRelation::Equal,
                Some(&cv), NSLayoutAttribute::Width, 1.0, 0.0
            )
        ])
    };
    unsafe {
        let _: () = msg_send![&cv, addConstraints: &*constraints];
    }

    split_view.addSubview(&input_scroll);

    // The SplitView will manage sizing the two scroll views.
    // The user can drag the horizontal divider.
    // Ensure the input scroll view doesn't collapse to 0:
    let constraints = unsafe {
        NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_scroll, NSLayoutAttribute::Height, NSLayoutRelation::GreaterThanOrEqual,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 50.0 // Minimum 50px input height
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &matrix_scroll, NSLayoutAttribute::Height, NSLayoutRelation::GreaterThanOrEqual,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 150.0 // Minimum 150px chat height
            )
        ])
    };
    unsafe {
        let _: () = msg_send![&split_view, addConstraints: &*constraints];
    }

    (unsafe { Retained::cast_unchecked::<NSView>(split_view) }, delegate)
}
