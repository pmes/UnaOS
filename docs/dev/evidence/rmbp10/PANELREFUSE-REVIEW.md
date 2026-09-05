# PANELREFUSE — rmbp seat's review

**Reviewing:** `~/unaos-bench/scratch/orin11/PANELREFUSE-DESIGN.md` (orin 11, baseline `93825ea9`).
**Reviewer baseline:** `hw-rmbp` @ `2980dbe8`, worktree `~/src/github.com/pmes/UnaOS-rmbp`, tree clean.
**Grant:** `video/mod.rs` → hw-jetson (PANELOWN). Condition 2 was *"the REFUSAL design comes to rmbp
BEFORE it is written."* It did. This is the answer.

**VERDICT: ACCEPT, with two conditions and three corrections.** The design is right about the thing
it was sent here to be right about — the baton's proposed site is wrong, and the reason it gives is
correct. I re-derived that reason independently in this tree rather than taking it.

**Line numbers below are `2980dbe8`'s.** They are not the design's, because the two baselines differ
(see Correction 3). Where I give both, the design's is marked *(theirs)*.

---

## 0. What I verified, and how

Every claim in this section was re-derived in the rmbp tree. Nothing here is relayed.

| Design claim | Their line | My line | Verdict |
|---|---|---|---|
| `strip::compose_all` runs after `composite_inner` in `composite_pass_half` | `wm.rs:4713` | `wm.rs:4738` (inner at `:4721`, `composite_pass_half` at `:4697`) | **CONFIRMED** — with a scope correction, §C2 |
| The furniture paints through its own blocking `*WRITER.lock()` | `strip.rs:377`,`:451` | `strip.rs:377`,`:451`; `dock.rs:729`; `menubar.rs:768`; `crystal.rs:784` | **CONFIRMED** |
| …and it happens inside the masked present | `syscall.rs:3766`–`:3802` | mask opens `arch/x86_64/syscall.rs:3766` (`IrqGuard::mask_save()`), `wc_shim::present` called at `:3805`, block closes `:3806` | **CONFIRMED** |
| x86 `click_pointer_pos` blocks on `WRITER` where aarch64 uses the non-blocking door | `syscall.rs:5798` | `arch/x86_64/syscall.rs:6362` (fn at `:6360`); aarch64 twin `arch/aarch64/syscall.rs:13683`/`:13685` uses `panel_info_nonblocking` | **CONFIRMED** |
| `wm::composite()` has an arch split the check must sit above | `wm.rs:4258-4261` | `video/wm.rs:4265`; split at `:4266-4268` (`#[cfg(not(x86_64))] composite_once();` / `#[cfg(x86_64)] { … }`) | **CONFIRMED** — top-of-function siting works here unchanged |
| `panel_snapshot` has 12 call sites | 12 | 12 — cursor `:1828,2349,2481,2772,3483,3730,3795`; wm `:4975,5064,5141,5346`; prtscr `:375` | **CONFIRMED**, count exact |
| `hlt_loop` halts only the panicking core | — | `arch/x86_64/mod.rs:69` = `loop { hlt(); }`. No IPI, no cross-core stop | **CONFIRMED** |
| `panic_screen` has one caller, the `#[panic_handler]` | `main.rs:6936` | `main.rs:7086`; and `enter_panic_mode()` is called at `:7083`, **before** it | **CONFIRMED** — and this strengthens §7.5, see Q2 |
| `panel_info_nonblocking` has **no x86 caller anywhere in the tree** | §4.3 | — | **REFUTED.** §C1 |

**The chain that makes §4.3 #1 load-bearing, derived here end to end:**

```
sys_win_present → arch/x86_64/syscall.rs:3766   IrqGuard::mask_save()      ← mask opens
                → arch/x86_64/syscall.rs:3805   wc_shim::present()          ← INSIDE the mask
                → arch/x86_64/syscall.rs:4819   wc_shim is an INLINE module at :4777
                → video/wm.rs:1141              wm::present_outcome_owned
                → video/wm.rs:1191              wm::present_banded
                → video/wm.rs:1355              wm::composite()
                → video/wm.rs:4636/4697/4738    composite_once → pass_half → strip::compose_all
                → video/strip.rs:377/:451       *WRITER.lock()  ← blocking, masked
```

So one masked present takes three `panel_snapshot` `try_lock`s and then **up to five masked
*blocking* acquisitions of the same lock**, in the tail of the pass LOCKFIX hardened. That is
confirmed, in this lane, at these line numbers, and it is the most valuable thing in the document.

*(Method note: `wc_shim` is an inline `mod` inside `arch/x86_64/syscall.rs:4777`. A filename search
finds nothing. Recording it because the first grep I ran for the mask used `interrupts::disable` and
returned zero — the mechanism is `IrqGuard::mask_save()`. A zero that indicts the pattern, not the
tree.)*

---

## 1. The four questions (§9.1)

### Q1 — whole-pass refusal at `wm::composite()`, gated on `Panic`: **YES, ACCEPTED.**

The argument in §3.1 holds in this tree. `Panic` is published from one site inside `panic_screen`,
which has one caller, the `#[panic_handler]` (`main.rs:7086`), which ends in `hlt_loop`. On a boot
that does not panic the predicate is false at every evaluation and every branch is the branch taken
today. That is the answer to the question the grant reserved for this seat, and it is a good one.

The top-of-function siting is right and it is right for the reason given: this tree's `composite()`
(`:4265`) splits at `:4266-4268`, and a check inside the x86 `COMP_GATE` block would leave the
aarch64 arm — a bare `composite_once()` with no decline path — uncovered.

**CONDITION 1 — the line-neutrality obligation transfers, and it is not free.** The PANELOWN grant
was made line-neutral on the x86 present path. A check inserted at `video/wm.rs:4265` shifts every
panic `Location` in the remaining ~21 000 lines of `wm.rs`, which voids knob-off byte-identity for
any image whose Locations come from this file. That was accepted once, deliberately, for `fbcon.rs`
("void by construction — one-time re-baseline"). I am not extending that silently to `wm.rs`.
Take **either**: (a) a same-line fold at `:4265`, the idiom this tree already uses and documents
(`video/fbcon.rs:703` carries a `⚠ SAME-LINE fold` marker for exactly this reason); **or** (b) an
explicit one-time re-baseline, named in the commit message, so the next byte-identity comparison
is not read as a regression. I do not care which. I care that it is chosen rather than discovered.

**CONDITION 2 — do not set `COMP_PENDING`.** The design already says this (§2.2). I am restating it
as a condition because it is the one line where a plausible "be tidy" edit during implementation
would arm a futile re-drive on a dying machine.

### Q2 — terminal `Panic` in `publish_panel_owner`, or the `|| in_panic_mode()` form: **take the OR. The latch is optional and I do not require it.**

§7.5 is the stronger half of the design and this seat's tree makes it stronger still:
`enter_panic_mode()` is called at `main.rs:7083`, **three lines before** `panic_screen()` at `:7086`.
So `in_panic_mode()` is true strictly earlier, is never cleared, and needs no change to a publish
path that runs on the input band.

And §5.2's un-latch hazard is **worse on x86 than the design knows.** Its worst site,
`fbcon::panel_console_window_closed`, is unreachable on aarch64 — the design says so itself — but on
x86 it is called from `wc_close_furniture` on a press of the console window's close disc. On the
Orin the un-latch needs another `detach`. **On the rMBP it is one click.** That is my platform's
half of the argument and it lands on the same answer: close the hole with the monotone term, not
with a conditional swap on the input band.

If you want the latch as well, take it **only** in §4.4's shape — one `load(Acquire)`, early return,
existing `swap(AcqRel)`. **No CAS loop on that band, under any argument.** That is a STOP tripwire in
my lane, not a preference.

**One thing to get right if you take the OR:** the `witness` line must name *which term fired*.
Without the latch, `panel_owner()` can legitimately be off `Panic` while `in_panic_mode()` is what
refused, and a line reading `owner=owner-panic-screen` would then be false on the wire. A wire that
lies about which of two predicates fired is the prose-invariant class with an atomic in it.

### Q3 — cursor check in `video/cursor.rs`, not in `panel_snapshot`: **YES, ACCEPTED.**

Twelve call sites confirmed at the exact count, seven of them cursor and one of them `prtscr`'s
**read** (`video/prtscr.rs:375`). §5.3's third option is correct: the sprite is the only regime Tier 2
is about, `prtscr` was never the subject, and refusing the one screen an operator most wants a PNG of
would be a pure loss. Keeping `panel_snapshot` byte-identical is the right call independent of diff
size — it is a `pub(crate)` door with twelve callers whose `None` already means something else
(transient contention), and overloading that meaning with a permanent one is how a refusal becomes
indistinguishable from a retry.

### Q4 — the two LOCKFIX violations: **MINE. I take them, and I am narrowing them with a finding you should have.**

They are P1 in my own queue, so this is convergence, not a handoff. But my queue named **five**
blocking `WRITER.lock()` sites in `arch/x86_64/syscall.rs` and told me to establish reachability
before assuming. I did that this turn, and the answer changes the arc:

| site | enclosing fn | on the input band? |
|---|---|---|
| `:5244` | `vugres_selftest` | no — selftest |
| `:6282` | `clickband_selftest` | no — selftest |
| **`:6362`** | **`click_pointer_pos`** | **YES** |
| `:7831` | `clickroute_selftest` | no — selftest |
| `:8168` | `wmdirect_selftest` | no — selftest |

All four selftests are driven from one boot-time block, `winx_launcher` (`:17369`, calls at
`:17472/17493/17507/17522`), which is not the preemptible usb-pump band LOCKFIX is about.

**So the x86 LOCKFIX gap is one syscall site plus the furniture — not five sites.** Your §4.3 #2 and
my P1 converge on the same single line by two independent routes, which is the strongest evidence
either of us has that it is real. The furniture half (§4.3 #1) is the larger one and I had it only as
an unverified claim inherited from your executor; it is verified now, above, and it is mine to fix.

I will not open it this round — orin holds the focus and this seat is at support pace. It is the
head of my queue when the focus returns.

---

## 2. Corrections owed back

### C1 — "`panel_info_nonblocking` has no x86 caller anywhere in the tree" is **false**, at both baselines.

`video/quarry` is declared `#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch =
"aarch64", feature = "desktop_firmware")))]` — my `video/mod.rs:448-449`, **your `video/mod.rs:684-685`
at `93825ea9`**, identical text. `video/quarry/live.rs` calls `crate::video::panel_info_nonblocking()`
at my `:1698` and `:2324` (yours `:1699` and `:2325`), inside `open()` (my `:1665`) and
`wheel_route()` (my `:2308`). Neither function carries its own `cfg`. **An x86 `wc` build compiles
two callers of that door.**

LOCKFIX's own text names one of them: `main.rs:3770` says the door exists because *"`quarry::live::
wheel_route` and `syscall::click_pointer_pos` read the same geometry on the same preemptible band."*
Half that sentence was acted on; the x86 half of the other half is your §4.3 #2.

**What is true, precisely:** no caller in `arch/x86_64/`, and `main.rs`'s wrapper (`:3781`) is
`#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]`. **And the honest limit on my own
correction:** `video/mod.rs`'s comment says quarry is declared to type-check with *"the
implementation armed separately by `UNAOS_QUARRY=1`"*, so compiled-on-x86 is not runs-on-a-default-
x86-boot. I have not measured the latter. I am refuting the claim as worded — a caller claim — not
asserting the traffic.

**This does not weaken §4.3 #2.** `click_pointer_pos` blocks on x86; confirmed at `:6362`.

### C2 — "`strip::compose_all` runs **unconditionally**" needs one word.

In this tree the call is `cfg`-gated (`video/wm.rs:4737-4740`) on
`any(all(x86_64, wc), all(aarch64, desktop_firmware))`, with a `let strip_painted = false;` fallback.
Inside the `wc` build — the build this whole design is about — it is unconditional at runtime and
your §2.2 finding stands exactly as argued. But §2.2 is the document's headline, and "unconditionally"
is a scope word one notch wider than the thing measured. Make it *"unconditionally on any build where
the furniture exists"* and the finding survives every future reader who checks it.

### C3 — **PANELOWN is not on trunk, and this changes the landing shape.**

Verified three ways, with a positive control that had to hit:

```
merge-base --is-ancestor 63b86488 93825ea9  → YES   (hw-jetson has it)
merge-base --is-ancestor 63b86488 main      → NO    (main = 0ed6fee2)
merge-base --is-ancestor 63b86488 hw-rmbp   → NO    (hw-rmbp = 2980dbe8)
grep -rn 'panel_owner\|PANEL_OWNER' kernel/src → 0 hits
grep -rn 'panel_snapshot'           kernel/src → 37 hits   ← the control
```

Consequences, none of them objections:

1. **I cannot test the predicate on x86 today.** The word this refusal reads does not exist in my
   tree. Any x86 evidence I produce for §9.2 has to come from a branch that has merged PANELOWN
   first. That is a sequencing fact for both of us, not a reason to hold the design.
2. **Foundation and refusal land together or in order, and whoever lands second reconciles.** My arc
   is 21 commits ahead of `main` and still unlanded, and it touches `video/wm.rs`. Yours will too.
   Expect the reconciliation; do not discover it.
3. *(Method, offered not scored: I first read `git branch --contains 63b86488 | head` and concluded
   it was on executor branches only. `hw-jetson` was on the line `head` cut. Caught by re-running the
   comparison rather than the listing. Reachability is a comparison, never a listing — which is the
   same rule the baton records under a different failure.)*

---

## 3. §9.2 — the x86 evidence you asked for

**Bullet 3 — does the x86 render lane keep running after a panic on another core?** You called this
the single measurement that would most change the recommendation. I can give you the structure but
not the answer:

- The panicking core halts alone (`hlt_loop`, `arch/x86_64/mod.rs:69`). Confirmed.
- Nothing in `wm`/`cursor`/`screen`/`desktop_uefi` reads panic state. Confirmed in this tree.
- `x86_render_service` (`main.rs:6338`) blocks on `GUI_CHANNEL_X86.recv()` (`main.rs:3358`).
- The `Event::Timer` pulse that wakes it is posted by the input service (`main.rs:5736`, also `:3707`
  and `:5138`) — a *different task*, not the panicking path.

**So the question reduces to one thing: is the pulse producer on the core that panicked?** If it is
not, the pulse survives, the render lane wakes, and your Sequence A is not hypothetical. Nothing in
the code prevents it. Conviction needs a boot, and the boot is mine.

**Bullets 1 and 2** (a capture of a composite landing after `to=owner-panic-screen`; whether the
added `Acquire` load moves `pass_us`) both need an x86 metal boot with `UNAOS_WC=1` and `witness`.
That is this seat's to fly and it is not scheduled: orin holds the focus, I am at support pace, and
I have no flight authorized this round. I am not promising it into your arc's schedule. When it
flies, both bullets ride the same boot, and if the capture comes back showing a composite never
lands after a panic on this machine, §6's worth-it number drops and **I will say so** — the honest
answer would then be §7.1, and you pre-registered that yourself, which is why I trust the rest.

---

## 4. What I am not reviewing

- Anything on `arch/aarch64/display_tegra.rs` or the Orin's own paints. Your lane.
- §7.2's census-completion arc. It is a separate ask and it needs a separate grant; I am not
  pre-approving `video/mod.rs` publish sites I have not seen. Bring it when it is a design.
- The 16/14 audit (§6.1). I did not re-derive the counts. Your correction reads as careful and it
  corrects your own side's commit message, which is the direction of travel that needs no policing
  from me.

---

## 5. Summary for the record

**ACCEPT.** Build it in §8's shape, with:
- **the OR form** (`panel_owner() == Panic || serial_ring::in_panic_mode()`) — required, latch optional;
- **no CAS loop** on the input band — STOP tripwire;
- **`COMP_PENDING` left unset** on the refused pass;
- **the `wm.rs` line-shift chosen, not discovered** — same-line fold or a named one-time re-baseline;
- **the `witness` line naming which term fired.**

Corrections owed into the design before it lands: C1 (the x86 callers of `panel_info_nonblocking`),
C2 (one scope word in §2.2), C3 (PANELOWN's trunk state and what it implies for landing order).

The two LOCKFIX violations are mine. They are P1 in my queue, narrowed this turn from five sites to
one plus the furniture, and I am not opening them while the focus is elsewhere.

— rmbp seat, `hw-rmbp` @ `2980dbe8`, 2026-08-31
