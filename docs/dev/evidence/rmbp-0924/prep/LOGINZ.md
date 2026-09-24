# LOGINZ — prep

## The finding

Flight 15 §2 (`docs/dev/evidence/rmbp-0915/flight15/FLIGHT15.md`):

> **LOGINZ (new, the boot-1 blocker)**: the set-password alert takes every press (SO44, by design)
> but is an ORDINARY window in z-order: created `win=3 … z=4` at 7177 ms, then `win=4 … z=5` at
> 7217 ms and, at 23703-23726 ms, `win=5/6 … z=6..10` with `[wc-fv] focus raise` — the
> launcher/fixture windows landed ABOVE the input-taker. Peter could not see it or click anything
> else. The keyboard still reached it (his mismatch, then his password).

Peter's words (exec brief): the screen and the alert are pinned topmost while open — a later create
or focus-raise cannot pass them; **go-red = a window created after the alert opens with a higher z.**

## Mechanism

- `login.rs:522 open_set_password` → `open_as` (`login.rs:552-611`) creates the modal via
  `wm::create_at(OWNER, …)` (`login.rs:596`), `OWNER = 0` (`login.rs:145`) — an ordinary asid, not a
  `KERNEL_OWNER_*` band (`wm.rs:1455-1479 is_kernel_owner`). `take_down` (`login.rs:610-619`) is the
  only close path; both `State::Open` (screen) and `State::SetPw` (alert) share this open/close pair.
- `wm.rs:3473-3527 create_inner`: every new window's `z` comes from one monotonic counter
  `t.next_z` (`wm.rs:3525-3526`). A window created later always gets a larger `z` — no ceiling check
  against an already-open modal exists.
- `wm.rs:3096 focus_changed(asid)`, RAISE arm (`asid != 0`, `wm.rs:3340-3353`): focusing any owner
  re-stamps every window it holds with a fresh `t.next_z` and prints `wm.rs:3433-3436`
  `"[wc-fv] focus raise asid=… z=… shell_z=…"` — the exact line the flight log shows at
  23703-23726 ms for `win=5/6 z=6..10`, landing above the login window's `z=4/5`.
- `hit_test` (`wm.rs:2975-3010`) and `composite` order strictly by `z`, ties by id — highest `z` is
  both drawn last (on top) and what the pointer hits.
- The repo already has this idiom as a **floor**: `SHELL_Z` (`wm.rs:473`) is re-claimed off the same
  `next_z` counter every shell raise (`wm.rs:3192-3213` furniture loop; `above_shell` buries
  `z < SHELL_Z`). There is no symmetric **ceiling** for a modal; LOGINZ needs one.
- Keyboard already reaches the modal regardless of `z` (SO44 router rule, `login.rs:94-95, 651-730`)
  — the defect is purely visual/pointer: the modal's pixels and hit-box get buried under later `z`s.

## Plan

**M1 — modal-ceiling primitive.** `wm.rs`, near `SHELL_Z` (`wm.rs:473-493`). Add
`static MODAL_WIN: AtomicU32`, `pub fn set_modal_top(id)` / `pub fn clear_modal_top(id)` (CAS-guarded
so a stale clear can't drop a newer modal), and `fn reassert_modal_top(t, except_id)`: if `MODAL_WIN`
names a live row not equal to `except_id`, give it a fresh top `z` off `t.next_z` — the furniture-raise
(`wm.rs:3192-3213`) pointed up instead of down. Witness: `":: LOGINZ: modal_win={} reasserted_z={} ::"`
printed only when it actually moves the modal's `z`. Go-red: no-op the `t.rows[slot].z = z` write —
the modal's `z` stays fixed while `next_z` advances under it.

**M2 — wire the login screen to it.** `login.rs` `open_as` (`login.rs:552-611`): after
`WIN.store(id, …)` (`login.rs:603`) call `wm::set_modal_top(id)`. `take_down` (`login.rs:610-619`):
before `wm::close(id)` call `wm::clear_modal_top(id)`. Covers screen and alert alike (M2 shared path).
Witness: add `modal=true` to the existing `"[login] screen open window=…"` line (`login.rs:606`).
Go-red: skip the `set_modal_top` call — today's behaviour, unprotected.

**M3 — enforce at create time.** `wm.rs:3473-3527 create_inner`, right after `t.rows[slot] = row`
publishes the new row (still under the table lock): call `reassert_modal_top(&mut t, id)`. This is
the flight's `win=5/6` create half. Go-red fixture: with the login screen open, `wm::create` a second
window and assert its `z` ends up `<` the modal's (today it's `>`).

**M4 — enforce at focus-raise time.** `wm.rs:3096 focus_changed`, RAISE arm (`wm.rs:3340-3353`),
right after the per-window bump loop, before the `"[wc-fv] focus raise"` print (`wm.rs:3433-3436`):
call `reassert_modal_top(&mut t, WIN_NONE)`. This is the exact `[wc-fv] focus raise` line the flight
log shows outrunning the modal. Go-red fixture: with the login screen open, create a window then
`wm::focus_changed` its asid; assert the modal's `z` is still max afterward (today it loses).

**M5 — fixture + spec wiring.** New `loginz_selftest()` beside `focusvis_selftest`/`vugprobe_selftest`
(`wm.rs:21852` area, `#[cfg(feature = "witness")]`): open the login screen, create + focus-raise a
rival window, print `":: LOGINZ: modal_z={} rival_z={} topmost=true|false -> PASS|FAIL ::"`. Wire it
where the neighboring `*_selftest()` fns are called (not read this round — see Next-session start).

## Spec pins

New `unaos/scripts/specs/x86-loginz.spec` (pattern from `x86-login.spec`):

```
REQUIRE :: LOGINZ: modal_z=\d+ rival_z=\d+ topmost=true -> PASS ::
FORBID :: LOGINZ: .* -> FAIL
FORBID :: LOGINZ: .* SKIP
```

Flat `\d+`/`.*` only — no regex look-around.

## Draft code (unbuilt)

```rust
// wm.rs — after SHELL_Z (wm.rs:473)
static MODAL_WIN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(WIN_NONE);
pub fn set_modal_top(id: WinId) { MODAL_WIN.store(id, core::sync::atomic::Ordering::Release); }
pub fn clear_modal_top(id: WinId) {
    let _ = MODAL_WIN.compare_exchange(id, WIN_NONE,
        core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Relaxed);
}
// Re-claims the ceiling off the SAME counter every other z comes from (SHELL_Z's furniture-raise,
// wm.rs:3192-3213, pointed up instead of down). `except_id` is the row that triggered this call.
fn reassert_modal_top(t: &mut Table, except_id: WinId) {
    let modal = MODAL_WIN.load(core::sync::atomic::Ordering::Acquire);
    if modal == WIN_NONE || modal == except_id { return; }
    let Some(slot) = t.rows.iter().position(|r| r.used && r.id == modal) else { return; };
    let z = t.next_z;
    t.next_z = t.next_z.wrapping_add(1).max(1);
    if z > t.rows[slot].z {
        t.rows[slot].z = z;
        t.rows[slot].damage_all();
        serial_println!(":: LOGINZ: modal_win={} reasserted_z={} ::", modal, z);
    }
}
```

```rust
// wm.rs — create_inner, right after `t.rows[slot] = row;`
reassert_modal_top(&mut t, id);
// wm.rs — focus_changed RAISE arm, right after the per-window z-bump loop (wm.rs:3340-3353)
reassert_modal_top(&mut t, WIN_NONE);
```

```rust
// login.rs — open_as, right after login.rs:603
WIN.store(id, Ordering::Relaxed);
wm::set_modal_top(id); // LOGINZ
// login.rs — take_down, before wm::close(id) (login.rs:613)
wm::clear_modal_top(id); // LOGINZ
```

## Open questions

1. Login's `OWNER` is `0` (`login.rs:145`), the SAME asid the shell's own furniture raise uses
   (`focus_changed`'s `asid == 0` arm). Is that overlap harmless, or should login move to a reserved
   `KERNEL_OWNER_BASE`-style id before this ships?
2. Does the pin need to survive Log Out → `reopen_after_logout` (`login.rs:626-635`, new window id)?
   M2 re-registers on every `open_as`, which should cover it — a person should confirm against a real
   reopen boot.
3. Exact call site for `loginz_selftest()` (M5) — not read this round (budget).

## Next-session start

1. `grep -n "_selftest()" unaos/crates/kernel/src/main.rs` to find where to wire `loginz_selftest`.
2. Apply the M1/M2 draft snippets to `wm.rs` and `login.rs`, then `./arroyo check`.
3. Write `loginz_selftest` (M5), `./arroyo test`, and read `target/serial.log` for `:: LOGINZ:` with
   `awk`, never bare `grep`.
