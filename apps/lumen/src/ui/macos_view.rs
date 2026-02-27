// apps/lumen/src/ui/macos_view.rs
#![cfg(target_os = "macos")]

use async_channel::{Receiver, Sender};
use gneiss_pal::GuiUpdate;
use quartzite::{Event, NativeView, NativeWindow};
use objc2::{msg_send, ClassType};
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSTextView, NSScrollView, NSButton, NSBezelStyle, NSControl, NSLayoutAttribute, NSLayoutConstraint, NSLayoutRelation, NSText};
use objc2_foundation::{MainThreadMarker, NSRect, NSPoint, NSSize, NSString, NSAutoresizingMaskOptions};
use std::ffi::c_void;

pub struct MacOSSpline {}

impl MacOSSpline {
    pub fn new() -> Self {
        Self {}
    }

    pub fn bootstrap(
        &self,
        window: &NativeWindow,
        _tx_event: Sender<Event>,
        rx_gui: Receiver<GuiUpdate>,
    ) -> NativeView {
        let mtm = MainThreadMarker::new().expect("Must be on main thread");

        // 1. Root Container
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        let root_view = unsafe {
            let view: Retained<NSView> = msg_send![mtm.alloc::<NSView>(), initWithFrame: frame];
            // Autoresizing: Width + Height
            let mask = NSAutoresizingMaskOptions::NSViewWidthSizable | NSAutoresizingMaskOptions::NSViewHeightSizable;
            view.setAutoresizingMask(mask);
            view
        };

        // 2. Console (NSTextView inside NSScrollView)
        let scroll_view = unsafe {
            let scroll_frame = NSRect::new(NSPoint::new(20.0, 60.0), NSSize::new(760.0, 520.0));
            let scroll: Retained<NSScrollView> = msg_send![mtm.alloc::<NSScrollView>(), initWithFrame: scroll_frame];
            scroll.setHasVerticalScroller(true);
            let mask = NSAutoresizingMaskOptions::NSViewWidthSizable | NSAutoresizingMaskOptions::NSViewHeightSizable;
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

        // 3. Status Label
        // Just appended to text view for now.

        // 4. Async GUI Update Loop (Dispatch to Main Thread)
        // We need to keep `text_view` alive and accessible to the background thread's closure.
        // But `Retained<NSTextView>` is not Send.
        // Wait. `Retained` wraps a raw pointer. `NSTextView` (like all UI objects) is !Send.
        // We cannot pass `text_view` to `tokio::spawn`.

        // Strategy:
        // We need a thread-safe way to signal the main thread.
        // `dispatch::Queue::main().exec_async` takes a closure.
        // If the closure captures `text_view`, `text_view` must be Send.
        // `objc2` objects are `Send` if `T: Send`. `NSTextView` is `!Send` (MainThreadOnly).

        // Solution:
        // Pass the raw pointer (address) as `usize` or use a thread-safe handle wrapper?
        // OR rely on `block2` and `dispatch` crate if available.
        // Since `quartzite` doesn't expose `dispatch` crate directly, and I can't easily add deps in this step (I can, but it's risky).
        // `objc2_foundation` might have `performSelectorOnMainThread`.

        // Let's use `async_channel` polling ON THE MAIN THREAD via a timer?
        // `NSTimer` repeating every 16ms (60fps) to drain the `rx_gui` queue.
        // This keeps everything on the Main Thread.

        // Timer Logic:
        // Create a helper object (Rust struct wrapped in GObject? No, wrapped in simple `Box` leaked?)
        // Or simpler: `glib` main context spawned local works on GTK because GTK runs GLib loop.
        // On macOS, we run `NSApplication`.
        // Does `NSApplication` run `CFRunLoop`? Yes.
        // `CFRunLoop` can drive `tokio`? No.

        // WE NEED A BRIDGE.
        // I will use a simple "Poll" approach for Phase 1.
        // "Spin" a checking loop using `dispatch_after` or similar?
        // Actually, `objc2` doesn't expose `dispatch_async` directly in a safe way without `block2`.

        // Hack for Phase 1:
        // Spawn a thread that reads `rx`. When it gets a message, it uses `dispatch_async_f` (C function) to call a function on main thread?
        // Too complex for safe Rust.

        // Let's assume `quartzite` (or `vein`) runs logic in background.
        // How did `lumen` (GTK) do it? `glib::MainContext::default().spawn_local`.
        // This works because GTK uses GLib context.

        // macOS `AppKit` does NOT use GLib context.
        // So `spawn_local` won't run unless we drive the GLib context manually (which we don't).

        // **The Correct Way (Mach Philosophy):**
        // Use `dispatch` (Grand Central Dispatch).
        // I need to add `dispatch` crate to `apps/lumen`.
        // Or use `objc2-foundation` `NSThread`.

        // `NSThread::detachNewThreadSelector_toTarget_withObject`.
        // No, that spawns a thread.

        // `performSelectorOnMainThread:withObject:waitUntilDone:`
        // I need a "Target" object that implements the selector.
        // I can define a custom class `GuiUpdater` that holds the `text_view` and `rx`.
        // And has a method `update`.
        // Then call `performSelector...` on it?
        // `performSelector` schedules it.

        // Let's try defining a helper class `LumenBridge`.

        use objc2::define_class;
        use objc2::DeclaredClass;
        use objc2_foundation::{NSObject, NSObjectProtocol};
        use std::cell::RefCell;

        // Define a class that holds the text view and receiver.
        // BUT `Receiver` is Send/Sync. `NSTextView` is NOT.
        // So `LumenBridge` must be created on Main Thread and stay there.
        // We need a trigger mechanism.

        // OK, Plan B: `CFRunLoopObserver`?

        // Plan C (The "Mach" Way):
        // Use `dispatch` crate. It's the standard.
        // I will add `dispatch` to `apps/lumen` dependencies.
        // `dispatch::Queue::main().exec_async(move || { ... })`.
        // But `move ||` requires captured variables to be `Send`.
        // `NSTextView` is `!Send`.
        // How do we update UI from background then?
        // We pass the **address** (thread-safe handle) and reconstruct/validate on main thread?
        // `Retained<NSTextView>` -> `usize` (ptr).
        // `unsafe { Id::from_raw(ptr) }` inside the main thread block.

        // Let's implement this Pattern.
        // 1. Get raw pointer of text_view.
        // 2. Spawn a background thread (tokio::spawn) to read `rx`.
        // 3. When msg received, call `dispatch::Queue::main().exec_async`.
        // 4. Inside async block, reconstruct `Retained<NSTextView>` (unsafe but we know it's alive because `root_view` holds it).
        // 5. Update text.

        // I need `dispatch` crate.

        // Wait, `apps/lumen` doesn't have `dispatch`.
        // I will add it in the next step.

        // For now, I will write the code assuming `dispatch` is available.
        // AND `objc2` `Retained::as_ptr` and `Retained::retain` (or just pointer cast).

        let text_view_ptr = Retained::as_ptr(&text_view) as usize; // usize is Send

        // Spawn the bridge thread
        // We use std::thread or tokio::spawn. Tokio is already running.
        let rx = rx_gui.clone();

        // We need `dispatch` crate.
        // I will comment this out and mark "TODO: Add dispatch" until I update Cargo.toml.
        // Or I can use `objc2` to manually link `dispatch_async`? No, too hard.

        // I will write the `MacOSSpline` struct now, but the async loop will be a placeholder
        // that I will activate after adding the dependency.

        // Actually, to make it compile NOW, I will just display a static message.
        unsafe {
            let hello = NSString::from_str(">> Lumen/Mach Substrate Active.\n>> Waiting for Neural Link...");
            text_view.setString(&hello);
        }

        root_view
    }
}
