use objc2::{msg_send, ClassType};
use objc2::rc::{Allocated, Retained};
use objc2_app_kit::{
    NSWindow, NSView, NSStackView, NSTextField, NSButton, NSImageView,
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
        let stack: Allocated<NSStackView> = msg_send![NSStackView::class(), alloc];
        let stack: Retained<NSStackView> = msg_send![stack, initWithFrame: frame];
        let _: () = msg_send![&stack, setOrientation: 1isize]; // Vertical
        
        let chrome_stack: Allocated<NSStackView> = msg_send![NSStackView::class(), alloc];
        let chrome_stack: Retained<NSStackView> = msg_send![chrome_stack, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 40.0))];
        let _: () = msg_send![&chrome_stack, setOrientation: 0isize]; // Horizontal
        
        let url_bar: Allocated<NSTextField> = msg_send![NSTextField::class(), alloc];
        let url_bar: Retained<NSTextField> = msg_send![url_bar, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 24.0))];
        
        let back_btn: Allocated<NSButton> = msg_send![NSButton::class(), alloc];
        let back_btn: Retained<NSButton> = msg_send![back_btn, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(40.0, 24.0))];
        let _: () = msg_send![&back_btn, setTitle: &*NSString::from_str("<")];
        
        let fwd_btn: Allocated<NSButton> = msg_send![NSButton::class(), alloc];
        let fwd_btn: Retained<NSButton> = msg_send![fwd_btn, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(40.0, 24.0))];
        let _: () = msg_send![&fwd_btn, setTitle: &*NSString::from_str(">")];
        
        let _: () = msg_send![&chrome_stack, addView: &*back_btn, inGravity: 1isize]; // Leading
        let _: () = msg_send![&chrome_stack, addView: &*fwd_btn, inGravity: 1isize];
        let _: () = msg_send![&chrome_stack, addView: &*url_bar, inGravity: 1isize];
        
        let content_view: Allocated<NSImageView> = msg_send![NSImageView::class(), alloc];
        let content_view: Retained<NSImageView> = msg_send![content_view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 560.0))];
        
        let _: () = msg_send![&stack, addView: &*chrome_stack, inGravity: 1isize]; // Top
        let _: () = msg_send![&stack, addView: &*content_view, inGravity: 3isize]; // Bottom
        
        unsafe { Retained::cast_unchecked::<NSView>(stack) }
    }
}
