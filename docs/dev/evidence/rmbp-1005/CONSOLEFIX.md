# CONSOLEFIX (B365) — a fixture never sets the live clock, a notice never takes the console's keys, a password never reaches the wire

CHARTER: Midden — shared-core (the redaction table is `midden_core::secret_from`/`redact`, both rings) · Kernel — kernel-by-ruling
(R65) for the one new kernel file `unaos/crates/kernel/src/pwwire.rs`. Branch `exec-rmbp-consolefix`, cut from 28996f11 (boot-24
integration). Stays out of INSTALLBARE's files (login/firstboot/service starters in `fs/users.rs`): the notice surface's half of
`video/login.rs` and the console are this arc's.

## Findings (flight 22, `rmbp-0915/flight22/f22-boots.log`, awk)
- **CLOCKCANNED.** `[05:13:37Z] … reject LI=3 alarm => malformed PASS` then `[15:30:45Z] :: [sntp-x86] canned reply sets clock =>
  2026-07-22T15:30:45Z PASS ::`: `sntp_x86_gate`'s 0x10 leg called `clock::set_anchor` on the LIVE anchor. The RTC (R75) had
  anchored the clock first, so `had_real_anchor` was true and the "defensive" branch LEFT the canned value in place (`pre-existing
  anchor left in place`). Boot 2 came back at 05:57 (the RTC was never written). The gate line also had no `-> PASS` arrow, so the
  tally read `skipped=[sntp]`. NETCLOCK said PASS with `[sntp] target=0.0.0.0 from=none` and `SOCK-5: … dhcpv4 no offer — static
  fallback stands 10.0.2.15/24` on every try: it measured the poll budget on a stack with no lease and never asked whether time came.
- **NOTICEKEYS.** A notice is the login screen's `State::Alert`. `login::is_open()` counts `Alert`, so `users::screen_up()` /
  `secret_input()` read true, every route's `screen_key` handed the key to `consume_key`, and `open_as` pinned the row modal
  (`wm::set_modal_top`) and suspended the console's present. 32 `EHCI-HID: KEY` → `KEY withheld` pairs; `tests selfbuild5`, `tests
  lumen`, `holocron put …`, `lumen` vanished. The "flash": the holocron client's answer is `notice_post`ed (queue only); the queue
  opens at the head of `consume_key`, i.e. on the NEXT typed byte (`[16:04:18Z] USB-DEBUG: KEY 0x68 'h'` then `NOTICE-OPEN:
  title=holocron`), the rest of the line went into the alert and the line's own `\r` dismissed it.
- **PWONWIRE.** `[16:02:51Z] :: [midden] cmd="holocron init qwerty peter" -> Exec holocron.elf ::` and `[16:09:45Z] … cmd="holocron
  put vein claude.api_key sk-ant-flight22-bench-key"`: `shell.rs` prints `cmd_line.trim()` raw. Worse, and not in the row: the
  SAME password went out a byte at a time on the key echoes — `EHCI-HID: KEY: 'q'` (unconditional in `drivers/ehci/mod.rs`, even on
  the login screen: lines 881/884/888 are the login password) and `USB-DEBUG: KEY 0x71 'q'` (gated only by the login screen).

## The seam
- M1: one path from an SNTP datagram to a clock, `smolnet::sntp_apply(pkt, &mut dyn clock::AnchorSink)`. The live client passes
  `clock::LiveClock`; the gate passes a `clock::FixtureClock` (same `UnixAnchor` arithmetic, a stack value). The gate snapshots
  `clock::raw_anchor()` before and after and FAILS if the live clock moved. `witness_clear_anchor` (the cleanup that could not clean
  an RTC-anchored boot) is deleted: nothing to clean.
- M2: a notice raised over the SESSION (`ALERT_PREV == 0`) is NON-MODAL: `is_open()` no longer counts it (so `screen_up`,
  `secret_input`, every route's `screen_key` and the press router pass through), `open_as` neither pins it modal nor suspends the
  console, `consume_key` hands every key on, a press on its OK (or anywhere on its row) is its own, and it closes itself after
  `NOTICE_TIMEOUT_MS` (`notice_service`, beside `users::service` on the three storage passes) or on a click. A notice over the login
  form / setter keeps the form's modality (that screen owns the keys anyway).
- M3: the redaction table lives in `midden_core` (both rings): `secret_from(line)` = the byte where a secret begins (`holocron
  init|unlock|put` keep 2 words, `login`/`adduser` keep 2, `passwd` keeps 1), `redact(line)` = `holocron init ***`. The kernel's
  tracer is one function `pwwire::trace` used by all three `[midden] cmd=` arms. The console publishes `secret_from(current_input)`
  after every edit (`pwwire::note_line`), and the three raw-key echoes (EHCI-HID KEY/KEYUP, USB-DEBUG, serialdoor) withhold while
  it is true (and EHCI now also withholds under the login screen, LOGIN13's own rule). A wire tap (`pwwire::wire_note`, from
  `serial_line::line_note`) counts lines and remembers the line number of the last line that carried the fixture password.

## Milestones
- M1 `tests sntp` on a fixture clock; `tests netclock` needs a real lease (`SKIP reason=no-lease`).
- M2 notices non-modal; `tests notice`.
- M3 tracer + key-echo redaction, KAT (`cargo test -p midden_core` + the kernel KAT inside `tests pwwire`), `tests pwwire`.
- M3b (coordinator, GLASSLAG's neighbour): every per-key trace names a CLASS, never a value or scancode
  (`pwwire::key_class`: printable/enter/backspace/tab/esc/control/nav) — EHCI/xHCI `KEY:`/`KEYUP`, `[hidkeys] keyup`,
  `USB-DEBUG: KEY`, `[serialdoor] key=`, `[quarry] key_route key=` (silent altogether while `withhold()`), `[keystat]`,
  `KEYREPEAT-X86`, tegra `JD2`/`JB2b`. `jetson-jd5.spec`'s `REQUIRE xHCI: KEY:` still matches (prefix kept).
- M4 this doc's witness section.

## Witness (what a metal boot prints)
- `:: [sntp-x86] canned reply anchors the FIXTURE clock => 2026-07-22T15:30:45Z PASS ::`
- `:: [sntp-x86] live clock untouched source=<rtc|unset|…> before=<unix|none> after=<unix|none> => PASS ::`
- `:: SNTP-X86-GATE: x86 sntp client battery PASS [w=0x3f] (…|fixture-clock|live-untouched) -> PASS ::`
- `:: NETCLOCK: … -> SKIP reason=no-lease ::` on the bench (no DHCP), `… lease=<ip> gw=<ip> clock=<src> -> PASS ::` with one.
- `:: NOTICE: typed_through=ok modal=none flash=none -> PASS ::`
- `:: [midden] cmd="holocron init ***" -> Exec holocron.elf ::`
- `:: PWWIRE: kat=17/17 trace=redacted keytrace=class keys_withheld=21/21 lines=<n> window=2000 hits=0 -> PASS ::`
- `EHCI-HID: KEY: class=printable` / `EHCI-HID: KEY withheld (a secret is being typed)` / `[quarry] key_route key=enter focus=1 took=1`
- `:: NOTICE-OPEN: title=Program stopped lines=1 modal=none -> PASS ::`, `[notice] closed by=timeout|click title=…`

## Compile legs (direct, inline)
- x86 metal shape + selfdiag,ahciroot,btc,lumen (+ arroyo's default ehcihid,kbdwit,smolnet): exit 0; same + usbdebug: exit 0.
- aarch64 `login,loginst,virt_el0,lumen`: exit 0. aarch64 `tegra` (the JD2/JB2b edits): exit 0. `cargo test -p midden_core`: 26 passed (2 new).

## Owed
- The holocron CLIENT still raises its answer as a notice (`user-holocron`, HOLOCRON2's file): non-modal now, so harmless, but the
  row's "a program's start line is a console line" would move that answer to the console — a seat call (see report).
- `history` keeps the typed line (glass only, never the wire); arm `sntp6`'s fixture still plants and restores the live anchor.
