# WHITE BOARD — GR14, 2026-08-03

**This is a whiteboard, not a record.** Wiped and rewritten whenever it changes. It carries one
thing: **what Peter needs to know or decide, right now.** Durable facts live in the baton; per-boot
status lives in `~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN — nothing needs a decision

No blocking questions. Two jobs in flight, both dispatched, neither needs input.

---

# THE FIND OF THE DAY — a kernel bug, not a program bug

The SYSCALL stub saves the ring-3 stack pointer into a **per-CPU** slot
(`PerCpuData::syscall_user_rsp`). `USER_RSP_OFFSET` is referenced in exactly one place and the
field has **no setter anywhere**; the scheduler's dispatch site installs CR3, `TSS.RSP0` and
`syscall_kernel_rsp` per task and not the user twin.

Any task that blocks *inside* a syscall — `SYS_YIELD` context-switches from within one, futex wait
likewise — leaves its user RSP in the shared slot, and the next task to `sysret` on that core runs
**on another task's stack**.

Caught on metal (s68): VUG's worker and parent aliased at `0x10(%rsp)`; a 32-bit `movl $0x0` in the
parent zeroed the low half of the worker's saved surface pointer — `USER_BASE+0x5000` → `USER_BASE`
exactly — and the worker wrote to its own read-only code page.

**How well it hid is the lesson.** It presented as a graphics bug in `render_band`, and the first
two hypotheses dispatched were both about the program. The only reason the corrupted value landed
on a recognisable constant instead of junk is that the aliasing store happened to be 32-bit.

Fox audited the aarch64 twin: **not present.** `__vec_svc` banks SP_EL0 onto the task's own kernel
stack, and its comment names this exact hazard as the reason. They hit the class at M6e and fixed
it structurally. The x86 fix converges on their design — `percpu.rs:46` currently calls the slot a
"scratch slot", which is the wrong mental model and is arguably why it survived.

---

# LANDED TODAY

**Merge closed.** Condition A met on both rigs at `bfa2c174`; trunk unified.

**Storage — S7 passes on metal for the first time ever**, and U10/U10c/U10d now run their real FAT
write-back leg. They had been "passing" in a silent in-memory fallback with no disk proof, because
the metal DATA volume never staged the fixtures the QEMU image did. Five fixtures now planted by
the builder, contents cross-checked against the witness constants.

**The sysret scrub, twice.** The kernel zeroes six GPRs on return to ring 3; the stubs declared
some of them. The first fix declared `r8/r9/r10` and left `rdi/rsi/rdx` open in the low-arity
stubs — its own commit message overclaimed, and it crashed metal. Now complete and proven by
disassembly, not assertion.

**Video re-land: M1, M2, M3 in.** Witness build repaired (trunk could not compile x86 under
`witness` at all). WC-BBSYNC re-landed — the `resync ABSENT` line is gone. FBCON-DMG substrate
landed behaviour-inert. **M4 (the banded present) is the last piece and is in flight.**

**Vocabulary.** ARM privilege terms out of x86 and arch-neutral files; `el0_input_*` →
`user_input_*` on both arches; `bg-el0` → `bg-user`. The one wire string a live gate matched was
changed with its spec lines in the same commit, and Fox boot-proved it: 91/91, 0 forbidden.

---

# THE PATTERN WORTH KEEPING

Six instruments this session looked authoritative and could not fail:

- a waker reported armed that was never started, then one that fired on every idle line
- a `strings` probe reading 0 because the tool was absent, and a second counting lines not registers
- a `pid=` echo for a command that failed with permission denied
- `cbw_fault=0` on the Pi with `n=0` — a counter that never counted (Fox's, same class)
- U9x/U10 "passing" in a fallback mode with no disk write-back
- byte-identity between two kernels, confounded entirely by an 8-char git-sha stamp

**The rule that came out of it:** a probe that cannot read zero proves nothing. Every check now
carries a positive AND a negative control, and a green banner is not evidence the code is
reachable — `strings` shows it is present, an executed witness on metal shows it runs.

---

# STANDING

- The card predates M2/M3 and both fixes. The next metal round wants a fresh build.
- 21 `wcg::` call sites in `wm.rs` are still aarch64-gated, so `[wc-k]` compiles on x86 and can
  never fire. Known, queued, real-risk against Pi semantics.
- On x86 a `bg`-launched VUG always reads `detached=false` (the `[0x20]` flags word is aarch64-only
  and stays zeroed here), so it takes the 300-frame cap — that path has never been exercised.
