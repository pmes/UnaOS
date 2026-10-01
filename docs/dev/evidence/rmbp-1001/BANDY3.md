# BANDY3 — fulfiller registration on the wire (ledger B301; audit row B289)

Branch `exec-rmbp-bandy3`, cut from `55352efc` (exec-rmbp-merge8). Seam: **fulfiller**.

## M1 — the design pass (written before any code)

**Finding (audit §1 root cause, B289; ROADMAP §3b).** The only ring-3 transport on the metal is the
BANDY v1 wire (`crate::bus`: 52-byte header, kernel-stamped principal, 4 KiB body ceiling, the KATs are
the spec of record), carried by `SYS_MSEND`/`SYS_MRECV` on both arches since BUSX86. Every verb on it
(ls/cat/cp/write/rm/mv, NOTICE, MENU_*) is fulfilled IN-KERNEL, and "fulfiller registration" was
deferred to "BANDY-3's design pass" (ROADMAP §3b). So no ring-3 program can own a verb, and every
desktop feature of three waves was built inside the kernel beside the live host handler. §3b principle 3:
a command is addressable the same way whether it is fulfilled in-kernel, by a handler or by a spawned
vessel. Principle 2: every message carries the caller's principal, stamped by the kernel.

**The seam.** The kernel becomes a ROUTER for a reserved verb range: a ring-3 program registers the
verb tags it fulfils, a caller's frame for such a verb is relayed to the fulfiller's mailbox re-stamped
with the caller's principal, and the fulfiller's reply is relayed back as a KERNEL-stamped reply. The
caller cannot tell an in-kernel verb from a registered one, which is principle 3. One shared,
arch-neutral module carries the table and the relay: `crates/kernel/src/bus_route.rs`
(`//! CHARTER: Kernel — fulfiller`). Each arch's syscall file gains only thin calls: three same-line hooks
and one ops table at the file tail.

### The registration frame

* `BUS_VERB_REGISTER = 127`, a kernel-owned tag at the top of the kernel range, kept away from the
  "next free tag" counter (11) the in-kernel verbs mint from. REQUEST body = 1..=8 verb tags, one byte
  each, no duplicates. Reply: an empty status-0 frame, or an errno with an empty body.
* **The verb space splits at 128.** Tags `1..=127` are the kernel's: ls/cat/cp/write/rm/mv, NOTICE,
  MENU_* and whatever the kernel mints next. Asking to register any of them is refused `-EEXIST`
  ("a fulfiller already exists: the kernel"). Kernel fulfilment always wins, so ls/cat/cp/write/rm/mv
  behave exactly as they do today. Tags `128..=255` (`BUS_VERB_FULFIL_MIN`) are REGISTRABLE.
* The kernel records `(verb -> registering row, that row's generation)`. The row is the ASID on aarch64
  and the HANDLES row on x86, the same key each arch's mailbox already uses. The principal needs no
  separate entry: it is the row's own kernel-held identity, re-derived at relay time and never cached
  where it could go stale. The table is all-or-nothing per frame. If any tag is refused, none is taken.
* Errnos: `-EINVAL` (empty, over 8 tags, or a duplicate in the body) · `-EEXIST` (a kernel tag, or a
  tag a different live row already owns; re-registering your own tag is idempotent) · `-ENOSPC`
  (the row would hold more than 8, or the 32-entry table is full).
* v1 has no explicit unregister verb. A fulfiller unregisters by exiting.

### The relay

* Caller A sends `REQUEST(verb v >= 128, corr c, principal ZERO, body b)`. Validation is unchanged:
  the frame is parsed fail-closed and a caller-supplied principal is `-EINVAL`. Then:
  * no live registration for v: A receives an ordinary KERNEL reply with status **`-ENOENT`**, never a
    hang;
  * the fulfiller's mailbox is full, or the pending table is full (32 total, at most 8 per caller):
    A receives **`-EAGAIN`**. A relay never blocks the kernel and never parks A;
  * otherwise the kernel builds a NEW frame `REQUEST(v, corr = r, principal = A's kernel stamp, b)` and
    pushes it into the fulfiller's mailbox. Here `r` is a kernel-minted 32-bit relay id, nonzero and
    monotonic. The kernel records `(r -> A's row, A's generation, c, v, the fulfiller's row and
    generation)`. `SYS_MSEND` returns 0 to A, and A's answer arrives later in A's mailbox.
* **The correlation id rides the header's `corr` field, not the body.** The 52-byte header is not
  changed. The kernel builds every relayed frame itself, so it writes its own relay id into the `corr`
  of the frame it hands the fulfiller and restores A's `c` on the way back. The body was ruled out on
  the merits: the frozen error-reply rule (an error reply is its errno, NEVER errno plus bytes) means a
  fulfiller's error reply has no body to carry an id in.
* The fulfiller F answers with `SYS_MSEND(REPLY(verb v, corr r, status s, principal ZERO, body))`.
  Under `busreg`, a REPLY-kind frame is accepted from ring 3 only as a relay answer. Its principal must
  be zero, because the kernel stamps, and `frame_parse` already enforces that an error reply carries no
  body. The kernel looks up `r` under F's own `(row, generation)` and the echoed verb. A miss is
  `-ENOENT`, so F cannot answer another fulfiller's request or forge a reply to an arbitrary caller.
  If A's mailbox is full, F gets `-EAGAIN` and the pending entry stays for a retry. Otherwise the
  kernel builds `REPLY(v, corr = c, status s, principal = the RESERVED KERNEL record, body)` into A's
  mailbox, generation-fenced to A's tenancy as every reply is today. A sees exactly the frame shape an
  in-kernel verb gives it.
* **Exit.** On row teardown (aarch64 `bus_mbox_clear(asid)` in the row clear, x86 `clear_handle_row`)
  the kernel drops every registration the row held. Every caller still waiting on that row as a
  fulfiller gets a KERNEL reply with **`-ECONNRESET`**, counted as an orphan. Pending entries whose
  CALLER died are left alone: the generation fence discards a late reply, and the entry is reclaimed
  when the fulfiller answers, exits, or needs the slot.

### Bounds

The existing per-row mailbox (depth 16) is reused: a relayed request occupies one slot in the
fulfiller's mailbox exactly as a reply does. Other bounds: at most N = 8 registrations per row and 32 in
total; at most 32 pending relays, 8 per caller. Every refusal is an errno and nothing blocks. The
kernel's staging is one heap box of exactly the frame's length, no larger than the 4148-byte
`BUS_FRAME_MAX` ceiling.

### The principal rule

The fulfiller reads A's principal in the relayed header. On aarch64 that is A's kernel-held
`PrincipalRecord` wire image (32 bytes: kind, len, value). x86 has no `PrincipalRecord`: its identity is
`(row, SLOT_GEN)` (BUSX86 note 1), so the kernel writes kind 3 (`PRIN_KERNEL_PID`, the reserved
launcher-minted kind) with the value `row:<r>/gen:<g>`. The bytes are the kernel's stamp, never A's
claim, since A's request had to present all-zero bytes. **The fulfiller cannot act AS A.** Nothing in
the relay lends it A's grants. Any object it touches, it touches through ordinary syscalls under ITS OWN
grants. A fulfiller is a service. Authorisation stays per object, in the kernel. A fulfiller that wants
to refuse A may consult A's principal, but whatever it reads or writes is checked against the
fulfiller's own principal, not A's. Delegation (a fulfiller acting with the caller's rights) does not
exist in v1 and would be its own ruled arc.

### AI-OFF / knob-off guarantee

Nothing registers by default. With no registrations, the only change in the request path is the
`127`/`>= 128` tags. They were `-EINVAL` before (`frame_parse` refused the verb). Now a caller gets an
ordinary reply: `-ENOENT` for an unowned tag, or the REGISTER verdict. Every other verb takes the old
path untouched. The whole module and every hook sit behind **`UNAOS_BUSREG=1` / feature `busreg`**.
With the knob off, `verb_valid` refuses the new tags exactly as before and the tree is byte-behaviour
unchanged.

### Milestones

* **M1** — this design, committed before code.
* **M2** — kernel: `una_abi` constants (`BUS_VERB_REGISTER`, `BUS_VERB_FULFIL_MIN`,
  `BUS_VERB_PREF_GET`/`BUS_VERB_PREF_LIST`, `BUS_REG_MAX_PER_ROW`, `EEXIST`, `ENOSPC`, `ECONNRESET`);
  `bus.rs` `verb_valid` admits the new tags under `busreg`, edited on its existing line because the file
  is line-neutral; `bus_route.rs` holds the table, relay, reply, exit and codec KATs (golden REGISTER
  request, golden relayed request carrying a stamped principal, golden fulfiller reply); hooks in both
  dispatchers; unregister on exit; knob plumbing (Cargo feature, arroyo env arm, `K8_FEATS` arm,
  builder).
* **M3** — the first fulfiller, `crates/user-prefs` → `PREFS.BIN`. A static ring-3 program built by the
  same arroyo builder steps as `user-pulse` and staged in `APPS/` beside PULSE.ELF on the ESP and the
  DATA volume. It registers `PrefGet`/`PrefList` (read-only in v1) and answers from a fixed small table.
  The real TOML parse through `prefs_core` is OWED: PREFS, a parallel arc, owns that crate.
* **Witness** — `tests bandy3` (registered in `tests.rs`) drives the PRODUCTION `sys_msend_for` /
  `busx_msend_for` with two scratch identities and prints one line:
  `:: BANDY3: registered=<n> relayed=<n> replied=<n> orphan=<n> -> PASS ::`. The legs: codec goldens;
  PrefGet with no fulfiller is `-ENOENT`; registering LS is `-EEXIST`; register GET+LIST is 0; a caller's
  PrefGet reaches the fulfiller's mailbox carrying the CALLER's principal and the relay corr; the
  fulfiller's reply reaches the caller KERNEL-stamped with the caller's own corr; a forged reply
  (wrong row or unknown relay id) is `-ENOENT`; the fulfiller's exit with a request pending gives the
  caller `-ECONNRESET` and the verb is unowned again (`-ENOENT`). PREFS.BIN prints its own ring-3 line
  when launched (`bg /apps/PREFS.BIN`):
  `:: PREFS: probe=-2 register=0 kernel_tag=-17 self_get=0 stamp=kernel -> PASS ::`, then serves.

### What stays owed

* MIDDEN.BIN (aarch64) or a second ring-3 fixture asking PrefGet across two live programs. The
  kernel fixture proves the cross-principal relay through the production path with scratch identities.
  PREFS.BIN proves the ring-3 round-trip against itself. The two-program ring-3 ladder leg is owed.
* `prefs_core` (the PREFS arc): the real TOML store and PrefSet/PrefChanged. In v1 PREFS.BIN answers a
  fixed table.
* An explicit UNREGISTER verb, delegation, and a fulfiller-side MRECV that blocks per verb rather than
  per mailbox.
* The metal boot (R78).

## Results (M2, M3) — compile legs only (R78: no QEMU)

**Numbers taken.** `BUS_VERB_REGISTER` = 127. `BUS_VERB_FULFIL_MIN` = 128. `BUS_VERB_PREF_GET` = 128 and
`BUS_VERB_PREF_LIST` = 129, the names the PREFS arc is told to use; one definition survives at the fold.
`BUS_REG_MAX_PER_ROW` = 8. New errnos in `una_abi`: `EEXIST` = -17, `ENOSPC` = -28, `ECONNRESET` = -104.
No new syscall: the transport is `SYS_MSEND`/`SYS_MRECV` (19/20).

**Files.** `crates/kernel/src/bus_route.rs` (new; `//! CHARTER: Kernel — fulfiller`). In `bus.rs`,
`verb_valid` changed on its own line and the file stays line-neutral. Each of `arch/{aarch64,x86_64}/syscall.rs`
gets three same-line hooks (the REPLY hook at the kind check, the route hook after `text`, the exit
hook beside the mailbox drain) and an ops table and fixture hooks at the file tail. `lib.rs` gets a
same-line `pub mod bus_route`. `tests.rs` gets the same-line `bandy3` registration. `una_abi` constants
and a host test are appended at the file tail. `crates/user-prefs` is new. In `arroyo`: the
`build_user_prefs_x86` step, a check-matrix row, the `_feats` arm and the `K8_FEATS` arm. In
`builder/src/main.rs`: the knob and the staging of `APPS/PREFS.BIN` on the ESP and the DATA volume.

**Witness lines a metal boot should print.**
`tests bandy3` →
`:: BANDY3: registered=2 relayed=2 replied=1 orphan=1 -> PASS ::` on a boot where it is the first
registrant (the counters are monotonic since boot, so a PREFS.BIN launched first adds its own).
`bg /apps/PREFS.BIN` →
`:: PREFS: probe=-2 register=0 kernel_tag=-17 self_get=0 stamp=kernel -> PASS ::`, then one
`:: PREFS: served verb=<v> caller_kind=<k> caller=<principal> status=<s> ::` per request served.

**Owed.** The two-program ring-3 leg (MIDDEN.BIN or a second fixture asking PREFS.BIN), `prefs_core`
(the real TOML read), PrefSet/PrefChanged, an aarch64 media build step for PREFS (only x86 is built
and staged), and the metal boot. One residual is stated rather than fixed: if a caller's mailbox is
full when its fulfiller exits, that caller's `-ECONNRESET` cannot be queued and is not counted. The
caller still has 16 frames to drain, so it does not hang, but it never receives that corr.
