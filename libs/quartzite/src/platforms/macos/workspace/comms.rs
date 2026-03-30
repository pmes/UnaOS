// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use objc2::{
    define_class, msg_send, msg_send_id, mutability, rc::Retained, ProtocolType
};
use objc2_app_kit::{
    NSScrollView, NSTextView, NSTextViewDelegate, NSView, NSVisualEffectView,
    NSVisualEffectMaterial, NSVisualEffectBlendingMode, NSVisualEffectState,
    NSLayoutConstraint, NSLayoutAnchor
};
use objc2_foundation::{NSArray, MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

/// Builds the Right Pane (Reactor / Comms Area) consisting of:
/// 1. An NSScrollView for the chat/event history (anchored to top/sides, bottom anchored to input)
/// 2. An NSTextView wrapped in an NSScrollView for user input (anchored to bottom)
/// 3. A floating NSVisualEffectView for the Pre-Flight Stack
pub fn build_right_pane(mtm: MainThreadMarker) -> Retained<NSView> {
    // 1. Root Container (Rigid NSView to prevent drag-resizing, strictly anchored)
    let container = NSView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0., 0.), NSSize::new(800., 800.)));
    container.setTranslatesAutoresizingMaskIntoConstraints(false);

    // 2. Chat/History Scroll View
    let history_scroll = NSScrollView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0., 0.), NSSize::new(800., 700.)));
    history_scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
    history_scroll.setHasVerticalScroller(true);
    history_scroll.setAutohidesScrollers(true);
    history_scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);

    // We would typically put an NSStackView or NSTableView here to render chat bubbles.
    let history_content = NSView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0., 0.), NSSize::new(800., 700.)));
    history_content.setTranslatesAutoresizingMaskIntoConstraints(false);
    history_scroll.setDocumentView(Some(&history_content));

    container.addSubview(&history_scroll);

    // 3. The Input Buffer (SourceView translated to NSTextView)
    let input_scroll = NSScrollView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0., 0.), NSSize::new(800., 100.)));
    input_scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
    input_scroll.setHasVerticalScroller(true);
    input_scroll.setAutohidesScrollers(true);
    input_scroll.setBorderType(objc2_app_kit::NSBorderType::GrooveBorder);

    let input_view = NSTextView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0., 0.), NSSize::new(800., 100.)));
    input_view.setTranslatesAutoresizingMaskIntoConstraints(false);
    input_view.setRichText(false);
    input_view.setImportsGraphics(false);
    input_view.setAllowsImageEditing(false);
    input_view.setUsesFontPanel(false);
    input_view.setAllowsUndo(true);
    input_view.setAutomaticSpellingCorrectionEnabled(false);
    input_view.setAutomaticQuoteSubstitutionEnabled(false);
    input_view.setAutomaticDashSubstitutionEnabled(false);

    input_scroll.setDocumentView(Some(&input_view));
    container.addSubview(&input_scroll);

    // Wire Input Delegate to capture Enter key
    let delegate = InputDelegate::new(mtm);
    input_view.setDelegate(Some(objc2::ProtocolObject::from_ref(&*delegate)));

    // 4. The Floating Pre-Flight Stack (NSVisualEffectView)
    let pre_flight = NSVisualEffectView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0., 0.), NSSize::new(400., 300.)));
    pre_flight.setTranslatesAutoresizingMaskIntoConstraints(false);
    pre_flight.setMaterial(NSVisualEffectMaterial::Popover); // Translates to GTK's "Card" styling
    pre_flight.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    pre_flight.setState(NSVisualEffectState::Active);
    // Hide initially until toggled via spline state
    pre_flight.setHidden(true);

    container.addSubview(&pre_flight);

    // 5. Build Strict Auto Layout Constraints
    let constraints = NSArray::from_vec(vec![
        // History Anchors (Top to Root, Sides to Root, Bottom to Input)
        history_scroll.topAnchor().constraintEqualToAnchor(container.topAnchor()),
        history_scroll.leadingAnchor().constraintEqualToAnchor(container.leadingAnchor()),
        history_scroll.trailingAnchor().constraintEqualToAnchor(container.trailingAnchor()),
        history_scroll.bottomAnchor().constraintEqualToAnchor_constant(input_scroll.topAnchor(), -8.0),

        // Input Anchors (Bottom/Sides to Root, Height limit)
        input_scroll.leadingAnchor().constraintEqualToAnchor_constant(container.leadingAnchor(), 16.0),
        input_scroll.trailingAnchor().constraintEqualToAnchor_constant(container.trailingAnchor(), -16.0),
        input_scroll.bottomAnchor().constraintEqualToAnchor_constant(container.bottomAnchor(), -16.0),
        input_scroll.heightAnchor().constraintGreaterThanOrEqualToConstant(40.0),
        input_scroll.heightAnchor().constraintLessThanOrEqualToConstant(200.0),

        // Pre-Flight Stack (Centered floating over history)
        pre_flight.centerXAnchor().constraintEqualToAnchor(history_scroll.centerXAnchor()),
        pre_flight.centerYAnchor().constraintEqualToAnchor(history_scroll.centerYAnchor()),
        pre_flight.widthAnchor().constraintEqualToConstant(500.0),
        pre_flight.heightAnchor().constraintEqualToConstant(400.0),
    ]);

    NSLayoutConstraint::activateConstraints(&constraints);

    // Prevent the delegate from dropping
    std::mem::forget(delegate);

    container
}

// -----------------------------------------------------------------------------
// NATIVE TEXTVIEW DELEGATE (Input Capture)
// -----------------------------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "UnaInputDelegate"]
    pub struct InputDelegate;

    unsafe impl NSTextViewDelegate for InputDelegate {
        #[method(textView:doCommandBySelector:)]
        fn text_view_do_command_by_selector(&self, _text_view: &NSTextView, command_selector: objc2::sel::Sel) -> bool {
            // Translates the GTK shift+enter logic.
            // In AppKit, we intercept `insertNewline:` to send the payload,
            // and `insertNewlineIgnoringFieldEditor:` (Shift+Enter) to insert a literal newline.
            // For now, we stub this out, as we would need to map the selector.
            false // Return false to let the textview handle it normally
        }
    }
);

impl InputDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };
        this
    }
}
