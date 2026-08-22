# orin-desktop.md — the Jetson Orin Nano desktop ladder

Scope: what the window compositor already is on the `hw-jetson` track, what stops
it reaching the panel, and the commit-sized rungs from here to a real desktop.

**Baseline: `hw-jetson` @ `3dc889e7`**, surveyed and measured 2026-08-22
(ORINDESK). Companion to [`PARITY.md`](PARITY.md) §8, whose §8.0 headline this
document was written to replace — that section claimed there was no window
manager on this branch, which was true of its `92997297` baseline and false ever
since the base sync (`ceaa32b8`, "Merge trunk @ `122ed63e` into hw-jetson — the
month of desktop/userspace/net work lands on the Orin track").

Two files in the citations below — `arch/aarch64/sched.rs` and
`arch/aarch64/syscall.rs` — were being edited by other seats while this survey
ran. **Every line number given for those two is against `3dc889e7`**, read with
`git show`, not against the working tree.

The survey itself ran against `adfc4be6`, which was recommitted as `3dc889e7`
mid-session (same parent `76ddba18`, same tree `5c99f55b`). Every measurement and
line number below therefore holds at `3dc889e7` unchanged; the earlier sha is
recorded here only so the two can be reconciled if `adfc4be6` turns up in another
seat's notes.

---

## §1 The measured inventory

Four columns, because they are four different states and conflating them is what
produced the headline this document corrects:

- **EXISTS** — the source is on this branch.
- **COMPILES** — it is in the aarch64 object code of a `tegra` build. Measured,
  not assumed: see §1.1 for the exact invocations.
- **REACHABLE** — some code path on a tegra boot actually calls it.
- **PROVEN** — observed working on Orin metal.

| Component | File | EXISTS | COMPILES (aarch64+`tegra`) | REACHABLE (tegra boot) | PROVEN (Orin metal) |
| --- | --- | :-: | :-: | :-: | :-: |
| Window manager core | `video/wm.rs` (20 652 lines) | ✅ | ✅ `video/mod.rs:46` declares `pub mod wm;` **unconditionally** | ⚠️ *trivially* — `main.rs:2026` calls `wm::retile_on_ready()`, which returns 0 on an empty table | ❌ table empty, nothing composited |
| Compositor staging buffer | `wm::reserve_stage`, `video/mod.rs:278` | ✅ | ✅ | ❌ sole caller is `init_panel`, which the tegra path skips (§3.3) | ❌ |
| Hit-test / focus primitives | `wm::hit_test` `video/wm.rs:2434`, `wm::focus_changed` `:2514` | ✅ | ✅ — **no `#[cfg]` on either** | ❌ no tegra caller (§3.4) | ❌ |
| Click router | `arch/aarch64/syscall.rs::wc_click_route` | ✅ | ✅ ungated within its module | ❌ `jd2_console_pump` never calls it | ❌ |
| Dock / strip / menubar / crystal | `video/dock.rs`, `strip.rs`, `menubar.rs`, `crystal.rs` | ✅ | ❌ gated `any(all(x86_64, wc), all(aarch64, pidesk))` — `video/mod.rs:97, 105, 115, 125` | ❌ | ❌ |
| Desktop-ready seam | `video/pidesk.rs` (564 lines) | ✅ | ❌ gated `all(aarch64, pidesk)` — `video/mod.rs:413` | ❌ and structurally unreachable (§3.1, §3.2) | ❌ |
| Quarry (file browser) | `video/quarry.rs`, `video/quarry/live.rs` | ✅ | ❌ gated as the furniture — `video/mod.rs:441` | ❌ | ❌ |
| Panel / inherited scanout | `arch/aarch64/display_tegra.rs`, `jd1_survey` at `:95` | ✅ | ✅ `feature = "tegra"` | ✅ | ✅ JD1 |
| Framebuffer + `WRITER` handle | `video/framebuffer.rs`, seeded `main.rs:2016` | ✅ | ✅ | ✅ | ✅ JD2 |
| `Screen` + damage tracking | `video/screen.rs` | ✅ | ✅ | ✅ | ✅ JD2 |
| Cursor sprite | `video/cursor.rs` | ✅ | ✅ | ✅ | ✅ JD20 |
| Cache-clean to PoC | `FrameBuffer::flush_range` (`video/framebuffer.rs`) | ✅ | ✅ | ✅ | ✅ the DCE does not snoop; JD1/JD2 pixels land |
| Keyboard input | `jd2_console_pump`, `main.rs:2665` | ✅ | ✅ `all(tegra, aarch64)`, `main.rs:2664` | ✅ | ✅ JD2 |
| Pointer input | same pump | ✅ | ✅ | ✅ | ✅ JD20 |
| Pointer **routing** | — | ❌ | — | ❌ `main.rs:2843` logs the button and returns | ❌ |

The three rows that matter most:

1. **`wm.rs` COMPILES into every Orin kernel today.** `video/mod.rs:46` is
   unconditional. This is not an inference — the `arm-tegra` type-check emits
   dead-code warnings *from inside `wm.rs`* (`video/wm.rs:13508`,
   `fn move_present_take` never used), which is only possible if the module is in
   the lib.
2. **The furniture's aarch64 arm is arch-generic, not Pi-specific.** The
   predicate is `all(target_arch = "aarch64", feature = "pidesk")` — no `pi`, no
   `baremetal`, no BCM2711. `video/pidesk.rs`'s body touches no VideoCore mailbox
   and no BCM2711 register; its floors are panel geometry and dock capacity, both
   board-neutral.
3. **Input is proven and unrouted.** Keyboard and pointer both reach the pump on
   metal; the button arm logs and drops. `wm::hit_test` and `wm::focus_changed`
   carry no `#[cfg]` at all — they are already compiled into the Orin kernel and
   waiting for a caller.

### §1.1 How the COMPILES column was measured

A detached worktree at `3dc889e7` (so no other seat's in-flight edits were in
scope), `CARGO_TARGET_DIR` in scratch, `cargo +nightly check --release --target
aarch64-unaos.json -Zbuild-std=… -Zjson-target-spec --features <set>`. The
`arm-tegra` set is `unaos/arroyo:1833` verbatim. `unaos/target/user_blob.bin` was
staged into the worktree first (§3.6 note).

| Feature set | Result |
| --- | --- |
| `arm-tegra` (matrix leg, verbatim) | **GREEN** — 21.99 s, 29 warnings, 0 errors |
| `arm-tegra` + `tegra_el0` | **GREEN** — 0 errors (control) |
| `arm-tegra` + `pidesk` | **2 errors** (§3.5) |
| `arm-tegra` + `tegra_el0` + `pidesk` | **2 errors** (§3.5) |
| `arm-tegra` + `tegra_el0` + `pidesk,quarry,livecon` | **3 errors** (§3.5) |

---

## §2 Provenance — `wm.rs` was born on aarch64

This reframes the whole exercise, so it is stated with its evidence rather than
asserted. The compositor was **not** written for x86 and ported to ARM. It was
written on the Pi track, for an aarch64 panel, and x86 was added afterwards.

| Claim | Evidence |
| --- | --- |
| `wm.rs` was created by `51d03376` | `video: WC-A1 — video::wm public API (window table types + create/present/move/close/composite)`, 2026-07-24. Diffstat: `video/mod.rs` +5, `video/wm.rs` +329. Exactly one add of this path in the whole repo — no rename ancestor |
| The WC-A…WC-K series is Pi-track work | 26 commits, 2026-07-24 → 2026-07-25. They repeatedly touch `arch/aarch64/mailbox.rs` (WC-D `95b46fdf`, WC-E `898531a5`, WC-F `373c01a4` — the VideoCore firmware mailbox) and `unaos/scripts/specs/pi4-regression.spec` (WC-D…WC-K). **No commit in the arc touches `arch/x86_64/`**, which already existed at the time |
| It entered trunk through the Pi parent | `c4b913cf`, `Merge hw-pi4 window-compositor arc to main (wc-op1, Peter-approved)`. `51d03376` is an ancestor of the second (hw-pi4) parent `3cad6111` and **not** of the first parent `8e9720c2` — it did not exist on main before that merge |
| It had no arch gating at birth | `git show 51d03376:…/wm.rs` contains zero hits for `aarch64`/`x86`/`target_arch`/`cfg(` — it was written against the single Pi kernel build. Its module doc names the **HVS** (the BCM2711 Hardware Video Scaler) and "EL0", an ARM exception level |
| x86 arrived later, inside `wm.rs` | First commit introducing `target_arch` into `wm.rs`: `95b46fdf` (WC-D, 2026-07-25). First introducing the string `x86`: `cdb00b02` (2026-07-26) |
| `pidesk.rs` was created by `0750e011` | `video: CONSWIN-PI / MENUBAR-PI M1 — the console gets a window and the bar gets turned on`, 2026-08-13, +226 lines |
| Both are ancestors of HEAD | `git merge-base --is-ancestor 51d03376 HEAD` → yes; same for `0750e011` |

Direction is **aarch64 → x86**. The Orin is not importing foreign code; it is
picking up a compositor that has only ever had one arch as its home, and that
arch is this one.

**UNVERIFIED:** the specific `exec-*` worktree branch `51d03376` was originally
authored on. What is verified is the track — it reached main via `c4b913cf`'s
`hw-pi4` parent and is absent from main's first-parent line before that merge.

---

## §3 The blockers

Six, each with evidence. None is large; the point of enumerating them is that
none is discoverable from the feature list alone.

### §3.1 `tegra_early_stop` diverges — the desktop seam has nothing to attach to

```
main.rs:1902   fn tegra_early_stop(boot_info: &'static mut BootInfo) -> ! {
main.rs:190        tegra_early_stop(boot_info);
main.rs:6240       unaos_kernel::video::pidesk::activate()
```

`tegra_early_stop` is declared `-> !` and diverges. It is entered at `main.rs:190`
and never returns, so `kernel_main` never runs on the Orin — and
`pidesk::activate()` at `main.rs:6240` sits on the `kernel_main` path, behind the
`fbcon::detach()` handoff at `main.rs:1316`. The code in `main.rs` already knows
this and says so at `main.rs:2018`: "`tegra_early_stop` diverges before
`kernel_main` step 3 ever runs".

**Consequence:** the Orin's desktop seam must attach to the tegra flow. Porting
the *call* is not enough; the arming point has to be chosen inside
`tegra_early_stop`, after the heap and the scheduler are up.

### §3.2 One `cfg` term on a three-line wrapper excludes the Orin — and it is not removable

```
main.rs:6238   #[cfg(all(target_arch = "aarch64", feature = "pidesk", feature = "baremetal"))]
main.rs:6239   fn pidesk_activate_maybe() -> bool {
main.rs:6240       unaos_kernel::video::pidesk::activate()
main.rs:6241   }
main.rs:6246   #[cfg(all(target_arch = "aarch64", not(all(feature = "pidesk", feature = "baremetal"))))]
main.rs:6248   fn pidesk_activate_maybe() -> bool { false }
```

`main.rs:6238` is the **only** `pidesk` + `baremetal` pairing among the 137
`feature = "pidesk"` sites in the tree that is also a live wire-in (the other two
are its own `not(...)` twin at `:6246` and one witness fixture,
`video/wm.rs:17720`). Every other `pidesk` cfg site is arch-generic.

The obvious fix — drop `baremetal` — is not a style question, because the term is
**unsatisfiable** on tegra, not merely unset:

```
Cargo.toml:183                    baremetal = ["pi"]
arch/aarch64/serial.rs:22-23      #[cfg(all(feature = "pi", feature = "tegra"))]
                                  compile_error!("kernel features `pi` and `tegra` are
                                                  mutually exclusive — pick one board UART")
```

`baremetal` implies `pi`, and `pi` + `tegra` is a hard `compile_error!`. The
source comment above that assertion states the chain outright. So on every tegra
build `pidesk_activate_maybe()` resolves to the constant-`false` twin at
`main.rs:6248` — which the `arm-tegra` type-check confirms by reporting it as
dead code ("function `pidesk_activate_maybe` is never used").

**Consequence:** a tegra desktop needs its own arming wrapper on a `tegra`-shaped
gate, not a widened `baremetal` one.

### §3.3 The compositor's staging buffer is never allocated

```
video/mod.rs:257   pub fn init_panel(base: usize, len: usize, info: FrameBufferInfo) {
video/mod.rs:278       wm::reserve_stage(&info);
main.rs:1264           unaos_kernel::video::init_panel(framebuffer_addr as usize, …);
main.rs:2016           unaos_kernel::video::WRITER.lock().init(fb.base as usize, fb.len, fb.info);
```

`wm::reserve_stage` (`video/wm.rs:15232`) has exactly one caller: `init_panel`.
`init_panel` has exactly one caller: `main.rs:1264`, on the `kernel_main` path the
tegra boot never reaches. The tegra path seeds `WRITER` directly at
`main.rs:2016` instead — which gives it a framebuffer handle but not a staging
buffer.

This is load-bearing, not cosmetic. `video/mod.rs:268-277` records why the sizing
happens where it does: `wm`'s staged presents run inside `SYS_WIN_PRESENT`'s IRQ
mask, so a buffer that grew on the pass would be a masked acquisition of the
global heap `Mutex` — the F1–F5 defect family. **`reserve_stage` must be called
on the tegra path, after heap init, before any composite.**

Note also `wm::live_core_count`: WEDGE-12 M2 sizes one entry per live core, which
is why the Pi's call site sits after SMP bring-up. The tegra arming point
inherits that ordering constraint.

### §3.4 Nothing routes pointer events into the window layer

```
main.rs:2664   #[cfg(all(feature = "tegra", target_arch = "aarch64"))]
main.rs:2665   fn jd2_console_pump(_arg: usize) {
main.rs:2841       Event::Button(mask) => {
main.rs:2842           // Log clicks as a JD2 line for now (no UI action wired yet).
main.rs:2843           serial_println!(":: tegra: JD20 — pointer BUTTON {:#04x} (down) ::", mask);
```

The router it should call already exists on this arch and is not gated:
`arch/aarch64/syscall.rs:13722::wc_click_route` (at `3dc889e7`). Its furniture arm
is `#[cfg(feature = "pidesk")]` and its window arms are not. `wm::hit_test` and
`wm::focus_changed` carry no `#[cfg]` and are already compiled in.

So this is genuinely one call, not a port — **provided** the surrounding rungs
have made a hit-test meaningful (see the ordering constraint in §6).

### §3.5 `tegra` + the desktop family is type-checked by no cfg-matrix leg — and does not compile

`unaos/arroyo`'s `KERNEL_CFG_MATRIX` has nine board legs. `arm-pi`
(`unaos/arroyo:1828`) carries `pidesk,quarry,livecon` but also `pi,baremetal`.
The five tegra legs (`arm-tegra` `:1833`, `arm-tegra-el0` `:1896`,
`arm-tegra-simmer` `:1908`, `arm-tegra-xusbfw` `:1919`, `arm-tegra-smpmark`
`:1940`) carry none of the desktop family; the two x86 legs that do (`x86-all`
`:1820`, `x86-vsyncpace` `:1850`) are x86. The derived `x86-mix-N` legs are all
emitted against `x86_64-unaos.json` (`unaos/arroyo:2084`), and
`x86_cfg_universe` (`:2023`) removes any feature named only by an `arm-*` leg
from the universe (`:2043`) — which is `tegra` and `pidesk` both — so no mix leg
can cover it either.

Worse for the operator: **`pidesk`, `quarry` and `livecon` have no entry in
arroyo's top-level env→feature map at all.** They exist only inside `kernel8()`'s
curated `K8_FEATS` (`unaos/arroyo:3788, 3812, 3827`), which `check` never calls.
`UNAOS_PIDESK=1 ./arroyo check` therefore arms nothing. (`wc` at `:561` and
`wcg-paygo` at `:179` *do* feed `$KERNEL_FEATURES` and survive `arm_features`,
so `UNAOS_TEGRA=1 UNAOS_WC=1 ./arroyo check` is reachable ad hoc — but nothing in
the repo invokes it, and `battery` at `:4505` does not.)

This is exactly the hole arroyo documents at `:1770` ("Real holes wearing a green
verdict") and `:1776-1777` — the `tegra_el0`/EXECGATE precedent, in the script's
own words: swept onto 8 x86 legs, type-checked by none of them, and the armed
Orin build broke anyway (EXECGATE `89967799`, reverted the same night; see
`:1855`).

**Measured 2026-08-22 at `3dc889e7`, the complete error list:**

| Feature set | Errors |
| --- | --- |
| `arm-tegra` + `pidesk` | `video/dock.rs:204` and `:218` — `E0433: cannot find syscall in aarch64`. `dock::focus_set`/`focus_get` call `crate::arch::aarch64::syscall::user_input_set_active`/`user_input_active`, and `arch/aarch64/mod.rs:46-47` gates `pub mod syscall;` on `any(baremetal, tegra_el0)` |
| `arm-tegra` + `tegra_el0` + `pidesk` | `arch/aarch64/syscall.rs:14890` and `:14893` — `E0425: cannot find function dragperf_selftest / dragwedge_selftest in module crate::video::wm`. **Gate mismatch:** the call sites are `#[cfg(all(witness, pidesk))]`; the definitions (`video/wm.rs:17721`, `:17951`) are `#[cfg(all(witness, aarch64, baremetal, pidesk))]`. On a `pi` build `baremetal` is always on, so the mismatch has never been visible |
| `arm-tegra` + `tegra_el0` + `pidesk,quarry,livecon` | the two above, plus `video/quarry/live.rs:1089` — `E0433: cannot find boot in aarch64`. `quarry` reads `crate::arch::aarch64::boot::USER_REGION_SIZE`, and `arch/aarch64/mod.rs:11` gates `pub mod boot;` on `baremetal` |

All three are gate mismatches with known precedent. The `boot` one in particular
has its fix already built: JETSON-EL0 M1b introduced the `uslots` facade
(`arch/aarch64/mod.rs:76`) precisely so `syscall.rs` could name it instead of
`super::boot`; `quarry` needs the same substitution. Adding `tegra_el0` to the leg
resolves the `dock.rs` pair on its own.

### §3.6 No aarch64 render service on tegra

`main.rs:4686`, `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))] fn
render_service(…)`. Same `baremetal` → `pi` → `compile_error!` chain as §3.2: on
tegra this function does not exist. Per SHELLUP (`e8dcb09c`), `render_service` on
the Pi is the sole `GUI_CHANNEL.recv()` in the tree, the sole drain of
`serial::shell_inbox`, the only site that mints the shell window, and the only
caller of `pulsewin::service()`. The Orin will need an equivalent, and §5 is about
what happened to the Pi's.

*Measurement note:* `arch/aarch64/syscall.rs:3327` reads
`../../target/user_blob.bin` at compile time under `tegra_el0`. A detached
worktree has no such artifact; it was copied in before the §1.1 runs, and the
resulting error is absent from the table above because it is an artifact of the
measurement, not of the code.

---

## §4 The GA10B boundary — stated once so nobody re-asks

**Scanout on Tegra234 is nvdisplay + the DCE. That is a different block from the
GA10B iGPU. No rung on this ladder needs the GPU.**

The compositor blits with the CPU into DRAM and cleans to Point of Coherency;
the DCE keeps scanning that DRAM and does not snoop. This is the same recipe the
Pi uses against the HVS, and it is already proven on this seat — JD1/JD2/JD20
pixels reach the Orin panel today with no GPU involvement whatsoever, and
RAST-TEGRA's software rasterizer measured 90 frames / 91 ms on the 1920×1200
scanout (PARITY §8.3).

Two standing prohibitions apply to every rung:

| Prohibition | Why | Evidence |
| --- | --- | --- |
| **Do not probe powergated blocks** | a register read of a gated Tegra partition is EL3-fatal | the JX1 lesson, recorded at `arch/aarch64/display_tegra.rs:50-55` |
| **Do not touch nvdisplay MMIO** | inherit-don't-reinit; the DTB `simple-framebuffer` handoff proves pixels without betting on the power state | `arch/aarch64/display_tegra.rs:56`, `pub const JD1_DC_PROBE: bool = false;` — a default-off read-only survey, to be flipped only at the bench with the panel confirmed lit and the DTB handoff absent |

Mode-set, vsync and multi-head are DC-programming work that does not exist and is
not required. Vsync-accurate pacing on the DCE stays future work until someone
proves a safe non-powergated vblank source.

---

## §5 The inherited hazard — a stop-line on this ladder

The Pi hit a **16 KiB kernel-stack overflow in the desktop-arming cascade twice,
on 2026-08-22, on consecutive metal boots.** Both were convicted by the same
`[spin6]` refusal (`arch/aarch64/sched.rs:4798`), and neither reproduces on any
QEMU gate in this tree.

| Boot | Victim | Wire | Commit |
| --- | --- | --- | --- |
| 10 | `render` | `[spin6] … task=103:render ctx_sp=0x2089f80 outside its stack [0x208a000,0x208e000)` — 96–128 B below its own low bound. Corroborated on the same wire by `[u7stk] at=after:pidesk_arm hw=16496`, i.e. the desktop-arming subtree alone carries a high-water 112 bytes past 16 KiB | SHELLUP `e8dcb09c` |
| 11 | `usb-pump` | `[spin6] cpu=3 REFUSING corrupt switch-in: task=98:usb-pump ctx_sp=0x207df00 outside its stack [0x207e000,0x2082000)`, then a synchronous exception at `ELR=0x1228` and a dead core. On glass: the quarry close/reopen wedge. Path is `quarry::open()` running synchronously at click-router depth on the input-drain task | STACKPOOL `71671423` |

**Why the gate cannot catch it**, in both commits' own words: the overflow needs a
preemption frame on an already-deep pass, and timer delivery on that path is
metal-only, so QEMU never preempts the task. STACKPOOL puts it structurally —
raspi4b delivers no timer IRQ on this path, so no gate in this tree can stack the
frame that produces the overrun. Three Pi arcs gated green on a path Pi hardware
does not take.

### §5.1 What the Orin has, and what it does not

Verified at `3dc889e7` by `git merge-base --is-ancestor` and `git grep`:

| Item | On `hw-jetson` @ `3dc889e7`? | Note |
| --- | --- | --- |
| `TASK_STACK_SIZE = 16 * 1024` | ✅ `arch/aarch64/sched.rs:41` | the blanket size both faults were charged against |
| `[spin6]` corrupt-switch-in refusal | ✅ `arch/aarch64/sched.rs:4798` | the detector is here |
| `[u7stk]` stack high-water probe | ✅ `arch/aarch64/sched.rs:87` (`stk_probe`), format string at `:131`, `witness`-gated | arrived via `50ec4ac0` (U7STK M1), which **is** on trunk |
| `[u7stk] at=after:pidesk_arm` checkpoint | ✅ `arch/aarch64/syscall.rs:15760` | the exact probe that convicted boot 10 |
| `#[inline(never)]` u7-launch frame fix + `U7_LAUNCH_STACK_SIZE = 32 KiB` | ✅ `main.rs:734-756` | |
| `[shellup]` desktop-tenancy census | ❌ 0 hits tree-wide | SHELLUP only |
| `[u7stk] at=render:pass` checkpoint | ❌ | SHELLUP only |
| `sched::spawn_prio_stack` | ❌ | SHELLUP only — the `spawn_stack` × `spawn_prio` cross |
| `RENDER_STACK_SIZE = 32 KiB` | ❌ | SHELLUP only |
| 32 KiB sizing for `usb-pump` / `input` | ❌ | STACKPOOL only |
| SHELLUP `e8dcb09c` | ❌ not an ancestor of HEAD | **is** on `origin/hw-pi4`; **not** on `origin/main` |
| STACKPOOL `71671423` | ❌ not an ancestor of HEAD | **not on origin at all** — it exists only on the local `hw-pi4` worktree branch, two commits ahead of `origin/hw-pi4` (`38f2ecbb`). It cannot be fetched today |

> ⚠️ **The instrument on this branch is the one STACKPOOL convicted as lying.**
> `arch/aarch64/sched.rs:82-83` at `3dc889e7` documents `[u7stk]`'s `headroom` as
> going negative once a chain has run off the bottom, "printed signed for exactly
> that reason". It cannot: the high-water scan starts at `base`, so `hw <= len`
> and `headroom >= 0` always. A chain 256 B past its floor and one that stopped
> exactly at it print the identical `hw=16384 headroom=0`. STACKPOOL corrected
> that doc in place and added a saturating note. **On this branch the correction
> is absent, so a `headroom=0` reading here means "at or past the floor, amount
> unknown" — never "exactly zero left".**

### §5.2 The stop-line

The Orin has inherited the cascade *and* the detector, but not the fixes and not
the two instruments that name the failure mode.

- **Below the line — safe now:** rung 0, one composited window, no furniture, no
  cascade. It mints one row, presents it, and arms nothing. Neither fault's path
  is entered.
- **At and above the line — blocked:** arming the full desktop (rung 5 and the
  furniture half of rung 3) drives the same cascade that overflowed twice on Pi
  metal, on the same blanket 16 KiB, with a probe whose `headroom` reading is
  known to saturate.

**Precondition, explicit:** do not arm the full desktop cascade on Orin until
SHELLUP and STACKPOOL arrive — via trunk sync, or via a cherry-pick agreed with
the pi seat. STACKPOOL additionally has to be **pushed** before it can be either;
raise that with the pi seat before planning around it.

---

## §6 The ladder

Seven rungs, each commit-sized, each with the witness that closes it. "Lane"
names the seat that owns the files under the parallel-arc rules in `CLAUDE.md`.

| # | Rung | What lands | Metal witness | Lane |
| --- | --- | --- | --- | --- |
| **0** | **One composited window** | call `wm::reserve_stage` on the tegra path after heap init (§3.3); mint one `wm` row; present it. No furniture, no `pidesk`, no cascade | one window visible on the Orin panel over the JD2 console; `wm` present counters non-zero on the wire | jetson |
| **1** | **The cfg leg** | fix the three gate mismatches in §3.5; add an `arm-tegra-desk` leg to `KERNEL_CFG_MATRIX`; add `pidesk`/`quarry`/`livecon` to arroyo's env map or to an `esp-jetson` curated list | `./arroyo check` green with the new leg — i.e. the combination is type-checked by something for the first time | jetson (arroyo + `video/` gates; `arch/aarch64/syscall.rs` needs the pi/rmbp seat's agreement) |
| **2** | **The desktop seam** | a tegra-shaped arming wrapper inside `tegra_early_stop` (§3.1, §3.2), replacing the unsatisfiable `pidesk`+`baremetal` gate at `main.rs:6238` | `pidesk::activate()` runs on an Orin boot and its floors print their verdicts | jetson |
| **3** | **Input routing** | `jd2_console_pump`'s `Event::Button` arm calls `wc_click_route` instead of `serial_println!` (§3.4) | a click on the Orin panel raises and focuses a window; `[clickroute]` on the wire | jetson |
| **4** | **Console as a window** | route the JD2 console into a `wm` row; `fbcon::console_is_routed`; skip the handoff detach when routing succeeded | the boot log keeps updating *inside a window*, and the minimise control has somewhere to go back to | jetson |
| **5** | **The real desktop** | dock, strip, menubar, crystal armed; the full `pidesk` cascade; a tegra `render_service` (§3.6) | the Orin comes up to a desktop | jetson — **blocked by §5.2** |
| **6** | **EL0 tenants** | user windows from EL0 through `SYS_WIN_*`, on the `tegra_el0` regime | an EL0 program owns a window on the Orin panel | jetson |

### §6.1 The ordering constraint, and why it is not negotiable

**Rung 3 (input routing) must land before rung 4 (console as a window).**

The reason is written into the seam itself. `video/pidesk.rs:39-44` states the
CONSOLEWIN law, inherited unchanged from `wcx`:

> the console window carries a minimise disc; the only route back from that park
> is the dock; `dock::Layout::for_panel` returns `None` when the strip will not
> fit at `MAX_WINDOWS` rows; **a control that hides a window with no way back is
> worse than no control**, so a panel that cannot guarantee the dock gets no
> console window.

The dock is only a way back once clicks route. Land rung 4 first and the Orin
ships a console window whose minimise button is a one-way trip — strictly worse
than the full-screen console it replaced. `pidesk.rs` enforces the panel-geometry
half of that law at runtime; the routing half is an ordering obligation on this
ladder, because no `#[cfg]` can express it.

Rungs 0–2 have no ordering constraint among themselves beyond the obvious (rung 1
before rung 2 if the seam is to be type-checked by anything). Rung 5 is gated on
§5.2, not on rung 4.

---

## §7 What this document does not claim

- **No rung is claimed done.** Rung 0 is not started. Every PROVEN cell in §1 that
  is ✅ refers to the JD1/JD2/JD20 panel path, not to the compositor.
- **The COMPILES column is a type-check, not a link or a boot.** `./arroyo check`
  green proves nothing about what the builder's env→feature map actually puts in
  the image — the full-knob rule in `docs/dev/LAWS.md` requires a `strings` check
  on the artifact, and no rung here has earned one.
- **Rung sizing is an estimate.** "Commit-sized" is a judgement about scope, not a
  measurement. Rung 1's error list is exhaustive as measured; whether fixing those
  three reveals a fourth is unknown until it is run.
- **The `[u7stk]` numbers quoted in §5 are Pi numbers.** The Orin's stack
  high-water on its own cascade has never been measured. `[u7stk]` is present and
  `witness`-gated here, so it can be — but until it is, §5's bound is inherited,
  not local.
