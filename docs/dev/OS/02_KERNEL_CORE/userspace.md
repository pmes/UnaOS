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
- Not yet: per-task address spaces/ASIDs (M6d), preemptible EL0 (M6e), validated
  user pointers (M6f).

### x86_64 (branch `hw-rmbp`) — starting from ~10–15 %

What exists: per-CPU GDT/TSS with descriptor slots reserved for user segments
(`arch/x86_64/gdt.rs`), IST double-fault stacks, the full SMP scheduler.

What is missing (arcs U1a/U1b): user code/data descriptors (SYSRET-compatible
STAR layout), `TSS.RSP0`, the SYSCALL MSRs (`EFER.SCE`, `LSTAR`, `STAR`,
`FMASK`), any U/S or NX bit in the inherited UEFI identity map, `CR4.SMEP`,
and fault handlers that kill a user task instead of `hlt_loop()`.

## The chain

| Arc | x86_64 (lead) | aarch64 (port) |
| :--- | :--- | :--- |
| Privilege round-trip | U1a | M6a ✅ |
| Per-page perms + fault→kill | U1b | M6b ✅ |
| Loadable programs | U2 (from FAT storage) | M6c ✅ (embedded blob) |
| Per-process address space | U3 (per-process PML4/CR3) | M6d (TTBR0 + ASID) |
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
