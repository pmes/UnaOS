# Desktop parity — the Orin section (hw-jetson fragment)

INTEGRATOR NOTE: the parity audit's body (the x86 `wc` ⇄ Pi 4 `pidesk` gate census,
§1–§7) is maintained on `hw-pi4` — this branch deliberately carries ONLY the Orin
section below, to keep the add/add merge trivial while the pi seat live-edits its
rows. At reconciliation, append this as §8 of the merged file.

Baseline for this section: hw-jetson @ `92997297`, audited 2026-08-18 against
`origin/hw-pi4` (shared-arc locations) and `origin/UnaOS-gemini` @ `122ed63e`.
Vocabulary follows the body: dispositions PORT IT / LEGIT ARCH-SPECIFIC / RULED
OUT; row classes (a) ported already / (b) experience gap / (c) legit
arch-specific / (d) in flight.

## §8 The Orin seat (hw-jetson)

### §8.0 The honest headline

There is no window manager on this branch. `video/` on hw-jetson HEAD contains
exactly `fbcon.rs, framebuffer.rs, mod.rs, screen.rs, vperf.rs, witness.rs` — no
`wm.rs`, `strip.rs`, `dock.rs`, `menubar.rs`, `crystal.rs`, `pulsewin.rs`,
`pidesk.rs`, `quarry*`. The kernel Cargo.toml here has no `wc`, `pidesk`,
`quarry`, or `wcg-paygo` feature. The panel today is: boot log on fbcon, then the
JD2 full-screen console (first key or ~8 s), with JD20 pointer drawing a cursor
whose clicks log `:: tegra: JD20 — pointer BUTTON … ::` and drive nothing.

Consequence: almost every desktop row's Orin cell is not "owed a port" but
**"arrives at base sync"** — the desktop stack is knob-gated on the paired
predicate `any(all(x86_64, wc), all(aarch64, pidesk))`, whose aarch64 arm is
arch-generic, not Pi-specific. The Orin does not port the desktop; it inherits
it at the next rebase and lights it with a build knob. The per-row exceptions
are §8.2.

### §8.1 The display seam, precisely

What drives the Orin panel today (JD1, `arch/aarch64/display_tegra.rs`, gated
`feature = "tegra"`):

- NVIDIA's UEFI GOP is `BltOnly` — no CPU-linear framebuffer, and `Blt()` dies at
  ExitBootServices, so `boot_info.framebuffer_addr` is 0 and the x86-style GOP
  path is inert. Instead, edk2-nvidia's display handoff publishes the live
  scanout as a `simple-framebuffer` DTB node (base/size/geometry/format).
  `jd1_survey()` (`display_tegra.rs:95`) walks the captured DTB, never touches
  display MMIO (a powergated nvdisplay read is EL3-fatal — the JX1 lesson;
  `JD1_DC_PROBE` stays default-off), maps the region Normal-WB into both tables
  (`mmu_tegra::map_fb_region`, survives the JM6 EL2→EL1 drop), and hands the
  result to `video::fbcon::init` + `video::WRITER` (`main.rs:1284–1300`). The
  DCE keeps scanning that DRAM; fbcon's `flush_*` cleans CPU writes to PoC
  (the Pi-HVS recipe — the DCE does not snoop).
- Inherit-don't-reinit is the standing rule (the JB6→JB9 XUSB lesson applied to
  display). Mode-set, vsync, and multi-head are all DC-programming work that
  does not exist yet and is NOT required for the desktop knob.

What the desktop knob needs from this seam: nothing new. The seam already yields
base/len/pitch/format through the same `PixelFormat` contract `wm`/`fbcon`
consume on the Pi. Lighting the desktop on Orin =
1. take the base sync (brings `wm.rs` and the whole video stack),
2. add `pidesk` to the tegra build recipe — note the knob class: `pidesk`/
   `quarry`/`pirast` are curated in arroyo's `K8_FEATS` block, not
   `builder/src/main.rs`; the Orin analogue is the `esp-jetson` recipe,
3. wire JD20 pointer events into `wc_click_route`/`strip::press_route` instead
   of the log line,
4. re-verify pacing: `wm`'s present paths assume the Pi's timer tick; the tegra
   timer (`crate::arch::ms()` CNTVCT fallback) needs the `[el0live]`/pacing tick
   sources re-checked on this seat.

Vsync-accurate pacing on the DCE (true vblank) is future DC work — class (c)
until someone proves a safe non-powergated vblank source.

### §8.2 The shared-arc rows (the month of pi/x86 work, classed for Orin)

| Arc | Where it lives | Orin class | Note |
| --- | --- | --- | --- |
| steal-half scheduler (`sched_spread.rs`) | shared top-level, consumers in both `arch/*/sched.rs` | **(a) at base sync** | arch-neutral policy file by design; `cr3_live` correctly not ported (aarch64 TLBI is IS-broadcast) — RULED OUT stays ruled out here too |
| `[el0live]` EL0-extinction witness | `arch/aarch64/sched.rs` + `timer.rs`, ungated | **(a) at base sync** | re-verify the tick source against the tegra timer divergences in `timer.rs` |
| EL0 fixture park fixes (real sleeps) | inline blobs in `arch/aarch64/syscall.rs` | **(a) at base sync** | ⚠ highest-conflict merge surface: the Orin copy of `syscall.rs` diverged heavily (JB/JD/JX); same park-fix class WILL bite tegra fixtures — verify, don't assume |
| `SYS_WIN_PRESENT_ROWS` aarch64 arm | ABI shared; aarch64 dispatch in shared `syscall.rs`; impl in `video/wm.rs` | **(a) at sync, inert** | no consumer until `wm.rs` arrives; nothing Orin-specific owed |
| serial-focus split | `main.rs` `serial_focus_selftest`, gated `all(aarch64, baremetal, witness)` and `baremetal = ["pi"]` | **(b) OWED — gate widening** | body is aarch64-generic but compiled out of every tegra image; Orin needs a tegra arm on the gate + a serial `shell_inbox` equivalent on the tegra console path |
| DRAGWEDGE interactive drain bound | `video/wm.rs` + fixture in shared `syscall.rs` | **(a) at sync, inert** | arrives only with `wm.rs`; fixture has no compositor to drive until then |
| BOT-PARK identity parking | `drivers/xhci/mod.rs` (shared driver, already on this branch) | **(a) at base sync — most directly transferable** | ⚠ composition with the xHCI-wall rules is UNVERIFIED: NEVER write CRCR at RS=1 on the inherit path stands; BOT-PARK's ladder must be audited against XCARVE/CRCRQ before it runs on Orin metal. Ledger evidence is a `2109:3431` hub — re-validate against the Orin's HS hub |

### §8.3 Class (c) on this seat — legit hardware-specific

- JD1 inherited-simplefb scanout itself (vs Pi HVS, vs x86 Kepler): three display
  paths, one `PixelFormat` contract above them. Correctly arch-specific.
- The XUSB/tegra xHCI quirks (CRCR wall, JB-series), the GICv3 parameterization,
  the A78AE erratum heal, `sdmmc_tegra`: all (c), owned by this track.
- RAST-TEGRA's spinning-cube demo (`rast` feature) is this seat's proven
  pixels-on-panel path (30.303 fps metal) — the renderer above it is arch-neutral
  (M3 statement is separate).

### §8.4 What this section does NOT claim

- No desktop milestone is claimed new or done here (the body's rule). The Orin
  cell for every §6 row is "at base sync" or the §8.2 exception, never "done" —
  metal confirmation on this seat has happened for exactly none of them.
- The multi-user chain's Orin status is tracked in ROADMAP §1 (the Jetson lane
  took the panel shell instead of the userspace port); its bring-up is the
  current arc's M1b, not a parity row.
