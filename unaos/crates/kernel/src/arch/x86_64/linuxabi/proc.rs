// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! LINUXABI2 M2 — the process table and the process syscalls (fork, execve, wait4/waitid, kill, pgid, clone).
//!
//! Lock order: a process's own `lp` mutex may be held while taking [`TABLE`]; [`TABLE`] is NEVER held while locking an `lp`
//! (gc and wait4 snapshot the `Arc`s first). Everything wait4/kill/getppid need lives in [`ProcInfo`]'s atomics, so no
//! process ever locks another's `lp` except gc freeing a settled zombie.

use super::{elf, sys, AddrSpace, LinuxProc, BRK_BASE, MMAP_BASE, TASK_NAME};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

pub const MAX_PROCS: usize = 16;
pub const FORK_MAX_PAGES: usize = 16384; // 64 MiB eager-copy bound
const ECHILD: i64 = 10;
const EAGAIN: i64 = 11;
const ENOMEM: i64 = 12;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;
const ESRCH: i64 = 3;
const ENOENT: i64 = 2;
const ENOEXEC: i64 = 8;
const E2BIG: i64 = 7;
const WNOHANG: u64 = 1;
const LIVE: i64 = -1;

pub struct ProcInfo {
    pub pid: u32,
    pub pml4: u64,
    pub ppid: AtomicU32,
    pub pgid: AtomicU32,
    /// -1 = live; else the Linux wait status (exit code << 8, or the fatal signal number).
    pub state: AtomicI64,
    pub exit_tick: AtomicU64,
    pub killsig: AtomicU32,
    pub reaped: AtomicBool,
    pub freed: AtomicBool,
    pub kill: Arc<crate::arch::sched::KillSwitch>,
    pub lp: Arc<spin::Mutex<LinuxProc>>,
}

impl ProcInfo {
    pub fn is_live(&self) -> bool {
        self.state.load(Ordering::Acquire) == LIVE
    }
    /// Record the final status once (first writer wins).
    pub fn finish(&self, status: i64) {
        if self.state.compare_exchange(LIVE, status, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            self.exit_tick.store(crate::arch::ticks(), Ordering::Release);
        }
    }
}

static TABLE: spin::Mutex<Vec<Arc<ProcInfo>>> = spin::Mutex::new(Vec::new());
static NEXT_PID: AtomicU32 = AtomicU32::new(1);
pub static FORKS: AtomicU64 = AtomicU64::new(0);
pub static CHILD_EXIT_OK: AtomicU64 = AtomicU64::new(0);
pub static ROOT_PID: AtomicU32 = AtomicU32::new(0);

fn with_table<R>(f: impl FnOnce(&mut Vec<Arc<ProcInfo>>) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| f(&mut TABLE.lock()))
}

pub fn snapshot() -> Vec<Arc<ProcInfo>> {
    with_table(|t| t.clone())
}

pub fn cur_info() -> Option<Arc<ProcInfo>> {
    let cr3 = crate::arch::sched::current_user_cr3()?;
    if cr3 == 0 {
        return None;
    }
    with_table(|t| t.iter().find(|i| i.pml4 == cr3).cloned())
}

pub fn find(pid: u32) -> Option<Arc<ProcInfo>> {
    with_table(|t| t.iter().find(|i| i.pid == pid && !i.reaped.load(Ordering::Acquire)).cloned())
}

pub fn live_count() -> usize {
    with_table(|t| t.iter().filter(|i| i.is_live()).count())
}

pub fn new_pid() -> u32 {
    NEXT_PID.fetch_add(1, Ordering::AcqRel)
}

pub fn reset_session() {
    NEXT_PID.store(1, Ordering::Release);
    FORKS.store(0, Ordering::Release);
    CHILD_EXIT_OK.store(0, Ordering::Release);
    with_table(|t| t.clear());
}

pub fn register(i: Arc<ProcInfo>) {
    with_table(|t| t.push(i));
}

pub fn make_info(
    pid: u32,
    ppid: u32,
    pgid: u32,
    asp: AddrSpace,
    kill: Arc<crate::arch::sched::KillSwitch>,
    lp_fill: impl FnOnce(AddrSpace) -> LinuxProc,
) -> Arc<ProcInfo> {
    let pml4 = asp.pml4;
    Arc::new(ProcInfo {
        pid,
        pml4,
        ppid: AtomicU32::new(ppid),
        pgid: AtomicU32::new(pgid),
        state: AtomicI64::new(LIVE),
        exit_tick: AtomicU64::new(0),
        killsig: AtomicU32::new(0),
        reaped: AtomicBool::new(false),
        freed: AtomicBool::new(false),
        kill,
        lp: Arc::new(spin::Mutex::new(lp_fill(asp))),
    })
}

/// Sweep: a task the scheduler reaped (kill) becomes a zombie; a zombie that has settled (its task has left the CR3) has its
/// address space and descriptors released. Called from every dispatch retry and from the verb loop.
pub fn gc() {
    let now = crate::arch::ticks();
    for i in snapshot() {
        if i.is_live() && i.kill.is_reaped() {
            let sig = i.killsig.load(Ordering::Acquire);
            i.finish(if sig == 0 { 9 } else { sig as i64 });
        }
        if !i.is_live() && !i.freed.load(Ordering::Acquire) && now >= i.exit_tick.load(Ordering::Acquire) + 50 {
            if let Some(mut lp) = i.lp.try_lock() {
                lp.fds.clear(); // pipe EOF for the peers
                lp.asp.free_frames();
                super::fs_tab_clear(i.pml4);
                i.freed.store(true, Ordering::Release);
            }
        }
    }
    with_table(|t| t.retain(|i| !(i.reaped.load(Ordering::Acquire) && i.freed.load(Ordering::Acquire))));
}

/// Ring-3 fault (called from `record_ring3_kill` via `note_fault`): the current process is a SIGSEGV zombie.
pub fn mark_fault_current() {
    if let Some(i) = cur_info() {
        i.finish(11);
    }
}

pub fn exit_status_for(code: u64) -> i64 {
    ((code & 0xff) as i64) << 8
}

/// Tear the calling process down (never returns). Descriptors close here so pipe peers see EOF at once.
pub fn exit_current(info: &Arc<ProcInfo>, code: u64, group: bool) -> ! {
    {
        let mut g = super::fd::lk(&info.lp);
        g.fds.clear();
    }
    let me = info.pid;
    for c in snapshot() {
        if c.ppid.load(Ordering::Acquire) == me && c.pid != me {
            c.ppid.store(1, Ordering::Release);
        }
    }
    let is_root = info.pid == ROOT_PID.load(Ordering::Acquire);
    if !is_root && code & 0xff == 0 {
        CHILD_EXIT_OK.fetch_add(1, Ordering::AcqRel);
    }
    info.finish(exit_status_for(code));
    if is_root {
        super::root_exited(code, group);
    }
    crate::arch::sched::exit() // restores the kernel CR3 and retires the task
}

// ---------------------------------------------------------------------------------------------
// fork / clone
// ---------------------------------------------------------------------------------------------

#[inline]
fn frame_w(ktop: u64, off: u64) -> u64 {
    unsafe { *((ktop - off) as *const u64) }
}

pub fn fork(p: &mut LinuxProc, info: &Arc<ProcInfo>, ktop: u64, child_sp: u64) -> i64 {
    if live_count() >= MAX_PROCS {
        return -EAGAIN;
    }
    if p.asp.pages() > FORK_MAX_PAGES {
        return -ENOMEM;
    }
    let Some(casp) = p.asp.fork_copy() else { return -ENOMEM };
    let rip = frame_w(ktop, 32);
    let usp = if child_sp != 0 { child_sp } else { frame_w(ktop, 8) };
    let regs = [
        frame_w(ktop, 40),
        frame_w(ktop, 48),
        frame_w(ktop, 56),
        frame_w(ktop, 64),
        frame_w(ktop, 72),
        frame_w(ktop, 80),
    ];
    let pid = new_pid();
    let kill = Arc::new(crate::arch::sched::KillSwitch::new());
    let fds = p.fds.clone();
    let (brk, brk_mapped, mmap_next, fs_base, cwd, umask) = (p.brk, p.brk_mapped, p.mmap_next, p.fs_base, p.cwd.clone(), p.umask);
    let cinfo = make_info(pid, info.pid, info.pgid.load(Ordering::Acquire), casp, kill.clone(), |asp| LinuxProc {
        asp,
        brk,
        brk_mapped,
        mmap_next,
        fds,
        fs_base,
        cwd,
        umask,
        sleep_until: None,
    });
    let pml4 = cinfo.pml4;
    register(cinfo);
    super::fs_tab_set(pml4, fs_base);
    super::push_fork_regs(pml4, regs);
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    let _ = crate::arch::sched::spawn_user_preemptible(TASK_NAME, rip, usp, cpu, pml4, kill);
    FORKS.fetch_add(1, Ordering::AcqRel);
    pid as i64
}

/// `clone(flags, stack, ptid, ctid, tls)`: only the fork-shaped flag sets (and vfork) are honoured.
pub fn clone(p: &mut LinuxProc, info: &Arc<ProcInfo>, ktop: u64, a: [u64; 6]) -> i64 {
    const CLONE_VM: u64 = 0x100;
    const CLONE_THREAD: u64 = 0x10000;
    const CLONE_VFORK: u64 = 0x4000;
    let flags = a[0];
    if flags & CLONE_THREAD != 0 || (flags & CLONE_VM != 0 && flags & CLONE_VFORK == 0) {
        return -38; // real threads / shared-VM clones: not in rung 2
    }
    fork(p, info, ktop, a[1])
}

// ---------------------------------------------------------------------------------------------
// execve
// ---------------------------------------------------------------------------------------------

fn read_strv(p: &LinuxProc, mut va: u64, max_n: usize, max_bytes: usize) -> Result<Vec<String>, i64> {
    let mut v = Vec::new();
    if va == 0 {
        return Ok(v);
    }
    let mut total = 0usize;
    loop {
        let mut b = [0u8; 8];
        if !p.asp.copy_in(va, &mut b) {
            return Err(-EFAULT);
        }
        let ptr = u64::from_le_bytes(b);
        if ptr == 0 {
            return Ok(v);
        }
        let Some(s) = p.asp.read_cstr(ptr, 32768) else { return Err(-EFAULT) };
        total += s.len() + 1;
        if v.len() >= max_n || total > max_bytes {
            return Err(-E2BIG);
        }
        v.push(String::from_utf8_lossy(&s).into_owned());
        va += 8;
    }
}

pub fn execve(p: &mut LinuxProc, info: &Arc<ProcInfo>, ktop: u64, path_va: u64, argv_va: u64, envp_va: u64) -> i64 {
    let full = match sys::resolve(p, sys::AT_FDCWD, path_va) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let args = match read_strv(p, argv_va, 256, 256 * 1024) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let envs = match read_strv(p, envp_va, 256, 256 * 1024) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let (full, bytes) = match super::read_image(&full) {
        Ok(v) => v,
        Err(_) => return -ENOENT,
    };
    let plan = match elf::parse(&bytes) {
        Ok(pl) => pl,
        Err(_) => return -ENOEXEC,
    };
    // ---- point of no return: the old image is gone (same PML4, so Task.user_cr3 stays valid) ----
    p.asp.reset();
    if elf::load(&mut p.asp, &bytes, &plan).is_err() {
        serial_println!("[linuxabi] execve {}: load failed after reset — process ends", full);
        return -ENOEXEC; // returns into unmapped memory: the process dies SIGSEGV
    }
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let envp: Vec<&str> = envs.iter().map(|s| s.as_str()).collect();
    let sp = match elf::build_stack(&mut p.asp, &plan, &full, &argv, &envp) {
        Ok(s) => s,
        Err(_) => return -ENOMEM,
    };
    p.brk = BRK_BASE;
    p.brk_mapped = BRK_BASE;
    p.mmap_next = MMAP_BASE;
    p.fs_base = 0;
    super::fs_tab_clear(info.pml4);
    for s in p.fds.iter_mut() {
        if s.as_ref().is_some_and(|e| e.cloexec) {
            *s = None;
        }
    }
    unsafe {
        let w = |off: u64, v: u64| *((ktop - off) as *mut u64) = v;
        w(8, sp);
        w(16, sp);
        w(32, plan.entry);
        for off in [40, 48, 56, 64, 72, 80] {
            w(off, 0);
        }
    }
    serial_println!("[linuxabi] execve pid={} path={} entry={:#x}", info.pid, full, plan.entry);
    0
}

// ---------------------------------------------------------------------------------------------
// wait / kill / ids
// ---------------------------------------------------------------------------------------------

/// Children of `me` matching a wait4 `pid` selector: `(any_child, first_zombie)`.
fn scan_children(me: &ProcInfo, sel: i64) -> (bool, Option<Arc<ProcInfo>>) {
    let (mut any, mut z) = (false, None);
    for c in snapshot() {
        if c.ppid.load(Ordering::Acquire) != me.pid || c.pid == me.pid || c.reaped.load(Ordering::Acquire) {
            continue;
        }
        let ok = match sel {
            -1 => true,
            0 => c.pgid.load(Ordering::Acquire) == me.pgid.load(Ordering::Acquire),
            s if s > 0 => c.pid as i64 == s,
            s => c.pgid.load(Ordering::Acquire) as i64 == -s,
        };
        if !ok {
            continue;
        }
        any = true;
        if z.is_none() && !c.is_live() {
            z = Some(c);
        }
    }
    (any, z)
}

pub fn wait4(p: &mut LinuxProc, me: &Arc<ProcInfo>, pid: i64, status_va: u64, options: u64) -> i64 {
    let (any, z) = scan_children(me, pid);
    if !any {
        return -ECHILD;
    }
    if let Some(c) = z {
        let st = (c.state.load(Ordering::Acquire) as i32).to_le_bytes();
        if status_va != 0 && !p.asp.copy_out(status_va, &st, false) {
            return -EFAULT;
        }
        c.reaped.store(true, Ordering::Release);
        return c.pid as i64;
    }
    if options & WNOHANG != 0 {
        0
    } else {
        sys::RETRY
    }
}

pub fn waitid(p: &mut LinuxProc, me: &Arc<ProcInfo>, idtype: u64, id: u64, infop: u64, options: u64) -> i64 {
    let sel = match idtype {
        0 => -1,
        1 => id as i64,
        2 => -(id as i64),
        _ => return -EINVAL,
    };
    let (any, z) = scan_children(me, sel);
    if !any {
        return -ECHILD;
    }
    if let Some(c) = z {
        let st = c.state.load(Ordering::Acquire);
        let mut si = [0u8; 128];
        si[0..4].copy_from_slice(&17i32.to_le_bytes()); // SIGCHLD
        let (code, status) = if st >= 256 || st == 0 { (1i32, (st >> 8) as i32) } else { (2i32, st as i32) };
        si[8..12].copy_from_slice(&code.to_le_bytes()); // CLD_EXITED / CLD_KILLED
        si[16..20].copy_from_slice(&c.pid.to_le_bytes());
        si[24..28].copy_from_slice(&status.to_le_bytes());
        if infop != 0 && !p.asp.copy_out(infop, &si, false) {
            return -EFAULT;
        }
        if options & 0x0100_0000 == 0 {
            c.reaped.store(true, Ordering::Release); // not WNOWAIT
        }
        return 0;
    }
    if options & WNOHANG != 0 {
        if infop != 0 {
            let _ = p.asp.copy_out(infop, &[0u8; 128], false);
        }
        0
    } else {
        sys::RETRY
    }
}

fn fatal_sig(sig: u64) -> bool {
    !matches!(sig, 0 | 17 | 18 | 19 | 20 | 21 | 22 | 23 | 28)
}

pub fn kill(me: &Arc<ProcInfo>, pid: i64, sig: u64) -> i64 {
    if sig > 64 {
        return -EINVAL;
    }
    let targets: Vec<Arc<ProcInfo>> = if pid > 0 {
        match find(pid as u32) {
            Some(t) if t.is_live() => alloc::vec![t],
            _ => return -ESRCH,
        }
    } else if pid == 0 || pid < -1 {
        let g = if pid == 0 { me.pgid.load(Ordering::Acquire) as i64 } else { -pid };
        snapshot().into_iter().filter(|i| i.is_live() && i.pgid.load(Ordering::Acquire) as i64 == g).collect()
    } else {
        snapshot().into_iter().filter(|i| i.is_live() && i.pid != me.pid && i.pid != 1).collect()
    };
    if targets.is_empty() {
        return -ESRCH;
    }
    if sig != 0 && fatal_sig(sig) {
        for t in targets {
            t.killsig.store(sig as u32, Ordering::Release);
            t.kill.request(); // ends the task at its next kill boundary; gc turns it into a signalled zombie
        }
    }
    0
}

pub fn setpgid(me: &Arc<ProcInfo>, pid: u64, pgid: u64) -> i64 {
    let t = if pid == 0 || pid as u32 == me.pid {
        me.clone()
    } else {
        match find(pid as u32) {
            Some(t) => t,
            None => return -ESRCH,
        }
    };
    let g = if pgid == 0 { t.pid } else { pgid as u32 };
    t.pgid.store(g, Ordering::Release);
    0
}

pub fn getpgid(me: &Arc<ProcInfo>, pid: u64) -> i64 {
    if pid == 0 || pid as u32 == me.pid {
        return me.pgid.load(Ordering::Acquire) as i64;
    }
    match find(pid as u32) {
        Some(t) => t.pgid.load(Ordering::Acquire) as i64,
        None => -ESRCH,
    }
}
