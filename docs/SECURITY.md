# UnaOS Security — model, threat framing, hardening ledger

> Engineering companion to the design vision in
> [`dev/OS/04_SECURITY_IMMUNITY/`](dev/OS/04_SECURITY_IMMUNITY) — the
> permission model there ("no root; apps start with zero permissions; the OS
> passes single file handles, not folder access") is the direction; this
> document tracks the concrete threat model, the implementation chain, and the
> per-arch hardening state. Arc sequencing lives in [`ROADMAP.md`](ROADMAP.md).
>
> Rule: **an arc that lands a ledger item checks it off here in the same
> commit.**

## Threat framing

UnaOS is being built to hold *direct authority over physical hardware*:
G-code streams to a 3-D printer, PWM to a vehicle's throttle, GPIO, SDR. A
compromise is not just data loss — it has physical consequences. Security is
therefore foundation work, not a retrofit (BeOS deferred multi-user and paid
for it; we won't repeat that).

Exposure classes, honestly stated:

1. **Ring-0-resident parsers.** The network stack (Ethernet/ARP/IP/TCP/DHCP),
   USB descriptor parsing, and the FAT reader all parse attacker-influenceable
   bytes with full kernel privilege on both architectures. Until the privilege
   boundary exists everywhere, these are the practical attack surface, and
   they remain kernel-resident even after.
2. **Privilege boundary not yet load-bearing (both arches).** The ring-3/EL0
   boundary now exists on BOTH architectures (x86 U1a/U1b: ring-3 round-trip +
   per-page perms + fault→task-kill; aarch64 M6a/M6b, the pioneer), but it is
   still exercised only by the in-kernel demo — real programs (U2+) do not run
   in it yet, and the kernel's own subsystems (drivers, FS, net) remain ring-0.
3. **No identity or authorization layer yet** — no principals, no grants;
   any code that runs can do anything. This is what the U-chain builds.

## The model: capabilities first, POSIX-layerable

Decision (2026-07-02): UnaOS targets a **single primary human with
capability-isolated software principals**, not classical login/uid multi-user.

- Every vessel, handler, and agent is a **principal** with an explicit grant
  set (e.g. Comscan: `serial:acm0`, `gpio:*`; Vug: `fs:/models rw`; an
  untrusted download: nothing).
- **Handles are capabilities.** The per-process handle table (arc U4) is the
  enforcement point: every syscall that touches a resource goes through it.
  Grant, attenuate, revoke are first-class operations; handle transfer over
  the bandy bus is unforgeable.
- **UnaFS stores the metadata natively**: `owner` and `grants:*` are ordinary
  typed attributes — queryable ("show every file the midi-agent can write"),
  and no on-disk format change is ever needed.
- **POSIX hedge**: if human multi-user is ever wanted, a "user" becomes a
  named bundle of capabilities layered on the same attributes. Nothing in the
  chain below has to be redone.

Implementation chain: **U1a → U1b → U2 → U3 → U4 → U5 → U6** (x86 leads, Pi
ports, Jetson joins — full table in `ROADMAP.md` §1).

## Hardening ledger

### x86_64
- [x] Ring-3 boundary: user GDT segments (DPL-3 code/data, SYSRET-compatible STAR), `TSS.RSP0`, SYSCALL/SYSRET path (`EFER.SCE`, `LSTAR`, `STAR`, `FMASK`) (U1a, 2026-07-02, **QEMU-verified; metal pending**)
- [x] EFER.NXE enabled; NX on all user data/stack pages; user **code page W^X enforced across cores** — mapped ring3-RX **read-only from the start** at `USER_BASE` (the blob is copied through the identity alias, never through `USER_BASE`), so no core can ever cache a writable mapping of it; supersedes the U1a single-core `invlpg` flip (U1a/**U1b B4**, 2026-07-02, **QEMU-verified + metal-confirmed** on the real 2012 rMBP: `u1b-code-write` faulted with `err=0x7` (Write bit set) on the real page tables)
- [x] User fault → task kill, never kernel halt — a ring-3 fault (`CS.RPL == 3`) on #PF/#GP/#UD/#SS/#NP/#DE/#BR/#AC `swapgs`es, logs `(task, vector, cr2)`, and kills the task via `sched::exit()`; a CPL-0 fault stays fatal (kernel bug never hidden). (**U1b A**, 2026-07-02, **QEMU-verified + metal-confirmed** on the real 2012 rMBP: 3 kills at `vec=14 err=0x7/0x7/0x15`, `exited=1 killed=3 -> PASS`, **kernel continued past all three kills** — proving `swapgs`/GS restore work on silicon)
- [x] SYSRET boundary hardening: caller-saved GPR scrub (`rdi/rsi/rdx/r8/r9/r10` zeroed before `sysretq` — no kernel-dispatcher pointer leaks to ring 3) + canonical-`rcx` guard (a non-canonical return RIP — the CVE-2012-0217 shape — is refused and the task killed, never `sysret`ed with the user rsp loaded) (**U1b B1/B2**, 2026-07-02, **QEMU-verified + metal-confirmed for the pass-through** — the ring-3 round-trip returns cleanly through the scrub+guard tail on real silicon. The guard's *refusal* path — a non-canonical `rcx` actually refused — has not been exercised in any run; a functional test is still future)
- [x] NMI on a dedicated IST stack — an NMI landing in the pre-`swapgs`/pre-stack-switch window of `unaos_syscall_entry` switches to a kernel stack unconditionally, so its frame can never be pushed onto the ring-3 stack; the handler stays GS-free (**U1b B3**, 2026-07-02, **QEMU-verified**: IST slot installed and boot-verified on QEMU and metal. The NMI-in-window *fire* path itself is untested — no NMI fixture exists yet, so metal confirmation of B3 is future; the fault-kill metal evidence does not exercise NMI)
- [x] CR4.SMEP set — code gates it on `CPUID.7:EBX.SMEP` and sets it when present; **metal-confirmed active on the real 2012 rMBP** (`:: SMEP on ::` photographed 2026-07-02 — TCG `qemu64` does **not** expose SMEP, logging `SMEP unsupported`, so this is the one line QEMU cannot show). A SMEP-*violation* functional test (supervisor fetch from a ring-3 page → #PF) is still future. **SMAP unavailable pre-Broadwell** — compensate with explicit `copy_from_user` discipline
- [ ] W^X audit of kernel mappings (no page both writable and executable) — note: `CR0.WP` is briefly cleared around the U1a/U1b page-table edits (firmware maps its tables read-only) and restored; scope is the mapper only
- [ ] Validated user-pointer access (`copy_from_user`/`copy_to_user`)
- [ ] Per-process address spaces — no shared user window (U3)

### aarch64
- [x] EL0/EL1 privilege split (M6a-0/M6a, 2026-07-01, metal-confirmed)
- [x] Per-page user permissions: user code EL0-RX, stacks RW-XN; UXN/PXN on user pages (M6b)
- [x] EL0 fault → task kill with syndrome-matched accounting (M6b)
- [x] Preemptible EL0 — SP_EL0 banked in the IRQ frame; `spawn_user` starts I-unmasked (M6e, 2026-07-02, QEMU-verified + **metal-confirmed** real Pi 4: `IRQs-taken-at-EL0=18`, spinner resumed correctly, no faults). W^X map **unchanged**: the shared user stack is retained (no EL0 program writes its stack), so no new user page and no permission change — the audit table below still holds.
- [x] Kernel W^X / WXN audit (SCTLR_EL1.WXN feasibility) — audit complete; WXN enable pending review (M6c/H1; table below)
- [ ] PAN (Privileged Access Never) where silicon supports it
- [ ] Validated user-pointer access (M6f)
- [x] Per-task address-space isolation via ASIDs (M6d, 2026-07-02, QEMU-verified + **★ metal-confirmed** real Pi 4) — each EL0 task runs in its own translation-table branch with its own 16 KiB backing mapped at the same VAs; user-window leaves are non-global (`nG=1`, ASID-tagged 1..8), kernel leaves stay Global; `TTBR0_EL1` is switched (root + ASID) on dispatch and the slot is broadcast-`TLBI ASIDE1IS`'d and repointed off at exit. A task can no longer see or corrupt another task's user memory at a shared VA. **Metal proof (2026-07-02, EL=1/54 MHz — the class of thing QEMU cannot test):** `same-VA isolation A=0xa5a5…1 B=0x5a5a…2 distinct -> PASS` on real A72 TLB/caches (backed by a deterministic kernel-side TTBR0-swap `nG` probe), EL0 stack write/readback + SP-sentinel `-> PASS`, and with EL0 preemption live in the same boot (aggregate `IRQs-taken-at-EL0=21`, QEMU=0 — a demo-wide counter with no per-task attribution) all four slot tasks reported correct sentinel/stack values across interleaved per-task `TTBR0`/ASID dispatches; M6b `exited=1 killed=3 -> PASS` unchanged, 0 unexpected faults.
- [x] EL0 first-entry GPR scrub (M6d, 2026-07-02) — `user_task_trampoline` zeroes x0–x30 before the first `eret` to EL0, so no live kernel value (the raw `Task` pointer, kernel x29/x30, ...) reaches EL0 **in the general-purpose file** at entry. The FP/SIMD file is NOT yet covered (next item). The aarch64 twin of the x86 SYSRET GPR scrub (merged with U1b).
- [ ] EL0 first-entry FP/SIMD scrub — `CPACR_EL1.FPEN=0b11` makes `v0-v31`/FPSR/FPCR EL0-readable, and the `+neon` kernel can leave live data in `v0-v7`/`v16-v31` at first entry (`v8-v15` are provably zero via the boot context frame); also initialize `TPIDR_EL0`/`TPIDRRO_EL0` (firmware-residue UNKNOWN today). Slotted for M6f.

#### aarch64 W^X map audit (H1, M6c — report only; WXN not enabled)

Walk of the EL1&0 boot page tables as built in `arch/aarch64/boot.rs` (`build_l1`,
`protect_user_code`; TTBR0_EL1 → one L1 of 1 GiB blocks, with L1[0] demoted to L2_USER/L3_USER for
the 4 KiB EL0 window). `AP[7:6]`: `0b00` = EL1-RW/EL0-none, `0b01` = EL1+EL0 RW, `0b11` = RO at both
ELs. `UXN` = bit 54 (EL0 execute-never), `PXN` = bit 53 (EL1 execute-never).

| Region (VA == PA) | Descriptor | Attr | AP | EL1 | EL0 | UXN/PXN | W∧X? |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| 0–1 GiB general RAM (L2_USER blocks + L3_USER pages outside `USER_REGION`) | 2 MiB block / 4 KiB page | Normal | `0b00` | RW+X | — | 0/0 | **yes (EL1 RWX)** |
| `USER_REGION` code page `[0x100000,0x101000)` after `protect_user_code` | 4 KiB page | Normal | `0b11` | RO, no-X | R+X | 0/1 | no |
| `USER_REGION` data/stack `[0x101000,0x104000)` | 4 KiB page | Normal | `0b01` | RW, no-X | RW, no-X | 1/1 | no |
| 1–2 GiB RAM (L1[1]) | 1 GiB block | Normal | `0b00` | RW+X | — | 0/0 | **yes (EL1 RWX)** |
| 2–3 GiB RAM (L1[2]) | 1 GiB block | Normal | `0b00` | RW+X | — | 0/0 | **yes (EL1 RWX)** |
| 3–4 GiB Device (L1[3]) | 1 GiB block | Device | `0b00` | RW, no-X | — | 1/1 | no |

Transient: the code page is a data page (`0b01`, UXN=PXN=1, RW no-X) during the blob copy, then
`protect_user_code` flips it to the row above — so it is never W∧X, at either EL, at any point.

Since M6d the two `USER_REGION` rows are also **non-global (`nG=1`)** and replicated per task: the shared
window is the ASID-0 boot context, and up to 8 per-task slots (`boot::alloc_user_slot`, ASID 1..8) map the
same two VAs to their own frames with identical AP/UXN/PXN. The W^X properties of both rows are unchanged
(nG affects only TLB ASID-tagging, not access permission); the VAs shown are illustrative and shift with
the kernel image size (the pool adds ~224 KiB BSS). Kernel rows stay Global (`nG=0`).

**Every EL0-reachable page already satisfies W^X**: the user code page is R+X but read-only, and the
data/stack pages are RW but execute-never. The only W∧X coincidences are in the **kernel's own coarse
RWX identity RAM** (0–3 GiB, `AP=0b00`, PXN=0): the kernel image `.text` (which must be executable)
shares one RWX attribute set with the heap/stack/BSS/data (which must be writable).

**SCTLR_EL1.WXN feasibility.** With `WXN=1` the EL1&0 regime forces "writable ⇒ execute-never":
EL1-writable regions become PXN, EL0-writable regions become UXN. The EL0 window is unaffected (the
code page is not writable at either EL; data/stack are already XN). But the kernel executes from the
`AP=0b00`, PXN=0 identity RAM above — under WXN that RAM becomes EL1-non-executable and the next
kernel instruction fetch faults, so the kernel cannot boot. Enabling WXN therefore requires FIRST
splitting the identity map into a read-only-executable kernel text/rodata region and RW-execute-never
data/heap/stack/BSS regions (a linker-symbol-driven map, mirroring the 4 KiB EL0 code/data split M6b
already performs). That refactor is a separate, review-gated change; **WXN is not enabled in M6c.**

### Ring-0 parser audits (today's practical attack surface)
- [ ] Network stack: header-length/bounds audit (Ethernet/ARP/IP/TCP options/DHCP options)
- [ ] USB: descriptor parsing bounds (config/interface/endpoint/HID report walks; hub paths)
- [ ] FAT: BPB/dirent/FAT-chain bounds and loop guards (partially hardened during the read-only arc — re-verify and record)

### Process & supply chain
- [ ] Adversarial review before metal and before merge on every arc (standing rule, `CLAUDE.md`)
- [ ] Code-signing / "self vs non-self" loader check (design: `dev/OS/04_SECURITY_IMMUNITY/intrusion_detection.md`) — becomes real at U2 (loadable programs); simple allowlist first, signatures when entropy + crypto land
