# WHITE BOARD — GR13 close, 2026-08-02

**This is a whiteboard, not a record.** Wiped and rewritten every time it changes. It carries
one thing: **what I need from Peter, right now.** Durable facts live in the baton
(`~/.claude/plans/unaos/active/unaos-gemini-coord-baton.md`); per-boot status lives in
`~/unaos-bench/PLAYBOOK-x86.md`.

---

# OPEN — one item

## 1. Two branch tips are not on origin

Trunk is safe: `UnaOS-gemini @ d5d4684f` is pushed and metal-proven — I verified local ==
origin by fetch, not by assumption. **These two are not, and they are the only x86 work that
exists on a single disk:**

git push origin wt/bgplace-x86 wt/postfold-doc-x86

- `wt/bgplace-x86 @ a4610035` — `bg_place_cpu` RESERVE, **two commits**
- `wt/postfold-doc-x86 @ 922e18b8` — doc-freshness sweep, **two commits**

If you would rather they stay local until they fold, say so and I will stop raising it — but
then it is a known, accepted risk rather than an oversight.

---

# → GR14 — THE PICKUP

## What is true right now

- **Trunk `d5d4684f`, pushed, and PROVEN ON METAL.** s61 boot1, 2026-08-01T21:40:01Z. Five
  arcs, fifteen commits, none of which had ever run on hardware — all five proved out on the
  **first** boot. All three cross-checks passed. `kepler=4393ms` named the 4.6 s GPU block.
- **The bench is cold for x86 and that is correct, not broken.** The `rmbp-gr13` session was
  closed cleanly (`MARK session-end` / `WATCH down`, 2026-08-02T15:02:14Z). No card staged,
  no watcher armed.
- **The Pi seat holds the serial line.** `/dev/ttyUSB0` is FTDI `ABAFUJCO` — the rMBP console
  adapter — and session `pi4-r23s1x` (opened 17:38Z today) has it open. **x86 cannot capture
  until Fox releases it.** Do not take it; ask.
- `~/unaos-bench/tools/waker.conf` is an explicitly **disarmed template**. Its patterns are
  correct and worth keeping; its `B_LOG` is a placeholder on purpose.

## Where the next arc starts

The two unfolded branches above. Then the GR13 menu that never got picked up — see the baton.

## Fox relayed twice on 2026-08-02; both verified in-tree, neither needs action now

- **The settle trim reconciles.** Fox reads the Pi's trim as 694→100, not 150→100. Both are
  right about different baselines: the settle used to be `hw_wait_budget()/4`, and that
  function really is arch-conditional (x86 calibrates off the TSC, aarch64 returns a fixed
  150 M ticks) — ~500 ms on x86, ~694 ms on the Pi. BOOTPACE M4 flattened it to 150 on both;
  my arc took that to 100. Fox's own evidence says 100 ms is **safe** on the Pi (`CCS=1`
  before the settle spin across five captures plus PA6; required settle 0 ms observed), so
  this is corroboration, not a conflict. Both seats now agree `/on` is unmeasured — and
  unmeasurable on the Pi. **The one thing left open is whether `hw-pi4` carries M4 at all**;
  if not, the two branches hold different settle histories and the merge needs a real answer.
- **WEDGE-8 is coming for the shared xHCI static, but it is not on origin.** Fox made
  `XHCI_CONTROLLER` private behind a claim/loan API. I checked: `dac6edb5` is not in this
  repo, `origin/hw-pi4` tips at WEDGE-7, and the static there is still public and unboxed. So
  it lives on Fox's disk only — nothing to rebase over, don't chase it. When it lands, this
  tree has **27 `.lock()` sites** to convert against their invariant of 3. Flagged for the
  integrator: 4 of those are in `aarch64/xusb_tegra.rs`, the **Jetson** lane's file, which
  Fox's converted-sites list does not mention.

## How Peter wants to be worked with

- **He pushes. The seat never pushes, ever** — no inference from "the last session did" or
  "durable means origin" overrides it. Raise it on the whiteboard and stop.
- **Metal is the verdict**, `strings` is the artifact. `./arroyo check` with the *full knob
  set* stays; the QEMU suites do not — emulation cannot reach these paths.
- **The whiteboard is his sheet.** Text in the file, one file, sections in it. Do not invent a
  new layout for the relay.
- Scratch media is scratch: compile, write, playbook. Ceremony over a scratch card is stolen
  money.

---

# CORRECTIONS I OWE, ALREADY MADE

- **I reported the bench "armed with a sound alarm." It was not armed at all.** The write that
  claimed it had been truncated by a shell-quoting failure and never reached the file; a `;`
  after the heredoc meant the success echo printed regardless. Then the config it *would* have
  written pointed at a closed capture. Both fixed — and the lesson is the one already on the
  wall in another form: **a success message that cannot fail is not evidence.**
- **KBDWIT has a false-CLEAN mode after all.** I previously wrote that its `adv` field has only
  a false-*alarm* direction. Wrong by half: the emitter falls through to zero on a failed
  FRINDEX read, so `ok=0` forces `adv=0x0000` (false alarm) **and** forces `hch=0 hse=0`
  (false clean). One fallthrough, both directions. Qualify every KBDWIT rule on `ok=1`.
  s61's two KBDWIT lines are both `ok=1`, so the s61 reading itself stands.
- **The playbook named two commits that were each the tip of a two-commit stack.** Corrected
  in place. Cherry-picking the named sha alone would have silently shipped half of each.
- **`pci-usb d=4620ms` is not 57% of a real boot.** Bench-media only; 113 ms on a default build.
- **CCSMARGIN was never n=1.** Seven captured boots, byte-identical.
- **The `gp_get=0` corruption claim was wider than the evidence.** No in-repo `gp_get=` capture
  and the beacon era ever overlapped — the witness was removed at `51b98bab` (pull 15), the
  beacons landed at `200be275` (pull 16). Retracted to Fox in writing.
- **The 150 ms settle leg is not blocked on PA6.** The Pi cannot produce that measurement *by
  construction* — firmware always wins the race to PP, so that rig is permanently the `/pre`
  case. Unmeasurable there, not merely unmeasured.

---

# STANDING, NO DECISION NEEDED

- The **`/on` (energised) settle leg** is inherited, not measured, by anyone. Nothing on this
  bench or the Pi can settle it. It sits at 150 ms — the pre-trim value — so nothing regressed.
- **`unaos/arroyo` has no `-smp` for x86 at all.** The core count lives only in
  `builder/src/main.rs`, and the placement pool now depends on it with zero slack at `-smp 6`.
- Reported and untouched in `kepler.rs`: three mutually inconsistent runlist entry encodings,
  two more count-bounded polls, six untimed `2_000_000`-iteration spins, and the fact that all
  three beacon regions get the **same** eight values — so `beacon SEEN` could never identify
  which structure the mirror window reflects, contrary to pull-16's stated discriminator.
- `envytools/` (53 MB upstream clone) sits untracked in the worktree root and is now
  gitignored, not deleted. If it was mine to fetch it should go; **the README bans nouveau
  register *semantics* on the Kepler lanes**, so its presence is a policy question, not a
  cleanup one. Your call.
