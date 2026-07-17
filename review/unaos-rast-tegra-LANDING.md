# RAST-TEGRA landing — the software rasterizer on the Orin panel

**Branch:** `us-rasttegra` (off main `45a06b2`). **Commit:** `635e9f7`.
**Lane:** tegra-feature + shared `rast` knob wiring. RAST-1's deferred Orin wire-in.

## What landed

RAST-1 merged the platform-neutral `rast` rasterizer with an x86/virt-only demo.
`rast_demo::run()` is arch-neutral (it drives only the public `Screen` API +
`crate::arch::ms()`), so this arc un-gates it and wires it to two more panels.

- **Un-gate the crate/module (both arches).** Moved the `optional` `rast` dep out of
  the `[target.'cfg(target_arch="x86_64")']` block into the shared `[dependencies]`
  (it is `no_std`/arch-neutral by construction), and dropped the `target_arch="x86_64"`
  cfg on `pub mod rast_demo`. Knob-off unlinks the dep + module on BOTH arches.
- **aarch64/virt wire-in (the QEMU witness).** The shared GUI setup in `kernel_main`
  had an x86-only demo block; broadened its cfg to
  `all(feature="rast", not(feature="pi"), not(feature="tegra"))`. The GICv2 `virt`
  boot (test-arm default) reaches that shared path with a `ramfb` framebuffer, so
  `UNAOS_RAST=1 ./arroyo test-arm` witnesses the identical arch-neutral render under
  QEMU. Edit is **line-count-neutral** (comment trimmed to the original 8-line block)
  so it introduces no panic-line shift.
- **aarch64/tegra wire-in (the Orin panel).** `tegra_rast_demo_maybe()` runs at the
  EL1 tail of `tegra_early_stop` — post-drop, right before `run_capstone_boot_core` —
  and draws the spinning cube through the **JD1-inherited scanout** (no mode-set, no
  scanout reprogramming). It builds a `Screen` over `video::WRITER` (seeded by JD1,
  mapped into both translation tables so the carveout is reachable at EL1), detaches
  fbcon's mirror, then calls the shared `rast_demo::run()` — **call-never-edit** on
  the panel surface. `crate::arch::ms()` reads `CNTVCT` on the timerless post-drop
  core (the VUGFIX fallback), so the honest fps line still ticks. **First 3D pixels
  drawn on Orin silicon.**
- **Docs.** `docs/dev/OS/08_VIDEO/rasterizer.md §4` rewritten for the three panels;
  `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` §RAST-TEGRA pointer. `arroyo` knob comment
  updated (own line only; zero behavior change — the knob already had no arch guard).

### The panic-line byte-identity hazard — honored

Mid-`kernel_main` gated blocks shift embedded panic `Location` line numbers and break
knob-off byte-identity (PI-V3D-1 bisect-proven). The tegra runner therefore lives at
the **file tail** (shifts nothing after it) and is **called on the same source line**
as the `run_capstone_boot_core` terminus, with an `#[inline(always)]` empty knob-off
twin. Net: the wire-in adds **zero source lines ahead of any panic Location**. Proven
below.

## Gate results — DONE gate PASS

- `./arroyo check` **knob-off** — ✅ x86_64 OK, ✅ aarch64 OK.
- `UNAOS_RAST=1 ./arroyo check` **knob-on** — ✅ x86_64 OK, ✅ aarch64 OK.
- `tegra` release build **knob-off** and **knob-on** (`--features ehcihid,tegra[,rast]`)
  — both compile clean; knob-on links 11 `rast` symbols, knob-off links 0.
- `./arroyo test-arm 22` — ✅ `MISSION SUCCESS` (v2, knob-off).
- `UNAOS_GICV3=1 ./arroyo test-arm 40` — ✅ CAPSTONE 6/6 COMPLETE.
- `./arroyo kernel8-test 35` — ✅ 55 PASS, **0 FAIL**, CAPSTONE COMPLETE.
- `./arroyo test 22` (x86, knob-off) — ✅ **0 FAIL**, `MISSION SUCCESS`.
- **Knob-on virt witness** (`UNAOS_RAST=1 ./arroyo test-arm 30`), serial:
  ```
  :: RAST: software rasterizer demo — 320x240 spinning cube centered on 800x600 panel, 90 frames ::
  :: RAST: 90 frames in 464 ms — 193.965 fps (software rasterizer, panel present) ::
  xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<
  ```
  (The demo runs on aarch64 and the boot proceeds normally afterward.)

### Knob-off byte-identity — BOTH arches (lane-cleanliness proof)

Section-wise vs the pre-arc base (the RAST-1 method; whole-ELF differs only in
path-tainted symtab/strtab). All loadable/semantic sections **byte-identical**, 0
`rast` symbols knob-off.

**aarch64 `tegra` kernel (`ehcihid,tegra`, knob-off), base `45a06b2` vs branch:**

| section | hash | result |
| --- | --- | --- |
| `.text` | `a2ce1599a4da38e889318a7657c83f4d94da693545eb93c67ec387f1cdd1094f` | MATCH |
| `.rodata` | `5d1f7604a54443466327cd2a927c9a492619542f787dc170c8868a3f45405784` | MATCH |
| `.data` | `4f1fe11ebf530e27031b551911de5b6fe1d12c5e43ce30a9449be3bc871380a4` | MATCH |
| `.data.rel.ro` | `e17e3b1384607f944659fbed14176b0eba569597522b648b84552b981fcd6905` | MATCH |

**x86_64 kernel (`ehcihid,smolnet`, knob-off), base `45a06b2` vs branch** — the arc
touches the shared `kernel_main` demo block, so re-verified: `.text` / `.rodata` /
`.data` / `.data.rel.ro` **all MATCH** (base `.text c8815627…`), 0 `rast` symbols.

## Staged media (for the next Orin sitting)

`~/unaos-bench/flash/orin/UnaOS-orin-esp-RAST-20260717T221750Z-635e9f7.tar`
- tar sha256 `8840fbe2c0040e98f5fb0582b782d5048e4b2efb07c0a8f6378bf53cbe68ccef`
- kernel.elf sha256 `06ddaa7be573e6e282794d34b2dc969c53f49e2c70d4546ab49a70a818c67dcc`
- knob-ON `UNAOS_RAST=1` + `tegra`; effective aarch64 features `rast,ehcihid,tegra`
  (smolnet stripped by `arm_features`). MANIFEST updated.

## Flagged / residual (for the sitting brief)

- **The visible cube on the real Orin panel is METAL-PENDING** — QEMU never builds
  `tegra`, so the on-panel render is unverified until an attended Orin sitting. The
  aarch64/virt QEMU witness above proves the identical arch-neutral render; the tegra
  sitting confirms it reaches the physical panel via the JD1 scanout at EL1. Expected:
  flash the staged tar, boot with a DisplayPort monitor, watch for the centered
  spinning cube + the two `:: RAST: … ::` serial lines just before CAPSTONE.
- The demo runs **before** CAPSTONE and detaches fbcon, so the cube is the last panel
  content until the JD2 console pump takes over on the first keystroke. A
  keystroke-triggered or persistent-until-input variant is a possible follow-up, not
  in this arc's lane.
- Present is per-pixel `put_pixel` at a fixed 320×240 (RAST-1's witnessable choice);
  a bulk-blit present is a future optimization.
- No security surface (arch-neutral leaf demo, call-never-edit on the panel API);
  seat-read tier per the brief — no shared panel surface changed, so no lens required.
