#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BGRUN-2: `stat.elf` — the PERSISTENCE app. A static ELF64 (aarch64) EL0 program that creates one
// compositor window, repaints a visibly changing readout, and RUNS UNTIL IT IS KILLED. It has no exit
// condition of its own: no frame cap, no timeout, no input-driven quit. `kill <pid>` is the whole remedy,
// which is exactly the BGRUN-1 contract ("nothing bounds a bg app; `kill` is the whole remedy").
//
// WHY IT EXISTS. BGRUN-1 landed `bg` / `jobs` / `kill`, and the headless BGRUN-ST witness proved the
// spawn->exit->reap and kill-mid-run legs on silicon. What it could NOT make operator-verifiable was the
// WC-TAB window ring: the only windowed EL0 app was UVUG, and a BACKGROUNDED uvug is UNFOCUSED, so no HID
// event ever reaches it, so it never leaves its deterministic auto path — 300 frames and gone. Two windows
// flashed past before a hand could reach TAB. This program is the missing fixture: `bg /fat/STAT.ELF`
// twice puts two windows on the panel that STAY there, and TAB walks between them for as long as the
// operator likes.
//
// WHAT IT DRAWS. A 128x128 ARGB8888 window (`SYS_WIN_CREATE`, one 64 KiB window slot = `FB_WIN_MAX_W/H`,
// the same geometry UVUG uses) carrying three things, all chosen so a photograph of the panel answers a
// question:
//   1. Its OWN PID in large digits (from `SYS_GETINFO`). Two instances of the same image are otherwise
//      pixel-identical; the pid is what lets the operator say WHICH window TAB just focused, and it is the
//      same number `jobs` prints and `kill` takes.
//   2. Its frame counter in small digits — monotonic proof the program is still looping, not a frozen
//      last-frame surface left behind by a wedged task.
//   3. A sweep block tracking the frame counter — motion visible from across the room, so "is it alive?"
//      needs no reading.
//
// WHAT IT DOES NOT DO, deliberately:
//   * No exit path. Not on unfocus, not on a frame cap, not on ESC. The unfocus batch-exit is the precise
//     defect this app exists to not have, and an ESC quit would let a stray keypress close the window an
//     operator is mid-TAB-walk through.
//   * No input polling. It never reads `SYS_INPUT_POLL`, so it needs no focus and behaves identically
//     focused or unfocused — the property the TAB test depends on. Its per-process input ring simply fills
//     and drops (bounded, harmless); the compositor's TAB binding lives in the kernel, not here.
//   * No threads. One task, one window, no futex barrier — nothing UVUG already proves is re-proved here.
//
// PACING. One `SYS_SLEEP_MS(50)` per frame (~20 fps). On metal that is a real timed sleep, so the app costs
// almost nothing while it sits on the panel. Under QEMU raspi4b the Group-1 timer is not live and
// `sys_sleep_ms` degrades to a cooperative yield, so the loop runs hot — which is fine and bounded: the only
// thing that runs this program headless is the BGRUN-ST persistence leg, which kills it after a couple of
// seconds.
//
// SERIAL. Two one-shot lines and then silence forever — a program with no exit must never be a serial
// firehose: `:: STAT: start pid=<n> win=<id> ::` at startup and `:: STAT: alive pid=<n> frames=<n> ::`
// once, at `ALIVE_MARK`. Nothing is printed per frame.
//
// EL0 owns only the OFF-SCREEN surface bytes — never the scan-out, never a physical address, never a kernel
// mapping (`SYS_WIN_PRESENT` is the only surface->screen path and it runs in the kernel). Window CHROME is
// drawn by the kernel from its own copy of the title, so this program cannot forge a frame. Page-permission
// laws (per-page perms, WXN) are untouched.

// ---------------------------------------------------------------------------------------------
// Syscall ABI — the ARCH SPLIT (WINX-4).
//
// The NUMBERS are shared across arches by law: a syscall number names the same verb everywhere, so the
// six constants below are declared once and are correct on both. Only the instruction and the register
// mapping differ, which is exactly what the two `sysabi` modules encapsulate. Everything below this
// block — the drawing, the formatting, the whole program — is arch-neutral Rust that never mentions a
// register.
//
//   aarch64: x8 = number, args x0..x5, return in x0, `svc #0`. The kernel SVC path preserves every GPR
//            except x0.
//   x86_64:  rax = number, args rdi/rsi/rdx, return in rax, `syscall`. The instruction itself clobbers
//            rcx (the saved RIP) and r11 (the saved RFLAGS), so both are declared clobbered — omitting
//            them would let the compiler keep a live value in a register the CPU is about to destroy.
// ---------------------------------------------------------------------------------------------
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
const SYS_SLEEP_MS: u64 = 5;
const SYS_GETINFO: u64 = 7;
const SYS_WIN_CREATE: u64 = 29;
const SYS_WIN_PRESENT: u64 = 30;

#[cfg(target_arch = "aarch64")]
mod sysabi {
    #[inline(always)]
    pub unsafe fn sys1(n: u64, a0: u64) -> u64 {
        let mut r: u64;
        unsafe { core::arch::asm!("svc #0", inout("x0") a0 => r, in("x8") n, options(nostack)) };
        r
    }
    #[inline(always)]
    pub unsafe fn sys2(n: u64, a0: u64, a1: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "svc #0",
                inout("x0") a0 => r,
                in("x1") a1,
                in("x8") n,
                options(nostack),
            )
        };
        r
    }
    #[inline(always)]
    pub unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "svc #0",
                inout("x0") a0 => r,
                in("x1") a1,
                in("x2") a2,
                in("x8") n,
                options(nostack),
            )
        };
        r
    }
}

#[cfg(target_arch = "x86_64")]
mod sysabi {
    #[inline(always)]
    pub unsafe fn sys1(n: u64, a0: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                in("rdi") a0,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            )
        };
        r
    }
    #[inline(always)]
    pub unsafe fn sys2(n: u64, a0: u64, a1: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                in("rdi") a0,
                in("rsi") a1,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            )
        };
        r
    }
    #[inline(always)]
    pub unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                in("rdi") a0,
                in("rsi") a1,
                in("rdx") a2,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            )
        };
        r
    }
}

use sysabi::{sys1, sys2, sys3};

#[inline(always)]
fn write_bytes(p: *const u8, len: usize) {
    unsafe { sys3(SYS_WRITE, 1, p as u64, len as u64) };
}
#[inline(always)]
fn exit(code: i32) -> ! {
    unsafe { sys1(SYS_EXIT, code as u64) };
    loop {
        core::hint::spin_loop();
    }
}
#[inline(always)]
fn sleep_ms(ms: u64) {
    unsafe { sys1(SYS_SLEEP_MS, ms) };
}

/// `SYS_GETINFO`'s `#[repr(C)] UserInfo { pid: u64, ticks: u64 }`, landed by `copy_to_user` into this
/// program's own R+W data segment (never the R+X code page — the loader maps them as separate leaves and
/// `copy_to_user` refuses a read-only target, which is the point of the M6f hostile fixture).
static mut INFO: [u64; 2] = [0; 2];

/// Read the kernel's (pid, ticks). Returns (0, 0) if the copy is refused — the program keeps running on a
/// zero pid rather than exiting, because "no exit path" is this app's whole contract.
fn getinfo() -> (u64, u64) {
    let p = core::ptr::addr_of_mut!(INFO) as u64;
    if unsafe { sys1(SYS_GETINFO, p) } >> 63 != 0 {
        return (0, 0);
    }
    unsafe {
        let i = &*core::ptr::addr_of!(INFO);
        (i[0], i[1])
    }
}

// ---------------------------------------------------------------------------------------------
// Surface geometry + palette. 128x128 is `boot::FB_WIN_MAX_W/H` — exactly one 64 KiB window slot, the
// same negotiated geometry UVUG uses, so this app needs no new window ABI.
// ---------------------------------------------------------------------------------------------
const SW: i32 = 128;
const SH: i32 = 128;
const STRIDE: usize = 512; // ARGB8888 row stride (bytes) = SW * 4
const BG: u32 = 0xFF14_1A20; // opaque near-black, a shade off UVUG's grey so the two are told apart
const FRAME_C: u32 = 0xFF3A_4A5A; // border
const PID_C: u32 = 0xFFE8_C98A; // pid digits — warm, the loudest thing in the window
const CNT_C: u32 = 0xFF8A_C9E8; // frame-counter digits — cool
const SWEEP_C: u32 = 0xFF4A_E8A0; // the motion block

/// 5x7 digit glyphs, one byte per row, bit 4 = leftmost column. Digits only — this window has no text.
static GLYPHS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

#[inline(always)]
unsafe fn put_px(surf: *mut u8, x: i32, y: i32, color: u32) {
    if x < 0 || x >= SW || y < 0 || y >= SH {
        return;
    }
    let off = (y as usize) * STRIDE + (x as usize) * 4;
    (surf.add(off) as *mut u32).write_volatile(color);
}

unsafe fn fill_rect(surf: *mut u8, x0: i32, y0: i32, w: i32, h: i32, color: u32) {
    let mut y = y0;
    while y < y0 + h {
        let mut x = x0;
        while x < x0 + w {
            put_px(surf, x, y, color);
            x += 1;
        }
        y += 1;
    }
}

/// Draw one digit at `scale`, top-left (x, y). Glyph cell is 5x7 source pixels.
unsafe fn draw_digit(surf: *mut u8, d: usize, x: i32, y: i32, scale: i32, color: u32) {
    let g = &GLYPHS[d % 10];
    let mut row = 0i32;
    while row < 7 {
        let bits = g[row as usize];
        let mut col = 0i32;
        while col < 5 {
            if bits & (1 << (4 - col)) != 0 {
                fill_rect(surf, x + col * scale, y + row * scale, scale, scale, color);
            }
            col += 1;
        }
        row += 1;
    }
}

/// Decompose `v` into up to 8 decimal digits; returns (digits, count). 0 renders as a single "0".
fn digits_of(v: u64) -> ([u8; 8], usize) {
    let mut tmp = [0u8; 8];
    let mut n = 0usize;
    let mut x = v;
    if x == 0 {
        return ([0; 8], 1);
    }
    while x > 0 && n < 8 {
        tmp[n] = (x % 10) as u8;
        x /= 10;
        n += 1;
    }
    // Reverse into most-significant-first order.
    let mut out = [0u8; 8];
    let mut i = 0usize;
    while i < n {
        out[i] = tmp[n - 1 - i];
        i += 1;
    }
    (out, n)
}

/// Draw a decimal number horizontally centred in the window at row `y`.
unsafe fn draw_number(surf: *mut u8, v: u64, y: i32, scale: i32, color: u32) {
    let (d, n) = digits_of(v);
    let advance = 6 * scale; // 5 glyph columns + 1 column of gap
    let width = (n as i32) * advance - scale;
    let mut x = (SW - width) / 2;
    let mut i = 0usize;
    while i < n {
        draw_digit(surf, d[i] as usize, x, y, scale, color);
        x += advance;
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Tiny formatting into a byte buffer (no core::fmt — keep the text segment small). Same shape as
// user-vug's Buf.
// ---------------------------------------------------------------------------------------------
struct Buf {
    b: [u8; 96],
    n: usize,
}
impl Buf {
    fn new() -> Self {
        Buf { b: [0; 96], n: 0 }
    }
    fn put(&mut self, s: &[u8]) {
        let mut i = 0;
        while i < s.len() && self.n < self.b.len() {
            self.b[self.n] = s[i];
            self.n += 1;
            i += 1;
        }
    }
    fn put_dec(&mut self, v: u64) {
        let (d, n) = digits_of(v);
        let mut i = 0usize;
        while i < n {
            let c = b'0' + d[i];
            self.put(&[c]);
            i += 1;
        }
    }
    fn flush(&self) {
        write_bytes(self.b.as_ptr(), self.n);
    }
}

/// Frame at which the single `alive` line is printed — late enough that it is real evidence of a program
/// that kept looping (not a start line from a task that wedged one frame later), early enough that the
/// headless BGRUN-ST persistence leg sees it inside its window.
const ALIVE_MARK: u64 = 40;

const PAINT_INTERVAL_MS: u64 = 50; // ~20 fps

// ---------------------------------------------------------------------------------------------
// Program entry. Single-threaded; the loop below has NO break.
// ---------------------------------------------------------------------------------------------
#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let base = _start as *const () as u64; // window base B (entry VA == base, e_entry offset 0)

    // Create the window. Fail-closed: a negative return means no window (no free slot, no per-process FB
    // region) — there is nothing to draw into, so say so and exit rather than write an unmapped VA and take
    // a fatal EL0 abort. This is the ONLY path out of this program that is not a kill.
    let win = unsafe { sys2(SYS_WIN_CREATE, SW as u64, SH as u64) };
    if win >> 63 != 0 {
        let mut b = Buf::new();
        b.put(b":: STAT: SYS_WIN_CREATE failed ::\n");
        b.flush();
        exit(1);
    }
    // Surface VA: the window region's slot 0, at a FIXED offset from this program's own window base
    // (base + 0x5000 = `boot::fb_info_va() + FB_INFO_SIZE`). Nothing is guessed — the surface slot layout
    // is part of the window ABI (userspace.md), and the kernel publishes the geometry in the RO info page
    // at base + 0x4000.
    let surf = (base + 0x5000) as *mut u8;

    let (pid, _t0) = getinfo();

    let mut b = Buf::new();
    b.put(b":: STAT: start pid=");
    b.put_dec(pid);
    b.put(b" win=");
    b.put_dec(win);
    b.put(b" ::\n");
    b.flush();

    let mut frame: u64 = 0;
    let mut said_alive = false;
    loop {
        unsafe {
            // Background + a 1 px border so the window's own bounds are readable against the compositor's
            // chrome at any upscale (WC-SCALE takes the panel to 4x at the bench).
            fill_rect(surf, 0, 0, SW, SH, BG);
            fill_rect(surf, 0, 0, SW, 1, FRAME_C);
            fill_rect(surf, 0, SH - 1, SW, 1, FRAME_C);
            fill_rect(surf, 0, 0, 1, SH, FRAME_C);
            fill_rect(surf, SW - 1, 0, 1, SH, FRAME_C);

            // The pid, large: the ONLY thing that distinguishes two instances of this image on the panel,
            // and the number `jobs` prints and `kill` takes.
            draw_number(surf, pid, 16, 4, PID_C);
            // The frame counter, small: monotonic proof the loop is still turning.
            draw_number(surf, frame % 100_000, 60, 2, CNT_C);
            // The sweep: a block whose x tracks the frame counter. Motion at a glance, no reading.
            let sweep_x = ((frame * 2) % (SW as u64 - 20)) as i32;
            fill_rect(surf, sweep_x + 1, 96, 20, 12, SWEEP_C);
        }

        // Present. The return is checked but NOT fatal: a transient present error must not end a program
        // whose contract is "runs until killed" — and a persistent one is visible as a frozen window with a
        // still-advancing pid the operator can `kill`.
        let _ = unsafe { sys1(SYS_WIN_PRESENT, win) };

        frame += 1;
        if !said_alive && frame >= ALIVE_MARK {
            said_alive = true;
            let mut b = Buf::new();
            b.put(b":: STAT: alive pid=");
            b.put_dec(pid);
            b.put(b" frames=");
            b.put_dec(frame);
            b.put(b" ::\n");
            b.flush();
        }
        sleep_ms(PAINT_INTERVAL_MS);
        // No exit condition. `kill <pid>` is the whole remedy — BGRUN-1's stated contract for a bg app.
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
