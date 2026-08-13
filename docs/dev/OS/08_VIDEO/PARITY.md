# Desktop parity: the complete gate audit (x86 `wc` ⇄ Pi 4 `pidesk`)

**Peter's ruling, and the premise of this document: ONE OS.** The crispy desktop experience is not an
x86 feature that the Pi approximates — it is *the* experience, and an `x86_64`-only gate around any
part of it is a defect until it is either ported or justified as hardware-specific in writing.

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
| `wm.rs` ×13 | **WPACE-PANEL present pacer** — `PACE_FRAME_US`, `PACE_LAST_CYC`, `PACE_PENDING`, `pace_frame_cycles`, `pace_admit`, `pace_service`, and the 5 decision sites in `present_banded` | **(b) → PORTED** | **exec-vugparity (this arc).** See §5. |
| `wm.rs` ×1 | `wpace_emit` — the `[wpace]` ledger line | **(b) → PORTED** | **exec-vugparity (this arc).** |
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
  must have its own gate ported alongside it or the build breaks. This arc hit exactly that (§5).
* The **launch-stall chunking** arc (`e855655c` / `24ac6b79`, "the ~1.26 s launch freeze") was a fix to
  the WC-D *witness*, not to the compositor: the freeze was the witness's un-chunked glass read-back.
  With `witness` off there is no freeze to fix, and WC-D is x86-only regardless. **Not an experience
  gap; nothing to port.** This is recorded here specifically so it is not re-opened as one.

---

## 5. What the `exec-vugparity` arc ported

### 5.1 The vug present pacer — WPACE-PANEL (the headline)

**The gap, in one sentence: on the Pi every ring-3 vug present composited the whole panel, at present
rate rather than at panel rate.** x86 gained a panel-locked pacer that admits one composite per window
per 16.67 ms frame and coalesces the rest, with a tail drain guaranteeing a stopped stream's final
frame still reaches glass within a frame; on aarch64 all 13 of its gates read `all(x86_64, "wc")`, so
the Pi ran the pre-pacer path — a six-vug fleet compositing at its own present rate, which is the
single largest difference in how vugs *feel* between the two machines.

The pacer's arithmetic was already arch-neutral. Ported as `any(all(x86_64, wc), all(aarch64, pidesk))`:

* `PACE_FRAME_US`, `PACE_LAST_CYC`, `PACE_PENDING`, the `MAX_WINDOWS <= 32` static assert,
  `pace_frame_cycles`, `pace_admit`, `pace_service`, and the five decision points in `present_banded`
  (`pace_exempt`, its assignment, `pace_go`, the witness `declared` pair, the coalesce return).
* The `WPACE_PACED` / `WPACE_COALESCED` / `WPACE_TAIL` counters and `wpace_emit`, so the Pi prints the
  same `[wpace] win=… paced=… coalesced=… tail=… frame_us=…` ledger the x86 capture is read against.

**One line was genuinely arch-specific** and is the reason the rest ported unchanged — the frame *rate*:

| | timebase (`arch::now_cycles`) | rate | frame |
|---|---|---|---|
| x86_64 | rdtsc | `apic::tsc_hz()` — `0` until `apic::calibrate` (~116 ms) | `hz / 60` |
| aarch64 | `CNTVCT_EL0` | `timer::cntfrq()` — architectural, readable from reset | `hz / 60` |

`cntfrq` is CNTVCT's own rate, so the pair is exact on the Pi exactly as rdtsc/`tsc_hz` is on x86
(~54 MHz on Pi 4, ~62.5 MHz under QEMU). The `hz == 0` degraded arm is unreachable on aarch64 but
stays: "drain immediately, never coalesce" is the correct answer to a rate we cannot trust, on either
arch. Notably **not** `arch::ms()`, which is derived and coarser than the frame it would be pacing.

**The tail drain's placement** is the other decision. On x86 it rides `wcx::desktop_app_service`; the
Pi has no `wcx` (that module *is* the x86 panel path), so it rides the structural twin — the
device-service lane — at **both** of that lane's entry points, on the same rule PIUSB-23 already
states for `pump_usb_into_gui`:

* `usb_pump` (PIUSB-26), `sleep_ticks(1)` ≈ 4 ms — metal only.
* the `input_service` poll-nap branch — **QEMU raspi4b, where `usb_pump` is never spawned.** A drain
  that rode only the metal task would leave a stopped stream's last frame coalesced in exactly the
  configuration the regression suite boots.

4 ms is coarser than x86's ~1 ms and still correct: the drain bounds a coalesced frame's wait, and
4 ms sits well inside the 16.67 ms frame being bounded. It is deliberately **not** on the render
service — that task blocks on `GUI_CHANNEL` and is therefore off the run queue during precisely the
input-idle stretch a stopped present stream leaves behind. The pump core is not the render core, so
the drain's composite cannot preempt-deadlock against a raw-spinlock holder.

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

### 5.3 Byte-identity

Every aarch64 addition sits behind `pidesk`, which is default-OFF. With the knob off, not one of these
bodies or call sites is compiled and `kernel8.img` is byte-identical to baseline — the discipline the
`pidesk` feature was defined with, kept by construction rather than by assertion.

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
