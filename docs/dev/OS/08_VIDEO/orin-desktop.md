# orin-desktop.md — the Jetson Orin Nano desktop ladder

Scope: what the window compositor already is on the `hw-jetson` track, what stops
it reaching the panel, and the commit-sized rungs from here to a real desktop.

**Baseline: `hw-jetson` @ `3dc889e7`**, surveyed and measured 2026-08-22
(ORINDESK). **Flight results folded in 2026-08-25 at `04d46aae`** — boot7f took rungs 0 and 3 to
metal; see §3.8, which is the load-bearing status update and which the older §1/§3.7/§6/§7 text is
annotated against wherever it now reads stale. Companion to [`PARITY.md`](PARITY.md) §8, whose §8.0 headline this
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
| Window manager core | `video/wm.rs` (20 652 lines) | ✅ | ✅ `video/mod.rs:46` declares `pub mod wm;` **unconditionally** | ✅ **non-trivially since boot7f** — one row minted and composited (§3.8); `main.rs:2026`'s `wm::retile_on_ready()` no longer walks an empty table | ✅ **boot7f 2026-08-25**, on the wire: `[orinwm1] … present=Composited -> COMPOSITED` (§3.8). On-glass NOT claimed |
| Compositor staging buffer | `wm::reserve_stage`, `video/mod.rs:278` | ✅ | ✅ | ✅ **since rung 0** — called on `tegra_early_stop`'s own heap line | ✅ **boot7f**: `stage=4194304` (4 MiB = `MAX_STAGE_BYTES`) on the `[orinwm1]` line |
| Hit-test / focus primitives | `wm::hit_test` `video/wm.rs:2434`, `wm::focus_changed` `:2514` | ✅ | ✅ — **no `#[cfg]` on either** | ⚠ called only from `orin_click`, i.e. only on a click that has not yet happened (§3.8) | ❌ no click has reached the router |
| Click router | `arch/aarch64/syscall.rs::wc_click_route` | ✅ | ✅ ungated within its module | ⚠ **has a caller since rung 3, behind `orinclick` (DEFAULT OFF)** — `orin_click` → `bl wc_click_route`, proven by disassembly | ⚠ **FLOWN AND ARMED, ROUTE UNTESTED** — boot7f carried the knob and printed `-> ARMED`, but no click was made (§3.8) |
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
| Pointer **routing** | `arch/aarch64/display_tegra.rs::orin_click` (rung 3) | ✅ | ✅ `orinclick` (implies `tegra_el0`) — leg `arm-tegra-orinclick` | ✅ **reached on boot7f** — the arm line printed from inside the pump; `main.rs:2852` still logs the button and hands the same edge to the router | ⚠ **ARMED, UNTESTED** — boot7f: `[orinclick] … -> ARMED` then 48 consecutive `IDLE-NO-CLICKS` censuses. The instrument is alive; the *route* has no evidence either way (§3.8) |

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
   ⚠ **UPDATED 2026-08-25 (boot7f).** On an `orinclick` image the caller exists and runs: the
   router armed from inside the pump and the census has been printing ever since. What is still
   missing is not a caller but an *event* — nobody has clicked on the Orin. See §3.8.

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

#### §3.2.1 RESOLVED 2026-08-25 (rung 2) — the seam LANDED, and it REFUSES

Measured at `f0106408`. The wrapper exists, is compiled, is reached on a tegra
boot, and declines with a named reason. It does **not** arm the desktop.

| Item | Where | State |
| --- | --- | --- |
| `tegradesk` Cargo feature | `crates/kernel/Cargo.toml` (file tail) | landed — `tegradesk = ["pidesk", "tegra_el0"]` |
| `tegra_desk_arm` wrapper | `crates/kernel/src/main.rs` (file tail) | landed, `#[cfg(all(target_arch = "aarch64", feature = "tegradesk"))]` |
| Call site | `main.rs`'s `tegra_early_stop` terminus line, appended statement | landed — `bl` proven by disassembly, below |
| `UNAOS_TEGRADESK=1` env map | `unaos/arroyo` | landed |
| `arm-tegra-seam` matrix leg | `unaos/arroyo` `KERNEL_CFG_MATRIX` | landed — 11 → 12 board legs (counted, not inherited) |
| `main.rs:6257 pidesk_activate_maybe` | unchanged | it is the live wire-in on a `pidesk`-armed Pi build (`main.rs:1316`, on the `kernel_main` path the Pi reaches and the Orin does not). This rung adds a wrapper BESIDE it; it does not widen, move or delete it. Its dead-code warning on a tegra build is still emitted, and is still the correct diagnosis |

**The gate.** `tegradesk = ["pidesk", "tegra_el0"]` — it IMPLIES rather than
standing alone, unlike `orindesk`/`jd1dc`/`smpmark`, and that is the rung's own
lesson rather than a convenience. The seam calls `video::pidesk::activate`
(`#[cfg(all(aarch64, pidesk))]`), and `pidesk` on aarch64 needs
`arch::aarch64::syscall` via `dock::focus_set`, gated `any(baremetal,
tegra_el0)` — `baremetal` being unsatisfiable here by §3.2's chain, the only
term left is `tegra_el0`. A standalone spelling would have let
`UNAOS_TEGRADESK=1 UNAOS_TEGRA=1` compile the seam's ABSENCE and report green,
which is the same "gate its own knob cannot satisfy" defect this rung exists to
remove.

**The call site, and why it is the terminus line.** §3.1 concludes the arming
point "has to be chosen inside `tegra_early_stop`, after the heap and the
scheduler are up". The statement is appended to the line that ends
`tegra_early_stop` — `el1_oneshot_proof(); tegra_el0_start_maybe();
tegra_desk_arm(); tegra_rast_demo_maybe(); run_capstone_boot_core(0);` — the
last instruction on this path, because everything the cascade needs holds there
and nowhere earlier: panel seeded (JD1), heap carved (step 3c), IRQs live (JM4),
SMP secondaries kicked off (JM5 — `wm::reserve_stage` sizes one stage entry per
LIVE core, so §3.3's ordering constraint is inherited), EL2 → EL1 dropped,
`percpu`/`mark_el1_core` stamped, run queue populated but not driven. It is the
tegra counterpart of the Pi's `main.rs:1316` GUI-handoff line, and it mirrors
that line's ordering: seam before the RAST demo. It also runs on the **boot
stack** rather than a 16 KiB `TASK_STACK_SIZE` task stack, which is a better
stack story than the Pi's — and is NOT a claim that the cascade fits.

**What it prints.** Six `[deskseam]` strings in the ARMED artifact — one census
and five refusals — every term derived from a value read on that boot, none
asserted. (A seventh exists in source, the `ARMED panel=…` success line; it is
eliminated with the branch the stop-line closes, and `LC_ALL=C grep -a -o` on
`unaos-kernel` confirms it is absent. That is the intended reading: the strings
in the image are exactly the verdicts this build can reach.)

| Wire | Derived from | What makes it print |
| --- | --- | --- |
| `REFUSE reason=already-armed` | `TEGRADESK_ENTERED.swap` | a second entry to the seam |
| `REFUSE reason=no-panel` | `FrameBuffer::is_ready()` | headless Orin — no DTB `simple-framebuffer`, JD1 printed `JB1b — geometry unresolved` and never seeded `WRITER` |
| `REFUSE reason=stage-unreserved stage=0` | `wm::reserve_stage(&info)` | the 48 MiB aarch64 heap cannot spare stage entry 0 |
| `floors panel=…x…x… stage=… cover=…% table=… console-window=GRANTED\|WITHHELD route=ROUTED\|UNROUTED click-route=… cascade=… rows=12` | all of the above + `wm::count()` + `dock::Layout::for_panel(MAX_WINDOWS, pw, ph)` + `fbcon::console_is_routed()` | every boot that clears the three floors |
| `REFUSE reason=table-not-empty live=N` | `wm::count()` | **`UNAOS_ORINDESK=1 UNAOS_TEGRADESK=1`** — rung 0's row is already composited when the seam runs, and `pidesk`'s step-1b DESKTOP-CLEAR writes the whole panel through the FRONT buffer on the stated premise that the window table is empty. On the Pi that premise holds by construction; on the Orin it does not |
| `REFUSE reason=stop-line-5.2` (was `rung3-unlanded+stop-line-5.2`) | `match (TEGRADESK_CLICK_ROUTED, TEGRADESK_CASCADE_OK)` | every boot this branch can build. **⚠ CHANGED BY RUNG 3 (§3.7):** `TEGRADESK_CLICK_ROUTED` is no longer `false` — it is `cfg!(feature = "orinclick")`, so a `tegradesk`+`orinclick` build now refuses naming only the STACK hazard, and a `tegradesk`-without-`orinclick` build still refuses `rung3-unlanded+stop-line-5.2` and is telling the truth about itself. Both strings stay live across builds; §3.7 re-ran the artifact grep for the one each build carries |

**Two instrument defects were found by checking the ARTIFACT, and both are
recorded because neither was visible in the diff or in `./arroyo check`.**

1. A `REFUSE reason=zero-geometry` floor was written, hoisting `pidesk`'s own
   step-1 `pw == 0 || ph == 0` test. `LC_ALL=C grep -a -o` found every other
   `[deskseam]` string in the armed `unaos-kernel` and not that one:
   `FrameBuffer::is_ready` is `base != 0 && len != 0 && info.width != 0 &&
   info.height != 0`, so the extent test is already inside the readiness test and
   the optimiser had proved the arm dead. **Removed.** A DECLINE arm that cannot
   be reached is not a floor.
2. The two stop-lines were written as sequential `if !CONST { … return }` blocks.
   The artifact carried `rung3-unlanded` and **not** `stop-line-5.2` — the
   second string was dead code the moment the first const was `false`, so an
   operator's capture would have named the ordering obligation and been SILENT
   about the stack hazard, which is the more dangerous of the two. **Merged into
   one `match` over both terms**, whose `(false, false)` arm is the string the
   build actually carries.

**The stop-line is enforced by CODEGEN, not merely by a runtime test** — a
stronger statement than intended and worth recording. `llvm-nm` finds no
`pidesk::activate` symbol in the armed image and `llvm-objdump -d` finds no
reference to it anywhere: because both consts are `false` in source, the armed
branch is eliminated before linking. `pidesk::activate()`'s call is therefore
type-checked **only** by the `arm-tegra-seam` leg, which is exactly what that leg
is for.

**Verification, all at `f0106408` + this arc's diff — and all of it re-run in an
ISOLATED THROWAWAY WORKTREE**, `git worktree add --detach` at `f0106408` with
only this arc's four files copied in. That is not ceremony: a peer executor's
in-flight SHELLUP cherry-pick appeared in `arch/aarch64/sched.rs` in the shared
`../UnaOS-orin` tree at 12:07:38 while this arc's armed build and gates were
running, and `sched.rs` compiles into every aarch64 image. A measurement taken
across another seat's uncommitted edit is not a measurement of this arc. (Never
`git stash` to get the clean baseline — one stack, shared across every worktree
of this repo.)

- `UNAOS_TEGRA=1 ./arroyo check` — **exit 0**. `✅ kernel cfg coverage OK
  (20 legs)`, `12 board + 8 x86 pairwise-mix`, `✅ arm-tegra-seam`. The board
  count is the +1 stated above, read off the run rather than asserted.
- `UNAOS_TEGRADESK=1 ./arroyo check` — **exit 0**, same 20 legs, and the banner
  confirms the knob maps through:
  `⚡ kernel features: ehcihid,kbdwit,sdhcblk,smolnet,tegradesk,pidesk,tegra_el0,tegra`.
- `./arroyo test-arm` — **exit 0**, `✅ aarch64 test complete`. `awk '/PANIC|panicked/'`
  over `target/serial-arm.log` → 0 lines; the single `/FAIL/` hit is prose inside
  a `[botclaim]` explanation, not a verdict. **This proves nothing about the
  cascade** and the seam's own refusal line says so on the wire: `test-arm` is
  the aarch64 *virt* machine, which compiles no `tegra` and no `tegradesk`
  (`awk '/deskseam/'` → 0 lines). It is a no-regression gate, not a witness.
- **`arm-tegra-seam` proven to GO RED**, because a leg that has never gone red is
  not evidence. Renaming the seam's central call
  (`pidesk::activate()` → `pidesk::activate_now()`) and re-running the gate reds
  **exactly one leg**:

  ```
  ✅ arm-pi   ✅ arm-tegra   ✅ arm-tegra-el0   ✅ arm-tegra-simmer
  ✅ arm-tegra-xusbfw   ✅ arm-tegra-smpmark   ✅ arm-tegra-orindesk
  ✅ arm-tegra-jd1dc   ✅ arm-tegra-desk
  error[E0425]: cannot find function `activate_now` in module `pidesk`
  ❌ arm-tegra-seam — unaos-kernel FAILED to compile
  ❌ kernel cfg coverage FAILED (crate unaos-kernel), legs: arm-tegra-seam
  ```

  (arm-* board legs quoted; the two x86 board legs and all eight mix legs stayed
  green as well — the failure line names one leg and it is the new one.)

  `arm-tegra-desk` stays green (it carries `pidesk` and `tegra_el0` but no
  `tegradesk`, so it compiles the callee and not the caller) and `arm-pi` stays
  green (it carries the whole desktop family, on `baremetal`, with no tegra
  caller at all) — which is the point: **no pre-existing leg can see a defect in
  the Orin's desktop-arming call site**, and the new one names both the defect
  and its configuration. Restored → green again.
- **Byte-identity, knob-off, at the KERNEL level** (never the ESP — that embeds
  `SRC.TGZ`, a tarball of the working tree, so an ESP-level compare reports the
  diff itself as a codegen change). Two independent `CARGO_TARGET_DIR`s, same
  source path, `--features ehcihid,tegra,tegrasmp` (the shipped default jetson
  set), `llvm-objcopy -O binary` for the loadable image:

  | Artifact | Baseline `f0106408` | With this arc, knob off | |
  | --- | --- | --- | --- |
  | `unaos-kernel` (kernel.elf) | `b5ffbb7e…14f8`, 1 866 824 B | `b5ffbb7e…14f8` | **identical** |
  | its `-O binary` flattening | `cc0a0693…147f`, 1 526 560 B | `cc0a0693…147f` | **identical** |

  Control of the control, run first: the same pristine tree built into the two
  target dirs produced the same `b5ffbb7e…14f8`, so the method has no
  target-dir sensitivity to hide behind.

- **Reachability by disassembly**, not by linkage:

  ```
  0000000000071250 <unaos_kernel::tegra_early_stop>:
     71adc:   bl  0x709f4 <unaos_kernel::tegra_desk_arm>
  ```

  and inside `tegra_desk_arm`, the floors themselves:
  `bl <video::wm::reserve_stage>`, `bl <video::wm::count>`,
  `bl <video::fbcon::console_is_routed>`, and two `bl`/one tail-`b` to
  `arch::aarch64::serial::_print`. `dock::Layout::for_panel` is **inlined**
  rather than called, and const-folds to two comparisons —
  `cmp x21, #0x4c` / `ccmp x20, #0x240, hs` / `cset w20, lo`, i.e.
  `ph >= 76 && pw >= 576` — which then selects the literal `GRANTED` (7 bytes at
  `.rodata` `0x341d8`) or `WITHHELD` (8 bytes at `0x340c0`). The `route=` term
  selects `ROUTED` (`0x341df`) / `UNROUTED` (`0x33fe8`) off
  `console_is_routed`'s return in the same shape. Both census terms are
  therefore derived at the machine-code level, not only in source.

- *Measurement artifact, not a code defect, and §3.6 predicted it:* the first
  armed build in the isolated worktree failed with `couldn't read
  …/target/user_blob.bin` at `arch/aarch64/syscall.rs:3327`. `tegradesk` implies
  `tegra_el0`, which compiles that `include_bytes!`, and a fresh worktree has no
  such artifact. Staged in before the run, exactly as §1.1 did. `./arroyo check`
  supplies it itself via `ensure_user_blob`, so only the hand-rolled `cargo
  build` needs the step.
- Every new string proven NEW to this build, not inherited:
  `git log --all -S` returns **0** commits for each of `[deskseam]`,
  `tegra_desk_arm`, `tegradesk`, `arm-tegra-seam`, `TEGRADESK_CLICK_ROUTED`,
  `reason=table-not-empty`, `reason=stage-unreserved`, `reason=rung3-unlanded`,
  `reason=stop-line-5.2`.

**NOT done by this rung, deliberately:**

- **`pidesk::activate()` has not run and cannot run on this branch.** The rung-2
  row in §6 gave its metal witness as "`pidesk::activate()` runs on an Orin boot
  and its floors print their verdicts". Those are two different claims and only
  the second is achievable now: `activate()` opens the console window (rung 4)
  and enables the menu bar (furniture), so running it crosses **both** §6.1's
  non-negotiable ordering constraint **and** §5.2's stop-line. The §6 row is
  corrected accordingly. The floors do print their verdicts.
- **No metal.** UNFLOWN. No Orin boot has been taken with `UNAOS_TEGRADESK=1`;
  every claim above is a build-time or artifact measurement.
- **The `WITHHELD` arm is live in codegen but not demonstrated reachable on Orin
  hardware.** The fold above puts the dock floor at `pw >= 576 && ph >= 76`, and
  no Orin panel this branch has seen is under it. It is reported as a census
  term rather than as a refusal for that reason, and because a withheld console
  window is not fatal to `pidesk` — the bar follows either way.
- **The seam measures no stack headroom.** §5's whole hazard is a stack hazard
  and §7 already records that every number quoted for it is a Pi number. The
  boot stack's extent is not exposed by any symbol reachable from `main.rs` on
  the tegra path, and a guessed extent would be a worse instrument than none.
  This stays owed by the rung that flips `TEGRADESK_CASCADE_OK`.
- **No `video/` edit.** None was needed.

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

**RESOLVED 2026-08-25 (rung 3), as a DEFAULT-OFF knob — see §3.7.** It was one call: the
caller is appended to the JD20 statement at `main.rs:2852` behind `orinclick`, and no `video/`
edit was needed. The line numbers quoted above are the pre-rung-3 ones; the JD20 log line is
still there (kept deliberately, and kept line-neutral), with the routing call appended to it.
Nothing on any board has run it.

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

#### §3.5.1 RESOLVED 2026-08-22 (rung 1) — what landed, and what did not

Re-measured at `fdbc0dfc` before any edit: all three errors reproduced exactly as
tabulated above (`dock.rs:204`/`:218`; `quarry/live.rs:1089`; the `dragperf`/
`dragwedge` pair, at `arch/aarch64/syscall.rs:14935`/`:14938` on this tip rather
than the `3dc889e7` line numbers the table quotes). Two of the three are fixed and
the armed configuration now type-checks; the third is a `video/` edit and is held.

| Error | Disposition |
| --- | --- |
| `video/dock.rs:204`, `:218` — `syscall` not found | **Fixed by the leg.** The new `arm-tegra-desk` leg carries `tegra_el0`, which is the only satisfiable way to give aarch64 `pidesk` the `arch::aarch64::syscall` module (`baremetal` implies `pi`, and `pi` + `tegra` is a `compile_error!`). No source change |
| `arch/aarch64/syscall.rs` — `dragperf_selftest` / `dragwedge_selftest` not found | **Fixed at the CALL SITE, not the definition.** Both call sites gained `feature = "baremetal"`, matching the gate the definitions already carry. This is the conservative side: `video/wm.rs:17930-17941` states outright that the definitions' `target_arch`/`baremetal` gate exists because leg 1 drives *the shipped Pi router*, so the definitions' gate is deliberate and the call sites' was the oversight — invisible on Pi, where `baremetal` is always on. Line-neutral, and both edits are inert on Pi (see the identity measurement below) |
| `video/quarry/live.rs:1089` — `boot` not found | **NOT fixed — out of lane.** The fix is one line and is written out in §3.5.2, but `video/` is a shared lane. `quarry` is therefore absent from the `arm-tegra-desk` leg and `UNAOS_QUARRY=1` on a tegra build is still E0433 |

**The leg.** `arm-tegra-desk` = `arm-tegra-el0`'s feature list verbatim +
`pidesk,livecon`. `KERNEL_CFG_MATRIX` goes 10 → 11 board legs and the gate 18 → 19
legs (the mix-leg count is untouched: `pidesk` was already arm-only via `arm-pi`,
and `livecon` is already in `x86_cfg_universe` via `x86-all`, so the universe does
not move). `UNAOS_TEGRA=1 ./arroyo check` → green, 19/19.

**The go-red proof**, because a leg that has never gone red is not evidence.
Re-introducing the `dragperf` mismatch alone (dropping `feature = "baremetal"` from
that one call site) and re-running the gate reds **exactly one leg**:

```
✅ arm-pi   ✅ arm-tegra   ✅ arm-tegra-el0   ✅ arm-tegra-orindesk   ✅ arm-tegra-jd1dc
error[E0425]: cannot find function `dragperf_selftest` in module `crate::video::wm`
❌ arm-tegra-desk — unaos-kernel FAILED to compile
   configuration: --target ../../aarch64-unaos.json --features …,tegra_el0,pidesk,livecon
❌ kernel cfg coverage FAILED (crate unaos-kernel), legs: arm-tegra-desk
```

`arm-pi` stays green (its `baremetal` satisfies the definition) and `arm-tegra-el0`
stays green (its `pidesk` is off) — which is the point: no pre-existing leg can see
this defect, and the new one names it and its configuration. Restored → green again.

**Identity, measured and not argued** (throwaway worktree at `fdbc0dfc`, same path
for both builds, `llvm-objcopy -O binary` then `cmp`):

| Image | Loadable image | `.elf` |
| --- | --- | --- |
| jetson disarmed (`tegra,tegrasmp`) | **identical**, 1 524 304 B | **identical** — `syscall.rs` is not compiled at all on this build |
| jetson armed-EL0 (`tegra,tegrasmp,tegra_el0`) | **identical**, 1 723 078 B | 16 B shorter, `.strtab` only |
| Pi (`baremetal,skip_xhci,witness,pidesk,quarry,livecon`) | **identical**, 2 144 632 B | 20 B shorter, `.strtab` only |

In both `.elf` cases `readelf -S`/`-l` show every other section and every program
header unchanged in size and address — the documented benign `.llvm.<hash>`
internal-symbol-suffix class, the same one the JB11 note in `arroyo` records. The
Pi row matters because `arch/aarch64/syscall.rs` **is** compiled into `kernel8.img`:
the change is line-neutral, so no panic `Location` moves, which is the discipline
PI-DESK's own byte-identity note in `arroyo` establishes.

#### §3.5.2 The held `video/` patch — `quarry`'s `uslots` substitution

One line, and it is the *verbatim* shape of the JETSON-EL0 M1b migration already
sitting at `shell.rs:4388` (same `const CAP`, same facade, same trailing comment):

```diff
--- a/unaos/crates/kernel/src/video/quarry/live.rs
+++ b/unaos/crates/kernel/src/video/quarry/live.rs
@@ -1086,7 +1086,7 @@ fn launch(path: &str) -> String {
-    const CAP: u64 = crate::arch::aarch64::boot::USER_REGION_SIZE as u64;
+    const CAP: u64 = crate::arch::aarch64::uslots::USER_REGION_SIZE as u64; // JETSON-EL0: uslots facade (boot.rs on pi / mmu_tegra_el0.rs on tegra)
```

Verified in a throwaway worktree, not merely reasoned: with it applied the
`arm-tegra-desk` list **plus `quarry`** type-checks green, `arm-pi` (which carries
`quarry` and `baremetal`) stays green, the x86 `quarry` leg stays green, and the Pi
loadable image is byte-identical to the same build without it — `uslots` re-exports
`boot::*` under `baremetal`, so `USER_REGION_SIZE` resolves to the same constant.

When it lands, **append `,quarry` to the `arm-tegra-desk` leg** (the leg carries a
comment saying so). A family leg missing a member is the same silent hole the leg
exists to close.

#### §3.5.3 Corrections to this section

- **§3.5.1's `quarry` row and §3.5.2's "held" framing are STALE, and both are
  superseded.** Re-measured 2026-08-25 at `f0106408`: the `uslots` substitution
  landed at `e14a9008` (`tegra: EL0CORE-CITE + ORINDESK-QUARRY`, verified an
  ancestor of HEAD by `git log -S "uslots::USER_REGION_SIZE" HEAD --
  …/video/quarry/live.rs`), `video/quarry/live.rs:1089` now reads
  `crate::arch::aarch64::uslots::USER_REGION_SIZE`, and `arm-tegra-desk` carries
  `,quarry` as §3.5.2 instructed. §3.5.1's row saying "NOT fixed — out of lane"
  and §3.5.2's title calling the patch "held" are both false as of that commit;
  they are left in place as the record of why the substitution was written the
  way it was, not as current state. **`arroyo`'s `UNAOS_QUARRY` env-map comment
  carries the same stale claim** ("⚠ NOT YET TYPE-CHECKED ON tegra, and
  deliberately absent from the `arm-tegra-desk` leg below") and was corrected in
  the same turn.

- **`x86-vsyncpace` does not carry the desktop family.** §3.5 lists it beside
  `x86-all` as one of "the two x86 legs that do". Its feature list is
  `nvidia-kepler,nvidia-kepler-takeover,wc,witness,vsyncpace,ehcihid,smolnet` — no
  `quarry`, no `livecon`, no `pidesk`. It reaches the furniture only through `wc`,
  which is the x86 arm of the `any(all(x86_64, wc), all(aarch64, pidesk))` gate.
  The section's conclusion is unaffected (both are x86 legs and neither can cover
  `tegra`), but the reason is `wc`, not the family knobs.
- **`quarry` and `livecon` are *in* `x86_cfg_universe`, `pidesk` is not.** §3.5 says
  the universe drops "`tegra` and `pidesk` both", which is right as far as it goes;
  the other two are named by `x86-all` and so ride the eight mix legs. That is real
  coverage of their x86 arm and no coverage at all of their aarch64 arm, which is
  why the new leg is the first thing to compile either of them on `tegra`.
- **Board-leg count and line numbers have drifted.** §3.5's "nine board legs" and
  its `:1828`/`:1833`/`:1896`/`:1908`/`:1919`/`:1940` citations were true at
  `3dc889e7`; `arm-tegra-orindesk` (rung 0), `arm-tegra-jd1dc`, `arm-tegra-desk`
  and now `arm-tegra-orinclick` (rung 3) have been appended since — **12 board
  legs and a 20-leg gate as of 2026-08-25.** Count the array, do not trust the
  line, and do not trust this number either: it has moved four times in three days.

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

### §3.7 RESOLVED 2026-08-25 (rung 3) — input routing, DEFAULT OFF and UNFLOWN

> ⚠ **STATUS SUPERSEDED the same day — see §3.8.** The "UNFLOWN" in this heading and every
> "nothing here has run on any board" below describe the arc as it landed. boot7f then flew the
> knob: the router armed and the census ran for 480 s. What remains unflown is the *click*, not the
> code. The reasoning in this section is unchanged and still correct; only the status is stale.

**Measured against `088d17c1` (rung 2's tip), not `f0106408`.** This arc opened on `f0106408`;
rung 2 landed underneath it mid-flight, so every count, hash and gate result below was RE-RUN on
the new base and the earlier numbers are discarded rather than carried.

§3.4 said the Orin was the one arch where the click router existed, compiled, and had no
caller. It has one now, behind `orinclick`. **Every verb below is matched to its evidence:
this rung LANDED and COMPILES; nothing here has run on any board.** QEMU models no
Tegra234, so no gate in this tree can boot the armed configuration — the metal witness is
owed, not deferred-and-forgotten.

#### What landed

| Piece | Where | Note |
| --- | --- | --- |
| The caller | `main.rs:2852`, appended to the JD20 log statement | `#[cfg(feature = "orinclick")] unaos_kernel::arch::display_tegra::orin_click(mask);` |
| The census hook | `main.rs:2888`, appended to the phase-2 `vugras::idle_sweep` statement | `orin_click_census(sweep_tick)` — the ~250 ms cadence the pump already runs |
| `orin_click` / `orin_click_census` + 14 statics | `arch/aarch64/display_tegra.rs`, **file-tail block** | the whole routing decision is `wc_click_route`'s; this adds no arm and no policy |
| The knob | `Cargo.toml` `orinclick = ["tegra_el0"]`; `arroyo` `UNAOS_ORINCLICK` | self-sufficient — `tegra_el0` implies `tegra` |
| The leg | `arm-tegra-orinclick` (`arm-tegra-el0`'s list + `orinclick`), and `,orinclick` appended to **`arm-tegra-seam`** for the `pidesk` cross | board legs 12 → 13, gate 20 → 21; mix legs unchanged at 8. `arm-tegra-desk` is left exactly as rung 2 set it |
| The rung-2 handshake | `main.rs:6355` | `TEGRADESK_CLICK_ROUTED`: `false` → `cfg!(feature = "orinclick")` — **not** a literal `true`; see below |

**No `video/` edit was required, and none was made.** `wm::hit_test`, `wm::focus_changed`,
`wm::count`, `wm::compat_live` and `video::panel_info_nonblocking` are all already `pub` and
already ungated on this arch — the rung is genuinely one call plus its instrument, as §3.4
predicted, and the shared lane was not touched.

**`orinclick` implies `tegra_el0`, and that is the shape of the configuration.**
`wc_click_route` lives in `arch/aarch64/syscall.rs`, which `arch/aarch64/mod.rs:46` gates on
`any(baremetal, tegra_el0)`; `baremetal` implies `pi` and `pi` + `tegra` is a hard
`compile_error!`. A standalone `orinclick = []` in the `orindesk`/`jd1dc` mould would have
been a knob that compiles *nothing* unless the operator separately guessed
`UNAOS_TEGRA_EL0=1` — a vacuous gate wearing a green verdict.

#### The stop-line (§5.2) is NOT crossed, and the reason is a `#[cfg]`, not a promise

`orinclick` implies `tegra_el0` and nothing else. Every furniture arm inside
`wc_click_route` — `strip::press_route`, `quarry::service`, `pulsewin::press_route`,
`quarry::press_route`, the DRAG-PI chrome arm and the SHELLWIN-PI arm — is
`#[cfg(feature = "pidesk")]` and is **compiled out** of the armed image. That includes
`quarry::open()`, which is what boot 11 overflowed the 16 KiB stack on, *at click-router
depth on the input-drain task* — i.e. exactly this call stack. What remains is the window
half: `hit_test`, the three control-disc arms, `focus_changed`, `user_input_set_active`. No
dock, no strip, no menubar, no crystal, no `render_service`, no `pidesk::activate`.

Checked in the artifact rather than argued. `orin_click` (`0x100da0`–`0x1013e0` in the armed
jetson kernel) contains **exactly five `bl` targets**, and this is the whole list:
`video::panel_info_nonblocking`, `video::wm::hit_test`, `video::wm::row` (the inlined
`compat_live`/`count` row walk), `arch::aarch64::syscall::wc_click_route` (two `bl` sites —
LLVM cloned the call across the two arms of the `pressed != 0` split; one call per
invocation) and `serial::_print`. No `quarry`, no `strip`, no `dock`, no `crystal`, no heap
allocation, no file I/O.

**Stack cost is UNMEASURED on this board.** `[u7stk]` is present and `witness`-gated here and
has never been pointed at this path. §5's numbers are Pi numbers.

#### The instrument, and what makes each line print a non-pass verdict

The recurring defect this project pays for is a witness that cannot fail. Three lines, and
the census is the load-bearing one.

| Line | Verdicts | What reachable state prints a non-pass |
| --- | --- | --- |
| `[orinclick] arm panel=… -> V` | `ARMED`, `DECLINE reason=no-target`, `…=no-panel`, `…=panel-locked` | **`no-target` is what the DEFAULT armed boot prints**: `orinclick` alone mints no window, so `wm::count()` is 0 and every press will take the miss arm. The instrument's non-pass path is the one a real boot takes |
| `[orinclick] edge=… -> V` | `RAISED`, `HIT-SAME`, `CONSUMED`, `MISS-SHELL`, `MISS-IDLE`, `MISS-FULLSCREEN`, `RELEASE-DROPPED`, `RELEASE-DELIVERED`, `NO-EDGE`, `DECLINE reason=no-geometry`, **`FAIL reason=no-raise`**, **`FAIL reason=miss-unhandled`** | `no-raise` = a press hit an unfocused window, was not consumed, and the focus did not move — a broken `user_input_set_active`, a `focus_changed` that declined the owner, or a `#[cfg]` widen that compiled out the `owner != cur` arm. `no-geometry` = `panel_info_nonblocking` refused, so the cursor read is the (0,0) clamp and the hit-test is not a statement about where the operator pointed |
| `[orinclick] census … -> V` | **`FAIL reason=stuck-focus`**, `IDLE-NO-CLICKS`, `DECLINE reason=no-target` / `…=release-only` / `…=all-miss` / `…=geometry-refused`, `ROUTING` | `stuck-focus` is sticky propagation of `no-raise`. `ROUTING` is earned: it requires at least one press that actually reached a window |

Every verdict is derived from the hit-test, the focus either side of the call and the
router's own return value. None is asserted, and none is a constant.

#### The census is the answer to "an absence is not evidence"

`[clickroute]` missing from a capture where nobody touched the mouse is an **unrun** test, not
a failing one — and the router's own lines cannot tell those apart: `wc_click_route` prints
on exactly two arms (a press that moved focus to a window; a miss while an app holds focus)
and is silent on a re-click of the focused window, on every release, and on every press while
focus is already the shell — which, on a fresh Orin boot, is all of them.

So the census prints **every ~10 s from inside the pump's own drain loop, whether or not
anything was clicked**:

* `census … btn=0 -> IDLE-NO-CLICKS` — alive, nobody clicked. **UNRUN.**
* `census … btn=N … -> FAIL/DECLINE` — clicks arrived and did not route. **FAILED.**
* `arm` printed, census then **STOPS** — the drain task is dead or wedged.
* `arm` absent while `:: tegra: JD2 — console OWNS the panel` is present — the loop was
  entered and did not survive its first quarter-second.
* neither present — the knob is off, or the boot was headless and the pump delegated to
  `kbd_pump_body`.

The dead-task case is why the census lives in that loop and not beside it. Nothing in this
tree inherits a dead task's singleton roles — `steal_ok` is false for every explicitly-pinned
task and there is **no re-home path in the source at all** — so a dead `jd2_console_pump`
means clicks stop routing for the rest of the boot and no other subsystem notices. Pi's boot
11 is the precedent for not trusting an observer here: `[el0live] verdict=LIVE` printed one
line **after** the synchronous exception that killed cpu 3, and `:: SCHED: load ::` read
`c3=100%` for the corpse. This census is not an observer of the routing task; it **is** the
routing task, printing on its own core off its own `CNTPCT_EL0`. It cannot report liveness it
does not have. `seq=` increments by exactly one per line so a gap names a lost serial line
rather than letting one evaporate, and `up=` is read at print time so two consecutive census
lines cannot carry the same clock.

The JD20 line (`:: tegra: JD20 — pointer BUTTON …`) is **kept**, not replaced. "An event
reached the pump" and "the router made a decision" are different claims, and keeping them on
separate lines is what lets a capture separate a decoder fault from a routing fault. (It is
also what makes the knob-off image byte-identical: removing the line would move every line
below it in `main.rs`, and panic `Location` records embed line numbers.)

#### Where the keyboard goes on a `pidesk`-off build — a real consequence, stated

With `pidesk` off, the SHELLWIN-PI arm (`is_kernel_owner` → hand the keyboard to asid 0) is
compiled out, so a press on `orin_wm1`'s row — owner `wm::KERNEL_OWNER_DESKTOP` — takes the
ordinary `owner != cur` arm and leaves `USER_INPUT_ACTIVE` holding that kernel pseudo-ASID.
On the Orin that is **inert for the keyboard, verified not assumed**: the only consumer of
`USER_INPUT_ACTIVE` for keystrokes is `pump_usb_into_gui`'s `user_input_active() != 0` branch
(`main.rs:3887`), which is `#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]` and
does not exist on tegra; `jd2_console_pump` feeds every `Event::Key` through `handle_key`
regardless of focus. The focus either side of the call is printed on every `[orinclick]` line
so an operator can see the pseudo-ASID land rather than take this paragraph on trust. When
rung 5 arms `pidesk`, the SHELLWIN-PI arm compiles in and takes over; no change is owed here.

#### The rung-2 handshake, and why it is not a literal `true`

Rung 2 (`088d17c1`) landed the desktop seam while this arc was in flight and left an explicit
obligation: `main.rs`'s `TEGRADESK_CLICK_ROUTED` — its §6.1 stop-line — was held `false` "because
this rung has not landed", to be flipped **in the same commit** that makes the Button arm call
`wc_click_route`. Left `false` afterwards, the seam refuses for a reason that is no longer true;
flipped early, it asserts a route back that does not exist, which is the CONSWIN one-way trip
itself.

It is flipped here, and it is flipped **derived, not asserted**:

```rust
const TEGRADESK_CLICK_ROUTED: bool = cfg!(feature = "orinclick");
```

A literal `true` would have been wrong, and the reason is a configuration that is buildable
today: **`tegradesk` does NOT imply `orinclick`.** `tegradesk = ["pidesk", "tegra_el0"]`, so
`UNAOS_TEGRADESK=1` alone produces an image whose seam claims clicks route and whose Button arm
does nothing but log. That is the exact defect the constant exists to prevent, re-entered through
the constant. `cfg!` makes the claim a property of the build.

Proven in the artifact, not in the diff — rung 2's own discipline, since its second stop-line
string was dead code and only an artifact grep found it:

| Build | `rung3-unlanded…` in `unaos-kernel`? | `stop-line-5.2`? | `[orinclick]` instrument? |
| --- | :-: | :-: | :-: |
| `tegra,tegrasmp,tegradesk` | **yes** — the one string is `rung3-unlanded+stop-line-5.2`, and it is TRUE of that image | (as part of it) | no |
| `tegra,tegrasmp,tegradesk,orinclick` | **no** — const-folded away | **yes**, standalone | yes |

So the seam still refuses on every buildable image (`TEGRADESK_CASCADE_OK` is untouched and still
`false` — §5.2 is rung 5's, not this rung's), and the refusal now names the *right* reason for the
image it is running on.

⚠ **Rung 2's cross-knob hazard applies unchanged and is not fixed here.**
`UNAOS_ORINDESK=1 UNAOS_TEGRADESK=1` is unsound — `pidesk`'s DESKTOP-CLEAR writes the whole panel
through the FRONT buffer on the premise of an empty window table, which rung 0's row falsifies —
and the seam refuses it with `reason=table-not-empty`. That is orthogonal to this rung:
**`orinclick` + `orindesk` is the rung-3 demonstration boot** (a row to click, a router to click
it with, no seam and no cascade), and adding `tegradesk` to it is what the seam declines.

#### Gate, and the go-red proof

All against `088d17c1`, in a dedicated worktree at `~/unaos-bench/scratch/orin4-rung3/tree`:

| Command | Result |
| --- | --- |
| `UNAOS_TEGRA=1 ./arroyo check` | green, **21 legs (13 board + 8 x86 pairwise-mix)** |
| `UNAOS_ORINCLICK=1 ./arroyo check` | green, 21 legs — the knob is self-sufficient (cargo resolves `orinclick` → `tegra_el0` → `tegra`) |
| `UNAOS_TEGRADESK=1 UNAOS_ORINCLICK=1 ./arroyo check` | green, 21 legs — the seam and the routing armed together |
| `./arroyo test-arm` | exit 0, complete; 2 PASS / 0 FAIL, no panic, no `[serial] dropped` |

A leg that has never gone red is not evidence. Injecting one type error into `orin_click` reds
**exactly the two legs that carry the knob** and nothing else — note `arm-tegra-desk` staying
green, which is the proof that rung 2's disarmed twin was not disturbed:

```
✅ x86-all  ✅ arm-pi  ✅ arm-tegra  ✅ x86-vsyncpace  ✅ arm-tegra-el0  ✅ arm-tegra-simmer
✅ arm-tegra-xusbfw  ✅ arm-tegra-smpmark  ✅ arm-tegra-orindesk  ✅ arm-tegra-jd1dc
✅ arm-tegra-desk
❌ arm-tegra-seam — unaos-kernel FAILED to compile
❌ arm-tegra-orinclick — unaos-kernel FAILED to compile
✅ x86-mix-0 … ✅ x86-mix-7
❌ kernel cfg coverage FAILED (crate unaos-kernel), legs: arm-tegra-seam arm-tegra-orinclick
```

#### Byte identity, measured at the KERNEL level

Same worktree, same absolute path both times (panic `Location` records embed the path as well as
the line number), two independent `CARGO_TARGET_DIR`s, only the patch varying,
`llvm-objcopy -O binary` then `cmp`. **The ESP/flashable image is deliberately NOT the subject: it
embeds `SRC.TGZ`, a tarball of the working tree, so an ESP compare reports the arc's own diff back
as if it were codegen.**

| Build | features | loadable image (`-O binary`) | sha256 (both sides) | `.elf` |
| --- | --- | --- | --- | --- |
| jetson default | `tegra,tegrasmp` | **IDENTICAL**, 1 526 560 B | `2703a4ed…` | 40 B shorter, `.strtab` only |
| jetson armed-EL0 | `tegra,tegrasmp,tegra_el0` | **IDENTICAL**, 1 729 372 B | `3fce4318…` | 112 B longer, `.strtab` only |
| virt (the `test-arm` build) | `witness,ehcihid` | **IDENTICAL**, 1 440 704 B | `4d30210c…` | — |
| Pi `kernel8.img` | `baremetal,skip_xhci,witness,pidesk,quarry,livecon` | **IDENTICAL**, 2 148 112 B | `5e4f0156…` | same size; 210 bytes differ, every one inside `.strtab`'s file range |
| jetson, **knob ON** | `tegra,tegrasmp,orinclick` | **DIFFERS** — the knob is not vacuous | `1af1d90a…` | — |

In every `.elf` row `readelf -S`/`-l` show `.strtab` (non-alloc, symbol names) as the only section
whose size or content moves, and every program header and every allocatable section identical — the
documented benign `.llvm.<hash>` internal-symbol-suffix class. The Pi row matters because `main.rs`
**is** compiled into `kernel8.img`: all three `main.rs` edits (the Button arm, the census hook, the
stop-line constant and its doc comment) are line-neutral — **6511 lines either side** — so no panic
`Location` moves.

#### Proofs of the instrument, in the artifact

* **Presence** — all **24** `[orinclick]` marks and verdict strings appear in `img-on-jetson.bin`
  and **zero** of them in `img-off-jetson.bin`. `LC_ALL=C grep -a -c -F`, never `strings`:
  `strings` defaults to `-n 4`, drops short marks and breaks at em dashes.
* **Freshness** — `git log -S` over all refs returns **0** prior commits for `orinclick`,
  `[orinclick]`, `IDLE-NO-CLICKS`, `stuck-focus`, `orin_click_census` and `arm-tegra-orinclick`.
  Nothing here is inherited or re-used.
* **Reachability** — `llvm-objdump -d` on the armed jetson kernel:
  `62d18: bl 0x1100b0 <unaos_kernel::arch::aarch64::display_tegra::orin_click>` and
  `62e74: bl 0x110914 <…::orin_click_census>`, both inside `unaos_kernel::jd2_console_pump`
  (`0x629f4`); and inside `orin_click`, `bl 0x8d798
  <unaos_kernel::arch::aarch64::syscall::wc_click_route>`. Compiled-in is not the claim; the `bl`
  is. `jd2_console_pump` itself is the JD2/JD20 pump already metal-PROVEN on this board for
  keyboard and pointer (§1), so the chain's one unproven link is the knob nobody has set.

#### What this rung deliberately did NOT do

* **No rung 4.** The console is not routed into a `wm` row, `fbcon::console_is_routed` is only
  *read* (by rung 2's seam, unchanged), and the handoff detach is untouched. Rung 3 is the
  *precondition* for rung 4 (§6.1); landing them together would defeat the ordering it enforces.
* **No rung 5.** Nothing arms the dock, the strip, the menubar, the crystal or a tegra
  `render_service`, and `TEGRADESK_CASCADE_OK` was not touched.
* **No `video/` edit.** None was needed; if one had been, it would have been written up here
  rather than made, exactly as §3.5.2 was.
* **`arm-tegra-desk` not modified.** An earlier cut of this arc appended `,orinclick` there; rung 2
  then claimed that leg as its disarmed twin, so the cross moved to `arm-tegra-seam` instead.
* **No extra composite.** `focus_changed` already ends in `composite()`. Whether that composite
  survives the console pump's own `Screen`/`pal.render()` blit is an on-glass question this rung
  has not answered and does not claim — see §7.

### §3.8 FLOWN 2026-08-25 (boot7f) — rungs 0 and 3 reach metal: composited, armed, unclicked

The first Orin flight to carry both `orindesk` and `orinclick`. Everything in this section is
quoted from the bench serial capture `~/unaos-bench/capture/line-acm0/orin.log`; **capture line
numbers are the primary anchor**, with the flight's boot id beside them, because the serial line is
lossy and some boots in that file lost their kernel banner to it. boot7f's kernel banner is at
capture line 11091 and the run's tail is line 11540.

Media: `~/unaos-bench/flash/orin/boot7f-nowinsweep-20260825T2034Z-04d46aa/`. ⚠ That directory's
`SRC.SHA` records `commit 29a55b9c` / `describe 29a55b9c-dirty` — the commit *before* the one the
directory name claims. The image bytes carry the JX1 no-winsweep change regardless (proven by its
own flight and by an artifact grep); it is the label that is wrong, not the kernel.

#### Rung 0 — the first composited window on the Orin

Capture line 11110:

```
[orinwm1] win=1 panel=1920x1200 surf=640x400 box=650x444 at (635,378) scale=1 stage=4194304 present=Composited -> COMPOSITED
```

`stage=4194304` closes §3.3 on metal: the compositor's staging buffer **is** allocated on the tegra
path, at `MAX_STAGE_BYTES`, so no composite falls back to lazy growth. `present=Composited` is
`present_outcome`'s own return and the trailing verdict is derived from it, so `-> COMPOSITED`
cannot stand over a `Suppressed` or a `NoRow`.

**The chrome was painted — the "no frame, painter dead-stripped" reading is REFUTED.** The theme
latched on the same boot four lines earlier, capture line 11106:

```
[crispy] theme=us-crispy-modern@0787ba9f frame=5 bevel=1 title_h=34 radius=12 ctrl=24 gap=12 …
```

and the geometry on the `[orinwm1]` line matches those constants exactly: 640x400 of surface becomes
a 650x444 box — `+2 x BORDER` (5+5) across and `+ TITLE_H + 2 x BORDER` (34+10) down, against
`video/theme.rs`'s `FRAME = 5` and `TITLE_HEIGHT = 34`. The flown artifact confirms it in the
codegen: `llvm-objdump -d` on that card's `kernel.elf` shows `video::wm::paint_window` and
`video::wm::draw_title` each reached by two `bl` sites, so the painter is not merely linked, it is
called. The open question was never whether the frame was drawn; it is **whether those pixels
reached the glass**, which no `[orinwm1]` field answers.

#### Rung 3 — the router armed, and then nothing was clicked

Capture line 11424, printed from inside `jd2_console_pump`'s own drain loop:

```
[orinclick] arm panel=1920x1200 rows=1 compat=0 focus=0x0 pidesk=0 t=31 -> ARMED
```

`rows=1` is rung 0's window — the `DECLINE reason=no-target` arm §3.7 predicts for an
`orinclick`-only image did not fire, because `orindesk` put a row on the panel. `pidesk=0` confirms
the furniture is compiled out, as §3.7's stop-line argument requires.

Then **48 consecutive `IDLE-NO-CLICKS` censuses**, `seq=1` at capture line 11425 through `seq=48` at
line 11540, every one of the form:

```
[orinclick] census seq=1 t=71 up=10s btn=0 press=0 rel=0 noedge=0 raised=0 same=0 miss=0 consumed=0 stuck=0 nogeom=0 dropped=0 rows=1 compat=0 focus=0x0 -> IDLE-NO-CLICKS
```

`seq` increments by exactly one across all 48, so no census line was lost to the serial; `up=`
advances 10 s per line, so the drain task stayed alive for the whole 480 s. **This is the census
doing precisely the job §3.7 built it for**: it proves the routing task is alive and reports
`btn=0`, i.e. UNRUN — not passing, not failing.

**The click itself is rung 3's open question and it is still open.** Nobody pressed the button on
this flight, so `wc_click_route` has never been entered on this board and no `[orinclick] edge=…`
line exists in any capture. Note that the pointer decoder *is* live on the same boot —
`:: MOUSE-1: 192 reports, last dx=0 dy=1 buttons=0x00 == witness ::` at capture line 11435 — so the
missing half is a button press, not a working pointer.

#### The display engine, same flight

boot7f also answered the register-model question and confirmed the window sweep must stay gated.
Those results belong with the rest of the nvdisplay work and are recorded in
[`../01_BOOT_HAL/arch_arm64.md`](../01_BOOT_HAL/arch_arm64.md), **FLOWN 2026-08-25** at the end of
the JD1-DC-MODEL section. The two headlines that bear on this ladder:

* `MODEL-VERDICT=NVDISPLAY-CLASS-C670` (capture line 11088) — the aperture is `NV_PDISP` rebased to
  offset 0, class `NVC67D`, Ampere ga10x: 2 heads, 2 SORs, 4 windows, and
  `FE_CHNCTL_CORE=0x00000021` says UEFI still owns the core channel. **The window map in this tree
  was Tegra186/194's and is wrong for this chip.**
* boot7e, twice, took an EL3 abort (`ESR 0xbe000011`) on the first window-register read at
  `0x13802e00` — an offset *inside* the DTB-declared aperture. The sweep is gated off at
  `04d46aae`. No rung on this ladder may reintroduce it against the T194 offsets.

#### What this flight did NOT establish

* **Nothing about the glass.** Every line above is a wire witness. §7's on-glass caveat stands
  unchanged. The instrument that will answer it (`[orinchrome]`, verdicts `CHROME-ON-GLASS` /
  `CHROME-PARTIAL` / `CHROME-MISSING` / `COMPOSITE-NOT-ON-GLASS`) landed at `e98d798b`, which is
  **not an ancestor of `04d46aae`**; the boot7f media predates it and cannot emit those lines.
* **Nothing about routing.** `-> ARMED` plus 48 `IDLE-NO-CLICKS` is a liveness claim about the
  instrument, not a claim about `wc_click_route`.
* **Nothing about stack cost.** `[u7stk]` was not pointed at the click-router depth on this boot;
  §5's numbers remain Pi numbers.

> ⚠ **The first two bullets above are DISCHARGED by boot7g — see §3.8.1 immediately below.** They
> are left standing as written because they are the correct reading of *boot7f*; what changed is
> the flight, not the reading.

#### §3.8.1 FLOWN 2026-08-25 (boot7g) — the click ROUTES, and the chrome is ON THE GLASS

The successor flight to boot7f, and the one that closes both halves boot7f left owed. Same capture
file, same anchoring law: `~/unaos-bench/capture/line-acm0/orin.log`, **capture line numbers are
the primary anchor**, boot id `boot7g` beside each. The slice begins at the MB1 coldboot banner,
capture line 11542 (`[0000.068] I> MB1 (version: 1.0.1.17-t234-54845784-9b0d5809)`) — the sixth and
last coldboot in that file.

Media: `boot7g-clickchrome-20260825T2124Z-1f2545c`, image built at `1f2545cb`. **What that image
does NOT contain matters as much as what it does:** every fold landed after `1f2545cb` is absent —
`orinconwin` (rung 4, §3.9), the SMPINSTR follow-ups and NET-4F. Nothing in this subsection may be
read as evidence about rung 4.

⚠ **The sitting was still live when this was folded.** The slice scored and quoted here ends at
capture line 13151 (byte 1020297); the board was at `up=230s` and still appending. Anything the
capture gains past line 13151 is **unscored** by this subsection.

##### Rung 3, the wire half — CLOSED

Somebody finally pressed the button. Capture lines 13084-13085, boot7g:

```
[clickroute] press hit asid=4294967042 win=1 (was 0) delivered
[orinclick] edge=press btn=0x01 at (1009,546) geom=yes hit=yes win=1 owner=0xffffff02 focus 0x0->0xffffff02 consumed=0 -> RAISED
```

`focus 0x0->0xffffff02` is the whole claim: the router entered `wc_click_route`, hit-tested the
row rung 0 put on the panel, and **raised it**. `asid=4294967042` is `0xffffff02` in decimal, so
the `[clickroute]` and `[orinclick]` lines name the same window from two different call sites. The
release completed the pair, capture line 13087:

```
[orinclick] edge=release btn=0x00 at (1009,546) geom=yes hit=no win=0 owner=0x0 focus 0xffffff02->0xffffff02 consumed=0 -> RELEASE-DELIVERED
```

and the census flipped off its UNRUN verdict on the next tick, capture line 13089:

```
[orinclick] census seq=6 t=271 up=60s btn=2 press=1 rel=1 noedge=0 raised=1 same=0 miss=0 consumed=0 stuck=0 nogeom=0 dropped=0 rows=1 compat=0 focus=0xffffff02 -> ROUTING
```

`IDLE-NO-CLICKS -> ROUTING` is the census transition §3.7 designed it to make, and it happened
because `btn` moved, not because a timer fired.

**The second press discriminates a raise from a no-op.** Capture line 13092:

```
[orinclick] edge=press btn=0x01 at (1066,408) geom=yes hit=yes win=1 owner=0xffffff02 focus 0xffffff02->0xffffff02 consumed=0 -> HIT-SAME
```

A click on the already-focused window returns `HIT-SAME`, not `RAISED` — the router distinguishes
"raise this window" from "this window is already on top", which a stub that unconditionally printed
`RAISED` could not. Three further verdicts printed on the same flight and each is a different
branch of the router, so the coverage is not one path taken six times:

| capture line | verdict | what it exercises |
| --- | --- | --- |
| 13125 | `-> CONSUMED` (with `[clickroute] close=win1 … settle=furniture-refused`, line 13124) | a hit on the close control — consumed by the furniture layer, which then *declined* to settle because `pidesk` is compiled out |
| 13133 | `-> MISS-SHELL` (with `[clickroute] press miss at (384,209) -> shell focus (was 4294967042)`, line 13132) | a click outside every row drops focus back to the shell — `focus 0xffffff02->0x0` |
| 13135 | `-> RELEASE-DROPPED` | the release half of a consumed press, correctly not delivered to a window |

Final census in the scored slice, capture line 13151: `[orinclick] census seq=23 t=951 up=230s
btn=12 press=6 rel=6 noedge=0 raised=1 same=3 miss=1 consumed=1 stuck=0 nogeom=0 dropped=0 rows=1
compat=0 focus=0x0 -> ROUTING`. **`stuck=0`, `nogeom=0`, `dropped=0`
across six press/release pairs** — the three failure counters the instrument carries specifically
so that a routing claim cannot be made over a silently degraded path all read zero.

##### Chrome on the glass — the on-glass question is CLOSED for rungs 0 and 3

`[orinchrome]` — the instrument §3.8 recorded as landed-but-unflyable, because `e98d798b` is not an
ancestor of the boot7f media — flew on this image and read the panel back. All six frame probes and
the content probe MATCHed, capture lines 12680-12686, boot7g:

```
[orinchrome] probe=kl_top   at (960,378) got=0xb4b4b9 want=0xb4b4b9 -> MATCH
[orinchrome] probe=kl_bot   at (960,821) got=0xb4b4b9 want=0xb4b4b9 -> MATCH
[orinchrome] probe=kl_left  at (635,600) got=0xb4b4b9 want=0xb4b4b9 -> MATCH
[orinchrome] probe=kl_right at (1284,600) got=0xb4b4b9 want=0xb4b4b9 -> MATCH
[orinchrome] probe=bev_lt   at (960,379) got=0xffffff want=0xffffff -> MATCH
[orinchrome] probe=bev_sh   at (960,820) got=0xaaaaaf want=0xaaaaaf -> MATCH
[orinchrome] win=1 box=650x444 at (635,378) frame=6/6 content=0xff00ff@(960,617) MATCH strip=0xf1f1f3 face=0xe9e9eb ctrl=0xf4f4f5 (ceramic — raw, compare with [crispy]) -> CHROME-ON-GLASS
```

(The four `kl_*` probes are reproduced with their columns aligned for reading; the wire text is one
probe per line, unpadded.)

**Why this is a glass claim and not another wire claim.** The `want=` values are the theme's own
constants — `keyline=0xb4b4b9`, `bevels=0xffffff/0xaaaaaf` on the `[crispy]` line four rows above at
capture line 12675 — and the `got=` values are **read back out of the scanout the panel is being
fed from**, at absolute panel coordinates derived from the box `[orinwm1]` reported. A composite
that ran and produced nothing visible cannot make `got` equal `want` at six independent
coordinates. The bevel probes are the sharp ones: `bev_lt` at `(960,379)` and `bev_sh` at
`(960,820)` are each **one pixel inside** their keyline neighbours at `(960,378)` and `(960,821)`,
and they return *different* colours. That is a one-pixel-accurate frame, not a fill.

The content probe closes the other half: `content=0xff00ff@(960,617) MATCH` — the window's magenta
body is present at the box centre, so the frame is not sitting over an empty or stale interior.

Rung 0's own line printed on this flight too, capture line 12679, byte-identical in shape to
boot7f's:

```
[orinwm1] win=1 panel=1920x1200 surf=640x400 box=650x444 at (635,378) scale=1 stage=4194304 present=Composited -> COMPOSITED
```

so `[orinchrome]`'s box and `[orinwm1]`'s box are the same box, and the on-glass verdict attaches to
the composite that the wire verdict describes.

##### OPERATOR OBSERVATION (Peter, boot7g) — the composite does NOT survive the console blit

Recorded because it is a measurement the serial cannot make, and because it answers a question §7
had left open. The operator, watching the panel through the sitting:

> "once I clicked the ghost of the window filled in, otherwise there is basic function."

Trigger event, capture line 13085, boot7g — the same `RAISED` quoted above:

```
[orinclick] edge=press btn=0x01 at (1009,546) geom=yes hit=yes win=1 owner=0xffffff02 focus 0x0->0xffffff02 consumed=0 -> RAISED
```

**Mechanism.** Between composites the JD2 console's back-buffer blit overdraws the composited
window's body: `jd2_console_pump` owns the panel through a double-buffered `Screen` whose
`pal.render()` writes the console buffer over the front scanout, and on this image nothing subtracts
the window from it. The window therefore decays to a "ghost" — the frame outlives the body, because
only the body is where the console writes. `focus_changed` ends in `composite()`, so **the click
repainted it**, which is exactly what the operator saw.

**This does not weaken the `CHROME-ON-GLASS` verdict — it explains its timing.** The `[orinchrome]`
probes ran at capture lines 12680-12686, immediately after the composite at line 12679, i.e. inside
the window between a composite and the next console blit. The verdict is true as measured: those
pixels *did* reach the glass. What the operator adds is that they do not *stay* there without a
recomposite.

**Consequence, and it is already built.** Rung 4 (`orinconwin`, §3.9, landed at `68c47585`) is the
designed fix: a routed console stops writing the panel behind the compositor's back, because
`Screen::present_background` subtracts `wm::occluders`. **It is NOT in the flown image** — `68c47585`
post-dates `1f2545cb` — so nothing here measures it. What boot7g *does* do for rung 4 is discharge
its §6.1 precondition: the metal `RAISED` capture that section demanded before rung 4 may lean on
the dock as a way back now exists, at capture line 13085.

##### The display engine and the rest of the board, same flight

* **JX2-NVC67D answered, all reads survived.** The channel-state probe and its
  `JX2-VERDICT=EFI-OWNED-LIVE` are recorded with the rest of the nvdisplay work in
  [`../01_BOOT_HAL/arch_arm64.md`](../01_BOOT_HAL/arch_arm64.md), **FLOWN 2026-08-25 (boot7g)**.
  The consequence for this ladder: the scanout rung 0 composites into is presented by a channel
  **we do not own**, so any future rung that wants the display engine takes it by a deliberate
  channel handoff, never by an MMIO poke.
* **The JX1 false cause is retracted on the wire.** `JX2-SWEEPDISABLED` (capture line 12657) and
  the retraction inside `JD1-DC VERDICT=DECODES-NOMATCH` (capture line 12659) both printed, so the
  gated-to-empty sweep now says so in its own output instead of being read as a measurement.
* **EL0 and SMP clean.** `[el0core] rollup: el0refuse=0 el1cores=0x1` at capture line 12983; all
  five APs online with the SMPMARK tag sequence `:P: :R1: :A: :R2: :A: :R3: :A: :R4: :A: :A: :R5:`
  (capture lines 12930-12949) — every AP reached the far side of `enable_mmu_virt`, **no park**.
  The shell came up: `JD2 — OUT | JD2: interactive shell on the inherited scanout. Type 'help'.`,
  capture line 12998.
* **Spec replay: PASS.** `unaos/scripts/mbench.py --replay` of the slice against
  `unaos/scripts/specs/jetson-sync1.spec` reports
  `✅ MBENCH PASS — 15/15 required witnesses, 0 forbidden hit(s), 1612 lines scanned, pending 5/6 matched`.
  The one unmatched PENDING is `TEGRA-SD.*block backend published`, which this flight does not
  exercise.

##### What this flight still did NOT establish

* **Nothing about rung 4.** `orinconwin` is not in the image. Every `[orinconwin]` verdict in §3.9
  remains unprinted.
* **Nothing about persistence.** The operator observation above is precisely the boundary: the
  chrome is on the glass *at composite time*, and a mechanism is named for why it does not persist.
  "The desktop stays on the glass" is a rung-4 claim and is not made here.
* **Nothing about stack cost.** `[u7stk]` was still not pointed at the click-router depth; §5's
  numbers remain Pi numbers.

> ⚠ **The first bullet is DISCHARGED by boot7h — see §3.9.1.** `orinconwin` flew the very next
> flight and ROUTED. The persistence bullet is *partially* moved: rung 4's mechanism flew, but the
> capture carries no post-routing read-back of win=1, so §3.9.1 states exactly what the wire can
> and cannot say about it. The stack bullet stands.

### §3.9 LANDED 2026-08-25 (rung 4) — the console as a window, DEFAULT OFF and UNFLOWN

**Measured against `e98d798b`.** Rung 4's row asked for three things — *"route the JD2 console into
a `wm` row; `fbcon::console_is_routed`; skip the handoff detach when routing succeeded"* — and all
three landed, behind `orinconwin`, **plus the ordering rule of §6.1 turned from an obligation on the
arc into a branch the build can take.** As with rungs 2 and 3: this COMPILES and is REACHED in
codegen; **no Orin has booted it.** QEMU models no Tegra234, so the metal witness is owed.
*(⚠ Superseded 2026-08-25, same day: boot7h booted it and it ROUTED — §3.9.1. The section below is
left as written because it is the correct record of what LANDED; §3.9.1 is the record of what
FLEW.)*

#### What landed

| where | what |
| --- | --- |
| `crates/kernel/Cargo.toml` | `orinconwin = ["pidesk", "tegra_el0"]` — self-sufficient, and deliberately does NOT imply `orindesk`/`orinclick` |
| `arch/aarch64/display_tegra.rs` (file TAIL) | `orin_conwin()`, its one-shot latch, and the two ordering-term consts `ORINCONWIN_DESK_ROW`/`ORINCONWIN_CLICK_ROUTED` (`cfg!()`, never literals) |
| `main.rs` terminus line | `#[cfg(feature = "orinconwin")] …display_tegra::orin_conwin();` appended beside DESKSEAM's call — zero source lines added |
| `main.rs` `jd2_console_pump` phase 2 | `fbcon::detach()` folded IN PLACE to `if !tegra_conwin_live() { … }` |
| `main.rs` file tail | `tegra_conwin_live()`, both cfg polarities; the off arm is `#[inline(always)] false` |
| `arroyo` | `UNAOS_ORINCONWIN` env map + the `arm-tegra-conwin` cfg-matrix leg (board legs 13 → 14, gate 22 → 23) |

**No `video/` edit, and none was needed.** Every verb is the shared implementation the Pi and x86
already reach: `fbcon::panel_console_face_arm`, `fbcon::panel_console_window_open`,
`fbcon::console_is_routed`, `dock::Layout::for_panel`, `wm::reserve_stage`, `wm::present_outcome`,
`wm::composite`. There is one `panel_console_window_open`, one `route_present_banded`, one
`Pending`, and this board now runs those same bytes — proven by disassembly, not by linkage:
`orin_conwin`'s only `bl` targets in the armed `kernel.elf` are `wm::reserve_stage`,
`fbcon::panel_console_window_open`, `wm::present_banded`, `wm::composite` and `serial::__print`.

#### The ordering rule is now a BRANCH, not a promise

§6.1's binding sentence — *"Rung 4 may not ship a console window on an image where `orinclick` is
off"* — is enforced by `orin_conwin` reading BOTH knobs through `cfg!()` (the
`TEGRADESK_CLICK_ROUTED` idiom, never a literal `true`) and declining, named, on an image missing
either. `orinconwin` therefore implies NEITHER, which is what keeps the decline reachable. The
conjunction adds `orindesk` to §6.1's letter, deliberately and one way only — stricter: §6.1's own
caveat records that on an `orinclick` image with no row on the panel every press takes the router's
`no-target` arm, so the "route back" would be unexercisable and its verdict unreadable.

**Both polarities were measured on real artifacts, because a decline that cannot print is an absent
instrument.** `LC_ALL=C grep -a -o` on `kernel.elf`:

| image | `[orinconwin] gate` | `DECLINE reason=ordering-rule` | `win=` / `dock-cannot-host-full-strip` |
| --- | :-: | :-: | :-: |
| knob-off jetson | 0 | 0 | 0 |
| `UNAOS_ORINCONWIN=1` alone | 1 | **1**, `held=no-desk-row+clicks-unrouted` | 0 (const-folded away) |
| `+UNAOS_ORINDESK=1 +UNAOS_ORINCLICK=1` | 1 | 0 (const-folded away) | 1 each |

That the ordering DECLINE is *absent* from the fully-armed image is the correct answer, not a hole:
on that image the rule cannot hold anything off. The refusal is ONE `serial_println!` with a `held`
string chosen by a `match` over both terms, for the reason DESKSEAM measured on its own artifact —
written as two sequential `if !CONST` blocks the second string is dead code the moment the first
const is `false`.

#### The detach guard, and why it is the whole of the "LIVE" claim

`detach` sets `GUI_ACTIVE`, after which `fbcon::_print` returns at its first test. A console window
opened at the terminus and then detached at phase 2 would hold the boot log and never change again —
the frozen snapshot `wcx.rs` ships on x86. The Orin does not inherit it, for the Pi's reason: the
REASON for the detach is discharged by the route itself. A routed console does not write the panel
(`FbCon::draw_fb` hands back `win_fb`), so "exactly one writer on the panel" is already true.
Codegen, in the armed `kernel.elf` inside `jd2_console_pump`:

```
72388:  bl   0xe49c0 <…video::fbcon::console_is_routed>
7238c:  tbnz w0, #0x0, 0x72394          ; routed → skip
72390:  bl   0xe574c <…video::fbcon::detach>
```

Fail-closed by construction: `console_is_routed()` answers `false` for every decline arm the open
path has AND for every arm `orin_conwin` itself takes, so a rung that refused anywhere leaves the
detach taken, unchanged.

#### §7's open question, answered in SOURCE only

§7 left this as rung-4 territory: `jd2_console_pump` owns the panel through a double-buffered
`Screen` whose `pal.render()` blits the console back buffer, and whether a composited row survives
that blit was unmeasured. In source it does — `Screen::present_background` subtracts the window
layer (`wm::occluders`, the WC-I loop) on **both** of its cfg arms, the aarch64 one included, so the
desktop present never writes a pixel inside a live window's box. **That is a source reading, not a
metal measurement**, and this rung does not claim otherwise.

#### The §5.2 stop-line is NOT crossed

`pidesk::activate()` is not called. Rung 4 takes exactly the two steps of `activate`'s sequence the
console window needs — 2a FONT-PI, 2-3 CONSOLEWIN — and none of the rest: no PIDESK DESKTOP-CLEAR,
no `menubar::set_enabled`, no crystal, no `render_service`, no window population.
`TEGRADESK_CASCADE_OK` was not touched. `quarry` is not implied, so `quarry::open()` — boot 11's
actual 16 KiB overflow, at click-router depth — is the `#[cfg(not(feature = "quarry"))]` `false`
stub in this build.

**What `pidesk` DOES bring in, stated because §3.7 promised the opposite for `orinclick` alone and
the difference must not pass unnoticed.** On an `orinconwin` image `wc_click_route`'s furniture arms
(`strip::press_route` → `crystal::press_at` + `dock::press_at`, `pulsewin::press_route`, the DRAG-PI
chrome arm, the SHELLWIN-PI arm) are compiled IN. That is not a tolerated widening — it is the
rung's precondition: `dock::press_at` **is** §6.1's route back, and `video/mod.rs` gates the whole
furniture family on `pidesk`, so without it a minimise disc really would be one-way.
`pulsewin::press_route` returns on a `WIN_NONE` id and `quarry::press_route` is the stub, so the two
deep arms are unreachable on this build.

#### Gate

* `UNAOS_TEGRA=1 ./arroyo check` — green, **23 legs** (13 → 14 board legs, 9 mix legs unchanged);
  green again under `UNAOS_ORINCONWIN=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1`.
* `arm-tegra-conwin` proven to go RED on a re-introduced mismatch (`panel_console_face_arm` renamed
  inside `orin_conwin`): that leg alone failed E0425 and every other leg stayed green.
* `./arroyo test-arm` — green, `MISSION SUCCESS`.
* **Knob-off byte identity, MEASURED at the LOADABLE-IMAGE level.** `esp-jetson` built knob-off in a
  worktree at `e98d798b` and in this arc's tree: `llvm-objcopy -O binary kernel.elf` →
  `71f98f5360ee222a7b32d858cd9eb792ea1a0a660a45897ea3691e85b2fecf12` on both, and every allocated
  section (`.text`/`.rodata`/`.data`) hashes identically. The two `.elf` FILES differ only in
  `.strtab` size (non-loaded; the `.llvm.<hash>` internal-symbol suffixes JB11's arroyo note already
  records as a build-path artefact) — compare the binary image, not the `.elf` sha256.
* Armed artifact: every new witness one-hit-grepped (table above), and `orin_conwin` proven
  REACHABLE by disassembly — `72fec: bl 0xf8ecc <…display_tegra::orin_conwin>`, its single caller,
  inside `tegra_early_stop` (`0x7275c`).

#### What this rung deliberately did NOT do

* **No rung 5.** No dock/strip/menubar/crystal arming, no tegra `render_service`,
  `TEGRADESK_CASCADE_OK` untouched.
* **No `video/` edit.** None was needed; if one had been, it would have been written up here rather
  than made, exactly as §3.5.2 was.
* **`tegradesk` was not put on the new leg, and rung 2's seam was not modified.** DESKSEAM's
  `table-not-empty` floor refuses when `orindesk` has already minted a row, so the two seams do not
  ship together and the matrix does not pretend they do.
* **The stack question is asked but not answered.** `orin_conwin` runs on the BOOT stack (the
  terminus line) for ORIN-WM1's reason, but once the route is installed every subsequent
  `serial_println!` reaches `route_present_banded` from whatever stack is printing. That is exactly
  what the Pi ships (paced, damage-limited) and the Orin's own `[u7stk]` high-water for it has never
  been read. §7's standing note applies: every stack number quoted for this ladder is a Pi number.
* **UNFLOWN, and it stays behind its knobs until the rung-3 click flight returns its verdict.**
  §6.1's second obligation — *"Rung 4 wants a metal capture showing `[orinclick] edge=… -> RAISED`
  before it leans on the dock as a way back"* — is NOT discharged by this arc. Nothing here makes
  the console window reachable on a default image. *(Both halves of this bullet resolved within the
  day: boot7g delivered the `RAISED` capture, boot7h flew the knob — §3.9.1.)*

#### §3.9.1 FLOWN 2026-08-25 (boot7h) — the console IS a window: ROUTED, LIVE, and clicked

The flight rung 4 was built for, flown the same day it landed. Same capture file, same anchoring
law: `~/unaos-bench/capture/line-acm0/orin.log`, **capture line numbers are the primary anchor**,
boot id `boot7h` beside each. The slice begins at the MB1 coldboot banner, capture line 13159
(`[0000.068] I> MB1 (version: 1.0.1.17-t234-54845784-9b0d5809)`) — the seventh coldboot in that
file, immediately after boot7g's scored slice (which ended at line 13151).

Media: `boot7h-conwin-net4-20260825T2208Z-68c4758`, image built at `68c47585` — the ORIN-CONWIN
commit itself, which also carries SMPINSTR (`a50358f0`) and NET-4F (`ca80655c`), all three absent
from boot7g's image. Knobs: the §6.1 conjunction in full (`UNAOS_ORINCONWIN=1 UNAOS_ORINDESK=1
UNAOS_ORINCLICK=1`) plus `UNAOS_NET4=1`.

⚠ **Scored to capture line 16290.** The board was at `up=6410s` — a ~107-minute sitting — and the
capture ends mid-cadence (census `seq=641`, line 16290). Anything the file gains past line 16290
is **unscored** by this subsection.

##### The gate took the GRANTED branch — §6.1 as a branch, on the wire

Capture line 14828, boot7h:

```
[orinconwin] gate panel=1920x1200x4 stage=4194304 table=1 dock=GRANTED route=UNROUTED orindesk=1 orinclick=1 rows=12
```

`orindesk=1 orinclick=1` is the ordering conjunction §3.9 turned into a branch, read back from the
build and printed before anything irreversible; `dock=GRANTED` is the way-back check —
`dock::Layout::for_panel` at `MAX_WINDOWS` — passing on this panel. `table=1` says rung 0's window
already existed (it did: `[orinwm1] … -> COMPOSITED`, line 14297); `route=UNROUTED` is the honest
pre-state. No `DECLINE` line printed on this flight.

##### The console became a window, and the route went LIVE

Capture lines 14830–14833, boot7h — the shared `fbcon` machinery and rung 4's own terminus:

```
[wc-x] console-window win=2 panel=1920x1200 surf=1295x736 box=1305x780 at (307,158) cell=7x16 cols=185 rows=46
[wc-x] console-route first-paint win=2 (glyphs -> window surface, damage-limited)
[wc-x] console-window panic-fallback armed win=2 (panic paints the PANEL, not the window)
[orinconwin] win=2 panel=1920x1200 cell=7x16 stage=4194304 table=2 present=Composited route=true live=LIVE -> ROUTED
```

Every claim in the terminus line is a read-back, not an assertion: `table=2` (the console row
joined rung 0's), `present=Composited` (the present pass ran and was not suppressed),
`route=true` (`fbcon::console_is_routed` after the install), `-> ROUTED` derived from the two.
The panic fallback armed *before* the route went live, so a panic on this image paints the panel,
not a window nobody can see.

**LIVE is not a label — the console kept printing through the window for the rest of the
sitting.** The detach guard (§3.9's `tegra_conwin_live()`) held: `jd2_console_pump` never detached,
and everything after the route — the shell banner (`JD2 — OUT | JD2: interactive shell on the
inherited scanout. Type 'help'.`, line 14864), every keystroke echo of the operator's typed
`bg /fat/vug.elf` (lines 14945–14980), and the `bg` verb's refusal line (15004) — went
through `route_present_banded` into win=2's surface. The JD4 arm still printed
(`console OWNS the panel … screen-on-boot`, line 14865): ownership of the *panel* and routing of
the *console* are different claims and both are true.

##### The console window was clicked — chrome consumed, close REFUSED as designed

Four press/release pairs this sitting, all adjudicated (final census, line 16290: `press=4 rel=4
… consumed=3 … miss=1 stuck=0 nogeom=0 dropped=0 rows=2 -> ROUTING`). The three that hit win=2
exercised chrome paths rung 3 never reached on boot7g:

* **Title-strip press → drag grab.** Lines 14899–14901: `[wm-act] drag-begin win=2
  owner=0xffffff01 at (1091,185) -> grabbed`, `[clickroute] press chrome win=2 … -> drag`,
  `[orinclick] edge=press … hit=yes win=2 … consumed=1 -> CONSUMED`. The release landed outside
  (`-> RELEASE-DROPPED`, 14908) and the drag ended `-> no-move` (14907) — grabbed, not moved, and
  nothing wedged.
* **The close control REFUSED — the CONSOLEWIN law, on the wire.** Lines 14926–14927:

  ```
  [wc-a] close_owner asid=0xffffff01 REFUSED furniture rows=1 ids=[2] — KERNEL FURNITURE IS NOT CLOSABLE
  [clickroute] close=win2 asid=4294967041 at (333,184) settle=furniture-refused
  ```

  A close on the console window is refused by the furniture layer with the window intact — the
  one-way-trip protection §6.1 exists for, taken as a branch on metal.
* **A miss behaved.** Press at (443,106) outside every row: `-> MISS-IDLE`, release
  `-> RELEASE-DELIVERED` (14941–14943). Focus stayed `0x0` all sitting — no click ever raised
  either window, which matters for the ghost question below.

##### Rung 0 and the chrome, reproduced on this image

`[orinwm1] win=1 … present=Composited -> COMPOSITED` (line 14297) and the full `[orinchrome]`
read-back — six frame probes MATCH plus `content=0xff00ff@(960,617) MATCH … -> CHROME-ON-GLASS`
(lines 14298–14304) — printed again, byte-identical in shape to boot7g's. Two flights, two images,
same on-glass verdict: the boot7g result is reproduced, not a one-off.

##### The ghost question — what the capture can and cannot say

boot7g's operator observation (§3.8.1) was that win=1's body is overdrawn between composites by
the console blit and restored by a click's recomposite. Rung 4 removes the mechanism: a ROUTED
console paints its glyphs into win=2's surface (damage-limited, then composited) instead of
blitting the panel behind the compositor's back — and this flight has zero raising clicks
(`raised=0` in every census; focus never left `0x0`), so nothing *else* would have repainted
win=1 either.

**Whether win=1's body actually stayed filled without a click, this capture cannot say: the
`[orinchrome]` probes ran once (14298–14304), before the console window existed, and no
post-routing read-back of win=1 exists on the wire — the answer for boot7h is on the panel and is
Peter's to give.**

What the wire *does* carry, stated at its own scope and no further: `[dock] live … clob=0` on
all 1,168 scan passes across the sitting (final: line 16289) — `clob=` counts *window paints over
the dock strip* (WCK5), so this says the dock's pixels were never overdrawn by a window, not that
win=1's were never overdrawn by the console.

##### The rest of the board, same flight

* **First `:: SCHED: load ::` lines ever printed on Orin silicon** (SMPINSTR, `a50358f0`) —
  recorded with the NET-4F fold in
  [`../01_BOOT_HAL/arch_arm64.md`](../01_BOOT_HAL/arch_arm64.md), **FLOWN 2026-08-25 (boot7h)**,
  along with the `bg` verb's graceful EL0-EL1CORE refusal and the NET-4F single-address-latch
  conviction (buffer 17).
* **JX2-NVC67D reproduced.** `JX2-VERDICT=EFI-OWNED-LIVE` printed again (line 14273) with all
  NEXTTOUCH reads survived — boot7g's channel-census verdict is now a two-flight result.
* **IRQEL-RT PASS, second flight.** `IRQEL-RT: first IRQ taken at EL1 on cpu 0 — banked vector
  path live (ELR_EL1 bank)` (line 14822) — the PASS arm's second consecutive metal capture
  (boot7g: line 12967).
* **A first for the `[redzone]` guard on this board.** Line 14934:
  `[redzone] cpu=0 LOW-REDZONE entered task=1:jd2-console — … ABSORBED … grow this task's stack`.
  The guard worked (absorbed, task resumed, sitting continued for ~100 more minutes), and the
  line's own advice stands as the finding: `jd2-console`'s stack crossed its floor under the
  routed-console + click load. Flagged, not fixed — `sched.rs`/stack sizing is outside this
  fold's lane.
* **Spec replay: PASS.** `unaos/scripts/mbench.py --replay` of the slice against
  `unaos/scripts/specs/jetson-sync1.spec` (with this fold's new rows) reports
  `✅ MBENCH PASS — 16/16 required witnesses, 0 forbidden hit(s), 3134 lines scanned, pending 9/10 matched`.
  The one unmatched PENDING is `TEGRA-SD.*block backend published`, not exercised by this flight.

##### What this flight still did NOT establish

* **win=1 persistence** — stated in full above; the capture cannot answer it.
* **The dock round-trip.** `presses=0 raises=0 unhides=0` on every `[dock]` line: the minimise
  disc was never clicked, so "the dock is a route back" remains exercised only as geometry
  (`dock=GRANTED`), never as a click. That is the next attended item for this ladder.
* **Glyphs-on-glass for win=2.** No `[orinchrome]`-style probe reads the console window's surface
  back off the scanout; `-> ROUTED` + the operator's use of the shell through the sitting is the
  evidence, and a read-back instrument for it would close this the way `[orinchrome]` closed
  rung 0's.
* **Stack cost.** Still Pi numbers (§5) — and the `[redzone]` absorb above is now a measured
  reason to care.

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

⚠ **UPDATED 2026-08-25 — the second prohibition now has a metal conviction behind it, and a
sharper reason.** JD1-DC flew on boot7e and boot7f (§3.8). The read-only survey found the
aperture perfectly readable at the capability registers — the block is **not** powergated — and
then took an EL3 abort on the first *window* register, `0x13802e00`, an offset **inside** the
DTB-declared aperture. So on this silicon the hazard is not only "the block may be gated"; it is
also "a correctly-bounded read of a sub-region the CCPLEX does not decode is EL3-fatal". The window
sweep is gated off at `04d46aae` and the T194-derived offsets are convicted wrong for this chip
(`MODEL-VERDICT=NVDISPLAY-CLASS-C670`). Full record:
[`../01_BOOT_HAL/arch_arm64.md`](../01_BOOT_HAL/arch_arm64.md), **FLOWN 2026-08-25**.

**Provenance for any future nvdisplay work, so the boundary above is not re-litigated.** The
permissive reference path is NVIDIA/open-gpu-doc (MIT) plus OE4T/nv-kernel-display-driver-source
(MIT per file), with NVIDIA/open-gpu-kernel-modules (MIT) for cross-checks. GPL Linux sources are
not used and document the wrong generation anyway — `drm/tegra`'s `of_match` ends at `tegra194`,
which is exactly the map boot7e disproved. **GA10B GPU-core acceleration remains closed**: its
microcode is signed and encrypted with boot-ROM-enforced verification, so no permissive path opens
it. That bounds the GPU only, and this ladder needs none of it.

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
| STACKPOOL `71671423` | ❌ not an ancestor of HEAD | **FETCHABLE on `origin/hw-pi4`** (`git merge-base --is-ancestor 71671423 origin/hw-pi4` → true, re-verified 2026-08-22). SHELLUP `e8dcb09c` and REDZONE `46c94c07` likewise. **Neither is an ancestor of `hw-jetson`**, so the stop-line below still stands for this branch; the remedy is a trunk sync or a cherry-pick. **This row deliberately names no tip sha.** It has now gone stale three times in one day, and the last version pinned itself to a literal that was three tips out of date under a date stamped a day in the future — in a row whose entire thesis is that reachability expires. Quote the tip YOU measured, in the turn you use it. ⚠️ **And run the right check:** `git log --oneline -1 <sha>` and `git cat-file -e` prove only that a commit is in the LOCAL object store. Under `git worktree` — this project's whole layout — every track shares ONE object store, so a peer's *unpushed* commit resolves here exactly like a pushed one; in a plain clone it would have failed honestly. Only `merge-base --is-ancestor` against a freshly-fetched remote-tracking ref answers "can a peer fetch this". |
| REDZONE `46c94c07` | ❌ not an ancestor of HEAD | **is** on `origin/hw-pi4` (verified ancestor 2026-08-22 at tip `2950719b`); **not** on `origin/main`. A 1 KiB absorber under every stack plus a guard above it, **always-on, NOT `witness`-gated — so it ships in a media build.** Two caveats travel with it. (1) **NOT-RUN on metal:** `46c94c07` is not an ancestor of `54ddef41`, the boot-12 build sha, so the absorber is committed and gated but has never flown. Treat a *missing* absorber line as "never fired", never as "the absorber held". (2) **1 KiB is not a wall:** all four recorded overruns (400 / 128 / 96 / 256 B) fit with 2.5× margin, but an overrun past 1024+512 B still escapes. The guards make the failure LOUD, not impossible. |

> ⚠️ **The instrument on this branch is the one STACKPOOL convicted as lying.**
> `arch/aarch64/sched.rs:82-83` at `3dc889e7` documents `[u7stk]`'s `headroom` as
> going negative once a chain has run off the bottom, "printed signed for exactly
> that reason". It cannot: the high-water scan starts at `base`, so `hw <= len`
> and `headroom >= 0` always. A chain 256 B past its floor and one that stopped
> exactly at it print the identical `hw=16384 headroom=0`. STACKPOOL corrected
> that doc in place and added a saturating note. **On this branch the correction
> is absent, so a `headroom=0` reading here means "at or past the floor, amount
> unknown" — never "exactly zero left".**

#### §5.1.1 Correction, re-measured 2026-08-25 at `f0106408` — the MECHANISMS arrived, the COMMITS did not, and SHELLUP still has not

§5.1's table is stale in one direction and still correct in the other. Both
halves were re-run this turn; neither is relayed from a previous session.

| Claim in §5.1 | Re-measured at `f0106408` |
| --- | --- |
| STACKPOOL `71671423` / REDZONE `46c94c07` / SHELLUP `e8dcb09c` "not an ancestor of HEAD" | **still true.** `git merge-base --is-ancestor <sha> HEAD` → false for all three |
| `sched::spawn_prio_stack` ❌ "SHELLUP only" | **now PRESENT** — `arch/aarch64/sched.rs` |
| REDZONE absorber ❌ | **now PRESENT** — `STACK_REDZONE = 1024`, `STACK_HIGHGUARD = 512`, `GUARD_FILL`, and both `[redzone] LOW-REDZONE` / `HIGH-GUARD` reports, `arch/aarch64/sched.rs:41` and `:5225`/`:5298` |
| `[shellup]` census / `at=render:pass` / `RENDER_STACK_SIZE` ❌ | **still absent**, 0 hits tree-wide |

The mechanisms arrived on this branch through the track's own
`f0106408` (`sched(aarch64): STACKPOOL + REDZONE`), not through a cherry-pick of
the Pi commits — which is why the sha rows and the mechanism rows disagree, and
why checking only the shas would have reported the absorber missing when it is
present.

**This does not lift §5.2.** SHELLUP's half is still absent; and per the REDZONE
row above, the absorber on this branch is likewise **NOT-RUN on Orin metal** —
a *missing* `[redzone]` line means "never fired", never "the absorber held".

⚠ **This row has a known expiry.** A sibling executor's SHELLUP cherry-pick was
observed IN FLIGHT (uncommitted `arch/aarch64/sched.rs` in `../UnaOS-orin`, 12:07
on the day above) while this section was being written, taking the
`spawn_prio_stack`-with-caller-sized-stack helper and gating it
`#[cfg(feature = "baremetal")]` with no caller on this branch. Read the tree, not
this table, once it commits — and note that a `baremetal`-gated item on
`hw-jetson` compiles to nothing on every tegra build by §3.2's own chain, so its
arrival will not by itself change what a tegra cascade runs on.

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
the pi seat. **UPDATED 2026-08-22 (orin 4): the push precondition is DISCHARGED.** All
three cascade commits — STACKPOOL `71671423`, REDZONE `46c94c07`, SHELLUP `e8dcb09c` —
are ancestors of `origin/hw-pi4` at `2950719b`, verified this turn. The pi seat was
explicit that its arc is **not** landing soon (boot 12 returned three on-glass defects and
a red replay), so waiting for its boundary would hold this rung for an unknown number of
arcs; it advised a cherry-pick instead, in the order STACKPOOL → REDZONE → SHELLUP.
Three hazards travel with that route and none of them are optional:
**(H1) line-neutrality does not travel.** These commits are line-neutral against *pi's*
base (`46c94c07` is 17+/17−, 8905 → 8905 lines). `arch/aarch64/*.rs` compiles knob-off and
panic `Location` embeds line numbers, so on a different base they may not be — PARITY §5.3
measured eleven added comment lines moving a knob-off hash. Run the byte-identity control
after **each** pick, not once at the end.
**(H2) STACKPOOL's sizes are pi spawn sites.** Take the constant and the helper, **not** the
placement. Pi's `usb-pump`/`input` are pinned to its `input_cpu`, and the pi seat's own audit
found that pinning them to the same core **cancelled the only real redundancy in its service
set** — both call `pump_usb_into_gui`, so either could have covered the other, and boot 11
lost both because they shared cpu 3. The redundancy existed in the code and was destroyed by
the placement. Importing it would import a defect, not a cure.
**(H3) the absorber is 1 KiB** — see the REDZONE row above.

**A placement rule the cascade will tempt you to break, from two seats on the same
day.** Neither is an Orin finding yet; both are mechanisms with numbers on them,
and the desktop cascade is exactly where they would come due.

- **rmbp 5, x86 input arc:** a gate *steal* handed composite work to the device
  core. `x86_usb_pump`'s loop body composites on the calling core, so once the
  steal made the gate acquirable, the core carrying USB polling and input started
  running full passes — `gate=` named c7 in **383 of 492** post-steal samples, 88%
  of those naming any core, against a measured pump median of 140 where 818 × 0.12
  ≈ 98 was predicted. SCHED-X86 has explicit rules keeping composite work off the
  device core and the steal silently violated them. Stated generally, and this is
  the part that travels: **a steal that hands a resource to whoever asks next hands
  it to the busiest core, because the busiest core asks most often.** If any Orin
  lane adopts a steal, the acquirer must be **CHOSEN, not merely NEXT.**
- **pi 4, boot 11:** pinning `usb-pump` and `input` to the same core **cancelled
  the only real redundancy in the service set.** Both call `pump_usb_into_gui`, so
  either could have covered the other; sharing cpu 3 meant one fault took both. The
  redundancy existed in the code and was destroyed by the placement.

One shape: **placement decided by convenience silently repeals a placement rule
that was load-bearing**, and neither repeal announced itself. On this board that
matters twice over, because nothing inherits a dead task's singleton roles —
`steal_ok` is false for every explicitly-pinned task and there is no re-home path
in the tree at all. Arm the cascade knowing the liveness instruments will tell you
it is fine: pi's boot 11 printed `[el0live] verdict=LIVE` **one line after** the
synchronous exception that killed cpu 3, and `:: SCHED: load ::` read `c3=100%`
for the dead core.

---

## §6 The ladder

Seven rungs, each commit-sized, each with the witness that closes it. "Lane"
names the seat that owns the files under the parallel-arc rules in `CLAUDE.md`.

| # | Rung | What lands | Metal witness | Lane |
| --- | --- | --- | --- | --- |
| **0** | **One composited window** — ✅ **LANDED, FLOWN, and CLOSED ON BOTH HALVES 2026-08-25** (§3.8, §3.8.1) | call `wm::reserve_stage` on the tegra path after heap init (§3.3); mint one `wm` row; present it. No furniture, no `pidesk`, no cascade | **wire half CLOSED**: boot7f, capture line 11110 (and again boot7g, capture line 12679), `[orinwm1] win=1 panel=1920x1200 surf=640x400 box=650x444 at (635,378) scale=1 stage=4194304 present=Composited -> COMPOSITED`. **On-glass half CLOSED**: boot7g, capture line 12686, `[orinchrome] win=1 box=650x444 at (635,378) frame=6/6 content=0xff00ff@(960,617) MATCH … -> CHROME-ON-GLASS` — six frame probes and the content probe read back out of the scanout at panel coordinates, all MATCH. ⚠ **at composite time**: the operator measured that the JD2 console blit overdraws the body between composites (§3.8.1), which is rung 4's problem, not rung 0's | jetson |
| **1** | **The cfg leg** — ✅ **LANDED 2026-08-22, less `quarry`** (§3.5.1) | `arm-tegra-desk` leg added (gate 18 → 19 legs); `pidesk`/`quarry`/`livecon` mapped in arroyo's env map; two of the three gate mismatches fixed | `UNAOS_TEGRA=1 ./arroyo check` green 19/19, and green again under `UNAOS_TEGRA_EL0=1 UNAOS_PIDESK=1 UNAOS_LIVECON=1`; the new leg proven to go red on a re-introduced mismatch | jetson (arroyo + `arch/aarch64/syscall.rs`); the `quarry` line is a `video/` edit and is **held** in §3.5.2 |
| **2** | **The desktop seam** — ✅ **LANDED 2026-08-25, and it REFUSES** (§3.2.1) | `tegradesk` feature + `main.rs::tegra_desk_arm` on `tegra_early_stop`'s terminus line + `UNAOS_TEGRADESK` env map + the `arm-tegra-seam` leg (11 → 12 board legs). The seam evaluates its floors and declines at two named stop-lines | **the floors half is UNFLOWN**: `[deskseam] floors …` + `REFUSE reason=…` print on an armed Orin boot, and nobody has taken one. **The `activate()` half is WITHDRAWN, not owed**: `pidesk::activate()` opens the console window and enables the bar, so running it crosses §6.1 *and* §5.2 — it belongs to rungs 3/5, and this row previously asked for something the same document forbids | jetson |
| **3** | **Input routing** — ✅ **LANDED 2026-08-25 as a DEFAULT-OFF knob; FLOWN, ARMED, and ROUTING ON METAL** (§3.7, §3.8, §3.8.1) | `orinclick` (implies `tegra_el0`) wires `jd2_console_pump`'s `Event::Button` arm into `wc_click_route` (§3.4) and adds the `[orinclick]` instrument at the tail of `display_tegra.rs`. **⚠ HANDSHAKE WITH RUNG 2, DISCHARGED IN THIS ARC:** `main.rs`'s `TEGRADESK_CLICK_ROUTED` no longer reads `false` — it reads `cfg!(feature = "orinclick")`, **not** a literal `true`, because `tegradesk` does not imply `orinclick` and a hard `true` would assert a route back on an image that has none: the one-way trip re-entered through the constant meant to prevent it. `arm-tegra-seam` now carries `orinclick` so the assertion is type-checked. COMPILES: gate green 21/21 knob off and on; the new `arm-tegra-orinclick` leg proven to go red. No gate in this tree can boot it — QEMU models no Tegra234 | ✅ **DISCHARGED, boot7g 2026-08-25** (§3.8.1): `[clickroute] press hit asid=4294967042 win=1 (was 0) delivered` (capture line 13084) and `[orinclick] edge=press btn=0x01 at (1009,546) geom=yes hit=yes win=1 owner=0xffffff02 focus 0x0->0xffffff02 consumed=0 -> RAISED` (capture line 13085); release `-> RELEASE-DELIVERED` (13087); census `IDLE-NO-CLICKS -> ROUTING` (13089); a second press on the focused row `-> HIT-SAME` (13092), plus `CONSUMED` (13125), `MISS-SHELL` (13133) and `RELEASE-DROPPED` (13135). Six press/release pairs with `stuck=0 nogeom=0 dropped=0`. **The prior owed item — boot7f's armed-but-unclicked state (`-> ARMED`, capture line 11424, then 48 `IDLE-NO-CLICKS`) — is closed.** Still owed: nothing on the wire; stack cost on this path (§5) is still a Pi number | jetson |
| **4** | **Console as a window** — ✅ **LANDED 2026-08-25 as a DEFAULT-OFF knob; FLOWN AND ROUTED the same day** (§3.9, §3.9.1) | `orinconwin` (implies `pidesk` + `tegra_el0`, and deliberately NOT `orindesk`/`orinclick`) calls the SHARED console-window machinery from `display_tegra::orin_conwin` on `tegra_early_stop`'s terminus line — `panel_console_face_arm` → `panel_console_window_open` → `console_is_routed` — and folds `jd2_console_pump`'s phase-2 `fbcon::detach()` to `if !tegra_conwin_live() { … }` so a routed console stays LIVE. **§6.1 IS NOW A BRANCH:** both ordering terms are read through `cfg!()` and an image missing either gets `[orinconwin] DECLINE reason=ordering-rule held=…` and NO window — measured on the artifact both ways. No `video/` edit; no `pidesk::activate()`, so §5.2 is untouched. Gate green 23/23 knob off and on; `arm-tegra-conwin` proven to go red; knob-off loadable image byte-identical | ✅ **DISCHARGED, boot7h 2026-08-25** (§3.9.1): `[orinconwin] gate … dock=GRANTED … orindesk=1 orinclick=1` (capture line 14828), then `[orinconwin] win=2 panel=1920x1200 cell=7x16 stage=4194304 table=2 present=Composited route=true live=LIVE -> ROUTED` (14833) with the `[wc-x] console-window / console-route first-paint / panic-fallback armed` trio beside it (14830–14832). The route stayed LIVE for a ~107-minute sitting — shell banner, keystroke echoes and verb output all landed through the window path; chrome clicks CONSUMED and the close control `REFUSED furniture` (14926–14927). **Still owed:** the dock round-trip (`presses=0` on every `[dock]` line — the minimise disc was never clicked) and a win=2 glyphs-on-glass read-back | jetson |
| **5** | **The real desktop** | dock, strip, menubar, crystal armed; the full `pidesk` cascade; a tegra `render_service` (§3.6) | the Orin comes up to a desktop | jetson — **blocked by §5.2** |
| **6** | **EL0 tenants** | user windows from EL0 through `SYS_WIN_*`, on the `tegra_el0` regime | an EL0 program owns a window on the Orin panel | jetson |

### §6.0 INHERITED FROM PI, NOT YET TAKEN — two shared-stack fixes waiting on the shelf

Both landed on `origin/hw-pi4` and are portable. Neither is on this branch. Pick
from `hw-pi4` and run this track's own battery — pi's combined battery has NOT
run (bgspread and chromeband both edited `pi4-regression.spec`; the picks
auto-merged but nothing re-ran under pi's hold).

| sha | what | when it becomes ours |
| --- | --- | --- |
| `99b0c867` | **CHROMEBAND** — `wm.rs`: where `row_bytes` makes `chunk_rows < box height`, `paint_window` runs per band but `fill_rect_ceramic` did not clip its row walk to the band | ⚠️ **DO NOT TAKE WITHOUT THE CLOSE-THE-WINDOW TEST — see the conviction below** |
| `1c44ea4b` | **BGSPREAD** — aarch64 `syscall.rs` fixture over-asserted "3 distinct cores"; correct contract is argmin membership on `el0_active` per launch (`inmin=3`). Same edit fixes the doc's tiebreak order: **key 1 = `el0_active`, key 2 = queue depth** | whenever this track copies or inherits that fixture |

> ### ⚠️ CONVICTED ON METAL 2026-08-25 — the banded-path fixes WEDGED the x86 compositor
>
> The rmbp seat took the same two shared-compositor fixes onto x86 (their own
> commits, their own base: `CHROMEBAND 6eba58f7`, `DRAGWIDE 237d9dc9`) and flew
> them. **Closing the shell window under six vugs wedged the board.** Same
> sitting, same workload, discriminated against a clean control:
>
> | image | close the window | wedge |
> | --- | --- | --- |
> | b17, pre-arc | **survived** | `phase=33 row=56` — classic BAR1 blit stall |
> | integration, post-arc | **WEDGED** | `phase=31 row=0` |
>
> **`phase=31` is the band-compose setup (`wm.rs:19375`), building the back layer
> over `stage` — cached RAM, band ZERO. It never reached BAR1.** So this is NOT
> the unanswerable-store class; it is a NEW SOFTWARE STALL in the banded path,
> and the banded path is exactly what these two commits change. `phase=31` is old
> instrumentation (`c9eebcf7`), so the stall genuinely RELOCATED rather than
> being newly labelled — the discriminator was clean.
>
> **What this does and does not say.** It convicts *rmbp's* rebases on *rmbp's*
> base. pi's `441755bb`/`4440cb59` are different commits on a different base and
> are not automatically implicated. But it is the same class of change to the
> same shared code, so:
>
> **BEFORE TAKING EITHER FIX ONTO THIS TRACK, CLOSE A WINDOW UNDER LOAD ON METAL
> AND PROVE IT SURVIVES.** A green gate says nothing here — the wedge needs a
> real close under real load, which no QEMU leg reproduces. The shelf row above
> stands as a record of what the fixes DO, not as permission to take them.
>
> Note the shape, because it is the one this ladder keeps meeting: the original
> defect was **invisible on glass** (per-row overhead, no pixel ever wrong) and
> the fix for it is **visible only as a wedge**, under a workload nobody runs on
> a gate. Neither end of that pair has an instrument that fires in CI.

**Rung 0 is NOT exposed to CHROMEBAND, measured rather than assumed.** Its window
is deliberately panel/3 each way, so at 1920x1200 the box is 640x400:
`row_bytes = 2560`, `chunk_rows = 4 MiB / 2560 = 1638` against a 400-row box —
**single band, chrome paints once.** Exposure needs `cw * ch > 1 Mpixel`, i.e. a
panel above ~9.4 Mpixel; 4K (8.3 M) does not reach it.

**The mechanism refinement is the part to remember, because it explains why this
survived so long.** The defect is **not** triple-painted pixels — out-of-band rows
were already clipped at the bottom of the chain. The 3x is per-row **OVERHEAD**
(ceramic shade, `encode4`, call + bounds) for rows that land nothing: pure
`compose_us` waste, **invisible on glass**. No visual witness could ever have
caught it, and none did. The fix clamps the row walk to the band and leaves the
single-band path verbatim-unchanged; pi measured `waste = 980 -> 0` at 1920x1200
and added a spec leg that REQUIREs `waste=0`, red-proven before the fix.

This is the same family as everything else this branch has paid for: **an
instrument that could not have failed.** Here there was no instrument at all,
because the symptom never reached a pixel.

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

⚠ **UPDATED 2026-08-25, and this is a caveat rung 4 must not read past.** Rung 3 has
LANDED (§3.7) — but as a **default-off knob whose routing has never been exercised**. The
obligation this section states is not "rung 3 is committed", it is "clicks actually route on the
image the console window ships in". Two things follow and neither is optional:

1. **Rung 4 may not ship a console window on an image where `orinclick` is off.** The knob
   and the console window have to travel together, or the minimise disc is a one-way trip
   again — the `#[cfg]` cannot express the law, so the rung-4 arc has to.
   *(⚠ 2026-08-25, boot7h: the rule was EXERCISED as a branch on metal — the gate printed
   `dock=GRANTED … orindesk=1 orinclick=1` (capture line 14828) and opened; and the sibling
   protection fired live: a close on the console window printed `[wc-a] close_owner …
   REFUSED furniture … KERNEL FURNITURE IS NOT CLOSABLE` (14926). The rule itself stays
   binding for every future image; what changed is that its GRANTED branch now has a
   capture. Note the dock way-back is still exercised only as geometry — no minimise click
   has ever been made (§3.9.1) — so the ordering law's justification is not yet
   round-trip-proven.)*
2. ~~**The routing half is still UNFLOWN.**~~ ✅ **DISCHARGED 2026-08-25 by boot7g** (§3.8.1).
   This item asked for "a metal capture showing `[orinclick] edge=… -> RAISED` before rung 4
   leans on the dock as a way back". That capture exists: capture line 13085,
   `[orinclick] edge=press btn=0x01 at (1009,546) geom=yes hit=yes win=1 owner=0xffffff02 focus
   0x0->0xffffff02 consumed=0 -> RAISED`, with `[clickroute] … delivered` beside it at line
   13084. Clicks route on this board; the claim is no longer compile-time. Item 1 above is
   **unaffected and still binding** — the knob and the console window must still travel
   together, because a routed console on an `orinclick`-off image has the same one-way
   minimise disc it always had. Note also that the default armed boot still prints
   `DECLINE reason=no-target` until `UNAOS_ORINDESK=1` (or rung 4 itself) puts a row on the
   panel; boot7g avoided it by carrying `orindesk`, which is why its arm line reads `rows=1`.

Rungs 0–2 have no ordering constraint among themselves beyond the obvious (rung 1
before rung 2 if the seam is to be type-checked by anything). Rung 5 is gated on
§5.2, not on rung 4.

---

## §7 What this document does not claim

- **Rung 2 is claimed landed and REFUSING, never working.** The seam compiles,
  links, is reached (`bl` proven by disassembly) and prints derived verdicts —
  on a build nobody has booted. **UNFLOWN on Orin metal.** Nothing on this
  branch reaches `pidesk::activate()`; §5.2's stop-line is untouched and is now
  enforced by codegen as well as by source.
- **Only rung 1 is claimed done, and only as a type-check.** Rung 1's claim is exactly
  "the armed tegra desktop configuration compiles and a gate leg compiles it" — nothing
  on this branch arms `pidesk::activate()` at runtime, and §5.2's stop-line is untouched.
  ⚠ Corrected 2026-08-25: this bullet used to add "every PROVEN cell in §1 that is ✅
  refers to the JD1/JD2/JD20 panel path, not to the compositor". That is no longer true —
  boot7f made the window-manager, staging-buffer and pointer-routing rows PROVEN in their
  own right (§3.8), and each of those cells names its capture line.
- **Rung 3 is claimed LANDED, COMPILED, ARMED and ROUTING ON METAL.** ⚠ Rewritten
  2026-08-25 by boot7g (§3.8.1). This bullet previously read "never ROUTING" and said
  `wc_click_route` had not been entered on this board. **That is no longer true and the
  claim is now made outright:** clicks route. Capture line 13085 is
  `[orinclick] edge=press … focus 0x0->0xffffff02 … -> RAISED`, with
  `[clickroute] press hit asid=4294967042 win=1 (was 0) delivered` beside it at line 13084,
  and five further router verdicts printed on the same flight (`HIT-SAME`,
  `RELEASE-DELIVERED`, `CONSUMED`, `MISS-SHELL`, `RELEASE-DROPPED`) — so this is branch
  coverage, not one path taken repeatedly. The census reads `-> ROUTING` with
  `stuck=0 nogeom=0 dropped=0`. **What is still NOT claimed:** the stack cost of the router
  on this board (§5's numbers remain Pi numbers), and anything about a build with `pidesk`
  furniture present — boot7g's close-control click printed `settle=furniture-refused`,
  which is the compiled-out path declining, not the furniture working.
- **That a raise PERSISTS on the glass is NOT claimed — and it is now measured that it does
  not.** ⚠ Rewritten 2026-08-25. This bullet used to say "whether the composited z-change
  survives that blit is unmeasured on this board". It is measured, by the operator's eyes on
  boot7g (§3.8.1): **it does not survive, and the recomposite restores it.** The mechanism is
  the one this bullet already named — `focus_changed` ends in `composite()`, which writes the
  front scanout, while `jd2_console_pump` owns the panel through a double-buffered `Screen`
  whose `pal.render()` blits the console back buffer over it. On the flown image nothing
  subtracts the window from that blit, so the body is overdrawn between composites and the
  frame outlives it; the click's `composite()` repaints the body, which is what the operator
  saw. This is rung-4 territory exactly as stated: `orinconwin` (§3.9) makes
  `Screen::present_background` subtract `wm::occluders`, and `orinconwin` is not in the
  boot7g image. **Until rung 4 flies, "the desktop stays on the glass" is not a claim this
  document makes.**
- **Rung 0 is claimed COMPOSITED ON THE WIRE AND ON THE GLASS.** ⚠ Rewritten 2026-08-25 by
  boot7g (§3.8.1). This bullet previously said the on-glass half was "simply not yet measured
  here". It is measured. `[orinwm1] … present=Composited -> COMPOSITED` remains the
  compositor's own derived verdict about a pass it ran — but boot7g added an independent
  read-back: `[orinchrome] … frame=6/6 content=0xff00ff@(960,617) MATCH … -> CHROME-ON-GLASS`
  (capture line 12686), six frame probes at absolute panel coordinates whose `want=` values
  are the theme constants on the `[crispy]` line and whose `got=` values come out of the
  scanout the panel is fed from, plus a content probe at the box centre. Two probes one pixel
  apart return different colours, so this is a one-pixel-accurate frame and not a fill.
  **The claim is scoped to composite time**, for the reason in the bullet above: the probes ran
  immediately after the composite, and persistence is a separate, refuted question.
- **Rung 4 is claimed LANDED, COMPILED, FLOWN and ROUTED — with two named gaps.** ⚠ Rewritten
  2026-08-25 by boot7h (§3.9.1). This bullet previously said no board had booted an image with
  `orinconwin` set. boot7h did: the gate took the GRANTED branch (capture line 14828), the terminus
  printed `… present=Composited route=true live=LIVE -> ROUTED` (14833), and the route stayed live
  for a ~107-minute sitting during which the shell banner, keystroke echoes and verb output all
  went through the window path. The close control refused as furniture (14926–14927). **What is
  still NOT claimed, and each is stated in §3.9.1 at its own scope:** (1) that the routed console's
  glyphs reach the GLASS — no `[orinchrome]`-style read-back of win=2 exists, so `-> ROUTED` plus
  attended use is the whole of the evidence; (2) that win=1's body PERSISTS between composites now
  that the overdraw mechanism is removed — the capture carries no post-routing probe of win=1 and
  no click ever recomposited it, so the ghost-fix answer is on the panel, not the wire. The stack
  cost of `route_present_banded` on this board is likewise unmeasured; the numbers below stay Pi
  numbers — and boot7h's `[redzone] … LOW-REDZONE entered task=1:jd2-console` (14934, absorbed) is
  now a measured reason to go read them.
- **The stack cost of the routing path on Orin is unmeasured.** `[u7stk]` exists
  here and `witness`-gates cleanly, and has never been pointed at the click-router
  depth on this board. §5's numbers remain Pi numbers.
- **The COMPILES column is a type-check, not a link or a boot.** `./arroyo check`
  green proves nothing about what the builder's env→feature map actually puts in
  the image — the full-knob rule in `docs/dev/LAWS.md` requires a `strings` check
  on the artifact, and no rung here has earned one.
- **Rung sizing is an estimate.** "Commit-sized" is a judgement about scope, not a
  measurement. Rung 1's error list was exhaustive as measured, and is now settled:
  fixing them revealed **no fourth error** — `arm-tegra-desk` is green, and green
  with `quarry` added once §3.5.2's held line is applied.
- **The `[u7stk]` numbers quoted in §5 are Pi numbers.** The Orin's stack
  high-water on its own cascade has never been measured. `[u7stk]` is present and
  `witness`-gated here, so it can be — but until it is, §5's bound is inherited,
  not local.
