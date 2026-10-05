/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 The Architect & Una
 *
 * SELFBUILDMETAL (B367) reproduction tool, not shipped. Build: gcc -O1 -o scrubtrace scrubtrace.c
 * Run:   taskset -c 0 ./scrubtrace scrub|keep <static Linux program> [args]   (SCRUBLOG=1 prints pread64/futex/mremap args)
 */
/* Host reproduction of the UnaOS x86 SYSCALL return: at every syscall EXIT zero rdi rsi rdx r8 r9 r10 (the stub's
 * U1b B1 scrub), or with mode=keep leave them (Linux). Follows threads/forks. Prints the first fatal signal (rip, addr). */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <sys/ptrace.h>
#include <sys/wait.h>
#include <sys/user.h>
int main(int argc, char **argv) {
    int scrub = strcmp(argv[1], "scrub") == 0;
    pid_t c = fork();
    if (c == 0) { ptrace(PTRACE_TRACEME, 0, 0, 0); raise(SIGSTOP); execv(argv[2], argv + 2); _exit(127); }
    int st; waitpid(c, &st, 0);
    ptrace(PTRACE_SETOPTIONS, c, 0, PTRACE_O_TRACESYSGOOD | PTRACE_O_TRACECLONE | PTRACE_O_TRACEFORK | PTRACE_O_TRACEVFORK | PTRACE_O_TRACEEXEC | PTRACE_O_EXITKILL);
    ptrace(PTRACE_SYSCALL, c, 0, 0);
    static char insys[1 << 22];
    long nsys = 0;
    for (;;) {
        pid_t p = waitpid(-1, &st, __WALL);
        if (p < 0) break;
        if (WIFEXITED(st) || WIFSIGNALED(st)) { if (p == c) { printf("[scrubtrace] root exit=%d sig=%d syscalls=%ld\n", WIFEXITED(st) ? WEXITSTATUS(st) : -1, WIFSIGNALED(st) ? WTERMSIG(st) : 0, nsys); } continue; }
        int sig = 0;
        if (WIFSTOPPED(st)) {
            int s = WSTOPSIG(st);
            if (s == (SIGTRAP | 0x80)) {
                insys[p] ^= 1;
                if (insys[p] && getenv("SCRUBLOG")) { struct user_regs_struct r; ptrace(PTRACE_GETREGS, p, 0, &r); if (r.orig_rax == 17 || r.orig_rax == 202 || r.orig_rax == 25) fprintf(stderr, "[scrubtrace] pid=%d sys=%lld a0=%#llx a1=%#llx a2=%#llx a3=%#llx\n", p, r.orig_rax, r.rdi, r.rsi, r.rdx, r.r10); }
                if (!insys[p]) {
                    nsys++;
                    if (scrub) {
                        struct user_regs_struct r; ptrace(PTRACE_GETREGS, p, 0, &r);
                        if (r.orig_rax != 59 || (long)r.rax < 0) { r.rdi = r.rsi = r.rdx = r.r8 = r.r9 = r.r10 = 0; ptrace(PTRACE_SETREGS, p, 0, &r); }
                    }
                }
            } else if (s == SIGTRAP) {
                /* ptrace event stop (clone/fork/exec): new tracee starts outside a syscall */
            } else if (s == SIGSTOP && (st >> 16) == 0) {
                /* new child's initial stop */
            } else {
                if (s == SIGSEGV || s == SIGBUS) {
                    siginfo_t si; ptrace(PTRACE_GETSIGINFO, p, 0, &si);
                    struct user_regs_struct r; ptrace(PTRACE_GETREGS, p, 0, &r);
                    printf("[scrubtrace] pid=%d sig=%d rip=%#llx addr=%p\n", p, s, r.rip, si.si_addr);
                }
                sig = s;
            }
        }
        ptrace(PTRACE_SYSCALL, p, 0, sig);
    }
    return 0;
}
