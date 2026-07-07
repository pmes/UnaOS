# UnaOS milestones

A running, quick-to-digest log of what landed each integration round — one
entry per arc, newest first. Each entry: **what it does**, **how it was tested**
(QEMU + metal), and the commit. Deep detail lives in the per-subsystem docs
under [`dev/OS/`](dev/OS); the ledger of hardening state is in
[`SECURITY.md`](SECURITY.md); direction is [`ROADMAP.md`](ROADMAP.md).

Legend: **✅ metal-confirmed** · **🔬 QEMU-green, metal pending** · dates ISO.

---

## hw-rmbp track — 2026-07-06 (landed on `hw-rmbp`, awaiting integration)

### U5x — handles as capabilities: the CHECK + grant/attenuate/revoke + routed `sys_write` + teardown-clear (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of aarch64 U5 — turns U4x's handle STRUCTURE into a real **capability**,
  keyed by the address-space **slot** where aarch64 keys by ASID. A handle now carries **rights**
  (`CAP_READ|CAP_WRITE|CAP_EXEC|CAP_GRANT|CAP_REVOKE`, in a sidecar `HANDLE_RIGHTS` array keyed
  identically to `HANDLES`, so U4x's `0`/`RESERVING` value-word sentinels stay byte-unperturbed) and
  names a **target** beyond "child pid" (a `HANDLE_CONSOLE = u64::MAX-1` token — two kinds,
  `Child(pid)`/`Console`, no general object table = U6). **The CHECK** is a single
  `handle_resolve(row, idx, req_rights)` at the one lookup point every handle-consuming path uses:
  out-of-range/Empty/`RESERVING` ⇒ the caller's own errno (`sys_wait` → `-ECHILD`, U4x ownership
  preserved; `sys_write`/`SYS_CAP` → `-EACCES`), missing-a-right ⇒ `-EACCES`. **`SYS_CAP=10`** carries
  GRANT (mints an **attenuated** handle — `req & !src_rights != 0` ⇒ `-EACCES`, so a grant can never
  amplify; requires `CAP_GRANT` on the source) and REVOKE (ownership-based). **`sys_write` routes
  through the table** — `fd` is a handle that must resolve to `Console`+`CAP_WRITE`; no ambient stdout.
- **The x86 divergence (SLOT vs ASID):** the shared kernel window (U1a/U1b/U2 run with `user_cr3 == 0`,
  so `current_slot()` is `None`) has no private slot, so `HANDLES`/`HANDLE_RIGHTS` grow one extra row
  `SHARED_ROW` (index `USER_SLOTS` — the x86 twin of aarch64 ASID 0); `caller_row()` maps `None →
  SHARED_ROW`. The console cap is endowed there in `setup()` (covers U1a/U1b/U2) and per child in
  `sys_spawn`, so every prior print still lands. The fixture conveys its 4-bit witness as its `sys_exit`
  **status**, routed by task name (x86 needs no `SYS_REPORT`).
- **Teardown-clear** (folds U4x's one deferred note): `memory::free_user_space_by_cr3` wipes the slot's
  handle row (values + rights) **before** releasing the used-flag; both teardown paths (normal `exit` +
  the KillSwitch reap) funnel through it, so the clear rides both.
- **Tested — QEMU:** `./arroyo test-fat part 30` (and `sf`) → after the U4x PASS line: the U5x setup
  line, `u5x: cap write` twice (the write-cap + the minted cap reaching the console), and
  `:: U5x: x86 capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded, revoke enforced,
  teardown-clear clean -> PASS ::`. **U1a/U1b/U2/U2.5/U3/U3.5/U4x all PASS byte-identical** (routing
  `sys_write` drops no print — every printing process holds the endowed cap); the default no-FAT
  `./arroyo test` stays MISSION SUCCESS with U2/U4x skipping cleanly (U5x, being an inline fixture,
  still runs + PASSes). `./arroyo check` both arches; **0 aarch64 files touched**.
- **Honest scope:** register-only + cooperative fixture; deferred to U6 (the pi4 U6 twin) — bandy
  handle-transfer, a general object table (fs/net kinds, first-free `Console`), cross-process revocation
  trees (`CAP_REVOKE` defined but reserved); PCID and `copy_from_user`/`copy_to_user` stay separately
  deferred; FP/SIMD-across-context-switch stays ledgered (U4x left it register-only).
- **Metal:** pending (pure syscall logic; the child loads ride U2's metal-confirmed FAT path).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

## hw-rmbp track — 2026-07-05 (landed on `hw-rmbp`, awaiting integration)

### U4x — the process model + per-process handle table (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of aarch64 M7/U4 — a parent loads a child program into its OWN
  address space, runs it ring-3, and reaps it by an owner-scoped **handle**. **Part 0 (the
  enabler):** the `TSS.RSP0` + per-CPU `syscall_kernel_rsp` install moved from the ring-3
  trampoline into the scheduler **DISPATCH** path (beside U3.5's CR3-at-dispatch), so a task
  RESUMED after a block (which never re-enters the trampoline) gets ITS OWN kernel stack —
  the prerequisite for >1 concurrent user task per core, closing a use-after-free where a
  resumed task's syscall/fault would otherwise land on a sibling's (possibly freed) kernel
  stack. **Part A:** `SYS_SPAWN=8` returns a small handle index into the caller's per-process
  handle table (`HANDLES`, keyed by the caller's address-space **slot** — the x86 stand-in
  for aarch64's ASID, read from the live CR3 via `current_slot`); `SYS_WAIT=9` blocks on the
  child's done-semaphore, returns its status, and **consumes** the handle. `PROCS` (pid-keyed)
  and `HANDLES` (slot-keyed) are separate, static, const-init — the reviewed aarch64 U4 design
  adopted directly. **Part B:** a parent spawns two children and reaps both by distinct
  handles; an orphan's `sys_wait(0)` returns `-ECHILD` (proving the tables are per-process).
- **Load path — an honest x86 divergence:** aarch64 reads `HELLO.BIN` inside the SVC handler
  (its EMMC2 driver is PIO). x86 storage is USB-over-xHCI, whose BOT read pump `hlt()`s to
  await completion — and `hlt` with IF=0 hangs, while the SYSCALL handler is IF-masked. So the
  FAT read is pre-staged off FAT ONCE on the BSP main loop (IF=1, the proven U2 path) and
  `sys_spawn` only memcpys the staged bytes into a fresh slot. Same observable behavior.
- **Tested — QEMU:** `UNAOS_FATIMG=1 ./arroyo test 30` (and `test-fat part`/`sf`) →
  `:: U4x: x86 process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD
  -> PASS ::`, with the two children each printing `hello from disk` (the 2nd/3rd in a full
  boot). **U1a/U1b/U2/U2.5/U3/U3.5 byte-identical** (proven by a pre-change baseline diff — the
  RSP0-at-dispatch move does not disturb the cooperative single-user-task path); U4x skips
  cleanly with no FAT volume (default / `UNAOS_USBSERIAL=1`). `./arroyo check` both arches; **0
  aarch64 files touched**. Two independent adversarial reviews (an 8-lens sweep + a 3-lens deep
  pass on the RSP0-at-dispatch use-after-free, the spawn/wait concurrency+leak+hang surface,
  and the blob's cross-syscall register ABI) each returned **0 findings**.
- **Honest scope:** register-only + cooperative fixtures (IF clear); `MAX_PROCS=4` + the static
  8-slot pool (STOP tripwires); no PCID; no `copy_from_user`/program-by-name; handle rows not
  cleared on slot teardown (harmless today — reapers consume handles; the capability CHECK +
  grant/attenuate/revoke + teardown-clear is U5). **FP/SIMD across a ring-3 context switch stays
  unsaved** (now reachable via Part 0's multi-task-per-core, not just U3.5 preemption) — the
  fixtures are register-only, so the gap is ledgered in SECURITY.md, not closed this arc.
- **Metal:** pending (rides the next reflash / FTDI cable day) — fully QEMU-verifiable (the reap
  wake is a scheduler post; child loads ride U2's metal-confirmed FAT path).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

## hw-jetson track — 2026-07-05 (landed on `hw-jetson`, awaiting integration)

### JM6 — drop the Orin boot core EL2 → EL1 + run the scheduler/CAPSTONE at EL1 🔬 QEMU-green / ⛔ metal FAILED `hw-jetson`
- **What:** repeats the JC3 drop on the **Orin** (Tegra234, Cortex-A78AE) boot core — it drops
  **EL2 → EL1** and runs the full six-primitive M4 CAPSTONE cooperatively at EL1, the first time
  the scheduler runs on Orin silicon. A new `arch/aarch64/boot_tegra.rs` (the tegra analogue of
  `boot_virt`) arms the EL1&0 regime at **`mmu_tegra`'s already-built identity `L1`** (`MmuInfo::
  ttbr0`) with `SCTLR_EL1.M=1` *while still at EL2* — dormant until the `eret`, so EL1 never runs
  an instruction with its MMU off — then a naked-asm drop (mask DAIF, seed VMPIDR/VPIDR, FP-enable,
  `HCR_EL2.RW`, **disable CNTP**, `SPSR_EL2 = 0x3c5`, `eret` to `x30`). `main.rs::tegra_early_stop`
  gains the JM6 terminus after JM4 (`fbcon::detach` → `boot_tegra::drop_to_el1(mmu.ttbr0)` →
  `percpu::init(0)` → `exceptions::install()` → `sched::run_capstone_boot_core(0)`, never returns).
  Single-core by design: JM5's Orin SMP (`CPU_ON`) is **parked** (metal-blocked on an external
  Tegra BL31/MCE RAS fault) and deliberately not invoked here, so JM6 sidesteps that wall.
- **Tested — QEMU (the DONE gate; Orin is not emulated):** `./arroyo check` both arches +
  `UNAOS_TEGRA=1 ./arroyo check` both legs, no new warnings. Non-regression: virt
  (`UNAOS_GICV3=1 test-arm 45`) SMP 3/3 + JC3 drop + `VBAR_EL1 = 0x7c38c000` + CAPSTONE 6/6 —
  **byte-identical** to JC3 (same VBAR address ⇒ virt binary layout unshifted); Pi (`kernel8-test`)
  **sorted-diff 0**; x86 (`test`) MISSION SUCCESS; `esp-jetson` media links. All JM6 code is
  `tegra`-gated, so every non-tegra build's cfg set (and output) is unchanged.
- **Metal — ⛔ FAILED (Peter-attended, Orin, 2026-07-05, 5 boots):** the boot core **dark-hangs at the
  EL2→EL1 drop.** JM3/JM4 + the heap init all run on silicon (every line through `:: tegra: JM6 —
  dropping … ::` prints), then dark. Localized: the `eret` reaches EL1 (no `VBAR_EL2` illegal-return
  fault), but the **first EL1 instruction fetch aborts** — a `VBAR_EL1` fault vector *and* a raw-UARTC
  sentinel stub armed before the eret both stayed dark, so `.text` is unexecutable at EL1 the instant
  the drop lands. Monitor-independent; `SCTLR_EL1`-independent (the `mmu_tegra` RMW pattern didn't help).
  Needs a dedicated investigation (see `arch_arm64.md` §3 JM6 result for the plan), NOT blind reboots.
  Captures `target/serial-orin-jm6-FAIL{,2,3,4}-*.log`.
- **Honest scope:** the reused EL2-built `L1` is correct for a kernel-only (no-EL0) core but not
  EL1-precise (RAM reads EL0-accessible via AP[1]=1; the device window is nominally EL1-executable
  though no code branches there) — an EL1-precise map is worth building only once EL0 runs on Orin.
  EL0-on-Orin and EL1 timer preemption (needs EL1-non-banking vectors) are follow-on arcs.
- **Commit:** this arc on `hw-jetson` (merge pending the integrator, who records the merge hash).

---

## hw-rmbp track — 2026-07-04 (landed on `hw-rmbp`, awaiting integration)

### U3.5 — preemptible ring 3 (x86) ✅ `hw-rmbp`
- **What:** completes the U3 process abstraction — a ring-3 task can now be dropped
  **preemptible** (`Task.preemptible` sets `RFLAGS.IF` in the `iretq` frame), so the
  local-APIC timer evicts it and other work shares its core. This closes the one-core
  DoS a program that never syscalls (`jmp $`) was. The x86 twin of aarch64 M6e. The
  timer ISR conditionally `swapgs`es on a CPL-3 entry so the scheduler sees the kernel
  per-CPU block, and the per-process **CR3 install moved from the trampoline into the
  scheduler DISPATCH path** so it covers a resumed-after-preemption task (which never
  re-enters the trampoline) as well as first entry. The full user register file is
  saved/restored across preempt/resume by the existing `x86-interrupt` + `switch_context`
  machinery. The cooperative fixtures (U1a/U1b/U2/U2.5/U3) stay `preemptible=false` — IF
  clear, never preempted — so they are byte-identical.
- **Tested — QEMU:** `./arroyo test 25` → `:: U3.5: ring-3 preemption — IRQs-at-ring3=160,
  co-task ran, spinner resumed -> PASS ::` (a preemptible spinner is preempted 160×, a
  kernel co-task on the same core runs to completion = the DoS fix, the spinner's
  private-CR3 counter climbs across preemptions = correct resume, and a watchdog reaps it
  via a scheduler `KillSwitch`). U1a/U1b/U2/U2.5/U3 byte-identical (only the U1a shared-blob
  size and 2 new U3.5 lines differ); coexists with the U2 disk loader (`UNAOS_FATIMG=1` →
  `hello from disk`), the U2.5 FTDI console (`UNAOS_USBSERIAL=1`), and the FAT regression
  (`test-fat part`/`sf`). `./arroyo check` both arches; 0 aarch64 files. Multi-lens
  adversarial review before commit.
- **Honest scope:** per-task opt-in (only the spinner is preemptible); FP/SIMD is NOT saved
  across preemption yet (no FXSAVE/FXRSTOR — the register-only spinner is safe); one user
  task per core (RSP0/`syscall_kernel_rsp` set at first entry only); no PCID.
- **Metal — ★ CONFIRMED (real 2012 rMBP, 2026-07-04, bootlog photo):** `:: SMEP on ::` (real
  supervisor-execute protection active while the preemptible spinner ran) then `:: U3.5: ring-3
  preemption — IRQs-at-ring3=156, co-task ran, spinner resumed -> PASS ::` — the real timer preempted
  the ring-3 spinner 156× and the swapgs-on-ring3-timer + CR3-at-dispatch + reap ran correctly on Ivy
  Bridge, every prior fixture PASS (U1a/U1b/U2-0a/U3 byte-consistent), 0 unexpected faults. The same
  boot also confirmed the U2.5 APIC ms-clock fix on metal: `initcnt=6236 [1 kHz calibrated]`,
  `ms-clock 999 Hz` (the old ~119 Hz reading is gone).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

---

## hw-pi4 track — 2026-07-04 (landed on `hw-pi4`, awaiting integration)

### U6b — real File handles: `SYS_OPEN`/`SYS_READ` routed through the object table via `File` + `CAP_READ` (aarch64) ✅ `hw-pi4`
- **What:** makes U6a's `File` **scaffold** real — the first resource syscall routed through a **non-Console**
  object, and the direct precursor to UnaFS grants (a program opening a disk file under a capability).
  **`SYS_OPEN = 11`**`(name_ptr, name_len)` → `copy_from_user`s the bounded 8.3 name, mounts the single
  read-only FAT volume, finds the top-level entry, records a per-task **open-file descriptor** and installs a
  `File` handle carrying `CAP_READ`; **`SYS_READ = 12`**`(handle, buf, len)` → the CHECK
  `handle_resolve(asid, handle, CAP_READ)` must yield a `File` (a missing right, a non-File kind, or no handle
  all give `-EACCES` — the twin of `sys_write`'s Console+`CAP_WRITE`), then it clamps to `min(len, size-offset)`,
  validates the destination (`user_range_ok(.., writable=true)` — a bad buffer is `-EFAULT` with no read and no
  offset move), reads through a new read-only **offset-aware** FAT reader (`fat::read_at`), `copy_to_user`s, and
  advances the descriptor's offset by the count delivered (`0` = EOF; sequential, no seek). The descriptor lives
  in a small **per-task FILES table** — parallel atomic arrays (`FILE_USED`/`FILE_CLUSTER`/`FILE_SIZE`/
  `FILE_OFFSET`, keyed `[asid][idx]`, `NFILE = 4`), the same lock-free sidecar shape as `HANDLE_RIGHTS`/
  `HANDLE_KIND`; the `File` handle's value word carries the **file-id = descriptor index + 1** (the `+1` bias
  keeps it clear of the value word's `0`/`u64::MAX` sentinels, structurally). **Teardown-clear** extends U5's
  discipline to files: `clear_handle_row` now also clears the FILES row, so a reused ASID starts with no stale
  file, no leaked offset, no aliasable descriptor. **Scope by design:** read-only, flat root, one FAT volume, no
  write/create/delete, no seek, no directory ops, no second mount — `SYS_OPEN` is the hook a later arc's UnaFS
  `owner`/`grants:*` enforcement rides. Lane: `arch/aarch64/syscall.rs` (the FILES table, `SYS_OPEN`/`SYS_READ`,
  the teardown-clear extension, the demo) + a `main.rs` launcher + `fs/fat.rs` (a read-only `read_at` +
  `first_cluster()` getter; `read_file` left **byte-identical** for its M6g/U4 caller); no scheduler primitive,
  no `boot.rs` change (the teardown-clear folds into `clear_handle_row`), no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test` → after the U6 PASS line: the U6b setup line and `:: U6b: real
  File handles — open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::`. The
  `el0-u6bfile` fixture opens `HELLO.BIN`, reads its first 16 bytes through the returned `File` capability and
  verifies they equal the kernel-planted on-disk prefix (`USER_BLOB[..16]` — `HELLO.BIN` on the media *is*
  `USER_BLOB`), then proves the CHECK denies both a **present File lacking `CAP_READ`** (the rights arm) and a
  **`Socket` carrying `CAP_READ`** (the kind arm, `SYS_READ` serves `File` only) with `-EACCES` — witness `0x1F`
  (all five bits). The launcher additionally proves the **file-row teardown-clear** kernel-side (the fixture
  exits holding two live descriptors — its own open + a pre-endowed no-cap File — so `files_row_is_clear`
  transitions false→true when its slot retires). Every M6b/M6d/M6e/M6f/M6g/U4/U5/U6 verdict line
  **byte-identical** (the shared FAT mount does not regress M6g/U4 — both still PASS their disk loads) and
  CAPSTONE 6/6, 0 unexpected faults. `check` both arches; x86 `test` MISSION SUCCESS; aarch64 virt v2 clean USB
  enumeration + GICv3 CAPSTONE 6/6 unchanged; zero x86 files.
- **Tested — metal:** ✅ **METAL-CONFIRMED on the real Pi 4 (2026-07-06)** — Peter booted (non-destructive
  `kernel8.img` swap on the mounted FAT; `HELLO.BIN` was already byte-identical to this build, so the
  bytes-match test is exact), I ran the Debug-Probe serial bridge. On silicon: `:: U6b: real File handles —
  open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::` — the fixture opened
  `HELLO.BIN` and read it through a `File` capability off the **real EMMC2/SD card** (the metal-only EMMC2-first
  leg `SD card @0xfe340000 identified — 31116288 blocks (15193 MiB, CSD v2)`, which QEMU cannot exercise), and
  both `-EACCES` denials held. `EL=1`/`CNTFRQ=54 MHz`, EL0 preempt live (`M6e IRQs-taken-at-EL0=23`,
  `M6f spsentinel=2`), full battery green (M6b `exited=1 killed=3`, M6d ×3, M6f ×3, M6g/U4/U5/U6 PASS), 0
  unexpected faults. (The scheduler CAPSTONE demo sat out this particular boot — only 3 of 4 cores came online,
  and it needs the full 4; that is a known metal SMP AP-bring-up variance in the scheduler track, orthogonal to
  U6b's pure-syscall logic.) Metal log `unaos/target/serial-pi.u6b-metal.log`.
- **Deferred:** file **writes**/create/delete, **seek**/`lseek`, **directory** ops (the natural extensions once
  read-only File handles are proven); real **`Socket`** handles (net syscalls — the fs twin of this arc); UnaFS
  `owner`/`grants:*` checked on `SYS_OPEN` (rides the kernel UnaFS mount, K2/K3); a second mount / media detect.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U6a — the general object table: `(kind, target, rights)` descriptors, first-free for ALL kinds, the `CONSOLE_FD` collision closed (aarch64) ✅ `hw-pi4`
- **What:** generalizes U5's fixed-shape handle into a general **object descriptor**. A handle now
  names one of four **kinds** — `Child(pid)` (U4), `Console` (U5), and the **scaffolds** `File(id)` /
  `Socket(id)` (defined + resolvable via `handle_resolve`, but no fs/net syscall routes through them
  yet — they prove the table is genuinely general, not that fs/net exists). The **kind rides in a
  parallel sidecar** `HANDLE_KIND[[AtomicU8; 8]; USER_SLOTS+1]` (keyed identically to `HANDLES`/
  `HANDLE_RIGHTS`), so the value word keeps U4/U5's sentinels **byte-identical** — `0` = Empty (the
  lock-free allocator's free marker), `u64::MAX` = `RESERVING` — and nothing else is reserved. Picking
  the sidecar over the value word's high bits makes the sentinel-collision STOP tripwire *structurally
  impossible* (a `File`/`Socket` id may be any non-`0`/non-`u64::MAX` word, no masking) and mirrors the
  rights sidecar 1:1 (kind + rights published Release BEFORE the live value; single-writer-per-ASID the
  backstop). **The `CONSOLE_FD` collision is closed** (the arc's raison d'être + U5's one design note):
  U5 pinned the console cap at a fixed index via an unconditional store while `handle_install`'s
  first-free scan allocated from index 0, so a process that both PRINTED and SPAWNED 2+ children could
  auto-allocate a child onto index 1 and have it clobbered by the console install. U6 makes `CONSOLE_FD`
  a **reserved index the first-free allocator SKIPS**: the console lives there by the `fd=1`/stdout
  convention (keeping every prior blob byte-identical), children/objects fill `{0, 2, 3, ..}`, so a
  console cap and N child/object caps coexist with **zero index collision for any interleaving of
  installs**. Every consumer is behaviour-preserved: `handle_resolve` dispatches on the kind sidecar
  (`sys_wait`→`Child`, `sys_write`→`Console`+`CAP_WRITE`, `sys_cap` grant/revoke on any kind); the
  **attenuation invariant is unchanged** and the mint copies the source's kind (attenuate rights, never
  re-kind); `handle_clear`/`clear_handle_row`/`handle_row_is_clear` also handle the kind. Lane:
  `arch/aarch64/syscall.rs` (the descriptor, reserved-index alloc, resolve, kind scaffold, the demo) +
  a `main.rs` launcher; no `boot.rs` change (`clear_handle_row` already wipes the whole row), no
  scheduler primitive, no driver, no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test` → after the U5 PASS line: the U6 setup line, `u6: parent
  print (pre-spawn)`, `u6: parent print (post-spawn; console survived 2 spawns)`, the two children's
  `hello from EL0`, and `:: U6: general object table — printing spawner + 2 children, no index
  collision, File/Socket kinds resolve -> PASS ::`. The `el0-u6spawn` fixture is the printing spawner U5
  could not serve: it prints, spawns 2 children (distinct auto-allocated handles, both `!= CONSOLE_FD`),
  prints AGAIN (the console cap survived the spawns), and reaps both by handle — witness `0xF`. The
  launcher additionally proves kernel-side that the `File`/`Socket` kinds resolve to their kind with the
  required right (and `Denied`/`-EACCES`-equivalent without) and that the exact U5-breaking
  console-vs-two-children interleaving no longer collides (`u6_kernel_check`). Every
  M6b/M6d/M6e/M6f/M6g/U4/U5 verdict line **byte-identical** (sorted set-diff: the only delta is
  `VBAR_EL1` shifting one page — benign binary growth from the added code — plus the new U6 lines) and
  CAPSTONE 6/6, 0 unexpected faults. `check` both arches; x86 `test` MISSION SUCCESS; aarch64 virt v2
  clean USB enumeration + GICv3 CAPSTONE 6/6 unchanged; zero x86 files.
- **Tested — metal:** none this arc — U6 is fully QEMU-verifiable (descriptor/allocator/resolve logic;
  the demo's two children ride U4/M7's already-metal-confirmed EMMC2 load path).
- **Deferred:** **U7** — cross-process handle-transfer (`SYS_XFER`, a cross-ASID write discipline that
  breaks the single-writer invariant) + revocation trees (`CAP_REVOKE`, reserved) + the bandy Ring-3
  delegation wrapper; and a later arc — real `File`/`Socket` fs/net syscalls routing through these
  kinds, plus UnaFS `owner`/`grants:*` enforcement on `SYS_OPEN` (rides the kernel UnaFS mount).
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U5 — handles as capabilities: the enforcement CHECK + grant/attenuate/revoke + routed `sys_write` + teardown-clear (aarch64) ✅ `hw-pi4`
- **What:** turns U4's handle STRUCTURE into a real **capability**. A handle now carries
  **rights** — a bitmask `CAP_READ|CAP_WRITE|CAP_EXEC|CAP_GRANT|CAP_REVOKE` in a **sidecar**
  `HANDLE_RIGHTS[[AtomicU32; 8]; USER_SLOTS+1]` keyed identically to `HANDLES`, so U4's
  `0`/`RESERVING` value-word sentinel logic stays byte-unperturbed — and names a **target**
  beyond "child pid" (a well-known `HANDLE_CONSOLE = u64::MAX-1` token; two kinds only,
  `CHILD(pid)` and `CONSOLE`, not a general object table — that is U6). The **CHECK** is a
  single `handle_resolve(asid, idx, req_rights)` at the one lookup point every handle-consuming
  path goes through: out-of-range/Empty ⇒ the caller's own errno (`sys_wait` → `-ECHILD`, U4's
  structural ownership preserved; `sys_write`/`SYS_CAP` → `-EACCES`), missing-a-right ⇒
  `-EACCES`. **`SYS_CAP` (10)** adds grant/attenuate/revoke: GRANT mints a new handle to the
  same target carrying a rights mask that must be a **subset** of the granter's rights on the
  source — the **attenuation (monotonic-decrease) invariant**, `req & !src_rights != 0` ⇒
  `-EACCES` (a grant can never amplify), requiring `CAP_GRANT` on the source; REVOKE drops a
  handle the caller owns (subsequent use ⇒ `-EACCES`/`-ECHILD`). **`sys_write` routes through
  the table** — `fd` is a handle index that must resolve to a `CONSOLE` handle with
  `CAP_WRITE`; no ambient stdout. Every printing EL0 process is **endowed** a `CONSOLE`+
  `CAP_WRITE` cap at `CONSOLE_FD = 1` at spawn/launch (shared window ASID 0 for `el0-hello`;
  each M6f/M6g/U4-child slot), and the `copy_from_user`/all-or-nothing `-EFAULT` path is
  byte-identical (the M6f hostile fixture holds the cap, so its bad-pointer writes still
  `-EFAULT`, not `-EACCES`). **Teardown-clear** folds U4's one deferred note:
  `boot::teardown_user_slot` wipes the whole `HANDLES[asid]` row + rights **before** releasing
  the slot's used-flag (not after — a post-release clear could race a concurrent
  `alloc_user_slot` on another core reclaiming the ASID), so no capability outlives its ASID.
  Lane: `arch/aarch64/syscall.rs` + a `boot.rs` row-clear (in `teardown_user_slot`) + a
  `main.rs` launcher; no scheduler primitive, no driver, no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test 8` → after the U4 PASS line: the U5 setup line,
  `u5: cap write` **twice** (the write-cap write + the write through the minted attenuated
  cap), and `:: U5: capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded,
  revoke enforced, teardown-clear clean -> PASS ::`. The `el0-u5cap` fixture proves four
  EL0-observable behaviours against its own table (write-cap OK; a write-less cap → `-EACCES`;
  a grant exceeding the granter's rights rejected while a subset grant works and its handle
  writes; a revoked handle → `-EACCES`) via a witness bitmask (`0xF`), and the launcher proves
  the fifth kernel-side (the fixture's handle row is clear after its slot teardown). Every
  M6b/M6d/M6e/M6f/M6g/U4 line byte-identical (hex/pid-normalized set-diff: only the four new U5
  lines added; all four prior `hello from EL0` land, the M6f `4 hostile … EFAULT` PASS holds,
  U4 PASS holds, CAPSTONE 6/6) and 0 unexpected faults. `check` both arches; x86 `test` MISSION
  SUCCESS; aarch64 virt v2 MISSION SUCCESS + GICv3 JC3 SMP 3/3 + CAPSTONE 6/6 unchanged.
- **Tested — metal:** none this arc — U5 is fully QEMU-verifiable (the checks/grants/revokes
  are pure syscall logic; the reap wake is a scheduler post; the child disk-loads ride
  U4/M7's already-metal-confirmed EMMC2 path). A future reflash would re-exercise the endowed
  prints off the real card, but nothing in U5 is metal-*gated*.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U4 — the process model + per-process handle table (aarch64) ✅ `hw-pi4`
- **What:** the ownership half of the process model — `sys_spawn` now returns a **handle**
  into the *caller's* per-process handle table (not a raw pid) and `sys_wait` takes that
  handle, so reaping is **structurally owner-scoped**: a task can only reap children whose
  handles are in ITS table (folding M7's review note — any task could `sys_wait` any pid —
  by construction). The table is a static, const-init `HANDLES[[AtomicU64; 8]; USER_SLOTS+1]`
  keyed by the caller's **ASID** (read from `TTBR0_EL1[63:48]` synchronously in the SVC
  handler); `PROCS` stays keyed by pid (exit-accounting control blocks) while `HANDLES` is
  keyed by ASID (the spawner's private namespace of child capabilities) — deliberately
  separate. No new syscall number, no new scheduler primitive, no driver, no boot change:
  the whole arc is `arch/aarch64/syscall.rs` + one `main.rs` launcher tweak. `sys_write`
  stays the `fd==1` path (routing a resource syscall through a handle is U5, when there is a
  capability *check* to add). This is the exact substrate U5 turns into capabilities (grant =
  transfer a handle, revoke = clear it; U5 adds the check at this same handle lookup).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → in place of the M7 line: the U4 setup line,
  **four** `hello from EL0` (M6c inline #1, M6g loader #2, the two U4 children #3/#4), and
  `:: U4: process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD
  (per-process handle tables) -> PASS ::`. The demo: a parent (`el0-u4parent`) `sys_spawn`s
  two children and reaps **both by handle**; an ownership negative (`el0-u4orphan`, its own
  slot/ASID) calls `sys_wait(0)` on an Empty handle and gets `-ECHILD`. Every
  M6b/M6c/M6d/M6e/M6f/M6g + CAPSTONE line byte-identical (hex/pid-normalized set-diff: only
  the M7 line → the U4 line, `hello` 3→4) and 0 unexpected faults. x86 (`test` +
  `UNAOS_FATIMG=1 test`) functionally byte-identical through the U-lines (seam is
  `baremetal`-only; sole diff is a QEMU timer/scheduling jitter on the async U2 exit at the
  25 s window boundary — reliably present with a longer window; U3/U3.5 untouched); `check`
  both arches; aarch64 virt v2 + GICv3 JM5 SMP 3/3 unchanged.
- **Tested — metal:** none this arc — U4 is fully QEMU-verifiable (every piece is
  scheduler/syscall logic: the handle install/resolve/clear, the owner-scoped reap, the
  two-child spawn/reap, and the `-ECHILD` negative are all deterministic under QEMU raspi4b —
  the reap wake is a scheduler post, not the timer). The child disk-loads ride the same EMMC2
  path M7 already metal-confirmed; a future reflash would show the two extra `hello from EL0`
  off the real card, but nothing in U4 is metal-*gated*.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### M7 — a minimal process model: sys_spawn + sys_wait (aarch64) ✅ `hw-pi4`
- **What:** the reaping half of a process model — an EL0 program can now spawn a child
  program and reap it. **`SYS_SPAWN` (8)** loads the fixed on-disk `HELLO.BIN` into a
  fresh per-task slot and runs it at EL0 as a *child*, returning its pid; **`SYS_WAIT`
  (9)** blocks the caller until that child exits and returns its exit status. A small
  static process table (cap 4) carries, per child, a counting `Semaphore` the child
  posts once (on exit *or* kill) and the parent waits once — a **scheduler** wake, so
  the reap is QEMU-testable, not timer-gated. The child's disk load reuses the M6g
  loader core, refactored into a shared, silent `load_program_into_slot()` (the M6g
  loader reconstructs its own lines from the result — its output stays byte-identical).
  The pid-recording race is closed by a co-location invariant (child queued on the
  caller's core, undispatchable until the parent yields in `sys_wait`), so **no
  scheduler change** was needed. The Pi pioneers roadmap-U4, as it did M6a–M6g.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the M6g lines: `:: M7: process
  model — sys_spawn + sys_wait (parent reaps a disk-loaded child) ::`, a **third** `hello
  from EL0` (the M7 child), `:: M7: parent spawned child pid=<p>, waited, child exited
  status 0 -> PASS ::`, with every M6b/M6c/M6d/M6e/M6f/M6g + CAPSTONE line byte-identical
  (hex/pid-normalized set-diff: only the two new M7 marker lines + `hello` 2→3) and 0
  unexpected faults. x86 (`test` + `UNAOS_FATIMG=1 test`) functionally byte-identical
  through the U-lines (seam is `baremetal`-only; sole diffs are QEMU timer-calibration
  jitter); `check` both arches; aarch64 virt v2 + GICv3 JC2 SMP 3/3 unchanged.
- **Tested — metal (real Pi 4, 2026-07-04):** ★ PASS on silicon. `:: M7: parent spawned
  child pid=41, waited, child exited status 0 -> PASS ::` — the parent `sys_spawn`ed a child
  that loaded `HELLO.BIN` off the **real** card via the EMMC2-first path QEMU cannot exercise
  (`SD card @0xfe340000 — 31116288 blocks (15193 MiB, CSD v2)`), printed the **third** `hello
  from EL0`, and exited status 0; the parent's blocking `sys_wait` was woken by the child's
  scheduler post and reaped it — all under a live timer (EL0 preemption live: M6e
  `IRQs-taken-at-EL0=23`, M6f `spsentinel=3`). Full battery green on metal: M6b `exited=1
  killed=3 PASS`, M6d ×3, M6f ×3, M6g `disk-loaded EL0 program exited ok -> PASS`, CAPSTONE
  6/6, EL=1/CNTFRQ=54 MHz, **0 unexpected faults, 0 FAIL lines**. (Prepped non-destructively by
  swapping `kernel8.img` on the mounted FAT volume — no re-flash needed; the gate-verified
  binary.) Log: `target/serial-pi.m7-metal.log`.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

## Round 6 — 2026-07-04 (landed on track branches; awaiting integration)

### U3 — per-process address spaces (CR3) (x86) 🔬 `hw-rmbp`
- **What:** each ring-3 process now runs in its OWN top-level page table (its own
  CR3) instead of sharing one user window — the x86 mirror of aarch64 M6d. A static
  8-slot pool of page tables each SHARES the kernel half (copies every kernel PML4
  entry except the user-window slot, so the identity map / MMIO / heap / kernel
  stacks are shared) and owns a PRIVATE user window at USER_BASE. The scheduler
  installs a task's CR3 before dropping to ring 3 and restores the kernel CR3 +
  frees the slot on exit. Two processes can map the same address to different
  memory. Plain `mov cr3` (full TLB flush) — PCID (the x86 ASID analogue) deferred.
- **Tested — QEMU:** `./arroyo test 25` → a deterministic kernel isolation probe
  (two spaces, same VA, distinct sentinels, swap CR3 and read → distinct) PASS, and
  two ring-3 tasks each in their own CR3 each read their own private sentinel PASS;
  U1a/U1b/U2/U2.5 byte-identical, no reboot loop; coexists with the U2 disk loader
  (`UNAOS_FATIMG=1`), the U2.5 FTDI console (`UNAOS_USBSERIAL=1`), and the FAT
  regression (`test-fat part`/`sf`). `./arroyo check` both arches; 0 aarch64 files.
  4-lens adversarial review → 0 confirmed findings.
- **Metal — PENDING:** rides the next rMBP reflash (FTDI cable day, ~2026-07-08).
- **Commits:** on `hw-rmbp` (see landing report); unmerged (Fable credits out).

---

## Round 5 — 2026-07-03

### M6g — load a program from storage (aarch64) ✅ `hw-pi4`
- **What:** the Pi twin of x86 U2 — the first *program loaded from the microSD the
  Pi booted from* into the EL0 boundary. A block-layer backend seam lets the
  read-only path dispatch to a new BCM2711 EMMC2/SDHCI microSD driver (PIO,
  single-block CMD17, polled, no writes) beside the untouched xHCI path; the driver
  probes **EMMC2 first, legacy Arasan second** (the reverse of QEMU, so the metal
  base is the first tried). The loader mounts the card's FAT volume, reads
  `HELLO.BIN`, size-checks it, copies it into a fresh per-task M6d slot (EL0-RX/EL1-RO
  before the task exists), and runs it at EL0 (`hello from EL0`). The loaded bytes are
  untrusted — bounded only by size, contained by EL0 + per-page perms + the M6b
  fault-kill net.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → `SD card @0xfe300000 identified —
  131072 blocks (64 MiB, CSD v1)`, `FAT mounted from SD (Fat32)`, `HELLO.BIN loaded
  from SD (51 bytes) -> EL0`, second `hello from EL0`, `disk-loaded EL0 program exited
  ok -> PASS`, with every prior milestone byte-identical and 0 unexpected faults; the
  `UNAOS_SDIMG=0` no-SD control adds exactly the two no-card lines + the loader-skipped
  line. x86 (`test` + `UNAOS_FATIMG=1 test`) byte-identical (seam inert); `check` both
  arches; aarch64 virt v2 + GICv3 JC2 SMP 3/3 unchanged.
- **Tested — metal (real Pi 4, 2026-07-04):** ★ the driver's EMMC2-first success leg —
  the one QEMU physically cannot exercise — ran on silicon: **no fallback line**, `SD card
  @0xfe340000 identified — 31116288 blocks (15193 MiB, CSD v2)` (the real ~16 GB microSD,
  SDHC/block-addressed — vs QEMU's 64 MiB/CSD v1 legacy fallback), then `FAT mounted from
  SD (Fat32)`, `HELLO.BIN loaded from SD (51 bytes) -> EL0`, second `hello from EL0`,
  `disk-loaded EL0 program exited ok -> PASS`. This reflash also carried M6f's pending
  metal: all three M6f verdicts PASS and the per-task preempt rider went > 0 on silicon
  (`spsentinel=3`); M6b `exited=1 killed=3 PASS`, M6d ×3 PASS, M6e `IRQs-taken-at-EL0=26`,
  CAPSTONE 6/6, 0 unexpected faults. EL=1, CNTFRQ=54 MHz.
- **Commit:** `faad571` (merge) · arcs `11b8191` `a072a48` `683d48c`.

### U2.5 — FTDI USB-serial console (x86) 🔬 `hw-rmbp`
- **What:** a captured console for the serial-less 2012 rMBP. The kernel
  enumerates an FTDI FT232 (0403:6001) on the xHCI bus, configures it
  (115200 8N1), and drains a 64 KiB boot-capture ring — fed by every
  `serial_print!` since the first — out its bulk-OUT endpoint, so the whole early
  boot log replays out the cable. Also folds three U2-review hardening items
  (first-entry x87/MMX scrub, per-CPU `DR7` clear, whole-ring-3-window zero before
  a load) and fixes the APIC ms-clock: the BSP heartbeat now re-arms *after* the
  calibrated rate is stored (it was pinned to the fixed fallback → metal read
  ~119 Hz instead of ~1000).
- **Tested — QEMU:** `UNAOS_USBSERIAL=1 ./arroyo test 25` → `FTDI USB-SERIAL
  DETECTED (0403:6001)`, `FTDI console up`, `FTDI TX mirror -> PASS (~17 KB
  replayed)`, `target/ftdi.log` carries the boot log; coexists with the U2 disk
  loader (`UNAOS_USBSERIAL=1 UNAOS_FATIMG=1` → both PASS) and the usbdebug view.
  No-knob boot log byte-identical but for the `DR7 cleared` line and the APIC
  re-arm. FAT regression (`test-fat part`/`sf`) green; `./arroyo check` both arches.
- **Metal — PENDING (~2026-07-08):** rides the physical FTDI cable (B0CJVC19CF).
  QEMU-only until then; the APIC ~1000 Hz truth and the FTDI console verify on the
  real rMBP on cable day (`UNAOS_USBDEBUG=1` boot, FTDI in a root USB-A port).
- **Commits:** `229d675` (Part 0 folds) · `ab0f975` (APIC re-arm) · `f7c929e` (FTDI console).

### U2 — loadable ring-3 programs from FAT (x86) ✅ `hw-rmbp`
- **What:** the first *real program loaded from disk* into the x86 privilege
  boundary. A flat ring-3 binary (`HELLO.BIN`) is read off a FAT volume,
  validated, copied read-only-from-start into the user code page, and run in
  ring 3 (`hello from disk`). Plus the boundary preconditions that make loading
  untrusted code safe: #DB and #MC on dedicated interrupt stacks (closing a
  user-triggerable CPU-halt), a register scrub at first entry, and self-test
  fixtures for the NMI stack and the CVE-2012-0217 guard.
- **Tested — QEMU:** `UNAOS_FATIMG=1 ./arroyo test 25` across all four FAT
  layouts → the three U2 lines + the Part-0 fixtures + U1a/U1b still PASS, full
  USB boot, 0 unexpected faults. `./arroyo check` both arches.
- **Tested — metal (real 2012 MacBook Pro, 2026-07-03):** Realtek USB3 SD reader
  → FAT16 card → `HELLO.BIN` (72 B) → `hello from disk` PASS. *Metal-pending:*
  the #DB-resume path (TCG can't model the single-step-on-SYSCALL trap) and the
  #MC fire path.
- **Commit:** `9cdf397` (merge) · arc `7d8a6bb`.

### M6f — validated user pointers + wider syscalls (aarch64) ✅ `hw-pi4`
- **What:** `copy_from_user`/`copy_to_user` (bounds- and overflow-checked; a bad
  pointer is an error return, not a task kill) plus the first "real" syscalls
  (`yield`, `sleep_ms`, `getpid`, `getinfo`). Also folds five hardening items
  from the M6d review, incl. scrubbing the FP/SIMD registers at first entry.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → the M6f fixtures PASS
  (getinfo round-trip; four hostile pointers all refused with no kills;
  yield/sleep interleave) with every prior milestone still green.
- **Tested — metal (real Pi 4, 2026-07-04, on the M6g reflash):** all three M6f
  verdicts PASS on silicon (getinfo/copy_to_user round-trip, 4 hostile pointers
  refused with 0 kills, yield/sleep interleave) and the per-task EL0 preempt rider
  went > 0 (`spsentinel=3`, QEMU shows all 0) — the timer preempted that slot task
  and it resumed correctly under its own ASID.
- **Commit:** `ee21e30` (merge) · arcs `71ed153` + `e65ffc0`.

### JM2 — Orin headless first light (aarch64/Jetson) 🔬 `hw-jetson`
- **What:** makes the Jetson build boot headless and safe. Gates the QEMU-virt
  SMP path off the Tegra build (it would otherwise touch Tegra memory that
  isn't there), makes the bootloader boot without a display (the shared
  bootloader — the MacBook boots through it too), and adds a boot-diagnostics
  knob that reports the firmware's real serial/display configuration.
- **Tested — QEMU:** full battery byte-stable — `./arroyo check` both arches;
  x86 U1a/U1b PASS; aarch64 virt v2 + GICv3 SMP lines intact; Pi `kernel8-test`
  unchanged. The Tegra feature is off in all of these, so nothing regresses.
- **Tested — metal (real Orin Nano, 2026-07-03):** the boot diagnostics ran on
  silicon (genuinely headless firmware: 0 display handles) and the headless
  path **entered the kernel for the first time on Orin** — which then faulted
  on its first Tegra UART register read because the firmware-handoff page
  tables don't map Tegra device memory. That diagnosis is the next arc (JM3:
  kernel-owned MMU).
- **Held at integration review:** the merge is waiting on a small must-fix —
  the boot-diagnostics DTB table scan compares against a wrong GUID constant
  (it can never match), so the captured "firmware publishes no DTB" line is
  withdrawn as unverified until a re-run with the corrected constant. Fix
  rides at the head of the JM3 arc.
- **Commit:** *(merge pending the fix)* · arcs `811259c` `d382677` `0bd0dae`
  `d0835c0` `27bf835`.

---

## Round 4 — 2026-07-02

### M6d — per-task address spaces + ASIDs (aarch64) ✅ `hw-pi4`
- **What:** every user task gets its own isolated page tables and its own stack,
  ASID-tagged so task switches need no TLB flush. This is what lets two programs
  use the same virtual address for different data — real process isolation.
- **Tested — metal (real Pi 4):** same-VA isolation proven distinct on real A72
  TLBs (QEMU can't test this — it has no TLB model), stack write/readback PASS,
  all under live timer preemption. `4a06a8c`.

### JC2 — PSCI SMP on GICv3 (aarch64/Jetson) 🔬 `hw-jetson`
- **What:** brings up secondary CPU cores on the QEMU-virt GICv3 path via PSCI,
  proven by cross-core interrupts (each core pinged, both directions).
- **Tested — QEMU:** `UNAOS_GICV3=1 ./arroyo test-arm 30` → 3 cores online +
  cross-core SGI both ways; v2 single-core and Pi SMP unchanged. Metal (Orin)
  pending. `18be259`.

## Round 3 — 2026-07-02

### U1b — ring-3 fault isolation + boundary hardening (x86) ✅ `hw-rmbp`
- **What:** a faulting ring-3 program is killed (kernel survives); plus register
  scrubbing, the CVE-2012-0217 guard, an NMI stack, and cross-core W^X.
- **Tested — metal (real 2012 MacBook Pro):** SMEP active, 3 fault-kills with
  correct syndromes, kernel alive past all three. `37d2af8`.

## Round 2 — 2026-07-02

### M6e — preemptible EL0 (aarch64) ✅ `hw-pi4`
- **What:** the timer can preempt a running user task and resume it correctly.
- **Tested — metal (real Pi 4):** 18 preemptions of a spinning EL0 task, all
  resumed correctly. `e62fd4c`.

## Round 1 — 2026-07-02

First rotation of the three-track machine: **U1a** (x86 ring-3 round-trip),
**M6c** (aarch64 loadable blob), **JC1** (GICv3 beside GICv2) — all landed,
reviewed, merged. `637ee5c`.
