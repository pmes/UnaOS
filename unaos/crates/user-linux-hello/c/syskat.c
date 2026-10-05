/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 The Architect & Una
 *
 * SELFBUILD1 M2 — SYSKAT.LNX: known-answer tests for the Linux syscalls a static toolchain (tcc on glibc-static)
 * and a static busybox reach, run as `tests linuxabi` (second witness). Freestanding (gcc -nostdlib -static
 * -fno-pie -no-pie): raw syscalls only, so the KAT measures the shim, not a libc. Checked against the host Linux
 * kernel first (arroyo runs it there before staging). Prints "syskat ok checks=<n>" and exits 0, or prints
 * "syskat fail <id>" and exits with the id of the first check that failed:
 *   1 mmap anon RW     2 mmap contents zero / writable   3 mprotect RO   4 mprotect unmapped -> ENOMEM
 *   5 munmap + MAP_FIXED remap is zero                   6 mmap W+X refused or honoured (never kills)
 *   7 brk(0) / growth  8 brk shrink+regrow zero          9 openat own image  10 fstat size  11 read ELF magic
 *  12 newfstatat dir   13 readlinkat /proc/self/exe      14 getdents64 "/"   15 pipe2+write  16 dup3+read
 *  17 dup3(fd,fd) EINVAL  18 rt_sigaction  19 rt_sigprocmask  20 sigaltstack  21 clock_gettime
 *  22 nanosleep delta  23 getrandom  24 uname  25 set_tid_address  26 set_robust_list  27 prlimit64
 *  28 rseq answers (never kills)  29 lseek  30 close
 */
typedef unsigned long u64;
typedef long i64;

static i64 sc6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f)
{
    i64 r;
    register i64 r10 __asm__("r10") = d;
    register i64 r8 __asm__("r8") = e;
    register i64 r9 __asm__("r9") = f;
    __asm__ volatile ("syscall" : "=a"(r) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9) : "rcx", "r11", "memory");
    return r;
}
#define S0(n) sc6(n, 0, 0, 0, 0, 0, 0)
#define S1(n, a) sc6(n, (i64)(a), 0, 0, 0, 0, 0)
#define S2(n, a, b) sc6(n, (i64)(a), (i64)(b), 0, 0, 0, 0)
#define S3(n, a, b, c) sc6(n, (i64)(a), (i64)(b), (i64)(c), 0, 0, 0)
#define S4(n, a, b, c, d) sc6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), 0, 0)

static u64 slen(const char *s) { u64 n = 0; while (s[n]) n++; return n; }
static void out(const char *s) { S3(1, 1, s, slen(s)); }
static void outn(u64 v)
{
    char b[24];
    int i = 23;
    b[i] = 0;
    do { b[--i] = '0' + v % 10; v /= 10; } while (v);
    out(b + i);
}
static void fail(int id)
{
    out("syskat fail ");
    outn(id);
    out("\n");
    S1(231, id);
}
#define CHECK(id, cond) do { if (!(cond)) fail(id); checks++; } while (0)

static int lower(int c) { return c >= 'A' && c <= 'Z' ? c + 32 : c; }

static char buf[4096];
static char pathbuf[512];

void cmain(u64 *sp)
{
    int checks = 0;
    char *argv0 = (char *)sp[1];
    /* mmap / mprotect / munmap on real pages */
    char *m = (char *)sc6(9, 0, 3 * 4096, 3 /*RW*/, 0x22 /*PRIVATE|ANON*/, -1, 0);
    CHECK(1, (i64)m > 0 && ((u64)m & 4095) == 0);
    int zero = 1;
    for (int i = 0; i < 3 * 4096; i++) if (m[i]) zero = 0;
    m[0] = 'a'; m[3 * 4096 - 1] = 'z';
    CHECK(2, zero && m[0] == 'a' && m[3 * 4096 - 1] == 'z');
    CHECK(3, S3(10, m, 4096, 1 /*PROT_READ*/) == 0 && m[0] == 'a');
    CHECK(4, S3(10, 0x10000000000UL - 0x10000000UL, 4096, 1) == -12);
    CHECK(5, S2(11, m + 4096, 4096) == 0
             && sc6(9, (i64)(m + 4096), 4096, 3, 0x32 /*PRIVATE|ANON|FIXED*/, -1, 0) == (i64)(m + 4096)
             && m[4096] == 0 && m[3 * 4096 - 1] == 'z');
    i64 wx = sc6(9, 0, 4096, 7, 0x22, -1, 0); /* UnaOS refuses W+X (-EACCES); Linux grants it */
    CHECK(6, wx == -13 || wx > 0);
    if (wx > 0) S2(11, wx, 4096);
    S2(11, m, 3 * 4096);
    /* brk growth */
    i64 b0 = S1(12, 0);
    i64 b1 = S1(12, b0 + 65536);
    CHECK(7, b0 > 0 && b1 == b0 + 65536);
    ((char *)b0)[65535] = 7;
    S1(12, b0 + 4096);
    S1(12, b0 + 65536);
    CHECK(8, ((char *)b0)[65535] == 0);
    /* files over the VFS: the program's own image */
    i64 fd = S4(257, -100 /*AT_FDCWD*/, argv0, 0 /*O_RDONLY*/, 0);
    CHECK(9, fd >= 3);
    u64 st[18];
    CHECK(10, S2(5, fd, st) == 0 && st[6] > 1024);
    CHECK(11, S3(0, fd, buf, 4) == 4 && buf[0] == 0x7f && buf[1] == 'E' && buf[2] == 'L' && buf[3] == 'F');
    CHECK(29, S3(8, fd, 0, 0) == 0 && S3(0, fd, buf, 1) == 1 && buf[0] == 0x7f);
    CHECK(30, S1(3, fd) == 0 && S1(3, fd) == -9);
    CHECK(12, S4(262, -100, "/", st, 0) == 0 && (st[3] & 0170000) == 0040000);
    i64 n = S4(267, -100, "/proc/self/exe", pathbuf, sizeof pathbuf - 1);
    CHECK(13, n > 4 && lower(pathbuf[n - 1]) == 'x' && lower(pathbuf[n - 2]) == 'n' && lower(pathbuf[n - 3]) == 'l');
    i64 dfd = S4(257, -100, "/", 0200000 /*O_DIRECTORY*/, 0);
    i64 dn = S3(217, dfd, buf, sizeof buf);
    int ents = 0;
    for (i64 off = 0; dn > 0 && off < dn; off += *(unsigned short *)(buf + off + 16)) ents++;
    S1(3, dfd);
    CHECK(14, dfd >= 0 && ents >= 3);
    /* pipe2 / dup3 */
    int pfd[2];
    CHECK(15, S2(293, pfd, 02000000 /*O_CLOEXEC*/) == 0 && S3(1, pfd[1], "kat!", 4) == 4);
    CHECK(16, S3(292, pfd[0], 10, 0) == 10 && S3(0, 10, buf, 4) == 4 && buf[0] == 'k' && buf[3] == '!');
    CHECK(17, S3(292, 10, 10, 0) == -22);
    S1(3, 10); S1(3, pfd[0]); S1(3, pfd[1]);
    /* signals: accepted, never delivered, never kill */
    u64 act[4] = { 1 /*SIG_IGN*/, 0, 0, 0 }, old[4] = { 9, 9, 9, 9 };
    CHECK(18, S4(13, 2 /*SIGINT*/, act, old, 8) == 0 && old[0] == 0);
    u64 set = 1UL << 1, oset = 99;
    CHECK(19, S4(14, 0 /*SIG_BLOCK*/, &set, &oset, 8) == 0 && oset != 99);
    S4(14, 1 /*SIG_UNBLOCK*/, &set, 0, 8);
    u64 ss[3] = { 9, 9, 9 };
    CHECK(20, S2(131, 0, ss) == 0 && (ss[1] & 0xffffffff) == 2 /*SS_DISABLE*/);
    /* time */
    i64 t0[2], t1[2];
    CHECK(21, S2(228, 1 /*CLOCK_MONOTONIC*/, t0) == 0 && t0[1] >= 0 && t0[1] < 1000000000);
    i64 req[2] = { 0, 30000000 };
    S2(35, req, 0);
    S2(228, 1, t1);
    i64 dms = (t1[0] - t0[0]) * 1000 + (t1[1] - t0[1]) / 1000000;
    CHECK(22, dms >= 20 && dms < 5000);
    /* getrandom / uname / start-up no-ops */
    unsigned char r[16] = { 0 };
    int nz = 0;
    CHECK(23, S3(318, r, 16, 1 /*GRND_NONBLOCK*/) == 16);
    for (int i = 0; i < 16; i++) if (r[i]) nz = 1;
    if (!nz) fail(23);
    static char u[390];
    CHECK(24, S1(63, u) == 0 && u[0] == 'L' && u[4] == 'x' && u[4 * 65] == 'x' && u[4 * 65 + 5] == '4');
    static int tidword;
    CHECK(25, S1(218, &tidword) > 0);
    static u64 robust[3];
    CHECK(26, S2(273, robust, 24) == 0);
    u64 rl[2] = { 0, 0 };
    CHECK(27, S4(302, 0, 7 /*RLIMIT_NOFILE*/, 0, rl) == 0 && rl[0] > 0);
    CHECK(28, S4(334, 0, 0, 0, 0) < 0);
    out("syskat ok checks=");
    outn(checks);
    out("\n");
    S1(231, 0);
}

__asm__(".globl _start\n_start:\n  mov %rsp, %rdi\n  and $-16, %rsp\n  call cmain\n  hlt\n");
