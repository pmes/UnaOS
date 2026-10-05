/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 The Architect & Una
 *
 * SELFBUILD4 (B356) — SYSKAT4.LNX: known-answer tests for the process-memory pieces a toolchain pipeline hits: copy-on-write
 * fork (the `sh -c "a | b"` pipeline forks), MAP_SHARED across fork, a LAZY execve image (the ELF mapped through file VMAs,
 * the .data/.bss boundary page zero past the file bytes), and SIGBUS for a file page wholly past EOF.
 * Freestanding (gcc -nostdlib -static -fno-pie -no-pie -mno-red-zone): raw syscalls only, so the KAT measures the shim, not
 * a libc. Checked against the host Linux kernel first (arroyo runs it there before staging).
 *
 * argv: SYSKAT4.LNX <scratch>   — <scratch> is a writable path (created with 100 bytes, mapped, unlinked).
 *       SYSKAT4.LNX exec        — the re-executed image (group exec): checks its own .data and .bss, exits 0 or the id.
 * Runs EVERY group, then prints one line
 *   "syskat4 ok cow=ok cow_kernel=ok cow_prot=ok shared_fork=ok exec_lazy=ok sigbus=ok checks=<n> fail=none"
 * and exits 0, or "syskat4 fail …" with each failed group as fail(<id>), exiting with the first failed id:
 *   cow    1 fork      2 the child sees the parent's .data (3 pages), anon page, brk page and stack value
 *          3 the child's stores to all four read back in the child   4 the child exits 0 (else its own failed id)
 *          5 the parent's four values are unchanged after the child's stores
 *   cow_kernel 10 read() from a pipe INTO a copy-on-write page in the child (a kernel store must break the share)
 *          11 the child exits 0                      12 the parent's page is unchanged
 *   cow_prot 15 the child mprotects a shared page RO then RW and stores (exit 0)   16 the parent's page is unchanged
 *   shared_fork 20 MAP_SHARED|MAP_ANONYMOUS page stored before fork  21 the child's store is seen by the parent
 *   exec   30 fork + execve("/proc/self/exe", "exec") exits 0 — inside the new image: 31 .data pattern on all 3 pages
 *          32 the .bss (whose first page shares the .data tail's page) reads zero   33 the anon/brk of the old image are gone
 *   sigbus 40 scratch file of 100 bytes mapped 2 pages MAP_PRIVATE: byte 50 reads, bytes 100..4095 read zero
 *          41 a child touching the second page (wholly past EOF) is killed by SIGBUS (7)
 */
typedef unsigned long u64;
typedef long i64;
typedef unsigned int u32;

static i64 sc6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f)
{
    i64 r;
    register i64 r10 __asm__("r10") = d;
    register i64 r8 __asm__("r8") = e;
    register i64 r9 __asm__("r9") = f;
    __asm__ volatile ("syscall" : "=a"(r) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9) : "rcx", "r11", "memory");
    return r;
}
#define S1(n, a) sc6(n, (i64)(a), 0, 0, 0, 0, 0)
#define S2(n, a, b) sc6(n, (i64)(a), (i64)(b), 0, 0, 0, 0)
#define S3(n, a, b, c) sc6(n, (i64)(a), (i64)(b), (i64)(c), 0, 0, 0)
#define S4(n, a, b, c, d) sc6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), 0, 0)
#define MMAP(a, l, p, f, fd, o) ((char *)sc6(9, (i64)(a), (i64)(l), p, f, fd, o))
#define BAD(p) ((i64)(p) < 0 && (i64)(p) > -4096)

#define PROT_R 1
#define PROT_W 2
#define MAP_SH 1
#define MAP_PR 2
#define MAP_AN 0x20
#define PG 4096UL

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

/* .data: 3 pages with a stamp at both ends of each page, then a small tail — the image's file pages (lazy on UnaOS). */
#define W8(i) ((i) * 2654435761UL + 0x5a5a)
static u64 gdata[3 * PG / 8] = {
    [0] = W8(0), [511] = W8(511), [512] = W8(512), [1023] = W8(1023), [1024] = W8(1024), [1535] = W8(1535),
};
static volatile u64 gtail[5] = { 11, 22, 33, 44, 55 };
/* keeps the .data end OFF a page boundary, so the .bss starts mid-page (the boundary page is part file, part zero). */
static volatile u64 gpad[37] = { 7, [36] = 9 };
/* .bss right after: its first page shares the page holding the .data tail (zero past the file bytes, never file junk). */
static u64 gbss[3 * PG / 8];

static int checks, first_fail;
static int g_cow, g_kern, g_prot, g_shared, g_exec, g_bus;
#define CHECK(g, id, cond) do { checks++; if (!(cond)) { if (!(g)) (g) = (id); if (!first_fail) first_fail = (id); goto g##_end; } } while (0)

static int data_ok(void)
{
    for (u64 i = 0; i < 3 * PG / 8; i++) {
        u64 want = (i % 512 == 0 || i % 512 == 511) ? W8(i) : 0;
        if (gdata[i] != want) return 0;
    }
    return gtail[0] == 11 && gtail[4] == 55 && gpad[0] == 7 && gpad[36] == 9;
}

static int bss_zero(void)
{
    for (u64 i = 0; i < 3 * PG / 8; i++) if (gbss[i]) return 0;
    return 1;
}

static i64 waitchild(i64 pid)
{
    int st = -1;
    if (S4(61, pid, &st, 0, 0) != pid) return -1;
    return st;
}

static void verdict(const char *name, int g)
{
    out(" ");
    out(name);
    out("=");
    if (!g) { out("ok"); return; }
    out("fail(");
    outn((u64)g);
    out(")");
}

void cmain(u64 *sp)
{
    char **argv = (char **)(sp + 1);
    const char *a1 = argv[1] ? argv[1] : "";

    if (a1[0] == 'e' && a1[1] == 'x' && a1[2] == 'e' && a1[3] == 'c' && a1[4] == 0) {
        /* ---- the re-executed image ---- */
        if (!data_ok()) S1(231, 31);
        if (!bss_zero()) S1(231, 32);
        S1(231, 0);
    }
    const char *scratch = a1;

    /* ---- cow: user stores in the child break the share; the parent keeps its bytes ---- */
    {
        char *an = MMAP(0, PG, PROT_R | PROT_W, MAP_PR | MAP_AN, -1, 0);
        u64 b0 = (u64)S1(12, 0);
        u64 *bk = (u64 *)b0;
        int brk_ok = S1(12, b0 + PG) == (i64)(b0 + PG);
        volatile u64 stackv = 0x1234;
        if (BAD(an) || !brk_ok) { CHECK(g_cow, 1, 0); }
        *(u64 *)an = 0xA11;
        *bk = 0xB22;
        gdata[600] = 0xD33;
        i64 pid = S1(57, 0);
        CHECK(g_cow, 1, pid >= 0);
        if (pid == 0) {
            if (!(*(u64 *)an == 0xA11 && *bk == 0xB22 && gdata[600] == 0xD33 && stackv == 0x1234)) S1(231, 2);
            *(u64 *)an = 0xC11; *bk = 0xC22; gdata[600] = 0xC33; stackv = 0xC44;
            if (!(*(u64 *)an == 0xC11 && *bk == 0xC22 && gdata[600] == 0xC33 && stackv == 0xC44)) S1(231, 3);
            S1(231, 0);
        }
        i64 st = waitchild(pid);
        CHECK(g_cow, (st >> 8) & 0xff ? (int)((st >> 8) & 0xff) : 4, st == 0);
        CHECK(g_cow, 5, *(u64 *)an == 0xA11 && *bk == 0xB22 && gdata[600] == 0xD33 && stackv == 0x1234);
        gdata[600] = 0;
        S1(12, b0);
        S2(11, an, PG);
    }
g_cow_end:

    /* ---- cow_kernel: a kernel copy_out (read from a pipe) into a shared page ---- */
    {
        int pfd[2];
        gdata[700] = 0x7777;
        if (S1(22, pfd) != 0) { CHECK(g_kern, 10, 0); }
        S3(1, pfd[1], "KERNELST", 8);
        i64 pid = S1(57, 0);
        if (pid == 0) {
            if (S3(0, pfd[0], &gdata[700], 8) != 8) S1(231, 10);
            if (((char *)&gdata[700])[0] != 'K') S1(231, 10);
            S1(231, 0);
        }
        CHECK(g_kern, 10, pid > 0);
        i64 st = waitchild(pid);
        CHECK(g_kern, (st >> 8) & 0xff ? (int)((st >> 8) & 0xff) : 11, st == 0);
        CHECK(g_kern, 12, gdata[700] == 0x7777);
        gdata[700] = 0;
        S1(3, pfd[0]);
        S1(3, pfd[1]);
    }
g_kern_end:

    /* ---- cow_prot: mprotect round trip on a shared page in the child ---- */
    {
        char *an = MMAP(0, PG, PROT_R | PROT_W, MAP_PR | MAP_AN, -1, 0);
        if (BAD(an)) { CHECK(g_prot, 15, 0); }
        an[0] = 'P';
        i64 pid = S1(57, 0);
        if (pid == 0) {
            if (S3(10, an, PG, PROT_R) != 0 || an[0] != 'P') S1(231, 15);
            if (S3(10, an, PG, PROT_R | PROT_W) != 0) S1(231, 15);
            an[0] = 'Q';
            S1(231, an[0] == 'Q' ? 0 : 15);
        }
        i64 st = waitchild(pid);
        CHECK(g_prot, 15, pid > 0 && st == 0);
        CHECK(g_prot, 16, an[0] == 'P');
        S2(11, an, PG);
    }
g_prot_end:

    /* ---- shared_fork: MAP_SHARED anonymous memory stays shared across fork ---- */
    {
        char *sh = MMAP(0, PG, PROT_R | PROT_W, MAP_SH | MAP_AN, -1, 0);
        CHECK(g_shared, 20, !BAD(sh));
        sh[0] = 1;
        i64 pid = S1(57, 0);
        if (pid == 0) {
            sh[0] = 2;
            S1(231, 0);
        }
        i64 st = waitchild(pid);
        CHECK(g_shared, 21, pid > 0 && st == 0 && sh[0] == 2);
        S2(11, sh, PG);
    }
g_shared_end:

    /* ---- exec: the image re-executed through /proc/self/exe checks its own .data / .bss ---- */
    {
        gbss[0] = 0xBADBAD; /* dirty our own .bss: the new image must not inherit it */
        i64 pid = S1(57, 0);
        if (pid == 0) {
            char *av[3] = { "SYSKAT4", "exec", 0 };
            char *ev[1] = { 0 };
            S3(59, "/proc/self/exe", av, ev);
            S1(231, 33);
        }
        i64 st = waitchild(pid);
        CHECK(g_exec, (st >> 8) & 0xff ? (int)((st >> 8) & 0xff) : 30, pid > 0 && st == 0);
    }
g_exec_end:
    gbss[0] = 0;

    /* ---- sigbus: a file page wholly past EOF ---- */
    {
        i64 fd = S4(257, -100, scratch, 0102 | 01000, 0644); /* O_RDWR|O_CREAT|O_TRUNC */
        char buf[100];
        for (int i = 0; i < 100; i++) buf[i] = (char)('a' + i % 26);
        CHECK(g_bus, 40, fd >= 0 && S3(1, fd, buf, 100) == 100);
        char *m = MMAP(0, 2 * PG, PROT_R, MAP_PR, fd, 0);
        CHECK(g_bus, 40, !BAD(m) && m[50] == buf[50]);
        int z = 1;
        for (u64 i = 100; i < PG; i++) if (m[i]) z = 0;
        CHECK(g_bus, 40, z);
        i64 pid = S1(57, 0);
        if (pid == 0) {
            volatile char c = m[PG + 8];
            (void)c;
            S1(231, 99);
        }
        i64 st = waitchild(pid);
        CHECK(g_bus, 41, pid > 0 && (st & 0x7f) == 7);
        S2(11, m, 2 * PG);
        S1(3, fd);
        S1(87, scratch);
    }
g_bus_end:

    out(first_fail ? "syskat4 fail" : "syskat4 ok");
    verdict("cow", g_cow);
    verdict("cow_kernel", g_kern);
    verdict("cow_prot", g_prot);
    verdict("shared_fork", g_shared);
    verdict("exec_lazy", g_exec);
    verdict("sigbus", g_bus);
    out(" checks=");
    outn((u64)checks);
    out(" fail=");
    if (first_fail) outn((u64)first_fail); else out("none");
    out("\n");
    S1(231, first_fail);
}

__asm__(".globl _start\n_start:\n  mov %rsp, %rdi\n  and $-16, %rsp\n  call cmain\n  hlt\n");
