# LINUXABI3 — SSE in ring 3 for Linux-ABI tasks, and the first SSE-compiled Linux code running (ledger B309, extends B286)

## Design
**Finding.** B286/LINUXABI2: "Ring 3 has no SSE enabled, which blocks busybox-class binaries." Read of the tree at b42b87cc: the kernel
target (`x86_64-unaos.json`) is `-mmx,-sse,…,+soft-float`, so no kernel code ever touches x87/XMM; CR4.OSFXSR is never set (nothing in
`arch/x86_64` writes it), so every SSE instruction in ring 3 is `#UD` → kill; there is no FXSAVE/FXRSTOR anywhere (SECURITY.md U2.5 Part 0-i:
first-entry x87 scrub only, "NO FXSAVE/FXRSTOR context switching"); vectors 16 (#MF) and 19 (#XM) have no IDT gate; the auxv carries
`AT_HWCAP=0` and no `AT_PLATFORM`. So BOTH halves were missing: SSE is fenced off for ring 3, and no XMM state is saved anywhere.

**Seam.** `CHARTER: Kernel — driver` (the compatibility box, `docs/dev/OS/03_COMPATIBILITY_BOX`). No handler owns CPU state; the work is
one new kernel file `arch/x86_64/linuxabi/fpu.rs` plus one hook line at each of the two scheduler sites LINUXABI2 already uses.

**Mechanism (eager FXSAVE, Linux tasks only, CR4.OSFXSR per dispatch).**
- Save format: `fxsave64`/`fxrstor64`, 512 B, 16-aligned, one slot per Linux process in a lock-free static table keyed by PML4 (the FS_TAB
  shape). Chosen over XSAVE because XCR0 = x87|SSE (no AVX: nothing saves YMM) holds exactly the state FXSAVE already covers, so XSAVE buys
  only the init/modified optimisation at the price of an XCR0 dependency; with CR4.OSXSAVE left clear, CPUID reports OSXSAVE=0, VEX/AVX is a
  clean `#UD` in ring 3 and libc dispatchers (glibc/musl ifuncs) select their SSE2 paths. Lazy CR0.TS/#NM rejected: the U2.5 first-entry
  `fninit`/`fldz` scrub runs at CPL 0 in the trampoline and would itself trap with TS set.
- Dispatch (`sched.rs` run loop, the `on_dispatch` site): incoming task named `linuxabi` with a private CR3 → per-core lazy init once
  (CR0: MP=1 NE=1 EM=0 TS=0; CR4.OSXMMEXCPT), set CR4.OSFXSR if this core's shadow says clear, `fxrstor64` its slot, mark the core "FP live
  for slot k". Incoming RING-3 non-Linux task while the shadow says set → clear CR4.OSFXSR, so the UnaOS ring-3 programs keep their `#UD`
  fence on SSE exactly as before. Kernel tasks touch nothing (softfloat kernel). CR4 is written only on Linux↔UnaOS-ring-3 transitions.
- Switch-back (same run loop, first statement after the task returns to the scheduler: yield, preempt, block, exit): if the core is FP-live,
  `fxsave64` into the slot (key re-checked), then `fxrstor64` the Linux initial image (FCW 0x37F, MXCSR 0x1F80, all registers zero) — the
  scrub, so no Linux XMM/x87 residue reaches the next task. Across the SYSCALL stub nothing is needed: the kernel is softfloat, so a
  syscall that does not block leaves the user XMM file in the registers, and one that blocks goes through the switch-back above.
- fork: the parent is current in kernel context, so its live registers ARE its FP state: `fxsave64` straight into the child's slot.
  execve: `fxrstor64` the initial image into the live registers. Slot freed where FS_TAB is (`gc`, session end).
- `#XM` (19) and `#MF` (16) get IDT gates that kill a CPL-3 task like the other user-provokable vectors and stay fatal at CPL 0.
- auxv: `AT_HWCAP` = CPUID.1:EDX (the Linux x86 meaning; carries FXSR/SSE/SSE2 bits 24/25/26), `AT_PLATFORM` → "x86_64", `AT_HWCAP2` = 0.
- Signals: LINUXABI2 delivers none (`rt_sigaction`/`rt_sigprocmask` accepted, never delivered; kill ends the task) and there is no
  `rt_sigreturn`, so there is no signal frame to carry FP state. Owed with signal delivery.

**Milestones.** M1 `fpu.rs` + CR0/CR4 + the two scheduler hooks + #XM/#MF gates. M2 auxv HWCAP/PLATFORM, fork copies the FP slot, execve
resets it. M3 `SSE.LNX` (`crates/user-linux-hello/asm/sse.s`, staged as `APPS/SSE.LNX`): movaps/paddd/cvtsi2sd/cvttsd2si/cvtsd2si, MXCSR
initial value, a 64-byte SSE memcpy whose destination is the "sse ok" it prints, then two forks that each verify inherited xmm8..15, mutate
them, sleep (context switches), re-verify; the parent re-verifies its own xmm8..15 and MXCSR after reaping. The generated file runs on a
Linux host and prints `sse ok` (exit 0) — the fixture is checked against a real kernel before it meets ours. M4 `tests linuxabi3`.

**Witness.** `:: LINUXABI3: cr4=osfxsr save=fx sse_lnx=<ok|fail|skip> fork_fp=<ok|fail|skip> busybox=<ok|fail|skip> -> PASS|FAIL|SKIP ::`
No new knob — rides `UNAOS_LINUXABI=1`.

**Owed / not in this arc.** busybox: no musl toolchain in the container (`musl-gcc`/`x86_64-linux-musl-gcc` absent, no egress to
busybox.net/deb.debian.org), so `busybox=skip` unless an operator drops a static musl busybox at `unaos/target/BUSYBOX.LNX` (the builder
stages it as `APPS/BUSYBOX.LNX`; the fixture runs `ls /`). AVX/XSAVE (needs a 832+ byte area and XCR0.AVX). Signal frames with FP state.
The `tste`/`storm` delta is unmeasured (R78: no QEMU) — by construction a non-Linux dispatch adds one atomic load and a name compare only
when the incoming task has a private CR3; the metal boot's `tste`/`storm` lines against boot-19 are the measurement.

## Written
- `unaos/crates/kernel/src/arch/x86_64/linuxabi/fpu.rs` (new, `//! CHARTER: Kernel — driver`): slot table, `dispatch`/`switch_out` hooks,
  `fork_into`, `exec_reset`, `release`, `hwcap`, counters.
- `sched.rs`: one line after the `target_cr3` block (`fpu::dispatch(cpu, user_cr3, name, user_entry != 0)`), one line before
  "The task switched back to us" (`fpu::switch_out(cpu)`), both `cfg(feature = "linuxabi")`.
- `interrupts.rs`: `#MF` (16) and `#XM` (19) gates + handlers at the tail (unconditional: a missing gate was a #GP/#NP escalation anyway).
- `linuxabi/elf.rs` auxv (`AT_HWCAP`, `AT_HWCAP2`, `AT_PLATFORM`); `proc.rs` fork (`fork_into`, -EAGAIN when no slot), execve
  (`exec_reset`), `gc` (`release`); `mod.rs` session end (`release`), `selftest3`, the busybox argv[0] fix-up in the `linux` verb.
- `tests.rs`: `register("linuxabi3", …)` on the LINUXABI line. `builder/src/main.rs`: stages `SSE.LNX` and an operator-dropped `BUSYBOX.LNX`.
- `crates/user-linux-hello`: `asm/sse.s` + `SSE_CODE` (1135 bytes), writes `SSE.LNX` beside HELLO/PIPE/LS. Host cross-check:
  `as --64 asm/sse.s && ld -static` and the generator's `SSE.LNX` both print `sse ok` and exit 0 on the container's Linux 6.18 kernel.

**What a metal boot should print** (`UNAOS_LINUXABI=1` media from `./arroyo esp-x86`, desktop shell `tests linuxabi3`):
```
[linuxabi] load path=/apps/SSE.LNX segs=1 entry=0x400078 ...
sse ok
[linuxabi] fpu restores=<n> saves=<n> fork_copies=2 cr4_flips=<n> slots_in_use_after=0
:: LINUXABI3: cr4=osfxsr save=fx sse_lnx=ok fork_fp=ok busybox=skip -> PASS ::
```
On a failure SSE.LNX's exit code names the check: 11 paddd, 12/13 cvttsd2si/cvtsd2si, 14 initial MXCSR, 15 parent xmm8..15 after the
children, 16 MXCSR across the switches, 17 fork, 18/19 wait4, 21 a child's inherited xmm8..15, 22 a child's mutated xmm8..15 after its sleep
(the line `[linuxabi] SSE.LNX exit=<n> …` precedes the witness). Interactive: `linux /apps/SSE.LNX` prints `sse ok`.
`tests linuxabi` / `tests linuxabi2` are unchanged and must still PASS on the same boot (HELLO/PIPE/LS never touch XMM; their dispatches now
restore an initial image — a regression there is this arc's).
