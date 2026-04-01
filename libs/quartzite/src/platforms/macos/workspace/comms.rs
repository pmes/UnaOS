// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSResponder, NSTextView, NSTextViewDelegate, NSTextDelegate,
    NSSplitView, NSSplitViewDelegate, NSScrollView, NSView,
    NSLayoutConstraint, NSLayoutAttribute, NSLayoutRelation,
    NSTextField, NSColor, NSStackView, NSStackViewGravity, NSBox
};
use objc2_foundation::{
    NSObjectProtocol, NSRect, NSPoint, NSSize, MainThreadMarker, NSArray,
    NSString, NSEdgeInsets
};
use std::cell::{Cell, RefCell};
use std::sync::{Arc, RwLock};
use bandy::state::AppState;

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
// COMMS DELEGATE (LUMEN REACTOR CHAT)
// -----------------------------------------------------------------------------
pub struct CommsDelegateIvars {
    pub doc_view: RefCell<Option<Retained<FlippedDocumentView>>>,
    pub stack_view: RefCell<Option<Retained<NSStackView>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaCommsDelegate"]
    #[ivars = CommsDelegateIvars]
    pub struct CommsDelegate;

    impl CommsDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(CommsDelegateIvars {
                doc_view: RefCell::new(None),
                stack_view: RefCell::new(None),
            });
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
            NSColor::windowBackgroundColor() // System messages get a dim/native background
        };
        let _: () = msg_send![&bubble, setFillColor: &*color];

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

// -----------------------------------------------------------------------------
// ASSEMBLY
// -----------------------------------------------------------------------------
pub fn create_comms(_mtm: MainThreadMarker, app_state: &Arc<RwLock<AppState>>) -> (Retained<NSView>, Retained<CommsDelegate>) {
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
        matrix_scroll.setHasVerticalScroller(true);
        matrix_scroll.setHasHorizontalScroller(false);
        matrix_scroll.setAutohidesScrollers(true);

        // Transparent Backgrounds for Comms
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

    // Anchor views into delegate
    *delegate.ivars().doc_view.borrow_mut() = Some(doc_view.clone());
    *delegate.ivars().stack_view.borrow_mut() = Some(stack_view.clone());

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

    // Load historical messages from AppState
    let history_items = {
        let state = app_state.read().unwrap();
        // Extract history items, filtering for chat messages to populate the matrix
        state.history.iter()
            .filter(|item| item.is_chat)
            .cloned()
            .collect::<Vec<_>>()
    }; // Drop read lock immediately before heavy UI layout loops

    println!("[MATRIX] Booting with {} historical messages", history_items.len());

    // Inject historical bubbles into the Matrix
    for item in history_items {
        let is_user = item.sender == "Architect";
        append_bubble(&doc_view, &stack_view, &item.content, &item.sender, "Chat", &item.timestamp, item.is_chat, is_user);
    }

    // Ensure the document view bounds the stack view at the bottom so it doesn't clip
    unsafe {
        let bottom_constraint = NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &doc_view, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                Some(&stack_view), NSLayoutAttribute::Bottom, 1.0, 16.0
            )
        ]);
        let _: () = msg_send![&doc_view, addConstraints: &*bottom_constraint];
    }

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

    // Explicitly lower the horizontal hugging priority of the scroll view so it expands to fill the space
    unsafe {
        // NSLayoutPriorityDefaultLow is 250.0
        let _: () = msg_send![&input_scroll, setContentHuggingPriority: 250.0f32, forOrientation: objc2_app_kit::NSLayoutConstraintOrientation::Horizontal];
    }

    // 5. The Symbols
    let attach_btn: Allocated<objc2_app_kit::NSButton> = unsafe { msg_send![objc2_app_kit::NSButton::class(), alloc] };
    let attach_btn: Retained<objc2_app_kit::NSButton> = unsafe { msg_send![attach_btn, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&attach_btn, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        let _: () = msg_send![&attach_btn, setBordered: objc2::runtime::Bool::NO];
        let _: () = msg_send![&attach_btn, setImagePosition: objc2_app_kit::NSCellImagePosition::ImageOnly];
        let _: () = msg_send![&attach_btn, setImageScaling: objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown];
        let img = objc2_app_kit::NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("plus"),
            None
        );
        let _: () = msg_send![&attach_btn, setImage: img.as_deref()];
    }

    let send_btn: Allocated<objc2_app_kit::NSButton> = unsafe { msg_send![objc2_app_kit::NSButton::class(), alloc] };
    let send_btn: Retained<objc2_app_kit::NSButton> = unsafe { msg_send![send_btn, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&send_btn, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        let _: () = msg_send![&send_btn, setBordered: objc2::runtime::Bool::NO];
        let _: () = msg_send![&send_btn, setImagePosition: objc2_app_kit::NSCellImagePosition::ImageOnly];
        let _: () = msg_send![&send_btn, setImageScaling: objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown];
        let img = objc2_app_kit::NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("arrow.up.message"),
            None
        );
        let _: () = msg_send![&send_btn, setImage: img.as_deref()];
    }

    // Force strict dimensions on the SF symbols so they don't shrink wrap
    let symbol_constraints = unsafe {
        NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &attach_btn, NSLayoutAttribute::Width, NSLayoutRelation::Equal,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 28.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &attach_btn, NSLayoutAttribute::Height, NSLayoutRelation::Equal,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 28.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &send_btn, NSLayoutAttribute::Width, NSLayoutRelation::Equal,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 28.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &send_btn, NSLayoutAttribute::Height, NSLayoutRelation::Equal,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 28.0
            ),
        ])
    };
    NSLayoutConstraint::activateConstraints(&symbol_constraints);

    // 6. The Input Horizontal Stack
    let input_stack: Allocated<NSStackView> = unsafe { msg_send![NSStackView::class(), alloc] };
    let input_stack: Retained<NSStackView> = unsafe { msg_send![input_stack, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&input_stack, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }
    input_stack.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Horizontal);
    input_stack.setSpacing(8.0);
    unsafe {
        let _: () = msg_send![&input_stack, setEdgeInsets: NSEdgeInsets { top: 8.0, left: 8.0, bottom: 8.0, right: 8.0 }];
    }

    // Order matters: Attachment Button, Input Buffer, Send Button
    input_stack.addView_inGravity(&attach_btn, NSStackViewGravity::Leading);
    input_stack.addView_inGravity(&input_scroll, NSStackViewGravity::Leading);
    input_stack.addView_inGravity(&send_btn, NSStackViewGravity::Leading);

    split_view.addSubview(&input_stack);

    // The SplitView will manage sizing the two scroll views.
    // The user can drag the horizontal divider.
    // Ensure the input stack doesn't collapse to 0:
    let constraints = unsafe {
        NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_stack, NSLayoutAttribute::Height, NSLayoutRelation::GreaterThanOrEqual,
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
