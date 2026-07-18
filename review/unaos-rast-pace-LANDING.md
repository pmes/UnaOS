# RAST-PACE landing — honest frame pacing for the cube demo

**Branch:** `us-rastpace` (off main `3338d55`). **Commit:** `cc248c0`.
**Lane:** `rast_demo.rs` (the knob-gated demo module) + `rasterizer.md` docs only.
RAST-TEGRA's frame-pacing follow-up.

## Why

The R21b Orin sitting proved the render (first 3D pixels on the panel) but all 90
frames completed in ~91 ms (989 fps) — the "spinning cube" presented as a ~0.1 s blue
flash. On x86 the same demo runs present-bound at ~21.9 fps. The demo must pace
honestly: a visible, platform-consistent spin.

## What landed

- **Frame pacing (pure delay).** Added `FRAME_MS = 33` (≈ 30 fps target) and a per-frame
  pace hold at the tail of the render+present loop in `rast_demo::run()`. The slot
  deadline for frame *n* is `t_start + (n+1)·FRAME_MS`; the loop busy-waits on
  `crate::arch::ms()` only while the current clock is *behind* that deadline. Because the
  deadline is measured from `t_start`, a frame whose render+present already overran its
  slot never waits — so a platform slower than the target (x86 panel present) runs at its
  own speed. **Pacing only ever DELAYS, never skips.**
- **Finite backstop.** The busy-wait is bounded by `PACE_POLL_CAP = 200_000_000` polls
  (never an unbounded spin). On real hardware the monotonic clock reaches the deadline
  long before the cap; the cap only guards a stuck/degenerate clock (e.g. a timerless
  fallback returning a constant) so the demo can never hang and QEMU still boots straight
  through. `core::hint::spin_loop()` in the wait body.
- **Honest fps line unchanged.** The emitted line still reports MEASURED elapsed time
  (`crate::arch::ms() - t_start`), so it reads ≈ 30 fps where pacing binds and
  present-bound fps where the platform is the slower one.
- **Docs.** `docs/dev/OS/08_VIDEO/rasterizer.md §4` — added a **Frame pacing (RAST-PACE)**
  paragraph after the fps-line example.

### Lane / byte-identity discipline — honored

The change is confined to `rast_demo.rs`, which is `#[cfg(feature = "rast")]`-gated at
`lib.rs:53` and fully unlinked knob-off. No edit to the `rast` crate, panel/`Screen`
code, shared wiring, `arroyo`, or the knob. The tail-positioned tegra runner
(`tegra_rast_demo_maybe`) and its same-line-as-terminus call site in `main.rs` are
untouched, so no panic `Location` line shifts — knob-off byte-identity holds (proven
below).

## Gate results — DONE gate PASS

- `./arroyo check` **knob-off** — ✅ x86_64 OK, ✅ aarch64 OK.
- `UNAOS_RAST=1 ./arroyo check` **knob-on** — ✅ x86_64 OK, ✅ aarch64 OK.
- **Knob-off byte-identity, both arches** — full-kernel `unaos-kernel` build, branch vs
  base `rast_demo.rs` (rebuilt with `git show main:…/rast_demo.rs` restored), hashes
  **identical**:

  | arch | knob-off kernel sha256 (branch == base) |
  | --- | --- |
  | x86_64 | `fc9434e91898d7c1a814052e54a270b987bd0cda6e29f2d514c756a161f82291` |
  | aarch64 | `448c72049db743bc37f92cef6b8cbd1dd34527cb43997a38919dc08ee07a865b` |

- **Paced fps witness** (`UNAOS_RAST=1 ./arroyo test-arm 30`, aarch64/virt ramfb), serial:
  ```
  :: RAST: software rasterizer demo — 320x240 spinning cube centered on 800x600 panel, 90 frames ::
  :: RAST: 90 frames in 2972 ms — 30.282 fps (software rasterizer, panel present) ::
  xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<
  ```
  **30.282 fps** — right at the 30 fps target (pre-arc this panel flashed past at hundreds
  of fps). MISSION SUCCESS, boot proceeds normally.
- **x86 knob-on unregressed** (`UNAOS_RAST=1 ./arroyo test 30`), serial:
  ```
  :: RAST: software rasterizer demo — 320x240 spinning cube centered on 1280x800 panel, 90 frames ::
  :: RAST: 90 frames in 4144 ms — 21.718 fps (software rasterizer, panel present) ::
  xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<
  ```
  **21.718 fps** — pacing correctly did NOT delay the present-bound x86 path (its
  ~46 ms/frame present is slower than the 33 ms target), so the honest present-bound
  number is unchanged.

## Staged media (for the next Orin sitting)

`~/unaos-bench/flash/orin/UnaOS-orin-esp-RASTPACE-20260718T022742Z-cc248c0.tar`
- tar sha256 `d92497286d785a36071f74840ab7d013b1b022614b17f6641946ed6461b5372f`
- kernel.elf sha256 `329412160328e460f1bcb08e4a346b1c0b2cc06228718a7a69c3d5f25fedaa7e`
- knob-ON `UNAOS_RAST=1` + `tegra`; effective aarch64 features
  `rast,ehcihid,smolnet,tegra,tegrasmp` (tegrasmp default-on per ORIN-SMP-DEFAULT).
  MANIFEST updated.
- **Metal witness rides the next natural Orin window.** Expected: flash the tar, boot
  with a DisplayPort monitor, watch for a **visibly spinning** centered cube (~30 fps,
  a couple of seconds of spin) + the two `:: RAST: … ::` serial lines just before
  CAPSTONE — not the prior ~0.1 s flash.

## Flagged / residual

- **Metal-pending:** QEMU never builds `tegra`, so the on-panel *paced* spin is confirmed
  only on the attended Orin bench. The aarch64/virt witness above (30.282 fps) is the
  honest QEMU proof of the identical arch-neutral pacing logic.
- **Unmerged hw-jetson doc folds** (`2c24975`/`08fa663`/`88757f1`, flagged in the brief):
  my only doc edit is a new paragraph in `rasterizer.md §4`; it adds content and does not
  overlap those folds' subject matter. No conflict expected, but the integrator should
  confirm at merge.
- **No security surface** (arch-neutral leaf demo, call-never-edit on the panel API; no
  shared surface changed) — **seat-read tier** per the brief, no lens required.
