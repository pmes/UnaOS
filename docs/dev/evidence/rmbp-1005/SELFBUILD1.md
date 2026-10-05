# SELFBUILD1 — the first rung of SH-5: UnaOS compiles a C program on itself (ledger B344)

## Design
**Finding.** ROADMAP §1c SH-5 ("UnaOS builds UnaOS") had no first rung: no native toolchain. The tree already runs static Linux
x86_64 binaries under the Linux ABI shim (LINUXABI1–3: loader, ~90 syscalls, fork/exec/pipes, SSE). A static toolchain IS a
static Linux binary, so the rung is: measure the syscall surface a static compiler reaches, close it, and compile on the metal.

**Seam.** `CHARTER: Kernel — driver` (the compatibility box, as LINUXABI3's `fpu.rs`). No handler owns the Linux ABI; the work is
two new kernel files under `arch/x86_64/linuxabi/` (`sys2.rs` the new syscall answers, `selfbuild.rs` the tests) plus one routing
line per touched arm in `sys.rs`, an `exe` field on `LinuxProc`, and host-side build/staging (`arroyo`, `builder`).

**Toolchain (host, this container).** `musl-gcc` is absent; gcc 13.3 + glibc 2.39 `libc.a` are present. So every staged binary is
**glibc-static** (`-static -no-pie`, ET_EXEC — the loader refuses static-PIE). glibc-static adds start-up syscalls a musl binary
would not make (`rseq`, `readlinkat("/proc/self/exe")`, `getrandom`, `prlimit64`, `set_robust_list`) — measured below, that is data.
- **tcc**: tinycc `mob`, fetched from `https://github.com/TinyCC/tinycc.git` at commit `43c7708b85681a2fd4451c8a541af4494a8919b2`
  (2026-10-03; `git archive` tarball sha256 `a9c4b77014ef4efa30dd3ee8e735e909a10b2f7ff83cb5c770ee3a2940607c57`; repo.or.cz was
  refused by the egress proxy). Built in `unaos/target/selfbuild/tinycc` (`./configure --cc=gcc --extra-cflags="-O2 -fno-pie"
  --extra-ldflags="-static -no-pie"; make tcc`), stripped: `TCC.LNX` 1226992 bytes, sha256
  `c1a1624147c38850cf4cae3e5bcb24cb686d28a27c5c6b55ae8f857c53418f75` (rebuilt twice, identical). Libc carried: **glibc 2.39 static**.
  **Licence note (R52-style):** tinycc is LGPL-2.1. Its source NEVER enters this tree; `arroyo` fetches it at the pinned commit into
  `target/` (git-ignored), and only the stripped binary is staged on the volume (`APPS/TCC.LNX`), its corresponding source being that
  public upstream commit. Same for the busybox probe (GPL-2.0, `https://github.com/mirror/busybox.git` at
  `371fe9f71d445d18be28c82a2a6d82115c8af19d`, 1.37.0; `PROBE.LNX` 2473056 bytes, sha256
  `3752671d944587c64b1b1c9357f3a776b4a60b8074f4c13ffed68817ed122871`, defconfig + STATIC + SH_STANDALONE + PREFER_APPLETS, TC off).
- **HELLO.C** (`crates/user-linux-hello/c/hello.c`, ours): freestanding — raw `write`/`exit_group` through tcc's inline asm and its own
  `_start`, because no libc (crt1.o, libc.a, headers) is staged on the volume. tcc is invoked `-nostdlib -static`, so it needs no
  `libtcc1.a` either. Output on the host: a 959-byte ET_EXEC (two PT_LOADs, R and RX, no PT_INTERP) that prints
  `hello from tcc on unaos`.

**Milestones.** M1 measure (host strace of the exact staged binaries + the shim's table). M2 implement what is missing, each with a
KAT (`SYSKAT.LNX`, second witness of `tests linuxabi`). M3 `tests selfbuild`: tcc compiles HELLO.C to `<home>/hello.lnx` on the volume,
the output runs. M4 this doc: the surface table, the rustc distance.

**Witness.** `:: SELFBUILD1: tcc=<ok|enosys:[…]|fail(…)|skip> hello=<ok|fail(…)|skip> probe=busybox probe_syscalls=<n> probe_missing=[…] -> PASS|FAIL|SKIP ::`
and `:: LINUXABI-KAT: path=/apps/SYSKAT.LNX checks=30 fail=none exit=0 syscalls=<n> enosys=[] -> PASS ::`. No new knob — rides
`UNAOS_LINUXABI=1` (the x86 metal shape carries `linuxabi`).

## M1 — the measured surface
`strace -f` on the host Linux 6.18 kernel of the EXACT binaries staged (`unaos/target/selfbuild/*.strace` when built). "Before" = the
LINUXABI1–3 table at the cut (8986c950); "now" = after M2.

**tcc** `tcc -nostdlib -static -o hello.lnx hello.c` — 29 calls, 19 distinct after execve, in order of first appearance:

| nr | syscall | calls | before SELFBUILD1 | now |
|---|---|---|---|---|
| 12 | brk | 5 | answered | answered |
| 158 | arch_prctl(SET_FS) | 1 | answered | answered |
| 218 | set_tid_address | 1 | answered (returns pid) | answered |
| 273 | set_robust_list | 1 | no-op 0 | no-op 0 |
| 334 | rseq | 1 | **-ENOSYS, noted: the FIRST enosys** | -ENOSYS on purpose (pre-4.18 answer; glibc runs without rseq), not noted |
| 302 | prlimit64(STACK) | 1 | answered | answered |
| 267 | readlinkat("/proc/self/exe") | 1 | -EINVAL (no symlinks) | the image path |
| 318 | getrandom(8, NONBLOCK) | 1 | answered (clock xorshift, not crypto) | same |
| 10 | mprotect(RELRO, READ) | 1 | answered | answered |
| 257 | openat(hello.c) | 1 | answered | answered |
| 9 | mmap(anon RW 260 KiB) | 2 | answered | answered |
| 0 | read | 2 | answered | answered |
| 3 | close | 2 | answered | answered |
| 11 | munmap | 2 | answered | answered |
| 87 | unlink(out) | 1 | answered (only under /home/<user>/) | same |
| 257 | openat(out, WRONLY/CREAT/TRUNC, 0777) | 1 | answered (create under /home/<user>/) | same |
| 72 | fcntl(F_GETFL) | 1 | answered | answered |
| 5 | fstat | 1 | answered | answered |
| 1 | write(959 B) | 1 | answered (write-through) | same |
| 231 | exit_group | 1 | answered | answered |

So tcc was ONE deliberate answer away (rseq — and glibc already survives -ENOSYS there) plus a soft one (`/proc/self/exe`).
No clone, no futex, no threads, no signals: **tcc needs neither clone nor futex** (not implemented — not needed).

**Probe: busybox 1.37 static**, `busybox sh -c 'echo probe | wc -c; ls /apps; head -n 3 /apps/HELLO.C; uname -a'` — 258 calls,
41 distinct, 5 processes (fork via `clone(CHILD_SETTID|CHILD_CLEARTID|SIGCHLD)`, applets re-exec `/proc/self/exe`):
newfstatat 42, rt_sigaction 28, close 15, brk 13, mmap 12, fstat 12, wait4 8, set_robust_list 8, prctl 7, write/read/openat/mprotect/ioctl/exit_group 6,
rt_sigreturn 5, getuid 5, clone 5, rt_sigprocmask/prlimit64/getppid/getpid/dup2 4, uname/set_tid_address/rseq/getrandom/getgid/geteuid/getegid/execve/arch_prctl/access 3,
readlinkat/pread64/pipe2/getdents64 2, munmap/lseek/getpgrp/getpeername 1.
Before SELFBUILD1 unanswered: rseq(334), prctl(157), getpeername(52), and `execve("/proc/self/exe")` (ENOENT — no /proc) and
`CLONE_CHILD_SETTID` ignored. Now all answered. Left: `rt_sigreturn`(15) — only ever called on return from a delivered signal handler
(SIGCHLD on Linux); the shim delivers no signals, so it is unreachable here (owed with signal delivery).

**rustc — UNPROBED as a static binary.** No static rustc can be built in this container (rustc is a dynamic program over
librustc_driver + LLVM; a static musl rustc needs a musl host toolchain we do not have). What IS measured: the host's dynamic
`rustc 1.99.0-nightly` (via the rustup proxy, which execs it, and the `cc` link step) compiling a one-line `main` — 5293 calls,
64 distinct, 20 threads/processes. Unanswered by the shim even now:

| syscall | calls | why rustc needs it |
|---|---|---|
| futex(202) | 1593 | every lock/condvar in a threaded rustc (jobserver, query system, LLVM codegen units) |
| clone3(435) → clone(CLONE_VM\|CLONE_THREAD) | 16 | threads: the shim answers `-ENOSYS` to both (no shared-address-space tasks) |
| statx(332) | 152 | std's `fs::metadata` prefers statx |
| ftruncate(77) | 2 | output files (rlib/rmeta) |
| flock(73) | 1 | incremental/dep-graph lock |
| eventfd2/epoll_create1/epoll_ctl/epoll_wait (290/291/233/232), socketpair(53), recvfrom(45) | 1 each | the jobserver/helper thread |
| rt_sigreturn(15) | 1 | signal delivery |

Plus the shapes the table hides: file-backed `mmap` of shared objects with PROT_EXEC (moot for a static binary), an mmap-heavy
allocator (rustc + LLVM routinely map hundreds of MiB; the shim caps a process at `MAX_PAGES` 24576 = 96 MiB and the mmap window at
768 MiB, files are slurped whole at open, fork copies eagerly), `/proc/self/*` reads beyond `exe` (none answered: there is no /proc),
`readlink` 965 calls (rustup's sysroot discovery; answered -EINVAL except `/proc/self/exe`), and `vfork`+`execve` of the linker
(`cc`, itself needing `ld`) — so a self-hosted rustc ALSO needs a static linker staged (tcc can link ELF itself; rustc cannot use it).

## M2 — written (the KAT is the proof; each id is a check in `SYSKAT.LNX`)
- `linuxabi/sys2.rs` (new, `//! CHARTER: Kernel — driver`): `readlink`/`readlinkat` (`/proc/self/exe` = `LinuxProc::exe`, else
  -EINVAL/-ENOENT; KAT 13), `rt_sigaction`/`rt_sigprocmask` accepted with the OLD values written back (SIG_DFL / empty mask; SIGKILL/STOP
  refused; KAT 18/19), `sigaltstack` (old = SS_DISABLE; KAT 20), `rseq`/`clone3` deliberate -ENOSYS (KAT 28: answers, never kills),
  `sched_getaffinity` (one CPU — pinned), `sched_setaffinity`/`personality`/`setrlimit`/`chown*` accepted, `chmod`/`fchmod*` accepted on
  existing paths (no mode bits on the VFS), `prctl` NAME, `getrusage`/`times`/`time`/`clock_getres`, `getsockname`/`getpeername`
  -ENOTSOCK, and `clone_settid` (CLONE_PARENT_SETTID / CLONE_CHILD_SETTID, glibc's fork).
- `sys.rs`: three arms route to sys2 (13/14, 89/267, the fall-through before `-ENOSYS`); `home_prefix` made `pub`.
- `proc.rs`: `exe` carried across fork, reset at execve; `execve("/proc/self/exe")` re-executes this image; clone calls `clone_settid`.
- `mod.rs`: `LinuxProc::exe`; `pub mod selfbuild; pub mod sys2;`; `selftest` (tests linuxabi) runs `selfbuild::kat()` after HELLO; the
  trace names sigaltstack/prctl/sched_getaffinity/dup3/getpeername/clone3.
- Already answered at the cut and now covered by the KAT: mmap/munmap/mprotect on real pages (1–6: anon RW zeroed, RO, unmapped
  -ENOMEM, munmap + MAP_FIXED remap zeroed, W+X -EACCES — UnaOS refuses what Linux grants, the KAT accepts both), brk growth and
  shrink+regrow zeroed (7/8), openat/fstat/read/lseek/close over the VFS (9–11, 29, 30), newfstatat dir (12), getdents64 `/` (14),
  pipe2 + dup3 (15–17), clock_gettime + nanosleep 30 ms (21/22), getrandom (23), uname (24), set_tid_address / set_robust_list /
  prlimit64 (25–27). `SYSKAT.LNX` prints `syskat ok checks=30` and exits 0 on the host Linux 6.18 kernel (arroyo refuses to stage it
  otherwise).

## M3 — written
- `linuxabi/selfbuild.rs` (new): `tests selfbuild` (registered on the LINUXABI line of `tests.rs`): unlinks a stale
  `<home>/hello.lnx`, runs `TCC.LNX -nostdlib -static -o <home>/hello.lnx /apps/HELLO.C` (30 s), prints the tcc session's own
  `:: LINUXABI:` witness (its first 48 syscalls are traced by the LINUXABI3 M5 tracer: `[linux] sys=<nr> <name> …`), checks the output
  exists, runs it (5 s) and matches `hello from tcc on unaos`, then runs the busybox probe (20 s) for its count and `-ENOSYS` list.
  `<home>` = `/home/<user>/` (the only writable tree for the shim, the editor's rule); without a session user it is `/home/`.
- `arroyo` `build_selfbuild_x86` (called from `build_user_linux_hello_x86`, so every `esp-x86` builds it): copies HELLO.C, builds +
  host-verifies SYSKAT.LNX, fetches/builds TCC.LNX and PROBE.LNX at the pinned commits into `target/selfbuild/` (skipped when the
  binary already sits in `target/`), prints the four sha256s. No egress = absent binaries = `-> SKIP`.
- `builder`: stages `TCC.LNX`, `HELLO.C`, `SYSKAT.LNX`, `PROBE.LNX` into `APPS/` beside LS.LNX (+3.7 MB on the ESP).

**What a metal boot should print** (`./arroyo esp-x86` with the metal shape, desktop shell, logged in as `<user>`):
```
tests linuxabi
:: LINUXABI: path=/apps/HELLO.LNX exit=0 syscalls=2 enosys=[] ms=<n> -> PASS ::
:: LINUXABI-KAT: path=/apps/SYSKAT.LNX checks=30 fail=none exit=0 syscalls=49 enosys=[] -> PASS ::   (49 on the host kernel)
tests selfbuild
[selfbuild] linux /apps/TCC.LNX -nostdlib -static -o /home/<user>/hello.lnx /apps/HELLO.C
[linux] sys=12 brk pid=1 … / sys=158 arch_prctl … / sys=334 rseq … -> -38 / sys=267 readlinkat … -> <len> / … / sys=1 write … -> 959
:: LINUXABI: path=/apps/TCC.LNX exit=0 syscalls=<~28> enosys=[] ms=<n> -> PASS ::
[selfbuild] output /home/<user>/hello.lnx bytes=959
hello from tcc on unaos
:: LINUXABI: path=/home/<user>/hello.lnx exit=0 syscalls=2 enosys=[] ms=<n> -> PASS ::
:: LINUXABI: path=/apps/PROBE.LNX exit=0 syscalls=<~250> enosys=[] ms=<n> -> PASS ::
:: SELFBUILD1: tcc=ok hello=ok probe=busybox probe_syscalls=<~250> probe_missing=[] -> PASS ::
```
Interactive: `linux /apps/TCC.LNX -nostdlib -static -o /home/<user>/hi.lnx /apps/HELLO.C` then `linux /home/<user>/hi.lnx`.
On a failure the KAT's `fail=<id>` names the check (table in `syskat.c`'s header); `tcc=fail(exit=1 out_bytes=0)` with tcc's own
message on `[selfbuild] tcc:` means the write under `/home/<user>/` was refused (no such directory on the volume, or no session user).

## M4 — the honest distance to SH-5
Rung reached (by construction, unflown): a C compiler runs on UnaOS and its output runs. Not reached: a C program that uses a libc
(stage musl's or glibc's crt1.o/libc.a/headers + tcc's libtcc1.a on the volume — data, no kernel work: tcc's own surface does not
change); `make` (fork/exec/wait — answered — and `pselect6`/`statx` likely); a Rust build. rustc needs, in order: **threads**
(clone with CLONE_VM|CLONE_THREAD sharing one PML4, per-thread FS_BASE and kernel stack, CLONE_CHILD_CLEARTID wake), **futex**
(WAIT/WAKE at least, keyed on the shared physical page), statx, ftruncate, flock, eventfd/epoll (or a `-Z threads=1` +
`CARGO_MAKEFLAGS`-free jobserver), signal delivery with rt_sigreturn, an mmap story past 96 MiB per process with lazy (fault-in)
anonymous memory, streamed (not slurped) files, and a static linker on the volume — then a static rustc itself, which needs a musl
host toolchain to build. That is several arcs; threads+futex is the next one and the largest.

## Not done / limits
- NEVER run (R78: no QEMU). The host strace runs are the only executions; the kernel side is compile-proven only.
- `rt_sigaction` reports SIG_DFL even after a handler was "installed" (no per-process table). `getrandom` stays non-cryptographic.
- `probe_missing` lists syscall NUMBERS the shim noted; a deliberate -ENOSYS (rseq, clone3) is not listed.
- `tests linuxabi3` is unchanged (`BUSYBOX.LNX` stays operator-supplied); the glibc busybox is staged as `PROBE.LNX` on purpose.
