# 3D on the Jetson Orin Nano — where it stands

Baseline: `hw-jetson` @ `92997297`, audited 2026-08-18 against `origin/hw-pi4` and
`origin/UnaOS-gemini`.

This note answers one question honestly: *what can the Orin draw today, and what
would each next rung actually cost?* It is a status document, not a plan. Where a
claim is code, it cites the file; where a claim is metal, it cites the sitting;
where a claim is neither, it says so.

Scope note: the desktop/window-manager question is **not** re-argued here. It is
settled in [`docs/dev/OS/08_VIDEO/PARITY.md`](../../../../../docs/dev/OS/08_VIDEO/PARITY.md)
§8 — the desktop stack arrives at trunk sync and lights with a build knob, it is
not owed a port. This note references that section and does not duplicate it.

---

## 1. What runs on Orin metal today

Two independent 3D-ish paths exist on this seat, both pure software, both
presenting through the JD1-inherited scanout. Neither touches a GPU.

### 1.1 RAST-TEGRA — the z-buffered software rasterizer

The renderer is the `rast` crate (`unaos/crates/rast/`): `no_std`, zero-alloc in
the hot path, `f32` via `libm`, no FMA, no fast-math. Its contract and API are
documented in
[`docs/dev/OS/08_VIDEO/rasterizer.md`](../../../../../docs/dev/OS/08_VIDEO/rasterizer.md);
this section records only the Orin-side facts.

- **Wire-in.** `unaos/crates/kernel/src/main.rs::tegra_rast_demo_maybe()` (file
  tail, gated `all(feature = "tegra", feature = "rast", target_arch = "aarch64")`),
  called from the tail of `tegra_early_stop` post-drop at EL1, on the *same source
  line* as the `run_capstone_boot_core` terminus (`main.rs:2459`). The same-line
  placement is load-bearing: it keeps the knob-off tegra image byte-identical by
  adding zero source lines ahead of any panic `Location` literal.
- **Surface.** It builds a `video::Screen` over `video::WRITER` — the scanout JD1
  inherited from edk2-nvidia's `simple-framebuffer` DTB node, mapped into both
  translation tables so it survives the EL2→EL1 drop. No mode-set, no scanout
  reprogramming, no display MMIO. `video::fbcon::detach()` first, so a straggler
  CAPSTONE line cannot paint over the frames.
- **Demo.** `unaos/crates/kernel/src/rast_demo.rs` — a 320×240 flat-shaded,
  z-buffered spinning cube, blitted centered, `FRAMES = 90`, back buffer and depth
  plane off the live 48 MiB heap, presented through the public
  `Screen::put_pixel`/`flush` API (call-never-edit on the shared video path).
  `crate::arch::ms()` reads CNTVCT on the timerless post-drop core (the VUGFIX
  fallback), so the fps line still ticks.

**The metal numbers, precisely.** The metal verdict of record
(`docs/dev/OS/01_BOOT_HAL/arch_arm64.md`, 2026-07-17/18 sittings; `docs/MILESTONES.md`)
is the **unpaced** run: witness build `b41989d0…` rendered the cube on the real
**1920×1200** inherited scanout, **90 frames in 91 ms — 989.010 fps**, CAPSTONE
COMPLETE the same boot, zero faults. Attended visual confirm: the animation
completes in ~0.1 s and reads as a blue flash. That is the first 3D pixels drawn
on Orin silicon.

RAST-PACE then added pure-delay pacing (`FRAME_MS = 33`, `PACE_POLL_CAP` backstop;
`rast_demo.rs:26–52,167–181`), which caps the demo at 90 × 33 ms = 2970 ms ⇒
**30.303 fps** by construction. That figure is the pacing *target*, and it gated
on QEMU (`virt` measured 30.28 fps; x86 21.7 fps unregressed, pacing correctly
never binds there). `docs/MILESTONES.md` §RAST-PACE still carries 🔬 — "Metal
(visible spin) staged." **The paced spin has not been witnessed on the Orin
panel.** PARITY §8.3's phrase "30.303 fps metal" is the design target, not a
sitting; it wants a one-word correction at reconciliation (out of this note's
lane — flagged, not edited).

### 1.2 vug — the Q16.16 fixed-point facet renderer

`unaos/crates/kernel/src/vug.rs` (897 lines) is the older, distinct path: a
painter's-order solid-facet engine over the Gneiss PAL, float-free (geometry,
rotation and projection all Q16.16 fixed point), not z-buffered. Entry points
`run_crystal(pal, Mode::Wire|Solid)`, `run_pulse`, `run_bebox_mode`; dispatched
from the `vug` / `pulse` shell verbs (`unaos/crates/kernel/src/shell.rs:2731–2757`).

On Orin it is reachable from the JD2 console: `jd2_console_pump` builds a
`Screen` over the inherited scanout and a `pal::TargetPal` over it
(`main.rs:1926–1928`), and `handle_key` routes a completed line through
`shell::dispatch_command(&cmd, console, pal)` (`main.rs:1799–1811`). Nothing in
that chain is arch-gated.

Metal history, honestly: `vug` has *run* on the Orin. The 2026-07-10 attended
sitting recorded that the console pump **survived** vug — keystrokes kept flowing
after it, no panic or wedge — with the explicit caveat that "vug-on-tegra
behaviour beyond survival [was] not formally assessed this bench"
(`arch_arm64.md` §JD2 metal verdict). Two Orin-specific defects were found and
fixed around it since:

- **VUGFIX** — vug's meters were blank/wrong on the timerless tegra EL1; the
  `crate::arch::ms()` CNTVCT fallback and an honest meter count were the fix.
- **ORIN-VUG-RAS** — running `vug` from the JD2 shell killed the box a few frames
  in with a SNOC `Carveout Uncorrectable` RAS FillWrite. Root cause was not vug:
  the kernel heap had been seated on a firmware-protected carveout that UEFI
  reports as Conventional, and vug's per-frame `String` formatting plus full-frame
  draws were simply the first workload to grow live heap use past the boundary.
  The fix is the carveout-aware heap (`mmu_tegra::select_heap_region` +
  `fdt_tegra::reserved_carveouts`, HEAP-GUARD witness, fail-closed). The residual
  hunt continues in `unaos/crates/kernel/src/vugras.rs` (the `vugras` knob) and the
  XCARVE series. **A clean paced vug/pulse run on Orin metal is still owed.**

---

## 2. vug on Orin, layer by layer — verifying the "arch-neutral and links today" claim

The claim is **true as stated, and narrower than it sounds.** Precisely:

| Layer | Where it lives | Status on Orin |
| --- | --- | --- |
| `rast` oracle crate | `unaos/crates/rast/{lib,math,raster}.rs` | **Shared, not forked.** `git diff origin/hw-pi4 HEAD -- unaos/crates/rast` and `git diff origin/UnaOS-gemini HEAD -- unaos/crates/rast` are both **empty**: one crate, byte-identical on all three branches. |
| `rast` golden oracle | `unaos/crates/rast/tests/golden.rs` (`GOLDEN_CUBE_07 = 0x1944_46bc_a3de_a139`) | Host-side, platform-neutral; the pinned digest is the cross-arch reference. Orin consumes it, owns nothing. |
| vug renderer | `unaos/crates/kernel/src/vug.rs` | **Links unconditionally.** `pub mod vug;` at `lib.rs:92` — no `cfg`, no feature, no arch gate. Compiled into every kernel build on both arches, tegra included. |
| PAL | `unaos/crates/kernel/src/pal.rs` (`GneissPal` / `TargetPal`) | Arch-neutral; `TargetPal` is always `Screen`-backed. The only `cfg` in the event path is the input source. |
| Panel surface | `video/{screen,framebuffer,fbcon}.rs` + JD1 | Present and metal-proven. One `PixelFormat` contract above three display paths. |
| Window manager | `video/wm.rs`, `strip.rs`, `dock.rs`, `desktop_firmware.rs`, … | **Absent on this branch.** See PARITY §8.0/§8.1 — arrives at trunk sync, not owed a port. |

So: *the renderer links and the renderer runs.* What does **not** exist is
everything that would make it an application rather than a full-screen takeover.
Concretely, the gap is:

1. **Windowing.** vug on Orin owns the whole panel and hands it back on exit
   (`console.draw(pal)` restores the shell). There is no window, no compositor, no
   damage-tracked present, because `wm.rs` is not on this branch. This resolves at
   trunk sync plus the `pidesk` knob on the `esp-jetson` recipe — PARITY §8.1
   steps 1–4, not repeated here.
2. **Input routing.** JD20 pointer events reach the pump and draw a cursor, but
   clicks only log (`:: tegra: JD20 — pointer BUTTON … ::`). A vug window needs
   those routed into `wc_click_route`/`strip::press_route` (PARITY §8.1 step 3).
   The `Event::Button` edge-detect vug already consumes on x86 exists; the tegra
   producer is the missing half.
3. **Pacing.** vug and `wm` both assume a timer tick the tegra post-drop core does
   not have. RAST-PACE showed the shape of the answer (pure delay off
   `crate::arch::ms()`/CNTVCT, never a skip); `wm`'s present paths need the same
   audit before they are trusted here. True vblank pacing off the DCE is display
   -controller work that does not exist and is not required.
4. **Memory hygiene.** The XCARVE/VUGRAS ledger is the live risk on this seat, and
   vug is its most reliable trigger — any 3D work that grows heap footprint should
   expect to meet it first. Read `vugras.rs` before adding an allocation to a frame
   loop here.

None of these are renderer work. The renderer is done; the seat around it is not.

---

## 3. The GPU rung — GA10B, and what it would actually cost

**Current state: there is no GPU code on this branch, of any kind.** The audit is
unambiguous — every hit for `falcon` in `unaos/` on `hw-jetson` is the **XUSB**
Falcon microcontroller (`bpmp_tegra.rs`, `xusb_tegra.rs`, `bootloader/src/main.rs`
JB6–JB8, the CRCR-wall rules), not a graphics Falcon. There are **zero** hits for
`nvgpu` and zero for `ga10b`. Nothing has been probed, mapped, or written.

The Orin Nano's iGPU is an **Ampere GA10B** on the Tegra234 die. The honest
statement of what bring-up would require, before any code:

- **Firmware is the wall, not the registers.** Ampere-class NVIDIA GPUs boot
  through signed microcode: the GSP/FECS/GPCCS Falcon (now RISC-V "Peregrine" on
  Ampere-and-later parts) will not accept unsigned code, and the signing key is
  NVIDIA's. This is categorically harder than the Kepler GK107 campaign on
  `origin/UnaOS-gemini` (see `docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` and
  `gpu_spec.md`), where a from-scratch reimplementation of the *initialization*
  firmware was at least conceivable. **What transfers from that campaign is the
  discipline, not the code and not the plan**: the four-tag evidence vocabulary
  (sitting id / DERIVED / EXT / UNPINNED), "a claim with no citation is not in
  this document", and the rule that a leg reports what it assumed and can void
  itself. Those are worth importing verbatim. The GK107 register map is worth
  nothing here — different vendor generation, different die, different boot chain.
- **The block is powergated.** GA10B sits behind BPMP power-domain and clock
  control exactly like XUSB did. Nothing may read its apertures until an MRQ
  proves the partition is on (see §4).
- **The blob law — Peter's call, and only Peter's.** Any NVIDIA firmware blob, and
  the `r8169` NIC firmware, fall under
  [`docs/MANIFESTO/CLEAN_ROOM_POLICY.md`](../../../../../docs/MANIFESTO/CLEAN_ROOM_POLICY.md)
  §4: the public tree never carries a proprietary blob, in source or in any media
  image it builds. Anything that stages, caches, or vendors such firmware lives in
  **UnaOS-bunker ONLY**. The precedent is set and consistent — see the BT/`bcm4331`
  boundary (`docs/dev/OS/06_NETWORK_STACK/bcm4331.md` §21.8,
  `docs/dev/OS/07_USB_STORAGE/usb_xhci.md`), where the sequence *witnesses* the
  firmware boundary explicitly and then stops. **The licensing decision is
  Peter's.** This document names the decision; no session makes it, and no arc may
  assume it has been made.

**The blob-free ceiling, stated plainly.** Without firmware the Orin gets:
software rasterization into the inherited scanout, at CPU speed, on up to six A78AE
cores. That is a real ceiling and a usable one — RAST-TEGRA cleared 989 fps at
320×240 unpaced — but it is a ceiling. Blob-free does **not** get you: shader
execution on GA10B, hardware transform/rasterization, video decode (NVDEC), any DC
mode-set path that depends on DCE firmware, or vsync off the DCE. Those are all
firmware-gated, and no amount of clever register work changes that. Any future
"GPU on Orin" arc that does not open with the firmware question is mis-scoped.

The one rung that is genuinely available and blob-free is **more CPU**: the
rasterizer is embarrassingly parallel and the seat has six cores with a working
steal-half scheduler arriving at trunk sync. That is the next honest performance
rung, and it needs no NVIDIA anything.

### 3.1 RAST-MC — the multi-core rung, as far as the shared crate allows

**The cores are real, and they already dispatch.** The statement above ("six
cores … arriving at trunk sync") understated what is on this branch today. The
tegra image arms `tegrasmp` **by default** (`unaos/arroyo:573` — `UNAOS_TEGRA=1`
without `UNAOS_NOTEGRASMP=1` adds the feature), so
`smp_virt::start_secondaries_tegra` (`smp_virt.rs:783`) `CPU_ON`s every DTB
`/cpus` secondary before the JM6 drop, and each woken core runs the *shared*
secondary tail `__secondary_rust_virt` (`smp_virt.rs:253`), which ends in
`timer::arm_this_core_ap()` + `sched::secondary_run(core)`
(`smp_virt.rs:329-345`). `secondary_run` (`sched.rs:5084`) calls `mark_online`
and enters the preemptive `run()` loop. **The Orin secondaries are therefore
full scheduler participants — `ONLINE_MASK` members, `CPU_AUTO` placement
candidates, steal targets — not parked cores.** The line
`start_secondaries_tegra` still prints, "AP timer PPI stretch deferred (JC3)",
is stale with respect to that shared tail; JC3 landed, and the AP arms its own
local-only tick. (Correcting that log string is a one-word edit in the SMP lane,
flagged here, not made.)

**What the shared `rast` crate can and cannot express.** Band/tile decomposition
— the decomposition that would parallelize both halves of a frame and scale with
core count — is **not expressible through `rast`'s public API**, and `rast` is
shared-lane and golden-pinned (§4.3), so the arc stopped rather than forking it.
Precisely what is missing:

- `render_mesh` maps NDC straight onto `target.width()`/`height()`
  (`rast/src/lib.rs:110-113`, `to_screen` at `lib.rs:84`). A band-sized `Target`
  therefore renders the *whole scene squashed into the band*, not the band's
  slice of the scene. There is no viewport origin and no scissor rectangle on
  `Target` (`rast/src/raster.rs:54-81` — only `width`/`height`/`stride`).
- The transform stage is not separable: `clip_near`, `divide_and_map` and
  `to_screen` are all private, so a caller cannot run transform+clip itself and
  feed band-offset `ScreenVert`s into the public `Target::triangle`
  (`raster.rs:142`) without re-implementing the pipeline — which is a fork of the
  oracle in all but file location, and would put `GOLDEN_CUBE_07` at risk.

  **The minimal shared-lane API that would unlock bands** (for whoever proposes
  it, on the shared lane, with the golden re-verified): either (a) a viewport
  origin on `Target` — `Target::new_offset(color, depth, w, h, stride, origin_x,
  origin_y)` where the *frame* dimensions used by `to_screen` stay `(w, h)` while
  the *writable* rows are the band — or (b) a public split of the pipeline:
  `pub fn project_mesh(model, view_proj, verts, indices, w, h, cull, &mut FnMut([ScreenVert;3], Rgba))`,
  which emits already-projected, already-shaded, already-clipped triangles that a
  caller may offset and hand to `Target::triangle`. (b) is the more useful of the
  two — it is also what a future GPU-vs-reference diff wants.

**What was implemented instead: frame pipelining, which needs no `rast`
change.** `unaos/crates/kernel/src/rast_demo.rs::run_mc` (tail, gated
`all(feature = "tegra", target_arch = "aarch64")`, linked only under `rast`)
probes each secondary with a pinned `sched::spawn`, enlists the ones that
actually dispatch, gives each an own full-size RGBA8 + f32-depth pair off the
heap, and assigns frames round-robin: core at slot `k` renders every frame
`f ≡ k (mod nslots)` with the *same* whole-frame `render_mesh` call the
single-core path makes, while the boot core presents finished frames **in strict
frame order** through `Screen::put_pixel`. Pixels and their sequence are
identical to single-core by construction. Wired in on the existing terminus line
(`main.rs:5114`) so the knob-off image adds zero source lines.

Three honesties belong on the record with it:

1. **Amdahl.** Only the render half is parallel; the present half (76 800
   `put_pixel` + one `flush` per frame) stays serial on the boot core. The
   ceiling is `total / max(present_total, render_total / nslots)` — of order 2×
   when render and present cost about the same, regardless of how many cores are
   online. The witness reports the **measured** ratio against a 1-core baseline
   taken in the *same boot, unpaced* (a paced comparison would read 30.303 fps on
   both arms by construction and mean nothing).
2. **Heap.** 600 KiB of back+depth buffer per render core (320×240). Five
   secondaries ⇒ 3.0 MiB more live heap, on the seat whose documented RAS trigger
   is exactly "grew live heap use past the carveout boundary" (§1.2,
   ORIN-VUG-RAS / XCARVE). The witness prints the footprint; the slot count is
   capped by the cores that check in.
3. **Unwitnessed.** RAST-MC has **not** run on Orin silicon. QEMU cannot stand in
   for it: the tegra path is metal-only, and the `virt` GICv3 path has the same
   shape but is not this code's gate. Its witness lines
   (`:: RAST-MC: N core(s), M frames, X fps — speedup Yx vs 1-core ::`, the
   per-core `:: RAST-MC: core C rendered F frame(s) ::`, and the 1-core baseline
   line) are **PENDING** until an attended sitting captures them.

The EL split is worth stating once because it is the non-obvious part that
works: the boot core presents from **EL1** under `mmu.ttbr0_el1` while the render
workers run at **EL2** under the EL2 table. Both map RAM Normal-WB
**inner-shareable** (`mmu_tegra.rs:488,505-512`), so the shared buffers and the
handshake atomics are hardware-coherent across that split; no cache maintenance
is needed and none is done.

---

## 4. Do not do

Short list, each with the precedent that earned it.

1. **Do not probe powergated blocks.** A read of a gated Tegra partition is
   **EL3-fatal** — the JX1 lesson, paid for on the bench. The standing discipline:
   prove the partition is on via a BPMP MRQ *first*, and emit a serial line
   *before* touching any new MMIO address class, so the last line in the capture
   names the killer. Precedent in code: `bpmp_tegra.rs:5,34,313–366,484`;
   `fdt_tegra.rs:430`; `mmu_tegra.rs:816`; `pcie_probe.rs:289`. This applies to
   GA10B apertures with full force.
2. **Do not touch nvdisplay MMIO.** JD1 proves pixels from the DTB
   `simple-framebuffer` node and never reads a display register;
   `display_tegra.rs:56` keeps `JD1_DC_PROBE = false` as a default-off fallback
   precisely because a powergated nvdisplay read is in the same fatal class
   (`display_tegra.rs:41,53`). Inherit-don't-reinit is the standing rule.
3. **Do not fork the oracle layer.** `unaos/crates/rast/` is byte-identical across
   `hw-jetson`, `hw-pi4` and `UnaOS-gemini` today, and that is the whole point: the
   pinned `GOLDEN_CUBE_07` digest is only a cross-arch reference while exactly one
   copy exists. An Orin-flavoured `rast` would destroy the property that makes
   future GPU output checkable ("GPU output == rasterizer reference"). If Orin
   needs something from `rast`, it is a shared-lane change, which means: **stop and
   report** — do not edit it from this track.
4. **Do not regenerate a golden to make a test pass.** Same reason. Regenerate only
   deliberately, with the reason recorded.
5. **Do not weaken a protection to get a frame out.** No disabling WXN, no widening
   a mapping past a HEAP-GUARD refusal, no un-clipping the VUGRAS span-B sweep past
   a carveout — `DC CIVAC` on a SNOC-protected line *is itself* the FillWrite RAS
   (`vugras.rs`, VUG-RAS-ANALYZE). Fail closed.

---

## See also

- [`docs/dev/OS/08_VIDEO/rasterizer.md`](../../../../../docs/dev/OS/08_VIDEO/rasterizer.md) — the `rast` crate contract, API and golden scheme.
- [`docs/dev/OS/08_VIDEO/PARITY.md`](../../../../../docs/dev/OS/08_VIDEO/PARITY.md) §8 — the Orin desktop seat; §8.1 is the display seam.
- [`docs/dev/OS/08_VIDEO/engine.md`](../../../../../docs/dev/OS/08_VIDEO/engine.md) — the vug facet engine's x86 history.
- [`docs/dev/OS/01_BOOT_HAL/arch_arm64.md`](../../../../../docs/dev/OS/01_BOOT_HAL/arch_arm64.md) — §RAST-TEGRA, §VUGFIX, §ORIN-VUG-RAS, §JETSON-RAS, the XCARVE series, and the metal verdicts of record.
- [`docs/MANIFESTO/CLEAN_ROOM_POLICY.md`](../../../../../docs/MANIFESTO/CLEAN_ROOM_POLICY.md) — the blob law.
