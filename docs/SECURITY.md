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
2. **Missing privilege boundary (x86).** Everything on x86_64 runs in ring 0
   today. aarch64 has the project's first real boundary (EL0 + per-page
   permissions + fault→task-kill, arcs M6a/M6b).
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
- [x] EFER.NXE enabled; NX on all user data/stack pages; user **code page W^X** (mapped ring3-RX, WRITABLE dropped before first entry) (U1a, 2026-07-02, **QEMU-verified; metal pending**)
- [ ] CR4.SMEP — code gates it on `CPUID.7:EBX.SMEP` and sets it when present (so it activates on Ivy Bridge metal); TCG `qemu64` does **not** expose SMEP, logged `SMEP unsupported`, so it is **not exercisable under QEMU** (metal pending). **SMAP unavailable pre-Broadwell** — compensate with explicit `copy_from_user` discipline
- [ ] W^X audit of kernel mappings (no page both writable and executable) — note: `CR0.WP` is briefly cleared around the U1a page-table edits (firmware maps its tables read-only) and restored; scope is the mapper only
- [ ] User fault → task kill, never kernel halt (U1b) — today a ring-3 fault is fatal but lands on the RSP0 kernel stack (no triple-fault)
- [ ] Validated user-pointer access (`copy_from_user`/`copy_to_user`)
- [ ] Per-process address spaces — no shared user window (U3)

### aarch64
- [x] EL0/EL1 privilege split (M6a-0/M6a, 2026-07-01, metal-confirmed)
- [x] Per-page user permissions: user code EL0-RX, stacks RW-XN; UXN/PXN on user pages (M6b)
- [x] EL0 fault → task kill with syndrome-matched accounting (M6b)
- [ ] Kernel W^X / WXN audit (SCTLR_EL1.WXN feasibility)
- [ ] PAN (Privileged Access Never) where silicon supports it
- [ ] Validated user-pointer access (M6f)
- [ ] Per-task address spaces + ASIDs (M6d)

### Ring-0 parser audits (today's practical attack surface)
- [ ] Network stack: header-length/bounds audit (Ethernet/ARP/IP/TCP options/DHCP options)
- [ ] USB: descriptor parsing bounds (config/interface/endpoint/HID report walks; hub paths)
- [ ] FAT: BPB/dirent/FAT-chain bounds and loop guards (partially hardened during the read-only arc — re-verify and record)

### Process & supply chain
- [ ] Adversarial review before metal and before merge on every arc (standing rule, `CLAUDE.md`)
- [ ] Code-signing / "self vs non-self" loader check (design: `dev/OS/04_SECURITY_IMMUNITY/intrusion_detection.md`) — becomes real at U2 (loadable programs); simple allowlist first, signatures when entropy + crypto land
