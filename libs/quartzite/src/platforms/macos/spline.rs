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

use bandy::SMessage;
use block2::RcBlock;
use objc2_foundation::MainThreadMarker;

/// Dispatches a closure safely to the AppKit main thread using Grand Central Dispatch.
/// We use this function rather than fighting raw GCD APIs directly, taking advantage
/// of `objc2-foundation`'s main thread queuing mechanism if available, or a fallback.
pub fn dispatch_to_main<F>(f: F)
where
    F: FnOnce(MainThreadMarker) + Send + 'static,
{
    // The idiomatic way to dispatch to main in objc2 is using NSRunLoop or block APIs.
    // For simplicity, we use the MainThreadMarker::run_on_main pattern or similar.
    // Since objc2 0.4.0+, `MainThreadMarker::run_on_main` exists, but we can also use dispatch queues.
    // For this bridge, we construct a block and dispatch it to the main thread.

    // We can use MainThreadMarker::alloc().performSelectorOnMainThread... but
    // a simpler approach is dispatch_async using GCD if exposed, or using objc2's run_on_main.
    // Let's use standard GCD dispatch_async.

    extern "C" {
        fn dispatch_get_main_queue() -> *mut std::ffi::c_void;
        fn dispatch_async(queue: *mut std::ffi::c_void, block: *mut std::ffi::c_void);
    }

    // We use the objc2 `block2::RcBlock` to define our closure.
    // However, `dispatch_async` takes ownership of the block, so we use `RcBlock::new`
    // and cast it to a C void pointer.
    let block = block2::RcBlock::new(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            f(mtm);
        }
    });

    let block_ptr = block.as_ptr() as *mut std::ffi::c_void;

    unsafe {
        let queue = dispatch_get_main_queue();
        dispatch_async(queue, block_ptr);
    }

    // `dispatch_async` inherently copies and retains the block for execution.
    // The Rust side must drop its local `RcBlock` reference normally to maintain
    // the correct retain count. Calling `std::mem::forget` here causes an unbounded leak.
}

/// The macOS native implementation of the Spline router.
/// Receives SMessage from the background and routes it to the main thread for AppKit execution.
pub async fn start_router(rx: async_channel::Receiver<SMessage>) {
    while let Ok(msg) = rx.recv().await {
        dispatch_to_main(move |_mtm| {
            // NOTE: The implementation of this match block will depend on exactly
            // how we structure the UI pointers (Left Pane / Right Pane / Toolbar)
            // and pass them to the spline router.
            //
            // For now, we simply catch the message and log it, ensuring the thread
            // hop was successful.
            println!(":: SPLINE (macOS) :: Received message: {:?}", msg);

            match msg {
                SMessage::StateInvalidated => {
                    // Trigger outline view reload, text view updates, etc.
                }
                SMessage::NetworkLog(log_msg) => {
                    // Update telemetry in the toolbar
                }
                _ => {}
            }
        });
    }
}
