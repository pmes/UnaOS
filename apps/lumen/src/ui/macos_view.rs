// apps/lumen/src/ui/macos_view.rs
#![cfg(target_os = "macos")]

use async_channel::{Receiver, Sender};
use gneiss_pal::GuiUpdate;
use quartzite::{Event, NativeView, NativeWindow};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSTextView, NSScrollView, NSAutoresizingMaskOptions};
use objc2_foundation::{MainThreadMarker, NSRect, NSPoint, NSSize, NSString};

pub struct MacOSSpline {}

impl MacOSSpline {
    pub fn new() -> Self {
        Self {}
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        _tx_event: Sender<Event>,
        rx_gui: Receiver<GuiUpdate>,
    ) -> NativeView {
        let mtm = MainThreadMarker::new().expect("Must be on main thread");

        // 1. Root Container
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        let root_view = unsafe {
            let view: Retained<NSView> = msg_send![mtm.alloc::<NSView>(), initWithFrame: frame];
            // Autoresizing: Width + Height
            let mask = NSAutoresizingMaskOptions::WidthSizable | NSAutoresizingMaskOptions::HeightSizable;
            view.setAutoresizingMask(mask);
            view
        };

        // 2. Console (NSTextView inside NSScrollView)
        let scroll_view = unsafe {
            let scroll_frame = NSRect::new(NSPoint::new(20.0, 60.0), NSSize::new(760.0, 520.0));
            let scroll: Retained<NSScrollView> = msg_send![mtm.alloc::<NSScrollView>(), initWithFrame: scroll_frame];
            scroll.setHasVerticalScroller(true);
            let mask = NSAutoresizingMaskOptions::WidthSizable | NSAutoresizingMaskOptions::HeightSizable;
            scroll.setAutoresizingMask(mask);
            scroll
        };

        let text_view = unsafe {
            let text_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(760.0, 520.0));
            let text: Retained<NSTextView> = msg_send![mtm.alloc::<NSTextView>(), initWithFrame: text_frame];
            text.setEditable(false);
            text.setRichText(false);
            text.setFont(Some(&objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(12.0, 400.0))); // weight 400 = Regular
            text
        };

        unsafe {
            scroll_view.setDocumentView(Some(&text_view));
            root_view.addSubview(&scroll_view);
        }

        // 3. Static Welcome Message (Async pending 'dispatch' crate)
        unsafe {
            let hello = NSString::from_str(">> Lumen/Mach Substrate Active.\n>> Waiting for Neural Link...");
            text_view.setString(&hello);
        }

        // Suppress unused warning for rx_gui until async implementation
        let _ = rx_gui;

        root_view
    }
}
