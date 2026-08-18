#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PULSE-1: `PULSE.ELF` — the WINDOWED PULSE app. A static ELF64 EL0 program that creates one compositor
// window and draws the kernel's BeOS-Pulse homage inside it: one segmented bar per core, `PULSE_SEGS`
// segments, filled in proportion to that core's load over a 250 ms measurement window, with a text row per
// core carrying the percent — or `park` for a core the machine is not scheduling.
//
// PULSEFLUID — WHY THE METERS MOVE SMOOTHLY NOW, AND WHAT THAT DID AND DID NOT CHANGE ABOUT THE
// MEASUREMENT. Peter, on Boots AN/AO: "i feel like the pulse leds should be more fluid if you have so many
// threads going". They were chunky for two independent reasons, and only one of them was about the data:
//
//   1. THE REPORTING RATE, not the measurement, was 4 Hz. The old loop slept 250 ms, sampled, painted, and
//      threw the previous snapshot away — so a viewer saw a new number five times less often than the
//      instrument could honestly produce one. `SYS_CPUPULSE` reports CUMULATIVE counters, not a
//      per-second publication, so any pair of samples is a valid window: there is no kernel-side
//      publication cadence to fix. The loop now samples every `FRAME_MS` = 50 ms and keeps the last
//      `WINDOW_SLOTS` = 5 snapshots in a ring, so each frame's deltas still span a FULL 250 ms — the
//      identical measurement window, recomputed 5x more often. This is oversampling, not interpolation:
//      every painted percent is a real ratio of real counter deltas over a real 250 ms span. The window
//      SLIDES, which also makes the printed number steadier, because consecutive frames share 4/5 of
//      their evidence.
//
//   2. THE TRACK QUANTISED TO 10% and the breath stepped one whole segment at a time. `PULSE_SEGS` is 10,
//      so the bar could only ever be at a multiple of 10% and a load walking 34% -> 46% moved the bar in
//      one visible jerk. The leading segment is now BLENDED between `METER_DIM` and `METER_PURPLE` in
//      proportion to the fractional part (Q8, so ~0.4% of a segment), which resolves the same measured
//      value ~256x more finely without inventing any of it. The `RUNNING`/idle breath likewise travels as
//      a continuous glow with a triangular falloff rather than hopping between segments.
//
// AND ONE THING THAT IS DISPLAY SMOOTHING, LABELLED AS SUCH. The BAR's fill is an exponential moving
// average of the measured percents (`disp = (2*disp + measured)/3` per 50 ms frame, ~100 ms time
// constant). That is a filter over real measurements — a rendering choice about how a bar catches up, not
// a claim about the machine — and it is confined to the bar. The TEXT CELL and the serial first-window
// witness both print the RAW measured percent, unfiltered, so the number an operator reads and the number
// a headless gate parses are the measurement itself. The EMA is also RESET rather than ramped whenever a
// core crosses between a percent and a `park`/`run` sentinel: a sentinel is not a load and must never be
// averaged with one.
//
// PACING. The loop still SLEEPS (`SYS_SLEEP_MS`) between frames and never spins; 20 Hz on a 128x128
// surface is one blocking syscall per 50 ms. `SYS_INPUT_WAIT` (28) is deliberately not used — it is
// implemented on aarch64 only, and a monitor must repaint on a timer rather than on input anyway. What
// keeps the 5x present rate from costing anything when nobody can see it is the VUGMIN hidden bit: the RO
// info page's process-flags word (base + 0x4000, word 0x20, bit 1) says "every window this process owns is
// hidden below the shell", and while it is set the loop keeps SAMPLING (so the window behind the first
// visible frame is real) but skips the raster and the present entirely. That is strictly less compositor
// work than the old 4 Hz loop did while hidden, which was to present regardless.
//
// WHY IT EXISTS. The kernel already has that instrument: the `pulse` shell verb (`vug::run_pulse`) takes
// the WHOLE screen, runs inside the shell command, and draws every pixel in ring 0. As a full-screen mode
// it cannot sit on the desktop beside anything else, and it cannot be `bg`'d, TAB'd to, or moved. This
// program is the same instrument as a ring-3 WINDOW — sibling to `STAT.ELF` (crates/user-stat) and
// `VUG.ELF` (crates/user-vug) — reading the per-core counters across the privilege boundary through
// `SYS_CPUPULSE(49)` and doing its own delta math. The kernel therefore keeps exactly ONE sampler
// (`vug::CpuPulse`, untouched by this arc) and gains no second in-kernel drawing loop.
//
// THE HONESTY RULE CROSSES THE BOUNDARY WITH IT. `vug::classify_load`'s two-source rule is restated below
// in `classify` and must not be regressed here:
//   * counters MOVED this window (db + di > 0) -> the scheduler accounted this core; show its honest busy
//     fraction. This is the case for every core an SMP kernel is actually running tasks or idling on.
//   * counters FROZEN and it is the OBSERVER's own core -> we are demonstrably executing on it (we just
//     made a syscall from it), so it is not parked. We have no second measurement of HOW busy it is, so we
//     claim none: the bar breathes and the text reads `run`. In the kernel the equivalent branch credits
//     the render loop's own measured busy%, which is a source a ring-3 monitor simply does not have — and
//     inventing one would be a fabrication, which is the whole thing this rule exists to prevent.
//   * counters FROZEN and it is ANOTHER core -> parked / never-woken. DASHED bar, text `park`. NEVER a
//     percent, and never a 0% — a 0% bar means "scheduled and idle", which is a different fact about the
//     machine and must not read the same.
//
// THE BASELINE IS NOT A READING. The first sample is taken at startup and is a pure BASELINE: cumulative
// counters with no window behind them. Painting it would show every core frozen-and-parked, which is an
// artefact of the instrument rather than a fact about the machine. So the program SLEEPS a full
// measurement window before its first PAINT — under PULSEFLUID that is `WINDOW_SLOTS` sample frames, whose
// only job is to fill the history ring — and the first frame it ever presents already has a real 250 ms
// window behind it, exactly as before. (A counter that reads its own pre-run baseline as a live rate
// cannot falsify anything.) Nothing shortens this: a partly-filled ring would paint a 50 ms window while
// the doc, the text cell and the witness all say 250 ms, which is the same class of lie.
//
// EXIT. ESC or `q` closes the window; otherwise it runs until `kill`, like its two siblings. This is
// `VUG.ELF`'s idiom rather than the kernel monitor's "any key exits": as a full-screen shell mode, exiting
// on any key is right, but a DESKTOP window an operator is TAB-walking through must not close because a
// stray keypress landed in it. Input is drained bounded (`INPUT_PER_FRAME`) at the top of each frame — an
// unbounded drain is the UVUG-9 freeze.
//
// SERIAL. One start line and one alive line, then silence — a long-lived app must never be a firehose.
//
// EL0 owns only the OFF-SCREEN surface bytes — never the scan-out, never a physical address, never a kernel
// mapping (`SYS_WIN_PRESENT` is the only surface->screen path and it runs in the kernel). Window CHROME is
// drawn by the kernel from its own copy of the title, so this program cannot forge a frame.

// ---------------------------------------------------------------------------------------------
// Syscall ABI — the ARCH SPLIT. The NUMBERS are shared across arches by law: a syscall number names the
// same verb everywhere, so the constants below are declared once and are correct on both. Only the
// instruction and the register mapping differ, which is what the two `sysabi` modules encapsulate.
// Everything below that block is arch-neutral Rust that never mentions a register.
//
//   aarch64: x8 = number, args x0..x5, return in x0, `svc #0`. The kernel SVC path preserves every GPR
//            except x0.
//   x86_64:  rax = number, args rdi/rsi/rdx, return in rax, `syscall`. The instruction itself clobbers
//            rcx (the saved RIP) and r11 (the saved RFLAGS), so both are declared clobbered — and the
//            kernel's `sysretq` tail additionally SCRUBS rdi/rsi/rdx/r8/r9/r10 to zero on the way back
//            to ring 3 (U1b B1, `arch/x86_64/syscall.rs`), so every one of those six is declared
//            `inlateout(...) => _` (the arguments) or `lateout(...) => _` (r8/r9/r10, which no stub
//            below uses as an argument) — never a bare `in(...)`, which would promise the compiler a
//            value the kernel has already zeroed.
// ---------------------------------------------------------------------------------------------
// ABIFREEZE: the numbers are IMPORTED, not re-typed — `una_abi` is the one declaration, shared with
// both kernels. See its divergence ledger for what the eight scattered copies had already cost.
use una_abi::{
    SYS_EXIT, SYS_GETINFO, SYS_INPUT_POLL, SYS_SLEEP_MS, SYS_WIN_CREATE, SYS_WIN_PRESENT, SYS_WRITE,
};
/// PULSE-1's own verb: fill a caller buffer with the per-core load sample (see `Pulse` below).
///
/// ABIFREEZE (divergence D2): 49 is reserved on BOTH arches but DISPATCHED only by the x86 kernel —
/// the aarch64 dispatcher has no arm for it and answers `-ENOSYS`, despite the x86 declaration's own
/// claim that the number was "minted on BOTH arches". `sample()` below already treats a negative
/// return as "keep the previous picture", so PULSE.ELF degrades honestly on the Pi rather than
/// drawing fabricated bars; that gap is now stated in one place instead of being invisible.
use una_abi::SYS_CPUPULSE;

#[cfg(target_arch = "aarch64")]
mod sysabi {
    #[inline(always)]
    pub unsafe fn sys0(n: u64) -> u64 {
        let mut r: u64;
        unsafe { core::arch::asm!("svc #0", out("x0") r, in("x8") n, options(nostack)) };
        r
    }
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

/// x86_64 — U1b B1: the kernel's `sysretq` tail zeroes rdi/rsi/rdx/r8/r9/r10 before returning to ring
/// 3, on EVERY syscall, unconditionally, regardless of how many arguments that syscall took. So every
/// stub below — regardless of its own arity — must declare all six of those registers as NOT
/// surviving the `syscall` instruction: `inlateout(reg) a => _` for whichever ones happen to carry
/// that stub's own arguments, `lateout(reg) _` for the rest. Naming only the registers a given stub
/// passes (and leaving the others unnamed) is the arity mistake this block used to make: the
/// compiler is then free to believe an unnamed register — say `rdx` in a 2-argument stub — survives
/// the call and reuse it, and it does not; it reads back zero. A bare `in(reg)` is worse still,
/// promising rustc the register still holds its value afterward — a promise the kernel breaks — and
/// that is undefined behaviour that surfaces only on real hardware, never in QEMU or `check`.
#[cfg(target_arch = "x86_64")]
mod sysabi {
    #[inline(always)]
    pub unsafe fn sys0(n: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                lateout("rdi") _,
                lateout("rsi") _,
                lateout("rdx") _,
                lateout("rcx") _,
                lateout("r11") _,
                lateout("r8") _,
                lateout("r9") _,
                lateout("r10") _,
                options(nostack),
            )
        };
        r
    }
    #[inline(always)]
    pub unsafe fn sys1(n: u64, a0: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                inlateout("rdi") a0 => _,
                lateout("rsi") _,
                lateout("rdx") _,
                lateout("rcx") _,
                lateout("r11") _,
                lateout("r8") _,
                lateout("r9") _,
                lateout("r10") _,
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
                inlateout("rdi") a0 => _,
                inlateout("rsi") a1 => _,
                lateout("rdx") _,
                lateout("rcx") _,
                lateout("r11") _,
                lateout("r8") _,
                lateout("r9") _,
                lateout("r10") _,
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
                inlateout("rdi") a0 => _,
                inlateout("rsi") a1 => _,
                inlateout("rdx") a2 => _,
                lateout("rcx") _,
                lateout("r11") _,
                lateout("r8") _,
                lateout("r9") _,
                lateout("r10") _,
                options(nostack),
            )
        };
        r
    }
}

use sysabi::{sys0, sys1, sys2, sys3};

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
#[inline(always)]
fn input_poll() -> u64 {
    unsafe { sys0(SYS_INPUT_POLL) }
}

/// `SYS_GETINFO`'s `#[repr(C)] UserInfo { pid: u64, ticks: u64 }`, landed by `copy_to_user` into this
/// program's own R+W data segment (never the R+X code page — the loader maps them as separate leaves and
/// `copy_to_user` refuses a read-only target).
static mut INFO: [u64; 2] = [0; 2];

/// Read the kernel's (pid, ticks). Returns (0, 0) if the copy is refused, rather than exiting: a monitor
/// that cannot learn its own pid can still draw honest bars.
///
/// The second element is in KERNEL TICKS, whose rate is per-arch — use [`getinfo_ms`], never the raw
/// value, wherever a millisecond is meant.
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

/// ABIFREEZE (divergence D1): the wall-clock read, in MILLISECONDS, on either arch.
///
/// `SYS_GETINFO`'s `ticks` field is NOT milliseconds everywhere, and this program used to assume it
/// was: it named the raw value `ms` and fed it straight to `ms % BREATH_MS` against a 3000-ms period.
/// That is right on x86 (1 kHz APIC tick == 1 ms) and wrong by 4x on aarch64 (250 Hz scheduler tick),
/// where the liveness breath swept in 12 s instead of the 3 s its own comment promises. The kernels'
/// clocks are shipped, metal-proven behaviour and neither moves; the rate now comes from
/// `una_abi::GETINFO_TICK_HZ`, which is `cfg`-selected to match whichever kernel this image runs on.
#[inline(always)]
fn getinfo_ms() -> u64 {
    una_abi::getinfo_ticks_to_ms(getinfo().1)
}

// ---------------------------------------------------------------------------------------------
// SYS_CPUPULSE's payload — the ABI, restated on this side of the boundary.
//
// The kernel writes `#[repr(C)] UserPulse { ncpu: u64, demo: u64, ticks: [u64; MAX_CPUS * 2] }`: `ncpu` at
// offset 0, `demo` at 8, then one `(busy, idle)` pair of CUMULATIVE tick counts per core, core-major — core
// c's busy count at word `2 + c*2`, its idle count at word `3 + c*2`. All u64, no padding, 272 bytes. It is
// read here as a flat `[u64; 2 + MAX_CPUS*2]` because that IS the layout and a flat array needs no
// assumptions about field alignment that the repr does not already guarantee.
//
// `demo` is the core the kernel was executing THIS syscall on — i.e. the core running this program at the
// moment it sampled. That is the same role the field plays in `vug::CpuPulse::refresh` (the core running
// the render loop), which is what lets `classify` below read exactly like the kernel's rule. A `demo` value
// >= `ncpu` is legal and means "the observer is on a core outside the meter's coverage"; nothing here
// special-cases it, and the consequence is simply that no core takes the observer branch.
// ---------------------------------------------------------------------------------------------
// ABIFREEZE: the core cap and the word count are the KERNEL's, imported. They size the buffer the
// kernel copies into, so a local copy of `16` here was a silent buffer-overrun waiting for the day
// the ABI cap moved.
use una_abi::{PULSE_MAX_CPUS as MAX_CPUS, PULSE_WORDS};
static mut PULSE: [u64; PULSE_WORDS] = [0; PULSE_WORDS];

/// One sampled window: `ncpu`, the observer's core, and the per-core cumulative counters.
#[derive(Clone, Copy)]
struct Sample {
    ncpu: usize,
    demo: usize,
    ticks: [(u64, u64); MAX_CPUS],
}

/// Take a sample. `None` if the kernel refused the copy (bit 63 set) — the caller keeps the previous
/// picture rather than drawing a fabricated one.
fn sample() -> Option<Sample> {
    let p = core::ptr::addr_of_mut!(PULSE) as u64;
    if unsafe { sys1(SYS_CPUPULSE, p) } >> 63 != 0 {
        return None;
    }
    let w = unsafe { &*core::ptr::addr_of!(PULSE) };
    let ncpu = if w[0] as usize > MAX_CPUS { MAX_CPUS } else { w[0] as usize };
    let mut s = Sample { ncpu, demo: w[1] as usize, ticks: [(0, 0); MAX_CPUS] };
    let mut c = 0usize;
    while c < ncpu {
        s.ticks[c] = (w[2 + c * 2], w[3 + c * 2]);
        c += 1;
    }
    Some(s)
}

// ---------------------------------------------------------------------------------------------
// The display decision — `vug::classify_load`'s rule, restated for a ring-3 observer.
//
// `PARKED` and `RUNNING` are sentinels disjoint from the 0..=100 percent range, exactly as the kernel's
// `vug::PARKED` (`u32::MAX`) is. They are not loads; they are the two "no percent is honest here" answers.
// ---------------------------------------------------------------------------------------------
/// A core whose counters are frozen this window and that is NOT the observer's core: parked /
/// never-scheduled. Draws DASHED, prints `park`.
const PARKED: u32 = u32::MAX;
/// The observer's OWN core with frozen counters: demonstrably executing (we made the syscall from it), but
/// with no second measurement of how busy. Draws the liveness breath, prints `run` — a claim of life, never
/// a claim of load.
const RUNNING: u32 = u32::MAX - 1;

/// PULSEFLUID: the sliding measurement window's history ring — the last [`WINDOW_SLOTS`] snapshots of the
/// per-core cumulative counters, one written per successful sample. Slot `n % WINDOW_SLOTS` is read (the
/// snapshot from `WINDOW_SLOTS` samples ago, i.e. exactly one 250 ms window back) and then overwritten
/// with the current one, which is what lets a 5-deep ring express a sliding window with no copying.
///
/// It lives in `.bss` rather than on the stack because it is 1280 bytes and the EL0 stack is not the place
/// for it; the data page has ~8 KiB of headroom below the info page at base + 0x4000.
static mut HIST: [[(u64, u64); MAX_CPUS]; WINDOW_SLOTS] = [[(0, 0); MAX_CPUS]; WINDOW_SLOTS];

/// PULSEFLUID: blend two opaque ARGB colours. `t` runs 0..=256 (Q8): 0 is `a`, 256 is `b`. Per-channel,
/// alpha forced opaque — this surface has no transparency and a blended alpha would only be a way to
/// produce an invisible pixel by arithmetic accident.
#[inline(always)]
fn lerp(a: u32, b: u32, t: u32) -> u32 {
    let u = 256 - t;
    let r = (((a >> 16) & 0xFF) * u + ((b >> 16) & 0xFF) * t) >> 8;
    let g = (((a >> 8) & 0xFF) * u + ((b >> 8) & 0xFF) * t) >> 8;
    let bl = ((a & 0xFF) * u + (b & 0xFF) * t) >> 8;
    0xFF00_0000 | (r << 16) | (g << 8) | bl
}

/// The pure per-core display decision. `db`/`di` are one core's busy/idle tick DELTAS over the refresh
/// window; `is_self` is whether that core is the one the observer sampled from.
fn classify(db: u64, di: u64, is_self: bool) -> u32 {
    if db + di > 0 {
        ((db * 100) / (db + di)) as u32
    } else if is_self {
        RUNNING
    } else {
        PARKED
    }
}

// ---------------------------------------------------------------------------------------------
// Surface geometry + the meter palette.
//
// 128x128 is `boot::FB_WIN_MAX_W/H` — exactly one 64 KiB window slot, the geometry STAT and VUG use, so
// this app needs no new window ABI.
//
// The colours are the kernel meter palette (`vug::METER_*`) COPIED, not linked: EL0 cannot link kernel
// code, and the point of the copy is that the windowed instrument reads like the full-screen one. The
// kernel constants are 0x00RRGGBB (its PAL ignores the top byte); this surface is ARGB8888 and must be
// opaque, so each is OR'd with 0xFF000000.
// ---------------------------------------------------------------------------------------------
const SW: i32 = 128;
const SH: i32 = 128;
const STRIDE: usize = 512; // ARGB8888 row stride (bytes) = SW * 4

const BG: u32 = 0xFF10_0E16; // opaque near-black, a shade off STAT's and UVUG's so the three are told apart
const FRAME_C: u32 = 0xFF3A_4A5A; // border, STAT's
const METER_DIM: u32 = 0xFF2A_2432; // vug::METER_DIM — an unfilled segment: alive-but-empty, never blank
const METER_PURPLE: u32 = 0xFF9B_59B6; // vug::METER_PURPLE — the load fill
const METER_BREATH: u32 = 0xFF5F_4E86; // vug::METER_BREATH — the one swept segment of an idle-but-live core
const METER_PARKED: u32 = 0xFF3A_3550; // vug::METER_PARKED — the dashes of a core nothing schedules
const METER_LABEL: u32 = 0xFF8A_8296; // vug::METER_LABEL — core numbers and percents

/// Fixed segment count of every pulse bar — `vug::PULSE_SEGS`, copied. Filled ∝ load; the EMPTY segments
/// draw dim, so an idle core reads alive-but-empty rather than blank.
const PULSE_SEGS: i32 = 10;

// ---------------------------------------------------------------------------------------------
// 5x7 glyphs, one byte per row, bit 4 = leftmost column. Digits, `%`, and just the letters the three text
// verdicts need: `park`, `run`.
// ---------------------------------------------------------------------------------------------
static DIGITS: [[u8; 7]; 10] = [
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

/// The non-digit glyphs, by ASCII code. A `_` (or any character not listed) renders as blank, which is how
/// a right-aligned percent pads itself.
static LETTERS: [(u8, [u8; 7]); 7] = [
    (b'%', [0b11001, 0b11010, 0b00100, 0b00100, 0b01000, 0b01011, 0b10011]),
    (b'p', [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000]),
    (b'a', [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
    (b'r', [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001]),
    (b'k', [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001]),
    (b'u', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
    (b'n', [0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001]),
];

fn glyph(ch: u8) -> Option<&'static [u8; 7]> {
    if ch.is_ascii_digit() {
        return Some(&DIGITS[(ch - b'0') as usize]);
    }
    let mut i = 0usize;
    while i < LETTERS.len() {
        if LETTERS[i].0 == ch {
            return Some(&LETTERS[i].1);
        }
        i += 1;
    }
    None
}

#[inline(always)]
unsafe fn put_px(surf: *mut u8, x: i32, y: i32, color: u32) {
    if x < 0 || x >= SW || y < 0 || y >= SH {
        return;
    }
    let off = (y as usize) * STRIDE + (x as usize) * 4;
    (surf.add(off) as *mut u32).write_volatile(color);
}

/// Fill a rectangle, CLAMPED TO THE SURFACE BEFORE THE LOOP RUNS.
///
/// The clamp is the loop bound — that is the whole point, not a tidiness pass. With `while y < y0 + h`
/// the row bound was an unclamped runtime value, and LLVM compiled the row loop to a rotated
/// `do {} while (y != y0 + h)` over a 64-bit induction variable with no zero-trip guard, while the
/// `put_px` bounds check inlined beside it stayed 32-bit (`leal`/`orl`/`testl $0xffffff80`). Those two
/// widths disagree: every row congruent to 0 mod 2^32 passes a 32-bit guard, so an unreachable bound
/// meant ~2^64 iterations stepping the row pointer 512 B at a time, every store suppressed until the
/// counter wrapped and exactly one landed at `surf + 2^41`.
///
/// Clamping first is what bounds it. `ye` can never exceed `SH`, so the bound is always reachable and
/// the trip count is bounded by the surface. What the compiler actually emits (verified by disassembly,
/// not assumed): the exit test is STILL `cmpq`/`jne`, but it is now preceded by a signed zero-trip
/// guard — `cmpl %ye, %ys; jge <past the loop>` — because `ys > ye` is possible (a negative `h`, a rect
/// entirely below the surface) and the loop is therefore not provably non-empty. With `0 <= ys < ye <=
/// SH` established before entry, both bounds are zero-extended to 64-bit from values proven in range,
/// so the counter cannot outrun the bound and the 32-bit guard beside it can no longer disagree with
/// the 64-bit pointer.
///
/// NOTE the residue: `put_px`'s guard is still a 32-bit test. This clamp bounds the trip count; it does
/// not widen that guard. Anything that corrupts the counter MID-loop is outside what this can catch.
///
/// `put_px` keeps its own guard. It is now redundant for every call arriving through here, which is
/// the intent: belt and braces, and it still guards the callers that plot points directly.
unsafe fn fill_rect(surf: *mut u8, x0: i32, y0: i32, w: i32, h: i32, color: u32) {
    let xs = x0.max(0);
    let ys = y0.max(0);
    let xe = x0.saturating_add(w).min(SW);
    let ye = y0.saturating_add(h).min(SH);
    let mut y = ys;
    while y < ye {
        let mut x = xs;
        while x < xe {
            put_px(surf, x, y, color);
            x += 1;
        }
        y += 1;
    }
}

/// Draw one 5x7 glyph cell at scale 1, top-left (x, y). Unknown characters draw nothing (the padding case).
unsafe fn draw_glyph(surf: *mut u8, ch: u8, x: i32, y: i32, color: u32) {
    let Some(g) = glyph(ch) else { return };
    let mut row = 0i32;
    while row < 7 {
        let bits = g[row as usize];
        let mut col = 0i32;
        while col < 5 {
            if bits & (1 << (4 - col)) != 0 {
                put_px(surf, x + col, y + row, color);
            }
            col += 1;
        }
        row += 1;
    }
}

/// Draw a byte string left-to-right at 6 px advance (5 glyph columns + 1 gap).
unsafe fn draw_text(surf: *mut u8, s: &[u8], x: i32, y: i32, color: u32) {
    let mut i = 0usize;
    while i < s.len() {
        draw_glyph(surf, s[i], x + (i as i32) * 6, y, color);
        i += 1;
    }
}

/// PULSEFLUID: the breath's sweep period — one traversal of the whole track. The old stepped breath moved
/// one whole segment every 300 ms, which at `PULSE_SEGS` = 10 is this same 3 s traversal; the glow below
/// covers the same ground at the same speed, continuously instead of in ten jerks.
const BREATH_MS: u64 = 3000;

/// PULSEFLUID: how far (Q8 segments) the breath glow reaches either side of its centre. 320 = 1.25
/// segments, so the glow always spans two adjacent LEDs and the handoff between them is a cross-fade
/// rather than a jump.
const BREATH_SPAN_Q: i32 = 320;

/// PULSEFLUID: the minimum Q8 segment-fill a NONZERO load may draw — 96/256 = 37.5% of the first LED.
/// This is the sub-segment restatement of the old `if filled < 1 { filled = 1 }`: the law is that a live
/// load must never round down to an unlit track. The old rule paid for it by inflating everything from 1%
/// to 4% into a full segment; this floor is strictly nearer the truth AND still visibly lit.
const MIN_LIT_Q: u32 = 96;

/// One segmented pulse bar — `vug::draw_pulse_bar`'s three cases, in ARGB on our own surface. Returns the
/// x just past the bar.
///
/// `load` is the MEASURED verdict (`PARKED`, `RUNNING`, or a percent) and decides WHICH case applies.
/// `disp_q` is PULSEFLUID's display-smoothed percent in Q8 (percent << 8) and decides only HOW FULL the
/// track is drawn in the percent case — see the PULSEFLUID note in the file header for why exactly one of
/// those two is a filter and why it touches nothing else.
///
///   * `PARKED`  — every other segment `METER_PARKED`, the rest bare background: a BROKEN track. Visually
///     separable from an idle core's solid-dim track, so "never woken" never reads like "idle 0%".
///   * `RUNNING`, or a percent whose smoothed fill has decayed to nothing — the PULSE-ALIVE breath: a glow
///     swept along the bar by wall-clock time, `METER_BREATH` at its centre, fading into `METER_DIM`. An
///     idle-but-scheduled core that drew an all-dim track read as dead on the bench panel; a breathing one
///     reads alive. Not a load claim — the text still says 0% (or `run`).
///   * a percent — whole segments in `METER_PURPLE`, the LEADING segment blended between `METER_DIM` and
///     `METER_PURPLE` by the fractional part, the rest `METER_DIM`.
unsafe fn draw_pulse_bar(
    surf: *mut u8,
    x: i32,
    y: i32,
    seg_w: i32,
    seg_h: i32,
    gap: i32,
    load: u32,
    disp_q: u32,
    ms: u64,
) -> i32 {
    let mut bx = x;
    if load == PARKED {
        let mut s = 0i32;
        while s < PULSE_SEGS {
            if s % 2 == 0 {
                fill_rect(surf, bx, y, seg_w, seg_h, METER_PARKED);
            }
            bx += seg_w + gap;
            s += 1;
        }
        return bx;
    }
    // `RUNNING` claims life and no load, so it never has a fill; a measured percent whose smoothed fill has
    // decayed below a quarter of a percent has nothing left to draw either, and breathes instead.
    let q = if load == RUNNING { 0 } else { disp_q };
    if q < 64 {
        // The glow's centre, in Q8 segment units, swept across the track once per `BREATH_MS`.
        let phase = ((ms % BREATH_MS) * (PULSE_SEGS as u64) * 256 / BREATH_MS) as i32;
        let mut s = 0i32;
        while s < PULSE_SEGS {
            let d = (s * 256 + 128 - phase).abs();
            let t = if d >= BREATH_SPAN_Q {
                0
            } else {
                (((BREATH_SPAN_Q - d) * 256) / BREATH_SPAN_Q) as u32
            };
            fill_rect(surf, bx, y, seg_w, seg_h, lerp(METER_DIM, METER_BREATH, t));
            bx += seg_w + gap;
            s += 1;
        }
        return bx;
    }
    // Q8 segments = Q8 percent * PULSE_SEGS / 100. Never round a live load down to nothing (`MIN_LIT_Q`),
    // never overrun the track.
    let mut fq = q * PULSE_SEGS as u32 / 100;
    if fq < MIN_LIT_Q {
        fq = MIN_LIT_Q;
    }
    let full = (PULSE_SEGS as u32) << 8;
    if fq > full {
        fq = full;
    }
    let whole = (fq >> 8) as i32;
    let frac = fq & 0xFF;
    let mut s = 0i32;
    while s < PULSE_SEGS {
        let color = if s < whole {
            METER_PURPLE
        } else if s == whole && frac != 0 {
            lerp(METER_DIM, METER_PURPLE, frac)
        } else {
            METER_DIM
        };
        fill_rect(surf, bx, y, seg_w, seg_h, color);
        bx += seg_w + gap;
        s += 1;
    }
    bx
}

// ---------------------------------------------------------------------------------------------
// Tiny formatting into a byte buffer (no core::fmt — keep the text segment small). Same shape as
// user-stat's / user-vug's Buf.
// ---------------------------------------------------------------------------------------------
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
    let mut out = [0u8; 8];
    let mut i = 0usize;
    while i < n {
        out[i] = tmp[n - 1 - i];
        i += 1;
    }
    (out, n)
}

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

/// The 4-character verdict cell drawn to the right of a bar: `park`, ` run`, or a right-aligned percent
/// (`  0%` .. `100%`). Blanks pad, because `glyph` draws nothing for a space.
fn verdict_cell(load: u32) -> [u8; 4] {
    if load == PARKED {
        return *b"park";
    }
    if load == RUNNING {
        return *b" run";
    }
    let (d, n) = digits_of(load as u64);
    let mut out = [b' '; 4];
    out[3] = b'%';
    let mut i = 0usize;
    while i < n && i < 3 {
        out[3 - n + i] = b'0' + d[i];
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Pacing + input.
// ---------------------------------------------------------------------------------------------
/// PULSEFLUID: the FRAME period — the sleep between passes, and therefore both the sample rate and the
/// repaint rate. 20 Hz. This used to be the same number as the measurement window, which is why the meters
/// were chunky; the two are now separate quantities and only this one is a pacing choice.
const FRAME_MS: u64 = 50;

/// PULSEFLUID: how many frames the sliding measurement window spans. `FRAME_MS * WINDOW_SLOTS` is
/// [`REFRESH_MS`], so the deltas `classify` divides still cover a full 250 ms — the window did not get
/// shorter, it got recomputed more often.
const WINDOW_SLOTS: usize = 5;

/// The measurement window: 250 ms, unchanged. It is the delta window `classify` divides over, which is what
/// makes the displayed percent a statement about a period a viewer can perceive. Shorter would be a noisier
/// instrument, not a better one; longer would lag a load spike off the panel. It is no longer the repaint
/// period — see [`FRAME_MS`].
const REFRESH_MS: u64 = FRAME_MS * WINDOW_SLOTS as u64;

/// PULSEFLUID: the display EMA's numerator — `disp = (2*disp + measured)/3` per frame, a ~100 ms time
/// constant at [`FRAME_MS`]. Fast enough that a load step is on the panel within about two frames, slow
/// enough that frame-to-frame sampling jitter does not strobe the leading LED. It smooths the BAR only.
const EMA_KEEP: u32 = 2;
/// PULSEFLUID: the display EMA's denominator (see [`EMA_KEEP`]).
const EMA_DIV: u32 = 3;

/// Frame at which the single `alive` line is printed — late enough to be real evidence of a program that
/// kept looping, early enough for a headless witness to see it inside its window. PULSEFLUID rescaled it
/// with the frame rate: 8 frames used to be 2 s of loop life and is now 400 ms, so 40 restores the meaning.
const ALIVE_MARK: u64 = 40;

/// UVUG-9: the per-frame cap on input events. An UNBOUNDED drain is a loop that can poll forever without
/// ever reaching the present — the precise shape of the UVUG-9 freeze. Anything still queued waits one
/// frame, which at [`FRAME_MS`] is 50 ms (it was a quarter second before PULSEFLUID, so the cap now costs
/// five times less latency for the same protection).
const INPUT_PER_FRAME: u32 = 8;

use una_abi::{INPUT_EV_KEY_DOWN as EV_KEYDOWN, KEY_ESC as K_ESC};

/// Drain up to `INPUT_PER_FRAME` events; true if the user asked to close the window (ESC or `q`). Every
/// other event is consumed and ignored — this instrument has no controls.
fn wants_quit() -> bool {
    let mut quit = false;
    let mut i = 0u32;
    while i < INPUT_PER_FRAME {
        let ev = input_poll();
        if ev >> 63 != 0 {
            break; // -EAGAIN: the ring is empty
        }
        let kind = una_abi::input_ev_type(ev);
        if kind == EV_KEYDOWN {
            let k = (ev & 0xFF) as u8;
            if k == K_ESC || k == b'q' || k == b'Q' {
                quit = true;
            }
        }
        i += 1;
    }
    quit
}

// ---------------------------------------------------------------------------------------------
// Program entry. Single-threaded — a monitor needs no workers, and nothing UVUG already proves about
// SYS_THREAD_SPAWN / SYS_FUTEX is re-proved here.
// ---------------------------------------------------------------------------------------------
#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let base = _start as *const () as u64; // window base B (entry VA == base, e_entry offset 0)

    // Create the window. Fail-closed: a negative return means no window (no free slot, no per-process FB
    // region) — there is nothing to draw into, so say so and exit rather than write an unmapped VA and take
    // a fatal ring-3 abort.
    let win = unsafe { sys2(SYS_WIN_CREATE, SW as u64, SH as u64) };
    if win >> 63 != 0 {
        let mut b = Buf::new();
        b.put(b":: PULSE-A: SYS_WIN_CREATE failed ::\n");
        b.flush();
        exit(1);
    }
    // Surface VA: the window region's slot 0, at a FIXED offset from this program's own window base
    // (base + 0x5000 = `boot::fb_info_va() + FB_INFO_SIZE`). Nothing is guessed — the surface slot layout
    // is part of the window ABI (userspace.md), and the kernel publishes the geometry in the RO info page
    // at base + 0x4000.
    let surf = (base + 0x5000) as *mut u8;

    // VUGMIN: the process-flags word the kernel publishes in the RO info page (base + 0x4000, word 0x20;
    // bit 1 = every window this process owns is hidden below the shell). Read AFTER `SYS_WIN_CREATE`,
    // which is what maps that page. The page is EL0-RO for this process's whole life, so a `read_volatile`
    // of a mapped word each frame costs less than the branch that consumes it.
    let flags_p = unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4) };

    let (pid, _t0) = getinfo();

    // The BASELINE sample. Not a reading: cumulative counters with no window behind them. If the verb is
    // refused outright there is nothing honest to draw, so say so and exit — a pulse window that cannot
    // read the pulse would be a lie shaped like an instrument.
    let Some(mut prev) = sample() else {
        let mut b = Buf::new();
        b.put(b":: PULSE-A: SYS_CPUPULSE refused - no honest sample, exiting ::\n");
        b.flush();
        exit(2);
    };

    let mut b = Buf::new();
    b.put(b":: PULSE-A: start pid=");
    b.put_dec(pid);
    b.put(b" win=");
    b.put_dec(win);
    b.put(b" ncpu=");
    b.put_dec(prev.ncpu as u64);
    b.put(b" cpu=");
    b.put_dec(prev.demo as u64);
    b.put(b" segs=");
    b.put_dec(PULSE_SEGS as u64);
    // PULSEFLUID: the two pacing numbers, on the wire. They used to be one number and it was implicit;
    // now that the measurement window and the frame period are different quantities, a capture that shows
    // a percent must also say what span that percent covers and how often it was recomputed, or the
    // reading cannot be checked against `[schedx86] load` after the fact.
    b.put(b" win_ms=");
    b.put_dec(REFRESH_MS);
    b.put(b" frame_ms=");
    b.put_dec(FRAME_MS);
    b.put(b" ::\n");
    b.flush();

    // Row metrics, derived from the core count so 1 core and 16 cores both fit the one window slot.
    let ncpu = if prev.ncpu == 0 { 1 } else { prev.ncpu } as i32;
    let mut row_h = (SH - 8) / ncpu;
    if row_h > 20 {
        row_h = 20;
    }
    if row_h < 7 {
        row_h = 7;
    }
    let mut seg_h = row_h - 3;
    if seg_h > 12 {
        seg_h = 12;
    }
    if seg_h < 4 {
        seg_h = 4;
    }
    let gap = 1;
    let seg_w = 7;
    let bar_x = 15;
    let text_x = bar_x + PULSE_SEGS * (seg_w + gap) + 2;
    let text_dy = if seg_h > 7 { (seg_h - 7) / 2 } else { 0 };

    // PULSEFLUID: prime every history slot with the BASELINE, so the first `WINDOW_SLOTS` frames read a
    // snapshot that exists. Those frames are still not painted (see `filled` below) — priming makes the
    // ring well-defined, it does not make a partial window paintable.
    unsafe {
        let h = &mut *core::ptr::addr_of_mut!(HIST);
        let mut s = 0usize;
        while s < WINDOW_SLOTS {
            h[s] = prev.ticks;
            s += 1;
        }
    }

    // LAUNCH-DRAW (GR27): the FIRST present happens NOW, on the launch instant — before the
    // measurement window has seen a single frame. Until this arc, the first paint was gated on
    // `filled` (a full `REFRESH_MS` = 250 ms of samples), so launching pulse bought the operator
    // ~300 ms of blank glass while the sampler's first window ran ON the launch path; the
    // launch-stall arc flagged that hitch after WCD-CHUNK removed the bigger one under it.
    //
    // What this frame may honestly say is exactly "the meter is here, nothing measured yet":
    // chrome, border, core labels and UNLIT tracks (every segment `METER_DIM`, the resting shade),
    // with the verdict cells left blank. Deliberately NOT `park`/`run`/a percent and NOT the
    // PULSE-ALIVE breath — each of those is a claim about the machine, and none has been measured
    // (the two-source rule: no invented numbers). The SAMPLER IS UNTOUCHED: the ring still fills
    // over `WINDOW_SLOTS` frames, the first measured paint still lands at ~250 ms, and the
    // `first-window` witness still spans a full `REFRESH_MS` with the same deltas as before — only
    // the blank-glass gap moved off the launch instant.
    unsafe {
        fill_rect(surf, 0, 0, SW, SH, BG);
        fill_rect(surf, 0, 0, SW, 1, FRAME_C);
        fill_rect(surf, 0, SH - 1, SW, 1, FRAME_C);
        fill_rect(surf, 0, 0, 1, SH, FRAME_C);
        fill_rect(surf, SW - 1, 0, 1, SH, FRAME_C);
        let mut y = 4;
        let mut c = 0usize;
        while c < prev.ncpu {
            let (d, n) = digits_of(c as u64 + 1);
            let mut lbl = [b' '; 2];
            let mut i = 0usize;
            while i < n && i < 2 {
                lbl[2 - n + i] = b'0' + d[i];
                i += 1;
            }
            draw_text(surf, &lbl, 3, y + text_dy, METER_LABEL);
            let mut bx = bar_x;
            let mut s = 0i32;
            while s < PULSE_SEGS {
                fill_rect(surf, bx, y, seg_w, seg_h, METER_DIM);
                bx += seg_w + gap;
                s += 1;
            }
            y += row_h;
            c += 1;
        }
    }
    // Same non-fatal contract as the loop's present: a refusal must not end a monitor.
    let _ = unsafe { sys1(SYS_WIN_PRESENT, win) };

    let mut load = [PARKED; MAX_CPUS]; // the MEASURED verdict — text cell, serial witness, bar case
    let mut disp = [0u32; MAX_CPUS]; // PULSEFLUID: the display-smoothed percent, Q8, BAR ONLY
    let mut disp_live = [false; MAX_CPUS]; // has `disp[c]` a percent to ramp from, or must it snap?
    let mut frame: u64 = 0;
    let mut samples: u64 = 0; // successful samples — the history ring's index, NOT the frame count
    let mut said_alive = false;
    let mut said_window = false;
    loop {
        if wants_quit() {
            break;
        }

        // THE FRAME closes here: sleep first, THEN take the sample. The MEASUREMENT window is the last
        // `WINDOW_SLOTS` of these frames, so no frame is ever painted from the startup baseline and every
        // painted frame's deltas span a full `REFRESH_MS`.
        sleep_ms(FRAME_MS);
        // ABIFREEZE D1: milliseconds, converted from the arch's own tick rate — NOT the raw `ticks`
        // field, which is 250 Hz on aarch64 and 1 kHz on x86.
        let ms = getinfo_ms();
        if let Some(now) = sample() {
            // The window's OPENING snapshot: the slot we are about to overwrite holds the counters from
            // exactly `WINDOW_SLOTS` samples ago. Read it out, then store the current one in its place —
            // a 5-deep ring expressing a sliding window with no copying.
            let slot = (samples as usize) % WINDOW_SLOTS;
            let base_ticks = unsafe {
                let h = &mut *core::ptr::addr_of_mut!(HIST);
                let old = h[slot];
                h[slot] = now.ticks;
                old
            };
            samples += 1;
            let mut c = 0usize;
            while c < now.ncpu {
                let db = now.ticks[c].0.wrapping_sub(base_ticks[c].0);
                let di = now.ticks[c].1.wrapping_sub(base_ticks[c].1);
                load[c] = classify(db, di, c == now.demo);
                c += 1;
            }
            // PULSE-ALIVE evidence, EL0 side: ONE bounded line on the FIRST real window, carrying the raw
            // per-core deltas AND the verdict each one produced. It is the twin of `CpuPulse::refresh`'s
            // `first-window per-core busy/idle tick deltas` line, and it exists for the same reason: the
            // rendered bars are invisible to a headless gate, so without it "the app drew something" and
            // "the app drew the RIGHT something" are indistinguishable. It also falsifies a garbled
            // payload — a wrong struct layout shows up here as absurd deltas, not as a plausible picture.
            //
            // PULSEFLUID gates it on the ring being FULL, so it still describes a 250 ms window and is
            // still emitted on the first frame this program paints. What it prints is the RAW measured
            // delta and the RAW verdict — the display EMA below is not in this path and must never be.
            if !said_window && samples >= WINDOW_SLOTS as u64 {
                said_window = true;
                // ONE LINE PER CORE, not one line for all of them. A single wide line is what the kernel's
                // twin emits, and on a contended serial path it gets TORN: another core's writer lands in
                // the middle of it and the tail of the evidence is unreadable (observed on the first
                // FAT-attached run of this arc, shredded by a `[wc-d] verify` line). Evidence an unrelated
                // writer can shred is not evidence. Each core's verdict now stands alone and short, so an
                // interleave can cost at most one core's reading instead of all of them. Bounded: `ncpu`
                // lines, one-shot, at most 16.
                let mut c = 0usize;
                while c < now.ncpu {
                    let db = now.ticks[c].0.wrapping_sub(base_ticks[c].0);
                    let di = now.ticks[c].1.wrapping_sub(base_ticks[c].1);
                    let mut b = Buf::new();
                    b.put(b":: PULSE-A: first-window c");
                    b.put_dec(c as u64);
                    b.put(b" busy/idle=");
                    b.put_dec(db);
                    b.put(b"/");
                    b.put_dec(di);
                    b.put(b" -> ");
                    match load[c] {
                        PARKED => b.put(b"park"),
                        RUNNING => b.put(b"run"),
                        pct => {
                            b.put_dec(pct as u64);
                            b.put(b"%");
                        }
                    }
                    if c == now.demo {
                        b.put(b" (observer)");
                    }
                    b.put(b" ::\n");
                    b.flush();
                    c += 1;
                }
            }
            prev = now;
        }
        // A refused sample leaves `load` exactly as it was: the last honest picture, held. It does not
        // decay to zeros, because 0% is a claim about the machine and we have not measured it.

        // PULSEFLUID: has the ring seen a full `REFRESH_MS` yet? Until it has, this program paints nothing
        // — see the BASELINE note in the header. A refused sample does not advance `samples`, so a stream
        // of refusals cannot fake a full window into existence either.
        let filled = samples >= WINDOW_SLOTS as u64;

        // PULSEFLUID — THE DISPLAY EMA. Purely a rendering filter over the measured percents, and confined
        // to `disp`, which only [`draw_pulse_bar`]'s fill reads. Two properties it must keep:
        //
        //   * A SENTINEL IS NOT A LOAD. `PARKED` and `RUNNING` are `u32::MAX`-adjacent answers meaning "no
        //     percent is honest here". Feeding either into an average would produce an enormous fake load
        //     AND would carry state across the boundary between "measured" and "not measured", so both
        //     instead INVALIDATE the filter: the next real percent snaps rather than ramps.
        //   * THE FIRST PERCENT SNAPS. Ramping a fresh core up from 0 would draw a rise that never
        //     happened. `disp_live` is what distinguishes "converging towards a value" from "has no value".
        //   * IT NEVER FOLDS AN UNDER-WINDOW SAMPLE. Review-caught: ungated, this ran on frames 1..4,
        //     whose spans are 50/100/150/200 ms — so the FIRST bar anyone sees would have been an average
        //     containing sub-`win_ms` measurements while the text cell, the witness and the start line all
        //     said 250 ms. That is exactly the lie the header's baseline rule forbids, and it also defeated
        //     "the first percent snaps" for the first *visible* frame. Gated on `filled`, the filter cannot
        //     start before a real window exists.
        if filled {
            let mut c = 0usize;
            while c < prev.ncpu {
                match load[c] {
                    PARKED | RUNNING => disp_live[c] = false,
                    pct => {
                        let target = pct << 8; // Q8 percent
                        disp[c] = if disp_live[c] {
                            (disp[c] * EMA_KEEP + target) / EMA_DIV
                        } else {
                            disp_live[c] = true;
                            target
                        };
                    }
                }
                c += 1;
            }
        }

        // VUGMIN: while every window this process owns is hidden below the shell, there is nothing for a
        // raster or a present to accomplish — so do neither, and keep sampling. Skipping the present is
        // what pays for the 5x frame rate: a hidden PULSE costs the compositor strictly less than it did
        // at 4 Hz, and the frame it draws on becoming visible again is built from a window that never
        // stopped being measured.
        //
        // `frame` counts LOOP PASSES, as it always has — not presents — so the `alive` line is emitted on
        // the same schedule whether or not the window happens to be visible, and a hidden monitor still
        // proves it is looping.
        frame += 1;
        if !said_alive && frame >= ALIVE_MARK {
            said_alive = true;
            let mut b = Buf::new();
            b.put(b":: PULSE-A: alive pid=");
            b.put_dec(pid);
            b.put(b" frames=");
            b.put_dec(frame);
            b.put(b" ::\n");
            b.flush();
        }
        let hidden = unsafe { flags_p.read_volatile() } & 2 != 0;
        if hidden || !filled {
            continue;
        }

        unsafe {
            // Background + a 1 px border so the window's own bounds are readable against the compositor's
            // chrome at any upscale.
            fill_rect(surf, 0, 0, SW, SH, BG);
            fill_rect(surf, 0, 0, SW, 1, FRAME_C);
            fill_rect(surf, 0, SH - 1, SW, 1, FRAME_C);
            fill_rect(surf, 0, 0, 1, SH, FRAME_C);
            fill_rect(surf, SW - 1, 0, 1, SH, FRAME_C);

            // One row per core: the core number, the segmented bar, the verdict cell.
            let mut y = 4;
            let mut c = 0usize;
            while c < prev.ncpu {
                let (d, n) = digits_of(c as u64 + 1);
                let mut lbl = [b' '; 2];
                let mut i = 0usize;
                while i < n && i < 2 {
                    lbl[2 - n + i] = b'0' + d[i];
                    i += 1;
                }
                draw_text(surf, &lbl, 3, y + text_dy, METER_LABEL);
                draw_pulse_bar(surf, bar_x, y, seg_w, seg_h, gap, load[c], disp[c], ms);
                draw_text(surf, &verdict_cell(load[c]), text_x, y + text_dy, METER_LABEL);
                y += row_h;
                c += 1;
            }
        }

        // Present. The return is checked but NOT fatal: a transient present error must not end a monitor,
        // and a persistent one is visible as a frozen window the operator can `kill`.
        let _ = unsafe { sys1(SYS_WIN_PRESENT, win) };
    }

    let mut b = Buf::new();
    b.put(b":: PULSE-A: closed pid=");
    b.put_dec(pid);
    b.put(b" frames=");
    b.put_dec(frame);
    b.put(b" ::\n");
    b.flush();
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
