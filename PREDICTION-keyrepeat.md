# PREDICTION — KEYREPEAT-X86 (branch `wt/keyrepeat`)

Written **before** the boot it describes. Falsifiable as stated: every claim below is either a
serial line an `awk` can find in the capture, or something the operator sees with their own eyes.

**Metal fact this answers** — Peter, Boot AL, rMBP bench: *"so far so good with keys except no key
repeat."*

**Change under test** — the shared host-side typematic tracker in `pal.rs` is now compiled on
`x86_64 + ehcihid`, fed at the HID report level from the EHCI keyboard decoder, disarmed on endpoint
death, and pumped once per x86 device-service pass.

**No new knob.** `ehcihid` is default-ON. Nothing needs to be set on the command line.

---

## 1. Operator-visible behaviour

Hold a key down at the shell for about two seconds.

| | before (Boot AL) | predicted (next boot) |
| --- | --- | --- |
| hold `x` at the shell | exactly **one** `x` | one `x`, a pause of ~**0.4 s**, then `x`s at ~**25/s** (one every 40 ms) for as long as it is held — ~40 characters in a 2 s hold |
| tap `x` (< 400 ms) | one `x` | **still exactly one `x`** — a tap must never repeat |
| arrows in `vug` | one step per press | held arrow steps continuously after the same ~0.4 s delay |
| backspace held on a shell line | one character erased | erases continuously at the same rate |
| release the key | — | repeat stops **immediately** (within one service pass, ~1 ms) — no run-on characters |
| hold `a`, then press `b` while still holding `a` | — | `b` repeats, `a` does not (newest-wins typematic) |

Everything GR21 proved on Boot AL must still hold, unchanged: caps lock toggles and lights, Ctrl+letter
works, SPACE both pauses **and** unpauses, WASD releases, `!` releases as `!`, no stuck keys, `kill`
works. **Any regression in that list refutes this arc even if repeat works.**

## 2. Serial lines

New, exactly once per boot, at the first synthesised repeat:

```
:: KEYREPEAT-X86: first synthesised repeat — key=0x78 'x' (host typematic armed on the EHCI keyboard) == witness ::
```

(`0x78 'x'` is whichever key is actually held first; the shape is what matters.)

Then, from shared code, once per hold that produced repeats — printed on the RELEASE:

```
[keystat] typematic hold end — key=0x78 repeats=40 re-arms=0 window=30000ms (boot: repeats=40 re-arms=0)
```

Predicted field values for a healthy ~2 s hold:

- `repeats=` roughly **(hold_ms − 400) / 40**, so ~40 for a 2 s hold. Anything in the 25–60 range
  for a hold the operator calls "about two seconds" is a pass.
- `re-arms=0`.
- **`window=30000`**, not `1000`. This keyboard does not idle re-report, so it must never earn the
  tight `LIVENESS_MS` window. `window=1000` on this hardware would mean the UVUG-9 evidence latch
  fired on something that is not streaming — see refute (d).

Must **NOT** appear:

- `[uvug9] typematic hold-max — …` at ordinary hold lengths (that is the 30 s backstop).
- `[keystat] typematic re-arm — …` on a healthy hold.

Find them with:

```
awk '/KEYREPEAT-X86|keystat|uvug9/' <serial log>
```

## 3. What a REFUTE looks like

- **(a) No repeat at all, and no `KEYREPEAT-X86` line.** The tracker was never armed. Either
  `typematic_note_report` is not being reached from the EHCI decoder (check that `EHCI-HID: KEY:`
  lines are present at all — if they are, the feed is wired but the arm failed), or `typematic_tick`
  is being called from a service loop this boot does not run. Distinguish with
  `awk '/SCHED-X86: usb-pump task dispatched/'` — if that line is absent, the boot took the inline
  BSP console loop, which is also wired, so absence of repeat on BOTH is a tracker fault, not a
  wiring one.
- **(b) Repeat starts but stops after roughly 10–15 characters, silently.** That is the UVUG-9
  symptom returning: the tight 1000 ms liveness window in force on a device that does not idle
  re-report. The `window=` field in the rollup convicts it directly — it will read `1000`.
- **(c) Repeat with NO initial delay** — a tap emits several characters. The tracker is not the
  source; the device is re-reporting, which would mean the premise in §18.2 of `usb_xhci.md` was
  wrong in the other direction. Look for `EHCI-HID: KEY:` lines arriving continuously during a hold.
- **(d) `window=1000` in the rollup on this machine.** The streaming verdict latched on a keyboard
  that does not stream — four consecutive reports with a byte-identical held-ascii set and no press
  edge. Survivable (repeat still runs for 1 s) but it is a real defect in the evidence gate as
  applied to this hardware and must be reported, not waved through.
- **(e) Repeat runs on after the key is released**, or characters keep arriving with nothing held.
  This is the P51 class. Should be impossible — release is learned from the report's held set, not
  from the event queue — and would mean the EHCI decoder is not calling `typematic_note_report` on
  the release report. It is bounded either way: the 30 s `HOLD_MAX_MS` backstop ends it and prints
  `[uvug9] typematic hold-max`.
- **(f) Input becomes sluggish or drops keys under a fast hold.** The backpressure guard should make
  this impossible (`typematic_tick` refuses to inject past a half-full `EVENT_QUEUE`). If it happens,
  check `[uvug10] evq` for a rising `drop` count.
- **(g) Any GR21 keyboard behaviour regresses** — stuck key, SPACE that pauses but will not unpause,
  a shifted symbol that will not release. The decoder's edge logic was not touched, so this would
  mean the feed block perturbed `prev_keys`/`cur_keys` handling. Refutes the arc outright.
- **(h) Repeat goes to the wrong window** — the operator holds a key while a ring-3 app has focus and
  the characters land in the shell (or vice versa). Repeats are pushed into `EVENT_QUEUE` and take
  the identical routing a real press takes, so this should be impossible; it would indicate the
  injection site sits on the wrong side of a drain.

## 4. aarch64 (Pi) — predicted UNCHANGED

The aarch64 `.text` is byte-identical before and after this change (measured; `usb_xhci.md` §18.5).
The Pi's typematic behaviour, and every `[keystat]`/`[uvug9]` line it emits, must be exactly as
before. Any change there refutes the "shared code, widened cfg" claim.

## 5. Gate results at the time of writing

- `./arroyo check` — **green, both arches**, zero warning delta against the pre-change baseline.
- `./arroyo kernel8` — builds; reproducible (same sources twice → same hash).
- Metal is the only thing that can decide the above. QEMU delivers no EHCI HID, so no repeat is ever
  synthesised there and the QEMU paths are byte-identical in behaviour.

## Ninth signature (review condition c)

- Repeat stops mid-hold with no release: check for an xHCI keyboard slot teardown line in the
  same window — `note_keyboard_detached` is cross-device, so an external keyboard bounce
  disarms the internal hold (safe direction). Distinct from the UVUG-9 latch (window=1000)
  and from HOLD_MAX_MS (30s, prints `[uvug9] typematic hold-max`).
- Operator expectation: held Enter re-executes the shell line at ~25/s (correct, Pi-equal, new here).
