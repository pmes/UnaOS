# NOTICEFIX (rmbp-ledger B491): the toast's timeout-close, on the compositor's clock

**Finding (f26-boot3.log, awk `[notify]`/`[notice]`).** The fixture's card was posted at 14:55:32Z and
`[notify] closed by=timeout title=Fixture notice` printed at 14:55:35Z, so the live timer works. Nothing
starved it: SVCLATCH (B462) does not gate `notify::service`, which runs on every storage pass. The fixture
was wrong, and the card clock had a seam:
1. `toast::expire_now` set `TQ.until` only when `TQ.cur` was Some, and NOTIFY (B418) never sets `cur`. So
   the fixture's "expire" did nothing and `timeout_close=false`.
2. The fixture held the toast queue but not NOTIFY's stack. Its card went onto the REAL glass (`show win=3`),
   into the real ring and the unread badge, and lived on until the real 3 s ran out.
3. `notify::show_card` stamped `until` from `crate::arch::ms()` and ignored the pass's own `now`. A pass
   driven on the fixture clock (`toast_fixture`: `pass(5000)`, then `pass(5000+CARD_MS+1)`) never reached
   that `until`, which is why the same boot printed `:: DIALOG: … toast=FAIL`.

**Seam.** Kernel — wm (NOTIFY is the one card store; the toast queue is only its inbound). There is no new
store, no new file and no new knob.

**Milestones.**
- M1: `show_card` takes the pass's `now` and records `at`. The timeout close prints
  `[notify] toast close reason=timeout after_ms=<n>` (n = now − at, on the compositor's clock `crate::arch::ms()`).
- M2: the notice fixture holds NOTIFY's stack headless (`notify::fixture_hold`/`fixture_restore`).
  `toast::expire_now` hands the expiry to NOTIFY (`notify::expire_now`: one pass at the latest card's
  `until`+1), so the real pass closes the card by `timeout`.

**Witness (next flight, `tests notice`).** `[notify] toast close reason=timeout after_ms=3001`,
`[notice] fixture up=true typed=27/27 timeout_close=true`, `:: NOTICE: typed_through=ok modal=none flash=none -> PASS ::`,
`:: DIALOG: … toast=ok -> PASS ::`. A live toast prints `[notify] toast close reason=timeout after_ms=<3000..3250>`
(the closing pass's lag past 3000 is the storage pass cadence).

**Owed.** A metal boot to read the lines above.
