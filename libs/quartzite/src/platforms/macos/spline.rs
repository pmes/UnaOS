// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! macOS UI Routing (The Spline)
//!
//! Grand Central Dispatch (GCD) router for macOS.
//! Ensures `SMessage` payloads originating from the `vein` async background
//! tasks are correctly bridged to the main thread via `dispatch_async`.

use bandy::SMessage;
use block2::RcBlock;
use dispatch2::{Queue, QueueAttribute};

/// Routes a telemetry event to the macOS main thread for UI consumption.
pub fn route_to_main<F>(msg: SMessage, handler: F)
where
    F: FnOnce(SMessage) + Send + 'static,
{
    // The `dispatch_async` queue expects a block to execute on the main thread.
    // The `SMessage` and `handler` are moved into the block closure.
    let block = RcBlock::new(move || {
        handler(msg);
    });

    // Obtain the main GCD queue.
    let main_queue = Queue::main();

    // Enqueue the block asynchronously.
    // `dispatch_async` will internally copy the block via `Block_copy` taking
    // ownership of its execution.
    main_queue.exec_async(&block);

    // CRITICAL MEMORY RULE:
    // Do NOT call `std::mem::forget(block)`.
    // The `RcBlock` must be dropped here at the end of the scope. When it drops,
    // it releases the Rust-side retention. The C-side `Block_copy` inside
    // `dispatch_async` handles the remaining lifecycle and will release it
    // once execution completes. If we forget it, we leak memory on every tick.
}
