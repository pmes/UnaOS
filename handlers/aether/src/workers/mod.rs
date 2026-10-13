//! A script worker thread: its own js_core realm (no DOM), fed source text over a channel, answering
//! with each completion value as a string ("Error: …" for a throw). The original groundwork; pages reach
//! workers through [`dedicated`] (AETHERWORKERS, SR123), not through this.
//!
//! AETHERSTORAGE (SR94): [`sw`] — service-worker registrations, lifecycle, control and the fetch interception
//! hook — and [`realm`], the ServiceWorkerGlobalScope realm each running worker gets on its own thread.
//!
//! AETHERWORKERS (SR123): [`dedicated`] — `new Worker()`'s thread and DedicatedWorkerGlobalScope realm (the page
//! half is `js::worker`; MessagePorts are `js::ports`).
//!
//! AETHERWORKERS3 (SR129): [`shared`] — `new SharedWorker()`'s process-wide registry, `connect`, and the
//! lifetime that outlives any one document (the thread and realm are `dedicated`'s, run as shared).

pub mod dedicated;
pub mod realm;
pub mod shared;
pub mod sw;

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

pub struct Worker {
    pub sender: Sender<String>,
}

pub fn spawn_worker() -> (Worker, Receiver<String>) {
    let (tx_in, rx_in) = mpsc::channel::<String>();
    let (tx_out, rx_out) = mpsc::channel::<String>();

    thread::Builder::new()
        .name("aether-worker".into())
        .stack_size(8 << 20)
        .spawn(move || {
            let mut vm = js_core::vm::Vm::new(Box::new(js_core::vm::NullHost));
            vm.budget = None;
            while let Ok(msg) = rx_in.recv() {
                vm.budget = Some(crate::js::TASK_BUDGET);
                vm.terminated = false;
                let out = match vm.run_script_str(&msg) {
                    Ok(v) => vm.to_string(&v).map(|s| s.to_rust()).unwrap_or_else(|_| "<unprintable>".into()),
                    Err(e) => format!("Error: {}", vm.error_string(&e)),
                };
                let _ = vm.checkpoint();
                if tx_out.send(out).is_err() {
                    break;
                }
            }
        })
        .expect("spawn worker thread");

    (Worker { sender: tx_in }, rx_out)
}
