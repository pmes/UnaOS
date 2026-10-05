# SELFBUILD4 — the first Rust program on UnaOS, copy-on-write fork, lazy execve (ledger B356)

## Design
**Finding.** After SELFBUILD1–3 (B344, B349, B353) the Linux ABI shim answers threads, futex, epoll, signals, lazy mmap and a
resident budget. That is the surface a static Rust `std` binary needs, and no Rust program had ever run on UnaOS. Two process-memory
debts sat on the road to a toolchain. `fork` copied every resident page eagerly (bounded at 64 MiB), and a pipeline forks only so the
child can `execve`. `execve` read the whole ELF into the heap (128 MiB cap) and copied it in.

**Seam.** `CHARTER: Kernel — driver` (the compatibility box, as SELFBUILD1–3). No handler owns the Linux ABI. There are four new
kernel files under `arch/x86_64/linuxabi/`: `cow.rs` (copy-on-write fork), `exec.rs` (lazy images), `selfbuild4.rs` (`tests
selfbuild4`) and `syskat4.c` on the C side. Existing files get routing only: `vm.rs` (fault hook, `free_frame`, `reprotect`, SIGBUS,
`fixed_ok`, `brk`), `mod.rs` (two store paths, `note_fault`, `run_inner`), `proc.rs` (fork, execve), `elf.rs` (`parse_sized`),
`sys.rs` (poll), `tests.rs` (register). Host side: the new crate `crates/user-linux-rust` (its own workspace root), arroyo
`build_selfbuild4_x86` and two builder rows. There is no new knob. The work rides `UNAOS_LINUXABI=1`, which the x86 metal shape
carries.

**Milestones.** M1 is the probe and its measured surface, plus the answers it lacked. M2 is the owed pieces the probe and the
pipeline hit (COW fork, lazy execve), plus SIGBUS, which is cheap. Each is proven by SYSKAT4 on the host kernel. M3 is `tests
selfbuild4`. M4 is this doc.

**Witness.** `:: SELFBUILD4: rust=ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 fork=ok pipe=ok exec_lazy=1
cow_pages=<n> -> PASS ::`. Before it come `:: LINUXABI-KAT4: … -> PASS ::`, `[selfbuild4] exec: …`, `[selfbuild4] tcc_run=ok` and
`[selfbuild4] kernel: …`.

## M1 — RUST.LNX and its surface
**Toolchain.** `rustup +nightly target add x86_64-unknown-linux-musl` WORKED through the proxy. Nightly `rustc 1.99.0-nightly
(da80ed070 2026-07-14)` now carries the musl std, so the probe is a real **musl** static binary, not glibc. musl and libunwind come
from the target's self-contained objects, and `cc` (gcc 13.3) is the linker. By default the target links **static-PIE**, which the
shim's loader refuses because it has no self-relocation. The crate's `.cargo/config.toml` therefore sets `-C relocation-model=static
-C target-feature=+crt-static`, giving ET_EXEC at 0x400000 with 4 PT_LOADs (R, RX, R, RW) plus a TLS segment. arroyo also asserts
`e_type == 2` before staging. `RUST.LNX` is the stripped binary: 499904 bytes, sha256
`5b6176b85a7f1590360d4ff902e216ee3785d7e69a3723dfcebe08e7a9865035`, identical over two clean builds.

**The probe** (`crates/user-linux-rust/src/main.rs`, ours) does the following: 4 `std::thread`s, each adding 100000 to a
`Mutex<u64>`; a 1000-entry `HashMap`; `fs::read_to_string(argv[1] or /apps/HELLO.C)`; `env::args`; `Instant`; a `panic!` caught by
`catch_unwind` (a quiet hook writes one stderr line); and `available_parallelism`. It prints ONE line,
`rust ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 args=<n> file_bytes=<n> file_lines=<n> cpus=<n> ms=<n>`, and exits 0.

**Measured** (`strace -f` on the host Linux 6.18 kernel, the staged binary): 160 calls and 27 distinct, over 5 tasks.

| syscall | calls | the shim before | now |
|---|---|---|---|
| mmap (TLS block, thread stacks 2 MiB PROT_NONE, MAP_STACK sigaltstacks, malloc) | 25 | lazy VMAs (SELFBUILD3) | same |
| munmap 21 · mprotect 9 (guard pages, stack RW) | 30 | answered | same |
| rt_sigprocmask 20 · rt_sigaction 5 (SIGPIPE ign, SIGSEGV/SIGBUS SA_ONSTACK) | 25 | table + mask (SELFBUILD2) | same |
| sigaltstack (every thread: query, install, disable) | 15 | accepted, old = SS_DISABLE | same (owed: delivery ON the altstack) |
| read 10 · open 4 · fcntl(F_SETFD) 4 · fstat 4 · close 4 · lseek 3 · stat 1 · write 2 | 32 | answered | same |
| clone(VM\|FS\|FILES\|SIGHAND\|THREAD\|SYSVSEM\|SETTLS\|PARENT_SETTID\|CHILD_CLEARTID\|DETACHED) | 4 | threads (SELFBUILD2; DETACHED ignored, as Linux) | same |
| futex (WAIT/WAKE_PRIVATE on the mutex; non-private WAIT on musl's thread-list lock = CLEARTID) | 8 | answered | same |
| gettid 4 · prctl(PR_SET_NAME) 4 · exit 4 · exit_group 1 · set_tid_address 1 · arch_prctl 1 | 15 | answered | same |
| **poll(fds 0-2, events=0, timeout 0)** | 1 | **revents = POLLOUT on the console: answered 3, not 0** | revents masked to `events \| ERR \| HUP` (+NVAL), so 0 |
| getrandom(16, GRND_INSECURE) · sched_getaffinity · brk 2 | 4 | answered | same |
| **mmap(brk start, 1 page, PROT_NONE, MAP_FIXED)** (musl mallocng's guard) | 1 | **-ENOMEM (MAP_FIXED only in the mmap windows)** | allowed in the brk range; `brk` refuses to grow over a mapping |
| open("/proc/self/cgroup") | 1 | -ENOENT | same (std falls back to sched_getaffinity) |

Where the row's expectations differ from what was measured: musl calls **no** `rseq`, `prlimit64` or `statx`. It uses `open`, `stat`
and `fstat`. `clock_gettime` is a vDSO call on the host, and there is no vDSO on UnaOS, so musl falls back to the syscall, which the
shim answers. There is no `mremap` here either: the probe's allocations stay under musl's mmap threshold. On UnaOS,
`sched_getaffinity` answers 1 CPU, so `cpus=1`. The probe needed exactly **two** fixes, and both are now written.

## M2 — the owed pieces that were hit (written; each proven by SYSKAT4 on the host kernel)
- **Copy-on-write fork** (`cow.rs`). The pipeline forks, and this was hit. `fork_cow` shares every resident frame:
  - Private pages become read-only in both spaces, with `COW` set (OS bit 10) and the logical W kept in `CW` (OS bit 11). This
    covers the ELF image, the eager stack, MAP_PRIVATE VMAs and brk.
  - MAP_SHARED pages stay writable in both spaces. They are faulted in first (up to 16384 pages per VMA), so the child really shares
    them. Before this arc the child got a copy.
  - `REFS` counts the holders of each frame. `vm::free_frame` asks `cow::unshare` first.
  - A write fault on a `COW|CW` leaf copies the page when the frame is still shared (`cow_copies`), or else takes the frame back
    writable (`cow_reuses`).
  - Kernel stores go through `page_for_store`: `copy_out`, `writable_range`, `read()` into a shared page.
  - `mprotect` keeps a shared page COW.
  - The parent's CR3 is reloaded after the downgrade. Every session runs on one core, so that reload is the whole TLB shoot-down.
  - The 64 MiB eager bound (`FORK_MAX_PAGES`) is retired.
- **Lazy execve** (`exec.rs`). Every exec hits this. `exec::image` reads the ELF header and the program-header table (one page, or up
  to 64 KiB) and parses against the file size (`elf::parse_sized`). `exec::load` then classifies each image page:
  - a page wholly inside one segment's file bytes becomes a **file VMA** (MAP_PRIVATE at the segment's offset);
  - a page wholly past `p_filesz` becomes an **anonymous VMA**;
  - every other page is **eager**, filled with exactly the segments' file bytes. This covers a segment's unaligned first page, the
    `.data` tail / `.bss` head page, and any page two segments share.

  Until a page is touched, the process's PML4[0] holds the kernel's identity map, which is present but not USER. The fault hook now
  treats that as not-present for a VMA page and flushes after backing it. A USER page already in the window refuses the load. For
  RUST.LNX the numbers are 4 eager pages, about 120 lazy pages in 5 VMAs, and roughly 17 KB read at exec out of 499904. The 128 MiB
  whole-file cap no longer applies to execs.
- **SIGBUS past EOF.** This was not hit, but it costs about 20 lines. `populate` refuses a file page whose offset is at or beyond the
  file's size (`Refuse::Bus`). The fault hook records `pid << 8 | 7`, and `note_fault` finishes the process with status 7 instead of
  11. A kernel copy into such a page answers -EFAULT, as Linux does. The page that holds EOF still reads zero past it.
- **Not written, because nothing hit them:**
  - the shared page cache;
  - page tables from the frame pool. The pool is still unflown, and moving every table onto it would widen the blast radius of a
    pool bug to every process;
  - `mremap`, which is not in the table at all. See the NEXT rung.

**SYSKAT4.LNX** (`crates/user-linux-hello/c/syskat4.c`, freestanding, gcc 13.3 `-static -nostdlib -fno-pie -no-pie -O1 -ffreestanding
-mno-red-zone`, 30688 bytes, sha256 `bf8bab5eded45b10a24846d0b5e5f496fd00434949f7536a58d724d9b5099c42`, identical over two builds)
checks:
- **cow** 1-5: the child sees the parent's 3-page .data, an anon page, a brk page and a stack value; it stores to all four; the
  parent's values are unchanged.
- **cow_kernel** 10-12: `read()` from a pipe into a shared .data page in the child.
- **cow_prot** 15-16: in the child, mprotect RO then RW, then a store.
- **shared_fork** 20-21: MAP_SHARED|ANON written by the child is seen by the parent.
- **exec_lazy** 30-33: `execve("/proc/self/exe", "exec")`. The new image checks its .data pattern on 3 pages and checks that its
  .bss reads zero. The .bss starts mid-page at 0x407180, and the file bytes past `p_filesz` are `.comment`/`.symtab` — the falsifier
  for a naive lazy page.
- **sigbus** 40-41: a 100-byte file mapped as 2 pages; byte 50 reads right and bytes 100..4095 are zero; a child touching page 2 dies
  with WTERMSIG 7.

Host result: `syskat4 ok cow=ok cow_kernel=ok cow_prot=ok shared_fork=ok exec_lazy=ok sigbus=ok checks=15 fail=none`, exit 0.
`strace -f` counts 72 calls: write 32, fork 6, wait4 6, exit_group 6, mmap 4, munmap 4, brk 3, close 3, execve 2, mprotect 2, pipe,
read, openat, unlink. Every one of them was answered by the shim before this arc. The KAT tests semantics, not new numbers.

## M3 — `tests selfbuild4` (written; `selfbuild4.rs`, registered on the LINUXABI line of `tests.rs`)
1. `SYSKAT4.LNX <home>kat4.tmp` (30 s) produces the `:: LINUXABI-KAT4: …` line.
2. `RUST.LNX /apps/HELLO.C` (30 s). Then `[selfbuild4] exec: path=/apps/RUST.LNX file_bytes= read_at_exec= lazy_pages= eager_pages=
   vmas=` is printed. `exec_lazy=1` when lazy pages > 0 and read_at_exec < file_bytes.
3. `PROBE.LNX sh -c 'sh -c "echo a; echo b" | cat'` (argv0 `sh`, 20 s). `fork=ok` needs at least 2 forks; `pipe=ok` needs exactly the
   lines `a`, `b` and exit 0. Host strace shows `pipe2`, then 2 `clone(SIGCHLD)`, then `execve("/proc/self/exe", ["sh","-c",…])` and
   `execve("/proc/self/exe", ["cat"])`, then 3 `wait4` and 2 `rt_sigreturn` (SIGCHLD): 102 calls, 27 distinct, all answered.
4. `PROBE.LNX sh -c "/apps/TCC.LNX -static -o <home>hello4.lnx -x c /apps/PRINTF.C && <home>hello4.lnx"` (90 s) produces
   `[selfbuild4] tcc_run=ok|skip|fail(…)`. ash forks, execs tcc, waits, and then execs the output in place. On the host this is 1782
   calls and 33 distinct, it prints `hello printf from tcc+musl on unaos 42`, and it exits 0. It is skipped without `/apps/LIB`.

The verdict is PASS when rust, fork and pipe are ok, `exec_lazy=1`, `cow_pages > 0` (pages that the pipeline's and tcc line's forks
shared instead of copying), KAT4 is ok, and tcc is ok or skip. Any fail gives FAIL. An absent RUST.LNX gives SKIP.

**Found on the way (fixed): tcc refuses `HELLO.C` / `PRINTF.C`.** tcc types its input by extension, case-sensitively, and the
volume's names are upper-case. `TCC.LNX -nostdlib -static -o h HELLO.C` fails on the host with `tcc: error: HELLO.C: unrecognized
file type`, exit 1. So `tests selfbuild` (SELFBUILD1) and `tests selfbuild3` (SELFBUILD3) would have failed on the metal. Their argv
now carries `-x c` before the source, and so does this test's line. The host proof of the fix is `-x c HELLO.C`, which prints
`hello from tcc on unaos`.

**What a metal boot should print** (`./arroyo esp-x86`, metal shape with `linuxabi`, logged in as `<user>`):
```
tests selfbuild4
:: LINUXABI: path=/apps/SYSKAT4.LNX exit=0 syscalls=<~72> enosys=[] ms=<n> -> PASS ::
:: LINUXABI-KAT4: cow=ok cow_kernel=ok cow_prot=ok shared_fork=ok exec_lazy=ok sigbus=ok fail=none exit=0 -> PASS ::
[selfbuild4] linux /apps/RUST.LNX /apps/HELLO.C
[linuxabi] thread pid=<p> tid=<t> key=+1 …            (x4)
[selfbuild4] exec: path=/apps/RUST.LNX file_bytes=499904 read_at_exec=16874 lazy_pages=119 eager_pages=5 vmas=4
[selfbuild4] rust: rust: panic raised (expected; caught)
[selfbuild4] rust: rust ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 args=2 file_bytes=<n> file_lines=23 cpus=1 ms=<n>
:: LINUXABI: path=/apps/RUST.LNX exit=0 syscalls=<~160> enosys=[] ms=<n> -> PASS ::
[selfbuild4] linux /apps/PROBE.LNX sh -c 'sh -c "echo a; echo b" | cat'
[selfbuild4] pipe: a
[selfbuild4] pipe: b
:: LINUXABI: path=/apps/PROBE.LNX exit=0 syscalls=<~102> enosys=[] ms=<n> -> PASS ::
[selfbuild4] linux /apps/PROBE.LNX sh -c "/apps/TCC.LNX -static -o /home/<user>/hello4.lnx -x c /apps/PRINTF.C && /home/<user>/hello4.lnx"
[selfbuild4] tcc: hello printf from tcc+musl on unaos 42
:: LINUXABI: path=/apps/PROBE.LNX exit=0 syscalls=<~1780> enosys=[] ms=<n> -> PASS ::
[selfbuild4] tcc_run=ok
[selfbuild4] kernel: cow_forks=3 cow_shared=<n> cow_copies=<small> cow_reuses=<n> lazy_execs=<n> faults_file=<n> faults_anon=<n> peak_resident=<n>
:: SELFBUILD4: rust=ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 fork=ok pipe=ok exec_lazy=1 cow_pages=<n> -> PASS ::
```
**Reading a failure:**
- `rust=fail(exit=…)` with a `[linuxabi] fault va=… -> NoVma|Prot` line points at the lazy image or a thread stack.
- `rust=fail(load)` with `not ET_EXEC` means a PIE build was staged.
- `fork=fail(forks=0)` means busybox ran nothing.
- `pipe=fail(lines=n)`: compare the `pipe:` lines.
- KAT4 `fail=<id>`: look the id up in the table in the C header (32 = `.bss` junk from the file, 21 = MAP_SHARED copied, 41 = SIGBUS
  reported as 11).

## M4 — the honest NEXT rung (what a static rustc / cargo would still lack)
Reached by construction (unflown): a musl-static Rust `std` program with threads, locks, a hasher, files and unwinding. On the
process side, a COW fork + pipe + exec pipeline and a compile-and-run line. Missing for `rustc` on UnaOS, in the order it would bite:
1. **`mremap`** (25), which is not answered at all. musl's `realloc` of any block past its mmap threshold (about 128 KiB) calls
   `mremap(MREMAP_MAYMOVE)`, so any Rust program growing a `Vec`/`String` past that gets -ENOSYS, and musl then falls back to
   malloc + copy + free. That fallback is correct but slow, and the gap is noted in `enosys=[25]`. It is the first thing to write: a
   move of leaves between VMAs.
2. **A rustc that is static.** The host rustc is a dynamic program over `librustc_driver-*.so` and LLVM, about 100 MB+. A static build
   means building rustc for the `x86_64-unknown-linux-musl` host with LLVM linked in, which is a several-hour bootstrap that this
   container cannot run inside its disk allowance. A static rustc **cannot `dlopen` proc-macro crates**, so either the shim grows a
   dynamic loader path (PT_INTERP / `ld-musl`, ET_DYN at a base, PROT_EXEC file mappings — W^X holds) or the build avoids
   proc-macros entirely (serde-derive, thiserror and the rest). That is the hard wall.
3. **A linker rustc can drive.** rustc execs `cc` → `ld`. The realistic path is `-C linker=rust-lld -C linker-flavor=ld.lld` plus the
   target's self-contained crt objects (a static `rust-lld` on the volume, about 70 MB). tcc cannot link Rust objects.
4. **The sysroot on the volume.** The musl `libstd`/`libcore` rlibs are about 60–100 MB, plus `/apps/LIB`-style placement. Mapping
   rlibs MAP_PRIVATE works now (SELFBUILD3). The 128 MiB exec cap is gone (lazy exec), but the file VMAs fault through the VFS one
   page at a time with a `stat` each, so they are slow.
5. **Memory and time.** A small crate puts rustc at roughly 300–600 MB peak resident, within the heap share + pool budget only if the
   pool (still unflown) works. All threads are pinned to one core, so `-Z threads`/codegen units give concurrency, not speed.
6. **Signals.** SA_ONSTACK delivery on the sigaltstack (std's stack-overflow detector, rustc's `stacker`), per-thread masks, and
   delivery at a fault rather than only at a syscall boundary.
7. **cargo** additionally needs a vendored, offline registry, `flock` (done), `statx` (done), `getdents64` over deep trees (done), and
   the jobserver over pipes (done). There is no network.

The cheaper NEXT rung is to run a **cross-compiled** program built by cargo on the host (which this arc does) and grow from there:
`mremap`, then a static `rust-lld` running on UnaOS to link host-made objects, then a static rustc.

## Host proof (R78: the only executions)
- `RUST.LNX HELLO.C` on Linux 6.18: `rust ok threads=4 counter=400000 hashmap=ok fs=ok panic_caught=1 args=2 file_bytes=897
  file_lines=23 cpus=4 ms=10`, exit 0. The strace table is in M1.
- `SYSKAT4.LNX`: `syskat4 ok … checks=15 fail=none`, exit 0.
- The pipeline (argv0 `sh`) printed `a` and `b` and exited 0. The tcc line printed `hello printf from tcc+musl on unaos 42` and
  exited 0.
- The whole `build_selfbuild_x86` (+ `build_selfbuild4_x86`) ran end to end in the worktree, twice, with identical sha256s.

## Not done / limits
- NEVER run (R76/R78: no QEMU). The kernel side is compile-proven only, on the x86 metal shape + linuxabi + selfdiag, ahciroot, btc,
  and on the aarch64 leg.
- COW: a MAP_SHARED VMA larger than 16384 pages is not pre-faulted at fork, so its untouched pages are private to whoever touches
  them first. Futexes shared across fork are still per address space.
- Lazy exec: each file-page fault `stat`s the file (for SIGBUS). A binary rewritten while it runs is seen half old and half new, as
  with Linux's ETXTBSY-less case.
- Owed (B353's list, unchanged): the shared page cache, page tables from the pool, `mremap`, sigaltstack delivery.
