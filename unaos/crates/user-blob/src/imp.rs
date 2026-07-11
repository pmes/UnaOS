#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// K2 (make-enforcement-LIVE): the IMPOSTOR EL0 program, carried onto the boot media as `K2IMP.BIN`.
// A THIRD distinct launchable named program — its distinct 8.3 name mints a distinct principal
// (`prog:K2IMP.BIN`) that is NEITHER the owner nor a grantee of `K2PRIV.BIN`. It exists to prove the
// DENIAL half of LIVE cross-reboot enforcement with a REAL program: after the owner's ACL is rebuilt
// from `UNAFS.ATR`, this program's attempt to open the owner's private file must be REFUSED by name.
//
// At EL0 it does a plain `SYS_OPEN` of `K2PRIV.BIN` RO (mode 0 — no O_CREAT, so it opens the existing
// owned file, never creates) and asserts the kernel returns `-EACCES` (-13): the by-name ACL denies a
// principal that is neither the owner (`prog:K2OWN.BIN`) nor a grantee. It reports a 1-bit witness and
// exits with the K2 sentinel. The launcher (`k2_liveenf_launcher`) spawns it AFTER the owner has
// created+persisted `K2PRIV.BIN` and the row has been rebuilt with the no-live-owner sentinel.
//
// Built + carried exactly like `owner.rs` / `main.rs`. Position-independent; ABI x8=nr, args x0..x5.
// Syscalls: SYS_OPEN = 11, SYS_REPORT = 3, SYS_EXIT = 2.

use core::arch::naked_asm;

/// `_start`: open `K2PRIV.BIN` RO, expect `-EACCES`, report a 1-bit witness (bit0 = denied as
/// expected), then `sys_exit(0x82)`. `#[naked]`; PC-relative name inline at `0:`.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    naked_asm!(
        "mov  x23, xzr",              // witness bitmask = 0
        // (0) SYS_OPEN K2PRIV.BIN RO (mode 0, no O_CREAT) -> the owned file exists; a non-owner is denied
        "mov  x8, #11",               // SYS_OPEN
        "adr  x0, 0f",                // name = "K2PRIV.BIN"
        "mov  x1, #(1f - 0f)",        // name length
        "mov  x2, #0",                // RO, no O_CREAT
        "svc  #0",
        "cmn  x0, #13",               // x0 == -13 (-EACCES)?  the impostor MUST be denied by name
        "b.ne 8f",
        "add  x23, x23, #1",          // bit0: non-owner open denied (-EACCES) — the deny half is REAL
        "8:",
        "mov  x0, x23",               // SYS_REPORT(final witness) -> the launcher reads K2_IMP_WITNESS
        "mov  x8, #3",
        "svc  #0",
        "mov  x8, #2",                // SYS_EXIT(K2_EXIT_STATUS) -> EL0_K2_DONE
        "movz x0, #0x82",
        "svc  #0",
        "9: b 9b",                    // sys_exit never returns; belt-and-braces guard
        ".balign 4",
        "0: .ascii \"K2PRIV.BIN\"",
        "1:",
    );
}

/// A no_std binary must define a panic handler; this routine never panics.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
