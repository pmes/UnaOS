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
  preempts. Metal re-confirm of that fixture pending.)
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
  unchanged and CAPSTONE 6/6. Metal-verify pending — the ASID/TLB/`nG` discipline and
  the SP-sentinel-under-preemption are exactly what QEMU (no TLB, no caches, no Group-1
  IRQ) cannot exercise; note that on metal `IRQs-taken-at-EL0` grows (four more
  preemptible EL0 tasks), which is expected and metal-variable.
- Not yet: validated user pointers (M6f).

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
- Not yet (arc U1b): user fault → task kill (today a ring-3 fault is fatal but
  lands cleanly on RSP0), full user-GPR preservation across a syscall,
  validated user pointers (`copy_from_user`).

## The chain

| Arc | x86_64 (lead) | aarch64 (port) |
| :--- | :--- | :--- |
| Privilege round-trip | U1a ✅ | M6a ✅ |
| Per-page perms + fault→kill | U1b | M6b ✅ |
| Loadable programs | U2 (from FAT storage) | M6c ✅ (embedded blob) |
| Per-process address space | U3 (per-process PML4/CR3) | M6d ✅ (TTBR0 + ASID) |
| Process model, PIDs, handle table | U4 (arch-neutral) | U4 |
| Capabilities at the syscall boundary | U5 (arch-neutral) | U5 |
| FS-backed grants | U6 (UnaFS attributes) | U6 |

Conventions shared across arches:

- **Syscall numbering is common** (`SYS_WRITE = 1`, `SYS_EXIT = 2`, …);
  register conventions per-arch (aarch64: `x8`/`x0..x5`; x86_64:
  `rax`/`rdi,rsi,rdx,r10,r8,r9`).
- **User faults kill the task, kernel faults stay fatal.** Fault accounting
  is matched (task, vector/EC, address) so demos assert exact outcomes.
- **User pages are never executable-and-writable**; code pages are read-only
  to the kernel after load.
