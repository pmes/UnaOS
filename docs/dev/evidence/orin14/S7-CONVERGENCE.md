# S7 — `render_service` convergence design

Seat: orin 14, executor S7CONVERGE. Tree: `hw-jetson` at `6cc8de8c`. Every `file:line` below is in
`unaos/crates/kernel/src/` unless a path says otherwise, and was read at that sha. This is a design
deliverable: no kernel line changes with this document; the implementation arc is owed
(`docs/dev/LEDGER.md` S7).

The ruling this answers is the GATE-FAMILY three-part answer in
`docs/dev/evidence/orin13/LANDING-REPORT.md:24-37`: the family has three members; the shared part is
the pass loop; the axis that differs is HOW THE PASS WAITS and WHO OWNS INPUT; a parameterised call
of the Pi member "WOULD work once the waiting axis is lifted into a parameter/trait". The size-3
ledger entry carries an expiry, and this document is the design that spends it.

## 0. Shape of the proposal

One pass body, `render_pass<W: RenderWait>`, living in `main.rs` next to the members it replaces,
with the two divergent axes lifted into a trait: `RenderWait::wait` answers *what woke the pass*
(an input event, a furniture tick, or an order to retire) and `RenderWait::try_next` answers *is
there more in this burst*; a second, marker-style trait `InputOwner` (one associated const) says
whether the pass dispatches input at all. Three impls, one per wait discipline and named by the
discipline rather than the board: `ChannelWait` (Pi — blocking `GUI_CHANNEL.recv()` plus the sole
`shell_inbox` drain), `FoldingChannelWait` (x86 — blocking recv with the motion fold, the pending
slot, and the epoch/revenant check), `CounterPollWait` (Orin — `yield_now` busy-poll with a
CNTPCT-derived tick and no input half). Migration is Pi first with the body converted **in place**
so `main.rs` line numbers below the Pi member do not move (PARITY §5.3), Orin second (its 138-line
member collapses to an impl and a spawn shim, and `orin_render_service` is retired in favour of a
subsystem-named entry), x86 last (the widest member; its extra machinery becomes the impl's private
state). Each step has a QEMU gate and a GATE-FAMILY size assertion (3 → 3 → 2 → 1).

## 1. The three members side by side

| id | Pi `render_service` | x86 `x86_render_service` | Orin `orin_render_service` |
|---|---|---|---|
| cfg | `all(aarch64, baremetal)` `:5272` | `target_arch = "x86_64"` `:6322` | `all(aarch64, orinrender)` `:8221` |
| spawn | `spawn_prio("render", …, render_cpu, PRIO_SERVICE)` `:1441-1447` | `spawn("render", …, render_cpu)` `:1554-1560`; re-homed twin `"render-rehomed"` `:3298-3304` | `spawn_stack("orin-render", …, cpu 0, 32 KiB)` `:8198-8205`, one-shot latch `:8107,:8133` |
| body length | 421 lines `:5273-5693` | 570 lines `:6323-6892` | 138 lines `:8222-8359` |

Steps of the pass, in the order the Pi runs them. "identical" means the same calls on the same
objects; "differs" means the step exists on both sides with a different mechanism; "absent" means the
member has no such step.

| step | Pi | x86 | Orin | verdict |
|---|---|---|---|---|
| surface: `WRITER` snapshot + `Screen::new` | `:5278-5279` | `:6329-6330` | `:8227-8230` | identical |
| backdrop seed | none (shell paints the panel at boot `:5334`) | `screen.paint_desktop_scene()` when `desktop` `:6341-6343` | `screen.fill_screen(wm::DESKTOP_BG)` `:8245` | differs (three seeds) |
| backdrop predicate | `desktop_owns_backdrop()` `:5298`, cfg `desktop_firmware` (`:6208` returns `true`) | `desktop_owns_backdrop()` `:6340` → `desktop_uefi::is_active()` `:6186` | absent — reads `desktop_firmware::armed()` in-loop `:8277` | differs |
| `TargetPal` + `Console` | `:5302-5303` | `:6369-6370` | `TargetPal` only `:8246`; the console is `jd2_console_pump`'s `:2877-2878` | differs (Orin owns no console) |
| shell-window locals | lazy, cfg `desktop_firmware` `:5314-5328` | eager at task start, cfg `wc` `:6385-6420` | `shell_id = WIN_NONE` const, `shell_declined` `:8251-8254` | differs (lazy / eager / never) |
| first frame | `console.draw` + `ui_status::draw` + `render` `:5334-5341` | `:6422-6452` (shell window first frame `:6431-6448`) | absent (seed fill only) | differs |
| **wait** | `GUI_CHANNEL.recv()` `:5370` | `gui_recv_blocking_x86()` `:6541` behind a `pending` slot `:6496,:6539` | none — `yield_now()` at the tail `:8357` | **the axis** |
| burst drain | none (one event per pass) | inner `loop` `:6550-6738`, `gui_try_recv_x86()` `:6592,:6734`, motion fold `:6587-6608` | none | differs |
| revenant / epoch retire | absent | `RENDER_ROLE_EPOCH_X86` check `:6514-6526`, `wedgeinj_park_maybe` `:6535` | absent | x86 only |
| livecon service | `fbcon::console_live_service()` folded, cfg `livecon` `:5372` | absent from this member | absent | Pi only |
| **input dispatch** (Key/Mouse/Abs/Button/Timer) | `match ev` `:5395-5470`, `handle_key` `:5412,:5416` | `wc_route_event` `:6633`, `match ev` `:6635-6698`, `wc_route_tail` `:6725` | **absent** — `jd2_console_pump` `:2935-2983` | **the axis** |
| serial inbox drain | `shell_inbox::take()` loop `:5497-5514` (the SOLE drain) | absent | absent (`serialrx::drain()` is in `jd2_console_pump` `:2854,:2915`) | Pi only |
| cursor auto-hide | `:5519-5523` (`cursor::undraw`) | `:6801-6805` (`cursor::restore`) | absent (`jd2_console_pump` `:3003-3006`) | differs |
| strip recompose / tick | `ui_status::draw` if `strip_dirty`, else `ui_status::tick` on `strip_tick` `:5525-5533` | absent | `ui_status::tick` every pass `:8316` | differs; R17 removes it on the Orin (orin-ledger A18) |
| `pulsewin::service()` | folded onto `:5532`, cfg `desktop_firmware` | absent (`pulsewin::service` has one caller in the tree: `main.rs:5532`) | retired `:8309-8312`; R17 restores it | differs |
| `instgui::service()` | absent | `:6810`, cfg `wc,instgui` | absent | x86 only |
| dock `SHELL_REOPEN` latch | absent | `dock::take_shell_reopen()` `:6763` | absent | x86 only (LEDGER S4) |
| shell mint / decline | lazy mint on `armed()` `:5552-5617` | eager `:6398-6416` + reopen `:6763-6797` | decline only `:8275-8307` | differs |
| present | if `dirty` `:5620-5638` | unconditional per pass `:6814` | if `dirty` `:8321-8323` | differs (x86 presents every pass) |
| shell-window present | `:5654-5662` | `:6822-6842` | absent | identical where present |
| stack probe | absent | absent | `stk_probe("orin-render:passN")`, witness-gated `:8338-8341` | Orin only (LEDGER S13) |
| census | `[sched6]` + `prio_witness`, 5 s on `arch::ms` `:5666-5691` | `[schedx86]` + `emit_load_witness` + `rtwit::rollup`, 5 s on `arch::ms` `:6849-6890` | `[orinrender] census`, ~1 s on CNTPCT `:8348-8354` | differs (three clocks, three lines) |

Read down the verdict column: the steps that are *identical* are the surface, the shell-window
present, and the "present at most once when dirty" rule that two of three already follow. The steps
that *differ* differ in one of two ways — the wait primitive, or the furniture set a board services.
Nothing else in the table is a third axis: the backdrop seed, the first frame, the mint policy and
the census are all consequences of which board this is, and they are expressible as constants and
small hooks on the same impl that supplies the wait.

## 2. The two axes, precisely

### 2.1 Waiting

| member | primitive | what wakes it | cost |
|---|---|---|---|
| Pi | `GUI_CHANNEL.recv()` `:5370` → `Semaphore::wait` parks the task (`STATE_BLOCKED`, `switch_context` to the scheduler) `arch/aarch64/sched.rs:6753-6763` | a `post` from `gui_send` `:3448-3451`: the input task's USB pump `:4589-4599`, the strip pulse `Event::Timer` from `status_tick` `:5701-5720` (metal) or the `input_service` fallback loop `:5120-5124` (QEMU), and the serial wake token `serial_to_shell` `:3687-3692` | an idle render core is off the run queue (`:5369`). Before SCHED-6 the pass presented on every inbound event and pegged c0 at ~96–100% under an idle USB mouse (`:5350-5353`, P33); the dirty-paced pass is the fix, and the Orin member cites that measurement as the reason for its own rule `:8318-8320` |
| x86 | `gui_recv_blocking_x86()` `:6541` → `GUI_CHANNEL_X86.recv()` `:3343`, same semaphore shape `arch/x86_64/sched.rs:4878-4885`; non-blocking `try_recv` for the burst `:3332` | `x86_input_service` (`:6081`), which sleeps 1 ms per pass `:6173` and posts `Event::Timer` every `X86_GUI_PULSE_MS` `:6162-6171`; the re-home path respawns the consumer when the render core is declared dead `:3295-3304` | blocking recv means the idle render core `hlt`s (`:6318-6321`); the fold `:6587-6608` bounds work per burst |
| Orin | none. The pass runs to the tail and `yield_now()` `:8357`; `yield_now` requeues the task as READY and switches to the scheduler `arch/aarch64/sched.rs:4444-4460` | the dispatcher picks it again: `run_capstone_boot_core`'s `while dispatch_next(cpu)` `arch/aarch64/sched.rs:10200-10203`. There is no timer IRQ on the post-drop EL1 core, no `drain_due_sleepers`, and `SCHED_ACTIVE` is never set (`:8215-8217`, `sched.rs:3155-3156`), so a task that `sleep_ticks` here parks in `SLEEPERS[0]` forever | the whole of core 0, shared with `jd2_console_pump` which busy-polls for the same reason `:2866,:3017`. S12: the terminus folded busy but never idle → a structural 100%; `341ca707` folds idle in the dispatch loop `sched.rs:10201`; render3b read `SCHED: load c0=85%` (orin-ledger A5). The census had to be moved off a pass count onto CNTPCT because `passes % 20000` printed a line per ~4 ms and was 82% of the capture `:8259-8261` |

Why the Pi's primitive cannot be called on the Orin, in two independent ways: (1) `GUI_CHANNEL` is
declared `cfg(all(aarch64, baremetal))` `:3030-3031`; `baremetal = ["pi", "aarch64_el0"]`
(`Cargo.toml:225`) and `pi` + `tegra` is `compile_error!` (`arch/aarch64/serial.rs:23`) — it does not
exist in a tegra image. (2) Even if it did, `recv` parks until a `post`, and on the Orin nothing posts:
the keyboard goes to `pal::EVENT_QUEUE` and is taken by `jd2_console_pump` `:2935`, the strip pulse
task uses `sleep_ticks` `:5708` which never wakes on this core, and the serial wake token is the Pi's
`input_service`. The member would park on its first pass and the panel would freeze at the seed fill.
Both facts are what the Orin's own doc comment states `:8099-8102`; the design must preserve both.

### 2.2 Input ownership

| board | who drains `pal::EVENT_QUEUE` | who drains `shell_inbox` | who calls `handle_key` |
|---|---|---|---|
| Pi | the input task: `pump_usb_into_gui` `:4377` (called from `input_service` `:5129` and `usb_pump` `:5741`) forwards to `gui_send` `:4589-4599`; the render task receives | the render task, unconditionally, after the `match` `:5497-5514`; the producers are `serial_to_shell` `:3675` (from `input_service` `:5084,:5117`) | the render task, into whichever surface `windowed` names `:5412,:5416,:5505,:5509` |
| x86 | `x86_input_service` `:6081` produces into `GUI_CHANNEL_X86`; the render task receives and routes through `wc_route_event` `:6633` | no serial inbox on this path | the render task `:6661,:6667,:6673` |
| Orin | `jd2_console_pump` `:2935-2983` — also the xHCI poll `:2913-2915`, the serial drain `:2915`, the cursor `:2987-3006`, and the present of its own console `:3007-3009` | the same task (`serialrx::drain` `:2854,:2915`, cfg `orinrx`) | `jd2_console_pump` `:2894,:2949` |

Why the Orin forbids a second drainer, quoted from the member's contract: "IT DOES NOT DRAIN EVENTS.
`jd2_console_pump` owns `pal::EVENT_QUEUE` and feeds `handle_key`; a second drainer would steal
keystrokes from the shell. This task owns the PAINT half only." `:8219-8220`. The decline arm at
`:8291-8300` states the consequence of porting the Pi's painter without its drain: a window with one
frame of a prompt no keystroke can reach. On the Pi the arrangement is the opposite — the render task
is the only `handle_key` caller and its serial drain is placed AFTER the match on purpose, so the pass
that returns from a foreground command takes the backlog before parking `:5479-5486`.

So the axis is binary per board: **the pass either owns input (Pi, x86) or it does not (Orin)** — and
where it does, the body is identical in shape (route, `handle_key`, set `dirty`). The Orin's
"cooperative pump owns the keys" is a fact about `jd2_console_pump`, not about the render pass, and the
design leaves that pump untouched.

## 3. The proposal

### 3.1 Types

Illustrative signatures; names are proposals, subsystem-named per S6 (no `orin`/`pi`/`rmbp` in any
symbol, task name, or wire tag).

```rust
/// What a pass was woken for. `Input` carries the routed event; `Tick` is the furniture cadence
/// (the strip/pulse pulse on the Pi and x86, the CNTPCT-derived period on a polled terminus);
/// `Retire` is the x86 revenant order and is never produced elsewhere.
enum Wake { Input(unaos_kernel::pal::Event), Tick, Retire }

/// THE WAITING AXIS. One impl per wait discipline.
trait RenderWait {
    /// Block, sleep, or yield until there is something to do, then say what.
    fn wait(&mut self) -> Wake;
    /// Non-blocking: more of the same burst? (x86 folds motion here; Pi and the polled terminus
    /// return `None`.)
    fn try_next(&mut self) -> Option<Wake>;
    /// Called once per pass after the present; the census hook (three clocks today).
    fn census(&mut self, presented: bool);
}

/// THE INPUT-OWNERSHIP AXIS. `OWNS_INPUT == false` makes every input arm of the body a `match`
/// on nothing: the body still compiles the arms, but `wait` never returns `Wake::Input`.
trait InputOwner { const OWNS_INPUT: bool; }

/// The furniture a board services on `Wake::Tick`. Constants, not runtime flags, so a knob-off
/// image compiles no call it does not make.
trait Furniture {
    const STRIP: bool;      // ui_status::draw / tick
    const PULSEWIN: bool;   // pulsewin::service
    const DOCK_REOPEN: bool; // dock::take_shell_reopen (x86 today; S4 says both aarch64 boards owe it)
    const INSTGUI: bool;    // instgui::service
    const STACK_PROBE: bool; // stk_probe after the first two presents (S13)
}

fn render_pass<W: RenderWait + InputOwner + Furniture>(w: &mut W, cpu: usize) -> ! { /* the Pi body */ }
```

Three impls:

```rust
/// Pi. `wait` = `GUI_CHANNEL.recv()` (:5370) mapped: `Event::Timer` -> `Wake::Tick`, else
/// `Wake::Input`. `try_next` = `None`. `census` = the `[sched6]` block (:5666-5691).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
struct ChannelWait { s6: Sched6Counters }
impl InputOwner for ChannelWait { const OWNS_INPUT: bool = true; }
impl Furniture  for ChannelWait { const STRIP: bool = true; const PULSEWIN: bool = cfg!(feature = "desktop_firmware"); /* … */ }

/// x86. `wait` = epoch check (:6514-6526) -> `Wake::Retire`; `wedgeinj_park_maybe` (:6535);
/// `pending.take()` or `gui_recv_blocking_x86()` (:6539-6542); the motion fold (:6587-6608) lives
/// in `try_next`. `census` = the `[schedx86]` block (:6849-6890).
#[cfg(target_arch = "x86_64")]
struct FoldingChannelWait { pending: Option<Event>, my_epoch: u64, cpu: usize, /* … */ }

/// Orin. `wait` = `yield_now()` (:8357) then: CNTPCT past the tick period -> `Wake::Tick`, else
/// a spin-cheap `Wake::Tick`-less return — see 3.4. `try_next` = `None`. `census` = the
/// `[orinrender] census` block (:8348-8354) on the same counter.
#[cfg(all(target_arch = "aarch64", feature = "orinrender"))]
struct CounterPollWait { period: u64, last_tick: u64, last_census: u64, passes: u64, presents: u64 }
impl InputOwner for CounterPollWait { const OWNS_INPUT: bool = false; }
```

### 3.2 The body, and the arms that go quiet

`render_pass` is the Pi member `:5273-5693` with four substitutions and no reordering:

1. `:5370` `let ev = GUI_CHANNEL.recv();` becomes `let wake = w.wait();` and the `match ev` at
   `:5395` becomes `match wake { Wake::Input(ev) if W::OWNS_INPUT => match ev { … the existing arms … },
   Wake::Tick => strip_tick = true, Wake::Retire => retire(), _ => {} }`. The Pi's
   `Event::Timer` arm `:5466-5468` is what `Wake::Tick` already means.
2. The serial inbox drain `:5497-5514` is guarded by `if W::OWNS_INPUT` — and by its existing cfg:
   `shell_inbox` is a `baremetal` item, so on the Orin the block is `cfg`-erased exactly as today.
3. The furniture block `:5525-5533` becomes `if W::STRIP { … } ; if W::PULSEWIN && strip_tick { pulsewin::service() }`.
   Note the Pi calls `pulsewin::service()` only on the tick path `:5532`; the body keeps that.
4. The census block `:5666-5691` becomes `w.census(dirty)`.

The mint arm `:5552-5617` stays as it is: its guard is `desktop && shell_id == WIN_NONE &&
!shell_declined && armed()`, which is already false on x86 (eager mint, `shell_id` set at start) and
on the Orin is the decline the member spells out at `:8287-8306` — that decline becomes the
`open_shell_window` answer for a routed console (`fbcon::console_is_routed()` `:8287`) and is moved
INTO `open_shell_window` `:6241` as a `DECLINE reason=console-already-windowed`, which is where the
other declines are named (`:6302`).

What is NOT in the body: the x86 revenant loop `:6523-6525` (that is `Wake::Retire`'s handler, three
lines), the x86 dock-reopen arm `:6762-6797` (behind `W::DOCK_REOPEN`, kept verbatim as a helper
because S4 wants it on aarch64 too), the x86 `instgui::service()` `:6810` (behind `W::INSTGUI`), and
the Orin stack probe `:8338-8341` (behind `W::STACK_PROBE`, label `"render:pass1"` / `"render:pass2"`).

### 3.3 Placement — `main.rs`, and why not `video/`

The body calls binary-crate-private items that the `unaos_kernel` library cannot see: `handle_key`
`:2726`, `open_shell_window` `:6241`, `click1_dispatch` `:5247`, `serfocus_witness` `:4780`,
`SCREEN_APP_ACTIVE`, `GUI_RECV` `:5371`, `SERIAL_WAKE_PENDING` `:3636`, `desktop_owns_backdrop`
`:6185-6210`. Placing `render_pass` in `video/` would require lifting the shell's key path into the
library first, which is a different and larger arc. So the body, the three traits and the three impls
go in `main.rs`, in the region the Pi member occupies today, with the additions placed at the END of
the file (after `:8570`) so no existing line moves (PARITY §5.3; LEDGER P7 for the fold rule).

Lanes. `main.rs` is shared kernel core; per `CLAUDE.md` the rmbp seat owns shared kernel-core files
mid-arc and each track edits its own regions of it by grant. This arc therefore needs, before step 1:
a grant from rmbp for the Pi region `:5273-5693` and the file tail; an ack from pi, because step 1
edits the region the Pi's knob-off byte-identity proof stands on (§4). No `video/` file is touched by
steps 1–2; step 3 touches none either unless the x86 dock-reopen helper is moved into `video/dock.rs`
(optional, rmbp's call). `arroyo` is not touched: `arm-tegra-render` (`unaos/arroyo:3106`) already
compiles the Orin polarity, `arm-pi` the Pi's, and every x86 leg the x86 one.

### 3.4 The cfg shape

- The three entry points keep their cfgs verbatim: `render_service` `:5272`, `x86_render_service`
  `:6322`, and the Orin entry `:8221`. Each becomes a shim: construct the impl, call `render_pass`.
  Spawn sites `:1441-1447`, `:1554-1560`, `:3298-3304`, `:8199-8205` are unchanged.
- The traits and `render_pass` are gated `#[cfg(any(all(target_arch = "aarch64", feature = "baremetal"),
  target_arch = "x86_64", all(target_arch = "aarch64", feature = "orinrender")))]` — the union of the
  three member gates, so no image compiles a body it does not spawn.
- `GUI_CHANNEL` stays `cfg(all(aarch64, baremetal))` `:3030`; `ChannelWait` is gated on the same
  predicate, so a tegra image cannot name the Pi's wait even by accident, and `pi` + `tegra` stays the
  `compile_error!` it is at `arch/aarch64/serial.rs:23` — the design adds no cfg that widens `baremetal`.
- `CounterPollWait::wait` never blocks: it is `yield_now()` followed by a counter compare. Its
  `Wake::Tick` fires when `cntpct() - last_tick >= period`, with `period` derived from
  `ui_status::PSTRIP_PERIOD_MS` and `cntfrq()` the way `jd2_console_pump` derives `sweep_ticks`
  `:2840-2846`, so the pulse window is serviced at the strip cadence rather than every pass. Between
  ticks `wait` returns a `Wake::Idle` (fourth variant, or `Tick` with a `changed=false` — the
  implementation picks; the body does nothing on it and reaches `yield_now` again). This keeps the
  Orin's "at most one present per pass, only when dirty" `:8318-8323` and drops the per-pass
  `ui_status::tick` call `:8316`, which R17 retires anyway.
- Knob-off byte identity on the Pi `kernel8.img` (PARITY §5.3): see §4 step 1. The rule the design
  obeys is LINE-NEUTRAL placement in `main.rs` plus in-place conversion; the proof is the measurement,
  never the reasoning (`PARITY.md:254-256`).

## 4. Migration order and gates

The GATE-FAMILY size is counted as the number of distinct pass-loop BODIES in `main.rs`.

**Step 1 — Pi, no behaviour change (size 3 → 3, body now generic).** Convert `:5273-5693` in place;
add `Wake`, the traits, `ChannelWait`, and the shim at the file tail. Expected wire: identical
(`[sched6]`, `[shellwin-pi]`, `[serfocus]` lines unchanged). Gate:
`cd unaos && ./arroyo check` (both arches) · `./arroyo test-arm` · `UNAOS_PIDESK=1 ./arroyo kernel8-test`
and knob-off `./arroyo kernel8-test` (MBENCH 108/108 both, the Pi's standing suite, PARITY §5.2) ·
byte identity: `sha256sum target/pi_baremetal/kernel8.img` before and after (`PARITY.md:255`).
Two acceptable outcomes, ranked: (a) the hash is identical; (b) the hash moves for a cause stated in
the commit BEFORE the build (the candidate cause is the monomorphised symbol `render_pass::<ChannelWait>`
replacing `render_service` in the link, not a line shift), with the 108/108 twin captures green and the
pi seat re-basing its baseline as it did for CAPREVOKE (+29 lines, LANDING-REPORT `:43-44`). An
unexplained hash move is a STOP tripwire. Assertion: `grep -c 'fn render_service\|fn x86_render_service\|fn orin_render_service' main.rs` is still 3 and `grep -c 'fn render_pass' main.rs` is 1.

**Step 2 — Orin (size 3 → 2).** Replace `:8222-8359` with `CounterPollWait` + a shim; rename per §5.4;
fold R17 (strip off, `pulsewin::service` on `Wake::Tick`). Expected wire: `[render] census
passes=… presents=… win=0 declined=1 -> RENDER-LIVE` at ~1 s, `[u7stk] … task=N:render` twice, and
ZERO `[pstrip]`/strip lines on the cascaded scene. Gate: `./arroyo check` · `./arroyo test-arm` ·
`UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 ./arroyo esp-jetson` then reachability by strings, not banner:
`strings -a target/…/kernel.elf | grep -c 'RENDER-LIVE'` ≥ 1 and `grep -c 'orin-render'` = 0 ·
the Pi byte-identity re-measure (the tail additions must not move a Pi line: `orinrender` code is
cfg-erased on the Pi, but the edit is in the shared file). Assertion: body count 2.

**Step 3 — x86 (size 2 → 1).** Replace `:6323-6892` with `FoldingChannelWait` + shim; the fold, the
pending slot, the epoch check and the rescue banner `:6356-6367` are the impl's state. Gate:
`./arroyo check` · `UNAOS_WC=1 ./arroyo test` with `wc` in the `⚡ kernel features:` banner AND the
Kepler knobs so `desktop_uefi::activate()` `drivers/gpu/kepler_display.rs:486` is reached (CLAUDE.md:
a video gate without ignition is vacuous) · `strings` reachability of `[schedx86] depth` and
`[shellwin] backdrop=crispy-scene` in the built kernel · the re-home fixture (`RENDER_RESCUE_X86` path
`:3295-3304`) in whichever `wcser` capture the rmbp suite already runs. Assertion: body count 1;
the S7 row flips to `landed` and the GATE-FAMILY entry for this family is struck.

Order rationale: the Pi member is the most constrained (sole drain, lazy mint, measured pacing) and
the ruling says to design from it; the Orin is the thinnest and the first to prove `OWNS_INPUT =
false` compiles the arms away; x86 carries the most private machinery and goes last so its impl is
written against a body two boards already run.

## 5. Risks

**5.1 The blocking recv on tegra.** Guarded twice: `GUI_CHANNEL` and `ChannelWait` share the
`baremetal` cfg (`:3030`), and `CounterPollWait::wait` is `yield_now` + a counter compare with no
semaphore in it. The residual risk is a future impl that calls `sleep_ticks` on the terminus — the
same hazard `:8215-8217` names; the impl's doc comment must carry that sentence, and `arm-tegra-render`
compiles the impl so a `baremetal`-only symbol in it is a red check, not a metal hang.

**5.2 Pulse-window service placement (R17, orin-ledger A18).** The Pi services `pulsewin` on the tick
path only `:5532`; the retired Orin call sat on every pass `:8309`. The body places it on `Wake::Tick`
for all three, at `PSTRIP_PERIOD_MS` cadence; `pulsewin::service` is itself signature-paced
(`video/pulsewin.rs:603-606`) so an extra tick costs a compare. `pulsewin::arm()` is called by
`desktop_firmware::activate()` step 6 (`video/desktop_firmware.rs:373`) on the cascade path, which is
the arming R17 wants; the scaffold path's own `pulsewin::arm()` stays retired `:8174-8178`. The strip
(`W::STRIP = false` on the Orin) is removed from the cascaded scene by the same constant — but note
`ui_status::tick` is also the load SAMPLER the pulse window reads (`video/pulsewin.rs:573-575`,
`ui_status.rs:1285`); the Orin impl must keep the sample and drop only the draw, which means splitting
`ui_status::tick` into sample and draw halves or calling `ui_status::loads` from the window alone. That
split is in `ui_status.rs` (shared, rmbp lane) and is the one item in this design that needs a `video/`-side
grant; it is also A18's own work, not this arc's.

**5.3 The `[u7stk]` probe (LEDGER S13).** The Orin's two probes `:8338-8341` are the jetson image's
only stack gauge; the body keeps them behind `W::STACK_PROBE` after the first present, passes 1 and 2.
On the Pi the constant is `false` (the Pi's gauges live in `u7_launcher`); turning it on later is a
witness-gated, knob-off-neutral change. The label must be subsystem-named (`"render:pass1"`).

**5.4 The board-named symbol (LEDGER S6).** `orin_render_service` `:8222`, `tegra_render_arm`
`:8115`, `ORINRENDER_ARMED` `:8107`, the task name `"orin-render"` `:8200`, and the `[orinrender]`
wire family `:2863,:8124,:8134,:8145,:8162,:8207,:8289,:8303,:8351` are all board-named in a shared file.
Proposed names: entry `render_service_polled`, arm `render_arm_polled`, latch `RENDER_POLLED_ARMED`,
task name `"render"` (the name the Pi and x86 spawns already use `:1442,:1555`; the scheduler
distinguishes tasks by id, and the re-homed x86 twin keeps `"render-rehomed"` `:3299`), wire family
`[render]` with a `wait=poll` field on the census line so a capture still says which discipline ran.
The Cargo feature `orinrender` (`Cargo.toml:2356`) and the `UNAOS_ORINRENDER` knob are board-named
too; they are S6's rename, not this arc's, and the design does not depend on them.

**5.5 x86's present-every-pass.** The x86 member presents unconditionally `:6814`; the body presents
when `dirty`. On x86 the cursor arms `:6684-6689` draw through `pal` (`draw_over`) rather than the
front-buffer sprite the Pi uses `:5440`, so a motion pass IS dirty there; the impl sets `dirty` in its
Mouse arms and the behaviour is preserved. The `[schedx86] depth` census must keep printing
`fold=` so the fold is still scored.

**5.6 Core-0 load on the Orin.** Unchanged by this design: the polled impl still spins with
`jd2_console_pump`. S12's accounting fix (`341ca707`, `sched.rs:10201`) makes the number honest; a
real idle on that core needs a wake source the terminus does not have (no timer IRQ post-drop), which
is the ORIN-BSPTICK question and outside S7.

## 6. Estimate

| file | lines touched (approx.) | owner / grant |
|---|---|---|
| `main.rs` step 1 | ~15 changed in place (`:5273`, `:5370`, `:5395`, `:5497`, `:5525-5533`, `:5666`), ~90 added at the tail (`Wake`, three traits, `ChannelWait`, shim) | shared core: rmbp grant; pi ack for the byte-identity baseline; orin implements |
| `main.rs` step 2 | −138 (`:8222-8359`) +~45 (`CounterPollWait`, shim); renames at `:8107,:8115,:8200,:8124-8351` | orin (its own region), rmbp grant already in hand from step 1 |
| `main.rs` step 3 | −570 (`:6323-6892`) +~130 (`FoldingChannelWait`, dock-reopen helper, shim) | rmbp (x86 member is its lane); orin proposes, rmbp lands |
| `ui_status.rs` (A18 sample/draw split) | ~20 | rmbp (`video/`-side), owed by A18 not S7 |
| `video/*` | 0 in steps 1–3 | rmbp |
| `unaos/arroyo` | 0 (no new leg; `arm-tegra-render` `:3106` covers the Orin polarity) | rmbp |
| `Cargo.toml` | 0 (feature doc comment at `:2356-2365` updated to point here) | orin |
| docs | `docs/dev/OS/08_VIDEO/PARITY.md` (a §5.x on the trait and the byte-identity outcome), `docs/dev/OS/orin-ledger.md` (A18 cross-ref), `docs/dev/LEDGER.md` S7 → S4/S13/S6 cross-refs | each seat its own |

Net: `main.rs` shrinks by roughly 450 lines across the three steps; the family goes 3 → 1; the
waiting axis is a trait with three impls and the input axis is one associated const; nothing in
`video/` moves for the convergence itself.
