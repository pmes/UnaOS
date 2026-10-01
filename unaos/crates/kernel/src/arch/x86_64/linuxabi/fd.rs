// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! LINUXABI2 — open file descriptions (shared across `dup`/`fork` via `Arc<Desc>`), pipes, and the stdin/stdout
//! plumbing between a Linux process and the shell verb that is running it.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub const PIPE_CAP: usize = 64 * 1024;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_APPEND: u32 = 0o2000;

/// A 64 KiB kernel ring. `readers`/`writers` count live DESCRIPTIONS (not fds): the last writer's drop is EOF.
pub struct Pipe {
    pub buf: spin::Mutex<VecDeque<u8>>,
    pub readers: AtomicUsize,
    pub writers: AtomicUsize,
}

pub enum Kind {
    /// The shell window: reads pop a line from [`STDIN`], writes go to serial + [`OUT`].
    Console,
    File { path: String, data: Vec<u8>, pos: u64, read: bool, write: bool },
    Dir { path: String, ents: Vec<(String, bool)>, pos: usize },
    PipeR(Arc<Pipe>),
    PipeW(Arc<Pipe>),
}

pub struct Desc {
    pub k: spin::Mutex<Kind>,
    pub flags: AtomicU32,
}

impl Desc {
    pub fn new(k: Kind, flags: u32) -> Arc<Desc> {
        Arc::new(Desc { k: spin::Mutex::new(k), flags: AtomicU32::new(flags) })
    }
    pub fn nonblock(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & O_NONBLOCK != 0
    }
}

impl Drop for Desc {
    fn drop(&mut self) {
        match self.k.get_mut() {
            Kind::PipeR(p) => {
                p.readers.fetch_sub(1, Ordering::AcqRel);
            }
            Kind::PipeW(p) => {
                p.writers.fetch_sub(1, Ordering::AcqRel);
            }
            _ => {}
        }
    }
}

/// A fresh pipe: `(read end, write end)`.
pub fn new_pipe(flags: u32) -> (Arc<Desc>, Arc<Desc>) {
    let p = Arc::new(Pipe { buf: spin::Mutex::new(VecDeque::new()), readers: AtomicUsize::new(1), writers: AtomicUsize::new(1) });
    (Desc::new(Kind::PipeR(p.clone()), flags), Desc::new(Kind::PipeW(p), flags))
}

#[derive(Clone)]
pub struct FdEnt {
    pub d: Arc<Desc>,
    pub cloexec: bool,
}

// ---------------------------------------------------------------------------------------------
// stdin / stdout of the foreground Linux session
// ---------------------------------------------------------------------------------------------

/// Lines typed (or injected by a test) for `read(0)`. Each entry ends in `\n`.
pub static STDIN: spin::Mutex<VecDeque<u8>> = spin::Mutex::new(VecDeque::new());
/// Everything the session wrote to fd 1/2 and has not yet been shown in the shell window / captured.
pub static OUT: spin::Mutex<Vec<u8>> = spin::Mutex::new(Vec::new());

pub fn stdin_push(bytes: &[u8]) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut q = STDIN.lock();
        for &b in bytes {
            if q.len() < 64 * 1024 {
                q.push_back(b);
            }
        }
    });
}

pub fn stdin_has_data() -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| !STDIN.lock().is_empty())
}

/// Pop up to `max` bytes, stopping after the first `\n` (line discipline).
pub fn stdin_take_line(max: usize) -> Vec<u8> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut q = STDIN.lock();
        let mut v = Vec::new();
        while v.len() < max {
            match q.pop_front() {
                Some(b) => {
                    v.push(b);
                    if b == b'\n' {
                        break;
                    }
                }
                None => break,
            }
        }
        v
    })
}

pub fn out_push(bytes: &[u8]) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut o = OUT.lock();
        o.extend_from_slice(bytes);
        let n = o.len();
        if n > 256 * 1024 {
            o.drain(..n - 128 * 1024);
        }
    });
}

pub fn out_take() -> Vec<u8> {
    x86_64::instructions::interrupts::without_interrupts(|| core::mem::take(&mut *OUT.lock()))
}

/// Lock that never spins against a PREEMPTED holder on this core (IF can be open after a `yield_now` inside a syscall): on contention,
/// yield and retry.
pub fn lk<T>(m: &spin::Mutex<T>) -> spin::MutexGuard<'_, T, spin::Spin> {
    loop {
        if let Some(g) = m.try_lock() {
            return g;
        }
        crate::arch::sched::yield_now();
    }
}
