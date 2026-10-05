// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFBUILD4 (B356): RUST.LNX, the probe. Usage: `RUST.LNX [file]` (default `/apps/HELLO.C`).
// Prints ONE verdict line the kernel's `tests selfbuild4` parses:
//   rust ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 args=<n> file_bytes=<n> file_lines=<n> cpus=<n> ms=<n>
// and exits 0, or prints `rust fail <what>` and exits 1. Nothing else on stdout.

use std::collections::HashMap;
use std::panic;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const THREADS: usize = 4;
const PER_THREAD: u64 = 100_000;

fn fail(what: &str) -> ! {
    println!("rust fail {what}");
    std::process::exit(1);
}

fn main() {
    let t0 = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| String::from("/apps/HELLO.C"));

    // 1. std::thread x4 joining on a Mutex<u64>: every increment takes the lock (futex PRIVATE on contention),
    //    every spawn maps a stack with a guard page and a sigaltstack.
    let counter = Arc::new(Mutex::new(0u64));
    let mut hs = Vec::with_capacity(THREADS);
    for i in 0..THREADS {
        let c = Arc::clone(&counter);
        let h = std::thread::Builder::new()
            .name(format!("probe{i}"))
            .spawn(move || {
                for _ in 0..PER_THREAD {
                    *c.lock().unwrap() += 1;
                }
            })
            .unwrap_or_else(|_| fail("spawn"));
        hs.push(h);
    }
    for h in hs {
        if h.join().is_err() {
            fail("join");
        }
    }
    let total = *counter.lock().unwrap();
    if total != THREADS as u64 * PER_THREAD {
        fail(&format!("counter={total}"));
    }

    // 2. A HashMap (RandomState seeds from getrandom).
    let mut m: HashMap<String, usize> = HashMap::new();
    for i in 0..1000usize {
        m.insert(format!("k{i}"), i * i);
    }
    let hashmap_ok = m.len() == 1000 && m.get("k999") == Some(&998001) && m.remove("k0") == Some(0);
    if !hashmap_ok {
        fail("hashmap");
    }

    // 3. std::fs (openat + statx + read).
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => fail(&format!("fs({path}: {e})")),
    };
    if text.is_empty() {
        fail("fs(empty)");
    }

    // 4. A panic caught with catch_unwind (the unwinder walks .eh_frame); a quiet hook keeps stdout one line.
    panic::set_hook(Box::new(|_| eprintln!("rust: panic raised (expected; caught)")));
    let r = panic::catch_unwind(|| {
        let v: Vec<u32> = Vec::new();
        if v.is_empty() {
            panic!("probe panic");
        }
        v.len()
    });
    let _ = panic::take_hook();
    let panic_caught = if r.is_err() { 1 } else { 0 };
    if panic_caught != 1 {
        fail("panic");
    }

    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    println!(
        "rust ok threads={THREADS} counter={total} hashmap=ok fs=ok panic_caught={panic_caught} args={} file_bytes={} file_lines={} cpus={cpus} ms={}",
        args.len(),
        text.len(),
        text.lines().count(),
        t0.elapsed().as_millis()
    );
}
