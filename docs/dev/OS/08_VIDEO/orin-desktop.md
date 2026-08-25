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
| `REFUSE reason=rung3-unlanded+stop-line-5.2` | `match (TEGRADESK_CLICK_ROUTED, TEGRADESK_CASCADE_OK)` | every boot this branch can build |

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
  `3dc889e7`; `arm-tegra-orindesk` (rung 0), `arm-tegra-jd1dc` and now
  `arm-tegra-desk` have been appended since. Count the array, do not trust the line.

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
| **0** | **One composited window** | call `wm::reserve_stage` on the tegra path after heap init (§3.3); mint one `wm` row; present it. No furniture, no `pidesk`, no cascade | one window visible on the Orin panel over the JD2 console; `wm` present counters non-zero on the wire | jetson |
| **1** | **The cfg leg** — ✅ **LANDED 2026-08-22, less `quarry`** (§3.5.1) | `arm-tegra-desk` leg added (gate 18 → 19 legs); `pidesk`/`quarry`/`livecon` mapped in arroyo's env map; two of the three gate mismatches fixed | `UNAOS_TEGRA=1 ./arroyo check` green 19/19, and green again under `UNAOS_TEGRA_EL0=1 UNAOS_PIDESK=1 UNAOS_LIVECON=1`; the new leg proven to go red on a re-introduced mismatch | jetson (arroyo + `arch/aarch64/syscall.rs`); the `quarry` line is a `video/` edit and is **held** in §3.5.2 |
| **2** | **The desktop seam** — ✅ **LANDED 2026-08-25, and it REFUSES** (§3.2.1) | `tegradesk` feature + `main.rs::tegra_desk_arm` on `tegra_early_stop`'s terminus line + `UNAOS_TEGRADESK` env map + the `arm-tegra-seam` leg (11 → 12 board legs). The seam evaluates its floors and declines at two named stop-lines | **the floors half is UNFLOWN**: `[deskseam] floors …` + `REFUSE reason=…` print on an armed Orin boot, and nobody has taken one. **The `activate()` half is WITHDRAWN, not owed**: `pidesk::activate()` opens the console window and enables the bar, so running it crosses §6.1 *and* §5.2 — it belongs to rungs 3/5, and this row previously asked for something the same document forbids | jetson |
| **3** | **Input routing** | `jd2_console_pump`'s `Event::Button` arm calls `wc_click_route` instead of `serial_println!` (§3.4). **⚠ HANDSHAKE WITH RUNG 2: flip `main.rs`'s `TEGRADESK_CLICK_ROUTED` to `true` in the SAME commit.** It is the seam's §6.1 stop-line and it is held `false` precisely because this rung has not landed; leaving it `false` afterwards silently keeps the seam refusing for a reason that is no longer true, and flipping it early is the one-way-trip defect §6.1 exists to prevent | a click on the Orin panel raises and focuses a window; `[clickroute]` on the wire | jetson |
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

- **Rung 2 is claimed landed and REFUSING, never working.** The seam compiles,
  links, is reached (`bl` proven by disassembly) and prints derived verdicts —
  on a build nobody has booted. **UNFLOWN on Orin metal.** Nothing on this
  branch reaches `pidesk::activate()`; §5.2's stop-line is untouched and is now
  enforced by codegen as well as by source.
- **Only rung 1 is claimed done, and only as a type-check.** Every PROVEN cell in
  §1 that is ✅ refers to the JD1/JD2/JD20 panel path, not to the compositor. Rung
  1's claim is exactly "the armed tegra desktop configuration compiles and a gate
  leg compiles it" — nothing on this branch arms `pidesk::activate()` at runtime,
  and §5.2's stop-line is untouched.
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
