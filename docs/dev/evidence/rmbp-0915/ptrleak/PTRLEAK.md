# PTRLEAK: the two owed cures of CURSORFLK (B186), made

Executor PTRLEAK, branch `exec-rmbp-ptrleak`, parent `94e90eae` (hw-rmbp), 2026-09-23. Ledger row
**rmbp-ledger B193**. Classified in `docs/dev/FIXTURE_FLAKES.md` §3b (now FIXED). Every run below is
`UNAOS_QEMU_FULL=1 ./arroyo test 120` in this worktree, sequential, one at a time. `runs.tsv` beside
this file has one row per run: load, tree, diff hash, rc, sidecar mode and completion, the mbench
verdict and the three wire lines. The captures are in the executor scratch
(`~/unaos-bench/scratch/rmbp-0915/ptrleak-logs/<run>-serial.log`). The lines that matter are quoted here.

## M1: QUIESCE. The input service stands down while PTRDEAD holds the ring

**Decision: a hold flag, not a tag on the event.** A tag would still let `x86_input_service` TAKE the
events. The fixture would keep losing its own queued input and would keep skipping its verdict, and
`Event` would widen for every consumer and for `pack_input`. The hold keeps the events where the
fixture pushed them. So the leak is gone and the leg judges instead of skipping.

- `pal.rs` (appended at the file tail): `FIXTURE_HOLDS_RING`, `fixture_ring_hold(bool)`,
  `next_event_unless_held()`. The flag is written and tested **under the `EVENT_QUEUE` lock**. So the
  service's stand-down test and its pop are one critical section, and the raise happens before the
  fixture's first push. A service pop either precedes the raise, in which case it took a pre-window
  event, or it follows the raise and sees the hold. The service runs on another core (`cpu=3`
  against `svc=Some(5)`), so a plain load followed by a pop would leave a window for a push to land in.
- `main.rs` `x86_input_service`: the drain pops through `next_event_unless_held` (a same-line fold).
- `arch/x86_64/syscall.rs` `ptrdead_selftest_body`: raise immediately before the first push, and
  release after the last drain and the release-edge retire (same-line folds). There is no early
  return between them. Every other drain (`next_event`, `pump_and_poll`, a focus discard) is
  untouched, so the SELFTEST-RACE SKIP arm stays.

**Proof: CURSORFLK's probe** (`../cursorflk/probe-apply.sh`, path-adjusted copy). It spins after the
push loop until `evq_pops()` moves, bounded at 200 ms. This forced the leak 2 of 2 in CURSORFLK.

| run | tree | wire | verdict |
| --- | --- | --- | --- |
| knob-off, probe + fix | `syscall.rs` ac7dbefe | `[cursorflk-probe] backlog handed to a foreign drain after 200ms pops=0`; `[ptrdead] backlog whole=true nodrop=true order=true pushed=192 entries=1 travel=(192,-192) folded=192 dropped=0 fpop12=0 fpop3=0 cpu=3 svc=Some(5) -> PASS`; no `[cursor] armed`; no `[cursor11]` | rc=0, MBENCH PASS 18/18, 0 forbidden, complete |
| knob-off, probe, hold mutated off (go-red) | 8aeda005 | `… after 4ms pops=1`; `[cursor] armed x=832 y=208`; `[ptrdead] order detail: got=[Some(Mouse { x: 2, y: 0 }), None, None, None] fpop=2 fpush=0`; `[ptrdead] backlog whole=skip nodrop=skip order=skip … fpop12=1 fpop3=2 … -> PASS`; `[cursor11] compose-through scope=desk passes=0 bracketed=0 px_deferred=0 px_installed=0 px_redrawn=0 flicker_frames=5 px_absorbed=0 absorb_refused=0 -> FLICKER` | **rc=1, MBENCH FAIL, 2 forbidden (`FORBID* -> FLICKER`)**, complete |
| wc, probe + fix (control) | ac7dbefe | `… after 200ms pops=0`; `[ptrdead] … fpop12=0 fpop3=0 … -> PASS`; `[cursor] armed x=427 y=283` (the wc fixtures' own pointer, not centre + travel); `[cursor11] scope=desk … flicker_frames=0 px_absorbed=486 absorb_refused=0 -> THROUGH` | rc=0, MBENCH PASS 18/18, complete |
| wc, probe, hold off (control) | 8aeda005 | `… after 5ms pops=1`; `[cursor] armed x=832 y=208`; `[cursor11] scope=desk … flicker_frames=0 px_absorbed=486 absorb_refused=0 -> THROUGH` | rc=0, MBENCH PASS 18/18, complete |

The go-red's `fpop3=2` goes beyond CURSORFLK's reading. There the service took leg 3's `Mouse` **and
its `Button(1)`**. A synthetic primary press reached the product, not only synthetic motion. The hold
spans leg 3 as well, and the fix row's `fpop3=0` is that span working.

The first go-red attempt (`m1-knoboff-probe-gored`, load 24) ended `completion=truncated` before
zeolite. It is not quoted as a verdict (a truncated run is never a pass or a fail). Its wire already
carried `[cursor] armed x=832 y=208` and `flicker_frames=3 … -> FLICKER`, and the complete re-run is
the row above.

Revert: `probe-on-fix.diff` (sha256 17c7fd25…0903) and `gored-mutation.diff` (sha256 0f7a786a…7f28)
were taken back out. `syscall.rs` returned to sha256 768d14e9…9815, the committed M1 content.

## M2: THE FORBID's MEANING. Cure (a), the knob-off desktop present withholds the arrow

**Decision: (a).** PTRREPAINT's withhold was `wc`-only for one stated reason, the occluder array's
capacity. `wm::occluders` takes exactly `[_; MAX_WINDOWS]`, and only the SHELLDESK arm staged it
through its own `wins`. The cost on the front buffer is bounded, and every flown `wc` image already
pays it. It is one box test per present, plus a per-pixel absorb over the sprite box (a few hundred
pixels) only on presents whose damage meets the arrow. (b) would have kept a real flicker and renamed
it.

- `video/screen.rs`: `DESK_SPRITE_OCC = cfg!(target_arch = "x86_64")`, where it was
  `all(x86_64, wc)`. The remaining term is the hardware one (front-buffer arrow). The
  `const _: () = assert!(!DESK_SPRITE_OCC || SPRITE_OWNS_PAINT)` still holds. The no-registry arm now
  fills through `desk_window_occluders` (file tail). On aarch64 that is the WC-I call on the same
  array, inlined. On knob-off x86 it stages through `wins` and leaves the sprite's slot.
- `video/cursor.rs`: the `flicker_frames` rustdoc now says the counter is the invariant on every x86
  build.
- `-> FLICKER` stays unique: `scorer-token-uniqueness.sh --verb compose-through '-> FLICKER'` gives
  `OK 1 emitter(s) verbs=compose-through unaos/crates/kernel/src/video/cursor.rs:1504`, rc=0, and the
  go-red `--verb ptrdead` gives rc=1. `mbench.py`'s `DEFAULT_FORBIDS` is unchanged.

**Proof.** With M1 in place a knob-off QEMU boot has no arrow, so a real arrow was forced by the probe
with M1's hold mutated off (scratch):

| run | tree | wire | verdict |
| --- | --- | --- | --- |
| knob-off, forced arrow, fix | screen.rs db7b013e | `[cursor] armed x=641 y=399`; `[cursor11] compose-through scope=desk passes=0 bracketed=0 px_deferred=0 px_installed=0 px_redrawn=0 flicker_frames=0 px_absorbed=405 absorb_refused=0 -> UNWITNESSED` | rc=0, MBENCH PASS 18/18, complete |
| same, `DESK_SPRITE_OCC` mutated back to `all(x86_64, wc)` (go-red) | eb7366df | `[cursor] armed x=832 y=208`; `[cursor11] … flicker_frames=4 px_absorbed=0 absorb_refused=0 -> FLICKER` | **rc=1, MBENCH FAIL, 2 forbidden**, complete |

`px_absorbed=405` is the same number CURSORFLK's `wc` control read (`p1-wc-r1`). The presents met the
arrow, and the knob-off build now withholds it the way `wc` does. `UNWITNESSED` is the ladder's honest
rung for a build with no composite passes (`passes=0 bracketed=0`), and it is not a FORBID.
`m2-gored-mutation.diff` (sha256 0ca27981…fb6c) was reverted. `screen.rs` returned to db7b013e…d1ba and
`syscall.rs` to 768d14e9…9815, and the working diff was byte-identical to the pre-run snapshot.

## M3: the rate, before and after

| population | leaks (fpop12 >= 1) | `[cursor] armed` on knob-off | `-> FLICKER` |
| --- | --- | --- | --- |
| before, peer population 2026-09-15..23 (CURSORFLK) | 4 in 68 knob-off boots | 4 (all 4 leaks) | **2 in 68 (about 3%)** |
| before, CURSORFLK's own 8 runs at `98fd8e66` | 0 in 8 | 0 | 0 |
| before, forced by the probe, hold absent or mutated off (CURSORFLK: 2 knob-off + 1 wc; this arc: 3 knob-off, one of them truncated, + 1 wc) | 7 in 7 | 5 in 5 knob-off | 5 in 5 knob-off, 0 in 2 wc |
| **after, forced by the probe** (knob-off and wc) | **0 in 2** (`pops=0` after the full 200 ms both times) | 0 | 0 |
| **after, 8 natural knob-off runs at `a99e9ebc`** (load 10 to 16, all complete, rc=0) | **0 in 8** | **0** | **0** |
| after, forced arrow with the M1 hold off (M2's proof) | 1 (scratch mutation) | 1 | **0** (`px_absorbed=405`) |

Every `[ptrdead]` and `[cursor]` line of the 8 natural knob-off runs (`m3-knoboff-r1` … `r8`) is
byte-identical:

```
[ptrdead] backlog whole=true nodrop=true order=true pushed=192 entries=1 travel=(192,-192) folded=192 dropped=0 fpop12=0 fpop3=0 cpu=3 svc=Some(5) -> PASS
```

There was no `[cursor] armed` line in any of the eight, and no `[cursor11]` line. The final `wc` run
(`m3-wc-final`) printed the same `[ptrdead]` line, `[cursor] armed x=427 y=283` (its own fixtures),
and `flicker_frames=0 px_absorbed=486 … -> THROUGH`.

The natural rate is too low for 8 runs to separate before from after (CURSORFLK's 8 were also 0).
The discriminating evidence is therefore the probe: the same deterministic stimulus leaked 7 of 7
before and 0 of 2 after. What 8 runs do show is the leg's new state: `whole=true nodrop=true
order=true` judged on every boot, with no `skip`.

## Flight 13: what to read on the rMBP

1. **`[cursor] armed` must come only after a real HID report.** The discriminator is `[deadman] …
   hid=N` (`deadman.rs:45`: HID IN-token completions in the last second, stamped at the qTD
   retirement before any `EVENT_QUEUE` push). An armed line with every preceding `[deadman]` reading
   `hid=0`, while `pmp=` is non-zero (so somebody polled), is a pointer that did not come from the
   hardware. On flight 12 the only `[cursor] armed x=961 y=617` came after `hid=0 … in=0/0/0` (the
   trackpad endpoint had halted at 32422 ms). That was this leak. On boot 13 that shape must not
   recur.
2. The `[ptrdead] backlog` line reads `fpop12=0 fpop3=0` and `whole=true nodrop=true order=true`.
   Any `skip` names a drain other than the input service. That is new and reportable.
3. With a live trackpad, `[cursor11] … flicker_frames=0` on every line of the boot, on the wc flight
   image as before. On a knob-off x86 image it is now the invariant too.
