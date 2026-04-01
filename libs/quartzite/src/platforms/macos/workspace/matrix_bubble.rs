// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSView, NSLayoutConstraint, NSLayoutAttribute, NSLayoutRelation,
    NSTextField, NSColor, NSStackView, NSStackViewGravity, NSBox
};
use objc2_foundation::{
    NSRect, NSPoint, NSSize, NSArray, NSString
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
                    NSLayoutConstraint::deactivateConstraints(&state.staggered_constraints);
                    NSLayoutConstraint::activateConstraints(&state.single_column_constraints);
                }
            } else {
                for state in bubbles.iter() {
                    NSLayoutConstraint::deactivateConstraints(&state.single_column_constraints);
                    NSLayoutConstraint::activateConstraints(&state.staggered_constraints);
                }
            }
        }
    }
);

// -----------------------------------------------------------------------------
// THE BUILDER: APPEND BUBBLE
// -----------------------------------------------------------------------------
pub fn append_bubble(
    doc_view: &Retained<FlippedDocumentView>,
    stack_view: &Retained<NSStackView>,
    content: &str,
    sender: &str,
    subject: &str,
    timestamp: &str,
    is_chat: bool,
    is_user: bool,
) -> Retained<NSView> {
    unsafe {
        // --- GTK PATTERN PORT: TRUNCATION & FORMATTING ---
        let mut display_text = String::new();

        if is_chat {
            let explicit_lines = content.trim_end().lines().count();
            let is_long_message = content.len() > 500 || explicit_lines > 7;

            if is_long_message {
                let mut byte_idx = 0;
                let mut line_count = 0;
                for (idx, c) in content.char_indices() {
                    if c == '\n' { line_count += 1; }
                    if line_count >= 7 || idx >= 500 {
                        byte_idx = idx;
                        break;
                    }
                    byte_idx = idx + c.len_utf8();
                }
                if byte_idx < content.len() {
                    display_text.push_str(&content[..byte_idx]);
                    display_text.push_str("\n... [Truncated - Expansion Pending]");
                } else {
                    display_text.push_str(content);
                }
            } else {
                display_text.push_str(content);
            }
        } else {
            // System/Memory Payload (GTK Expander Port)
            display_text.push_str(&format!("▶ {} | {} | {}\n", sender, subject, timestamp));
            // Safely find the byte index of the 100th character
            let preview = match content.char_indices().nth(100) {
                Some((idx, _)) => format!("{}...", &content[..idx]),
                None => content.to_string(),
            };
            display_text.push_str(&preview);
        }

        // 1. Create the Bubble Container (NSBox)
        let bubble: Allocated<NSBox> = msg_send![NSBox::class(), alloc];
        let bubble: Retained<NSBox> = msg_send![bubble, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(100., 30.))];
        let _: () = msg_send![&bubble, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];

        let _: () = msg_send![&bubble, setBoxType: 4isize]; // NSBoxCustom
        let _: () = msg_send![&bubble, setBorderType: 0isize]; // NSNoBorder
        let _: () = msg_send![&bubble, setCornerRadius: 8.0f64];
        let _: () = msg_send![&bubble, setTitlePosition: 0isize]; // NSNoTitle

        // GTK Color Segregation
        let color = if is_chat {
            if is_user { NSColor::systemBlueColor() } else { NSColor::systemGrayColor() }
        } else {
            // FIXED: Use controlColor to provide actual contrast against the window background
            NSColor::controlColor()
        };
        let _: () = msg_send![&bubble, setFillColor: &*color];

        // FIXED: Force CoreAnimation to guarantee the custom fill actually renders
        let _: () = msg_send![&bubble, setWantsLayer: objc2::runtime::Bool::YES];

        // 2. Create the NSTextField
        let text_field: Allocated<NSTextField> = msg_send![NSTextField::class(), alloc];
        let text_field: Retained<NSTextField> = msg_send![text_field, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(100., 30.))];
        let _: () = msg_send![&text_field, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];

        text_field.setStringValue(&NSString::from_str(&display_text));
        let _: () = msg_send![&text_field, setEditable: objc2::runtime::Bool::NO];
        let _: () = msg_send![&text_field, setSelectable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&text_field, setBordered: objc2::runtime::Bool::NO];
        let _: () = msg_send![&text_field, setDrawsBackground: objc2::runtime::Bool::NO];

        let text_color = if is_chat && is_user { NSColor::whiteColor() } else { NSColor::labelColor() };
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
        let doc_view_nsview = Retained::cast_unchecked::<NSView>(doc_view.clone());
        let max_width = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &bubble, NSLayoutAttribute::Width, NSLayoutRelation::LessThanOrEqual,
            Some(&doc_view_nsview), NSLayoutAttribute::Width, 0.75, -32.0
        );

        let stagger_x = if is_user {
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &bubble, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                Some(&doc_view_nsview), NSLayoutAttribute::Trailing, 1.0, -16.0
            )
        } else {
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &bubble, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&doc_view_nsview), NSLayoutAttribute::Leading, 1.0, 16.0
            )
        };

        let staggered_constraints = NSArray::from_slice(&[&*max_width, &*stagger_x]);

        // 6. Build X-Axis Constraints (Single-Column)
        let single_x = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &bubble, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
            Some(&doc_view_nsview), NSLayoutAttribute::Leading, 1.0, 16.0
        );
        let single_max_width = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &bubble, NSLayoutAttribute::Width, NSLayoutRelation::LessThanOrEqual,
            Some(&doc_view_nsview), NSLayoutAttribute::Width, 1.0, -32.0
        );
        let single_column_constraints = NSArray::from_slice(&[&*single_max_width, &*single_x]);

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

        let bubble_nsview = Retained::cast_unchecked::<NSView>(bubble);
        bubble_nsview
    }
}
