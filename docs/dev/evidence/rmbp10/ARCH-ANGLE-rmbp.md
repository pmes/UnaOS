# Architectural insight — the x86 / shared-kernel-core angle

**rmbp 10, `hw-rmbp` @ `2980dbe8`.** Peter's question: how do you give us the architectural insight
to develop UnaOS correctly, so a change isn't "every line defensible, the result wrong."
Independent angle; orin 12's is negative-space-made-executable, and I agree with it and think it is
incomplete in one specific way (§3).

Everything below is measured in this tree tonight. Where a measurement of mine was wrong, the wrong
one is shown, because **the correction is the answer**, not an embarrassment beside it.

---

## 1. The headline instance, and it is mine

Peter asked for real instances of what an incoming change broke that no gate could see. Here is the
one that generalises.

**`./arroyo check` — the DONE gate every arc in this project runs — does not compile the bootloader.**
`check_both()` (`arroyo:3654`) does `cd crates/kernel` and runs two `cargo check`s there; the
userspace legs iterate `USER_CHECK_MATRIX` (6 crates). Every loader edit in this project's history
went into a commit whose gate could not see it. Found by orin 12; confirmed here independently.

**Then I over-claimed it, and the correction is the useful part.** My first pass listed the crates
not *named* in a check leg and reported **6 of 13 never type-checked**: boot-info, bootloader, net,
rast, una-abi, xusb-fw. That is a listing, not a measurement. `cargo check` compiles the dependency
graph, so:

| crate | why it is actually covered |
|---|---|
| boot-info | non-optional path dep of kernel (`Cargo.toml:2420`) |
| net | non-optional path dep of kernel (`:2435`) |
| una-abi | dep of user-blob / user-elf / user-blob-x86 / user-stat, all in the matrix |
| rast | optional, but 34 GATE-CFG rows enable it |
| xusb-fw | optional, but 17 GATE-CFG rows enable it |

**The true number is 1 of 13, and the reason is structural and generalises:**

> The gate covers the dependency graph reachable from its named roots. Library crates come free —
> something checked pulls them in. **A binary is its own root.** `bootloader` is the workspace's only
> leaf binary that no check leg names, which is exactly why it is the only invisible one — and why
> the *next* binary added will be invisible too, silently, by the same rule.

That is the shape of the whole problem in one case: an invariant that is true, load-bearing, and
recorded nowhere; a gate whose *coverage* was never itself asserted; and a first measurement that
was true about what it measured and false about what was asked.

## 2. Three more, briefly, all from tonight

- **A protection was added and the thing it protects against survived 30 lines down its own call
  chain.** LOCKFIX (`7847ceea`) forbids blocking panel acquires from the preemptible band. The
  masked present opens at `arch/x86_64/syscall.rs:3766` and reaches `strip::compose_all`
  (`video/wm.rs:4738`), under which `strip.rs:377`/`:451`, `dock.rs:729`, `menubar.rs:768`,
  `crystal.rs:784` all take bare **blocking** `*WRITER.lock()`. Five masked blocking acquisitions in
  the tail of the pass LOCKFIX hardened. No gate; the fix asserted its own completeness in prose.
- **`video/mod.rs:405` claims `panel_info_nonblocking` is "the ONE panel acquisition the input path
  makes."** False twice: `panel_snapshot` is reachable from the click router, and `click_pointer_pos`
  blocks on `WRITER` on x86 (`syscall.rs:6362`) where its aarch64 twin does not.
- **PANELOWN published an owner word whose only reader was `witness`-gated.** A published state with
  no consumer is an instrument wearing an interlock's clothes. Nothing detects that.

## 3. Is the lane rule the cause? No — and orin's hypothesis is the right shape aimed one step short

Orin: lanes make "add it to my platform" the cheapest correct move, so triplication is the process
equilibrium. **The gradient is real; the cause is not lanes.**

Evidence against lanes-as-cause, from merges I have actually lived: the cross-lane path *works* when
used. PANELOWN entered `video/mod.rs` through a grant and produced a shared word. The wm/fbcon/
screen/menubar parity port went across a grant. CONWINCLOSE, found in the Orin's lane, turned out to
reach the Pi. None of those duplicated anything.

**What is missing is not permission to share. It is a price on not sharing.** Crossing a lane costs
a negotiation, a recorded grant, and a review. Duplicating costs *nothing* and shows up in no
measurement. So the cheapest move is the duplicating one, not because sharing is forbidden but
because only sharing has a visible bill. Loosening lanes would trade this for merge collisions and
would not touch the gradient. **Price the duplication instead** — §4's A2 is that, and it is the
piece I think orin's negative-space framing is missing: negative space tells you what must not
happen; it does not make the wrong thing *expensive*, and cost is what actually steers three
independent seats.

## 4. Three structural assertions for `./arroyo`, concrete, each with go-red available today

**A1 — GATE-ROOTS: every binary target in the workspace is a named check root.**
Not "every crate" — libraries come free through the dep graph, and asserting them would be 12 rows of
noise for 1 real gap. Assert the *class that cannot come free*: enumerate binary targets
(`[[bin]]` / `src/main.rs`), assert each appears in a check leg. **Red today: `bootloader`.**
This is orin 12's GATE-BOOTLOADER generalised from the instance to its cause, so the next binary is
not invisible for another two years.

**A2 — GATE-FAMILY: a ratchet on platform-split symbol families.** Measured in this tree:

```
9 platform-split families, EVERY ONE of them currently size 2:
  render_service/x86_render_service   input_service/x86_input_service   usb_pump/x86_usb_pump
  on_block/pi_on_block   ladder/orin_ladder_arm   fault_handler{aarch64,tegra}
  run_bsp{,_tegra}   start_secondaries{,_tegra}   rast_demo_maybe{pi,tegra}
```
A checked-in ledger of family → size; growing a family goes red until the ledger is updated with a
one-line justification. **`orin_render_service` would have made the first size-3 family in the tree,
and A2 would have gone red at the moment the NAME was chosen — before the code was written.** That
is the only point where the cost is still one rename. This is the assertion that prices duplication.

**A3 — GATE-CLAIM: a prose invariant asserting uniqueness or a count must name a runnable check.**
Seeds red today: `video/mod.rs:405` ("the ONE panel acquisition"), `xhci/mod.rs:571` ("the compiler
enforces it" — four `.lock()` sites, not three), `arroyo:2491` (rotted four times and records its own
rot in its own text).
⚠ **Build it only after measuring its population.** The P7 lesson from my own baton: a warning that
fires 22 times teaches people to scroll past the region the real one appears in. Scope it to the
shared core first and widen on evidence.

## 5. How I get oriented, and it is repeatable

Not intuition — five habits, each of which caught a real error tonight:

1. **Read the artifact before trusting any claim about it.** The butler's marker set was a five-line
   read that refuted a claim two seats had blessed.
2. **Every measurement carries a positive control that MUST hit.** `panel_owner` = 0 meant nothing
   until `panel_snapshot` = 37 proved the search worked.
3. **Reachability is a comparison, never a listing.** `git branch --contains | head` cut `hw-jetson`
   off the line and I briefly concluded the wrong thing; `merge-base --is-ancestor` cannot lie that way.
4. **When measuring behaviour, exclude comments.** A routing grep returned 18 hits and 0 of them were
   code; I was one sentence from reporting a false axis in a review.
5. **Resolve a line number to its enclosing function before believing what the line means.** Four of
   the five "deadlock sites" my own baton named turned out to be selftests under `winx_launcher`.

Every one of those is mechanisable, and (2) and (4) could be lint-level today.

## 6. What I will do with this

Peter's cafe trip puts the rMBP live and this seat first against whatever we land. My commitment:
A1 and A2 are small enough to build and go-red in one arc, and A2 is the one I would build first,
because it is the only one that changes what the *cheapest* next move is.
