# SANITYLEGS (rmbp-ledger B488) — every compile-time-sanity fn gets a leg

Branch `exec-rmbp-sanitylegs`, cut from 21521a53 (merge19). Answers FLIGHT26 §2 PANICASSERT / §3.

## Finding
Flight 26 (image 19) panicked on every boot after `FIRSTBOOT stage=installer` at
`video/winmenu.rs:2269 assertion failed: BAR_BOXES_MAX == MENU_TITLES_MAX + 2`, and the panic path reboots: a
reboot loop. The assert sat in `winmenu::uimetrics_assert`, one of EIGHT `uimetrics_assert*` fns UIMETRICS (B372)
turned from `const` asserts into runtime asserts (the metrics became DPI-latched) and called ONCE at boot from
`video::metrics::ignite` (the first desktop service pass, `quarry/live.rs:2887`). No compile leg evaluated them, so
APPMENU2's `+ 5` and the stale `+ 2` disagreed silently until the glass. A scan of the whole kernel
(`unaos/scripts/sanity-legs.py`, brace-context aware) finds NO other runtime assert over consts outside a
`const _`/`const fn`/`#[cfg(test)]`: all 67 sanity asserts live in these eight fns, plus `ignite`'s
`m.check()` -> `panic!`.

## The enumeration (file:line at 21521a53) and where each goes
CONST (operands are `const` items -> `const _: () = assert!(…)` IN PLACE, line-neutral; the compile legs prove them):
- video/dock.rs:374 `LABEL_MAX <= wm::MAX_TITLE`
- video/strip.rs:818 `STRIP_MAX >= 1`; :820 `DOCK_SLOT < STRIP_MAX`; :821 `MENUBAR_SLOT < STRIP_MAX`; :822 `DOCK_SLOT != MENUBAR_SLOT`; :825 `MAX_STRIP_W >= 2048`
- video/crystal.rs:291 `ITEM_COUNT >= 1`
- video/theme.rs:406 `LINE_HEIGHT_PCT > 0`; :431 `LINE_HEIGHT_PCT > 100`
- video/winmenu.rs:2252 `MENU_TITLES_MAX >= 1`; :2254 `MENU_LABEL_MAX/ITEMS_MAX >= 1 && MENU_DEPTH_MAX == 2`; :2267 `wm::MAX_TITLE <= 16`; **:2269 `BAR_BOXES_MAX == MENU_TITLES_MAX + 5`** (the flight-26 panic, b976af1b's value); :2271 `APP_MENU_DEFAULT.len() <= MENU_ITEMS_MAX`; :2273 `APP_ITEM_QUIT != APP_ITEM_ABOUT`
- video/menubar.rs:380 `TITLE_GLYPHS <= wm::MAX_TITLE`; :417 `BATT_PCT_GLYPHS >= 4`
= 17 const asserts.

RUNTIME (operands are the DPI-latched metric readers `CELL_W()`, `BAR_H()`, `TITLE_HEIGHT()` …; proven for every
scale 1.0..=4.0 by `ui.rs`'s compile-time table proof) -> `ck.t(…)` on a `metrics::Sane` counter, run by `tests sanity`:
- video/dock.rs:364,367,369,372,373 (5) · video/strip.rs:827 (1) · video/crystal.rs:283,287,289,293,295 (5)
- video/theme.rs:397-405 (9, `uimetrics_assert_positive`) · :415-429 (7, `uimetrics_assert_relations`)
- video/winmenu.rs:2257,2258,2260,2264,2265 (5) · video/menubar.rs:372-423 (17; its other 2 are const)
- video/wm.rs:130 (1, `uimetrics_assert_title_cell`, same-line edit) · video/metrics.rs `ignite` `m.check()` panic (1)
The fns are renamed `uimetrics_assert*` -> `uimetrics_sanity*` and take `&mut metrics::Sane`; `ignite` no longer
calls them nor panics (R80: no test at boot) — it prints `[ui] metrics … asserts=tests` (was `asserts=ok`).

## Seam
Kernel — wm (the furniture's own files; no new file, no new store, no new knob). The leg is registered by
`metrics::ensure_tests` (already called from `tests::shell_verb`), so `tests.rs` is untouched.

## Milestones
- M1 (c043154a) the 17 → `const _`, the 50 → `ck.t`, the boot call and the boot panic removed, `tests sanity` at metrics.rs's tail.
- M2 `unaos/scripts/sanity-legs.py` (GATE-SANITY, `--selftest`): refuses a runtime `assert!`/`assert_eq!`/`assert_ne!`
  whose operands are all const-shaped (UPPER idents, UPPER() metric readers, literals, paths) outside a
  `const _`/`const` item/`const fn`/`#[cfg(test)]`/a tests fixture fn (`*selftest*`, `*fixture*`, `*sanity*`), and any
  call of a `*_assert*`/`*sanity*` fn from `ignite`. Joins `GATES_SET` as `sanity`, with a `_gates_plant` arm.

## Witness (tests leg, not boot)
`tests sanity` -> `:: SANITY: const_asserts=17 runtime_moved=51 left=0 -> PASS ::` where `left` = moved relations that
do NOT hold on this boot's latched metrics (PASS iff 0). Boot: `[ui] metrics … consts=0 asserts=tests`.
Host: `python3 unaos/scripts/sanity-legs.py unaos` -> `GATE-SANITY: … findings=0 -> PASS`.

## Owed
The metal reading of `tests sanity` on the next flight; STATUS rows ST179/ST180 quote the old `asserts=ok` (past wires,
untouched).

## Results (host + compile legs, this branch)
- x86 metal-shape `cargo check` (the seat's gate line) exit 0; aarch64 `login,loginst,virt_el0,lumen,desktop_firmware,quarry,facet,usbnet` exit 0; `tegra,login,loginst,virt_el0` exit 0.
- `sanity-legs.py unaos` -> `GATE-SANITY: files=346 const_asserts=17 fixture_asserts=0 findings=0 -> PASS`; on the 21521a53 tree it reads `findings=76 -> FAIL` (the 66 const-shaped asserts incl. `BAR_BOXES_MAX == MENU_TITLES_MAX + 2`, `ignite`'s 8 calls + its `panic!`, the undeclared count). `--selftest` 14 cases PASS; `./arroyo gates --selftest` `plants=12 caught=12 -> PASS`.
- Seen in passing, not this arc's: `video/menubar.rs` ~2726 `#[cfg(feature = "sntp6")] const _: () = { … TITLE_X0() … CELL_W() … }` calls the (UIMETRICS-runtime) metric fns inside a const block — the `sntp6` knob will not compile; GATE-SANITY does not flag it (it is a const context).
