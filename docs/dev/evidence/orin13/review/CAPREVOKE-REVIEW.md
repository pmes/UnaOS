# CAPREVOKE adversarial review — commit 06858185 (hw-jetson)

Subject: `06858185 aarch64/syscall: CAPREVOKE — a File-handle revoke frees its descriptor, as the x86 twin already does`
File: `unaos/crates/kernel/src/arch/aarch64/syscall.rs` (+34/-5, net +29 — matches `git show --stat`).
Reviewer: independent adversarial agent, read-only tree at hw-jetson tip 06858185. No build, no QEMU.
Line numbers below are the post-commit file (hw-jetson tip). x86 twin: `unaos/crates/kernel/src/arch/x86_64/syscall.rs`.

## Verdict

**No blocking finding.** The port is shape-faithful to x86, the lock/context claims hold, the fixture
exercises the same value a real `sys_open` stores, and no board-specific helper diverges in the new
branch. Everything below that is a real hazard is inherited from x86 or from a pre-existing aarch64
condition; the commit adds one more participant to those, it does not create them.

## Findings, ranked

### F1 — LOW (inherited x86 semantics, but a NEW reachable state on aarch64): the surviving GRANT-duplicate becomes an uncloseable handle

Evidence:
- `sys_cap_grant` mints a second handle carrying the SAME value word (`handle_get` at 8866 → `handle_set(asid, idx, target)` at 8914), so root R and dup G share one descriptor and one `OPEN_FILES` refcount (one `openfile_incref` per `sys_open`, 9108/9126).
- New branch 8949–8955: revoking either R or G runs `files_free` on the shared descriptor. A dup minted with `CAP_WRITE` only (no `CAP_REVOKE`) still frees it — the descriptor free is gated on `KIND_FILE`, not on the right.
- The survivor then cannot be closed: `sys_close` 9400–9402 returns `-EBADF` from `file_desc_validate == None` BEFORE `handle_clear` (9405), so the handle slot stays occupied until a `sys_cap_revoke` of it (validate → None → skipped, `handle_clear` at 8967 still runs) or teardown (`clear_handle_row`).

Failure scenario: EL0 `open F → h; cap_grant(h, CAP_READ) → g; cap_revoke(g)` (g lacks CAP_REVOKE, so this is the "local drop" path). h's next read: `-EACCES`; `close(h)`: `-EBADF`; h occupies one of `NHANDLE = 8` slots until exit. A program that does not know to `cap_revoke(h)` leaks a handle slot per iteration and hits `-EAGAIN` on handle install — bounded per process, reclaimed at teardown, fail-closed for data (no access to the freed descriptor: every consumer re-validates through `file_desc_validate`, 7324/9180/9242/9281/9400/9434).

Status: x86 has exactly this ("Honest scope", 14714–14716) and the aarch64 comment reproduces it (8945–8947). Faithful port; before this commit the aarch64 state was the worse one (descriptor leak → permanent `-EMFILE`). Not a blocker; worth a ledger line that `sys_close` of a validate-failing File handle leaves the handle slot in place on both arches.

### F2 — LOW (pre-existing, not introduced here): the "single-writer per ASID" premise the branch inherits is already broken by ELF-2 threads

Evidence:
- `sys_cap` doc 8841–8842: "Runs single-writer over the caller's table (one SVC at a time, one live task per ASID), so no lock is needed." The new branch relies on this (free → mark → clear is not atomic).
- `sys_thread_spawn` (11018+; doc lines "the caller's shared address space, which a spawned thread runs under", `asid = ttbr0 >> 48`) puts several tasks under ONE ASID. On the Orin (`tegrasmp`) they can run truly concurrently.
- `files_free` reads `FILE_OPENROW` (4821) before clearing it and decrements that row (4832 → 5014–5029). Two concurrent frees of the same descriptor from two threads (close+close, close+unlink, and now close+revoke / revoke+revoke of R/G dups) both read the same row and decrement twice; `openfile_decref_at` guards only `refcount == 0` (5016), not a double decrement from 2 → 0.

Failure scenario (threads on SMP only): process P (2 threads) and process Q both hold F open, refcount 2, F unlinked by Q (`unlink_pending`). P's thread A `sys_close(h)` and thread B `sys_cap_revoke(g)` (g a GRANT dup of h) interleave between 4821 and 4827: two decrements → refcount 0 → chain returned → `free_orphan_chain` frees F's clusters under Q's live descriptor.

Status: identical race already exists between `sys_close`/`sys_close` and `sys_close`/`sys_unlink` on this file; the commit adds a third participant of the same shape. Not a blocker for this arc; it belongs to whichever arc owns the threads-vs-handle-table concurrency question (the U6b comment at 4677–4680 still asserts single-writer). Flagging so it is not silently inherited by the next twin port.

### F3 — LOW (process, not code): the line-neutrality baseline moves, and the ack is a commit-message claim

Evidence: in-file rule at 13211 ("a line ADDED to this file renumbers every panic `Location` under it and breaks the knob-off `kernel8.img` byte-identity proof (PI-DESK's rule, and this file is named in it)"). The diff adds 29 net lines at 8930–8971 and 20708–20736, both above/among panic-bearing code. The commit message declares the baseline moves and that "pi 6 agreed to re-base over ccd". I cannot verify the ccd ack from the tree; the landing seat should have it recorded in both sessions before the pi battery next compares a knob-off image. `grep -rn 'byte-ident\|kernel8.img' docs/dev/LAWS.md` returns nothing — the rule lives only in-file and in pi's baseline record, so there is no doc to update for it here.

### F4 — INFO: the `HANDLE_RESERVING` filter is dead-but-harmless (copied from x86)

8950 filters a `KIND_FILE` handle whose value is `HANDLE_RESERVING`. That state exists only inside `sys_open` between `handle_install(asid, HANDLE_RESERVING)` (9135) / `handle_set_kind(.., KIND_FILE)` (9148) and `handle_set(.., file_id)` (9150), in the same task — unobservable by a revoke (and by F2's threads it would be observable, in which case the filter is the correct fail-safe). Identical to x86 14722. No action.

## Claims verified sound

1. **Branch shape = x86 modulo `row: usize` → `asid: u64`.** aarch64 8948–8955 vs x86 14717–14727: `handle_kind == KIND_FILE` → `handle_get(..).filter(!= HANDLE_RESERVING)` → `file_desc_validate` → `files_free`. Ordering after it — `HANDLE_RIGHTS`/`HANDLE_DERIV` loads, `deriv_revoke`, `handle_clear` — is line-for-line x86 14734–14739 vs aarch64 8962–8967. `file_desc_validate` (4755–4768) masks the low half, `checked_sub(1)`, bounds-checks, requires `FILE_USED`, matches `FILE_GEN` — no bare subtract.

2. **Lock discipline.** `sys_cap` holds no lock (8843–8851; `current_asid()` is a register read, 4583–4588). Callees in the new branch: `handle_kind` (4409) and `handle_get` (4627) are atomic loads; `file_desc_validate` (4755) is atomic loads; `files_free` (4818–4833) is atomic stores plus ONE bounded `OPEN_FILES.lock()` inside `openfile_decref_at` (5014) that is dropped before it returns (5027–5029; doc 5006–5007 "Only the short table mutation runs under the lock"). `handle_clear` (4642–4664) → `xfer_rec_free` (9694–9700) and `deriv_drop` (10176–10234): no `lock()` in either body. So at 8970–8972 `free_orphan_chain(fc)` runs with **no lock held** — the same state as `sys_close` 9404–9410 (`files_free` → `handle_clear` → `free_orphan_chain`). `deferred_free_push`'s "NEVER under OPEN_FILES" rule (5250) is honoured because that guard died inside `openfile_decref_at`. No deadlock, no missing lock: the row is the caller's own and every other writer to `OPEN_FILES` (`openfile_incref`, `_mark_unlink_pending_at`, `_by_dir`) takes the same single lock without nesting.

3. **Context of `free_orphan_chain`.** `SYS_CAP` (6897) and `SYS_CLOSE` (6902) are dispatched from the same `aarch64_svc_handler` (6841), so the IRQ/DAIF state and stack are identical to `sys_close`'s; `free_orphan_chain`'s own contract (5073–5076: "ONLY from SYSCALL context ... NEVER from teardown") is met. `asid` is always `current_asid()` from the dispatcher (8844–8847); the only other caller is the fixture (20725, `A = 6`), whose descriptor is a scaffold (`files_alloc` direct → `FILE_OPENROW = OPENROW_NONE`, 4792–4795 → `openfile_decref_at` returns `None` at 5011–5013), so the fixture can never reach `free_orphan_chain` from the launcher task. There is no API by which a parent revokes on a child's ASID.

4. **Ordering window (descriptor freed before `deriv_revoke`/`handle_clear`).** No cross-task observer exists: `File` is NOT a transferable kind (`sys_xfer_from` 9797–9798 admits only `KIND_CONSOLE`/`KIND_SOCKET`; x86 13277–13278 identical), `sys_cap_grant` mints only into the caller's own row (8860, 8909–8914), and a row has one live task (modulo F2). Within the window the descriptor is `FILE_USED = false`, so any resolve-then-validate consumer fails closed. Same window as x86; nothing newly introduced.

5. **`file_desc_validate` → `None` path.** Descriptor untouched, derivation still marked (if `CAP_REVOKE`), handle still cleared (8967), returns 0 — x86 14723–14740 identical. Revoke after `sys_unlink`'s `files_free_by_dir` (which already freed + bumped gen) validates `None` → no double free / no double decref. Double revoke → `ECHILD` at 8937–8939. Close after revoke → `-EBADF` (9395).

6. **Fixture realism.** `sys_open` stores `file_id_pack(FILE_GEN[asid][fid].load(Acquire), fid)` (9134) into the handle (9150); the fixture's 20711 is the same expression against row 6. x86's fixture uses the same shape (20951). The pre-commit `(fid + 1)` was the gen-0 encoding (4739–4740) — correct only while `FILE_GEN[6][fid] == 0`, i.e. dependent on which row-6 fixture (U11's at 21300/21736 also use row 6 and bump gens) ran first; the new form is correct regardless of order. `files_row_is_clear(A)` (4905–4908) now can only pass if the revoke freed the slot, since the trailing `files_free` is gone. Not a synthetic value.

7. **Board dependence.** No `#[cfg]` anywhere in 4690–5300 (the descriptor/refcount/orphan helpers), nor in the new branch. `syscall.rs` is gated as a whole on `aarch64_el0` (`arch/aarch64/mod.rs` 46–47), which covers both `baremetal` (Pi) and `tegra_el0` (Orin), so both boards compile the same bytes for this function. The only board-conditional code downstream is inside `fs/fat.rs` (`BlockSource::TegraSd`, 611/690–691 — read-only veto) and is reached identically by `sys_close`'s orphan free; the port adds no new stub asymmetry.

8. **Naming / GATE-FAMILY.** No new symbol. The tag `CAPREVOKE` appears only in comments; locals `orphan`/`fc` mirror `sys_close`'s. Nothing board-named.

9. **hw-pi4 claim.** `git show origin/hw-pi4:<file>` (origin/hw-pi4 = 24937b32, fetched this review): `sys_cap_revoke` body contains 0 `files_free` calls. No pi fix exists to copy — confirmed.

10. **Diff arithmetic.** `+34/-5` per `git show --stat`; net +29 as stated.

## Recommendation

Land as-is. Carry F1 and F2 into the ledger (`docs/SECURITY.md` hardening ledger or the U11 doc, whichever the arc's brief names) as inherited items, not as this commit's debt: F1 is a `sys_close` gap (EBADF-without-clear) on both arches; F2 is the threads-vs-single-writer premise that predates this commit. Confirm pi 6's re-base ack is recorded before the next knob-off `kernel8.img` byte-identity comparison (F3).
