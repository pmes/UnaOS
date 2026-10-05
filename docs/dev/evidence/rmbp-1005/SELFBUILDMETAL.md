# SELFBUILDMETAL — why every self-build program failed on the metal in flight 22 (ledger B367)

## Design
**Finding.** In flight 22 SYSKAT2, SYSKAT3, SYSKAT4, SYSKAT5 and RUST.LNX all failed on the rMBP. The row's premise was "they
passed in QEMU". That premise is wrong: SELFBUILD2–5 were never run on UnaOS before flight 22. Each of those docs says
"NEVER run (R78: no QEMU)". Their only runs were on the host Linux kernel, so flight 22 was the first time UnaOS executed them.

All five failures have **one cause, and it is not specific to the metal**. On every SYSCALL return, UnaOS's x86 SYSCALL stub
(`arch/x86_64/syscall.rs`, `unaos_syscall_entry`) zeroed `rdi rsi rdx r8 r9 r10`. This was the U1b B1 "scrub". Linux keeps every
GPR across a syscall except `rax rcx r11`. gcc and musl both depend on that: they list a syscall's argument registers as inputs
only, and keep using them after the `syscall` instruction. With those registers zeroed:
- the timeout pointer became NULL (SYSKAT2),
- the fd became 0 (SYSKAT3),
- a buffer pointer became NULL (SYSKAT5),
- musl's `struct sigaction *` became NULL, so it read `sa_flags` at +0x88 (RUST.LNX),
- every fork group misread its own state (SYSKAT4).

**Seam.** `CHARTER: Kernel — driver`, the Linux-ABI compatibility box (as SELFBUILD1–6). No new kernel file.
- The stub (`syscall.rs`) now pushes the six argument registers and pops them back. Restoring the task's own values leaks nothing,
  so U1b B1's goal (no kernel leftover reaches ring 3) still holds.
- `linuxabi/mod.rs` gets `FRAME_ARGS`, `frame_args` and `set_frame_args`, plus the fault line.
- `proc.rs`: `execve` zeroes the six registers, and a `fork` child inherits them.
- `signal.rs`: `rt_sigreturn` reloads them from the `ucontext`.
- `interrupts.rs`: one call that parks the faulting rip for the fault line.

There is no new knob. Everything rides `UNAOS_LINUXABI=1`. The stub is shared with native ring 3, which loses nothing: every
native syscall wrapper already lists those registers as clobbered.

**Milestones.**
- M1: the fault table and the per-failure shape analysis (§1, §2).
- M2: the reproduction (§3), the fix, and the fault line (§4).
- M3: this doc's table of fixed / instrumented / still open (§5).

**Witness (boot 24).** Each program's own PASS line:
- `:: LINUXABI-KAT2: … -> PASS ::` / `:: SELFBUILD2: kat2=ok … -> PASS ::`
- `:: LINUXABI-KAT3: … -> PASS ::`
- `:: LINUXABI-KAT4: cow=ok cow_kernel=ok cow_prot=ok shared_fork=ok exec_lazy=ok sigbus=ok … -> PASS ::`
- `:: SELFBUILD4: rust=ok … ::`
- `:: SELFBUILD5: mremap=ok altstack=ok … ::`

If any of them still fails, the new line `[linuxabi] fault pid= vec= err= rip= cr2= vma= rip_vma= path= syscall= nsys=` names
the cause.

**Stays owed.** The metal run itself, which is boot 24 (R78). The QEMU metal-shape leg the row allowed was not run (§3 says why).
The stale "the sysret tail scrubs rdi/rsi/rdx/r8-r10" comments next to the native U-fixtures in `syscall.rs` were left alone.
They describe a constraint that no longer exists, and no fixture depends on it.

## §1 — The flight-22 fault table (`docs/dev/evidence/rmbp-0915/flight22/f22-boots.log`, read with awk)

| program | exit | syscalls | ms | blocked in / fault | the last syscalls on the wire |
|---|---|---|---|---|---|
| SYSKAT2.LNX | TIMEOUT | 27 | 20050 | `futex(202)`: `futex a0=0x40501c a1=0x89 a2=0x5 -> blocks`, never returns | `futex(fw, WAIT, 4)` → -11 (check 7), `futex(fw, WAIT, 5, rel 30 ms)` → -110 (check 8), `futex(fw, WAKE)` → 0 (check 9), `clock_gettime`, then FUTEX_WAIT_BITSET (check 10) |
| SYSKAT3.LNX | TIMEOUT | 5 | 30050 | `pread64(fd=0)`: `pread64 a0=0x0 a1=0x414020 a2=0x381 -> blocks` | `openat` → 3, `fstat(3)`, `mmap(0, 0x381, R, PRIVATE, 3)`, `pread64(3, &last, 1)` → 1, then `pread64(0, pbuf, 0x381)` |
| SYSKAT4.LNX | 1 | 71 | 1042 | `cow=fail(1) cow_kernel=fail(255) cow_prot=fail(15) shared_fork=fail(21) exec_lazy=fail(255) sigbus=fail(41)`. Also `RING-3 FAULT vec=14 err=0x4 rip=0x401f88 cr2=0x10040004008`, the sigbus group's EXPECTED child fault: `[linuxabi] fault … -> Bus (SIGBUS: file page past EOF)` | `execve pid=5 path=/apps/SYSKAT4.LNX` (exec_lazy) |
| RUST.LNX | FAULT | 11 | 1372 | `RING-3 FAULT vec=14 err=0x5 rip=0x4514c8 cr2=0x88` (user-mode read, page not present) | `[selfbuild4] exec: file_bytes=495936 read_at_exec=15570 lazy_pages=118 eager_pages=5 vmas=4` |
| SYSKAT5.LNX | FAULT | 3 | 142 | `RING-3 FAULT vec=14 err=0x5 rip=0x401180 cr2=0x0` | `mmap` (4 pages), `munmap(a+2 pages)`, `mremap(a, 2→4 pages)`, then the fault |

`SELFBUILD5: mremap=fail(…cr2=0x0) altstack=fail(…cr2=0x0)` is that one SYSKAT5 run counted twice. Both groups belong to the
same process, which died in group 1. Every fault is a **ring-3** fault: rip is in the program, `task 'linuxabi' KILLED`. None is
a NULL dereference inside the shim. HELLO.LNX on the same boot: `exit=0 syscalls=2 -> PASS`.

## §2 — M1: the shape question, failure by failure

The row asked what differs between the QEMU/host gates and the metal. The answer: there was no QEMU gate, and the host gate is
Linux. So the question became "what differs between Linux and UnaOS on a path every program takes". Each hypothesis the row named
was checked against the code and the binaries. The SYSKAT binaries the host builds today have the flight's entry points
(`0x401059 / 0x401000 / 0x401000 / 0x401008`), and SYSKAT2's and SYSKAT5's sha256 match the ones SELFBUILD2/5 recorded.
- **SYSKAT2, futex vs the metal tick: refuted.** `clock_gettime` and the futex deadline read the same clock (`arch::ms`,
  `sys.rs now_ts` / `thread.rs wait`). The relative wait timed out correctly in 20–5000 ms (check 8). The disassembly of check 10
  (gcc 13.3, `-O1`) is:
  ```
  4018b9: mov $0xe4,%eax ; mov $1,%edi ; syscall   # clock_gettime(1, abs_)  rsi = &abs_
  ...
  4018ee: mov %rsi,%r10                             # timeout = rsi, assumed to survive the syscall
  401910: syscall                                   # futex(fw, WAIT_BITSET|PRIVATE, 5, r10, 0, -1)
  ```
  The stub zeroed rsi, so the wait had **no timeout** and blocked until the harness gave up at 20 s. It does this on any machine.
- **SYSKAT3, pread64(fd=0) on the console door: a symptom, not a door bug.** The program never meant to read fd 0:
  ```
  4014ec: mov %rbp,%rdi ; syscall                   # pread64(fd, &last, 1, size-1)
  401557: mov $0x11,%eax ; mov $0x414020,%esi ; syscall   # pread64(rdi = fd assumed kept, pbuf, n0, 0)
  ```
  With rdi zeroed, the second call read fd 0, the console, which correctly blocks a Linux read. The shim's RETRY loop yields, so
  the kernel kept running (later tests ran), and the `tests` deadline ended it at 30 s. The lazy file fault (`m[size-1]`) worked
  (`[selfbuild3] kernel: faults_file=2`).
- **RUST.LNX, the NULL dereference at 0x88: musl's `struct sigaction`, not a shim struct.** The fault is a ring-3 fault, so the
  thing at offset 0x88 is in the program. In the host rebuild of RUST.LNX the same fault lands in `__libc_sigaction`:
  ```
  452a1c: syscall                                   # rt_sigprocmask(…)  (the one-time unmask)
  452a28: mov 0x88(%r8),%eax                        # sa->sa_flags: sa kept in r8 across the syscall
  ```
  `0x88` = 8 (`sa_handler`) + 128 (`sa_mask`), which is musl's `sa_flags` offset. r8 was zeroed, so the read was
  `NULL+0x88 = cr2 0x88`.
- **SYSKAT5, mremap cr2=0x0.** After `mremap(a, …) == a`, gcc calls `stamped(a, …)` with `a` still in rdi from the syscall
  (`4016d8: mov %rbx,%rdi ; syscall … 40172d: call stamped`). rdi was zeroed, so `cmp %rdx,(%rdi)` faulted at cr2 0. The
  altstack group never ran.
- **SYSKAT4, exec_lazy/sigbus = 255 "identically": not the file-backed fault handler.** Under the scrub, every fork group's parent
  and child read zeroed argument registers after `fork`/`wait4`/`mmap`. The SDHC lazy path did its job: the sigbus child took the
  expected `Bus` refusal. §3 reproduces the six verdicts **byte for byte** off the metal.
- **Why the rips do not match the host rebuild (0x401180 vs 0x40113b, 0x4514c8 vs 0x452a28, 0x401f88 vs 0x401f48).** The
  flight's SYSKAT5/RUST.LNX were built by the image builder's toolchain, not this container's. The fault addresses, the syscall
  counts (RUST 11 vs 12) and the argument values all agree. The instructions are the same. Only their placement differs.

## §3 — M2: the reproduction (fixed / reproduced / not reproduced)

**The QEMU metal-shape leg was not run.** It would have to:
- build the full x86 image with the selfbuild fixtures, which means fetching musl and tinycc and attempting the pinned LLVM build
  for LLD.LNX,
- boot it under `arroyo test`,
- type `tests selfbuild2..5` into the guest through a typist.

That does not fit the shared disk: 8.9 GB free across 22 executors when this leg was due. R78 still names the metal as the proof.

What stood in for it is a cheaper reproduction of exactly the difference that mattered. The tool is
`docs/dev/evidence/rmbp-1005/selfbuildmetal/scrubtrace.c`, a ptrace harness. It runs a static Linux program on the host kernel
and, at every syscall exit, either zeroes `rdi rsi rdx r8 r9 r10` (`scrub`, the old UnaOS stub) or leaves them alone (`keep`,
Linux and the fixed stub). It is pinned to one CPU with `taskset -c 0`, as the shim pins a process's threads to one core. The
binaries are the host-built SYSKATs (`gcc -static -nostdlib -fno-pie -no-pie -O1 -ffreestanding -fno-stack-protector -fno-builtin
-mno-red-zone`, arroyo's line) and RUST.LNX (`crates/user-linux-rust`, `x86_64-unknown-linux-musl`, stripped).

| program | flight 22 (metal) | `scrub` (old stub emulated) | `keep` (Linux = fixed stub) |
|---|---|---|---|
| SYSKAT2 | TIMEOUT, `futex a1=0x89 a2=0x5 -> blocks` | **reproduced**: `futex a0=0x40501c a1=0x89 a2=0x5 a3=0` (NULL timeout), hangs, 4 of 4 runs | `syskat2 ok threads=4 counter=400000 futex_waits=5 epoll=ok statx=ok sigreturn=ok checks=26` |
| SYSKAT3 | TIMEOUT, `pread64 a0=0x0 a1=0x414020 a2=0x381` | **reproduced**: `pread64 a0=0 a1=0x414020 a2=0x381` (same args; a host pipe returns EOF where the UnaOS console blocks) | `syskat3 ok mmap=ok shared=ok anon_mib=256 resident_pages=256 … checks=37 fail=none` |
| SYSKAT4 | `cow=fail(1) cow_kernel=fail(255) cow_prot=fail(15) shared_fork=fail(21) exec_lazy=fail(255) sigbus=fail(41)` | **reproduced byte for byte**: `syskat4 fail cow=fail(1) cow_kernel=fail(255) cow_prot=fail(15) shared_fork=fail(21) exec_lazy=fail(255) sigbus=fail(41) checks=11 fail=1` | `syskat4 ok cow=ok … exec_lazy=ok sigbus=ok checks=15 fail=none` |
| RUST.LNX | FAULT vec=14 cr2=0x88 after 11 syscalls | **reproduced**: SIGSEGV `addr=0x88` after 12 syscalls (`__libc_sigaction`) | `rust ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 …` |
| SYSKAT5 | FAULT vec=14 cr2=0x0 after 3 syscalls | **reproduced**: SIGSEGV `addr=0` after 4 syscalls (`stamped`, rdi = 0) | `syskat5 ok grow=ok move=ok fixed=ok dontunmap=ok errors=ok altstack=ok small=ok checks=42 fail=none` |

**Not reproduced: nothing.** Every flight-22 self-build failure reproduces off the metal from the scrub alone. No timer, SDHC,
memory-size or SMP difference is needed to explain any of them.

## §4 — The fix and the fault line

**The fix (`syscall.rs`, the stub).**
- Entry: after the six callee-saved pushes, `push rdi, rsi, rdx, r10, r8, r9`. This happens before the C-ABI shuffle, so they are
  the user's values. They land at `ktop-88 … ktop-128`, below the frame every existing reader indexes (`ktop-8 … ktop-80`), so
  fork, clone, signals, sigaltstack and execve keep their offsets. Six pushes keep `rsp` 16-aligned at the `call`.
- Exit: the U1b B2 canonical-rcx guard moves up. It runs right after the call, on the saved rcx slot (`[rsp+96]` = `ktop-32`),
  while rcx/rdx are still scratch. Then come the six pops, then the old pops. The six `xor` lines are gone.
- U1b B1's property is kept by different means. No caller-saved GPR carries a dispatcher leftover to ring 3: each one carries the
  value the task itself put there.

**The Linux layer follows the frame (`linuxabi/`).**
- `FRAME_ARGS` / `frame_args` / `set_frame_args` live in `mod.rs`.
- `execve` zeroes the six registers. The new image must not inherit the caller's; glibc's `_start` takes rdx as `rtld_fini`.
- A `fork` child gets the parent's six, where it used to get zeros. A thread already received its creator's.
- `rt_sigreturn` reloads them from the `ucontext`: `mc[8] mc[9] mc[12] mc[2] mc[0] mc[1]` = rdi rsi rdx r10 r8 r9, as the delivery
  wrote them.

**The fault line (M2's instrumentation, for anything boot 24 still shows).** It prints one line per fatal Linux-task fault, after
the `RING-3 FAULT` line:
```
[linuxabi] fault pid=<n> vec=<v> err=<e> rip=<rip> cr2=<cr2> vma=<lo>-<hi> <rwx><s|p>|none rip_vma=<…> path=<mapped file or exe> syscall=<nr> <name> nsys=<n>
```
How it is produced:
- `interrupts::ring3_fault_kill` parks the rip (`linuxabi::note_fault_rip`).
- `linuxabi::note_fault` prints the line. It only tries the process lock, never waits for it, and prints `vma=locked` when a
  thread holds it.
- The last syscall comes from `LAST_SYS`, which `dispatch` stores on entry.

**Gates (direct legs, this branch).**
- charter-check: `bash /home/user/UnaOS/unaos/scripts/charter-check.sh …/unaos` exit 0.
- x86 metal shape + linuxabi + selfdiag,ahciroot,btc: `cargo +nightly check --release --target ../../x86_64-unaos.json …
  --features "wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,facet,beam,sdw,sdwrite,sdhcblk,selfhost,
  linuxabi,ahci,unafs,busreg,lumen,netring3,prefs_reset,census,installdemo,instgui,witness,selfdiag,ahciroot,btc"` exit 0.
- aarch64: `--features "login,loginst,virt_el0"` exit 0 (user_blob.bin head `28 00 80 d2`).
- builder: `cargo check --release` exit 0.
- Host: the five programs green under `keep` (§3). The arroyo host proofs are unchanged.

## §5 — M3: fixed / instrumented / still open

| item | state |
|---|---|
| SYSKAT2 futex BITSET waits forever (NULL timeout) | **fixed**: stub restores rsi; reproduced off-metal and green under Linux semantics |
| SYSKAT3 pread64(fd=0) | **fixed**: stub restores rdi; the console door blocking a real fd-0 read is correct and is not changed |
| SYSKAT4 cow/cow_kernel/cow_prot/shared_fork/exec_lazy/sigbus | **fixed**: the six verdicts reproduce byte for byte from the scrub alone; a fork child now also inherits rdi..r9 |
| RUST.LNX cr2=0x88 | **fixed**: musl `sigaction` `sa_flags` through a zeroed r8 |
| SYSKAT5 mremap/altstack cr2=0x0 | **fixed**: rdi zeroed after `mremap`; altstack never ran (same process) |
| a fatal Linux-task fault on metal that the above does not explain | **instrumented**: `[linuxabi] fault … rip= cr2= vma= rip_vma= path= syscall= nsys=` |
| the proof on the rMBP | **open**: boot 24 (R78); the QEMU metal-shape leg was not run (§3) |
| stale "the sysret tail scrubs rdi/rsi/rdx/r8-r10" comments beside the native U-fixtures in `syscall.rs` | **open** (cosmetic) |
