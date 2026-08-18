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
| `wm.rs` ×1 | `dock_tiles` — the dock's tile count feeding `occ_clip` | (b) | **OWED** — §6.4. |
| `wm.rs` ×1 (16493) | MENUFIT witness — menubar reservation line | (d) IN FLIGHT | **exec-conswin** (menubar). |
| `fbcon.rs` ×42 | **The console window** — `CONSOLE_WIN`, the route/pace/pending machinery (`ROUTE_BUSY`, `PACE_HZ`, `pend_merge`, `route_present*`, `console_service`, `console_flush`), the window-backed console store (`win_store`, `win_fb`, `win_content_extent`), `panel_console_window_open` / `_closed` | (d) IN FLIGHT | **exec-conswin.** Not touched by this arc. |
| `main.rs` ×1 | `open_shell_window` | (d) IN FLIGHT | **exec-shellport.** |
| `main.rs` ×2 | `desktop_owns_backdrop` (SHELLNOTDESK) — the crispy scene as the desktop layer, shell demoted to plumbing | (b) | **CLOSED** — §6.1. SHELLWIN-PI (`b2e0fb4a`) put the live shell in a window; REALDESK (exec-realdesk) took the last two backdrop tenants — the pulse band and the status line — off the glass with it. |
| `main.rs` ×2 | `instgui::service`, `instgui::consume_key` | (c) | `instgui` is the x86 installer GUI on the `wcx` panel path. |
| `mod.rs` ×2 | `pub mod wcx;`, `pub mod instgui;` | (c) | `wcx` **is** the x86 panel path (ignited by the Kepler takeover); `instgui` rides it. |
| `screen.rs` ×1 (1050) | Occluder-array assembly for the blit clip | (b) | Part of the occlusion family — **OWED**, §6.2. |
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
| Occlusion *witness legs* | `crystal.rs` ×1, `menubar.rs` ×1, `wm.rs` ×1, `wcg.rs` ×8 | These probe `wm::occ_clip`; they become meaningful only once §6.2 lands, and follow it rather than lead it. |

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

On x86 this is latent because `wm::occ_clip` withholds the bar's columns from every window blit. **On
aarch64 `occ_clip` is `#[cfg(target_arch = "x86_64")]` and returns `OccClip::none()`** — §6.2, still
owed — so on the Pi nothing protects the strips *and* nothing repaired them. The bar and the open
dropdown now both ask `dock_scan`'s clobber question against the rect they last painted and repaint on
a yes: `dock::compose`'s WCK5 condition, generalised to tenants #2 and #3. `clob=` is on both ledger
lines as the falsifier.

**This is a repair, not the protection.** §6.2 remains owed and remains the correct fix — a clobber
repaint still costs one frame with a window's pixels standing in the bar. Recorded here so a later
session does not read `clob=` as evidence that §6.2 has been closed. **`clob=0` on every QEMU surface
run this arc**, at 640×480 and at bench geometry: the repair is structural and its firing is
metal-owed.

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
| 6.1 | **Desktop backdrop layer (SHELLNOTDESK)** — **CLOSED**, REALDESK 2026-08-17 | `main.rs` (the mint arm), `ui_status.rs` ×3, `video/mod.rs` (the latch) | Landed in three steps. **PIDESK-CLEAR** `8f78399d` gave the Pi the panel-wide `DESKTOP_BG` clear. **SHELLWIN-PI** `b2e0fb4a` put the live text shell in a window and claimed the backdrop in the same pass, which retired the *shell* tenant. **REALDESK** retires the two that were left: `ui_status`'s pulse LED band and its host/ip/UTC status line, both of which wrote the desktop back buffer directly. See §6.1a for the full tenancy ledger and what the retirement does NOT cover. |
| 6.2 | **Windows do not respect menubar / dock / open dropdown** | `wm.rs:12059` (`OccClip::push`), `wm.rs:12281` (`occ_clip`), `wm.rs:12605` (`erase_clip`), `screen.rs:1050`, + 11 witness legs in §3 | On aarch64 the clip is structurally `OccClip::none`, so a window blit paints **over** the menu bar and dock and a deferred erase publishes over them. `pidesk` has already put those strips on the Pi's glass, so **the exposure is live today.** This is the same defect class x86 fixed. Largest owed item; its own arc. **STILL OWED after `exec-crystalpi`** — that arc added the *repair* (§5.5c: the bar and the dropdown now notice a clobber and repaint the same pass) but not the *protection*, so the exposure is bounded to one frame rather than removed. |
| 6.3 | **Quiet boot screen** | `fbcon.rs` ×9 (`QUIET-PANEL` `_print` suppression, `PANEL_MUTE_TAGS`, `TAG_SNIFF` + impls, `PANIC_MIRROR`) | x86 paints milestone lines only and mutes `[wc-g]/[wc-h]/[wc-d]/[wcn]` telemetry from the glass, with `PANIC_MIRROR` as the panic override; the Pi mirrors the raw serial stream across the boot panel. All arch-neutral policy over `_print`. Self-contained. |
| 6.4 | **Dock tile count for the blit clip** | `wm.rs` `dock_tiles` | `video::dock` is already on the Pi via `pidesk`, but its tile count feeding `occ_clip` is not. Small; **do it with 6.2**, which is the consumer. |
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
| **LAUNCHVUG** `fa439109` | Bare `vug` at the shell resolves `VUG.ELF` on the volume executables live on and launches it **detached**. Fixed Peter's bench report *"still no way to launch vug"*. | **x86-only, explicitly.** `shell.rs:5007` `#[cfg(target_arch = "x86_64")] fn bare_exec`, and `shell.rs:2509` `exec: cfg!(target_arch = "x86_64")`. | **OWED — 6.6a. Highest priority.** |

| # | Gap | Sites | Scope |
|---|---|---|---|
| 6.6a | **`vug` does not launch on the Pi** | `shell.rs:5007` (`bare_exec`, `#[cfg(target_arch = "x86_64")]`), `shell.rs:2509` (`Facts::exec = cfg!(x86_64)`), `shell.rs:2899` (the `Plan::Exec` arm, whose `not(x86_64)` branch prints *"Unknown command."*) | Typing `vug` on the Pi prints **"Unknown command."** — verbatim the failure Peter reported at the bench on x86, still shipping on aarch64. The operator must type `bg /fat/VUG.ELF`. **This is also the gate on the shard itself:** VUGSCENE renders only when `overlay = detached \|\| interactive`, and `detached` is bit 0 of the info-page flags set by `bg` — so the launch path *is* the drawing path. `spawn_user_image_bg` already exists on aarch64 (`arch/aarch64/syscall.rs:8145`); only the resolver/dispatch half is missing, and the `not(x86_64)` arm was left in place precisely so "the compiler points here" when a loader arrives. **Smallest change with the largest visible payoff. Start here.** |
| 6.6b | **Pi media stages only one vug image** | `arroyo:2344-2359` (`build_user_aarch64` builds one unfeatured `VUG.ELF`), `arroyo:2876` (stages that one file); cf. `arroyo:1034-1079` `build_one_vug_x86` emitting three | x86 media carries `VUG.ELF` (adaptive), `VUGC.ELF` (`pinlo`, classic baseline) and `VUGX.ELF` (`pinhi`, full per-pixel shard). **Pi media has neither `VUGC.ELF` nor `VUGX.ELF`**, so the pinned-detail images — the ones that show the shard at full density and give the A/B baseline — cannot be run on the Pi at all. Build-plumbing only; the crate already builds for aarch64 under both feature pins. |
| 6.6c | ~~**No scheduler placement/steal repair on the Pi**~~ **LANDED — `exec-vugspread`, see §6.7** | `sched_spread.rs` (new, shared); `arch/aarch64/sched.rs`; `arch/x86_64/sched.rs` (delegation only) | The LOD ladder self-tunes off achieved fps, so **less CPU headroom settles the shard at a LOWER detail rung — the Pi literally draws a simpler scene.** The row was written as "placement AND steal"; §6.7 shows the Pi already had a deeper PLACEMENT lattice than x86 (SPREAD-3..14), and what it lacked was the STEAL half — the only correction that can reach a thread which never parks. That half is now ported. |
| 6.6d | ~~**No damage-present on the Pi**~~ — **CLOSED**, `exec-presentrows` | `SYS_WIN_PRESENT_ROWS` (33) dispatched at `arch/x86_64/syscall.rs:2594`; now also at `arch/aarch64/syscall.rs` (`sys_win_present_rows`, beside `sys_win_present`) | The Pi answered `-ENOSYS` and `user-vug` fell back to a whole-box present. Ported: same number, same argument shape, same errnos in the same order, band range-checked against the presenting row's own `h` under the hold that proved ownership. See §6.6d-closed below for the correction the port forced on this row's own framing. |

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
Pi feels it more only because 6.6a denies it the easy `bg` path). `user-pulse` is fully at parity: its
drawing is common code and `arroyo:2885` already stages `PULSE.ELF` on Pi media.

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
| Console window, its route/pace machinery, furniture strips, menubar reservation | `fbcon.rs` ×42, `screen.rs:367`, `ui_status.rs:597`, `wm.rs:16493` | **exec-conswin** |
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
single most-used launch verb) that sweep will find more, and the same "is this really the desired
experience?" test from the header must be applied to each hit before anything is ported.
