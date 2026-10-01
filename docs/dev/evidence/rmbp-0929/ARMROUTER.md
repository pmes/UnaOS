# ARMROUTER — the aarch64 input router gets the arms x86 got

**Finding.** Tuesday's rows (WINCYCLE, DOCKRUN, SCREENLOCK, APPMENU, BRIGHTKEYS, VOLKEYS) each say "aarch64 owed": the
shared seams (`wm::cycle_pick/commit`, `login::lock`, `dock::right_press_at/menu_press/lp_*`, `appmenu::verb_*`,
`brightkeys::key/service`, `status::volkey_usage`) exist, the aarch64 decode never called them. FIRSTBOOT3 moved x86's
`hittest` behind `tests` but left aarch64's `wcb_launcher` ladder running at boot.

**Mechanism** (same seam calls as x86, cfg `desktop_firmware` where x86 has `wc`).
- WINCYCLE + SCREENLOCK: top of `arch/aarch64/syscall.rs::wc_focus_key` (mirrors `x86_64/syscall.rs:5818`, minus `drag_settle_disarm`).
- DOCKRUN: `wc_click_route` after `let cur` (mirrors `x86_64/syscall.rs:7425-7436`); `lp_service` + `brightkeys::service` on `bootpace.rs:343`.
- BRIGHTKEYS: `pal::push_event` consumes brightness actions on aarch64 too; the board has no gmux so `gmux_written=0` (`write_gmux` is cfg-off) = "unsupported".
- VOLKEYS: xHCI key decode (`drivers/xhci/mod.rs`, the Pi/Orin path) calls `status::volkey_usage` (EHCI already did); `amp_written=false` (no HDA).
- APPMENU: `video::appmenu` opened to aarch64 (owner = asid; `owner_of_row` identity, pick via new `aarch64::syscall::user_input_push_owner`), 3 bus arms beside BUS_VERB_NOTICE, `wm` reap cfg widened.
- Tests: `hittest`, `ctrldecline`, `dragperf`, `dragwedge` now `tests::register` (run on the spot under `UNAOS_TESTS_AT_BOOT`).
- Witness `armrouter_witness()` at the tail, called from `u7_launcher` (same line as `desktop_firmware::arm()`).

**Witness.** `:: ARMROUTER: arms=[wincycle,dockrun,lock,appmenu,brightkeys,volkeys] tests_moved=<n> -> PASS ::`
**Pin caveat.** `arm-login.spec`'s lane is `login,loginst,virt_el0` (header) with NO `desktop_firmware` (that is `UNAOS_PIDESK`), so there the
line reads `arms=[]`; the spec pins the `:: ARMROUTER:` prefix and FORBIDs `-> FAIL`. The six-arm text is for a PIDESK lane.

## Written
All milestones in one commit. Boot 17 (Pi/Orin desktop image) should show the six-arm line once; not compiled or run (R76/R78).
