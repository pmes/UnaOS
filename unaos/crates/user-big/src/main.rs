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
extern crate alloc;

use una_abi::{ENOMEM, SYS_EXIT, SYS_SBRK, SYS_WRITE};

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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(0xF0)
}

/// EXECNAME (B322, R82): this program's launch declaration — a console program (no SYS_WIN_CREATE): a bare `big` runs in the foreground like `run`.
/// Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(0);
