/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 The Architect & Una
 *
 * SELFBUILD2 (B349) — SYSKAT2.LNX: known-answer tests for the THREADED surface a modern toolchain (rustc, cargo, a
 * linker) reaches beyond SYSKAT.LNX: clone/clone3 threads sharing the process, futex, statx, ftruncate/fallocate,
 * flock, eventfd2 + epoll, socketpair(AF_UNIX), sendfile, and a real signal delivery through rt_sigreturn.
 * Freestanding (gcc -nostdlib -static -fno-pie -no-pie -mno-red-zone): raw syscalls only, so the KAT measures the
 * shim, not a libc. Checked against the host Linux kernel first (arroyo runs it there before staging).
 *
 * argv: SYSKAT2.LNX <file> <size> <scratch>   — <file> is statx'd and must be <size> bytes (the loader's view of
 * /apps/HELLO.C on UnaOS); <scratch> is a writable path (created, truncated, locked, unlinked).
 * Prints "syskat2 ok threads=4 counter=400000 futex_waits=<n> epoll=ok statx=ok sigreturn=ok checks=<n>" and exits 0,
 * or prints "syskat2 fail <id> <the same fields, ? where unproven> checks=<n>" and exits with the id of the first check that failed:
 *   1 clone(CLONE_THREAD) x2 + PARENT_SETTID   2 clone3(CLONE_THREAD) x2 (fn in rdx, arg in r8: glibc's shape)
 *   3 join: CHILD_CLEARTID zeroed + futex-woke  4 counter == 4 x 100000 under a futex mutex
 *   5 per-thread FS_BASE (CLONE_SETTLS)         6 gettid distinct per thread, main gettid == getpid
 *   7 FUTEX_WAIT value mismatch -> EAGAIN        8 FUTEX_WAIT relative timeout -> ETIMEDOUT after >= 20 ms
 *   9 FUTEX_WAKE with no waiter -> 0            10 FUTEX_WAIT_BITSET absolute timeout -> ETIMEDOUT
 *  11 FUTEX_WAKE_BITSET wakes only a matching bitset   12 FUTEX_REQUEUE moves 2 waiters, WAKE on the target wakes 2
 *  13 eventfd2 NONBLOCK read of 0 -> EAGAIN    14 epoll_create1 + epoll_ctl ADD (EEXIST on a second ADD)
 *  15 epoll_wait(0) on a quiet eventfd -> 0    16 epoll_wait woken by a thread's eventfd write (data + EPOLLIN, value 1)
 *  17 statx(path) size/mode/mask               18 statx(fd, "", AT_EMPTY_PATH) == fstat size
 *  19 ftruncate shrink + grow (zero-filled)    20 fallocate grows the file
 *  21 flock EX vs a second description (EWOULDBLOCK), UN, then granted   22 socketpair(AF_UNIX) both directions
 *  23 rt_sigaction install + read back         24 kill(self, SIGUSR1): handler ran ONCE, siginfo, callee-saved regs kept
 *  25 blocked SIGUSR1 stays pending, unblock delivers it (once)          26 sendfile file -> socket
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
#define S5(n, a, b, c, d, e) sc6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), (i64)(e), 0)
#define S6(n, a, b, c, d, e, f) sc6(n, (i64)(a), (i64)(b), (i64)(c), (i64)(d), (i64)(e), (i64)(f))

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
/* SMALLFIX (B380): the fields as far as the run got, printed on failure too, so the wire names more than a number:
 * "syskat2 fail <id> threads=<n|?> counter=<n|?> futex_waits=<n> epoll=<ok|?> statx=<ok|?> sigreturn=<ok|?> checks=<n>". */
static int f_threads = -1, f_epoll, f_statx, f_sigret;
static u64 counter;
static int nwaits;
static void okq(const char *k, int ok) { out(k); out(ok ? "ok" : "?"); }
static void fail_at(int id, int checks)
{
    out("syskat2 fail ");
    outn(id);
    out(" threads=");
    if (f_threads < 0) out("?"); else outn(f_threads);
    out(" counter=");
    if (f_threads < 0) out("?"); else outn(counter);
    out(" futex_waits=");
    outn(nwaits);
    okq(" epoll=", f_epoll);
    okq(" statx=", f_statx);
    okq(" sigreturn=", f_sigret);
    out(" checks=");
    outn(checks);
    out("\n");
    S1(231, id);
}
#define CHECK(id, cond) do { if (!(cond)) fail_at(id, checks); checks++; } while (0)

static u64 atou(const char *s) { u64 v = 0; while (*s >= '0' && *s <= '9') v = v * 10 + (u64)(*s++ - '0'); return v; }
static i64 now_ms(void) { i64 t[2]; S2(228, 1, t); return t[0] * 1000 + t[1] / 1000000; }
static void sleep_ms(i64 ms) { i64 req[2] = { ms / 1000, (ms % 1000) * 1000000 }; S2(35, req, 0); }

/* ---- futex ---- */
#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_REQUEUE 3
#define FUTEX_WAIT_BITSET 9
#define FUTEX_WAKE_BITSET 10
#define FUTEX_PRIVATE 128
static i64 futex(int *u, int op, int val, i64 ts_or_val2, int *u2, int val3) { return S6(202, u, op, val, ts_or_val2, u2, val3); }

static void lock(int *m)
{
    int c = 0;
    if (__atomic_compare_exchange_n(m, &c, 1, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST)) return;
    if (c != 2) c = __atomic_exchange_n(m, 2, __ATOMIC_SEQ_CST);
    while (c != 0) {
        __atomic_fetch_add(&nwaits, 1, __ATOMIC_SEQ_CST);
        futex(m, FUTEX_WAIT | FUTEX_PRIVATE, 2, 0, 0, 0);
        c = __atomic_exchange_n(m, 2, __ATOMIC_SEQ_CST);
    }
}
static void unlock(int *m)
{
    if (__atomic_fetch_sub(m, 1, __ATOMIC_SEQ_CST) != 1) {
        __atomic_store_n(m, 0, __ATOMIC_SEQ_CST);
        futex(m, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0, 0, 0);
    }
}

/* ---- threads: raw clone / clone3 ---- */
#define CLONE_FLAGS 0x3d0f00UL /* VM|FS|FILES|SIGHAND|THREAD|SYSVSEM|SETTLS|PARENT_SETTID|CHILD_CLEARTID: glibc's set */
i64 kclone(u64 flags, void *stack_top, int *ptid, int *ctid, u64 tls, void (*fn)(void *), void *arg);
i64 kclone3(void *cl_args, u64 size, void (*fn)(void *), void *arg);
__asm__(
    ".globl kclone\nkclone:\n"
    "  mov 8(%rsp), %rax\n  sub $16, %rsi\n  mov %r9, 0(%rsi)\n  mov %rax, 8(%rsi)\n"
    "  mov %rcx, %r10\n  mov $56, %eax\n  syscall\n  test %rax, %rax\n  jz 1f\n  ret\n"
    "1:\n  xor %ebp, %ebp\n  pop %rax\n  pop %rdi\n  call *%rax\n  xor %edi, %edi\n  mov $60, %eax\n  syscall\n  hlt\n"
    ".globl kclone3\nkclone3:\n"
    "  mov %rcx, %r8\n  mov $435, %eax\n  syscall\n  test %rax, %rax\n  jz 2f\n  ret\n"
    "2:\n  xor %ebp, %ebp\n  mov %r8, %rdi\n  call *%rdx\n  xor %edi, %edi\n  mov $60, %eax\n  syscall\n  hlt\n");

#define NT 8
#define STK 65536
static char stacks[NT][STK] __attribute__((aligned(16)));
static u64 tls[NT][8] __attribute__((aligned(64)));
static int tidw[NT];

struct targ {
    int idx;
    int tid;
    int tls_ok;
    void (*job)(struct targ *);
    i64 r;
};
static struct targ ta[NT];

static int mtx;
static void count_job(struct targ *t)
{
    for (int i = 0; i < 100000; i++) {
        lock(&mtx);
        counter++;
        unlock(&mtx);
    }
    (void)t;
}
static void tmain(void *a)
{
    struct targ *t = a;
    u64 fs0;
    __asm__ volatile ("mov %%fs:0, %0" : "=r"(fs0));
    t->tls_ok = fs0 == (u64)&tls[t->idx][0];
    t->tid = (int)S0(186);
    t->job(t);
}
static i64 spawn(int i, int use3, void (*job)(struct targ *))
{
    ta[i].idx = i;
    ta[i].job = job;
    tls[i][0] = (u64)&tls[i][0]; /* x86_64 TCB self pointer */
    tidw[i] = 0;
    if (!use3) return kclone(CLONE_FLAGS, stacks[i] + STK, &tidw[i], &tidw[i], (u64)&tls[i][0], tmain, &ta[i]);
    u64 ca[11] = { 0 };
    ca[0] = CLONE_FLAGS;
    ca[2] = (u64)&tidw[i]; /* child_tid (CLEARTID) */
    ca[3] = (u64)&tidw[i]; /* parent_tid */
    ca[5] = (u64)stacks[i];
    ca[6] = STK;
    ca[7] = (u64)&tls[i][0];
    return kclone3(ca, 88, tmain, &ta[i]);
}
static int join(int i)
{
    i64 t0 = now_ms();
    int v;
    while ((v = __atomic_load_n(&tidw[i], __ATOMIC_SEQ_CST)) != 0) {
        __atomic_fetch_add(&nwaits, 1, __ATOMIC_SEQ_CST);
        i64 ts[2] = { 1, 0 };
        futex(&tidw[i], FUTEX_WAIT, v, (i64)ts, 0, 0);
        if (now_ms() - t0 > 10000) return 0;
    }
    return 1;
}

/* ---- futex unit jobs ---- */
static int fw, fa, fb, ready;
static void bitset_job(struct targ *t)
{
    i64 ts[2];
    S2(228, 1, ts);
    ts[0] += 3;
    __atomic_fetch_add(&ready, 1, __ATOMIC_SEQ_CST);
    t->r = futex(&fw, FUTEX_WAIT_BITSET | FUTEX_PRIVATE, 0, (i64)ts, 0, 2);
}
static void requeue_job(struct targ *t)
{
    i64 ts[2] = { 3, 0 };
    __atomic_fetch_add(&ready, 1, __ATOMIC_SEQ_CST);
    t->r = futex(&fa, FUTEX_WAIT | FUTEX_PRIVATE, 0, (i64)ts, 0, 0);
}
static int efd;
static void efd_job(struct targ *t)
{
    sleep_ms(30);
    u64 one = 1;
    t->r = S3(1, efd, &one, 8);
}

/* ---- signals ---- */
static volatile int hits, lastsig, infosig;
static void handler(int sig, void *si, void *uc)
{
    hits++;
    lastsig = sig;
    infosig = *(int *)si;
    (void)uc;
}
void restorer(void);
__asm__(".globl restorer\nrestorer:\n  mov $15, %eax\n  syscall\n  hlt\n");
/* kill(pid, sig) with every callee-saved register (and rbp) holding a sentinel across the syscall: 1 = all survived. */
static int kill_sentinel(i64 pid, i64 sig, i64 *rc)
{
    i64 r;
    u64 ok;
    __asm__ volatile (
        "push %%rbx\n push %%r12\n push %%r13\n push %%r14\n push %%r15\n push %%rbp\n"
        "mov $0x1111, %%ebx\n mov $0x2222, %%r12d\n mov $0x3333, %%r13d\n mov $0x4444, %%r14d\n mov $0x5555, %%r15d\n mov $0x6666, %%ebp\n"
        "mov $62, %%eax\n syscall\n"
        "xor %%ecx, %%ecx\n"
        "cmp $0x1111, %%rbx\n jne 1f\n cmp $0x2222, %%r12\n jne 1f\n cmp $0x3333, %%r13\n jne 1f\n"
        "cmp $0x4444, %%r14\n jne 1f\n cmp $0x5555, %%r15\n jne 1f\n cmp $0x6666, %%rbp\n jne 1f\n"
        "mov $1, %%ecx\n"
        "1:\n pop %%rbp\n pop %%r15\n pop %%r14\n pop %%r13\n pop %%r12\n pop %%rbx\n"
        : "=a"(r), "=c"(ok) : "D"(pid), "S"(sig) : "r11", "rdx", "r8", "r9", "r10", "memory");
    *rc = r;
    return ok == 1;
}

static char buf[4096];

void cmain(u64 *sp)
{
    int checks = 0;
    i64 argc = (i64)sp[0];
    const char *file = argc > 1 ? (const char *)sp[2] : "/apps/HELLO.C";
    u64 want = argc > 2 ? atou((const char *)sp[3]) : 0;
    const char *scratch = argc > 3 ? (const char *)sp[4] : "/tmp/syskat2.tmp";
    i64 pid = S0(39);

    /* ---- M1: 4 threads contend a futex mutex ---- */
    i64 t0 = spawn(0, 0, count_job), t1 = spawn(1, 0, count_job);
    CHECK(1, t0 > 0 && t1 > 0 && t0 != t1 && t0 != pid);
    i64 t2 = spawn(2, 1, count_job), t3 = spawn(3, 1, count_job);
    CHECK(2, t2 > 0 && t3 > 0 && t2 != t3 && t2 != t0);
    int joined = join(0) & join(1) & join(2) & join(3);
    CHECK(3, joined);
    CHECK(4, counter == 400000);
    CHECK(5, ta[0].tls_ok && ta[1].tls_ok && ta[2].tls_ok && ta[3].tls_ok);
    f_threads = 4;
    CHECK(6, ta[0].tid == t0 && ta[1].tid == t1 && ta[2].tid == t2 && ta[3].tid == t3 && S0(186) == pid);

    /* ---- futex unit checks ---- */
    fw = 5;
    CHECK(7, futex(&fw, FUTEX_WAIT | FUTEX_PRIVATE, 4, 0, 0, 0) == -11);
    i64 rel[2] = { 0, 30000000 };
    i64 m0 = now_ms();
    i64 r8 = futex(&fw, FUTEX_WAIT | FUTEX_PRIVATE, 5, (i64)rel, 0, 0);
    i64 dm = now_ms() - m0;
    CHECK(8, r8 == -110 && dm >= 20 && dm < 5000);
    CHECK(9, futex(&fw, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0, 0, 0) == 0);
    i64 abs_[2];
    S2(228, 1, abs_);
    abs_[1] += 20000000;
    if (abs_[1] >= 1000000000) { abs_[1] -= 1000000000; abs_[0]++; }
    CHECK(10, futex(&fw, FUTEX_WAIT_BITSET | FUTEX_PRIVATE, 5, (i64)abs_, 0, -1) == -110);
    fw = 0;
    ready = 0;
    spawn(4, 0, bitset_job);
    while (__atomic_load_n(&ready, __ATOMIC_SEQ_CST) < 1) sleep_ms(1);
    sleep_ms(30);
    i64 wb1 = futex(&fw, FUTEX_WAKE_BITSET | FUTEX_PRIVATE, 1, 0, 0, 1);
    i64 wb2 = futex(&fw, FUTEX_WAKE_BITSET | FUTEX_PRIVATE, 1, 0, 0, 2);
    CHECK(11, join(4) && wb1 == 0 && wb2 == 1 && ta[4].r == 0);
    fa = 0;
    fb = 0;
    ready = 0;
    spawn(5, 0, requeue_job);
    spawn(6, 1, requeue_job);
    while (__atomic_load_n(&ready, __ATOMIC_SEQ_CST) < 2) sleep_ms(1);
    sleep_ms(30);
    i64 rq = futex(&fa, FUTEX_REQUEUE | FUTEX_PRIVATE, 0, 2, &fb, 0);
    i64 wk = futex(&fb, FUTEX_WAKE | FUTEX_PRIVATE, 10, 0, 0, 0);
    CHECK(12, join(5) && join(6) && rq == 2 && wk == 2 && ta[5].r == 0 && ta[6].r == 0);

    /* ---- eventfd2 + epoll ---- */
    efd = (int)S2(290, 0, 04000 /*EFD_NONBLOCK*/);
    u64 ev = 0;
    CHECK(13, efd >= 0 && S3(0, efd, &ev, 8) == -11);
    i64 ep = S1(291, 02000000 /*EPOLL_CLOEXEC*/);
    unsigned char ee[12 * 4];
    *(u32 *)ee = 1; /* EPOLLIN */
    *(u64 *)(ee + 4) = 0x1234;
    CHECK(14, ep >= 0 && S4(233, ep, 1 /*ADD*/, efd, ee) == 0 && S4(233, ep, 1, efd, ee) == -17);
    CHECK(15, S4(232, ep, ee, 4, 0) == 0);
    spawn(7, 0, efd_job);
    i64 nev = S4(232, ep, ee, 4, 3000);
    u64 val = 0;
    CHECK(16, join(7) && ta[7].r == 8 && nev == 1 && (*(u32 *)ee & 1) && *(u64 *)(ee + 4) == 0x1234
              && S3(0, efd, &val, 8) == 8 && val == 1);
    f_epoll = 1;
    S1(3, ep);

    /* ---- statx ---- */
    unsigned char stx[256];
    for (int i = 0; i < 256; i++) stx[i] = 0xee;
    CHECK(17, S5(332, -100, file, 0, 0x7ff /*STATX_BASIC_STATS*/, stx) == 0 && (*(u32 *)stx & 0x200 /*SIZE*/)
              && *(u64 *)(stx + 40) == want && want > 0 && (*(unsigned short *)(stx + 28) & 0170000) == 0100000);
    i64 ffd = S4(257, -100, file, 0, 0);
    u64 st[18];
    CHECK(18, ffd >= 0 && S2(5, ffd, st) == 0 && S5(332, ffd, "", 0x1000 /*AT_EMPTY_PATH*/, 0x7ff, stx) == 0
              && *(u64 *)(stx + 40) == st[6]);
    f_statx = 1;

    /* ---- ftruncate / fallocate / flock on a scratch file ---- */
    S1(87, scratch);
    i64 w = S4(257, -100, scratch, 0102 /*O_RDWR|O_CREAT*/ | 01000 /*O_TRUNC*/, 0644);
    int ok19 = w >= 0 && S3(1, w, "0123456789", 10) == 10 && S2(77, w, 5) == 0 && S2(5, w, st) == 0 && st[6] == 5
               && S2(77, w, 4103) == 0 && S2(5, w, st) == 0 && st[6] == 4103;
    if (ok19) {
        buf[0] = 1;
        ok19 = S4(17, w, buf, 8, 4) == 8 && buf[0] == '4' && buf[1] == 0 && buf[7] == 0;
    }
    CHECK(19, ok19);
    CHECK(20, S4(285, w, 0, 0, 8192) == 0 && S2(5, w, st) == 0 && st[6] == 8192);
    i64 w2 = S4(257, -100, scratch, 0, 0);
    CHECK(21, w2 >= 0 && S2(73, w, 2 /*LOCK_EX*/) == 0 && S2(73, w2, 2 | 4 /*NB*/) == -11 && S2(73, w, 8 /*UN*/) == 0
              && S2(73, w2, 2 | 4) == 0 && S2(73, w2, 8) == 0);
    S1(3, w2);
    S1(3, w);
    S1(87, scratch);

    /* ---- socketpair(AF_UNIX) + sendfile ---- */
    int sv[2] = { -1, -1 };
    int ok22 = S4(53, 1 /*AF_UNIX*/, 1 /*SOCK_STREAM*/, 0, sv) == 0 && S3(1, sv[0], "ping", 4) == 4
               && S3(0, sv[1], buf, 16) == 4 && buf[0] == 'p' && buf[3] == 'g' && S3(1, sv[1], "pong!", 5) == 5
               && S3(0, sv[0], buf, 16) == 5 && buf[4] == '!';
    CHECK(22, ok22);
    S3(8, ffd, 0, 0);
    i64 sf = S4(40, sv[0], ffd, 0, 64);
    CHECK(26, sf == (want < 64 ? (i64)want : 64) && S3(0, sv[1], buf, 128) == sf);
    S1(3, ffd);
    S1(3, sv[0]);
    S1(3, sv[1]);

    /* ---- signals: a real delivery and rt_sigreturn ---- */
    u64 act[4] = { (u64)handler, 0x04000000 /*SA_RESTORER*/ | 4 /*SA_SIGINFO*/, (u64)restorer, 0 }, old[4] = { 9, 9, 9, 9 };
    CHECK(23, S4(13, 10 /*SIGUSR1*/, act, 0, 8) == 0 && S4(13, 10, 0, old, 8) == 0 && old[0] == (u64)handler
              && (old[1] & 4));
    i64 krc = -1;
    int kept = kill_sentinel(pid, 10, &krc);
    CHECK(24, kept && krc == 0 && hits == 1 && lastsig == 10 && infosig == 10);
    u64 set = 1UL << 9;
    S4(14, 0 /*SIG_BLOCK*/, &set, 0, 8);
    S2(62, pid, 10);
    int held = hits == 1;
    S4(14, 1 /*SIG_UNBLOCK*/, &set, 0, 8);
    CHECK(25, held && hits == 2);
    f_sigret = 1;

    out("syskat2 ok threads=4 counter=");
    outn(counter);
    out(" futex_waits=");
    outn(nwaits);
    out(" epoll=ok statx=ok sigreturn=ok checks=");
    outn(checks);
    out("\n");
    S1(231, 0);
}

__asm__(".globl _start\n_start:\n  mov %rsp, %rdi\n  and $-16, %rsp\n  call cmain\n  hlt\n");
