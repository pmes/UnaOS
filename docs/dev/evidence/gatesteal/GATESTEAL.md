# GATESTEAL (rmbp-ledger B657) — the loop closed: the pump's core dies on every W5 steal

Arc: the key task of wave 35 (FLIGHT35.md §4 "GATESTEAL", Peter's order BEFORE EVERYTHING). Branch
`exec-rmbp-gatesteal`, cut from `45e03964`. Design written before the code (R106), from the three files'
own records and the flight's wire; file:line are at `45e03964`.

## 1. The wire (f35-boot1.log 21:32:42, f35-boot3.log 22:04:01 — the same shape, byte for byte)

```
:: W5: site=post-steal from=c1 dead=c7 held=1671 …                              (boot 1; boot 3: held=1500)
:: W5: nmi core=c7 taken=y rip=0x798b5ac6 in_blit=n cs=0x8 memcpy=0x7a254c00..0x7a254c34 icr=ok ::
:: [wcser] GATE STOLEN from c7 by c1 after 1671ms — the holder was in phase 4 row 1238 and had not moved.
   … pick=render render=1 svc=7; desktop resumes on this core, c7 is DEAD and its singleton roles are owed a re-home == tripwire ::
:: PUMPWEDGE: age_ms=2003 last_step=bootlog holder=- -> FAIL :: pump=9@cpu7 passes=479907 wedges=1 bound_ms=2000
[lock] task=9 holds=[] waits=none (PUMPWEDGE: the usb-pump, stopped)
[pump] census why=wedge cores=2 c0=- c1=- c2=266 c3=- c4=45 c5=- c6=- c7=- pump=9@cpu7
[pump] cable rescue n=1 … outcome=drained      — then once a second to power-off (78 rescues on boot 1)
```

Boot 3 reads `held=1500`, `rip=0x79715ac6 … memcpy=0x7a0b4c00..`, `phase 46 row 23`, `last_step=prtscr`.
After the steal line NOTHING from c7 is on the wire again (`awk 'index($0,"cpu7")'` past the steal: the PUMPWEDGE
and census lines only); no `[wcser] REHOME` line ever prints on either boot; no `wedge cleared`.

## 2. What the records settle, with file:line

**2.1 The loop that nobody closed — the re-home's only consumer is the corpse.** The steal
(`video/wm.rs:4822-4921`) publishes the dead core into the `COMP_DEAD_CORE` mailbox (`wm.rs:4891`, a CAS
from the free sentinel) and says so on the line: "its singleton roles are owed a re-home". The mailbox's
ONE consumer is `comp_dead_core_take` (`wm.rs:10006`), called from `render_rehome_service`
(`main.rs:3255-3330`), which is called from `usb_pump` (`main.rs:5922`) — i.e. it runs ON THE PUMP, on the
SERVICE core, "the core the steal has never killed" (its own doc, `main.rs:3246-3251`). Flight 35 killed it
twice: `dead=c7`, `svc=7`. When the dead core is the pump's, the re-home is owed to a task that never runs
again. That is the whole of what the wire shows after the steal, and it is structural, not a race.

**2.2 "Had not moved" is a stale breadcrumb, not a parked core.** The `phase`/`row` gauges on the GATE
STOLEN line are `COMP_PASS_PHASE`/`COMP_PASS_ROW` (`wm.rs:10123-10149`), stamped by `comp_mark` at
`wm.rs:6222/6288/6343/6367` (phases 1-4 of the direct draw: 4 = "post-draw witnesses") and by
`PW_STAGED + PW_*` at `wm.rs:20362-20904` on the staged path (46 = `PW_STAGED(40) + PW_CAPTION(6)`,
`wm.rs:20689`), and `comp_mark_row` inside the flush loop. Nothing a holder runs AFTER its window loop stamps:
`strip::compose_all`, the cursor tail, the WC-D read-back that sits inside the phase-4 witness bracket
(`wm.rs:6367`: "`verify_window`'s read-back and `[wc-d]`" — `[wc-d] valve OPEN resumed suspended=25 after
5050ms closed` is on the steal's own second on boot 1), the re-run loop's `any_damaged`/`deferred_owed`/
`menu_paint_owed`, `blit_debt_heal_maybe`. A holder past its last stamp reads "had not moved" however much it
moves. The NMI probe is the instrument that settles it: `taken=y` — the core takes an NMI, so it is NOT
hardware-parked on a store (the W5SCOPE branch (d) the probe was built to find); `cs=0x8` — kernel text;
`in_blit=n` — not in the copy.

**2.3 The holder sat on ONE instruction, in both boots.** `rip − memcpy_lo` = `0x798b5ac6 − 0x7a254c00` on
boot 1 and `0x79715ac6 − 0x7a0b4c00` on boot 3 = the same `−0x99F13A` (the image relocated by `0x1A0000`
between the boots; both deltas agree). 1.5–1.7 s into the hold, in two boots whose steps differ (`bootlog`,
`prtscr`), the probe landed on the same instruction: a loop, not pass progress. LOCKREG sees nothing
(`holds=[] waits=none`: no `sync::Mutex` held or spun on), the pump's step word never advanced
(`lag::PUMP_CUR`, `lag.rs:1072/1086` — only `pump_seg` moves it, and it stayed at the step), and the pump's
last stamp was at the hold's start (`age_ms=2003` at steal + 0.3 s). So the pump was stuck BEFORE the
steal, inside a composite pass it held, at an instruction the kernel image cannot name (no symbol table in
the loaded image; the seat's `nm` of image 27h at `nm(memcpy) − 0x99F13A` names it). What the steal did to
c7 afterwards is exactly one NMI (`nmi_handler`, `interrupts.rs:827-845`: count, `nmi_catch_up` — returns at
once with `ESC_MASK == 0`, `stackguard.rs:781-784` —, the IST check, `w5nmi::record`; straight-line, no
frame edit, `iretq` back to the interrupted rip). The core resumed where it was. The steal is the SECOND
half of the fault; the first half is the pump's own pass never ending.

**2.4 Where the pump's steps take the gate.** Only `composite()` takes `COMP_GATE` (`wm.rs:4930-4983`;
the only other writer is the `wedgeinj` park, knob-off). The `bootlog` step is `bootpace::service_dump`
(`main.rs:6022-6026`; `bootpace.rs:322-343`), and its line 343 calls `crate::video::wm::paygo_service()` —
the PAYGO taker (`wm.rs:4149`, `paygo_service_pass` `wm.rs:4188`), whose own doc prices one take at "a 1.6 s
read-back" (`wm.rs:4162`): a full-coverage BAR1 read-back under the gate, from the device-service core, as an
UNMASKED holder — the holder that also runs the re-run loop and the lost-wakeup re-acquire
(`wm.rs:5026-5117`). The `prtscr` step (`prtscr.rs:526` `service` → `service_owed`) takes NO compositor
gate (no `composite`, `COMP_GATE` or `wm::` call in `prtscr.rs`; the panel is read through
`panel_snapshot`'s handle, `prtscr.rs:1193`); the c7 holder during boot 3's capture was another c7 lane's
present (the console/serial-door route `fbcon::route_present_banded`, `fbcon.rs:1142`, reached from a print —
`serial.rs:227` `fbcon::_print`, "self-guards (try_lock + interrupts off)" — or a staged `paint_window` at
the caption phase, which is what `phase 46` names). The GATE STOLEN line named the core; `[lag] stall …
holder=usb-pump:prtscr hold_ms=96 masked_ms=400` (22:04:00) says the pump held IRQs masked for 400 ms of
that second. A MASKED holder on the device core starves every task pinned to it — `xhci-poll`, `ehci-hid`,
`smc-batt`, the pump's own tick (`[hid] stall census … pump_cpu=7 -> CLEAR` after the steal is what a tick
that never fires reads) — which is the "glass dies" of §1: the input lanes ride c7.

**2.5 What the records do NOT settle.** The function at `nm(memcpy) − 0x99F13A`, and whether the holder spins
with IF=0 or IF=1. The W5 probe line now carries `if=<0|1>` (the interrupted RFLAGS.IF, recorded in the same
NMI handler slot as rip/cs), and the pump watch re-probes the pump's core at the wedge, so the next flight
answers both without a theory.

## 3. The mechanism (the code, file:line at this commit)

(a) **The pump's lane never holds the gate long.** (i) The PAYGO taker runs on a `paygo-take` task on a
sibling core, never on the pump: `bootpace.rs:343` now calls `sessionwedge::paygo_off_pump()` (spawns once;
falls back to the inline call when no sibling dispatches, so a one-core leg is unchanged). (ii) In
`composite()` the SERVICE core is a ONE-PASS holder exactly as a masked holder is: `wm.rs:5026`
`let masked = irqs_masked() || me == service_cpu()` — no re-run rounds, no lost-wakeup re-acquire; its
damage stays on the table for the render lane (`service_damage`, the standing backstop). (iii) A MASKED
`composite()` on the SERVICE core DECLINES (`wm.rs:4930`, `comp_svc_masked_decline`): it publishes
`COMP_PENDING` and returns, like any decline — the device core's pinned tasks are never starved by a masked
pass; counted in `comp_svc_masked_declined()`.

(b) **The dead core's roles ARE re-homed, by the scheduler, from a live core.** `wm.rs:4891` stamps
`COMP_DEAD_LAST`/`COMP_STEAL_GEN` beside the mailbox (gauges, never taken). The pump WATCH
(`sessionwedge::look`, `pump-watch` on a sibling core, armed at the ignition — the lane the pump's death
cannot kill) reads them on every look: when the dead core is the pump's core it calls
`sched::rehome_dead_core(dead, live)` (`sched.rs` tail): every task in `RUN_QUEUES[dead]` and
`SLEEPERS[dead]` — the pin contract (`steal_ok`) is void on a core the steal declared dead — is re-homed
the way `try_steal` re-homes (`task.cpu`, `migrations`, `migrate_ms`; `sched.rs:6215-6240`) and pushed on
`live` (`RunQueue::push` + `poke_for`, the `make_ready` shape `sched.rs:3617`; a sleeper's deadline is
re-based from the dead core's tick domain to the live core's). `live` is chosen by the placement authority
(`smp::xhci_worker_cpu(n)`, then `sched::sibling_online_cpu`), never the render core (the
`XHCI_CONTROLLER` deadlock rule, `sched.rs:6073`) and never a dead core; the SERVICE role follows the pump
(`smp::publish_sched_split(render, live)`), so `try_steal`'s exclusion follows it too. The dead core is
PARKED BY NAME: `sched::note_dead_core(dead)`; `pick_cpu` (`sched.rs:3535`) and `sibling_online_cpu`
(`sched.rs:3080`) skip it. The task CURRENT on the dead core cannot be moved by anyone (it is in no queue);
the line says so (`current=<tid>`), the watch retries while the pump is the current one (a sleeping pump is
caught in `SLEEPERS`), and `rehome=` on PUMPWEDGE reads `none` until the pump stamps from its new core.

(c) **`tests gatesteal`** (`sessionwedge.rs` tail; registered from `tests.rs:112` so a leg with no session
ignition reaches it): arms a FORCED LONG STEP — the pump's `bootlog` step takes the gate as `composite`
does and holds it, sleeping, for `GATESTEAL_HOLD_MS` (3000) — spawns a `gatesteal-drive` task on a sibling
core that calls `composite()` and `look()` every 100 ms (the steal is a branch inside `composite`, not a
timer: WEDGEINJ's lesson), and reads: `long_step=held` (the hold began), `steal=seen` (`comp_steal_gen`
rose), `rehome=ok(pump=<tid>@cpu<live>)` (the pump STAMPED from a core that is not the dead one),
`passes_after=<n>` (stamps in the second after), `input=alive` (passes rose and the `xhci-poll` task is
armed — input and the serial door ride the pump's cadence). PASS iff all four. Go-red: `GATESTEAL_REHOME`
= false disables the watch's re-home → `rehome=none` → FAIL. While the fixture runs, a PUMPWEDGE from the
held pump prints `-> SEEN :: fixture=1` (the live-wedge formatter, fixture-flagged), never a live FAIL.

(d) **PUMPWEDGE gains `rehome=<ok(pump=<tid>@cpu<n>)|none>`** (`sessionwedge::say`).

## 4. Expected wire, next flight

A pump step that composites on the device core now reads `[wcser] … declined=` for its masked presents and
one pass per unmasked hold; the paygo take is `paygo-take`'s. If the holder-at-one-instruction returns,
the W5 line reads `… rip=<r> if=<0|1> …` and, on the wedge, `:: GATESTEAL: probe core=c7 taken=<y|n>
rip=<r> if=<b> ::` beside `[pump] REHOME: c7 is dead and carried the usb-pump … -> c<live>: moved ready=<a>
sleeping=<b> current=<tid|->`, then PUMPWEDGE `rehome=ok(pump=9@cpu<live>)` once the pump stamps there, or
`rehome=none` with `current=9` naming the one task the scheduler cannot move. QEMU: `:: GATESTEAL:
long_step=held steal=seen rehome=ok(pump=<tid>@cpu<live>) passes_after=<n> input=alive -> PASS ::`.
