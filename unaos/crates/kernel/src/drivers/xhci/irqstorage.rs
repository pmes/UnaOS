// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// STOR-1 (S1): the storage SERVICE TASK + the BlockRequest submit/complete handshake — the
// interrupt-driven x86 storage spine. See unaos/docs/dev/OS/07_USB_STORAGE/x86_interrupt_storage.md.
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
/// resolved BY NAME on the live volume). S3: `WriteFile` (in-place write-through). S4: the ALLOCATOR
/// ops (`Create`/`Grow`/`Delete`) — grow/create/delete run SYNCHRONOUSLY in-syscall via `fat.rs`, out
/// of the IF-masked handler, retiring the U10x deferred op-queue when the knob is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockOp {
    /// Raw sector read: read one block at `lba` into `buf[..len]`.
    Read,
    /// Raw sector write: write `buf[..len]` (zero-padded to the block size) to the block at `lba`.
    Write,
    /// File-range read: resolve `name` on the live volume (`find_located`), then `read_at(cluster,
    /// on-disk size, offset, .., len)` into `buf`. Returns the byte count read (`>= 0`), or `-EIO`.
    ReadFile,
    /// File-range write-through: resolve `name`, then `write_at(cluster, on-disk size, offset,
    /// buf[..len])` IN PLACE (never grows/allocs/touches the dir or FAT). Returns the byte count
    /// written (`>= 0`), or `-EIO`.
    WriteFile,
    /// S4a CREATE: idempotently create a 0-length root-directory entry for `name` — `find_located`
    /// FIRST (idempotent: an already-present name is left untouched), else `create_in_root(name,
    /// 0x20)`. Allocates NO clusters, touches only the one directory sector. Returns `0` (present or
    /// created) or `-EIO`.
    Create,
    /// S4b GROW: resolve `name`, then `write_grow(first_cluster, on-disk size, dir_lba, dir_off,
    /// offset, buf[..len])` — extend the file past EOF (alloc + zero-fill + chain, RMW the data, bump
    /// the dir size LAST), or overwrite in place when the write stays within EOF. Returns the byte
    /// count written (`>= 0`), or `-EIO`.
    Grow,
    /// S4c DELETE: resolve `name`, then `delete_located(dir_lba, dir_off, first_cluster)` (dir entry
    /// `0xE5` FIRST, then free the whole chain in all FAT copies). Idempotent — a `name` already
    /// absent returns `0`. Returns `0` (deleted or already gone) or `-EIO`.
    Delete,
    /// S7 STAT: resolve `name` on the live volume (`find_located`) and return its ON-DISK SIZE (`>= 0`,
    /// bytes) — the one fact the IF-masked syscall handler cannot get itself (it can't mount/walk the
    /// FAT). `sys_open`'s arbitrary-file path submits this to size the read-only descriptor it opens over
    /// a PRE-EXISTING on-disk file that is neither staged nor a U10 name. `-ENOENT` (absent, or a
    /// directory — the file-open path never opens a dir), `-EIO` (no volume / I/O error / a size that does
    /// not fit the `i32` result channel). Allocates nothing, touches no sector beyond the directory walk.
    Stat,
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
/// S7: "no such file" — a `Stat` of a name absent from the live volume (or naming a directory). Distinct
/// from `EIO` so the syscall layer maps it to `-ENOENT` (name not found) vs. `-EIO` (a real I/O error).
/// Matches the syscall layer's `ENOENT`.
const ENOENT: i32 = -2;

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
/// WITCORE: one-shot guard for the "no lock-safe core" decline line. `start_service_once` is called
/// once per device-service pass, so the decline must announce itself exactly once, not per pass.
static SERVICE_DECLINED: AtomicBool = AtomicBool::new(false);
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
    // Push the address, then wake the service task. The REQ_QUEUE hold MUST be IRQ-masked: the service
    // task pops it at IF=1 and is timer-preemptible there, and run queues are per-CPU with no steal, so
    // a same-core submitter spinning at IF=0 against a preempted holder would deadlock the core (the
    // `sched.rs:31` "a lock holder can't be preempted" invariant a plain spinlock relies on). Mask BOTH
    // sides, scoped to the O(1) push — the mask ENDS here, never across the pump's `hlt`. A syscall
    // submitter already runs IF=0 (no-op); this closes the window for an IF=1 kernel-task submitter
    // (the self-test / any future one).
    x86_64::instructions::interrupts::without_interrupts(|| {
        REQ_QUEUE.lock().push_back(req as usize);
    });
    REQ_READY.post();
    // Block until the service task posts. On a scheduled task this restores the caller's IF snapshot
    // across the switch (IF-safe even though the handler entered IF=0). A submitter MUST be a scheduled
    // task — an off-scheduler `false` (no `current` to block) would leave the request queued for the
    // service task to DMA/post into a since-freed stack frame, so fail loud rather than dangle it
    // (unreachable today: every caller runs on an AP task).
    assert!(
        unsafe { (*req).done.wait() },
        "irqstorage::submit called off a scheduled task — the request would dangle"
    );
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

/// S3 helper the syscall layer calls: write `len` bytes at byte `offset` of file `name` on the LIVE
/// volume, IN PLACE, from the KERNEL buffer `kbuf` (into which the handler has already bounced the
/// ring-3 source). Returns the byte count written (`>= 0`) or `-EIO`. Blocks the caller on the
/// service task.
///
/// SAFETY: `kbuf` must be a live kernel buffer of at least `len` bytes on the caller's stack. The
/// caller must be a scheduled task.
pub unsafe fn submit_write_file(name: &[u8], offset: u32, kbuf: *mut u8, len: usize) -> i32 {
    let mut req = BlockRequest::new_file(BlockOp::WriteFile, name, offset, kbuf, len);
    req.done.init();
    unsafe { submit(&mut req as *mut BlockRequest) }
}

/// S4a helper the syscall layer calls: idempotently CREATE a 0-length root-directory entry for `name`
/// on the LIVE volume. Returns `0` (present or created) or `-EIO`. Blocks the caller on the service
/// task. No data buffer (a 0-length file owns no clusters).
///
/// SAFETY: the caller must be a scheduled task (so it can block on the service task).
pub unsafe fn submit_create(name: &[u8]) -> i32 {
    let mut req = BlockRequest::new_file(BlockOp::Create, name, 0, core::ptr::null_mut(), 0);
    req.done.init();
    unsafe { submit(&mut req as *mut BlockRequest) }
}

/// S4b helper the syscall layer calls: GROW file `name` on the LIVE volume — write `len` bytes at byte
/// `offset` from the KERNEL buffer `kbuf` (into which the handler has already bounced the ring-3
/// source), extending the file (alloc + chain) when the write runs past EOF. Returns the byte count
/// written (`>= 0`) or `-EIO`. Blocks the caller on the service task.
///
/// SAFETY: `kbuf` must be a live kernel buffer of at least `len` bytes on the caller's stack. The
/// caller must be a scheduled task.
pub unsafe fn submit_grow(name: &[u8], offset: u32, kbuf: *mut u8, len: usize) -> i32 {
    let mut req = BlockRequest::new_file(BlockOp::Grow, name, offset, kbuf, len);
    req.done.init();
    unsafe { submit(&mut req as *mut BlockRequest) }
}

/// S4c helper the syscall layer calls: DELETE file `name` from the LIVE volume (`0xE5` the dir entry,
/// free the chain in all FAT copies). Idempotent — a `name` already absent returns `0`. Returns `0`
/// (deleted or already gone) or `-EIO`. Blocks the caller on the service task.
///
/// SAFETY: the caller must be a scheduled task (so it can block on the service task).
pub unsafe fn submit_delete(name: &[u8]) -> i32 {
    let mut req = BlockRequest::new_file(BlockOp::Delete, name, 0, core::ptr::null_mut(), 0);
    req.done.init();
    unsafe { submit(&mut req as *mut BlockRequest) }
}

/// S7 helper the syscall layer calls: STAT file `name` on the LIVE volume — return its on-disk size
/// (`>= 0`, bytes) or a negative errno (`-ENOENT` absent / a directory, `-EIO` no volume / I/O error /
/// size overflow). Blocks the caller on the service task. No data buffer (a stat reads only the
/// directory). The arbitrary-file `sys_open` path uses the size to bound the read-only descriptor's EOF.
///
/// SAFETY: the caller must be a scheduled task (so it can block on the service task).
pub unsafe fn submit_stat(name: &[u8]) -> i32 {
    let mut req = BlockRequest::new_file(BlockOp::Stat, name, 0, core::ptr::null_mut(), 0);
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
        // MF1: mask IF across the O(1) pop (see `submit`) — the service task runs IF=1 and is
        // preemptible, so a plain spinlock hold here could be preempted and deadlock a same-core
        // submitter. The mask ends BEFORE `service_one` — the pump's `hlt` runs IF=1 (the whole point).
        let addr = x86_64::instructions::interrupts::without_interrupts(|| REQ_QUEUE.lock().pop_front());
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
        BlockOp::WriteFile => service_write_file(req, fs),
        BlockOp::Create => service_create_file(req, fs),
        BlockOp::Grow => service_grow_file(req, fs),
        BlockOp::Delete => service_delete_file(req, fs),
        BlockOp::Stat => service_stat_file(req, fs),
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

/// S3: a live in-place file-range write-through. Resolve `name` on the live volume, then `write_at`
/// its on-disk chain at `[offset, offset+len)` from the kernel bounce buffer. `write_at` is strictly
/// bounded — it never grows the file, allocs/frees clusters, or touches the directory/FAT — so this
/// persists a bounded overwrite synchronously. Returns the byte count written (`>= 0`), or `-EIO`.
fn service_write_file(req: &mut BlockRequest, fs: &mut Option<crate::fs::fat::FatFs>) -> i32 {
    let Some(fs) = mounted(fs) else { return EIO };
    let Ok(name) = core::str::from_utf8(&req.name[..req.name_len]) else { return EIO };
    let (de, _lba, _off) = match fs.find_located(name) {
        Ok(loc) if !loc.0.is_dir => loc,
        _ => return EIO,
    };
    let data = unsafe { core::slice::from_raw_parts(req.buf, req.len) };
    match fs.write_at(de.first_cluster(), de.size, req.offset, data) {
        Ok(w) => w as i32,
        Err(_) => EIO,
    }
}

/// S4a: idempotently CREATE a 0-length root-directory entry for `name` on the live volume. `find_located`
/// FIRST — an already-present name (a re-submit, or a metal card that still holds it) is left untouched
/// (the `create_in_root` "caller must confirm absent" contract; never plant a duplicate 8.3 slot) — else
/// `create_in_root(name, 0x20)`. Allocates NO clusters. Returns `0` (present or created) or `-EIO`.
fn service_create_file(req: &mut BlockRequest, fs: &mut Option<crate::fs::fat::FatFs>) -> i32 {
    let Some(fs) = mounted(fs) else { return EIO };
    let Ok(name) = core::str::from_utf8(&req.name[..req.name_len]) else { return EIO };
    match fs.find_located(name) {
        Ok(_) => 0, // already present — idempotent, no duplicate
        Err(crate::fs::fat::FatError::NotFound) => match fs.create_in_root(name, 0x20) {
            Ok(_) => 0,
            Err(_) => EIO,
        },
        Err(_) => EIO, // a real I/O / mount error — do NOT create over it
    }
}

/// S4b: GROW file `name` on the live volume via `write_grow` — resolve its on-disk directory location +
/// chain head + size, then extend (alloc + zero-fill + chain, RMW the data, bump the dir size LAST) or
/// overwrite in place. Bounded by the caller's one-page grow. Returns the byte count written (`>= 0`) or
/// `-EIO` (no volume / not found / a directory / an I/O error). `write_grow` publishes the new size +
/// chain head to the directory itself.
fn service_grow_file(req: &mut BlockRequest, fs: &mut Option<crate::fs::fat::FatFs>) -> i32 {
    let Some(fs) = mounted(fs) else { return EIO };
    let Ok(name) = core::str::from_utf8(&req.name[..req.name_len]) else { return EIO };
    let (de, lba, off) = match fs.find_located(name) {
        Ok(loc) if !loc.0.is_dir => loc,
        _ => return EIO,
    };
    let data = unsafe { core::slice::from_raw_parts(req.buf, req.len) };
    match fs.write_grow(de.first_cluster(), de.size, lba, off, req.offset, data) {
        Ok((w, _ns, _nf)) => w as i32,
        Err(_) => EIO,
    }
}

/// S4c: DELETE file `name` from the live volume via `delete_located` (dir entry `0xE5` FIRST, then free
/// the whole chain in ALL FAT copies — crash-safe order). IDEMPOTENT: a `name` already absent returns
/// `0` (the last-close delete of an already-reaped file, or a metal re-run), so a benign double-delete
/// never surfaces as an error. Returns `0` (deleted or already gone) or `-EIO` (an I/O error).
fn service_delete_file(req: &mut BlockRequest, fs: &mut Option<crate::fs::fat::FatFs>) -> i32 {
    let Some(fs) = mounted(fs) else { return EIO };
    let Ok(name) = core::str::from_utf8(&req.name[..req.name_len]) else { return EIO };
    match fs.find_located(name) {
        Ok((de, lba, off)) if !de.is_dir => match fs.delete_located(lba, off, de.first_cluster()) {
            Ok(_) => 0,
            Err(_) => EIO,
        },
        Ok(_) => EIO, // a directory under this name — never delete it via the file path
        Err(crate::fs::fat::FatError::NotFound) => 0, // already gone — idempotent
        Err(_) => EIO,
    }
}

/// S7: STAT file `name` on the live volume — resolve it (`find_located`) and return its on-disk SIZE
/// (`>= 0`, bytes), the EOF bound `sys_open`'s arbitrary-file path gives the read-only descriptor it
/// opens. A directory under this name is `-ENOENT` (the file path never opens a dir, mirroring
/// `service_read_file`'s `!is_dir` gate). A size that would not fit the `i32` result channel (`>
/// i32::MAX`) is `-EIO` — a >2 GiB arbitrary file is not openable via this path (documented S7 bound;
/// no demo/test file approaches it). Absent -> `-ENOENT`; a real mount/I/O error -> `-EIO`. Reads only
/// the directory (via `find_located`), allocates nothing.
fn service_stat_file(req: &mut BlockRequest, fs: &mut Option<crate::fs::fat::FatFs>) -> i32 {
    let Some(fs) = mounted(fs) else { return EIO };
    let Ok(name) = core::str::from_utf8(&req.name[..req.name_len]) else { return EIO };
    match fs.find_located(name) {
        Ok((de, _lba, _off)) if !de.is_dir => {
            if de.size > i32::MAX as u32 {
                return EIO; // the size does not fit the i32 result channel — not openable via S7
            }
            de.size as i32 // >= 0: the on-disk byte size
        }
        Ok(_) => ENOENT, // a directory under this name — not an openable file
        Err(crate::fs::fat::FatError::NotFound) => ENOENT, // absent from the live volume
        Err(_) => EIO,   // a real mount / I/O error
    }
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
    // WITCORE: `xhci_worker_cpu`, NOT `online_aps().first()`. `service_one` -> `block::read_block`
    // takes the raw `XHCI_CONTROLLER` spin lock, and this task is PREEMPTIBLE — so it is exactly the
    // class SCHED-X86's rule 1 forbids co-locating with another preemptible taker. `.first()` named
    // the RENDER core (which takes the lock through FAT reads / `pal::pump_and_poll` / `usbinfo`),
    // and `.last()` is the pump's core, which takes it directly. `None` means no core is free of
    // both: DECLINE rather than start into a deadlock — the syscall layer's staged fallback covers a
    // missing service task (`service_ready()` gates every live route).
    let Some(cpu) = crate::arch::smp::xhci_worker_cpu(0) else {
        if !SERVICE_DECLINED.swap(true, Ordering::AcqRel) {
            serial_println!(
                ":: STOR-1: no core free of the render + service cores — storage service DECLINED (XHCI_CONTROLLER rule) ::"
            );
        }
        return false; // no lock-safe AP to host the service task
    };
    if SERVICE_STARTED.swap(true, Ordering::AcqRel) {
        return true; // lost the race — another pass already spawned it
    }
    REQ_READY.init(); // reserve the service task's wait-list before it can block
    // MF1: mask IF for uniformity with the push/pop sites (this runs once on the BSP before the service
    // task exists, so it can't deadlock — but keep every REQ_QUEUE hold masked by the same rule).
    x86_64::instructions::interrupts::without_interrupts(|| {
        REQ_QUEUE.lock().reserve(REQ_QUEUE_CAP); // never reallocate under the spinlock
    });
    // STOR-1 S5: the service task runs at PRIO_HIGH, NOT PRIO_NORMAL. It is a SYSTEM SERVICE that arbitrary
    // ring-3 tasks BLOCK on (a `submit` sleeps its caller until the service task completes the transfer), so it
    // must PREEMPT a lower-priority user task the moment a request lands — otherwise a cooperatively-spinning
    // ring-3 fixture co-located on the service task's core (a PRIO_NORMAL peer that never yields its round-robin
    // slot — e.g. u6gx's owner A parked on its GO word while the grantee B, on another core, awaits a live read)
    // STARVES the service task, and B's cross-core read never completes (a deadlock: the launcher waits on B, B
    // waits on the service task, the service task waits behind A's spin). A cross-core wake only sends the preempt
    // IPI when the new task out-ranks the running one (`poke_for`: `prio > running`), so PRIO_HIGH is what makes
    // the wake preempt. The task does BOUNDED work per request then blocks on `REQ_READY` (off every run queue),
    // so it never hogs a core when idle — high priority buys prompt I/O completion, not monopoly.
    sched::spawn("storage-svc", service_task, 0, cpu, sched::PRIO_HIGH);
    serial_println!(
        ":: STOR-1: storage service task live on cpu {} (irqstorage, PRIO_HIGH) ::",
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
    // Host the self-test on a DIFFERENT AP than the service task, so the submit crosses cores (the
    // metal wake path).
    //
    // WITCORE: from the xHCI-safe pool, not `online_aps()`, and STRICTLY the second entry.
    // `selftest_task` reads the sector BOTH through the service task AND directly via the polled
    // `block::read_block` — so it, too, is a preemptible `XHCI_CONTROLLER` taker, bound by the same
    // rule as the service task above. The old `.get(1).or(first())` fallback ("with one AP they
    // coincide and cooperate on that core") predates SCHED-X86 pinning the pool's head to the render
    // service; co-locating the two takers is now the rule-1 deadlock, not a degradation. Skip loudly
    // instead — `selftest_verdict()` stays 0 ("not run"), which is what a skipped witness must read.
    let Some(cpu) = crate::arch::smp::xhci_worker_cpu(1) else {
        if !SELFTEST_DONE.swap(true, Ordering::AcqRel) {
            serial_println!(
                ":: STOR-1: bx-blockreq SKIPPED — no second core free of render + service (XHCI_CONTROLLER rule) ::"
            );
        }
        return;
    };
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
