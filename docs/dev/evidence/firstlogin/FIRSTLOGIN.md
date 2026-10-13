# FIRSTLOGIN (rmbp-ledger B636) — the two first-login costs of flight 34, read off its wire

Source: `docs/dev/evidence/rmbp-0915/flight34/f34-boot1.log` (fresh card) and `f34-boot2.log` (same card, una in the store). No QEMU run (wave-32 head, R107: the seat runs the checks at the fold).

## (A) The bar: `bar_ms=341` (boot 1) against `bar_ms=40` (boot 2) — the step is the usbnet bring-up, not the first login

Boot 1, 15:01:15–19Z, in order:

- The first-login work all prints BEFORE the session opens, on the create form's worker: `[users] home=/home/una created`, `[users] kdf calibrated … ms=250`, `[prefs] load`, `:: PREFS: path=/home/una/settings domains=0 …`, `[settings] loaded n=0`, `[holocron] ring=created at=login user=una ms=1313`, then `[login] submit kdf_ms=2318 adduser_ms=1292 on=worker input_blocked_ms=0 kind=create` and `[login] session open user=una role=admin`.
- `[boot] phase=desktop from=setter why=session at=22031ms`, `[boot] services up first=bt at=22036ms`, `[filetypes] login=none stamp=absent … ms=11 owed=after-bar` (the line just before the bar's ask in `login::close_into_session`), then the setter tail `[users] stage-resolve sub=… at_ms=22076` (stage published 22076).
- Then, inside the bar's span: `[usbnet] bring-up try=0 of 3 kind=ax88179 slot=2 ms=333 ep0_timeouts=0 at=- -> armed`, and only after it `[status] poll n=1 answered=1 src=smc` (the battery source), `[hda] probe deferred-start at=22380ms`, the walk, `[strip] paint tenant=menubar items=input,net,battery,…`, `[desktop] built at=login ok bar_ms=341`.

Ask ≈ 22050 ms; 22050 + 333 ≈ 22383 = the HDA start; + 341 = the paint. Boot 2 has no dongle at the login (its bring-up is at 15:27:51Z, five minutes later, `slot=4 ms=334`) and reads `bar_ms=40`. Image 25's boot 2 (46) is the same no-dongle shape.

Mechanism: `drivers/xhci/usbnet.rs:386` `bringup_pending` holds the class bring-up on `boot::services_gate("usbnet")` until the ignition, then the bring-up runs in the usb-pump's `xhci` step (main.rs, `lag::P_XHCI`), the same loop whose `desktop-app` step (`lag::P_DESKAPP`, `desktop_uefi::desktop_app_service`) runs `desktopbuild::service` and the battery's `status::poll`. 333 ms of `xhci` step = 333 ms of no build pass. KVMLAT2 (B576) moved the HDA walk out of the same span by `hda::bar_holds`; usbnet is the second tenant.

What this arc lands (video/desktopbuild.rs, line-neutral, x86 `wc` only):

- `[desktop] bar steps=stage:<n>,batt:<n>,build:<n>,paint:<n> bar_ms=<n> gap_max_ms=<n> passes=<n> worst=<step>:<ms> why=<pump-gap|own> held_starters=<a+b|none>`, once per build, only when `bar_ms > 100`. On flight 34 boot 1's timeline it reads `stage:≈333 … gap_max_ms≈333 … worst=stage:≈333 why=pump-gap`: the span is a pump gap, not the stage's own work.
- `desktopbuild::starter_holds(who)`: KVMLAT2's `bar_holds` as one call for every pump starter (hold while the desktop is not settled, bounded by 10 s from the first hold; `[desktop] starter after-bar who=<w> waited_ms=<n> why=<settled|timeout>` once per starter).

OWED (outside this arc's files — STOP per the head's item 3): the one call in `drivers/xhci/usbnet.rs:386`, after the services gate:

```
- if STATE.load(Ordering::Relaxed) != ST_CONFIGURED || crate::arch::ms() < RETRY_AT_MS.load(Ordering::Relaxed) || !crate::boot::services_gate("usbnet") {
+ if STATE.load(Ordering::Relaxed) != ST_CONFIGURED || crate::arch::ms() < RETRY_AT_MS.load(Ordering::Relaxed) || !crate::boot::services_gate("usbnet") || usbnet_bar_holds() {
```

with `usbnet_bar_holds()` a one-line fn folded onto an existing line of that file: `#[cfg(all(target_arch = "x86_64", feature = "wc"))] fn usbnet_bar_holds() -> bool { crate::video::desktopbuild::starter_holds("usbnet") } #[cfg(not(all(target_arch = "x86_64", feature = "wc")))] fn usbnet_bar_holds() -> bool { false }` (usbnet.rs is compiled into the Pi image: the fold must be line-neutral there). Expected after it, a boot-1-shaped login with the dongle plugged: `[desktop] built at=login ok bar_ms=<under 80> …`, then `[desktop] starter after-bar who=usbnet waited_ms=<n> why=settled`, then `[usbnet] bring-up try=0 of 3 … -> armed`.

## (B) FILETYPES: `masked_max_ms=183` — the step is text/html's per-type create transaction

Boot 2: `[filetypes] login=none stamp=stale` (boot 1's 15:04:01Z `[filetypes] stamp invalidated by=/apps/AETHER.ELF type=text/html written=1` — Aether's sighting declared text/html), then ONE object built: `[filetypes] type=text/html made=1 filled=0 blocks=344 cmds=135 ms=741 masked_ms=183 txs=5 group=1`, `built at=worker … created=1 … masked_max_ms=183`, `swap registry=live on=worker build_ms=1214 … masked_max_ms=183`. The swap itself is one `REG.swap` (fs/assoc.rs `build_stamped`) and masks nothing: the 183 is the per-object step. Every boot-1 object reads 41–88 ms masked with 0–96 blocks; text/html carries 344 blocks — the volume's first write of the boot (cold index/B-tree blocks) read INSIDE the object's create transaction, because every UnaFS operation runs whole inside one IRQ-masked `MOUNT` hold (`fs/unafs.rs:793` `with_unafs_attempt`, `without_interrupts`). `tests filetypes`' own rebuild (15:23:57Z, `created=0 filled=0 blocks_read=312 masked_max_ms=141`) has the same shape with no object made: the reads under the hold are the cost, not the publish.

OWED (outside this arc's files — the brief names `fs/filetype.rs`, but the swap and `seed_in` live in `fs/assoc.rs`; nothing in filetype.rs is on this path): in `fs/assoc.rs` `seed_in`, before an object's create, warm its path in short read-only transactions of their own (`mt.stat(TYPES_DIR)` and the `read_dir_kinds` already taken, plus a `mt.stat(&obj)` miss), so `create_files_batch` / `stamp_grouped` find the index blocks cached; and say the per-transaction masked span (`create=<ms> stamp=<ms>×<n>`) on the per-type row. Whether one cold B-tree insert is itself over 50 ms masked is the unafs commit's (B551's owed note), and moving reads out of `with_unafs`'s masked hold is unafs.rs's.

## Update — both OWED changes LANDED (second commit, scope widened by the seat)

- `drivers/xhci/usbnet.rs` `bringup_pending`: `… || !crate::boot::services_gate("usbnet") || usbnet_bar_holds()`, the cfg'd fn pair folded onto the `cfg_value` line (line count unchanged).
- `fs/assoc.rs` `seed_in`: before a missing object's create, `mt.stat(TYPES_DIR)` and `mt.stat(&obj)` as their own transactions (a yield after each); after the create, `mt.list_attrs(&obj)` before the stamp; the fill arm's `list_attrs` is followed by a yield. The per-type row gains `masked_pre=<n> masked_create=<n> masked_stamp_max=<n>`; `masked_ms` stays the max of the three (the FILETYPES bound reads it).
