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
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSResponder, NSTextView, NSTextField, NSTextViewDelegate, NSTextDelegate,
    NSSplitView, NSSplitViewDelegate, NSScrollView, NSView,
    NSLayoutConstraint, NSLayoutAttribute, NSLayoutRelation,
    NSColor, NSTableView, NSTableViewDataSource, NSTableViewDelegate,
    NSTableColumn, NSTableCellView, NSControlTextEditingDelegate
};
use objc2_foundation::{
    NSObjectProtocol, NSRect, NSPoint, NSSize, MainThreadMarker, NSArray,
    NSString, NSInteger, NSRange, NSMutableAttributedString, NSAttributedString, NSDictionary
};
use std::cell::RefCell;
use std::sync::{Arc, RwLock};
use bandy::state::{AppState, HistoryItem};

// -----------------------------------------------------------------------------
// COMMS DELEGATE (LUMEN REACTOR CHAT)
// -----------------------------------------------------------------------------
pub struct CommsDelegateIvars {
    pub table_view: RefCell<Option<Retained<NSTableView>>>,
    pub history: RefCell<Vec<HistoryItem>>,
    pub active_text_view: RefCell<Option<Retained<NSTextField>>>,
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
                table_view: RefCell::new(None),
                history: RefCell::new(Vec::new()),
                active_text_view: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    // --- NSTableViewDataSource ---
    unsafe impl NSTableViewDataSource for CommsDelegate {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows_in_table_view(&self, _table_view: &NSTableView) -> NSInteger {
            self.ivars().history.borrow().len() as NSInteger
        }
    }

    // --- NSTableViewDelegate ---
    unsafe impl NSTableViewDelegate for CommsDelegate {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn table_view_view_for_table_column_row(
            &self,
            table_view: &NSTableView,
            _table_column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            let history = self.ivars().history.borrow();

            if let Some(item) = history.get(row as usize) {
                let identifier = NSString::from_str("ChatBubbleCell");
                let mut cell: Option<Retained<NSTableCellView>> = unsafe {
                    let recycled: *mut AnyObject = msg_send![table_view, makeViewWithIdentifier: &*identifier, owner: self];
                    if !recycled.is_null() {
                        Some(Retained::cast_unchecked::<NSTableCellView>(Retained::retain(recycled).unwrap()))
                    } else {
                        None
                    }
                };

                if cell.is_none() {
                    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 50.0)); // Initial approximate size
                    let new_cell: Allocated<NSTableCellView> = unsafe { msg_send![NSTableCellView::class(), alloc] };
                    let new_cell: Retained<NSTableCellView> = unsafe { msg_send![new_cell, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&new_cell, setIdentifier: &*identifier];
                    }

                    // Create NSTextField for the bubble content to enable Auto Layout intrinsic sizing
                    let text_field: Allocated<NSTextField> = unsafe { msg_send![NSTextField::class(), alloc] };
                    let text_field: Retained<NSTextField> = unsafe { msg_send![text_field, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&text_field, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&text_field, setDrawsBackground: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&text_field, setBordered: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&text_field, setEditable: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&text_field, setSelectable: objc2::runtime::Bool::YES];

                        // Lower horizontal compression resistance to allow wrapping
                        let _: () = msg_send![&text_field, setContentCompressionResistancePriority: 250.0f32, forOrientation: objc2_app_kit::NSLayoutConstraintOrientation::Horizontal];

                        // Enable wrapping on its cell
                        let cell_obj: *mut AnyObject = msg_send![&text_field, cell];
                        if !cell_obj.is_null() {
                            let _: () = msg_send![cell_obj, setWraps: objc2::runtime::Bool::YES];
                            let _: () = msg_send![cell_obj, setLineBreakMode: 0isize]; // NSLineBreakByWordWrapping
                        }
                    }

                    new_cell.addSubview(&text_field);

                    // Anchor text field to cell
                    let constraints = unsafe {
                        NSArray::from_slice(&[
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &text_field, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                                Some(&new_cell), NSLayoutAttribute::Top, 1.0, 8.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &text_field, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                                Some(&new_cell), NSLayoutAttribute::Bottom, 1.0, -8.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &text_field, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                                Some(&new_cell), NSLayoutAttribute::Leading, 1.0, 16.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &text_field, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                                Some(&new_cell), NSLayoutAttribute::Trailing, 1.0, -16.0
                            ),
                        ])
                    };
                    NSLayoutConstraint::activateConstraints(&constraints);

                    cell = Some(new_cell);
                }

                let cell = cell.unwrap();

                // Safe subview iteration to find the NSTextField
                let subviews: Retained<NSArray<NSView>> = cell.subviews();
                let mut found_text_field = None;

                for i in 0..subviews.len() {
                    let subview = subviews.objectAtIndex(i);
                    if let Ok(text_field) = subview.downcast::<NSTextField>() {
                        found_text_field = Some(text_field);
                        break;
                    }
                }

                let text_field = found_text_field.expect("NSTextField must exist in ChatBubbleCell");

                // Keep track of the active text field for streaming if this is the last cell
                if row == (history.len() - 1) as NSInteger {
                    *self.ivars().active_text_view.borrow_mut() = Some(text_field.clone());
                }

                // Format string appropriately based on sender using semantic typography
                let prefix = format!("{}:\n", item.sender);
                let full_text = format!("{}{}", prefix, item.content);
                let ns_text = NSString::from_str(&full_text);

                unsafe {
                    let attr_string: Allocated<NSMutableAttributedString> = msg_send![NSMutableAttributedString::class(), alloc];
                    let attr_string: Retained<NSMutableAttributedString> = msg_send![attr_string, initWithString: &*ns_text];

                    let regular_font: Retained<objc2_app_kit::NSFont> = msg_send![objc2_app_kit::NSFont::class(), systemFontOfSize: 14.0 weight: objc2_app_kit::NSFontWeightRegular];
                    let bold_font: Retained<objc2_app_kit::NSFont> = msg_send![objc2_app_kit::NSFont::class(), systemFontOfSize: 14.0 weight: objc2_app_kit::NSFontWeightBold];
                    let text_color = NSColor::textColor();

                    let bold_attrs: Retained<NSDictionary<objc2_app_kit::NSAttributedStringKey, AnyObject>> = NSDictionary::from_keys_and_objects(
                        &[
                            &*objc2_app_kit::NSFontAttributeName,
                            &*objc2_app_kit::NSForegroundColorAttributeName
                        ],
                        &[
                            &*Retained::cast::<AnyObject>(bold_font),
                            &*Retained::cast::<AnyObject>(text_color.clone())
                        ]
                    );

                    let regular_attrs: Retained<NSDictionary<objc2_app_kit::NSAttributedStringKey, AnyObject>> = NSDictionary::from_keys_and_objects(
                        &[
                            &*objc2_app_kit::NSFontAttributeName,
                            &*objc2_app_kit::NSForegroundColorAttributeName
                        ],
                        &[
                            &*Retained::cast::<AnyObject>(regular_font),
                            &*Retained::cast::<AnyObject>(text_color)
                        ]
                    );

                    // Apply regular font to whole string
                    let full_range = NSRange::new(0, full_text.encode_utf16().count());
                    let _: () = msg_send![&attr_string, setAttributes: &*regular_attrs range: full_range];

                    // Apply bold font to sender
                    let prefix_range = NSRange::new(0, prefix.encode_utf16().count());
                    let _: () = msg_send![&attr_string, setAttributes: &*bold_attrs range: prefix_range];

                    let _: () = msg_send![&text_field, setAttributedStringValue: &*attr_string];
                }

                Some(unsafe { Retained::cast_unchecked::<NSView>(cell) })
            } else {
                None
            }
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
unsafe impl NSControlTextEditingDelegate for CommsDelegate {}

impl CommsDelegate {
    pub fn append_stream_token(&self, token: &str) {
        if let Some(text_field) = self.ivars().active_text_view.borrow().as_ref() {
            unsafe {
                let current_attr_string: Retained<NSAttributedString> = msg_send![&**text_field, attributedStringValue];

                let mutable_attr_string: Allocated<NSMutableAttributedString> = msg_send![NSMutableAttributedString::class(), alloc];
                let mutable_attr_string: Retained<NSMutableAttributedString> = msg_send![mutable_attr_string, initWithAttributedString: &*current_attr_string];

                let token_ns = NSString::from_str(token);

                let regular_font: Retained<objc2_app_kit::NSFont> = msg_send![objc2_app_kit::NSFont::class(), systemFontOfSize: 14.0 weight: objc2_app_kit::NSFontWeightRegular];
                let text_color = NSColor::textColor();

                let regular_attrs: Retained<NSDictionary<objc2_app_kit::NSAttributedStringKey, AnyObject>> = NSDictionary::from_keys_and_objects(
                    &[
                        &*objc2_app_kit::NSFontAttributeName,
                        &*objc2_app_kit::NSForegroundColorAttributeName
                    ],
                    &[
                        &*Retained::cast::<AnyObject>(regular_font),
                        &*Retained::cast::<AnyObject>(text_color)
                    ]
                );

                let token_attr_string: Allocated<NSAttributedString> = msg_send![NSAttributedString::class(), alloc];
                let token_attr_string: Retained<NSAttributedString> = msg_send![token_attr_string, initWithString: &*token_ns attributes: &*regular_attrs];

                let _: () = msg_send![&mutable_attr_string, appendAttributedString: &*token_attr_string];

                let _: () = msg_send![&**text_field, setAttributedStringValue: &*mutable_attr_string];

                // If it's inside a scroll view or table, we might need to tell the table to update layouts
                // But typically NSTableView with automatic row heights picks up intrinsic size changes
                // on the next layout pass.
            }
        }
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

    // 3. Top Split: Bubble Matrix Placeholder (NSScrollView & NSTableView)
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

    let table_view: Allocated<NSTableView> = unsafe { msg_send![NSTableView::class(), alloc] };
    let table_view: Retained<NSTableView> = unsafe { msg_send![table_view, initWithFrame: frame] };
    unsafe {
        // Use automatic row heights to avoid clipping
        let _: () = msg_send![&table_view, setUsesAutomaticRowHeights: objc2::runtime::Bool::YES];
        let _: () = msg_send![&table_view, setSelectionHighlightStyle: -1isize]; // NSTableViewSelectionHighlightStyleNone
        let _: () = msg_send![&table_view, setHeaderView: None::<&AnyObject>];
        let clear_color = NSColor::clearColor();
        let _: () = msg_send![&table_view, setBackgroundColor: &*clear_color];

        table_view.setDataSource(Some(ProtocolObject::from_ref(&*delegate)));
        table_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        // Let table view handle column sizing uniformly
        let _: () = msg_send![&table_view, setColumnAutoresizingStyle: 1isize]; // NSTableViewUniformColumnAutoresizingStyle

        // Create the main column
        let column: Allocated<NSTableColumn> = msg_send![NSTableColumn::class(), alloc];
        let column_id = NSString::from_str("ChatColumn");
        let column: Retained<NSTableColumn> = msg_send![column, initWithIdentifier: &*column_id];
        // Ensure column stretches to width of table view
        let _: () = msg_send![&column, setResizingMask: 1isize]; // NSTableColumnAutoresizingMask
        // Hide the column title since we disabled the header view
        table_view.addTableColumn(&column);
    }

    // Anchor NSTableView into NSScrollView
    matrix_scroll.setDocumentView(Some(&table_view));

    // Anchor views into delegate
    *delegate.ivars().table_view.borrow_mut() = Some(table_view.clone());

    // Load historical messages from AppState
    let history_items = {
        let state = app_state.read().unwrap();
        state.history.iter()
            .filter(|item| item.is_chat)
            .cloned()
            .collect::<Vec<_>>()
    };

    println!("[MATRIX] Booting with {} historical messages", history_items.len());
    *delegate.ivars().history.borrow_mut() = history_items;

    unsafe {
        let _: () = msg_send![&table_view, reloadData];
    }

    // Add it to the split view
    split_view.addSubview(&matrix_scroll);

    // 4. Bottom Split: Input Buffer
    let input_scroll: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let input_scroll: Retained<NSScrollView> = unsafe { msg_send![input_scroll, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&input_scroll, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        let _: () = msg_send![&input_scroll, setBorderType: 2isize]; // NSBezelBorder
        let _: () = msg_send![&input_scroll, setDrawsBackground: objc2::runtime::Bool::YES];
    }
    input_scroll.setHasVerticalScroller(true);
    input_scroll.setHasHorizontalScroller(false);
    input_scroll.setAutohidesScrollers(true);

    let input_height_constraint = unsafe {
        NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            &input_scroll, NSLayoutAttribute::Height, NSLayoutRelation::Equal,
            None, NSLayoutAttribute::NotAnAttribute, 1.0, 32.0
        )
    };
    let input_height_array = NSArray::from_slice(&[&*input_height_constraint]);
    NSLayoutConstraint::activateConstraints(&input_height_array);

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
    let input_container: Allocated<NSView> = unsafe { msg_send![NSView::class(), alloc] };
    let input_container: Retained<NSView> = unsafe { msg_send![input_container, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&input_container, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }

    input_container.addSubview(&attach_btn);
    input_container.addSubview(&input_scroll);
    input_container.addSubview(&send_btn);

    // Convert NSStackView layout to standard auto layout constraints
    let input_constraints = unsafe {
        NSArray::from_slice(&[
            // Attach button to the left
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &attach_btn, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&input_container), NSLayoutAttribute::Leading, 1.0, 8.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &attach_btn, NSLayoutAttribute::CenterY, NSLayoutRelation::Equal,
                Some(&input_container), NSLayoutAttribute::CenterY, 1.0, 0.0
            ),
            // Input scroll view next to attach button
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_scroll, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&attach_btn), NSLayoutAttribute::Trailing, 1.0, 8.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_scroll, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                Some(&input_container), NSLayoutAttribute::Top, 1.0, 8.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_scroll, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                Some(&input_container), NSLayoutAttribute::Bottom, 1.0, -8.0
            ),
            // Send button next to input scroll view
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &send_btn, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&input_scroll), NSLayoutAttribute::Trailing, 1.0, 8.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &send_btn, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                Some(&input_container), NSLayoutAttribute::Trailing, 1.0, -8.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &send_btn, NSLayoutAttribute::CenterY, NSLayoutRelation::Equal,
                Some(&input_container), NSLayoutAttribute::CenterY, 1.0, 0.0
            ),
        ])
    };
    NSLayoutConstraint::activateConstraints(&input_constraints);

    split_view.addSubview(&input_container);

    // The SplitView will manage sizing the two scroll views.
    let constraints = unsafe {
        NSArray::from_slice(&[
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &matrix_scroll, NSLayoutAttribute::Height, NSLayoutRelation::GreaterThanOrEqual,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 150.0 // Minimum 150px chat height
            )
        ])
    };
    unsafe {
        let _: () = msg_send![&split_view, addConstraints: &*constraints];

        let _: () = msg_send![&split_view, setHoldingPriority: 250.0f32, forSubviewAtIndex: 0isize];
        let _: () = msg_send![&split_view, setHoldingPriority: 750.0f32, forSubviewAtIndex: 1isize];
    }

    (unsafe { Retained::cast_unchecked::<NSView>(split_view) }, delegate)
}
