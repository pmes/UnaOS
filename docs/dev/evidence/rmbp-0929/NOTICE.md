# NOTICE — the OS's notice surface

**Finding.** exec-rmbp-logout added `open_alert`/`alert_ok`/`State::Alert` (login.rs) for ONE purpose (refused Log Out, title "Log Out", one line). The OS has other things to say (stick pulled, store read-only, program died) and no surface for them.

**Mechanism.** The alert window is generalised: `Note{title<=24, 2 lines<=46}`, `NOTICES` (cur + queue of 4) at the login.rs tail. `notice_show` = post+pump; `notice_post` = queue only (`try_lock`, no heap, no wm — safe from xHCI/fault/flush/bus); `notice_pump` opens the oldest when none is open (head of `consume_key`, every dismissal); `alert_ok` -> `notice_dismissed` opens the next. `refused_alert` is a caller (`notice_show(b"Log Out", ..)`). Anchors: login.rs `open_alert`, `alert_ok`, `repaint` Alert branch, `open_as` title.
Callers (all `fs::users::screen_notice`, no-op without a desktop): USB stick removed = drivers/xhci/mod.rs `unpublish_usb_geometry` returned true (it WAS the storage device); store veto = fs/users.rs `write_root_file`; program fault = arch/x86_64/interrupts.rs `ring3_fault_kill` (fixtures `u<digit>..` silent).
Ring 3: `BUS_VERB_NOTICE = 10` (una-abi; 7/8/9 are APPMENU's), body = up to two `\n` lines, title = owner's armed program name via `wm::app_name_of(owner)` (`Program` if none); x86 owner = row+1, aarch64 = asid.

**Milestones.** M1 surface + refused_alert caller; M2 kernel callers; M3 bus verb.
**Witness.** `:: NOTICE: title=Fixture-A lines=2 queued=1 shown=1 dismissed=1 -> PASS ::` (headless form, loginst). Also `:: NOTICE-OPEN: title=.. lines=.. -> PASS ::` per open.
**Spec pins.** unaos/scripts/specs/x86-login.spec REQUIRE the witness, FORBID `:: NOTICE: .* -> FAIL`. No knob.

## Written
All three milestones written, uncompiled. Not done: aarch64 fault-kill caller; a periodic pump (posted notices open on the next key); bus fixture through `busx_msend_for` (needs APPMENU's `appmenu_call`); no ring-3 lib wrapper.
