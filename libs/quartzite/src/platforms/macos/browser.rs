use objc2::rc::Retained;
use objc2_app_kit::{
    NSWindow, NSView, NSStackView, NSTextField, NSButton, NSImageView,
    NSUserInterfaceLayoutOrientation, NSStackViewGravity
};
use objc2_foundation::{MainThreadMarker, NSRect, NSPoint, NSSize, NSString};
use bandy::{SMessage, Synapse};
use tokio::sync::broadcast::Receiver;

pub fn bootstrap_browser(
    _window: &NSWindow,
    _tx: Synapse,
    _rx: Receiver<SMessage>,
) -> Retained<NSView> {
    let _mtm = MainThreadMarker::new().expect("bootstrap_browser must run on the main thread");

    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
    
    unsafe {
        let stack: Retained<NSStackView> = objc2::msg_send_id![NSStackView::class(), alloc];
        let stack: Retained<NSStackView> = objc2::msg_send_id![&stack, initWithFrame: frame];
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        
        let chrome_stack: Retained<NSStackView> = objc2::msg_send_id![NSStackView::class(), alloc];
        let chrome_stack: Retained<NSStackView> = objc2::msg_send_id![&chrome_stack, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 40.0))];
        chrome_stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        
        let url_bar: Retained<NSTextField> = objc2::msg_send_id![NSTextField::class(), alloc];
        let url_bar: Retained<NSTextField> = objc2::msg_send_id![&url_bar, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 24.0))];
        
        let back_btn: Retained<NSButton> = objc2::msg_send_id![NSButton::class(), alloc];
        let back_btn: Retained<NSButton> = objc2::msg_send_id![&back_btn, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(40.0, 24.0))];
        back_btn.setTitle(&NSString::from_str("<"));
        
        let fwd_btn: Retained<NSButton> = objc2::msg_send_id![NSButton::class(), alloc];
        let fwd_btn: Retained<NSButton> = objc2::msg_send_id![&fwd_btn, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(40.0, 24.0))];
        fwd_btn.setTitle(&NSString::from_str(">"));
        
        chrome_stack.addView_inGravity(&back_btn, NSStackViewGravity::Leading);
        chrome_stack.addView_inGravity(&fwd_btn, NSStackViewGravity::Leading);
        chrome_stack.addView_inGravity(&url_bar, NSStackViewGravity::Leading);
        
        let content_view: Retained<NSImageView> = objc2::msg_send_id![NSImageView::class(), alloc];
        let content_view: Retained<NSImageView> = objc2::msg_send_id![&content_view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 560.0))];
        
        stack.addView_inGravity(&chrome_stack, NSStackViewGravity::Top);
        stack.addView_inGravity(&content_view, NSStackViewGravity::Bottom);
        
        // Note: For full macOS implementation, we'd wire the NSTextField delegate and event loops
        // similarly to GTK, but using objc2 delegation macros.
        
        stack.upcast()
    }
}
