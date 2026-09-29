# WINCYCLE — window management a person expects

**Finding.** `<TAB>` cycles *owners* by id and only on a bare Tab; Alt+Tab/Cmd+Tab was deliberately swallowed
(`xhci::hid_key_ascii`). Zoom exists only as the green disc (`wm::zoom`, video/wm.rs). A title double-click did nothing.

**Mechanism.** M1: `keymap::Action::CycleWindow` (`theme.rs` CRISPY `alt-tab`+`cmd-tab`, PC `alt-tab`) is pushed as
`Event::Action` by the xHCI decoder; `x86_64/syscall.rs::wc_focus_key` intercepts it -> `wm::cycle_pick` (least
recently raised live app window, z order; furniture/compat/parked excluded) -> `user_input_set_active` ->
`wm::cycle_commit` = `focus_changed` + `raise_one` (both end in `reassert_modal_top`, LOGINZ wins).
M2: `wm::title_dblclick` (400 ms, same id) in the x86 title-bar arm -> `wm::zoom_titled` -> existing `wm::zoom`
(max work-area scale below the menubar / above the dock strip, pre-zoom placement in `Window::zoom_saved`).
M3: `[wm-act] cycle ... -> action=cycle raised`, `[wm-act] action=zoom win=N route=title-dbl -> zoomed`.

**Witness.** `:: WINCYCLE: windows=N order=[..] after_tab=<id> zoom=<w>x<h>-><W>x<H> restored=1 -> PASS ::`
(`wm::wincycle_selftest`, folded onto the `closemin_selftest` line). Pins: `unaos/scripts/specs/x86-wc.spec`. No knob.

## Written
M1 keymap+router+wm cycle, M2 dblclick zoom (x86 router), M3 witness+spec. aarch64 title-arm/key hook NOT wired
(shared wm fns are ready; the aarch64 router arm is a giant one-liner).
