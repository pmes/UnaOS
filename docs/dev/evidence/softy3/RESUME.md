# SOFTY3 — RESUME (stopped 2026-10-13 by the seat: wrong model; resumes on Opus)

Branch `exec-rmbp-softy3`, worktree /home/user/exec/softy3, cut from `8da376cb`. WIP commit `45cd4944` holds everything
below (code, ledger row B651, evidence). The arc file: /root/unaos-bench/scratch/cloud/wave34/softy3.md.

## DONE (all compiled once, rc=0)

1. **statusmenu.rs:399** — the input leg reads the table name back from the keymap (`n0 = active().name`;
   `pc_selected()` flips and the name moves away, then clears and the name returns); no literal typed. One line,
   line-neutral (statusmenu is unconditional, video/mod.rs:1236).
2. **notify.rs**
   * tail: `pub fn request_center(open: bool)` — REQ_CENTER 4 = open, 5 = close (1 = bell toggle, 2 = outside close,
     3 = clear are untouched); `fn softy_center_at(pw, sw, sh) -> Option<(x, y)>` — under Softy the Center sits above
     `softy::taskbar::top` behind a FLY_GAP band at the right edge, LEFT of the calendar flyout when
     `startmenu::open_kind() == 2` and its x leaves room; `ph` from the `PANEL` atomic (so `center_rect(pw)` keeps its
     signature and four callers).
   * `center_rect` (≈:315): returns `softy_center_at` when Some, else the bar placement as before.
   * `pass` match (≈:822): `4 if open => {}`, `1 | 4 => center_show` with the wire's new trailing field `by=<bell|clock>`,
     `5 => center_close(.., "clock")`.
   * `press_at` (≈:648): with the Center open under Softy AND the calendar open, a press outside the Center returns
     `false` (not spent, no request) so `strip::press_route` falls to the dock arm = `softy::taskbar::press_at`, which
     closes the calendar (clock toggle / outside) and that closes the Center through the seam.
   * 16 legacy alias calls repointed (chrome_face→surface_window, frame_line→line_subtle, button_face→surface_raised,
     content_text/title_text_active→text_primary, title_text_inactive→text_secondary); header doc wire list updated.
3. **softy/startmenu.rs** — `open(2)` calls `notify::request_center(true)` after its open line (≈:749); `close` of kind
   2 calls `request_center(false)` (≈:767).
4. **softy/mod.rs** — fixture leg 10 before the switch back: press the clock (`probe_hot(2)`), `notify::service()`,
   assert calendar + Center open; press again, both closed; the operator's open Center asked back. Second line
   `:: SOFTY: clock->center open=ok close=ok -> PASS ::` after the SOFTY2 line. tests.rs untouched (rides `softy`).
5. **docs/dev/OS/rmbp-ledger.md** — row B651 inserted after B645 (line 681; 7 cells, no `|`, no claim word).
6. **docs/dev/evidence/softy3/SOFTY3.md** (design, fixture, go-red, the two not-done items with their exact change)
   and **compile.txt**.

## NOT DONE — the exact next step

* `(3) Action::Start` and `(4) SOFTY.md`'s owed list are OUTSIDE the arc's FILES (keymap.rs / theme.rs;
  docs/dev/OS/05_USER_EXPERIENCE/SOFTY.md) — HEAD rule 3: not made; the exact change is in SOFTY3.md "Not done".
  smallfix3.rs and clipboard.rs are therefore untouched (arms without the variant do not compile).
* The pre-commit leg (`./arroyo gates`, HEAD rule 12 as corrected) was run once: `GATES: 17 run 16 green -> FAIL`,
  the one red is `gate=deps rc=1 deps-audit --check (policy any): 89 crates, 1 failing` — NOT this arc's (no Cargo.toml
  or dependency touched; the deps audit reads crates.io policy). Log: /root/unaos-bench/scratch/cloud/wave34/softy3/gates.log
  and unaos/target/gates/ (the shared target). The resuming executor re-runs gates and, if deps is still the only red,
  reports it as foreign.
* Then: squash/retitle the WIP commit into the real one — `video: SOFTY3 — the clock opens notify's Center; the
  statustray leg reads the keymap's name` with the design body (SOFTY3.md §(1)/(2) is that body), ticking B651.

## The last compile

```
cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave34/target cargo +nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat /root/unaos-bench/scratch/cloud/wave34/kfeats24.txt),stria,gen7r9,wifi9,usbnet,wifi10"
```
rc=0 (1m 55s; log /root/unaos-bench/scratch/cloud/wave34/softy3/check-x86.log). Only the two pre-existing chrome.rs
private_interfaces warnings name this arc's files.

## Learned, not in the arc file

* `notify` and `softy` are both `#[cfg(any(all(x86_64, wc), all(aarch64, desktop_firmware)))]` (video/mod.rs:1205,
  :1260): not lexed into the knob-off image, so line counts in them are free; statusmenu.rs is unconditional.
* `strip::press_route` (strip.rs:793) runs `notify::press_at` FIRST; under Softy the dock arm forwards to
  `softy::taskbar::press_at` (dock.rs:1087). Without the press_at carve-out the clock's second press would close the
  Center and be swallowed, leaving the calendar up.
* `notify::pass` swaps REQ_CENTER to 0 BEFORE `ST.try_lock()`; on contention the request is lost (pre-existing; the
  fixture drives one pass per press and may read `no` under a concurrent desktop pass).
* The arc file's "docs/dev/evidence/softy/SOFTY.md" does not exist; the owed list is docs/dev/OS/05_USER_EXPERIENCE/SOFTY.md:164.
