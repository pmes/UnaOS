// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// STOR-1 (S1): the storage SERVICE TASK + the BlockRequest submit/complete handshake — the
// interrupt-driven x86 storage spine. See docs/dev/OS/07_USB_STORAGE/x86_interrupt_storage.md.
//
// The problem this exists to solve: every x86 storage syscall runs IF-masked (SFMASK clears IF on
// `syscall` entry), and the only kernel->sector path is the xHCI Bulk-Only Transport (BOT) pump,
// which `hlt()`s awaiting an asynchronous transfer-completion event. A `hlt` under IF=0 never wakes,
// so an in-handler disk read/write would hang the core. The whole staged-buffer family (U6bx staged
// read / U9x flush queue / U10x op-queue) exists to defer the disk half to an IF=1 context.
//
// This module gives x86 the aarch64 *semantics* (a syscall that blocks on a real transfer completion
// and reads/writes the live volume in place) via a different *mechanism* (interrupt completion + a
// sleeping syscall, since x86 storage is async DMA, not synchronous PIO):
//
//   * one scheduled kernel task — the STORAGE SERVICE TASK — owns the BOT pump. It runs at IF=1 (a
//     normal `sched::spawn`ed task, like the U6bx/U9x/U10x launchers already do their IF=1 drains),
//     so the pump's `hlt` wakes on the storage MSI-X IRQ.
//   * a storage syscall builds a `BlockRequest` on its OWN kernel stack, pushes its address onto
//     `REQ_QUEUE`, wakes the service task (`REQ_READY.post()`), and blocks on the request's own
//     `done` semaphore. `Semaphore::wait` restores the caller's IF snapshot as it switches away, so
//     the handler sleeps with IF=1 SEMANTICS even though it entered IF-masked — the crux that makes
//     it IF-safe. The task is off every run queue while blocked (STATE_BLOCKED), so nothing runs the
//     half-finished syscall on another core, and the frame (hence the request + its semaphore) stays
//     pinned and alive for the whole transfer — the same lifetime trick `Condvar::wait` relies on.
//   * the service task pops the request, runs the transaction (raw sector ops go through the block
//     layer; file ops through `fat.rs` — S2..S4), writes the result, and `done.post()`s, which
//     `make_ready`s the submitter back onto its pinned CPU. All wakeups happen HERE, at IF=1 — never
//     in interrupt context — so the wake-only MSI-X handler stays lock-free (SECURITY / sched.rs:28
//     invariant preserved: handlers never `make_ready`).
//
// The MSI-X handler is UNCHANGED: it still only acks + EOIs to wake the pump's `hlt`. The single BOT
// submitter when this path is active is the service task, so there is exactly one drainer — the
// self-deadlock the IRQ handler avoids today is avoided here by construction.
//
// x86_64 + `irqstorage` only (a compile-time knob; the default build never links this file, so the
// staged path is byte-identical). Metal-PENDING: transfer-event-driven I/O on the 2012 rMBP Panther
// Point xHCI is unproven — a bounded wall-clock timeout in `pump_until_bot_done` surfaces a wedged
// transfer as `BlockError::Io` to the sleeping submitter (it never hangs), and endpoint recovery
// beyond that is explicitly out of scope.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};

use alloc::collections::VecDeque;
use spin::Mutex as SpinMutex;

use crate::arch::sched::{self, Semaphore, PRIO_NORMAL};
use crate::drivers::block;

/// Upper bound on a file name carried in a request — a dotted 8.3 name is at most 12 bytes (matches
/// the syscall layer's `MAX_NAME`).
pub const NAME_MAX: usize = 12;

/// The kind of transaction a `BlockRequest` carries. S1: the raw single-sector ops (the block-layer
/// primitive + the `bx-blockreq` self-test). S2: `ReadFile` (a live in-place file-range read,
/// resolved BY NAME on the live volume). S3..S4 add the write / allocator ops.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockOp {
    /// Raw sector read: read one block at `lba` into `buf[..len]`.
    Read,
    /// Raw sector write: write `buf[..len]` (zero-padded to the block size) to the block at `lba`.
    Write,
    /// File-range read: resolve `name` on the live volume (`find_located`), then `read_at(cluster,
    /// on-disk size, offset, .., len)` into `buf`. Returns the byte count read (`>= 0`), or `-EIO`.
    ReadFile,
}

/// A storage transaction submitted to the service task. It lives on the SUBMITTER'S kernel stack:
/// the submitter is blocked in `done.wait()` for the whole transfer, so the frame is pinned and the
/// raw pointer the service task dereferences stays valid (the `Condvar::wait`/`done_sem` lifetime
/// discipline). `result` carries the outcome the syscall returns: `>= 0` is a byte count / success,
/// `< 0` is a negative errno.
pub struct BlockRequest {
    pub op: BlockOp,
    /// Raw ops: the target LBA. File ops: unused.
    pub lba: u64,
    /// File ops: the byte offset within the file the read/write starts at.
    pub offset: u32,
    /// The data buffer — ALWAYS a KERNEL address (the submitter's kernel stack, in the shared kernel
    /// half). It is NOT the ring-3 buffer: the user window lives in the submitter's private CR3
    /// (PML4[2]), unreachable by the service task under the kernel CR3, so a file syscall BOUNCES —
    /// the IF-masked handler copies user<->this kernel buffer, and the service task only ever touches
    /// this kernel buffer. Raw ops / the self-test pass a kernel stack buffer directly.
    pub buf: *mut u8,
    /// Bytes to transfer (raw ops: the block size; file ops: the requested/available byte count).
    pub len: usize,
    /// File ops: the 8.3 name to resolve on the live volume (`name[..name_len]`).
    pub name: [u8; NAME_MAX],
    pub name_len: usize,
    /// Outcome: `>= 0` a byte count / 0 success, `< 0` a negative errno. Written by the service task
    /// (Release) before `done.post()`; read by the submitter (Acquire) after `done.wait()` returns.
    pub result: AtomicI32,
    /// The submitter blocks here; the service task posts it on completion. Must be `init()`ed by the
    /// submitter before `submit()` (reserve the waiter list so the park-side push never reallocates).
    pub done: Semaphore,
}

impl BlockRequest {
    /// A fresh raw-op request. The submitter fills `buf`/`len`, calls `done.init()`, then `submit()`.
    pub const fn new_raw(op: BlockOp, lba: u64, buf: *mut u8, len: usize) -> Self {
        BlockRequest {
            op,
            lba,
            offset: 0,
            buf,
            len,
            name: [0u8; NAME_MAX],
            name_len: 0,
            result: AtomicI32::new(0),
            done: Semaphore::new(0),
        }
    }

    /// A fresh file-op request over `name[..name_len]` at byte `offset`, `len` bytes to/from the
    /// KERNEL buffer `buf`. The submitter calls `done.init()` then `submit()`.
    pub fn new_file(op: BlockOp, name: &[u8], offset: u32, buf: *mut u8, len: usize) -> Self {
        let mut nb = [0u8; NAME_MAX];
        let n = name.len().min(NAME_MAX);
        nb[..n].copy_from_slice(&name[..n]);
        BlockRequest {
            op,
            lba: 0,
            offset,
            buf,
            len,
            name: nb,
            name_len: n,
            result: AtomicI32::new(0),
            done: Semaphore::new(0),
        }
    }
}

/// Negative errno the block layer's failures map to (matches the syscall layer's `EIO`).
const EIO: i32 = -5;

/// The submission queue: raw `*mut BlockRequest` addresses (stored as `usize` so the static is
/// `Sync` — the scheduler's `park_waiters` idiom). Pushed by submitters (IF already masked in the
/// syscall handler), popped by the service task; never held across I/O, so it is a short spinlock.
static REQ_QUEUE: SpinMutex<VecDeque<usize>> = SpinMutex::new(VecDeque::new());

/// Counts pending requests: a submitter `post`s after pushing; the service task `wait`s before
/// popping. post-before-push-visible is impossible (push precedes post in program order; the
/// semaphore's lock release/acquire orders it before the service task's pop), so a `wait()` return
/// always finds its request.
static REQ_READY: Semaphore = Semaphore::new(0);

/// One-shot spawn guard for the service task.
static SERVICE_STARTED: AtomicBool = AtomicBool::new(false);
/// One-shot guard for the `bx-blockreq` self-test.
static SELFTEST_DONE: AtomicBool = AtomicBool::new(false);
/// The self-test's verdict, recorded in memory as well as printed — so the witness is deterministic
/// even on the run where the best-effort console drops the verdict line under multi-AP contention
/// (the documented U8x/U10x console-drop). `0` = not run, `1` = PASS, `2` = FAIL. Readable via
/// `selftest_verdict()` for a stable re-print / metal-bench check.
static SELFTEST_RESULT: AtomicU8 = AtomicU8::new(0);
/// Best-effort bound so the queue never reallocates under the spinlock (reserved at spawn).
const REQ_QUEUE_CAP: usize = 16;

/// Submit a request and BLOCK until the service task completes it, then return its `result`. Called
/// from the submitter's context (the IF-masked syscall handler, or the self-test kernel task). MUST
/// be on a scheduled task — the block needs a `current` to switch away from; off one (the unscheduled
/// BSP) `done.wait()` returns false without blocking and we fall through with whatever `result` holds
/// (a caller off a scheduled task is a bug — the syscall paths always run on an AP task).
///
/// SAFETY: `req` must point at a live `BlockRequest` that outlives the call — it does, because the
/// caller is blocked in `done.wait()` for the whole transfer, pinning the stack frame it lives in.
/// The caller must have `done.init()`ed it (reserve the single-waiter capacity).
pub unsafe fn submit(req: *mut BlockRequest) -> i32 {
    // Push the address, then wake the service task. IF is already masked in the syscall handler; on
    // the self-test task `post`/`wait` snapshot and restore IF themselves, so no explicit guard here.
    REQ_QUEUE.lock().push_back(req as usize);
    REQ_READY.post();
    // Block until the service task posts. On a scheduled task this restores the caller's IF snapshot
    // across the switch (IF-safe even though the handler entered IF=0).
    let _ = unsafe { (*req).done.wait() };
    unsafe { (*req).result.load(Ordering::Acquire) }
}

/// S2 helper the syscall layer calls: read `want` bytes of file `name` at byte `offset` from the
/// LIVE volume into the KERNEL buffer `kbuf`, returning the byte count (`>= 0`) or `-EIO`. Blocks the
/// caller on the service task. The caller then copies `kbuf` out to the ring-3 destination (the
/// bounce — `kbuf` is reachable by the service task; the user VA is not).
///
/// SAFETY: `kbuf` must be a live kernel buffer of at least `want` bytes on the caller's stack (pinned
/// while it blocks). The caller must be a scheduled task (so it can block).
pub unsafe fn submit_read_file(name: &[u8], offset: u32, kbuf: *mut u8, want: usize) -> i32 {
    let mut req = BlockRequest::new_file(BlockOp::ReadFile, name, offset, kbuf, want);
    req.done.init();
    unsafe { submit(&mut req as *mut BlockRequest) }
}

/// The service task body: pop one request, run it at IF=1, post the submitter. A normal scheduled
/// kernel task, so its `hlt` inside the BOT pump wakes on the storage MSI-X. Never returns (the
/// infinite loop makes the effective type `!`; `spawn` wants a `fn(usize)`, so it is left `()`).
fn service_task(_: usize) {
    // Cache the mounted volume's GEOMETRY across requests (the BPB fields are stable for the boot;
    // the FAT/dir/data are re-read fresh per op through the block layer, so a create/delete is always
    // seen). Lazily mounted on the first file op. Raw ops never touch it.
    let mut fs: Option<crate::fs::fat::FatFs> = None;
    loop {
        // Block until a request is queued. A counting semaphore, so each `wait()` return pairs with
        // exactly one queued request; a (should-not-happen) empty pop just loops back to wait.
        if !REQ_READY.wait() {
            // Off a scheduled task — impossible for this spawned task, but never busy-spin.
            sched::yield_now();
            continue;
        }
        let addr = REQ_QUEUE.lock().pop_front();
        let Some(addr) = addr else { continue };
        let req = addr as *mut BlockRequest;
        // SAFETY: the submitter is blocked in `done.wait()`, so `*req` is pinned + live until we post.
        let result = unsafe { service_one(&mut *req, &mut fs) };
        unsafe {
            (*req).result.store(result, Ordering::Release);
            (*req).done.post();
        }
    }
}

/// Mount the volume once, caching its geometry; return a reference for the file ops. `None` if no FAT
/// volume is present (the caller then fails the request `-EIO`).
fn mounted<'a>(fs: &'a mut Option<crate::fs::fat::FatFs>) -> Option<&'a crate::fs::fat::FatFs> {
    if fs.is_none() {
        *fs = crate::fs::fat::mount().ok();
    }
    fs.as_ref()
}

/// Run one request to completion at IF=1 and return the submitter's `result`. Raw sector ops go
/// through the block layer; file ops resolve the name on the live volume and use `fat.rs` (both
/// lock the controller + drive the BOT pump, which wakes because we run IF=1). A timeout surfaces as
/// `BlockError::Io`/`FatError` -> `EIO` — never a hang.
fn service_one(req: &mut BlockRequest, fs: &mut Option<crate::fs::fat::FatFs>) -> i32 {
    match req.op {
        BlockOp::Read => {
            let buf = unsafe { core::slice::from_raw_parts_mut(req.buf, req.len) };
            match block::read_block(req.lba, buf) {
                Ok(n) => n as i32,
                Err(_) => EIO,
            }
        }
        BlockOp::Write => {
            let buf = unsafe { core::slice::from_raw_parts(req.buf, req.len) };
            match block::write_block(req.lba, buf) {
                Ok(()) => req.len as i32,
                Err(_) => EIO,
            }
        }
        BlockOp::ReadFile => service_read_file(req, fs),
    }
}

/// S2: a live in-place file-range read. Resolve `name` on the live volume, then `read_at` its
/// on-disk chain at `[offset, offset+len)` into the kernel bounce buffer. Returns the byte count
/// delivered (`>= 0`), or `-EIO` (no volume / not found / a directory / an I/O error). The syscall
/// already clamped `len` to the descriptor's EOF; `read_at` re-clamps to the live on-disk size.
fn service_read_file(req: &mut BlockRequest, fs: &mut Option<crate::fs::fat::FatFs>) -> i32 {
    let Some(fs) = mounted(fs) else { return EIO };
    let Ok(name) = core::str::from_utf8(&req.name[..req.name_len]) else { return EIO };
    let (de, _lba, _off) = match fs.find_located(name) {
        Ok(loc) if !loc.0.is_dir => loc,
        _ => return EIO,
    };
    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs
        .read_at(de.first_cluster(), de.size, req.offset, &mut out, req.len)
        .is_err()
    {
        return EIO;
    }
    let n = out.len().min(req.len);
    unsafe {
        core::ptr::copy_nonoverlapping(out.as_ptr(), req.buf, n);
    }
    n as i32
}

/// Spawn the storage service task once, on an online AP, when a block device is present. Idempotent
/// (one-shot). Called from the x86 main loop under the `irqstorage` knob — BEFORE any storage syscall
/// can submit (the storage fixtures gate on the same block-device presence and run later in the loop).
/// Returns `true` once the task is (or was already) running.
pub fn start_service_once() -> bool {
    if SERVICE_STARTED.load(Ordering::Acquire) {
        return true;
    }
    if block::info().is_none() {
        return false; // retry next loop iteration until storage enumerates
    }
    let online = crate::arch::smp::online_aps();
    let Some(&cpu) = online.first() else {
        return false; // no AP to host the service task yet
    };
    if SERVICE_STARTED.swap(true, Ordering::AcqRel) {
        return true; // lost the race — another pass already spawned it
    }
    REQ_READY.init(); // reserve the service task's wait-list before it can block
    {
        let mut q = REQ_QUEUE.lock();
        q.reserve(REQ_QUEUE_CAP); // never reallocate under the spinlock
    }
    sched::spawn("storage-svc", service_task, 0, cpu, PRIO_NORMAL);
    serial_println!(
        ":: STOR-1: storage service task live on cpu {} (irqstorage) ::",
        cpu
    );
    true
}

/// The S1 witness: `bx-blockreq`. A one-shot kernel self-test that reads a known sector BOTH directly
/// (polled `block::read_block`, IF=1) AND through a `BlockRequest` submitted to the service task, then
/// compares — proving the submit/block/complete handshake delivers the same bytes as the polled path.
/// Spawned once from the main loop after the service task is up. Prints a PASS/FAIL line.
pub fn selftest_once() {
    if SELFTEST_DONE.load(Ordering::Acquire) {
        return;
    }
    if !SERVICE_STARTED.load(Ordering::Acquire) || block::info().is_none() {
        return; // service not up yet — retry next pass
    }
    let online = crate::arch::smp::online_aps();
    // Host the self-test on a DIFFERENT AP than the service task when possible, so the submit crosses
    // cores (the metal wake path); with one AP they coincide and cooperate on that core.
    let cpu = online.get(1).or_else(|| online.first()).copied();
    let Some(cpu) = cpu else { return };
    if SELFTEST_DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    sched::spawn("bx-blockreq", selftest_task, 0, cpu, PRIO_NORMAL);
}

/// The self-test verdict for a stable re-print / metal check: `0` = not run, `1` = PASS, `2` = FAIL.
pub fn selftest_verdict() -> u8 {
    SELFTEST_RESULT.load(Ordering::Acquire)
}

/// Whether the storage service task is running — a syscall must only `submit()` once it is, else the
/// request would queue with no drainer. The storage syscalls (S2..S4) gate their live-routing on this
/// (AND a mounted volume), falling back to the staged path until the service task is up.
pub fn service_ready() -> bool {
    SERVICE_STARTED.load(Ordering::Acquire)
}

/// The self-test body (a scheduled kernel task, IF=1). LBA 0 (the boot sector / BPB — always present
/// on any enumerated volume) is read twice and compared.
fn selftest_task(_: usize) {
    const LBA: u64 = 0;
    let bs = match block::info() {
        Some(d) => (d.block_size as usize).min(512),
        None => {
            serial_println!(":: bx-blockreq: no block device — self-test skipped ::");
            return;
        }
    };
    // 1) The polled reference read (IF=1, directly through the block layer).
    let mut polled = [0u8; 512];
    let polled_ok = block::read_block(LBA, &mut polled[..bs]).is_ok();

    // 2) The SAME sector through a BlockRequest submitted to the service task (submit -> block on the
    //    completion -> resume). Proves the handshake delivers the same bytes as the polled path.
    let mut via = [0u8; 512];
    let mut req = BlockRequest::new_raw(BlockOp::Read, LBA, via.as_mut_ptr(), bs);
    req.done.init(); // reserve the single-waiter capacity before we block on it
    let rc = unsafe { submit(&mut req as *mut BlockRequest) };

    let matches = polled_ok && rc == bs as i32 && polled[..bs] == via[..bs];
    SELFTEST_RESULT.store(if matches { 1 } else { 2 }, Ordering::Release);
    if matches {
        serial_println!(
            ":: bx-blockreq: PASS — service-task read of LBA {} matched the polled read ({} bytes) ::",
            LBA, bs
        );
    } else {
        serial_println!(
            ":: bx-blockreq: FAIL — polled_ok={} rc={} equal={} ::",
            polled_ok, rc, polled[..bs] == via[..bs]
        );
    }
}
