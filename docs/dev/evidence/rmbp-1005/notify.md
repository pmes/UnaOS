# NOTIFY — rmbp-ledger B418, MACPARITY row 24

## Design (written before the code)

**Finding.** DIALOG's TOAST (`video/toast.rs`, B395) shows ONE 3 s card at a time from a 4-deep queue and forgets
it; DIALOG2 (B404, its branch) routes every informational notice there (`toast::post`). No stack, no record, no
tray: a user who looked away has nothing. The bar has no notification item.

**Seam.** `Kernel — wm` (the toast's own charter): notifications are kernel chrome (compat overlay rows that are
never hit-tested or focused, R88) plus one bar status item. ONE module, `video/notify.rs`; `toast.rs` stays the
posting front (its queue-only `post`, its API and DIALOG2's fixture helpers untouched) and its `service`
hands the queue to NOTIFY — the toast is NOTIFY's transient face. The Do-Not-Disturb flag is Principia's
(`system.notify.dnd`, a `prefs_core::schema` row; R79: a preference is Principia's), written by Settings.

**Milestones.**
- M1 `video/notify.rs`: the stack (<= 4 cards top right, newest on top, APPRES icon, title, line, optional action
  button; click on the button runs the action, a click elsewhere dismisses; hover pauses the timeout) and the
  session ring (100, newest first). `toast::service` delegates; `video::notify(..)` is the cfg-free post shim.
- M2 the bell: a menubar status item left of the battery slot (our glyph) with the unread badge; a press opens the
  Notification Center panel (today's notifications grouped by app, newest first, `Clear`); a press outside closes it.
  Routed first in `strip::press_route` (after the session gate).
- M3 posters: USBSTOR mount / unmount (action `Open`, Quarry at the volume), PANICSCREEN's previous-boot stop
  (collected quietly beside its dialog, action `Show log`), NETCLOCK's DHCP lease. Battery levels arrive through
  DIALOG2's router (`Low Battery` -> `toast::post` -> NOTIFY) at the fold.
- M4 DND: `system.notify.dnd` (schema row), Settings > General row 8 toggle; on = collect silently (badge only).
- M5 `tests notify`: model-only (headless) fixture over the real post/service/press code.

**Witness.** `:: NOTIFY: stack_max=4 center=ok ring=100 badge=ok dnd=<0/1> posted=<n> -> PASS ::` (`tests notify`);
on the glass path: `[notify] post app=<a> title=<t> dnd=<0/1> -> banner|collected`, `[notify] show win=<n> slot=<i>
title=<t>`, `[notify] closed by=<timeout|click|overflow|clear> title=<t>`, `[notify] action <label> -> <what>`,
`[notify] center open items=<n> apps=<n>` / `center closed by=<..>`.

**Owed.** The alert sound (MACPARITY row 26). The bus verb TOAST's action field: `BUS_VERB_TOAST` is DIALOG2's
(not on this tree); `notify::post_full` takes the action and `ACT_ANSWER` carries `(owner, token)` — at the fold
DIALOG2's `bus_fulfil` TOAST arm calls it and answers on `INPUT_EV_DIALOG_ANSWER`. A Settings > Notifications pane.
Clock-click opening the center (the Mac's); the bell is the item. Slide-in animation.
