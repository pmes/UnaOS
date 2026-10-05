// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//! SELFBUILD2 M1 (B349) — Linux THREADS and FUTEX for the Linux ABI shim (the compatibility box).
//!
//! A thread is one more scheduler task (`spawn_user_preemptible`, [`TASK_NAME`], the process PML4, pinned to the session's core)
//! sharing everything the process owns: `cur_info()` resolves the same `ProcInfo` by CR3, so the address space, the fd table,
//! brk/mmap and cwd are one `LinuxProc` behind one mutex. What a thread owns ALONE is keyed by its THREAD KEY: the leader's key
//! is the PML4 (so every single-threaded process is exactly as before), thread *i*'s is `PML4 + i` (the PML4 is page-aligned,
//! the low 12 bits are free). [`key_for`] maps the CURRENT scheduler task to its key through a lock-free table (the `FS_TAB`
//! shape) and is what the three per-task hooks ask: FS_BASE (`on_dispatch`), the FXSAVE slot (`fpu::dispatch`) and the
//! first-entry register file (`take_fork_regs`).
//!
//! Lifetimes: `exit` (60) of a thread ends that task only (CHILD_CLEARTID: zero the word, FUTEX_WAKE one); the LAST live thread's
//! `exit`, or any `exit_group` (231), ends the process — the others are killed at their next kill boundary, and the address space
//! is freed only when every thread task is gone ([`all_gone`]). A fatal signal or a ring-3 fault takes the whole group.
//!
//! Futex: a per-session wait table keyed by `(PML4, uaddr)` — private futexes; every thread of a process is on one core and a
//! syscall runs with IF masked under the process mutex, so "check the word, then queue" is atomic against every waker. A
//! waiter is a RETRY loop (the shim's blocking shape): the first call queues it, later calls return 0 once woken or
//! `-ETIMEDOUT` past the deadline (`arch::ms`, the clock `clock_gettime` reports).

use super::proc::{self, ProcInfo};
use super::{fd, sys, LinuxProc, TASK_NAME};
use crate::arch::sched::KillSwitch;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Live non-leader threads per session (each takes an `FS_TAB` entry and an FXSAVE slot; 16 processes take the rest).
pub const MAX_THREADS: usize = 12;
const EAGAIN: i64 = 11;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;
const ENOSYS: i64 = 38;
const ETIMEDOUT: i64 = 110;

pub struct Thr {
    pub pid: u32,
    pub tid: u32,
    pub leader: bool,
    /// Scheduler task id (`sched::current_id`).
    pub task: AtomicU64,
    pub key: u64,
    pub key_live: AtomicBool,
    pub kill: Arc<KillSwitch>,
    pub clear_tid: AtomicU64,
    pub exited: AtomicBool,
    /// This thread's nanosleep/poll/epoll deadline + 1 (0 = none), swapped into `LinuxProc::sleep_until` around each syscall.
    sleep: AtomicU64,
}

impl Thr {
    fn gone(&self) -> bool {
        self.exited.load(Ordering::Acquire) || self.kill.is_reaped()
    }
}

static THREADS: spin::Mutex<Vec<Arc<Thr>>> = spin::Mutex::new(Vec::new());
static NTHR: AtomicUsize = AtomicUsize::new(0);
/// Scheduler task id -> thread key, for non-leader threads (lock-free: read at the dispatch site).
static KEYTAB: [(AtomicU64, AtomicU64); 32] = [const { (AtomicU64::new(0), AtomicU64::new(0)) }; 32];
static NKEY: AtomicUsize = AtomicUsize::new(0);
/// `set_tid_address` of a leader that has no thread entry yet: `(pid, va)`.
static LEADER_CTID: spin::Mutex<Vec<(u32, u64)>> = spin::Mutex::new(Vec::new());
pub static SPAWNED: AtomicU64 = AtomicU64::new(0);
pub static FUTEX_BLOCKED: AtomicU64 = AtomicU64::new(0);

fn with<R>(f: impl FnOnce(&mut Vec<Arc<Thr>>) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = THREADS.lock();
        let r = f(&mut g);
        NTHR.store(g.len(), Ordering::Release);
        r
    })
}

fn of_pid(pid: u32) -> Vec<Arc<Thr>> {
    if NTHR.load(Ordering::Acquire) == 0 {
        return Vec::new();
    }
    with(|v| v.iter().filter(|t| t.pid == pid).cloned().collect())
}

fn keytab_set(id: u64, key: u64) -> bool {
    for (t, k) in KEYTAB.iter() {
        if t.compare_exchange(0, u64::MAX, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            k.store(key, Ordering::Release);
            t.store(id, Ordering::Release);
            NKEY.fetch_add(1, Ordering::AcqRel);
            return true;
        }
    }
    false
}

fn keytab_clear(id: u64) {
    if id == 0 {
        return;
    }
    for (t, k) in KEYTAB.iter() {
        if t.load(Ordering::Acquire) == id {
            k.store(0, Ordering::Release);
            t.store(0, Ordering::Release);
            NKEY.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// The thread key of the task CURRENT on this core, whose address space is `cr3`: `cr3` itself unless it is a non-leader thread.
/// Lock-free; called from the scheduler dispatch site (IF=0) and the first-entry trampoline.
pub fn key_for(cr3: u64) -> u64 {
    if NKEY.load(Ordering::Acquire) == 0 || cr3 == 0 {
        return cr3;
    }
    let Some(id) = crate::arch::sched::current_id() else { return cr3 };
    for (t, k) in KEYTAB.iter() {
        if t.load(Ordering::Acquire) == id {
            let key = k.load(Ordering::Acquire);
            if key & !0xfff == cr3 {
                return key;
            }
        }
    }
    cr3
}

/// The calling task's thread entry, if its process has threads.
pub fn cur(info: &ProcInfo) -> Option<Arc<Thr>> {
    if NTHR.load(Ordering::Acquire) == 0 {
        return None;
    }
    let id = crate::arch::sched::current_id()?;
    with(|v| v.iter().find(|t| t.pid == info.pid && t.task.load(Ordering::Acquire) == id).cloned())
}

fn ensure_leader(info: &Arc<ProcInfo>) -> Arc<Thr> {
    let id = crate::arch::sched::current_id().unwrap_or(0);
    let ctid = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut l = LEADER_CTID.lock();
        let i = l.iter().position(|(p, _)| *p == info.pid);
        i.map(|i| l.swap_remove(i).1).unwrap_or(0)
    });
    with(|v| {
        if let Some(t) = v.iter().find(|t| t.pid == info.pid && t.leader) {
            return t.clone();
        }
        let t = Arc::new(Thr {
            pid: info.pid,
            tid: info.pid,
            leader: true,
            task: AtomicU64::new(id),
            key: info.pml4,
            key_live: AtomicBool::new(true),
            kill: info.kill.clone(),
            clear_tid: AtomicU64::new(ctid),
            exited: AtomicBool::new(false),
            sleep: AtomicU64::new(0),
        });
        v.push(t.clone());
        t
    })
}

/// The process (tgid) a thread id belongs to (`tgkill`/`tkill` name threads).
pub fn tgid_of(tid: u32) -> Option<u32> {
    if NTHR.load(Ordering::Acquire) == 0 {
        return None;
    }
    with(|v| v.iter().find(|t| t.tid == tid && !t.gone()).map(|t| t.pid))
}

/// `gettid` (186).
pub fn gettid(info: &ProcInfo) -> i64 {
    cur(info).map_or(info.pid, |t| t.tid) as i64
}

/// `set_tid_address(tidptr)` (218): remembered for CHILD_CLEARTID-at-exit; returns the caller's tid.
pub fn set_tid_address(info: &ProcInfo, va: u64) -> i64 {
    if let Some(t) = cur(info) {
        t.clear_tid.store(va, Ordering::Release);
        return t.tid as i64;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut l = LEADER_CTID.lock();
        l.retain(|(p, _)| *p != info.pid);
        if va != 0 {
            l.push((info.pid, va));
        }
    });
    info.pid as i64
}

/// Per-thread sleep deadline in / out of `LinuxProc::sleep_until` around one syscall (no-op for a process without threads).
pub fn sleep_in(t: &Option<Arc<Thr>>, p: &mut LinuxProc) {
    if let Some(t) = t {
        let v = t.sleep.load(Ordering::Acquire);
        p.sleep_until = if v == 0 { None } else { Some(v - 1) };
    }
}

pub fn sleep_out(t: &Option<Arc<Thr>>, p: &mut LinuxProc) {
    if let Some(t) = t {
        t.sleep.store(p.sleep_until.take().map_or(0, |v| v + 1), Ordering::Release);
    }
}

#[inline]
fn fw(ktop: u64, off: u64) -> u64 {
    unsafe { *((ktop - off) as *const u64) }
}

fn put_u32(p: &LinuxProc, va: u64, v: u32) -> bool {
    p.asp.copy_out(va, &v.to_le_bytes(), false)
}

fn live_threads() -> usize {
    with(|v| v.iter().filter(|t| !t.leader && !t.gone()).count())
}

/// `clone(flags, stack, ptid, ctid, tls)` / `clone3` with CLONE_THREAD: a new task in this process. `regs` = the caller's
/// rdi, rsi, rdx, r10, r8, r9 at the syscall (the child gets them back, as Linux gives it every register but rax: glibc's
/// clone3 keeps the thread function in rdx and its argument in r8).
#[allow(clippy::too_many_arguments)]
pub fn clone_thread(
    p: &mut LinuxProc,
    info: &Arc<ProcInfo>,
    ktop: u64,
    flags: u64,
    stack: u64,
    ptid: u64,
    ctid: u64,
    tls: u64,
    regs: [u64; 6],
) -> i64 {
    const VM: u64 = 0x100;
    const FS: u64 = 0x200;
    const FILES: u64 = 0x400;
    const SIGHAND: u64 = 0x800;
    const THREAD: u64 = 0x10000;
    const SETTLS: u64 = 0x80000;
    const PARENT_SETTID: u64 = 0x10_0000;
    const CHILD_CLEARTID: u64 = 0x20_0000;
    const CHILD_SETTID: u64 = 0x100_0000;
    let need = VM | FS | FILES | SIGHAND | THREAD;
    if flags & need != need {
        return -EINVAL; // a thread that does not share fs/files/handlers: this layer has one LinuxProc per address space
    }
    if flags & SETTLS != 0 && x86_64::VirtAddr::try_new(tls).is_err() {
        return -EINVAL;
    }
    if live_threads() >= MAX_THREADS {
        return -EAGAIN;
    }
    let _leader = ensure_leader(info);
    let used: Vec<u64> = with(|v| v.iter().filter(|t| t.pid == info.pid && t.key_live.load(Ordering::Acquire)).map(|t| t.key).collect());
    let Some(idx) = (1..32u64).find(|i| !used.contains(&(info.pml4 + i))) else { return -EAGAIN };
    let key = info.pml4 + idx;
    super::remap::forget_key(key); // SELFBUILD5: a new thread starts with no alternate stack
    let fs = if flags & SETTLS != 0 { tls } else { super::fs_tab_get(key_for(info.pml4)).unwrap_or(p.fs_base) };
    super::fs_tab_set(key, fs);
    if super::fs_tab_get(key) != Some(fs) {
        super::fs_tab_clear(key);
        return -EAGAIN;
    }
    if !super::fpu::fork_into(key) {
        super::fs_tab_clear(key);
        return -EAGAIN; // no FXSAVE slot: refuse rather than run the thread without its x87/XMM state
    }
    let tid = proc::new_pid();
    let rip = fw(ktop, 32);
    let usp = if stack != 0 { stack } else { fw(ktop, 8) };
    let r = [
        fw(ktop, 40), fw(ktop, 48), fw(ktop, 56), fw(ktop, 64), fw(ktop, 72), fw(ktop, 80),
        regs[0], regs[1], regs[2], regs[3], regs[4], regs[5],
    ];
    super::push_fork_regs(key, &r);
    if flags & PARENT_SETTID != 0 && ptid != 0 {
        let _ = put_u32(p, ptid, tid);
    }
    if flags & CHILD_SETTID != 0 && ctid != 0 {
        let _ = put_u32(p, ctid, tid);
    }
    let kill = Arc::new(KillSwitch::new());
    let t = Arc::new(Thr {
        pid: info.pid,
        tid,
        leader: false,
        task: AtomicU64::new(0),
        key,
        key_live: AtomicBool::new(true),
        kill: kill.clone(),
        clear_tid: AtomicU64::new(if flags & CHILD_CLEARTID != 0 { ctid } else { 0 }),
        exited: AtomicBool::new(false),
        sleep: AtomicU64::new(0),
    });
    with(|v| v.push(t.clone()));
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    // The key must be published before the task is first dispatched: same core, pinned, IF masked here.
    x86_64::instructions::interrupts::without_interrupts(|| {
        let id = crate::arch::sched::spawn_user_preemptible(TASK_NAME, rip, usp, cpu, info.pml4, kill);
        t.task.store(id, Ordering::Release);
        keytab_set(id, key);
    });
    SPAWNED.fetch_add(1, Ordering::AcqRel);
    serial_println!("[linuxabi] thread pid={} tid={} key=+{} rip={:#x} sp={:#x} tls={:#x}", info.pid, tid, idx, rip, usp, fs);
    tid as i64
}

/// `clone3(cl_args, size)` (435): CLONE_THREAD = a thread, else the fork-shaped `clone` with the struct's fields.
pub fn clone3(p: &mut LinuxProc, info: &Arc<ProcInfo>, ktop: u64, a: [u64; 6]) -> i64 {
    if a[1] < 64 || a[1] > 4096 {
        return -EINVAL;
    }
    let mut b = [0u8; 64];
    if !p.asp.copy_in(a[0], &mut b) {
        return -EFAULT;
    }
    let f = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
    let (flags, child_tid, parent_tid, exit_signal, stack, stack_size, tls) = (f(0), f(2), f(3), f(4), f(5), f(6), f(7));
    if flags & 0x1000 != 0 || exit_signal > 64 {
        return -EINVAL; // CLONE_PIDFD: no pidfds here
    }
    let sp = if stack != 0 { stack.wrapping_add(stack_size) } else { 0 };
    if flags & 0x10000 != 0 {
        return clone_thread(p, info, ktop, flags, sp, parent_tid, child_tid, tls, a);
    }
    proc::clone(p, info, ktop, [flags | (exit_signal & 0xff), sp, parent_tid, child_tid, tls, 0])
}

fn release_key(t: &Thr) {
    if t.leader || !t.key_live.swap(false, Ordering::AcqRel) {
        return;
    }
    super::fs_tab_clear(t.key);
    super::fpu::release_slot(t.key);
    keytab_clear(t.task.load(Ordering::Acquire));
}

fn clear_tid_wake(info: &ProcInfo, t: &Thr) {
    let ctid = t.clear_tid.swap(0, Ordering::AcqRel);
    if ctid == 0 {
        return;
    }
    let ok = {
        let p = fd::lk(&info.lp);
        put_u32(&p, ctid, 0)
    };
    if ok {
        wake(info.pml4, ctid, 1, u32::MAX);
    }
}

/// `exit` / `exit_group` hook (from `dispatch`, before `proc::exit_current`). Returns when the PROCESS ends (exit_group, or the
/// last live thread's exit); a thread's `exit` with siblings still live ends the task here and never returns.
pub fn on_exit(info: &Arc<ProcInfo>, group: bool) {
    if NTHR.load(Ordering::Acquire) == 0 {
        return;
    }
    let me = cur(info);
    let others: Vec<Arc<Thr>> = of_pid(info.pid)
        .into_iter()
        .filter(|t| !t.gone() && me.as_ref().is_none_or(|m| !Arc::ptr_eq(m, t)))
        .collect();
    if group || others.is_empty() {
        for t in &others {
            t.kill.request(); // exit_group: the siblings leave at their next kill boundary
        }
        if let Some(m) = &me {
            m.exited.store(true, Ordering::Release);
        }
        return;
    }
    let Some(m) = me else {
        for t in &others {
            t.kill.request();
        }
        return;
    };
    clear_tid_wake(info, &m);
    m.exited.store(true, Ordering::Release);
    release_key(&m);
    crate::arch::sched::exit()
}

/// A fatal signal / ring-3 fault takes the whole thread group.
pub fn kill_group(pid: u32) {
    for t in of_pid(pid) {
        if !t.gone() {
            t.kill.request();
        }
    }
}

/// A ring-3 fault in the current task (it leaves through `ring3_fault_kill`, not a reap): mark it gone, then take the group.
pub fn fault_current(info: &ProcInfo) {
    if let Some(m) = cur(info) {
        m.exited.store(true, Ordering::Release);
        release_key(&m);
    }
    kill_group(info.pid);
}

/// gc's "was this process killed": the leader's switch alone without threads; with threads, every task gone and one reaped.
pub fn proc_reaped(info: &ProcInfo) -> bool {
    let ts = of_pid(info.pid);
    if ts.is_empty() {
        return info.kill.is_reaped();
    }
    ts.iter().any(|t| t.kill.is_reaped()) && ts.iter().all(|t| t.gone())
}

/// May the address space of `pid` be freed (no thread task can still run on it)?
pub fn all_gone(pid: u32) -> bool {
    of_pid(pid).iter().all(|t| t.gone())
}

/// Process freed: release every thread key, forget its threads, waiters and signal state.
pub fn release_all(pid: u32, pml4: u64) {
    for t in of_pid(pid) {
        release_key(&t);
    }
    with(|v| v.retain(|t| t.pid != pid));
    x86_64::instructions::interrupts::without_interrupts(|| LEADER_CTID.lock().retain(|(p, _)| *p != pid));
    fx(|v| v.retain(|w| w.pml4 != pml4));
    super::signal::forget(pid);
}

/// Session teardown: kill every thread still running and wait (until `deadline` ticks) for each to be gone.
pub fn wait_all(deadline: u64) -> bool {
    let all = with(|v| v.clone());
    for t in all.iter().filter(|t| !t.gone()) {
        t.kill.request();
    }
    for t in all.iter() {
        while !t.gone() && crate::arch::ticks() < deadline {
            crate::arch::sched::yield_now();
        }
    }
    all.iter().all(|t| t.gone())
}

/// `execve` from a threaded process: every other thread dies (Linux de-threads on exec).
pub fn exec_kill_others(info: &ProcInfo) {
    let me = cur(info);
    for t in of_pid(info.pid) {
        if !t.gone() && me.as_ref().is_none_or(|m| !Arc::ptr_eq(m, &t)) {
            t.kill.request();
        }
    }
}

pub fn reset() {
    with(|v| v.clear());
    for (t, k) in KEYTAB.iter() {
        k.store(0, Ordering::Release);
        t.store(0, Ordering::Release);
    }
    NKEY.store(0, Ordering::Release);
    x86_64::instructions::interrupts::without_interrupts(|| LEADER_CTID.lock().clear());
    fx(|v| v.clear());
    SPAWNED.store(0, Ordering::Release);
    FUTEX_BLOCKED.store(0, Ordering::Release);
}

// ---------------------------------------------------------------------------------------------
// futex
// ---------------------------------------------------------------------------------------------

struct Waiter {
    pml4: u64,
    uaddr: u64,
    bitset: u32,
    task: u64,
    woken: bool,
    /// `arch::ms` deadline, 0 = none.
    deadline: u64,
}

static FUTEX: spin::Mutex<Vec<Waiter>> = spin::Mutex::new(Vec::new());

fn fx<R>(f: impl FnOnce(&mut Vec<Waiter>) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| f(&mut FUTEX.lock()))
}

/// Wake up to `n` waiters on `(pml4, uaddr)` whose bitset meets `bs` (Linux wakes at least one when `n <= 0`). Returns the count.
pub fn wake(pml4: u64, uaddr: u64, n: i32, bs: u32) -> i64 {
    fx(|v| {
        let mut c = 0i64;
        for w in v.iter_mut() {
            if w.woken || w.pml4 != pml4 || w.uaddr != uaddr || w.bitset & bs == 0 {
                continue;
            }
            w.woken = true;
            c += 1;
            if c >= n as i64 {
                break;
            }
        }
        c
    })
}

fn requeue(pml4: u64, uaddr: u64, nwake: i32, nreq: i32, uaddr2: u64) -> i64 {
    fx(|v| {
        let (mut woke, mut moved) = (0i64, 0i64);
        for w in v.iter_mut() {
            if w.woken || w.pml4 != pml4 || w.uaddr != uaddr {
                continue;
            }
            if woke < nwake.max(0) as i64 {
                w.woken = true;
                woke += 1;
            } else if moved < nreq.max(0) as i64 {
                w.uaddr = uaddr2;
                moved += 1;
            } else {
                break;
            }
        }
        woke + moved
    })
}

/// A signal interrupted the calling task's wait: drop its waiter record.
pub fn cancel_wait() {
    let Some(id) = crate::arch::sched::current_id() else { return };
    fx(|v| v.retain(|w| w.task != id));
}

fn read_u32(p: &LinuxProc, va: u64) -> Option<u32> {
    let mut b = [0u8; 4];
    if p.asp.copy_in(va, &mut b) { Some(u32::from_le_bytes(b)) } else { None }
}

fn wait(p: &mut LinuxProc, pml4: u64, a: [u64; 6], bitset_op: bool) -> i64 {
    let task = crate::arch::sched::current_id().unwrap_or(0);
    let now = crate::arch::ms();
    // A retry of this task's queued wait?
    let again = fx(|v| {
        let i = v.iter().position(|w| w.task == task)?;
        if v[i].woken {
            v.remove(i);
            return Some(0);
        }
        if v[i].deadline != 0 && now >= v[i].deadline {
            v.remove(i);
            return Some(-ETIMEDOUT);
        }
        Some(sys::RETRY)
    });
    if let Some(r) = again {
        return r;
    }
    let bitset = if bitset_op { a[5] as u32 } else { u32::MAX };
    if bitset == 0 {
        return -EINVAL;
    }
    let Some(cur) = read_u32(p, a[0]) else { return -EFAULT };
    if cur != a[2] as u32 {
        return -EAGAIN;
    }
    let mut deadline = 0u64;
    if a[3] != 0 {
        let mut t = [0u8; 16];
        if !p.asp.copy_in(a[3], &mut t) {
            return -EFAULT;
        }
        let s = i64::from_le_bytes(t[0..8].try_into().unwrap_or([0; 8]));
        let ns = i64::from_le_bytes(t[8..16].try_into().unwrap_or([0; 8]));
        if s < 0 || !(0..1_000_000_000).contains(&ns) {
            return -EINVAL;
        }
        let ms = (s as u64).saturating_mul(1000).saturating_add((ns as u64).div_ceil(1_000_000));
        // FUTEX_WAIT: relative. FUTEX_WAIT_BITSET: absolute on the clock clock_gettime reports (every clock id reads `arch::ms`).
        deadline = if bitset_op { ms } else { now.saturating_add(ms) };
        if deadline <= now {
            return -ETIMEDOUT;
        }
    }
    fx(|v| v.push(Waiter { pml4, uaddr: a[0], bitset, task, woken: false, deadline }));
    FUTEX_BLOCKED.fetch_add(1, Ordering::AcqRel);
    sys::RETRY
}

/// `futex(uaddr, op, val, timeout|val2, uaddr2, val3)` (202). Private and shared futexes are the same here (one address space
/// per key; a futex shared across fork is not supported). Answered: WAIT, WAKE, REQUEUE, CMP_REQUEUE, WAIT_BITSET, WAKE_BITSET.
pub fn futex(p: &mut LinuxProc, info: &Arc<ProcInfo>, a: [u64; 6]) -> i64 {
    let op = a[1] & 0x7f; // strip FUTEX_PRIVATE_FLAG (128) and FUTEX_CLOCK_REALTIME (256)
    if a[0] & 3 != 0 {
        return -EINVAL;
    }
    match op {
        0 | 9 => wait(p, info.pml4, a, op == 9),
        1 | 10 => {
            let bs = if op == 10 { a[5] as u32 } else { u32::MAX };
            if bs == 0 {
                return -EINVAL;
            }
            wake(info.pml4, a[0], a[2] as u32 as i32, bs)
        }
        3 | 4 => {
            if a[4] & 3 != 0 {
                return -EINVAL;
            }
            if op == 4 {
                match read_u32(p, a[0]) {
                    Some(v) if v == a[5] as u32 => {}
                    Some(_) => return -EAGAIN,
                    None => return -EFAULT,
                }
            }
            requeue(info.pml4, a[0], a[2] as u32 as i32, a[3] as u32 as i32, a[4])
        }
        _ => {
            serial_println!("[linuxabi] futex op={} unanswered (-ENOSYS)", op);
            -ENOSYS
        }
    }
}
