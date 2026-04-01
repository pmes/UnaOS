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
    NSTextField, NSColor, NSStackView, NSStackViewGravity, NSBox,
    NSButton, NSCellImagePosition, NSImageScaling, NSImage
};
use objc2_foundation::{
    NSRect, NSPoint, NSSize, NSArray, NSString, NSObject, NSObjectProtocol
};
use std::cell::{Cell, RefCell};

// -----------------------------------------------------------------------------
// INTERACTIVE EXPANDER TARGET (TARGET-ACTION BRIDGE)
// -----------------------------------------------------------------------------
pub struct ExpanderIvars {
    pub is_expanded: Cell<bool>,
    pub full_text: RefCell<String>,
    pub truncated_text: RefCell<String>,
    pub text_field: RefCell<Option<Retained<NSTextField>>>,
    pub button: RefCell<Option<Retained<NSButton>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "UnaBubbleExpanderTarget"]
    #[ivars = ExpanderIvars]
    pub struct BubbleExpanderTarget;

    impl BubbleExpanderTarget {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(ExpanderIvars {
                is_expanded: Cell::new(false),
                full_text: RefCell::new(String::new()),
                truncated_text: RefCell::new(String::new()),
                text_field: RefCell::new(None),
                button: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(toggle:))]
        fn toggle(&self, _sender: &objc2::runtime::AnyObject) {
            let expanded = !self.ivars().is_expanded.get();
            self.ivars().is_expanded.set(expanded);

            let tf_ref = self.ivars().text_field.borrow();
            let btn_ref = self.ivars().button.borrow();

            if let (Some(tf), Some(btn)) = (tf_ref.as_ref(), btn_ref.as_ref()) {
                unsafe {
                    if expanded {
                        tf.setStringValue(&NSString::from_str(&self.ivars().full_text.borrow()));
                        let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                            &NSString::from_str("chevron.up"), None
                        );
                        let _: () = msg_send![btn, setImage: img.as_deref()];
                    } else {
                        tf.setStringValue(&NSString::from_str(&self.ivars().truncated_text.borrow()));
                        let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                            &NSString::from_str("chevron.down"), None
                        );
                        let _: () = msg_send![btn, setImage: img.as_deref()];
                    }
                }
            }
        }
    }
);
unsafe impl NSObjectProtocol for BubbleExpanderTarget {}

// -----------------------------------------------------------------------------
// BUBBLE LAYOUT STATE (MATRIX WEAVER)
// -----------------------------------------------------------------------------

pub struct BubbleLayoutState {
    pub is_user: bool,
    pub staggered_constraints: Retained<NSArray<NSLayoutConstraint>>,
    pub single_column_constraints: Retained<NSArray<NSLayoutConstraint>>,
    pub expander_target: Option<Retained<BubbleExpanderTarget>>,
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
        let mut full_text = String::new();
        let mut truncated_text = String::new();
        let mut is_long_message = false;

        if is_chat {
            let explicit_lines = content.trim_end().lines().count();
            is_long_message = content.len() > 500 || explicit_lines > 7;
            full_text.push_str(content);

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
                    truncated_text.push_str(&content[..byte_idx]);
                    truncated_text.push_str("\n...");
                } else {
                    truncated_text.push_str(content);
                    is_long_message = false;
                }
            } else {
                truncated_text.push_str(content);
            }
        } else {
            is_long_message = true;
            let header = format!("▶ {} | {} | {}\n", sender, subject, timestamp);
            full_text.push_str(&header);
            full_text.push_str(content);

            truncated_text.push_str(&header);
            let preview = match content.char_indices().nth(100) {
                Some((idx, _)) => format!("{}...", &content[..idx]),
                None => content.to_string(),
            };
            truncated_text.push_str(&preview);
        }

        let bubble: Allocated<NSBox> = msg_send![NSBox::class(), alloc];
        let bubble: Retained<NSBox> = msg_send![bubble, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(100., 30.))];
        let _: () = msg_send![&bubble, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        let _: () = msg_send![&bubble, setBoxType: 4isize];
        let _: () = msg_send![&bubble, setBorderType: 0isize];
        let _: () = msg_send![&bubble, setCornerRadius: 8.0f64];
        let _: () = msg_send![&bubble, setTitlePosition: 0isize];

        let _: () = msg_send![&bubble, setWantsLayer: objc2::runtime::Bool::YES];

        let color = if is_chat {
            if is_user { NSColor::systemBlueColor() } else { NSColor::systemGrayColor() }
        } else {
            NSColor::controlColor()
        };
        let _: () = msg_send![&bubble, setFillColor: &*color];

        let text_field: Allocated<NSTextField> = msg_send![NSTextField::class(), alloc];
        let text_field: Retained<NSTextField> = msg_send![text_field, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(100., 30.))];
        let _: () = msg_send![&text_field, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];

        text_field.setStringValue(&NSString::from_str(if is_long_message { &truncated_text } else { &full_text }));
        let _: () = msg_send![&text_field, setEditable: objc2::runtime::Bool::NO];
        let _: () = msg_send![&text_field, setSelectable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&text_field, setBordered: objc2::runtime::Bool::NO];
        let _: () = msg_send![&text_field, setDrawsBackground: objc2::runtime::Bool::NO];

        let text_color = if is_chat && is_user { NSColor::whiteColor() } else { NSColor::labelColor() };
        text_field.setTextColor(Some(&text_color));

        let cell: *mut objc2::runtime::AnyObject = msg_send![&text_field, cell];
        if !cell.is_null() {
            let _: () = msg_send![cell, setWraps: objc2::runtime::Bool::YES];
        }

        bubble.addSubview(&text_field);

        let mut expander_target: Option<Retained<BubbleExpanderTarget>> = None;
        let trailing_padding = if is_long_message { -32.0 } else { -8.0 };

        if is_long_message {
            let btn: Allocated<NSButton> = msg_send![NSButton::class(), alloc];
            let btn: Retained<NSButton> = msg_send![btn, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(20., 20.))];
            let _: () = msg_send![&btn, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
            let _: () = msg_send![&btn, setBordered: objc2::runtime::Bool::NO];
            let _: () = msg_send![&btn, setImagePosition: NSCellImagePosition::ImageOnly];
            let _: () = msg_send![&btn, setImageScaling: NSImageScaling::ScaleProportionallyUpOrDown];

            let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("chevron.down"), None
            );
            let _: () = msg_send![&btn, setImage: img.as_deref()];

            let target: Allocated<BubbleExpanderTarget> = msg_send![BubbleExpanderTarget::class(), alloc];
            let target: Retained<BubbleExpanderTarget> = msg_send![target, init];

            *target.ivars().full_text.borrow_mut() = full_text.clone();
            *target.ivars().truncated_text.borrow_mut() = truncated_text.clone();
            *target.ivars().text_field.borrow_mut() = Some(text_field.clone());
            *target.ivars().button.borrow_mut() = Some(btn.clone());

            let sel = objc2::sel!(toggle:);
            let _: () = msg_send![&btn, setTarget: &*target];
            let _: () = msg_send![&btn, setAction: sel];

            bubble.addSubview(&btn);

            let btn_constraints = NSArray::from_slice(&[
                &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                    &btn, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                    Some(&bubble), NSLayoutAttribute::Top, 1.0, 4.0
                ),
                &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                    &btn, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                    Some(&bubble), NSLayoutAttribute::Trailing, 1.0, -4.0
                ),
                &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                    &btn, NSLayoutAttribute::Width, NSLayoutRelation::Equal,
                    None, NSLayoutAttribute::NotAnAttribute, 1.0, 20.0
                ),
                &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                    &btn, NSLayoutAttribute::Height, NSLayoutRelation::Equal,
                    None, NSLayoutAttribute::NotAnAttribute, 1.0, 20.0
                ),
            ]);
            let _: () = msg_send![&bubble, addConstraints: &*btn_constraints];

            expander_target = Some(target);
        }

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
                Some(&bubble), NSLayoutAttribute::Trailing, 1.0, trailing_padding
            ),
        ]);
        let _: () = msg_send![&bubble, addConstraints: &*internal_constraints];

        stack_view.addView_inGravity(&bubble, NSStackViewGravity::Top);

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

        let single_x = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &bubble, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
            Some(&doc_view_nsview), NSLayoutAttribute::Leading, 1.0, 16.0
        );
        let single_max_width = NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &bubble, NSLayoutAttribute::Width, NSLayoutRelation::LessThanOrEqual,
            Some(&doc_view_nsview), NSLayoutAttribute::Width, 1.0, -32.0
        );
        let single_column_constraints = NSArray::from_slice(&[&*single_max_width, &*single_x]);

        let state = BubbleLayoutState {
            is_user,
            staggered_constraints,
            single_column_constraints,
            expander_target,
        };
        doc_view.ivars().bubbles.borrow_mut().push(state);

        let currently_single_column = doc_view.ivars().is_single_column.get();
        if currently_single_column {
            NSLayoutConstraint::activateConstraints(&doc_view.ivars().bubbles.borrow().last().unwrap().single_column_constraints);
        } else {
            NSLayoutConstraint::activateConstraints(&doc_view.ivars().bubbles.borrow().last().unwrap().staggered_constraints);
        }

        Retained::cast_unchecked::<NSView>(bubble)
    }
}
