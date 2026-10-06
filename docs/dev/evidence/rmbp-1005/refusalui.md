# REFUSALUI (rmbp-ledger B468) — one on-screen refusal surface; the toast's button answers its poster

**Finding (on 575194d1, the flight-24/25 wires carry no refusal line).** Three refusals exist only on the wire:
OPENERTRUST's `[openers] preferred=<v> refused=not-a-registrant` (fs/assoc.rs `refused_wire`), FWPIN's
`:: wifi: <role> REJECTED … reason=<unpinned|violates-layout|pin-mismatch>` (wifi/firmware.rs `admit`), and the
system-tree refusal (today SECREVIEW F2's silent `EACCES` in fs/attrsys.rs `do_set`; ROOTACL's `[rootacl] refused`
line when that arc lands). And `dialog::bus_fulfil`'s TOAST arm posts `toast::post(title, message)` — the poster's
buttons and token are dropped, so NOTIFY's `ACT_ANSWER` arm can only say `answer-owed(dialog2-bus-fold)`.

**The seam.** No new store, no new verb, no second router. `dialog::refused(what, why)` is THE refusal entry beside
`dialog::notice`: one table `REFUSALS` (`what` -> alert or toast, and whether the person caused it), the alert built
exactly as `notice`'s error arm builds it, the toast through `toast::post` (NOTIFY's stack). `why` is the wire's own
words. Once: the same `(what, why)` twice in a row shows nothing the second time (the wire still says it).
The TOAST verb keeps `toast::queued()` (DIALOG2's fixture count): the toast queue entry carries the poster's
default (last) button label, owner, token and index; NOTIFY's `take_inbound` turns it into an `ACT_ANSWER` card
(`arg` = owner 8 LE + token 4 LE + button 1), and `run_action`'s `ACT_ANSWER` arm calls `dialog::toast_answer`, which
is the alert path's own `reply` — `INPUT_EV_DIALOG_ANSWER` (`token<<8 | button`) to the owner's input ring.

**Milestones.** M1 this design. M2 the toast verb answers (toast.rs entry fields + `take_full`, notify ACT_ANSWER,
dialog `toast_answer`). M3 `dialog::refused` + the three call sites (Quarry/launcher open path via
`openers::open` reading `assoc::override_refusal`, FWPIN's three `refuse(n)` sites, attrsys F2). M4 `tests refusalui`.

**Witness.** `[refusal] what=<what> why=<why> -> <alert|toast|repeat>` at every refusal; `[notify] action <label>
app=… title=… -> answered(token=<t> button=<b>)` then `[dialog] answer=ok button=<b> token=<t> owner=<o> delivered=<0|1>`.
`tests refusalui`: `:: REFUSALUI: opener=alert toast_action=answered fwpin=toast rootacl=alert -> PASS ::`.
Metal: the boot card with an unpinned b43 image shows one toast `Wi-Fi firmware refused` and prints
`[refusal] what=Wi-Fi firmware refused why=b43-ucode reason=unpinned -> toast`.

**Owed.** ROOTACL (B456) is design-only on its branch: at its fold its `[rootacl] refused` line calls
`crate::video::dialog::refused(crate::video::dialog::WHAT_SYSTEM_FILES, why)` (cfg'd as the attrsys site is) and
F2's `system_tree` call — which carries the call today — goes with it. The opener alert fires on the OPEN (Quarry
press / launcher pick), never from the per-row resolver a listing runs.
