#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// K2 (make-enforcement-LIVE): the OWNER EL0 program, carried onto the boot media as `K2OWN.BIN`
// alongside `HELLO.BIN`. It exists to be a SECOND, genuinely-distinct launchable named program — a
// distinct 8.3 filename mints a distinct persistent principal (`prog:K2OWN.BIN`) via the kernel's
// sole mint path (`load_program_into_slot` -> `slot_ppid_stamp`), which is the honest precondition
// for flipping `by_name_spawn_multivalued()` and turning K1's proven-but-gated cross-reboot ACL LIVE.
//
// At EL0 it does a PRIVATE `O_CREAT` of `K2PRIV.BIN` (mode 3 = RW|O_CREAT, O_PUBLIC clear) — so the
// kernel records the caller (this program's principal) as the file's OWNER and WRITE-THROUGH persists
// the owner row to `UNAFS.ATR` — then writes 16 bytes (a grow-from-empty on the first spawn; an
// in-place owner write on a re-spawn after the ACL was rebuilt from disk). The launcher
// (`k2_liveenf_launcher`) spawns this program twice: once to CREATE+OWN+persist, and once AFTER a
// (simulated) reboot to prove the SAME principal is RE-ADMITTED to its own file by name.
//
// Built as a SEPARATE link product exactly like `crates/user-blob/src/main.rs` (the `hello` blob):
// `arroyo kernel8` builds every `[[bin]]` in this crate, objcopy's this one to `target/owner_blob.bin`,
// and copies it onto the FAT media as `K2OWN.BIN`. Fully position-independent (byte-granular `adr` +
// an inline name/pattern, no `.rodata`/GOT), so it runs at whatever EL0 VA the kernel loads it to.
//
// ABI (Linux-aarch64): x8 = syscall number, args x0..x5, return in x0. Syscalls used: SYS_OPEN = 11
// (name_ptr, name_len, mode), SYS_WRITE = 1 (fd, buf, len), SYS_REPORT = 3 (value), SYS_EXIT = 2.

use core::arch::naked_asm;

/// `_start`: private-create + write `K2PRIV.BIN`, report a 2-bit witness (bit0 open admitted, bit1
/// write OK), then `sys_exit(0x82)` (the K2 sentinel the kernel routes to `EL0_K2_DONE`). `#[naked]`
/// so the flat blob is exactly this instruction stream; `_start` is forced first by `.text.entry` +
/// the linker script. All PC-relative — the name (`0:`) and 16-byte pattern (`2:`) live inline.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    naked_asm!(
        "mov  x23, xzr",              // witness bitmask = 0
        // (0) SYS_OPEN K2PRIV.BIN, mode 3 = RW | O_CREAT, O_PUBLIC CLEAR -> PRIVATE (this program owns it)
        "mov  x8, #{sys_open}",       // SYS_OPEN
        "adr  x0, 0f",                // name = "K2PRIV.BIN" (PC-relative EL0 VA)
        "mov  x1, #(1f - 0f)",        // name length
        "mov  x2, #{o_create_rw}",    // O_CREAT | RW, private
        "svc  #0",
        "mov  x19, x0",               // x19 = handle (>=0) or -errno
        "tbnz x19, #63, 8f",          // a negative return (denied/error) -> report the partial witness
        "add  x23, x23, #1",          // bit0: open admitted (created-and-owned, or re-admitted by name)
        // (1) SYS_WRITE 16 bytes -> grow-from-empty on the create; in-place owner write on a re-open
        "adr  x1, 2f",                // buf = the 16-byte pattern
        "mov  x8, #{sys_write}",      // SYS_WRITE
        "mov  x0, x19",               // fd = handle
        "mov  x2, #16",               // len = 16
        "svc  #0",
        "cmp  x0, #16",
        "b.ne 8f",
        "add  x23, x23, #2",          // bit1: owner write OK
        "8:",
        "mov  x0, x23",               // SYS_REPORT(final witness) -> the launcher reads K2_OWN_WITNESS
        "mov  x8, #{sys_report}",
        "svc  #0",
        "mov  x8, #{sys_exit}",       // SYS_EXIT(EXIT_STATUS_K2) -> EL0_K2_DONE
        "movz x0, #{k2_status}",
        "svc  #0",
        "9: b 9b",                    // sys_exit never returns; belt-and-braces guard
        ".balign 4",
        "0: .ascii \"K2PRIV.BIN\"",
        "1:",
        ".balign 8",
        "2: .ascii \"K2-OWNED-SECRET!\"", // exactly 16 bytes — the owner's pattern
        // ABIFREEZE: `const` operands out of `una_abi` — the one declaration of the table, shared
        // with the kernel arm that dispatches these numbers and matches this exit status.
        sys_open = const una_abi::SYS_OPEN,
        sys_write = const una_abi::SYS_WRITE,
        sys_report = const una_abi::SYS_REPORT,
        sys_exit = const una_abi::SYS_EXIT,
        o_create_rw = const (una_abi::O_CREAT | una_abi::O_RW),
        k2_status = const una_abi::EXIT_STATUS_K2,
    );
}

/// A no_std binary must define a panic handler; this routine never panics (a single naked asm stream),
/// so an empty spin suffices and never runs.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
