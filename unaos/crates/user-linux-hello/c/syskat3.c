/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 The Architect & Una
 *
 * SELFBUILD3 (B353) — SYSKAT3.LNX: known-answer tests for LAZY memory and file mappings, the shape every real toolchain
 * (rustc, cargo, ld.lld, a libc's malloc) uses: mmap of files MAP_PRIVATE / MAP_SHARED, big anonymous reservations touched
 * sparsely, PROT_NONE reservations opened in part, msync/munmap write-back, madvise(DONTNEED), mincore, brk past 64 MiB.
 * Freestanding (gcc -nostdlib -static -fno-pie -no-pie -mno-red-zone): raw syscalls only, so the KAT measures the shim, not
 * a libc. Checked against the host Linux kernel first (arroyo runs it there before staging).
 *
 * argv: SYSKAT3.LNX <file> <size> <scratch>   — <file> is mapped and must be <size> bytes (UnaOS: /apps/HELLO.C and the
 * size its VFS reports); <scratch> is a writable path (created, sized, mapped shared, unlinked).
 * Runs EVERY group, then prints one line
 *   "syskat3 ok mmap=ok shared=ok anon_mib=256 resident_pages=256 prot=ok brk=ok madv=ok checks=<n> fail=none"
 * and exits 0, or the same line starting "syskat3 fail" with each failed group as fail(<id>) and fail=<first id>, exiting
 * with the first failed id:
 *   mmap   1 open+fstat size == argv size        2 mmap(file, PROT_READ, MAP_PRIVATE) succeeds
 *          3 last byte through the mapping == pread of the last byte      4 first page == pread of the first page
 *          5 MAP_PRIVATE|PROT_WRITE file mapping: a store is seen in memory and NOT in the file
 *          6 mmap of a file at a page offset past EOF maps (reads zeroes)  — Linux SIGBUSes past EOF, so 6 only maps it
 *   shared 10 create+ftruncate(2 pages)  11 mmap(MAP_SHARED, RW)  12 store both pages; msync page 0; pread page 0 sees it
 *          13 munmap; pread page 1 sees the store (write-back at munmap)   14 re-map shared RO: both stores read back
 *   anon   20 mmap 256 MiB anon RW (MADV_NOHUGEPAGE asked: the host would back 2 MiB per touch otherwise)
 *          21 mincore before any touch: 0 resident                         22 touch one page per MiB: values read back
 *          23 mincore after: exactly 256 resident (resident_pages)          24 munmap 256 MiB
 *   prot   30 mmap 64 MiB PROT_NONE reservation  31 mprotect 1 MiB in its middle RW, store, read back
 *          32 mincore over the reservation: only the touched page resident  33 mprotect PROT_NONE then RW keeps the bytes
 *          34 mprotect RO keeps the bytes                                  35 MAP_FIXED over part of it replaces with zeroes
 *          36 munmap of the whole reservation; mincore there -> ENOMEM
 *   brk    40 brk(0)   41 brk +128 MiB (past the pre-SELFBUILD3 64 MiB cap), touch first and last page
 *          42 shrink to +4 KiB then regrow: the far page reads zero        43 back to the start
 *   madv   50 anon page stored, MADV_DONTNEED, reads zero again            51 file MAP_PRIVATE page dirtied, DONTNEED, re-reads the file
 *   big    60 mmap 160 MiB anon RW   61 touch EVERY page (40960: past the 96 MiB heap-backed share, so UnaOS serves the rest
 *          from its user frame pool)  62 every page reads back its own stamp   63 munmap
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
#define MAP_FIX 0x10
#define MAP_AN 0x20
#define PG 4096UL
#define MIB (1UL << 20)

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
static u64 atou(const char *s) { u64 v = 0; while (*s >= '0' && *s <= '9') v = v * 10 + (u64)(*s++ - '0'); return v; }

static int checks, first_fail;
/* group verdicts: 0 = ok, else the first failed id of that group */
static int g_mmap, g_shared, g_anon, g_prot, g_brk, g_madv, g_big;
#define CHECK(g, id, cond) do { checks++; if (!(cond)) { if (!(g)) (g) = (id); if (!first_fail) first_fail = (id); goto g##_end; } } while (0)

static unsigned char pbuf[PG];
static unsigned char vec[65536];
static u64 resident_pages;

static u64 count_resident(char *a, u64 len)
{
    if (S3(27, a, len, vec) != 0) return (u64)-1;
    u64 n = 0;
    for (u64 i = 0; i < len / PG; i++) n += vec[i] & 1;
    return n;
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
    const char *file = argv[1], *scratch = argv[3];
    u64 size = atou(argv[2]);

    /* ---- mmap: a read-only file mapped MAP_PRIVATE ---- */
    {
        i64 fd = S4(257, -100, file, 0, 0);
        u64 st[18];
        CHECK(g_mmap, 1, fd >= 0 && S2(5, fd, st) == 0 && st[6] == size && size > 0);
        char *m = MMAP(0, size, PROT_R, MAP_PR, fd, 0);
        CHECK(g_mmap, 2, !BAD(m));
        unsigned char last = 0;
        CHECK(g_mmap, 3, S4(17, fd, &last, 1, size - 1) == 1 && (unsigned char)m[size - 1] == last);
        u64 n0 = size < PG ? size : PG;
        CHECK(g_mmap, 4, S4(17, fd, pbuf, n0, 0) == (i64)n0);
        int same = 1;
        for (u64 i = 0; i < n0; i++) if ((unsigned char)m[i] != pbuf[i]) same = 0;
        CHECK(g_mmap, 4, same);
        S2(11, m, size);
        char *w = MMAP(0, size, PROT_R | PROT_W, MAP_PR, fd, 0);
        CHECK(g_mmap, 5, !BAD(w));
        unsigned char was = (unsigned char)w[0];
        w[0] = (char)(was ^ 0x5a);
        CHECK(g_mmap, 5, (unsigned char)w[0] == (was ^ 0x5a) && S4(17, fd, pbuf, 1, 0) == 1 && pbuf[0] == was);
        S2(11, w, size);
        char *e = MMAP(0, PG, PROT_R, MAP_PR, fd, ((size + PG) & ~(PG - 1)) + PG);
        CHECK(g_mmap, 6, !BAD(e));
        S2(11, e, PG);
        S1(3, fd);
    }
g_mmap_end:

    /* ---- shared: a scratch file mapped MAP_SHARED, written back on msync and on munmap ---- */
    {
        S1(87, scratch);
        i64 fd = S4(257, -100, scratch, 0102 /*O_RDWR|O_CREAT*/, 0644);
        CHECK(g_shared, 10, fd >= 0 && S2(77, fd, 2 * PG) == 0);
        char *m = MMAP(0, 2 * PG, PROT_R | PROT_W, MAP_SH, fd, 0);
        CHECK(g_shared, 11, !BAD(m));
        for (u64 i = 0; i < PG; i++) { m[i] = (char)('A' + i % 26); m[PG + i] = (char)('a' + i % 26); }
        CHECK(g_shared, 12, S3(26, m, PG, 4 /*MS_SYNC*/) == 0 && S4(17, fd, pbuf, PG, 0) == (i64)PG && pbuf[0] == 'A' && pbuf[PG - 1] == (unsigned char)('A' + (PG - 1) % 26));
        CHECK(g_shared, 13, S2(11, m, 2 * PG) == 0 && S4(17, fd, pbuf, PG, PG) == (i64)PG && pbuf[0] == 'a' && pbuf[100] == (unsigned char)('a' + 100 % 26));
        char *r = MMAP(0, 2 * PG, PROT_R, MAP_SH, fd, 0);
        CHECK(g_shared, 14, !BAD(r) && r[1] == 'B' && r[PG + 2] == 'c');
        S2(11, r, 2 * PG);
        S1(3, fd);
        S1(87, scratch);
    }
g_shared_end:

    /* ---- anon: 256 MiB, one page per MiB ---- */
    {
        u64 len = 256 * MIB;
        char *a = MMAP(0, len, PROT_R | PROT_W, MAP_PR | MAP_AN, -1, 0);
        CHECK(g_anon, 20, !BAD(a));
        S3(28, a, len, 15 /*MADV_NOHUGEPAGE*/);
        CHECK(g_anon, 21, count_resident(a, len) == 0);
        for (u64 i = 0; i < 256; i++) a[i * MIB + 7] = (char)(i ^ 0x33);
        int okv = 1;
        for (u64 i = 0; i < 256; i++) if (a[i * MIB + 7] != (char)(i ^ 0x33) || a[i * MIB] != 0) okv = 0;
        CHECK(g_anon, 22, okv);
        resident_pages = count_resident(a, len);
        CHECK(g_anon, 23, resident_pages == 256);
        CHECK(g_anon, 24, S2(11, a, len) == 0);
    }
g_anon_end:

    /* ---- prot: a PROT_NONE reservation opened in part (malloc arenas, thread stacks) ---- */
    {
        u64 len = 64 * MIB;
        char *r = MMAP(0, len, 0, MAP_PR | MAP_AN, -1, 0);
        CHECK(g_prot, 30, !BAD(r));
        char *mid = r + 32 * MIB;
        CHECK(g_prot, 31, S3(10, mid, MIB, PROT_R | PROT_W) == 0);
        mid[5] = 42;
        CHECK(g_prot, 31, mid[5] == 42);
        CHECK(g_prot, 32, count_resident(r, len) == 1);
        CHECK(g_prot, 33, S3(10, mid, MIB, 0) == 0 && S3(10, mid, MIB, PROT_R | PROT_W) == 0 && mid[5] == 42);
        CHECK(g_prot, 34, S3(10, mid, MIB, PROT_R) == 0 && mid[5] == 42);
        char *f = MMAP(mid, PG, PROT_R | PROT_W, MAP_PR | MAP_AN | MAP_FIX, -1, 0);
        CHECK(g_prot, 35, f == mid && mid[5] == 0);
        CHECK(g_prot, 36, S2(11, r, len) == 0 && S3(27, r, PG, vec) == -12);
    }
g_prot_end:

    /* ---- brk past the old 64 MiB cap ---- */
    {
        i64 b0 = S1(12, 0);
        CHECK(g_brk, 40, b0 > 0);
        u64 big = 128 * MIB;
        CHECK(g_brk, 41, S1(12, b0 + big) == (i64)(b0 + big));
        ((char *)b0)[0] = 1;
        ((char *)b0)[big - 1] = 2;
        CHECK(g_brk, 41, ((char *)b0)[0] == 1 && ((char *)b0)[big - 1] == 2);
        S1(12, b0 + PG);
        CHECK(g_brk, 42, S1(12, b0 + big) == (i64)(b0 + big) && ((char *)b0)[big - 1] == 0 && ((char *)b0)[0] == 1);
        CHECK(g_brk, 43, S1(12, b0) == b0);
    }
g_brk_end:

    /* ---- madvise(DONTNEED) ---- */
    {
        char *a = MMAP(0, PG, PROT_R | PROT_W, MAP_PR | MAP_AN, -1, 0);
        a[9] = 9;
        CHECK(g_madv, 50, !BAD(a) && a[9] == 9 && S3(28, a, PG, 4 /*MADV_DONTNEED*/) == 0 && a[9] == 0);
        S2(11, a, PG);
        i64 fd = S4(257, -100, file, 0, 0);
        char *m = MMAP(0, PG, PROT_R | PROT_W, MAP_PR, fd, 0);
        unsigned char was = BAD(m) ? 0 : (unsigned char)m[0];
        if (!BAD(m)) m[0] = (char)(was ^ 0xff);
        CHECK(g_madv, 51, !BAD(m) && S3(28, m, PG, 4) == 0 && (unsigned char)m[0] == was);
        S2(11, m, PG);
        S1(3, fd);
    }
g_madv_end:

    /* ---- big: 160 MiB resident, every page touched ---- */
    {
        u64 len = 160 * MIB;
        char *b = MMAP(0, len, PROT_R | PROT_W, MAP_PR | MAP_AN, -1, 0);
        CHECK(g_big, 60, !BAD(b));
        S3(28, b, len, 15 /*MADV_NOHUGEPAGE*/);
        for (u64 i = 0; i < len / PG; i++) *(u64 *)(b + i * PG + 8) = i * 2654435761UL + 1;
        CHECK(g_big, 61, count_resident(b, 256 * PG) == 256);
        int okb = 1;
        for (u64 i = 0; i < len / PG; i++) if (*(u64 *)(b + i * PG + 8) != i * 2654435761UL + 1 || b[i * PG] != 0) okb = 0;
        CHECK(g_big, 62, okb);
        CHECK(g_big, 63, S2(11, b, len) == 0);
    }
g_big_end:

    out(first_fail ? "syskat3 fail" : "syskat3 ok");
    verdict("mmap", g_mmap);
    verdict("shared", g_shared);
    out(" anon_mib=");
    outn(g_anon ? 0 : 256);
    out(" resident_pages=");
    outn(resident_pages == (u64)-1 ? 0 : resident_pages);
    verdict("anon", g_anon);
    verdict("prot", g_prot);
    verdict("brk", g_brk);
    verdict("madv", g_madv);
    out(" big_mib=");
    outn(g_big ? 0 : 160);
    out(" checks=");
    outn((u64)checks);
    out(" fail=");
    if (first_fail) outn((u64)first_fail); else out("none");
    out("\n");
    S1(231, first_fail);
}

__asm__(".globl _start\n_start:\n  mov %rsp, %rdi\n  and $-16, %rsp\n  call cmain\n  hlt\n");
