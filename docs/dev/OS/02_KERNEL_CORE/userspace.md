# Userspace: the kernel's privilege boundary (Ring 3 / EL0)

> Living document — the executing session updates this as each U-/M6-arc lands
> (per `CLAUDE.md`, the doc update is part of the arc's DONE gate). Sequencing
> in [`../../../ROADMAP.md`](../../../ROADMAP.md) §1; security model in
> [`../../../SECURITY.md`](../../../SECURITY.md).

## Current state (2026-07-02)

### aarch64 (branch `hw-pi4`) — the pioneer

- **M6a-0** — the kernel dropped from EL2 to **EL1** (EL2 residency was an
  accident of UEFI handoff; an EL0 SVC with `HCR_EL2.TGE=0` is taken to EL1,
  so EL1 is where the kernel must live). Fresh `TCR_EL1`/`SCTLR_EL1` setup;
  UEFI/QEMU-virt builds still enter at EL2.
- **M6a** — first EL0 round-trip: a scheduled task `eret`s to EL0, the user
  routine invokes `svc #0` with a Linux-style ABI (syscall number in `x8`,
  args `x0..x5`), handled at `VBAR_EL1 + 0x400`. Syscalls: `SYS_WRITE = 1`,
  `SYS_EXIT = 2`. User code copied to its page with the
  `DC CVAU` / `IC IVAU` cache-maintenance sequence. Metal-confirmed
  (2026-07-01): `hello from EL0` on the real Pi 4.
- **M6b** — per-page permissions + fault→task-kill: L2→L3 demotion to 4 KiB
  pages for a 16 KiB identity-mapped user window (code EL0-RX/EL1-RO via the
  kernel's first live PTE update + broadcast `tlbi vaae1is`; data/stack
  EL0-RW with UXN+PXN). EL0 synchronous faults kill the offending task via
  the scheduler — with `(task, EC, FAR)`-matched accounting and a
  cross-core verdict — instead of halting the kernel. Metal-confirmed
  (2026-07-01), including the stale-TLB case QEMU cannot test.
- **M6c** — loadable user program: the well-behaved `hello` routine moved OUT of
  the kernel's `.text` into a separately linked crate (`crates/user-blob`),
  `llvm-objcopy`'d to a flat `target/user_blob.bin`, `include_bytes!`'d and copied
  into the EL0 code page at boot (the `DC CVAU`/`IC IVAU` maintenance is kept for
  the copy). Position-independent (byte-granular `adr`, inline message), so it runs
  wherever it lands — no ELF loader yet. The M6b fault fixtures stay inline. QEMU-
  verified (2026-07-02): `:: M6c: user blob loaded (51 bytes) ::`, `hello from EL0`,
  the M6b verdict still `PASS`, capstone 6/6. Metal-verify pending — the D-cache/
  I-cache copy path is a no-op in QEMU.
- **M6e** — preemptible EL0 (metal-only preemption): `__vec_irq` now banks **SP_EL0**
  (the user stack pointer) onto the preempted task's own kernel stack alongside
  ELR_EL1/SPSR_EL1/FP, and `spawn_user` starts the EL0 task with **IRQ unmasked**
  (SPSR `0x2C0 → 0x240`), so the generic timer preempts a running EL0 task and it
  resumes with the correct user SP. The shared user stack is retained — no EL0 demo
  program writes it (hello and the new spinner are register-only; the fault fixtures
  fault before any push) — so a stack-writing program would need per-task stacks (M6d).
  Demo: a long register-only EL0 spinner (`el0-spin`) on the demo core + an
  `m6e-verdict` that reports `spinner completed` and `IRQs-taken-at-EL0` (counted in
  `aarch64_irq_handler` when the banked SPSR shows an EL0t return). QEMU-verified
  (2026-07-02): the `:: M6e: EL0 preemptible … ::` setup + verdict lines, the M6b
  verdict still `PASS`, capstone 6/6, no regression. **Metal-only, now confirmed:** QEMU
  raspi4b delivers no Group-1 timer IRQ, so `IRQs-taken-at-EL0 = 0` there and EL0 is not
  actually preempted — but on the real Pi 4 (2026-07-02) the run shows `spinner
  completed=1 IRQs-taken-at-EL0=18` (EL=1, CNTFRQ=54 MHz): the timer preempted the
  spinning EL0 task 18 times AND it resumed correctly (`completed=1` after the
  preempts proves the banked SP_EL0 restored a usable user stack), with the M6b
  verdict still `PASS`, capstone 6/6, and zero fault lines. (The spinner is register-
  only, so it demonstrates *resume-correctness* but cannot observe the banked SP_EL0
  *value*; the M6d `SP-relative sentinel readback` fixture below is what proves value
  fidelity — a preempted EL0 task reads its own stack sentinel through SP after the
  preempts — metal-confirmed with M6d, 2026-07-02.)
- **M6d** — per-task address spaces (ASIDs) + per-task user stacks. Each EL0 task gets
  its own translation-table branch (private `L1`/`L2_USER`/`L3_USER` copied from the
  boot tables, only entry 0 differs) and its own 16 KiB backing mapped at the *same*
  virtual addresses, from a static pool of 8 slots (`boot::alloc_user_slot`, no heap in
  the switch path). ASID = slot + 1 (1..8); ASID 0 is the shared/boot context. **All
  user-window leaves are now non-global (`nG=1`)** so the same VA maps different frames
  per task with no same-VA global/non-global TLB conflict; kernel leaves stay Global
  (ASID-agnostic — a task switch needs no kernel-mapping flush). `dispatch_next`
  installs the incoming task's `TTBR0_EL1 = root | asid<<48` only when the live root
  differs; `exit` tears a slot down (repoint TTBR0 off the dead root → broadcast
  `TLBI ASIDE1IS` → free the slot). First-entry to EL0 also scrubs x0–x30 (hardening;
  the aarch64 twin of the x86 SYSRET scrub). Demos (QEMU-provable — isolation is visible
  without interrupts): `:: M6d: per-task address spaces (8 slots, ASID 1-8, nG user /
  global kernel) ::`, **same-VA isolation** (two tasks read distinct slot-private
  sentinels at the identical VA — `A=0xa5a5…1 B=0x5a5a…2 distinct -> PASS`, backed by a
  deterministic kernel-side TTBR0-swap nG probe that catches a broken `nG` on metal),
  **EL0 stack write/readback** (the capability this arc unlocks — a program pushes/pops
  its own stack, impossible on the old shared window), and **SP-relative sentinel
  readback**. QEMU-verified (2026-07-02): all three `-> PASS`, with M6c/M6b/M6e all
  unchanged and CAPSTONE 6/6. **★ METAL-CONFIRMED on the real Pi 4 (2026-07-02, EL=1,
  CNTFRQ=54 MHz):** all three M6d lines `-> PASS` on real A72 caches/TLB — the same-VA
  isolation + the TTBR0-swap `nG` probe prove the ASID/`nG` discipline discriminates on
  silicon (QEMU can only re-walk), and with EL0 preemption live in the same boot
  (`M6e: … IRQs-taken-at-EL0=21`, QEMU=0 — an aggregate, demo-wide counter) all four
  M6d slot tasks reported correct values across interleaved dispatches — strong but
  aggregate evidence that per-task `TTBR0`/ASID switching + SP_EL0 banking hold under
  preemption (a per-task preempt counter would make the attribution exact; M6f). M6b `exited=1 killed=3 -> PASS` (same 3 ECs), CAPSTONE 6/6, 0 unexpected
  faults. (`IRQs-taken-at-EL0` grew 18→21 vs M6e — the extra preemptible tasks, expected
  and metal-variable.) M6d METAL-COMPLETE.
- **M6f** — validated user pointers + a wider syscall surface. `copy_from_user`/
  `copy_to_user` are factored, range-checked primitives: a user pointer is
  rejected (`-EFAULT`, an error RETURN — never a task-kill) unless `[va, va+len)`
  lies fully inside the caller's EL0 window with a non-wrapping length; the
  to-user direction additionally excludes the read-only code page, so a write
  aimed there is refused *before* the store instead of faulting the kernel.
  SYS_WRITE now streams through `copy_from_user` (validate-whole-range then
  chunk, so a bad pointer yields `-EFAULT` with no partial output). New syscalls,
  all thin over existing scheduler/timer primitives: `SYS_YIELD = 4`
  (`sched::yield_now`), `SYS_SLEEP_MS = 5` (`sched::sleep_ticks`, ms→ticks at the
  250 Hz tick, round up; a cooperative yield under QEMU where the timer IRQ isn't
  delivered), `SYS_GETPID = 6`, `SYS_GETINFO = 7` (writes a fixed {pid, ticks}
  struct via `copy_to_user`). Four EL0 fixtures on private slots (the getinfo
  fixture writes its stack, so it needs its own): a well-behaved getinfo
  round-trip, a hostile-pointer fixture (four bad pointers, each must EFAULT and
  not kill), and a yield/sleep pair that cooperatively interleaves on one core.
  Also lands the M6d review folds: an FP/SIMD first-entry scrub (zero `v0-v31`
  + FPSR/FPCR + TPIDR_EL0/RO in `user_task_trampoline`), a slot-alloc unwind
  (`boot::alloc_user_slots` releases on partial failure), and a per-task EL0
  preempt counter (refining M6d's aggregate `IRQs-taken-at-EL0`). QEMU-verified
  (2026-07-02): `:: M6f: validated user pointers … ::`, `getinfo/copy_to_user
  round-trip -> PASS`, `4 hostile pointers refused (EFAULT), 0 kills -> PASS`,
  `yield/sleep interleave -> PASS`, the per-task preempt line (all 0 under QEMU;
  > 0 on metal), with M6c/M6b/M6d/M6e + CAPSTONE all unchanged and 0 unexpected
  faults. ★ Metal-confirmed (real Pi 4, 2026-07-04, on the M6g reflash): all three M6f
  verdicts PASS on silicon and the per-task preempt counter went > 0 (`spsentinel=3`).
- **M6g** — load a program FROM STORAGE and run it at EL0 (the Pi twin of x86 U2).
  The block layer (`drivers::block`) now dispatches over a registered backend: the
  default xHCI USB-MSC path is untouched, and a new `register_sd` (cfg-gated to
  `aarch64 + baremetal`) flips it to a BCM2711 EMMC2/SDHCI microSD driver
  (`drivers::emmc2`) — so the x86 read/write path stays byte-identical (a nonempty
  x86 diff would be a STOP). The driver is deliberately minimal (PIO, single-block
  CMD17 reads, polled, no DMA, no writes; every wait CNTPCT-bounded so a dead/absent
  controller fails cleanly instead of hanging boot) and probes TWO candidate bases:
  **EMMC2 @0xFE34_0000 first (the real microSD slot on silicon), then the legacy
  Arasan SDHCI @0xFE30_0000** — the reverse of QEMU, whose `if=sd` card lands on the
  legacy base. So QEMU exercises the fallback leg and the **EMMC2 success leg runs on
  metal only**. On a successful init ladder (CMD0/CMD8/ACMD41/CMD2/CMD3/CMD9 →
  capacity from the CSD, remembering the R2 off-by-8 / CMD7/CMD16, then a clock bump)
  it registers the SD geometry; the M6g loader — a kernel task spawned after the M6f
  verdict — mounts the FAT volume off that card, reads `HELLO.BIN` (the M6c blob
  bytes, carried onto the boot media next to `kernel8.img`), size-checks it against
  one code page from the on-disk directory entry, copies it into a fresh M6d slot,
  protects the page EL0-RX/EL1-RO *before the task exists*, and drops it to EL0. The
  loaded bytes are UNTRUSTED — bounded only by size and contained by EL0 + per-page
  perms + the M6b fault-kill net (no signature/allowlist; that is the U-chain's
  code-signing item). QEMU-verified (`./arroyo kernel8-test 30`): `:: M6g: SD card
  @0xfe300000 identified — 131072 blocks (64 MiB, CSD v1) ::`, `FAT mounted from SD
  (Fat32)`, `HELLO.BIN loaded from SD (51 bytes) -> EL0`, a second `hello from EL0`,
  `disk-loaded EL0 program exited ok -> PASS`, with M6b/M6c/M6d/M6e/M6f + CAPSTONE
  all unchanged and 0 unexpected faults; the `UNAOS_SDIMG=0` control adds exactly the
  two no-card lines + the loader-skipped line. **★ Metal-confirmed (real Pi 4,
  2026-07-04):** the EMMC2-first success leg — which QEMU cannot exercise — ran on
  silicon: no fallback line, `SD card @0xfe340000 identified — 31116288 blocks (15193
  MiB, CSD v2)` (the real ~16 GB SDHC card, block-addressed), then FAT mount + `HELLO.BIN
  loaded from SD (51 bytes) -> EL0` + second `hello from EL0` + `disk-loaded EL0 program
  exited ok -> PASS`, with M6b/M6d/M6e/M6f + CAPSTONE 6/6 all green and 0 unexpected
  faults. The reflash also carried M6f's metal (all three M6f verdicts PASS; the per-task
  preempt rider went > 0 — `spsentinel=3`).
- **M7** — a minimal process model: `sys_spawn` + `sys_wait` (the Pi pioneers the
  roadmap-U4 process model, as it did M6a–M6g). Two new syscalls continue the M6f
  surface: **`SYS_SPAWN = 8`** loads the fixed on-disk program (`HELLO.BIN`) into a
  fresh per-task slot and runs it at EL0 as a *child* of the caller, returning the
  child's pid (no name/path argument this arc — that is M8, needing a validated
  `copy_from_user` name); **`SYS_WAIT = 9`** blocks the caller until that child exits
  and returns its exit status. A small static process table (`PROCS`, cap 4 « the 8
  slots) survives each child's slot teardown; each entry carries a counting
  `Semaphore` the child *posts* once (on exit **or** kill) and the parent *waits* once
  — a scheduler wake, so the whole reap round-trip is **QEMU-testable** (the waker is a
  scheduler post, not the timer). `sys_spawn` reuses the M6g loader core, refactored
  into a shared, silent `load_program_into_slot()` (the M6g loader now reconstructs its
  own serial lines from the result, so its output stays byte-identical). The child's
  pid-recording race is closed by a **co-location invariant**: the child is queued on
  the caller's core and cannot be dispatched until the parent yields (in `sys_wait`),
  which is strictly after `sys_spawn` records the pid — all IRQ-masked in the SVC
  handler, so no `sched.rs` change is needed. The demo is a gated launcher (the M6g
  shape) that, after M6g frees its slots, spawns a parent fixture (`el0-m7parent`) that
  `sys_spawn`s a child, `sys_wait`s it, and reports the reaped pid as its witness.
  QEMU-verified (`./arroyo kernel8-test 30`, after the M6g lines):
  `:: M7: process model — sys_spawn + sys_wait (parent reaps a disk-loaded child) ::`,
  a **third** `hello from EL0` (M6c inline #1, M6g loader #2, the M7 child #3), and
  `:: M7: parent spawned child pid=<p>, waited, child exited status 0 -> PASS ::`, with
  every M6b/M6c/M6d/M6e/M6f/M6g + CAPSTONE line unchanged and 0 unexpected faults.
  **★ Metal-confirmed (real Pi 4, 2026-07-04):** `:: M7: parent spawned child pid=41,
  waited, child exited status 0 -> PASS ::` on silicon — the child loaded off the real
  card via the EMMC2-first path QEMU cannot exercise (`SD card @0xfe340000 — 15193 MiB,
  CSD v2`), printed the third `hello from EL0`, and was reaped by the parent's blocking
  `sys_wait` (woken by the child's scheduler post) under a live timer, with the whole prior
  battery green (M6b/M6d/M6e/M6f/M6g + CAPSTONE 6/6) and 0 unexpected faults.
- **U4** — the process model + **per-process handle table** (roadmap-U4: "PIDs,
  spawn/wait/exit status, per-process handle table"). Evolves M7 without a new syscall
  number: **`SYS_SPAWN = 8`** now installs the child into the *spawner's* handle table
  and returns a small **handle index** (not a raw pid); **`SYS_WAIT = 9`** takes that
  *handle*. Ownership becomes **structural** — a task can only reap children whose handles
  are in ITS table, which folds M7's review note (any task could `sys_wait` any pid) by
  construction. The table is a static, const-init `HANDLES[[AtomicU64; 8]; USER_SLOTS+1]`
  keyed by the caller's **ASID** (read from `TTBR0_EL1[63:48]` synchronously in the SVC
  handler — the caller's root is live there); a handle value is the child pid (0 = Empty).
  `PROCS` stays keyed by pid (the exit-accounting control blocks); `HANDLES` is keyed by
  ASID (the spawner's private *namespace of child capabilities*) — deliberately separate.
  `sys_spawn` reserves a handle slot *before* loading (a full table fails `-EAGAIN` with
  nothing to un-spawn), then stores the real pid into both the `PROCS` entry and the handle
  slot; `sys_wait` resolves the handle → pid, reaps M7-style, then *consumes* the handle
  (a second wait on it returns `-ECHILD`). `sys_write` is **left untouched** (routing a
  resource syscall through a handle is deferred to U5, when there is a capability check to
  add). The demo evolves the M7 launcher: a parent fixture (`el0-u4parent`) `sys_spawn`s
  **two** children and reaps **both by handle**, and an ownership negative (`el0-u4orphan`,
  its own slot/ASID) calls `sys_wait(0)` on an Empty handle and must get `-ECHILD`.
  QEMU-verified (`./arroyo kernel8-test 30`, in place of the M7 line): the U4 setup line,
  **four** `hello from EL0` (M6c inline #1, M6g loader #2, the two U4 children #3/#4), and
  `:: U4: process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD
  (per-process handle tables) -> PASS ::`, with every M6b/M6c/M6d/M6e/M6f/M6g + CAPSTONE
  line byte-identical (hex/pid-normalized set-diff: only the M7 line → the U4 line, `hello`
  3 → 4) and 0 unexpected faults. **No metal this arc** — every piece is scheduler/syscall
  logic (handle install/resolve/clear, the owner-scoped reap, the `-ECHILD` negative), all
  deterministic under QEMU raspi4b (the reap wake is a scheduler post, not the timer); the
  child disk-loads ride the same EMMC2 path M7 already metal-confirmed. This is the exact
  substrate U5 turns into capabilities (a child handle IS a capability to that process;
  grant = transfer the handle, revoke = clear it — U5 adds the *check* at this handle lookup).
- **U5** — handles as **capabilities**: the enforcement CHECK + grant/attenuate/revoke +
  routed `sys_write` + teardown-clear (roadmap-U5). Turns U4's handle STRUCTURE into a
  checked capability. A handle now carries **rights** — a bitmask `CAP_READ`(0x1)/`CAP_WRITE`
  (0x2)/`CAP_EXEC`(0x4)/`CAP_GRANT`(0x8)/`CAP_REVOKE`(0x10) — held in a **sidecar**
  `HANDLE_RIGHTS[[AtomicU32; 8]; USER_SLOTS+1]` keyed identically to `HANDLES`, so U4's value-
  word sentinels (`0` = Empty, `u64::MAX` = RESERVING) are byte-unperturbed — and names a
  **target** beyond "child pid": a well-known `HANDLE_CONSOLE = u64::MAX-1` token (two kinds
  only, `CHILD(pid)` and `CONSOLE`; a general object table is U6). The CHECK is one
  `handle_resolve(asid, idx, req_rights)` at the single lookup point every handle-consuming
  path uses: out-of-range/Empty ⇒ the caller's own errno (`sys_wait` → `-ECHILD`, preserving
  U4 structural ownership; `sys_write`/`SYS_CAP` → `-EACCES`), present-but-missing-a-right ⇒
  `-EACCES`. **`SYS_CAP = 10`** (sub-op in `x0`: `GRANT=0`/`REVOKE=1`) adds the first-class
  ops: **GRANT**(src_idx, req_rights) mints a new handle to the same target carrying a rights
  mask that must be a **subset** of the granter's rights on the source — the **attenuation
  (monotonic-decrease) invariant**, `req & !src_rights != 0` ⇒ `-EACCES` (a grant can never
  amplify), requiring `CAP_GRANT` on the source; **REVOKE**(idx) drops a handle the caller
  owns (ownership-based — a process may always drop its own caps; subsequent use ⇒ `-EACCES`/
  `-ECHILD`). **`sys_write` routes through the table**: `fd` is a handle index that must
  resolve to a `CONSOLE` handle with `CAP_WRITE` — there is no ambient stdout. Every printing
  EL0 process is **endowed** a `CONSOLE`+`CAP_WRITE` cap at the conventional index
  `CONSOLE_FD = 1` at spawn/launch (`install_console_cap`: the shared window ASID 0 for
  `el0-hello`; each M6f/M6g/U4-child slot); the `copy_from_user` validation + all-or-nothing
  `-EFAULT` path is byte-identical, so the M6f hostile fixture (which holds the cap) still gets
  `-EFAULT`, not `-EACCES`. **Teardown-clear** folds U4's one deferred lifecycle note:
  `boot::teardown_user_slot` wipes the whole `HANDLES[asid]` row + its rights **before**
  releasing the slot's used-flag (not after — a post-release clear could race a concurrent
  `alloc_user_slot` on another core that reclaims the same ASID), so no capability outlives its
  owning ASID; the clear lives behind `syscall::clear_handle_row(asid)` (both modules are
  `#[cfg(feature = "baremetal")]`). QEMU-verified (`./arroyo kernel8-test 8`, after the U4
  PASS): the U5 setup line, `u5: cap write` **twice** (the write-cap write + the write through
  the minted attenuated cap), and `:: U5: capabilities — write-cap OK, no-cap -EACCES,
  attenuated grant bounded, revoke enforced, teardown-clear clean -> PASS ::`. The `el0-u5cap`
  fixture proves the four EL0-observable behaviours against its own table (write-cap OK; a
  write-less cap → `-EACCES`; a grant exceeding the granter rejected while a subset grant works
  and its handle writes; a revoked handle → `-EACCES`) via a witness bitmask (`0xF`); the
  launcher proves the fifth kernel-side (the fixture's handle row is clear after its slot
  teardown). Every M6b/M6d/M6e/M6f/M6g/U4 line byte-identical (only the four U5 lines added;
  all four prior `hello from EL0`, the M6f `EFAULT` PASS, U4 PASS, CAPSTONE 6/6) and 0
  unexpected faults. No metal this arc (pure syscall logic; the child loads ride U4/M7's
  metal-confirmed EMMC2 path). Lane: `arch/aarch64/syscall.rs` + the `boot.rs` row-clear + a
  `main.rs` launcher — no scheduler primitive, no driver, no x86 file.
- **U6a** — the **general object table**: `(kind, target, rights)` descriptors, first-free for
  ALL kinds, the `CONSOLE_FD` collision closed (roadmap-U6 part (a)). Generalizes U5's
  fixed-shape handle. The **kind** rides in a parallel sidecar `HANDLE_KIND[[AtomicU8; 8];
  USER_SLOTS+1]` (keyed like `HANDLES`/`HANDLE_RIGHTS`), so the value word keeps U4/U5's `0`=Empty
  / `u64::MAX`=`RESERVING` sentinels byte-identical and nothing else is reserved; a handle names
  `Child(pid)`, `Console`, or the **scaffolds** `File(id)` / `Socket(id)` (resolvable via
  `handle_resolve`, but no fs/net syscall routes through them yet). `handle_resolve` now dispatches
  on the kind sidecar. **The fixed `CONSOLE_FD = 1` pin is retired**: it is a *reserved* index the
  first-free allocator (`handle_install`) SKIPS, so the console cap (installed there by the
  `fd=1`/stdout convention — keeping every prior blob byte-identical) and auto-allocated child/
  object handles (`{0, 2, 3, ..}`) never collide, for any interleaving — closing U5's one design
  note (a process that both prints and spawns 2+ children). Attenuation is unchanged and a grant
  now copies the source handle's kind (attenuate rights, never re-kind); `handle_clear`/
  `clear_handle_row`/`handle_row_is_clear` also handle the kind. Evidence — `./arroyo kernel8-test`
  → after the U5 PASS line, the `el0-u6spawn` printing spawner (prints, spawns 2 children off the
  reserved index, prints again proving the console survived, reaps both by handle) plus a
  kernel-side check (File/Socket kinds resolve with/without rights; the U5-breaking interleaving no
  longer collides): `:: U6: general object table — printing spawner + 2 children, no index
  collision, File/Socket kinds resolve -> PASS ::`. Every M6b/M6d/M6e/M6f/M6g/U4/U5 line
  byte-identical (only `VBAR_EL1` shifts one page — benign binary growth — plus the new U6 lines),
  CAPSTONE 6/6, 0 unexpected faults. No metal this arc (pure syscall logic; the two children ride
  U4/M7's metal-confirmed EMMC2 path). Lane: `arch/aarch64/syscall.rs` + a `main.rs` launcher — no
  `boot.rs` change (`clear_handle_row` already wipes the whole row), no scheduler primitive, no
  driver, no x86 file.
- **U6b** — **real `File` handles**: `SYS_OPEN`/`SYS_READ` routed through the object table via
  `File` + `CAP_READ` (makes U6a's `File` scaffold real — the first resource syscall on a
  **non-Console** object, the precursor to UnaFS grants). **`SYS_OPEN = 11`**`(name_ptr, name_len)`
  `copy_from_user`s the bounded 8.3 name, mounts the single **read-only** FAT volume, finds the
  top-level entry (a directory ⇒ `-EISDIR`, missing ⇒ `-ENOENT`), records a per-task **open-file
  descriptor**, and installs a `File` handle carrying `CAP_READ`; **`SYS_READ = 12`**`(handle, buf,
  len)` is the CHECK — `handle_resolve(asid, handle, CAP_READ)` must yield a `File` (a missing right,
  a non-File kind, or no handle all ⇒ `-EACCES`, the twin of `sys_write`'s Console+`CAP_WRITE`) — then
  clamps to `min(len, size − offset)`, validates the destination (`user_range_ok(.., writable=true)`;
  a bad buffer ⇒ `-EFAULT`, no read, **no offset advance**), reads via a read-only offset-aware FAT
  reader (`fat::read_at`; `read_file` left byte-identical), `copy_to_user`s, and advances the offset
  by the count delivered (`0` = EOF; **sequential, no seek**). The descriptor lives in a per-task
  **FILES table** — parallel atomic arrays `FILE_USED`/`FILE_CLUSTER`/`FILE_SIZE`/`FILE_OFFSET` keyed
  `[asid][idx]` (`NFILE = 4`), the same sidecar shape as `HANDLE_RIGHTS`/`HANDLE_KIND`; the `File`
  handle's value word carries the **file-id = descriptor index + 1** (the `+1` keeps it clear of the
  `0`/`u64::MAX` sentinels). `sys_open` claims resources last (mirroring `sys_spawn`'s reserve/unwind
  — a full handle table after a descriptor was claimed frees it then `-EAGAIN`, no leak).
  Teardown-clear now folds files in: `clear_handle_row` also clears the FILES row, so a reused ASID
  sees no stale file. Evidence — `./arroyo kernel8-test` → after the U6 PASS line, the `el0-u6bfile`
  fixture opens `HELLO.BIN`, reads it through the `File` capability and verifies the bytes against the
  kernel-planted on-disk prefix, then proves the CHECK denies both a present File lacking `CAP_READ`
  (rights arm) and a `Socket` carrying `CAP_READ` (kind arm) with `-EACCES` — witness `0x1F`; the
  launcher proves the file-row teardown-clear kernel-side: `:: U6b: real File handles — open+read via
  a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::`. Every
  M6b/M6d/M6e/M6f/M6g/U4/U5/U6 line byte-identical (the shared FAT mount does not regress M6g/U4),
  CAPSTONE 6/6, 0 unexpected faults. ✅ **METAL-CONFIRMED on the real Pi 4 (2026-07-06)**: U6b PASS on
  silicon — the fixture read `HELLO.BIN` through a `File` capability off the real EMMC2/SD card (the
  metal-only `@0xfe340000 15193 MiB CSD v2` leg), both `-EACCES` denials held; `EL=1`/54 MHz, full
  battery green (metal log `unaos/target/serial-pi.u6b-metal.log`; the scheduler CAPSTONE sat out that
  boot — only 3 of 4 cores online, a metal SMP variance orthogonal to U6b). Scope: read-only, flat root, one
  volume — no write/create/delete, no seek, no dir ops. Lane: `arch/aarch64/syscall.rs` + a `main.rs`
  launcher + a read-only `fs/fat.rs` helper — no `boot.rs` change, no scheduler primitive, no x86 file.
- **U7 (landed 2026-07-07)** — cross-process transfer WITHOUT breaking single-writer: `SYS_XFER = 13`
  deposits an attenuated `(kind, target, rights)` descriptor into the recipient's per-ASID transfer
  **inbox** (tx-exact CAS discipline; the sender never writes the recipient's handle row);
  `SYS_RECV = 14` installs it into the caller's OWN row; `SYS_CAP` XREVOKE (sender-owned record) makes
  the received cap stale at its next `handle_resolve`. Owner-scoped (`dest` = a `Child` handle in the
  sender's table), `Console`/`Socket` payloads, single-level revoke. Demo: the parent's over-rights
  transfer refused, the child prints through the transferred cap, the revoked cap denies with
  `-EACCES`, and the launcher proves the child's row byte-clear while the deposit was pending. See
  the `SECURITY.md` U7 ledger entry for the full mechanism.
- **ELF-1 (landed 2026-07-22)** — the loader graduated from flat blob to a minimal static **ELF64**
  (`load_program_into_slot` dispatches on the magic; `validate_elf` walks the PT_LOAD program headers and
  maps each segment into the 16 KiB slot window with per-segment permissions — R+X → code page, R+W → data
  page). The flat path stays the fallback for magic-less `.BIN` fixtures. Witness: `:: ELF1: static ELF64
  loaded (… bytes, 2 PT_LOAD segs) -> EL0 ran … -> PASS ::`.
- **ELF-2 (landed 2026-07-23)** — first rung of the **EL0-threading** ladder: multiple EL0 tasks SHARING one
  address space (the parent's slot `ttbr0`/ASID), the substrate for multi-threaded programs. Three syscalls:
  `SYS_THREAD_SPAWN = 21` (entry, sp, arg, placement → a new EL0 thread under the caller's `ttbr0`/ASID,
  started at `entry` on a caller-carved `sp` with `arg` in x0, on the caller's core or a sibling; returns a
  per-process thread handle), `SYS_THREAD_JOIN = 23` (block on the thread's completion `Semaphore`, then
  reap), `SYS_THREAD_EXIT = 22` (post completion + release the slot). The slot now carries a **live-task
  refcount** (`boot::SLOT_REFCOUNT`): `alloc_user_slot` seeds it to 1, `slot_thread_retain` bumps it per
  thread, and `teardown_user_slot` frees the slot only on the 1→0 edge — so a shared address space outlives
  the first thread to exit and is torn down only when the last leaves. **Multi-core soundness** (the same ASID
  live on two cores): `teardown_user_slot` now repoints EACH exiting thread's core off the slot root
  unconditionally (not only the last), so the final broadcast `TLBI ASIDE1IS` races no live slot root on any
  other core — preserving `build_slot`'s "the ASID was flushed at teardown, no core can speculatively
  re-cache it" realloc invariant. Completion is posted at the single point in `sched::exit()` (moved out of
  `task_trampoline`), covering both a kernel thread's return and an EL0 thread's `SYS_THREAD_EXIT`; the thread
  arg rides in x0 out of `user_task_trampoline` (placed AFTER the GPR/FP scrub — a deliberate ABI value, the
  scrub's no-leak property unweakened). Test: a parent zeroes a shared counter, spawns 2 workers (one
  co-located, one on a sibling core), each **atomically** (A72 LL/SC — ARMv8.0, no LSE) increments the
  counter and exits, the parent joins both and reports the total. QEMU-verified (2026-07-23):
  `:: EL0: threads test — spawned=2 joined=2 counter=2 cores=1,2 :: PASS ::` — genuine cross-core EL0 with a
  shared `ttbr0`. Metal note: cross-core LL/SC atomicity depends on the user window being Normal
  Inner-Shareable cacheable memory (an arc-boundary hardware check, not gated by QEMU).
- **ELF-3 (landed 2026-07-23)** — rung 2 of the EL0-vug ladder: give EL0 something to **draw on** + real
  **synchronisation**. Three syscalls (`SYS_FB_MAP = 24`, `SYS_FB_PRESENT = 25`, `SYS_FUTEX = 26`).
  - **`SYS_FB_MAP()`** maps the calling process's dedicated, kernel-allocated **off-screen surface**
    (32×32 ARGB8888 = one page, EL0-RW Normal-cacheable) plus a **read-only info page** (magic, width,
    height, stride, format, size, surface-offset) into its EL0 window, and returns the surface VA. The
    surface + info live in a **reserved VA hole** carved immediately above the 16 KiB program window (the
    per-slot backing region grew from 0x4000 to 0x6000, 0x8000-aligned so the whole region stays inside one
    2 MiB `L3_USER` block); `boot::map_slot_fb` repoints the slot's private L3 leaves at the slot's OWN FB
    frames with a proper break-before-make (info EL0-RO `user_ro_page`, surface EL0-RW `user_data_page`).
    **EL0 NEVER receives the real scan-out**, any kernel mapping, or a physical address — page-permission
    laws (WXN, per-page perms) untouched.
  - **`SYS_FB_PRESENT()`** is the only path from the surface to the screen: the kernel composites the
    surface via a **present hook** (`syscall::register_fb_present_hook` — a public seam the video subsystem
    registers, calling the existing dirty-rect damage+flush). Until that hook is wired (a 3-line deferred
    diff in `video/screen.rs`, documented at the seam), present is a no-op composite; either way it records
    a checksum of the surface for the self-verifying witness. EL0 owns the surface bytes, never the scan-out.
  - **`SYS_FUTEX(uaddr, op, val)`** is a minimal wait/wake: `op=0` FUTEX_WAIT blocks iff `*uaddr == val`,
    `op=1` FUTEX_WAKE wakes up to `val` waiters. Keyed by the **physical address** of the user word (via
    `AT s1e0r` — globally unique, so a word shared across a process's threads keys the same bucket) on a
    bounded kernel wait-queue pool (`sched::futex_wait`/`futex_wake`, reusing the Semaphore `PARK_WAITQ`
    lock-handoff — lost-wakeup-safe). `uaddr` is validated inside the caller's writable user window. Enough
    to build a userspace mutex/condvar.
  - Test (`__fb_prog_*`, in-RAM): the parent maps its surface, reads geometry from the RO info page (proving
    EL0-read), spawns 2 draw threads (one co-located, one on a sibling core) that each fill THEIR HALF of
    the surface (top = 0xA1, bottom = 0xB2), an atomic counter + FUTEX wake/wait synchronises the parent to
    both halves being drawn, the parent PRESENTS (the kernel checksums the surface), joins both threads, and
    exits. QEMU-verified (2026-07-23): `:: EL0: fb test — mapped=32x32 threads=2 present=1 checksum=<hex>
    :: PASS ::`, the checksum self-verified against the kernel-computed expected pattern. **DEFERRED wiring:
    the `video/screen.rs` present-hook registration (3 lines) — out of the syscall/sched/boot lane.**
- **EXEC-1 (landed 2026-07-23)** — connect the ELF-1 loader to the VFS: a panel shell **`run <path>`**
  command that loads and executes an ELF64 (or flat) EL0 program off the filesystem and reports its exit
  status. The EL1/ASID-0 shell reads the whole file through the VFS `MountTable` (`/fat` = FAT boot
  partition, `/usb` = USB stick, `/` = native UnaFS), bounds it to the 16 KiB user window (an oversize file
  is rejected `-E2BIG`, never silently truncated), pre-checks the ELF64 magic + aarch64 machine for a
  friendly reason, then hands the bytes to the kernel loader **`run_user_image(name, bytes, deadline)`**.
  The loader shares the ELF-1 mapping core: `load_program_into_slot` (the by-name FAT loader) and
  `run_user_image` (the VFS-read path) both call a new FatKind-free **`map_image_into_slot`** — validate
  fully → `alloc_user_slot` → copy+map+protect each PT_LOAD (per-segment W^X) → stamp the IMAGE_SHA256
  principal — so the two paths never drift (m6g/ELF1 stay byte-identical). `run_user_image` endows the slot
  a console write-cap, plants a **Proc entry** so the program's exit rides the SAME generic child-reap
  short-circuit `sys_wait` uses (no dedicated `SYS_EXIT` arm — the program runs under an arbitrary name),
  spawns it co-located, and deadline-bounded-yields until it exits or the fault-kill net contains a fault
  (a killed run-image task marks its Proc entry with a kill sentinel, off the M6b `killed_unexpected`
  count). On return the scheduler has already repointed the core to the boot root (ASID 0), honouring the
  shell's ASID-0 invariant. The panel prints `run: <path>: exited with status <n>`; the headless witness is
  `:: EXEC: run <path> — loaded <n> bytes, entry 0x<..>, exit=<code> ::`. QEMU-verified (2026-07-23) via a
  boot self-test that reads `/fat/ELFHELLO.ELF` through the VFS and runs it through `run_user_image`:
  `:: EXEC1: run /fat/ELFHELLO.ELF — loaded 8560 bytes, entry 0x268000, exit=0 -> PASS ::` (the program
  prints its own `elf hello from EL0`).
- **UVUG-1 (landed 2026-07-23)** — the first REAL EL0 graphics program: a userspace **mini-vug**, and the
  affirmative answer to "is it possible to make EL0 vug more multi-threaded?". A static ELF64 program
  (`crates/user-uvug`, staged onto the FAT media as **UVUG.ELF** by `arroyo`, same build path as ELFHELLO)
  loaded and run through the EXEC-1 `run_user_image` machinery — the identical path the panel `run
  /fat/UVUG.ELF` drives. It maps its off-screen surface (`SYS_FB_MAP`), spawns **two persistent EL0 worker
  threads** (`SYS_THREAD_SPAWN` — one co-located, one on a **sibling core**) that each render HALF of an
  animated integer-math gradient into the shared surface, drives a **per-frame barrier**, `SYS_FB_PRESENT`s
  each of 300 frames, then `SYS_THREAD_JOIN`s both, computes a deterministic FNV-1a checksum of the final
  surface, and prints its own witness `:: UVUG: frames=300 threads=2 checksum=<hex> ::` before exiting 0.
  The barrier direction is deliberately split for robustness under QEMU raspi4b (which delivers **no Group-1
  timer IRQ**, so a core that parks with nothing else runnable is not preemptively rescheduled): **arrival**
  (worker→parent) is a real `SYS_FUTEX` (workers bump `done` + WAKE, parent WAITs — the fb-proven direction,
  reliable because the run core is kept cycling by the `run_user_image` driver loop); **release**
  (parent→worker) is a `SYS_YIELD` poll on a `phase` word (keeps each worker runnable on its own core, so no
  cross-core re-dispatch of an idle sibling is needed). Both wait loops re-check their condition, so the
  barrier is lost-wakeup-safe by construction; on metal (real timer IRQs) either direction works. The
  checksum is deterministic (the final surface is a pure integer function of pixel position and the last
  frame index, independent of thread interleaving), so QEMU and metal agree. EL0 owns only the off-screen
  surface bytes — never the scan-out, a physical address, or a kernel mapping; page-permission laws (WXN,
  per-page perms) are untouched. Kernel side is a small additive fix only: `run_user_image` now calls the
  idempotent `sched::futex_init()` (so a plain non-witness boot arms the futex pool before an EL0 program
  can call `SYS_FUTEX`), plus a `uvug_witness` battery self-test that reads UVUG.ELF through the VFS and runs
  it, asserting `exit=0`. QEMU-verified (`UNAOS_V3D=1 UNAOS_GENET=1 UNAOS_PIUSB=1 ./arroyo kernel8-test`,
  reproducible across runs): `:: UVUG: frames=300 threads=2 checksum=0x0313e510f24daae5 ::` then
  `:: EXEC-UVUG: run /fat/UVUG.ELF — loaded 8544 bytes, entry 0x270000, exit=0 -> PASS ::`, with the workers
  scheduled on cores 2 (co-located) + 1 (sibling) and the whole prior battery (CAPSTONE 6/6, EXEC1,
  ELF-2/-3) byte-equivalent. **DEFERRED (out of this lane):** the live panel animation still needs the
  ELF-3 present-hook wiring (the 3-line `video/screen.rs` `register_fb_present_hook` registration) — until
  it lands, `SYS_FB_PRESENT` composites nothing to the screen, so the panel run prints the witness and exits
  cleanly but shows no pixels. Lane: `crates/user-uvug` + `arroyo` (build/stage) + an additive
  `arch/aarch64/syscall.rs` (`futex_init` in `run_user_image` + the `uvug_witness` self-test) + this doc.
- **ELF-5 (landed 2026-07-23)** — rung 4 of the EL0-vug ladder: **input into EL0**. An interactive EL0 app
  (built on ELF-3's surface + ELF-2's threads + ELF-3's futex) needs keys/mouse; this is the delivery half.
  **`SYS_INPUT_POLL = 27`** is a NON-BLOCKING dequeue: it returns the next input event queued for the CALLING
  process, or `-EAGAIN` when its ring is empty. The event is a **packed u64** whose bit 63 is always clear
  (so it never aliases a negative errno): `[55:48]` = type (`1` KeyDown, `2` KeyUp, `3` MouseRel, `4`
  MouseAbs, `5` Button), the low 32 bits the payload — key ASCII / button mask in `[7:0]`, mouse x/dx in
  `[31:16]` and y/dy in `[15:0]` (i16). Kernel side mirrors how the GUI routes input to kernel apps today
  (the GUI-CLICK-2b `SCREEN_APP_ACTIVE` gate + `gui_watchdog`: only the ACTIVE app receives input): a small
  **per-ASID ring** (`EL0_INPUT_BUF`/`HEAD`/`TAIL`, cap 32), a single producer (the router) + single
  consumer (the EL0 task) so it is a **lock-free SPSC ring** (drop-newest on a full ring — an unread event is
  never overwritten). Two **public in-lane seams** (the ELF-3 present-hook twins): `el0_input_enqueue(ev)`
  (the router pushes a decoded `pal::Event` into the active process's ring) and `el0_input_set_active(asid)` /
  `el0_input_active()` (focus registration — the ELF-5 analogue of `SCREEN_APP_ACTIVE`; setting a focus
  resets that ring, so a freshly-focused app starts clean). Teardown folds in: `clear_handle_row` now resets
  the ASID's ring and clears the active designation if the dying slot held it (a reused ASID inherits no
  stale input; the router never enqueues to a dead slot). **SYS_INPUT_WAIT = 28 is DEFERRED** (documented at
  the seam) — a blocking variant on a per-ASID `Semaphore` that `el0_input_enqueue` would post; QEMU has no
  real input source, so `SYS_INPUT_POLL` is the QEMU-provable rung this arc lands. Test (`__input_prog`,
  in-RAM, register-only): the program polls its ring empty (`-EAGAIN`), the launcher — after the initial
  empty observation, so the ordering is exact — injects ONE `KeyDown('A')` through the real
  `el0_input_enqueue` seam, the program poll+yields until the event arrives, verifies the packed value, then
  polls empty again. QEMU-verified (`UNAOS_V3D=1 UNAOS_GENET=1 UNAOS_PIUSB=1 ./arroyo kernel8-test`):
  `:: EL0: input test — poll-empty=EAGAIN enqueue=1 event=0x1000000000041 drained=EAGAIN :: PASS ::` (the
  packed event = `(KeyDown<<48) | 'A'`), with the whole prior battery (CAPSTONE 6/6, ELF1/EXEC1, ELF-2 threads,
  ELF-3 fb, UVUG) byte-equivalent. **HONEST QEMU NOTE:** QEMU raspi4b delivers no USB HID, so the
  kernel-injected event is what proves the enqueue->drain + `-EAGAIN` paths — the router edge (real HID ->
  ring) is metal-only, lit up by the deferred fold. **DEFERRED ROUTER WIRING (2-3 lines, OUTSIDE this lane —
  `main.rs` `pump_usb_into_gui`, the next arc folds):** when an EL0 app owns the screen (an ELF-5 analogue of
  `SCREEN_APP_ACTIVE`, its ASID registered via `el0_input_set_active`), route drained pal events to the EL0
  ring instead of `GUI_CHANNEL`:
  ```rust
  while let Some(ev) = unaos_kernel::pal::next_event() {
      unaos_kernel::arch::aarch64::syscall::el0_input_enqueue(ev); // -> the active EL0 app's ring
  }
  ```
  Lane: `arch/aarch64/syscall.rs` (the ring + seams + `sys_input_poll` + the `clear_handle_row` fold + the
  in-RAM witness) + this doc — no scheduler primitive, no driver, no `main.rs`/`pal.rs` change, no x86 file.
- **INPUT-WIRE (landed 2026-07-23)** — folds ELF-5's deferred router wiring so a running EL0 program receives
  REAL keys/mouse. Two halves:
  - **Router (main.rs `pump_usb_into_gui`).** When an EL0 program holds input focus (`el0_input_active() != 0`),
    the GUI router drains `pal::next_event()` into that process's per-process ring via the `el0_input_enqueue`
    seam — **keyboard AND mouse** (Key/KeyUp/Mouse/MouseAbsolute/Button; Timer/None/Unknown are dropped by the
    seam) — instead of `GUI_CHANNEL`. Factored into `route_input_to_active_el0()`, the single router->ring choke
    point. This branch takes **precedence over `SCREEN_APP_ACTIVE`**: the panel `run` verb dispatches through
    `dispatch_command` (which sets `SCREEN_APP_ACTIVE`), so both flags are live during an EL0 `run`, and the EL0
    ring is the real sink. Unlike the kernel-app `SCREEN_APP_ACTIVE` gate (which LEAVES events in `EVENT_QUEUE`
    for the app's own `pump_and_poll`), the EL0 branch **drains** — an EL0 app cannot reach `EVENT_QUEUE`, so a
    left event would never be consumed.
  - **Focus lifecycle (`run_user_image`).** Focus is registered (`el0_input_set_active(asid)`) right after the
    slot exists and the pid is published, and BEFORE the wait loop yields (the co-located task cannot dispatch
    until then, so no input is missed). It is cleared (`el0_input_set_active(0)`) on return: `clear_handle_row`
    already clears the designation on slot teardown (verified — ELF-5's `EL0_INPUT_ACTIVE` compare-exchange), so
    the explicit clear is belt-and-suspenders for the exit/kill path and the **sole** clear on the Timeout path
    (task still alive, no teardown). The shell regains input the instant the program returns.
  - **Watchdog / escape hatch — decision (UVUG-5 correction).** The original ELF-5 decision left the
    `gui_watchdog` `note_progress`/`poll` path **byte-untouched**, reasoning that `run_user_image`'s 5 s deadline
    (`shell.rs`, equal to `gui_watchdog::WATCHDOG_TIMEOUT_SECS`) is the sole liveness bound for a focused EL0 app.
    That was **wrong in practice** (P47): the `run` command sets `SCREEN_APP_ACTIVE` and calls
    `gui_watchdog::on_app_enter`, so the wedge watchdog is *also* armed for the EL0 program — but an EL0 app drains
    input through `SYS_INPUT_POLL`, not the kernel `pump_and_poll` that feeds `note_progress`, so the watchdog saw
    **no heartbeat** and FALSELY reclaimed a healthy, polling UVUG at 5 s (`[gui] watchdog app wedged 5s (no drain
    since …)` — the app never lost its poll but the shell was handed the keyboard back mid-run). **Fix:**
    `sys_input_poll` now calls `gui_watchdog::note_progress()` on **every** poll (before the empty-ring return) —
    the EL0 twin of the kernel app's per-drain heartbeat: a program calling `SYS_INPUT_POLL` IS making drain
    progress. A live EL0 app is never falsely wedged; a genuinely dead one still loses the screen at the timeout.
    `note_progress` is a no-op when no app owns the screen, so the call is safe on any caller.
  - **QEMU proof.** A new BSP-side `input_router_selftest()` runs the REAL router drain against a fake active
    focus (ASID 1, before any service task or EL0 slot is live, `EVENT_QUEUE` empty): a Key + a Mouse pushed into
    `EVENT_QUEUE` are routed to the focused ring (`routed == 2`), a Timer is dropped, and `GUI_CHANNEL` is bypassed
    (`GUI_SENT` unchanged) — `:: EL0: input router — routed=2 (key+mouse) to active-focus ring, Timer dropped,
    GUI_CHANNEL bypassed :: PASS ::`. This proves the router->ring edge; the ELF-5 `:: EL0: input test … ::`
    witness proves ring->EL0-drain; together they cover the full path. **HONEST QEMU NOTE:** the real HID *edge*
    (a USB keypress landing in `EVENT_QUEUE`) is metal-only — QEMU raspi4b delivers no USB HID — so the selftest
    drives the router with a synthetically pushed event. **What an interactive EL0 program can now do:** an ELF-3
    surface + ELF-2 threads app run via `run <path>` receives live keyboard and mouse from real hardware through
    `SYS_INPUT_POLL`, drawing/responding to input — the last delivery gap in the EL0-vug ladder.
    Lane: `main.rs` (the router branch + `route_input_to_active_el0` + `input_router_selftest`) + the
    `run_user_image` focus-registration call site in `arch/aarch64/syscall.rs` + this doc.
- **UVUG-3 (landed 2026-07-23)** — the mini-vug becomes the first **interactive** EL0 application. `crates/user-uvug`
  is rewritten from the UVUG-1 animated gradient into a real **vug-style wireframe quartz crystal**: the Q16.16
  fixed-point sin table, rotation (yaw-then-pitch), and the 14-vertex elongated hexagonal bipyramid are
  reimplemented in the user crate from the kernel `vug.rs` geometry (integer math, no float), projected
  screen-space to the surface and drawn as 30 Bresenham edges. **Surface size:** SYS_FB_MAP exposes exactly one
  page (32×32 ARGB8888 — `boot::FB_SURFACE_W`/`FB_REGION_SIZE`, out of this lane), so the crystal is scaled to
  32×32; the brief's "e.g. 256×256" is capped there until the kernel FB region grows. The two persistent worker
  threads (one co-located, one sibling-core) keep the ELF-3 futex frame barrier, but now each **rasterises its
  half** of the surface (worker A rows 0..16, B rows 16..32): it clears its band and draws every crystal edge
  clipped to its band from the shared projected-vertex arrays the parent publishes each frame (release/acquire
  on the `phase` word carries the `PX`/`PY` writes). Input folds into the parent's per-frame state exactly like
  kernel game-mode: **SYS_INPUT_POLL** is drained each frame; WASD/arrows are TRUE held-state (KeyDown sets a
  movement bit, KeyUp clears it) driving yaw/pitch, Q/E zoom the camera distance, a mouse drag (Button-press →
  MouseRel while held → Button-release) rotates, and a click or ESC exits cleanly (workers signalled via a
  `PHASE_EXIT` sentinel, both joined, exit 0). **Dual-path (the UVUG-3 crux):** if NO input event arrives within
  the first `DETECT_FRAMES` (60) frames — always true under QEMU raspi4b, which has no USB HID — the program
  COMMITS to the deterministic auto path: it keeps the fixed idle tumble it ran from frame 0 (yaw += 3, pitch +=
  1 brad/frame), runs to `AUTO_FRAMES` = 300 total, FNV-1a-checksums the final surface, and prints the **unchanged
  witness** `:: UVUG: frames=300 threads=2 checksum=<hex> ::` before exiting 0 (the checksum is a pure function
  of the frame-300 geometry, so it is deterministic and thread-interleaving-independent). The interactive path is
  metal-only, runs until an exit event (bounded by `INTERACTIVE_CAP` = 36000 frames), and prints
  `:: UVUG: interactive exit=<key|click> frames=<n> ::`. The program is now written in Rust (a tiny `_start` in
  `.text.entry` + the worker entry, syscalls via inline-asm helpers) rather than a single asm stream, staying
  position-independent (relocation-model=static → adrp/add, **zero relocations** in the linked image; verified)
  and fitting the 16 KiB window (12568-byte ELF, two PT_LOAD segments, per-segment W^X). QEMU-verified
  (`UNAOS_V3D=1 UNAOS_GENET=1 UNAOS_PIUSB=1 ./arroyo kernel8-test`, and again with `UNAOS_VUGPAR=1`; reproducible):
  `:: UVUG: frames=300 threads=2 checksum=0x48221e4101db3924 ::` *(superseded by WC-C -> `0xe68285b85121ac7c`)* then `:: EXEC-UVUG: run /fat/UVUG.ELF — loaded
  12568 bytes, entry 0x270000, exit=0 -> PASS ::`, with the whole prior battery (CAPSTONE 6/6, EXEC1, ELF-2
  threads, ELF-3 fb, ELF-5/INPUT-WIRE input router+drain) byte-equivalent. The auto checksum is a NEW deterministic
  value (0x48221e4101db3924 *(superseded by WC-C -> `0xe68285b85121ac7c`)*, vs UVUG-1's gradient 0x0313e510f24daae5 — the rendered content changed from gradient
  to wireframe). **Metal is where the interactive path lights up:** at the panel, `run /fat/UVUG.ELF` now shows a
  rotating wireframe crystal (via the UVUG-2 present hook) that the operator drives with WASD/arrows/Q/E and the
  mouse, exiting on a click or ESC — the `:: UVUG: interactive exit=… ::` line is the metal-only witness. Lane:
  `crates/user-uvug` only (no kernel/syscall/boot/arroyo change — same crate, linker script, and build/stage
  path) + this doc.
- **UVUG-4 (landed 2026-07-23)** — makes the interactive switch **input-driven** instead of time-boxed. P46 metal
  showed UVUG never entered interactive mode: the old `DETECT_FRAMES` (60) fallback window elapsed in well under a
  second at EL0 frame rates — the auto path committed and the checksum ran before a human could touch a key. The
  fix drops the detection window entirely: the parent polls **SYS_INPUT_POLL every frame for the program's whole
  life**, and the FIRST input event AT ANY FRAME flips it to interactive permanently — cancelling the auto-tumble
  and the 300-frame cap and switching to held-state control. QEMU determinism is preserved automatically: raspi4b
  has no USB HID, so zero events ever arrive, the 300-frame auto path + FNV-1a checksum run identically, and the
  witness is byte-for-byte the same (`:: UVUG: frames=300 threads=2 checksum=0x48221e4101db3924 ::` *(superseded by WC-C -> `0xe68285b85121ac7c`)*, reproducible
  across `kernel8-test` and `UNAOS_VUGPAR=1`). Two new/changed witnesses: `:: UVUG: interactive takeover at frame
  <n> ::` prints at the switch (proving on metal that the input arrived and at which frame), and the exit line is
  unchanged (`:: UVUG: interactive exit=<key|click> frames=<n> ::`). Drag-rotate is retuned toward the kernel
  game-mode feel Peter flagged as awkward: the kernel `vug.rs` maps pointer motion 1 px = 1 brad with no scaling,
  so instead of copying that this arc scales pointer delta down (`DRAG_DIV` = 8) and per-frame clamps it
  (`DRAG_CLAMP` = 64 brad/axis) so a **full-panel drag ≈ one revolution** (256 brads over ~2048 px, panel ~1920 px
  wide) and no single HID delta can spin past a quarter-turn in one frame. Lane: `crates/user-uvug` only + this doc.
- **UVUG-5 (this arc)** — two input-side corrections after the P47 metal capture (`run /fat/UVUG.ELF` ran the
  300-frame auto batch to `exit=0` with the unchanged checksum, but showed **no interactive takeover** and a
  spurious `[gui] watchdog app wedged 5s`). (1) **Watchdog false-fire** — the `run` command arms `gui_watchdog`
  via `on_app_enter`, but nothing fed `note_progress` on the EL0 path, so a healthy polling app was declared
  wedged at 5 s and the shell was handed the keyboard back mid-run; fixed by feeding `gui_watchdog::note_progress`
  from `sys_input_poll` (see the corrected ELF-5 decision above). (2) **Router-delivery witness** — the
  router→ring code path is verified correct (`el0_input_active()` precedence in `pump_usb_into_gui`;
  `current_asid()` == the `el0_input_set_active` ASID both derive from `TTBR0_EL1[63:48]`), so the metal
  no-takeover is a HID-delivery/timing question QEMU (no HID) cannot reproduce; a rate-limited
  `[el0in] routed N event(s) to active EL0 ring` line now fires the instant the router hands the active EL0 app
  real input, so the next sitting reads delivery directly instead of inferring it. (3) **Host-side typematic** —
  a USB HID boot keyboard under `SET_IDLE(0)` (which the held-state contract requires) never auto-repeats, so a
  held key produced exactly one `Event::Key` everywhere; key repeat is the host's job. The USB pump
  (`pump_usb_into_gui`, ~250×/s) now tracks the most-recently-pressed key from the Key/KeyUp edges it drains and,
  once it has been held past `TYPEMATIC_DELAY_MS` (400 ms), injects a fresh `Event::Key` into `EVENT_QUEUE` at
  `TYPEMATIC_RATE_MS` (40 ms ≈ 25 chars/s). Injecting into `EVENT_QUEUE` means the repeat rides the SAME routing
  every real key takes — shell (`GUI_CHANNEL`), a kernel full-screen app's own `pump_and_poll` drain, AND an EL0
  app's per-process ring — with no per-path code. Newest key wins; releasing the repeating key stops it. QEMU
  raspi4b delivers no HID, so no key is ever held and no repeat is synthesised — the deterministic auto paths stay
  byte-identical (`checksum=0x48221e4101db3924` *(superseded by WC-C -> `0xe68285b85121ac7c`)*, verified on `kernel8-test`). Lane: `arch/aarch64/syscall.rs`
  (`sys_input_poll` heartbeat) + `main.rs` (typematic + `[el0in]` witness) + this doc.
- **UVUG-6 (this arc)** — the UVUG-5 typematic tracker observed Key/KeyUp edges as they were **drained out of**
  `EVENT_QUEUE`. `EventQueue::push` silently **drops** on a full 64-slot ring, so a `KeyUp` pushed while the
  queue was saturated was never enqueued, never drained, never observed: the tracker held the key forever and
  injected `Event::Key` every 40 ms, which kept the queue full, which dropped every subsequent real edge — a
  **self-sustaining wedge** matching the P51 metal capture (keyboard events stop, no detach, repeat broken).
  Fix — re-root the observation at the **HID report level, before any queue push**. The state + logic moved into
  `pal` (`typematic_note_report` / `typematic_tick`); `drivers::xhci` feeds each keyboard report's newest press
  and full held-ascii set directly, so a **release is learned from the report** (armed key absent from the held
  set) and cannot be dropped by the queue. Three disarm layers cover every miss class: (1) report-level release;
  (2) keyboard-detach generation (UVUG-5, unplug-mid-hold); (3) **positive liveness** — a still-"held" key with
  no HID report for ~1 s is dropped (catches a release report that never reached the decode). Plus a
  **backpressure guard**: `typematic_tick` refuses to inject while `EVENT_QUEUE` is past half full, so a stuck
  repeat can never saturate the ring and starve real input. The drain-fed `typematic_observe` is gone. A QEMU
  witness (`typematic_selftest`, run once on the BSP) proves all three legs — baseline repeat, backpressure
  suppression, and dropped-`KeyUp` disarm — emitting `:: uvug6: typematic … :: PASS ::`. QEMU delivers no HID,
  so no report is ever fed on the boot path and the deterministic auto paths stay byte-identical. Liveness note:
  on a keyboard that sends **zero** reports while a key is held still (strict `SET_IDLE(0)`, no idle re-reports),
  the 1 s liveness stops an ongoing repeat while physically held — a benign, self-correcting degradation
  deliberately preferred over the catastrophic wedge it guards. **UVUG-9 retired that trade** — see below.
  Lane: `pal.rs` (tracker + queue depth) + `drivers/xhci/mod.rs` (report-level feed) + `main.rs` (pump call +
  selftest) + this doc.
- **WC-B (this arc)** — the **window surface/verb seam**: the syscall half of the window-compositor arc
  (unit WC-A owns the compositor core in `video/`). Four new verbs, `SYS_WIN_CREATE = 29`,
  `SYS_WIN_PRESENT = 30`, `SYS_WIN_MOVE = 31`, `SYS_WIN_CLOSE = 32` (28 stays reserved for the deferred
  `SYS_INPUT_WAIT`). ELF-3 exposed exactly ONE 4 KiB surface per process; WC-B turns that into **8 window
  surface slots of 64 KiB each**.
  - **FB region.** The reserved VA hole above the 16 KiB program window grew from `0x2000` to `0x81000`:
    the RO info page, then 8 × 64 KiB window slots (`boot::FB_WIN_SLOTS`, `FB_WIN_SLOT_SIZE`). 64 KiB is
    exactly a 128×128 ARGB8888 surface — the arc's maximum (`FB_WIN_MAX_W/H`). A surface's size is
    **negotiated at create** and only its `ceil(w*4*h / 4096)` pages are mapped; the rest of the slot keeps
    its reserved EL1-only identity leaf, so a process that asked for 32×32 cannot reach the remainder of
    its own slot. Every mapped surface page uses the **same `user_data_page` shape** (EL0+EL1 RW, UXN,
    Normal-cacheable, nG) the single ELF-3 surface page had — no MMU attribute changed.
    `map_slot_fb_win` / `unmap_slot_fb_win` do proper **break-before-make** per page; unmap restores the
    boot `L3_USER` descriptor, so a closed surface is unreachable from EL0 the instant the TLBI completes.
    `map_slot_fb_info` is **idempotent** — it skips the leaf edit entirely when the descriptor is already
    correct, because `SYS_WIN_CREATE` (unlike `SYS_FB_MAP`) carries no "before any thread spawns" ordering,
    and a break-before-make on a live info page would fault a sibling thread on a correct program.
    The VA anchor `USER_REGION` is now `align(0x100000)` (≥ its `0x85000` size) so the region still cannot
    straddle a 2 MiB `L3_USER` block; the per-slot **backings** got their own page-aligned type, since a
    backing is consumed one page at a time and does not need the anchor's alignment.
  - **Slot recycling scrubs the FB region.** `build_slot` zeroes `[USER_REGION_SIZE, USER_STATIC_SIZE)` of
    the slot's backing. Teardown retires the ASID and its mappings but never the backing bytes, and the
    loader only writes the 16 KiB program window — so without this a recycled slot's `SYS_WIN_CREATE` would
    map up to 16 pages of the PREVIOUS tenant's frame back in, EL0-RW. Build (not map) is the scrub point:
    it is exactly the recycle boundary, runs once per tenant, and cannot wipe a caller's own pixels the way
    zeroing on map would for a second (documented-idempotent) `SYS_FB_MAP`.
  - **Two indices, deliberately distinct.** The **window id** (0..8) is global — what EL0 passes and what
    the compositor names a window by. The **region slot** (0..8) is per-address-space — which 64 KiB
    surface slot of the *owner's own* FB region backs it. Region slots are allocated lowest-first per
    ASID, so a process's first window is always region slot 0, at the VA the single ELF-3 surface used.
  - **Compat is exact.** `SYS_FB_MAP` and `SYS_FB_PRESENT` are wrappers over "window 0" = the caller's
    region slot 0: `SYS_FB_MAP` returns the **same VA** as before and writes the **same legacy info-page
    header** (magic, w, h, stride, format, size, surface-offset at offset 0), so the existing `UVUG.ELF`
    binary runs unchanged. Per-window geometry is published alongside it at `0x40 + rslot*0x20`
    (magic, win id, w, h, stride, format, surface-offset), zeroed on close so EL0 can tell live from stale.
    `SYS_FB_PRESENT` deliberately does **not** require a window-table row — it presents the region-slot-0
    surface with the legacy accounting either way, so the ELF-3 fb-test verdict cannot regress on table
    exhaustion.
  - **Ownership is authoritative in the syscall layer**, not the compositor: every verb resolves the
    caller's ASID from `TTBR0` (`current_asid`, the same read the handle gates use) and refuses a window
    it does not own, errno-for-errno with those gates — `-EBADF` for an out-of-range or free id (the
    `sys_close` shape), `-EACCES` for a live window owned by another ASID (the rights-denial shape). The
    table is a `SpinMutex` taken IRQ-masked via `IrqGuard` on every access (syscall context AND the
    IRQ-masked teardown path), held across the MMU maintenance so a create and a close on two cores cannot
    interleave break-before-make on the same leaf. **Both present verbs hold that lock across the composite
    itself**, not merely across the ownership check: a close+create pair on other cores can recycle a window
    id, and a validate-then-drop-then-present would land the caller's pixels under the new owner's window
    identity. `WINDOWS` is a leaf lock and `video::wm` state is acquired strictly inside it.
  - **Teardown.** `clear_handle_row` closes every window the dying ASID still owns — the window twin of the
    handle/file/input/latch clears, and for the same reason: a surviving row would name a surface inside a
    backing frame the slot's NEXT tenant gets, compositing the next program's private memory to the panel.
  - **UVUG-8 invariants preserved.** `SYS_WIN_PRESENT` and `SYS_FB_PRESENT` run one shared body, so the
    **focus-scoped** `EL0_FOCUSED_PRESENT_COUNT` bump (under the same `EL0_INPUT_ACTIVE == asid` guard)
    happens for window presents too — a window verb that skipped it would hand a focused app a way to
    render forever without counting as progress, defeating the UVUG-8r2 suspension cap. Takeover latch,
    heartbeat and orphan lifecycle are untouched.
  - **Integration seam.** WC-A's `video::wm` does not exist in this unit's worktree (the units are
    lane-disjoint and concurrent), so the four compositor calls go through a stateless private `wc_shim`
    module, each call site marked `// WC-INTEGRATION:` with the real API it stands for
    (`create`/`present`/`move_window`/`destroy`). The shim carries no state and no policy — ownership,
    geometry validation and surface mapping all live in the gates above it — so the swap cannot move a
    security check. Until then `present` forwards to the ELF-3 present hook, i.e. today's behaviour.
- **UVUG-9 (this arc)** — three P54b metal defects, root-caused from the instrumentation the previous arcs
  installed rather than from fresh guesses. QEMU raspi4b delivers no HID, so all three are metal-verified only;
  the gates below prove no regression.
  - **The interactive freeze** (crystal stops mid-animation, program keeps polling, presents stop, the 60 s
    no-render cap fires). Confirmed **by the kernel's own witness algebra**: holding the takeover suspension for
    `TAKEOVER_SUSPEND_MAX_SECS` requires the heartbeat — stamped *only* by `sys_input_poll` — to stay fresher
    than `TAKEOVER_STALE_SECS` on every pass, while `EL0_FOCUSED_PRESENT_COUNT`, bumped by `sys_fb_present`
    under the **identical focus predicate**, never moves. Polling forever while presenting never is a state only
    one phase of UVUG's frame loop can occupy: the input drain, which UVUG-8 wrote as an **unbounded**
    `loop { poll() }` running until the ring reported empty. Fix — a per-frame drain budget
    (`MAX_DRAIN_PER_FRAME` = 64, twice the kernel's `INPUT_RING_CAP`); leftovers are consumed by the next
    frame, so the render/present half of the loop is always reached and the freeze becomes input latency at
    worst. The futex barrier was **refuted** as the cause, not merely doubted: `sched::futex_wait` compares
    `*uaddr` against `val` under the same bucket lock `futex_wake` takes, so the park is race-free by
    construction, and a parked parent could not have kept the heartbeat fresh anyway.
  - **Present errors were invisible.** UVUG-8 discarded `SYS_FB_PRESENT`'s return, so a present that began
    failing mid-run would have been indistinguishable from a freeze. It is now checked and witnessed.
  - **`[uvug9]` stall witness** — an app-side, per-phase progress witness naming *which* phase stopped:
    `[uvug9] stall frame=<n> phase=poll drained=<n>` (drain budget spent with the ring still non-empty — the
    freeze signature, now a diagnosis of a runaway producer rather than a hang), `phase=barrier done=<0|1>`
    (which worker half is missing), `phase=present rc=<errno>`. EL0 has no clock syscall, so the barrier budget
    is counted in **passes**, which is both deterministic and exactly what "made no progress" means here; the
    stated limitation is that it catches a *spinning* barrier, not a *parked* one (that case is refuted in the
    kernel, above). There is no knob channel into EL0, so each phase **self-gates on its own anomaly** and
    latches after one report — a healthy run prints nothing, and the QEMU auto path reaches no anomaly at all,
    which is why its 300-frame checksum is untouched.
  - **Key repeat stopped after ~10 characters at the shell.** Not a mystery: UVUG-6's liveness layer (3) firing
    on a healthy keyboard. A strict `SET_IDLE(0)` boot keyboard sends one report on the press and then
    **nothing** while the key is held still, so `LAST_REPORT_MS` freezes at the press; repeats run from
    `DELAY_MS` (400 ms) to `LIVENESS_MS` (1000 ms) at `RATE_MS` (40 ms) — ~15, fewer once the press report's own
    latency counts. The guard inferred "wedged" from silence on a device class whose correct behaviour *is*
    silence, so it could never have been sound there. Fix — **evidence-gate** it: `typematic_note_report` sets a
    sticky `STREAMS_WHILE_HELD` only on a **true idle re-report** — the armed key still down, no press edge, and
    the held set **byte-identical** to the previous report's — sustained for `IDLE_RUN_TO_LATCH` (4) consecutive
    reports. With that evidence the 1 s window applies unchanged and the P51 wedge stays shut; without it the
    bound becomes `HOLD_MAX_MS` (30 s), a coarse backstop that keeps the pathological case finite while letting
    a held key repeat as long as anyone actually holds one. Cleared on keyboard detach (along with the evidence
    it was derived from), so a swapped device re-earns the verdict. Layers 1 and 2 are untouched and remain the
    real release paths.
    **Why the gate is that strict** — one false latch re-imposes the 1 s window for the whole boot and brings
    the ~10-repeat stop straight back, so the gate is a fix-durability surface in its own right. "No press edge"
    alone would have latched on (a) a two-key **rollover release** (press a, press b, release a — no press edge,
    armed b still held) and (b) a **non-ascii tap** while holding (F-keys map to ascii 0). The byte-identical
    test excludes (a); it cannot see (b), because the ascii projection the tracker receives has already
    discarded the keycode that changed, so both the tap's press and its release arrive as unchanged-held
    reports. The run threshold closes (b) from this side — a tap yields two such reports, a genuinely
    idle-re-reporting keyboard yields them continuously. (Feeding raw keycodes would close it at the source, but
    that is a `drivers::xhci` signature change and outside this arc's lane.) All three cases are now asserted by
    the `uvug6` selftest: legs **(D)** rollover-release and **(E)** non-ascii-tap must NOT latch, leg **(F)**
    genuine idle re-reports MUST — so the wedge guard still arms on the hardware it exists for.
    A `[uvug9] typematic hold-max` line fires when the 30 s backstop is what disarmed a key, so the bench can
    tell backstop from bug; the tight `LIVENESS_MS` disarm stays silent (it is the ordinary end of a hold).
  - **Orphan residue: the GUI watchdog was silently disarmed for the rest of the boot.** `sys_input_poll` called
    `gui_watchdog::note_progress()` **unscoped**. A timed-out run leaves its EL0 task alive and spinning on that
    very syscall (this arch has no asynchronous kill primitive), so one timeout kept feeding the watchdog
    forever — disabling the escape hatch that hands the screen back when a *later* full-screen app wedges. Fix —
    focus-scope it, joining the takeover heartbeat under one `EL0_INPUT_ACTIVE == asid` test; they were always
    the same question. This is the part of the orphan residue that outlives the run.
  - **Mouse dead at the shell after a timeout while arrow keys still work** — **instrumented, not fixed.** The
    asymmetry's likeliest owner is `drivers::xhci`: the dup-Success guard (`mouse_expect_phys`) returns from the
    transfer dispatch **without** calling `queue_mouse_read`, so a single mismatched completion retires the
    pointer read permanently while the keyboard's independently-armed endpoint carries on — on the endpoint that
    generates by far the most traffic. That file is **outside this arc's lane**, so this arc adds the witness
    that decides it instead of changing the driver: `[uvug9] shell-path input key=<n> ptr=<n>` counts keys and
    pointer events separately at the shell-destined drain. Read against the existing `MOUSE-1` xHCI decode
    counter: `MOUSE-1` advancing while `ptr=` is frozen puts the loss in the router/queue seam (this lane);
    both frozen while `key=` advances puts it on the pointer interrupt-IN endpoint (the driver lane).
  - Gates: `./arroyo check` green x86_64 + aarch64; `./arroyo kernel8` builds; `./arroyo kernel8-test` 46 PASS /
    0 FAIL, UVUG batch `frames=300 checksum=0x48221e4101db3924 exit=0` **unchanged** *(superseded by WC-C -> `0xe68285b85121ac7c`)*, `:: uvug6: typematic … ::
    PASS ::` still green (the evidence gate leaves all three selftest legs on their original verdicts). Lane:
    `crates/user-uvug/src/main.rs` + `pal.rs` (typematic liveness) + `arch/aarch64/syscall.rs`
    (`sys_input_poll` focus scope) + `main.rs` (shell-path witness) + this doc.
- **UVUG-10 (this arc)** — the P55b pointer bisect, and the boot fixture that was costing a core every metal
  boot.
  - **`ptr=0` forever, from boot.** P55b read `[uvug9] shell-path input key=<n climbing> ptr=0` on metal for
    the whole boot, while the xHCI `MOUSE-1` witness reported a live pointer — the mouse was never lost *after*
    a takeover focus-drop, it never worked at all. UVUG-9 framed that reading as deciding between "the pointer
    stopped being decoded" and "the pointer is decoded but does not reach the shell path", but the two
    witnesses bracket `pal::EVENT_QUEUE` without measuring it, so the reading was not actually decisive: both
    the driver's `push_event` and the queue itself sit inside the unmeasured gap.
  - **The loss is at or after `EVENT_QUEUE` — settled, not suspected.** The driver's
    `push_event(Event::Mouse)` (`drivers/xhci/mod.rs:2278/2286`) precedes the `MOUSE-1` print in
    straight-line code with **no platform fork between them**, so P55b's `last dx=3 dy=5` is direct proof
    that pointer events were pushed. An earlier all-zero-report-buffer suspect is refuted by that same fact,
    and UVUG-9's dup-Success-guard theory is independently weakened (`queue_mouse_read` and
    `queue_keyboard_read` are structurally identical in their `expect_phys`/`prev_phys` bookkeeping, so a
    guard mis-firing on the pointer would mis-fire on the working keyboard).
  - **Unified theory — and this arc's fixture gate is probably also the mouse fix.** The boot
    `input_launcher` orphan held `el0_input_active()` for the *entire* boot, so the router's EL0 branch
    (`route_input_to_active_el0`) swallowed the whole queue into a ring nothing would ever read. Keys still
    reached the shell because `input_service`'s UART path calls `gui_send` **directly** and never touches
    `EVENT_QUEUE` at all. One mechanism, no second defect, and it accounts for every observed fact including
    "from boot" and "keys but never pointer". Gating the fixture off metal removes the orphan, and with it
    the standing focus.
  - **`[uvug10] evq` — the measurement that decides it on the wire.** `pal::push_event` classifies every
    offered event (pointer / key) and, because `EventQueue::push` silently discards on a full ring, counts
    the **drops** separately; `pop_event` counts consumption across *every* consumer. `event_queue_stats`
    exposes the five totals and the router prints
    `[uvug10] evq push ptr=<n> key=<n> / drop ptr=<n> key=<n> / pop=<n> depth=<n>`, throttled by a pass
    counter (raspi4b's `ms()` is pinned at 0, so a wall-clock throttle would flood) and emitted only when a
    counter moved. Two calibrations the reader needs. **Baseline:** the boot selftests are producers —
    `input_router_selftest` pushes a synthetic `Mouse{3,-4}` — so "never produced" reads **`push ptr=1`**,
    not `0`; QEMU's own line is `push ptr=1 key=38 / drop ptr=0 key=0 / pop=40`. **Re-circulation:** the
    `SCREEN_APP_ACTIVE` peek drains and re-pushes the ring every pump pass (~250×/s) while a kernel app owns
    the panel; that is neither production nor consumption, and counting it would inflate push/pop/drop
    exactly in the state where a stalled drain is the hypothesis under test — so the peek now runs through
    an uncounted seam (`pal::peek_event_uncounted` / `pal::requeue_event`) and the totals keep meaning
    "entered the pipeline once" / "left it for good". Drop accounting earns its place regardless: a moving
    mouse produces ~125 events/s against a handful of keystrokes, so any drain stall starves the pointer
    class first, and did so invisibly before this arc.
  - **P56 verdict table.** Expected: `[uvug9] ptr` climbs normally with a moving mouse and no orphan is
    alive — the orphan theory held and nothing further is owed. If `[uvug9] ptr` is **still 0 with no
    orphan**, the theory is refuted and the hunt resumes at or after the queue: `push ptr>1` with
    `drop ptr≈push ptr` is a saturated ring behind a stalled drain; `push ptr>1` with `drop ptr=0` means a
    second consumer takes them first (`pop` far above the router's own `[uvug9]` totals names it — an EL0
    focus ring, or the focus-change pre-launch discard); `push ptr=1` unmoved should be unreachable given
    the settled xHCI finding, and would put the question back in the driver lane.
  - **The boot interactive-launcher fixture no longer runs on metal.** ELF-5's `input_launcher` is a *real*
    interactive takeover — it registers itself as the active input target and spawns an EL0 program that
    spins on `SYS_INPUT_POLL` until it drains the one event the launcher injects. That premise holds only
    where the kernel injection is the sole producer. On metal, live HID traffic reaches the same ring first,
    the program's fixed three-observation script stops matching, and the launcher's 2 s bounded wait expires
    **with the EL0 task still alive**; with no asynchronous kill primitive on this arch the abandoned program
    then spins its poll/yield loop for the rest of the boot — the pause during boot plus a core pegged at
    100%, on every boot, that also starved later runs (`EXEC1` FAIL). It is now gated on
    `timer::is_live()`, the same metal/QEMU discriminator `main` already uses to decide which service tasks
    to spawn, and prints an uncounted `:: EL0: input test — SKIP on metal … ::`. **Why that gate:** a
    HID-presence test would race asynchronous enumeration and run the fixture anyway behind a slow device,
    and the real objection is not "a mouse is plugged in" but "this is an interactive machine whose input
    belongs to the user"; a compile-time knob was rejected because the metal image and the QEMU battery
    image are built from one feature set, so the knob would have to be remembered at every flash and a
    forgotten knob silently restores a boot-time core leak. QEMU keeps the fixture and its verdict verbatim.
    **Recorded residual:** `timer::is_live()` is a *proxy* — it asserts "the Group-1 timer IRQ was observed
    delivering", not "this is hardware", so a metal boot whose Group-1 delivery failed reads `false` and
    reinstates the orphan. Accepted rather than papered over: the same proxy already gates `rx_backstop` /
    `status_tick` / `usb_pump`, so a board where it lies has already lost its timer-driven services and the
    orphan is not what the bench would be chasing, and the failure is loud (`AARCH64: timer heartbeat live`
    is absent from such a capture). A real "am I on hardware" predicate belongs with platform
    identification, not with a fixture gate.
  - Gates: `./arroyo check` green x86_64 + aarch64; `./arroyo kernel8` builds; `./arroyo kernel8-test 120`
    **46/46 required witnesses, 0 forbidden**, UVUG batch `checksum=0x48221e4101db3924` **unchanged**,
    `:: uvug6: typematic … PASS ::` green, `:: EL0: input test … PASS ::` still green in QEMU. Lane:
    `pal.rs` (queue accounting) + `main.rs` (router witness) + `arch/aarch64/syscall.rs` (fixture gate) +
    this doc.
- **SKILL-1 (this arc)** — the **asynchronous kill primitive**, and the retirement of the orphan.
  Every arc above had to work around the same missing piece: `sched::exit` retires only the **calling**
  task, so nothing could stop a task that had stopped cooperating. `run_user_image`'s `Timeout` path could
  only park the `Proc` row `PORPHANED` and walk away, and the metal cost of that was severe and cumulative
  — the abandoned EL0 task kept **spinning a core at 100 % forever**, kept **rendering over the shell's
  screen**, and starved every later run (observed on two boots as `EXEC1 … did not exit in time` on the run
  *after* a timeout). UVUG-9's watchdog fix above ("orphan residue") treated one symptom of exactly this.
  - **A kill is a request, never register surgery.** `sched::kill(tid, asid)` publishes an entry in a
    four-slot table; the target retires **itself, on its own core**, at a boundary where switching away for
    good is already proven safe. No cross-CPU stack or register is ever touched. Task ids are monotonic and
    never reused, so a request cannot be mis-delivered to a later task.
  - **The unit of a kill is the ADDRESS SPACE, not the thread.** An ELF-2 program can hold several tasks
    under one ASID (`SYS_THREAD_SPAWN`), so a request that named only the tid `run_user_image` happens to
    hold would retire one thread while its siblings kept running — still spinning, still rendering — under
    a row we had just reported *reaped*. (Nothing unsafe: `teardown_user_slot` is refcounted and only the
    last thread out retires the slot. The defect is a **false claim**, which is worse for being silent.) A
    non-zero `asid` widens the request to every task in the slot; both arms match on it, and a per-ASID
    live-task count (`ASID_THREADS`, maintained at every user spawn and every task death) makes
    `kill_settle` **withhold the confirmation until the last sibling has retired**. If some sibling never
    does, the witness says so outright — `confirmed=0 row=orphaned — <n> sibling thread(s) survive` — and
    the row stays parked. A reap is only ever reported when the whole process is gone.
  - **Slot-protocol races, all resolved by CAS.** `kill_settle` transitions with a
    `compare_exchange(PENDING → DONE)`, never a load-then-store: it races `kill_detach` at precisely the
    worst moment (a requester's bounded wait expiring is exactly when a slow kill lands), and a
    load-then-store would stamp `KILL_DONE` over `KILL_DETACHED` — a terminal state with no owner left to
    release it, four of which retire the primitive for the boot. Losing the CAS now means the state is
    `DETACHED` and the slot is freed **inline**, on the owner-less path. A late settle likewise frees only
    a slot it can CAS out of `DETACHED`, so a slot another requester has re-armed can never be clobbered.
    `kill_retract` closes the mirror-image leak: the `PRUNNING` guard before arming is a *sampled* read,
    and the task can complete its whole `SYS_EXIT` in the gap, leaving a request nothing can ever settle —
    so the requester **re-checks the row after arming** and releases the slot at once if the target is
    already gone. Exhaustion is never silent: `[skill] slot table EXHAUSTED — reverting to PORPHANED`
    prints once per boot.
  - **Two arms.** *(a) OFF-CPU* — `dispatch_next` checks the table after popping a task and **before**
    switching into it: a killed task is never dispatched again, and is torn down on the scheduler's own
    stack. This arm needs no cooperation from the target at all and covers ready, sleeping and wait-queued
    tasks alike, since they all re-enter through a dispatch. *(b) ON-CPU* — `timer_preempt`, `yield_now`
    and the SVC dispatcher call `kill_check_current`, which routes a killed current task straight into
    `exit()`. The quantum tick is the load-bearing one: it is the **only involuntary boundary a spinning
    EL0 task ever reaches**, and it is what turns the 100 %-core orphan into an actual death (~one tick,
    ≈4 ms, on metal). All three call sites already run IRQ-masked on the task's own kernel stack and
    already tolerate a never-returning `exit()` — that is precisely what the M6b EL0 fault-kill path does.
  - **Teardown mirrors `exit()`** in both arms: `boot::teardown_user_slot(asid)` first (it repoints the
    core's `TTBR0` off the dead root before the ASID broadcast-flush and the slot release), then the
    `done_sem` post so a `JoinHandle` is never left blocked, then the kernel stack, and the kill slot is
    settled **last**. The two arms are **deliberately asymmetric about the stack**, and the confirmation
    contract is written to the weaker of them: the off-CPU arm owns the Box and drops it before publishing
    `KILL_DONE`, whereas `exit()` is still *executing on* that stack and cannot, so there the stack is
    reclaimed moments later by whichever core takes the Box. What both arms guarantee at confirmation — and
    all a requester may rely on — is: **the address-space slot is torn down, the joiner is released, and
    the task will never execute again.** Kernel-stack reclamation is scheduler-internal and explicitly not
    part of the contract.
  - **Confirmation is what licenses the reap.** `run_user_image` now kills **before** it parks:
    `kill` → bounded wait (`KILL_CONFIRM_SECS` = 2 s, wall-clock off CNTPCT, yielding so the co-located
    target reaches a dispatch) → on confirmation the row is **reaped outright**, because nothing is left
    alive to post a late status onto a recycled entry, which was the entire reason `PORPHANED` existed. If
    the task *did* manage a real `SYS_EXIT` or fault-kill on its way out (a boundary is where those land
    too), the row is already `PEXITED` and is reaped through the ordinary status+`done` path instead.
  - **Fail-closed everywhere.** No free kill slot, or a kill that does not confirm inside the bounded wait,
    degrades to **exactly** the pre-arc `PORPHANED` behaviour — never to freeing a row under a live task.
    A detached request stays **armed**, so the task still dies at its next boundary, and
    `note_killed_task_retired` reclaims the parked row when it does (scoped to `PORPHANED` rows only, so it
    can never race the confirmed path or touch a live parent/child row). Witness, printed only when a kill
    actually happens: `[skill] killed pid=<n> asid=<n> confirmed=<0|1> row=<reaped|orphaned>`.
  - **What a boundary-driven kill cannot reach — stated, not glossed.** A task **blocked on a wake that
    never comes** (a semaphore nobody posts, a futex nobody wakes) is in neither a run queue nor on a CPU:
    it reaches no dispatch and no preemption, so *neither* arm can see it, and its request stays
    `KILL_DETACHED` for the boot. Sweeping the wait lists at detach time is the real fix and is
    deliberately **not** attempted here — the waiters live behind `Semaphore`'s and `FutexBucket`'s own raw
    spinlocks, and reaching into them from an arbitrary requester context would invert the lock order the
    park-side handoff depends on (`Semaphore::wait` pushes the Box *while holding* that lock). That is a
    wait-queue-ownership change, not a scheduler-kill change, and belongs to its own arc. The same is true
    of a task that reaches no boundary at all (the QEMU case; on metal the quantum tick always arrives).
    Both are bounded and **visible**: the table is four entries, every other path returns its slot, and the
    resulting exhaustion is witnessed rather than silent.
  - **QEMU proves both arms; only the timer *trigger* is metal-only.** The new `:: SKILL-1: async kill …
    :: PASS ::` witness runs cooperatively on the boot core (before `SCHED_ACTIVE`, self-contained and
    bounded, like the `prio_mix`/`load_accounting` witnesses beside it) and drives `dispatch_next` by hand,
    so every step is observed rather than raced: an immortal victim is shown **running**, then killed and
    shown **frozen** (never dispatched again — the actual claim, not merely "eventually stopped"), the
    request confirms in **one pass**, the queue drains; a second victim arms its own kill *while running*
    and is retired by `kill_check_current`; a third leg drives the **detach-vs-settle interleave** (arm,
    detach while the victim is still off-CPU and un-reaped, *then* let the retirement land) and asserts the
    slot comes back `KILL_FREE` — the exact ordering that stranded a slot before the CAS settle, and
    verified to have teeth by a negative control (restoring the load-then-store settle fails that leg, and
    only that leg: `det_freed=false pool=false`, everything else still true). Both tickets release, the
    slot pool returns clean, and a fresh task then spawns, runs and exits. What QEMU raspi4b **cannot**
    show is the timer-driven trigger: it
    delivers no Group-1 timer IRQ, so `timer_preempt` never fires and a genuinely spinning task (no yield,
    no syscall) could not be interrupted there at all — it would wedge the single cooperative core. That
    trigger rides the bench.
  - Gates: `./arroyo check` green x86_64 + aarch64; `./arroyo kernel8` builds; `./arroyo kernel8-test 120`
    **46/46 required witnesses, 0 forbidden**, UVUG batch `frames=300 checksum=0x48221e4101db3924 exit=0`
    **unchanged**. Lane: `arch/aarch64/sched.rs` + the `run_user_image` `Timeout` path and the SVC boundary
    in `arch/aarch64/syscall.rs` + this doc.
- **EXEC1-M (this arc)** — the **late-publish window** in `run_user_image`, and the end of the metal-only
  `EXEC1` failure. On every real Pi 4 boot (P55b, P56) the run-path witness reported
  `:: EXEC1: run /fat/ELFHELLO.ELF — EL0 program did not exit in time -> FAIL ::`, while QEMU raspi4b
  reported `exit=0 -> PASS` from the same image. UVUG-10's fixture gate above removed the boot-time input
  orphan and left cpu2 idle at boot, and `EXEC1` still failed — so orphan starvation was never the cause.
  - **What actually happened.** `run_user_image` cannot know the pid until `sched::spawn_user_slot`
    returns, so it spawned first and published `PROCS[pi].asid` / `.pid` afterwards. The comment justifying
    that order — "the caller yields only in the wait loop below, so the pid is always stored before the
    exit path's `proc_find_running` lookup" — is the *cooperative* reading of the `sys_spawn` co-location
    invariant, and this scheduler is **preemptive**: a quantum tick dispatches the co-located EL0 task with
    no yield involved. The window is therefore real, and on metal it is **milliseconds wide**, because
    `spawn_user_slot` pushes the task onto the run queue and only *then* emits its
    `:: SCHED: task '…' -> core N ::` placement line — a ~70-byte 115200-baud UART write, ≈6 ms. ELFHELLO
    is three syscalls long (write, report, exit), so it ran to completion inside that print. Its `SYS_EXIT`
    found no row keyed by its pid, fell through to the generic counters, and retired. The row stayed
    `PRUNNING` forever; `run_user_image` waited out its whole 5 s deadline **on a task that had already
    exited correctly** and returned `Timeout`. The SKILL-1 kill that follows then targeted an
    already-retired task and could never confirm — which is exactly the
    `[skill] killed pid=108 asid=1 confirmed=0 row=orphaned` in the P56 capture, and why the kill primitive
    looked like it was failing when it was in fact being handed a corpse.
  - **Why QEMU was green.** Nothing about the code differs; only the width of the window does. QEMU's
    serial write is effectively free, so the gap between the run-queue push and the pid store is
    nanoseconds and the tick essentially never lands there. This is the ordinary timing class — the null
    hypothesis was our code throughout, and it held.
  - **The fix: make the ASID the key that survives the window.** The ASID is known *before* the task can
    exist, so it is now stored **before** the spawn. `SYS_EXIT` gains a rescue arm after the pid lookup:
    `proc_find_unpublished(current_asid())` matches a row that is `PRUNNING` **and** `pid == 0` **and**
    carries our ASID — by construction only a row that is mid-publish, since a live EL0 ASID names exactly
    one slot and an ELF-2 sibling's process row already has its pid published. The status and `done` post
    are identical to the live-child arm, so the parent observes `PEXITED` on its next pass. The pid store
    still happens (it remains the key for wait/kill/orphan-reclaim); it is simply no longer load-bearing
    for the correctness of the exit path.
  - **Which other publishers the arm can match — corrected.** The first cut of this write-up claimed
    `sys_spawn` / U7 / U6-grants rows "read `asid == 0` through their own window, so the new arm cannot
    match them". **That was false**, and is recorded here rather than quietly deleted because it planted an
    invariant the code does not have. Those paths spawn first and then store `asid` *before* `pid` — the
    same order `run_user_image` used to — so mid-publish their rows genuinely are `PRUNNING`, `pid == 0`,
    carrying the child's ASID, with the child already dispatchable. **They can match, and matching is
    correct there:** the row is the exiting child's own, so posting its status and `done` is precisely what
    the parent's `sys_wait` is blocked on. The arm silently closes the identical latent race on the spawn
    path. Those windows are a handful of instructions with no UART write inside them, which is why only
    `run_user_image` — whose window contains `spawn_user_slot`'s placement print — was ever wide enough to
    fail on metal.
  - **The two publishers where a match *would* be wrong are fixed at the source.** `proc_free` stored
    `pid = 0` before `asid = 0`, leaving a transient `PRUNNING` / `pid == 0` / *old* asid row — matchable,
    and a match would post a stray `done` permit onto a row about to be recycled, breaking the
    "reaped-then-reused entry always starts at 0 permits" invariant `proc_orphan` documents. It now clears
    the **asid first**, making the row unmatchable for the whole teardown. `u8_kernel_check` plants
    synthetic rows under scratch ASIDs 6/7/8, which are **real allocatable ASIDs** — a genuine unrelated
    process under ASID 7 exiting in the two-instruction window would have been mis-attributed onto a
    synthetic row belonging to no parent. It now plants **pid-first**, so the row is never
    `asid`-matchable; the planted pids (`0xE1`/`0xE2`) are not live task ids, so the pid lookup cannot
    match either. Both are order-only changes with identical settled state.
  - **Proven in QEMU by widening the window, not by argument.** A temporary 100 ms delay inserted between
    the spawn and the pid store reproduces the metal failure in QEMU **exactly**, down to the SKILL-1
    signature: `[uvug8] run deadline expired … parked PORPHANED (pid=102, asid=1)`,
    `[skill] killed pid=102 asid=1 confirmed=0 row=orphaned`,
    `:: EXEC1: … did not exit in time -> FAIL ::`. With the rescue arm restored and the same delay in
    place, the run reports `[exec1] LATE-PUBLISH rescue — EL0 task exited (status=0) before its parent
    stored the pid; Proc row 0 resolved by asid=1 and reaped normally` followed by
    `:: EXEC1: … exit=0 -> PASS ::`. Both probes were removed before the commit; they are recorded here
    because the negative control is what makes the diagnosis a proof rather than a plausible story.
  - **Metal telemetry.** The `[exec1] LATE-PUBLISH rescue …` line is latched once per boot and is the
    arc's one-line verdict on the bench: its **presence** means the window opened on that boot and was
    handled instead of costing a spurious `Timeout`; its **absence** alongside `:: EXEC1: … PASS ::` means
    the window never opened. Either way the FAIL line is gone, and it remains covered by the spec's
    always-on `-> FAIL` forbid, so a regression fails the harness without a spec change.
  - Gates: `./arroyo check` green x86_64 + aarch64; `./arroyo kernel8` builds; `./arroyo kernel8-test 120`
    **49/49 required witnesses, 0 forbidden**, `:: EXEC1: run /fat/ELFHELLO.ELF — loaded 8560 bytes, entry
    0x300000, exit=0 -> PASS ::`; `./arroyo test-arm` green. Lane: `arch/aarch64/syscall.rs`
    (`run_user_image` publish order, `proc_find_unpublished`, the `SYS_EXIT` rescue arm, `proc_free` and
    `u8_kernel_check` store order) + this doc.
- **WC-D (scan-out verification)** — the answer to a P56 bench report of a **garbled** 128×128 windowed
  crystal while every serial witness was green. Full write-up: `docs/dev/OS/08_VIDEO/engine.md` §8 "WC-D".
  Nothing in the userspace ABI changes; what changes is what the kernel can *prove* about it:
  - **`[wc-d] verify` — a panel verdict, not another surface checksum.** `[wc-c]`'s checksum hashes the
    surface an app wrote, so it can only say the app drew; it is blind to every defect between that surface
    and the scan-out. `wm::verify_window` re-derives each destination pixel of a window's content rect from
    the source and reads the framebuffer back **twice**. `bad_cache` is the verdict on the BLIT (stride,
    upscale indexing, colour encoding, clipping). `bad_ram` is read after a bare `DC IVAC` — invalidate
    without write-back — so it reports whether those pixels reached the memory the HVS scans; a
    clean+invalidate there would repair a short flush before measuring it. The line also carries `cksum=`
    and `nonzero=` so a blank-but-faithful PASS is distinguishable from a verified crystal, and a
    guarded-out window emits `-> SKIP` rather than silently burning its one-shot latch. Pinned in
    `pi4-regression.spec` as a REQUIRE plus a `-> FAIL` FORBID.
  - **`UNAOS_FBW` / `UNAOS_FBH` — force the panel geometry.** QEMU raspi4b is 640×480 and the bench Pi is
    1920×1200, and `wm::place` derives a window's integer upscale FROM the panel: the same 128×128 window
    is scale **1** on the gate and scale **4** on the bench. The gate could not reach the bench's blit path
    at all. These compile-time knobs (default off — the firmware mode is queried, unchanged) override the
    mailbox request so `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test` reproduces bench geometry.
  - **What it settled — and what it did not.** At the bench's exact geometry the verdict is
    `checked=262144 bad_cache=0 bad_ram=0 -> PASS`. The `bad_cache` half **earns** the exclusion of
    scale-blit indexing, stride/pitch arithmetic and pixel format as causes of the P56 garble. `bad_ram`
    passing under QEMU earns nothing about metal — QEMU does not model the non-coherent scan-out — so
    coherency remains the live suspect, and that column is what reports it in one line on the next bench
    boot. Flush *extent* is excluded separately, by inspection: `draw_window` flushes whole scanlines over
    the `outer_box`, a strict superset of the blitted pixels.
  - **`bad_ram` is unvalidated off-metal, in both directions.** The falsifier (delete `draw_window`'s
    `flush_range`, re-run at bench geometry) still reports PASS under QEMU — because QEMU raspi4b does not
    model a non-coherent framebuffer, so there is nothing for `bad_ram` to detect. Its correctness rests on
    the primitive being right by inspection, not on a green gate; the next bench boot is its first real
    exercise.
  - **Hazard: a `witness` build is not a neutral observer.** The `IVAC` can drop un-flushed pixels, so
    `verify_window` redraws the window afterwards. In the presence of a flush defect an instrumented
    build's panel can differ from a default build's, in either direction.

- **WC-C (window compositor, clients + focus)** — the arc where real EL0 programs use the window verbs.
  Full write-up: `docs/dev/OS/08_VIDEO/engine.md` §8 "WC-C". Userspace-visible changes:
  - **UVUG is windowed.** `crates/user-uvug` drops `SYS_FB_MAP`/`SYS_FB_PRESENT` for
    `SYS_WIN_CREATE(128, 128)` + `SYS_WIN_PRESENT(id)`. It reads its surface at its own window base +
    `0x5000` (window region slot 0 — the VA `SYS_FB_MAP` returned), which is part of the window ABI, not a
    guess; the kernel also publishes the geometry in the RO info page at base + `0x4000`. `FOCAL` scales
    6 → 24 so the crystal keeps its framing at 4× the linear resolution.
  - **SPEC UPDATE, deliberate — the 300-frame auto checksum changes.** It is a pure function of the
    surface, so a 128×128 surface produces a new one:
    `:: UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c ::`, **superseding
    `0x48221e4101db3924`** wherever it appears in the UVUG-1..9 records above. The brief allowed either a
    compat shim that kept the old value or a deliberate spec change; this is the spec change, because a
    shim would mean shipping the 32×32 render forever to protect a constant. What *is* preserved
    byte-for-byte is the **compat path itself** — `SYS_FB_MAP` + `SYS_FB_PRESENT` still produce the
    identical centred, chrome-less blit, and the ELF-3 fb test's `mapped=32x32 …
    checksum=0x8d99530ca96d4b25` is unchanged. The UVUG-8 takeover/cap line is unaffected:
    `sys_win_present` runs the same present body as `sys_fb_present`, so the focus-guarded
    `EL0_FOCUSED_PRESENT_COUNT` bump happens per window.
  - **midden owns a window.** `MIDDEN.BIN` creates a 24×16 window and renders its own bus stats — two
    rows of four hex digits (live witness bitmask; legs passed) from a packed 3×5 font, blitted 1:1 at
    EL0. Kernel draws border + title strip and nothing else. A refused create makes every repaint a
    no-op, so the bus witnesses never depend on the compositor. Fitting the 4 KiB flat-blob code page
    moved `crates/user-blob`'s release profile from `opt-level = "s"` to `"z"` (3792 B).
  - **TAB is the window system's key.** Reserved at `el0_input_enqueue` (the single router→ring choke
    point, so no app can withhold it) whenever two or more windows are in `wm::focus_ring`; it advances
    `EL0_INPUT_ACTIVE` to the next owner ASID in window-id order via `el0_input_set_active`, so the ring
    reset, the takeover-latch clear and the UVUG-8 cap all keep working per window. The matching KeyUp is
    swallowed with it. Fewer than two windows: TAB is delivered as an ordinary key. The ring carries one
    slot beyond the windows — **the shell** (focus 0) — so tabbing into a window is not a trap. WC-C
    shipped that as a one-way exit (with focus 0 the router never calls the seam); **WC-TAB** closed the
    loop by calling `syscall::wc_shell_focus_key` from `pump_usb_into_gui`'s non-routing paths — a second
    entry point onto the same cycle body, sharing its `n < 2` guard, so TAB re-enters the ring at its
    head and a system with fewer than two windows keeps an ordinary TAB at the shell. The load-bearing
    site is the `SCREEN_APP_ACTIVE` peek/requeue branch, since `run_user_image` parks the shell task for
    the whole EL0 run and that branch returns first; the TAB is consumed inside the scan and the buffer
    dropped, never forwarded (`render_service` is blocked, so a `GUI_CHANNEL` send would saturate it).
    Scope, as WC-C already conceded: the boot's real programs do not overlap, so a ring of two or more
    exists today only under the `el0-wcb` fixture — this completes the mechanism, not yet an operator
    workflow.
  - Gates: `./arroyo check` green both arches; `./arroyo kernel8` builds (per-blob page assertions);
    `./arroyo kernel8-test 120` MBENCH **49/49 required, 0 forbidden** (three new
    `pi4-regression.spec` directives pin the UVUG checksum, the `witness=0x1fff` ledger and the
    side-by-side line — the contracts this arc changed were machine-checked nowhere);
    `:: EL0: window verbs — … witness=0x1fff … :: PASS ::` (the `el0-wcb` ledger widened 10 → 13 bits by
    the side-by-side leg); `:: EXEC-UVUG: … exit=0 -> PASS ::`; all four BANDY verdicts PASS.
    `target/pi-screen.png` **re-baselined by design** (the desktop repaint removes the WC-INT residue) —
    new sha256 `2686a884320dbc389d6c33b1f37b097fa15eba769b51a751449e2c91a986bc19`. Lane:
    `video/wm.rs`, `crates/user-uvug/src/main.rs`, `crates/user-blob` (midden + profile),
    `arch/aarch64/syscall.rs` (TAB seam + the `el0-wcb` fixture) + this doc + `08_VIDEO/engine.md`.
- Not yet: **revocation trees** (a derived copy — re-grant or onward re-transfer — escapes
  single-level revoke today; derivation records + `CAP_REVOKE` are that arc), the **bandy Ring-3
  delegation wrapper**, `File` transfer (descriptor migration), real `Socket` fs/net syscalls.
  Not yet (M8): an arbitrary program-by-name `sys_spawn`, and a code-signing / allowlist gate on the
  loader (`SECURITY.md`).

### x86_64 (branch `hw-rmbp`)

- **U1a** — first ring-3 round-trip (the x86 mirror of aarch64 M6a). A
  scheduled task drops to **ring 3** via `iretq` (`sched::spawn_user` /
  `user_task_trampoline`), runs an embedded position-independent blob that does
  `sys_write("hello from ring 3\n")` then `sys_exit(0)` via **SYSCALL/SYSRET**,
  and the scheduler reclaims it. Syscall ABI mirrors aarch64: number in `rax`
  (`SYS_WRITE = 1`, `SYS_EXIT = 2`), args `rdi, rsi, rdx`, return in `rax`.
  Hardening landed with it: **EFER.NXE**, **NX** on the user data/stack pages,
  W^X on the user code page (mapped ring3-RX, kernel drops WRITABLE before first
  entry), **CR4.SMEP** (CPUID-gated — set on Ivy Bridge metal; TCG `qemu64`
  lacks it, logged), and **TSS.RSP0** so a ring-3 fault lands on a kernel stack
  instead of triple-faulting. QEMU-verified (2026-07-02: `hello from ring 3` +
  `U1a … PASS`, no faults, full boot/SMP/USB regression intact). **Partial metal
  evidence (real 2012 rMBP, 2026-07-02):** `:: SMEP on ::` (the one thing QEMU/TCG
  cannot show — real supervisor-execute protection active), the 1 TiB window
  mapped on the real firmware map (the `CR0.WP` page-table write succeeds on
  silicon), 8 CPUs online. The round-trip's own lines needed a console-output fix
  first — `sys_write` now mirrors to the framebuffer (the rMBP has no 16550) and
  the verdict prints from a BSP-quiet window so the AP's lines aren't dropped by
  fbcon lock contention; round-trip metal re-photograph pending.
- x86-specific shape (adapted from the aarch64 reference, which assumes infra
  x86 lacks): the SYSCALL stack-switch anchors are folded into `PerCpuData`
  (reached via `IA32_GS_BASE`; `KERNEL_GS_BASE` holds that pointer while ring 3
  runs, restored by the entry `swapgs`) rather than a separate struct — the
  scheduler's `this_cpu()` dependency requires GS to stay `&PerCpuData` in the
  handler. The 4 KiB user window at `USER_BASE = 1 TiB` (fresh `PML4[2]`) is
  built by a minimal mapper (`memory.rs`: `translate` + `map_user_page`) over
  the firmware identity map — there is no `OffsetPageTable`; `CR0.WP` is toggled
  around the entry writes (the firmware maps its page tables read-only).
- **U1b** — per-fixture fault→task-kill + boundary hardening (the x86 mirror of
  aarch64 M6b). A ring-3 fault (`CS.RPL == 3`) on any user-provokable vector
  (#PF/#GP/#UD/#SS/#NP/#DE/#BR/#AC) now KILLS the offending task via the
  scheduler instead of halting the kernel; a CPL-0 fault stays fatal. The fault
  handler `swapgs`es to restore per-CPU GS (a ring-3 fault does not, unlike the
  SYSCALL stub), logs `:: EL0-equiv FAULT: task '…' KILLED — vec=… err=… rip=…
  cr2=… ::`, records `(task, vector, CR2)`-matched accounting, then reuses
  `sched::exit()` (same context `sys_exit` runs in). Three inline fixtures prove
  it — write to a kernel VA (#PF U+W), write to the RO code page (#PF U+W), exec
  from the NX stack (#PF U+I) — each KILLED at the expected vector while a
  well-behaved task still exits 0. Four boundary-hardening gaps from the U1a
  review closed alongside: **(1)** SYSRET GPR scrub (zero `rdi/rsi/rdx/r8/r9/r10`
  before `sysretq` so no kernel-dispatcher pointer leaks to ring 3); **(2)**
  canonical-`rcx` guard before `sysretq` (refuse a non-canonical return RIP —
  the CVE-2012-0217 shape — and kill instead); **(3)** a dedicated NMI IST stack
  so an NMI in the pre-swapgs syscall-entry window can't push onto the ring-3
  stack; **(4)** the user code page is mapped **read-only from the start** at
  `USER_BASE` (copied through the identity alias, never through `USER_BASE`), so
  no core can ever cache a writable mapping of it — W^X enforced across cores by
  construction, replacing the U1a single-core `invlpg` flip. QEMU-verified
  (2026-07-02): U1a still `PASS`, `exited=1 killed=3 (all expected vecs) -> PASS`,
  full boot/SMP/xHCI regression intact, 0 unexpected fault lines. **Metal-confirmed
  the same day on the real 2012 rMBP**: `:: SMEP on ::` (the one line TCG can't
  show), the 3 kills at `vec=14 err=0x7/0x7/0x15` (the `0x15` = the NX
  instruction-fetch bit, enforced by the real MMU), both U1a+U1b `PASS`, and the
  kernel continuing past all three kills — proving the `swapgs`/GS-restore and
  fault→`sched::exit()` paths work on silicon, not just under emulation. (The
  NMI-IST slot is installed on both, but its fire path is untested — no NMI
  fixture yet — so it is not part of the silicon-proven set.)
- **U2** — loadable ring-3 program from FAT + boundary preconditions (the x86
  analogue of aarch64 M6c, but loaded FROM DISK rather than `include_bytes!`d).
  - **The loader** (`syscall::u2_probe_once`, one-shot from the main loop once a
    block device is present — mirroring `fat::probe_once`'s gate, since
    `fat::mount()` needs the asynchronously-enumerated usb-storage device):
    mount the FAT volume, `find_in_root("HELLO.BIN")`, `read_file` capped at one
    code page, validate the size, copy the bytes into the RO-from-start code
    page at `USER_BASE` **through the identity alias** (the U1b B4 W^X discipline
    — the ring-3 mapping is never writable, so W^X holds across cores by
    construction), and `spawn_user` it on a scheduled AP. The loaded bytes are
    **untrusted**: nothing about them is trusted beyond the size bound — the
    program runs only under ring-3 + NX + W^X + SMEP + the U1b fault-kill net.
    A missing volume / file / oversize logs one clean line and skips (the earlier
    U1a/U1b demos are unaffected). `HELLO.BIN` is a separate flat link product
    (`crates/user-blob-x86`, `llvm-objcopy --only-section=.text` → `target/hello.bin`,
    position-independent, entry at byte 0), copied onto the FAT image and the ESP
    by the build. QEMU-verified (2026-07-02): `:: U2: HELLO.BIN loaded from FAT
    (72 bytes) -> ring 3 ::`, `hello from disk`, `:: U2: loaded program exited ok
    -> PASS ::`, across MBR-FAT32 and superfloppy layouts; U1a/U1b unchanged.
    **Metal-confirmed on the real 2012 rMBP (2026-07-03)**: a Realtek USB3 SD
    reader (`VID 0x0bda PID 0x0326`) enumerated a **FAT16** SD card, the loader
    read `HELLO.BIN` (72 B) off it, copied it into the RO-from-start `USER_BASE`
    code page, and ran it in ring 3 — `hello from disk` + `:: U2: loaded program
    exited ok -> PASS ::` photographed on the framebuffer via the USBDEBUG view
    (which keeps fbcon attached; the plain GUI build detaches it before the main
    loop). The **first disk-loaded ring-3 program to run on UnaOS silicon**; the
    first-entry GPR scrub (0b) is in this path, so ring-3 entry works with it on
    metal. (The 0a #DB-resume and #MC *fire* paths are not exercised here and
    remain metal-untested — see `SECURITY.md`.)
  - **Part-0 boundary preconditions** (they become live the moment ring 3 can run
    arbitrary loaded code): **(0a)** #DB (vector 1) and #MC (vector 18) each get
    their own IST stack. #DB closes a DoS — `RFLAGS.TF` is writable at any CPL, so
    ring 3 can `popfq` TF then `SYSCALL`; the pending single-step trap lands on the
    first `LSTAR` instruction at CPL 0 with GS/RSP still ring-3. Without a #DB gate
    that escalates to #NP whose frame lands on the user stack → a user-triggerable
    halt. The GS-free #DB handler: a ring-3 #DB kills the task; a CPL-0 #DB whose
    RIP is inside the syscall-entry stub clears TF and `iretq`s (resumes the
    syscall — long-mode `iretq` restores RSP/SS unconditionally); any other CPL-0
    #DB is fatal. #MC is fatal on its IST. **(0b)** first-entry GPR scrub in
    `user_task_trampoline` — every GPR but `rsp` is zeroed after the five iretq
    frame words are pushed and before `iretq`, so no live kernel value (the Task
    Box pointer, kernel-stack top, entry VA) reaches ring 3 in a register at first
    entry (the x86 twin of the aarch64 M6d first-`eret` scrub; the U1b SYSRET scrub
    covered only the return half). **(0c)** two closed caveats: a real self-NMI
    (`apic::send_ipi(own_id, 0x4400)`) is confirmed **taken on the NMI IST** (the
    handler checks its RSP against the IST bounds), and the canonical-`rcx` guard's
    refusal logic is unit-exercised kernel-side (`rcx_canonical` refuses
    `0x8000_0000_0000_0000`). QEMU-verified: `:: U2-0a: TF+SYSCALL survived -> PASS
    ::` (kernel not halted; `db_resumed=0` — QEMU TCG does not model the
    TF-on-`SYSCALL` trap, so the #DB *resume* fire path is metal-only), `:: U2-0c:
    self-NMI taken on IST -> PASS ::`, `:: U2-0c: canonical-rcx guard refuses … ->
    PASS ::`.
- Not yet: full user-GPR preservation across a syscall, validated user pointers
  (`copy_from_user`), per-process address spaces (U3), a self-vs-non-self loader
  check (signatures / allowlist — U2 loads unverified bytes today, safe only
  because ring-3 isolation contains them).

## The chain

| Arc | x86_64 (lead) | aarch64 (port) |
| :--- | :--- | :--- |
| Privilege round-trip | U1a ✅ | M6a ✅ |
| Per-page perms + fault→kill | U1b ✅ | M6b ✅ |
| Loadable programs | U2 ✅ (from FAT storage) | M6c ✅ (embedded blob) |
| Per-process address space | U3 (per-process PML4/CR3) | M6d ✅ (TTBR0 + ASID) |
| Process model, PIDs, handle table | U4 (x86 port pending) | M7 ✅ → **U4 ✅** (aarch64 `sys_spawn`→handle / `sys_wait`(handle); per-process handle table, owner-scoped reap) |
| Capabilities at the syscall boundary | U5 (arch-neutral) | U5 |
| FS-backed grants | U6 (UnaFS attributes) | U6 |

Conventions shared across arches:

- **Syscall numbering is common** (register conventions are per-arch — aarch64:
  number in `x8`, args `x0..x5`, return `x0`; x86_64: number in `rax`, args
  `rdi,rsi,rdx,r10,r8,r9`, return `rax`). The number space so far:

  | # | Name | Args | Returns | Notes |
  | :--- | :--- | :--- | :--- | :--- |
  | 1 | `SYS_WRITE` | fd, buf, len | byte count / `-errno` | fd 1 (stdout) only; buf validated via `copy_from_user` (aarch64 M6f) |
  | 2 | `SYS_EXIT` | status | — (no return) | scheduler reclaims the task |
  | 3 | `SYS_REPORT` | value | 0 | **demo-only** accounting channel (aarch64 M6d/M6f); not a real syscall |
  | 4 | `SYS_YIELD` | — | 0 | aarch64 M6f — thin over `yield_now` |
  | 5 | `SYS_SLEEP_MS` | ms | 0 | aarch64 M6f — `sleep_ticks` (cooperative yield where no timer IRQ) |
  | 6 | `SYS_GETPID` | — | task id | aarch64 M6f |
  | 7 | `SYS_GETINFO` | ptr | 0 / `-EFAULT` | aarch64 M6f — writes {pid, ticks} via `copy_to_user` |
  | 8 | `SYS_SPAWN` | — | **handle** / `-errno` | aarch64 M7→**U4** — loads the fixed `HELLO.BIN` into a fresh slot, runs it at EL0 as a child, and returns a **handle index into the caller's per-process handle table** (U4 — not the raw pid; arbitrary program-by-name is M8) |
  | 9 | `SYS_WAIT` | **handle** | exit status / `-ECHILD` | aarch64 M7→**U4** — blocks until the child that *handle* refers to exits (scheduler wake via the child's `done` post); `-ECHILD` if the handle is not in the caller's table (structural ownership) |

  (Numbers 10–20 are the aarch64 capability/FS/bus surface — `SYS_CAP`=10, `SYS_OPEN`=11,
  `SYS_READ`=12, `SYS_XFER`=13, `SYS_RECV`=14, `SYS_SEEK`=15, `SYS_UNLINK`=16, `SYS_CLOSE`=17,
  `SYS_FGRANT`=18, `SYS_MSEND`=19, `SYS_MRECV`=20 — documented in their U5–U11 / BANDY entries.)

  | # | Name | Args | Returns | Notes |
  | :--- | :--- | :--- | :--- | :--- |
  | 21 | `SYS_THREAD_SPAWN` | entry, sp, arg, place | **thread handle** / `-errno` | aarch64 **ELF-2** — a new EL0 thread SHARING the caller's `ttbr0`/ASID; `place` 0=caller-core, 1=sibling-core; `arg` in x0; retains the slot (freed on the last thread's exit) |
  | 22 | `SYS_THREAD_EXIT` | — | — (no return) | aarch64 **ELF-2** — posts the thread's completion + releases the slot; scheduler reclaims the task |
  | 23 | `SYS_THREAD_JOIN` | handle | 0 / `-ESRCH` | aarch64 **ELF-2** — blocks on the thread's completion `Semaphore`; `-ESRCH` if the handle is not the caller's live thread |
  | 24 | `SYS_FB_MAP` | — | surface VA / `-errno` | aarch64 **ELF-3** — maps the process's off-screen surface (EL0-RW) + RO info page; EL0 never gets the scan-out |
  | 25 | `SYS_FB_PRESENT` | — | 0 / `-errno` | aarch64 **ELF-3** — kernel composites the surface to the screen (present hook); records the surface checksum |
  | 26 | `SYS_FUTEX` | uaddr, op, val | op-specific / `-errno` | aarch64 **ELF-3** — op 0 WAIT (block iff `*uaddr==val`), op 1 WAKE (wake ≤`val`); phys-addr-keyed wait queue |
  | 27 | `SYS_INPUT_POLL` | — | packed event ≥ 0 / `-EAGAIN` | aarch64 **ELF-5** — nonblocking next input event for the caller (packed u64: `[55:48]`=type, low 32=payload), `-EAGAIN` when its per-process ring is empty; the router fills the ACTIVE process's ring via `el0_input_enqueue` |
  | 28 | *(reserved)* | — | — | `SYS_INPUT_WAIT` — DEFERRED blocking twin of 27; the number stays unused |
  | 29 | `SYS_WIN_CREATE` | w, h | win id ≥ 0 / `-errno` | aarch64 **WC-B** — allocate a window owned by the caller's ASID and map its negotiated (page-multiple) ARGB8888 surface; `w`,`h` ∈ 1..=128; `-EINVAL` bad geometry / no per-process slot, `-EMFILE` the caller used all 8 of its own region slots, `-ENFILE` the global 8-window table is full |
  | 30 | `SYS_WIN_PRESENT` | win | 0 / `-errno` | aarch64 **WC-B** — damage-mark + composite the window; `-EBADF` unknown/free id, `-EACCES` owned by another ASID. Bumps the focus-scoped present counter under the same focus guard as `SYS_FB_PRESENT` (the UVUG-8r2 cap reads it) |
  | 31 | `SYS_WIN_MOVE` | win, x, y | 0 / `-errno` | aarch64 **WC-B** — reposition the window's top-left in screen space; same ownership gate, `-EINVAL` outside ±4096 |
  | 32 | `SYS_WIN_CLOSE` | win | 0 / `-errno` | aarch64 **WC-B** — unmap the surface (leaves revert to the reserved EL1-only descriptors) and free the row; same ownership gate |

  aarch64 leads 4–9 (M6f 4–7, M7 8–9) and 21–23 (ELF-2 threading); the x86 U-side port adopts the same
  numbers so the arches stay aligned (x86 adds SMAP considerations aarch64's PAN-less A72 lacks).
- **User faults kill the task, kernel faults stay fatal.** Fault accounting
  is matched (task, vector/EC, address) so demos assert exact outcomes.
- **User pages are never executable-and-writable**; code pages are read-only
  to the kernel after load.
