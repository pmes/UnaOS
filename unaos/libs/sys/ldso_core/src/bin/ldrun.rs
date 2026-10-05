// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — shared-core
//!
//! SELFBUILD6 (B360): `ldrun` — the HOST fulfiller of `ldso_core::Space`, over `mmap` on Linux x86-64. It is the arc's
//! only EXECUTION proof (R78: no QEMU): the same mapping, symbol lookup, relocation and TLS code the kernel links runs
//! the real payload (a musl PIE + `.so`, musl `rust-lld`, the musl-host `rustc`) under the host kernel.
//!
//! `ldrun [-L dir]... [--stats] <program> [args...]` maps the program and its `DT_NEEDED` objects, builds the System V
//! initial stack (with the loader's SYNTHETIC `AT_PHDR`), and jumps to the program's entry on this thread; it never
//! returns (musl's `exit` ends the process).
//!
//! Two host-only cares: the runner's own glibc must not share the program break with musl's malloc, so the runner's
//! allocator is `mmap`-backed; and a `dlopen` / `dlsym` / `dlclose` / `dladdr` gate runs Rust code on the program's
//! thread, so it swaps `FS` back to the runner's thread pointer for the call and restores the program's on return
//! (the program's `FS` is also the `tp` a late object's TLS image is written under).

use ldso_core::{Loader, Space, Start, PROT_R, PROT_W, PROT_X};
use std::alloc::{GlobalAlloc, Layout};
use std::ffi::CStr;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn open(path: *const u8, flags: i32, ...) -> i32;
    fn pread(fd: i32, buf: *mut u8, n: usize, off: i64) -> isize;
    fn fstat(fd: i32, st: *mut u8) -> i32;
    fn getauxval(t: u64) -> u64;
    fn getuid() -> u32;
    fn geteuid() -> u32;
    fn getgid() -> u32;
    fn getegid() -> u32;
}

const MAP_PRIVATE: i32 = 0x02;
const MAP_FIXED: i32 = 0x10;
const MAP_ANON: i32 = 0x20;
const MAP_NORESERVE: i32 = 0x4000;
const MAP_FIXED_NOREPLACE: i32 = 0x10_0000;
const MAP_FAILED: *mut u8 = !0usize as *mut u8;

// ---- an mmap-backed allocator: the runner never moves the program break (musl's mallocng owns it) ----
struct MmapAlloc;
static BUMP: AtomicU64 = AtomicU64::new(0);
static BUMP_END: AtomicU64 = AtomicU64::new(0);
static ALOCK: AtomicBool = AtomicBool::new(false);
const BIG: usize = 256 * 1024;

unsafe impl GlobalAlloc for MmapAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.size() >= BIG || l.align() > 4096 {
            let n = (l.size() + 4095) & !4095;
            let p = unsafe { mmap(core::ptr::null_mut(), n, 3, MAP_PRIVATE | MAP_ANON, -1, 0) };
            return if p == MAP_FAILED { core::ptr::null_mut() } else { p };
        }
        while ALOCK.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        let mut cur = BUMP.load(Ordering::Relaxed);
        let a = l.align() as u64;
        let mut at = (cur + a - 1) & !(a - 1);
        if cur == 0 || at + l.size() as u64 > BUMP_END.load(Ordering::Relaxed) {
            let chunk = 8 << 20;
            let p = unsafe { mmap(core::ptr::null_mut(), chunk, 3, MAP_PRIVATE | MAP_ANON, -1, 0) };
            if p == MAP_FAILED {
                ALOCK.store(false, Ordering::Release);
                return core::ptr::null_mut();
            }
            cur = p as u64;
            BUMP_END.store(cur + chunk as u64, Ordering::Relaxed);
            at = (cur + a - 1) & !(a - 1);
        }
        BUMP.store(at + l.size() as u64, Ordering::Relaxed);
        ALOCK.store(false, Ordering::Release);
        at as *mut u8
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if l.size() >= BIG || l.align() > 4096 {
            unsafe { munmap(p, (l.size() + 4095) & !4095) };
        }
    }
}

#[global_allocator]
static GLOBAL: MmapAlloc = MmapAlloc;

// ---- the Space over this process ----
struct Host;

fn prot_of(p: u32) -> i32 {
    (if p & PROT_R != 0 { 1 } else { 0 }) | (if p & PROT_W != 0 { 2 } else { 0 }) | (if p & PROT_X != 0 { 4 } else { 0 })
}

impl Space for Host {
    type File = i32;
    fn open(&mut self, path: &[u8]) -> Option<(i32, u64)> {
        let mut c = path.to_vec();
        c.push(0);
        let fd = unsafe { open(c.as_ptr(), 0o2000000) }; // O_RDONLY | O_CLOEXEC
        if fd < 0 {
            return None;
        }
        let mut st = [0u8; 144];
        if unsafe { fstat(fd, st.as_mut_ptr()) } != 0 {
            return None;
        }
        let mode = u32::from_le_bytes(st[24..28].try_into().unwrap());
        if mode & 0o170000 != 0o100000 {
            return None;
        }
        Some((fd, u64::from_le_bytes(st[48..56].try_into().unwrap())))
    }
    fn read_file(&mut self, f: &i32, off: u64, buf: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            let n = unsafe { pread(*f, buf[done..].as_mut_ptr(), buf.len() - done, (off + done as u64) as i64) };
            if n <= 0 {
                return false;
            }
            done += n as usize;
        }
        true
    }
    fn reserve(&mut self, len: u64, fixed: Option<u64>) -> Result<u64, String> {
        let (addr, fl) = match fixed {
            Some(a) => (a as *mut u8, MAP_FIXED_NOREPLACE),
            None => (core::ptr::null_mut(), 0),
        };
        let p = unsafe { mmap(addr, len as usize, 0, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE | fl, -1, 0) };
        if p == MAP_FAILED || (fixed.is_some() && p != addr) {
            return Err(format!("cannot reserve {} bytes{}", len, fixed.map_or(String::new(), |a| format!(" at {:#x}", a))));
        }
        Ok(p as u64)
    }
    fn map_file(&mut self, va: u64, len: u64, f: &i32, off: u64, prot: u32) -> bool {
        let p = unsafe { mmap(va as *mut u8, len as usize, prot_of(prot), MAP_PRIVATE | MAP_FIXED, *f, off as i64) };
        p as u64 == va
    }
    fn map_anon(&mut self, va: u64, len: u64, prot: u32) -> bool {
        let p = unsafe { mmap(va as *mut u8, len as usize, prot_of(prot), MAP_PRIVATE | MAP_FIXED | MAP_ANON, -1, 0) };
        p as u64 == va
    }
    fn map_code(&mut self, va: u64, bytes: &[u8]) -> bool {
        if !self.map_anon(va, 4096, PROT_R | PROT_W) {
            return false;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), va as *mut u8, bytes.len().min(4096));
            mprotect(va as *mut u8, 4096, 5) == 0
        }
    }
    fn write(&mut self, va: u64, data: &[u8]) -> bool {
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), va as *mut u8, data.len()) };
        true
    }
    fn read(&mut self, va: u64, out: &mut [u8]) -> bool {
        unsafe { core::ptr::copy_nonoverlapping(va as *const u8, out.as_mut_ptr(), out.len()) };
        true
    }
}

// ---- the gates: the program calls these through the trampoline's G_* slots ----
static mut LOADER: Option<Loader<i32>> = None;
static LLOCK: AtomicBool = AtomicBool::new(false);
static RUNNER_FS: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
unsafe fn arch_prctl(code: u64, arg: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") 158u64 => r, in("rdi") code, in("rsi") arg,
            lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    r
}

#[inline(always)]
unsafe fn get_fs() -> u64 {
    let mut v: u64 = 0;
    unsafe { arch_prctl(0x1003, &mut v as *mut u64 as u64) };
    v
}

#[inline(always)]
unsafe fn set_fs(v: u64) {
    unsafe { arch_prctl(0x1002, v) };
}

/// Run `f` on the loader with the runner's thread pointer installed; `tp` is the program's.
#[inline(always)]
unsafe fn gate(f: impl FnOnce(&mut Loader<i32>, &mut Host, u64) -> u64) -> u64 {
    unsafe {
        let tp = get_fs();
        set_fs(RUNNER_FS.load(Ordering::Relaxed));
        while LLOCK.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        #[allow(static_mut_refs)]
        let r = match LOADER.as_mut() {
            Some(l) => f(l, &mut Host, tp),
            None => 0,
        };
        LLOCK.store(false, Ordering::Release);
        set_fs(tp);
        r
    }
}

extern "C" fn gate_open(path: *const u8, flags: i32) -> u64 {
    unsafe {
        gate(|l, h, tp| {
            let p = if path.is_null() { None } else { Some(CStr::from_ptr(path as *const core::ffi::c_char).to_bytes()) };
            if std::env::var_os("LDRUN_TRACE").is_some() {
                eprintln!("[ldrun] dlopen {:?} flags={:#x}", p.map(String::from_utf8_lossy), flags);
            }
            l.dlopen(h, p, flags as u64, tp)
        })
    }
}

extern "C" fn gate_sym(handle: u64, name: *const u8) -> u64 {
    unsafe {
        gate(|l, h, _| {
            let n = CStr::from_ptr(name as *const core::ffi::c_char).to_bytes();
            let r = l.dlsym(h, handle, n);
            if std::env::var_os("LDRUN_TRACE").is_some() {
                eprintln!("[ldrun] dlsym {:#x} {} -> {:#x}", handle, String::from_utf8_lossy(n), r);
            }
            r
        })
    }
}

extern "C" fn gate_close(handle: u64) -> u64 {
    unsafe { gate(|l, h, _| l.dlclose(h, handle)) }
}

extern "C" fn gate_addr(addr: u64, info: u64) -> u64 {
    unsafe { gate(|l, h, _| l.dladdr(h, addr, info)) }
}

// ---- the initial stack ----
fn build_stack(st: &Start, argv: &[Vec<u8>], envp: &[Vec<u8>]) -> u64 {
    let size = 8usize << 20;
    let base = unsafe { mmap(core::ptr::null_mut(), size, 3, MAP_PRIVATE | MAP_ANON | 0x20000 /* MAP_STACK */, -1, 0) };
    assert!(base != MAP_FAILED, "stack mmap");
    let top = base as u64 + size as u64;
    let mut cur = top - 16;
    let mut put = |b: &[u8]| -> u64 {
        cur -= b.len() as u64;
        unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), cur as *mut u8, b.len()) };
        cur
    };
    let mut rnd = [0u8; 16];
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64);
    for (i, b) in rnd.iter_mut().enumerate() {
        *b = (t.rotate_left(i as u32 * 7).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8;
    }
    let random_p = put(&rnd);
    let platform_p = put(b"x86_64\0");
    let mut z = |v: &Vec<u8>| {
        let mut c = v.clone();
        c.push(0);
        put(&c)
    };
    let execfn_p = z(&argv[0]);
    let env_p: Vec<u64> = envp.iter().map(&mut z).collect();
    let arg_p: Vec<u64> = argv.iter().map(&mut z).collect();
    let aux: [(u64, u64); 18] = unsafe {
        [
            (3, st.phdr),
            (4, st.phent),
            (5, st.phnum),
            (6, 4096),
            (7, st.tramp),
            (8, 0),
            (9, st.entry),
            (11, getuid() as u64),
            (12, geteuid() as u64),
            (13, getgid() as u64),
            (14, getegid() as u64),
            (16, getauxval(16)),
            (17, 100),
            (23, 0),
            (25, random_p),
            (31, execfn_p),
            (15, platform_p),
            (0, 0),
        ]
    };
    let mut words: Vec<u64> = vec![arg_p.len() as u64];
    words.extend_from_slice(&arg_p);
    words.push(0);
    words.extend_from_slice(&env_p);
    words.push(0);
    for (k, v) in aux {
        words.push(k);
        words.push(v);
    }
    let sp = (cur - words.len() as u64 * 8) & !15;
    for (i, w) in words.iter().enumerate() {
        unsafe { *((sp + i as u64 * 8) as *mut u64) = *w };
    }
    sp
}

fn main() {
    let mut args: Vec<Vec<u8>> = std::env::args_os().skip(1).map(|a| a.as_bytes().to_vec()).collect();
    let mut search: Vec<Vec<u8>> = Vec::new();
    let mut stats = false;
    while let Some(a) = args.first().cloned() {
        if a == b"-L" && args.len() > 1 {
            search.push(args[1].clone());
            args.drain(..2);
        } else if a == b"--stats" {
            stats = true;
            args.remove(0);
        } else {
            break;
        }
    }
    if let Some(p) = std::env::var_os("LDRUN_PATH") {
        search.extend(p.as_bytes().split(|&c| c == b':').map(|x| x.to_vec()));
    }
    if args.is_empty() {
        eprintln!("usage: ldrun [-L dir]... [--stats] <program> [args...]");
        std::process::exit(2);
    }
    let envp: Vec<Vec<u8>> = std::env::vars_os()
        .filter(|(k, _)| k != "LDRUN_PATH")
        .map(|(k, v)| {
            let mut e = k.as_bytes().to_vec();
            e.push(b'=');
            e.extend_from_slice(v.as_bytes());
            e
        })
        .collect();
    let t0 = std::time::Instant::now();
    let mut l: Loader<i32> = Loader::new(search);
    let gates = [
        gate_open as extern "C" fn(*const u8, i32) -> u64 as usize as u64,
        gate_sym as extern "C" fn(u64, *const u8) -> u64 as usize as u64,
        gate_close as extern "C" fn(u64) -> u64 as usize as u64,
        gate_addr as extern "C" fn(u64, u64) -> u64 as usize as u64,
    ];
    let st = match l.load_program(&mut Host, &args[0], Some(gates)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ldrun: {}", e);
            std::process::exit(127);
        }
    };
    let ms = t0.elapsed().as_millis();
    if stats {
        eprintln!(
            "[ldrun] loaded objects={} pages_mapped={} relocs={} lookups={} file_bytes_read={} load_ms={}",
            l.stats.objects, l.stats.pages_mapped, l.stats.relocs, l.stats.lookups, l.stats.file_bytes_read, ms
        );
        for o in &l.objs {
            eprintln!(
                "[ldrun]   {} base={:#x} span={} relocs={} tls_dist={}",
                String::from_utf8_lossy(&o.path),
                o.base,
                o.hi - o.lo,
                o.relocs,
                if o.tls.is_some() { o.tls_dist as i64 } else { -1 }
            );
        }
    }
    let sp = build_stack(&st, &args, &envp);
    unsafe {
        RUNNER_FS.store(get_fs(), Ordering::Relaxed);
        LOADER = Some(l);
        core::arch::asm!(
            "mov rsp, {sp}",
            "xor ebp, ebp",
            "xor edx, edx",
            "jmp {entry}",
            sp = in(reg) sp,
            entry = in(reg) st.entry,
            options(noreturn)
        );
    }
}
