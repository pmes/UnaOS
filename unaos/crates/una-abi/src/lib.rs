// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// =================================================================================================
// una-abi — THE UnaOS syscall ABI, declared exactly once.
// =================================================================================================
//
// WHY THIS CRATE EXISTS. The syscall number/layout table used to be declared EIGHT times: once in
// `arch/x86_64/syscall.rs`, once in `arch/aarch64/syscall.rs`, and again — partially, each crate
// re-typing the subset it happened to use — in `user-vug`, `user-stat`, `user-pulse`,
// `user-blob/midden.rs`, and as bare asm immediates in `user-blob`, `user-elf`, `user-blob-x86`.
// `arroyo`'s own USER_CHECK_MATRIX note names those crates "exactly where a syscall-number or stub
// drift between arches can hide". It had already happened (ledger below). Nothing in the build
// could catch it, because nothing compared the copies.
//
// Now there is one copy. Both kernels and every ring-3 program import it, so a number or a flag
// cannot mean two things at once: a divergence becomes a type error or a deliberate, documented
// per-arch `cfg` in THIS file, where a reviewer can see both halves at the same time.
//
// WHAT BELONGS HERE. Only what crosses the privilege boundary: syscall numbers, sub-op and flag
// encodings ring 3 passes in, the packed layouts ring 3 reads back, and shared magic values (wire
// tags, sentinel exit statuses, key codes). Kernel-internal bookkeeping (object-table `KIND_*`,
// handle rights *storage*, per-fixture counters) stays in the kernel — it is not ABI.
//
// WHAT IS NOT DECIDED HERE. Which verbs an arch actually IMPLEMENTS. A number means the same verb
// everywhere; whether a given kernel answers it or falls to `-ENOSYS` is that kernel's business and
// is recorded in the per-number notes. Reserving a number on both arches is what keeps a later
// implementation from colliding with something else.
//
// NO DEPENDENCIES, and nothing but `const`/`const fn`: this crate is linked into flat EL0 blobs
// whose 4 KiB page budget `arroyo kernel8` asserts per blob, so it must contribute zero bytes.
//
// -------------------------------------------------------------------------------------------------
// DIVERGENCE LEDGER — what the eight copies had already drifted to, and how the freeze resolved it.
// The rule applied throughout: the KERNEL's current behaviour is the truth, because the ring-3
// binaries in this tree are built against it and run on metal today.
//
//  D1. `SYS_GETINFO`'s `ticks` FIELD HAD TWO DIFFERENT UNITS, and the two arch-neutral programs that
//      read it assumed OPPOSITE ones. The x86 kernel fills it from `arch::ticks()`, calibrated to
//      `apic::TICK_HZ` = 1000 Hz — one tick is one millisecond. The aarch64 kernel fills it from
//      `timer::ticks()`, the 250 Hz scheduler tick. Both are shipped behaviour and neither moves.
//      But `user-vug` hard-coded `const TICK_HZ: u32 = 250` and divided its frame count by it, so
//      every fps figure VUG-X86.ELF has ever drawn on the x86 panel was 4x LOW (and its
//      "once per second" refresh fired four times a second); while `user-pulse` named the same field
//      `ms` and took `ms % BREATH_MS` against a 3000-MILLISECOND period, so PULSE.ELF's liveness
//      breath on the Pi sweeps in 12 s, not the 3 s its own comment claims. Same field, same build
//      of the same source, opposite errors on opposite arches — the exact silent-wrong-value class.
//      RESOLVED: `GETINFO_TICK_HZ` below is the one declaration of the rate, `cfg`-selected per
//      target arch to match each kernel exactly; `getinfo_ticks_to_ms` converts. Callers ask the ABI
//      instead of guessing. No kernel behaviour changed.
//
//  D2. `SYS_CPUPULSE` (49) is defined and dispatched ONLY by the x86 kernel; the aarch64 kernel does
//      not declare 49 at all — even though the x86 declaration's own comment states the number "is
//      minted on BOTH arches per the shared-numbering law". `user-pulse` is built for both arches
//      (PULSE.ELF and PULSE-X86.ELF) and calls 49 unconditionally. RESOLVED as a RESERVATION: the
//      number lives here for both arches, the aarch64 kernel still answers `-ENOSYS` (its behaviour
//      is the truth and implementing the handler is not this arc), and the divergence is now stated
//      in one place instead of being invisible. `user-pulse` already tolerates the failure.
//
//  D3. The BUS v1 wire header was declared twice — `arch/aarch64/bus.rs` (`BUS_*`) and again in
//      `user-blob/src/midden.rs` (`HDR`, `FRAME_MAX`, `VERB_*`), with midden's copy carrying only the
//      comment "mirrors arch/aarch64/bus.rs byte-for-byte" as its guarantee. Values agreed at the
//      freeze; the guarantee is now structural. RESOLVED: the wire constants live here and both
//      sides import them.
//
//  D4. Sentinel exit statuses and the ELF-1 witness token are kernel-side `const`s matched against
//      immediates hand-written into ring-3 asm (`movz x0, #0x82`, `#0xB5`, `#0x1E`). Values agreed.
//      RESOLVED: declared here; the kernel imports them and the blobs assert against them (the asm
//      sites keep their immediates — see `user-blob/src/owner.rs` — with a `const _: () = assert!`
//      tripwire beside each, because an `asm!` immediate cannot be a `use`d path).
//
//  Not a divergence, recorded so the next reader does not re-derive it: verbs 3, 6, 19, 20, 24, 25,
//  31, 32 are aarch64-only and 33, 40..=49 are x86-only TODAY. Those are implementation gaps at
//  AGREED numbers, not drift, and every one of them is reserved on both arches below.
// -------------------------------------------------------------------------------------------------

#![no_std]
#![forbid(unsafe_code)]
// Every consumer imports a SUBSET. A shared ABI table whose unused half warns would push each caller
// into `#[allow]`s or, worse, into re-declaring only what it needs — the thing this crate exists to
// stop. The table is deliberately complete.
#![allow(dead_code)]

// =================================================================================================
// Calling convention
// =================================================================================================
//
// aarch64: `svc #0`; x8 = number, args x0..x5, return in x0. The kernel's SVC path restores the full
//   x0-x30 + FP register file, so a ring-3 stub may declare its argument registers as plain `in(...)`
//   — see the REGISTER-SURVIVAL INVARIANT note in `user-vug/src/main.rs`.
// x86_64: `syscall`; rax = number, args rdi/rsi/rdx/r10 (SysV order with r10 for arg 4, because
//   `syscall` destroys rcx), return in rax. The kernel's `sysretq` tail SCRUBS rdi/rsi/rdx/r8/r9/r10
//   to zero on EVERY return regardless of arity, and `syscall` itself destroys rcx and r11, so an
//   x86 stub must declare all eight as clobbers whatever its own arity.
//
// A negative return is `-errno`. Every verb that returns a value returns it non-negative, and the
// packed input event deliberately keeps bit 63 clear so it can never be mistaken for one.

// =================================================================================================
// Syscall numbers
// =================================================================================================
//
// 1..=33 is the SHARED block: a number here means the same verb on both arches, whether or not both
// implement it. 40..=49 is the block the x86 socket family was moved into (see `SYS_SOCKET`).
//
// Implementation matrix at the freeze (`Y` = dispatched, `-` = reserved, falls to `-ENOSYS`):
//
//     #   verb                 x86  arm     #   verb                 x86  arm
//     1   WRITE                 Y    Y     18   FGRANT                Y    Y
//     2   EXIT                  Y    Y     19   MSEND                 -    Y
//     3   REPORT                -    Y     20   MRECV                 -    Y
//     4   YIELD                 Y    Y     21   THREAD_SPAWN          Y    Y
//     5   SLEEP_MS              Y    Y     22   THREAD_EXIT           Y    Y
//     6   GETPID                -    Y     23   THREAD_JOIN           Y    Y
//     7   GETINFO               Y    Y     24   FB_MAP                -    Y
//     8   SPAWN                 Y    Y     25   FB_PRESENT            -    Y
//     9   WAIT                  Y    Y     26   FUTEX                 Y    Y
//    10   CAP                   Y    Y     27   INPUT_POLL            Y    Y
//    11   OPEN                  Y    Y     28   INPUT_WAIT            Y    Y
//    12   READ                  Y    Y     29   WIN_CREATE            Y    Y
//    13   XFER                  Y    Y     30   WIN_PRESENT           Y    Y
//    14   RECV                  Y    Y     31   WIN_MOVE              -    Y
//    15   SEEK                  Y    Y     32   WIN_CLOSE             -    Y
//    16   UNLINK                Y    Y     33   WIN_PRESENT_ROWS      Y    -
//    17   CLOSE                 Y    Y     40..=48  socket family     Y*   -
//                                          49   CPUPULSE              Y    -   (D2)
//
//   * the socket family is additionally gated on the `smolnet` kernel feature.
//
// A ring-3 program that calls a `-` verb gets `-ENOSYS` from that arch's dispatcher default arm.
// That is a designed outcome, not a failure mode: `user-vug`'s `present_rows` and its
// `SYS_INPUT_WAIT` park both carry an explicit fallback for exactly this.

/// `SYS_WRITE(fd, buf, len) -> bytes written / -errno`. `fd == 1` is the console; a `File` handle
/// carrying `CAP_WRITE` writes the file. The buffer is bound-checked against the caller's window.
pub const SYS_WRITE: u64 = 1;
/// `SYS_EXIT(status)` — never returns. The scheduler reclaims the task; the LAST thread of an
/// address space tears the slot down.
pub const SYS_EXIT: u64 = 2;
/// `SYS_REPORT(value) -> 0` — the demo accounting channel: hand the kernel a value read out of the
/// caller's own (slot-private) address space, keyed by the calling task's name, so a verdict can
/// check isolation. aarch64 only; x86 routes fixture witnesses by task name through `SYS_EXIT`
/// instead and has never needed it. Reserved on both arches regardless.
pub const SYS_REPORT: u64 = 3;
/// `SYS_YIELD() -> 0` — cooperatively give up the CPU.
pub const SYS_YIELD: u64 = 4;
/// `SYS_SLEEP_MS(ms) -> 0` — a real timed park on both arches. Milliseconds, NOT ticks: each kernel
/// converts through its own `ms_to_ticks`, which is why this argument needs no `GETINFO_TICK_HZ`
/// correction (contrast D1).
pub const SYS_SLEEP_MS: u64 = 5;
/// `SYS_GETPID() -> pid`. aarch64 only; nothing on x86 yet asks for a bare pid that `SYS_GETINFO`
/// does not already carry. Reserved on both arches.
pub const SYS_GETPID: u64 = 6;
/// `SYS_GETINFO(ptr) -> 0 / -EFAULT` — write [`UserInfo`]'s two words to the caller's buffer.
/// **Read [`GETINFO_TICK_HZ`] before touching the `ticks` field**: its unit is per-arch (D1).
pub const SYS_GETINFO: u64 = 7;
/// `SYS_SPAWN() -> child handle / -errno` — load the fixed on-disk program and run it as a CHILD,
/// returning a handle into the CALLER's table (never a raw pid).
pub const SYS_SPAWN: u64 = 8;
/// `SYS_WAIT(handle) -> child exit status / -ECHILD` — block until that child exits.
pub const SYS_WAIT: u64 = 9;
/// `SYS_CAP(op, ...) -> op-specific / -errno` — operate on the caller's own handle table as
/// capabilities. `op` is one of [`CAP_OP_GRANT`], [`CAP_OP_REVOKE`], [`CAP_OP_XREVOKE`].
pub const SYS_CAP: u64 = 10;
/// `SYS_OPEN(name_ptr, name_len, mode) -> File handle / -errno`. `mode` is the [`O_RW`] /
/// [`O_CREAT`] / [`O_PUBLIC`] bit set. The minted handle carries `CAP_READ` (plus `CAP_WRITE` when
/// opened RW).
pub const SYS_OPEN: u64 = 11;
/// `SYS_READ(handle, buf, len) -> count (0 = EOF) / -errno`. Needs `File` + [`CAP_READ`].
pub const SYS_READ: u64 = 12;
/// `SYS_XFER(dest_child_handle, src_handle, req_rights) -> transfer id / -errno` — deposit an
/// ATTENUATED copy of a capability into the recipient's inbox. The recipient is named owner-scoped,
/// by a `Child` handle the SENDER holds: there is no global process namespace.
pub const SYS_XFER: u64 = 13;
/// `SYS_RECV() -> handle / -errno` — pull a pending capability out of the CALLER's own inbox into
/// the CALLER's own handle row, so every row keeps its single writer.
pub const SYS_RECV: u64 = 14;
/// `SYS_SEEK(handle, offset) -> new offset / -errno`. Absolute. Seeking TO the size (the EOF
/// position) is legal; past it is `-EINVAL`.
pub const SYS_SEEK: u64 = 15;
/// `SYS_UNLINK(handle) -> 0 / -errno` — delete the file an open `File` + [`CAP_WRITE`] handle names.
/// Gated by the same single write CHECK, because a delete is a mutation.
pub const SYS_UNLINK: u64 = 16;
/// `SYS_CLOSE(handle) -> 0 / -errno`. Needs NO right — you may always close a handle you hold.
/// A double-close returns cleanly (`-EBADF`); a use-after-close is denied.
pub const SYS_CLOSE: u64 = 17;
/// `SYS_FGRANT(file_handle, child_handle, rights) -> 0 / -errno` — the owner of a private file
/// grants (a [`CAP_READ`]|[`CAP_WRITE`] subset) or revokes (`rights == 0`) access to the principal a
/// `Child` handle names. An ACL edge on the FILE; nothing is delivered to the grantee's table.
pub const SYS_FGRANT: u64 = 18;
/// `SYS_MSEND(frame_ptr, frame_len) -> 0 / -errno` — submit ONE v1 bus REQUEST frame (see the
/// `BUS_*` block). The kernel validates it fail-closed and STAMPS the sender's principal itself; a
/// caller-supplied principal is `-EINVAL`, never overwritten. aarch64 only today.
pub const SYS_MSEND: u64 = 19;
/// `SYS_MRECV(buf_ptr, buf_len) -> reply length / -errno` — dequeue the next reply, blocking while
/// the caller's mailbox is empty. aarch64 only today.
pub const SYS_MRECV: u64 = 20;
/// `SYS_THREAD_SPAWN(entry, sp, arg, place) -> thread handle / -errno` — a new ring-3 task in the
/// CALLER's OWN address space (same page tables), entered at `entry` on stack `sp` with `arg` in the
/// first-argument register. `place`: 0 = the caller's core, 1 = a sibling online core.
pub const SYS_THREAD_SPAWN: u64 = 21;
/// `SYS_THREAD_EXIT()` — never returns. Posts this thread's completion (waking a joiner) and drops
/// its hold on the shared address space; teardown happens only on the LAST thread.
pub const SYS_THREAD_EXIT: u64 = 22;
/// `SYS_THREAD_JOIN(handle) -> 0 / -ESRCH` — block until that thread finishes, then reap it.
pub const SYS_THREAD_JOIN: u64 = 23;
/// `SYS_FB_MAP() -> surface VA / -errno` — the SINGLE-surface compat path, superseded by
/// [`SYS_WIN_CREATE`]. aarch64 only. Ring 3 never receives the real scan-out either way.
pub const SYS_FB_MAP: u64 = 24;
/// `SYS_FB_PRESENT() -> 0 / -errno` — composite the compat surface. aarch64 only.
pub const SYS_FB_PRESENT: u64 = 25;
/// `SYS_FUTEX(uaddr, op, val) -> op-specific / -errno` — the wait/wake a ring-3 mutex or frame
/// barrier is built out of. `op` is [`FUTEX_WAIT`] or [`FUTEX_WAKE`]; `uaddr` is validated to lie in
/// the caller's writable window.
pub const SYS_FUTEX: u64 = 26;
/// `SYS_INPUT_POLL() -> packed event / -EAGAIN` — NON-BLOCKING drain of the calling process's own
/// input ring. The event layout is the `INPUT_EV_*` block; bit 63 is always clear.
pub const SYS_INPUT_POLL: u64 = 27;
/// `SYS_INPUT_WAIT() -> 0 / -EINVAL` — the BLOCKING half. Blocks until the ring MAY be non-empty and
/// dequeues NOTHING, so the caller's ordinary [`SYS_INPUT_POLL`] drain still sees every event and the
/// two compose without a "wait consumed my event" hazard.
pub const SYS_INPUT_WAIT: u64 = 28;
/// `SYS_WIN_CREATE(w, h) -> win id / -errno` — allocate a compositor window owned by the caller and
/// map its ARGB8888 surface (`stride = w * 4`) into the caller's window. The surface VA is published
/// in the read-only info page.
pub const SYS_WIN_CREATE: u64 = 29;
/// `SYS_WIN_PRESENT(win) -> 0 / -errno` — damage-mark + composite the WHOLE window. Fail-closed on a
/// free id (`-EBADF`) or another owner's window (`-EACCES`).
pub const SYS_WIN_PRESENT: u64 = 30;
/// `SYS_WIN_MOVE(win, x, y) -> 0 / -errno`. aarch64 only today.
pub const SYS_WIN_MOVE: u64 = 31;
/// `SYS_WIN_CLOSE(win) -> 0 / -errno`. aarch64 only today; on both arches slot teardown closes every
/// window the address space still owns.
pub const SYS_WIN_CLOSE: u64 = 32;
/// `SYS_WIN_PRESENT_ROWS(win, y0, y1) -> 0 / -errno` — the DAMAGE-CARRYING present: declare which
/// SOURCE rows changed so the compositor repaints only those.
///
/// ADDITIVE by necessity, not by taste. Widening 30 in place is not safe: three of the four ring-3
/// stub sets declare the extra argument registers as clobbers they never write, so junk that landed
/// in range would silently UNDER-repaint an existing caller of 30. A new number cannot be reached by
/// an old binary at all. x86 only today, which is why `user-vug`'s `present_rows` carries a
/// MANDATORY whole-box fallback rather than treating the `-ENOSYS` as an error.
pub const SYS_WIN_PRESENT_ROWS: u64 = 33;

// --- 40..=48: the x86 socket family. -------------------------------------------------------------
//
// It lives at 40..=48 and NOT at the 19..=27 it originally claimed. The shared-number law is that a
// number means the same verb on every arch, and aarch64 had already spent 19..=27 on MSEND/MRECV,
// the thread verbs, FB_MAP/FB_PRESENT, FUTEX and INPUT_POLL. Nothing caught the collision because
// the two families never had to coexist — until x86 grew the window and thread verbs, at which point
// `SYS_INPUT_POLL` and `SYS_ACCEPT` would both have had to be 27 in the SAME dispatch. Moving the
// x86-only family (the arch alone in using these ids) to a free contiguous block restored the law.
// Relative order is preserved so the family still reads the same.

/// `SYS_SOCKET(domain, ty) -> handle / -errno`. `ty`: 0 = datagram (UDP), 1 = stream (TCP). The
/// handle is a capability exactly like a `File`, minted with [`CAP_READ`]|[`CAP_WRITE`], so
/// [`SYS_CAP`] GRANT can attenuate a socket to send-only or recv-only.
pub const SYS_SOCKET: u64 = 40;
/// `SYS_BIND(handle, port) -> 0 / -errno` — name a local UDP port. Needs [`CAP_WRITE`] (a
/// configuring authority).
pub const SYS_BIND: u64 = 41;
/// `SYS_SENDTO(handle, msg_ptr, msg_len) -> count / -errno`. `msg` is [`SOCKADDR_HDR_LEN`] bytes of
/// destination header followed by the payload. Needs [`CAP_WRITE`].
pub const SYS_SENDTO: u64 = 42;
/// `SYS_RECVFROM(handle, buf, len) -> total / -EAGAIN` — writes the same header shape (SOURCE
/// address) followed by the payload. NON-BLOCKING: the IF-masked handler cannot block. Needs
/// [`CAP_READ`].
pub const SYS_RECVFROM: u64 = 43;
/// `SYS_CONNECT(handle, msg_ptr, msg_len) -> 0 / -EINPROGRESS / -ECONNREFUSED` — active-open to the
/// peer in `msg`'s [`SOCKADDR_HDR_LEN`]-byte header. NON-BLOCKING; ring 3 polls by re-calling.
pub const SYS_CONNECT: u64 = 44;
/// `SYS_SEND(handle, buf, len) -> count queued / -EAGAIN / -ENOTCONN`. Needs [`CAP_WRITE`].
pub const SYS_SEND: u64 = 45;
/// `SYS_SOCK_RECV(handle, buf, len) -> count / -EAGAIN / 0 at clean end-of-stream`. Named
/// `SOCK_RECV`, not `RECV`, because 14 is the capability-transfer inbox recv. Needs [`CAP_READ`].
pub const SYS_SOCK_RECV: u64 = 46;
/// `SYS_LISTEN(handle, backlog) -> 0 / -errno` — arm a passive listener.
pub const SYS_LISTEN: u64 = 47;
/// `SYS_ACCEPT(handle) -> new handle / -EAGAIN` — poll for an inbound connection and mint a fresh
/// socket capability for it.
pub const SYS_ACCEPT: u64 = 48;

/// `SYS_CPUPULSE(ptr) -> 0 / -EFAULT` — copy the per-core load SAMPLE ([`UserPulse`]'s layout) out
/// to ring 3.
///
/// It hands out RAW CUMULATIVE counters, never percentages, and that is the point: "load" is only
/// defined relative to a refresh window the kernel does not know, and a caller handed a pre-cooked
/// percent could not tell an honest 0% from a FROZEN counter — the one distinction the display rule
/// exists to preserve. `(busy, idle)` per core plus the observer's own core index gives ring 3 every
/// input that rule needs and no fabricated ones.
///
/// x86 dispatches it; aarch64 does not (D2) — reserved there, answers `-ENOSYS`.
pub const SYS_CPUPULSE: u64 = 49;

// =================================================================================================
// Sub-ops and flags — the encodings ring 3 passes IN
// =================================================================================================

/// [`SYS_FUTEX`] `op`: block iff `*uaddr == val`.
pub const FUTEX_WAIT: u64 = 0;
/// [`SYS_FUTEX`] `op`: wake up to `val` waiters on that key; returns the count woken.
pub const FUTEX_WAKE: u64 = 1;

/// [`SYS_CAP`] op: `a1` = source handle, `a2` = requested rights -> a new ATTENUATED handle to the
/// same target. Requires [`CAP_GRANT`] on the source.
pub const CAP_OP_GRANT: u64 = 0;
/// [`SYS_CAP`] op: `a1` = handle to drop -> 0. Clears a handle the caller owns.
pub const CAP_OP_REVOKE: u64 = 1;
/// [`SYS_CAP`] op: `a1` = a transfer id [`SYS_XFER`] returned. Sender-only and single-level —
/// revoking makes the RECEIVED capability stale at its next resolve (and discards it if still
/// pending in the inbox), but does NOT cascade through further re-transfers.
pub const CAP_OP_XREVOKE: u64 = 2;

/// Capability rights mask. Gates [`SYS_READ`] and socket recv.
pub const CAP_READ: u32 = 1 << 0;
/// Gates [`SYS_WRITE`] to a `File`, [`SYS_UNLINK`], socket send, and socket configuration.
pub const CAP_WRITE: u32 = 1 << 1;
/// Reserved: execute.
pub const CAP_EXEC: u32 = 1 << 2;
/// Required on the SOURCE handle of a [`CAP_OP_GRANT`].
pub const CAP_GRANT: u32 = 1 << 3;
/// Revoking a handle that carries this kills its whole derivation SUBTREE.
pub const CAP_REVOKE: u32 = 1 << 4;

/// [`SYS_OPEN`] `mode` bit 0: open read-WRITE. Bit 0 clear is read-only.
pub const O_RW: u64 = 1 << 0;
/// [`SYS_OPEN`] `mode` bit 1: create the file if absent. A create is inherently RW, so the fixtures
/// pass `O_CREAT | O_RW` = 3. `O_TRUNC`/`O_EXCL`/`O_APPEND` stay reserved.
pub const O_CREAT: u64 = 1 << 1;
/// [`SYS_OPEN`] `mode` bit 2: opt an `O_CREAT` of a NEW name OUT of owned-by-default into
/// world-access. Ignored on an open of an existing file (ownership is fixed at create) and outside
/// `O_CREAT`. A private create records the creator as OWNER; this keeps the pre-U6 open-by-anyone
/// behaviour.
pub const O_PUBLIC: u64 = 1 << 2;

// =================================================================================================
// Packed layouts — what ring 3 reads BACK
// =================================================================================================

/// [`SYS_INPUT_POLL`]'s packed event: the type field's shift. `[55:48]` is the type, the payload is
/// the low 32 bits, and bit 63 is always clear so an event can never be mistaken for `-errno`.
pub const INPUT_EV_TYPE_SHIFT: u32 = 48;
/// Mask for the type field once shifted down.
pub const INPUT_EV_TYPE_MASK: u64 = 0xFF;

/// A key PRESS. Payload `[7:0]` = ASCII, or one of the `KEY_*` C0 arrow codes.
pub const INPUT_EV_KEY_DOWN: u64 = 1;
/// A key RELEASE. Same payload as the press.
pub const INPUT_EV_KEY_UP: u64 = 2;
/// Relative pointer motion. Payload `[31:16]` = dx, `[15:0]` = dy, both `i16`.
pub const INPUT_EV_MOUSE_REL: u64 = 3;
/// Absolute pointer position. Payload `[31:16]` = x, `[15:0]` = y, both `i16`.
pub const INPUT_EV_MOUSE_ABS: u64 = 4;
/// A pointer button state. Payload `[7:0]` = button bitmask.
pub const INPUT_EV_BUTTON: u64 = 5;

/// Pack an event the way both kernels do. One definition, so the encode and the decode below cannot
/// drift apart.
#[inline]
pub const fn input_ev_pack(ty: u64, payload: u64) -> u64 {
    (ty << INPUT_EV_TYPE_SHIFT) | payload
}
/// The type field of a packed event.
#[inline]
pub const fn input_ev_type(ev: u64) -> u64 {
    (ev >> INPUT_EV_TYPE_SHIFT) & INPUT_EV_TYPE_MASK
}
/// The payload field of a packed event.
#[inline]
pub const fn input_ev_payload(ev: u64) -> u64 {
    ev & 0xFFFF_FFFF
}

/// Arrow keys ride the otherwise-unused C0 range `0x1C..=0x1F` (the four ASCII "information
/// separator" codes), so one byte of payload carries printable keys and arrows alike with no escape
/// sequences and no second field. Shift does not change an arrow.
pub const KEY_RIGHT: u8 = 0x1C;
/// See [`KEY_RIGHT`].
pub const KEY_LEFT: u8 = 0x1D;
/// See [`KEY_RIGHT`].
pub const KEY_DOWN: u8 = 0x1E;
/// See [`KEY_RIGHT`].
pub const KEY_UP: u8 = 0x1F;
/// ESC — plain ASCII, listed here because it sits one below the arrow block and every consumer of
/// the arrows wants it too.
pub const KEY_ESC: u8 = 0x1B;

/// [`SYS_GETINFO`]'s payload: `#[repr(C)]`, no padding, so the byte view is exactly the two fields
/// in declaration order — `pid` at 0, `ticks` at 8.
///
/// `pid` is the scheduler task id. For `ticks`, read [`GETINFO_TICK_HZ`]: **its unit is per-arch**.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UserInfo {
    /// The calling task's scheduler id.
    pub pid: u64,
    /// Monotonic since boot, in units of [`GETINFO_TICK_HZ`] per second.
    pub ticks: u64,
}

/// The rate of [`UserInfo::ticks`], in Hz. **THE D1 CONSTANT** — this is the whole reason the
/// divergence ledger exists, so it is worth the paragraph.
///
/// The two kernels fill that field from two different clocks, and both are shipped, metal-proven
/// behaviour that this crate does not get to change:
///
///   * x86_64 — `arch::ticks()`, calibrated to `apic::TICK_HZ` = 1000 Hz. One tick is one
///     millisecond.
///   * aarch64 — `timer::ticks()`, the 250 Hz scheduler tick. One tick is four milliseconds.
///
/// Ring 3 must therefore NEVER hard-code a rate, and must never assume the field is milliseconds.
/// Ask this constant, or convert with [`getinfo_ticks_to_ms`]. Both mistakes were live in the tree:
/// `user-vug` assumed 250 Hz everywhere (fps 4x low on x86) and `user-pulse` assumed milliseconds
/// everywhere (a 3 s animation taking 12 s on the Pi).
///
/// The fallback arm exists only so the crate compiles when someone type-checks it for the host; no
/// UnaOS kernel runs on a third arch.
#[cfg(target_arch = "x86_64")]
pub const GETINFO_TICK_HZ: u64 = 1000;
/// See the `x86_64` arm.
#[cfg(target_arch = "aarch64")]
pub const GETINFO_TICK_HZ: u64 = 250;
/// See the `x86_64` arm.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub const GETINFO_TICK_HZ: u64 = 1000;

/// Convert a [`UserInfo::ticks`] reading (or a difference of two) to milliseconds on whichever arch
/// this is compiled for.
#[inline]
pub const fn getinfo_ticks_to_ms(ticks: u64) -> u64 {
    ticks * 1000 / GETINFO_TICK_HZ
}

/// The ABI cap on cores [`SYS_CPUPULSE`] reports. FIXED, because it sizes a `#[repr(C)]` struct a
/// ring-3 program declares from this document — it is ABI, not an implementation detail, and it is
/// deliberately NOT derived from the in-kernel meter's scratch cap (which is free to change without
/// breaking a shipped ring-3 binary). They happen to be equal today at 16.
pub const PULSE_MAX_CPUS: usize = 16;

/// The [`SYS_CPUPULSE`] payload as a count of `u64` words: `ncpu`, `demo`, then
/// [`PULSE_MAX_CPUS`] `(busy, idle)` pairs.
pub const PULSE_WORDS: usize = 2 + PULSE_MAX_CPUS * 2;

/// [`SYS_CPUPULSE`]'s payload. `#[repr(C)]` plain-old-data, no padding (all `u64`), so the byte
/// layout is stable for the ring-3 program reading it back: `ncpu` at 0, `demo` at 8, then
/// `PULSE_MAX_CPUS` pairs of cumulative `(busy, idle)` tick counts, core-major — core `c`'s busy
/// count at `16 + c*16`, its idle count at `24 + c*16`. 272 bytes total. Entries at or past `ncpu`
/// are ZEROED, never stale, so a caller that trusts `ncpu` and one that scans the whole array agree.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UserPulse {
    /// Cores the meter covers, `<= PULSE_MAX_CPUS`.
    pub ncpu: u64,
    /// The core the CALLER is executing on.
    pub demo: u64,
    /// `(busy, idle)` pairs, core-major.
    pub ticks: [u64; PULSE_MAX_CPUS * 2],
}

/// The 8-byte address header the UDP/TCP socket verbs put in front of a payload:
/// `[ip[4]][port u16 LE][pad u16]`.
pub const SOCKADDR_HDR_LEN: usize = 8;

// =================================================================================================
// The BUS v1 wire (D3) — the on-UnaOS SMessage transport carried by SYS_MSEND / SYS_MRECV
// =================================================================================================

/// Frame magic: `b"UBS1"` — UnaOS Bus, wire v1.
pub const BUS_MAGIC: [u8; 4] = *b"UBS1";
/// Wire version. A bump is a protocol break — rule on it; never bump it silently.
pub const BUS_VERSION: u8 = 1;
/// Header length. Layout (little-endian):
///   `0..4` magic, `4..5` version, `5..6` kind, `6..7` verb, `7..8` reserved(=0),
///   `8..12` corr(u32), `12..16` status(i32), `16..48` principal(32 B), `48..52` body_len(u32).
///
/// REQUEST: `status` MUST be 0 and `principal` MUST be all-zero — the kernel stamps the sender's
/// record itself, after validation, and a caller-supplied one is rejected rather than overwritten.
/// REPLY: `verb` and `corr` echo the request; `status` is 0 with a typed body, or a negative errno
/// with an EMPTY body.
pub const BUS_HDR_LEN: usize = 52;
/// HARD decode ceiling for the body — the max forced allocation per frame.
pub const BUS_BODY_MAX: usize = 4096;
/// The whole-frame ceiling: header + max body.
pub const BUS_FRAME_MAX: usize = BUS_HDR_LEN + BUS_BODY_MAX;

/// Frame kind: a request from ring 3.
pub const BUS_KIND_REQUEST: u8 = 1;
/// Frame kind: the kernel's reply. A reply echoes its request's verb.
pub const BUS_KIND_REPLY: u8 = 2;

/// Read-side verb: list the root directory.
pub const BUS_VERB_LS: u8 = 1;
/// Read-side verb: read a root file.
pub const BUS_VERB_CAT: u8 = 2;
/// Read-side verb: copy a root file.
pub const BUS_VERB_CP: u8 = 3;
/// Write-side verb: create-or-truncate a root file with a typed content payload.
pub const BUS_VERB_WRITE: u8 = 4;
/// Write-side verb: unlink a root file by name — owner-only, durable ACL clear first.
pub const BUS_VERB_RM: u8 = 5;
/// Write-side verb: rename a root file in place — owner-only, ACL name re-bind.
pub const BUS_VERB_MV: u8 = 6;

/// 8.3 name bound — mirrors the `SYS_OPEN` name bound the equivalence witness holds `cat`/`cp` to.
pub const BUS_NAME_MAX: usize = 12;

// =================================================================================================
// Shared sentinel values (D4)
// =================================================================================================
//
// These are magic numbers a ring-3 program writes and the kernel matches. They are ABI in the only
// sense that matters: change one side alone and a witness silently stops firing.

/// The `SYS_EXIT` status both K2 enforcement blobs use, so their exits are accounted separately.
pub const EXIT_STATUS_K2: u64 = 0x82;
/// The `SYS_EXIT` status `midden` uses.
pub const EXIT_STATUS_MIDDEN: u64 = 0xB5;
/// The token `ELFHELLO.ELF` hands the kernel through [`SYS_REPORT`].
pub const ELF1_WITNESS_TOKEN: u64 = 0x1E;

// =================================================================================================
// Errno — the negative returns ring 3 must recognise
// =================================================================================================
//
// Linux values, as `i64`, ready to compare a raw syscall return against.

/// No such process / thread handle.
pub const ESRCH: i64 = -3;
/// No such file or directory.
pub const ENOENT: i64 = -2;
/// Bad file descriptor / handle.
pub const EBADF: i64 = -9;
/// No such child.
pub const ECHILD: i64 = -10;
/// Would block — the honest answer from every non-blocking verb, never an error.
pub const EAGAIN: i64 = -11;
/// Permission denied — the capability and ACL refusal.
pub const EACCES: i64 = -13;
/// Bad address: the pointer failed the copy seam's validation.
pub const EFAULT: i64 = -14;
/// Invalid argument.
pub const EINVAL: i64 = -22;
/// Not connected.
pub const ENOTCONN: i64 = -107;
/// Operation now in progress — a TCP connect still handshaking.
pub const EINPROGRESS: i64 = -115;
/// Connection refused.
pub const ECONNREFUSED: i64 = -111;
/// The dispatcher's default arm: this kernel does not implement that number. A ring-3 program that
/// calls a verb its arch reserves but does not dispatch gets exactly this.
pub const ENOSYS: i64 = -38;

// =================================================================================================
// Self-checks — the table's own invariants, enforced at compile time in every consumer.
// =================================================================================================

/// The shared block is contiguous and the socket family starts clear of it: a future verb minted at
/// 34..=39 cannot silently land on a socket number, and a socket number cannot land on a shared one.
const _: () = assert!(SYS_WIN_PRESENT_ROWS == 33 && SYS_SOCKET == 40);
/// `SYS_CPUPULSE` is the high-water mark; the next verb minted takes 50.
const _: () = assert!(SYS_CPUPULSE > SYS_ACCEPT);
/// The packed event must never collide with a negative return: type 5 shifted left 48 leaves bit 63
/// clear with room to spare.
const _: () = assert!(input_ev_pack(INPUT_EV_BUTTON, 0xFFFF_FFFF) < (1u64 << 63));
/// The pulse payload's word count and its struct must describe the same bytes.
const _: () = assert!(PULSE_WORDS * 8 == core::mem::size_of::<UserPulse>());
/// `UserInfo` is the two-word POD its copy-out seam assumes.
const _: () = assert!(core::mem::size_of::<UserInfo>() == 16);
/// `O_CREAT | O_RW` is the `3` every create fixture passes.
const _: () = assert!(O_CREAT | O_RW == 3);
