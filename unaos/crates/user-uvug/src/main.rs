#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UVUG-1: the first real EL0 graphics program — a userspace mini-vug, and the affirmative answer to "is it
// possible to make EL0 vug more multi-threaded?". A static ELF64 (aarch64) program, loaded by the kernel's
// EXEC-1 machinery (`run_user_image`) into a fresh per-process slot and run at EL0. It:
//
//   1. maps its dedicated off-screen surface via SYS_FB_MAP (a 32x32 ARGB8888 page + a read-only info page),
//   2. spawns TWO PERSISTENT EL0 worker threads via SYS_THREAD_SPAWN — one co-located, one on a SIBLING
//      CORE — each of which renders HALF of an animated integer-math gradient into the shared surface,
//   3. drives a per-frame barrier: each frame the parent publishes the frame number (the `phase` word) to
//      RELEASE both workers, then blocks on a real FUTEX (SYS_FUTEX WAIT on the `done` word) until both
//      workers have ARRIVED (each wakes the parent with SYS_FUTEX WAKE), then PRESENTS (SYS_FB_PRESENT) —
//      the kernel composites the surface to the real screen through the registered present hook,
//   4. after UVUG_FRAMES frames the workers exit (SYS_THREAD_EXIT), the parent joins both (SYS_THREAD_JOIN),
//      computes a deterministic FNV-1a checksum of the final surface, prints the witness
//      `:: UVUG: frames=<n> threads=2 checksum=<hex> ::` via SYS_WRITE, and exits 0 (SYS_EXIT).
//
// Barrier direction split (deliberate, for robustness under QEMU raspi4b, which delivers NO Group-1 timer
// IRQ — see docs userspace.md M6e — so a core that parks with nothing else runnable is not preemptively
// rescheduled):
//   * ARRIVAL (worker -> parent): a real FUTEX. The workers atomically bump `done` and SYS_FUTEX WAKE the
//     parent; the parent SYS_FUTEX WAITs on `done`. Reliable because the run core is kept cycling by the
//     kernel's `run_user_image` driver loop, so the wake is always promptly picked up.
//   * RELEASE (parent -> worker): a YIELD-POLL. Each worker spins on the `phase` word with SYS_YIELD until
//     it changes. Polling keeps the worker RUNNABLE on its core (so a sibling core with only this worker
//     keeps cooperatively re-dispatching it, even with no timer IRQ) and depends on no cross-core wake —
//     the parent never has to re-dispatch an idle sibling core. Both wait loops re-check their condition, so
//     the barrier is lost-wakeup-safe by construction. On metal (real timer IRQs) either direction works.
//
// The witness checksum is deterministic: the final surface is a pure integer function of pixel position and
// the last frame index, independent of thread interleaving, so QEMU and metal agree.
//
// EL0 owns only the OFF-SCREEN surface bytes — never the scan-out, never a physical address, never a kernel
// mapping (SYS_FB_PRESENT is the only surface->screen path, and it runs in the kernel). Page-permission laws
// (per-page perms, WXN) are untouched.
//
// ABI (Linux-aarch64): x8 = syscall number, args x0..x5, return in x0. The kernel SVC path preserves every
// GPR except x0, so state carried across `svc` in x9..x28 survives. Syscalls used:
//   SYS_WRITE=1, SYS_EXIT=2, SYS_YIELD=4, SYS_THREAD_SPAWN=21, SYS_THREAD_EXIT=22, SYS_THREAD_JOIN=23,
//   SYS_FB_MAP=24, SYS_FB_PRESENT=25, SYS_FUTEX=26 (op 0 = WAIT, op 1 = WAKE).
//
// Window / surface layout (base B = the slot's 16 KiB user window, set up by the loader):
//   B+0x0000  code page          (R+X)   — this .text segment (`_start` first, so e_entry = B)
//   B+0x1000  data page          (R+W)   — this .data segment: the phase/done words + the witness message
//   B+0x3000  worker A stack top          (register-only worker; the shared window backing is EL0-RW)
//   B+0x3800  worker B stack top
//   B+0x4000  window top = the parent's initial SP_EL0 (set by the loader) + the RO info page
//   B+0x5000  the mapped off-screen surface (32x32 ARGB8888 = 4096 bytes = one page)

use core::arch::global_asm;

// The whole program as explicit-section asm so the linker script's PHDRS assigns .text to the R+X segment
// and .data to the R+W segment. `_start` is forced first in .text (KEEP .text.entry) so the ELF entry point
// (e_entry) lands on it and the loader enters at bias + e_entry = the window base.
//
// UVUG_FRAMES = 300 animated frames (bounded); the surface is 32x32 with a 128-byte stride (ARGB8888).
global_asm!(
    r#"
.section .text.entry,"ax",@progbits
.balign 4
.globl _start
_start:
    // Parent. x19..x25 hold state across `svc` (the kernel preserves all GPRs but x0).
    adr  x19, _start                       // x19 = window base B (PC-relative; _start is at VA 0)
    mov  x8, #24                           // SYS_FB_MAP -> x0 = surface VA (== B+0x5000)
    svc  #0
    mov  x22, x0                           // x22 = surface VA
    adr  x20, g_phase                      // x20 = &phase (release word)
    adr  x21, g_done                       // x21 = &done  (arrival word)
    str  wzr, [x20]                        // phase = 0
    str  wzr, [x21]                        // done  = 0
    dmb  ish

    // Spawn worker A: place 0 (caller core), arg 0 (top half), stack top B+0x3000.
    adr  x0, __uvug_worker
    add  x1, x19, #0x3000
    mov  x2, #0
    mov  x3, #0
    mov  x8, #21                           // SYS_THREAD_SPAWN
    svc  #0
    mov  x23, x0                           // x23 = handle_a

    // Spawn worker B: place 1 (sibling core), arg 1 (bottom half), stack top B+0x3800.
    adr  x0, __uvug_worker
    add  x1, x19, #0x3000
    add  x1, x1, #0x800
    mov  x2, #1
    mov  x3, #1
    mov  x8, #21
    svc  #0
    mov  x24, x0                           // x24 = handle_b

    mov  x25, #0                           // x25 = frame counter (0 .. UVUG_FRAMES-1)
.Lframe:
    // Release both workers for this frame: reset the arrival word, then publish phase = frame+1. The workers
    // yield-poll `phase`, so no cross-core wake is needed here.
    str  wzr, [x21]                        // done = 0 (both workers are yield-polling phase here)
    dmb  ish
    add  w9, w25, #1                       // phase value = frame+1 (1-based; the worker's frame param)
    str  w9, [x20]
    dmb  ish
    // Block until both workers have arrived (done == 2), via a real FUTEX WAIT (workers WAKE us).
.Lwait:
    ldr  w9, [x21]                         // v = done
    cmp  w9, #2
    b.ge .Lpresent
    mov  x0, x21                           // SYS_FUTEX(&done, FUTEX_WAIT, v)
    mov  x1, #0
    mov  w2, w9
    mov  x8, #26
    svc  #0
    b    .Lwait
.Lpresent:
    mov  x8, #25                           // SYS_FB_PRESENT (kernel composites + checksums the surface)
    svc  #0
    add  x25, x25, #1                      // frame++
    cmp  x25, #300                         // UVUG_FRAMES
    b.lt .Lframe

    // Join both workers (they exited after rendering the final frame).
    mov  x0, x23
    mov  x8, #23                           // SYS_THREAD_JOIN(handle_a)
    svc  #0
    mov  x0, x24
    mov  x8, #23                           // SYS_THREAD_JOIN(handle_b)
    svc  #0

    // FNV-1a 64-bit over the final surface (4096 bytes) -> x9. Deterministic (the surface is frame 300).
    movz x9,  #0x2325
    movk x9,  #0x8422, lsl #16
    movk x9,  #0x9ce4, lsl #32
    movk x9,  #0xcbf2, lsl #48             // h = 0xcbf29ce484222325 (FNV offset basis)
    movz x10, #0x01b3
    movk x10, #0x0001, lsl #32             // prime = 0x00000100000001b3
    mov  x11, #0                           // i
.Lcksum:
    ldrb w12, [x22, x11]
    eor  x9, x9, x12
    mul  x9, x9, x10
    add  x11, x11, #1
    cmp  x11, #4096
    b.lt .Lcksum

    // Format the 16 hex digits (MSB first) into the witness message's placeholder (g_hex, in .data R+W).
    adr  x13, g_hex
    mov  x14, #0                           // digit index i (0..15)
.Lhex:
    mov  x15, #15
    sub  x15, x15, x14
    lsl  x15, x15, #2                      // shift = (15-i)*4
    lsr  x16, x9, x15
    and  x16, x16, #0xf                    // nibble
    cmp  x16, #10
    b.lo .Lhex_dec
    add  x16, x16, #0x57                   // 'a'-10
    b    .Lhex_put
.Lhex_dec:
    add  x16, x16, #0x30                   // '0'
.Lhex_put:
    strb w16, [x13, x14]
    add  x14, x14, #1
    cmp  x14, #16
    b.lo .Lhex

    // Print the witness line: SYS_WRITE(fd=1, uvug_msg, uvug_msgend - uvug_msg).
    mov  x0, #1
    adr  x1, uvug_msg
    adr  x2, uvug_msgend
    sub  x2, x2, x1
    mov  x8, #1
    svc  #0

    mov  x0, #0                            // SYS_EXIT(0)
    mov  x8, #2
    svc  #0
0:  b 0b                                   // sys_exit never returns; spin as a guard

    // ---------------------------------------------------------------------------------------------
    // The worker. x0 = arg (0 = top half / rows 0..15, 1 = bottom half / rows 16..31). Renders its half of
    // an animated gradient each frame, under the barrier, until it has rendered the final frame.
    .balign 4
    .globl __uvug_worker
__uvug_worker:
    adr  x9,  _start                       // window base B
    add  x28, x9, #0x5000                  // x28 = surface base (B+0x5000)
    adr  x27, g_phase                      // x27 = &phase
    adr  x26, g_done                       // x26 = &done
    lsl  x25, x0, #4                       // x25 = y0 = arg*16  (0 -> 0, 1 -> 16)
    add  x24, x25, #16                     // x24 = y1 = y0+16
    mov  w23, #0                           // w23 = last (the phase value already rendered)
.Lwloop:
    // Wait for the parent to release the next frame: yield-poll `phase` until it changes. Polling (not a
    // futex park) keeps this worker runnable on its core, so it needs no cross-core wake (QEMU raspi4b has
    // no timer IRQ to re-dispatch an idle core).
.Lwwait:
    ldr  w4, [x27]                         // p = phase
    cmp  w4, w23
    b.ne .Lwgot
    mov  x8, #4                            // SYS_YIELD (cooperative; re-dispatched on the same core)
    svc  #0
    b    .Lwwait
.Lwgot:
    mov  w23, w4                           // last = p; p (w4) = frame index 1..300, the animation param

    // Render this worker's half: for y in [y0,y1), x in [0,32): pixel(x,y) = f(x,y,p).
    mov  x13, x25                          // y = y0
.Lrow:
    cmp  x13, x24
    b.ge .Lrend
    mov  x14, #0                           // x = 0
.Lcol:
    cmp  x14, #32
    b.ge .Lrownext
    lsl  x15, x13, #7                      // y*128 (stride)
    add  x15, x15, x14, lsl #2             // + x*4 -> byte offset into the surface
    // r = (x*8 + p) & 0xff
    lsl  w16, w14, #3
    add  w16, w16, w4
    and  w16, w16, #0xff
    // g = (y*8 + p) & 0xff
    lsl  w17, w13, #3
    add  w17, w17, w4
    and  w17, w17, #0xff
    // b = (x + y + p) & 0xff
    add  w18, w14, w13
    add  w18, w18, w4
    and  w18, w18, #0xff
    // color = 0xFF000000 | (r<<16) | (g<<8) | b
    movz w5, #0xff00, lsl #16              // 0xFF000000 (opaque alpha)
    orr  w5, w5, w16, lsl #16
    orr  w5, w5, w17, lsl #8
    orr  w5, w5, w18
    str  w5, [x28, x15]
    add  x14, x14, #1
    b    .Lcol
.Lrownext:
    add  x13, x13, #1
    b    .Lrow
.Lrend:
    dmb  ish                               // publish the drawn bytes before signalling arrival

    // Atomically increment the arrival word (A72 LL/SC — ARMv8.0, no LSE), then WAKE the parent (FUTEX).
.Linc:
    ldxr w6, [x26]
    add  w6, w6, #1
    stxr w7, w6, [x26]
    cbnz w7, .Linc
    dmb  ish
    mov  x0, x26                           // SYS_FUTEX(&done, FUTEX_WAKE, 1)
    mov  x1, #1
    mov  x2, #1
    mov  x8, #26
    svc  #0

    cmp  w4, #300                          // rendered the final frame?
    b.ge .Lwexit
    b    .Lwloop
.Lwexit:
    mov  x8, #22                           // SYS_THREAD_EXIT (posts completion; never returns)
    svc  #0
1:  b 1b

.section .data,"aw",@progbits
.balign 4
.globl g_phase
g_phase:
    .word 0                                // frame-release word (parent writes frame+1, workers yield-poll)
.globl g_done
g_done:
    .word 0                                // arrival word (workers increment + FUTEX WAKE, parent WAITs for 2)

.balign 4
.globl uvug_msg
uvug_msg:
    .ascii ":: UVUG: frames=300 threads=2 checksum=0x"
.globl g_hex
g_hex:
    .ascii "0000000000000000"              // 16 hex digits, patched in place at runtime
    .ascii " ::\n"
.globl uvug_msgend
uvug_msgend:
"#
);

// A no_std binary must define a panic handler; this program never panics (the body is a single asm stream,
// no Rust control flow), so an empty spin suffices and never runs.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
