# CRAWLE — wave36 crawl E findings: the 2026-10-21 folds

Range 3340e13c..6f51ca22 (29 commits: HELPVERBDOC+HELPCLIP, PREFSCLIENTKEYS, WCPAR2, EHCIWAKE, STOPENDPOINT, TICKLESSENV,
RING3SCROLL, ORINPCIEROWS, AVRCPCT, SINKALLDOCS, PATHNS, SESSIONGENPOLL, the seat's hand unions and 6f51ca22). Every
merge was read with `git show --remerge-diff`, and every line a branch added was read once. Line numbers are from 6f51ca22.
The fixes are built on 28d01e8c, the range end after the fast-forward to the track tip; the branch's parent is f2484026.
Not run on QEMU (a review crawl: the compile legs and `./arroyo gates` only).

Seat unions: tests.rs usbsuspend (both tails kept; the verdict and print union is correct), power.md BP-power-7
(correct; the §4/§5 text around it is stale, A7), audio.md BP-audio-16 (AVRCPCT's clause is kept), look.rs/locale.rs
`service` (correct on the desktop families; A1, G3), power.rs header (correct). The one wrong union was the WCPAR2
register token (R1).

## Disposition
- R1: FIXED BY THE SEAT BEFORE THE CRAWL LANDED (132e60b3, named by the seat's own Orin leg); nothing built here.
- R2: built (drivers/bt/avctp.rs `Ct::on_rsp`; `legs_ct` adds the `quantized` leg).
- R3: built (user-lumen/src/main.rs `render`: the gutter band is skipped only while `sv_live`).
- A4: IN FLIGHT, being removed by MIXERPUMP (`level_changed` replaces the pump); nothing built here.
- A7, A8, A9: built (power.md §4/§5 and BP-power-7; input.md §5; tests.rs `prefsclientkeys_leg`: unset = the default,
  a failed GET = `skip`).
- A1: owed row on security.md §5. A2 and A3: owed rows on power.md §5.
- A5, A6, G1-G5: recorded here only.

## RED (3)
R1  tests.rs:192 — `register("wcpar", wcpar_selftest)` carries no cfg (WCPAR2 fold ea754d35); the fn is
    `#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc", feature = "facet"))]` (tests.rs:5661); the
    DONE2 block is witness + (x86 wc | aarch64 desktop_firmware). The Orin line (witness + desktop_firmware) does not
    compile: E0425 `cannot find value wcpar_selftest` (the seat's own /root/unaos-bench/scratch/cloud/wave36/seat-orin-6f51ca22.log,
    the only error). Any x86 wc witness image without `facet` breaks the same way.
    FIX (local, line-neutral): one token `#[cfg(all(target_arch = "x86_64", feature = "wc", feature = "facet"))]` before
    `register("wcpar"` on line 192 (the `veilcost` neighbour's shape; before the line's first `//`).
R2  drivers/bt/avctp.rs `Ct::on_rsp` (:473; the ACCEPTED arm :501-504) with `Watch::step` (:553; the push test :576): on
    ACCEPTED the CT stores the headset's reported volume in `pushed`; `step` pushes again whenever `pushed != master`.
    AVRCP 1.6 §6.13.2: the ACCEPTED response carries the volume ACTUALLY set — a headset with coarser steps answers a
    rounded value. Input: master level 9 -> `abs_of(9)` = 71 sent, headset answers ACCEPTED 72 -> `pushed`=72 != 71 ->
    next BTHID pass sends SetAbsoluteVolume 71 again -> ACCEPTED 72 ... forever while the master sits there: an
    endless BT command ping-pong and two serial lines (`ct cmd=set-volume … sent`, `… accepted volume=72`) per round trip.
    FIX (local, one fn): in `on_rsp`'s `CtEv::Accepted(_)` arm set `carries = true` only and leave `pushed` as the value
    `step` recorded when it sent (the master's abs); the reported value still goes to the wire line. `legs_ct` (same
    file) gains one leg: an ACCEPTED carrying a different value -> the next `step` at the same master pushes nothing
    (`Err("quantized")`). No other file; BP-net-14 / BP-audio-16 kept (the CT still pushes only when the master differs
    from what the headset was given or set itself).
R3  user-lumen/src/main.rs `render` :2352-2362 — while the transcript overflows, the right 6-px gutter band
    [3, 3 + rows x lh) is left unpainted on the assumption that wm paints the bar. On aarch64 there is no arm for verb
    28 (Lumen's own comment, main.rs:772-775; `sv_live` is always false) and on x86 whenever SCROLL_SET answers NONE nothing
    paints it. Input: aarch64 desktop (virt v3 / Orin), Lumen with a transcript longer than the view: the band keeps
    whatever was there — the surface's first pixels when the transcript overflows on its first paint, the OLD theme's
    ground after a light/dark switch — and the scroll-position indicator is gone (RING3SCROLL deleted the 2-px one).
    FIX (local, one fn): the band is skipped only when `a.sv_live`; otherwise the whole gutter is filled with the
    ground as before. x86 unchanged in effect: on the first overflowing frame `sv_live` is still false, the gutter is
    filled, then `sv_sync` registers and `scroll_set_ring3` paints the bar (`ring3_bar`) before the present. The
    aarch64 indicator stays owed with the aarch64 arm of verb 28 (already owed). BP-video-38 kept.
    COMPILE: user-lumen is not on the arc's two kernel lines; needs ONE ring-3 check:
    `cd unaos/crates/user-lumen && CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave36/target cargo +nightly check
    --release -Z json-target-spec --target ../../x86_64-unknown-none.json --features cryptocore` (arroyo
    build_user_lumen_x86's line, check not build). IF the seat does not want a third compile: R3 becomes an owed row on
    video.md §5 with this fix, not built.

## AMBER (9) — not built; non-local ones proposed as owed lines
A1  look.rs:111-114 / locale.rs:391-395 (SESSIONGENPOLL x PREFSCLIENTKEYS union): a login/logout now reaches them ONLY
    through the last-changed ring, and `note_changed` runs only when the KERNEL subscriber is on
    (prefs_client.rs:177 `if !kq { return; }`). Today the lock subscribes on its first pass (lock.rs:182, `login`), so it
    holds; crawl D 14 proposes making that subscribe count-only, and the switch fixture subscribes itself
    (tests.rs settingsmap_switch_leg) so it cannot see the dependency. Proposal (owed, security.md §5): ring the
    last-changed ring on every accepted write regardless of `kq` (note_changed before the `kq` return).
A2  xhci/mod.rs:3902 + suspend::endpoints_stop/:endpoints_restart (:18234-18276): `HELD[slot]` bit 0 suppresses EVERY
    doorbell of the slot, but the restart rings only DCIs whose Stop read state 3. A TRB queued during the hold on any
    other DCI of the slot (a Stop that failed and cleared its bit, a DCI not Running at entry) loses its doorbell and the
    endpoint idles after the resume. Proposal: restart rings every DCI of a held slot whose state reads Running or
    Stopped (power.md §5).
A3  ehci/mod.rs suspend::note_hub: HUBS never forgets a hub. A re-enumerated/unplugged hub leaves a stale Target;
    `remote_wake_one` on it fails -> `hub && has_kbd` -> unarmed -> that controller keeps every port up
    (`why=wake-unarmed`) for the rest of the boot; 4 slots fill. Proposal: drop the record on the hub's detach.
A4  video/status.rs:667 (SINKALLDOCS): `audio::mixer::pump()` now runs in the key path — inside the xHCI event handler
    (xhci/mod.rs:5178) and the EHCI_HID walk (ehci/mod.rs:16784) via fnrow.rs:91 — up to 64 mixed periods per press
    under the controller lock (input latency); the locks it nests (OUT try, MIX, avdtp PCM) are leaves today.
A5  hda_play.rs:30-31 (SINKALLDOCS ring 683 -> 213 ms): a service pass longer than 213 ms laps LPIB
    (:276-281 sees prev_cur == cur, no refill) and the stale ring replays; the 479 ms coded-play start gap is
    already owed on audio.md §5. Stated, not new.
A6  una-abi lib.rs `scroll_set` (RING3SCROLL): skips and DROPS up to 7 non-matching mailbox frames while it waits for its
    reply (window_raise's shape) — now called from Lumen's paint; any other frame in Lumen's mailbox then is lost.
A7  docs/dev/blueprint/power.md: §5 :44 ("USB port suspend is not done") and :45 ("A tickless idle … is unbuilt")
    contradict :43/:46; §4 :35 (`tick=<1khz|tickless-idle> usb=up`) and :36 (`tick=1khz usb=suspend`) are two versions
    of one line; BP-power-7 says "between 1 kHz ticks" while image-line.env arms tickless.
A8  docs/dev/blueprint/input.md:49: "ring-3 windows have no registration verb yet" — RING3SCROLL built it.
A9  tests.rs prefsclientkeys_leg (:5621): `sys_flag(..).unwrap_or(true)` — a failed GET makes the fixture write the
    user's `system.privacy.remember` to false and "restore" it to true.

## GREEN (5)
G1  power.md BP-power-7 anchor `xhci/mod.rs:18229` is a blank line (endpoints_stop is :18234).
G2  tests.rs wcpar_selftest: the stall leg can be vacuous — if the presenter claims every band no worker consumes
    STALL_US; `pass = crc_eq && stall_eq` does not need stall_late >= 1.
G3  locale.rs:423-429 non-desktop twins: `locale_changed()` = false never moves SEEN_CHANGES off u32::MAX, so `login`
    stays true and every pass would read 3 keys and print `via=login`; unreachable today (both callers are desktop).
G4  HELPVERBDOC's Action::Help / theme rows / action_code 53 are unconditional: every image's bytes move (knob-off
    baselines should expect it); on aarch64 the PC F1 row resolves to Help but only x86 `wc` has the door
    (shortcuts.rs help_action) — the press reaches ring 3 as INPUT_EV_ACTION 53 only.
G5  AVRCPCT: a headset's CHANGED moves the master, and the next watch pass sends the TG's kept CHANGED back to that
    headset (an echo of its own change; allowed by AVRCP).
