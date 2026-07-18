// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The macOS AppKit **Editor** pane: a monospace, editable `NSTextView` in a
//! scroll view. It is the right-pane counterpart to `comms.rs` for the `Code`
//! layout — same `NSScrollView` + `NSTextView` scaffolding, minus the chat
//! manager / table / input stack. `MacOSSpline` builds it when the workspace's
//! right pane is `ViewEntity::Editor` and routes `SMessage::EditorLoad` into it
//! via `set_content` (`setString:`).

use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, ClassType, DefinedClass};
use objc2_app_kit::{
    NSResponder, NSTextView, NSTextViewDelegate, NSTextDelegate,
    NSScrollView, NSView, NSColor, NSFont,
};
use objc2_foundation::{
    NSObjectProtocol, NSRect, NSPoint, NSSize, MainThreadMarker, NSString,
    NSNotification,
};
use std::cell::RefCell;
use bandy::state::EditorState;
use bandy::SMessage;

// -----------------------------------------------------------------------------
// EDITOR DELEGATE (CODE PANE)
// -----------------------------------------------------------------------------
pub struct EditorDelegateIvars {
    pub text_view: RefCell<Option<Retained<NSTextView>>>,
    /// UI → brain sender. The editor fires `EditorEdited` on every text change
    /// and `EditorSaveRequest` on Cmd+S / menu Save. Mirrors how the other
    /// delegates receive `tx_event` at construction (threaded in via
    /// `create_editor`).
    pub tx_event: RefCell<Option<async_channel::Sender<SMessage>>>,
}

define_class!(
    #[unsafe(super(NSResponder))]
    #[name = "UnaEditorDelegate"]
    #[ivars = EditorDelegateIvars]
    pub struct EditorDelegate;

    impl EditorDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(EditorDelegateIvars {
                text_view: RefCell::new(None),
                tx_event: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        /// Menu / Cmd+S action target. Wired by the main-menu "Save" item, whose
        /// target is set to this delegate after bootstrap. Fires `EditorSaveRequest`
        /// so the brain performs the actual write.
        #[unsafe(method(saveDocument:))]
        fn save_document(&self, _sender: Option<&objc2::runtime::AnyObject>) {
            self.fire(SMessage::EditorSaveRequest);
        }
    }

    // --- NSTextViewDelegate ---
    unsafe impl NSTextViewDelegate for EditorDelegate {
        #[unsafe(method(textView:doCommandBySelector:))]
        fn text_view_do_command_by_selector(
            &self,
            _text_view: &NSTextView,
            _command_selector: objc2::runtime::Sel,
        ) -> objc2::runtime::Bool {
            // Let the text view handle every command itself (default editing).
            objc2::runtime::Bool::NO
        }
    }

    // --- NSTextDelegate ---
    // `textDidChange:` is declared on NSTextDelegate (NSTextViewDelegate's
    // parent protocol); objc2 verifies the selector against the protocol the
    // impl block names, so it must live here, not in NSTextViewDelegate.
    unsafe impl NSTextDelegate for EditorDelegate {
        /// Fires on every buffer edit. We forward the full current buffer to the
        /// brain as `EditorEdited` so it can hold the live document.
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _notification: &NSNotification) {
            let content = self
                .ivars()
                .text_view
                .borrow()
                .as_ref()
                .map(|tv| {
                    let s: Retained<NSString> = unsafe { msg_send![&**tv, string] };
                    s.to_string()
                })
                .unwrap_or_default();
            self.fire(SMessage::EditorEdited { content });
        }
    }
);

unsafe impl NSObjectProtocol for EditorDelegate {}

impl EditorDelegate {
    /// Send a UI → brain message on the stored `tx_event`, if present.
    /// Non-blocking (`try_send`): a full/closed channel drops the event rather
    /// than stalling the main thread.
    fn fire(&self, msg: SMessage) {
        if let Some(tx) = self.ivars().tx_event.borrow().as_ref() {
            let _ = tx.try_send(msg);
        }
    }

    /// Replace the editor buffer with `text`. Must run on the main thread
    /// (the `MacOSSpline` router dispatches `EditorLoad` here via the main queue).
    pub fn set_content(&self, text: &str) {
        if let Some(text_view) = self.ivars().text_view.borrow().as_ref() {
            let s = NSString::from_str(text);
            unsafe {
                let _: () = msg_send![&**text_view, setString: &*s];
            }
        }
    }
}

// -----------------------------------------------------------------------------
// ASSEMBLY
// -----------------------------------------------------------------------------
/// Build the editor pane view + its delegate, seeded from `editor_state.content`.
/// `tx_event` is the UI → brain sender the delegate uses to emit `EditorEdited`
/// / `EditorSaveRequest`.
pub fn create_editor(
    _mtm: MainThreadMarker,
    editor_state: &EditorState,
    tx_event: async_channel::Sender<SMessage>,
) -> (Retained<NSView>, Retained<EditorDelegate>) {
    // 1. Instantiate the delegate.
    let delegate: Allocated<EditorDelegate> = unsafe { msg_send![EditorDelegate::class(), alloc] };
    let delegate: Retained<EditorDelegate> = unsafe { msg_send![delegate, init] };
    *delegate.ivars().tx_event.borrow_mut() = Some(tx_event);

    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(774.0, 768.0));

    // 2. Scroll view host (mirrors comms.rs's input scroll setup).
    let scroll: Allocated<NSScrollView> = unsafe { msg_send![NSScrollView::class(), alloc] };
    let scroll: Retained<NSScrollView> = unsafe { msg_send![scroll, initWithFrame: frame] };
    unsafe {
        let _: () = msg_send![&scroll, setTranslatesAutoresizingMaskIntoConstraints: objc2::runtime::Bool::NO];
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(true);
        let _: () = msg_send![&scroll, setDrawsBackground: objc2::runtime::Bool::YES];
    }

    // 3. The editable, monospace text view.
    let text_view: Allocated<NSTextView> = unsafe { msg_send![NSTextView::class(), alloc] };
    let text_view: Retained<NSTextView> = unsafe { msg_send![text_view, initWithFrame: frame] };
    unsafe {
        text_view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        // Editable, not field-editor; grow vertically with content.
        let _: () = msg_send![&text_view, setEditable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&text_view, setSelectable: objc2::runtime::Bool::YES];
        let _: () = msg_send![&text_view, setRichText: objc2::runtime::Bool::NO];
        let _: () = msg_send![&text_view, setAutomaticQuoteSubstitutionEnabled: objc2::runtime::Bool::NO];

        // Monospace font — the fixed-pitch user font, code-editor sizing.
        let font = NSFont::userFixedPitchFontOfSize(13.0);
        if let Some(font) = font {
            let _: () = msg_send![&text_view, setFont: &*font];
        }

        // Subtle dark editor backdrop with light text.
        let bg = NSColor::textBackgroundColor();
        let _: () = msg_send![&text_view, setBackgroundColor: &*bg];
        let fg = NSColor::textColor();
        let _: () = msg_send![&text_view, setTextColor: &*fg];
    }

    text_view.setVerticallyResizable(true);
    text_view.setHorizontallyResizable(true);

    // 4. Seed the buffer from the workspace snapshot.
    if !editor_state.content.is_empty() {
        let s = NSString::from_str(&editor_state.content);
        unsafe {
            let _: () = msg_send![&text_view, setString: &*s];
        }
    }

    // 5. Anchor text view into the scroll view + remember it for later updates.
    scroll.setDocumentView(Some(&text_view));
    *delegate.ivars().text_view.borrow_mut() = Some(text_view.clone());

    (unsafe { Retained::cast_unchecked::<NSView>(scroll) }, delegate)
}
