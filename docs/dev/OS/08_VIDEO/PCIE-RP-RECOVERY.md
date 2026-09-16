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
`x86_usb_pump` reaches `composite()` twice — via `desktop_uefi::desktop_app_service()` →
`wm::pace_service()` (`video/desktop_uefi.rs:634-644`, `wm.rs:1531`) and via `bootpace::service_dump()` →
`wm::paygo_service()` (`wm.rs:3458`). It survived boot 11 because the decline path returns
cleanly, but on a different interleaving it could win the CAS and become the wedged holder
itself. A recovery task must therefore be immune **by construction — never entering `wm` at all**
— rather than immune by observed luck.

There is a further sharp edge: `svc_cpu` is **not** a core the compositor never runs on, despite
the probe docstring saying so. `x86_usb_pump` shares `svc_cpu` (`main.rs:1463`) and reaches
`composite()` twice — `desktop_uefi::desktop_app_service()` → `wm::pace_service()` → `composite()`
(`video/desktop_uefi.rs:634-644`, `wm.rs:1531`), and `bootpace::service_dump()` → `wm::paygo_service()` →
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

**Landed since (SECSTA2, §11.4):** that second sample exists, as a second W1C clear at the tail of
`pci::init` — and building it corrected the sentence above about *which* enumeration was left.
This section, and §11.1's W7 row, named "the EHCI driver's own `0..=255` walk". `ehci::init` is
called from `arch/x86_64/pci.rs:838` and the Kepler dispatch that reaches `census` from
`arch/x86_64/pci.rs:1016`: **EHCI walks the bus BEFORE the at-arm clear, and is already wiped by
it.** The walks that actually follow it are `sdhc::probe` → `storage_inventory`
(`drivers/pci.rs:84`), `ahci::probe` (`drivers/ahci.rs:286`, knob-gated) and `init_network` →
`find_device` (`drivers/pci.rs:141`) — all in the tail of the same function, which is where the
second clear goes.

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
5. resume the panel console and call `video::desktop_uefi::activate()`.

The scanout is alive because **Apple's EFI GOP driver programmed it at boot** — PLLs, output
resource, panel link, timings, scanout base — and this kernel has inherited that state without
ever touching it. `video/desktop_uefi.rs:394-402` says as much: the takeover "keeps the scan-out there",
and `WRITER` was seeded from the same `BootInfo` triple, so the surface is adopted as
already-live.

A secondary bus reset returns the endpoint to power-on defaults. That includes the display
engine. **This kernel has no Kepler mode-set code and no VBIOS devinit execution path**, so after
an SBR there is nothing that can put a picture back on the internal panel.

Three corollaries, all load-bearing:

* **SBR on this machine is, today, a one-way trip to a dark panel.** Not a risk to be managed —
  the expected outcome.
* Re-running the takeover after a reset would blit into an aperture nothing scans out, and would
  be refused anyway: `desktop_uefi::ACTIVATED` is a consumed one-shot (`video/desktop_uefi.rs:254, 365`) that
  prints `activate REFUSE reason=already-active` on a second call, and adopting a fresh surface
  is a hard refusal (`desktop_uefi.rs:403-412`) partly because `FB_WC_DONE` is itself a consumed one-shot
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
| `wcx` activation | Cannot be re-run (consumed one-shot, `desktop_uefi.rs:254/365`) |

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
   Not a panic, not `hlt_loop()`. The kernel's panic path never reboots (`main.rs:5085-5099`).
   Since FADTRESET (`b7901763`) the tree DOES have a reboot facility — the shell's `reboot` verb →
   `power::reboot` → `acpi_power::reboot()`: the FADT `RESET_REG` write, then the 8042 pulse, then an
   honest `hlt` park — but it is an OPERATOR verb, not something the condemn path invokes, and this
   design does not change that. Two metal facts about it (flight 6, 2026-09-03): the verb did reset
   the rMBP, and none of its `[pwrreboot]` witnesses reached the FTDI console, because the ladder
   writes them to the 16550 that this laptop does not have and the reset lands before the xHCI
   service pass could drain the mirror ring. BOOTFADT (`857c6dc8`) therefore prints the FADT reset
   facts once at boot instead. The other clean exit is `acpi_power::poweroff()`
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
   into a dead aperture, and `desktop_uefi::activate()` would refuse regardless (consumed one-shot,
   `desktop_uefi.rs:254/365`). This is the most consequential correction: the note names a restore step
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
| 1 | W1C the secondary-status latch at arm time, so every later sample is a delta | low | **landed, WIDENED, and knob-gated: BAR1WEDGE (§11). It clears THREE latches, not one — secondary status was the only one anybody had noticed** |
| 1b | **SECSTA2** — a SECOND W1C clear once enumeration is complete, so the first-stall deltas are measured against the end of the bus walks and not against `pci::init`'s Kepler dispatch; it also prints `relatch=`, which bits enumeration ITSELF sets, measured rather than assumed (§11.4). **DEVSTA (2026-09-15) closed its one named leftover**: Device Status (`cap + 0x0A`) is re-baselined with the other two, so all three `wedge-sample` deltas now measure the same window | low — the at-arm clear's own write path, one shot, on the BSP | **landed, behind the same `UNAOS_BAR1WEDGE` knob (§11.4; shut-out register §6 P7)**. Rung 1 |
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

---

## 11. BAR1WEDGE — the wedge-theory ladder, and every register field decoded

**Status: IMPLEMENTED, knob-gated, DEFAULT OFF (`UNAOS_BAR1WEDGE=1`, Cargo feature `bar1wedge`,
`drivers/gpu/pcihealth.rs` file tail). Never flown.** Unlike everything in §§2–10, this section is
not a plan: the instrument exists and the next flight can score it. What it is *for* is §11.1's W4 —
the one coded, never-flown experiment the shut-out register names for ledger A1.

Sections 1–10 above are about **recovery**: what to do once the wedge has happened. This section is
about **the wedge itself** — the ladder of theories, written in the form `RULINGS.md` R19 requires
(every rung records the conditions it failed under, its code and its knob are KEPT, and nothing is
ever "ruled out"), and the register facts a boot has to carry for the next rung to mean anything.

### 11.1 The wedge-theory ladder

| # | theory | armed by | last metal verdict | failed under | depends on | status |
| --- | --- | --- | --- | --- | --- | --- |
| W1 | the GK107 drops into an ASPM low-power state under a quiescent link and stops accepting transactions | `UNAOS_NOASPM` (register §6 P3) | boot 11: `[pcih] aspm cleared rp 0043->0040 ep 0043->0040`, **wedged anyway at 118 s**; flight 4 repeated it, three strikes | WC-typed BAR1 aperture (PAT PA4), sustained compositor paint bursts, Kepler FIFO+CE present *and* (rmbp-5 boot 17) absent, ASPM confirmed cleared on BOTH ends | — | **shut-out as a cure; proven as a mechanism.** The switch stays armed: a different failure (L1 substate entry under an idle desktop, a retrain) needs the same lever |
| W2 | the link itself faults or retrains under the burst | — (P4 reads it every kepler boot) | boot 9 `lnksta=d881` (Link Training SET), boot 11 `lnksta=d081` (CLEAR) — same wedge either way | link up and trained at the sample; no AER capability on the Ivy Bridge PEG root port (`aer=n`) to corroborate | W1 | **shut-out — with a caveat this arc found and §11.2 states: two of the bits in `d081` are RW1C latches this kernel had never cleared, so "the link is clean" was read off a register that was partly reporting the whole boot, not the instant** |
| W3 | the holder is parked in a non-posted READ-BACK out of the aperture | — | boot 15's ISR row trace: 99 samples, one a second, `row=897` throughout — the holder is stopped inside one STORE, not slow and not reading (`engine.md` §WCSER-ISR) | — | — | **refuted.** Recorded, not deleted |
| W4 | **WC store-buffer / posted-write backpressure**: the CPU's write-combining buffers drain into a host interface that stops accepting them, and the core dies holding the store | `UNAOS_BAR1EXP=uc` (register §6 P5), scored by `UNAOS_BAR1WEDGE=1` | **NEVER FLOWN.** Flight 5 declined to arm it: *"UC is ~6.8x slower and would corrupt the power numbers"* | — | W1 (ASPM excluded), W2, W3 | **never-run — the ranked next rung.** UC retypes the aperture so the write path is strongly ordered and unbuffered; a wedge under UC exonerates memory type, a wedge-free UC boot convicts the WC drain |
| W5 | PCIe credit exhaustion / the GPU's own BAR1 window path (M2/M3 of `phase31-root.md`) | — | — | — | W4 (it is what W4's UC arm discriminates *against*) | **never-run** |
| W6 | the root port's completion timeout never fires, so a core stalled on the aperture can never be released at all | `UNAOS_BAR1WEDGE` prints the configuration; nothing yet exercises it | never flown — the value has never been READ, let alone tested | — | — | **never-run.** This is the number §3.2's sacrificial probe and §8.2's "is the core freed?" both rest on, and it is a boot-time constant that cost nothing to print and had never been printed |
| W7 | `secsta` bit 13 (Received Master Abort) is the wedge's signature — the endpoint stopped answering | — | boots 8, 9 and 11 all read `secsta=2000` at the wedge | **UNFALSIFIABLE AS READ** (§1.3): a W1C latch this kernel never cleared, and ordinary bus enumeration sets it. `[pcih] rp-boot` (landed with this document) narrowed it to "before or after `pci::init`"; BAR1WEDGE's arm-time clear narrows it to "after `pci::init`". The EHCI driver's own `0..=255` walk is the named remaining contributor | — | **open — and narrowed again by SECSTA2 (§11.4), which also CORRECTED the named contributor: `ehci::init` (`arch/x86_64/pci.rs:838`) runs BEFORE the at-arm clear (`arch/x86_64/pci.rs:1016`), so the EHCI walk was never the residual. The second clear now runs after the last walk of `pci::init`, and its `relatch=` field measures what enumeration itself latches instead of leaving it as a hypothesis** |

**What would change a verdict**

- **W2** — re-read it from a boot whose LNKSTA bandwidth latches were cleared at arm time. If
  `lnksta` at the wedge still carries bits [15:14] after the clear, the link retrained *during this
  boot* and W2 re-opens as "bandwidth renegotiation under burst", which is not the same claim as
  "link training error" and was never separately tested. If they read 0, W2's shut-out is stronger
  than it has ever been, because for the first time the reading is about the instant.
- **W4** — fly `UNAOS_BAR1EXP=uc` with `UNAOS_BAR1WEDGE=1` on a boot scored **wedge / no-wedge**,
  never throughput. Both knobs, or the flight is unscorable: see §11.3.
- **W7** — one boot. Either `d_secsta=0000` at the first stall, and the master-abort reading is dead
  (with it, §3.1's table has no entries left and §3.2's sacrificial probe becomes the only route to a
  classifier); or `d_secsta=2000`, and the latch moved after `pci::init` — still not proof it moved
  at the wedge, because of the EHCI walk, but a much smaller window than the one boot 11 had.
  **Amended by SECSTA2 (§11.4):** the EHCI walk is not the alternative — it precedes the at-arm
  clear. With the post-enum clear in, `d_secsta=2000` at `n=1` means the latch moved after EVERY bus
  walk of `pci::init`, and the only named alternative left is a `wifi`-armed boot's own census from
  the main loop (`wifi/bus.rs:125`) — a knob no A1 flight row asks for (`grep -c UNAOS_WIFI
  docs/dev/OS/rmbp-queue.md docs/dev/OS/rmbp-ledger.md` = 0/0), and one any boot settles from its own
  `⚡ kernel features:` banner. The same boot also
  prints `relatch=`, which says whether a bus walk on this machine latches bit 13 at all — the
  premise the whole "enumeration residue" reading rests on, never once measured.

### 11.2 The registers BAR1WEDGE prints, field by field

Sources: **PCI Express Base Specification Revision 3.0** (§7.8 PCI Express Capability Structure;
§7.10 Advanced Error Reporting Capability) and the **PCI-to-PCI Bridge Architecture Specification
Revision 1.2** (§3.2.5 configuration-space registers). Offsets marked `cap + …` are relative to the
root port's PCIe capability header, which `find_cap` bounds; `0x1E`/`0x3E` are absolute type-1
header offsets. **RW1C** = write-1-to-clear: the bit latches on the event and stays set until
something writes a 1 to it. This kernel had never written a 1 to any of them.

#### Link Status — `cap + 0x12`, 16-bit, PCIe r3.0 §7.8.8

| bits | field | in the quoted `lnksta=d081` | what it says |
| --- | --- | --- | --- |
| [3:0] | Current Link Speed | `0x1` | 2.5 GT/s — Gen1 rate |
| [9:4] | Negotiated Link Width | `0x08` | x8 |
| [10] | Undefined in r3.0 (reserved) | 0 | — |
| [11] | Link Training | 0 | the LTSSM was not retraining **at the instant of this read**. Boot 9's `d881` has it SET; both boots wedged |
| [12] | Slot Clock Configuration | 1 | the port uses the platform's reference clock |
| [13] | Data Link Layer Link Active | 0 | **meaningful only if Link Capabilities [20] (DLL Link Active Reporting Capable) is set** — the census's `lnkcap=` field is where to check. On a non-hot-plug CPU-integrated PEG port it is normally 0, in which case this bit is 0 on a perfectly healthy link and says nothing |
| [14] | Link Bandwidth Management Status | **1** | **RW1C.** Set when the link retrained because bandwidth was renegotiated (or the port's speed/width was changed by software) |
| [15] | Link Autonomous Bandwidth Status | **1** | **RW1C.** Set when the link autonomously changed speed or width for reliability or power |

**This is the finding of the decode, and it is about evidence rather than about hardware.** `d081`
has been quoted three times as "the link is clean and trained at the wedge". Bits [15:14] of it are
since-boot latches, so the honest reading of `d081` was always *"clean at this instant, and at some
point since power-on the link renegotiated its bandwidth at least once"* — which is a fact about the
whole boot, printed in the same field as facts about the instant, with nothing to separate them.
Exactly the `secsta` trap of §1.3, in the register the ladder trusted most. The arm-time clear is
what separates them.

#### Link Control — `cap + 0x10`, 16-bit, PCIe r3.0 §7.8.7

Read by the existing sampler **and then discarded** — `rp_at_wedge` loads the LNKCTL|LNKSTA dword
and keeps only the top half. So until this rung, no capture had the wedge-time value of any of these.

| bits | field | in the census's `lnkctl=0043` | note |
| --- | --- | --- | --- |
| [1:0] | ASPM Control | `0b11` = L0s+L1 | `0040` under `UNAOS_NOASPM`; this is the field the P3 clear writes |
| [3] | Read Completion Boundary | 0 | |
| [4] | Link Disable | 0 | **must stay 0.** A stolen CF8 store landing here is the catastrophe the PCIH-NOCF8 refusal in `census` exists to prevent, and printing it at the wedge is how we would ever find out |
| [5] | Retrain Link | 0 | write-1 to trigger; reads back 0 |
| [6] | Common Clock Configuration | 1 | the `0x40` in `0043` |
| [7] | Extended Synch | 0 | |
| [8] | Enable Clock Power Management | 0 | |

#### Device Status — `cap + 0x0A`, 16-bit, PCIe r3.0 §7.8.5

| bits | field | in `devsta=0000` | note |
| --- | --- | --- | --- |
| [0] | Correctable Error Detected | 0 | **RW1C** |
| [1] | Non-Fatal Error Detected | 0 | **RW1C** |
| [2] | Fatal Error Detected | 0 | **RW1C** |
| [3] | Unsupported Request Detected | 0 | **RW1C** |
| [4] | AUX Power Detected | 0 | read-only |
| [5] | Transactions Pending | 0 | read-only — set while the port has issued non-posted requests that have not completed. **`devsta=0000` at the wedge therefore says the ROOT PORT had no outstanding non-posted request of its own**, which is consistent with W3's refutation (the holder is in a posted store) and is the closest thing the existing capture has to a positive statement about traffic |

#### Device Capabilities 2 — `cap + 0x24`, 32-bit, PCIe r3.0 §7.8.15

| bits | field | note |
| --- | --- | --- |
| [3:0] | Completion Timeout Ranges Supported | one bit per class: A = 50 µs–10 ms, B = 10 ms–250 ms, C = 250 ms–4 s, D = 4 s–64 s |
| [4] | Completion Timeout Disable Supported | whether [4] of Device Control 2 does anything |

#### Device Control 2 — `cap + 0x28`, 16-bit, PCIe r3.0 §7.8.16

**The register W6 turns on, and nothing in this tree had ever read it.**

| bits | field | encodings (r3.0 Table 7-20) |
| --- | --- | --- |
| [3:0] | Completion Timeout Value | `0x0` 50 µs–50 ms (default) · `0x1` 50–100 µs (A) · `0x2` 1–10 ms (A) · `0x5` 16–55 ms (B) · `0x6` 65–210 ms (B) · `0x9` 260–900 ms (C) · `0xA` 1–3.5 s (C) · `0xD` 4–13 s (D) · `0xE` 17–64 s (D) · all others reserved |
| [4] | Completion Timeout Disable | **1 = the timeout mechanism is OFF and a non-posted request that is never answered is never abandoned.** If Apple's firmware leaves this set, §3.2's sacrificial endpoint probe does not merely "probably survive" — it is guaranteed not to, and §8.2's hope that an SBR frees the seized core loses its main mechanism |

These two registers exist only from **PCIe capability version 2** (PCI Express Capabilities Register,
`cap + 0x02` bits [3:0], §7.8.2). On a version-1 capability `cap + 0x24` is whatever the device put
there next, so the rung checks the version first and prints `v2=0` / `cto rp UNREADABLE` rather than
a fiction.

#### Secondary Status — bridge offset `0x1E`, 16-bit, PCI-to-PCI r1.2 §3.2.5.7

| bits | field | in `secsta=2000` | note |
| --- | --- | --- | --- |
| [8] | Master Data Parity Error | 0 | **RW1C** |
| [10:9] | DEVSEL Timing | 0 | read-only |
| [11] | Signaled Target Abort | 0 | **RW1C** |
| [12] | Received Target Abort | 0 | **RW1C** |
| [13] | **Received Master Abort** | **1** | **RW1C** — the `0x2000`. Every config probe of an absent device below this bridge sets it, which is why §1.3 calls the reading ambiguous |
| [14] | Received System Error | 0 | **RW1C** |
| [15] | Detected Parity Error | 0 | **RW1C** |

#### Bridge Control — bridge offset `0x3E`, 16-bit, PCI-to-PCI r1.2 §3.2.5.18

Sampled at boot by the `[pcih] rp-boot` line and **not touched by this rung**. Bit [6] is Secondary
Bus Reset; §6.2's pulse is a read-modify-write of this register and every other bit in it (VGA
enable, ISA enable, error forwarding, the parity/SERR enables) must be carried through unchanged.

#### AER — `aer + 0x04` and `aer + 0x10`, 32-bit, PCIe r3.0 §7.10.2 / §7.10.5

Uncorrectable Error Status (`+0x04`): [4] Data Link Protocol Error · [12] Poisoned TLP · [13] Flow
Control Protocol Error · **[14] Completion Timeout** · [15] Completer Abort · [16] Unexpected
Completion · [17] Receiver Overflow · [18] Malformed TLP · [19] ECRC Error · [20] Unsupported Request
Error. Correctable Error Status (`+0x10`): [0] Receiver Error · [6] Bad TLP · [7] Bad DLLP ·
[8] REPLAY_NUM Rollover · [12] Replay Timer Timeout · [13] Advisory Non-Fatal Error. All RW1C.

**On the bench machine there is nothing here to read, and that is itself the fact.** The census has
printed `rp … aer=n` on every boot since 8: the Ivy Bridge PEG root port genuinely has no AER
extended capability. The *endpoint* has one (`ep … aer=y`), and the sampler will never read it —
that is the module's first refusal, and it is not negotiable while the endpoint's host interface is
the thing under suspicion. Recovering the endpoint's AER log is a job for the recovery task of §4,
after a condemn, through `ep_ecam_page()`.

### 11.3 What the flight prints, and what each outcome means for A1

Flight line: the existing rMBP line **plus `UNAOS_BAR1WEDGE=1`**, and — for the rung that pays —
**plus `UNAOS_BAR1EXP=uc`**. Both, or the boot is not the experiment: `bar1wedge` without
`bar1exp=uc` is a baseline WC boot with a better instrument (worth one boot on its own), and
`bar1exp=uc` without `bar1wedge` is flight 5's refusal repeated — a single bit that the 6.8x
slowdown can explain away.

At kepler init, three lines:

```
:: BAR1WEDGE: rung=first-stall armed=UNAOS_BAR1WEDGE aperture=uc rp=0:1.0 capver=2 v2=1 baseline=lnksta=d081(2.5GT/s x8) lnkctl=0043(aspm=L0sL1) devsta=0000 secsta=2000 devctl2=.... aer=n ::
[pcih] bar1wedge cto rp devcap2=........ ranges=. cto_dis_sup=. devctl2=.... value=... dis=. — the bound a non-posted read to a silent endpoint completes within
[pcih] bar1wedge sticky-cleared at-arm lnksta d081->1081 devsta 0000->0000 secsta 2000->0000 (w1c written c000/0000/2000) — EHCI's later bus walk can still re-latch secsta; this narrows the window, it does not close it
```

Then ONE more line — **not at kepler init**, but later in the same `pci::init`, after its last bus
walk (SECSTA2, §11.4):

```
[pcih] bar1wedge sticky-cleared post-enum rp=0:1.0 secsta=....->0000 lnksta=....->.... relatch=secsta:.... lnksta:.... at-arm=secsta:0000 lnksta:1081 (w1c written ..../....) — relatch is what ENUMERATION set after the at-arm clear; wedge-sample d_secsta/d_lnksta now delta against THIS baseline, d_devsta still against at-arm
```

Then, at every `[wcser] PASS OVERDUE … == tripwire ::` crossing, beside the unchanged
`[pcih] rp-at-wedge` line:

```
[pcih] wedge-sample n=1 first=1 aperture=uc lnksta=.... d_lnksta=.... (...) lnkctl=.... lnkctl0=0043 aspm=... lnkdis=0 retrain=0 devsta=.... d_devsta=.... secsta=.... d_secsta=.... devctl2=.... cto=... dis=. aer=n uesta=00000000 cesta=00000000
```

Read `n=1` — that is the first stall, the sample this rung exists for. Read it with
`awk 'index($0,"[pcih]")'`, never a bare `grep`. Since the DEVSTA close (§11.4) all three deltas on
this line — `d_lnksta`, `d_secsta`, `d_devsta` — are measured from the SAME baseline, the read-back
at the end of enumeration; before it, `d_devsta` alone was measured from `pci::init`'s Kepler
dispatch, so the three could not be compared with each other.

| what the wire says | what it means for ledger A1 |
| --- | --- |
| `aperture=uc` present and **no `[wcser] PASS OVERDUE` in a full `storm`** | W4 CONVICTED: the WC posted-write drain is the wedge, and the fix is a memory-type or fencing change on the blit path rather than anything in PCIe. The knob-off baseline (`aperture=wc`, same boot length, same storm) is the control and must wedge, or the boot proves only that the storm was weak |
| **⬅ FIRED (flight 9)** `aperture=uc` and the wedge happens anyway | W4 EXONERATED and the aperture's memory type leaves the ladder: the store is not being held by CPU write-combining. W5 (credits / the GPU window path) becomes the head of the ladder with nothing above it |
| `lnkdis=1` at any crossing | STOP EVERYTHING. The link was disabled by software, and the only software that writes LNKCTL is this kernel — PCIH-NOCF8's stolen-store hazard would be realised, not theoretical |
| `d_lnksta=c000` (or either bit alone) at `n=1` | the link renegotiated bandwidth **during this boot**, after `pci::init`. W2 re-opens as "bandwidth renegotiation under burst" — a claim never separately tested, and not the "link training error" W2 was shut out on |
| **⬅ FIRED (both legs, 28/28 samples)** `d_lnksta=0000` across every crossing | W2's shut-out is confirmed on an instrument that can finally tell the instant from the boot |
| **⬅ FIRED (both legs)** `relatch=secsta:2000` on the `sticky-cleared post-enum` line | **enumeration on this machine DOES latch Received Master Abort** — the premise §1.3 argued from, measured for the first time. The `secsta=2000` of boots 8/9/11 is then fully explained without the wedge, and W7's reading dies on evidence rather than on an argument about what bus walks generally do. It also makes the post-enum baseline load-bearing rather than tidy: `d_secsta` at `n=1` is now the only master-abort reading worth quoting |
| `relatch=secsta:0000` | no bus walk below this bridge master-aborted at all this boot, so `secsta` was NOT being set by enumeration after the at-arm clear. A `d_secsta=2000` at `n=1` then has one named alternative left (a `wifi`-armed boot's own census, `wifi/bus.rs:125` — a knob no A1 flight row asks for, `grep -c UNAOS_WIFI docs/dev/OS/rmbp-queue.md docs/dev/OS/rmbp-ledger.md` = 0/0, and one the boot's own `⚡ kernel features:` banner settles) and otherwise points at the wedge |
| `relatch=devsta:0004` (Unsupported Request Detected) or any nonzero `devsta:` bit | **enumeration on this machine latches a Device Status error bit** — the same measurement `relatch=secsta:` makes, for the register that records the root port's OWN errors rather than the bridge's secondary side. Whatever `d_devsta` then reads at `n=1` is about the burst and not about the bus walks, which before the DEVSTA close it could not be, because `d_devsta` was measured from the Kepler dispatch while its neighbours were measured from the end of enumeration |
| `relatch=devsta:0000` with `devsta=0000->0000` | the expected reading, and now an asserted one rather than an assumed one: nothing between the at-arm clear and the end of enumeration set a correctable, non-fatal, fatal or unsupported-request bit on the root port. A nonzero `d_devsta` at `n=1` then belongs to the burst |
| `devsta=....->0004` (the after value nonzero) | a latch that did not clear, read exactly as the `secsta=....->2000` row below: a sticky `1` after a plain RW1C write is a hardware fact worth its own rung, and every later `d_devsta` on that boot is measured against a nonzero baseline that the line prints |
| `relatch=lnksta:c000` (or either bit alone) | the link renegotiated bandwidth DURING ENUMERATION — before any compositor paint. Whatever `d_lnksta` then reads at `n=1` is about the burst and not about boot-time link churn, which is the confound that made `d081` unreadable in the first place |
| the `sticky-cleared post-enum` line is ABSENT on a boot whose `:: BAR1WEDGE:` line is present | the call site did not run. It is guarded on `PCIH_READY` and sits at the tail of `pci::init`, so its absence with the arm line present means `pci::init` did not reach its end — a boot that died in the GPU/SDHC/NIC tail, which is itself the finding |
| `secsta=....->2000` (the after value nonzero) | a latch that did not clear. The write is a plain RW1C to a bridge status register, so a sticky `1` in the read-back is a hardware fact worth its own rung, and every later `d_secsta` on that boot is measured against a nonzero baseline (the line prints it, so nothing is silently wrong) |
| `d_secsta=0000` at `n=1` | W7 DEAD: nothing master-aborted below the bridge after `pci::init`, so the `secsta=2000` of boots 8/9/11 was enumeration residue. §3.1's classifier table is then EMPTY and §3.2's sacrificial probe is the only remaining route to one |
| **⬅ FIRED (both legs) — but see the flown paragraph: the named alternative is LIVE** `d_secsta=2000` at `n=1` | the latch moved after `pci::init`. Not yet proof it moved at the wedge — the EHCI bus walk is the named alternative — but the window is now minutes rather than the whole boot, and the next rung (a second clear once enumeration completes) closes it |
| `dis=1` in the `cto` line | W6 CONVICTED without a flight of its own: completion timeouts are disabled on this port, §3.2's prober is guaranteed to be lost rather than "probably" surviving, and §8.2's argument that an SBR frees the seized core loses its mechanism |
| **⬅ FIRED, with a caveat (both legs read `value=50us-50ms(default)`, the spec default, `devcap2=00000000 ranges=0`)** `dis=0` with a `value=` in class A or B | the prober survives within tens of milliseconds; §3.2's rung is cheap and §3.3's classifier is buildable |
| `v2=0` / `cto rp UNREADABLE` | the root port's PCIe capability is version 1 or sits too high in config space. W6 stays unanswerable on this machine and the reason is on the wire instead of being inferred from a missing line |
| the `:: BAR1WEDGE:` line is ABSENT on a boot whose banner claims `bar1wedge` | the build is the defect, not the hardware. Check the artifact with `LC_ALL=C grep -a -o -F ':: BAR1WEDGE:'` before reading anything else into the boot |

**FLOWN 2026-09-16 — WHICH OUTCOME FIRED (flights 8 WC control and 9 UC experiment, one `storm`
each; evidence [`docs/dev/evidence/rmbp-0916/flight8-9/FLIGHT8-9.md`](../../evidence/rmbp-0916/flight8-9/FLIGHT8-9.md) §5).**
The UC leg wedged: `[wcser] PASS OVERDUE holder=c1 … blit_inflight=1` fired **12 times under UC and
16 times under WC**, so the row that fired is **`aperture=uc` and the wedge happens anyway — W4 is
EXONERATED**, the panel aperture's memory type leaves the ladder, and W5 (credits / the GPU window
path) becomes the head with nothing above it. Three more rows fired with it and all three are about
evidence rather than hardware: `d_lnksta=0000 d_devsta=0000 lnkdis=0` on **all 28 samples of both
boots**, so W2's shut-out is confirmed on an instrument that can finally tell the instant from the
boot; `relatch=secsta:2000` on both legs, so **enumeration on this machine DOES latch Received
Master Abort** — the premise §1.3 argued from, measured for the first time, and W7's residue reading
dies on evidence rather than on an argument about what bus walks generally do; and `d_secsta=2000`
at `n=1` on both legs, which **does NOT reach the wedge**, because this flight found the alternative
this section dismissed to be live: both images carried `UNAOS_WIFI=1 UNAOS_WIFI2=1`, and
`wifi::bus::census()`'s full `for bus in 0u16..256` configuration sweep ran **253 ms AFTER the
post-enum sticky clear** (clear at 23263 ms, `:: wifi: brcm net function 03:00.0 …` at 23516 ms),
below the same bridge — see rmbp-ledger B113, and move the clear below the census before re-reading
this field. No row fired for `lnkdis=1`, `d_lnksta=c000`, `relatch=secsta:0000`,
`relatch=lnksta:c000`, `secsta=…->2000`, `d_secsta=0000`, `dis=1`, `v2=0`, or either ABSENT-line
row: `capver=2 v2=1`, `cto … value=50us-50ms(default) dis=0` (the spec default with
`devcap2=00000000 ranges=0`, so the port advertises no optional class and the prober is bounded at
50 ms — §3.2's cheap case), and both `[pcih] rp-boot` and `[pcih] rp-at-wedge` present and unchanged
in shape on both legs. **Both machines survived**: presents continued, no `PANIC`, no `REHOMED`, no
`DEAD c<n>`; the UC leg's price is on the present path instead — `[schedx86] depth … inflight=59`,
shell `longpres=20`, `maxpresent_us=69120` against WC's `0` and `11630` (rmbp-ledger A13).

**⚠ THE ABSENCE CONTROLS BELOW WERE UNSCORABLE ON THIS FLIGHT, and the reason is a defect of our
own (rmbp-ledger B112).** `:: x86 bar1exp: UC arm ARMED` printed **0 times on the UC leg** and
`:: x86 fb-wc:` printed **0 times on the WC leg** — neither alternative at that site reached the
cable, because the retype moved to `main.rs:112`, ahead of the FTDI mirror this laptop depends on,
while the `BPACE: fb-wc` / `fb-wc-done` stamps inside the same one-shot latch prove the function ran
on both boots. (`fb-wc` "present" is that BPACE stage stamp, which prints on flight 7 too and is not
the framebuffer's memory type.) **The arm is nonetheless proven, on a line this section did not
pre-register:** `:: x86 mmio-map: 0x90000000..0xa0000000 uc=113 (PAT PA3) wc-kept=15 ::` on the WC
leg against `uc=128 (PAT PA3) wc-kept=0` on the UC leg — the only differing line in each boot's
eight-line map set, and the code's own documented UC signature. **And the field a reader would reach
for instead is a compile-time constant:** `pcihealth.rs::bw_aperture()` is
`if cfg!(feature = "bar1exp-uc") { "uc" } else { "wc" }`, so `aperture=uc` on every line above names
which BUILD flew and never what the page tables carry. Re-state these controls against the
`mmio-map` pair before the next A1 flight. **RESOLVED 2026-09-16 (PHASE31WIT), and the transport was
worse than "ahead of the FTDI mirror" — the line was emitted, mirrored, and then EVICTED: this
laptop's only carrier is a drop-oldest ring that does not replay until `ftdi:console-up` (23437 ms),
and the flight-8/flight-9 replays measure 260 958 and 258 584 bytes against a 262 144-byte cap, i.e.
pinned at capacity on both boots and each beginning mid-token, with the UNCONDITIONAL `:: video:
WRITER seeded` absent from both as the control no cfg arm can explain; the witness is now latched at
the retype and spoken from `bootpace::service_dump` as `:: x86 bar1exp: UC arm ARMED
via=<create|retype> leaves=<n> range=<lo>..<hi> ::`, the ring is 1 MiB, and `:: FTDI-CAP: replayed=…
cap=… lost=… head_cut=…` states the capture's own integrity on every boot — so the controls below
are scorable as written on the next flight, and `via=` additionally reports WHICH path armed UC.**

**Absence controls for this flight, pre-registered:** `fb-wc` must be ABSENT and
`:: x86 bar1exp: UC arm ARMED` PRESENT on the UC leg (they are alternatives at the same site); the
reverse on the WC control leg. `[pcih] rp-boot` and `[pcih] rp-at-wedge` must be present on BOTH
legs and unchanged in shape — this rung adds lines and clears latches, it removes nothing.

### 11.4 SECSTA2 — the second clear, and the walk that was named wrongly

**Status: IMPLEMENTED behind the SAME knob (`UNAOS_BAR1WEDGE=1`, no new knob), DEFAULT OFF, never
flown.** Shut-out register §6 rung **P7**. Code: `drivers/gpu/pcihealth.rs`'s
`sticky_clear_post_enum`, called from ONE site at the tail of `arch/x86_64/pci::init`.

§11's BAR1WEDGE block closed with a residual in its own words: *"`census` runs inside `pci::init`,
and the EHCI driver's own `0..=255` bus walk happens LATER, so a master abort it provokes can
re-latch secondary status after this clear … Closing it needs a second clear once enumeration is
complete — a second call site, in another file."* Building that clear found the residual was real
and its attribution was not.

**The EHCI walk is not later.** `crate::drivers::ehci::init()` — whose walk is
`drivers/ehci/mod.rs:17468` — is called from `arch/x86_64/pci.rs:838`. The Kepler dispatch that
reaches `pcihealth::census`, and so `bw_arm`'s at-arm clear, is at `arch/x86_64/pci.rs:1016`. EHCI
enumerates **before** the at-arm clear; anything it latched is inside the `secsta=` that line prints
as its "before" value and is wiped by the write that follows. The enumeration that genuinely
survives the at-arm clear is the tail of the same function:

| walk | site | buses | runs |
| --- | --- | --- | --- |
| `sdhc::probe` → `PciScanner::storage_inventory` | `drivers/pci.rs:84` | 0..=255 | unconditionally |
| `ahci::probe` | `drivers/ahci.rs:286` | 0..=255 | `UNAOS_AHCI=1` only |
| `init_network` → `PciScanner::find_device` | `drivers/pci.rs:141` | 0..=255 | unconditionally — **the last walk `pci::init` performs** |

So the second call site is not in `drivers/ehci` at all: it is the boot's "all buses enumerated"
point, the tail of `pci::init`, after the GPACE report block (before it, the call would land inside
`span` and inflate `resid`, making a knob-ON boot's pacing row disagree with every baseline taken
without the knob). It is a **line-neutral append** to that block's closing brace, the same shape and
for the same reason as the AHCI hook three statements above it: a cfg'd-OFF block still shifts
`panic::Location` line numbers below it, and knob-off x86 image byte-identity is this rung's stated
invariant.

**What it does.** Re-read Secondary Status (`0x1E`), Link Status (`cap + 0x12`) and Device Status
(`cap + 0x0A`) on the root port; compute `relatch` = the RW1C bits set now that the at-arm read-back
did not carry; write the observed-set RW1C bits back (never a bit that was not read as set, never a
register with nothing latched, never a control register); read back; print; and store the read-backs
as the baselines `bw_sample`'s `d_secsta` / `d_lnksta` / `d_devsta` delta against. The at-arm values
stay on the wire in the same line, so nothing is lost by overwriting the statics.

**`relatch=` is the finding.** Everything §1.3 argues rests on a premise nobody had measured: that
ordinary bus enumeration sets bit 13 on THIS bridge. `relatch=secsta:2000` measures it true;
`relatch=secsta:0000` measures it false, and a `d_secsta=2000` at the first stall then has almost
nothing left to blame but the wedge. Either way W7 stops being an argument about what bus walks
generally do. Score card: the four `relatch`/`post-enum` rows in §11.3.

**Three properties held, and one deliberately not.**

* **No new knob.** Everything is behind the existing `bar1wedge` feature; the arroyo map, the
  builder read and the `k8-reach.registry` row are untouched.
* **Every write is a W1C to a STATUS register** — `0x1E` and `cap + 0x12`, both inside the legacy
  256-byte config region, both bounded by the predicates `bw_arm` already asserts.
* **Nothing on the input band or in an ISR.** One shot, on the BSP, inside `pci::init` — the same
  sequential boot phase `census` reads config space in and `bw_arm` already writes it in. The write
  path is CF8/CFC, exactly the at-arm one, so PCIH-NOCF8's refusal (which is about the ~1 kHz
  non-BSP tripwire band) is untouched and there is no ECAM mapping whose writability would have to
  be re-verified at this later point.
* **Device Status: WAS the deliberate leftover, now CLOSED (DEVSTA, 2026-09-15).** SECSTA2 left
  `BW_DEVSTA0` holding its at-arm value, so `d_devsta` on the `wedge-sample` line deltaed against
  the Kepler dispatch while `d_secsta`/`d_lnksta` deltaed against the end of enumeration, and it
  named the close as "two lines in `sticky_clear_post_enum`, left to a seat rather than taken
  silently". Those two lines are now in: `cap + 0x0A` is read, its `DEVSTA_W1C` bits are cleared on
  the same write path, and the read-back is stored, so **all three deltas measure the same window**
  and the line carries `devsta=<before>-><after>` and `relatch=… devsta:<bits>` beside the other
  two. The VALUE is still `devsta=0000` on every capture there has ever been; what changed is the
  reading hazard — three deltas on one line read as one measurement, and one of them silently was
  not. A `d_devsta=0004` (Unsupported Request Detected, the bit a read of a wedged BAR is likeliest
  to set) could have been latched by any of the three post-at-arm walks in the table above with
  nothing on the wire to say so; `relatch=devsta:` is now that measurement. No new register mapping
  was verified for it: `cap + 0x0A` is the offset `bw_arm`'s at-arm clear already reads and writes
  through CF8, and `0x0A < 0x12` puts it inside the span `cap_fits(cap, PCIE_CAP_SPAN)` already
  bounds. No new knob, no new call site, no QEMU fixture (q35 has no Kepler, see below): `check`
  only.

**Residual, in the same voice.** `wifi::service` (`wifi/bus.rs:125`, buses 0..=255, knob
`UNAOS_WIFI`) sweeps config space from the main loop, i.e. after this clear, on every boot that arms
it. No A1 flight row asks for that knob (`grep -c UNAOS_WIFI docs/dev/OS/rmbp-queue.md
docs/dev/OS/rmbp-ledger.md` = 0/0) and any boot settles it from its own `⚡ kernel features:`
banner; on one that did carry it, the window would be "after the first wifi census" and `d_secsta`
would have that one alternative left.

**q35 says nothing about this rung.** `sticky_clear_post_enum` is guarded on `PCIH_READY`, which is
set only at the end of `pcihealth::census`, which runs only from `kepler::init`. QEMU q35 has no
GK107, so `census` never runs, `PCIH_READY` stays false, and the post-enum line is honestly absent.
There is no QEMU fixture for this rung; it is scored on metal, on the A1 flight, or not at all.
