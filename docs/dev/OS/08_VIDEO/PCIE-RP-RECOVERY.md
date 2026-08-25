# PCIe root-port recovery for the compositor wedge — design

**Status: DESIGN ONLY. Nothing in this document is implemented.** The one thing that landed with
it is a boot-time sample of the two bridge registers the design depends on
(`drivers/gpu/pcihealth.rs`, the `[pcih] rp-boot` line) plus the bounds and CF8 hardening of that
module. Everything below is a plan and a set of constraints, written so the next arc can be
argued with before it is built.

**Scope:** the 2012 MacBook Pro Retina (`MacBookPro10,1`), GK107 endpoint at `1:0.0` below the
Ivy Bridge PEG root port at `0:1.0`. Some of the reasoning is machine-specific and says so.

---

## 1. What the evidence says, and what it does not

> **Update, 2026-08-22 — the wedge has since been localised, and this section's boot-11 reading was
> the first instance of it.** Boots 13–16 reproduced boot 11's `win=5 phase=33 row=704` signature
> four more times, and boot 15's ISR-driven row trace (99 samples, one a second, `row=897`
> throughout) proved the holder is **stopped inside one store into BAR1, not slow**. WCSER-STEAL
> now takes the gate from a holder in-pass past 4 s, so a wedge no longer freezes the desktop —
> though it does not recover the core. The read-back hypothesis is refuted. Full write-up:
> [`engine.md`](engine.md) §WCSER-ISR / WCSER-STEAL. Nothing in *this* design document has been
> implemented; the recovery it plans is still owed, and §1.1a's "no operator in the loop"
> constraint is unchanged.

Settled on metal before this arc and not re-derived here:

* The wedge is **endpoint-class**, not ASPM. Boot 11 ran `UNAOS_NOASPM=1` with the clear
  confirmed on the wire (`[pcih] aspm cleared rp 0043->0040 ep 0043->0040`) and wedged anyway, at
  118 s.
* **Link training is exonerated.** Boot 9 read `lnksta=d881` with the Link Training bit SET; boot
  11 read `lnksta=d081` with it CLEAR. Same wedge either way.
* The holder core is seized and there is no panic.

What the wedge looks like from the surviving side, read out of
`~/unaos-bench/capture/rmbp3-boot11/ttyUSB0.log`:

```
[ 117669ms]  (hold t0, derived from the first tripwire's age_ms=1000)
[ 118669ms] :: [wcser] PASS OVERDUE holder=c1 age_ms=1000  pending=true win=5 phase=33 row=704 == tripwire ::
[ 118669ms] [pcih] rp-at-wedge lnksta=d081 devsta=0000 secsta=2000 aer=n
[ 123668ms] :: [wcser] PASS OVERDUE holder=c1 age_ms=6000  pending=true win=5 phase=33 row=704 == tripwire ::
[ 123668ms] [pcih] rp-at-wedge lnksta=d081 devsta=0000 secsta=2000 aer=n
   ... no further tripwire, no further rp-at-wedge, ever ...
[ 126481ms] [wcser] scope=live entered=0 declined=1305 declined_pct=100 holder=1 held_ms=8814  -> WEDGED
[ 211480ms] [wcser] scope=live entered=0 declined=1175 declined_pct=100 holder=1 held_ms=93822 -> WEDGED
[ 212892ms] [vugfps] wf=5144
```

Four readings matter, and the one that has been given the most weight is the one least entitled
to it (§1.3).

### 1.1 The machine is not dead. Only the picture is.

At 212 s — 94 seconds into the hold — ring-3 vessels are still running and still presenting
(`[wpace] rollup ... pres=3711 ... -> FREE`), the window census still runs, serial still talks.
`comp=0` on every window: presents are accepted and then declined at the compositor gate, so
nothing reaches the glass. The operator's report that *"nothing ran right the whole time it was
booted"* is the panel frozen at 118 s on a machine that otherwise kept working.

That is the single most important fact for this design, because it changes what recovery is
**for**. The goal is not primarily to resurrect the GPU. It is to stop losing a working machine
to a frozen rectangle — and, secondarily, to learn whether the GPU can be resurrected at all.

### 1.1a There is no operator-in-the-loop. Both input channels are gone.

This constrains the design more than anything else in this section, and an earlier draft of this
document got it wrong.

* **The bench FTDI console is TX-only.** The machine can tell its story out the wire; the
  operator cannot type back. There is no serial command channel, at any time, wedged or not.
* **The USB keyboard path is down too.** The last decoded keystroke in boot 11 is
  `[ 117759ms] EHCI-HID: KEYUP` — 910 ms *before* the tripwire — and there is not one after, in
  94 seconds of uptime. (Partly this is the operator stopping when the screen froze; but the
  route is broken regardless, because `x86_input_service` is what forwards decoded events into
  the GUI channel and it is the task that blocked.)
* **The panel is frozen**, so nothing can be displayed to prompt for a decision.

**Consequence: any design step that says "ask the operator" or "wait for a keypress" is
unimplementable on this bench.** The recovery must reach a safe terminal state entirely on its
own, and every policy decision must be made at *boot* time — a compile-time feature or a
`UNAOS_*` knob baked into the media — never at wedge time. §7 is written to that constraint.

One speculative exception, recorded but not planned: a recovery task could read the HID decode
directly (`pal::next_event()`) instead of through the blocked `gui_send_x86` path, which might
make a dedicated hotkey usable as a trigger. Whether the queue is still being filled at that
point is unverified, so this is a possibility to test, not a mechanism to rely on.

### 1.2 One service loop survives the wedge; the one the probe lives on does not

`rp_at_wedge` fired twice and then stopped, while the 5-second `[wcser]` rollups continued for
another 88 seconds. Those two lines come from different places:

* The tripwire and the sampler run from `wcser_overdue_probe()` (`video/wm.rs:7940`), called as
  the **first statement** of `x86_input_service`'s loop (`main.rs:4499-4500`), on `svc_cpu`
  (`main.rs:1470`). It is first in the loop precisely because boot 8B proved the event pump can
  block into a wedged GUI (`main.rs:4494-4498`).
* The rollups ride `wcn_tick()`, which is called only from `present_banded`
  (`video/wm.rs:1316, 1331, 1336`) — i.e. **by whichever core called `wm::present*`**, including
  on the decline path. In boot 11 those callers were ring-3 apps going through
  `sys_win_present`.

So the input task stopped within ~6 s of the wedge — it blocked downstream of the probe, in
`gui_send_x86`, once the 64-slot GUI channel filled behind a render task parked on the gate —
while ring-3 present traffic kept the rollups alive. The probe survived exactly two crossings.

**But a kernel service loop did survive, and the capture proves it.** `x86_usb_pump`
(`main.rs:4321`) kept running for at least another 80 seconds:

```
[ 123880ms] :: PWR: window_ms=10109 state=plugged (charging) samples=10 ... == rollup ::
[ 133879ms] :: PWR: window_ms=10000 ...
[ 143878ms] :: PWR: window_ms=10000 ...
   ... one every 10 s, no jitter ...
[ 203871ms] :: PWR: window_ms=10000 state=plugged (charging) samples=10 ... == rollup ::
```

That line is emitted from `smc::battery::refresh_if_due()`, called at `main.rs:4370` inside the
pump's loop body. Nine consecutive rollups at an exact 10-second cadence is a healthy loop, not a
dying one. `[vuglod]`, `[vugpause2]` and the `SMC-BATT` witness ride the same window.

So the accurate statement is not "nothing survives" — it is that **two tasks on the same core
diverged**: `x86_input_service` and `x86_usb_pump` are both spawned on `svc_cpu`
(`main.rs:1463, 1470`), the input task blocked in `send`, and the pump kept being scheduled.

**Consequence, and it cuts two ways.** A recovery hung off the tripwire path would have had a
roughly five-second window in boot 11 and might have had none — so §4's dedicated task is still
required, and the earlier design paragraph is still wrong about which context to use (§9). But
the kernel is not starting from nothing: there is a demonstrated-live service body to hang
detection off, which makes that rung considerably cheaper than it first looked.

One caution against reading too much into the pump's survival: it is **not structurally immune**.
`x86_usb_pump` reaches `composite()` twice — via `wcx::desktop_app_service()` →
`wm::pace_service()` (`video/wcx.rs:634-644`, `wm.rs:1531`) and via `bootpace::service_dump()` →
`wm::paygo_service()` (`wm.rs:3458`). It survived boot 11 because the decline path returns
cleanly, but on a different interleaving it could win the CAS and become the wedged holder
itself. A recovery task must therefore be immune **by construction — never entering `wm` at all**
— rather than immune by observed luck.

There is a further sharp edge: `svc_cpu` is **not** a core the compositor never runs on, despite
the probe docstring saying so. `x86_usb_pump` shares `svc_cpu` (`main.rs:1463`) and reaches
`composite()` twice — `wcx::desktop_app_service()` → `wm::pace_service()` → `composite()`
(`video/wcx.rs:634-644`, `wm.rs:1531`), and `bootpace::service_dump()` → `wm::paygo_service()` →
`composite()` (`wm.rs:3458`). The probe's core can therefore be the wedged holder's core.

### 1.3 `secsta=0x2000` may be boot residue, and until this arc nothing could tell

Secondary status bit 13 is Received Master Abort. It has been read as the wedge's signature. But
secondary status is a **write-1-to-clear latch that this kernel never clears**, and the ordinary
way bit 13 gets set is bus enumeration: every config probe of an absent device below the bridge
master-aborts and latches it. This kernel walks buses `0..=255` in more than one place (the EHCI
driver's enumerator among them).

So `secsta=2000` at 118 s is equally consistent with *"the endpoint stopped answering"* and with
*"something probed an empty slot on bus 1 during boot, ninety seconds earlier"*. The sampler had
nothing to compare against.

**Landed this arc:** `census` now prints the boot value of both secondary status and Bridge
Control before anything else can set them:

```
[pcih] rp-boot bdf=0:1.0 secsta=XXXX bridgectl=XXXX (secsta is a since-boot W1C latch — compare rp-at-wedge against THIS, not against zero)
```

The next metal boot settles it. If that line already reads `secsta=2000`, the wedge-time reading
carries no information about the wedge and the classifier in §3 loses its only remaining input.
The line is a **read**; clearing the latch (W1C) is the instrument that would make every later
sample a true delta, and it is recommended for the next arc — it is a write to a shared bridge
register and did not belong in a bounds-hardening change.

Caveat the line cannot fix alone: `census` runs inside `pci::init`, so enumeration that happens
later can still set the latch afterwards. A zero there narrows the window; it does not close it.
Closing it needs a second sample taken after enumeration is complete.

---

## 2. The finding that reorders everything: the takeover programs no display state

`kepler_display::takeover_display` (`drivers/gpu/kepler_display.rs:35`) does **not** program the
display engine. It imports `mmio_write` at line 18 and never calls it; the EVO/PDISPLAY registers
it names (`0x640460`, `0x6101E0`, `0x61D1E0`, `0x640080`, lines 271-278) are read into
`pre_asm`/`pre_armed`/`pre_shadow` and never written. What the function actually does is:

1. locate the firmware GOP framebuffer (`video/fbcon.rs:470/480`),
2. re-derive BAR1 from config space and compute `gop_vram_offset = gop_fb_phys - vram_base`,
3. read-only recon of four heads,
4. **blit pixels through the BAR1 aperture** at `(bar1 + gop_vram_offset)`,
5. resume the panel console and call `video::wcx::activate()`.

The scanout is alive because **Apple's EFI GOP driver programmed it at boot** — PLLs, output
resource, panel link, timings, scanout base — and this kernel has inherited that state without
ever touching it. `video/wcx.rs:394-402` says as much: the takeover "keeps the scan-out there",
and `WRITER` was seeded from the same `BootInfo` triple, so the surface is adopted as
already-live.

A secondary bus reset returns the endpoint to power-on defaults. That includes the display
engine. **This kernel has no Kepler mode-set code and no VBIOS devinit execution path**, so after
an SBR there is nothing that can put a picture back on the internal panel.

Three corollaries, all load-bearing:

* **SBR on this machine is, today, a one-way trip to a dark panel.** Not a risk to be managed —
  the expected outcome.
* Re-running the takeover after a reset would blit into an aperture nothing scans out, and would
  be refused anyway: `wcx::ACTIVATED` is a consumed one-shot (`video/wcx.rs:254, 365`) that
  prints `activate REFUSE reason=already-active` on a second call, and adopting a fresh surface
  is a hard refusal (`wcx.rs:403-412`) partly because `FB_WC_DONE` is itself a consumed one-shot
  (`arch/x86_64/memory.rs:3528, 3620`), so a new aperture would come up uncached.
* There is **no separate framebuffer to fall back to**. `WRITER` (`video/mod.rs:153`) and `FBCON`
  (`video/fbcon.rs:262`) are two handles over the *same* physical GOP framebuffer, reached
  CPU-side through BAR1. Losing the GPU loses both.

This does not make the SBR rung worthless — see §6.4, where reclaiming the *seized core* is the
honest prize — but it does mean the rung that pays is not the one the earlier note assumed.

---

## 3. Detection: link-class vs software-class, and who decides

### 3.1 What is available today, and why it does not classify

| Signal | Boot 11 at wedge | Discriminating? |
|---|---|---|
| `lnksta` (root port) | `d081`, identical to the boot census | No — link up and trained |
| `devsta` (root port) | `0000` throughout | No — no error latched, no transactions pending |
| `secsta` bit 13 | `2000` | **Unknown** until the `rp-boot` baseline lands (§1.3) |
| AER UNC / COR | absent — the IVB PEG port reports `aer=n` | Not available on this machine |
| `COMP_PASS_WIN/PHASE/ROW` | `win=5 phase=33 row=704`, frozen | Says *where*, not *why* |

On this machine, with AER absent on the root port and `secsta` ambiguous, **there is currently no
positive link-class signal at all.** That is not a gap to be papered over; it is the central
reason nothing in this design may fire automatically (§7).

### 3.2 The discriminator worth building: a sacrificial endpoint probe

One non-posted config read of the endpoint's vendor/device ID separates the two classes cleanly:

* returns `0x10DE...` promptly → the endpoint's config space is answering; the wedge is in the
  BAR1/MMIO path or in software, and **no PCIe reset is warranted**;
* returns all-ones → the endpoint is not answering config; endpoint/link-class;
* never returns → the endpoint is not answering and completion timeout is not rescuing us; also
  endpoint-class, and the prober is gone.

The sampler deliberately never reads the endpoint (`pcihealth.rs`, `rp_at_wedge` doc) because
doing so could capture the last surviving witness. The way to get the answer anyway is to make
the reader **expendable and loud**:

* a dedicated probe task on a core with no GUI duty (neither `render_cpu` nor `svc_cpu`);
* it publishes `PROBE_ISSUED` to a static **before** the read and `PROBE_RESULT` after;
* one probe per wedge, ever, latched;
* the reader of those statics is the recovery decision, and *"issued but never completed"* is
  itself a verdict, not a missing datum.

Confidence: **moderate.** Config reads below a root port are normally terminated by the root
complex on completion timeout and return all-ones rather than hanging forever, so the prober
probably survives — but "probably" is the operative word, which is why it is designed to be
lost.

### 3.3 Who decides

Not the tripwire, and not automatically-on-today's-evidence. The decision belongs to a **recovery
task** (§4) that reads published facts and never touches the compositor.

There is no operator in the loop to defer to (§1.1a): the console is TX-only and the keyboard
route is down, so "print the evidence and wait for a human" is not available. The decision
therefore has to be **pre-committed at boot** — the operator chooses the policy when they build
the media, and the machine executes it without further consultation. Concretely:

* default: **classify and report only**, never act;
* `UNAOS_RPCONDEMN=1`: on an endpoint-class verdict, run the condemn-and-survive path (§5), which
  issues no PCIe write and cannot darken anything that is not already frozen;
* `UNAOS_RPRECOVER=1`: additionally permit the one SBR attempt (§6), with the dark-panel contract
  of §6.4 accepted in advance.

The escalation is strictly ordered and each level implies the one below it. Automatic firing of
the *SBR* level should not be enabled until the classifier has at least one signal that is
positively discriminating on this machine — which, today, it does not have (§3.1).

---

## 4. The context recovery must run in

§1.2 shows that survival is task-specific rather than core-specific — `x86_usb_pump` lived while
`x86_input_service` died on the same core — so the recovery task is part of the design, and its
defining property is immunity **by construction** rather than by observed luck:

* **Its own kernel task, on a core that is neither `render_cpu` nor `svc_cpu`**
  (`main.rs:1423, 1470, 1477`), and not the BSP — the BSP advances `arch::ms()`, which the whole
  witness apparatus depends on (`wm.rs:7936-7939`).
* **It never calls into `wm`, and never sends on a channel.** These are the two ways the two
  surviving/dying tasks were distinguished: the input task died in `gui_send_x86` on a full
  channel, and the pump survives only because every `composite()` it reaches happens to decline.
  A recovery task must be unable to take the compositor gate and unable to block on a queue —
  which means it can consult published statics and write to serial, and nothing else.
* **It takes no lock. Ever.** Not `WINDOWS`, not `FBCON`, not the allocator. Any of them may be
  held by the seized core. This is the discipline `fbcon::panic_screen` already follows
  (`video/fbcon.rs:2029-2034`: `try_lock` only, `mem::forget` rather than free), applied to
  recovery.
* **Every delay is TSC-based** (`arch::now_cycles()`), never `arch::ms()` and never
  `sched::sleep_ticks` — the recovery must not depend on the timer tick or the scheduler, either
  of which may be the thing that is broken.
* **It publishes each step to a static before performing it**, so the last published step names
  where it died. This is the same property that makes the sacrificial probe useful.
* **It calls `serial_ring::enter_panic_mode()`** (`serial_ring.rs:347`) before the first
  irreversible action. That switches serial to raw, lock-free, synchronous byte writes that
  cannot deadlock — without invoking the panic handler, which would paint
  (`main.rs:5085-5099` → `fbcon::panic_screen()`) and then `hlt_loop()` forever.

---

## 5. Quiesce: seal the gate, do not break it

The brief's hard question — *what must be quiesced, and by whom, given the seized core may never
return* — has a better answer than the obvious one.

**Do not try to release the compositor gate.** `wm.rs:7710-7734` records that a stale-holder
breaker was considered and declined, for two reasons that are still correct: `COMP_GATE` is a
plain `AtomicBool`, so the release sites store `false` unconditionally and a breaker admits a
**double release** (the original holder's later store frees the *second* core's gate); and even
with an owner token, breaking a live-but-slow holder puts two compositors on the same glass, and
`[comp2] max_us = 41048` is a real measured pass.

Instead, **seal it**: a monotonic `COMP_SEALED` latch checked in `composite()` immediately before
the CAS at `wm.rs:3828`. Once set:

* no core ever acquires the gate again;
* the wedged holder stays wedged — it was not coming back either way — and if it *does* return
  and store `false`, that is harmless because nothing will take the gate;
* callers fall straight into the existing decline path (`wm.rs:3832-3866`), which already returns
  without spinning, publishes `COMP_PENDING`, and defers the cursor sprite via
  `cursor::owe_repaint()`. That path is heavily exercised — boot 11 declined 1175 times in a
  single 5 s window.

Sealing is one-way, needs no owner token, and cannot itself wedge: one relaxed load on a path
that already has an early return.

Sealing the gate is necessary but not sufficient, because `composite()` is not the only writer
into BAR1. The full quiesce is one latch — call it **PANEL CONDEMNED** — consulted by:

1. `composite()` before the CAS (above);
2. `fbcon`'s panel re-attach paths — `attach_shadow()` (`fbcon.rs:1633`) and
   `panel_console_resume()` (`fbcon.rs:1707`) — which must become no-ops;
3. `fbcon::panic_screen()` (`fbcon.rs:2024`), which must not paint a condemned panel;
4. the cursor sprite path.

The kernel is *already* in serial-only console mode on the desktop path: `fbcon::detach()`
(`fbcon.rs:1573`) sets `GUI_ACTIVE` and the print path early-returns at `fbcon.rs:645-649`
without touching the framebuffer. `detach()` runs at `main.rs:1458`, immediately before the three
task spawns. So condemning the panel is close to *pinning a state the machine is already in* —
which is why this is the cheapest and safest part of the whole design.

**What cannot be quiesced, and does not need to be:** the seized holder, and any core currently
inside a BAR1 access. The design must be correct in their presence rather than try to stop them.
After a reset their stalled accesses resolve — posted writes drain to a range nothing claims,
non-posted reads return all-ones on completion timeout — and they resume into a condemned panel
whose writes are dropped by the bridge. That is benign **provided the condemn happens before the
reset**, which is why §6 orders it that way.

---

## 6. The SBR sequence, if it is attempted

### 6.1 Save first — the state does not exist yet

The endpoint's configuration is **not** recorded anywhere. BAR0/BAR1 base and size are locals in
`kepler::init` (`drivers/gpu/kepler.rs:1318-1372`), dropped when it returns, and `GpuInfo`
(`drivers/gpu/detect.rs:4`) carries BAR0 only — BAR1 is not in it at all. `kepler_display.rs:70-76`
re-derives BAR1 from config space precisely because nothing kept it.

So a save/restore must be **added at census time**, before any recovery is possible. Minimum set,
following Linux's `pci_save_state`/`pci_restore_state` shape:

* the 64-byte standard header — all six BARs, the ROM BAR, COMMAND, Cache Line Size, Latency
  Timer, Interrupt Line;
* the PCIe capability body — **DEVCTL especially**: `ep devctl=2930` at boot encodes Max Payload
  Size and Max Read Request Size, and an endpoint that comes back with a smaller MPS than the
  root port is a malformed-TLP generator;
* LNKCTL, so whatever ASPM policy was in force (`0043` normally, `0040` under `noaspm`) is
  restored rather than left at the reset default;
* the AER capability body on the endpoint (`ep ... aer=y`);
* on the **root port**, the boot value of Bridge Control — landed this arc as `[pcih] rp-boot
  bridgectl=` — because the reset pulse is a read-modify-write of that register and every other
  bit in it (VGA enable, ISA enable, error forwarding, the parity/SERR enables) must be carried
  through unchanged.

Restore order matters: BARs and COMMAND before anything that decodes memory; MPS/MRRS before any
traffic; ASPM last.

Confidence: **good.** This part is well-specified, bounded, and has a known-correct reference
shape. It is also independently useful — the saved state is what makes any future reset story
possible.

### 6.2 The pulse

Bridge Control is at offset `0x3E` and is 16-bit; `arch/x86_64/pci.rs` has `write_config_16`
(there is no `write_config_8`), so the register is directly writable. The sequence:

1. condemn the panel and seal the gate (§5); publish `CONDEMNED`;
2. `enter_panic_mode()` on serial;
3. read Bridge Control, set bit 6 (Secondary Bus Reset), write it back;
4. **hold ≥ 2 ms** — the spec's minimum assertion is 1 ms; Linux uses 2 ms and there is no reason
   to be tighter than Linux on a machine we cannot single-step;
5. clear bit 6, restoring every other bit to the value read in step 3;
6. **wait ≥ 100 ms** before issuing the first configuration request (the spec's post-reset
   requirement), then poll the endpoint's vendor ID with a **hard deadline of ~1 s** — the spec
   permits a device to answer with Configuration Retry Status for up to a second. On expiry:
   publish and stop. Never loop.
7. if and only if the vendor ID comes back correct, restore §6.1's saved state;
8. sample and publish the root port's LNKSTA / DEVSTA / secondary status again, and W1C the
   secondary-status latches so the post-reset state is readable.

All waits are TSC-based (§4). Every step publishes before it acts.

The root port itself is not reset by its own SBR — its command register, BARs and bus numbers
survive, so the endpoint's BDF stays `1:0.0` and the existing `map_mmio_window` mappings for
BAR0/BAR1 remain correct **provided the BARs are restored to their original values**, which
§6.1 ensures. Nothing needs remapping. (Re-mapping would in any case hit `FB_WC_DONE`, the
consumed WC one-shot at `arch/x86_64/memory.rs:3528`.)

### 6.3 What is re-established afterwards — and what is not

| Thing | After SBR + restore |
|---|---|
| Endpoint BARs, COMMAND, MPS/MRRS, ASPM | Restored from §6.1 |
| Endpoint on the link, answering config | **Hoped for, unproven** (§8) |
| BAR1 aperture responds to CPU access | Follows from BARs + Memory Space Enable |
| Root port secondary status / Bridge Control | Restored and cleared |
| **Display pipe, PLLs, output resource, panel link, scanout base** | **Gone. Nothing in this kernel can restore them.** (§2) |
| In-flight compositor state | Discarded — the gate is sealed and the panel condemned; damage is never replayed |
| `wcx` activation | Cannot be re-run (consumed one-shot, `wcx.rs:254/365`) |

### 6.4 Then why do it at all?

Because of §1.1 and the last row of §5: the machine survives the wedge, and the thing the wedge
costs beyond the picture is **a seized CPU core and whatever it holds**. An SBR resolves the
stalled transactions that hold that core — posted writes drain, non-posted reads time out — and
plausibly returns it. Reclaiming a core and unblocking the software behind it is a real recovery
even when the picture is unrecoverable.

It also produces evidence nothing else can: *does the GK107 come back on the link after a reset?*
A yes says the endpoint's PCIe layer is healthy and the wedge lives above it; a no says something
much deeper. Either answer is worth a controlled experiment on a machine with serial attached.

State this contract plainly wherever the knob is documented: **the SBR rung trades the panel for
the core, and the trade is not reversible without a power cycle.**

---

## 7. Failing safe

A recovery path that can itself wedge is worse than none. The rules, in priority order:

1. **Nothing fires unless it was armed at boot.** Not on today's evidence — §3.1 has no
   positively discriminating signal on this machine, and an SBR that mis-classified a software
   wedge would darken a machine that was only showing a frozen rectangle. The three-level
   boot-time escalation in §3.3 is the whole policy surface; the default level acts on nothing.

   **There is no operator trigger to fall back on** (§1.1a): the FTDI console is TX-only and the
   keyboard route dies with the input task, so the machine cannot be asked and cannot be told.
   This removes the safety valve most such designs lean on, and it is the reason every remaining
   rule below is about *self-limitation* rather than *supervision*. It is also the reason rule 2
   matters so much: the rung that needs no permission is the rung that should carry the weight.
2. **The condemn-and-survive path issues no PCIe write at all.** §5 is pure software: one
   monotonic latch on paths that already early-return. It is separately armable from the SBR, and
   it is the rung that should land first — it is what turns boot 11 from *"nothing ran right"*
   into *"the compositor died at 118 s, here is why, the machine is still yours."*
3. **One attempt, ever.** A `RECOVERY_ATTEMPTED` latch checked before anything. A retry loop is
   how one wedge becomes an SBR storm.
4. **Refuse to start on any missing precondition** and say which: no verified ECAM page, no
   cached endpoint state, no `rp_bdf()`, recovery already attempted, or the classifier reporting
   software-class. A refusal that names its reason is a good outcome.
5. **Every wait has a hard cycle deadline; on expiry, publish and stop.** No unbounded poll
   anywhere on the path.
6. **No locks, no allocation, no scheduler dependency** (§4).
7. **The terminal state is defined, reachable, and honest about what it leaves behind:**
   condemned panel, machine still running, **serial still narrating outward but un-drivable**.
   Not a panic, not `hlt_loop()`. The kernel's panic path never reboots (`main.rs:5085-5099`) and
   there is **no reboot facility of any kind** in this tree — no `reboot()`, no 0xCF9 PCH reset,
   no ACPI `RESET_REG` use (the FADT is parsed only for the PM timer and the S5 block), no
   deliberate triple-fault helper. The one clean exit is `acpi_power::poweroff()`
   (`arch/x86_64/acpi_power.rs:345`).

   Say the consequence plainly rather than dressing it up: after a condemn, the operator's only
   remaining action is the power button. That is **not a regression** — it is exactly where boot
   11 already left them, ninety-four seconds in, with no explanation. What the condemn adds is
   the explanation, a reclaimed core, and a machine that stopped pretending. What it must never
   do is take away the one thing boot 11 *did* preserve: a live serial narration. Hence rule 8.

   Because `poweroff()` cannot be requested by a human here, the recovery path must not call it
   either — an automatic power-off would end the narration and destroy the evidence the sitting
   exists to collect. It stays available for a future policy level, deliberately unused now.
8. **The recovery must be correct if it is killed at any point.** Because it publishes before
   acting and holds no lock, being lost mid-sequence leaves a condemned panel and a log that says
   where it stopped — which is exactly the terminal state of rule 7.

### How the operator learns

* **Serial is the only channel, and it is one-way** (§1.1a). One loud block at condemn time
  carrying the classification, the `win/phase/row` breadcrumb, and the boot-vs-wedge register
  deltas now that `rp-boot` gives a baseline. Because it is the only channel and it cannot be
  interrogated afterwards, the block must be **complete at the moment it is printed** — every
  fact the next sitting will want, emitted once, with no "run X to see more". Assume it is the
  last thing the machine ever says.
* **The panel is not a channel** — by construction.
* **The flight recorder** (`flight_recorder::service()`) writes the captured boot log to
  `UNAOS.LOG` on the FAT volume, which is how the operator gets the story without serial. Note
  the hazard: it is serviced from the `x86_usb_pump` loop on `svc_cpu`, the same loop §1.2 shows
  can block. A condemn that wants to be durable should force a flush from the recovery task
  itself, or accept that the on-disk log may end before the condemn.

---

## 8. What I am not confident about

Named deliberately, because a reset that half-works on a display device is how a machine goes
dark permanently.

1. **Whether the GK107 comes back on the link at all.** Apple's EFI may leave the device in a
   state that needs VBIOS devinit even to re-enumerate. Unknown, and it is the load-bearing
   unknown of the whole SBR rung.
2. **Whether the seized core is actually freed.** §6.4 is the main argument for doing this and it
   is a plausibility argument, not a proof. It depends on how the stall is held — a full store
   buffer behind a posted write is not the same as a core parked on a non-posted read.
3. **The panel mux.** The `MacBookPro10,1` routes the internal panel through a gmux between the
   Ivy Bridge IGD and the GK107. This kernel has an `intel-ivb` / `igpu::init` path but no gmux
   support, and which side EFI left the panel on has never been established. If it is on the
   dGPU, SBR guarantees darkness. If it were on (or could be moved to) the IGD, **the dGPU
   becomes expendable and this entire design gets much better** — that possibility is worth
   investigating before building the SBR rung, and may be a better long-term answer than reset.
4. **Whether SBR on a CPU-integrated PEG root port behaves like a discrete bridge's.** It should.
   PEG ports have chipset quirks and this one is Apple-configured.
5. **Whether Apple firmware/SMM reacts to a link-down event** (SMI storms, thermal or fan
   handoff). Entirely unexamined.
6. **The 100 ms / 1 s timings are the spec's**, not this machine's. Apple firmware may want more.
7. **Whether `secsta=0x2000` means anything at all** — the `rp-boot` line settles it next boot,
   and if it turns out to be residue then §3.1's table has *no* remaining entries and §3.2's
   sacrificial probe becomes the only path to a classifier.
8. **Whether the endpoint returns in D0 and initialised**, or in a power state that needs handling
   before config restore.

---

## 9. Where this disagrees with the earlier design paragraph

The prior sketch is in `~/unaos-bench/scratch/rmbp2-close/pcihealth/NOTES.md`, under "Follow-up
rung if confirmed". Its skeleton — set Bridge Control bit 6, hold ≥ 1 ms, clear, wait for link
training, re-walk the endpoint from config zero, re-write COMMAND and the BARs — is right, and
§6.2 keeps it. It is also right that the two cores captured in BAR1 accesses are the hard part
and that the compositor must be quiesced before anyone touches BAR1 again. Three of its claims do
not survive contact with the code.

1. **"re-run the kepler takeover to repoint scan-out, since the reset destroys all device state
   including the display controller's."** The takeover does not point the scanout. It writes no
   display register at all (§2) — the scanout belongs to Apple's EFI. Re-running it would blit
   into a dead aperture, and `wcx::activate()` would refuse regardless (consumed one-shot,
   `wcx.rs:254/365`). This is the most consequential correction: the note names a restore step
   that does not exist, and with it goes the assumption that SBR can give the picture back.

2. **"restore the BARs the firmware assigned (already known from kepler init)."** They are not
   known. BAR0/BAR1 are locals in `kepler::init` (`kepler.rs:1318-1372`), and `GpuInfo`
   (`detect.rs:4`) carries BAR0 only. Nothing persists them past that function. A save has to be
   built before a restore can be written (§6.1).

3. **"it is a root-port register write, so it is issuable from the surviving input-service core
   even with the endpoint hung."** The input **service** does not survive, and the register write
   being cheap does not help if nothing is left to issue it. Boot 11 is the proof: `rp-at-wedge`
   printed at 118669 ms and 123668 ms and never again, while `[wcser] WEDGED` kept printing past
   211480 ms from the ring-3 present path. The input task blocked downstream of the probe, exactly
   as `main.rs:4494-4498` anticipated for the pump.

   The precise correction is worth stating, because the sloppy version of it is also wrong: the
   *core* survived — `x86_usb_pump` shares `svc_cpu` and kept its 10-second cadence to 203871 ms
   (§1.2). What died was one task on it. So the fix is not "pick a different core", it is "build a
   context that cannot block and cannot take the compositor gate" (§4).

One further difference of emphasis. The note treats recovery as *"the self-heal that turns a dead
machine into a logged hiccup."* On this machine the wedge does not produce a dead machine — it
produces a frozen panel on a live one (§1.1). The self-heal worth building first is therefore
**condemn-and-survive**, which needs no PCIe write, cannot itself wedge, and is what the operator
actually lost during boot 11.

---

## 10. Proposed order of work

| Rung | What | Risk | Depends on |
|---|---|---|---|
| 0 | `rp-boot` baseline for `secsta`/`bridgectl` | none (read) | **landed with this document** |
| 1 | W1C the secondary-status latch at arm time, so every later sample is a delta | low | rung 0's reading |
| 2 | **Condemn-and-survive**: `COMP_SEALED` + PANEL CONDEMNED + the loud serial block | low, no PCIe write | §5 |
| 3 | A recovery task immune by construction (§4) — never enters `wm`, never sends on a channel | low | rung 2 |
| 4 | Sacrificial endpoint probe → a real classifier (§3.2) | medium — may lose a core, by design | rung 3 |
| 5 | Endpoint config save at census time (§6.1) | low (read + statics) | — |
| 6 | **SBR at the `UNAOS_RPRECOVER=1` policy level, dark-panel contract pre-accepted** | **high, irreversible** | rungs 2-5 |
| — | gmux / IGD failover investigation (§8.3) | unknown | may obsolete rung 6 |

Rungs 0-5 are all worth doing on their own merits and none of them can darken the machine. Rung 6
should not be attempted until rung 4 has produced a classifier and the trade in §6.4 has been
accepted in advance — which, given §1.1a, is a decision made when the media is built, because it
cannot be made while the machine is wedged.

**If only one rung is ever built, build rung 2.** It needs no PCIe write, no classifier, and no
permission; it cannot darken anything that is not already frozen; and it converts boot 11's
outcome from *"nothing ran right the whole time it was booted"* into a machine that says exactly
what died, keeps narrating, and hands back the core it was holding.
