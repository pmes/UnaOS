// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use objc2::rc::{Allocated, Retained};
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSResponder, NSTextView, NSTextField, NSTextViewDelegate, NSTextDelegate,
    NSView, NSLayoutConstraint, NSLayoutAttribute, NSLayoutRelation,
    NSColor, NSTableView, NSTableViewDataSource, NSTableViewDelegate,
    NSTableColumn, NSTableCellView, NSControlTextEditingDelegate,
    NSBox, NSStackView, NSButton, NSImage, NSImageScaling, NSCellImagePosition
};
use objc2_foundation::{
    NSObjectProtocol, NSRect, NSPoint, NSSize, NSArray,
    NSString, NSInteger, NSRange, NSMutableAttributedString, NSAttributedString,
};
use std::cell::RefCell;
use bandy::state::HistoryItem;

// -----------------------------------------------------------------------------
// CHAT BOX MANAGER DELEGATE (macOS)
// -----------------------------------------------------------------------------
pub struct ChatBoxManagerIvars {
    pub table_view: RefCell<Option<Retained<NSTableView>>>,
    pub history: RefCell<Vec<HistoryItem>>,
    pub active_text_view: RefCell<Option<Retained<NSTextField>>>,
    pub expanded_rows: RefCell<std::collections::HashSet<usize>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaChatBoxManager"]
    #[ivars = ChatBoxManagerIvars]
    pub struct ChatBoxManager;

    impl ChatBoxManager {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(ChatBoxManagerIvars {
                table_view: RefCell::new(None),
                history: RefCell::new(Vec::new()),
                active_text_view: RefCell::new(None),
                expanded_rows: RefCell::new(std::collections::HashSet::new()),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    // --- NSTableViewDataSource ---
    unsafe impl NSTableViewDataSource for ChatBoxManager {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows_in_table_view(&self, _table_view: &NSTableView) -> NSInteger {
            self.ivars().history.borrow().len() as NSInteger
        }
    }

    // --- NSTableViewDelegate ---
    unsafe impl NSTableViewDelegate for ChatBoxManager {
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

                let is_user = item.sender == "Architect";

                if cell.is_none() {
                    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 50.0)); // Initial approximate size
                    let new_cell: Allocated<NSTableCellView> = unsafe { msg_send![NSTableCellView::class(), alloc] };
                    let new_cell: Retained<NSTableCellView> = unsafe { msg_send![new_cell, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&new_cell, setIdentifier: &*identifier];
                    }

                    // Root container (NSView)
                    let root_view: Allocated<NSView> = unsafe { msg_send![NSView::class(), alloc] };
                    let root_view: Retained<NSView> = unsafe { msg_send![root_view, initWithFrame: frame] };
                    unsafe { let _: () = msg_send![&root_view, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO]; }

                    // Bubble Box (NSBox)
                    let bubble_box: Allocated<NSBox> = unsafe { msg_send![NSBox::class(), alloc] };
                    let bubble_box: Retained<NSBox> = unsafe { msg_send![bubble_box, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&bubble_box, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&bubble_box, setBoxType: 4isize]; // NSBoxCustom
                        let _: () = msg_send![&bubble_box, setBorderType: 0isize]; // NSNoBorder
                        let _: () = msg_send![&bubble_box, setTransparent: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&bubble_box, setWantsLayer: objc2::runtime::Bool::YES];
                        let _: () = msg_send![&bubble_box, setCornerRadius: 8.0f64];
                    }

                    // Bubble content StackView
                    let content_stack: Allocated<NSStackView> = unsafe { msg_send![NSStackView::class(), alloc] };
                    let content_stack: Retained<NSStackView> = unsafe { msg_send![content_stack, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&content_stack, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&content_stack, setOrientation: 1isize]; // Vertical
                        let _: () = msg_send![&content_stack, setSpacing: 4.0f64];
                        let _: () = msg_send![&content_stack, setAlignment: objc2_app_kit::NSLayoutAttribute::NotAnAttribute]; // Handled by constraints
                    }

                    // Header Box
                    let header_box: Allocated<NSStackView> = unsafe { msg_send![NSStackView::class(), alloc] };
                    let header_box: Retained<NSStackView> = unsafe { msg_send![header_box, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&header_box, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&header_box, setOrientation: 0isize]; // Horizontal
                        let _: () = msg_send![&header_box, setSpacing: 8.0f64];
                        let _: () = msg_send![&header_box, setAlignment: objc2_app_kit::NSLayoutAttribute::CenterY];
                    }

                    // Expander Button
                    let expander_btn: Allocated<NSButton> = unsafe { msg_send![NSButton::class(), alloc] };
                    let expander_btn: Retained<NSButton> = unsafe { msg_send![expander_btn, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&expander_btn, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&expander_btn, setBordered: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&expander_btn, setImagePosition: NSCellImagePosition::ImageOnly];
                        let _: () = msg_send![&expander_btn, setImageScaling: NSImageScaling::ScaleProportionallyUpOrDown];

                        // Target action to toggle expansion
                        let action = objc2::sel!(toggleExpansion:);
                        let _: () = msg_send![&expander_btn, setTarget: self];
                        let _: () = msg_send![&expander_btn, setAction: action];
                    }

                    // Meta label
                    let meta_label: Allocated<NSTextField> = unsafe { msg_send![NSTextField::class(), alloc] };
                    let meta_label: Retained<NSTextField> = unsafe { msg_send![meta_label, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&meta_label, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&meta_label, setBordered: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&meta_label, setDrawsBackground: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&meta_label, setEditable: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&meta_label, setSelectable: objc2::runtime::Bool::NO];

                        let dim_color: Retained<NSColor> = msg_send![NSColor::class(), secondaryLabelColor];
                        let _: () = msg_send![&meta_label, setTextColor: &*dim_color];

                        let font: Retained<objc2_app_kit::NSFont> = msg_send![objc2_app_kit::NSFont::class(), systemFontOfSize: 11.0, weight: objc2_app_kit::NSFontWeightRegular];
                        let _: () = msg_send![&meta_label, setFont: &*font];
                    }

                    // Text Field for content
                    let text_field: Allocated<NSTextField> = unsafe { msg_send![NSTextField::class(), alloc] };
                    let text_field: Retained<NSTextField> = unsafe { msg_send![text_field, initWithFrame: frame] };
                    unsafe {
                        let _: () = msg_send![&text_field, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&text_field, setDrawsBackground: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&text_field, setBordered: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&text_field, setEditable: objc2::runtime::Bool::NO];
                        let _: () = msg_send![&text_field, setSelectable: objc2::runtime::Bool::YES];

                        let _: () = msg_send![&text_field, setContentCompressionResistancePriority: 250.0f32, forOrientation: objc2_app_kit::NSLayoutConstraintOrientation::Horizontal];

                        let cell_obj: *mut AnyObject = msg_send![&text_field, cell];
                        if !cell_obj.is_null() {
                            let _: () = msg_send![cell_obj, setWraps: objc2::runtime::Bool::YES];
                            let _: () = msg_send![cell_obj, setLineBreakMode: 0isize]; // NSLineBreakByWordWrapping
                        }
                    }

                    // Assembly
                    unsafe {
                        let _: () = msg_send![&header_box, addView: &*expander_btn, inGravity: 1isize]; // NSStackViewGravityLeading
                        let _: () = msg_send![&header_box, addView: &*meta_label, inGravity: 1isize];

                        let _: () = msg_send![&content_stack, addView: &*header_box, inGravity: 1isize]; // Top
                        let _: () = msg_send![&content_stack, addView: &*text_field, inGravity: 1isize];

                        bubble_box.addSubview(&content_stack);
                        root_view.addSubview(&bubble_box);
                        new_cell.addSubview(&root_view);
                    }

                    let constraints = unsafe {
                        NSArray::from_slice(&[
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &root_view, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                                Some(&new_cell), NSLayoutAttribute::Top, 1.0, 4.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &root_view, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                                Some(&new_cell), NSLayoutAttribute::Bottom, 1.0, -4.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &root_view, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                                Some(&new_cell), NSLayoutAttribute::Leading, 1.0, 16.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &root_view, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                                Some(&new_cell), NSLayoutAttribute::Trailing, 1.0, -16.0
                            ),

                            // Bubble constraints inside root
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &bubble_box, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                                Some(&root_view), NSLayoutAttribute::Top, 1.0, 0.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &bubble_box, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                                Some(&root_view), NSLayoutAttribute::Bottom, 1.0, 0.0
                            ),

                            // Let the width be bounded by the root view
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &bubble_box, NSLayoutAttribute::Width, NSLayoutRelation::LessThanOrEqual,
                                Some(&root_view), NSLayoutAttribute::Width, 0.8, 0.0 // Max 80% width
                            ),

                            // Content Stack inside Bubble
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &content_stack, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                                Some(&bubble_box), NSLayoutAttribute::Top, 1.0, 8.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &content_stack, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                                Some(&bubble_box), NSLayoutAttribute::Bottom, 1.0, -8.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &content_stack, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                                Some(&bubble_box), NSLayoutAttribute::Leading, 1.0, 12.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &content_stack, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                                Some(&bubble_box), NSLayoutAttribute::Trailing, 1.0, -12.0
                            ),

                            // Expander button sizing
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &expander_btn, NSLayoutAttribute::Width, NSLayoutRelation::Equal,
                                None, NSLayoutAttribute::NotAnAttribute, 1.0, 16.0
                            ),
                            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                                &expander_btn, NSLayoutAttribute::Height, NSLayoutRelation::Equal,
                                None, NSLayoutAttribute::NotAnAttribute, 1.0, 16.0
                            ),
                        ])
                    };
                    NSLayoutConstraint::activateConstraints(&constraints);

                    cell = Some(new_cell);
                }

                let cell = cell.unwrap();

                // Safe subview iteration to find the components
                let root_views: Retained<NSArray<NSView>> = cell.subviews();
                let mut found_root = None;
                for i in 0..root_views.len() {
                    let subview = root_views.objectAtIndex(i);
                    // It's just NSView
                    found_root = Some(subview);
                    break;
                }

                let root_view = found_root.unwrap();
                let bubble_views: Retained<NSArray<NSView>> = root_view.subviews();
                let mut found_bubble = None;
                for i in 0..bubble_views.len() {
                    let subview = bubble_views.objectAtIndex(i);
                    if let Ok(b) = subview.downcast::<NSBox>() {
                        found_bubble = Some(b);
                        break;
                    }
                }
                let bubble_box = found_bubble.unwrap();

                let stack_views: Retained<NSArray<NSView>> = bubble_box.subviews();
                let content_stack = stack_views.objectAtIndex(0).downcast::<NSStackView>().unwrap();

                let stack_subviews: Retained<NSArray<NSView>> = content_stack.subviews();
                let header_box = stack_subviews.objectAtIndex(0).downcast::<NSStackView>().unwrap();
                let text_field = stack_subviews.objectAtIndex(1).downcast::<NSTextField>().unwrap();

                let header_subviews: Retained<NSArray<NSView>> = header_box.subviews();
                let expander_btn = header_subviews.objectAtIndex(0).downcast::<NSButton>().unwrap();
                let meta_label = header_subviews.objectAtIndex(1).downcast::<NSTextField>().unwrap();

                // Keep track of the active text field for streaming if this is the last cell
                if row == (history.len() - 1) as NSInteger {
                    *self.ivars().active_text_view.borrow_mut() = Some(text_field.clone());
                }

                // Apply styles based on sender
                let bg_color: Retained<NSColor> = if is_user {
                    unsafe { msg_send![NSColor::class(), controlAccentColor] } // Blueish
                } else {
                    unsafe { msg_send![NSColor::class(), windowBackgroundColor] } // Darker grey
                };
                unsafe { let _: () = msg_send![&bubble_box, setFillColor: &*bg_color]; }

                // Align Bubble Layout Left/Right
                unsafe {
                    // Remove old leading/trailing alignment constraints on bubble relative to root
                    let existing_constraints: Retained<NSArray<NSLayoutConstraint>> = root_view.constraints();
                    let mut constraints_to_keep = Vec::new();
                    let mut old_align = None;

                    for i in 0..existing_constraints.len() {
                        let c = existing_constraints.objectAtIndex(i);
                        let first_attr: NSLayoutAttribute = msg_send![&c, firstAttribute];
                        if first_attr == NSLayoutAttribute::Leading || first_attr == NSLayoutAttribute::Trailing {
                            // Check if it involves bubble_box and root_view
                            let first_item: *mut AnyObject = msg_send![&c, firstItem];
                            let second_item: *mut AnyObject = msg_send![&c, secondItem];
                            let bubble_ptr = Retained::as_ptr(&bubble_box) as *mut AnyObject;

                            if first_item == bubble_ptr || second_item == bubble_ptr {
                                old_align = Some(c.clone());
                                continue;
                            }
                        }
                        constraints_to_keep.push(c.clone());
                    }

                    if let Some(old_c) = old_align {
                        let old_array = NSArray::from_slice(&[&*old_c]);
                        NSLayoutConstraint::deactivateConstraints(&old_array);
                    }

                    let new_align = if is_user {
                        NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                            &bubble_box, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                            Some(&root_view), NSLayoutAttribute::Trailing, 1.0, 0.0
                        )
                    } else {
                        NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                            &bubble_box, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                            Some(&root_view), NSLayoutAttribute::Leading, 1.0, 0.0
                        )
                    };

                    let new_array = NSArray::from_slice(&[&*new_align]);
                    NSLayoutConstraint::activateConstraints(&new_array);
                }

                // Set Metadata
                let meta_str = format!("{} • {}", item.sender, item.timestamp);
                unsafe { let _: () = msg_send![&meta_label, setStringValue: &*NSString::from_str(&meta_str)]; }

                // Text Content & Truncation
                let is_expanded = self.ivars().expanded_rows.borrow().contains(&(row as usize));
                let mut display_text = item.content.clone();

                let mut truncation_idx = None;
                if !is_expanded {
                    truncation_idx = gneiss_pal::types::calculate_truncation(&item.content, 7, 500);
                }

                if let Some(idx) = truncation_idx {
                    display_text.truncate(idx);
                    display_text.push_str("\n...");
                }

                // Update text view
                let full_text = display_text;
                let ns_text = NSString::from_str(&full_text);

                unsafe {
                    let attr_string: Allocated<NSMutableAttributedString> = msg_send![NSMutableAttributedString::class(), alloc];
                    let attr_string: Retained<NSMutableAttributedString> = msg_send![attr_string, initWithString: &*ns_text];

                    let regular_font: Retained<objc2_app_kit::NSFont> = msg_send![objc2_app_kit::NSFont::class(), systemFontOfSize: 14.0, weight: objc2_app_kit::NSFontWeightRegular];
                    let text_color: Retained<NSColor> = if is_user {
                        msg_send![NSColor::class(), whiteColor] // White on accent color
                    } else {
                        msg_send![NSColor::class(), textColor]
                    };

                    let font_attr_name = &*objc2_app_kit::NSFontAttributeName;
                    let color_attr_name = &*objc2_app_kit::NSForegroundColorAttributeName;

                    let full_range = NSRange::new(0, full_text.encode_utf16().count());
                    let _: () = msg_send![&attr_string, addAttribute: font_attr_name, value: &*Retained::cast_unchecked::<AnyObject>(regular_font), range: full_range];
                    let _: () = msg_send![&attr_string, addAttribute: color_attr_name, value: &*Retained::cast_unchecked::<AnyObject>(text_color.clone()), range: full_range];

                    let _: () = msg_send![&text_field, setAttributedStringValue: &*attr_string];
                }

                // Handle Expander UI
                let needs_expander = gneiss_pal::types::calculate_truncation(&item.content, 7, 500).is_some();
                unsafe {
                    let _: () = msg_send![&expander_btn, setHidden: objc2::runtime::Bool::new(!needs_expander)];

                    // Store the row in the button tag so action handler knows what to toggle
                    let _: () = msg_send![&expander_btn, setTag: row];
                }

                if needs_expander {
                    let icon_name = if is_expanded { "chevron.up" } else { "chevron.down" };
                    unsafe {
                        let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                            &NSString::from_str(icon_name),
                            None
                        );
                        let _: () = msg_send![&expander_btn, setImage: img.as_deref()];
                    }
                }


                Some(unsafe { Retained::cast_unchecked::<NSView>(cell) })
            } else {
                None
            }
        }
    }

    // --- Action Handlers ---
    impl ChatBoxManager {
        #[unsafe(method(toggleExpansion:))]
        fn toggle_expansion(&self, sender: &AnyObject) {
            unsafe {
                let row: NSInteger = msg_send![sender, tag];
                let row_usize = row as usize;

                let mut expanded = self.ivars().expanded_rows.borrow_mut();
                if expanded.contains(&row_usize) {
                    expanded.remove(&row_usize);
                } else {
                    expanded.insert(row_usize);
                }

                if let Some(tv) = self.ivars().table_view.borrow().as_ref() {
                    let index_set: Retained<objc2_foundation::NSIndexSet> = msg_send![objc2_foundation::NSIndexSet::class(), indexSetWithIndex: row as objc2_foundation::NSUInteger];
                    let _: () = msg_send![tv, noteHeightOfRowsWithIndexesChanged: &*index_set];
                    // Also reload to update content and icon
                    let col_index_set: Retained<objc2_foundation::NSIndexSet> = msg_send![objc2_foundation::NSIndexSet::class(), indexSetWithIndex: 0isize as objc2_foundation::NSUInteger];
                    let _: () = msg_send![tv, reloadDataForRowIndexes: &*index_set, columnIndexes: &*col_index_set];
                }
            }
        }
    }

    // --- NSTextViewDelegate ---
    unsafe impl NSTextViewDelegate for ChatBoxManager {
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

unsafe impl NSObjectProtocol for ChatBoxManager {}
unsafe impl NSTextDelegate for ChatBoxManager {}
unsafe impl NSControlTextEditingDelegate for ChatBoxManager {}

impl ChatBoxManager {
    pub fn append_stream_token(&self, token: &str) {
        if let Some(text_field) = self.ivars().active_text_view.borrow().as_ref() {
            unsafe {
                let current_attr_string: Retained<NSAttributedString> = msg_send![&**text_field, attributedStringValue];

                let mutable_attr_string: Allocated<NSMutableAttributedString> = msg_send![NSMutableAttributedString::class(), alloc];
                let mutable_attr_string: Retained<NSMutableAttributedString> = msg_send![mutable_attr_string, initWithAttributedString: &*current_attr_string];

                let token_ns = NSString::from_str(token);

                let regular_font: Retained<objc2_app_kit::NSFont> = msg_send![objc2_app_kit::NSFont::class(), systemFontOfSize: 14.0, weight: objc2_app_kit::NSFontWeightRegular];
                // Assuming stream token is AI response
                let text_color: Retained<NSColor> = msg_send![NSColor::class(), textColor];

                let font_attr_name = &*objc2_app_kit::NSFontAttributeName;
                let color_attr_name = &*objc2_app_kit::NSForegroundColorAttributeName;

                let token_mut_attr_string: Allocated<NSMutableAttributedString> = msg_send![NSMutableAttributedString::class(), alloc];
                let token_mut_attr_string: Retained<NSMutableAttributedString> = msg_send![token_mut_attr_string, initWithString: &*token_ns];

                let token_range = NSRange::new(0, token.encode_utf16().count());
                let _: () = msg_send![&token_mut_attr_string, addAttribute: font_attr_name, value: &*Retained::cast_unchecked::<AnyObject>(regular_font), range: token_range];
                let _: () = msg_send![&token_mut_attr_string, addAttribute: color_attr_name, value: &*Retained::cast_unchecked::<AnyObject>(text_color), range: token_range];

                let _: () = msg_send![&mutable_attr_string, appendAttributedString: &*token_mut_attr_string];

                let _: () = msg_send![&**text_field, setAttributedStringValue: &*mutable_attr_string];

                // If it's inside a scroll view or table, we might need to tell the table to update layouts
                // But typically NSTableView with automatic row heights picks up intrinsic size changes
                // on the next layout pass.
            }
        }
    }
}
