// SPDX-License-Identifier: GPL-3.0-or-later

//! The Spline
//!
//! The GCD boundary responsible for pulling `SMessage` signals from the cross-thread
//! async reactor and pushing them onto the main thread via Grand Central Dispatch.
//! This is the strict isolation boundary between Una's asynchronous intelligence
//! and the synchronous, MainThreadOnly Apple AppKit event loop.

use bandy::{SMessage, synapse::Synapse};
use dispatch2::DispatchQueue;
use tokio::runtime::Handle;

/// Starts a dedicated thread that blocks on the Synapse broadcast receiver,
/// translating asynchronous SMessage events into main-thread AppKit mutations.
pub fn initialize_spline(synapse: Synapse) {
    let mut rx = synapse.subscribe();

    // We detach the spline to its own thread to avoid blocking the caller
    // (typically the NSApplication startup logic).
    std::thread::spawn(move || {
        // Since we are running on our own detached OS thread with no active tokio reactor,
        // and tokio::sync::broadcast provides a blocking_recv() method,
        // we use a simple blocking loop.
        loop {
            match rx.blocking_recv() {
                Ok(msg) => {
                    // The event loop boundary
                    // Send the UI update request to the main thread via GCD.
                    DispatchQueue::main().exec_async(move || {
                        handle_smessage_on_main(msg);
                    });
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    log::error!("Synapse closed. The spline is dead.");
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    log::warn!("Spline lagging! Skipped {} action potentials.", skipped);
                }
            }
        }
    });
}

/// The actual execution phase on the `MainThreadOnly` context.
/// Here we translate semantic events into AppKit redraws or view updates.
fn handle_smessage_on_main(msg: SMessage) {
    match msg {
        SMessage::StateInvalidated => {
            log::info!("SMessage::StateInvalidated received on the main thread. Reloading outline view...");
            use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSWindow};
            use objc2::runtime::ProtocolObject;
            use objc2::{msg_send, sel};

            // Re-acquire the AppDelegate to trigger a workspace reload
            // In a more complex architecture, we might pass a weak reference to the spline,
            // but for this direct billet payload, we can use the singleton to grab the window.
            let app = unsafe { NSApplication::sharedApplication() };
            if let Some(delegate_proto) = unsafe { app.delegate() } {
                // Since our AppDelegate is just a state bag for the workspace,
                // we broadcast a generic refresh via NSNotification or custom method if needed.
                // For this J02.03 payload without mutating the defined class further,
                // we can post a notification that the UI components could listen to,
                // OR we can directly grab the key window and reload its responders.
                // We'll broadcast an NSNotification to trigger a generic reload.
                use objc2_foundation::{NSNotificationCenter, NSString};
                let center = unsafe { NSNotificationCenter::defaultCenter() };
                let notif_name = NSString::from_str("UnaStateInvalidated");
                unsafe { center.postNotificationName_object(&notif_name, None) };
            }
        }
        SMessage::Log { level, source, content } => {
            log::debug!("[{}] {}: {}", level, source, content);
            // In a real app we might route this to a debug console text view.
        }
        SMessage::Kill(reason) => {
            log::warn!("Kill signal received via Spline: {}", reason);
            // Trigger NSApplication termination
            use objc2_app_kit::NSApplication;
            use objc2::rc::Retained;
            // Unsafe: NSApp is a global singleton, but we are guaranteed to be on the Main Thread here
            // because `handle_smessage_on_main` is executed via `DispatchQueue::main().exec_async`.
            // We use `sharedApplication` to get the reference.
            let app = unsafe { NSApplication::sharedApplication() };
            unsafe { app.terminate(None) };
        }
        _ => {
            // Unhandled or non-UI relevant messages.
        }
    }
}
