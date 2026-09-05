# A17 — the second Print Screen press: an empty `SCREEN1.PNG` and no verdict (PRTSCR2)

Executor A17, seat orin 14, track `hw-jetson`, base `6cc8de8c`. Ledger row: `docs/dev/OS/orin-ledger.md` A17.
Ruling: `docs/dev/RULINGS.md` R17 ("hit the button twice and the 2nd file is empty").

Peter's requirement: the second press either produces a valid PNG or a NAMED refusal on the wire.

## 1. The wire, measured

The repo excerpt `docs/dev/evidence/orin13/render3b-boot1.log` ends BEFORE the presses (its FLIGHT-RESULT
row 6 says "pending Peter's press at scoring time"). The presses are in the butler's raw capture,
`~/unaos-bench/capture/line-acm0/raw.log`, lines 109385-109395 — the last eleven lines of the file:

```
awk 'NR>=109384 {print NR": "$0}' ~/unaos-bench/capture/line-acm0/raw.log | cat -v
109384: [orinrender] census passes=89352069 presents=261 win=0 declined=1 -> RENDER-LIVE
109385: :: PRTSCR: PrintScreen (HID 0x46) down on xHCI -> capture armed ::
109386: :: PSRC: psrc=global reason=fallback-order verdict=unbuilt boot_serial=0x00000000 handles=global=present sdhc=unbuilt ::
109387: :: PRTSCR: PrintScreen (HID 0x46) down on xHCI -> capture armed ::
109388: :: PRTSCR: SCREEN0.PNG 1920x1200 6913793 bytes -> OK ::
109389: [serialrx] rx=3 (+0) polls=89411663 refused=0 lsr0=0x00000200 -> RX-LIVE (...)
109390: :: SCHED: load c0=98%/f=0ms c1=0%/f=1ms ... ::
109391: [pulse5] live c0=0ms ... folds=6555
109392: [pstrip] rollup samples=30 redraws=3 skipped=27 srcdelta=3 rate=0.1/s srate=1.9/s gapmax=7894ms lat_max_ms=7894 period=250ms decay=1000ms
109393: [wc-w] rollup presents=264 ... -> WIDENED
109394: [orinrender] census passes=89411659 presents=262 win=0 declined=1 -> RENDER-LIVE
109395: ^@
```

What each line proves:

| fact | evidence |
|---|---|
| The first capture ran between the two `armed` lines, not after them. | 109386 `PSRC` is `mount_program_source`'s one-shot verdict — the first mount, i.e. `capture()` step 2 — and it sits between armed #1 and armed #2. |
| The first capture stalled the pump for **7.9 s**. | 109392 `[pstrip] gapmax=7894ms lat_max_ms=7894`: the strip sampler on the same task missed 7.89 s. The `[orinrender]` census advanced only 59,590 passes in that window (109384 -> 109394) against ~338,000 per line before it — core 0 was the capture's (`SCHED: load c0=98%`). |
| The second press was decoded INSIDE the first capture's storage write. | The only tegra pump that drives the xHCI decoder is `jd2_console_pump` (main.rs:2801), and it was blocked in `capture()`. `drivers/xhci/mod.rs:3654-3668`: `drain_event_ring_once` is "the SINGLE entry point for consuming events — used by both `poll_events()` and the synchronous BOT pump", and it dispatches through `handle_event_trb` (:3701), whose keyboard arm prints `capture armed` and calls `prtscr::request()` at :4931-4934. So the keyboard's interrupt-endpoint completion was dispatched by the BOT write's own event drain, `request()` set `PENDING` (which `service()` had cleared at prtscr.rs:132 before starting), and armed #2 printed mid-write. |
| The second capture STARTED. | After 109388 the pump ran exactly one more sweep round (109389-109394), then went silent. The next sweep (~250 ms later; main.rs:3012) found `PENDING` set and entered `capture()` again. The card afterwards holds `SCREEN1.PNG`: `create_in_dir` (prtscr.rs:370) landed, so `capture()` step 5 was reached. |
| The boot ended **inside** the second capture, ~0.6 s before its verdict. | 109395 is a lone NUL — in this raw.log every earlier NUL-only line (52793, 61330, 81317, 81321, 87866) is immediately followed by `=== line-butler released /dev/ttyACM0 ===`: it is the line-drop artefact of the board losing power. Timestamps: `orin.log` (the butler's per-board copy) last write `2026-09-05 12:41:55.477` = line 109394; `raw.log` last write `12:42:02.755` = the NUL. **7.28 s** elapsed between the last census line and power-off. Capture #2 began ≤0.25 s after 109394 and needed ~7.9 s (the measured duration of capture #1, same geometry, same medium); at power-off it was ~7.0 s in. |
| A 0-byte entry is the interrupted-write signature, by design. | `fs/fat.rs:3020-3032` (`write_grow`, "SAFE ORDER"): the chain is allocated and written first and the directory size is published LAST — "A crash before step 4 leaves the OLD (smaller) size on disk". The entry `create_in_dir` made has `size = 0` (fat.rs:3098). Power-off during steps 2-3 leaves exactly `SCREEN1.PNG` at 0 bytes with the clusters it had claimed. |

Rule-out of the alternatives the brief listed:

- **A refusal whose `report()` prints nothing** — none exists. `service()` (prtscr.rs:126-149 at base) prints on
  `Ok` (:136) and calls `why.report()` on every `Err` (:146); `report()` (:183-222) has an arm with a
  `serial_println!` for all eight variants. `capture()` has no exit that is not one of those two.
- **`Refusal::Short` leaving a partial file silently** — `Short` prints (:217), and it is in practice
  unreachable: `write_grow` returns `Err(BadChain)` rather than a short count when `write_span` comes up
  short (fat.rs:3088-3095), so a short write surfaces as `Fat("write", BadChain)` and prints too.
- **`write_grow` hanging in USB BOT** — not needed to explain the wire (the power-off window is inside the
  measured capture duration) and not indicated: the BOT pump is budgeted (`hw_wait_budget`, aarch64
  `mod.rs:351`), `busy_retry` (prtscr.rs:276) is bounded to `BUSY_ATTEMPTS = 64` or one budget, and a
  BOT timeout returns `Io` -> `Fat("write", Io)` -> a printed line. No such line exists because the boot
  ended first.
- **A swallowed panic** — the `#[panic_handler]` (main.rs:6936-6950) enters serial panic mode and prints
  `=== KERNEL PANIC ===` lock-free; nothing of the kind is on the wire.
- **`next_free_name` clobbering / both presses targeting one name** — the two captures were sequential
  (one task, one pump; see above), so the second `next_free_name` (prtscr.rs:300) saw `SCREEN0.PNG` with
  its final size and took `SCREEN1.PNG`. Consistent with the card: `SCREEN0.PNG` 6,913,793 bytes valid,
  `SCREEN1.PNG` 0 bytes. No entry was renamed or overwritten.

## 2. Answers to the brief's three questions

1. **Is `service()` re-entrant on the Orin?** No. Its only tegra caller is the `jd2_console_pump` sweep
   (main.rs:3014, `#[cfg(feature = "holocron")]`); the three other call sites are unreachable on tegra
   (:1199 and :1689 are in `kernel_main` below `tegra_early_stop`'s divergence at :190, :5935 is
   `x86_usb_pump`, `#[cfg(target_arch = "x86_64")]`). There is no lock, and there was none needed for the
   key path: `service()` clears `PENDING` before `capture()` (:132), the press that lands mid-capture
   re-arms it from inside the BOT event drain, and the next sweep runs the second capture. The
   unguarded concurrency that DOES exist is key-vs-verb: `shell.rs:3546` calls `capture()` from the
   console task; two callers past `next_free_name` at once would both choose the same index and
   `create_in_dir` "does not de-duplicate" (fat.rs:3104).
2. **What leaves a 0-byte entry with no verdict?** Only an exit that never returns to `service()`: the
   boot ending inside the write (this case, measured) or a hang. Every `return` in `capture()` prints.
3. **Did `next_free_name` see `SCREEN0.PNG` as taken?** Yes — sequentially, after the first write's size
   was published. Had the captures been concurrent, the entry would still be visible (it exists at size 0
   from `create_in_dir` on), so the index would not have been reused; the real concurrent hazard is two
   callers both BEFORE `create_in_dir`, which the fix closes.

## 3. The fix (PRTSCR2) — `unaos/crates/kernel/src/video/prtscr.rs`

Three small changes, all in `prtscr.rs`; no other file changes. (Lane: rmbp's; the source change is
carried as `A17-prtscr.patch` beside this document until the grant is in hand — see §5.)

1. **The wire names the file before the medium can hold it.** In `capture_inner`, after `next_free_name`
   and before a pixel is read (prtscr.rs:422-426 patched):
   `:: PRTSCR: SCREEN1.PNG 1920x1200 -> capturing (N bytes reserved; the verdict line follows — a boot cut before it leaves the entry at 0 bytes) ::`.
   With it, every capture on the wire ends in exactly one of `-> OK`, a `— capture skipped` refusal, or
   nothing after `-> capturing` — and the third is now a NAMED state that says what the 0-byte file is.
2. **One capture at a time — the `IN_FLIGHT` door** (prtscr.rs:133, :382-391). `capture()` takes it by
   compare-exchange and releases it at one site after `capture_inner` returns, so every exit path
   releases. A second caller gets `Refusal::InFlight` (:231), printed as
   `:: PRTSCR: refused — capture in flight (...) ::` (report) / `screenshot: a capture is already in
   flight — retry after its verdict` (verb sentence).
3. **`service()` defers rather than refuses** (prtscr.rs:188-193). On `InFlight` it re-arms `PENDING`
   and prints the refusal once per episode (`DEFERRED_SAID`, :138), so a key press during a verb capture
   runs on the first sweep after the door opens instead of being counted as a refusal.

Not changed, on purpose: the clear-before-capture order in `service()` (it is what turns a mid-capture
press into a second capture rather than a lost one), and the create -> write -> publish-size order in
`write_grow` (it is the crash-consistent order; the 0-byte file is its honest residue). Writing the
data before creating the entry would need `fs/fat.rs` to grow a "chain without an entry" path — out
of lane and larger than the requirement.

Cost: one atomic compare-exchange and one serial line per capture. `prtscr.rs` is compiled into every
image unconditionally (`video/mod.rs` declares it; `service()` on tegra stays behind `holocron`), so
this is a code change in all images, not a knob-off byte-identity question — no `main.rs` line moves.

## 4. Expected wire on render4 (two presses, board left on)

```
:: PRTSCR: PrintScreen (HID 0x46) down on xHCI -> capture armed ::
:: PSRC: psrc=global ... ::                                        (first mount only)
:: PRTSCR: SCREEN0.PNG 1920x1200 -> capturing (N bytes reserved; ...) ::
:: PRTSCR: PrintScreen (HID 0x46) down on xHCI -> capture armed ::   (decoded inside the write; may land before or after the line above)
:: PRTSCR: SCREEN0.PNG 1920x1200 6913793 bytes -> OK ::
[serialrx] ... / [pstrip] rollup ... gapmax=~7900ms ...              (one sweep round)
:: PRTSCR: SCREEN1.PNG 1920x1200 -> capturing (N bytes reserved; ...) ::
:: PRTSCR: SCREEN1.PNG 1920x1200 <bytes> -> OK ::
```

Card: `SCREEN0.PNG` and `SCREEN1.PNG`, both valid PNGs. If the board is powered off before the second
`-> OK`, the wire ends with `SCREEN1.PNG ... -> capturing` and the card holds `SCREEN1.PNG` at 0
bytes — the named state, not a silence. If a `screenshot` verb is in flight when the key is pressed:
`:: PRTSCR: refused — capture in flight (...) ::` once, then the key capture's own `-> capturing` and
`-> OK` after the verb's verdict. On render4 the operator should wait for the second `-> OK` (about
8 s after the second `-> capturing`) before pulling power.

Scoring: `awk '/PRTSCR/' <log>` — count `-> capturing` lines against `-> OK` + `capture skipped` +
`refused` lines; a `capturing` without a partner is a boot cut mid-capture, and the card's 0-byte file
must carry that name.

## 5. Gate and lane

Run with the patch APPLIED in the A17 worktree at `6cc8de8c` (logs: `~/unaos-bench/scratch/orin14/a17/`):

| command | exit | evidence |
|---|---|---|
| `./arroyo check` | 0 | 61 `Finished` lines, no `error[`; `check.log` |
| `UNAOS_WC=1 ./arroyo test 150` | 0 | banner `kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`; 0 `KERNEL PANIC`; `test-wc.log` |
| `./arroyo test-arm 60` | 0 | `aarch64 test complete`; 0 `KERNEL PANIC`; `test-arm.log` |
| `UNAOS_PRTSCRST=1 ./arroyo test-fat sf` | 0 | the patched path end to end (`awk '/PRTSCR/' test-fat.log`): `:: PRTSCR: SCREEN0.PNG 1280x800 -> capturing (3073098 bytes reserved; ...) ::` then `:: PRTSCR: SCREEN0.PNG 1280x800 3073098 bytes -> OK ::` then `:: PRTSCR-ST: SCREEN0.PNG on the medium — 3073098 bytes, PNG signature OK, IHDR 1280x800 depth 8 colour 2 non-interlaced, IEND OK -> PASS ::` |

`prtscr.rs` is the rmbp lane; per
the executor brief the source change is committed as `docs/dev/evidence/orin14/A17-prtscr.patch`
(`git diff` against `6cc8de8c`) with the source reverted, until the seat relays `GRANT prtscr.rs`.
Apply: `git apply docs/dev/evidence/orin14/A17-prtscr.patch` at `6cc8de8c` or later.
