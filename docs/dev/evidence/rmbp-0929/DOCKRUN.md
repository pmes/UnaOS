# DOCKRUN — the dock as a person expects it

**Finding.** M1-M3 already exist in `unaos/crates/kernel/src/video/dock.rs`: every live window is a tile (the dock is a window switcher), `paint` lights the pip (`theme::ACCENT`, `IND_D`, ~line 930), `press_at` (~1025) launches pin tiles and raises live ones through `wm::focus_changed` + `wm::raise_one`. Tiles leave on last close via `wm::dock_scan`. Missing: M4.

**Mechanism (M4, tail block "DOCKRUN").** `right_press_at` (right-click) or `lp_arm` (called from `press_at`'s raise arm) + `lp_service` (600 ms hold) open a menu (`MENU_OPEN/OWNER/TILE`); `menu_rect` gives its box above the tile; `menu_press` item 0 = Quit -> `quit_owner` (registered close-box hook `set_quit_hook`, fallback `wm::close_owner`), item 1 = Keep/Remove -> `toggle_keep` (`KEPT`, boot-scoped).

**Witness.** `:: DOCKRUN: tiles=N running=N pinned=N raise=ok quit=ok -> PASS ::` from `dockrun_selftest` (witness), called after `dockid_selftest`. Spec pins: REQUIRE/FORBID in `unaos/scripts/specs/x86-wc.spec`. No knob.

## Written
M1-M3 pre-existing. M4 state machine, geometry, quit, keep toggle, fixture, spec pins. NOT written: menu painting, arch wiring of `right_press_at`/`lp_release`/`lp_service`/`menu_press`/`set_quit_hook` (router ignores secondary buttons), and Keep-in-Dock does not yet render a tile for a non-running kept app (no generic launch seam).
