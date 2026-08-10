#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// UVUG-3: the first INTERACTIVE EL0 application — a userspace mini-vug that draws a real vug-style
// wireframe quartz crystal and responds to live keyboard/mouse. A static ELF64 (aarch64) program,
// loaded by the kernel's EXEC-1 machinery (`run_user_image`) into a fresh per-process slot and run at
// EL0 — the identical path the operator drives with `run /fat/VUG.ELF`.
//
// WHAT IT DOES
//   1. WC-C: creates its own 128x128 ARGB8888 WINDOW via SYS_WIN_CREATE — a real compositor window with
//      kernel-drawn chrome, tiled beside whatever else is on the panel, rather than the single 32x32
//      full-screen-centred compat surface SYS_FB_MAP exposed. 128x128 is `boot::FB_WIN_MAX_W/H` (one
//      64 KiB window slot); the crystal projection is screen-space-scaled to it.
//   2. Spawns TWO PERSISTENT EL0 worker threads via SYS_THREAD_SPAWN — one co-located, one on a SIBLING
//      CORE — each of which rasterises HALF of the surface (worker A: rows 0..64, worker B: rows 64..128):
//      it clears its band to the background and Bresenham-draws every crystal edge clipped to its band,
//      from the projected vertex coordinates the parent publishes each frame.
//      VUGGUARD: both spawns are CHECKED, and the thread pool is a request, not a guarantee — the
//      kernel's thread-handle table is a small GLOBAL pool that returns -EAGAIN when full. Any band
//      without a worker (spawn refused, or a live worker that misses the frame barrier's pass budget)
//      is rasterised INLINE by the parent instead, so the program degrades to single-threaded and
//      keeps drawing rather than blocking on a barrier that can never complete. See `_start`.
//   3. Each frame the PARENT reads input (SYS_INPUT_POLL), folds it into per-frame rotation/zoom state,
//      rotates + projects the 14 crystal vertices (integer Q16.16 math reimplemented from the kernel
//      vug.rs — no float), publishes the pixel coordinates, RELEASES both workers (the `phase` word),
//      blocks on a FUTEX until both have ARRIVED (the `done` word), and PRESENTS (SYS_WIN_PRESENT).
//   4. On exit (ESC, or the interactive frame cap) it signals the workers to leave, JOINs both,
//      and prints its witness before exiting 0.
//
// VUGCLICK — CLICK SEMANTICS IN A WINDOWED WORLD. Until VUGCLICK a click EXITED the program. That rule
// was written for the full-screen takeover era, where the vug owned the panel and any click meant "done".
// Since WC-C there is no takeover mode left to reach: every vug creates its own compositor window
// (SYS_WIN_CREATE, unconditional, below), tiled beside other windows. In that world clicking is how an
// operator focuses or interacts with a window, so "click exits" meant every attempt to touch a vug killed
// it — and with WC-J erasing a dead window instantly, the death read as a spontaneous crash (P62,
// "vug is crashing", on a wire showing no panic, no fault, and the program's own designed exit path).
// VUGCLICK's answer was to make a click toggle PAUSE instead of exiting; CLICK-ONE, below, retires that
// answer along with the question.
//
// CLICK-ONE — ONE VISIBLE RULE FOR STOP AND START. P74, blocked at the bench: "click stop/start is all
// messed up cannot continue test." Nothing was broken in isolation; THREE independently correct stop
// states had accumulated, and a click could land in any of them:
//   1. FROZEN BY UNFOCUS (VUGMIN-C) — only the focused vug runs; the rest hold still.
//   2. PAUSED BY A DELIVERED CLICK (VUGCLICK) — the demo's own historical click meaning.
//   3. PARKED ON AN EMPTY RING (VUGPAUSE-2) — no input, nothing to draw, so nothing runs.
// CLICK-SWALLOW then made the FIRST click on an unfocused vug focus-only, so the SECOND click toggled
// pause. The visible result of a click had become a function of invisible state — which vug had focus,
// and whether this was the first click or the second. No operator can hold that model, and the bench
// report is the proof.
//
// CLICK-ONE's answer was that **a click is FOCUS/RESTORE ONLY and never app input**: the focused vug
// runs, unfocused vugs freeze in place, and the pause verb moves to the KEYBOARD (SPACE — `K_SPACE`,
// chosen because nothing bound it). A DRAG still rotates, ESC still exits, and interactive takeover is
// keyboard-armed and untouched.
//
// CLICK-PLAIN — THE CLICK IS ACKNOWLEDGED WHERE THE OPERATOR CAN SEE IT. P75, on metal: "stop works
// like absolute garbage there is no reason to it." CLICK-ONE removed the click's run-state meaning but
// left the operator with NO feedback for a click at all, while the kernel side (CLICK-SWALLOW's swallowed
// press, VUGMIN-C's hide of the DEPARTING focus owner) meant a click still appeared to stop a vug — one
// click later, and a different vug than the one clicked. Both kernel halves are gone this arc: the router
// DELIVERS the focus-changing press to the window it just raised, and a focus change never hides anything
// any more (only focusing the SHELL idles the fleet).
//
// What this program does with the delivered click is deliberately split into two layers, so the final
// grammar is a decision rather than an accident:
//   * LAYER 1 (this file, unconditional) — a click is ACKNOWLEDGED and nothing else. It advances a
//     counter drawn beside the fps readout in the top-left corner, and prints `:: UVUG: click n=<N> ::`.
//     Zero coupling to run state; SPACE remains the only stop/start control. That makes "did my click
//     reach the window under the cursor?" a question the panel answers by itself.
//   * LAYER 2 (one clearly-fenced hunk in the frame loop, `LAYER 2 (CLICK-RUN)`) — a click ALSO toggles
//     the run state, defined ABSOLUTELY rather than as an inversion of an invisible flag: not running
//     (paused OR hidden) -> RUNS; running -> STOPS. Delete the fenced lines and Layer 1 remains exactly.
//
// A click is a press+release whose pointer travel stayed under `CLICK_THRESH`; anything further is a drag
// and rotates, exactly as before.
//
// TWO PATHS — deterministic auto (QEMU) vs interactive (metal). The switch is INPUT-DRIVEN, not
// time-boxed (UVUG-4): the parent polls SYS_INPUT_POLL EVERY frame for the program's whole life, and
// the FIRST input event AT ANY FRAME flips it to interactive permanently. There is no detection window
// to race — the old DETECT_FRAMES fallback closed in well under a second at EL0 frame rates, before a
// human could touch a key.
//   * QEMU raspi4b delivers no USB HID, so no input ever arrives — zero events ever — and the program
//     stays on the deterministic auto path: it keeps the fixed idle tumble (yaw += 3, pitch += 1
//     brad/frame) exactly as it did from frame 0, runs to AUTO_FRAMES (300) total, computes a
//     deterministic FNV-1a checksum of the final surface (a pure integer function of the final frame's
//     geometry, independent of thread interleaving), and prints
//     `:: UVUG: frames=300 threads=2 checksum=<hex> ::` — the existing witness, still green and
//     deterministic. This is what the kernel's `uvug_witness` boot self-test asserts exit=0 on.
//   * On metal a keypress/mouse arrives whenever the operator acts, so the program enters INTERACTIVE
//     mode at that frame: it prints `:: UVUG: interactive takeover at frame <n> ::` (proving the input
//     arrived on metal), cancels the auto-tumble and the 300-frame cap, and switches to held-state
//     control — WASD/arrows rotate (TRUE held state from KeyDown/KeyUp), Q/E zoom, a mouse drag rotates
//     (per-frame clamped delta, full-panel-drag ≈ one revolution), SPACE toggles pause, ESC exits. It
//     runs until ESC and prints `:: UVUG: interactive exit=<key|frames …> frames=<n> ::`.
//     Interactive is metal-only (no HID in QEMU).
//
// VUGLIFE — DESKTOP VUGS DO NOT DIE OF OLD AGE. INTERACTIVE_CAP was the last surviving demo-era run
// deadline, and it killed exactly the vugs an operator was using. The kill is worse than the number
// suggests: a DETACHED vug already runs its auto path uncapped (VUG-BG), so it can sit on the desktop
// for hundreds of thousands of frames — and the moment the operator TABS TO IT, the first input event
// flips `interactive` on, the already-past cap is tested for the first time, and the program exits
// instantly. That is P64's "the vugs crash as I tab": four deaths, all
// `:: UVUG: interactive exit=frames frames=<36000..271484> ::`, no fault anywhere on the wire — the
// same shape as the VUGCLICK relic, a designed exit that a long-lived desktop turned into a crash.
//
// The split is by LAUNCH MODE, which the program can already see (the info-page DETACHED bit), not by
// a kernel-side special case:
//   * DETACHED (`bg /fat/VUG.ELF` — the desktop spawn): UNBOUNDED. It exits on ESC or `kill`, never on
//     a frame counter. At the frame the old cap would have fired it prints ONE
//     `[vuglife] budget waived (interactive) frames=<n>` line and keeps running, so the next attended
//     boot PROVES the waiver fired rather than inferring it from an absence.
//   * FOREGROUND (`run`, and every fixture/battery launch — `uvug_witness`, BGRUN-ST): the bounded
//     budget STAYS. Gate liveness depends on a vug that terminates, and a foreground run is exactly
//     what the batteries drive. When that exit is taken it now says
//     `exit=frames_budget frames=<n> (fixture mode)` — a single bare token in the parsed field, the
//     prose qualifier outside it — so no future sitting re-diagnoses this as a crash.
// The deterministic AUTO path (AUTO_FRAMES = 300, the checksum witness) is untouched in both modes.
//
// VUGPAUSE — A PAUSED VUG MUST NOT BURN A CORE. VUGCLICK made a gesture PAUSE the rotation (a click
// then, SPACE since CLICK-ONE — the mechanism below is indifferent to which), and that is
// where it stopped: pause froze the frame's ADVANCE (the orientation stopped changing) but the frame
// LOOP kept running at full rate — every paused frame still drained input, released both workers,
// rasterised 128x128 twice, futex-barriered and PRESENTED, redrawing a surface that could not differ
// from the one already on the panel. P67v2 paid for that on silicon: six click-paused vugs, six full
// render pipelines, cores pinned with the crystals standing still.
//
// So: when the vug is `paused` AND its render state (orientation, zoom, and the displayed fps digits —
// everything this program can put on its surface) is UNCHANGED since the last present, the frame is
// SKIPPED entirely — no worker release, no rasterise, no barrier, no present. The parent polls input
// and calls `SYS_YIELD`, which is the whole idle loop. Consequences, each deliberate:
//   * UNPAUSE LATENCY is one idle iteration — a poll plus a yield — because the idle loop is exactly
//     the input half of the normal frame. The SPACE that unpauses, ESC, and a drag are all seen on the
//     very next pass, and the first frame after ANY state change renders normally.
//   * THE WINDOW STAYS LIVE AND KILLABLE. VUGGUARD's P60 lesson is that a vug which parks in an
//     unbounded `futex_wait` is an unkillable empty window; this idle path never blocks on anything. It
//     is a runnable yield loop, so `kill` and the compositor see an ordinary running process.
//   * `frame` COUNTS FRAMES PRESENTED, so it does not advance while idled. That is what makes the
//     VUGFPS readout honest: the once-per-second refresh keeps running FROM the idle loop and the
//     rate falls to 0 rather than freezing at a stale number. A changed digit is the one thing that can
//     still redraw while idled — overlay only, then one present, at most once per second.
//   * THE DETERMINISTIC AUTO PATH CANNOT REACH THIS. `paused` is set only inside the `interactive`
//     branch, and interactive is armed only by a real input event; QEMU raspi4b delivers no HID, so the
//     300-frame checksum run never evaluates the idle predicate as true. The auto path's geometry, its
//     code shape and its checksum are untouched by construction, and `kernel8-test` proves it.
//   * A FOREGROUND (fixture-mode) vug held paused therefore also stops consuming INTERACTIVE_CAP. That
//     is correct rather than a hole: pause is operator-driven, so a paused foreground vug is one an
//     operator is holding, and it still exits on ESC or `kill`. No battery leg can reach it (no HID).
// A one-shot `[vugpause] idle engaged frame=<n>` names the first engagement on the wire; it is latched,
// never per-frame.
//
// VUGMIN — A VUG NOBODY CAN SEE MUST NOT BURN A CORE EITHER. Peter's ruling at P69: "if vug is minimized
// it should shut off". The audit that opened this arc found that UnaOS has no minimize verb and needs
// none — the state he named ALREADY exists under another name. `wm::focus_changed`'s shell arm (the
// operator TABs to the console) pushes every window below `SHELL_Z` and erases its box: the vug is gone
// from the panel, and its frame loop keeps running at full rate against a surface the compositor will
// not read. That is VUGPAUSE's exact disease with a different trigger, so it takes VUGPAUSE's exact cure
// rather than a second mechanism.
//
// The kernel publishes "every window you own is hidden" as bit 1 of the RO info page's process-flags
// word. This program polls it EVERY FRAME — unlike bit 0 (DETACHED), which is fixed before the process
// runs and is read once at start-up — because it changes underneath a running vug. The poll is one
// `read_volatile` of a mapped word; it costs less than the branch consuming it, which is why it is
// unconditional rather than sampled.
//
// `hidden` then joins `paused` into `frozen`, and `frozen` replaces `paused` in TWO places, both
// load-bearing:
//   * THE ORIENTATION FOLD. A hidden vug holds its orientation exactly as a paused one does. It was
//     written to keep the skip predicate reachable (the predicate then compared this frame's orientation
//     against the last presented one, and an advancing idle tumble made that comparison fail forever);
//     VUG-PACE removed that comparison, so what the fold now carries is the whole of restore-as-a-resume —
//     without it a hidden vug's crystal would drift through the hidden interval and JUMP when it came
//     back. It is also the invariant the predicate now rests on: while frozen, the render state cannot
//     change, because this arm is the one that runs.
//   * THE SKIP PREDICATE's first conjunct, and only the first. `presented` and the overlay conjunct still
//     gate, so a vug hidden before its first present still renders that frame, and the frame that
//     restores it to the panel is an ordinary rendered frame from the preserved state — restore is a
//     resume, not a jump.
// One difference from pause, and it is the ruling itself: the once-per-second fps refresh does NOT run
// while hidden. A paused vug is on the panel and its readout is owed to the operator; a hidden vug's
// refresh would be discarded by the compositor, so drawing it is the very waste this arc ends, merely at
// one hertz. A one-shot `[vugmin] idle engaged frame=<n>` mirrors `[vugpause]`, on its own latch so
// neither reason can silence the other.
//
// DORMANT AS SHIPPED (VUGMIN-A). No kernel path sets bit 1 yet: the two `wm` call sites belong to
// VUGMIN-B, since `wm.rs` is another session's lane this arc. The bit therefore reads a constant 0 and
// nothing above is reachable. Note the auto path is untouched even once B lands, and for a stronger
// reason than pause's: a headless QEMU run has no HID, so nothing ever TABs to the shell, so nothing is
// ever hidden. `kernel8-test`'s 300-frame checksum proves it both before and after.
//
// VSYNC-PACE (GR22, x86) — THE CLAUSE BELOW IS NOW HALF FALSE, AND THE HALF THAT DIED IS THE KERNEL'S.
// "…and none in the kernel's present path either" held until GR22. On x86 with the window compositor
// armed (`wc`, i.e. every desktop boot, unless `UNAOS_NOPACE=1`), `SYS_WIN_PRESENT` and
// `SYS_WIN_PRESENT_ROWS` now SLEEP the caller to the panel's 16667 µs frame boundary before compositing.
//
// That is a different thing from an fps target in this program, and the distinction is the reason the
// clause below is amended rather than deleted. An fps target is a TUNING CONSTANT: it has to be guessed,
// it is wrong on the next panel, and it has to be re-guessed in every program that ever opens a window —
// and this program still has none, which is why it needed no change at all for the pacer to work. What
// the kernel added is the PANEL'S PHYSICS: the beam publishes one frame every 16667 µs, so a second
// present inside that interval is composited, paid for, and overwritten before anyone can see it. That
// fact is known on the side of the seam that owns the panel and is identical for every client, so it
// belongs there and nowhere else.
//
// Consequences for anyone reading this loop: on a paced x86 boot the loop's rate converges to ~60/s with
// no code here doing anything about it, `[wcn] gap` collapses onto 16..17 ms (which the WC-N watch-list
// used to call a finding and now calls the healthy signature there), and the VUGFPS overlay reads ~60
// rather than the 55.8–100.5 scatter Boot AO measured. aarch64 is untouched — the Pi's plateau was and
// remains the barrier story below, which is a different mechanism with a different fix.
//
// VUG-PACE — A VUG RUNS AT THE MACHINE'S SPEED, NOT THE SCHEDULER'S. P73: "there's a delay to a vug
// speeding up when it's the only one running", and "vug still wants to go back to what it thinks its fps is
// supposed to be even though it could run faster". There is no fps target in this program and never has
// been — no sleep, no frame budget, no throttle, and (see VSYNC-PACE above) none in the kernel's present
// path either until GR22 put the PANEL's cadence there on x86. The plateau
// was the FRAME BARRIER parking on its first pass: a park costs a wake plus a dispatch, dispatch latency
// belongs to the run queue rather than to the spare CPU, and two arrivals per frame put two of those round
// trips under every healthy frame. A floor made of dispatch latency does not fall when the machine empties
// out, and a stable floor quantises the VUGFPS readout into a number that looks like a target.
//
// The barrier now waits the way the workers already wait — `BARRIER_SPIN_YIELDS` passes of `SYS_YIELD`,
// then `futex_wait` — which is adaptive with no estimate and no window, so the frame AFTER contention drops
// is already faster. The long argument, including why the spin cannot become a hog and what it cost in
// bytes, sits at the barrier itself.
//
// The same arc carries the P73 mouse-preempt triage's Fix C, on the OTHER side of the loop: the skip
// predicate's orientation conjuncts required the render state to already match the last present before it
// would park, which could only ever fail on the transition frame and failed CLOSED there — a vug frozen
// mid-motion kept running the whole frame loop instead of parking (`[vugpause2] blocked=` pinned at 8192
// across ~500 s of saturation). They are gone; the invariant they were testing holds by construction. The
// rest of VUGPAUSE-2's idle contract is untouched: paused, hidden and idle vugs still park in
// `SYS_INPUT_WAIT` exactly as designed.
//
// VUG-PACE-2 — BOTH RESIDUALS WERE OUTSIDE THIS PROGRAM, and the record here is so no future arc
// re-audits this file for them. (a) THE RESIDUAL "PREDESTINED FPS" WAS THE SCHEDULER'S PLACEMENT
// LATCH, not a pacer: SPREAD-5 re-asked the core-placement question only after a >=100 ms park, and a
// frame-paced vug never parks that long, so contention-era packing (two vug threads time-sharing one
// core while others idled — s1q wire: win1 pinned at 30.9/s, c2 at 99%, rewake frozen) persisted
// forever after the contention left. SPREAD-6 (sched.rs) lets a micro-park wake re-ask every 250 ms.
// Note the discriminator the eye already carries: the idle tumble below is FRAME-based (3 brads per
// rendered frame, never time-based), so on the auto path rotation speed IS the frame rate made
// visible — a crystal that "returns to its old speed" is a real fps reversion, not a perceptual
// artifact, and the on-window VUGFPS digits and the rotation can never honestly disagree.
// (b) THE WIN1 LOCKUP (att=0, no fault, HUD frozen, no resume edge on click) was the frame barrier's
// ARRIVAL park below with a worker STRANDED by a kernel futex defect: two waiters entering
// futex_wait together on a key with no standing bucket could mint two buckets for one key (this
// program's PHASE word is the only two-concurrent-waiter key in the system), and futex_wake stopped
// at the first — wake_phase woke one worker, the other slept forever, DONE never reached `live`, and
// the parent parked at the barrier making no passes (so BARRIER_PASS_BUDGET, which counts returned
// passes, could never fire — the documented "parked forever" limitation, observed in anger). Fixed in
// sched.rs (FUTEX-DUP: claim joins an existing same-key bucket; wake scans every bucket serving the
// key; `[futexdup]` witnesses an absorbed race). This program's barrier protocol was and is
// lost-wakeup-safe against a correct futex; nothing here changed.
//
// Barrier direction split (deliberate, robust under QEMU raspi4b's lack of a Group-1 timer IRQ — see
// docs userspace.md M6e): ARRIVAL (worker -> parent) is a real FUTEX (workers atomically bump `done` +
// SYS_FUTEX WAKE, the parent SYS_FUTEX WAITs); RELEASE (parent -> worker) is a SYS_YIELD poll on `phase`
// (keeps each worker runnable on its own core, needing no cross-core wake). Both wait loops re-check their
// condition, so the barrier is lost-wakeup-safe. On metal (real timer IRQs) either direction works.
//
// EL0 owns only the OFF-SCREEN surface bytes — never the scan-out, never a physical address, never a
// kernel mapping (SYS_WIN_PRESENT is the only surface->screen path, and it runs in the kernel). Window
// CHROME is drawn by the kernel from its own copy of the title, so this program cannot forge a frame.
// Page-permission laws (per-page perms, WXN) are untouched.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------------------------
// Syscall ABI (Linux-aarch64): x8 = number, args x0..x5, return in x0. The kernel SVC path preserves
// every GPR except x0.
// ---------------------------------------------------------------------------------------------
// ABIFREEZE: every number below is IMPORTED from `una_abi` — the ONE declaration, shared with both
// kernels and with every other ring-3 program. This block used to be fourteen local `const`s that
// nothing in the build compared against the dispatchers they were calling.
use una_abi::{SYS_EXIT, SYS_WRITE, SYS_YIELD};
/// VUGPARK-FALLBACK: `SYS_SLEEP_MS(ms)` — a real timed park on both arches (5 on each; see
/// `arch/{aarch64,x86_64}/syscall.rs`). Used ONLY when `SYS_INPUT_WAIT` answers a negative, which
/// today means the x86 dispatcher has no verb 28 and its default arm returned `-ENOSYS`.
use una_abi::{
    SYS_FUTEX, SYS_GETINFO, SYS_INPUT_POLL, SYS_SLEEP_MS, SYS_THREAD_EXIT, SYS_THREAD_JOIN,
    SYS_THREAD_SPAWN,
};
// VUGPAUSE-2: the BLOCKING half of the input pair. Returns 0 once the ring may be non-empty; it does not
// dequeue, so the ordinary `drain_input` at the top of the loop still sees every event.
use una_abi::SYS_INPUT_WAIT;
// WC-C: the WINDOW verbs replace the single-surface SYS_FB_MAP/SYS_FB_PRESENT compat pair.
use una_abi::{SYS_WIN_CREATE, SYS_WIN_PRESENT};
// FBCON-DMG: `SYS_WIN_PRESENT_ROWS(win, y0, y1)` — a present that declares which SOURCE rows of this
// program's own surface actually changed, so the compositor repaints only those. x86 only, for now; the
// aarch64 kernel does not define 33 and answers `-ENOSYS` (-38) from its dispatcher's default arm, which
// is why `present_rows` below carries a MANDATORY whole-box fallback rather than treating a failure as an
// error. Additive: 30 is untouched and still the only present this program makes on aarch64.
use una_abi::SYS_WIN_PRESENT_ROWS;

// WITSWEEP — REGISTER-SURVIVAL INVARIANT (sys0..sys4): these stubs use `in("x1")`/`in("x2")`/
// `in("x3")`/`in("x8")`, which PROMISES the compiler those registers hold their values across the
// `svc`. That is sound today only because the kernel's SVC return path restores the full x0-x30 + FP
// register file (`__vec_svc` → SAVE_GPRS/RESTORE_GPRS in arch/aarch64/exceptions.rs) — nothing about
// the AArch64 call convention guarantees it. The x86 tree already hardens its sysret with a GPR
// scrub; if that hardening ever lifts to aarch64, any SVC-return scrub arc MUST flip these
// constraints to `inout("x1") a1 => _` (etc.) IN THE SAME COMMIT, or every stub here becomes
// undefined behavior the moment the kernel stops restoring the clobbered registers. The identical
// stub set in user-stat/src/main.rs carries the same invariant.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn sys0(n: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!("svc #0", inout("x0") 0u64 => r, in("x8") n, options(nostack));
    r
}
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn sys1(n: u64, a0: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!("svc #0", inout("x0") a0 => r, in("x8") n, options(nostack));
    r
}
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn sys2(n: u64, a0: u64, a1: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "svc #0",
        inout("x0") a0 => r,
        in("x1") a1,
        in("x8") n,
        options(nostack),
    );
    r
}
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "svc #0",
        inout("x0") a0 => r,
        in("x1") a1,
        in("x2") a2,
        in("x8") n,
        options(nostack),
    );
    r
}
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn sys4(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "svc #0",
        inout("x0") a0 => r,
        in("x1") a1,
        in("x2") a2,
        in("x3") a3,
        in("x8") n,
        options(nostack),
    );
    r
}

// ── x86_64 stubs, GRAFTED AT MERGE ASSEMBLY from the x86 trunk's WINX-7/TEARDOWN-1 port
// (UnaOS-gemini f36ab3d5) — my body, their ABI layer. TEARDOWN-1's discipline carried intact:
// `syscall` destroys rcx (return RIP) and r11 (RFLAGS) so both are clobbered. The kernel's
// sysretq tail additionally scrubs SIX registers — rdi/rsi/rdx/r8/r9/r10 — to zero on EVERY
// syscall return, unconditionally, regardless of how many arguments that syscall actually took.
// So every stub here, no matter its arity, must declare all six: `inlateout(reg) a => _` for
// whichever ones happen to carry that stub's own arguments, `lateout(reg) _` for the rest. A
// stub that only names the registers it passes (the arity mistake this block used to make) still
// lets the compiler believe an unnamed one — say `rdx` in a 2-argument stub — survives the
// syscall; it does not, and reusing it after the call reads back zero.
// The clobber list states the ABI the kernel actually implements, so the compiler reloads what
// it must (declaring them `in(...)` once cost the second THREAD_SPAWN its entry pointer: the
// kernel's scrubbed rdi=0 was validated and refused with -EFAULT).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn sys0(n: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => r,
        lateout("rdi") _, lateout("rsi") _, lateout("rdx") _,
        lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _, lateout("r10") _,
        options(nostack),
    );
    r
}
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn sys1(n: u64, a0: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => r,
        inlateout("rdi") a0 => _,
        lateout("rsi") _, lateout("rdx") _,
        lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _, lateout("r10") _,
        options(nostack),
    );
    r
}
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn sys2(n: u64, a0: u64, a1: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => r,
        inlateout("rdi") a0 => _,
        inlateout("rsi") a1 => _,
        lateout("rdx") _,
        lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _, lateout("r10") _,
        options(nostack),
    );
    r
}
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn sys3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => r,
        inlateout("rdi") a0 => _,
        inlateout("rsi") a1 => _,
        inlateout("rdx") a2 => _,
        lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _, lateout("r10") _,
        options(nostack),
    );
    r
}
/// The fourth argument goes in **r10, not rcx** — `syscall` writes the return RIP into rcx as its
/// first act, so a value there would be destroyed before the kernel could read it; r10 is SysV's
/// nominee for exactly this reason.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn sys4(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let mut r: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => r,
        inlateout("rdi") a0 => _,
        inlateout("rsi") a1 => _,
        inlateout("rdx") a2 => _,
        inlateout("r10") a3 => _,
        lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _,
        options(nostack),
    );
    r
}

#[inline(always)]
fn sys_yield() {
    unsafe { sys0(SYS_YIELD) };
}
#[inline(always)]
fn write_bytes(p: *const u8, len: usize) {
    unsafe { sys3(SYS_WRITE, 1, p as u64, len as u64) };
}
#[inline(always)]
fn exit(code: i32) -> ! {
    unsafe { sys1(SYS_EXIT, code as u64) };
    loop {
        core::hint::spin_loop();
    }
}
// SIZE — read this before adding code to this program. `.text` must end at or below 0x2000. One byte past
// it and the linker puts `.bss` on the next page, the stripped image jumps a full 4 KiB to 16664 bytes,
// and `arroyo` rejects it against `USER_REGION_SIZE` (16 KiB) as a hard build failure. VUGMIN-A hit this
// wall from the message side; VUGPAUSE-2 hit it from the code side and landed at exactly 0x2000, so the
// next arc has NO headroom and should expect to pay for what it adds.
//
// Two measurements worth keeping, both counter-intuitive and both re-measured on this program:
//   * `futex_wait`/`futex_wake` are CHEAPER inlined. The body is three instructions and a `ret`, so a
//     call site costs more than the stub once argument setup is counted (the `inline(never)` variant of
//     the pair measured 0x201c against 0x1fe8).
//   * `wake_phase`/`phase_exit` are cheaper OUT of line, because what they share is a whole stub with its
//     arguments already fixed. Folding the two `PHASE_EXIT` sites into `phase_exit` was the last 12 bytes.
// VUG-PACE worked against that wall and had to BUY everything it added. Its four economies, in case a later
// arc needs the same tricks, none of them behavioural:
//   * FOLD A NEW LOOP COUNTER INTO AN EXISTING ONE where their meanings allow it — the barrier's spin
//     budget rides `passes` rather than a second `spins`, worth 48 bytes (8268 -> 8220).
//   * REPLACE A DERIVED VALUE COMPUTED BY A LOOP WITH A LADDER OF CONSTANTS — `draw_fps` derived its
//     leading power of ten with `while k < n { div *= 10 }`; one `if/else if/else` yielding the pair is
//     32 bytes cheaper (8220 -> 8188).
//   * EVALUATE A REPEATED PREDICATE ONCE — `detached || interactive` was inline at four sites in the frame
//     loop and is now the `overlay` local.
//   * PRE-JOIN LITERALS AT THE CALL SITE. `stall_witness` assembled `phase`, a separator, `label` and `=`
//     from two slices with four `put` calls; passing ONE joined literal (`b"barrier done="`) emits the same
//     bytes with three fewer calls and two fewer arguments at each of three sites — 174 bytes, the largest
//     single saving in the file (8196 -> 8022).
// Landed at 8022, so the next arc inherits ~170 bytes rather than none. Spend them knowing the ceiling is
// a cliff, not a slope: one byte past 0x2000 costs a whole 4 KiB page and the build.
//
// CLICK-PLAIN spent them and then some, and had to buy the difference. Its first draft measured 8477 —
// 285 OVER — for a click counter, a wire line and the LAYER 2 hunk. Three economies brought both layers
// in at 8121, and the first two are new tricks worth keeping:
//   * ONE WITNESS HELPER FOR A REPEATED LINE SHAPE. `say(label, n)` emits `<label><n> ::\n`; three sites
//     inline a `Buf` and four `put`s before it, and a fourth (the interactive-takeover line) had the same
//     shape and joined for free. Worth 92 + 56 bytes.
//   * FOLD A FLAG AND ITS COMPANION COUNTER INTO ONE WORD. `dragging: &mut bool` plus VUGCLICK's
//     `drag_motion: &mut i32` became one `drag: &mut u32` — 0 while no button is down, else 1 + travel.
//     One argument instead of two, at every site. Worth 68 bytes.
//   * ONE CALL FOR A PAIR OF DRAWS. `draw_hud` wraps the two `draw_num` calls so neither of the two
//     overlay sites sets up an offset and a colour twice.
// Landed at 8057 with LAYER 2 in, 8073 with the hunk deleted — the two-line hunk measures NEGATIVE
// because it changes inlining around `paused`; treat both as ~8060 and ~130 bytes of headroom.
#[inline(always)]
fn futex_wait(word: *const AtomicU32, val: u32) {
    unsafe { sys3(SYS_FUTEX, word as u64, 0, val as u64) };
}
#[inline(always)]
fn futex_wake(word: *const AtomicU32, n: u32) {
    unsafe { sys3(SYS_FUTEX, word as u64, 1, n as u64) };
}
/// VUGPAUSE-2: release any worker parked on `PHASE`. Out of line — see the size note above.
#[inline(never)]
fn wake_phase() {
    futex_wake(core::ptr::addr_of!(PHASE), 2);
}
/// VUGPAUSE-2: retire the worker pool — publish the exit sentinel and release anyone parked on it. Both
/// callers (the barrier's retirement arm and the normal end of the run) mean exactly this, and folding
/// them costs nothing: the store and the wake are inseparable now that a worker can be asleep on the word.
#[inline(never)]
fn phase_exit() {
    PHASE.store(PHASE_EXIT, Ordering::Release);
    wake_phase();
}
#[inline(always)]
fn input_poll() -> u64 {
    unsafe { sys0(SYS_INPUT_POLL) }
}
/// VUGPAUSE-2: block until this process's input ring may be non-empty. Never called from the rendering
/// path — only from the idle path, where the alternative was `sys_yield` in a runnable spin.
///
/// VUGPARK-FALLBACK — THE RETURN VALUE IS NOW READ, BECAUSE ON x86 IT IS AN ERROR.
///
/// `SYS_INPUT_WAIT` (28) is implemented on aarch64 and NOT on x86_64: that dispatcher's default arm
/// answers `-ENOSYS` (-38) having done nothing at all. Discarding the return therefore turned the one
/// call this program makes to STOP burning a core into a no-op, and the idle path — a `continue` back
/// to the top of the same loop — degenerated into a tight busy spin. Boot AJ measured what that costs
/// once the compositor stops charging for a suppressed present: ~42,000 present attempts per second
/// per hidden window, `comp_rate=0.0/s`, verdict `STARVED`.
///
/// So: check it, and on ANY negative fall back to a real timed park. `SYS_SLEEP_MS` exists on both
/// arches and is a genuine tick sleep on metal, so the idle vug leaves the run queue either way. 16 ms
/// is one frame at 60 Hz — long enough that the park is a park, short enough that a keystroke arriving
/// the instant after the sleep begins is acted on within a frame, which is below the threshold at which
/// a human calls an app unresponsive.
///
/// This is DELIBERATELY NOT the real fix. `SYS_INPUT_WAIT` on x86 (the aarch64 park + the router's wake
/// seam) is a separate arc; when it lands, this call starts returning 0 and the fallback goes cold on
/// its own with nothing to remove. Arch-neutral by construction: on aarch64 the syscall succeeds, the
/// branch is never taken, and the observable behaviour of this program is exactly what it is today.
///
/// `#[inline(never)]`, AND IT IS LOAD-BEARING — this was `#[inline(always)]`. `VUG-X86.ELF` is gated at
/// a hard 16384 bytes and `.text` sits within tens of bytes of the 8 KiB boundary at which `.bss` moves
/// to the next page and the FILE grows by 4096 in one step (12568 -> 16664, i.e. straight through the
/// ceiling). Measured, this arc's two hunks: `always` 8165 -> 8237 `.text`, over the boundary and the
/// gate fails; `never` 8141, twenty-four bytes UNDER the untouched program. Same phenomenon the SIZE
/// note above `futex_wait` records — the cost is inlining pressure in the frame loop, not the code
/// written here. Nothing about correctness depends on it; the image ceiling does.
#[inline(never)]
fn input_wait() {
    /// One 60 Hz frame — the fallback park's period. See the note above.
    const FALLBACK_PARK_MS: u64 = 16;
    let rc = unsafe { sys0(SYS_INPUT_WAIT) };
    if rc >> 63 != 0 {
        unsafe { sys1(SYS_SLEEP_MS, FALLBACK_PARK_MS) };
    }
}

/// FBCON-DMG: present SOURCE rows `[y0, y1)` of `win`, with the MANDATORY whole-box fallback.
///
/// Returns exactly what a present returns — 0, or a negative errno with bit 63 set — so every call site
/// keeps checking the result the way it already does (`rc >> 63 != 0` -> `stall_witness`).
///
/// THE FALLBACK IS THE POINT, and its shape is deliberately the dullest one available: try the banded
/// verb; on ANY negative return, fall through to the byte-for-byte call this program made before this
/// function existed. Nothing is latched, nothing is remembered, no ordering changes, and nothing about
/// what has been drawn depends on which branch ran. So on aarch64 — where 33 is undefined and the
/// dispatcher's default arm answers `-ENOSYS` (-38) having done nothing else — the observable behaviour
/// of this program is what it is today, plus one syscall that returns -38 and touches nothing. It is NOT
/// an error path: an older kernel is not a failure, it is a kernel without this verb.
///
/// Rows are the SURFACE's own, not the panel's, and `[y0, y1)` is half-open. The kernel refuses an empty,
/// inverted or over-tall band with `-EINVAL` rather than quietly widening it, so a mistake in a caller's
/// damage arithmetic shows up as a whole-box present here (via this fallback) instead of hiding.
#[inline(always)]
fn present_rows(win: u64, y0: u32, y1: u32) -> u64 {
    let rc = unsafe { sys3(SYS_WIN_PRESENT_ROWS, win, y0 as u64, y1 as u64) };
    if rc >> 63 == 0 {
        return rc;
    }
    unsafe { sys1(SYS_WIN_PRESENT, win) }
}

// ---------------------------------------------------------------------------------------------
// Fixed-point maths (Q16.16), reimplemented from kernel vug.rs. No float.
// ---------------------------------------------------------------------------------------------
type Fx = i32;
const ONE: Fx = 1 << 16;

/// sin(theta) in Q16.16, theta in brads (256 brads = one turn). Verbatim from vug.rs::SIN.
static SIN: [Fx; 256] = [
    0, 1608, 3216, 4821, 6424, 8022, 9616, 11204, 12785, 14359, 15924, 17479, 19024, 20557, 22078,
    23586, 25080, 26558, 28020, 29466, 30893, 32303, 33692, 35062, 36410, 37736, 39040, 40320,
    41576, 42806, 44011, 45190, 46341, 47464, 48559, 49624, 50660, 51665, 52639, 53581, 54491,
    55368, 56212, 57022, 57798, 58538, 59244, 59914, 60547, 61145, 61705, 62228, 62714, 63162,
    63572, 63944, 64277, 64571, 64827, 65043, 65220, 65358, 65457, 65516, 65536, 65516, 65457,
    65358, 65220, 65043, 64827, 64571, 64277, 63944, 63572, 63162, 62714, 62228, 61705, 61145,
    60547, 59914, 59244, 58538, 57798, 57022, 56212, 55368, 54491, 53581, 52639, 51665, 50660,
    49624, 48559, 47464, 46341, 45190, 44011, 42806, 41576, 40320, 39040, 37736, 36410, 35062,
    33692, 32303, 30893, 29466, 28020, 26558, 25080, 23586, 22078, 20557, 19024, 17479, 15924,
    14359, 12785, 11204, 9616, 8022, 6424, 4821, 3216, 1608, 0, -1608, -3216, -4821, -6424, -8022,
    -9616, -11204, -12785, -14359, -15924, -17479, -19024, -20557, -22078, -23586, -25080, -26558,
    -28020, -29466, -30893, -32303, -33692, -35062, -36410, -37736, -39040, -40320, -41576, -42806,
    -44011, -45190, -46341, -47464, -48559, -49624, -50660, -51665, -52639, -53581, -54491, -55368,
    -56212, -57022, -57798, -58538, -59244, -59914, -60547, -61145, -61705, -62228, -62714, -63162,
    -63572, -63944, -64277, -64571, -64827, -65043, -65220, -65358, -65457, -65516, -65536, -65516,
    -65457, -65358, -65220, -65043, -64827, -64571, -64277, -63944, -63572, -63162, -62714, -62228,
    -61705, -61145, -60547, -59914, -59244, -58538, -57798, -57022, -56212, -55368, -54491, -53581,
    -52639, -51665, -50660, -49624, -48559, -47464, -46341, -45190, -44011, -42806, -41576, -40320,
    -39040, -37736, -36410, -35062, -33692, -32303, -30893, -29466, -28020, -26558, -25080, -23586,
    -22078, -20557, -19024, -17479, -15924, -14359, -12785, -11204, -9616, -8022, -6424, -4821,
    -3216, -1608,
];

#[inline(always)]
fn fsin(brad: i32) -> Fx {
    SIN[(brad & 0xFF) as usize]
}
#[inline(always)]
fn fcos(brad: i32) -> Fx {
    SIN[((brad + 64) & 0xFF) as usize]
}
#[inline(always)]
fn fmul(a: Fx, b: Fx) -> Fx {
    (((a as i64) * (b as i64)) >> 16) as Fx
}

// ---------------------------------------------------------------------------------------------
// The crystal: an elongated hexagonal bipyramid (a quartz point) — 14 vertices, reimplemented from
// vug.rs. Wireframe: 30 edges.
// ---------------------------------------------------------------------------------------------
const APEX: Fx = 88474; // 1.35
const TY: Fx = 32768; //   0.50 — half prism height
const RING: [(Fx, Fx); 6] = [
    (52429, 0),
    (26214, 45405),
    (-26214, 45405),
    (-52429, 0),
    (-26214, -45405),
    (26214, -45405),
];

/// Base (un-rotated) crystal vertices as (x, y, z) Q16.16 triples.
fn crystal_vertices() -> [(Fx, Fx, Fx); 14] {
    let mut v = [(0, 0, 0); 14];
    v[0] = (0, APEX, 0); // top apex
    let mut i = 0;
    while i < 6 {
        v[1 + i] = (RING[i].0, TY, RING[i].1); // top ring
        v[7 + i] = (RING[i].0, -TY, RING[i].1); // bottom ring
        i += 1;
    }
    v[13] = (0, -APEX, 0); // bottom apex
    v
}

/// The 30 wireframe edges (vertex index pairs).
static EDGES: [(u8, u8); 30] = [
    // top apex -> top ring
    (0, 1), (0, 2), (0, 3), (0, 4), (0, 5), (0, 6),
    // top ring loop
    (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (6, 1),
    // vertical prism edges
    (1, 7), (2, 8), (3, 9), (4, 10), (5, 11), (6, 12),
    // bottom ring loop
    (7, 8), (8, 9), (9, 10), (10, 11), (11, 12), (12, 7),
    // bottom apex -> bottom ring
    (13, 7), (13, 8), (13, 9), (13, 10), (13, 11), (13, 12),
];

// ---------------------------------------------------------------------------------------------
// Surface geometry + palette.
// ---------------------------------------------------------------------------------------------
// WC-C: the crystal renders into a 128x128 WINDOW surface (SYS_WIN_CREATE), not the 32x32 compat page.
// 128x128 is `boot::FB_WIN_MAX_W/H` — exactly one 64 KiB window slot — and is 4x the linear resolution
// the compat path allowed, so the wireframe is drawn rather than approximated. FOCAL scales with it (6 ->
// 24 px/unit) so the crystal occupies the SAME fraction of its surface as before; the visible change is
// sharpness, not framing.
const SW: i32 = 128; // surface width  (px)
const SH: i32 = 128; // surface height (px)
const STRIDE: usize = 512; // ARGB8888 row stride (bytes)
const FOCAL: i32 = 24; // pixels-per-unit at the crystal's centre depth
const BG: u32 = 0xFF1E_1E1E; // opaque Can-Am dark grey
const EDGE: u32 = 0xFFC9_A6E8; // opaque paler lilac seam

// ---------------------------------------------------------------------------------------------
// Shared state (same address space across parent + both workers; the phase/done words carry the
// release/acquire handoff, so the plain PX/PY writes the parent makes before the Release store on
// PHASE are visible to a worker after its Acquire load of PHASE).
// ---------------------------------------------------------------------------------------------
static PHASE: AtomicU32 = AtomicU32::new(0); // parent publishes frame+1; workers yield-poll; MAX = exit
static DONE: AtomicU32 = AtomicU32::new(0); // workers bump on arrival; parent futex-waits for 2
static SURF: AtomicU64 = AtomicU64::new(0); // surface VA (parent sets before spawning workers)
static mut PX: [i32; 14] = [0; 14]; // projected pixel X per vertex
static mut PY: [i32; 14] = [0; 14]; // projected pixel Y per vertex

const PHASE_EXIT: u32 = u32::MAX;
/// VUGPAUSE-2: `SYS_YIELD` passes a worker spends polling `PHASE` before it parks on it. Sized to be
/// unreachable on the rendering path — a worker waits one projection plus one present between frames, a
/// handful of passes even on a loaded machine — and trivially reachable on the idle path, where the parent
/// stops releasing frames entirely. Erring long is the safe direction: an over-long spin costs an idling
/// vug a few milliseconds of yielding ONCE per idle interval, while too short a spin puts a park/wake pair
/// in the middle of every rendered frame, which is the shape that failed the checksum run.
const WORKER_SPIN_YIELDS: u32 = 4096;
/// VUG-PACE: the PARENT's mirror of `WORKER_SPIN_YIELDS` — `SYS_YIELD` passes the frame barrier spends
/// polling `DONE` before it parks on it. Two orders smaller than the worker's on purpose: this budget is
/// spent EVERY frame on a healthy run rather than once per idle interval, so it is sized as a latency
/// threshold — enough passes that an arrival which is merely a raster away is never parked for, few enough
/// that a genuinely contended wait reaches the park after a bounded handful of syscalls. Re-armed per frame,
/// so nothing about one frame's contention is carried into the next.
const BARRIER_SPIN_YIELDS: u32 = 64;
const AUTO_FRAMES: u32 = 300; // deterministic QEMU path length (used only while no input ever arrives)
// VUGLIFE: the interactive budget. It is a real deadline ONLY in fixture mode (a foreground launch —
// `run`, and every battery leg); a detached/desktop vug waives it once and runs unbounded.
const INTERACTIVE_CAP: u32 = 36000; // interactive frame budget (fixture mode); waived when detached

// Drag-rotate sensitivity (UVUG-4). The kernel game-mode (vug.rs) maps pointer motion 1 px = 1 brad
// with no scaling; Peter found that too twitchy. The panel is ~1920 px wide (mailbox FALLBACK_W), so we
// scale pointer delta down to make a full-panel drag ≈ one revolution (256 brads over ~2048 px):
// DRAG_DIV = 8 gives 256 brads per 2048 px. Each per-frame step is clamped so one large HID delta can't
// spin the crystal past a quarter-turn in a single frame.
const DRAG_DIV: i32 = 8; // px → brad divisor (full-panel drag ≈ one revolution)
const DRAG_CLAMP: i32 = 64; // max |brad| a single frame's drag may contribute per axis

// ---------------------------------------------------------------------------------------------
// Rasterisation (worker side).
// ---------------------------------------------------------------------------------------------
#[inline(always)]
unsafe fn put_px(surf: *mut u8, x: i32, y: i32, color: u32) {
    if x < 0 || x >= SW || y < 0 || y >= SH {
        return;
    }
    let off = (y as usize) * STRIDE + (x as usize) * 4;
    (surf.add(off) as *mut u32).write_volatile(color);
}

/// Bresenham line, plotting only points whose row is in [y_lo, y_hi) (the worker's band). Off-band and
/// off-surface points are skipped, so a worker never writes outside its half.
unsafe fn draw_line(surf: *mut u8, mut x0: i32, mut y0: i32, x1: i32, y1: i32, y_lo: i32, y_hi: i32, color: u32) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if y0 >= y_lo && y0 < y_hi {
            put_px(surf, x0, y0, color);
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Render one worker's band for a frame: clear the band to BG, then draw every crystal edge clipped to
/// the band from the shared projected coordinates.
unsafe fn render_band(surf: *mut u8, y_lo: i32, y_hi: i32) {
    // Clear the band.
    let mut y = y_lo;
    while y < y_hi {
        let row = surf.add((y as usize) * STRIDE) as *mut u32;
        let mut x = 0usize;
        while x < SW as usize {
            row.add(x).write_volatile(BG);
            x += 1;
        }
        y += 1;
    }
    // Draw edges (read the parent-published projection).
    let px = &*core::ptr::addr_of!(PX);
    let py = &*core::ptr::addr_of!(PY);
    let mut i = 0usize;
    while i < EDGES.len() {
        let (a, b) = EDGES[i];
        let (a, b) = (a as usize, b as usize);
        draw_line(surf, px[a], py[a], px[b], py[b], y_lo, y_hi, EDGE);
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Worker thread entry. arg 0 = top half (rows 0..16), arg 1 = bottom half (rows 16..32).
// ---------------------------------------------------------------------------------------------
#[no_mangle]
extern "C" fn uvug_worker(arg: usize) -> ! {
    let surf = SURF.load(Ordering::Acquire) as *mut u8;
    let (y_lo, y_hi) = if arg == 0 { (0, SH / 2) } else { (SH / 2, SH) };
    let mut last: u32 = 0;
    loop {
        // Wait for the parent to release the next frame.
        //
        // VUGPAUSE-2 — SPIN, THEN PARK. The `SYS_YIELD` poll is the OTHER half of the idle fleet's load,
        // and the larger half by headcount: a vug is one parent and TWO workers, and while the parent
        // idled on `SYS_YIELD` both workers idled on this loop. Blocking the parent alone would have left
        // two thirds of the residue running.
        //
        // The SPIN stays, and the arc measured why. The first cut made this a pure `futex_wait`, on the
        // symmetry argument that the ARRIVAL direction has always been a real futex on this same QEMU. The
        // 300-frame checksum run then FAILED — `:: EXEC-UVUG: … did not exit in time ::`, with all three
        // tasks parked at the kill. Whatever the mechanism (the barrier note at the top of this file blames
        // raspi4b's missing Group-1 timer for making the release direction the fragile one), the release
        // direction empirically cannot be a bare park there. So it is not one: `WORKER_SPIN_YIELDS` passes
        // of the original poll come first, and the rendering path never gets past them — between two frames
        // a worker waits only for the parent's projection and present. The park is reached ONLY when the
        // parent has stopped releasing frames altogether, which is exactly the VUGPAUSE/VUGMIN idle this
        // arc is about, and it is reached once per idle interval rather than once per frame.
        //
        // Lost-wakeup-safe: `futex_wait` compares `PHASE` against the value just read, under the same
        // bucket lock the parent's `wake_phase` takes, so a release landing between the load and the wait
        // returns `Mismatch` instead of parking. Kill-safe: this is KILLBOUND's futex, the same one the
        // arrival barrier already blocks on. Every parent-side `PHASE` store — frame release, worker
        // retirement, and the exit sentinel — is followed by `wake_phase`, which is what makes both the
        // barrier and the final `SYS_THREAD_JOIN` terminate.
        let mut spins: u32 = WORKER_SPIN_YIELDS;
        let p = loop {
            let p = PHASE.load(Ordering::Acquire);
            if p != last {
                break p;
            }
            if spins != 0 {
                spins -= 1;
                sys_yield();
            } else {
                futex_wait(core::ptr::addr_of!(PHASE), p);
            }
        };
        last = p;
        if p == PHASE_EXIT {
            unsafe { sys0(SYS_THREAD_EXIT) };
            loop {
                core::hint::spin_loop();
            }
        }
        unsafe { render_band(surf, y_lo, y_hi) };
        // Arrive: atomically bump `done`, then FUTEX WAKE the parent.
        DONE.fetch_add(1, Ordering::Release);
        futex_wake(core::ptr::addr_of!(DONE), 1);
    }
}

// ---------------------------------------------------------------------------------------------
// Input decode (SYS_INPUT_POLL packed u64; see docs userspace.md ELF-5).
// ---------------------------------------------------------------------------------------------
// ABIFREEZE: the packed-event type tags, imported from the crate the kernels pack them with.
use una_abi::{
    INPUT_EV_BUTTON as EV_BUTTON, INPUT_EV_KEY_DOWN as EV_KEYDOWN, INPUT_EV_KEY_UP as EV_KEYUP,
    INPUT_EV_MOUSE_REL as EV_MOUSE_REL,
};

// Held-state bits.
const H_YAW_L: u32 = 1 << 0;
const H_YAW_R: u32 = 1 << 1;
const H_PIT_U: u32 = 1 << 2;
const H_PIT_D: u32 = 1 << 3;
const H_ZOOM_IN: u32 = 1 << 4;
const H_ZOOM_OUT: u32 = 1 << 5;
/// CLICK-ONE: SPACE rides the SAME held word as the motion keys, for one reason only — typematic
/// repeat suppression. `pal.rs`'s engine synthesises a KeyDown every `RATE_MS` (40 ms) once a key has
/// been down `DELAY_MS` (400 ms), and a toggle driven off raw KeyDowns would flip 25 times a second
/// under a resting thumb. Held state tells a TRUE press edge from a repeat for free, which is what
/// `held` already exists to do.
const H_PAUSE: u32 = 1 << 6;
/// The held bits that mean MOTION. `H_PAUSE` is deliberately outside it: a held SPACE must not read as
/// manual control, or holding the pause key would silently stop the idle tumble of an unpaused vug.
const H_MOTION: u32 = H_YAW_L | H_YAW_R | H_PIT_U | H_PIT_D | H_ZOOM_IN | H_ZOOM_OUT;

/// VUGPAUSE-KEYUP: has this program EVER been delivered an `EV_KEYUP`? One-way, 0 -> 1. It is the
/// evidence that decides whether the REST of the held word can be trusted — see the note at the
/// `H_PAUSE` toggle in `drain_input`.
///
/// It rides the held word instead of getting a static of its own because a static costs this program
/// 4096 bytes — `.bss` gains a page, measured — and `VUG-X86.ELF` is built against a HARD 16384-byte
/// EL0 user window that the 12568-byte binary has under 4 KiB of headroom in. A spare bit in a `u32`
/// already passed by `&mut` everywhere it is needed costs nothing.
///
/// It is NOT a key bit and must never be treated as one: `key_bit` cannot return it, `H_MOTION`
/// excludes it, and the ONE place that clears the word wholesale — `EV_BUTTON`'s hot-unplug net —
/// preserves it explicitly. A click must not make this program forget what it learned about its
/// keyboard.
const H_SAW_KEYUP: u32 = 1 << 7;

// HID-KEYS arrow C0 codes (see vug.rs), ESC, and CLICK-ONE's pause key.
// ABIFREEZE: the arrow block's C0 codes are the kernel input router's, imported.
use una_abi::{
    KEY_DOWN as K_DOWN, KEY_ESC as K_ESC, KEY_LEFT as K_LEFT, KEY_RIGHT as K_RIGHT, KEY_UP as K_UP,
};
/// CLICK-ONE: SPACE toggles pause. Chosen because it is UNBOUND here — `key_bit` maps WASD/arrows,
/// Q/E and the +/- family and nothing else, and ESC is handled ahead of it — so no existing gesture
/// changes meaning. It is also the conventional pause key, and the xHCI table delivers it as plain
/// ASCII 0x20 (keycode 0x2C, shifted and unshifted alike), so no modifier state is involved.
const K_SPACE: u8 = b' ';

fn key_bit(k: u8) -> u32 {
    let k = k.to_ascii_lowercase();
    match k {
        b'a' | K_LEFT => H_YAW_L,
        b'd' | K_RIGHT => H_YAW_R,
        b'w' | K_UP => H_PIT_U,
        b's' | K_DOWN => H_PIT_D,
        b'e' | b'+' | b'=' => H_ZOOM_IN,
        b'q' | b'-' | b'_' => H_ZOOM_OUT,
        K_SPACE => H_PAUSE,
        _ => 0,
    }
}

/// UVUG-9 — the per-frame cap on how many input events one frame may consume.
///
/// ROOT CAUSE (P54b freeze). The UVUG-8 drain was an UNBOUNDED `loop { poll() }`: it ran until the ring
/// reported empty, with no bound whatsoever. That is the only phase of this program's frame loop that can
/// call `SYS_INPUT_POLL` indefinitely WITHOUT reaching the present, and the kernel's own UVUG-8r2
/// instrumentation proves that is exactly where P54b sat: the run held its takeover suspension for the full
/// `TAKEOVER_SUSPEND_MAX_SECS` (60 s), which requires the heartbeat — stamped ONLY by `sys_input_poll` — to
/// have stayed fresher than `TAKEOVER_STALE_SECS` (2 s) on every pass, while `EL0_FOCUSED_PRESENT_COUNT` —
/// bumped by `sys_fb_present` under the IDENTICAL focus predicate — never moved once. Polling forever,
/// presenting never, is not a state any other phase of this loop can occupy. Hence: the drain spun.
///
/// A drain that outlives its frame is a rendering freeze even though nothing is deadlocked: the workers stay
/// parked on `phase`, the surface keeps its last frame, the screen shows a static crystal, and the kernel's
/// no-render cap eventually ends the run. Bounding the drain converts that hard freeze into, at worst, input
/// latency — the leftovers are simply consumed by the NEXT frame's drain, which is what a frame budget is for.
///
/// The cap is 2x the kernel's `INPUT_RING_CAP` (32), so a frame can always empty a completely full ring plus a
/// full ring's worth of concurrent arrivals; hitting it means the producer is outrunning a tight EL0 syscall
/// loop, which no HID device can legitimately do. That anomaly is witnessed (`[uvug9] drain saturated`) rather
/// than absorbed silently — it names the remaining upstream suspect for P55 instead of hiding it.
const MAX_DRAIN_PER_FRAME: u32 = 64;

/// Accumulated input for one frame.
#[derive(Default)]
struct FrameInput {
    any: bool,      // any event at all this frame (arms interactive mode)
    exit_key: bool, // ESC pressed
    /// CLICK-ONE: TRUE SPACE press edges this frame (repeats excluded — see `H_PAUSE`), each one a
    /// pause toggle. Counted rather than latched so the consumer can take the PARITY: two presses
    /// inside one frame must leave the state where it started, and a bool would turn that into a
    /// spurious toggle. This replaces VUGCLICK's `clicks`: a click is no longer app input at all
    /// (see the click-semantics note above `_start`'s frame loop).
    pause_keys: u32,
    /// CLICK-PLAIN: CLICKS this frame — press+release pairs whose travel stayed under `CLICK_THRESH`.
    /// A click is app input again (the router delivers the focus-changing press since CLICK-PLAIN), and
    /// this is what the program does with it. Counted, not latched, for the same reason `pause_keys` is.
    clicks: u32,
    mdx: i32, // summed relative mouse dx while dragging
    mdy: i32, // summed relative mouse dy while dragging
    /// UVUG-9: the drain hit `MAX_DRAIN_PER_FRAME` with the ring still non-empty — the freeze signature.
    saturated: bool,
}

/// Drain this frame's queued input events. Updates `held`/`drag` in place and returns the
/// per-frame accumulation. BOUNDED at `MAX_DRAIN_PER_FRAME` events (see that constant for the P54b root
/// cause): whatever is left stays in the ring for the next frame, so the render/present half of the loop is
/// always reached.
fn drain_input(held: &mut u32, drag: &mut u32) -> FrameInput {
    /// CLICK-PLAIN: the greatest `drag` word (= 1 + accumulated |dx|+|dy| since the press) that still
    /// counts as a CLICK rather than a drag. VUGCLICK's original 6 px of slack, restored unchanged — a
    /// hand that means to click a 128 px window moves a pixel or two doing it, and a hand that means to
    /// rotate moves far more.
    const CLICK_THRESH: u32 = 6;
    let mut fi = FrameInput::default();
    let mut budget = MAX_DRAIN_PER_FRAME;
    loop {
        if budget == 0 {
            fi.saturated = true;
            break; // frame budget spent — the rest waits for the next frame (never spin here)
        }
        budget -= 1;
        let ev = input_poll();
        if ev >> 63 != 0 {
            break; // -EAGAIN: ring empty
        }
        fi.any = true;
        let ty = una_abi::input_ev_type(ev);
        let lo = una_abi::input_ev_payload(ev);
        match ty {
            EV_KEYDOWN => {
                let k = (lo & 0xFF) as u8;
                if k == K_ESC {
                    fi.exit_key = true;
                } else {
                    let b = key_bit(k);
                    // CLICK-ONE: SPACE toggles on the TRUE press edge — a bit that is not already set.
                    // Every later KeyDown for a key still down is a typematic repeat and is absorbed by
                    // the `|=` below, exactly as it always has been for the motion keys.
                    //
                    // VUGPAUSE-KEYUP — and the edge test is SUSPENDED while this program has never been
                    // shown a release. Boot AJ is what that costs otherwise: the kernel's EHCI decoder
                    // emitted presses only, so `H_PAUSE` was set by the first SPACE and NOTHING could
                    // ever clear it (`EV_KEYUP` below is the sole writer that does). Pause latched on,
                    // `frozen` latched on, and the vug had to be killed. The decoder is fixed in the
                    // same arc, and this is the ring-3 half of the belt: an app should not be one
                    // driver bug away from a state its operator cannot leave.
                    //
                    // WHY IT IS CONDITIONED ON `H_SAW_KEYUP` RATHER THAN JUST DELETING THE `!*held`
                    // TERM. Deleting it would toggle pause 25 times a second on aarch64, where
                    // `pal.rs`'s typematic engine SYNTHESISES a KeyDown every 40 ms under a resting
                    // thumb — the exact reason the edge test was written (see `H_PAUSE`). One bit of
                    // evidence tells the two worlds apart with no `cfg` and no guesswork: a program that
                    // has received even ONE release is on a path whose releases work, so its held state
                    // is true and the edge test is right; a program that has never received one cannot
                    // trust `*held` at all, and a level-free toggle is strictly better than a wedge.
                    // The bit is one-way — a keyboard that emits releases does not stop — so the
                    // fallback ends for good at the first release and never oscillates.
                    //
                    // The suspension is implemented at the BOTTOM of this function rather than as a
                    // second term here: retiring `H_PAUSE` from the held word once per drain is one
                    // test-and-mask outside the event loop, and this program is 45 bytes of `.text`
                    // from a hard 16 KiB image ceiling (see `H_SAW_KEYUP`). The behaviour differs only
                    // for two SPACE presses inside ONE drain, which no human hand produces.
                    if b & !*held & H_PAUSE != 0 {
                        fi.pause_keys += 1;
                    }
                    *held |= b;
                }
            }
            EV_KEYUP => {
                // VUGPAUSE-KEYUP: the evidence bit. Set from ANY release, mapped or not — the question
                // it answers is "does this input path deliver releases at all?", which a release for a
                // key this program does not bind answers just as well as one for a key it does.
                *held |= H_SAW_KEYUP;
                let k = (lo & 0xFF) as u8;
                let b = key_bit(k);
                if b != 0 {
                    *held &= !b;
                }
            }
            EV_BUTTON => {
                let mask = lo & 0xFF;
                // CLICK-PLAIN: the click/drag discrimination CLICK-ONE deleted is back, because a
                // click is app input again. A press opens the gesture, a release closes it, and the
                // accumulated pointer travel decides which gesture it was: under `CLICK_THRESH` it is a
                // CLICK, at or over it it was a drag (which has already done its rotating, below).
                //
                // SIZE (see the SIZE note — "fold a new counter into an existing one"): the `dragging`
                // FLAG and VUGCLICK's separate `drag_motion` accumulator are ONE word. `drag` is 0 while
                // no button is down and `1 + travel` while one is, so "is a drag open?" is `!= 0` and
                // "was it a click?" is `<= CLICK_THRESH` — one `u32` through one `&mut`, in place of a
                // bool and an i32 through two.
                if mask != 0 {
                    *drag = 1;
                    // Press edge also clears held keys — the vug.rs hot-unplug net. VUGPAUSE-KEYUP:
                    // every KEY bit goes, and the evidence bit stays. What the net exists to undo is a
                    // key whose release never arrived; what this program has learned about whether
                    // releases arrive AT ALL is not held state and a click is no evidence against it.
                    *held &= H_SAW_KEYUP;
                } else {
                    if *drag <= CLICK_THRESH {
                        fi.clicks += 1;
                    }
                    *drag = 0;
                }
            }
            EV_MOUSE_REL => {
                let dx = ((lo >> 16) & 0xFFFF) as u16 as i16 as i32;
                let dy = (lo & 0xFFFF) as u16 as i16 as i32;
                if *drag != 0 {
                    *drag += (dx.abs() + dy.abs()) as u32;
                    fi.mdx += dx;
                    fi.mdy += dy;
                }
            }
            _ => {}
        }
    }
    // VUGPAUSE-KEYUP: on an input path that has never delivered a release, `H_PAUSE` is not allowed to
    // persist past the drain that set it — nothing else would ever clear it, and a latched bit turns
    // the press-edge test above into a one-way switch (Boot AJ: pause on, never off, kill the app).
    // Retiring it here rather than suppressing the edge test per event costs one mask per drain and
    // keeps the aarch64 typematic absorption exactly as it is: there, `H_SAW_KEYUP` is set by the first
    // key release the operator ever makes, and this line stops firing for the rest of the run.
    //
    // THE ONE RESIDUE, and its true scope — stated rather than hidden, and NARROWER on one arch than
    // the other for a reason worth knowing at a bench.
    //
    // Until the first release, a release-emitting path is indistinguishable from a release-less one,
    // so a SPACE held past `DELAY_MS` (400 ms) as the VERY FIRST key of a run would see its
    // synthesised repeats toggle pause. It costs one keystroke to leave and cannot recur — any
    // release at all, of any key, ends it for the process's lifetime.
    //
    //   * On x86 it is unreachable in the launch flow, and that is a property of the kernel rather
    //     than luck. `SYS_WIN_CREATE` grants focus to the first window while the shell is idle
    //     (`:: wc-x86: input focus -> slot N (first window, shell was idle) ::`), and x86's
    //     `user_input_set_active` deliberately does NOT drain `pal::EVENT_QUEUE`. So the RELEASE of
    //     the Enter that launched the program — tens of milliseconds behind its press, well inside a
    //     human key hold — routes into the freshly focused ring and sets this bit long before any
    //     400 ms delay could expire. (Since the keyrepeat arc x86 runs the shared typematic
    //     synthesiser too — its repeats land here and are absorbed by the same press-edge test.)
    //   * On aarch64 — the arch where the repeat engine that makes this matter actually LIVES — that
    //     mitigation does NOT transfer, and assuming it did would be the comfortable wrong answer.
    //     `user_input_set_active` there drains and DISCARDS the pre-launch queue on purpose
    //     (UVUG-8r2: `[uvug8] focus asid=N — discarded K pre-launch event(s) from EVENT_QUEUE`),
    //     precisely so the launch keystroke is not mistaken for in-app interaction — and the
    //     launching Enter's release is one of the events it throws away. So the first release this
    //     program sees on the Pi is a genuinely in-app one, and the residue stands exactly as written
    //     above: first key of the run, held past 400 ms, pause flickers and settles on the parity of
    //     the repeat count. Bounded, self-ending, one tap to correct.
    if *held & H_SAW_KEYUP == 0 {
        *held &= !H_PAUSE;
    }
    fi
}

// ---------------------------------------------------------------------------------------------
// UVUG-9 — the per-frame STALL WITNESS.
//
// P54b showed a rendering freeze with no crash, no fault and no deadlock report: the crystal stopped moving
// while the program kept polling. From the outside, "stopped presenting" is all you can see; from in here we
// can say WHICH PHASE of the frame stopped making progress. The loop has exactly three phases that can fail
// to complete, and this witness names them:
//
//   [uvug9] stall frame=<n> phase=poll drained=<64> — the drain hit its frame budget with the ring still
//       non-empty. The P54b signature. Post-fix this is no longer fatal (the frame proceeds), so the line is a
//       DIAGNOSIS of a runaway upstream producer, not a symptom of this program.
//   [uvug9] stall frame=<n> phase=barrier done=<0|1> — the frame-barrier wait burned its pass budget with
//       fewer than its live workers arrived; `done` says how many did (0 = neither worker ran, 1 = one worker
//       wedged). Distinguishes a worker-side wedge from an input-side one. VUGGUARD: this is now a DEADLINE
//       as well as a witness — on the pass that prints it, the parent retires the worker pool and takes every
//       band inline, so the line marks the ONE frame that presents partially-stale content, not a permanent
//       state. A worker that was merely slow rather than wedged sees PHASE_EXIT and leaves.
//   [uvug9] stall frame=<n> phase=present rc=<errno> — SYS_WIN_PRESENT returned an error. UVUG-8 IGNORED this
//       syscall's return entirely, so a present that started failing mid-run would have looked exactly like a
//       freeze: frames advancing, nothing on screen, and the kernel's no-render cap firing. Now it is visible.
//
// GATING. There is no env/knob channel into EL0, so each phase self-gates on its own ANOMALY and latches after
// the first report. Consequences: a healthy run prints nothing new, and — decisive for the gates — the
// deterministic QEMU auto path (no HID, no events, workers always arrive, present always succeeds) reaches no
// anomaly at all, so its 300-frame surface checksum is untouched.
//
// The barrier budget is expressed in PASSES, not milliseconds: EL0 has no clock syscall (SYS_GETINFO's tick
// field would need a copy_to_user round trip per pass, which would itself distort the thing being measured),
// and a pass budget is both deterministic and precisely what "made no progress" means here. LIMITATION, stated
// rather than papered over: this catches a barrier that is SPINNING (futex_wait returning -EAGAIN on a value
// mismatch), not one PARKED forever on a lost wakeup. A parked parent cannot execute a witness at all, and the
// kernel's futex compares `*uaddr` against `val` under the same bucket lock `futex_wake` takes, so that park is
// race-free by construction — a lost wakeup here is refuted in the kernel, not monitored from EL0.
//
// VUGGUARD, on that limitation: P60's wedge WAS a park, and no pass budget could ever have caught it — the
// parent was blocked in `futex_wait` on a `done` count that no living thread would ever bump, because the
// spawns had been refused and it had not looked. That class is closed STRUCTURALLY, not by monitoring: the
// barrier's target is the number of workers that exist, so with none it is never entered. The budget below
// remains for the narrower case it can see — a thread that exists and stops arriving.
const BARRIER_PASS_BUDGET: u32 = 1 << 20;

/// One-shot latches, one per phase, so a witness fires at most once per program run.
static W_POLL: AtomicU32 = AtomicU32::new(0);
static W_BARRIER: AtomicU32 = AtomicU32::new(0);
static W_PRESENT: AtomicU32 = AtomicU32::new(0);

/// Emit one `[uvug9] stall` line: `frame`, then `tail` — the phase name and the detail label, PRE-JOINED
/// by the caller as one literal (`b"barrier done="`), followed by the detail value.
///
/// VUG-PACE, size only: the emitted bytes are unchanged. The phase name and the label used to arrive as two
/// slices and be assembled here with three more `put` calls (one for each of the separator, the label and
/// the `=`); each of those is an argument triple plus a call at a site that runs at most once per program.
/// Joining them at the literal moves the assembly to link time and pays for part of the barrier's spin.
fn stall_witness(latch: &AtomicU32, frame: u32, tail: &[u8], value: u32) {
    if latch.swap(1, Ordering::Relaxed) != 0 {
        return; // already reported this phase — never flood the serial line
    }
    let mut b = Buf::new();
    b.put(b"[uvug9] stall frame=");
    b.put_dec(frame);
    b.put(b" phase=");
    b.put(tail);
    b.put_dec(value);
    b.put(b"\n");
    b.flush();
}

// ---------------------------------------------------------------------------------------------
// VUGFPS — the on-window frames-per-second readout.
//
// The stagger observation (s1p: replacement vugs visibly outpace the originals) needs a PER-VUG
// number, and the serial line cannot carry one per frame for six windows. So each vug measures and
// draws its own rate in its top-left corner: frames presented per second, from `SYS_GETINFO`'s
// `ticks` field (the 250 Hz scheduler tick — the only EL0-reachable clock; CNTVCT_EL0 is not
// EL0-enabled). One getinfo per frame is one syscall beside the existing input poll; the displayed
// value refreshes once per second, so the digits are readable rather than flickering.
//
// CHECKSUM DISCIPLINE: the overlay is drawn ONLY when `detached || interactive` — a desktop
// (`bg`) or operator-driven vug. The FOREGROUND auto path (every fixture/battery leg, the QEMU
// 300-frame checksum witness) takes neither branch and its surface stays byte-identical.
// ---------------------------------------------------------------------------------------------
/// ABIFREEZE (divergence D1): the rate of `SYS_GETINFO`'s `ticks` field — IMPORTED, because it is
/// NOT the same number on both arches and this constant used to claim it was.
///
/// It read `const TICK_HZ: u32 = 250` with the comment "kernel scheduler tick rate", which is exactly
/// right on aarch64 and wrong on x86, where the field is filled from `arch::ticks()` at
/// `apic::TICK_HZ` = 1000 Hz (one tick per millisecond). VUG-X86.ELF therefore divided a one-second
/// frame count by 250 and drew an fps figure FOUR TIMES TOO LOW on the panel — and refreshed it four
/// times a second while the comment below still says "once per second". Neither kernel's clock moves
/// (both are shipped behaviour); the divisor now comes from the ABI and is correct on either arch.
const TICK_HZ: u32 = una_abi::GETINFO_TICK_HZ as u32;
const FPS_C: u32 = 0xFFE8_C98A; // fps digits — warm amber, same as user-stat's pid

/// 5x7 digit glyphs, one byte per row, bit 4 = leftmost column (verbatim from user-stat).
static GLYPHS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

/// Draw one digit at (x, y), 1:1 scale (5x7 px — the window is only 128 wide).
unsafe fn draw_digit(surf: *mut u8, d: usize, x: i32, y: i32, color: u32) {
    let g = &GLYPHS[d % 10];
    let mut row = 0i32;
    while row < 7 {
        let bits = g[row as usize];
        let mut col = 0i32;
        while col < 5 {
            if bits & (1 << (4 - col)) != 0 {
                put_px(surf, x + col, y + row, color);
            }
            col += 1;
        }
        row += 1;
    }
}

/// `SYS_GETINFO` -> the kernel's 250 Hz tick count, or 0 on error (a 0 delta just skips the update).
fn getinfo_ticks() -> u64 {
    let mut info = [0u64; 2]; // {pid, ticks}, #[repr(C)] — see kernel sys_getinfo
    let p = info.as_mut_ptr() as u64;
    if unsafe { sys1(SYS_GETINFO, p) } >> 63 != 0 {
        return 0;
    }
    info[1]
}

/// Widest backing box a readout ever paints: 3 digits at 6 px advance + 2 px pad.
const FPS_BOX_W: i32 = 3 * 6 + 3;
/// CLICK-PLAIN: the click counter sits immediately right of the fps box, in the same 11-row band.
const CLICK_X: i32 = FPS_BOX_W + 3;
/// CLICK-PLAIN: click-counter digits — cool cyan, so the two numbers in the corner never read as one.
const CLICK_C: u32 = 0xFF6C_D8E8;

/// FBCON-DMG: the exact SOURCE rows `draw_hud` touches — the half-open band `[HUD_Y0, HUD_Y1)`.
///
/// Derived from the painter, not guessed. `draw_hud` is two `draw_num` calls and nothing else; `draw_num`
/// clears its backing box over `while y < 11` (rows 0..=10) and then stamps digits through `draw_digit`
/// at `y = 2` with a 7-row glyph (rows 2..=8), which is strictly inside that clear. The two calls differ
/// only in `x0`/colour, so their row extent is identical. Nothing else in this program writes rows 0..11
/// on the HUD-only path. If `draw_num`'s clear height or `draw_digit`'s `y` origin ever moves, THIS
/// CONSTANT MOVES WITH IT — an under-declared band leaves stale pixels on the panel.
const HUD_Y0: u32 = 0;
const HUD_Y1: u32 = 11;

/// Draw a 1-3 digit readout (clamped to 999) at `x0` in the top band, over whatever the frame rendered.
/// Runs in the PARENT, after the frame barrier and before the present, so no worker is writing.
///
/// VUGPAUSE: the backing box is the FIXED maximum width, not the current digit count's. On the
/// rendering path the band clear wipes the corner every frame so either would do; on the VUGPAUSE idle
/// path there is no band clear, so a readout shrinking (e.g. 47 -> 0) would leave the old digit's
/// pixels stranded. Clearing the same box every time makes the overlay self-erasing. Checksum-safe:
/// this function is called only when `detached || interactive`, which the foreground auto path is not.
///
/// CLICK-PLAIN generalised this from `draw_fps` by adding `x0`/`color`, so the click counter is the same
/// code at a different offset rather than a second copy of it — the cheapest way to add a second readout
/// against the `.text` ceiling (see the SIZE note).
unsafe fn draw_num(surf: *mut u8, fps: u32, x0: i32, color: u32) {
    let v = fps.min(999);
    // VUG-PACE, size only (no behaviour change): the digit count and its leading power of ten come out of
    // ONE ladder. The old form derived `div` from `n` with a `while k < n { div *= 10 }` loop, and this
    // program pays for every instruction — see the SIZE note. The barrier's spin budget was bought here.
    let (n, mut div) = if v >= 100 {
        (3i32, 100u32)
    } else if v >= 10 {
        (2, 10)
    } else {
        (1, 1)
    };
    let mut y = 0;
    while y < 11 {
        let row = surf.add((y as usize) * STRIDE) as *mut u32;
        let mut x = x0 as usize;
        while x < (x0 + FPS_BOX_W) as usize {
            row.add(x).write_volatile(BG);
            x += 1;
        }
        y += 1;
    }
    let mut i = 0i32;
    while i < n {
        draw_digit(surf, ((v / div) % 10) as usize, x0 + 2 + i * 6, 2, color);
        div /= 10;
        i += 1;
    }
}

/// CLICK-PLAIN: one `:: UVUG: <label><n> ::` witness line.
///
/// Three call sites emit exactly this shape (`pause=` from SPACE, `click n=` from a delivered click,
/// `pause=` again from the LAYER 2 hunk), and a `Buf` plus four `put`s inlined three times is the kind of
/// duplication this program cannot afford (see the SIZE note). The trailing ` ::\n` is folded in here
/// because every caller wants it — the label is the only thing that varies.
fn say(label: &[u8], v: u32) {
    let mut b = Buf::new();
    b.put(label);
    b.put_dec(v);
    b.put(b" ::\n");
    b.flush();
}

/// CLICK-PLAIN: draw BOTH corner readouts — fps at the left, the delivered-click count beside it.
///
/// A wrapper rather than two calls at each site, and that is a SIZE decision (see the SIZE note): the
/// overlay is drawn from two places, and hoisting the pair behind one two-argument call removes a whole
/// set of argument setup (offset + colour, twice) from each of them.
unsafe fn draw_hud(surf: *mut u8, fps: u32, clicks: u32) {
    draw_num(surf, fps, 0, FPS_C);
    draw_num(surf, clicks, CLICK_X, CLICK_C);
}

/// Fold the kernel tick clock into the displayed fps, refreshing at most once per second. `ticks`/`mark`
/// are the (tick, frame) pair sampled at the last refresh; `frame` counts frames PRESENTED. Returns the
/// value to display — unchanged until a full second of ticks has passed.
///
/// VUGPAUSE calls this from the idle loop as well as from the rendering frame path, so a pause-idled vug
/// keeps its once-per-second refresh alive. Because `frame` does not advance while idled, the quotient
/// falls to 0 — the readout tells the truth (this vug is presenting nothing) instead of freezing on the
/// last rate it happened to be running at.
fn fps_refresh(ticks: &mut u64, mark: &mut u32, fps: u32, frame: u32) -> u32 {
    let now = getinfo_ticks();
    if now > *ticks {
        let dt = (now - *ticks) as u32;
        if dt >= TICK_HZ {
            // frames since last refresh, scaled to per-second at the 250 Hz tick.
            let v = ((frame.wrapping_sub(*mark) as u64 * TICK_HZ as u64 + (dt / 2) as u64) / dt as u64) as u32;
            *ticks = now;
            *mark = frame;
            return v;
        }
    } else if now != 0 && now < *ticks {
        // A stale first read (getinfo error returned 0) — resync rather than divide nonsense.
        *ticks = now;
        *mark = frame;
    }
    fps
}

// ---------------------------------------------------------------------------------------------
// Transform: rotate + project the 14 vertices into PX/PY.
// ---------------------------------------------------------------------------------------------
fn project(base: &[(Fx, Fx, Fx); 14], ay: i32, ax: i32, dist: Fx) {
    let (sy, cy) = (fsin(ay), fcos(ay));
    let (sx, cx) = (fsin(ax), fcos(ax));
    let mut i = 0usize;
    while i < 14 {
        let (vx, vy, vz) = base[i];
        // Rotate around Y then X (vug.rs::Vec3::rotate).
        let x1 = fmul(vx, cy) - fmul(vz, sy);
        let z1 = fmul(vx, sy) + fmul(vz, cy);
        let y2 = fmul(vy, cx) - fmul(z1, sx);
        let z2 = fmul(vy, sx) + fmul(z1, cx);
        let zc = (z2 + dist).max(ONE / 4); // keep depth positive
        let ppu = (FOCAL as i64) * (dist as i64) / (zc as i64);
        let sxp = SW / 2 + (((x1 as i64) * ppu) >> 16) as i32;
        let syp = SH / 2 - (((y2 as i64) * ppu) >> 16) as i32;
        unsafe {
            (*core::ptr::addr_of_mut!(PX))[i] = sxp;
            (*core::ptr::addr_of_mut!(PY))[i] = syp;
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// FNV-1a 64-bit over the whole surface (deterministic auto-path witness).
// ---------------------------------------------------------------------------------------------
fn surface_checksum(surf: *const u8) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let prime: u64 = 0x0000_0100_0000_01b3;
    let mut i = 0usize;
    while i < (SH as usize) * STRIDE {
        let byte = unsafe { surf.add(i).read_volatile() };
        h ^= byte as u64;
        h = h.wrapping_mul(prime);
        i += 1;
    }
    h
}

// ---------------------------------------------------------------------------------------------
// Tiny formatting into a byte buffer (no core::fmt — keep the text segment small).
// ---------------------------------------------------------------------------------------------
struct Buf {
    b: [u8; 96],
    n: usize,
}
impl Buf {
    fn new() -> Self {
        Buf { b: [0; 96], n: 0 }
    }
    fn put(&mut self, s: &[u8]) {
        let mut i = 0;
        while i < s.len() && self.n < self.b.len() {
            self.b[self.n] = s[i];
            self.n += 1;
            i += 1;
        }
    }
    fn put_hex64(&mut self, mut v: u64) {
        let digits = b"0123456789abcdef";
        let mut i = 0;
        while i < 16 {
            let nib = ((v >> 60) & 0xF) as usize;
            if self.n < self.b.len() {
                self.b[self.n] = digits[nib];
                self.n += 1;
            }
            v <<= 4;
            i += 1;
        }
    }
    fn put_dec(&mut self, v: u32) {
        let mut tmp = [0u8; 10];
        let mut k = 0;
        let mut x = v;
        if x == 0 {
            self.put(b"0");
            return;
        }
        while x > 0 {
            tmp[k] = b'0' + (x % 10) as u8;
            x /= 10;
            k += 1;
        }
        while k > 0 {
            k -= 1;
            let c = tmp[k];
            self.put(&[c]);
        }
    }
    fn flush(&self) {
        write_bytes(self.b.as_ptr(), self.n);
    }
}

// ---------------------------------------------------------------------------------------------
// Program entry: the parent thread. Forced first in .text so e_entry lands on it.
// ---------------------------------------------------------------------------------------------
#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let base = _start as *const () as u64; // the window base B (entry VA == base, e_entry offset 0)

    // WC-C: create a 128x128 WINDOW instead of mapping the 32x32 compat surface. `SYS_WIN_CREATE`
    // returns the window ID (>= 0) and maps the negotiated 16-page surface slot; the surface VA is the
    // window region's slot 0, at a FIXED offset from the program's own window base (base + 0x5000 —
    // `boot::fb_info_va() + FB_INFO_SIZE`), which is the same VA `SYS_FB_MAP` used to return. Nothing is
    // guessed: the kernel publishes the geometry in the RO info page at base + 0x4000, and the surface
    // slot layout is part of the window ABI (userspace.md).
    //
    // Fail-closed: a negative return means no window (no free slot, no per-process FB region). There is
    // nothing to draw into, so exit rather than write to an unmapped VA and take a fatal EL0 abort.
    let win = unsafe { sys2(SYS_WIN_CREATE, SW as u64, SH as u64) };
    if win >> 63 != 0 {
        let mut b = Buf::new();
        b.put(b":: UVUG: SYS_WIN_CREATE failed ::\n");
        b.flush();
        exit(1);
    }
    // VUG-BG: read the process-flags word the kernel publishes in the RO info page (base + 0x4000, u32
    // index 0x20/4 — see the info-page layout in userspace.md). Bit 0 says this process was started
    // DETACHED, i.e. by `bg /fat/VUG.ELF` rather than `run`. A detached vug has no operator to press ESC
    // and, being unfocused, never receives input — so the 300-frame auto cap would end it in about a
    // second, which is exactly what read as "the app crashed" on the bench. Detached therefore means: run
    // the SAME deterministic auto path, but with no frame cap, until `kill` takes it down.
    //
    // Read AFTER SYS_WIN_CREATE, which is what maps the info page and publishes the word — reading it
    // before the window exists would fault on an unmapped VA.
    //
    // VUGMIN: the same word carries bit 1 = HIDDEN, so the pointer is kept rather than the read being
    // thrown away. The two bits are read on DIFFERENT schedules and that difference is the whole design:
    // bit 0 is fixed before the process runs, so one read here is complete; bit 1 changes under a running
    // process (the operator TABs to the shell and back), so it is re-read every frame in the loop below.
    // The page is mapped EL0-RO for this process's whole life and a `read_volatile` of a mapped word is
    // a single load — polling it per frame costs less than the branch that consumes it.
    let flags_p = unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4) };
    let detached = unsafe { flags_p.read_volatile() } & 1 != 0;
    let surf_va = base + 0x5000;
    SURF.store(surf_va, Ordering::Release);
    let surf = surf_va as *mut u8;

    // Spawn the two worker threads (one co-located, one on a sibling core). Stacks carved from the
    // window: A top = B+0x3000, B top = B+0x3800 (identical layout to UVUG-1).
    //
    // VUGGUARD: CHECK BOTH RETURNS. `SYS_THREAD_SPAWN` returns a negative errno when it cannot give
    // this process a thread — notably `-EAGAIN` when the kernel's fixed thread-handle table is full,
    // which is a GLOBAL, system-wide resource, not a per-process one. Until this arc the two returns
    // were captured only to be joined at exit, so a vug that got NO workers ran the whole frame loop
    // as though it had them: `DONE` could never reach 2, the frame barrier below blocked forever, and
    // the process parked in `futex_wait` BEFORE its first `SYS_WIN_PRESENT` — kernel-drawn chrome with
    // no content, unkillable from the shell. That is P60's empty window, and its root in this program
    // is exactly one thing: the app proceeded as though a resource it requested had been granted.
    //
    // The chosen behaviour is DEGRADE, not fail-fast. Every band a worker does not own, the parent
    // rasterises INLINE with the identical `render_band` on the identical published projection — so a
    // vug launched while the thread table is full still comes up, still draws, still responds to
    // input and still exits cleanly; it is merely single-threaded. It costs no restructuring of the
    // frame loop (the inline raster sits between the release and the barrier, exactly where the
    // parent otherwise idles) and therefore leaves the WC-H present discipline untouched. Because the
    // raster is the same function over the same coordinates, the final surface — and so the
    // deterministic auto-path CHECKSUM — is byte-identical to the two-worker run.
    let entry = uvug_worker as *const () as u64;
    let rc_a = unsafe { sys4(SYS_THREAD_SPAWN, entry, base + 0x3000, 0, 0) };
    let rc_b = unsafe { sys4(SYS_THREAD_SPAWN, entry, base + 0x3800, 1, 1) };
    let ok_a = rc_a >> 63 == 0;
    let ok_b = rc_b >> 63 == 0;
    let spawned = ok_a as u32 + ok_b as u32;

    // Bands the PARENT must rasterise itself this run (top = rows 0..64, bottom = rows 64..128).
    let mut inline_top = !ok_a;
    let mut inline_bot = !ok_b;
    // How many worker arrivals the frame barrier may legitimately wait for. Never more than the number
    // of threads that actually exist — a barrier target that cannot be reached is the wedge itself.
    let mut live: u32 = spawned;
    // Handles are joinable only if they are real. Joining a value that is a negative errno is a bogus
    // syscall; joining a thread that never started would be a lie about what was reclaimed.
    let mut join_a = ok_a;
    let mut join_b = ok_b;

    if spawned < 2 {
        // Name the denied resource on the serial line. This is the diagnostic whose absence made P60
        // look like a compositor fault: the app knew it had been refused and said nothing.
        let mut sb = Buf::new();
        sb.put(b":: UVUG: SYS_THREAD_SPAWN denied a=");
        sb.put_dec(if ok_a { 0 } else { (rc_a as i64).unsigned_abs() as u32 });
        sb.put(b" b=");
        sb.put_dec(if ok_b { 0 } else { (rc_b as i64).unsigned_abs() as u32 });
        sb.put(b" workers=");
        sb.put_dec(spawned);
        sb.put(b" -> inline raster ::\n");
        sb.flush();
    }

    let vbase = crystal_vertices();

    // Interactive/auto state.
    let mut ay: i32 = 0;
    let mut ax: i32 = 0;
    let mut dist: Fx = 4 * ONE;
    let mut held: u32 = 0;
    // CLICK-PLAIN: the drag word — 0 = no button down, else 1 + |dx|+|dy| travelled since the press.
    let mut drag: u32 = 0;

    let mut interactive = false; // flipped permanently by the first input event, at any frame
    let mut exit_key = false;
    // CLICK-ONE: rotation pause, toggled by SPACE. Purely cosmetic and interactive-only.
    let mut paused = false;
    // CLICK-PLAIN: clicks DELIVERED to this window since it started — the acknowledgement counter. It is
    // drawn beside the fps readout and printed on the wire, and it is the whole of what a click does in
    // this layer: unmissable proof that the router addressed the click to the window under the cursor,
    // with zero coupling to run state.
    let mut clicks: u32 = 0;
    // VUGLIFE: one-shot latch for the waived-budget witness (detached/interactive only).
    let mut budget_waived = false;

    // VUGFPS measurement state: the tick/frame pair at the last displayed-value refresh.
    let mut fps_ticks: u64 = getinfo_ticks();
    let mut fps_frame: u32 = 0;
    let mut fps: u32 = 0;

    // VUGPAUSE: the render state of the LAST PRESENT. The idle path engages only when a present has
    // actually happened (so the first frame is never skipped), when the overlay state of that present
    // matches what this frame would draw (so the frame that turns the overlay on is never skipped), and
    // when nothing that reaches the surface has changed since.
    let mut presented = false;
    let mut presented_overlay = false;
    let mut idle_witnessed = false;
    // VUGMIN: a SEPARATE one-shot latch from `idle_witnessed`. The two idle reasons are different facts
    // about the system — "the operator paused this vug" and "this vug is off-screen" — and a shared latch
    // would let whichever happened first silence the other forever. One line each, at most, per run.
    let mut min_witnessed = false;

    let mut frame: u32 = 0;
    loop {
        // --- input (polled EVERY frame for the program's whole life) ---
        let fi = drain_input(&mut held, &mut drag);
        if fi.saturated {
            // UVUG-9: the drain spent its whole frame budget with events still queued. Pre-fix this loop had
            // no budget to spend and simply never returned — the P54b freeze. Report once and carry on.
            stall_witness(&W_POLL, frame, b"poll drained=", MAX_DRAIN_PER_FRAME);
        }
        if !interactive && fi.any {
            // First input at any frame takes over: cancel the auto-tumble + the 300-frame cap and
            // switch to held-state control. The witness proves the input arrived on metal.
            interactive = true;
            // SIZE: the takeover line has exactly `say`'s shape (label, number, ` ::` terminator), so it
            // uses it — CLICK-PLAIN added the helper for its own two lines and this one was free.
            say(b":: UVUG: interactive takeover at frame ", frame);
        }
        // --- VUGMIN: is this vug currently off-screen? ---
        // Peter's ruling, P69: "if vug is minimized it should shut off". In UnaOS the state he is naming
        // is HIDDEN — `wm::focus_changed`'s shell arm pushes every window below `SHELL_Z` and erases its
        // box, so the vug is gone from the panel while its frame loop runs at full rate. The kernel
        // publishes that fact as bit 1 of the process-flags word; this is the poll.
        //
        // CLICK-PLAIN moved the read UP to here, ahead of the input block, from just above the `frozen`
        // fold. One volatile load per frame either way; what it buys is that the input block can ask
        // whether this vug is RUNNING (`paused || hidden`) at the moment a gesture arrives, which the
        // LAYER 2 hunk below needs and nothing else in between disturbs.
        //
        // Since VUGMIN-B the kernel really does set the bit (`wm::focus_changed`'s shell arm), so this is
        // no longer a dormant read. The deterministic auto path is still unaffected and the 300-frame
        // checksum still byte-identical, because nothing in a headless QEMU run ever TABs to the shell.
        let hidden = unsafe { flags_p.read_volatile() } & 2 != 0;
        if interactive {
            if fi.exit_key {
                exit_key = true;
            }
            // CLICK-ONE: SPACE toggles pause — the keyboard control, and under LAYER 1 the only one
            // that reaches run state at all. Parity, not a loop: two
            // press edges inside one frame are two toggles and cancel, so only an odd count changes
            // state — and only a change is worth a line. Human-rate by construction (one line per
            // press of a key a person is pressing), and the line doubles as proof that a keystroke
            // reached EL0.
            if fi.pause_keys & 1 != 0 {
                paused = !paused;
                say(b":: UVUG: pause=", paused as u32);
            }
            // CLICK-PLAIN: ACKNOWLEDGE the click and nothing more. The counter advances, the corner
            // readout shows it, and the wire carries one line per click — human-rate by construction, and
            // the proof that the press reached THIS window's ring rather than whichever window happened
            // to hold focus. Run state is untouched here on purpose: the mouse is a routing question in
            // this layer and the keyboard (SPACE, above) is the only control that stops or starts.
            if fi.clicks > 0 {
                clicks = clicks.wrapping_add(fi.clicks);
                say(b":: UVUG: click n=", clicks);
                // LAYER 2 (CLICK-RUN) DELETED — Peter's verdict on P76/P77 metal: click SELECTS
                // (focus + the ack above), SPACE stops/starts. A click changes no run state.
            }
        }

        // VUG-PACE, size only: "does this vug draw the VUGFPS overlay?" evaluated ONCE per frame. Both
        // inputs are settled by here — `detached` before the loop, `interactive` by the block just above —
        // and the answer is wanted at four sites below (the skip predicate, the idle refresh, the render
        // refresh, and the record of what was presented). Four inline copies is four times the branch.
        let overlay = detached || interactive;

        // The state that must HOLD STILL for a frame to be skippable. `paused` is the operator's explicit
        // request; `hidden` is the same conclusion reached from the other direction — pixels nobody can
        // see are not worth computing. They are folded into one word here rather than at the predicate
        // because BOTH halves need them, and the fold below is the half that is easy to miss: it is what
        // holds the orientation still across the whole frozen interval, so restore is a resume rather than
        // a jump — and, since VUG-PACE, it is also the INVARIANT the skip predicate rests on instead of an
        // equality test (a frozen vug's render state cannot change, because the empty arm is the one that
        // runs).
        let frozen = paused || hidden;

        // --- fold input into rotation/zoom ---
        let manual = interactive && (held & H_MOTION != 0 || (drag != 0 && (fi.mdx != 0 || fi.mdy != 0)));
        if frozen {
            // CLICK-ONE/CLICK-PLAIN: paused (by SPACE, or by a click under LAYER 2) — hold the
            // current orientation. VUGPAUSE: holding it is now
            // also what lets the frame below be skipped entirely; the window stays live because the
            // idle path keeps polling and yielding, not because it keeps redrawing. VUGMIN: a hidden
            // vug holds the identical way, so the orientation the operator left on screen is the
            // orientation they get back — restore is a resume, not a jump.
        } else if manual {
            let mut yaw = 0i32;
            let mut pit = 0i32;
            if held & H_YAW_L != 0 {
                yaw -= 4;
            }
            if held & H_YAW_R != 0 {
                yaw += 4;
            }
            if held & H_PIT_U != 0 {
                pit -= 4;
            }
            if held & H_PIT_D != 0 {
                pit += 4;
            }
            if held & H_ZOOM_IN != 0 {
                dist = (dist - ONE / 16).max(2 * ONE + ONE / 2);
            }
            if held & H_ZOOM_OUT != 0 {
                dist = (dist + ONE / 16).min(8 * ONE);
            }
            if drag != 0 {
                // Pointer motion → rotation, scaled + per-frame clamped (see DRAG_DIV/DRAG_CLAMP).
                yaw += (fi.mdx / DRAG_DIV).clamp(-DRAG_CLAMP, DRAG_CLAMP);
                pit += (fi.mdy / DRAG_DIV).clamp(-DRAG_CLAMP, DRAG_CLAMP);
            }
            ay = (ay + yaw) & 0xFF;
            ax = (ax + pit) & 0xFF;
        } else {
            // Idle tumble — the SAME deterministic advance from frame 0, so the auto path's 300-frame
            // checksum is a pure function of the frame count.
            ay = (ay + 3) & 0xFF;
            ax = (ax + 1) & 0xFF;
        }

        // --- VUGPAUSE: a paused vug whose surface cannot have changed renders NOTHING ---
        // The predicate is the honest one: paused, something already presented, the overlay in the
        // state the last present left it, and the full render state (yaw, pitch, zoom) identical. When
        // it holds, this frame would recompute the identical projection, rasterise the identical
        // pixels and present the identical surface — so it is skipped in full: no PHASE store (the
        // workers stay parked on their poll instead of rasterising), no inline raster, no barrier, no
        // present, and `frame` does not advance because no frame was presented.
        //
        // What DOES run is the input half plus one `SYS_YIELD` — the loop is bounded and runnable by
        // construction, never a park (VUGGUARD/P60), so the window stays live and `kill`-able and the
        // SPACE that unpauses is acted on within one iteration.
        //
        // VUGMIN widened the FIRST conjunct only, via `frozen`. `presented` still gates, so a vug hidden
        // before its first present renders that first frame normally rather than idling on an empty
        // surface; the overlay conjunct still gates, so the frame that turns the readout on is never
        // skipped. The render state is preserved across the whole hidden interval, which is what makes
        // restore a resume.
        //
        // VUG-PACE (P73 mouse-preempt triage, Fix C) — THE ORIENTATION CONJUNCTS ARE GONE, and the reason
        // is that they were an equality test standing in for an INVARIANT that holds by construction. While
        // `frozen`, the fold above executes its empty arm: `ay`, `ax` and `dist` are assigned nowhere else
        // in this loop, so a frozen vug's render state CANNOT change. Comparing it against the last
        // presented state could therefore only ever fail on the TRANSITION frame — and when it failed there
        // it failed CLOSED, refusing to park a vug that was frozen mid-motion and leaving it spinning the
        // full frame loop forever, which is the opposite of what the predicate exists to do. The wire datum
        // is `[vugpause2] blocked=` pinned at 8192 across ~500 s of saturation: under load the fleet stopped
        // parking altogether. Dropping the three conjuncts makes the first frozen frame the one that parks,
        // unconditionally. Its whole cost is that a vug frozen mid-motion holds the surface of the PREVIOUS
        // frame rather than of the frame it froze on — one tumble step, 3 brads, on a crystal that has just
        // stopped moving — and `last_ay`/`last_ax`/`last_dist` go with them, since nothing else read them.
        if frozen && presented && presented_overlay == overlay {
            // Name the REASON this idle engaged, not merely that it did — the two are different system
            // facts and the operator debugging a vug that stopped moving needs to know which. `hidden`
            // wins the tie because it is the stronger claim: a vug that is BOTH paused and hidden is
            // off-screen, and that is what an operator wants told.
            //
            // ONE emit site, a chosen tag and a chosen latch, rather than two blocks. Not style: this
            // program is linked into a 16 KiB `USER_REGION_SIZE` and `arroyo` refuses the image outright
            // if it does not fit. Duplicating the `Buf` + `put_dec` + `flush` sequence for the second
            // message built a VUG.ELF of 16664 bytes — 280 over the limit, a hard build failure; sharing
            // the body links at 12568. The two LATCHES stay separate, which is the part that matters:
            // neither idle reason may silence the other's one-shot line.
            let (tag, latch): (&[u8], &mut bool) = if hidden {
                (b"[vugmin] idle engaged frame=", &mut min_witnessed)
            } else {
                (b"[vugpause] idle engaged frame=", &mut idle_witnessed)
            };
            if !*latch {
                *latch = true;
                let mut ib = Buf::new();
                ib.put(tag);
                ib.put_dec(frame);
                ib.put(b"\n");
                ib.flush();
            }
            // ESC is honoured from the idle loop exactly as from a rendered frame.
            if exit_key {
                break;
            }
            // Keep the fps readout alive and honest (it falls to 0 — `frame` is frozen). A changed
            // digit is the only thing that can still reach the panel while idled: overlay only, one
            // present, at most once per second.
            //
            // VUGMIN: not while HIDDEN. This is the one place the two idle reasons must behave
            // differently, and the difference is the ruling itself. A paused vug is ON the panel, so its
            // readout is something the operator can see and is owed. A hidden vug is not on the panel at
            // all — every pixel of that refresh is discarded by the compositor — so drawing it would be
            // exactly the "still burning CPU while minimized" that this arc exists to end, merely at one
            // hertz instead of sixty. `fps` is left holding its last value; it is stale only while
            // nobody can read it, and the first rendered frame after restore resumes the refresh.
            //
            // CLICK-PLAIN: a CLICK is the second thing that can reach the panel from here, and it must
            // be. A vug stopped on this idle path is exactly the one an operator clicks to ask "are you
            // listening?", and under LAYER 1 the click changes no run state — so without this arm the
            // ack would sit in the surface, unpresented, until a digit happened to change. `fi.clicks`
            // is the frame's own count, so this fires once per click and nothing else keeps it awake.
            if !hidden && overlay {
                let v = fps_refresh(&mut fps_ticks, &mut fps_frame, fps, frame);
                if v != fps || fi.clicks > 0 {
                    fps = v;
                    unsafe { draw_hud(surf, fps, clicks) };
                    // FBCON-DMG: this is the one present in the program whose damage is genuinely a BAND.
                    // Nothing rendered this pass — the frame path did not run — and the only writer since
                    // the last present was `draw_hud` immediately above, whose extent is `[HUD_Y0, HUD_Y1)`
                    // by construction. So 11 of 128 source rows is the whole truth about what changed, and
                    // repainting the other 117 is work the compositor was being asked to do for nothing,
                    // once per second, for as long as a vug sits idled on an operator's desktop.
                    let rc = present_rows(win, HUD_Y0, HUD_Y1);
                    if rc >> 63 != 0 {
                        stall_witness(&W_PRESENT, frame, b"present rc=", (rc as i64).unsigned_abs() as u32);
                    }
                }
            }
            // VUGPAUSE-2: BLOCK, do not spin. VUGPAUSE stopped this frame from rendering and left behind a
            // runnable idle loop — drain + `SYS_YIELD` — which is still a task in a run queue on every
            // dispatch pass. On silicon at P69 that residue held the six-vug fleet at 47-61% per core with
            // nothing moving. `SYS_INPUT_WAIT` parks the task on its own input ring instead, so an idle vug
            // leaves the run queues entirely.
            //
            // Responsiveness is not traded away for it: the kernel issues the wake from the router's own
            // enqueue, so the SPACE that unpauses and the keystroke that exits re-ready this task in the
            // same pass the event arrives — sooner than the yield loop, which had to wait to be dispatched
            // before it could look. Focus arrival and un-hiding wake it too, since neither moves the ring.
            //
            // The syscall does not DEQUEUE, which is what lets it sit here with no restructuring: control
            // returns to the top of the loop and `drain_input` reads the event exactly as before.
            //
            // The P60 objection ("an unbounded futex_wait is an unkillable empty window") is answered
            // rather than ignored: KILLBOUND made the futex kill-aware from both sides, and a periodic
            // kernel backstop wakes parked waiters a few times a second so the `run` deadline and the GUI
            // wedge watchdog — both of which measure liveness in polls — keep seeing a live app.
            input_wait();
            continue;
        }

        // --- transform + publish, then release any LIVE workers ---
        // VUGGUARD: the release is conditional on there being someone to release. With `live == 0` the
        // phase word is nobody's signal, and storing to it would be the only remaining way for this
        // program to advertise a frame it is rendering entirely by itself.
        project(&vbase, ay, ax, dist);
        if live > 0 {
            DONE.store(0, Ordering::Relaxed);
            PHASE.store(frame + 1, Ordering::Release); // 1-based; never PHASE_EXIT (frame < cap)
            // VUGPAUSE-2: release any worker that outspun `WORKER_SPIN_YIELDS` and parked. UNCONDITIONAL,
            // and that is a decision rather than an oversight. Gating it on a "someone is parked" flag
            // would be a second lock-free protocol between three threads whose failure mode is a worker
            // that sleeps through its release — which the barrier would then diagnose as a dead worker and
            // RETIRE the pool over. A wake with nobody parked is a bucket scan the kernel completes in
            // microseconds; that is a much better thing to spend than a correctness argument.
            wake_phase();
        }

        // --- VUGGUARD: rasterise every band no worker owns, inline, while the workers run ---
        // Placed AFTER the release and BEFORE the barrier so the healthy path is untouched (both
        // predicates false, no work) and the degraded path keeps whatever parallelism it still has:
        // with one worker alive, the parent draws the other half concurrently with it. The bands are
        // disjoint by construction and `draw_line`/`put_px` clip to the band, so no two writers ever
        // touch a pixel.
        if inline_top {
            unsafe { render_band(surf, 0, SH / 2) };
        }
        if inline_bot {
            unsafe { render_band(surf, SH / 2, SH) };
        }

        // --- barrier: wait for the live workers to arrive (FUTEX) ---
        // UVUG-9: the wait itself is unchanged (re-check + compare-and-block is lost-wakeup-safe); a pass
        // counter is added, so a barrier that spins without its workers arriving names itself once.
        //
        // VUGGUARD makes the barrier honest in two ways. First, it waits for `live`, never a fixed 2:
        // with no workers it does not execute at all, and with one it waits for one. Second, the pass
        // budget is now a DEADLINE, not just a printed observation — burning it means a thread that
        // does exist is not arriving, so the parent RETIRES the worker pool: it signals PHASE_EXIT,
        // drops `live` to zero and takes both bands inline for the rest of the run. Every later frame
        // then renders and presents with no wait at all. UVUG-9 printed the same witness and went
        // straight back into `futex_wait`, i.e. it diagnosed the wedge and then re-entered it.
        //
        // VUG-PACE — SPIN, THEN PARK, ON THIS SIDE TOO. Until this arc the parent PARKED on the very first
        // pass: it stored the release, then immediately `futex_wait`ed on `DONE`. That single line was the
        // program's whole frame pace, and it paced by the wrong quantity. A park costs a WAKE plus a
        // DISPATCH, and dispatch latency is a property of the scheduler, not of how much CPU is spare: the
        // woken parent runs when its core next picks it, which on a raspi4b with no Group-1 timer IRQ means
        // when whatever is running there yields. Two workers arrive per frame, so a healthy frame paid TWO
        // of those round trips, and the frame time was floored by them at a value that does not fall when
        // the machine empties out. That is P73 exactly: "a delay to a vug speeding up when it's the only one
        // running", and a rate that "goes back to what it thinks its fps is supposed to be" — the plateau
        // was never a target fps (this program has never had one, no sleep, no budget, no target), it was
        // the round-trip floor quantising the readout to a stable-looking number.
        //
        // It also LATCHED. `WORKER_SPIN_YIELDS` passes are what keep a worker off the park path; frames slow
        // enough to outlast that spin put a park/wake in each worker's release too, which lengthens the
        // frame, which keeps them parked. Contention could enter that state and its own cost would hold it
        // there after the contention left.
        //
        // So the barrier waits the way the workers already wait, and for the reasons VUGPAUSE-2 gave for
        // them: `BARRIER_SPIN_YIELDS` passes of `SYS_YIELD` first, `futex_wait` only after. This is adaptive
        // with NO estimate and NO window, which is why the speed-up is immediate rather than earned back
        // over some interval:
        //   * CORES FREE — `SYS_YIELD` finds nothing else to run and returns at once, so the parent sees the
        //     arrival the instant the worker stores it. No wake, no dispatch, no floor. The frame runs at
        //     whatever the raster and the present cost, which is the definition of "as fast as the machine
        //     allows", and the very NEXT frame after contention drops is already faster — there is no state
        //     carried between frames for a stale one to live in (`spins` is re-armed here every frame).
        //   * CONTENDED — `SYS_YIELD` is a real handoff: the parent gives its core to whoever wants it on
        //     every pass, so spinning here cannot starve a sibling. It degrades to cooperative, not to a hog.
        //   * TRULY LONG WAIT — the spin is bounded and the park is still underneath it, so a wedged or
        //     retired worker costs a parked task, not a burned core. VUGPAUSE-2's idle contract is untouched:
        //     nothing here runs on the idle/hidden/paused path, which still parks in `SYS_INPUT_WAIT`.
        // `BARRIER_SPIN_YIELDS` is a LATENCY threshold, not a rate: short enough that a genuinely contended
        // wait reaches the park after a bounded handful of syscalls, long enough that no healthy frame ever
        // does. `passes` still counts only PARKED passes, so `BARRIER_PASS_BUDGET` keeps its old meaning.
        let mut passes: u32 = 0;
        while live > 0 {
            let d = DONE.load(Ordering::Acquire);
            if d >= live {
                break;
            }
            passes = passes.wrapping_add(1);
            if passes <= BARRIER_SPIN_YIELDS {
                sys_yield();
            } else if passes == BARRIER_PASS_BUDGET {
                stall_witness(&W_BARRIER, frame, b"barrier done=", d);
                // A worker still alive will see this and leave. VUGPAUSE-2: "will see this" now requires a
                // WAKE as well as a store — a worker that parked cannot poll its way to the sentinel — so
                // this is `phase_exit`, not a bare store. Without the wake, retirement would leave a live
                // thread asleep on a word nobody writes again: the precise shape of the P60 empty window.
                phase_exit();
                live = 0;
                inline_top = true;
                inline_bot = true;
                // Do NOT join a retired worker. `sys_thread_join` blocks until the thread finishes, so
                // joining the very thread that just failed to arrive would park this parent forever at
                // exit — the exact symptom this arc removes. The kernel handle it holds is leaked, and
                // that is the deliberate trade: a leaked row is the kernel's to reclaim, a parked
                // process is not recoverable from anywhere.
                join_a = false;
                join_b = false;
                break;
            } else {
                futex_wait(core::ptr::addr_of!(DONE), d);
            }
        }

        // --- VUGFPS: measure, refresh once per second, draw (desktop/interactive only) ---
        // CLICK-PLAIN: the click counter rides the same gate, the same band and the same self-erasing
        // box — one more readout, drawn every frame the fps readout is, so an ack is on the panel within
        // one frame of the click that earned it.
        if overlay {
            fps = fps_refresh(&mut fps_ticks, &mut fps_frame, fps, frame);
            unsafe { draw_hud(surf, fps, clicks) };
        }

        // --- present ---
        // UVUG-9: CHECK the return. UVUG-8 discarded it, so a `sys_fb_present` that began failing mid-run (a
        // lost per-process slot, a torn-down surface) would present as an unexplained freeze — frames still
        // advancing here, nothing changing on screen, and the kernel's no-render cap firing on a program that
        // believed it was drawing. An error has bit 63 set (negative errno), exactly like an empty input poll.
        //
        // FBCON-DMG: WHOLE BOX HERE, deliberately, and this is not an oversight. The rendering path just ran
        // the crystal across the FULL surface (the two workers own the top and bottom halves and between
        // them write every row), so the damaged band IS `[0, SH)` and there is nothing to narrow. Declaring
        // it through the banded verb would buy the compositor no work back and would cost every aarch64
        // frame a syscall that can only fail. The banded verb belongs on the idle HUD path above, where the
        // damage is 11 rows; here the honest answer is the one this line already gives.
        let rc = unsafe { sys1(SYS_WIN_PRESENT, win) };
        if rc >> 63 != 0 {
            stall_witness(&W_PRESENT, frame, b"present rc=", (rc as i64).unsigned_abs() as u32);
        }
        // VUGPAUSE: this is now the surface the panel is showing — record what produced it, so the next
        // frame can tell whether it would draw anything different.
        presented = true;
        presented_overlay = overlay;
        frame += 1;

        // --- exit conditions ---
        if interactive {
            // VUGCLICK: ESC ends an interactive run. A click does not.
            if exit_key {
                break;
            }
            // VUGLIFE: the frame budget binds only a FIXTURE-mode (foreground) run. A detached vug is a
            // desktop window with an operator in front of it: waive the budget once, say so on the wire,
            // and keep tumbling until ESC or `kill`. The witness is one-shot — `budget_waived` latches —
            // because the test is true on every frame after the cap, and a per-frame line would drown
            // the serial log it exists to be found in.
            if frame >= INTERACTIVE_CAP {
                if !detached {
                    break;
                }
                if !budget_waived {
                    budget_waived = true;
                    let mut wb = Buf::new();
                    wb.put(b"[vuglife] budget waived (interactive) frames=");
                    wb.put_dec(frame);
                    wb.put(b"\n");
                    wb.flush();
                }
            }
        } else if !detached && frame >= AUTO_FRAMES {
            // No input has ever arrived (QEMU): the deterministic auto path ends at 300 frames — the
            // surface at that frame is what the checksum witness asserts. VUG-BG: a DETACHED launch skips
            // this cap entirely and tumbles until it is killed. The two are disjoint by construction —
            // the checksum witness runs through `run_user_image` (foreground), which clears the detached
            // bit — so the 300-frame checksum is untouched by this branch.
            break;
        }
    }

    // Signal the workers to exit, then join the ones that exist and are still expected to answer.
    // VUGPAUSE-2: the wake is what makes the join terminate. A vug that exits FROM the idle path (ESC
    // while paused, or the interactive budget) has both workers parked on `PHASE`, so the sentinel store
    // alone would leave them asleep and `SYS_THREAD_JOIN` would block on threads that never finish —
    // turning the arc's idle win into a hung exit. `phase_exit` is store-then-wake, before the joins.
    phase_exit();
    if join_a {
        unsafe { sys1(SYS_THREAD_JOIN, rc_a) };
    }
    if join_b {
        unsafe { sys1(SYS_THREAD_JOIN, rc_b) };
    }

    // Witness.
    let mut buf = Buf::new();
    if interactive {
        buf.put(b":: UVUG: interactive exit=");
        // VUGCLICK: the reason must be true. Pre-arc the only two spellings were `key` and `click`, so a
        // run that ran out its INTERACTIVE_CAP reported `click` — a click that never happened. With click
        // no longer an exit, the honest pair is `key` (ESC) and `frames` (the safety cap).
        // VUGLIFE: the `frames` spelling now names its own cause. Post-arc it can only be reached by a
        // FOREGROUND (fixture/battery) run — a detached desktop vug waives the budget instead — so the
        // line says so, and the next sitting that meets it need not re-derive that this was designed.
        // The reason stays a SINGLE BARE TOKEN (`frames_budget`, not `frames (budget, …)`): the bench
        // parses this line with `exit=(\w+)`, and spaces or parens inside the field would break it. The
        // human-readable qualifier therefore rides AFTER `frames=<n>`, outside every parsed field.
        buf.put(if exit_key {
            b"key" as &[u8]
        } else {
            b"frames_budget"
        });
        buf.put(b" frames=");
        buf.put_dec(frame);
        if !exit_key {
            buf.put(b" (fixture mode)");
        }
        buf.put(b" ::\n");
    } else {
        let cksum = surface_checksum(surf);
        buf.put(b":: UVUG: frames=");
        buf.put_dec(frame);
        // VUGGUARD: report the workers this run actually GOT, not the two it asked for. On the healthy
        // path that is the literal `2` this line has always carried (the gate REQUIREs the exact
        // string); on a degraded run it is the honest count, and the checksum beside it is unchanged
        // because the parent rasterised the orphaned bands with the same code over the same geometry.
        buf.put(b" threads=");
        buf.put_dec(spawned);
        buf.put(b" checksum=0x");
        buf.put_hex64(cksum);
        buf.put(b" ::\n");
    }
    buf.flush();

    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
