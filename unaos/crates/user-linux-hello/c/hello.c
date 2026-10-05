/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 The Architect & Una
 *
 * SELFBUILD1 fixture: the one-file C program UnaOS compiles ON ITSELF with tcc under the Linux ABI shim,
 * `linux /apps/TCC.LNX -nostdlib -static -o <home>/hello.lnx /apps/HELLO.C`, then runs as `linux <home>/hello.lnx`.
 * Freestanding on purpose: no libc is staged on the volume (no crt1.o/libc.a/headers), so it speaks raw
 * Linux syscalls through tcc's inline asm and brings its own _start.
 */
static long sys3(long n, long a, long b, long c)
{
    long r;
    __asm__ volatile ("syscall" : "=a"(r) : "a"(n), "D"(a), "S"(b), "d"(c) : "rcx", "r11", "memory");
    return r;
}

static const char msg[] = "hello from tcc on unaos\n";

void _start(void)
{
    sys3(1, 1, (long)msg, sizeof msg - 1); /* write(1, msg, n) */
    sys3(231, 0, 0, 0);                    /* exit_group(0) */
    for (;;) {}
}
