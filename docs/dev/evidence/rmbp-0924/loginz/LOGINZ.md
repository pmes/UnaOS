# LOGINZ — the login screen and the set-password alert are the z ceiling (rmbp-ledger B223), 2026-09-24

Handed over by `docs/dev/evidence/rmbp-0915/flight15/FLIGHT15.md` §2 (the boot-1 blocker: "the pw dialog got
covered up and the mouse cannot click and drag anymore … i cannot select a window or open the crystal menu").
Prep: `docs/dev/evidence/rmbp-0924/prep/LOGINZ.md` (executor, same day). Branch `claude/optimistic-ramanujan-r3qyu5`.

## Mechanism (as flown)

The screen and the alert take every key by design (SO44) but were ordinary rows in z: `wm::create_inner`
hands out z from the one monotonic `next_z`, and `focus_changed`'s raise arm re-stamps a focused owner's rows
above everything. Flight 15: the alert `win=3 z=4` at 7177 ms, then `win=4 z=5`, `win=5/6 z=6..10` with
`[wc-fv] focus raise` — later windows on top of the input-taker; the keyboard still reached it.

## The fix (`video/wm.rs` tail; three same-line folds; `video/login.rs` two folds)

- `MODAL_WIN` (AtomicU32), `set_modal_top(id)`, `clear_modal_top(id)` (CAS so a stale clear cannot drop a newer
  pin), `reassert_modal_top(t, except_id)` — under the table lock: if the pinned row is live, not `except_id`
  and not already above every other live non-compat row, it takes a fresh z off `next_z`. `SHELL_Z` is this
  file's FLOOR idiom; this is the same idiom pointed up.
- Called at the three z writers: `create_inner` beside the row publish (the newcomer is `except_id`),
  `focus_changed`'s raise arm after the per-window bump loop, and `raise_one` (the dock's tile press).
- `login::open_as` pins the row it mints (`modal=true` on the `[login] screen open` line); `take_down` clears
  the pin before `wm::close`. The screen and the alert share this path, so a LOGOUTUI alert built on it
  (B222, R70) is pinned for free.
- `MODAL_LAST` records the reassert's last verdict (2 except, 3 dead, 4 already-top, 5 moved) for the fixture;
  nothing prints under the guard.

## Fixture and pins

`wm::loginz_selftest()` (witness): pin a row, create a rival after it, focus-raise the rival, then clear the
pin and raise again as the control. Called in the x86 fixture block (`arch/x86_64/syscall.rs`, folded before
`wci_rollup`) and beside `focusvis_selftest` on aarch64. Pinned in `x86-wc.spec`:
`REQUIRE :: LOGINZ: … create_verdict=5 after_create=1 … after_raise=1 unpinned_rival_above=1 -> PASS ::`,
`FORBID :: LOGINZ: .* -> FAIL`; `x86-login.spec`'s screen-open REQUIRE now ends `modal=true`.

## Proof — wc lane `UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 ./arroyo test 90`

Clean (final run, serial copied to the session scratchpad):
```
:: LOGINZ: modal_z=127 rival_z=126 create_verdict=5 after_create=1 raised_modal_z=129 raised_rival_z=128 after_raise=1 unpinned_rival_above=1 -> PASS ::
✅ MBENCH PASS — 43/43 required witnesses, 0 forbidden hit(s)   (x86-wc.spec replay)
```
The lane's rc=1 is the container's SOCK-3 (environmental, `docs/dev/FIXTURE_FLAKES.md`).

GO-RED BY MUTATION (`reassert_modal_top` not writing `z`; reverted byte-for-byte):
```
:: LOGINZ: modal_z=125 rival_z=126 create_verdict=5 after_create=0 raised_modal_z=125 raised_rival_z=128 after_raise=0 unpinned_rival_above=1 -> FAIL ::
❌ MBENCH FAIL — 42/43 required witnesses, 3 forbidden hit(s)
```

One false start, kept for the record: the first fold on the publish line was appended AFTER that line's
same-line comment and so was comment text — the fixture read `create_verdict=0` (never ran). The fold now
sits before the `//`. A same-line fold on a commented line goes before the comment, always.

aarch64: `UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_FATIMG=sf ./arroyo test-arm 60`
rc=0 (compiles; the fixture's call site is under the desktop cfg and does not run on headless virt).

## Not flown

Boot 16: with a fresh card, the set-password alert must stay visible and clickable while the launcher and
fixture windows come up (`[login] screen open window=… modal=true`, then no `[wc-fv] focus raise` line whose
z exceeds the alert's without a matching bump of the alert). Peter's read: the dialog stays on top.
