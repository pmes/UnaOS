# Kernel de-crud — audit and strip ledger

**Scope.** Ring 0 only (`unaos/crates/kernel`). This document enumerates code that lives in the
kernel but is not kernel business, classifies it, and records what was removed or gated versus what
was deliberately deferred and why.

**Standing rule this document serves.** `arroyo`'s DEFAULT-QUIET law already says that a witness
family must be behind a knob, default OFF, so that a default boot reaches the shell with the
boot-honesty lines only. This audit applies the same question to code that is not a witness at all:
*what does a default image carry that no default boot can reach?*

**Method.** Every entry below was derived from the tree at `14e54538` by call-graph tracing plus a
`kernel8-test` capture at both QEMU geometry (640x480, no `pidesk`) and bench geometry
(`UNAOS_PIDESK=1 UNAOS_FBW=1920 UNAOS_FBH=1200`). Where an entry makes a claim about what a boot
does or does not do, the claim is from a capture, not from reading the source.

---

## 1. What the x86 / `UnaOS-gemini` side already did

This work was started on the x86 trunk, and the results are already merged into this track's base.
Recording them here so the next reader does not re-derive them, and because two of them are the
precedent the strips in §3 follow.

| sha | What it removed | Notes |
|---|---|---|
| `ee6bfd97` | **`kernel: delete the in-kernel vug`** — the whole 1390-line `vug.rs` on x86: crystal, bebox, the full-screen pulse demo, the `[vugfps]`/`[vugfps4]` emitter, and the `vug`/`pulse` shell verbs. | The direct precedent for §3.1. What did NOT die: `METER_DIM`/`METER_BREATH`/`METER_PARKED`, `PARKED`, `classify_load_scaled` and `parked_display_witness`, which moved to `ui_status.rs` because the always-on strip is their real consumer. |
| `77e39d05` / `ba62e779` | **the desktop's second window stops being 110 lines of kernel** — the x86 demo window became a ring-3 program, `STAT.ELF`, auto-launched. | The shape this audit recommends generally: a demo that wants a window is a program, not kernel code. |
| `bfa2c174` | stripped orphaned x86-wc call sites (`wm::present_rows`, `screen::adopt_desktop_bg`) left behind by a merge. | Dead-call-site class. |
| `dcb19baa` | removed an always-on wedge exposure from a scheduler witness, and retired an inference that was being printed as a measurement. | Not a strip, but the instrument-honesty rule the fixture work in §3.3 follows. |
| `864df40f`, `8510168c` | `igpu` dead `start_head`, a stale revert engine excised on review. | Dead-code class, x86 lane. |

**Note on the surviving justification.** `lib.rs` carried a comment claiming the Pi kept `vug.rs`
because *"`run_bsp` is why user vugs run"*. That claim was already retracted in-tree before this
arc: `run_bsp` appears nowhere in `vug.rs` (it is in `arch/{aarch64,x86_64}/sched.rs`), and the EL0
vug is the `VUG.ELF` vessel, not the module. Only the classifier half of the claim was ever true,
and it points at `ui_status`. §3.1 acts on that retraction.

---

## 2. Audit

Sizes are lines of source. "Default boot" means no environment knobs: `./arroyo kernel8` for the Pi,
`esp-x86`/`vm-image` for x86. The `*-test` batteries auto-arm `UNAOS_WITNESS=1` and are therefore
NOT default boots.

### 2.1 Class (a) — demo / showcase code in Ring 0

| Item | File:line | Size | On a default boot? | Verdict |
|---|---|---|---|---|
| `vug.rs` — crystal sculptor, `run_pulse`, `run_bebox_mode`, the fixed-point `Fx`/`Vec3`/`shade` math, `CpuPulse`, `draw_meters`, `drain_game_input` | `crates/kernel/src/vug.rs:46-222`, `251-1319` | **~1292** | Linked into every aarch64 image; reachable only by an operator typing `vug` / `vug wire` / `vug bebox` / `pulse` | **GATED — §3.1.** EL0 replacements ship in the same image. |
| `vug.rs` — `classify_load`, `parked_display_witness` | `vug.rs:223-249` | 27 | `parked_display_witness` runs on the `virt` CAPSTONE boot | **KEPT, MOVED — §3.1.** Metal-earned falsifier; not a demo. |
| `rast_demo.rs` — software-rasterizer spinning cube, plus its three wire-ins | `crates/kernel/src/rast_demo.rs:1-186`; `main.rs:1470-1474`, `4946-4966`, `4996-5041` | 186 + ~65 | **No.** `feature = "rast"` / `pirast`, `UNAOS_RAST` / `UNAOS_PIRAST` | **NO ACTION — already correct.** Knob-off the whole `rast` graph is unlinked and the image is byte-identical to baseline. This is the model the rest of the class should match. |
| `splash.rs` — ray-marched crystal-cluster boot splash, per-wavelength Snell's-law refraction | `crates/kernel/src/splash.rs` (whole file); call `main.rs:232-234` | 642 | **YES on x86** — gated only `not(any(usbdebug, bootlog, witness))` | **DEFERRED — §4.1.** x86 lane, and a product decision, not a code decision. |
| x86 SMP scheduler demo — `start_demo`, `demo_rw_*`, `demo_ageref_*`, `demo_cvcap_*`, `demo_jt_*`, `demo_done` and their statics | `crates/kernel/src/arch/x86_64/sched.rs:6093-6612` | **520** | The *spawn* is gated (`sched_demo`, `main.rs:946-950`); the **definitions are not**, and `shell.rs:3895` calls `demo_done()` ungated | **DEFERRED — §4.2.** x86 lane. The knob does not unlink what it claims to. |
| aarch64 `demo_cooperative` — 3 tasks x 3 rounds on the boot core | `crates/kernel/src/arch/aarch64/sched.rs:5977-6019` | 43 | **YES, every default Pi boot** | **KEEP — §4.3.** Misnamed. It is the workload that makes two witnesses non-degenerate. |
| `video/crystal.rs` | `crates/kernel/src/video/crystal.rs` | 960 | Yes, with `pidesk` | **NOT CRUD.** Despite the name this is the SHARD *system menu* — kernel-owned live UI with real callers in `wm.rs`, `screen.rs:1181`, `menubar.rs:273` and both `syscall.rs`. Named here only so the next audit does not flag it again. |

No `starfield`, `mandelbrot`, `plasma`, `screensaver` or bouncing-logo code exists in the kernel
crate. The only `tribute`/`homage` code is `run_bebox_mode` and `run_pulse`, both covered above.

### 2.2 Class (b) — witness fixtures that create user-visible artifacts

**The headline finding is a negative one, and it corrects a standing belief.**

The two kernel-band fixture windows that were believed to ride every desktop boot — `closeiso`'s
`ci-k` (`KERNEL_OWNER_CONSOLE`) and `ctrldecline`'s `cd-f` (`KERNEL_OWNER_BASE + 0x40`) — **do not
leak on this tree.** Both are reaped. This was fixed by two arcs already in the baseline:
`53cd29c0` (`wm: closing a window closes exactly that window`) added `closeiso`'s `close(wk)`, and
`dc301080` (`wm: review conditions on the controls-declined witness`) turned `ctrldecline`'s reap
into an asserted verdict bit.

Evidence, from a bench-geometry `pidesk` capture (`UNAOS_PIDESK=1 UNAOS_FBW=1920 UNAOS_FBH=1200`):

```
[wc-a] create win=2 asid=0xffffff01 surf=160x8 stride=640 scale=4x at (17,85) z=19     <- ci-k
[wc-a] close_owner asid=0xffffff01 REFUSED furniture rows=2 ids=[1, 2] — KERNEL FURNITURE IS NOT CLOSABLE
[wc-a] close win=2                                                                     <- ci-k reaped
...
[wc-a] create win=4 asid=0xffffff40 surf=160x8 stride=640 scale=4x at (17,173) z=44    <- cd-f
[wc-a] close win=4                                                                     <- cd-f reaped
:: WMCTRL: controls-declined — … reaped=true :: PASS ::
[wc-a] composite windows=1 drawn=1                                                     <- only the real console
```

The boot ends with exactly one live window: the real console (`win=1`, `asid=0xffffff01`,
1296x736 — the CONSWIN-PI console window, which is furniture and is meant to be there).

**What IS still wrong is the guard, not the behaviour.** Of the nine window-minting fixtures in
`wm.rs`, only two asserted their own teardown before this arc. The rest reap correctly today and
would leak *silently* if they ever stopped. `closeiso_selftest` was the worst of them because it is
the one that mints a **kernel-band** row: `close_owner` refuses that band by design (which is the
contract `closeiso` itself proves), so a failed reap there leaves a row on a desktop boot that no
gesture can remove for the rest of the boot — and nothing on the wire would have said so. Fixed in
§3.3.

| Fixture | File:line | Size | Windows | Teardown asserted? |
|---|---|---|---|---|
| `closeiso_selftest` | `video/wm.rs:16209-16366` | 158 | `ci-k` (**kernel band**), `ci-a`, `ci-f` | **was: no** → **§3.3: yes** |
| `ctrldecline_selftest` | `video/wm.rs:18158-18373` | 216 | `cd-n`, `cd-w`, `cd-f` (**kernel band**) | yes (`reaped=`, leg 6) |
| `hittest_selftest` | `video/wm.rs:17701-17918` | 218 | `ht-a`, `ht-b`, `ht-s`, `ht-x` | yes (`wm.rs:17902-17911`) |
| `focusvis_selftest` | `video/wm.rs:15458-15564` | 107 | `fv-a`, `fv-b` | no — balanced, unasserted |
| `reopen_selftest` | `video/wm.rs:15593-15668` | 76 | `re-a`, `re-b`, `re-c` | no |
| `vacate_selftest` | `video/wm.rs:15696-15774` | 79 | `vc-a` | no |
| `movevacate_selftest` | `video/wm.rs:15815-15932` | 118 | `mv-a` | no |
| `retile_selftest` | `video/wm.rs:16391-16453` | 63 | `rt-a`, `rt-b` | no |
| `dragperf_selftest` | `video/wm.rs:16023-16206` | 184 | `dgp` | no |

Two structural notes for whoever picks up §4.4:

* **The gate list under-counts the fixtures that run by three.** `closeiso_selftest`,
  `retile_selftest` and `movevacate_selftest` are not named at the `wcb_launcher` call site at all —
  they are reached three frames deep, via `reopen_selftest` → `vacate_selftest` → them
  (`wm.rs:15667`, `15770-15772`). Auditing the launcher's `#[cfg]` list alone misses them.
  `menubar::selftest` is nested inside `dock::selftest` (`video/dock.rs:953-971`) for the same
  reason, with an in-tree `⚠ FOR THE INTEGRATOR` note already asking for it to be re-seated.
* **On the Pi there is no non-fixture window source.** `video/pidesk.rs:54-56` states it outright:
  the Pi's window population comes from the `u11` fixture cascade and the ring-3 vug loader. There is
  no `DESKTOP_APP_ARMED` equivalent. So on a `pidesk` + `witness` boot every window on the glass
  except the console is fixture-minted, and a leak is indistinguishable from normal furniture by
  eye. That is why the reap has to be asserted on the wire rather than judged from a photograph.

### 2.3 Class (c) — Ring 0 code duplicating userland

| Item | Kernel copy | Ring-3 equivalent | Verdict |
|---|---|---|---|
| The vug renderer and pulse monitor | `crates/kernel/src/vug.rs` (~1292 lines of demo) | `crates/user-vug/src/main.rs` (2674 lines) → `VUG.ELF`; `crates/user-pulse/src/main.rs` (1142 lines) → `PULSE.ELF` | **The clearest case in the tree.** The ring-3 programs re-derive `crystal_vertices`, `fsin`/`fcos`/`fmul`, `drain_input` and `draw_pulse_bar` outright, and `user-pulse` restates `classify_load`'s honesty rule in ring 3. Both are built for aarch64 and staged into the FAT root of **every kernel8 image** — `arroyo` builds them in `build_user_aarch64()`. The bench runs the ring-3 ones: `UVUG:` streams while the in-kernel `[vugfps]` counter reads 0. **GATED — §3.1.** |

### 2.4 Class (d) — dead knobs and dead arms

| Item | File:line | Size | Verdict |
|---|---|---|---|
| `#[cfg(test)] mod tests` in `gui_watchdog.rs` | `crates/kernel/src/gui_watchdog.rs:162-190` | 28 | **DEAD BY CONSTRUCTION — STRIPPED, §3.2.** Nothing runs `cargo test` on this `no_std` kernel crate; `./arroyo check` is the gate and `#[cfg(test)]` is invisible to it. `drivers/gpu/kepler.rs:3900-3906` removed the identical construct for exactly this reason and recorded the rule; this one survived that sweep. |
| `UNAOS_GIT_SHA` | read only at `arch/aarch64/genet.rs:2572` | — | **Effectively dead on every default build.** It is exported by `arroyo:50` for compile-time embedding, but its sole reader sits inside `genet`, which is default-OFF. Not removed: the export is cheap and the intent (a build ID in the image) is right; the *reader* is in the wrong place. §4.5. |
| `UNAOS_NET4_DHCP_MS` | read at `arch/aarch64/rtl8168_tegra.rs:1366` | — | **Undocumented knob** — absent from `arroyo` entirely. Works only by env inheritance and is invisible to anyone reading `arroyo`. Jetson lane. §4.5. |
| `noportsw` | `arch/x86_64/pci.rs:107, 900` | — | `arroyo:190` itself calls it a *"never-run no-routing experiment"*. x86 lane. §4.5. |
| `wcg-paygo` | `video/wm.rs:1768` and `video/wcg.rs` | — | Conditionally dead: reaches nothing without `witness`, so `UNAOS_WCG_PAYGO=1` alone is a byte-identical no-op. Documented in `arroyo:124`; not a defect. |
| `sdhc::write_selftest` `armed` arm; `memory::wxn_ro_stage` | `drivers/sdhc.rs:3166`; `arch/x86_64/memory.rs:3169` | — | `cfg!` (not `#[cfg]`) dead arms **by design** — both print an honest "not armed" census line rather than vanishing. Both files document it. No action. |
| Zero-caller functions | `drivers/ehci/mod.rs:3119` `live_port_smoke` (~25L, the largest); `gui_watchdog.rs:129` `is_app_active`; `arch/x86_64/sched.rs:3654` `futex_dup_count`; `arch/x86_64/syscall.rs:4998` `user_input_stats`; `drivers/sdhc.rs:1003` `card_block_addressed`; `install/mod.rs:95` `sector_size` (dead trait default) | ~40 total | **DEFERRED — §4.5.** All in foreign lanes (x86 / EHCI / install). Each is small; the sweep should be one arc, not six. |

**Cargo features: no dead knobs.** All 100 features declared in `crates/kernel/Cargo.toml` have at
least one reader in `crates/kernel/src` and are named by `arroyo` and/or `builder/src/main.rs`. The
cross-arch scan is also clean: all 13 features `arm_features` strips (`arroyo:699-721`) have readers
only in x86-only modules.

### 2.5 One finding outside the four classes, flagged because it is the sharpest

`drivers/xhci/mod.rs:10314-10461` — `mission_write_selftest` (148 lines) has **no `cfg` gate
anywhere in its call chain**. It is called unconditionally from `service_storage()`
(`mod.rs:10227`), which is called from `main.rs:1083, 1519, 3200, 4210`, all ungated. It performs a
read-modify-write-restore of one scratch sector **on the operator's USB stick**, on every default
x86 boot that enumerates USB mass storage. It is the only fixture in the tree that mutates user
media with no knob in front of it. x86/storage lane — reported, not touched. §4.6.

---

## 3. What this arc stripped

Every change below is behaviour-neutral on a default boot; §5 records the gate evidence.

### 3.1 DECRUD-1 — the in-kernel vug/pulse/bebox demos move behind a knob, default OFF

`vug.rs` is now `#[cfg(all(target_arch = "aarch64", feature = "vugdemo"))]`, with `UNAOS_VUGDEMO=1`
arming it through `arroyo`'s curated `K8_FEATS` block. `shell.rs` gates its `vug` and `pulse` verb
arms on the same feature, and `took_screen` gained the matching `cfg!(feature = "vugdemo")` term so
that knob-off the words stop claiming the screen — the rule that comment already stated for x86,
now true on the Pi for the same reason.

The argument for OFF being the default, in the order that matters:

1. **Nothing on any boot path calls into the module, on either arch.** The only entry is an operator
   typing at the console. A default `kernel8.img` was carrying ~1.3 kloc of Ring-0 software renderer
   that nothing on the machine could reach without a keystroke.
2. **The replacements ship in the same image.** `VUG.ELF` and `PULSE.ELF` are built for aarch64 and
   staged into the FAT root of every kernel8 image. The bench already runs those.
3. **The stated reason for keeping it was already retracted in-tree** — see §1.
4. Gated rather than deleted, because deletion also retires three shell verbs and ~14 documents
   reference them. That is a product call, not a cleanup call. See §4.7.

**The one piece that did NOT get gated.** `classify_load` and `parked_display_witness` moved to
`ui_status.rs` first. `parked_display_witness` is a metal-earned falsifier for the VUG-HONESTY
display rule, wired into the `virt` CAPSTONE boot; leaving it inside a demo module would have made a
live witness hostage to a demo's knob. `ui_status` is ungated, compiles on both arches, and already
owned the rule (`classify_load_scaled`), so the decision still has exactly one definition.
`arch/aarch64/sched.rs:7509` repoints to `crate::ui_status::parked_display_witness()`.

### 3.2 DECRUD-2 — the dead `#[cfg(test)]` block becomes compile-time assertions

`gui_watchdog.rs`'s four `#[test]` fns had never been compiled on either arch, for the reason
`kepler.rs` already recorded. **The coverage moved up rather than out:** `wedge_decision` is now a
`const fn`, and the same four cases with the same values ride a `const _: () = { … }` block beside
it. They are const-evaluated on every `./arroyo check` for both arches, so a regression is now a
build failure instead of a test nobody runs. Net: 28 dead lines removed, four assertions gained.

### 3.3 DECRUD-3 — `closeiso_selftest` proves its own teardown

Two additions, no change to what the fixture witnesses:

* **The kernel-band row's reap is asserted.** `close(wk)` was an unchecked statement; it is now
  `reaped_k = close(wk) && info_box(wk).is_none()`, folded into the verdict and printed as
  `reaped=`. This is `ctrldecline` leg 6's contract applied to the sibling fixture that needed it
  more: `close_owner` refuses the kernel band — the very contract `closeiso` leg 3 proves — so an
  owner-scoped sweep is structurally blind to `wk`, and `close` by id is the only thing that can
  reap it.
* **A teardown sweep that speaks.** `wa` is deliberately reaped by the `close_owner(ASID_APP)`
  gesture *under test*; if that leg ever regressed to freeing zero rows, `ci-a` would reach the
  desktop and only a verdict bit would have noticed. The sweep costs two calls and turns a silent
  leak into a named FAIL. `KERNEL_OWNER_CONSOLE` is deliberately not swept: on a desktop boot it is
  the live console's own owner.

### 3.4 DECRUD-4 — UNCOVER-REPAINT: a falsifier for the hole Peter saw in the console

**Not a de-crud change — a witness this arc's own teardown work made cheap to add, for a defect
observed on metal the same day.** Peter: a fixture window opened and closed over the console window
and left an **unpainted hole** in the console beneath it, which clicking did not repair.

**Why nothing in the tree could see it.** Every close witness in `wm.rs` asks the WC-J question —
*did the closed window's own box come back as `DESKTOP_BG`?* That question is the exact complement of
the defect. A vacated box is only legitimately desktop-coloured **where nothing was under it**; where
something was, it must come back as *that window*. `reclaim` (`wm.rs:17049`) does `erase` — paint
desktop — and then `damage_intersecting`, so the repaint of the uncovered row rides entirely on the
second half. If that damage flag is set and then eaten by a composite that runs before the row is
redrawn, the erase has already landed and the hole is permanent. Clicking cannot repair it because a
click marks no damage on a row it does not move. `[wc-j] vacate` and `[wc-j] retile` pass throughout.

**The geometry says this is reachable on a real desktop boot, not a contrived case.** On the bench
capture, `ctrldecline`'s pinned `cd-f` row lands at (17,173) spanning 640x32, and the console window
is at (312,197) spanning 1296x736. Those overlap in a 345x8 strip — and `cd-f` is then closed, which
is exactly "a fixture window closed over the console window".

**The leg**, added to `closeiso_selftest` because that fixture already holds a live **kernel-band**
row (`wk` — furniture, the same class as the console window) and this arc was already in its
teardown. Three reads, printed as `uncover=` and folded into the verdict:

1. `base` — `wk`'s content origin reads `wk`'s colour with nothing over it.
2. `occluded` — the overlapper is genuinely on top. Without this read, a leg that could not occlude
   would pass read 3 trivially, which is the way this witness could otherwise convict nothing.
3. `restored` — after `close(wo)`, that same pixel is `wk`'s colour **again**, not `DESKTOP_BG`.
   Desktop there *is* the hole.

`wk` is pinned for the duration and unpinned after. Without that, `close(wo)` re-tiles the unpinned
survivors, `wk` moves out from under the hole, and read 3 passes on a broken tree — the precise
vacuity this file's siblings have twice been convicted for.

**Scope.** If this leg reds, the fix is in the compositor's damage/reclaim path, **not** in the
fixture — that is the freeze-grab executor's area, and this arc does not touch it. What this arc
contributes is the falsifier that names it on the wire.

---

## 4. Deferred — the follow-up list

These are named with enough detail to be picked up cold. None of them are code in this arc.

1. **`splash.rs` (642 lines, x86).** Runs on a default x86 boot with no dedicated knob — it is off
   only for `usbdebug`/`bootlog`/`witness` builds, i.e. off for the test media and on for the user.
   It is also the largest piece of pure showcase rendering a normal user actually boots into. If it
   is kept, it should earn its own knob rather than inherit the test-media gate. Product decision.
2. **x86 SMP scheduler demo (520 lines).** `arch/x86_64/sched.rs:6093-6612` is not `#[cfg]`-gated at
   the *definitions*, and `shell.rs:3895` calls `demo_done()` ungated from the `sched`/`ps` verb, so
   all 520 lines and their `RWL`/`RW_*`/`AGEREF_*` statics link into every x86 image regardless of
   `UNAOS_SCHED_DEMO`. Only the spawn is gated. The fix is to drop the `demo_done()` call from the
   shell verb and put `#[cfg(feature = "sched_demo")]` on the block, so the knob unlinks what it
   claims to. **rmbp lane.**
3. **`demo_cooperative` — do not strip it, rename it.** `arch/aarch64/sched.rs:5977-6019` runs on
   every default Pi boot and looks like the most obvious crud in the tree. It is not: it is the
   workload that makes `load_accounting_witness` non-degenerate, and it hosts `prio_mix_witness`,
   `load_accounting_witness` and `skill_kill_witness`. The capture is explicit —
   `:: AARCH64 SCHED: load-accounting PASS (max busy 100%, 99 ctx-switches total) ::` — and those 99
   context switches are the demo's. Removing the spawn would zero the counter the witness asserts on.
   The honest follow-up is a rename, not a strip.
4. **The remaining seven `wm.rs` fixtures should assert their teardowns**, on §3.3's pattern. None
   leaks today; all would leak silently if they regressed. Related: re-seat `closeiso`, `retile` and
   `movevacate` at the `wcb_launcher` call site so the gate list names every fixture that runs, and
   re-seat `menubar::selftest` out of `dock::selftest` per the existing in-tree integrator note.
5. **Dead-accessor sweep**, one arc: `live_port_smoke` (`drivers/ehci/mod.rs:3119`, the largest at
   ~25 lines and a full periodic-schedule manipulation with no caller), `is_app_active`,
   `futex_dup_count`, `user_input_stats`, `card_block_addressed`, `install/mod.rs:95` `sector_size`.
   Also in the same sweep: give `UNAOS_NET4_DHCP_MS` an `arroyo` block, move `UNAOS_GIT_SHA`'s reader
   somewhere a default build reaches, and retire `noportsw`. **Foreign lanes — needs the integrator.**
6. **`mission_write_selftest` needs a knob** (§2.5). It writes to the operator's USB stick on every
   default x86 boot with no gate anywhere in its call chain. **x86/storage lane.**
7. **Whether to delete `vug.rs` outright**, as `ee6bfd97` did on x86, rather than leave it gated.
   That retires the `vug` / `vug wire` / `vug bebox` / `pulse` shell verbs and creates a doc debt
   across ~14 files that reference them. Peter's call.

---

## 5. Gate evidence

| Gate | Result |
|---|---|
| `./arroyo check` (x86_64 + aarch64) | **exit 0**, both arches |
| `UNAOS_WC=1 ./arroyo check` | **exit 0** (full feature matrix, incl. `arm-pi` / `arm-tegra` / `x86-mix-0..7`) |
| `UNAOS_VUGDEMO=1 ./arroyo kernel8` | **exit 0** — the knob is not a lie: the demo still compiles when armed |
| `./arroyo kernel8-test 210` — **baseline** at `14e54538` | **MBENCH PASS — 108/108 required, 0 forbidden**, 7156 lines scanned |
| `./arroyo kernel8-test 210` — **after this arc** | **MBENCH PASS — 108/108 required, 0 forbidden**, 11080 lines scanned |

No deltas to explain: the same 108 required witnesses pass before and after, and no forbidden
pattern is hit in either. `[wc-iso]` gained two fields, `uncover=true reaped=true`, and still reads
`-> PASS`. The line-count difference between the two captures is boot-to-boot log volume, not a
change in what is asserted.

**Host-load caveat.** These runs were taken with sibling executors live on the same box — load
average 27-35 on 20 cores throughout. The `arroyo` comment on this gate warns that a loaded host can
TRUNCATE the 210 s window, which mbench reports as exit 3 (inconclusive), not as a regression.
Neither run truncated and both returned exit 0, so nothing here needs a quiet-host re-gate. Recorded
because a reader comparing timings against a quiet-host capture will find these slow.

**Reachability captures**, both at the `14e54538` baseline, establishing what a default boot does:

* **QEMU geometry** (640x480, no `pidesk`): the in-kernel vug emits **zero** lines — no `[vugfps]`,
  no `VUG-HONESTY` from the demo path, no `Vug:` — confirming §2.1's reachability claim on the wire
  rather than from the call graph. Final state: one live window.
* **Bench geometry** (`UNAOS_PIDESK=1 UNAOS_FBW=1920 UNAOS_FBH=1200`): the §2.2 capture. Both
  kernel-band fixture rows reaped; final state one live window, the real console.

**Size.** Measured on the flashable default image (`./arroyo kernel8`, no `witness`), which is the
one that matters because it is what goes on the card:

| `kernel8.img` | bytes |
|---|---|
| `UNAOS_VUGDEMO=1` (demo armed) | 1,121,088 |
| default (knob off) | **1,105,352** |
| **removed from every default image** | **15,736 (15.4 KiB)** |

For reference, the witness-armed gate image went 1,785,216 → 1,760,824 (−24,392), but that figure is
not the demo's alone — it also carries §3.3/§3.4's additions and the `witness` build's own code.

**What DECRUD-4 does and does not show.** `uncover=true` on the gate config. That is a pass, not a
refutation of Peter's metal report: the gate boot has no `pidesk`, no console window, and a different
panel scale. The leg's value is that it now exists and is folded into a verdict — the configuration
where it reds is the one that names the bug.
