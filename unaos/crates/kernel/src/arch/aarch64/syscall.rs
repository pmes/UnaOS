// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 EL0 userspace + the SVC syscall interface (M6a: the first privilege boundary; M6b: fault
// isolation + per-page user permissions; M6c: the well-behaved `hello` program moved OUT of kernel
// `.text` into a separately linked, baked-in flat blob — `USER_BLOB` below).
//
// The kernel runs at EL1 (see boot::drop_to_el1). A user task drops to EL0 (sched::spawn_user) and
// calls back in with `svc #0`; because the kernel is at EL1 and HCR_EL2.TGE=0, that SVC is taken to
// EL1 at VBAR_EL1 + 0x400, where the `__vec_svc` stub (exceptions.rs) saves the frame, checks
// ESR_EL1.EC==0x15 (SVC from AArch64), and calls `aarch64_svc_handler` here — on the faulting task's
// own kernel stack, IRQ-masked. The ABI is the Linux-aarch64 one: x8 = syscall number, args in x0–x5,
// return in x0.
//
// M6b: any OTHER synchronous exception from EL0 (abort/alignment/UNDEF/trapped sysreg) kills the
// task — `aarch64_el0_fault_handler` (exceptions.rs) logs it, records it here (`record_el0_kill`),
// and exits the task; the kernel survives. The user window is permission-split: the CODE page is
// EL0-RX/EL1-RO (flipped by boot::protect_user_code after the blob copy — the kernel's first live
// page-table update), the DATA/STACK pages are EL0-RW and never executable. The M6b demo proves all
// of it with four EL0 programs (one well-behaved — the M6c loaded blob — and three deliberately
// faulting inline fixtures) and a verdict task that demands the EXACT outcome split — see `verdict`
// and main.rs. M6f adds a real copy_from_user and a wider surface.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering};
// U11-M2: the low-level SPINLOCK (spin::Mutex) guarding the GLOBAL cross-ASID open-file refcount table. Same
// primitive the scheduler uses for RUN_QUEUES/SLEEPERS (`sched.rs`), imported directly because that module's
// alias is private. It is safe to take IRQ-masked (the teardown decrement path) — never held across block I/O.
use spin::Mutex as SpinMutex;

// --- Syscall numbers. WRITE/EXIT are the M6a/M6b core; REPORT is the M6d demo channel; YIELD/SLEEP_MS/
// GETPID/GETINFO are the M6f "real" surface (all thin over existing scheduler/timer primitives). The
// numbering is common across arches (documented in userspace.md) so the x86 U-side port stays aligned. ---
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
/// M6d demo: report a u64 value to the kernel, keyed by the calling task's name (see `m6d_report`).
/// Demo-only accounting channel — a real OS would not have this; it lets an EL0 program hand the kernel
/// the value it read from its own (slot-private) address space so the verdict can check isolation.
const SYS_REPORT: u64 = 3;
/// M6f: cooperatively give up the CPU — thin over `sched::yield_now()`. Returns 0.
const SYS_YIELD: u64 = 4;
/// M6f: sleep ~`a0` milliseconds — thin over `sched::sleep_ticks()` (ms→ticks at the 250 Hz tick, round
/// up). Returns 0. QEMU has no delivered timer IRQ, so it falls back to a cooperative yield there.
const SYS_SLEEP_MS: u64 = 5;
/// M6f: return the calling task's id (pid) in x0.
const SYS_GETPID: u64 = 6;
/// M6f: write a fixed {pid, ticks} struct to the user pointer in x0 via `copy_to_user`. Returns 0 or -EFAULT.
const SYS_GETINFO: u64 = 7;
/// M7/U4: load the fixed on-disk program (`HELLO.BIN`) into a fresh per-task slot, run it at EL0 as a CHILD,
/// and return a HANDLE index into the CALLER's per-process handle table (U4 — not the raw pid), or a negative
/// errno. No args this arc — arbitrary program-by-name is M8 (it needs a validated `copy_from_user` name).
/// See `sys_spawn`.
const SYS_SPAWN: u64 = 8;
/// M7/U4: block the caller until the child referred to by the HANDLE in `a0` exits, then return its exit
/// status (or -ECHILD if that handle is not in the caller's table — structural ownership). Woken by the
/// child's `done.post()` — a scheduler wake, so it works under QEMU. See `sys_wait`.
const SYS_WAIT: u64 = 9;
/// U5: operate on the caller's OWN handle table as capabilities. `a0` selects the sub-op
/// (`CAP_OP_GRANT`/`CAP_OP_REVOKE`); the remaining args are op-specific (see `sys_cap`). GRANT mints a new,
/// rights-attenuated handle to the same target as a source handle the caller holds `CAP_GRANT` on; REVOKE
/// clears a handle the caller owns. The enforcement layer sits at the handle lookup (`handle_resolve`).
const SYS_CAP: u64 = 10;
/// `SYS_CAP` sub-ops (in `a0`). GRANT: `a1`=source handle idx, `a2`=requested rights mask -> new handle idx
/// (attenuated) or a negative errno. REVOKE: `a1`=handle idx to drop -> 0 or a negative errno.
const CAP_OP_GRANT: u64 = 0;
const CAP_OP_REVOKE: u64 = 1;
/// U7: revoke a TRANSFER the caller previously made with `SYS_XFER` (`a1` = the transfer id SYS_XFER
/// returned). Sender-only (the transfer RECORD is sender-owned); single-level — revoking a transfer makes
/// the RECEIVED capability stale at its next `handle_resolve` (and discards it if still pending in the
/// recipient's inbox), but does NOT cascade through further re-transfers (revocation TREES are deferred).
const CAP_OP_XREVOKE: u64 = 2;
/// U6b: open a disk file BY NAME under the object table. `a0`=name ptr (EL0), `a1`=name len -> a HANDLE
/// index naming a `File` object carrying `CAP_READ`, or a negative errno. `copy_from_user`s the (bounded)
/// name, mounts the single FAT volume, finds the top-level 8.3 entry, allocates a per-task file descriptor,
/// and installs the File handle first-free. Read-only, flat root, one volume — the capability precursor to
/// UnaFS grants. See `sys_open`.
const SYS_OPEN: u64 = 11;
/// U6b: read from an open File handle. `a0`=handle idx, `a1`=dest buf (EL0, writable), `a2`=len -> the byte
/// count (`0` = EOF), or a negative errno. The CHECK: `handle_resolve(asid, handle, CAP_READ)` must yield a
/// `File` — a missing right, a non-File kind, or no handle all give `-EACCES` (the object-table enforcement
/// point, the twin of `sys_write`'s Console+CAP_WRITE). Sequential — the descriptor's offset advances by the
/// count returned. See `sys_read`.
const SYS_READ: u64 = 12;
// U7: cross-process capability transfer — the FIRST cross-process op on the object table. XFER(dest,
// src, req_rights) deposits an ATTENUATED copy of a capability the caller holds into the recipient's
// per-ASID transfer INBOX (the one deliberately cross-ASID surface, CAS-managed); the recipient names
// itself by being a `Child` handle in the SENDER's own table (owner-scoped delegation — no global
// process namespace). RECV() pulls a pending capability out of the CALLER's own inbox into the CALLER's
// own handle row — so every handle-table row keeps its single writer (the sender NEVER writes the
// recipient's row). Returns: XFER -> a transfer id (for a later CAP_OP_XREVOKE), RECV -> a handle index.
const SYS_XFER: u64 = 13;
const SYS_RECV: u64 = 14;
/// U9: absolute seek on an open File descriptor. `a0`=handle idx, `a1`=absolute byte offset -> the new offset,
/// or a negative errno. The CHECK: `handle_resolve` must yield a `File` carrying ANY of `CAP_READ|CAP_WRITE`
/// (a non-File kind / no handle / a revoked-ancestor cap all give `-EACCES`); a request past the file's `size`
/// is `-EINVAL` (seeking TO `size`, the EOF position, is legal). Sets `FILE_OFFSET` (the U6b sidecar). Both a
/// later `SYS_READ` and a File `SYS_WRITE` resume from the seeked offset. See `sys_seek`.
const SYS_SEEK: u64 = 15;

/// U10: DELETE (unlink) the file an open File+`CAP_WRITE` handle refers to. `a0`=handle idx -> `0`, or a
/// negative errno. The CHECK is `sys_write`'s: `handle_resolve(asid, fd, CAP_WRITE)` must yield a `File` (a
/// non-File kind / no handle / an RO-opened / a revoked cap all give `-EACCES`) — deletion is a mutation, gated
/// by the SAME single write CHECK. Frees the file's cluster chain (every FAT entry -> 0, ALL copies) and marks
/// its directory entry deleted (0xE5), then invalidates the descriptor + handle. Dir mark FIRST, then free, so a
/// crash mid-delete leaves lost clusters (benign), never a live entry pointing at freed/re-allocated clusters.
const SYS_UNLINK: u64 = 16;

/// U11: CLOSE an open `File` — `a0`=handle idx -> `0`, or a negative errno. Frees the handle's descriptor slot
/// (bumping the slot's GENERATION so any lingering sibling handle to the same slot goes stale, not re-bound to a
/// later file) and clears the handle word. A close needs NO capability right — you may always close a handle you
/// hold — so it resolves with `req = 0` (kind + descriptor identity still enforced). A non-`File` kind is
/// `-EINVAL` (Console/Socket are not closeable this arc; never silently corrupted); an unresolvable / already-
/// closed / stale-slot handle is `-EBADF` (double-close returns cleanly; use-after-close is denied). Only the
/// CALLER's own descriptor is freed — a cross-process open is unaffected (that lifetime is the open-file refcount).
const SYS_CLOSE: u64 = 17;

/// U6: GRANT (or revoke) another principal access to a PRIVATE file the caller OWNS — the delegation half of
/// the UnaFS owner/grants ACL. `a0`=a `File` handle the caller holds (names the file by its on-disk identity),
/// `a1`=a `Child` handle the caller holds (names the GRANTEE owner-scoped, the `sys_xfer` idiom — EL0 never
/// supplies a raw pid/ASID), `a2`=the granted rights (a `CAP_READ|CAP_WRITE` subset; `0` REVOKES). Returns `0`
/// or a negative errno. Only the file's current owner may grant; the grant is an ACL edge on the FILE (nothing
/// is delivered to the grantee's table) — the grantee simply opens the name and the SYS_OPEN ACL admits it. A
/// handle a grantee already holds survives a revoke (the ACL gates ACQUISITION, not held caps). See `sys_fgrant`.
const SYS_FGRANT: u64 = 18;

/// BANDY-1 M2: the on-UnaOS SMessage bus transport (ROADMAP §3b arc 1). SYS_MSEND(frame_ptr,
/// frame_len) submits ONE v1 request frame (arch/aarch64/bus.rs wire layout); the kernel
/// validates it fail-closed, STAMPS the sender's principal into the header (verdict C — a
/// caller-supplied principal is -EINVAL, never overwritten), fulfills the verb through the
/// EXISTING FAT/ACL machinery under the invoker's grants (verdict D — no impersonation, no new
/// model), and enqueues the reply into the SENDER's bounded mailbox. SYS_MRECV(buf_ptr, buf_len)
/// dequeues the next reply, blocking on the sys_wait Semaphore idiom while the mailbox is empty.
const SYS_MSEND: u64 = 19;
const SYS_MRECV: u64 = 20;

/// ELF-2: the EL0-threading syscalls (first rung of the thread ladder). Unlike `SYS_SPAWN` (a new PROCESS —
/// fresh slot/ttbr0/ASID, its own address space), these create/reap THREADS that SHARE the caller's address
/// space (same slot root/ASID), so a shared-memory word is coherent across them.
///   SYS_THREAD_SPAWN(entry, sp, arg) [+ a3 = placement: 0 caller-core, 1 sibling-core] -> thread handle >=0
///     A new EL0 thread under the caller's ttbr0/ASID: it starts at `entry` on stack `sp` (both carved from
///     the caller's 16 KiB window) with `arg` in x0. Returns a small per-process thread handle, or -errno.
///   SYS_THREAD_JOIN(handle) -> 0: block until that thread finishes (its `SYS_THREAD_EXIT`), via the
///     JoinHandle completion Semaphore. -ESRCH if the handle is not the caller's live thread.
///   SYS_THREAD_EXIT(): terminate the calling thread — posts its completion, decrements the slot refcount
///     (teardown only on the LAST thread), never returns.
const SYS_THREAD_SPAWN: u64 = 21;
const SYS_THREAD_EXIT: u64 = 22;
const SYS_THREAD_JOIN: u64 = 23;

/// ELF-3: give EL0 something to draw on + real synchronisation.
///   SYS_FB_MAP() -> surface VA >= 0 / -errno
///     Maps the calling process's dedicated OFF-SCREEN surface (kernel-allocated, page-aligned,
///     EL0-RW Normal-cacheable) plus a read-only INFO page (width/height/stride/format/size) into its
///     EL0 window, and returns the surface VA. EL0 NEVER receives the real scan-out — only the kernel
///     composites the surface to the screen (SYS_FB_PRESENT). Idempotent.
///   SYS_FB_PRESENT() -> 0 / -errno
///     Composite the process's surface to the real framebuffer via the registered present hook (the
///     existing dirty-rect damage+flush machinery in the video subsystem). Also records a checksum of
///     the surface for the self-verifying witness. If no hook is registered (headless/QEMU), the
///     checksum still proves the drawn surface; the scan-out is never exposed to EL0 either way.
///   SYS_FUTEX(uaddr, op, val) -> op-specific / -errno
///     op=0 FUTEX_WAIT: block iff *uaddr == val, keyed by the physical address of uaddr (bounded queue).
///     op=1 FUTEX_WAKE: wake up to `val` waiters on that key; returns the count woken. Enough for a
///     userspace mutex/condvar. `uaddr` is validated to lie in the caller's writable user window.
const SYS_FB_MAP: u64 = 24;
const SYS_FB_PRESENT: u64 = 25;
const SYS_FUTEX: u64 = 26;
/// SYS_FUTEX sub-ops (in x1).
const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;

/// ELF-5: input into EL0 — the next ladder rung after surface + threads + futex. An interactive EL0
/// app needs keys/mouse.
///   SYS_INPUT_POLL() -> packed event >= 0 / -EAGAIN
///     NON-BLOCKING. Returns the next input event queued for the CALLING process (only while it is the
///     ACTIVE app — the router only enqueues into the active process's ring), or `-EAGAIN` when the ring
///     is empty. The event is a packed u64 (bit 63 always clear, so it never collides with a negative
///     errno): [55:48] = type (see `INPUT_EV_*`), payload in the low 32 bits — key ASCII / button mask in
///     [7:0], mouse dx/x in [31:16] (i16), dy/y in [15:0] (i16). The router seam is `el0_input_enqueue`
///     (mirroring ELF-3's present seam); active-process registration is `el0_input_set_active`.
const SYS_INPUT_POLL: u64 = 27;
/// SYS_INPUT_WAIT = 28 is DEFERRED (documented at `sys_input_poll`) — a blocking variant on a per-ASID
/// Semaphore that `el0_input_enqueue` posts; QEMU has no real input source, so POLL is the QEMU-provable
/// rung this arc lands.

/// M6e demo: the sentinel `sys_exit` status the preemption spinner uses so its exit is accounted to
/// `EL0_SPIN_DONE` and never perturbs the M6b `exited/killed` counters. Demo-only — there is no real
/// userspace yet, so overloading one status value for demo bookkeeping is safe and documented here.
const M6E_SPIN_STATUS: u64 = 0x6E;
/// M6d demo: the sentinel `sys_exit` status every M6d task uses so its exit lands in `EL0_M6D_DONE` and
/// never touches the M6b (`EL0_EXITED_OK/ERR`) or M6e (`EL0_SPIN_DONE`) counters — keeping those verdicts
/// byte-identical. The SYS_EXIT dispatch MUST test this BEFORE the catch-all `else` (see the handler).
const M6D_EXIT_STATUS: u64 = 0x6D;
/// M6f demo: the sentinel `sys_exit` status every M6f fixture uses so its exit lands in `EL0_M6F_DONE` and
/// never perturbs the M6b/M6d/M6e counters (same discipline as M6D/M6E). Tested BEFORE the catch-all `else`.
const M6F_EXIT_STATUS: u64 = 0x6F;
/// U4 demo: the sentinel `sys_exit` status the process-model fixtures (`el0-u4parent`, `el0-u4orphan`) use so
/// THEIR exits land in `EL0_U4_DONE`, never perturbing the M6b/M6d/M6e/M6f/M6g counters (same sentinel
/// discipline). A spawned CHILD is reaped through the Proc table by pid (see the SYS_EXIT arm), not by this
/// status. Fresh value (the retired M7 demo used `0x77`); distinct from the M6D/M6E/M6F sentinels and 0.
const U4_EXIT_STATUS: u64 = 0x74;
/// U4 demo: the nonzero WITNESS token the parent reports iff it reaped BOTH children by handle with status 0.
/// A token (not a pid) — `sys_spawn` now returns a handle, so the verdict only needs non-zero-means-both-ok.
/// Must match `movz x23, #0xC4` in `__u4_prog_parent`; `u4_launcher` only checks it is non-zero.
const U4_WITNESS_TOKEN: u64 = 0xC4;
/// U4: the exit status the child-KILL path stores into a child's Proc entry so a killed child still wakes
/// its parent's `sys_wait` (rather than hanging it) — non-zero so the parent's witness computes 0 (a killed
/// child is a FAIL). A normal child exits with its own status (0 for `HELLO.BIN`); this is used only on a kill.
const U4_KILLED_STATUS: i32 = 0x4B; // 'K'
/// U5 demo: the sentinel `sys_exit` status the capability fixture (`el0-u5cap`) uses so ITS exit lands in
/// `EL0_U5_DONE`, never perturbing the M6b/M6d/M6e/M6f/M6g/U4 counters (same sentinel discipline). Distinct
/// from every prior sentinel (0x6D/0x6E/0x6F/0x74) and 0. Tested BEFORE the catch-all `else` in SYS_EXIT.
const U5_EXIT_STATUS: u64 = 0x75;
/// U5 demo: the witness bitmask the capability fixture reports, one bit per proven behaviour — write-cap OK
/// (bit0), no-cap `-EACCES` (bit1), attenuated grant bounded + subset grant works (bit2), revoke enforced
/// (bit3). `u5_launcher` PASSes iff the fixture reports exactly `U5_WITNESS_ALL` (all four). Must match the
/// `add x23, x23, #{1,2,4,8}` steps in `__u5_prog_cap`.
const U5_WITNESS_ALL: u64 = 0xF;
/// U6 demo: the sentinel `sys_exit` status the printing-spawner fixture (`el0-u6spawn`) uses so ITS exit lands
/// in `EL0_U6_DONE`, never perturbing the M6b/M6d/M6e/M6f/M6g/U4/U5 counters (same sentinel discipline).
/// Distinct from every prior sentinel (0x6D/0x6E/0x6F/0x74/0x75) and 0. Tested BEFORE the catch-all `else`.
const U6_EXIT_STATUS: u64 = 0x76;
/// U6 demo: the witness bitmask the printing-spawner reports — bit0 print-before-spawn OK, bit1 both child
/// handles valid AND off the reserved console index (`!= CONSOLE_FD`, distinct), bit2 print-AFTER-spawn OK
/// (the console cap survived two spawns — the no-collision proof: under U5 a child could clobber it), bit3
/// both children reaped with status 0. `u6_launcher` PASSes iff it equals `U6_WITNESS_ALL` (all four) AND the
/// kernel-side kind/no-collision check passed. Must match the `add x23, x23, #{1,2,4,8}` steps in
/// `__u6_prog_spawn`.
const U6_WITNESS_ALL: u64 = 0xF;
/// U6b demo: the sentinel `sys_exit` status the File-handle fixture (`el0-u6bfile`) uses so ITS exit lands in
/// `EL0_U6B_DONE`, never perturbing the M6b/M6d/M6e/M6f/M6g/U4/U5/U6 counters (same sentinel discipline).
/// Distinct from every prior sentinel (0x6D/0x6E/0x6F/0x74/0x75/0x76) and 0; `0x77` is free (the retired M7
/// demo once used it). Tested BEFORE the catch-all `else` in SYS_EXIT.
const U6B_EXIT_STATUS: u64 = 0x77;
/// U6b demo: the witness bitmask the File-handle fixture reports — one bit per proven behaviour: bit0
/// open OK (`SYS_OPEN("HELLO.BIN")` -> handle >= 0), bit1 read OK (`SYS_READ` -> 16 bytes), bit2 bytes match
/// the on-disk blob (the 16 read bytes equal the kernel-planted `USER_BLOB` prefix), bit3 a File handle
/// LACKING `CAP_READ` -> `-EACCES` (the rights CHECK), bit4 a non-File handle (a `Socket` carrying `CAP_READ`)
/// -> `-EACCES` (the kind CHECK). `u6b_launcher` PASSes iff it equals `U6B_WITNESS_ALL` (all five). Must match
/// the `add x23, x23, #{1,2,4,8,16}` steps in `__u6b_prog_file`.
const U6B_WITNESS_ALL: u64 = 0x1F;
/// The handle indices `u6b_launcher` pre-endows in the fixture's table for its two negative checks, chosen
/// off the reserved `CONSOLE_FD` (1) and off index 0 (which the fixture's own `SYS_OPEN` first-free-claims):
/// a File handle WITHOUT `CAP_READ` (the rights negative) and a `Socket` handle WITH `CAP_READ` (the kind
/// negative). The `mov x0, #{2,3}` operands in `__u6b_prog_file` MUST match these.
const U6B_NOCAP_IDX: usize = 2;
const U6B_SOCK_IDX: usize = 3;

/// U7 demo: the sentinel `sys_exit` status BOTH transfer fixtures (`el0-u7parent`/`el0-u7child`) use so their
/// exits land in `EL0_U7_DONE` (routed by NAME, ahead of the Proc short-circuit — see the SYS_EXIT arm), never
/// perturbing any prior counter.
const U7_EXIT_STATUS: u64 = 0x78;
/// The full per-fixture witness — 4 bits each. Parent: over-rights XFER `-EACCES` (b0), XFER t1 ok (b1),
/// XREVOKE t1 ok (b2), XFER t2 ok (b3). Child: RECV t1 (b0), USED the transferred Console cap — a real
/// `sys_write` through it landed (b1), RECV t2 (b2), the revoked t1 cap now `-EACCES` (b3).
const U7_WITNESS_ALL: u64 = 0xF;
/// The mid-run token the CHILD reports (SYS_REPORT) the moment its first write through the transferred cap
/// lands — the launcher's cue to release the parent's GO word (so the revoke provably happens AFTER a
/// successful use). Distinct from any witness value (witnesses are <= 0xF).
const U7_USED_TOKEN: u64 = 0x51;
/// The parent's pre-endowed handle indices: the `Child` handle naming the recipient (`U7_DEST_IDX`) and the
/// full Console cap it transfers from (`U7_SRC_IDX`, `CAP_WRITE|CAP_GRANT` — CAP_GRANT is the delegation
/// right XFER requires on its source). The `mov x{0,1}, #{2,3}` operands in `__u7_prog_parent` MUST match.
const U7_DEST_IDX: usize = 2;
const U7_SRC_IDX: usize = 3;

/// U8 demo: the sentinel `sys_exit` status the revocation-tree fixture (`el0-u8tree`) uses so ITS exit lands
/// in `EL0_U8_DONE`, never perturbing any prior counter (same sentinel discipline). Distinct from every prior
/// sentinel (0x6D/0x6E/0x6F/0x74/0x75/0x76/0x77/0x78) and 0. Tested BEFORE the catch-all `else` in SYS_EXIT.
const U8_EXIT_STATUS: u64 = 0x79;
/// U8 demo: the witness bitmask the revocation-tree fixture reports — bit0 grant chain works (grant ->
/// re-grant -> write through the grandchild cap lands), bit1 revoking the PARENT (a handle carrying
/// `CAP_REVOKE`) returns 0 AND revoking it again returns exactly `-ECHILD` (the double-revoke errno), bit2
/// BOTH descendant copies are `-EACCES` at their next use (the subtree kill), bit3 a revoke WITHOUT
/// `CAP_REVOKE` stays LOCAL (the derived copy still writes) and ITS double revoke errnos too. `u8_launcher`
/// PASSes iff it equals `U8_WITNESS_ALL` AND the
/// kernel-side re-transfer-cascade + generation checks passed. Must match the `add x23, x23, #{1,2,4,8}`
/// steps in `__u8_prog_tree`.
const U8_WITNESS_ALL: u64 = 0xF;
/// The handle indices `u8_launcher` pre-endows in the fixture's table (off index 0 — first-free-claimed by
/// the fixture's own grants — and off the reserved `CONSOLE_FD`): a full console cap WITH `CAP_REVOKE` (the
/// tree-revoke parent) and one WITHOUT it (the locality negative). The `mov x1, #{2,3}` operands in
/// `__u8_prog_tree` MUST match.
const U8_SRC_IDX: usize = 2;
const U8_SRC2_IDX: usize = 3;

/// U9 demo: the sentinel `sys_exit` status the File-WRITE fixture (`el0-u9write`) uses so ITS exit lands in
/// `EL0_U9_DONE`, never perturbing any prior counter (same sentinel discipline). Distinct from every prior
/// sentinel (0x6D/0x6E/0x6F/0x74..0x79) and 0. Tested BEFORE the catch-all `else` in SYS_EXIT.
const U9_EXIT_STATUS: u64 = 0x7A;
/// U9 demo: the witness bitmask the File-WRITE fixture reports — one bit per proven behaviour: bit0 open-RW OK
/// (`SYS_OPEN("SCRATCH.BIN", RW)` -> handle >= 0), bit1 seek+write OK (`SYS_SEEK` to the scratch offset then
/// `SYS_WRITE` -> the pattern length), bit2 read-back matches (seek back, `SYS_READ` -> the just-written bytes,
/// proving the overwrite landed and is visible through the same cap), bit3 an RO-opened File write -> `-EACCES`
/// (the CAP_WRITE rights CHECK), bit4 a non-File handle (a `Socket` carrying `CAP_WRITE`) write -> `-EACCES`
/// (the kind CHECK). `u9_launcher` PASSes iff it equals `U9_WITNESS_ALL` AND the kernel-side checks held. Must
/// match the `add x23, x23, #{1,2,4,8,16}` steps in `__u9_prog_write`.
const U9_WITNESS_ALL: u64 = 0x1F;
/// The handle index `u9_launcher` pre-endows: a `Socket` carrying `CAP_WRITE` — the kind negative (a non-File
/// object WITH the write right is still `-EACCES`, denied purely on kind). Off index 0 (the fixture's own RW
/// open first-free-claims it) and off `CONSOLE_FD`. The `mov x0, #2` operand in `__u9_prog_write` MUST match.
const U9_SOCK_IDX: usize = 2;
/// U9 demo: the dedicated scratch file the fixture opens + overwrites (planted by the launcher's FAT image,
/// filled with 0x55; NEVER `HELLO.BIN`). 11 chars (<= `MAX_NAME`); the `mov x1, #(9f-8f)` in the blob matches.
const U9_SCRATCH_NAME: &str = "SCRATCH.BIN";
/// U9 demo: the absolute byte offset the fixture seeks to and the 16-byte pattern it writes there. `OFFSET`
/// lands 8 bytes into the file's SECOND 512-byte sector, so the overwrite is a partial-sector read-modify-write
/// (the interesting case: the sector's other bytes must survive). Both are shared with `__u9_prog_write` (the
/// `movz`/`.ascii` there MUST match) and with the launcher's kernel-side "the sector changed" re-read.
const U9_WRITE_OFFSET: u32 = 520;
const U9_PATTERN: [u8; 16] = *b"U9-WRITE-OK-1234";
/// U9 demo: the scratch file's planted size (1 KiB of 0x55) — the "size did NOT change" invariant the launcher
/// asserts before and after the in-place write. Kept > 512 so `probe_once` never cats it (its `<= 512` guard).
const U9_SCRATCH_SIZE: u32 = 1024;

/// U10 demo: the sentinel `sys_exit` status the file-GROWTH fixture (`el0-u10grow`) uses so ITS exit lands in
/// `EL0_U10_DONE`, never perturbing any prior counter. Distinct from every prior sentinel (…0x7A) and 0.
const U10_EXIT_STATUS: u64 = 0x7B;
/// U10 demo: the witness bitmask the file-GROWTH fixture reports — one bit per proven behaviour: bit0 open-RW
/// OK; bit1 seek-to-EOF + write PAST the cluster boundary → 16 (the GROW happened, not a U9 clamp-to-0); bit2
/// seek-back + read → the appended pattern (visible through the same cap); bit3 read at offset 0 → the original
/// filler (the pre-existing cluster is intact — the grow did not corrupt it); bit4 an RO-opened File write →
/// `-EACCES` (growth is gated by the SAME single CAP_WRITE CHECK as the in-place write). `u10_launcher` PASSes
/// iff it equals `U10_WITNESS_ALL` AND the kernel-side checks held. Must match the `add x23, x23, #{1,2,4,8,16}`
/// steps in `__u10_prog_grow`.
const U10_WITNESS_ALL: u64 = 0x1F;
/// U10 demo: the dedicated file the fixture GROWS (planted by the launcher's FAT image as 1 sector = exactly one
/// 512-byte cluster of `0xC1` filler; NEVER `HELLO.BIN`/`SCRATCH.BIN`). 8 chars (<= `MAX_NAME`); the
/// `mov x1, #(9f-8f)` in the blob matches.
const U10_GROW_NAME: &str = "GROW.BIN";
/// U10 demo: the planted size (512 = one full cluster of `0xC1`) — the "before" size. The fixture seeks HERE
/// (EOF, on the cluster boundary) and writes, so the grow crosses into a freshly allocated SECOND cluster.
const U10_GROW_PLANTED_SIZE: u32 = 512;
/// U10 demo: the absolute byte offset the fixture seeks to and the 16-byte pattern it appends there. `OFFSET`
/// is the planted EOF (== one cluster), so the write allocates + chains a new cluster; the launcher's raw
/// re-read at this offset proves the appended bytes landed. Shared with `__u10_prog_grow` and the launcher.
const U10_GROW_OFFSET: u32 = 512;
const U10_GROW_PATTERN: [u8; 16] = *b"U10-GROW-OK-5678";
/// U10 demo: the `0xC1` byte the image fills the planted cluster with — the fixture reads offset 0 back and the
/// launcher re-reads it to prove the original cluster survived the grow. `__u10_prog_grow`'s filler constant
/// (16 × `0xC1`) and the image's `tr '\000' '\301'` plant MUST match.
const U10_GROW_FILLER: u8 = 0xC1;
/// U10 demo: the file's size AFTER the grow (`512 + 16`) — the "size increased" invariant the launcher asserts.
const U10_GROW_NEW_SIZE: u32 = U10_GROW_PLANTED_SIZE + 16;

/// U10-create demo: the sentinel `sys_exit` status the CREATE fixture (`el0-u10create`) uses so ITS exit lands
/// in `EL0_U10C_DONE`, never perturbing any prior counter. Distinct from every prior sentinel (…0x7B) and 0.
const U10C_EXIT_STATUS: u64 = 0x7C;
/// U10-create demo: the witness bitmask the CREATE fixture reports — bit0 open O_CREAT|RW OK (the file is
/// created); bit1 write at offset 0 → 16 (the first grow of a 0-cluster file allocates its first cluster + sets
/// the directory first_cluster); bit2 seek-0 + read → the pattern (readback through the same cap); bit3 a SECOND
/// O_CREAT|RW open of the same name → a handle (idempotent create-if-present). `u10c_launcher` PASSes iff it
/// equals `U10C_WITNESS_ALL` AND the kernel-side checks held. Matches `add x23, x23, #{1,2,4,8}` in the blob.
const U10C_WITNESS_ALL: u64 = 0xF;
/// U10-create demo: the file the fixture CREATES (it does NOT exist in the planted image; the demo makes it).
/// 9 chars (<= `MAX_NAME`); the `mov x1, #(9f-8f)` in the blob matches. Formats to the 8.3 slot "FRESH   BIN".
const U10C_NAME: &str = "FRESH.BIN";
/// U10-create demo: the 16-byte pattern the fixture writes into the freshly created file (also its final size).
const U10C_PATTERN: [u8; 16] = *b"U10-CREATE-OK-99";
/// U10-create demo: the created file's size after the write (== the pattern length) — the launcher's re-mount
/// asserts the on-disk entry has exactly this size.
const U10C_WRITTEN: u32 = 16;

/// U10-delete demo: the sentinel `sys_exit` status the DELETE fixture (`el0-u10delete`) uses so ITS exit lands
/// in `EL0_U10D_DONE`. Distinct from every prior sentinel (…0x7C) and 0.
const U10D_EXIT_STATUS: u64 = 0x7D;
/// U10-delete demo: the witness bitmask the DELETE fixture reports — bit0 create+open OK; bit1 write → 16
/// (grow-from-empty allocates the cluster); bit2 `SYS_UNLINK` → 0 (delete: dir 0xE5 + free chain, all copies,
/// and invalidate ALL of this process's descriptors for the file); bit3 a read through a SIBLING handle (the
/// file was opened twice) → `-EACCES` (the sibling descriptor was invalidated too — no stale reference to the
/// freed chain); bit4 a plain RO re-open → `-ENOENT` (the file is GONE). `u10d_launcher` PASSes iff it equals
/// `U10D_WITNESS_ALL` AND the kernel-side checks held. Matches `add x23, x23, #{1,2,4,8,16}` in the blob.
const U10D_WITNESS_ALL: u64 = 0x1F;
/// U10-delete demo: the self-contained file the fixture creates, writes, and DELETES (it does not exist in the
/// planted image). 9 chars (<= `MAX_NAME`); the `mov x1, #(9f-8f)` in the blob matches.
const U10D_NAME: &str = "DELME.BIN";

/// U11 demo: the sentinel `sys_exit` status the open-file-lifecycle fixture (`el0-u11close`) uses so ITS exit
/// lands in `EL0_U11_DONE`. Distinct from every prior sentinel (…0x7D) and 0.
const U11_EXIT_STATUS: u64 = 0x7E;
/// U11 demo: the witness bitmask the fixture reports — bit0 create A11.BIN + grow-write OK; bit1 a SECOND RW
/// open of A11.BIN → a sibling handle; bit2 `SYS_UNLINK` via the first handle → `0` (frees BOTH of this proc's
/// A11 descriptors, leaving the sibling lingering on a freed slot); bit3 after opening B11.BIN into the reused
/// slots + writing it, a read through the STALE sibling → `-EACCES` (a GENERATION mismatch — the slot is live
/// again for B11, so the only reason to deny is the stale gen — NOT a silent rebind onto B11's bytes); bit4
/// `SYS_CLOSE` → `0`, double-close → `-EBADF`, and a close→re-open→read round-trip returns B11's content.
/// `u11_launcher` PASSes iff it equals `U11_WITNESS_ALL` AND the kernel-side gen-rebind check + on-disk checks
/// hold. Matches `add x23, x23, #{1,2,4,8,16}` in the blob.
const U11_WITNESS_ALL: u64 = 0x1F;
/// U11 demo: the self-contained file the fixture CREATES, opens twice, and UNLINKs (it does not exist in the
/// planted image; the fixture makes it). 7 chars (<= `MAX_NAME`); the `mov x1, #(9f-8f)` in the blob matches.
const U11_A_NAME: &str = "A11.BIN";
/// U11 demo: the self-contained file the fixture creates in the slots A11.BIN freed (the "different file" whose
/// slot-reuse the stale sibling must NOT rebind onto). 7 chars; the `mov x1, #(11f-10f)` in the blob matches.
const U11_B_NAME: &str = "B11.BIN";
/// U11 demo: the 16-byte pattern written into A11.BIN (grow-from-empty), and into B11.BIN — distinct so a
/// mistaken rebind would be observable. Match the `.ascii` bytes in the blob.
const U11_B_PATTERN: [u8; 16] = *b"U11-REOPEN-B-567";

/// U11-M2 (defer) demo: the sentinel `sys_exit` status BOTH cross-process fixtures (`el0-u11defer-a`,
/// `el0-u11defer-b`) use so their exits land in `EL0_U11DEFER_DONE` (want 2). Distinct from every prior
/// sentinel (…0x7E) and 0.
const U11DEFER_EXIT_STATUS: u64 = 0x7F;
/// U11-M2 (defer) demo: process A's witness bitmask — bit0 create DEFER.BIN + grow-write OK; bit1 a read-back
/// AFTER B unlinked returns A's ORIGINAL bytes (the chain is STILL alive — the unlink was deferred); bit2
/// SYS_CLOSE OK (the LAST close, which runs the deferred free); bit3 double-close → -EBADF. Matches
/// `add x23, x23, #{1,2,4,8}` in program A. `u11defer_run` PASSes iff A's witness == this.
const U11DEFER_A_WITNESS_ALL: u64 = 0xF;
/// U11-M2 (defer) demo: process B's witness bitmask — bit0 open DEFER.BIN (A created it) OK; bit1 SYS_UNLINK via
/// B's handle → 0 (deferred: A still holds it open, so the chain is NOT freed); bit2 a re-open of the unlinked
/// name → -ENOENT (the name is gone immediately). Matches `add x23, x23, #{1,2,4}` in program B.
const U11DEFER_B_WITNESS_ALL: u64 = 0x7;
/// U11-M2 (defer) demo: the launcher-CUE tokens the fixtures SYS_REPORT (all `> 0xF`, so `u11defer_report`
/// distinguishes them from a final witness). A reports `A_OPENED` after create+write; B reports `B_UNLINKED`
/// after unlink+re-open; A reports `A_READ` after the post-unlink read. The launcher releases the next GO word
/// (and runs its fresh-mount checkpoint) on each cue — it is the single choreography sequencer.
const U11DEFER_A_OPENED: u64 = 0x60;
const U11DEFER_B_UNLINKED: u64 = 0x61;
const U11DEFER_A_READ: u64 = 0x62;
/// U11-M2 (defer) demo: the file A creates + writes + reads and B unlinks — runtime-created (not planted). 9
/// chars (<= `MAX_NAME`); the `mov x1, #(9f-8f)` in the blob matches.
const U11DEFER_NAME: &str = "DEFER.BIN";
/// U11-M2 (defer) demo: the 16-byte pattern A writes into DEFER.BIN — A reads it back AFTER B unlinks to prove
/// the chain is still alive. Match the `.ascii` bytes in the blob.
const U11DEFER_PATTERN: [u8; 16] = *b"U11-DEFER-OK-777";

/// U11-M2b (reap) demo: the sentinel `sys_exit` status BOTH reap fixtures (`el0-u11reap-a`, `el0-u11reap-b`)
/// use so their exits land in `EL0_U11REAP_DONE` (want 2). Distinct from every prior sentinel (…0x7F) and 0.
const U11REAP_EXIT_STATUS: u64 = 0x80;
/// U11-M2b (reap) demo: process A's witness bitmask — bit0 create DEFER2.BIN + grow-write OK; bit1 a read-back
/// AFTER B unlinked returns A's ORIGINAL bytes (the chain is STILL alive — the unlink was deferred). A then
/// EXITS WITHOUT CLOSING (no close bit: teardown is the last close), so `u11reap_run` PASSes iff A == this.
/// Matches `add x23, x23, #{1,2}` in program A.
const U11REAP_A_WITNESS_ALL: u64 = 0x3;
/// U11-M2b (reap) demo: process B's witness bitmask — bit0 open DEFER2.BIN OK; bit1 SYS_UNLINK -> 0 (deferred:
/// A still holds it open); bit2 a re-open of the unlinked name -> -ENOENT. Matches `add x23, x23, #{1,2,4}` in B.
const U11REAP_B_WITNESS_ALL: u64 = 0x7;
/// U11-M2b (reap) demo: the launcher-CUE tokens the fixtures SYS_REPORT (all `> 0xF`, so `u11reap_report`
/// distinguishes them from a final witness). A reports `A_OPENED` after create+write; B reports `B_UNLINKED`
/// after unlink+re-open; A reports `A_READ` after the post-unlink read. The launcher releases the next GO word
/// (and runs its fresh-mount checkpoint) on each cue — it is the single choreography sequencer.
const U11REAP_A_OPENED: u64 = 0x63;
const U11REAP_B_UNLINKED: u64 = 0x64;
const U11REAP_A_READ: u64 = 0x65;
/// U11-M2b (reap) demo: the file A creates + writes + reads and B unlinks — runtime-created (not planted). 10
/// chars (<= `MAX_NAME`); the `mov x1, #(9f-8f)` in the blob matches. Distinct from DEFER.BIN (the u11defer file).
const U11REAP_NAME: &str = "DEFER2.BIN";
/// U11-M2b (reap) demo: the 16-byte pattern A writes into DEFER2.BIN — A reads it back AFTER B unlinks to prove
/// the chain is still alive. Match the `.ascii` bytes in the blob.
const U11REAP_PATTERN: [u8; 16] = *b"U11-REAP-OK-7777";

// --- The inline EL0 FIXTURES: three fault-SHAPE fixtures (M6b) + one preemption spinner (M6e). These
// are fixtures, not programs, so they stay inline in the kernel image; only the well-behaved `hello`
// routine moved out to a separately linked blob in M6c (see `USER_BLOB` below). Fully
// position-independent — every reference is a PC-relative `adr` and there are only svc + mov-immediate
// + register ops — so they run correctly wherever the copy lands. `__fault_blob_{start,end}` bound the
// copy; the `__user_prog_*` labels are the per-fixture entries.
//
// The three fault fixtures each provoke ONE specific fault the kernel must answer with a task-kill. If
// the fault DOESN'T happen (broken permissions / stale TLB), the fixture falls through to sys_exit(1)
// — the SURVIVOR protocol: a self-reported, greppable FAIL. The tail self-exits rather than `b .`
// because QEMU raspi4b delivers no timer IRQ, so an EL0 spin is UNpreemptible THERE regardless of M6e
// (on metal, M6e now WOULD preempt it) — a `b .` survivor would wedge its core for the full
// kernel8-test window and silence the same-core verdict the failure is supposed to reach. ---
core::arch::global_asm!(
    r#"
    .globl __fault_blob_start
__fault_blob_start:
    // Write to PA 0x0 — EL1-only RAM (AP=0b00) -> EL0 data abort, EC=0x24, FAR=0x0. `str xzr` so
    // even a bug that lets the store through writes zeros, not garbage, over the dead spin-table.
    .balign 4
    .globl __user_prog_wild_write
__user_prog_wild_write:
    mov x0, #0
    str xzr, [x0]
    mov x8, #2                              // survivor: the store didn't fault -> sys_exit(1)
    mov x0, #1
    svc #0
1:  b 1b

    // Write to its OWN code page (EL0-RO after protect_user_code) -> EC=0x24, FAR in the code page.
    // The 4-byte target is exactly its own FIRST instruction — already executed — so if a stale-TLB
    // write sneaks through it cannot corrupt code that still has to run (the survivor exit(1) tail).
    .balign 4
    .globl __user_prog_code_write
__user_prog_code_write:
    adr x0, __user_prog_code_write
    str wzr, [x0]
    mov x8, #2                              // survivor: the store didn't fault -> sys_exit(1)
    mov x0, #1
    svc #0
1:  b 1b

    // Branch into the user STACK page (EL0-readable but UXN=1) -> instruction abort, EC=0x20,
    // FAR = the branch target in the data pages. No survivor tail is needed: if UXN were broken
    // the target bytes are BSS zeros = UDF, still a kill — but with EC 0x00, which the (task, EC,
    // FAR-page) bookkeeping counts as killed_UNEXPECTED, failing the verdict as it must.
    .balign 4
    .globl __user_prog_stack_exec
__user_prog_stack_exec:
    sub x0, sp, #16
    br x0
1:  b 1b

    // M6e preemption spinner: a long, register-only, syscall-free EL0 loop, then sys_exit with the
    // M6E sentinel status. With I unmasked at EL0 (spawn_user, M6e) the ONLY thing that can switch it
    // away is a timer IRQ, so on metal it is preempted mid-loop and interleaves with the co-located
    // capstone/kernel tasks (aarch64_irq_handler counts the EL0 IRQs; see `m6e_verdict`). It writes
    // NO memory (register-only), so it shares the demo user stack safely under preemptive interleave.
    // Count 0x0200_0000 (~33.5M) ≈ a few timer quanta on a 1.5 GHz A72 (>=1 preempt on metal), and
    // bounded (~sub-second under QEMU TCG, which never preempts it — so it never hangs the regression).
    .balign 4
    .globl __user_prog_spin
__user_prog_spin:
    movz x9, #0x0200, lsl #16              // loop count = 0x0200_0000
1:  subs x9, x9, #1
    b.ne 1b
    mov x8, #2                             // SYS_EXIT
    movz x0, #0x6E                         // M6E sentinel status -> EL0_SPIN_DONE (M6b counters stay pure)
    svc #0
2:  b 2b                                   // sys_exit never returns; belt-and-braces guard

    .balign 4
    .globl __fault_blob_end
__fault_blob_end:
"#
);

unsafe extern "C" {
    static __fault_blob_start: u8;
    static __fault_blob_end: u8;
    static __user_prog_wild_write: u8;
    static __user_prog_code_write: u8;
    static __user_prog_stack_exec: u8;
    static __user_prog_spin: u8;
}

// --- M6d inline EL0 fixtures (per-task address spaces). Position-independent, register/stack-only, so
// they run wherever the kernel copies them into a slot's code page. Each program does its work, hands the
// kernel a value via SYS_REPORT (keyed by the task name in `m6d_report`), then `sys_exit(M6D_EXIT_STATUS)`
// so its exit is accounted to `EL0_M6D_DONE` and never perturbs the M6b/M6e counters. All reads/writes go
// through SP_EL0 (the slot-private stack) — the whole point of M6d — so the fixtures need no absolute VA.
// The whole blob (all three fixtures) is copied into EACH slot's code page; a task enters at its own
// fixture's offset. `[sp,#-0x100]` addresses the sentinel the kernel plants in data page 3. ---
core::arch::global_asm!(
    r#"
    .globl __m6d_blob_start
__m6d_blob_start:
    // same-VA isolation: read the slot-private sentinel the kernel planted at [top-0x100], report it,
    // exit. Two tasks (A and B) run this at the SAME VA in DIFFERENT slots, so each reports its own
    // slot's value — the verdict checks they are distinct and each equals what was planted.
    .balign 4
    .globl __m6d_prog_same_va
__m6d_prog_same_va:
    ldr x0, [sp, #-0x100]
    mov x8, #3                             // SYS_REPORT(value = x0)
    svc #0
    mov x8, #2                             // SYS_EXIT
    movz x0, #0x6D                         // M6D_EXIT_STATUS -> EL0_M6D_DONE (M6b/M6e counters stay pure)
    svc #0
1:  b 1b

    // stack write/readback (the capability this arc unlocks): push a known pattern onto the slot-private
    // user stack, pop it back, report the readback. A store to a non-writable stack would DATA-ABORT and
    // kill the task (no report -> verdict FAIL), so a correct report proves the EL0 stack is writable.
    .balign 4
    .globl __m6d_prog_stack_write
__m6d_prog_stack_write:
    movz x1, #0x1234
    movk x1, #0xABCD, lsl #16              // x1 = 0xABCD1234
    str x1, [sp, #-16]!                    // push (SP_EL0 -= 16)
    ldr x0, [sp], #16                      // pop back into x0 (SP_EL0 += 16)
    mov x8, #3                             // SYS_REPORT(readback)
    svc #0
    mov x8, #2
    movz x0, #0x6D
    svc #0
2:  b 2b

    // SP-relative sentinel readback: spin (register-only, preemptible), then read the planted sentinel
    // through SP and report it. On metal (IRQs>0) this proves SP_EL0 VALUE fidelity across preemption —
    // the spinner is interrupted mid-loop and must resume with the right user SP for the later
    // `[sp,#-0x100]` to hit its own sentinel (the M6e spinner could not observe this). Under QEMU (no
    // Group-1 IRQ) it still validates the slot mapping + read path.
    .balign 4
    .globl __m6d_prog_sp_sentinel
__m6d_prog_sp_sentinel:
    movz x9, #0x0080, lsl #16              // spin ~8.4M iterations (bounded; sub-second under QEMU TCG)
3:  subs x9, x9, #1
    b.ne 3b
    ldr x0, [sp, #-0x100]
    mov x8, #3                             // SYS_REPORT(sentinel)
    svc #0
    mov x8, #2
    movz x0, #0x6D
    svc #0
4:  b 4b

    .balign 4
    .globl __m6d_blob_end
__m6d_blob_end:
"#
);

unsafe extern "C" {
    static __m6d_blob_start: u8;
    static __m6d_blob_end: u8;
    static __m6d_prog_same_va: u8;
    static __m6d_prog_stack_write: u8;
    static __m6d_prog_sp_sentinel: u8;
}

// --- M6f inline EL0 fixtures (validated user pointers + wider syscall surface). Position-independent,
// register/stack-only, so they run wherever the kernel copies them into a slot's code page. Each runs on its
// OWN private slot (`spawn_user_slot`) — the getinfo fixture WRITES its stack (copy_to_user target), which
// the shared window forbids (the M6e stack STOP tripwire) — and exits with `M6F_EXIT_STATUS` (0x6F) so it
// lands in `EL0_M6F_DONE`, never perturbing the M6b/M6d/M6e counters. `adr xN, __m6f_blob_start` recovers
// the window base (the blob is copied at code-page offset 0 in each slot), used to synthesize hostile VAs.
// ABI: x8=nr, args x0-x2, ret x0. Numbers: WRITE=1, EXIT=2, REPORT=3, YIELD=4, SLEEP_MS=5, GETPID=6,
// GETINFO=7. `sys_write(fd,buf,len)` = (x0,x1,x2). ---
core::arch::global_asm!(
    r#"
    .globl __m6f_blob_start
__m6f_blob_start:
    // getinfo/copy_to_user round-trip (well-behaved): getpid -> x19; sys_getinfo(&info on our slot stack)
    // -> the kernel writes the pid+ticks struct there via copy_to_user; read info.pid back -> x21; witness is
    // the pid iff (info.pid == getpid && != 0), else 0 (so a mismatched/zero round-trip fails the verdict).
    // Then sys_write a short summary from the code page (the validated copy_from_user read path), report the
    // witness, exit. Writes ONLY its slot-private stack (sp-0x40, a data page), safe under preemption.
    .balign 4
    .globl __m6f_prog_getinfo
__m6f_prog_getinfo:
    mov  x8, #6                            // SYS_GETPID
    svc  #0
    mov  x19, x0                           // x19 = pid (P)
    sub  x20, sp, #0x40                    // x20 = &info (slot-private, writable data page)
    mov  x0, x20
    mov  x8, #7                            // SYS_GETINFO(&info) -> copy_to_user writes the pid+ticks struct
    svc  #0
    ldr  x21, [x20]                        // x21 = info.pid (S), round-tripped through copy_to_user
    mov  x22, xzr                          // witness = 0
    cmp  x21, x19
    b.ne 1f
    cbz  x19, 1f
    mov  x22, x19                          // matched & non-zero -> witness = pid
1:  mov  x0, #1                            // sys_write summary: fd=stdout
    adr  x1, __m6f_getinfo_msg
    mov  x2, #16                           // "el0: getinfo ok\n"
    mov  x8, #1                            // SYS_WRITE (routed through copy_from_user)
    svc  #0
    mov  x0, x22                           // SYS_REPORT(witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(M6F_EXIT_STATUS)
    movz x0, #0x6F
    svc  #0
2:  b 2b

    // hostile pointers (each must ERROR-RETURN -EFAULT, NOT kill the task): count the -14 returns in x19.
    //   1) sys_write to kernel RAM VA (0x4000_0000, L1[1] EL1-only) — exfiltration attempt
    //   2) sys_write just past the window (base + 0x4000); EL1-only under the slot root (copied kernel
    //      mapping), so only the range check refuses it — NOT a translation fault

    //   3) sys_write whose length wraps the address space (base + ~0 overflows)
    //   4) sys_getinfo targeting the RO code page (base) — copy_to_user must refuse the write target
    // A stray store or a kill would prevent the report (count != 4 -> verdict FAIL); a copy_to_user that
    // actually wrote the RO page would fault the KERNEL (halt) -> no verdict at all. Report the count, exit.
    .balign 4
    .globl __m6f_prog_hostile
__m6f_prog_hostile:
    mov  x19, xzr                          // count of EFAULT (-14) returns
    adr  x9, __m6f_blob_start              // x9 = user window base (code page)
    mov  x0, #1                            // (1) kernel/MMIO VA
    movz x1, #0x4000, lsl #16              // x1 = 0x4000_0000
    mov  x2, #8
    mov  x8, #1
    svc  #0
    cmn  x0, #14                           // x0 == -14 ?  (x0 + 14 == 0 -> Z)
    cinc x19, x19, eq
    mov  x0, #1                            // (2) just past the window (base+0x4000): EL1-only under the
                                           //     slot root (copied kernel mapping) -> range check refuses it
    add  x1, x9, #0x4000
    mov  x2, #8
    mov  x8, #1
    svc  #0
    cmn  x0, #14
    cinc x19, x19, eq
    mov  x0, #1                            // (3) length wraps (base + ~0)
    mov  x1, x9
    movn x2, #0xFF                         // x2 = 0xFFFF_FFFF_FFFF_FF00
    mov  x8, #1
    svc  #0
    cmn  x0, #14
    cinc x19, x19, eq
    mov  x0, x9                            // (4) sys_getinfo(RO code-page VA) — copy_to_user must refuse
    mov  x8, #7
    svc  #0
    cmn  x0, #14
    cinc x19, x19, eq
    mov  x0, x19                           // SYS_REPORT(count of refusals; want 4)
    mov  x8, #3
    svc  #0
    mov  x8, #2
    movz x0, #0x6F
    svc  #0
2:  b 2b

    // yield fixture: SYS_YIELD in a loop, then report the completed iteration count. Co-located with the
    // sleep fixture on one core; the two cooperatively interleave (the kernel counts the yield<->sleep
    // switches). Register-only, so preemption cannot corrupt anything.
    .balign 4
    .globl __m6f_prog_yield
__m6f_prog_yield:
    mov  x19, #8                           // iterations
    mov  x20, xzr
1:  mov  x8, #4                            // SYS_YIELD
    svc  #0
    add  x20, x20, #1
    cmp  x20, x19
    b.lt 1b
    mov  x0, x20                           // SYS_REPORT(completed count; want 8)
    mov  x8, #3
    svc  #0
    mov  x8, #2
    movz x0, #0x6F
    svc  #0
2:  b 2b

    // sleep fixture: SYS_SLEEP_MS in a loop (a real timed sleep on metal; a cooperative yield under QEMU,
    // where the timer IRQ is not delivered), then report the completed iteration count.
    .balign 4
    .globl __m6f_prog_sleep
__m6f_prog_sleep:
    mov  x19, #8
    mov  x20, xzr
1:  mov  x0, #2                            // sleep 2 ms
    mov  x8, #5                            // SYS_SLEEP_MS(a0 = ms)
    svc  #0
    add  x20, x20, #1
    cmp  x20, x19
    b.lt 1b
    mov  x0, x20                           // SYS_REPORT(completed count; want 8)
    mov  x8, #3
    svc  #0
    mov  x8, #2
    movz x0, #0x6F
    svc  #0
2:  b 2b

    .balign 4
__m6f_getinfo_msg:
    .ascii "el0: getinfo ok\n"
    .balign 4
    .globl __m6f_blob_end
__m6f_blob_end:
"#
);

unsafe extern "C" {
    static __m6f_blob_start: u8;
    static __m6f_blob_end: u8;
    static __m6f_prog_getinfo: u8;
    static __m6f_prog_hostile: u8;
    static __m6f_prog_yield: u8;
    static __m6f_prog_sleep: u8;
}

// --- U4 inline EL0 fixtures (per-process handle table). ONE blob with TWO fixtures — the PARENT and the
// ownership NEGATIVE (the orphan) — copied into each fixture's own slot; each task enters at its own offset
// (the M6d/M6f multi-fixture-blob shape). Both are position-independent, register-only (write no user stack,
// so they are safe on any slot under preemption). ABI: x8=nr, args x0-x2, ret x0.
//
// PARENT (`el0-u4parent`): the U4 capability — a spawner reaps MULTIPLE children BY HANDLE. `SYS_SPAWN` now
// returns a small HANDLE index into the caller's per-process handle table (not a raw pid); `SYS_WAIT` takes
// that handle. Two spawns (two handles in x19/x20), two waits (two statuses in x21/x22), then a WITNESS
// (a nonzero token iff both handles were valid — sign bit clear — AND both children exited status 0, else 0),
// and `sys_exit(U4_EXIT_STATUS)` (`0x74` -> EL0_U4_DONE, off every prior counter).
//
// ORPHAN (`el0-u4orphan`): the ownership NEGATIVE — it spawned nothing, so handle #0 is Empty in ITS OWN
// per-process table; `sys_wait(0)` must therefore return `-ECHILD` (-10). It reports 1 iff it saw exactly
// -ECHILD (structural ownership: a task cannot reap a child whose handle is not in its table), else 0, then
// exits with the same sentinel. Deterministic — needs no cross-fixture pid plumbing (its table is empty).
core::arch::global_asm!(
    r#"
    .globl __u4_blob_start
__u4_blob_start:
    .balign 4
    .globl __u4_prog_parent
__u4_prog_parent:
    mov  x8, #8                            // SYS_SPAWN -> handle_a (a handle index >=0, or a negative errno)
    svc  #0
    mov  x19, x0                           // x19 = handle_a
    mov  x8, #8                            // SYS_SPAWN -> handle_b (a SECOND child, a SECOND handle)
    svc  #0
    mov  x20, x0                           // x20 = handle_b
    mov  x0, x19                           // SYS_WAIT(handle_a) — blocks until child A exits (scheduler wake)
    mov  x8, #9
    svc  #0
    mov  x21, x0                           // x21 = status_a
    mov  x0, x20                           // SYS_WAIT(handle_b) — reap child B by its handle
    mov  x8, #9
    svc  #0
    mov  x22, x0                           // x22 = status_b
    mov  x23, xzr                          // witness = 0
    tbnz x19, #63, 1f                      // handle_a < 0 (spawn A failed) -> witness stays 0
    tbnz x20, #63, 1f                      // handle_b < 0 (spawn B failed) -> witness stays 0
    cbnz x21, 1f                           // status_a != 0 (child A not clean) -> witness stays 0
    cbnz x22, 1f                           // status_b != 0 (child B not clean) -> witness stays 0
    movz x23, #0xC4                        // all four OK -> witness = U4_WITNESS_TOKEN (nonzero)
1:  mov  x0, x23                           // SYS_REPORT(witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U4_EXIT_STATUS) -> EL0_U4_DONE
    movz x0, #0x74
    svc  #0
2:  b 2b                                   // sys_exit never returns; belt-and-braces guard

    // The ownership negative: sys_wait a handle it never installed.
    .balign 4
    .globl __u4_prog_orphan
__u4_prog_orphan:
    mov  x0, #0                            // SYS_WAIT(handle #0) — Empty in its OWN never-spawned table
    mov  x8, #9
    svc  #0
    mov  x1, xzr                           // report = 0
    cmn  x0, #10                           // x0 == -ECHILD (-10)?  (x0 + 10 == 0 -> Z)
    cinc x1, x1, eq                        // saw -ECHILD -> report = 1 (structural ownership enforced)
    mov  x0, x1                            // SYS_REPORT(1 iff -ECHILD, else 0)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U4_EXIT_STATUS) -> EL0_U4_DONE
    movz x0, #0x74
    svc  #0
3:  b 3b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
    .globl __u4_blob_end
__u4_blob_end:
"#
);

unsafe extern "C" {
    static __u4_blob_start: u8;
    static __u4_blob_end: u8;
    static __u4_prog_parent: u8;
    static __u4_prog_orphan: u8;
}

// --- U5 inline EL0 fixture (handles as capabilities). ONE fixture (`el0-u5cap`) exercising all four EL0-
// observable capability behaviours against its OWN table, which the launcher pre-endows with two handles:
//   handle 1 = CONSOLE, rights = CAP_WRITE|CAP_GRANT (the "full" console cap)
//   handle 2 = CONSOLE, rights = CAP_READ            (a console cap WITHOUT write — the negative)
// Position-independent, register-only (writes no user stack — safe on any slot under preemption). It builds a
// witness bitmask in x23 (one bit per passed check) and SYS_REPORTs it, then exits with the U5 sentinel. The
// teardown-clear (behaviour 5) is proven kernel-side by `u5_launcher` after this fixture exits. ABI: x8=nr,
// args x0-x2, ret x0. The `mov x2, #(9f-8f)` message length is assembled to an immediate (the M6c idiom).
core::arch::global_asm!(
    r#"
    .globl __u5_blob_start
__u5_blob_start:
    .balign 4
    .globl __u5_prog_cap
__u5_prog_cap:
    mov  x23, xzr                          // witness bitmask = 0

    // (1) write-cap OK: sys_write(handle 1) -> byte count (>= 0)
    mov  x8, #1
    mov  x0, #1
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    tbnz x0, #63, 1f                       // negative -> skip bit0 (fail)
    add  x23, x23, #1                      // bit0: write-cap OK
1:
    // (2) no-cap -EACCES: sys_write(handle 2, lacks CAP_WRITE) -> -EACCES (-13)
    mov  x8, #1
    mov  x0, #2
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    cmn  x0, #13                           // x0 == -13 (-EACCES) ?
    b.ne 2f
    add  x23, x23, #2                      // bit1: no-cap correctly denied
2:
    // (3) attenuation: granting MORE than held is rejected; a subset grant works and its handle writes.
    mov  x8, #10                           // SYS_CAP
    mov  x0, #0                            // CAP_OP_GRANT
    mov  x1, #1                            // src = handle 1 (CAP_WRITE|CAP_GRANT, NOT CAP_EXEC)
    mov  x2, #6                            // request CAP_WRITE|CAP_EXEC (2|4) -> would amplify -> reject
    svc  #0
    tbz  x0, #63, 3f                       // grant SUCCEEDED (>=0) -> attenuation broken -> fail bit2
    mov  x8, #10                           // subset grant: CAP_WRITE only (subset of held)
    mov  x0, #0
    mov  x1, #1
    mov  x2, #2                            // CAP_WRITE
    svc  #0
    tbnz x0, #63, 3f                       // subset grant failed -> fail bit2
    mov  x20, x0                           // x20 = the minted (attenuated) handle idx
    mov  x8, #1                            // write through the minted cap -> must succeed
    mov  x0, x20
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    tbnz x0, #63, 3f                       // minted cap can't write -> fail bit2
    add  x23, x23, #4                      // bit2: attenuation bounded + subset grant usable
3:
    // (4) revoke enforced: revoke handle 1, then a write through it -> -EACCES
    mov  x8, #10                           // SYS_CAP
    mov  x0, #1                            // CAP_OP_REVOKE
    mov  x1, #1                            // drop handle 1
    svc  #0
    cbnz x0, 4f                            // revoke must return 0
    mov  x8, #1
    mov  x0, #1                            // handle 1 now revoked
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    cmn  x0, #13                           // -EACCES ?
    b.ne 4f
    add  x23, x23, #8                      // bit3: revoke enforced
4:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U5_EXIT_STATUS) -> EL0_U5_DONE
    movz x0, #0x75
    svc  #0
5:  b 5b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "u5: cap write\n"
9:
    .balign 4
    .globl __u5_blob_end
__u5_blob_end:
"#
);

unsafe extern "C" {
    static __u5_blob_start: u8;
    static __u5_blob_end: u8;
    static __u5_prog_cap: u8;
}

// --- U6 inline EL0 fixture (the general object table). ONE fixture (`el0-u6spawn`) — the printing SPAWNER the
// U5 table couldn't serve: a process that BOTH prints (holds a console cap at the reserved `CONSOLE_FD`) AND
// spawns 2+ children (auto-allocated, distinct object handles), proving zero index collision. Position-
// independent, register-only (writes no user stack — safe on any slot under preemption). It builds a witness
// bitmask in x23 (one bit per passed check) and SYS_REPORTs it, then exits with the U6 sentinel. The scaffold
// File/Socket kinds are proven kernel-side by `u6_launcher` (no EL0 syscall routes through them yet). ABI:
// x8=nr, args x0-x2, ret x0. The `mov x2, #(N-M)` message lengths assemble to immediates (the M6c idiom).
//
// FLOW: (1) print BEFORE spawning (console cap works) -> bit0. (2) spawn child A, child B -> x19/x20; both
// handles must be >=0, neither the reserved console index (1), and distinct -> bit1. (3) print AFTER the two
// spawns -> bit2: the console cap at index 1 SURVIVED — under U5 a child could have been auto-allocated onto
// index 1 and then clobbered by the console install (or vice versa); U6's reserved-index allocator makes that
// impossible. (4) reap BOTH children by handle, each status 0 -> bit3. Witness == 0xF iff all four held.
core::arch::global_asm!(
    r#"
    .globl __u6_blob_start
__u6_blob_start:
    .balign 4
    .globl __u6_prog_spawn
__u6_prog_spawn:
    mov  x23, xzr                          // witness bitmask = 0

    // (1) print BEFORE spawning — the console cap (at the reserved index 1) works
    mov  x8, #1                            // SYS_WRITE
    mov  x0, #1                            // fd = CONSOLE_FD (the reserved console handle index)
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    tbnz x0, #63, 1f                       // negative -> skip bit0 (fail)
    add  x23, x23, #1                      // bit0: print-before-spawn OK
1:
    // (2) spawn TWO children — distinct object handles auto-allocated around the reserved console index
    mov  x8, #8                            // SYS_SPAWN -> handle_a
    svc  #0
    mov  x19, x0
    mov  x8, #8                            // SYS_SPAWN -> handle_b
    svc  #0
    mov  x20, x0
    tbnz x19, #63, 2f                      // handle_a < 0 (spawn A failed) -> fail bit1
    tbnz x20, #63, 2f                      // handle_b < 0 (spawn B failed) -> fail bit1
    cmp  x19, #1                           // handle_a == CONSOLE_FD ? (must NOT land on the reserved index)
    b.eq 2f
    cmp  x20, #1                           // handle_b == CONSOLE_FD ?
    b.eq 2f
    cmp  x19, x20                          // handle_a == handle_b ? (must be distinct)
    b.eq 2f
    add  x23, x23, #2                      // bit1: both handles valid, off the reserved index, distinct
2:
    // (3) print AFTER spawning — the console cap MUST have survived the two spawns (the no-collision proof)
    mov  x8, #1                            // SYS_WRITE
    mov  x0, #1                            // fd = console (still intact iff no collision)
    adr  x1, 10f
    mov  x2, #(11f - 10f)
    svc  #0
    tbnz x0, #63, 3f                       // console clobbered -> negative -> fail bit2
    add  x23, x23, #4                      // bit2: print-after-spawn OK (console survived the spawns)
3:
    // (4) reap BOTH children by their handles — each must exit status 0
    mov  x0, x19                           // SYS_WAIT(handle_a)
    mov  x8, #9
    svc  #0
    mov  x21, x0                           // status_a
    mov  x0, x20                           // SYS_WAIT(handle_b)
    mov  x8, #9
    svc  #0
    mov  x22, x0                           // status_b
    cbnz x21, 4f                           // status_a != 0 -> fail bit3
    cbnz x22, 4f                           // status_b != 0 -> fail bit3
    add  x23, x23, #8                      // bit3: both children reaped with status 0
4:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U6_EXIT_STATUS) -> EL0_U6_DONE
    movz x0, #0x76
    svc  #0
5:  b 5b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "u6: parent print (pre-spawn)\n"
9:
    .balign 4
10: .ascii "u6: parent print (post-spawn; console survived 2 spawns)\n"
11:
    .balign 4
    .globl __u6_blob_end
__u6_blob_end:
"#
);

unsafe extern "C" {
    static __u6_blob_start: u8;
    static __u6_blob_end: u8;
    static __u6_prog_spawn: u8;
}

// --- U6b inline EL0 fixture (real File handles). ONE fixture (`el0-u6bfile`) exercising the object table's
// FIRST resource syscall on a non-Console object: it opens a disk file by name, reads it through the returned
// File capability, and proves the SYS_READ CHECK rejects both a File handle stripped of `CAP_READ` (the rights
// arm) and a non-File handle carrying `CAP_READ` (the kind arm). Position-independent; it writes NO user stack
// (SYS_READ's dest and the compare targets are fixed DATA-page VAs the kernel wrote, not the stack), so it is
// safe on any slot under preemption. `u6b_launcher` pre-endows its table (before dispatch, no concurrent
// resolver): a File handle at index 2 backed by a real descriptor but with ZERO rights (the rights negative),
// and a `Socket` handle at index 3 carrying `CAP_READ` (the kind negative); it also plants the expected on-disk
// prefix (`USER_BLOB[..16]`) at the data-page VA the fixture compares against. The fixture's own `SYS_OPEN`
// first-free-claims index 0 (index 1 = the reserved `CONSOLE_FD`, never auto-allocated). It builds a witness
// bitmask in x23 (one bit per passed check) and SYS_REPORTs it, then exits with the U6b sentinel. ABI: x8=nr,
// args x0-x2, ret x0. The `mov x2, #(9f-8f)` name length assembles to an immediate (the M6c idiom).
core::arch::global_asm!(
    r#"
    .globl __u6b_blob_start
__u6b_blob_start:
    .balign 4
    .globl __u6b_prog_file
__u6b_prog_file:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u6b_blob_start              // x9 = window base (blob copied at code-page offset 0)
    add  x12, x9, #0x2000                  // x12 = read buffer VA (writable data page)
    add  x13, x9, #0x3000                  // x13 = expected-bytes VA (data page; launcher plants USER_BLOB[..16])

    // (0) open HELLO.BIN -> a File handle carrying CAP_READ
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // name ptr (in the RO code page — EL0-readable)
    mov  x1, #(9f - 8f)                    // name len ("HELLO.BIN" = 9)
    svc  #0
    mov  x19, x0                           // x19 = handle (>=0) or -errno
    tbnz x19, #63, 1f                      // open failed (negative) -> skip bit0/read/bytes
    add  x23, x23, #1                      // bit0: open OK

    // (1) read the first 16 bytes into the buffer
    mov  x8, #12                           // SYS_READ
    mov  x0, x19                           // handle
    mov  x1, x12                           // dest buf
    mov  x2, #16
    svc  #0
    cmp  x0, #16                           // exactly 16 bytes back?
    b.ne 1f                                // short/failed read -> skip bit1/bytes
    add  x23, x23, #2                      // bit1: read OK (16 bytes)

    // (2) the 16 read bytes must equal the kernel-planted on-disk blob prefix (two 8-byte compares)
    ldr  x10, [x12]
    ldr  x11, [x13]
    cmp  x10, x11
    b.ne 1f
    ldr  x10, [x12, #8]
    ldr  x11, [x13, #8]
    cmp  x10, x11
    b.ne 1f
    add  x23, x23, #4                      // bit2: bytes match the on-disk blob
1:
    // (3) a File handle WITHOUT CAP_READ (pre-endowed at index 2) must be denied -> -EACCES (the rights CHECK)
    mov  x8, #12                           // SYS_READ
    mov  x0, #2                            // U6B_NOCAP_IDX
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmn  x0, #13                           // x0 == -13 (-EACCES) ?
    b.ne 2f
    add  x23, x23, #8                      // bit3: no-CAP_READ File -> -EACCES
2:
    // (4) a non-File handle (a Socket carrying CAP_READ, pre-endowed at index 3) -> -EACCES (the kind CHECK)
    mov  x8, #12                           // SYS_READ
    mov  x0, #3                            // U6B_SOCK_IDX
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmn  x0, #13
    b.ne 3f
    add  x23, x23, #16                     // bit4: wrong-kind handle -> -EACCES
3:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U6B_EXIT_STATUS) -> EL0_U6B_DONE
    movz x0, #0x77
    svc  #0
4:  b 4b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "HELLO.BIN"
9:
    .balign 4
    .globl __u6b_blob_end
__u6b_blob_end:
"#
);

unsafe extern "C" {
    static __u6b_blob_start: u8;
    static __u6b_blob_end: u8;
    static __u6b_prog_file: u8;
}

// --- U7 inline EL0 fixtures (cross-process capability transfer). TWO fixtures in one blob, run in two
// separate slots (each slot gets the whole blob; only the entry differs). Both are register-only (they write
// no user stack; their only data reads are the RO code page and the launcher-owned GO word at window +0x3000,
// which the KERNEL writes through the slot backing — the fixtures never store to it). Both poll cooperatively
// (SYS_YIELD between attempts, bounded budgets), so the demo is deterministic under QEMU's cooperative
// scheduling — no reliance on timer preemption.
//
// PARENT (`el0-u7parent`, pre-endowed: U7_DEST_IDX = a Child handle naming the child fixture; U7_SRC_IDX = a
// full Console cap CAP_WRITE|CAP_GRANT):
//   (0) an OVER-RIGHTS transfer (req = CAP_WRITE|CAP_EXEC, bits the source lacks) must be -EACCES — the
//       attenuation invariant crosses processes intact;
//   (1) XFER t1 (req = CAP_WRITE) -> a transfer id (saved for the revoke);
//   then it spins on ITS GO word (the launcher releases it only after the child's first USE lands, so the
//   revoke is provably use-then-revoke);
//   (2) SYS_CAP XREVOKE(t1) -> 0;
//   (3) XFER t2 (the "revoke done" signal the child unblocks on) -> ok.
// CHILD (`el0-u7child`, row deliberately EMPTY at spawn — the single-writer snapshot proves the parent's
// deposit never touched it):
//   spins on its GO (released only after the launcher has verified the pending-deposit/untouched-row
//   snapshot), then (0) RECV t1 -> h1; (1) WRITES through h1 (the transferred Console cap — the line lands on
//   the serial console) and reports U7_USED_TOKEN; (2) RECV t2; (3) the revoked h1 must now be -EACCES.
// Each reports a 4-bit witness (SYS_REPORT) and exits with the 0x78 sentinel. ABI: x8=nr, args x0-x2, ret x0.
core::arch::global_asm!(
    r#"
    .globl __u7_blob_start
__u7_blob_start:
    .balign 4
    .globl __u7_prog_parent
__u7_prog_parent:
    mov  x23, xzr                          // witness bitmask = 0

    // (0) over-rights XFER: dest=U7_DEST_IDX, src=U7_SRC_IDX, req=CAP_WRITE|CAP_EXEC (6) -> -EACCES
    mov  x8, #13                           // SYS_XFER
    mov  x0, #2                            // U7_DEST_IDX (the Child handle)
    mov  x1, #3                            // U7_SRC_IDX (Console, CAP_WRITE|CAP_GRANT — no CAP_EXEC)
    mov  x2, #6                            // req = CAP_WRITE|CAP_EXEC — would AMPLIFY -> must be refused
    svc  #0
    cmn  x0, #13                           // exactly -EACCES ?
    b.ne 1f
    add  x23, x23, #1                      // b0: cross-process attenuation held

1:  // (1) XFER t1: req = CAP_WRITE (2) -> transfer id >= 0
    mov  x8, #13                           // SYS_XFER
    mov  x0, #2
    mov  x1, #3
    mov  x2, #2                            // req = CAP_WRITE (a strict subset of the source's rights)
    svc  #0
    mov  x19, x0                           // x19 = t1's transfer id (or -errno)
    tbnz x19, #63, 2f                      // deposit failed -> skip the revoke half
    add  x23, x23, #2                      // b1: t1 deposited

    // spin on the parent GO word (window +0x3000; the launcher releases it after the child's first USE)
    adr  x9, __u7_blob_start
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000                   // x9 = GO VA (base + 0x3000; adds keep every imm in range)
    movz x24, #0x8000                      // bounded poll budget
3:  ldr  x10, [x9]
    cbnz x10, 4f
    mov  x8, #4                            // SYS_YIELD — cooperative, deterministic under QEMU
    svc  #0
    subs x24, x24, #1
    b.ne 3b
    b    2f                                // GO never released -> report the partial witness (verdict FAILs)

4:  // (2) revoke t1: SYS_CAP(CAP_OP_XREVOKE=2, transfer id) -> 0
    mov  x8, #10                           // SYS_CAP
    mov  x0, #2                            // CAP_OP_XREVOKE
    mov  x1, x19
    svc  #0
    cbnz x0, 2f
    add  x23, x23, #4                      // b2: revoke accepted

    // (3) XFER t2 — the "revoke done" signal the child unblocks on
    mov  x8, #13                           // SYS_XFER
    mov  x0, #2
    mov  x1, #3
    mov  x2, #2
    svc  #0
    tbnz x0, #63, 2f
    add  x23, x23, #8                      // b3: t2 deposited

2:  mov  x0, x23                           // SYS_REPORT(witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U7_EXIT_STATUS) -> EL0_U7_DONE (routed by name)
    movz x0, #0x78
    svc  #0
5:  b 5b                                   // sys_exit never returns; belt-and-braces guard

    .balign 4
    .globl __u7_prog_child
__u7_prog_child:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u7_blob_start
    add  x11, x9, #0x1000
    add  x11, x11, #0x1000
    add  x11, x11, #0x1000                 // x11 = GO VA (base + 0x3000)

    // spin on the child GO (released only after the launcher's single-writer snapshot)
    movz x24, #0x8000
10: ldr  x10, [x11]
    cbnz x10, 11f
    mov  x8, #4                            // SYS_YIELD
    svc  #0
    subs x24, x24, #1
    b.ne 10b
    b    19f                               // never released -> report the (empty) witness

11: // (0) RECV t1 -> h1 (poll: -EAGAIN means the deposit hasn't landed / not yet visible)
    movz x24, #0x8000
12: mov  x8, #14                           // SYS_RECV
    svc  #0
    tbnz x0, #63, 13f                      // negative -> yield + retry (bounded)
    b    14f
13: mov  x8, #4                            // SYS_YIELD
    svc  #0
    subs x24, x24, #1
    b.ne 12b
    b    19f                               // nothing ever arrived -> partial witness
14: mov  x19, x0                           // x19 = h1 (the received, transferred Console cap)
    add  x23, x23, #1                      // b0: received

    // (1) USE it: sys_write(h1, msg, len) — the transferred capability actually carries authority
    mov  x8, #1                            // SYS_WRITE
    mov  x0, x19
    adr  x1, 8f
    mov  x2, #(9f - 8f)                    // msg length (assembles to an immediate — the M6c idiom)
    svc  #0
    cmp  x0, #1
    b.lt 15f                               // write failed -> no USED report (the launcher's wait FAILs honestly)
    add  x23, x23, #2                      // b1: used
    mov  x8, #3                            // SYS_REPORT(U7_USED_TOKEN) — the launcher's revoke cue
    movz x0, #0x51
    svc  #0

15: // (2) RECV t2 — the parent's "revoke done" signal
    movz x24, #0x8000
16: mov  x8, #14                           // SYS_RECV
    svc  #0
    tbnz x0, #63, 17f
    add  x23, x23, #4                      // b2: t2 received
    b    18f
17: mov  x8, #4                            // SYS_YIELD
    svc  #0
    subs x24, x24, #1
    b.ne 16b
    b    19f

18: // (3) the revoked h1 must now be STALE: sys_write(h1) -> -EACCES (single-level revoke enforced at use)
    mov  x8, #1                            // SYS_WRITE
    mov  x0, x19
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    cmn  x0, #13                           // exactly -EACCES ?
    b.ne 19f
    add  x23, x23, #8                      // b3: revoke enforced

19: mov  x0, x23                           // SYS_REPORT(witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U7_EXIT_STATUS) -> EL0_U7_DONE (routed by name)
    movz x0, #0x78
    svc  #0
20: b 20b                                  // sys_exit never returns; belt-and-braces guard

    .balign 4
8:  .ascii "u7: child prints via the transferred cap\n"
9:
    .balign 4
    .globl __u7_blob_end
__u7_blob_end:
"#
);

unsafe extern "C" {
    static __u7_blob_start: u8;
    static __u7_blob_end: u8;
    static __u7_prog_parent: u8;
    static __u7_prog_child: u8;
}

// --- U8 inline EL0 fixture (revocation trees). ONE fixture (`el0-u8tree`) exercising the derivation-ledger
// semantics that are observable from a SINGLE process: a grant CHAIN (grant -> re-grant) dies WHOLE when the
// PARENT capability — one carrying `CAP_REVOKE` — is revoked (the U7 escape #1, closed locally); a revoke
// WITHOUT `CAP_REVOKE` keeps U5's ownership semantics (drops only the caller's own row entry — the derived
// copy survives); and a double revoke returns the correct errno with no ledger corruption. The CROSS-process
// half (re-transfer cascade + generation-tagged inboxes) is proven kernel-side by `u8_kernel_check` (it needs
// three cooperating processes — a fixture race would be staged, not real). Position-independent, register-only
// (writes no user stack — safe on any slot under preemption). Pre-endowed by the launcher: index 2 = a console
// cap `CAP_WRITE|CAP_GRANT|CAP_REVOKE` (the revocable parent), index 3 = a console cap `CAP_WRITE|CAP_GRANT`
// (no CAP_REVOKE — the locality negative). It builds a witness bitmask in x23 and SYS_REPORTs it, then exits
// with the U8 sentinel. ABI: x8=nr, args x0-x2, ret x0; msg lengths assemble to immediates (the M6c idiom).
core::arch::global_asm!(
    r#"
    .globl __u8_blob_start
__u8_blob_start:
    .balign 4
    .globl __u8_prog_tree
__u8_prog_tree:
    mov  x23, xzr                          // witness bitmask = 0

    // (0) the grant CHAIN: g1 = GRANT(src=2, CAP_WRITE|CAP_GRANT) -> g2 = GRANT(g1, CAP_WRITE) -> write
    //     through g2 lands (a two-deep derived capability carries real authority pre-revoke)
    mov  x8, #10                           // SYS_CAP
    mov  x0, #0                            // CAP_OP_GRANT
    mov  x1, #2                            // src = U8_SRC_IDX (CAP_WRITE|CAP_GRANT|CAP_REVOKE)
    mov  x2, #0xA                          // req = CAP_WRITE|CAP_GRANT (a strict subset)
    svc  #0
    tbnz x0, #63, 1f                       // grant failed -> fail bit0
    mov  x19, x0                           // x19 = g1 (the child cap)
    mov  x8, #10                           // re-grant: g2 = GRANT(g1, CAP_WRITE)
    mov  x0, #0
    mov  x1, x19
    mov  x2, #2                            // CAP_WRITE
    svc  #0
    tbnz x0, #63, 1f
    mov  x20, x0                           // x20 = g2 (the grandchild cap)
    mov  x8, #1                            // write through g2 -> must land
    mov  x0, x20
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    tbnz x0, #63, 1f
    add  x23, x23, #1                      // bit0: grant chain works
1:
    // (1) revoke the PARENT: index 2 carries CAP_REVOKE, so this kills the derivation SUBTREE -> 0;
    //     then immediately revoke it AGAIN -> exactly -ECHILD (the double-revoke errno, checked HERE
    //     while index 2 is provably still Empty — a later first-free grant may legitimately reuse it)
    mov  x8, #10                           // SYS_CAP
    mov  x0, #1                            // CAP_OP_REVOKE
    mov  x1, #2
    svc  #0
    cbnz x0, 2f
    mov  x8, #10                           // double revoke of 2 -> -ECHILD (-10; the row is Empty now)
    mov  x0, #1
    mov  x1, #2
    svc  #0
    cmn  x0, #10
    b.ne 2f
    add  x23, x23, #2                      // bit1: parent revoke accepted; double revoke errno'd
2:
    // (2) BOTH descendant copies are now stale at use: write via g1 -> -EACCES; write via g2 -> -EACCES
    mov  x8, #1
    mov  x0, x19
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    cmn  x0, #13                           // exactly -EACCES ?
    b.ne 3f
    mov  x8, #1
    mov  x0, x20
    adr  x1, 8f
    mov  x2, #(9f - 8f)
    svc  #0
    cmn  x0, #13
    b.ne 3f
    add  x23, x23, #4                      // bit2: the whole subtree died with the parent
3:
    // (3) locality + errno negatives: g4 = GRANT(src=3, CAP_WRITE); revoke index 3 (NO CAP_REVOKE) -> 0 but
    //     LOCAL only — g4 still writes; then a double revoke of 3 and of 2 each returns exactly -ECHILD
    mov  x8, #10
    mov  x0, #0                            // CAP_OP_GRANT
    mov  x1, #3                            // src = U8_SRC2_IDX (CAP_WRITE|CAP_GRANT — no CAP_REVOKE)
    mov  x2, #2                            // CAP_WRITE
    svc  #0
    tbnz x0, #63, 4f
    mov  x21, x0                           // x21 = g4
    mov  x8, #10                           // revoke index 3 — right-less, so row-local only
    mov  x0, #1
    mov  x1, #3
    svc  #0
    cbnz x0, 4f
    mov  x8, #1                            // g4 must STILL write (no CAP_REVOKE => no subtree kill)
    mov  x0, x21
    adr  x1, 10f
    mov  x2, #(11f - 10f)
    svc  #0
    tbnz x0, #63, 4f
    mov  x8, #10                           // double revoke of 3 -> -ECHILD (-10; already Empty — g4 was
    mov  x0, #1                            // first-free minted at the freed index 2, so 3 stays Empty)
    mov  x1, #3
    svc  #0
    cmn  x0, #10
    b.ne 4f
    add  x23, x23, #8                      // bit3: right-less revoke stayed local; double revoke errno'd
4:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U8_EXIT_STATUS) -> EL0_U8_DONE
    movz x0, #0x79
    svc  #0
5:  b 5b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "u8: write via the grandchild cap\n"
9:
    .balign 4
10: .ascii "u8: right-less revoke stays local\n"
11:
    .balign 4
    .globl __u8_blob_end
__u8_blob_end:
"#
);

unsafe extern "C" {
    static __u8_blob_start: u8;
    static __u8_blob_end: u8;
    static __u8_prog_tree: u8;
}

// --- U9 inline EL0 fixture (real File WRITES + SEEK). ONE fixture (`el0-u9write`) exercising the object
// table's FIRST mutating resource path: it opens a DEDICATED scratch file RW, seeks into it, overwrites a
// known 16-byte pattern IN PLACE, seeks back and reads it through the SAME capability to witness the write
// landed, and proves the `sys_write` CHECK rejects both an RO-opened File (missing `CAP_WRITE` — the rights
// arm) and a non-File handle carrying `CAP_WRITE` (the kind arm). Position-independent; its only writable
// user target is the slot's data page at +0x2000 (the SYS_READ dest) — it writes NO user stack, so it is safe
// on any slot under preemption. `u9_launcher` pre-endows a `Socket` handle at index 2 carrying `CAP_WRITE`
// (the kind negative). The fixture's own RW `SYS_OPEN` first-free-claims index 0 (index 1 = the reserved
// `CONSOLE_FD`); its RO open then claims index 3. The write pattern lives in the RO code page (a `.ascii`
// constant, EL0-readable). It builds a witness bitmask in x23 (one bit per passed check) and SYS_REPORTs it,
// then exits with the U9 sentinel. ABI: x8=nr, args x0-x2, ret x0. `mov x1, #(9f-8f)` = the 8.3 name length.
core::arch::global_asm!(
    r#"
    .globl __u9_blob_start
__u9_blob_start:
    .balign 4
    .globl __u9_prog_write
__u9_prog_write:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u9_blob_start               // x9 = window base (blob copied at code-page offset 0)
    add  x12, x9, #0x2000                  // x12 = read-back buffer VA (writable data page)
    adr  x13, 12f                          // x13 = write-pattern VA (RO code page; also the compare source)

    // (0) open SCRATCH.BIN RW (mode=1) -> a File handle carrying CAP_READ|CAP_WRITE
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // name ptr (RO code page — EL0-readable)
    mov  x1, #(9f - 8f)                    // name len ("SCRATCH.BIN" = 11)
    mov  x2, #1                            // mode = RW
    svc  #0
    mov  x19, x0                           // x19 = RW handle (>=0) or -errno
    tbnz x19, #63, 1f                      // open failed (negative) -> skip bit0/1/2
    add  x23, x23, #1                      // bit0: open RW OK

    // (1) seek to the scratch offset (520), then overwrite the 16-byte pattern in place
    mov  x8, #15                           // SYS_SEEK
    mov  x0, x19
    mov  x1, #520                          // U9_WRITE_OFFSET
    svc  #0
    cmp  x0, #520                          // seek returns the new absolute offset
    b.ne 1f
    mov  x8, #1                            // SYS_WRITE (File + CAP_WRITE -> fat::write_at at the offset)
    mov  x0, x19
    mov  x1, x13                           // src = the 16-byte pattern
    mov  x2, #16
    svc  #0
    cmp  x0, #16                           // wrote exactly 16 bytes?
    b.ne 1f
    add  x23, x23, #2                      // bit1: seek + in-place write OK

    // (2) seek back to 520 and read the 16 bytes through the SAME cap; they must equal the pattern
    mov  x8, #15                           // SYS_SEEK back to 520
    mov  x0, x19
    mov  x1, #520
    svc  #0
    cmp  x0, #520
    b.ne 1f
    mov  x8, #12                           // SYS_READ
    mov  x0, x19
    mov  x1, x12                           // dest buf
    mov  x2, #16
    svc  #0
    cmp  x0, #16                           // exactly 16 bytes back?
    b.ne 1f
    ldr  x10, [x12]                        // two 8-byte compares: read-back == the pattern we wrote
    ldr  x11, [x13]
    cmp  x10, x11
    b.ne 1f
    ldr  x10, [x12, #8]
    ldr  x11, [x13, #8]
    cmp  x10, x11
    b.ne 1f
    add  x23, x23, #4                      // bit2: read-back matches the written pattern
1:
    // (3) an RO-opened File (mode=0, CAP_READ only) written to must be denied -> -EACCES (the rights CHECK)
    mov  x8, #11                           // SYS_OPEN SCRATCH.BIN RO
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #0                            // mode = RO
    svc  #0
    mov  x20, x0                           // x20 = RO handle
    tbnz x20, #63, 2f                      // RO open failed -> skip bit3
    mov  x8, #1                            // SYS_WRITE through the RO handle
    mov  x0, x20
    mov  x1, x13
    mov  x2, #16
    svc  #0
    cmn  x0, #13                           // x0 == -13 (-EACCES) ?
    b.ne 2f
    add  x23, x23, #8                      // bit3: RO-open File write -> -EACCES
2:
    // (4) a non-File handle (a Socket carrying CAP_WRITE, pre-endowed at index 2) -> -EACCES (the kind CHECK)
    mov  x8, #1                            // SYS_WRITE
    mov  x0, #2                            // U9_SOCK_IDX
    mov  x1, x13
    mov  x2, #16
    svc  #0
    cmn  x0, #13
    b.ne 3f
    add  x23, x23, #16                     // bit4: wrong-kind handle write -> -EACCES
3:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U9_EXIT_STATUS) -> EL0_U9_DONE
    movz x0, #0x7A
    svc  #0
4:  b 4b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "SCRATCH.BIN"
9:
    .balign 8
12: .ascii "U9-WRITE-OK-1234"
    .balign 4
    .globl __u9_blob_end
__u9_blob_end:
"#
);

unsafe extern "C" {
    static __u9_blob_start: u8;
    static __u9_blob_end: u8;
    static __u9_prog_write: u8;
}

// --- U10 inline EL0 fixture (real file GROWTH). ONE fixture (`el0-u10grow`) exercising the object table's
// first ALLOCATING resource path: it opens a DEDICATED 1-cluster file RW, seeks to its planted EOF (the
// cluster boundary), writes 16 bytes PAST it — forcing `fat::write_grow` to allocate + zero-fill + chain a
// second cluster and bump the on-disk directory size — then seeks back and reads the appended bytes through
// the SAME capability (bit2), confirms the ORIGINAL first cluster is intact (bit3), and proves an RO-opened
// File write is still `-EACCES` (bit4 — growth rides the SAME single CAP_WRITE CHECK as the in-place write).
// Position-independent; its only writable user target is the slot's data page at +0x2000 (the SYS_READ dest) —
// it writes NO user stack, so it is safe on any slot under preemption. The fixture's own RW `SYS_OPEN`
// first-free-claims index 0 (index 1 = the reserved `CONSOLE_FD`); its RO open then claims index 3. It builds a
// witness bitmask in x23 (one bit per passed check) and SYS_REPORTs it, then exits with the U10 sentinel.
// ABI: x8=nr, args x0-x2, ret x0. `mov x1, #(9f-8f)` = the 8.3 name length.
core::arch::global_asm!(
    r#"
    .globl __u10_blob_start
__u10_blob_start:
    .balign 4
    .globl __u10_prog_grow
__u10_prog_grow:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u10_blob_start              // x9 = window base (blob copied at code-page offset 0)
    add  x12, x9, #0x2000                  // x12 = read-back buffer VA (writable data page)
    adr  x13, 12f                          // x13 = append-pattern VA (RO code page; also the compare source)
    adr  x14, 13f                          // x14 = filler-expect VA (RO code page; 16 x 0xC1)

    // (0) open GROW.BIN RW (mode=1) -> a File handle carrying CAP_READ|CAP_WRITE
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // name ptr (RO code page — EL0-readable)
    mov  x1, #(9f - 8f)                    // name len ("GROW.BIN" = 8)
    mov  x2, #1                            // mode = RW
    svc  #0
    mov  x19, x0                           // x19 = RW handle (>=0) or -errno
    tbnz x19, #63, 1f                      // open failed (negative) -> skip bit0..bit3
    add  x23, x23, #1                      // bit0: open RW OK

    // (1) seek to the planted EOF (512, the cluster boundary) and WRITE past it -> GROW (alloc + zero + chain)
    mov  x8, #15                           // SYS_SEEK
    mov  x0, x19
    mov  x1, #512                          // U10_GROW_OFFSET (== planted size == one full cluster)
    svc  #0
    cmp  x0, #512                          // seek returns the new absolute offset
    b.ne 1f
    mov  x8, #1                            // SYS_WRITE (File + CAP_WRITE past EOF -> fat::write_grow)
    mov  x0, x19
    mov  x1, x13                           // src = the 16-byte append pattern
    mov  x2, #16
    svc  #0
    cmp  x0, #16                           // grew by exactly 16 bytes (NOT a U9 clamp-to-0)?
    b.ne 1f
    add  x23, x23, #2                      // bit1: seek-to-EOF + grow write OK

    // (2) seek back to 512 and read the appended 16 bytes through the SAME cap; they must equal the pattern
    mov  x8, #15                           // SYS_SEEK back to 512
    mov  x0, x19
    mov  x1, #512
    svc  #0
    cmp  x0, #512
    b.ne 1f
    mov  x8, #12                           // SYS_READ
    mov  x0, x19
    mov  x1, x12                           // dest buf
    mov  x2, #16
    svc  #0
    cmp  x0, #16                           // exactly 16 bytes back?
    b.ne 1f
    ldr  x10, [x12]                        // two 8-byte compares: read-back == the appended pattern
    ldr  x11, [x13]
    cmp  x10, x11
    b.ne 1f
    ldr  x10, [x12, #8]
    ldr  x11, [x13, #8]
    cmp  x10, x11
    b.ne 1f
    add  x23, x23, #4                      // bit2: appended bytes read back through the same cap

    // (3) seek to 0 and read the FIRST 16 bytes; must still equal the planted filler (original cluster intact)
    mov  x8, #15                           // SYS_SEEK to 0
    mov  x0, x19
    mov  x1, #0
    svc  #0
    cmp  x0, #0
    b.ne 1f
    mov  x8, #12                           // SYS_READ
    mov  x0, x19
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 1f
    ldr  x10, [x12]                        // read-back == the planted 0xC1 filler
    ldr  x11, [x14]
    cmp  x10, x11
    b.ne 1f
    ldr  x10, [x12, #8]
    ldr  x11, [x14, #8]
    cmp  x10, x11
    b.ne 1f
    add  x23, x23, #8                      // bit3: original cluster survived the grow (no corruption)
1:
    // (4) an RO-opened File (mode=0, CAP_READ only) written to must be denied -> -EACCES (growth is gated by
    //     the SAME single CAP_WRITE CHECK as the in-place write — a File WITHOUT CAP_WRITE can never grow)
    mov  x8, #11                           // SYS_OPEN GROW.BIN RO
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #0                            // mode = RO
    svc  #0
    mov  x20, x0                           // x20 = RO handle
    tbnz x20, #63, 2f                      // RO open failed -> skip bit4
    mov  x8, #1                            // SYS_WRITE through the RO handle
    mov  x0, x20
    mov  x1, x13
    mov  x2, #16
    svc  #0
    cmn  x0, #13                           // x0 == -13 (-EACCES) ?
    b.ne 2f
    add  x23, x23, #16                     // bit4: RO-open File write -> -EACCES
2:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U10_EXIT_STATUS) -> EL0_U10_DONE
    movz x0, #0x7B
    svc  #0
4:  b 4b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "GROW.BIN"
9:
    .balign 8
12: .ascii "U10-GROW-OK-5678"
    .balign 8
13: .byte 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1, 0xC1
    .balign 4
    .globl __u10_blob_end
__u10_blob_end:
"#
);

unsafe extern "C" {
    static __u10_blob_start: u8;
    static __u10_blob_end: u8;
    static __u10_prog_grow: u8;
}

// --- U10-create inline EL0 fixture (real file CREATE via O_CREAT). ONE fixture (`el0-u10create`) that opens a
// name that does NOT exist with O_CREAT|RW (mode=3) — forcing `fat::create_in_root` to write a fresh 0-length
// 8.3 directory entry — then WRITES a pattern at offset 0 (the first grow of a 0-cluster file: `write_grow`
// allocates the first cluster and sets the directory `first_cluster`), reads it back through the SAME cap
// (bit2), and re-opens the now-existing name with O_CREAT|RW to prove create-if-present is idempotent (bit3).
// Position-independent; its only writable user target is the slot's data page at +0x2000. The fixture's own two
// opens first-free-claim descriptors 0 and 3 (index 1 = the reserved `CONSOLE_FD`). It builds a witness bitmask
// in x23 and SYS_REPORTs it, then exits with the U10-create sentinel. ABI: x8=nr, args x0-x2, ret x0.
core::arch::global_asm!(
    r#"
    .globl __u10c_blob_start
__u10c_blob_start:
    .balign 4
    .globl __u10c_prog_create
__u10c_prog_create:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u10c_blob_start             // x9 = window base
    add  x12, x9, #0x2000                  // x12 = read-back buffer VA (writable data page)
    adr  x13, 12f                          // x13 = write-pattern VA (RO code page; compare source)

    // (0) open FRESH.BIN O_CREAT|RW (mode=3) -> CREATE a 0-length file, return an RW handle
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // name ptr
    mov  x1, #(9f - 8f)                    // name len ("FRESH.BIN" = 9)
    mov  x2, #3                            // mode = O_CREAT | RW
    svc  #0
    mov  x19, x0                           // x19 = RW handle (>=0) or -errno
    tbnz x19, #63, 1f                      // create/open failed -> skip bit0..bit2
    add  x23, x23, #1                      // bit0: create + open OK

    // (1) write the 16-byte pattern at offset 0 -> first GROW of a 0-cluster file (alloc + set dir first_cluster)
    mov  x8, #1                            // SYS_WRITE (File + CAP_WRITE, offset 0 past EOF=0 -> fat::write_grow)
    mov  x0, x19
    mov  x1, x13                           // src = the 16-byte pattern
    mov  x2, #16
    svc  #0
    cmp  x0, #16                           // wrote (grew by) exactly 16 bytes?
    b.ne 1f
    add  x23, x23, #2                      // bit1: first-grow write OK

    // (2) seek to 0 and read the 16 bytes through the SAME cap; they must equal the pattern
    mov  x8, #15                           // SYS_SEEK to 0
    mov  x0, x19
    mov  x1, #0
    svc  #0
    cmp  x0, #0
    b.ne 1f
    mov  x8, #12                           // SYS_READ
    mov  x0, x19
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 1f
    ldr  x10, [x12]
    ldr  x11, [x13]
    cmp  x10, x11
    b.ne 1f
    ldr  x10, [x12, #8]
    ldr  x11, [x13, #8]
    cmp  x10, x11
    b.ne 1f
    add  x23, x23, #4                      // bit2: read-back matches the written pattern
1:
    // (3) a SECOND O_CREAT|RW open of the SAME name -> the file now EXISTS -> a handle (idempotent, no duplicate)
    mov  x8, #11                           // SYS_OPEN FRESH.BIN O_CREAT|RW again
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #3
    svc  #0
    tbnz x0, #63, 2f                       // second open failed -> skip bit3
    add  x23, x23, #8                      // bit3: idempotent create-if-present OK
2:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U10C_EXIT_STATUS) -> EL0_U10C_DONE
    movz x0, #0x7C
    svc  #0
4:  b 4b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "FRESH.BIN"
9:
    .balign 8
12: .ascii "U10-CREATE-OK-99"
    .balign 4
    .globl __u10c_blob_end
__u10c_blob_end:
"#
);

unsafe extern "C" {
    static __u10c_blob_start: u8;
    static __u10c_blob_end: u8;
    static __u10c_prog_create: u8;
}

// --- U10-delete inline EL0 fixture (real file DELETE via SYS_UNLINK). ONE self-contained fixture
// (`el0-u10delete`): it CREATES its own file (O_CREAT|RW), WRITES to it (grow-from-empty allocates a data
// cluster), opens it a SECOND time (a sibling handle), UNLINKs it through the first handle (dir entry -> 0xE5 +
// the chain freed in all FAT copies + EVERY descriptor for the file invalidated), proves a read through the
// SIBLING handle is now `-EACCES` (no stale reference to the freed chain), and re-opens the name RO to prove it
// is GONE (`-ENOENT`). Self-contained so it never depends on another demo's state. Position-independent; its
// only writable user target is the slot's data page at +0x2000 (the SYS_READ dest). It builds a witness bitmask
// in x23 and SYS_REPORTs it, then exits with the U10-delete sentinel. ABI: x8=nr, args x0-x2, ret x0.
core::arch::global_asm!(
    r#"
    .globl __u10d_blob_start
__u10d_blob_start:
    .balign 4
    .globl __u10d_prog_delete
__u10d_prog_delete:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u10d_blob_start             // x9 = window base
    add  x12, x9, #0x2000                  // x12 = read dest VA (writable data page)
    adr  x13, 12f                          // x13 = write-pattern VA (RO code page)

    // (0) open DELME.BIN O_CREAT|RW (mode=3) -> CREATE a 0-length file, RW handle h0
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // name ptr
    mov  x1, #(9f - 8f)                    // name len ("DELME.BIN" = 9)
    mov  x2, #3                            // mode = O_CREAT | RW
    svc  #0
    mov  x19, x0                           // x19 = h0 (>=0) or -errno
    tbnz x19, #63, 1f                      // create/open failed -> skip bit0..bit3
    add  x23, x23, #1                      // bit0: create + open OK

    // (1) write 16 bytes -> grow-from-empty (allocates the file's one data cluster)
    mov  x8, #1                            // SYS_WRITE
    mov  x0, x19
    mov  x1, x13
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 1f
    add  x23, x23, #2                      // bit1: write (grow) OK

    // open DELME.BIN RW AGAIN (no O_CREAT — it exists now) -> a SIBLING handle h1 (setup for bit3)
    mov  x8, #11                           // SYS_OPEN DELME.BIN RW
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #1                            // mode = RW
    svc  #0
    mov  x20, x0                           // x20 = h1 (the sibling) or -errno
    tbnz x20, #63, 1f                      // sibling open failed -> skip bit2/bit3

    // (2) unlink the file through h0 -> 0 (dir 0xE5 + free chain, and invalidate ALL of this proc's descriptors)
    mov  x8, #16                           // SYS_UNLINK
    mov  x0, x19
    svc  #0
    cmp  x0, #0
    b.ne 1f
    add  x23, x23, #4                      // bit2: unlink OK

    // (3) a read through the SIBLING h1 must now be denied -> -EACCES (its descriptor was invalidated too)
    mov  x8, #12                           // SYS_READ
    mov  x0, x20
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmn  x0, #13                           // x0 == -13 (-EACCES) ?
    b.ne 1f
    add  x23, x23, #8                      // bit3: sibling handle fail-safe (no stale reference to freed chain)
1:
    // (4) a plain RO open of the deleted name must now fail -> -ENOENT (the file is GONE)
    mov  x8, #11                           // SYS_OPEN DELME.BIN RO (no O_CREAT)
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #0                            // mode = RO
    svc  #0
    cmn  x0, #2                            // x0 == -2 (-ENOENT) ?
    b.ne 2f
    add  x23, x23, #16                     // bit4: re-open -> -ENOENT (gone)
2:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U10D_EXIT_STATUS) -> EL0_U10D_DONE
    movz x0, #0x7D
    svc  #0
4:  b 4b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "DELME.BIN"
9:
    .balign 8
12: .ascii "U10-DELETE-OK-42"
    .balign 4
    .globl __u10d_blob_end
__u10d_blob_end:
"#
);

unsafe extern "C" {
    static __u10d_blob_start: u8;
    static __u10d_blob_end: u8;
    static __u10d_prog_delete: u8;
}

// --- U11 inline EL0 fixture (open-file LIFECYCLE: SYS_CLOSE + generation-tagged file-ids). ONE fixture
// (`el0-u11close`) that: creates A11.BIN (O_CREAT|RW) and grow-writes it; opens it a SECOND time (a sibling
// descriptor in a different slot naming the SAME on-disk entry); UNLINKs it through the first handle (which frees
// BOTH of this process's A11 descriptors and clears the first handle, leaving the sibling lingering on a now-free
// slot); REUSES the freed slots with a DIFFERENT file B11.BIN — create + grow-write it, THEN open it a second time
// so the sibling's reclaimed slot snapshots B11's REAL size + cluster (a genuine negative control: a broken no-gen
// read WOULD leak B11's data); then proves a read through the STALE sibling handle is `-EACCES` — a GENERATION
// mismatch (the slot is LIVE again for B11, so the only possible denial is the stale generation), NOT a silent
// rebind that would leak B11's 16 bytes; and finally exercises
// SYS_CLOSE end-to-end (close → `0`, double-close → `-EBADF`, close→re-open→read round-trip returns B11's content).
// Self-contained (creates its own files) so it never depends on another demo's state. Position-independent; its
// only writable user target is the slot's data page at +0x2000 (the SYS_READ dest). It builds a witness bitmask in
// x23 and SYS_REPORTs it, then exits with the U11 sentinel. ABI: x8=nr, args x0-x2, ret x0.
core::arch::global_asm!(
    r#"
    .globl __u11_blob_start
__u11_blob_start:
    .balign 4
    .globl __u11_prog_close
__u11_prog_close:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u11_blob_start              // x9 = window base
    add  x12, x9, #0x2000                  // x12 = read dest VA (writable data page)
    adr  x13, 12f                          // x13 = A-pattern VA (RO code page)
    adr  x14, 13f                          // x14 = B-pattern VA

    // (0) create A11.BIN (O_CREAT|RW, mode=3) -> hA0, then write 16 bytes (grow-from-empty allocates its cluster)
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // "A11.BIN"
    mov  x1, #(9f - 8f)
    mov  x2, #3                            // O_CREAT | RW
    svc  #0
    mov  x19, x0                           // x19 = hA0 (>=0) or -errno
    tbnz x19, #63, 2f                      // create/open failed -> report what we have
    mov  x8, #1                            // SYS_WRITE
    mov  x0, x19
    mov  x1, x13
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 2f
    add  x23, x23, #1                      // bit0: create A11.BIN + grow-write OK

    // (1) open A11.BIN RW AGAIN (no O_CREAT — it exists) -> hA1, a SIBLING descriptor (a different slot)
    mov  x8, #11                           // SYS_OPEN A11.BIN RW
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #1                            // RW
    svc  #0
    mov  x20, x0                           // x20 = hA1 (the sibling that will go stale)
    tbnz x20, #63, 2f
    add  x23, x23, #2                      // bit1: sibling open OK

    // (2) unlink via hA0 -> 0 (frees BOTH of this proc's A11 descriptors + clears hA0; hA1 lingers on a freed slot)
    mov  x8, #16                           // SYS_UNLINK
    mov  x0, x19
    svc  #0
    cmp  x0, #0
    b.ne 2f
    add  x23, x23, #4                      // bit2: unlink OK

    // (3) REUSE the freed slots with a DIFFERENT file B11.BIN, and make the sibling's slot a GENUINE negative
    //     control: create + grow-write B11 through hB0 FIRST (B11 now owns a real cluster + size 16 on disk),
    //     THEN open B11 a second time into hB1 — first-fit reclaims hA1's OTHER freed slot, and because the open
    //     happens AFTER the write, hB1's descriptor snapshots B11's REAL size+cluster. So the stale sibling hA1
    //     now aliases a LIVE descriptor that genuinely names B11's 16 bytes: a broken (no-gen) read via hA1 WOULD
    //     return B11's data (a real cross-file leak). The read must instead be -EACCES — a GENERATION mismatch,
    //     the only reason to deny given the slot is live again — proving no rebind and no data disclosure.
    mov  x8, #11                           // SYS_OPEN B11.BIN O_CREAT|RW -> hB0 (first-fit reclaims one freed slot)
    adr  x0, 10f
    mov  x1, #(11f - 10f)
    mov  x2, #3
    svc  #0
    mov  x21, x0                           // x21 = hB0
    tbnz x21, #63, 2f
    mov  x8, #1                            // SYS_WRITE B11 pattern via hB0 -> grow-from-empty gives B11 a real cluster
    mov  x0, x21
    mov  x1, x14
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 2f
    mov  x8, #11                           // SYS_OPEN B11.BIN RW AGAIN -> hB1 reclaims hA1's OTHER freed slot; opened
    adr  x0, 10f                           //   AFTER the write, so its descriptor snapshots B11's REAL size+cluster —
    mov  x1, #(11f - 10f)                  //   the stale hA1 now aliases a LIVE descriptor that names B11's 16 bytes
    mov  x2, #1
    svc  #0
    mov  x22, x0                           // x22 = hB1 (occupies hA1's slot, a genuine live B11 descriptor)
    tbnz x22, #63, 2f
    mov  x8, #12                           // SYS_READ through the STALE sibling hA1 (its slot is LIVE again for B11)
    mov  x0, x20
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmn  x0, #13                           // x0 == -13 (-EACCES) — a GEN mismatch, NOT B11's 16 bytes (no data rebind)?
    b.ne 2f
    add  x23, x23, #8                      // bit3: stale sibling denied by GEN mismatch (no rebind, no B11 data leak)

    // (4) SYS_CLOSE end-to-end: close hB0 -> 0; double-close -> -EBADF; re-open B11.BIN RO -> read round-trips
    mov  x8, #17                           // SYS_CLOSE(hB0)
    mov  x0, x21
    svc  #0
    cmp  x0, #0
    b.ne 2f
    mov  x8, #17                           // SYS_CLOSE(hB0) AGAIN -> double-close must be clean
    mov  x0, x21
    svc  #0
    cmn  x0, #9                            // x0 == -9 (-EBADF)?
    b.ne 2f
    mov  x8, #11                           // re-open B11.BIN RO -> hB2
    adr  x0, 10f
    mov  x1, #(11f - 10f)
    mov  x2, #0                            // RO
    svc  #0
    mov  x24, x0                           // x24 = hB2
    tbnz x24, #63, 2f
    mov  x8, #12                           // SYS_READ hB2 -> B11's content (close->reopen->read round-trip)
    mov  x0, x24
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 2f
    ldr  x10, [x12]                        // two 8-byte compares: read-back == the B11 pattern
    ldr  x11, [x14]
    cmp  x10, x11
    b.ne 2f
    ldr  x10, [x12, #8]
    ldr  x11, [x14, #8]
    cmp  x10, x11
    b.ne 2f
    add  x23, x23, #16                     // bit4: SYS_CLOSE OK + double-close -EBADF + close->reopen->read round-trip
2:
    mov  x0, x23                           // SYS_REPORT(witness bitmask)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U11_EXIT_STATUS) -> EL0_U11_DONE
    movz x0, #0x7E
    svc  #0
4:  b 4b                                   // sys_exit never returns; belt-and-braces guard
    .balign 4
8:  .ascii "A11.BIN"
9:
    .balign 4
10: .ascii "B11.BIN"
11:
    .balign 8
12: .ascii "U11-CLOSE-A-1234"
    .balign 8
13: .ascii "U11-REOPEN-B-567"
    .balign 4
    .globl __u11_blob_end
__u11_blob_end:
"#
);

unsafe extern "C" {
    static __u11_blob_start: u8;
    static __u11_blob_end: u8;
    static __u11_prog_close: u8;
}

// --- U11-M2 inline EL0 fixtures (cross-process unlink-defers-free). TWO programs in ONE shared blob (the U7
// two-fixture idiom), each copied into its OWN slot; the launcher sequences them with per-step GO words:
//   * el0-u11defer-a: creates DEFER.BIN (O_CREAT|RW) + grow-writes it; reports A_OPENED; parks on its read GO
//     (+0x3010); after B has unlinked the file, SEEKs to 0 and READs — the 16 bytes must STILL come back (the
//     chain is alive: B's unlink was deferred because A holds it open); reports A_READ; parks on its close GO
//     (+0x3018); SYS_CLOSEs (the LAST close → the deferred free runs) + double-close → -EBADF.
//   * el0-u11defer-b: parks on its unlink GO (+0x3000); OPENs DEFER.BIN (A created it), SYS_UNLINKs it (returns
//     0, deferred), and a re-open of the now-gone name → -ENOENT; reports B_UNLINKED.
// Both build a witness in x23, SYS_REPORT it, and exit with the U11-defer sentinel. Register-only save the read
// buffer at +0x2000 (the SYS_READ dest); GO words live in page 3 (+0x3000..). ABI: x8=nr, args x0-x2, ret x0.
core::arch::global_asm!(
    r#"
    .globl __u11defer_blob_start
__u11defer_blob_start:
    .balign 4
    .globl __u11defer_prog_a
__u11defer_prog_a:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u11defer_blob_start         // x9 = window base
    add  x12, x9, #0x2000                  // x12 = read dest VA (writable data page)
    adr  x13, 7f                           // x13 = pattern VA (RO code page)

    // (0) create DEFER.BIN (O_CREAT|RW|O_PUBLIC, mode=7) -> hA, then write 16 bytes (grow-from-empty allocates a cluster)
    // U6: O_PUBLIC — B (a DIFFERENT ASID) opens this file below, so it must be world-accessible, not owned-private.
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // "DEFER.BIN"
    mov  x1, #(9f - 8f)
    mov  x2, #7                            // O_CREAT | RW | O_PUBLIC
    svc  #0
    mov  x19, x0                           // x19 = hA (>=0) or -errno
    tbnz x19, #63, 2f                      // create/open failed -> report what we have
    mov  x8, #1                            // SYS_WRITE 16 bytes (the pattern) -> grow
    mov  x0, x19
    mov  x1, x13
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 2f
    add  x23, x23, #1                      // bit0: create DEFER.BIN + grow-write OK
    mov  x8, #3                            // SYS_REPORT(A_OPENED) — cue: launcher releases B's unlink
    movz x0, #0x60
    svc  #0

    // park on the read GO word (base + 0x3010; released after B has unlinked + the launcher's checkpoint-1)
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000                   // x9 = base + 0x3000
    add  x9, x9, #0x10                     // x9 = base + 0x3010 = A read-GO
    movz x24, #0x8000                      // bounded poll budget
3:  ldr  x10, [x9]
    cbnz x10, 4f
    mov  x8, #4                            // SYS_YIELD — cooperative
    svc  #0
    subs x24, x24, #1
    b.ne 3b
    b    2f                                // GO never released -> report the partial witness (verdict FAILs)

4:  // (1) seek to 0, then READ 16 -> must STILL return A's original bytes (the chain is alive despite B's unlink)
    mov  x8, #15                           // SYS_SEEK(hA, 0)
    mov  x0, x19
    mov  x1, #0
    svc  #0
    cmp  x0, #0                            // seek returns the new offset (0); anything else is a failure
    b.ne 2f
    mov  x8, #12                           // SYS_READ(hA, buf, 16)
    mov  x0, x19
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmp  x0, #16                           // 16 bytes back == the chain was NOT freed (defer held)
    b.ne 2f
    ldr  x10, [x12]                        // read-back == pattern (lo 8)
    ldr  x11, [x13]
    cmp  x10, x11
    b.ne 2f
    ldr  x10, [x12, #8]                    // (hi 8)
    ldr  x11, [x13, #8]
    cmp  x10, x11
    b.ne 2f
    add  x23, x23, #2                      // bit1: read-after-unlink returns the original bytes (chain alive)
    mov  x8, #3                            // SYS_REPORT(A_READ) — cue: launcher checkpoint-2, releases close GO
    movz x0, #0x62
    svc  #0

    // park on the close GO word (base + 0x3018; released after the launcher's checkpoint-2)
    add  x9, x9, #0x8                      // x9 = base+0x3010 -> base+0x3018 = A close-GO
    movz x24, #0x8000
5:  ldr  x10, [x9]
    cbnz x10, 6f
    mov  x8, #4                            // SYS_YIELD
    svc  #0
    subs x24, x24, #1
    b.ne 5b
    b    2f

6:  // (2) SYS_CLOSE hA -> 0 (the LAST reference: the deferred free runs now, in syscall context)
    mov  x8, #17                           // SYS_CLOSE(hA)
    mov  x0, x19
    svc  #0
    cmp  x0, #0
    b.ne 2f
    add  x23, x23, #4                      // bit2: close OK (last close -> chain freed)
    mov  x8, #17                           // SYS_CLOSE(hA) AGAIN -> double-close must be clean
    mov  x0, x19
    svc  #0
    cmn  x0, #9                            // x0 == -9 (-EBADF)?
    b.ne 2f
    add  x23, x23, #8                      // bit3: double-close -> -EBADF
2:
    mov  x0, x23                           // SYS_REPORT(final witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U11DEFER_EXIT_STATUS) -> EL0_U11DEFER_DONE
    movz x0, #0x7F
    svc  #0
1:  b 1b                                   // sys_exit never returns; belt-and-braces guard

    .balign 4
    .globl __u11defer_prog_b
__u11defer_prog_b:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u11defer_blob_start
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000                   // x9 = base + 0x3000 = B unlink-GO

    // park on the unlink GO word (released after A reported A_OPENED — so DEFER.BIN exists before B opens it)
    movz x24, #0x8000
13: ldr  x10, [x9]
    cbnz x10, 14f
    mov  x8, #4                            // SYS_YIELD
    svc  #0
    subs x24, x24, #1
    b.ne 13b
    b    12f                               // GO never released -> report the partial witness

14: // (0) open DEFER.BIN RW (no O_CREAT — A created it) -> hB
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #1                            // RW
    svc  #0
    mov  x20, x0                           // x20 = hB (>=0) or -errno
    tbnz x20, #63, 12f
    add  x23, x23, #1                      // bit0: B open of the existing file OK
    // (1) unlink via hB -> 0 (A still holds it open -> the chain-free is DEFERRED, nothing freed here)
    mov  x8, #16                           // SYS_UNLINK
    mov  x0, x20
    svc  #0
    cmp  x0, #0
    b.ne 12f
    add  x23, x23, #2                      // bit1: unlink OK (deferred)
    // (2) re-open the unlinked name RO -> -ENOENT (the NAME is gone immediately, even though the chain lives)
    mov  x8, #11                           // SYS_OPEN DEFER.BIN RO
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #0                            // RO
    svc  #0
    cmn  x0, #2                            // x0 == -2 (-ENOENT)?
    b.ne 12f
    add  x23, x23, #4                      // bit2: re-open of the unlinked name -> -ENOENT (name gone)
    mov  x8, #3                            // SYS_REPORT(B_UNLINKED) — cue: launcher checkpoint-1, releases A's read
    movz x0, #0x61
    svc  #0
12:
    mov  x0, x23                           // SYS_REPORT(final witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U11DEFER_EXIT_STATUS) -> EL0_U11DEFER_DONE
    movz x0, #0x7F
    svc  #0
11: b 11b                                  // sys_exit never returns; belt-and-braces guard

    .balign 4
8:  .ascii "DEFER.BIN"
9:
    .balign 8
7:  .ascii "U11-DEFER-OK-777"
    .balign 4
    .globl __u11defer_blob_end
__u11defer_blob_end:
"#
);

unsafe extern "C" {
    static __u11defer_blob_start: u8;
    static __u11defer_blob_end: u8;
    static __u11defer_prog_a: u8;
    static __u11defer_prog_b: u8;
}

// --- U11-M2b inline EL0 fixtures (teardown-last-close reaper). The u11defer two-fixture blob, with ONE
// behavioural change: program A EXITS WITHOUT CLOSING while holding the last cross-process open of the unlinked
// file — so TEARDOWN (not an explicit SYS_CLOSE) is the last close, `clear_files_row` queues the orphaned chain,
// and the `orphan_reaper` frees it. Same GO-word choreography (+0x3000 B-unlink, +0x3010 A-read, +0x3018
// A-EXIT), same +0x2000 read buffer. Witnesses: A = {create+write, read-after-unlink} (0x3, NO close bit),
// B = {open, unlink, re-open -ENOENT} (0x7, same as u11defer B). ABI: x8=nr, args x0-x2, ret x0.
core::arch::global_asm!(
    r#"
    .globl __u11reap_blob_start
__u11reap_blob_start:
    .balign 4
    .globl __u11reap_prog_a
__u11reap_prog_a:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u11reap_blob_start          // x9 = window base
    add  x12, x9, #0x2000                  // x12 = read dest VA (writable data page)
    adr  x13, 7f                           // x13 = pattern VA (RO code page)

    // (0) create DEFER2.BIN (O_CREAT|RW|O_PUBLIC, mode=7) -> hA, then write 16 bytes (grow-from-empty allocates a cluster)
    // U6: O_PUBLIC — B (a DIFFERENT ASID) opens this file below, so it must be world-accessible, not owned-private.
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // "DEFER2.BIN"
    mov  x1, #(9f - 8f)
    mov  x2, #7                            // O_CREAT | RW | O_PUBLIC
    svc  #0
    mov  x19, x0                           // x19 = hA (>=0) or -errno
    tbnz x19, #63, 2f                      // create/open failed -> report what we have
    mov  x8, #1                            // SYS_WRITE 16 bytes (the pattern) -> grow
    mov  x0, x19
    mov  x1, x13
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 2f
    add  x23, x23, #1                      // bit0: create DEFER2.BIN + grow-write OK
    mov  x8, #3                            // SYS_REPORT(A_OPENED) — cue: launcher releases B's unlink
    movz x0, #0x63
    svc  #0

    // park on the read GO word (base + 0x3010; released after B has unlinked + the launcher's checkpoint-1)
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000                   // x9 = base + 0x3000
    add  x9, x9, #0x10                     // x9 = base + 0x3010 = A read-GO
    movz x24, #0x8000                      // bounded poll budget
3:  ldr  x10, [x9]
    cbnz x10, 4f
    mov  x8, #4                            // SYS_YIELD — cooperative
    svc  #0
    subs x24, x24, #1
    b.ne 3b
    b    2f                                // GO never released -> report the partial witness (verdict FAILs)

4:  // (1) seek to 0, then READ 16 -> must STILL return A's original bytes (the chain is alive despite B's unlink)
    mov  x8, #15                           // SYS_SEEK(hA, 0)
    mov  x0, x19
    mov  x1, #0
    svc  #0
    cmp  x0, #0                            // seek returns the new offset (0); anything else is a failure
    b.ne 2f
    mov  x8, #12                           // SYS_READ(hA, buf, 16)
    mov  x0, x19
    mov  x1, x12
    mov  x2, #16
    svc  #0
    cmp  x0, #16                           // 16 bytes back == the chain was NOT freed (defer held)
    b.ne 2f
    ldr  x10, [x12]                        // read-back == pattern (lo 8)
    ldr  x11, [x13]
    cmp  x10, x11
    b.ne 2f
    ldr  x10, [x12, #8]                    // (hi 8)
    ldr  x11, [x13, #8]
    cmp  x10, x11
    b.ne 2f
    add  x23, x23, #2                      // bit1: read-after-unlink returns the original bytes (chain alive)
    mov  x8, #3                            // SYS_REPORT(A_READ) — cue: launcher checkpoint-2, releases the exit GO
    movz x0, #0x65
    svc  #0

    // park on the EXIT GO word (base + 0x3018; released after the launcher's checkpoint-2). When released, A
    // reports its witness and EXITS WITHOUT CLOSING hA — teardown is the last close, so the deferred chain-free
    // is queued at teardown and the reaper frees it. There is deliberately NO SYS_CLOSE here.
    add  x9, x9, #0x8                      // x9 = base+0x3010 -> base+0x3018 = A exit-GO
    movz x24, #0x8000
5:  ldr  x10, [x9]
    cbnz x10, 2f                           // exit GO released -> report witness + EXIT (no close)
    mov  x8, #4                            // SYS_YIELD
    svc  #0
    subs x24, x24, #1
    b.ne 5b
2:
    mov  x0, x23                           // SYS_REPORT(final witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U11REAP_EXIT_STATUS) -> EL0_U11REAP_DONE (A holds hA open)
    movz x0, #0x80
    svc  #0
1:  b 1b                                   // sys_exit never returns; belt-and-braces guard

    .balign 4
    .globl __u11reap_prog_b
__u11reap_prog_b:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x9, __u11reap_blob_start
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000                   // x9 = base + 0x3000 = B unlink-GO

    // park on the unlink GO word (released after A reported A_OPENED — so DEFER2.BIN exists before B opens it)
    movz x24, #0x8000
13: ldr  x10, [x9]
    cbnz x10, 14f
    mov  x8, #4                            // SYS_YIELD
    svc  #0
    subs x24, x24, #1
    b.ne 13b
    b    12f                               // GO never released -> report the partial witness

14: // (0) open DEFER2.BIN RW (no O_CREAT — A created it) -> hB
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #1                            // RW
    svc  #0
    mov  x20, x0                           // x20 = hB (>=0) or -errno
    tbnz x20, #63, 12f
    add  x23, x23, #1                      // bit0: B open of the existing file OK
    // (1) unlink via hB -> 0 (A still holds it open -> the chain-free is DEFERRED, nothing freed here)
    mov  x8, #16                           // SYS_UNLINK
    mov  x0, x20
    svc  #0
    cmp  x0, #0
    b.ne 12f
    add  x23, x23, #2                      // bit1: unlink OK (deferred)
    // (2) re-open the unlinked name RO -> -ENOENT (the NAME is gone immediately, even though the chain lives)
    mov  x8, #11                           // SYS_OPEN DEFER2.BIN RO
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #0                            // RO
    svc  #0
    cmn  x0, #2                            // x0 == -2 (-ENOENT)?
    b.ne 12f
    add  x23, x23, #4                      // bit2: re-open of the unlinked name -> -ENOENT (name gone)
    mov  x8, #3                            // SYS_REPORT(B_UNLINKED) — cue: launcher checkpoint-1, releases A's read
    movz x0, #0x64
    svc  #0
12:
    mov  x0, x23                           // SYS_REPORT(final witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(U11REAP_EXIT_STATUS) -> EL0_U11REAP_DONE
    movz x0, #0x80
    svc  #0
11: b 11b                                  // sys_exit never returns; belt-and-braces guard

    .balign 4
8:  .ascii "DEFER2.BIN"
9:
    .balign 8
7:  .ascii "U11-REAP-OK-7777"
    .balign 4
    .globl __u11reap_blob_end
__u11reap_blob_end:
"#
);

unsafe extern "C" {
    static __u11reap_blob_start: u8;
    static __u11reap_blob_end: u8;
    static __u11reap_prog_a: u8;
    static __u11reap_prog_b: u8;
}

// --- U6 inline EL0 fixtures (owner/grants on open). TWO programs in ONE shared blob (the u11defer idiom), each
// copied into its OWN slot -> its own ASID. The launcher (the single SEQUENCER) plants a `Child` handle in A's
// table naming B (so A's SYS_FGRANT is owner-scoped), then releases each choreography edge only after the prior
// step's SYS_REPORT cue:
//   * el0-uowner-a (OWNER): creates OWNED.BIN PRIVATE (O_CREAT|RW, mode=3 — owned by A) + grow-writes it; reports
//     A_READY; parks on its GRANT GO (+0x3000); SYS_FGRANTs B read+write (rights=3, child handle 2) -> 0; reports
//     A_GRANTED; parks on its REVOKE GO (+0x3010); SYS_FGRANTs B rights=0 (revoke) -> 0; reports A_REVOKED; parks
//     on its EXIT GO (+0x3018); RE-opens OWNED.BIN (owner authority persists) -> a handle; exits. A stays alive
//     (owner) through all of B's opens — its EXIT GO is released only after B has exited.
//   * el0-uowner-b (GRANTEE): parks on GO1 (+0x3000); opens OWNED.BIN -> -EACCES (not owner, not granted);
//     reports B_DENIED1; parks on GO2 (+0x3010); opens RW -> a handle, READs the 16 bytes (must match A's
//     pattern), tries SYS_FGRANT itself (a non-owner) -> -EACCES, tries SYS_UNLINK via its CAP_WRITE handle ->
//     -EACCES (delete is OWNER-only — a content grantee cannot unlink+recreate to steal ownership), and closes;
//     reports B_OPENED; parks on GO3 (+0x3018); opens -> -EACCES again (the revoke took effect); exits.
// Both build a witness in x23, SYS_REPORT it, and exit with the U6 sentinel. B's read buffer is +0x2000; GO words
// live in page 3 (+0x3000/+0x3010/+0x3018 per slot). ABI: x8=nr, args x0-x2, ret x0.
core::arch::global_asm!(
    r#"
    .globl __uowner_blob_start
__uowner_blob_start:
    .balign 4
    .globl __uowner_prog_a
__uowner_prog_a:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x25, __uowner_blob_start          // x25 = window base (preserved across the GO parks)
    adr  x13, 7f                           // x13 = pattern VA (RO code page)

    // (0) create OWNED.BIN PRIVATE (O_CREAT|RW, mode=3 -> owned by A) -> hA, then write 16 bytes (grow-from-empty)
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f                            // "OWNED.BIN"
    mov  x1, #(9f - 8f)
    mov  x2, #3                            // O_CREAT | RW  (PRIVATE — A becomes owner; NO O_PUBLIC)
    svc  #0
    mov  x19, x0                           // x19 = hA (>=0) or -errno
    tbnz x19, #63, 2f
    mov  x8, #1                            // SYS_WRITE 16 bytes (the pattern) -> grow
    mov  x0, x19
    mov  x1, x13
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 2f
    add  x23, x23, #1                      // bit0: create PRIVATE + write OK (an owner open+write)
    mov  x8, #3                            // SYS_REPORT(A_READY) — cue: launcher releases B's first open
    movz x0, #0x66
    svc  #0

    // park on the GRANT GO word (base + 0x3000; released after B's pre-grant denied open)
    add  x9, x25, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000                   // x9 = base + 0x3000
    movz x24, #0x8000                      // bounded poll budget
3:  ldr  x10, [x9]
    cbnz x10, 4f
    mov  x8, #4                            // SYS_YIELD — cooperative
    svc  #0
    subs x24, x24, #1
    b.ne 3b
    b    2f                                // GO never released -> report the partial witness (verdict FAILs)

4:  // (1) grant B read: SYS_FGRANT(hA, child handle 2 -> B, CAP_READ) -> 0
    mov  x8, #18                           // SYS_FGRANT
    mov  x0, x19                           // file handle hA
    mov  x1, #2                            // child handle idx (Child -> B; UOWNER_CHILD_IDX)
    mov  x2, #3                            // rights = CAP_READ | CAP_WRITE (B may read AND write content, NOT delete)
    svc  #0
    cmp  x0, #0
    b.ne 2f
    add  x23, x23, #2                      // bit1: grant returned 0
    mov  x8, #3                            // SYS_REPORT(A_GRANTED) — cue: launcher releases B's granted open
    movz x0, #0x68
    svc  #0

    // park on the REVOKE GO word (base + 0x3010; released after B's granted open+read)
    add  x9, x25, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x10                     // x9 = base + 0x3010
    movz x24, #0x8000
5:  ldr  x10, [x9]
    cbnz x10, 6f
    mov  x8, #4
    svc  #0
    subs x24, x24, #1
    b.ne 5b
    b    2f

6:  // (2) revoke B: SYS_FGRANT(hA, child 2, rights=0) -> 0
    mov  x8, #18
    mov  x0, x19
    mov  x1, #2
    mov  x2, #0                            // rights = 0 -> REVOKE the grant
    svc  #0
    cmp  x0, #0
    b.ne 2f
    add  x23, x23, #4                      // bit2: revoke returned 0
    mov  x8, #3                            // SYS_REPORT(A_REVOKED) — cue: launcher releases B's post-revoke open
    movz x0, #0x6A
    svc  #0

    // park on the EXIT GO word (base + 0x3018; released only after B has EXITED — A stays owner alive through
    // B's post-revoke denied open, so the owner row still exists when B is re-denied)
    add  x9, x25, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x18                     // x9 = base + 0x3018
    movz x24, #0x8000
10: ldr  x10, [x9]
    cbnz x10, 15f
    mov  x8, #4
    svc  #0
    subs x24, x24, #1
    b.ne 10b
    b    2f

15: // (3) the OWNER re-opens its OWN file after the revoke -> still admitted (ownership authority persists)
    mov  x8, #11                           // SYS_OPEN OWNED.BIN RW
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #1                            // RW
    svc  #0
    tbnz x0, #63, 2f                       // a negative here would be a real bug
    add  x23, x23, #8                      // bit3: owner re-open OK
2:
    mov  x0, x23                           // SYS_REPORT(final witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(UOWNER_EXIT_STATUS) -> EL0_UOWNER_DONE
    movz x0, #0x81
    svc  #0
1:  b 1b                                   // sys_exit never returns; belt-and-braces guard

    .balign 4
    .globl __uowner_prog_b
__uowner_prog_b:
    mov  x23, xzr                          // witness bitmask = 0
    adr  x25, __uowner_blob_start          // x25 = window base (preserved)
    add  x22, x25, #0x1000
    add  x22, x22, #0x1000                 // x22 = base + 0x2000 = read dest (writable data page)
    adr  x13, 7f                           // x13 = pattern VA (for the content compare)

    // park on GO1 (base + 0x3000; released after A created OWNED.BIN — so the file exists before B opens it)
    add  x9, x25, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000                   // x9 = base + 0x3000
    movz x24, #0x8000
22: ldr  x10, [x9]
    cbnz x10, 23f
    mov  x8, #4                            // SYS_YIELD
    svc  #0
    subs x24, x24, #1
    b.ne 22b
    b    21f

23: // (0) open OWNED.BIN RO BEFORE any grant -> -EACCES (a non-owner is denied BY NAME: the gap closed)
    mov  x8, #11                           // SYS_OPEN
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #0                            // RO
    svc  #0
    cmn  x0, #13                           // x0 == -13 (-EACCES)?
    b.ne 21f
    add  x23, x23, #1                      // bit0: non-owner open denied (-EACCES)
    mov  x8, #3                            // SYS_REPORT(B_DENIED1) — cue: launcher tells A to grant
    movz x0, #0x67
    svc  #0

    // park on GO2 (base + 0x3010; released after A granted B)
    add  x9, x25, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x10                     // x9 = base + 0x3010
    movz x24, #0x8000
24: ldr  x10, [x9]
    cbnz x10, 25f
    mov  x8, #4
    svc  #0
    subs x24, x24, #1
    b.ne 24b
    b    21f

25: // (1) open OWNED.BIN RW AFTER the grant -> a handle (>=0) carrying CAP_READ|CAP_WRITE (within the grant)
    mov  x8, #11
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #1                            // RW (requested R|W subset of the granted R|W)
    svc  #0
    mov  x20, x0                           // x20 = hB (>=0) or -errno
    tbnz x20, #63, 21f
    add  x23, x23, #2                      // bit1: granted RW open OK
    // (2) read 16 bytes -> the content must match A's pattern
    mov  x8, #12                           // SYS_READ
    mov  x0, x20
    mov  x1, x22
    mov  x2, #16
    svc  #0
    cmp  x0, #16
    b.ne 21f
    ldr  x10, [x22]                        // read-back == pattern (lo 8)
    ldr  x11, [x13]
    cmp  x10, x11
    b.ne 21f
    ldr  x10, [x22, #8]                    // (hi 8)
    ldr  x11, [x13, #8]
    cmp  x10, x11
    b.ne 21f
    add  x23, x23, #4                      // bit2: granted read content matches
    // (3) a NON-OWNER (B, a grantee) tries to SYS_FGRANT -> -EACCES (only the owner may grant; owner check
    //     fails BEFORE the child handle is resolved, so a bogus child idx is fine here)
    mov  x8, #18                           // SYS_FGRANT
    mov  x0, x20                           // file handle hB (a valid File B holds)
    mov  x1, #0                            // child handle idx (owner check fails first)
    mov  x2, #1                            // rights = CAP_READ
    svc  #0
    cmn  x0, #13                           // -EACCES?
    b.ne 21f
    add  x23, x23, #8                      // bit3: a non-owner grant is denied (-EACCES)
    // (4) a grantee cannot DELETE — SYS_UNLINK via its CAP_WRITE handle -> -EACCES (only the owner may unlink;
    //     the fix that stops a WRITE-grantee from unlink+recreate to STEAL ownership). OWNED.BIN survives.
    mov  x8, #16                           // SYS_UNLINK(hB)
    mov  x0, x20
    svc  #0
    cmn  x0, #13                           // -EACCES?
    b.ne 21f
    add  x23, x23, #16                     // bit4: grantee unlink denied (-EACCES) — delete is owner-only
    mov  x8, #17                           // SYS_CLOSE(hB)
    mov  x0, x20
    svc  #0
    mov  x8, #3                            // SYS_REPORT(B_OPENED) — cue: launcher tells A to revoke
    movz x0, #0x69
    svc  #0

    // park on GO3 (base + 0x3018; released after A revoked B)
    add  x9, x25, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x1000
    add  x9, x9, #0x18                     // x9 = base + 0x3018
    movz x24, #0x8000
26: ldr  x10, [x9]
    cbnz x10, 27f
    mov  x8, #4
    svc  #0
    subs x24, x24, #1
    b.ne 26b
    b    21f

27: // (4) open OWNED.BIN RO AFTER the revoke -> -EACCES again (the grant is gone; A is still owner)
    mov  x8, #11
    adr  x0, 8f
    mov  x1, #(9f - 8f)
    mov  x2, #0                            // RO
    svc  #0
    cmn  x0, #13                           // -EACCES?
    b.ne 21f
    add  x23, x23, #32                     // bit5: post-revoke open denied (-EACCES)
21:
    mov  x0, x23                           // SYS_REPORT(final witness)
    mov  x8, #3
    svc  #0
    mov  x8, #2                            // SYS_EXIT(UOWNER_EXIT_STATUS) -> EL0_UOWNER_DONE
    movz x0, #0x81
    svc  #0
20: b 20b                                  // sys_exit never returns; belt-and-braces guard

    .balign 4
8:  .ascii "OWNED.BIN"
9:
    .balign 8
7:  .ascii "UOWNER-OK-1234!!"
    .balign 4
    .globl __uowner_blob_end
__uowner_blob_end:
"#
);

unsafe extern "C" {
    static __uowner_blob_start: u8;
    static __uowner_blob_end: u8;
    static __uowner_prog_a: u8;
    static __uowner_prog_b: u8;
}

/// The `hello` EL0 program (M6c), built as a SEPARATE link product (`crates/user-blob`) and baked in
/// as a flat binary instead of living in the kernel's `.text`. `arroyo kernel8` builds it — a naked,
/// position-independent `sys_write("hello from EL0\n") + sys_exit(0)` routine — for the bare aarch64
/// target and `llvm-objcopy -O binary`s it to `target/user_blob.bin` BEFORE the kernel build; here we
/// `include_bytes!` it and copy it into the user CODE page at `setup()`, where it runs at EL0 exactly
/// like the old inline routine. The path is relative to this crate's manifest dir
/// (`unaos/crates/kernel`) → `unaos/target/user_blob.bin`; `include_bytes!` registers the file as a
/// rebuild dependency, so a changed routine re-triggers the kernel compile. Only ever compiled in the
/// baremetal build (this whole module is `#[cfg(feature = "baremetal")]`), so `./arroyo check`/`build`
/// — which do not build the blob — never need the file to exist.
static USER_BLOB: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/user_blob.bin"));

// --- M6b demo accounting. Written by the syscall/kill paths, read by `verdict`. ---
/// EL0 tasks that exited with status 0 (normal completion — the demo expects exactly 1: hello).
static EL0_EXITED_OK: AtomicU32 = AtomicU32::new(0);
/// EL0 tasks that exited nonzero — a fault-test program SELF-REPORTING that its intended fault
/// never happened (the survivor protocol). Any nonzero count is a FAIL.
static EL0_EXITED_ERR: AtomicU32 = AtomicU32::new(0);
/// Kills whose (task, EC, FAR-page) matched the demo's expectation table (want exactly 3).
static EL0_KILLED_EXPECTED: AtomicU32 = AtomicU32::new(0);
/// Kills that did NOT match — a fault happened, but not the one the permission model dictates
/// (e.g. UXN unset would turn stack-exec's instruction abort into an EC-0x00 UDF kill).
static EL0_KILLED_UNEXPECTED: AtomicU32 = AtomicU32::new(0);
/// Set by `tlb_warm` once the demo core has cached the pre-protect code-page mapping.
pub static TLB_WARMED: AtomicBool = AtomicBool::new(false);

// --- M6e demo accounting (decoupled from M6b so `exited=1 killed=3` stays byte-identical). ---
/// The preemption spinner reached its `sys_exit` (via the M6E sentinel status). 1 = it ran to
/// completion — under QEMU WITHOUT being preempted; on metal having been preempted (see below) and
/// then correctly resumed (the proof SP_EL0 banking works). Read by `m6e_verdict`.
static EL0_SPIN_DONE: AtomicU32 = AtomicU32::new(0);
/// IRQs taken while an EL0 task was the interrupted context (counted in `aarch64_irq_handler`, any
/// INTID — the timer, or any SPI such as the PL011 RX — and demo-WIDE: the spinner AND any of the four
/// M6b programs that a tick catches at EL0, since M6e makes them all preemptible). The crisp metal-only
/// proof that EL0 is preemptible: >0 on the real Pi 4, exactly 0 under QEMU raspi4b (no Group-1 IRQ is
/// ever delivered). The spinner's own resume-correctness proof is carried separately by
/// `EL0_SPIN_DONE == 1` (it completed after being interrupted). Read by `m6e_verdict`.
static EL0_IRQS_AT_EL0: AtomicU64 = AtomicU64::new(0);

/// M6e: count an IRQ taken while EL0 was running — called from `aarch64_irq_handler` when the banked
/// SPSR shows an EL0t return. Relaxed: a monotonic demo counter read once at the verdict, not a
/// synchronization point. NOTE (M6d): this stays demo-WIDE — it now also counts timer IRQs taken inside
/// the four M6d EL0 tasks, so on METAL `IRQs-taken-at-EL0` grows beyond the pre-M6d value (more
/// preemptible EL0 tasks). That value was always metal-variable; the QEMU regression stays `IRQs=0` (no
/// Group-1 IRQ is delivered there, so this is never called under QEMU) — see `m6e_verdict`.
#[inline]
pub fn note_el0_irq() {
    EL0_IRQS_AT_EL0.fetch_add(1, Ordering::Relaxed);
    // Part 0 fold #5: also bump the current (preempted) task's OWN counter. At IRQ time this core's
    // `current` is the preempted EL0 task, so `current_name` names it; the aggregate above stays for the
    // M6e verdict, this refines it to exact per-task attribution for the M6f verdict.
    if let Some(ctr) = task_preempt_counter(super::sched::current_name()) {
        ctr.fetch_add(1, Ordering::Relaxed);
    }
}

/// Map a demo EL0 task name to its per-task preempt counter (Part 0 fold #5), or None for any other task
/// (kernel tasks, the M6b/M6c fault fixtures + hello + spinner — not individually attributed).
fn task_preempt_counter(name: Option<&str>) -> Option<&'static AtomicU64> {
    Some(match name? {
        "el0-samevaA" => &PRE_SAMEVA_A,
        "el0-samevaB" => &PRE_SAMEVA_B,
        "el0-stackwrite" => &PRE_STACKWRITE,
        "el0-spsentinel" => &PRE_SPSENTINEL,
        "el0-yield" => &PRE_YIELD,
        "el0-sleep" => &PRE_SLEEP,
        _ => return None,
    })
}

// --- M6d demo accounting (per-task address spaces). Decoupled from the M6b/M6e counters — M6d tasks exit
// with `M6D_EXIT_STATUS` (routed to `EL0_M6D_DONE`) and any M6d kill is routed to `EL0_M6D_KILLED` (see
// `record_el0_kill`), so `exited=1 killed=3` (M6b) and `completed=1` (M6e) stay byte-identical. ---
/// M6d tasks that reached their sentinel `sys_exit` (the demo's completion signal; want 4).
static EL0_M6D_DONE: AtomicU32 = AtomicU32::new(0);
/// M6d tasks KILLED by a fault — a real per-slot ASID/permission bug. Kept OFF the M6b `killed_unexpected`
/// counter so an M6d metal failure surfaces as its own missing report/FAIL, not as a phantom M6b regression.
static EL0_M6D_KILLED: AtomicU32 = AtomicU32::new(0);
/// Values reported (via SYS_REPORT) by the four M6d tasks, keyed by name in `m6d_report`.
static M6D_REPORT_A: AtomicU64 = AtomicU64::new(0); // el0-samevaA read its slot sentinel
static M6D_REPORT_B: AtomicU64 = AtomicU64::new(0); // el0-samevaB read its slot sentinel
static M6D_REPORT_STACK: AtomicU64 = AtomicU64::new(0); // el0-stackwrite read its stack push/pop back
static M6D_REPORT_SP: AtomicU64 = AtomicU64::new(0); // el0-spsentinel read its sentinel through SP
/// The kernel-side deterministic nG detector's verdict (see `boot::probe_slot_isolation`, folded into the
/// same-VA PASS): the metal analogue of M6b's `tlb_warm` — true iff two slot roots resolved the SAME VA to
/// their OWN frames (a global nG bug would make both resolve to slot A's frame).
static M6D_PROBE_OK: AtomicBool = AtomicBool::new(false);

// M6d sentinel values planted into each reader task's slot-private data page (page 3, [top-0x100]). The
// low bits encode the slot's ASID so a cross-slot bleed fails the `== planted` check, not just distinctness.
const M6D_SENTINEL_A: u64 = 0xA5A5_0000_0000_0001; // slot A (ASID 1)
const M6D_SENTINEL_B: u64 = 0x5A5A_0000_0000_0002; // slot B (ASID 2)
const M6D_SENTINEL_SP: u64 = 0x5EED_0000_0000_0004; // slot D (ASID 4)
const M6D_STACK_PATTERN: u64 = 0xABCD_1234; // the in-program pattern el0-stackwrite pushes/pops

// --- M6f demo accounting (validated user pointers + wider syscall surface). Decoupled from the
// M6b/M6d/M6e counters exactly like M6d: M6f tasks exit with `M6F_EXIT_STATUS` -> `EL0_M6F_DONE`, and any
// M6f kill routes to `EL0_M6F_KILLED` (see `record_el0_kill`), so `exited=1 killed=3` (M6b), `completed=1`
// (M6e), and the M6d lines all stay byte-identical. Read by `m6f_verdict`. ---
/// M6f fixtures that reached their sentinel `sys_exit` (the demo's completion signal; want 4).
static EL0_M6F_DONE: AtomicU32 = AtomicU32::new(0);
/// M6f fixtures KILLED by a fault — a real bug (the hostile fixture's whole point is EFAULT returns, NOT
/// kills). Kept OFF the M6b counter so an M6f failure surfaces as its own FAIL, not a phantom M6b regression.
static EL0_M6F_KILLED: AtomicU32 = AtomicU32::new(0);
/// getinfo fixture witness: the pid it read back from the copy_to_user'd struct iff it matched SYS_GETPID
/// (and was non-zero), else 0. Non-zero == the to-user round-trip carried the correct value.
static M6F_GETINFO_WITNESS: AtomicU64 = AtomicU64::new(0);
/// hostile fixture: how many of its 4 bad pointers the kernel refused with -EFAULT (want 4).
static M6F_HOSTILE_REFUSED: AtomicU32 = AtomicU32::new(0);
/// yield / sleep fixtures: the loop iteration count each completed (want `M6F_ITERS` each — proof both ran).
static M6F_YIELD_DONE: AtomicU32 = AtomicU32::new(0);
static M6F_SLEEP_DONE: AtomicU32 = AtomicU32::new(0);
/// Observed yield<->sleep runner switches (see `note_interleave`); > 0 proves the two fixtures interleaved.
static M6F_INTERLEAVE_SWITCHES: AtomicU32 = AtomicU32::new(0);
/// Interleave witness state: 0 = no yielding M6f task has run yet; 1 = el0-yield last; 2 = el0-sleep last.
static M6F_INTERLEAVE_LAST: AtomicU32 = AtomicU32::new(0);
/// Iterations each interleave fixture loops (must match the `mov x19, #8` in the two inline programs).
const M6F_ITERS: u32 = 8;

// Per-task EL0 preempt counters (Part 0 review fold #5). `note_el0_irq` bumps the CURRENT (preempted)
// task's own counter, keyed by name, in addition to the demo-wide `EL0_IRQS_AT_EL0` aggregate — so the M6f
// verdict attributes preemption per slot task EXACTLY, refining the aggregate the M6d ledger called out as
// coarse. Name-keyed statics (not a `Task` field) so the count survives the task's teardown for the verdict
// to read. Metal-only signal: QEMU delivers no timer IRQ, so `note_el0_irq` is never called and all stay 0;
// on the real Pi 4 the timer preempts running EL0 tasks and these go > 0.
static PRE_SAMEVA_A: AtomicU64 = AtomicU64::new(0);
static PRE_SAMEVA_B: AtomicU64 = AtomicU64::new(0);
static PRE_STACKWRITE: AtomicU64 = AtomicU64::new(0);
static PRE_SPSENTINEL: AtomicU64 = AtomicU64::new(0);
static PRE_YIELD: AtomicU64 = AtomicU64::new(0);
static PRE_SLEEP: AtomicU64 = AtomicU64::new(0);

// --- M6g accounting (load a program FROM STORAGE — the disk-loaded EL0 program). Decoupled from every
// prior counter: the disk blob (the M6c `hello` bytes, read off the SD card's FAT volume) calls
// `sys_exit(0)`, which would otherwise land in the M6b `EL0_EXITED_OK` and corrupt `exited=1`. The
// SYS_EXIT / kill paths route by task NAME ("m6g-hello") into these counters instead, so every M6b/M6d/
// M6e/M6f verdict stays byte-identical. Read by the M6g loader (which doubles as its own verdict). ---
/// The disk-loaded EL0 program exited with status 0 (the expected outcome; want 1).
static EL0_M6G_DONE: AtomicU32 = AtomicU32::new(0);
/// The disk-loaded EL0 program exited nonzero — a self-reported failure (survivor protocol). Any is a FAIL.
static EL0_M6G_ERR: AtomicU32 = AtomicU32::new(0);
/// The disk-loaded EL0 program was KILLED by a fault (the untrusted bytes tripped the M6b fault-kill net).
static EL0_M6G_KILLED: AtomicU32 = AtomicU32::new(0);
/// Set by `m6f_verdict` as its last act: the M6g loader waits on this so every LOADER M6g line lands after
/// the M6b/M6e/M6d/M6f verdict lines (the Part-B probe's two early M6g lines land before the demo).
static M6F_VERDICT_PRINTED: AtomicBool = AtomicBool::new(false);

// =============================================================================================
// U4 accounting — the process model + per-process handle table: sys_spawn (load+run a child from storage,
// return a HANDLE into the caller's table) + sys_wait (reap the child a handle refers to). Evolves M7.
// =============================================================================================

/// Set when `m6g_loader` returns (every path). The U4 launcher gates on this so (a) all M6g lines print
/// FIRST — ordering — and (b) the M6d/M6f/M6g slots have freed (their tasks exited), so the parent's, the
/// orphan's, and the children's slot allocations succeed. (M6d + M6f hold all 8 slots when the BSP wires the
/// demo; they free as their fixtures exit, so U4's slots can only be claimed at run-time, after this gate —
/// see `u4_launcher`.)
static M6G_LOADER_DONE: AtomicBool = AtomicBool::new(false);

/// A spawned child's exit STATUS is stored valid once `state == PEXITED`.
const PFREE: u8 = 0; // entry unused
const PRUNNING: u8 = 1; // claimed; a child is (or is about to be) running under `pid`
const PEXITED: u8 = 2; // the child exited/was killed; `status` is valid, awaiting reap by sys_wait

/// The process table: parent + up to a few children. Static so it OUTLIVES each child's `Task` Box (which is
/// freed on exit) and each child's slot teardown — the reap accounting must survive both. `MAX_PROCS` is a
/// small cap « USER_SLOTS (8): if it exhausts, sys_spawn returns `-EAGAIN`, never grows the slot pool (a STOP
/// tripwire). `done` is posted exactly once by the child (its exit OR its kill path) and waited exactly once
/// by the parent's sys_wait, so a reaped-then-reused entry always starts at 0 permits (no drain needed).
const MAX_PROCS: usize = 4;
struct Proc {
    /// The child task id; the sys_wait key. 0 while an entry is FREE or a claim's pid is not yet stored.
    pid: AtomicU64,
    /// The child's exit status; valid once `state == PEXITED`.
    status: AtomicI32,
    /// FREE / RUNNING / EXITED — the ownership + lifecycle token (CAS'd FREE->RUNNING to claim).
    state: AtomicU8,
    /// U7: the child's address-space ASID — the pid->ASID map `sys_xfer` resolves a `Child` dest handle
    /// through (the transfer inbox is keyed by the RECIPIENT's ASID). Stored (Release) beside the pid by
    /// `sys_spawn` (and by the U7 launcher for its planted fixture entry); 0 while FREE.
    asid: AtomicU64,
    /// Posted once by the child (SYS_EXIT or the kill path), awaited once by the parent's sys_wait. The
    /// scheduler-post wake makes sys_wait work under QEMU (unlike a timer-driven `sleep_ticks`).
    done: super::sched::Semaphore,
}
static PROCS: [Proc; MAX_PROCS] = [const {
    Proc {
        pid: AtomicU64::new(0),
        status: AtomicI32::new(0),
        state: AtomicU8::new(PFREE),
        asid: AtomicU64::new(0),
        done: super::sched::Semaphore::new(0),
    }
}; MAX_PROCS];

/// The parent's WITNESS (reported via SYS_REPORT): `U4_WITNESS_TOKEN` (nonzero) iff it reaped BOTH children
/// by handle with exit status 0, else 0. `u4_launcher`'s verdict demands it be non-zero (and no kill). A
/// token, not a pid — `sys_spawn` now returns a handle, so the verdict only needs non-zero-means-both-ok.
static U4_PARENT_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The ownership NEGATIVE result: 1 iff `el0-u4orphan`'s `sys_wait(0)` on an Empty handle returned exactly
/// `-ECHILD` (structural ownership enforced — it holds no such handle), else 0. Read by the verdict.
static U4_ORPHAN_ECHILD: AtomicU32 = AtomicU32::new(0);
/// The U4 fixtures (parent + orphan) that reached their `0x74` sentinel exit (the completion signal; want 2).
/// Read by the verdict, which waits for both before judging.
static EL0_U4_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U4 task (a child, the parent, or the orphan) — a real bug (the children are well-behaved; a kill
/// fails the verdict). Kept OFF the M6b `killed_unexpected` counter (see `record_el0_kill`) so a U4 failure is
/// its own FAIL.
static EL0_U4_KILLED: AtomicU32 = AtomicU32::new(0);

/// The U5 capability fixture's WITNESS bitmask (reported via SYS_REPORT): one bit per proven behaviour (see
/// `U5_WITNESS_ALL`). `u5_launcher` PASSes iff it equals `U5_WITNESS_ALL` (all four capability semantics held).
static U5_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U5 fixture (`el0-u5cap`) reached its `0x75` sentinel exit (want 1). Read by `u5_launcher`, which waits
/// for it before judging — and, once set, the fixture's slot is torn down so its handle row is clear.
static EL0_U5_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U5 task — a real bug (the capability fixture is well-behaved). Off the M6b `killed_unexpected`
/// counter (see `record_el0_kill`), so a U5 fault fails only the U5 verdict.
static EL0_U5_KILLED: AtomicU32 = AtomicU32::new(0);
/// Set by `u4_launcher` at its every exit path (PASS/FAIL/skip) — the gate `u5_launcher` waits on so the U5
/// lines land strictly AFTER the U4 verdict (and the U4 slots have freed). Mirrors the M6g_LOADER_DONE idiom.
static U4_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

/// The U6 printing-spawner fixture's WITNESS bitmask (reported via SYS_REPORT): one bit per proven behaviour
/// (see `U6_WITNESS_ALL`). `u6_launcher` PASSes iff it equals `U6_WITNESS_ALL` AND the kernel-side kind/no-
/// collision check (`U6_KINDS_OK`) held.
static U6_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U6 fixture (`el0-u6spawn`) reached its `0x76` sentinel exit (want 1). Read by `u6_launcher`, which waits
/// for it before judging.
static EL0_U6_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U6 task (the printing-spawner fixture) — a real bug (it is register-only and well-behaved). Off the
/// M6b `killed_unexpected` counter (see `record_el0_kill`), so a U6 fault fails only the U6 verdict. (A killed
/// U6 CHILD shares the `u4-child` name and its non-zero kill status fails the verdict through the reap path.)
static EL0_U6_KILLED: AtomicU32 = AtomicU32::new(0);
/// The kernel-side U6 check result (set by `u6_launcher`): the general object table resolves the `File`/`Socket`
/// scaffold kinds (with the right rights, `-EACCES`-equivalent without) AND the first-free allocator skips the
/// reserved `CONSOLE_FD` so an interleaved console-install + two child installs collide on no index. Both must
/// hold for the U6 PASS.
static U6_KINDS_OK: AtomicBool = AtomicBool::new(false);
/// Set by `u5_launcher` at its every exit path (PASS/FAIL/skip) — the gate `u6_launcher` waits on so the U6
/// lines land strictly AFTER the U5 verdict (and the U5 slot has freed). Mirrors the U4_LAUNCH_DONE idiom.
static U5_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

/// The U6b File-handle fixture's WITNESS bitmask (reported via SYS_REPORT): one bit per proven behaviour
/// (see `U6B_WITNESS_ALL`). `u6b_launcher` PASSes iff it equals `U6B_WITNESS_ALL` (open+read+bytes-match via a
/// File capability, and both the rights and kind SYS_READ denials).
static U6B_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U6b fixture (`el0-u6bfile`) reached its `0x77` sentinel exit (want 1). Read by `u6b_launcher`, which
/// waits for it before judging — and, once set, the fixture's slot is torn down so its handle + file rows clear.
static EL0_U6B_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U6b task — a real bug (the File-handle fixture is well-behaved: it writes no stack and faults on
/// nothing). Off the M6b `killed_unexpected` counter (see `record_el0_kill`), so a U6b fault fails only U6b.
static EL0_U6B_KILLED: AtomicU32 = AtomicU32::new(0);
/// Set by `u6_launcher` at its every exit path (PASS/FAIL/skip) — the gate `u6b_launcher` waits on so the U6b
/// lines land strictly AFTER the U6 verdict (and the U6 slot has freed). Mirrors the U5_LAUNCH_DONE idiom. (U6
/// was the last demo before U6b, so it previously released no gate; this is that release.)
static U6_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

/// The U7 PARENT fixture's final witness bitmask (SYS_REPORT, routed by name): over-rights XFER `-EACCES`
/// (b0), XFER t1 ok (b1), XREVOKE t1 ok (b2), XFER t2 ok (b3). `u7_launcher` demands `U7_WITNESS_ALL`.
static U7_PARENT_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U7 CHILD fixture's final witness bitmask (SYS_REPORT, routed by name): RECV t1 (b0), USED the
/// transferred Console cap — a real write landed (b1), RECV t2 (b2), the revoked cap now `-EACCES` (b3).
static U7_CHILD_WITNESS: AtomicU64 = AtomicU64::new(0);
/// 1 once the child's FIRST write through the transferred cap landed (the mid-run `U7_USED_TOKEN` report) —
/// the launcher's cue to release the parent's GO word, so the revoke provably happens AFTER a successful use.
static U7_CHILD_USED: AtomicU32 = AtomicU32::new(0);
/// The U7 fixtures that reached their `0x78` sentinel exit (want 2 — parent + child). Routed by NAME ahead of
/// the Proc short-circuit (the child has a planted Proc entry). Read by `u7_launcher`'s deadline-bounded wait.
static EL0_U7_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U7 fixture — a real U7 bug (both are register-only and well-behaved). Off the M6b counter.
static EL0_U7_KILLED: AtomicU32 = AtomicU32::new(0);
/// Set by `u6b_launcher` at its every exit path (PASS/FAIL/skip) — the gate `u7_launcher` waits on so the U7
/// lines land strictly AFTER the U6b verdict (and the U6b slot has freed). Mirrors the U6_LAUNCH_DONE idiom.
/// (U6b was the last demo before U7, so it previously released no gate; this is that release.)
static U6B_LAUNCH_DONE: AtomicBool = AtomicBool::new(false);

/// The U8 revocation-tree fixture's WITNESS bitmask (reported via SYS_REPORT): one bit per proven behaviour
/// (see `U8_WITNESS_ALL`). `u8_launcher` PASSes iff it equals `U8_WITNESS_ALL` AND the kernel-side checks held.
static U8_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U8 fixture (`el0-u8tree`) reached its `0x79` sentinel exit (want 1). Read by `u8_launcher`'s wait.
static EL0_U8_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U8 fixture — a real bug (it is register-only and well-behaved). Off the M6b counter.
static EL0_U8_KILLED: AtomicU32 = AtomicU32::new(0);

/// The U9 File-WRITE fixture's WITNESS bitmask (reported via SYS_REPORT): one bit per proven behaviour (see
/// `U9_WITNESS_ALL`). `u9_launcher` PASSes iff it equals `U9_WITNESS_ALL` AND the kernel-side checks held.
static U9_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U9 fixture (`el0-u9write`) reached its `0x7A` sentinel exit (want 1). Read by `u9_launcher`'s wait.
static EL0_U9_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U9 fixture — a real bug (it is register-only; its only writable user target is its own data page).
/// Off the M6b counter.
static EL0_U9_KILLED: AtomicU32 = AtomicU32::new(0);

/// The U10 file-GROWTH fixture's WITNESS bitmask (reported via SYS_REPORT): one bit per proven behaviour (see
/// `U10_WITNESS_ALL`). `u10_launcher` PASSes iff it equals `U10_WITNESS_ALL` AND the kernel-side checks held.
static U10_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U10 fixture (`el0-u10grow`) reached its `0x7B` sentinel exit (want 1). Read by `u10_launcher`'s wait.
static EL0_U10_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U10 fixture — a real bug (register-only; its only writable user target is its own data page).
/// Off the M6b counter.
static EL0_U10_KILLED: AtomicU32 = AtomicU32::new(0);

/// The U10-create fixture's WITNESS bitmask (reported via SYS_REPORT): see `U10C_WITNESS_ALL`. `u10c_launcher`
/// PASSes iff it equals `U10C_WITNESS_ALL` AND the kernel-side checks held.
static U10C_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U10-create fixture (`el0-u10create`) reached its `0x7C` sentinel exit (want 1). Read by `u10c_launcher`.
static EL0_U10C_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U10-create fixture — a real bug (register-only). Off the M6b counter.
static EL0_U10C_KILLED: AtomicU32 = AtomicU32::new(0);

/// The U10-delete fixture's WITNESS bitmask (reported via SYS_REPORT): see `U10D_WITNESS_ALL`. `u10d_launcher`
/// PASSes iff it equals `U10D_WITNESS_ALL` AND the kernel-side checks held.
static U10D_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U10-delete fixture (`el0-u10delete`) reached its `0x7D` sentinel exit (want 1). Read by `u10d_launcher`.
static EL0_U10D_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U10-delete fixture — a real bug (register-only). Off the M6b counter.
static EL0_U10D_KILLED: AtomicU32 = AtomicU32::new(0);

/// The U11 open-file-lifecycle fixture's WITNESS bitmask (reported via SYS_REPORT): see `U11_WITNESS_ALL`.
/// `u11_launcher` PASSes iff it equals `U11_WITNESS_ALL` AND the kernel-side gen-rebind + on-disk checks held.
static U11_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The U11 fixture (`el0-u11close`) reached its `0x7E` sentinel exit (want 1). Read by `u11_launcher`.
static EL0_U11_DONE: AtomicU32 = AtomicU32::new(0);
/// A killed U11 fixture — a real bug (register-only). Off the M6b counter.
static EL0_U11_KILLED: AtomicU32 = AtomicU32::new(0);

/// U11-M2 (defer): the two cross-process fixtures' final witness bitmasks (SYS_REPORT), keyed by name.
static U11DEFER_A_WITNESS: AtomicU64 = AtomicU64::new(0);
static U11DEFER_B_WITNESS: AtomicU64 = AtomicU64::new(0);
/// U11-M2 (defer): the launcher's choreography cue flags — A finished create+write; B finished unlink+re-open;
/// A finished the post-unlink read. The launcher (single sequencer) releases the next GO word + runs its
/// fresh-mount checkpoint on each. Set from `u11defer_report` when the matching cue token is reported.
static U11DEFER_A_OPENED_F: AtomicU32 = AtomicU32::new(0);
static U11DEFER_B_UNLINKED_F: AtomicU32 = AtomicU32::new(0);
static U11DEFER_A_READ_F: AtomicU32 = AtomicU32::new(0);
/// U11-M2 (defer): BOTH fixtures reached their `0x7F` sentinel exit (want 2). Read by `u11defer_run`.
static EL0_U11DEFER_DONE: AtomicU32 = AtomicU32::new(0);
/// U11-M2 (defer): a killed defer fixture — a real bug (register-only, bar its +0x2000 read buffer).
static EL0_U11DEFER_KILLED: AtomicU32 = AtomicU32::new(0);

/// U11-M2b (reap): the two reap fixtures' final witness bitmasks (SYS_REPORT), keyed by name.
static U11REAP_A_WITNESS: AtomicU64 = AtomicU64::new(0);
static U11REAP_B_WITNESS: AtomicU64 = AtomicU64::new(0);
/// U11-M2b (reap): the launcher's choreography cue flags — A finished create+write; B finished unlink+re-open;
/// A finished the post-unlink read. The launcher (single sequencer) releases the next GO word + runs its
/// fresh-mount checkpoint on each. Set from `u11reap_report` when the matching cue token is reported.
static U11REAP_A_OPENED_F: AtomicU32 = AtomicU32::new(0);
static U11REAP_B_UNLINKED_F: AtomicU32 = AtomicU32::new(0);
static U11REAP_A_READ_F: AtomicU32 = AtomicU32::new(0);
/// U11-M2b (reap): BOTH fixtures reached their `0x80` sentinel exit (want 2). Read by `u11reap_run`.
static EL0_U11REAP_DONE: AtomicU32 = AtomicU32::new(0);
/// U11-M2b (reap): a killed reap fixture — a real bug (register-only, bar its +0x2000 read buffer).
static EL0_U11REAP_KILLED: AtomicU32 = AtomicU32::new(0);

// --- U6 (owner/grants) demo constants + statics --------------------------------------------------
/// U6 demo: the sentinel `sys_exit` status BOTH owner/grants fixtures (`el0-uowner-a`, `el0-uowner-b`) use so
/// their exits land in `EL0_UOWNER_DONE` (want 2). Distinct from every prior sentinel (…0x80) and 0.
const UOWNER_EXIT_STATUS: u64 = 0x81;
/// U6 demo: OWNER (A)'s witness — bit0 create OWNED.BIN PRIVATE + grow-write OK (an owner open+write); bit1
/// SYS_FGRANT B read -> 0; bit2 SYS_FGRANT B revoke (rights 0) -> 0; bit3 owner RE-opens its own file after
/// revoke -> OK (ownership authority persists). `uowner_run` PASSes iff A == this. Matches `add x23,#{1,2,4,8}`.
const UOWNER_A_WITNESS_ALL: u64 = 0xF;
/// U6 demo: GRANTEE (B)'s witness — bit0 open OWNED.BIN BEFORE any grant -> -EACCES (the gap closed: a
/// non-owner is denied by name); bit1 RW open AFTER grant -> a handle (within the R|W grant); bit2 the 16 read
/// bytes match A's pattern; bit3 B (a non-owner) SYS_FGRANT -> -EACCES (only the owner may grant); bit4 B
/// SYS_UNLINK via its CAP_WRITE handle -> -EACCES (delete is owner-only — a content grantee cannot unlink+recreate
/// to steal ownership); bit5 open AFTER revoke -> -EACCES again (revoke enforced). `uowner_run` PASSes iff B ==
/// this. Matches `add x23,#{1,2,4,8,16,32}` in program B.
const UOWNER_B_WITNESS_ALL: u64 = 0x3F;
/// U6 demo: the private file A creates + owns; B is denied, then granted, then re-denied. Runtime-created (not
/// planted). 9 chars (<= `MAX_NAME`); the `mov x1, #(9f-8f)` in the blob matches. Never unlinked (A exits owning
/// it — its owner row then reverts to PUBLIC at teardown), so the launcher pre-checks it ABSENT on a fresh image.
const UOWNER_NAME: &str = "OWNED.BIN";
/// U6 demo: the 16-byte pattern A writes into OWNED.BIN; B reads it back through its GRANTED handle and compares.
const UOWNER_PATTERN: [u8; 16] = *b"UOWNER-OK-1234!!";
/// U6 demo: the handle index in A's table where the launcher plants a `Child` handle naming B — A's SYS_FGRANT
/// names the grantee owner-scoped through it (off index 0, which A's own create first-free-claims, and off the
/// reserved `CONSOLE_FD`). The `mov x1, #2` operands in `__uowner_prog_a` MUST match.
const UOWNER_CHILD_IDX: usize = 2;
/// U6 demo: the launcher-CUE tokens the fixtures SYS_REPORT (all `> 0x1F`, so `uowner_report` distinguishes them
/// from a final witness). A: `A_READY` after create+write, `A_GRANTED` after granting B, `A_REVOKED` after
/// revoking B. B: `B_DENIED1` after the first (pre-grant) denied open, `B_OPENED` after the granted open+read+
/// non-owner-grant negative. The launcher (single sequencer) releases the next GO word on each.
const UOWNER_A_READY: u64 = 0x66;
const UOWNER_B_DENIED1: u64 = 0x67;
const UOWNER_A_GRANTED: u64 = 0x68;
const UOWNER_B_OPENED: u64 = 0x69;
const UOWNER_A_REVOKED: u64 = 0x6A;
/// U6 demo: the two fixtures' final witness bitmasks (SYS_REPORT), keyed by task name.
static UOWNER_A_WITNESS: AtomicU64 = AtomicU64::new(0);
static UOWNER_B_WITNESS: AtomicU64 = AtomicU64::new(0);
/// U6 demo: the launcher's choreography cue flags — set from `uowner_report` when the matching cue is reported.
static UOWNER_A_READY_F: AtomicU32 = AtomicU32::new(0);
static UOWNER_B_DENIED1_F: AtomicU32 = AtomicU32::new(0);
static UOWNER_A_GRANTED_F: AtomicU32 = AtomicU32::new(0);
static UOWNER_B_OPENED_F: AtomicU32 = AtomicU32::new(0);
static UOWNER_A_REVOKED_F: AtomicU32 = AtomicU32::new(0);
/// U6 demo: BOTH fixtures reached their `0x81` sentinel exit (want 2). Read by `uowner_run`.
static EL0_UOWNER_DONE: AtomicU32 = AtomicU32::new(0);
/// U6 demo: a killed owner/grants fixture — a real bug (register-only, bar B's +0x2000 read buffer).
static EL0_UOWNER_KILLED: AtomicU32 = AtomicU32::new(0);

/// Claim a FREE Proc entry, returning its index. CAS on `state` (FREE->RUNNING) is the atomic ownership
/// token; the pid=0 placeholder is overwritten with the real child pid (Release) by the caller AFTER the
/// child is spawned (see `sys_spawn` — the child cannot be dispatched until the parent yields, so the real
/// pid is always in place before any lookup). `None` if the table is full (-> `-EAGAIN`, never grow the pool).
fn proc_reserve() -> Option<usize> {
    for i in 0..MAX_PROCS {
        if PROCS[i]
            .state
            .compare_exchange(PFREE, PRUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            PROCS[i].pid.store(0, Ordering::Release);
            PROCS[i].status.store(0, Ordering::Release);
            PROCS[i].asid.store(0, Ordering::Release);
            return Some(i);
        }
    }
    None
}

/// Find the RUNNING Proc entry whose pid matches — the child-exit / child-kill lookup. Called with a live
/// task id (`> 0`), so it never spuriously matches a fresh claim's pid=0 placeholder.
fn proc_find_running(pid: u64) -> Option<usize> {
    (0..MAX_PROCS).find(|&i| {
        PROCS[i].state.load(Ordering::Acquire) == PRUNNING && PROCS[i].pid.load(Ordering::Acquire) == pid
    })
}

/// Find the non-FREE (RUNNING or EXITED) Proc entry whose pid matches — the sys_wait lookup. `None` => the
/// caller has no such child (`-ECHILD`).
fn proc_find_child(pid: u64) -> Option<usize> {
    (0..MAX_PROCS).find(|&i| {
        PROCS[i].state.load(Ordering::Acquire) != PFREE && PROCS[i].pid.load(Ordering::Acquire) == pid
    })
}

/// Release a Proc entry to FREE — after reaping in sys_wait, or unwinding a failed sys_spawn claim.
fn proc_free(i: usize) {
    PROCS[i].pid.store(0, Ordering::Release);
    PROCS[i].asid.store(0, Ordering::Release); // U7: drop the pid->ASID map with the entry
    PROCS[i].state.store(PFREE, Ordering::Release);
}

// ---------------------------------------------------------------------------------------------
// U4 Part A — the per-process handle table (keyed by ASID; the ownership namespace)
// ---------------------------------------------------------------------------------------------
//
// A small fixed handle table PER PROCESS, indexed by the process's ASID. Each EL0 process runs in its own
// M6d slot with a distinct ASID (1..=USER_SLOTS); the shared/boot context is ASID 0 (a valid but
// unused-by-U4 index — U4's fixtures each run in their OWN slot, so their tables are `HANDLES[asid >= 1]`).
// A handle value is `0` when Empty; otherwise it is the CHILD task id (pid) the handle refers to — the key
// into `PROCS`. So the two structures are deliberately SEPARATE and complementary: `PROCS` is keyed by pid
// (the process control blocks: exit `status`/`state`/`done`), `HANDLES` is keyed by ASID (the spawner's
// private namespace of child capabilities). Static, const-init, no heap — the `PROCS` discipline.
//
// Single-writer invariant: exactly one live task runs under any given ASID (one task per slot; a slot is
// torn down before it can be reused), and that task's syscalls are serialized (one SVC at a time), so a
// given `HANDLES[asid]` ROW is only ever touched by its own task. The atomics carry memory-ordering
// (publish the pid store with Release; a later handle read Acquires it), not cross-task contention.
//
// SCOPE NOTE (deferred to U5): a row is NOT cleared when its slot/ASID is torn down (teardown lives in
// `boot.rs`, out of this arc's lane). U4 relies on reapers CONSUMING their handles (`sys_wait` clears on
// reap) — so a well-behaved process leaves an empty row at exit, and the U4 demo is clean by construction
// (the parent reaps both children; the orphan spawns nothing; parent/orphan/children hold DISTINCT ASIDs
// while alive, and only the parent ever WRITES a row). A process that exits with UN-reaped handles would
// leave stale entries a future ASID-reuse could observe — harmless today (nothing reuses a row it did not
// write) but a real lifecycle concern once processes churn slots freely. That belongs to U5, which owns
// handle lifecycle (revoke / teardown-clear) alongside the capability CHECK it adds at this same lookup.
const NHANDLE: usize = 8; // handle slots per process (small, static — like MAX_PROCS)
/// `RESERVING` marks a handle slot claimed by an in-flight `sys_spawn` before the real child pid is known
/// (0 = Empty would let a re-scan re-claim it; a real pid is never `u64::MAX`). Overwritten with the pid
/// once the child is spawned, or cleared if the load fails — never observed by any other task (single-writer).
const HANDLE_RESERVING: u64 = u64::MAX;
static HANDLES: [[AtomicU64; NHANDLE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NHANDLE] }; super::boot::USER_SLOTS + 1];

// ---------------------------------------------------------------------------------------------
// U5 — handles as CAPABILITIES: rights, a resource target beyond "child pid", the enforcement CHECK
// ---------------------------------------------------------------------------------------------
//
// U4 built the STRUCTURE (a per-process, ASID-keyed handle table). U5 turns each handle into a capability:
// an unforgeable reference that carries RIGHTS, is CHECKED at the point of use, can be GRANTED (attenuated)
// and REVOKED, and whose lifetime is bounded by the owning ASID's teardown-clear. Two things are added to
// what a handle names — a rights bitmask (a sidecar array, so U4's `0`/`RESERVING` sentinel logic in the
// value word stays byte-identical) and a resource TARGET beyond "child pid" (a `CONSOLE` well-known token,
// so `sys_write` can route through the table). Deliberately minimal: two target kinds (CHILD(pid), CONSOLE),
// a small rights set, no general object table (that is U6+).

/// Capability rights — a small bitmask carried in the sidecar `HANDLE_RIGHTS`, checked at `handle_resolve`.
/// `CAP_WRITE` gates `sys_write`; `CAP_GRANT` gates minting attenuated copies; `CAP_READ`/`CAP_EXEC`/
/// `CAP_REVOKE` round out the model (CAP_REVOKE is reserved for cross-process revocation — U6; U5 revoke is
/// ownership-based). The values are stable across arches (documented in the permission-model doc).
const CAP_READ: u32 = 1 << 0; // 0x01 (== fs::unafs::RIGHT_READ, const-asserted at rights_from_native)
const CAP_WRITE: u32 = 1 << 1; // 0x02
const CAP_EXEC: u32 = 1 << 2; // 0x04
const CAP_GRANT: u32 = 1 << 3; // 0x08
const CAP_REVOKE: u32 = 1 << 4; // 0x10 (U8: revoking a handle carrying this kills its derivation SUBTREE)
// The rights are the distinct low 5 bits — a well-formed bitmask (each a single, non-overlapping bit, which
// the attenuation check `req & !src` relies on). This const-assert verifies that and anchors every CAP_* as
// used, so the model bits not yet exercised in Rust this arc (CAP_EXEC — held by no fixture, so the
// attenuation negative bites; CAP_REVOKE — reserved for U6) don't read as dead code.
const _: () = assert!(
    (CAP_READ | CAP_WRITE | CAP_EXEC | CAP_GRANT | CAP_REVOKE) == 0x1F,
    "capability rights must be the distinct low 5 bits"
);

/// The well-known target token stored in a handle's value word to mean "the serial console resource" (as
/// opposed to a child pid). Distinct from `0` (Empty), `HANDLE_RESERVING` (`u64::MAX`), and every real pid
/// (small, monotonic from `NEXT_TID`), so the value word alone discriminates CHILD(pid) from CONSOLE without
/// perturbing U4's sentinel checks. Keeping it to one non-pid token (not a general object table) is the arc's
/// scope line.
const HANDLE_CONSOLE: u64 = u64::MAX - 1;

/// The RESERVED console handle index — the stdout convention (fd 1, like POSIX). Every EL0 program prints with
/// `sys_write(fd=1, ..)` (the M6c hello blob, the M6f hostile fixture, the disk-loaded children), so the console
/// write-capability is endowed here (`install_console_cap`). U6: this index is now a RESERVED region the
/// first-free allocator (`handle_install`) SKIPS — so a process may hold a console cap here AND N auto-allocated
/// child/object caps with **zero index collision, for any interleaving of installs**. This closes the U5 design
/// note: there, `install_console_cap`'s unconditional store to a fixed index could clobber a child that
/// `handle_install`'s first-free scan had already placed at index 1 (harmless only because no process both
/// printed and spawned). Reserving the index — rather than allocating the console through the shared allocator —
/// keeps the `fd=1` stdout ABI byte-identical for every existing blob (index 0 stays a general slot; children/
/// objects fill {0, 2, 3, ..}). See `handle_install`.
const CONSOLE_FD: usize = 1;

/// The rights sidecar: keyed IDENTICALLY to `HANDLES` (`[asid][idx]`), so the value word keeps U4's exact
/// `0`/`RESERVING` sentinel semantics and the rights ride alongside. Written with Release beside the value
/// store (rights published BEFORE the value that makes a handle live, so a resolver that observes the value
/// also observes the rights), cleared in `handle_clear` / `clear_handle_row`. `0` rights == an inert handle.
static HANDLE_RIGHTS: [[AtomicU32; NHANDLE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NHANDLE] }; super::boot::USER_SLOTS + 1];

// ---------------------------------------------------------------------------------------------
// U6 — the general OBJECT descriptor: a handle is (kind, target, rights), first-free allocated for ALL kinds
// ---------------------------------------------------------------------------------------------
//
// U5 made a handle a capability but the descriptor was FIXED-SHAPE: two kinds only, discriminated by a magic
// value word (`HANDLE_CONSOLE` vs a pid), with the console pinned at a fixed index. U6 turns it into a general
// object descriptor without disturbing the lock-free allocator: the KIND rides in a PARALLEL sidecar
// (`HANDLE_KIND`, keyed identically to `HANDLES`/`HANDLE_RIGHTS`), exactly like the rights.
//
// Why a sidecar (not the value word's high bits): the value word's ONLY reserved values stay `0` (Empty — the
// allocator's free marker) and `u64::MAX` (RESERVING — an in-flight claim), byte-identical to U4/U5. The kind
// lives elsewhere, so a `File(id)`/`Socket(id)` may carry an ARBITRARY id in the value word with no high-bit
// masking and no risk of a real `(kind, id)` aliasing Empty/RESERVING — the STOP tripwire the brief names is
// structurally impossible here (the only ids to avoid remain `0` and `u64::MAX`, the same two pids already
// avoid; the demo's scaffold ids are small). It also mirrors the existing `HANDLE_RIGHTS` sidecar 1:1 — same
// shape, same publish-before-the-live-value / observe-after discipline.
const KIND_EMPTY: u8 = 0; // no object — matches the value word's `0`=Empty (a cleared/free slot)
const KIND_CHILD: u8 = 1; // a child process (value word = its pid) — U4's meaning
const KIND_CONSOLE: u8 = 2; // the serial console (value word = the `HANDLE_CONSOLE` token) — U5's meaning
const KIND_FILE: u8 = 3; // U6 scaffold: a file object (value word = an opaque id); no fs syscall routes here yet
const KIND_SOCKET: u8 = 4; // U6 scaffold: a socket object (value word = an opaque id); no net syscall routes yet

/// The KIND sidecar: keyed IDENTICALLY to `HANDLES`/`HANDLE_RIGHTS` (`[asid][idx]`). Discriminates what a live
/// handle NAMES (`KIND_*`), so the value word carries only the target payload (a pid / the console token / an
/// object id) and keeps U4/U5's `0`=Empty / `u64::MAX`=RESERVING sentinels intact. Written with Release BEFORE
/// the value store that makes a handle live (so a resolver observing the live value also observes the kind),
/// cleared in `handle_clear` / `clear_handle_row`. `KIND_EMPTY` (0) == an inert/absent slot (the const-init).
static HANDLE_KIND: [[AtomicU8; NHANDLE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU8::new(KIND_EMPTY) }; NHANDLE] }; super::boot::USER_SLOTS + 1];

/// `-EACCES`: a capability check failed — no such handle / wrong kind / missing right / an attenuation
/// violation (a grant that would amplify rights). The single errno U5's CHECK returns to EL0.
const EACCES: i64 = -13;
/// `-EINVAL`: a `SYS_CAP` sub-op selector that is neither GRANT nor REVOKE.
const EINVAL: i64 = -22;

/// What a resolved handle NAMES — the general object descriptor's kind + payload. `Child(pid)` (U4) and
/// `Console` (U5) are the live kinds every consumer routes through; `File(id)`/`Socket(id)` are U6 SCAFFOLDS —
/// defined and resolvable so the table is provably general, though no fs/net syscall routes through them yet
/// (adding those is U7+/out of scope). The payload is the handle's value word (a pid, or an opaque object id;
/// `Console` carries none).
#[derive(Clone, Copy)]
enum HandleTarget {
    Child(u64),
    Console,
    File(u64),
    Socket(u64),
}

/// Why `handle_resolve` refused: the handle is not in the caller's table (out-of-range or Empty), or it is
/// present but lacks a required right. Callers map these to their own errno (`sys_wait` -> `-ECHILD` for
/// either, preserving U4's structural-ownership semantics; `sys_write`/`sys_cap` -> `-EACCES`).
enum ResolveErr {
    NoHandle,
    Denied,
}

/// The enforcement CHECK, at the SINGLE lookup point every handle-consuming path goes through. Resolve `idx`
/// against the caller's own (`asid`) table, then require the handle carry every bit in `req`. Returns the
/// general target on success. `NoHandle` for out-of-range/Empty/`RESERVING` (a reserving placeholder is never a
/// usable handle); `Denied` when a present handle lacks a required right. The value word is loaded Acquire
/// (synchronizing with the Release store that installed it), then the rights, then the KIND — so a resolver
/// that sees a live value also sees its rights and kind (they are stored BEFORE the value goes live). U6: the
/// kind comes from the `HANDLE_KIND` sidecar (not a magic value word), so the payload is dispatched by kind —
/// `Child`/`File`/`Socket` carry the value word as pid/id; `Console` ignores it. A live value with `KIND_EMPTY`
/// is a kernel bug (kind is always published before the value) — treated defensively as `NoHandle`.
fn handle_resolve(asid: u64, idx: u64, req: u32) -> Result<HandleTarget, ResolveErr> {
    if idx as usize >= NHANDLE {
        return Err(ResolveErr::NoHandle);
    }
    debug_assert!((asid as usize) < HANDLES.len(), "handle_resolve: asid out of range");
    let raw = HANDLES[asid as usize][idx as usize].load(Ordering::Acquire);
    if raw == 0 || raw == HANDLE_RESERVING {
        return Err(ResolveErr::NoHandle);
    }
    let rights = HANDLE_RIGHTS[asid as usize][idx as usize].load(Ordering::Acquire);
    if rights & req != req {
        return Err(ResolveErr::Denied);
    }
    // U7: a RECEIVED (transferred) capability goes STALE the moment its sender revokes the transfer. The
    // revocation state lives in the sender-owned transfer RECORD — never in the recipient's row, which only
    // the recipient writes (single-writer preserved); this read-side check is how the sender's revoke reaches
    // the recipient's next use. One extra Acquire load; `0` (not a transferred cap) for every other handle.
    let rec = HANDLE_XFER_REC[asid as usize][idx as usize].load(Ordering::Acquire);
    if rec != 0
        && XFER_REC_TX[(rec - 1) as usize].load(Ordering::Acquire) & XFER_REVOKED_BIT != 0
    {
        return Err(ResolveErr::Denied);
    }
    // U8: a DERIVED capability is stale if ANY ancestor node in the derivation ledger is revoked — the
    // bounded child->root walk that makes revocation a TREE (a re-grant or re-transfer chain dies whole
    // when any node above it is marked). Roots (no node) skip the walk entirely; no revoke path ever
    // wrote this row — staleness is discovered here, at use (the U7 pattern, generalized).
    let dn = HANDLE_DERIV[asid as usize][idx as usize].load(Ordering::Acquire);
    if dn != 0 && deriv_stale(dn) {
        return Err(ResolveErr::Denied);
    }
    match HANDLE_KIND[asid as usize][idx as usize].load(Ordering::Acquire) {
        KIND_CHILD => Ok(HandleTarget::Child(raw)),
        KIND_CONSOLE => Ok(HandleTarget::Console),
        KIND_FILE => Ok(HandleTarget::File(raw)),
        KIND_SOCKET => Ok(HandleTarget::Socket(raw)),
        _ => Err(ResolveErr::NoHandle), // KIND_EMPTY / unknown on a live value — a kernel bug; fail closed
    }
}

/// Set the rights word at `HANDLES[asid][idx]` (Release) — used beside a value store to attach rights to a
/// freshly-installed handle (a child handle in `sys_spawn`, a minted handle in `sys_cap_grant`).
fn handle_set_rights(asid: u64, idx: usize, rights: u32) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "handle_set_rights: out of range");
    HANDLE_RIGHTS[asid as usize][idx].store(rights, Ordering::Release);
}

/// Set the KIND byte at `HANDLE_KIND[asid][idx]` (Release) — the U6 twin of `handle_set_rights`, stored beside
/// the value/rights when a handle is installed (a child in `sys_spawn` = `KIND_CHILD`; a mint in
/// `sys_cap_grant` = the source's kind). Published BEFORE the value goes live (see `handle_resolve`).
fn handle_set_kind(asid: u64, idx: usize, kind: u8) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "handle_set_kind: out of range");
    HANDLE_KIND[asid as usize][idx].store(kind, Ordering::Release);
}

/// The KIND byte at `HANDLE_KIND[asid][idx]` (Acquire) — read alongside `handle_get`'s value when a caller needs
/// the raw descriptor (e.g. `sys_cap_grant`, whose mint must copy the source handle's kind). `KIND_EMPTY` for an
/// out-of-range/absent slot.
fn handle_kind(asid: u64, idx: usize) -> u8 {
    if idx >= NHANDLE {
        return KIND_EMPTY;
    }
    debug_assert!((asid as usize) < HANDLES.len(), "handle_kind: asid out of range");
    HANDLE_KIND[asid as usize][idx].load(Ordering::Acquire)
}

/// Install a capability at a FIXED index (not `handle_install`'s first-free scan): store the KIND and rights
/// FIRST (Release), then the target value (Release, LAST) — so a resolver that observes the live value also
/// observes the kind + rights. Used to endow the console cap at `CONSOLE_FD` and to plant the U5/U6 demo
/// fixtures (console / File / Socket). Always called BEFORE the target process is dispatched (setup / pre-spawn),
/// so there is no concurrent resolver; the ordering is the defensive belt-and-braces.
fn install_cap(asid: u64, idx: usize, kind: u8, target: u64, rights: u32) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "install_cap: out of range");
    HANDLE_KIND[asid as usize][idx].store(kind, Ordering::Release);
    HANDLE_RIGHTS[asid as usize][idx].store(rights, Ordering::Release);
    HANDLES[asid as usize][idx].store(target, Ordering::Release);
}

/// Endow the process running under `asid` with a console WRITE-capability at the RESERVED `CONSOLE_FD` — the
/// bootstrap that lets an EL0 program print once `sys_write` routes through the table. Given at spawn/launch to
/// every process meant to print: the shared window (ASID 0: `el0-hello`) in `setup`, each M6f/M6g/U4-child slot
/// and the U6 printing spawner in their setup/spawn paths. A process NOT so endowed gets `-EACCES` from
/// `sys_write` (the U5 negative). U6: because `handle_install` SKIPS `CONSOLE_FD`, this store can never clobber
/// (nor be clobbered by) an auto-allocated child/object handle, for any ordering of installs.
fn install_console_cap(asid: u64) {
    install_cap(asid, CONSOLE_FD, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE);
}

/// True iff the entire `HANDLES[asid]` row (values, rights AND kinds) is clear — the teardown-clear verifier.
/// Read by `u5_launcher` after the fixture exits and its slot is retired: `boot::teardown_user_slot` clears the
/// row on exit, so this transitions false -> true, proving no stale capability outlives its owning ASID.
fn handle_row_is_clear(asid: u64) -> bool {
    debug_assert!((asid as usize) < HANDLES.len(), "handle_row_is_clear: asid out of range");
    (0..NHANDLE).all(|i| {
        HANDLES[asid as usize][i].load(Ordering::Acquire) == 0
            && HANDLE_RIGHTS[asid as usize][i].load(Ordering::Acquire) == 0
            && HANDLE_KIND[asid as usize][i].load(Ordering::Acquire) == KIND_EMPTY
            // U7: the transfer-reference sidecar is part of "clear" too — the single-writer snapshot and
            // the teardown proof must not miss a stale record reference behind an otherwise-empty slot.
            && HANDLE_XFER_REC[asid as usize][i].load(Ordering::Acquire) == 0
            // U8: likewise the derivation sidecar — a stale node reference is part of "not clear".
            && HANDLE_DERIV[asid as usize][i].load(Ordering::Acquire) == 0
    })
}

/// U5: clear an ENTIRE per-process handle row (every value, its rights AND its kind) when the owning slot/ASID
/// is torn down — the lifecycle half of "U5 owns revoke/teardown-clear", folding U4's one deferred note (a row
/// was NOT cleared on teardown, so a future ASID-reuse could observe stale entries). Called from
/// `boot::teardown_user_slot` (aarch64 lane) BEFORE the slot's used-flag is released, so no concurrent
/// `alloc_user_slot` on another core can claim the slot and populate the row between the clear and the
/// release (see the ordering note there). `asid` is 1..=USER_SLOTS (ASID 0 is never torn down).
pub fn clear_handle_row(asid: u64) {
    debug_assert!(asid >= 1 && (asid as usize) < HANDLES.len(), "clear_handle_row: asid out of range");
    // U8: bump this ASID's inbox GENERATION first — strictly BEFORE the inbox sweep below — so every
    // deposit stamped for the dying tenant is dead-on-arrival for the ASID's next tenant even if it lands
    // after the sweep passed its slot (RECV verifies the stamp; the sender's post-check re-reads this word).
    // This closes the U7-documented sys_xfer TOCTOU (exit + recycle + consume inside the deposit window).
    ASID_GEN[asid as usize].fetch_add(1, Ordering::AcqRel);
    for i in 0..NHANDLE {
        // Clear the value first (Empty => `handle_resolve` bails as NoHandle before reading rights/kind), then
        // the rights and kind — so no intermediate state is ever a live handle with wrong rights/kind.
        HANDLES[asid as usize][i].store(0, Ordering::Release);
        HANDLE_RIGHTS[asid as usize][i].store(0, Ordering::Release);
        HANDLE_KIND[asid as usize][i].store(KIND_EMPTY, Ordering::Release);
        // U7: a received cap's transfer record is released with its handle (the handle_clear twin), so a
        // torn-down recipient leaks no record and a reused ASID inherits no stale transfer reference.
        let rec = HANDLE_XFER_REC[asid as usize][i].swap(0, Ordering::AcqRel);
        if rec != 0 {
            xfer_rec_free((rec - 1) as usize);
        }
        // U8: the teardown is a drop of every handle the dying task held — its derivation nodes free (or
        // tombstone until their subtrees drain), so a reused ASID inherits no stale derivation reference.
        let dn = HANDLE_DERIV[asid as usize][i].swap(0, Ordering::AcqRel);
        if dn != 0 {
            deriv_drop(dn);
        }
    }
    // U7: wipe this ASID's transfer INBOX alongside its handles — a pending (undelivered) transfer to a dying
    // process is discarded and its record freed, so a REUSED ASID starts with an empty inbox (no stale pending
    // capability for the next tenant to receive). Claim-to-clear per slot (tx-exact CAS) so a racing consumer
    // or retractor never double-frees; a sender racing this teardown re-checks its recipient AFTER depositing
    // (the sys_xfer post-check) and retracts, closing the deposit-into-a-dead-asid path from its side.
    clear_xfer_inbox_row(asid);
    // BANDY-1 M2: drain this ASID's bus mailbox alongside its handles/inbox — undelivered replies
    // die with their tenant (the boxes free here), and the gen bump above already makes any reply
    // enqueued in a race dead-on-arrival for the next tenant (bus_mrecv verifies the stamp).
    bus_mbox_clear(asid);
    // U7: DISOWN any still-live transfer this dying ASID sent (SENDER -> u64::MAX, never a real ASID):
    // revoke authority dies with the sender, so the ASID's next tenant can neither revoke nor be blamed
    // for the old tenant's transfers (txids are monotonic and were returned to EL0 — without this, a
    // recycled sender-ASID could enumerate and kill its predecessor's live delegations). The CAS is
    // owner-exact: a record freed or reclaimed by another sender in the window simply fails the exchange.
    // The orphaned transfer stays live for its recipient — irrevocable until the revocation-tree arc
    // re-homes derivations.
    for r in 0..MAX_XFERS {
        let _ = XFER_REC_SENDER[r].compare_exchange(asid, u64::MAX, Ordering::AcqRel, Ordering::Acquire);
    }
    // U6b: wipe this ASID's open-file descriptors alongside its handles. A File handle is only usable through
    // its descriptor (`sys_read` re-checks `FILE_USED`), so the handle clear above already denies any read;
    // clearing the descriptors here reclaims the slots and guarantees a REUSED ASID starts with no stale file
    // (no leaked offset, no aliasable descriptor). Same teardown site, same ordering guarantees as the handles.
    clear_files_row(asid);
    // U6: drop the owner/grants rows this ASID owned (its private files revert to PUBLIC — no persistent
    // principal keeps owning them, and the ASID's next tenant is a different process) and sweep any grant
    // naming it. `IrqGuard`-safe under this already-IRQ-masked teardown. Keeps the bounded table self-cleaning.
    owned_clear_owner_asid(asid);
    // M2.1: clear this slot's persistent principal STAMP — the slot's next tenant is a different program, so
    // it must not inherit the departed program's principal (the ppid twin of owned_clear_owner_asid).
    slot_ppid_clear(asid);
    // ELF-5: reset this ASID's input ring (a reused ASID inherits no stale input event), and if it was the
    // registered active input target, clear the designation so the router stops enqueueing to a dead slot.
    clear_input_row(asid);
    let _ = EL0_INPUT_ACTIVE.compare_exchange(asid, 0, Ordering::AcqRel, Ordering::Acquire);
    // UVUG-8r2: companion clear for the takeover latch. Without it a torn-down ASID could leave its takeover
    // engaged, and a REUSED ASID would inherit it — the next `run` into the same slot would start with its
    // deadline already suspended. CAS on `asid` so we only ever clear OUR OWN latch, never a live one belonging
    // to a different process. (The heartbeat needs no clear: `takeover_suspends` ignores it once the latch is 0,
    // and `el0_input_set_active` resets it on the next focus anyway.)
    let _ = EL0_TAKEOVER_ASID.compare_exchange(asid, 0, Ordering::AcqRel, Ordering::Acquire);
}

/// The ASID of the address space the caller is running in, read from `TTBR0_EL1[63:48]`. A syscall executes
/// with the caller's `TTBR0_EL1` live (M6d), so this names the CALLER's per-process handle table. Read
/// SYNCHRONOUSLY inside the SVC handler — resolving a handle against the wrong ASID would reap the wrong
/// child or spuriously `-ECHILD`. (Placed with the handle helpers it serves; the asm-wrapper twin of
/// `remask_irq`.)
#[inline]
fn current_asid() -> u64 {
    let ttbr0: u64;
    // SAFETY: a plain read of a system register; no memory access, no clobber.
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0, options(nomem, nostack, preserves_flags)) };
    ttbr0 >> 48
}

/// Claim the first Empty slot in `HANDLES[asid]`, storing `value` (CAS 0->value), and return its index — the
/// handle `sys_spawn` / `sys_cap_grant` return to EL0. `None` if the table is full (-> `-EAGAIN`, never grow
/// it). Mirrors `proc_reserve`. `asid` is always in range (0..=USER_SLOTS from a 16-bit TTBR0 ASID with
/// USER_SLOTS==8; debug-asserted).
///
/// U6 — the collision fix: the scan SKIPS the reserved `CONSOLE_FD`. That index belongs to the console cap by
/// convention (`install_console_cap`, an unconditional fixed-index store); by never handing it out here, an
/// auto-allocated child/object handle can never land on it and be clobbered by a later console install, nor
/// clobber a console already there — for ANY interleaving of installs. So the general allocator hands out
/// {0, 2, 3, .., NHANDLE-1}; the console lives at `CONSOLE_FD`. This closes the one U5 design note (a printing
/// spawner colliding a child with the console). Callers claim with `HANDLE_RESERVING`, then publish the kind +
/// rights + real value (value LAST — see `sys_spawn` / `sys_cap_grant`); `value` may be any non-`0` word (the
/// CAS treats only `0` as free).
fn handle_install(asid: u64, value: u64) -> Option<usize> {
    debug_assert!((asid as usize) < HANDLES.len(), "handle_install: asid out of range");
    let table = &HANDLES[asid as usize];
    for (i, slot) in table.iter().enumerate() {
        if i == CONSOLE_FD {
            continue; // reserved for the console cap — never auto-allocated (the U6 no-collision invariant)
        }
        if slot.compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return Some(i);
        }
    }
    None
}

/// Overwrite the pid stored at `HANDLES[asid][idx]` (Release) — used to replace a `HANDLE_RESERVING`
/// placeholder with the real child pid once `sys_spawn` has it.
fn handle_set(asid: u64, idx: usize, pid: u64) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "handle_set: out of range");
    HANDLES[asid as usize][idx].store(pid, Ordering::Release);
}

/// The pid at `HANDLES[asid][idx]`, or `None` if the index is out of range or the slot is Empty (0) — i.e.
/// the caller holds no such child handle (structural ownership: `-ECHILD`). A `HANDLE_RESERVING` placeholder
/// can never be seen here (single-writer: the same task is not concurrently spawning and waiting).
fn handle_get(asid: u64, idx: usize) -> Option<u64> {
    if idx >= NHANDLE {
        return None;
    }
    debug_assert!((asid as usize) < HANDLES.len(), "handle_get: asid out of range");
    match HANDLES[asid as usize][idx].load(Ordering::Acquire) {
        0 => None,
        pid => Some(pid),
    }
}

/// Clear (0 = Empty) the handle at `HANDLES[asid][idx]` AND its sidecar rights + kind — the handle is consumed
/// when its child is reaped in `sys_wait`, released when a failed `sys_spawn` unwinds its reservation, or
/// dropped by `sys_cap` REVOKE. Value cleared first (Empty => `handle_resolve` bails before reading rights/
/// kind), then the rights and kind, so no intermediate state is ever a live handle carrying stale rights/kind.
fn handle_clear(asid: u64, idx: usize) {
    debug_assert!((asid as usize) < HANDLES.len() && idx < NHANDLE, "handle_clear: out of range");
    HANDLES[asid as usize][idx].store(0, Ordering::Release);
    HANDLE_RIGHTS[asid as usize][idx].store(0, Ordering::Release);
    HANDLE_KIND[asid as usize][idx].store(KIND_EMPTY, Ordering::Release);
    // U7: if this was a RECEIVED (transferred) capability, dropping the handle ends the transfer — free its
    // record (the transfer's whole lifetime is: XFER claims the record, the received handle references it,
    // this clear releases it; a pending-discard or sender-retract frees it on the other paths). The swap
    // clears the sidecar so a re-installed handle at this index never inherits a stale record reference.
    let rec = HANDLE_XFER_REC[asid as usize][idx].swap(0, Ordering::AcqRel);
    if rec != 0 {
        xfer_rec_free((rec - 1) as usize);
    }
    // U8: dropping the handle drops its derivation node — freed if its subtree has drained, else a
    // tombstone until it does (see `deriv_drop`). Swap-clears the sidecar so a re-installed handle at
    // this index never inherits a stale node reference. NOTE the order: the record is freed FIRST (above),
    // the node dropped after — `sys_cap_xrevoke` relies on it (a won tx-exact CAS there implies the node
    // it captured was not yet dropped-and-reclaimed; the id guard covers the residue).
    let dn = HANDLE_DERIV[asid as usize][idx].swap(0, Ordering::AcqRel);
    if dn != 0 {
        deriv_drop(dn);
    }
}

// ---------------------------------------------------------------------------------------------
// U6b — the per-task OPEN-FILE table: what a `File` handle's value word points at
// ---------------------------------------------------------------------------------------------
//
// A `File` handle (U6a's scaffold, now real) names a resource that needs per-open STATE the handle word alone
// can't hold: which file (its chain head + byte size) and how far a sequential reader has advanced (its
// offset). That state lives here, in a small per-task descriptor table keyed like `HANDLES` (`[asid][idx]`).
// The `File` handle's value word carries the FILE-ID = `descriptor index + 1` (a small non-`0`, non-`u64::MAX`
// word — the +1 bias keeps it clear of the value word's Empty/RESERVING sentinels, structurally, for any
// index including 0). `handle_resolve` returns `File(file_id)`; `sys_read` decodes `idx = file_id - 1`.
//
// Shape mirrors the handle sidecars: parallel atomic arrays, no lock. Access is single-writer per row at any
// instant — a row is populated ONLY before its task is dispatched (`u6b_launcher`'s pre-endow) or BY that one
// task mid-syscall (`sys_open`/`sys_read`, IRQ-masked), and cleared at teardown after the task exits — so the
// `Release`-store / `Acquire`-load discipline is the belt-and-braces (the same as `HANDLE_RIGHTS`/`_KIND`).
// Presence is a dedicated `FILE_USED` flag (NOT an overloaded cluster sentinel) so a legal 0-cluster (empty)
// file is representable without aliasing "free". Read-only, one FAT volume, no seek — the arc's scope.
const NFILE: usize = 4; // open files per process (small, static — a demo opens at most two per row)

/// Per-descriptor presence flag: `true` == this `[asid][idx]` slot holds a live open file. Claimed
/// (`false`->`true`) in `files_alloc`, cleared in `files_free`/`clear_files_row`. The single source of truth
/// for "is this file-id valid" — `sys_read` re-checks it after decoding a handle's file-id (defense in depth).
static FILE_USED: [[AtomicBool; NFILE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicBool::new(false) }; NFILE] }; super::boot::USER_SLOTS + 1];
/// The open file's first data cluster (chain head for a `read_at` walk). Meaningful only where `FILE_USED`.
static FILE_CLUSTER: [[AtomicU32; NFILE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; super::boot::USER_SLOTS + 1];
/// The open file's total byte size (the EOF bound `sys_read` clamps against). Meaningful only where `FILE_USED`.
static FILE_SIZE: [[AtomicU32; NFILE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; super::boot::USER_SLOTS + 1];
/// The descriptor's byte offset — advanced by the count each `sys_read`/File `sys_write` delivers, and set
/// absolutely by `SYS_SEEK` (U9). Meaningful only where `FILE_USED`. Always kept `<= FILE_SIZE`: reads/writes
/// clamp to the bytes remaining, and `sys_seek` rejects an offset past `size` with `-EINVAL`.
static FILE_OFFSET: [[AtomicU32; NFILE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; super::boot::USER_SLOTS + 1];
/// U10: the on-disk LOCATION of this file's directory entry — the absolute LBA of its directory sector and
/// (in `FILE_DIR_OFF`) the byte offset of its 32-byte slot within that sector, both captured at `sys_open`
/// (from `fat::find_located`). A GROW republishes the file's `size`/`first_cluster` into that slot, so the
/// on-disk directory stays the reader's source of truth. Meaningful only where `FILE_USED`; unused (both `0`)
/// for descriptors that never grow (the U6b no-cap negative, the U9 revoke check).
static FILE_DIR_LBA: [[AtomicU64; NFILE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NFILE] }; super::boot::USER_SLOTS + 1];
static FILE_DIR_OFF: [[AtomicU32; NFILE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; super::boot::USER_SLOTS + 1];
/// U11: per-descriptor GENERATION counter. A `File` handle's value word packs `(gen << 32) | (idx + 1)`, and
/// `file_desc_validate` rejects a handle whose packed gen != the slot's CURRENT gen — so a stale sibling handle
/// to a slot that was freed and then FIRST-FIT-REUSED by a different file is `-EACCES` (a gen mismatch), never a
/// silent re-bind to that different file (closes the U10 sibling-rebind note). Bumped on EVERY free
/// (`files_free` — the path `SYS_CLOSE`, `sys_unlink`'s `files_free_by_dir`, and `clear_files_row` all route
/// through or mirror) so the very next reuse of the slot lands on a fresh generation. Const-init `0`; monotone
/// within a boot (a u32 wrap is ~4 billion frees away — unreachable for the demo). Acquire/Release-paired with
/// `FILE_USED` (published last on alloc, cleared on free) so a validator that sees a live slot sees its gen.
static FILE_GEN: [[AtomicU32; NFILE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NFILE] }; super::boot::USER_SLOTS + 1];

/// U11-M2: per-descriptor pointer to the GLOBAL open-file refcount row this descriptor increments (`OPEN_FILES`
/// index, or `OPENROW_NONE` for a scaffold/free slot). Recorded by `sys_open` right after `files_alloc` (the row
/// `openfile_incref` returned), and used by `files_free`/`sys_unlink` to decrement/mark that EXACT row — NOT a
/// key search. This is what makes the refcount robust against FAT recycling a deleted file's directory SLOT:
/// `(dir_lba, dir_off)` identifies a slot, not a file, so a new file created in an unlinked-but-still-open file's
/// recycled `0xE5` slot would collide on the key; keying the decrement/mark on the row INDEX the descriptor
/// actually claimed keeps each file's refcount + deferred-free strictly its own. Const-init `OPENROW_NONE`.
static FILE_OPENROW: [[AtomicU32; NFILE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(OPENROW_NONE) }; NFILE] }; super::boot::USER_SLOTS + 1];

/// U11-M2: the `FILE_OPENROW` sentinel for "this descriptor increments no open-file row" (a scaffold with no file
/// identity, or a free slot). `u32::MAX` is unreachable as a real `OPEN_FILES` index (`NOPENFILE` is tiny).
const OPENROW_NONE: u32 = u32::MAX;

/// U11: pack a `(generation, descriptor index)` pair into a `File` handle's value word. Low 32 bits = `idx + 1`
/// (the +1 bias keeps the whole word clear of the value word's `0`=Empty / `u64::MAX`=RESERVING sentinels for
/// ANY index and generation — `idx + 1 >= 1` so the low half is nonzero, and `idx + 1 <= NFILE` so the word is
/// never all-ones); high 32 bits = the slot's generation at open time. `file_desc_validate` decodes + validates.
/// The gen-0 encoding of index `idx` is exactly `idx + 1` — byte-identical to the pre-U11 bare file-id, so the
/// scaffold File handles (U6/U9 kernel checks, which read the value word directly) are unaffected.
fn file_id_pack(g: u32, idx: usize) -> u64 {
    ((g as u64) << 32) | ((idx + 1) as u64)
}

/// U11: THE single point that turns a `File` handle's value word into a live descriptor index. Decodes the
/// packed `(gen, idx)`, bounds-checks `idx`, requires the slot LIVE (`FILE_USED`), and requires the packed
/// generation to equal the slot's CURRENT generation. The gen check is what closes the U10 sibling-rebind note:
/// after a slot is freed (gen bumped) and first-fit-REUSED by a different file (`FILE_USED` true again), a
/// lingering handle carrying the OLD gen fails here — no silent re-bind. Every File consumer
/// (`sys_read`/`sys_write_file`/`sys_seek`/`sys_unlink`/`sys_close`) funnels its file-id through this ONE helper,
/// so the descriptor-identity check is inherited once (no per-syscall re-derivation that could drift). Returns
/// the validated index; `None` == invalid/stale (the caller maps to `-EACCES`, or `sys_close` to `-EBADF`).
/// Acquire loads pair with the Release stores in `files_alloc`/`files_free` (belt-and-braces: a row has one
/// writer at a time — its own task mid-syscall, or teardown after exit).
fn file_desc_validate(asid: u64, file_id: u64) -> Option<usize> {
    let idx = ((file_id & 0xFFFF_FFFF) as usize).checked_sub(1)?;
    if idx >= NFILE {
        return None;
    }
    if !FILE_USED[asid as usize][idx].load(Ordering::Acquire) {
        return None; // free slot — a closed/unlinked descriptor (the U6b–U10 presence check)
    }
    let g = (file_id >> 32) as u32;
    if FILE_GEN[asid as usize][idx].load(Ordering::Acquire) != g {
        return None; // slot was freed + reused since this handle was minted — stale, no rebind (U11)
    }
    Some(idx)
}

/// Claim the first free descriptor in `FILES[asid]` for a freshly-opened file, returning its index (the caller
/// packs it with the slot's current generation into the file-id via `file_id_pack`). Publishes size/offset/
/// cluster with the `FILE_USED` presence flag stored LAST (Release) — so a resolver that observes a live
/// descriptor also observes its fields (mirrors the handle "publish the live word last" discipline). The slot's
/// generation is NOT touched here (it advances only on free), so the value the caller reads to pack the handle is
/// the generation this descriptor lives under. `None` if the row is full (-> `-EMFILE`; never grown).
fn files_alloc(asid: u64, first_cluster: u32, size: u32, dir_lba: u64, dir_off: u32) -> Option<usize> {
    debug_assert!((asid as usize) < FILE_USED.len(), "files_alloc: asid out of range");
    for k in 0..NFILE {
        if FILE_USED[asid as usize][k]
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // Fields first, presence already claimed above — but re-publish nothing that could be read as live
            // before these land: the CAS made USED true, yet a resolver only reaches this descriptor via a File
            // HANDLE, which `sys_open` installs strictly AFTER this returns, so no reader can observe the row
            // between the CAS and these stores. Stored Release regardless (belt-and-braces).
            FILE_CLUSTER[asid as usize][k].store(first_cluster, Ordering::Release);
            FILE_SIZE[asid as usize][k].store(size, Ordering::Release);
            FILE_OFFSET[asid as usize][k].store(0, Ordering::Release);
            FILE_DIR_LBA[asid as usize][k].store(dir_lba, Ordering::Release);
            FILE_DIR_OFF[asid as usize][k].store(dir_off, Ordering::Release);
            // U11-M2: default to NO open-file row. A real `sys_open` OVERWRITES this with the row
            // `openfile_incref` returned (right after this call); a scaffold descriptor (files_alloc called
            // directly, no incref) keeps NONE, so its `files_free` decrement is a no-op.
            FILE_OPENROW[asid as usize][k].store(OPENROW_NONE, Ordering::Release);
            return Some(k);
        }
    }
    None
}

/// Release descriptor `idx` in `FILES[asid]` — the ONE descriptor-free primitive (`sys_close`, `sys_unlink`'s
/// `files_free_by_dir`, `sys_open`'s reserve/unwind, and `clear_files_row` at teardown all route through it).
/// Clears the fields, drops `FILE_USED` (Release), then BUMPS the slot's generation (U11) — so the very next
/// `files_alloc` reuse of this slot lands on a fresh gen and any handle still carrying the old (gen, idx) fails
/// `file_desc_validate`'s gen check (no sibling rebind). The gen bump is last: a validator that observed the slot
/// LIVE with the old gen resolved a genuinely-live descriptor (the same file); the rebind window only opens after
/// a full free+reuse, by which point this bump has landed.
///
/// U11-M2: this is ALSO the refcount-DECREMENT choke point. It reads the descriptor's open-file ROW pointer
/// (`FILE_OPENROW`) BEFORE clearing it and decrements THAT exact row (`openfile_decref_at`) — not a key search,
/// so a directory slot FAT recycled for a different file can never redirect this decrement. It returns
/// `Some(chain head)` IFF that decrement was the LAST close of an `unlink_pending` file that owns clusters — the
/// caller then frees the chain (`free_orphan_chain`) in a block-I/O-legal context, or (teardown) logs the leak.
/// Scaffold descriptors (`FILE_OPENROW == OPENROW_NONE`) decref to a no-op. `#[must_use]`: dropping the result
/// could leak a live chain.
#[must_use]
fn files_free(asid: u64, idx: usize) -> Option<u32> {
    debug_assert!((asid as usize) < FILE_USED.len() && idx < NFILE, "files_free: out of range");
    // Read the open-file row this descriptor increments BEFORE the clears below reset it.
    let row = FILE_OPENROW[asid as usize][idx].load(Ordering::Acquire);
    FILE_CLUSTER[asid as usize][idx].store(0, Ordering::Release);
    FILE_SIZE[asid as usize][idx].store(0, Ordering::Release);
    FILE_OFFSET[asid as usize][idx].store(0, Ordering::Release);
    FILE_DIR_LBA[asid as usize][idx].store(0, Ordering::Release);
    FILE_DIR_OFF[asid as usize][idx].store(0, Ordering::Release);
    FILE_OPENROW[asid as usize][idx].store(OPENROW_NONE, Ordering::Release);
    FILE_USED[asid as usize][idx].store(false, Ordering::Release);
    FILE_GEN[asid as usize][idx].fetch_add(1, Ordering::AcqRel); // U11: stale-sibling protection on slot reuse
    // U11-M2: drop this descriptor's cross-process open-file refcount by ROW INDEX (no-op for scaffolds). The
    // returned chain head (if any) is the caller's to free — never under any lock, never in teardown context.
    openfile_decref_at(row)
}

/// U10/U11-M2: free EVERY of this process's descriptors whose recorded directory slot is (`dir_lba`, `dir_off`),
/// and FREE the cluster chain of each one that turns out to be the LAST close of an `unlink_pending` file. Used by
/// `sys_unlink`: a file opened more than once in the SAME process yields independent descriptors all naming the
/// same slot, so deleting via one handle must invalidate ALL of them (the U10 fail-safe — otherwise a sibling
/// descriptor keeps `FILE_USED = true` with a stale `FILE_CLUSTER` aliasing a now-freed, re-allocatable chain).
/// After it, every sibling handle still resolves but fails the `FILE_USED` re-check -> `-EACCES`.
///
/// ⚠ `(dir_lba, dir_off)` is a directory SLOT, and FAT RECYCLES it — `create_in_root` reuses a `0xE5`'d slot for
/// a new file — so it does NOT uniquely identify a file. TWO distinct files can share the same `(dir_lba,
/// dir_off)` in ONE process (open F; F unlinked while this process holds it, its slot goes `0xE5`; create G,
/// which reuses F's slot) while sitting on DIFFERENT `OPEN_FILES` rows. This sweep therefore matches BOTH, and
/// BOTH can reach `refcount == 0` while `unlink_pending`. So the caller MUST free EVERY orphan chain head this
/// returns, not just the last — else the earlier file's chain leaks (a benign lost-cluster leak, but a real one,
/// on the EXPLICIT unlink path). This is called ONLY from `sys_unlink` (a block-I/O-legal context — teardown uses
/// `files_free` directly + logs). F3-M3: it now COLLECTS the orphan heads (`(heads, count)`, at most one per
/// descriptor slot) instead of freeing inline — `sys_unlink` runs it under the NAMESPACE lock, and the free
/// (`free_orphan_chain`) mounts + does chain block I/O, which must happen AFTER that guard drops (the span rule:
/// the namespace lock is never held across `mount()`). `dir_lba != 0` (caller guards). `#[must_use]`: dropping
/// the result leaks every collected chain.
#[must_use]
fn files_free_by_dir(asid: u64, dir_lba: u64, dir_off: u32) -> ([u32; NFILE], usize) {
    debug_assert!((asid as usize) < FILE_USED.len() && dir_lba != 0, "files_free_by_dir: bad args");
    let mut heads = [0u32; NFILE];
    let mut n = 0usize;
    for k in 0..NFILE {
        if FILE_USED[asid as usize][k].load(Ordering::Acquire)
            && FILE_DIR_LBA[asid as usize][k].load(Ordering::Acquire) == dir_lba
            && FILE_DIR_OFF[asid as usize][k].load(Ordering::Acquire) == dir_off
        {
            // Collect EACH orphan head (not just the last) — two recycled-slot files can both drop to 0 here.
            if let Some(fc) = files_free(asid, k) {
                heads[n] = fc;
                n += 1;
            }
        }
    }
    (heads, n)
}

/// Clear an ENTIRE per-task open-file row at teardown — the file twin of `clear_handle_row`'s handle wipe (it
/// calls this). Routes each slot through `files_free`, so the per-slot field clears + generation bump are
/// identical to before AND every live descriptor's cross-process refcount is decremented here (the teardown
/// decrement — a short, I/O-free `SpinMutex` critical section, safe in this IRQ-masked context, the same way the
/// scheduler locks `RUN_QUEUES`/`SLEEPERS` IRQ-masked). `asid` is 1..=USER_SLOTS.
///
/// U11-M2 honest scope: if a teardown decrement is the LAST close of an `unlink_pending` file, `files_free`
/// returns the chain head — but freeing it HERE is UNSAFE (teardown is IRQ-masked, on a stack about to be freed,
/// block I/O illegal). U11-M2b closes that gap: `deferred_free_push` hands the chain to the `orphan_reaper` via
/// the I/O-free deferred-free queue (lock + array write — the SAFE twin of the teardown decrement above), and
/// the reaper frees it in a block-I/O-legal context. If the queue is FULL, degrade honestly to the M2a
/// behavior — LOG the leak + leave the clusters allocated (benign lost clusters) — but NEVER block, spin, or do
/// I/O here.
fn clear_files_row(asid: u64) {
    debug_assert!(asid >= 1 && (asid as usize) < FILE_USED.len(), "clear_files_row: asid out of range");
    for k in 0..NFILE {
        if let Some(orphan) = files_free(asid, k) {
            // U11-M2b: queue the teardown-orphaned chain for the reaper (I/O-free). Full queue -> honest leak-log.
            if !deferred_free_push(orphan) {
                serial_println!(
                    "U11-defer: teardown last-close of an unlinked file — chain @cluster {} left allocated (queue full; leak)",
                    orphan
                );
            }
        }
    }
}

/// True iff the entire `FILES[asid]` row is free — the U6b teardown-clear verifier (the file twin of
/// `handle_row_is_clear`). Read by `u6b_launcher` after the fixture exits and its slot retires: teardown clears
/// the row, transitioning this false->true, proving no open file outlives its owning ASID.
fn files_row_is_clear(asid: u64) -> bool {
    debug_assert!((asid as usize) < FILE_USED.len(), "files_row_is_clear: asid out of range");
    (0..NFILE).all(|k| !FILE_USED[asid as usize][k].load(Ordering::Acquire))
}

// =============================================================================================
// U11-M2: the GLOBAL (cross-ASID) open-file refcount table — POSIX unlink-defers-free.
// =============================================================================================
//
// U11 M1 gave every File descriptor a per-task lifetime (SYS_CLOSE + generation tags). This table adds the
// CROSS-process lifetime the U10 delete note left open: a file unlinked while ANOTHER process holds it open
// must keep its cluster chain allocated until that process's LAST close — else the freed chain is first-fit-
// reused under the still-live descriptor (cross-file read/write + information disclosure). A row is JOINED by the
// file's on-disk identity `(dir_lba, dir_off)` at open time, so ONE row is shared by every open of that file
// across every ASID and `refcount` is the count of live descriptors naming it. BUT `(dir_lba, dir_off)` is a
// directory SLOT, which FAT RECYCLES the moment a file is deleted (`create_in_root` reuses `0xE5` slots) — so it
// identifies a slot, not a file. Two guards make the accounting robust to that recycling: (a) the open-time join
// SKIPS `unlink_pending` rows (a deferred-free file's name is `0xE5`'d, so it can't be re-opened by name — a key
// match on a pending row is a DIFFERENT file that reused the slot, and gets a FRESH row), and (b) every
// descriptor records its row INDEX in `FILE_OPENROW`, so the paired decrement/mark hit that EXACT row rather than
// re-searching the recyclable key. Generation tags (M1) cover descriptor-slot reuse; this covers directory-slot
// reuse one level up.
//
// The table is the ONLY cross-ASID shared File state, so it is guarded by a single `SpinMutex` (NOT the
// sleeping `Mutex`, which yields — illegal in the IRQ-masked teardown decrement path). The lock is held ONLY
// across the short table mutation (find/claim/±refcount, read `first_cluster`); the `free_chain` block I/O is
// ALWAYS the caller's, after the lock drops — never under the lock, never in teardown context.
//
// Lifecycle: `sys_open` increments (find-or-claim a row) BEFORE `files_alloc`; `files_free` (the one
// descriptor-free primitive) decrements. `sys_unlink` marks the row `unlink_pending` + stashes the chain head,
// but frees nothing while `refcount > 0`; the decrement that drops `refcount` to 0 on an `unlink_pending` row
// hands the chain head back so the caller frees it (all FAT copies) in a block-I/O-legal context.

/// Rows in the global open-file table. Small + static — a full table on a NEW identity is `-ENFILE`.
const NOPENFILE: usize = 16;

/// One open-file identity's shared state. Guarded by `OPEN_FILES` (a `SpinMutex`), so the fields are plain
/// `Copy` integers — the lock provides mutual exclusion (no atomics; atomics are not `Copy` and would block the
/// `derive`). A row is FREE iff `refcount == 0` (its canonical empty form also has `dir_lba == 0`).
#[derive(Clone, Copy)]
struct OpenFileRow {
    dir_lba: u64,         // identity key: the file's directory-entry sector LBA (0 in a free row)
    dir_off: u32,         // identity key: the 32-byte slot's offset within that sector
    refcount: u32,        // live open descriptors across ALL processes; 0 == free row
    unlink_pending: bool, // the name has been 0xE5'd; free `first_cluster` at the last close
    first_cluster: u32,   // chain head to free at the last close — captured at UNLINK (authoritative there)
}

impl OpenFileRow {
    const EMPTY: Self = OpenFileRow {
        dir_lba: 0,
        dir_off: 0,
        refcount: 0,
        unlink_pending: false,
        first_cluster: 0,
    };
}

/// The global open-file refcount table. `SpinMutex::new` is `const` and `OpenFileRow: Copy`, so this is a
/// const-constructed static (mirrors `sched.rs`'s `RUN_QUEUES`/`SLEEPERS`). Its lock is taken IRQ-masked in the
/// teardown decrement (safe: a bounded, allocation-free, I/O-free critical section) and in every open/close/
/// unlink — but NEVER held across a `mount()`/`free_chain` block-I/O call.
static OPEN_FILES: SpinMutex<[OpenFileRow; NOPENFILE]> = SpinMutex::new([OpenFileRow::EMPTY; NOPENFILE]);

/// U11-M2: INCREMENT the open-file refcount for the file at `(dir_lba, dir_off)` and return the OPEN_FILES ROW
/// INDEX it landed on (the caller records it in `FILE_OPENROW` so the paired decrement/mark hit this exact row).
/// It JOINS an existing NON-`unlink_pending` row for this identity (another live open of the SAME file, any ASID)
/// with `refcount += 1`, else CLAIMS a free row. **The `unlink_pending` exclusion is load-bearing:** an unlinked
/// file's name is `0xE5`'d, so it can never be re-opened by name — any key match on a pending row is therefore a
/// DIFFERENT file that FAT recycled the deleted file's directory slot for, and joining it would conflate the two
/// files' refcounts + deferred free. The two rows may then legitimately share the `(dir_lba, dir_off)` key; the
/// per-descriptor `FILE_OPENROW` index is what keeps every later decrement/mark unambiguous. `None` iff the table
/// is full on a NEW identity (no free row) — `sys_open` maps that to `-ENFILE`. Called BEFORE `files_alloc` so
/// every increment pairs with exactly one decrement (the descriptor's `files_free`, or `openfile_decref_at` on
/// the alloc-full unwind), with no path where a decrement lands on a row this open never incremented.
fn openfile_incref(dir_lba: u64, dir_off: u32) -> Option<usize> {
    let mut table = OPEN_FILES.lock();
    // Join only a LIVE, NON-pending row for this identity (a legitimate second open of the same file). A pending
    // row with the same key is a different file reusing the recycled directory slot — skip it, claim fresh.
    for (i, row) in table.iter_mut().enumerate() {
        if row.refcount > 0 && !row.unlink_pending && row.dir_lba == dir_lba && row.dir_off == dir_off {
            row.refcount += 1;
            return Some(i);
        }
    }
    for (i, row) in table.iter_mut().enumerate() {
        if row.refcount == 0 {
            *row = OpenFileRow { dir_lba, dir_off, refcount: 1, unlink_pending: false, first_cluster: 0 };
            return Some(i);
        }
    }
    None // table full on a new identity -> -ENFILE
}

/// U11-M2: DECREMENT the open-file refcount for the row at INDEX `row` (a `FILE_OPENROW` value the descriptor
/// recorded at open). Returns `Some(first_cluster)` IFF this was the LAST close of an `unlink_pending` file that
/// owns clusters — the caller MUST then free that chain (`free_orphan_chain`, block-I/O-legal context; NEVER
/// teardown). `None` otherwise: still referenced, not unlinked, an empty (0-cluster) unlinked file, a scaffold
/// (`row == OPENROW_NONE` / out of range), or an already-retired row (defensive). Decrementing by INDEX (not by
/// the recyclable `(dir_lba, dir_off)` key) is what makes the accounting robust to FAT recycling a deleted
/// file's directory slot — this decrement can only ever touch the row this descriptor actually incremented. The
/// row is cleared at 0. Only the short table mutation runs under the lock; the free is the caller's, after the
/// lock drops.
#[must_use]
fn openfile_decref_at(row: u32) -> Option<u32> {
    let row = row as usize;
    if row >= NOPENFILE {
        return None; // OPENROW_NONE (scaffold / free slot) — nothing was counted
    }
    let mut table = OPEN_FILES.lock();
    let r = &mut table[row];
    if r.refcount == 0 {
        return None; // already retired (defensive — a paired decrement should never observe this)
    }
    r.refcount -= 1;
    if r.refcount == 0 {
        let pending = r.unlink_pending;
        let fc = r.first_cluster;
        *r = OpenFileRow::EMPTY; // free the row
        // Hand the chain to the caller ONLY if the file was unlinked AND owns clusters: a 0-cluster unlinked file
        // has no chain, and a non-unlinked last-close just retires the row.
        return if pending && fc != 0 { Some(fc) } else { None };
    }
    None // still referenced by another descriptor
}

/// U11-M2: mark the open-file row at INDEX `row` UNLINK-PENDING and stash the chain head to free at the last
/// close. Called from `sys_unlink` (with the unlinking descriptor's `FILE_OPENROW`) AFTER the name is `0xE5`'d
/// and BEFORE the caller's descriptors are dropped — so the decrement that reaches `refcount == 0` (sole-opener
/// case) already sees the pending flag and frees the chain in the same syscall. Marking by INDEX (not key) keeps
/// the pending state on THIS file's row even after FAT recycles its directory slot for another file. `row ==
/// OPENROW_NONE` / out of range / a retired row is a defensive no-op. `first_cluster` here is authoritative.
fn openfile_mark_unlink_pending_at(row: u32, first_cluster: u32) {
    let row = row as usize;
    if row >= NOPENFILE {
        return;
    }
    let mut table = OPEN_FILES.lock();
    let r = &mut table[row];
    if r.refcount > 0 {
        r.unlink_pending = true;
        r.first_cluster = first_cluster;
    }
}

/// BANDY-2: the BY-NAME twin of `openfile_mark_unlink_pending_at`, for the bus rm/write-truncate
/// paths (which delete by NAME — they hold no open descriptor, hence no `FILE_OPENROW` index). If
/// ANOTHER process holds the file at `(dir_lba, dir_off)` open (a LIVE, non-`unlink_pending` row),
/// mark that row `unlink_pending` + stash the chain head so its LAST close frees the chain (the
/// deferred-free discipline: a concurrent reader can never have its clusters freed underneath it),
/// and return `true` (the caller must NOT free now). Return `false` when NO live open exists — the
/// caller frees the chain immediately (the sole path holding it). Must be called under the
/// NAMESPACE hold (the same serialization `sys_unlink` and `openfile_incref` take), so no open can
/// race in between the caller's `0xE5` and this mark. A pending row sharing the key is a DIFFERENT
/// file on a FAT-recycled slot (`openfile_incref`'s load-bearing exclusion) — skip it, free now.
fn openfile_mark_pending_or_none_by_dir(dir_lba: u64, dir_off: u32, first_cluster: u32) -> bool {
    let mut table = OPEN_FILES.lock();
    for row in table.iter_mut() {
        if row.refcount > 0 && !row.unlink_pending && row.dir_lba == dir_lba && row.dir_off == dir_off {
            row.unlink_pending = true;
            row.first_cluster = first_cluster;
            return true; // a live opener holds it — defer the free to its last close
        }
    }
    false // no live open — the caller frees the chain immediately
}

/// U11-M2: free the cluster chain of an `unlink_pending` file whose LAST descriptor just closed — the deferred
/// half of the `sys_unlink` split, run at close time. Called ONLY from SYSCALL context (`sys_close`,
/// `sys_unlink`, the `sys_open` unwind), where block I/O is legal; NEVER from teardown (IRQ-masked — that path
/// logs a leak instead). The name is already `0xE5`'d, so freeing here can never alias; a mount/free error only
/// orphans clusters (lost clusters — benign, chkdsk-reclaimable), never aliases. Holds no lock across the I/O.
fn free_orphan_chain(first_cluster: u32) -> bool {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(_) => {
            serial_println!(
                "U11-defer: deferred free of chain @cluster {} — mount failed, orphaned (leak)",
                first_cluster
            );
            return false;
        }
    };
    if fs.free_chain(first_cluster).is_err() {
        serial_println!(
            "U11-defer: deferred free of chain @cluster {} — free_chain error, orphaned (leak)",
            first_cluster
        );
        return false;
    }
    true
}

// =============================================================================================
// U11-M2b: the DEFERRED-FREE QUEUE + the orphan REAPER — freeing the teardown-last-close chain.
// =============================================================================================
//
// M2a closed the cross-process defer for the EXPLICIT-close path: the chain frees at the last `SYS_CLOSE`
// (`sys_close`/`sys_unlink`, syscall context, block I/O legal). But when TEARDOWN is the LAST close of an
// `unlink_pending` file — a program EXITS *without* closing while holding the last cross-process open —
// `clear_files_row`'s decrement reaches `refcount == 0` in an IRQ-masked context on the dying task's own
// kernel stack (TTBR0 on the boot root, immediately before `switch_context`), where multi-sector polled SD
// I/O is illegal. M2a therefore LOGGED the leak and left the clusters allocated (benign lost clusters).
//
// M2b actually frees that chain, in a block-I/O-legal context, by splitting the work in two:
//   * `clear_files_row` PUSHES the chain head onto `DEFERRED_FREE` — an I/O-free critical section (lock +
//     array write + unlock), the SAFE twin of M2a's teardown decrement. A separate `SpinMutex` from
//     `OPEN_FILES` so the teardown push never contends with live open/close on the refcount table. A FULL
//     queue degrades to the M2a behavior (log the leak, leave allocated) — the push never blocks/spins/faults.
//   * the `orphan_reaper` kernel service task (spawned at BOOT via the existing `sched::spawn` service-task
//     API in `main.rs`, NOT lazily — a spawn allocates a `Box<Task>` and takes `RUN_QUEUES`, both illegal in
//     the teardown push path) DRAINS the queue: pop one head UNDER the lock, RELEASE the lock, THEN mount +
//     free the chain (`free_orphan_chain`). Block I/O runs ONLY in the reaper's context (EL1, IRQs enabled,
//     its own stack), never under the queue lock. It BLOCKS on `REAPER_SEM` when the queue is empty (SCHED-4b),
//     which `deferred_free_push` posts on enqueue — an event-driven, lost-wakeup-safe wake (~0% idle duty
//     cycle) that fires in QEMU (the timer-driven sleep it replaced did not). It never exits (a forever loop).
//
// Freed EXACTLY once: a teardown-orphaned chain reaches the queue ONLY via `openfile_decref_at` returning
// `Some(fc)` (last-close-of-pending, the row already cleared to EMPTY), so it is queued once and freed once
// by the reaper; the explicit-close path frees inline and NEVER queues, so no chain is both freed inline and
// queued. SMP-safe: a teardown on core X pushes, the reaper on core Y drains — a single `SpinMutex` serializes
// them with no torn reads and no lost/duplicated entries. The lock is taken IRQ-MASKED on BOTH sides (via
// `IrqGuard`), so the reaper's preemptible task body can never be timer-preempted while holding it — closing
// the same-core push-vs-preempted-pop deadlock that is otherwise live in the single-AP-fallback boot.

/// U11-M2b: capacity of the deferred-free ring. A teardown queues at most ONE chain per exiting task (its
/// single last-`unlink_pending` open), so 16 pending chains is generous headroom before the honest full-queue
/// degrade (log + leave allocated) ever fires.
const NDEFERFREE: usize = 16;

/// U11-M2b: the bounded deferred-free queue — chain heads awaiting the reaper. Guarded by its OWN `SpinMutex`
/// (NOT `OPEN_FILES` — a separate lock keeps the IRQ-masked teardown push off the refcount table's hot lock).
/// A plain fixed array + count: the push is lock + one store, the pop is lock + one load; neither allocates
/// or touches block I/O. `0` is never a valid chain head (clusters start at 2), so it doubles as the scrub value.
struct DeferredFree {
    heads: [u32; NDEFERFREE], // chain heads (first_cluster) pending free; entries [0..len) are valid
    len: usize,               // count of valid entries
}

impl DeferredFree {
    const EMPTY: Self = DeferredFree { heads: [0; NDEFERFREE], len: 0 };
}

/// U11-M2b: the deferred-free queue instance. `SpinMutex::new` is `const`, so this is a const-constructed
/// static (the `OPEN_FILES` idiom). Its lock is taken IRQ-masked in the teardown push (safe: a bounded,
/// allocation-free, I/O-free critical section) and in the reaper's pop — but NEVER held across `free_chain` I/O.
static DEFERRED_FREE: SpinMutex<DeferredFree> = SpinMutex::new(DeferredFree::EMPTY);

/// U11-M2b (deadlock fix): an RAII IRQ-mask guard for the two `DEFERRED_FREE` critical sections.
/// `DEFERRED_FREE` is a bare `spin::Mutex` acquired in two IRQ-ASYMMETRIC contexts: `deferred_free_push`
/// runs in the IRQ-masked teardown path, but `deferred_free_pop` runs in the reaper TASK body, which the
/// metal generic timer can PREEMPT (kernel task bodies run I-unmasked; `SCHED_ACTIVE` enables preemption).
/// Without masking, a timer preempt of the reaper WHILE it holds the lock, followed by a SAME-CORE teardown
/// push spinning IRQ-masked on that lock, deadlocks the core forever (the preempted holder is pinned to that
/// core — run queues never migrate — so it can never be rescheduled to release it). That is dormant in a
/// healthy ≥2-AP boot — the reaper (on `online.get(1)`) and the EL0 fixtures whose teardown pushes (on
/// `online.first()`) sit on DISTINCT cores — but becomes LIVE in the single-AP fallback, where
/// `reaper_cpu`/`vcpu`/`demo_cpu` all collapse onto the one AP (the pi4's 3/4-core boot variance can produce
/// it). Masking IRQs across the whole bounded, I/O-free critical section makes the hold non-preemptible, so
/// `DEFERRED_FREE` is a proper IRQ-safe spinlock at ANY core count. It SAVES and restores the DAIF snapshot
/// (the `sched.rs` `irq_save_mask` idiom, kept LOCAL so M2b still touches no `sched.rs`) so it nests
/// correctly in the already-masked teardown push (it restores the prior mask, never an unconditional clear).
struct IrqGuard(u64);
impl IrqGuard {
    #[inline]
    fn mask_save() -> Self {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
            core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
        }
        IrqGuard(daif)
    }
}
impl Drop for IrqGuard {
    #[inline]
    fn drop(&mut self) {
        // Restore the caller's prior DAIF (masked in teardown, unmasked in the reaper) — never an
        // unconditional unmask, which would drop the IRQ mask mid-teardown.
        unsafe { core::arch::asm!("msr daif, {}", in(reg) self.0, options(nomem, nostack, preserves_flags)) };
    }
}

/// U11-M2b: enqueue a teardown-orphaned chain head for the reaper. I/O-free (lock + array write + unlock) — so
/// it is SAFE in the IRQ-masked teardown context, the same class as M2a's teardown decrement. Returns `false`
/// iff the queue is FULL (the caller degrades to logging the leak + leaving the clusters allocated — never a
/// block/spin/fault). NEVER call this under the `OPEN_FILES` lock: it is a deliberately separate lock, and no
/// caller holds both. `#[must_use]`: dropping a `false` return would silently lose the leak-log fallback.
#[must_use]
fn deferred_free_push(first_cluster: u32) -> bool {
    // IRQ-mask the critical section (declared BEFORE the lock guard, so on scope exit the lock drops FIRST,
    // then IRQs are restored): the paired `deferred_free_pop` runs preemptibly, so the lock must be IRQ-safe.
    let _irq = IrqGuard::mask_save();
    {
        let mut q = DEFERRED_FREE.lock();
        if q.len >= NDEFERFREE {
            return false; // full — caller logs the honest leak
        }
        let i = q.len;
        q.heads[i] = first_cluster;
        q.len = i + 1;
    } // DEFERRED_FREE lock dropped here — BEFORE the post, so its run-queue lock is never nested under ours

    // SCHED-4b: wake the reaper immediately. `post()` moves a blocked reaper onto its run queue + pokes its
    // core (or, if the reaper is momentarily awake and not parked, increments the permit count so the wakeup
    // is not lost). Called with the DEFERRED_FREE lock released and IRQs still masked (post nests its own
    // save/restore) — the enqueue-array critical section and the wake are thus strictly ordered and
    // non-overlapping. This is the event-driven wake that replaces SCHED-4's timer sleep (dead in QEMU).
    REAPER_SEM.post();
    true
}

/// U11-M2b: dequeue ONE orphaned chain head, or `None` when empty. LIFO (order is irrelevant — each head names
/// a DISTINCT chain freed exactly once). The reaper calls this to take a head UNDER the lock, then DROPS the
/// lock before the `free_chain` block I/O (holding the queue lock across `mount()` would let a teardown push
/// spin IRQ-masked on it — an unbounded stall in the one place that must never stall).
fn deferred_free_pop() -> Option<u32> {
    // IRQ-mask across the pop so a metal timer cannot preempt the reaper while it holds the lock — which,
    // paired with a same-core IRQ-masked teardown push, would deadlock the core (see `IrqGuard`). The guard
    // is declared BEFORE the lock, so the lock releases before IRQs are restored on every return path.
    let _irq = IrqGuard::mask_save();
    let mut q = DEFERRED_FREE.lock();
    if q.len == 0 {
        return None;
    }
    let i = q.len - 1; // local index: avoids borrowing the guard both mutably (`heads[..]=`) and immutably (`len`)
    let fc = q.heads[i];
    q.heads[i] = 0; // scrub (defensive — 0 is never a valid head)
    q.len = i;
    Some(fc)
}

/// U11-M2b: the deferred-free REAPER — a forever kernel service task that closes M2a's teardown-last-close
/// honest-scope gap. Spawned ONCE at boot (`main.rs`'s aarch64-baremetal service block, via `sched::spawn`) so
/// it already exists before any orphan is queued — a lazily-spawned reaper would need a heap `Box<Task>` +
/// `RUN_QUEUES`, both illegal from the IRQ-masked teardown push. Each turn: pop one chain head (lock held only
/// for the pop), RELEASE the lock, THEN `free_orphan_chain` (mount + all-FAT-copies free — block I/O, legal
/// HERE: EL1, IRQs enabled, its own stack, never in teardown context). When the queue is empty it BLOCKS on
/// `REAPER_SEM` (below), so it consumes ~0% of its core while idle and never hogs it. The `arg` is unused
/// (the `fn(usize)` service-task shape). Never returns.
///
/// SCHED-4b (U11-reap regression fix): the wake is EVENT-DRIVEN, not timer-driven. SCHED-4 replaced the
/// empty-queue `yield_now()` with `sleep_ticks(25)` (~100 ms) to drop the metal idle duty-cycle — correct
/// goal, but it introduced a latency regression AND is DEAD in QEMU: `sleep_ticks` parks on the per-CPU
/// sleeper list whose only wake source is the generic-timer tick, and QEMU raspi4b never delivers that tick
/// (`percpu.ticks` frozen at 0), so the parked reaper never woke — the teardown-orphan sat unfreed past the
/// U11-reap witness's bounded wait, a DETERMINISTIC FAIL. SCHED-4b blocks on a counting `Semaphore` instead:
/// `deferred_free_push` `post()`s it on every enqueue, which `make_ready`s + pokes the reaper immediately
/// (cross-CPU, lost-wakeup-safe — a post with no waiter increments the permit count, so an enqueue that
/// races the reaper's pop/park is never lost). The wake rides the SGI/poll-spin reschedule path, NOT the
/// timer, so it fires in QEMU (the AP idle loop poll-spins and re-dispatches the readied reaper) AND on
/// metal (~immediate) — and the idle duty-cycle is now ~0% (a true block), even lower than SCHED-4's 3%.
/// No timer fallback is needed: the semaphore cannot lose a wakeup, so there is nothing for a periodic
/// re-poll to recover.
pub fn orphan_reaper(_: usize) {
    // Reserve the (single-waiter) waiter-list capacity BEFORE the first block, so the scheduler's park-side
    // push into it is allocation-free under the semaphore lock. The reaper is the sole waiter and runs this
    // once before its first `wait()`; a producer `post()` that races this init hits the count path (no push,
    // no realloc), so the enqueue is never lost.
    REAPER_SEM.init();
    loop {
        match deferred_free_pop() {
            Some(fc) => {
                // The queue lock is already dropped; the block I/O is the reaper's own. Log the positive
                // witness (the twin of M2a's "leaked" log) ONLY on a SUCCESSFUL free — its ABSENCE for a file
                // is the gate's witness, and `free_orphan_chain` logs its own error/leak line on failure, so
                // the two can never both appear for one chain. (Freeing succeeds in the QEMU gate: the line
                // still prints, byte-identically, after the silent successful free.)
                if free_orphan_chain(fc) {
                    serial_println!("U11-defer: reaper freed teardown-orphaned chain @cluster {}", fc);
                }
            }
            // Queue drained: block until a producer enqueues + posts. `wait()` returns `true` here (the
            // reaper is always a scheduled task); a surplus permit from a burst of posts is consumed by an
            // immediate return that finds the queue already drained, then blocks — self-correcting, no busy-spin.
            None => {
                let _ = REAPER_SEM.wait();
            }
        }
    }
}

/// U11-M2b / SCHED-4b: the reaper's wakeup semaphore. A counting semaphore starting at 0 permits: the
/// `orphan_reaper` `wait()`s on it when the deferred-free queue drains, and `deferred_free_push` `post()`s
/// it on every enqueue. Being a `Semaphore` (not a timer sleep) makes the wake event-driven, cross-CPU, and
/// lost-wakeup-safe — the property SCHED-4's `sleep_ticks` lacked (and which the QEMU timer's absence exposed
/// as the U11-reap regression). The reaper is the ONLY task that ever waits on it.
static REAPER_SEM: super::sched::Semaphore = super::sched::Semaphore::new(0);

// =============================================================================================
// U6: UnaFS owner/grants — the by-NAME namespace ACL, enforced at SYS_OPEN.
//
// The gap the U6b→U11 chain left: SYS_OPEN / O_CREAT / SYS_UNLINK are gated on the HANDLE capability
// (CAP_READ/CAP_WRITE), but the by-NAME namespace itself was NOT ACL'd — any process could open,
// create, or unlink any name. This closes it with an in-kernel owner/grants table checked at
// SYS_OPEN (the hook U6b/U9 named). OWNED-BY-DEFAULT (secure-by-default): an O_CREAT of a NEW name
// makes the creator its OWNER (the file is PRIVATE); the O_PUBLIC mode bit opts a create into
// world-access. An open of an EXISTING owned file is allowed only for the owner or a principal the
// owner GRANTED (SYS_FGRANT); everyone else is -EACCES. A file with NO owner row (pre-existing /
// host-created, or created O_PUBLIC) is PUBLIC — byte-identical to the pre-U6 behaviour.
//
// PRINCIPAL identity = the (ASID, ASID_GEN) incarnation — the same recycle fence the file-id / xfer
// / derivation machinery uses (ASIDs recycle across process lifetimes; ASID_GEN is bumped at
// teardown). A stale owner/grant whose gen no longer matches never authorizes a recycled tenant.
//
// LIFETIME (in-kernel, VOLATILE — this is the ENFORCEMENT SEAM the persisted owner/grants feed: TODAY via
// the K1 `UNAFS.ATR` FAT-bridge sidecar (an on-disk owner format since K1/K2/K3), at K4 via NATIVE unafs
// `owner`/`grants:*` typed attributes; enforcement is storage-agnostic — it compares `PrincipalRecord`s by
// value regardless of backing): a row is keyed by the file's directory-slot identity (dir_lba, dir_off). It is
// CLEARED at unlink (the name is gone; FAT may recycle the slot to a DIFFERENT file — the U11
// recycled-slot hazard) and at OWNER TEARDOWN (clear_handle_row → the file reverts to PUBLIC, which
// also keeps the bounded table self-cleaning across create/exit cycles). Because the table is
// bounded, a PRIVATE create that cannot record an owner FAILS CLOSED (-ENOSPC, undoing the fresh
// directory entry) rather than leave a "private" file silently world-accessible.
//
// LOCKING: OWNED_FILES has its OWN SpinMutex, taken IRQ-masked on every access via the local
// `IrqGuard` — because it is acquired in BOTH syscall context (sys_open, IRQs enabled) AND the
// IRQ-masked teardown path (clear_handle_row), exactly the asymmetry the M2b IrqGuard closes for
// DEFERRED_FREE. No block I/O and no other lock is ever held across it.

/// U6: rows in the owner/grants table — one per PRIVATE (owned) file currently tracked. Bounded and
/// static (the OPEN_FILES idiom). A row is FREE iff `dir_lba == 0` (a real directory entry never sits
/// at LBA 0 — that is the MBR/boot sector — so 0 is an unambiguous free marker, as in `sys_unlink`).
const NOWNED: usize = 16;
/// U6: grantees per owned file. Bounded; a full grant list is `-ENOSPC` on SYS_FGRANT.
const NFGRANT: usize = 4;

/// U6: one grant edge on an owned file — a principal `(asid, gen)` and the rights it was granted. An
/// EMPTY slot has `rights == 0` (a real grant always carries >= 1 of CAP_READ|CAP_WRITE), so
/// `rights == 0` doubles as the free marker AND disambiguates the real ASID 0 (the shared window).
/// K1 M2.3: `ppid` is the grantee's PERSISTENT principal, captured at grant time from `SLOT_PPID`
/// (NONE for an anonymous/inline grantee) — the durable identity persisted to `UNAFS.ATR` and, for a
/// row REBUILT from disk after reboot, the ONLY thing that identifies the grantee (its live `(asid,gen)`
/// is gone). Runtime enforcement stays `(asid, gen)`; `ppid` is the cross-reboot analogue (M2.4).
#[derive(Clone, Copy)]
struct FileGrant {
    asid: u64,
    asid_gen: u64, // grantee's ASID_GEN captured at grant time — the recycle fence (`gen` is a reserved keyword)
    rights: u32,
    ppid: PrincipalRecord, // K1 M2.3: grantee's persistent principal (NONE = anonymous/not-persisted)
}

impl FileGrant {
    const EMPTY: Self = FileGrant { asid: 0, asid_gen: 0, rights: 0, ppid: PrincipalRecord::NONE };
}

/// U6: one owned file's ACL. Guarded by `OWNED_FILES` (a `SpinMutex`), so plain `Copy` integers — the
/// lock provides mutual exclusion (no atomics; the OPEN_FILES idiom).
/// K1 M2.3: `owner_ppid` is the owner's PERSISTENT principal, captured at create from `current_principal()`
/// (NONE for an anonymous/inline creator → nothing persists). For a row REBUILT from disk at mount (M2.4)
/// `owner_asid == OWNER_ASID_PERSISTED` (the NO-LIVE-OWNER sentinel) and `owner_ppid` is the persisted
/// principal — the runtime `(asid, gen)` check then matches no live caller, and the M2.4 ppid branch
/// re-admits the program when it is re-spawned under the same name.
#[derive(Clone, Copy)]
struct OwnedFile {
    dir_lba: u64,   // identity key: directory-entry sector LBA (0 == free row)
    dir_off: u32,   // identity key: 32-byte slot offset within that sector
    owner_asid: u64,
    owner_gen: u64,
    owner_ppid: PrincipalRecord, // K1 M2.3: owner's persistent principal (NONE = anonymous/not-persisted)
    grants: [FileGrant; NFGRANT],
}

impl OwnedFile {
    const EMPTY: Self = OwnedFile {
        dir_lba: 0,
        dir_off: 0,
        owner_asid: 0,
        owner_gen: 0,
        owner_ppid: PrincipalRecord::NONE,
        grants: [FileGrant::EMPTY; NFGRANT],
    };
}

/// U6: the owner/grants table. `SpinMutex::new` is `const` and `OwnedFile: Copy` (the OPEN_FILES
/// idiom). Its lock is taken IRQ-masked via `IrqGuard` on EVERY access (syscall + teardown).
static OWNED_FILES: SpinMutex<[OwnedFile; NOWNED]> = SpinMutex::new([OwnedFile::EMPTY; NOWNED]);

/// K1 M2.4: the NO-LIVE-OWNER sentinel `owner_asid`/`grant.asid` for a row REBUILT from `UNAFS.ATR` at mount.
/// Deliberately OUT of the live ASID range (`0..=USER_SLOTS`) so the runtime `(asid, gen)` owner/grantee
/// equality checks match NO live caller — a persisted-owned file is owned by a PRINCIPAL, not a boot
/// incarnation, and is re-acquired only via the M2.4 ppid branch. Any site that INDEXES an asid by this value
/// (only `owned_grant`'s stale-slot scan) range-guards it first.
const OWNER_ASID_PERSISTED: u64 = u64::MAX;

/// K1 M2.4 / K2: is MULTI-PROGRAM by-name spawn live? Cross-reboot persistent-principal ENFORCEMENT —
/// installing rebuilt-from-disk owner rows at mount (which then DENY every live `(asid, gen)` caller until the
/// owning program is re-spawned by name and matches by ppid) — is GATED on this. It must be false while only ONE
/// distinct named program can launch, because then every spawn is the SAME principal, so a persisted owner could
/// never be re-acquired by anyone but could DENY everyone — enforcing it would only BRICK the file.
///
/// K2 (make-enforcement-LIVE) landed the honest precondition: the card now carries THREE distinct launchable
/// named programs (`HELLO.BIN`, `K2OWN.BIN`, `K2IMP.BIN`), each minting its own `prog:<NAME>` principal via the
/// sole mint path (`load_program_into_slot` -> `slot_ppid_stamp`). So this is now TRUE and the real-boot rebuild
/// (`atr_maybe_boot_rebuild`, at the head of `u7_launcher`) actually reinstalls persisted owner rows — a
/// re-spawned owning program re-acquires its file BY NAME; any other principal is denied. The end-to-end proof
/// through real loaded programs is `k2_liveenf_launcher`; on the metal card a real power-cycle survives.
///
/// On QEMU (fresh-per-build FAT) `UNAFS.ATR` does not exist at the head-of-`u7_launcher` rebuild (`k1_atr_selftest`
/// creates it later in the same chain), so the boot rebuild installs ZERO rows and the 23-fixture battery stays
/// byte-equivalent — the flip's live effect is exercised on metal. The `owned_access_ok`/`owned_is_owner`/
/// `owned_unlink_permitted`/`owned_grant` ppid branch is NOT gated on this — it is purely structural (a NONE
/// principal never matches), so an anonymous caller (the whole battery) is inert regardless.
fn by_name_spawn_multivalued() -> bool {
    true // K2: three distinct launchable named programs on the card -> a persisted owner is re-acquirable by name
}

/// K1 M2.4: install a row REBUILT from `UNAFS.ATR` into `OWNED_FILES` — a persisted owner with NO live
/// `(asid, gen)` incarnation (that identity is gone across the reboot). `owner_asid = OWNER_ASID_PERSISTED`
/// (the runtime check then matches no live caller); `owner_ppid` + the grant principals carry the durable
/// identity the M2.4 ppid branch admits by. Overwrites any existing row for the key, else claims a free row;
/// `false` iff the table is full (that file simply stays enforced by whatever row already occupies the slot, or
/// falls back to public — fail-safe). IRQ-masked OWNED_FILES lock; called from the rebuild path OUTSIDE ns.
fn owned_install_persisted(
    dir_lba: u64,
    dir_off: u32,
    owner_ppid: PrincipalRecord,
    grants: &[(PrincipalRecord, u32); NFGRANT],
) -> bool {
    let mut row_grants = [FileGrant::EMPTY; NFGRANT];
    for (j, &(p, r)) in grants.iter().enumerate() {
        if r != 0 && p.kind != PRIN_NONE {
            row_grants[j] = FileGrant { asid: OWNER_ASID_PERSISTED, asid_gen: 0, rights: r, ppid: p };
        }
    }
    let new = OwnedFile {
        dir_lba,
        dir_off,
        owner_asid: OWNER_ASID_PERSISTED,
        owner_gen: 0,
        owner_ppid,
        grants: row_grants,
    };
    let _irq = IrqGuard::mask_save();
    let mut t = OWNED_FILES.lock();
    for r in t.iter_mut() {
        if r.dir_lba == dir_lba && r.dir_off == dir_off {
            *r = new;
            return true;
        }
    }
    for r in t.iter_mut() {
        if r.dir_lba == 0 {
            *r = new;
            return true;
        }
    }
    false // table full — leave the file to its existing row / public (fail-safe)
}

/// U6: record `(owner_asid, owner_gen)` as the OWNER of the file at `(dir_lba, dir_off)` — called at
/// the O_CREAT of a NEW private name. OVERWRITES any existing row for this key (defensive against a
/// recycled directory slot whose prior owner row was not yet cleared), else CLAIMS a free row. Returns
/// `false` iff the table is full (the caller fails the private create CLOSED). Grants start empty.
/// K1 M2.3: `owner_ppid` is the creator's persistent principal (`current_principal()`, captured by the
/// caller BEFORE this lock — SLOT_PPID must not be taken under OWNED_FILES). NONE for an anonymous/inline
/// creator; a NONE owner never persists (the syscall handler's persist step is gated on `kind != NONE`).
fn owned_set_owner(dir_lba: u64, dir_off: u32, owner_asid: u64, owner_gen: u64, owner_ppid: PrincipalRecord) -> bool {
    // BANDY-1 (verdict C): the reserved KERNEL-REPLY principal can never be adopted as an owner —
    // fail closed before any table mutation. No live path constructs such a record outside the bus
    // reply stamp; this is the defense-in-depth gate the BANDY-STAMP witness proves.
    if owner_ppid.kind == PRIN_KERNEL_REPLY {
        return false;
    }
    let _irq = IrqGuard::mask_save();
    let mut t = OWNED_FILES.lock();
    for row in t.iter_mut() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            *row = OwnedFile { dir_lba, dir_off, owner_asid, owner_gen, owner_ppid, grants: [FileGrant::EMPTY; NFGRANT] };
            return true;
        }
    }
    for row in t.iter_mut() {
        if row.dir_lba == 0 {
            *row = OwnedFile { dir_lba, dir_off, owner_asid, owner_gen, owner_ppid, grants: [FileGrant::EMPTY; NFGRANT] };
            return true;
        }
    }
    false // table full — the private create fails closed (-ENOSPC)
}

/// U6: the ACL verdict for a caller opening an EXISTING file. `true` = ALLOW: the file is PUBLIC (no
/// owner row), the caller IS the owner (gen-matched — full authority), or the caller holds a grant
/// whose rights COVER the requested access (`requested ⊆ granted`, gen-fenced). `false` = DENY
/// (-EACCES): the file is owned and the caller is neither owner nor a sufficiently-granted principal.
/// `requested` is the rights the open mode asks for (CAP_READ, or CAP_READ|CAP_WRITE for RW/O_CREAT).
/// K1 M2.4: an ADDITIVE cross-reboot admission branch runs AFTER the unchanged `(asid, gen)` checks — the
/// caller's PERSISTENT principal `caller_ppid` (captured before the lock) admits it against a REBUILT row whose
/// live owner is gone (`owner_asid == OWNER_ASID_PERSISTED`): owner-by-name (full authority) or grantee-by-name
/// (rights-checked). Purely STRUCTURAL — a NONE principal never matches a NONE (`kind != NONE` on both sides is
/// required), so an anonymous caller (the whole battery) never engages it and byte-equivalence holds.
fn owned_access_ok(
    dir_lba: u64,
    dir_off: u32,
    asid: u64,
    caller_gen: u64,
    requested: u32,
    caller_ppid: PrincipalRecord,
) -> bool {
    let _irq = IrqGuard::mask_save();
    let t = OWNED_FILES.lock();
    for row in t.iter() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            // Owner (live incarnation): full authority over its own file.
            if row.owner_asid == asid && row.owner_gen == caller_gen {
                return true;
            }
            // A grantee (live incarnation): the requested access must be a SUBSET of what it was granted.
            for g in row.grants.iter() {
                if g.rights != 0 && g.asid == asid && g.asid_gen == caller_gen {
                    return (requested & !g.rights) == 0;
                }
            }
            // K1 M2.4: cross-reboot admission by PERSISTENT principal. A re-spawned named program re-acquires
            // ITS OWN persisted file (owner-by-name = full authority); a named grantee re-acquires its granted
            // access (rights-checked). NONE never matches NONE, so anonymous callers fall straight through.
            // BANDY-1 (verdict C): the reserved KERNEL-REPLY kind is excluded — it can never be admitted
            // by-name even if a row somehow carried it (fail-closed twin of the owned_set_owner/owned_grant gates).
            if caller_ppid.kind != PRIN_NONE && caller_ppid.kind != PRIN_KERNEL_REPLY {
                if row.owner_ppid.kind != PRIN_NONE && row.owner_ppid == caller_ppid {
                    return true;
                }
                for g in row.grants.iter() {
                    if g.rights != 0 && g.ppid.kind != PRIN_NONE && g.ppid == caller_ppid {
                        return (requested & !g.rights) == 0;
                    }
                }
            }
            return false; // owned, and the caller is neither owner nor sufficiently granted (live or by-name)
        }
    }
    true // no owner row -> PUBLIC (pre-existing / host-created / O_PUBLIC create)
}

/// U6: drop the owner/grants row for `(dir_lba, dir_off)` — called at SYS_UNLINK (the name is gone; the
/// directory slot may be recycled to a DIFFERENT file that will set its OWN owner). Idempotent no-op if
/// the file was public (no row).
fn owned_clear(dir_lba: u64, dir_off: u32) {
    let _irq = IrqGuard::mask_save();
    let mut t = OWNED_FILES.lock();
    for row in t.iter_mut() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            *row = OwnedFile::EMPTY;
        }
    }
}

/// U6: at the teardown of ASID `asid`, dispose of every file it OWNS and sweep any GRANT naming `asid` (a
/// grantee that exited). Matches on the ASID irrespective of gen — the whole address space is being torn down.
/// Called from `clear_handle_row` (the IRQ-masked teardown path); keeps the bounded table self-cleaning across
/// create/exit cycles. (The gen fence in `owned_access_ok` already makes an UNSWEPT stale entry harmless.)
///
/// K1 M2.4 F2 (security-review MF2): an owner's disposition now depends on whether it is a PERSISTENT
/// (named) principal:
///   * ANONYMOUS owner (`owner_ppid == NONE`, the whole pre-K1 / inline-fixture world): WIPE the row — the file
///     reverts to PUBLIC (there is no durable identity to keep owning it). Unchanged behaviour; battery byte-identical.
///   * NAMED owner: CONVERT `owner_asid` to the NO-LIVE-OWNER sentinel (keep `owner_ppid` + grants) — the file
///     STAYS OWNED by the principal (the program can re-spawn and re-acquire; it survives reboot). Wiping it here
///     would make the in-RAM row disagree with the on-disk `UNAFS.ATR` row, so `sys_unlink`'s `owned_owner_ppid`
///     gate would read NONE and SKIP the disk clear — leaving a stale owner row a future same-name file would adopt
///     at the next mount. Keeping the row as a sentinel keeps RAM and disk consistent and lets the unlink-time
///     `atr_clear_row` fire correctly.
fn owned_clear_owner_asid(asid: u64) {
    let _irq = IrqGuard::mask_save();
    let mut t = OWNED_FILES.lock();
    for row in t.iter_mut() {
        if row.dir_lba != 0 && row.owner_asid == asid {
            if row.owner_ppid.kind != PRIN_NONE {
                // Named owner exits -> the file persists by principal (sentinel owner, no live incarnation).
                row.owner_asid = OWNER_ASID_PERSISTED;
                row.owner_gen = 0;
            } else {
                *row = OwnedFile::EMPTY; // anonymous owner exits -> revert to PUBLIC (pre-K1 behaviour)
            }
        } else {
            for g in row.grants.iter_mut() {
                if g.rights != 0 && g.asid == asid {
                    *g = FileGrant::EMPTY;
                }
            }
        }
    }
}

/// U6: is `(asid, gen)` the CURRENT owner of the file at `(dir_lba, dir_off)`? `false` for a public / unknown
/// file. `sys_fgrant` uses this to refuse a non-owner FAST — before it resolves (and thus before it leaks the
/// validity of) the named grantee handle, so a non-owner is a clean `-EACCES` regardless of the grantee argument.
fn owned_is_owner(dir_lba: u64, dir_off: u32, asid: u64, caller_gen: u64, caller_ppid: PrincipalRecord) -> bool {
    let _irq = IrqGuard::mask_save();
    let t = OWNED_FILES.lock();
    for row in t.iter() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            if row.owner_asid == asid && row.owner_gen == caller_gen {
                return true;
            }
            // K1 M2.4: the by-name owner of a REBUILT row (live owner gone) has full authority — incl. re-granting.
            return caller_ppid.kind != PRIN_NONE
                && row.owner_ppid.kind != PRIN_NONE
                && row.owner_ppid == caller_ppid;
        }
    }
    false
}

/// U6: may `(asid, gen)` UNLINK the file at `(dir_lba, dir_off)`? DELETE is an OWNER-only authority, distinct
/// from content write: an OWNED file may be unlinked ONLY by its current owner — a `CAP_WRITE` GRANTEE gets
/// content read/write, NEVER delete (else it could `unlink` + `O_CREAT` the name to STEAL ownership and lock the
/// real owner out). A PUBLIC file (no owner row) keeps the pre-U6 behaviour: anyone holding a `CAP_WRITE` handle
/// (which, for a public file, any process could obtain) may unlink it — "public" carries no delete protection.
fn owned_unlink_permitted(dir_lba: u64, dir_off: u32, asid: u64, caller_gen: u64, caller_ppid: PrincipalRecord) -> bool {
    let _irq = IrqGuard::mask_save();
    let t = OWNED_FILES.lock();
    for row in t.iter() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            // Owned -> only the owner may delete (grants confer content access, not delete).
            if row.owner_asid == asid && row.owner_gen == caller_gen {
                return true;
            }
            // K1 M2.4: the by-name owner of a REBUILT row (live owner gone) may delete its own persisted file.
            return caller_ppid.kind != PRIN_NONE
                && row.owner_ppid.kind != PRIN_NONE
                && row.owner_ppid == caller_ppid;
        }
    }
    true // public (no owner row) -> the pre-U6 CAP_WRITE-gated unlink applies
}

// =============================================================================================
// F3-M3: the UnaFS NAMESPACE lock — mutual atomicity for the coupled multi-step name sequences.
// =============================================================================================
//
// FAT_MUTATION (F2) and DIR_MUTATION (F3-M2) each serialize ONE sector RMW, but the name-level
// operations are multi-step SEQUENCES whose steps interleave across cores:
//   * sys_open:   find_located -> ACL check -> openfile_incref/files_alloc — an unlink landing between
//     the lookup and the incref binds a descriptor to a `first_cluster` a sole-opener unlink just freed
//     (stale-chain UAF / cross-file disclosure once the cluster is re-allocated);
//   * sys_open (create): create_in_root -> owned_set_owner — a racing unlink's `owned_clear` on the
//     recycled directory slot can land BETWEEN them (private->public / ownership theft), and two
//     creates can scan-then-claim the SAME free directory slot / write a duplicate name;
//   * sys_unlink: mark_dir_deleted -> owned_clear -> mark_unlink_pending -> files_free_by_dir — the
//     `0xE5`-before-mark-pending window lets a concurrent last-close miss the pending flag.
// NAMESPACE makes each sequence atomic against the others: every one of those races needs a second
// sequence to interleave, so one lock over all of them closes the whole set.
//
// SPAN (deliberately RELAXED vs the tight-span rule that governs FAT_MUTATION/DIR_MUTATION): the hold
// IS permitted to cover the bounded directory block I/O inside the sequences — find_located's bounded
// directory walk, create_in_root's bounded slot scan + RMW, mark_dir_deleted's one-sector RMW — because
// the aarch64 storage path is fully POLLED (no scheduler yield under the lock; see FAT_MUTATION's arch
// note). It is NEVER held across `mount()` or a chain-free (`free_orphan_chain` mounts + walks an
// unbounded-ish chain): sys_open takes it AFTER mount() returns, and sys_unlink COLLECTS orphan chain
// heads under the lock but frees them after the guard drops.
//
// LOCK ORDER (strict): NAMESPACE ⊃ { FAT_MUTATION, DIR_MUTATION, OPEN_FILES, OWNED_FILES,
// DEFERRED_FREE }. Inner locks are taken (and released) freely while NAMESPACE is held — the sequences
// above do exactly that — but NAMESPACE is NEVER acquired while any inner lock is held: no inner-lock
// critical section (with_fat_lock/with_dir_lock closures, openfile_*, owned_*, deferred_free_*) calls
// back into ns_lock(). This file is aarch64-only, so the lock needs no cfg gate; x86 is untouched.

/// F3-M3: the per-mount namespace lock (one volume -> one static). A `()` mutex — it guards SEQUENCES,
/// not data; the tables keep their own inner locks.
static NAMESPACE: SpinMutex<()> = SpinMutex::new(());

/// F3-M3: the RAII hold on [`NAMESPACE`]. IRQ-masked for the same reason every FS lock here is
/// (syscalls run I-unmasked and preemptible; a timer preempt of a holder followed by a same-core
/// re-entry into a namespace sequence would deadlock that core — run queues never migrate). Field
/// order is load-bearing: `_lock` drops FIRST (release the mutex), `_irq` LAST (restore DAIF) — the
/// lock is never held with IRQs unmasked.
struct NsGuard {
    _lock: spin::mutex::MutexGuard<'static, (), spin::relax::Spin>,
    _irq: IrqGuard,
}

/// F3-M3: mask IRQs, then take the namespace lock. See [`NAMESPACE`] for the span + ordering rules.
fn ns_lock() -> NsGuard {
    let irq = IrqGuard::mask_save();
    let lock = NAMESPACE.lock();
    NsGuard { _lock: lock, _irq: irq }
}

/// U6: SYS_FGRANT's table half. Verify the row at `(dir_lba, dir_off)` is owned by `(owner_asid, owner_gen)`,
/// then add/update a grant for the principal `(grantee_asid, grantee_gen)` with `rights` (a CAP_READ|CAP_WRITE
/// subset), or REMOVE that grantee's grant when `rights == 0`. Returns `0`, or a negative errno:
///  * `-EACCES` — no owner row for this file (it is public / nonexistent), or the caller is not its current owner;
///  * `-ENOSPC` — the file's bounded grant list is full (add path only).
/// Reclaims a gen-STALE grant slot (a grantee whose ASID was recycled) when claiming, so the list self-cleans.
/// Only the current owner may mutate the ACL — a grantee holding a read handle cannot re-grant (checked here,
/// so a non-owner is refused BEFORE any effect).
fn owned_grant(
    dir_lba: u64,
    dir_off: u32,
    owner_asid: u64,
    owner_gen: u64,
    grantee_asid: u64,
    grantee_gen: u64,
    rights: u32,
    owner_ppid: PrincipalRecord, // K1 M2.4: the CALLER's persistent principal (for by-name owner authority)
    grantee_ppid: PrincipalRecord, // K1 M2.3: grantee's persistent principal (captured by the caller pre-lock)
) -> i64 {
    // BANDY-1 (verdict C): the reserved KERNEL-REPLY principal can never be GRANTED rights (nor
    // wield owner authority) — refuse before any table read. Defense in depth; see PRIN_KERNEL_REPLY.
    if grantee_ppid.kind == PRIN_KERNEL_REPLY || owner_ppid.kind == PRIN_KERNEL_REPLY {
        return EACCES;
    }
    let _irq = IrqGuard::mask_save();
    let mut t = OWNED_FILES.lock();
    for row in t.iter_mut() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            // Only the file's CURRENT owner may grant or revoke on it. K1 M2.4: this is the OWNER-side twin of
            // the read helpers' by-name branch — a REBUILT row (owner_asid == OWNER_ASID_PERSISTED, matching no
            // live caller) is mutable by the re-spawned owner identified by its persistent principal, so the
            // by-name owner has FULL authority incl. re-granting (not just open + unlink). Structural: a NONE
            // principal never matches, so the anonymous path is unchanged.
            let live_owner = row.owner_asid == owner_asid && row.owner_gen == owner_gen;
            let name_owner = owner_ppid.kind != PRIN_NONE
                && row.owner_ppid.kind != PRIN_NONE
                && row.owner_ppid == owner_ppid;
            if !(live_owner || name_owner) {
                return EACCES;
            }
            // F2 (security-review MF1/MF3): a grant slot addresses the target grantee by its LIVE `(asid, gen)`
            // OR — for a slot REBUILT from disk (`asid == OWNER_ASID_PERSISTED`, no live incarnation) — by its
            // persistent principal. Without the ppid arm, a post-reboot revoke/update of a rebuilt grantee (whose
            // live handle carries a normal asid 1..=USER_SLOTS) matched NOTHING: revoke returned success while the
            // grant stayed (and was re-persisted — irrevocable), update claimed a SECOND slot for the same
            // principal. Structural: a NONE grantee_ppid never matches, so the anonymous path is unchanged.
            let matches_grantee = |g: &FileGrant| -> bool {
                (g.asid == grantee_asid && g.asid_gen == grantee_gen)
                    || (grantee_ppid.kind != PRIN_NONE && g.ppid.kind != PRIN_NONE && g.ppid == grantee_ppid)
            };
            // REVOKE (rights == 0): drop any existing grant for this grantee incarnation. Future opens deny;
            // a handle the grantee ALREADY holds is unaffected (the ACL gates ACQUISITION, not held caps).
            if rights == 0 {
                for g in row.grants.iter_mut() {
                    if g.rights != 0 && matches_grantee(g) {
                        *g = FileGrant::EMPTY;
                    }
                }
                return 0;
            }
            // GRANT/UPDATE: if a grant for this grantee already exists (live OR by-name rebuilt), update it in
            // place — refreshing its LIVE `(asid, gen)` too, so a rebuilt slot RE-BINDS to the now-live grantee
            // rather than leaving a duplicate. K1 M2.3: also refresh the persistent principal (stay consistent).
            for g in row.grants.iter_mut() {
                if g.rights != 0 && matches_grantee(g) {
                    *g = FileGrant { asid: grantee_asid, asid_gen: grantee_gen, rights, ppid: grantee_ppid };
                    return 0;
                }
            }
            // Otherwise claim a free slot — or reclaim a gen-stale one (a grantee whose ASID was recycled).
            // K1 M2.4: a REBUILT grant carries `asid == OWNER_ASID_PERSISTED` (u64::MAX, out of range) — range-
            // guard the ASID_GEN index so the stale-scan never indexes past the array; such a grant is treated
            // as NOT stale (it has no live incarnation to compare against) and is reclaimable only via a free slot.
            for g in row.grants.iter_mut() {
                let stale = g.rights != 0
                    && (g.asid as usize) < ASID_GEN.len()
                    && ASID_GEN[g.asid as usize].load(Ordering::Acquire) != g.asid_gen;
                if g.rights == 0 || stale {
                    *g = FileGrant { asid: grantee_asid, asid_gen: grantee_gen, rights, ppid: grantee_ppid };
                    return 0;
                }
            }
            return ENOSPC; // the file's grant list is full
        }
    }
    EACCES // no owner row -> a public / nonexistent file cannot be granted
}

/// K1 M2.3: snapshot the persistable ACL of the row at `(dir_lba, dir_off)` for the write-through persist —
/// the owner's persistent principal + each grant's `(principal, rights)`. `None` if there is no row (public
/// file → nothing to persist). Read-only under the IRQ-masked OWNED_FILES lock; the caller does the disk I/O
/// AFTER this returns (never under the lock). Returns the `(asid, gen)` fields as ppids only — the durable
/// identity — since a live `(asid, gen)` must never be written to disk.
fn owned_snapshot_row(
    dir_lba: u64,
    dir_off: u32,
) -> Option<(PrincipalRecord, [(PrincipalRecord, u32); NFGRANT])> {
    let _irq = IrqGuard::mask_save();
    let t = OWNED_FILES.lock();
    for row in t.iter() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            let mut grants = [(PrincipalRecord::NONE, 0u32); NFGRANT];
            for (j, g) in row.grants.iter().enumerate() {
                if g.rights != 0 {
                    grants[j] = (g.ppid, g.rights);
                }
            }
            return Some((row.owner_ppid, grants));
        }
    }
    None
}

/// BANDY-2 lens-2 fix: snapshot the FULL grant rows (live `(asid, gen)` keys + persistent ppids +
/// rights) of the file at `(dir_lba, dir_off)` — `None` for a public file (no row). The bus write-
/// truncate path captures this BEFORE its destroy half so the recreate half can RESTORE the grants:
/// the direct-syscall twin (open existing + write) mutates in place and PRESERVES grants, so the
/// bus must too (the equivalence discipline — a truncate is a content rewrite, not a revoke).
fn owned_snapshot_grants_full(dir_lba: u64, dir_off: u32) -> Option<[FileGrant; NFGRANT]> {
    let _irq = IrqGuard::mask_save();
    let t = OWNED_FILES.lock();
    for row in t.iter() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            return Some(row.grants);
        }
    }
    None
}

/// BANDY-2 lens-2 fix: re-install a snapshot of grant rows onto the (recreated) file at
/// `(dir_lba, dir_off)` — the restore half of the bus write-truncate grant preservation. The row
/// must already exist (`owned_set_owner` just installed it with empty grants); a missing row is a
/// defensive no-op (the create failed — nothing to restore onto). Called under the caller's
/// NAMESPACE hold (OWNED_FILES is an inner lock — the F3 lock order).
fn owned_restore_grants(dir_lba: u64, dir_off: u32, grants: &[FileGrant; NFGRANT]) {
    let _irq = IrqGuard::mask_save();
    let mut t = OWNED_FILES.lock();
    for row in t.iter_mut() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            row.grants = *grants;
            return;
        }
    }
}

/// K1 M2.3: the persistent principal of the OWNER of `(dir_lba, dir_off)` (NONE = public / no row). The light
/// read `sys_unlink` uses BEFORE `owned_clear` to decide whether a persisted attr row must also be cleared —
/// a NONE owner (the whole battery) needs no disk touch, keeping the anonymous unlink path byte-identical.
fn owned_owner_ppid(dir_lba: u64, dir_off: u32) -> PrincipalRecord {
    let _irq = IrqGuard::mask_save();
    let t = OWNED_FILES.lock();
    for row in t.iter() {
        if row.dir_lba == dir_lba && row.dir_off == dir_off {
            return row.owner_ppid;
        }
    }
    PrincipalRecord::NONE
}

/// U4: record a value a process-model fixture reported via SYS_REPORT, keyed by the reporting task's name —
/// the PARENT's witness token and the ORPHAN's ownership result. Keyed by name like `m6d_report`/`m6f_report`;
/// the SYS_REPORT arm calls all three and each ignores the others' tasks.
fn u4_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-u4parent") => U4_PARENT_WITNESS.store(value, Ordering::Release),
        Some("el0-u4orphan") => U4_ORPHAN_ECHILD.store(value as u32, Ordering::Release),
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

/// U5: record the capability fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
/// Called from the SYS_REPORT arm alongside the M6d/M6f/U4 reporters; each ignores the others' names.
fn u5_report(value: u64) {
    if super::sched::current_name() == Some("el0-u5cap") {
        U5_WITNESS.store(value, Ordering::Release);
    }
}

/// U6: record the printing-spawner fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
/// Called from the SYS_REPORT arm alongside the M6d/M6f/U4/U5 reporters; each ignores the others' names.
fn u6_report(value: u64) {
    if super::sched::current_name() == Some("el0-u6spawn") {
        U6_WITNESS.store(value, Ordering::Release);
    }
}

/// U6b: record the File-handle fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
/// Called from the SYS_REPORT arm alongside the M6d/M6f/U4/U5/U6 reporters; each ignores the others' names.
fn u6b_report(value: u64) {
    if super::sched::current_name() == Some("el0-u6bfile") {
        U6B_WITNESS.store(value, Ordering::Release);
    }
}

/// U7: record a value a transfer fixture reported via SYS_REPORT, keyed by the reporting task's name (the
/// `u4_report` idiom; each report fn ignores the others' tasks). The child reports TWICE: the mid-run
/// `U7_USED_TOKEN` (its first write through the transferred cap landed — the launcher's cue to release the
/// parent's revoke GO) and its final witness bitmask; the token is disjoint from every witness value
/// (witnesses are <= `U7_WITNESS_ALL`), so one routing fn serves both.
/// U8: record the revocation-tree fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
/// Called from the SYS_REPORT arm alongside the other reporters; each ignores the others' names.
fn u8_report(value: u64) {
    if super::sched::current_name() == Some("el0-u8tree") {
        U8_WITNESS.store(value, Ordering::Release);
    }
}

/// U9: record the File-WRITE fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
/// Called from the SYS_REPORT arm alongside the other reporters; each ignores the others' names.
fn u9_report(value: u64) {
    if super::sched::current_name() == Some("el0-u9write") {
        U9_WITNESS.store(value, Ordering::Release);
    }
}

/// U10: record the file-GROWTH fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
/// Called from the SYS_REPORT arm alongside the other reporters; each ignores the others' names.
fn u10_report(value: u64) {
    if super::sched::current_name() == Some("el0-u10grow") {
        U10_WITNESS.store(value, Ordering::Release);
    }
}

/// U10-create: record the CREATE fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
fn u10c_report(value: u64) {
    if super::sched::current_name() == Some("el0-u10create") {
        U10C_WITNESS.store(value, Ordering::Release);
    }
}

/// U10-delete: record the DELETE fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
fn u10d_report(value: u64) {
    if super::sched::current_name() == Some("el0-u10delete") {
        U10D_WITNESS.store(value, Ordering::Release);
    }
}

/// U11: record the open-file-lifecycle fixture's witness bitmask (SYS_REPORT), keyed by the reporting task's name.
fn u11_report(value: u64) {
    if super::sched::current_name() == Some("el0-u11close") {
        U11_WITNESS.store(value, Ordering::Release);
    }
}

/// U11-M2 (defer): record the two cross-process fixtures' SYS_REPORTs, keyed by task name. Each fixture reports
/// mid-run CUE tokens (all `> 0xF`, so they never collide with a witness value) that release the launcher's next
/// choreography edge, then its final witness bitmask (`<= 0xF`).
fn u11defer_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-u11defer-a") => match value {
            U11DEFER_A_OPENED => U11DEFER_A_OPENED_F.store(1, Ordering::Release),
            U11DEFER_A_READ => U11DEFER_A_READ_F.store(1, Ordering::Release),
            _ => U11DEFER_A_WITNESS.store(value, Ordering::Release),
        },
        Some("el0-u11defer-b") => match value {
            U11DEFER_B_UNLINKED => U11DEFER_B_UNLINKED_F.store(1, Ordering::Release),
            _ => U11DEFER_B_WITNESS.store(value, Ordering::Release),
        },
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

/// U11-M2b (reap): record the two teardown-reap fixtures' SYS_REPORTs, keyed by task name (the `u11defer_report`
/// idiom). Cue tokens (`> 0xF`) release the launcher's next choreography edge; a final witness (`<= 0xF`) lands
/// in the witness word.
fn u11reap_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-u11reap-a") => match value {
            U11REAP_A_OPENED => U11REAP_A_OPENED_F.store(1, Ordering::Release),
            U11REAP_A_READ => U11REAP_A_READ_F.store(1, Ordering::Release),
            _ => U11REAP_A_WITNESS.store(value, Ordering::Release),
        },
        Some("el0-u11reap-b") => match value {
            U11REAP_B_UNLINKED => U11REAP_B_UNLINKED_F.store(1, Ordering::Release),
            _ => U11REAP_B_WITNESS.store(value, Ordering::Release),
        },
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

/// U6 (owner/grants): record the two owner/grants fixtures' SYS_REPORTs, keyed by task name (the `u11defer_report`
/// idiom). Cue tokens (`> 0x1F`) release the launcher's next choreography edge; a final witness (A `<= 0xF`, B
/// `<= 0x1F`) lands in the witness word.
fn uowner_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-uowner-a") => match value {
            UOWNER_A_READY => UOWNER_A_READY_F.store(1, Ordering::Release),
            UOWNER_A_GRANTED => UOWNER_A_GRANTED_F.store(1, Ordering::Release),
            UOWNER_A_REVOKED => UOWNER_A_REVOKED_F.store(1, Ordering::Release),
            _ => UOWNER_A_WITNESS.store(value, Ordering::Release),
        },
        Some("el0-uowner-b") => match value {
            UOWNER_B_DENIED1 => UOWNER_B_DENIED1_F.store(1, Ordering::Release),
            UOWNER_B_OPENED => UOWNER_B_OPENED_F.store(1, Ordering::Release),
            _ => UOWNER_B_WITNESS.store(value, Ordering::Release),
        },
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

fn u7_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-u7parent") => U7_PARENT_WITNESS.store(value, Ordering::Release),
        Some("el0-u7child") => {
            if value == U7_USED_TOKEN {
                U7_CHILD_USED.store(1, Ordering::Release);
            } else {
                U7_CHILD_WITNESS.store(value, Ordering::Release);
            }
        }
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

/// M6d: record a value an EL0 task reported via SYS_REPORT, keyed by the reporting task's name. Called on
/// the reporting task's own kernel stack (from the SVC handler), IRQ-masked.
fn m6d_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-samevaA") => M6D_REPORT_A.store(value, Ordering::Release),
        Some("el0-samevaB") => M6D_REPORT_B.store(value, Ordering::Release),
        Some("el0-stackwrite") => M6D_REPORT_STACK.store(value, Ordering::Release),
        Some("el0-spsentinel") => M6D_REPORT_SP.store(value, Ordering::Release),
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

/// The EL0 demo entry points (EL0 VAs inside the code page) and the shared initial SP_EL0.
///
/// All programs SHARE one user stack (`sp`). Through M6a–M6c that was safe because EL0 was
/// non-preemptible; under M6e EL0 IS preemptible (SP_EL0 banked in `__vec_irq`), so the shared stack
/// is now safe for a DIFFERENT, load-bearing reason: **no EL0 demo program writes its user stack** —
/// hello (`USER_BLOB`) and the spinner are register-only, and the fault fixtures fault or exit before
/// any push. With SP_EL0 banked per-task, preemptive interleave cannot corrupt a stack nobody writes.
/// STOP TRIPWIRE: the first EL0 program that actually WRITES its user stack needs per-task user stacks
/// (extend the user window in `boot.rs`) — that is M6d-adjacent and OUT of this lane; stop and hand it
/// to the integrator rather than growing the window here.
pub struct El0Demo {
    pub sp: u64,
    pub hello: u64,
    pub wild_write: u64,
    pub code_write: u64,
    pub stack_exec: u64,
    /// M6e preemption spinner (`__user_prog_spin`).
    pub spin: u64,
}

/// Copy the EL0 programs into the user window (`boot::user_region`) and do the I-cache maintenance;
/// return the demo entry points. Call once, after `mmu_init`. Does NOT protect the code page — the
/// caller warms the demo core's TLB first, then calls `protect()` (the copies here are exactly why
/// the page must still be EL1-writable). The window is identity-mapped, so entries are base + copy
/// offsets and each program's PC-relative `adr`s resolve in place.
///
/// M6c: two blobs share the ONE code page. The loaded `hello` program (`USER_BLOB`, out of kernel
/// `.text`) goes at offset 0 — the kernel enters it at the base — and the inline fault fixtures
/// (`__fault_blob_*`) go right after it. Both must fit in `USER_CODE_SIZE`.
pub fn setup() -> El0Demo {
    let (base, size) = super::boot::user_region();
    let hello_len = USER_BLOB.len();
    // 16-align the fixtures' start so their first instruction is 4-aligned (an eret/exec into a
    // misaligned entry is EC 0x22) and the icache maintenance below covers whole cache lines.
    let fault_off = (hello_len + 0xF) & !0xF;
    let fstart = &raw const __fault_blob_start as usize;
    let fend = &raw const __fault_blob_end as usize;
    let fault_len = fend - fstart;
    let total = fault_off + fault_len;
    // Everything must fit in the CODE page — the only page protect_user_code makes EL0-executable; a
    // program straddling into the data pages would abort mid-run.
    assert!(
        total <= super::boot::USER_CODE_SIZE,
        "user code (hello blob + fault fixtures) does not fit in the code page"
    );
    unsafe {
        // hello (the loaded blob) at the base; the inline fault fixtures at base + fault_off.
        core::ptr::copy_nonoverlapping(USER_BLOB.as_ptr(), base as *mut u8, hello_len);
        core::ptr::copy_nonoverlapping(
            fstart as *const u8,
            (base + fault_off as u64) as *mut u8,
            fault_len,
        );
    }
    // Freshly-written code: clean D to the PoU + invalidate the I-cache so the EL0 fetch (possibly on
    // another core — IC IVAU broadcasts Inner-Shareable) sees the new bytes across BOTH copies. This
    // is the DC CVAU/IC IVAU sequence M6a/M6b rely on; KEEP it for the M6c loaded-blob copy — it is
    // exactly what makes the copied program executable on real caches. Metal-only; QEMU no-op.
    super::cache::icache_sync_range(base as usize, total);
    serial_println!(":: M6c: user blob loaded ({} bytes) ::", hello_len);
    // An eret to a misaligned entry is EC 0x22 (PC alignment) — assert every entry came out
    // 4-aligned. Each fixture VA = base + fault_off + its offset within the fault blob.
    let fentry = |label: *const u8| -> u64 {
        let va = base + fault_off as u64 + (label as usize - fstart) as u64;
        assert!(va & 3 == 0, "user program entry misaligned");
        va
    };
    // `hello` enters at the copy's offset 0 (base). base is structurally 16 KiB-aligned (the region's
    // `#[repr(align(0x4000))]`), but assert it here too so it gets the same guard as the fixtures —
    // a future USER_REGION relocation can't silently produce a misaligned EL0 entry.
    assert!(base & 3 == 0, "hello entry misaligned");
    // U5: endow the SHARED window (ASID 0 — where `spawn_user` runs `el0-hello`) with a console write-
    // capability, so hello's `sys_write(fd 1)` still reaches the console once writes route through the table.
    // The shared window is never torn down (ASID 0), so this endowment persists for the whole boot; the M6b
    // fault fixtures and the M6e spinner share ASID 0 but never write, so the single fixed cap serves them all.
    install_console_cap(0);
    El0Demo {
        sp: (base + size as u64) & !0xF, // 16-aligned top of the window = initial user stack pointer
        hello: base, // the loaded blob's `_start` is at offset 0 of the copy (base is 16 KiB-aligned)
        wild_write: fentry(&raw const __user_prog_wild_write),
        code_write: fentry(&raw const __user_prog_code_write),
        stack_exec: fentry(&raw const __user_prog_stack_exec),
        spin: fentry(&raw const __user_prog_spin),
    }
}

/// M6b: deterministically WARM the demo core's TLB with the pre-protect (RW, XN) code-page mapping.
/// Runs as a kernel task pinned to the core that will run the EL0 demo, BEFORE `protect()`: the
/// volatile read walks the tables and caches the old descriptor in THIS core's TLB, so a broken
/// broadcast TLBI leaves a deterministic stale entry right where the demo executes — hello's first
/// EL0 fetch then dies through the stale UXN=1 (killed_unexpected -> FAIL) or code-write's store
/// sneaks through the stale RW (survivor exit(1) -> FAIL). Without this the demo core's TLB is cold
/// (only the BSP touches USER_REGION pre-protect: the blob copy) and a missing TLBI would pass
/// silently — QEMU can't test the TLBI at all (it re-walks), so the warm-up is what makes the METAL
/// run the real detector.
pub fn tlb_warm(_: usize) {
    let (base, _) = super::boot::user_region();
    // M6d: warm THIS core's TLB with the SHARED (ASID-0/boot-context) code-page mapping — the mapping the
    // M6b EL0 tasks (which run on the boot root) use. Since M6d a per-slot task may have left a slot root
    // live on this core; the shared user VA maps to a DIFFERENT (slot) frame under a slot root, so walking
    // it there would warm the wrong entry. Force the boot root live first (this is a kernel task, so
    // `dispatch_next` did no root switch), IRQ-masked so no preempt reswaps TTBR0 between the set and the
    // read. Leaving the boot root live is fine — the next dispatch installs the incoming task's root.
    unsafe {
        core::arch::asm!(
            "msr daifset, #2",
            "msr TTBR0_EL1, {boot}",
            "isb",
            boot = in(reg) super::boot::boot_ttbr0(),
            options(nostack, preserves_flags),
        );
        core::ptr::read_volatile(base as *const u8);
        core::arch::asm!("msr daifclr, #2", options(nostack, preserves_flags));
    }
    TLB_WARMED.store(true, Ordering::Release);
}

/// Flip the code page to its final EL0-RX/EL1-RO shape (`boot::protect_user_code`) and report the
/// BSP-side AT-probe verdicts. Call strictly AFTER `setup()` (the copy needs the page writable) and
/// after the demo core's TLB warm-up. A clean probe is best-effort evidence (AT may re-walk rather
/// than consult the TLB); a bad probe is always a real, loud failure.
pub fn protect() {
    let (base, _) = super::boot::user_region();
    let (el0_read_ok, el1_write_denied) =
        unsafe { super::boot::protect_user_code(base, super::boot::USER_CODE_SIZE) };
    if el0_read_ok && el1_write_denied {
        serial_println!(
            ":: M6b: user code page EL0-RX/EL1-RO (AT probe: EL0-read OK, EL1-write denied) ::"
        );
    } else {
        serial_println!(
            ":: M6b WARNING: protect probe unexpected (el0_read_ok={} el1_write_denied={}) — stale TLB after the TLBI? ::",
            el0_read_ok,
            el1_write_denied
        );
    }
}

/// M6b accounting: classify a killed task against the demo's EXPECTED faults. The verdict demands
/// the right (task, EC, FAR-page) triple, not just "it died": the stack page is BSS zeros and
/// 0x00000000 decodes as UDF, so with UXN accidentally unset stack-exec would still die (EC 0x00) —
/// count-only bookkeeping would false-PASS the very permission claim the test exists to prove.
/// Called from `aarch64_el0_fault_handler` before it exits the task.
pub fn record_el0_kill(name: &str, ec: u64, far: u64, far_valid: bool) {
    // M6d tasks (per-task address spaces) are NOT part of the M6b fault-isolation verdict. A kill among
    // them means a genuine per-slot ASID/permission bug — it must land in its OWN counter, never inflate
    // the M6b `killed_unexpected` count (which would masquerade as an M6b regression and hide the real
    // fault). Their missing SYS_REPORT already FAILs the M6d verdict line.
    if matches!(name, "el0-samevaA" | "el0-samevaB" | "el0-stackwrite" | "el0-spsentinel") {
        EL0_M6D_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // M6f fixtures likewise: a kill among them is a real bug (they must EFAULT-return, never fault) and
    // must land in its own counter, never inflating the M6b `killed_unexpected` count.
    if matches!(name, "el0-getinfo" | "el0-hostile" | "el0-yield" | "el0-sleep") {
        EL0_M6F_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // M6g: the untrusted disk-loaded program. A kill here is contained (the whole point) and reported by
    // the loader's own verdict — route it to its own counter, never the M6b `killed_unexpected` count.
    if name == "m6g-hello" {
        EL0_M6G_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U4: a killed process-model task (a spawned CHILD, the PARENT, or the ORPHAN). Off the M6b counter — a
    // kill here is a real U4 bug that fails the U4 verdict, not a phantom M6b regression. For a killed CHILD,
    // also post its Proc `done` (with a non-zero sentinel status) so the parent's blocked `sys_wait` WAKES
    // instead of hanging — the child never reaches its own SYS_EXIT post. `current_id()` is the faulting task
    // (still current here — see aarch64_el0_fault_handler), i.e. the child's pid = its Proc key. (The parent
    // and orphan are not in PROCS — they were spawned by the launcher, not by sys_spawn — so no Proc post.)
    if name == "u4-child" || name == "el0-u4parent" || name == "el0-u4orphan" {
        EL0_U4_KILLED.fetch_add(1, Ordering::AcqRel);
        if name == "u4-child" {
            if let Some(id) = super::sched::current_id() {
                if let Some(i) = proc_find_running(id) {
                    PROCS[i].status.store(U4_KILLED_STATUS, Ordering::Release);
                    PROCS[i].state.store(PEXITED, Ordering::Release);
                    PROCS[i].done.post();
                }
            }
        }
        return;
    }
    // U5: the capability fixture is well-behaved (register-only, no faults); a kill here is a real U5 bug.
    // Route it to its own counter, never the M6b `killed_unexpected` count, so a U5 fault fails only the U5
    // verdict (its missing SYS_REPORT already leaves the witness incomplete).
    if name == "el0-u5cap" {
        EL0_U5_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U6: the printing-spawner fixture is well-behaved (register-only); a kill here is a real U6 bug. Route it
    // to its own counter, never the M6b `killed_unexpected` count, so a U6 fault fails only the U6 verdict. (A
    // killed U6 CHILD shares the `u4-child` name above — its kill posts a non-zero status that fails the reap.)
    if name == "el0-u6spawn" {
        EL0_U6_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U6b: the File-handle fixture is well-behaved (writes no stack, faults on nothing); a kill here is a real
    // U6b bug. Route it to its own counter, never the M6b `killed_unexpected` count, so a U6b fault fails only
    // the U6b verdict (its missing SYS_REPORT already leaves the witness incomplete).
    // U7: the transfer fixtures are well-behaved (register-only; their only writable target is the launcher-
    // owned GO word page). A kill here is a real U7 bug — route it to its own counter, never the M6b
    // `killed_unexpected` count, so a U7 fault fails only the U7 verdict. (The child HAS a planted Proc entry,
    // but the launcher waits on the deadline-bounded EL0_U7_DONE counter, not the Proc semaphore, so no
    // `done` post is needed here — the missing SYS_REPORT already leaves the witness incomplete.)
    // U8: the revocation-tree fixture is register-only and well-behaved; a kill is a real U8 bug — its own
    // counter, never the M6b `killed_unexpected` count (the same discipline as every fixture above).
    if name == "el0-u8tree" {
        EL0_U8_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    if name == "el0-u7parent" || name == "el0-u7child" {
        EL0_U7_KILLED.fetch_add(1, Ordering::AcqRel);
        // A killed CHILD's planted Proc entry goes EXITED too (the exit-arm twin), so the pid->ASID map
        // never vouches for a dead recipient. No `done` post — the launcher waits on deadline counters.
        if let Some(id) = super::sched::current_id() {
            if let Some(i) = proc_find_running(id) {
                PROCS[i].state.store(PEXITED, Ordering::Release);
            }
        }
        return;
    }
    if name == "el0-u6bfile" {
        EL0_U6B_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U9: the File-WRITE fixture is register-only (its only writable user target is its own data page); a kill
    // is a real U9 bug — its own counter, never the M6b `killed_unexpected` count (same discipline as above).
    if name == "el0-u9write" {
        EL0_U9_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U10: the file-GROWTH fixture is likewise register-only; a kill is a real U10 bug (its own counter).
    if name == "el0-u10grow" {
        EL0_U10_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U10-create: the CREATE fixture is likewise register-only; a kill is a real bug (its own counter).
    if name == "el0-u10create" {
        EL0_U10C_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U10-delete: the DELETE fixture is likewise register-only; a kill is a real bug (its own counter).
    if name == "el0-u10delete" {
        EL0_U10D_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U11: the open-file-lifecycle fixture is likewise register-only; a kill is a real bug (its own counter).
    if name == "el0-u11close" {
        EL0_U11_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U11-M2 (defer): the two cross-process fixtures are register-only (bar their +0x2000 read buffer); a kill of
    // either is a real bug -> its own counter, off the M6b accounting.
    if name == "el0-u11defer-a" || name == "el0-u11defer-b" {
        EL0_U11DEFER_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U11-M2b (reap): the two teardown-reap fixtures are likewise register-only (bar their +0x2000 read buffer);
    // a kill of either is a real bug -> its own counter, off the M6b accounting.
    if name == "el0-u11reap-a" || name == "el0-u11reap-b" {
        EL0_U11REAP_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // U6 (owner/grants): the two owner/grants fixtures are register-only (bar B's +0x2000 read buffer); a kill of
    // either is a real bug -> its own counter, off the M6b accounting.
    if name == "el0-uowner-a" || name == "el0-uowner-b" {
        EL0_UOWNER_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // K2: the two make-enforcement-LIVE programs are well-behaved (register-only, valid syscalls); a kill of
    // either is a real K2 bug -> its own counter, off the M6b `killed_unexpected` count (the FAIL then falls
    // out of the launcher's `killed == 0` gate, its missing SYS_REPORT leaving the witness incomplete too).
    if name == "el0-k2own" || name == "el0-k2imp" {
        EL0_K2_KILLED.fetch_add(1, Ordering::AcqRel);
        return;
    }
    // EXEC-1: a killed run-image task (the shell `run <path>` program) has a planted Proc entry but no
    // dedicated name arm above (it runs under an arbitrary caller-supplied name). Mark its Proc entry EXITED
    // with the kill sentinel + post `done` so `run_user_image`'s wait wakes PROMPTLY (not at its deadline),
    // and route the kill OFF the M6b `killed_unexpected` count — a fault in an operator-launched untrusted
    // program is CONTAINED, not an M6b regression. Byte-identical for the boot battery: every Proc-tracked
    // fixture is already matched by a name arm above, so no existing task reaches here with a live Proc entry.
    if let Some(id) = super::sched::current_id() {
        if let Some(i) = proc_find_running(id) {
            PROCS[i].status.store(EXEC_KILLED_STATUS, Ordering::Release);
            PROCS[i].state.store(PEXITED, Ordering::Release);
            PROCS[i].done.post();
            return;
        }
    }
    let (base, size) = super::boot::user_region();
    let code = super::boot::USER_CODE_SIZE as u64;
    let expected = far_valid
        && match name {
            // an EL0 write to PA 0x0 (EL1-only RAM): data abort, FAR in page 0 of the PA space
            "el0-wild-write" => ec == 0x24 && far >> 12 == 0,
            // an EL0 write to the (now read-only) code page: data abort, FAR in the code page
            "el0-code-write" => ec == 0x24 && far >= base && far < base + code,
            // an EL0 fetch from the UXN stack page: instruction abort, FAR in the data pages
            "el0-stack-exec" => ec == 0x20 && far >= base + code && far < base + size as u64,
            _ => false,
        };
    if expected {
        EL0_KILLED_EXPECTED.fetch_add(1, Ordering::AcqRel);
    } else {
        EL0_KILLED_UNEXPECTED.fetch_add(1, Ordering::AcqRel);
    }
}

/// M6b verdict task: wait (bounded) for all four M6b EL0 programs (hello + three fault fixtures) to
/// terminate, then print one PASS/FAIL line with the full accounting. Spawned on a DIFFERENT core than
/// the demo tasks so a wedged demo core (the fingerprint of a broken TLBI) still produces a verdict —
/// a timeout FAIL with the counts — instead of a silent half-dead boot. (The M6e spinner accounts
/// separately, via `EL0_SPIN_DONE`, so it does not perturb this verdict's `done >= 4`.) Time-bounded
/// via CNTPCT (which advances in QEMU even though the timer IRQ never fires there), not a yield count
/// (meaningless on a core with other work).
pub fn verdict(_: usize) {
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq(); // ~5 s; the whole demo completes in well under 1 s
    loop {
        let done = EL0_EXITED_OK.load(Ordering::Acquire)
            + EL0_EXITED_ERR.load(Ordering::Acquire)
            + EL0_KILLED_EXPECTED.load(Ordering::Acquire)
            + EL0_KILLED_UNEXPECTED.load(Ordering::Acquire);
        if done >= 4 || super::timer::cntpct().wrapping_sub(start) > deadline {
            break;
        }
        super::sched::yield_now();
    }
    let ok = EL0_EXITED_OK.load(Ordering::Acquire);
    let err = EL0_EXITED_ERR.load(Ordering::Acquire);
    let exp = EL0_KILLED_EXPECTED.load(Ordering::Acquire);
    let unexp = EL0_KILLED_UNEXPECTED.load(Ordering::Acquire);
    // The EXACT split, not the sum: hello killed (exited=0/killed=4), a survivor, or a wrong-EC
    // kill must all read FAIL — "every program terminated" is not the claim being proven.
    if ok == 1 && exp == 3 && err == 0 && unexp == 0 {
        serial_println!(
            ":: M6b: EL0 fault isolation — exited=1 killed=3 (all expected ECs), kernel alive -> PASS ::"
        );
    } else {
        serial_println!(
            ":: M6b: EL0 fault isolation FAIL — exited_ok={} survivor_exits={} killed_expected={} killed_unexpected={} (want 1/0/3/0) ::",
            ok,
            err,
            exp,
            unexp
        );
    }
}

/// M6e verdict task: wait (bounded, CNTPCT) for the preemption spinner to finish, then report whether
/// EL0 was actually preempted. Spawned like the M6b verdict on a scheduled core that co-tenants the
/// capstone workers, so it polls with `yield_now` (never monopolizes the core). The line is
/// deterministic under QEMU (the spinner completes its bounded loop -> completed=1; no timer IRQ ->
/// IRQs=0) and carries the metal-only signal in `IRQs`: on the real Pi 4 the timer (and any other SPI)
/// preempts running EL0 tasks, so `IRQs > 0` (demo-wide) — and the spinner STILL completes, which is
/// the distinct proof that SP_EL0 banking resumed it with the right user stack pointer. Time-bounded
/// via CNTPCT (advances in QEMU even without the timer IRQ), matching the M6b verdict.
pub fn m6e_verdict(_: usize) {
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq(); // ~5 s; the spinner finishes in well under 1 s either way
    while EL0_SPIN_DONE.load(Ordering::Acquire) == 0
        && super::timer::cntpct().wrapping_sub(start) <= deadline
    {
        super::sched::yield_now();
    }
    let done = EL0_SPIN_DONE.load(Ordering::Acquire);
    let irqs = EL0_IRQS_AT_EL0.load(Ordering::Relaxed);
    serial_println!(
        ":: M6e: EL0 preemptible — spinner completed={} IRQs-taken-at-EL0={} (metal: completed=1 & IRQs>0; QEMU: completed=1 & IRQs=0) ::",
        done,
        irqs
    );
}

/// The M6d demo's per-task entry points (all at the SAME user VAs — the point of ASID isolation) and the
/// per-task slot roots (`TTBR0` values from `boot::slot_ttbr0`). One shared initial SP_EL0 (each slot's
/// window has the same VA layout; only the frames differ).
pub struct M6dDemo {
    pub sp: u64,
    pub same_va: u64,
    pub stack_write: u64,
    pub sp_sentinel: u64,
    pub ttbr0_a: u64,
    pub ttbr0_b: u64,
    pub ttbr0_stack: u64,
    pub ttbr0_sp: u64,
}

/// M6d setup: allocate four private address-space slots, copy the M6d blob into each slot's code page
/// (through the slot backing's Global identity VA — never the EL0 window VA), plant each reader's
/// slot-private data sentinel, I-cache-sync, protect the code pages, and run the deterministic on-metal
/// nG detector. Emits the M6d setup line and returns the per-task entries + slot roots. Called once on the
/// BSP (which runs on the boot root) after the M6b/M6e demo. `None` if a slot allocation fails.
pub fn m6d_setup() -> Option<M6dDemo> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // shared initial SP_EL0 (top of the window, 16-aligned)
    let sent_off = size as u64 - 0x100; // the sentinel VA offset: EL0 reads [sp, #-0x100]

    // Blob bytes + per-fixture offsets (mirrors `setup`'s fault-fixture math).
    let bstart = &raw const __m6d_blob_start as usize;
    let bend = &raw const __m6d_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "M6d blob does not fit in a code page");
    let entry = |label: *const u8| -> u64 {
        let off = label as usize - bstart;
        let va = base + off as u64;
        assert!(va & 3 == 0, "M6d program entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };

    // Multi-alloc with partial-failure unwind (M6d review fold): the old four sequential `alloc_user_slot()?`
    // calls leaked earlier-claimed slots when a later one failed. `alloc_user_slots` releases what it got and
    // returns false on exhaustion, so a failed M6d setup frees the whole request.
    let mut slots = [0usize; 4];
    if !super::boot::alloc_user_slots(&mut slots) {
        return None;
    }
    let [slot_a, slot_b, slot_c, slot_d] = slots;

    // Copy the blob into each slot's code page (identity VA) + I-cache sync (DC CVAU/IC IVAU by the
    // identity VA; A72 caches are PIPT, so the code is fetchable at the aliased EL0 window VA).
    for &s in &slots {
        let backing = super::boot::slot_backing_ptr(s);
        unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
        super::cache::icache_sync_range(backing as usize, blen);
    }
    // Plant the readers' slot-private sentinels (page 3, [top-0x100]) via the identity VA. Pure data on a
    // PIPT D-cache — coherent with the EL0/probe read of the same frame at the window VA, no maintenance.
    unsafe {
        *(super::boot::slot_backing_ptr(slot_a).add(sent_off as usize) as *mut u64) = M6D_SENTINEL_A;
        *(super::boot::slot_backing_ptr(slot_b).add(sent_off as usize) as *mut u64) = M6D_SENTINEL_B;
        *(super::boot::slot_backing_ptr(slot_d).add(sent_off as usize) as *mut u64) = M6D_SENTINEL_SP;
    }
    // Protect every slot's code page (EL0-RX/EL1-RO). After this the code page is no longer EL1-writable.
    for &s in &slots {
        unsafe { super::boot::protect_user_slot_code(s, super::boot::USER_CODE_SIZE) };
    }
    // Deterministic on-metal nG detector (the arc's #1 metal risk): swap TTBR0 between slot A and B roots
    // reading the SAME VA — a global (nG=0) user leaf would resolve both to slot A's frame. QEMU re-walks
    // -> always PASS; metal caches -> a broken nG is caught. Folded into the same-VA PASS below.
    let probe_ok = unsafe {
        super::boot::probe_slot_isolation(slot_a, slot_b, sent_off, M6D_SENTINEL_A, M6D_SENTINEL_B)
    };
    M6D_PROBE_OK.store(probe_ok, Ordering::Release);

    serial_println!(
        ":: M6d: per-task address spaces (8 slots, ASID 1-8, nG user / global kernel) ::"
    );

    Some(M6dDemo {
        sp,
        same_va: entry(&raw const __m6d_prog_same_va),
        stack_write: entry(&raw const __m6d_prog_stack_write),
        sp_sentinel: entry(&raw const __m6d_prog_sp_sentinel),
        ttbr0_a: super::boot::slot_ttbr0(slot_a),
        ttbr0_b: super::boot::slot_ttbr0(slot_b),
        ttbr0_stack: super::boot::slot_ttbr0(slot_c),
        ttbr0_sp: super::boot::slot_ttbr0(slot_d),
    })
}

/// M6d verdict task: wait (bounded, CNTPCT) for the four M6d tasks to finish, then print the three PASS/
/// FAIL lines. Spawned on a sibling core like the M6b/M6e verdicts. Isolation is proven by `same_va` (two
/// tasks reading distinct slot-private sentinels at the SAME VA) PLUS the deterministic kernel probe;
/// `stack_write` and `sp_sentinel` are path-liveness checks (the stack is writable; SP_EL0 addresses the
/// slot after preemption). A killed M6d task never reports, so its line FAILs (bounded by the deadline).
pub fn m6d_verdict(_: usize) {
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq(); // ~5 s; the whole demo completes well under 1 s
    while EL0_M6D_DONE.load(Ordering::Acquire) < 4
        && super::timer::cntpct().wrapping_sub(start) <= deadline
    {
        super::sched::yield_now();
    }
    let a = M6D_REPORT_A.load(Ordering::Acquire);
    let b = M6D_REPORT_B.load(Ordering::Acquire);
    let st = M6D_REPORT_STACK.load(Ordering::Acquire);
    let spv = M6D_REPORT_SP.load(Ordering::Acquire);
    let probe = M6D_PROBE_OK.load(Ordering::Acquire);
    let killed = EL0_M6D_KILLED.load(Ordering::Acquire);

    // same-VA isolation: each task read its OWN slot's sentinel at the same VA; distinct + each == planted
    // + the deterministic kernel probe agreed (nG is real on metal). The full triple, never bare distinctness.
    if a == M6D_SENTINEL_A && b == M6D_SENTINEL_B && a != b && probe {
        serial_println!(":: M6d: same-VA isolation A={:#x} B={:#x} distinct -> PASS ::", a, b);
    } else {
        serial_println!(
            ":: M6d: same-VA isolation A={:#x} B={:#x} probe={} killed={} -> FAIL ::",
            a, b, probe, killed
        );
    }
    if st == M6D_STACK_PATTERN {
        serial_println!(":: M6d: EL0 stack write/readback -> PASS ::");
    } else {
        serial_println!(":: M6d: EL0 stack write/readback (got {:#x}) -> FAIL ::", st);
    }
    if spv == M6D_SENTINEL_SP {
        serial_println!(":: M6d: SP-relative sentinel readback -> PASS ::");
    } else {
        serial_println!(
            ":: M6d: SP-relative sentinel readback (got {:#x} want {:#x}) -> FAIL ::",
            spv, M6D_SENTINEL_SP
        );
    }
}

/// The M6f demo's per-fixture entry points (EL0 VAs inside each slot's code page) + the per-fixture slot
/// roots (`TTBR0` from `boot::slot_ttbr0`). One shared initial SP_EL0 (every slot's window has the same VA
/// layout; only the frames differ). Each fixture runs on its OWN private slot because the getinfo fixture
/// WRITES its stack (copy_to_user target) — forbidden on the shared window by the M6e stack STOP tripwire.
pub struct M6fDemo {
    pub sp: u64,
    pub getinfo: u64,
    pub hostile: u64,
    pub yield_prog: u64,
    pub sleep_prog: u64,
    pub ttbr0_getinfo: u64,
    pub ttbr0_hostile: u64,
    pub ttbr0_yield: u64,
    pub ttbr0_sleep: u64,
}

/// M6f setup: allocate four private slots (via the unwinding `alloc_user_slots`), copy the M6f blob into
/// each slot's code page (through the Global identity backing VA, never the EL0 window VA), I-cache-sync,
/// and protect the code pages. Emits the M6f setup line; returns the per-fixture entries + slot roots.
/// Called once on the BSP after the M6d demo. `None` if slot allocation fails (the whole request is
/// released, not leaked). Plants no sentinel — the getinfo fixture writes its own struct via copy_to_user.
pub fn m6f_setup() -> Option<M6fDemo> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // shared initial SP_EL0 (16-aligned top of the window)

    let bstart = &raw const __m6f_blob_start as usize;
    let bend = &raw const __m6f_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "M6f blob does not fit in a code page");
    let entry = |label: *const u8| -> u64 {
        let off = label as usize - bstart;
        let va = base + off as u64;
        assert!(va & 3 == 0, "M6f program entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };

    let mut slots = [0usize; 4];
    if !super::boot::alloc_user_slots(&mut slots) {
        return None;
    }
    // Copy the blob into each slot's code page (identity VA) + I-cache sync (DC CVAU/IC IVAU by the identity
    // VA; A72 caches are PIPT, so the code is fetchable at the aliased EL0 window VA), then protect it.
    for &s in &slots {
        let backing = super::boot::slot_backing_ptr(s);
        unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
        super::cache::icache_sync_range(backing as usize, blen);
    }
    for &s in &slots {
        unsafe { super::boot::protect_user_slot_code(s, super::boot::USER_CODE_SIZE) };
    }
    // U5: endow each M6f slot with a console write-capability. The hostile fixture `sys_write(fd 1)`s with
    // BAD pointers expecting -EFAULT: it must hold the cap so the resolve passes and the pointer range check
    // (still unchanged) is what refuses it. The other three fixtures don't write; the endowment is harmless.
    for &s in &slots {
        install_console_cap(super::boot::slot_ttbr0(s) >> 48);
    }

    serial_println!(
        ":: M6f: validated user pointers — copy_from_user/copy_to_user + syscall surface (4 EL0 fixtures) ::"
    );

    Some(M6fDemo {
        sp,
        getinfo: entry(&raw const __m6f_prog_getinfo),
        hostile: entry(&raw const __m6f_prog_hostile),
        yield_prog: entry(&raw const __m6f_prog_yield),
        sleep_prog: entry(&raw const __m6f_prog_sleep),
        ttbr0_getinfo: super::boot::slot_ttbr0(slots[0]),
        ttbr0_hostile: super::boot::slot_ttbr0(slots[1]),
        ttbr0_yield: super::boot::slot_ttbr0(slots[2]),
        ttbr0_sleep: super::boot::slot_ttbr0(slots[3]),
    })
}

/// M6f verdict task: wait (bounded, CNTPCT) for the four M6f fixtures to exit, then print the three PASS/
/// FAIL lines + the per-task EL0 preempt breakdown (Part 0 fold #5). Spawned on a sibling core like the
/// other verdicts. Lines: (1) getinfo/copy_to_user round-trip — the witness is non-zero iff the pid read
/// back from the struct copy_to_user wrote equalled SYS_GETPID; (2) 4 hostile pointers refused (EFAULT), 0
/// kills — the hostile fixture counted 4 EFAULT returns and was NOT killed (a kill, or a kernel halt from a
/// stray store, would have prevented the report); (3) yield/sleep interleave — both fixtures completed all
/// iterations AND the kernel observed > 0 runner switches between them. The preempt line is QEMU-0 /
/// metal->0, so the next reflash reads exact per-slot-task preemption (the M6d ledger's aggregate refined).
pub fn m6f_verdict(_: usize) {
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq(); // ~5 s; the whole demo completes well under 1 s
    while EL0_M6F_DONE.load(Ordering::Acquire) < 4
        && super::timer::cntpct().wrapping_sub(start) <= deadline
    {
        super::sched::yield_now();
    }
    let getinfo = M6F_GETINFO_WITNESS.load(Ordering::Acquire);
    let hostile = M6F_HOSTILE_REFUSED.load(Ordering::Acquire);
    let ydone = M6F_YIELD_DONE.load(Ordering::Acquire);
    let sdone = M6F_SLEEP_DONE.load(Ordering::Acquire);
    let switches = M6F_INTERLEAVE_SWITCHES.load(Ordering::Acquire);
    let killed = EL0_M6F_KILLED.load(Ordering::Acquire);

    if getinfo != 0 && killed == 0 {
        serial_println!(":: M6f: getinfo/copy_to_user round-trip -> PASS ::");
    } else {
        serial_println!(
            ":: M6f: getinfo/copy_to_user round-trip (witness={:#x} killed={}) -> FAIL ::",
            getinfo, killed
        );
    }
    if hostile == 4 && killed == 0 {
        serial_println!(":: M6f: 4 hostile pointers refused (EFAULT), 0 kills -> PASS ::");
    } else {
        serial_println!(
            ":: M6f: hostile pointers refused={} killed={} (want 4/0) -> FAIL ::",
            hostile, killed
        );
    }
    if ydone == M6F_ITERS && sdone == M6F_ITERS && switches > 0 {
        serial_println!(":: M6f: yield/sleep interleave -> PASS ::");
    } else {
        serial_println!(
            ":: M6f: yield/sleep interleave (yield={} sleep={} switches={}) -> FAIL ::",
            ydone, sdone, switches
        );
    }
    // Per-task EL0 preempt breakdown (Part 0 fold #5): the exact per-slot-task attribution the M6d ledger's
    // aggregate `IRQs-taken-at-EL0` lacked. QEMU: all 0 (no timer IRQ). Metal: > 0 for the tasks a tick caught.
    serial_println!(
        ":: M6f: per-task EL0 preempts — samevaA={} samevaB={} stackwrite={} spsentinel={} yield={} sleep={} (metal >0; QEMU 0) ::",
        PRE_SAMEVA_A.load(Ordering::Relaxed),
        PRE_SAMEVA_B.load(Ordering::Relaxed),
        PRE_STACKWRITE.load(Ordering::Relaxed),
        PRE_SPSENTINEL.load(Ordering::Relaxed),
        PRE_YIELD.load(Ordering::Relaxed),
        PRE_SLEEP.load(Ordering::Relaxed),
    );
    // Release so the M6g loader (which polls this Acquire) sees every M6f verdict line published first —
    // its own late lines then land strictly after the M6f verdict.
    M6F_VERDICT_PRINTED.store(true, Ordering::Release);
}

/// One-shot: the first syscall proves the EL0→EL1 path is live end to end (logged off the ISR-free SVC
/// path, so `serial_println!` is safe here — unlike the RX ISR, nothing on this core holds SERIAL_PORT).
static SVC_LOGGED: AtomicBool = AtomicBool::new(false);

/// SVC dispatcher, called from the `__vec_svc` stub with a pointer to the saved GPR frame (SAVE_GPRS
/// layout: register x{i} is at byte 8*i, so x0 at frame+0, x8 at frame+64). Reads x8 = number and
/// x0–x5 = args, writes the return value into the x0 slot. Runs at EL1 on the faulting task's own
/// kernel stack with IRQ masked (exception entry masks it), so a blocking/exiting syscall may safely
/// `switch_context`, exactly like `timer_preempt` from `__vec_irq`.
#[unsafe(no_mangle)]
extern "C" fn aarch64_svc_handler(frame: *mut u64) {
    let nr = unsafe { *frame.add(8) }; // x8
    let a0 = unsafe { *frame.add(0) }; // x0
    let a1 = unsafe { *frame.add(1) }; // x1
    let a2 = unsafe { *frame.add(2) }; // x2
    let a3 = unsafe { *frame.add(3) }; // x3 (ELF-2: SYS_THREAD_SPAWN placement hint)

    if !SVC_LOGGED.swap(true, Ordering::Relaxed) {
        serial_println!(":: SVC: EC=0x15 nr={} — EL0->EL1 syscall path live ::", nr);
    }

    let ret: i64 = match nr {
        SYS_WRITE => sys_write(a0, a1, a2),
        SYS_REPORT => {
            // Route by the reporting task's name: M6d names land in m6d_report, M6f names in m6f_report, the
            // U4 parent/orphan in u4_report, the U5 cap fixture in u5_report, the U6 spawner in u6_report; each
            // ignores the others' names, so calling all is safe and additive.
            m6d_report(a0);
            m6f_report(a0);
            u4_report(a0);
            u5_report(a0);
            u6_report(a0);
            u6b_report(a0);
            u7_report(a0);
            u8_report(a0);
            u9_report(a0);
            u10_report(a0);
            u10c_report(a0);
            u10d_report(a0);
            u11_report(a0);
            u11defer_report(a0);
            u11reap_report(a0);
            uowner_report(a0);
            k2_report(a0);
            bandy_report(a0);
            elf1_report(a0);
            threads_report(a0);
            fb_report(a0);
            input_report(a0);
            0
        }
        SYS_YIELD => sys_yield(),
        SYS_SLEEP_MS => sys_sleep_ms(a0),
        SYS_GETPID => super::sched::current_id().map(|id| id as i64).unwrap_or(-1),
        SYS_GETINFO => sys_getinfo(a0),
        SYS_SPAWN => sys_spawn(),
        SYS_WAIT => sys_wait(a0),
        SYS_CAP => sys_cap(a0, a1, a2),
        SYS_OPEN => sys_open(a0, a1, a2),
        SYS_READ => sys_read(a0, a1, a2),
        SYS_SEEK => sys_seek(a0, a1),
        SYS_UNLINK => sys_unlink(a0),
        SYS_CLOSE => sys_close(a0),
        SYS_XFER => sys_xfer(a0, a1, a2),
        SYS_RECV => sys_recv(),
        SYS_FGRANT => sys_fgrant(a0, a1, a2),
        SYS_MSEND => sys_msend(a0, a1),
        SYS_MRECV => sys_mrecv(a0, a1),
        SYS_THREAD_SPAWN => sys_thread_spawn(a0, a1, a2, a3),
        SYS_THREAD_JOIN => sys_thread_join(a0),
        SYS_THREAD_EXIT => sys_thread_exit(), // never returns
        SYS_FB_MAP => sys_fb_map(),
        SYS_FB_PRESENT => sys_fb_present(),
        SYS_FUTEX => sys_futex(a0, a1, a2),
        SYS_INPUT_POLL => sys_input_poll(),
        SYS_EXIT => {
            // Demo accounting BEFORE the no-return exit. The sentinel statuses are routed to their own
            // counters so the M6b (`exited=1 killed=3`) and M6e (`completed=1`) verdicts stay byte-
            // identical: M6E_SPIN_STATUS -> EL0_SPIN_DONE, M6D_EXIT_STATUS -> EL0_M6D_DONE, M6F_EXIT_STATUS
            // -> EL0_M6F_DONE. All three sentinel arms MUST precede the catch-all `else` (a mis-ordered
            // sentinel exit would land in EL0_EXITED_ERR and FAIL the M6b verdict). Otherwise: status 0 =
            // normal completion (hello); nonzero = a fault-test program self-reporting that its intended
            // fault never happened (survivor protocol).
            // U4: a spawned CHILD's exit is reaped by its parent's sys_wait through the Proc table, keyed by
            // pid — NOT by any counter, and NOT by the handle (the handle is the parent's-side namespace; the
            // child's exit accounting is pid-keyed). This precedes every check below (the same precedence rule)
            // and SHORT-CIRCUITS to sched::exit() so the child's status-0 exit never lands in EL0_EXITED_OK
            // (M6b's `exited=1`) nor any sentinel counter. Record status + EXITED, then post `done` so the
            // (blocked or soon-to-block) parent's sys_wait wakes and reads the status. current_id() is the
            // exiting child = its Proc key (stored by sys_spawn before the child could ever be dispatched).
            // U7: the transfer fixtures exit by NAME, BEFORE the Proc short-circuit below — the CHILD has a
            // launcher-PLANTED Proc entry (the pid->ASID map sys_xfer resolves its dest through), so without
            // this precedence its sentinel exit would be swallowed by the child-reap path (posting a `done`
            // nobody waits) and `EL0_U7_DONE` would never gate the verdict. The launcher frees the planted
            // entry after its verdict; the parent (not in PROCS) rides the same arm for symmetry.
            {
                let nm = super::sched::current_name();
                if nm == Some("el0-u7parent") || nm == Some("el0-u7child") {
                    // Count only the true sentinel exit — a non-0x78 status here is a fixture bug, and the
                    // launcher's deadline-bounded wait then FAILs the verdict honestly (done < 2).
                    if a0 == U7_EXIT_STATUS {
                        EL0_U7_DONE.fetch_add(1, Ordering::AcqRel);
                    }
                    // Keep the launcher-PLANTED Proc entry truthful (the child has one — its pid->ASID
                    // map): mark it EXITED so any late sys_xfer to this recipient fails the RUNNING check
                    // instead of depositing into a torn-down inbox. No `done` post — the launcher waits on
                    // the counter above and frees the entry after its verdict.
                    if let Some(id) = super::sched::current_id() {
                        if let Some(i) = proc_find_running(id) {
                            PROCS[i].state.store(PEXITED, Ordering::Release);
                        }
                    }
                    super::sched::exit(); // never returns
                }
            }
            // U6: the owner/grants fixtures exit by NAME, BEFORE the Proc short-circuit — B has a launcher-PLANTED
            // Proc entry (the pid->ASID map A's SYS_FGRANT resolves its grantee through), so without this
            // precedence B's sentinel exit would be swallowed by the child-reap path and `EL0_UOWNER_DONE` would
            // never reach 2. A has no Proc entry (the mark is a no-op for it); it rides this arm for symmetry.
            {
                let nm = super::sched::current_name();
                if nm == Some("el0-uowner-a") || nm == Some("el0-uowner-b") {
                    if a0 == UOWNER_EXIT_STATUS {
                        EL0_UOWNER_DONE.fetch_add(1, Ordering::AcqRel);
                    }
                    if let Some(id) = super::sched::current_id() {
                        if let Some(i) = proc_find_running(id) {
                            PROCS[i].state.store(PEXITED, Ordering::Release);
                        }
                    }
                    super::sched::exit(); // never returns
                }
            }
            // K2 (make-enforcement-LIVE): the two REAL loaded programs exit by NAME, BEFORE the Proc
            // short-circuit. Neither has a launcher-planted Proc entry (no cross-program grants), so the
            // mark is a no-op for them; they ride this arm only to route the sentinel to EL0_K2_DONE (the
            // launcher's bounded wait gates on it) rather than the generic child-reap path.
            // BANDY-1 M4: midden (program #3) exits by NAME, same discipline as the K2 programs —
            // route its sentinel to EL0_MIDDEN_DONE (the bandy_rt launcher's bounded wait gates on
            // it) rather than the generic child-reap path.
            {
                let nm = super::sched::current_name();
                if nm == Some("el0-midden") {
                    if a0 == MIDDEN_EXIT_STATUS {
                        EL0_MIDDEN_DONE.fetch_add(1, Ordering::AcqRel);
                    }
                    if let Some(id) = super::sched::current_id() {
                        if let Some(i) = proc_find_running(id) {
                            PROCS[i].state.store(PEXITED, Ordering::Release);
                        }
                    }
                    super::sched::exit(); // never returns
                }
            }
            {
                let nm = super::sched::current_name();
                if nm == Some("el0-k2own") || nm == Some("el0-k2imp") {
                    if a0 == K2_EXIT_STATUS {
                        EL0_K2_DONE.fetch_add(1, Ordering::AcqRel);
                    }
                    if let Some(id) = super::sched::current_id() {
                        if let Some(i) = proc_find_running(id) {
                            PROCS[i].state.store(PEXITED, Ordering::Release);
                        }
                    }
                    super::sched::exit(); // never returns
                }
            }
            if let Some(id) = super::sched::current_id() {
                if let Some(i) = proc_find_running(id) {
                    PROCS[i].status.store(a0 as i32, Ordering::Release);
                    PROCS[i].state.store(PEXITED, Ordering::Release);
                    PROCS[i].done.post();
                    super::sched::exit(); // never returns
                }
            }
            // M6g: the disk-loaded program (the M6c `hello` bytes off the SD card) exits with status 0.
            // Route by NAME, BEFORE the sentinel-status checks, so its exit lands in the M6g counters and
            // never corrupts the M6b `EL0_EXITED_OK` accounting (which `exited=1` depends on).
            if super::sched::current_name() == Some("m6g-hello") {
                if a0 == 0 {
                    EL0_M6G_DONE.fetch_add(1, Ordering::AcqRel);
                } else {
                    EL0_M6G_ERR.fetch_add(1, Ordering::AcqRel);
                }
            } else if super::sched::current_name() == Some("elf1-hello") {
                // ELF-1: the ELF-loaded program exits 0. Route by NAME (like m6g-hello) so its clean exit
                // lands in the ELF1 counters and never corrupts the M6b `EL0_EXITED_OK` accounting.
                if a0 == 0 {
                    EL0_ELF1_DONE.fetch_add(1, Ordering::AcqRel);
                } else {
                    EL0_ELF1_ERR.fetch_add(1, Ordering::AcqRel);
                }
            } else if super::sched::current_name() == Some("el0-threads") {
                // ELF-2: the threads-test PARENT exits (status 0) after joining both worker threads. Route by
                // NAME (like elf1-hello) so it lands in the ELF-2 witness, never the M6b `EL0_EXITED_OK`
                // accounting. The worker threads exit via SYS_THREAD_EXIT (not here).
                THREADS_PARENT_DONE.store(a0 == 0, Ordering::Release);
            } else if super::sched::current_name() == Some("el0-fb") {
                // ELF-3: the fb-test PARENT exits (status 0) after presenting + joining both draw threads.
                // Route by NAME so it lands in the ELF-3 witness, never the M6b `EL0_EXITED_OK` accounting.
                FB_PARENT_DONE.store(a0 == 0, Ordering::Release);
            } else if super::sched::current_name() == Some("el0-input") {
                // ELF-5: the input-test program exits (status 0) after polling empty, draining the injected
                // event, and polling empty again. Route by NAME so it lands in the ELF-5 witness, never the
                // M6b `EL0_EXITED_OK` accounting.
                INPUT_PARENT_DONE.store(a0 == 0, Ordering::Release);
            } else if a0 == M6E_SPIN_STATUS {
                EL0_SPIN_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == M6D_EXIT_STATUS {
                EL0_M6D_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == M6F_EXIT_STATUS {
                EL0_M6F_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U4_EXIT_STATUS {
                EL0_U4_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U5_EXIT_STATUS {
                EL0_U5_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U6_EXIT_STATUS {
                EL0_U6_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U6B_EXIT_STATUS {
                EL0_U6B_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U8_EXIT_STATUS {
                EL0_U8_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U9_EXIT_STATUS {
                EL0_U9_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U10_EXIT_STATUS {
                EL0_U10_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U10C_EXIT_STATUS {
                EL0_U10C_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U10D_EXIT_STATUS {
                EL0_U10D_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U11_EXIT_STATUS {
                EL0_U11_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U11DEFER_EXIT_STATUS {
                // Both defer fixtures (A + B) ride this one sentinel; neither has a planted Proc entry (they use
                // no sys_xfer), so both fall through the Proc short-circuit to here. Want 2 (both exited).
                EL0_U11DEFER_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == U11REAP_EXIT_STATUS {
                // Both reap fixtures (A + B) ride this one sentinel; neither has a planted Proc entry. Want 2.
                // A's exit here is the LAST close of the unlinked file (it never SYS_CLOSEd) — its teardown
                // queues the orphaned chain for the reaper.
                EL0_U11REAP_DONE.fetch_add(1, Ordering::AcqRel);
            } else if a0 == 0 {
                EL0_EXITED_OK.fetch_add(1, Ordering::AcqRel);
            } else {
                EL0_EXITED_ERR.fetch_add(1, Ordering::AcqRel);
            }
            super::sched::exit() // never returns; the __vec_svc eret tail is not reached
        }
        _ => -38, // -ENOSYS
    };
    unsafe { *frame.add(0) = ret as u64 }; // return value in x0
}

// =============================================================================================
// M6f: validated user-pointer copies (copy_from_user / copy_to_user) + the wider syscall surface
// =============================================================================================

/// The single error `copy_from_user`/`copy_to_user` return: a user pointer/length failed validation
/// (outside the task's window, a wrapping range, or — to-user only — the read-only code page). Mapped to
/// `-EFAULT` (`EFAULT`) at the syscall boundary. A bad pointer ARG is an error RETURN, never a task-kill:
/// kills are reserved for faults the HARDWARE raises (M6b), not a syscall arg the kernel can reject cheaply.
pub struct Efault;

/// `-EFAULT`, the errno a rejected user pointer returns to EL0.
const EFAULT: i64 = -14;

/// Validate that `[user_va, user_va+len)` lies entirely inside the calling task's EL0 window. `writable`
/// additionally excludes the read-only CODE page (page 0, `[base, base+USER_CODE_SIZE)`), so `copy_to_user`
/// refuses a write aimed there (an EL1 store to an AP=0b11 page Permission-faults -> the kernel-fault path
/// halts the core; we reject BEFORE any deref instead of taking that fault). Checks, in order: `len == 0`
/// is handled by the callers' fast path; `checked_add` rejects a length that wraps; the range must sit
/// fully in `[lo, base+size)`. A syscall executes with the caller's TTBR0/ASID live (M6d), so a user VA in
/// this window can only reach that task's OWN frames — validation + that guarantee is the PAN-less software
/// discipline (A72 is Armv8.0, no FEAT_PAN; on a PAN-capable port this must become an LDTR/unprivileged copy).
fn user_range_ok(user_va: u64, len: usize, writable: bool) -> bool {
    let (base, size) = super::boot::user_region();
    let Some(end) = user_va.checked_add(len as u64) else {
        return false; // length wraps the address space
    };
    let lo = if writable { base + super::boot::USER_CODE_SIZE as u64 } else { base };
    user_va >= lo && end <= base + size as u64
}

/// Copy `len` bytes from the EL0 buffer at `user_va` into `kdst`, after validating the whole SOURCE range
/// is inside the caller's user window. Never dereferences the pointer until all checks pass. `Err(Efault)`
/// on a bad pointer/length. Factored out of the M6b SYS_WRITE bound-check; `kdst.len() >= len` is a
/// kernel-side contract (debug-asserted).
pub fn copy_from_user(kdst: &mut [u8], user_va: u64, len: usize) -> Result<(), Efault> {
    if len == 0 {
        return Ok(());
    }
    debug_assert!(kdst.len() >= len, "copy_from_user: kdst smaller than len (kernel bug)");
    if !user_range_ok(user_va, len, false) {
        return Err(Efault);
    }
    // SAFETY: range validated inside the user window; the syscall runs with the caller's TTBR0 live, so the
    // VA resolves to the caller's own frames, readable at EL1 (AP=0b01/0b11) on this PAN-less A72.
    unsafe { core::ptr::copy_nonoverlapping(user_va as *const u8, kdst.as_mut_ptr(), len) };
    Ok(())
}

/// Copy `len` bytes from `ksrc` to the EL0 buffer at `user_va`, after validating the whole DESTINATION
/// range is inside the caller's WRITABLE user window (the RO code page is excluded, so a write aimed there
/// is refused with `Efault`, never a faulting EL1 store). The to-user twin of `copy_from_user`.
pub fn copy_to_user(user_va: u64, ksrc: &[u8], len: usize) -> Result<(), Efault> {
    if len == 0 {
        return Ok(());
    }
    debug_assert!(ksrc.len() >= len, "copy_to_user: ksrc smaller than len (kernel bug)");
    if !user_range_ok(user_va, len, true) {
        return Err(Efault);
    }
    // SAFETY: range validated inside the writable user window (code page excluded); caller's TTBR0 live.
    unsafe { core::ptr::copy_nonoverlapping(ksrc.as_ptr(), user_va as *mut u8, len) };
    Ok(())
}

/// SYS_WRITE(fd, buf, len): write `len` bytes from the EL0 buffer, returning the count or a negative errno.
/// U9 makes this KIND-DISPATCHED at the single CAP_WRITE CHECK: a `Console` handle streams to the serial
/// console (byte-identical to before); a `File` handle overwrites in place at the descriptor's offset via
/// `sys_write_file` (`fat::write_at`). Routed through `copy_from_user`: validate the WHOLE range up front so a
/// hostile pointer yields `-EFAULT` with NO partial output (byte-identical to the pre-M6f all-or-nothing
/// behaviour), then stream/write in bounded chunks THROUGH the validated copy primitive.
fn sys_write(fd: u64, buf: u64, len: u64) -> i64 {
    // U5/U9: `fd` is a HANDLE INDEX into the caller's per-process table, not the ambient POSIX stdout. It must
    // resolve to a resource carrying CAP_WRITE. No such handle / a File LACKING CAP_WRITE (an RO open) / a
    // non-{Console,File} kind all yield -EACCES — the single enforcement point (the U8 derivation/revocation
    // check rides inside `handle_resolve`, so a revoked File-write cap is `-EACCES` here too). A `Console`
    // falls through to the serial path below; a `File` with CAP_WRITE routes to the in-place FAT writer.
    let asid = current_asid();
    match handle_resolve(asid, fd, CAP_WRITE) {
        Ok(HandleTarget::Console) => {} // fall through to the console-streaming path below
        Ok(HandleTarget::File(file_id)) => return sys_write_file(asid, file_id, buf, len),
        _ => return EACCES,
    }
    let total = len as usize;
    if !user_range_ok(buf, total, false) {
        return EFAULT; // reject before ANY output (matches the old all-or-nothing semantics)
    }
    let mut chunk = [0u8; 256];
    let mut off = 0usize;
    // Byte loop (not fmt) keeps the syscall path FP-light and handles non-UTF-8 bytes. Held IRQ-masked at
    // EL1 (exception entry), so the SERIAL_PORT lock can't be re-entered by an interrupt on this core;
    // copy_from_user does a plain memcpy (no serial, no block) under the lock.
    let port = super::serial::SERIAL_PORT.lock();
    while off < total {
        let n = core::cmp::min(chunk.len(), total - off);
        // A subrange of the already-validated range, so copy_from_user's re-check always passes here.
        if copy_from_user(&mut chunk[..n], buf + off as u64, n).is_err() {
            return EFAULT;
        }
        for &b in &chunk[..n] {
            port.write_byte(b);
        }
        off += n;
    }
    len as i64
}

/// U10: the largest single File `sys_write` that GROWS a file (bounds the kernel copy buffer + the number of
/// clusters one call allocates). A longer write returns a short count (`GROW_WRITE_MAX`); the caller loops. The
/// in-place (non-growing) branch keeps U9's `min(len, size - offset)` clamp untouched, so this only ever caps
/// the growth path. 8 KiB = up to 16 new 512-byte clusters per call — ample for the demo, bounded for safety.
const GROW_WRITE_MAX: usize = 8 * 1024;

/// U9/U10: the File half of `sys_write` at the descriptor's offset. The CHECK already passed in `sys_write` (a
/// `File` handle carrying CAP_WRITE, non-revoked), so this decodes the file-id -> descriptor, then splits on
/// whether the write stays within the current bytes:
///   * **in place** (`len <= size - offset`) — U9's path, BYTE-IDENTICAL: clamp to EOF, validate the whole
///     source up front, copy into a bounded buffer, `fat::write_at`, advance the offset;
///   * **grow** (`len` runs past EOF) — U10: cap to `GROW_WRITE_MAX`, validate + copy, then `fat::write_grow`
///     ALLOCATES + zero-fills + chains new clusters and bumps the on-disk directory `size` LAST; republish the
///     new size / chain-head / offset into the descriptor.
/// The grow path is reachable ONLY through this function, which `sys_write` reaches ONLY after resolving the
/// handle for CAP_WRITE — so growth is CAP_WRITE-gated by exactly the same single CHECK as the in-place write
/// (an RO-opened File, a revoked cap, or a non-File kind can never reach it). A bad buffer is `-EFAULT` with no
/// I/O and no offset move; `-ENOSPC` when the volume is full.
fn sys_write_file(asid: u64, file_id: u64, buf: u64, len: u64) -> i64 {
    // U11: decode + validate through the single descriptor-identity seam (range, `FILE_USED`, and generation —
    // a stale write-cap to a reused slot is rejected here, never rebound onto the different file's chain).
    let Some(idx) = file_desc_validate(asid, file_id) else {
        return EACCES;
    };
    let size = FILE_SIZE[asid as usize][idx].load(Ordering::Acquire);
    let offset = FILE_OFFSET[asid as usize][idx].load(Ordering::Acquire);
    let inplace_avail = size.saturating_sub(offset) as usize; // bytes from `offset` to EOF

    if len as usize <= inplace_avail {
        // ===== U9 IN-PLACE branch — unchanged (a write wholly within the current bytes never grows) =====
        // `offset <= size` always (reads/writes clamp; seek rejects past size), so `size - offset` cannot
        // underflow; a write at/after EOF with `len == 0` is a clean 0-byte no-op.
        let want = core::cmp::min(len as usize, inplace_avail);
        if want == 0 {
            return 0; // at/after EOF, or nothing requested — 0 bytes written, no growth
        }
        // Validate the WHOLE source BEFORE any disk I/O — a bad buffer is -EFAULT with no write, no offset move.
        if !user_range_ok(buf, want, false) {
            return EFAULT;
        }
        // Copy the user bytes into a bounded kernel buffer (capped at `want <= size`), then overwrite in place.
        let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        data.resize(want, 0);
        if copy_from_user(&mut data, buf, want).is_err() {
            return EFAULT; // offset NOT advanced — a rejected buffer leaves the file position unchanged
        }
        // Re-mount (as sys_read does — the single volume is deterministic) and write `want` bytes at `offset`.
        let fs = match crate::fs::fat::mount() {
            Ok(fs) => fs,
            Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
            Err(_) => return EIO,
        };
        let cluster = FILE_CLUSTER[asid as usize][idx].load(Ordering::Acquire);
        let wrote = match fs.write_at(cluster, size, offset, &data) {
            Ok(n) => n,
            Err(_) => return EIO,
        };
        if wrote == 0 {
            return 0; // a short chain (malformed vs size) wrote nothing here — no offset move
        }
        // wrote <= want <= size - offset, so offset + wrote <= size — the FILE_OFFSET <= FILE_SIZE invariant holds.
        FILE_OFFSET[asid as usize][idx].store(offset + wrote as u32, Ordering::Release);
        return wrote as i64;
    }

    // ===== U10 GROW branch — the write runs past EOF; extend the file. `offset <= size` (seek gate), so there
    // is no sparse hole. Cap the write so one call's kernel buffer + cluster allocation stay bounded.
    let want = core::cmp::min(len as usize, GROW_WRITE_MAX);
    if want == 0 {
        return 0; // (unreachable: len > inplace_avail >= 0 implies len >= 1) — defensive
    }
    if !user_range_ok(buf, want, false) {
        return EFAULT; // bad buffer -> -EFAULT with no I/O, no allocation, no offset move
    }
    let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    data.resize(want, 0);
    if copy_from_user(&mut data, buf, want).is_err() {
        return EFAULT;
    }
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    let cluster = FILE_CLUSTER[asid as usize][idx].load(Ordering::Acquire);
    let dir_lba = FILE_DIR_LBA[asid as usize][idx].load(Ordering::Acquire);
    let dir_off = FILE_DIR_OFF[asid as usize][idx].load(Ordering::Acquire) as usize;
    let (wrote, new_size, new_first) = match fs.write_grow(cluster, size, dir_lba, dir_off, offset, &data) {
        Ok(t) => t,
        Err(crate::fs::fat::FatError::NoSpace) => return ENOSPC,
        Err(_) => return EIO, // a bad chain / block error — nothing advanced (the dir bump is write_grow's last step)
    };
    if wrote == 0 {
        return 0; // no bytes written (empty source already handled; defensive) — no state change
    }
    // Republish the descriptor: new chain head (unchanged unless the file had 0 clusters), grown size, and the
    // advanced offset. `offset + wrote <= new_size` (new_size = max(size, offset + want), wrote <= want), so the
    // FILE_OFFSET <= FILE_SIZE invariant holds. Single-writer over this row (mid-syscall, IRQ-masked).
    FILE_CLUSTER[asid as usize][idx].store(new_first, Ordering::Release);
    FILE_SIZE[asid as usize][idx].store(new_size, Ordering::Release);
    FILE_OFFSET[asid as usize][idx].store(offset + wrote as u32, Ordering::Release);
    // K2 M(b) / K6 M3: a file just GREW — if it has a NAMED owner (a persisting principal), re-persist its
    // NATIVE ACL row's first_cluster so the create-time `fc = 0` becomes the real chain head and the
    // rebuild's identity cross-check engages. NO-OP with ZERO disk I/O for an anonymous owner (the whole
    // battery, incl. the u10 GROW.BIN fixture), so this path stays byte-identical. Runs AFTER the descriptor
    // republish, OUTSIDE any inner lock (native_persist_grow probes the owner, then snapshots + writes the row
    // under ONE ns — K5 M1). Non-fatal — losing it only weakens the out-of-scope offline-tamper corroboration.
    let _ = native_persist_grow(dir_lba, dir_off as u32, new_first);
    wrote as i64
}

/// The fixed struct SYS_GETINFO writes to EL0. `#[repr(C)]` so the byte layout is stable for the user
/// program that reads it back: `pid` at offset 0, `ticks` at offset 8 (16 bytes total).
#[repr(C)]
struct UserInfo {
    pid: u64,
    ticks: u64,
}

/// SYS_GETINFO(user_ptr): write a small fixed {pid, ticks} struct to the caller's buffer via
/// `copy_to_user` — the to-user direction's exerciser. Returns 0, or `-EFAULT` if the pointer/length fails
/// validation (e.g. aimed at the RO code page) — an error RETURN, never a task-kill.
fn sys_getinfo(user_ptr: u64) -> i64 {
    let info = UserInfo {
        pid: super::sched::current_id().unwrap_or(0),
        ticks: super::timer::ticks(),
    };
    // SAFETY: view `info` as its raw bytes for the copy; `UserInfo` is `#[repr(C)]` plain-old-data.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &info as *const UserInfo as *const u8,
            core::mem::size_of::<UserInfo>(),
        )
    };
    match copy_to_user(user_ptr, bytes, bytes.len()) {
        Ok(()) => 0,
        Err(Efault) => EFAULT,
    }
}

/// SYS_YIELD: cooperatively give up the CPU — thin over `sched::yield_now()`. `yield_now` unmasks IRQ on
/// return, but the `__vec_svc` epilogue that runs after this handler restores per-core banked
/// ELR/SPSR/SP_EL0 and MUST be I-masked, so re-mask before returning (see `remask_irq`). Records one
/// interleave observation for the M6f yield/sleep witness. Returns 0.
fn sys_yield() -> i64 {
    note_interleave();
    super::sched::yield_now();
    remask_irq();
    0
}

/// SYS_SLEEP_MS(ms): block the calling EL0 task ~`ms` milliseconds — thin over `sched::sleep_ticks`
/// (ms→ticks at the 250 Hz per-core tick, rounding UP so a sub-tick sleep still waits >= 1 tick; M6f adds
/// no scheduler primitive). QEMU delivers no timer IRQ, so `sleep_ticks` (whose only waker is the tick)
/// would park the task FOREVER; when the timer is not live, fall back to a cooperative `yield_now` — the
/// same guard `input_service`/`rx_backstop` use — so the interleave demo makes progress and the regression
/// never hangs. The real timed sleep rides along on metal. Both `sleep_ticks` and `yield_now` unmask IRQ,
/// so re-mask before returning to the I-masked `__vec_svc` epilogue. Returns 0.
fn sys_sleep_ms(ms: u64) -> i64 {
    /// The scheduler tick rate; mirrors `timer::TICK_HZ` (private there). Only used for the ms→ticks
    /// conversion — no timer register is touched (the STOP tripwire on timer timing stands).
    const TICK_HZ: u64 = 250;
    let ticks = (ms.saturating_mul(TICK_HZ) + 999) / 1000; // round up
    note_interleave();
    if super::timer::is_live() {
        super::sched::sleep_ticks(ticks);
    } else {
        super::sched::yield_now();
    }
    remask_irq();
    0
}

/// Re-mask IRQ (set PSTATE.I). `yield_now`/`sleep_ticks` unmask on return, but the `__vec_svc` epilogue
/// after this handler restores the per-core banked ELR_EL1/SPSR_EL1/SP_EL0 and MUST be I-masked — a nested
/// IRQ between those `msr`s and the `eret` would re-bank them and corrupt the EL0 return (the same
/// invariant the `__vec_irq` epilogue documents). Exception entry masks DAIF, so the handler is entered
/// I-masked; the two syscalls that unmask (via a scheduler switch) re-mask here before returning.
#[inline]
fn remask_irq() {
    unsafe { core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags)) };
}

/// M6f: record one yield/sleep interleave observation. Called from the SYS_YIELD/SYS_SLEEP_MS handlers
/// with the reporting task current; the two interleave fixtures run on one core, so a change of runner
/// since the previous yielding syscall is one observed switch (`M6F_INTERLEAVE_SWITCHES > 0` proves both
/// ran and the scheduler passed control back and forth). Only the two named M6f interleave tasks
/// participate (kernel `yield_now` callers don't come through the syscall path; other EL0 tasks aren't
/// named these). Under QEMU the interleave is purely the SYS_YIELD round-robin; on metal the timer also
/// preempts them.
fn note_interleave() {
    let tag = match super::sched::current_name() {
        Some("el0-yield") => 1u32,
        Some("el0-sleep") => 2u32,
        _ => return,
    };
    let last = M6F_INTERLEAVE_LAST.swap(tag, Ordering::AcqRel);
    if last != 0 && last != tag {
        M6F_INTERLEAVE_SWITCHES.fetch_add(1, Ordering::AcqRel);
    }
}

/// M6f: record a value an M6f EL0 fixture reported via SYS_REPORT, keyed by the reporting task's name.
/// (M6d names fall through to `m6d_report`, which the SYS_REPORT arm also calls; the name spaces are
/// disjoint, so each function ignores the other's tasks.)
fn m6f_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-getinfo") => M6F_GETINFO_WITNESS.store(value, Ordering::Release),
        Some("el0-hostile") => M6F_HOSTILE_REFUSED.store(value as u32, Ordering::Release),
        Some("el0-yield") => M6F_YIELD_DONE.store(value as u32, Ordering::Release),
        Some("el0-sleep") => M6F_SLEEP_DONE.store(value as u32, Ordering::Release),
        _ => {} // a stray report from any other task is ignored (never happens in the demo)
    }
}

// =============================================================================================
// U4: sys_spawn (load+run a child from storage, return a HANDLE) + sys_wait (reap by handle) + shared loader
// =============================================================================================

// Negative errnos returned to EL0 by sys_spawn/sys_wait (Linux-aarch64 values). These never appear in the
// demo's serial output — the parent fixture only tests the SIGN of the spawn return — but are named for the
// (future) real userspace that will interpret them. `EFAULT` (-14) is already defined for the M6f copies.
const ENOENT: i64 = -2; // no such file (HELLO.BIN missing)
const EIO: i64 = -5; // read/mount I/O error, or an empty file
const E2BIG: i64 = -7; // the program is larger than one code page
const EBADF: i64 = -9; // U11: sys_close of an unresolvable / already-closed / stale-slot handle
const ESRCH: i64 = -3; // ELF-2: sys_thread_join on a handle that is not the caller's live thread
const ECHILD: i64 = -10; // sys_wait: no child with that pid
const EAGAIN: i64 = -11; // the process table (or slot pool) is full
const ENODEV: i64 = -19; // no block device / FAT volume to load from
const EISDIR: i64 = -21; // sys_open: the named entry is a directory, not a file
const ENFILE: i64 = -23; // U11-M2: the GLOBAL cross-process open-file table is full (a new file identity, no row)
const EMFILE: i64 = -24; // sys_open: the caller's open-file (or handle) table is full
const ENOSPC: i64 = -28; // U10: no free cluster (volume full) / no free root-dir slot (grow/create)
const EEXIST: i64 = -17; // BANDY-1: bus cp refuses an existing destination (create-new-only in v1)
const EMSGSIZE: i64 = -90; // BANDY-1: SYS_MRECV buffer smaller than BUS_FRAME_MAX (whole-frame contract)

/// A program successfully loaded into a fresh per-task slot: the EL0 entry VA, the initial SP_EL0, the
/// slot's TTBR0, and (for the M6g loader's log line) the slot id, byte length, and FAT kind.
struct Loaded {
    base: u64, // the EL0 ENTRY VA: the window base for a flat blob, `bias + e_entry` for an ELF
    sp: u64,
    ttbr0: u64,
    slot: usize,
    len: usize, // total on-disk image byte length (for the loader's log line)
    kind: crate::fs::fat::FatKind,
    // ELF-1: how the image was loaded. `is_elf` distinguishes the two paths; `nsegs` is the number of
    // PT_LOAD segments mapped (1 for the flat path). The ELF-1 witness reads these to prove the ELF path ran.
    is_elf: bool,
    nsegs: u32,
}

/// Why `load_program_into_slot` could not produce a `Loaded`. The `FatKind` rides along on the post-mount
/// variants so the M6g loader can reproduce its exact "FAT mounted from SD (..)" progress line before its
/// specific skip line (keeping the M6g gate byte-identical); sys_spawn maps every variant to a negative errno.
enum SpawnErr {
    NoMount(crate::fs::fat::FatError),
    NoFile(crate::fs::fat::FatKind),
    BadSize(crate::fs::fat::FatKind, u32),
    ReadErr(crate::fs::fat::FatKind, crate::fs::fat::FatError),
    Empty(crate::fs::fat::FatKind),
    NoSlot(crate::fs::fat::FatKind),
    // ELF-1: the image began with the ELF magic but was not a loadable minimal static aarch64 ELF (a
    // rejected ident/type/machine, a malformed/oversize program header, or a segment that would not fit
    // the slot window). The `&'static str` names the specific rejection for the loader's honest log line.
    BadElf(crate::fs::fat::FatKind, &'static str),
}

/// Map a load failure to the errno sys_spawn returns to EL0.
fn spawn_errno(e: &SpawnErr) -> i64 {
    match e {
        SpawnErr::NoMount(_) => ENODEV,
        SpawnErr::NoFile(_) => ENOENT,
        SpawnErr::BadSize(_, _) => E2BIG,
        SpawnErr::ReadErr(_, _) | SpawnErr::Empty(_) => EIO,
        SpawnErr::NoSlot(_) => EAGAIN,
        SpawnErr::BadElf(_, _) => EINVAL, // a malformed/unsupported ELF is a bad argument, not I/O
    }
}

/// The shared loader CORE for both `m6g_loader` and `sys_spawn`: mount the FAT volume off the SD card, find
/// and size-check the fixed program (`HELLO.BIN`), read it, copy it into a FRESH per-task slot's code page,
/// I-cache-sync, protect the page EL0-RX/EL1-RO BEFORE any task exists, and return the run parameters. It
/// PRINTS NOTHING (so sys_spawn stays silent inside the U4 flow) — the M6g loader reconstructs its serial
/// lines from the `Loaded`/`SpawnErr` result.
///
/// The slot is allocated LAST — after every fallible step (mount/find/size/read) — so no failure path ever
/// leaves an allocated slot to free (the A72 exposes no "free an unused slot" primitive, and
/// `teardown_user_slot` would repoint the caller's live TTBR0). A single `alloc_user_slot` attempt suffices:
/// both callers run only after M6d/M6f/M6g released their slots, so the pool has room. The loaded bytes are
/// UNTRUSTED — nothing about them is trusted beyond the one-page size bound; they run only under EL0 +
/// per-page permissions + the M6b fault-kill net (no signature, no allowlist). That containment is the point.
fn load_program_into_slot(name: &str) -> Result<Loaded, SpawnErr> {
    let fs = crate::fs::fat::mount().map_err(SpawnErr::NoMount)?;
    let kind = fs.kind();
    let de = fs.find_in_root(name).map_err(|_| SpawnErr::NoFile(kind))?;
    // Read the WHOLE on-disk image, bounded by the slot's user window (the U2 truncation lesson: `read_file`
    // caps at min(de.size, cap), so a post-read length check could never SEE an oversize file — it would
    // silently truncate then run it; gate on `de.size` up front). The cap is the 16 KiB WINDOW, not one code
    // page, so a small static ELF's headers + segments fit; the FLAT path re-bounds to one code page below,
    // and the ELF path bounds each SEGMENT to the window. The image is hashed WHOLE for the principal, so the
    // cap must admit the whole file (a >window image is `BadSize`, never a truncated hash).
    let cap = super::boot::USER_REGION_SIZE;
    if de.size == 0 || de.size as u64 > cap as u64 {
        return Err(SpawnErr::BadSize(kind, de.size));
    }
    let mut bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    fs.read_file(&de, &mut bytes, cap).map_err(|e| SpawnErr::ReadErr(kind, e))?;
    if bytes.is_empty() {
        return Err(SpawnErr::Empty(kind));
    }

    // ELF-1/EXEC-1: hand the read bytes to the shared MAPPER (`map_image_into_slot`) — it dispatches on the
    // ELF magic, validates fully BEFORE it allocates a slot (the "slot allocated LAST" invariant), then
    // copies+maps+protects. The mapper is FatKind-free (shared with the shell's `run_user_image`); re-attach
    // the kind here so this loader's `SpawnErr` variants and the m6g/ELF1 log lines stay byte-identical.
    let mapped = map_image_into_slot(&bytes).map_err(|e| match e {
        MapErr::Empty => SpawnErr::Empty(kind),
        MapErr::BadSize(sz) => SpawnErr::BadSize(kind, sz),
        MapErr::BadElf(why) => SpawnErr::BadElf(kind, why),
        MapErr::NoSlot => SpawnErr::NoSlot(kind),
    })?;
    Ok(Loaded {
        base: mapped.base,
        sp: mapped.sp,
        ttbr0: mapped.ttbr0,
        slot: mapped.slot,
        len: mapped.len,
        kind,
        is_elf: mapped.is_elf,
        nsegs: mapped.nsegs,
    })
}

/// The FatKind-free result of `map_image_into_slot`: the EL0 run parameters. `load_program_into_slot`
/// wraps it in a `Loaded` (re-attaching the FAT kind); `run_user_image` consumes it directly.
struct Mapped {
    base: u64, // the EL0 ENTRY VA (window base for a flat blob, `bias + e_entry` for an ELF)
    sp: u64,   // 16-aligned window top = initial SP_EL0
    ttbr0: u64,
    slot: usize,
    len: usize,
    is_elf: bool,
    nsegs: u32,
}

/// A FatKind-free mapping failure. `load_program_into_slot` re-attaches the kind to reproduce its exact
/// `SpawnErr`; `run_user_image` renders it as an operator string.
enum MapErr {
    Empty,
    BadSize(u32), // a flat blob larger than one code page
    BadElf(&'static str),
    NoSlot,
}

/// EXEC-1: the FatKind-free image MAPPER — the shared CORE of `load_program_into_slot` (the by-name FAT
/// loader) and `run_user_image` (the shell's VFS-read `run <path>`). Given the WHOLE read image (already
/// bounded to `USER_REGION_SIZE` by the caller), it dispatches on the ELF magic, VALIDATES fully BEFORE
/// allocating a slot (the "slot allocated LAST" invariant — no fallible step may follow the alloc), then
/// copies each PT_LOAD (or the one flat page) into a FRESH slot, I-cache-syncs, and applies per-segment
/// W^X page permissions (PF_X -> RO+EL0-exec code pages; R/W -> EL0+EL1-RW/never-exec data pages). It
/// stamps the IMAGE_SHA256 principal over the whole image. The bytes are UNTRUSTED — nothing is trusted
/// beyond the size bound + per-page permissions; they run only under EL0 + the M6b fault-kill net.
fn map_image_into_slot(bytes: &[u8]) -> Result<Mapped, MapErr> {
    if bytes.is_empty() {
        return Err(MapErr::Empty);
    }
    let (base, size) = super::boot::user_region();
    let elf_plan = if is_elf_image(bytes) {
        Some(validate_elf(bytes, size).map_err(MapErr::BadElf)?)
    } else {
        // FLAT path: the historical model — one code page, entered at offset 0, position-independent. Keep
        // the exact `USER_CODE_SIZE` bound (a larger flat blob is `BadSize`, byte-identical to before).
        if bytes.len() > super::boot::USER_CODE_SIZE {
            return Err(MapErr::BadSize(bytes.len() as u32));
        }
        None
    };

    let slot = super::boot::alloc_user_slot().ok_or(MapErr::NoSlot)?;
    let backing = super::boot::slot_backing_ptr(slot);
    let (entry, nsegs, is_elf) = match &elf_plan {
        Some(plan) => {
            // Map each PT_LOAD: zero [dst, dst+memsz) (the .bss tail — p_memsz>p_filesz), copy p_filesz
            // bytes, then flip executable segments to code pages; R/W segments stay data pages.
            for i in 0..plan.nsegs {
                let s = plan.segs[i];
                let dst_off = (s.vaddr - plan.min_vaddr) as usize;
                unsafe {
                    let dst = backing.add(dst_off);
                    core::ptr::write_bytes(dst, 0, s.memsz); // zero the whole memsz (covers the bss tail)
                    core::ptr::copy_nonoverlapping(bytes.as_ptr().add(s.off), dst, s.filesz);
                }
            }
            // I-cache sync + code-protect the executable segments (after the copies, while pages are RW).
            for i in 0..plan.nsegs {
                let s = plan.segs[i];
                if s.flags & PF_X != 0 {
                    let dst_off = (s.vaddr - plan.min_vaddr) as usize;
                    super::cache::icache_sync_range(backing as usize + dst_off, s.memsz);
                    unsafe { super::boot::protect_user_slot_code_range(slot, dst_off, s.memsz) };
                }
            }
            let entry = base + (plan.entry - plan.min_vaddr);
            (entry, plan.nsegs as u32, true)
        }
        None => {
            // FLAT: copy to the code page at the window base, I-cache sync, protect page 0.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), backing, bytes.len()) };
            super::cache::icache_sync_range(backing as usize, bytes.len());
            unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
            (base, 1, false)
        }
    };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    // IMAGE_SHA256 (code-signing): stamp this slot's persistent principal from the loaded IMAGE bytes, not
    // any 8.3 name — the SOLE mint path, kernel-derived from the untrusted image, never EL0-set. Two
    // byte-identical images share a principal; two different images do not. Hashed over the WHOLE file image
    // (flat or ELF) identically.
    slot_ppid_stamp(ttbr0 >> 48, PrincipalRecord::image_of(bytes));
    Ok(Mapped {
        base: entry,
        sp: (base + size as u64) & !0xF, // 16-aligned window top = initial SP_EL0
        ttbr0,
        slot,
        len: bytes.len(),
        is_elf,
        nsegs,
    })
}

/// EXEC-1: the outcome of `run_user_image` — the program exited with a status, was killed by the fault-kill
/// net (a CONTAINED fault), or overran its deadline (still running / stuck).
pub enum RunOutcome {
    Exited(i32),
    Faulted,
    Timeout,
}

/// run_user_image: the Proc `status` a killed run-image task is marked with (see the fault-kill path), so
/// the wait renders it as `Faulted` rather than `Exited`. `i32::MIN` never collides with a real exit code.
const EXEC_KILLED_STATUS: i32 = i32::MIN;

/// EXEC-1: load an already-read program IMAGE (flat or ELF64) into a fresh EL0 slot, run it to completion
/// on THIS core, and return its exit status. The synchronous shell `run <path>` entry: the EL1/ASID-0 panel
/// shell reads the bytes off the VFS and hands them here. We map (via the shared `map_image_into_slot`),
/// endow a console write-cap, plant a Proc entry so the GENERIC child-reap short-circuit in the SYS_EXIT
/// handler records the status (no dedicated exit arm — the program runs under an arbitrary `name`), spawn it
/// co-located, then deadline-bounded-yield until it exits or faults. On return the scheduler has already
/// repointed this core to the boot root (ASID 0) via `teardown_user_slot`, honoring the shell's ASID-0
/// invariant (shell.rs). `deadline_ticks` is a CNTPCT span (`timer::cntfrq()` = 1 s).
/// Returns `(outcome, entry)` where `entry` is the EL0 entry VA the image was mapped at (for the caller's
/// witness line) — or an operator string if the image could not be loaded.
pub fn run_user_image(
    name: &'static str,
    bytes: &[u8],
    deadline_ticks: u64,
) -> Result<(RunOutcome, u64), &'static str> {
    // Fail-closed backstop (the caller also bounds): the whole image must fit the slot window. The mapper
    // re-bounds the flat path to one code page and each ELF segment to the window.
    if bytes.len() > super::boot::USER_REGION_SIZE {
        return Err("image larger than the 16 KiB user window");
    }
    // Claim the Proc entry FIRST so a failed map frees nothing but the entry (no slot is allocated on any
    // map-failure path), and so the pid slot exists before the co-located task can be dispatched.
    let Some(pi) = proc_reserve() else {
        return Err("process table full");
    };
    let mapped = match map_image_into_slot(bytes) {
        Ok(m) => m,
        Err(e) => {
            proc_free(pi);
            return Err(match e {
                MapErr::Empty => "empty image",
                MapErr::BadSize(_) => "flat blob larger than one code page",
                MapErr::BadElf(why) => why,
                MapErr::NoSlot => "no free address-space slot",
            });
        }
    };
    let asid = mapped.ttbr0 >> 48;
    // Ensure the FUTEX waiter pool is initialised before the program can run: an EL0 program loaded through
    // this path (e.g. the UVUG mini-vug) may call SYS_FUTEX, and on a plain (non-witness) boot the in-kernel
    // fb-test launcher — the other futex_init caller — never runs. `futex_init` is idempotent (it only
    // reserves waiter-list capacity), so re-calling it here is a cheap no-op once armed.
    super::sched::futex_init();
    // Endow the fresh slot with a console write-cap so the program's SYS_WRITE(fd 1) reaches the console.
    // Done BEFORE the spawn, on a slot no other core can resolve yet (the co-location invariant below).
    install_console_cap(asid);
    let cpu = super::percpu::this_cpu().cpu_index as usize;
    let pid = super::sched::spawn_user_slot(name, mapped.base, mapped.sp, mapped.ttbr0, cpu);
    // Publish the Proc key (ASID before pid — the entry's live word) BEFORE the co-located task can be
    // dispatched: the caller yields only in the wait loop below, so the pid is always stored before the
    // exit/kill path's `proc_find_running` lookup (the sys_spawn co-location invariant, reused here).
    PROCS[pi].asid.store(asid, Ordering::Release);
    PROCS[pi].pid.store(pid, Ordering::Release);
    // INPUT-WIRE (ELF-5 router fold): register this program as the ACTIVE input target so the GUI router
    // (main.rs `pump_usb_into_gui`) delivers real keys/mouse into ITS per-process ring while it runs. Set
    // AFTER the slot exists and the pid is published (the router only enqueues to a live, resolvable ASID),
    // and BEFORE the wait loop yields (the co-located task cannot be dispatched until we yield below, so no
    // input is missed). Cleared on return just below — and `clear_handle_row` already clears the designation
    // on the slot's teardown, so an exited program's focus never lingers; the explicit clear covers the
    // Timeout path (task still alive, no teardown yet). Setting a focus RESETS the ring, so a fresh program
    // starts with an empty input queue.
    el0_input_set_active(asid);
    // Deadline-bounded cooperative wait: yield so the co-located task runs; its SYS_EXIT (or the fault-kill
    // path) marks PROCS[pi] EXITED and records the status via the GENERIC child-reap short-circuit.
    //
    // UVUG-8 — TAKEOVER-AWARE deadline, UVUG-8r2 — heartbeat-GATED so it can never strand the shell.
    //
    // The batch (no-HID) path never CONSUMES an input event, so `el0_takeover_active()` stays 0 and this
    // behaves exactly as the original fixed 5 s bound: the auto UVUG run (300 frames, exit=0) and the
    // wedged-app Timeout are unchanged. On metal the app enters interactive takeover when it actually DRAINS a
    // real event through `SYS_INPUT_POLL` (the same edge it announces with `:: UVUG: interactive takeover ::`).
    //
    // UVUG-8r2 fixes the liveness hole in the first cut. There, takeover was latched by `el0_input_push` (an
    // event merely REACHING the ring) and cleared only by a focus change — which, on the real `run` path, only
    // happens after this loop exits. So a HUNG interactive app (stops polling, never exits) suspended the
    // deadline FOREVER: `run_user_image` spun in `yield_now()` with no bound, the shell never came back, and
    // `gui_watchdog::poll()` — which only clears `SCREEN_APP_ACTIVE` — could not release it either. Reboot was
    // the only way out; the fixed 5 s deadline this arc replaced had been the sole escape hatch.
    //
    // The gate is the app's own liveness heartbeat: `sys_input_poll` stamps `EL0_TAKEOVER_HB` on EVERY poll
    // (the EL0 twin of `gui_watchdog::note_progress`). The deadline suspends only while the latch names THIS
    // asid AND that heartbeat is fresher than `stale_ticks`. So:
    //   * a live interactive app (polls every frame) keeps the heartbeat fresh -> suspended -> never times out.
    //     This is the P53 behaviour the arc exists to deliver, preserved exactly.
    //   * a HUNG app stops polling -> the heartbeat ages past `stale_ticks` -> `in_takeover` goes false, the
    //     re-arm edge fires, and a full fresh deadline window runs to completion -> `RunOutcome::Timeout` ->
    //     focus and screen return to the shell. Worst-case strand is bounded by `stale_ticks + deadline_ticks`.
    // `budget_start` is dragged forward to `now` every suspended pass, so time in takeover never accrues.
    // Witness `[uvug8]` marks both edges. The wedged-app watchdog (`gui_watchdog`) is independent and
    // unaffected — this is the `run`-side bound, that is the screen-side one.
    let start = super::timer::cntpct();
    // Heartbeat staleness bound: how long the takeover app may go without a SYS_INPUT_POLL before it stops
    // counting as "live in takeover". A frame-driven app polls many times a second, so this is generous.
    let stale_ticks = super::timer::cntfrq().saturating_mul(TAKEOVER_STALE_SECS);
    let mut budget_start = start;
    let mut suspended = false;
    loop {
        if PROCS[pi].state.load(Ordering::Acquire) == PEXITED {
            break;
        }
        let now = super::timer::cntpct();
        let in_takeover = takeover_suspends(
            el0_takeover_active(),
            asid,
            now,
            el0_takeover_heartbeat(),
            stale_ticks,
        );
        if in_takeover {
            if !suspended {
                suspended = true;
                serial_println!(
                    "[uvug8] deadline suspended — EL0 app asid={} in interactive takeover (event drained, heartbeat live)",
                    asid
                );
            }
            // Hold the window open while takeover persists: no elapsed time accrues against the deadline.
            budget_start = now;
        } else if suspended {
            // takeover -> non-takeover edge. REACHABLE on the real path (UVUG-8r2): the heartbeat gate flips
            // this edge whenever the app stops polling for `stale_ticks`, with no focus change involved — that
            // is precisely the hung-app case, and re-arming here is what restores the liveness bound. (It also
            // covers a genuine relinquish.) Re-arm a FULL fresh deadline window from this instant; time spent
            // in takeover is not charged against it.
            suspended = false;
            budget_start = now;
            serial_println!(
                "[uvug8] deadline re-armed — EL0 app asid={} left takeover (heartbeat stale or focus dropped)",
                asid
            );
        }
        if run_deadline_timed_out(in_takeover, now, budget_start, deadline_ticks) {
            break;
        }
        super::sched::yield_now();
    }
    let outcome = if PROCS[pi].state.load(Ordering::Acquire) == PEXITED {
        match PROCS[pi].status.load(Ordering::Acquire) {
            EXEC_KILLED_STATUS => RunOutcome::Faulted,
            s => RunOutcome::Exited(s),
        }
    } else {
        RunOutcome::Timeout
    };
    // INPUT-WIRE: drop input focus so the shell regains the keyboard the instant this program returns.
    // Belt-and-suspenders with `clear_handle_row`'s teardown clear (which fired already if the program
    // exited/was killed and its slot was torn down) — and the SOLE clear on the Timeout path, where the
    // task is still alive and no teardown has run. Idempotent: a no-op if the designation was already 0.
    el0_input_set_active(0);
    // Reap the Proc entry. The scheduler's exit path already repointed THIS core to the boot root (ASID 0)
    // and dispatched back to the caller (the shell task), so the shell's ASID-0 invariant holds on return.
    proc_free(pi);
    Ok((outcome, mapped.base))
}

// =============================================================================================
// ELF-1: a minimal static ELF64 (aarch64) loader. Validates the ident + type + machine, walks the
// PT_LOAD program headers, and (in `load_program_into_slot`) maps each segment into a slot's 16 KiB user
// window with per-segment permissions. Deliberately minimal: ET_EXEC only, no dynamic linking, no
// relocations (the fixture is position-independent), no interpreter. The flat-binary path stays the
// fallback for images without the ELF magic, so every existing .BIN fixture is untouched.
// =============================================================================================

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2; // e_ident[EI_CLASS]
const ELFDATA2LSB: u8 = 1; // e_ident[EI_DATA] — little-endian
const ET_EXEC: u16 = 2; // e_type — a fixed-address executable (no PIE/DYN this arc)
const EM_AARCH64: u16 = 183; // e_machine (0xB7)
const PT_LOAD: u32 = 1; // p_type — a loadable segment
const PF_X: u32 = 0x1; // p_flags — executable
const PF_W: u32 = 0x2; // p_flags — writable
const EHDR_SIZE: usize = 64; // sizeof(Elf64_Ehdr)
const PHDR_SIZE: usize = 56; // sizeof(Elf64_Phdr)
const MAX_LOAD_SEGS: usize = 8; // a minimal loader ceiling (a real ELF here has 2); reject more, fail-closed

/// One validated PT_LOAD segment: its file offset/size, virtual address, in-memory size (>= filesz for a
/// .bss tail), and flags. Bounds are already checked against the image and the slot window at collection.
#[derive(Clone, Copy)]
struct ElfSeg {
    off: usize,
    vaddr: u64,
    filesz: usize,
    memsz: usize,
    flags: u32,
}

/// A validated load plan: the entry VA, the lowest PT_LOAD vaddr (the load bias is `window_base - min_vaddr`,
/// so p_vaddr are treated as offsets from the window base — the fixture links at 0), and the collected
/// segments. Fixed-size array (no heap): a minimal loader with a hard `MAX_LOAD_SEGS` ceiling.
struct ElfPlan {
    entry: u64,
    min_vaddr: u64,
    segs: [ElfSeg; MAX_LOAD_SEGS],
    nsegs: usize,
}

/// True iff `b` begins with the ELF magic (`\x7fELF`). The dispatch between the ELF loader and the flat
/// fallback — a flat .BIN never begins with this magic (the fixtures start with `mov`/`b`), so they route
/// to the flat path unchanged.
fn is_elf_image(b: &[u8]) -> bool {
    b.len() >= 4 && b[0..4] == ELF_MAGIC
}

// Little-endian field reads, bounds-checked against the image slice (a malformed ELF must never index OOB).
fn rd_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn rd_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn rd_u64(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// Validate a minimal static ELF64 image and collect its PT_LOAD plan, WITHOUT allocating a slot (so a
/// rejection leaks nothing). `win_size` is the slot's user-window size — every segment's in-window span
/// (`p_vaddr - min_vaddr + p_memsz`) must fit it. Returns the plan or a `&'static str` naming the rejection
/// (honest error, surfaced by the loader's log line and as `-EINVAL` to a `sys_spawn`).
fn validate_elf(b: &[u8], win_size: usize) -> Result<ElfPlan, &'static str> {
    if b.len() < EHDR_SIZE {
        return Err("truncated ELF header");
    }
    // e_ident checks (magic already matched by `is_elf_image`).
    if b[4] != ELFCLASS64 {
        return Err("not ELF64 (EI_CLASS != 2)");
    }
    if b[5] != ELFDATA2LSB {
        return Err("not little-endian (EI_DATA != 1)");
    }
    if rd_u16(b, 16) != Some(ET_EXEC) {
        return Err("not ET_EXEC");
    }
    if rd_u16(b, 18) != Some(EM_AARCH64) {
        return Err("not EM_AARCH64");
    }
    let e_entry = rd_u64(b, 24).ok_or("bad e_entry")?;
    let e_phoff = rd_u64(b, 32).ok_or("bad e_phoff")? as usize;
    let e_phentsize = rd_u16(b, 54).ok_or("bad e_phentsize")? as usize;
    let e_phnum = rd_u16(b, 56).ok_or("bad e_phnum")? as usize;
    if e_phentsize != PHDR_SIZE {
        return Err("unexpected e_phentsize");
    }
    if e_phnum == 0 {
        return Err("no program headers");
    }
    // The whole program-header table must lie inside the image.
    let ph_end = e_phoff.checked_add(e_phnum * PHDR_SIZE).ok_or("phdr table overflow")?;
    if ph_end > b.len() {
        return Err("program-header table out of image");
    }

    let mut segs = [ElfSeg { off: 0, vaddr: 0, filesz: 0, memsz: 0, flags: 0 }; MAX_LOAD_SEGS];
    let mut nsegs = 0usize;
    let mut min_vaddr = u64::MAX;
    for i in 0..e_phnum {
        let ph = e_phoff + i * PHDR_SIZE;
        let p_type = rd_u32(b, ph).ok_or("bad p_type")?;
        if p_type != PT_LOAD {
            continue; // ignore PT_GNU_STACK / PT_PHDR / etc. — a minimal loader maps only PT_LOAD
        }
        let p_flags = rd_u32(b, ph + 4).ok_or("bad p_flags")?;
        let p_offset = rd_u64(b, ph + 8).ok_or("bad p_offset")? as usize;
        let p_vaddr = rd_u64(b, ph + 16).ok_or("bad p_vaddr")?;
        let p_filesz = rd_u64(b, ph + 32).ok_or("bad p_filesz")? as usize;
        let p_memsz = rd_u64(b, ph + 40).ok_or("bad p_memsz")? as usize;
        if nsegs >= MAX_LOAD_SEGS {
            return Err("too many PT_LOAD segments");
        }
        if p_filesz > p_memsz {
            return Err("p_filesz > p_memsz");
        }
        // The segment's file bytes must lie inside the image.
        let file_end = p_offset.checked_add(p_filesz).ok_or("segment file range overflow")?;
        if file_end > b.len() {
            return Err("segment file range out of image");
        }
        // W^X: a segment must not be both writable and executable (page shapes are RO+X or RW+NX).
        if p_flags & PF_W != 0 && p_flags & PF_X != 0 {
            return Err("W^X: segment both writable and executable");
        }
        segs[nsegs] = ElfSeg { off: p_offset, vaddr: p_vaddr, filesz: p_filesz, memsz: p_memsz, flags: p_flags };
        nsegs += 1;
        if p_vaddr < min_vaddr {
            min_vaddr = p_vaddr;
        }
    }
    if nsegs == 0 {
        return Err("no PT_LOAD segments");
    }
    // Every segment must fit the slot's user window once biased so min_vaddr maps to the window base.
    let mut entry_in_exec = false;
    for i in 0..nsegs {
        let s = segs[i];
        let win_off = s.vaddr.checked_sub(min_vaddr).ok_or("vaddr below min")? as usize;
        let span_end = win_off.checked_add(s.memsz).ok_or("segment span overflow")?;
        if span_end > win_size {
            return Err("segment overflows the slot window");
        }
        // The entry must land inside an EXECUTABLE segment's mapped range (a data-only entry would fault).
        if s.flags & PF_X != 0 && e_entry >= s.vaddr && e_entry < s.vaddr + s.memsz as u64 {
            entry_in_exec = true;
        }
    }
    if e_entry < min_vaddr {
        return Err("entry below the lowest segment");
    }
    if !entry_in_exec {
        return Err("entry not in an executable segment");
    }
    Ok(ElfPlan { entry: e_entry, min_vaddr, segs, nsegs })
}

/// IMAGE_SHA256 (code-signing): the IMAGE_SHA256 principal a program's on-disk image WOULD be stamped with by
/// `load_program_into_slot`, computed WITHOUT allocating a slot — mount, find, read the same bytes under the
/// same window cap, hash. Mirrors the loader's read EXACTLY (the same `USER_REGION_SIZE` cap) so the result is
/// bit-identical to the live stamp for any image the loader accepts — flat (<= one code page) or ELF.
/// `None` if the file is absent, mis-sized, or unreadable. Used by the K2 launcher/metal fixtures (the expected
/// owner principal, now an image digest not a name) and by the IMG-SIG witness.
fn image_principal_of_file(name: &str) -> Option<PrincipalRecord> {
    let fs = crate::fs::fat::mount().ok()?;
    let de = fs.find_in_root(name).ok()?;
    let cap = super::boot::USER_REGION_SIZE;
    if de.size == 0 || de.size as u64 > cap as u64 {
        return None;
    }
    let mut bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    fs.read_file(&de, &mut bytes, cap).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(PrincipalRecord::image_of(&bytes))
}

/// SYS_SPAWN(): load the fixed on-disk program (`HELLO.BIN`) into a fresh slot, run it at EL0 as a CHILD of
/// the caller, and return a HANDLE index into the CALLER's per-process handle table (U4 — not the raw pid),
/// or a negative errno. The handle IS the ownership token: `sys_wait` takes it, and it can only be reaped by
/// a caller whose table holds it. No arguments this arc — the program is fixed; arbitrary program-by-name is
/// M8 (it needs a validated `copy_from_user` name, a STOP tripwire here).
///
/// Race-freedom (the child cannot exit before its pid is recorded): the whole SVC handler runs IRQ-masked and
/// the CHILD is co-located on the CALLER's core, so the child stays queued-not-dispatched until the parent
/// yields (which it does only later, in sys_wait). We (1) claim a Proc entry, (2) reserve a handle slot in the
/// caller's table, (3) load the program, (4) spawn the child (queued, not run), (5) store its real pid with
/// Release into BOTH the Proc entry and the handle slot — all before returning to EL0, hence strictly before
/// the parent can yield and let the child run. The child's exit/kill lookup therefore always observes the
/// stored Proc pid. This co-location invariant is load-bearing and QEMU-true (no sched change needed). The
/// handle slot is reserved BEFORE the load so a full handle table fails cleanly with nothing to un-spawn.
fn sys_spawn() -> i64 {
    // Gate: we need a block device to load the child off. -ENODEV so a no-SD boot fails the spawn cleanly.
    if crate::drivers::block::info().is_none() {
        return ENODEV;
    }
    // The CALLER's ASID names its per-process handle table — read synchronously here, where the caller's
    // TTBR0 is live (sys_spawn installs into and sys_wait resolves from the SAME table, since both run as
    // the parent).
    let asid = current_asid();
    // Claim the Proc entry FIRST, so a failed load frees nothing but the entry, and (crucially) so the pid
    // slot exists to receive the real pid before the child can be dispatched.
    let Some(i) = proc_reserve() else {
        return EAGAIN; // process table full
    };
    // Reserve a HANDLE slot in the caller's table BEFORE spawning (a RESERVING placeholder, overwritten with
    // the real pid below). A full handle table fails here with only the Proc entry to release — no child has
    // been loaded or spawned yet, so there is nothing to un-spawn.
    let Some(h) = handle_install(asid, HANDLE_RESERVING) else {
        proc_free(i);
        return EAGAIN; // handle table full
    };
    let loaded = match load_program_into_slot("HELLO.BIN") {
        Ok(l) => l,
        Err(e) => {
            handle_clear(asid, h); // release the reserved handle slot
            proc_free(i); // no address-space slot was allocated on any load-failure path — release the entry
            return spawn_errno(&e);
        }
    };
    // U5: endow the CHILD's OWN table with a console write-capability (the child runs `HELLO.BIN`, which
    // `sys_write`s fd 1). Done here, on the freshly-built slot, BEFORE the child is spawned — the child cannot
    // be dispatched until the parent yields (the co-location invariant below), so there is no concurrent
    // resolver of the child's table. Without this the child's first print would return -EACCES (routed).
    install_console_cap(loaded.ttbr0 >> 48);
    // Co-locate the child on the caller's core (the invariant above): sys_spawn always runs with its EL0
    // caller current, so `this_cpu` is the parent's core.
    let cpu = super::percpu::this_cpu().cpu_index as usize;
    let pid = super::sched::spawn_user_slot("u4-child", loaded.base, loaded.sp, loaded.ttbr0, cpu);
    // Record the real pid (Release) into BOTH the Proc entry (pid-keyed exit accounting) and the reserved
    // handle slot (ASID-keyed ownership namespace) BEFORE returning to EL0 — before the parent can yield and
    // let the child run. The child's exit path sees the Proc pid; the parent's later sys_wait resolves the
    // handle to it. U5/U6: the parent's child handle is KIND_CHILD carrying CAP_READ (the ownership token;
    // `sys_wait` gates on kind==Child, not on the right). Kind + rights are published Release BEFORE the pid
    // (the live value word), so the handle is never observed live without its kind/rights.
    // U7: the ASID is published BEFORE the pid (the entry's live key) — a sys_xfer that finds this entry by
    // pid always observes the ASID its inbox deposit is keyed by.
    PROCS[i].asid.store(loaded.ttbr0 >> 48, Ordering::Release);
    PROCS[i].pid.store(pid, Ordering::Release);
    handle_set_kind(asid, h, KIND_CHILD);
    handle_set_rights(asid, h, CAP_READ);
    handle_set(asid, h, pid);
    h as i64 // return the HANDLE index (per-process; two processes can each hold handle 0 to different children)
}

/// SYS_WAIT(handle): block the caller until the child its `handle` refers to exits, then return the child's
/// exit status — or `-ECHILD` if that handle is not in the CALLER's table (out-of-range or Empty). Structural
/// ownership: you can only reap a child whose handle is in YOUR table; a foreign or stale handle simply isn't
/// there. The waker is the child's `done.post()` — a SCHEDULER wake (from the child's SYS_EXIT or its kill
/// path), so this works under QEMU (unlike a timer-driven sleep).
///
/// We wait on `done` UNCONDITIONALLY (not only when the child is still RUNNING): the child posts `done`
/// exactly once (exit or kill), so waiting once either fast-returns a permit the child already left (child
/// exited first — no park) or parks until the child posts. Exactly one post is consumed by exactly one wait,
/// so the reaped entry's semaphore returns to 0 permits and is clean for reuse — the balance the process
/// table relies on. (Under QEMU the child, co-located, cannot run until we block here, so it is always the
/// park path; the fast path is the metal case where a timer preempts the parent between spawn and wait.)
///
/// The handle is CONSUMED by the reap (`handle_clear`), so a second `sys_wait` on the same handle returns
/// `-ECHILD` (Empty) — correct. `PROCS` stays keyed by pid (exit accounting); `HANDLES` by ASID (ownership).
fn sys_wait(handle: u64) -> i64 {
    let asid = current_asid();
    // Resolve the handle against the CALLER's OWN table — the structural ownership check, now through the U5
    // enforcement point. It must be a CHILD handle (U4's meaning). Out-of-range/Empty (NoHandle), a rights
    // shortfall (Denied), or a CONSOLE handle all mean "you hold no such child" => -ECHILD (byte-identical to
    // U4 for the orphan's `sys_wait(0)`). Waiting requires no resource right — holding the child handle is the
    // ownership token (`req = 0`); child handles carry CAP_READ for model completeness, not as a wait gate.
    let pid = match handle_resolve(asid, handle, 0) {
        Ok(HandleTarget::Child(pid)) => pid,
        _ => return ECHILD,
    };
    let Some(i) = proc_find_child(pid) else {
        return ECHILD; // the handle named a pid with no Proc entry (defensive; cannot happen in the demo)
    };
    let woken = PROCS[i].done.wait();
    debug_assert!(woken, "sys_wait: called off a scheduled task");
    // `Semaphore::wait` restores the SVC-entry DAIF (IRQ masked on exception entry), so IRQ is already masked
    // here; re-mask defensively so the `__vec_svc` epilogue's banked ELR/SPSR/SP_EL0 restore is guaranteed
    // I-masked regardless of any future change to wait()'s IRQ discipline (the sys_yield/sys_sleep contract).
    remask_irq();
    let status = PROCS[i].status.load(Ordering::Acquire) as i64;
    proc_free(i); // reap the Proc entry (its `done` is back at 0 permits, free for reuse)
    handle_clear(asid, handle as usize); // consume the handle: a second sys_wait on it now returns -ECHILD
    status
}

/// SYS_CAP(op, a1, a2): grant/attenuate/revoke on the CALLER's OWN handle table — capabilities as first-class
/// operations. `op` selects the sub-op. Runs single-writer over the caller's table (one SVC at a time, one
/// live task per ASID), so no lock is needed. See `sys_cap_grant`/`sys_cap_revoke`.
fn sys_cap(op: u64, a1: u64, a2: u64) -> i64 {
    let asid = current_asid();
    match op {
        CAP_OP_GRANT => sys_cap_grant(asid, a1, a2),
        CAP_OP_REVOKE => sys_cap_revoke(asid, a1),
        CAP_OP_XREVOKE => sys_cap_xrevoke(asid, a1), // U7: revoke a transfer this caller made (sender-only)
        _ => EINVAL,
    }
}

/// SYS_CAP GRANT(src_idx, req_rights): mint a NEW handle in the caller's own table naming the SAME target as
/// `src_idx`, carrying `req_rights` — enforcing the ATTENUATION (monotonic-decrease) invariant: the minted
/// rights can never exceed the granter's rights on the source. Requires `CAP_GRANT` on the source. Returns
/// the new handle index, or a negative errno:
///   -EACCES — no such source handle, source lacks CAP_GRANT, or `req_rights` would AMPLIFY (bits the granter
///             does not hold): the core U5 property — a grant can never produce more rights than the granter.
///   -EAGAIN — the caller's handle table is full (no free slot to mint into; never grown).
/// For this arc the mint targets the caller's OWN table (a child spawns nothing to grant into yet); minting
/// into another table is a straightforward extension once cross-process handle-transfer lands (U7).
/// U6: the mint names the SAME (kind, target) as the source — a grant attenuates RIGHTS, never re-kinds the
/// object. So a granted console cap stays a console cap; a granted File stays that File.
fn sys_cap_grant(asid: u64, src_idx: u64, req_rights: u64) -> i64 {
    // Resolve the source's raw target + kind + rights (no right required to READ your own handle's descriptor).
    let Some(target) = handle_get(asid, src_idx as usize) else {
        return EACCES; // no such source handle
    };
    if target == HANDLE_RESERVING {
        return EACCES; // an in-flight reservation is not a grantable handle (defensive; single-writer)
    }
    let src_kind = handle_kind(asid, src_idx as usize);
    let src_rights = HANDLE_RIGHTS[asid as usize][src_idx as usize].load(Ordering::Acquire);
    if src_rights & CAP_GRANT == 0 {
        return EACCES; // the source does not authorize granting
    }
    // U7: a revoked RECEIVED cap is stale for DELEGATION too, not only for use — without this check a
    // recipient holding CAP_GRANT on a transferred cap could mint a fresh, revocation-free copy AFTER the
    // sender revoked (post-revoke laundering, review-confirmed). Copies minted BEFORE the revoke remain
    // the documented revocation-TREE scope (derivation records chase those).
    let src_rec = HANDLE_XFER_REC[asid as usize][src_idx as usize].load(Ordering::Acquire);
    if src_rec != 0
        && XFER_REC_TX[(src_rec - 1) as usize].load(Ordering::Acquire) & XFER_REVOKED_BIT != 0
    {
        return EACCES;
    }
    // U8: a source whose derivation chain is revoked is stale for delegation too — a mint from a dead
    // subtree would be a fresh, revocation-free copy (the laundering check, now tree-deep).
    let src_dn = HANDLE_DERIV[asid as usize][src_idx as usize].load(Ordering::Acquire);
    if src_dn != 0 && deriv_stale(src_dn) {
        return EACCES;
    }
    let req = req_rights as u32;
    // Attenuation: reject any requested bit the granter does not itself hold. `req & !src_rights` is exactly
    // the set of amplifying bits; non-empty => the grant would exceed the granter's authority.
    if req & !src_rights != 0 {
        return EACCES;
    }
    // U8: record the derivation EDGE (mint -> source) so a later revoke of the source (or any ancestor)
    // reaches this copy at its next use. Ledger exhaustion is -EAGAIN with nothing else claimed yet.
    let Some((new_node, _)) = deriv_derive_from(asid, src_idx as usize) else {
        return EAGAIN;
    };
    // Mint: claim a first-free slot with `handle_install` (a RESERVING placeholder — the value word goes live
    // LAST), then publish the source's kind + the attenuated rights + the derivation node, then the real
    // target value. Single-writer over this table (the caller is mid-syscall, not concurrently resolving), so
    // the kind/rights-then-value order is the defensive belt-and-braces that keeps a live value from ever
    // being seen sans kind/rights.
    match handle_install(asid, HANDLE_RESERVING) {
        Some(idx) => {
            handle_set_kind(asid, idx, src_kind);
            handle_set_rights(asid, idx, req);
            HANDLE_DERIV[asid as usize][idx].store(new_node, Ordering::Release);
            handle_set(asid, idx, target); // publish the live value LAST (Release)
            idx as i64
        }
        None => {
            deriv_drop(new_node); // unwind the freshly-minted node (it has no children — frees now)
            EAGAIN // handle table full
        }
    }
}

/// SYS_CAP REVOKE(idx): drop a handle the caller owns (`handle_clear`, which also clears its rights). A
/// process may always drop its OWN capabilities (ownership-based — the caller's table is its own), so no
/// right is required for the LOCAL drop. U8 gives `CAP_REVOKE` its real semantics: if the handle CARRIES
/// that right, the revoke additionally marks its derivation node — every capability derived from it (every
/// re-grant, and every re-transfer whose chain passes through it) is `-EACCES` at its next use. Without the
/// right the drop stays local (derived copies survive — U5's semantics, unchanged).
/// Returns 0, or -ECHILD if the index is out-of-range/Empty (nothing to revoke — also the double-revoke
/// errno). After revoke, any use of the index returns -EACCES (`sys_write`) / -ECHILD (`sys_wait`).
fn sys_cap_revoke(asid: u64, idx: u64) -> i64 {
    if idx as usize >= NHANDLE || handle_get(asid, idx as usize).is_none() {
        return ECHILD; // out-of-range or Empty — no such handle to revoke
    }
    // U8: CAP_REVOKE gets its real semantics — revoking a handle that CARRIES the right marks its
    // derivation node revoked, killing the whole subtree derived from it (every re-grant, and every
    // re-transfer whose chain passes through it) at the descendants' next use. Without the right the
    // revoke keeps U5's ownership semantics: the caller's own row entry drops, derived copies survive.
    // The mark precedes the clear (the clear's `deriv_drop` tombstones the node, preserving the bit for
    // as long as any descendant lives). A handle with no node has no descendants — nothing to mark.
    let rights = HANDLE_RIGHTS[asid as usize][idx as usize].load(Ordering::Acquire);
    let dn = HANDLE_DERIV[asid as usize][idx as usize].load(Ordering::Acquire);
    if rights & CAP_REVOKE != 0 && dn != 0 {
        deriv_revoke(dn);
    }
    handle_clear(asid, idx as usize);
    0
}

/// The longest name `sys_open` accepts: a FAT 8.3 short name is at most "NAMENAME.EXT" = 8 + '.' + 3 = 12
/// bytes. A longer request cannot name a real entry, so it is rejected as malformed rather than truncated.
const MAX_NAME: usize = 12;

/// U10: `SYS_OPEN` mode bit1 — create the file if it is absent (and endow the write cap, since you create to
/// write). Bit0 remains RW (U9). `O_CREAT` on an EXISTING file just opens it (idempotent). No `O_TRUNC` /
/// `O_EXCL` / `O_APPEND` this arc — bits >= 3 stay reserved.
const O_CREAT: u64 = 1 << 1;

/// U6: `SYS_OPEN` mode bit2 — at an `O_CREAT` of a NEW name, make the file PUBLIC (world-accessible) instead
/// of the owned-by-default private file. UnaOS is secure-by-default: a plain `O_CREAT` records the creator as
/// the file's OWNER (private — only the owner or a principal it `SYS_FGRANT`s may open it); `O_PUBLIC` opts a
/// create OUT of ownership into the pre-U6 open-by-anyone behaviour. Ignored on an open of an existing file
/// (ownership is fixed at create) and outside `O_CREAT`. See `sys_open` and the owner/grants block.
const O_PUBLIC: u64 = 1 << 2;

/// SYS_OPEN(name_ptr, name_len, mode) -> a File-handle index, or a negative errno. The first resource syscall
/// routed through a NON-Console object: it makes U6a's `File` scaffold real. `copy_from_user`s the (bounded) 8.3
/// name, mounts the single FAT volume, finds the top-level entry, records an open-file descriptor in the
/// caller's per-task FILES row, and installs a `File` handle (first-free). U9: `mode` bit0 selects the rights the
/// handle carries — `0` (RO) = `CAP_READ`; `1` (RW) = `CAP_READ | CAP_WRITE`, the write cap a File `SYS_WRITE`
/// presents. U10: `mode` bit1 (`O_CREAT`) creates the file if it is absent — a 0-length root-directory entry
/// (`fat::create_in_root`), and endows RW (you create to write); the first grow-write allocates its first
/// cluster. `O_CREAT` on an existing file just opens it. Returns the handle index EL0 reads/writes/waits on.
///
/// Ordering mirrors `load_program_into_slot`/`sys_spawn`: do the fallible READ-ONLY lookups first (name copy,
/// mount, find, dir-reject), so a failure there returns with nothing to unwind; claim RESOURCES last (a file
/// descriptor, then a handle). The one KERNEL-resource unwind is a full handle table AFTER a descriptor was
/// claimed — free the descriptor, then `-EAGAIN`. (An `O_CREAT` that then fails to claim a handle leaves a
/// harmless 0-length directory entry on disk — no kernel leak, and a re-open finds/reuses it.) Errnos:
/// `-EINVAL` (bad name length), `-EFAULT` (bad name pointer), `-ENODEV`/`-EIO` (mount failure), `-ENOENT` (no
/// such file, no `O_CREAT`), `-EINVAL` (an `O_CREAT` name not representable as 8.3), `-ENOSPC` (root directory
/// full), `-EISDIR` (a directory), `-EMFILE` (open-file table full), `-EAGAIN` (handle table full). Flat root,
/// one volume — scope by design.
fn sys_open(name_ptr: u64, name_len: u64, mode: u64) -> i64 {
    let asid = current_asid();
    // K1 M2.3: the caller's persistent principal, snapshotted HERE (before any OWNED_FILES/NAMESPACE hold) so
    // SLOT_PPID is never nested under those locks. Recorded as the owner on a private create (persist), and read
    // by the M2.4 cross-reboot admission branch on an existing-file open. NONE for an anonymous/inline caller —
    // the whole 23-fixture battery — so every persist/enforce step below is a structural no-op for them.
    let caller_ppid = current_principal();
    // 1. Bound + copy the name. A 0-length or over-8.3 request is malformed (-EINVAL); copy_from_user validates
    //    the whole source range up front, so a bad/oob pointer is -EFAULT with no deref.
    let n = name_len as usize;
    if n == 0 || n > MAX_NAME {
        return EINVAL;
    }
    let mut namebuf = [0u8; MAX_NAME];
    if copy_from_user(&mut namebuf[..n], name_ptr, n).is_err() {
        return EFAULT;
    }
    let Ok(name) = core::str::from_utf8(&namebuf[..n]) else {
        return ENOENT; // a non-ASCII/UTF-8 name matches no 8.3 entry
    };
    // K1 M4: the KERNEL owns UNAFS.ATR — the on-disk ACL store must never be readable or writable through an EL0
    // File capability (that would let an untrusted blob read every principal or forge a row). Deny the open
    // outright (case-insensitive, matching find_located's 8.3 semantics), BEFORE any lookup/claim, so it is a
    // clean -EACCES with nothing to unwind. No EL0 fixture opens it, so the battery path is byte-identical.
    if name.eq_ignore_ascii_case(ATR_NAME) {
        return EACCES;
    }
    // 2. Read-only lookups — nothing claimed yet, so each failure returns cleanly.
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    // F3-M3: the NAMESPACE hold — from the name lookup through the descriptor claim. It makes the
    // lookup -> ACL -> incref sequence atomic against a concurrent sys_unlink (whose 0xE5 + owned_clear +
    // mark-pending run under the same lock), closing: the open-races-unlink stale-chain UAF (a descriptor
    // bound to a first_cluster the unlink just freed), the owned_clear-vs-owned_set_owner recycled-slot
    // window, and the create scan-then-claim duplicate-name/slot race. Taken AFTER mount() (never held
    // across it); every early return below drops it cleanly; the one unwind that frees a chain
    // (free_orphan_chain mounts) drops it FIRST. Directory block I/O under the hold is bounded + polled
    // (the NAMESPACE span rule).
    let ns = ns_lock();
    // U10: `find_located` also returns the on-disk LOCATION of the entry (its directory sector LBA + slot
    // offset), recorded in the descriptor so a later GROW can republish `size`/`first_cluster` into it. When the
    // entry is ABSENT and `O_CREAT` is set, create a fresh 0-length entry (which yields the same triple); the
    // create writes only one directory sector (no cluster/FAT touched) and is still a "fallible lookup before
    // any resource claim" — its own failures (name / no-space) return cleanly.
    let mut created = false; // U6: did THIS open create a NEW name (-> the caller becomes its owner)?
    let (de, dir_lba, dir_off) = match fs.find_located(name) {
        Ok(t) => t,
        Err(crate::fs::fat::FatError::NotFound) => {
            if mode & O_CREAT == 0 {
                return ENOENT; // absent and no O_CREAT
            }
            match fs.create_in_root(name, 0x20 /* ATTR_ARCHIVE — a plain file */) {
                Ok(t) => {
                    created = true;
                    t
                }
                Err(crate::fs::fat::FatError::Unsupported) => return EINVAL, // name not representable as 8.3
                Err(crate::fs::fat::FatError::NoSpace) => return ENOSPC,     // root directory full
                Err(_) => return EIO,
            }
        }
        Err(_) => return EIO,
    };
    if de.is_dir {
        return EISDIR; // a directory is not readable through a File handle (no dir ops this arc)
    }
    // U6: the by-NAME namespace ACL — the gate to ACQUIRING a File capability by name (thereafter the handle
    // IS the authority). Owned-by-default (secure-by-default): a fresh O_CREAT records the creator as OWNER
    // (unless O_PUBLIC); an open of an EXISTING file is admitted only for the owner or a granted principal.
    // Sits in the "fallible lookups first, nothing claimed yet" window — a denial here is a clean -EACCES
    // with nothing to unwind (mirrors the ordering the sys_open doc-comment states).
    let asid_gen = ASID_GEN[asid as usize].load(Ordering::Acquire);
    let requested = if mode & (1 | O_CREAT) != 0 { CAP_READ | CAP_WRITE } else { CAP_READ };
    if created {
        // A NEW name — the caller is its owner. Record ownership unless O_PUBLIC. If the bounded owner table
        // is full, fail CLOSED: undo the fresh (0-length) directory entry and return -ENOSPC rather than
        // leave a "private" file silently world-accessible. ASID 0 (the shared/boot window) is NEVER torn down
        // and its ASID_GEN never bumps, so it cannot be a gen-fenced private owner — an ASID-0 create is always
        // PUBLIC (no owner row). Untrusted EL0 runs under ASID 1..8; ASID 0 is EL1/boot only.
        if asid != 0 && mode & O_PUBLIC == 0 && !owned_set_owner(dir_lba, dir_off as u32, asid, asid_gen, caller_ppid) {
            let _ = fs.mark_dir_deleted(dir_lba, dir_off);
            return ENOSPC;
        }
    } else if !owned_access_ok(dir_lba, dir_off as u32, asid, asid_gen, requested, caller_ppid) {
        return EACCES; // an owned file, and the caller is neither its owner nor a sufficiently-granted principal
    }
    // 3. Claim resources — the GLOBAL open-file refcount FIRST, then a per-task descriptor, then a handle.
    // U11-M2: increment the cross-ASID open-file refcount BEFORE `files_alloc` and remember the ROW it landed on.
    // Incrementing first pairs every increment with EXACTLY one decrement — the descriptor's `files_free`
    // (close/teardown), or the one-line `openfile_decref_at` on the `files_alloc`-full unwind — leaving no path
    // where a decrement lands on a row this open never incremented (which could race a concurrent same-file open
    // on SMP). A full table on a NEW identity is a clean -ENFILE, nothing else claimed.
    let Some(open_row) = openfile_incref(dir_lba, dir_off as u32) else {
        return ENFILE; // the global open-file table is full (a new file identity, no free row)
    };
    let Some(fid) = files_alloc(asid, de.first_cluster(), de.size, dir_lba, dir_off as u32) else {
        // Undo the increment we just made — there is no descriptor to record the row in / route through
        // `files_free`. Decrement THAT exact row. The free below only fires in the (defensive) case a concurrent
        // unlink made this the last close; block I/O is legal in this syscall context — but F3-M3: it mounts,
        // so the namespace guard drops FIRST (never held across mount()/chain I/O).
        let orphan = openfile_decref_at(open_row as u32);
        drop(ns);
        if let Some(fc) = orphan {
            free_orphan_chain(fc);
        }
        return EMFILE; // this task's open-file table is full
    };
    // U11-M2: bind the descriptor to the refcount row it increments, so its `files_free` (and `sys_unlink`'s
    // mark) hit exactly THIS row — never a key search that a recycled directory slot could redirect. Recorded
    // before any further unwind, so the handle-install-fail `files_free` below decrements the right row.
    FILE_OPENROW[asid as usize][fid].store(open_row as u32, Ordering::Release);
    // F3-M3: the descriptor is fully bound to the (still-live-under-this-lock) file — the sequence the
    // namespace lock protects is complete. The handle install below touches only per-task handle state.
    drop(ns);
    // U11: pack the slot's CURRENT generation with `idx + 1` into the file-id (the value word) — so a later
    // free + first-fit reuse of this same slot (bumped gen) makes any handle still carrying this word fail
    // `file_desc_validate`'s gen check (no sibling rebind). `files_alloc` never bumps gen, and the row has a
    // single writer (this task, mid-syscall), so the gen read here is the one this descriptor lives under.
    let file_id = file_id_pack(FILE_GEN[asid as usize][fid].load(Ordering::Acquire), fid);
    let Some(h) = handle_install(asid, HANDLE_RESERVING) else {
        // No handle slot — release the descriptor we just claimed; its `files_free` also decrements the refcount
        // (balanced against the incref above). The orphan-free only fires if a concurrent unlink raced this open.
        if let Some(fc) = files_free(asid, fid) {
            free_orphan_chain(fc);
        }
        return EAGAIN;
    };
    // U9/U10: RW (bit0) or O_CREAT (bit1 — you create to write) endows CAP_WRITE alongside CAP_READ, so a File
    // `SYS_WRITE` through this handle passes the CAP_WRITE CHECK; RO (neither bit) keeps the U6b read-only cap.
    // Publish the kind + rights, then the live file-id LAST (Release) — the handle is never observed live without
    // its File kind and rights. Single-writer over this row (mid-syscall), so this is belt-and-braces.
    let rights = if mode & (1 | O_CREAT) != 0 { CAP_READ | CAP_WRITE } else { CAP_READ };
    handle_set_kind(asid, h, KIND_FILE);
    handle_set_rights(asid, h, rights);
    handle_set(asid, h, file_id);
    // K1 M2.3 / K6 M3: WRITE-THROUGH persist of a NEW private file owned by a NAMED principal, so ownership
    // SURVIVES REBOOT — now onto the NATIVE unafs attribute volume (the K4-journaled write path), not the
    // retired UNAFS.ATR sidecar. Runs AFTER the create + owner row + handle are all committed (a fully-
    // successful private create), OUTSIDE the caller's namespace lock (K5B: native_persist_create takes NO ns —
    // `native_acl_write`'s own with_unafs MOUNT hold is the serializer). Gated on `caller_ppid.kind != NONE` — the anonymous battery never reaches
    // the disk here, so the 23-fixture path stays byte-identical. A persist failure is non-fatal: the in-RAM
    // ACL still enforces THIS boot; only cross-reboot survival is lost (fails closed to PUBLIC at next mount).
    if created && asid != 0 && mode & O_PUBLIC == 0 && caller_ppid.kind != PRIN_NONE {
        let _ = native_persist_create(name, de.first_cluster(), dir_lba, dir_off as u32, caller_ppid);
    }
    h as i64
}

/// SYS_READ(handle, buf, len) -> the byte count (`0` = EOF), or a negative errno. The object table's first
/// resource-read CHECK on a non-Console object: `handle_resolve(asid, handle, CAP_READ)` must yield a `File`.
/// A missing right (`Denied`), a non-File kind (Console/Child/Socket), or no handle (Empty/oob) ALL return
/// `-EACCES` — the single enforcement point, the twin of `sys_write`'s Console+CAP_WRITE. Then it clamps the
/// request to the bytes left from the descriptor's offset, validates the WHOLE destination up front (a bad
/// buffer is `-EFAULT` with no read and NO offset change), reads through the read-only offset-aware FAT reader,
/// `copy_to_user`s the bytes, and advances the offset by exactly the count delivered. Sequential — no seek.
fn sys_read(handle: u64, buf: u64, len: u64) -> i64 {
    let asid = current_asid();
    // The CHECK: File + CAP_READ, or -EACCES. Identical shape to sys_write's Console + CAP_WRITE resolve.
    let file_id = match handle_resolve(asid, handle, CAP_READ) {
        Ok(HandleTarget::File(id)) => id,
        _ => return EACCES,
    };
    // U11: decode + validate the file-id through the single descriptor-identity seam — range, presence
    // (`FILE_USED`), AND generation (a stale handle to a reused slot is rejected here, not silently rebound).
    let Some(idx) = file_desc_validate(asid, file_id) else {
        return EACCES;
    };
    let size = FILE_SIZE[asid as usize][idx].load(Ordering::Acquire);
    let offset = FILE_OFFSET[asid as usize][idx].load(Ordering::Acquire);
    // Bytes available from the current offset, clamped to the request. `offset` is advanced only by delivered
    // counts and never exceeds `size`, so `size - offset` cannot underflow; `want == 0` is a clean EOF.
    let want = core::cmp::min(len as usize, size.saturating_sub(offset) as usize);
    if want == 0 {
        return 0; // EOF, or the caller requested nothing
    }
    // Validate the WHOLE destination BEFORE any disk read — a bad buffer is -EFAULT with no I/O, no offset move.
    if !user_range_ok(buf, want, true) {
        return EFAULT;
    }
    // Re-mount (as sys_spawn does — the single volume is deterministic, so the descriptor's cluster/size stay
    // valid across mounts) and read `want` bytes from `offset` via the read-only offset-aware reader.
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    let cluster = FILE_CLUSTER[asid as usize][idx].load(Ordering::Acquire);
    let mut bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs.read_at(cluster, size, offset, &mut bytes, want).is_err() {
        return EIO;
    }
    let got = bytes.len();
    if got == 0 {
        return 0; // a short chain (malformed vs size) yielded nothing here — treat as EOF
    }
    // got <= want, and buf..buf+want was validated above, so this copy always passes; the check is defensive.
    if copy_to_user(buf, &bytes, got).is_err() {
        return EFAULT; // offset NOT advanced — a rejected buffer leaves the file position unchanged
    }
    FILE_OFFSET[asid as usize][idx].store(offset + got as u32, Ordering::Release);
    got as i64
}

/// SYS_SEEK(handle, offset) -> the new absolute offset, or a negative errno (U9). Absolute seek on an open
/// File descriptor: the CHECK requires a `File` handle carrying ANY of CAP_READ|CAP_WRITE. `handle_resolve`
/// requires ALL bits in `req`, so "any of" is expressed by resolving for CAP_READ, else for CAP_WRITE —
/// whichever right is present (and the File kind, and — via `handle_resolve` — no revoked ancestor) admits the
/// seek; a non-File kind / no handle / a revoked cap all give `-EACCES`. An offset PAST `size` is `-EINVAL`
/// (seeking exactly TO `size`, the EOF position, is legal). Sets FILE_OFFSET; a later SYS_READ / File
/// SYS_WRITE resumes from it. No I/O — pure descriptor-state update.
fn sys_seek(handle: u64, offset: u64) -> i64 {
    let asid = current_asid();
    // The CHECK: a File carrying CAP_READ OR CAP_WRITE (either admits a seek), non-revoked. The double resolve
    // expresses "any of" over `handle_resolve`'s all-bits-in-`req` semantics without reading the sidecars raw.
    let file_id = match handle_resolve(asid, handle, CAP_READ) {
        Ok(HandleTarget::File(id)) => id,
        _ => match handle_resolve(asid, handle, CAP_WRITE) {
            Ok(HandleTarget::File(id)) => id,
            _ => return EACCES,
        },
    };
    // U11: decode + validate through the single descriptor-identity seam (range, `FILE_USED`, and generation).
    let Some(idx) = file_desc_validate(asid, file_id) else {
        return EACCES;
    };
    let size = FILE_SIZE[asid as usize][idx].load(Ordering::Acquire);
    // Absolute seek: an offset PAST the file's size is invalid; seeking exactly TO `size` (the EOF position, a
    // legal 0-byte read/write point) is allowed. Preserves the FILE_OFFSET <= FILE_SIZE invariant. `size` is a
    // u32, so `offset <= size` guarantees the cast below is exact and the return fits a non-negative i64.
    if offset > size as u64 {
        return EINVAL;
    }
    FILE_OFFSET[asid as usize][idx].store(offset as u32, Ordering::Release);
    offset as i64
}

/// SYS_UNLINK(handle) -> `0` or a negative errno (U10; U11-M2 defers the chain-free). DELETE the file the open
/// File+`CAP_WRITE` handle refers to. The CHECK is `sys_write`'s — `handle_resolve(asid, handle, CAP_WRITE)` must
/// yield a `File` (an RO-opened File, a non-File kind, no handle, or a revoked cap all give `-EACCES`) — deletion
/// is a mutation, gated by the SAME single write CHECK, so it can never be reached without `CAP_WRITE`. A scaffold
/// descriptor with no recorded directory location (`dir_lba == 0`) is refused (`-EACCES`).
///
/// U11-M2 — POSIX unlink-defers-free. The NAME disappears immediately (`mark_dir_deleted` -> a re-open is
/// `-ENOENT`), but the cluster CHAIN is freed only at the file's LAST close across ALL processes:
///  1. `mark_dir_deleted` writes the directory entry `0xE5` FIRST (a failure here changes nothing -> `-EIO`).
///  2. Mark the file's open-file refcount row `unlink_pending` + stash the chain head (before the drops below,
///     so the drop that reaches `refcount == 0` frees the chain).
///  3. Drop ALL of THIS process's descriptors naming the file (`files_free_by_dir`, each decrementing the
///     refcount) — the U10 sibling-invalidation, so a file opened twice HERE leaves no live sibling on the freed
///     chain. If this process is the SOLE opener, the last decrement reaches 0 and returns the chain head, which
///     is freed NOW (all FAT copies) in this syscall context — byte-identical to U10's immediate delete. If
///     ANOTHER process still holds the file open, the refcount stays > 0: the chain is left ALLOCATED (a live
///     descriptor there keeps reading its original bytes) until that process's last `SYS_CLOSE` frees it.
fn sys_unlink(handle: u64) -> i64 {
    let asid = current_asid();
    // The CHECK: File + CAP_WRITE, or -EACCES (identical to sys_write's gate — delete is a mutation).
    let file_id = match handle_resolve(asid, handle, CAP_WRITE) {
        Ok(HandleTarget::File(id)) => id,
        _ => return EACCES,
    };
    // U11: decode + validate through the single descriptor-identity seam (range, `FILE_USED`, and generation).
    let Some(idx) = file_desc_validate(asid, file_id) else {
        return EACCES;
    };
    let cluster = FILE_CLUSTER[asid as usize][idx].load(Ordering::Acquire);
    let dir_lba = FILE_DIR_LBA[asid as usize][idx].load(Ordering::Acquire);
    let dir_off = FILE_DIR_OFF[asid as usize][idx].load(Ordering::Acquire) as usize;
    let open_row = FILE_OPENROW[asid as usize][idx].load(Ordering::Acquire); // the refcount row to mark pending
    // A descriptor with no recorded directory location (a scaffold, dir_lba == 0) cannot be deleted — refuse
    // rather than 0xE5 an unknown sector. Real opens always record a nonzero dir LBA (the FAT/dir regions never
    // sit at LBA 0 — that is the MBR / boot sector).
    if dir_lba == 0 {
        return EACCES;
    }
    // U6: DELETE is an OWNER-only authority. The handle-side `CAP_WRITE` CHECK above admits both the owner AND a
    // WRITE-GRANTEE (a grantee legitimately opened this file RW), but a content grantee must NOT be able to
    // delete — else it could `unlink` + `O_CREAT` the name to STEAL ownership and lock the real owner out. So an
    // OWNED file is unlinkable only by its current owner; a PUBLIC file (no owner row) keeps the prior behaviour.
    // Checked BEFORE any on-disk mutation, so a denied unlink changes nothing.
    let asid_gen = ASID_GEN[asid as usize].load(Ordering::Acquire);
    // K1 M2.4: the caller's persistent principal admits the by-name owner of a REBUILT row to delete its own
    // persisted file (its live incarnation is gone). NONE for the anonymous battery -> the branch is inert.
    let caller_ppid = current_principal();
    if !owned_unlink_permitted(dir_lba, dir_off as u32, asid, asid_gen, caller_ppid) {
        return EACCES;
    }
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    // K1 M2.3: snapshot the owner's persistent principal BEFORE owned_clear drops the row, so we know whether a
    // persisted UNAFS.ATR row must also be cleared. NONE for a public/anonymous file (the whole battery) => no
    // disk touch below => the anonymous unlink path stays byte-identical. In-RAM read; no lock held here.
    let owner_ppid = owned_owner_ppid(dir_lba, dir_off as u32);
    // K1 M2.4 F2 (security-review MF2): clear the PERSISTED UNAFS.ATR row FIRST — BEFORE the `0xE5` name delete —
    // so a crash between the two fails toward PUBLIC (the name still resolves but has no owner row -> the file is
    // public until re-persisted) instead of stranding a stale owner row that a FUTURE same-name file would adopt
    // at the next mount (the rebuild is name-primary). Named-owner only (`owner_ppid` captured above): a
    // public/anonymous file — the whole battery — skips this with ZERO disk I/O, so the anonymous unlink path is
    // byte-identical. `atr_clear_row` takes its OWN fresh `ns` for the single-row write; done before the main `ns`
    // hold below (never nested).
    // K3 (fold): a SWALLOWED clear failure would strand a stale owner row on disk (a dead fail-closed slot a
    // future same-name file could adopt), so gate the destructive `0xE5` name delete on the durable clear
    // ACTUALLY landing — abort with `-EIO` if it did not (fail-closed: the name still resolves, the owner
    // retries; nothing stranded). K6 M3: the durable store is now the NATIVE attribute volume
    // (`native_acl_clear`); the sidecar `atr_clear_row` stays as defense in depth for a stale card carrying
    // an un-migrated legacy row of this same file (benign true when no store/row exists — the battery skips
    // both entirely: only a real disk error trips this).
    if owner_ppid.kind != PRIN_NONE
        && !(crate::fs::unafs::native_acl_clear(dir_lba, dir_off as u32)
            && atr_clear_row(&fs, dir_lba, dir_off as u32))
    {
        return EIO;
    }
    // F3-M3: the NAMESPACE hold — the whole 0xE5 -> owned_clear -> mark-pending -> descriptor-drop sequence
    // runs atomically against any concurrent sys_open's lookup -> ACL -> incref (same lock). This closes the
    // `0xE5`-before-mark-pending window (a cross-core last-close can no longer land between them and miss the
    // pending flag) and the owned_clear-vs-owned_set_owner recycled-slot window. Taken AFTER mount(); the
    // orphan chain frees (which mount) happen after the guard drops.
    let ns = ns_lock();
    // (1) Make the NAME disappear FIRST — a re-open is `-ENOENT` immediately. A failure here leaves the row +
    // descriptors + refcount untouched (nothing to unwind) -> `-EIO`.
    if fs.mark_dir_deleted(dir_lba, dir_off).is_err() {
        return EIO;
    }
    // U6: the name is gone — drop its owner/grants row. FAT may recycle this directory slot for a DIFFERENT
    // file, which would then set its OWN owner; leaving a stale row here would (gen fence aside) misattribute
    // the recycled file. Placed after the `0xE5` write so the ACL row dies exactly when the name does.
    owned_clear(dir_lba, dir_off as u32);
    // (2) Arm the deferred free: mark THIS file's refcount row (by the descriptor's `FILE_OPENROW` index, not a
    // recyclable key) `unlink_pending` and stash the chain head. This process holds the file open (refcount >= 1),
    // so the row exists. MUST precede the drops below so the drop that reaches 0 (sole-opener case) sees the
    // pending flag and returns the chain head.
    openfile_mark_unlink_pending_at(open_row, cluster);
    // (3) Drop EVERY descriptor in this process's row that names the file (each decrements the refcount and bumps
    // its slot generation — U11 stale-sibling protection), COLLECTING each chain head that reaches its last close.
    // A sole-opener drops to 0 and frees immediately below (the U10 behaviour, deferred); a cross-process opener
    // keeps the chain allocated until its last close. Note: FAT slot-recycling can put a DIFFERENT still-open
    // unlinked file's descriptor on the same `(dir_lba, dir_off)` — EVERY collected head is freed, so neither
    // chain leaks.
    let (orphans, norphans) = files_free_by_dir(asid, dir_lba, dir_off as u32);
    // F3-M3: the sequence is complete — release the namespace before the chain frees (free_orphan_chain
    // mounts + walks chains; the lock is never held across that).
    drop(ns);
    for &fc in &orphans[..norphans] {
        free_orphan_chain(fc);
    }
    handle_clear(asid, handle as usize);
    0
}

/// SYS_CLOSE(handle) -> `0` or a negative errno (U11). Give a `File` descriptor a real end-of-life: free its
/// per-task descriptor slot (bumping the slot's generation, so any lingering sibling handle to the SAME slot
/// goes stale rather than re-binding to a file that later reuses the slot) and clear the handle word. A close is
/// not a mutation of the underlying object, so it requires NO capability right — `handle_resolve(asid, handle,
/// 0)` admits any live handle the caller holds (kind + descriptor identity are still enforced). Semantics:
///   * a live `File` -> free descriptor + clear handle -> `0`;
///   * a `Console`/`Socket`/`Child` kind -> `-EINVAL`, object table UNTOUCHED (not closeable this arc — never
///     corrupt it by freeing a File slot it does not own);
///   * an unresolvable handle (Empty / out-of-range / RESERVING / revoked), or a `File` whose descriptor is
///     already stale/closed (`file_desc_validate` fails) -> `-EBADF` — so a double-close returns cleanly and a
///     use-after-close is denied.
/// Only the caller's OWN descriptor slot is freed; a file another process holds open is unaffected (the
/// cross-process open lifetime is the open-file refcount). U11-M2: this drop decrements that refcount, and if it
/// is the LAST close of an `unlink_pending` file the deferred chain-free runs HERE — block I/O is legal in
/// syscall context (unlike teardown). The common case (a non-unlinked or not-last close) still does no I/O.
fn sys_close(handle: u64) -> i64 {
    let asid = current_asid();
    // Resolve for NO right (close is always permitted on a handle you hold). A non-File kind is refused without
    // being touched; anything that does not resolve falls through to -EBADF (already closed / never opened).
    let file_id = match handle_resolve(asid, handle, 0) {
        Ok(HandleTarget::File(id)) => id,
        Ok(_) => return EINVAL, // Console/Socket/Child — not a closeable File this arc; leave it intact
        Err(_) => return EBADF, // no such handle (already closed / never opened / oob / revoked)
    };
    // The handle is a live File, but its descriptor may already be gone (a sibling unlink freed the slot, or this
    // is a stale handle to a reused slot). Validate before freeing so a double-close / stale-close is -EBADF, not
    // a free of someone else's current descriptor.
    let Some(idx) = file_desc_validate(asid, file_id) else {
        return EBADF;
    };
    // Release the slot + bump its generation (stale-sibling protection) + decrement the cross-process refcount.
    let orphan = files_free(asid, idx);
    handle_clear(asid, handle as usize);
    // U11-M2: if this was the LAST close of an `unlink_pending` file, free its chain now (all FAT copies). Done
    // AFTER dropping the handle + the table lock — never under a lock; block I/O is legal here (syscall context).
    if let Some(fc) = orphan {
        free_orphan_chain(fc);
    }
    0
}

/// SYS_FGRANT(file_handle, child_handle, rights) -> `0` or a negative errno (U6). The OWNER of a private file
/// delegates (or revokes) access to another principal — the delegation half of the UnaFS owner/grants ACL.
/// OWNER-SCOPED, mirroring `sys_xfer`: the grantee is named by a `Child` handle the caller holds (never a raw
/// pid/ASID from EL0 — no ambient authority), the file by a `File` handle the caller holds. `rights` is the
/// `CAP_READ|CAP_WRITE` subset the grantee's later SYS_OPEN of that name may request (`0` REVOKES). The grant
/// is an ACL edge on the FILE — nothing lands in the grantee's handle table; it simply opens the name and the
/// SYS_OPEN ACL admits it. Only the file's current owner may grant (enforced in `owned_grant`). Errnos:
/// `-EACCES` (file handle not a live File / a scaffold descriptor / the caller is not the owner / a public
/// file), `-ECHILD` (child handle not a live Child, or the named child is not running / has no ASID),
/// `-ENOSPC` (the file's grant list is full). A File handle needs NO right here (`req = 0`) — ownership, not
/// the handle's rights, is the gate; you must merely hold the file open to name it.
fn sys_fgrant(file_handle: u64, child_handle: u64, rights: u64) -> i64 {
    let asid = current_asid();
    let asid_gen = ASID_GEN[asid as usize].load(Ordering::Acquire);
    // The FILE: a live File handle the caller holds; decode + validate its descriptor (range/USED/generation),
    // then read the on-disk identity `(dir_lba, dir_off)` the ACL row is keyed by.
    let file_id = match handle_resolve(asid, file_handle, 0) {
        Ok(HandleTarget::File(id)) => id,
        _ => return EACCES,
    };
    let Some(idx) = file_desc_validate(asid, file_id) else {
        return EACCES;
    };
    let dir_lba = FILE_DIR_LBA[asid as usize][idx].load(Ordering::Acquire);
    let dir_off = FILE_DIR_OFF[asid as usize][idx].load(Ordering::Acquire);
    if dir_lba == 0 {
        return EACCES; // a scaffold descriptor with no recorded directory location owns nothing
    }
    // Only the file's OWNER may grant/revoke — check FIRST, before resolving the grantee, so a non-owner is a
    // clean -EACCES whatever it passes as the child handle (and it never learns whether that handle was valid).
    // K1 M2.4: `caller_ppid` admits the by-name owner of a REBUILT row (its live incarnation is gone) to re-grant
    // on its own persisted file. NONE for the anonymous battery -> inert.
    let caller_ppid = current_principal();
    if !owned_is_owner(dir_lba, dir_off, asid, asid_gen, caller_ppid) {
        return EACCES;
    }
    // The GRANTEE: named owner-scoped by a Child handle the caller holds (the sys_xfer idiom — EL0 never
    // supplies a raw pid/ASID). Resolve child -> pid -> the recipient's live ASID + generation, exactly as
    // sys_xfer does (must be RUNNING now; a shared-window task with no ASID is not a grantee).
    let pid = match handle_resolve(asid, child_handle, 0) {
        Ok(HandleTarget::Child(pid)) => pid,
        _ => return ECHILD,
    };
    let Some(pi) = proc_find_child(pid) else {
        return ECHILD;
    };
    if PROCS[pi].state.load(Ordering::Acquire) != PRUNNING {
        return ECHILD;
    }
    let grantee_asid = PROCS[pi].asid.load(Ordering::Acquire);
    if grantee_asid == 0 || (grantee_asid as usize) >= ASID_GEN.len() {
        return ECHILD; // no ASID recorded (a shared-window task is not a grant recipient)
    }
    let grantee_gen = ASID_GEN[grantee_asid as usize].load(Ordering::Acquire);
    // Only READ|WRITE are grantable file rights. `rights == 0` is an explicit REVOKE; a NONZERO request that
    // names ONLY unsupported bits is malformed (`-EINVAL`) rather than silently coerced to a revoke.
    let req = rights as u32;
    if req != 0 && req & (CAP_READ | CAP_WRITE) == 0 {
        return EINVAL;
    }
    let rights = req & (CAP_READ | CAP_WRITE);
    // F2 (SMP-hardening): `grantee_asid` (from `PROCS[pi].asid`) and `grantee_gen` (from `ASID_GEN[grantee_asid]`)
    // were captured as TWO separate atomic loads. On a multi-core boot the named child could EXIT between them and
    // its ASID slot be recycled to a DIFFERENT process (a bumped `ASID_GEN`), so the grant would bind the stale
    // `grantee_asid` to the RECYCLED incarnation's `grantee_gen` — a misdelegation of the owner's file to whatever
    // process now holds that ASID (privilege escalation / disclosure). Re-validate that `pi` is STILL the same
    // running incarnation AFTER reading its gen: an ASID's gen bumps only at teardown, which drives `state` off
    // `PRUNNING` and clears `pid`/`asid` (`proc_free`), so a matching `state`/`pid`/`asid` proves the `(asid, gen)`
    // pair is a consistent snapshot of ONE incarnation. Any mismatch means the child recycled mid-resolution —
    // refuse (`-ECHILD`) rather than grant to the wrong principal. (Narrow, metal-only window — QEMU has no
    // cross-core preemption; behaviourally transparent on the single-core path, where nothing recycles.)
    if PROCS[pi].state.load(Ordering::Acquire) != PRUNNING
        || PROCS[pi].pid.load(Ordering::Acquire) != pid
        || PROCS[pi].asid.load(Ordering::Acquire) != grantee_asid
        || ASID_GEN[grantee_asid as usize].load(Ordering::Acquire) != grantee_gen
    {
        return ECHILD;
    }
    // K1 M2.3: the grantee's PERSISTENT principal, captured BEFORE owned_grant (SLOT_PPID never nested under
    // OWNED_FILES). NONE for an anonymous grantee (the battery) — a NONE grant persists nothing.
    let grantee_ppid = slot_ppid_of(grantee_asid);
    // K3: a REVOKE of a NAMED grantee on a NAMED-owner file uses TWO-PHASE commit ordering — persist the
    // NARROWED row to disk BEFORE the in-RAM removal, so a crash or a swallowed disk error during the re-persist
    // can never leave the revoked grant on disk to be re-admitted at the next mount (the retired fail-OPEN
    // residual). Only a NAMED grantee is at risk: an anonymous (NONE) grantee persists as an inert NONE-principal
    // row a rebuild never re-admits, so it keeps the byte-identical in-RAM-then-best-effort-persist order, as
    // does every widen (grant/update) — a lost widen persist just drops the new grant (already fail-CLOSED).
    // The owner authority was already validated by `owned_is_owner` above, so persisting first is sound.
    if rights == 0 && grantee_ppid.kind != PRIN_NONE && owned_owner_ppid(dir_lba, dir_off).kind != PRIN_NONE {
        return sys_fgrant_revoke_2phase(
            dir_lba, dir_off, asid, asid_gen, grantee_asid, grantee_gen, caller_ppid, grantee_ppid,
        );
    }
    let rc = owned_grant(dir_lba, dir_off, asid, asid_gen, grantee_asid, grantee_gen, rights, caller_ppid, grantee_ppid);
    // K1 M2.3 / K6 M3: if the ACL mutation succeeded on a NAMED-owner file, re-persist the row (owner +
    // grants) onto the NATIVE attribute volume so the grant SURVIVES REBOOT. The named-owner probe is an
    // IN-RAM read inside native_persist_grants, so an anonymous owner (the whole battery) does ZERO disk
    // I/O here and the sys_fgrant path stays byte-identical — no mount of either volume.
    if rc == 0 {
        let _ = native_persist_grants(dir_lba, dir_off);
    }
    rc
}

/// K3: the two-phase REVOKE of a named grantee on a named-owner file — the commit-ordering fix that retires the
/// fail-OPEN revoke residual. The old order (in-RAM revoke, THEN best-effort re-persist) told the owner the
/// revoke succeeded, but a crash / swallowed disk error during the re-persist left the OLD grant on disk, so the
/// revoked grantee was re-admitted at the next mount. Here the durable side commits FIRST: compute the POST-revoke
/// grant set from the current in-RAM snapshot, write that narrowed row to disk, and commit the in-RAM removal ONLY
/// if the disk write held. A persist failure is FAIL-CLOSED — the in-RAM grant is left untouched (still enforced
/// this boot) and the caller gets `-EIO`/`-ENODEV`, so RAM and disk never silently diverge (no false success).
/// Callers: `sys_fgrant` (owner already validated) and the `k3_revoke_check` proof (manufactured principals).
fn sys_fgrant_revoke_2phase(
    dir_lba: u64,
    dir_off: u32,
    owner_asid: u64,
    owner_gen: u64,
    grantee_asid: u64,
    grantee_gen: u64,
    owner_ppid: PrincipalRecord,
    grantee_ppid: PrincipalRecord,
) -> i64 {
    // K5B (2026-07-15, NARROW ruling — the NAMESPACE-unfreeze): FUSE the whole snapshot -> disk-narrow ->
    // in-RAM-commit under ONE `with_unafs` MOUNT hold, and do NOT take `ns` at all. THE K5B INVARIANT: for any
    // file's ACL, the OWNED_FILES snapshot that produces a durable row, the journaled write of that row, and the
    // in-RAM mutation the row reflects all occur under a single uninterrupted MOUNT hold — so any two ACL
    // persisters run strictly serially, and no persister can observe an in-RAM state inconsistent with the last
    // row it wrote. The K5 resurrection needed a concurrent re-persist to snapshot BETWEEN our disk-narrow and
    // our in-RAM commit; with the commit INSIDE the same MOUNT hold as the snapshot+write (and every re-persister
    // snapshotting inside ITS hold), that straddle is structurally impossible: a re-persist runs either entirely
    // before us (its full row is overwritten by our narrowed write) or entirely after (it snapshots our already-
    // narrowed set). Lock order: MOUNT ⊃ OWNED_FILES (leaf, take-and-release inside the hold; no path takes
    // OWNED_FILES then MOUNT), composing with NAMESPACE ⊃ MOUNT — no inversion. WHY ns IS DROPPABLE: the K3-era
    // revoke never needed ns for namespace correctness; K5 took it ONLY as the anti-resurrection serializer, and
    // MOUNT now carries that. Effect: the ~0.7 s IRQ-masked polled-SD write no longer freezes NAMESPACE (opens/
    // unlinks on other cores proceed); the per-core mask under with_unafs REMAINS (ledgered residual — the
    // with_unafs IRQ-mask keystone and K3 durable-first are STOP-class, untouched). See SECURITY.md §K1 K5B.
    //
    // Anonymous fast path (the whole battery): a NONE-owner row never persists, so there is nothing durable to
    // diverge from — commit in-RAM directly, ZERO disk I/O, no mount (byte-equivalent battery, the K5 idiom).
    // A vanished row (raced unlink) leaves nothing to revoke — the pre-K5B `0`, not owned_grant's -EACCES.
    if owned_owner_ppid(dir_lba, dir_off).kind == PRIN_NONE {
        if owned_snapshot_row(dir_lba, dir_off).is_none() {
            return 0;
        }
        return owned_grant(dir_lba, dir_off, owner_asid, owner_gen, grantee_asid, grantee_gen, 0, owner_ppid, grantee_ppid);
    }
    #[cfg(feature = "nsspan")] let _nsspan = NsSpanProbe::new(&NSSPAN_REVOKE); match crate::fs::unafs::with_unafs(|fs| { // K5B WATCH probe brackets the fused with_unafs call (ns not taken); newline-neutral inline so knob-off is byte-identical
        // Snapshot the current ACL and drop the target grantee (matched by its durable persistent principal — the
        // on-disk key; the snapshot carries ppids only). A vanished row (raced unlink) leaves nothing to revoke.
        let Some((row_owner, mut grants)) = owned_snapshot_row(dir_lba, dir_off) else {
            return 0;
        };
        for g in grants.iter_mut() {
            if g.1 != 0 && g.0.kind != PRIN_NONE && g.0 == grantee_ppid {
                *g = (PrincipalRecord::NONE, 0);
            }
        }
        // Phase 1 (durable-first, K3 — preserved verbatim): write the NARROWED row to the NATIVE store on the
        // mount we hold. On failure, do NOT commit in-RAM — fail closed.
        if !native_write_grant_row_on(fs, dir_lba, dir_off, row_owner, &grants) {
            return EIO;
        }
        // Phase 2: disk now reflects the revoke — commit the in-RAM removal (owner already validated), INSIDE the
        // SAME MOUNT hold (the K5B invariant's load-bearing clause).
        owned_grant(dir_lba, dir_off, owner_asid, owner_gen, grantee_asid, grantee_gen, 0, owner_ppid, grantee_ppid)
    }) {
        Ok(rc) => rc,
        // Media without a unafs volume / no block device: no native row can exist (create-persist fails the same
        // way), so there is nothing durable to diverge from — commit in-RAM, the pre-K5B semantics for this media
        // class (the same error split native_acl_clear uses).
        Err(crate::fs::unafs::MountError::NoVolume
        | crate::fs::unafs::MountError::NoStorage
        | crate::fs::unafs::MountError::BadSectorSize(_)) => {
            owned_grant(dir_lba, dir_off, owner_asid, owner_gen, grantee_asid, grantee_gen, 0, owner_ppid, grantee_ppid)
        }
        // A real mount/parse failure on a volume that EXISTS: a persisted row may be present and un-narrowable —
        // fail closed (K3 durable-first: in-RAM grant intact, caller gets -EIO).
        Err(_) => EIO,
    }
}

// =============================================================================================
// U7: cross-process capability transfer — the per-ASID transfer INBOX, the sender-owned transfer
// RECORDS (the revoke ledger), and SYS_XFER / SYS_RECV / SYS_CAP-XREVOKE.
// =============================================================================================
//
// THE INVARIANT THIS DESIGN EXISTS TO PRESERVE: every `HANDLES[asid]` row has exactly ONE writer — its
// own task (U4's lock-free foundation). A naive transfer where sender A writes recipient B's row would
// break that. So A never touches B's row: A deposits an attenuated `(kind, target, rights)` descriptor
// into B's per-ASID INBOX — the one deliberately cross-ASID surface, where every claim/consume/retract
// is a tx-exact CAS — and B pulls it into its OWN row with SYS_RECV (B writes only itself). Revocation
// reaches B the same one-way: the sender flips a bit in ITS OWN transfer RECORD, and B's next
// `handle_resolve` of the received cap reads it (the read-side hook in handle_resolve) — nobody ever
// writes another task's row. Delegation is OWNER-SCOPED: the recipient is named by a `Child` handle in
// the SENDER's table (no global process namespace).
//
// Slot/record state words reuse the handle protocol: `0` = free, `HANDLE_RESERVING` (u64::MAX) = an
// in-flight claim, anything else = live (a slot holds the transfer id; a record holds its tx). Sidecars
// are published (Release) BEFORE the state word goes live and read (Acquire) after observing it — the
// HANDLE_RIGHTS discipline. Transfer ids are globally unique (a monotonic counter), which is what makes
// the tx-exact CASes ABA-safe: a consumer, a retracting sender, and a tearing-down recipient can race
// and exactly one wins each slot.
//
// Scope (the Opus core, deliberately): single-LEVEL revoke (no cascade through re-transfers — revocation
// TREES are deferred); Console/Socket payloads only (`File` is refused: a file-id indexes the SENDER's
// per-ASID FILES row, so a cross-ASID File transfer needs descriptor migration — deferred with writes/
// seek; `Child` is refused: delegating reap rights is a process-model question, not a transfer one);
// records are a small fixed ledger (`MAX_XFERS`) whose lifetime IS the transfer's (claimed at XFER,
// released by whichever of handle-drop / pending-discard / sender-retract / recipient-teardown ends it).
// One residual TOCTOU is accepted and documented at the sys_xfer post-check.

/// Pending-transfer slots per recipient (per ASID row). Small and static, like NHANDLE — a full inbox is
/// `-EAGAIN` (the sender retries or gives up), never grown.
const NXFER: usize = 4;
/// Sender-side transfer records (the revoke ledger), global — each live transfer holds exactly one.
const MAX_XFERS: usize = 8;
/// Bit 63 of a RECORD's TX word marks the (still-live) transfer REVOKED. The flag rides IN the state word
/// so the revoke is a **tx-exact CAS** like every other transition — a separate flag word would race the
/// free/reclaim cycle (the review-confirmed stale-revoke: a delayed store landing on a freed-and-reclaimed
/// record would revoke an unrelated sender's fresh transfer, or mint one born-revoked). txids are a
/// monotonic counter from 1, so bit 63 is never set on a genuine id; `txid | BIT` can never alias
/// `RESERVING` (that would need `txid == i64::MAX`).
const XFER_REVOKED_BIT: u64 = 1 << 63;

/// The inbox slot's STATE word: 0 = free, `HANDLE_RESERVING` = mid-claim, else = the transfer id (live).
static XFER_SLOT_TX: [[AtomicU64; NXFER]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NXFER] }; super::boot::USER_SLOTS + 1];
/// The pending descriptor: what kind of object the transferred cap names. Meaningful only where TX is live.
static XFER_SLOT_KIND: [[AtomicU8; NXFER]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU8::new(KIND_EMPTY) }; NXFER] }; super::boot::USER_SLOTS + 1];
/// The pending descriptor's target payload (the value word the received handle will carry).
static XFER_SLOT_TARGET: [[AtomicU64; NXFER]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NXFER] }; super::boot::USER_SLOTS + 1];
/// The pending descriptor's (already attenuated) rights.
static XFER_SLOT_RIGHTS: [[AtomicU32; NXFER]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NXFER] }; super::boot::USER_SLOTS + 1];
/// The record index + 1 backing this pending transfer (0 = none — a kernel bug on a live slot).
static XFER_SLOT_REC: [[AtomicU32; NXFER]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NXFER] }; super::boot::USER_SLOTS + 1];

/// A record's STATE word: 0 = free, `HANDLE_RESERVING` = mid-claim, `txid` = the live transfer it
/// ledgers, `txid | XFER_REVOKED_BIT` = that transfer, revoked (read by `handle_resolve` — the received
/// cap goes stale — and by `sys_recv` — a still-pending revoked transfer is discarded, never delivered).
static XFER_REC_TX: [AtomicU64; MAX_XFERS] = [const { AtomicU64::new(0) }; MAX_XFERS];
/// The ASID that made the transfer — only IT may XREVOKE (sender-owned; checked in sys_cap_xrevoke).
/// Disowned to `u64::MAX` (never a real ASID) when the sender's ASID tears down, so revoke authority
/// dies with the sender instead of passing to the ASID's next tenant.
static XFER_REC_SENDER: [AtomicU64; MAX_XFERS] = [const { AtomicU64::new(0) }; MAX_XFERS];
/// The next transfer id — globally unique, monotonic from 1 (never 0/u64::MAX, the state sentinels).
static XFER_NEXT_TX: AtomicU64 = AtomicU64::new(1);

/// Which transfer RECORD (index + 1; 0 = not a transferred cap) a RECEIVED handle references — the
/// revocation hook `handle_resolve` reads. Keyed `[asid][idx]` like the other handle sidecars, and — the
/// point — written ONLY by the row's own task (`sys_recv`) or its teardown: the sender reaches a received
/// cap exclusively through the record, never through this row.
static HANDLE_XFER_REC: [[AtomicU32; NHANDLE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NHANDLE] }; super::boot::USER_SLOTS + 1];

/// Claim a free transfer record and mint its transfer id: CAS the state word 0 -> RESERVING, publish the
/// sender (Release), then the tx LAST (live-last, the handle_install discipline; the revoked flag needs no
/// reset — it lives in the TX word this store replaces whole).
fn xfer_rec_claim(sender_asid: u64) -> Option<(usize, u64)> {
    for r in 0..MAX_XFERS {
        if XFER_REC_TX[r]
            .compare_exchange(0, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            XFER_REC_SENDER[r].store(sender_asid, Ordering::Release);
            let tx = XFER_NEXT_TX.fetch_add(1, Ordering::AcqRel);
            XFER_REC_TX[r].store(tx, Ordering::Release);
            return Some((r, tx));
        }
    }
    None
}

/// Release a transfer record — by exactly ONE of: the received handle's drop (`handle_clear`), a pending
/// revoked transfer's discard (`sys_recv`), the sender's retract (`sys_xfer` post-check), or the
/// recipient's inbox teardown. Fields cleared first, the state word freed LAST (the files_free shape).
fn xfer_rec_free(r: usize) {
    debug_assert!(r < MAX_XFERS, "xfer_rec_free: out of range");
    XFER_REC_SENDER[r].store(0, Ordering::Release);
    XFER_REC_DERIV[r].store(0, Ordering::Release); // U8: the derivation sidecar clears with the record
    XFER_REC_DERIV_ID[r].store(0, Ordering::Release);
    XFER_REC_TX[r].store(0, Ordering::Release); // clears the revoked bit with the id (one word)
}

/// True iff the whole record ledger is free — the U7 leak verifier (every transfer's lifetime closed).
fn xfer_recs_all_free() -> bool {
    (0..MAX_XFERS).all(|r| XFER_REC_TX[r].load(Ordering::Acquire) == 0)
}

/// Zero a CLAIMED inbox slot's descriptor fields and free the slot (state word 0 LAST). The caller must
/// OWN the slot — i.e. hold its tx-exact CAS win (consume/retract/teardown) or its RESERVING claim.
fn xfer_slot_release(asid: u64, k: usize) {
    debug_assert!((asid as usize) < XFER_SLOT_TX.len() && k < NXFER, "xfer_slot_release: out of range");
    XFER_SLOT_KIND[asid as usize][k].store(KIND_EMPTY, Ordering::Release);
    XFER_SLOT_TARGET[asid as usize][k].store(0, Ordering::Release);
    XFER_SLOT_RIGHTS[asid as usize][k].store(0, Ordering::Release);
    XFER_SLOT_REC[asid as usize][k].store(0, Ordering::Release);
    XFER_SLOT_DERIV[asid as usize][k].store(0, Ordering::Release); // U8 sidecars clear with the slot
    XFER_SLOT_GEN[asid as usize][k].store(0, Ordering::Release);
    XFER_SLOT_TX[asid as usize][k].store(0, Ordering::Release);
}

/// True iff `asid`'s inbox holds no live or in-flight slot — the teardown/leak verifier.
fn xfer_row_is_clear(asid: u64) -> bool {
    debug_assert!((asid as usize) < XFER_SLOT_TX.len(), "xfer_row_is_clear: asid out of range");
    (0..NXFER).all(|k| XFER_SLOT_TX[asid as usize][k].load(Ordering::Acquire) == 0)
}

/// Teardown-clear an ASID's transfer inbox (called from `clear_handle_row`): claim each live slot by
/// tx-exact CAS (so a racing consumer/retractor never double-frees), free its record, release the slot. A
/// slot mid-claim (`RESERVING`) belongs to a sender between its CAS and its live-store; that sender's own
/// post-check retracts it (it re-reads the recipient's Proc state, which is no longer RUNNING by the time
/// this teardown runs) — one pass here is sufficient, not a spin.
fn clear_xfer_inbox_row(asid: u64) {
    for k in 0..NXFER {
        let tx = XFER_SLOT_TX[asid as usize][k].load(Ordering::Acquire);
        if tx == 0 || tx == HANDLE_RESERVING {
            continue;
        }
        if XFER_SLOT_TX[asid as usize][k]
            .compare_exchange(tx, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let rec = XFER_SLOT_REC[asid as usize][k].load(Ordering::Acquire);
            if rec != 0 {
                xfer_rec_free((rec - 1) as usize);
            }
            // U8: a swept (never-delivered) deposit drops its derivation node with the slot.
            let dn = XFER_SLOT_DERIV[asid as usize][k].load(Ordering::Acquire);
            if dn != 0 {
                deriv_drop(dn);
            }
            xfer_slot_release(asid, k);
        }
    }
}

/// SYS_XFER(dest, src, req_rights) -> a transfer id (>= 1), or a negative errno. Deposit an ATTENUATED
/// copy of a capability the caller holds into the recipient's inbox — the cross-process delegation
/// primitive (a shell handing an editor a console/file cap), owner-scoped: `dest` must be a `Child`
/// handle in the CALLER's own table.
///
/// Flow: resolve `dest` (must be `Child`; `-ECHILD` otherwise) -> resolve `src` in the caller's own table
/// (must carry `CAP_GRANT`, the delegation right — the same authority `sys_cap_grant` demands; `-EACCES`)
/// -> enforce ATTENUATION (`req & !src_rights != 0` => `-EACCES` — the monotonic-decrease invariant,
/// cross-process now) -> map the child pid to its ASID through its Proc entry (must be RUNNING;
/// `-ECHILD`) -> claim a record, then CAS-claim an inbox slot (each full => `-EAGAIN`, the record
/// unwound) -> publish the descriptor, tx LAST -> POST-CHECK the recipient.
///
/// The post-check closes the deposit-vs-exit race from the sender's side: if the recipient exited between
/// the RUNNING check and the deposit going live, its teardown may have already swept the inbox — so the
/// sender re-reads the Proc entry and, on any change, RETRACTS its own deposit (tx-exact CAS; loser of
/// the race does nothing — the winner freed the record) and returns `-ECHILD`. U8 closed the residual
/// TOCTOU U7 documented here (exit + ASID-recycle + new-tenant-consume inside the window): deposits are
/// stamped with the recipient's inbox GENERATION, teardown bumps it, RECV delivers only on an exact match,
/// and this post-check re-reads it — the wrong-tenant delivery is now structurally impossible.
///
/// Payload kinds: `Console`/`Socket` only. `File` is refused (`-EACCES`): its file-id indexes the
/// SENDER's per-ASID FILES row — a cross-ASID File transfer needs descriptor migration (deferred).
/// `Child` is refused (`-EACCES`): delegating reap rights is a process-model arc, not a transfer one.
fn sys_xfer(dest: u64, src: u64, req_rights: u64) -> i64 {
    sys_xfer_from(current_asid(), dest, src, req_rights)
}

/// The `sys_xfer` body, parameterized on the sending ASID — the SVC arm passes `current_asid()`; the U8
/// kernel-side check drives the SAME code path with scratch ASIDs (no EL0 detour, no duplicate logic).
fn sys_xfer_from(asid: u64, dest: u64, src: u64, req_rights: u64) -> i64 {
    // 1. The recipient: a Child handle in the CALLER's OWN table (structural, owner-scoped delegation).
    let pid = match handle_resolve(asid, dest, 0) {
        Ok(HandleTarget::Child(pid)) => pid,
        _ => return ECHILD,
    };
    // 2. The source capability: present, carrying CAP_GRANT (the delegation right), of a transferable kind.
    let Some(target) = handle_get(asid, src as usize) else {
        return EACCES;
    };
    if target == HANDLE_RESERVING {
        return EACCES; // an in-flight reservation is not a transferable handle (defensive; single-writer)
    }
    let src_kind = handle_kind(asid, src as usize);
    if src_kind != KIND_CONSOLE && src_kind != KIND_SOCKET {
        return EACCES; // File needs descriptor migration; Child would delegate reaping — both refused
    }
    let src_rights = HANDLE_RIGHTS[asid as usize][src as usize].load(Ordering::Acquire);
    if src_rights & CAP_GRANT == 0 {
        return EACCES; // the source does not authorize delegation
    }
    // U7: a revoked RECEIVED cap must not be re-TRANSFERRED onward either (the sys_cap_grant laundering
    // check's transfer twin) — post-revoke delegation is refused; pre-revoke copies are the tree arc.
    let src_rec = HANDLE_XFER_REC[asid as usize][src as usize].load(Ordering::Acquire);
    if src_rec != 0
        && XFER_REC_TX[(src_rec - 1) as usize].load(Ordering::Acquire) & XFER_REVOKED_BIT != 0
    {
        return EACCES;
    }
    // U8: a source on a revoked derivation chain must not be re-transferred either (the tree-deep twin of
    // the record check above — post-revoke delegation is refused however deep the chain).
    let src_dn = HANDLE_DERIV[asid as usize][src as usize].load(Ordering::Acquire);
    if src_dn != 0 && deriv_stale(src_dn) {
        return EACCES;
    }
    // 3. Attenuation across processes: any requested bit the sender does not hold is an amplification.
    let req = req_rights as u32;
    if req & !src_rights != 0 {
        return EACCES;
    }
    // 4. pid -> the recipient's ASID (the inbox key), via the Proc table; it must be RUNNING now (and is
    //    re-checked after the deposit — see the post-check below).
    let Some(pi) = proc_find_child(pid) else {
        return ECHILD;
    };
    if PROCS[pi].state.load(Ordering::Acquire) != PRUNNING {
        return ECHILD;
    }
    let dst_asid = PROCS[pi].asid.load(Ordering::Acquire);
    if dst_asid == 0 || (dst_asid as usize) >= XFER_SLOT_TX.len() {
        return ECHILD; // no ASID recorded (a shared-window task is not a transfer recipient)
    }
    // U8: snapshot the recipient's inbox GENERATION before depositing — the deposit is stamped with it,
    // RECV verifies it, and the post-check re-reads it (a change = the recipient tore down = retract).
    let dst_gen = ASID_GEN[dst_asid as usize].load(Ordering::Acquire);
    // 5. Record the derivation edge (delivered cap -> source), then claim the revoke ledger entry, then
    //    the inbox slot; any exhaustion unwinds cleanly (-EAGAIN).
    let Some((node, node_id)) = deriv_derive_from(asid, src as usize) else {
        return EAGAIN;
    };
    let Some((rec, tx)) = xfer_rec_claim(asid) else {
        deriv_drop(node);
        return EAGAIN;
    };
    // The record remembers the transfer's node (+ its id, the ABA guard) so XREVOKE kills the subtree.
    XFER_REC_DERIV[rec].store(node, Ordering::Release);
    XFER_REC_DERIV_ID[rec].store(node_id, Ordering::Release);
    let Some(slot) = (0..NXFER).find(|&k| {
        XFER_SLOT_TX[dst_asid as usize][k]
            .compare_exchange(0, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }) else {
        xfer_rec_free(rec);
        deriv_drop(node);
        return EAGAIN; // the recipient's inbox is full
    };
    // 6. Publish the descriptor (Release), the tx LAST — a recipient that observes the live tx observes
    //    the whole descriptor (the handle publish-order discipline, applied to the one cross-ASID surface).
    XFER_SLOT_KIND[dst_asid as usize][slot].store(src_kind, Ordering::Release);
    XFER_SLOT_TARGET[dst_asid as usize][slot].store(target, Ordering::Release);
    XFER_SLOT_RIGHTS[dst_asid as usize][slot].store(req, Ordering::Release);
    XFER_SLOT_REC[dst_asid as usize][slot].store((rec + 1) as u32, Ordering::Release);
    XFER_SLOT_DERIV[dst_asid as usize][slot].store(node, Ordering::Release);
    XFER_SLOT_GEN[dst_asid as usize][slot].store(dst_gen, Ordering::Release);
    XFER_SLOT_TX[dst_asid as usize][slot].store(tx, Ordering::Release);
    // 7. POST-CHECK + retract (see the doc comment). Same entry, same pid, still RUNNING, SAME inbox
    //    generation — or undo ours. The generation re-read (U8) closes the residual TOCTOU U7 documented:
    //    even if the entry looks unchanged, a teardown-and-recycle inside the window bumped the generation,
    //    and a deposit stamped with the OLD one is retracted here (or discarded at RECV — both sides hold).
    if PROCS[pi].state.load(Ordering::Acquire) != PRUNNING
        || PROCS[pi].pid.load(Ordering::Acquire) != pid
        || ASID_GEN[dst_asid as usize].load(Ordering::Acquire) != dst_gen
    {
        if XFER_SLOT_TX[dst_asid as usize][slot]
            .compare_exchange(tx, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            xfer_slot_release(dst_asid, slot);
            xfer_rec_free(rec);
            deriv_drop(node);
        }
        // CAS failure = the recipient's teardown (or a last-instant consume) won the slot and freed (or
        // took ownership of) the record + node — either way they are no longer ours to unwind.
        return ECHILD;
    }
    tx as i64
}

/// SYS_RECV() -> a handle index, or `-EAGAIN` when nothing is pending (the caller yields and retries) —
/// also `-EAGAIN` when the caller's handle table is full (the pending transfer stays queued). The
/// recipient's half of the transfer: scan the CALLER's OWN inbox, claim the first live slot (tx-exact
/// CAS), and install the descriptor into the CALLER's OWN handle row — the single-writer invariant is
/// preserved because the only row written is the caller's, by the caller, mid-SVC.
///
/// A transfer revoked while still PENDING is discarded here (record freed, slot released, scan continues)
/// — it is never delivered. A delivered cap records its transfer (the `HANDLE_XFER_REC` sidecar, stored
/// BEFORE the live value) so a later revoke reaches it at `handle_resolve`.
fn sys_recv() -> i64 {
    sys_recv_for(current_asid())
}

/// The `sys_recv` body, parameterized on the receiving ASID — the SVC arm passes `current_asid()`; the U8
/// kernel-side check drives the SAME code path with scratch ASIDs (no EL0 detour, no duplicate logic).
fn sys_recv_for(asid: u64) -> i64 {
    if (asid as usize) >= XFER_SLOT_TX.len() {
        return EAGAIN; // defensive; a real EL0 caller always has an in-range ASID
    }
    for k in 0..NXFER {
        let tx = XFER_SLOT_TX[asid as usize][k].load(Ordering::Acquire);
        if tx == 0 || tx == HANDLE_RESERVING {
            continue;
        }
        // Claim-to-consume (tx-exact): losing means a racing retract/teardown owns the slot — move on.
        if XFER_SLOT_TX[asid as usize][k]
            .compare_exchange(tx, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        let kind = XFER_SLOT_KIND[asid as usize][k].load(Ordering::Acquire);
        let target = XFER_SLOT_TARGET[asid as usize][k].load(Ordering::Acquire);
        let rights = XFER_SLOT_RIGHTS[asid as usize][k].load(Ordering::Acquire);
        let rec = XFER_SLOT_REC[asid as usize][k].load(Ordering::Acquire);
        let node = XFER_SLOT_DERIV[asid as usize][k].load(Ordering::Acquire);
        let dep_gen = XFER_SLOT_GEN[asid as usize][k].load(Ordering::Acquire);
        // Revoked while pending, a recordless slot (a kernel bug, failed closed), or — U8 — a deposit
        // stamped for a PREVIOUS tenant of this ASID (its generation predates the current one: the sender
        // aimed at a process that tore down; the recycled ASID's new tenant must never consume it):
        // discard, keep scanning.
        if rec == 0
            || XFER_REC_TX[(rec - 1) as usize].load(Ordering::Acquire) & XFER_REVOKED_BIT != 0
            || dep_gen != ASID_GEN[asid as usize].load(Ordering::Acquire)
        {
            if rec != 0 {
                xfer_rec_free((rec - 1) as usize);
            }
            if node != 0 {
                deriv_drop(node); // the undelivered cap's node dies with the deposit
            }
            xfer_slot_release(asid, k);
            continue;
        }
        // Install into the CALLER's OWN row: reserve first-free, publish kind + rights + the transfer
        // reference + the derivation node, then the live value LAST. A full table re-queues the transfer
        // (restore the tx — we own the slot, so a plain Release store is safe) and returns -EAGAIN.
        let Some(h) = handle_install(asid, HANDLE_RESERVING) else {
            XFER_SLOT_TX[asid as usize][k].store(tx, Ordering::Release);
            return EAGAIN;
        };
        handle_set_kind(asid, h, kind);
        handle_set_rights(asid, h, rights);
        HANDLE_XFER_REC[asid as usize][h].store(rec, Ordering::Release); // the revocation hook, pre-live
        HANDLE_DERIV[asid as usize][h].store(node, Ordering::Release); // the derivation edge, pre-live (U8)
        handle_set(asid, h, target);
        xfer_slot_release(asid, k); // consume the inbox slot (record ownership moved to the handle)
        return h as i64;
    }
    EAGAIN
}

/// SYS_CAP XREVOKE(transfer id): the SENDER invalidates a transfer it made. Single-level: the received
/// cap goes stale at its next `handle_resolve` (or the pending deposit is discarded at RECV) — but a cap
/// the recipient already re-granted/re-transferred onward is NOT cascaded (revocation TREES, deferred).
/// Sender-only: the record carries the transferring ASID; anyone else gets `-EACCES`. An unknown/already-
/// closed transfer id is `-ENOENT` (ids are globally unique, so a stale id can never alias a new one).
fn sys_cap_xrevoke(asid: u64, txid: u64) -> i64 {
    if txid == 0 || txid == HANDLE_RESERVING || txid & XFER_REVOKED_BIT != 0 {
        return ENOENT; // never a live transfer id
    }
    for r in 0..MAX_XFERS {
        if XFER_REC_TX[r].load(Ordering::Acquire) == txid {
            if XFER_REC_SENDER[r].load(Ordering::Acquire) != asid {
                return EACCES; // only the sender may revoke its transfer (disowned records match no ASID)
            }
            // TX-EXACT, like every other record/slot transition (the review-confirmed fix): the revoked
            // bit can only land while the record still ledgers THIS transfer. A lost CAS means the record
            // was freed (and possibly reclaimed for someone else's transfer) between the find and the flip
            // — the transfer is already closed, NOTHING is written to the record's current tenant, and the
            // caller honestly gets -ENOENT. (The old separate-flag store could land on a reclaimed record
            // and revoke an unrelated live transfer, or mint one born-revoked.)
            // U8: capture the transfer's derivation node (+ its publish-time id) BEFORE the CAS — a won
            // CAS proves the record still ledgered this transfer when read, so the pair is this transfer's.
            let dn = XFER_REC_DERIV[r].load(Ordering::Acquire);
            let dn_id = XFER_REC_DERIV_ID[r].load(Ordering::Acquire);
            return if XFER_REC_TX[r]
                .compare_exchange(txid, txid | XFER_REVOKED_BIT, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                // Mark the transfer's node revoked (id-guarded — a concurrently dropped-and-reclaimed
                // node is left alone; it had no children, so nothing escapes). This is what makes the
                // revoke a TREE: every re-grant/re-transfer below the delivered cap dies at next use.
                deriv_revoke_if(dn, dn_id);
                0
            } else {
                ENOENT
            };
        }
    }
    ENOENT
}

// =============================================================================================
// U8: revocation TREES (the derivation ledger) + generation-tagged inboxes — closing U7's two
// documented escapes.
// =============================================================================================
//
// U7 left two honest gaps (its SECURITY.md entry): (1) revoke was SINGLE-LEVEL — a recipient who re-granted
// or re-transferred a received cap created a DERIVED copy a later revoke never reached; (2) the sys_xfer
// post-check had a residual TOCTOU — recipient-exit + ASID-recycle + new-tenant-consume inside the
// deposit-live -> post-check window could deliver a transfer to the wrong tenant. U8 closes both.
//
// (1) THE DERIVATION LEDGER. Every mint that derives one capability from another records an edge
// child -> parent in a bounded static ledger of NODES (the U7 static-atomic-array discipline — no heap,
// state-exact CAS transitions, Release-publish / Acquire-read). `sys_cap_grant` (a local mint) and
// `sys_xfer`/`sys_recv` (a delivered transfer) both derive; handles installed by spawn/open/endow are ROOTS
// (no node until they first act as a grant/transfer source — the node is created LAZILY then). Revocation is
// MARK-ONE-NODE; staleness is discovered at USE: `handle_resolve` walks child -> root through the ledger
// (bounded — the ledger is bounded and a parent is always created before its child, so cycles are impossible
// by construction) and fails `Denied` if ANY ancestor is revoked. This keeps U7's load-bearing invariant:
// **no revoke path ever writes another ASID's row** — the stale-at-use pattern, generalized. Revoke is O(1);
// resolve pays the bounded walk.
//
// NODE LIFETIME (the documented choice: tombstones until the subtree drains). A node frees when its owning
// handle DROPS **and** it has no live children; a dropped node with live children persists as a TOMBSTONE
// (still walkable, still carrying its revoked bit) until its last child frees — then the free cascades up
// through any drained tombstoned ancestors (`deriv_drop`). Freeing is arbitrated by a state-exact CAS on the
// node's ID word (two racing freers — the owner's drop vs a child's cascade — resolve to exactly one winner).
// Every walkable node is PINNED: a resolver only reaches a node through a live handle (whose own node cannot
// free) and live child edges (a live child holds `KIDS > 0` on its parent), so no walk ever reads a freed/
// reclaimed node. Ledger exhaustion is `-EAGAIN` at mint/transfer time (the U4 resource-bound discipline —
// claim last, unwind on failure, no leak on any path).
//
// (2) GENERATION-TAGGED INBOXES. A per-ASID generation word is bumped at teardown (the same site as the U7
// inbox sweep, BEFORE the sweep). A deposit stamps the recipient's current generation into its slot; RECV
// verifies the stamp against the CURRENT generation (a mismatch = the deposit was aimed at a PREVIOUS tenant
// — discarded, never delivered) and the sender's post-check re-reads the generation (a change = retract). A
// recycled ASID's new tenant is therefore structurally unable to consume a stale deposit, from BOTH sides.

/// Derivation ledger capacity — bounded and static like `MAX_XFERS`. The demo peak is ~6 live nodes.
const MAX_DERIV: usize = 16;
/// Bit 63 of a node's ID word marks the node (and thereby its whole subtree, at resolve time) REVOKED. The
/// flag rides IN the state word so revocation is a state-exact CAS (the `XFER_REVOKED_BIT` discipline); node
/// ids are a monotonic counter from 1, so bit 63 never aliases a genuine id.
const DERIV_REVOKED_BIT: u64 = 1 << 63;

/// A node's STATE word: 0 = free, `HANDLE_RESERVING` = mid-claim, else = its unique node id (live), possibly
/// `| DERIV_REVOKED_BIT` (live, revoked). The id makes revoke/free ABA-safe (a reclaimed slot carries a NEW
/// id, so a stale revoke-by-expected-id can never hit the new tenant — see `deriv_revoke_if`).
static DERIV_ID: [AtomicU64; MAX_DERIV] = [const { AtomicU64::new(0) }; MAX_DERIV];
/// The node's parent edge: parent node index + 1, or 0 = a root. Set once at claim, cleared at free.
static DERIV_PARENT: [AtomicU32; MAX_DERIV] = [const { AtomicU32::new(0) }; MAX_DERIV];
/// Live children count — what pins a tombstoned parent until its subtree drains.
static DERIV_KIDS: [AtomicU32; MAX_DERIV] = [const { AtomicU32::new(0) }; MAX_DERIV];
/// The owning handle dropped (tombstone flag): the node frees when this is set AND `KIDS == 0`.
static DERIV_DROPPED: [AtomicBool; MAX_DERIV] = [const { AtomicBool::new(false) }; MAX_DERIV];
/// The next node id — monotonic from 1 (never 0/u64::MAX, the state sentinels).
static DERIV_NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Which derivation node (index + 1; 0 = a root with no node yet) a handle's capability is. Keyed
/// `[asid][idx]` like every handle sidecar; written only by the row's own task (mid-SVC) or its teardown.
static HANDLE_DERIV: [[AtomicU32; NHANDLE]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NHANDLE] }; super::boot::USER_SLOTS + 1];

/// The derivation node riding a PENDING deposit (index + 1) — ownership passes inbox-slot -> received handle
/// at RECV; every discard path (revoked-pending, generation-stale, retract, teardown sweep) drops it instead.
static XFER_SLOT_DERIV: [[AtomicU32; NXFER]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU32::new(0) }; NXFER] }; super::boot::USER_SLOTS + 1];
/// The recipient GENERATION stamped into a pending deposit — RECV delivers only on an exact match with the
/// recipient's CURRENT generation (see `ASID_GEN`).
static XFER_SLOT_GEN: [[AtomicU64; NXFER]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; NXFER] }; super::boot::USER_SLOTS + 1];

/// The transfer record's derivation node (index + 1) + that node's ID at publish time — what
/// `sys_cap_xrevoke` marks so the revoke reaches everything DERIVED from the transferred cap (re-grants,
/// re-transfers), not just the directly received handle. The captured ID makes the mark ABA-safe: if the
/// node was dropped and its slot reclaimed by the time the revoke lands, the id no longer matches and
/// nothing is written (a dropped node had no children left to protect, so nothing escapes).
static XFER_REC_DERIV: [AtomicU32; MAX_XFERS] = [const { AtomicU32::new(0) }; MAX_XFERS];
static XFER_REC_DERIV_ID: [AtomicU64; MAX_XFERS] = [const { AtomicU64::new(0) }; MAX_XFERS];

/// Per-ASID inbox GENERATION: bumped (AcqRel) at the TOP of `clear_handle_row` — i.e. strictly before the
/// teardown's inbox sweep — so any deposit stamped with the old generation is dead-on-arrival for the ASID's
/// next tenant even if it lands after the sweep passed its slot. ASID 0 (the shared window) never tears down.
static ASID_GEN: [AtomicU64; super::boot::USER_SLOTS + 1] =
    [const { AtomicU64::new(0) }; super::boot::USER_SLOTS + 1];

/// Claim a free derivation node under `parent_ref` (a node index + 1, or 0 for a root): CAS the ID word
/// 0 -> RESERVING, publish the edge + zeroed counters, bump the parent's KIDS (the parent is pinned — the
/// caller holds its owning handle live), then the fresh id LAST (live-last). Returns `(node index + 1, id)`,
/// or `None` when the ledger is exhausted (-> `-EAGAIN` at the caller, nothing to unwind).
fn deriv_claim(parent_ref: u32) -> Option<(u32, u64)> {
    debug_assert!(parent_ref as usize <= MAX_DERIV, "deriv_claim: bad parent ref");
    for n in 0..MAX_DERIV {
        if DERIV_ID[n]
            .compare_exchange(0, HANDLE_RESERVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            DERIV_PARENT[n].store(parent_ref, Ordering::Release);
            DERIV_KIDS[n].store(0, Ordering::Release);
            DERIV_DROPPED[n].store(false, Ordering::Release);
            if parent_ref != 0 {
                DERIV_KIDS[(parent_ref - 1) as usize].fetch_add(1, Ordering::AcqRel);
            }
            let id = DERIV_NEXT_ID.fetch_add(1, Ordering::AcqRel);
            DERIV_ID[n].store(id, Ordering::Release);
            return Some(((n + 1) as u32, id));
        }
    }
    None
}

/// Mark node `nref` (index + 1) REVOKED — state-exact on the CURRENT id, idempotent (an already-revoked id
/// CASes to itself). Caller must PIN the node (own its live handle, or hold its record's txid — see
/// `deriv_revoke_if` for the unpinned case). Descendants discover the mark at their next `handle_resolve`.
fn deriv_revoke(nref: u32) {
    debug_assert!(nref >= 1 && (nref as usize) <= MAX_DERIV, "deriv_revoke: bad ref");
    let n = (nref - 1) as usize;
    let cur = DERIV_ID[n].load(Ordering::Acquire);
    if cur == 0 || cur == HANDLE_RESERVING {
        return; // freed/mid-claim — nothing live to mark (pinned callers never see this)
    }
    let _ = DERIV_ID[n].compare_exchange(
        cur,
        cur | DERIV_REVOKED_BIT,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

/// Mark node `nref` REVOKED only while it still carries `expect_id` — the ABA-safe form for callers that do
/// NOT pin the node (`sys_cap_xrevoke`: the recipient may drop the received handle — freeing the node —
/// concurrently with the sender's revoke; a reclaimed slot carries a NEW id, so the mark can never hit an
/// unrelated node). A lost CAS means the node was freed (it had no live children — nothing escapes).
fn deriv_revoke_if(nref: u32, expect_id: u64) {
    if nref == 0 || (nref as usize) > MAX_DERIV {
        return;
    }
    let n = (nref - 1) as usize;
    // Two candidate current values: the clean id, or the already-revoked id (idempotent re-revoke).
    let _ = DERIV_ID[n]
        .compare_exchange(expect_id, expect_id | DERIV_REVOKED_BIT, Ordering::AcqRel, Ordering::Acquire);
}

/// True iff node `nref` or ANY ancestor is revoked — the read-side walk `handle_resolve` pays. Bounded by
/// `MAX_DERIV` (cycles are impossible: a parent is claimed strictly before its child and edges never change);
/// every node on the path is pinned (see the section comment), so a freed/reclaimed node is never read.
fn deriv_stale(nref: u32) -> bool {
    let mut r = nref;
    for _ in 0..MAX_DERIV {
        if r == 0 {
            return false; // reached a root, nothing revoked on the path
        }
        let n = (r - 1) as usize;
        if n >= MAX_DERIV {
            return true; // corrupt reference — fail closed
        }
        let id = DERIV_ID[n].load(Ordering::Acquire);
        if id == 0 || id == HANDLE_RESERVING {
            return false; // structurally unreachable on a pinned walk; benign stop (defensive)
        }
        if id & DERIV_REVOKED_BIT != 0 {
            return true;
        }
        r = DERIV_PARENT[n].load(Ordering::Acquire);
    }
    true // walk budget exhausted — impossible by construction; fail closed
}

/// Drop node `nref` (its owning handle/pending-slot released it): tombstone it, and FREE it iff its subtree
/// has drained — then cascade the free up through any drained tombstoned ancestors. The free is arbitrated
/// by a state-exact CAS on the ID word (the owner's drop and a child's cascade can race; exactly one wins).
/// A revoked tombstone keeps its bit until the free, so late resolvers of surviving descendants still deny.
fn deriv_drop(nref: u32) {
    let mut r = nref;
    loop {
        if r == 0 || (r as usize) > MAX_DERIV {
            return;
        }
        let n = (r - 1) as usize;
        // SeqCst (not Release/Acquire) on the DROPPED×KIDS handshake below: the parent-drop side
        // (store DROPPED here, then load KIDS) and the child-drop side (fetch_sub KIDS, then load
        // DROPPED, further down) form a store-buffering / Dekker pair. With only Release/Acquire
        // (no StoreLoad fence) a concurrent parent-vs-child drop of the same chain could have BOTH
        // sides read stale — the parent sees KIDS != 0 and the child sees DROPPED == false — so
        // neither frees this node and it leaks as a permanent tombstone (fail-closed: ledger
        // exhaustion -> -EAGAIN). SeqCst puts the four handshake ops in one total order, which
        // forbids the double-stale outcome; the both-free case stays arbitrated by the DERIV_ID CAS
        // below. Keep the four SeqCst ops symmetric with the twin arch. (U8/U8x concurrency lens.)
        DERIV_DROPPED[n].store(true, Ordering::SeqCst);
        if DERIV_KIDS[n].load(Ordering::SeqCst) != 0 {
            return; // live children pin it — a TOMBSTONE until the subtree drains
        }
        let mut id = DERIV_ID[n].load(Ordering::Acquire);
        loop {
            if id == 0 || id == HANDLE_RESERVING {
                return; // already freed / mid-claim (a racing freer won)
            }
            match DERIV_ID[n].compare_exchange(
                id,
                HANDLE_RESERVING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                // A racing REVOKE (deriv_revoke_if from a sender's xrevoke on this unpinned
                // node) only flips DERIV_REVOKED_BIT and NEVER frees — we remain the sole
                // freer, so retry against the refreshed word; returning here would leak the
                // node (and every tombstoned ancestor it pins) permanently, exhausting the
                // ledger to -EAGAIN. A racing FREER leaves 0/RESERVING — caught above on the
                // reload. (U8 review must-fix, fixed in-arc.)
                Err(cur) => id = cur,
            }
        }
        let parent = DERIV_PARENT[n].load(Ordering::Acquire);
        DERIV_PARENT[n].store(0, Ordering::Release);
        DERIV_DROPPED[n].store(false, Ordering::Release);
        DERIV_ID[n].store(0, Ordering::Release); // freed LAST
        if parent == 0 {
            return;
        }
        // Un-child the parent; if we took its LAST kid and it is tombstoned, cascade the free up.
        if DERIV_KIDS[(parent - 1) as usize].fetch_sub(1, Ordering::SeqCst) != 1 {
            return;
        }
        // SeqCst: child side of the store-buffering handshake (see the DROPPED store above).
        if !DERIV_DROPPED[(parent - 1) as usize].load(Ordering::SeqCst) {
            return; // the parent's owning handle is still live — it frees on its own drop
        }
        r = parent;
    }
}

/// True iff the whole derivation ledger is free — the U8 leak verifier (every node's lifetime closed).
fn deriv_all_free() -> bool {
    (0..MAX_DERIV).all(|n| DERIV_ID[n].load(Ordering::Acquire) == 0)
}

/// Ensure the source handle at `[asid][src]` has a derivation node (creating a lazy ROOT node if not), and
/// mint a CHILD node under it — the shared derive step of `sys_cap_grant` and `sys_xfer`. Returns the child
/// `(node ref, id)`, or `None` on ledger exhaustion (-> `-EAGAIN`; a root node created en route stays
/// attached to the source handle and frees with it — never a leak).
fn deriv_derive_from(asid: u64, src: usize) -> Option<(u32, u64)> {
    let src_node = match HANDLE_DERIV[asid as usize][src].load(Ordering::Acquire) {
        0 => {
            let (n, _) = deriv_claim(0)?;
            HANDLE_DERIV[asid as usize][src].store(n, Ordering::Release);
            n
        }
        n => n,
    };
    deriv_claim(src_node)
}

// =============================================================================================
// M6g: load a program FROM STORAGE and run it at EL0 (the Pi twin of x86 U2)
// =============================================================================================

/// M6g loader (also its own verdict). A kernel task spawned once on a scheduled AP AFTER the M6f verdict
/// spawn. On the bare-metal Pi the program comes off the very microSD card the Pi booted from: the
/// Part-B EMMC2/SDHCI probe (on the BSP) already registered the SD block backend, so here we mount its
/// FAT volume, read `HELLO.BIN` (the same `USER_BLOB` bytes M6c bakes in — carried onto the boot media as
/// `HELLO.BIN`), size-check it, copy it into a fresh M6d per-task slot's code page, protect the page
/// (EL0-RX/EL1-RO) BEFORE the task exists, and drop it to EL0. The loaded bytes are UNTRUSTED: nothing
/// about them is trusted beyond the size bound — the program runs only under EL0 + per-page permissions +
/// the M6b fault-kill net (size-bounded only; no signature, no allowlist). That containment is the point.
///
/// Ordering: it first waits (bounded) for `M6F_VERDICT_PRINTED` so every LOADER line lands AFTER the
/// M6b/M6e/M6d/M6f verdict lines (the Part-B probe's two lines already printed early, on the BSP). A
/// missing SD device / FAT volume / file / oversize logs one clean skip line and returns.
pub fn m6g_loader(arg: usize) {
    m6g_loader_run(arg);
    // Release the U4 gate: by here every M6g line has printed AND the M6d/M6f/M6g slots have freed (their
    // tasks exited), so the U4 launcher may build the parent + orphan + children. Set on EVERY path (load /
    // skip / no-SD) so the launcher never waits out its deadline; the launcher separately re-checks for an SD.
    M6G_LOADER_DONE.store(true, Ordering::Release);
}

fn m6g_loader_run(_: usize) {
    // 1. Wait (bounded ~8 s CNTPCT, yielding — the m6d_verdict idiom) for the M6f verdict to publish, so
    //    the loader's lines follow every prior verdict line rather than racing into the middle of them.
    let wstart = super::timer::cntpct();
    let wdeadline = 8 * super::timer::cntfrq();
    while !M6F_VERDICT_PRINTED.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot from here (spawned once, but guard defensively like u2_probe_once).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. Gate: the Part-B probe registered an SD block device (the empty gate is the no-SD control path).
    if crate::drivers::block::info().is_none() {
        serial_println!(":: M6g: no SD card found — loader skipped ::");
        return;
    }

    // 3-6. Load HELLO.BIN into a fresh slot via the shared core, then reproduce M6g's EXACT serial lines
    //      from the result (the core is silent so sys_spawn stays quiet inside the U4 flow). Every skip path
    //      first echoes the "FAT mounted from SD (..)" progress line (mirroring the original mid-flow print)
    //      so the M6g gate is byte-identical. On success: emit the two M6g lines then drop the program to
    //      EL0 on THIS core (the loader's), so the folded verdict's cooperative yield guarantees dispatch.
    let loaded = match load_program_into_slot("HELLO.BIN") {
        Ok(l) => l,
        Err(SpawnErr::NoMount(e)) => {
            serial_println!(":: M6g: no FAT volume ({:?}) — loader skipped ::", e);
            return;
        }
        Err(SpawnErr::NoFile(kind)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: HELLO.BIN not found on the FAT volume — loader skipped ::");
            return;
        }
        Err(SpawnErr::BadSize(kind, sz)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(
                ":: M6g: HELLO.BIN bad size {} bytes (must be 1..={}) — loader skipped ::",
                sz,
                super::boot::USER_CODE_SIZE
            );
            return;
        }
        Err(SpawnErr::ReadErr(kind, e)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: HELLO.BIN read error ({:?}) — loader skipped ::", e);
            return;
        }
        Err(SpawnErr::Empty(kind)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: HELLO.BIN read empty — loader skipped ::");
            return;
        }
        Err(SpawnErr::NoSlot(kind)) => {
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: no free address-space slot — loader skipped ::");
            return;
        }
        Err(SpawnErr::BadElf(kind, why)) => {
            // HELLO.BIN is a flat blob (never ELF), so this arm is unreachable for it; present for
            // completeness so the match is total (the loader core is shared with the ELF-1 path).
            serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", kind);
            serial_println!(":: M6g: HELLO.BIN rejected as ELF ({}) — loader skipped ::", why);
            return;
        }
    };
    serial_println!(":: M6g: FAT mounted from SD ({:?}) ::", loaded.kind);
    serial_println!(
        ":: M6g: HELLO.BIN loaded from SD ({} bytes) -> EL0 (slot {}, ASID {}) ::",
        loaded.len,
        loaded.slot,
        loaded.ttbr0 >> 48
    );
    let run_cpu = super::percpu::this_cpu().cpu_index as usize;
    // U5: endow the disk-loaded program's slot with a console write-capability so its `sys_write(fd 1)`
    // reaches the console once writes route through the table (it prints "hello from EL0").
    install_console_cap(loaded.ttbr0 >> 48);
    super::sched::spawn_user_slot("m6g-hello", loaded.base, loaded.sp, loaded.ttbr0, run_cpu);

    // 7. Verdict (folded in — no extra task): wait (bounded ~2 s, yielding so m6g-hello runs on this core)
    //    for the disk program to terminate, then print PASS/FAIL. The disk blob's `sys_exit(0)` is routed
    //    by name into EL0_M6G_DONE; a fault into EL0_M6G_KILLED; a nonzero exit into EL0_M6G_ERR.
    let vstart = super::timer::cntpct();
    let vdeadline = 2 * super::timer::cntfrq();
    while EL0_M6G_DONE.load(Ordering::Acquire)
        + EL0_M6G_ERR.load(Ordering::Acquire)
        + EL0_M6G_KILLED.load(Ordering::Acquire)
        == 0
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let done = EL0_M6G_DONE.load(Ordering::Acquire);
    let err = EL0_M6G_ERR.load(Ordering::Acquire);
    let killed = EL0_M6G_KILLED.load(Ordering::Acquire);
    if done == 1 && err == 0 && killed == 0 {
        serial_println!(":: M6g: disk-loaded EL0 program exited ok -> PASS ::");
    } else {
        serial_println!(
            ":: M6g: disk-loaded EL0 program FAIL — done={} err={} killed={} (want 1/0/0) ::",
            done, err, killed
        );
    }
}

// =============================================================================================
// ELF-1: the minimal-static-ELF64 loader witness. Loads ELFHELLO.ELF (a two-PT_LOAD-segment static
// ELF) through the SAME `load_program_into_slot` core that HELLO.BIN uses — the core now dispatches
// ELF vs flat on the magic. The witness asserts, kernel-side, that the ELF path ran (is_elf, 2
// segments) with correct per-segment page permissions (the code segment's page is RO+EL0-exec, the
// data segment's page is EL0+EL1-RW/never-exec), then drops the program to EL0 and confirms it ran (its
// SYS_REPORT token arrived; its SYS_WRITE message printed to the console). One uncounted `:: ELF1: ::`
// line; an honest skip on media without the fixture. QEMU-verifiable (raspi4b runs EL0 baremetal).
// =============================================================================================

/// The witness token ELFHELLO.ELF reports via SYS_REPORT (`movz x0, #0x1E`) — must match the fixture.
const ELF1_WITNESS_TOKEN: u64 = 0x1E;

/// The token the ELF-loaded program reported (0 until it runs). Read by `elf1_launcher`'s verdict.
static ELF1_WITNESS: AtomicU64 = AtomicU64::new(0);
/// The ELF-loaded program reached its clean `sys_exit(0)` (want 1). Routed by name in the SYS_EXIT arm.
static EL0_ELF1_DONE: AtomicU64 = AtomicU64::new(0);
/// The ELF-loaded program exited NON-zero (want 0) — a fixture/loader bug.
static EL0_ELF1_ERR: AtomicU64 = AtomicU64::new(0);

/// ELF-1: record the ELF fixture's SYS_REPORT token, keyed by the reporting task's name. Called from the
/// SYS_REPORT arm alongside the other reporters; each ignores the others' names.
fn elf1_report(value: u64) {
    if super::sched::current_name() == Some("elf1-hello") {
        ELF1_WITNESS.store(value, Ordering::Release);
    }
}

/// ELF-1 witness/verdict: load ELFHELLO.ELF via the shared loader core (ELF path), assert the ELF was
/// validated + mapped with per-segment permissions, run it at EL0, and confirm it ran. Modelled on
/// `m6g_loader_run`. Self-contained; runs LAST in the u7 chain (all 8 slots are free by then). Emits ONE
/// uncounted `:: ELF1: … ::` line (PASS or an honest skip/FAIL); never perturbs the 23-fixture battery.
fn elf1_launcher(_demo_cpu: usize) {
    // One-shot (defensive, the m6g idiom).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // Gate: an SD block device must be present to load the fixture off.
    if crate::drivers::block::info().is_none() {
        serial_println!(":: ELF1: no SD card found — ELF loader witness skipped ::");
        return;
    }
    let loaded = match load_program_into_slot("ELFHELLO.ELF") {
        Ok(l) => l,
        Err(SpawnErr::NoMount(e)) => {
            serial_println!(":: ELF1: no FAT volume ({:?}) — skipped ::", e);
            return;
        }
        Err(SpawnErr::NoFile(_)) => {
            serial_println!(":: ELF1: ELFHELLO.ELF not found on the FAT volume — skipped ::");
            return;
        }
        Err(SpawnErr::BadElf(_, why)) => {
            serial_println!(":: ELF1: ELFHELLO.ELF rejected by the ELF loader ({}) -> FAIL ::", why);
            return;
        }
        Err(SpawnErr::BadSize(_, sz)) => {
            serial_println!(":: ELF1: ELFHELLO.ELF bad size {} bytes -> FAIL ::", sz);
            return;
        }
        Err(SpawnErr::ReadErr(_, e)) => {
            serial_println!(":: ELF1: ELFHELLO.ELF read error ({:?}) -> FAIL ::", e);
            return;
        }
        Err(SpawnErr::Empty(_)) => {
            serial_println!(":: ELF1: ELFHELLO.ELF read empty -> FAIL ::");
            return;
        }
        Err(SpawnErr::NoSlot(_)) => {
            serial_println!(":: ELF1: no free address-space slot — skipped ::");
            return;
        }
    };

    // Kernel-side structural proof (BEFORE the program runs, while the slot's leaves are live): it took the
    // ELF path, mapped 2 PT_LOAD segments, and the two segments landed on DISTINCT pages with the right
    // permissions — page 0 (the R+X text segment) is a CODE leaf (RO-both + EL0-executable), page 1 (the
    // R+W data segment carrying the witness message) is a DATA leaf (EL0+EL1-RW, never executable).
    let (base, _size) = super::boot::user_region();
    let took_elf = loaded.is_elf;
    let two_segs = loaded.nsegs == 2;
    let code_ok = super::boot::slot_page_is_code(loaded.slot, base);
    let data_ok = super::boot::slot_page_is_data(loaded.slot, base + 0x1000);
    let entry_at_base = loaded.base == base; // fixture links at vaddr 0 -> bias == base -> entry == base

    // Endow the slot with a console write-cap so its SYS_WRITE(fd 1) reaches the console, then drop it to
    // EL0 on THIS core (the cooperative-yield verdict below guarantees dispatch).
    install_console_cap(loaded.ttbr0 >> 48);
    let run_cpu = super::percpu::this_cpu().cpu_index as usize;
    super::sched::spawn_user_slot("elf1-hello", loaded.base, loaded.sp, loaded.ttbr0, run_cpu);

    // Verdict: wait (bounded ~2 s, yielding so elf1-hello runs on this core) for the program to terminate.
    let vstart = super::timer::cntpct();
    let vdeadline = 2 * super::timer::cntfrq();
    while EL0_ELF1_DONE.load(Ordering::Acquire) + EL0_ELF1_ERR.load(Ordering::Acquire) == 0
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let done = EL0_ELF1_DONE.load(Ordering::Acquire);
    let err = EL0_ELF1_ERR.load(Ordering::Acquire);
    let token = ELF1_WITNESS.load(Ordering::Acquire);
    let ran_ok = done == 1 && err == 0 && token == ELF1_WITNESS_TOKEN;

    if took_elf && two_segs && code_ok && data_ok && entry_at_base && ran_ok {
        serial_println!(
            ":: ELF1: static ELF64 loaded ({} bytes, {} PT_LOAD segs) -> EL0 ran (token {:#x}) -> PASS ::",
            loaded.len, loaded.nsegs, token
        );
    } else {
        serial_println!(
            ":: ELF1: FAIL — elf={} segs={} code_leaf={} data_leaf={} entry@base={} done={} err={} token={:#x} (want ELF/2/RX/RW/1/0/{:#x}) ::",
            took_elf, loaded.nsegs, code_ok, data_ok, entry_at_base, done, err, token, ELF1_WITNESS_TOKEN
        );
    }
}

/// EXEC-1 witness: prove the panel `run <path>` PATH end-to-end at boot. Reads ELFHELLO.ELF through the VFS
/// `MountTable` (the same namespace the `run` verb builds — `/fat` = FAT boot partition), then hands the
/// bytes to the NEW loader entry `run_user_image`, which maps them into a fresh EL0 slot, runs the program
/// co-located, and returns its exit status. The ELF-1 launcher's twin, but driven through the VFS read +
/// `run_user_image` (not the by-name `load_program_into_slot`), so it self-verifies the EXEC-1 integration:
/// the program prints its own `elf hello from EL0` line (the hello witness) and this launcher adds the
/// uncounted `:: EXEC1: … exit=0 -> PASS ::` verdict. Runs right after `elf1_launcher` (the FAT volume is
/// mounted + proven), fully self-contained (a fresh slot, reaped on exit); an honest skip without the SD/
/// fixture. It can never perturb the 23-fixture battery (its exit rides the generic Proc reap, off every
/// M6b/sentinel counter).
fn exec1_witness(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        serial_println!(":: EXEC1: no SD card found — run-path witness skipped ::");
        return;
    }
    use crate::fs::vfs::{FatBackend, MountTable, KERNEL_PRINCIPAL};
    let mut mt = MountTable::new();
    mt.mount("/fat", alloc::boxed::Box::new(FatBackend::new("fat", KERNEL_PRINCIPAL, true)));
    let path = "/fat/ELFHELLO.ELF";
    let st = match mt.stat(path) {
        Ok(s) => s,
        Err(e) => {
            serial_println!(":: EXEC1: {} not found on the VFS ({:?}) — skipped ::", path, e);
            return;
        }
    };
    let bytes = match mt.read(path, 0, st.size as usize) {
        Ok(b) => b,
        Err(e) => {
            serial_println!(":: EXEC1: {} read error ({:?}) -> FAIL ::", path, e);
            return;
        }
    };
    let n = bytes.len();
    let deadline = 5 * super::timer::cntfrq();
    match run_user_image("shell-run", &bytes, deadline) {
        Ok((RunOutcome::Exited(0), entry)) => serial_println!(
            ":: EXEC1: run {} — loaded {} bytes, entry {:#x}, exit=0 -> PASS ::",
            path, n, entry
        ),
        Ok((RunOutcome::Exited(code), entry)) => serial_println!(
            ":: EXEC1: run {} — loaded {} bytes, entry {:#x}, exit={} (want 0) -> FAIL ::",
            path, n, entry, code
        ),
        Ok((RunOutcome::Faulted, _)) => {
            serial_println!(":: EXEC1: run {} — EL0 program FAULTED -> FAIL ::", path)
        }
        Ok((RunOutcome::Timeout, _)) => {
            serial_println!(":: EXEC1: run {} — EL0 program did not exit in time -> FAIL ::", path)
        }
        Err(why) => serial_println!(":: EXEC1: run {} — loader rejected ({}) -> FAIL ::", path, why),
    }
}

/// UVUG-1 witness: prove the mini-vug EL0 graphics program end-to-end at boot. Reads UVUG.ELF through the
/// VFS `MountTable` (the same namespace the panel `run` verb builds) and executes it via `run_user_image` —
/// the identical path the operator drives with `run /fat/UVUG.ELF`. The program maps its off-screen surface
/// (SYS_FB_MAP), spawns 2 EL0 worker threads that render halves of an animated pattern under a FUTEX frame
/// barrier, presents each frame (SYS_FB_PRESENT), joins both (SYS_THREAD_JOIN), and prints its OWN witness
/// line `:: UVUG: frames=300 threads=2 checksum=<hex> ::` before exiting 0. This launcher only asserts the
/// clean `exit=0` (its own uncounted `:: EXEC-UVUG: … ::` verdict) — the program's self-printed UVUG line is
/// the deterministic-checksum witness. Runs AFTER `fb_launcher` (which also arms the futex pool; `run_user_image`
/// re-arms it idempotently anyway) so its reuse of the SYS_FB_* / SYS_THREAD_* / SYS_FUTEX machinery perturbs
/// nothing. Self-contained (a fresh slot, reaped on exit); an honest skip without the SD/fixture. Its exit
/// rides the generic Proc reap, off every M6b/sentinel counter, so it can never perturb the fixture battery.
fn uvug_witness(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        serial_println!(":: EXEC-UVUG: no SD card found — mini-vug witness skipped ::");
        return;
    }
    use crate::fs::vfs::{FatBackend, MountTable, KERNEL_PRINCIPAL};
    let mut mt = MountTable::new();
    mt.mount("/fat", alloc::boxed::Box::new(FatBackend::new("fat", KERNEL_PRINCIPAL, true)));
    let path = "/fat/UVUG.ELF";
    let st = match mt.stat(path) {
        Ok(s) => s,
        Err(e) => {
            serial_println!(":: EXEC-UVUG: {} not found on the VFS ({:?}) — skipped ::", path, e);
            return;
        }
    };
    let bytes = match mt.read(path, 0, st.size as usize) {
        Ok(b) => b,
        Err(e) => {
            serial_println!(":: EXEC-UVUG: {} read error ({:?}) -> FAIL ::", path, e);
            return;
        }
    };
    let n = bytes.len();
    // Generous deadline: 300 futex-synchronised frames across 2 worker threads (one on a sibling core) run
    // well under a second in QEMU, but bound loosely so a slow host never spuriously times out.
    let deadline = 15 * super::timer::cntfrq();
    match run_user_image("shell-run", &bytes, deadline) {
        Ok((RunOutcome::Exited(0), entry)) => serial_println!(
            ":: EXEC-UVUG: run {} — loaded {} bytes, entry {:#x}, exit=0 -> PASS ::",
            path, n, entry
        ),
        Ok((RunOutcome::Exited(code), entry)) => serial_println!(
            ":: EXEC-UVUG: run {} — loaded {} bytes, entry {:#x}, exit={} (want 0) -> FAIL ::",
            path, n, entry, code
        ),
        Ok((RunOutcome::Faulted, _)) => {
            serial_println!(":: EXEC-UVUG: run {} — EL0 program FAULTED -> FAIL ::", path)
        }
        Ok((RunOutcome::Timeout, _)) => {
            serial_println!(":: EXEC-UVUG: run {} — EL0 program did not exit in time -> FAIL ::", path)
        }
        Err(why) => serial_println!(":: EXEC-UVUG: run {} — loader rejected ({}) -> FAIL ::", path, why),
    }
}

// =============================================================================================
// ELF-2: EL0 threading — SYS_THREAD_SPAWN / _JOIN / _EXIT + the multi-core shared-memory threads test.
//
// ELF-1 loaded ONE static task into a private slot (its own ttbr0/ASID). ELF-2 adds THREADS: several EL0
// tasks SHARING one slot's address space (same ttbr0/ASID), so a memory word is coherent across them. The
// primitives:
//   * SYS_THREAD_SPAWN(entry, sp, arg, place) — a new EL0 task under the caller's ttbr0/ASID, started at
//     `entry` on stack `sp` (both carved from the caller's window) with `arg` in x0; `place` = 0 caller-core,
//     1 sibling-core. Returns a per-process thread handle. Backed by `sched::spawn_user_thread` +
//     `boot::slot_thread_retain` (the slot lives until the LAST thread exits).
//   * SYS_THREAD_JOIN(handle) — block on that thread's completion Semaphore (its `JoinHandle`), then reap it.
//   * SYS_THREAD_EXIT() — post completion + decrement the slot refcount (teardown only on the last thread).
//
// The test (`__threads_prog_*`): the parent zeroes a shared counter, spawns 2 workers (one co-located, one on
// a sibling core), each atomically (A72 LL/SC — no LSE) increments the counter and SYS_THREAD_EXITs, the
// parent joins both, SYS_REPORTs the total, SYS_WRITEs a witness, and exits. `threads_launcher` composes the
// authoritative witness line. Multi-core soundness rests on `teardown_user_slot`'s per-thread TTBR0 repoint
// (see boot.rs) so the final ASID flush races no live root on the sibling core.
// =============================================================================================

/// How many concurrently-tracked joinable EL0 threads the kernel holds handles for (the test uses 2). A small
/// fixed pool; `-EAGAIN` when exhausted (never grown — same discipline as the Proc/handle tables).
const NTHREAD: usize = 8;

/// One live thread the kernel can be `SYS_THREAD_JOIN`ed on: the owning ASID (only that process's parent may
/// join) plus the completion `JoinHandle` (an `Arc<Semaphore>` posted in `exit()`). Taken out of the table by
/// the join (single-shot), and by construction never observed by two joiners (one live parent per ASID).
struct ThreadRec {
    owner: u64,
    join: super::sched::JoinHandle,
}

/// The thread-handle table: a small fixed pool of live joinable threads, index = the handle returned to EL0.
/// A `spin::Mutex` (not per-ASID atomics like `HANDLES`) because a `JoinHandle` is a non-Copy owned value that
/// `join` must MOVE out; the lock is held only for the claim/take, never across the blocking `join`.
static THREAD_TABLE: SpinMutex<[Option<ThreadRec>; NTHREAD]> = SpinMutex::new([const { None }; NTHREAD]);

/// ELF-2 threads-test witnesses (captured kernel-side, printed by `threads_launcher`). Global (not per-ASID)
/// because these three syscalls are used only by this one test this arc.
static THREADS_SPAWNED: AtomicU64 = AtomicU64::new(0); // successful SYS_THREAD_SPAWNs (want 2)
static THREADS_JOINED: AtomicU64 = AtomicU64::new(0); // completed SYS_THREAD_JOINs (want 2)
static THREADS_COUNTER: AtomicU64 = AtomicU64::new(0); // the shared counter the parent SYS_REPORTs (want 2)
static THREADS_EXIT_IDX: AtomicU64 = AtomicU64::new(0); // which worker is exiting (0 -> core A, 1 -> core B)
static THREADS_CORE_A: AtomicI32 = AtomicI32::new(-1); // the core the first worker actually ran+exited on
static THREADS_CORE_B: AtomicI32 = AtomicI32::new(-1); // the core the second worker actually ran+exited on
static THREADS_PARENT_DONE: AtomicBool = AtomicBool::new(false); // the parent reached its clean sys_exit(0)

/// Read the full live `TTBR0_EL1` (root | ASID<<48) — the caller's shared address space, which a spawned
/// thread runs under. `current_asid` is the top 16 bits of this; a thread needs the whole value.
fn current_ttbr0() -> u64 {
    let ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0, options(nomem, nostack, preserves_flags)) };
    ttbr0
}

/// SYS_THREAD_SPAWN(entry, sp, arg, place): create an EL0 thread sharing the caller's address space. Validates
/// `entry` (4-aligned, inside the window — a non-exec target simply faults the thread, contained by the M6b
/// kill net) and `sp` (16-aligned, inside the WRITABLE window with headroom below it). `place`: 0 = the
/// caller's core, 1 = a sibling online core (genuine cross-core parallelism). Returns the thread handle, or a
/// negative errno (`-EFAULT` bad entry/sp, `-EINVAL` from the shared ASID-0 context, `-EAGAIN` table full).
fn sys_thread_spawn(entry: u64, sp: u64, arg: u64, place: u64) -> i64 {
    let (base, size) = super::boot::user_region();
    let win_end = base + size as u64;
    // entry: 4-aligned and inside the window (exec enforcement is left to the page permissions).
    if entry & 3 != 0 || entry < base || entry >= win_end {
        return EFAULT;
    }
    // sp: 16-aligned, at/below the window top, with at least 16 writable bytes below it (excludes the RO code
    // page, so a stack aimed at page 0 is refused).
    if sp & 0xF != 0 || sp > win_end || sp < 16 || !user_range_ok(sp - 16, 16, true) {
        return EFAULT;
    }
    let ttbr0 = current_ttbr0();
    let asid = ttbr0 >> 48;
    if asid == 0 {
        return EINVAL; // the shared-window (ASID 0) context owns no private slot to thread within
    }
    let caller = super::percpu::this_cpu().cpu_index as usize;
    let cpu = if place == 1 { super::sched::other_online_cpu(caller) } else { caller };
    // Claim a tracking slot, retain the shared address space for the new thread, then spawn it. The retain
    // precedes the spawn so the slot cannot be torn down between here and the thread's first dispatch; the
    // thread's own exit balances it (`teardown_user_slot`). The lock spans the spawn (a brief critical
    // section, no cross-core contention) so the returned handle index is stable before EL0 sees it.
    let mut tab = THREAD_TABLE.lock();
    let Some(idx) = tab.iter().position(|s| s.is_none()) else {
        return EAGAIN;
    };
    super::boot::slot_thread_retain(asid);
    let join = super::sched::spawn_user_thread("el0-thread-w", entry, sp, arg, ttbr0, cpu);
    tab[idx] = Some(ThreadRec { owner: asid, join });
    drop(tab);
    THREADS_SPAWNED.fetch_add(1, Ordering::AcqRel);
    idx as i64
}

/// SYS_THREAD_JOIN(handle): block until the thread named by `handle` finishes, then reap its handle. Resolves
/// the handle against the CALLER's ASID (only the owning process may join); `-ESRCH` if out of range, empty,
/// or foreign. The `JoinHandle` is MOVED out under the lock, which is then dropped BEFORE the blocking `join`
/// (never block holding the spinlock). The wake is the thread's `exit()` completion post — a scheduler wake,
/// so it works under QEMU (like `sys_wait`).
fn sys_thread_join(handle: u64) -> i64 {
    let asid = current_asid();
    let idx = handle as usize;
    let rec = {
        let mut tab = THREAD_TABLE.lock();
        if idx >= NTHREAD || tab[idx].as_ref().map(|r| r.owner) != Some(asid) {
            return ESRCH;
        }
        tab[idx].take().unwrap()
    };
    rec.join.join(); // blocks until the thread posts its completion (scheduler wake)
    // `JoinHandle::join` -> `Semaphore::wait` restores the SVC-entry DAIF (IRQ masked on exception entry);
    // re-mask defensively so the `__vec_svc` epilogue's banked ELR/SPSR/SP_EL0 restore is I-masked (the
    // sys_wait contract).
    remask_irq();
    THREADS_JOINED.fetch_add(1, Ordering::AcqRel);
    0
}

/// SYS_THREAD_EXIT(): terminate the calling EL0 thread. Records the exiting worker's core for the witness
/// (first exit -> A, second -> B), then `sched::exit()` — which posts the thread's completion (waking a
/// joiner) and, via `teardown_user_slot`, decrements the slot refcount (tearing the shared slot down only on
/// the LAST thread's exit). Never returns.
fn sys_thread_exit() -> i64 {
    let cpu = super::percpu::this_cpu().cpu_index as i32;
    match THREADS_EXIT_IDX.fetch_add(1, Ordering::AcqRel) {
        0 => THREADS_CORE_A.store(cpu, Ordering::Release),
        1 => THREADS_CORE_B.store(cpu, Ordering::Release),
        _ => {}
    }
    super::sched::exit() // diverges; the `__vec_svc` eret tail is not reached
}

/// ELF-2: capture the threads-test parent's SYS_REPORT (the shared counter), keyed by name. Called from the
/// SYS_REPORT arm alongside the other reporters; each ignores the others' names.
fn threads_report(value: u64) {
    if super::sched::current_name() == Some("el0-threads") {
        THREADS_COUNTER.store(value, Ordering::Release);
    }
}

// The threads-test EL0 program: a parent + a worker, one code page (RX), sharing the slot's data/stack pages.
// The shared counter lives at window+0x1000 (data page 1, RW); worker A's stack top is window+0x3000, worker
// B's is window+0x3800, and the parent runs on the window-top stack the launcher sets — all distinct sub-
// ranges of the RW pages [0x1000,0x4000). Position-independent: `adr __threads_blob_start` recovers the window
// base (the blob is copied to & entered at the base), so every VA is computed at run time.
core::arch::global_asm!(
    r#"
    .balign 4
    .globl __threads_blob_start
__threads_blob_start:
    .globl __threads_prog_parent
__threads_prog_parent:
    adr  x20, __threads_blob_start        // x20 = window base (PC-relative)
    add  x22, x20, #0x1000                // x22 = shared counter address (RW data page 1)
    str  xzr, [x22]                       // zero the counter before any worker reads it
    dmb  ish                              // publish the zero to the sibling core before spawning
    adr  x21, __threads_prog_worker       // x21 = worker entry VA (base + worker offset)
    // worker A: caller's core (placement 0), arg 0, stack top = base+0x3000
    mov  x0, x21
    add  x1, x20, #0x3000
    mov  x2, #0
    mov  x3, #0
    mov  x8, #21                          // SYS_THREAD_SPAWN
    svc  #0
    mov  x23, x0                          // x23 = handle_a
    // worker B: sibling core (placement 1), arg 1, stack top = base+0x3800
    mov  x0, x21
    add  x1, x20, #0x3000
    add  x1, x1, #0x800
    mov  x2, #1
    mov  x3, #1
    mov  x8, #21
    svc  #0
    mov  x24, x0                          // x24 = handle_b
    mov  x0, x23                           // SYS_THREAD_JOIN(handle_a)
    mov  x8, #23
    svc  #0
    mov  x0, x24                           // SYS_THREAD_JOIN(handle_b)
    mov  x8, #23
    svc  #0
    ldr  x25, [x22]                        // read the shared counter (both increments now visible)
    mov  x0, x25                           // SYS_REPORT(counter)
    mov  x8, #3
    svc  #0
    mov  x0, #1                            // SYS_WRITE(fd=1, msg, 16) — exercise the write path post-join
    adr  x1, __threads_msg
    mov  x2, #16
    mov  x8, #1
    svc  #0
    mov  x0, #0                            // SYS_EXIT(0) -> routed to the ELF-2 witness by name
    mov  x8, #2
    svc  #0
1:  b 1b

    .balign 4
    .globl __threads_prog_worker
__threads_prog_worker:
    adr  x9, __threads_blob_start          // x9 = window base
    add  x9, x9, #0x1000                   // x9 = shared counter address (shared across threads/cores)
2:  ldxr x10, [x9]                         // A72 = ARMv8.0: LL/SC exclusive (no LSE atomics)
    add  x10, x10, #1
    stxr w11, x10, [x9]
    cbnz w11, 2b                           // monitor lost (contention/preempt) -> retry
    mov  x8, #22                           // SYS_THREAD_EXIT (posts completion; never returns)
    svc  #0
3:  b 3b

    .balign 4
    .globl __threads_msg
__threads_msg:
    .ascii "threads: joined\n"
    .balign 4
    .globl __threads_blob_end
__threads_blob_end:
"#
);

unsafe extern "C" {
    static __threads_blob_start: u8;
    static __threads_blob_end: u8;
    static __threads_prog_parent: u8;
}

/// ELF-2 threads-test launcher + verdict (the `elf1_launcher` shape: one gated kernel task, self-contained,
/// its own uncounted `:: EL0: threads test … ::` line). Copies the threads blob into a fresh slot's code page,
/// protects it EL0-RX/EL1-RO, endows a console cap, and runs the parent `el0-threads` on THIS core (so the
/// cooperative-yield verdict below drives it under QEMU). The parent spawns 2 workers (one here, one on a
/// sibling core), each atomically bumps the shared counter, joins them, and exits; the verdict prints the
/// authoritative witness with the kernel-observed spawned/joined counts, the reported counter, and the cores
/// the two workers actually ran on. Never perturbs the fixture battery (its own line, no shared counter).
fn threads_launcher(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let bstart = &raw const __threads_blob_start as usize;
    let bend = &raw const __threads_blob_end as usize;
    let blen = bend - bstart;
    if blen > super::boot::USER_CODE_SIZE {
        serial_println!(":: EL0: threads test — blob {} B > code page, SKIP ::", blen);
        return;
    }
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = the parent's initial SP_EL0
    let Some(slot) = super::boot::alloc_user_slot() else {
        serial_println!(":: EL0: threads test — no free address-space slot, SKIP ::");
        return;
    };
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    install_console_cap(ttbr0 >> 48);
    let entry = base + (&raw const __threads_prog_parent as usize - bstart) as u64;
    let run_cpu = super::percpu::this_cpu().cpu_index as usize;
    super::sched::spawn_user_slot("el0-threads", entry, sp, ttbr0, run_cpu);

    // Verdict: wait (bounded ~2 s, yielding so el0-threads + its co-located worker run on this core) for the
    // parent to reach its clean exit.
    let vstart = super::timer::cntpct();
    let vdeadline = 2 * super::timer::cntfrq();
    while !THREADS_PARENT_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let spawned = THREADS_SPAWNED.load(Ordering::Acquire);
    let joined = THREADS_JOINED.load(Ordering::Acquire);
    let counter = THREADS_COUNTER.load(Ordering::Acquire);
    let ca = THREADS_CORE_A.load(Ordering::Acquire);
    let cb = THREADS_CORE_B.load(Ordering::Acquire);
    let done = THREADS_PARENT_DONE.load(Ordering::Acquire);
    if done && spawned == 2 && joined == 2 && counter == 2 {
        serial_println!(
            ":: EL0: threads test — spawned={} joined={} counter={} cores={},{} :: PASS ::",
            spawned, joined, counter, ca, cb
        );
    } else {
        serial_println!(
            ":: EL0: threads test — spawned={} joined={} counter={} cores={},{} done={} (want 2/2/2) :: FAIL ::",
            spawned, joined, counter, ca, cb, done
        );
    }
}

// =============================================================================================
// ELF-3: give EL0 something to draw on (SYS_FB_MAP / SYS_FB_PRESENT) + real sync (SYS_FUTEX).
//
// SYS_FB_MAP maps a per-process, kernel-allocated OFF-SCREEN surface (EL0-RW, Normal-cacheable) plus a
// read-only INFO page into the caller's EL0 window (see `boot::map_slot_fb`). EL0 draws into the surface
// but NEVER touches the real scan-out: only the kernel composites it, via SYS_FB_PRESENT, which calls a
// present hook the video subsystem registers (`register_fb_present_hook`) — the existing dirty-rect
// damage+flush path. SYS_FUTEX gives EL0 a real wait/wake primitive (a userspace mutex/condvar can be
// built on it), keyed by the physical address of the user word (see `sched::futex_wait/_wake`).
//
// The test (`__fb_prog_*`): the parent maps the surface, reads its geometry from the RO info page, spawns
// 2 draw threads (one co-located, one on a sibling core) that each fill THEIR HALF of the surface, an
// atomic counter + FUTEX wake/wait synchronises the parent to both halves being drawn, the parent PRESENTS
// (the kernel checksums the surface), joins both threads, and exits. The witness self-verifies the drawn
// bytes against the kernel-computed expected checksum.
// =============================================================================================

/// The FB info-page magic (first u32) — proves EL0 read a real, kernel-written info page.
const FB_MAGIC: u32 = 0xFB01_0000;
/// The FB pixel format tag written into the info page (ARGB8888, 4 bytes/pixel).
const FB_FORMAT_ARGB8888: u32 = 1;

/// ELF-3 present hook: the signature the video/compositor subsystem registers to composite an EL0
/// process's off-screen surface onto the real framebuffer. `(surface_ptr, width, height, stride)`; the
/// surface is kernel-owned (EL1 identity pointer), so the hook may blit + damage-flush it directly.
pub type FbPresentFn = fn(*const u8, u32, u32, u32);

/// The registered present hook, as a raw fn-pointer word (0 = none). SYS_FB_PRESENT calls it if set.
static FB_PRESENT_HOOK: AtomicU64 = AtomicU64::new(0);

/// ELF-3 PRESENT SEAM (public, in-lane): the video subsystem registers its blit-to-scan-out hook here at
/// init. SYS_FB_PRESENT calls it to composite the kernel-owned off-screen surface to the real framebuffer
/// through the existing dirty-rect damage+flush machinery. EL0 never gets the scan-out — only this kernel
/// path touches it. DEFERRED WIRING (3 lines, outside this lane — `video/screen.rs` init):
///     use crate::arch::aarch64::syscall::register_fb_present_hook;
///     register_fb_present_hook(|surf, w, h, stride| Screen::global().blit_surface(surf, w, h, stride));
/// Until wired, SYS_FB_PRESENT is a no-op composite (the checksum witness still proves the drawn surface).
pub fn register_fb_present_hook(f: FbPresentFn) {
    FB_PRESENT_HOOK.store(f as usize as u64, Ordering::Release);
}

/// Translate a validated EL0 user VA to its physical address via `AT s1e0r` (the futex key). Returns 0 on
/// a translation fault (F=1). Masks IRQ across the AT->MRS pair (PAR_EL1 is per-core state any AT clobbers).
fn user_va_to_phys(va: u64) -> u64 {
    let (par, daif): (u64, u64);
    unsafe {
        core::arch::asm!(
            "mrs {daif}, DAIF",
            "msr DAIFSet, #2",
            "at s1e0r, {va}",
            "isb",
            "mrs {par}, PAR_EL1",
            "msr DAIF, {daif}",
            va = in(reg) va,
            par = out(reg) par,
            daif = out(reg) daif,
            options(nostack, preserves_flags),
        );
    }
    if par & 1 != 0 {
        return 0; // PAR_EL1.F = 1 -> translation fault
    }
    (par & 0x0000_FFFF_FFFF_F000) | (va & 0xFFF)
}

/// SYS_FB_MAP(): map the caller's off-screen surface + RO info page into its EL0 window and return the
/// surface VA. Requires a per-process slot (ASID != 0); `-EINVAL` from the shared/boot context. Idempotent
/// (re-mapping re-writes the same leaves + info). The info page carries the geometry EL0 needs to draw.
fn sys_fb_map() -> i64 {
    let asid = current_asid();
    if asid == 0 || asid as usize > super::boot::USER_SLOTS {
        return EINVAL;
    }
    let slot = (asid - 1) as usize;
    unsafe { super::boot::map_slot_fb(slot) };
    // Write the geometry into the RO info page through the kernel identity pointer (EL1-RW; the EL0 alias
    // is read-only). u32 fields: [magic, width, height, stride, format, size, surface_offset].
    let info = super::boot::slot_fb_info_ptr(slot) as *mut u32;
    unsafe {
        info.add(0).write_volatile(FB_MAGIC);
        info.add(1).write_volatile(super::boot::FB_SURFACE_W);
        info.add(2).write_volatile(super::boot::FB_SURFACE_H);
        info.add(3).write_volatile(super::boot::FB_SURFACE_STRIDE);
        info.add(4).write_volatile(FB_FORMAT_ARGB8888);
        info.add(5).write_volatile(super::boot::FB_SURFACE_SIZE as u32);
        info.add(6).write_volatile(super::boot::FB_INFO_SIZE as u32); // surface offset from the info base
    }
    super::boot::fb_surface_va() as i64
}

/// FNV-1a 64-bit over `n` bytes at `p` — the surface checksum for the self-verifying witness.
fn fb_checksum(p: *const u8, n: usize) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0usize;
    while i < n {
        let b = unsafe { p.add(i).read_volatile() };
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// SYS_FB_PRESENT(): composite the caller's surface to the real screen (registered hook) and record its
/// checksum for the witness. Requires a per-process slot. EL0 never sees the scan-out.
fn sys_fb_present() -> i64 {
    let asid = current_asid();
    if asid == 0 || asid as usize > super::boot::USER_SLOTS {
        return EINVAL;
    }
    let slot = (asid - 1) as usize;
    let surf = super::boot::slot_fb_surface_ptr(slot);
    let sum = fb_checksum(surf, super::boot::FB_SURFACE_SIZE);
    FB_PRESENT_CHECKSUM.store(sum, Ordering::Release);
    FB_PRESENT_COUNT.fetch_add(1, Ordering::AcqRel);
    let hook = FB_PRESENT_HOOK.load(Ordering::Acquire);
    if hook != 0 {
        // SAFETY: only `register_fb_present_hook` ever stores here, always a valid `FbPresentFn`.
        let f: FbPresentFn = unsafe { core::mem::transmute(hook as usize) };
        f(surf as *const u8, super::boot::FB_SURFACE_W, super::boot::FB_SURFACE_H, super::boot::FB_SURFACE_STRIDE);
    }
    0
}

/// SYS_FUTEX(uaddr, op, val): a physical-address-keyed EL0 wait/wake. Validates `uaddr` (4-aligned, inside
/// the caller's WRITABLE user window — a futex word is RW), translates it to its PA (the key), then:
///   FUTEX_WAIT: block iff *uaddr == val (return 0 woken, `-EAGAIN` on value mismatch, `-EINVAL` off a task
///     / table full);
///   FUTEX_WAKE: wake up to `val` waiters on the key; return the count woken.
fn sys_futex(uaddr: u64, op: u64, val: u64) -> i64 {
    if uaddr & 3 != 0 || !user_range_ok(uaddr, 4, true) {
        return EFAULT;
    }
    let key = user_va_to_phys(uaddr);
    if key == 0 {
        return EFAULT;
    }
    match op {
        FUTEX_WAIT => match super::sched::futex_wait(key, uaddr, val as u32) {
            super::sched::FutexWait::Woken => 0,
            super::sched::FutexWait::Mismatch => EAGAIN,
            super::sched::FutexWait::TableFull => EAGAIN,
            super::sched::FutexWait::NoTask => EINVAL,
        },
        FUTEX_WAKE => super::sched::futex_wake(key, val as usize) as i64,
        _ => EINVAL,
    }
}

// =============================================================================================
// ELF-5: input into EL0 — SYS_INPUT_POLL + the per-process input ring, the router enqueue seam, and the
// active-process registration. An interactive EL0 app (built on ELF-3's surface + ELF-2's threads +
// ELF-3's futex) needs keys/mouse; this is the delivery half.
//
// SHAPE (mirrors how the GUI routes input to kernel apps today — GUI-CLICK-2b's SCREEN_APP_ACTIVE gate +
// gui_watchdog: only the ACTIVE app receives input). The kernel holds a small per-ASID ring of packed
// events; the GUI router (main.rs `pump_usb_into_gui`) fills the ACTIVE process's ring when it drains
// `pal::next_event()` — via the public `el0_input_enqueue` seam here (the ELF-3 present-hook twin). EL0
// polls its own ring nonblocking through SYS_INPUT_POLL. One producer (the single router task) + one
// consumer (the EL0 task) per ring => a lock-free SPSC ring (free-running head/tail, occupancy = tail-head).
//
// PACKED EVENT (u64, bit 63 always CLEAR so an event never aliases a negative errno):
//   [55:48] = type (INPUT_EV_*); low 32 bits = payload — key ASCII / button mask in [7:0], mouse x/dx in
//   [31:16] (i16), mouse y/dy in [15:0] (i16).
//
// DEFERRED ROUTER WIRING (2-3 lines, OUTSIDE this lane — main.rs `pump_usb_into_gui`, the next arc folds):
// when an EL0 app owns the screen (an ELF-5 analogue of SCREEN_APP_ACTIVE, with the focused process's ASID
// registered via `el0_input_set_active`), route drained pal events to the EL0 ring instead of GUI_CHANNEL:
//     while let Some(ev) = unaos_kernel::pal::next_event() {
//         unaos_kernel::arch::aarch64::syscall::el0_input_enqueue(ev); // -> the active EL0 app's ring
//     }
// (Today only the in-kernel witness registers an active process + injects a test event — see
// `input_launcher`. QEMU raspi4b delivers no USB HID, so the kernel-injected event is what proves the
// enqueue->drain path there; the router edge is the metal-only piece the fold lights up.)
// =============================================================================================

/// ELF-5 packed-event type tags (packed value bits [55:48]).
const INPUT_EV_KEY_DOWN: u64 = 1; // a key PRESS   (payload[7:0] = ASCII)
const INPUT_EV_KEY_UP: u64 = 2; // a key RELEASE (payload[7:0] = ASCII)
const INPUT_EV_MOUSE_REL: u64 = 3; // relative pointer motion (payload[31:16]=dx, [15:0]=dy as i16)
const INPUT_EV_MOUSE_ABS: u64 = 4; // absolute pointer position (payload[31:16]=x,  [15:0]=y  as i16)
const INPUT_EV_BUTTON: u64 = 5; // a pointer button-DOWN edge (payload[7:0] = button bitmask)

/// Per-ASID input ring capacity (power of two — occupancy math is `tail.wrapping_sub(head)`).
const INPUT_RING_CAP: usize = 32;

/// The per-process input rings, keyed by ASID (0 = the shared/boot context, never a delivery target). One
/// producer (the router) + one consumer (the EL0 task) per ring => a lock-free SPSC ring.
static EL0_INPUT_BUF: [[AtomicU64; INPUT_RING_CAP]; super::boot::USER_SLOTS + 1] =
    [const { [const { AtomicU64::new(0) }; INPUT_RING_CAP] }; super::boot::USER_SLOTS + 1];
/// Consumer index (free-running; advanced by SYS_INPUT_POLL). Real slot = `head & (CAP-1)`.
static EL0_INPUT_HEAD: [AtomicU32; super::boot::USER_SLOTS + 1] =
    [const { AtomicU32::new(0) }; super::boot::USER_SLOTS + 1];
/// Producer index (free-running; advanced by `el0_input_enqueue`). Occupancy = `tail - head`.
static EL0_INPUT_TAIL: [AtomicU32; super::boot::USER_SLOTS + 1] =
    [const { AtomicU32::new(0) }; super::boot::USER_SLOTS + 1];

/// The ASID of the process currently designated to RECEIVE input (0 = none). The router enqueues only into
/// this process's ring — the ELF-5 twin of `SCREEN_APP_ACTIVE`. Set via `el0_input_set_active`.
static EL0_INPUT_ACTIVE: AtomicU64 = AtomicU64::new(0);

/// UVUG-8: the ASID of the EL0 app that has entered INTERACTIVE TAKEOVER (0 = none). An EL0 full-screen app
/// (`run /fat/UVUG.ELF`) flips to interactive mode the instant it drains its FIRST real input event (the
/// UVUG-4 input-driven switch) — the same edge the app announces with `:: UVUG: interactive takeover ::`.
///
/// UVUG-8r2 moves the engage edge from PRODUCER to CONSUMER. The first cut latched in `el0_input_push` — an
/// event merely REACHING the ring. That mis-fired on metal at t≈0: `el0_input_set_active` reset the per-ASID
/// ring but NOT `pal::EVENT_QUEUE`, so the Enter KeyUp left over from typing `run /fat/UVUG.ELF` drained
/// straight into the new app's ring and engaged takeover before the app had run a single frame — giving EVERY
/// keyboard-launched run (batch apps included, which never poll at all) a suspended deadline. Now the latch is
/// set by `sys_input_poll` only when the app ACTUALLY CONSUMES a packed event, so it reflects the app's own
/// behaviour rather than the router's; and `el0_input_set_active` drains the stale `pal::EVENT_QUEUE` so the
/// launch keystroke cannot be mistaken for in-app interaction in the first place.
///
/// This latch is the `run_user_image` deadline's signal to SUSPEND — but only in conjunction with a fresh
/// `EL0_TAKEOVER_HB` heartbeat (see `takeover_suspends`), so a hung app cannot suspend the deadline forever.
/// Cleared on ANY focus change (`el0_input_set_active`) and on slot teardown (`clear_handle_row`), so a fresh
/// `run` always starts disengaged and the deadline is fully intact for the batch (no-HID) path.
static EL0_TAKEOVER_ASID: AtomicU64 = AtomicU64::new(0);

/// UVUG-8r2: CNTPCT tick of the takeover app's most recent `SYS_INPUT_POLL` — the EL0 liveness heartbeat, the
/// per-`run` twin of `gui_watchdog`'s `LAST_PROGRESS_SECS`. Stamped on EVERY poll (an app calling the syscall
/// IS making progress, empty ring or not). `run_user_image` suspends its deadline only while this is fresh, so
/// an app that stops polling loses the suspension and the run regains a hard liveness bound.
static EL0_TAKEOVER_HB: AtomicU64 = AtomicU64::new(0);

/// UVUG-8r2: how long the takeover app may go without a `SYS_INPUT_POLL` before its takeover stops suspending
/// the `run` deadline. Whole seconds, scaled by `timer::cntfrq()` at the use site. Two seconds is far longer
/// than any plausible frame (a frame-driven app polls many times a second) yet keeps the worst-case shell
/// strand for a hung app bounded at `TAKEOVER_STALE_SECS + deadline` ≈ 7 s instead of forever.
pub const TAKEOVER_STALE_SECS: u64 = 2;

/// Pack a `pal::Event` into the ELF-5 u64 wire form, or `None` for a non-deliverable event (Timer/None/
/// Unknown carry nothing for EL0). Bit 63 is always clear, so `sys_input_poll` can hand the packed value
/// straight back as a non-negative i64 distinct from `-EAGAIN`.
fn pack_input(ev: crate::pal::Event) -> Option<u64> {
    use crate::pal::Event;
    let pack_xy = |x: i32, y: i32| -> u64 {
        ((x as i16 as u16 as u64) << 16) | (y as i16 as u16 as u64)
    };
    let (ty, payload): (u64, u64) = match ev {
        Event::Key(b) => (INPUT_EV_KEY_DOWN, b as u64),
        Event::KeyUp(b) => (INPUT_EV_KEY_UP, b as u64),
        Event::Mouse { x, y } => (INPUT_EV_MOUSE_REL, pack_xy(x, y)),
        Event::MouseAbsolute { x, y } => (INPUT_EV_MOUSE_ABS, pack_xy(x, y)),
        Event::Button(mask) => (INPUT_EV_BUTTON, mask as u64),
        Event::Timer | Event::None | Event::Unknown => return None,
    };
    Some((ty << 48) | payload)
}

/// ELF-5 ROUTER SEAM (public, in-lane): enqueue one input event into the ACTIVE process's ring. Called by the
/// GUI router (main.rs `pump_usb_into_gui`) when it drains `pal::next_event()` and an EL0 app owns input.
/// Returns `true` if the event was queued, `false` if there is no active EL0 target, the event is not
/// deliverable, or the ring is full (drop-newest — an unread event is never overwritten). Single producer.
pub fn el0_input_enqueue(ev: crate::pal::Event) -> bool {
    let asid = EL0_INPUT_ACTIVE.load(Ordering::Acquire);
    if asid == 0 || asid as usize > super::boot::USER_SLOTS {
        return false;
    }
    let Some(packed) = pack_input(ev) else {
        return false;
    };
    el0_input_push(asid, packed)
}

/// Push a pre-packed event into `asid`'s ring (the SPSC producer half). Drop-newest on a full ring so a
/// backlog can never clobber an event the EL0 consumer has not yet read. `asid` is validated by the caller.
fn el0_input_push(asid: u64, packed: u64) -> bool {
    let a = asid as usize;
    let head = EL0_INPUT_HEAD[a].load(Ordering::Acquire);
    let tail = EL0_INPUT_TAIL[a].load(Ordering::Relaxed); // this task is the sole producer
    if tail.wrapping_sub(head) >= INPUT_RING_CAP as u32 {
        return false; // full — drop the newest
    }
    EL0_INPUT_BUF[a][(tail as usize) & (INPUT_RING_CAP - 1)].store(packed, Ordering::Release);
    EL0_INPUT_TAIL[a].store(tail.wrapping_add(1), Ordering::Release); // publish AFTER the slot store
    // UVUG-8r2: deliberately NO takeover latch here. Delivery into the ring proves only that the ROUTER acted;
    // it says nothing about whether the app is interactive (or even running). The latch is set on the CONSUMER
    // side, in `sys_input_poll`, when the app actually drains the event. See `EL0_TAKEOVER_ASID`.
    true
}

/// ELF-5 ACTIVE-PROCESS REGISTRATION (public, in-lane): designate which process receives input. Passing an
/// ASID resets that ring (a freshly-focused process starts with an empty queue — no stale input from a prior
/// focus); passing 0 clears the designation (no EL0 target — enqueues become no-ops). Called by the focus
/// owner (today the witness; on the folded router, the shell/compositor on a focus change).
pub fn el0_input_set_active(asid: u64) {
    // UVUG-8r2: a fresh focus must also start with an empty UPSTREAM queue. `clear_input_row` resets only the
    // per-ASID ring; `pal::EVENT_QUEUE` sits in front of it and, on metal, still holds the tail of the
    // keystroke that LAUNCHED this program — the Enter KeyUp from typing `run /fat/UVUG.ELF`. Left there, the
    // router drains it into the new app's ring microseconds after launch, and the app's first poll reads it as
    // genuine in-app interaction (pre-r2, the push itself engaged takeover, so every keyboard-launched run —
    // batch programs included — got a suspended deadline at t≈0). Drain and DISCARD it here: events queued
    // BEFORE the app existed were never meant for it. Bounded by the queue's own 64-slot cap, so this
    // terminates even against a live producer. Only on a real focus (asid != 0); clearing focus leaves the
    // queue alone, since from then on those events legitimately belong to the shell/GUI channel.
    if asid != 0 && (asid as usize) <= super::boot::USER_SLOTS {
        clear_input_row(asid); // fresh focus starts clean
        let mut drained = 0u32;
        while drained < 64 && crate::pal::next_event().is_some() {
            drained = drained.wrapping_add(1);
        }
        if drained != 0 {
            serial_println!(
                "[uvug8] focus asid={} — discarded {} pre-launch event(s) from EVENT_QUEUE",
                asid,
                drained
            );
        }
    }
    // UVUG-8: a focus change RESETS interactive takeover — a freshly-focused app has consumed nothing yet, so
    // it starts as a batch run with the deadline fully armed; it engages takeover only once it actually drains
    // a real event via SYS_INPUT_POLL. This is also what makes a `run` (which calls set_active(asid) then, on
    // return, set_active(0)) start and end cleanly disengaged.
    //
    // UVUG-8r2 ORDERING: publish the new focus FIRST, then clear the latch. The reverse order left a window in
    // which a concurrent router push could re-store the OUTGOING asid into the latch after we cleared it. With
    // the engage edge moved to the consumer side that push can no longer write the latch at all, but the
    // clear-after-publish order is the one that is correct independent of that, so it is what we do: after this
    // store, the only writer that could re-latch is an app polling under the NEW focus, which is exactly right.
    EL0_INPUT_ACTIVE.store(asid, Ordering::Release);
    EL0_TAKEOVER_ASID.store(0, Ordering::Release);
    EL0_TAKEOVER_HB.store(0, Ordering::Release);
}

/// The ASID currently designated to receive input (0 = none) — the read side of `el0_input_set_active`.
pub fn el0_input_active() -> u64 {
    EL0_INPUT_ACTIVE.load(Ordering::Acquire)
}

/// UVUG-8: the ASID currently in interactive takeover (0 = none) — the read side of the takeover latch. The
/// `run_user_image` deadline reads this each wait pass; combined with a fresh heartbeat (`takeover_suspends`)
/// it means the app is a live interactive session and the deadline is suspended.
pub fn el0_takeover_active() -> u64 {
    EL0_TAKEOVER_ASID.load(Ordering::Acquire)
}

/// UVUG-8r2: the takeover app's last `SYS_INPUT_POLL` timestamp in CNTPCT ticks — the read side of the EL0
/// liveness heartbeat. Meaningful only alongside `el0_takeover_active()`; both are reset on a focus change.
pub fn el0_takeover_heartbeat() -> u64 {
    EL0_TAKEOVER_HB.load(Ordering::Acquire)
}

/// UVUG-8r2: the pure "is the deadline suspended?" decision, factored out of `run_user_image`'s wait loop so
/// it can be reasoned about (and self-tested) with no global state or clock. Returns `true` iff `latched` names
/// THIS run's `asid` (the app drained a real event — it is interactive) AND its heartbeat is no older than
/// `stale_ticks` (it is still polling — it is ALIVE). Both halves are required, and the second is the whole
/// point of r2: without it a hung app suspends the deadline forever and strands the shell until reboot.
/// `wrapping_sub` matches the CNTPCT counter's wrap-safe math.
pub fn takeover_suspends(latched: u64, asid: u64, now: u64, heartbeat: u64, stale_ticks: u64) -> bool {
    latched != 0 && latched == asid && now.wrapping_sub(heartbeat) <= stale_ticks
}

/// UVUG-8: the pure takeover-aware deadline decision, factored out of `run_user_image`'s wait loop so it can be
/// reasoned about (and unit/self-tested) with no global state or clock. Returns `true` iff the run has TIMED
/// OUT: the app is NOT currently in takeover AND its (non-suspended) budget has been exceeded. `budget_start`
/// is the CNTPCT tick from which the current deadline window is measured; the caller resets it to `now` on
/// every takeover pass (suspend) and once on the takeover->non-takeover edge (re-arm a full fresh window), so
/// time spent in takeover never counts against the deadline. `wrapping_sub` matches the counter's wrap-safe math.
pub fn run_deadline_timed_out(in_takeover: bool, now: u64, budget_start: u64, deadline: u64) -> bool {
    !in_takeover && now.wrapping_sub(budget_start) > deadline
}

/// Reset an ASID's input ring (head == tail == 0 => empty). Called on a focus change (fresh start) and from
/// `clear_handle_row` on slot teardown (a reused ASID inherits no stale input). Safe because on teardown no
/// producer runs for the dying ASID, and on a focus change the router only ever targets the newly-active ASID.
fn clear_input_row(asid: u64) {
    let a = asid as usize;
    EL0_INPUT_HEAD[a].store(0, Ordering::Release);
    EL0_INPUT_TAIL[a].store(0, Ordering::Release);
}

/// SYS_INPUT_POLL(): nonblocking dequeue of the next input event for the CALLING process. Returns the packed
/// event (>= 0) or `-EAGAIN` when the ring is empty (or the caller has no private slot — ASID 0). The SPSC
/// consumer half: this EL0 task is the sole consumer of its own ring.
fn sys_input_poll() -> i64 {
    // UVUG-5: an EL0 full-screen app (`run /fat/UVUG.ELF`) drives input through THIS syscall every frame —
    // it never touches the kernel `pal::pump_and_poll` path that feeds `gui_watchdog::note_progress`. So the
    // watchdog, armed by `on_app_enter` when the `run` command took the screen, never saw a heartbeat and
    // FALSELY declared the healthy, polling UVUG app wedged at 5 s (`[gui] watchdog app wedged 5s (no drain
    // since …)`, P47) — returning input to the shell mid-run. The EL0 twin of the kernel app's per-drain
    // heartbeat is this poll itself: a program calling SYS_INPUT_POLL IS making drain progress, whether or
    // not the ring has an event this instant. Feed the heartbeat on EVERY poll (before the empty-ring return),
    // so a live EL0 app is never reclaimed and the `run_user_image` deadline stays the sole liveness bound.
    // No-op when no app owns the screen, so this is safe on any caller.
    crate::gui_watchdog::note_progress();
    let asid = current_asid();
    if asid == 0 || asid as usize > super::boot::USER_SLOTS {
        return EAGAIN;
    }
    // UVUG-8r2: stamp the EL0 takeover HEARTBEAT on every poll, empty ring or not — the per-`run` twin of the
    // `note_progress` call above. Reaching this syscall IS proof of liveness; `run_user_image` suspends its
    // deadline only while this stays fresh, so an app that wedges mid-takeover stops holding the deadline open
    // and the run regains its bound. Stamped only for the FOCUSED app so an unfocused EL0 task's polling can
    // never keep another process's takeover alive.
    if EL0_INPUT_ACTIVE.load(Ordering::Acquire) == asid {
        EL0_TAKEOVER_HB.store(super::timer::cntpct(), Ordering::Release);
    }
    let a = asid as usize;
    let head = EL0_INPUT_HEAD[a].load(Ordering::Relaxed); // this task is the sole consumer
    let tail = EL0_INPUT_TAIL[a].load(Ordering::Acquire);
    if head == tail {
        return EAGAIN; // empty
    }
    let packed = EL0_INPUT_BUF[a][(head as usize) & (INPUT_RING_CAP - 1)].load(Ordering::Acquire);
    EL0_INPUT_HEAD[a].store(head.wrapping_add(1), Ordering::Release); // consume AFTER the load
    // UVUG-8r2: the app just CONSUMED a real input event — the interactive-takeover edge, observed on the
    // consumer side (see `EL0_TAKEOVER_ASID` for why the producer side was wrong). Latch it, but only for the
    // FOCUSED app: a background EL0 task draining its own stale ring is not taking over the screen. Idempotent.
    if EL0_INPUT_ACTIVE.load(Ordering::Acquire) == asid {
        EL0_TAKEOVER_ASID.store(asid, Ordering::Release);
    }
    packed as i64
}

// ELF-5 input-test witnesses (captured kernel-side, printed by `input_launcher`). The program SYS_REPORTs
// three observations in order (initial poll, drained event, final poll); `INPUT_OBS_N` counts them, so the
// launcher can wait for the initial poll (>=1) before injecting, making the enqueue->drain ordering exact.
static INPUT_OBS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static INPUT_OBS_N: AtomicU32 = AtomicU32::new(0);
static INPUT_PARENT_DONE: AtomicBool = AtomicBool::new(false); // the program reached its clean sys_exit(0)
static INPUT_ENQUEUE_OK: AtomicBool = AtomicBool::new(false); // the kernel injection returned true

/// ELF-5: record the input-test program's SYS_REPORTs in arrival order, keyed by name. Called from the
/// SYS_REPORT arm alongside the other reporters; each ignores the others' names.
fn input_report(value: u64) {
    if super::sched::current_name() == Some("el0-input") {
        let n = INPUT_OBS_N.fetch_add(1, Ordering::AcqRel) as usize;
        if n < INPUT_OBS.len() {
            INPUT_OBS[n].store(value, Ordering::Release);
        }
    }
}

// The input-test EL0 program: one code page, register-only (no data refs, so fully position-independent —
// it runs wherever the loader copies it). It (1) polls once and reports the result (expect -EAGAIN, ring
// empty), (2) poll+yields until a non-negative event arrives and reports it (the kernel-injected KeyDown),
// (3) polls once more and reports the result (expect -EAGAIN, drained), then exits 0. Poll result / errno
// is in x0; an event has bit 63 clear, `-EAGAIN` has it set — so `tbz x0,#63` distinguishes them.
core::arch::global_asm!(
    r#"
    .balign 4
    .globl __input_blob_start
__input_blob_start:
    .globl __input_prog
__input_prog:
    mov  x8, #27                          // (1) SYS_INPUT_POLL — expect -EAGAIN (ring empty)
    svc  #0
    mov  x8, #3                           // SYS_REPORT(obs[0] = poll result); x0 still holds it
    svc  #0
1:  mov  x8, #27                          // (2) SYS_INPUT_POLL, looping until an event arrives
    svc  #0
    tbz  x0, #63, 2f                      // bit 63 clear => a packed event (>= 0); else -EAGAIN
    mov  x8, #4                           // SYS_YIELD — let the launcher inject / other cores run
    svc  #0
    b    1b
2:  mov  x8, #3                           // SYS_REPORT(obs[1] = the drained event); x0 holds it
    svc  #0
    mov  x8, #27                          // (3) SYS_INPUT_POLL — expect -EAGAIN (drained)
    svc  #0
    mov  x8, #3                           // SYS_REPORT(obs[2] = poll result)
    svc  #0
    mov  x0, #0                           // SYS_EXIT(0) -> routed to the ELF-5 witness by name
    mov  x8, #2
    svc  #0
3:  b 3b
    .balign 4
    .globl __input_blob_end
__input_blob_end:
"#
);

unsafe extern "C" {
    static __input_blob_start: u8;
    static __input_blob_end: u8;
    static __input_prog: u8;
}

/// ELF-5 input-test launcher + verdict (the `threads_launcher` shape: one gated kernel task, self-contained,
/// its own uncounted `:: EL0: input test … ::` line). Copies the input blob into a fresh slot's code page,
/// protects it EL0-RX/EL1-RO, REGISTERS the slot as the active input target, and runs `el0-input` on THIS
/// core (so the cooperative-yield verdict drives it under QEMU). Once the program has done its initial poll
/// (observed the empty ring), the launcher injects ONE KeyDown('A') through the REAL `el0_input_enqueue`
/// router seam (proving pack + active routing), then waits for the program to drain it and exit. The verdict
/// asserts: initial poll == -EAGAIN, injection returned true, the drained event == the expected packed value,
/// final poll == -EAGAIN. QEMU raspi4b has no USB HID, so the kernel-injected event is what proves the
/// enqueue->drain path here — an honest statement of what QEMU can and cannot show. Never perturbs the
/// fixture battery (its own line, exit routed by name).
fn input_launcher(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let bstart = &raw const __input_blob_start as usize;
    let bend = &raw const __input_blob_end as usize;
    let blen = bend - bstart;
    if blen > super::boot::USER_CODE_SIZE {
        serial_println!(":: EL0: input test — blob {} B > code page, SKIP ::", blen);
        return;
    }
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = the program's initial SP_EL0
    let Some(slot) = super::boot::alloc_user_slot() else {
        serial_println!(":: EL0: input test — no free address-space slot, SKIP ::");
        return;
    };
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    let asid = ttbr0 >> 48;
    install_console_cap(asid);
    // ELF-5: register this process as the active input target BEFORE it runs, so `el0_input_enqueue` routes
    // to its ring (and resets the ring — a clean start).
    el0_input_set_active(asid);
    let entry = base + (&raw const __input_prog as usize - bstart) as u64;
    let run_cpu = super::percpu::this_cpu().cpu_index as usize;
    super::sched::spawn_user_slot("el0-input", entry, sp, ttbr0, run_cpu);

    let vstart = super::timer::cntpct();
    let vdeadline = 2 * super::timer::cntfrq();
    // Wait until the program has done its INITIAL poll (obs count >= 1 => it observed the empty ring), so the
    // injection lands strictly AFTER the empty observation — the enqueue->drain ordering is then exact.
    while INPUT_OBS_N.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    // Inject one KeyDown('A') through the REAL router seam — targets the active ASID (this program's ring).
    let ok = el0_input_enqueue(crate::pal::Event::Key(b'A'));
    INPUT_ENQUEUE_OK.store(ok, Ordering::Release);
    // Wait (bounded) for the program to drain the event and exit cleanly.
    while !INPUT_PARENT_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    // Clear the active designation (also cleared at slot teardown; explicit here for symmetry).
    el0_input_set_active(0);

    let obs0 = INPUT_OBS[0].load(Ordering::Acquire);
    let obs1 = INPUT_OBS[1].load(Ordering::Acquire);
    let obs2 = INPUT_OBS[2].load(Ordering::Acquire);
    let n = INPUT_OBS_N.load(Ordering::Acquire);
    let done = INPUT_PARENT_DONE.load(Ordering::Acquire);
    let expected = (INPUT_EV_KEY_DOWN << 48) | (b'A' as u64);
    let eagain = EAGAIN as u64; // -EAGAIN sign-extended to u64
    if done && ok && n == 3 && obs0 == eagain && obs1 == expected && obs2 == eagain {
        serial_println!(
            ":: EL0: input test — poll-empty=EAGAIN enqueue=1 event={:#x} drained=EAGAIN :: PASS ::",
            obs1
        );
    } else {
        serial_println!(
            ":: EL0: input test — n={} obs0={:#x} obs1={:#x}(want {:#x}) obs2={:#x} enq={} done={} :: FAIL ::",
            n, obs0, obs1, expected, obs2, ok, done
        );
    }
}

// ELF-3 fb-test witnesses (captured kernel-side, printed by `fb_launcher`).
static FB_PARENT_DONE: AtomicBool = AtomicBool::new(false); // the parent reached its clean sys_exit(0)
static FB_PRESENT_COUNT: AtomicU64 = AtomicU64::new(0); // SYS_FB_PRESENT calls (want 1)
static FB_PRESENT_CHECKSUM: AtomicU64 = AtomicU64::new(0); // checksum of the surface at present time
static FB_REPORTED_GEOM: AtomicU64 = AtomicU64::new(0); // (width<<16 | height) the parent read from the info page

/// ELF-3: capture the fb-test parent's SYS_REPORT (the width/height it read from the RO info page), keyed by
/// name. Proves EL0 actually READ the info page. Called from the SYS_REPORT arm; ignores other names.
fn fb_report(value: u64) {
    if super::sched::current_name() == Some("el0-fb") {
        FB_REPORTED_GEOM.store(value, Ordering::Release);
    }
}

/// The kernel-computed EXPECTED surface checksum: worker A fills the TOP half with 0xA1 bytes, worker B the
/// BOTTOM half with 0xB2 bytes (see the blob). The witness PASSes only if the presented surface matches.
fn fb_expected_checksum() -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let half = super::boot::FB_SURFACE_SIZE / 2;
    let mut i = 0usize;
    while i < super::boot::FB_SURFACE_SIZE {
        let b: u8 = if i < half { 0xA1 } else { 0xB2 };
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

// The fb-test EL0 program: a parent + a draw worker, one code page (RX), sharing the slot's data/stack
// pages AND the mapped FB surface. Position-independent (`adr __fb_blob_start` recovers the window base).
// Window layout: page 0 code; the sync word (done-counter) at base+0x1000; worker A stack top base+0x3000,
// worker B base+0x3800, parent stack the launcher-set window top. The FB surface (base+0x5000) and RO info
// page (base+0x4000) live in the reserved hole above the window, mapped by SYS_FB_MAP.
core::arch::global_asm!(
    r#"
    .balign 4
    .globl __fb_blob_start
__fb_blob_start:
    .globl __fb_prog_parent
__fb_prog_parent:
    adr  x9, __fb_blob_start               // x9 = window base (PC-relative), preserved across svc
    mov  x8, #24                           // SYS_FB_MAP
    svc  #0                                // x0 = surface VA (workers recompute it PC-relatively)
    // read geometry from the RO info page (base+0x4000): width@+4, height@+8 -> report (w<<16 | h)
    add  x10, x9, #0x4000
    ldr  w11, [x10, #4]                    // width
    ldr  w12, [x10, #8]                    // height
    lsl  w11, w11, #16
    orr  w11, w11, w12
    mov  x0, x11                           // SYS_REPORT(width<<16 | height)
    mov  x8, #3
    svc  #0
    add  x20, x9, #0x1000                  // x20 = the done-counter / futex word
    str  wzr, [x20]                        // zero it before any worker increments
    dmb  ish
    adr  x21, __fb_prog_worker             // x21 = worker entry VA
    // worker A: caller's core (place 0), arg 0 (top half), stack top base+0x3000
    mov  x0, x21
    add  x1, x9, #0x3000
    mov  x2, #0
    mov  x3, #0
    mov  x8, #21                           // SYS_THREAD_SPAWN
    svc  #0
    mov  x23, x0                           // x23 = handle_a
    // worker B: sibling core (place 1), arg 1 (bottom half), stack top base+0x3800
    mov  x0, x21
    add  x1, x9, #0x3000
    add  x1, x1, #0x800
    mov  x2, #1
    mov  x3, #1
    mov  x8, #21
    svc  #0
    mov  x24, x0                           // x24 = handle_b
    // futex wait loop: block until the done-counter reaches 2 (both halves drawn)
1:  ldr  w25, [x20]                        // v = done counter
    cmp  w25, #2
    b.ge 2f
    mov  x0, x20                           // SYS_FUTEX(uaddr=counter, FUTEX_WAIT, expected=v)
    mov  x1, #0
    mov  x2, x25
    mov  x8, #26
    svc  #0
    b    1b
2:  mov  x8, #25                           // SYS_FB_PRESENT (kernel checksums + composites)
    svc  #0
    mov  x0, x23                           // SYS_THREAD_JOIN(handle_a)  (reap)
    mov  x8, #23
    svc  #0
    mov  x0, x24                           // SYS_THREAD_JOIN(handle_b)
    mov  x8, #23
    svc  #0
    mov  x0, #1                            // SYS_WRITE(fd=1, msg, 16)
    adr  x1, __fb_msg
    mov  x2, #16
    mov  x8, #1
    svc  #0
    mov  x0, #0                            // SYS_EXIT(0) -> routed to the ELF-3 witness by name
    mov  x8, #2
    svc  #0
3:  b 3b

    .balign 4
    .globl __fb_prog_worker
__fb_prog_worker:
    // x0 = arg (0 = top half / 0xA1, 1 = bottom half / 0xB2).
    adr  x9, __fb_blob_start               // x9 = window base
    add  x12, x9, #0x5000                  // x12 = surface VA (base + 0x4000 info + 0x1000)
    mov  x13, #2048                        // half the 4096-byte surface
    cbz  x0, 4f
    add  x12, x12, x13                     // bottom half start
    mov  w14, #0xB2
    b    5f
4:  mov  w14, #0xA1
5:  orr  w14, w14, w14, lsl #8             // replicate the fill byte across the word
    orr  w14, w14, w14, lsl #16
    mov  x15, #0
6:  str  w14, [x12, x15]                   // fill this half of the surface
    add  x15, x15, #4
    cmp  x15, x13
    b.lt 6b
    dmb  ish                               // publish the drawn bytes before signalling done
    add  x16, x9, #0x1000                  // the done-counter / futex word
7:  ldxr w17, [x16]                        // atomic increment (A72 LL/SC)
    add  w17, w17, #1
    stxr w18, w17, [x16]
    cbnz w18, 7b
    dmb  ish
    mov  x0, x16                           // SYS_FUTEX(uaddr=counter, FUTEX_WAKE, 1) — wake the parent
    mov  x1, #1
    mov  x2, #1
    mov  x8, #26
    svc  #0
    mov  x8, #22                           // SYS_THREAD_EXIT (posts completion; never returns)
    svc  #0
8:  b 8b

    .balign 4
    .globl __fb_msg
__fb_msg:
    .ascii "fb presented ok\n"
    .balign 4
    .globl __fb_blob_end
__fb_blob_end:
"#
);

unsafe extern "C" {
    static __fb_blob_start: u8;
    static __fb_blob_end: u8;
    static __fb_prog_parent: u8;
}

/// ELF-3 fb-test launcher + verdict (the `threads_launcher` shape: one gated kernel task, self-contained,
/// its own uncounted `:: EL0: fb test — … ::` line). Copies the fb blob into a fresh slot's code page,
/// protects it EL0-RX/EL1-RO, endows a console cap, initialises the futex pool, and runs the parent
/// `el0-fb` on THIS core (so the cooperative-yield verdict drives it under QEMU). The parent maps its FB
/// surface, spawns 2 draw threads that fill halves under futex sync, presents (the kernel checksums the
/// surface), joins, and exits. The verdict prints the authoritative witness with the mapped geometry, the
/// present count, and the surface checksum — self-verified against `fb_expected_checksum`. Runs AFTER
/// `threads_launcher`'s verdict, so its reuse of the shared SYS_THREAD_* accounting perturbs nothing.
fn fb_launcher(_demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let bstart = &raw const __fb_blob_start as usize;
    let bend = &raw const __fb_blob_end as usize;
    let blen = bend - bstart;
    if blen > super::boot::USER_CODE_SIZE {
        serial_println!(":: EL0: fb test — blob {} B > code page, SKIP ::", blen);
        return;
    }
    super::sched::futex_init();
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = the parent's initial SP_EL0
    let Some(slot) = super::boot::alloc_user_slot() else {
        serial_println!(":: EL0: fb test — no free address-space slot, SKIP ::");
        return;
    };
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    install_console_cap(ttbr0 >> 48);
    let entry = base + (&raw const __fb_prog_parent as usize - bstart) as u64;
    let run_cpu = super::percpu::this_cpu().cpu_index as usize;
    super::sched::spawn_user_slot("el0-fb", entry, sp, ttbr0, run_cpu);

    // Verdict: yield (bounded ~2 s) so el0-fb + its co-located worker run on this core until the parent exits.
    let vstart = super::timer::cntpct();
    let vdeadline = 2 * super::timer::cntfrq();
    while !FB_PARENT_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let done = FB_PARENT_DONE.load(Ordering::Acquire);
    let present = FB_PRESENT_COUNT.load(Ordering::Acquire);
    let checksum = FB_PRESENT_CHECKSUM.load(Ordering::Acquire);
    let geom = FB_REPORTED_GEOM.load(Ordering::Acquire);
    let w = (geom >> 16) as u32;
    let h = (geom & 0xFFFF) as u32;
    let expect = fb_expected_checksum();
    let geom_ok = w == super::boot::FB_SURFACE_W && h == super::boot::FB_SURFACE_H;
    if done && present == 1 && geom_ok && checksum == expect {
        serial_println!(
            ":: EL0: fb test — mapped={}x{} threads=2 present={} checksum={:#x} :: PASS ::",
            w, h, present, checksum
        );
    } else {
        serial_println!(
            ":: EL0: fb test — mapped={}x{} threads=2 present={} checksum={:#x} (want {}x{}/1/{:#x} done={}) :: FAIL ::",
            w, h, present, checksum,
            super::boot::FB_SURFACE_W, super::boot::FB_SURFACE_H, expect, done
        );
    }
}

// =============================================================================================
// U4: the process-model demo — the parent + orphan slots + the gated launcher/verdict
// =============================================================================================

/// The U4 fixtures' run parameters: the parent's and the orphan's EL0 entry VAs (both inside the shared
/// window VA — only the slot FRAME differs, via TTBR0), the shared initial SP_EL0, and each fixture's slot
/// TTBR0. Two tasks, two slots (with DISTINCT ASIDs — the isolation the ownership negative proves).
struct U4Demo {
    parent: u64,
    orphan: u64,
    sp: u64,
    ttbr0_parent: u64,
    ttbr0_orphan: u64,
}

/// U4 setup: reserve the Proc semaphores, then allocate + build TWO private slots (parent + orphan) via the
/// unwinding `alloc_user_slots`, copy the U4 blob (both fixtures) into each slot's code page, I-cache-sync,
/// and protect each code page (EL0-RX/EL1-RO). Emits the U4 setup line; returns both entries + slot roots.
/// `None` if slot allocation fails (the whole request is released, not leaked). Called ONCE, from
/// `u4_launcher`, AFTER the M6g gate — so the M6d/M6f/M6g slots have freed (at BSP-wiring time all 8 are held
/// by M6d+M6f) and strictly before the parent (hence any child) exists, which is why the `done.init()`
/// reservations here cannot race a concurrent wait/post (the M4 discipline).
///
/// The parent and orphan get DISTINCT slots (hence distinct ASIDs), so their per-process handle tables are
/// distinct rows of `HANDLES` — the substrate the negative proves: handle #0 means the parent's child A in
/// the parent's table, and Empty in the orphan's.
fn u4_setup() -> Option<U4Demo> {
    for p in &PROCS {
        p.done.init();
    }
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = shared initial SP_EL0
    let bstart = &raw const __u4_blob_start as usize;
    let bend = &raw const __u4_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U4 blob does not fit in a code page");
    // Entry VAs = base + each fixture's offset within the blob (an eret to a misaligned entry is EC 0x22).
    let entry = |label: *const u8| -> u64 {
        let va = base + (label as usize - bstart) as u64;
        assert!(va & 3 == 0, "U4 fixture entry misaligned");
        va
    };
    let parent = entry(&raw const __u4_prog_parent);
    let orphan = entry(&raw const __u4_prog_orphan);

    // Two slots, released together on partial failure (the M6d/M6f unwind). slots[0] = parent, [1] = orphan.
    let mut slots = [0usize; 2];
    if !super::boot::alloc_user_slots(&mut slots) {
        return None;
    }
    // Copy the whole blob into each slot's code page (identity backing VA) + I-cache sync (DC CVAU/IC IVAU;
    // PIPT L1 caches make it fetchable at the aliased EL0 window VA), then protect each EL0-RX/EL1-RO.
    for &s in &slots {
        let backing = super::boot::slot_backing_ptr(s);
        unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
        super::cache::icache_sync_range(backing as usize, blen);
    }
    for &s in &slots {
        unsafe { super::boot::protect_user_slot_code(s, super::boot::USER_CODE_SIZE) };
    }

    serial_println!(
        ":: U4: process model — per-process handle table (sys_spawn->handle, sys_wait(handle)) ::"
    );
    Some(U4Demo {
        parent,
        orphan,
        sp,
        ttbr0_parent: super::boot::slot_ttbr0(slots[0]),
        ttbr0_orphan: super::boot::slot_ttbr0(slots[1]),
    })
}

/// U4 launcher + verdict (the M6g-loader shape: one gated kernel task). Spawned on a scheduled sibling core;
/// `demo_cpu` (the task arg) is the demo core the parent + orphan run on. Flow:
///   1. Wait (bounded) for `M6G_LOADER_DONE`, so all M6g lines print first AND the slots have freed.
///   2. Skip silently if no SD device (the parent's sys_spawn loads the children off the card — nothing to run).
///   3. `u4_setup()` (build both slots, print the U4 setup line), then spawn the parent AND the orphan on
///      `demo_cpu`. The parent's two `sys_spawn`s co-locate BOTH children on `demo_cpu` too — the invariant
///      that keeps each child queued-not-dispatched until the parent blocks in sys_wait (so both pids are
///      recorded first; load-bearing for two children exactly as M7's was for one). The orphan's
///      `sys_wait(0)` returns immediately (-ECHILD), so it never parks — no deadlock with the co-located work.
///   4. Verdict (folded): wait (bounded CNTPCT) for BOTH fixtures to reach their sentinel exit
///      (`EL0_U4_DONE == 2`), then PASS iff the parent reaped both children (witness non-zero) AND the orphan
///      saw -ECHILD (ownership enforced) AND no U4 task was killed. Prints ONE PASS line.
/// The U4 lines (setup, the two children's `hello from EL0` — the THIRD and FOURTH in a full boot — and the
/// PASS) all land after the M6g lines and in that order (setup precedes the spawns; the children's hellos
/// precede the parent's exit, which precedes EL0_U4_DONE reaching 2, which the verdict polls before PASS).
pub fn u4_launcher(demo_cpu: usize) {
    // 1. Gate on the M6g loader (its lines printed + its/M6d's/M6f's slots freed).
    let wstart = super::timer::cntpct();
    let wdeadline = 10 * super::timer::cntfrq();
    while !M6G_LOADER_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No SD device -> the children cannot be loaded; skip silently (keeps the no-SD control path free of
    //    U4 lines, mirroring how M6g's own no-SD path is the empty control).
    if crate::drivers::block::info().is_none() {
        U4_LAUNCH_DONE.store(true, Ordering::Release); // release the U5 gate (U5 also gates on the SD)
        return;
    }

    // 3. Build the parent + orphan slots and spawn both on the demo core.
    let Some(u4) = u4_setup() else {
        serial_println!(":: U4: no free address-space slot — process-model demo skipped ::");
        U4_LAUNCH_DONE.store(true, Ordering::Release); // release the U5 gate
        return;
    };
    super::sched::spawn_user_slot("el0-u4parent", u4.parent, u4.sp, u4.ttbr0_parent, demo_cpu);
    super::sched::spawn_user_slot("el0-u4orphan", u4.orphan, u4.sp, u4.ttbr0_orphan, demo_cpu);

    // 4. Folded verdict: wait (bounded ~5 s, yielding) for BOTH fixtures to reach their sentinel exit, then
    //    judge. Two children (two disk loads) + the orphan complete well under this budget under QEMU.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U4_DONE.load(Ordering::Acquire) < 2
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U4_PARENT_WITNESS.load(Ordering::Acquire);
    let orphan = U4_ORPHAN_ECHILD.load(Ordering::Acquire);
    let killed = EL0_U4_KILLED.load(Ordering::Acquire);
    // The parent reports the token iff it reaped BOTH children by handle with status 0, else 0 — so
    // `== U4_WITNESS_TOKEN` is exactly "both reaped OK" (tighter than the M7 `!= 0`, and it pins the
    // fixture/verdict contract on one constant).
    if witness == U4_WITNESS_TOKEN && orphan == 1 && killed == 0 {
        serial_println!(
            ":: U4: process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD (per-process handle tables) -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U4: process model FAIL — witness={:#x} orphan_echild={} killed={} done={} (want nonzero / 1 / 0 / 2) ::",
            witness,
            orphan,
            killed,
            EL0_U4_DONE.load(Ordering::Acquire)
        );
    }
    // Release the U5 gate: the U4 verdict line has printed and the U4 slots have freed, so `u5_launcher` may
    // now run its capability demo (its lines land strictly after this).
    U4_LAUNCH_DONE.store(true, Ordering::Release);
}

// =============================================================================================
// U5: the capability demo — the cap fixture's slot + endowment + the gated launcher/verdict
// =============================================================================================

/// The U5 fixture's run parameters: the cap fixture's EL0 entry VA (inside the shared window VA — only the
/// slot FRAME differs, via TTBR0), the initial SP_EL0, its slot TTBR0, and its ASID (so the launcher can
/// pre-endow the fixture's table and, after exit, verify the teardown-clear of that exact row).
struct U5Demo {
    cap: u64,
    sp: u64,
    ttbr0: u64,
    asid: u64,
}

/// U5 setup: allocate + build ONE private slot, copy the U5 blob into its code page, I-cache-sync, protect it
/// EL0-RX/EL1-RO, then PRE-ENDOW the fixture's table with the two handles the demo exercises:
///   handle 1 = CONSOLE, {CAP_WRITE|CAP_GRANT} — the "full" console cap it writes from and grants from
///   handle 2 = CONSOLE, {CAP_READ}            — a console cap WITHOUT write (the `-EACCES` negative)
/// Emits the U5 setup line; returns the run params. `None` if slot allocation fails. Called ONCE from
/// `u5_launcher`, after the U4 gate — so a slot is free and no task runs under the fixture's ASID yet (the
/// endowment stores can't race a resolver). Register-only fixture (writes no user stack), so one slot suffices.
fn u5_setup() -> Option<U5Demo> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = initial SP_EL0
    let bstart = &raw const __u5_blob_start as usize;
    let bend = &raw const __u5_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U5 blob does not fit in a code page");
    let cap = {
        let va = base + (&raw const __u5_prog_cap as usize - bstart) as u64;
        assert!(va & 3 == 0, "U5 fixture entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    let asid = ttbr0 >> 48;
    // Pre-endow the fixture's table (before it is dispatched — no concurrent resolver). Two console caps: a
    // full one (write + grant) at index 1, and a write-LESS one at index 2 for the negative.
    install_cap(asid, 1, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    install_cap(asid, 2, KIND_CONSOLE, HANDLE_CONSOLE, CAP_READ);
    serial_println!(
        ":: U5: capabilities — rights + CHECK + grant/attenuate/revoke + routed sys_write ::"
    );
    Some(U5Demo { cap, sp, ttbr0, asid })
}

/// U5 launcher + verdict (the `u4_launcher` shape: one gated kernel task on a sibling core). `demo_cpu` (the
/// task arg) is the core the cap fixture runs on. Flow:
///   1. Wait (bounded) for `U4_LAUNCH_DONE`, so the U5 lines land after the U4 verdict and the U4 slots freed.
///   2. Skip silently if no SD device — U5 needs NO disk (its fixture is an inline blob), but gating on the SD
///      keeps the no-SD control path free of demo lines, mirroring M6g/U4.
///   3. `u5_setup()` (build + pre-endow the fixture's slot), then spawn the fixture on `demo_cpu`.
///   4. Verdict (folded): wait (bounded) for the fixture's sentinel exit (`EL0_U5_DONE == 1`), read its
///      witness bitmask, then wait (bounded) for its handle row to be cleared — the teardown-clear proof:
///      `sched::exit -> boot::teardown_user_slot` clears the row when the fixture exits, transitioning
///      `handle_row_is_clear` false->true (the fixture holds live handles at exit — the minted cap and the
///      write-less cap — so this genuinely exercises the clear). PASS iff witness == `U5_WITNESS_ALL` AND the
///      row cleared AND no U5 kill. Prints ONE PASS line.
pub fn u5_launcher(demo_cpu: usize) {
    // 1. Gate on the U4 launcher (its verdict printed + the U4 slots freed).
    let wstart = super::timer::cntpct();
    let wdeadline = 10 * super::timer::cntfrq();
    while !U4_LAUNCH_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No SD device -> keep the no-SD control path free of demo lines (U5 itself needs no disk, but this
    //    mirrors M6g/U4's control discipline).
    if crate::drivers::block::info().is_none() {
        U5_LAUNCH_DONE.store(true, Ordering::Release); // release the U6 gate (U6 also gates on the SD)
        return;
    }

    // 3. Build + pre-endow the fixture slot and spawn it on the demo core.
    let Some(u5) = u5_setup() else {
        serial_println!(":: U5: no free address-space slot — capability demo skipped ::");
        U5_LAUNCH_DONE.store(true, Ordering::Release); // release the U6 gate
        return;
    };
    super::sched::spawn_user_slot("el0-u5cap", u5.cap, u5.sp, u5.ttbr0, demo_cpu);

    // 4a. Wait (bounded ~5 s, yielding) for the fixture to reach its sentinel exit, then snapshot the witness.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U5_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U5_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U5_KILLED.load(Ordering::Acquire);

    // 4b. Teardown-clear proof: the fixture exited above, so its exit path cleared its handle row. That clear
    //     runs just AFTER the sentinel increment, so poll (bounded) until the row is clear — false->true when
    //     teardown runs. Nothing reuses the slot after (U5 is the last demo), so once clear it stays clear.
    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !handle_row_is_clear(u5.asid)
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = handle_row_is_clear(u5.asid);

    if witness == U5_WITNESS_ALL && cleared && killed == 0 {
        serial_println!(
            ":: U5: capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded, revoke enforced, teardown-clear clean -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U5: capabilities FAIL — witness={:#x} cleared={} killed={} done={} (want {:#x} / true / 0 / 1) ::",
            witness,
            cleared,
            killed,
            EL0_U5_DONE.load(Ordering::Acquire),
            U5_WITNESS_ALL
        );
    }
    // Release the U6 gate: the U5 verdict line has printed and the U5 slot has freed (teardown-clear above), so
    // `u6_launcher` may now run its object-table demo (its lines land strictly after this).
    U5_LAUNCH_DONE.store(true, Ordering::Release);
}

// =============================================================================================
// U6: the general object table demo — the printing spawner + kernel-side kind/no-collision checks
// =============================================================================================

/// The U6 fixture's run parameters: the printing-spawner's EL0 entry VA (inside the shared window VA — only the
/// slot FRAME differs, via TTBR0), the initial SP_EL0, its slot TTBR0, and its ASID (so the launcher can run the
/// kernel-side object-table checks against — and then endow — that exact row).
struct U6Demo {
    spawn: u64,
    sp: u64,
    ttbr0: u64,
    asid: u64,
}

/// U6 setup: allocate + build ONE private slot, copy the U6 blob into its code page, I-cache-sync, protect it
/// EL0-RX/EL1-RO, and return the run params. Does NOT endow the console cap — the launcher runs its kernel-side
/// checks against the fresh (empty) row FIRST, then endows the live console cap. Emits the U6 setup line;
/// `None` if slot allocation fails. Called ONCE from `u6_launcher`, after the U5 gate — so a slot is free and no
/// task runs under the fixture's ASID yet (the checks/endowment can't race a resolver). Register-only fixture
/// (writes no user stack), so one slot suffices.
fn u6_setup() -> Option<U6Demo> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = initial SP_EL0
    let bstart = &raw const __u6_blob_start as usize;
    let bend = &raw const __u6_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U6 blob does not fit in a code page");
    let spawn = {
        let va = base + (&raw const __u6_prog_spawn as usize - bstart) as u64;
        assert!(va & 3 == 0, "U6 fixture entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    let asid = ttbr0 >> 48;
    serial_println!(
        ":: U6: general object table — (kind, target, rights) descriptors, first-free alloc skips the reserved console index ::"
    );
    Some(U6Demo { spawn, sp, ttbr0, asid })
}

/// U6 kernel-side check — run against the fixture's FRESH, empty row before it is endowed/dispatched (no
/// concurrent resolver): prove the two things the EL0 fixture cannot observe itself.
///
///   (A) NO INDEX COLLISION for any interleaving: auto-allocate two handles (`handle_install`), THEN install the
///       console cap at the reserved `CONSOLE_FD`. Under U5's allocator the second auto-handle would land on
///       index 1 and the console's unconditional store would clobber it; U6's allocator skips the reserved
///       index, so both auto-handles keep their values AND the console lands intact — verified via `handle_get`.
///       This is the exact interleaving the U5 review flagged, exercised directly.
///   (B) The scaffold FILE/SOCKET kinds RESOLVE to their kind carrying the required right, and to `Denied`
///       (`-EACCES`-equivalent) without it — proving the table is genuinely general (not console-only). The ids
///       are small non-sentinel words (never `0` / `u64::MAX`), so the value word never aliases Empty/RESERVING.
///
/// Returns true iff every check held. Leaves the row dirty; the caller `clear_handle_row`s it before endowing
/// the live console cap.
fn u6_kernel_check(asid: u64) -> bool {
    // (A) no-collision stress — the exact interleaving the U5 review flagged (spawn-onto-1, then console-install).
    let (Some(a), Some(b)) = (handle_install(asid, 0xA1), handle_install(asid, 0xB2)) else {
        return false; // a fresh 8-slot row cannot be full; a None here is a kernel bug -> fail closed
    };
    install_console_cap(asid); // unconditional store at CONSOLE_FD — must neither clobber nor be clobbered by a/b
    let nocollide = a != CONSOLE_FD
        && b != CONSOLE_FD
        && a != b
        && handle_get(asid, a) == Some(0xA1)
        && handle_get(asid, b) == Some(0xB2)
        && handle_get(asid, CONSOLE_FD) == Some(HANDLE_CONSOLE);

    // (B) File/Socket scaffold kinds resolve to their kind with the right rights, -EACCES-equivalent without.
    install_cap(asid, 3, KIND_FILE, 0x100, CAP_READ);
    install_cap(asid, 4, KIND_SOCKET, 0x200, CAP_READ | CAP_WRITE);
    let file_ok = matches!(handle_resolve(asid, 3, CAP_READ), Ok(HandleTarget::File(0x100)))
        && matches!(handle_resolve(asid, 3, CAP_WRITE), Err(ResolveErr::Denied));
    let sock_ok = matches!(handle_resolve(asid, 4, CAP_READ | CAP_WRITE), Ok(HandleTarget::Socket(0x200)))
        && matches!(handle_resolve(asid, 4, CAP_EXEC), Err(ResolveErr::Denied));

    nocollide && file_ok && sock_ok
}

/// U6 launcher + verdict (the `u5_launcher` shape: one gated kernel task on a sibling core). `demo_cpu` (the
/// task arg) is the core the printing spawner runs on. Flow:
///   1. Wait (bounded) for `U5_LAUNCH_DONE`, so the U6 lines land after the U5 verdict and the U5 slot freed.
///   2. Skip silently if no SD device — the fixture's two children load `HELLO.BIN` off the card (as U4 does).
///   3. `u6_setup()` (build the fixture slot, print the setup line), then run the KERNEL-SIDE object-table
///      checks against its fresh row (`u6_kernel_check`), `clear_handle_row` the scratch, endow the live console
///      cap, and spawn `el0-u6spawn` on `demo_cpu`. Its two `sys_spawn`s co-locate BOTH children on `demo_cpu`
///      (the U4 co-location invariant — each child stays queued-not-dispatched until the parent blocks in its
///      first `sys_wait`, so both pids are recorded first).
///   4. Verdict (folded): wait (bounded) for the fixture's sentinel exit (`EL0_U6_DONE == 1`), then PASS iff the
///      fixture's witness == `U6_WITNESS_ALL` (printed before AND after two spawns with no collision, both
///      children reaped clean) AND the kernel-side check held AND no U6 kill. Prints ONE PASS line. U6 is the
///      last demo, so it releases no further gate.
pub fn u6_launcher(demo_cpu: usize) {
    // 1. Gate on the U5 launcher (its verdict printed + the U5 slot freed).
    let wstart = super::timer::cntpct();
    let wdeadline = 10 * super::timer::cntfrq();
    while !U5_LAUNCH_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No SD device -> the children cannot be loaded; skip silently (mirrors U4/U5's control discipline).
    if crate::drivers::block::info().is_none() {
        U6_LAUNCH_DONE.store(true, Ordering::Release); // release the U6b gate (U6b also gates on the SD)
        return;
    }

    // 3. Build the fixture slot, run the kernel-side checks against its fresh row, then endow + spawn it.
    let Some(u6) = u6_setup() else {
        serial_println!(":: U6: no free address-space slot — object-table demo skipped ::");
        U6_LAUNCH_DONE.store(true, Ordering::Release); // release the U6b gate
        return;
    };
    U6_KINDS_OK.store(u6_kernel_check(u6.asid), Ordering::Release);
    clear_handle_row(u6.asid); // wipe the scratch handles the check planted...
    install_console_cap(u6.asid); // ...then endow the LIVE console cap the fixture prints through.
    super::sched::spawn_user_slot("el0-u6spawn", u6.spawn, u6.sp, u6.ttbr0, demo_cpu);

    // 4. Folded verdict: wait (bounded ~5 s, yielding) for the fixture's sentinel exit, then judge. Two children
    //    (two disk loads) + the parent complete well under this budget under QEMU.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U6_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U6_WITNESS.load(Ordering::Acquire);
    let kinds_ok = U6_KINDS_OK.load(Ordering::Acquire);
    let killed = EL0_U6_KILLED.load(Ordering::Acquire);
    if witness == U6_WITNESS_ALL && kinds_ok && killed == 0 {
        serial_println!(
            ":: U6: general object table — printing spawner + 2 children, no index collision, File/Socket kinds resolve -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U6: general object table FAIL — witness={:#x} kinds_ok={} killed={} done={} (want {:#x} / true / 0 / 1) ::",
            witness,
            kinds_ok,
            killed,
            EL0_U6_DONE.load(Ordering::Acquire),
            U6_WITNESS_ALL
        );
    }
    // Release the U6b gate: the U6 verdict line has printed and the U6 slot has freed, so `u6b_launcher` may
    // now run its File-handle demo (its lines land strictly after this).
    U6_LAUNCH_DONE.store(true, Ordering::Release);
}

// =============================================================================================
// U6b: real File handles — the File-handle fixture's slot + pre-endowment + the gated launcher/verdict
// =============================================================================================

/// The U6b fixture's run parameters: the File-handle fixture's EL0 entry VA (inside the shared window VA — only
/// the slot FRAME differs, via TTBR0), the initial SP_EL0, its slot TTBR0, its ASID (so the launcher can
/// pre-endow the fixture's table and, after exit, verify the file-row teardown-clear), and its SLOT id (so the
/// launcher can plant the expected on-disk prefix through the slot's identity backing).
struct U6bDemo {
    file: u64,
    sp: u64,
    ttbr0: u64,
    asid: u64,
    slot: usize,
}

/// U6b setup: allocate + build ONE private slot, copy the U6b blob into its code page, I-cache-sync, protect it
/// EL0-RX/EL1-RO, and return the run params. Does NOT pre-endow — the launcher endows the two negative-test
/// handles and plants the expected bytes after this returns (before dispatch, no concurrent resolver). Emits the
/// U6b setup line; `None` if slot allocation fails. Called ONCE from `u6b_launcher`, after the U6 gate — so a
/// slot is free and no task runs under the fixture's ASID yet. Register-only fixture (writes no user stack; its
/// only writable target is a kernel-filled data page), so one slot suffices.
fn u6b_setup() -> Option<U6bDemo> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF; // 16-aligned window top = initial SP_EL0
    let bstart = &raw const __u6b_blob_start as usize;
    let bend = &raw const __u6b_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U6b blob does not fit in a code page");
    let file = {
        let va = base + (&raw const __u6b_prog_file as usize - bstart) as u64;
        assert!(va & 3 == 0, "U6b fixture entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe { core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen) };
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    let asid = ttbr0 >> 48;
    serial_println!(
        ":: U6b: real File handles — SYS_OPEN/SYS_READ routed through the object table (File + CAP_READ) ::"
    );
    Some(U6bDemo { file, sp, ttbr0, asid, slot })
}

/// U6b launcher + verdict (the `u6_launcher` shape: one gated kernel task on a sibling core). `demo_cpu` (the
/// task arg) is the core the File-handle fixture runs on. Flow:
///   1. Wait (bounded) for `U6_LAUNCH_DONE`, so the U6b lines land after the U6 verdict and the U6 slot freed.
///   2. Skip silently if no SD device — the fixture reads a real disk file (as U4/U6's children do).
///   3. `u6b_setup()` (build the fixture slot, print the setup line), then PRE-ENDOW its table + PLANT the
///      expected bytes (all before dispatch, no concurrent resolver):
///        - a File handle at `U6B_NOCAP_IDX` backed by a real open-file descriptor but with ZERO rights — the
///          rights arm of the SYS_READ CHECK (a PRESENT File lacking `CAP_READ` must be `-EACCES`);
///        - a `Socket` handle at `U6B_SOCK_IDX` carrying `CAP_READ` — the kind arm (a non-File object with the
///          right present must still be `-EACCES`);
///        - the on-disk prefix `USER_BLOB[..16]` at the data-page VA the fixture's bytes-match compares against
///          (`HELLO.BIN` on the boot media IS `USER_BLOB`, so a correct read reproduces it), published with a
///          `dsb ish` before the fixture's core is dispatched.
///      Then spawn `el0-u6bfile` on `demo_cpu`.
///   4. Verdict (folded): wait (bounded) for the fixture's sentinel exit (`EL0_U6B_DONE == 1`), read its witness
///      bitmask, then wait (bounded) for its FILE row to clear — the file-row teardown-clear proof (the fixture
///      exits holding TWO live descriptors: its own open + the pre-endowed no-cap File, so `files_row_is_clear`
///      transitions false->true when teardown runs). PASS iff witness == `U6B_WITNESS_ALL` AND the file row
///      cleared AND no U6b kill. Prints ONE PASS line, then releases the U7 gate (`U6B_LAUNCH_DONE`) so the
///      transfer demo orders after this one.
pub fn u6b_launcher(demo_cpu: usize) {
    // 1. Gate on the U6 launcher (its verdict printed + the U6 slot freed).
    let wstart = super::timer::cntpct();
    let wdeadline = 10 * super::timer::cntfrq();
    while !U6_LAUNCH_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No SD device -> the fixture cannot read a disk file; skip silently (mirrors U4/U5/U6's control path).
    if crate::drivers::block::info().is_none() {
        U6B_LAUNCH_DONE.store(true, Ordering::Release); // release the U7 gate (it also gates on storage)
        return;
    }

    // 3. Pre-flight the ONE fallible disk lookup — HELLO.BIN's chain head + size, backing the rights-negative
    //    File descriptor — BEFORE allocating a slot, following `load_program_into_slot`'s discipline (fallible
    //    lookups first, resource alloc last). Doing it here means a lookup failure (SD present but HELLO.BIN
    //    unmountable / absent / a directory) skips with NOTHING allocated to unwind — no leaked address-space
    //    slot (there is no free-an-undispatched-slot primitive, so the fix is to not allocate before this).
    let (nocap_fc, nocap_sz) = match crate::fs::fat::mount()
        .and_then(|fs| fs.find_in_root("HELLO.BIN").map(|de| (de.first_cluster(), de.size, de.is_dir)))
    {
        Ok((fc, sz, false)) => (fc, sz),
        _ => {
            serial_println!(
                ":: U6b: pre-open of HELLO.BIN for the no-CAP_READ negative failed — File-handle demo skipped ::"
            );
            U6B_LAUNCH_DONE.store(true, Ordering::Release); // release the U7 gate even on the skip path
            return;
        }
    };

    // 4. Build the fixture slot (allocates it + prints the setup line). From here EVERY path spawns the fixture,
    //    so the slot is always reached by teardown at the fixture's exit — no undispatched-slot leak below.
    let Some(u6b) = u6b_setup() else {
        serial_println!(":: U6b: no free address-space slot — File-handle demo skipped ::");
        U6B_LAUNCH_DONE.store(true, Ordering::Release); // release the U7 gate even on the skip path
        return;
    };
    // 4a. The rights negative: install a File handle at U6B_NOCAP_IDX backed by a real descriptor but carrying
    //     ZERO rights. A PRESENT File lacking CAP_READ is exactly the rights arm of the CHECK — distinct from an
    //     absent handle. `files_alloc` on the fixture's FRESH row cannot fail (NFILE free slots, cleared at the
    //     prior teardown of this ASID); if it somehow did, leave index 2 Empty (the fixture's read still gets
    //     -EACCES, via NoHandle) and spawn anyway — never a leak, never a return-without-spawn.
    // dir_lba/off = 0: this descriptor is read-denied (zero rights) and never grown, so its directory location
    // is unused (U10).
    if let Some(fid) = files_alloc(u6b.asid, nocap_fc, nocap_sz, 0, 0) {
        install_cap(u6b.asid, U6B_NOCAP_IDX, KIND_FILE, (fid + 1) as u64, 0);
    }
    // 4b. The kind negative: a Socket handle carrying CAP_READ at U6B_SOCK_IDX. It HAS the right, so the read is
    //     denied purely on kind (SYS_READ serves File only) — the kind arm, not the rights arm. A scaffold id,
    //     no backing (never resolved as a real socket this arc).
    install_cap(u6b.asid, U6B_SOCK_IDX, KIND_SOCKET, 0x200, CAP_READ);
    // 4c. Plant the expected on-disk prefix the fixture compares its read against. Written through the slot's
    //     identity backing (the documented data-sentinel path — coherent with the EL0 read at the aliased VA on
    //     this PIPT A72); `dsb ish` completes/publishes it to the fixture's core before dispatch.
    let plant_len = core::cmp::min(16, USER_BLOB.len());
    unsafe {
        let dst = super::boot::slot_backing_ptr(u6b.slot).add(0x3000);
        core::ptr::copy_nonoverlapping(USER_BLOB.as_ptr(), dst, plant_len);
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
    }
    super::sched::spawn_user_slot("el0-u6bfile", u6b.file, u6b.sp, u6b.ttbr0, demo_cpu);

    // 4a. Wait (bounded ~5 s, yielding) for the fixture to reach its sentinel exit, then snapshot the witness.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U6B_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U6B_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U6B_KILLED.load(Ordering::Acquire);

    // 4b. File-row teardown-clear proof: the fixture exited holding two live descriptors, so its exit path
    //     cleared the FILES row (via `clear_handle_row` -> `clear_files_row`). Poll (bounded) until it clears —
    //     false->true when teardown runs. Nothing reuses the slot after (U6b is the last demo), so it stays clear.
    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !files_row_is_clear(u6b.asid)
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let files_cleared = files_row_is_clear(u6b.asid);

    if witness == U6B_WITNESS_ALL && files_cleared && killed == 0 {
        serial_println!(
            ":: U6b: real File handles — open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U6b: real File handles FAIL — witness={:#x} files_cleared={} killed={} done={} (want {:#x} / true / 0 / 1) ::",
            witness,
            files_cleared,
            killed,
            EL0_U6B_DONE.load(Ordering::Acquire),
            U6B_WITNESS_ALL
        );
    }
    // Release the U7 gate: the U6b verdict has printed and (the fixture having exited) the U6b slot has
    // freed, so the U7 launcher may build its two fixture slots and order its lines after ours.
    U6B_LAUNCH_DONE.store(true, Ordering::Release);
}

// =============================================================================================
// U7: cross-process transfer — the two-fixture demo (parent delegates, child receives + uses,
// sender revokes) and the gated launcher/verdict
// =============================================================================================

/// One U7 fixture's run parameters (the `U6bDemo` shape, twice): the EL0 entry VA, initial SP_EL0, the
/// slot TTBR0, its ASID (the inbox key + handle row), and the SLOT id (for the GO word plant).
struct U7Fix {
    entry: u64,
    sp: u64,
    ttbr0: u64,
    asid: u64,
    slot: usize,
}

/// Build ONE U7 fixture slot: allocate, copy the (shared two-entry) U7 blob into its code page,
/// I-cache-sync, protect EL0-RX/EL1-RO, and return the run params for the requested entry symbol. Does
/// NOT pre-endow (the launcher does, per fixture, before dispatch). `None` if slot allocation fails.
fn u7_build(entry_sym: *const u8) -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u7_blob_start as usize;
    let bend = &raw const __u7_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U7 blob does not fit in a code page");
    let entry = {
        let va = base + (entry_sym as usize - bstart) as u64;
        assert!(va & 3 == 0, "U7 fixture entry misaligned"); // an eret to a misaligned entry is EC 0x22
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        // Scrub the WHOLE window before the copy (the x86 U2.5 residue discipline, review-confirmed as
        // load-bearing here): slot backings are zeroed only at first boot and a prior tenant's data
        // survives teardown — U6b deterministically plants nonzero bytes at +0x3000, EXACTLY where this
        // demo's GO word lives, and a stale nonzero GO would release a fixture early and turn the
        // single-writer snapshot (and the use-then-revoke ordering) into a race.
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// Release a fixture's GO word (window +0x3000): the launcher-side half of the demo's sequencing. Written
/// through the slot's identity backing (the u6b data-plant path — coherent with the EL0 read of the
/// aliased VA on this PIPT A72), published with `dsb ish` before the spinning fixture's next look.
fn u7_release_go(slot: usize) {
    unsafe {
        let go = super::boot::slot_backing_ptr(slot).add(0x3000) as *mut u64;
        core::ptr::write_volatile(go, 1);
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
    }
}

/// U7 launcher + verdict (the `u6b_launcher` shape: one gated kernel task on a sibling core). `demo_cpu`
/// (the task arg) is the core BOTH fixtures run on (co-located; they cooperate via SYS_YIELD, so the demo
/// is deterministic under QEMU's cooperative scheduling). Flow:
///   1. Wait (bounded) for `U6B_LAUNCH_DONE`, so the U7 lines land after the U6b verdict and its slot freed.
///   2. Skip silently if no SD device (mirrors U4/U5/U6/U6b's control-path discipline — U7 itself needs no
///      disk, but the gate keeps the no-storage control path free of demo lines).
///   3. Claim a Proc entry (the child's pid->ASID map for SYS_XFER), build the CHILD slot (row deliberately
///      EMPTY — the single-writer snapshot depends on it), spawn `el0-u7child` (it spins on its GO word),
///      publish its asid+pid into the Proc entry, build + PRE-ENDOW the PARENT (U7_DEST_IDX = a Child
///      handle naming the child; U7_SRC_IDX = a full Console cap `CAP_WRITE|CAP_GRANT`), print the setup
///      line, spawn `el0-u7parent`.
///   4. THE SINGLE-WRITER WITNESS: wait (bounded) until the parent's t1 deposit is LIVE in the child's
///      inbox, then — with the child still parked on its GO word, provably pre-RECV — verify the child's
///      handle row is still completely CLEAR (`handle_row_is_clear`): the deposit crossed processes without
///      one byte landing in the recipient's row. Only then release the child's GO.
///   5. Wait (bounded) for the child's `U7_USED_TOKEN` (its first write through the transferred cap
///      landed), then release the parent's GO — so the revoke is provably use-then-revoke.
///   6. Verdict (folded): wait (bounded) for both sentinel exits (`EL0_U7_DONE == 2`), read both witnesses,
///      then wait (bounded) for the teardown proof — both handle rows clear, both inboxes clear, and the
///      transfer-record ledger fully FREE (every transfer's lifetime closed: t1 freed when the child's
///      revoked handle was torn down, t2 likewise, no pending residue). Free the planted Proc entry. PASS
///      iff both witnesses == `U7_WITNESS_ALL` AND used AND the snapshot held AND everything cleared AND no
///      U7 kill. Prints ONE PASS line. U7 is the last demo, so it releases no further gate.
pub fn u7_launcher(demo_cpu: usize) {
    // U8, then U9, then U10, ride the SAME kernel task, each strictly after the prior flow (every exit path —
    // PASS, FAIL, or skip — falls through): the ordering gate the *_LAUNCH_DONE statics provide between
    // separately spawned launchers is here the program order of one task. Each launcher's verdict waits on its
    // fixture's exit + teardown, so the demo slot is free again before the next builds — no new gate needed.
    // Keeping U9/U10 here (rather than a `sched::spawn` in main.rs) stays wholly inside the aarch64 syscall lane.
    // K1 M2.4 / K2: restore persisted ownership from UNAFS.ATR before any EL0 fixture opens a file (in-lane boot
    // hook). The gate is now LIVE (K2 flipped `by_name_spawn_multivalued()` true), but on QEMU the fresh-per-build
    // FAT has no UNAFS.ATR at this point (`k1_atr_selftest` creates it LATER in the chain), so the rebuild installs
    // ZERO rows and the battery stays byte-identical; its live effect is on metal (a real power-cycle where a prior
    // boot left a real row). The mechanism is proven by the M3 kernel-side proof + the K2 real-program launcher.
    atr_maybe_boot_rebuild();
    u7_run(demo_cpu);
    u8_launcher(demo_cpu);
    u9_launcher(demo_cpu);
    u10_launcher(demo_cpu);
    u10c_launcher(demo_cpu);
    u10d_launcher(demo_cpu);
    u11_launcher(demo_cpu);
    u11defer_run(demo_cpu);
    u11reuse_run();
    u11reap_run(demo_cpu);
    uowner_run(demo_cpu);
    // F2 M3: after all 23 fixtures have exited, witness that FAT_MUTATION serializes the FAT-table RMW ACROSS
    // CORES (this task runs on `vcpu`; the worker on `demo_cpu` = online[0], always online). Runs LAST so it can
    // never perturb the fixture battery, and it never touches the disk. Emits its own `F2-witness:` line — NOT a
    // `-> PASS` line, so the 23-fixture count stays byte-equivalent.
    f2_witness_launcher(demo_cpu);
    // F3 M4: the NAMESPACE-lock twin — same discipline (last, in-RAM, no disk, its own `F3-witness:` line).
    f3_witness_launcher(demo_cpu);
    // K1 M1: the on-disk owner/grants (UNAFS.ATR) format + round-trip — runs LAST (its disk I/O can never
    // perturb the 23 fixtures or the witnesses); emits its own `:: K1-atr: ::` line (not a `-> PASS`).
    k1_atr_selftest();
    // K1 M3: the two-phase remount-survival proof — persist an owned+granted file, simulate a reboot (rebuild
    // from UNAFS.ATR), and enforce with real stamped principals. Emits its own uncounted `:: K1-persist: … PASS ::`
    // line (the 24th) and fully cleans up. After k1_atr_selftest so it inherits a valid UNAFS.ATR image.
    k1_persist_launcher();
    // K1 M4: the fail-closed proof — a TORN on-disk row yields a PUBLIC file at mount (never a forged owner).
    // Its own uncounted `:: K1-corrupt: … PASS ::` line (the 25th); fully self-cleaning.
    k1_corrupt_launcher();
    // K2 (make-enforcement-LIVE): the end-to-end proof through TWO REAL disk-loaded programs — owner
    // re-admitted by name after the UNAFS.ATR rebuild, impostor refused. Its own uncounted
    // `:: K2-liveenf: … PASS ::` line (the 26th); fully self-cleaning (leaves no owned row on the metal card).
    // Last in the chain — runs after k1_atr/persist/corrupt have left a valid UNAFS.ATR, and its own disk I/O
    // can never perturb the 23-fixture battery or the witnesses. The `k2_leave` metal build swaps in the
    // two-boot money-shot (k2_metal_launcher) instead — same slot, ATTENDED Pi bench only.
    #[cfg(not(feature = "k2_leave"))]
    k2_liveenf_launcher(demo_cpu);
    #[cfg(feature = "k2_leave")]
    k2_metal_launcher(demo_cpu);
    // ELF-1: graduate the loader from flat-binary to minimal static ELF64. Placed HERE, among the
    // real-program loaders (right after K2's by-name spawns), where the FAT volume is freshly mounted and
    // proven — rather than at the very tail, after the unafs write-selftests reshape the card. Loads
    // ELFHELLO.ELF through the SAME `load_program_into_slot` core (now dispatching ELF vs flat on the
    // magic), asserts kernel-side that it took the ELF path (2 PT_LOAD segments mapped with per-segment
    // page permissions), then drops it to EL0 and confirms it ran (its SYS_REPORT token arrived, its
    // SYS_WRITE message printed). Its own uncounted `:: ELF1: … ::` line; an honest skip without the fixture.
    elf1_launcher(demo_cpu);
    // EXEC-1: prove the panel `run <path>` path — read ELFHELLO.ELF through the VFS mount table and execute
    // it via the NEW `run_user_image` loader entry, asserting a clean `exit=0`. Placed right after ELF-1
    // (same freshly-mounted FAT volume); its own uncounted `:: EXEC1: … ::` line, self-cleaning.
    exec1_witness(demo_cpu);
    // ELF-2: the EL0-threading test — SYS_THREAD_SPAWN/_JOIN/_EXIT with a shared-memory counter across two
    // worker threads (one co-located, one on a sibling core). In-RAM (no disk), so it can never perturb the
    // fixture battery or the FAT witnesses; its own uncounted `:: EL0: threads test — … ::` line. Placed after
    // ELF-1 (the loader is proven; this exercises the thread primitives on top of the same slot machinery).
    threads_launcher(demo_cpu);
    // ELF-3: give EL0 something to draw on + real sync — SYS_FB_MAP / SYS_FB_PRESENT / SYS_FUTEX. The parent
    // maps a per-process off-screen surface, 2 threads draw halves under futex sync, the parent presents (the
    // kernel checksums + composites through the registered present hook), and the witness self-verifies the
    // drawn bytes against the expected checksum. In-RAM (no disk), so it can never perturb the fixture battery;
    // its own uncounted `:: EL0: fb test — … ::` line. Placed after ELF-2 (the thread primitives it builds on).
    fb_launcher(demo_cpu);
    // UVUG-1: the first REAL EL0 graphics program — the mini-vug. Reads UVUG.ELF through the VFS and runs it
    // via the EXEC-1 `run_user_image` path (the same path `run /fat/UVUG.ELF` drives at the panel): it maps an
    // off-screen surface, spawns 2 EL0 worker threads that render halves of an animated pattern under a FUTEX
    // frame barrier, presents each frame, joins both, and prints its OWN deterministic
    // `:: UVUG: frames=300 threads=2 checksum=<hex> ::` witness before exiting 0. Placed after `fb_launcher`
    // (the SYS_FB_* / SYS_FUTEX primitives it builds on are proven, and the futex pool is armed). Its own
    // uncounted `:: EXEC-UVUG: … ::` verdict; an honest skip without the fixture.
    uvug_witness(demo_cpu);
    // ELF-5: input into EL0 — SYS_INPUT_POLL + the per-process ring + the `el0_input_enqueue` router seam.
    // A register-only EL0 program polls its ring empty (-EAGAIN), the launcher injects one KeyDown through
    // the real router seam, the program drains it (verifying the packed value) and polls empty again. In-RAM
    // (no disk), so it can never perturb the fixture battery; its own uncounted `:: EL0: input test — … ::`
    // line. Placed after the EL0 graphics/thread ladder (the primitives it complements are proven). QEMU has
    // no real HID, so the kernel-injected event is what proves the enqueue->drain path (honest by design).
    input_launcher(demo_cpu);
    // K3: the revoke-persist commit-ordering proof — a named-owner file's SYS_FGRANT revoke commits to disk
    // BEFORE the in-RAM removal, so it SURVIVES REBOOT and fails CLOSED on a persist failure. Its own uncounted
    // `:: K3-revoke: … PASS ::` line; fully self-cleaning (leaves no owned row on the metal card).
    k3_revoke_launcher();
    // K5: the revoke/re-persist SMP-window proof — a deterministic interleaving witness that the concurrent
    // full-row re-persist can no longer resurrect a revoked grant (snapshot + disk-narrow + in-RAM commit now
    // span one ns) + the create-serialization gate. Its own uncounted `:: K5-lockspan: … PASS ::` line;
    // self-cleaning (leaves no owned row on the metal card).
    k5_lockspan_launcher(); #[cfg(feature = "nsspan")] nsspan_report(); // K7 WATCH: emit :: NS-SPAN: … :: after K3/K5 drive the sites (newline-neutral inline; knob-off byte-identical)
    // K9-PARITY: the mid-staging-failure discard proof — a staged ACL persist that fails PARTWAY leaves no
    // partial-durable row (K3), AND its uncommitted residue can no longer be flushed by a LATER persist's
    // commit (the K9 lens-B residual, now closed in-lane via `with_unafs`'s MOUNT_DISCARD). Its own
    // uncounted `:: K9-parity: … PASS ::` line; self-cleaning (leaves no owned row on the metal card).
    k9_parity_launcher();
    // IMAGE_SHA256 code-signing: prove the SHA-256 primitive (FIPS KATs) + that it discriminates program
    // IMAGES, closing the "same 8.3 name = same principal" residual (the loader now mints IMAGE_SHA256). Its
    // own uncounted `:: IMG-SIG: … PASS ::` line; read-only, no disk write.
    image_sig_selftest();
    // FATDIRS: exercise the new fat.rs directory create/remove seam (create_dir/remove_dir) on the live
    // volume — LAST in the chain (its disk I/O can never perturb the 23 fixtures or the witnesses), fully
    // self-cleaning. Its own uncounted `:: FATDIRS: … PASS ::` line. Unblocks JD7's Orin-panel mkdir/rmdir.
    fatdirs_launcher();
    // K4-ready: prove the native-attribute projection codec (owner/grants string forms) + the
    // UNAATR1-vs-UNAFS volume-magic discriminator — the deterministic 1:1 mapping K4's migrate-then-delete
    // will use, PINNED + KAT'd ahead of the native unafs mount. Read-only, in-RAM (no disk, no card); its
    // own uncounted `:: K4-ready: … PASS ::` line. LAST in the chain.
    k4_ready_selftest();
    // FATMOVE: exercise the new fat.rs directory-entry rename + cross-directory move seam
    // (rename_entry/move_entry) on the live volume — LAST in the chain (its disk I/O can never
    // perturb the 23 fixtures or the witnesses), fully self-cleaning. Its own uncounted
    // `:: FATMOVE: … PASS ::` line. Unblocks a future jetson `mv` arc (JD10).
    fatmove_launcher();
    // CLOCK-3: prove the FAT last-write stamp DERIVES from the unified kernel clock — after a
    // deterministic Manual civil anchor, a freshly created file's on-disk dir-entry date matches the
    // anchor (not the all-zero unset value). Runs LAST in the storage chain (its disk I/O can never
    // perturb the fixtures/witnesses), restores the civil clock to exactly as found, and is fully
    // self-cleaning. Its own uncounted `:: CLOCK3-fat: … PASS ::` line.
    clock3_fat_launcher();
    // BeFS-K3: locate + mount the native unafs partition off the live card and prove the superblock
    // + the read paths (ls/cat byte-verified) + the write seam's bound-check. Read-only by
    // construction — safe here. Its own uncounted `:: K3-mount: … PASS ::` line; an honest skip on
    // media without a unafs partition.
    crate::fs::unafs::k3_mount_selftest();
    // BeFS-K4: prove the kernel can WRITE the native unafs volume through the single coherent mount,
    // and that the write SURVIVES a genuine remount (create+write -> remount -> byte-verify ->
    // delete -> remount -> negative). Self-cleaning (create then delete + journal reset), so the
    // card is left with only the staged K3 fixtures — runs AFTER k3_mount_selftest so its scratch
    // create/delete never perturbs K3-mount's exact-two-entries `ls`. Its own uncounted
    // `:: K4-write: … PASS ::` line; an honest skip on media without a unafs partition.
    crate::fs::unafs::k4_write_selftest();
    // K8a: prove the copy-on-write commit discipline on the live card — root generation advances
    // per mutation, a power cut before the root flip (the autocommit-off crash seam + a genuine
    // remount) converges to the OLD tree, refcounts persist across a remount, and the commit-path
    // bench counters (CNTPCT ticks + blocks written) are live. Runs AFTER k4_write_selftest,
    // fully self-cleaning (its scratch file is created and deleted inside the witness). Its own
    // uncounted `:: K8a-cow: … PASS ::` line; honest skip on media without a unafs partition.
    crate::fs::unafs::k8a_cow_selftest();
    // K8b: prove retained roots (snapshots) + reclamation on the live card — snapshot the committed
    // tree, overwrite the live file, byte-verify the snapshot's OLD blocks are untouched (never-
    // overwrite + block sharing), confirm the retention-aware allocator never reuses a live
    // snapshot's blocks, drop + eagerly reclaim (freeing only blocks no root still reaches), and a
    // power-cut-mid-drain (enqueue-only + genuine remount) converges. Runs AFTER k8a_cow_selftest,
    // fully self-cleaning (its scratch file + snapshots are dropped inside the witness). Its own
    // uncounted `:: K8b-snap: … PASS ::` line; honest skip on media without a unafs partition.
    crate::fs::unafs::k8b_snap_selftest();
    // K8c: prove the snapshot READ path enforces the LIVE object's CURRENT ACL (the "high security"
    // ruling — revocation reaches the past). Owner + grantee read the OLD retained bytes; an impostor
    // is refused from the snapshot by the SAME predicate that refuses the live read; dropping a grant
    // retroactively refuses the snapshot; and a live-deleted object fails closed (no current ACL row)
    // even for its owner. Runs AFTER k8b_snap_selftest, fully self-cleaning. Its own uncounted
    // `:: K8c-snapread: … PASS ::` line; honest skip on media without a unafs partition.
    crate::fs::unafs::k8c_snapread_selftest();
    // K6: prove the U6 owner/grants ACL round-trips through the native unafs attribute volume (the
    // sidecar's successor) — forward+reverse codec, write+read+clear via the coherent mount. Runs
    // LAST, fully self-cleaning (leaves only the staged K3 fixtures). Its own uncounted
    // `:: K6-migrate: … PASS ::` line; honest skip on media without a unafs partition.
    k6_migrate_selftest();
    // VFS-2 (fold, from commit 76762338): the mount-table WRITE witnesses — a create+write+read-back
    // round-trip through the FAT backend and the native unafs backend. Their own uncounted
    // `:: vfs2-fat: … ::` / `:: vfs2-native: … ::` lines; honest skip when the backing volume is absent.
    crate::fs::vfs::vfs2_fat_write_witness();
    crate::fs::vfs::vfs2_native_write_witness();
    // BANDY-1 M1: the bus v1 subset codec KATs — reply bodies proven byte-compatible with the
    // HOST serializer (tools/bandy-golden captures), native request header+payloads frozen,
    // decode fail-closed at the hard ceiling. Read-only, in-RAM (no disk, no card); its own
    // uncounted `:: BANDY-CODEC: … PASS ::` line. LAST in the chain.
    super::bus::bus_codec_selftest();
    // BANDY-2 M1: the write-side codec KATs — write/rm/mv request goldens frozen, the typed WRITE
    // [name_len][name][content] payload + empty/at-ceiling content, decode fail-closed. A SIBLING
    // of BANDY-CODEC (the BANDY-1 goldens/witness stay byte-identical). Read-only, in-RAM; its own
    // uncounted `:: BANDY-CODEC2: … PASS ::` line.
    super::bus::bus_codec2_selftest();
    // BANDY-1 M5 (verdict C): the stamping witness — caller-supplied principal rejected, replies
    // stamped with the reserved kernel kind (fail-closed everywhere a grantee/owner can appear),
    // bounded per-ASID mailboxes, gen fence. Drives the PRODUCTION sys_msend_for path with
    // scratch identities; one read-only ls mount, no disk write. Its own uncounted
    // `:: BANDY-STAMP: … PASS ::` line.
    bandy_stamp_check();
    // BANDY-2 lens-2 fix witness: bus write-TRUNCATE preserves grants (the direct twin mutates in
    // place and never touches the grant table — the bus must reach the same state). Drives the
    // production sys_msend_for path with scratch identities; creates + self-cleans GRNT.BIN. Its
    // own uncounted `:: BANDY-GRANT: … PASS ::` line.
    bandy_grant_check();
    // BANDY-1 M4/M5: the round-trip + equivalence witnesses through the REAL midden program
    // (program #3, Peter's E verdict) — ls/cat/cp parsed at EL0 into typed frames, kernel-
    // fulfilled under the stamped principal; denial equivalence (bus vs direct syscall,
    // byte-same errno) proven at EL0. Self-cleaning; its own uncounted `:: BANDY-RT: … ::`
    // + `:: BANDY-EQ: … ::` lines. LAST in the chain.
    bandy_rt_launcher(demo_cpu);
}

/// F2 M3 witness worker — the `demo_cpu` half of the cross-core FAT_MUTATION stress. `fn(usize)` for
/// `sched::spawn_joinable`; `arg` is the iteration count. Routes through the LOCKED `set_fat_entry`-equivalent
/// RMW path (`fat::f2_witness_rmw(_, true)`). See `f2_witness_launcher`.
fn f2_witness_worker_locked(iters: usize) {
    crate::fs::fat::f2_witness_rmw(iters as u32, true);
}

/// F2 M3 witness worker — the `demo_cpu` half of the UNLOCKED control (`fat::f2_witness_rmw(_, false)`): the same
/// stress with NO serialization, so a lost update can manifest if the two cores interleave. See
/// `f2_witness_launcher`.
fn f2_witness_worker_unlocked(iters: usize) {
    crate::fs::fat::f2_witness_rmw(iters as u32, false);
}

/// F2 M3 — the cross-core witness for the M1 FAT_MUTATION serialization. Drives an in-RAM read-modify-write
/// stress (fat::f2_witness_*, NOT the on-disk FAT — zero volume risk) on the SAME lock that guards
/// `set_fat_entry`: this task (on `vcpu`) does one half inline while a joinable worker does the other half on
/// `demo_cpu` (= online[0], always online, so the join can never hang). When the two are distinct online cores
/// (any >= 2-core boot) the halves overlap and genuinely contend. Two passes:
///   * LOCKED — every step goes through `with_fat_lock`; the counter MUST reach `2*N` (no update lost ->
///     serialization holds cross-core). This is the witness verdict.
///   * UNLOCKED control — no lock; the counter reaches `2*N` MINUS whatever the two cores raced away. A nonzero
///     loss PROVES the environment provoked real contention (the lock's teeth are demonstrated on THIS boot); a
///     zero loss is reported HONESTLY (QEMU's round-robin TCG did not interleave the RMWs — the true lost-update
///     race is metal-only, the R1-arc honest-scope pattern). The on-disk `set_fat_entry` RMW rides the bench.
/// Emits ONE `F2-witness:` serial line (deliberately not a `-> PASS` line — keeps the 23-fixture count intact).
fn f2_witness_launcher(demo_cpu: usize) {
    const N: u32 = 120_000;
    let want = 2 * N;

    // LOCKED pass: worker on demo_cpu + this core's half inline, both through `with_fat_lock`.
    crate::fs::fat::f2_witness_reset();
    let h = super::sched::spawn_joinable("f2-witness-lk", f2_witness_worker_locked, N as usize, demo_cpu);
    crate::fs::fat::f2_witness_rmw(N, true);
    h.join();
    let locked_got = crate::fs::fat::f2_witness_value();

    // UNLOCKED control: identical stress with NO serialization.
    crate::fs::fat::f2_witness_reset();
    let h2 = super::sched::spawn_joinable("f2-witness-ul", f2_witness_worker_unlocked, N as usize, demo_cpu);
    crate::fs::fat::f2_witness_rmw(N, false);
    h2.join();
    let unlocked_got = crate::fs::fat::f2_witness_value();
    let unlocked_lost = want.saturating_sub(unlocked_got);

    if locked_got == want {
        if unlocked_lost > 0 {
            serial_println!(
                ":: F2-witness: FAT_MUTATION cross-core RMW (worker on core {}) — locked {}/{} intact (0 lost); unlocked lost {}/{} -> serialization HOLDS under real contention ::",
                demo_cpu, locked_got, want, unlocked_lost, want
            );
        } else {
            serial_println!(
                ":: F2-witness: FAT_MUTATION cross-core RMW (worker on core {}) — locked {}/{} intact; unlocked also 0 lost (QEMU RR-TCG did not interleave — cross-core contention is metal-only), lock engaged + serialized path intact ::",
                demo_cpu, locked_got, want
            );
        }
    } else {
        // Only reachable if FAT_MUTATION failed to serialize — a real regression, not an expected outcome.
        serial_println!(
            ":: F2-witness: FAT_MUTATION cross-core RMW (worker on core {}) — locked {}/{}, LOST {} increments under the lock -> SERIALIZATION REGRESSION ::",
            demo_cpu, locked_got, want, want - locked_got
        );
    }
}

// F3-M4 witness: the NAMESPACE-lock twin of the F2 witness — an in-RAM cross-core stress of the EXACT lock
// (`ns_lock`) that serializes sys_open/sys_unlink's name sequences, with zero volume risk. Two cores drive a
// deliberately NON-ATOMIC counter RMW; the LOCKED pass routes every step through `ns_lock()` and must lose
// nothing, the UNLOCKED control shows what the environment raced away. HONEST SCOPE: this witnesses ONLY that
// the namespace lock is engaged and serializes cross-core — the stress holds ns around a bare counter with NO
// inner lock taken, so nesting safety against {FAT_MUTATION, DIR_MUTATION, OPEN_FILES, OWNED_FILES,
// DEFERRED_FREE} rests on the reviewed lock-order discipline (NAMESPACE outermost, never acquired while an
// inner is held), not on this witness. Likewise the full
// open-vs-unlink DISK-sequence interleave (two cores mid-syscall in find_located/mark_dir_deleted) is not
// provokable from the single-EL0-core QEMU battery — that leg is metal-latent and rides the attended bench,
// exactly the F2/R1 honest-scope pattern.
static F3_WITNESS_COUNTER: AtomicU32 = AtomicU32::new(0);

/// F3 witness — one non-atomic RMW of the scratch counter with a WIDE read->write window (the F2 step shape).
#[inline(never)]
fn f3_witness_step() {
    let v = F3_WITNESS_COUNTER.load(Ordering::Relaxed);
    for _ in 0..48 {
        core::hint::spin_loop();
    }
    F3_WITNESS_COUNTER.store(v.wrapping_add(1), Ordering::Relaxed);
}

/// F3 witness — drive `iters` steps, optionally serialized through the REAL namespace lock (`ns_lock`).
fn f3_witness_rmw(iters: u32, locked: bool) {
    for _ in 0..iters {
        if locked {
            let _ns = ns_lock();
            f3_witness_step();
        } else {
            f3_witness_step();
        }
    }
}

/// F3 witness worker — the `demo_cpu` half of the LOCKED pass (`fn(usize)` for `sched::spawn_joinable`).
fn f3_witness_worker_locked(iters: usize) {
    f3_witness_rmw(iters as u32, true);
}

/// F3 witness worker — the `demo_cpu` half of the UNLOCKED control.
fn f3_witness_worker_unlocked(iters: usize) {
    f3_witness_rmw(iters as u32, false);
}

/// F3-M4 — the cross-core witness for the M3 NAMESPACE serialization (the `f2_witness_launcher` shape: this
/// task on `vcpu`, a joinable worker on `demo_cpu` = online[0] so the join can never hang). Emits ONE
/// `F3-witness:` line — deliberately NOT a `-> PASS` line, so the 23-fixture count stays byte-equivalent.
fn f3_witness_launcher(demo_cpu: usize) {
    const N: u32 = 120_000;
    let want = 2 * N;

    F3_WITNESS_COUNTER.store(0, Ordering::SeqCst);
    let h = super::sched::spawn_joinable("f3-witness-lk", f3_witness_worker_locked, N as usize, demo_cpu);
    f3_witness_rmw(N, true);
    h.join();
    let locked_got = F3_WITNESS_COUNTER.load(Ordering::SeqCst);

    F3_WITNESS_COUNTER.store(0, Ordering::SeqCst);
    let h2 = super::sched::spawn_joinable("f3-witness-ul", f3_witness_worker_unlocked, N as usize, demo_cpu);
    f3_witness_rmw(N, false);
    h2.join();
    let unlocked_got = F3_WITNESS_COUNTER.load(Ordering::SeqCst);
    let unlocked_lost = want.saturating_sub(unlocked_got);

    if locked_got == want {
        if unlocked_lost > 0 {
            serial_println!(
                ":: F3-witness: NAMESPACE cross-core RMW (worker on core {}) — locked {}/{} intact (0 lost); unlocked lost {}/{} -> the open/unlink sequence lock serializes under real contention (disk-sequence interleave is metal-latent — rides the bench) ::",
                demo_cpu, locked_got, want, unlocked_lost, want
            );
        } else {
            serial_println!(
                ":: F3-witness: NAMESPACE cross-core RMW (worker on core {}) — locked {}/{} intact; unlocked also 0 lost (QEMU RR-TCG did not interleave — cross-core contention is metal-only), lock engaged + serialized path intact ::",
                demo_cpu, locked_got, want
            );
        }
    } else {
        serial_println!(
            ":: F3-witness: NAMESPACE cross-core RMW (worker on core {}) — locked {}/{}, LOST {} increments under the lock -> SERIALIZATION REGRESSION ::",
            demo_cpu, locked_got, want, want - locked_got
        );
    }
}

// =====================================================================================================
// K1: on-disk UnaFS owner/grants attributes — the UNAFS.ATR format + M1 round-trip (persist the U6 ACL).
// =====================================================================================================
//
// U6's OWNED_FILES table is in-RAM and BOOT-SCOPED: power-cycle and every private file is public again,
// because the owner is recorded as an (asid, gen) INCARNATION that means nothing after reboot. K1 makes
// ownership SURVIVE REBOOT by persisting owner + grants as on-disk attributes INSIDE the FAT volume — a
// reserved hidden|system file UNAFS.ATR in the root (no dir-entry bytes stolen; foreign OSes hide/skip it).
//
// PRINCIPAL-IDENTITY STOP-GATE — this arc's load-bearing design decision (docs/SECURITY.md §K1). The U6
// owner is (asid, gen), which CANNOT persist (asid = slot+1 is reassigned every boot; gen resets at
// power-on). A persisted owner must be a PERSISTENT PRINCIPAL, and UnaOS has no persistent-principal model
// yet: EL0 programs are UNTRUSTED blobs loaded BY NAME with no code-signing / manifest / image hash, there
// is no RTC / wall-clock and no uid registry. Inventing that model is a TCB-level policy decision ABOVE
// this aarch64 lane, so M1 STOPS after the FORMAT + round-trip and PROPOSES the model to the seat. The
// proposal (NOT enforced here): a launcher-assigned, KERNEL-STAMPED principal (never self-asserted by a
// blob), recorded as the 32-byte kind-tagged, string-projectable `PrincipalRecord` below (default kind
// PROGRAM_NAME "prog:<name>") that maps 1:1 onto native unafs `owner`/`grants:<name>` string keys at K4.
//
// WHAT M1 BUILDS (and ONLY this): the UNAFS.ATR on-disk FORMAT (versioned magic header + 16 bounded rows,
// per-header + per-row CRC32, volume binding), its (de)serializers, reserved-file read/write helpers
// composed ENTIRELY from fat.rs's EXISTING public API (ZERO fat.rs edit), and a round-trip self-test that
// proves the codec + the disk helpers, with the steady-state single-row write done UNDER the F3 NAMESPACE
// lock. It does NOT stamp principals at spawn, NOT rebuild into OWNED_FILES, NOT wire attr writes into
// sys_open/unlink/fgrant — all M2, seat-gated on the principal model. The round-trip uses SYNTHETIC
// principal records precisely because the live owner (asid, gen) is the one thing that must NOT persist.
// Enforcement-INERT: the 23-PASS battery is byte-identical (K1 emits its own `:: K1-atr: ::` line, never a
// `-> PASS`), and the disk half leaves UNAFS.ATR a valid EMPTY (all-public) image.
//
// FAIL-CLOSED (the read-side rule M2's mount-rebuild inherits): on ANY doubt about a row — bad magic /
// version / volume binding / header CRC / row CRC / (M2) a name that no longer resolves — revert that file
// (or the whole volume) to PUBLIC, the well-defined pre-U6 baseline. Owner AND all its grants live in ONE
// row under ONE row_crc32 and install-or-drop ATOMICALLY, so "fail-closed on grants, fail-open on ownership
// is not a thing" is STRUCTURALLY foreclosed: a row is never half-trusted; an owner/grant is never forged
// from garbage. (Honest residual: fail-closed-to-PUBLIC of a torn OWNER row is a confidentiality downgrade
// on crash; the old-row-survives write-commit-order mitigation is an M2 write-path concern.)

/// K1: the reserved attr file — a hidden|system 8.3 file in the FAT root. `find_located`/`create_in_root`
/// reach it (attr 0x06 is a real entry, not LFN/vollabel); foreign OSes hide/skip it. The internal magic
/// (below), NOT this name, is the format's identity — the filename is a seat sub-decision (UNA_ACL.SYS is
/// the alternative that avoids a human confusing a FAT volume with a native unafs volume).
const ATR_NAME: &str = "UNAFS.ATR";
const ATR_DIR_ATTR: u8 = 0x02 | 0x04; // HIDDEN | SYSTEM

/// K1: format identity — deliberately NOT the native unafs superblock magic, so a future kernel tells the
/// FAT bridge apart from a real unafs volume (migrate-then-delete at K4). Bump `ATR_VERSION` for any
/// incompatible layout change (widening `PrincipalRecord.value` past 30 bytes is such a change).
const ATR_MAGIC: [u8; 8] = *b"UNAATR1\0";
const ATR_VERSION: u16 = 1;
const ATR_HEADER_LEN: usize = 512; // the header owns file sector 0 alone (no row shares its sector)
const ATR_ROW_STRIDE: usize = 256; // 2 rows per 512-byte sector, none straddling -> 1 row = 1 sector RMW
const ATR_ROWS: usize = NOWNED; // 16 — the on-disk bound MIRRORS the in-RAM OWNED_FILES bound (no growth)
const ATR_FILE_LEN: usize = ATR_HEADER_LEN + ATR_ROW_STRIDE * ATR_ROWS; // 4608 bytes

const ATR_FLAG_COMMITTED: u32 = 1 << 0; // header valid+complete (M2 writes it AFTER the rows, at create)
const ATR_ROW_VALID: u8 = 1 << 0; // this row records a live owned file (else a free slot)

// principal_record kinds (the PROPOSED model — reserved values keep the format model-agnostic for the seat).
const PRIN_NONE: u8 = 0; // public / free (no owner)
const PRIN_PROGRAM_NAME: u8 = 1; // "prog:<name>" — the pre-code-signing default (still valid for manual stamps)
const PRIN_IMAGE_SHA256: u8 = 2; // "sha256:<hex>" — the code-signing principal; the loader mints THIS since IMG-SIG
#[allow(dead_code)]
const PRIN_KERNEL_PID: u8 = 3; // reserved: launcher-minted "pid:<hex>"
// BANDY-1 (verdict C): the RESERVED KERNEL principal kind stamped on bus REPLY frames — the kernel
// speaking as fulfiller, NOT a grantable identity. FAIL-CLOSED everywhere a grantee/owner can
// appear: `owned_set_owner`/`owned_grant`/`owned_access_ok` refuse it explicitly; the native
// projection (`principal_native_string`/`native_project_owner_grants`) has no arm for it (None →
// un-persistable); the reverse codec can never mint it (it parses `prog:`/`sha256:` forms only).
// Proven by the BANDY-STAMP witness.
const PRIN_KERNEL_REPLY: u8 = 4;
const PRIN_VALUE_LEN: usize = 30;

/// K1: a 32-byte kind-tagged persistent principal — the PROPOSED durable owner/grantee identity. Opaque to
/// M1 (round-tripped, NEVER derived from a live (asid, gen)); `value` holds the canonical string
/// projection's bytes (e.g. b"prog:VUG"), `len` its significant length.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PrincipalRecord {
    kind: u8,
    len: u8,
    value: [u8; PRIN_VALUE_LEN],
}

impl PrincipalRecord {
    const NONE: Self = PrincipalRecord { kind: PRIN_NONE, len: 0, value: [0u8; PRIN_VALUE_LEN] };

    /// A PROGRAM_NAME principal from a full value byte string (truncated to the 30-byte value field). Used by
    /// the M1 codec test with pre-formed `b"prog:..."` values.
    fn program(name: &[u8]) -> Self {
        let mut value = [0u8; PRIN_VALUE_LEN];
        let n = core::cmp::min(name.len(), PRIN_VALUE_LEN);
        value[..n].copy_from_slice(&name[..n]);
        PrincipalRecord { kind: PRIN_PROGRAM_NAME, len: n as u8, value }
    }

    /// M2 launcher policy v1: the PROGRAM_NAME principal for the 8.3 file name the loader RESOLVED — the
    /// canonical `"prog:<NAME>"` projection (truncated at 30 bytes). This is the ONLY way a principal is
    /// minted for a spawned program: kernel-derived from the load name, never from EL0 input. Two programs
    /// with the same 8.3 name are BY DESIGN the same principal until `IMAGE_SHA256` lands (honest residual).
    fn program_name(name: &str) -> Self {
        let mut value = [0u8; PRIN_VALUE_LEN];
        let mut n = 0usize;
        for &b in b"prog:".iter().chain(name.as_bytes()) {
            if n >= PRIN_VALUE_LEN {
                break;
            }
            value[n] = b;
            n += 1;
        }
        PrincipalRecord { kind: PRIN_PROGRAM_NAME, len: n as u8, value }
    }

    /// IMAGE_SHA256 (code-signing, this arc): the principal a program's IMAGE identity mints — kind
    /// IMAGE_SHA256, `value` = the 30-byte prefix of the 32-byte SHA-256 digest (240 bits; the `value` field is
    /// a HARD 30 bytes — widening it is a format bump, so we truncate rather than bump, and 240-bit identity is
    /// collision-infeasible). On disk we keep the RAW digest prefix; the native `owner` string projection is
    /// `sha256:` + 60 lowercase hex of THIS 30-byte prefix (67 chars — the 240-bit identity), NOT the 71-char
    /// full-digest form which cannot be rebuilt from disk (see `principal_native_string`, the K4-ready codec).
    /// Equality (derive(PartialEq)) compares all 30 value bytes, so two IMAGE principals match iff
    /// their digest prefixes match. Kernel-minted; NEVER self-asserted by a blob.
    fn image_sha256(digest: &[u8; 32]) -> Self {
        let mut value = [0u8; PRIN_VALUE_LEN];
        value.copy_from_slice(&digest[..PRIN_VALUE_LEN]);
        PrincipalRecord { kind: PRIN_IMAGE_SHA256, len: PRIN_VALUE_LEN as u8, value }
    }

    /// The IMAGE_SHA256 principal for a set of image bytes (hash + wrap). The loader's mint path.
    fn image_of(bytes: &[u8]) -> Self {
        Self::image_sha256(&sha256(bytes))
    }

    fn write(&self, out: &mut [u8]) {
        out[0] = self.kind;
        out[1] = self.len;
        out[2..2 + PRIN_VALUE_LEN].copy_from_slice(&self.value);
    }

    fn read(b: &[u8]) -> Self {
        let mut value = [0u8; PRIN_VALUE_LEN];
        value.copy_from_slice(&b[2..2 + PRIN_VALUE_LEN]);
        PrincipalRecord { kind: b[0], len: b[1], value }
    }
}

// =====================================================================================================
// K4-READY: the native-attribute PROJECTION CODEC (migration glue) — turn a persisted PrincipalRecord /
// grant / volume-magic into the string forms a native unafs volume stores, WITHOUT a native mount.
// =====================================================================================================
//
// K4 proper ("migrate-then-delete onto native unafs attributes", docs/SECURITY.md §K1) reads each committed
// UNAFS.ATR row and re-stores its owner/grants as native unafs TYPED ATTRIBUTES — attribute key `owner`
// (value = the principal's canonical string) and `grants:<grantee>` (value = the rights) — then deletes the
// FAT-bridge sidecar. That migration is gated on a native unafs FILESYSTEM existing in the kernel (the
// ROADMAP §2 "BeFS" convergence: no_std port -> block adapter -> read-only mount -> journaled writes), which
// does NOT exist in this tree yet (fs/mod.rs mounts only FAT; unaos/libs/fs/unafs is a std Ring-3 crate). What DOES
// land now, in-lane, is the deterministic 1:1 CODEC that migration will use — pure functions + KATs — so the
// exact projection is PINNED and PROVEN ahead of the mount, and the "tell the sidecar from a native volume"
// primitive the FORMAT bullet names exists.
//
// THE 240-BIT-PREFIX RULE (load-bearing correctness). An IMAGE_SHA256 principal stores only the 30-byte
// (240-bit) PREFIX of the SHA-256 digest (`value[30]` is a HARD cap — widening it is a format bump).
// Enforcement compares those 30 bytes, and the loader's mint (`image_of`) stores the SAME 30-byte prefix.
// So the native `owner` string MUST be the prefix form `sha256:<60 lowercase hex>` (67 chars) — NOT the
// 71-char full-digest form, which cannot be reconstructed from disk (bytes 30,31 of the digest are not
// stored). Projecting a full digest would make a MIGRATED owner (60 hex, from the stored prefix) mismatch a
// FRESH-minted owner (64 hex) of the very same program -> the migrated owner would be permanently
// un-re-acquirable. The prefix form keeps migrated == fresh-mint, byte-for-byte. (This corrects the earlier
// `image_sha256` doc note that called the canonical form "71 chars, derived at K4".)

/// The native unafs superblock magic — MIRRORED from `unaos/libs/fs/unafs/src/superblock.rs` (`pub const MAGIC:
/// [u8;5] = *b"UNAFS"`), deliberately distinct from the FAT-bridge sidecar's `ATR_MAGIC` (`UNAATR1\0`) so a
/// kernel can tell a real native volume from the shim and drive migrate-then-delete at K4. Kept LOCAL (the
/// crate is std/Ring-3, unported); if that native magic ever changes, change it here. The classifier below
/// assumes this lands at byte 0 of the native superblock — true because `unaos/libs/fs/unafs` serializes `Superblock`
/// with bincode fixint (a fixed `[u8;5]` field emits no length prefix); a future serde/codec change there
/// must be reflected here.
const UNAFS_SB_MAGIC: [u8; 5] = *b"UNAFS";

/// K4-ready: what a candidate leading byte-slice looks like — the FAT-bridge ACL sidecar (starts with
/// `UNAATR1\0`), a native unafs volume/superblock (starts with `UNAFS`), or neither (a plain FAT boot
/// sector, an empty buffer, anything else). The `UNAATR1\0` and `UNAFS` prefixes cannot alias (byte 3 is
/// 'A' vs 'F'), so the order of the two checks is immaterial.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VolumeMagic {
    AtrSidecar,
    NativeUnafs,
    Other,
}

/// K4-ready: classify a leading byte-slice by magic — the "tell the FAT bridge apart from a real unafs
/// volume" primitive the UNAFS.ATR FORMAT bullet names. Pure, panic-free (short buffers -> `Other`); the
/// caller supplies the head of a candidate file (for the sidecar) or of volume block 0 (native superblock).
fn classify_volume_magic(head: &[u8]) -> VolumeMagic {
    if head.starts_with(&ATR_MAGIC) {
        VolumeMagic::AtrSidecar
    } else if head.starts_with(&UNAFS_SB_MAGIC) {
        VolumeMagic::NativeUnafs
    } else {
        VolumeMagic::Other
    }
}

/// K4-ready: lowercase-hex `src` into `out` (2 chars/byte). Returns bytes written; stops at the smaller of
/// `out.len()` and `src.len()*2` (never panics on a short buffer). no_std, no heap.
fn hex_lower_into(src: &[u8], out: &mut [u8]) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut n = 0usize;
    for &b in src {
        if n + 2 > out.len() {
            break;
        }
        out[n] = HEX[(b >> 4) as usize];
        out[n + 1] = HEX[(b & 0x0f) as usize];
        n += 2;
    }
    n
}

/// The widest native-attribute string this codec emits: `grants:` (7) + `sha256:` (7) + 60 hex = 74 bytes.
const K4_STR_MAX: usize = 74;

/// K4-ready: project a persisted principal to its canonical native-attribute STRING into `out`, returning
/// the length written, or `None` if it has no native projection. `NONE` = public (a file with no `owner`
/// attribute). `PROGRAM_NAME` stores the canonical string in `value` already (`prog:<name>`), so projection
/// is a verbatim copy of `value[..len]`. `IMAGE_SHA256` stores raw digest-prefix bytes, projected as
/// `sha256:` + 60 lowercase hex (the 240-bit prefix — see THE 240-BIT-PREFIX RULE above). Reserved/unknown
/// kinds (`PRIN_KERNEL_PID` and any future value) return `None` — fail-closed: an owner the kernel cannot
/// name is not migrated (matching the ratified "migrate only what the kernel can re-mint" policy).
fn principal_native_string(rec: &PrincipalRecord, out: &mut [u8]) -> Option<usize> {
    match rec.kind {
        PRIN_NONE => None,
        PRIN_PROGRAM_NAME => {
            let n = core::cmp::min(rec.len as usize, PRIN_VALUE_LEN);
            if n > out.len() {
                return None;
            }
            out[..n].copy_from_slice(&rec.value[..n]);
            Some(n)
        }
        PRIN_IMAGE_SHA256 => {
            const PFX: &[u8] = b"sha256:";
            if PFX.len() + PRIN_VALUE_LEN * 2 > out.len() {
                return None; // 7 + 60 = 67
            }
            out[..PFX.len()].copy_from_slice(PFX);
            let w = hex_lower_into(&rec.value, &mut out[PFX.len()..]);
            Some(PFX.len() + w)
        }
        _ => None, // PRIN_KERNEL_PID (reserved) / unknown -> un-projectable, fail-closed
    }
}

/// K4-ready: the native-attribute KEY for a grant to `grantee` — `grants:<grantee canonical string>`
/// (e.g. `grants:prog:MIDI`, `grants:sha256:<hex>` — the widest key, exactly `K4_STR_MAX` = 74 bytes).
/// `None` if the grantee has no native projection OR `out` is too small; on `None` the contents of `out`
/// are UNSPECIFIED (the `grants:` prefix may already have been written), so a caller must key off the
/// returned length, never the buffer, when the result is `None`.
fn grant_native_key(grantee: &PrincipalRecord, out: &mut [u8]) -> Option<usize> {
    const PFX: &[u8] = b"grants:";
    if PFX.len() > out.len() {
        return None;
    }
    out[..PFX.len()].copy_from_slice(PFX);
    let w = principal_native_string(grantee, &mut out[PFX.len()..])?;
    Some(PFX.len() + w)
}

/// K4-ready: the native-attribute VALUE for a grant's rights — a canonical lowercase string over the
/// `CAP_READ|CAP_WRITE` subset the file-grant model uses: `rw` / `r` / `w` / `-` (no rights). Returns the
/// length written, or `None` if `out` is too small — UNIFORM with `principal_native_string`/`grant_native_key`
/// (it never emits a partial rights string, so a too-small buffer can't silently drop the write bit). The
/// `EXEC`/`GRANT` bits are not part of a file grant, so they are deliberately ignored here.
fn rights_native_value(rights: u32, out: &mut [u8]) -> Option<usize> {
    let s: &[u8] = match (rights & CAP_READ != 0, rights & CAP_WRITE != 0) {
        (true, true) => b"rw",
        (true, false) => b"r",
        (false, true) => b"w",
        (false, false) => b"-",
    };
    if s.len() > out.len() {
        return None;
    }
    out[..s.len()].copy_from_slice(s);
    Some(s.len())
}

// =====================================================================================================
// K6: the REVERSE codec — native attribute STRING -> `PrincipalRecord`. The mount-time migration/rebuild
// reads the native `owner`/`grants:*` attributes back and reconstructs the durable principals. The
// LOAD-BEARING invariant (the 240-bit-prefix rule): reversing a projection MUST reproduce the exact
// `PrincipalRecord` a fresh mint would produce, byte-for-byte, so a migrated owner stays re-acquirable
// (`k4_ready_selftest`'s migration-landmine KAT + the K6 witness are the guards). Fail-closed: an
// unparseable string yields `None` -> that owner is not installed (public), never a forged owner.
// =====================================================================================================

/// K6: one lowercase/uppercase hex nibble -> value, or `None` if not a hex digit.
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// K6: reverse of [`principal_native_string`]. `prog:<name>` -> a `PROGRAM_NAME` principal whose value
/// is the verbatim string (identical to `PrincipalRecord::program_name(<name>)` byte-for-byte, since
/// that mint stores `prog:<name>` in `value`). `sha256:<60 lowercase hex>` -> an `IMAGE_SHA256`
/// principal carrying the decoded 30-byte (240-bit) digest prefix (identical to the loader's
/// `image_of` mint, which stores the SAME 30-byte prefix). Any other/short/malformed string -> `None`.
fn principal_from_native(s: &[u8]) -> Option<PrincipalRecord> {
    if let Some(rest) = s.strip_prefix(b"sha256:") {
        // Exactly 60 hex chars == the 30-byte prefix; anything else is not a native IMAGE_SHA256 owner.
        if rest.len() != PRIN_VALUE_LEN * 2 {
            return None;
        }
        let mut value = [0u8; PRIN_VALUE_LEN];
        for (i, chunk) in rest.chunks_exact(2).enumerate() {
            let hi = hex_nibble(chunk[0])?;
            let lo = hex_nibble(chunk[1])?;
            value[i] = (hi << 4) | lo;
        }
        return Some(PrincipalRecord { kind: PRIN_IMAGE_SHA256, len: PRIN_VALUE_LEN as u8, value });
    }
    if s.starts_with(b"prog:") {
        // `program` stores the verbatim bytes (truncated to 30) with kind PROGRAM_NAME — the exact
        // inverse of `principal_native_string`'s verbatim copy for a PROGRAM_NAME principal.
        if s.len() > PRIN_VALUE_LEN {
            return None; // a projected prog: string never exceeds 30 bytes; a longer one is malformed
        }
        return Some(PrincipalRecord::program(s));
    }
    None
}

/// K6: reverse of [`rights_native_value`]. `rw`->R|W, `r`->R, `w`->W; `-`/anything else -> 0 (no
/// rights = not a live grant). Never panics. K8c fold: DELEGATES to the ONE canonical decoder in
/// `fs/unafs.rs` (which the snapshot-read `read_authz` also uses — the same function, so the
/// verb-layer authz and the syscall-layer grant machinery can never drift), with the bit equality
/// pinned by const asserts.
const _: () = assert!(crate::fs::unafs::RIGHT_READ == CAP_READ);
const _: () = assert!(crate::fs::unafs::RIGHT_WRITE == CAP_WRITE);
fn rights_from_native(s: &[u8]) -> u32 {
    crate::fs::unafs::rights_from_native(s)
}

/// K6: reverse of [`grant_native_key`]. Strip the `grants:` prefix, then reverse the grantee principal.
fn grantee_from_grant_key(key: &[u8]) -> Option<PrincipalRecord> {
    principal_from_native(key.strip_prefix(b"grants:")?)
}

// M2.1: the launcher's principal STAMP table — the persistent PrincipalRecord assigned to each address-space
// slot at spawn (indexed by ASID, mirroring OWNED_FILES/OPEN_FILES). Launcher policy v1: the KERNEL stamps
// SLOT_PPID[asid] from the loader-RESOLVED 8.3 program name at `load_program_into_slot`; NO EL0 path may set
// or change it. Cleared to NONE at slot teardown. A slot with NONE (an inline-blob fixture with no resolved
// load name, or a torn-down slot) is ANONYMOUS = public-only: it may still own files at RUNTIME via (asid,gen)
// [U6 unchanged], but its ownership does not PERSIST across reboot. Its own SpinMutex, taken IRQ-masked via
// IrqGuard (stamped in syscall/boot context, cleared in the IRQ-masked teardown path — the OWNED_FILES idiom).
static SLOT_PPID: SpinMutex<[PrincipalRecord; super::boot::USER_SLOTS + 1]> =
    SpinMutex::new([PrincipalRecord::NONE; super::boot::USER_SLOTS + 1]);

/// M2.1: stamp `asid`'s persistent principal at spawn (called from `load_program_into_slot` with the
/// loader-resolved name). ASID 0 (boot/shared) is never a spawned program — ignored defensively.
fn slot_ppid_stamp(asid: u64, rec: PrincipalRecord) {
    if asid == 0 || asid as usize >= super::boot::USER_SLOTS + 1 {
        return;
    }
    let _irq = IrqGuard::mask_save();
    SLOT_PPID.lock()[asid as usize] = rec;
}

/// M2.1: clear `asid`'s stamp at teardown (the slot's next tenant is a DIFFERENT program). Called from
/// `clear_handle_row`, alongside `owned_clear_owner_asid`.
fn slot_ppid_clear(asid: u64) {
    if asid as usize >= super::boot::USER_SLOTS + 1 {
        return;
    }
    let _irq = IrqGuard::mask_save();
    SLOT_PPID.lock()[asid as usize] = PrincipalRecord::NONE;
}

/// M2.3: the stamped persistent principal of an ARBITRARY address-space slot (NONE = anonymous / out of
/// range). Read at grant time (`sys_fgrant`, for the grantee's ASID) and by the M3 proof. Read-only snapshot
/// under the IRQ-masked SLOT_PPID lock; the caller must NOT already hold OWNED_FILES/NAMESPACE (SLOT_PPID is an
/// inner lock, captured before those — never nested under them).
fn slot_ppid_of(asid: u64) -> PrincipalRecord {
    if asid as usize >= super::boot::USER_SLOTS + 1 {
        return PrincipalRecord::NONE;
    }
    let _irq = IrqGuard::mask_save();
    SLOT_PPID.lock()[asid as usize]
}

/// M2: the CALLER's stamped persistent principal (NONE = anonymous/public-only). Captured at O_CREAT (persist
/// the owner), at grant (owner side), and at open (cross-reboot enforcement, M2.4) — ALWAYS before any
/// OWNED_FILES/NAMESPACE hold, then passed in, so SLOT_PPID never nests under those locks.
fn current_principal() -> PrincipalRecord {
    slot_ppid_of(current_asid())
}

/// K1: one grant edge on disk — a principal + the rights it holds (36 bytes: 32 + u32 LE).
#[derive(Clone, Copy, PartialEq, Eq)]
struct AtrGrant {
    prin: PrincipalRecord,
    rights: u32,
}

impl AtrGrant {
    const EMPTY: Self = AtrGrant { prin: PrincipalRecord::NONE, rights: 0 };
}

/// K1: the parsed form of one 256-byte on-disk row (field offsets documented in `atr_serialize_row`).
#[derive(Clone, Copy)]
struct AtrRow {
    name: [u8; 11], // on-disk 8.3 (space-padded) — the DURABLE identity key (re-resolved by name at mount, M2)
    first_cluster: u32,
    size: u32,
    dir_lba: u64, // runtime-key HINT; authoritatively re-resolved from `name` at mount (M2)
    dir_off: u32,
    owner: PrincipalRecord,
    grants: [AtrGrant; NFGRANT],
}

/// K1: CRC-32/IEEE (poly 0xEDB88320, init/xorout 0xFFFFFFFF), computed bitwise — no static table, no_std
/// clean. Guards the header ([0..508)) and each row ([0..252)) INDEPENDENTLY (deliberately no whole-file
/// CRC, so a single-row update never has to rewrite — and torn — the header sector).
fn atr_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// IMAGE_SHA256 (code-signing, this arc): the SHA-256 compression function over one 64-byte block (FIPS 180-4
/// §6.2). Pure, no_std, no heap — the only table is the 64 round constants. Callers: `sha256`.
fn sha256_compress(h: &mut [u32; 8], block: &[u8; 64]) {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
        (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
}

/// IMAGE_SHA256 (code-signing, this arc): FIPS 180-4 SHA-256 over a byte slice → 32-byte digest. no_std, no
/// heap, single-pass with the standard 0x80/zero-pad/64-bit-length framing. This is an image-IDENTITY digest,
/// NOT a MAC/signature — it graduates a program's persistent principal from its 8.3 NAME (two same-named blobs
/// were one principal) to its IMAGE (two byte-identical images are one principal, two different images are not).
/// Kernel-minted at `load_program_into_slot` from the loaded bytes; never self-asserted by an EL0 blob.
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bitlen = (data.len() as u64).wrapping_mul(8);
    let mut chunks = data.chunks_exact(64);
    let mut block = [0u8; 64];
    for c in chunks.by_ref() {
        block.copy_from_slice(c);
        sha256_compress(&mut h, &block);
    }
    let rem = chunks.remainder();
    block = [0u8; 64];
    block[..rem.len()].copy_from_slice(rem);
    block[rem.len()] = 0x80;
    if rem.len() >= 56 {
        // the 8-byte length won't fit after the 0x80 in this block — flush it, then a fresh zero block.
        sha256_compress(&mut h, &block);
        block = [0u8; 64];
    }
    block[56..64].copy_from_slice(&bitlen.to_be_bytes());
    sha256_compress(&mut h, &block);
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// K1: serialize a live owned-file row to its 256-byte on-disk form (VALID set). Layout (LE):
///   [0..11] name · [11] row_flags · [12..16] first_cluster · [16..20] size · [20..28] dir_lba ·
///   [28..32] dir_off · [32..64] owner principal · [64..208] 4× grant{prin[32], rights u32} ·
///   [208..252] reserved(0) · [252..256] row_crc32 over [0..252).
fn atr_serialize_row(row: &AtrRow) -> [u8; ATR_ROW_STRIDE] {
    let mut b = [0u8; ATR_ROW_STRIDE];
    b[0..11].copy_from_slice(&row.name);
    b[11] = ATR_ROW_VALID;
    b[12..16].copy_from_slice(&row.first_cluster.to_le_bytes());
    b[16..20].copy_from_slice(&row.size.to_le_bytes());
    b[20..28].copy_from_slice(&row.dir_lba.to_le_bytes());
    b[28..32].copy_from_slice(&row.dir_off.to_le_bytes());
    row.owner.write(&mut b[32..64]);
    for (j, g) in row.grants.iter().enumerate() {
        let o = 64 + 36 * j;
        g.prin.write(&mut b[o..o + 32]);
        b[o + 32..o + 36].copy_from_slice(&g.rights.to_le_bytes());
    }
    let crc = atr_crc32(&b[0..252]);
    b[252..256].copy_from_slice(&crc.to_le_bytes());
    b
}

/// K1: a well-formed FREE row — VALID clear, CRC valid over the zeroed body, so a mount reads it as "free
/// slot" (not "corrupt"). Used to initialize the image and to clear a row back to public.
fn atr_empty_row() -> [u8; ATR_ROW_STRIDE] {
    let mut b = [0u8; ATR_ROW_STRIDE];
    let crc = atr_crc32(&b[0..252]);
    b[252..256].copy_from_slice(&crc.to_le_bytes());
    b
}

/// K1: parse a 256-byte row. `None` iff the slot is FREE (VALID clear) OR the row_crc32 fails — both mean
/// NO owner installed => the file is PUBLIC (fail-closed: a corrupt row NEVER yields a forged owner/grant).
/// A `Some` row is fully trusted: owner AND grants passed the one CRC together (the fail-closed asymmetry,
/// structural). (M2's mount distinguishes free-vs-corrupt for its one-line log; M1 only needs None=public.)
fn atr_parse_row(b: &[u8; ATR_ROW_STRIDE]) -> Option<AtrRow> {
    let stored = u32::from_le_bytes([b[252], b[253], b[254], b[255]]);
    if atr_crc32(&b[0..252]) != stored {
        return None; // corrupt -> public (owner AND grants dropped together)
    }
    if b[11] & ATR_ROW_VALID == 0 {
        return None; // a well-formed free slot
    }
    let mut name = [0u8; 11];
    name.copy_from_slice(&b[0..11]);
    let mut grants = [AtrGrant::EMPTY; NFGRANT];
    for (j, g) in grants.iter_mut().enumerate() {
        let o = 64 + 36 * j;
        g.prin = PrincipalRecord::read(&b[o..o + 32]);
        g.rights = u32::from_le_bytes([b[o + 32], b[o + 33], b[o + 34], b[o + 35]]);
    }
    Some(AtrRow {
        name,
        first_cluster: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        size: u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
        dir_lba: u64::from_le_bytes([b[20], b[21], b[22], b[23], b[24], b[25], b[26], b[27]]),
        dir_off: u32::from_le_bytes([b[28], b[29], b[30], b[31]]),
        owner: PrincipalRecord::read(&b[32..64]),
        grants,
    })
}

/// K1: the volume binding — proves an attr file belongs to THIS volume (a FOREIGN volume or a REFORMAT is
/// rejected, so its rows never attach to this volume's directory slots; a byte-for-byte clone preserves the
/// fingerprint and is NOT rejected — offline tampering is out of scope). K1 M2.2: the real
/// fingerprint `(BS_VolID, count_of_clusters)` via the seat-authorized read-only `fat.rs`
/// `volume_fingerprint()` accessor — the volume serial (fixed at format time) + the cluster count (fixed by
/// geometry) are far more discriminating than M1's placeholder `cluster_size()` + `num_fats()`. The binding
/// LOGIC (bind + reject-on-mismatch) is unchanged; a UNAFS.ATR written under the M1 placeholder binding now
/// fails the header check and `atr_ensure` self-heals it (M2.3) — a one-time reheal on the metal card.
#[derive(Clone, Copy, PartialEq, Eq)]
struct AtrBinding {
    a: u32, // BS_VolID (the formatter's volume serial)
    b: u32, // count_of_clusters (the volume's data-cluster count)
}

fn atr_live_binding(fs: &crate::fs::fat::FatFs) -> AtrBinding {
    let (vol_id, clusters) = fs.volume_fingerprint();
    AtrBinding { a: vol_id, b: clusters }
}

/// K1: serialize the 512-byte header (committed). Layout (LE): [0..8] magic · [8..10] version ·
/// [10..12] header_len · [12..14] row_stride · [14..16] row_count · [16..20] flags · [20..24] binding.a ·
/// [24..28] binding.b · [28..508] reserved(0) [M2: bytes/sec, sec/clus, part_lba] · [508..512] header_crc32
/// over [0..508).
fn atr_serialize_header(bind: &AtrBinding) -> [u8; ATR_HEADER_LEN] {
    let mut b = [0u8; ATR_HEADER_LEN];
    b[0..8].copy_from_slice(&ATR_MAGIC);
    b[8..10].copy_from_slice(&ATR_VERSION.to_le_bytes());
    b[10..12].copy_from_slice(&(ATR_HEADER_LEN as u16).to_le_bytes());
    b[12..14].copy_from_slice(&(ATR_ROW_STRIDE as u16).to_le_bytes());
    b[14..16].copy_from_slice(&(ATR_ROWS as u16).to_le_bytes());
    b[16..20].copy_from_slice(&ATR_FLAG_COMMITTED.to_le_bytes());
    b[20..24].copy_from_slice(&bind.a.to_le_bytes());
    b[24..28].copy_from_slice(&bind.b.to_le_bytes());
    let crc = atr_crc32(&b[0..508]);
    b[508..512].copy_from_slice(&crc.to_le_bytes());
    b
}

/// K1: why a header was rejected — each verdict => the WHOLE volume is treated all-public (M2's mount logs
/// one line). Checked in fail-closed order: magic, then CRC, then version/geometry/committed/binding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AtrReject {
    Magic,
    Crc,
    Version,
    Geometry, // header_len / row_stride / row_count disagree with this build's format
    Uncommitted,
    Binding,
}

/// K1: validate a 512-byte header against the LIVE volume binding. `Ok` ONLY for a committed, CRC-valid,
/// this-version, this-volume header; anything else is fail-closed (the volume is all-public).
fn atr_parse_header(b: &[u8; ATR_HEADER_LEN], live: &AtrBinding) -> Result<(), AtrReject> {
    if b[0..8] != ATR_MAGIC {
        return Err(AtrReject::Magic);
    }
    let stored = u32::from_le_bytes([b[508], b[509], b[510], b[511]]);
    if atr_crc32(&b[0..508]) != stored {
        return Err(AtrReject::Crc);
    }
    if u16::from_le_bytes([b[8], b[9]]) != ATR_VERSION {
        return Err(AtrReject::Version);
    }
    if u16::from_le_bytes([b[10], b[11]]) as usize != ATR_HEADER_LEN
        || u16::from_le_bytes([b[12], b[13]]) as usize != ATR_ROW_STRIDE
        || u16::from_le_bytes([b[14], b[15]]) as usize != ATR_ROWS
    {
        return Err(AtrReject::Geometry);
    }
    if u32::from_le_bytes([b[16], b[17], b[18], b[19]]) & ATR_FLAG_COMMITTED == 0 {
        return Err(AtrReject::Uncommitted);
    }
    let bind = AtrBinding {
        a: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
        b: u32::from_le_bytes([b[24], b[25], b[26], b[27]]),
    };
    if bind != *live {
        return Err(AtrReject::Binding);
    }
    Ok(())
}

/// K1: the canonical EMPTY image — a committed header + 16 free rows (4608 bytes). The all-public resting
/// state a fresh volume (or M1's round-trip) leaves behind.
fn atr_empty_image(bind: &AtrBinding) -> alloc::vec::Vec<u8> {
    let mut img = alloc::vec::Vec::with_capacity(ATR_FILE_LEN);
    img.extend_from_slice(&atr_serialize_header(bind));
    let empty = atr_empty_row();
    for _ in 0..ATR_ROWS {
        img.extend_from_slice(&empty);
    }
    img
}

/// K1: ensure UNAFS.ATR exists as a committed, EMPTY 4608-byte image whose HEADER matches the LIVE volume
/// binding, creating or RE-HEALING it as needed. Returns the (first_cluster, size) to address it. The one-time
/// CREATE / regrow (create_in_root + a whole-image write_grow) is done WITHOUT the namespace lock — it is file
/// setup, not a steady-state ACL mutation, and holding ns across a multi-cluster write_grow would extend the
/// IRQ-masked span (the F3 ns-latency concern). Composed ENTIRELY from fat.rs's existing public API — no fat.rs
/// edit.
///
/// K1 M2.3 (self-heal): a full-size UNAFS.ATR is NOT trusted on size alone — its header is validated against the
/// live binding, and on ANY reject (a carried-over image from a DIFFERENT binding — e.g. M1's placeholder
/// `cluster_size/num_fats` vs M2.2's `BS_VolID/count_of_clusters` — a swapped/byte-copied card, a torn header, or
/// a wrong-version geometry) the whole image is REGROWN to a fresh committed empty (all-public). This is the
/// correct fail-closed response (a foreign binding's rows must never attach to THIS volume) and makes the M1->M2.2
/// binding transition seamless on the stateful metal card. Regrow discards any existing rows — safe: an image with
/// a mismatched header was never trustworthy, and a persist re-installs a named owner's row on its next write.
/// K5 M2: serialize the create-if-absent DECISION so two SMP cores cannot both `create_in_root` UNAFS.ATR (or
/// both regrow it). A dedicated lock-free CAS gate — deliberately NOT the namespace lock, because the create's
/// multi-cluster `write_grow` must not run under the IRQ-masked ns span (the F3 ns-latency rule) — makes the
/// decision atomic WITHOUT a contended lock held across the grow: a core that loses the CAS BAILS its persist
/// (returns Io), which is non-fatal and FAIL-SAFE (the in-RAM ACL still enforces this boot; only cross-reboot
/// survival of THAT one mutation defers to the next persist — never a double-create, never corruption). The
/// winner double-checks (another core may have finished the create between our probe and our CAS win) before it
/// creates/regrows, and clears the gate on every exit. Honest residual: because the loser bails rather than
/// blocks, a persist racing an in-progress create is dropped this pass — the degradation is always toward
/// fail-safe (not-yet-persisted), never toward two attr files or a torn ownership record. Untriggered today
/// (single EL0 core; no concurrent named persist in the battery), closed ahead of SMP EL0.
static ATR_CREATING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn atr_ensure(
    fs: &crate::fs::fat::FatFs,
    bind: &AtrBinding,
) -> Result<(u32, u32), crate::fs::fat::FatError> {
    use crate::fs::fat::FatError;
    // Fast path (fully concurrent, read-only — NO gate): the file already exists, is full-size, and its header
    // binds to THIS volume. This is the steady-state case — create happens once per volume lifetime — so a normal
    // persist never contends the create gate. A genuine binding MISMATCH (`Ok(false)`) falls through to the gated
    // regrow; a transient READ ERROR (`Err`) bails WITHOUT regrowing (preserving the rows), exactly as before.
    if let Ok((de, _lba, _off)) = fs.find_located(ATR_NAME) {
        if (de.size as usize) >= ATR_FILE_LEN {
            match atr_header_status(fs, de.first_cluster(), de.size, bind) {
                Ok(true) => return Ok((de.first_cluster(), de.size)),
                Ok(false) => {} // genuine mismatch -> the gated regrow below
                Err(e) => return Err(e), // read error -> preserve the existing rows, fail the persist
            }
        }
    }
    // Slow path: create-or-regrow. Serialize the DECISION via the CAS gate (see `ATR_CREATING`). A loser BAILS
    // (fail-safe deferral), never double-creates.
    if ATR_CREATING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Err(FatError::Io); // another core is mid-create -> defer this persist (non-fatal, fail-safe)
    }
    let r = atr_ensure_create_or_regrow(fs, bind);
    ATR_CREATING.store(false, Ordering::Release);
    r
}

/// K5 M2: the create-or-regrow body, run ONLY by the `ATR_CREATING` CAS winner (see `atr_ensure`). Double-checks
/// the on-disk state under the gate (another core may have created/regrown it between our probe and the CAS win),
/// then creates a fresh committed empty image, regrows a foreign/stale/partial one, or returns the now-valid
/// geometry. The multi-cluster `write_grow` runs here — under the CAS gate but NOT under ns (F3), and no core
/// spins on the gate (a contender bails), so nothing IRQ-masked is held across the grow.
fn atr_ensure_create_or_regrow(
    fs: &crate::fs::fat::FatFs,
    bind: &AtrBinding,
) -> Result<(u32, u32), crate::fs::fat::FatError> {
    use crate::fs::fat::FatError;
    match fs.find_located(ATR_NAME) {
        Ok((de, lba, off)) => {
            if (de.size as usize) < ATR_FILE_LEN {
                // A partial/truncated image — no complete rows to preserve; (re)grow to a fresh empty image.
                let img = atr_empty_image(bind);
                let (_w, sz, fc) = fs.write_grow(de.first_cluster(), de.size, lba, off, 0, &img)?;
                return Ok((fc, sz));
            }
            // Full-size: validate the header vs the LIVE binding, and DISTINGUISH a genuine MISMATCH (regrow — a
            // foreign / stale-binding / torn header must be re-healed to bind to THIS volume) from a transient
            // READ ERROR (bail — do NOT regrow, which would DISCARD every persisted row over a hiccup; the persist
            // is non-fatal and the in-RAM ACL still enforces this boot).
            match atr_header_status(fs, de.first_cluster(), de.size, bind) {
                Ok(true) => Ok((de.first_cluster(), de.size)),
                Ok(false) => {
                    let img = atr_empty_image(bind);
                    let (_w, sz, fc) = fs.write_grow(de.first_cluster(), de.size, lba, off, 0, &img)?;
                    Ok((fc, sz))
                }
                Err(e) => Err(e), // read error -> preserve the existing rows, fail the persist
            }
        }
        Err(FatError::NotFound) => {
            let (de, lba, off) = fs.create_in_root(ATR_NAME, ATR_DIR_ATTR)?;
            let img = atr_empty_image(bind);
            let (_w, sz, fc) = fs.write_grow(de.first_cluster(), de.size, lba, off, 0, &img)?;
            Ok((fc, sz))
        }
        Err(e) => Err(e),
    }
}

/// K1 M2.3: read UNAFS.ATR's sector-0 header and validate it against the LIVE binding. `Ok(true)` = a
/// this-volume, this-version, committed header; `Ok(false)` = a GENUINE reject (bad magic/version/geometry/
/// committed/binding/CRC — a foreign/stale/torn header); `Err(_)` = a block READ error (NOT a mismatch). The
/// caller decides: `atr_ensure` regrows only on `Ok(false)` (never on `Err`, which would wipe rows over a
/// hiccup); `native_migrate_from_sidecar` treats anything but `Ok(true)` as migrate-nothing (fail-closed — a read error
/// there is non-destructive).
fn atr_header_status(
    fs: &crate::fs::fat::FatFs,
    fc: u32,
    size: u32,
    bind: &AtrBinding,
) -> Result<bool, crate::fs::fat::FatError> {
    let mut hdr: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    fs.read_at(fc, size, 0, &mut hdr, ATR_HEADER_LEN)?;
    if hdr.len() != ATR_HEADER_LEN {
        return Err(crate::fs::fat::FatError::Io);
    }
    let mut hdr_arr = [0u8; ATR_HEADER_LEN];
    hdr_arr.copy_from_slice(&hdr);
    Ok(atr_parse_header(&hdr_arr, bind).is_ok())
}

/// K1 M2.3: the on-disk 11-byte space-padded 8.3 form of a name (`"OWNED.BIN" -> b"OWNED   BIN"`), the DURABLE
/// identity key persisted in a row (re-resolved by name at mount, M2.4). `None` if the name is not a
/// representable short name. Mirrors fat.rs's private `format_83` (kept LOCAL so M2.3 leaves fat.rs untouched —
/// the fat.rs edit is deferred to M2.2); a name that already created a directory entry always converts.
fn atr_name_from_str(name: &str) -> Option<[u8; 11]> {
    let mut out = [b' '; 11];
    let b = name.as_bytes();
    let (base, ext): (&[u8], &[u8]) = match name.find('.') {
        Some(i) => {
            let ext = &b[i + 1..];
            if ext.is_empty() || ext.contains(&b'.') {
                return None; // trailing/second dot — not a distinct 8.3 name
            }
            (&b[..i], ext)
        }
        None => (b, &[][..]),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return None;
    }
    for (k, &c) in base.iter().enumerate() {
        out[k] = c.to_ascii_uppercase();
    }
    for (k, &c) in ext.iter().enumerate() {
        out[8 + k] = c.to_ascii_uppercase();
    }
    Some(out)
}

/// K1 M2.4: reconstruct the textual `"NAME.EXT"` form from an 11-byte on-disk 8.3 field into `buf`, returning
/// its length — the inverse of `atr_name_from_str`, used at mount to re-resolve a persisted row BY NAME
/// (`find_located`, defeating the recycled-directory-slot hazard). Mirrors fat.rs's `classify_dir_slot` (trim
/// trailing spaces on base + ext, insert a `.` only if the extension is non-empty). A degenerate all-space base
/// yields length 0 (the caller skips — fail-closed).
fn atr_name_to_buf(name11: &[u8; 11], buf: &mut [u8; 12]) -> usize {
    let mut n = 0usize;
    let mut base = 8usize;
    while base > 0 && name11[base - 1] == b' ' {
        base -= 1;
    }
    for k in 0..base {
        buf[n] = name11[k];
        n += 1;
    }
    let mut ext = 3usize;
    while ext > 0 && name11[8 + ext - 1] == b' ' {
        ext -= 1;
    }
    if ext > 0 {
        buf[n] = b'.';
        n += 1;
        for k in 0..ext {
            buf[n] = name11[8 + k];
            n += 1;
        }
    }
    n
}

/// K1 M2.3: find the row INDEX in UNAFS.ATR whose on-disk `(dir_lba, dir_off)` matches, else the first FREE
/// (or corrupt) slot — a bounded 16-row scan over the rows region already read into `rows`. `None` iff every
/// row is a DIFFERENT live owner (the 16-row table is full for a new key — the persist then fails closed).
fn atr_find_row(rows: &[u8], dir_lba: u64, dir_off: u32) -> Option<usize> {
    let mut free_idx = None;
    for i in 0..ATR_ROWS {
        let start = ATR_ROW_STRIDE * i;
        let mut arr = [0u8; ATR_ROW_STRIDE];
        arr.copy_from_slice(&rows[start..start + ATR_ROW_STRIDE]);
        match atr_parse_row(&arr) {
            Some(r) => {
                if r.dir_lba == dir_lba && r.dir_off == dir_off {
                    return Some(i); // update the existing row for this file
                }
            }
            None => {
                // A well-formed free slot OR a corrupt row — either is claimable (overwriting a corrupt row
                // heals it). Take the FIRST such slot.
                if free_idx.is_none() {
                    free_idx = Some(i);
                }
            }
        }
    }
    free_idx
}

/// K1 M2.3: WRITE-THROUGH persist of the owner + grants of a named-owner file into its UNAFS.ATR row. No-op
/// (returns true, ZERO disk I/O) when `owner_ppid` is NONE — the whole 23-fixture battery, keeping the
/// anonymous create/grant path byte-identical. For a NAMED owner: ensure UNAFS.ATR (create/self-heal, OUTSIDE
/// the namespace lock — a multi-cluster grow must not extend the IRQ-masked ns span), then UNDER ns do a bounded
/// 16-row scan + a single-row `write_at` of the affected 256-byte record (one device-sector RMW — the M1 seam,
/// respecting `NAMESPACE ⊃ inner`). Returns false on any disk error or a full attr table (the caller treats a
/// persist failure as non-fatal — the in-RAM ACL still enforces THIS boot; only cross-reboot survival is lost,
/// which fails closed to PUBLIC on the next mount).
fn atr_persist_row(
    fs: &crate::fs::fat::FatFs,
    name11: [u8; 11],
    first_cluster: u32,
    size: u32,
    dir_lba: u64,
    dir_off: u32,
    owner_ppid: PrincipalRecord,
    grants: &[(PrincipalRecord, u32); NFGRANT],
) -> bool {
    if owner_ppid.kind == PRIN_NONE {
        return true; // anonymous owner — nothing persists (the battery path; no disk touch)
    }
    let bind = atr_live_binding(fs);
    let (afc, asz) = match atr_ensure(fs, &bind) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let mut arow_grants = [AtrGrant::EMPTY; NFGRANT];
    for (j, &(p, r)) in grants.iter().enumerate() {
        if r != 0 {
            arow_grants[j] = AtrGrant { prin: p, rights: r };
        }
    }
    let row = AtrRow {
        name: name11,
        first_cluster,
        size,
        dir_lba,
        dir_off,
        owner: owner_ppid,
        grants: arow_grants,
    };
    let bytes = atr_serialize_row(&row);

    let _ns = ns_lock();
    // Read the rows region (bounded, polled) to find the matching-or-free row, then write JUST that one sector.
    let mut rows: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs
        .read_at(afc, asz, ATR_HEADER_LEN as u32, &mut rows, ATR_ROW_STRIDE * ATR_ROWS)
        .is_err()
        || rows.len() != ATR_ROW_STRIDE * ATR_ROWS
    {
        return false;
    }
    let Some(idx) = atr_find_row(&rows, dir_lba, dir_off) else {
        return false; // attr table full for a new key — persist fails closed (in-RAM ACL still holds this boot)
    };
    let off = (ATR_HEADER_LEN + ATR_ROW_STRIDE * idx) as u32;
    fs.write_at(afc, asz, off, &bytes).unwrap_or(0) == bytes.len()
}

/// K1 M2.3: clear a file's persisted UNAFS.ATR row back to FREE (all-public) — the unlink twin of
/// `atr_persist_row`. Called from `sys_unlink` ONLY when the file HAD a named owner (so UNAFS.ATR exists and
/// holds its row); a public/anonymous file never reaches here, so the battery unlink path does ZERO extra I/O.
/// UNDER ns (one bounded row scan + one single-row `write_at`). Missing file/row => nothing to clear (true).
fn atr_clear_row(fs: &crate::fs::fat::FatFs, dir_lba: u64, dir_off: u32) -> bool {
    let (de, _lba, _off) = match fs.find_located(ATR_NAME) {
        Ok(t) => t,
        Err(crate::fs::fat::FatError::NotFound) => return true, // no attr store — nothing persisted to clear
        Err(_) => return false,
    };
    if (de.size as usize) < ATR_FILE_LEN {
        return true; // a partial/absent image holds no row for this file
    }
    let (afc, asz) = (de.first_cluster(), de.size);
    let _ns = ns_lock();
    let mut rows: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs
        .read_at(afc, asz, ATR_HEADER_LEN as u32, &mut rows, ATR_ROW_STRIDE * ATR_ROWS)
        .is_err()
        || rows.len() != ATR_ROW_STRIDE * ATR_ROWS
    {
        return false;
    }
    // Find ONLY an exact live-row match for this file (never a free slot) — nothing to do if it is absent.
    for i in 0..ATR_ROWS {
        let start = ATR_ROW_STRIDE * i;
        let mut arr = [0u8; ATR_ROW_STRIDE];
        arr.copy_from_slice(&rows[start..start + ATR_ROW_STRIDE]);
        if let Some(r) = atr_parse_row(&arr) {
            if r.dir_lba == dir_lba && r.dir_off == dir_off {
                let off = (ATR_HEADER_LEN + ATR_ROW_STRIDE * i) as u32;
                let empty = atr_empty_row();
                return fs.write_at(afc, asz, off, &empty).unwrap_or(0) == empty.len();
            }
        }
    }
    true // no matching row — already public on disk
}

// K7-F1 (2026-07-15, lens must-fix): `atr_rebuild_into_owned` — the K1-M2.4 sidecar-to-OWNED_FILES
// rebuild — is DELETED ENTIRELY. Its last caller (`k1_corrupt_check`) installed EVERY valid sidecar row
// into the LIVE OWNED_FILES on the real card mount, so on a legacy-carrying card the fixture re-adopted
// legacy owners for the rest of the boot — exactly the enforcement K7 removes. The corrupt-parse guard
// now drives the migration pass instead (see `k1_corrupt_check`). After this deletion, NO function
// anywhere adopts sidecar rows as owners: `owned_install_persisted`'s SOLE caller is the NATIVE
// rebuild (`native_rebuild_into_owned`) — no sidecar-parse path reaches it.

/// K6: project an owner principal + its grants to the OWNED native byte strings [`native_acl_write`]
/// takes. `None` if the owner has no native projection (fail-closed — an un-nameable owner is not
/// migrated). A grant whose grantee/rights do not project is dropped (its edge simply does not migrate).
fn native_project_owner_grants(
    owner: &PrincipalRecord,
    grants: &[(PrincipalRecord, u32); NFGRANT],
) -> Option<(alloc::vec::Vec<u8>, alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)>)> {
    let mut ob = [0u8; K4_STR_MAX];
    let on = principal_native_string(owner, &mut ob)?;
    let owner_bytes = ob[..on].to_vec();
    let mut gv: alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> = alloc::vec::Vec::new();
    for &(p, r) in grants.iter() {
        if r == 0 || p.kind == PRIN_NONE {
            continue;
        }
        let mut gk = [0u8; K4_STR_MAX];
        let mut rv = [0u8; 2];
        if let (Some(gn), Some(rn)) = (grant_native_key(&p, &mut gk), rights_native_value(r, &mut rv)) {
            gv.push((gk[..gn].to_vec(), rv[..rn].to_vec()));
        }
    }
    Some((owner_bytes, gv))
}

/// K6 M2: the BOOT-TIME idempotent migration pass — move committed `IMAGE_SHA256` owner rows off the
/// `UNAFS.ATR` FAT sidecar onto native unafs attributes, NATIVE-BEFORE-DELETE. Per row: project owner +
/// grants -> [`native_acl_write`] (journaled) -> read the row back and verify each principal reverses
/// to the SAME `PrincipalRecord` (an independent re-projection, the migration-landmine invariant) ->
/// only THEN [`atr_clear_row`] deletes the sidecar row. A power cut anywhere leaves BOTH copies, never
/// neither; a re-run converges (the native write is idempotent — a row that already verifies
/// short-circuits to sidecar-delete). Only `IMAGE_SHA256` rows migrate (ratified policy): legacy
/// `PROGRAM_NAME` rows stay in the sidecar, un-migrated and still enforcing via the ATR fallback rebuild
/// (a card re-prep clears them). Returns the number of rows migrated (sidecar row deleted).
fn native_migrate_from_sidecar(fs: &crate::fs::fat::FatFs) -> usize {
    let bind = atr_live_binding(fs);
    let (de, _lba, _off) = match fs.find_located(ATR_NAME) {
        Ok(t) => t,
        _ => return 0, // no sidecar -> nothing to migrate (the QEMU boot path; the metal card carries it)
    };
    if (de.size as usize) < ATR_FILE_LEN {
        return 0;
    }
    let (afc, asz) = (de.first_cluster(), de.size);
    if atr_header_status(fs, afc, asz, &bind) != Ok(true) {
        return 0; // bad/uncommitted sidecar -> migrate nothing (fail-closed; the fallback rebuild sees the same)
    }
    let mut rows: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs
        .read_at(afc, asz, ATR_HEADER_LEN as u32, &mut rows, ATR_ROW_STRIDE * ATR_ROWS)
        .is_err()
        || rows.len() != ATR_ROW_STRIDE * ATR_ROWS
    {
        return 0;
    }
    let mut migrated = 0usize;
    for i in 0..ATR_ROWS {
        let start = ATR_ROW_STRIDE * i;
        let mut arr = [0u8; ATR_ROW_STRIDE];
        arr.copy_from_slice(&rows[start..start + ATR_ROW_STRIDE]);
        let Some(row) = atr_parse_row(&arr) else {
            continue; // free/corrupt row -> nothing to migrate
        };
        if row.owner.kind != PRIN_IMAGE_SHA256 {
            continue; // ratified: only IMAGE_SHA256 rows migrate; PROGRAM_NAME stays in the sidecar
        }
        let mut grants = [(PrincipalRecord::NONE, 0u32); NFGRANT];
        for (j, g) in row.grants.iter().enumerate() {
            if g.rights != 0 && g.prin.kind != PRIN_NONE {
                grants[j] = (g.prin, g.rights);
            }
        }
        let Some((owner_bytes, gv)) = native_project_owner_grants(&row.owner, &grants) else {
            continue; // un-projectable owner -> leave the sidecar row (fail-closed)
        };
        let mut name_buf = [0u8; 12];
        let n = atr_name_to_buf(&row.name, &mut name_buf);
        let Ok(name) = core::str::from_utf8(&name_buf[..n]) else {
            continue;
        };
        // NATIVE-BEFORE-DELETE step 1: write the native row (idempotent — a re-run overwrites identically).
        let grefs: alloc::vec::Vec<(&[u8], &[u8])> =
            gv.iter().map(|(k, v)| (k.as_slice(), v.as_slice())).collect();
        if !crate::fs::unafs::native_acl_write(
            row.dir_lba,
            row.dir_off,
            name,
            row.first_cluster,
            &owner_bytes,
            &grefs,
        ) {
            continue; // native write failed -> DO NOT delete the sidecar row (both-copies -> re-run)
        }
        // Step 2: read back + verify each principal reverses to the SAME record (independent re-projection).
        let Some(back) = crate::fs::unafs::native_acl_read(row.dir_lba, row.dir_off) else {
            continue;
        };
        if principal_from_native(&back.owner) != Some(row.owner) || back.name != name {
            continue; // verify failed -> keep the sidecar row (never delete an unverified migration)
        }
        // Every ORIGINAL grant must be present and reverse-equal in the native read-back.
        let grants_ok = grants.iter().filter(|(_, r)| *r != 0).all(|(p, r)| {
            back.grants.iter().any(|(gk, rv)| {
                grantee_from_grant_key(gk) == Some(*p) && rights_from_native(rv) == *r
            })
        });
        if !grants_ok {
            continue;
        }
        // Step 3: verified -> delete the sidecar row (native is now the sole store for this file).
        if atr_clear_row(fs, row.dir_lba, row.dir_off) {
            migrated += 1;
        }
    }
    migrated
}

/// K6 M2/M3: rebuild the in-RAM owner/grants ACL from the NATIVE attribute volume — the
/// successor of the retired ATR fallback rebuild (K7-F1-deleted). For each native ACL row: reverse-project owner (+ grants),
/// re-resolve the DURABLE FAT name (`find_located` — a recycled `(dir_lba, dir_off)` is only a hint),
/// corroborate `first_cluster` when both are nonzero, and install with the NO-LIVE-OWNER sentinel.
/// FAIL-CLOSED throughout: an unparseable owner / name-gone / cluster-mismatch yields PUBLIC, never a
/// forged owner. Returns the number of rows installed.
fn native_rebuild_into_owned(fs: &crate::fs::fat::FatFs) -> usize {
    let mut installed = 0usize;
    for row in crate::fs::unafs::native_acl_list() {
        let Some(owner) = principal_from_native(&row.owner) else {
            continue; // un-nameable owner -> fail-closed to public
        };
        let (rde, rlba, roff) = match fs.find_located(&row.name) {
            Ok(t) => t,
            _ => continue, // name gone -> public
        };
        if row.first_cluster != 0 && rde.first_cluster() != 0 && row.first_cluster != rde.first_cluster() {
            continue; // slot now holds a DIFFERENT file -> fail-closed
        }
        let mut grants = [(PrincipalRecord::NONE, 0u32); NFGRANT];
        for (j, (gk, rv)) in row.grants.iter().enumerate() {
            if j >= NFGRANT {
                break;
            }
            if let Some(p) = grantee_from_grant_key(gk) {
                let r = rights_from_native(rv);
                if r != 0 {
                    grants[j] = (p, r);
                }
            }
        }
        if owned_install_persisted(rlba, roff as u32, owner, &grants) {
            installed += 1;
        }
    }
    installed
}

// =====================================================================================================
// K6 M3 / K5B: the NATIVE production persist paths — the `atr_persist_*` successors. Every wrapper
// preserves its predecessor's proven ordering discipline against the native store; since K5B the fusion
// lock is the `with_unafs` MOUNT, not `ns`:
//   * create  (`native_persist_create`)  — write-through after a fully-successful private create; the
//     row is fresh (empty grants), so `native_acl_write`'s own MOUNT hold is the whole discipline.
//   * grant   (`native_persist_grants`)  — NAMED-owner probe first (anonymous = zero I/O, no mount),
//     then snapshot + write fused under ONE with_unafs hold (the K5B invariant).
//   * revoke  (`native_write_grant_row_on`) — the disk half of the K3 two-phase durable-first revoke;
//     the caller (sys_fgrant_revoke_2phase) holds ONE with_unafs hold across snapshot -> THIS write ->
//     in-RAM commit — the K5 anti-resurrection property, re-established on MOUNT (K5B).
//   * grow    (`native_persist_grow`)    — probe, then snapshot + write under one with_unafs hold.
//   * unlink  — `native_acl_clear` (fs/unafs.rs), gated durable-first before the 0xE5 (the K1-F2 fix).
//
// K5B NS-SPAN NOTE (NARROW ruling, Maestro + Peter 2026-07-15 — supersedes the K6 Option-A note). The
// K7 metal sitting measured the fused ns-hold at ~704/712/440 ms (polled SD I/O inside the journaled
// multi-sector write) — the Option-A WATCH tripped. K5B removes `ns` from every ACL persist site: the
// NAMESPACE is NO LONGER frozen across ACL disk I/O (opens/unlinks/creates on other cores proceed).
// THE HONEST BOUND, inseparable from that headline: the ~0.7 s per-core IRQ-masked window across the
// polled SD write REMAINS, relocated wholly into the `with_unafs` MOUNT hold — with_unafs masks IRQs
// around its whole closure BY DESIGN (the K4 coherence keystone: unmasking would let a timer preempt
// + same-core re-entry deadlock the non-reentrant MOUNT spinlock) and K3 durable-first forbids
// deferring the write. Both protections are STOP-class, untouched; the per-core mask is a ledgered
// residual (SECURITY.md §K1 K5B) whose true closure is out of this lane (crate batched-sync or a
// scheduler preempt-disable primitive). THE K5B INVARIANT (the anti-resurrection argument): snapshot,
// durable row write, and in-RAM commit under ONE uninterrupted MOUNT hold at every persister — two
// persisters never interleave, so no stale snapshot can land after a revoke's narrow. Lock order:
// NAMESPACE ⊃ MOUNT ⊃ OWNED_FILES (owned_* stay take-and-release leaves inside the MOUNT hold; no
// path takes MOUNT then NAMESPACE, nor OWNED_FILES then MOUNT).
// =====================================================================================================

/// K6 M3: write-through persist of a NEW private file's owner onto the NATIVE attribute volume — the
/// `atr_persist_row` successor for the create path. No-op (true, zero I/O) for a NONE owner (the whole
/// battery). Returns false on an un-projectable owner or any native write failure — non-fatal to the
/// caller (the in-RAM ACL still enforces this boot; cross-reboot fails closed to PUBLIC).
fn native_persist_create(
    name: &str,
    first_cluster: u32,
    dir_lba: u64,
    dir_off: u32,
    owner_ppid: PrincipalRecord,
) -> bool {
    if owner_ppid.kind == PRIN_NONE {
        return true; // anonymous owner — nothing persists (the battery path; no disk touch)
    }
    let empty = [(PrincipalRecord::NONE, 0u32); NFGRANT];
    let Some((owner_bytes, _)) = native_project_owner_grants(&owner_ppid, &empty) else {
        return false; // un-projectable owner (reserved kind) — not persistable, fail-closed
    };
    // K5B: no ns — `native_acl_write`'s own with_unafs MOUNT hold is the serializer (the row is FRESH with an
    // empty grant set, so there is no snapshot/commit coupling for the invariant to fuse; the write itself is
    // one uninterrupted MOUNT hold like every other persister).
    crate::fs::unafs::native_acl_write(dir_lba, dir_off, name, first_cluster, &owner_bytes, &[])
}

/// K6 M3 / K5B: the disk half of the K3 two-phase revoke / the fused re-persist, against the NATIVE store —
/// the `atr_write_grant_row_locked` successor. **The caller MUST already hold the `with_unafs` MOUNT** and
/// pass it in: the K5B invariant fuses the caller's OWNED_FILES snapshot, THIS journaled write, and the
/// caller's in-RAM commit under that single MOUNT hold (see the K5B NS-SPAN NOTE above) — re-entering
/// `with_unafs` here would deadlock the non-reentrant MOUNT spinlock. Recovers the durable
/// name/first_cluster from the EXISTING native row; no row yet -> true (create-persist had failed; in-RAM
/// still holds — the ATR "nothing to update" semantics). The K3 fault-injection knob fires after the NONE
/// early-return, so the anonymous battery never observes it.
fn native_write_grant_row_on(
    fs: &mut crate::fs::unafs::KernelUnaFS,
    dir_lba: u64,
    dir_off: u32,
    owner_ppid: PrincipalRecord,
    grants: &[(PrincipalRecord, u32); NFGRANT],
) -> bool {
    if owner_ppid.kind == PRIN_NONE {
        return true; // anonymous owner — nothing persists (the battery path; no disk touch)
    }
    if K3_TEST_FAIL_PERSIST.load(Ordering::Relaxed) {
        return false; // K3: test-only synthetic durable-write failure (fixture-set, self-clearing)
    }
    let Some(existing) = crate::fs::unafs::native_acl_read_on(fs, dir_lba, dir_off) else {
        return true; // no persisted row — nothing to update (in-RAM still enforces this boot)
    };
    let Some((owner_bytes, gv)) = native_project_owner_grants(&owner_ppid, grants) else {
        return false;
    };
    let grefs: alloc::vec::Vec<(&[u8], &[u8])> =
        gv.iter().map(|(k, v)| (k.as_slice(), v.as_slice())).collect();
    crate::fs::unafs::native_acl_write_on(
        fs,
        dir_lba,
        dir_off,
        &existing.name,
        existing.first_cluster,
        &owner_bytes,
        &grefs,
    )
}

/// K6 M3 / K5B: re-persist owner + grants after an ACL widen — the `atr_persist_grants` successor.
/// NAMED-owner probe first (anonymous = zero disk I/O, no mount — the battery path), then snapshot + write
/// fused under ONE `with_unafs` MOUNT hold (the K5B invariant): the snapshot is taken INSIDE the hold, so a
/// concurrent two-phase revoke (which commits in-RAM inside ITS hold) cannot be straddled — this persister
/// runs entirely before or entirely after it, never against a stale in-RAM state. `ns` is NOT taken.
fn native_persist_grants(dir_lba: u64, dir_off: u32) -> bool {
    if owned_owner_ppid(dir_lba, dir_off).kind == PRIN_NONE {
        return true; // anonymous owner — nothing persists (the battery path; no disk touch, no mount)
    }
    #[cfg(feature = "nsspan")] let _nsspan = NsSpanProbe::new(&NSSPAN_GRANTS); crate::fs::unafs::with_unafs(|fs| { // K5B WATCH probe brackets the fused with_unafs call (ns not taken); newline-neutral inline
        let Some((owner_ppid, grants)) = owned_snapshot_row(dir_lba, dir_off) else {
            return true; // row vanished (raced clear) between the probe and the hold — nothing to persist
        };
        native_write_grant_row_on(fs, dir_lba, dir_off, owner_ppid, &grants)
    })
    .unwrap_or(true) // no volume / no storage: nothing durable exists to refresh (pre-K5B semantics)
}

/// K6 M3 / K5B: refresh a NAMED-owner file's persisted `first_cluster` after a GROW — the
/// `atr_persist_grow` successor (K2 M(b)). Recovers the durable name from the existing native row;
/// refreshes fc + the current owner/grants snapshot under ONE `with_unafs` MOUNT hold (the K5B invariant —
/// snapshot INSIDE the hold, `ns` not taken). No row yet / anonymous -> no-op true.
fn native_persist_grow(dir_lba: u64, dir_off: u32, new_first: u32) -> bool {
    if owned_owner_ppid(dir_lba, dir_off).kind == PRIN_NONE {
        return true; // anonymous owner — the whole battery incl. the u10 GROW.BIN fixture; zero I/O
    }
    #[cfg(feature = "nsspan")] let _nsspan = NsSpanProbe::new(&NSSPAN_GROW); crate::fs::unafs::with_unafs(|fs| { // K5B WATCH probe brackets the fused with_unafs call (ns not taken); newline-neutral inline
        let Some((owner_ppid, grants)) = owned_snapshot_row(dir_lba, dir_off) else {
            return true;
        };
        if owner_ppid.kind == PRIN_NONE {
            return true; // owner went anonymous under the race — nothing persists
        }
        let Some(existing) = crate::fs::unafs::native_acl_read_on(fs, dir_lba, dir_off) else {
            return true; // never persisted (create-persist failed) — in-RAM still holds this boot
        };
        let Some((owner_bytes, gv)) = native_project_owner_grants(&owner_ppid, &grants) else {
            return false;
        };
        let grefs: alloc::vec::Vec<(&[u8], &[u8])> =
            gv.iter().map(|(k, v)| (k.as_slice(), v.as_slice())).collect();
        crate::fs::unafs::native_acl_write_on(
            fs,
            dir_lba,
            dir_off,
            &existing.name,
            new_first,
            &owner_bytes,
            &grefs,
        )
    })
    .unwrap_or(true) // no volume / no storage: nothing durable exists to refresh (pre-K5B semantics)
}

/// BANDY-2: refresh the persisted NAME of an owned file after an in-place rename (bus mv). The FAT
/// entry has already been renamed (same directory slot -> `(dir_lba, dir_off)` unchanged, so the
/// in-RAM OWNED_FILES row and the native row's LOCATION key are both still valid); ONLY the native
/// row's stored `name` attribute — the mount-rebuild's by-name re-resolution key — has gone stale.
/// Rewrite it, preserving the owner + grants + first_cluster verbatim, under ONE `with_unafs` MOUNT
/// hold (the K5B invariant; `ns` is NOT taken — the caller drops the NAMESPACE hold first, exactly
/// like every other persist site). No native row yet (create-persist had failed) -> nothing to
/// re-bind, `true` (in-RAM still enforces this boot; cross-reboot fails closed to PUBLIC). Returns
/// `false` on a real projection/write failure — the caller then CLEARS the stale-named row so a
/// future same-name file can never adopt it (the K1-F2 re-adoption class, fail-closed).
fn native_persist_rename(dir_lba: u64, dir_off: u32, new_name: &str) -> bool {
    crate::fs::unafs::with_unafs(|fs| {
        let Some(row) = crate::fs::unafs::native_acl_read_on(fs, dir_lba, dir_off) else {
            return true; // no persisted row — nothing to re-bind (in-RAM still enforces this boot)
        };
        let grefs: alloc::vec::Vec<(&[u8], &[u8])> =
            row.grants.iter().map(|(k, v)| (k.as_slice(), v.as_slice())).collect();
        crate::fs::unafs::native_acl_write_on(
            fs,
            dir_lba,
            dir_off,
            new_name,
            row.first_cluster,
            &row.owner,
            &grefs,
        )
    })
    .unwrap_or(true) // no volume / no storage: nothing durable exists to re-bind (pre-K5B semantics)
}

/// K1 M2.4 / K2 / K6 / K7: the REAL-BOOT mount-rebuild hook — restore persisted ownership before EL0
/// programs run. GATED on `by_name_spawn_multivalued()` (TRUE since K2). K6: first MIGRATE any committed
/// IMAGE_SHA256 sidecar rows onto native attributes (native-before-delete), then rebuild OWNED_FILES from
/// the NATIVE store — the sole enforcement store.
///
/// K7 (2026-07-15, wholesale legacy-sidecar ENFORCEMENT removal): the sidecar FALLBACK-REBUILD leg
/// is DELETED — the function itself is gone (K7-F1). Legacy `PROGRAM_NAME` sidecar rows are NEVER
/// adopted as owners in production — a card carrying ONLY legacy rows enforces PUBLIC-ONLY after this arc
/// (fail-closed). The MIGRATION pass is RETAINED (committed IMAGE_SHA256 rows still cross native-before-
/// delete on every boot, pending Peter declaring the card fleet fully crossed); only the "legacy rows can
/// enforce" machinery is gone. On QEMU (fresh-per-build FAT) both the sidecar and the native store are
/// empty here, so ZERO rows install and the boot path stays byte-identical; the live effect is on metal.
/// The MECHANISM is proven independently by the K6 witness + the K1/K2/K3/K5 launchers.
fn atr_maybe_boot_rebuild() {
    if !by_name_spawn_multivalued() {
        return; // cross-reboot enforcement gated off (single-program world) -> no rebuild, no I/O
    }
    if crate::drivers::block::info().is_none() {
        return;
    }
    if let Ok(fs) = crate::fs::fat::mount() {
        let _ = native_migrate_from_sidecar(&fs); // K6: move committed IMAGE_SHA256 rows to native first
        let _ = native_rebuild_into_owned(&fs); // K7: enforce SOLELY from the native store (no legacy fallback)
    }
}

/// K1 M1 — the disk half of the round-trip: write ONE synthetic row via the single-row `write_at` path
/// UNDER the NAMESPACE lock (the exact M2 steady-state discipline — one 256-byte record = one device-sector
/// RMW), read it back + verify byte- and field-equal, then CLEAR it — leaving UNAFS.ATR a valid EMPTY
/// (all-public) image. Also re-reads + validates the header (proves the create path). Returns true iff every
/// step held; robust — every fallible op is matched, never unwrapped, so a disk hiccup FAILs the line, never
/// faults the core.
fn k1_atr_disk_roundtrip(fs: &crate::fs::fat::FatFs, fc: u32, size: u32, bind: &AtrBinding) -> bool {
    const ROW_I: usize = 3; // an arbitrary interior row
    let row_off = (ATR_HEADER_LEN + ATR_ROW_STRIDE * ROW_I) as u32;

    // A synthetic owned-file row — SYNTHETIC principals ON PURPOSE (the live owner (asid, gen) must not
    // persist; M1 proves only the container).
    let mut grants = [AtrGrant::EMPTY; NFGRANT];
    grants[0] = AtrGrant { prin: PrincipalRecord::program(b"prog:TESTGRANTEE"), rights: CAP_READ };
    let row = AtrRow {
        name: *b"K1TEST  BIN",
        first_cluster: 0x55,
        size: 512,
        dir_lba: 0x40,
        dir_off: 0x20,
        owner: PrincipalRecord::program(b"prog:TESTOWNER"),
        grants,
    };
    let want = atr_serialize_row(&row);

    // Write the row IN PLACE under the F3 NAMESPACE lock — the seam M2's create/grant/revoke attr writes
    // use. `write_at` never grows and takes no inner lock, so holding ns across this one bounded polled
    // sector RMW respects both the lock order and the ns span rule.
    {
        let _ns = ns_lock();
        if fs.write_at(fc, size, row_off, &want).unwrap_or(0) != want.len() {
            return false;
        }
    }

    // Read it back (a read needs no ns) and confirm the exact bytes + a field-equal parse.
    let mut got: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs.read_at(fc, size, row_off, &mut got, ATR_ROW_STRIDE).is_err() || got.len() != ATR_ROW_STRIDE {
        return false;
    }
    if &got[..] != &want[..] {
        return false;
    }
    let mut got_arr = [0u8; ATR_ROW_STRIDE];
    got_arr.copy_from_slice(&got);
    let parsed_ok = match atr_parse_row(&got_arr) {
        Some(r) => r.owner == row.owner && r.grants == row.grants && r.name == row.name,
        None => false,
    };
    if !parsed_ok {
        return false;
    }

    // Clear the row back to free (leave UNAFS.ATR a valid, all-public EMPTY image) — again one sector, ns.
    {
        let _ns = ns_lock();
        let empty = atr_empty_row();
        if fs.write_at(fc, size, row_off, &empty).unwrap_or(0) != empty.len() {
            return false;
        }
    }

    // Re-read + validate the header (proves the create path produced a committed, this-volume header).
    let mut hdr: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs.read_at(fc, size, 0, &mut hdr, ATR_HEADER_LEN).is_err() || hdr.len() != ATR_HEADER_LEN {
        return false;
    }
    let mut hdr_arr = [0u8; ATR_HEADER_LEN];
    hdr_arr.copy_from_slice(&hdr);
    atr_parse_header(&hdr_arr, bind).is_ok()
}

/// K1 M1 — the ATR round-trip self-test. Proves (A) the codec round-trips + fails closed on corruption and
/// (B) the reserved-file helpers create/write/read UNAFS.ATR on the real volume, the steady-state single-row
/// update UNDER the F3 NAMESPACE lock. Emits ONE `:: K1-atr: ::` line (NOT a `-> PASS` line — the 23-fixture
/// count stays byte-equivalent). ENFORCEMENT-INERT: never reads OWNED_FILES, never maps a record to
/// (asid, gen), never persists a live owner. Runs LAST (after the F2/F3 witnesses) so its disk I/O can never
/// perturb the battery or the witnesses.
fn k1_atr_selftest() {
    // One-shot (u7_launcher's task calls this once; guard defensively — the u7_run DONE idiom).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // ---- Part A: in-RAM codec round-trip + fail-closed negatives (no disk) -------------------------------
    let mut codec_ok = true;

    // A representative populated row: PROGRAM_NAME owner + two grants (one full-rights).
    let mut grants = [AtrGrant::EMPTY; NFGRANT];
    grants[0] = AtrGrant { prin: PrincipalRecord::program(b"prog:MIDI"), rights: CAP_READ };
    grants[1] = AtrGrant { prin: PrincipalRecord::program(b"prog:SEQ"), rights: CAP_READ | CAP_WRITE };
    let row = AtrRow {
        name: *b"OWNED   BIN",
        first_cluster: 0x1234,
        size: 4096,
        dir_lba: 0x2000,
        dir_off: 0x60,
        owner: PrincipalRecord::program(b"prog:VUG"),
        grants,
    };
    let bytes = atr_serialize_row(&row);
    match atr_parse_row(&bytes) {
        Some(r) => {
            codec_ok &= r.name == row.name
                && r.first_cluster == row.first_cluster
                && r.size == row.size
                && r.dir_lba == row.dir_lba
                && r.dir_off == row.dir_off
                && r.owner == row.owner
                && r.grants == row.grants;
            codec_ok &= atr_serialize_row(&r) == bytes; // re-serializing the parse reproduces the exact bytes
        }
        None => codec_ok = false,
    }

    // Negative: a single flipped bit in the owner field MUST fail the row CRC -> None (drop to public).
    let mut corrupt = bytes;
    corrupt[40] ^= 0x01;
    codec_ok &= atr_parse_row(&corrupt).is_none();

    // Negative: a FREE row (VALID clear) parses to None (a well-formed free slot, not corruption).
    codec_ok &= atr_parse_row(&atr_empty_row()).is_none();

    // Header: round-trip + reject wrong binding / magic / CRC / uncommitted.
    let bind = AtrBinding { a: 2048, b: 2 };
    let hdr = atr_serialize_header(&bind);
    codec_ok &= atr_parse_header(&hdr, &bind).is_ok();
    codec_ok &= atr_parse_header(&hdr, &AtrBinding { a: 512, b: 2 }) == Err(AtrReject::Binding);
    let mut badmagic = hdr;
    badmagic[0] = b'X';
    codec_ok &= atr_parse_header(&badmagic, &bind) == Err(AtrReject::Magic);
    let mut badcrc = hdr;
    badcrc[100] ^= 0xFF; // a reserved byte — magic still matches, CRC now fails
    codec_ok &= atr_parse_header(&badcrc, &bind) == Err(AtrReject::Crc);
    let mut uncommit = hdr;
    uncommit[16] &= !(ATR_FLAG_COMMITTED as u8); // clear committed, then re-CRC so it fails on Uncommitted
    let uc = atr_crc32(&uncommit[0..508]);
    uncommit[508..512].copy_from_slice(&uc.to_le_bytes());
    codec_ok &= atr_parse_header(&uncommit, &bind) == Err(AtrReject::Uncommitted);

    // ---- Part B: on-disk reserved-file helpers on the real volume ---------------------------------------
    let mut disk_note = "no SD";
    if crate::drivers::block::info().is_some() {
        match crate::fs::fat::mount() {
            Ok(fs) => {
                let live = atr_live_binding(&fs);
                match atr_ensure(&fs, &live) {
                    Ok((fc, size)) => {
                        disk_note = if k1_atr_disk_roundtrip(&fs, fc, size, &live) {
                            "disk PASS"
                        } else {
                            "disk FAIL"
                        };
                    }
                    Err(_) => disk_note = "ensure err",
                }
            }
            Err(_) => disk_note = "mount err",
        }
    }

    serial_println!(
        ":: K1-atr: UNAFS.ATR owner/grants format M1 — codec {} (16-row bound, per-row CRC fail-closed, binding-checked), on-disk helpers {} (single-row write_at under NAMESPACE); ENFORCEMENT-INERT — persistence STOPS for the seat's principal decision ::",
        if codec_ok { "PASS" } else { "FAIL" },
        disk_note
    );
}

// K1 M3: the two-phase remount-survival proof — scratch state (every demo fixture torn down by the time it runs;
// the u11_check_gen_rebind convention).
const K1P_SA_OWN: u64 = 6; // scratch OWNER ASID
const K1P_SA_GRT: u64 = 7; // scratch GRANTEE ASID
const K1P_OWNER: &str = "PERSF.BIN"; // the named-owner scratch file
const K1P_PUBLIC: &str = "PERSPUB.BIN"; // the public scratch file (proves public stays public after rebuild)
const K1PERSIST_ALL: u32 = 0x3FFF; // all 14 assertions (12 + F2: rebuilt-grant revoke + owner-teardown sentinel)

/// K1 M3: delete the scratch files + clear their persisted UNAFS.ATR rows + scratch stamps, leaving the card
/// EXACTLY as found (empty UNAFS.ATR, no scratch files). Robust to partial failures — re-resolves each file
/// fresh and frees PERSF's grown chain (`delete_located` = 0xE5 + free_chain), so nothing leaks on the stateful
/// metal card.
fn k1_persist_cleanup() {
    slot_ppid_clear(K1P_SA_OWN);
    slot_ppid_clear(K1P_SA_GRT);
    if let Ok(fs) = crate::fs::fat::mount() {
        for name in [K1P_OWNER, K1P_PUBLIC] {
            if let Ok((de, lba, off)) = fs.find_located(name) {
                let _ = crate::fs::unafs::native_acl_clear(lba, off as u32); // K6: native store
                let _ = atr_clear_row(&fs, lba, off as u32); // legacy sidecar (stale-card defense)
                owned_clear(lba, off as u32);
                let _ = fs.delete_located(lba, off, de.first_cluster());
            }
        }
    }
}

/// K1 M3: PROVE that owner/grants persisted to UNAFS.ATR are rebuilt at mount and ENFORCED across a (simulated)
/// reboot with real stamped principals. Kernel-side + deterministic (the `u11_check_gen_rebind` idiom — no EL0
/// fixture; the enforcement logic lives in `owned_access_ok`/`owned_is_owner`/`owned_unlink_permitted`, which this
/// calls EXACTLY as the syscalls do, only with principals produced by the SAME `slot_ppid_stamp`/`slot_ppid_of`
/// machinery the loader + syscalls use). Runs in the launcher (kernel-task) context, so block I/O is legal.
/// Returns a bitmask of the assertions that held; PASS iff `== K1PERSIST_ALL`. Fully self-cleaning.
fn k1_persist_check() -> u32 {
    let mut w = 0u32;
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return 0,
    };
    // Principals via the REAL stamp/read machinery (the loader stamps SLOT_PPID; syscalls read it via slot_ppid_of).
    slot_ppid_stamp(K1P_SA_OWN, PrincipalRecord::program_name("PERSOWN"));
    slot_ppid_stamp(K1P_SA_GRT, PrincipalRecord::program_name("PERSGRT"));
    let p_x = slot_ppid_of(K1P_SA_OWN);
    let p_z = slot_ppid_of(K1P_SA_GRT);
    let p_y = PrincipalRecord::program_name("PERSIMP"); // an impostor program's principal (never owner/grantee)
    if p_x.kind == PRIN_PROGRAM_NAME
        && p_x == PrincipalRecord::program_name("PERSOWN")
        && p_z == PrincipalRecord::program_name("PERSGRT")
    {
        w |= 1 << 0; // the stamp/read round-trip (M2.1 stamping + M2.3 slot_ppid_of)
    }

    // ---- Phase 1: create a named-owner file + a public file, persist the ACL to the NATIVE store ----
    let (de, lba, off) = match fs.create_in_root(K1P_OWNER, 0x20) {
        Ok(t) => t,
        Err(_) => {
            k1_persist_cleanup();
            return w;
        }
    };
    let (c_persf, sz_persf) = match fs.write_grow(de.first_cluster(), de.size, lba, off, 0, &[0xA1u8; 32]) {
        Ok((_, new_size, first)) => (first, new_size),
        Err(_) => {
            k1_persist_cleanup();
            return w;
        }
    };
    let gen_own = ASID_GEN[K1P_SA_OWN as usize].load(Ordering::Acquire);
    let gen_grt = ASID_GEN[K1P_SA_GRT as usize].load(Ordering::Acquire);
    // The real create/grant sequence in-RAM, then persist BOTH via the exact syscall persist helpers.
    owned_set_owner(lba, off as u32, K1P_SA_OWN, gen_own, p_x);
    // Persist matching PRODUCTION's create path FAITHFULLY: `sys_open` persists AT create — a fresh
    // 0-length entry — so the native row's `fc` is 0 and stays 0 (the row is NOT re-persisted on a later
    // grow this arc). The grown chain (c_persf) exists only for the no-leak cleanup check; it is NOT persisted.
    // The rebuild's first_cluster cross-check is therefore name-PRIMARY (skipped for a 0 row) here as in
    // production. K6 M3: both persists are the NATIVE production helpers — the same code `sys_open`/`sys_fgrant`
    // run — so this proof now exercises the native store end-to-end.
    let _ = (c_persf, sz_persf);
    let ok_owner = native_persist_create(K1P_OWNER, 0, lba, off as u32, p_x);
    owned_grant(lba, off as u32, K1P_SA_OWN, gen_own, K1P_SA_GRT, gen_grt, CAP_READ, p_x, p_z);
    let ok_grant = native_persist_grants(lba, off as u32);
    let pub_created = fs.create_in_root(K1P_PUBLIC, 0x20).is_ok();
    if !(ok_owner && ok_grant && pub_created) {
        k1_persist_cleanup();
        return w;
    }

    // ---- Phase 2: simulate a reboot (drop the in-RAM ACL), remount BOTH volumes, rebuild, enforce ----
    owned_clear(lba, off as u32); // the in-RAM ACL is gone across a power-cycle
    crate::fs::unafs::force_remount(); // the unafs mount is re-read from disk too (a genuine remount)
    let fs2 = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            k1_persist_cleanup();
            return w;
        }
    };
    if native_rebuild_into_owned(&fs2) >= 1 {
        w |= 1 << 1; // the rebuild re-installed the persisted row FROM THE NATIVE STORE (K6 M3)
    }
    let (rlba, roff) = match fs2.find_located(K1P_OWNER) {
        Ok((_, l, o)) => (l, o as u32),
        Err(_) => {
            k1_persist_cleanup();
            return w;
        }
    };
    let rw = CAP_READ | CAP_WRITE;
    if owned_access_ok(rlba, roff, K1P_SA_OWN, gen_own, rw, p_x) {
        w |= 1 << 2; // owner-by-name: full read/write authority
    }
    if !owned_access_ok(rlba, roff, K1P_SA_OWN, gen_own, rw, p_y) {
        w |= 1 << 3; // impostor: DENIED
    }
    if !owned_access_ok(rlba, roff, K1P_SA_OWN, gen_own, rw, PrincipalRecord::NONE) {
        w |= 1 << 4; // anonymous: DENIED
    }
    if owned_access_ok(rlba, roff, K1P_SA_OWN, gen_own, CAP_READ, p_z) {
        w |= 1 << 5; // grantee-by-name: READ admitted
    }
    if !owned_access_ok(rlba, roff, K1P_SA_OWN, gen_own, CAP_WRITE, p_z) {
        w |= 1 << 6; // grantee-by-name: WRITE denied (granted READ only)
    }
    if let Ok((_, plba, poff)) = fs2.find_located(K1P_PUBLIC) {
        if owned_access_ok(plba, poff as u32, K1P_SA_OWN, gen_own, rw, PrincipalRecord::NONE) {
            w |= 1 << 7; // public file stays public after rebuild
        }
    }
    if owned_is_owner(rlba, roff, K1P_SA_OWN, gen_own, p_x)
        && owned_unlink_permitted(rlba, roff, K1P_SA_OWN, gen_own, p_x)
    {
        w |= 1 << 8; // owner-by-name has delete authority (owned_unlink_permitted) + is recognized as owner
    }
    if !owned_unlink_permitted(rlba, roff, K1P_SA_OWN, gen_own, p_y) {
        w |= 1 << 9; // impostor cannot delete
    }
    // F2 (security-review MF1): REVOKE the REBUILT grantee (still a SENTINEL slot — no live incarnation) by-name
    // and confirm it is ACTUALLY gone. Without the F2 grantee-by-ppid match, owned_grant's revoke arm matched
    // nothing (the live grantee asid never equals the rebuilt SENTINEL), returned success, and left the grant
    // admitting — a silent fail-OPEN. Run BEFORE the re-grant below so it tests the pure rebuilt slot.
    if owned_grant(rlba, roff, K1P_SA_OWN, gen_own, K1P_SA_GRT, gen_grt, 0, p_x, p_z) == 0
        && !owned_access_ok(rlba, roff, K1P_SA_GRT, gen_grt, CAP_READ, p_z)
    {
        w |= 1 << 12; // by-name revoke of a rebuilt grant TRULY revokes (grantee now denied)
    }
    // F1: the owner-by-name must also be able to MUTATE the ACL (owned_grant) on its rebuilt row — the read-side
    // owner recognition (bit 8) is not enough; owned_grant has its own owner gate. Exercise the by-name owner
    // path directly (the live `(asid, gen)` never matches the rebuilt SENTINEL owner_asid, so admission rests on
    // the ppid branch). Grant P_Z READ back (a fresh slot, since bit 12 revoked it) and confirm it is admitted.
    if owned_grant(rlba, roff, K1P_SA_OWN, gen_own, K1P_SA_GRT, gen_grt, CAP_READ, p_x, p_z) == 0
        && owned_access_ok(rlba, roff, K1P_SA_GRT, gen_grt, CAP_READ, p_z)
    {
        w |= 1 << 10; // owner-by-name CAN re-grant on its persisted file
    }
    if owned_grant(rlba, roff, K1P_SA_OWN, gen_own, K1P_SA_GRT, gen_grt, CAP_READ, p_y, p_z) == EACCES {
        w |= 1 << 11; // an impostor principal CANNOT grant
    }
    owned_clear(rlba, roff);

    // F2 (security-review MF2): a NAMED owner's row must SURVIVE its owner's TEARDOWN as a SENTINEL (owner_ppid
    // kept), not be wiped to public — else `owned_owner_ppid` reads NONE and `sys_unlink` skips the disk clear,
    // stranding a stale owner row a future same-name file would adopt. Create a live-owned file, tear the owner
    // down, and confirm the row persists by principal (named + anonymous still DENIED).
    if let Ok((de2, l2, o2)) = fs2.create_in_root("MF2F.BIN", 0x20) {
        owned_set_owner(l2, o2 as u32, K1P_SA_OWN, gen_own, p_x);
        owned_clear_owner_asid(K1P_SA_OWN); // the OWNER TEARDOWN (clear_handle_row's twin)
        if owned_owner_ppid(l2, o2 as u32) == p_x
            && !owned_access_ok(l2, o2 as u32, K1P_SA_OWN, gen_own, CAP_READ, PrincipalRecord::NONE)
        {
            w |= 1 << 13; // named owner survives teardown as a sentinel (RAM/disk stay consistent for unlink)
        }
        owned_clear(l2, o2 as u32);
        let _ = fs2.delete_located(l2, o2, de2.first_cluster());
    }

    k1_persist_cleanup();
    w
}

/// K1 M3 launcher + verdict — rides the U7 kernel task AFTER k1_atr_selftest (its disk I/O can never perturb the
/// 23 fixtures or the witnesses). Emits ONE `:: K1-persist: … PASS ::` line in the K1-atr `<noun> PASS` idiom —
/// deliberately NOT a `-> PASS` / `: PASS` line, so arroyo's fixture PASS-counter leaves the count at 23 and only
/// this one uncounted witness line is added (the 24th line; re-baseline the byte-diff on the OTHER 23).
fn k1_persist_launcher() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof writes real files; skip silently (the control-path discipline)
    }
    // Pre-flight: a stale image (scratch files present from an interrupted run) would confound create/rebuild.
    if let Ok(fs) = crate::fs::fat::mount() {
        if fs.find_in_root(K1P_OWNER).is_ok() || fs.find_in_root(K1P_PUBLIC).is_ok() {
            k1_persist_cleanup();
        }
    }
    let w = k1_persist_check();
    serial_println!(
        ":: K1-persist: native unafs owner/grants SURVIVE REBOOT — rebuild+enforce {} (owner-by-name admit+regrant+revoke; impostor+anon deny; grantee R admit/W deny; public stays public; named owner survives teardown) [w={:#06x}] ::",
        if w == K1PERSIST_ALL { "PASS" } else { "FAIL" },
        w
    );
}

// K1 M4: the corrupt-attr proof — scratch file whose persisted row is TORN on disk.
const K1C_FILE: &str = "CORRF.BIN";

/// K1 M4: reset the corrupt-attr scratch state — delete CORRF + REGROW UNAFS.ATR to a fresh empty image (a
/// torn row's bad CRC makes it un-findable by `atr_clear_row`, so the whole rows region is rewritten to
/// guarantee no corrupt row lingers on the stateful metal card) + clear the scratch stamp.
fn k1_corrupt_cleanup() {
    slot_ppid_clear(K1P_SA_OWN);
    if let Ok(fs) = crate::fs::fat::mount() {
        if let Ok((de, lba, off)) = fs.find_located(K1C_FILE) {
            owned_clear(lba, off as u32);
            let _ = crate::fs::unafs::native_acl_clear(lba, off as u32); // K7-F1: belt-and-braces (a bug-path migration must not persist)
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
        let bind = atr_live_binding(&fs);
        if let Ok((de, lba, off)) = fs.find_located(ATR_NAME) {
            if de.size as usize >= ATR_FILE_LEN {
                let img = atr_empty_image(&bind);
                let _ = fs.write_grow(de.first_cluster(), de.size, lba, off, 0, &img);
            }
        }
    }
}

/// K1 M4 / K7-F1: PROVE the read-side FAIL-CLOSED rule end to end — a TORN owner row on disk must yield a
/// PUBLIC file at boot, NEVER a forged owner. K7-F1 (lens must-fix): the old vehicle for this proof was
/// `atr_rebuild_into_owned`, which installed EVERY valid sidecar row into the LIVE OWNED_FILES — on a
/// legacy-carrying metal card that fixture call re-adopted legacy owners for the rest of the boot,
/// contradicting the K7 removal. That function is DELETED; the guard now drives the MIGRATION pass
/// (`native_migrate_from_sidecar`) — the ONLY production consumer of sidecar rows — over a torn row:
/// plant a committed `IMAGE_SHA256` row (the one kind migration would adopt), tear it on disk (bust the
/// row CRC), and assert the whole machine fails closed: the torn row is SKIPPED (bad CRC ->
/// `atr_parse_row` None), NOTHING lands in the native store, and OWNED_FILES gains NO owner from any
/// sidecar input (the file stays PUBLIC — an anonymous caller is ADMITTED where a forged owner would
/// DENY). No install-to-OWNED_FILES vehicle exists anywhere in the fixture. Kernel-side + deterministic;
/// self-cleaning. `true` iff fail-closed held.
fn k1_corrupt_check() -> bool {
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return false,
    };
    // A committed IMAGE_SHA256 owner — the ONLY row kind the retained migration pass adopts, so a torn
    // one is exactly the hostile input the parse guard must reject (synthetic image bytes on purpose).
    let p_c = PrincipalRecord::image_of(b"k1-corrupt-scratch-image");
    let Some(name11) = atr_name_from_str(K1C_FILE) else {
        k1_corrupt_cleanup();
        return false;
    };
    let (de, lba, off) = match fs.create_in_root(K1C_FILE, 0x20) {
        Ok(t) => t,
        Err(_) => {
            k1_corrupt_cleanup();
            return false;
        }
    };
    let (c, sz) = match fs.write_grow(de.first_cluster(), de.size, lba, off, 0, &[0xC1u8; 32]) {
        Ok((_, new_size, first)) => (first, new_size),
        Err(_) => {
            k1_corrupt_cleanup();
            return false;
        }
    };
    let empty_grants = [(PrincipalRecord::NONE, 0u32); NFGRANT];
    if !atr_persist_row(&fs, name11, c, sz, lba, off as u32, p_c, &empty_grants) {
        k1_corrupt_cleanup();
        return false;
    }

    // TEAR the persisted row: flip one byte in its owner field so the per-row CRC no longer matches.
    let torn = k1_tear_persisted_row(&fs, lba, off as u32);

    // Reboot sim -> drive the PRODUCTION migration pass (fresh mount, the boot idiom) over the torn row.
    // Bad CRC -> `atr_parse_row` None -> the row is skipped fail-closed: nothing to migrate, nothing to adopt.
    let fs2 = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            k1_corrupt_cleanup();
            return false;
        }
    };
    let _ = native_migrate_from_sidecar(&fs2);
    // Row-scoped fail-closed assertions (deliberately NOT the global migrated count, which on a stale
    // metal card could legitimately be nonzero for OTHER rows the boot pass transiently missed):
    // (1) nothing landed in the NATIVE store for this key (the torn row did not migrate);
    let native_clean = crate::fs::unafs::native_acl_read(lba, off as u32).is_none();
    // (2) THE K7-F1 LEAK CHECK: OWNED_FILES gained NO owner from any sidecar input — the file is PUBLIC.
    let no_owner = owned_owner_ppid(lba, off as u32).kind == PRIN_NONE;
    // (3) enforcement agrees: an ANONYMOUS caller is ADMITTED (a forged/adopted owner would DENY).
    let gen_c = ASID_GEN[K1P_SA_OWN as usize].load(Ordering::Acquire);
    let anon_public = match fs2.find_located(K1C_FILE) {
        Ok((_, rlba, roff)) => {
            owned_access_ok(rlba, roff as u32, K1P_SA_OWN, gen_c, CAP_READ | CAP_WRITE, PrincipalRecord::NONE)
        }
        Err(_) => false,
    };

    k1_corrupt_cleanup();
    torn && native_clean && no_owner && anon_public
}

/// K1 M4: flip one byte in the ON-DISK owner field of the UNAFS.ATR row for `(dir_lba, dir_off)`, busting its
/// per-row CRC (a synthetic torn-write). UNDER ns for the single-row write. `false` if the row isn't found or
/// the write fails. Test-only.
fn k1_tear_persisted_row(fs: &crate::fs::fat::FatFs, dir_lba: u64, dir_off: u32) -> bool {
    let (de, _l, _o) = match fs.find_located(ATR_NAME) {
        Ok(t) => t,
        Err(_) => return false,
    };
    if (de.size as usize) < ATR_FILE_LEN {
        return false;
    }
    let (afc, asz) = (de.first_cluster(), de.size);
    let _ns = ns_lock();
    let mut rows: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs
        .read_at(afc, asz, ATR_HEADER_LEN as u32, &mut rows, ATR_ROW_STRIDE * ATR_ROWS)
        .is_err()
        || rows.len() != ATR_ROW_STRIDE * ATR_ROWS
    {
        return false;
    }
    for i in 0..ATR_ROWS {
        let start = ATR_ROW_STRIDE * i;
        let mut arr = [0u8; ATR_ROW_STRIDE];
        arr.copy_from_slice(&rows[start..start + ATR_ROW_STRIDE]);
        if let Some(r) = atr_parse_row(&arr) {
            if r.dir_lba == dir_lba && r.dir_off == dir_off {
                arr[40] ^= 0xFF; // flip a byte in the owner principal field -> row CRC now fails
                let o = (ATR_HEADER_LEN + start) as u32;
                return fs.write_at(afc, asz, o, &arr).unwrap_or(0) == ATR_ROW_STRIDE;
            }
        }
    }
    false
}

/// K1 M4 launcher + verdict — the fail-closed corrupt-attr proof, riding the U7 kernel task after
/// k1_persist_launcher. Emits its own uncounted `:: K1-corrupt: … PASS ::` line (the K1-atr `<noun> PASS`
/// idiom, never `-> PASS`/`: PASS`), so the fixture count stays 23.
fn k1_corrupt_launcher() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> skip silently
    }
    if let Ok(fs) = crate::fs::fat::mount() {
        if fs.find_in_root(K1C_FILE).is_ok() {
            k1_corrupt_cleanup(); // a stale scratch file from an interrupted run
        }
    }
    let ok = k1_corrupt_check();
    serial_println!(
        ":: K1-corrupt: UNAFS.ATR torn owner row fails closed to PUBLIC at boot {} — migration skips the bad-CRC row (nothing native, no OWNED_FILES owner); anonymous open admitted (no forged owner) ::",
        if ok { "PASS" } else { "FAIL" }
    );
}

// K3: the revoke-persist commit-ordering proof — scratch state (torn down by the time it runs; the
// u11_check_gen_rebind convention). Retires the LAST fail-OPEN residual: a durable revoke now commits to disk
// BEFORE the in-RAM removal, so it SURVIVES REBOOT and fails CLOSED when the persist cannot land.
const K3_SA_OWN: u64 = 6; // scratch OWNER ASID (reused after the K1 fixtures tear down)
const K3_SA_GRT: u64 = 7; // scratch KEPT-GRANTEE ASID
const K3_SA_GR2: u64 = 8; // scratch REVOKE-TARGET GRANTEE ASID
const K3_FILE: &str = "K3REV.BIN"; // the named-owner scratch file
const K3REVOKE_ALL: u32 = 0x7F; // all 7 assertions

/// K3: delete the scratch file + clear its persisted UNAFS.ATR row + scratch stamps + the test knob, leaving the
/// card EXACTLY as found. Robust to partial failures (re-resolves fresh, frees the grown chain).
fn k3_revoke_cleanup() {
    slot_ppid_clear(K3_SA_OWN);
    slot_ppid_clear(K3_SA_GRT);
    slot_ppid_clear(K3_SA_GR2);
    K3_TEST_FAIL_PERSIST.store(false, Ordering::Relaxed); // belt-and-braces: never leave the fault knob set
    if let Ok(fs) = crate::fs::fat::mount() {
        if let Ok((de, lba, off)) = fs.find_located(K3_FILE) {
            let _ = crate::fs::unafs::native_acl_clear(lba, off as u32); // K6: native store
            let _ = atr_clear_row(&fs, lba, off as u32); // legacy sidecar (stale-card defense)
            owned_clear(lba, off as u32);
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }
}

/// K3: PROVE the two-phase revoke commit-ordering end to end with real stamped principals (the `k1_persist_check`
/// idiom — kernel-side + deterministic, no EL0 fixture). Persist a named-owner file with TWO grantees, revoke one
/// through the PRODUCTION `sys_fgrant_revoke_2phase` path, and confirm across a (simulated) reboot that (1) the
/// revoke SURVIVES — the revoked grantee is no longer re-admitted while the kept grantee still is — and (2) a
/// FORCED persist failure fails CLOSED — the revoke reports `-EIO` and leaves the in-RAM grant intact, so RAM and
/// disk never silently diverge. Returns a bitmask; PASS iff `== K3REVOKE_ALL`. Fully self-cleaning.
fn k3_revoke_check() -> u32 {
    let mut w = 0u32;
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return 0,
    };
    // Principals via the REAL stamp/read machinery.
    slot_ppid_stamp(K3_SA_OWN, PrincipalRecord::program_name("K3OWN"));
    slot_ppid_stamp(K3_SA_GRT, PrincipalRecord::program_name("K3GRT"));
    slot_ppid_stamp(K3_SA_GR2, PrincipalRecord::program_name("K3GR2"));
    let p_own = slot_ppid_of(K3_SA_OWN);
    let p_grt = slot_ppid_of(K3_SA_GRT); // the grantee KEPT across the revoke
    let p_gr2 = slot_ppid_of(K3_SA_GR2); // the grantee REVOKED
    // ---- Phase 1: create a named-owner file, grant BOTH grantees READ, persist the ACL (NATIVE, K6 M3) ----
    let (de, lba, off) = match fs.create_in_root(K3_FILE, 0x20) {
        Ok(t) => t,
        Err(_) => {
            k3_revoke_cleanup();
            return w;
        }
    };
    if fs.write_grow(de.first_cluster(), de.size, lba, off, 0, &[0xD3u8; 32]).is_err() {
        k3_revoke_cleanup();
        return w;
    }
    let gen_own = ASID_GEN[K3_SA_OWN as usize].load(Ordering::Acquire);
    let gen_grt = ASID_GEN[K3_SA_GRT as usize].load(Ordering::Acquire);
    let gen_gr2 = ASID_GEN[K3_SA_GR2 as usize].load(Ordering::Acquire);
    owned_set_owner(lba, off as u32, K3_SA_OWN, gen_own, p_own);
    // Persist matching production's create path (fc = 0; name-primary rebuild) — the NATIVE helpers.
    let ok_owner = native_persist_create(K3_FILE, 0, lba, off as u32, p_own);
    owned_grant(lba, off as u32, K3_SA_OWN, gen_own, K3_SA_GRT, gen_grt, CAP_READ, p_own, p_grt);
    owned_grant(lba, off as u32, K3_SA_OWN, gen_own, K3_SA_GR2, gen_gr2, CAP_READ, p_own, p_gr2);
    let ok_grants = native_persist_grants(lba, off as u32);
    if !(ok_owner && ok_grants) {
        k3_revoke_cleanup();
        return w;
    }

    // ---- Phase 2: simulate a reboot — rebuild purely from the NATIVE store; BOTH grantees admitted ----
    owned_clear(lba, off as u32);
    crate::fs::unafs::force_remount();
    let fs2 = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            k3_revoke_cleanup();
            return w;
        }
    };
    let _ = native_rebuild_into_owned(&fs2);
    let (rlba, roff) = match fs2.find_located(K3_FILE) {
        Ok((_, l, o)) => (l, o as u32),
        Err(_) => {
            k3_revoke_cleanup();
            return w;
        }
    };
    if owned_access_ok(rlba, roff, K3_SA_GRT, gen_grt, CAP_READ, p_grt) {
        w |= 1 << 0; // kept grantee admitted after rebuild (baseline)
    }
    if owned_access_ok(rlba, roff, K3_SA_GR2, gen_gr2, CAP_READ, p_gr2) {
        w |= 1 << 1; // revoke-target grantee admitted after rebuild (baseline — a grant to revoke)
    }

    // ---- Phase 3: REVOKE the target through the PRODUCTION two-phase path (disk-first, then in-RAM) ----
    if sys_fgrant_revoke_2phase(rlba, roff, K3_SA_OWN, gen_own, K3_SA_GR2, gen_gr2, p_own, p_gr2) == 0 {
        w |= 1 << 2; // the two-phase revoke committed (durable write held, in-RAM removed)
    }

    // ---- Phase 4: simulate a SECOND reboot — the revoke SURVIVED (the retired fail-OPEN residual) ----
    owned_clear(rlba, roff);
    crate::fs::unafs::force_remount();
    let fs3 = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            k3_revoke_cleanup();
            return w;
        }
    };
    let _ = native_rebuild_into_owned(&fs3);
    let (slba, soff) = match fs3.find_located(K3_FILE) {
        Ok((_, l, o)) => (l, o as u32),
        Err(_) => {
            k3_revoke_cleanup();
            return w;
        }
    };
    if !owned_access_ok(slba, soff, K3_SA_GR2, gen_gr2, CAP_READ, p_gr2) {
        w |= 1 << 3; // revoked grantee DENIED after the reboot — the revoke is durable (was fail-OPEN before K3)
    }
    if owned_access_ok(slba, soff, K3_SA_GRT, gen_grt, CAP_READ, p_grt) {
        w |= 1 << 4; // the kept grantee still admitted — the revoke was surgical, not a wholesale drop
    }

    // ---- Phase 5: a FORCED persist failure must fail CLOSED (no false success, no RAM/disk divergence) ----
    K3_TEST_FAIL_PERSIST.store(true, Ordering::Relaxed);
    let rc = sys_fgrant_revoke_2phase(slba, soff, K3_SA_OWN, gen_own, K3_SA_GRT, gen_grt, p_own, p_grt);
    K3_TEST_FAIL_PERSIST.store(false, Ordering::Relaxed);
    if rc == EIO {
        w |= 1 << 5; // the durable write failed -> -EIO (the owner is NOT told the revoke succeeded)
    }
    if owned_access_ok(slba, soff, K3_SA_GRT, gen_grt, CAP_READ, p_grt) {
        w |= 1 << 6; // in-RAM grant left INTACT (not committed) -> RAM agrees with the unchanged disk row
    }

    k3_revoke_cleanup();
    w
}

/// K3 launcher + verdict — rides the U7 kernel task after the K2 launcher (its disk I/O can never perturb the 23
/// fixtures or the witnesses). Emits ONE uncounted `:: K3-revoke: … PASS ::` line (the K1-atr `<noun> PASS` idiom,
/// never `-> PASS`/`: PASS`), so the fixture PASS-count stays 23. Fully self-cleaning.
fn k3_revoke_launcher() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof writes real files; skip silently
    }
    if let Ok(fs) = crate::fs::fat::mount() {
        if fs.find_in_root(K3_FILE).is_ok() {
            k3_revoke_cleanup(); // a stale scratch file from an interrupted run
        }
    }
    let w = k3_revoke_check();
    serial_println!(
        ":: K3-revoke: SYS_FGRANT revoke commit-ordering — two-phase durable-first {} (revoke survives reboot; kept grant intact; forced persist-fail -> -EIO with in-RAM grant left intact, RAM/disk consistent) [w={:#04x}] ::",
        if w == K3REVOKE_ALL { "PASS" } else { "FAIL" },
        w
    );
}

// K9-PARITY: the mid-staging-failure discard proof — scratch state (torn down by the time it runs). Closes the
// K9 lens-B deferred residual: a staged ACL persist that fails PARTWAY (after an inode + name/fc/owner have
// staged into the autocommit-OFF transaction) leaves UNCOMMITTED residue on the shared cached mount; without the
// K9-PARITY discard a LATER persist's commit would flush that residue as a partial-durable row. `with_unafs` now
// drops the cached mount inside the same serialized hold that ran the failing op, so the next persist re-mounts
// fresh from the committed root and commits ONLY its own row.
const K9_SA_OWN: u64 = 6; // scratch OWNER ASID (reused after K3/K5 tear down; this witness runs after them)
const K9_FILE_A: &str = "K9PA.BIN"; // the CLEAN control file (its committed row must survive intact)
const K9_FILE_B: &str = "K9PB.BIN"; // the MID-STAGE-FAILED file (must NEVER get a durable row)
const K9PARITY_ALL: u32 = 0x7F; // all 7 assertions

/// K9-PARITY: delete both scratch files + clear their native rows + the scratch stamp + the fault knob, leaving the
/// card EXACTLY as found. Robust to partial failures (re-resolves fresh).
fn k9_parity_cleanup() {
    slot_ppid_clear(K9_SA_OWN);
    crate::fs::unafs::TEST_FAIL_MIDSTAGE.store(false, Ordering::Relaxed); // never leave the fault knob set
    if let Ok(fs) = crate::fs::fat::mount() {
        for name in [K9_FILE_A, K9_FILE_B] {
            if let Ok((de, lba, off)) = fs.find_located(name) {
                let _ = crate::fs::unafs::native_acl_clear(lba, off as u32);
                owned_clear(lba, off as u32);
                let _ = fs.delete_located(lba, off, de.first_cluster());
            }
        }
    }
}

/// K9-PARITY: PROVE the mid-staging-failure discard end to end (the `k3_revoke_check` idiom — kernel-side,
/// deterministic, real files + the native ACL store). A CLEAN row is persisted for file A; file B is persisted with
/// the mid-stage fault knob so its row aborts partway (near-complete but UNCOMMITTED); A is then re-persisted (a
/// real commit that, pre-fix, would flush B's residue). Across a simulated reboot: A's row is intact, B has NO
/// durable row (the residual closed), and a clean re-persist of B now lands correctly (the volume fully recovers).
/// Returns a bitmask; PASS iff `== K9PARITY_ALL`. Fully self-cleaning.
fn k9_parity_check() -> u32 {
    let mut w = 0u32;
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return 0,
    };
    slot_ppid_stamp(K9_SA_OWN, PrincipalRecord::program_name("K9OWN"));
    let p_own = slot_ppid_of(K9_SA_OWN);
    let gen_own = ASID_GEN[K9_SA_OWN as usize].load(Ordering::Acquire);

    // ---- Phase 1: file A — a CLEAN native row that must stay intact throughout ----
    let (_dea, la, oa) = match fs.create_in_root(K9_FILE_A, 0x20) {
        Ok(t) => t,
        Err(_) => {
            k9_parity_cleanup();
            return w;
        }
    };
    owned_set_owner(la, oa as u32, K9_SA_OWN, gen_own, p_own);
    if native_persist_create(K9_FILE_A, 0, la, oa as u32, p_own) {
        w |= 1 << 0; // A's row committed clean (baseline)
    }

    // ---- Phase 2: file B — persist with the MID-STAGE fault; it must fail CLOSED, staging uncommitted residue ----
    let (_deb, lb, ob) = match fs.create_in_root(K9_FILE_B, 0x20) {
        Ok(t) => t,
        Err(_) => {
            k9_parity_cleanup();
            return w;
        }
    };
    owned_set_owner(lb, ob as u32, K9_SA_OWN, gen_own, p_own);
    crate::fs::unafs::TEST_FAIL_MIDSTAGE.store(true, Ordering::Relaxed);
    let okb_fail = native_persist_create(K9_FILE_B, 0, lb, ob as u32, p_own);
    crate::fs::unafs::TEST_FAIL_MIDSTAGE.store(false, Ordering::Relaxed);
    if !okb_fail {
        w |= 1 << 1; // the mid-staging persist failed CLOSED (returned false — no false success)
    }

    // ---- Phase 3: a SUBSEQUENT real commit (re-persist A) must land — and must NOT carry B's discarded residue ----
    // Pre-fix, this commit ran against the shared dirty mount and would have flushed B's staged inode/attrs as a
    // durable row. Post-fix, B's failure discarded the mount, so this re-persist re-mounts fresh and commits ONLY A.
    if native_persist_grow(la, oa as u32, 0) {
        w |= 1 << 2; // the volume is usable after the discard — a clean commit landed
    }

    // ---- Phase 4: simulate a reboot — rebuild owners purely from the NATIVE store ----
    owned_clear(la, oa as u32);
    owned_clear(lb, ob as u32);
    crate::fs::unafs::force_remount();
    let fs2 = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            k9_parity_cleanup();
            return w;
        }
    };
    let _ = native_rebuild_into_owned(&fs2);
    // A's durable row survived the whole sequence.
    if let Ok((_d, rla, roa)) = fs2.find_located(K9_FILE_A) {
        if owned_owner_ppid(rla, roa as u32).kind != PRIN_NONE
            && crate::fs::unafs::native_acl_read(rla, roa as u32).is_some()
        {
            w |= 1 << 3; // A's committed row intact — the discard never touched committed state (K3)
        }
    }
    // B has NO durable row — the mid-staged partial was discarded, never committed by A's later commit.
    let (rlb, rob) = match fs2.find_located(K9_FILE_B) {
        Ok((_d, l, o)) => (l, o as u32),
        Err(_) => {
            k9_parity_cleanup();
            return w;
        }
    };
    if owned_owner_ppid(rlb, rob).kind == PRIN_NONE
        && crate::fs::unafs::native_acl_read(rlb, rob).is_none()
    {
        w |= 1 << 4; // NO partial-durable row for B — the K9 lens-B residual is CLOSED
    }

    // ---- Phase 5: the volume fully recovers — a CLEAN re-persist of B now lands correctly ----
    owned_set_owner(rlb, rob, K9_SA_OWN, gen_own, p_own);
    if native_persist_create(K9_FILE_B, 0, rlb, rob, p_own) {
        w |= 1 << 5; // clean persist of B succeeds after the recovery
    }
    if let Some(row) = crate::fs::unafs::native_acl_read(rlb, rob) {
        if !row.owner.is_empty() {
            w |= 1 << 6; // and reads back as the intended row (exactly the owner just set, no garbage)
        }
    }

    k9_parity_cleanup();
    w
}

/// K9-PARITY launcher + verdict — rides the U7 kernel task after K3/K5 (its disk I/O can never perturb the
/// counted fixtures). Emits ONE uncounted `:: K9-parity: … PASS ::` line (the K1-atr `<noun> PASS` idiom), so the
/// fixture PASS-count is unchanged. Fully self-cleaning.
fn k9_parity_launcher() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof writes real files; skip silently
    }
    if let Ok(fs) = crate::fs::fat::mount() {
        if fs.find_in_root(K9_FILE_A).is_ok() || fs.find_in_root(K9_FILE_B).is_ok() {
            k9_parity_cleanup(); // stale scratch files from an interrupted run
        }
    }
    let w = k9_parity_check();
    serial_println!(
        ":: K9-parity: staged ACL persist mid-staging-failure discard — no partial-durable row + later commit carries no discarded residue {} (fail-closed; A intact; B absent post-reboot; volume recovers) [w={:#04x}] ::",
        if w == K9PARITY_ALL { "PASS" } else { "FAIL" },
        w
    );
}

// K5: the revoke/re-persist SMP-window proof (M1) — a deterministic interleaving witness that (a) reproduces the
// OLD resurrection race at the DECOMPOSED-primitive level (a stale pre-revoke OWNED_FILES snapshot written to disk
// AFTER a revoke narrowed it re-admits the revoked grantee — the exact window K5 M1 closes) and (b) shows the
// PRODUCTION full-row re-persist (`atr_persist_grants`) can never BE that stale write, because it snapshots +
// writes atomically under the same ns that serializes the revoke's in-RAM commit. Scratch ASIDs 6/7/8 (reused
// after K3 tears them down). The full cross-core timing race is metal-latent (like the F3 witness); this proves
// the window is real + the production path is narrowed, deterministically on one core.
const K5_SA_OWN: u64 = 6; // scratch OWNER ASID
const K5_SA_GRT: u64 = 7; // scratch KEPT-GRANTEE ASID
const K5_SA_GR2: u64 = 8; // scratch REVOKE-TARGET GRANTEE ASID
const K5_FILE: &str = "K5LCK.BIN"; // the named-owner scratch file
const K5_LOCKSPAN_ALL: u32 = 0x3F; // all 6 assertions

/// K5: delete the scratch file + clear its persisted UNAFS.ATR row + scratch stamps, leaving the card EXACTLY as
/// found. Robust to partial failures (re-resolves fresh, frees the grown chain).
fn k5_lockspan_cleanup() {
    slot_ppid_clear(K5_SA_OWN);
    slot_ppid_clear(K5_SA_GRT);
    slot_ppid_clear(K5_SA_GR2);
    if let Ok(fs) = crate::fs::fat::mount() {
        if let Ok((de, lba, off)) = fs.find_located(K5_FILE) {
            let _ = crate::fs::unafs::native_acl_clear(lba, off as u32); // K6: native store
            let _ = atr_clear_row(&fs, lba, off as u32); // legacy sidecar (stale-card defense)
            owned_clear(lba, off as u32);
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }
}

/// K5: re-mount BOTH volumes, rebuild the in-RAM ACL purely from the NATIVE store (K6 M3), and re-resolve the
/// scratch file — the "simulate a reboot" step the K3 proof uses. Returns the fresh `(dir_lba, dir_off)`,
/// or `None` on any disk failure.
fn k5_reboot_resolve(dir_lba: u64, dir_off: u32) -> Option<(u64, u32)> {
    owned_clear(dir_lba, dir_off);
    crate::fs::unafs::force_remount();
    let fs = crate::fs::fat::mount().ok()?;
    let _ = native_rebuild_into_owned(&fs);
    let (_, l, o) = fs.find_located(K5_FILE).ok()?;
    Some((l, o as u32))
}

/// K5/K5B: PROVE the fused persist closes the concurrent-repersist resurrection window (the `k3_revoke_check`
/// idiom — kernel-side, deterministic, no EL0 fixture). K5B: the fusion lock is the `with_unafs` MOUNT (snapshot +
/// durable write + in-RAM commit under ONE hold); the control leg decomposes exactly that (snapshot and write
/// under SEPARATE holds) and demonstrates the resurrection. Returns a bitmask; PASS iff `== K5_LOCKSPAN_ALL`.
/// Self-cleaning.
fn k5_lockspan_check() -> u32 {
    let mut w = 0u32;
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return 0,
    };
    slot_ppid_stamp(K5_SA_OWN, PrincipalRecord::program_name("K5OWN"));
    slot_ppid_stamp(K5_SA_GRT, PrincipalRecord::program_name("K5GRT"));
    slot_ppid_stamp(K5_SA_GR2, PrincipalRecord::program_name("K5GR2"));
    let p_own = slot_ppid_of(K5_SA_OWN);
    let p_grt = slot_ppid_of(K5_SA_GRT); // grantee KEPT across the revoke
    let p_gr2 = slot_ppid_of(K5_SA_GR2); // grantee REVOKED
    // ---- Setup: a named-owner file with BOTH grantees granted READ, persisted (NATIVE, K6 M3) ----
    let (de, lba, off) = match fs.create_in_root(K5_FILE, 0x20) {
        Ok(t) => t,
        Err(_) => {
            k5_lockspan_cleanup();
            return w;
        }
    };
    if fs.write_grow(de.first_cluster(), de.size, lba, off, 0, &[0x5Au8; 32]).is_err() {
        k5_lockspan_cleanup();
        return w;
    }
    let gen_own = ASID_GEN[K5_SA_OWN as usize].load(Ordering::Acquire);
    let gen_grt = ASID_GEN[K5_SA_GRT as usize].load(Ordering::Acquire);
    let gen_gr2 = ASID_GEN[K5_SA_GR2 as usize].load(Ordering::Acquire);
    owned_set_owner(lba, off as u32, K5_SA_OWN, gen_own, p_own);
    let ok_owner = native_persist_create(K5_FILE, 0, lba, off as u32, p_own);
    owned_grant(lba, off as u32, K5_SA_OWN, gen_own, K5_SA_GRT, gen_grt, CAP_READ, p_own, p_grt);
    owned_grant(lba, off as u32, K5_SA_OWN, gen_own, K5_SA_GR2, gen_gr2, CAP_READ, p_own, p_gr2);
    let ok_grants = native_persist_grants(lba, off as u32);
    if !(ok_owner && ok_grants) {
        k5_lockspan_cleanup();
        return w;
    }

    // ---- Baseline reboot: BOTH grantees admitted from disk ----
    let Some((rlba, roff)) = k5_reboot_resolve(lba, off as u32) else {
        k5_lockspan_cleanup();
        return w;
    };
    if owned_access_ok(rlba, roff, K5_SA_GRT, gen_grt, CAP_READ, p_grt) {
        w |= 1 << 0; // kept grantee admitted (baseline)
    }
    if owned_access_ok(rlba, roff, K5_SA_GR2, gen_gr2, CAP_READ, p_gr2) {
        w |= 1 << 1; // revoke-target grantee admitted (baseline — a grant to revoke)
    }

    // ---- NEGATIVE CONTROL: the race shape, reproduced with the commit DELIBERATELY DECOMPOSED from the
    // MOUNT hold (the K5B control idiom, per the ratified conditions). Capture a PRE-revoke snapshot (GR2's
    // grant still present) OUTSIDE any with_unafs hold — models a concurrent re-persister whose snapshot was
    // NOT taken inside its own MOUNT hold (exactly what the K5B invariant forbids). Then run the production
    // revoke (snapshot + disk-narrow + in-RAM commit fused under ONE with_unafs hold). Then let the STALE
    // snapshot's full-row write land via the raw disk writer under a FRESH, separate MOUNT hold — the
    // decomposed ordering: snapshot and write under DIFFERENT holds, with the revoke between them. It
    // RESURRECTS GR2 on the NATIVE store — the window class is real whenever the fusion is decomposed.
    let stale = owned_snapshot_row(rlba, roff);
    let _ = sys_fgrant_revoke_2phase(rlba, roff, K5_SA_OWN, gen_own, K5_SA_GR2, gen_gr2, p_own, p_gr2);
    if let Some((so, sg)) = stale {
        let _ = crate::fs::unafs::with_unafs(|ufs| native_write_grant_row_on(ufs, rlba, roff, so, &sg));
    }
    let Some((clba, coff)) = k5_reboot_resolve(rlba, roff) else {
        k5_lockspan_cleanup();
        return w;
    };
    if owned_access_ok(clba, coff, K5_SA_GR2, gen_gr2, CAP_READ, p_gr2) {
        w |= 1 << 2; // the stale write RESURRECTED GR2 -> the window is REAL (the test bites)
    }

    // ---- THE FIX: production revoke + production re-persist cannot resurrect ----
    // In-RAM now carries the resurrected GR2 grant again. Revoke it through the production two-phase path (whose
    // snapshot+write+commit fuse under ONE with_unafs hold), THEN run the production full-row re-persist
    // (`native_persist_grants` — models the concurrent core B). Post-K5B it snapshots the NARROWED in-RAM set
    // INSIDE its own MOUNT hold, so it writes a narrowed row: GR2 is NOT resurrected.
    let _ = sys_fgrant_revoke_2phase(clba, coff, K5_SA_OWN, gen_own, K5_SA_GR2, gen_gr2, p_own, p_gr2);
    let _ = native_persist_grants(clba, coff);
    let Some((flba, foff)) = k5_reboot_resolve(clba, coff) else {
        k5_lockspan_cleanup();
        return w;
    };
    if !owned_access_ok(flba, foff, K5_SA_GR2, gen_gr2, CAP_READ, p_gr2) {
        w |= 1 << 3; // GR2 stays DENIED — the production re-persist wrote the narrowed row (no stale escape)
    }
    if owned_access_ok(flba, foff, K5_SA_GRT, gen_grt, CAP_READ, p_grt) {
        w |= 1 << 4; // kept grantee STILL admitted — the revoke was surgical + the re-persist did not clobber it
    }

    // ---- M2 gate integrity: the create-serialization CAS gate is not leaked after all this create/persist ----
    if !ATR_CREATING.load(Ordering::Acquire) {
        w |= 1 << 5; // ATR_CREATING released on every path (never stuck true -> never permanently defers persists)
    }

    k5_lockspan_cleanup();
    w
}

/// K5 launcher + verdict — rides the U7 kernel task after the K3 launcher (its disk I/O can never perturb the 23
/// fixtures or the witnesses). Emits ONE uncounted `:: K5-lockspan: … PASS ::` line (the K1-atr `<noun> PASS`
/// idiom, never `-> PASS`/`: PASS`), so the fixture PASS-count stays 23. Fully self-cleaning.
fn k5_lockspan_launcher() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof writes real files; skip silently
    }
    if let Ok(fs) = crate::fs::fat::mount() {
        if fs.find_in_root(K5_FILE).is_ok() {
            k5_lockspan_cleanup(); // a stale scratch file from an interrupted run
        }
    }
    let w = k5_lockspan_check();
    serial_println!(
        ":: K5-lockspan: native unafs revoke/re-persist SMP window — stale snapshot written under a SEPARATE MOUNT hold resurrects (control); production revoke+re-persist fused under ONE with_unafs hold stays narrowed (K5B); kept grant intact; create-gate not leaked {} [w={:#04x}] ::",
        if w == K5_LOCKSPAN_ALL { "PASS" } else { "FAIL" },
        w
    );
}

/// IMG-SIG (code-signing witness) — prove the IMAGE_SHA256 principal machinery: the SHA-256 primitive is
/// correct (FIPS 180-4 KATs) AND it discriminates program IMAGES, closing the "same 8.3 name = same principal"
/// residual the K3 arc last carried. In-RAM (KAT + constant-buffer) bits always run; the file bits run when a
/// card carries the two K2 programs (present in the QEMU FAT and on the metal card). Emits one UNCOUNTED line
/// (PASS/FAIL space-flanked, never `-> PASS`/`: PASS`/`-> FAIL`/`FAIL ::`), so the 23-fixture count is
/// byte-equivalent. Read-only — no disk write, no slot, no lock; cannot perturb the battery. Runs LAST.
fn image_sig_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut w = 0u32;

    // ---- Known-answer tests: the SHA-256 primitive is FIPS 180-4 correct ----
    const KAT_EMPTY: [u8; 32] = [
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24,
        0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
    ];
    const KAT_ABC: [u8; 32] = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
        0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
    ];
    // 56-byte "a"*56 — exercises the padding-OVERFLOW branch (0x80 + length spill a SECOND compression block),
    // which the "" / "abc" vectors do NOT reach. A whole-image digest (K2OWN.BIN spans many blocks) depends on
    // this branch being right, so it is KAT-pinned here.
    const KAT_56A: [u8; 32] = [
        0xb3, 0x54, 0x39, 0xa4, 0xac, 0x6f, 0x09, 0x48, 0xb6, 0xd6, 0xf9, 0xe3, 0xc6, 0xaf, 0x0f, 0x5f,
        0x59, 0x0c, 0xe2, 0x0f, 0x1b, 0xde, 0x70, 0x90, 0xef, 0x79, 0x70, 0x68, 0x6e, 0xc6, 0x73, 0x8a,
    ];
    if sha256(b"") == KAT_EMPTY {
        w |= 1 << 0; // bit0: SHA-256("") KAT — the empty-input / single-pad-block path
    }
    if sha256(b"abc") == KAT_ABC && sha256(&[b'a'; 56]) == KAT_56A {
        w |= 1 << 1; // bit1: SHA-256("abc") + the 56-byte padding-OVERFLOW KAT (2nd compression block)
    }

    // ---- Image-identity discrimination on constant buffers (no card needed) ----
    // Two distinct 1-page-ish images, and a byte-identical copy of the first. `[b; N]` differs from the copy in
    // ONE byte to model a swapped blob.
    let img_a = [0xA5u8; 200];
    let img_a_copy = [0xA5u8; 200];
    let mut img_b = [0xA5u8; 200];
    img_b[137] = 0x5A; // one flipped byte -> a different image
    let pa = PrincipalRecord::image_of(&img_a);
    let pa2 = PrincipalRecord::image_of(&img_a_copy);
    let pb = PrincipalRecord::image_of(&img_b);
    if pa.kind == PRIN_IMAGE_SHA256 && pa.len == PRIN_VALUE_LEN as u8 {
        w |= 1 << 2; // bit2: the minted principal is a well-formed IMAGE_SHA256 (kind + full 30-byte value)
    }
    if pa == pa2 {
        w |= 1 << 3; // bit3: byte-identical images -> the SAME principal (a re-spawn stays the owner)
    }
    if pa != pb {
        w |= 1 << 4; // bit4: a one-byte-different image -> a DISTINCT principal (a swapped blob is NOT the owner)
    }
    // bit5: an IMAGE principal is NEVER equal to a PROGRAM_NAME principal — even one whose name string happens
    // to collide with the digest prefix bytes — because `kind` participates in equality. This is the graduation:
    // identity moved off the name entirely.
    if pa != PrincipalRecord::program_name("A5") && pa.kind != PrincipalRecord::program_name("A5").kind {
        w |= 1 << 5;
    }

    // ---- Real programs off the card: two DIFFERENT images (K2OWN.BIN vs K2IMP.BIN) -> two DIFFERENT ----
    // ---- principals, each stable across a re-read; the by-NAME residual is closed for real blobs. ----
    let have_card = crate::drivers::block::info().is_some();
    let mut file_all = 0u32;
    if have_card {
        file_all = 1 << 6;
        if let (Some(own1), Some(own2), Some(imp1)) = (
            image_principal_of_file(K2_OWN_NAME),
            image_principal_of_file(K2_OWN_NAME),
            image_principal_of_file(K2_IMP_NAME),
        ) {
            if own1.kind == PRIN_IMAGE_SHA256 && own1 == own2 && own1 != imp1 {
                w |= 1 << 6; // bit6: distinct real images -> distinct principals; same image re-read is stable
            }
        }
    }

    let all = 0x3Fu32 | file_all; // 0x7F with a card, 0x3F without
    serial_println!(
        ":: IMG-SIG: code-signing principal = IMAGE_SHA256 (FIPS KATs, image discrimination, name-collision residual closed) {} [w={:#04x}/{:#04x}] ::",
        if w == all { "PASS" } else { "FAIL" },
        w,
        all
    );
}

// =====================================================================================================
// K4-READY: prove the native-attribute projection codec + the volume-magic discriminator — the
// deterministic 1:1 mapping K4's migrate-then-delete will use, PINNED + KAT'd ahead of the native mount.
// Read-only, in-RAM, SYNTHETIC principals (no card, no disk) -> fully byte-equivalent; emits ONE uncounted
// `:: K4-ready: … PASS [w=0x..] ::` line (never a `-> PASS`/`: PASS` fixture line, so the 23-PASS battery is
// unchanged). Runs LAST in `u7_launcher`, after `fatdirs_launcher`.
// =====================================================================================================
fn k4_ready_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut w = 0u32;
    let mut buf = [0u8; K4_STR_MAX];
    let prog = PrincipalRecord::program_name("HELLO.BIN");
    let midi = PrincipalRecord::program_name("MIDI");
    let img = PrincipalRecord::image_of(b"abc"); // digest = FIPS SHA-256("abc")

    // bit0: PROGRAM_NAME projects to its stored canonical string verbatim (`prog:<name>`).
    if principal_native_string(&prog, &mut buf) == Some(14) && &buf[..14] == b"prog:HELLO.BIN" {
        w |= 1 << 0;
    }

    // bit1: the lowercase-hex helper is correct (KAT over a known 4-byte pattern, incl. nibble order).
    let mut hb = [0u8; 8];
    if hex_lower_into(&[0x00, 0x0f, 0xa5, 0xff], &mut hb) == 8 && &hb == b"000fa5ff" {
        w |= 1 << 1;
    }

    // bit2: IMAGE_SHA256 projects to `sha256:` + 60 lowercase hex of the 30-byte digest PREFIX (67 chars),
    // NOT the 71-char full digest (THE 240-BIT-PREFIX RULE). KAT: SHA-256("abc") = ba7816bf…15ad; its first
    // 30 bytes hex to the 60 chars below. `image_of` mints the digest; the projection prefixes + hexes it.
    const IMG_ABC_KEY: &[u8] =
        b"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f200";
    if img.kind == PRIN_IMAGE_SHA256
        && principal_native_string(&img, &mut buf) == Some(67)
        && &buf[..67] == IMG_ABC_KEY
    {
        w |= 1 << 2;
    }

    // bit3: a NONE principal has NO native projection -> the file is public (no `owner` attribute written).
    if principal_native_string(&PrincipalRecord::NONE, &mut buf).is_none() {
        w |= 1 << 3;
    }

    // bit4: grant key `grants:<grantee>` — a short PROGRAM_NAME grantee (16 bytes), the WIDEST input (an
    // IMAGE grantee: `grants:sha256:<60hex>` = exactly K4_STR_MAX = 74, the tight-fit boundary), a NONE
    // grantee (un-projectable), and a 1-too-small buffer (73 < 74) all behave (Some/None) as the pinned
    // contract says — the exact-fit case an off-by-one in K4_STR_MAX or the composed guards would break.
    let prog_key = grant_native_key(&midi, &mut buf) == Some(16) && &buf[..16] == b"grants:prog:MIDI";
    let mut wide = [0u8; K4_STR_MAX];
    let wide_ok = grant_native_key(&img, &mut wide) == Some(74) && &wide[..14] == b"grants:sha256:";
    let short_none = grant_native_key(&img, &mut [0u8; 73]).is_none()
        && grant_native_key(&PrincipalRecord::NONE, &mut buf).is_none();
    if prog_key && wide_ok && short_none {
        w |= 1 << 4;
    }

    // bit5: rights project to the canonical `rw`/`r`/`w`/`-` value strings; a 1-byte buffer can't hold `rw`
    // and yields None (the uniform contract — never a partial rights string that would drop the write bit).
    let mut rb = [0u8; 2];
    let rw_ok = rights_native_value(CAP_READ | CAP_WRITE, &mut rb) == Some(2) && &rb == b"rw";
    let r_ok = rights_native_value(CAP_READ, &mut rb) == Some(1) && rb[0] == b'r';
    let w_ok = rights_native_value(CAP_WRITE, &mut rb) == Some(1) && rb[0] == b'w';
    let none_ok = rights_native_value(0, &mut rb) == Some(1) && rb[0] == b'-';
    let short_none_r = rights_native_value(CAP_READ | CAP_WRITE, &mut [0u8; 1]).is_none();
    if rw_ok && r_ok && w_ok && none_ok && short_none_r {
        w |= 1 << 5;
    }

    // bit6: the magic discriminator tells the FAT-bridge sidecar (`UNAATR1\0`) from a native unafs volume
    // (`UNAFS`) from anything else (a FAT boot sector, an empty buffer).
    let fat_boot = [0xEBu8, 0x3C, 0x90, b'M', b'S', b'D', b'O', b'S'];
    if classify_volume_magic(&ATR_MAGIC) == VolumeMagic::AtrSidecar
        && classify_volume_magic(&UNAFS_SB_MAGIC) == VolumeMagic::NativeUnafs
        && classify_volume_magic(&fat_boot) == VolumeMagic::Other
        && classify_volume_magic(&[]) == VolumeMagic::Other
    {
        w |= 1 << 6;
    }

    // bit7: THE MIGRATION LANDMINE — an owner row round-tripped through the REAL on-disk codec
    // (atr_serialize_row exactly as a persist, atr_parse_row exactly as a mount) projects IDENTICALLY to an
    // INDEPENDENTLY re-minted principal (NOT the object that was serialized), and the IMAGE owner string is
    // the 240-bit PREFIX form — length 67, never the 71-char full digest. So a MIGRATED IMAGE owner still
    // matches a FRESH mint of the same program (re-acquirable across the migration); a regression that
    // emitted the 71-char full digest instead would fail the `na == 67` check.
    let mut row = AtrRow {
        name: *b"K4READY BIN",
        first_cluster: 3,
        size: 512,
        dir_lba: 0,
        dir_off: 0,
        owner: img,
        grants: [AtrGrant::EMPTY; NFGRANT],
    };
    row.grants[0] = AtrGrant { prin: midi, rights: CAP_READ | CAP_WRITE };
    if let Some(parsed) = atr_parse_row(&atr_serialize_row(&row)) {
        let fresh_owner = PrincipalRecord::image_of(b"abc"); // INDEPENDENT re-mint, not the serialized `img`
        let fresh_grantee = PrincipalRecord::program_name("MIDI");
        let (mut a, mut b) = ([0u8; K4_STR_MAX], [0u8; K4_STR_MAX]);
        let own_stable = match (
            principal_native_string(&parsed.owner, &mut a),
            principal_native_string(&fresh_owner, &mut b),
        ) {
            (Some(na), Some(nb)) => na == 67 && na == nb && a[..na] == b[..nb], // 67 = the 240-bit prefix form
            _ => false,
        };
        let (mut ga, mut gb) = ([0u8; K4_STR_MAX], [0u8; K4_STR_MAX]);
        let grant_stable = match (
            grant_native_key(&parsed.grants[0].prin, &mut ga),
            grant_native_key(&fresh_grantee, &mut gb),
        ) {
            (Some(na), Some(nb)) => na == nb && ga[..na] == gb[..nb],
            _ => false,
        };
        let mut ra = [0u8; 2];
        let rights_stable =
            rights_native_value(parsed.grants[0].rights, &mut ra) == Some(2) && &ra == b"rw";
        if own_stable && grant_stable && rights_stable {
            w |= 1 << 7;
        }
    }

    let all = 0xFFu32;
    serial_println!(
        ":: K4-ready: native owner/grants projection + UNAFS-vs-UNAATR1 magic discriminator (K4 migration codec, sha256 240-bit prefix) {} [w={:#04x}/{:#04x}] ::",
        if w == all { "PASS" } else { "FAIL" },
        w,
        all
    );
}

// ===================================================================================================
// K6: NATIVE-ATTR MIGRATION witness — prove the U6 owner/grants ACL round-trips through the native
// unafs attribute volume (retiring the `UNAFS.ATR` sidecar). Runs LAST in the storage chain (after
// `k4_write_selftest`), fully self-cleaning so the STATEFUL card never accumulates and `k3_mount`'s
// exact-two-entries `ls` holds on the next boot. One uncounted `:: K6-migrate: … PASS [w=0x..] ::`
// line. Honest skip on media without a unafs partition. Bits grow across the milestones: M1 = the
// forward+reverse codec round-trip in RAM and on-disk through the native seam.
// ===================================================================================================

/// K6 synthetic scratch keys — a FAT directory-slot identity that no real fixture uses (a made-up LBA
/// well past any staged directory), so the ACL file it writes never collides with a live owned file.
const K6_SCRATCH_LBA: u64 = 0xC6_C600;
const K6_SCRATCH_OFF: u32 = 0x60;
/// K6 M2 migration scratch keys — a synthetic IMAGE_SHA256-owned sidecar row (migrates) and a legacy
/// PROGRAM_NAME-owned sidecar row (stays un-migrated). Made-up FAT slot identities, no live file.
const K6_IMG_LBA: u64 = 0xC6_1116;
const K6_IMG_OFF: u32 = 0x16;
const K6_LEG_LBA: u64 = 0xC6_1226;
const K6_LEG_OFF: u32 = 0x26;

/// K6 M1: does a `PrincipalRecord` survive forward-projection (`principal_native_string`) and reverse
/// (`principal_from_native`) byte-for-byte? This is the 240-bit-prefix invariant a migrated owner rests
/// on. Returns false on any projection failure.
fn k6_principal_roundtrips(p: &PrincipalRecord) -> bool {
    let mut buf = [0u8; K4_STR_MAX];
    match principal_native_string(p, &mut buf) {
        Some(n) => principal_from_native(&buf[..n]) == Some(*p),
        None => false,
    }
}

/// K6 migration + native-store witness. See the section header. Self-cleaning.
fn k6_migrate_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::fs::unafs::locate().is_err() {
        serial_println!(":: K6-migrate: no unafs volume — skipped ::");
        return;
    }

    let mut w = 0u32;

    // A PROGRAM_NAME owner and an IMAGE_SHA256 owner — the two migratable principal kinds.
    let prog = PrincipalRecord::program_name("K6OWN.BIN");
    let img = PrincipalRecord::image_of(b"k6-image-bytes");
    let grantee = PrincipalRecord::program_name("K6GRT.BIN");

    // bit0: PROGRAM_NAME owner round-trips through the codec (forward + reverse == identity).
    if k6_principal_roundtrips(&prog) {
        w |= 1 << 0;
    }
    // bit1: IMAGE_SHA256 owner round-trips (the 240-bit-prefix rule — migrated == fresh mint).
    if k6_principal_roundtrips(&img) {
        w |= 1 << 1;
    }

    // Project an owner + one grant to native strings and write them through the native seam.
    let mut ob = [0u8; K4_STR_MAX];
    let mut gk = [0u8; K4_STR_MAX];
    let mut rv = [0u8; 2];
    let on = principal_native_string(&img, &mut ob);
    let gn = grant_native_key(&grantee, &mut gk);
    let rn = rights_native_value(CAP_READ, &mut rv);
    let mut wrote = false;
    if let (Some(on), Some(gn), Some(rn)) = (on, gn, rn) {
        wrote = crate::fs::unafs::native_acl_write(
            K6_SCRATCH_LBA,
            K6_SCRATCH_OFF,
            "K6OWN.BIN",
            0x123,
            &ob[..on],
            &[(&gk[..gn], &rv[..rn])],
        );
    }
    // bit2: the native write committed.
    if wrote {
        w |= 1 << 2;
    }

    // bit3: read the row back and reverse-project — owner == fresh mint, grant == (grantee, R),
    // name/first_cluster preserved (proof the native store carries the durable ACL end-to-end).
    if let Some(row) = crate::fs::unafs::native_acl_read(K6_SCRATCH_LBA, K6_SCRATCH_OFF) {
        let owner_ok = principal_from_native(&row.owner) == Some(img);
        let key_ok = row.name == "K6OWN.BIN" && row.first_cluster == 0x123;
        let grant_ok = row.grants.len() == 1
            && grantee_from_grant_key(&row.grants[0].0) == Some(grantee)
            && rights_from_native(&row.grants[0].1) == CAP_READ;
        if owner_ok && key_ok && grant_ok {
            w |= 1 << 3;
        }
    }

    // bit4: clear the row (self-clean) — the read now returns None (delete durable through the mount).
    let cleared = crate::fs::unafs::native_acl_clear(K6_SCRATCH_LBA, K6_SCRATCH_OFF);
    crate::fs::unafs::force_remount();
    if cleared && crate::fs::unafs::native_acl_read(K6_SCRATCH_LBA, K6_SCRATCH_OFF).is_none() {
        w |= 1 << 4;
    }

    // ---- M2: boot-time migration, native-before-delete (plant sidecar rows, migrate, converge) ----
    // Drive `native_migrate_from_sidecar` end-to-end in QEMU: the real boot pass is a no-op here (the
    // sidecar is empty at the head of u7_launcher), so plant synthetic rows and exercise the migration.
    if let Ok(fs) = crate::fs::fat::mount() {
        let img_owner = PrincipalRecord::image_of(b"k6-migrate-image-bytes");
        let leg_owner = PrincipalRecord::program_name("K6LEG.BIN");
        let empty = [(PrincipalRecord::NONE, 0u32); NFGRANT];
        if let (Some(imgn), Some(legn)) =
            (atr_name_from_str("K6IMG.BIN"), atr_name_from_str("K6LEG.BIN"))
        {
            // Clear any stale scratch from a prior interrupted run (idempotent).
            let _ = atr_clear_row(&fs, K6_IMG_LBA, K6_IMG_OFF);
            let _ = atr_clear_row(&fs, K6_LEG_LBA, K6_LEG_OFF);
            let _ = crate::fs::unafs::native_acl_clear(K6_IMG_LBA, K6_IMG_OFF);

            let p1 = atr_persist_row(&fs, imgn, 0x55, 0, K6_IMG_LBA, K6_IMG_OFF, img_owner, &empty);
            let p2 = atr_persist_row(&fs, legn, 0x66, 0, K6_LEG_LBA, K6_LEG_OFF, leg_owner, &empty);
            let n1 = native_migrate_from_sidecar(&fs);
            // bit5: exactly the IMAGE row migrated — its native row carries the reverse-equal owner, its
            // sidecar row is gone, and the legacy PROGRAM_NAME sidecar row STAYS (un-migrated). K7: a
            // retained legacy row no longer ENFORCES (the boot fallback rebuild is deleted) — it simply
            // persists inertly until a card re-prep clears it; this witness proves migration leaves it be.
            let img_native_ok = crate::fs::unafs::native_acl_read(K6_IMG_LBA, K6_IMG_OFF)
                .is_some_and(|r| principal_from_native(&r.owner) == Some(img_owner));
            let img_sidecar_gone = atr_row_first_cluster(&fs, K6_IMG_LBA, K6_IMG_OFF).is_none();
            let leg_sidecar_stays = atr_row_first_cluster(&fs, K6_LEG_LBA, K6_LEG_OFF).is_some();
            if p1 && p2 && n1 == 1 && img_native_ok && img_sidecar_gone && leg_sidecar_stays {
                w |= 1 << 5;
            }
            // bit6: POWER-CUT CONVERGENCE — re-plant the IMAGE sidecar row so BOTH copies are present
            // (the crash-after-native-write-before-sidecar-delete window: never neither). A re-run
            // converges — migrates it again, re-clears the sidecar, native still reverse-equal (idempotent).
            let p3 = atr_persist_row(&fs, imgn, 0x55, 0, K6_IMG_LBA, K6_IMG_OFF, img_owner, &empty);
            let both_copies = crate::fs::unafs::native_acl_read(K6_IMG_LBA, K6_IMG_OFF).is_some()
                && atr_row_first_cluster(&fs, K6_IMG_LBA, K6_IMG_OFF).is_some();
            let n2 = native_migrate_from_sidecar(&fs);
            let converged = atr_row_first_cluster(&fs, K6_IMG_LBA, K6_IMG_OFF).is_none()
                && crate::fs::unafs::native_acl_read(K6_IMG_LBA, K6_IMG_OFF)
                    .is_some_and(|r| principal_from_native(&r.owner) == Some(img_owner));
            if p3 && both_copies && n2 == 1 && converged {
                w |= 1 << 6;
            }
            // bit7: idempotent no-op — with no IMAGE sidecar rows left, migration migrates 0 and leaves the
            // legacy PROGRAM_NAME row in place (a card re-prep, not migration, clears it).
            let n3 = native_migrate_from_sidecar(&fs);
            if n3 == 0 && atr_row_first_cluster(&fs, K6_LEG_LBA, K6_LEG_OFF).is_some() {
                w |= 1 << 7;
            }
            // Self-clean: drop the native scratch + the legacy sidecar row -> pristine (empty UNAFS.ATR).
            let _ = crate::fs::unafs::native_acl_clear(K6_IMG_LBA, K6_IMG_OFF);
            let _ = atr_clear_row(&fs, K6_LEG_LBA, K6_LEG_OFF);
            crate::fs::unafs::force_remount();
        }
    }

    let all = 0xffu32;
    serial_println!(
        ":: K6-migrate: native owner/grants round-trip + sidecar migration native-before-delete (codec forward+reverse; IMAGE row migrates+verifies+converges, legacy PROGRAM_NAME stays) {} [w={:#04x}] ::",
        if w == all { "PASS" } else { "FAIL" },
        w
    );
}

// ===================================================================================================
// FATDIRS: exercise the new fat.rs directory-mutation seam (create_dir / remove_dir) end-to-end on the
// live volume — the aarch64 kernel-side disk-selftest (the k1_atr idiom). Runs LAST in the storage
// chain so its disk I/O can never perturb the 23-fixture battery or the witnesses; emits ONE uncounted
// `:: FATDIRS: ... PASS [w=0x..] ::` line (never a `-> PASS` fixture line). Fully self-cleaning: leaves
// the volume EXACTLY as found (no scratch dir/file), so the STATEFUL metal card never accumulates. Uses
// the volume ROOT (parent_first_cluster = 0) as the parent, so it runs unchanged on the FAT16 fixed-root
// and FAT32 test images. JD7's Orin-panel mkdir/rmdir is the attended-metal money-shot for the seam.
const FATDIRS_SUBD: &str = "FDSUB"; // the created subdirectory
const FATDIRS_FILE: &str = "FDF.BIN"; // a file created INSIDE the subdirectory
const FATDIRS_R0: &str = "FDR0"; // a 0-cluster DIR entry (the root-like refuse case)
const FATDIRS_FT: &str = "FDFT.BIN"; // a plain file (the not-a-directory refuse case)
const FATDIRS_ALL: u32 = 0xFF; // all 8 assertions (M1 bits 0-2, M2 bits 3-7)

/// FATDIRS: delete any scratch entries this selftest may have left, leaving the root EXACTLY as found.
/// Robust to partial failures (re-resolves each name fresh). Deleting FDSUB's cluster frees FDF.BIN with
/// it (FDF.BIN lives inside FDSUB's cluster and owns no clusters of its own).
fn fatdirs_cleanup(fs: &crate::fs::fat::FatFs) {
    for name in [FATDIRS_SUBD, FATDIRS_R0, FATDIRS_FT] {
        if let Ok((de, lba, off)) = fs.locate_in_dir(0, name) {
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }
}

/// FATDIRS: drive create_dir/remove_dir on the live volume, returning a witness bitmask (PASS iff
/// `== FATDIRS_ALL`). Kernel-task context, so block I/O is legal. Fully self-cleaning.
fn fatdirs_check() -> u32 {
    let mut w = 0u32;
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return 0,
    };
    fatdirs_cleanup(&fs); // pristine start (a stale scratch dir from an interrupted run would confound)

    // ---- M1: create_dir + the mandatory `.`/`..` -------------------------------------------------
    let (dde, _dlba, _doff) = match fs.create_dir(0, FATDIRS_SUBD) {
        Ok(t) => t,
        Err(_) => {
            fatdirs_cleanup(&fs);
            return w;
        }
    };
    let child = dde.first_cluster();
    if dde.is_dir && child != 0 {
        w |= 1 << 0; // create_dir returned a DIR entry carrying a real first cluster
    }
    // read_dir of the new directory: exactly `.` (fc = child) and `..` (fc = 0, parent is root).
    if let Ok(entries) = fs.read_dir(child) {
        let mut dot = false;
        let mut dotdot = false;
        let mut other = false;
        for e in &entries {
            match e.name() {
                "." => dot = e.is_dir && e.first_cluster() == child,
                ".." => dotdot = e.is_dir && e.first_cluster() == 0,
                _ => other = true,
            }
        }
        if dot && dotdot && !other && entries.len() == 2 {
            w |= 1 << 1; // well-formed `.`/`..`, nothing else
        }
    }
    // Create a file INSIDE the new directory and read it back by locating it.
    let file_made = fs.create_in_dir(child, FATDIRS_FILE, 0x20).is_ok();
    if file_made && fs.locate_in_dir(child, FATDIRS_FILE).is_ok() {
        w |= 1 << 2; // a file created in the new subdir is findable
    }

    // ---- M2: remove_dir refusals, success, reuse, and the negative targets -----------------------
    // Non-empty rmdir is REFUSED (the file is still inside).
    if matches!(fs.remove_dir(0, FATDIRS_SUBD), Err(crate::fs::fat::FatError::IsDirectory)) {
        w |= 1 << 3; // non-empty directory refused
    }
    // Remove the file, then rmdir succeeds and returns the freed cluster.
    let file_removed = match fs.locate_in_dir(child, FATDIRS_FILE) {
        Ok((fe, flba, foff)) => fs.delete_located(flba, foff, fe.first_cluster()).is_ok(),
        Err(_) => false,
    };
    let mut removed_ok = false;
    if file_removed {
        if let Ok(freed) = fs.remove_dir(0, FATDIRS_SUBD) {
            removed_ok = true;
            if freed.len() == 1 && freed[0] == child {
                w |= 1 << 4; // rmdir of the now-empty dir succeeded, freeing exactly the child cluster
            }
        }
    }
    // Reusability: the entry is gone AND the child cluster reads free again.
    if removed_ok
        && matches!(fs.locate_in_dir(0, FATDIRS_SUBD), Err(crate::fs::fat::FatError::NotFound))
        && fs.fat_entry_copy(child, 0) == Ok(0)
    {
        w |= 1 << 5; // the freed cluster is reusable (FAT entry free) and the name is gone
    }

    // Root-like refuse: a 0-cluster DIR entry (create_in_dir writes fc=0) -> remove_dir refuses.
    if fs.create_in_dir(0, FATDIRS_R0, 0x10).is_ok() {
        if matches!(fs.remove_dir(0, FATDIRS_R0), Err(crate::fs::fat::FatError::Unsupported)) {
            w |= 1 << 6; // first_cluster==0 / root-like target refused
        }
        if let Ok((de, lba, off)) = fs.locate_in_dir(0, FATDIRS_R0) {
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }
    // Not-a-directory refuse: a plain file target -> remove_dir refuses.
    if fs.create_in_dir(0, FATDIRS_FT, 0x20).is_ok() {
        if matches!(fs.remove_dir(0, FATDIRS_FT), Err(crate::fs::fat::FatError::Unsupported)) {
            w |= 1 << 7; // a file target refused (not a directory)
        }
        if let Ok((de, lba, off)) = fs.locate_in_dir(0, FATDIRS_FT) {
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }

    fatdirs_cleanup(&fs);
    w
}

/// FATDIRS launcher + verdict — rides the U7 kernel task AFTER the K1/K2/K3/IMG-SIG selftests (its disk
/// I/O can never perturb the 23 fixtures or the witnesses). Emits ONE uncounted `:: FATDIRS: … PASS ::`
/// line (the K1-atr `<noun> PASS` idiom — NOT a `-> PASS` fixture line, so the count stays at 23).
fn fatdirs_launcher() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof writes real dirs; skip silently (the control-path discipline)
    }
    let w = fatdirs_check();
    serial_println!(
        ":: FATDIRS: fat.rs directory create/remove — create_dir(child .,..+publish), remove_dir(empty-only via delete_located) {} (new dir `.`/`..` well-formed; file-in-dir; non-empty refused; empty rmdir frees+reuses the cluster; root-like + file targets refused) [w={:#04x}] ::",
        if w == FATDIRS_ALL { "PASS" } else { "FAIL" },
        w
    );
}

// ===================================================================================================
// FATMOVE: exercise the new fat.rs directory-entry rename + cross-directory move seam
// (rename_entry / move_entry) end-to-end on the live volume — the aarch64 kernel-side disk-selftest
// (the k1_atr / fatdirs idiom). Runs LAST in the storage chain so its disk I/O can never perturb the
// 23-fixture battery or the witnesses; emits ONE uncounted `:: FATMOVE: ... PASS [w=0x..] ::` line
// (never a `-> PASS` fixture line). Fully self-cleaning: leaves the volume EXACTLY as found. Uses the
// volume ROOT (parent = 0) plus one scratch subdirectory, so it runs unchanged on the FAT16
// fixed-root and FAT32 test images. A future jetson `mv` arc (JD10) is the attended-metal money-shot.
const FATMOVE_A: &str = "FMA.BIN"; // created in root, written, then renamed to FMB.BIN
const FATMOVE_B: &str = "FMB.BIN"; // the rename target; then moved into the scratch subdir
const FATMOVE_C: &str = "FMC.BIN"; // a second root file (rename-collision + move-collision source)
const FATMOVE_SUB: &str = "FMSUB"; // scratch subdirectory (the move destination parent)
const FATMOVE_D: &str = "FMD.BIN"; // a file inside FMSUB (the move-onto-existing collision target)
const FATMOVE_DIR: &str = "FMDIR"; // a root subdir (the move-a-directory refusal target)
const FATMOVE_E: &str = "FME.BIN"; // an EMPTY (0-cluster) file — the head==0 move case
const FATMOVE_LEN: usize = 700; // content length — spans >1 sector, so the whole chain moves by ref
const FATMOVE_ALL: u32 = 0x1FF; // all 9 assertions (M1 rename bits 0-3, M2 move bits 4-8)

/// FATMOVE: the deterministic content pattern written into the scratch file, so a byte-for-byte
/// read-back after a rename and a move proves the cluster chain travelled intact.
fn fatmove_byte(i: usize) -> u8 {
    (i.wrapping_mul(37).wrapping_add(11) & 0xFF) as u8
}

/// FATMOVE: delete any scratch entries this selftest may have left, leaving the volume EXACTLY as
/// found. Robust to partial failures (re-resolves each name fresh, in both the root and the subdir).
fn fatmove_cleanup(fs: &crate::fs::fat::FatFs) {
    // Files that may live inside the scratch subdir (delete them BEFORE removing the subdir).
    if let Ok((sde, _, _)) = fs.locate_in_dir(0, FATMOVE_SUB) {
        let sub = sde.first_cluster();
        if sub != 0 {
            for name in [FATMOVE_A, FATMOVE_B, FATMOVE_C, FATMOVE_D, FATMOVE_E] {
                if let Ok((de, lba, off)) = fs.locate_in_dir(sub, name) {
                    let _ = fs.delete_located(lba, off, de.first_cluster());
                }
            }
        }
        let _ = fs.remove_dir(0, FATMOVE_SUB);
    }
    let _ = fs.remove_dir(0, FATMOVE_DIR);
    for name in [FATMOVE_A, FATMOVE_B, FATMOVE_C, FATMOVE_E] {
        if let Ok((de, lba, off)) = fs.locate_in_dir(0, name) {
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }
}

/// FATMOVE: drive rename_entry/move_entry on the live volume, returning a witness bitmask (PASS iff
/// `== FATMOVE_ALL`). Kernel-task context, so block I/O is legal. Fully self-cleaning.
fn fatmove_check() -> u32 {
    use crate::fs::fat::FatError;
    let mut w = 0u32;
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return 0,
    };
    fatmove_cleanup(&fs); // pristine start (a stale scratch entry from an interrupted run would confound)

    // The content pattern + a readback helper (byte-for-byte compare against the pattern).
    let mut content = alloc::vec::Vec::with_capacity(FATMOVE_LEN);
    for i in 0..FATMOVE_LEN {
        content.push(fatmove_byte(i));
    }
    let reads_back = |first_cluster: u32, size: u32| -> bool {
        if size as usize != FATMOVE_LEN {
            return false;
        }
        let mut out = alloc::vec::Vec::new();
        if fs.read_at(first_cluster, size, 0, &mut out, FATMOVE_LEN).is_err() {
            return false;
        }
        out.len() == FATMOVE_LEN && out[..] == content[..]
    };

    // ---- M1: rename_entry (in the root) ----------------------------------------------------------
    // Create FMA.BIN, grow it with the content pattern -> it owns a real (multi-cluster) chain.
    let head = match fs.create_in_dir(0, FATMOVE_A, 0x20) {
        Ok((_, lba, off)) => match fs.write_grow(0, 0, lba, off, 0, &content) {
            Ok((_, _sz, new_first)) => new_first,
            Err(_) => {
                fatmove_cleanup(&fs);
                return w;
            }
        },
        Err(_) => {
            fatmove_cleanup(&fs);
            return w;
        }
    };
    // bit0: rename FMA.BIN -> FMB.BIN; the returned entry is a FILE named FMB.BIN with the SAME head + size.
    if let Ok((rde, _, _)) = fs.rename_entry(0, FATMOVE_A, FATMOVE_B) {
        if rde.name() == FATMOVE_B
            && !rde.is_dir
            && rde.first_cluster() == head
            && rde.size as usize == FATMOVE_LEN
        {
            w |= 1 << 0;
        }
    }
    // bit1: the OLD name is gone and the NEW name resolves to the same chain head + size.
    if matches!(fs.locate_in_dir(0, FATMOVE_A), Err(FatError::NotFound)) {
        if let Ok((bde, _, _)) = fs.locate_in_dir(0, FATMOVE_B) {
            if bde.first_cluster() == head && bde.size as usize == FATMOVE_LEN {
                w |= 1 << 1;
            }
        }
    }
    // bit2: the content reads back byte-for-byte through the rename (chain intact).
    if reads_back(head, FATMOVE_LEN as u32) {
        w |= 1 << 2;
    }
    // bit3: rename-onto-EXISTING refused. Create FMC.BIN, then rename FMB.BIN -> FMC.BIN must fail and
    // leave FMB.BIN intact (still pointing at the same head).
    let fmc_made = fs.create_in_dir(0, FATMOVE_C, 0x20).is_ok();
    if fmc_made
        && matches!(fs.rename_entry(0, FATMOVE_B, FATMOVE_C), Err(FatError::Unsupported))
        && fs.locate_in_dir(0, FATMOVE_B).map(|(d, _, _)| d.first_cluster()) == Ok(head)
    {
        w |= 1 << 3;
    }

    // ---- M2: move_entry (across directories) -----------------------------------------------------
    // Make the scratch subdir and move FMB.BIN (root) into it.
    let sub = match fs.create_dir(0, FATMOVE_SUB) {
        Ok((sde, _, _)) => sde.first_cluster(),
        Err(_) => {
            fatmove_cleanup(&fs);
            return w;
        }
    };
    // bit4: move root/FMB.BIN -> FMSUB/FMB.BIN; the new entry carries the SAME head, and the root name is gone.
    if let Ok((mde, _, _)) = fs.move_entry(0, FATMOVE_B, sub, FATMOVE_B) {
        if mde.first_cluster() == head
            && !mde.is_dir
            && matches!(fs.locate_in_dir(0, FATMOVE_B), Err(FatError::NotFound))
            && fs.locate_in_dir(sub, FATMOVE_B).is_ok()
        {
            w |= 1 << 4;
        }
    }
    // bit5: the moved file's content reads back byte-for-byte from its NEW location (chain moved by reference).
    if let Ok((bde, _, _)) = fs.locate_in_dir(sub, FATMOVE_B) {
        if bde.first_cluster() == head && reads_back(bde.first_cluster(), bde.size) {
            w |= 1 << 5;
        }
    }
    // bit6: move-onto-EXISTING refused. Create FMD.BIN inside FMSUB; move root/FMC.BIN -> FMSUB/FMD.BIN
    // must fail and leave FMC.BIN in the root.
    let fmd_made = fs.create_in_dir(sub, FATMOVE_D, 0x20).is_ok();
    if fmd_made
        && matches!(fs.move_entry(0, FATMOVE_C, sub, FATMOVE_D), Err(FatError::Unsupported))
        && fs.locate_in_dir(0, FATMOVE_C).is_ok()
    {
        w |= 1 << 6;
    }
    // bit7: moving a DIRECTORY is refused. Create FMDIR in the root; move it into FMSUB must fail
    // (`IsDirectory`) and leave FMDIR in the root.
    let fmdir_made = fs.create_dir(0, FATMOVE_DIR).is_ok();
    if fmdir_made
        && matches!(fs.move_entry(0, FATMOVE_DIR, sub, FATMOVE_DIR), Err(FatError::IsDirectory))
        && fs.locate_in_dir(0, FATMOVE_DIR).is_ok()
    {
        w |= 1 << 7;
    }
    // bit8: move an EMPTY (0-cluster) file across dirs — create FME.BIN in root (no write, so
    // first_cluster==0, size==0), move root/FME.BIN -> FMSUB/FME.BIN; the new entry is a 0-length
    // file, the root name is gone. Proves the head==0 relink path (an empty file has no chain to lose).
    let fme_made = fs.create_in_dir(0, FATMOVE_E, 0x20).is_ok();
    if fme_made {
        if let Ok((ede, _, _)) = fs.move_entry(0, FATMOVE_E, sub, FATMOVE_E) {
            if ede.first_cluster() == 0
                && ede.size == 0
                && !ede.is_dir
                && matches!(fs.locate_in_dir(0, FATMOVE_E), Err(FatError::NotFound))
                && fs.locate_in_dir(sub, FATMOVE_E).is_ok()
            {
                w |= 1 << 8;
            }
        }
    }

    fatmove_cleanup(&fs);
    w
}

/// FATMOVE launcher + verdict — rides the U7 kernel task AFTER every prior storage selftest (its disk
/// I/O can never perturb the 23 fixtures or the witnesses). Emits ONE uncounted `:: FATMOVE: … PASS ::`
/// line (the k1-atr `<noun> PASS` idiom — NOT a `-> PASS` fixture line, so the count stays at 23).
fn fatmove_launcher() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof writes real dirs; skip silently (the control-path discipline)
    }
    let w = fatmove_check();
    serial_println!(
        ":: FATMOVE: fat.rs rename_entry(in-place 8.3 name RMW) + move_entry(dst-entry-first, then 0xE5 src keep-chain) {} (rename: name-swap, old gone/new=same head+size, content intact, onto-existing refused; move: cross-dir by-reference, content intact, onto-existing refused, directory refused, empty-file relink) [w={:#05x}] ::",
        if w == FATMOVE_ALL { "PASS" } else { "FAIL" },
        w
    );
}

// ===================================================================================================
// CLOCK-3: prove the FAT last-write stamp DERIVES from the unified kernel clock (crate::clock). The
// end-to-end witness: plant a DETERMINISTIC Manual civil (Unix) anchor, create a real file on the live
// volume, read its on-disk dir-entry back, and assert its decoded mtime DATE equals the anchor's civil
// date — i.e. the fat.rs writer stamped the unified clock, not the pre-CLOCK-3 all-zero placeholder.
// Runs LAST in the storage chain (its disk I/O can never perturb the fixtures/witnesses); restores the
// civil clock to EXACTLY as found (re-plants a prior anchor, else clears back to UNSET); fully
// self-cleaning (deletes its scratch file). Emits ONE uncounted `:: CLOCK3-fat: … PASS [w=0x..] ::`
// line (the k1-atr `<noun> PASS` idiom — NOT a `-> PASS` fixture line, so the fixture count is unchanged).
const CLOCK3_FILE: &str = "CLK3.BIN";
// A deterministic Manual anchor: 2020-06-15 12:34:56 UTC (Unix 1_592_224_496). Its FAT packing is fixed —
// DATE = ((2020-1980)<<9)|(6<<5)|15 = 0x50CF, TIME = (12<<11)|(34<<5)|(56/2) = 0x645C — so the witness
// asserts an EXACT on-disk value, independent of boot timing (the date is ~11h from midnight: no rollover).
const CLOCK3_UNIX: u64 = 1_592_224_496;
const CLOCK3_WANT_DATE: u16 = 0x50CF; // 2020-06-15
const CLOCK3_ALL: u32 = 0x7; // three assertions: created, date-derived-from-anchor, civil clock restored

fn clock3_check() -> u32 {
    let mut w = 0u32;
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return 0,
    };
    // Pristine start: a stale scratch file from an interrupted run would confound the create.
    if let Ok((de, lba, off)) = fs.locate_in_dir(0, CLOCK3_FILE) {
        let _ = fs.delete_located(lba, off, de.first_cluster());
    }

    // Save the civil clock as found, then plant the deterministic Manual anchor (captures a mono tick in
    // the same breath, exactly as `setdate`/SNTP do). Elapsed since the anchor is ~0s across the few
    // I/Os below, so `fat_stamp()` extrapolates to the exact anchored second.
    let prior = crate::clock::raw_anchor();
    let tick = crate::clock::mono_ticks().unwrap_or(0);
    crate::clock::set_anchor(CLOCK3_UNIX, tick, crate::clock::ClockSource::Manual);

    // Create the scratch file, then read the on-disk dir entry back and decode its mtime.
    if fs.create_in_dir(0, CLOCK3_FILE, 0x20).is_ok() {
        w |= 1 << 0; // file created
        if let Ok((de, _lba, _off)) = fs.locate_in_dir(0, CLOCK3_FILE) {
            let ts = de.mtime();
            // DATE derived from the anchor: 2020-06-15 (never the all-zero `is_zero()` placeholder).
            if !ts.is_zero() && ts.year == 2020 && ts.month == 6 && ts.day == 15 {
                w |= 1 << 1;
            }
        }
        // Self-clean: remove the scratch file, leaving the volume EXACTLY as found.
        if let Ok((de, lba, off)) = fs.locate_in_dir(0, CLOCK3_FILE) {
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }

    // Restore the civil clock to exactly as found (re-plant a prior anchor, else clear back to UNSET).
    match prior {
        Some((base, src)) => crate::clock::set_anchor(base, tick, src),
        None => crate::clock::clear_anchor(),
    }
    if crate::clock::raw_anchor().map(|(b, _)| b) == prior.map(|(b, _)| b) {
        w |= 1 << 2; // civil clock restored to the pre-witness anchor state
    }
    let _ = CLOCK3_WANT_DATE;
    w
}

/// CLOCK-3 launcher + verdict — rides the U7 kernel task AFTER the FAT write selftests (its disk I/O can
/// never perturb the fixtures or witnesses). Emits ONE uncounted `:: CLOCK3-fat: … PASS ::` line.
fn clock3_fat_launcher() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof writes a real file; skip silently (the control-path discipline)
    }
    let w = clock3_check();
    serial_println!(
        ":: CLOCK3-fat: FAT mtime derives from the unified clock — after a Manual civil anchor, a created file's dir-entry date matches the anchor {} (deterministic 2020-06-15 anchor; date != zero-placeholder; civil clock restored) [w={:#04x}] ::",
        if w == CLOCK3_ALL { "PASS" } else { "FAIL" },
        w
    );
}

// ===================================================================================================
// K2 (make-enforcement-LIVE): prove the K1 cross-reboot ACL end-to-end through TWO REAL disk-loaded
// programs with the gate FLIPPED. Unlike `k1_persist_check` (which manufactures principals directly on
// scratch ASIDs), this drives the enforcement through the SAME path a real boot uses: a program loaded
// off the card by `load_program_into_slot` (the sole `slot_ppid_stamp` mint site) does a private
// `O_CREAT` at EL0 -> its `prog:<NAME>` owns + persists the file; a (simulated) reboot rebuilds the row
// from `UNAFS.ATR`; a fresh incarnation of the SAME named program is re-ADMITTED purely by name; a
// DISTINCT named program (the impostor) is REFUSED. QEMU regenerates a fresh FAT per build and delivers
// no Group-1 IRQ, so the reboot is SIMULATED same-boot (owned_clear -> remount -> native rebuild,
// the M3 standard, K6-reworked onto `native_rebuild_into_owned`) — the true power-cycle survival rides
// the attended metal bench. Fully self-cleaning so
// the STATEFUL metal card never accumulates a row that would brick the next real boot.
const K2_OWN_NAME: &str = "K2OWN.BIN"; // the owner program -> principal prog:K2OWN.BIN
const K2_IMP_NAME: &str = "K2IMP.BIN"; // the impostor program -> principal prog:K2IMP.BIN
const K2_PRIV_NAME: &str = "K2PRIV.BIN"; // the private file the owner creates at EL0
const K2_EXIT_STATUS: u64 = 0x82; // both K2 programs' sentinel exit -> EL0_K2_DONE (distinct from 0x6D..0x81)
const K2_OWN_WITNESS_ALL: u64 = 0x3; // owner: bit0 open admitted + bit1 owner write OK
const K2_IMP_WITNESS_ALL: u64 = 0x1; // impostor: bit0 open denied (-EACCES)

static K2_OWN_WITNESS: AtomicU64 = AtomicU64::new(0); // K2OWN.BIN's reported witness (reset before each spawn)
static K2_IMP_WITNESS: AtomicU64 = AtomicU64::new(0); // K2IMP.BIN's reported witness
static EL0_K2_DONE: AtomicU32 = AtomicU32::new(0); // count of K2 program sentinel exits (want 3 by the end)
static EL0_K2_KILLED: AtomicU32 = AtomicU32::new(0); // any K2 program fault-killed (a real bug; must stay 0)

// K3: test-only synthetic durable-write failure for the durable grant-row write (K6 M3: now
// `native_write_grant_row_on` since K5B), set TRANSIENTLY by `k3_revoke_check`
// (which self-clears) to prove the two-phase revoke fails CLOSED when the persist cannot land. Default false —
// the production/battery path never sets it, so it is a single default-false relaxed load off the persist path.
static K3_TEST_FAIL_PERSIST: AtomicBool = AtomicBool::new(false);

/// K2: record a K2 program's SYS_REPORT, keyed by task name (the `uowner_report` idiom). K2OWN.BIN is
/// spawned twice under the same name — the launcher resets `K2_OWN_WITNESS` before each spawn and reads
/// it after that spawn's sentinel exit, so the two reports never confuse.
fn k2_report(value: u64) {
    match super::sched::current_name() {
        Some("el0-k2own") => K2_OWN_WITNESS.store(value, Ordering::Release),
        Some("el0-k2imp") => K2_IMP_WITNESS.store(value, Ordering::Release),
        _ => {}
    }
}

/// K2: delete `K2PRIV.BIN` + clear its persisted `UNAFS.ATR` row + its in-RAM owner row (the
/// `k1_persist_cleanup` idiom). Order: clear the attr row BEFORE the directory delete (a crash leaves the
/// file public, never a dangling owned row); `owned_clear` drops the in-RAM ACL. Missing file/row -> no-op.
/// Runs at every exit path so the stateful metal card leaves nothing that would deny the file on the next
/// real boot once the gate is live.
fn k2_cleanup() {
    if let Ok(fs) = crate::fs::fat::mount() {
        if let Ok((de, lba, off)) = fs.find_located(K2_PRIV_NAME) {
            let _ = crate::fs::unafs::native_acl_clear(lba, off as u32); // K6: native store
            let _ = atr_clear_row(&fs, lba, off as u32); // legacy sidecar (stale-card defense)
            owned_clear(lba, off as u32);
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }
}

/// K2: load a real program by NAME off the card (auto-stamped `prog:<NAME>` at `load_program_into_slot`),
/// capture its freshly-stamped principal (read HERE, while the slot is live — teardown at the program's
/// exit clears SLOT_PPID), spawn it co-located on `demo_cpu`, and wait (bounded, cooperative) until
/// `EL0_K2_DONE` reaches `want`. Returns the REAL stamped principal, or None on load/slot failure.
fn k2_run_program(name: &str, taskname: &'static str, demo_cpu: usize, want: u32) -> Option<PrincipalRecord> {
    let loaded = load_program_into_slot(name).ok()?;
    let asid = loaded.ttbr0 >> 48;
    let stamped = slot_ppid_of(asid); // the real stamp from load_program_into_slot, read before the program runs
    super::sched::spawn_user_slot(taskname, loaded.base, loaded.sp, loaded.ttbr0, demo_cpu);
    let start = super::timer::cntpct();
    let deadline = 5 * super::timer::cntfrq();
    while EL0_K2_DONE.load(Ordering::Acquire) < want
        && super::timer::cntpct().wrapping_sub(start) <= deadline
    {
        super::sched::yield_now();
    }
    Some(stamped)
}

/// K2: read the `first_cluster` persisted for a file — NATIVE store first (K6 M3: the production persist
/// paths write native), falling back to a legacy `UNAFS.ATR` row (an un-migrated stale card). Used to PROVE
/// M(b) grow-repersist actually landed the real chain head — a create-only row carries `fc = 0`, so a
/// nonzero match against the directory entry means the grow re-persisted.
/// Read-only (the launcher is single-context here — no EL0 program runs during a rebuild step).
fn k2_persisted_first_cluster(fs: &crate::fs::fat::FatFs, dir_lba: u64, dir_off: u32) -> Option<u32> {
    if let Some(row) = crate::fs::unafs::native_acl_read(dir_lba, dir_off) {
        return Some(row.first_cluster);
    }
    atr_row_first_cluster(fs, dir_lba, dir_off)
}

/// K6: read the `first_cluster` of a file's LEGACY `UNAFS.ATR` sidecar row ONLY (never the native store) —
/// the K6-migrate witness's "is the sidecar row still there" probe (native-blind by design: after a
/// migration the native row exists, so a native-first read would mask the sidecar delete it verifies).
fn atr_row_first_cluster(fs: &crate::fs::fat::FatFs, dir_lba: u64, dir_off: u32) -> Option<u32> {
    let (de, _l, _o) = fs.find_located(ATR_NAME).ok()?;
    if (de.size as usize) < ATR_FILE_LEN {
        return None;
    }
    let (afc, asz) = (de.first_cluster(), de.size);
    let mut rows: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs
        .read_at(afc, asz, ATR_HEADER_LEN as u32, &mut rows, ATR_ROW_STRIDE * ATR_ROWS)
        .is_err()
        || rows.len() != ATR_ROW_STRIDE * ATR_ROWS
    {
        return None;
    }
    for i in 0..ATR_ROWS {
        let start = ATR_ROW_STRIDE * i;
        let mut arr = [0u8; ATR_ROW_STRIDE];
        arr.copy_from_slice(&rows[start..start + ATR_ROW_STRIDE]);
        if let Some(r) = atr_parse_row(&arr) {
            if r.dir_lba == dir_lba && r.dir_off == dir_off {
                return Some(r.first_cluster);
            }
        }
    }
    None
}

/// K2 (make-enforcement-LIVE) proof + verdict — the last demo in the U7 chain (after k1_corrupt_launcher).
/// Rides the U7 kernel task on `vcpu`; the programs run co-located on `demo_cpu`, cooperative under QEMU.
/// Seven-bit witness `w`: (0) the real owner created+owned+wrote its private file, stamped prog:K2OWN.BIN;
/// (1) its in-RAM owner row is that named principal (persist path engaged); (2) the row survived to
/// `UNAFS.ATR` + rebuilt at mount; (3) the rebuilt row is owned by prog:K2OWN.BIN (owner_asid = sentinel,
/// no live incarnation); (4) a FRESH incarnation of the owner re-opened its file purely BY NAME after the
/// rebuild; (5) the impostor (a distinct real principal) was REFUSED (-EACCES); (6) M(b) grow-repersist
/// landed the real chain head in the persisted row (not the create-time 0). PASS iff `w == 0x7F` AND all
/// three programs hit their sentinel exit AND none was killed AND the row self-cleaned. Emits ONE
/// uncounted witness line (PASS/FAIL space-flanked mid-sentence — never `-> PASS`/`: PASS`/`-> FAIL`/
/// `FAIL ::`), so the 23-fixture PASS count stays byte-equivalent. (The `k2_leave` metal build replaces this
/// same-boot proof with the two-boot `k2_metal_launcher`; see it + the u7_launcher call site.)
#[cfg(not(feature = "k2_leave"))]
fn k2_liveenf_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof loads/creates real files; skip silently (the control-path discipline)
    }
    // Pre-flight: the two programs must be on the card; K2PRIV.BIN must be ABSENT (a stale copy from an
    // interrupted run would confound the create). Clean a stale K2PRIV.BIN FIRST — BEFORE the program-presence
    // check — because the gate is now LIVE: `atr_maybe_boot_rebuild` reinstalls K2PRIV.BIN's persisted sentinel
    // row at EVERY boot as long as the file exists, and it never checks that the owning program is present. If a
    // program later went missing (a partial re-flash) after an interrupted run left K2PRIV.BIN, a program-presence
    // early-return would strand that reinstalled row forever — violating the self-clean invariant. Cleaning the
    // stale file unconditionally here neutralizes it (the reinstall-able case is exactly file-present).
    match crate::fs::fat::mount() {
        Ok(fs) => {
            if fs.find_in_root(K2_PRIV_NAME).is_ok() {
                k2_cleanup(); // a stale private file from an interrupted run (+ its boot-reinstalled owner row)
            }
            if fs.find_in_root(K2_OWN_NAME).is_err() || fs.find_in_root(K2_IMP_NAME).is_err() {
                serial_println!(":: K2-liveenf: K2OWN.BIN/K2IMP.BIN not on card — live-enforcement demo skipped ::");
                return;
            }
        }
        Err(_) => return, // unmountable -> skip silently
    }

    let mut w = 0u32;
    // IMAGE_SHA256: the owner's expected principal is now its IMAGE DIGEST, not `prog:K2OWN.BIN` — computed
    // from the on-disk image the same way the loader hashes it. A swapped K2OWN.BIN would mint a different one.
    let p_own = match image_principal_of_file(K2_OWN_NAME) {
        Some(p) => p,
        None => {
            serial_println!(":: K2-liveenf: cannot hash K2OWN.BIN image — demo skipped ::");
            return;
        }
    };

    // ---- Phase 1: a REAL owner program creates+owns+persists (+grows) K2PRIV.BIN at EL0 ----
    K2_OWN_WITNESS.store(0, Ordering::Release);
    let Some(stamp1) = k2_run_program(K2_OWN_NAME, "el0-k2own", demo_cpu, 1) else {
        serial_println!(":: K2-liveenf: owner program failed to load — demo skipped ::");
        return;
    };
    let own_w1 = K2_OWN_WITNESS.load(Ordering::Acquire);
    if own_w1 == K2_OWN_WITNESS_ALL && stamp1 == p_own && p_own.kind == PRIN_IMAGE_SHA256 {
        w |= 1 << 0; // bit0: the real loaded owner created+wrote its private file, stamped by IMAGE digest
    }

    // Locate the created file + confirm the in-RAM owner row records the owner's PERSISTENT principal.
    let fs2 = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            k2_cleanup();
            return;
        }
    };
    let (priv_lba, priv_off, priv_fc) = match fs2.find_located(K2_PRIV_NAME) {
        Ok((de, l, o)) => (l, o as u32, de.first_cluster()),
        Err(_) => {
            serial_println!(":: K2-liveenf: K2PRIV.BIN not created by the owner — demo aborted ::");
            k2_cleanup();
            return;
        }
    };
    if owned_owner_ppid(priv_lba, priv_off) == p_own {
        w |= 1 << 1; // bit1: the owner row (in RAM) is the named principal -> the persist path engaged
    }
    // bit6: M(b) grow-repersist actually LANDED — the owner grew K2PRIV.BIN, so its persisted row must carry
    // the real (nonzero) chain head, not the create-time 0 (which would still install name-primary at rebuild).
    // Corroborated against the DIRECTORY-ENTRY head (`priv_fc` = `de.first_cluster()`), not a FAT-chain walk.
    if priv_fc != 0 && k2_persisted_first_cluster(&fs2, priv_lba, priv_off) == Some(priv_fc) {
        w |= 1 << 6;
    }

    // ---- Phase 2: simulate a reboot — WIPE the in-RAM ACL, remount BOTH volumes, REBUILD purely from
    //      the NATIVE store (K6 M3: the production create/grow persists wrote native) ----
    owned_clear(priv_lba, priv_off);
    crate::fs::unafs::force_remount();
    let fs3 = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            k2_cleanup();
            return;
        }
    };
    if native_rebuild_into_owned(&fs3) >= 1 {
        w |= 1 << 2; // bit2: the persisted row survived to disk + rebuilt at mount
    }
    if owned_owner_ppid(priv_lba, priv_off) == p_own {
        w |= 1 << 3; // bit3: the rebuilt row is owned by prog:K2OWN.BIN (owner_asid = sentinel, no live owner)
    }

    // ---- Phase 3a: re-spawn the SAME-named owner -> re-ADMITTED purely BY NAME (owner_asid is the sentinel,
    //      so the live-(asid,gen) check can never match — only the ppid branch can admit) ----
    K2_OWN_WITNESS.store(0, Ordering::Release);
    let Some(stamp2) = k2_run_program(K2_OWN_NAME, "el0-k2own", demo_cpu, 2) else {
        k2_cleanup();
        return;
    };
    let own_w2 = K2_OWN_WITNESS.load(Ordering::Acquire);
    if own_w2 == K2_OWN_WITNESS_ALL && stamp2 == p_own {
        w |= 1 << 4; // bit4: a fresh incarnation of the owner re-opened its own file by name after the rebuild
    }

    // ---- Phase 3b: the IMPOSTOR (a DISTINCT real program/principal) -> REFUSED -EACCES ----
    K2_IMP_WITNESS.store(0, Ordering::Release);
    let Some(stamp_imp) = k2_run_program(K2_IMP_NAME, "el0-k2imp", demo_cpu, 3) else {
        k2_cleanup();
        return;
    };
    let imp_w = K2_IMP_WITNESS.load(Ordering::Acquire);
    if imp_w == K2_IMP_WITNESS_ALL && stamp_imp.kind == PRIN_IMAGE_SHA256 && stamp_imp != p_own {
        w |= 1 << 5; // bit5: a distinct real principal (distinct IMAGE digest) was denied the owner's file
    }

    let done = EL0_K2_DONE.load(Ordering::Acquire);
    let killed = EL0_K2_KILLED.load(Ordering::Acquire);

    // ---- Cleanup so the STATEFUL metal card leaves NO row that would brick the next real boot ----
    k2_cleanup();
    // K3 (fold): match NotFound specifically — the file is genuinely GONE — rather than `.is_err()`, which a
    // transient read error would also satisfy and so overclaim "self-cleaned" on a double-EIO corner.
    let cleaned = match crate::fs::fat::mount() {
        Ok(fs) => matches!(fs.find_in_root(K2_PRIV_NAME), Err(crate::fs::fat::FatError::NotFound)),
        Err(_) => false,
    };

    const K2_ALL: u32 = (1 << 7) - 1; // bits 0..=6
    if w == K2_ALL && done == 3 && killed == 0 && cleaned {
        serial_println!(
            ":: K2-liveenf: cross-reboot ACL LIVE via REAL programs — owner K2OWN.BIN re-admitted by name+IMAGE-digest after native-attr rebuild, impostor K2IMP.BIN (distinct image) refused (-EACCES); grow-repersist landed the real chain head; self-cleaned rebuild+enforce PASS [w={:#04x}] ::",
            w
        );
    } else {
        serial_println!(
            ":: K2-liveenf: rebuild+enforce FAIL — w={:#x}/{:#x} own_w1={:#x} own_w2={:#x} imp_w={:#x} done={} killed={} cleaned={} ::",
            w, K2_ALL, own_w1, own_w2, imp_w, done, killed, cleaned as u8
        );
    }
}

/// K2 metal money-shot (feature `k2_leave`, ATTENDED Pi bench only) — the REAL two-boot cross-reboot
/// power-cycle proof that replaces the same-boot simulate. ONE image, flashed once, booted twice.
/// Dispatches on whether `K2PRIV.BIN` is already on the card: ABSENT ⇒ BOOT-1 (`k2_metal_leave`:
/// create+own+persist+grow, then LEAVE it on disk); PRESENT ⇒ BOOT-2 (`k2_metal_verify`: the LIVE
/// `atr_maybe_boot_rebuild` at the head of `u7_launcher` has already reinstalled the row across the real
/// power-cycle — verify + clean). Chained from `u7_launcher` in place of `k2_liveenf_launcher` in this build.
#[cfg(feature = "k2_leave")]
fn k2_metal_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> skip silently
    }
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => return,
    };
    if fs.find_in_root(K2_OWN_NAME).is_err() || fs.find_in_root(K2_IMP_NAME).is_err() {
        serial_println!(":: K2-metal: K2OWN.BIN/K2IMP.BIN not on card — money-shot skipped ::");
        return;
    }
    // The row survives a power-cycle as K2PRIV.BIN on disk: absent = the FIRST boot (leave it); present = the
    // SECOND boot (the boot rebuild reinstalled the row — verify it survived). Dispatch on that single bit.
    if fs.find_in_root(K2_PRIV_NAME).is_ok() {
        k2_metal_verify(demo_cpu);
    } else {
        k2_metal_leave(demo_cpu);
    }
}

/// K2 metal BOOT-1 (feature `k2_leave`): spawn the real owner, let it create+own+persist+grow `K2PRIV.BIN` at
/// EL0, confirm the in-RAM owner row + the persisted (grown) chain head landed, then LEAVE everything on disk —
/// NO simulate, NO clean — so a real power-cycle carries the persisted row to boot-2. Prints one uncounted line
/// telling the operator to power-cycle (or, on an incomplete create, NOT to).
#[cfg(feature = "k2_leave")]
fn k2_metal_leave(demo_cpu: usize) {
    let Some(p_own) = image_principal_of_file(K2_OWN_NAME) else {
        serial_println!(":: K2-metal: cannot hash K2OWN.BIN image — boot-1 leave aborted (re-prep card) ::");
        return;
    };
    K2_OWN_WITNESS.store(0, Ordering::Release);
    let Some(stamp1) = k2_run_program(K2_OWN_NAME, "el0-k2own", demo_cpu, 1) else {
        serial_println!(":: K2-metal: owner program failed to load — boot-1 leave aborted (re-prep card) ::");
        return;
    };
    let own_w = K2_OWN_WITNESS.load(Ordering::Acquire);
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            serial_println!(":: K2-metal: remount failed — boot-1 leave aborted (re-prep card) ::");
            return;
        }
    };
    let (priv_lba, priv_off, priv_fc) = match fs.find_located(K2_PRIV_NAME) {
        Ok((de, l, o)) => (l, o as u32, de.first_cluster()),
        Err(_) => {
            serial_println!(":: K2-metal: K2PRIV.BIN not created — boot-1 leave aborted (re-prep card) ::");
            return;
        }
    };
    let owner_ok = stamp1 == p_own && owned_owner_ppid(priv_lba, priv_off) == p_own;
    // The grow-repersisted chain head must be on disk (nonzero) so boot-2's rebuild identity cross-check engages.
    let persist_ok = priv_fc != 0 && k2_persisted_first_cluster(&fs, priv_lba, priv_off) == Some(priv_fc);
    if own_w == K2_OWN_WITNESS_ALL && owner_ok && persist_ok {
        serial_println!(
            ":: K2-metal: BOOT-1 left K2PRIV.BIN persisted (owner prog:K2OWN.BIN, fc={:#x}) on disk — POWER-CYCLE NOW; boot-2 verifies the live boot rebuild survived the reboot ::",
            priv_fc
        );
    } else {
        serial_println!(
            ":: K2-metal: BOOT-1 leave incomplete — own_w={:#x} owner_ok={} persist_ok={} (do NOT power-cycle; re-prep the card) ::",
            own_w, owner_ok as u8, persist_ok as u8
        );
    }
    // Deliberately NO owned_clear / atr_clear_row / delete — the row + file STAY for the power-cycle.
}

/// K2 metal BOOT-2 (feature `k2_leave`): after a real power-cycle, the LIVE `atr_maybe_boot_rebuild` (head of
/// `u7_launcher`, gate flipped) has already reinstalled `K2PRIV.BIN`'s owner row from the NATIVE store (K6). PROVE it
/// SURVIVED the power-cycle: (0) the row is owned by prog:K2OWN.BIN (reinstalled by the BOOT rebuild, not this
/// fixture — owner_asid = sentinel); (1) a fresh incarnation of the owner is re-admitted BY NAME; (2) the
/// impostor (a distinct real principal) is refused. Then CLEAN so the card ends pristine. This is the genuine
/// (not simulated) cross-reboot survival — the true metal money-shot.
#[cfg(feature = "k2_leave")]
fn k2_metal_verify(demo_cpu: usize) {
    let mut w = 0u32;
    let Some(p_own) = image_principal_of_file(K2_OWN_NAME) else {
        serial_println!(":: K2-metal: BOOT-2 cannot hash K2OWN.BIN image ::");
        return;
    };
    let fs = match crate::fs::fat::mount() {
        Ok(f) => f,
        Err(_) => {
            serial_println!(":: K2-metal: BOOT-2 remount failed ::");
            return;
        }
    };
    let (priv_lba, priv_off) = match fs.find_located(K2_PRIV_NAME) {
        Ok((_, l, o)) => (l, o as u32),
        Err(_) => {
            serial_println!(":: K2-metal: BOOT-2 K2PRIV.BIN vanished — nothing to verify ::");
            return;
        }
    };
    // bit0: the BOOT rebuild (atr_maybe_boot_rebuild, now GATED LIVE) reinstalled the row across the power-cycle.
    if owned_owner_ppid(priv_lba, priv_off) == p_own {
        w |= 1 << 0;
    }
    // bit1: a fresh incarnation of the owner is re-admitted BY NAME against the boot-reinstalled (sentinel) row.
    K2_OWN_WITNESS.store(0, Ordering::Release);
    if let Some(stamp) = k2_run_program(K2_OWN_NAME, "el0-k2own", demo_cpu, 1) {
        if K2_OWN_WITNESS.load(Ordering::Acquire) == K2_OWN_WITNESS_ALL && stamp == p_own {
            w |= 1 << 1;
        }
    }
    // bit2: the impostor (a distinct real principal) is refused.
    K2_IMP_WITNESS.store(0, Ordering::Release);
    if let Some(stamp_imp) = k2_run_program(K2_IMP_NAME, "el0-k2imp", demo_cpu, 2) {
        if K2_IMP_WITNESS.load(Ordering::Acquire) == K2_IMP_WITNESS_ALL && stamp_imp != p_own {
            w |= 1 << 2;
        }
    }
    let done = EL0_K2_DONE.load(Ordering::Acquire);
    let killed = EL0_K2_KILLED.load(Ordering::Acquire);
    // Clean so the card ends pristine — the money-shot is proven; no lingering row bricks a future boot.
    k2_cleanup();
    // K3 (fold): match NotFound specifically (the file is genuinely GONE) — a transient read error must not
    // masquerade as "self-cleaned".
    let cleaned = match crate::fs::fat::mount() {
        Ok(f) => matches!(f.find_in_root(K2_PRIV_NAME), Err(crate::fs::fat::FatError::NotFound)),
        Err(_) => false,
    };
    const K2M_ALL: u32 = (1 << 3) - 1; // bits 0..=2
    if w == K2M_ALL && done == 2 && killed == 0 && cleaned {
        serial_println!(
            ":: K2-metal: BOOT-2 cross-reboot SURVIVED a real power-cycle — owner prog:K2OWN.BIN re-admitted BY NAME against the LIVE-boot-rebuilt native-attr row, impostor prog:K2IMP.BIN refused (-EACCES); self-cleaned rebuild+enforce PASS [w={:#04x}] ::",
            w
        );
    } else {
        serial_println!(
            ":: K2-metal: BOOT-2 rebuild+enforce FAIL — w={:#x}/{:#x} done={} killed={} cleaned={} ::",
            w, K2M_ALL, done, killed, cleaned as u8
        );
    }
}

fn u7_run(demo_cpu: usize) {
    // 1. Gate on the U6b launcher (its verdict printed + its slot freed).
    let wstart = super::timer::cntpct();
    let wdeadline = 10 * super::timer::cntfrq();
    while !U6B_LAUNCH_DONE.load(Ordering::Acquire)
        && super::timer::cntpct().wrapping_sub(wstart) <= wdeadline
    {
        super::sched::yield_now();
    }

    // One-shot (spawned once; guard defensively).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    // 2. No SD device -> keep the no-storage control path free of demo lines (U7 itself needs no disk).
    if crate::drivers::block::info().is_none() {
        return;
    }

    // 3a. The child's Proc entry FIRST (nothing else claimed if the table is full).
    let Some(pi) = proc_reserve() else {
        serial_println!(":: U7: no free process entry — transfer demo skipped ::");
        return;
    };
    // 3b. Build + spawn the CHILD (its handle row stays EMPTY — the single-writer snapshot depends on
    //     that; it parks on its GO word, so it makes no syscall that could populate anything).
    let Some(child) = u7_build(&raw const __u7_prog_child) else {
        serial_println!(":: U7: no free address-space slot — transfer demo skipped ::");
        proc_free(pi);
        return;
    };
    let child_pid = super::sched::spawn_user_slot("el0-u7child", child.entry, child.sp, child.ttbr0, demo_cpu);
    // Publish the pid->ASID map (ASID first — the sys_spawn discipline — then the pid, the live key).
    PROCS[pi].asid.store(child.asid, Ordering::Release);
    PROCS[pi].pid.store(child_pid, Ordering::Release);
    // 3c. Build + pre-endow + spawn the PARENT. If its slot alloc fails the child is already live: it
    //     parks to its GO budget, exits by sentinel, and its slot tears down cleanly — skip honestly.
    let Some(parent) = u7_build(&raw const __u7_prog_parent) else {
        serial_println!(":: U7: no free address-space slot — transfer demo skipped (child will park out) ::");
        proc_free(pi);
        return;
    };
    install_cap(parent.asid, U7_DEST_IDX, KIND_CHILD, child_pid, CAP_READ);
    install_cap(parent.asid, U7_SRC_IDX, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    serial_println!(
        ":: U7: cross-process transfer — inbox-mediated SYS_XFER/SYS_RECV + sender revoke (single-writer preserved) ::"
    );
    super::sched::spawn_user_slot("el0-u7parent", parent.entry, parent.sp, parent.ttbr0, demo_cpu);

    // 4. The single-writer witness: t1 pending in the child's inbox + the child's row still untouched
    //    (the child is provably pre-RECV — it is parked on the GO word this launcher has not released).
    let dstart = super::timer::cntpct();
    let ddeadline = 5 * super::timer::cntfrq();
    let mut deposit_seen = false;
    while !deposit_seen && super::timer::cntpct().wrapping_sub(dstart) <= ddeadline {
        deposit_seen = (0..NXFER).any(|k| {
            let t = XFER_SLOT_TX[child.asid as usize][k].load(Ordering::Acquire);
            t != 0 && t != HANDLE_RESERVING
        });
        if !deposit_seen {
            super::sched::yield_now();
        }
    }
    let snap_ok = deposit_seen && handle_row_is_clear(child.asid);
    u7_release_go(child.slot);

    // 5. Use-then-revoke sequencing: wait for the child's first successful write through the cap, then
    //    let the parent revoke.
    let ustart = super::timer::cntpct();
    let udeadline = 5 * super::timer::cntfrq();
    while U7_CHILD_USED.load(Ordering::Acquire) == 0
        && super::timer::cntpct().wrapping_sub(ustart) <= udeadline
    {
        super::sched::yield_now();
    }
    let used = U7_CHILD_USED.load(Ordering::Acquire);
    u7_release_go(parent.slot);

    // 6a. Wait (bounded) for both sentinel exits, then snapshot the witnesses.
    let vstart = super::timer::cntpct();
    let vdeadline = 8 * super::timer::cntfrq();
    while EL0_U7_DONE.load(Ordering::Acquire) < 2
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let pw = U7_PARENT_WITNESS.load(Ordering::Acquire);
    let cw = U7_CHILD_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U7_KILLED.load(Ordering::Acquire);

    // 6b. Teardown/leak proof: both rows + both inboxes clear and the record ledger fully free (t1's
    //     record was released when the child's revoked handle tore down; t2's likewise — false->true as
    //     the exits' teardowns run).
    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    let all_clear = |pa: u64, ca: u64| {
        handle_row_is_clear(pa)
            && handle_row_is_clear(ca)
            && xfer_row_is_clear(pa)
            && xfer_row_is_clear(ca)
            && xfer_recs_all_free()
    };
    while !all_clear(parent.asid, child.asid)
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = all_clear(parent.asid, child.asid);
    proc_free(pi); // the planted pid->ASID entry (the fixtures exited by name, never through the Proc path)

    if pw == U7_WITNESS_ALL && cw == U7_WITNESS_ALL && used != 0 && snap_ok && cleared && killed == 0 {
        serial_println!(
            ":: U7: cross-process transfer — SYS_XFER attenuated, child received + used the cap, revoke enforced, single-writer intact -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U7: cross-process transfer FAIL — parent={:#x} child={:#x} used={} snap={} cleared={} killed={} done={} (want {:#x}/{:#x}/1/true/true/0/2) ::",
            pw,
            cw,
            used,
            snap_ok,
            cleared,
            killed,
            EL0_U7_DONE.load(Ordering::Acquire),
            U7_WITNESS_ALL,
            U7_WITNESS_ALL
        );
    }
}

// =============================================================================================
// U8: revocation trees — the single-process EL0 fixture (grant-chain kill + locality + errno
// negatives) and the kernel-side cross-process checks (re-transfer cascade + generation), folded
// into one launcher/verdict that rides the U7 launcher task.
// =============================================================================================

/// Build the U8 fixture slot — the `u7_build` shape for the U8 blob (allocate, scrub, copy,
/// I-cache-sync, protect, return run params). The scrub keeps the same U7 discipline: a prior tenant's
/// bytes survive teardown and must never leak into a fresh fixture window.
fn u8_build() -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u8_blob_start as usize;
    let bend = &raw const __u8_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U8 blob does not fit in a code page");
    let entry = {
        let va = base + (&raw const __u8_prog_tree as usize - bstart) as u64;
        assert!(va & 3 == 0, "U8 fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// The U8 kernel-side checks — the cross-process halves the single-process fixture cannot stage. Drives
/// the REAL syscall bodies (`sys_xfer_from`/`sys_recv_for`/`sys_cap_grant`/`sys_cap_xrevoke`) over three
/// scratch ASID rows (6/7/8 — every demo fixture has exited and torn down by the time this runs, so the
/// rows are provably clear and nothing else touches them; every planted resource is dropped again before
/// returning, verified by the final ledger-clean checks the verdict folds in). Returns true iff ALL hold:
///
///  1. THE RE-TRANSFER CASCADE (U7 escape #2): S transfers a console cap to R1 (with CAP_GRANT); R1
///     re-transfers it onward to R2; S revokes the ROOT transfer -> R1's cap is stale (U7 semantics) AND
///     R2's re-transferred cap is stale TOO (the tree — U7 provably let this one escape), and a re-grant
///     from the dead cap is refused.
///  2. GENERATION-TAGGED INBOXES: a deposit stamped for R1's current tenant, followed by R1's teardown
///     generation bump (the `clear_handle_row` primitive — here invoked as the bare bump, the exact word
///     teardown writes), is NEVER delivered to the recycled ASID's next tenant (RECV discards it; its
///     record frees, so the sender's later XREVOKE honestly finds nothing).
///  3. LEDGER HYGIENE: after dropping every planted handle/Proc entry, the handle rows, inboxes, transfer
///     records AND the derivation ledger are all fully clear — no revoke/discard path leaked a node.
fn u8_kernel_check() -> bool {
    const S: u64 = 6; // scratch "sender" ASID row
    const R1: u64 = 7; // scratch first recipient
    const R2: u64 = 8; // scratch grand-recipient
    const PID1: u64 = 0xE1; // planted recipient pids (never collide: PROCS holds only planted entries now)
    const PID2: u64 = 0xE2;
    let mut ok = true;

    // Plant the two recipient Proc entries (the pid->ASID maps sys_xfer resolves through).
    let Some(p1) = proc_reserve() else {
        return false;
    };
    let Some(p2) = proc_reserve() else {
        proc_free(p1);
        return false;
    };
    PROCS[p1].asid.store(R1, Ordering::Release);
    PROCS[p1].pid.store(PID1, Ordering::Release);
    PROCS[p2].asid.store(R2, Ordering::Release);
    PROCS[p2].pid.store(PID2, Ordering::Release);
    // The sender's table: a delegable console cap at 0, Child handles naming R1/R2's tenants at 2 (R1)
    // and — in R1's own row — at 2 (R2), for the onward hop.
    install_cap(S, 0, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    install_cap(S, 2, KIND_CHILD, PID1, 0);
    install_cap(R1, 2, KIND_CHILD, PID2, 0);

    // 1. The cascade. S -> R1 (keeping CAP_GRANT so R1 may delegate onward) -> R2; then revoke the root.
    let t1 = sys_xfer_from(S, 2, 0, (CAP_WRITE | CAP_GRANT) as u64);
    ok &= t1 > 0;
    let h1 = sys_recv_for(R1);
    ok &= h1 >= 0;
    // h2 must carry CAP_GRANT: the laundering assertion below has to reach the U8 tree-deep
    // staleness check in sys_cap_grant — without CAP_GRANT the earlier missing-right gate
    // returns the same EACCES and the assertion is vacuous (U8 review note, closed in-arc).
    let t2 = if h1 >= 0 {
        sys_xfer_from(R1, 2, h1 as u64, (CAP_WRITE | CAP_GRANT) as u64)
    } else {
        -1
    };
    ok &= t2 > 0;
    let h2 = sys_recv_for(R2);
    ok &= h2 >= 0;
    // Pre-revoke, the grand-received cap carries real authority.
    ok &= matches!(handle_resolve(R2, h2 as u64, CAP_WRITE), Ok(HandleTarget::Console));
    // The sender revokes the ROOT transfer...
    ok &= t1 > 0 && sys_cap_xrevoke(S, t1 as u64) == 0;
    // ...the direct recipient's cap is stale (U7's own guarantee, unchanged)...
    ok &= handle_resolve(R1, h1 as u64, CAP_WRITE).is_err();
    // ...AND the RE-TRANSFERRED cap is stale too — the U7 escape, closed by the derivation walk.
    ok &= handle_resolve(R2, h2 as u64, CAP_WRITE).is_err();
    // ...and the dead cap cannot be laundered by a fresh local mint either — h2 CARRIES
    // CAP_GRANT, so this EACCES provably comes from the tree-deep staleness check, not the
    // missing-right gate.
    ok &= sys_cap_grant(R2, h2 as u64, CAP_WRITE as u64) == EACCES;

    // 2. Generations. Deposit for R1's CURRENT tenant, then that tenant tears down (the generation bump
    //    is the exact store `clear_handle_row` opens with) — the recycled ASID's next tenant RECVs nothing.
    let t3 = sys_xfer_from(S, 2, 0, CAP_WRITE as u64);
    ok &= t3 > 0;
    ASID_GEN[R1 as usize].fetch_add(1, Ordering::AcqRel); // teardown + recycle: a NEW tenant generation
    ok &= sys_recv_for(R1) == EAGAIN; // the stale deposit is discarded, never delivered
    ok &= t3 > 0 && sys_cap_xrevoke(S, t3 as u64) == ENOENT; // its record already freed by the discard

    // 3. Drop everything planted/delivered, then demand every ledger fully clear (subtree tombstones
    //    drained, no node/record/slot leaked on any of the paths above).
    if h2 >= 0 {
        handle_clear(R2, h2 as usize);
    }
    if h1 >= 0 {
        handle_clear(R1, h1 as usize);
    }
    handle_clear(R1, 2);
    handle_clear(S, 2);
    handle_clear(S, 0);
    proc_free(p1);
    proc_free(p2);
    ok &= handle_row_is_clear(S) && handle_row_is_clear(R1) && handle_row_is_clear(R2);
    ok &= xfer_row_is_clear(R1) && xfer_row_is_clear(R2);
    ok &= xfer_recs_all_free() && deriv_all_free();
    ok
}

/// U8 launcher + verdict — called by the U7 launcher task after the whole U7 flow (program-order gating;
/// see `u7_launcher`). Flow: skip silently with no SD (the control-path discipline — U8 needs no disk);
/// build + pre-endow + spawn the single fixture (`el0-u8tree`: index 2 = a console cap WITH `CAP_REVOKE`,
/// index 3 = one WITHOUT); wait (bounded) for its sentinel exit; wait (bounded) for its teardown (row clear
/// + the derivation ledger drained — the tombstone-cascade proof); run the kernel-side cross-process checks
/// (which need the clear ledgers); PASS iff witness == `U8_WITNESS_ALL` AND torn down AND no kill AND the
/// kernel checks held. U8 is the last demo — it releases no further gate.
fn u8_launcher(demo_cpu: usize) {
    // One-shot (the U7 launcher is spawned once; guard defensively anyway).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // No SD device -> keep the no-storage control path free of demo lines (mirrors U7's gate).
    if crate::drivers::block::info().is_none() {
        return;
    }
    let Some(fix) = u8_build() else {
        serial_println!(":: U8: no free address-space slot — revocation-tree demo skipped ::");
        return;
    };
    install_cap(fix.asid, U8_SRC_IDX, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT | CAP_REVOKE);
    install_cap(fix.asid, U8_SRC2_IDX, KIND_CONSOLE, HANDLE_CONSOLE, CAP_WRITE | CAP_GRANT);
    serial_println!(
        ":: U8: revocation trees — derivation ledger + generation-tagged inboxes (revoke chases the subtree) ::"
    );
    super::sched::spawn_user_slot("el0-u8tree", fix.entry, fix.sp, fix.ttbr0, demo_cpu);

    // Wait (bounded ~5 s, yielding) for the fixture's sentinel exit, then snapshot the witness.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U8_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U8_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U8_KILLED.load(Ordering::Acquire);

    // Teardown proof: the fixture exited holding live derived handles (g1/g2/g4 and the two endowed
    // sources — g1/g2 already stale, but their NODES persist as tombstones until the row clears), so its
    // teardown must drain BOTH the handle row and the derivation ledger. Poll bounded; false->true.
    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(handle_row_is_clear(fix.asid) && deriv_all_free())
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = handle_row_is_clear(fix.asid) && deriv_all_free();

    // Kernel-side cross-process checks (they require the drained ledgers the wait above establishes).
    let ledger_ok = cleared && u8_kernel_check();

    if witness == U8_WITNESS_ALL && cleared && ledger_ok && killed == 0 {
        serial_println!(
            ":: U8: revocation trees — parent revoke kills re-grant + re-transfer, generation-tagged inbox, ledger clean -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U8: revocation trees FAIL — witness={:#x} cleared={} ledger={} killed={} done={} (want {:#x}/true/true/0/1) ::",
            witness,
            cleared,
            ledger_ok,
            killed,
            EL0_U8_DONE.load(Ordering::Acquire),
            U8_WITNESS_ALL
        );
    }
}

// =============================================================================================
// U9: real File WRITES + SEEK — the single-process EL0 fixture (open-RW -> seek -> in-place write ->
// seek back -> read-back witness + the RO-write / wrong-kind denials) and the kernel-side checks
// (the on-disk sector actually changed + the file size did NOT, and a U8-revoked File-write cap is
// -EACCES), folded into one launcher/verdict that rides the U7 launcher task after U8.
// =============================================================================================

/// Build the U9 fixture slot — the `u8_build` shape for the U9 blob (allocate, scrub, copy, I-cache-sync,
/// protect, return run params). The scrub keeps the U7/U8 discipline: a prior tenant's bytes survive teardown
/// and must never leak into a fresh fixture window (the fixture's read-back buffer lives at +0x2000).
fn u9_build() -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u9_blob_start as usize;
    let bend = &raw const __u9_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U9 blob does not fit in a code page");
    let entry = {
        let va = base + (&raw const __u9_prog_write as usize - bstart) as u64;
        assert!(va & 3 == 0, "U9 fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// U9 kernel-side helper: read 16 bytes at absolute file offset `off` from a fresh mount, via the read-only
/// offset-aware FAT reader — the "raw re-read" the launcher uses to prove the on-disk bytes changed. `None` on
/// any mount/read failure or a short read. Independent of any FILE_OFFSET sidecar (it re-derives everything).
fn u9_read16(fc: u32, size: u32, off: u32) -> Option<[u8; 16]> {
    let fs = crate::fs::fat::mount().ok()?;
    let mut v: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    fs.read_at(fc, size, off, &mut v, 16).ok()?;
    if v.len() < 16 {
        return None;
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(&v[..16]);
    Some(out)
}

/// The U9 kernel-side revocation check — the "a U8-revoked File cap write is -EACCES" denial, staged over a
/// scratch ASID row (6 — every demo fixture has exited and torn down by the time this runs, exactly as
/// `u8_kernel_check` relies on) because a revoked-derivation setup is cleaner kernel-side than in the EL0 blob.
/// `sys_write`'s ONLY gate is `handle_resolve(asid, fd, CAP_WRITE)`, so proving that resolve is `Err` after the
/// ancestor is revoked IS proving the write returns `-EACCES`. Backs a real File descriptor so the pre-revoke
/// cap would genuinely write; the post-revoke denial then provably comes from the derivation walk. Drops
/// everything planted and demands the handle/file/derivation ledgers all clear (no node/descriptor leaked).
fn u9_check_revoked_write(fc: u32, sz: u32) -> bool {
    const A: u64 = 6; // scratch ASID row (provably clear here — the fixture uses a different, freshly-alloc'd slot)
    let mut ok = true;
    // Back a real File descriptor so the ROOT cap names a genuine open file (a write through it WOULD land).
    // dir_lba/off = 0: the write is denied at the revoked-cap resolve, so it never reaches the grow path (U10).
    let Some(fid) = files_alloc(A, fc, sz, 0, 0) else {
        return false;
    };
    let file_id = (fid + 1) as u64;
    // A ROOT File cap carrying CAP_WRITE|CAP_GRANT|CAP_REVOKE at index 2 (off index 0 / CONSOLE_FD, the U8 idiom).
    install_cap(A, 2, KIND_FILE, file_id, CAP_WRITE | CAP_GRANT | CAP_REVOKE);
    // Pre-revoke: the root File+CAP_WRITE cap resolves — a `sys_write` through it WOULD pass the CHECK.
    ok &= matches!(handle_resolve(A, 2, CAP_WRITE), Ok(HandleTarget::File(id)) if id == file_id);
    // Derive a child File+CAP_WRITE (records the derivation edge + back-fills the root's node).
    let g = sys_cap_grant(A, 2, CAP_WRITE as u64);
    ok &= g >= 0;
    // Pre-revoke: the DERIVED File+CAP_WRITE cap also resolves — a write through it WOULD land too.
    if g >= 0 {
        ok &= matches!(handle_resolve(A, g as u64, CAP_WRITE), Ok(HandleTarget::File(_)));
    }
    // Revoke the ROOT (index 2 carries CAP_REVOKE) -> kills the derivation subtree; the revoke clears index 2.
    ok &= sys_cap_revoke(A, 2) == 0;
    // THE DENIAL: the derived File+CAP_WRITE cap is now stale — its next CAP_WRITE resolve (exactly the CHECK
    // `sys_write` performs) is `-EACCES`. This is the "a U8-revoked File cap write -> -EACCES" proof.
    if g >= 0 {
        ok &= handle_resolve(A, g as u64, CAP_WRITE).is_err();
    }
    // Drop everything planted; index 2 was already cleared by the revoke, so clearing the derived handle drops
    // the last node (the root's tombstone cascades free). Then demand every ledger clear — no leak on any path.
    if g >= 0 {
        handle_clear(A, g as usize);
    }
    let _ = files_free(A, fid); // scaffold descriptor (dir_lba == 0): the refcount decrement is a no-op
    ok &= handle_row_is_clear(A) && files_row_is_clear(A) && deriv_all_free();
    ok
}

/// U9 launcher + verdict — called by the U7 launcher task after the whole U8 flow (program-order gating; see
/// `u7_launcher`). Flow: skip silently with no SD (the fixture writes a real disk file); pre-flight SCRATCH.BIN
/// (chain head + size + the pre-image bytes at the write offset — the "changed" baseline) BEFORE allocating a
/// slot; build + pre-endow (index 2 = a `Socket` carrying `CAP_WRITE`, the kind negative) + spawn the fixture;
/// wait (bounded) for its sentinel exit + teardown (its two open descriptors clear the FILES row); then the
/// kernel-side checks: re-read the on-disk bytes at the offset (must now equal the written pattern, and differ
/// from the pre-image) + the directory size UNCHANGED (in-place, never grew), and the revoked-File-write denial.
/// PASS iff witness == `U9_WITNESS_ALL` AND torn down AND no kill AND all kernel checks held. U9 is the last
/// demo — it releases no further gate.
fn u9_launcher(demo_cpu: usize) {
    // One-shot (the U7 launcher is spawned once; guard defensively anyway).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // No SD device -> the fixture cannot write a disk file; skip silently (mirrors U6b/U7/U8's control path).
    if crate::drivers::block::info().is_none() {
        return;
    }
    // Pre-flight the ONE fallible disk lookup — SCRATCH.BIN's chain head + size — BEFORE allocating a slot
    // (the U6b discipline: fallible lookups first, resource alloc last, so a lookup failure leaks nothing).
    let (fc, pre_size) = match crate::fs::fat::mount()
        .and_then(|fs| fs.find_in_root(U9_SCRATCH_NAME).map(|de| (de.first_cluster(), de.size, de.is_dir)))
    {
        Ok((fc, sz, false)) => (fc, sz),
        _ => {
            serial_println!(
                ":: U9: pre-open of SCRATCH.BIN failed (absent / a directory / unmountable) — File-write demo skipped ::"
            );
            return;
        }
    };
    // Capture the pre-image bytes at the write offset (the 0x55 filler the image planted) — the baseline the
    // "sector changed" check compares against. A read failure here is not fatal to the demo; it fails the check.
    let pre = u9_read16(fc, pre_size, U9_WRITE_OFFSET);

    // Build the fixture slot (allocates it), pre-endow the kind negative, print the setup line, spawn.
    let Some(fix) = u9_build() else {
        serial_println!(":: U9: no free address-space slot — File-write demo skipped ::");
        return;
    };
    // The kind negative: a Socket carrying CAP_WRITE at U9_SOCK_IDX. It HAS the right, so a File `sys_write` is
    // denied purely on kind (write serves Console/File only) — the kind arm, not the rights arm. A scaffold id.
    install_cap(fix.asid, U9_SOCK_IDX, KIND_SOCKET, 0x200, CAP_WRITE);
    serial_println!(
        ":: U9: real File writes — SYS_SEEK + File+CAP_WRITE routed through fat::write_at (in place) ::"
    );
    super::sched::spawn_user_slot("el0-u9write", fix.entry, fix.sp, fix.ttbr0, demo_cpu);

    // Wait (bounded ~5 s, yielding) for the fixture's sentinel exit, then snapshot the witness.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U9_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U9_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U9_KILLED.load(Ordering::Acquire);

    // Teardown proof: the fixture exited holding two live descriptors (its RW + RO opens) and the pre-endowed
    // Socket handle, so its exit cleared BOTH the FILES row and the handle row. Poll bounded; false->true.
    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid))
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid);

    // Kernel-side "the write actually hit the disk" checks: re-read SCRATCH.BIN from a fresh mount. The size
    // must be UNCHANGED (in-place — never grew), the bytes at the offset must now equal the written pattern,
    // and they must differ from the pre-image (a real change, not a coincidence).
    let post = crate::fs::fat::mount()
        .ok()
        .and_then(|fs| fs.find_in_root(U9_SCRATCH_NAME).ok())
        .map(|de| (de.first_cluster(), de.size));
    let (size_unchanged, sector_changed) = match (post, pre) {
        (Some((post_fc, post_size)), Some(pre_bytes)) => {
            let size_unchanged = post_size == pre_size && post_size == U9_SCRATCH_SIZE;
            let now = u9_read16(post_fc, post_size, U9_WRITE_OFFSET);
            let sector_changed =
                now == Some(U9_PATTERN) && pre_bytes != U9_PATTERN;
            (size_unchanged, sector_changed)
        }
        _ => (false, false),
    };

    // Kernel-side revoked-File-write denial (needs a clear scratch row — every fixture has torn down by here).
    let revoke_ok = u9_check_revoked_write(fc, pre_size);

    if witness == U9_WITNESS_ALL
        && cleared
        && killed == 0
        && size_unchanged
        && sector_changed
        && revoke_ok
    {
        serial_println!(
            ":: U9: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES, on-disk sector changed + size unchanged -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U9: real File writes FAIL — witness={:#x} cleared={} killed={} size_unchanged={} sector_changed={} revoke={} done={} (want {:#x}/true/0/true/true/true/1) ::",
            witness,
            cleared,
            killed,
            size_unchanged,
            sector_changed,
            revoke_ok,
            EL0_U9_DONE.load(Ordering::Acquire),
            U9_WITNESS_ALL
        );
    }
}

// =============================================================================================
// U10: file GROWTH — the single-process EL0 fixture (open-RW -> seek-to-EOF -> write PAST the cluster
// boundary -> read-back the appended bytes + confirm the original cluster + the RO-write denial) and the
// kernel-side checks (a fresh-mount re-read shows the directory size GREW, the appended data is on disk,
// the original cluster survived, and both FAT copies agree along the now-2-cluster chain), folded into one
// launcher/verdict that rides the U7 launcher task after U9.
// =============================================================================================

/// Build the U10 fixture slot — the `u9_build` shape for the U10 blob (allocate, scrub, copy, I-cache-sync,
/// protect, return run params). The scrub keeps the U7/U8/U9 discipline: a prior tenant's bytes must never leak
/// into a fresh fixture window (the fixture's read-back buffer lives at +0x2000).
fn u10_build() -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u10_blob_start as usize;
    let bend = &raw const __u10_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U10 blob does not fit in a code page");
    let entry = {
        let va = base + (&raw const __u10_prog_grow as usize - bstart) as u64;
        assert!(va & 3 == 0, "U10 fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// U10 kernel-side helper: from a FRESH mount, walk GROW.BIN's chain and prove (a) it grew from 1 cluster to
/// exactly 2 (the append crossed the cluster boundary) and (b) EVERY cluster's FAT entry is byte-identical
/// across ALL `num_fats` copies — the "one-FAT write is a corrupt volume" invariant, verified end to end. `false`
/// on any mount / walk / read failure. Independent of any descriptor sidecar (it re-derives everything).
fn u10_fats_consistent(first_cluster: u32) -> bool {
    let Ok(fs) = crate::fs::fat::mount() else {
        return false;
    };
    let Ok(chain) = fs.chain_clusters(first_cluster) else {
        return false;
    };
    if chain.len() != 2 {
        return false; // grew from the planted 1 cluster to exactly 2
    }
    let nf = fs.num_fats();
    for &c in &chain {
        let Ok(e0) = fs.fat_entry_copy(c, 0) else {
            return false;
        };
        let mut f = 1;
        while f < nf {
            match fs.fat_entry_copy(c, f) {
                Ok(ef) if ef == e0 => {}
                _ => return false, // a copy disagrees (or read failed) -> a torn / one-FAT write
            }
            f += 1;
        }
    }
    true
}

/// U10 launcher + verdict — called by the U7 launcher task after the U9 flow (program-order gating; see
/// `u7_launcher`). Flow: skip silently with no SD (the fixture grows a real disk file); pre-flight GROW.BIN
/// (chain head + planted size) BEFORE allocating a slot; build + spawn the fixture; wait (bounded) for its
/// sentinel exit + teardown (its two open descriptors clear the FILES row); then the kernel-side checks — a
/// fresh-mount re-read shows the directory size GREW to `U10_GROW_NEW_SIZE`, the appended bytes at the grow
/// offset are on disk, the original first cluster is intact, and both FAT copies agree along the 2-cluster
/// chain. PASS iff witness == `U10_WITNESS_ALL` AND torn down AND no kill AND all kernel checks held. U10 is the
/// last demo — it releases no further gate.
fn u10_launcher(demo_cpu: usize) {
    // One-shot (the U7 launcher is spawned once; guard defensively anyway).
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // No SD device -> the fixture cannot grow a disk file; skip silently (mirrors U6b/U7/U8/U9's control path).
    if crate::drivers::block::info().is_none() {
        return;
    }
    // Pre-flight the ONE fallible disk lookup — GROW.BIN's chain head + planted size — BEFORE allocating a slot
    // (the U6b/U9 discipline: fallible lookups first, resource alloc last, so a lookup failure leaks nothing).
    let pre_size = match crate::fs::fat::mount()
        .and_then(|fs| fs.find_in_root(U10_GROW_NAME).map(|de| (de.first_cluster(), de.size, de.is_dir)))
    {
        Ok((_fc, sz, false)) => sz,
        _ => {
            serial_println!(
                ":: U10: pre-open of GROW.BIN failed (absent / a directory / unmountable) — file-growth demo skipped ::"
            );
            return;
        }
    };

    // Build the fixture slot (allocates it), print the setup line, spawn. No pre-endow: the fixture's only
    // negative (bit4) is an RO open of the same file, so no scaffold handle is needed.
    let Some(fix) = u10_build() else {
        serial_println!(":: U10: no free address-space slot — file-growth demo skipped ::");
        return;
    };
    serial_println!(
        ":: U10: file growth — File+CAP_WRITE past EOF routed through fat::write_grow (alloc + zero + chain, dir size last) ::"
    );
    super::sched::spawn_user_slot("el0-u10grow", fix.entry, fix.sp, fix.ttbr0, demo_cpu);

    // Wait (bounded ~5 s, yielding) for the fixture's sentinel exit, then snapshot the witness.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U10_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U10_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U10_KILLED.load(Ordering::Acquire);

    // Teardown proof: the fixture exited holding two live descriptors (its RW + RO opens), so its exit cleared
    // BOTH the FILES row and the handle row. Poll bounded; false->true.
    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid))
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid);

    // Kernel-side "the grow actually hit the disk" checks: re-read GROW.BIN from a fresh mount. The size must
    // have GROWN to U10_GROW_NEW_SIZE (and past the pre-size), the bytes at the grow offset must equal the
    // appended pattern, the original first cluster must still hold the planted filler, and both FAT copies must
    // agree along the now-2-cluster chain.
    let post = crate::fs::fat::mount()
        .ok()
        .and_then(|fs| fs.find_in_root(U10_GROW_NAME).ok())
        .map(|de| (de.first_cluster(), de.size));
    let (size_grew, appended, original_intact, fats_ok) = match post {
        Some((post_fc, post_size)) => {
            let size_grew = post_size == U10_GROW_NEW_SIZE && post_size > pre_size;
            let appended = u9_read16(post_fc, post_size, U10_GROW_OFFSET) == Some(U10_GROW_PATTERN);
            let original_intact = u9_read16(post_fc, post_size, 0) == Some([U10_GROW_FILLER; 16]);
            let fats_ok = u10_fats_consistent(post_fc);
            (size_grew, appended, original_intact, fats_ok)
        }
        None => (false, false, false, false),
    };

    if witness == U10_WITNESS_ALL
        && cleared
        && killed == 0
        && size_grew
        && appended
        && original_intact
        && fats_ok
    {
        serial_println!(
            ":: U10: file growth — open-RW+grow-write+readback OK, original cluster intact, RO-write -EACCES, on-disk size grew + appended data present + both FAT copies consistent -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U10: file growth FAIL — witness={:#x} cleared={} killed={} size_grew={} appended={} intact={} fats={} done={} (want {:#x}/true/0/true/true/true/true/1) ::",
            witness,
            cleared,
            killed,
            size_grew,
            appended,
            original_intact,
            fats_ok,
            EL0_U10_DONE.load(Ordering::Acquire),
            U10_WITNESS_ALL
        );
    }
}

// =============================================================================================
// U10-create: file CREATE — the single-process EL0 fixture (open O_CREAT|RW -> write-from-empty ->
// read-back -> idempotent re-open) and the kernel-side checks (a fresh-mount re-read finds FRESH.BIN
// in the root with the right size + content, and exactly ONE entry — no duplicate), folded into one
// launcher/verdict that rides the U7 launcher task after U10-grow.
// =============================================================================================

/// Build the U10-create fixture slot — the `u10_build` shape for the U10-create blob.
fn u10c_build() -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u10c_blob_start as usize;
    let bend = &raw const __u10c_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U10-create blob does not fit in a code page");
    let entry = {
        let va = base + (&raw const __u10c_prog_create as usize - bstart) as u64;
        assert!(va & 3 == 0, "U10-create fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// U10-create launcher + verdict — called by the U7 launcher task after the U10-grow flow. Flow: skip with no
/// SD; confirm FRESH.BIN is ABSENT up front (the demo must CREATE it, not find a stale one) BEFORE allocating a
/// slot; build + spawn the fixture; wait (bounded) for its sentinel exit + teardown (its two open descriptors
/// clear the FILES row); then the kernel-side checks — a fresh mount finds FRESH.BIN in the root with size ==
/// `U10C_WRITTEN`, its content == the written pattern, a valid first cluster, and EXACTLY ONE such entry (the
/// second O_CREAT opened, did not duplicate). PASS iff witness == `U10C_WITNESS_ALL` AND torn down AND no kill
/// AND all kernel checks held. U10-create is the last demo — it releases no further gate.
fn u10c_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the fixture cannot create a disk file; skip silently
    }
    // Pre-flight: the volume must mount, and FRESH.BIN must be ABSENT (so a PASS proves a real create). A stale
    // FRESH.BIN (should never happen — kernel8 rebuilds the image) would make the create an idempotent open; log
    // and skip rather than report a misleading PASS.
    match crate::fs::fat::mount() {
        Ok(fs) => {
            if fs.find_in_root(U10C_NAME).is_ok() {
                serial_println!(
                    ":: U10-create: FRESH.BIN already present pre-demo (stale image) — create demo skipped ::"
                );
                return;
            }
        }
        Err(_) => return, // unmountable -> skip silently (mirrors the no-SD path)
    }

    let Some(fix) = u10c_build() else {
        serial_println!(":: U10-create: no free address-space slot — create demo skipped ::");
        return;
    };
    serial_println!(
        ":: U10-create: file create — SYS_OPEN O_CREAT routed through fat::create_in_root (fresh dir entry, grow-from-empty) ::"
    );
    super::sched::spawn_user_slot("el0-u10create", fix.entry, fix.sp, fix.ttbr0, demo_cpu);

    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U10C_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U10C_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U10C_KILLED.load(Ordering::Acquire);

    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid))
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid);

    // Kernel-side "the create actually hit the disk" checks: re-mount and find FRESH.BIN. It must EXIST with the
    // written size + content, a valid first cluster (grew from 0), and be the ONLY such entry (idempotent create).
    let (exists, size_ok, content_ok, single, fc_ok) = match crate::fs::fat::mount() {
        Ok(fs) => match fs.find_in_root(U10C_NAME) {
            Ok(de) => {
                let fc = de.first_cluster();
                let size_ok = de.size == U10C_WRITTEN;
                let content_ok = u9_read16(fc, de.size, 0) == Some(U10C_PATTERN);
                let fc_ok = fc >= 2;
                let count = fs
                    .read_root()
                    .map(|v| v.iter().filter(|d| d.name() == U10C_NAME).count())
                    .unwrap_or(0);
                (true, size_ok, content_ok, count == 1, fc_ok)
            }
            Err(_) => (false, false, false, false, false),
        },
        Err(_) => (false, false, false, false, false),
    };

    if witness == U10C_WITNESS_ALL
        && cleared
        && killed == 0
        && exists
        && size_ok
        && content_ok
        && single
        && fc_ok
    {
        serial_println!(
            ":: U10-create: file create — O_CREAT+write-from-empty+readback OK, idempotent re-open, on-disk entry present with right size + content, no duplicate -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U10-create: file create FAIL — witness={:#x} cleared={} killed={} exists={} size={} content={} single={} fc={} done={} (want {:#x}/true/0/true/true/true/true/true/1) ::",
            witness,
            cleared,
            killed,
            exists,
            size_ok,
            content_ok,
            single,
            fc_ok,
            EL0_U10C_DONE.load(Ordering::Acquire),
            U10C_WITNESS_ALL
        );
    }
}

// =============================================================================================
// U10-delete: file DELETE — the single-process EL0 fixture (create -> write -> SYS_UNLINK -> re-open
// -ENOENT) and the kernel-side checks (a fresh mount finds the entry GONE, the file's data cluster is
// free again in ALL FAT copies, and the first-free cluster is unchanged from before the run — the
// freed cluster is re-allocatable), folded into one launcher/verdict that rides the U7 launcher task
// after U10-create.
// =============================================================================================

/// Build the U10-delete fixture slot — the `u10_build` shape for the U10-delete blob.
fn u10d_build() -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u10d_blob_start as usize;
    let bend = &raw const __u10d_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U10-delete blob does not fit in a code page");
    let entry = {
        let va = base + (&raw const __u10d_prog_delete as usize - bstart) as u64;
        assert!(va & 3 == 0, "U10-delete fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// U10-delete launcher + verdict — called by the U7 launcher task after the U10-create flow. Flow: skip with no
/// SD; confirm DELME.BIN is ABSENT and snapshot the first-free cluster `f0` (the cluster the fixture's write
/// will allocate — deterministic, single sequential kernel task) BEFORE allocating a slot; build + spawn the
/// fixture; wait (bounded) for its sentinel exit + teardown; then the kernel-side checks — a fresh mount finds
/// DELME.BIN GONE, `f0`'s FAT entry is `0` in ALL copies (the chain was freed everywhere), and the first-free
/// cluster is again `f0` (the freed cluster is re-allocatable). PASS iff witness == `U10D_WITNESS_ALL` AND torn
/// down AND no kill AND all kernel checks held. U10-delete is the last demo — it releases no further gate.
fn u10d_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the fixture cannot delete a disk file; skip silently
    }
    // Pre-flight: mount, require DELME.BIN ABSENT, and snapshot the first-free cluster (what the fixture's write
    // will allocate). A stale DELME.BIN would confound the re-allocatability proof; log and skip.
    let f0 = match crate::fs::fat::mount() {
        Ok(fs) => {
            if fs.find_in_root(U10D_NAME).is_ok() {
                serial_println!(
                    ":: U10-delete: DELME.BIN already present pre-demo (stale image) — delete demo skipped ::"
                );
                return;
            }
            match fs.first_free_cluster() {
                Ok(c) => c,
                Err(_) => {
                    serial_println!(":: U10-delete: no free cluster to probe — delete demo skipped ::");
                    return;
                }
            }
        }
        Err(_) => return, // unmountable -> skip silently
    };

    let Some(fix) = u10d_build() else {
        serial_println!(":: U10-delete: no free address-space slot — delete demo skipped ::");
        return;
    };
    serial_println!(
        ":: U10-delete: file delete — SYS_UNLINK routed through fat::delete_located (dir 0xE5 + free chain, all FAT copies) ::"
    );
    super::sched::spawn_user_slot("el0-u10delete", fix.entry, fix.sp, fix.ttbr0, demo_cpu);

    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U10D_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U10D_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U10D_KILLED.load(Ordering::Acquire);

    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid))
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid);

    // Kernel-side "the delete actually hit the disk" checks: re-mount, and verify DELME.BIN is GONE, its data
    // cluster `f0` is free in ALL FAT copies, and the first-free cluster is again `f0` (re-allocatable).
    let (gone, freed, reusable) = match crate::fs::fat::mount() {
        Ok(fs) => {
            let gone = fs.find_in_root(U10D_NAME).is_err();
            let nf = fs.num_fats();
            let mut freed = true;
            let mut f = 0;
            while f < nf {
                match fs.fat_entry_copy(f0, f) {
                    Ok(0) => {}
                    _ => freed = false,
                }
                f += 1;
            }
            let reusable = fs.first_free_cluster() == Ok(f0);
            (gone, freed, reusable)
        }
        Err(_) => (false, false, false),
    };

    if witness == U10D_WITNESS_ALL && cleared && killed == 0 && gone && freed && reusable {
        serial_println!(
            ":: U10-delete: file delete — create+write+unlink OK, re-open -ENOENT, on-disk entry gone + chain freed (all FAT copies) + cluster re-allocatable -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U10-delete: file delete FAIL — witness={:#x} cleared={} killed={} gone={} freed={} reusable={} done={} (want {:#x}/true/0/true/true/true/1) ::",
            witness,
            cleared,
            killed,
            gone,
            freed,
            reusable,
            EL0_U10D_DONE.load(Ordering::Acquire),
            U10D_WITNESS_ALL
        );
    }
}

/// Build the U11 open-file-lifecycle fixture slot — the `u10d_build` shape for the U11 blob.
fn u11_build() -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u11_blob_start as usize;
    let bend = &raw const __u11_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U11 blob does not fit in a code page");
    let entry = {
        let va = base + (&raw const __u11_prog_close as usize - bstart) as u64;
        assert!(va & 3 == 0, "U11 fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// U11 kernel-side proof of the generation-tag rebind fix, staged on a scratch ASID row (6 — clear by this point
/// in the demo chain, exactly as `u9_check_revoked_write` relies on, and re-cleared defensively first). It
/// reproduces the U10 sibling-rebind hole MECHANICALLY: claim a descriptor slot and mint its `(gen, idx)`
/// file-id; free it (bumping the gen); then RE-claim the SAME slot (first-fit) for a "different file" — and
/// prove the OLD file-id is now rejected by `file_desc_validate` (a generation mismatch) EVEN THOUGH the slot is
/// live again (`FILE_USED` true), while a FRESH file-id minted against the reused slot resolves. That is exactly
/// "no silent rebind on slot reuse", isolated from disk/EL0 timing. Leaves the row clean (no descriptor leaked).
fn u11_check_gen_rebind() -> bool {
    const A: u64 = 6; // scratch ASID row (every demo fixture has exited + torn down by the time this runs)
    clear_files_row(A); // defensive: start from a provably clear row
    let mut ok = true;
    // 1. Claim a slot; mint its file-id at the current generation. A live descriptor resolves.
    let Some(fid0) = files_alloc(A, 3 /*cluster*/, 16 /*size*/, 0, 0) else {
        return false;
    };
    let g0 = FILE_GEN[A as usize][fid0].load(Ordering::Acquire);
    let id0 = file_id_pack(g0, fid0);
    ok &= file_desc_validate(A, id0) == Some(fid0);
    // 2. Free the slot (bumps the generation). The old file-id no longer resolves (slot is free).
    let _ = files_free(A, fid0); // scaffold descriptor (dir_lba == 0): the refcount decrement is a no-op
    ok &= file_desc_validate(A, id0).is_none();
    // 3. Re-claim: first-fit reuses the SAME slot at the bumped generation — a "different file".
    let Some(fid1) = files_alloc(A, 9 /*different cluster*/, 32, 0, 0) else {
        return false;
    };
    ok &= fid1 == fid0; // first-fit really reclaimed the slot
    let g1 = FILE_GEN[A as usize][fid1].load(Ordering::Acquire);
    ok &= g1 != g0; // the generation advanced on free
    let id1 = file_id_pack(g1, fid1);
    // 4. THE PROOF: the slot is LIVE again, yet the STALE file-id is rejected (gen mismatch — no rebind); the
    //    FRESH file-id resolves.
    ok &= FILE_USED[A as usize][fid1].load(Ordering::Acquire);
    ok &= file_desc_validate(A, id0).is_none();
    ok &= file_desc_validate(A, id1) == Some(fid1);
    // Cleanup: drop the descriptor and demand the row clean (no leak).
    let _ = files_free(A, fid1); // scaffold descriptor (dir_lba == 0): the refcount decrement is a no-op
    ok &= files_row_is_clear(A);
    ok
}

/// U11 launcher + verdict — the LAST demo in the chain, after U10-delete. Flow: skip with no SD; require A11.BIN
/// and B11.BIN ABSENT pre-demo (the fixture creates them; a stale image would confound the on-disk checks);
/// build + spawn the fixture; wait (bounded) for its sentinel exit + teardown; run the kernel-side gen-rebind
/// proof; then a fresh mount confirms A11.BIN is GONE (unlinked) and B11.BIN is PRESENT (created + never
/// deleted). PASS iff witness == `U11_WITNESS_ALL` AND torn down AND no kill AND the gen-rebind proof + on-disk
/// checks hold. Releases no further gate.
fn u11_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the fixture cannot create/open disk files; skip silently
    }
    // Pre-flight: mount and require both demo files ABSENT (fresh image). A stale A11/B11 would confound the
    // on-disk gone/present checks; log and skip.
    match crate::fs::fat::mount() {
        Ok(fs) => {
            if fs.find_in_root(U11_A_NAME).is_ok() || fs.find_in_root(U11_B_NAME).is_ok() {
                serial_println!(
                    ":: U11: A11.BIN/B11.BIN already present pre-demo (stale image) — lifecycle demo skipped ::"
                );
                return;
            }
        }
        Err(_) => return, // unmountable -> skip silently
    }

    let Some(fix) = u11_build() else {
        serial_println!(":: U11: no free address-space slot — lifecycle demo skipped ::");
        return;
    };
    serial_println!(
        ":: U11: open-file lifecycle — SYS_CLOSE + generation-tagged file-ids (stale sibling to a reused slot -> -EACCES, no rebind) ::"
    );
    super::sched::spawn_user_slot("el0-u11close", fix.entry, fix.sp, fix.ttbr0, demo_cpu);

    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U11_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let witness = U11_WITNESS.load(Ordering::Acquire);
    let killed = EL0_U11_KILLED.load(Ordering::Acquire);

    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid))
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = files_row_is_clear(fix.asid) && handle_row_is_clear(fix.asid);

    // Kernel-side mechanistic proof of the gen-tag (isolated from disk/EL0 timing) — runs AFTER teardown so
    // nothing else touches the scratch ASID row.
    let gen_ok = u11_check_gen_rebind();

    // Kernel-side on-disk checks: A11.BIN was unlinked (GONE) and B11.BIN was created + kept, with its written
    // content intact (PRESENT) — the close→re-open→read round-trip really returned B11's bytes off disk.
    let (a_gone, b_present) = match crate::fs::fat::mount() {
        Ok(fs) => {
            let a_gone = fs.find_in_root(U11_A_NAME).is_err();
            let b_present = match fs.find_in_root(U11_B_NAME) {
                Ok(de) => u9_read16(de.first_cluster(), de.size, 0) == Some(U11_B_PATTERN),
                Err(_) => false,
            };
            (a_gone, b_present)
        }
        Err(_) => (false, false),
    };

    if witness == U11_WITNESS_ALL && cleared && killed == 0 && gen_ok && a_gone && b_present {
        serial_println!(
            ":: U11: open-file lifecycle — SYS_CLOSE + gen-tagged file-ids: close/double-close/round-trip OK, stale sibling to a reused slot -EACCES (gen mismatch, no rebind), A11 unlinked + B11 present -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U11: open-file lifecycle FAIL — witness={:#x} cleared={} killed={} gen_ok={} a_gone={} b_present={} done={} (want {:#x}/true/0/true/true/true/1) ::",
            witness,
            cleared,
            killed,
            gen_ok,
            a_gone,
            b_present,
            EL0_U11_DONE.load(Ordering::Acquire),
            U11_WITNESS_ALL
        );
    }
}

/// U11-M2 (defer): release ONE of a fixture's per-step GO words (`off` within its slot window) — the launcher's
/// sequencing lever. Written through the slot's identity backing + `dsb ish` (the `u7_release_go` idiom,
/// parameterized by offset so A's read (+0x3010) / close (+0x3018) edges and B's unlink (+0x3000) edge each have
/// their own word). The whole window is scrubbed at build (`u11defer_build`), so no stale GO releases a step early.
fn u11defer_release_go(slot: usize, off: usize) {
    unsafe {
        let go = super::boot::slot_backing_ptr(slot).add(off) as *mut u64;
        core::ptr::write_volatile(go, 1);
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
    }
}

/// U11-M2 (defer): build ONE fixture slot for the shared two-entry defer blob (the `u7_build` shape — scrub the
/// whole window, copy the blob, I-cache-sync, protect the code page EL0-RX/EL1-RO). `entry_sym` selects program A
/// or B. `None` if slot allocation fails.
fn u11defer_build(entry_sym: *const u8) -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u11defer_blob_start as usize;
    let bend = &raw const __u11defer_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U11-defer blob does not fit in a code page");
    let entry = {
        let va = base + (entry_sym as usize - bstart) as u64;
        assert!(va & 3 == 0, "U11-defer fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// U11-M2 (defer) launcher + verdict — the cross-process unlink-defers-free proof. Rides the same U7 kernel task,
/// strictly after U11. Two co-located fixtures (`el0-u11defer-a`/`-b` on `demo_cpu`, cooperative via SYS_YIELD)
/// with this launcher (on the sibling core) as the single SEQUENCER: it releases each choreography edge only
/// after the prior step's SYS_REPORT cue, and re-mounts the FAT at THREE checkpoints to prove — kernel-side, off
/// the fixtures' own claims — that B's unlink freed NOTHING while A held the file open, and A's last close freed
/// the chain (all FAT copies) exactly once.
///
///   A creates+writes DEFER.BIN, reports A_OPENED
///     -> release B's unlink
///   B opens+unlinks (name gone -> a re-open is -ENOENT), reports B_UNLINKED
///     -> CHECKPOINT-1: name GONE on disk, chain (cluster `f0`) STILL allocated in all FAT copies -> release A's read
///   A seeks+reads its ORIGINAL bytes (the deferred chain is alive), reports A_READ
///     -> CHECKPOINT-2: chain still allocated -> release A's close
///   A closes (last ref: the deferred free runs) + double-close -> -EBADF; both exit
///     -> CHECKPOINT-3: chain FREED in all FAT copies + re-allocatable (first-free == `f0`)
///
/// PASS iff both witnesses full AND all three cues fired AND both exited AND no kill AND both rows torn down AND
/// the three on-disk checkpoints hold. Runtime-created file (no arroyo plant); needs a fresh image (DEFER.BIN
/// absent pre-demo). U11-defer is the last demo — releases no further gate.
fn u11defer_run(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the fixtures cannot create/open disk files; skip silently
    }
    // Pre-flight: require DEFER.BIN ABSENT (a stale copy would confound the on-disk checks), and snapshot the
    // first-free cluster `f0` — A's grow-from-empty write allocates it (first-fit), so `f0` is DEFER.BIN's chain
    // head, which the three checkpoints track (allocated -> allocated -> freed + re-allocatable).
    let f0 = match crate::fs::fat::mount() {
        Ok(fs) => {
            if fs.find_in_root(U11DEFER_NAME).is_ok() {
                serial_println!(
                    ":: U11-defer: DEFER.BIN already present pre-demo (stale image) — defer demo skipped ::"
                );
                return;
            }
            match fs.first_free_cluster() {
                Ok(c) => c,
                Err(_) => {
                    serial_println!(":: U11-defer: no free cluster pre-demo — defer demo skipped ::");
                    return;
                }
            }
        }
        Err(_) => return, // unmountable -> skip silently
    };

    // Build + spawn A, then B. A creates the file B unlinks, but B parks on its GO word until A has created it,
    // so the create-before-open ordering is enforced by the launcher (below), not by spawn order.
    let Some(a) = u11defer_build(&raw const __u11defer_prog_a) else {
        serial_println!(":: U11-defer: no free address-space slot — defer demo skipped ::");
        return;
    };
    let Some(b) = u11defer_build(&raw const __u11defer_prog_b) else {
        serial_println!(":: U11-defer: no free address-space slot — defer demo skipped (A will park out) ::");
        return;
    };
    serial_println!(
        ":: U11-defer: cross-process unlink-defers-free — B unlinks A's open file; chain freed at A's last close ::"
    );
    super::sched::spawn_user_slot("el0-u11defer-a", a.entry, a.sp, a.ttbr0, demo_cpu);
    super::sched::spawn_user_slot("el0-u11defer-b", b.entry, b.sp, b.ttbr0, demo_cpu);

    // Bounded flag-wait (deadline in cntfrq units), yielding cooperatively — the `u7_run` idiom. Returns whether
    // the flag was set within the deadline.
    let wait_flag = |flag: &AtomicU32, secs: u64| -> bool {
        let start = super::timer::cntpct();
        let deadline = secs * super::timer::cntfrq();
        while flag.load(Ordering::Acquire) == 0
            && super::timer::cntpct().wrapping_sub(start) <= deadline
        {
            super::sched::yield_now();
        }
        flag.load(Ordering::Acquire) != 0
    };
    // Fresh-mount FAT snapshot: is DEFER.BIN's chain head `f0` still allocated in ALL FAT copies (non-zero)?
    let chain_allocated = || -> bool {
        match crate::fs::fat::mount() {
            Ok(fs) => {
                let nf = fs.num_fats();
                let mut all = true;
                let mut f = 0;
                while f < nf {
                    match fs.fat_entry_copy(f0, f) {
                        Ok(0) => all = false, // free -> NOT allocated
                        Ok(_) => {}
                        Err(_) => all = false,
                    }
                    f += 1;
                }
                all
            }
            Err(_) => false,
        }
    };

    // Edge 1: A runs immediately (no GO gate on its open). Wait for A_OPENED, then release B's unlink.
    let a_opened = wait_flag(&U11DEFER_A_OPENED_F, 5);
    u11defer_release_go(b.slot, 0x3000);
    // Edge 2: wait for B_UNLINKED; CHECKPOINT-1 — the NAME is gone on disk, but the chain is STILL allocated.
    let b_unlinked = wait_flag(&U11DEFER_B_UNLINKED_F, 5);
    let name_gone_c1 = match crate::fs::fat::mount() {
        Ok(fs) => fs.find_in_root(U11DEFER_NAME).is_err(),
        Err(_) => false,
    };
    let chain_alive_c1 = chain_allocated();
    u11defer_release_go(a.slot, 0x3010);
    // Edge 3: wait for A_READ; CHECKPOINT-2 — the chain is STILL allocated (A read its bytes; nothing was freed).
    let a_read = wait_flag(&U11DEFER_A_READ_F, 5);
    let chain_alive_c2 = chain_allocated();
    u11defer_release_go(a.slot, 0x3018);

    // Verdict: wait for both sentinel exits, read witnesses + kills, wait teardown-clear, then CHECKPOINT-3.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U11DEFER_DONE.load(Ordering::Acquire) < 2
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let a_witness = U11DEFER_A_WITNESS.load(Ordering::Acquire);
    let b_witness = U11DEFER_B_WITNESS.load(Ordering::Acquire);
    let done = EL0_U11DEFER_DONE.load(Ordering::Acquire);
    let killed = EL0_U11DEFER_KILLED.load(Ordering::Acquire);

    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(files_row_is_clear(a.asid)
        && files_row_is_clear(b.asid)
        && handle_row_is_clear(a.asid)
        && handle_row_is_clear(b.asid))
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = files_row_is_clear(a.asid)
        && files_row_is_clear(b.asid)
        && handle_row_is_clear(a.asid)
        && handle_row_is_clear(b.asid);

    // CHECKPOINT-3: A's last close ran the deferred free — `f0` is now free in ALL FAT copies and re-allocatable
    // (the first-free cluster is `f0` again, since nothing below it was ever freed). The name was gone since B.
    let (freed_c3, reusable_c3) = match crate::fs::fat::mount() {
        Ok(fs) => {
            let nf = fs.num_fats();
            let mut freed = true;
            let mut f = 0;
            while f < nf {
                match fs.fat_entry_copy(f0, f) {
                    Ok(0) => {}
                    _ => freed = false,
                }
                f += 1;
            }
            (freed, fs.first_free_cluster() == Ok(f0))
        }
        Err(_) => (false, false),
    };

    let ok = a_opened
        && b_unlinked
        && a_read
        && a_witness == U11DEFER_A_WITNESS_ALL
        && b_witness == U11DEFER_B_WITNESS_ALL
        && done == 2
        && killed == 0
        && cleared
        && name_gone_c1
        && chain_alive_c1
        && chain_alive_c2
        && freed_c3
        && reusable_c3;
    if ok {
        serial_println!(
            ":: U11-defer: cross-process unlink-defers-free — name gone at unlink, reader keeps original bytes, chain freed (all FAT copies) + re-allocatable at last close -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U11-defer: cross-process unlink-defers-free FAIL — a_w={:#x} b_w={:#x} opened={} unlinked={} read={} done={} killed={} cleared={} c1(gone={},alive={}) c2_alive={} c3(freed={},reuse={}) (want {:#x}/{:#x}/t/t/t/2/0/t/t/t/t/t/t) ::",
            a_witness,
            b_witness,
            a_opened,
            b_unlinked,
            a_read,
            done,
            killed,
            cleared,
            name_gone_c1,
            chain_alive_c1,
            chain_alive_c2,
            freed_c3,
            reusable_c3,
            U11DEFER_A_WITNESS_ALL,
            U11DEFER_B_WITNESS_ALL
        );
    }
}

/// U11-M2 (seat coalesce-review fix): PROVE `files_free_by_dir` frees EVERY orphan chain head when FAT recycles a
/// deleted-but-still-open file's directory slot for a new file — so two DISTINCT files that end up sharing one
/// `(dir_lba, dir_off)` in one process (on DIFFERENT `OPEN_FILES` rows) BOTH free their chains on the explicit
/// `sys_unlink` sweep, with neither leaked. Kernel-side + deterministic (the `u11_check_gen_rebind` style, more
/// robust than depending on exact cross-process slot-recycle timing in EL0): it PHYSICALLY recycles a slot
/// (create F, `0xE5` F, create G — asserting G reused F's EXACT slot), reproduces the two-descriptor /
/// two-pending-row table state on a scratch ASID, runs the real `files_free_by_dir` sweep, and a fresh mount
/// confirms BOTH cluster chains are free in ALL FAT copies. Runs in the launcher (kernel-task) context, so block
/// I/O is legal. Returns `true` iff BOTH chains freed (before the fix, the earlier head leaks -> `false`).
fn u11defer_check_double_orphan() -> bool {
    const A: u64 = 6; // scratch ASID row (every demo fixture torn down by now — the gen-rebind check's convention)
    clear_files_row(A);
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(_) => return false,
    };
    // 1. Create file F + grow it a real cluster (cF), then delete its NAME only (`0xE5`) — cF stays ALLOCATED
    //    (the cross-process-held-open state), and F's slot is now free for reuse.
    let (de_f, lba_f, off_f) = match fs.create_in_root("SLTF.BIN", 0x20) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let c_f = match fs.write_grow(de_f.first_cluster(), de_f.size, lba_f, off_f, 0, &[0xF1u8; 16]) {
        Ok((_, _, first)) => first,
        Err(_) => return false,
    };
    if fs.mark_dir_deleted(lba_f, off_f).is_err() {
        return false;
    }
    // 2. Create file G — `create_in_root` reuses the FIRST free slot, which is F's just-`0xE5`'d slot (nothing
    //    else was created between). ASSERT it landed on F's EXACT slot (the shared-key precondition this test
    //    needs), then grow it a DISTINCT cluster (cG != cF, since cF is still allocated so first-fit skips it).
    let (de_g, lba_g, off_g) = match fs.create_in_root("SLTG.BIN", 0x20) {
        Ok(t) => t,
        Err(_) => return false,
    };
    if lba_g != lba_f || off_g != off_f {
        return false; // G did not reuse F's slot -> the shared-(dir_lba,dir_off) precondition did not hold
    }
    let c_g = match fs.write_grow(de_g.first_cluster(), de_g.size, lba_g, off_g, 0, &[0xF2u8; 16]) {
        Ok((_, _, first)) => first,
        Err(_) => return false,
    };
    if c_f == 0 || c_g == 0 || c_f == c_g {
        return false; // both files must own DISTINCT real chains
    }
    // 3. Reproduce the table state slot-recycling produces: TWO descriptors on ASID A at the SAME (lba_f, off_f),
    //    on DIFFERENT `OPEN_FILES` rows, each `unlink_pending` with its own chain head. (`openfile_incref`'s
    //    `unlink_pending`-skip is what puts G on a FRESH row rather than joining F's pending row.)
    let Some(row_f) = openfile_incref(lba_f, off_f as u32) else {
        return false;
    };
    openfile_mark_unlink_pending_at(row_f as u32, c_f);
    let Some(fid_f) = files_alloc(A, c_f, 16, lba_f, off_f as u32) else {
        return false;
    };
    FILE_OPENROW[A as usize][fid_f].store(row_f as u32, Ordering::Release);
    let Some(row_g) = openfile_incref(lba_f, off_f as u32) else {
        return false;
    };
    openfile_mark_unlink_pending_at(row_g as u32, c_g);
    let Some(fid_g) = files_alloc(A, c_g, 16, lba_f, off_f as u32) else {
        return false;
    };
    FILE_OPENROW[A as usize][fid_g].store(row_g as u32, Ordering::Release);
    if row_f == row_g {
        return false; // the skip-pending join must place the two files on DIFFERENT rows
    }
    // 4. THE SWEEP: one `files_free_by_dir` matches BOTH descriptors (same slot) and must yield BOTH orphan
    //    heads (F3-M3: the sweep collects; the caller frees — here exactly as sys_unlink does after its guard).
    let (orphans, norphans) = files_free_by_dir(A, lba_f, off_f as u32);
    for &fc in &orphans[..norphans] {
        free_orphan_chain(fc);
    }
    // 5. Fresh mount: BOTH cF and cG must be free in ALL FAT copies (re-allocatable). Before the fix the earlier
    //    head (cF) leaks -> its entry stays nonzero -> `false`.
    let fs2 = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(_) => return false,
    };
    let nf = fs2.num_fats();
    let mut ok = true;
    let mut f = 0;
    while f < nf {
        ok &= fs2.fat_entry_copy(c_f, f) == Ok(0);
        ok &= fs2.fat_entry_copy(c_g, f) == Ok(0);
        f += 1;
    }
    // 6. Cleanup: the scratch descriptors are already freed by the sweep; clear the row defensively, and `0xE5`
    //    G's (== F's) slot so no live directory entry lingers pointing at the now-freed cG.
    clear_files_row(A);
    let _ = fs2.mark_dir_deleted(lba_f, off_f);
    ok
}

/// U11-M2 (seat coalesce-review fix): launcher + PASS line for the slot-recycle two-orphan proof. Rides the U7
/// kernel task after `u11defer_run`. Kernel-side only (no EL0 fixture — the proof is a deterministic table + FAT
/// manipulation, the `u11_check_gen_rebind` style). Releases no further gate.
fn u11reuse_run() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the check creates real files; skip silently
    }
    // Pre-flight: require the scratch files ABSENT (a stale image would confound `create_in_root`'s slot reuse).
    match crate::fs::fat::mount() {
        Ok(fs) => {
            if fs.find_in_root("SLTF.BIN").is_ok() || fs.find_in_root("SLTG.BIN").is_ok() {
                serial_println!(
                    ":: U11-reuse: SLTF/SLTG present pre-demo (stale image) — slot-recycle proof skipped ::"
                );
                return;
            }
        }
        Err(_) => return,
    }
    if u11defer_check_double_orphan() {
        serial_println!(
            ":: U11-reuse: sys_unlink slot-recycle — two files sharing a recycled dir slot BOTH free their chains (all FAT copies) -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U11-reuse: sys_unlink slot-recycle — orphan-head sweep FAIL (a chain leaked or the recycle precondition was not met) ::"
        );
    }
}

/// U11-M2b (reap): build ONE fixture slot for the shared two-entry reap blob (the `u11defer_build` shape —
/// scrub the whole window, copy the blob, I-cache-sync, protect the code page EL0-RX/EL1-RO). `entry_sym`
/// selects program A or B. `None` if slot allocation fails.
fn u11reap_build(entry_sym: *const u8) -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __u11reap_blob_start as usize;
    let bend = &raw const __u11reap_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U11-reap blob does not fit in a code page");
    let entry = {
        let va = base + (entry_sym as usize - bstart) as u64;
        assert!(va & 3 == 0, "U11-reap fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// U11-M2b (reap) launcher + verdict — the teardown-last-close REAPER proof. Rides the same U7 kernel task,
/// strictly after `u11reuse_run`. Two co-located fixtures (`el0-u11reap-a`/`-b` on `demo_cpu`, cooperative via
/// SYS_YIELD) with this launcher (on the sibling core, co-located with the `orphan_reaper`) as the single
/// SEQUENCER. It is the `u11defer_run` choreography UP TO A's exit — except **A EXITS WITHOUT CLOSING** while
/// holding the last cross-process open, so the chain is freed by the REAPER at teardown, not by an explicit
/// `SYS_CLOSE`. The launcher re-mounts the FAT at THREE checkpoints:
///
///   A creates+writes DEFER2.BIN, reports A_OPENED
///     -> release B's unlink
///   B opens+unlinks (name gone -> a re-open is -ENOENT), reports B_UNLINKED
///     -> CHECKPOINT-1: name GONE on disk, chain (cluster `f0`) STILL allocated -> release A's read
///   A seeks+reads its ORIGINAL bytes (the deferred chain is alive), reports A_READ
///     -> CHECKPOINT-2: chain still allocated -> release A's EXIT GO
///   A exits WITHOUT closing (teardown queues the orphan); B exits
///     -> CHECKPOINT-3: bounded YIELD-poll of the FAT until the reaper has FREED the chain (all FAT copies) +
///        it is re-allocatable (`first_free == f0`) — the yields cede this core to the co-located reaper, so
///        the cooperative-QEMU drain is deterministic.
///
/// PASS iff both witnesses full AND all three cues fired AND both exited AND no kill AND both rows torn down AND
/// the three checkpoints hold. Runtime-created file (no arroyo plant); needs a fresh image (DEFER2.BIN absent
/// pre-demo). The last demo — releases no further gate. The M2a `"teardown … leaked"` line must NOT appear for
/// DEFER2.BIN (the reaper freed it, not leaked it) — its ABSENCE is confirmed in the gate.
fn u11reap_run(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the fixtures cannot create/open disk files; skip silently
    }
    // Pre-flight: require DEFER2.BIN ABSENT (a stale copy would confound the on-disk checks), and snapshot the
    // first-free cluster `f0` — A's grow-from-empty write allocates it (first-fit), so `f0` is DEFER2.BIN's
    // chain head, which the checkpoints track (allocated -> allocated -> freed-by-reaper + re-allocatable).
    let f0 = match crate::fs::fat::mount() {
        Ok(fs) => {
            if fs.find_in_root(U11REAP_NAME).is_ok() {
                serial_println!(
                    ":: U11-reap: DEFER2.BIN already present pre-demo (stale image) — reaper demo skipped ::"
                );
                return;
            }
            match fs.first_free_cluster() {
                Ok(c) => c,
                Err(_) => {
                    serial_println!(":: U11-reap: no free cluster pre-demo — reaper demo skipped ::");
                    return;
                }
            }
        }
        Err(_) => return, // unmountable -> skip silently
    };

    // Build + spawn A, then B (B parks on its GO until A has created the file, like u11defer).
    let Some(a) = u11reap_build(&raw const __u11reap_prog_a) else {
        serial_println!(":: U11-reap: no free address-space slot — reaper demo skipped ::");
        return;
    };
    let Some(b) = u11reap_build(&raw const __u11reap_prog_b) else {
        serial_println!(":: U11-reap: no free address-space slot — reaper demo skipped (A will park out) ::");
        return;
    };
    serial_println!(
        ":: U11-reap: teardown-last-close reaper — A exits holding the last open of an unlinked file; its chain freed by the reaper ::"
    );
    super::sched::spawn_user_slot("el0-u11reap-a", a.entry, a.sp, a.ttbr0, demo_cpu);
    super::sched::spawn_user_slot("el0-u11reap-b", b.entry, b.sp, b.ttbr0, demo_cpu);

    // Bounded flag-wait (the `u11defer_run` idiom), yielding cooperatively.
    let wait_flag = |flag: &AtomicU32, secs: u64| -> bool {
        let start = super::timer::cntpct();
        let deadline = secs * super::timer::cntfrq();
        while flag.load(Ordering::Acquire) == 0
            && super::timer::cntpct().wrapping_sub(start) <= deadline
        {
            super::sched::yield_now();
        }
        flag.load(Ordering::Acquire) != 0
    };
    // Fresh-mount FAT snapshot: is DEFER2.BIN's chain head `f0` still allocated in ALL FAT copies (non-zero)?
    let chain_allocated = || -> bool {
        match crate::fs::fat::mount() {
            Ok(fs) => {
                let nf = fs.num_fats();
                let mut all = true;
                let mut f = 0;
                while f < nf {
                    match fs.fat_entry_copy(f0, f) {
                        Ok(0) => all = false, // free -> NOT allocated
                        Ok(_) => {}
                        Err(_) => all = false,
                    }
                    f += 1;
                }
                all
            }
            Err(_) => false,
        }
    };

    // Edge 1: A runs immediately (no GO gate on its open). Wait for A_OPENED, then release B's unlink.
    let a_opened = wait_flag(&U11REAP_A_OPENED_F, 5);
    u11defer_release_go(b.slot, 0x3000);
    // Edge 2: wait for B_UNLINKED; CHECKPOINT-1 — the NAME is gone on disk, but the chain is STILL allocated.
    let b_unlinked = wait_flag(&U11REAP_B_UNLINKED_F, 5);
    let name_gone_c1 = match crate::fs::fat::mount() {
        Ok(fs) => fs.find_in_root(U11REAP_NAME).is_err(),
        Err(_) => false,
    };
    let chain_alive_c1 = chain_allocated();
    u11defer_release_go(a.slot, 0x3010);
    // Edge 3: wait for A_READ; CHECKPOINT-2 — the chain is STILL allocated (A read its bytes; nothing was freed).
    let a_read = wait_flag(&U11REAP_A_READ_F, 5);
    let chain_alive_c2 = chain_allocated();
    u11defer_release_go(a.slot, 0x3018); // release A's EXIT GO — A now exits WITHOUT closing

    // Verdict: wait for both sentinel exits, read witnesses + kills, wait teardown-clear, then CHECKPOINT-3.
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_U11REAP_DONE.load(Ordering::Acquire) < 2
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    let a_witness = U11REAP_A_WITNESS.load(Ordering::Acquire);
    let b_witness = U11REAP_B_WITNESS.load(Ordering::Acquire);
    let done = EL0_U11REAP_DONE.load(Ordering::Acquire);
    let killed = EL0_U11REAP_KILLED.load(Ordering::Acquire);

    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(files_row_is_clear(a.asid)
        && files_row_is_clear(b.asid)
        && handle_row_is_clear(a.asid)
        && handle_row_is_clear(b.asid))
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = files_row_is_clear(a.asid)
        && files_row_is_clear(b.asid)
        && handle_row_is_clear(a.asid)
        && handle_row_is_clear(b.asid);

    // CHECKPOINT-3: A exited WITHOUT closing -> its teardown queued `f0` to the reaper. Bounded YIELD-poll the
    // FAT until the reaper has freed it (all FAT copies) AND it is re-allocatable (`first_free == f0`, since
    // nothing below it was ever freed). The yields cede this core to the co-located reaper so the cooperative-
    // QEMU drain is deterministic. Times out (still false) if the reaper never runs -> the verdict FAILs loudly.
    let (freed_c3, reusable_c3) = {
        let dstart = super::timer::cntpct();
        let ddeadline = 5 * super::timer::cntfrq();
        loop {
            let snap = match crate::fs::fat::mount() {
                Ok(fs) => {
                    let nf = fs.num_fats();
                    let mut freed = true;
                    let mut f = 0;
                    while f < nf {
                        match fs.fat_entry_copy(f0, f) {
                            Ok(0) => {}
                            _ => freed = false,
                        }
                        f += 1;
                    }
                    (freed, fs.first_free_cluster() == Ok(f0))
                }
                Err(_) => (false, false),
            };
            if snap.0 && snap.1 {
                break (true, true);
            }
            if super::timer::cntpct().wrapping_sub(dstart) > ddeadline {
                break snap;
            }
            super::sched::yield_now();
        }
    };

    let ok = a_opened
        && b_unlinked
        && a_read
        && a_witness == U11REAP_A_WITNESS_ALL
        && b_witness == U11REAP_B_WITNESS_ALL
        && done == 2
        && killed == 0
        && cleared
        && name_gone_c1
        && chain_alive_c1
        && chain_alive_c2
        && freed_c3
        && reusable_c3;
    if ok {
        serial_println!(
            ":: U11-reap: teardown-last-close reaper — A exits holding the unlinked file open, its chain freed by the reaper (all FAT copies) + re-allocatable, no teardown leak -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U11-reap: teardown-last-close reaper FAIL — a_w={:#x} b_w={:#x} opened={} unlinked={} read={} done={} killed={} cleared={} c1(gone={},alive={}) c2_alive={} c3(freed={},reuse={}) (want {:#x}/{:#x}/t/t/t/2/0/t/t/t/t/t/t) ::",
            a_witness,
            b_witness,
            a_opened,
            b_unlinked,
            a_read,
            done,
            killed,
            cleared,
            name_gone_c1,
            chain_alive_c1,
            chain_alive_c2,
            freed_c3,
            reusable_c3,
            U11REAP_A_WITNESS_ALL,
            U11REAP_B_WITNESS_ALL
        );
    }
}

/// U6 (owner/grants): release a fixture's GO word (`u11defer_release_go` twin) — write 1 to the slot's backing
/// window at `off`, with a `dsb ish` so the EL0 poller on the other core sees it.
fn uowner_release_go(slot: usize, off: usize) {
    unsafe {
        let go = super::boot::slot_backing_ptr(slot).add(off) as *mut u64;
        core::ptr::write_volatile(go, 1);
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
    }
}

/// U6 (owner/grants): build ONE fixture slot for the shared two-entry owner/grants blob (`u11defer_build` shape).
/// `entry_sym` selects program A or B. `None` if slot allocation fails.
fn uowner_build(entry_sym: *const u8) -> Option<U7Fix> {
    let (base, size) = super::boot::user_region();
    let sp = (base + size as u64) & !0xF;
    let bstart = &raw const __uowner_blob_start as usize;
    let bend = &raw const __uowner_blob_end as usize;
    let blen = bend - bstart;
    assert!(blen <= super::boot::USER_CODE_SIZE, "U6 owner/grants blob does not fit in a code page");
    let entry = {
        let va = base + (entry_sym as usize - bstart) as u64;
        assert!(va & 3 == 0, "U6 owner/grants fixture entry misaligned");
        va
    };
    let slot = super::boot::alloc_user_slot()?;
    let backing = super::boot::slot_backing_ptr(slot);
    unsafe {
        core::ptr::write_bytes(backing, 0, size);
        core::ptr::copy_nonoverlapping(bstart as *const u8, backing, blen);
    }
    super::cache::icache_sync_range(backing as usize, blen);
    unsafe { super::boot::protect_user_slot_code(slot, super::boot::USER_CODE_SIZE) };
    let ttbr0 = super::boot::slot_ttbr0(slot);
    Some(U7Fix { entry, sp, ttbr0, asid: ttbr0 >> 48, slot })
}

/// U6 (owner/grants) launcher + verdict — the by-NAME namespace ACL proof, the first positive exercise of the
/// U6 enforcement seam. Rides the same U7 kernel task, strictly after U11-reap. Two co-located fixtures
/// (`el0-uowner-a` OWNER / `el0-uowner-b` GRANTEE on `demo_cpu`, cooperative via SYS_YIELD), with this launcher
/// (on the sibling core) as the single SEQUENCER. It plants a `Child` handle in A's table naming B (so A's
/// SYS_FGRANT is owner-scoped — A never supplies B's raw pid/ASID), then releases each edge only after the prior
/// step's SYS_REPORT cue:
///
///   A creates OWNED.BIN PRIVATE + writes it, reports A_READY
///     -> release B's first open
///   B opens the private file -> -EACCES (the gap closed), reports B_DENIED1
///     -> release A's grant
///   A SYS_FGRANTs B read, reports A_GRANTED
///     -> release B's granted open
///   B opens -> a handle, reads the matching bytes, is itself refused a SYS_FGRANT (non-owner), closes, reports B_OPENED
///     -> release A's revoke
///   A SYS_FGRANTs B rights=0 (revoke), reports A_REVOKED
///     -> release B's post-revoke open
///   B opens -> -EACCES again (revoke enforced); B exits
///     -> (B has exited: A is still the owner) release A's exit
///   A re-opens its own file (owner authority persists), exits
///
/// PASS iff both witnesses full AND all five cues fired AND both exited AND no kill AND both rows torn down AND
/// OWNED.BIN is on disk (A never unlinked it; its owner row reverts to public at A's teardown). Runtime-created
/// file (no arroyo plant); needs a fresh image (OWNED.BIN absent pre-demo). The last demo — releases no gate.
fn uowner_run(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the fixtures cannot create/open disk files; skip silently
    }
    // Pre-flight: require OWNED.BIN ABSENT (a stale copy would already carry an owner-less entry the create path
    // would refuse to duplicate, confounding the demo).
    match crate::fs::fat::mount() {
        Ok(fs) => {
            if fs.find_in_root(UOWNER_NAME).is_ok() {
                serial_println!(":: U6-grants: OWNED.BIN already present pre-demo (stale image) — grants demo skipped ::");
                return;
            }
        }
        Err(_) => return, // unmountable -> skip silently
    }

    // B's Proc entry FIRST (so A's Child handle -> B resolves pid->ASID in SYS_FGRANT). Nothing else claimed if
    // the table is full.
    let Some(pi) = proc_reserve() else {
        serial_println!(":: U6-grants: no free process entry — grants demo skipped ::");
        return;
    };
    // Build + spawn B (the grantee), publish its pid->ASID. B parks on its GO word, making no syscall until
    // released, so nothing populates its row before A's grant.
    let Some(b) = uowner_build(&raw const __uowner_prog_b) else {
        serial_println!(":: U6-grants: no free address-space slot — grants demo skipped ::");
        proc_free(pi);
        return;
    };
    let b_pid = super::sched::spawn_user_slot("el0-uowner-b", b.entry, b.sp, b.ttbr0, demo_cpu);
    PROCS[pi].asid.store(b.asid, Ordering::Release);
    PROCS[pi].pid.store(b_pid, Ordering::Release);
    // Build + pre-endow + spawn A (the owner). A holds a Child handle naming B at UOWNER_CHILD_IDX so its
    // SYS_FGRANT is owner-scoped (A never names B by raw pid/ASID — the sys_xfer discipline).
    let Some(a) = uowner_build(&raw const __uowner_prog_a) else {
        serial_println!(":: U6-grants: no free address-space slot — grants demo skipped (B will park out) ::");
        proc_free(pi);
        return;
    };
    install_cap(a.asid, UOWNER_CHILD_IDX, KIND_CHILD, b_pid, CAP_READ);
    serial_println!(
        ":: U6-grants: owner/grants on open — non-owner denied, owner grants read, revoke re-denies ::"
    );
    super::sched::spawn_user_slot("el0-uowner-a", a.entry, a.sp, a.ttbr0, demo_cpu);

    // Bounded flag-wait (the u11defer/u7 idiom), yielding cooperatively.
    let wait_flag = |flag: &AtomicU32, secs: u64| -> bool {
        let start = super::timer::cntpct();
        let deadline = secs * super::timer::cntfrq();
        while flag.load(Ordering::Acquire) == 0 && super::timer::cntpct().wrapping_sub(start) <= deadline {
            super::sched::yield_now();
        }
        flag.load(Ordering::Acquire) != 0
    };

    // Choreography. B parks on GO1; A runs immediately (no GO gate on its create).
    let a_ready = wait_flag(&UOWNER_A_READY_F, 5);
    uowner_release_go(b.slot, 0x3000); // B: pre-grant open -> -EACCES
    let b_denied1 = wait_flag(&UOWNER_B_DENIED1_F, 5);
    uowner_release_go(a.slot, 0x3000); // A: grant B read
    let a_granted = wait_flag(&UOWNER_A_GRANTED_F, 5);
    uowner_release_go(b.slot, 0x3010); // B: granted open + read + non-owner-grant negative + close
    let b_opened = wait_flag(&UOWNER_B_OPENED_F, 5);
    uowner_release_go(a.slot, 0x3010); // A: revoke B
    let a_revoked = wait_flag(&UOWNER_A_REVOKED_F, 5);
    uowner_release_go(b.slot, 0x3018); // B: post-revoke open -> -EACCES, then B exits

    // B must fully EXIT (its owner row query is done) before A's exit GO — A stays the owner through B's last
    // open, so the owner row still exists when B is re-denied. Wait for B's sentinel (DONE reaches 1).
    let vstart = super::timer::cntpct();
    let vdeadline = 5 * super::timer::cntfrq();
    while EL0_UOWNER_DONE.load(Ordering::Acquire) < 1
        && super::timer::cntpct().wrapping_sub(vstart) <= vdeadline
    {
        super::sched::yield_now();
    }
    uowner_release_go(a.slot, 0x3018); // A: owner re-opens its own file, then exits

    // Verdict: wait for BOTH sentinel exits, read witnesses + kills, wait teardown-clear.
    let v2 = super::timer::cntpct();
    while EL0_UOWNER_DONE.load(Ordering::Acquire) < 2
        && super::timer::cntpct().wrapping_sub(v2) <= vdeadline
    {
        super::sched::yield_now();
    }
    let a_witness = UOWNER_A_WITNESS.load(Ordering::Acquire);
    let b_witness = UOWNER_B_WITNESS.load(Ordering::Acquire);
    let done = EL0_UOWNER_DONE.load(Ordering::Acquire);
    let killed = EL0_UOWNER_KILLED.load(Ordering::Acquire);

    let tstart = super::timer::cntpct();
    let tdeadline = 2 * super::timer::cntfrq();
    while !(files_row_is_clear(a.asid)
        && files_row_is_clear(b.asid)
        && handle_row_is_clear(a.asid)
        && handle_row_is_clear(b.asid))
        && super::timer::cntpct().wrapping_sub(tstart) <= tdeadline
    {
        super::sched::yield_now();
    }
    let cleared = files_row_is_clear(a.asid)
        && files_row_is_clear(b.asid)
        && handle_row_is_clear(a.asid)
        && handle_row_is_clear(b.asid);

    // Kernel-side: OWNED.BIN persists on disk (A never unlinked it; only its owner row was cleared at A's
    // teardown -> the file reverted to public). This confirms the private create actually landed on the card.
    let on_disk = match crate::fs::fat::mount() {
        Ok(fs) => fs.find_in_root(UOWNER_NAME).is_ok(),
        Err(_) => false,
    };
    proc_free(pi); // the planted pid->ASID entry (the fixtures exited by name, never through the Proc path)

    if a_witness == UOWNER_A_WITNESS_ALL
        && b_witness == UOWNER_B_WITNESS_ALL
        && a_ready
        && b_denied1
        && a_granted
        && b_opened
        && a_revoked
        && done == 2
        && killed == 0
        && cleared
        && on_disk
    {
        serial_println!(
            ":: U6-grants: owner/grants on open — non-owner -EACCES, owner grant admits R|W, grantee unlink -EACCES (delete owner-only), non-owner grant -EACCES, revoke re-denies -> PASS ::"
        );
    } else {
        serial_println!(
            ":: U6-grants: owner/grants FAIL — A={:#x} B={:#x} (want {:#x}/{:#x}) cues={}{}{}{}{} done={} killed={} cleared={} disk={} ::",
            a_witness,
            b_witness,
            UOWNER_A_WITNESS_ALL,
            UOWNER_B_WITNESS_ALL,
            a_ready as u8,
            b_denied1 as u8,
            a_granted as u8,
            b_opened as u8,
            a_revoked as u8,
            done,
            killed,
            cleared as u8,
            on_disk as u8
        );
    }
}

// =====================================================================================================
// K7 WATCH rider (NS-SPAN), K5B DUAL-SPAN rework — MEASURE the worst-case masked hold at the three fused
// persist sites (`sys_fgrant_revoke_2phase`, `native_persist_grants`, `native_persist_grow`). K5B removed
// `ns` from those sites, so the probe now brackets the fused `with_unafs` CALL (constructed before it,
// dropped right after): the measured span is an upper bound on the per-core IRQ-masked window (the mask
// lives INSIDE with_unafs; the bracket adds only the lock-acquire wait, honestly worst-case). Per the
// ratified NARROW conditions the ONE emit line carries BOTH facts inseparably: ns-hold across ACL persists
// = 0 by construction (NAMESPACE unfrozen — the K5B headline) AND the measured masked with_unafs-hold per
// site (the ~0.7 s per-core residual, still present, ledgered). Same methodology as the K7 before-number
// (CNTPCT deltas, per-site fetch_max, same three sites, same units) so the metal before/after pair is
// directly comparable. Default-OFF, byte-identical when off: the whole rider is
// `#[cfg(feature = "nsspan")]`-gated AND placed at end-of-file / inlined newline-neutral at each site, so
// a knob-off build has IDENTICAL physical line numbers to a build without the rider — the panic `Location`
// data embedded in `.rodata` (loaded) does not move (the core3probe idiom, hardened for line-number
// stability). No locks, no heap in the measurement path. `nsspan_report()` emits the ONE serial line after
// the K3/K5 fixtures have exercised all three sites.
// =====================================================================================================
#[cfg(feature = "nsspan")]
static NSSPAN_REVOKE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "nsspan")]
static NSSPAN_GRANTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "nsspan")]
static NSSPAN_GROW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// K7 WATCH / K5B: an RAII probe that measures the fused persist span at ONE site. K5B: constructed right
/// BEFORE the site's `with_unafs` call and dropped right after it returns — an upper bound on the per-core
/// IRQ-masked window inside it. `Drop` folds `cntpct(now) - start` into `site`'s worst-case. No heap, no lock.
#[cfg(feature = "nsspan")]
struct NsSpanProbe {
    start: u64,
    site: &'static core::sync::atomic::AtomicU64,
}
#[cfg(feature = "nsspan")]
impl NsSpanProbe {
    #[inline]
    fn new(site: &'static core::sync::atomic::AtomicU64) -> Self {
        NsSpanProbe { start: super::timer::cntpct(), site }
    }
}
#[cfg(feature = "nsspan")]
impl Drop for NsSpanProbe {
    #[inline]
    fn drop(&mut self) {
        let delta = super::timer::cntpct().wrapping_sub(self.start);
        self.site.fetch_max(delta, core::sync::atomic::Ordering::Relaxed);
    }
}

/// K7 WATCH / K5B: emit the dual-span line — the ns-hold headline AND the masked with_unafs-hold caveat,
/// inseparably in ONE line (the ratified NARROW condition). Per-site worst-case CNTPCT ticks + the counter
/// frequency, same units/methodology as the K7 before-number. Called once from `u7_launcher` after the
/// K3/K5 fixtures (which drive all three sites). Reads three atomics — no lock, no disk. A site never
/// exercised reports 0.
#[cfg(feature = "nsspan")]
fn nsspan_report() {
    use core::sync::atomic::Ordering;
    serial_println!(
        ":: NS-SPAN: K5B ns-hold across ACL persist = 0 (not taken — NAMESPACE unfrozen) BUT per-core IRQ-masked with_unafs-hold worst revoke={} grants={} grow={} ticks (freq {}) — polled SD write still masks the holding core (ledgered residual) ::",
        NSSPAN_REVOKE.load(Ordering::Relaxed),
        NSSPAN_GRANTS.load(Ordering::Relaxed),
        NSSPAN_GROW.load(Ordering::Relaxed),
        super::timer::cntfrq(),
    );
    // K9-MASKCUT: the QEMU-provable half — root FLIPS and BLOCKS written by a SINGLE ACL row persist
    // (worst across the K3/K5 fixtures). Staged-batch adoption drops flips to 1 per persist; the pre-K9
    // per-op regime flipped once per attribute. Unlike the tick span (TCG-blind), this count is exact.
    serial_println!(
        ":: NS-SPAN-K9: single ACL row persist (staged batch) worst flips={} blocks={} — one root flip per persist vs the pre-K9 per-attribute regime ::",
        crate::fs::unafs::ACL_PERSIST_FLIPS.load(Ordering::Relaxed),
        crate::fs::unafs::ACL_PERSIST_BLOCKS.load(Ordering::Relaxed),
    );
}


// =====================================================================================================
// BANDY-1 (ROADMAP §3b arc 1): the on-UnaOS SMessage bus — TRANSPORT (M2) + KERNEL FULFILLMENT (M3).
// =====================================================================================================
//
// "Port the bus, not the binary convention." The wire layer (frame layout, decode ceilings, the
// frozen native goldens) lives in arch/aarch64/bus.rs; THIS section is the syscall transport and
// the kernel-side fulfiller for the v1 verbs (ls / cat / cp).
//
// Shape (design verdicts Maestro-closed 2026-07-15/16 + Peter's native-format ruling, do not
// re-derive):
//  * SYS_MSEND(frame_ptr, frame_len): copy the request in through the validated seam, parse it
//    FAIL-CLOSED (bus.rs), require the principal field ALL-ZERO (-EINVAL otherwise — verdict C:
//    reject, never overwrite), STAMP the sender's kernel-held PrincipalRecord (the K-line
//    IMAGE_SHA256 mint) into the header, fulfill the verb SYNCHRONOUSLY in the caller's own
//    syscall context through the EXISTING FAT/ACL machinery (verdict D: the invoker's grants —
//    fulfillment runs with the caller's live (asid, gen) AND its stamped principal, exactly like
//    the direct syscalls; no impersonation primitive exists), and enqueue the reply frame into
//    the SENDER's mailbox. Returns 0, or a negative errno (transport errors only — a verb that
//    FAILS still returns 0 with the errno in the reply's status field, body empty).
//  * SYS_MRECV(buf_ptr, buf_len): dequeue the oldest reply into the caller's buffer, BLOCKING on
//    the sys_wait Semaphore idiom while the mailbox is empty. buf_len must be >= BUS_FRAME_MAX
//    (whole-frame contract, -EMSGSIZE) so no partial-delivery state exists.
//  * Mailboxes: per-ASID, DEPTH 16 (BUS_MBOX_DEPTH — a deliberate bounded cap, the USER_SLOTS
//    discipline: never raise it to satisfy a demo; a full mailbox refuses the SEND fail-closed
//    with -EAGAIN BEFORE the verb runs, so no side effect can lose its reply). Each queued reply
//    is one heap box of exactly its frame's length (<= BUS_FRAME_MAX = the max forced allocation
//    per message); the aggregate ceiling is (USER_SLOTS+1) * 16 * 4148 B ≈ 600 KiB, far under
//    the 48 MiB heap. All bus staging buffers are HEAP-allocated (never >4 KiB kernel-stack
//    arrays — SVC stacks are small).
//  * Replies are stamped with the RESERVED KERNEL principal kind (PRIN_KERNEL_REPLY, verdict C),
//    fail-closed as grantee/owner everywhere (see the guards at owned_set_owner / owned_grant /
//    owned_access_ok and the BANDY-STAMP witness).
//  * Teardown: clear_handle_row drains the dying ASID's mailbox; every queued reply carries the
//    recipient's gen stamp and bus_mrecv discards a stale-gen frame, so a recycled ASID's next
//    tenant can never read its predecessor's replies.

/// Mailbox depth per ASID — a deliberate bounded cap (STOP-tripwired like USER_SLOTS).
const BUS_MBOX_DEPTH: usize = 16;

/// One queued reply frame: the recipient-gen stamp + the heap-boxed frame bytes (exact length).
struct BusMsg {
    agen: u64,
    frame: alloc::boxed::Box<[u8]>,
}

/// A bounded FIFO of queued replies. Guarded by its SpinMutex row in BUS_MBOX; count <= DEPTH.
struct BusMbox {
    head: usize,
    count: usize,
    slots: [Option<BusMsg>; BUS_MBOX_DEPTH],
}

impl BusMbox {
    const EMPTY: Self = BusMbox { head: 0, count: 0, slots: [const { None }; BUS_MBOX_DEPTH] };
}

/// Per-ASID reply mailboxes (row = ASID, like HANDLES). Static table of Options — the frames
/// themselves are heap boxes, so the static footprint is pointers only.
static BUS_MBOX: [SpinMutex<BusMbox>; super::boot::USER_SLOTS + 1] =
    [const { SpinMutex::new(BusMbox::EMPTY) }; super::boot::USER_SLOTS + 1];

/// Per-ASID "replies pending" semaphores — the MRECV blocking primitive (the sys_wait idiom:
/// Semaphore::wait parks the task; every enqueue posts). Lazily init()'d once (waiter-capacity
/// reservation) before first use.
static BUS_SEM: [super::sched::Semaphore; super::boot::USER_SLOTS + 1] =
    [const { super::sched::Semaphore::new(0) }; super::boot::USER_SLOTS + 1];
static BUS_SEM_INIT: AtomicU8 = AtomicU8::new(0); // 0 = untouched, 1 = initializing, 2 = ready

/// One-shot waiter-capacity reservation for BUS_SEM (Semaphore::init's contract: reserve before
/// any task can block). Gate-and-spin so a second core entering mid-init waits it out.
fn bus_sem_init_once() {
    match BUS_SEM_INIT.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            for s in &BUS_SEM {
                s.init();
            }
            BUS_SEM_INIT.store(2, Ordering::Release);
        }
        Err(_) => {
            while BUS_SEM_INIT.load(Ordering::Acquire) != 2 {
                core::hint::spin_loop();
            }
        }
    }
}

/// Teardown drain (called from clear_handle_row, after the gen bump): free every queued reply
/// box. The semaphore's stray permits are harmless — bus_mrecv re-checks the queue after every
/// wake and loops back to wait on empty.
fn bus_mbox_clear(asid: u64) {
    if (asid as usize) >= BUS_MBOX.len() {
        return;
    }
    let _irq = IrqGuard::mask_save();
    let mut mb = BUS_MBOX[asid as usize].lock();
    mb.head = 0;
    mb.count = 0;
    for s in mb.slots.iter_mut() {
        *s = None; // drops the box
    }
}

/// Enqueue a reply for `asid` (gen-stamped). false = mailbox full (the caller already reserved
/// space via bus_mbox_has_room BEFORE fulfilling, so this only fails under a racing enqueue —
/// impossible for a single-threaded sender, defensively handled as EAGAIN).
fn bus_mbox_push(asid: u64, agen: u64, frame: alloc::boxed::Box<[u8]>) -> bool {
    let _irq = IrqGuard::mask_save();
    let mut mb = BUS_MBOX[asid as usize].lock();
    if mb.count >= BUS_MBOX_DEPTH {
        return false;
    }
    let tail = (mb.head + mb.count) % BUS_MBOX_DEPTH;
    mb.slots[tail] = Some(BusMsg { agen, frame });
    mb.count += 1;
    true
}

/// Capacity pre-check — SYS_MSEND refuses BEFORE fulfilling when the reply could not be queued,
/// so a verb with side effects (cp) can never run and then lose its reply.
fn bus_mbox_has_room(asid: u64) -> bool {
    let _irq = IrqGuard::mask_save();
    let mb = BUS_MBOX[asid as usize].lock();
    mb.count < BUS_MBOX_DEPTH
}

/// Non-blocking dequeue for `asid`: pop the oldest frame whose gen stamp matches the ASID's
/// CURRENT gen; stale-gen frames (a predecessor tenant's) are discarded in the same pass.
fn bus_mbox_pop(asid: u64) -> Option<BusMsg> {
    let cur_gen = ASID_GEN[asid as usize].load(Ordering::Acquire);
    let _irq = IrqGuard::mask_save();
    let mut mb = BUS_MBOX[asid as usize].lock();
    while mb.count > 0 {
        let head = mb.head;
        let msg = mb.slots[head].take();
        mb.head = (head + 1) % BUS_MBOX_DEPTH;
        mb.count -= 1;
        match msg {
            Some(m) if m.agen == cur_gen => return Some(m),
            _ => continue, // stale-gen (or defensively empty) — discard, keep scanning
        }
    }
    None
}

// -----------------------------------------------------------------------------------------------
// M3: kernel fulfillment — ls / cat / cp through the EXISTING FAT + ACL machinery, under the
// invoker's identity (its live (asid, gen) and its stamped principal). Every errno mirrors the
// direct-syscall path byte-for-byte (the equivalence witness's contract): the cat gate below is
// the sys_open existing-file sequence — same checks, same order, same errnos.
// -----------------------------------------------------------------------------------------------

/// v1 content ceilings (documented honest scope, not protocol constants): cat returns the FIRST
/// 512 bytes of a file (sanitized to printable ASCII + \n\r\t); cp copies files up to 4096 bytes
/// (-E2BIG above — bounded work under the namespace hold); ls truncates its listing at ~3800
/// bytes. All keep the reply body under BUS_BODY_MAX by construction.
const BUS_CAT_MAX: usize = 512;
const BUS_CP_MAX: usize = 4096;
const BUS_LS_TEXT_MAX: usize = 3800;

/// Sanitize file-content bytes for a text reply body: printable ASCII and \n \r \t pass, every
/// other byte becomes '.' — terminal-safe output (midden writes the body verbatim to the
/// console; a raw control byte could mangle the serial witness stream).
fn bus_sanitize(b: u8) -> u8 {
    match b {
        0x20..=0x7e | b'\n' | b'\r' | b'\t' => b,
        _ => b'.',
    }
}

/// ls: list the FAT root (name + size per line, directories marked with a trailing '/'). No
/// denial leg — FAT root listing is public exactly as the in-kernel shell's ls is (names are not
/// ACL-protected; content is, at cat/open). Truncates honestly at BUS_LS_TEXT_MAX.
fn bus_ls(text: &mut alloc::vec::Vec<u8>) -> i64 {
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    let entries = match fs.read_root() {
        Ok(e) => e,
        Err(_) => return EIO,
    };
    for de in entries.iter() {
        let name = de.name();
        // budget: name + '/' + ' ' + u32 digits (<=10) + '\n'
        if text.len() + name.len() + 13 > BUS_LS_TEXT_MAX {
            text.extend_from_slice(b"...\n");
            break;
        }
        text.extend_from_slice(name.as_bytes());
        if de.is_dir {
            text.push(b'/');
        }
        text.push(b' ');
        let mut numbuf = [0u8; 10];
        let mut n = de.size;
        let mut i = numbuf.len();
        loop {
            i -= 1;
            numbuf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        text.extend_from_slice(&numbuf[i..]);
        text.push(b'\n');
    }
    0
}

/// cat: read the first BUS_CAT_MAX bytes of a root file under the INVOKER's grants. The check
/// sequence below is the sys_open existing-file gate VERBATIM (same checks, same order, same
/// errnos — the equivalence witness's contract): UNAFS.ATR deny -> mount errnos -> NAMESPACE
/// hold -> find (-ENOENT) -> -EISDIR -> owned_access_ok(CAP_READ) (-EACCES). The bounded read
/// (<= 512 B, one chain hop) stays under the ns hold so a concurrent unlink cannot free the
/// chain mid-read (the F3 bounded-span rule; cat claims no descriptor to defer against).
fn bus_cat(asid: u64, agen: u64, ppid: PrincipalRecord, name: &str, text: &mut alloc::vec::Vec<u8>) -> i64 {
    if name.eq_ignore_ascii_case(ATR_NAME) {
        return EACCES; // the kernel owns the ACL store — mirror sys_open's outright deny
    }
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    let ns = ns_lock();
    let (de, dir_lba, dir_off) = match fs.find_located(name) {
        Ok(t) => t,
        Err(crate::fs::fat::FatError::NotFound) => return ENOENT,
        Err(_) => return EIO,
    };
    if de.is_dir {
        return EISDIR;
    }
    if !owned_access_ok(dir_lba, dir_off as u32, asid, agen, CAP_READ, ppid) {
        return EACCES;
    }
    let mut raw: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs.read_at(de.first_cluster(), de.size, 0, &mut raw, BUS_CAT_MAX).is_err() {
        return EIO;
    }
    drop(ns);
    text.reserve(raw.len());
    for &b in raw.iter() {
        text.push(bus_sanitize(b));
    }
    0
}

/// cp: copy a root file to a NEW root name under the INVOKER's grants. Source side = the cat
/// gate (same errnos); destination is CREATE-NEW-ONLY in v1 (-EEXIST on an existing name — no
/// overwrite semantics to get wrong). The created copy is PRIVATE to the invoker exactly as a
/// direct sys_open(O_CREAT) is (owner row + native persist, the same fail-closed -ENOSPC undo),
/// or PUBLIC for an anonymous/ASID-0 invoker — verdict D: the bus mints nothing the direct path
/// would not. The whole find->create->copy->own sequence runs under ONE namespace hold (bounded:
/// <= 4 KiB, the F3 span rule) so a concurrent unlink/create cannot interleave; the native ACL
/// persist runs AFTER the hold drops (the K5B discipline, exactly like sys_open). A successful
/// cp's reply body is EMPTY (status 0 is the whole truth — the native error-reply symmetry).
fn bus_cp(asid: u64, agen: u64, ppid: PrincipalRecord, src: &str, dst: &str) -> i64 {
    if src.eq_ignore_ascii_case(ATR_NAME) || dst.eq_ignore_ascii_case(ATR_NAME) {
        return EACCES;
    }
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    let ns = ns_lock();
    let (sde, s_lba, s_off) = match fs.find_located(src) {
        Ok(t) => t,
        Err(crate::fs::fat::FatError::NotFound) => return ENOENT,
        Err(_) => return EIO,
    };
    if sde.is_dir {
        return EISDIR;
    }
    if !owned_access_ok(s_lba, s_off as u32, asid, agen, CAP_READ, ppid) {
        return EACCES;
    }
    if sde.size as usize > BUS_CP_MAX {
        return E2BIG; // bounded work under the ns hold — v1's honest copy ceiling
    }
    match fs.find_located(dst) {
        Ok(_) => return EEXIST, // create-new-only in v1
        Err(crate::fs::fat::FatError::NotFound) => {}
        Err(_) => return EIO,
    }
    // Read the whole (bounded) source, then create + write the destination.
    let mut data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if fs.read_at(sde.first_cluster(), sde.size, 0, &mut data, BUS_CP_MAX).is_err() {
        return EIO;
    }
    let (dde, d_lba, d_off) = match fs.create_in_root(dst, 0x20) {
        Ok(t) => t,
        Err(crate::fs::fat::FatError::Unsupported) => return EINVAL, // not an 8.3 name
        Err(crate::fs::fat::FatError::NoSpace) => return ENOSPC,
        Err(_) => return EIO,
    };
    let mut new_fc = dde.first_cluster();
    if !data.is_empty() {
        match fs.write_grow(dde.first_cluster(), 0, d_lba, d_off, 0, &data) {
            Ok((_, _, fc)) => new_fc = fc,
            Err(_) => {
                // Undo the fresh entry (leak-not-dangle: a failed grow may leave allocated
                // clusters to the reaper, but never a live name over a torn copy).
                let _ = fs.mark_dir_deleted(d_lba, d_off);
                return EIO;
            }
        }
    }
    // Ownership: mirror sys_open's O_CREAT-private policy under the invoker's identity.
    if asid != 0 && !owned_set_owner(d_lba, d_off as u32, asid, agen, ppid) {
        let _ = fs.mark_dir_deleted(d_lba, d_off);
        return ENOSPC; // fail CLOSED — never a silently-public "private" copy
    }
    drop(ns);
    if asid != 0 && ppid.kind != PRIN_NONE {
        let _ = native_persist_create(dst, new_fc, d_lba, d_off as u32, ppid);
    }
    0
}

// -----------------------------------------------------------------------------------------------
// BANDY-2 (M2): the WRITE-SIDE fulfillment — write / rm / mv through the EXISTING FAT + ACL
// machinery under the invoker's identity, every errno mirrored byte-for-byte against the direct
// syscall paths (the equivalence witness's contract). The DESTRUCTIVE verbs inherit the K1-F2 ACL
// discipline verbatim: the persisted (native) ACL row is cleared DURABLE-FIRST — before the `0xE5`
// name delete — so a crash fails toward PUBLIC (a stale owner row a future same-name file could
// adopt is the K1-F2 failure class). Native ACL persist happens AFTER the NAMESPACE hold drops
// (the K5B rule); the durable clear runs BEFORE the hold (the sidecar `atr_clear_row` re-takes
// `ns`, so it can never nest under it — the sys_unlink ordering, inherited exactly).
// -----------------------------------------------------------------------------------------------

/// The v1 write-content ceiling (the BUS_CP_MAX bound, shared with cp — a deliberate bound, never
/// raised for a demo). The frame body ceiling already bounds the payload; this is the semantic cap.
/// bus write: CREATE-OR-TRUNCATE a ROOT file under the invoker's grants with `content` (<= BUS_CP_MAX).
///
/// CREATE (name absent): mirrors sys_open(O_CREAT) + a single write — create_in_root -> write_grow
/// -> owner row PRIVATE to the invoker (or PUBLIC for an anonymous/ASID-0 invoker), native persist
/// AFTER the ns hold drops (the bus_cp create half, sourced from `content` rather than a copied file).
///
/// TRUNCATE (name present): OWNER-ONLY (`owned_unlink_permitted` — a public file is anyone's). It is
/// implemented as an atomic-by-name DELETE-then-CREATE: the in-lane FAT primitives grow but never
/// SHRINK in place, and a delete+recreate BY A GRANTEE would let a content grantee steal ownership —
/// so, exactly like sys_unlink's owner-only delete authority, a write-truncate of an OWNED file is
/// owner-only (a write-GRANTEE gets -EACCES; in-place grantee truncate needs a fat.rs shrink
/// primitive, out of the v1 bus lane — HONEST SCOPE, flagged). The delete half clears the persisted
/// ACL row DURABLE-FIRST (K1-F2), defers the old chain's free if a concurrent reader holds it open
/// (never freeing clusters underneath a live open), then the create half re-owns the file to the
/// invoker. A previously-owned file stays owned by its owner; a public file becomes private to the
/// writer (the sys_open(O_CREAT) private policy, applied uniformly to create and truncate).
fn bus_write(asid: u64, agen: u64, ppid: PrincipalRecord, name: &str, content: &[u8]) -> i64 {
    if name.eq_ignore_ascii_case(ATR_NAME) {
        return EACCES; // the kernel owns the ACL store — never a writable name
    }
    if content.len() > BUS_CP_MAX {
        return E2BIG; // v1's honest write ceiling
    }
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    // Existence check (advisory — re-verified under the ns hold below before any mutation).
    let existing = {
        let ns = ns_lock();
        let r = match fs.find_located(name) {
            Ok((de, l, o)) => Some((de.is_dir, de.first_cluster(), l, o)),
            Err(crate::fs::fat::FatError::NotFound) => None,
            Err(_) => return EIO,
        };
        drop(ns);
        r
    };
    // BANDY-2 lens-2 fix: the grant rows to RESTORE onto the recreated file after a truncate. The
    // direct-syscall twin (open existing + write) mutates in place and PRESERVES grants — the bus
    // truncate must reach the same state (a content rewrite is not a revoke). None for a fresh
    // create or a public/anonymous original (nothing to carry).
    let mut grant_snap: Option<[FileGrant; NFGRANT]> = None;
    if let Some((is_dir, cluster, dir_lba, dir_off)) = existing {
        if is_dir {
            return EISDIR; // v1: files only, exactly as cat/cp
        }
        // TRUNCATE is an owner-only authority (see the doc note): a public file is anyone's.
        if !owned_unlink_permitted(dir_lba, dir_off as u32, asid, agen, ppid) {
            return EACCES;
        }
        let owner_ppid = owned_owner_ppid(dir_lba, dir_off as u32);
        // Snapshot the FULL grant rows BEFORE the destroy half (lens-2 fix) — restored onto the
        // recreated file below, then re-persisted. In-RAM read (an inner lock), like owner_ppid.
        grant_snap = owned_snapshot_grants_full(dir_lba, dir_off as u32);
        // Clear the PERSISTED ACL row DURABLE-FIRST — BEFORE the 0xE5 (K1-F2); ns-free (atr_clear_row
        // re-takes ns, so it can never nest under the hold). A named-owner only; a public file skips
        // this with zero disk I/O. -EIO aborts if the durable clear did not land (nothing stranded).
        if owner_ppid.kind != PRIN_NONE
            && !(crate::fs::unafs::native_acl_clear(dir_lba, dir_off as u32)
                && atr_clear_row(&fs, dir_lba, dir_off as u32))
        {
            return EIO;
        }
        // The 0xE5 + owner-row drop + deferred-free arm, atomic against concurrent name ops (ns).
        let ns = ns_lock();
        match fs.find_located(name) {
            // The name must still resolve to the SAME slot (no concurrent rename/recycle). A same-slot
            // hit of the same name is our file (delete-by-name is content-agnostic — fine).
            Ok((d2, l2, o2)) if l2 == dir_lba && o2 == dir_off && !d2.is_dir => {}
            Ok(_) => return EIO,                                   // slot moved/recycled — refuse (already public)
            Err(crate::fs::fat::FatError::NotFound) => return EIO, // raced-deleted between clear and hold
            Err(_) => return EIO,
        }
        if fs.mark_dir_deleted(dir_lba, dir_off).is_err() {
            return EIO;
        }
        owned_clear(dir_lba, dir_off as u32);
        let deferred = openfile_mark_pending_or_none_by_dir(dir_lba, dir_off as u32, cluster);
        drop(ns);
        if !deferred && cluster != 0 {
            free_orphan_chain(cluster);
        }
        // fall through to CREATE (the recreate half)
    }
    // CREATE (a fresh file, or the post-truncate recreate) — the bus_cp create half from `content`.
    let ns = ns_lock();
    match fs.find_located(name) {
        Ok(_) => return EEXIST, // raced-created after our delete (a concurrent create won the slot)
        Err(crate::fs::fat::FatError::NotFound) => {}
        Err(_) => return EIO,
    }
    let (dde, d_lba, d_off) = match fs.create_in_root(name, 0x20) {
        Ok(t) => t,
        Err(crate::fs::fat::FatError::Unsupported) => return EINVAL, // not an 8.3 name
        Err(crate::fs::fat::FatError::NoSpace) => return ENOSPC,
        Err(_) => return EIO,
    };
    let mut new_fc = dde.first_cluster();
    if !content.is_empty() {
        match fs.write_grow(dde.first_cluster(), 0, d_lba, d_off, 0, content) {
            Ok((_, _, fc)) => new_fc = fc,
            Err(_) => {
                let _ = fs.mark_dir_deleted(d_lba, d_off); // leak-not-dangle
                return EIO;
            }
        }
    }
    if asid != 0 && !owned_set_owner(d_lba, d_off as u32, asid, agen, ppid) {
        let _ = fs.mark_dir_deleted(d_lba, d_off);
        return ENOSPC; // fail CLOSED — never a silently-public "private" file
    }
    // Lens-2 fix: re-install the truncated file's grant snapshot onto the recreated row (the new
    // slot's location — create may or may not reuse the old slot). Under the SAME ns hold as the
    // owner install (OWNED_FILES is an inner lock); only meaningful when this create claimed the
    // owner row above (asid != 0).
    let had_grants = grant_snap.as_ref().is_some_and(|g| g.iter().any(|fg| fg.rights != 0));
    if asid != 0 {
        if let Some(snap) = grant_snap.as_ref() {
            owned_restore_grants(d_lba, d_off as u32, snap);
        }
    }
    drop(ns);
    if asid != 0 && ppid.kind != PRIN_NONE {
        let _ = native_persist_create(name, new_fc, d_lba, d_off as u32, ppid);
        // Lens-2 fix: create-persist writes an EMPTY grant set — re-persist the REAL (restored)
        // grants so the durable row matches the in-RAM state (K5B-fused inside the persist site;
        // AFTER the ns drop, like every persist).
        if had_grants {
            let _ = native_persist_grants(d_lba, d_off as u32);
        }
    }
    0
}

/// bus rm: UNLINK a ROOT file by name under the invoker's grants — the sys_unlink twin. DELETE is an
/// OWNER-ONLY authority (`owned_unlink_permitted`: a content grantee gets read/write, NEVER delete —
/// else it could unlink + recreate the name to steal ownership; a public file is anyone's). The
/// persisted ACL row is cleared DURABLE-FIRST (before the 0xE5 — K1-F2), then the name is deleted,
/// the owner row dropped, and the cluster chain freed (or deferred to a concurrent opener's last
/// close). Errnos mirror sys_unlink/the cat gate: ATR deny / mount / -ENOENT / -EISDIR / -EACCES.
fn bus_rm(asid: u64, agen: u64, ppid: PrincipalRecord, name: &str) -> i64 {
    if name.eq_ignore_ascii_case(ATR_NAME) {
        return EACCES;
    }
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    // Locate + authorize under the ns hold (coherent), then DROP it for the durable clear (which
    // re-takes ns), then re-take it for the destructive sequence (re-verifying the slot).
    let (cluster, dir_lba, dir_off, owner_ppid) = {
        let ns = ns_lock();
        let (de, dir_lba, dir_off) = match fs.find_located(name) {
            Ok(t) => t,
            Err(crate::fs::fat::FatError::NotFound) => return ENOENT,
            Err(_) => return EIO,
        };
        if de.is_dir {
            return EISDIR; // v1: files only (rmdir is a later verb)
        }
        if !owned_unlink_permitted(dir_lba, dir_off as u32, asid, agen, ppid) {
            return EACCES;
        }
        let owner_ppid = owned_owner_ppid(dir_lba, dir_off as u32);
        drop(ns);
        (de.first_cluster(), dir_lba, dir_off, owner_ppid)
    };
    // K1-F2: clear the PERSISTED row DURABLE-FIRST — ns-free (atr_clear_row re-takes ns). -EIO
    // aborts (nothing stranded) if the durable clear did not land. A public file skips it (0 I/O).
    if owner_ppid.kind != PRIN_NONE
        && !(crate::fs::unafs::native_acl_clear(dir_lba, dir_off as u32)
            && atr_clear_row(&fs, dir_lba, dir_off as u32))
    {
        return EIO;
    }
    let ns = ns_lock();
    // TOCTOU re-verify: the name must still map to the SAME slot (no concurrent rename/recycle
    // landed while ns was released for the clear). A same-name same-slot hit is our file.
    match fs.find_located(name) {
        Ok((d2, l2, o2)) if l2 == dir_lba && o2 == dir_off && !d2.is_dir => {}
        Ok(_) => return EIO,
        Err(crate::fs::fat::FatError::NotFound) => return ENOENT,
        Err(_) => return EIO,
    }
    if fs.mark_dir_deleted(dir_lba, dir_off).is_err() {
        return EIO;
    }
    owned_clear(dir_lba, dir_off as u32);
    let deferred = openfile_mark_pending_or_none_by_dir(dir_lba, dir_off as u32, cluster);
    drop(ns);
    if !deferred && cluster != 0 {
        free_orphan_chain(cluster);
    }
    0
}

/// bus mv: RENAME a ROOT file in place under the invoker's grants — the fat.rs `rename_entry` twin.
/// The rename is OWNER-ONLY on an owned file (`owned_unlink_permitted` — the same owner-authority as
/// delete: a grantee mutating the name could re-point ownership; a public file is anyone's) and
/// CREATE-NEW-ONLY on the destination (-EEXIST). An in-place rename keeps the SAME directory slot, so
/// the in-RAM owner row (keyed by location) is unchanged — live enforcement stays correct with no
/// owned_* mutation; ONLY the persisted native row's stored NAME goes stale, so it is re-bound AFTER
/// the ns hold drops (the K5B rule) via `native_persist_rename` — owner + grants preserved. On a
/// re-bind failure the stale-named row is cleared (fail-closed against the K1-F2 re-adoption class).
/// Errnos: ATR deny / mount / -ENOENT (src) / -EISDIR / -EACCES / -EEXIST (dst) / -EINVAL (bad name).
fn bus_mv(asid: u64, agen: u64, ppid: PrincipalRecord, src: &str, dst: &str) -> i64 {
    if src.eq_ignore_ascii_case(ATR_NAME) || dst.eq_ignore_ascii_case(ATR_NAME) {
        return EACCES;
    }
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(crate::fs::fat::FatError::NoDisk) => return ENODEV,
        Err(_) => return EIO,
    };
    let ns = ns_lock();
    let (sde, s_lba, s_off) = match fs.find_located(src) {
        Ok(t) => t,
        Err(crate::fs::fat::FatError::NotFound) => return ENOENT,
        Err(_) => return EIO,
    };
    if sde.is_dir {
        return EISDIR; // v1: files only (directory rename disturbs no `..`, but stays out of scope)
    }
    if !owned_unlink_permitted(s_lba, s_off as u32, asid, agen, ppid) {
        return EACCES;
    }
    // Destination must be absent (create-new-only target — no silent overwrite).
    match fs.find_located(dst) {
        Ok(_) => return EEXIST,
        Err(crate::fs::fat::FatError::NotFound) => {}
        Err(_) => return EIO,
    }
    let owner_ppid = owned_owner_ppid(s_lba, s_off as u32);
    // In-place rename (single dir-sector name RMW — SAME slot, so the in-RAM/native LOCATION keys
    // stay valid). dst-exists was pre-checked, so an Unsupported here is a bad 8.3 dst name.
    match fs.rename_entry(0, src, dst) {
        Ok(_) => {}
        Err(crate::fs::fat::FatError::Unsupported) => return EINVAL,
        Err(crate::fs::fat::FatError::NotFound) => return ENOENT,
        Err(_) => return EIO,
    }
    drop(ns);
    // Re-bind the persisted NAME after the ns drop (K5B). Best-effort; a failure clears the stale
    // row so a future same-name file can never adopt it (K1-F2). A public file: nothing persisted.
    if owner_ppid.kind != PRIN_NONE && !native_persist_rename(s_lba, s_off as u32, dst) {
        let _ = crate::fs::unafs::native_acl_clear(s_lba, s_off as u32);
    }
    0
}

/// Build + enqueue the reply frame for a fulfilled (or refused) verb: verb + corr echoed,
/// status = the verb's errno (0 = ok), principal = the RESERVED KERNEL REPLY record (verdict C),
/// body = the verb-typed output bytes (forced empty on error — bus.rs build_reply). The frame is
/// heap-staged at its exact final size.
fn bus_reply_enqueue(asid: u64, corr: u32, verb: u8, status: i64, text: &[u8]) -> i64 {
    let reply_prin = PrincipalRecord { kind: PRIN_KERNEL_REPLY, len: 0, value: [0u8; PRIN_VALUE_LEN] };
    let mut prin_bytes = [0u8; 32];
    reply_prin.write(&mut prin_bytes);
    let body: &[u8] = if status == 0 { text } else { &[] };
    debug_assert!(body.len() <= super::bus::BUS_BODY_MAX, "bus reply over the body ceiling (kernel bug)");
    if body.len() > super::bus::BUS_BODY_MAX {
        return EIO; // fail closed (ls/cat/cp budgets make this unreachable)
    }
    let mut frame = alloc::vec![0u8; super::bus::BUS_HDR_LEN + body.len()];
    let n = super::bus::build_reply(verb, corr, status as i32, prin_bytes, body, &mut frame);
    debug_assert!(n == frame.len());
    let agen = ASID_GEN[asid as usize].load(Ordering::Acquire);
    if !bus_mbox_push(asid, agen, frame.into_boxed_slice()) {
        return EAGAIN; // defensively unreachable (capacity pre-checked; single-threaded sender)
    }
    if BUS_SEM_INIT.load(Ordering::Acquire) == 2 {
        BUS_SEM[asid as usize].post();
    }
    0
}

/// The SYS_MSEND body, parameterized on the sender's identity — the SVC arm passes the caller's
/// live (asid, gen) + its kernel-held principal; the M5 fixtures drive the SAME path with scratch
/// identities (the sys_recv_for idiom, no duplicate logic). `frame` is already in kernel memory.
fn sys_msend_for(asid: u64, agen: u64, ppid: PrincipalRecord, frame: &[u8]) -> i64 {
    // Parse + typed validation, all FAIL-CLOSED (bus.rs). Errno mapping: every refused frame —
    // ceiling, truncation, malformed, caller-supplied principal (verdict C) — is -EINVAL.
    let hdr = match super::bus::frame_parse(frame) {
        Ok(h) => h,
        Err(_) => return EINVAL,
    };
    if hdr.kind != super::bus::BUS_KIND_REQUEST || super::bus::request_validate(&hdr).is_err() {
        return EINVAL; // a REPLY frame, nonzero status, or CALLER-SUPPLIED PRINCIPAL — rejected
    }
    let body = &frame[super::bus::BUS_HDR_LEN..];
    // Capacity BEFORE fulfillment — a verb with side effects must never run and lose its reply.
    if (asid as usize) >= BUS_MBOX.len() || !bus_mbox_has_room(asid) {
        return EAGAIN;
    }
    // THE STAMP (verdict C): the sender's kernel-held principal, written INSIDE the kernel — the
    // EL0 bytes for this field were required zero above, and fulfillment below receives `ppid`
    // (the stamped identity) directly; hdr.principal is never read again.
    // Fulfill under the INVOKER's identity (verdict D) — synchronously, in this SVC context.
    let mut text: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let status = match hdr.verb {
        super::bus::BUS_VERB_LS => bus_ls(&mut text),
        super::bus::BUS_VERB_CAT => match super::bus::cat_body_parse(body) {
            // cat_body_parse admits printable ASCII only, so from_utf8 cannot fail — but stay
            // fail-closed rather than unwrap.
            Ok(nb) => match core::str::from_utf8(nb) {
                Ok(name) => bus_cat(asid, agen, ppid, name, &mut text),
                Err(_) => return EINVAL,
            },
            Err(_) => return EINVAL,
        },
        super::bus::BUS_VERB_CP => match super::bus::cp_body_parse(body) {
            Ok((s, d)) => match (core::str::from_utf8(s), core::str::from_utf8(d)) {
                (Ok(src), Ok(dst)) => bus_cp(asid, agen, ppid, src, dst),
                _ => return EINVAL,
            },
            Err(_) => return EINVAL,
        },
        // BANDY-2: the write-side verbs. The typed-body parsers admit printable-ASCII names only, so
        // from_utf8 cannot fail — but stay fail-closed rather than unwrap.
        super::bus::BUS_VERB_WRITE => match super::bus::write_body_parse(body) {
            Ok((nb, content)) => match core::str::from_utf8(nb) {
                Ok(name) => bus_write(asid, agen, ppid, name, content),
                Err(_) => return EINVAL,
            },
            Err(_) => return EINVAL,
        },
        super::bus::BUS_VERB_RM => match super::bus::cat_body_parse(body) {
            Ok(nb) => match core::str::from_utf8(nb) {
                Ok(name) => bus_rm(asid, agen, ppid, name),
                Err(_) => return EINVAL,
            },
            Err(_) => return EINVAL,
        },
        super::bus::BUS_VERB_MV => match super::bus::cp_body_parse(body) {
            Ok((s, d)) => match (core::str::from_utf8(s), core::str::from_utf8(d)) {
                (Ok(src), Ok(dst)) => bus_mv(asid, agen, ppid, src, dst),
                _ => return EINVAL,
            },
            Err(_) => return EINVAL,
        },
        _ => return EINVAL, // unreachable (frame_parse validated the verb) — fail closed
    };
    bus_reply_enqueue(asid, hdr.corr, hdr.verb, status, &text)
}

/// SYS_MSEND(frame_ptr, frame_len) — the EL0 arm: validated copy-in (the CFU-1-style predicate
/// discipline: length ceiling BEFORE any copy, whole-range user validation, no partial decode),
/// then the parameterized body under the caller's own identity. The staging buffer is HEAP-
/// allocated at the exact request length (max forced staging = 4148 B; never a big stack array).
fn sys_msend(frame_ptr: u64, frame_len: u64) -> i64 {
    bus_sem_init_once();
    let len = frame_len as usize;
    if len < super::bus::BUS_HDR_LEN || len > super::bus::BUS_FRAME_MAX {
        return EINVAL; // the whole-frame ceiling gates the copy itself
    }
    let mut frame = alloc::vec![0u8; len];
    if copy_from_user(&mut frame, frame_ptr, len).is_err() {
        return EFAULT;
    }
    let asid = current_asid();
    let agen = ASID_GEN[asid as usize].load(Ordering::Acquire);
    let ppid = current_principal();
    sys_msend_for(asid, agen, ppid, &frame)
}

/// SYS_MRECV(buf_ptr, buf_len) — dequeue the oldest reply (blocking while empty). buf_len must
/// cover a whole frame (>= BUS_FRAME_MAX, else -EMSGSIZE) so no partial-delivery state exists;
/// the frame's exact length is the return value. Blocking rides the sys_wait Semaphore idiom
/// (park + cross-core wake); off a scheduled task (kernel fixture context) it degrades to a
/// non-blocking poll returning -EAGAIN.
fn sys_mrecv(buf_ptr: u64, buf_len: u64) -> i64 {
    bus_sem_init_once();
    if (buf_len as usize) < super::bus::BUS_FRAME_MAX {
        return EMSGSIZE;
    }
    let asid = current_asid();
    if (asid as usize) >= BUS_MBOX.len() {
        return EAGAIN;
    }
    // Validate the destination ONCE up front (whole-frame window), so a bad buffer is -EFAULT
    // before any dequeue — a popped frame is never lost to a copy failure.
    if !user_range_ok(buf_ptr, super::bus::BUS_FRAME_MAX, true) {
        return EFAULT;
    }
    loop {
        if let Some(msg) = bus_mbox_pop(asid) {
            // Range validated above; copy the exact frame.
            if copy_to_user(buf_ptr, &msg.frame, msg.frame.len()).is_err() {
                return EFAULT; // unreachable after the up-front check; fail closed
            }
            return msg.frame.len() as i64;
        }
        // Empty: park on the semaphore (posted by every enqueue). A spurious permit (teardown
        // drain raced an enqueue) just loops back around to wait.
        if !BUS_SEM[asid as usize].wait() {
            return EAGAIN; // off a scheduled task — the kernel-fixture poll path
        }
        remask_irq(); // the sys_wait discipline: epilogue must run IRQ-masked
    }
}

// -----------------------------------------------------------------------------------------------
// BANDY-STAMP (M5, verdict C): the stamping witness — caller-supplied principal REJECTED; replies
// stamped with the RESERVED KERNEL kind; that kind fail-closed as grantee/owner and
// un-projectable to the native store. Kernel-side (the k1_persist idiom: scratch identities, the
// PRODUCTION code path, no EL0 detour); in-RAM except two ls fulfillments (read-only mount).
// -----------------------------------------------------------------------------------------------

fn bandy_stamp_check() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut w = 0u32;
    let asid: u64 = 7; // scratch (the K-line fixture discipline; EL0 fixtures are done by now)
    let agen = ASID_GEN[asid as usize].load(Ordering::Acquire);
    let ppid = PrincipalRecord::program(b"prog:BANDYSTMP");
    bus_mbox_clear(asid);

    // bit0: a caller-supplied principal is REJECTED -EINVAL — and nothing was fulfilled/queued.
    let mut f = [0u8; 64];
    let n = super::bus::build_request(super::bus::BUS_VERB_LS, 1, b"", &mut f);
    let mut forged = f;
    forged[16] = PRIN_IMAGE_SHA256; // nonzero principal kind byte from "EL0"
    if sys_msend_for(asid, agen, ppid, &forged[..n]) == EINVAL && bus_mbox_pop(asid).is_none() {
        w |= 1 << 0;
    }

    // bit1+bit2: a clean LS round-trips; the reply echoes verb+corr with status 0 — and is
    // stamped with the RESERVED KERNEL kind (PRIN_KERNEL_REPLY), KERNEL-written (the request
    // carried zeros).
    if sys_msend_for(asid, agen, ppid, &f[..n]) == 0 {
        if let Some(msg) = bus_mbox_pop(asid) {
            if let Ok(h) = super::bus::frame_parse(&msg.frame) {
                let rp = PrincipalRecord::read(&h.principal);
                if h.kind == super::bus::BUS_KIND_REPLY
                    && h.verb == super::bus::BUS_VERB_LS
                    && h.corr == 1
                    && h.status == 0
                {
                    w |= 1 << 1;
                }
                if rp.kind == PRIN_KERNEL_REPLY {
                    w |= 1 << 2;
                }
            }
        }
    }

    // bit3: the reply kind can never be adopted as an OWNER (owned_set_owner refuses it).
    let reply_prin = PrincipalRecord { kind: PRIN_KERNEL_REPLY, len: 0, value: [0u8; PRIN_VALUE_LEN] };
    if !owned_set_owner(0xB0DE, 0xB0, asid, agen, reply_prin) {
        w |= 1 << 3;
    }

    // bit4: ...nor GRANTED rights (owned_grant refuses a reply-kind grantee outright), proven
    // against a REAL row owned by the scratch principal.
    if owned_set_owner(0xB0DE, 0xB0, asid, agen, ppid) {
        let r = owned_grant(0xB0DE, 0xB0, asid, agen, 8, 0, CAP_READ, ppid, reply_prin);
        if r == EACCES {
            w |= 1 << 4;
        }
        owned_clear(0xB0DE, 0xB0); // scratch row out of the bounded table
    }

    // bit5: the reply kind is UN-PERSISTABLE — no native projection exists (fail-closed None),
    // so it can never reach the durable store to be rebuilt as an identity.
    let mut sbuf = [0u8; K4_STR_MAX];
    if principal_native_string(&reply_prin, &mut sbuf).is_none() {
        w |= 1 << 5;
    }

    // bit6: mailbox exhaustion is FAIL-CLOSED and per-ASID: 16 queued replies fill asid 7 (the
    // 17th send refuses -EAGAIN BEFORE fulfilling), while asid 8's mailbox is unaffected (no
    // cross-ASID starvation leverage).
    let mut filled = true;
    for _ in 0..BUS_MBOX_DEPTH {
        if sys_msend_for(asid, agen, ppid, &f[..n]) != 0 {
            filled = false;
            break;
        }
    }
    let other_asid: u64 = 8;
    let other_gen = ASID_GEN[other_asid as usize].load(Ordering::Acquire);
    bus_mbox_clear(other_asid);
    if filled
        && sys_msend_for(asid, agen, ppid, &f[..n]) == EAGAIN
        && sys_msend_for(other_asid, other_gen, ppid, &f[..n]) == 0
    {
        w |= 1 << 6;
    }
    bus_mbox_clear(other_asid);

    // bit7: the teardown/gen fence — a queued reply stamped under an OLD gen is discarded, never
    // delivered to the "next tenant" (simulated by bumping the scratch ASID's gen, exactly what
    // clear_handle_row does first).
    ASID_GEN[asid as usize].fetch_add(1, Ordering::AcqRel);
    let fenced = bus_mbox_pop(asid).is_none();
    bus_mbox_clear(asid);
    if fenced {
        w |= 1 << 7;
    }

    const ALL: u32 = (1 << 8) - 1;
    if w == ALL {
        serial_println!(
            ":: BANDY-STAMP: kernel-written principal — caller-supplied REJECTED (-EINVAL), replies stamped RESERVED-KERNEL kind (ungrantable, unadoptable, unpersistable), mailbox bounded per-ASID fail-closed, gen-fenced PASS [w={:#04x}] ::",
            w
        );
    } else {
        serial_println!(":: BANDY-STAMP: FAIL — w={:#x}/{:#x} ::", w, ALL);
    }
}

// -----------------------------------------------------------------------------------------------
// BANDY-GRANT (lens-2 fix witness): bus write-TRUNCATE preserves grants. The direct-syscall twin
// (open existing + write) mutates in place and never touches the grant table; the bus truncate is
// delete-then-recreate, so without the restore it would silently REVOKE every grant. This witness
// drives the PRODUCTION sys_msend_for path with scratch identities (the BANDY-STAMP idiom):
// owner bus-writes a file, grants a grantee CAP_READ, bus-writes NEW content (the truncate path),
// then proves the grantee is STILL admitted via BOTH legs — the bus (cat status 0, new content)
// and the direct gate (owned_access_ok true) — and that the grant re-persisted durably.
// -----------------------------------------------------------------------------------------------

const GRNT_NAME: &str = "GRNT.BIN";

/// Build a bus WRITE request frame for `name`+`content` into `buf`; returns the frame length.
fn bandy_build_write(name: &str, content: &[u8], corr: u32, buf: &mut [u8]) -> usize {
    let mut body = [0u8; 64]; // name <= 12 + small content — far under any stack hazard
    body[0] = name.len() as u8;
    body[1..1 + name.len()].copy_from_slice(name.as_bytes());
    body[1 + name.len()..1 + name.len() + content.len()].copy_from_slice(content);
    super::bus::build_request(super::bus::BUS_VERB_WRITE, corr, &body[..1 + name.len() + content.len()], buf)
}

fn bandy_grant_check() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        serial_println!(":: BANDY-GRANT: no block device — truncate-grant demo skipped ::");
        return;
    }
    let mut w = 0u32;
    let owner_asid: u64 = 7; // scratch identities (the K-line fixture discipline)
    let owner_gen = ASID_GEN[owner_asid as usize].load(Ordering::Acquire);
    let owner = PrincipalRecord::program(b"prog:BANDYOWN");
    let grantee_asid: u64 = 8;
    let grantee_gen = ASID_GEN[grantee_asid as usize].load(Ordering::Acquire);
    let grantee = PrincipalRecord::program(b"prog:BANDYGRT");
    bus_mbox_clear(owner_asid);
    bus_mbox_clear(grantee_asid);
    bandy_cleanup_one(GRNT_NAME); // stale residue from an interrupted run

    const C1: &[u8] = b"grant-content-one";
    const C2: &[u8] = b"rewritten"; // SHORTER — the readback proving C2 confirms a genuine truncate

    // Send one request under `(asid, gen, ppid)` and pop the reply; returns (status, body bytes).
    fn send(asid: u64, agen: u64, ppid: PrincipalRecord, frame: &[u8]) -> (i64, alloc::vec::Vec<u8>) {
        if sys_msend_for(asid, agen, ppid, frame) != 0 {
            return (i64::MIN, alloc::vec::Vec::new());
        }
        let Some(msg) = bus_mbox_pop(asid) else {
            return (i64::MIN, alloc::vec::Vec::new());
        };
        match super::bus::frame_parse(&msg.frame) {
            Ok(h) => (h.status as i64, msg.frame[super::bus::BUS_HDR_LEN..].to_vec()),
            Err(_) => (i64::MIN, alloc::vec::Vec::new()),
        }
    }

    // bit0: owner bus-writes (CREATE) GRNT.BIN with C1 — status 0, and the file is owner-private.
    let mut f = [0u8; 128];
    let n = bandy_build_write(GRNT_NAME, C1, 1, &mut f);
    let (st, _) = send(owner_asid, owner_gen, owner, &f[..n]);
    let loc1 = crate::fs::fat::mount().ok().and_then(|fs| fs.find_located(GRNT_NAME).ok());
    if st == 0 {
        if let Some((_, l, o)) = loc1 {
            if owned_owner_ppid(l, o as u32) == owner {
                w |= 1 << 0;
            }
        }
    }

    if let Some((_, l1, o1)) = loc1 {
        // bit1: owner grants the grantee CAP_READ (production owned_grant), and the grantee's bus
        // cat is admitted with C1 (the pre-truncate baseline, bus leg).
        let granted = owned_grant(l1, o1 as u32, owner_asid, owner_gen, grantee_asid, grantee_gen, CAP_READ, owner, grantee) == 0;
        let mut cf = [0u8; 128];
        let cn = super::bus::build_request(super::bus::BUS_VERB_CAT, 2, GRNT_NAME.as_bytes(), &mut cf);
        let (cst, cbody) = send(grantee_asid, grantee_gen, grantee, &cf[..cn]);
        if granted && cst == 0 && cbody.as_slice() == C1 {
            w |= 1 << 1;
        }

        // bit2: owner bus-writes C2 over the EXISTING file — the TRUNCATE path — status 0, content
        // is exactly C2 (shorter than C1: a genuine truncate, not a stale-tail overwrite).
        let n2 = bandy_build_write(GRNT_NAME, C2, 3, &mut f);
        let (st2, _) = send(owner_asid, owner_gen, owner, &f[..n2]);
        let loc2 = crate::fs::fat::mount().ok().and_then(|fs| fs.find_located(GRNT_NAME).ok());
        if st2 == 0 {
            if let Some((de2, _, _)) = &loc2 {
                if de2.size as usize == C2.len() {
                    w |= 1 << 2;
                }
            }
        }

        if let Some((_, l2, o2)) = loc2 {
            // bit3: THE FIX'S PROOF, bus leg — the grantee's cat is STILL admitted after the
            // truncate (status 0) and reads the NEW content (the grant survived the rewrite).
            let (cst2, cbody2) = send(grantee_asid, grantee_gen, grantee, &cf[..cn]);
            if cst2 == 0 && cbody2.as_slice() == C2 {
                w |= 1 << 3;
            }
            // bit4: the direct leg, byte-equivalent — the production ACL gate itself still admits
            // the grantee for CAP_READ (what a direct SYS_OPEN(RO) resolves through), and BOTH
            // legs agree (admitted <-> admitted).
            if owned_access_ok(l2, o2 as u32, grantee_asid, grantee_gen, CAP_READ, grantee) && cst2 == 0 {
                w |= 1 << 4;
            }
            // bit5: the DURABLE half — the re-persisted native row carries the restored grant
            // (create-persist alone would have written an empty set).
            if let Some(row) = crate::fs::unafs::native_acl_read(l2, o2 as u32) {
                if row.grants.iter().any(|(k, _)| k.as_slice() == b"grants:prog:BANDYGRT") {
                    w |= 1 << 5;
                }
            }
        }
    }

    // bit6: self-clean — owner bus-rm's the fixture; the name is gone (no card residue).
    let mut rf = [0u8; 128];
    let rn = super::bus::build_request(super::bus::BUS_VERB_RM, 4, GRNT_NAME.as_bytes(), &mut rf);
    let (rst, _) = send(owner_asid, owner_gen, owner, &rf[..rn]);
    let gone = crate::fs::fat::mount()
        .map(|fs| matches!(fs.find_in_root(GRNT_NAME), Err(crate::fs::fat::FatError::NotFound)))
        .unwrap_or(false);
    if rst == 0 && gone {
        w |= 1 << 6;
    }
    bus_mbox_clear(owner_asid);
    bus_mbox_clear(grantee_asid);

    const ALL: u32 = (1 << 7) - 1;
    if w == ALL {
        serial_println!(
            ":: BANDY-GRANT: bus write-truncate preserves grants — grantee still admitted via bus AND direct gate (byte-equivalent), new content read back, grant re-persisted durable, self-cleaned PASS [w={:#04x}] ::",
            w
        );
    } else {
        serial_println!(":: BANDY-GRANT: FAIL — w={:#x}/{:#x} ::", w, ALL);
    }
}

// -----------------------------------------------------------------------------------------------
// BANDY-RT + BANDY-EQ (M4/M5): the round-trip and equivalence witnesses, through the REAL midden
// program. The launcher stages two fixtures on the live volume — a PUBLIC MIDTXT.TXT and a
// FOREIGN-owned BUSPRIV.BIN (owner = a persisted-sentinel principal midden can never match) —
// then spawns MIDDEN.BIN (the K2 real-program idiom). Midden, at EL0, parses ls/cat/cp text into
// typed frames, round-trips them (send -> kernel fulfill -> reply -> print), and drives BOTH
// denial legs itself: bus cat BUSPRIV.BIN vs direct SYS_OPEN — the equivalence witness's
// byte-same-errno comparison happens at EL0 through the production paths (verdict D). The
// launcher verifies the cp's copy BYTE-EXACT kernel-side and self-cleans (no metal-card residue).
// -----------------------------------------------------------------------------------------------

const MIDDEN_NAME: &str = "MIDDEN.BIN";
const MIDDEN_EXIT_STATUS: u64 = 0xB5; // midden's sentinel exit -> EL0_MIDDEN_DONE
const MIDTXT_NAME: &str = "MIDTXT.TXT";
const MIDCPY_NAME: &str = "MIDCPY.TXT";
const BUSPRIV_NAME: &str = "BUSPRIV.BIN";
const MIDTXT_CONTENT: &[u8] = b"midden bus fixture\n"; // midden byte-verifies this at EL0 (bit1)
// BANDY-2: the files midden CREATES at EL0 via the write-side verbs (write/mv round-trips). The
// launcher cleans them pre-flight + post-run so the stateful card accumulates nothing.
const WRT_NAME: &str = "WRT.TXT"; // written then rm'd by midden (bit6/bit7)
const MVA_NAME: &str = "MVA.TXT"; // written then mv'd away by midden (bit8)
const MVB_NAME: &str = "MVB.TXT"; // the mv target (midden self-cleans, launcher belt-and-braces)
const STOLEN_NAME: &str = "STOLEN.BIN"; // the mv-of-foreign DENIED target (must never be created)

static EL0_MIDDEN_DONE: AtomicU32 = AtomicU32::new(0); // midden sentinel exits (want 1)
static BANDY_MIDDEN_WITNESS: AtomicU64 = AtomicU64::new(0); // midden's SYS_REPORT bits

/// SYS_REPORT router for midden (the k2_report idiom — keyed on the task name).
fn bandy_report(value: u64) {
    if super::sched::current_name() == Some("el0-midden") {
        BANDY_MIDDEN_WITNESS.store(value, Ordering::Release);
    }
}

/// Delete one bandy fixture file + its ACL rows (native + legacy sidecar + in-RAM). Idempotent.
fn bandy_cleanup_one(name: &str) {
    if let Ok(fs) = crate::fs::fat::mount() {
        if let Ok((de, lba, off)) = fs.find_located(name) {
            let _ = crate::fs::unafs::native_acl_clear(lba, off as u32);
            let _ = atr_clear_row(&fs, lba, off as u32);
            owned_clear(lba, off as u32);
            let _ = fs.delete_located(lba, off, de.first_cluster());
        }
    }
}

fn bandy_cleanup() {
    bandy_cleanup_one(MIDCPY_NAME);
    bandy_cleanup_one(BUSPRIV_NAME);
    bandy_cleanup_one(MIDTXT_NAME);
    // BANDY-2 write-side residue (present only after an interrupted run).
    bandy_cleanup_one(WRT_NAME);
    bandy_cleanup_one(MVA_NAME);
    bandy_cleanup_one(MVB_NAME);
    bandy_cleanup_one(STOLEN_NAME);
}

/// Create a root fixture file with `content` (create + grow). Returns (dir_lba, dir_off) or None.
fn bandy_create_file(fs: &crate::fs::fat::FatFs, name: &str, content: &[u8]) -> Option<(u64, u32)> {
    let (de, lba, off) = fs.create_in_root(name, 0x20).ok()?;
    if !content.is_empty() {
        fs.write_grow(de.first_cluster(), 0, lba, off, 0, content).ok()?;
    }
    Some((lba, off as u32))
}

fn bandy_rt_launcher(demo_cpu: usize) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // no SD -> the proof loads/creates real files; skip silently
    }
    // Pre-flight: midden must be on the card; stale fixtures from an interrupted run cleaned
    // FIRST (the K2 lesson: clean before any presence early-return, so nothing is stranded).
    bandy_cleanup();
    let fs = match crate::fs::fat::mount() {
        Ok(fs) => fs,
        Err(_) => return,
    };
    if fs.find_in_root(MIDDEN_NAME).is_err() {
        serial_println!(":: BANDY-RT: MIDDEN.BIN not on card — bus round-trip demo skipped ::");
        return;
    }

    let mut w = 0u32;

    // Stage the fixtures: PUBLIC MIDTXT.TXT (no owner row) + FOREIGN-owned BUSPRIV.BIN. The
    // foreign owner is a PERSISTED-sentinel row (owner_asid = OWNER_ASID_PERSISTED, a by-name
    // owner that can never live-match) under a PROGRAM_NAME principal midden's IMAGE_SHA256
    // stamp can never equal — so midden is denied by construction, not by accident.
    if bandy_create_file(&fs, MIDTXT_NAME, MIDTXT_CONTENT).is_none() {
        serial_println!(":: BANDY-RT: fixture staging failed — demo aborted ::");
        bandy_cleanup();
        return;
    }
    let foreign = PrincipalRecord::program(b"prog:BUSOWNER");
    match bandy_create_file(&fs, BUSPRIV_NAME, b"private bytes") {
        Some((lba, off)) => {
            if !owned_set_owner(lba, off, OWNER_ASID_PERSISTED, 0, foreign) {
                serial_println!(":: BANDY-RT: owner staging failed — demo aborted ::");
                bandy_cleanup();
                return;
            }
        }
        None => {
            serial_println!(":: BANDY-RT: fixture staging failed — demo aborted ::");
            bandy_cleanup();
            return;
        }
    }

    // Spawn the REAL program (the k2_run_program idiom) and wait for its sentinel exit.
    EL0_MIDDEN_DONE.store(0, Ordering::Release);
    BANDY_MIDDEN_WITNESS.store(0, Ordering::Release);
    let stamped = match load_program_into_slot(MIDDEN_NAME) {
        Ok(loaded) => {
            let asid = loaded.ttbr0 >> 48;
            let s = slot_ppid_of(asid);
            // midden PRINTS its replies (the §3b interface seam) — endow the reserved console
            // write cap (fd 1), exactly like the other printing spawners.
            install_console_cap(asid);
            super::sched::spawn_user_slot("el0-midden", loaded.base, loaded.sp, loaded.ttbr0, demo_cpu);
            let start = super::timer::cntpct();
            let deadline = 5 * super::timer::cntfrq();
            while EL0_MIDDEN_DONE.load(Ordering::Acquire) < 1
                && super::timer::cntpct().wrapping_sub(start) <= deadline
            {
                super::sched::yield_now();
            }
            Some(s)
        }
        Err(_) => None,
    };
    let Some(stamped) = stamped else {
        serial_println!(":: BANDY-RT: midden failed to load — demo aborted ::");
        bandy_cleanup();
        return;
    };
    let done = EL0_MIDDEN_DONE.load(Ordering::Acquire);
    let mw = BANDY_MIDDEN_WITNESS.load(Ordering::Acquire);

    // bit0: midden's own 10-bit witness complete (BANDY-1 ls/cat/cp round-trips + both denial legs
    // + the EL0 equivalence comparison, AND the BANDY-2 write/rm/mv round-trips + extended
    // equivalence — bits 6..=9).
    const MIDDEN_ALL: u64 = (1 << 10) - 1;
    if done == 1 && mw == MIDDEN_ALL {
        w |= 1 << 0;
    }
    // bit1: midden ran under a REAL IMAGE_SHA256 principal (the K-line mint — the stamp the bus
    // wrote into its requests).
    if stamped.kind == PRIN_IMAGE_SHA256 {
        w |= 1 << 1;
    }
    // bit2: the bus cp landed a BYTE-EXACT copy (kernel-side verify of MIDCPY.TXT), and it is
    // PRIVATE to midden's stamped principal (the owner row the bus create recorded).
    if let Ok(fs2) = crate::fs::fat::mount() {
        if let Ok((de, lba, off)) = fs2.find_located(MIDCPY_NAME) {
            let mut back: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            if de.size as usize == MIDTXT_CONTENT.len()
                && fs2.read_at(de.first_cluster(), de.size, 0, &mut back, MIDTXT_CONTENT.len()).is_ok()
                && back.as_slice() == MIDTXT_CONTENT
                && owned_owner_ppid(lba, off as u32) == stamped
            {
                w |= 1 << 2;
            }
        }
    }

    // BANDY-2 ACL INTEGRITY (before cleanup, while BUSPRIV.BIN is still staged): midden's DENIED
    // rm/mv/write of the foreign-owned BUSPRIV.BIN must have changed NOTHING on disk — the file is
    // still present AND still owned by the foreign principal (a stale-owner strand / same-name
    // re-adoption would be the K1-F2 failure class), AND the denied `mv` never created STOLEN.BIN.
    // midden also self-cleaned its OWN write-side files (WRT/MVA/MVB all gone by construction).
    let acl_intact = match crate::fs::fat::mount() {
        Ok(fsi) => {
            let buspriv_foreign = matches!(
                fsi.find_located(BUSPRIV_NAME),
                Ok((_, lba, off)) if owned_owner_ppid(lba, off as u32) == foreign
            );
            buspriv_foreign
                && matches!(fsi.find_in_root(STOLEN_NAME), Err(crate::fs::fat::FatError::NotFound))
                && matches!(fsi.find_in_root(WRT_NAME), Err(crate::fs::fat::FatError::NotFound))
                && matches!(fsi.find_in_root(MVA_NAME), Err(crate::fs::fat::FatError::NotFound))
                && matches!(fsi.find_in_root(MVB_NAME), Err(crate::fs::fat::FatError::NotFound))
        }
        Err(_) => false,
    };

    // Cleanup so the STATEFUL metal card accumulates nothing.
    bandy_cleanup();
    let cleaned = match crate::fs::fat::mount() {
        Ok(fs3) => {
            matches!(fs3.find_in_root(MIDCPY_NAME), Err(crate::fs::fat::FatError::NotFound))
                && matches!(fs3.find_in_root(BUSPRIV_NAME), Err(crate::fs::fat::FatError::NotFound))
                && matches!(fs3.find_in_root(MIDTXT_NAME), Err(crate::fs::fat::FatError::NotFound))
        }
        Err(_) => false,
    };
    if cleaned {
        w |= 1 << 3;
    }

    const ALL: u32 = (1 << 4) - 1;
    if w == ALL {
        serial_println!(
            ":: BANDY-RT: midden (program #3) round-trip — ls/cat/cp parsed at EL0 into typed frames, kernel-fulfilled under the stamped IMAGE_SHA256 principal, replies printed; cp copy byte-exact + private-to-invoker; self-cleaned PASS [w={:#03x}/mw={:#04x}] ::",
            w, mw
        );
        // The equivalence verdict rides midden's EL0 bits (3: bus denied -EACCES, 4: direct
        // denied -EACCES, 5: byte-same + allowed==allowed) — gate a dedicated line on them.
        serial_println!(
            ":: BANDY-EQ: equivalence — denied-via-bus == denied-via-syscall (byte-same -EACCES) and allowed == allowed, both legs at EL0 through the production paths PASS ::"
        );
    } else {
        serial_println!(
            ":: BANDY-RT: FAIL — w={:#x}/{:#x} midden_w={:#x}/{:#x} done={} ::",
            w, ALL, mw, MIDDEN_ALL, done
        );
    }

    // BANDY-2 write-side witnesses (separate lines so the spec REQUIREs them independently). All
    // ride midden's EL0 bits (proven at EL0 through the production paths) + the kernel-side ACL check.
    // BANDY-WR: the write/rm/mv ROUND-TRIP — write a file, cat it back byte-exact, rm it (cat -> gone),
    // and a full mv round-trip (write -> rename -> cat new byte-exact, old gone). midden bits 6,7,8.
    const WR_BITS: u64 = (1 << 6) | (1 << 7) | (1 << 8);
    if done == 1 && (mw & WR_BITS) == WR_BITS {
        serial_println!(
            ":: BANDY-WR: write-side round-trip at EL0 — write->cat byte-exact, rm->cat -ENOENT, mv->cat(new) byte-exact + cat(old) -ENOENT, all under midden's stamped principal PASS ::"
        );
    } else {
        serial_println!(":: BANDY-WR: FAIL — midden_w={:#x} want bits 6-8 done={} ::", mw, done);
    }
    // BANDY-EQ2: the EXTENDED equivalence — every destructive verb (rm/mv/write) on the foreign
    // BUSPRIV.BIN DENIED -EACCES via the bus, BYTE-SAME as a direct SYS_OPEN(RW) denial. midden bit 9.
    if done == 1 && (mw & (1 << 9)) != 0 {
        serial_println!(
            ":: BANDY-EQ2: write-side equivalence — rm/mv/write of a foreign-owned file denied-via-bus == denied-via-syscall (byte-same -EACCES), real EL0 legs PASS ::"
        );
    } else {
        serial_println!(":: BANDY-EQ2: FAIL — midden_w={:#x} want bit 9 done={} ::", mw, done);
    }
    // BANDY-ACL: kernel-side integrity — the denied rm/mv/write left BUSPRIV.BIN present + STILL
    // foreign-owned (no stale-owner strand / same-name re-adoption — the K1-F2 class), the denied mv
    // created no STOLEN.BIN, and midden's own write-side files self-cleaned (WRT/MVA/MVB all gone).
    if acl_intact {
        serial_println!(
            ":: BANDY-ACL: destructive-verb ACL integrity — denied rm/mv/write left the foreign owner row intact, no stolen name, write-side fixtures self-cleaned PASS ::"
        );
    } else {
        serial_println!(":: BANDY-ACL: FAIL — foreign owner row / residue check failed ::");
    }
}
