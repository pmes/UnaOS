/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 The Architect & Una
 *
 * SELFBUILD5 (B357) — SYSKAT5.LNX: known-answer tests for what a linker and a growing Rust program hit after SELFBUILD4:
 * mremap (musl's realloc past ~128 KiB, lld's output buffer), the alternate signal stack honoured at delivery (Rust std's
 * stack-overflow handler is SA_ONSTACK), getcpu, madvise(MADV_HUGEPAGE), sched_yield.
 * Freestanding (gcc -nostdlib -static -fno-pie -no-pie -mno-red-zone): raw syscalls only, so the KAT measures the shim, not
 * a libc. Checked against the host Linux kernel first (arroyo runs it there before staging). No arguments.
 * Runs EVERY group, then prints one line
 *   "syskat5 ok grow=ok move=ok fixed=ok dontunmap=ok errors=ok altstack=ok small=ok checks=<n> fail=none"
 * and exits 0, or "syskat5 fail …" with each failed group as fail(<id>), exiting with the first failed id:
 *   grow   1 mremap(a, 2 pages, 4 pages, 0) answers a (the next 2 pages were unmapped: grow IN PLACE)
 *          2 the first 2 pages keep their bytes   3 the 2 new pages read zero and take a store
 *          4 shrink to 1 page answers a, and the dropped page is unmapped (mincore -ENOMEM)
 *   move   10 without MREMAP_MAYMOVE a grow blocked by the next mapping is -ENOMEM
 *          11 with MAYMOVE it answers a NEW address   12 the moved pages keep their bytes   13 the new tail reads zero
 *          14 the old range is unmapped (mincore -ENOMEM)   15 the blocking neighbour is untouched
 *          16 realloc's shape: 1 MiB stamped every 4 KiB grown to 4 MiB (MAYMOVE), every stamp intact
 *   fixed  20 MAYMOVE|FIXED onto a reserved range answers that address   21 the bytes are there   22 the source is unmapped
 *   dontunmap 30 MAYMOVE|DONTUNMAP answers a new address holding the bytes   31 the old range stays mapped and reads zero
 *          32 the old range takes a store
 *   errors 40 unaligned old -EINVAL   41 DONTUNMAP with new_len != old_len -EINVAL   42 an unmapped old range -EFAULT
 *          43 FIXED without MAYMOVE -EINVAL
 *   altstack 50 sigaltstack install (64 KiB) answers 0 and reads back   51 rt_sigaction(SIGUSR1, SA_ONSTACK|SA_SIGINFO)
 *          52 kill(self) ran the handler once   53 the handler's frame was ON the alternate stack
 *          54 sigaltstack queried inside the handler says SS_ONSTACK   55 changing it there is -EPERM
 *          56 the frame's uc_stack.ss_sp is the alternate stack   57 after return the query says not on stack (flags 0)
 *          58 a handler WITHOUT SA_ONSTACK runs on the normal stack   59 SS_DISABLE then reads back SS_DISABLE
 *   small  60 getcpu answers 0   61 madvise(MADV_HUGEPAGE) answers 0   62 sched_yield answers 0
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
#define S0(n) sc6(n, 0, 0, 0, 0, 0, 0)
#define S1(n, a) sc6(n, (i64)(a), 0, 0, 0, 0, 0)
#define S2(n, a, b) sc6(n, (i64)(a), (i64)(b), 0, 0, 0, 0)
#define S3(n, a, b, c) sc6(n, (i64)(a), (i64)(b), (i64)(c), 0, 0, 0)
#define S4(n, a, b, c, d) sc6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), 0, 0)
#define MMAP(a, l, p, f, fd, o) ((char *)sc6(9, (i64)(a), (i64)(l), p, f, fd, o))
#define MREMAP(o, ol, nl, fl, na) ((char *)sc6(25, (i64)(o), (i64)(ol), (i64)(nl), (i64)(fl), (i64)(na), 0))
#define BAD(p) ((i64)(p) < 0 && (i64)(p) > -4096)

#define PROT_R 1
#define PROT_W 2
#define MAP_PR 2
#define MAP_AN 0x20
#define PG 4096UL
#define MAYMOVE 1
#define FIXED 2
#define DONTUNMAP 4
#define SS_ONSTACK 1
#define SS_DISABLE 2

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

static int checks, first_fail;
static int g_grow, g_move, g_fixed, g_dont, g_err, g_alt, g_small;
#define CHECK(g, id, cond) do { checks++; if (!(cond)) { if (!(g)) (g) = (id); if (!first_fail) first_fail = (id); goto g##_end; } } while (0)

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

static char *anon(u64 len) { return MMAP(0, len, PROT_R | PROT_W, MAP_PR | MAP_AN, -1, 0); }
static void stamp(char *p, u64 pages, u64 seed) { for (u64 i = 0; i < pages; i++) *(u64 *)(p + i * PG) = seed * 1000003UL + i; }
static int stamped(char *p, u64 pages, u64 seed)
{
    for (u64 i = 0; i < pages; i++) if (*(u64 *)(p + i * PG) != seed * 1000003UL + i) return 0;
    return 1;
}
static int zero(char *p, u64 len) { for (u64 i = 0; i < len; i += 8) if (*(u64 *)(p + i)) return 0; return 1; }
static int unmapped(char *p, u64 len) { unsigned char v[64]; return S3(27, p, len, v) == -12; }

/* ---- the signal handler (SA_SIGINFO; returns through our restorer) ---- */
struct stk { u64 sp; u32 flags; u32 pad; u64 size; };
static char *alt;
static volatile u64 h_runs, h_local, h_ucsp;
static volatile i64 h_q, h_set;
static volatile u32 h_flags;
static void handler(int sig, void *si, void *uc)
{
    volatile int local = sig;
    struct stk o = { 1, 99, 0, 1 }, n = { 0, 0, 0, 0x10000 };
    (void)si;
    h_runs++;
    h_local = (u64)&local;
    h_q = S2(131, 0, &o);
    h_flags = o.flags;
    n.sp = (u64)alt;
    h_set = S2(131, &n, 0);
    h_ucsp = *(u64 *)((char *)uc + 16); /* ucontext: uc_flags, uc_link, then uc_stack.ss_sp */
}
void restorer(void);
__asm__(".globl restorer\nrestorer:\n  mov $15, %eax\n  syscall\n  hlt\n");

void cmain(u64 *sp)
{
    (void)sp;
    /* ---- grow in place, shrink ---- */
    {
        char *a = anon(4 * PG);
        CHECK(g_grow, 1, !BAD(a) && S2(11, a + 2 * PG, 2 * PG) == 0);
        stamp(a, 2, 1);
        CHECK(g_grow, 1, MREMAP(a, 2 * PG, 4 * PG, 0, 0) == a);
        CHECK(g_grow, 2, stamped(a, 2, 1));
        CHECK(g_grow, 3, zero(a + 2 * PG, 2 * PG));
        *(u64 *)(a + 3 * PG) = 77;
        CHECK(g_grow, 3, *(u64 *)(a + 3 * PG) == 77);
        CHECK(g_grow, 4, MREMAP(a, 4 * PG, PG, 0, 0) == a && unmapped(a + PG, PG) && *(u64 *)a == 1000003UL);
        S2(11, a, PG);
    }
g_grow_end:

    /* ---- move ---- */
    {
        char *c = anon(3 * PG);
        CHECK(g_move, 11, !BAD(c));
        stamp(c, 2, 2);
        c[2 * PG] = 'N';
        CHECK(g_move, 11, S3(10, c + 2 * PG, PG, PROT_R) == 0); /* a separate mapping right after the 2 pages */
        CHECK(g_move, 10, MREMAP(c, 2 * PG, 8 * PG, 0, 0) == (char *)-12);
        char *m = MREMAP(c, 2 * PG, 8 * PG, MAYMOVE, 0);
        CHECK(g_move, 11, !BAD(m) && m != c);
        CHECK(g_move, 12, stamped(m, 2, 2));
        CHECK(g_move, 13, zero(m + 2 * PG, 6 * PG));
        CHECK(g_move, 14, unmapped(c, 2 * PG));
        CHECK(g_move, 15, c[2 * PG] == 'N');
        S2(11, m, 8 * PG);
        S2(11, c + 2 * PG, PG);
        char *big = anon(256 * PG);
        CHECK(g_move, 16, !BAD(big));
        stamp(big, 256, 3);
        char *nb = MREMAP(big, 256 * PG, 1024 * PG, MAYMOVE, 0);
        CHECK(g_move, 16, !BAD(nb) && stamped(nb, 256, 3) && zero(nb + 256 * PG, 8 * PG));
        nb[1024 * PG - 1] = 1;
        S2(11, nb, 1024 * PG);
    }
g_move_end:

    /* ---- fixed ---- */
    {
        char *t = anon(2 * PG), *s = anon(2 * PG);
        CHECK(g_fixed, 20, !BAD(t) && !BAD(s));
        stamp(s, 2, 4);
        CHECK(g_fixed, 20, MREMAP(s, 2 * PG, 2 * PG, MAYMOVE | FIXED, t) == t);
        CHECK(g_fixed, 21, stamped(t, 2, 4));
        CHECK(g_fixed, 22, unmapped(s, 2 * PG));
        S2(11, t, 2 * PG);
    }
g_fixed_end:

    /* ---- dontunmap ---- */
    {
        char *s = anon(2 * PG);
        CHECK(g_dont, 30, !BAD(s));
        stamp(s, 2, 5);
        char *d = MREMAP(s, 2 * PG, 2 * PG, MAYMOVE | DONTUNMAP, 0);
        CHECK(g_dont, 30, !BAD(d) && d != s && stamped(d, 2, 5));
        CHECK(g_dont, 31, !unmapped(s, 2 * PG) && zero(s, 2 * PG));
        s[8] = 'x';
        CHECK(g_dont, 32, s[8] == 'x' && stamped(d, 2, 5));
        S2(11, s, 2 * PG);
        S2(11, d, 2 * PG);
    }
g_dont_end:

    /* ---- errors ---- */
    {
        char *e = anon(2 * PG);
        CHECK(g_err, 40, MREMAP(e + 1, PG, 2 * PG, MAYMOVE, 0) == (char *)-22);
        CHECK(g_err, 41, MREMAP(e, PG, 2 * PG, MAYMOVE | DONTUNMAP, 0) == (char *)-22);
        S2(11, e, 2 * PG);
        CHECK(g_err, 42, MREMAP(e, PG, 2 * PG, MAYMOVE, 0) == (char *)-14);
        CHECK(g_err, 43, MREMAP(e, PG, PG, FIXED, e + 64 * PG) == (char *)-22);
    }
g_err_end:

    /* ---- the alternate signal stack ---- */
    {
        alt = anon(16 * PG);
        struct stk ss = { (u64)alt, 0, 0, 16 * PG }, q = { 0, 99, 0, 0 };
        CHECK(g_alt, 50, !BAD(alt) && S2(131, &ss, 0) == 0 && S2(131, 0, &q) == 0 && q.sp == (u64)alt && q.size == 16 * PG && q.flags == 0);
        u64 act[4] = { (u64)handler, 0x08000000 /*SA_ONSTACK*/ | 0x04000000 /*SA_RESTORER*/ | 4 /*SA_SIGINFO*/, (u64)restorer, 0 };
        CHECK(g_alt, 51, S4(13, 10, act, 0, 8) == 0);
        i64 me = S0(39);
        S2(62, me, 10);
        CHECK(g_alt, 52, h_runs == 1);
        CHECK(g_alt, 53, h_local >= (u64)alt && h_local < (u64)alt + 16 * PG);
        CHECK(g_alt, 54, h_q == 0 && (h_flags & SS_ONSTACK));
        CHECK(g_alt, 55, h_set == -1);
        CHECK(g_alt, 56, h_ucsp == (u64)alt);
        q.flags = 99;
        CHECK(g_alt, 57, S2(131, 0, &q) == 0 && q.flags == 0 && q.sp == (u64)alt);
        act[1] = 0x04000000 | 4; /* no SA_ONSTACK */
        CHECK(g_alt, 58, S4(13, 10, act, 0, 8) == 0);
        S2(62, me, 10);
        CHECK(g_alt, 58, h_runs == 2 && !(h_local >= (u64)alt && h_local < (u64)alt + 16 * PG) && !(h_flags & SS_ONSTACK));
        struct stk dis = { 0, SS_DISABLE, 0, 0 };
        q.flags = 0;
        CHECK(g_alt, 59, S2(131, &dis, 0) == 0 && S2(131, 0, &q) == 0 && q.flags == SS_DISABLE);
        u64 dfl[4] = { 0, 0, 0, 0 };
        S4(13, 10, dfl, 0, 8);
    }
g_alt_end:

    /* ---- the small answers ---- */
    {
        u32 cpu = 77, node = 77;
        CHECK(g_small, 60, S3(309, &cpu, &node, 0) == 0 && cpu < 4096 && node < 64);
        char *h = anon(512 * PG);
        CHECK(g_small, 61, !BAD(h) && S3(28, h, 512 * PG, 14 /*MADV_HUGEPAGE*/) == 0);
        S2(11, h, 512 * PG);
        CHECK(g_small, 62, S0(24) == 0);
    }
g_small_end:

    out(first_fail ? "syskat5 fail" : "syskat5 ok");
    verdict("grow", g_grow);
    verdict("move", g_move);
    verdict("fixed", g_fixed);
    verdict("dontunmap", g_dont);
    verdict("errors", g_err);
    verdict("altstack", g_alt);
    verdict("small", g_small);
    out(" checks=");
    outn((u64)checks);
    out(" fail=");
    if (first_fail) outn((u64)first_fail); else out("none");
    out("\n");
    S1(231, first_fail);
}

__asm__(".globl _start\n_start:\n  mov %rsp, %rdi\n  and $-16, %rsp\n  call cmain\n  hlt\n");
