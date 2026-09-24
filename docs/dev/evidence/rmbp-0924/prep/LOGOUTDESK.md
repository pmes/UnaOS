# LOGOUTDESK — prep

## The finding

Flight 15: `[users] logout epoch=3 ended=0 windows=0`. Peter: "we need to work on completely
closing down the desktop when logging out" (R69, `docs/dev/RULINGS.md` — grep `| R69 |`).

R69: on Log Out the desktop closes down COMPLETELY — not only the session's programs (SECLOGIN
M3, `session_logout_as`) but every window on the glass: console, shell, Quarry, the dock's strip,
any kernel window — so the login screen stands over an empty desktop and a fresh session gets a
fresh desktop. This supersedes `docs/dev/OS/04_SECURITY_IMMUNITY/multiuser.md` §8.1.

Sentence to rewrite (quoted verbatim, NOT edited this round):
`docs/dev/OS/04_SECURITY_IMMUNITY/multiuser.md:334-335`:
> What is NOT "over nothing": kernel windows (the console, the shell, Quarry) are not programs and stay
> on the desktop beneath the screen; the screen takes every key and press regardless.

## Mechanism

- `unaos/crates/kernel/src/fs/users.rs:956-975` `logout()` -> `arch_session_logout(root)` ->
  `(ended, windows)`; prints `[users] logout epoch=… ended=… windows=…`. SECLOGIN M3: ends the
  session's own PROGRAMS only.
- `unaos/crates/kernel/src/arch/x86_64/syscall.rs:23851` `session_logout_as` and
  `:28665-28690` `session_end_processes` — walks `PROCS`, calls `wm::close_owner(owner)` per
  running proc stamped in the closing epoch; `windows` is the sum of its per-proc counts.
- `unaos/crates/kernel/src/video/wm.rs:2724-2760+` `close_owner(owner_asid)` — **by design refuses
  kernel-owned rows** (CLOSEISO doc comment: "KERNEL FURNITURE IS NEVER REAPED BY AN OWNER-SCOPED
  CLOSE"; `is_kernel_owner(r.owner_asid)` rows go to `refused[]`, never closed). This is exactly
  why flight 15 measured `windows=0`: kernel windows were never in `close_owner`'s scope — immune
  by construction.
- `unaos/crates/kernel/src/video/wm.rs:1455-1479` — `KERNEL_OWNER_BASE = 0xFFFF_FF00`;
  `KERNEL_OWNER_CONSOLE = BASE+1`; `KERNEL_OWNER_DESKTOP = BASE+2` (shell, `dock.rs:584`); dock
  furniture `BASE+0x50` (`dock.rs:1252`); Quarry has its own `OWNER` const in `video/quarry.rs`
  (not located this round). `is_kernel_owner` (`wm.rs:1478`) is `asid` in `[BASE, BASE+0xFF]` — one
  band covers every furniture row.
- `unaos/crates/kernel/src/video/wm.rs:2641-2680` `close(id)` — the ID-SCOPED primitive with NO
  kernel-owner refusal (already used for a furniture row's own close disc, NORMALWIN comment,
  same block). M1 reuses this: collect every used row's id, `close(id)` each.
- `unaos/crates/kernel/src/video/wm.rs:345,351,84` — `rows: [Window; MAX_WINDOWS]`, `MAX_WINDOWS =
  12`; small fixed table, linear sweep is the existing pattern (`wm.rs:382-384`).
- `unaos/crates/kernel/src/video/login.rs:637-644` `reopen_after_logout()` — the one call site
  after `users::logout()` and before `open()` re-shows the screen; the single logout exit for root
  and user sessions (`x86-login.spec:403`). Sweep and witness belong here.
- Re-ignition: `unaos/crates/kernel/src/video/desktop_uefi.rs:674` `desktop_app_service()` mints
  the desktop at boot (`main.rs:5999`), gated by `DESKTOP_IGNITED` (`fs/users.rs:2033`) and by
  whether a `KERNEL_OWNER_DESKTOP` row already exists (`dock.rs:1312`). Risk M2 checks: the next
  login's pass must re-mint from scratch once M1 closes those rows.

## Plan

- **M1 — sweep every remaining row on logout, not just the closing session's owner.**
  File: `unaos/crates/kernel/src/video/wm.rs`, add `close_all_furniture()` next to `close_owner`
  (~`wm.rs:2760`): collect every `r.used` row's id under `table()`'s lock, no owner filter (only
  kernel-owned rows can remain once `session_end_processes` has run), `close(id)` each.
  Call site: `unaos/crates/kernel/src/video/login.rs:637-644` `reopen_after_logout()`, right after
  `users::logout();`.
  Witness: `:: LOGOUTDESK: closed=N kernel=N remaining=0 -> PASS ::` (`remaining` = post-close
  re-scan of `table()`, must be 0).
  Go-red: no-op the `close_all_furniture()` call — `remaining` > 0 (console survives) FAILs; also
  catches a half-fix that calls `close_owner` instead of `close`, since `close_owner` still refuses
  the furniture band.

- **M2 — confirm re-ignition on the next login is not blocked by stale "already exists" guards.**
  Files: `unaos/crates/kernel/src/video/desktop_uefi.rs` (`desktop_app_service`, ~line 674),
  `unaos/crates/kernel/src/video/dock.rs:1312,1670-1690` (`KERNEL_OWNER_DESKTOP`-row-exists check
  gating a fresh shell mint). Confirm the mint path triggers off "row absent", not a separate
  boot-only flag (`DESKTOP_IGNITED`); if boot-only, reset whatever gate blocks a second ignition.
  Witness: `:: LOGOUTDESK-REIGNITE: prev_closed=true fresh_shell=true fresh_console=true -> PASS
  ::`, from a loginst-style fixture (log out, log back in, assert both furniture rows exist again
  under NEW window ids).
  Go-red: skip resetting the guard — second login's pass finds a leftover flag "already
  satisfied" and never re-mints; `fresh_shell=false` FAILs.

- **M3 — pin the multiuser.md §8.1 sentence's supersession** (docs-only, next session): replace
  the quoted sentence at `docs/dev/OS/04_SECURITY_IMMUNITY/multiuser.md:334-335` with the R69
  wording, keep the surrounding aarch64/`STAT.ELF` "owed" notes intact. No witness; go-red N/A.

## Spec pins

Target file: `unaos/scripts/specs/x86-login.spec` (new section after the existing LOGIN-LOGOUT /
LOGIN-END block, `x86-login.spec:142-222`).

```
REQUIRE :: LOGOUTDESK: closed=\d+ kernel=\d+ remaining=0 -> PASS ::
FORBID :: LOGOUTDESK: .* -> FAIL
FORBID :: LOGOUTDESK: closed=\d+ kernel=0 remaining=0 -> PASS
REQUIRE :: LOGOUTDESK-REIGNITE: prev_closed=true fresh_shell=true fresh_console=true -> PASS ::
FORBID :: LOGOUTDESK-REIGNITE: .* -> FAIL
```

The `FORBID … kernel=0 …` line guards against a fixture that passes vacuously because no kernel
window was up to begin with (mirrors the arm-login.spec rule "Require a PROPERTY, never a
LIMITATION").

## Draft code (unbuilt)

```rust
// unaos/crates/kernel/src/video/wm.rs — after close_owner's closing brace (~wm.rs:2760+)
/// LOGOUTDESK (R69): close EVERY remaining row, kernel-owned or not. Returns (closed, kernel).
pub fn close_all_furniture() -> (usize, usize) {
    let mut ids = [WIN_NONE; MAX_WINDOWS];
    let (mut n, mut kernel) = (0, 0);
    {
        let t = table();
        for r in t.rows.iter() {
            if r.used {
                if is_kernel_owner(r.owner_asid) { kernel += 1; }
                ids[n] = r.id;
                n += 1;
            }
        }
    }
    for &id in &ids[..n] { close(id); }
    (n, kernel)
}
```

```rust
// unaos/crates/kernel/src/video/login.rs — inside reopen_after_logout, after `users::logout();`
let (closed, kernel) = crate::video::wm::close_all_furniture();
let remaining = crate::video::wm::live_window_count(); // add if no such scan exists yet
serial_println!(":: LOGOUTDESK: closed={} kernel={} remaining={} -> {} ::",
    closed, kernel, remaining, if remaining == 0 { "PASS" } else { "FAIL" });
```

## Open questions

- Does `wm.rs` already expose a live-row-count helper, or does M1 need to add
  `table().rows.iter().filter(|r| r.used).count()`? Not confirmed this round.
- Is Quarry's window owned under the `KERNEL_OWNER_BASE` band or its own non-kernel `OWNER` const
  (`video/quarry.rs`, not grepped this round)? Affects whether `close_owner` already reaps it, but
  M1's `remaining=0` check catches either case.
- Should `close_all_furniture` run even when `root_logout_refused()` is true (early return in
  `reopen_after_logout`, `login.rs:638-640`)? Needs a person's call.

## Next-session start

1. `grep -n "pub const OWNER" unaos/crates/kernel/src/video/quarry.rs` and
   `grep -n "fn live_window_count\|rows.iter().filter" unaos/crates/kernel/src/video/wm.rs` to
   settle the two open questions above before writing code.
2. Add `close_all_furniture()` in `unaos/crates/kernel/src/video/wm.rs` next to `close_owner`
   (~line 2760), then wire it into `unaos/crates/kernel/src/video/login.rs:637-644`
   `reopen_after_logout()` with the `:: LOGOUTDESK: …` witness.
3. Add the M2 re-ignition check and both spec blocks to `unaos/scripts/specs/x86-login.spec`, run
   `./arroyo test` locally once a build seat is free (not this round — no builds this session).
