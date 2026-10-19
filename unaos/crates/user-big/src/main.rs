#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// RING3WIN M4 (rmbp-ledger B316): BIG.ELF — the program the 16 KiB window could not hold.
//
//   1. static — a 64 KiB BSS array (zero on entry: the loader's memsz > filesz path), filled with
//      a[i] = i*7 + (i>>8) and FNV-1a-32 checksummed. The kernel recomputes the same value itself.
//   2. stack  — four nested frames of 32 KiB each (128 KiB live) on the 256 KiB stack this image
//      declares in PT_GNU_STACK; the default 64 KiB would fault, which is the point of the note.
//   3. heap   — a global allocator over SYS_SBRK (58), and a 100 KiB `Vec<u8>` filled and re-read.
//   4. cap    — SYS_SBRK of the whole window (USER_WINDOW_BYTES, 64 MiB since WINDOW2) must be refused
//      -ENOMEM: the image and stack already sit in it. (RING3WIN asked 8 MiB of a 4 MiB window.)
//   5. alloc48m (WINDOW2, B361) — SYS_SBRK of 48 MiB (una_abi::WINDOW2_ALLOC_BYTES), one word written and read
//      back on EVERY page, then handed back (the break returns to where it was).
//
// Exit status: low byte = the pass bits (0x1F = all five; `tests ring3win` reads 0x0F, `tests window` 0x10),
// bits 8..30 = the checksum's bits 8..30.
// One line to the console says the same in words.
//
// FPU (rmbp-ledger B666): `big fpu` runs ONLY the FPU probe instead (`fpu_verb`, below; `tests fpu` reads it).
extern crate alloc;

use core::sync::atomic::{AtomicU32, Ordering};
use una_abi::{ENOMEM, SYS_EXIT, SYS_SBRK, SYS_THREAD_EXIT, SYS_THREAD_JOIN, SYS_THREAD_SPAWN, SYS_WRITE, SYS_YIELD};

#[inline(always)]
unsafe fn sys1(n: u64, a0: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") n => r, inlateout("rdi") a0 => _, lateout("rsi") _,
            lateout("rdx") _, lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _,
            lateout("r10") _, options(nostack))
    };
    r
}
#[inline(always)]
unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") n => r, inlateout("rdi") a0 => _, inlateout("rsi") a1 => _,
            inlateout("rdx") a2 => _, lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _,
            lateout("r10") _, options(nostack))
    };
    r
}

fn sbrk(delta: i64) -> i64 {
    unsafe { sys1(SYS_SBRK, delta as u64) as i64 }
}
fn write(b: &[u8]) {
    unsafe { sys3(SYS_WRITE, 1, b.as_ptr() as u64, b.len() as u64) };
}
fn exit(code: u64) -> ! {
    unsafe { sys1(SYS_EXIT, code) };
    loop {}
}

// --- the allocator: a bump over SYS_SBRK (free is a no-op; this proves `alloc` links and runs) ---
struct Sbrk;
static mut BRK_CUR: u64 = 0;
static mut BRK_END: u64 = 0;
unsafe impl core::alloc::GlobalAlloc for Sbrk {
    unsafe fn alloc(&self, l: core::alloc::Layout) -> *mut u8 {
        unsafe {
            if BRK_END == 0 {
                let b = sbrk(0);
                if b < 0 {
                    return core::ptr::null_mut();
                }
                BRK_CUR = b as u64;
                BRK_END = b as u64;
            }
            let p = (BRK_CUR + l.align() as u64 - 1) & !(l.align() as u64 - 1);
            let end = p + l.size() as u64;
            if end > BRK_END {
                if sbrk((end - BRK_END) as i64) < 0 {
                    return core::ptr::null_mut();
                }
                BRK_END = end;
            }
            BRK_CUR = end;
            p as *mut u8
        }
    }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static A: Sbrk = Sbrk;

static mut ARR: [u8; 65536] = [0; 65536];

fn fnv(b: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for &x in b {
        h ^= x as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// 32 KiB of frame per level; returns a value that depends on every byte so nothing is elided.
#[inline(never)]
fn deep(n: u32) -> u32 {
    let mut f = [0u8; 32 * 1024];
    let f = core::hint::black_box(&mut f);
    for (i, b) in f.iter_mut().enumerate() {
        *b = (i as u32 ^ n) as u8;
    }
    let mut s = 0u32;
    for &b in f.iter() {
        s = s.wrapping_add(b as u32);
    }
    if n == 0 { s } else { s.wrapping_add(deep(n - 1)) }
}

struct Line {
    b: [u8; 200],
    n: usize,
}
impl Line {
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn dec(&mut self, v: u64) {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..]);
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    if una_abi::args().and_then(|a| a.get(1)) == Some(b"fpu".as_slice()) {
        fpu_verb(); // FPU (B666): never returns
    }
    let mut bits = 0u64;
    // 1. static: the BSS must arrive zeroed, then fill + checksum.
    let arr = unsafe { &mut *core::ptr::addr_of_mut!(ARR) };
    let zero = arr.iter().all(|&b| b == 0);
    for (i, b) in arr.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(7).wrapping_add(i as u32 >> 8) as u8;
    }
    let ck = fnv(core::hint::black_box(&arr[..]));
    if zero {
        bits |= 1;
    }
    // 2. stack: 4 levels x 32 KiB. Expected: sum over n=0..3 of sum_i ((i ^ n) as u8) = 4 * 128 * (255*256/2).
    let st = deep(core::hint::black_box(3));
    if st == 4 * 128 * (255 * 256 / 2) {
        bits |= 2;
    }
    // 3. heap: a 100 KiB Vec through the global allocator over SYS_SBRK.
    let b0 = sbrk(0);
    let mut v: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(100 * 1024);
    for i in 0..100 * 1024u32 {
        v.push((i % 251) as u8);
    }
    let heap_ok = v.len() == 100 * 1024 && v.iter().enumerate().all(|(i, &b)| b == (i as u32 % 251) as u8);
    let grown = sbrk(0) - b0;
    if heap_ok && b0 > 0 && grown >= 100 * 1024 {
        bits |= 4;
    }
    // 5. WINDOW2: 48 MiB of heap, every page touched and read back, then given back.
    let want = una_abi::WINDOW2_ALLOC_BYTES as i64;
    let before = sbrk(0);
    let a = sbrk(want);
    let mut touched = 0u64;
    if a > 0 {
        let base = ((a as u64) + 4095) & !4095;
        let pages = (want as u64 - 4096) / 4096;
        for i in 0..pages {
            unsafe { core::ptr::write_volatile((base + i * 4096) as *mut u64, i ^ 0x5749_4E44_4F57_3200) };
        }
        for i in 0..pages {
            if unsafe { core::ptr::read_volatile((base + i * 4096) as *const u64) } == i ^ 0x5749_4E44_4F57_3200 {
                touched += 1;
            }
        }
        let back = sbrk(-want);
        if touched == pages && back == a + want && sbrk(0) == before {
            bits |= 0x10;
        }
    }
    // 4. the cap: the whole window again is refused (the image and stack already live in it).
    let big = sbrk(una_abi::USER_WINDOW_BYTES as i64);
    if big == ENOMEM {
        bits |= 8;
    }
    let mut l = Line { b: [0; 200], n: 0 };
    l.put(b"BIG: static=");
    l.put(if bits & 1 != 0 { b"ok".as_slice() } else { b"bad".as_slice() });
    l.put(b" fnv=");
    l.dec(ck as u64);
    l.put(b" stack=");
    l.put(if bits & 2 != 0 { b"ok".as_slice() } else { b"bad".as_slice() });
    l.put(b" sbrk=");
    l.dec(grown.max(0) as u64);
    l.put(b" cap=");
    l.put(if bits & 8 != 0 { b"refused".as_slice() } else { b"bad".as_slice() });
    l.put(b" alloc48m=");
    l.put(if bits & 0x10 != 0 { b"ok".as_slice() } else { b"bad".as_slice() });
    l.put(b" pages=");
    l.dec(touched);
    l.put(b"\n");
    write(&l.b[..l.n]);
    exit(((ck as u64) & 0x7FFF_FF00) | bits)
}

// --- FPU (rmbp-ledger B666): `big fpu` — the x87/SSE context survives a reschedule, per task ---
//
// Two ring-3 tasks of this process — the program (task 0) and one SYS_THREAD_SPAWN worker on its core (task 1) — each run
// FPU_ROUNDS rounds of: write a round-unique pattern, load it into xmm0..xmm15 and the task's OWN MXCSR (task 0 round-to-
// nearest, task 1 round-toward-zero), SYS_YIELD, store the sixteen registers and MXCSR back, compare. The kernel saves the
// image at every switch-back and SCRUBS the registers to the initial image, so a missing restore reads back zeros, and a
// restore of the wrong task reads the other pattern. A round in which the other task's counter moved across the yield is an
// observed interleave. Then task 0 keeps an f64 accumulator in registers across yields and checks it bit-exactly against
// the same arithmetic with no yields.
//
// Exit status: low byte = 0x80 (the verb ran — its SSE instructions did not #UD) | 0x01 task-0 xmm | 0x02 task-1 xmm |
// 0x04 both MXCSRs | 0x08 float math | 0x10 worker joined; bits 8..23 = observed interleaves.
const FPU_ROUNDS: u32 = 64;
const MXCSR_NEAREST: u32 = 0x1F80;
const MXCSR_TOWARD_ZERO: u32 = 0x7F80;

/// One task's buffers: the pattern loaded, the registers stored back, MXCSR in and out (offsets the asm names).
#[repr(C, align(16))]
struct FpuCtx {
    pat: [u8; 256],
    out: [u8; 256],
    mx_in: u32,
    mx_out: u32,
    _pad: [u32; 2],
}
static mut FPU_CTX: [FpuCtx; 2] = [const { FpuCtx { pat: [0; 256], out: [0; 256], mx_in: 0, mx_out: 0, _pad: [0; 2] } }; 2];
static FPU_TICK: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
static FPU_WORKER: AtomicU32 = AtomicU32::new(0);

#[repr(C, align(16))]
struct WorkerStack([u8; 16 * 1024]);
static mut FPU_STACK: WorkerStack = WorkerStack([0; 16 * 1024]);

#[inline(always)]
unsafe fn sys4(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") n => r, inlateout("rdi") a0 => _, inlateout("rsi") a1 => _,
            inlateout("rdx") a2 => _, inlateout("r10") a3 => _, lateout("rcx") _, lateout("r11") _, lateout("r8") _,
            lateout("r9") _, options(nostack))
    };
    r
}

/// SYS_YIELD with NO vector clobbers: whatever the compiler keeps in xmm across it must survive the kernel.
#[inline(always)]
fn yield_keep() {
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") SYS_YIELD => _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _,
            lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _, lateout("r10") _, options(nostack))
    };
}

/// Task `t`'s rounds: `(xmm ok, mxcsr ok, interleaves)`.
fn fpu_rounds(t: usize) -> (bool, bool, u32) {
    let ctx = unsafe { core::ptr::addr_of_mut!(FPU_CTX[t]) };
    let mx = if t == 0 { MXCSR_NEAREST } else { MXCSR_TOWARD_ZERO };
    let (mut xmm_ok, mut mx_ok, mut seen) = (true, true, 0u32);
    for r in 0..FPU_ROUNDS {
        unsafe {
            for (k, b) in (*ctx).pat.iter_mut().enumerate() {
                *b = (k as u8).wrapping_mul(13) ^ (0x5A * t as u8 + 1) ^ (r as u8).wrapping_mul(29);
            }
            (*ctx).out = [0; 256];
            (*ctx).mx_in = mx;
            (*ctx).mx_out = 0;
        }
        let other = FPU_TICK[1 - t].load(Ordering::Acquire);
        unsafe {
            core::arch::asm!(
                "ldmxcsr [{c} + 512]",
                "movdqa xmm0, [{c}]", "movdqa xmm1, [{c} + 16]", "movdqa xmm2, [{c} + 32]", "movdqa xmm3, [{c} + 48]",
                "movdqa xmm4, [{c} + 64]", "movdqa xmm5, [{c} + 80]", "movdqa xmm6, [{c} + 96]", "movdqa xmm7, [{c} + 112]",
                "movdqa xmm8, [{c} + 128]", "movdqa xmm9, [{c} + 144]", "movdqa xmm10, [{c} + 160]", "movdqa xmm11, [{c} + 176]",
                "movdqa xmm12, [{c} + 192]", "movdqa xmm13, [{c} + 208]", "movdqa xmm14, [{c} + 224]", "movdqa xmm15, [{c} + 240]",
                "mov eax, {y}",
                "syscall",
                "movdqa [{c} + 256], xmm0", "movdqa [{c} + 272], xmm1", "movdqa [{c} + 288], xmm2", "movdqa [{c} + 304], xmm3",
                "movdqa [{c} + 320], xmm4", "movdqa [{c} + 336], xmm5", "movdqa [{c} + 352], xmm6", "movdqa [{c} + 368], xmm7",
                "movdqa [{c} + 384], xmm8", "movdqa [{c} + 400], xmm9", "movdqa [{c} + 416], xmm10", "movdqa [{c} + 432], xmm11",
                "movdqa [{c} + 448], xmm12", "movdqa [{c} + 464], xmm13", "movdqa [{c} + 480], xmm14", "movdqa [{c} + 496], xmm15",
                "stmxcsr [{c} + 516]",
                c = in(reg) ctx,
                y = const SYS_YIELD,
                // `out`, not `lateout`: the base register must not be one the syscall (or the kernel's return) writes.
                out("rax") _, out("rcx") _, out("r11") _, out("rdi") _, out("rsi") _, out("rdx") _,
                out("r8") _, out("r9") _, out("r10") _,
                out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _, out("xmm4") _, out("xmm5") _, out("xmm6") _,
                out("xmm7") _, out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _, out("xmm12") _, out("xmm13") _,
                out("xmm14") _, out("xmm15") _,
                options(nostack),
            );
        }
        FPU_TICK[t].fetch_add(1, Ordering::Release);
        if FPU_TICK[1 - t].load(Ordering::Acquire) != other {
            seen += 1;
        }
        unsafe {
            if (*ctx).out != (*ctx).pat {
                xmm_ok = false;
            }
            // MXCSR may gain sticky exception flags (bits 0..5) from nothing here; compare control bits only.
            if (*ctx).mx_out & !0x3F != mx & !0x3F {
                mx_ok = false;
            }
        }
    }
    unsafe { core::arch::asm!("ldmxcsr [{}]", in(reg) &MXCSR_NEAREST, options(nostack, readonly)) };
    (xmm_ok, mx_ok, seen)
}

/// The worker (task 1): its rounds, then its verdict in FPU_WORKER (bit 31 = done), then SYS_THREAD_EXIT.
extern "C" fn fpu_worker(_arg: u64) -> ! {
    let (x, m, seen) = fpu_rounds(1);
    FPU_WORKER.store(1 << 31 | (x as u32) << 30 | (m as u32) << 29 | seen.min(0xFFFF), Ordering::Release);
    unsafe { sys1(SYS_THREAD_EXIT, 0) };
    loop {}
}

/// f64 arithmetic with `yield_keep` between steps (the accumulator may live in xmm across each).
#[inline(never)]
fn fpu_math(yields: bool) -> u64 {
    let mut acc = core::hint::black_box(1.0f64);
    let mut s = core::hint::black_box(0.0f32);
    for i in 0..32u32 {
        acc = acc * 1.000_123_4 + (i as f64) / 7.0;
        s = s * 0.5 + (i as f32) * 0.25;
        if yields {
            yield_keep();
        }
    }
    acc.to_bits() ^ (s.to_bits() as u64) << 1
}

fn fpu_verb() -> ! {
    let top = unsafe { core::ptr::addr_of_mut!(FPU_STACK) } as u64 + 16 * 1024;
    let h = unsafe { sys4(SYS_THREAD_SPAWN, fpu_worker as *const () as u64, top, 1, 0) } as i64;
    let (x0, m0, seen0) = fpu_rounds(0);
    let mut bits = 0x80u64;
    if x0 {
        bits |= 1;
    }
    if h >= 0 && unsafe { sys1(SYS_THREAD_JOIN, h as u64) } as i64 == 0 {
        bits |= 0x10;
    }
    let w = FPU_WORKER.load(Ordering::Acquire);
    let done = w >> 31 != 0;
    if done && w & 1 << 30 != 0 {
        bits |= 2;
    }
    if m0 && done && w & 1 << 29 != 0 {
        bits |= 4;
    }
    let math = fpu_math(true) == fpu_math(false);
    if math {
        bits |= 8;
    }
    let seen = seen0 + if done { w & 0xFFFF } else { 0 };
    let mut l = Line { b: [0; 200], n: 0 };
    l.put(b"FPU: tasks=");
    l.dec(1 + (bits & 0x10 != 0) as u64);
    l.put(b" rounds=");
    l.dec(FPU_ROUNDS as u64);
    l.put(b" interleaves=");
    l.dec(seen as u64);
    l.put(b" xmm=");
    l.put(if bits & 3 == 3 { b"ok".as_slice() } else { b"bad".as_slice() });
    l.put(b" mxcsr=");
    l.put(if bits & 4 != 0 { b"ok".as_slice() } else { b"bad".as_slice() });
    l.put(b" math=");
    l.put(if math { b"ok".as_slice() } else { b"bad".as_slice() });
    l.put(b"\n");
    write(&l.b[..l.n]);
    exit((seen.min(0xFFFF) as u64) << 8 | bits)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(0xF0)
}

/// EXECNAME (B322, R82): this program's launch declaration — a console program (no SYS_WIN_CREATE): a bare `big` runs in the foreground like `run`.
/// Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(0);
