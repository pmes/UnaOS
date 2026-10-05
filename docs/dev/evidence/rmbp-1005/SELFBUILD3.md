# SELFBUILD3 — lazy file-backed mmap, a resident-page budget and a libc on the volume (ledger B353)

## Design
**Finding.** After SELFBUILD2 (B349) the Linux ABI shim answers the threaded surface of a modern toolchain but kept the two
ceilings SELFBUILD1 measured: every `open` read the WHOLE file into the descriptor (`Kind::File { data }`, 128 MiB cap) and
`mmap` was EAGER — anonymous memory allocated and zeroed at map time, a file mapping a copy of the slurped bytes, MAP_SHARED
writable refused (`-EACCES`, "no write-back in rung 1"), all under a per-process cap of 24576 frames (96 MiB) drawn from the
256 MiB kernel heap. The kernel's #PF path had no hook: any ring-3 page fault killed the task. rustc, cargo, ld.lld and every
libc's malloc reserve far more than they touch and mmap their inputs.

**Seam.** `CHARTER: Kernel — driver` (the compatibility box, as LINUXABI3's `fpu.rs` and SELFBUILD1/2). No handler owns the Linux
ABI. New kernel files under `arch/x86_64/linuxabi/`: `vm.rs` (VMAs, the #PF hook, the memory syscalls, the resident budget, the
user frame pool) and `selfbuild3.rs` (`tests selfbuild3`). Existing files get routing: `AddrSpace` (mod.rs) keeps its API, its
tables/frames bookkeeping moves into a `RefCell` so a kernel `copy_in`/`copy_out` through `&self` can back a lazy page; one hook in
`interrupts.rs::page_fault_handler` (ring-3 branch, `#[cfg(feature = "linuxabi")]`); two read-only accessors at the tail of
`memory.rs` (Usable RAM, the heap window); `Kind::File` loses `data`. Host side: `arroyo` `build_selfbuild_x86` builds musl and
re-configures tcc onto `/apps/LIB`; `builder` stages `APPS/LIB`. No new knob — rides `UNAOS_LINUXABI=1` (the x86 metal shape
carries `linuxabi`).

**Milestones.** M1 lazy mmap (anonymous zero-fill and file pages on first touch through the VFS, MAP_PRIVATE/MAP_SHARED, PROT_*,
munmap/mprotect/madvise/msync/mincore, write-back on msync/munmap/execve/exit), lazy brk + stack, `open` no longer slurps.
M2 the cap becomes a RESIDENT budget, raised to what the machine has, printed on refusal. M3 musl staged as DATA under `/apps/LIB`,
TCC.LNX configured to find it. M4 `tests selfbuild3` + `SYSKAT3.LNX` + this doc.

**Witness.** `:: SELFBUILD3: mmap=ok shared=ok anon_mib=256 resident_pages=256 libc=musl tcc_libc=ok -> PASS ::`, with
`:: LINUXABI-KAT3: … -> PASS ::` and `[selfbuild3] kernel: peak_resident=… pool_peak=… limit_pages=…` before it.

## M1 — lazy memory (written; `vm.rs`, routing in `mod.rs`/`sys.rs`/`sys3.rs`/`fd.rs`/`elf.rs`/`interrupts.rs`)
- **VMAs.** `AddrSpace.vm: vm::Vm` — a sorted, non-overlapping `Vec<Vma>` (`start`, `end`, `r/w/x`, `shared`, `file: Option<Arc<String>>`
  (the VFS path), `foff`). `mmap` records a VMA and allocates nothing. Split on partial munmap/mprotect (file offset carried).
- **The fault hook.** `page_fault_handler`, ring-3 branch, after the existing `swapgs`: `linuxabi::vm::fault(cr2, err)`. For a Linux
  task (`is_linux_task`, process by CR3) it `try_lock`s the process (contended = another thread of the process is inside a syscall:
  return and re-execute; the timer lets the holder finish), and for a not-present fault inside a VMA whose protection allows the
  access it takes ONE frame, fills it (zero, or `mt.read(path, foff + (va - start), 4096)` — UnaFS or FAT, through the block cache
  under them — BEFORE the PTE goes live, so no sibling thread sees a half-read page), installs the PTE with the VMA's bits, then
  `swapgs` back and `iretq`: the instruction re-executes. A present-page protection fault is resolved only if the PTE already
  allows it (stale TLB). Anything else falls through to `ring3_fault_kill` exactly as before, after one
  `[linuxabi] fault va=… err=… -> NoVma|Prot (SIGSEGV)` line. Same context as a syscall (task kernel stack, IF=0 as SFMASK
  gives the SYSCALL path), so the VFS read is legal there for the same reason it is in `read()`.
- **Kernel copies.** `copy_in`/`copy_out`/`writable_range`/`read_cstr` back an unbacked VMA page on the way (`fault_in`), so a
  `read()` into fresh anonymous memory, a `futex` word in a lazy page or a `stat` buffer in brk all work; `copy_out` sets the PTE
  dirty bit so a kernel store into a MAP_SHARED page is written back too.
- **MAP_SHARED** file mappings need the fd open for writing (`-EACCES` otherwise, as Linux); dirty pages (PTE D bit) are written
  back, clamped to the file's current size, on `msync` (dirty bits cleared), `munmap`, `MAP_FIXED` over them, `madvise(DONTNEED)`,
  `execve` (`AddrSpace::reset`) and process exit (`free_frames`, run by `proc::gc` / the session's end). MAP_PRIVATE file pages are
  private copies, never written back. Anonymous MAP_SHARED = private (no fork sharing, as before).
- **PROT_NONE is real.** An unbacked page of a PROT_NONE VMA is refused at fault time; a RESIDENT page made PROT_NONE becomes a
  not-present leaf that keeps its frame and OS bit 9 (`vm::SWN`), so `mprotect` back restores the contents (KAT 33). `munmap`,
  `fork` and `free_frames` see those leaves. Before SELFBUILD3 `PROT_NONE` on a resident page left it readable.
- **Calls.** `mmap` (9: MAP_FIXED, MAP_FIXED_NOREPLACE `-EEXIST`, MAP_POPULATE best-effort, W+X still `-EACCES`), `mprotect` (10: every
  page in a VMA or resident, else `-ENOMEM`; SYSKAT check 4 unchanged), `munmap` (11: write back, drop, forget; unmapped ranges are
  fine), `brk` (12), `msync` (26, new), `mincore` (27, new: bit 0 = resident), `madvise` (28: DONTNEED/FREE drop resident pages so they
  fault back zero / re-read; every other advice accepted). Leaf walks (`AddrSpace::leaf_range`) skip absent tables wholesale, so a
  64 GiB PROT_NONE reservation costs nothing to munmap.
- **Layout.** brk = a lazy anonymous VMA from `BRK_BASE` up to `MMAP_BASE` (`BRK_MAX` 64 MiB -> 1008 MiB); the main stack = the 128
  eager pages under `STACK_TOP` plus a lazy VMA down to 8 MiB (`vm::STACK_LAZY_LO`); `mmap` is FIRST-FIT over two windows — the
  original 768 MiB `MMAP_BASE..MMAP_LIMIT` and a new 448 GiB `MMAP2_BASE..MMAP2_LIMIT` above the stack and the signal trampoline,
  all inside PML4[2] (never inherited from the kernel) — so munmap'd space is reused (the old bump pointer `mmap_next` never
  reclaimed) and nothing can land in PML4[0], where the ELF window (< 16 MiB) and the kernel's identity map live. `MAP_FIXED` is
  honoured only inside those windows.
- **Files no longer slurped.** `Kind::File { path, pos, read, write }`: `read`/`pread`/`readv`/`sendfile` call `mt.read(path, off, n)`
  per call (clamped to the size `mt.stat` reports now), `write` goes through `mt.write` as before, `fstat`/`statx`/`lseek(SEEK_END)`/
  `O_APPEND` ask the VFS for the current size, so every description of a file sees the same bytes (SELFBUILD2's "shared descriptions
  see their own copy" is gone). `ftruncate`/`fallocate`/`truncate` read the kept prefix only when a backend needs a rewrite.

## M2 — the resident budget (written; `vm.rs`, `memory.rs` tail)
- A process may hold `vm::limit_pages()` = `HEAP_SHARE` (24576 = the old 96 MiB cap, still served from the kernel heap, so every
  workload that ran before allocates exactly as before) + the user frame POOL: every page of the UEFI map's Usable RAM at/above
  16 MiB, below 1 TiB (PML4[2] would alias the process window), outside the heap window, kept only where the identity map really
  reaches it (`translate(pa) == pa`, probed every 2 MiB and at each region's last page; the first failure truncates the region).
  The pool is a bump pointer over up to 32 regions plus an intrusive free list (the next pointer lives in the free frame); frames
  are zeroed on hand-out; `free_frame` returns a frame to whichever allocator owns it. Only RESIDENT pages count: 256 MiB mapped
  and one page per MiB touched is 256 pages, not 65536.
- Refusal (budget reached or pool exhausted) prints once per address space:
  `[linuxabi] resident limit: resident=<n> pages limit=<n> pages (<MiB> MiB = heap share 24576 + pool <n>; pool in use <n>) va=… -> refused`;
  a fault then kills the task (the RING-3 FAULT line follows), a kernel copy answers `-EFAULT`, `mmap` itself never fails for size.
  The first pool frame prints `[linuxabi] frame pool: first frame … (pool <n> pages = <MiB> MiB past the heap)`. `sysinfo` reports
  the budget. Page TABLES still come from the heap.

## M3 — musl on the volume (written; `arroyo` `build_selfbuild_x86`, `builder`)
- **musl 1.2.5**, MIT, commit `0784374d561435f7c787a555aeab8ede699ed298` (tag v1.2.5; `git archive --prefix=musl-1.2.5/` tarball sha256
  `0c04b18aa4021c22e5d01287a3266c92b70219bd5418545eb32abf7477449832`), fetched from the GitHub mirror `github.com/kraj/musl` —
  `git.musl-libc.org` and `musl.libc.org` were refused by the egress proxy (403); the commit id pins the bytes. `./configure
  --disable-shared CC=gcc CFLAGS=-O2`, gcc 13.3. Staged as DATA in `target/LIB` -> `APPS/LIB`: `crt1.o` (sha256 `e5736a6d…`),
  `crti.o` (`00741e79…`), `crtn.o` (`8ed7acba…`), `libc.a` 2763754 B (`68e8773bff1096fc35bf32024413951328dd144d72d2a1401543f58766fe55f3`),
  `include/` (1.2 MB, long names via VFAT LFN), `MUSL.TXT`. Peter's word on the fetch is owed (the row says so).
- **tcc** (same tinycc commit as SELFBUILD1, `43c7708b…`) is now configured onto the volume: `--tccdir=/apps/LIB/tcc
  --crtprefix={B}/.. --libpaths={B}:{B}/.. --sysincludepaths={B}/../include:{B}/include --config-musl`; `{B}` is tcc's lib dir, so on
  UnaOS a bare `TCC.LNX -static -o prog prog.c` finds crt1.o/libc.a in `/apps/LIB`, musl's headers, then tcc's own headers and
  `libtcc1.a` (49 KB, sha256 `20503cab15cca60fb68836ba56a3bcff7356d6d56a9dc379b5c8f0aa5936ee13`, compiled by that tcc against the staged
  tree) in `/apps/LIB/tcc`; on the host `-B target/LIB/tcc` points the SAME binary at the staged tree. `TCC.LNX` 1226992 B, sha256
  `b1f77349d66fe856bd55a888f0d4681c05af992e05d278f080762a4eda873de6` (two builds identical; SELFBUILD1's was `c1a16241…` — only the
  configured paths differ; `tests selfbuild` passes `-nostdlib` so its leg is unaffected). Rebuilt when `LIB` lacks `libtcc1.a`.
- arroyo stages `LIB` only after the host proof below passes; no egress = no `LIB` = `libc=none tcc_libc=skip`. `+3.9 MB` on the ESP.

## M4 — `tests selfbuild3` (written; `selfbuild3.rs`, `crates/user-linux-hello/c/syskat3.c`, `printf.c`)
1. `SYSKAT3.LNX /apps/HELLO.C <size the VFS reports> <home>kat3.tmp` (30 s): freestanding, every group runs, one verdict line
   (ids in the C header): **mmap** 1-6 (HELLO.C MAP_PRIVATE, its LAST byte through the mapping == pread, first page == pread,
   private RW store not in the file, a map past EOF), **shared** 10-14 (two MAP_SHARED pages of a scratch file, pread sees them after
   msync and after munmap, re-mapped RO), **anon** 20-24 (256 MiB, MADV_NOHUGEPAGE, mincore 0 -> touch one page per MiB -> mincore
   exactly 256), **prot** 30-36 (64 MiB PROT_NONE reservation, 1 MiB opened RW, NONE/RW/RO round trips keep the byte, MAP_FIXED
   replaces, munmap then mincore -ENOMEM), **brk** 40-43 (+128 MiB, shrink/regrow zero), **madv** 50-51 (DONTNEED re-zero / re-read),
   **big** 60-63 (160 MiB, EVERY page stamped and read back — 40960 resident pages, past the heap share: the pool's falsifier).
2. If `/apps/LIB/libc.a` is staged: `TCC.LNX -static -o <home>hellop.lnx /apps/PRINTF.C` (60 s; PRINTF.C mallocs, strcpys and
   printfs), then the output must print `hello printf from tcc+musl on unaos 42`.

Verdict: KAT fail or tcc fail = FAIL; both ok = PASS; anything absent = SKIP.

## Host proof (R78: the only executions)
- `SYSKAT3.LNX` (gcc 13.3 `-static -nostdlib -fno-pie -no-pie -O1 -ffreestanding -mno-red-zone`, 14216 B, sha256
  `df7ca9d1d88ed91f97211060be5cda69c8f50af27b63961a43f3507df71b5c9f`) on the host Linux 6.18 kernel (THP `madvise`):
  `syskat3 ok mmap=ok shared=ok anon_mib=256 resident_pages=256 anon=ok prot=ok brk=ok madv=ok big_mib=160 checks=37 fail=none`,
  exit 0, on every run, all cores and `taskset -c 0`. `strace -c`: write 36, mmap 11, munmap 10, pread64 5, brk 5, mincore 5 (1 ENOMEM
  — check 36), mprotect 4, madvise 4, openat 3, close 3, unlink 2, msync, ftruncate, fstat, execve — 92 calls, every one answered
  by the shim after this arc (msync/mincore are new; madvise was a no-op).
- `TCC.LNX -B target/LIB/tcc -static -o hellop printf.c` on the host: exit 0, a 39018-byte static ET_EXEC (sha256 `8720bc88…`,
  identical across two compiles) that prints `hello printf from tcc+musl on unaos 42`. tcc's own surface over that compile
  (`strace -f`): read 1121, lseek 543, openat 21, close 19, write 10, brk 7, mmap 2, munmap 2, plus the SELFBUILD1 start-up set —
  no new syscall, so tcc needs nothing beyond SELFBUILD1 + the streamed reads; the output program (musl start-up): arch_prctl,
  set_tid_address, brk 2, mmap 2, munmap, ioctl(TIOCGWINSZ), writev, exit_group — all answered.
- The whole `build_selfbuild_x86` ran end to end in the worktree (rc 0; musl, tinycc, libtcc1.a, the hellop proof, SYSKAT3).

## What a metal boot should print (`./arroyo esp-x86`, metal shape with `linuxabi`, logged in as `<user>`)
```
tests selfbuild3
:: LINUXABI: path=/apps/SYSKAT3.LNX exit=0 syscalls=<~92> enosys=[] ms=<n> -> PASS ::
[selfbuild3] kernel: peak_resident=<~41100> faults_anon=<~41500> faults_file=<~8> writeback_pages=<2..3> pool_peak=<~16500> pool_pages_used=0 limit_pages=<24576 + pool> refusals=0
[selfbuild3] kat3: syskat3 ok mmap=ok shared=ok anon_mib=256 resident_pages=256 anon=ok prot=ok brk=ok madv=ok big_mib=160 checks=37 fail=none
:: LINUXABI-KAT3: mmap=ok shared=ok anon_mib=256 resident_pages=256 prot=ok brk=ok madv=ok big_mib=160 fail=none exit=0 -> PASS ::
[selfbuild3] linux /apps/TCC.LNX -static -o /home/<user>/hellop.lnx /apps/PRINTF.C
:: LINUXABI: path=/apps/TCC.LNX exit=0 syscalls=<~1750> enosys=[] ms=<n> -> PASS ::
[selfbuild3] output /home/<user>/hellop.lnx bytes=<~39018>
:: LINUXABI: path=/home/<user>/hellop.lnx exit=0 syscalls=<~10> enosys=[] ms=<n> -> PASS ::
[selfbuild3] hellop: hello printf from tcc+musl on unaos 42
:: SELFBUILD3: mmap=ok shared=ok anon_mib=256 resident_pages=256 libc=musl tcc_libc=ok -> PASS ::
```
plus, once per boot, `[linuxabi] frame pool: first frame … (pool <n> pages = <MiB> MiB past the heap)` during the big group.
`tests linuxabi`/`selfbuild`/`selfbuild2` keep their wires (SYSKAT checks 1-8 now run on lazy pages).
Reading a failure: `fail=<id>` is the C header's table; `mmap=fail(3)` = the file page fault read the wrong bytes; `resident_pages=65536`
= eager memory; `big_mib=0` with a `resident limit` line = the pool (its numbers are on that line); `tcc_libc=fail(tcc exit=1 …)` with
`[selfbuild3] tcc: … not found` = `/apps/LIB` incomplete on the volume.

## Owed
- A shared PAGE CACHE: MAP_SHARED pages and `read()`/`write()` of the same file are coherent only after `msync`/`munmap`; two
  processes mapping one file MAP_SHARED each have their own frames. MAP_SHARED across `fork` is copied (as LINUXABI2's eager fork
  always did); copy-on-write fork (the fault hook now exists; the FORK_MAX_PAGES 64 MiB eager bound stays).
- SIGBUS past EOF (those pages read zeroes); `execve` still reads the ELF whole (128 MiB cap) and maps it eagerly; page tables
  come from the kernel heap; the pool is one global free list (no per-process accounting beyond the shared budget).
- Peter's word on the musl fetch (a GitHub mirror) — and on the `/apps/LIB` location (`<home>`-relative or a `system/` path are the
  alternatives).
- Then (SELFBUILD1's list): a static linker on the volume for rustc, and a static rustc (needs a musl host toolchain — this arc's
  musl build is the first piece of one).

## Not done / limits
- NEVER run (R78: no QEMU). The host runs are the only executions; the kernel side is compile-proven (x86 metal shape with
  `linuxabi`, aarch64 leg). The user frame pool has never handed out a frame on any machine: the `big` group is its first falsifier.
- All threads of a session stay pinned to the verb's core (SELFBUILD2); the fault hook relies on it for its local TLB flush.
