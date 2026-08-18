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
  (`crates/user-vug`, staged onto the FAT media as **VUG.ELF** by `arroyo`, same build path as ELFHELLO)
  loaded and run through the EXEC-1 `run_user_image` machinery — the identical path the panel `run
  /fat/VUG.ELF` drives. It maps its off-screen surface (`SYS_FB_MAP`), spawns **two persistent EL0 worker
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
  can call `SYS_FUTEX`), plus a `uvug_witness` battery self-test that reads VUG.ELF through the VFS and runs
  it, asserting `exit=0`. QEMU-verified (`UNAOS_V3D=1 UNAOS_GENET=1 UNAOS_PIUSB=1 ./arroyo kernel8-test`,
  reproducible across runs): `:: UVUG: frames=300 threads=2 checksum=0x0313e510f24daae5 ::` then
  `:: EXEC-UVUG: run /fat/VUG.ELF — loaded 8544 bytes, entry 0x270000, exit=0 -> PASS ::`, with the workers
  scheduled on cores 2 (co-located) + 1 (sibling) and the whole prior battery (CAPSTONE 6/6, EXEC1,
  ELF-2/-3) byte-equivalent. **DEFERRED (out of this lane):** the live panel animation still needs the
  ELF-3 present-hook wiring (the 3-line `video/screen.rs` `register_fb_present_hook` registration) — until
  it lands, `SYS_FB_PRESENT` composites nothing to the screen, so the panel run prints the witness and exits
  cleanly but shows no pixels. Lane: `crates/user-vug` + `arroyo` (build/stage) + an additive
  `arch/aarch64/syscall.rs` (`futex_init` in `run_user_image` + the `uvug_witness` self-test) + this doc.
- **ELF-5 (landed 2026-07-23)** — rung 4 of the EL0-vug ladder: **input into EL0**. An interactive EL0 app
  (built on ELF-3's surface + ELF-2's threads + ELF-3's futex) needs keys/mouse; this is the delivery half.
  **`SYS_INPUT_POLL = 27`** is a NON-BLOCKING dequeue: it returns the next input event queued for the CALLING
  process, or `-EAGAIN` when its ring is empty. The event is a **packed u64** whose bit 63 is always clear
  (so it never aliases a negative errno): `[55:48]` = type (`1` KeyDown, `2` KeyUp, `3` MouseRel, `4`
  MouseAbs, `5` Button), the low 32 bits the payload — key ASCII / button mask in `[7:0]`, mouse x/dx in
  `[31:16]` and y/dy in `[15:0]` (i16). Kernel side mirrors how the GUI routes input to kernel apps today
  (the GUI-CLICK-2b `SCREEN_APP_ACTIVE` gate + `gui_watchdog`: only the ACTIVE app receives input): a small
  **per-ASID ring** (`USER_INPUT_BUF`/`HEAD`/`TAIL`, cap 32), a single producer (the router) + single
  consumer (the EL0 task) so it is a **lock-free SPSC ring** (drop-newest on a full ring — an unread event is
  never overwritten). Two **public in-lane seams** (the ELF-3 present-hook twins): `user_input_enqueue(ev)`
  (the router pushes a decoded `pal::Event` into the active process's ring) and `user_input_set_active(asid)` /
  `user_input_active()` (focus registration — the ELF-5 analogue of `SCREEN_APP_ACTIVE`; setting a focus
  resets that ring, so a freshly-focused app starts clean). Teardown folds in: `clear_handle_row` now resets
  the ASID's ring and clears the active designation if the dying slot held it (a reused ASID inherits no
  stale input; the router never enqueues to a dead slot). **SYS_INPUT_WAIT = 28 is DEFERRED** (documented at
  the seam) — a blocking variant on a per-ASID `Semaphore` that `user_input_enqueue` would post; QEMU has no
  real input source, so `SYS_INPUT_POLL` is the QEMU-provable rung this arc lands. Test (`__input_prog`,
  in-RAM, register-only): the program polls its ring empty (`-EAGAIN`), the launcher — after the initial
  empty observation, so the ordering is exact — injects ONE `KeyDown('A')` through the real
  `user_input_enqueue` seam, the program poll+yields until the event arrives, verifies the packed value, then
  polls empty again. QEMU-verified (`UNAOS_V3D=1 UNAOS_GENET=1 UNAOS_PIUSB=1 ./arroyo kernel8-test`):
  `:: EL0: input test — poll-empty=EAGAIN enqueue=1 event=0x1000000000041 drained=EAGAIN :: PASS ::` (the
  packed event = `(KeyDown<<48) | 'A'`), with the whole prior battery (CAPSTONE 6/6, ELF1/EXEC1, ELF-2 threads,
  ELF-3 fb, UVUG) byte-equivalent. **HONEST QEMU NOTE:** QEMU raspi4b delivers no USB HID, so the
  kernel-injected event is what proves the enqueue->drain + `-EAGAIN` paths — the router edge (real HID ->
  ring) is metal-only, lit up by the deferred fold. **DEFERRED ROUTER WIRING (2-3 lines, OUTSIDE this lane —
  `main.rs` `pump_usb_into_gui`, the next arc folds):** when an EL0 app owns the screen (an ELF-5 analogue of
  `SCREEN_APP_ACTIVE`, its ASID registered via `user_input_set_active`), route drained pal events to the EL0
  ring instead of `GUI_CHANNEL`:
  ```rust
  while let Some(ev) = unaos_kernel::pal::next_event() {
      unaos_kernel::arch::aarch64::syscall::user_input_enqueue(ev); // -> the active EL0 app's ring
  }
  ```
  Lane: `arch/aarch64/syscall.rs` (the ring + seams + `sys_input_poll` + the `clear_handle_row` fold + the
  in-RAM witness) + this doc — no scheduler primitive, no driver, no `main.rs`/`pal.rs` change, no x86 file.
- **INPUT-WIRE (landed 2026-07-23)** — folds ELF-5's deferred router wiring so a running EL0 program receives
  REAL keys/mouse. Two halves:
  - **Router (main.rs `pump_usb_into_gui`).** When an EL0 program holds input focus (`user_input_active() != 0`),
    the GUI router drains `pal::next_event()` into that process's per-process ring via the `user_input_enqueue`
    seam — **keyboard AND mouse** (Key/KeyUp/Mouse/MouseAbsolute/Button; Timer/None/Unknown are dropped by the
    seam) — instead of `GUI_CHANNEL`. Factored into `route_input_to_active_el0()`, the single router->ring choke
    point. This branch takes **precedence over `SCREEN_APP_ACTIVE`**: the panel `run` verb dispatches through
    `dispatch_command` (which sets `SCREEN_APP_ACTIVE`), so both flags are live during an EL0 `run`, and the EL0
    ring is the real sink. Unlike the kernel-app `SCREEN_APP_ACTIVE` gate (which LEAVES events in `EVENT_QUEUE`
    for the app's own `pump_and_poll`), the EL0 branch **drains** — an EL0 app cannot reach `EVENT_QUEUE`, so a
    left event would never be consumed.
  - **Focus lifecycle (`run_user_image`).** Focus is registered (`user_input_set_active(asid)`) right after the
    slot exists and the pid is published, and BEFORE the wait loop yields (the co-located task cannot dispatch
    until then, so no input is missed). It is cleared (`user_input_set_active(0)`) on return: `clear_handle_row`
    already clears the designation on slot teardown (verified — ELF-5's `USER_INPUT_ACTIVE` compare-exchange), so
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
    (`GUI_SENT` unchanged) — `:: USER: input router — routed=2 (key+mouse) to active-focus ring, Timer dropped,
    GUI_CHANNEL bypassed :: PASS ::`. This proves the router->ring edge; the ELF-5 `:: EL0: input test … ::`
    witness proves ring->EL0-drain; together they cover the full path. **HONEST QEMU NOTE:** the real HID *edge*
    (a USB keypress landing in `EVENT_QUEUE`) is metal-only — QEMU raspi4b delivers no USB HID — so the selftest
    drives the router with a synthetically pushed event. **What an interactive EL0 program can now do:** an ELF-3
    surface + ELF-2 threads app run via `run <path>` receives live keyboard and mouse from real hardware through
    `SYS_INPUT_POLL`, drawing/responding to input — the last delivery gap in the EL0-vug ladder.
    Lane: `main.rs` (the router branch + `route_input_to_active_el0` + `input_router_selftest`) + the
    `run_user_image` focus-registration call site in `arch/aarch64/syscall.rs` + this doc.
- **INROUTE (landed 2026-07-25)** — the router selftest above was **flaky**, roughly 1 boot in 7 under a loaded
  cascade: `:: USER: input router — routed=1|0 gui_sent_delta=0 :: FAIL ::`, seen by two independent executors on
  unrelated diffs, so the race predated both. **Root cause:** the selftest's stated precondition was false. It
  borrows the two pieces of *global* input state — it fakes focus onto **ASID 1** and pushes synthetic events
  into `pal::EVENT_QUEUE` — and then counts deliveries, so any concurrent owner of either makes the count wrong.
  Its call site sat beside the input/render task spawn under the comment "EVENT_QUEUE is empty and no EL0 slot is
  live". By that point in the boot the whole M6b..U7 fixture cascade has been spawned and is running on the APs,
  and **M6d alone holds all eight slots, ASIDs 1–8** (`:: M6d: per-task address spaces (8 slots, ASID 1-8…) ::`
  prints ~27 lines *before* the selftest's own verdict). ASID 1 was therefore a real, live fixture slot: when
  that fixture exited mid-test its teardown ran `clear_handle_row(1)` → `USER_INPUT_ACTIVE.compare_exchange(1, 0)`
  and **revoked the focus the test had just set**. A router pass that had already enqueued the Key found no
  active target for the Mouse and returned `routed=1`. Nothing was dropped by the queue (`[uvug10] evq drop`
  stayed `0`) and nothing leaked into `GUI_CHANNEL` — the events were routed to a focus that no longer existed.
  **Real input is unaffected, and neither mechanism is a bug:** revoking focus on teardown is required (a dead
  slot must stop receiving input), and `user_input_set_active`'s pre-launch discard is correct for a real
  keystroke too (UVUG-8r2 — an event queued before an app existed was never meant for it). The defect was the
  *test's* precondition. **Fix:** make the precondition true by construction — `input_router_selftest()` now runs
  from the `start_aps` block **before the secondaries are started**, the only point in the boot where sole
  ownership of focus and `EVENT_QUEUE` is structural. Rejected: a private sink for the synthetic events (it
  would stop exercising the real global seam, which is the entire point of the witness) and any retry-on-mismatch
  (launders a real race into a slower green). **Witness:** a new `[inroute] router window — routed=2
  stale_dropped=1 revokes=0 gui_sent_delta=0` line prints before the verdict; `revokes` is a new counter
  incremented whenever a slot teardown actually revokes the *live* focus (`el0_focus_revokes()`), so `revokes=0`
  is the standing proof the measurement window stayed clean and a reintroduced concurrent owner is diagnosable
  from the log alone. **Gate:** the `:: USER: input router … PASS ::` and `[inroute] … revokes=0` lines are both
  REQUIREs in `pi4-regression.spec` now (the FAIL half was already covered by the default `FAIL ::` FORBID, but
  nothing required the PASS, so a selftest that silently stopped running went green). A/B: the interleaving
  forced deterministically at the old call site reproduces `routed=1`; 20 consecutive runs on the fixed build are
  clean. Lane: `main.rs` (call site + `input_router_selftest` witness) + `arch/aarch64/syscall.rs`
  (`EL0_FOCUS_REVOKES` at the teardown CAS) + `scripts/specs/pi4-regression.spec` + this doc.
- **UVUG-3 (landed 2026-07-23)** — the mini-vug becomes the first **interactive** EL0 application. `crates/user-vug`
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
  metal-only, runs until an exit event, and prints `:: UVUG: interactive exit=<key|frames …> frames=<n> ::`.
  (`INTERACTIVE_CAP` = 36000 frames is still a bound in **fixture mode** — a foreground launch — but a
  detached/desktop vug waives it and runs unbounded; see **VUGLIFE** below.) The program is now written in Rust (a tiny `_start` in
  `.text.entry` + the worker entry, syscalls via inline-asm helpers) rather than a single asm stream, staying
  position-independent (relocation-model=static → adrp/add, **zero relocations** in the linked image; verified)
  and fitting the 16 KiB window (12568-byte ELF, two PT_LOAD segments, per-segment W^X). QEMU-verified
  (`UNAOS_V3D=1 UNAOS_GENET=1 UNAOS_PIUSB=1 ./arroyo kernel8-test`, and again with `UNAOS_VUGPAR=1`; reproducible):
  `:: UVUG: frames=300 threads=2 checksum=0x48221e4101db3924 ::` *(superseded by WC-C -> `0xe68285b85121ac7c`)* then `:: EXEC-UVUG: run /fat/VUG.ELF — loaded
  12568 bytes, entry 0x270000, exit=0 -> PASS ::`, with the whole prior battery (CAPSTONE 6/6, EXEC1, ELF-2
  threads, ELF-3 fb, ELF-5/INPUT-WIRE input router+drain) byte-equivalent. The auto checksum is a NEW deterministic
  value (0x48221e4101db3924 *(superseded by WC-C -> `0xe68285b85121ac7c`)*, vs UVUG-1's gradient 0x0313e510f24daae5 — the rendered content changed from gradient
  to wireframe). **Metal is where the interactive path lights up:** at the panel, `run /fat/VUG.ELF` now shows a
  rotating wireframe crystal (via the UVUG-2 present hook) that the operator drives with WASD/arrows/Q/E and the
  mouse, exiting on a click or ESC — the `:: UVUG: interactive exit=… ::` line is the metal-only witness. Lane:
  `crates/user-vug` only (no kernel/syscall/boot/arroyo change — same crate, linker script, and build/stage
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
  wide) and no single HID delta can spin past a quarter-turn in one frame. Lane: `crates/user-vug` only + this doc.
- **UVUG-5 (this arc)** — two input-side corrections after the P47 metal capture (`run /fat/VUG.ELF` ran the
  300-frame auto batch to `exit=0` with the unchanged checksum, but showed **no interactive takeover** and a
  spurious `[gui] watchdog app wedged 5s`). (1) **Watchdog false-fire** — the `run` command arms `gui_watchdog`
  via `on_app_enter`, but nothing fed `note_progress` on the EL0 path, so a healthy polling app was declared
  wedged at 5 s and the shell was handed the keyboard back mid-run; fixed by feeding `gui_watchdog::note_progress`
  from `sys_input_poll` (see the corrected ELF-5 decision above). (2) **Router-delivery witness** — the
  router→ring code path is verified correct (`user_input_active()` precedence in `pump_usb_into_gui`;
  `current_asid()` == the `user_input_set_active` ASID both derive from `TTBR0_EL1[63:48]`), so the metal
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
    header** (magic, w, h, stride, format, size, surface-offset at offset 0), so the existing `VUG.ELF`
    binary runs unchanged. Per-window geometry is published alongside it at `0x40 + rslot*0x20`
    (magic, win id, w, h, stride, format, surface-offset), zeroed on close so EL0 can tell live from stale.
    The **canonical info-page layout**, in one place:

    | offset | contents |
    | :--- | :--- |
    | `0x00`–`0x1B` | legacy ELF-3 header — magic, w, h, stride, format, size, surface-offset (describes region slot 0) |
    | `0x20` | **process flags** — bit 0 = `DETACHED` (VUG-BG: launched by `bg`, not `run`); bit 1 = `HIDDEN` (VUGMIN: every window this process owns sits below the shell's z); rest reserved, zero |
    | `0x24`–`0x3F` | reserved, zero |
    | `0x40 + rslot*0x20` | per region slot — magic, win id, w, h, stride, format, surface-offset; magic 0 = no live window |

    Both info-page publishers write the flags word (the legacy header is only refreshed for region slot 0,
    so the per-window publisher writes it too — a process whose first window landed on a higher slot would
    otherwise read a zeroed, i.e. wrongly "not detached", word). Both now go through one
    `fb_info_flags(slot)` helper, so a future bit cannot reintroduce that split-publisher bug.
    **The two bits have different lifetimes, and EL0 must read them differently.** Bit 0 is fixed before
    the process runs, so the write the window verbs perform anyway is always in time and one read at
    start-up is complete. Bit 1 **changes under a running process**, so `set_hidden` republishes the flags
    word itself (only that `u32`, single-copy-atomic on the A72 — a concurrent EL0 read sees the old value
    or the new one, never a tear); EL0 polls it **per frame**.
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
    **focus-scoped** `EL0_FOCUSED_PRESENT_COUNT` bump (under the same `USER_INPUT_ACTIVE == asid` guard)
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
    focus-scope it, joining the takeover heartbeat under one `USER_INPUT_ACTIVE == asid` test; they were always
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
    `crates/user-vug/src/main.rs` + `pal.rs` (typematic liveness) + `arch/aarch64/syscall.rs`
    (`sys_input_poll` focus scope) + `main.rs` (shell-path witness) + this doc.
- **VUGGUARD — an app must not proceed as though a requested resource was granted** (`crates/user-vug` only;
  the kernel-side thread-row reclamation and kill-aware futex are a separate arc). Attended P60 on a Pi 4:
  after a few kill/relaunch cycles, new vugs came up as **empty windows** that could not be killed, and
  eventually nothing would launch at all. The app's share of that chain is one omission. `SYS_THREAD_SPAWN`
  draws from a **fixed, GLOBAL** 8-row thread-handle table (`NTHREAD`) whose rows are freed only by the owner's
  voluntary `SYS_THREAD_JOIN`; UVUG joins its two workers only *after* its main loop, so a killed vug leaks both
  rows and four such kills exhaust the table. From then on every spawn returns `-EAGAIN` — and UVUG **never read
  the return**, capturing the two values only to join them later. A vug that got zero workers therefore entered
  the frame barrier waiting for a `done` count of 2 that no living thread would ever bump, and parked in
  `futex_wait` **before its first `SYS_WIN_PRESENT`**: kernel-drawn chrome, no content, and a process wedged in
  a wait no kill reached.
  - **Both returns are now checked**, and the failure behaviour is **DEGRADE**, not fail-fast. Every band no
    worker owns is rasterised **inline by the parent** with the same `render_band` over the same published
    projection, placed between the worker release and the barrier — exactly where the parent otherwise idles,
    so with one worker alive the two halves still draw concurrently. A vug launched while the table is full
    still comes up, still draws, still takes input and still exits 0; it is only single-threaded. Fail-fast was
    the alternative and was rejected: table exhaustion is a *transient* condition, and refusing to launch would
    convert a recoverable system into a hard one at exactly the moment the operator is trying to get a window
    back. Because the inline raster is the same code over the same geometry, the **final surface is
    byte-identical** to a two-worker run — the deterministic auto-path checksum is a property of the frame
    count, not of how many threads drew it.
  - **The barrier waits for the workers that EXIST**, never a hard-coded 2 — with none it is not entered at all.
    That closes P60's class *structurally*: no pass budget could ever have caught it, because a parked parent
    executes nothing. UVUG-9's `BARRIER_PASS_BUDGET` survives for the narrower case it can see — a thread that
    exists and stops arriving — and is now a **deadline as well as a witness**: on the pass that prints
    `[uvug9] stall … phase=barrier`, the parent signals `PHASE_EXIT`, retires the pool and takes both bands
    inline for the rest of the run. UVUG-9 printed that line and then went straight back into the same wait.
    The line now marks the one frame that presents partially-stale content, not a permanent state.
  - **A retired worker is not joined.** `sys_thread_join` blocks until the thread finishes, so joining the very
    thread that just failed to arrive would park the parent forever at exit — the symptom being removed. Its
    kernel row is deliberately leaked instead: a leaked row is reclaimable from the kernel, a parked process is
    not reclaimable from anywhere.
  - The exit witness reports the workers the run actually **got** (`threads=<n>`) rather than the two it asked
    for; on the healthy path that is the literal `2` the pi4 spec `REQUIRE`s. A degraded launch also names the
    denied resource up front: `:: UVUG: SYS_THREAD_SPAWN denied a=<errno> b=<errno> workers=<n> -> inline
    raster ::` — the diagnostic whose absence made P60 read as a compositor fault.
  - **Healthy path untouched by construction.** The additions are two `if` predicates that are false when both
    spawns succeed, one `live > 0` guard on the release, and `d >= live` in place of `d >= 2`. No syscall is
    added, removed or reordered anywhere in the frame, so the WC-H present discipline is unchanged.
  - Gates: `./arroyo check` green x86_64 + aarch64; `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60`
    replayed through `scripts/specs/pi4-regression.spec` → **63/63 required, 0 forbidden**; `./arroyo test-arm`
    MISSION SUCCESS. Healthy-path evidence: the auto witness is byte-identical to the pre-change baseline
    (`:: UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c ::`), `EXEC-UVUG … exit=0 -> PASS`, and no
    `[uvug9] stall` or spawn-denied line appears anywhere in a healthy boot. Lane:
    `crates/user-vug/src/main.rs` + this doc.
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
    `input_launcher` orphan held `user_input_active()` for the *entire* boot, so the router's EL0 branch
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
    > **Superseded by KILLBOUND (below).** The first limitation was not a bounded inconvenience — it was
    > the P60 wedge, and it was reachable by an ordinary operator in an ordinary session. It is now closed
    > from both sides, and **the lock-order objection recorded above was simply wrong**: the eviction takes
    > exactly the lock `futex_wake`/`Semaphore::post` already take, in the same order, and calls
    > `make_ready` outside it. The second limitation (no boundary at all) is genuine and stands.
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
- **BGRUN-1 (background EL0 runs)** — `bg <path>` / `jobs` / `kill <pid>`: the concurrent-apps line.
  `run` blocks the shell in `run_user_image`'s wait loop until the program dies, so two real windows never
  coexisted longer than the `el0-wcb` fixture's split second — the WC-TAB focus ring was mechanism without
  a workflow (bench-observed: windows open for a fraction of a second, nothing to TAB between).
  - **`spawn_user_image_bg`** (syscall.rs) is `run_user_image`'s front half, verbatim — same bounds, same
    console-cap endowment, same EXEC1-M asid-before-spawn publish order (the SYS_EXIT rescue arm covers the
    same mid-publish window here, correctly) — and then RETURNS `(pid, asid, entry)` instead of waiting.
    Deliberate non-inheritances: no `user_input_set_active` (focus stays put; the operator TABs into a bg
    window, which resets its ring exactly as the foreground path does), and no deadline/UVUG-8 machinery
    (nothing waits, so nothing can strand the SHELL; and — stated plainly, lens-corrected — NOTHING bounds
    a bg app: no deadline, no watchdog, no compositor cap. `kill <pid>` is the sole remedy, which is why
    the focus-ring guard keys on where focus is (a focused window can ALWAYS TAB out, even at n = 0) —
    the shell, and therefore `kill`, must be unreachable never.)
  - **`jobs` is the SOLE reaper**: a bg row stays claimed after exit (`PEXITED`, `done` posted) until
    `bg_poll(reap=true)` consumes the permit and frees it — bounded and honest (a shell that never reaps
    eventually reads "process table full", not silent loss). The shell records jobs in a bounded
    `BG_JOBS` table — deliberately headroom over the true ceiling, the Proc table's **MAX_PROCS**, which
    binds first: on aarch64 that is 6 bg jobs, or 5 alongside one foreground `run`; a spawn the table
    cannot record is killed, not leaked. (`BG_JOBS` is arch-neutral and went 8 → 12 at **HEADROOM**,
    following x86's `MAX_PROCS` to 10; on aarch64 the extra rows can never be claimed. See
    `scheduler.md`, "Fleet headroom, x86".)
  - **The capacity, and why it is 6 (`PROCS-6`).** The cap was 4; it is 6 so the operator can fill the
    panel with background vugs. Nothing else moved, because every consumer of the cap is parametric —
    the reserve/free/find/census sweeps are `0..MAX_PROCS`, reap accounting is per-row CAS, and both
    scavenges (`BGRUN-SCAV`'s `PEXITED` reclaim and `KILLBOUND`'s `PORPHANED` reclaim behind its
    quiescence witness) walk the same range. There is no bitmask and no packed index keyed on the old
    value. The three neighbouring tables all stay strictly above it: `USER_SLOTS` = 8 EL0 address
    spaces (so 6 live bg programs still leave 2 for a foreground `run` and for the launcher fixtures'
    scratch tenancies — this headroom is the reason the cap is 6 and not 8, which would make the Proc
    table and the slot pool exhaust together and disguise every slot-pressure failure as a table-full
    one), `wm::MAX_WINDOWS` compositor rows (only EL0 programs mint windows; the shell console is
    desktop-level, so 6 windowed bg programs fit with room over — that constant is arch-neutral and went
    8 → 12 at HEADROOM, which on aarch64 only widens the margin), and the shell's `BG_JOBS`.
    The first two are now compile-time assertions beside the constant rather than prose (the slot one
    as `MAX_PROCS <= USER_SLOTS - 2`, the margin the sentence above actually promises — a bare `<`
    would have permitted 7, satisfying the letter while starving exactly those two callers).
    - **The kill table is COUPLED to the cap, and is now derived from it.** `sched::MAX_KILL_REQS` was
      the literal `4`, and its rationale was openly "`MAX_PROCS` bounds how many rows can be at risk" —
      so it was never an independent choice, and raising the cap silently decoupled them. The shortfall
      is not benign: 6 killable rows against 4 kill slots means an operator hammering `kill` across a
      full panel of vugs exhausts the kill table first, and every request past the fourth arms nothing,
      falls back to `PORPHANED` and parks a row — reclaimable then only through KILLBOUND's narrower
      quiescence witness (which needs the victim's ASID drained). That is the shape of the P60 wedge
      this machinery exists to prevent, re-introduced by a capacity change one file away. It is now
      `MAX_KILL_REQS = MAX_PROCS` with an assert holding `MAX_PROCS <= MAX_KILL_REQS`, which makes the
      shortfall unrepresentable: `KILL_EXHAUSTED` becomes reachable only by concurrent requesters
      racing the same rows, never by capacity. The assert is an inequality on purpose — kill headroom
      above the row count is allowed, below it never.
    - **x86_64 keeps its own `MAX_PROCS = 4`.** The tables are per-arch and are not required to track
      each other; the divergence is noted at both constants so it is discoverable from either side.
    The cost is
    two more static rows plus their `done` waiter reservations — a few hundred bytes, entirely off the
    per-slot `SLOT_BACKING` budget, which `USER_SLOTS` governs and this change does not touch.
    Witnessed once per boot by `:: BGRUN-ST: process table capacity = 6 rows (bg programs alive at
    once; EL0 slots 8) ::`, with a spec REQUIRE, so a silent regression of the cap fails there.
  - **`kill <pid>`** is the SKILL-1 primitive (ASID-scoped so ELF-2 siblings die too), condensed: bounded
    confirm wait, post-arm already-dead retract, `PORPHANED` fallback with the kill left armed. A
    CONFIRMED kill reaps the row in place, both arms (a dispatch-boundary kill never reaches SYS_EXIT,
    so waiting for `jobs` would read "running" forever); a repeat `kill` on a parked row returns early
    (`proc_orphan`'s PRUNNING precondition is honoured — the round-1 lens showed the alternative parks
    the shell task forever on a permitless `done.wait`).
  - Witnesses: `:: BGRUN: bg <path> — … DETACHED ::` on spawn, `:: BGRUN: jobs — pid=… reaped ::` on reap,
    and the boot-time `BGRUN-ST` selftest (spawn->exit->reap + kill-mid-run + BGRUN-2's persist+kill),
    each spec-REQUIREd plus a `-> FAIL` FORBID — the round-1 lens showed the headless-observable core of
    the contract was gateable after all. Only the INTERACTIVE half is bench-only (QEMU has no HID): see
    BGRUN-2 below for the app that makes it testable. One widening worth knowing at the bench: the shell now consumes TAB at
    n == 1, so TABbing during a single-window foreground `run` drops focus to the parked shell — TAB back
    re-enters; linger instead and the takeover re-arm can SKILL-1 the app ~5 s later. The safety property
    (never weld the operator away from `kill`) outranks that exposure, deliberately.
- **BGRUN-2 (`stat.elf` — the persistence app)** — the fixture BGRUN-1 was missing. BGRUN-1's bench recipe
  was "`bg /fat/uvug.elf` twice → TAB between two live crystals", and it does not work: a BACKGROUNDED UVUG
  is UNFOCUSED, so no HID event ever reaches it, so it never leaves its deterministic auto path — 300 frames
  and gone. Both windows flash past before a hand reaches TAB. The window ring was untestable at the bench
  not because of a compositor defect but because there was no app that STAYS.
  > **Superseded in part by VUG-BG (below):** a backgrounded `VUG.ELF` now persists too, so the original
  > "`bg` the vug twice" recipe works after all. `STAT.ELF` is not thereby redundant — it has no exit
  > condition *at all*, focused or not, which is a stronger property than VUG's conditional persistence,
  > and it is what `BGRUN-ST` leg 3 rests on. It also puts its pid on screen, which is what makes the TAB
  > walk checkable against `jobs`.
  - **`crates/user-stat` → `STAT.ELF`** is that app: a static ELF64 EL0 program built and staged exactly
    like `VUG.ELF` (own workspace + `user-stat.ld` PHDRS: R+X text, R+W data; `arroyo kernel8` builds it,
    checks the ELF magic and the 16 KiB `USER_REGION_SIZE` bound, and copies it to the FAT staging dir).
    ~8.5 KiB. It creates one 128x128 window (`SYS_WIN_CREATE`, the same one-slot geometry UVUG negotiates),
    and each frame repaints its **own pid in large digits** (from `SYS_GETINFO`), a **frame counter** and a
    **sweep block**, then `SYS_WIN_PRESENT`s and `SYS_SLEEP_MS(50)`s (~20 fps; on metal that is a real timed
    sleep, so it costs nearly nothing sitting on the panel).
  - **It has no exit condition.** Not on unfocus, not on a frame cap, not on ESC. `kill <pid>` is the whole
    remedy — which is not a shortcut but BGRUN-1's stated contract for a bg app, now exercised rather than
    described. It also does **not poll input**, so it behaves identically focused and unfocused (its ring
    fills and drops, bounded and harmless) — the precise property the TAB walk depends on. The only
    non-kill path out is a failed `SYS_WIN_CREATE`, which prints and exits 1 rather than writing an unmapped
    VA.
  - **The pid on screen is the point.** Two instances of the same image are otherwise pixel-identical; the
    large pid is what lets the operator say WHICH window TAB just focused, and it is the same number `jobs`
    prints and `kill` takes.
  - Witnesses: `:: STAT: start pid=<n> win=<id> ::` and, once, `:: STAT: alive pid=<n> frames=40 ::` —
    two one-shot lines and then silence forever (a program with no exit must never be a serial firehose).
    Headless, `BGRUN-ST` leg 3 spawns it detached, dwells 2 s (comfortably longer than UVUG's entire
    300-frame auto run, so "still running" cannot be confused with "has not got round to exiting"),
    REQUIREs `Running`, kills it and requires the row settles: `:: BGRUN-ST: persist+kill PASS (pid=…) ::`.
  - **Gate length: the QEMU window moved 35 s → 60 s with this arc**, in `pi4-regression.spec`'s header and
    in `arroyo`'s `battery` step. Leg 3's dwell (2 s, plus STAT's yield-amplified cost while QEMU's
    degraded `SYS_SLEEP_MS` makes it spin) consumed headroom that was only ~10% to begin with AND was
    machine-dependent: one host still read 54/54 at 35 s while another dropped to 42/54, losing the twelve
    witnesses that print LAST (K8b-snap, K8c-snapread, K6-migrate, all BANDY-*) — a truncation that reads
    as a regression in arcs nobody touched. Measured on this branch: 24 s / 27 s → 44/54; 30 / 35 / 45 /
    60 s → 54/54; at 60 s the last required witness lands ~40% into the log. **Known hazard, unfixed and
    out of this lane:** `battery`'s pi4 step pattern-matches `CAPSTONE COMPLETE` only, which prints EARLY
    — so a truncated log still reports the step GREEN. The battery cannot currently go red on a short
    window; only an explicit `mbench --replay … --spec` can. Assert the spec, not the battery step.
  - **Bench recipe (the TAB test, at last).** At the panel shell:
    `bg /fat/STAT.ELF` → `bg /fat/STAT.ELF` → `jobs` (two `running` rows; note the pids) → press `TAB`
    repeatedly and watch focus walk shell → window A → window B → shell, checking the large pid against the
    `jobs` list each stop → `kill <pidA>` (its window vanishes; the line reads `killed — row reaped`) →
    `jobs` (one row left) → `kill <pidB>` → `jobs` (`none`). Use `bg`, never `run`: a foreground `run` of a
    program that never exits ends at `run_user_image`'s deadline with a SKILL-1 kill.
- **VUG/STAT** — the two EL0 apps get their real names, a backgrounded vug stops looking like a
  crash, and the compositor says which window has the keyboard. Three folds, one arc; app UX only.
  - **Fold 1 — the rename.** `crates/user-uvug` → **`crates/user-vug`** (`UVUG.ELF` → **`VUG.ELF`**,
    `user-uvug.ld` → `user-vug.ld`). The persistence app was renamed to `KVUG` in the same fold and that
    half was **reverted by STAT-NAME (below)**: it is `crates/user-stat` → **`STAT.ELF`** again, the name
    it has here throughout. Swept through `arroyo`'s build and staging stanzas,
    the FAT staging names, the in-kernel witness paths that load the images by name (`uvug_witness`'s
    `/fat/VUG.ELF`, `BGRUN-ST`'s kill and persistence legs), and the doc mentions here and in
    `08_VIDEO/engine.md`.
    - **The serial witness TAGS did NOT change, deliberately** — `UVUG:`, `EXEC-UVUG:`, `BGRUN-ST:`,
      `STAT:` and the `UVUG-1`..`UVUG-10` arc identifiers all stand. Those name **arcs and witnesses**,
      not files: they are the keys `pi4-regression.spec` matches on and the labels every landing report,
      capture and hazard note in this repo already uses, and silently repointing them would make the
      historical record unsearchable to buy nothing. The rename is of the *artifacts*; the ledger keeps
      its names. Consequence, stated so it is not read as an oversight: `run /fat/VUG.ELF` prints
      `:: EXEC-UVUG: …`, and `pi4-regression.spec` needed **no pattern change** for this fold.
  - **Fold 2 — VUG-BG: a backgrounded vug persists.** `bg /fat/VUG.ELF` used to look like a crash. The
    app was fine; the design was wrong. A bg'd vug is UNFOCUSED, so no input ever reaches it, so it never
    left its deterministic auto path — 300 frames, `[gui] app-exit dur=0s wedged=false`, gone. The read
    from the bench ("it crashes") was the only reasonable one.
    - **Mechanism: the kernel tells the process how it was launched.** A per-ASID bit (`DETACHED_ASIDS`
      in `arch/aarch64/syscall.rs`) is **set** by `spawn_user_image_bg` and explicitly **cleared** by
      `run_user_image` — cleared rather than merely left alone, because ASIDs recycle and a slot last used
      by a `bg` spawn must not hand its stale answer to the next foreground program that inherits the
      number. It is published to EL0 in the **RO info page** as a process-flags `u32` at offset **`0x20`**
      (bit 0 = `DETACHED`), in the reserved gap between the legacy ELF-3 header and the per-window
      entries, so no existing field moves. Written by both info-page publishers (the legacy header, and
      the per-window entry — the latter because the legacy header is only refreshed for region slot 0).
    - **What the app does with it.** `crates/user-vug` reads the word once, after `SYS_WIN_CREATE` (which
      is what maps the page), and a detached vug **skips the 300-frame cap** — same deterministic tumble,
      no end. `kill <pid>` is the whole remedy, exactly as for `STAT.ELF`. Focused `run` is byte-identical
      to before.
    - **The checksum witness is untouched, structurally.** `uvug_witness` runs `VUG.ELF` through
      `run_user_image` — the FOREGROUND launcher, which clears the bit — so `REQUIRE UVUG: frames=300
      threads=2 checksum=0xe68285b85121ac7c` cannot be reached by this branch. `BGRUN-ST` leg 2 (kill a
      bg'd vug mid-run) gets strictly *more* robust: its target no longer races its own exit.
  - **Fold 3 — FOCUS-HL: the focused window is visible.** `video/wm.rs` records the focused ASID in
    `FOCUS_ASID` (set only by `focus_changed`, `0` = the shell) and the composite pass snapshots it once,
    for the same reason it snapshots `SHELL_Z` once — one pass, one focus owner, so no pass can draw two
    highlights. `draw_window` takes a `focused` flag and swaps **only two colours**: `CHROME_BORDER` →
    `CHROME_BORDER_FOCUS` and `CHROME_TITLE_BG` → `CHROME_TITLE_BG_FOCUS`. No geometry changes, so focus
    never moves a pixel and costs nothing per present — it repaints the frame and strip that were going to
    be painted anyway.
    - **Shell-focused highlights nothing**, which is the honest reading: no app has the keyboard.
    - **Both ends repaint.** `focus_changed` already damages the windows it raises; it now also damages
      the windows of the ASID that is LOSING focus, which the raise never touches (and which the shell
      branch does not raise at all). Without that the old holder would keep the bright chrome until
      something unrelated happened to damage it.
- **SPINHUNT (this arc)** — the **orphaned worker thread**: `SYS_EXIT` retired one task, not the address
  space, so a leader that exited without joining its workers left them running forever. P61 was an attended
  sitting: with several bg vugs launched, killed and relaunched, `:: SCHED: load c0=68% c1=99% c2=0% c3=0% ::`
  was **sustained** — one core pegged — while `jobs` listed every vug pid as `exited 0 (reaped)`.
  - **Why the process table was clean and the core was not.** Everything the operator can inspect is keyed
    on a `Proc` row, and an EL0 **worker thread** has none — `SYS_THREAD_SPAWN` creates a task under the
    caller's existing slot, tracked only by a `THREAD_TABLE` row and the slot's live-task count. So `jobs`
    was telling the truth about every row it had; the thing burning core 1 was not in it. WC-J had already
    ruled the compositor out by construction, and that was correct: the spinner was never kernel code.
  - **The lifecycle gap.** `boot::slot_thread_retain` keeps a slot mapped until the **last** thread under it
    exits. Nothing in the kernel ever made the last one exit. `SYS_THREAD_EXIT` retired a thread and
    `SYS_EXIT` retired the calling task — neither retired the *address space*. An unjoined worker therefore
    outlived its leader with no parent, no joiner, and no terminus.
  - **Why it burns a core rather than idling.** `user-vug`'s barrier is asymmetric by design: ARRIVE
    (worker → parent) is a futex, but RELEASE (parent → worker) is a **`SYS_YIELD` poll** on the phase word
    (`uvug_worker`). A parked orphan would have been invisible and harmless; a polling orphan is
    **runnable** — in a run queue, dispatched every pass, at 100% of its pinned core for the rest of the
    boot. `place = 1` sends it to a sibling core, which is why the load landed on one core and stayed there.
  - **And it is self-sealing.** KILLBOUND's `THREAD_TABLE` scavenge is gated on `ASID_GEN[owner]` having
    been bumped, which happens on the slot's 1→0 teardown edge, which requires the last thread under the
    slot to exit — the very thing the orphan will never do. The row is unreclaimable, the slot unrecyclable
    and the core pegged, permanently. This is the one shape KILLBOUND's discipline could not reach: it made
    every *reclaim* wait for a positive quiescence witness, and here quiescence never arrives.
  - **Fix: `SYS_EXIT` terminates the address space.** At the top of the `SYS_EXIT` arm — before every
    name-routed short-circuit, so there is one insertion point for all of them — if the caller's ASID still
    holds live siblings, `sched::orphan_kill(asid)` arms an **address-space-scoped** kill through exactly
    the machinery a `kill` uses. Each orphan is matched at its own next boundary (an EL0 syscall — a
    yield-polling worker reaches one every pass — or a preemption on metal) and retires through its own
    `exit()`. The request is **owner-less**: there is no requester to confirm to (the leader is on its way
    into `exit()`), so the ticket is armed and immediately `kill_detach`ed — `kill_slot_for` still matches
    `KILL_DETACHED`, and the last orphan out CASes the slot `KILL_DETACHED → KILL_FREE` inline, returning
    it to the four-entry pool exactly once. A full request table is an honest, witnessed, bounded failure,
    not a silent one.
  - **KILLBOUND's discipline is preserved, not weakened.** Nothing is reclaimed at the leader's exit. Each
    orphan decrements `SLOT_REFCOUNT` itself; only at zero does `teardown_user_slot` bump `ASID_GEN`, and
    only then does the thread-table scavenge consider the row dead. The fix makes that edge **reachable**;
    it does not move it earlier, and no row is ever freed while its task could be mid-execution on any core.
  - **Semantics.** `SYS_EXIT` is the *process* terminus; `SYS_THREAD_EXIT` is the *thread* terminus. Only
    the second one previously did anything. `user-vug`'s own `PHASE_EXIT`-then-join is now belt-and-braces
    rather than the sole guarantee — which matters because VUGGUARD deliberately makes a killed vug skip the
    join (joining a worker that is not answering would park the parent forever).
  - **Witness: `BGRUN-ST` leg 0b / `:: SPINHUNT: …`,** `+4` spec REQUIREs and `+1` FORBID (66 → 70). The
    fixture is an in-kernel flat blob (no SD card, no staged `.ELF`): the leader spawns two workers that
    sign in and then yield-poll forever, waits for both sign-ins, and calls `SYS_EXIT(0)` with **no**
    `SYS_THREAD_JOIN`. The positive witness — `2 sibling thread(s) left unjoined` — is stated by the
    **leader itself** at the only instant it is exactly true; a poller on the witness core can miss that
    window entirely, and a leg whose positive witness is a race is a leg that passes for the wrong reason.
    The verdict is that the ASID drains to **zero** live tasks inside a bounded window. Two load rows
    bracket it (evidence, not assertions — QEMU raspi4b delivers no timer IRQ, so the percentages there
    read the dispatch-span accounting rather than the metal load line).
  - **Verified to have teeth (A/B).** With `orphan_kill` disabled and nothing else changed, the leg reports
    `orphaned=2 (want 2) reaped=true drained=false leftover=2 live task(s) under asid 1 -> FAIL`, and the
    orphan-window row reads `c0=58 … / settled c0=61` where the fixed kernel reads `c0=0` on both — the P61
    pegged core, reproduced in QEMU, with the leader's row already reaped. The leader's status is `0` in
    both arms, so `jobs` reads clean in both: the A/B *is* the P61 symptom.
  - Gates: `./arroyo check` green x86_64 + aarch64; `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 90`
    → **70/70 required witnesses, 0 forbidden**; `./arroyo test-arm` MISSION SUCCESS. Lane:
    `arch/aarch64/sched.rs` (`orphan_kill`) + `arch/aarch64/syscall.rs` (the `SYS_EXIT` hook, the fixture
    and the leg) + the spec + this doc. QEMU-green ≠ correct: the metal confirmation rides the attended
    sitting, where the property to watch is the `SCHED: load` line after a vug is killed and relaunched.
- **U7FIX (this arc)** — the **iteration-denominated park**: a fixture wait bounded by a *count of syscalls*
  standing in for a wait bounded by *time*. P63 was an attended boot, and U7 was the only failing line on a
  tip QEMU gated 79/79 green:
  `:: U7: cross-process transfer FAIL — parent=0x3 child=0x0 used=0 snap=false cleared=true killed=0 done=2 ::`
  - **Decoding `parent=0x3` names the step.** The parent's witness bits are `b0` over-rights XFER refused,
    `b1` XFER t1 deposited, `b2` XREVOKE t1 accepted, `b3` XFER t2 deposited. `0x3` is `b0|b1` and nothing
    more, and the only path out of the parent between `b1` and `b2` is its **GO park** — so `0x3` is precisely
    "the parent deposited t1 and then gave up waiting to be released." `child=0x0` is the same statement about
    the child's own GO park, one step earlier.
  - **Root cause: the launcher and the fixtures were bounded in different currencies.** Every launcher
    deadline in `u7_run` is wall clock (`wait_while_secs(5)`, `(5)`, `(8)`); every fixture park was a budget
    of `0x8000` iterations of `SYS_YIELD`. Those two do not convert at a fixed rate. Under QEMU an EL0 syscall
    round trip is *emulated*, costing ~1 ms, so `0x8000` of them outlast **30 s** (measured directly, by
    stalling the launcher that long and watching the parent still report `0xf`). On a Cortex-A72 with a
    genuinely idle sibling core the same round trip is a few hundred nanoseconds, so the identical budget
    retires in **single-digit milliseconds** — three orders of magnitude, entirely in the direction that
    breaks. The fixture had been calibrated, implicitly and invisibly, against QEMU's emulation cost. The
    header comment even advertised the mistake as a virtue: *"cooperative SYS_YIELD polling — deterministic
    under QEMU."* It was deterministic under QEMU. That was the whole problem.
  - **The whole wire line follows from that one fact.** The child's park expired while the launcher still had
    a full user-window scrub, a slot build and a ~110-character PL011 line at 115200 baud to get through, so
    the child exited with an empty witness (`child=0x0`, `used=0`, `done` +1). Its teardown then wiped its
    ASID's transfer inbox — the `clear_handle_row` twin that drops a dying ASID's undelivered deposits — which
    is the inbox the parent's t1 deposit was sitting in, so the launcher's deposit poll found nothing and
    `snap=false`. `U7_CHILD_USED` never rose, so the launcher burned its full 5 s before releasing the
    parent's GO, by which time the parent's park had expired too: `parent=0x3`, `done=2`. `cleared=true` and
    `killed=0` because nothing actually malfunctioned — both fixtures shut down cleanly, having simply given
    up. **Nothing was wrong with the transfer path itself**, which is the verdict worth stating plainly: this
    is a fixture defect, not a `SYS_XFER` coherency bug, and the cross-core publish ordering was never
    implicated once `parent=0x3` was decoded.
  - **Not BG-SPREAD, despite the timing.** The obvious suspect was the commit immediately before, but the U7
    fixtures are spawned `spawn_user_slot(.., demo_cpu)` — **caller-pinned**, never `CPU_AUTO` — so BG-SPREAD
    cannot have moved them, and U7 runs at the head of the boot battery, before any `bg` launch exists to
    stack. The defect is older than BG-SPREAD; P63 is simply the boot that happened to expose it.
  - **Fix: put the fixtures back in the launcher's currency.** All four bounded parks (the parent's GO wait,
    the child's GO wait, and the child's two `SYS_RECV` polls) now step on `SYS_SLEEP_MS(1)` instead of a bare
    `SYS_YIELD`. On metal that is a real 250 Hz tick, so `0x8000` iterations is ~131 s — comfortably past the
    launcher's ~18 s worst case — and a parked fixture stops spinning a core while it waits. On QEMU
    `SYS_SLEEP_MS` has no delivered timer IRQ and falls back to a cooperative yield, so the demo stays
    deterministic and needs no timer preemption to make progress, exactly as before. **Deliberately not
    fixed by re-pinning or by enlarging the magic number**: either would have restored the green line while
    leaving the units mismatched.
  - **QEMU reproduced the failure but cannot gate the fix — stated plainly, because it constrains what the
    witness is allowed to claim.** The shape reproduces readily: stall `u7_run` deliberately while the
    fixtures are parked and the pre-fix tip returns P63's wire line exactly, `parent=0x3 child=0x0 used=0`,
    both arms (3 s suffices with the child alone on the core; 8 s is needed once the parent shares
    `demo_cpu` and each drains its budget at half rate). But `SYS_SLEEP_MS` **falls back to a cooperative
    yield under QEMU**, since no timer IRQ is delivered there — so under QEMU the fixed park and the broken
    park are the same instructions' worth of waiting, and a stall long enough to break one breaks the other.
    A "stall and survive" leg would therefore have been a guard that only appeared to guard. It was built,
    measured, and discarded; the fix's own confirmation belongs to the bench.
  - **Witness: `:: [u7fix] park margin — … ::`,** `+1` spec REQUIRE and `+2` FORBIDs (79 → 80). What QEMU
    *can* gate, and what P63 was actually missing, is an **assertion rather than a stall**: each fixture must
    still be parked at the moment its GO is released. A fixture that gave up mid-park has already run its
    `SYS_EXIT`, so `EL0_U7_DONE` counts it, and neither U7 program can legitimately exit before its GO — a
    non-zero reading there is a parked-out fixture and nothing else. The launcher was holding that fact all
    along and never looked at it, which is precisely why P63 reported the *consequence* (`child=0x0`) and left
    the *cause* to be inferred from a bitmask. The line also reports both parks' measured durations, and those
    numbers are the arc's most uncomfortable finding: **6 ms and 2 ms**. The launcher was never slow. The
    pre-fix budget retired in single-digit milliseconds on metal, so the margin was never orders of magnitude
    — it was the same order of magnitude as the wait, and P63 is simply the boot on which it lost. The margins
    now print every boot, so the next erosion is visible before it is a failure.
  - **Verified to have teeth (A/B).** Reverting only the park primitive, with a stall to expose it, the
    witness reads `child parked 8005ms before GO (parked_out=2), parent parked 13001ms before GO
    (parked_out=true)` and the gate drops to **78/80 with 2 forbidden hits** — and, unlike P63, the log now
    *names the defect on its own line* instead of requiring the bitmask to be decoded.
  - Gates: `./arroyo check` green x86_64 + aarch64; `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 90`
    → **80/80 required witnesses, 0 forbidden**; `./arroyo test-arm` MISSION SUCCESS. Lane:
    `arch/aarch64/syscall.rs` (the U7 fixture blob + `u7_run`) + the spec + this doc. QEMU-green ≠ correct:
    the metal confirmation rides the next attended boot, where the property to watch is simply the U7 line —
    `-> PASS` rather than the `parent=0x3` partial.
- **U11FIX (this arc)** — **the same defect, three more blobs**: U7FIX repaired the iteration-denominated park
  in the U7 fixture and nowhere else, but three later fixture blobs had been copied from the *pre-fix* U7 blob
  and carried the bug forward verbatim. PA41 was an attended Pi 4 boot on a tip QEMU-gated 108/108 green, and
  U11-defer was the failing line:
  `:: U11-defer: cross-process unlink-defers-free FAIL — head=28471 ff_busy=28468 measured=true want_ff=28468 a_w=0x1 b_w=0x0 opened=true unlinked=false read=false done=2 killed=0 cleared=true c1(gone=false,alive=true) c2_alive=true c3(freed=false,reuse=true) ::`
  - **`b_w=0x0` with `done=2` is the whole conviction.** B's witness bits are `b0` open of A's file, `b1`
    unlink accepted, `b2` re-open of the gone name `-ENOENT`. `0x0` means B set *none* of them — yet `done=2`
    says B ran its `SYS_EXIT` and `killed=0` says nothing terminated it. The only path through program B that
    reaches its exit with an empty witness and no fault is the **unlink-GO park falling off the end of its
    budget**. `a_w=0x1` is the same statement about A one step later: A created and grow-wrote the file
    (`b0`), reported `A_OPENED`, and then gave up in its read-GO park while the launcher was burning its full
    5 s waiting for a `B_UNLINKED` cue that could never come. Every remaining field follows mechanically —
    `unlinked=false`, `read=false`, `c1(gone=false)` because the name was never removed, `c3(freed=false)`
    because A never reached its last close. `cleared=true` because both fixtures shut down cleanly, having
    simply given up. **Nothing was wrong with the cross-process unlink-defer path**, which is the verdict
    worth stating plainly: `sys_unlink`'s deferred free was never reached, let alone implicated.
  - **The reaper did NOT cover this leak** — worth correcting, because the boot log invites the opposite
    reading. `U11-defer: reaper freed teardown-orphaned chain @cluster 28474` appears further down PA41's
    wire, but the defer file's measured head was **28471**. 28474 is the U11-**reap** fixture's chain (the two
    demos share that one log string). DEFER.BIN's chain was left allocated *with its directory entry intact* —
    an ordinary file, not an orphan, and therefore not something the reaper is even looking for.
  - **Why this one lost harder than U7 did.** The currencies mismatch exactly as in P63 — launcher deadlines in
    wall clock, fixture parks in `SYS_YIELD` iterations — but the interval B must outlast is far longer than
    U7's. B parks at spawn and is not released until the launcher has (a) let A create DEFER.BIN and grow-write
    it, which costs an `alloc_cluster` first-fit scan, and (b) run the U11-MEASURE mount, which calls
    `first_free_cluster` — a linear first-fit scan from cluster 2. On PA41 the free set started at **28468**,
    so that is ~28k FAT entries ≈ 222 sector reads *per scan*, against a real SD card. The QEMU margins the fix
    now prints are the measurement: **B parked 83 ms** before its unlink-GO, where U7's child parked 3 ms.
    Under QEMU an emulated yield round trip costs ~1 ms, so the old `0x8000` budget covered ~30 s and 83 ms fit
    with three orders of magnitude to spare — 108/108, every run. On a Cortex-A72 the same budget retires in
    **single-digit milliseconds** while the work it must outlast grows by the SD card's latency. That is not a
    race PA41 happened to lose; it is one the fixture could not win, which is why the failure was deterministic
    rather than flaky.
  - **Fix: the U7FIX park, applied everywhere it was missed.** All seven remaining bounded GO parks — three in
    the u11defer blob, three in u11reap, two in u6owner — now step on `SYS_SLEEP_MS(1)` instead of a bare
    `SYS_YIELD`, making `0x8000` ~131 s of real 250 Hz ticks on metal and leaving QEMU's cooperative-yield
    behaviour unchanged. The budget constant, the launcher deadlines and every on-disk checkpoint are untouched.
  - **Witness: `:: [u11fix] park margin — … ::`,** `+1` spec REQUIRE and `+3` FORBIDs (108 → 109). Same
    limitation as u7fix, stated up front: `SYS_SLEEP_MS` degrades to a yield under QEMU, so **QEMU cannot gate
    the park primitive itself** — that confirmation belongs to the bench. What it does gate is the launcher's
    **parked-out assertion**: B must still be parked when its unlink-GO is released, and A must still be parked
    at both of its releases. A fixture that gave up has already run `SYS_EXIT`. B legitimately exits right
    after its `B_UNLINKED` cue, so the bare `EL0_U11DEFER_DONE` count cannot serve for A's two checks; a
    name-keyed `EL0_U11DEFER_A_EXITED` flag makes them exact. This is the fact that names the defect directly,
    and its absence is precisely why `b_w=0x0` was indistinguishable from a failed `SYS_OPEN`.
  - **Sharper witness words, so the next flight discriminates for free.** The fixtures now mark *which* park
    died: bit4 on a read-GO/unlink-GO timeout, bit5 on a close-GO timeout, bit6 when `SYS_OPEN(O_CREAT)` itself
    failed. PA41's line would have read `a_w=0x11 b_w=0x10` — self-describing — instead of a `0x1`/`0x0` pair
    that had to be traced back through the assembly. The PASS masks stay `0xF`/`0x7` exactly, so every added
    bit still fails the verdict: the fixture can still fail, and now says why.
  - **Self-heal: a failure must not disable its own detector.** The pre-flight skipped the whole demo if
    DEFER.BIN was already present ("stale image"). On a fresh QEMU image that never fires; on the bench the SD
    card is persistent, so PA41's leftover would have skipped U11-defer on **every subsequent boot** — the fix
    would have been unverifiable on the very next flight. The pre-flight now deletes the residue (it can only
    be a prior failed run's — a PASSing run leaves the name unlinked) and proceeds, logging that it did so;
    only an *undeletable* copy still skips. This restores the documented precondition rather than relaxing any
    check: all three on-disk checkpoints run exactly as before, against a freshly created file.
  - Gates: `./arroyo check` green x86_64 + aarch64, `UNAOS_WC=1 ./arroyo check` green; `./arroyo kernel8-test
    210` → **109/109 required witnesses, 0 forbidden**. Lane: `arch/aarch64/syscall.rs` (the u11defer/u11reap/
    u6owner fixture blobs + `u11defer_run` + the sentinel-exit hook) + the spec + this doc. QEMU-green ≠
    correct: the metal confirmation rides the next attended boot, where the properties to watch are the
    `[u11fix] park margin` line (all three `parked_out` reading `0`/`false`, and the margins themselves — on
    metal they will be much larger than QEMU's 83/97/101 ms) and the U11-defer line reading `-> PASS`.
- **BG-SPREAD (this arc)** — the **stacked background parent**: every `bg` launch pinned its parent task to
  the launcher's core, so background programs piled onto one core no matter how idle the rest of the machine
  was. P62 was an attended sitting: four bg vugs, each visibly slower than the last, while
  `SCHED: load c0=51 c1=99 c2=52 c3=0` stayed flat — c1 saturated, **c3 completely idle**.
  - **The meter was right; the placement was the bug.** This is the inverse of P61. There, the load line was
    accurate and the *process table* was blind; here the load line is accurate and there is nothing wrong
    with it at all. The evidence was in the scheduler's own placement witness, which printed the same thing
    for every launch: `:: SCHED: task 'bg-user' -> core 1 (policy: caller-pinned EL0, no-migrate) ::`. Every
    `bg` runs from the same shell context, so "the caller's core" is one fixed core for the whole boot.
  - **The parents stacked; the workers never did.** A bg vug's ELF-2 worker threads already spread —
    `SYS_THREAD_SPAWN`'s `place = 1` routes them through `sched::other_online_cpu`, the least-loaded
    *sibling* core. Only the parent, the task that owns the address space and does the frame work, was
    pinned. So the symptom was specifically that each new bg program slowed *the ones already running*.
  - **Why the pin was there — inheritance, not intent.** BGRUN-1 built `spawn_user_image_bg` as a mirror of
    `run_user_image`'s front half, and copied `let cpu = this_cpu()` with it. In `run_user_image` that line
    is the sys_spawn **co-location invariant**: the foreground launcher blocks in a wait loop immediately
    after the spawn, so putting the child on the caller's core guaranteed the child could not be dispatched
    until the parent yielded — which is what made the pre-EXEC1-M publish order safe. **Neither half of that
    rationale survives in `bg`.** `bg` does not wait (it returns to the shell at once, so there is no yield
    to sequence against), and EXEC1-M had already removed the dependence on co-location by publishing the
    ASID *before* the spawn: the `SYS_EXIT` rescue arm (`proc_find_unpublished`, keyed off the exiting task's
    live `TTBR0_EL1`) covers a child that runs to completion on another core before the `pid` store lands.
    The pin bought nothing and cost the entire spread.
  - **Fix: `CPU_AUTO`.** `spawn_user_image_bg` now passes the SCHED-3 sentinel instead of `this_cpu()`, so
    `pick_cpu` places the parent on the **least-loaded online core** — minimum ready-queue depth, then the
    lower rolling-window busy fraction, then a rotating cursor so equal-load cores fill round-robin. This is
    the same discipline the orphan-reaper (SCHED-3b) and the ELF-2 worker threads already use; nothing new
    was invented. **Spreading happens at SPAWN only.** Placement is still decided exactly once and the task
    is still `steal_ok: false` — EL0 slots carry per-core TTBR0/ASID state, so they remain no-migrate. The
    foreground `run` path deliberately keeps `this_cpu()`: its co-location invariant is real.
  - **No interaction with SPINHUNT or KILLBOUND**, checked rather than assumed. SPINHUNT's orphan reaping is
    keyed on the **ASID** (`orphan_kill(asid)`, `asid_live_threads`, the `SLOT_REFCOUNT` 1→0 edge) and its
    kill requests are matched at each task's own next boundary on whatever core it sits on; KILLBOUND's
    quiescence witness is likewise ASID-scoped (`ASID_GEN[owner]` bumped only at teardown) and its
    `kill_wake_parked` sweep walks the futex buckets globally. Neither reads a core index, so a bg parent
    that starts on a different core changes nothing either of them observes — and both legs pass unchanged
    in the gate below.
  - **Witness: `BGRUN-ST` leg 0c / `:: BGSPREAD: …`,** `+1` spec REQUIRE and `+1` FORBID (77 → 78). The
    single-launch legs structurally cannot catch this: stacking is a relationship *between* launches, and a
    spec REQUIRE matches one line at a time and cannot count distinct cores across several. The leg launches
    **3** copies of a new in-kernel flat fixture (KILLBOUND with the threads removed: zero one futex word and
    park on it forever — no `SYS_THREAD_SPAWN`, so three concurrent copies cannot exhaust the eight-row
    `THREAD_TABLE` and no worker's own spread muddies a witness whose subject is the parent), records each
    parent's core from `sched::last_user_placement()`, then kills and reaps all three so the process table is
    left as it was found. The assertion is **`distinct >= 2`**, not `== 3`: a load-balanced policy may
    legally reuse a core when it is genuinely still the least loaded, and `== 3` would make a correct
    scheduler flap.
  - **Verified to have teeth (A/B).** Reverting the one-line placement change and nothing else, the leg
    reports `launched=3/3 distinct=1 settled=3 -> FAIL` and the gate drops to **77/78 with 3 forbidden hits**
    (all three the same line, matched by the generic `-> FAIL` FORBIDs). `distinct=1` there is not luck: all
    three launches run from one witness task on one core, so co-location can only ever produce 1. With the
    fix the same run reports `3 bg launches over 4 online cores -> cores 0,0,1 distinct=2 PASS` — core 0 is
    the idle BSP under QEMU raspi4b (render is on core 1, input on core 3), so "least loaded" correctly
    prefers it, which is the policy working rather than a new pin.
  - Gates: `./arroyo check` green x86_64 + aarch64; `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 90`
    → **78/78 required witnesses, 0 forbidden**; `./arroyo test-arm` MISSION SUCCESS. Lane:
    `arch/aarch64/syscall.rs` (the placement one-liner, the fixture and the leg) +
    `arch/aarch64/sched.rs` (`last_user_placement` / `online_cpu_count`, witness aids only) + the spec + this
    doc. QEMU-green ≠ correct: the metal confirmation rides the attended sitting, where the property to watch
    is the `SCHED: task 'bg-user' -> core N` lines across several `bg` launches — and then whether the fourth
    bg vug still slows the first three.
- **KILLBOUND (this arc)** — the **unkillable parked task**, and the two bounded tables it wedged. P60 was
  an attended sitting on a real Pi 4, and the failure was reached by hand, not by a fixture: after several
  `bg` vug launches and kills, `kill <pid>` stopped working. The shell printed `kill armed but unconfirmed`
  and then, on retry, `already killed`; `jobs` listed the pids as `running` indefinitely; no
  `[skill] killed … confirmed=1` line ever reached the wire for them; the relaunched programs showed an
  **empty window** even though their creation blit byte-verified `PASS`; and once four such rows had piled
  up, `bg` refused **every** further launch with `process table full`. Three faces, one fault.
  - **The chain, end to end.** `THREAD_TABLE` (`syscall.rs`) is **global** and holds `NTHREAD` rows for
    the whole machine — **8 on aarch64**, raised to **24 on x86 at HEADROOM** — and a row is released
    *only* by the owning program's own voluntary
    `SYS_THREAD_JOIN`. A program that is **killed** never reaches its joins, so it leaks every row it
    holds, permanently. `user-vug` spawns two workers, so on an 8-row table **four killed vugs exhaust
    it**; from then
    on every `SYS_THREAD_SPAWN` returns `-EAGAIN`. vug does not check that return (its handles are used
    only for the join), so the next launch runs with **no workers**, its `DONE` word never reaches 2, and
    its per-frame barrier blocks in `futex_wait` **forever — before its first `SYS_WIN_PRESENT`**. That is
    the empty window. And a task parked in a futex bucket is in no run queue and is no core's `current`, so
    **neither SKILL-1 boundary can see it**: `asid_thread_leave` never runs, the ASID-scoped request is
    never settled, the row is parked `PORPHANED` for the rest of the boot. That is the unkillable pid, and
    four of them are the permanent `process table full`.
  - **Refuted while measuring.** `slot_thread_retain(asid)` was reported as never balanced. It **is**
    balanced — every spawned thread's own death path (`sched::exit` and the off-CPU `retire_killed`) calls
    `teardown_user_slot(asid)`, which decrements `SLOT_REFCOUNT`. The leak is the `ThreadRec` row, not the
    slot refcount. Worth recording, because a refcount "fix" here would have been a real bug.
  - **Fix (a) — the waits are kill-aware, from both sides.** *Before parking*, `Semaphore::wait` and
    `futex_wait` test the armed-kill flag and route the task straight into `exit()` — the wait's own
    predicate is the right place for it, and those call sites qualify as kill boundaries for exactly the
    reasons `kill_check_current`'s do (IRQ-masked, on the task's own kernel stack); the raw lock is
    released first, since `exit()` never returns. *After parking*, `sched::kill` **evicts** already-parked
    targets: `futex_wake_killed` walks all 16 buckets and `kill_wake_parked_semaphores` walks every
    `Semaphore` an EL0 task can reach (`Proc::done`/`SYS_WAIT`, `BUS_SEM`/`SYS_MRECV`, a `ThreadRec`'s
    join handle/`SYS_THREAD_JOIN` — the set is enumerable, which is why this is a sweep and not a
    registry), re-readying each so the **off-CPU dispatch boundary** retires it before it executes another
    instruction. The two orders — arm-then-park and park-then-arm — are covered one each; together the
    property is total. No permit is handed over on eviction, and that is sound because an evicted task
    **provably never resumes**: the request that matched it cannot be cleared while it is alive
    (`kill_release` needs the target retired, `kill_settle` withholds that while the task is still counted,
    `kill_retract` needs the requester to have observed it dead). `THREAD_TABLE` is swept under `try_lock`,
    never `lock` — a kill must not be able to block behind a spinlock held by a preempted task.
  - **Fix (b) — both bounded tables reclaim, each on a POSITIVE QUIESCENCE WITNESS.** Neither reclaim uses
    elapsed time, and neither ever frees a resource whose task could still be executing on cores 1-3.
    - *`Proc` rows.* `proc_reserve`'s last resort reclaims a `PORPHANED` row iff
      `sched::asid_live_threads(row.asid) == 0`. That is a proof, not an estimate: every EL0 task is
      counted by `asid_thread_enter` **before** its run-queue push (so a task that could still execute is
      counted), and the count is decremented only from `sched::exit` — past the slot teardown, the joiner
      post, and every syscall the task can make, with only the final `switch_context` left — or from
      `retire_killed`, after the Box and its kernel stack have been dropped. Zero therefore means every
      task ever entered under that ASID has passed the point where it can touch a `Proc` row. ASID reuse
      cannot forge it in the unsafe direction: a live successor re-enters the count and reads non-zero, and
      a successor can only exist if the slot was freed, which required our victim to exit first.
    - *Thread rows.* `sys_thread_spawn` scavenges when the table is full, reclaiming any row whose
      `ASID_GEN[owner] != rec.agen` — the gen word is bumped by `clear_handle_row` on
      `teardown_user_slot`'s 1→0 edge, so a bump proves the **last live task under that ASID has retired**.
      Deliberately lazy (reclaim under pressure) rather than eager at teardown: `teardown_user_slot` runs
      IRQ-masked, sometimes on the scheduler's own stack, and taking that `SpinMutex` there would add a
      lock-order hazard in exchange for nothing. The same gen word also makes `SYS_THREAD_JOIN` fail closed
      (`-ESRCH`) when a **recycled-ASID successor** would otherwise have joined and reaped its
      predecessor's thread handle.
    - A still-*living* parked victim is reclaimed by **neither** — correctly. That case is closed at the
      source by fix (a), which is why the two halves are complementary rather than redundant.
  - **The operator messages now tell the truth.** `process table full (run `jobs` to reap …)` was right for
    a table of corpses and a **lie** for a table of `PORPHANED` rows — `jobs` cannot reap those and never
    could, so following the advice taught the operator nothing and blamed the wrong resource (this cost
    real bench time). The refusal now names the state that actually caused it, and `bg_kill`'s unconfirmed
    string no longer promises that the row "settles at the task's next boundary" — the untrue part when the
    target had no next boundary.
  - **Witness: `BGRUN-ST` leg 0 / `:: KILLBOUND: …`,** `+1` spec REQUIRE and `+1` FORBID (63 → 64). The
    three existing BGRUN legs all kill **runnable** targets (VUG makes syscalls every frame, KVUG spins),
    which is why all three passed green on the very boot where the operator's Pi wedged. The new leg kills
    a target that is **parked**: an in-kernel flat blob (no SD card, no staged `.ELF`) zeroes two futex
    words, spawns two worker threads, and every one of the three blocks in `SYS_FUTEX(FUTEX_WAIT)` on a
    word nobody ever wakes. Each round waits for a **positive park witness** — `sched::futex_parked_total`
    must rise by 3 — so a passing round cannot be passing vacuously, then kills and requires
    kill-confirmed + row `Gone` + ASID drained inside a bounded window. It runs `NTHREAD/2 + 1 = 5` rounds
    because each round leaks two thread rows on the pre-fix code: rounds 1–4 fill the global table and
    round 5 is the one that must scavenge. **Verified to have teeth**: with the kill-awareness disabled and
    nothing else changed, the leg reports `rounds_ok=0 parked_ok=4 of 5 (kill armed but unconfirmed …) ->
    FAIL` — and `parked_ok=4` is the P60 chain reproducing itself in QEMU, round 5 unable even to reach the
    parked state because the thread table was exhausted by the four leaked pairs.
  - Gates: `./arroyo check` green x86_64 + aarch64; `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 60`
    → **64/64 required witnesses, 0 forbidden**; `./arroyo test-arm` MISSION SUCCESS. Lane:
    `arch/aarch64/sched.rs` (the kill primitive + the two wait primitives) + `arch/aarch64/syscall.rs`
    (proc/thread row lifecycle, the fixture and the leg) + the spec + this doc. QEMU-green ≠ correct: the
    metal confirmation rides the attended sitting.
- **BGRUN-SCAV (this arc)** — P59b sent back two bench observations. **One of them is not a bug**, and
  saying so is the more useful finding; the other is real and is fixed here.
  - **NOT A BUG: "`jobs` reports `running` for a process that already exited."** The proposed mechanism
    was that a row's state is only updated when something reaps it. **Measured, and false.** `SYS_EXIT`
    stores `PEXITED` and posts `done` before the task retires, and `BGRUN-ST` leg 1 already asserts
    exactly the claimed-broken property — a **non-reaping** `bg_poll` on an exited bg ELFHELLO returns
    `Exited(0)` — and has passed on every gate run in this repo's history.
    - The multithreaded case (a bg'd `VUG.ELF`, which is what the operator actually launched) was the one
      genuinely untested gap, so it was measured directly rather than argued: with the detached bit
      temporarily disabled to restore pre-arc behaviour, a probe polled a backgrounded vug non-reapingly
      until it flipped. It **flipped to non-running at ~3 s**, and the vug's own
      `:: UVUG: frames=300 … ::` witness printed. The row is not stale; the program was simply still
      running.
    - **Why 3 s and not "instant".** An unfocused vug still renders all 300 frames; it just gets less CPU
      and no input. So `jobs` saying `running` 1–3 s after the launch was **honest**, and `kill` finding a
      live target "every time" is the consistent reading, not a contradiction of it.
    - **Where the `dur=0s` came from.** `[gui] app-exit … dur=0s wedged=false` is emitted by
      `gui_watchdog`, which tracks the **foreground GUI session** (`main.rs`'s `on_app_enter`/
      `on_app_exit` around the screen handover). It says nothing about background processes. Correlating
      those lines with the two bg'd vugs is what produced the "they exited instantly" reading.
  - **REAL, and fixed: exited-unreaped rows deny launches.** BGRUN-1 held that a row stays claimed until
    `jobs` reaps it, and called the resulting "process table full" *honest* — right about the exit STATUS,
    wrong about the ergonomics. `MAX_PROCS` was **4** at the time (**6** since `PROCS-6`), so four
    short-lived `bg` launches with no
    intervening `jobs` exhaust the table, and the operator is refused by a table of corpses with the exact
    message `process table full (run `jobs` to reap exited background programs)`.
    - **The fix (`BGRUN-SCAV`)** is the minimal one: when `proc_reserve` finds no `PFREE` row it makes a
      second pass and reclaims a `PEXITED` one, via a `PEXITED -> PRUNNING` **CAS**. The CAS is what keeps
      reap-once intact — the row is claimed by exactly one of the scavenger and `jobs`, and a scavenged
      job's later `jobs` entry takes the pre-existing `Gone` arm. The `done` permit is consumed here for
      the same reason the reap arm consumes it (a reused entry must start at zero permits); it cannot
      block, because `PEXITED` is published strictly after the permit is posted on every path that sets it.
    - **The trade, stated:** an **unobserved exit status is discarded** (a later `jobs` prints `gone`
      rather than `exited N`) in exchange for not refusing a launch the machine can satisfy. Never silent
      — the reclaim prints `:: BGRUN-SCAV: process table full — reclaimed row N from EXITED unreaped
      pid=… (status=… DISCARDED; `jobs` will read `gone`) ::`.
    - **Witness: `BGRUN-ST` leg 1b**, `+1` spec REQUIRE (59 → 60). It spawns ELFHELLO `MAX_PROCS + 2`
      times, waits for each to exit, and pointedly **never reaps**, requiring every one to succeed. It was
      verified to be a real instrument rather than a decoration: with the scavenge disabled the leg went
      **red at the fifth launch** — `slot reclaim — spawn 4 of 6 refused (table full on corpses) -> FAIL`.
      Because the count is **derived** from the cap rather than written down, `PROCS-6` moved the fill
      point without touching the leg: at 6 rows it drives **8** launches, the last two of which only the
      scavenge can serve. Had the leg hard-coded 6 the raise would have made it vacuous.
  - **VUG-BG lifecycle (lens):** the detached bit is now cleared in `boot::teardown_user_slot`'s
    final-release arm, beside `clear_handle_row` and before `SLOT_USED` is released. ASIDs recycle, and
    `run_user_image`/`spawn_user_image_bg` only cover the two FAT-image launchers — `sys_spawn`'s
    `load_program_into_slot` and the in-kernel fixture launchers produce address spaces too. One line at
    the teardown funnel beats a rule every future launcher has to remember.
- **STAT-NAME (this arc)** — the persistence app is the **stats viewer**, and its name is **STAT**. The
  VUG/STAT rename above also renamed it `crates/user-stat` → `crates/user-kvug` (`STAT.ELF` →
  `KVUG.ELF`); the K-for-kernel prefix is simply wrong for an EL0 app that draws its own pid and a frame
  counter. This arc is the exact reverse sweep of that half: `crates/user-kvug` → **`crates/user-stat`**,
  `KVUG.ELF` → **`STAT.ELF`**, `user-kvug.ld` → `user-stat.ld`, through `arroyo`'s build and staging
  stanzas, the FAT staging name, `BGRUN-ST`'s persistence leg (which loads `/fat/STAT.ELF` by name), and
  the docs here and in `08_VIDEO/engine.md`. The `VUG.ELF` half of that arc is untouched.
  - **The serial witness tags are unchanged, for the same reason they were unchanged then**: `STAT:`,
    `BGRUN-ST:`, `UVUG:` and friends name arcs and witnesses, not files, so `pi4-regression.spec` needed
    **no pattern change** — only the file names inside its comments.
- **EXEC1-M** — the **late-publish window** in `run_user_image`, and the end of the metal-only
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
  - **UVUG is windowed.** `crates/user-vug` drops `SYS_FB_MAP`/`SYS_FB_PRESENT` for
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
  - **TAB is the window system's key.** Reserved at `user_input_enqueue` (the single router→ring choke
    point, so no app can withhold it) whenever two or more windows are in `wm::focus_ring`; it advances
    `USER_INPUT_ACTIVE` to the next owner ASID in window-id order via `user_input_set_active`, so the ring
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
    `video/wm.rs`, `crates/user-vug/src/main.rs`, `crates/user-blob` (midden + profile),
    `arch/aarch64/syscall.rs` (TAB seam + the `el0-wcb` fixture) + this doc + `08_VIDEO/engine.md`.
- **UVUG7-W (witness-state honesty)** — the closing of the s1j residual: *"the `[uvug7]` witness NEVER
  printed on the P53 metal wire."* No code was broken. The witness is **kept**, not retired.
  - **What `[uvug7]` proves.** Two arch-wide aarch64 sites, both added by UVUG-7 and both
    `#[cfg(feature = "witness")]`. (1) `arch/aarch64/timer.rs::init` — `[uvug7] ms clock: CNTFRQ=<f> Hz …
    core-count-independent`, the standing record that `arch::ms()` is derived from `CNTVCT/CNTFRQ` and
    **not** from `ticks()*4`; the global tick counter sums every core's timer IRQ, so on 4-core BCM2711
    metal the old derivation ran `ms` ~4× fast and typematic repeated ~4× too fast. (2)
    `video/screen.rs::present_surface` — `[uvug7] surface <w>x<h> scaled <n>x -> …`, the integer
    nearest-neighbour upscale factor and centred placement actually chosen for the panel in hand. Both are
    one-shot; neither is duplicated by the uvug8/9/10 family (those cover takeover deadlines, shell-path
    input and event-queue algebra — nothing about the ms derivation or the present geometry).
  - **Why it never printed on metal: default-quiet, not a dead gate.** `witness` is armed only for the
    battery commands (`test`/`test-fat`/`test-arm`/`kernel8-test`). `./arroyo kernel8` — the **flashable
    media** command, which is what P53 was staged from — leaves it OFF, so both call sites are compiled
    clean out. Proven at the byte level: a default `kernel8` image contains **zero** `uvug7` strings
    (`strings -a target/UnaOS-pi4-baremetal.img | grep -c uvug7` → 0).
  - **It fires correctly on metal — audited, not assumed.** The P57 capture
    (`rmbp-s18/cu.usbmodem143302.log`, staged from a witness-armed image at `hw-pi4@91bcc2d7`) carries
    **4 `[uvug7]` hits** — both sites, across two boots: `ms clock: CNTFRQ=54000000 Hz` and
    `surface 32x32 scaled 15x -> 480x480 at (720,360) on 1920x1200 panel`. So the witness is honest on
    silicon; only the P53 *image* could not carry it. (Note the metal panel is 1920×1200 → scale 15,
    versus QEMU raspi4b's 640×480 → scale 6 — the present witness is genuinely geometry-dependent, which
    is most of its value and is unreachable from the gate alone.)
  - **The real defect, and the fix.** Nothing named the build's witness state, so a default boot log was
    **indistinguishable** from a witness-armed boot whose gate never became true: a missing `[uvugN]` read
    as a live bug rather than as absent code. That ambiguity is what let the mystery survive four boots
    (P53–P56). `arch/aarch64/mod.rs::boot_diag` now emits, unconditionally,
    `:: AARCH64 build: witness=<on|off> — witness-gated [uvugN]/fixture lines are <PRESENT|ABSENT BY
    CONSTRUCTION (not failures)> in this image ::`. The answer is now always in the log itself. A stale comment in `arroyo` that
    claimed `witness` "gates nothing" on aarch64 and that arming it for `test-arm` was "a functional
    no-op" was **false** (UVUG-7 added the two arch-wide sites above) and is corrected — `test-arm` does
    emit `[uvug7] ms clock`, verified in `target/serial-arm.log`.
  - **Retirement considered and rejected.** `[uvug7]` proves something no other witness does, and it
    demonstrably fires. It is referenced by **no** `pi4-regression.spec` directive, so it is left unpinned
    (pinning it would force every future metal image to be witness-armed). The ms-clock line stays
    `witness`-gated rather than promoted to a boot-honesty line, because the adjacent unconditional
    `:: AARCH64 generic timer armed (CNTFRQ=… ) ::` already publishes `CNTFRQ` — `[uvug7]` adds only the
    derivation restatement, which is re-proof, exactly what DEFAULT-QUIET exists to silence.
  - Gates: `./arroyo check` green both arches; `./arroyo kernel8` builds; `./arroyo kernel8-test 120`
    MBENCH **50/50 required, 0 forbidden**; `./arroyo test-arm` green. Lane: `arch/aarch64/mod.rs`
    (the witness-state line) + `arroyo` (comment correction) + this doc.
- **BANDY-LOAD (witness load-immunity)** — the `BANDY-RT` launcher's wait on midden used to be a bare
  5 s `CNTPCT` deadline. Under QEMU that is a **host-load thermometer, not a guest-progress measure**:
  when the host is busy (parallel worktree builds), the guest gets fewer cycles per wall-second, so
  midden's work — unchanged in guest terms — no longer fits the window. The run *completed*; the
  verdict simply arrived after the launcher had given up (`done=0`, `midden_w=0x0`). That is budget
  truncation, and it recurred as a **false red** in the battery.
  - **The fix: verdict on work done, backstop denominated in scheduling opportunities.** Every
    iteration of the wait calls `yield_now()`, so each iteration is one chance for midden to run.
    `MIDDEN_YIELD_BUDGET` (1 000 000) bounds the wait in units host load does not dilate. Measured:
    an unloaded completion spends **~74 k yields** (in 4.7 s of the old 5.0 s budget — the false red
    was *that* close); the same completion under a saturated host spends **~107 k yields**. The
    wall-clock blew straight past its budget; the yield count moved by only 1.45×. That ratio *is*
    the argument for the design.
  - **Honest margin — which bound actually binds.** The two guards are not independent: the wait can
    only spend as many yields as 45 s of wall-clock affords it, and host dilation lowers the yield
    *rate*. So under saturation the **wall ceiling binds first, not the yield budget** — the
    effective budget truncates to roughly **500–700 k yields**. Against the measured loaded
    completion of ~107 k, the real immunity margin is therefore **~5–6×, not the ~13× the nominal
    1 000 000 budget suggests**. That is still ample for the observed failure mode, but the nominal
    budget is a ceiling the loaded case never reaches and must not be quoted as the margin. A future
    arc wanting true wall-clock independence would have to raise `MIDDEN_HARD_BACKSTOP_SECS` in step.
  - **What this does not weaken.** A genuine hang still FAILs, and still within a bounded time — a
    wedged midden burns the budget and falls through to the same `done=0` FAIL line. Two guards keep
    the bound honest in both directions: `MIDDEN_MIN_WAIT_SECS` (5 s) is a **floor** the budget may
    not fire before, so the wait can never truncate *earlier* than it did before this change; and
    `MIDDEN_HARD_BACKSTOP_SECS` (45 s, 9× the measured unloaded completion) is an absolute ceiling —
    and, per the honest-margin note above, the guard that actually binds under load. The PASS line and its
    `[w=…/mw=…]` ledger are byte-identical — the witness proves exactly what it proved before. The
    FAIL line gains `yields=`, which separates the two failure shapes at a glance: a value at the
    budget means the wait genuinely ran out of scheduling opportunities (a real hang), anything well
    below it means midden finished and failed on merit.
  - **Gates:** `./arroyo check` green both arches; `./arroyo kernel8` builds; `./arroyo kernel8-test`
    MBENCH **53/53 required, 0 forbidden** unloaded (1816 lines) **and under a saturated host**
    (1083 lines, load average ~200–310). The immunity was proven by A/B under the *same* load: with
    the budget forced to 0 (the old wall-clock behaviour reproduced exactly) the witness FAILs
    `done=0 yields=107432`; with the budget restored it PASSes. Lane: `arch/aarch64/syscall.rs`
    (the `bandy_rt_launcher` wait + `bandy_wait_expired`) + this doc.
  - **Siblings sharing the hazard (NOT touched — out of this arc's scope).** The bare
    `N * timer::cntfrq()` wait is the house idiom across the aarch64 witness family: ~40 sites in
    `arch/aarch64/syscall.rs` alone (the `wdeadline`/`vdeadline`/`tdeadline` spawn-and-wait triads at
    U-line, K-line and window-compositor fixtures, 2–15 s each). They are all load-sensitive by the
    same mechanism; `BANDY-RT` is simply the one whose margin was thinnest (94% consumed). Promoting
    `bandy_wait_expired` into a shared witness-wait helper is the obvious follow-on arc.
- Not yet: **revocation trees** (a derived copy — re-grant or onward re-transfer — escapes
  single-level revoke today; derivation records + `CAP_REVOKE` are that arc), the **bandy Ring-3
  delegation wrapper**, `File` transfer (descriptor migration), real `Socket` fs/net syscalls.
  Not yet (M8): an arbitrary program-by-name `sys_spawn`, and a code-signing / allowlist gate on the
  loader (`SECURITY.md`).

- **MBENCH-HONEST (the pi4 gate stops being able to lie)** — a **test-tooling** arc; zero kernel
  source touched. The pi4 gate had two independent ways to report something it had not established,
  and both were paid for repeatedly.
  - **The truncation trap.** `./arroyo kernel8-test` defaulted to an **8 s** window. 8 s does not
    finish a pi4 boot on this class of host: QEMU is killed part-way through the `u7_launcher`
    fixture cascade and the witnesses that print LAST — K3/K4/K8/K6/BANDY and others — never get a
    chance. The capture then replays at 25–41 of 63 witnesses (25–34 as reported this round; 41 when
    re-measured on this host, 501 lines against a 60 s run's 1682) — which reads *exactly* like a
    regression. Four separate executors investigated it independently in one round; one settled it
    by gating a **clean base** and getting the identical shortfall. The spec header already carried
    the warning and people still lost an hour to it — which is the evidence that the warning belonged
    in the **tool**, not the prose.
  - **The unfalsifiable battery step.** `battery()`'s pi4 step asserted one `awk` for
    `CAPSTONE COMPLETE` against the serial log. CAPSTONE prints at **line 111** of a ~1700-line
    capture, so *every* truncation this gate exists to catch still satisfied it while dozens of tail
    witnesses were absent. The step could not go red on a short log — which is why the track's
    standing law became "gate claims are evidenced by the runner's OWN mbench replay only", after two
    false-greens in one round.
  - **The fix — a third verdict.** `mbench.py` gains a `COMPLETE <regex>` spec directive: an
    **end-of-run marker**. A capture that reaches no declared marker, *or* that ends mid-line (no
    terminating newline — direct evidence the writer was killed mid-write), is **TRUNCATED /
    INCONCLUSIVE**, exit code **3**: never a pass, and explicitly *not* a regression. Precedence is
    one function (`Matcher.run_verdict`): a FORBID hit outranks everything (a `PANIC` in a short log
    is still a fault); then truncation; then a short REQUIRE/COUNT in a **complete** capture, which
    is a genuine regression and still FAILs; then PASS. **PASS semantics are unchanged** — 63/63
    required, 0 forbidden — and a spec that declares no `COMPLETE` line can never be truncated, so
    the x86/arm/jetson replays keep their exact prior behaviour and exit codes.
  - **The markers, and why they are trustworthy.** `pi4-regression.spec` declares
    `:: SCHED: task 'el0-midden' -> core` and `:: BANDY-RT:`. The first is `spawn_user_slot`'s own
    placement line for MIDDEN.BIN, and `bandy_rt_launcher` is documented in `syscall.rs` as **LAST in
    the chain** — reaching it means the boot got through every earlier fixture in the spec. It is
    **structural, not a verdict**: no regression in any witness can suppress it, so a real regression
    still reads FAIL instead of hiding behind "inconclusive". The second covers the launcher's honest
    early exits (no card / MIDDEN.BIN absent / staging failed), which are also complete runs and must
    fail on their missing witnesses. The residual is stated in the spec rather than hidden: a capture
    severed inside the ~79-line window between the midden spawn and the five BANDY verdicts, exactly
    on a newline, reports FAIL rather than TRUNCATED. Both are red; pinning anything later would mean
    pinning a *verdict*, which is precisely what would let a genuine regression disguise itself.
  - **The gate can now go red.** `kernel8-test`'s default window is **60 s** (the measured floor;
    `kernel8-test` is pi4-only, so no x86/arm caller is affected), and it now **replays the spec
    itself and propagates mbench's exit status** instead of printing a path and always exiting 0.
    `UNAOS_K8T_ASSERT=0` opts out for deliberately off-spec configurations (`UNAOS_SDIMG=0` no-card
    control, single-subsystem knob runs). `battery()`'s pi4 step therefore derives its verdict from
    that exit status, and prints a distinct TRUNCATED line in the summary so a short boot is never
    bisected as a regression.
  - **Gates:** `./arroyo check` green both arches; `./arroyo kernel8-test 60` MBENCH **63/63
    required, 0 forbidden**; `./arroyo test-arm 22` MISSION SUCCESS (the x86/arm replay paths
    unregressed); `mbench --self-test` **28/28** (was 25), the ten new cases asserting the three
    verdicts as genuinely *distinct* in both replay and follow — complete+all-witnesses → PASS,
    cut-before-marker → TRUNCATED, complete-minus-one-witness → FAIL, mid-line cut → TRUNCATED, a
    `PANIC` in a short log → FAIL (never "inconclusive"), and a marker-less spec keeping its plain
    exit codes. Reproduced end-to-end on real captures: `kernel8-test 60` → **PASS 63/63** (markers
    seen at lines 943 and 1016 of 1488); `kernel8-test 8` → **TRUNCATED 41/63, exit 3**, and the
    command says so in words instead of printing a witness count; that same 60 s capture with one
    `K3-mount` line deleted → **FAIL 62/63, exit 1**, annotated "the end-of-run marker was seen — the
    run completed, so a missing witness here is a GENUINE regression". Lane: `scripts/mbench.py`,
    `scripts/specs/pi4-regression.spec`, the `kernel8-test`/`battery` stanzas of `arroyo`, and this
    doc.
- **FLAKE-1 (the gate names its own flakes)** — a **test-tooling** arc: `unaos/arroyo`'s
  `test_kernel8()` stanza only; zero kernel source touched. MBENCH-HONEST above made the gate's
  *verdict* honest; two bench artifacts from 2026-07-28 showed the *harness* could still fail in
  ways that bypassed that verdict entirely.
  - **The silent no-capture run.** A `kernel8-test 150` finished looking green while QEMU had
    produced **no `serial-pi.log` at all**; mbench then errored `[Errno 2] No such file or
    directory` and a downstream `&&`/pipe masked it, so the wrapper ended green. A *missing capture*
    was being read as a pass — the exact class of lie MBENCH-HONEST was built to end.
  - **The race, stated precisely.** The QMP port is picked by an `lsof` pre-scan and QEMU binds it
    **seconds later**, after the image build. That is a **check-then-bind race**, and it cannot be
    made atomic from shell: the script never holds the socket, QEMU does. This host runs several
    worktree gates concurrently, so a colliding QEMU can win the bind and kill ours instantly —
    before `-serial file:` has created the log. The pre-scan is kept (it narrows the window and is
    nearly always sufficient); it is *not* the fix, and the arroyo comment now says so.
  - **The fix — a liveness gate and a bounded retry.** Within `UNAOS_K8T_LIVE_SECS` (default 5 s,
    polled at 10 Hz so a healthy launch costs a fraction of a second, and short-circuited the moment
    the pid dies) the QEMU pid must be alive **and** `$logf` must exist and be non-empty. A launch
    failing either test is killed, reaped, and relaunched on the next free port, printing a loud
    `⚠ kernel8-test: QEMU launch flake (port <n>) — retrying on <n+1>`. Bounded at **2 retries**.
  - **A fourth exit code.** If all three attempts fail — or if `$logf` is missing/empty where mbench
    would run (belt and braces) — the command exits **4: HARNESS FLAKE** and says in words that the
    run carries no verdict: not a pass, not a regression, not a truncation. **0 PASS / 1 FAIL /
    3 TRUNCATED are unchanged**; 4 only occupies ground that was previously reported as 0 or 1 by
    accident. mbench is never reached with a missing log.
  - **TRUNCATED now names host load.** Second artifact: under concurrent build/QEMU load a 150 s
    window truncated (exit 3 — honestly) where **210 s passed clean**. Not a bug, but the reader was
    left to guess. The existing TRUNCATED explanation is kept verbatim and gains one line noting the
    sufficient window scales with host load, citing the 150/210 precedent. This is the same
    MACHINE-DEPENDENT margin the spec header has warned about since BGRUN-2 — observed within *one*
    machine over time rather than between machines.
  - **Siblings sharing the shape (NOT touched — lane discipline).** `install_pi()` and
    `test_aarch64()` launch QEMU the same way and carry the same latent race. They are deliberately
    out of this arc and can inherit the retry loop as a shared helper later.
  - **Gates:** `bash -n unaos/arroyo` clean; `./arroyo check` green both arches. Four `kernel8-test`
    runs, one per verdict. (1) Healthy: `kernel8-test 210` → **MBENCH PASS 86/86 required, 0
    forbidden**, 22501 lines, exit **0**, no flake line — the healthy path is unchanged. (2) The
    race, *reproduced*: two sockets **bound but not listening** on 4600/4601 are invisible to
    `lsof -sTCP:LISTEN` (the pre-scan passes them) yet fatal to QEMU's bind — a faithful stand-in for
    losing the race. QEMU printed `Failed to find an available port: Address already in use` twice,
    the harness printed both retry lines, the third attempt bound 4602 and the run **recovered**:
    PASS 86/86, exit **0**. (3) Unrecoverable: `UNAOS_QEMU_EXTRA=-flake1-forced-bad-arg` kills every
    launch → both retry lines, then the HARNESS FLAKE message and exit **4**. (4) `kernel8-test 8` →
    TRUNCATED 64/86, exit **3**, carrying the new host-load line. Lane: the `test_kernel8()` stanza
    of `unaos/arroyo`, `unaos/scripts/specs/pi4-regression.spec`'s header, and this doc.
- **VUGCLICK (a click stops killing the vug)** — *(the click→pause half is **superseded by CLICK-ONE**
  below: a click is now focus/restore only and carries no run-state meaning; SPACE is the pause toggle.
  The half that stands is the one this arc was about — a click does not exit.)* — an **app-only** arc:
  `crates/user-vug` + this doc, no
  kernel change. P62 attended metal reported "vug is crashing". The wire showed no panic, no fault and a
  clean load — what it showed was `:: UVUG: interactive takeover at frame 24340 ::` followed immediately
  by `:: UVUG: interactive exit=click frames=36000 ::`, twice. The vugs were leaving through their own
  **designed** click-to-exit path. That rule was written for the full-screen takeover era, when the vug
  owned the panel and any click meant "done"; since **WC-C** there is no takeover mode left to reach —
  `_start` calls `SYS_WIN_CREATE` unconditionally, so every vug is a windowed app tiled beside others. In
  a windowed desktop a click is how you focus or interact, so "click exits" meant *every attempt to touch
  a vug killed it*, and **WC-J**'s instant erase of a dead window removed the ghost that would otherwise
  have shown it had exited — which is precisely why a designed exit read as a spontaneous crash.
  **Change:** a click (button press→release with drag motion under `CLICK_THRESH`) now toggles a
  **pause** of the rotation instead of exiting — harmless, reversible, and visible, and it prints
  `:: UVUG: click pause=<0|1> ::` per toggle, which doubles as proof that a click reached EL0 (clicks are
  human-rate, so the line cannot flood the log). A **drag** still rotates, unchanged (`DRAG_DIV` /
  `DRAG_CLAMP` untouched). **ESC remains the keyboard exit** and is now the only operator-driven one — it
  already existed (`K_ESC` = 0x1B), so no new key was added. The exit witness is made honest in the same
  stroke: the interactive line's reason was previously spelled `key` or `click`, so a run that merely ran
  out `INTERACTIVE_CAP` claimed a click that never happened; the pair is now
  `:: UVUG: interactive exit=<key|frames> frames=<n> ::`. **Background persistence is unchanged by
  construction:** the auto/deterministic path is untouched (no HID in QEMU → no events → no interactive
  mode → the 300-frame `checksum=0xe68285b85121ac7c` witness is byte-identical), a DETACHED vug still
  runs uncapped until `kill`, and this arc *removes* an exit path rather than adding one — nothing the
  BGRUN-ST legs depend on (they kill runnable targets) changed. **Gates:** `./arroyo check` green both
  arches; `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 90` + `mbench --replay` **77/77
  including the COMPLETE markers, 0 forbidden** (the `UVUG:` / `EXEC-UVUG:` / BGRUN-ST witnesses all
  green); `./arroyo test-arm` MISSION SUCCESS.

- **VUGLIFE (desktop vugs stop dying of old age)** — an **app-only** arc: `crates/user-vug` + this doc, no
  kernel change. P64 attended metal: four vugs "crashed" as the operator tabbed between them. The wire
  again showed no panic and no fault — it showed
  `:: UVUG: interactive exit=frames frames=<N> ::` four times, `N` = 36000 … 271484. The cause is
  `INTERACTIVE_CAP`, the last surviving **demo-era run deadline**, and the interaction that makes it lethal
  is subtle: under **VUG-BG** a DETACHED vug runs its auto path *uncapped*, so a desktop vug can sit at
  hundreds of thousands of frames; the cap is only ever tested on the interactive branch, so it is tested
  for the very first time at the instant the operator TABS TO THE WINDOW and the first input event flips
  `interactive` on — and the program exits immediately. Hence `N` far above the 36000 cap, and hence
  "it crashes when I touch it". Same relic family as **VUGCLICK**: a designed exit that a long-lived
  windowed desktop turned into a crash. **Change — split by LAUNCH MODE, using state the program already
  has** (the info-page `DETACHED` bit at `base + 0x4000`, offset `0x20`, bit 0), not a kernel-side special
  case:
  - **Detached (`bg /fat/VUG.ELF` — the desktop spawn): UNBOUNDED.** It exits on **ESC** or `kill`, never
    on a frame counter. At the frame the old cap would have fired it prints exactly one
    `[vuglife] budget waived (interactive) frames=<n>` line (latched, not per-frame) — a positive witness,
    so the next attended boot **proves** the waiver fired instead of inferring it from an absence.
  - **Foreground (`run`, and every fixture/battery launch): the bounded budget STAYS.** Gate liveness
    depends on a vug that terminates and the batteries drive foreground launches, so nothing that could
    hang `kernel8-test` was relaxed. When that exit is taken the reason now names its own cause:
    `:: UVUG: interactive exit=frames_budget frames=<n> (fixture mode) ::`. The reason is deliberately a
    **single bare token** — bench parsing reads `exit=(\w+)`, so the prose qualifier rides outside every
    parsed field rather than inside the `exit=` one.

  **Qualifying clause on "detached ⇒ unbounded":** the waiver depends on `set_detached`/`is_detached`,
  whose backing store is a **64-bit ASID bitmask**; ASIDs ≥ 64 are silently ignored and read back as *not
  detached*, so such a vug would still die at the cap. This is **unreachable today** — `boot::USER_SLOTS`
  = 8 and ASID = slot + 1, so ASID ≤ 8 — and a `debug_assert!(asid < 64)` in `set_detached` now pins that
  relationship, so widening the slot table past 63 without widening the mask trips in debug rather than
  silently un-waiving desktop vugs.

  **Untouched by construction:** the deterministic auto path (`AUTO_FRAMES` = 300 and the checksum
  witness), UVUG-8's takeover/deadline logic, and the `EL0_FOCUSED_PRESENT_COUNT` cap. **Gates:**
  `./arroyo check` green both arches; `./arroyo kernel8` clean; `UNAOS_FBW=1920 UNAOS_FBH=1200
  ./arroyo kernel8-test 90` → **84/84 required witnesses, 0 forbidden**, with
  `:: UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c ::` byte-identical and the BGRUN-ST /
  `EXEC-UVUG` legs unchanged in timing. The waived-budget path is metal-only (no HID in QEMU → no
  interactive mode), so it is **unverified in QEMU by nature** and is for the next attended boot to confirm.
- **EL0IN-FOCUS (the pointer stops being dead until first focus)** — P67v2 attended metal: after boot, mouse
  movement produced **no cursor motion at all** until the operator focused the shell window; from that moment
  the pointer was alive for the rest of the boot. The cause is a **self-disabling predicate** in the EL0 input
  router, and the chain is short enough to state in full:
  1. `pump_usb_into_gui` (`main.rs:2503`) branches on `user_input_active() != 0` **first**. With a windowed EL0
     app focused — the ordinary desktop state after `run` / `bg` — every drained pal event goes to
     `route_input_to_active_el0` and the function `return`s. `render_service`'s `Event::Mouse` /
     `Event::MouseAbsolute` arms (`main.rs:3122` / `3135`) — the `pal::cursor::move_rel` callers on the Pi's
     **live** GUI path — are unreachable in that state. (The main/GUI loop's own arms at `main.rs:1199` are
     the pre-scheduler loop that does not run on Pi baremetal+fb; `vug.rs`'s are inside kernel full-screen
     apps, a different branch of the same pump.)
  2. Inside the router, FOCUS-VIS's system-cursor keep-alive was gated on `pal::cursor::has_reported()`
     (`main.rs:2137`, pre-fix), whose latch (`cursor::LAST_INPUT_MS`) is stamped **only** by `move_rel` /
     `set_abs`.
  3. So on a boot where focus reached an EL0 app **before** the first pointer report, both paths that could
     set the latch were unreachable: the shell arms by (1), the router arm by its own gate. The predicate
     could only become true through a path it had itself disabled — the shared pointer position never moved,
     `cursor::visible()` stayed false, and `video::cursor::repaint` drew nothing. Reports were **not lost**:
     `user_input_enqueue` delivered them and `[el0in] routed N event(s) to active EL0 ring` fired throughout.
     What was dead was the *system cursor*, not the routing.
  4. TAB to the shell → `user_input_set_active(0)` → the next pump pass takes the bare-shell drain →
     `gui_send` → `render_service`'s ungated `move_rel` → latch stamped. `has_reported()` is true from then
     on, so the router's keep-alive works for the rest of the boot. Exactly the observed "dead, then alive".

  **Fix (small, `main.rs`-only):** scope the guard to what it was actually protecting. It existed so the
  boot-time `input_router_selftest`, which drives the real router with a **synthetic** `Event::Mouse`, would
  not arm a cursor and print `[cursor] armed` on a QEMU panel that has no pointer. That is now a
  `ROUTER_SELFTEST` flag, set for the duration of the selftest and cleared after its last router call: the
  synthetic events still arm nothing and **QEMU gate output is unchanged**, while every *real* pointer report
  moves the system cursor from the first report of the boot regardless of who holds focus.
  `pal::cursor::has_reported()` is kept (it is still the honest "does this machine have a pointer?"
  predicate) but now has no callers; its doc comment records why it stopped being the router's gate.
  Lane: `main.rs` + one doc comment in `pal.rs` + this doc. `video/wm.rs` and `video/cursor.rs` untouched.

  **Audited and deliberately NOT fixed here — click-to-focus does not exist.** The brief's second half
  ("clicks should focus-raise what is under the cursor") is not a regression; it was never built. Focus
  changes have exactly three sources: `run_user_image`'s grant and clear
  (`arch/aarch64/syscall.rs:7034` / `7175`) and the TAB ring (`wc_shell_focus_key` → `user_input_set_active`
  → `wm::focus_changed`, `syscall.rs:11777`). The pointer-button path has **no window hit-test at all** —
  while an EL0 app holds focus a `Button` goes straight into its ring via `route_input_to_active_el0`, and on
  the shell path `click1_dispatch` (`main.rs:3022`) hit-tests only *console vs status strip*
  (`click1_hit_test`, `main.rs:2980`), never the `wm` window table. A click on an unfocused window is
  therefore delivered to the **focused** app instead: the aarch64 reading of the x86 CLICK-3 "15/16 clicks
  eaten" datum is that the clicks are not dropped, they are routed to the wrong window because nothing maps
  cursor position → window. Building it means a new `wm` hit-test seam (position → window id → owner ASID)
  plus a focus-set on the press edge — a new feature in the compositor's shared convergence surface, not a
  small fix. Left for its own arc.

  **Gates:** the session that made this change ran in a container with **no C toolchain (`cc` absent, so
  every build script fails) and no `qemu-system-aarch64`**, so `./arroyo check` and `./arroyo kernel8-test`
  **could not be run and are unverified**; only a parse-level syntax check of the two edited files was
  possible. The change is metal-behaviour-only by construction (QEMU raspi4b delivers no HID, so no real
  pointer report exists on any gate boot, and the selftest's synthetic events stay suppressed exactly as
  before), but the gates still owe a run before this is trusted.

- **CLICK-ROUTE (a click goes to the window under the cursor, not to whoever is focused)** — the arc
  EL0IN-FOCUS deferred, built. **P69, attended:** *"out of focus clicks cause the focused vug to stop."*
  Clicking anywhere — another window, the desktop, the console — click-paused the **focused** vug, because
  the `Button` event was delivered to the focused app's ring regardless of pointer position. That is the
  audit's finding acted on rather than restated: focus had exactly three sources (the `run` grant, its
  clear, and the TAB ring), and the button path had **no window hit-test at all**.

  **The seam (`video/wm.rs`, ~40 lines):** `pub fn hit_test(x, y) -> Option<(WinId, owner_asid, z)>` — the
  topmost **visible** window whose outer box (chrome included) contains the point. Read-only, one `TABLE`
  lock, a scan of eight rows, **no new lock and no new lock order**; the same shape as `focus_ring` and
  `occluders`. Two exclusions, both inherited rather than invented:
    - **Below the shell is not hittable.** "Visible" is a *position*, not a flag: `SHELL_Z` is allocated
      out of the same counter window z's come from, so `above_shell` — the predicate the **compositor**
      uses to decide whether a row draws at all — is the predicate used here. What you can click is
      exactly what you can see, so clicking a console that covers a window reaches the console.
    - **Compat rows are excluded,** for the reason `focus_ring` excludes them: the full-screen
      `present_surface` shim carries owner ASID 0 and is not addressable as a focus target, so a "hit" on
      one would name nobody. A full-screen app reads as *no hit*, and the router's fallback is what keeps
      its clicks working (below).
  This is deliberately the whole of the window system's contribution — the convergence surface the x86
  tree inherits under GR7. The **policy** lives in the arch input router, not in `wm`.

  **The routing rule (`arch/aarch64/syscall.rs::wc_click_route`), on a PRESS edge:**
    - **hit on a DIFFERENT window than the focused one** → raise it first through the *one* focus
      primitive that exists — `user_input_set_active` then `wm::focus_changed`, in that order, exactly as
      `wc_focus_key` calls them — then let the press fall through to the enqueue, which re-reads the now
      **new** active ASID and delivers there. No second focus path was invented: this is a new **caller**
      of the WEDGE path, so a click-driven raise emits the same `<F1>`–`<F9>` breadcrumbs a TAB does.
      *(Superseded by CLICK-SWALLOW below: the raise stands, the fall-through does not — a
      focus-changing press is now consumed. Left standing as the record of what was tried.)*
    - **hit on the focused window** → delivered exactly as before. The common case, unchanged.
    - **no window hit** → the click is the desktop's or the console's, not the focused app's, so it is
      **consumed** rather than delivered. That single arm is P69's fix. Two deliberate limits on it:
      with focus at the **shell** (`cur == 0`) nothing is consumed — the caller's normal path *is* the
      shell path (`gui_send` → `click1_dispatch`), which is where a desktop click belongs; and if the
      focused app owns **no hittable window at all** the press is delivered as before, because that is the
      **full-screen** app (its compat row covers the panel but can never be hit) and dropping its clicks
      would break UVUG's own click-to-exit.
    - A miss **does not move focus.** `focus_changed(0)` gives `SHELL_Z` the fresh z and buries every
      window under the console (the FOCUS-VIS "shell" leg), which is a far larger claim than P69 makes.
      Routing a click and moving focus are separate decisions here; only the hit arm does both.

  **Press/release pairing.** A release delivered to an app that never saw the press is a **fabricated
  click** — for a click-to-pause vug, an invented click. So the release edge is never hit-tested and never
  re-routed: it is compared against `CLICK_PRESS_TARGET` (where the press went, or a `DROP` sentinel) and
  either follows the press or is dropped. A TAB, or the app exiting, between press and release costs the
  release; it never fabricates one in a second app. The router keeps its **own** previous-mask tracker
  rather than sharing `main.rs`'s `CLICK1_PREV_MASK`: that one belongs to `click1_dispatch` and only ever
  sees the events that actually *reach* the shell. `wc_click_route` is idempotent per edge (the mask is
  swapped on entry), which is what lets the shell caller re-enter it through `user_input_enqueue`.

  **Two callers, one body,** mirroring `wc_focus_key` / `wc_shell_focus_key`:
    - `user_input_enqueue` — the single choke point every event bound for an EL0 ring passes through, right
      after the TAB interception and for the same reason. A consumed click returns `false` (**not**
      queued), so `[el0in] routed N` stays truthful.
    - the **bare-shell drain** in `pump_usb_into_gui` — click-to-focus from the shell slot, where today a
      click on a live `bg` app's window does nothing at all. On a raise it delivers the press itself and
      **breaks** the loop, exactly as the TAB interception breaks: that loop's destination is fixed at
      `gui_send` but focus has just moved. The press is delivered *there* rather than left for the next
      pump pass because `user_input_set_active` drains `pal::EVENT_QUEUE` as part of granting focus (the
      UVUG-8r2 "a fresh focus starts clean" contract), so an event left behind would not survive the raise.

  **Untouched by construction:** the TAB ring and both run-grant focus sources; `focus_changed` and the
  WEDGE breadcrumb chain (new caller, not a new path); `click1_dispatch` and its console/status-strip
  hit-test, which still owns every click that misses the window layer; and the `SCREEN_APP_ACTIVE`
  peek/requeue branch, where the events belong to a kernel full-screen app's own drain.

  **Residual, stated:** on the shell path the shared cursor is moved by `render_service` one `GUI_CHANNEL`
  hop downstream of the router, so the hit-test reads the position as of the last motion report the render
  task has already consumed. A click is preceded by the pointer coming to rest, so the lag is nil in
  practice; the EL0-focus path has no such gap (FOCUS-VIS made the router move the cursor itself).

  **Witnesses.** `[clickroute] press hit asid=<a> win=<w> (was <b>)` — emitted on the **refocus arm only**,
  the one arm that changes behaviour, and human-rate by construction, so it needs no throttle. Plus a
  headless-drivable selftest, `wm::hittest_selftest`, ordered after every one-shot per-window latch and
  self-cleaning on the FOCUS-VIS pattern: two windows at one origin, five legs — *inside* (the stack is
  addressable), *topmost* (the **later** window owns the overlap, not the first matching row), *raise*
  (after `focus_changed(A)` the same point hits A — the z-order and the click-order are one order),
  *outside* (misses are misses; the desktop arm consumes on the strength of a `None`), *hidden* (after
  `focus_changed(0)` the shell is above both and the point hits nothing). A table test rather than a
  read-back, because unlike FOCUS-VIS the question is about an **ASID**, not a colour — and because the
  pointer is exactly what QEMU raspi4b does not have.

  **Gates (all run this session; container `cc`/`ld`/`qemu-system-aarch64` bridged from `/run/host` with
  `--sysroot=/run/host`, the CURSOR-8 recipe):**
    - `./arroyo check` → ✅ x86_64 OK, ✅ aarch64 OK.
    - `UNAOS_QMP_PORT=4501 ./arroyo kernel8-test 210` → **✅ MBENCH PASS — 86/86 required witnesses,
      0 forbidden hit(s), 26281 lines scanned**, with
      `[clickroute] hit-test at (215,135) inside=true topmost=true raise=true outside=true hidden=true -> PASS`
      and `:: USER: input router — routed=2 (key+mouse) … :: PASS ::` unchanged (the router selftest pushes
      Key/Mouse/Timer and **no** Button, so it never consults the hit-test and stays deterministic).
    - the aarch64/virt leg: `./arroyo test-arm` could not resolve AAVMF firmware in this container (arroyo
      searches five absolute paths, none writable here and no override knob), so the run was performed by
      **replicating `test_aarch64`'s QEMU invocation verbatim** against the ESP arroyo had just packaged,
      with `QEMU_EFI-pflash.raw` bridged from `/run/host` — 4202 serial lines, **0 FAIL, 0 panic**, boot
      complete through `block:up`, `MOUSE-1` enumerating the `usb-tablet`. It is a no-regression leg by
      construction as well as by observation: the routing changes live in the Pi router
      (`pump_usb_into_gui`, `baremetal`-gated) and in `user_input_enqueue`, which has no caller on virt, and
      the window-verb witness block that hosts `hittest_selftest` does not run there at all.
    - **Metal is what proves the arc.** QEMU raspi4b delivers no HID, so no real `Button` exists on any
      gate boot and **every routing arm above is unverified in QEMU by nature**; the gates prove the
      hit-test, the wiring, and no regression. The next attended boot should see `[clickroute] press hit`
      on a click into an unfocused window, and — the actual P69 verdict — a focused vug that **keeps
      running** when the operator clicks somewhere else.

- **CLICK-SHELL / CLICK-SHELL r2 (a desktop click focuses the shell — for every focus, not just a
  windowed one)** — two rulings against the CLICK-ROUTE bullet's last sub-point above, which is now
  superseded and left standing only as the record of what was tried.

  **CLICK-SHELL (P71, attended).** "A miss does not move focus" left the out-of-focus vug holding the
  keyboard while the operator was demonstrably interacting with the shell, and left `TAB` as the only
  gesture that reaches the console at all. The miss arm now does what the hit arm does with the shell as
  the target: the one focus primitive, `user_input_set_active(0)` then `wm::focus_changed(0)`, which is
  literally the shell slot of `wc_focus_key`'s ring — no second shell-focus state. The burial of every
  window under the console is the intended effect and is the same z-order move `TAB`-to-shell has always
  made. The press stays **consumed** (`CLICK_TARGET_DROP`, so the release is dropped with it): a
  press/release pair must not be split, and the console never saw the press edge.

  **CLICK-SHELL r2 (P72, attended):** *"shell can't be focused until it's cycled through a tab
  sequence."* CLICK-SHELL shipped its full-screen exemption as `click_owner_is_windowed(cur)` — deliver
  unless the focused app is in the **focus ring**. That is the wrong question. The exemption exists for
  the app that owns the **panel** without owning a window; the focus ring answers "does this app own a
  window at all", and the two coincide only while every focused app is either windowed or full-screen.
  The bench state that falsifies it is ordinary — a focused app with **no window and no compat row**:
  a `run` program between the focus grant in `run_user_image` and its first present (or one that never
  presents, i.e. every batch program, for its whole run), or a windowed app that closed its last window
  and kept running. There, every desktop press took the deliver arm, went to an app that owns nothing on
  the panel, and printed no line at all — which is why the P72 capture has only two `press miss` lines
  in eleven thousand. `TAB` did not rescue it in one press either: `wc_focus_key`'s unknown-focus arm
  sends a focus that is in no ring slot to `ring[0]`, **not** to the shell, so the operator had to cycle
  the whole ring before the shell slot came up. That is the report, verbatim.

  **Fix:** the predicate asks the question the exemption was written for. `wm::compat_live()` (new,
  read-only, `COMPAT_WIN` + the existing `is_compat_row` identity test) answers "is a full-screen app
  presenting"; the router's miss arm becomes `cur != 0 && !click_owner_is_fullscreen(cur)`. `hit_test`
  has already answered "no **window** owns this pixel", and the compat row is the only other thing that
  can own it — so everything else is the desktop, whoever holds focus. UVUG's click-to-exit is untouched
  (a live compat row still delivers); the `cur == 0` no-op is unchanged; `TAB`, `focus_changed` and the
  press/release pairing are untouched.

  **Witness.** `hittest_selftest` gains **leg 7, `bare=`** — the same press from a focus that owns
  nothing (synthetic ASID `0xC0C`, never passed to `create`), asserting the press is consumed **and**
  both halves of the focus primitive moved to the shell (`user_input_active() == 0` *and* `FOCUS_ASID ==
  0`); `skip` when a compat row is live, since that is the arm it must not assert against. Leg 6
  (`shell=`) covers the windowed focus and passed on every gate boot for the whole time the bench could
  not click into the shell — leg 7 is the honesty repair. Verified by reverting the predicate alone:
  `bare=false -> FAIL`, one forbidden hit, gate red.

  **Gates:** `./arroyo check` ✅ x86_64 / ✅ aarch64; `./arroyo kernel8-test` **✅ MBENCH PASS — 86/86,
  0 forbidden**, with
  `[clickroute] hit-test at (215,135) inside=true topmost=true raise=true outside=true hidden=true shell=true bare=true -> PASS`
  and both legs' router lines (`press miss … (was 3082)`, `press miss … (was 3084)`). **Metal still
  proves the arc** — QEMU raspi4b delivers no HID, so the live arms remain unverified in QEMU by nature.

- **CLICK-SWALLOW (a focus-changing click is a window-manager gesture, not app input)** — the ruling on
  the item VUGPAUSE-2r2 flagged and deferred to this lane (see its "Not changed" note below), which
  supersedes the CLICK-ROUTE bullet's **first** sub-point exactly as CLICK-SHELL superseded its last.
  *(The rule stands; its motivating example does not. **CLICK-ONE** below removed the second meaning a
  delivered click carried — "for a vug, a delivered click **is** the pause toggle" is false by design
  since that arc — so read this entry for its general form: an app never sees a click it did not own the
  focus for.)*

  **P73, attended:** *"sometimes a click gets lost… restarting a vug I have to click twice."* CLICK-ROUTE
  shipped the hit arm as raise-then-**fall-through**: the press that raised a window was then delivered
  to it. For a vug, a delivered click **is** the `VUGCLICK` pause toggle, so the one gesture that
  restores a backgrounded vug carried two meanings at once — *focus this* and *toggle this* — and the vug
  came back **paused**. It looks dead; the operator clicks again; that second (now ordinary, focused)
  click resumes it. That is the "click twice", and it is also the whole of the pattern the report could
  not pin down: it bites exactly when the clicked vug was **not already focused**, and never otherwise.
  Note this is not a lost click at all — both clicks arrive, the first one just spends itself on a
  meaning the operator did not intend.

  **The rule: an app never sees a click it did not own the focus for.** A press that *changes* focus is
  addressed to the window system, not to the app. The hit arm now does what the miss arm has always done
  — `CLICK_TARGET_DROP`, return `true`, and the matching release is dropped with it so no half-pair
  reaches anyone. A press on the **already focused** window under the cursor is untouched and still
  delivered: that is ordinary app input and it is the common case. Click-to-focus **without**
  focus-follows-through, which is where every window manager that has thought about it converged; it
  costs the operator exactly the one click they were already spending on a refocus, only now that click
  does one thing instead of two.

  **What the swallow does *not* touch — the restore chain.** Both VUGPAUSE-2r2 wake edges fire *before*
  the consume decision, from inside the two calls the arm already made: the focus arrival
  (`user_input_set_active` → `user_input_wake_edge(asid, "focus")`) and the unhide (`wm::focus_changed` →
  `set_hidden(asid, false)` → `user_input_wake_edge(asid, "unhide")`). The swallow withholds a **ring
  push** and nothing else, so a press on a parked, hidden, unfocused vug still produces the `[vugpause2]
  resume` pair and still re-readies the waiter — the vug wakes, observes its own restore, and is simply
  not handed a click. A fix that consumed the press *early* would have passed every behavioural check and
  reintroduced P72 in a worse form (the vug would not come back at all), which is why the witness asserts
  this edge and not only the delivery.

  **Both callers converge without a second decision.** The EL0-focus path (`user_input_enqueue`) reads
  `true` and returns "not queued". The **shell** path (`main.rs`'s `Button` arm) reads `true` and
  `continue`s its drain — which lands it exactly where its old `break` did, because
  `user_input_set_active` has already drained `EVENT_QUEUE` as part of granting focus, so the loop finds
  nothing behind the press either way. `main.rs` is unchanged; its `user_input_active() != before` arm is
  now unreachable and left standing as the defensive check it was.
  UVUG's click-to-exit and the compat row are untouched: `hit_test` excludes compat rows and owner-0
  rows, so a full-screen app still reads as *no hit* and still takes the miss arm's deliver exemption.

  **Witness.** The router's line names the disposition — `[clickroute] press hit asid=<a> win=<w> (was
  <m>) swallowed` — and `hittest_selftest` gains **leg 8**, three fields, because the fix fails in three
  directions: `swallow=` (an unfocused hit moves focus **and** the owner's ring depth stays 0, checked
  after the press *and* after the release), `deliver=` (the very next press, now on that focused window,
  *is* queued and the ring depth is 1), `wake=` (the named-edge counter advanced by 2 across the
  swallowed press — focus arrival, then unhide). Two new read-only seams carry it: `user_input_depth(asid)`
  (unread events in a ring — the only honest way to ask what the *app* got, as opposed to what the
  *router* decided) and `user_input_wake_edges()` (cumulative named wake edges *asked for*, which is the
  question a headless gate can answer; `USER_INPUT_WAKES` counts waiters actually released and is always
  0 there). Leg 8 is the only leg needing a real private-slot ASID — rings exist only for slots — so it
  borrows the **highest** slot (8, the last one a live app would hold), skips if that slot holds focus or
  owns a window, and hands it back reset. It places its window **under** the live cursor rather than
  moving the pointer, and drives `user_input_enqueue` rather than `wc_click_route`, since the claim is
  about delivery and the push lives on the far side of the router.

  **Gates:** `./arroyo check` ✅ x86_64 / ✅ aarch64 (and with `UNAOS_WITNESS=1`); `./arroyo kernel8-test`
  **✅ MBENCH PASS — 86/86, 0 forbidden**, 6177 lines, first run, with
  `[clickroute] press hit asid=8 win=3 (was 3082) swallowed` and
  `[clickroute] hit-test at (215,135) … shell=true bare=true swallow=true deliver=true wake=true -> PASS`.
  `[vugmin] wm scope=fixture hides=0 unhides=0 … -> DORMANT` unchanged — leg 8 raises but never buries,
  so it perturbs no counter. **Metal still proves the arc**: QEMU raspi4b delivers no HID, so the
  operator-facing claim (one click restores a vug *running*) is a bench observation by nature.

- **VUGPAUSE (a paused vug stops burning cores)** — an **app-only** arc: `crates/user-vug` + this doc, no
  kernel change. **VUGCLICK** made a click *pause* a vug, and that is as far as it went: pause froze the
  frame's **advance** (the orientation stopped changing) while the frame **loop** kept running at full
  rate. Every paused frame still drained input, released both workers, rasterised 128×128 twice,
  futex-barriered and **presented** — redrawing a surface that could not differ from the one already on
  the panel. P67v2 paid for that on silicon: six click-paused vugs, six full render pipelines, cores
  pinned with every crystal standing still. **Change:** when the vug is `paused` **and** its render state
  is unchanged since the last present, the frame is skipped in full — no `PHASE` store (the workers stay
  parked on their yield-poll instead of rasterising), no inline raster, no barrier, **no present**. The
  idle loop is the input half of the normal frame plus one `SYS_YIELD`, and nothing else.
  - **The predicate** is `paused && presented && presented_overlay == (detached || interactive) &&
    ay == last_ay && ax == last_ax && dist == last_dist`, compared against the state recorded at the last
    present. The `presented` term means the first frame is never skipped; the `presented_overlay` term
    means the frame that first turns the VUGFPS overlay on is never skipped; the three state terms are
    everything this program can put on its surface. **The first frame after unpause, or after any state
    change, renders normally** — the predicate simply stops holding.
  - **Unpause latency is one idle iteration** (a poll plus a yield), because the click that unpauses,
    ESC, and a drag are all drained by the same `drain_input` the rendering path uses. There is no
    separate wake channel to get wrong.
  - **Never a park.** VUGGUARD/P60's lesson is that a vug blocked in an unbounded `futex_wait` is an
    unkillable empty window. This idle path blocks on nothing: it is a runnable yield loop, so the
    window stays live, `kill` works, and the compositor sees an ordinary running process.
  - **`frame` counts frames PRESENTED**, so it does not advance while idled. That is what makes the
    **VUGFPS** readout honest: the once-per-second refresh (now factored into `fps_refresh`, called from
    *both* the rendering path and the idle loop) keeps running and the rate falls to **0** rather than
    freezing on a stale number. A changed digit is the only thing that can still reach the panel while
    idled — overlay only, one present, at most once per second. `draw_fps` therefore clears a **fixed
    maximum-width** backing box (`FPS_BOX_W`) rather than one sized to the current digit count: with no
    band clear behind it, a shrinking readout (47 → 0) would otherwise strand the old digit's pixels.
  - **A foreground (fixture-mode) vug held paused also stops consuming `INTERACTIVE_CAP`.** Deliberate,
    not a hole: pause is operator-driven, so a paused foreground vug is one an operator is holding, and
    it still exits on **ESC** or `kill`. No battery leg can reach it — QEMU delivers no HID.
  - **Witness:** one `[vugpause] idle engaged frame=<n>` at the first engagement, latched, never
    per-frame (the same discipline as `[vuglife] budget waived`).

  **Untouched by construction:** `paused` is set only inside the `interactive` branch, and `interactive`
  is armed only by a real input event — QEMU raspi4b delivers no HID, so the deterministic auto path
  never evaluates the idle predicate as true and its geometry, code shape and 300-frame
  `checksum=0xe68285b85121ac7c` witness are unchanged. `draw_fps` is likewise reached only when
  `detached || interactive`, which the foreground auto path is not. **Gates:** see the arc's landing
  report — the idle path itself is metal-only by nature (no HID in QEMU → no pause), so it is
  **unverified in QEMU** and is for the next attended boot to confirm; what QEMU proves is the negative,
  that the deterministic path is untouched.
- **KEYSTAT (this arc)** — an **audit** arc: two P69 attended anomalies traced to their exact
  predicates. One reporting hole fixed (`arch/aarch64/exceptions.rs`); the other chain is named and
  left alone, because its fix lands outside this arc's lane.

  **Anomaly 1 — "key repeat times out".** Holding a key repeats for a while and then stops with the
  key still down. There is no repeat *counter* and no per-hold budget; the bound is a **liveness
  window**, and there are two of them. `typematic_tick` (`pal.rs`) picks the window at
  `let window = if STREAMS_WHILE_HELD { LIVENESS_MS } else { HOLD_MAX_MS }` and disarms on
  `last != 0 && now - last > window` — `KEY_P1 = 0`, repeat over.
  - **Chain A — the UVUG-9 backstop.** `HOLD_MAX_MS = 30_000`. A strict `SET_IDLE(0)` keyboard sends
    one report on the press and nothing more, so `LAST_REPORT_MS` freezes at the press and the hold
    ends at 30 s (~750 characters at `RATE_MS = 40`). This is **deliberate** — UVUG-9 chose a coarse
    finite bound over "forever" — and it announces itself: `[uvug9] typematic hold-max …`.
  - **Chain B — the sticky streaming verdict.** `STREAMS_WHILE_HELD` latches once
    `IDLE_RUN_TO_LATCH = 4` consecutive byte-identical held-set reports arrive during a hold, and is
    **sticky for the rest of the boot** (cleared only on a keyboard detach). Once latched the window
    is `LIVENESS_MS = 1000` for *every* later hold, so a keyboard that re-reports only
    intermittently — or with a period above 1 s — stops after ~15 repeats. That disarm is
    **deliberately silent** (see the comment at the `window == HOLD_MAX_MS` guard: the tight window's
    disarm "is the ordinary, expected end of a hold"), so chain B leaves *nothing* on the wire and is
    indistinguishable at the bench from the ~10-repeat stop UVUG-9 exists to have removed.
  - **The distinguishing evidence is one line:** `[uvug9] typematic hold-max` present ⇒ chain A (30 s,
    by design); absent ⇒ chain B.
  - **What is arguably a bug, and why it is not fixed here.** Neither disarm re-arms. Re-arming
    requires a PRESS edge (`typematic_note_report` stores `KEY_P1` only when `newest_press != 0`), so
    a report that positively proves the key is *still held* (`held.contains(&k)`) cannot restart the
    repeat — the operator must lift and re-press. For chain B that is a plain non-re-arming cap: the
    tracker disarms on silence and then ignores the very evidence that would refute it. The repair
    belongs in `crates/kernel/src/pal.rs`, which is **outside this arc's lane** (`arch/aarch64` +
    `user-stat`), so it is recorded here rather than attempted. Suggested shape for whoever owns it:
    on a post-disarm report whose held set still contains the last-armed ascii, re-arm at
    `DELAY_MS`; and scope the `STREAMS_WHILE_HELD` verdict to the hold that earned it rather than to
    the boot.

  **Anomaly 2 — a `stat` instance died with nothing on the wire.** `kill <pid>` answered
  ``already exited — run `jobs` to reap it``, which is `bg_kill`'s **`PEXITED`** early-out. Working
  back from that state, a `Proc` row on aarch64 reaches `PEXITED` from exactly three places, and only
  two of them can name a `bg` app:
  1. **`SYS_EXIT`, generic live-child arm** (`syscall.rs`, `proc_find_running` → `status`/`PEXITED`/
     `done.post()`) — **silent by design**; a normal EL0 exit prints nothing.
  2. **fault-kill** (`record_el0_kill`'s EXEC-1 arm → `EXEC_KILLED_STATUS` + `PEXITED`), always
     preceded by the `:: EL0 FAULT: … KILLED … ::` line in `aarch64_el0_fault_handler`.
  3. the named-fixture arms (`u4-child`, `el0-u7*`, K2/midden, …) — unreachable for a `bg` app.

  Everything else that retires an EL0 task leaves the row **`PRUNNING`**, not `PEXITED`:
  `kill_check_current` and `retire_killed` (SKILL-1's two kill boundaries) post no status, so a
  SKILL-1 kill reads as `running` / `kill armed but unconfirmed`, never `already exited`. The
  `[uvug8]` orphan arms and the `BGRUN-SCAV` / `KILLBOUND` reclaims all print, and the latter two
  leave `gone`, not `already exited`.

  **`crates/user-stat` has no designed exit that could account for it.** The loop has no frame cap,
  no timeout and no input-driven quit (it never calls `SYS_INPUT_POLL` at all); `SYS_WIN_PRESENT`'s
  return is explicitly discarded and `getinfo()` falls back to pid 0 rather than exiting. The single
  `exit(1)` is the `SYS_WIN_CREATE` failure at startup — before any window exists, so it cannot
  explain an app that was on the panel — and it prints `:: STAT: SYS_WIN_CREATE failed ::` first.
  (`ALIVE_MARK = 40` is a one-shot print, not a cap.) The VUGLIFE-era "run deadline masquerading as a
  crash" shape is **not** present in this app.

  **Which leaves the reporting hole, and it is a real one.** A fatal EL0 fault does print — but the
  line named only the **task name**, and every `bg` launch runs under the single literal `"bg-user"`
  (`spawn_user_image_bg`). With two `STAT.ELF` instances on the panel, the line said "one of them
  died" and nothing more; the operator's `kill <pid>` was the first evidence of *which*, and the
  death reads as silent. **Fix:** `aarch64_el0_fault_handler` now prints
  `:: EL0 FAULT: task '<name>' pid=<n> KILLED — EC=… ISS=… ELR=… FAR=… ::`. `pid` comes from
  `sched::current_id()` — the same id `SYS_GETPID`/`SYS_GETINFO` return, the same value
  `spawn_user_slot` gave the `Proc` row, the same number `jobs` prints, `kill` takes and the app
  draws in its own window. It is **lock-free** (one `percpu` read, one `Acquire` load, one field
  read), which is the binding constraint on this path: the handler runs at EL1 with DAIF masked on
  the faulting task's own kernel stack and must not reach for anything the interrupted context could
  hold. One line per kill, bounded, no new state. No spec pattern matched the old text, so
  `pi4-regression.spec` needed no change.

- **PAL-TYPEMATIC (this arc) — chain B closed, exactly as KEYSTAT specified it.** KEYSTAT named the
  defect and left it for the owner of `crates/kernel/src/pal.rs`; this arc is that repair. It was
  **verified still present at HEAD first**: `git log 3b5cd2e5..HEAD -- crates/kernel/src/pal.rs` is
  empty, i.e. the typematic tracker had not been touched since the audit, and both predicates read
  exactly as the audit described them. Nothing about the arc depends on the P73 TAB report — Peter has
  since corrected that premise (TAB repeat *does* fire; the slowness he saw is per-cycle window
  **drawing**, a different arc). Chain B is a real defect on its own evidence, and this is its fix.

  **1. A lapse now re-arms on evidence.** Every disarm in `typematic_tick` stored `KEY_P1 = 0`, and the
  only writer that put a key back was the PRESS-edge arm at the bottom of `typematic_note_report`
  (`newest_press != 0`). A report proving the key was **still held** — `held.contains(&k)`, the
  strongest evidence the tracker ever receives — therefore could not restart the repeat: the operator
  had to lift and re-press. The liveness guard disarmed on *silence* and then ignored the one fact that
  refuted its own inference. The lapse arm now goes through `typematic_lapse_disarm`, which clears the
  armed slot and **parks** the key in `LAPSED_P1`; the next report whose held set still contains it
  re-arms at `DELAY_MS`. `LAPSED_P1` is not a second armed slot — nothing repeats off it — and the two
  absolute release paths outrank it unconditionally: a report **without** the key clears it (layer 1)
  and a **detach** clears it (layer 2). The re-arm therefore cannot reopen the P51 stuck-repeat wedge.
  The fresh `DELAY_MS` (rather than `RATE_MS`) is deliberate: a device re-reporting faster than the
  initial delay can never turn re-arming into a free-running spew.

  **2. The streaming verdict is scoped to its hold.** `STREAMS_WHILE_HELD` latched on the first hold
  that produced `IDLE_RUN_TO_LATCH` idle re-reports and was **sticky until detach**, so one streaming
  hold imposed the tight `LIVENESS_MS` window on *every* later hold of that boot — including holds
  during which the device happens not to re-report, which then stopped after ~15 repeats and did so
  **silently** (the tight window's disarm is deliberately quiet, which is what made chain B
  indistinguishable at the bench from the ~10-repeat stop UVUG-9 exists to have removed). A report with
  an **empty held set** is now the end of the hold and expires the verdict and its evidence. The P51
  protection is not weakened: a genuinely streaming keyboard re-earns the latch within
  `IDLE_RUN_TO_LATCH` report periods — tens of ms at any real polling interval, i.e. well before
  `DELAY_MS` has elapsed and the first repeat is even due.

  **Witness (`[keystat]`).** A re-arm names itself for the first `REARM_LOG_MAX = 3` per hold
  (`[keystat] typematic re-arm — key=0x67 still held after a liveness lapse; repeat resumed at
  delay=400ms (hold re-arms=1 boot re-arms=1)`), and every hold that produced repeats closes with one
  rollup (`[keystat] typematic hold end — key=0x78 repeats=1 re-arms=0 window=30000ms (boot: repeats=1
  re-arms=0)`), carrying the window that was in force **during** that hold — which is why the rollup is
  emitted before the verdict is cleared, in both the release path and the detach path. Output is bounded
  by human key holds. `[uvug9] typematic hold-max`'s tail text now says the next still-held report
  re-arms, instead of "re-press resumes".

  **Arch-neutrality — and a correction for the GR7 relay.** The whole tracker
  (`mod typematic`, `typematic_note_report`, `typematic_tick`, every test aid) is gated
  `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]`, and its sole producer is the Pi's xHCI
  boot-keyboard decode. **x86 does not inherit this fix, because x86 has no host-side typematic engine
  at all** — the rMBP bench gets key repeat from the hardware/driver path, not from this code. Nothing
  in this arc touches a shared file: `pal.rs`'s edits are inside the aarch64+baremetal cfg, and
  `main.rs`'s are inside the equally-gated `typematic_selftest`.

  **Gate.** `./arroyo check`: x86_64 OK, aarch64 OK. `./arroyo kernel8-test 210`: **MBENCH PASS —
  86/86** required witnesses, 0 forbidden hits, 25295 lines scanned. `pi4-regression.spec` is
  **unchanged**: the new legs ride the existing `:: uvug6: typematic … :: PASS ::` verdict, whose FAIL
  half is already covered by mbench's default `FAIL ::` forbid. That selftest gains three legs —
  **(G)** a lapse then a still-held report must re-arm *and* repeat again, with no release and no
  re-press anywhere in the sequence; **(H)** a lapse then a *release* must not re-arm and must produce
  no ghost repeat across 64 forced-due ticks (the leg that proves the re-arm did not reopen P51);
  **(I)** a legitimately latched verdict must be **gone** once the hold ends. The lapse is driven
  through the same `typematic_lapse_disarm` seam the production path uses rather than by the clock, so
  no test hook enters the window comparison and the selftest does not stall for a real `LIVENESS_MS`.

- **VUGMIN-A (a vug nobody can see stops burning cores too)** — Peter's ruling at **P69**: *"if vug is
  minimized it should shut off."* The audit that opened the arc found there is **no minimize feature in
  UnaOS at all** — no `minimized` field on the window table, no minimize verb on `wm`, no chrome
  hit-testing; `theme::CONTROL_MID` is documented as the "minimise" control fill and has no consumer
  outside `theme.rs`'s own self-tests. What *does* exist is the state Peter was describing:
  `wm::focus_changed`'s **shell arm** (the operator TABs to the console) pushes every window below
  `SHELL_Z` and erases its box. The vug is gone from the panel and its frame loop runs at full rate
  against a surface the compositor will not read — VUGPAUSE's disease with a different trigger. So the
  honest name for the state is **HIDDEN**, no new verb is needed, and the cure is VUGPAUSE's idle loop
  rather than a second mechanism.
  - **Kernel side.** `HIDDEN_ASIDS: AtomicU64` in `arch/aarch64/syscall.rs`, a deliberate mirror of
    VUG-BG's `DETACHED_ASIDS` down to the 64-bit bound and the fail-safe direction, published as
    **bit 1** of the info-page process-flags word (see the layout table above). Public
    `set_hidden(asid, on)` is the seam for `video::wm`; `clear_hidden(asid)` is called from
    `boot::teardown_user_slot`'s final-release arm beside `clear_detached`, under the same ordering rule.
    That clear matters *more* than bit 0's: ASIDs are recycled, and a stale hidden bit would give the next
    tenant a vug that comes up **already idling** — a window that never draws — having never been hidden.
  - **The setter is the publisher.** Unlike bit 0, bit 1 changes under a running process, and the
    info-page writers run only on create/map. Without republishing from `set_hidden` the bit would be an
    unobservable atomic and the whole mechanism inert.
  - **EL0 side.** `user-vug` keeps the info-page pointer instead of discarding it after the `DETACHED`
    read, and polls bit 1 every frame (one `read_volatile` of a mapped word — cheaper than the branch
    that consumes it). `hidden` folds with `paused` into `frozen`, which replaces `paused` in **two**
    places, both load-bearing: the **orientation fold** and the **skip predicate's first conjunct**. The
    fold is the one that is easy to miss — on the auto path the else-arm advances the idle tumble every
    frame, so without it `ay != last_ay` would hold forever, the predicate could never once be true, and
    the vug would read its own hidden bit correctly and burn the core anyway. The other four conjuncts are
    untouched, so a vug hidden before its first present still renders that frame, and the frame that
    restores it to the panel is an ordinary rendered frame from the preserved state — **restore is a
    resume, not a jump**.
  - **One difference from pause, and it is the ruling itself:** the once-per-second fps refresh does
    **not** run while hidden. A paused vug is on the panel and its readout is owed to the operator; a
    hidden vug's refresh is discarded by the compositor, so drawing it is the very waste this arc ends,
    merely at one hertz instead of sixty.
  - **`SYS_WIN_PRESENT` is not cheap while hidden**, contrary to the assumption the arc started from:
    `sys_win_present` → `wm_bridge::present` → `wm::present` ends in an unconditional `composite()`, with
    no visibility predicate anywhere on the path. Suppressing it kernel-side is a `wm.rs` change and is
    left to VUGMIN-B; the EL0 idle loop is what stops the presents from being issued in the first place.
  - **Witness:** one-shot `[vugmin] idle engaged frame=<n>`, mirroring `[vugpause]` and on its **own**
    latch — the two idle reasons are different system facts and a shared latch would let whichever
    happened first silence the other forever.

  **DORMANT AS SHIPPED, and deliberately so.** VUGMIN-A is plumbing only: **nothing calls `set_hidden`
  yet**, so bit 1 reads a constant `0` and no EL0 branch above it is reachable. The two call sites belong
  in `video::wm` (the shell arm that hides, the raise path that unhides), and `wm.rs` was another
  session's lane during this arc — hence the split. **VUGMIN-B** is that wiring, plus the `wm.rs` present
  suppression, and until it lands this arc changes no observable behaviour on hardware. The deterministic
  QEMU auto path is untouched now and stays untouched after B, for a stronger reason than pause's: a
  headless run has no HID, so nothing ever TABs to the shell, so nothing is ever hidden — the 300-frame
  `checksum=0xe68285b85121ac7c` witness proves it on both sides of the change.

- **VUGMIN-B (the bit gets a writer, and the compositor stops doing invisible work)** — the second half
  of VUGMIN. A's bit was set by nobody; B is the two call sites in `video::wm` and the kernel-side
  present suppression A's audit measured. With it the mechanism is **live**: TAB to the shell and every
  vug on the panel goes to its VUGPAUSE idle loop; TAB back and it resumes from preserved state.
  - **The seam is a DIRECT CALL, not a registered callback.** `wm.rs` already reaches
    `crate::arch::aarch64::now_cycles()` from the composite path under a plain `cfg`, so a callback table
    would be a second, weaker convention for the same thing — and a seam whose entire content is "one
    `u64`, one `bool`, no return value" earns no indirection. `wm::vugmin_publish` wraps
    `set_hidden` under `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]`, which is exactly
    the gate `arch::aarch64::syscall` itself carries; on x86_64 and on the hosted aarch64 build it
    compiles to nothing.
  - **One predicate, both arms.** `focus_changed` does not reason about "which arm am I in". After the
    z-order has settled — still under `TABLE` — `vugmin_scan` walks the eight rows once and returns
    `(owner asid, hidden?)` for every live owner; the guard drops, and each pair is published. The shell
    arm therefore hides every owner because every owner *is* below the new `SHELL_Z`, and a raise unhides
    the owner it raised because that owner is now above it. The published value is a function of the
    table's actual state rather than of the code path that reached it.
    **Superseded by VUGMIN-C for the raise arm** — the paragraph above is why it was written that way and
    why exactly half of it was wrong; see below.
  - **The quantifier is per-owner and all-or-nothing.** `owner_hidden(asid)` is true only when EVERY
    live, non-compat window that ASID owns sits below `SHELL_Z` — an app with one window still on the
    panel is not hidden however many of its others are buried, which matters because the focus ring is
    keyed by ASID and raises an app's windows together. Single-window vugs are the case that actually
    occurs; the quantifier is what stops a two-window app being told to idle while half of it is visible.
    An owner with no live window answers `false` (the empty case falls to the safe side, keep rendering),
    and ASID 0 — the compat/console path — is never marked, for `above_shell`'s reason: a compat row is
    not a focus target and can never be raised back over a shell that overtook it.
  - **Present suppression.** `wm::present` still marks the row `damaged` and `presented` exactly as
    before, still takes WC-G's owner-declared checksum (that number is about the surface, not about the
    panel, and dropping it while hidden would make the `app` leg disagree with itself) — and then
    **returns without calling `composite()`** when the presenting owner is hidden. The pass would have
    read the surface, clipped it, and written it nowhere: `composite` draws only rows satisfying
    `above_shell`, and by hypothesis none of this owner's do. Nothing is deferred; a pass is skipped, not
    owed. On unhide the panel is repainted from the LATEST surface content by `focus_changed`'s own
    unconditional `composite()` (the call after the `<F6>` wedge mark), which the raise arm has already
    re-marked the raised rows damaged for. The EL0 idle loop stops most presents from being issued at
    all; this makes the ones that still arrive — a vug hidden mid-frame, a program that never polls the
    bit — cost one table scan instead of a full compositor pass.
  - **No new lock, and nothing called out from under `TABLE`.** `set_hidden` takes no lock at all: an
    `AtomicU64` RMW plus one `write_volatile` of a `u32` into the slot's info page, whose address is pure
    pointer arithmetic (`slot_fb_info_ptr` is base + offset and a `debug_assert!`). It would be safe to
    call with the table held; it is called after the guard drops anyway, because "snapshot under the
    lock, act after it" is the shape every other outward call in `wm.rs` has, and keeping that a rule
    beats auditing it case by case. The publish is unconditional — `set_hidden` is idempotent, and
    skipping it on the strength of `wm`'s witness-side shadow would let one stale bit (ASID recycle
    clears the real one through `boot::teardown_user_slot`, behind `wm`'s back) leave a fresh tenant
    permanently idling.
  - **Witness:** `[vugmin] wm scope=<s> hides=<n> unhides=<n> presents_skipped=<n> hidden_now=<n> -> …`,
    printed from the tail of `wci_rollup_scoped` beside the other compositor rollups, because
    `presents_skipped` is a subtraction from the `cursor_passes` on the line above it. `hides`/`unhides`
    count STATE TRANSITIONS, not focus changes, off a `wm`-local shadow mask whose only job that is.
    The verdict is `DORMANT` when `hides == 0` rather than `CLEAN`: the headless gate has no HID, so
    nothing ever TABs to the shell, so all three counters are 0 there and the line's honest claim is
    "wired, and nothing hid by accident" — the number that carries the arc is `hides > 0` on the bench.
    The 300-frame `checksum=0xe68285b85121ac7c` is byte-identical across B, as predicted.

- **VUGMIN-C (a raise publishes a transition, not a census)** — B's "one predicate, both arms" is right
  about the shell arm and wrong about a raise, and P73 on the bench is what the difference costs. The
  shell arm really is whole-table: `focus_changed(0)` puts `SHELL_Z` above everything, so every owner's
  answer changes in one step. A RAISE changes exactly one owner's z — but `vugmin_scan` re-published
  `hidden=false` for **every** owner still above `SHELL_Z`, which after any prior raise is the entire
  stack. So a single TAB or click into one vug un-minimized all of them. On the P73 wire one
  `[clickroute] press hit asid=4` is followed by `[vugpause2] resume … edge=unhide` for two OTHER
  ASIDs (p73 L6523-6525), and a six-vug fleet stayed lit on all four cores for ~500 s with no operator
  input and no decay (`c1=c2=c3=99%`, ~795k ctx/window). The defect shipped with B and was INERT until
  VUGPAUSE-2r2 made the unhide edge able to reach a parked task.
  - **The raise arm now publishes at most two rows.** The arriving owner unhides; the owner that just
    lost focus hides; every other owner's bit is untouched. `vugmin_scan` is kept, unchanged, for the
    shell arm alone.
  - **The departing owner is NOT moved below `SHELL_Z`.** Its z and its pixels stay exactly where they
    are — the fleet is tiled at non-overlapping positions and is genuinely on-screen, and this arc does
    not pretend otherwise. What changes is only what the owner is TOLD: it stops rendering and parks, so
    it freezes in place, visible and static, until it is clicked or TABbed back. That is the bench
    semantic of record — only the focused vug runs — and it makes P69's "if a vug is minimized it should
    shut off" apply to the raise path as well as the hide path.
  - **The hide is gated on `owner_live`.** An ASID with no live, non-compat window is never marked
    hidden on the way out: a vug that exited while focused has already had its bit cleared by
    `boot::teardown_user_slot`, and re-setting it would leave a set bit on a free slot whose next tenant
    would come up permanently idle with nothing left to unhide it.
  - **The VUGPAUSE-2r2 wake edges are intact.** The unhide for the newly focused owner is exactly the
    edge that restarts a parked vug, and it still fires on every raise; only the unhides for owners
    nobody focused are gone.
  - **Witness:** `[vugmin] focus asid=<a> unhid=1 hid=asid=<b> others=untouched` (or `hid=none` when
    focus arrived from the shell or did not move), one line per raise beside `[wc-fv] focus raise`. The
    old behaviour was invisible from `wm`'s side — its only trace was N `edge=unhide` resumes in the
    syscall layer for ASIDs nobody had focused — so the scope is now asserted where it is decided.
  - **Wire hygiene, same arc.** `[vugpause2] resume` fired 6-12 times per focus change from inside the
    input path, on several cores, and the P73 capture shows the UART overrun mid-line in exactly those
    bursts (`resu<e7>sid=1`, `[f8>fv]`, `<fu>pause2]`). The print is now paced per ASID on powers of two
    (1st, 2nd, 4th, 8th, …) and carries `n=<cumulative resumes for this ASID>` so elisions are
    countable. **Counters are unchanged** — `USER_INPUT_WAKES`, and therefore the `[vugpause2]
    blocked=/wakes=` rollup, still counts every resume. The pacing counter is per-ASID (a global cadence
    would let a busy vug silence a newly launched one's first resume) and is reset only by slot teardown.
  - **Headless gate:** unchanged. `[wc-fv] focus-vis`'s four legs are framebuffer read-backs of z-order
    and this arc moves no window; its synthetic ASIDs (`0xF0A`/`0xF0B`) are outside the 64-wide hidden
    mask, so `vugmin_publish` no-ops for them as it always did, and `[vugmin] wm … -> DORMANT` still
    holds with no HID to TAB with.

- **VUGPAUSE-2 (an idle vug leaves the run queues)** — VUGPAUSE and VUGMIN stopped an idle vug from
  RENDERING; they did not stop it from RUNNING. What they left is a **yield-polling floor**: the idle loop
  is `drain_input` + `SYS_YIELD`, which is runnable by construction, so an idled vug is still in a run
  queue on every dispatch pass. P69 on silicon measured the floor at **47/58/57/55 %** per core for a
  six-vug fleet with nothing moving on the panel — down from 99/80/85/99, and still most of the machine.
  This arc replaces both halves of that spin with real blocking waits.
  - **`SYS_INPUT_WAIT` = 28.** The id ELF-5 reserved and deferred, now spent. `SYS_INPUT_WAIT() -> 0 /
    -EINVAL` blocks until the caller's input ring may be non-empty. It **does not dequeue**: the caller's
    ordinary `SYS_INPUT_POLL` drain runs next and sees every event, so the two compose with no "the wait
    ate my event" hazard and the vug's loop needed no restructuring — one call site swaps `SYS_YIELD` for
    it on the idle path, and the rendering path is untouched.
  - **Built on the existing futex, not a new primitive.** `sched::futex_wait/_wake` keyed by
    `input_futex_key(asid)` = `(1 << 63) | asid`. A `sys_futex` key is the PHYSICAL address of an EL0
    word and a PA is < 2^40 here, so bit 63 puts every input key in a space no user word can name — an
    EL0 program cannot forge one and reach another process's ring. The compare word is the ring's own
    `TAIL`, read at EL1 under the bucket lock that `user_input_push`'s wake must take, which is what makes
    the wait race-free against the router.
  - **Three wake edges, each load-bearing.** `user_input_push` (the event itself — this is the
    latency-critical one, and it makes the vug re-ready in the same pass the router runs, sooner than the
    yield loop could act); `user_input_set_active` (a focus ARRIVAL, which RESETS the ring rather than
    filling it, so nothing else would move `TAIL`); and `set_hidden(asid, false)` (an UNHIDE — the hidden
    bit lives in the info page, so becoming visible touches no ring at all).
  - **The workers had to go too.** A vug is one parent and TWO workers, and the workers yield-polled
    `PHASE` for the release direction — two thirds of the residue by headcount. They now **spin then
    park**: `WORKER_SPIN_YIELDS` (4096 as landed here; **cut to 64 by VUGSPIN below**, whose Boot A
    arithmetic falsified this constant's sizing premise — the spin-then-park SHAPE described in this bullet
    is unchanged) passes of the original poll, then `futex_wait` on `PHASE`, with
    every parent-side `PHASE` store followed by a wake. The spin is not conservatism. The first cut made
    this a bare park on the symmetry argument that the ARRIVAL direction has always been a real futex on
    the same QEMU; `kernel8-test` then FAILED with `:: EXEC-UVUG: … did not exit in time ::` and all
    three tasks parked at the kill. The M6e note's claim that the release direction is the fragile one
    under raspi4b's missing Group-1 timer is therefore **measured, not folklore** — the rendering path
    keeps the poll, and only a parent that has stopped releasing frames drains the spin.
  - **`NFUTEX` 16 → 64.** A vug used to hold ONE live key (`DONE`); it now holds three while idle
    (`DONE`, `PHASE`, its input ring) — 18 for a six-vug fleet, over the old pool. The overflow does not
    fail loudly: `TableFull` makes every caller degrade to a spin, so the arc's whole benefit would have
    evaporated silently on exactly the workload it was built for.
  - **Liveness: the backstop.** A parked task issues no `SYS_INPUT_POLL`, and two bounds are fed by
    exactly that syscall — `gui_watchdog`'s 5 s wedge timer and UVUG-8r2's 2 s takeover heartbeat. Rather
    than teach either a new state, `sched::run` wakes parked input waiters every `INPUT_WAIT_BACKSTOP_TICKS`
    (64 ticks ≈ 256 ms, one CAS-claimed pass machine-wide), so the vug still polls — a few times a second
    instead of thousands. Metal-only by construction: `timer::ticks()` needs the timer IRQ QEMU raspi4b
    never delivers, which costs nothing there because a headless run has no HID and nothing ever freezes.
  - **Kill/reap are unchanged, and that is KILLBOUND's doing.** VUGPAUSE's note that "an unbounded
    `futex_wait` is an unkillable empty window" was true when it was written and is not now: `futex_wait`
    tests the armed-kill flag before parking and `futex_wake_killed` evicts already-parked targets, so a
    vug blocked in `SYS_INPUT_WAIT` is exactly as killable as one blocked in the frame barrier.
  - **Witness:** `[vugpause2] blocked=<n> wakes=<n> asid=<a>`, emitted on a power-of-two cadence — a
    per-park line would flood and a one-shot line would not show the mechanism still working, while a
    logarithmic one is bounded at ~40 lines for any boot. `wakes` beside `blocked` is the pair that
    matters: `blocked` climbing while `wakes` stalls is a vug going to sleep and not being woken.
  - **Size.** `VUG.ELF` links at 12568 B with `.text` at exactly **0x2000** — the hard ceiling, since one
    byte more pushes `.bss` to the next page and the image to 16664 B, which `arroyo` rejects against
    `USER_REGION_SIZE`. The next arc to touch `user-vug` has no headroom; the file's own size note records
    which inlining choices measured smaller and why.
  - **Gates:** `./arroyo check` green both arches; `./arroyo kernel8-test` **MBENCH PASS 86/86, 0
    forbidden**, `UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c` byte-identical — the headless
    path has no HID, so it never freezes, never parks, and proves the rendering path is untouched.

- **VUGPAUSE-2r2 (a backgrounded vug can be restarted)** — P72 on silicon: *"once a vug goes into the
  background it is stopped and cannot be restarted. If it's already stopped it can't be restarted."*
  Clicking the window, focusing it, un-minimising it — nothing brought it back. The wire said the router
  was doing its job (`[clickroute] press hit asid=3 win=3 (was 2)` on the stopped vugs) and that parking
  and most waking worked (`blocked=` and `wakes=` tracking closely), which is what made it look like a
  routing or compositing fault rather than the one-store bug it was.
  - **Root cause: a stale-CLEAR of the park hint, by the focus arrival itself.** `USER_INPUT_PARKED[asid]`
    is the "someone is parked on this ring" flag, and `user_input_wake` returns early when it is clear — so
    it gates **every** wake edge, not merely the backstop that VUGPAUSE-2 described as its only consumer.
    `clear_input_row` cleared it, and `user_input_set_active` calls `clear_input_row` on a focus **arrival**
    — precisely the moment a parked, backgrounded vug is being handed the keyboard. The arrival therefore
    wiped the flag microseconds before its own wake read it; the wake took the fast path and released
    nobody, and so did `focus_changed`'s unhide and the press enqueue that followed. The backstop, gated on
    the same flag, skipped the slot for the rest of the boot. The vug sat on a live futex key nothing in
    the kernel would ever name again: permanently stopped, by construction, on its first trip to the
    background.
  - **The fix, in three parts.** (1) `clear_input_row` no longer touches the hint; the single site that
    legitimately clears it — slot teardown, where the parker `exit()`s out of `futex_wait`'s pre-park kill
    boundary and never reaches its own clear — calls the new `clear_input_parked` explicitly. The
    invariant is now stated where the flag lives: **set by the parker before parking, cleared by the
    parker on return or by teardown, and by nothing else.** (2) `user_input_wake_backstop` no longer reads
    the hint at all — it wakes all eight slot keys unconditionally. A backstop that can be disabled by the
    bug it exists to survive is not a backstop; unconditional costs ~31 `futex_wake` calls a second on
    keys that are almost always empty, and it caps this entire failure class at one ~256 ms period instead
    of forever. (3) The two operator-rate edges name themselves on the wire.
  - **Why both the focus and the unhide edge are needed.** `wc_click_route` calls `user_input_set_active`
    *then* `wm::focus_changed`, so the focus wake fires while the vug's hidden bit may still be set: the
    woken vug re-reads its flags word, finds itself frozen, and re-parks. The unhide wake is the one that
    fires after the flags word says visible, and the press enqueue behind it moves `TAIL`, so a vug
    parking in that window loses the compare rather than the wakeup. Three edges, in that order, and the
    restore is race-free across all of them.
  - **Witness:** `[vugpause2] resume asid=<a> edge=focus|unhide woken=<n>`, emitted only when a wake
    actually releases someone. The router's per-event push edge stays silent (mouse-motion rate). This is
    the line that separates the two outcomes on wire: a **stranded** vug shows `[clickroute] press hit`
    with no `resume` after it; a **restored** one shows the pair.
  - **Not changed here, flagged instead — and since FIXED by CLICK-SWALLOW (above):** the press that
    raises a window was also delivered to it (CLICK-ROUTE's documented design), so the click that
    restores a vug was *also* a `VUGCLICK` pause toggle — the restored vug came back paused and needed a
    second click to resume rotating. That was a UX question about click-to-focus semantics rather than
    the strand, so it was left to the CLICK-ROUTE lane; P73 is the bench report that carried it, and the
    hit arm now swallows a focus-changing press. This bullet's three wake edges are unaffected: the
    swallow sits strictly downstream of both of the ones a restore crosses.
  - **Gates:** `./arroyo check` green both arches; `./arroyo kernel8-test` **MBENCH PASS 86/86, 0
    forbidden**, `UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c` unchanged.

- **VUG-PACE (a vug runs at the machine's speed, not the scheduler's)** — an **app-only** arc:
  `crates/user-vug` + this doc, no kernel change. P73 on silicon: *"there's a delay to a vug speeding up
  when it's the only one running"*, and *"vug still wants to go back to what it thinks its fps is supposed
  to be even though it could run faster."*
  - **There was never an fps target to go back to.** The audit that opened the arc looked for the ceiling
    the symptom implies and found none, in either layer: no sleep, no frame budget, no target-rate
    throttle in `user-vug`'s loop; and no per-process pacing under `SYS_WIN_PRESENT` — `present_surface_common`
    checksums, counts and composites, and returns. The plateau was real; the target was not.
  - **Root cause: the frame barrier parked on its FIRST pass.** The parent stored the release and went
    straight into `futex_wait` on `DONE`. A park costs a wake plus a **dispatch**, and dispatch latency is
    a property of the run queue, not of how much CPU is spare — under raspi4b's missing Group-1 timer IRQ
    the woken parent runs when whatever holds its core yields. Two workers arrive per frame, so a healthy
    frame paid **two** of those round trips, and the frame time was floored by them. A floor made of
    dispatch latency does not fall when the machine empties out, and a stable floor quantises the VUGFPS
    readout to a stable-looking number — which is exactly the "fps it thinks it's supposed to be".
  - **And it latched.** `WORKER_SPIN_YIELDS` is what keeps a worker off the park path. Frames slow enough
    to outlast that spin add a park/wake to each worker's release too, which lengthens the frame, which
    keeps the workers parked. Contention could push a vug into that state and the state's own cost held it
    there after the contention left — the "delay to speeding up", with no memory of the contention anywhere
    in the program.
  - **The fix: the barrier waits the way the workers already wait.** `BARRIER_SPIN_YIELDS` (64) passes of
    `SYS_YIELD` polling `DONE`, then `futex_wait` — VUGPAUSE-2's spin-then-park, applied to the direction
    it was not applied to. This is adaptive with **no estimate and no window**, which is why the speed-up
    is immediate rather than earned back over an interval: cores free ⇒ `SYS_YIELD` finds nothing else to
    run and returns at once, so the parent sees the arrival the instant the worker stores it (no wake, no
    dispatch, no floor); contended ⇒ `SYS_YIELD` is a real handoff on every pass, so the spin cannot
    starve a sibling and the wait degrades to cooperative rather than to a hog; genuinely long wait ⇒ the
    spin is bounded and the park is still underneath it. The budget is re-armed **every frame**, so no
    frame's contention is carried into the next.
  - **Sized as a latency threshold, not a rate.** 64 against the worker's 4096, because this budget is
    spent every frame on a healthy run rather than once per idle interval: enough passes that an arrival
    which is merely a raster away is never parked for, few enough that a genuinely contended wait reaches
    the park after a bounded handful of syscalls. `passes` still counts only **parked** passes, so
    `BARRIER_PASS_BUDGET` and the `phase=barrier` retirement deadline keep their old meaning.
  - **The idle path keeps its design, and gains the parking it was missing (P73 mouse-preempt triage,
    Fix C).** VUGPAUSE's skip predicate required the frame's render state to ALREADY equal the last
    presented state — `ay == last_ay && ax == last_ax && dist == last_dist` — before it would idle. That
    was an equality test standing in for an invariant that holds by construction: while `frozen` the
    orientation fold runs its empty arm, so `ay`/`ax`/`dist` are assigned nowhere and **cannot** change.
    The comparison could therefore only ever fail on the TRANSITION frame — and there it failed **closed**,
    refusing to park a vug frozen mid-motion and leaving it running the full frame loop indefinitely. The
    wire datum is `[vugpause2] blocked=` pinned at **8192 across ~500 s of saturation**: under load the
    fleet stopped parking at all. The three conjuncts are gone (with `last_ay`/`last_ax`/`last_dist`, which
    nothing else read), so the FIRST frozen frame parks, unconditionally. `presented` and the overlay
    conjunct still gate. The whole cost is that a vug frozen mid-motion holds the previous frame's surface
    rather than the one it froze on — one tumble step, 3 brads, on a crystal that has just stopped moving.
  - **Otherwise the idle contract is untouched:** the `SYS_INPUT_WAIT` park, the hidden-vs-paused split on
    the fps refresh, and the workers' own spin-then-park all stand as VUGPAUSE-2 designed them. The arc
    removes a floor from the **rendering** path and a false guard from the **entry** to the idle one.
  - **Size — the constraint that shaped the patch.** VUGPAUSE-2 landed `.text` at exactly `0x2000` with
    zero headroom, so both changes had to be **bought**, not added: the naive barrier alone built a
    16664 B image, over `USER_REGION_SIZE`. Four economies, none of them behavioural — fold the spin
    counter into the existing `passes` counter (8268 → 8220); collapse `draw_fps`'s digit-count/divisor
    derivation from a `while k < n { div *= 10 }` loop into one ladder (8220 → 8188); evaluate
    `detached || interactive` once per frame instead of at four sites (8196 → suppressed the Fix C
    regrowth); and PRE-JOIN `stall_witness`'s phase name and detail label into one literal at each of the
    three call sites, which deletes three `put` calls and two arguments (→ **8022 B**). `VUG.ELF` links at
    **12568 B**, the same size as the pre-arc image, and the next arc inherits ~170 bytes of headroom
    rather than none.
  - **Gates:** `./arroyo check` green both arches; `./arroyo kernel8-test` **MBENCH PASS 86/86, 0
    forbidden, 4758 lines scanned**, `UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c` unchanged —
    the barrier change alters only WHEN the parent observes an arrival, never any value that reaches the
    surface, so the deterministic 300-frame witness is untouched by construction and in fact.

- **CLICK-ONE (one visible rule for stop and start)** — an **app-only** arc: `crates/user-vug` + this
  doc, no kernel change. **P74, blocked at the bench:** *"click stop/start is all messed up cannot
  continue test."*

  **THE SINGLE STOP MODEL — read this paragraph and nothing else is needed.** *A click on a vug is
  **focus/restore only**, always, and is never app input that changes run state. **The focused vug runs;
  unfocused vugs freeze in place.** **SPACE** on the focused vug toggles pause/resume. A vug with an
  empty input ring **parks** (VUGPAUSE-2) and is woken by the next event addressed to it.* Focus decides
  running; SPACE pauses; ring-empty parks — three mechanisms, one per question, none of them reachable by
  the same gesture. Everything below is why, and what it supersedes.

  **The defect was accumulation, not a bug.** Nothing was wrong in isolation; three independently correct
  stop states had piled up and a click could land in any of them — frozen-by-unfocus (**VUGMIN-C**),
  paused-by-a-delivered-click (**VUGCLICK**), parked-on-an-empty-ring (**VUGPAUSE-2**). **CLICK-SWALLOW**
  then made the first click on an unfocused vug focus-only, so the *second* click toggled pause. The
  visible result of a click had become a function of invisible state: which window held focus, and
  whether this was the first click or the second. That is not a model an operator can hold, and P74 is
  the proof — the bench could not proceed, with every individual mechanism behaving as designed.

  **Change (app side, `crates/user-vug/src/main.rs`).** The pause toggle leaves the pointer and moves to
  the keyboard, where it can only reach the **focused** vug by construction:
    - **Click/button edges carry no run-state meaning at all.** A press starts a drag, a release ends
      one, and that is the whole of it. `CLICK_THRESH` and the drag-motion accumulator that fed it are
      **gone** — with no click/drag discrimination left to make, a click is simply a drag of zero pixels:
      it rotates nothing and changes nothing.
    - **SPACE toggles pause** (`K_SPACE`, ASCII 0x20). Chosen because nothing bound it: `key_bit` maps
      WASD/arrows, Q/E and the `+`/`-` family and nothing else, and ESC is handled ahead of it, so no
      existing gesture changed meaning. The witness is `:: UVUG: pause=<0|1> ::`, one line per **state
      change** (superseding `:: UVUG: click pause=<0|1> ::`), still human-rate and still doubling as
      proof that the input reached EL0.
    - **Typematic repeats cannot flutter it.** `pal.rs` synthesises a KeyDown every `RATE_MS` (40 ms)
      once a key has been held `DELAY_MS` (400 ms), so a toggle driven off raw KeyDowns would flip 25
      times a second under a resting thumb. SPACE therefore rides the existing `held` word as `H_PAUSE`
      and toggles only on a bit that was **not already set** — a true press edge. `H_PAUSE` is
      deliberately **outside** the new `H_MOTION` mask that `manual` tests, so a held SPACE does not read
      as manual control and silently stop an unpaused vug's tumble.
    - **Drag-rotate, interactive takeover, and ESC are untouched.** Takeover is keyboard-armed and
      unaffected; a drag is motion, not a click, and was never a run-state gesture.

  **The router needed no change and got none.** `wc_click_route`'s already-focused arm still **delivers**
  the press, which stays correct — apps may legitimately want clicks, and the vug now simply ignores them
  for run-state purposes. The CLICK-SWALLOW arm (focus-changing press consumed) and the CLICK-SHELL miss
  arm (desktop press focuses the shell) are the *focus* half of the same one rule. Two stale readings in
  the bullets above are superseded by this entry rather than rewritten: CLICK-ROUTE's full-screen
  exemption is no longer motivated by "UVUG's own click-to-exit" (that exit died at VUGCLICK; the
  exemption stands on `compat_live` per CLICK-SHELL r2), and CLICK-SWALLOW's "for a vug, a delivered
  click **is** the pause toggle" is now false by design — which removes the *second* meaning the swallow
  was protecting the operator from, and leaves the swallow correct for the general reason it also gave
  ("an app never sees a click it did not own the focus for").

  **Size.** Removing the click-toggle **bought** bytes rather than spending them: `.text` **8022 →
  8012 B** against the hard `0x2000` cliff, and `VUG.ELF` links at **12568 B**, byte-for-byte the same
  size as before.

  **The deterministic auto path cannot reach any of this by construction** — QEMU raspi4b delivers no
  HID, so `interactive` never arms, so `paused` is never written and no click or SPACE is ever seen.

- **CLICK-PLAIN (a click goes to the window under the cursor, and is acknowledged there)** — router +
  window manager + app: `arch/aarch64/syscall.rs` (`wc_click_route`), `video/wm.rs` (`focus_changed`
  raise arm, hit-test selftest leg 8), `crates/user-vug/src/main.rs`, and this doc. **P75, on metal:**
  *"stop works like absolute garbage there is no reason to it."*

  **THE MODEL — read this paragraph and nothing else is needed.** *A press goes to the **window under
  the cursor**; if that window was not focused, the focus moves there **first** and the press is then
  delivered to it, whole (press and release together). **A focus change never stops anything** — it
  starts the window it names and leaves every other window exactly as the operator last saw it. Idling
  the whole fleet is still one deliberate gesture: focus the **shell** (click the desktop, or TAB to
  it), whose arm hides every owner at once. What the app does with a delivered click is the app's
  decision.*

  **The defect, and why it was not in any one place.** CLICK-ONE removed the click's run-state meaning
  in the app, which was right, but two kernel-side rules from the previous arcs survived it and combined
  into something no operator could model:
    - **CLICK-SWALLOW** consumed the focus-changing press instead of delivering it. Correct while a
      delivered click *was* a pause toggle; after CLICK-ONE it only meant the first click on a window
      produced no observable effect at all.
    - **VUGMIN-C's departing-owner hide** stopped the vug that was *losing* focus. So clicking vug B
      appeared to stop vug A — a different window from the one clicked, one click after the fact, with
      no gesture on screen that explained it.
  Together: a click did nothing visible to what you clicked and stopped something you did not click.
  That is P75 exactly.

  **Change 1 — the router delivers the focus-changing press (`wc_click_route`, hit arm).** The arm keeps
  its order and loses its swallow: `user_input_set_active(owner)` then `wm::focus_changed(owner)` (both
  wake edges — focus arrival and unhide — fire *first*, so a parked, hidden vug is re-readied before
  anything is pushed), then it records the **raised owner** in `CLICK_PRESS_TARGET` and returns `false`.
  Both callers' normal paths now address the new focus, so the press lands in the raised window's ring
  and the matching release follows it there. Wire: `[clickroute] press hit asid=<a> win=<w> (was <c>)
  **delivered**` (was `swallowed`). The **miss** arm is unchanged — a desktop press still focuses the
  shell and is still consumed (CLICK-SHELL / r2).

  **Change 2 — a raise publishes an arrival only (`wm::focus_changed`, raise arm).** The departing
  owner's `hidden=true` publication is **removed**; the arriving owner's `hidden=false` (the VUGPAUSE-2r2
  wake edge) stays. Every other owner's bit is untouched, exactly as VUGMIN-C intended for them. The
  shell arm is **unchanged** — `vugmin_scan` over the whole table, every owner hidden — and is now the
  only place a hidden bit is ever *set*, which is what keeps VUGMIN-A/B's shell-focus-idles-the-fleet
  semantic intact. `owner_live` went with the hide it guarded (its hazard — stranding a set bit on a
  freed slot — cannot arise when the arm only ever *clears*). Wire: `[vugmin] focus asid=<a> unhid=1
  **hid=none** others=untouched`, on every raise.

  **Change 3 — the app acknowledges the click, in two layers.** `crates/user-vug` regains its click/drag
  discrimination (a press+release whose travel stayed under `CLICK_THRESH` is a **click**; anything
  further is a drag and rotates as before), and then:
    - **LAYER 1 (unconditional).** A click advances a **click counter drawn in the window's top-left
      band**, in cyan immediately right of the amber fps digits, and prints `:: UVUG: click n=<N> ::`.
      **No run-state coupling whatsoever** — SPACE remains the only stop/start control in this layer.
      This is the unmissable proof that routing worked: the number under the cursor is the number that
      moves.
    - **LAYER 2 (`LAYER 2 (CLICK-RUN)`, one fenced hunk in the frame loop).** A click **also** toggles
      run state, defined **absolutely** rather than as the inversion of an invisible flag:
      `paused = !(paused || hidden)` — *not running (paused OR hidden) → **runs**; running → **stops***.
      Deleting the fenced lines leaves Layer 1 exactly; nothing outside the block refers to it.

  **Witness (leg 8 of the hit-test selftest, `wm.rs`).** The leg keeps its three fields and inverts its
  assertions with the rule: `hit=` (an unfocused hit moves focus **and** the raised owner's ring holds
  the whole pair — depth 1 after the press, 2 after the release), `deliver=` (the next press on that now
  focused window lands too — depth 3), `wake=` (≥ 2 named wake edges ran, which is what pins the wake
  chain *ahead of* the push). Line: `[clickroute] hit-test … hit=<…> deliver=<…> wake=<…> -> PASS`.

  **Size.** `.text` **8012 → 8057 B** with Layer 2 in (**8073 B** with the hunk deleted — the two-line
  hunk measures *negative*, because it shifts inlining around `paused`) against the hard `0x2000`
  cliff. The first draft measured 8477 — 285 over — and was bought back with three economies now recorded
  in the file's SIZE note: one `say(label, n)` helper for the four `:: UVUG: … ::` lines that share a
  shape; folding the `dragging` flag and the drag-motion accumulator into one `drag: u32` word (0 = no
  button down, else 1 + travel); and one `draw_hud` call in place of two `draw_num` calls at each of the
  two overlay sites.

  **The deterministic auto path cannot reach any of this** — QEMU raspi4b delivers no HID, so
  `interactive` never arms, no click is ever seen, and the 300-frame checksum is byte-identical.

- **VUG-PACE-2 (both residuals were outside the program)** — the s1q re-opens closed, with **zero
  user-vug code change** (a comment records both verdicts at the barrier note): the fixes live in
  `arch/aarch64/sched.rs` and are written up in `scheduler.md` (§ SPREAD-6, § FUTEX-DUP).
  - **(a) The residual "predestined fps" was the scheduler's placement latch.** VUG-PACE removed the
    barrier-park floor; the s1q wire then showed win1 pinned at 30.7–30.9/s for tens of seconds while
    win6 ran 88–93/s from the same binary — two runnable EL0 tasks time-sharing c2 at 99 % while three
    cores idled, `[spread4] rewake=` frozen. SPREAD-5 re-asked core placement only after a ≥ 100 ms
    park, which a frame-paced vug never takes, so contention-era packing was permanent. SPREAD-6 lets a
    micro-park wake re-ask every 250 ms (`refresh=` on the `[spread4]` rollup); moves stay margin- and
    freshness-gated.
  - **The eye was honest, and the tumble is the instrument that proves it.** The idle tumble is
    FRAME-based (3 brads per rendered frame — never time-based), so rotation speed *is* the frame rate
    made visible: a crystal that "returns to its old speed" is a true fps reversion, and the on-window
    VUGFPS digits and the rotation can never honestly disagree. The time-based-tumble hypothesis is
    refuted by construction; no discriminating instrument needed beyond what the window already draws.
  - **(b) The win1 lockup was the barrier's arrival park behind a kernel futex defect** (FUTEX-DUP):
    two waiters entering `futex_wait` together on a bucketless key could mint two buckets for one key,
    and `futex_wake` stopped at the first — and the only two-concurrent-waiter key in the system is
    this program's `PHASE` word (both workers, same instant, once per frame). One worker was stranded,
    `DONE` never reached `live`, and the parent parked at the barrier making no passes — which is why
    `BARRIER_PASS_BUDGET` (a *pass* counter) never fired and the wire showed `att=0`, no fault, no
    stall witness, no resume edge on the restoring click and no click ack. `kill` was the only
    recovery (`futex_wake_killed` already scanned all buckets). Fixed both-sided in the kernel (claim
    joins an existing same-key bucket; wake scans every bucket serving the key; `[futexdup]` witnesses
    an absorbed race). The barrier protocol here was always lost-wakeup-safe against a correct futex.
  - **Gates:** `./arroyo check` green both arches; `./arroyo kernel8-test` MBENCH PASS 86/86,
    `UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c` unchanged — VUG.ELF is byte-identical
    (comment-only edit), and the scheduler changes alter only when placement is re-asked and which
    bucket a wake visits, never any value that reaches a surface.

- **VUGSPIN (the worker spin was the frame rate, and the meter was never on the wire)** — an **app-only**
  arc on `crates/user-vug/src/main.rs`, convicted from Boot A
  (`~/unaos-bench/capture/gr25-bootA/ttyUSB0.log`) after Peter's metal report: *"the fps is total bs now
  and smp is still not fluid"*.

  - **The brief's premise did not survive the capture, and that is recorded first.** The arc was briefed
    on `[spread] pack=0 spare=7`, `steal=2/~1.5M` and *"exactly ONE runnable thread in rqp all boot"*.
    That is an EARLY-BOOT sample, before the vugs launch. At the times the vugs are actually running the
    same capture reads `[spread] pack=0 spare=2..5`, six or seven runnable threads, and
    `steal=74272/2675999`. There are also **nine to ten windows on the wire, not one**: `[wpace] win=1`
    and `win=2` sit at 19.6/s and 17.4/s while `win=0` and `win=3..8` run 46–63/s from the same binary.
    Any reading of this arc that starts from "one thread, idle machine" is reading the wrong five seconds.

  - **⚠ THE CONVICTION WAS WITHDRAWN UNDER REVIEW. Recorded, not deleted.** This arc first convicted the
    yield-spin worker barrier of causing the 19.6/s frame rate, citing the table row below. **The capture
    refutes it, and the refuting number is in the same log.** The frame period is **LOAD-INVARIANT**:

    | sample | windows | machine | `[wcn] win=3` |
    | --- | --- | --- | --- |
    | 43 916 ms | 2 | every core 0–3 %, **~1 980 sw/s** | `rate=19.2/s gap=52..52ms` |
    | 804 512 ms | 10 | cores 60–85 %, **~1.45 M sw/s** | `rate=19.6/s gap=51..51ms` |

    **A 731× change in machine load moved the period from 52 ms to 51 ms — it got 2 % FASTER**, and it is
    flat to the millisecond across all 112 `[wcn] win=3` samples spanning the whole boot. A self-sustaining
    spin collapse lengthens the frame as load rises; this does the opposite. Note further that
    `51 ms / 16 667 µs = 3.06` — **three panel frames almost exactly** — which is the signature of a
    cadence lock in the present/composite path, not of a scheduler or spin equilibrium. The arc's own
    falsification row ("rate unchanged → REFUTED, next suspect the compositor") was answered by this
    capture before the next boot, which is the outcome a falsifier is for.

  - **Primary suspect is now the COMPOSITOR, and the evidence is already on the wire.** `[wcn]` shows
    `comp` far above `att`: win=10 reads `att=253 comp=1858`, `comp_rate=365/s` against a 49.7/s present
    rate — **every window is recomposited on every other window's present**, ~450–500 full composites/s
    on a 60 Hz panel. That is a `video/wm.rs` arc and was deliberately not attempted here.

  - **The table row that was cited, and why it does not select uniquely (review D4).**
    `packseen/passes = 272154/2675999` ≈ **10.2 %** with `pack=0` at almost every census and the rate
    unchanged does match this row of the table in `sched.rs` (mirror in `scheduler.md`), verbatim:

    > | `pack=0` but `packseen/passes` materially non-zero, rate unchanged | the packing is real and
    > TRANSIENT — sub-census, forming and clearing inside a frame. Neither the floor nor the pin can hold
    > a queue that is empty whenever it is looked at; this is a barrier/wake-latency story, not a
    > placement one. |

    But **the CHURN criterion fires on the same window, on all three of its numeric legs** — `remig/moves`
    ≈ 1.0 (threshold > 0.5), `moves` ≈ **157/s** (threshold > 100/s), `Δcr3sw/Δmoves` ≈ **16.3** (threshold
    ~2). So `steal_floor` churn is a LIVE COMPETING DIAGNOSIS for the same signature, and row 3 is not a
    unique selection. Neither is convicted here.

  - **What the switch arithmetic does and does not say.** Summing per-core `sw` deltas from
    `[schedx86] load` over one 11.25 s window gives **1.45 M context switches/s** on eight cores against
    `[spread] cr3sw` of 2 550/s, so 99.8 % never change address space. That places them inside some
    address space's own threads and bounds the yield loops **as a class** — it does **not** separate the
    worker release-poll from the parent's `BARRIER_SPIN_YIELDS` spin, and nothing in the capture does.
    Against `[wpace] rollup rate ≈ 450/s` that is ~3 200 switches per presented frame, i.e. **~1 600
    passes per worker per frame — 39 % of the 4096 budget, never reached, so a worker on the bench never
    parks.** Read that as an upper bound on this loop's share.

  - **Two OPPOSITE slow-window shapes, not one (review D2).** They must not be lumped:
    `[wcn] win=3` (asid `0x2`) is flat — 19.6/s, `gap=51..51ms`, zero jitter all boot. `[wcn] win=4`
    (asid `0x3`) is the opposite — 15.2–21.7/s with `gap=8..204ms`, heavy jitter. All nine vugs are
    identical 128×128 windows from one binary with identical placement, and **nothing yet identified
    distinguishes asid `0x2`**. A single mechanism that explains both shapes has not been found; any next
    arc should treat them as two observations, not two samples of one.

  - **⚠ `[wcn] win=` and `[wpace] win=` are DIFFERENT NAMESPACES (review D3).** `[wcn]` enumerates every
    live window including the console (`win=1 asid=0xffffff01`, composites only); `[wpace]` indexes the
    window-id table. On Boot A the offset is **`wcn = wpace + 2`** — `[wcn] win=3 asid=0x2` and
    `[wpace] win=1` are the same window, both 19.6/s. Pair `[vugfps]` against `[wpace]` only; it carries
    the program's own `SYS_WIN_CREATE` handle, which is the `[wpace]` index. Re-derive the offset per
    boot rather than assuming 2.

  - **Fix (waste and instrument hygiene, NOT fluidity): `WORKER_SPIN_YIELDS` 4096 → 64**, sized the way
    `BARRIER_SPIN_YIELDS` already is — as a LATENCY threshold, not a rate. It is justified independently
    of the withdrawn conviction: spinning ~1 600 times to wait for an event is waste whether or not it is
    the bottleneck, and a budget too large to reach hides the park it is supposed to fall back on. The
    spin **still exists**: VUGPAUSE-2 recorded that a *bare* `futex_wait` here failed the 300-frame
    checksum run on raspi4b with all three tasks parked at the kill, and that result convicted a budget of
    **zero**, not a small one.
    - **⚠ Say plainly what 64 does on the bench (review D8).** On a 51 ms frame a worker exhausts 64
      passes long before the release arrives, so **every worker now parks and is woken once per frame, BY
      DESIGN** — two futex round trips per frame instead of ~1 600 yields, on a machine with idle cores to
      dispatch to. The inherited claim that "the rendering path never gets past the spin" is therefore
      **QEMU-scoped** from here on: true on an unloaded fixture machine, false on the bench. If the next
      boot shows the slow windows getting *slower*, this trade is the first thing to re-examine.

  - **Fix (truth, part 1): a failed present is no longer counted as a frame.** `frame += 1`,
    `presented = true` and `presented_overlay` ran unconditionally, immediately after the branch that had
    just established `SYS_WIN_PRESENT` returned a negative errno. Both are lies about the panel: `frame`
    is the numerator of the VUGFPS readout ("counts frames PRESENTED"), so a failing present inflated the
    on-window fps by exactly the failure rate; and `presented = true` would let the VUGPAUSE idle
    predicate SKIP the very frame that would have repaired the window. Both now sit on the success path.
    Deliberately a guard and **not** a `continue` — skipping the iteration would skip the exit block with
    it, so a window whose presents had started failing would stop answering ESC.
    - **⚠ And the exit budgets had to move off `frame` with it (review D5).** `INTERACTIVE_CAP` and
      `AUTO_FRAMES` both read `frame`, so freezing `frame` on failure would freeze the DEADLINE too: a run
      whose presents had all started failing would never terminate — `:: EXEC-UVUG: … did not exit in
      time ::` on any fixture leg where the present path breaks, a hang introduced by a fix for a lie.
      A separate `attempts` counter now clocks the budgets; it counts present ATTEMPTS, which is exactly
      what `frame` meant here before. The two are equal on every healthy run, so the 300-frame checksum
      path is bit-identical; they diverge only in the failure case, where each is the right quantity for
      its own job — `frame` for the meter, `attempts` for the deadline.

  - **Fix (truth, part 2): `[vugfps] wf=` — the panel's number reaches the wire.** GR24 fixed this
    readout's arithmetic (ABIFREEZE D1: VUG-X86.ELF divided by 250 on a 1 kHz kernel, four times too low)
    and the next sitting still called the shown fps wrong. That could not be settled from a capture for a
    structural reason: **the program had never printed the number it draws.** It now emits
    `[vugfps] wf=<win*1000 + shown>` whenever the displayed digits CHANGE (`v != fps` — free rate-limiting,
    and strictly more informative than a period: a window holding a steady rate goes quiet rather than
    repeating itself once a second into a log with ten vugs in it). Decode `win = wf / 1000`,
    `shown = wf % 1000`; `shown` carries the same 999 clamp `draw_num` applies to the painted digits, so
    the packing cannot collide.

  - **The falsifier table for the next boot.** `shown` and `[wpace] win=N rate=` measure the same event
    and are the same quantity by construction (`frame` advances only on a successful present, which is
    exactly what `wpace_note_present` counts).

    ⚠ **GR25 (VSYNC-PACE r3) changed what the right-hand side MEANS, and the table below still reads
    correctly only if you check `[wpace] mode=` first.** The present pacer is now opt-in and the shipped
    desktop is unpaced, so `[wpace] win=N rate=` is the program's own rate rather than a 60-ish ceiling.
    The pairing itself is unaffected — `wpace_note_present` moved from `pace_advance` into the present
    verbs precisely so it keeps counting on an unpaced boot — so row 1 is still the finding it was, and
    row 2's "rate still ~19.6/s" is still the EXPECTED reading for the slow windows (those windows read
    `paced=0 slept=0ms` on Boot B and were never slept, so removing the sleep cannot move them). The
    FAST windows, however, are no longer capped, so a `shown`/`rate` pair in the low hundreds is the new
    healthy shape there and must not be read against this table's 19.6 figure. See
    `docs/dev/OS/08_VIDEO/engine.md` "VSYNC-PACE r3".

    | next boot shows | reading |
    | --- | --- |
    | `shown` far from `[wpace] win=N rate=` | **the disagreement is the finding**, and the first time it could be one. The count is shared by construction, so a gap means presents the kernel never saw or saw twice. This is the question the arc set out to answer. |
    | `shown ≈ [wpace] rate`, rate still ~19.6/s, `gap` still ~51 ms | **EXPECTED** — the meter is honest and the frame rate was never the spin. Hand off to the compositor arc; do not re-touch `WORKER_SPIN_YIELDS`. |
    | `shown ≈ [wpace] rate` and the slow windows have RISEN | the spin was costing more than this arc credited it with. Welcome, but it does not restore the withdrawn conviction — the load-invariance above still has to be explained. |
    | the slow windows have gone SLOWER | the 64-pass budget's park-per-frame trade (D8) is not paying on the bench. Re-examine `WORKER_SPIN_YIELDS` first. |
    | `sw` deltas still summing to ~1.4 M/s with `cr3sw` flat | the worker poll was not the switch source. The parent's `BARRIER_SPIN_YIELDS` is the other half of the class the 99.8 % figure bounds, and it was not changed. |
    | `[wcn] win=3` still flat at `gap=51..51` while `win=4` still jitters `8..204` | the two shapes (D2) are still unexplained and still opposite; a single-mechanism theory is still wrong. |

  - **A witness that was dropped because it could not fail.** An earlier cut carried `frames=` beside
    `shown=`. It is not the clock check it looked like: `shown = Δframe * TICK_HZ / dt` and the refresh
    fires at `dt >= TICK_HZ`, so `shown ≈ frames` is an **algebraic identity that holds however wrong
    `TICK_HZ` is** — under the exact D1 bug GR24 fixed, the refresh simply fired four times a second and
    `shown` still equalled `frames`. The only real check on this program's clock is the kernel's number
    beside it.

  - **Size — the constraint that shaped the patch, measured rather than guessed.** VUG-X86.ELF's `.text`
    was `0x1fcd` of a `0x2000` page — **51 bytes of headroom** — past which `.bss` moves up a page and the
    ELF file grows by 4096 **in one step**. That cliff, not code size, is why a second label/number pair
    on the witness line is unaffordable (every variant cost 16736 B against a 16384 B limit) and why the
    two fields are packed into one decimal. It was paid for by collapsing `say` and `sayn` onto one `emit`
    body. **Net `.text` is `0x1fcd` — byte-for-byte the baseline figure**, with the whole arc (new witness,
    the `attempts` counter, the meter guard) paid for out of that one collapse, and both ELFs are 12568 B, the
    same as before the arc. Note for the next reader: the build script's printed file size overstates the
    real footprint (section headers plus page padding); the LOADable memory here is ~12.4 KiB of the
    16 KiB window. Check trims with `readelf -lW` against the `0x2000` line, not against that number.

  - **Gates:** `./arroyo check` and `UNAOS_WC=1 ./arroyo check` green both arches, `user-vug` green on
    both targets; `./arroyo kernel8-test` **MBENCH PASS 105/105, 0 forbidden hits** with
    `UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c` unchanged — the checksum is the exact witness
    that a bare park broke, so it is what licenses the spin change; `./arroyo test` MISSION SUCCESS. The
    `[vugfps]` line cannot perturb the checksum run: it rides `fps_refresh`, which the foreground auto
    path never reaches (overlay is `detached || interactive`), and no `[vugfps]` line appears in either
    QEMU log.

- **VUGSCENE (the shard, as real 3D geometry)** — an **app-only** arc: `crates/user-vug`, its `arroyo`
  build function, the builder's media staging, and this doc. No kernel file, no `wm.rs`, no syscall path,
  and — the point of the arc — **no pacing machinery of any kind**. Peter's ruling on metal was that a lone
  vug reads 999 fps because its drawing is trivial: *"make the damn drawing more complex instead of
  pacing."* So the renderer got heavier and nothing else changed; the fps the meter reports is the honest
  rate of a frame that now costs something.

  - **What it draws.** A solid, faceted SHARD — the brand mark as geometry rather than as a wireframe
    outline. An elongated hexagonal bipyramid with irregular girdle radii and asymmetric apexes, so it
    reads as a broken shard and not a jeweller's stone; one orbiting light; flat per-facet shading with a
    hard specular kick as a facet sweeps the light. The palette is **read from the menu bar's crystal**
    (`video/menubar.rs::crystal_facet` -> `theme::CONTROL_CLOSE`/`CONTROL_MID`/`CONTROL_ZOOM`), so the
    crystal on a vug window and the crystal in the bar are the same object family by construction rather
    than by coincidence.

  - **How, and why that method.** The shard is CONVEX, so it is exactly the intersection of **18
    half-spaces** — and that representation is the whole renderer. There is no vertex list, no face list
    and no projection on this path: each screen sample is a ray, clipped against the 18 slabs, keeping the
    last plane it entered through. Convexity makes that answer **exact** hidden-surface removal, which is
    what buys the arc a z-buffer it could not have afforded (a 128x128 depth buffer is 32 KiB against a
    16 KiB user window) and a depth sort it does not need. The surface stays 128x128 — that is
    `FB_WIN_MAX_W/H`, one 64 KiB window slot, and the ABI ceiling, not a choice.

  - **The ladder is ray density, and it adapts.** Level 0 is the classic wireframe, byte for byte; levels
    1-3 cast one ray per 4x4 cell, per 2x2 cell and per pixel, so cost scales 1:4:16. The program reads
    its OWN achieved rate off `fps_refresh` — the meter's existing one-second window on `SYS_GETINFO`
    ticks, no new syscall and no second clock — and steps the level to fit: below 24/s it falls (two rungs
    at once if it is far under, so calibration finishes in a second or two), above 55/s it climbs. Two
    hysteresis mechanisms, because one is not enough: a wide dead band, and a **ceiling** pinned by any
    step down that relaxes one rung only after 8 quiet windows. **Adaptation changes work per frame and
    never rate** — there is no sleep in this program and this arc adds none.

  - **The level is visible.** A third readout joins fps and clicks in the corner band, in the gem ramp's
    lit tone, and every change prints `[vuglod] lvl=<L*1000 + rate>` on the wire — so "which level did
    that boot run at?" is answered from the panel and from the capture, not inferred from the fps.

  - **Pinned images for benchmarking.** `bg`/`run` take a path and no argv on either arch
    (`shell.rs`: `match args.first()`), so a pin has no runtime channel to travel down; it is a cargo
    feature, and `arroyo` links the same source three times. **VUG.ELF** (adaptive, the default and what
    the desktop launches), **VUGC.ELF** (`pinlo` — level 0 forever, the classic pattern, byte-honest so
    old fps baselines stay comparable), **VUGX.ELF** (`pinhi` — the full shard forever, for measuring the
    heaviest frame this program can draw). All three are 8.3-clean and staged on both the ESP and the
    x86 data volume beside `STAT.ELF`/`PULSE.ELF`.

  - **The 300-frame checksum is untouched, and by construction rather than by luck.** The scene is gated
    on `overlay` (`detached || interactive`) — the same predicate that already gates the fps overlay — so
    the foreground/no-input path that the checksum witness runs on renders level 0, the unmodified
    wireframe, in every one of the three images. The one shared routine that changed shape is `fsin`,
    whose 256-entry table became a 64-entry quarter table to buy `.text`; it was verified **bit-identical
    for all 256 inputs** against the table it replaces, so `pi4-regression.spec`'s
    `UVUG: frames=300 threads=2 checksum=0xe68285b85121ac7c` still holds.

  - **Size — again the constraint that shaped the patch.** `.text` must end at or below `0x2000`; one byte
    past it and `.bss` moves a page and the image jumps 4096 bytes through the 16384-byte gate. The
    baseline was `0x1fcd` (51 bytes of headroom) and the scene needed ~3 KiB. What paid for it, measured
    at each step: the quarter sine table (-508), an edge-function rasteriser abandoned for the ray/convex
    tracer (-2000 against the same feature set), dropping in-pixel supersampling for a fourth rung (-346),
    `u16` sine and plane-offset tables (-54), packing each facet's colour once instead of three channels
    (-12) — and, the largest single item, **`fsin` marked `#[inline(never)]` (-432)**, because six inlined
    copies of a table lookup with a branch is what a quarter table costs if you let it inline. Two
    counter-intuitive results worth recording: a blanket `#[inline(never)]` on the cold start-up routines
    made the image **larger** (+132), and so did hand-folding the ray basis LLVM was already folding
    (+68). `opt-level` moved `"s"` -> `"z"` and that is a requirement, not a preference: the same source is
    9534 bytes of `.text` at `"s"` and 8023 at `"z"`. Final `.text`: **0x1f57 (8023) adaptive, 0x17d6
    (6102) `pinlo`, 0x1e8e (7822) `pinhi`, 0x1e33 (7731) aarch64** — all four under the cliff, all three
    x86 images 12568 B, unchanged from before the arc.

  - **Gates:** `./arroyo check` and `UNAOS_WC=1 ./arroyo check` green both arches, `user-vug` green on
    both targets; all three x86 images build and pass `arroyo`'s four loader checks (present, ELF magic,
    `e_machine = 0x3e`, <= 16384 B). Metal day — no QEMU legs, by operator order. **Next boot should show:**
    the shard on glass in the menu bar's blues, tumbling, with a glint crossing its facets; `[wcn] rate=`
    and the on-window readout settling to an honest number **below 999** that falls as the level rises;
    `[vuglod] lvl=` naming the rung the machine settled on within the first second or two; and per-core
    load reflecting real rendering rather than an idle spin. `bg /fat/VUGX.ELF` beside it should read
    slower still, and `bg /fat/VUGC.ELF` should reproduce the old wireframe rate exactly.

### x86_64 (branch `hw-rmbp`)

- **HEADROOM** — the ring-3 ceilings, raised coherently (Boot AL). `USER_SLOTS`
  8 → 12, `MAX_PROCS` 6 → 10, `NTHREAD` 8 → 24, `WIN_MAX` 8 → 12,
  `wm::MAX_WINDOWS` 8 → 12, `sched::NFUTEX` 16 → 64, `shell::BG_JOBS` 8 → 12;
  `FB_WIN_SLOTS` deliberately stays 8. The `MAX_PROCS <= USER_SLOTS - 2` reserve
  is preserved at equality, so none of the new capacity was bought from the
  margin a foreground `run` depends on. What it buys: **8 vugs all with
  `workers=2`** alongside the resident desktop app, instead of 5 vugs of which
  only 4 got workers. The full ledger — which cap bit, why each dependent had to
  move with it, the RAM cost, and the recomputed `storm` arithmetic — is in
  [`scheduler.md`](scheduler.md), "Fleet headroom, x86".

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
  SYSCALL stub), logs `:: RING-3 FAULT: task '…' KILLED — vec=… err=… rip=…
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

## CRYSTAL-HD — the window surface cap rises to 288, on both arches (2026-08-18)

Peter's sign-off, 2026-08-18, verbatim option "1": **`FB_WIN_MAX` 128 → 288, window slots 8 → 4,
+15 MiB `.bss`.** The held `crystal-graphics-hold` commit (`e65d5d9b`) proposed it for x86 only; this
landing applies it to **both** arches, because a `target_arch` gate in the experience layer with no
hardware reason behind it is what the ONE-OS ruling (2026-08-13) fails at review.

**The constants, both arches.** `arch/x86_64/memory.rs` and `arch/aarch64/boot.rs` now carry the same
cluster: `FB_WIN_MAX_W/H = 288`, `FB_WIN_SLOT_SIZE = 0x51000` (81 pages — exactly 288×288 ARGB8888),
`FB_WIN_SLOTS = 8 → 4`, `USER_STATIC_SIZE = 0x149000`. The slot count comes down because the slot
size went up 5×: the whole per-slot FB region must still fit the ONE page table each arch's
`build_slot` wires (512 × 4 KiB = 2 MiB; 0x149000 is 329 of those pages), and four windows per
address space is still 4× what any shipped program opens. On aarch64 the `USER_REGION` VA anchor's
alignment moves 0x100000 → 0x200000 with it — the old alignment guaranteed "cannot straddle a 2 MiB
L3 block" only because the size was ≤ 0x100000, and 0x149000 is not.

**What it costs the Pi, measured.** The aarch64 kernel heap is hand-placed at 32 MiB
(`boot::MEM_REGIONS`), so `.bss` has a hard ceiling there. `readelf -S` on the `kernel8` build:
`.bss` at `0x200000`, size `0x68fb88` → `0xeafb88`, i.e. **end 8.56 MiB → 16.69 MiB, margin to the
heap floor 15.31 MiB**. The SD image does not grow by a byte — `.bss` is NOBITS and `kernel8.img` is
an objcopy of the loaded sections only. No memory-gated cap was needed.

**Two seams the cap divergence opened, both closed here.**

* `WIN_MAX == FB_WIN_SLOTS` on aarch64 became `FB_WIN_SLOTS <= WIN_MAX`, matching the x86 twin. The
  two count different things: `WIN_MAX` (8) is how many windows the SYSTEM may have live,
  `FB_WIN_SLOTS` (4) how many surface slots ONE address space reserves.
* `sys_win_create`'s region-slot search on aarch64 ranged over `WIN_MAX`, which was harmless only
  while the caps were equal. With 4 slots and 8 ids it would have handed out region slot 4..7 and
  `map_slot_fb_win` would have mapped 81 pages past the end of the FB region, into the next slot's
  backing. The CANDIDATE range is now `FB_WIN_SLOTS`; the SCAN range stays `WIN_MAX` (any global row
  could belong to this ASID). The fifth window gets the `-EMFILE` the verb already documents.

**The `el0-wcb` fixture, updated honestly, ledger unchanged.** The witness mask stays `0x3ffff` —
all eighteen bits, go-red preserved — but three facts inside it had to move with the cap: b7's
over-max create is `create(289, 10)` (129 is now legal), region slot 1's surface in the blob is
`base + 0x56000` (was `base + 0x15000`, the 64 KiB stride), and `wcb_expected_checksum` is over the
fixture's OWN 128×128 window (new `WCB_W`) rather than over `FB_WIN_MAX_W/H`, which was only ever a
coincidence. QEMU-verified: `:: EL0: window verbs — create/present/present_rows/move/close
witness=0x3ffff surface=128x128 checksum=0xfabe809492cf2325 :: PASS ::`.

**SPEC UPDATE, deliberate — the 300-frame auto checksum changes again.** `user-vug`'s surface is
288×288 on both arches now (`FOCAL` 24 → 54, so the framing is identical and the change is
resolution), and the checksum is a pure function of the render:
`:: UVUG: frames=300 threads=2 checksum=0xf18f983557b87a55 ::`, **superseding
`0xe68285b85121ac7c`** (which itself superseded `0x48221e4101db3924`). Re-pinned in
`scripts/specs/pi4-regression.spec`, both the `REQUIRE` and the negative-lookahead `FORBID`.
Reproduced identically across separate `kernel8-test` runs.

**CRYSTAL-PACE is NOT landed, and the ABI records why.** The held commit's third half made
`SYS_WIN_PRESENT` answer a new status (`WIN_PRESENT_COALESCED = 2`) so `user-vug` could park until
the x86 compositor's frame edge admitted a present. Both the status and the ring-3 pace loop are
dropped. Peter's ruling of 2026-08-13 (`9d12e7e0`, `08_VIDEO/PARITY.md` §5.1) is that a vug renders
**unpaced on every chip** — "more drawing complexity, never artificial pacing" — and a status whose
only consumer is a self-pacing render loop is that pacer with the sleep moved one syscall outward.
Independently, keeping `0` for every success is what keeps verb 30's contract **identical on both
arches**: aarch64 has no coalescing pacer and could never answer a third status. The LOD ladder's
`LOD_UP` climb threshold is likewise unchanged — the held commit replaced it with a render-time
utilisation license on the premise that a paced meter reads ~60 regardless of headroom, and that
premise was the pace loop's.

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

- **The syscall ABI is FROZEN in one crate: [`unaos/crates/una-abi`](../../../../unaos/crates/una-abi/src/lib.rs).**
  That crate — `no_std`, no dependencies, nothing but `const` — is the single
  declaration of the syscall numbers, the sub-op/flag encodings, the packed
  layouts ring 3 reads back, and the shared magic values. `arch/x86_64/syscall.rs`,
  `arch/aarch64/syscall.rs` and every `user-*` crate `use` it; **no site may
  re-declare a number**, and a table below that disagrees with the crate is a bug
  in this document, not in the crate.

  Before the freeze the table was declared eight times — both kernels plus a
  partial re-typing in each ring-3 crate — and `arroyo`'s own `USER_CHECK_MATRIX`
  note already named those crates "exactly where a syscall-number or stub drift
  between arches can hide". It had happened; the DIVERGENCE LEDGER at the top of
  `una-abi/src/lib.rs` records each case and how it was resolved. The headline one:
  **`SYS_GETINFO`'s `ticks` field has never had the same unit on both arches** —
  x86 fills it at 1 kHz (one tick = one millisecond), aarch64 at the 250 Hz
  scheduler tick — and the two arch-neutral programs that read it had assumed
  OPPOSITE units, so `VUG-X86.ELF` drew an fps figure 4x low and `PULSE.ELF` on the
  Pi swept a "3-second" animation in 12 s. Both kernels' clocks are shipped
  behaviour and neither moved; ring 3 now reads the rate from
  `una_abi::GETINFO_TICK_HZ`.

- **Register conventions are per-arch.** aarch64: number in `x8`, args `x0..x5`,
  return `x0`, `svc #0`; the SVC path restores the full GPR + FP file. x86_64:
  number in `rax`, args `rdi,rsi,rdx,r10,r8,r9`, return `rax`, `syscall`; the
  `sysretq` tail SCRUBS `rdi/rsi/rdx/r8/r9/r10` on **every** return regardless of
  arity, and `syscall` itself destroys `rcx`/`r11`, so an x86 ring-3 stub must
  declare all eight as clobbers whatever its own arity.

- **A number means the same verb on every arch, implemented or not.** Numbers
  1..=33 are the shared block; 40..=48 is where the x86 socket family was moved
  after it was found colliding with aarch64's 19..=27. A verb an arch reserves but
  does not dispatch falls to that dispatcher's default arm and returns `-ENOSYS`,
  which is a designed outcome — `user-vug`'s `present_rows` and its
  `SYS_INPUT_WAIT` park each carry an explicit fallback for exactly that.

  Implementation matrix at the freeze (`Y` = dispatched, `-` = reserved):

  | # | Name | x86 | arm | Args → returns |
  | :--- | :--- | :--- | :--- | :--- |
  | 1 | `SYS_WRITE` | Y | Y | fd, buf, len → count / `-errno`. fd 1 = console; a `File` handle with `CAP_WRITE` writes the file |
  | 2 | `SYS_EXIT` | Y | Y | status → *(no return)* |
  | 3 | `SYS_REPORT` | - | Y | value → 0. Demo accounting channel keyed by task name |
  | 4 | `SYS_YIELD` | Y | Y | — → 0 |
  | 5 | `SYS_SLEEP_MS` | Y | Y | **milliseconds** → 0. Each kernel converts through its own `ms_to_ticks`, so this argument needs no rate correction |
  | 6 | `SYS_GETPID` | - | Y | — → task id |
  | 7 | `SYS_GETINFO` | Y | Y | ptr → 0 / `-EFAULT`. Writes `una_abi::UserInfo {pid, ticks}`. **`ticks` is in `GETINFO_TICK_HZ` units, which differ per arch** |
  | 8 | `SYS_SPAWN` | Y | Y | — → child **handle** / `-errno` (never a raw pid) |
  | 9 | `SYS_WAIT` | Y | Y | handle → exit status / `-ECHILD` |
  | 10 | `SYS_CAP` | Y | Y | op, … → op-specific. `CAP_OP_GRANT`/`REVOKE`/`XREVOKE` |
  | 11 | `SYS_OPEN` | Y | Y | name, len, mode → `File` handle / `-errno`. `mode` = `O_RW`\|`O_CREAT`\|`O_PUBLIC` |
  | 12 | `SYS_READ` | Y | Y | handle, buf, len → count (0 = EOF) / `-errno`. Needs `CAP_READ` |
  | 13 | `SYS_XFER` | Y | Y | dest-child, src, rights → transfer id / `-errno` |
  | 14 | `SYS_RECV` | Y | Y | — → handle / `-errno`. The caller's own capability inbox |
  | 15 | `SYS_SEEK` | Y | Y | handle, offset → new offset / `-errno` |
  | 16 | `SYS_UNLINK` | Y | Y | handle → 0 / `-errno`. Needs `CAP_WRITE` |
  | 17 | `SYS_CLOSE` | Y | Y | handle → 0 / `-errno`. Needs no right |
  | 18 | `SYS_FGRANT` | Y | Y | file, child, rights → 0 / `-errno`. An ACL edge on the FILE |
  | 19 | `SYS_MSEND` | - | Y | frame, len → 0 / `-errno`. One BUS v1 request frame |
  | 20 | `SYS_MRECV` | - | Y | buf, len → reply length / `-errno` |
  | 21 | `SYS_THREAD_SPAWN` | Y | Y | entry, sp, arg, place → thread handle / `-errno` |
  | 22 | `SYS_THREAD_EXIT` | Y | Y | — → *(no return)* |
  | 23 | `SYS_THREAD_JOIN` | Y | Y | handle → 0 / `-ESRCH` |
  | 24 | `SYS_FB_MAP` | - | Y | — → surface VA / `-errno`. The single-surface compat path |
  | 25 | `SYS_FB_PRESENT` | - | Y | — → 0 / `-errno` |
  | 26 | `SYS_FUTEX` | Y | Y | uaddr, op, val → op-specific. `FUTEX_WAIT`=0, `FUTEX_WAKE`=1 |
  | 27 | `SYS_INPUT_POLL` | Y | Y | — → packed event / `-EAGAIN`. `[55:48]` = type, low 32 = payload, bit 63 always clear |
  | 28 | `SYS_INPUT_WAIT` | Y | Y | — → 0 / `-EINVAL`. Blocks; dequeues nothing, so it composes with 27 |
  | 29 | `SYS_WIN_CREATE` | Y | Y | w, h → win id / `-errno`. ARGB8888, `stride = w*4`, 1..=288 each (CRYSTAL-HD; was 1..=128) |
  | 30 | `SYS_WIN_PRESENT` | Y | Y | win → 0 / `-errno`. Whole-window damage |
  | 31 | `SYS_WIN_MOVE` | - | Y | win, x, y → 0 / `-errno` |
  | 32 | `SYS_WIN_CLOSE` | - | Y | win → 0 / `-errno` |
  | 33 | `SYS_WIN_PRESENT_ROWS` | Y | - | win, y0, y1 → 0 / `-errno`. ADDITIVE, because widening 30 in place would let a stale clobber register silently UNDER-repaint |
  | 40–48 | socket family | Y\* | - | `SOCKET`, `BIND`, `SENDTO`, `RECVFROM`, `CONNECT`, `SEND`, `SOCK_RECV`, `LISTEN`, `ACCEPT`. \* also gated on the `smolnet` feature |
  | 49 | `SYS_CPUPULSE` | Y | - | ptr → 0 / `-EFAULT`. Writes `una_abi::UserPulse` — RAW cumulative `(busy, idle)` per core, never percentages |

  49 is the high-water mark; the next verb minted takes 50, and 34..=39 stay free
  between the shared block and the socket family.

- **User faults kill the task, kernel faults stay fatal.** Fault accounting
  is matched (task, vector/EC, address) so demos assert exact outcomes.
- **User pages are never executable-and-writable**; code pages are read-only
  to the kernel after load.
