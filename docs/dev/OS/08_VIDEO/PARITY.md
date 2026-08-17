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
| `main.rs` ×2 | `desktop_owns_backdrop` (SHELLNOTDESK) — the crispy scene as the desktop layer, shell demoted to plumbing | (b) | **OWED** — §6.1. Adjacent to exec-shellport; coordinate before starting. |
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

## 6. OWED — class (b) gaps this arc did not take

Each is scoped to one line so the next session can pick it up without re-deriving it. **Claim a row
here before starting; strike it when it lands.**

| # | Gap | Sites | Scope |
|---|---|---|---|
| 6.1 | **Desktop backdrop layer (SHELLNOTDESK)** | `main.rs` ×2 (`desktop_owns_backdrop`), `screen.rs:669` (`paint_desktop_scene`) | The Pi's `render_service` still paints the text shell as the backdrop; x86 paints the crispy scene and demotes the shell to plumbing. `Screen`/`fill_screen`/`mark_full` are all arch-neutral — the work is the trigger (x86's is `wcx::activate`; the Pi needs the `pidesk` panel bring-up) and the render-service seam. **Coordinate with exec-shellport before starting.** |
| 6.2 | **Windows do not respect menubar / dock / open dropdown** | `wm.rs:12059` (`OccClip::push`), `wm.rs:12281` (`occ_clip`), `wm.rs:12605` (`erase_clip`), `screen.rs:1050`, + 11 witness legs in §3 | On aarch64 the clip is structurally `OccClip::none`, so a window blit paints **over** the menu bar and dock and a deferred erase publishes over them. `pidesk` has already put those strips on the Pi's glass, so **the exposure is live today.** This is the same defect class x86 fixed. Largest owed item; its own arc. |
| 6.3 | **Quiet boot screen** | `fbcon.rs` ×9 (`QUIET-PANEL` `_print` suppression, `PANEL_MUTE_TAGS`, `TAG_SNIFF` + impls, `PANIC_MIRROR`) | x86 paints milestone lines only and mutes `[wc-g]/[wc-h]/[wc-d]/[wcn]` telemetry from the glass, with `PANIC_MIRROR` as the panic override; the Pi mirrors the raw serial stream across the boot panel. All arch-neutral policy over `_print`. Self-contained. |
| 6.4 | **Dock tile count for the blit clip** | `wm.rs` `dock_tiles` | `video::dock` is already on the Pi via `pidesk`, but its tile count feeding `occ_clip` is not. Small; **do it with 6.2**, which is the consumer. |
| 6.5 | **Fast glyph painting** | `fbcon.rs:154` (`draw_glyph` encode4 path), `framebuffer.rs:268` (`put_raw4`) | x86 hoists the pixel-format decode out of the 8×8 bit loop and pokes pre-encoded words; the Pi runs `put_pixel` with a per-pixel `match` on `pixel_format`. `encode4`/`word4` already exist on both arches (aarch64 uses them in `fill_span4`). Performance parity, felt as console/glyph repaint speed. |

### 6.6 THE VUG UPGRADE — the real answer to "where is the upgraded vug"

**This is the replacement for the pacer work, and it is where Peter's direction actually points.** The
trunk's vug arcs are surveyed below. The finding that matters: **the shard renderer is arch-neutral and
already links for aarch64 (7731 bytes of `.text`) — the Pi is not missing the drawing code. It is
missing the ways to REACH it, and the headroom to run it at full detail.**

| Arc | What it added | Arch status | Disposition |
|---|---|---|---|
| **VUGSCENE** `a5bd93ee` | The real drawing-complexity arc. Replaces the trivial wireframe with a solid faceted **SHARD** — an 18-half-space convex-intersection ray tracer (exact HSR, no z-buffer), orbiting light, per-facet flat shading with a specular kick, palette read from the menu bar's crystal. Plus a **ray-density LOD ladder** (lvl 0 wireframe → 3 = one ray per pixel, cost 1:4:16) that self-tunes off its own fps meter. Commit body: *"Peter: make the drawing complex, not the pacing (999fps was a trivial pattern), and the scene IS the crystal."* | **Arch-neutral.** `crates/user-vug/src/main.rs` carries no `#[cfg(target_arch)]` outside the syscall stubs; it compiles and links for aarch64. | Renderer needs no port. **The reach does** — 6.6a/6.6b/6.6c below. |
| **VUGTRUTH** `51b66a2c` | Failed presents no longer count as frames or clock the exit budget; fps on the wire as `[vugfps]`; spin budget 4096→64. The *measurement* the VUGSCENE ladder later reads. | **Arch-neutral**, gates were run green on both arches. | **AT PARITY. Not a gap.** Added no rendered content. Recorded so it is not re-opened. |
| **VUGSPREAD** `b30d81f3` | Scheduler repair for vug's 19 fps: unstealable spawned threads, load-blind sibling pick, a steal floor that could not see 2-on-1 packing. Adds per-task `migrations`, a placement hint, per-victim `steal_floor`, `cr3_live` shadow, `[spread]` witness, and an escalating steal cooldown. Renders no pixels. | **x86-only by directory** — every line is in `arch/x86_64/sched.rs` (~50 `VUGSPREAD` tags). `arch/aarch64/sched.rs` has none of it. | **OWED — 6.6c.** The Pi gets none of the placement/steal repair. |
| **LAUNCHVUG** `fa439109` | Bare `vug` at the shell resolves `VUG.ELF` on the volume executables live on and launches it **detached**. Fixed Peter's bench report *"still no way to launch vug"*. | **x86-only, explicitly.** `shell.rs:5007` `#[cfg(target_arch = "x86_64")] fn bare_exec`, and `shell.rs:2509` `exec: cfg!(target_arch = "x86_64")`. | **OWED — 6.6a. Highest priority.** |

| # | Gap | Sites | Scope |
|---|---|---|---|
| 6.6a | **`vug` does not launch on the Pi** | `shell.rs:5007` (`bare_exec`, `#[cfg(target_arch = "x86_64")]`), `shell.rs:2509` (`Facts::exec = cfg!(x86_64)`), `shell.rs:2899` (the `Plan::Exec` arm, whose `not(x86_64)` branch prints *"Unknown command."*) | Typing `vug` on the Pi prints **"Unknown command."** — verbatim the failure Peter reported at the bench on x86, still shipping on aarch64. The operator must type `bg /fat/VUG.ELF`. **This is also the gate on the shard itself:** VUGSCENE renders only when `overlay = detached \|\| interactive`, and `detached` is bit 0 of the info-page flags set by `bg` — so the launch path *is* the drawing path. `spawn_user_image_bg` already exists on aarch64 (`arch/aarch64/syscall.rs:8145`); only the resolver/dispatch half is missing, and the `not(x86_64)` arm was left in place precisely so "the compiler points here" when a loader arrives. **Smallest change with the largest visible payoff. Start here.** |
| 6.6b | **Pi media stages only one vug image** | `arroyo:2344-2359` (`build_user_aarch64` builds one unfeatured `VUG.ELF`), `arroyo:2876` (stages that one file); cf. `arroyo:1034-1079` `build_one_vug_x86` emitting three | x86 media carries `VUG.ELF` (adaptive), `VUGC.ELF` (`pinlo`, classic baseline) and `VUGX.ELF` (`pinhi`, full per-pixel shard). **Pi media has neither `VUGC.ELF` nor `VUGX.ELF`**, so the pinned-detail images — the ones that show the shard at full density and give the A/B baseline — cannot be run on the Pi at all. Build-plumbing only; the crate already builds for aarch64 under both feature pins. |
| 6.6c | **No scheduler placement/steal repair on the Pi** | all of `arch/x86_64/sched.rs`'s VUGSPREAD work; aarch64 twin absent | The LOD ladder self-tunes off achieved fps, so **less CPU headroom settles the shard at a LOWER detail rung — the Pi literally draws a simpler scene.** This is the deepest link to Peter's "more drawing complexity" direction: the Pi's shard is dimmer because its scheduler is worse, not because its renderer is. Large; its own arc; needs a scheduler-lane owner, not a video-lane one. |
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

### Class (b) rows owned by arcs already in flight — do not duplicate

| Gap | Sites | Owner |
|---|---|---|
| Title-bar window **dragging** — `wm::drag_begin/motion/end` are arch-neutral but called only from `arch/x86_64/syscall.rs` | `main.rs:1819` (`wc_route_tail`) | **exec-dragperf** |
| **Click-to-focus / raise / close-button** on windows — aarch64's `click1_dispatch` hit-tests only console-vs-status-strip | `main.rs:1693` (`wc_route_event`) | **exec-dragperf** / **exec-conswin** — settle ownership before starting |
| Console window, its route/pace machinery, furniture strips, menubar reservation | `fbcon.rs` ×42, `screen.rs:367`, `ui_status.rs:597`, `wm.rs:16493` | **exec-conswin** |
| Shell window | `main.rs` `open_shell_window` | **exec-shellport** |

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
