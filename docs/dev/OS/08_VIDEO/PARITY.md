# Desktop parity: the complete gate audit (x86 `wc` ⇄ Pi 4 `pidesk`)

**Peter's ruling, and the premise of this document: ONE OS.** The crispy desktop experience is not an
x86 feature that the Pi approximates — it is *the* experience, and an `x86_64`-only gate around any
part of it is a defect until it is accounted for in writing.

Every gate therefore ends in exactly one of **three** dispositions, not two:

1. **PORT IT** — the Pi is missing something a user sees, and the code is not hardware-bound.
2. **LEGIT ARCH-SPECIFIC** — it is x86 hardware (Kepler, EDID, APIC, port I/O) and cannot be neutral.
3. **RULED OUT** — the x86 mechanism is *not* the experience Peter wants, so matching it would be
   parity with the wrong thing. §5.1 is the worked example, and it is the disposition most likely to
   be missed: **parity is a rule about the EXPERIENCE, not about the code.** An x86-only mechanism is
   only a gap if what it produces is what users should feel. Check that before porting anything.

This table exists for one reason: **so that no parity milestone is claimed twice and none is missed.**
Every gate that excludes aarch64 from the desktop experience is enumerated below with an owner. Before
announcing a parity win, find its row. Before starting parity work, find its row. If a gate is not in
this table, the table is wrong — regenerate it (§1) and fix it.

Baseline for the census: `da06e3ef` (hw-pi4 tip). Deltas from the `exec-vugparity` arc are marked.

---

## 1. Method — how to regenerate this census

The census is mechanical, so it can be re-taken after any arc rather than trusted from memory. From
`unaos/crates/kernel/src`:

```sh
python3 - <<'EOF'
import glob
files = ["main.rs", "pal.rs", "ui_status.rs"] + sorted(glob.glob("video/*.rs"))
for f in files:
    L = open(f, encoding="utf8", errors="replace").read().split("\n")
    for i, s0 in enumerate(L):
        s = s0.strip()
        if s.startswith("#[cfg(") and "x86_64" in s:
            j = i + 1
            while j < len(L) and (L[j].strip().startswith("#[")
                                  or L[j].strip().startswith("//")
                                  or not L[j].strip()):
                j += 1
            print(f"{f}:{i+1}\t{s[:130]}\t{L[j].strip()[:110] if j < len(L) else '<EOF>'}")
EOF
```

Two cautions learned the hard way while taking it:

* **Do not count raw `x86_64` occurrences.** `grep -c 'target_arch = "x86_64"'` reports 237 in `wm.rs`
  and 90 in `fbcon.rs`; those numbers include the paired `not(...)` arms (which are *evidence of
  porting*, not gates), the inline `cfg!()` macro form, and multi-predicate attributes. The gate count
  that matters is attribute *sites*, split by predicate family.
* **The family is the unit of judgement, not the line.** A 42-site block in `fbcon.rs` is one
  subsystem (the console window), not 42 independent decisions.

### Census at `da06e3ef` — 501 attribute sites, by predicate family

| # | Family | Predicate | Sites | Bearing on the desktop experience |
|---|---|---|---|---|
| 1 | **Already ported** | `any(all(x86_64, wc), all(aarch64, pidesk))` | 10 | The `pidesk` furniture seam. Class (a) by construction. |
| 2 | **`wc` desktop gates** | `all(x86_64, wc)` | 67 | **The primary parity surface.** Broken out in §2. |
| 3 | **Plain arch gates** | `all(x86_64, <non-`wc` feature>)` or bare | 165 | Mixed; broken out in §3. |
| 4 | **Witness** | `all(witness, x86_64, …)` | 162 | Test instrumentation, not user experience. Class (c) as a family — see §4. |
| 5 | **`wcg-paygo`** | `all(x86_64, wcg-paygo)` | 44 | The paygo glass-verify sampler: a witness of the compositor, not a user-facing behaviour. Class (c) as a family. |
| 6 | **`not(...)` arms** | `not(all(x86_64, …))` / `not(x86_64)` | 53 | The **aarch64 side** of already-split gates. Evidence of class (a), never a gap. |

The brief's known counts are confirmed against family 2: `fbcon.rs` 42 ✓, `wm.rs` 15 ✓, `mod.rs` 2 ✓;
`main.rs` is **5**, not 4, and `screen.rs` is **2**, not 3. `wcf.rs` has **0** gate sites — its two
`x86_64` mentions are one doc-comment quotation and one inline `cfg!()` in a `let`, neither of which
is an attribute gate.

---

## 2. Family 2 — the `all(x86_64, "wc")` desktop gates (67 sites)

| Sites | Subsystem | Class | Owner / disposition |
|---|---|---|---|
| `wm.rs` ×13 | **WPACE-PANEL present pacer** — `PACE_FRAME_US`, `PACE_LAST_CYC`, `PACE_PENDING`, `pace_frame_cycles`, `pace_admit`, `pace_service`, and the 5 decision sites in `present_banded` | **NOT A GAP — RULED OUT** | **Do not port. See §5.1.** |
| `wm.rs` ×1 | `wpace_emit` — the `[wpace]` ledger line | **NOT A GAP — RULED OUT** | The ledger for the pacer above; same ruling, §5.1. |
| `wm.rs` ×1 | `dock_tiles` — the dock's tile count feeding `occ_clip` | (b) | **LANDED** — `exec-occ62` M2 (§6.4). |
| `wm.rs` ×1 (16493) | MENUFIT witness — menubar reservation line | (d) IN FLIGHT | **exec-conswin** (menubar). |
| `fbcon.rs` ×42 | **The console window** — `CONSOLE_WIN`, the route/pace/pending machinery (`ROUTE_BUSY`, `PACE_HZ`, `pend_merge`, `route_present*`, `console_service`, `console_flush`), the window-backed console store (`win_store`, `win_fb`, `win_content_extent`), `panel_console_window_open` / `_closed` | (b) | **CLOSED** — CONSWIN-PI widened the whole family to `any(all(x86_64, wc), all(aarch64, pidesk))` and gave the Pi a console window; **LIVECON** (§6.12, `livecon` knob) closed the half CONSWIN-PI measured and reverted, by moving the presents off print context onto the render core so the window's text is LIVE instead of a frozen boot-log snapshot. The route/pace machinery is the MECHANISM of that fix and is itself unchanged. |
| `main.rs` ×1 | `open_shell_window` | (d) IN FLIGHT | **exec-shellport.** |
| `main.rs` ×2 | `desktop_owns_backdrop` (SHELLNOTDESK) — the crispy scene as the desktop layer, shell demoted to plumbing | (b) | **CLOSED** — §6.1. SHELLWIN-PI (`b2e0fb4a`) put the live shell in a window; REALDESK (exec-realdesk) took the last two backdrop tenants — the pulse band and the status line — off the glass with it. |
| `main.rs` ×2 | `instgui::service`, `instgui::consume_key` | (c) | `instgui` is the x86 installer GUI on the `wcx` panel path. |
| `mod.rs` ×2 | `pub mod wcx;`, `pub mod instgui;` | (c) | `wcx` **is** the x86 panel path (ignited by the Kepler takeover); `instgui` rides it. |
| `screen.rs` ×1 (1050) | Occluder-array assembly for the blit clip | (b) | Part of the occlusion family — **STILL OWED** after `exec-occ62`, which took the window-blit clip but not the deferred-erase side. See §6.10. |
| `screen.rs` ×1 (367) | `DESK_STRIP_MAX` — furniture-strip array bound | (d) IN FLIGHT | **exec-conswin** (furniture strips). |
| `ui_status.rs` ×1 | `top_chrome_h` — menubar row reservation in the shared layout | (d) IN FLIGHT | **exec-conswin** (menubar). |

---

## 3. Family 3 — the plain arch gates (165 sites)

Classified individually; the full per-site list was taken with the §1 census. Totals:

| Class | Sites |
|---|---|
| (a) PORTED ALREADY — an aarch64 arm exists | 42 |
| (c) LEGIT ARCH-SPECIFIC | 106 |
| **(b) EXPERIENCE GAP** | **17** |

### (c) — the justification, by group

These are x86 *hardware* or x86-only subsystems. They are not defects and should not be re-audited.

| Group | Sites | Why it cannot be arch-neutral |
|---|---|---|
| ACPI / APIC / SMP bring-up | `main.rs` ×10 | RSDP, MADT, DMAR, PM-timer, TSC calibration, INIT-SIPI-SIPI. aarch64 discovers CPUs via the DTB and paces off `CNTFRQ_EL0`. |
| Apple rMBP hardware | `main.rs` ×3, `pal.rs` ×1 | EHCI internal keyboard/trackpad, SMC battery (port I/O), internal SD (`sdhcblk`). |
| Intel iGPU (`intel-ivb`) | `main.rs` ×1, `framebuffer.rs` ×2 | GPU blitter fill/copy, boot-trace publish. |
| Kepler / `wcx` panel path | `fbcon.rs` ×6, `mod.rs` ×1 | `PANEL_CONSOLE`, Retina `PANEL_SCALE`, `panel_console_resume`, `PanelSink`. The compositor's ignition on x86 *is* the Kepler takeover. |
| Uncached-PCIe VRAM shadow | `fbcon.rs` ×9 | `shadow_store` exists because x86 scan-out memory is uncached PCIe; the Pi's framebuffer is not. |
| `videobench` / `videocap` | `framebuffer.rs` ×2, `fbcon.rs` ×4, `mod.rs` ×1 | Bench levers over the x86-only `vperf` module. |
| x86-only compositor gate | `wm.rs` ×8 | `COMP_GATE` / `COMP_PENDING` / `COMP_RERUN_MAX` + `wcser_emit`. Documented in-tree as "x86 ONLY, and the gate is the justification". |
| WCD-TEARDOWN interlock | `wm.rs` ×7, `wcg.rs` ×1 | `panel_seq`, `vrect`, `panel_stable`. In-tree doc: "aarch64 has no interlock at all". |
| Wide glass read-back | `wcg.rs` ×5 | x86 scan-out `GlassRow` reads. |
| ACPI soft-off | `crystal.rs` ×1, `instgui.rs` ×1 | S5 poweroff; the Pi has no soft-off and its arm answers honestly (PSCI). |
| x86 PCI wifi (bcma), flight recorder, selfhost, irqstorage, installdemo | `main.rs` ×15 | Broadcom over x86 PCI (not the Pi's SDIO part); the recorder taps the x86 serial print seam. |
| Occlusion *witness legs* | `crystal.rs` ×1, `menubar.rs` ×1, `wm.rs` ×1, `wcg.rs` ×8 | These probe `wm::occ_clip`. §6.2 has landed (`exec-occ62`), so the `wcg.rs` legs — `occluded=`/`occ=` on `[wc-d]`/`[wc-g]` — are now LIVE on aarch64 and carried the proof. The `[drag-occ]` legs (`occclip_dock`/`occclip_bar`, `wm.rs`) are deliberately **still x86**: that wire belongs to the drag instrument, not to §6.2. See §6.10. |

---

## 4. Families 4 and 5 — witness (162) and `wcg-paygo` (44)

**Class (c) as families, and deliberately so.** Both are instrumentation: `witness` is the boot-witness
harness, `wcg-paygo` the amortised glass-verify sampler. Neither paints anything a user sees; both
exist to *prove* the compositor correct on the arch that hosts the bench. Two consequences worth
stating so they are not mistaken for gaps:

* The `arm-pi` build **does** carry `witness`, so a witness counter used inside a newly-ported body
  must have its own gate ported alongside it or the build breaks — a real failure mode, hit and fixed
  during this arc's (since-removed) pacer port. Expect it on any port whose body touches a counter.
* The **launch-stall chunking** arc (`e855655c` / `24ac6b79`, "the ~1.26 s launch freeze") was a fix to
  the WC-D *witness*, not to the compositor: the freeze was the witness's un-chunked glass read-back.
  With `witness` off there is no freeze to fix, and WC-D is x86-only regardless. **Not an experience
  gap; nothing to port.** This is recorded here specifically so it is not re-opened as one.

---

## 5. The `exec-vugparity` arc

### 5.1 vug present pacing — **RULED OUT, do not port**

> **vug present pacing: RULED OUT by Peter 2026-08-13 — the vug upgrade direction is drawing
> complexity, unpaced presents are the desired behavior on every chip.**

This row exists to stop a future session "fixing" it. The 14 `wm.rs` gates in §2 that carry the
WPACE-PANEL pacer are **not** an experience gap, and the Pi's unpaced present path is **not** a defect
to be closed.

The reasoning that made it look like one is recorded here so the mistake is not repeated. The audit
found that x86 admits one composite per window per 16.67 ms frame and coalesces the rest, while the Pi
composites on every ring-3 present; that asymmetry is real, and this arc initially ported the pacer to
`any(all(x86_64, wc), all(aarch64, pidesk))` on the reasoning that a difference in how vugs *feel*
between the two machines is exactly what "ONE OS" forbids. **That reasoning was wrong, and it was wrong
in a way worth naming: parity is a rule about the EXPERIENCE, not about the code.** Making the Pi match
an x86 mechanism is only parity if the mechanism is what Peter wants users to feel. Here it is not:

* Peter's judgement on the bench is that **vug on the Pi was working better unpaced** — pacing makes
  the app feel like it is *fighting itself*.
* His standing direction for vug is **more drawing complexity, never artificial pacing.** A vug that
  wants to look richer should draw more, not present less.

So the correct disposition of these gates is: leave them x86-only, and treat the Pi's behaviour as the
reference rather than the deficit. The port was removed from this arc; `wm.rs` and `main.rs` are
byte-identical to `da06e3ef`.

**Corroboration from the x86 side.** Peter also notes that the x86 desktop's own shipped polarity
points the same way — the `arroyo` leg comments record **vsyncpace POLARITY INVERTED**, i.e. the
shipped x86 desktop is itself **unpaced**. If that holds, the pacer was never the shipped x86 *feel*
either, and "port it to the Pi for parity" was chasing a mechanism that the reference platform does not
actually present to users. See §5.4 for the verbatim `arroyo` evidence.

**The real vug work is §6.6** — the drawing-content arcs (`vugscene` / `vugtruth` / `vugspread` /
`launchvug`). That is where "WHERE IS THE UPGRADED VUG" is actually answered.

### 5.2 FOCUS_ASID release on self-exit

`2e473660` gave x86 a `wm::focus_release` call on the process-teardown funnel
(`memory::free_user_space_by_cr3`). **aarch64 had no twin.** Its funnel
(`arch/aarch64/syscall.rs::clear_handle_row`) already did the keyboard half (the `USER_INPUT_ACTIVE`
CAS) and reaped the dying ASID's rows (`win_close_asid` → `close_owner` → `close_compat`), but
`FOCUS_ASID` is `video::wm`'s own word and only `focus_release` may drop it — so nothing on the Pi did.

Consequence, and it is the ordinary case rather than an edge: a vug that **exits on its own** while
focused — `pulse` finishing, a vug returning from `main` — left the focus highlight naming a dead
ASID. Window ids being generation-less `slot + 1`, the slot's next tenant then came up holding a focus
it never earned, with the shell unable to take it back. (The close-*click* path releases before it
kills, so it was never the symptom.)

`focus_release` was already arch-neutral; only the call site was missing. Added after the row reap, so
its "loser repaints" sweep finds nothing and composites nothing on a teardown path — the same
placement x86 uses, with the **same** `route=self-exit …` token so one arch's capture stays readable
against the other's.

The tree already knew about this hole. `arch/aarch64/syscall.rs`'s DRAINSTALL (PA38) note, at the
`wc_close_click` line, says it outright: CLOSE-TEARDOWN's ruling — *"closing a window causes the other
open vug stat pulse windows to minimize"* — has a fix, `focus_release`, which **"has zero aarch64
callers."** This arc gives it its caller.

**This call is deliberately NOT behind `pidesk`,** and that is a considered exception to §5.3 rather
than an oversight. `wc_click_route` — and through it `wc_focus_key` / `wc_close_click`, the paths that
*write* `FOCUS_ASID` — are called unconditionally from the aarch64 input loop (`main.rs:3426`), with no
`pidesk` gate. Focus is therefore reachable on the Pi in the **shipped, knob-off media build**, so the
stranded-focus bug is live there and a `pidesk`-gated fix would leave it unfixed in the exact image
users flash. The consequence is stated plainly in §5.3: this arc **does** move the knob-off hash, on
purpose.

**Honest limit on the evidence: the regression suite does not exercise this fix.** Both gate captures
(`UNAOS_PIDESK=1 … 300` and knob-off `… 210`, each MBENCH PASS 108/108) contain plenty of `[wm-act]`
traffic — `park … cause=shell-raise`, `cause=minimise` — and `[wc-fv] focus raise` / `[vugmin] focus`
lines, but **no `route=self-exit` line**, because `focus_release` prints only when its CAS succeeds,
i.e. only when a dying ASID actually held focus. Nothing in the current cascade self-exits a *focused*
vug: the wm fixtures drive synthetic ASIDs (`0xf0a`/`0xf0b`) through explicit close paths, and the
teardown-heavy fixtures never take focus first. So the 108/108 proves this change **breaks nothing**;
it does not prove the bug is fixed. **A follow-up should add the missing fixture** — focus a real
ring-3 vug, let it return from `main`, then assert `[wm-act] … route=self-exit` and that the next
tenant of the slot comes up unfocused. That is the witness this row deserves and does not yet have.

### 5.3 Byte-identity, and the line-neutral rule every parity port will hit

The `pidesk` feature's standing promise is that with the knob OFF, none of the desktop-furniture code
is compiled and `kernel8.img` is byte-identical to baseline. Keep that promise for anything that is
*furniture*.

**This arc knowingly moves the knob-off hash**, because its one surviving change is not furniture: the
§5.2 `focus_release` call is a teardown-correctness fix on a path that runs in the shipped knob-off
image. The `pidesk` promise is about the *desktop feature* not leaking into the base image, not a
prohibition on ever fixing the base image. The right bar for a change like this is the one applied
here: keep it line-neutral, state the expected delta in advance, and prove the knob-off regression
suite still passes — not force it behind a knob where it would fix nothing users run.

**Line-neutrality is a separate constraint, and it is the trap in this campaign.** `cfg`-gating your
new code is *not* sufficient to hold byte-identity. `arroyo`'s PI-DESK block records the measurement: a `cfg`-only change to `wm.rs` still moved
the knob-off hash (`42355ca2…` → `1143ecc5…`), because panic `Location` records embed **file line
numbers** — eleven added *comment* lines shifted every location below them.

So: **a change to a file compiled on the knob-off Pi image must be LINE-NEUTRAL.** Fit new prose into
the line count already there; never add to it. The files that matter are the ones compiled knob-off —
`wm.rs`, `main.rs`, `arch/aarch64/*.rs`, `fbcon.rs`, `screen.rs`, `framebuffer.rs`. The four `pidesk`
furniture modules (`strip`/`dock`/`menubar`/`crystal`) are **not** compiled knob-off and are free of it.

Practical consequences, learned on this arc:

* **Put the rationale in this document, not in the source.** `docs/` is not compiled into `kernel8.img`,
  so prose here is free; a paragraph at the call site is not. Leave a one-line pointer (`See PARITY.md
  §x`) and write the argument here.
* Fold added statements onto the line they follow — `pump_usb_into_gui(); #[cfg(…)] pace_service();` is
  valid Rust and costs zero lines.
* Reclaim lines by rewrapping *adjacent existing* comments without dropping their content, rather than
  by deleting documentation.
* **Re-measure, never reason.** `arroyo` says so explicitly, and it is right: build knob-off and compare
  `sha256sum target/pi_baremetal/kernel8.img` against the pre-change image. Line-neutrality is
  necessary, not provably sufficient.

### 5.4 The pacing evidence — verbatim, and one thing it does *not* say

Peter's corroboration checks out, and it is stronger than expected: **the OS already ruled this exact
question once, and the ruling is quoted in-tree.** `crates/kernel/Cargo.toml:60-68`, defining
`vsyncpace`:

> ⚠ THE POLARITY IS INVERTED FROM GR22's, DELIBERATELY […] That argument decides a question the OS is
> not entitled to decide FOR the program. **P, GR25: *"fps is made up/forced to run a certain speed
> rather than running unrestricted as does the load across cores."*** A present is a ring-3 program
> asking the machine to do work; the machine's answer is how long the work takes, not a sleep chosen on
> the program's behalf. […] So: DEFAULT OFF.

And `arroyo:1581-1583`, on which check leg is the shipped one:

> ⚠ WHICH LEG IS THE SHIPPED ONE CHANGED WITH THE POLARITY. `x86-all` deliberately does NOT carry
> `vsyncpace` — **it is the shipped-desktop leg and the shipped desktop is now UNPACED**, so `x86-all`
> holds the pacer's DISARMED polarity under type-check.

Verified: `arroyo:1556`'s `x86-all` feature list carries `wc` and **omits** `vsyncpace`; the shipped
body is `#[cfg(not(all(feature = "wc", feature = "vsyncpace")))] fn present_pace(..) {}` — an empty
stub (`arch/x86_64/syscall.rs:4304`). GR25's ruling and Peter's 2026-08-13 ruling are the same ruling.

**But the evidence does not say what it first appears to, and the difference matters.** There are
**two** pacers, at different layers, and only one has a knob:

| | `vsyncpace` | WPACE-PANEL (this audit's §2 rows) |
|---|---|---|
| Layer | syscall — `arch/x86_64/syscall.rs` | compositor — `video/wm.rs` |
| Method | **sleeps** the presenting caller to the frame | **coalesces** the present, never sleeps |
| Gate | `all(wc, vsyncpace)` — knob, **default OFF** | `all(x86_64, wc)` — **no knob** |
| In the shipped x86 desktop? | **No** | **Yes** — `x86-all` carries `wc` |

So "the shipped x86 desktop is unpaced" is true of the *sleeping* pacer and **not** of the coalescing
one: WPACE-PANEL ships whenever `wc` does. The corroboration for Peter's ruling is therefore about
*intent* — the OS decided in GR25 that it is not entitled to choose a program's frame rate, and
WPACE-PANEL makes that same choice at a lower layer, by a different mechanism, without a knob to turn
it off. **That is a question for Peter about the x86 build, not something this arc decides**, and it is
flagged here rather than acted on. It does not change the Pi's disposition either way: `vsyncpace` is
stripped from aarch64 entirely (`arroyo:786`), everything it gates is x86-only, and **the Pi has never
been paced by either mechanism.** Leaving it that way is now the ruling.

---

## 5.5 The `exec-crystalpi` arc — the SHARD menu on the Pi (PA41)

Two metal defects on `hw-pi4@14e54538`, reported as one symptom: *"the crystal ignores clicks; the bar
came up incomplete — crystal missing — and filled in only when I moved the mouse."* They are **one root
cause wearing two faces: furniture state changes did not drive the pass that paints them, and the
furniture's damage test could not see the one writer that destroys it.**

### 5.5a `press=inert` was a stale WITNESS WORD, not a routing defect

`[menubar] … press=inert` is a hardcoded string from the arc when the bar had no press seam at all. By
PA41 the crystal *was* routed — `arch/aarch64/syscall.rs::wc_click_route` calls `strip::press_route`
ahead of every window arm, and `:: SHARD-MENU: crystal_press=open` on the same captures proves the arm
fired. An operator reading the two lines together could only conclude the routing was latched off.

The line now reads `press=crystal` (x86 trunk `122ed63e` made the identical edit for the identical
reason). **`FORBID \[menubar\] .* press=inert` is in `pi4-regression.spec`** so the retired word cannot
come back. The rule this cost a round to learn: *a witness term must track the code it describes, and a
term describing an absent feature must be retired the day the feature lands.*

### 5.5b MENU-DRIVE — an open menu must drive the pass that paints it

`crystal::compose` runs **only** from `strip::compose_all`, at the tail of `wm::composite_once`. Every
other state-changing gesture in the window system (a close, a drag, a zoom, a minimise, a dock raise)
runs a composite itself. `crystal::open`/`dismiss` did not: they flipped `OPEN`, printed, and returned.
On a quiet desktop nothing else composites — the render lane blocks on its channel, the backdrop's
timer flush carries no damage, `wm::service_damage` returns without compositing while no row is dirty —
so the menu opened *in state* and never reached the glass. On the Pi the pointer itself drives
composites, which is why the menu "appeared on click" for an operator whose hand was moving and never
appeared for one whose hand was still.

The fix is `122ed63e`'s: `open`/`dismiss` call `wm::composite()`, with one retry gated on `paint_owed()`
(the same `OPEN != slot-non-empty` test `compose` acts on), so the retry runs iff the first pass did not
land. **What was deliberately NOT ported is the x86 half.** `122ed63e` also threads a `menu_paint_owed()`
term through `wm::composite`'s re-run loop and lost-wakeup gate, because on x86 `composite()` can be
DECLINED into a concurrent `COMP_GATE` holder. On aarch64 `wm::composite()` **is** `composite_once()` —
no gate, no decline, no re-run loop — so that term would have no consumer on this arch, and adding it
would cost lines in `wm.rs`, which §5.3 forbids. **`wm.rs` is untouched by this arc.**

### 5.5c CLOBBER-REPAIR — the bar could not see the writer that destroys it

`menubar::Model::read` called `wm::dock_scan(&mut rows, (0,0,0,0))`: it took the caption and threw the
damage answer away. The bar's signature is a function of the model and the rect, so **a window that
paints over the bar changes nothing the bar's own damage test can see** — the pixels are destroyed and
`compose` returns `false` for the rest of the boot, until the focused caption or the clock minute
happens to change. That is the "came up INCOMPLETE, filled in slowly alongside cursor activity" reading
exactly, and it is why the crystal — the leftmost 16×22 of a 1920-wide strip — could be missing while
the strip around it looked fine.

On x86 this is latent because `wm::occ_clip` withholds the bar's columns from every window blit. **When
this was written, aarch64's `occ_clip` was `#[cfg(target_arch = "x86_64")]` and returned
`OccClip::none()`** — §6.2, then owed — so on the Pi nothing protected the strips *and* nothing repaired
them. The bar and the open
dropdown now both ask `dock_scan`'s clobber question against the rect they last painted and repaint on
a yes: `dock::compose`'s WCK5 condition, generalised to tenants #2 and #3. `clob=` is on both ledger
lines as the falsifier.

**This is a repair, not the protection.** A clobber repaint still costs one frame with a window's
pixels standing in the bar. Recorded here so a later session does not read `clob=` as evidence that
§6.2 has been closed. **`clob=0` on every QEMU surface run this arc**, at 640×480 and at bench
geometry: the repair is structural and its firing is metal-owed.

> **UPDATE — §6.2 LANDED (`exec-occ62`).** The protection is now in place on aarch64: the window
> half-space (M1) and the furniture strips (M2) are both in `occ_clip` on the Pi, so a window blit
> withholds the bar's and the dock's columns instead of publishing over them and being repaired a
> frame later. The repair above stays as the second line of defence it was designed to be, and
> `clob=` remains its falsifier — but a `clob=0` capture is now the expected reading rather than an
> unproven one.

### 5.5d BRINGUP-PAINT — read the paint back, do not infer it

`pidesk::activate`'s step 5 printed `menubar PAINTED` from its own control flow — the inference the same
function already refuses two steps above for `fbcon::console_is_routed`. `strip::paint` declines without
touching a pixel on a contended `SCRATCH` (the dock is tenant #1 and takes the same scratch in the same
pass) or on a surface that is not yet `word4`, and the bar is then left ENABLED — so
`screen::present_background` is already subtracting its rows from every desktop present — with nothing
on the glass and no damage condition able to notice. The seam now reads `menubar::owns_pixels()` back
and re-runs once; the line carries `owns_pixels=` and `retried=`.

### 5.5e The Pi's first furniture fixture

`crystal::selftest`, `dock::selftest` and `menubar::selftest` are all invoked from
`arch/x86_64/syscall.rs`. **No aarch64 boot had ever run one**, so the whole furniture family was
unwitnessed on the arch it had just been ported to. `crystal::routed_selftest` (`:: SHARD-PRESS:`) is
invoked from `video/pidesk.rs` — the only Pi point where the bar is known enabled and painted, and a
file that is *not* compiled knob-off, so §5.3's line-neutral rule does not reach it
(`arch/aarch64/syscall.rs` is compiled knob-off, which is why the call is not there). It drives
`strip::press_route`, the live shared router core both arch routers call, and asserts `painted=` — the
leg that reds without 5.5b. What it does **not** cover is per-arch and named rather than implied: the
button-mask edge detection, the press-target latch and the input rings all sit above that seam.

**Fault-injected, not assumed.** Commenting out `open`'s `wm::composite()` (and its retry) and re-running
`UNAOS_PIDESK=1 ./arroyo kernel8-test 210` printed exactly the defect PA41 saw, and the new rule caught
it:

```
:: SHARD-PRESS: menu=282x105+12+34 panel=640x480 routed=true(open) opened=true painted=false
                dismissed=true(dismiss) closed=true erased=true :: FAIL ::
❌ MBENCH FAIL — 108/108 required witnesses, 2 forbidden hit(s)
```

`routed=true(open) opened=true painted=false` is the whole diagnosis on one line: the router worked, the
state changed, and no pixel reached the panel. The pre-arc wire could not say the third thing.

### 5.5f Gate note — `UNAOS_PIDESK=1` at bench geometry reds, and did so before this arc

`UNAOS_PIDESK=1 UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` does **not** reach 108/108 on a
loaded host, and the baseline at `14e54538` reds identically (104/108, 31 forbidden hits) with the same
signature: `[wc-g] … slow=yes -> BLIT`/`RACE-*`, `[wc-h] -> AT-RISK`, `[wc-c] windows=2 drawn=1`,
`[wc-j] vacate -> FAIL`, `[dragperf] -> FAIL`. It is also **not deterministic** — two consecutive runs
of the same image gave 104/108 / 29 hits and 103/108 / 40 hits. Every one of those witnesses asserts an
exact pixel or a present duration against a 1920×1200 panel QEMU emulates six times slower than the
640×480 default, so this is a host-speed artefact of the surface, not a kernel verdict. **Compare
against a same-host baseline before reading any of it as a regression.**

---

## 6. OWED — class (b) gaps this arc did not take

Each is scoped to one line so the next session can pick it up without re-deriving it. **Claim a row
here before starting; strike it when it lands.**

| # | Gap | Sites | Scope |
|---|---|---|---|
| 6.1 | **Desktop backdrop layer (SHELLNOTDESK)** — **CLOSED**, REALDESK 2026-08-17 | `main.rs` (the mint arm), `ui_status.rs` ×3, `video/mod.rs` (the latch) | Landed in three steps. **PIDESK-CLEAR** `8f78399d` gave the Pi the panel-wide `DESKTOP_BG` clear. **SHELLWIN-PI** `b2e0fb4a` put the live text shell in a window and claimed the backdrop in the same pass, which retired the *shell* tenant. **REALDESK** retires the two that were left: `ui_status`'s pulse LED band and its host/ip/UTC status line, both of which wrote the desktop back buffer directly. See §6.1a for the full tenancy ledger and what the retirement does NOT cover. **METAL-PROVEN 2026-08-18** (PA50 boot: `PI-DESK: desktop armed` + `[realdesk]` on the wire) — after **U7STK** removed the u7-launch stack overflow that had made `arm()` unreachable on hardware; the whole saga is §6.1b. |
| 6.2 | ~~**Windows do not respect menubar / dock / open dropdown**~~ **LANDED — `exec-occ62`, see §6.10** | `wm.rs` (`occ_clip`, `OccClip::push`, `OccSnap`/`occluders_above`/`occ_excuse`, `dock_tiles`), `wcg.rs` (`Probe`, `begin`/`end`, `readback`, `OccNote`) | The window half-space and the furniture strips are both in the clip on aarch64 now, so a blit withholds the occluder's columns instead of publishing over them. Landed in two commits: **M1** the window term plus the read-back excuse that had to travel with it, **M2** the dock/bar/dropdown arm on the furniture family's own dual gate. `erase_clip` and `screen.rs:1050` are **NOT** covered — the deferred-erase side is still x86-only; see §6.10 for what remains. |
| 6.3 | **Quiet boot screen** | `fbcon.rs` ×9 (`QUIET-PANEL` `_print` suppression, `PANEL_MUTE_TAGS`, `TAG_SNIFF` + impls, `PANIC_MIRROR`) | x86 paints milestone lines only and mutes `[wc-g]/[wc-h]/[wc-d]/[wcn]` telemetry from the glass, with `PANIC_MIRROR` as the panic override; the Pi mirrors the raw serial stream across the boot panel. All arch-neutral policy over `_print`. Self-contained. |
| 6.4 | ~~**Dock tile count for the blit clip**~~ **LANDED — `exec-occ62` M2** | `wm.rs` `dock_tiles` | Moved to the furniture family's dual gate alongside its consumer, exactly as this row asked ("do it with 6.2"). The SHELLPIN residual `dock.rs` discloses travels with it and is **not** closed — see §6.10. |
| 6.5 | ~~**Fast glyph painting**~~ **LANDED — `exec-fontwire`, see §6.8** | `fbcon.rs:166` (`draw_glyph`'s span path) | x86 hoisted the pixel-format decode out of the 8×8 bit loop and poked pre-encoded words; the Pi ran `put_pixel` with a per-pixel `match` on `pixel_format`. Closed by making the hoist arch-neutral **and** run-coalesced, so it overshoots the parity the row asked for on both arches. `put_raw4` (`framebuffer.rs:268`) now has no callers. Accounting in §6.8. |
| 6.9 | **The console's FACE was an arch gate nobody had named** — *raised and closed by `exec-fontwire`, recorded here because the census missed it* | `fbcon.rs:258` (`FbCon::aa`), `fbcon.rs:1707` (`panel_console_resume`, `#[cfg(target_arch = "x86_64")]`) | Not in the 501-site census, because it is not an attribute: `aa` had exactly ONE writer and that writer was inside an x86-only function. The Pi therefore drew anti-aliased captions, bar, crystal and dock captions — all of which resolve through the arch-neutral `wm::TITLE_CELL_*` — around a console still painting a 1-bit 8×8 cell. **A parity census that greps `cfg` attributes cannot see a gap of this shape.** The general lesson is worth more than the fix: look for *single-writer state whose writer is arch-gated*, not just for arch-gated state. |

### 6.1a REALDESK — the backdrop tenancy ledger, and what the retirement does not cover

Peter, 2026-08-17: *"WHEN WILL THE REAL DESKTOP APPEAR?! SHELL IS STILL THERE. OLD EMBEDDED PULSE AND
INFO BAR"*. Three tenants were named; the ledger below is what was actually on the glass, measured off
an armed bench-geometry boot (`UNAOS_PIDESK=1 UNAOS_QUARRY=1 UNAOS_FBW=1920 UNAOS_FBH=1200`) rather
than derived. **Row heights below are the readings at base `6de03c87`, i.e. before FONT-PI
(`cb787847`) armed the Pi console's `noto16-aa` face.** Nothing in the mechanism depends on them —
`chrome_h` and `dock_reserve_h` are computed from `ui::Metrics` and the dock's own constants at
runtime, and FONT-PI moves neither `theme::BUTTON_HEIGHT` nor `theme::GAP` — but a re-read on the
merged tree may show a different `bottom_reserved=` if the console line pitch moved.

| Tenant | Rows on the 1920x1200 panel | Painter | Disposition |
|---|---|---|---|
| The live text shell | whole panel | `render_service`'s `console.draw(&mut pal)` | **Already retired** by SHELLWIN-PI `b2e0fb4a`. After the mint pass the shell draws only into its own window surface; the panel is claimed once by `pal.clear_screen(DESKTOP_BG)` and no code path in the service writes it again. Nothing was owed here — §6.1's "still owed" text predated that arc. |
| The pulse LED band | `(280, 1096, 1480x92)` — `[pstrip] armed panel=(…)` | `ui_status::draw` → `draw_panel_at(pal, None)` | **RETIRED.** The instrument is `video::pulsewin`'s window, through the *same renderer* (`draw_panel_at(pal, Some(rect))`) reading the same envelope. |
| The info bar | full width, `y = 1188..1200` | `ui_status::draw` (a `STRIP_BG` band + `hostname / ip / UTC` text) | **RETIRED.** See "what has no home" below. |
| The dock | centred, `y = 1136..1188` | `video::dock` via `strip::compose_all` | **STAYS.** Real desktop furniture and the console window's only way back. |
| The menu bar | flush top, `y = 0..34` | `video::menubar` | **STAYS.** Crystal + focused caption + UTC `HH:MM`. |
| WC-F ground-truth marks | bottom-left ramp 264x256, bottom-right twins 144x64 | `wcf`, straight to the framebuffer | **Not chrome — gate instrumentation.** `witness`-gated, and the metal image is built without `witness`, so no bench boot has ever shown them. |

**The two retired bands were an aarch64-only desktop.** `x86_render_service` calls
`screen.paint_desktop_scene()` and never reaches `ui_status` at all — x86 has neither band. That makes
them a ONE OS defect (*"arch gates in the experience layer are defects"*) independently of taste.

**They also collided with the dock.** `chrome_h(1200)` = 92 + 12 = 104 rows, and the dock is seated at
`ph - PAD - STRIP_H` = rows 1136..1188 — *inside* the instrument's own band. `present_background`
subtracts the dock's rect from every desktop present, so what an operator saw at the bench was a 40-px
sliver of LED rows, the dock standing in the middle of the rest of them, and a text line beneath it.

**The latch, and why it is runtime.** `video::desktop_scene_owns_backdrop()` (tail of `video/mod.rs`) is
armed by `main.rs`'s render service in the *same statement* as the backdrop hand-off, i.e. only on the
pass where `open_shell_window` actually returned a window. Where that declines, the shell legitimately
still owns the backdrop and the instrument stays — a desktop with no scene on it must keep its
readout. The three seams that read it are `ui_status::draw` (both bands), `ui_status::tick` (whose
sampler, envelope and every `[pstrip]` pacing number are untouched — only the panel present goes) and
`ui_status::chrome_h`.

**`chrome_h` shrinks; it does not go to zero.** The band's 104 rows would otherwise be handed straight
to `wm::place`, and on this arch `occ_clip` is structurally `OccClip::none` (§6.2), so a tiled window
would be blitted over the dock. The reservation therefore becomes `strip::PAD + dock::STRIP_H` = 64:
40 rows return to the work area and the furniture keeps its floor. `[realdesk] bottom_reserved=104->64`
is that number on the wire.

**What has no windowed home, stated rather than orphaned.** The menu bar carries UTC `HH:MM`; the
retired info bar also carried the **mDNS hostname**, the **settled lease IPv4** and the **date +
seconds**. All three remain reachable from the shell window (`netinfo`, `date`, `time`) and on the
serial wire, but there is no longer an always-on indicator for host/IP. A network status tenant — a
menu-bar item or a small window — is **OWED** and is the natural companion to §6.2.

**Spec, deliberately untouched.** `[realdesk] … == witness ::` is `pidesk`-gated, so it is absent from
the knob-off battery the standing gate runs; a REQUIRE for it belongs in the armed battery under the
same argument `[dragperf]` and the `[clickroute]` furniture legs already carry. `scripts/specs/` is
`exec-chromespec`'s lane this cycle, so the line is on the wire and the spec entry is left to that arc.

**A finding about the witness, not about the code.** `wcf::chrome_truth`'s desktop probe latches ~100
lines *before* the render service's first paint, so its `pt=desktop … -> HIT` has never been able to
see either band. It reports the state of the panel at bring-up, which is a real and useful reading, but
it is not evidence that the desktop is still clean at desktop-ready. Anything that wants that claim
needs a probe after the arming.

**Gates (base `6de03c87`).** `./arroyo check` and `UNAOS_WC=1 ./arroyo check` green, 12 legs each, no
new warnings. Knob-off `./arroyo kernel8-test 210` → **MBENCH PASS 111/111, 0 forbidden**, and the
knob-off `kernel8.img` is byte-identical to a reverted control (§5.3). The two armed geometries were
run as **back-to-back A/B pairs on the same loaded host**, patch reverted then re-applied, because
five sibling QEMUs were live and §2's `[wc-h]`/`[wc-g]` host-speed family is unreadable otherwise:

| Gate | Baseline (reverted) | This arc | REQUIRE delta |
|---|---|---|---|
| `UNAOS_PIDESK=1 UNAOS_QUARRY=1` (640x480) | 106/111, 19 forbidden | 106/111, 16 forbidden | **0** — the same five misses, verbatim |
| + `UNAOS_FBW=1920 UNAOS_FBH=1200` | 108/111, 120 forbidden | 108/111, 134 forbidden | **0** — the same three misses, verbatim |

Every failing fixture in both pairs prints *before* `[realdesk]` does (640x480: lines 219/934/949/1169
against the arm at 1510), so none of them can have executed a line of this arc. The forbidden spread
is `[wc-h] AT-RISK` alone — 284 raw AT-RISK lines on the baseline against 265 on this arc, i.e. the
arc's side is if anything the quieter one. A quiet-host re-run is still owed before any bench-geometry
number here is read as a verdict.

### 6.1b U7STK — the overflow that kept the desktop off metal, measured and fixed

**Status: the mechanism is CLOSED in the tree; the metal verdict belongs to the next boot.**

*(Integrator note: the FINDING half of this section was written by `exec-deskreal` `b01feaa1`, which
branched from the same base. If both arcs land, fold the two — this section carries the measurement
and the fix, that one carries the bench evidence that motivated them. Nothing outside the heading
overlaps.)*

**THE FINDING (exec-deskreal `b01feaa1`, off three metal boots + the PA44 capture
`~/unaos-bench/capture/pi4-pi1-b1/ttyACM0.log`).** `video::pidesk::arm()` — the only writer of
`ARMED`, and hence the gate on the shell-window mint, on `pal.clear_screen(DESKTOP_BG)` and on
`retire_desktop_chrome` — has never executed on Pi hardware. It sits one statement after
`wcb_launcher` inside `u7_launcher`, on the `u7-launch` task, and on metal the task is killed
between the two statements:

```
:: EL0: window verbs — … :: PASS ::
[spin6] cpu=2 REFUSING corrupt switch-in: task=70:u7-launch ctx_sp=0x20c9e70
outside its stack [0x20ca000,0x20ce000) — the parked frame was OVERWRITTEN
(neighboring stack overflow?). Task dropped; core keeps dispatching
```

`ctx_sp` lands 144–928 bytes *below* the task's own 16 KiB low bound, varying per boot — the
launcher's own frame chain, not a neighbour. Everything after that statement (the desktop, BGRUN,
FATDIRS, FATMOVE) is dead on metal.

**WHAT WAS MISSING WAS A NUMBER.** SPIN-6 can say a frame ended up outside its stack; nothing in the
tree could say how deep the chain actually goes, so any fix would have been a guess. This arc adds
the instrument first and reads it.

**THE INSTRUMENT (`[u7stk]`).** `sched::spawn_inner` paints every fresh kernel stack with
`STACK_POISON` (0xAB); `sched::stk_probe` reads the live SP and scans up from the stack's low end
for the first byte the task has ever touched. The reading is a **high-water over the task's whole
life**, which is what lets one probe placed *after* a call report how deep that call went — so
forty-seven checkpoints through `u7_launcher` convict one launcher without instrumenting any of
their interiors. Both halves are `witness`-gated, so they ride the `kernel8-test` battery and the
metal witness image unconditionally (an instrument you must re-arm is one that will not be armed on
the boot that matters), while a plain `./arroyo kernel8` media build carries neither.

**THE MEASUREMENT (QEMU raspi4b, armed, 16 KiB stack, before the fix):**

```
[u7stk] at=entry            task=69:u7-launch len=16384 used=5952 hw=5952  headroom=10432
[u7stk] at=after:k1_persist task=69:u7-launch len=16384 used=5952 hw=13064 headroom=3320
[u7stk] at=after:exec1      task=69:u7-launch len=16384 used=5952 hw=14984 headroom=1400
[u7stk] at=after:wcb        task=69:u7-launch len=16384 used=5952 hw=16384 headroom=0
```

Two facts, and the first is the convict.

1. **`u7_launcher`'s own frame is 5952 bytes** — 36% of the task's entire stack, spent before it has
   called anything. Statically the prologue is `stp x29,x30,[sp,#-0x60]! ; sub sp,sp,#0x1,lsl #12 ;
   sub sp,sp,#0x5e0` = **5696 bytes**, the second-largest stack frame in the whole kernel image
   (only `video::wm::composite` at 6640 exceeds it). The function declares no locals of its own:
   **38 of its ~47 callees live in the same module and were inlined into it**, so all of their local
   buffers are slots in one frame — a frame that stays live for the entire cascade, including while
   a non-inlined callee runs twelve kilobytes deep beneath it.
2. **`headroom` reaches 0 at `after:wcb`** — every byte of the 16 KiB touched. QEMU survives by
   exactly zero bytes. Metal does not, and the reason is visible in the same disassembly: the metal
   storage path adds `xhci::bot_transfer → bot_rescue_escalate → run_bot_stage → … →
   fbcon::__print → wm::composite` under the same chain, a subtree measuring **5520 bytes on its
   own**, and QEMU raspi4b models no xHCI so it never takes it. That is the whole "QEMU-green,
   metal-dead" asymmetry of this defect, stated in bytes.

**THE FIX, PART 1 — the convicted frame.** `#[inline(never)]` on exactly those 38 launchers (all in
`arch/aarch64/syscall.rs`; each carries a one-line note). Their locals return to their own frames,
live only while that launcher runs, so the peak becomes `u7_launcher`'s residual frame plus the
deepest *single* launcher rather than the sum of all of them. The cost is one `bl` per fixture on a
path that already does disk I/O. It is **unconditional, not witness-gated**, because the defect is
not: a knob-off media image overflows exactly as readily as a witness one — so knob-off codegen
moves with this arc, deliberately, and the battery is what proves the move behaviour-neutral. A
blanket `TASK_STACK_SIZE` increase was rejected as the *primary* fix: it would charge every kernel
task in the system for one task's frame, and it would leave the 5.7 KiB frame in place to be
re-discovered later. Measured effect:

| | before | after |
|---|---|---|
| `u7_launcher` own frame (static) | 5696 B | **16 B** |
| `u7_launcher` own frame (`at=entry`, measured) | 5952 B | **272 B** |
| peak high-water (QEMU, whole cascade) | 16384 B — `headroom=0` | **12296 B — `headroom=4088`** |
| static worst case over the reachable call graph | 26880 B | **14240 B** |

**THE FIX, PART 2 — the right-sized stack, and why it is still needed.** Part 1 leaves the static
worst case at 14240 B inside a 16384 B stack: a 2144-byte squeeze. That is not enough, and the
reason is the same asymmetry that hid the defect for three arcs — the metal-only
`xhci → fbcon::__print → wm::composite` subtree is 5520 B, and 12296 + 5520 = 17816 > 16384. Sizing
the fix to the QEMU number would be repeating the original mistake with a smaller margin. So
`u7-launch` alone is spawned through a new `sched::spawn_stack` with
`U7_LAUNCH_STACK_SIZE = 32 KiB` (`main.rs`, the constant carrying the four measurements above in
its comment) — 1.84× the static worst case, 2.66× the QEMU peak, at a cost of 16 KiB of heap for
one task. `TASK_STACK_SIZE` is untouched, so every other kernel task is unaffected.

**THE GATE HOLE, CLOSED.** The SPIN-6 refusal carries no `FAIL`, so no default FORBID caught it, and
every witness the dropped task would have printed is absence-shaped, which the mbench grammar cannot
convict. Three Pi arcs therefore gated green on captures in which a kernel task had ceased to exist.
`scripts/specs/pi4-regression.spec` now carries `FORBID REFUSING corrupt switch-in: task=` — written
bracket-free because a literal `[spin6]` is a character class matching one of `6insp`, and because
the distinctive half of the message needs no bracket at all. It is deliberately **not** scoped to
`u7-launch`: losing any kernel task to a corrupt parked frame is the same defect wearing a different
name. It adds no REQUIRE, so the witness floor is unchanged at 117.

Proven able to fire, per this repo's go-red discipline, against the arc's own green capture:

```
control  (unmodified)                 ✅ MBENCH PASS — 117/117, 0 forbidden hit(s)   rc=0
+ the verbatim metal line spliced in  ❌ MBENCH FAIL — 117/117, 1 forbidden hit(s)   rc=1
                                         FORBID hit @ line 693: [spin6] cpu=2 REFUSING corrupt
                                         switch-in: task=70:u7-launch ctx_sp=0x20c9e70 …
```

One line is the entire difference between the two runs, and it is the difference between pass and
fail — which is exactly the property the old spec did not have.

**⚠ FLAGGED FOR THE INTEGRATOR — this arc trips a `[wc-h]` FORBID it does not regress.** The
knob-off battery is clean (117/117, 0 forbidden). The **armed** battery
(`UNAOS_PIDESK=1 UNAOS_FBW=1920 UNAOS_FBH=1200`) was already red before this arc and is still red
after, but the shape changed, and the reason is worth writing down rather than leaving the next
reader to rediscover it:

| capture | `[wc-h] rollup` lines | raw `-> AT-RISK` | matches `presspread=[0-9] .*-> AT-RISK` |
|---|---|---|---|
| pre-fix (this arc's instrument, 16 KiB stack) | 292 | 282 | **7** |
| post-fix run 1 | 292 | 280 | **109** |
| post-fix run 2 | 292 | 199 | **117** |

The FORBID's `presspread=[0-9] ` carries a trailing space, so it convicts the **single-digit** spread
class only — deliberately, per the WCH-SPREAD block above: `presspread=0` means "no present recorded
at all", and the single-digit class is convicted alongside it so the unmeasured case fails safe.
*(Superseded in part by WCHFIX, 2026-08-18: the pattern is now
`presspread=[0-9] presspop=([2-9]|[0-9]{2,}) .*-> AT-RISK`, and the `presspread=0` fail-safe clause is
retired — a spread with fewer than two points is not a spread. See engine.md §WCHFIX. The counts in
the table above are the pre-WCHFIX pattern's and are left as recorded.)* The
rollup count is identical (292) and the raw AT-RISK rate is flat-to-better (282 → 280 → 199). What
moved is the *distribution*: `presspread` on AT-RISK lines went from mostly 10/29/34/45 before to
mostly 3/6/11/12 after. A **lower** spread is a more consistent compositor — and it is precisely that
improvement which drops those lines into the class this FORBID convicts.

So the arc's effect is neutral-to-positive on every raw counter, and the 100-hit jump is a bucket
change rather than a regression. Reproduced twice, on a host at load ~12 with 5–7 concurrent QEMU
instances, so it is not a one-off. **Not fixed here, deliberately:** `wc-h` is `wm.rs`'s lane and
belongs to another executor, and the only way to green it from this side would be to widen the
FORBID — the one thing this spec says a witness must never do. Left for whoever owns WCH-SPREAD,
with the raw counters above as the evidence that the underlying signal did not move.

**WHAT THIS ARC DOES NOT CLAIM.** Whether the desktop finally arms on hardware is the next bench
boot's verdict, not this one's. What is established here is that the arming statement is now reached
with headroom to spare in QEMU, that the reason it was not is measured rather than inferred, and
that a future boot which loses a kernel task mid-cascade reds the gate instead of passing quietly.

### 6.6 THE VUG UPGRADE — the real answer to "where is the upgraded vug"

**This is the replacement for the pacer work, and it is where Peter's direction actually points.** The
trunk's vug arcs are surveyed below. The finding that matters: **the shard renderer is arch-neutral and
already links for aarch64 (7731 bytes of `.text`) — the Pi is not missing the drawing code. It is
missing the ways to REACH it, and the headroom to run it at full detail.**

| Arc | What it added | Arch status | Disposition |
|---|---|---|---|
| **VUGSCENE** `a5bd93ee` | The real drawing-complexity arc. Replaces the trivial wireframe with a solid faceted **SHARD** — an 18-half-space convex-intersection ray tracer (exact HSR, no z-buffer), orbiting light, per-facet flat shading with a specular kick, palette read from the menu bar's crystal. Plus a **ray-density LOD ladder** (lvl 0 wireframe → 3 = one ray per pixel, cost 1:4:16) that self-tunes off its own fps meter. Commit body: *"Peter: make the drawing complex, not the pacing (999fps was a trivial pattern), and the scene IS the crystal."* | **Arch-neutral.** `crates/user-vug/src/main.rs` carries no `#[cfg(target_arch)]` outside the syscall stubs; it compiles and links for aarch64. | Renderer needs no port. **The reach does** — 6.6a/6.6b/6.6c below. |
| **VUGTRUTH** `51b66a2c` | Failed presents no longer count as frames or clock the exit budget; fps on the wire as `[vugfps]`; spin budget 4096→64. The *measurement* the VUGSCENE ladder later reads. | **Arch-neutral**, gates were run green on both arches. | **AT PARITY. Not a gap.** Added no rendered content. Recorded so it is not re-opened. |
| **VUGSPREAD** `b30d81f3` | Scheduler repair for vug's 19 fps: unstealable spawned threads, load-blind sibling pick, a steal floor that could not see 2-on-1 packing. Adds per-task `migrations`, a placement hint, per-victim `steal_floor`, `cr3_live` shadow, `[spread]` witness, and an escalating steal cooldown. Renders no pixels. | **x86-only by directory** — every line was in `arch/x86_64/sched.rs` (~50 `VUGSPREAD` tags). | **LANDED — §6.7.** The policy is now `crates/kernel/src/sched_spread.rs`, shared; the Pi has the steal half. |
| **LAUNCHVUG** `fa439109` | Bare `vug` at the shell resolves `VUG.ELF` on the volume executables live on and launches it **detached**. Fixed Peter's bench report *"still no way to launch vug"*. | ~~x86-only, explicitly~~ — **PORTED**, `exec-barename`. `bare_exec` is now one shared body over one per-arch re-resolve, and `Facts::exec` stands on `proc_verbs` rather than on an arch. | **CLOSED — 6.6a, see §6.6a-closed.** |

| # | Gap | Sites | Scope |
|---|---|---|---|
| 6.6a | ~~**`vug` does not launch on the Pi**~~ — **CLOSED**, `exec-barename`, see §6.6a-closed | `shell.rs` (`bare_exec` + `bare_exec_reresolve` + `exec_resolve`/`exec_canon`/`EXEC_ROOT`, `Facts::exec`, `adopt_bg_job`, the `Plan::Exec` arm); `libs/sys/midden_core/src/lib.rs` (`Facts::vugdemo`, `Avail::VugDemo`, `help`) | The operator had to type `bg /fat/VUG.ELF`. **This is also the gate on the shard itself:** VUGSCENE renders only when `overlay = detached \|\| interactive`, and `detached` is bit 0 of the info-page flags set by the detached spawn — so the launch path *is* the drawing path. `spawn_user_image_bg` already existed on aarch64 (`arch/aarch64/syscall.rs:8211`); the resolver/dispatch half is now ported, and the `not(x86_64)` arm did its job — the compiler pointed at it. **This row's own symptom claim was wrong, and the correction is the second half of the fix — see §6.6a-closed.** |
| 6.6b | ~~**Pi media stages only one vug image**~~ — **CLOSED**, `exec-vugstage`, see §6.6b-closed | `arroyo`: `build_one_vug_aarch64` + `build_user_vug_aarch64` (new — the twin of `build_one_vug_x86`), called from `kernel8` where the single inline `VUG.ELF` recipe used to sit; the FAT staging block copies all three into `$KERNEL8_DIR` | x86 media carries `VUG.ELF` (adaptive), `VUGC.ELF` (`pinlo`, classic baseline) and `VUGX.ELF` (`pinhi`, full per-pixel shard). **Pi media had neither `VUGC.ELF` nor `VUGX.ELF`**, so the pinned-detail images — the ones that show the shard at full density and give the A/B baseline — could not be run on the Pi at all. Build-plumbing only, and the row's own claim held: the crate links for aarch64 under both pins with no source change. |
| 6.6c | ~~**No scheduler placement/steal repair on the Pi**~~ **LANDED — `exec-vugspread`, see §6.7** | `sched_spread.rs` (new, shared); `arch/aarch64/sched.rs`; `arch/x86_64/sched.rs` (delegation only) | The LOD ladder self-tunes off achieved fps, so **less CPU headroom settles the shard at a LOWER detail rung — the Pi literally draws a simpler scene.** The row was written as "placement AND steal"; §6.7 shows the Pi already had a deeper PLACEMENT lattice than x86 (SPREAD-3..14), and what it lacked was the STEAL half — the only correction that can reach a thread which never parks. That half is now ported. |
| 6.6d | ~~**No damage-present on the Pi**~~ — **CLOSED**, `exec-presentrows` | `SYS_WIN_PRESENT_ROWS` (33) dispatched at `arch/x86_64/syscall.rs:2594`; now also at `arch/aarch64/syscall.rs` (`sys_win_present_rows`, beside `sys_win_present`) | The Pi answered `-ENOSYS` and `user-vug` fell back to a whole-box present. Ported: same number, same argument shape, same errnos in the same order, band range-checked against the presenting row's own `h` under the hold that proved ownership. See §6.6d-closed below for the correction the port forced on this row's own framing. |

#### 6.6a-closed — the bare name launches on the Pi, and the phantom verb that was really in the way

`exec-barename` ports BARE-NAME LAUNCH to aarch64. Peter's order (PA44, 2026-08-18): typing `vug`
must start it, retiring `bg` as a *requirement* on the Pi. `bg` stays — it is the explicit form and
x86 has it too; what changed is that the bare name works.

**The row above said typing `vug` on the Pi printed "Unknown command." It does not, and never did.**
The QEMU repro on the pre-arc image is unambiguous:

```
:: [midden] cmd="vug" -> Host verb=vug ::
```

`vug` was claimed as a **VERB**. `midden_core`'s table carried `("vug", Avail::Aarch64)` and
`("pulse", Avail::Aarch64)`, but the kernel `match` arms that service them carry
`#[cfg(all(target_arch = "aarch64", feature = "vugdemo"))]` — and `vugdemo` is **DEFAULT OFF**
(DECRUD-1). So on the image users flash, the core advertised two verbs the build does not carry,
routed them to a `match` with no such arm, and answered out of `other =>`. That is a defect on its
own; but verb-ness is **absolute** over bare-name launch (`plan` consults `resolve_exec` only after
`is_verb` says no), so the phantom verb was ALSO what stood between the operator and `VUG.ELF`.

**Turning on `Facts::exec` alone would have changed nothing for the one word Peter typed.** The two
halves of this arc are therefore inseparable, and the second is not scope creep — it is the fix:

1. **`Facts::exec = proc_verbs`.** Bare-name launch exists exactly where `spawn_user_image_bg` and
   the shell job table exist, which is what `proc_verbs` already means. One fact, no second arch
   gate — the ONE-OS law forbids one here.
2. **`Avail::VugDemo`** (`facts.aarch64 && facts.vugdemo`), with `vugdemo: cfg!(feature = "vugdemo")`
   at the kernel's single facts call site. The table's standing contract is that each entry's
   condition mirrors the `#[cfg]` on its match arm *one for one*; this restores that. Knob-off, `vug`
   and `pulse` are not verbs, and `help` stops listing them (they are named on the bare-name lines
   instead). Knob-on, both are verbs again and beat the program, unchanged.
3. **`help`'s bare-name lines key off `facts.exec`, not `facts.x86`.** A help line that names a
   capability must be gated on the capability.

**Resolution — the search path, and why it is x86's order rather than a new policy.** x86 has no VFS:
its whole path universe *is* the program-source FAT, so "resolve from the cwd" and "resolve on the
volume executables live on" are one sentence there, and `/fat` is carried only as an alias for that
volume's root. On the Pi the two come apart — `/` is native UnaFS and the images are on the SD FAT at
`/fat`. `exec_resolve` is that same sentence with the halves separated, in order:

1. **cwd-relative, through `vfs_path`** — the VFS-1 seam `ls`/`cat`/`run`/`bg`/`vfs` share, so a bare
   name means exactly what those verbs say it means. Not a private path scheme.
2. **the program-source root, `EXEC_ROOT = "/fat"`** — relative tokens only, skipped when it would
   repeat probe 1. On x86 this step is not absent, it is *implied* by the cwd already sitting on the
   program source. Dropping it would leave the operator at `/` unable to type `vug`, i.e. the whole
   defect intact.

Case handling matches x86 for the same underlying reason: the FAT backend behind `/fat` matches
components case-insensitively, so arm 2 of the core's resolver (`vug` → `vug.elf`) hits the on-disk
`VUG.ELF` and the upper-cased arm 3 stays latent on both arches. `canon` — the on-disk spelling
`jobs` and the witness lines quote — is read from a FAT directory entry on x86; VFS `stat` returns no
name, so `exec_canon` recovers it from the parent listing case-insensitively, falling back to the
resolved path (a display name is never worth a refusal). No `EXEC_BIND` stamp on this arch: that
instrument and its `fatverb_storage_witness` reader compare FAT *handles*, and this arch binds a
mount table.

**Shape of the port.** `bare_exec` is now ONE body under
`any(all(baremetal, aarch64), x86_64)` — the gate `BgJob`/`BG_JOBS`/`bg_program`/`adopt_bg_job`
already carry — with the single per-arch step split out as `bare_exec_reresolve`, returning
`(load_path, canon)`. x86's `load_path` is the token the core returned, untouched, so **every x86
panel and serial line is byte-identical**; only aarch64 needs the absolute path, because there the
token alone is ambiguous between the native root and the program source. Everything after the
re-resolve — the ELF64-magic refusal, `spawn_user_image_bg`, `adopt_bg_job` (the BGRUN-1 ledger `bg`
writes), the launch line, the job-table-full kill — is shared source, so the two arches cannot drift.

**Byte-identity, stated honestly.** This arc **moves the knob-off `kernel8.img` hash**, deliberately
and on the §5.3 grounds: it is not desktop furniture, it is a capability change on the shipped image,
and it is the change Peter ordered. No attempt at line-neutrality was made or claimed.

**Proof (QEMU raspi4b, knob-off media).** `arroyo kernel8-test` uses `-serial file:` — write-only —
so the suite types nothing and cannot witness this. The repro therefore runs the same image under a
**bidirectional** first UART (`-chardev socket` + `-serial chardev:`) and types at the shell for
real. Post-arc, from `/`:

```
:: [midden] cmd="vug" -> Exec vug.elf ::
:: BAREXEC: /fat/VUG.ELF (typed 'vug') — loaded 12568 bytes, entry 0x400000, pid=30 slot=1 DETACHED, left RUNNING ::
:: [midden] cmd="nosuchprogram" -> TerminalError len=44 ::
:: BGRUN: jobs — 1 tracked job(s) after the sweep ::
```

and, after `cd /fat` (the cwd leg, and the two honest-failure legs):

```
:: BAREXEC: /fat/VUGX.ELF (typed 'vugx') — loaded 12568 bytes, entry 0x600000, pid=148 slot=1 DETACHED, left RUNNING ::
:: BAREXEC: /fat/HELLO.BIN (typed 'hello.bin') — REFUSED: not an ELF64 image (no magic); 51 bytes read ::
:: [midden] cmd="stat" -> Host verb=stat ::
```

i.e. the bare name launches from either root, `jobs` tracks it, a genuinely unknown name still gets
the unknown-command refusal (the negative control), a non-ELF file is refused rather than fed to the
flat loader, and a real verb still beats a program of the same name. **No spec line moved**: nothing
in `pi4-regression.spec` anchors on "Unknown command" (the string reaches the console only, never
serial), and knob-off `kernel8-test 210` is 117/117, 0 forbidden.

**Not done here** — ~~The Pi's regression suite still cannot type, so none of the above is *gated* — it
is a reproducible manual witness, not a REQUIRE. `mbench.py` already has the `--inject`/`--follow`
machinery (metal-bridge only today); wiring a bidirectional chardev into `test_kernel8` and adding a
BARENAME script would turn this proof into a standing gate, and is the obvious follow-up.~~
**DONE — `exec-suitetype`, see §6.6a-typed.**

#### 6.6a-typed — the suite can type, and the bare name is now a REQUIRE

`exec-suitetype` closes the hole §6.6a-closed named. `test_kernel8` gains an OPTIONAL typed-input
mode, **default off**: `UNAOS_K8_SCRIPT=<file>` swaps UART0's write-only `-serial file:` for
`-chardev socket,…,logfile=<same log>` + `-serial chardev:` — bidirectional, and still writing
*exactly* the capture mbench replays, because QEMU's own chardev `logfile=` does the writing.
Nothing downstream of the chardev can tell the difference, and knob-off the qemu argv is
byte-for-byte the argv it has always been. `scripts/k8_type.py` is the typist; it reads
`mbench.py`'s **existing** inject-script grammar (`SLEEP` / `WAIT <secs> <regex>` / a line to type),
so `scripts/specs/pi4-barename.inject` drives QEMU here and the metal bridge via `mbench --inject`
without a second dialect. Harness and spec only — no kernel source was touched.

**The three witnesses are in a SECOND spec** (`scripts/specs/pi4-barename.spec`), not in
`pi4-regression.spec`. A REQUIRE for a typed line cannot be satisfied by a run that types nothing,
so putting it in the base spec would red the classic gate for behaving correctly — the same argument
the `pidesk`-gated `[dragperf]`/`[dragwedge]` families make there, taken one step further: those had
to settle for a FORBID because there was one spec and one battery, whereas a second spec asserted
*additionally* and only when the knob is armed keeps the base count fixed and still gets real
REQUIREs. **The suite floor**: knob off, **117/117** required / 0 forbidden, unchanged; knob on,
**117/117 and then 3/3** — 120 required witnesses across the two batteries. The base spec runs first
and owns the COMPLETE markers, so it alone can say TRUNCATED, and a non-PASS base verdict
short-circuits: the typed spec is only ever consulted on a capture already called complete and clean.

**The negative control the base capture supplies for free**: in the knob-off 26359-line capture,
`BAREXEC`, `tracked job(s) after the sweep` and `nosuchprogram` appear **zero** times. All three
REQUIREs are therefore typing-only by measurement, not by assumption.

**Readiness was re-anchored, and the old anchor was wrong.** The §6.6a-closed repro waited on
`[click2] depth`; measured on this arc's knob-off baseline, that line first appears at **line 210** —
before the fixture cascade starts — and accounts for **25529 of 26359** lines. It proves the input
pump exists and nothing about the boot being finished, so typing 12 s after it means typing *into*
the cascade. The script now waits on `:: BANDY-ACL:`, the base spec's LAST REQUIRED witness (line
1432 of that capture; everything after is steady-state `[sched6]`/`[prio]`/`[pstrip]` noise), then
settles 12 s. Measured on the gate run (host load average 18.9 at launch): readiness at **t=17.0 s**,
`vug` typed at t=29.1 s, `nosuchprogram` at t=43.3 s, `jobs` at t=58.0 s — the script is done inside
~70 s, so the typed window's cost over the classic 210 is margin rather than need. Gate at 300.

**Gate result** (`UNAOS_K8_SCRIPT=scripts/specs/pi4-barename.inject ./arroyo kernel8-test 300`):
base `117/117 required, 0 forbidden, 43891 lines`, then typed `3/3 required, 0 forbidden`. The
three lines are the same three §6.6a-closed quotes, now produced by the suite rather than by hand:

```
:: BAREXEC: /fat/VUG.ELF (typed 'vug') — loaded 12568 bytes, entry 0x600000, pid=152 slot=1 DETACHED, left RUNNING ::
:: [midden] cmd="nosuchprogram" -> TerminalError len=44 ::
:: BGRUN: jobs — 1 tracked job(s) after the sweep ::
```

Classic knob-off `kernel8-test 210` was run before and after the change on the same tree: `117/117,
0 forbidden` both times, and the only stdout difference between the two consoles is the firmware
fetch the first run had to do and the second took from cache. The serial captures differ by 1074
lines, of which 1072 are the free-running `[click2] depth` flood; the 480 distinct line SHAPES are
identical modulo run-to-run counter values.

#### 6.6b-closed — three images on the Pi FAT, and the falsifier that proves the pin travelled

`exec-vugstage` mirrored the x86 three-image build onto aarch64. `arroyo` gained
`build_one_vug_aarch64 <out> <feats>` — the same shape as `build_one_vug_x86`: one cargo build per
feature set, each in its own `--target-dir` (`crates/user-vug/target/{arm,arm-pinlo,arm-pinhi}`, because
the feature set is part of cargo's fingerprint and a shared dir would relink the crate on every call),
then `llvm-objcopy --strip-all` into `target/<out>`. Four assertions per image, one of them new on this
arch: **e_machine must read `b700` (EM_AARCH64 = 183 LE)**, the aarch64 form of the guard the x86 twin
already carried — both arches' images share one `target/` directory, so a mixed-up basename is exactly
the mistake that would otherwise surface as a loader refusal at the bench rather than a build failure.

The staged result, from `mdir` on a freshly built `UnaOS-pi4-baremetal.img`:

| FAT name | pin | bytes | sha256 (first 24) |
|---|---|---|---|
| `VUG.ELF` | *(none)* — adaptive ladder | 12568 | `8595824d2dca2fd454f04d3c` |
| `VUGC.ELF` | `pinlo` — level 0, classic wireframe | 12568 | `3da5602410c46bde94850a2a` |
| `VUGX.ELF` | `pinhi` — `LOD_MAX`, full per-pixel shard | 12568 | `fe271640abc079af52bbe73c` |

**Equal sizes are not equal images**, and the three hashes are the first half of saying so. The second
half is a falsifier that reads the pin rather than the bytes: `LOD_PIN` is `Some(_)` under either
feature, so `LOD_PIN.is_none()` is a compile-time-foldable false and the `[vuglod] lvl=` ladder trace
becomes dead code. `strings` finds that literal in `VUG.ELF` and in **neither** pinned image — the pin
travelled, and the equal sizes are just the 4 KiB segment padding `-z max-page-size=0x1000` imposes.
Cost to the media: 25 KiB of the 64 MiB image, 0.04%.

**No kernel change was needed for reach**, which is what makes this row build-plumbing only. Both launch
paths take any `.ELF` on the volume by name: `bg /fat/VUGX.ELF` at the shell, and Quarry's double-click
(`video/quarry/live.rs` treats `.ELF`/`.BIN` as launchable and hands the image to the same
`spawn_user_image_bg` the `bg` verb calls). Putting the bytes in the FAT root *is* the port. The UVUG
witness still reads `VUG.ELF` and is unchanged — knob-off `kernel8-test` stayed at 117/117.

#### 6.6d-closed — what the port delivers, and the claim in the row above that was wrong

`exec-presentrows` landed `SYS_WIN_PRESENT_ROWS` on aarch64 (`arch/aarch64/syscall.rs`), plus five
banded-present bits in the `el0-wcb` fixture (b13..b17: the happy path and its four refusals) and the
matching widening of `pi4-regression.spec`'s ledger pin from `0x1fff` to `0x3ffff`. The gate proves the
verb reaches the compositor rather than degrading: `[wc-h] rollup win=1 … banded=1 minspan=22
minspan_bytes=23408` — 22 panel rows repainted for an 11-row source band, against a whole box of the
same window.

**The correction.** The row above said *"`user-vug` falls back to a whole-box present every frame"*, and
the second half of that is not true — on EITHER arch. `user-vug` calls the banded verb from exactly one
site (`main.rs:2350`, the idle HUD refresh, `[HUD_Y0, HUD_Y1)` = 11 of 128 source rows, at most once per
second). Its per-frame render present is `SYS_WIN_PRESENT` at `main.rs:2535`, deliberately and with the
reason on the line: the two workers between them write every row, so the damaged band IS the whole
surface and there is nothing to narrow. x86 does the same thing. **So this gap was never the per-frame
cost, and closing it does not by itself move the Pi's frame rate.** What it removes is a real but small
waste — a whole 128-row box repainted once a second for 11 changed rows on every idled vug on the
desktop — and, more importantly, it removes the ARCH ASYMMETRY: a ring-3 program that bands its damage
is now answered the same way on both chips, so the next client to arrive (a shell window, a console
window, a partially-updating app) gets the cheap path on the Pi without another port.

The stutter Peter reported at the bench remains owned by **6.6c** (no VUGSPREAD placement/steal repair
on the Pi) and, for reach, **6.6a**. Neither is touched here.

**Owed elsewhere, not touched by this arc (lane discipline).** Two comment blocks in `video/wm.rs` now
carry a premise this arc falsified — that no caller of `present_rows` compiles for aarch64, and that the
`[comp2]` ledger's `dmg_px` and `box_px` are therefore equal on that arch by construction (`wm.rs`
~8564 and ~13902). The CODE is unaffected — it charges the extent that actually ran, which is what those
blocks argue for — but the "cannot move" claim is now false on a boot that takes a banded present.
`una-abi`'s verb table (`lib.rs:131`) and `user-vug`'s import comment (`main.rs:297`) likewise still read
*x86 only*. All four are doc-only and belong to lanes this arc does not own.

**Not gaps, recorded so they are not mistaken for them:** the `overlay` gate that renders level 0 on a
foreground `run` behaves identically on x86 — it is shared ring-3 behaviour, not a parity defect (the
Pi felt it more only because 6.6a denied it the easy bare-name path, until `exec-barename`).
`user-pulse` is fully at parity: its
drawing is common code and `arroyo:2885` already stages `PULSE.ELF` on Pi media.

#### 6.6e — the `exec-crystalhd` arc: CRYSTAL-HD/AA land on both chips, CRYSTAL-PACE does not

The held `crystal-graphics-hold` commit (`e65d5d9b`) was three coupled halves. Two landed on both
arches; the third is refused by a ruling already in this document.

**CRYSTAL-HD + CRYSTAL-AA — landed, arch-neutral.** The window surface cap rises 128 → 288 in BOTH
`arch/x86_64/memory.rs` and `arch/aarch64/boot.rs` (Peter's sign-off, 2026-08-18, verbatim option
"1": `FB_WIN_MAX` 128 → 288, window slots 8 → 4, +15 MiB `.bss`), and `user-vug` renders a 288×288
shard on both chips with antialiased facet edges and silhouette at the top LOD rung. The held commit
kept aarch64 at 128 for two reasons that were retired rather than overruled: the aarch64 FB hole was
"another session's lane" (this arc raises it, with the Pi's `.bss`-to-heap margin measured at
15.31 MiB), and the Pi gate's 300-frame checksum was called "a 128 fact" when it is a fact about the
render, restated at 288 in `pi4-regression.spec`. A bare `target_arch` gate in the experience layer
with no hardware reason is what §"ARCH-NEUTRAL BY DEFAULT" fails at review. Full seam-by-seam record:
`02_KERNEL_CORE/userspace.md`, "CRYSTAL-HD".

**CRYSTAL-PACE — NOT landed; §5.1 already decided it.** The third half added a `SYS_WIN_PRESENT`
status (`WIN_PRESENT_COALESCED = 2`) telling ring 3 its present had been absorbed into the panel
frame in flight, and a `pace_edge` loop in `user-vug` that parked one kernel tick at a time,
re-presenting until the frame edge admitted one — locking the render loop to the composite cadence.
Three findings, each sufficient on its own:

1. **It is the pacer §5.1 removed, one layer out.** Peter's ruling of 2026-08-13 (`9d12e7e0`) is that
   a vug renders unpaced on every chip, and that the direction is *more drawing complexity, never
   artificial pacing*. A render loop that sleeps until the compositor lets it through is `vsyncpace`'s
   sleep moved across the syscall boundary into the program — and GR25's quoted argument (§5.4) is
   that the machine's answer to a present is how long the work takes, not a sleep chosen on the
   program's behalf. HD and AA land precisely because they are the ruling's other side: 2.25× the
   source density and resolved edges, on the same free-running loop.
2. **It was the only arm that could split verb 30's success contract between the arches.** aarch64
   has no coalescing pacer and could never answer a third status, so a ring-3 program written against
   a 3-valued x86 contract would silently mean something else on the Pi — the divergence the
   present-rows port (§6.6d-closed) was careful not to open. `SYS_WIN_PRESENT` keeps `0` for every
   success on both chips; coalescing stays a compositor-side decision ring 3 cannot see.
3. **Its LOD-ladder change was downstream of the pace loop and dies with it.** CRYSTAL-PACE replaced
   `LOD_UP` with a render-time utilisation license (`busy * 16 < TICK_HZ * 3`) on the premise that
   "with presents locked to the panel a healthy window reads ~60 whether the raster took 2 ms or 15".
   That premise is `pace_edge`'s: without it the vug's meter counts the frames it RENDERS and nothing
   throttles that loop, so the achieved rate is still the honest signal `LOD_UP` was written against.
   The Pi's reading of these constants was fixed at the source instead (§6.8a/§6.8b, `abi_ticks`),
   and that fix deliberately retuned no ladder constant. This arc retunes none either.

**FINDING, and a decision for Peter — at 288 the shard's ON-GLASS box gets SMALLER, on both chips.**
The held commit justified the number 288 as "the largest edge whose window still tiles at an integer
scale >= 3" on the 2880×1800 panel, from `min(2880/2/288, 1800/2/288) = 3`. That arithmetic used the
raw panel height; `wm::place_scale` divides the WORK AREA (`work_h` = panel − `top_chrome_h` −
`chrome_h`). Measured on the bench-geometry gate (`UNAOS_FBW=1920 UNAOS_FBH=1200`, armed):

```
[wc-a] create win=4 asid=0x1 surf=288x288 stride=1152 scale=1x at (17,85)   <- this arc
[wc-a] create win=4 asid=0x1 surf=128x128 stride=512  scale=4x at (17,85)   <- base d99bec68
```

Pi bench, 1920×1200: `top_chrome_h` = 34 (menu bar) and `chrome_h` = `dock_reserve_h` = 64, so
`work_h` = 1102 and `work_h/2/288` = **1**. The shard's content box goes 512 px → 288 px — 44 %
smaller on glass, while gaining 2.25× the source density. The same arithmetic on the rMBP 2880×1800
gives `work_h` = 1702 and `work_h/2/288` = **2**, i.e. 576 px against the 128×128 window's 768 px at
6×: smaller there too. **288 is the first edge that falls off the 2× step on a 1200-row work area**
(2× needs `work_h >= 1152`; there are 1102).

This is reported rather than acted on, because every resolution is someone else's call:

* **(a) Ship 288 as landed** — sharpest possible shard, visibly smaller window. What is committed.
* **(b) Have `user-vug` request 256 rather than the cap.** One line in `main.rs`, entirely in this
  lane, no ABI or cap change: `work_h/2/256` = 2 on the Pi bench → a 512 px content box, **the exact
  on-glass size the 128×128 shard has today, at 2× the source density**; 3 on the rMBP → 768 px, also
  today's size. 256 divides every LOD cell (1/2/4) and every band boundary as cleanly as 288.
* **(c) Change `place_scale`'s fit rule.** `wm.rs`, shared, moves x86 layout for every window — not a
  lane this arc may take, and a desktop-wide design change rather than a vug one.

The 288 CAP is Peter's signed number and is what costs the `.bss`; the vug's REQUESTED edge is a
separate number and (b) is available without re-litigating the cap.

**Gates (this arc).** `./arroyo check` and `UNAOS_WC=1 ./arroyo check` green, both arches. Knob-off
`kernel8-test` **117/117, 0 forbidden**. Armed (`UNAOS_PIDESK=1 UNAOS_QUARRY=1`) and bench-geometry
armed are red on this tree and **equally red on the base**, A/B'd two runs each: 117/117 required on
every run of both trees, the forbidden set drawn from the same nondeterministic
`[wc-d]`/`[wc-g]`/`[dragperf]` families, and at bench geometry this tree shows FEWER distinct failing
families than the base (6 vs 8). `[dragperf]` fails on the base too — its fixture issues one drag
report per 8 ms of wall clock and needs them inside `DRAG_MOTION_MS` = 16 to observe a coalesce; a
loaded host stretches the iteration past 16 ms and `coalesced` reads 0. `[wc-g] rollup win=4 … ->
CLEAN` for the 288 surface at bench geometry: the wider blit is coherent.

### 6.7 The `exec-vugspread` arc — what VUGSPREAD actually is, and what the Pi was missing

**The row above named the gap as "placement/steal repair". Only half of that was true, and the half
that was false is the more interesting one.** Reading `arch/aarch64/sched.rs` against
`arch/x86_64/sched.rs`: the Pi does not have a *worse* scheduler than x86 in general — for PLACEMENT
it has a considerably more elaborate one. The `SPREAD-3` … `SPREAD-14` lineage (committed EL0
residents, parked-vs-runnable load, a per-address-space co-residency bias so a vug's triple lands
together, an idle-core recruit lane, a spare-core predicate, a split/repack pair) has no x86 twin at
all. What it has instead of x86's `try_steal` correction is `make_ready` → `rewake_place`: an EL0
task's placement is re-asked **when it wakes**.

**And that is the hole, stated exactly.** A wake-time corrector can only correct a task that
*sleeps*. `SPREAD-6`'s escapement already saw part of this and rate-limited a re-ask onto a 250 ms
clock for tasks that micro-park; but a thread that is genuinely CPU-bound between presents — a vug
worker ray-tracing its share of the shard — does not park at all, never reaches `make_ready`, and so
**its spawn-time core is permanent.** On x86 that same thread is corrected by an idle core pulling it
over. On the Pi nothing could, because every EL0 task was `steal_ok = false`.

**The stated reason for that flag was stale, and had been since SPREAD-4.** The doc comment read
"EVERY EL0/slot task (which carry per-core TTBR0/ASID state) are pinned". Nothing about an EL0 task is
per-core: `user_ttbr0` is a *value* the task carries and `dispatch_next` installs on whichever core
runs it. SPREAD-4's own soundness block says so in as many words while *moving* parked EL0 tasks
between cores — the address space follows the task, the old core's residual TTBR0 is benign (slot L1s
are a static array; teardown broadcasts `tlbi aside1is`), and `spawn_user_thread`'s "Multi-core
soundness" note already contemplates one ASID live on two cores. So the pin was not protecting a
hardware invariant; it was a sentence that stopped being true and never got re-read.

#### What x86's VUGSPREAD repair is, piece by piece — and the aarch64 verdict on each

| # | x86 piece | aarch64 before | Ported? |
|---|---|---|---|
| 1 | **Ring-3 threads are steal-eligible.** `spawn_user_thread` was `steal_ok = target_cpu == CPU_AUTO`, and `SYS_THREAD_SPAWN` always names a core, so it was always `false`. A ring-3 `place` is a HINT; marking it a pin promoted a user-space hint into a kernel guarantee. | `steal_ok: false` on **both** EL0 slots and EL0 threads. | **YES**, for THREADS only. Slots stay pinned — a windowed app parks on input every frame, so the rewake lane genuinely serves it, and releasing it would have been a second change with no evidence behind it. |
| 2 | **Load-aware sibling pick.** `sibling_online_cpu` returned the first index-order match, so every `place=1` thread on the machine landed on the same low core. | **Already correct.** `other_online_cpu` picks the shallowest ready queue. (It lacks x86's busy-pct tie-break, rotating cursor and render/service deprioritisation — a smaller, separate question, not this gap.) | **NOT A GAP.** |
| 3 | **Per-victim steal floor.** A run queue holds only READY tasks — the task a core is *executing* is in `current`. So a flat floor of 2 needs THREE runnable tasks before a core looks loaded, and 2-on-1 packing sits at depth ONE, invisible by construction. Floor is asked of the victim: 1 if it is running something, 2 if it is between tasks. | `const STEAL_MIN_DEPTH: usize = 2`, flat. Same defect, same shape. | **YES**, via the shared `steal_floor`. |
| 4 | **VUGSPREAD-COOL, the escalating brake.** Per-task `migrations` + `migrate_ms`; a task may not be re-stolen for `16 ms << min(migrations, 4)`. A *flat* window does not stop a ping-pong, it stretches its period. | Nothing — no migration history at all. | **YES.** It is not optional: it is what makes piece 3 safe. |
| 5 | `cr3_live` shadow, `hint_placed` attribution, the `[spread]` census. | n/a (`cr3_live` is an x86 TLB-generation concern with no aarch64 analogue — TLBI is IS-broadcast here). | **Census yes, `cr3_live` no** — legit arch-specific. |

#### What moved, and where

The brief's rule was *arch-neutral, gated on a feature knob, never `target_arch`*. Both schedulers are
irreducibly arch files (`SCHED`/`RUN_QUEUES` + APIC-ms on one side, `rq()`/`CUR_PRIO` + CNTPCT on the
other) and nothing here tries to merge them. What **is** arch-neutral is the *policy* — three
constants and two predicates — and that is exactly what was trapped inside `arch/x86_64/sched.rs`,
which is *why* the Pi never got it. It now lives in **`crates/kernel/src/sched_spread.rs`**:
`STEAL_MIN_DEPTH`, `STEAL_COOLDOWN_MS`, `STEAL_COOLDOWN_ESC_CAP`, `steal_floor(victim_running)`,
`steal_cooldown_ms(migrations)`, `steal_cooled(migrations, migrate_ms, now_ms)`. No `cfg(target_arch)`
appears in the file. Both arches call in; each keeps only the plumbing that is genuinely arch-bound
(*how* you ask "is this core running something", *where* milliseconds come from). x86's behaviour is
byte-for-byte unchanged — its constants and functions became re-exports and one-line delegations.

There is deliberately **no feature knob.** The repair is a correctness fix on the shipped image, the
same disposition §5.2 took for `focus_release`: a knob-gated fix would leave the Pi Peter actually
flashes unrepaired. Per §5.3 this **moves the knob-off `kernel8.img` hash on purpose**, and line
neutrality does not apply — this is not `pidesk` furniture.

#### The accounting transfer, which a naive port would have dropped

`try_steal` retargets `task.cpu`. On x86 that is the whole re-home. On aarch64 it is not: SPREAD-3's
`EL0_RESIDENTS` and SPREAD-10's `SLOT_CORE_RES` are per-(core) and per-(core, ASID slot) commitments
released at exit **against `task.cpu`**. A stolen EL0 thread whose credit stayed behind would grow the
old core's count without bound and saturate the new core's at zero — and every later `pick_cpu_slot`
and `rewake_place` decision would then steer around load that is not there, *permanently*. The port
therefore transfers both credits at re-home, in `make_ready`'s exact order (resident, then slot), and
re-stamps `place_cyc` because a steal **is** a placement decision — without that, the task's very next
wake would find a stale refresh clock and re-ask immediately, fighting the move just made.
`EL0_PARKED` needs no transfer: a task in a run queue is READY by construction.

#### The witness

`[spread4]` gains five fields: `steal= d1= remig= cool= pack=`. They sit on the placement line
deliberately — `rewake` and `steal` are two lanes of ONE question, and a vug worker spinning through a
frame can only ever appear in the second. Read as a set: `pack` = idle passes that SAW a packed
neighbour; `steal` = moves taken; `d1` = moves from a victim at locked depth 1, i.e. the ones the old
constant floor would have refused (**the floor repair's own attribution — a Pi that speeds up with
`d1=0` was not sped up by the floor**); `remig` = moves of a task that had already moved; `cool` =
candidates the escalating brake passed over. Health is `steal` stepping at convergence edges with
`remig` near zero. The revert criterion is `remig` tracking `steal` while `cool` also climbs — the
brake refusing and serving the same oscillation, which is the flat-window failure the escalation
exists to end.

#### One correction to the metal reading this arc was opened on

The brief cited `[spread10] spare=3 sparepct=0 recruit=0` beside a core pinned at 99% while
`[vugfps] wf=1..2`. Convicting that against the logs rather than the code: in
`boot3-inputdeath-tail.txt` every spread counter is **frozen** and `[spread4] live` reads `0/0` on all
four cores — there is no EL0 resident left to place, so that capture is downstream of the failure, not
a picture of it. The capture that *does* carry the vug is `boot2-freeze-tail.txt`, and it says
something different: for most of its length the vug runs at **30–60 fps** with
`[spread4] live c0=0/1 c2=1/1 c3=1/1` and `SCHED: load c0=57% c2=45% c3=44%` — **the triple is spread
across three cores and the Pi's placement lattice is doing its job.** `wf` then falls off a cliff to a
hard-locked `1,2,1,2,…` with `c3=99%` and every other core at 0%, and it never recovers. **That is a
freeze, not a starvation**, and this arc does not claim it. Stated plainly so the next session does not
credit a scheduler port with a fix it did not make: **VUGSPREAD raises the Pi's ceiling; it is not the
`wf=1..2` bug.** The `wf=1..2` tail wants its own arc, and the honest first question there is what on
c3 is at 99% while `[spread4] live` shows no EL0 resident on it.

### 6.8 The `exec-vugslomo` arc — the `wf=1..2` tail, answered

§6.7 closed by naming the open question: *"the honest first question there is what on c3 is at 99%
while `[spread4] live` shows no EL0 resident on it."* This arc answers it, and the answer has two
halves that were being read as one bug.

#### 6.8a THE METER WAS 4x LOW ON EVERY PI METAL BOOT — `wf` was never the vug's rate

`sys_getinfo` handed EL0 `timer::ticks()` **raw**. That is the GLOBAL tick counter and
`timer::on_tick` bumps it from **every core's** timer IRQ, so on 4-core BCM2711 metal it advances at
`4 × TICK_HZ` ≈ 1000 Hz while `una_abi::GETINFO_TICK_HZ` told ring 3 to divide by 250. Every ring-3
program that read the field measured time 4x fast; every rate it computed came out **4x low**.

This is UVUG-7 (P52) one caller further out — the same defect, in the same counter, that made
`arch::ms()` run 4x fast and typematic repeat 4x too quickly. `ms()` was moved to CNTVCT then;
`sys_getinfo` was not.

**The ABIFREEZE assert could not catch it and still cannot.** `assert!(GETINFO_TICK_HZ ==
timer::TICK_HZ)` compares the rate each core *arms its timer at* — both sides were always 250. The
constant was never wrong. What was wrong is that the quantity being published was a **sum over cores
rather than a clock**. The fix is therefore on the publishing side: `timer::abi_ticks()`, off CNTVCT,
which *is* a `TICK_HZ`-rate clock by construction — so the assert now guards what it always read as
guarding.

**Evidence, two metal boots, ~1 600 samples, ratio 4.0 both times** (capture
`pi4-pi0-b1/ttyACM0.log`):

| Boot | `[vugfps] wf=` | `[wcn] win=6 asid=0x1` — the SAME window | ratio |
|---|---|---|---|
| boot6 / PA43 `6de03c87` | `1,2,1,2,…` — **409 ones and 409 twos, perfectly alternating**, 818 consecutive samples | `rate=5.8..6.0/s`, `gap=144..231ms` | **4.0** |
| boot5 / PA42 `0f9a1a4b` | `wf=25..41` | `rate=96..214/s` | **≈3.9** |

The alternation is not noise and not a stall — it is the arithmetic signature. A ~6 fps rate sampled
over the **0.25 s** window the wrong divisor opens gives 1.5 frames, which the integer meter renders
`1,2,1,2,…` forever. The vug's own meter doc predicted exactly this disagreement and named it as the
finding it was built to make; this is that finding's other end.

#### 6.8b IT WAS NOT ONLY A READOUT — the LOD ladder eats the meter's number

`user-vug`'s `lod_adapt` consumes `fps_refresh`'s return directly: `rate < LOD_DOWN` (24) drops a
rung, `rate * 4 < LOD_DOWN` drops **two**, `rate > LOD_UP` (55) climbs one, and `CALM_WINDOWS` (8)
counts the "quiet seconds" before the ceiling relaxes. Against a 4x-low meter on a 0.25 s window those
constants meant something else entirely on the Pi:

| Constant | What it says | What it MEANT on Pi metal |
|---|---|---|
| `LOD_DOWN = 24` | drop below 24 fps | drop below **96** real fps |
| `LOD_UP = 55` | climb above 55 fps | climb above **220** real fps |
| `CALM_WINDOWS = 8` | 8 seconds of calm | **2** seconds |

**This is the PA42 "fps SWING", and it is a limit cycle, not a scheduler artefact.** boot5's
`[vuglod]` trace is a clean two-state oscillator, ~130 flips over the boot:

```
lvl=2011 → 3056 → 2012 → 3058 → 2010 → 3057 → 2013 → 3057 → 2016 → …
   (lod 2, fps 11)   (lod 3, fps 56)   …
```

At level 2 the reported rate (≈57) clears `LOD_UP`, so the ladder climbs; at level 3 the frame costs
~L² more and the reported rate (≈13) falls under `LOD_DOWN`, so it drops. The dead band's stated
guarantee — *"wide enough that no single level can sit on both edges"* — is **false on the Pi**: the
2↔3 pair straddles the whole band. The ceiling pin that exists to stop precisely this releases every
2 s instead of every 8 s, for the same clock reason, so the cycle restarts indefinitely.

**The clock fix breaks the cycle without touching a ladder constant.** With an honest meter, level 3's
real ≈52 fps lands *inside* the dead band `[24, 55]` and the ladder settles there — which is what the
band was designed to do, and it only ever failed because the meter was 4x low. **No LOD constant is
retuned by this arc, deliberately:** retuning them would move x86, where they were always correct.

#### 6.8c THE `wf=1..2` TAIL — throttle, and the correlation verdict the brief asked for

With the meter corrected, PA43's real rate is **≈6 fps against PA42's ≈100–220**. That collapse is
real and it is **not** the meter. The correlation test:

* **The pump storm is continuous, so "inside vs outside a TIMEOUT window" is not answerable on this
  capture.** boot6 carries 97 `BOT pump TIMEOUT` lines, 61 of them inside the vug's lifetime, running
  effectively back to back across the whole ~450 s the vug is up. There is no non-timeout window of
  any length to compare against. Any claim of the form "it recovers between stalls" is unfalsifiable
  here and this arc does not make one.
* **The machine is nevertheless capable of full rate while the pump is wedged.** Fourteen samples at
  four distinct moments read `wf=23..75` (real ≈92–300 fps) *inside* the storm, each alongside a
  `[comp2] rate=` burst of 20–44/s. So the pump is not a hard ceiling — it is a duty-cycle tax.
* **The discriminator is `[fluid3]`, and it is unambiguous.** PA42: `parks=0` for the entire boot —
  the frame barrier never parks. PA43: `parks≈30/5 s, park_us mean=169 602, max=412 786` — and 170 ms
  is *exactly* the vug's `[wcn] gap=144..231ms`. Two committed residents are parked essentially 100 %
  of the time (58 parks × 170 ms ≈ 2 × the 5.07 s span, `depth_max=2`).
* **The placement is the mechanism, and it is EL0-blind.** `[spread4] live c0=0/1 c1=0/0 c2=0/1
  c3=1/1` — the three EL0 residents *are* spread one per core, so the lattice reports itself
  balanced. But `SCHED: load` reads `c0=0% c1=0..3% c2=0% c3=99%` for the whole vug lifetime, because
  `usb-pump` is **`caller-pinned, no-migrate` to core 3** and burning it. **The balancer counts EL0
  residents; it cannot see a pinned kernel task.** The vug's one runnable thread therefore
  time-shares a 99 %-consumed core while three cores sit idle, and its workers park 170 ms a frame
  waiting for it. This answers §6.7's question verbatim: *what is on c3 at 99 % with no EL0 resident
  there* — a pinned `usb-pump`, and `[spread4]`'s columns are structurally unable to say so.

**Verdict: throttle explains the PA43 rate; the meter explains the PA43 and PA42 *readings*; neither
explains the other.** The pinned-pump mechanism is **exec-botpark2's**, and this arc does not touch
`xhci`/`main.rs`. **OWED (new class (b) row):** the aarch64 placement lattice should consult per-core
load, not EL0 resident count alone — `[pulse5]` and `SCHED: load` already carry the number it needs.

#### 6.8d THE LAUNCH PAUSE IS STORAGE, and the x86 launch-stall fix has no Pi analogue

Peter's *"took forever to come up"* is **not** in the present path. The capture places the whole cost
before the program exists:

```
7452  :: BOT: rescue retry rung=port-cycle result=fail err=Timeout budget_scale=1 ::
7454  :: BOT: SURRENDER slot=5 cause=Timeout … disk marked FAILED and retracted
7458  :: BGRUN: bg /fat/vug.elf — loaded 12568 bytes, entry 0x500000, pid=122 asid=1 DETACHED
7486  [wc-a] create win=6 asid=0x1 surf=128x128 stride=512 scale=4x at (17,85) z=17
7489  [wc-h] win=6 box=522x556 … compose_us=1423 present_us=1135 -> BUFFERED
7499  [wc-h] rollup win=6 emit=1 age_ms=51 …
```

The read of `/fat/vug.elf` completes **only after the BOT retry ladder surrenders the wedged reader**;
`[gui] app-exit t=552s dur=12s wedged=true` on the line after `BGRUN` prices the previous attempt at
**12 s**. Once the program is loaded, **spawn → window → four presents → rollup is `age_ms=51`** —
51 ms, of which each present is `present_us≈1 135..1 212`. **The present path is exonerated.**

This is the exact opposite of x86, where `WCD_CHUNK_US` was introduced because a stage-2 verdict cost
~1.26 s inside the launching app's own present. See 6.8e for why that cannot happen here.

#### 6.8e AUDIT — the three merged trunk video arcs against the Pi path

| Merged arc | Pi-path finding | Verdict |
|---|---|---|
| **vug-text-flicker / `PACE_SHADOW`** | `pace_admit`, `pace_service`, `pace_shadow_refresh`, `PACE_SHADOW`, `PACE_SHADOW_OK` and the whole WPACE block are under `#[cfg(all(target_arch = "x86_64", feature = "wc"))]` — 99 such sites in `wm.rs`. The only three `cfg(feature = "wc")` sites without an arch gate are dock/menu/strip **geometry** (`menu_paint_owed`, the WCK5 strip clip, STRIPFACTOR), none of which pace anything. | **CLEAN — Peter's `9d12e7e0` ruling survives the merge. The Pi's vug is unpaced, and this arc adds no pacing.** |
| **wcg-chunk (`WCG_CHUNK_BYTES = 32 KiB`)** | Also `x86_64 + wcg-paygo`, so on the Pi `wcg::on_present` falls through to the **unchunked** full-surface FNV at `wcg.rs:1686`. That is bounded, not a leak: `budget_left` caps it at `SAMPLES = 4` per window id. Measured on boot6: the vug window's entire witness cost is `wit_us=10 076` (10 ms, once); the 1296×736 console's is `wit_us=226 153`. | **NOT a mis-tune. The constant is unreachable on the Pi and its absence costs a bounded one-off.** |
| **blit-collapse (`WCD_CHUNK_US = 2 000`, `WCD_CHUNK_ROWS_MAX = 64`)** | Same gate, so WC-D verdicts run **unchunked** on the Pi — and that is correct here for a reason the x86 constant's own note supplies. The chunking exists because a Kepler-BAR probe costs **~1.06 µs**. The Pi's panel is cached RAM: boot6 measures `probes=953 856 readback_us=10 539` → **~11 ns/probe**, and `probes=16 384 readback_us=520` → ~32 ns/probe. The vug window's whole 262 144-probe verdict is `us=2 760`. **~100x cheaper per probe**, so the 1.26 s hold the chunking exists to break is ~10 ms here. | **NOT a mis-tune, and NOT tuned for the Pi either. Flagged: if the Pi panel ever becomes uncached MMIO, these constants are unreachable and the launch stall arrives with no brake.** |

**None of the three is the `wf=1..2` cause.** The two chunk constants were tuned against a 1.06 µs/probe
PCIe BAR and are simply not on the Pi's path; the pacer is arch-excluded exactly as ruled.

#### 6.8f VUGSPREAD: tune, do not revert — and the revert criterion is reading a downstream signal

§6.7's revert criterion is *"`remig` tracking `steal` while `cool` also climbs."*

* **PA42 (boot5) meets it on its face:** `steal=2136 d1=1656 remig=2122 cool=89603 pack=210953` —
  `remig/steal = 0.993`, `cool` 42x `steal`.
* **PA43 (boot6) cannot test it:** `steal=41 d1=26 remig=28 cool=586 pack=389291` — `remig/steal =
  0.68`, and the whole EL0 population is **three tasks with three of four cores idle** under the
  pinned-pump throttle. Stealing barely arises. Read under throttle, boot6 **neither confirms nor
  refutes** the criterion, and must not be quoted as either.
* **The criterion is firing on a downstream signal.** 6.8b establishes an oscillator **upstream of the
  scheduler**: every LOD flip changes each worker's per-frame spin by ~L², which *is* the run-queue
  depth the stealer reads. An oscillator in the workload will make `remig` track `steal` no matter how
  the brake is tuned — the criterion's own words, *"the brake is refusing and serving the same
  oscillation"*, describe what is happening, but the oscillation is **not the brake's to damp**.

**Verdict: TUNE, and the tune is 6.8a — no cooldown constant is changed by this arc.** The criterion
is **re-armed, not retired**: boot7 must re-read `steal`/`d1`/`remig`/`cool` with an honest meter and
a settled ladder, on a **vug storm**, before the relaxed floor is judged again. A revert taken on
PA42's numbers today would have reverted a scheduler port to fix a ring-3 divisor.

#### 6.8g Gates, and what only metal can prove

`./arroyo check` + `UNAOS_WC=1 ./arroyo check` green both arches.

| Gate | Result |
|---|---|
| knob-off `./arroyo kernel8-test 210` | **111/111**, 0 forbidden, 23 412 lines scanned |
| armed `UNAOS_WC=1 UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` — **this tree** | **111/111**, 0 forbidden, 12 196 lines scanned; banner `witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`; `FB Size: 1920x1200` |
| armed, same command — **baseline `6de03c87`** (throwaway worktree, A/B control) | **111/111**, 0 forbidden, 14 337 lines scanned; same banner and geometry |

The change is **line-neutral by construction** — it adds no `serial_println!`; the `abi_ticks` divisor
rides as fields on the existing `[uvug7] ms clock` line. (The two armed runs' scanned-line totals
differ because `kernel8-test 210` is wall-clock bounded and the rollup instruments free-run inside it;
that spread is not a diff signal, which is why the claim rests on the source and on the knob-off gate.)

The one QEMU-visible A/B is that line itself:

```
before  [uvug7] ms clock: CNTFRQ=62500000 Hz (=62500 kHz per ms); ms=CNTVCT/(CNTFRQ/1000), core-count-independent
after   [uvug7] ms clock: … ; sys_getinfo ticks=CNTVCT/(CNTFRQ/250)=250000 (NOT the per-core-summed tick counter)
```

**QEMU cannot prove the 4x, and this arc does not claim it does.** The defect needs four cores
actually delivering timer IRQs; QEMU raspi4b delivers **none**, so `timer::ticks()` there is frozen at
0 and the old readout never refreshed at all. What QEMU does prove is that the replacement clock is
live and correctly scaled on that host — `[uvug7] ms clock: CNTFRQ=62500000 Hz … sys_getinfo
ticks=CNTVCT/(CNTFRQ/250)=250000`. **The falsifier is on metal, and it is sharp:** boot7 must show
`[vugfps] wf=` and `[wcn] win=N rate=` for the same window **agreeing**, where boot5 and boot6 both
read them 4x apart across ~1 600 samples. If they still disagree by 4, the unit is not the fault and
this section is wrong.

### Class (b) rows owned by arcs already in flight — do not duplicate

| Gap | Sites | Owner |
|---|---|---|
| Title-bar window **dragging** — `wm::drag_begin/motion/end` are arch-neutral but called only from `arch/x86_64/syscall.rs` | `main.rs:1819` (`wc_route_tail`) | **exec-dragperf** |
| **Click-to-focus / raise / close-button** on windows — aarch64's `click1_dispatch` hit-tests only console-vs-status-strip | `main.rs:1693` (`wc_route_event`) | **exec-dragperf** / **exec-conswin** — settle ownership before starting |
| Console window, its route/pace machinery, furniture strips, menubar reservation | `fbcon.rs` ×42, `screen.rs:367`, `ui_status.rs:597`, `wm.rs:16493` | **exec-conswin** — the console-window half is **DONE**: CONSWIN-PI minted the window, **LIVECON** (§6.12) made its text live |
| Shell window | `main.rs` `open_shell_window` | **exec-shellport** |

---

## 6.8 The `exec-fontwire` arc — the face audit, §6.5, and one metric that was never derived

Opened to answer a narrow question — *the trunk's anti-aliased Noto face merged into `hw-pi4`; does
every Pi text surface actually RENDER it?* — and to fold in §6.5 while inside `draw_glyph`. It
returned three things: a face census, a closed §6.5, and a metric defect the bench found first.

### 6.8a The face census — what the Pi actually drew, before and after

Every chrome surface resolves its cell through `wm::TITLE_CELL_W`/`_H`, which is arch-neutral, so the
captions/bar/menu/dock were **already** the anti-aliased face on the Pi at the merge. The console was
not, and the reason was invisible to §1's method — see §6.9.

| Surface | Draw site | Face BEFORE | Face AFTER |
|---|---|---|---|
| Window captions | `wm.rs` `draw_title` | noto16-aa | **noto20-aa** (chrome) |
| Menu bar title + clock | `menubar.rs:669/676` | noto16-aa | **noto20-aa** (chrome) |
| Crystal menu rows | `crystal.rs:854` | noto16-aa | **noto20-aa** (chrome) |
| Dock tile captions | `dock.rs:858` | noto16-aa | **noto20-aa** (chrome) |
| Console / shell window glyphs | `fbcon.rs` `draw_glyph` | **font8x8, 8×8 cell** | **noto16-aa, 7×16 cell** |
| Quarry tree + list text | `quarry/live.rs:596` | font8x8 | font8x8 — *`exec-quarry2`'s lane* |
| Pulse window labels | `pal.rs:135` via `pulsewin` | font8x8 | font8x8 — **still owed** |
| `ui_status` strip, `console.rs` | `pal.rs:135` | font8x8 | font8x8 — **still owed** |
| `instgui` dialogs | `instgui.rs:190` | font8x8 | font8x8 — x86-only, not a Pi surface |

**The fallback boundary, stated honestly.** It is not architectural, which is how the merge's own
summary ("font8x8 fallback for early-boot/aarch64/panic") had it. It is *has this surface reached its
desktop seam*: `fbcon` is font8x8 from reset to the arch's desktop-ready point (x86 `panel_console_
resume`, aarch64 the new `panel_console_face_arm`), and after that point both arches — panic screen
included — are on the face. The three `pal`/`quarry`/`instgui` rows above are a **gap, not a fold**:
each owns a cached-RAM surface and could take `font::draw_row` unchanged. Naming them keeps the
boundary a ledger instead of an excuse.

### 6.8b §6.5 closed — and the accounting, since aarch64 has no glyph-timing instrument

`vperf`'s pixel counter is `cfg(all(target_arch = "x86_64", feature = "videobench"))` and `[wc-g]`
times compositor checksums, not glyph paint, so **there is no QEMU-cycle before/after to quote**. The
measurement below is instead a static store count over `font8x8`'s printable set (0x20..0x7E) — which
is the better evidence anyway, being exact and load-independent, where a QEMU wall-clock on a
contended host is neither.

Per glyph, at the console's live `scale = 1`:

| | calls | format `match` | store instructions |
|---|---|---|---|
| BEFORE (Pi: `put_pixel`) | ~22 | ~22 | ~68 byte stores |
| AFTER (both arches: `fill_span4` over runs) | ~9 | **1** | ~22 word stores |

~22 set pixels per glyph fall into only ~9 horizontal runs (2.5 pixels per run), so coalescing is
where most of the win is and it was free — the bit scan was already per row. x86 improves too: it was
hoisting the decode but still poking one pixel at a time.

The change is deliberately **line-neutral** in `fbcon.rs` (18 lines replaced by 18), per §5.2.3's
rule, so no panic `Location` below it renumbers. The knob-off `kernel8.img` bytes still move — this is
a real codegen change on a path the knob-off boot runs — which is the legitimate half of that rule.

### 6.8c FONT-METRIC — the chrome raster is now DERIVED from the bar

Peter, bench PA43 (1920x1200): *"fonts were looking good BUT window title font size is small for the
size of the title and menu bars."* Not a wiring fault — a metric one. `theme::TITLE_HEIGHT` is 34 px
and carried a 16 px raster (~47% of the band) where the platform the kit quotes sets its caption at
~60%. **x86 has no scale factor the Pi was failing to apply**; the face was genuinely fixed at 16 on
both arches, so the scale seam was the work.

`video::font` now carries two atlases. `Face::Body` stays 16 px — its size is set by a character
grid's capacity, not by furniture. `Face::Chrome`'s raster is `chrome_raster(theme::TITLE_HEIGHT)`:
three fifths of the bar, snapped DOWN to a raster the atlas is built at. At 34 that is **Size20**
(advance 7 → 9). Nothing else learned a number — `menubar`, `crystal` and `dock` already defined
their cells as `wm::TITLE_CELL_*` and their layouts as expressions over those, so `crystal::ITEM_H`
went 24 → 28 keeping its 4 px of air, and the `wm::controls` floor went 149 → 151 by the same
arithmetic it has always used. Two const-asserts that were *pins on today's numbers* rather than
statements of intent (`ITEM_H == 24`) had to be rewritten as the invariants they meant.

### 6.8d Witness, and one observation for the integrator

`[pidesk] faces=title:…,menu:…,crystal:…,dock:…,console:… chrome=WxH body=WxH bar=H ::` — every term
read from the surface's own metric, so a boot that regressed a surface to the bitmap says so on the
serial with nobody looking at the glass.

**Observed, not fixed:** on the armed `UNAOS_PIDESK=1 UNAOS_QUARRY=1` boot at 1920x1200, `[wc-d]
verify` intermittently FAILs for the small fixture windows at `(17,85)` — they sit under the console
window's box and the probe reads panel pixels the console legitimately owns. That is **§6.2** (the Pi
has no `OccClip`), surfaced by this arc only because arming the face changed the console's cell and
therefore its box. `bad_ram=0` on the cache-view variants confirms the pixels themselves are correct.
The armed 1920x1200 configuration is red on the unmodified baseline too (108/111); the knob-off gate
this track actually gates on is 111/111.

---

## 6.9 The `exec-armedfix` arc — DESKHOLD, and the one writer three witnesses were reading

CHROMESPEC (`508ca35d`), REALDESK (`96cc0c0d`) and FONTWIRE (`cb787847`) are each green on their own
branch. Assembled, the armed gate (`UNAOS_PIDESK=1 UNAOS_QUARRY=1 ./arroyo kernel8-test 300`) is red
on two hard FAILs neither arc has alone. Both are the SAME second writer, and it is not one any of the
three introduced — all three did was make it visible, louder, and unavoidable.

### 6.9a The writer: aarch64 has never had x86's QUIET-PANEL gate

`fbcon::_print` on x86 returns at its FIRST test unless `bootlog` or `PANEL_CONSOLE` is set, so a `wc`
desktop is **never** mirrored to. aarch64 has no such gate: the whole serial stream paints the panel,
from every core, until `GUI_ACTIVE`. Harmless while the Pi's glass *was* the console. Not harmless
from the line `pidesk::activate` clears the panel to `DESKTOP_BG` and starts compositing onto it —
from there until `panel_console_window_open` installs the glyph route there are **two writers on the
same pixels**, and the compositor is the quiet one:

* `FbCon::plan_newline` emits two `Op::Fill` bands per newline, and `paint_ops` renders them with
  `fill_rows` — **full panel width**, one cell tall. A printing core therefore repaints `2 * cell_h`
  rows of the glass edge to edge, per line, at `fbcon::BG_DEFAULT` — which is `0x000000`.
* FONTWIRE took that cell from `8x8` to `7x16`: **twice the rows blacked per line**, and the cursor
  now sweeps the whole 480-row panel in 30 lines instead of 60. It also arms the face at the TOP of
  `activate`, i.e. inside the two-writer window, and the anti-aliased path paints each glyph's **full
  opaque cell** rather than ~22 set bits.
* CHROMESPEC judged `wcf::reserved`'s two boxes separately, which is correct and is what made
  `[wc-f] twin` RUN on the armed desktop at all. Its one shot lands on the first composite pass with
  `drawn > 0` — which is `wm::create_at` **inside** `panel_console_window_open`, i.e. squarely inside
  the two-writer window.

### 6.9b The two convictions, by colour

**`[wc-f] twin … checked=8192/8192 comp_bad=4096 direct_bad=4096 first=(480,400) got=0x000000
want=0x1010f0 -> FAIL`.** 100% of *both* blocks, at `0x000000`. The probe paints, cleans, invalidates
and reads back inside one function; nothing in the compositor writes black there. `0x000000` is
`fbcon::BG_DEFAULT` and nothing else on this panel is (`video::PANEL_BG` is `0x1e1e1e`,
`wm::DESKTOP_BG` `0x2d2b55`). The same capture corroborates it 14 px away, on a pixel WC-F never
touches: `[chrome-truth] pt=desktop at=(638,478) want=0x2d2b55 got=0x000000 -> NOCLEAR`. With the hold
in place that same probe reads `got=0x2d2b55 -> HIT`, which is the conviction closed from the other
end.

**`[wc-d] verify win=1 surf=560x352 band=0..80 … bad_cache=1183 bad_ram=1183 first=(183,93)
got=0x1b1b1b want=0x000000 -> FAIL`.** The panel carries an anti-aliased `fbcon::FG_DEFAULT`
(`0xc0c0c0`) edge over `BG_DEFAULT` where the window's surface is black — i.e. console glyphs painted
straight onto the glass on top of the window's blit. **`0x1b1b1b` is FONTWIRE's fingerprint**: the
1-bit `font8x8` face could only ever put `0xc0c0c0` or nothing on the glass, and the tree's own record
of this class (pi4-regression.spec, THE RESIDUE) reads `got=0xc0c0c0`.

### 6.9c DESKHOLD — the fix, and where it is *not*

`fbcon::panel_mirror_hold(true)` is armed by `pidesk::activate` in the same block as the DESKTOP-CLEAR
that takes the glass. Held, `_print` charges the tap `suppressed` and returns **before it touches the
console lock at all**; serial is untouched. It lifts by construction the moment `CONSOLE_WIN` is
installed — from there `draw_fb()` is the window's surface and there is no panel write left to hold —
and stands for the boot on the decline path, where there is no window. `PANIC_MIRROR` overrides it, so
the red backdrop still reaches the glass. One line changed in `_print` (line-neutral, §5.3), the rest
appended at `fbcon.rs`'s existing APPEND-ONLY TAIL.

Two things this deliberately does **not** do. It does not touch a witness: `wcf.rs` and `wm.rs` are
byte-identical to the merge tip, so every go-red path is the one CHROMESPEC proved. And it does not
reach the residue class `pidesk.rs` already names and assigns — *"a console that presents from
arbitrary print context is an unsynchronised compositor client"*. After the route install fbcon writes
the console window's **surface** from print context while the compositor blits and checksums it, which
is what still produces the odd `[wc-g] win=1 … -> COHER` and, less often, a `[wc-d] verify win=1`
whose panel lags an unpresented band. That is `exec-shellport`'s pacing lane, unchanged by this arc
and now the only cause left rather than one of two.

### 6.9d Two OWED items this arc measured and did not take

1. **The dock paints over WC-F's twin box.** `wcf::reserved` seats the twins at `ph-MARGIN-SIDE` —
   rows `400..464` at 640x480 — and `dock::Layout` seats the strip at `ph-PAD-STRIP_H`, rows
   `416..468`, roughly `x 49..591`. The marks the bench operator is asked to photograph are therefore
   overpainted across 48 of their 64 rows. `ui_status::free_span` already yields the corners to WC-F;
   the dock has no equivalent deference and no witness says so.
2. **The twin's retry path is closed once the desktop fills.** `wcf::clearance` is each box's
   FULL-WIDTH row span (correct — the cache maintenance is per-scanline), so any window reaching rows
   `400..464` vetoes the twins. REALDESK's `bottom_reserved=76->64` widens the unreserved slice above
   the dock from 4 rows to 16, and every armed capture on this tip duly ends with a late
   `[wc-f] twin -> DEFER`. The verdict therefore rests entirely on winning the FIRST composite pass.
   The derived fix is for the bottom reservation to cover what the bottom strip actually holds —
   `dock_reserve_h()` maxed against WC-F's own published `MARGIN + SIDE` (80 rows) — but that moves
   `wm::place`'s work area and with it every pinned geometry the spec reads, so it wants its own arc
   and its own gate, not a late edit in this one.

### 6.10 §6.2 — what `exec-occ62` closed, what it did not, and the one fault it had to fix on the way

**The mechanism was never rebuilt.** The x86 occlusion clip was already complete and correct in-tree;
this was call-site and `cfg` work, plus one redesign the aarch64 stack forced. Two commits:

| | What moved | Gate it now carries |
|---|---|---|
| **M1** | `occ_clip`'s WINDOW half-space — every live, non-`compat` row above the shell, stacked after the subject under `(z, id)` | unconditional (behaviour, not a knob) |
| **M1** | The read-back EXCUSE: `OccSnap`, `occluders_above`, `occ_excuse`, the per-pixel attribution arm, `occluded=`/`occ=` on `[wc-d]`/`[wc-g]` | `feature = "witness"` |
| **M2** | The FURNITURE arm — dock strip, menu bar, SHARD dropdown — plus `dock_tiles` (§6.4) and `OccClip::push` | `any(all(x86_64, wc), all(aarch64, pidesk))` |

**Why the excuse had to land in the same commit as the clip.** A clip without one MANUFACTURES
failures: the blit declines pixels by design, the witness reads the panel's older contents inside a
verified rect, and prints `-> FAIL` for a defect that does not exist. The `OccSnap` ledger states the
rule directly — *the excuse must never be narrower than the clip* — so the arch gate on the excuse was
never independent of the arch gate on the clip. It came off with it.

**Knob-off byte-identity moves on aarch64, deliberately.** The window term is unconditional behaviour
and `[wc-d]`/`[wc-g]` now carry `occluded=`/`occ=` on that arch. Both are insertions inside spans
`pi4-regression.spec` already matches with `.*`, no terminal verdict moved, and the knob-off gate reads
**117/117, 0 forbidden**. DRAG-PI set this precedent; it is the honest cost of a real behaviour change.

**THE FAULT, because it is the most useful thing in this section.** The first cut ported x86's shape
verbatim, and that shape passes `OccSnap` BY VALUE: a field in `wcg::Probe`, a field in `wm::VerifyRef`,
and an argument temporary at each of `wcg::begin`/`end` — four copies of `MAX_WINDOWS * 32 + 8` = **392
bytes** live on the compositor frame per window. x86 absorbed it. The aarch64 task stack did not:

```
this arc, armed 1920x1200 : [wc-a] create win=3 … surf=960x583
                            === AARCH64 EXCEPTION:            ← boot dies, 1391 lines
base sha b9ae9112, same cmd: reaches the SAME create event and runs on, 21096 lines, 0 exceptions
```

The fix is that the snapshot is **owned once per window iteration in `composite_inner` and lent out**:
`Probe` and `VerifyRef` lose their fields, `wcg::begin` drops the parameter, `wcg::end` /
`verify_window` / `readback` borrow. x86 took the same shape — paying four copies it never needed is
the same defect, merely unconvicted there. Taking ONE pre-blit snapshot where two were taken also
strengthens the law `occ_excuse` exists for: all four excuse points now speak from one instant rather
than two that could drift, and it cannot narrow the excuse below the clip because `occ_excuse` unions
the clip in unconditionally.

*The general lesson, worth more than the fix:* **a by-value type that is affordable on one arch's stack
is not thereby portable.** A parity port that only moves `cfg` attributes cannot see this class — the
census greps attributes, and `size_of` is not an attribute. A sibling arc (`u7stack`) convicted the same
class independently in the same sitting.

**The evidence the protection works**, from the armed bench-geometry run:

```
base sha : [wc-d] verify win=3 … at (17,85) … bad_cache=1745 bad_ram=7488 …
                  first=(17,155) got=0x2d2b55 want=0xff2020 -> FAIL
this arc : [wc-d] verify win=3 … at (17,85) … bad_cache=0 bad_ram=0 … occluded=0 occ=1/1 … -> PASS
this arc : [wc-d] verify win=3 … at (640,339) … occluded=20736 occ=1/1 … -> PASS
```

`occ=1/1` proves the excuse set is populated rather than structurally zero; `occluded=20736` on a
fully-covered subject proves it FIRES. Zero `[wc-d] … -> FAIL` remain on the armed run.

#### What is still OWED after this arc

1. **The deferred-erase side.** `erase_clip` and `screen.rs:1050` are untouched and still x86-only.
   §6.2 as originally written named both the blit and the erase; **only the blit is closed.** A
   deferred erase on the Pi can still publish over a strip.
2. **The `[drag-occ]` witness legs** (`occclip_dock`/`occclip_bar`, `OD_*`/`OB_*`) stay x86 by choice —
   that wire belongs to the drag instrument another arc owns. On aarch64 the strips ARE in the clip and
   those fields do not report it, so **silence there is an absent instrument, not a zero.** Named in
   `occ_clip`'s ledger so no future capture is misread.
3. **The SHELLPIN residual**, carried from `dock.rs`'s integrator note and now also disclosed on the
   consumer. `dock_tiles` counts dock-addressable ROWS and cannot see the tile `dock::pin_shell`
   appends while the shell is closed, so the clip protects a strip one tile narrower than the painted
   one; a drag across the pinned tile clobbers it for one pass and `dock::compose` repairs it. The fix
   is one `+ 1` term — **not taken here because `dock_tiles` also feeds the x86 clip and the term would
   move x86 pixels**, which this arc's constraint forbids.

*(Items 1 and 3 are answered in §6.11 — item 3 was already false when it was written. Item 2 stands.)*

---

### 6.11 The `exec-eraseclip` arc — §6.2's ERASE half, and a disclosure that outlived its defect

**§6.2 is now closed on both paths.** occ62 closed the WINDOW BLIT; this closes the DEFERRED ERASE.
One commit, `9937ac9e`, and — as with occ62 — the mechanism was never rebuilt: `erase_clip`'s body is
the shape x86 has run since WCK4, and the work was `cfg` boundaries plus the witness wire.

| | What moved | Gate it now carries |
|---|---|---|
| **M1** | `erase_clip`'s WINDOW half-space — every live, non-`compat` row above the shell | unconditional (behaviour, not a knob), matching occ62's window term |
| **M1** | `erase_clip`'s FURNITURE arm — `strip::rects`, i.e. dock + menu bar from the registry | `any(all(x86_64, wc), all(aarch64, pidesk))` — the furniture family's own gate |
| **M1** | `OccClip::push` | ungated (was `any(x86_64, all(aarch64, pidesk))`) |
| **M1** | The erase-side witness: `DO_FILL_*`, `DO_CLIP_N`, `DO_DOCK_BOX`, `DO_STRIP_N`, `DO_BAR_W`, `dragfill_box`, `span_occ`, `covered_len` and their `stage_fill` legs | `feature = "witness"` (arch term dropped) |
| **M1** | `[erase-occ]` — a NEW aarch64 report line | `witness` + `aarch64` |

**Why `push` had to be ungated, which an attribute census could not have predicted.** `occ_clip`'s
window loop writes `boxes[n]` directly under an `OCC_CLIP_MAX` bound; `erase_clip`'s admits through
`push`, because it has no `boxes_overlap` pre-filter and must REPORT a drop rather than write past the
end. Making the erase's window loop unconditional therefore made `push` reachable on the **knob-off**
aarch64 build, where its old gate did not compile it — an `E0599` whose fault is in which arm *calls*
the function, not in the function's own gate. Same family as occ62's `OccSnap` stack fault: a parity
port that only moves `cfg` attributes cannot see this class.

**`erase_clip` keeps the lock occ62's clip could not have.** It runs ONCE PER DRAIN, so its `TABLE`
acquisition and the `dock_scan` inside `strip::rects` are affordable; occ62's "no second table lock"
rule was a property of the per-window blit loop and is deliberately **not** imported here.

#### The witness FIRES on aarch64, and why it is a new tag

§6.10 item 2 left `[drag-occ]`'s `occclip_*` fields x86-only and documented the silence as an absent
instrument. For the ERASE the instruction was the opposite — fire the legs — and they do, on
**`[erase-occ]`**, not on `[drag-occ]`. `[drag-occ]` carries the whole gesture budget, including
blit-side terms (`occ_px`, `clip_px`, `buried`, `direct`, `occclip_*`) fed by statics this arc does
not own. Printing the erase half under that tag with the rest missing is the "one field, two meanings
on two arches" defect GR13 convicted three instruments for. Every field on `[erase-occ]` means exactly
what the identically-named field on `[drag-occ]` means.

**The evidence the erase clip works**, from the armed bench-geometry run — these lines do not exist at
the base sha at all, because the aarch64 clip was structurally empty:

```
[erase-occ] win=3 owner=0xd40 moves=10 fill_px=0 fillpub_px=1308476 fillclip_px=2164162
            fillover_px=0 fillruns=9190 clipn=5 dock=444x52+738+1136 bars=2/2 bar=1920
            fillclip_dock_px=0 -> CLEAN
[wc-k] erase box=480x480 staged=yes rowbytes=1920 runs=480 spans=0   contig=yes ... -> BUFFERED
[wc-k] erase box=298x332 staged=yes rowbytes=1192 runs=332 spans=298 contig=yes ... -> BUFFERED
```

`clipn=5` proves the clip is populated rather than structurally zero (three windows plus both
furniture strips); `fillclip_px=2164162` is 2.16M pixels the erase WITHHELD that the base sha would
have published over live windows and strips; `fillover_px=0` is the span-walk audit clean. The
`[wc-k]` pair is the arch-neutral corroboration: `runs=480 spans=0` is a fully-occluded erase
publishing nothing at all, `runs=332 spans=298` a partially-occluded one — both impossible on aarch64
before this arc, where `spans` always equalled `runs`.

#### SHELLPIN — there was nothing to take; there were three stale disclosures to fix

§6.10 item 3 was **already false when it was written**. The `+ 1` term is in the tree: the integrator
landed it as `4c6ca42d` ("SHELLPIN integrator fix — `occ_clip`'s dock width counts the pinned tile"),
on both arches, taking the x86 pixel delta deliberately. `dock_tiles` adds the pinned tile when no
live row carries `KERNEL_OWNER_DESKTOP`, capped at `MAX_WINDOWS` exactly as `dock::pin_shell` is — so
the clip cannot be made one tile WIDER than the painted strip, which is the inverse defect.

So the honest option was neither of the two this arc's brief offered. The term did not need adding for
one arch (that would BE an arch gate in the experience layer) and did not need adding for both (it is
already there). What it needed was for `occ_clip`'s ledger, `dock.rs`'s module note and this document
to stop telling readers to expect a residual the code no longer has. All three are corrected in place
rather than deleted, because the text was quoted forward while the defect was live and a reader
meeting one copy needs the others to say plainly that it no longer holds.

**A stale disclosure is worse than no disclosure, because it is read as current** — and this one had
propagated to three files before anyone re-read the function it described. The mirror of occ62's
lesson: a census greps attributes, and a disclosure is not an attribute either.

#### Gates

* `./arroyo check` and `UNAOS_WC=1 ./arroyo check` — green, both arches.
* Knob-off `./arroyo kernel8-test 210` — **MBENCH PASS, 117/117 required witnesses, 0 forbidden hit(s),
  19506 lines scanned.** At floor. Note the knob-off leg runs **no drags at all** (`[drag]` count 0),
  so `[erase-occ]` is legitimately absent there — a vacant instrument, not a silent one, and the
  `[wc-k]` `runs`/`spans` pair is what carries the erase reading on that leg.
* The armed bench-geometry leg (`UNAOS_PIDESK=1 UNAOS_FBW=1920 UNAOS_FBH=1200`) is **red at the base
  sha as well** (116/117). Its A/B and the attribution of every residue are in the arc's landing
  report; none of the shared residue belongs to §6.2.

#### What is still OWED after this arc

1. `[drag-occ]`'s `occclip_*` fields (§6.10 item 2) remain x86 by choice — unchanged by this arc.
2. The armed leg's pre-existing red (`[wc-d] verify win=3 … -> FAIL`, `[wc-c] side-by-side drawn=1`,
   `[wc-g] … slow=yes -> RACE`, `[dragperf] … -> FAIL`) is present at the base sha and belongs to
   whichever arc owns those instruments. It is named here so a future capture is not misread as this
   arc's.

---

### 6.12 The `exec-livecon` arc — the console window's text is LIVE, and it presents from the render core

**What was owed.** CONSWIN-PI gave the Pi a console window and then, in `video/pidesk.rs`'s
live-console ledger, recorded the one thing it could not deliver: the window is a **frozen boot-log
snapshot**. Keeping it live was implemented, measured at bench geometry, and reverted — a 108/108 run
became **97/108 with 37 forbidden hits and a synchronous exception**. That ledger also named the
repair, and named it precisely enough to implement without re-deriving it:

> move the console's presents off print context entirely and onto the RENDER core, one paced call per
> frame, via the hook `fbcon::console_service` already provides for exactly this on x86's bench lane.

This arc is that paragraph, taken. It is `livecon`, default OFF.

#### The diagnosis the revert established, restated because it is the whole design

`fbcon::detach()` is **not** what freezes the window; it is only the last of two things that do. The
route itself already discharges the detach's stated reason — `console_is_routed`'s argument is that
the detach exists so exactly one core writes the PANEL, and a routed console writes kernel RAM. That
argument is correct and it is incomplete: *discharging "who writes the panel" does not discharge "who
drives the COMPOSITOR."* A routed console presents from **print context, on whatever core printed**,
and after the handoff the Pi prints from every core it has. The window was live and the console was an
unsynchronised compositor client — `[wc-g] … slow=yes -> RACE-BLIT`, `[wc-d] verify … -> FAIL`, and a
synchronous exception. The blocker was never the detach. It was the *cadence and the core count*.

#### What was ported — call sites, not a rebuild

Nothing in the console's route/pace/pending machinery moved. `PEND`, the `Owed` three-way, `PACE_HZ`,
the `ROUTE_BUSY` guard, the recycled-id fence and `pend_take`'s degradations are untouched; the
already-existing guarantee "a HELD band is carried by the next present" is the exact guarantee the
port leans on. Three statements:

1. **`fbcon`'s three PRINT-CONTEXT present entries defer.** `route_present`, `route_present_rows` and
   `route_present_pending` record their rows in `PEND` — precisely as a pacing-gate hold records them
   — and return without taking `wm`'s window table and without compositing. The latch is
   `fbcon::PRESENT_DEFERRED` and the test is `present_deferred()`; both live in the file's
   **append-only tail**, and all three entries are **LINE-NEUTRAL folds** (§5.3).
2. **The Pi's `render_service` takes the ledger, once per pass**, through
   `fbcon::console_live_service()` — a readback bracket around the pre-existing
   `fbcon::console_service()`, the hook x86's `usbdebug` loop has called since FBCON-PACE. This arc
   gives that hook its **second caller** rather than inventing a second mechanism. Also a LINE-NEUTRAL
   fold, into `main.rs`'s `let t0 = …` line.
3. **`pidesk::activate` therefore returns `routed`**, so the GUI handoff's existing
   `if !pidesk_activate_maybe() { detach(); }` guard skips the detach and `_print` keeps reaching the
   window's surface for the rest of the boot. The arming of the deferral is at the **tail** of
   `activate`, which is load-bearing: everything `activate` itself printed reached the window inline,
   on the BSP, before the render task exists — deferring from the top would leave the window blank
   until the first event.

The console therefore stops being an N-core client at line cadence and becomes a **single-core client
at frame cadence**, which is the shape the witness battery can survive.

#### THE PANIC PATH LAW is preserved, and preserved at the latch

A deferred present would be a present the panic path never receives — the render task is not scheduled
again after a panic. So `present_deferred()` is `PRESENT_DEFERRED && !PANIC_MIRROR`: from
`panic_screen` onwards every print composites inline on the panicking core exactly as it does today.
That is the second of two independent guards (`panic_screen` also clears `CONSOLE_WIN`), the same
belt-and-braces `draw_fb` and `panel_mirror_held` keep. Every other degradation is toward staleness,
never toward a dropped band: a wedged render core leaves rows *owed* in `PEND`, and the next take — a
pass, a `console_flush`, or `detach`'s sync point — carries them.

#### The cost, stated rather than hidden

`render_service` blocks on `GUI_CHANNEL.recv()`, so a line printed by a core that generates no GUI
event waits for the next pass. The floor on that is the strip pulse's `ui_status::PSTRIP_PERIOD_MS`
timer — the same free wake `pidesk::armed()` rides. **The console is live at the pulse rate at worst
and immediately on interaction at best; it is not a 60 Hz console.** Adding a wake from print context
would put channel traffic back on the very path this arc is taking work off, which is how the reverted
cut failed.

#### ONE OS: the gate is `feature = "livecon"`, never `target_arch`

Every gate this arc adds is the knob and nothing else. On x86 the latch exists and is simply never
armed (`wcx::activate` does not call `console_present_defer`), so the `wc` desktop and the `usbdebug`
bench lane are byte-unchanged — and x86's own desktop lane, which ships the same frozen snapshot for
the same reason, is **one call site** from the same fix whenever that arc wants it.

#### Gates

* `./arroyo check` — **green, both arches** (`x86-all` and `arm-pi` legs both carry `livecon`, so both
  polarities of every new `#[cfg]` are type-checked).
* `UNAOS_WC=1 ./arroyo check` — **green**.
* `./arroyo kernel8` — builds; **knob-off `kernel8.img` is BYTE-IDENTICAL to the base sha**
  (`c0099483`): `e30fb32d99bcc0d4e597f0b16b835374` from both trees. The line-neutral discipline held.
* Knob-off `./arroyo kernel8-test 150` — **MBENCH PASS, 117/117 required witnesses, 0 forbidden hits,
  8366 lines scanned.** At floor.
* **The proof, on the wire.** `UNAOS_PIDESK=1 UNAOS_LIVECON=1 UNAOS_FBW=1920 UNAOS_FBH=1200
  ./arroyo kernel8-test 150`:

  ```
  [wc-x] console-window win=1 panel=1920x1200 surf=1295x736 box=1305x780 at (307,158) cell=7x16 cols=185 rows=46
  [wc-x] console-route first-paint win=1 (glyphs -> window surface, damage-limited)
  [pidesk] livecon ARMED console_win=1 (presents deferred off print context; …the handoff SKIPS the detach and the window stays LIVE)
  [wc-x] livecon census presents=8 ran=13 held=18 busy=26 idle=0 (the console window presented 8 times FROM THE RENDER SERVICE after the GUI handoff — a frozen snapshot presents none and prints no such line)
  ```

  `presents=8` is read back from `PACE_RAN` across the service call, so it counts presents that
  actually happened rather than passes that asked for one. `held=18` is where the deferred
  print-context lines landed. **The mechanism as two numbers: text arrived on N cores, pixels left on
  one.** A second run reproduced it (`presents=8 ran=12 held=14 busy=21`).

#### The armed bench-geometry leg — A/B, and it is red in BOTH polarities

That leg is **red at the base sha** (§6.10 records the same), so the reading that matters is the
comparison, not the verdict. Five runs on this tip:

| Leg | Verdict | Forbidden hits |
|---|---|---|
| `UNAOS_PIDESK=1` (control) | 116/117 | 9 |
| `UNAOS_PIDESK=1` (control) | 116/117 | 12 |
| `UNAOS_PIDESK=1 UNAOS_LIVECON=1` | 116/117 | 9 |
| `UNAOS_PIDESK=1 UNAOS_LIVECON=1` | 116/117 | 65 |
| `UNAOS_PIDESK=1 UNAOS_LIVECON=1` | 115/117 | 16 |

**No failure class is introduced by the knob.** Every hit in every armed run is drawn from the same
set the control produces — `[wc-c] side-by-side drawn=1` (the missing REQUIRE, present in all five),
`[wc-d] verify win=3 … -> FAIL`, `[wc-g] … -> COHER`/`RACE`, `[dragperf] -> FAIL` — all of which §6.10
already attributes to arcs that own those instruments. Compare the reverted cut, whose signature was a
**new** class outright (a synchronous exception) and 37 hits against a 108/108 control.

Two residues named honestly rather than averaged away:

1. **The 65-hit run is 54 hits of one noisy pattern**, `[wc-h] .*presspread=[0-9] .*-> AT-RISK`, all on
   `win=1`. It is **not livecon-exclusive** — the control produced 7 of the same in one run and an
   armed run produced 9 — but the outlier is real and it is the console window's own rollup. The
   plausible mechanism is benign and worth stating: a live console presents repeatedly, so `[wc-h]`
   finally has a *population* for `win=1` to roll up, where a frozen console presents ~4 times all
   boot and rarely emits at all. It is the compositor's telemetry describing a window that is now
   busy, not a new fault. **A bench run should confirm or refute that reading.**
2. **The 115/117 run's extra miss** is `[pstrip] rollup … skipped=[1-9]`, i.e. the SCHED-6 dirty-flag
   pacer reporting no skipped passes. `console_live_service` does not set `dirty`, so it cannot
   suppress a skip by construction; the run was also the shortest of the five (12912 lines scanned
   against 20004). Read as run-length noise, and named so a future capture is not misled.

#### What is still OWED after this arc

1. **x86's desktop lane still freezes its console**, and now for no reason but a missing call:
   `wcx::activate` needs the same `console_present_defer(true)` and `x86_render_service` the same
   `console_live_service()` line. Deliberately not taken here — the lane, the files and the gate are
   x86's, and this is a Pi-track arc.
2. **The default is still the frozen snapshot.** The knob exists because the fixed *shape* is argued
   and QEMU-measured but not bench-measured, and the reverted cut is the standing proof that this
   particular argument is worth checking on metal. Flipping the default is a bench decision.
3. **`[wc-h]`'s `win=1` AT-RISK population** (residue 1 above) wants a bench reading.

### 6.13 The `exec-wchun` arc — the bench reading §6.12 asked for, and what it says about both FORBIDs

Investigation arc against `~/unaos-bench/capture/pi4-pi1-b1/ttyACM0.log` (three boot windows: an
earlier-session boot here called **PA44**, then **boot 7** and **boot 8** from the later session).
It answers §6.12's residue 1 and owed item 3, and it prices the `AT-RISK` FORBID against metal.

#### The `-> UNSTAGED` flood is ONE WINDOW, in ONE BOOT — not a spec artifact and not a compositor regression

`[wc-h] … -> UNSTAGED` fires **663 times in PA44 and zero times in boot 7 and boot 8**, and all 663
are the same window:

| boot | `wc-h` lines | rollups | `-> UNSTAGED` | `-> AT-RISK` | windows declining |
|---|---|---|---|---|---|
| PA44 | 2584 | 2561 | **663** (all `win=6`) | 921 | `win=6` only |
| boot 7 | 1639 | 1616 | **0** | 264 | none |
| boot 8 | 52 | 30 | **0** | 0 | none |

Boot 7 is a fair control, not a short one: 1616 rollups against PA44's 2561, over a comparable soak.
So the condition is **real, sustained, and window-scoped**, not a replay artifact of a QEMU-shaped
spec — the FORBID assumes no fixture cadence the desktop lacks, it simply reports what PA44 did.

`win=6` in PA44 is a *second* 288x288 tile minted at `(17,85)` (`[wc-a] create win=6 asid=0x1
surf=288x288 … z=23`), stacked over the `win=4` tile already at that exact spot. Boots 7 and 8 mint
their second tile onto an existing id instead, and neither declines at all. Its decline census is
**bursty, not steady** — long plateaus punctuated by jumps — and it never tears:

```
emit=1   age_ms=99       declines=0    whole=4
emit=61  age_ms=124335   declines=391  whole=7380
emit=181 age_ms=368661   declines=424  whole=19515     <- 120 emits, +10 declines
emit=241 age_ms=491341   declines=1126 whole=24531     <- +702 in 60
emit=661 age_ms=1348782  declines=3281 whole=62107
```

~5% of that window's composites took the direct path, with `torn=0` throughout. **The verdict is
correct and the diagnosis was impossible**, which is the finding that mattered: `declines=` is
unbudgeted, but a decline's *reason* was only ever written on the per-sample
`[wc-h] win=N staged=no reason=… -> DIRECT` line, and that line spends the same 4-sample budget the
staged presents spend. `win=6` burned its four on the three `-> BUFFERED` composites of its first
paint, so the entire capture contains **one** `reason=` line (`win=1 … reason=fixture`) and not one
for any of the 3281 declines. This is exactly the flaw `H_BTAKEN` already documents about banded
presents — *an instrument whose budget is spent by the control can never see the treatment*.

**Fix taken (in lane, `wcg.rs`):** an unbudgeted per-reason decline census, printed as an INSERTION
directly after the `declines=` total it decomposes —
`declines=N decl_geom=N decl_cap=N decl_lock=N decl_alloc=N`. No verdict, precedence or existing key
moves. A repeat of PA44 now names which of the four exits of `stage_window` the fallback took, and
the four are four different faults: a permanent `DECL_CAP` and a bursty `DECL_LOCK` read identically
today. **No spec change** — the `-> UNSTAGED` FORBID is right and the capture is genuinely red.

#### The `presspread` metal baseline, and the `AT-RISK` FORBID's price

§6.12's residue 1 guessed that `win=1`'s AT-RISK flood was "the console window finally having a
population to report". **The population half is confirmed; the FORBID half is refuted.** Boot 8's
real window, per busy window:

| win | box / role | presents (`whole=`) | `minpresent_us` | `maxpresent_us` | `presspread=` | verdict |
|---|---|---|---|---|---|---|
| 1 | 1305x780 console | 108 (+5 banded) | 78 | 3245 | 1 → 2 → 4 → **5** | TEAR-FREE |
| 2 | 1162x764 app | 111 | 16 | 2832 | 1 → **181** | TEAR-FREE |
| 3 | 1290x164 strip | 106 | 525 | 672 | **1** | TEAR-FREE |
| 4 | 288x288 tile (busiest) | 471 (+1 banded) | 14 | 1529 | 5 → **6** | TEAR-FREE |
| 5 | 64x64 tile | 32 | 40 | 174 | 1 → **2** | TEAR-FREE |
| 6 | 8x8 sprite | 4 | 47 | 60 | **1** | TEAR-FREE |

Across all three boots the two populations are cleanly separated, and **not where the FORBID sits**:

* **Healthy (`TEAR-FREE`) metal spreads: 1, 2, 3, 4, 5, 6, 7** — plus one outlier at **181**
  (boot 8 `win=2`, a cold first present at 2832 µs beside warm ones at 16 µs). Boot 7 sustains
  `presspread=7` on 241 healthy rollups.
* **`AT-RISK` metal spreads: 32, 33, 84, 136 — and nothing else.** Every one carries a huge
  `maxpresent_us` beside a normal floor: `win=1 maxpresent_us=77636 minpresent_us=294 presspread=32`,
  `win=2 maxpresent_us=218876 minpresent_us=2594 presspread=84`, boot 7's
  `win=4 maxpresent_us=75715 minpresent_us=13 presspread=136`.

Boot 7's `win=4` shows the transition inside one boot and one window: `presspread=7 … -> TEAR-FREE`
at `whole≈500–1000`, then `presspread=136 … -> AT-RISK` from `whole=1320` on, as a single 75 ms
outlier lands. There is **no intermediate** — a stall on this machine is either absent or enormous.

Two conclusions:

1. **`FORBID \[wc-h\] .*presspread=[0-9] .*-> AT-RISK` has ZERO REACH on metal, and it is not
   mispriced — it is untested.** It convicts the single-digit (uniformly-slow) class, and no metal
   window has ever produced a uniform-slow tear: all 921 + 264 metal AT-RISK lines are two- or
   three-digit. It correctly stays silent on this capture (the replay's forbidden hits are 663
   `UNSTAGED` + 9 others, and not one `presspread`). **Leave it exactly as it is** — it is an
   absence-shaped assertion, and its silence here is a pass, not a gap. The single-digit flood
   §6.12 saw is a **QEMU** shape: a uniformly slow vCPU compresses the spread into the convicted
   class, which is why the *more consistent* post-fix compositor scored *more* hits.
2. **rmbp's `STALL_SPREAD=8` is well-placed but changes what the spec asserts.** 8 falls in the empty
   gap between metal's healthy ceiling (7) and its AT-RISK floor (32), so it separates the two
   populations cleanly and will not misfire on a healthy window — the observed margin is one unit on
   the healthy side (boot 7 sustains 7) and 24 on the other, so **8 is the lowest defensible value,
   not a comfortable one; anywhere in 8–32 is supported by this baseline.** But it would suppress
   **100% of the AT-RISK verdicts this hardware has ever produced**, leaving the pi4 spec with no
   live tear assertion on metal at all. That is arguably correct — a 218 ms present is a scheduling
   stall, not a copy losing to the beam — but it must be taken as a deliberate trade with a
   replacement assertion beside it, not as a quiet threshold tweak.

#### The x86 column (rmbp 1's re-price, landed their side at 08941dd4 — recorded 2026-08-19)

The same measurement on the rMBP **inverts the discriminator verdict**: presspread is NOT a
discriminator on x86 at all. Seven boots, 8133 `[wc-h]` rollups, every one `torn=0 stalls=0
TEAR-FREE`: values run **continuously 1→118 with no bimodal gap**; 20.9% of healthy rollups sit
above 8; and the outliers are **fast-floor artifacts** (`minpresent_us` 23–726µs pulling the
ratio's denominator down while `maxpresent_us` stays in the normal 1723–7276µs band) — the mirror
image of pi's slow-max shape, and a ratio cannot tell the two apart. pi's honest gap is thereby
real but incidental. Consequences, per-tree under the WMCTRL precedent (exact pins per tree,
union at merge):

| | pi 4 (this file's baseline) | x86 rMBP (rmbp 1, 08941dd4) |
|---|---|---|
| healthy presspread | 1–7 sustained (one 181 outlier) | **1–118 continuous, no gap** |
| AT-RISK population | {32, 33, 84, 136}, slow-max shape | none observed (fast-floor artifacts only) |
| `STALL_SPREAD` pin | **8** (lowest defensible; 8–32 supported) | **256** (2.2x healthy ceiling; loud-false-alarm pricing) |
| pin location | arch-conditional in `wcg.rs`, each cited to its own captures | same commit |

The **shared replacement assertion** both trees adopt (the deliberate trade this section
demanded): `STALL_PRESENT_US = 33334` (2 frames) **absolute, not a ratio** — 4.6x above every
healthy present either machine has produced, 6.5x below pi's smallest captured stall — with a
`longpres=` census and a per-window `-> STALL` line, diverting nothing from `torn=`. **No spec
token on either tree**: `maxpresent_us` suffers the same QEMU vCPU compression as presspread
(a 58–407x desched takes a healthy 1937µs present past any sane bound); the bound is read on
metal via the playbooks. The pi playbook carries the read guidance from dsktp boot 9 on.

---

## 7. Accounting check

501 sites. 10 already ported (family 1) + 67 `wc` gates (family 2) + 165 plain (family 3) + 162
witness (family 4) + 44 paygo (family 5) + 53 `not(...)` arms (family 6). Families 4, 5 and 6 carry no
experience gaps by construction. Families 2 and 3 are itemised above, and every one of their sites
appears in exactly one row of §2, §3, §5 or §6.

**Scope caveat, stated so the count is not read as more than it is.** The census covers `main.rs`,
`pal.rs`, `ui_status.rs` and `video/*.rs` — the files the brief named. **§6.6 reaches outside it**, to
`shell.rs` (6.6a), `arroyo` + `builder` (6.6b), `arch/x86_64/sched.rs` (6.6c) and the syscall
dispatchers (6.6d), because that is where the vug experience actually lives. Those files have **not**
had the §1 census run over them, so §6.6 is a survey of four named arcs, not an exhaustive audit of
its own. **A follow-up arc should run §1's script over `shell.rs`, `arch/*/sched.rs` and both syscall
dispatchers** — on the evidence of 6.6a alone (a bare `#[cfg(target_arch = "x86_64")]` hiding the
single most-used launch verb, *and* a `midden_core` table entry looser than the `#[cfg]` it stood
for, which is a second sweep the census would have to run: every `Avail` against its match arm) that
sweep will find more, and the same "is this really the desired
experience?" test from the header must be applied to each hit before anything is ported.
