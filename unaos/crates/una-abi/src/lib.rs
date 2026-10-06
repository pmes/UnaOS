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
//      RESOLVED: declared here, and BOTH sides now name this declaration. The asm blobs were the
//      expected exception — a raw `asm!` immediate cannot be a `use`d path, so the freeze looked
//      like it would have to settle for an `assert!` tripwire beside each one. It did not:
//      `naked_asm!`/`global_asm!` accept `const` OPERANDS, so `user-blob`, `user-blob-x86`,
//      `user-elf`, `owner.rs` and `imp.rs` feed these values (and their syscall numbers) straight
//      out of this crate — `mov x8, #{sys_write}`, `sys_write = const una_abi::SYS_WRITE`. That is
//      strictly stronger than a tripwire: a tripwire DETECTS a divergence, a `const` operand makes
//      one unrepresentable. It is also byte-neutral — every blob objcopies identically to the
//      hand-written immediates, which `arroyo kernel8`'s entry-bytes and page-budget assertions
//      independently confirm.
//
//  Not a divergence, recorded so the next reader does not re-derive it: verbs 3, 6, 19, 20, 24, 25,
//  31, 32 are aarch64-only and 33, 40..=49 are x86-only TODAY. Those are implementation gaps at
//  AGREED numbers, not drift, and every one of them is reserved on both arches below.
// -------------------------------------------------------------------------------------------------

#![no_std]
#![deny(unsafe_code)] // RING3ABI2 (B333): was `forbid`; ONE `#[allow(unsafe_code)]` exists — `args()`, the ring-3 read of the fixed args page (target_os = "none" only, never on the host)
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
/// `SYS_WIN_PRESENT(win) -> status / -errno` — damage-mark + composite the WHOLE window. Fail-closed
/// on a free id (`-EBADF`) or another owner's window (`-EACCES`). The non-error statuses (bit 63
/// clear, so every `rc >> 63` failure test keeps working unchanged): `0` = the surface reached the
/// compositor — composited, headless, or coalesced into the panel frame in flight, three cases ring 3
/// deliberately CANNOT distinguish; `1` = declined, every window the caller owns is hidden
/// (PRESSURE-1 — a good citizen stops rendering).
///
/// CRYSTAL-HD, on the status that is deliberately NOT here. The held CRYSTAL-PACE half added a third
/// non-error status (`WIN_PRESENT_COALESCED = 2`) so a renderer could lock its render loop to the x86
/// compositor's frame edge. It is dropped, for two reasons that stand independently:
///
/// * Peter's ruling of 2026-08-13 (`9d12e7e0`, `docs/dev/OS/08_VIDEO/PARITY.md` §5.1) — a vug renders
///   UNPACED on every chip, "more drawing complexity, never artificial pacing". A status whose only
///   consumer is a self-pacing render loop is that pacer with the sleep moved one syscall outward.
/// * `0` for every success keeps this verb's contract IDENTICAL on both arches. aarch64 has no
///   coalescing pacer and could never answer a third status, so a ring-3 program written against a
///   3-valued x86 contract would silently mean something else on the Pi — exactly the divergence the
///   present-rows port was careful not to open.
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

/// `SYS_RENAME(src_ptr, src_len, dst_ptr, dst_len) -> 0 / -errno` (STOR-1 M2) — re-bind a
/// runtime-created file from one name to another.
///
/// NAME to NAME, not handle to name, and the argument shape is MATCHED FROM THE Pi rather than
/// invented. aarch64 has no rename syscall at all, so its ONLY rename ABI is the v1 bus verb `mv`,
/// whose frozen body is `[src_len][src][dst]` — two names in that order, owner-only, create-new-only
/// on the destination. Taking the same two names in the same order with the same errno table is what
/// lets a program be written ONCE: `mv A B` spoken over the bus and `SYS_RENAME(A, B)` spoken
/// directly are then the SAME QUESTION, which is exactly the property the bus equivalence witness
/// exists to assert. A handle-based rename could not have been that — and on x86 it could barely
/// have worked, because after STOR-1 M1 a created file commonly has NO open descriptor and so no
/// handle to name it by.
///
/// The fourth argument rides `r10` from ring 3 on x86 (`SYSCALL` destroys `rcx`), as [`SYS_THREAD_SPAWN`]'s
/// fourth does, and the entry stub moves it to the 5th C register; on aarch64 it is `x3`, like every SVC's.
///
/// Errnos SHARED by both arches for the same cause: `-EINVAL` (empty or oversized name, checked before any
/// byte is read) · `-EFAULT` (a bad name pointer) · `-ENOENT` (no such source, a non-UTF-8 name, or a name
/// with a directory component — both rename in the root) · `-EACCES` (not the owner: DELETE authority, since
/// a content grantee able to re-point a name could steal it; x86 also the shared window and a staged name,
/// aarch64 the ACL store) · `-EEXIST` (the destination exists). Arch storage conditions: x86 `-EBUSY` (the
/// destination's deferred delete is pending, or a RELEASE of the source is mid-way on another core — a
/// transient) and `-EIO`; aarch64 `-EISDIR` `-ENODEV` `-EAGAIN` `-EIO`. NEITHER arch refuses an OPEN source
/// (STOR-2, rmbp-ledger B185): the descriptor follows the FILE, and the old name is `-ENOENT` while it lives.
///
/// Dispatched on BOTH arches since STOR-2: aarch64's arm is `bus_mv`'s body under the caller's own identity,
/// x86's is the body `busx_mv` calls, so `mv A B` and `SYS_RENAME(A, B)` are one function on each arch.
pub const SYS_RENAME: u64 = 50;

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
/// WHEEL: one scroll-wheel report. Payload `[7:0]` = the HID boot-mouse wheel byte, a SIGNED `i8` in
/// the usual HID sense — POSITIVE is scroll-up / away from the user, negative is scroll-down. Ring 3
/// must sign-extend: `input_ev_payload(ev) as u8 as i8`. Only ever emitted for a NONZERO delta, so a
/// consumer never sees a no-op detent; and only from a report that ACTUALLY carried a wheel byte — a
/// 3-byte relative boot report (a mouse with no wheel) emits nothing rather than a fabricated zero.
pub const INPUT_EV_WHEEL: u64 = 6;
/// APPCLIP (R61): a RESOLVED DESKTOP ACTION — the `⌘C`/`⌘V`/`⌘X`/`⌘A` family, judged by the theme's
/// binding table at the HID decoder and delivered to the focused program exactly the way a key is.
/// Payload `[7:0]` = the action's DISCRIMINANT, which is never `0`:
/// `1` screenshot, `2` screenshot-region, `3` copy, `4` cut, `5` paste, `6` select-all, `7` log-out,
/// and TERMSEL's selection actions `8` select-left, `9` select-right, `10` select-line-start,
/// `11` select-line-end, `12` deselect (appended, 2026-09-23), and TERMSEL2's caret actions `13`
/// cursor-left, `14` cursor-right, `15` cursor-line-start, `16` cursor-line-end (appended, 2026-09-23).
/// The kernel side of that mapping is `video::clipboard::action_code` — ONE definition, read by both
/// arches' `pack_input`. Adding an action appends a value; changing one is an ABI break.
///
/// A program that does not implement the clipboard ignores this exactly as it ignores
/// [`INPUT_EV_WHEEL`]: the type is unknown to it and its dispatch falls through. `Ctrl-C` never
/// arrives as one of these — the table does not claim it, so it is still an ordinary
/// [`INPUT_EV_KEY_DOWN`] carrying `0x03`, which is R61's point.
///
/// Numbered 7, the next free code, and NOT inserted beside [`INPUT_EV_KEY_UP`] where it belongs by
/// kind: these values are on the wire, so the block reads in kind order only until the first
/// addition, and what the ABI fixes is the number, not its position in this file.
pub const INPUT_EV_ACTION: u64 = 7;

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
///   * x86_64 — `arch::ticks()`, from the local-APIC heartbeat ARMED at `apic::TICK_HZ` = 1000 Hz.
///     One tick is one millisecond. Strictly: one tick is one millisecond once `apic::calibrate` has
///     set the divisor; before that the heartbeat runs on the fixed fallback count (~0.8 ms/tick
///     under QEMU, unmeasured on metal), so the earliest ticks of a boot are approximate. Every
///     consumer of this timebase already lives with that — `arch::ms_to_ticks`, and therefore
///     `sched::sleep_ms`, derive from the same armed rate — and it is orders of magnitude smaller
///     than the 4x this constant exists to prevent.
///   * aarch64 — `timer::ticks()`, the 250 Hz scheduler tick. One tick is four milliseconds; the
///     down-counter reload is `CNTFRQ / TICK_HZ`, so there is no calibration phase to qualify.
///
/// Both kernels bind this constant to their own timer with a `const _: () = assert!` beside
/// `sys_getinfo` (`== apic::TICK_HZ` on x86, `== super::timer::TICK_HZ` on aarch64), so this is not a
/// fourth hand-copy of the number: retuning either heartbeat without moving this constant is a
/// COMPILE ERROR, not a silently wrong fps readout.
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
/// deliberately NOT derived from `vug::MAX_METER_CPUS` (the in-kernel meter's scratch cap, which is
/// free to change without breaking a shipped ring-3 binary). They happen to be equal today at 16.
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
/// NOTICE: raise a notice on the glass — body = up to two lines of text (`\n`-separated, printable ASCII), the
/// TITLE is the caller's own program name (kernel-stamped owner, never caller-supplied). Fire-and-forget:
/// the reply is an empty status-0 frame. 7/8/9 are APPMENU's MENU_PUBLISH/CLEAR/GET; 10 is the next free tag.
pub const BUS_VERB_NOTICE: u8 = 10;

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
/// `SYS_RENAME` is the high-water mark; the next verb minted takes 51. (STOR-1 M2 took 50, which is
/// the number this line said was next — the note is kept accurate rather than kept.)
const _: () = assert!(SYS_RENAME > SYS_CPUPULSE && SYS_CPUPULSE > SYS_ACCEPT);
/// The packed event must never collide with a negative return: type 5 shifted left 48 leaves bit 63
/// clear with room to spare.
const _: () = assert!(input_ev_pack(INPUT_EV_BUTTON, 0xFFFF_FFFF) < (1u64 << 63));
/// The pulse payload's word count and its struct must describe the same bytes.
const _: () = assert!(PULSE_WORDS * 8 == core::mem::size_of::<UserPulse>());
/// `UserInfo` is the two-word POD its copy-out seam assumes.
const _: () = assert!(core::mem::size_of::<UserInfo>() == 16);
/// `O_CREAT | O_RW` is the `3` every create fixture passes.
const _: () = assert!(O_CREAT | O_RW == 3);

// =================================================================================================
// APPMENU (R73, arc (a)) — the menu verb. An app publishes a menu TREE over the frozen bus; the kernel
// registry holds it keyed by the caller's kernel-stamped identity; a pick comes back to the owner as
// `INPUT_EV_MENU_PICK`. No new syscall: `SYS_MSEND`/`SYS_MRECV` are board-uniform since BUSX86. The
// design ledger is `video/menubar.rs` (THE MENU PROTOCOL); the two corrections it needed are applied
// here — the input tag is 8 (6 and 7 landed first), and the transport is no longer aarch64-only.
// Appended at the file tail so no existing line moves.
// =================================================================================================

/// Bus verb: publish this principal's menu tree (replaces its entry). Body = [`MenuWireHdr`] then
/// `count` [`MenuWireItem`]s, exactly `MENU_HDR_LEN + count * MENU_ITEM_LEN` bytes. Refused whole.
pub const BUS_VERB_MENU_PUBLISH: u8 = 7;
/// Bus verb: drop this principal's menu entry. Empty body.
pub const BUS_VERB_MENU_CLEAR: u8 = 8;
/// Bus verb: read a menu tree. Body = empty (the caller's own) or 8 bytes LE = the target owner id.
/// The reply body is the tree in the publish shape, or EMPTY when that owner has none.
pub const BUS_VERB_MENU_GET: u8 = 9;

/// A menu pick delivered to the TREE'S OWNER (never to the focused slot). Payload `[31:0]` = the
/// publisher's own item id. Bit 63 stays clear ([`input_ev_pack`]'s invariant).
pub const INPUT_EV_MENU_PICK: u64 = 8;
/// WINRESIZE (R75): the window system resized this process's window (a frame drag, or Ctrl+Shift+arrow).
/// Payload `[31:16]` = new content width, `[15:0]` = new content height, both SOURCE pixels (`u16`). The
/// surface slot and stride are unchanged: the program redraws `w` x `h` of it. Programs that do not
/// resize ignore the unknown type. Numbered 9, the next free code.
pub const INPUT_EV_WIN_RESIZE: u64 = 9;
/// APPMENU2 (B393): the window manager asks this process to QUIT (Cmd-Q, the app menu's Quit, the dock
/// tile's Quit). Payload `[31:0]` = the window id the request was taken on. The app may save and close its
/// windows itself and exit; the WM kills the process if it is still running 3 s later. Programs that do not
/// handle it are killed at the bound, exactly as before. Numbered 10, the next free code.
pub const INPUT_EV_CLOSE_REQ: u64 = 10;
/// APPMENU2 — the bound, in milliseconds, between a close request and the WM's kill.
pub const CLOSE_REQ_BOUND_MS: u64 = 3000;

/// The wire encoding's version. A bump is a protocol break: rule on it, never bump silently.
pub const MENU_WIRE_VERSION: u8 = 1;
/// Label bytes per item, ASCII, not NUL-terminated.
pub const MENU_LABEL_MAX: usize = 24;
/// Levels: a title and its rows. Deeper is refused.
pub const MENU_DEPTH_MAX: usize = 2;
/// Items per tree, total across depths.
pub const MENU_ITEMS_MAX: usize = 64;
/// Item flag: not pickable.
pub const MENU_FLAG_DISABLED: u32 = 1 << 0;
/// Item flag: a separator row.
pub const MENU_FLAG_SEPARATOR: u32 = 1 << 1;
/// Item flag: carries a check mark.
pub const MENU_FLAG_CHECKED: u32 = 1 << 2;
/// Item flag: a top-level TITLE whose children (items whose `parent` is this item's `id`) are its rows.
pub const MENU_FLAG_SUBMENU: u32 = 1 << 3;
/// The flag bits a publisher may set; any other bit refuses the tree.
pub const MENU_FLAGS_KNOWN: u32 = MENU_FLAG_DISABLED | MENU_FLAG_SEPARATOR | MENU_FLAG_CHECKED | MENU_FLAG_SUBMENU;

/// One item on the wire: fixed width, so the kernel walks records and parses nothing.
/// `parent == 0` is a top-level item, which MUST be a `MENU_FLAG_SUBMENU` title with a nonzero `id`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MenuWireItem {
    pub id: u32,
    pub parent: u32,
    pub flags: u32,
    pub label_len: u8,
    pub label: [u8; MENU_LABEL_MAX],
    pub _pad: [u8; 3],
}
/// Bytes per wire item.
pub const MENU_ITEM_LEN: usize = 40;
/// The body header: `[version u8][count u8][0][0]`.
pub const MENU_HDR_LEN: usize = 4;

impl MenuWireItem {
    /// An all-zero item (an unused record).
    pub const ZERO: MenuWireItem = MenuWireItem { id: 0, parent: 0, flags: 0, label_len: 0, label: [0; MENU_LABEL_MAX], _pad: [0; 3] };
    /// Build an item from an ASCII label; a label over [`MENU_LABEL_MAX`] is truncated HERE (a ring-3
    /// helper's convenience) — the kernel never truncates, it refuses.
    pub const fn new(id: u32, parent: u32, flags: u32, label: &[u8]) -> MenuWireItem {
        let mut it = MenuWireItem::ZERO;
        it.id = id;
        it.parent = parent;
        it.flags = flags;
        let n = if label.len() < MENU_LABEL_MAX { label.len() } else { MENU_LABEL_MAX };
        let mut i = 0;
        while i < n {
            it.label[i] = label[i];
            i += 1;
        }
        it.label_len = n as u8;
        it
    }
    /// The 40 wire bytes, little-endian, padding zero.
    pub const fn to_bytes(&self) -> [u8; MENU_ITEM_LEN] {
        let mut b = [0u8; MENU_ITEM_LEN];
        let (i, p, f) = (self.id.to_le_bytes(), self.parent.to_le_bytes(), self.flags.to_le_bytes());
        let mut k = 0;
        while k < 4 {
            b[k] = i[k];
            b[4 + k] = p[k];
            b[8 + k] = f[k];
            k += 1;
        }
        b[12] = self.label_len;
        let mut j = 0;
        while j < MENU_LABEL_MAX {
            b[13 + j] = self.label[j];
            j += 1;
        }
        b
    }
    /// Decode one record from exactly [`MENU_ITEM_LEN`] bytes (`None` on a wrong length).
    pub fn from_bytes(b: &[u8]) -> Option<MenuWireItem> {
        if b.len() != MENU_ITEM_LEN {
            return None;
        }
        let rd = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let mut label = [0u8; MENU_LABEL_MAX];
        label.copy_from_slice(&b[13..13 + MENU_LABEL_MAX]);
        Some(MenuWireItem { id: rd(0), parent: rd(4), flags: rd(8), label_len: b[12], label, _pad: [b[37], b[38], b[39]] })
    }
}

/// The fixed record is 40 bytes and a full tree fits one bus body with room.
const _: () = assert!(core::mem::size_of::<MenuWireItem>() == MENU_ITEM_LEN);
const _: () = assert!(MENU_HDR_LEN + MENU_ITEMS_MAX * MENU_ITEM_LEN <= BUS_BODY_MAX);
const _: () = assert!(MENU_ITEMS_MAX <= 255); // the header's count is a u8
const _: () = assert!(input_ev_pack(INPUT_EV_MENU_PICK, 0xFFFF_FFFF) < (1u64 << 63));

#[cfg(test)]
mod appmenu_tests {
    extern crate std;
    use super::*;
    #[test]
    fn appmenu_wire_fits() {
        assert_eq!(MENU_ITEMS_MAX * MENU_ITEM_LEN, 2560);
        let it = MenuWireItem::new(7, 1, MENU_FLAG_CHECKED, b"Run");
        assert_eq!(MenuWireItem::from_bytes(&it.to_bytes()), Some(it));
        std::println!(":: APPMENU: abi items={} wire_bytes={} cap_bytes={} body_max={} -> PASS ::", MENU_ITEMS_MAX, MENU_ITEM_LEN, MENU_ITEMS_MAX * MENU_ITEM_LEN, BUS_BODY_MAX);
    }
}

// =================================================================================================
// PREFS (rmbp-ledger B300): Principia's preference verbs on the v1 wire — additive verb tags, no header
// or ceiling change (the BANDY-2 shape). Fulfilled in-kernel by `crate::prefs::bus_fulfil` over the ONE
// store (`<home>/.config/unaos/preferences.toml`). Bodies (ASCII; a value is a TOML scalar literal):
// GET `<ns>.<key>` -> reply body = the literal, -ENOENT unset; SET `<ns>.<key>` NUL `<literal>` -> empty,
// -EACCES unless the caller runs in the open session; LIST `<ns>` or empty -> `<ns>.<key> = <literal>\n` lines.
// ===================================================================================/// Bus verb: read one preference.
pub const BUS_VERB_PREF_GET: u8 = 16;
/// Bus verb: set one preference (session user only).
pub const BUS_VERB_PREF_SET: u8 = 17;
/// Bus verb: list a namespace (or every namespace).
pub const BUS_VERB_PREF_LIST: u8 = 18;
/// RESERVED for BANDY-3: the unsolicited PrefChanged frame (kind REPLY, corr 0, body = the SET body).
/// Not admitted by `verb_valid` until interest registration exists to deliver it.
pub const BUS_VERB_PREF_CHANGED: u8 = 19;
// BANDY3 (ROADMAP §3b, the fulfiller seam) — fulfiller registration on the wire. A ring-3 program
// registers the verb tags it fulfils; the kernel relays a caller's request to it re-stamped with the
// CALLER's principal (the kernel's stamp, never the caller's claim) and relays the fulfiller's answer
// back as a KERNEL-stamped reply. The verb space splits at 128: `1..=127` is the kernel's (a register
// of any of them is `-EEXIST` — kernel fulfilment wins); `128..=255` is registrable. The correlation id
// of a relay rides the header's `corr` field of the frame the kernel BUILDS for the fulfiller (the
// header is unchanged; see docs/dev/evidence/rmbp-1001/BANDY3.md). Appended at the file tail.
// =================================================================================================

/// Bus verb: register this row as the fulfiller of the verb tags in the body (1..=
/// [`BUS_REG_MAX_PER_ROW`] tags, one byte each, all `>=` [`BUS_VERB_FULFIL_MIN`], no duplicates). Reply:
/// an empty status-0 frame, or `-EINVAL` / [`EEXIST`] / [`ENOSPC`]. A kernel-owned tag at the top of the
/// kernel range, clear of the next-free counter the in-kernel verbs mint from.
pub const BUS_VERB_REGISTER: u8 = 127;
/// The first REGISTRABLE verb tag. Every tag below it is the kernel's.
pub const BUS_VERB_FULFIL_MIN: u8 = 128;
/// Registrations one row may hold.
pub const BUS_REG_MAX_PER_ROW: usize = 8;
/// The ring-3 fulfiller DEMO pair (BANDY3): a preference read answered by `PREFS.ELF` from the registrable
/// range. Principia's real verbs are the kernel-fulfilled `BUS_VERB_PREF_*` (16..=19, PREFS); this pair
/// proves registration and relay, and retires when a ring-3 Principia takes the real tags over.
pub const BUS_VERB_R3PREF_GET: u8 = 128;
/// Principia's preference listing: body = a key prefix (may be empty); reply body = `key=value\n` lines.
pub const BUS_VERB_R3PREF_LIST: u8 = 129;

/// File exists — and, on the bus, "that verb already has a fulfiller" (the kernel, or another live row).
pub const EEXIST: i64 = -17;
/// No space — a full registration table, or a row over [`BUS_REG_MAX_PER_ROW`].
pub const ENOSPC: i64 = -28;
/// Connection reset — the fulfiller a caller was waiting on exited before it answered.
pub const ECONNRESET: i64 = -104;

const _: () = assert!(BUS_VERB_REGISTER < BUS_VERB_FULFIL_MIN);
const _: () = assert!(BUS_VERB_NOTICE < BUS_VERB_REGISTER && BUS_VERB_MENU_GET < BUS_VERB_REGISTER);
const _: () = assert!(BUS_VERB_R3PREF_GET >= BUS_VERB_FULFIL_MIN && BUS_VERB_R3PREF_LIST >= BUS_VERB_FULFIL_MIN);
const _: () = assert!(BUS_REG_MAX_PER_ROW <= u8::MAX as usize);

#[cfg(test)]
mod bandy3_tests {
    extern crate std;
    use super::*;
    #[test]
    fn bandy3_verb_space() {
        // Every in-kernel verb is below the register tag; the registrable range starts right after it.
        for v in [BUS_VERB_LS, BUS_VERB_CAT, BUS_VERB_CP, BUS_VERB_WRITE, BUS_VERB_RM, BUS_VERB_MV, BUS_VERB_NOTICE, BUS_VERB_MENU_PUBLISH, BUS_VERB_MENU_CLEAR, BUS_VERB_MENU_GET] {
            assert!(v < BUS_VERB_REGISTER);
        }
        assert_eq!(BUS_VERB_FULFIL_MIN, BUS_VERB_REGISTER + 1);
        assert_ne!(BUS_VERB_R3PREF_GET, BUS_VERB_R3PREF_LIST);
        // A full registration body fits a frame with room.
        assert!(BUS_REG_MAX_PER_ROW < BUS_BODY_MAX);
        std::println!(":: BANDY3: abi register={} fulfil_min={} pref_get={} pref_list={} max_per_row={} -> PASS ::", BUS_VERB_REGISTER, BUS_VERB_FULFIL_MIN, BUS_VERB_R3PREF_GET, BUS_VERB_R3PREF_LIST, BUS_REG_MAX_PER_ROW);
    }
}

// ATTRSURF (B299) — the typed-attribute surface: five syscalls, five bus verbs, ONE byte layout.
//
// The syscall inputs ARE the bus request bodies and the syscall outputs ARE the bus reply bodies, so
// a program speaks the same bytes either way (the SYS_RENAME / `mv` equivalence, kept on purpose).
// All integers little-endian. Appended at the file tail so no existing line moves.
//
//   value wire   [AttrWireHdr: tag u8, rsv [u8;3] = 0, len u32][len payload bytes]
//                tag 1 int (len 8, i64) · 2 float (len 8, f64) · 3 string (UTF-8) · 4 blob ·
//                5 vector (len % 4 == 0, f32 each); len <= ATTR_VALUE_MAX
//   request      [path_len u16][key_len u16][path][key][value wire — SET only, absent otherwise]
//                path absolute, 1..=ATTR_PATH_MAX; key 0..=ATTR_KEY_MAX (0 only for LIST)
//   LIST reply   repeated [key_len u16][key][value wire]
//   QUERY reply  repeated [id u64][path_len u16][path]
//   STAT reply   UserStat (32 bytes)
// =================================================================================================

/// `SYS_ATTR_SET(req_ptr, req_len) -> 0 / -errno` — set (create or replace) one typed attribute.
pub const SYS_ATTR_SET: u64 = 51;
/// `SYS_ATTR_GET(req_ptr, req_len, out_ptr, out_cap) -> value-wire bytes written / -errno`
/// (`-ENODATA` absent key, `-ERANGE` out too small).
pub const SYS_ATTR_GET: u64 = 52;
/// `SYS_ATTR_LIST(path_ptr, path_len, out_ptr, out_cap) -> bytes written / -errno`.
pub const SYS_ATTR_LIST: u64 = 53;
/// `SYS_QUERY(expr_ptr, expr_len, out_ptr, out_cap) -> bytes written / -errno` — every readable hit.
pub const SYS_QUERY: u64 = 54;
/// `SYS_STAT(path_ptr, path_len, out_ptr) -> 0 / -errno` — writes one [`UserStat`].
pub const SYS_STAT: u64 = 55;

/// Bus verbs, same bodies as the syscalls above (request body = syscall input; reply body = output).
pub const BUS_VERB_ATTR_SET: u8 = 11;
pub const BUS_VERB_ATTR_GET: u8 = 12;
pub const BUS_VERB_ATTR_LIST: u8 = 13;
pub const BUS_VERB_ATTR_QUERY: u8 = 14;
pub const BUS_VERB_ATTR_STAT: u8 = 15;

/// Value tags.
pub const ATTR_TAG_INT: u8 = 1;
pub const ATTR_TAG_FLOAT: u8 = 2;
pub const ATTR_TAG_STR: u8 = 3;
pub const ATTR_TAG_BLOB: u8 = 4;
pub const ATTR_TAG_VECTOR: u8 = 5;

/// Payload ceiling of one value. A SET request at the ceiling still fits one bus body.
pub const ATTR_VALUE_MAX: usize = 3072;
/// Path and key ceilings (each fits the u16 length field with room).
pub const ATTR_PATH_MAX: usize = 255;
pub const ATTR_KEY_MAX: usize = 255;
/// Bytes of the value header.
pub const ATTR_WIRE_HDR_LEN: usize = 8;
/// Bytes of the request's two length fields.
pub const ATTR_REQ_HDR_LEN: usize = 4;
/// Ceiling on an out buffer the kernel will fill (GET/LIST/QUERY) — one bus body.
pub const ATTR_OUT_MAX: usize = BUS_BODY_MAX;

/// `-ERANGE`: the caller's out buffer is smaller than the answer.
pub const ERANGE: i64 = -34;
/// `-ENODATA`: the object has no attribute by that key.
pub const ENODATA: i64 = -61;
/// `-ENOTSUP`: the volume carries no typed attributes (FAT).
pub const ENOTSUP: i64 = -95;
/// `-ENOTDIR` / `-EISDIR` / `-ENODEV` / `-EIO` as the attribute verbs report them.
pub const ENOTDIR: i64 = -20;
pub const EISDIR: i64 = -21;
pub const ENODEV: i64 = -19;
pub const EIO: i64 = -5;

/// The value header, `#[repr(C)]` so a ring-3 program may overlay it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttrWireHdr {
    pub tag: u8,
    pub _rsv: [u8; 3],
    pub len: u32,
}

impl AttrWireHdr {
    pub const fn to_bytes(&self) -> [u8; ATTR_WIRE_HDR_LEN] {
        let l = self.len.to_le_bytes();
        [self.tag, 0, 0, 0, l[0], l[1], l[2], l[3]]
    }
}

/// Validate one value wire at the head of `b`: returns `(tag, payload, bytes consumed)`. Fail-closed:
/// unknown tag, nonzero reserved bytes, a length past the ceiling or past `b`, a scalar whose len is
/// not 8, a vector whose len is not a multiple of 4, a string that is not UTF-8.
pub fn attr_wire_parse(b: &[u8]) -> Result<(u8, &[u8], usize), i64> {
    if b.len() < ATTR_WIRE_HDR_LEN || b[1] != 0 || b[2] != 0 || b[3] != 0 {
        return Err(EINVAL);
    }
    let tag = b[0];
    let len = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
    if len > ATTR_VALUE_MAX || ATTR_WIRE_HDR_LEN + len > b.len() {
        return Err(EINVAL);
    }
    let p = &b[ATTR_WIRE_HDR_LEN..ATTR_WIRE_HDR_LEN + len];
    let ok = match tag {
        ATTR_TAG_INT | ATTR_TAG_FLOAT => len == 8,
        ATTR_TAG_STR => core::str::from_utf8(p).is_ok(),
        ATTR_TAG_BLOB => true,
        ATTR_TAG_VECTOR => len % 4 == 0,
        _ => false,
    };
    if !ok {
        return Err(EINVAL);
    }
    Ok((tag, p, ATTR_WIRE_HDR_LEN + len))
}

/// Split a request: `(path, key, rest)`. `rest` is the value wire for SET and must be EMPTY for the
/// other verbs (the caller checks). Fail-closed on any length that does not fit.
pub fn attr_req_parse(b: &[u8]) -> Result<(&[u8], &[u8], &[u8]), i64> {
    if b.len() < ATTR_REQ_HDR_LEN {
        return Err(EINVAL);
    }
    let pl = u16::from_le_bytes([b[0], b[1]]) as usize;
    let kl = u16::from_le_bytes([b[2], b[3]]) as usize;
    if pl == 0 || pl > ATTR_PATH_MAX || kl > ATTR_KEY_MAX || ATTR_REQ_HDR_LEN + pl + kl > b.len() {
        return Err(EINVAL);
    }
    let path = &b[ATTR_REQ_HDR_LEN..ATTR_REQ_HDR_LEN + pl];
    let key = &b[ATTR_REQ_HDR_LEN + pl..ATTR_REQ_HDR_LEN + pl + kl];
    Ok((path, key, &b[ATTR_REQ_HDR_LEN + pl + kl..]))
}

/// Build a request into `out`; returns its length, or `None` when it does not fit / a field is
/// over its ceiling. `value` is an already-encoded value wire (empty for GET/LIST).
pub fn attr_req_build(path: &[u8], key: &[u8], value: &[u8], out: &mut [u8]) -> Option<usize> {
    let n = ATTR_REQ_HDR_LEN + path.len() + key.len() + value.len();
    if path.is_empty() || path.len() > ATTR_PATH_MAX || key.len() > ATTR_KEY_MAX || n > out.len() {
        return None;
    }
    out[0..2].copy_from_slice(&(path.len() as u16).to_le_bytes());
    out[2..4].copy_from_slice(&(key.len() as u16).to_le_bytes());
    let mut o = ATTR_REQ_HDR_LEN;
    out[o..o + path.len()].copy_from_slice(path);
    o += path.len();
    out[o..o + key.len()].copy_from_slice(key);
    o += key.len();
    out[o..o + value.len()].copy_from_slice(value);
    Some(n)
}

/// `SYS_STAT`'s answer. `flags` bit0 = `id` valid, bit1 = `mtime` valid (unix seconds).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UserStat {
    /// 0 file, 1 directory.
    pub kind: u32,
    pub flags: u32,
    pub size: u64,
    pub id: u64,
    pub mtime: u64,
}
pub const STAT_HAS_ID: u32 = 1;
pub const STAT_HAS_MTIME: u32 = 2;
pub const USER_STAT_LEN: usize = 32;

impl UserStat {
    pub const fn to_bytes(&self) -> [u8; USER_STAT_LEN] {
        let mut b = [0u8; USER_STAT_LEN];
        let (k, f, s, i, m) = (self.kind.to_le_bytes(), self.flags.to_le_bytes(), self.size.to_le_bytes(), self.id.to_le_bytes(), self.mtime.to_le_bytes());
        let mut j = 0;
        while j < 4 { b[j] = k[j]; b[4 + j] = f[j]; j += 1; }
        j = 0;
        while j < 8 { b[8 + j] = s[j]; b[16 + j] = i[j]; b[24 + j] = m[j]; j += 1; }
        b
    }
}

/// The number line moved: `SYS_STAT` is the high-water mark, the next verb minted takes 56.
const _: () = assert!(SYS_ATTR_SET == SYS_RENAME + 1 && SYS_STAT == 55);
const _: () = assert!(core::mem::size_of::<UserStat>() == USER_STAT_LEN);
const _: () = assert!(core::mem::size_of::<AttrWireHdr>() == ATTR_WIRE_HDR_LEN);
/// A SET at every ceiling fits one bus body.
const _: () = assert!(ATTR_REQ_HDR_LEN + ATTR_PATH_MAX + ATTR_KEY_MAX + ATTR_WIRE_HDR_LEN + ATTR_VALUE_MAX <= BUS_BODY_MAX);
const _: () = assert!(BUS_VERB_ATTR_SET > BUS_VERB_NOTICE);

#[cfg(test)]
mod attrsurf_tests {
    extern crate std;
    use super::*;
    #[test]
    fn attr_wire_roundtrip_and_refusals() {
        let mut buf = [0u8; 64];
        let v = 42i64.to_le_bytes();
        let mut val = [0u8; 16];
        val[..8].copy_from_slice(&AttrWireHdr { tag: ATTR_TAG_INT, _rsv: [0; 3], len: 8 }.to_bytes());
        val[8..].copy_from_slice(&v);
        let n = attr_req_build(b"/home/A", b"n", &val, &mut buf).unwrap();
        let (p, k, rest) = attr_req_parse(&buf[..n]).unwrap();
        assert_eq!((p, k), (b"/home/A".as_slice(), b"n".as_slice()));
        let (tag, payload, used) = attr_wire_parse(rest).unwrap();
        assert_eq!((tag, payload, used), (ATTR_TAG_INT, v.as_slice(), 16));
        // refusals
        assert_eq!(attr_wire_parse(&[9, 0, 0, 0, 0, 0, 0, 0]), Err(EINVAL)); // unknown tag
        assert_eq!(attr_wire_parse(&[1, 0, 0, 0, 4, 0, 0, 0, 1, 2, 3, 4]), Err(EINVAL)); // int len 4
        assert_eq!(attr_wire_parse(&[5, 0, 0, 0, 3, 0, 0, 0, 1, 2, 3]), Err(EINVAL)); // vector len 3
        assert_eq!(attr_wire_parse(&[3, 1, 0, 0, 0, 0, 0, 0]), Err(EINVAL)); // reserved byte
        assert_eq!(attr_req_parse(&[0, 0, 0, 0]), Err(EINVAL)); // empty path
        assert_eq!(attr_req_parse(&[5, 0, 0, 0, b'/']), Err(EINVAL)); // path overruns
        let st = UserStat { kind: 1, flags: STAT_HAS_ID, size: 7, id: 9, mtime: 0 }.to_bytes();
        assert_eq!(st[0], 1);
        assert_eq!(st[16], 9);
        std::println!(":: ATTRSURF-ABI: wire hdr={} value_max={} syscalls=51..=55 bus=11..=15 -> PASS ::", ATTR_WIRE_HDR_LEN, ATTR_VALUE_MAX);
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// Multi-frame replies (VEINCORE B304 minted it; LUMENAPP B323 retired the chat verbs 130..=133 that first
// used it, R82 — the flag stays as bus mechanism for any registered fulfiller).
// ─────────────────────────────────────────────────────────────────────────────────────────────────
/// The one POSITIVE reply status: a non-final frame of a multi-frame answer. The relay delivers it WITH
/// its body and keeps the correlation open; status 0 or an errno closes it.
pub const BUS_STATUS_MORE: i32 = 1;
// ==========================================================================================
// NETRING3 (B306) — entropy and name resolution for ring 3, the rungs under a metal HTTPS client.
// Both numbers mean the same verb on both arches. Appended at the file tail so no existing line moves.
// ==========================================================================================
/// `SYS_GETRANDOM(buf, len) -> bytes written / -errno` — fill `buf` from the kernel DRBG (SHA-256,
/// seeded from RDSEED/RDRAND/RNDR plus cycle-counter jitter, reseeded every 64 KiB). At most
/// [`GETRANDOM_MAX`] bytes per call; a short count is a legal answer and ring 3 loops for more.
pub const SYS_GETRANDOM: u64 = 56;
/// Most bytes one `SYS_GETRANDOM` call writes.
pub const GETRANDOM_MAX: usize = 256;
/// `SYS_RESOLVE(name_ptr, name_len, out_ptr) -> 0 / -errno` — resolve a DNS name through the kernel's
/// resolver (the DHCP-leased nameserver, gateway fallback). Writes [`RESOLVE_OUT_LEN`] bytes:
/// `[v4 4][v6 16]` (the v6 slot is zero in v1). `-ENODEV` no network stack / no NIC, `-ENOENT` no
/// answer, `-EINVAL` an empty or over-long name (1..=[`RESOLVE_NAME_MAX`]).
pub const SYS_RESOLVE: u64 = 57;
/// Bytes `SYS_RESOLVE` writes.
pub const RESOLVE_OUT_LEN: usize = 20;
/// Longest name `SYS_RESOLVE` accepts.
pub const RESOLVE_NAME_MAX: usize = 253;
// =================================================================================================
// RING3WIN (rmbp-ledger B316) — the ELF window. A UnaOS ring-3 program gets the address space its ELF
// asks for: beside the classic 16 KiB window at the window base (and the FB hole above it, which is ABI
// and does not move), every address space carries a second window at `base + USER_XWIN_OFF` of
// `USER_WINDOW_BYTES`. An image whose lowest PT_LOAD p_vaddr is `>= USER_XWIN_VA_X86` is placed there
// (p_vaddr = the absolute VA `USER_XWIN_VA_X86 + …`); its stack sits at the top (PT_GNU_STACK p_memsz or
// `USER_STACK_DEFAULT`) and `SYS_SBRK` grows its heap from the page after its highest segment. An image
// linked at 0 keeps the fixed 16 KiB model unchanged, and gets the whole ELF window as its heap.
// arroyo's `USER_WINDOW_BYTES` and the kernel's constant are proven equal to this one by
// `unaos/scripts/window-parity.sh`.
// =================================================================================================

/// Grow (or query, delta 0; or shrink, delta < 0) the caller's heap. Returns the OLD break VA, or a
/// negative errno (`ENOMEM` past the cap / the stack guard / out of frames). Additive after NETRING3's
/// 56 (SYS_GETRANDOM) and 57 (SYS_RESOLVE).
pub const SYS_SBRK: u64 = 58;
/// Out of memory — the ELF window cap, or the kernel heap, refused the request.
pub const ENOMEM: i64 = -12;
/// Byte offset of the ELF window from the ring-3 window base: PD entry 1 of the slot's user PDPT, clear
/// of the 2 MiB the classic window + FB hole live in.
pub const USER_XWIN_OFF: u64 = 0x20_0000;
/// x86_64: the ring-3 window base (USER_BASE, PML4 index 2 = 1 TiB) — the kernel asserts it equals its own.
pub const USER_BASE_X86: u64 = 0x0000_0100_0000_0000;
/// x86_64: the ELF window's absolute VA. An elf-model program LINKS here (`. = 0x10000200000;` in its
/// linker script): its p_vaddr are real addresses, so absolute pointers in its data (vtables, `&str` in
/// statics, `core::fmt`) are correct with no relocation — the linuxabi shape (fixed vaddrs).
pub const USER_XWIN_VA_X86: u64 = USER_BASE_X86 + USER_XWIN_OFF;
/// The ELF window size = the per-program cap (image span + stack + guard, or heap): 64 MiB. WINDOW2
/// (rmbp-ledger B361, R85 "the window will need to be raised, might as well do it now") raised it from
/// RING3WIN's 4 MiB: Holocron's Argon2id runs in ring 3 again, Lumen gets a real face beside TLS. The page
/// tables behind it are taken on demand (x86: one PT per 2 MiB touched; aarch64: one L3), never `.bss`.
pub const USER_WINDOW_BYTES: u64 = 64 << 20;
/// The stack an elf-model program gets when its ELF declares none (no PT_GNU_STACK or p_memsz 0).
pub const USER_STACK_DEFAULT: u64 = 64 << 10;
/// The largest stack an elf-model program may declare.
pub const USER_STACK_MAX: u64 = 1 << 20;
/// The classic fixed program window (code + data + two stack pages) — unchanged since U1a.
pub const USER_FIXED_WINDOW_BYTES: u64 = 16 << 10;

const _: () = assert!(USER_XWIN_OFF % (2 << 20) == 0 && USER_WINDOW_BYTES % (2 << 20) == 0);
const _: () = assert!(USER_STACK_MAX + 4096 < USER_WINDOW_BYTES && USER_STACK_DEFAULT <= USER_STACK_MAX);
const _: () = assert!(SYS_SBRK > SYS_STAT + 2); // 56/57 are NETRING3's

#[cfg(test)]
mod ring3win_tests {
    extern crate std;
    use super::*;
    #[test]
    fn ring3win_window_constants() {
        assert_eq!(SYS_SBRK, 58);
        assert_eq!(USER_WINDOW_BYTES, 67_108_864); // WINDOW2 (B361): 64 MiB
        assert!(USER_XWIN_OFF >= USER_FIXED_WINDOW_BYTES + 0x14_5000); // clear of the FB hole
        assert_eq!(USER_XWIN_VA_X86, 0x0000_0100_0020_0000);
        std::println!(":: RING3WIN-ABI: sbrk={} window={} xwin_off={:#x} -> PASS ::", SYS_SBRK, USER_WINDOW_BYTES, USER_XWIN_OFF);
    }
}
// TRASHTIME (B308) — what a trashed object IS, declared once for both Finders (the kernel's Quarry
// Trash, `fs/trash.rs`, and Matrix's host Finder over a UnaFS vault): three attributes ON the object,
// found by one query scoped to `/home/<user>/.Trash/`. Strings, not code: zero bytes in an EL0 blob.
/// The original absolute path (String).
pub const ATTR_KEY_TRASH_ORIGIN: &str = "una:trash-origin";
/// When it was trashed, unix seconds (Int).
pub const ATTR_KEY_TRASH_TIME: &str = "una:trash-time";
/// The session user who trashed it (String).
pub const ATTR_KEY_TRASH_BY: &str = "una:trash-by";
/// The Trash folder's name under the user's home.
pub const TRASH_DIR_NAME: &str = ".Trash";
/// The listing query (every object carrying an origin; callers scope it to their Trash folder).
pub const TRASH_QUERY: &str = "una:trash-origin != \"\"";
// EXECNAME (B322, R82) — a ring-3 program DECLARES how it is launched, in its own image: one ELF note
// in section `.note.unaos.app` (SHT_NOTE, allocated, kept by each x86 link script under a PT_NOTE
// header). Name "UnaOS" (namesz 6, padded to 8), type APP_NOTE_TYPE, desc one LE u32 of APP_FLAG_*.
// The shell reads it (`midden_core::app_note_flags`) when a bare name resolves to a program: WINDOWED or
// RESIDENT detaches (the `bg` path: job row, window title), otherwise the program runs in the foreground
// (the `run` path). A missing note is flags 0 — foreground. Zero code in an EL0 blob: a const struct.
/// The note's section name (the user crates' `#[link_section]`).
pub const APP_NOTE_SECTION: &str = ".note.unaos.app";
/// The note's owner name, NUL-terminated as the ELF note format counts it (namesz = 6).
pub const APP_NOTE_NAME: &[u8; 6] = b"UnaOS\0";
/// The note's type: the launch-flags record.
pub const APP_NOTE_TYPE: u32 = 1;
/// The program creates a window (SYS_WIN_CREATE): a bare-name launch detaches.
pub const APP_FLAG_WINDOWED: u32 = 1 << 0;
/// The program stays running to serve (a bus fulfiller, e.g. PREFS.ELF): a bare-name launch detaches.
pub const APP_FLAG_RESIDENT: u32 = 1 << 1;
/// The whole note record exactly as it lies in the image (24 bytes, 4-aligned).
#[repr(C, align(4))]
pub struct AppNote {
    pub namesz: u32,
    pub descsz: u32,
    pub ntype: u32,
    pub name: [u8; 8],
    pub desc: u32,
}
impl AppNote {
    /// The note a program with `flags` carries:
    /// `#[used] #[link_section = ".note.unaos.app"] static APP_NOTE: AppNote = AppNote::new(..);`
    pub const fn new(flags: u32) -> Self {
        let n = APP_NOTE_NAME;
        AppNote { namesz: 6, descsz: 4, ntype: APP_NOTE_TYPE, name: [n[0], n[1], n[2], n[3], n[4], n[5], 0, 0], desc: flags }
    }
}
const _: () = assert!(core::mem::size_of::<AppNote>() == 24);

// SETTINGSBUS (rmbp-ledger B337) — Principia's ring-3 WRITE tag beside BANDY3's read pair. The kernel's
// preference client (`prefs_client.rs`) offers every PREF_SET to whoever holds this tag (PREFS.ELF) before
// the kernel fulfiller answers; body = the PREF_SET body (`<ns>.<key>` NUL `<TOML literal>`), reply empty
// or an errno. Retires with the read pair when a ring-3 Principia takes 16..=19 over. Appended at the tail.
pub const BUS_VERB_R3PREF_SET: u8 = 130;
const _: () = assert!(BUS_VERB_R3PREF_SET >= BUS_VERB_FULFIL_MIN && BUS_VERB_R3PREF_SET != BUS_VERB_R3PREF_GET && BUS_VERB_R3PREF_SET != BUS_VERB_R3PREF_LIST);
// SELFDIAG (rmbp-ledger B324, R82) — whole-path file I/O for ring 3, fulfilled by the kernel over the VFS
// (the ATTRSURF pattern: one request buffer, the kernel resolves the path in the live namespace). The
// diagnosis program (APPS/DIAG.ELF) reads the boot log, the owners table and the selfhost tree, and writes
// its record and the patched tree. Under the kernel feature `selfdiag` (x86); off, the numbers fall to the
// unknown-syscall default. Appended at the file tail so no existing line moves.
//
//   request  [path_len u16][flags u16][rsv u32 = 0][offset u64][path][data — WRITE only]
//            path absolute, 1..=PATH_IO_PATH_MAX, no `..` component; data 0..=PATH_IO_MAX
/// `SYS_PATH_READ(req_ptr, req_len, out_ptr, out_cap) -> bytes read / -errno` (0 = end of file). At most
/// `PATH_IO_MAX` bytes per call from `offset`.
pub const SYS_PATH_READ: u64 = 59;
/// `SYS_PATH_WRITE(req_ptr, req_len) -> bytes written / -errno`. Writes `data` at `offset` (the file must
/// exist and `offset <= size`), or with [`PATH_W_TRUNC`] creates-or-truncates first (offset must be 0).
/// [`PATH_W_UNLINK`] removes the file (empty data). [`PATH_W_MKDIRS`] creates missing parent directories.
pub const SYS_PATH_WRITE: u64 = 60;
pub const PATH_W_TRUNC: u16 = 1 << 0;
pub const PATH_W_MKDIRS: u16 = 1 << 1;
pub const PATH_W_UNLINK: u16 = 1 << 2;
/// Bytes of the request header.
pub const PATH_IO_HDR_LEN: usize = 16;
/// Path ceiling.
pub const PATH_IO_PATH_MAX: usize = 255;
/// Data ceiling per call (read or write).
pub const PATH_IO_MAX: usize = 32 * 1024;
const _: () = assert!(SYS_PATH_READ == SYS_SBRK + 1 && SYS_PATH_WRITE == SYS_PATH_READ + 1);
// =================================================================================================
// RING3ABI2 (rmbp-ledger B333) — the ring-3 surface's loose ends: argv/envp (the ARGS PAGE), who am I
// (`SYS_WHOAMI`), and the aarch64 twin of the ELF window. Appended at the file tail; no line above moves.
// Design: docs/dev/evidence/rmbp-1005/RING3ABI2.md.
// =================================================================================================
//
// THE ARGS PAGE. Every address space a loader places carries one read-only page at a FIXED VA:
// `USER_ARGS_VA` = (x86) `USER_BASE_X86 + USER_ARGS_OFF` — the last page of the slot's first 2 MiB,
// above the FB hole and below the ELF window, the same VA in the fixed, flat and elf models — and
// (aarch64) `USER_EXT_BASE_ARM + USER_ARGS_OFF`, the same offset in the slot's EXTENSION GiB (see
// `USER_EXT_BASE_ARM`). The kernel writes it before the program's first instruction; ring 3 reads it
// (it is never a legal syscall OUTPUT buffer; it IS a legal input, so `argv[i]` may be handed straight
// to `SYS_OPEN`/`SYS_RESOLVE`). Layout, all little-endian, at most `USER_ARGS_BYTES`:
//
//     +0   u32  ARGS_MAGIC ("ARGS")
//     +4   u32  argc (0..=ARGS_MAX; 0 = launched by a path that passes none, e.g. a kernel fixture)
//     +8   u64  window_base — the classic window base VA (the FB landmarks hang off it: +0x4000 info
//               page, +0x5000 surface slot 0). An elf-model program cannot derive it from `_start`.
//     +16  u64  argv[0..argc] — ABSOLUTE ring-3 VAs of NUL-terminated strings inside this page
//     ...  u64  0 (the argv terminator; envp is empty in v1 and follows as one more 0)
//     ...  u8   the strings, NUL-joined, in argv order
//
// argv[0] is the word that named the program (`net`, or the path given to `run`/`bg`).

/// `SYS_WHOAMI(buf, len) -> bytes written / -errno` — who the CALLER runs as: `[WhoAmIHdr][name][home]`
/// (see [`whoami_build`]). `-ENOENT` no session (an anonymous program), `-ERANGE` a buffer shorter than
/// the record, `-EFAULT` a bad buffer. Both arches dispatch it.
pub const SYS_WHOAMI: u64 = 61; // merge12 fold: 59/60 are SELFDIAG's SYS_PATH_READ/WRITE; WHOAMI moved from 59
/// Byte offset of the args page from the window base (x86) / the extension base (aarch64).
pub const USER_ARGS_OFF: u64 = 0x1F_F000;
/// The args page is one page.
pub const USER_ARGS_BYTES: usize = 4096;
/// The most argv words one launch passes.
pub const ARGS_MAX: usize = 32;
/// The args page's first word.
pub const ARGS_MAGIC: u32 = u32::from_le_bytes(*b"ARGS");
/// The fixed header before `argv[]`.
pub const ARGS_HDR_LEN: usize = 16;
/// aarch64: the EXTENSION base — L1 entry 481 of every slot's own TTBR0 table (1 GiB at 481 GiB). The
/// classic aarch64 window has no fixed VA on the Pi (it is the identity PA of a kernel `.bss` anchor), so
/// an image that LINKS at an absolute address needs a VA that is the same on every board and that no
/// kernel mapping can reach: above every identity window (Pi RAM GiB 0..=3, Orin DRAM and PCIe below
/// ~200 GiB, the Orin's own classic window at 480 GiB) and below the 512 GiB ceiling of the 39-bit
/// TTBR0 VA (T0SZ = 25). Inside it the x86 offsets are reused verbatim: args page at +`USER_ARGS_OFF`,
/// ELF window at +`USER_XWIN_OFF` for `USER_WINDOW_BYTES`.
pub const USER_EXT_BASE_ARM: u64 = 481 << 30;
/// aarch64: the ELF window's absolute VA — an elf-model aarch64 image links here.
pub const USER_XWIN_VA_ARM: u64 = USER_EXT_BASE_ARM + USER_XWIN_OFF;
/// x86_64: the args page VA.
pub const USER_ARGS_VA_X86: u64 = USER_BASE_X86 + USER_ARGS_OFF;
/// aarch64: the args page VA.
pub const USER_ARGS_VA_ARM: u64 = USER_EXT_BASE_ARM + USER_ARGS_OFF;
/// The args page VA on the arch this crate is built for.
#[cfg(target_arch = "aarch64")]
pub const USER_ARGS_VA: u64 = USER_ARGS_VA_ARM;
#[cfg(not(target_arch = "aarch64"))]
pub const USER_ARGS_VA: u64 = USER_ARGS_VA_X86;
/// The ELF window VA on the arch this crate is built for.
#[cfg(target_arch = "aarch64")]
pub const USER_XWIN_VA: u64 = USER_XWIN_VA_ARM;
#[cfg(not(target_arch = "aarch64"))]
pub const USER_XWIN_VA: u64 = USER_XWIN_VA_X86;

const _: () = assert!(USER_ARGS_OFF + USER_ARGS_BYTES as u64 <= USER_XWIN_OFF); // below the ELF window
const _: () = assert!(USER_ARGS_OFF >= USER_FIXED_WINDOW_BYTES + 0x14_5000); // above the FB hole
const _: () = assert!(USER_EXT_BASE_ARM % (1 << 30) == 0 && (USER_EXT_BASE_ARM >> 30) < 512);
const _: () = assert!(ARGS_HDR_LEN + (ARGS_MAX + 2) * 8 < USER_ARGS_BYTES);
const _: () = assert!(SYS_WHOAMI == SYS_SBRK + 3);

/// Lay out the args page for `words` into `out` (>= `USER_ARGS_BYTES`, zeroed by the caller or not —
/// every byte up to the returned length is written, the rest is zeroed). `page_va` is the VA the page
/// is mapped at in ring 3 (the argv pointers are absolute). `None` = more than `ARGS_MAX` words or the
/// strings do not fit; the kernel then refuses the launch rather than truncate a command line.
pub fn args_build(page_va: u64, window_base: u64, words: &[&str], out: &mut [u8]) -> Option<usize> {
    if words.len() > ARGS_MAX || out.len() < USER_ARGS_BYTES {
        return None;
    }
    let out = &mut out[..USER_ARGS_BYTES];
    for b in out.iter_mut() {
        *b = 0;
    }
    out[0..4].copy_from_slice(&ARGS_MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&(words.len() as u32).to_le_bytes());
    out[8..16].copy_from_slice(&window_base.to_le_bytes());
    let mut s = ARGS_HDR_LEN + (words.len() + 2) * 8; // argv[], its 0, envp's 0
    for (i, w) in words.iter().enumerate() {
        let b = w.as_bytes();
        if b.contains(&0) {
            return None;
        }
        let end = s.checked_add(b.len())?.checked_add(1)?;
        if end > USER_ARGS_BYTES {
            return None;
        }
        let p = ARGS_HDR_LEN + i * 8;
        out[p..p + 8].copy_from_slice(&(page_va + s as u64).to_le_bytes());
        out[s..s + b.len()].copy_from_slice(b);
        s = end; // the NUL is already there
    }
    Some(s)
}

/// A parsed args page. Every read is bounds-checked against the page, so a corrupt page is `None`, never
/// a fault.
#[derive(Clone, Copy)]
pub struct Args<'a> {
    page: &'a [u8],
    va: u64,
}

impl<'a> Args<'a> {
    /// Parse `page` (the args page's bytes) mapped at `va`. `None` if the magic or the count is wrong.
    pub fn parse(page: &'a [u8], va: u64) -> Option<Self> {
        if page.len() < ARGS_HDR_LEN || page[0..4] != ARGS_MAGIC.to_le_bytes() {
            return None;
        }
        let a = Args { page, va };
        if a.argc() > ARGS_MAX {
            return None;
        }
        Some(a)
    }
    fn u64_at(&self, o: usize) -> Option<u64> {
        let s = self.page.get(o..o + 8)?;
        Some(u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
    }
    /// The word count.
    pub fn argc(&self) -> usize {
        u32::from_le_bytes([self.page[4], self.page[5], self.page[6], self.page[7]]) as usize
    }
    /// The classic window base (the FB landmarks' anchor).
    pub fn window_base(&self) -> u64 {
        self.u64_at(8).unwrap_or(0)
    }
    /// Word `i` (without its NUL), or `None` past `argc` or for a pointer outside the page.
    pub fn get(&self, i: usize) -> Option<&'a [u8]> {
        if i >= self.argc().min(ARGS_MAX) {
            return None;
        }
        let p = self.u64_at(ARGS_HDR_LEN + i * 8)?;
        let off = p.checked_sub(self.va)? as usize;
        let tail = self.page.get(off..)?;
        let n = tail.iter().position(|&c| c == 0)?;
        Some(&tail[..n])
    }
    /// The ring-3 VA of word `i` (to hand a syscall the string in place).
    pub fn ptr(&self, i: usize) -> Option<u64> {
        self.get(i)?;
        self.u64_at(ARGS_HDR_LEN + i * 8)
    }
}

/// RING 3 ONLY: this program's args page, read in place at [`USER_ARGS_VA`]. `None` when the page does
/// not carry the magic (a loader that did not place one).
///
/// The ONE unsafe item in this crate. It is sound under the loader contract above: every address space
/// a UnaOS loader places maps `USER_ARGS_BYTES` readable bytes at `USER_ARGS_VA`, read-only to ring 3
/// and written only before the program's first instruction, so a `'static` shared slice over it is never
/// aliased by a writer. Compiled only for `target_os = "none"` (the ring-3 and kernel targets); the host
/// build of this crate has no such page and no such function.
#[cfg(target_os = "none")]
#[allow(unsafe_code)]
pub fn args() -> Option<Args<'static>> {
    // SAFETY: see the doc above — a mapped, read-only, never-rewritten page for the program's lifetime.
    let page: &'static [u8] = unsafe { core::slice::from_raw_parts(USER_ARGS_VA as *const u8, USER_ARGS_BYTES) };
    Args::parse(page, USER_ARGS_VA)
}

/// The `SYS_WHOAMI` record header: uid, gid (UnaOS has no group store; a user's group is its own uid),
/// and the two lengths that follow it (`name` then `home`, no NULs).
pub const WHOAMI_HDR_LEN: usize = 16;
/// The longest record `SYS_WHOAMI` writes (a 255-byte name and a 255-byte home).
pub const WHOAMI_MAX: usize = WHOAMI_HDR_LEN + 255 + 255;

/// Encode a `SYS_WHOAMI` answer: `[uid u32][gid u32][name_len u16][home_len u16][rsvd u32][name][home]`.
pub fn whoami_build(uid: u32, gid: u32, name: &[u8], home: &[u8], out: &mut [u8]) -> Option<usize> {
    if name.len() > 255 || home.len() > 255 {
        return None;
    }
    let n = WHOAMI_HDR_LEN + name.len() + home.len();
    let o = out.get_mut(..n)?;
    o[0..4].copy_from_slice(&uid.to_le_bytes());
    o[4..8].copy_from_slice(&gid.to_le_bytes());
    o[8..10].copy_from_slice(&(name.len() as u16).to_le_bytes());
    o[10..12].copy_from_slice(&(home.len() as u16).to_le_bytes());
    o[12..16].copy_from_slice(&[0; 4]);
    o[16..16 + name.len()].copy_from_slice(name);
    o[16 + name.len()..n].copy_from_slice(home);
    Some(n)
}

/// A decoded `SYS_WHOAMI` answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WhoAmI<'a> {
    pub uid: u32,
    pub gid: u32,
    pub name: &'a [u8],
    pub home: &'a [u8],
}

/// Decode the bytes `SYS_WHOAMI` wrote.
pub fn whoami_parse(b: &[u8]) -> Option<WhoAmI<'_>> {
    let h = b.get(..WHOAMI_HDR_LEN)?;
    let uid = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
    let gid = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
    let nl = u16::from_le_bytes([h[8], h[9]]) as usize;
    let hl = u16::from_le_bytes([h[10], h[11]]) as usize;
    let name = b.get(WHOAMI_HDR_LEN..WHOAMI_HDR_LEN + nl)?;
    let home = b.get(WHOAMI_HDR_LEN + nl..WHOAMI_HDR_LEN + nl + hl)?;
    Some(WhoAmI { uid, gid, name, home })
}

#[cfg(test)]
mod ring3abi2_tests {
    extern crate std;
    use super::*;
    #[test]
    fn args_page_roundtrip_and_refusals() {
        let mut page = [0xAAu8; USER_ARGS_BYTES];
        let n = args_build(USER_ARGS_VA_X86, USER_BASE_X86, &["net", "example.com"], &mut page).unwrap();
        assert!(n < 128);
        let a = Args::parse(&page, USER_ARGS_VA_X86).unwrap();
        assert_eq!(a.argc(), 2);
        assert_eq!(a.window_base(), USER_BASE_X86);
        assert_eq!(a.get(0), Some(b"net".as_slice()));
        assert_eq!(a.get(1), Some(b"example.com".as_slice()));
        assert_eq!(a.get(2), None);
        let p1 = a.ptr(1).unwrap();
        assert!(p1 > USER_ARGS_VA_X86 && p1 < USER_ARGS_VA_X86 + USER_ARGS_BYTES as u64);
        // argv terminator and the empty envp
        assert_eq!(&page[ARGS_HDR_LEN + 16..ARGS_HDR_LEN + 32], &[0u8; 16]);
        // zero words still carries the header (the window base)
        args_build(USER_ARGS_VA_ARM, 0x4020_0000, &[], &mut page).unwrap();
        let a = Args::parse(&page, USER_ARGS_VA_ARM).unwrap();
        assert_eq!((a.argc(), a.window_base()), (0, 0x4020_0000));
        // refusals: too many words, a word with a NUL, strings past the page, a bad magic
        let many = ["x"; ARGS_MAX + 1];
        assert!(args_build(USER_ARGS_VA_X86, 0, &many, &mut page).is_none());
        assert!(args_build(USER_ARGS_VA_X86, 0, &["a\0b"], &mut page).is_none());
        let big = std::string::String::from_utf8(std::vec![b'z'; 4000]).unwrap();
        assert!(args_build(USER_ARGS_VA_X86, 0, &[big.as_str(), big.as_str()], &mut page).is_none());
        let mut bad = page;
        bad[0] = 0;
        assert!(Args::parse(&bad, USER_ARGS_VA_X86).is_none());
        // a pointer outside the page is None, never a panic
        let mut evil = [0u8; USER_ARGS_BYTES];
        args_build(USER_ARGS_VA_X86, 0, &["a"], &mut evil).unwrap();
        evil[16..24].copy_from_slice(&(USER_ARGS_VA_X86 + 0x10_000).to_le_bytes());
        assert_eq!(Args::parse(&evil, USER_ARGS_VA_X86).unwrap().get(0), None);
        std::println!(":: RING3ABI2-ABI: args_va_x86={:#x} args_va_arm={:#x} xwin_arm={:#x} whoami={} -> PASS ::", USER_ARGS_VA_X86, USER_ARGS_VA_ARM, USER_XWIN_VA_ARM, SYS_WHOAMI);
    }
    #[test]
    fn whoami_roundtrip() {
        let mut b = [0u8; WHOAMI_MAX];
        let n = whoami_build(7, 7, b"peter", b"/home/peter", &mut b).unwrap();
        let w = whoami_parse(&b[..n]).unwrap();
        assert_eq!(w, WhoAmI { uid: 7, gid: 7, name: b"peter", home: b"/home/peter" });
        assert!(whoami_build(1, 1, b"x", b"/home/x", &mut [0u8; 8]).is_none());
        assert!(whoami_parse(&b[..n - 1]).is_none());
    }
}
// VEINTLS (LEDGER SR36) — wall-clock time for ring 3. A TLS client checks every certificate's validity
// window against NOW (RFC 5280 §6.1.3 (a)(2)), and ring 3 had only SYS_GETINFO's boot-relative ticks.
// `SYS_TIME() -> UTC Unix seconds (>= 0) / -EAGAIN` — the kernel's civil clock (`clock::unix_now`: the
// RTC or SNTP anchor plus the monotonic counter since). -EAGAIN while the clock has never been anchored
// this boot: a caller that needs the time (certificate checks) must refuse rather than guess.
// merge12 fold: 59/60 are SELFDIAG's path verbs, 61 WHOAMI, 62 PROF; this verb is 63.
/// `SYS_TIME() -> unix seconds / -EAGAIN` (VEINTLS, SR36). Both arches, unconditional.
pub const SYS_TIME: u64 = 63; // merge12 fold: 59/60 SELFDIAG, 61 WHOAMI, 62 PROF (PROFILE2), 63 TIME
const _: () = assert!(SYS_TIME == SYS_SBRK + 5);
// =================================================================================================
// PROFILE2 (rmbp-ledger B340, M5) — `SYS_PROF`: a program profiles itself. The kernel's sampler (the
// `prof` verb's rings) is shared: `OP_START` arms it at the tick rate when idle (the caller then owns the
// run), `OP_STOP` disarms a run the caller owns, `OP_READ` copies the caller's OWN samples (its tid, or
// its program id) into `buf` as `Sample` records — a bounded copy, at most `READ_MAX` records and never
// more than `len` bytes — and `OP_STATUS` returns how many the caller has. 62 sits clear of the numbers
// other arcs of this wave hold (59..61).
// =================================================================================================

/// `SYS_PROF(op, buf, len) -> i64` — see [`prof`].
pub const SYS_PROF: u64 = 62;

/// The `SYS_PROF` vocabulary.
pub mod prof {
    /// Arm the sampler at the tick rate if idle; returns the armed rate in Hz.
    pub const OP_START: u64 = 0;
    /// Disarm a run the caller armed; returns 0, or 1 when the run is not the caller's (left armed).
    pub const OP_STOP: u64 = 1;
    /// Copy the caller's own samples into `buf` (at most `len / SAMPLE_BYTES`, at most `READ_MAX`);
    /// returns the record count, or -14 (`EFAULT`) for a buffer the kernel could not write.
    pub const OP_READ: u64 = 2;
    /// The caller's sample count in the current (or last) run.
    pub const OP_STATUS: u64 = 3;
    /// INPUTSTALL (rmbp-ledger B375): a program NOTES one of its own events to the kernel's instrument —
    /// `SYS_PROF(OP_NOTE, kind, value)`, returns 0 (`-22` for an unknown kind). The kernel lines the note up
    /// beside its own stall columns (`[lag] stall … strand=`); nothing is stored per program.
    pub const OP_NOTE: u64 = 4;
    /// `OP_NOTE` kind: one presented frame; `value != 0` when the frame stranded (a worker band released and
    /// not started inside the barrier's yield budget — the vug's `strand=`).
    pub const NOTE_FRAME: u64 = 1;
    /// Records one `OP_READ` copies at most.
    pub const READ_MAX: usize = 512;
    /// Bytes of one [`Sample`] on the wire.
    pub const SAMPLE_BYTES: usize = 24;

    /// One sample: the interrupted PC, the first caller the kernel's frame walk found (0 when none —
    /// ring-3 samples are not walked), the task, its ring (0 or 3), CPU, walk depth and program id.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Sample {
        pub rip: u64,
        pub caller: u64,
        pub tid: u32,
        pub ring: u8,
        pub cpu: u8,
        pub depth: u8,
        pub prog: u8,
    }

    impl Sample {
        /// The little-endian wire form (`SAMPLE_BYTES`). `const fn`: this crate contributes no code.
        pub const fn to_bytes(&self) -> [u8; SAMPLE_BYTES] {
            let mut b = [0u8; SAMPLE_BYTES];
            let (r, c, t) = (self.rip.to_le_bytes(), self.caller.to_le_bytes(), self.tid.to_le_bytes());
            let mut i = 0;
            while i < 8 {
                b[i] = r[i];
                b[8 + i] = c[i];
                if i < 4 {
                    b[16 + i] = t[i];
                }
                i += 1;
            }
            b[20] = self.ring;
            b[21] = self.cpu;
            b[22] = self.depth;
            b[23] = self.prog;
            b
        }

        /// Decode one record (`None` for a short slice).
        pub const fn from_bytes(b: &[u8]) -> Option<Self> {
            if b.len() < SAMPLE_BYTES {
                return None;
            }
            Some(Self {
                rip: u64::from_le_bytes(le8(b, 0)),
                caller: u64::from_le_bytes(le8(b, 8)),
                tid: u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
                ring: b[20],
                cpu: b[21],
                depth: b[22],
                prog: b[23],
            })
        }
    }

    const fn le8(b: &[u8], o: usize) -> [u8; 8] {
        [b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7]]
    }

    const _: () = assert!(core::mem::size_of::<Sample>() == SAMPLE_BYTES);

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn sample_round_trips() {
            let s = Sample { rip: 0x1000_0020_0040, caller: 0xffff_8000_0000_1234, tid: 77, ring: 3, cpu: 5, depth: 0, prog: 2 };
            assert_eq!(Sample::from_bytes(&s.to_bytes()), Some(s));
            assert_eq!(Sample::from_bytes(&[0u8; SAMPLE_BYTES - 1]), None);
        }
    }
}
// LUMENUX (rmbp-ledger B348) — the ring-3 CLIPBOARD, the two verbs `video/clipboard.rs` named and did not build.
// Over the kernel's one session-owned text buffer (`video::clipboard::set`/`get`): text only (printable ASCII,
// `\n`, `\t`), at most [`CLIP_CAP`] bytes, refused rather than truncated. OWNERSHIP (the question clipboard.rs
// left open): only the program holding KEYBOARD FOCUS may set or read it (`-EACCES` otherwise), so a background
// program can neither overwrite nor read what the operator copied. 59..=62 are taken on exec-rmbp-merge12
// (SELFDIAG's SYS_PATH_READ/WRITE, RING3ABI2's SYS_WHOAMI, PROFILE2's SYS_PROF, VEINTLS's SYS_TIME is 63), hence 64/65. x86 dispatches
// them; elsewhere they fall to the unknown-syscall default. Appended at the file tail so no existing line moves.
/// `SYS_CLIP_SET(ptr, len) -> len / -errno` — replace the clipboard. `-EINVAL` a non-text byte or
/// `len > CLIP_CAP`, `-EACCES` not the focused program, `-EFAULT` a bad buffer. `len == 0` clears it.
pub const SYS_CLIP_SET: u64 = 64; // merge12 fold: 63 is VEINTLS's SYS_TIME
/// `SYS_CLIP_GET(ptr, cap) -> len / -errno` — copy the clipboard out. `-ERANGE` when `cap` is shorter than
/// the content (nothing is copied: ask again with `CLIP_CAP`), `-EACCES` not the focused program.
pub const SYS_CLIP_GET: u64 = 65;
/// The clipboard's capacity (the kernel's `video::clipboard::CLIP_CAP`; `tests lumen` asserts they agree).
pub const CLIP_CAP: usize = 4096;
// =================================================================================================
// HOLOCRON2 (rmbp-ledger B355) — what the metal's secrets handler (APPS/HOLOCRON.ELF) needs from the kernel.
// Design: docs/dev/evidence/rmbp-1005/HOLOCRON2.md. Appended at the file tail so no existing line moves.
//
// SYS_KDF: Argon2id (RFC 9106, v0x13) computed by the kernel through `holocron_core::cc::CryptoCore` (the
// ONE implementation: CRYPTOCORE's), because the ring key's floor (19 MiB) and default (64 MiB) do not fit
// the 4 MiB ring-3 ELF window. Request, little-endian:
//     [m_kib u32][t u32][p u32][pw_len u16][salt_len u16][password][salt]
// Output: exactly KDF_OUT_LEN bytes at `out`. `-EINVAL` parameters outside [floor, KDF_*_MAX] or lengths
// outside the caps, `-ENOMEM` the kernel heap could not hold the memory blocks, `-EFAULT` a bad buffer.
// x86_64 under the kernel feature `lumen`; elsewhere the unknown-syscall default.
// =================================================================================================

/// `SYS_KDF(req_ptr, req_len, out_ptr, out_len) -> 0 / -errno` — see the block above.
pub const SYS_KDF: u64 = 66; // after LUMENUX's SYS_CLIP_GET (65)
/// Request header bytes.
pub const KDF_HDR_LEN: usize = 16;
/// Password ceiling.
pub const KDF_PW_MAX: usize = 1024;
/// Salt ceiling.
pub const KDF_SALT_MAX: usize = 64;
/// Output bytes (the ring key).
pub const KDF_OUT_LEN: usize = 32;
/// Parameter floor (holocron_core `KdfParams::FLOOR`, the OWASP 2023 Argon2id floor): 19 MiB, t 2, p 1.
pub const KDF_M_KIB_MIN: u32 = 19 * 1024;
pub const KDF_T_MIN: u32 = 2;
/// Parameter ceilings (RFC 9106 §4's second recommended option is 64 MiB, t 3, p 4).
pub const KDF_M_KIB_MAX: u32 = 1 << 16;
pub const KDF_T_MAX: u32 = 10;
pub const KDF_P_MAX: u32 = 8;
/// HOLOCRON2: `SYS_PATH_READ` flag — read a DIRECTORY's entry names (`\n`-joined, a directory with a trailing
/// `/`) from `offset` of that listing; only inside the caller's own home. Without it a directory is `-EISDIR`.
pub const PATH_R_LIST: u16 = 1 << 3;
/// Holocron's bus verbs (holocron_core::wire, SecretGet..Status), BANDY3's registrable range.
pub const BUS_VERB_HOLOCRON_FIRST: u8 = 144;
pub const BUS_VERB_HOLOCRON_LAST: u8 = 151;
const _: () = assert!(SYS_KDF == SYS_CLIP_GET + 1);
const _: () = assert!(BUS_VERB_HOLOCRON_FIRST >= BUS_VERB_FULFIL_MIN && (BUS_VERB_HOLOCRON_LAST - BUS_VERB_HOLOCRON_FIRST) as usize + 1 == BUS_REG_MAX_PER_ROW);
const _: () = assert!(PATH_R_LIST & (PATH_W_TRUNC | PATH_W_MKDIRS | PATH_W_UNLINK) == 0);

/// Encode a `SYS_KDF` request into `out`; the length, or `None` when a length is over its cap.
pub fn kdf_request(m_kib: u32, t: u32, p: u32, password: &[u8], salt: &[u8], out: &mut [u8]) -> Option<usize> {
    if password.len() > KDF_PW_MAX || salt.len() > KDF_SALT_MAX {
        return None;
    }
    let n = KDF_HDR_LEN + password.len() + salt.len();
    let o = out.get_mut(..n)?;
    o[0..4].copy_from_slice(&m_kib.to_le_bytes());
    o[4..8].copy_from_slice(&t.to_le_bytes());
    o[8..12].copy_from_slice(&p.to_le_bytes());
    o[12..14].copy_from_slice(&(password.len() as u16).to_le_bytes());
    o[14..16].copy_from_slice(&(salt.len() as u16).to_le_bytes());
    o[16..16 + password.len()].copy_from_slice(password);
    o[16 + password.len()..n].copy_from_slice(salt);
    Some(n)
}

/// Decode a `SYS_KDF` request: `(m_kib, t, p, password, salt)`; `None` when malformed or out of range.
pub fn kdf_parse(b: &[u8]) -> Option<(u32, u32, u32, &[u8], &[u8])> {
    let h = b.get(..KDF_HDR_LEN)?;
    let u = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
    let (m, t, p) = (u(0), u(4), u(8));
    let pl = u16::from_le_bytes([h[12], h[13]]) as usize;
    let sl = u16::from_le_bytes([h[14], h[15]]) as usize;
    if b.len() != KDF_HDR_LEN + pl + sl || pl > KDF_PW_MAX || sl > KDF_SALT_MAX || sl < 8 {
        return None;
    }
    if !(KDF_M_KIB_MIN..=KDF_M_KIB_MAX).contains(&m) || !(KDF_T_MIN..=KDF_T_MAX).contains(&t) || !(1..=KDF_P_MAX).contains(&p) || m < 8 * p {
        return None;
    }
    Some((m, t, p, &b[KDF_HDR_LEN..KDF_HDR_LEN + pl], &b[KDF_HDR_LEN + pl..]))
}

#[cfg(test)]
mod holocron2_tests {
    extern crate std;
    use super::*;
    #[test]
    fn kdf_request_roundtrip_and_bounds() {
        let mut b = [0u8; 128];
        let n = kdf_request(1 << 16, 3, 4, b"pw", &[7u8; 16], &mut b).unwrap();
        assert_eq!(kdf_parse(&b[..n]), Some((1 << 16, 3, 4, &b"pw"[..], &[7u8; 16][..])));
        let n = kdf_request(1024, 3, 1, b"pw", &[7u8; 16], &mut b).unwrap();
        assert_eq!(kdf_parse(&b[..n]), None, "below the floor");
        let n = kdf_request(1 << 17, 3, 1, b"pw", &[7u8; 16], &mut b).unwrap();
        assert_eq!(kdf_parse(&b[..n]), None, "over the ceiling");
        std::println!(":: HOLOCRON2-ABI: kdf={} path_r_list={} verbs={}..={} -> PASS ::", SYS_KDF, PATH_R_LIST, BUS_VERB_HOLOCRON_FIRST, BUS_VERB_HOLOCRON_LAST);
    }
}
// =================================================================================================
// WINDOW2 (rmbp-ledger B361, R85) — the ring-3 window raised to 64 MiB (`USER_WINDOW_BYTES` above). The KDF
// known-answer pair `tests window` uses: HOLOCRON.ELF `--kdf-selftest` derives Argon2id over these inputs IN
// RING 3 at Holocron's metal parameters (48 MiB, t 3, p 4 — the memory the window holds beside image, heap
// and stack) and the kernel recomputes the same key through SYS_KDF's body; the exit status carries 23 bits.
// =================================================================================================

/// WINDOW2: Holocron's metal ring parameters — Argon2id memory in KiB (48 MiB).
pub const WINDOW2_KDF_M_KIB: u32 = 48 * 1024;
/// WINDOW2: passes.
pub const WINDOW2_KDF_T: u32 = 3;
/// WINDOW2: lanes.
pub const WINDOW2_KDF_P: u32 = 4;
/// WINDOW2: the KAT password (not a secret).
pub const WINDOW2_KAT_PW: &[u8] = b"window2-kat";
/// WINDOW2: the KAT salt.
pub const WINDOW2_KAT_SALT: [u8; 16] = *b"UnaOS-WINDOW2-16";
/// WINDOW2: the heap a ring-3 program must be able to touch (`tests window` `alloc48m`).
pub const WINDOW2_ALLOC_BYTES: u64 = 48 << 20;
const _: () = assert!((WINDOW2_KDF_M_KIB as u64) * 1024 + (8 << 20) <= USER_WINDOW_BYTES);
const _: () = assert!(WINDOW2_ALLOC_BYTES + (8 << 20) <= USER_WINDOW_BYTES);

#[cfg(test)]
mod window2_tests {
    extern crate std;
    use super::*;
    #[test]
    fn window2_constants() {
        assert_eq!(USER_WINDOW_BYTES, 67_108_864);
        assert_eq!(USER_XWIN_OFF / (2 << 20) + USER_WINDOW_BYTES / (2 << 20), 33); // PD/L2 entries 1..=32
        let mut b = [0u8; KDF_HDR_LEN + KDF_PW_MAX + KDF_SALT_MAX];
        let n = kdf_request(WINDOW2_KDF_M_KIB, WINDOW2_KDF_T, WINDOW2_KDF_P, WINDOW2_KAT_PW, &WINDOW2_KAT_SALT, &mut b).unwrap();
        assert!(kdf_parse(&b[..n]).is_some(), "the KAT parameters are inside SYS_KDF's bounds");
        std::println!(":: WINDOW2-ABI: bytes={} kdf_m_kib={} alloc={} -> PASS ::", USER_WINDOW_BYTES, WINDOW2_KDF_M_KIB, WINDOW2_ALLOC_BYTES);
    }
}
// APPRES (rmbp-ledger B398, MACPARITY §16 B4 / row 38) — a program's RESOURCES: a second UnaOS note, type
// APP_RES_NOTE_TYPE, in a NON-ALLOC SHT_NOTE section `.note.unaos.res` with no program header (the loader maps
// PT_LOAD only and never sees it). `tools/una-res` writes it after the strip; the kernel's registrar
// (`fs/appres.rs`) reads it through the section table and caches it as attributes. Desc layout and keys:
// `midden_core::RES_MAGIC` and the `RES_KEY_*` constants (the parse lives there). Appended at the tail.
/// The resource note's section name.
pub const APP_RES_SECTION: &str = ".note.unaos.res";
/// The resource note's type (owner "UnaOS", like APP_NOTE_TYPE's).
pub const APP_RES_NOTE_TYPE: u32 = 2;

// ==========================================================================================// DIALOG2 (rmbp-ledger B404) — a ring-3 program raises its OWN alert, sheet or toast, and hears the answer.
// Appended at the file tail so no existing line moves.
// =================================================================================================

/// Bus verb: a free-standing alert owned by the caller (app-modal to the caller's windows). Body: [`dialog_body`].
pub const BUS_VERB_DIALOG: u8 = 20;
/// Bus verb: the same alert as a SHEET on the caller's front window.
pub const BUS_VERB_SHEET: u8 = 21;
/// Bus verb: a one-line toast titled with the caller's (kernel-stamped) name; buttons are ignored, no answer.
pub const BUS_VERB_TOAST: u8 = 22;
const _: () = assert!(BUS_VERB_DIALOG > BUS_VERB_PREF_CHANGED && BUS_VERB_TOAST < BUS_VERB_REGISTER);
/// The answer to a dialog/sheet, delivered to the POSTER's input ring (never to the focused slot). Payload
/// `[15:8]` = the caller's token, `[7:0]` = the button index (left to right; the default is the last).
pub const INPUT_EV_DIALOG_ANSWER: u64 = 11; // 10 is INPUT_EV_CLOSE_REQ (APPMENU2); renumbered at merge17
/// The field separator inside a dialog body (ASCII unit separator).
pub const DIALOG_SEP: u8 = 0x1F;
/// Buttons per dialog (the default is the rightmost — the last).
pub const DIALOG_BTN_MAX: usize = 3;

/// A parsed dialog request: `token` comes back in the answer; `info` may hold up to three `\n` lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogReq<'a> {
    pub token: u8,
    pub message: &'a [u8],
    pub info: &'a [u8],
    pub buttons: [&'a [u8]; DIALOG_BTN_MAX],
    pub nb: u8,
}

/// Encode: `[token][nb] message SEP info (SEP button){nb}`. `None` when a field holds the separator, there are
/// more than three buttons, or `out` is too small.
pub fn dialog_body(token: u8, message: &[u8], info: &[u8], buttons: &[&[u8]], out: &mut [u8]) -> Option<usize> {
    if buttons.len() > DIALOG_BTN_MAX || message.contains(&DIALOG_SEP) || info.contains(&DIALOG_SEP) || buttons.iter().any(|b| b.contains(&DIALOG_SEP)) {
        return None;
    }
    let need = 2 + message.len() + 1 + info.len() + buttons.iter().map(|b| b.len() + 1).sum::<usize>();
    if out.len() < need {
        return None;
    }
    out[0] = token;
    out[1] = buttons.len() as u8;
    let mut n = 2;
    let mut put = |s: &[u8], n: &mut usize| {
        out[*n..*n + s.len()].copy_from_slice(s);
        *n += s.len();
    };
    put(message, &mut n);
    put(&[DIALOG_SEP], &mut n);
    put(info, &mut n);
    for b in buttons {
        put(&[DIALOG_SEP], &mut n);
        put(b, &mut n);
    }
    Some(n)
}

/// Decode a [`dialog_body`] frame; refused whole (`None`) when the field count does not match `nb`.
pub fn dialog_parse(body: &[u8]) -> Option<DialogReq<'_>> {
    if body.len() < 2 || body[1] as usize > DIALOG_BTN_MAX {
        return None;
    }
    let (token, nb) = (body[0], body[1]);
    let mut it = body[2..].split(|&b| b == DIALOG_SEP);
    let message = it.next()?;
    let info = it.next()?;
    let mut buttons: [&[u8]; DIALOG_BTN_MAX] = [&[]; DIALOG_BTN_MAX];
    for slot in buttons.iter_mut().take(nb as usize) {
        *slot = it.next()?;
    }
    if it.next().is_some() {
        return None;
    }
    Some(DialogReq { token, message, info, buttons, nb })
}

/// The packed answer event a poster reads from its input ring.
pub const fn dialog_answer_pack(token: u8, button: u8) -> u64 {
    input_ev_pack(INPUT_EV_DIALOG_ANSWER, ((token as u64) << 8) | button as u64)
}

#[cfg(test)]
mod dialog2_tests {
    use super::*;
    #[test]
    fn dialog_body_round_trip() {
        let mut b = [0u8; 128];
        let n = dialog_body(7, b"Save changes?", b"Your changes will be lost.", &[b"Don't Save", b"Cancel", b"Save"], &mut b).unwrap();
        let r = dialog_parse(&b[..n]).unwrap();
        assert_eq!((r.token, r.nb, r.message, r.info), (7, 3, &b"Save changes?"[..], &b"Your changes will be lost."[..]));
        assert_eq!(r.buttons[2], b"Save");
        let n = dialog_body(1, b"hello", b"", &[], &mut b).unwrap();
        assert_eq!(dialog_parse(&b[..n]).unwrap().nb, 0);
        assert!(dialog_body(1, b"a\x1fb", b"", &[], &mut b).is_none());
        assert!(dialog_parse(&[1, 2, b'm', DIALOG_SEP, b'i', DIALOG_SEP, b'x']).is_none(), "two buttons declared, one sent");
        assert_eq!(dialog_answer_pack(7, 2) >> INPUT_EV_TYPE_SHIFT, INPUT_EV_DIALOG_ANSWER);
        assert_eq!(dialog_answer_pack(7, 2) & 0xFFFF, 0x0702);
    }
}

// =================================================================================================
// SETTINGSFILES (rmbp-ledger B407, R98): a program declares its own settings stanza (`app.<name>.*`,
// stored in `<home>/settings/<name>`). Body: `<name>` NUL then `<key>\t<spec>\t<default literal>\t<doc>`
// lines (prefs_core::declare). Kernel-fulfilled beside PREF_GET/SET/LIST; -EACCES outside the session,
// -EINVAL malformed. Appended at the file tail.
// =================================================================================================

/// Bus verb: declare a program's settings stanza.
pub const BUS_VERB_PREF_DECLARE: u8 = 23; // 20..=22 are DIALOG2's dialog/sheet/toast; renumbered at merge17
const _: () = assert!(BUS_VERB_PREF_DECLARE > BUS_VERB_TOAST && BUS_VERB_PREF_DECLARE < BUS_VERB_REGISTER);
=======
// SMALLFIX3 (rmbp-ledger B416) — THE CODES ARE UNIQUE BY CONSTRUCTION. Three arcs of one wave each took the
// next free number from their own branch (event 10 twice, bus verb 20 twice): a clash only showed at the fold.
// Every input-ring event type and every kernel bus verb is listed here and a duplicate fails the BUILD on both
// arches and the host. An arc minting a code appends its constant to the list (tail-neutral); the kernel's
// `tests smallfix3` prints `event_codes=unique bus_verbs=unique` from the same lists.
/// Every input-ring event type (`INPUT_EV_*` the ring carries).
pub const INPUT_EV_ALL: &[u64] = &[
    INPUT_EV_KEY_DOWN, INPUT_EV_KEY_UP, INPUT_EV_MOUSE_REL, INPUT_EV_MOUSE_ABS, INPUT_EV_BUTTON, INPUT_EV_WHEEL,
    INPUT_EV_ACTION, INPUT_EV_MENU_PICK, INPUT_EV_WIN_RESIZE, INPUT_EV_CLOSE_REQ,
];
/// Every kernel bus verb tag (the registrable range `>= BUS_VERB_FULFIL_MIN` is the bus's, not listed). The
/// HOLOCRON band is listed by its two ends; [`codes_unique_u8`] also refuses any other tag inside the band.
pub const BUS_VERB_ALL: &[u8] = &[
    BUS_VERB_LS, BUS_VERB_CAT, BUS_VERB_CP, BUS_VERB_WRITE, BUS_VERB_RM, BUS_VERB_MV, BUS_VERB_MENU_PUBLISH,
    BUS_VERB_MENU_CLEAR, BUS_VERB_MENU_GET, BUS_VERB_NOTICE, BUS_VERB_ATTR_SET, BUS_VERB_ATTR_GET, BUS_VERB_ATTR_LIST,
    BUS_VERB_ATTR_QUERY, BUS_VERB_ATTR_STAT, BUS_VERB_PREF_GET, BUS_VERB_PREF_SET, BUS_VERB_PREF_LIST,
    BUS_VERB_PREF_CHANGED, BUS_VERB_REGISTER, BUS_VERB_HOLOCRON_FIRST, BUS_VERB_HOLOCRON_LAST,
];
/// No two entries equal (and none zero). `const` so the assertion below runs in the compiler.
pub const fn codes_unique_u64(v: &[u64]) -> bool {
    let mut i = 0;
    while i < v.len() {
        if v[i] == 0 {
            return false;
        }
        let mut j = i + 1;
        while j < v.len() {
            if v[i] == v[j] {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}
/// The bus-verb form: unique, non-zero, below the registrable range, and nothing but the band's own two ends
/// inside the HOLOCRON band.
pub const fn codes_unique_u8(v: &[u8]) -> bool {
    let mut i = 0;
    while i < v.len() {
        let x = v[i];
        let in_band = x >= BUS_VERB_HOLOCRON_FIRST && x <= BUS_VERB_HOLOCRON_LAST;
        let band_end = x == BUS_VERB_HOLOCRON_FIRST || x == BUS_VERB_HOLOCRON_LAST;
        if x == 0 || (x >= BUS_VERB_FULFIL_MIN && !in_band) || (in_band && !band_end) {
            return false;
        }
        let mut j = i + 1;
        while j < v.len() {
            if x == v[j] {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}
const _: () = assert!(codes_unique_u64(INPUT_EV_ALL), "SMALLFIX3: two INPUT_EV_* event types share a code");
const _: () = assert!(codes_unique_u8(BUS_VERB_ALL), "SMALLFIX3: two BUS_VERB_* verbs share a tag");
