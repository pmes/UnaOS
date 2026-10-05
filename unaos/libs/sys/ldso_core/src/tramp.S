// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFBUILD6 (B360): the ring-3 half of the loader — ONE code page (R+X) followed by ONE data page (RW) the loader
// fills. Position-independent: every data reference is RIP-relative to .Lb + 4096. `tools/gen_tramp.py` assembles this
// file and writes `src/tramp_bytes.rs` (the bytes + entry offsets); `tests/tramp.rs` re-assembles and compares.
//
// Data page slots (offset from the data page): see `DATA_*` in lib.rs. The kernel's gates are private syscalls
// 7504..7507 (dlopen, dlsym, dlclose, dladdr); the host runner points the G_* slots at its own functions instead.
        .intel_syntax noprefix
        .text
        .globl tramp_base
tramp_base:
.Lb:
        .set D, 4096
        .set REAL_MAIN, D + 0x00
        .set LIBC_START, D + 0x08
        .set INIT_LIST, D + 0x10
        .set FINI_LIST, D + 0x18
        .set ATEXIT, D + 0x20
        .set G_OPEN, D + 0x28
        .set G_SYM, D + 0x30
        .set G_CLOSE, D + 0x38
        .set ERRSTR, D + 0x40
        .set OBJ_TAB, D + 0x48
        .set OBJ_N, D + 0x50
        .set PENDING, D + 0x58
        .set G_ADDR, D + 0x60

// __libc_start_main(main, argc, argv, init, fini, rtld_fini): keep main, hand libc our hook instead.
        .globl t_start_main
t_start_main:
        mov     qword ptr [rip + .Lb + REAL_MAIN], rdi
        lea     rdi, [rip + .Lmain_hook]
        jmp     qword ptr [rip + .Lb + LIBC_START]

// main_hook(argc, argv, envp): libc and TLS are up — run every other object's constructors (deps first), register the
// fini runner with atexit, then the real main.
        .globl t_main_hook
t_main_hook:
.Lmain_hook:
        push    rbx
        push    r12
        push    r13
        push    r14
        sub     rsp, 8
        mov     r12d, edi
        mov     r13, rsi
        mov     r14, rdx
        mov     rbx, qword ptr [rip + .Lb + INIT_LIST]
        test    rbx, rbx
        jz      2f
1:      mov     rax, qword ptr [rbx]
        test    rax, rax
        jz      2f
        mov     edi, r12d
        mov     rsi, r13
        mov     rdx, r14
        call    rax
        add     rbx, 8
        jmp     1b
2:      mov     rax, qword ptr [rip + .Lb + ATEXIT]
        test    rax, rax
        jz      3f
        cmp     qword ptr [rip + .Lb + FINI_LIST], 0
        je      3f
        lea     rdi, [rip + .Lfini_hook]
        call    rax
3:      mov     edi, r12d
        mov     rsi, r13
        mov     rdx, r14
        add     rsp, 8
        pop     r14
        pop     r13
        pop     r12
        pop     rbx
        jmp     qword ptr [rip + .Lb + REAL_MAIN]

        .globl t_fini_hook
t_fini_hook:
.Lfini_hook:
        push    rbx
        mov     rbx, qword ptr [rip + .Lb + FINI_LIST]
1:      mov     rax, qword ptr [rbx]
        test    rax, rax
        jz      2f
        call    rax
        add     rbx, 8
        jmp     1b
2:      pop     rbx
        ret

// Run (and clear) the constructor list the loader left for the object a dlopen just loaded.
t_run_pending:
        push    rbx
        mov     rbx, qword ptr [rip + .Lb + PENDING]
        mov     qword ptr [rip + .Lb + PENDING], 0
        test    rbx, rbx
        jz      2f
1:      mov     rax, qword ptr [rbx]
        test    rax, rax
        jz      2f
        call    rax
        add     rbx, 8
        jmp     1b
2:      pop     rbx
        ret

        .globl t_dlopen
t_dlopen:
        push    rbx
        call    qword ptr [rip + .Lb + G_OPEN]
        mov     rbx, rax
        call    t_run_pending
        mov     rax, rbx
        pop     rbx
        ret

        .globl t_dlsym
t_dlsym:
        jmp     qword ptr [rip + .Lb + G_SYM]

        .globl t_dlclose
t_dlclose:
        jmp     qword ptr [rip + .Lb + G_CLOSE]

        .globl t_dladdr
t_dladdr:
        jmp     qword ptr [rip + .Lb + G_ADDR]

        .globl t_dlerror
t_dlerror:
        mov     rax, qword ptr [rip + .Lb + ERRSTR]
        mov     qword ptr [rip + .Lb + ERRSTR], 0
        ret

// __tls_get_addr(&{module, offset}): every module is in the static block; the loader wrote the module's TP offset.
        .globl t_tls_get_addr
t_tls_get_addr:
        mov     rax, qword ptr fs:[0]
        add     rax, qword ptr [rdi]
        add     rax, qword ptr [rdi + 8]
        ret

// dl_iterate_phdr(cb, data): walk the loader's table of 64-byte dl_phdr_info records.
        .globl t_dl_iterate_phdr
t_dl_iterate_phdr:
        push    rbx
        push    r12
        push    r13
        push    r14
        sub     rsp, 8
        mov     r12, rdi
        mov     r13, rsi
        xor     ebx, ebx
1:      cmp     rbx, qword ptr [rip + .Lb + OBJ_N]
        jae     3f
        mov     rdi, rbx
        shl     rdi, 6
        add     rdi, qword ptr [rip + .Lb + OBJ_TAB]
        mov     esi, 64
        mov     rdx, r13
        call    r12
        test    eax, eax
        jnz     4f
        inc     rbx
        jmp     1b
3:      xor     eax, eax
4:      add     rsp, 8
        pop     r14
        pop     r13
        pop     r12
        pop     rbx
        ret

// The kernel's gates (UnaOS): G_OPEN/G_SYM/G_CLOSE point here; the host runner points them at its own functions.
        .globl t_gate_open
t_gate_open:
        mov     eax, 7504
        syscall
        ret
        .globl t_gate_sym
t_gate_sym:
        mov     eax, 7505
        syscall
        ret
        .globl t_gate_close
t_gate_close:
        mov     eax, 7506
        syscall
        ret
        .globl t_gate_addr
t_gate_addr:
        mov     eax, 7507
        syscall
        ret
        .globl tramp_end
tramp_end:
