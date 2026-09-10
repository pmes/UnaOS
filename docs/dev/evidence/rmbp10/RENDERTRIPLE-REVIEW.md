# Adversarial review: "three OSes smashed together" / render-service triplication

**Reviewer:** rmbp 10, `hw-rmbp` @ `2980dbe8`. **Requested by:** Peter, via orin 12.
**Scope limit, stated first:** I measured **two** of the three services — `render_service` (Pi) and
`x86_render_service` (mine). `orin_render_service` is `c61b47e3` on `hw-jetson` and is not in my
tree. Every number below is a two-way comparison, not a three-way one.

## Verdict

**The triplication is real. The "shared 90%" is not supported, and "how do I wait" is not the only
axis.** The arc is worth doing; it is scoped wrong, and the mis-scoping has a specific failure mode
(below).

## 1. How much is actually shared

Bodies: `render_service` `main.rs:5288-5708` (421 lines), `x86_render_service` `:6338-7032` (695).
Both are 54% comment.

| measure | Pi | x86 | shared |
|---|---|---|---|
| non-trivial code lines (braces etc. removed) | 157 | 246 | **52** |
| → as a share | 33% | 21% | |
| distinct callees (comments stripped) | 44 | 64 | **28** |
| → as a share | 64% | 44% | |

**The gap between those two rows is the most useful number here.** Line-identity says 21-33%;
callee-identity says 44-64%. The services **call mostly the same things and express it differently**
— which is exactly what incidental divergence looks like, and it is the strongest evidence *for*
extraction. It is also why "90%" felt true: at the level of *what the code does* the overlap is high;
at the level of *what is written* it is not.

The 52 shared lines are concentrated exactly where orin says: front-fb capture, `Screen::new`,
`TargetPal`, `Console::new`, the `open_shell_window` mint, `mark_in_window`, the Key/Mouse/
MouseAbsolute match arms, cursor-visibility tracking, and `present_outcome_owned` with
`KERNEL_OWNER_DESKTOP`. **That part of the thesis holds.**

## 2. The axes — "how do I wait" is one of at least three

**A. How to wait — CONFIRMED.** Pi calls `recv`. x86 calls `gui_recv_blocking_x86` /
`gui_try_recv_x86` / `sleep_ticks`. Real, and orin's characterisation is right.

**B. Instance identity and role rehoming — x86 ONLY, and it is not incidental.**
x86-only callees: **`shell_remint`, `confirm_render_core`, `mint`**. The Pi service has **none**, and
its single textual `retire` hit is `retire_desktop_chrome` — a different concern (retiring backdrop
tenants), not instance retirement.
This machinery exists because on the rMBP the core running the service can *die*: boots 13/14/15
parked a core inside a single Kepler BAR1 store. So the service re-checks "am I still the incumbent"
every pass, **retires rather than returns** (wm's rows hold raw pointers), and a replacement instance
*adopts the dead one's window* rather than minting a second. **That is a property of this machine's
GPU, not a divergence in style.** It cannot be pushed into a driver that only answers "how do I wait".

**C. Focus / key-sink semantics — divergent BOTH ways.** x86-only: `user_input_set_active` (x4),
`focus_changed`, `shell_key_sink_note`. Pi-only: `click1_dispatch`, `drag_route_tail`.
Orin guessed this axis ("cursor bracket, focus") and was right to.

**Present/damage semantics are NOT an axis** — `present_outcome_owned` and `KERNEL_OWNER_DESKTOP`
are shared verbatim. That is a point in the arc's favour and the cleanest thing to extract first.

## 3. A finding I withdrew, recorded because the review asked for adversarial honesty

I first measured axis C with `grep -cE 'wc_click_route|user_input_route|wc_tab|FURNITUREFOCUS|instgui'`
and got **x86 = 18, Pi = 0**, and was about to report "the x86 service is also the input router."
Re-running with comment lines excluded returned **zero**. All 18 were commentary; the routing calls
are not in this function. The real axis-C evidence is the callee-level data above, which says
something different and weaker: both services do focus work, in different vocabularies.
**A grep counted comment density and I read it as behaviour.** Same class as everything else this
round; it survived only because the second measurement was run.

## 4. Answers to the four questions

**Is the triplication real, or pattern-matching?** Real. But the number that justifies the arc is the
callee overlap (44-64%), not 90%, and the arc should be sold on that.

**Is "how do I wait" the only axis?** No. B is x86-only and structural; C is two-sided. An extraction
that leaves "three thin drivers answering only how do I wait" will not fit x86, because x86's driver
must also own incumbency, retirement, and remint-by-adoption.

**One seat or three?** One owner — but not for orin's reason, and the choice of owner matters.
The failure mode is **designing the shared abstraction from the thinnest instance.**
`orin_render_service` is hours old and is presumably the simplest of the three; a `render_pass()`
modelled on it will meet axis B late, in x86, after the shape is fixed. Either the owner is the seat
holding the most-constrained service, or the design is validated against x86's remint path *before*
any extraction lands. Three seats negotiating is not what produced the triplication — each service
absorbing jobs beyond rendering is.

**Item 3, the empty rectangle:** decline the mint, and `fbcon::console_is_routed()` is the right
predicate — it names the *condition* (a console window already exists) rather than the *board*, so it
is a shared rule rather than an Orin special case. Minting-and-filling Pi-style would give that board
two console windows, which is the defect with more steps.

## 5. Item 1 — the `[spin6]` defect is not on my board

`[spin6]` is emitted only from `arch/aarch64/sched.rs` (`:5455`; the other hits are its own doc
comments). With `=== AARCH64 EXCEPTION: SYNCHRONOUS ===` alongside it, that capture is aarch64 — Pi
or Orin, not the rMBP. Correcting the address; the finding still needs an owner.
