// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The macOS AppKit **Console** pane: a read-only, monospace `NSTextView`
//! (output) stacked over a one-line `NSTextField` (input). It is the bottom-pane
//! counterpart to the editor/comms right panes. `MacOSSpline` builds it when the
//! workspace's `bottom_pane` is `Some`, stacks it under the right pane in a
//! vertical split, routes `SMessage::ConsoleAppend` into it (via `append_line`),
//! and receives `SMessage::ConsoleInput` from it on the input field's Enter.

use objc2::rc::{Allocated, Retained};
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSResponder, NSTextView, NSTextViewDelegate, NSTextDelegate,
    NSScrollView, NSView, NSColor, NSFont, NSTextField,
    NSControlTextEditingDelegate, NSLayoutConstraint, NSLayoutAttribute,
    NSLayoutRelation,
};
use objc2_foundation::{
    NSObjectProtocol, NSRect, NSPoint, NSSize, MainThreadMarker, NSString, NSArray,
};
use std::cell::RefCell;
use bandy::SMessage;

// -----------------------------------------------------------------------------
// CONSOLE DELEGATE (BOTTOM PANE)
// -----------------------------------------------------------------------------
pub struct ConsoleDelegateIvars {
    pub output_view: RefCell<Option<Retained<NSTextView>>>,
    pub input_field: RefCell<Option<Retained<NSTextField>>>,
    /// UI → brain sender: the input field fires `ConsoleInput` on Enter.
    pub tx_event: RefCell<Option<async_channel::Sender<SMessage>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaConsoleDelegate"]
    #[ivars = ConsoleDelegateIvars]
    pub struct ConsoleDelegate;

    impl ConsoleDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(ConsoleDelegateIvars {
                output_view: RefCell::new(None),
                input_field: RefCell::new(None),
                tx_event: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        /// Target/action for the input `NSTextField`. AppKit sends this when the
        /// user presses Enter in the field. We forward the line as `ConsoleInput`
        /// and clear the field.
        #[unsafe(method(consoleSubmit:))]
        fn console_submit(&self, _sender: Option<&objc2::runtime::AnyObject>) {
            let line = self
                .ivars()
                .input_field
                .borrow()
                .as_ref()
                .map(|f| {
                    let s: Retained<NSString> = unsafe { msg_send![&**f, stringValue] };
                    s.to_string()
                })
                .unwrap_or_default();

            if line.is_empty() {
                return;
            }

            if let Some(tx) = self.ivars().tx_event.borrow().as_ref() {
                let _ = tx.try_send(SMessage::ConsoleInput(line));
            }

            // Clear the field for the next command.
            if let Some(field) = self.ivars().input_field.borrow().as_ref() {
                let empty = NSString::from_str("");
                unsafe {
                    let _: () = msg_send![&**field, setStringValue: &*empty];
                }
            }
        }
    }

    unsafe impl NSTextViewDelegate for ConsoleDelegate {}
);

unsafe impl NSObjectProtocol for ConsoleDelegate {}
unsafe impl NSTextDelegate for ConsoleDelegate {}
unsafe impl NSControlTextEditingDelegate for ConsoleDelegate {}

impl ConsoleDelegate {
    /// Append one line to the read-only output view and scroll to the bottom.
    /// Must run on the main thread (the `MacOSSpline` router dispatches
    /// `ConsoleAppend` here via the main queue).
    pub fn append_line(&self, line: &str) {
        if let Some(text_view) = self.ivars().output_view.borrow().as_ref() {
            let existing: Retained<NSString> = unsafe { msg_send![&**text_view, string] };
            let combined = format!("{}{}\n", existing, line);
            let s = NSString::from_str(&combined);
            unsafe {
                let _: () = msg_send![&**text_view, setString: &*s];
                // Scroll to the end so the newest line is visible (length in
                // UTF-16 code units, which is what NSString/NSTextView expect).
                let len: usize = s.length();
                let end = objc2_foundation::NSRange { location: len, length: 0 };
                let _: () = msg_send![&**text_view, scrollRangeToVisible: end];
            }
        }
    }
}

// -----------------------------------------------------------------------------
// ASSEMBLY
// -----------------------------------------------------------------------------
/// Build the console pane view + its delegate. `tx_event` is the UI → brain
/// sender used by the input field's Enter action.
pub fn create_console(
    _mtm: MainThreadMarker,
    tx_event: async_channel::Sender<SMessage>,
) -> (Retained<NSView>, Retained<ConsoleDelegate>) {
    // 1. Instantiate the delegate.
    let delegate: Allocated<ConsoleDelegate> = unsafe { msg_send![ConsoleDelegate::class(), alloc] };
    let delegate: Retained<ConsoleDelegate> = unsafe { msg_send![delegate, init] };
    *delegate.ivars().tx_event.borrow_mut() = Some(tx_event);

    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(774.0, 200.0));

    // 2. Container that stacks output (top) over the one-line input (bottom).
    let container: Allocated<NSView> = unsafe { msg_send![NSView::class(), alloc] };
    let container: Retained<NSView> = unsafe { msg_send![container, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&container, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
    }

    // 3. Output scroll view + read-only monospace text view.
    let scroll: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let scroll: Retained<NSScrollView> = unsafe { msg_send![scroll, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&scroll, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(false);
        scroll.setAutohidesScrollers(true);
        let _: () = msg_send![&scroll, setDrawsBackground: objc2::runtime::Bool::YES];
    }

    let text_view: Allocated<NSTextView> = unsafe { msg_send![NSTextView::class(), alloc] };
    let text_view: Retained<NSTextView> = unsafe { msg_send![text_view, initWithFrame: frame] };
    unsafe {
        // Read-only output surface.
        let _: () = msg_send![&text_view, setEditable: objc2::runtime::Bool::NO];
        let _: () = msg_send![&text_view, setSelectable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&text_view, setRichText: objc2::runtime::Bool::NO];

        let font = NSFont::userFixedPitchFontOfSize(12.0);
        if let Some(font) = font {
            let _: () = msg_send![&text_view, setFont: &*font];
        }
        let bg = NSColor::textBackgroundColor();
        let _: () = msg_send![&text_view, setBackgroundColor: &*bg];
        let fg = NSColor::textColor();
        let _: () = msg_send![&text_view, setTextColor: &*fg];
    }
    text_view.setVerticallyResizable(true);
    text_view.setHorizontallyResizable(false);
    scroll.setDocumentView(Some(&text_view));

    // 4. One-line input field, target/action → consoleSubmit: on Enter.
    let input_field: Allocated<NSTextField> = unsafe { msg_send![NSTextField::class(), alloc] };
    let input_field: Retained<NSTextField> = unsafe { msg_send![input_field, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&input_field, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        let _: () = msg_send![&input_field, setEditable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&input_field, setBordered: objc2::runtime::Bool::YES];
        let _: () = msg_send![&input_field, setBezeled: objc2::runtime::Bool::YES];
        let placeholder = NSString::from_str("›");
        let _: () = msg_send![&input_field, setPlaceholderString: &*placeholder];

        let font = NSFont::userFixedPitchFontOfSize(12.0);
        if let Some(font) = font {
            let _: () = msg_send![&input_field, setFont: &*font];
        }

        // Fire consoleSubmit: on Enter (target/action; no field delegate needed).
        let _: () = msg_send![&input_field, setTarget: &*delegate];
        let _: () = msg_send![&input_field, setAction: objc2::sel!(consoleSubmit:)];
    }

    // 5. Layout: input pinned to the bottom (fixed height), output fills above.
    container.addSubview(&scroll);
    container.addSubview(&input_field);

    let constraints = unsafe {
        NSArray::from_slice(&[
            // Output scroll: top + sides of container, bottom to input top.
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &scroll, NSLayoutAttribute::Top, NSLayoutRelation::Equal,
                Some(&container), NSLayoutAttribute::Top, 1.0, 0.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &scroll, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&container), NSLayoutAttribute::Leading, 1.0, 0.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &scroll, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                Some(&container), NSLayoutAttribute::Trailing, 1.0, 0.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &scroll, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                Some(&input_field), NSLayoutAttribute::Top, 1.0, -4.0
            ),
            // Input field: sides + bottom of container, fixed height.
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_field, NSLayoutAttribute::Leading, NSLayoutRelation::Equal,
                Some(&container), NSLayoutAttribute::Leading, 1.0, 4.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_field, NSLayoutAttribute::Trailing, NSLayoutRelation::Equal,
                Some(&container), NSLayoutAttribute::Trailing, 1.0, -4.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_field, NSLayoutAttribute::Bottom, NSLayoutRelation::Equal,
                Some(&container), NSLayoutAttribute::Bottom, 1.0, -4.0
            ),
            &*NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                &input_field, NSLayoutAttribute::Height, NSLayoutRelation::Equal,
                None, NSLayoutAttribute::NotAnAttribute, 1.0, 24.0
            ),
        ])
    };
    NSLayoutConstraint::activateConstraints(&constraints);

    // 6. Remember the pieces for later routing.
    *delegate.ivars().output_view.borrow_mut() = Some(text_view.clone());
    *delegate.ivars().input_field.borrow_mut() = Some(input_field.clone());

    (container, delegate)
}
