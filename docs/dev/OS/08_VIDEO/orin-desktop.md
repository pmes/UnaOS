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
| Desktop-ready seam | `video/desktop_firmware.rs` (564 lines) | ✅ | ❌ gated `all(aarch64, pidesk)` — `video/mod.rs:413` | ❌ and structurally unreachable (§3.1, §3.2) | ❌ |
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
   `baremetal`, no BCM2711. `video/desktop_firmware.rs`'s body touches no VideoCore mailbox
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

### §1.2 The arch-drift ruler was under-reporting by a third — corrected 2026-08-28

The tree-wide drift census is taken with `~/unaos-bench/tools/parity-arch-gates.sh`,
which walks `unaos/crates/kernel/src` (excluding `arch/`, per-arch by construction)
and reports gates one arch has and the other cannot. **Every count taken with it
before 2026-08-28 is a floor, not a total**, and the shortfall is large enough to
change conclusions rather than merely soften them.

Three defects, all closed in v4:

- **Prose-pairing.** The pairing window matched raw substrings over ±25 lines and
  did not distinguish code from comment. A genuinely one-sided gate whose own
  justifying comment named the other arch was scored PAIRED and never reported.
- **Phantom gates.** Comment lines that merely quoted cfg text were counted as
  real gates.
- **Negation.** The scanner skipped any line containing `not(`. That hid
  negation-form gates — `not(target_arch = "aarch64")` is an x86-only gate — and
  also hid every ordinary arch gate that happened to carry a `not(feature = ..)`
  beside it, which is the larger of the two classes.

**Controlled comparison, one variable.** Both runs are the same tree at
`0ed6fee2`, the same 111 `.rs` files; only the ruler differs. The pre-change
script is kept at `~/unaos-bench/scratch/orin10/parity-arch-gates.sh.pre-orin10`,
so this is a re-run, not a recollection.

| ruler | PAIRED | UNPAIRED x86-only | UNPAIRED aarch64-only |
| --- | :-: | :-: | :-: |
| v3 (blind) | 722 | 344 (bare 77 · arch+feature 267) | 245 (bare 99 · 146) |
| **v4 (fixed)** | **708** | **459** (bare 119 · 340) | **285** (bare 104 · 181) |
| delta | −14 | **+115 (+33%)** | **+40 (+16%)** |

Video scope at the same HEAD: PAIRED 344 · x86-only 224 · aarch64-only 63
(v3 read 346 / 181 / 44).

**Why prose-pairing was the one to fix first.** Negation-form under-reported as
flat absence — a uniform shortfall. Prose-pairing under-reported *selectively*,
in favour of exactly the gates someone had bothered to document, because the
pairing window fed on the explanation itself. It corrupted the signal rather than
shrinking it, and it did so in proportion to how well a decline was written.

**The closure, and it is against this branch's own work.** `82d9bc97` (ORIN-VPAR,
this track) ported one `screen.rs` item and declined two *with reasons*. Its
banner comment — `ORIN-VPAR — THE aarch64 PARITY WITNESSES FOR THIS FILE'S THREE
x86-ONLY ITEMS`, six lines below the gates — is what paired `video/screen.rs:361`
and `:364` away. Both are real `all(feature = "witness", target_arch = "x86_64")`
statics; the old window found six `aarch64` mentions in it, all six of them
comments. **The commit that carefully documented its declines is the commit that
blinded the instrument to them.** Under v4 both report UNPAIRED x86-only and the
banner at `:368` is correctly not scored as a gate.

The general form, because it will recur: **an instrument that reads prose cannot
audit code that is required to carry prose.** Decline-with-reasons is right and
drift detection is right; the tool is what had to change. This is not fixed by
writing fewer reasons.

**Residual limitation, stated so it is not rediscovered as a finding.** 67 lines
carry a `not(` that encloses the arch *indirectly* —
`not(all(feature = "deadman", target_arch = "x86_64"))`,
`not(any(target_arch = "x86_64", target_arch = "aarch64"))`. These are classified
by the arch **named**, the same rule every positive gate gets; inverting them
needs a real cfg-expression parser. Most name both arches and land in PAIRED
anyway. Triage the report — do not trust the direction label on a `not(all(..))`
line.

#### §1.2.1 AMENDED SAME DAY — v4 was itself a floor. Two more defects, and the numbers moved again

The v4 figures above were superseded within hours by triage work that used them. Recorded rather
than silently replaced, because the *sequence* is the lesson: each fix made the next defect
visible, and every intermediate number was published as if it were a total.

**Defect 4 — CROSS-ITEM PAIRING (a false negative).** v4 correctly stopped pairing against
comments, but still paired on PROXIMITY ALONE: any counterpart-arch gate inside the ±25 window,
even one guarding a completely different item. Canonical case, found by hand during a
`fbcon.rs` triage and confirmed with a live control: `video/fbcon.rs:2043` is a real, effective,
one-sided bare x86 gate on `PANIC_MIRROR.store(true, …)` inside an **ungated** `panic_screen()`.
It scored PAIRED because the two-armed `CONSOLE_WIN` gate at `:2050` — a *different statement* —
sits seven lines away. Two more archetypes: `main.rs:1235` (a WiFi firmware load) was paired
against `main.rs:1239` (a USB FAT mount), and `drivers/mod.rs:13` (the rMBP battery monitor `smc`)
against `drivers/mod.rs:31` (the Pi microSD `emmc2`) — hiding seven rows in that file alone.
Different capabilities, not per-arch dispatch.

**Defect 5 — SUB-LINE PHANTOM.** v4 skipped a line only when it *started* with a comment marker,
so cfg syntax quoted in a **trailing** comment on a live code line was still parsed as a gate.
`video/fbcon.rs:703` is a real instance. Comment text is now blanked at character granularity,
with string and raw-string literals respected. This had to land first: under a stricter pairing
rule it would have surfaced as a bogus new finding.

v5 also stops counting `cfg!()` const-bool expressions as gates. A `cfg!()` type-checks in BOTH
polarities wherever the code compiles, so it can never make code absent on an arch — this project
already retracted a coverage finding for exactly that reason. They are now reported in a separate
NOT-GATES category.

| ruler | PAIRED | UNPAIRED x86-only | UNPAIRED aarch64-only |
| --- | :-: | :-: | :-: |
| v3 (blind) | 722 | 344 | 245 |
| v4 | 708 | 459 | 285 |
| **v5** | **586** | **516** (bare 138 · 378) | **342** (bare 105 · 237) |

Video scope under v5: PAIRED 328 · x86-only 229 · aarch64-only 67, plus 9 `cfg!()` expressions
tree-wide excluded from the census.

**Against v3, x86-only drift was under-reported by 172 gates — exactly half again as many as it
reported.** Three of v5's 342 aarch64-only rows are this session's own `[orinstkdepth]` instrument
(`main.rs:88`, `:8079`, `:8086`), attributed by diffing the report across the fold; the census
tool and the code it measures moved in the same session, and saying so is cheaper than letting a
later reader discover an unexplained +3.

**v5's rule, stated so it can be argued with.** A gate is PAIRED only if its own attribute carries
both arms; or an exact `#[cfg(X)]`/`#[cfg(not(X))]` complement exists on the same-named item or
adjacent block; or a counterpart in the window names the other arch, **does not name this one**,
and gates an item with the same key. It deliberately OVER-reports adjacent gates on different
items, and that bias is the source of most of the new rows. The bias is intentional: an
over-report is triaged in seconds, an under-report is invisible forever — which is precisely how
prose-pairing survived for months.

**Still unseen by v5, stated so nobody rediscovers it as a finding:** renamed cross-arch twins,
same-name pairs more than 25 lines apart, reordered complement predicates, and the `not(all(..))`
direction-label residual. A control probe now guards the tool itself —
`~/unaos-bench/scratch/orin10/ctrl-expect.sh`, 12 hand-built cases and 21 assertions including the
uppercase-feature (`bcmaS1`) parser-rot mode. It passes 21/21 on v5 and **fails 12 against v4**,
so it discriminates rather than rubber-stamps. Re-run it after any edit to the tool.

⚠ **Numbers published elsewhere with the v3 ruler are not comparable to these.**
`WCG-TRIAGE.md` §1 and §7 quote tree totals taken with it (x86-only 471 → 421 for
the `wcg.rs` port). That *delta* is ruler-consistent — both sides were measured
the same way — but the absolutes are not comparable to any v4 figure, and reading
one against the other suggests a 100-gate improvement that no code change
produced. That file is the rmbp lane's; the correction was relayed to its owner
rather than applied here.

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
| `desktop_firmware.rs` was created by `0750e011` | `video: CONSWIN-PI / MENUBAR-PI M1 — the console gets a window and the bar gets turned on`, 2026-08-13, +226 lines |
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
main.rs:6240       unaos_kernel::video::desktop_firmware::activate()
```

`tegra_early_stop` is declared `-> !` and diverges. It is entered at `main.rs:190`
and never returns, so `kernel_main` never runs on the Orin — and
`desktop_firmware::activate()` at `main.rs:6240` sits on the `kernel_main` path, behind the
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
main.rs:6240       unaos_kernel::video::desktop_firmware::activate()
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
lesson rather than a convenience. The seam calls `video::desktop_firmware::activate`
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
`desktop_firmware::activate` symbol in the armed image and `llvm-objdump -d` finds no
reference to it anywhere: because both consts are `false` in source, the armed
branch is eliminated before linking. `desktop_firmware::activate()`'s call is therefore
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
  (`desktop_firmware::activate()` → `desktop_firmware::activate_now()`) and re-running the gate reds
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

- **`desktop_firmware::activate()` has not run and cannot run on this branch.** The rung-2
  row in §6 gave its metal witness as "`desktop_firmware::activate()` runs on an Orin boot
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
dock, no strip, no menubar, no crystal, no `render_service`, no `desktop_firmware::activate`.

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
* ~~**Spec replay: PASS.** `unaos/scripts/mbench.py --replay` of the slice against
  `unaos/scripts/specs/jetson-sync1.spec` reports
  `✅ MBENCH PASS — 15/15 required witnesses, 0 forbidden hit(s), 1612 lines scanned, pending 5/6 matched`.
  The one unmatched PENDING is `TEGRA-SD.*block backend published`, which this flight does not
  exercise.~~ ⚠ **SUPERSEDED 2026-08-28 (SPECGATE, `0ea79938`) — THIS SLICE IS NO LONGER A GREEN
  REFERENCE.** The PASS was true against the spec as it stood; the spec now carries FORBIDs on the
  discriminated takeover tokens, and boot7f/7g/7h all pre-date them, so all three replay FAIL. That
  is correct, not a regression: these captures came from images that could not say which phase 2 had
  printed. See §3.13. Do not weaken the rows to recover a green — fly the fold.

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
the frozen snapshot `desktop_uefi.rs` ships on x86. The Orin does not inherit it, for the Pi's reason: the
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

⚠ **AMENDED 2026-08-28 — this subsection was true of the site it quotes and FALSE of the image.**
The codegen above is `jd2_console_pump`'s detach, and it was correct. It was not the only detach on
the terminus line. `tegra_rast_demo_maybe` carried a **second, unguarded** `fbcon::detach()`
(`main.rs:6964`) that runs AFTER `orin_conwin()` on the same line (`main.rs:2717`), so on every
image carrying **both** `orinconwin` and `rast` the route was installed and then silenced two
statements later. ⚠ Scope it exactly: without `rast` the whole helper is the `#[inline(always)]`
empty stub at `main.rs:6983`, so boot7h — knobs `UNAOS_ORINCONWIN=1 UNAOS_ORINDESK=1
UNAOS_ORINCLICK=1 UNAOS_NET4=1`, no `UNAOS_RAST` — was never exposed. What WAS exposed is every
`orinconwin` check leg, all five of which carry `rast`, and any future flight that arms both. ~~"the detach guard is the whole of the LIVE claim"~~ was
therefore a claim about one call site read as a claim about the boot. Both halves are fixed in
§3.13 (`d33b6c3e`): the twin takes the same guard, and `live=` stopped being a compile-time literal.
**boot7h's `live=LIVE` (below, and in the §6 ladder table) was an ASSERTION about the build, not a
measurement** — it would have printed `LIVE` on an image whose window never received another glyph.

#### §7's open question, answered in SOURCE only

§7 left this as rung-4 territory: `jd2_console_pump` owns the panel through a double-buffered
`Screen` whose `pal.render()` blits the console back buffer, and whether a composited row survives
that blit was unmeasured. In source it does — `Screen::present_background` subtracts the window
layer (`wm::occluders`, the WC-I loop) on **both** of its cfg arms, the aarch64 one included, so the
desktop present never writes a pixel inside a live window's box. **That is a source reading, not a
metal measurement**, and this rung does not claim otherwise.

#### The §5.2 stop-line is NOT crossed

`desktop_firmware::activate()` is not called. Rung 4 takes exactly the two steps of `activate`'s sequence the
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
post-routing read-back of win=1 exists on the wire — the answer for boot7h was on the panel and
Peter gave it (2026-08-25, operator observation of the boot7h sitting): "I did not see the ghost
window." The rung-4 routed console removed the overdraw mechanism and the body-ghost with it —
the §3.8.1 defect is CLOSED on operator evidence, at operator-evidence scope: no wire-side
read-back corroborates it yet, so the win=2/win=1 glyphs-on-glass probe (§ ladder, owed) remains
the instrument that would make this capture-provable on a future flight.**

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
  ⚠ **SUPERSEDED 2026-08-28 (exec-orin10-specgate) — THIS SLICE IS NO LONGER A GREEN REFERENCE, and
  the reading above is kept as history rather than corrected in place.** The BOOT 7k block added to
  `jetson-sync1.spec` forbids the *pre-fold* shape of the takeover line — `console OWNS the panel
  (Screen back buffer live); screen-on-boot` with no `path=` token — because until the TERMINUS fold
  (`405b21f6`) the two `console OWNS the panel` sites printed byte-identical literals and no capture
  could say which had run. boot7h predates the fold, so replaying the same slice now reports
  `❌ MBENCH FAIL — 17/17 required witnesses, 1 forbidden hit(s), 3134 lines scanned, pending 8/23 matched`,
  the single hit being slice line 1709 — *the same line its own `REQUIRE JD4.*console OWNS the panel`
  matches*. That coincidence is the row's whole argument: its reachability is guaranteed by another
  `REQUIRE` in the same spec, so its silence can never be dismissed as an undriven path. Every
  required witness still passes; nothing regressed in the image. **There is no green reference for
  `jetson-sync1.spec` until a post-`405b21f6` image flies** — the correct response is to fly the fold,
  never to weaken the row.

##### What this flight still did NOT establish

* **win=1 persistence** — stated in full above; the capture cannot answer it.
* **The dock round-trip.** `presses=0 raises=0 unhides=0` on every `[dock]` line: the minimise
  disc was never clicked, so "the dock is a route back" remains exercised only as geometry
  (`dock=GRANTED`), never as a click. That is the next attended item for this ladder.
  ⚠ **INSTRUMENTED 2026-08-25 (§3.11), still UNFLOWN** — `[orindock] park`/`restore`/`census` now
  adjudicate the gesture and its two halves separately. Nothing about the flight's absence changed;
  what changed is that a flight can now be scored from the wire, and the arm line prints the disc's
  and the strip's rects so the operator does not have to guess where to click.
* **Glyphs-on-glass for win=2.** No `[orinchrome]`-style probe reads the console window's surface
  back off the scanout; `-> ROUTED` + the operator's use of the shell through the sitting is the
  evidence, and a read-back instrument for it would close this the way `[orinchrome]` closed
  rung 0's. ⚠ **INSTRUMENTED 2026-08-25 (§3.11); FLOWN boot7j 2026-08-26 and NOT YET CLOSED** —
  `[oringlass]` is that read-back, with the discriminator inverted (the population of the console's
  own paper and ink answers the question; the six frame constants become the discriminator, not the
  subject). boot7j returned `paper=1001 ink=23 stem=0 -> INK-NO-STEM` stably with `frame=6/6` and
  `onpanel=yes`; that verdict could not distinguish anti-aliased text of this console's own colour
  from text in another colour from a foreign surface, so ORIN-GLASSINK (§3.11) split it into three
  and the next flight is what settles which one it was.
* **Stack cost.** Still Pi numbers (§5) — and the `[redzone]` absorb above is now a measured
  reason to care.

### §3.10 LANDED 2026-08-25 (rung 6) — EL0 window tenants: the parity fix, the knob, and the instrument. UNFLOWN

**Measured against `ca4fa538`.** Rung 6's row asked for *"user windows from EL0 through `SYS_WIN_*`,
on the `tegra_el0` regime"* — and the survey found the surface ALREADY THERE and one platform constant
table quietly forbidding it. As with rungs 2-4: everything below COMPILES and is REACHED in codegen;
**no Orin has booted it.** QEMU models no Tegra234 (and `test-arm`'s virt build compiles neither
`tegra` nor `tegra_el0`), so the metal witness is owed, and it rides a later image batch.

#### The finding first: the syscalls were never the gap — the GEOMETRY was

`SYS_WIN_CREATE/_PRESENT/_PRESENT_ROWS/_MOVE/_CLOSE` are the shared arch-neutral WC-B surface
(`arch/aarch64/syscall.rs`, numbers in `una-abi`, x86 twin in `arch/x86_64/syscall.rs`), compiled on
this board by `tegra_el0` alone since JETSON-EL0 M1b. Rung 6 adds **no verb and gates none**. What it
fixes is `arch/aarch64/mmu_tegra_el0.rs`'s FB-region table: that module was written **ten hours
after** CRYSTAL-HD (`92435fb8`, 2026-08-18 00:23 vs `c2d916c1`, 10:09 the same day) and copied the
PRE-CRYSTAL-HD geometry — 8 slots x 64 KiB, 128x128 cap — under a header claiming the values are
"byte-for-byte those of `boot.rs`". False at birth, and two live defects grew out of it:

| defect | mechanism |
| --- | --- |
| **No EL0 program could ever own a window on the Orin** | the shipped `user-vug` asks `SYS_WIN_CREATE(288, 288)` (`SW`/`SH`, 288-as-committed, Peter-ruled 2026-08-18); tegra's 128 cap answered `-EINVAL`, vug printed `:: UVUG: SYS_WIN_CREATE failed ::` and exited(1). `run /fat/vug.elf` on this board died at its first syscall |
| **Latent EL0 fault in every `tegra_el0` witness image** | the WC-B window-verb fixture hardcodes region slot 1's surface at `base + 0x5000 + 0x51000` (the Pi stride, `add x12, x9, #0x56, lsl #12`); against tegra's 0x1_0000 stride the kernel mapped slot 1 at `base + 0x15000`, so the fixture's b10/b11 stores aimed at RESERVED (invalid) leaves |

**The fix is parity, unconditional under `tegra_el0`** (not knob-gated — hiding an ABI repair behind
a demo knob would leave every other `tegra_el0` image broken): `FB_WIN_SLOTS` 8 -> 4,
`FB_WIN_SLOT_SIZE` 0x1_0000 -> 0x5_1000, `FB_WIN_MAX_W/H` 128 -> 288, matching `boot.rs` and the x86
twin exactly. The slot-0 offset (`base + 0x5000`, what `SYS_FB_MAP` returns) is untouched. The
arithmetic is held by three new local const asserts plus `syscall.rs`'s pre-existing WC-B cross-checks
(`FB_WIN_SLOTS <= WIN_MAX`, `cap x cap x 4 == slot size`), which now bind against the tegra values on
every `tegra_el0` leg. Heap cost: per-slot backing 0x85000 -> 0x149000, `alloc_zeroed`ed lazily — 1
shared + 8 slots = 11.6 MiB worst case against the 48 MiB tegra heap, and a slot never claimed costs
nothing.

#### The ownership model, stated against rmbp 6's standing warning

The x86 seat's open wedge is a wm row holding pointers into a dead render function's task-owned
memory. **The aarch64 tenant surface has never had that shape, verified end-to-end this arc:** the
compositor row's `surf` names the slot's FB backing — kernel-heap, allocated once per slot
(`SLOT_BACKING`), never freed, recycled per tenant with a `build_slot` scrub — mapped INTO the tenant
at the fixed window VA, never lent BY it. No compositor pointer ever names memory whose lifetime is
the EL0 task's. Exit path (shared with the Pi, both platform teardowns funnel through it):
`clear_handle_row` -> `win_close_asid` (unmap surfaces, free rows, under `WINDOWS`) ->
`wm::close_owner` + `close_compat` (outside the hold, drain-barrier reason) -> `focus_release`. A
window row cannot outlive its owner into the next tenant's frames, and `[orintenant] reap` is that
funnel's wire witness on this board.

#### Close and minimise policy for tenants, decided and stated

* **A tenant CLOSES** — the boot7h contrast. Kernel furniture refuses the close disc
  (`furniture-refused`, the CONSOLEWIN law); an EL0-owned window runs the ungated CLOSE-CLEAN chain:
  `close_owner(asid)` kills the process with `EXEC_CLOSED_STATUS`, `run` reports
  `closed (window close box)`, the teardown funnel reaps the row. Already built, arch-neutral,
  untouched by this rung.
* **Minimise is reported, not repainted as policy.** The minimise arm is ungated, so a tenant can be
  parked on any image; the routes back are the dock (`pidesk` aboard — the conjunction image) or
  kill/exit, and the dock round-trip is still the ladder's next attended item (§3.9.1). The census
  prints `pidesk=` and the arm line prints all four sibling knobs so a capture names which image
  shape a park happened on.

#### What landed

| where | what |
| --- | --- |
| `crates/kernel/src/arch/aarch64/mmu_tegra_el0.rs` | the CRYSTAL-HD parity fix + 3 const asserts (unconditional under `tegra_el0`) |
| `crates/kernel/Cargo.toml` | `orintenant = ["tegra_el0"]` — implies nothing else; the syscalls are deliberately NOT behind it |
| `arch/aarch64/display_tegra.rs` (file TAIL, all `#[cfg(orintenant)]`) | `orin_tenant_arm` (terminus: `wm::reserve_stage` — §3.3's F-family reason, so a bare-`orintenant` image's first EL0 present never grows the stage under its own IRQ mask — plus the pre-state line), the five note fns, `orin_tenant_census` |
| `main.rs` terminus line + JD2 sweep line | `orin_tenant_arm()` / `orin_tenant_census(sweep_tick)` appended — **line-neutral, 6990 -> 6990 lines** |
| `arch/aarch64/syscall.rs` | four LINE-NEUTRAL in-place call sites (create-refuse, create, close, reap-row/reap-done) + file-tail `orin_tenant_win_stats` (+26 lines, all past old EOF; Pi panic Locations unmoved) |
| `arroyo` | `UNAOS_ORINTENANT` env map; `arm-tegra-tenant` leg (arm-tegra-el0's list + `orintenant` — the shippable bare-tenant shape, `pidesk` OFF); `,orintenant` on `arm-tegra-conwin-tenant` (the full-conjunction flight cross; renamed from a second `arm-tegra-conwin` row at `dbda97fa`). Board legs 14 -> 15, gate 23 -> 24 |

**No `video/` edit, and none was needed** — `wm::create/present/close_owner/reserve_stage/count` and
the whole chrome/close/raise vocabulary are the shared implementation already reached from three
seams; this board now runs those same bytes from a fourth caller, EL0's SVC.

#### The instrument

Per-event wires (`create -> TENANT-WINDOW` / `-> HEADLESS-COMPOSITOR-REFUSED` /
`DECLINE reason=geometry-over-max`, `close -> CLOSED-BY-OWNER`, `reap -> TENANT-REAPED`) plus the
~10 s census from the pump's own drain loop — rung 3's liveness argument verbatim. Census verdict
ladder, each arm reachable and none constant: `FAIL reason=geometry-refused` (a create was refused
over the cap — the pre-parity defect observed live; must never print on a post-parity image running
shipped binaries) > `DECLINE reason=headless-rows` (verbs green, compositor refused rows — glass
empty, said out loud) > `TENANT-LIVE` > `TENANT-EXITED-CLEAN` > `IDLE-NO-TENANTS` (**UNRUN, never
PASS**). Presents are counted from the pre-existing global `FB_PRESENT_COUNT` (bumped by the ONE
shared present body), so no present verb was touched. All tokens are longer than 8 bytes.

#### Gate, measured on artifacts

* `UNAOS_TEGRA=1 ./arroyo check` — green, **24 legs as of this arc (15 board + 9 x86 pairwise-mix)**; green again
  under `UNAOS_ORINTENANT=1` alone (knob self-sufficient) and under the full four-knob conjunction.
* **Go-red proven:** renaming `orin_tenant_win_stats` reds **exactly**
  `arm-tegra-conwin-tenant` + `arm-tegra-tenant` — the two legs carrying the knob — every other leg green.
  Restored -> green.
* `./arroyo test-arm` and `./arroyo test` (x86) — both green; `awk '/PANIC|panicked/'` over
  `serial-arm.log` -> 0 (the single `/FAIL/` hit is the known `[botclaim]` prose).
* **Knob-off byte identity, loadable-image level** (same tree, same absolute path, arc applied vs
  `git apply -R`, two independent `CARGO_TARGET_DIR`s; method control ran first and matched):
  jetson default (`tegra,tegrasmp`) `cccc97c9…` 1 541 332 B **identical**; Pi `kernel8.img`
  (`baremetal,skip_xhci,witness,pidesk,quarry,livecon`) `0d3f47a5…` 2 162 320 B **identical** (elf
  delta `.strtab`-only, program headers identical — the documented benign class). The armed-EL0
  image (`tegra,tegrasmp,tegra_el0`) **differs by design** — that is M1 landing — and the knob-on
  image differs from default, so the knob is not vacuous.
* **Witness presence:** all 11 `[orintenant]` marks/verdict strings one-hit in the armed artifacts
  (`LC_ALL=C grep -a -o -F`, never `strings`), **zero** in the knob-off default AND in the
  armed-EL0-without-knob image (negative control).
* **Reachability by disassembly**, all eight edges: `tegra_early_stop -> orin_tenant_arm`,
  `jd2_console_pump -> orin_tenant_census`, `aarch64_svc_handler -> note_refuse/note_create/
  note_close` (the verbs inline into the SVC dispatcher — the EL0 path itself),
  `win_close_asid -> note_reap_row`, `clear_handle_row -> note_reap_done`,
  `census -> syscall::orin_tenant_win_stats`.
* **Freshness:** `git log --all -S` -> 0 prior commits for `orintenant`, `[orintenant]`,
  `IDLE-NO-TENANTS`, `TENANT-WINDOW`, `TENANT-REAPED`, `orin_tenant_arm`, `arm-tegra-tenant`,
  `UNAOS_ORINTENANT`.

#### What the metal flight must watch for

Image: the §6.1 conjunction + this knob
(`UNAOS_ORINTENANT=1 UNAOS_ORINCONWIN=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1`), then
`run /fat/vug.elf` at the shell. Expected wire, in order: `[orintenant] arm … cap=288x288 rslots=4 …
-> ARMED` (the arm line IS the parity witness — a pre-parity image reads `cap=128x128`);
`[orintenant] create asid=… win=… surf=288x288 wm-bound=1 -> TENANT-WINDOW`; census
`IDLE-NO-TENANTS -> TENANT-LIVE`; presents climbing; on ESC/exit either `close -> CLOSED-BY-OWNER`
or `reap … -> TENANT-REAPED`, census `-> TENANT-EXITED-CLEAN`. `FAIL reason=geometry-refused` on
this flight = the parity fix is not in the image (STOP: wrong media). Watch also: the `[redzone]`
guard (boot7h already absorbed a LOW-REDZONE on `jd2-console`; the tenant's present path adds load —
§5's stack numbers are still Pi numbers), and the EL0 input delivery (`run` has never been typed on
this board's metal — boot7f/g/h all confirm — so the census plus vug's own witnesses adjudicate the
whole `run`-plus-input chain, not only the window half). `bg /fat/vug.elf` stays REFUSED
(EL0-EL1CORE) — use `run`, pinned core 0; placement policy untouched.

#### What this rung deliberately did NOT do

* **No rung 5.** No furniture arming, no tegra `render_service`, `TEGRADESK_CASCADE_OK` untouched.
* **No `video/` edit**, and no `sched.rs` edit (two parallel executors hold claims there).
* **No ordering-rule enforcement in the arm.** Unlike `orin_conwin`, this rung cannot decline its
  way out of the hazard it reports: `SYS_WIN_CREATE` is reachable from EL0 on every `tegra_el0`
  image whatever the knob says, so the census REPORTS the image shape instead of pretending a
  `#[cfg]` could hold the syscall off.
* **UNFLOWN.** Every claim above is a build-time or artifact measurement. The flight card above is
  the adjudicator.

---

### §3.11 LANDED 2026-08-25 — the two items §3.9.1 left owed get an adjudicator. UNFLOWN

§3.9.1's "What this flight still did NOT establish" named two things after boot7h and gave neither an
instrument: **glyphs-on-glass for win=2**, and **the dock round trip** (`presses=0 raises=0 unhides=0`
on every `[dock]` line — the minimise disc has never been pressed on this board). This section is
that pair of instruments, landed as one DEFAULT-OFF knob.

#### The finding first: neither rung was missing a MECHANISM

It is worth saying plainly, because it changes what the metal flight is for. The dock round trip
already ships end to end on this branch, and has since rung 3:

| step | who does it | since |
| --- | --- | --- |
| a press on the minimise disc is recognised | `wc_click_route`'s `minimise_hit` arm (`arch/aarch64/syscall.rs`) | rung 3 |
| the row is parked below the shell | `wm::minimise` — `z = 0`, and `set_hidden` publishes the owner's hidden bit | shared |
| the parked row still appears in the dock's tile model | `wm::dock_scan` enumerates rows regardless of visibility, deliberately | shared |
| a press on the strip beats every window arm | `strip::press_route`, called from the router's `pidesk` furniture arm | PI-DESK |
| the tile press raises and un-hides | `dock::press_at` -> `focus_set` + `wm::focus_changed` | shared |

Nothing in that column is new and nothing in it is knob-gated beyond `pidesk`, which `orinconwin`
already carries. **What was missing is an ADJUDICATOR** — a wire that says which half of the trip
happened, and a read-back that says whether the half that happened reached the glass. So this rung
adds no behaviour at all: not one pixel path, not one routing decision, not one control.

#### What landed

`orinladder`, DEFAULT OFF, arming two instrument families at the tail of
`arch/aarch64/display_tegra.rs`, plus two LINE-NEUTRAL statements appended to `main.rs`'s
`tegra_early_stop` terminus line and its JD2 phase-2 sweep line.

**⚠ NOT ONE LINE OF `video/` IS TOUCHED, and that is a lane decision rather than a convenience.** The
`hw-rmbp` track carries an unlanded ~1200-line `wm.rs`/`screen.rs` delta that meets this branch at the
next sync, so every fact the instrument needs was taken through an accessor that already exists:

| fact | accessor | note |
| --- | --- | --- |
| which row is the console window | `wm::info` over `1..=MAX_WINDOWS`, matching `owner_asid == wm::KERNEL_OWNER_CONSOLE` | a public const; `close_owner` refuses the reserved kernel band, so that owner names exactly one row. No "which window is the console" accessor was minted |
| its box on the panel | `wm::info`'s `x`/`y`/`w`/`h`/`scale` | the outer box is re-derived the way `panel_console_window_open` built it — content origin minus `BORDER` and `TITLE_H + BORDER` |
| where the minimise disc is | `wm::control_disc_rect(id, Ctrl::Minimise)` | the painter's own accessor, for `close_box_rect`'s stated reason: a fixture must press the disc the compositor actually drew |
| whether a way back exists | `dock::strip_rect` + `wm::dock_scan` | the registry hook `wm::erase_clip` reads, so the strip named is the strip that will be painted |
| what the dock's last press did | `dock::last_press_outcome` | CLICK-BAND's own witness word |
| what is actually on the glass | `FrameBuffer::read_pixel` | the compositor's own verify primitive and the one place the read-back ban is lifted by name — `orin_chrome_probe`'s reason, verbatim |

If any of those had needed a new signature, a new field or a reordering in `wm.rs`, the arc would have
stopped and asked. None did.

#### The knob, and why its implication set is FORCED rather than chosen

`orinladder = ["orinconwin", "orinclick", "orindesk"]`, env `UNAOS_ORINLADDER`.

`orin_conwin` itself REFUSES to open a console window on an image missing either `orindesk` or
`orinclick` (`[orinconwin] DECLINE reason=ordering-rule`, §6.1), and this instrument's entire subject
is that window and the minimise disc on it. So:

* `orinladder = []` would arm a probe for a window the build guarantees does not exist;
* `orinladder = ["orinconwin"]` would arm one for a window `orin_conwin` declines to open;
* `orinclick` is additionally what makes the disc a GESTURE rather than a decoration — without the
  router there is no press to witness.

`orinconwin` transitively supplies `pidesk` (the `dock`/`strip` modules) and `tegra_el0` -> `tegra`,
so the closure IS the flight image. **One env var now arms what boot7h needed three for.** This is
`orinclick = ["tegra_el0"]`'s own argument applied one rung up.

New leg `arm-tegra-ladder` (board legs 19 -> 20, gate 28 -> 29), hosted on `arm-tegra-conwin`'s
full-conjunction list + `orinladder`; no lighter host exists, because cargo would widen any shorter
list straight back to this one. It deliberately does NOT carry `orintenant` — rung 6's tenant path is
orthogonal and already has its own leg.

#### The instruments

**`[oringlass]` — rung (a), the win=2 read-back. Its discriminator is INVERTED from `[orinchrome]`'s.**
That probe knew a constant inside the CONTENT (the magenta block `orin_wm1` writes) and used it to
separate "chrome missing" from "nothing landed". Here the content is anti-aliased TEXT: nobody can
predict which glyph is at any coordinate, and `panel_console_face_arm` sets `c.aa = true`, so glyph
EDGE pixels are alpha blends and no single pixel carries an exact expectation. What IS predictable is
the POPULATION — a text surface on the glass shows the console's own paper AND fully covered strokes
of its own ink, and nothing else on this panel shows that pair. So the roles swap: the CENSUS answers
the question, and the six frame constants become the discriminator.

Sampling: 8 scanlines spread down the content box, 4 contiguous 32-pixel runs each, 1024 samples.
Contiguous runs rather than an even grid because at the bench panel the content box is ~1900 px wide
against a 7-px cell — an evenly spread scanline samples about one pixel per four character cells and
can miss every stroke on a sparse line, where a 32-pixel run crosses ~4.5 cells end to end. The
question wants LOCAL density and GLOBAL spread; this is the cheapest shape with both. Classified
against `fbcon`'s own documented pair (`BG_DEFAULT = 0x0000_0000`, `FG_DEFAULT = 0x00C0_C0C0`,
`fbcon.rs:114-115`), restated in `display_tegra.rs` with provenance rather than reached for — making
them `pub` would be exactly the shared-seam edit this rung refuses.

| verdict | what the wire is saying |
| --- | --- |
| `DECLINE reason=no-console-row` | no `wm` row carries `KERNEL_OWNER_CONSOLE`. Not a failure of this rung — the image is not the conjunction, or `orin_conwin` declined and named its own reason above |
| `DECLINE reason=no-panel` | headless boot; there is no scanout to read back |
| `UNREADABLE` | every sample fell outside the mapped length: the row's geometry and the panel's disagree. A defect, and one no present count could show |
| `WIN2-NOT-ON-GLASS` | not ONE sample is the console's background. Whatever occupies those panel coordinates, it is not this window's surface |
| `BLANK-NO-GLYPHS` | every sample IS the background: the surface reached the glass and its TEXT did not. **The exact shape §3.9.1 could not rule out** |
| `GLYPHS-AA-NO-CHROME` | no fully covered stroke, but blends are a supermajority of the ink AT TWO OR MORE COVERAGE LEVELS: anti-aliased text of this console's own colour, with the frame overdrawn |
| `GLYPHS-AA-ON-GLASS` | the same blend supermajority with the frame intact. **Rung (a) closed on anti-aliased evidence** — `stem=0` here is a property of the face, not a defect |
| `INK-FLAT-FILL` | a blend supermajority at exactly ONE level (`blevels=1`): a flat fill of a ramp colour, not text. `video::PANEL_BG` (`0x001E_1E1E`) is such a colour — this arm is what stops the desktop showing through from reading as a pass |
| `INK-OFF-COLOUR` | no stroke, no blend supermajority, and one OFF-RAMP value holds a majority of the ink: text (or a fill) in a colour that is not `LAD_INK`. `ink1=` names the measured value |
| `INK-NO-STEM` | non-paper pixels inside the box, but not one fully covered stroke of the console's own ink, no blend supermajority and no dominant colour — scattered foreign values |
| `GLYPHS-NO-CHROME` | paper and ink strokes both on the glass, and the FRAME is not — §3.8.1's measured JD2-blit overdraw, caught in the act |
| `GLYPHS-ON-GLASS` | frame and glyphs both read back at panel coordinates. **Rung (a) closed** |

##### ORIN-GLASSINK — why the ink census names what it saw (2026-08-26)

boot7j read `paper=1001 ink=23 stem=0 -> INK-NO-STEM`, stably, across at least four censuses, with
all six chrome probes `MATCH` and `onpanel=yes` — and that verdict was then read as "the text is not
on the glass". **It never supported that reading.** `ink` was defined as *not exactly `LAD_PAPER`* and
`stem` as *exactly `LAD_INK`*, so the pair says only that 23 samples were neither exact black nor
exact light-grey. Three different worlds produce it, and the probe reported `first=` (the first
sample, which was paper) while throwing away the one datum that separates them — what the 23 ink
samples actually WERE.

The census now partitions the non-paper population by geometry in colour space. An anti-aliased glyph
edge at coverage `a` is `paper + a*(ink - paper)` in every channel with the same `a`, so
`lad_classify` recovers `a` from the channel with the widest paper→ink span and requires the other two
to agree within `LAD_BLEND_TOL` (8 of 192, ~4%). The test is written against the two constants as
VARIABLES, never against black: correct either constant and the test follows it.

| field | what it counts |
| --- | --- |
| `blend=` | samples ON the PAPER→INK segment — partial coverage of exactly this console's two colours |
| `blevels=` | `0` / `1` / `2+` — how many DISTINCT blend levels were seen. **The term that makes the AA pass safe**, see below |
| `off=` | non-paper, not exactly ink, and NOT on that segment — a foreign colour |
| `ink1..ink3` / `n1..n3` | the heaviest non-paper values with their counts (`n=0` = no such entry) |
| `inkvals=` / `exact=` | distinct non-paper values, and whether the counts are exact or LOWER BOUNDS |

`paper + ink == read` and `stem + blend + off == ink` hold on every line: a line that does not balance
is a defect in the instrument, not in the panel. The histogram is a fixed six-slot Misra-Gries table
(48 bytes of stack, no allocation, one linear scan per non-paper sample), so any value above a seventh
of the ink population is retained; once it has to start decrementing, `exact=no` says the counts are
lower bounds — and `exact=no` is itself evidence, since it means more than six distinct non-paper
values, the signature of a scattered field rather than of text in one colour.

⚠ **`blevels` is load-bearing, and a host run of the shipped `lad_classify` is what found out why.**
`video::PANEL_BG` is `0x001E_1E1E` — a GREY, therefore ON the black→light-grey ramp, therefore a
"blend" by the segment test. A box holding some paper and a lot of desktop would otherwise clear the
supermajority and read as a PASS. What separates a flat fill from anti-aliased text is not the colour
but the NUMBER OF COVERAGE LEVELS: a fill has exactly one; glyph edges sampled across many strokes
have several. The AA verdicts therefore require `blevels=2+`, and `blevels=1` gets its own verdict
(`INK-FLAT-FILL`) rather than being folded into the scatter bucket. Two extra locals, and the only
false-PASS path this rung had is closed.

⚠ **`LAD_INK` was deliberately NOT changed to match the board.** A constant tuned to the observation
would make the probe agree with reality by construction and prove nothing. If the evidence says it is
wrong, `INK-OFF-COLOUR` reports the measured value and correcting it is a separate decision on
separate evidence.

Rung (b)'s ledger derives `painted` from `lad_glass_painted` — the one place the passing set is
written down (`GLYPHS-ON-GLASS`, `GLYPHS-NO-CHROME`, `GLYPHS-AA-ON-GLASS`, `GLYPHS-AA-NO-CHROME`) —
so an anti-aliased restore cannot read as `FAIL reason=restore-blank`. `INK-OFF-COLOUR` and
`INK-FLAT-FILL` stay OUTSIDE that set on purpose: neither a foreign colour nor a flat fill in the box
is a confirmation of this console's text.

**`[orindock]` — rung (b), the round trip. It samples EVERY tick and prints every ~10 s**, and that
asymmetry is the design. Rungs 3 and 6 census COUNTERS, which are monotone: a 10 s cadence loses
timing, never events. This one reads a STATE — the console row's `z` — and the event is a park
followed by a restore that an operator completes in seconds. A 10 s sampler would see the row on the
panel, then on the panel again, and report `IDLE-NEVER-PARKED` for a round trip that happened. So the
edge detector runs every tick (one `wm::info` walk, ~12 table lookups under one lock, ~4/s — a
strictly smaller footprint than the `wm::hit_test` rung 3 already takes per pointer event) and only
the census PRINT is at the 10 s period.

**Rung (a) does not depend on rung (b)'s gesture.** The census takes a read-back of its own on
`seq == 1` (~10 s after the arm, with the boot's own tail already in the window) and every ~60 s after
that, budgeted. The reason is an ambiguity the arm sample alone cannot resolve: the arm fires at the
terminus, moments after `panel_console_window_open` re-rendered the console into the new surface, so a
near-empty window would read `BLANK-NO-GLYPHS` for a TIMING reason with no second opinion available
until somebody minimised and restored. A genuine blank stays blank across every sample; an arm-time
artefact resolves on the next line. A PARKED row is deliberately NOT probed (`glass=parked`): its
content box holds whatever is behind it, so a read-back there would answer a question about the
desktop and print `WIN2-NOT-ON-GLASS` for a window that is correctly hidden.

The `park` line carries whether the dock's tile model contains the row **at the moment of the park**,
because that is the question §6.1 is about. The `restore` line derives `via=` from
`dock::last_press_outcome()`, so a `<TAB>` back is `RESTORED-OFF-DOCK` and is **not credited** as the
round trip; and it re-fires the read-back, which is what makes "a restore that paints nothing" a
different line from a restore that paints.

| census verdict | what the wire is saying |
| --- | --- |
| `DECLINE reason=no-console-row` | no console window on this image. No subject; not a failure |
| `DECLINE reason=no-dock-strip` | the panel cannot host the strip. The trip is not merely untaken, it is impossible |
| `FAIL reason=park-no-tile` | parked NOW and the dock's tile model does not contain the row. **The one-way trip, realised.** Structural, never timed — a slow operator must not read as a failure |
| `FAIL reason=restore-blank` | it came back and every read-back said the content did not paint |
| `PARKED-AWAITING-DOCK` | parked, a tile names it, nobody has pressed it. The honest in-flight state |
| `DOCK-ROUNDTRIP` | a dock tile press brought it back AND the read-back found its glyphs on the glass. **Rung (b) closed** |
| `RESTORED-NOT-VIA-DOCK` | it came back, but not through a dock tile. NOT closed, and the census refuses to pretend |
| `IDLE-NEVER-PARKED` | the disc has not been pressed. **UNRUN, never PASS** — boot7h's state, and it must stay distinguishable from a passing one |

#### Gate, measured on artifacts

* `UNAOS_TEGRA=1 ./arroyo check` — green, **29 legs (20 board + 9 x86 pairwise-mix)**, exit 0; green
  again under `UNAOS_TEGRA=1 UNAOS_ORINLADDER=1`, whose DEFAULT aarch64 leg banner reads
  `ehcihid,kbdwit,sdhcblk,smolnet,tegra,tegrasmp,orinladder,orinconwin,orinclick,orindesk,pidesk,tegra_el0`
  — the ARMED polarity, not merely the knob-off twin.
* **Go-red proven:** renaming `orin_ladder_arm` reds **exactly** `arm-tegra-ladder`
  (`error[E0425]: cannot find function orin_ladder_arm`, exit 101) while `arm-tegra-conwin` and
  `arm-tegra` stay green (exit 0). Restored -> green.
* `./arroyo test-arm` exit 0 — 2 `-> PASS`, 0 `-> FAIL`, 0 `PANIC|panicked` over 193 `::` markers;
  the single `/FAIL/` hit is the known `[botclaim]` prose. `./arroyo test` (x86) exit 0 — **44
  `-> PASS`, 0 `-> FAIL`, 0 `PANIC|panicked`**.
* **Knob-off byte identity, MEASURED on `objcopy -O binary` FLAT images** (never `.elf` shas — a
  `.strtab` uniquing moves without a mapped byte). Same worktree, same absolute path
  (`~/unaos-bench/scratch/orin7/ladder`), arc applied vs `git apply -R`, independent
  `CARGO_TARGET_DIR`s, **method control run FIRST and matched**. Jetson default (`tegra,tegrasmp`):

  | image | sha256 of the flat image | bytes |
  | --- | --- | --- |
  | B (arc applied) | `0de025c1a645ee1826969b65f76705f762928d6dc3fe8038a3f6cd87150f07ec` | 1 543 316 |
  | B2 (method control, same source, second target dir) | `0de025c1a645ee1826969b65f76705f762928d6dc3fe8038a3f6cd87150f07ec` | 1 543 316 |
  | A (`git apply -R`, baseline) | `0de025c1a645ee1826969b65f76705f762928d6dc3fe8038a3f6cd87150f07ec` | 1 543 316 |

  **A == B == control — identical**, and re-measured identical again after the census read-back was
  added (`0de025c1…`, 1 543 316 B — the third build of the B side). `main.rs` is 6990 lines before and
  after, so no panic `Location` renumbers; every one of the 669 new source lines is APPENDED at the
  tail of `display_tegra.rs`. The armed image differs by design (`3163206c…`, 1 907 176 B), so the
  knob is not vacuous.

  ⚠ **The Pi `kernel8.img` half was NOT measured** and is not claimed: a bare `cargo build` of the
  `baremetal` set fails to link without `./arroyo kernel8`'s linker script (`undefined symbol:
  __bss_end`, `__stack_top`). The structural argument is strong — `display_tegra.rs` is not compiled
  on the Pi at all, and `main.rs`'s two edits are in-line appends that leave the file at 6990 lines —
  but an argument is not a measurement, and this one is offered as the former.
* **Witness presence, and TWO negative controls.** All 22 knob-exclusive `[oringlass]`/`[orindock]`
  marks and verdict strings hit in the armed flat image, by `LC_ALL=C grep -a -o -F` on the binary AND
  by `strings -a` (identical counts), fragments >8 bytes throughout; **zero** in the knob-off jetson
  default AND **zero** in the same §6.1 conjunction built with `orinladder` OFF (`f55dc2d7…` —
  boot7h's own image shape). The second control is the sharp one: it proves the marks come from this
  knob and not from a sibling. Stated honestly: two further fragments this instrument prints,
  `UNREADABLE` and `UNMAPPED (off-panel`, are **shared string literals with `[orinchrome]`** and are
  deduplicated by the linker, so they hit in the negative control too and carry no discrimination.
  That is why the proof rests on the other 22 and not on them.
* **ORIN-GLASSINK re-measured the same three properties on its own A/B (2026-08-26).** Knob-off jetson
  flat image, `llvm-objcopy -O binary` on the `ehcihid,tegra,tegrasmp` build, baseline and changed tree
  BOTH `f9f95424e16ac855408e4ecd2aa89419c47ec2a844183f94087706f4655046b2` and `cmp`-identical — the
  same sha this section's baseline already carried, which is what makes the A/B the canonical one
  rather than a private definition of it. The six new fragments (`GLYPHS-AA-ON-GLASS`,
  `GLYPHS-AA-NO-CHROME`, `INK-OFF-COLOUR`, `INK-FLAT-FILL`, `" inkvals="`, `" blevels="`; 9-19 bytes)
  hit exactly once each in the armed image by `grep -a -o -F` AND by `strings -a`, and **zero** in
  both negative controls — the knob-off default and the same §6.1 conjunction built with `orinladder`
  OFF. Reachability by disassembly: `tegra_early_stop -> orin_ladder_arm` (`bl` at `0x737f8`),
  `jd2_console_pump -> orin_ladder_census` (`bl` at `0x72ed8`), `orin_ladder_arm ->
  orin_glass_probe` (`0x11868c`), `orin_ladder_census -> orin_glass_probe` (`0x11bba4`, `0x11bbb8`);
  and **all ten** verdict strings are materialised by live `adr`/`adrp+add` inside
  `orin_glass_probe`, the four new ones at `0x435a6`, `0x435b4`, `0x435c1`, `0x435d4` — with
  `orin_ladder_census` itself referencing `GLYPHS-ON-GLASS`, `GLYPHS-NO-CHROME` and both AA verdicts,
  which is `lad_glass_painted` inlined and therefore the proof that the passing set on the wire is
  the passing set the round-trip ledger uses.
* **The classifier was RUN, not argued.** `lad_chan`, `lad_classify`, `lad_hist_add`, `lad_hist_rank`
  and `lad_glass_painted` were extracted VERBATIM from `display_tegra.rs` (`awk` on the function
  bodies, `#[cfg]` lines stripped — a replica would have been worthless) and compiled for the host:
  13 colour cases (exact paper/ink, 0.5%/50%/99% coverage greys, a channel exactly at the ±8
  tolerance and one past it, white, saturated red and blue, `orin_wm1`'s magenta, a same-luma wrong-hue
  dark red, and `PANEL_BG`), 3 histogram cases (exact counts with 3 distinct values; the appended-
  scatter signature `inkvals=36 exact=no`; an interleaved run where the Misra-Gries count is a REAL
  lower bound, `n1=252` against a true 300), 5 end-to-end verdict scenarios and the 12-verdict
  `lad_glass_painted` set. All pass. **That run is what found the `PANEL_BG` hole** — it was not
  reasoned to, and the first version of this instrument would have shipped with it.
* **Reachability by disassembly, not by banner.** `tegra_early_stop -> orin_ladder_arm`
  (`bl` at `0x747a4`, immediately after `orin_conwin`'s at `0x747a0`);
  `jd2_console_pump -> orin_ladder_census` (`bl` at `0x73e84`, after `orin_click_census`'s at
  `0x73e7c`); `orin_ladder_arm -> orin_glass_probe` / `dock::strip_rect` / `wm::dock_scan`;
  `orin_ladder_census -> wm::dock_scan` (x2) / `dock::strip_rect` / `orin_glass_probe`.
  **Those two `bl` edges are also the append-after-comment negative control** — both call sites are
  in-line appends before a line's trailing `//`, and a statement that had fallen into comment text
  would compile vacuously with the feature banner unchanged and no `bl` in the caller.

#### What the metal flight must watch for — RUNG (a), both outcomes

Image: `UNAOS_ORINLADDER=1` **alone** (it implies the conjunction). Rides **boot7j**. Expected wire at
the terminus, in order — the `[orinconwin]` trio is boot7h's, unchanged, and is the precondition:

```
[orinconwin] gate panel=1920x1200x4 stage=4194304 table=1 dock=GRANTED route=UNROUTED orindesk=1 orinclick=1 rows=12
[wc-x] console-window win=2 panel=1920x1200 … cell=7x16 …
[orinconwin] win=2 panel=1920x1200 cell=7x16 stage=4194304 table=2 present=Composited route=true live=LIVE -> ROUTED
[oringlass] probe=kl_top  at (…) got=0x… want=0x… -> MATCH          ← six of these
[oringlass] phase=arm win=2 box=…x… at (…,…) content=…x… at (…,…) scale=1 onpanel=yes frame=6/6 samples=1024 read=1024 paper=… ink=… stem=… blend=… blevels=2+ off=… ink1=0x00c0c0c0 n1=… ink2=0x00…… n2=… ink3=0x00…… n3=… inkvals=… exact=yes first=0x00000000 uniform=no -> GLYPHS-ON-GLASS
```

**Rung (a) is CLOSED iff that last line reads `-> GLYPHS-ON-GLASS` with `frame=6/6`, `paper>0` and
`stem>0`, OR `-> GLYPHS-AA-ON-GLASS` with `frame=6/6`, `paper>0` and a blend supermajority.** The
second is the anti-aliased-evidence close ORIN-GLASSINK added; `stem=0` on its own is no longer a
failure, because the face is anti-aliased and a 1024-sample grid can legitimately miss every fully
covered pixel.

**The shapes `INK-NO-STEM` used to hide, and how to tell them apart from the wire alone.** All of
them print `stem=0`; the ink fields are what separate them, and only one of them is healthy:

| what prints | reading, and what to do |
| --- | --- |
| `blend=23 blevels=2+ off=0 ink1=0x00…… inkvals≤6 exact=yes -> GLYPHS-AA-ON-GLASS` — the ink is a supermajority of PAPER→INK blends at two or more levels, `ink1` is a grey between `0x00000000` and `0x00c0c0c0`, `n1+n2+n3 ≈ ink` | **HEALTHY. Rung (a) is CLOSED.** The glyphs are on the glass and the sample grid landed only on anti-aliased edges. boot7j's `paper=1001 ink=23 stem=0` is expected to resolve to exactly this shape; if it does, the old `INK-NO-STEM` reading was a false conviction and nothing is wrong with the panel |
| `blend=… blevels=1 ink1=0x001e1e1e -> INK-FLAT-FILL` — one single ramp colour fills the ink | **the desktop is showing through, or a flat fill is over the content.** `ink1=0x001e1e1e` is `video::PANEL_BG` by name: the window's content did not paint over the panel background. Any other single grey is some other flat fill. Cross with `frame=`: `6/6` says the window's own chrome IS on the glass and only the content is missing, which localises to the content flush exactly as `WIN2-NOT-ON-GLASS` with `frame=6/6` does. Report |
| `blend=0 off=23 ink1=0x00…… n1≥12 exact=yes -> INK-OFF-COLOUR` — one value holds a majority of the ink and is NOT on the ramp | **the constant, or the face, is wrong — and the wire now names which value.** Read `ink1=`: that is the colour the console is actually painting. Cross with `[wc-x] console-window`'s cell and with `fbcon.rs`'s `FG_DEFAULT`. **Do NOT edit `LAD_INK` from the bench**: report the measured value, because a constant tuned to the board proves nothing. If `ink1=0x00ffffff` the face is being armed white; if it is a hue, something else is painting into the box |
| `blend=… off=… exact=no` (or `exact=yes` with no dominant value) `-> INK-NO-STEM` | **the original reading, and now the only one it can carry.** Scattered unrelated values inside the content box: a foreign surface is over the content. `inkvals=` at the six-slot ceiling with `exact=no` is the signature. Cross with `frame=`: `6/6` says the window's own chrome is intact and something is painting INSIDE it. Report |

Every other shape the line can take, and what each one means:

| what prints instead | reading, and what to do |
| --- | --- |
| `-> BLANK-NO-GLYPHS` (`paper=1024 ink=0 stem=0 uniform=yes first=0x00000000`) | the window's surface reached the glass and its TEXT did not. `present=Composited` was true and no glyph is in the scanout — the glyph route painted into a surface the flush did not carry, or painted nowhere. **This is the failure §3.9.1 could not rule out, and the whole reason the rung exists.** Report it; do not "fix" it from the bench |
| `-> WIN2-NOT-ON-GLASS` (`paper=0`) | not one sample is the console's background. Cross with `frame=`: `frame=0/6` = the whole window is absent from the scanout; `frame=6/6` = the FRAME landed and the CONTENT did not, which localises to the content flush |
| `-> GLYPHS-NO-CHROME` (`stem>0`, `frame<6/6`) | text on the glass, frame overdrawn. This is §3.8.1's measured JD2 console-blit overdraw, and it is the first time an instrument has convicted it. Rung (a)'s own question (did the glyphs land) is ANSWERED YES; the frame damage is rung 4's problem |
| `-> INK-NO-STEM` / `-> INK-OFF-COLOUR` / `-> INK-FLAT-FILL` / `-> GLYPHS-AA-*` | the `stem=0` shapes — see the table above, which is the only place the ink fields are read |
| `-> GLYPHS-AA-NO-CHROME` (`stem=0`, blend supermajority, `frame<6/6`) | anti-aliased text on the glass with the frame overdrawn. Rung (a)'s own question is ANSWERED YES; the frame damage is rung 4's problem, exactly as for `GLYPHS-NO-CHROME` |
| a line where `paper + ink != read`, or `stem + blend + off != ink` | **the instrument, not the panel.** The two identities hold by construction (one classification per sample, every counter derived from it), so a line that does not balance is a defect in `orin_glass_probe`. STOP and report |
| `-> UNREADABLE` (`read=0`, six `UNMAPPED` probe lines) | the row's geometry and the panel's mapped length disagree. A real defect, invisible to any present count. STOP and report |
| `-> DECLINE reason=no-console-row` | no console window. Read `[orinconwin] DECLINE reason=…` on the line above it — ordering-rule means **wrong media**, STOP |
| no `[oringlass]` line at all | the terminus never reached `orin_ladder_arm`. Check the `⚡ kernel features:` banner for `orinladder`; if it is there, the boot died before the terminus and that is the finding |

#### What the metal flight must watch for — RUNG (b), both outcomes

The arm line tells the operator exactly where to click, because a flight that has to guess at a 24-px
disc on a 1920x1200 panel reports "nothing happened" when the truth was "you missed":

```
[orindock] arm panel=1920x1200 win=2 disc=(X,Y,D) strip=(x,y,WxH) tiles=N glass=GLYPHS-ON-GLASS orinconwin=1 orinclick=1 orindesk=1 pidesk=1 -> ARMED
```

**Click 1 — the minimise disc, at `(X + D/2, Y + D/2)`.** Expected:

```
[orinclick] edge=press btn=0x01 at (…,…) geom=yes hit=yes win=2 owner=0xffffff01 focus …->… consumed=1 -> CONSUMED
[wm-act] minimise win=2 owner=4294967041 at (…,…) -> settle=…
[orindock] park win=2 z=0 shellz=… tiles=… tiled=1 t=… -> PARKED
```

**Click 2 — a tile in the dock strip, inside the `strip=` rect.** Expected:

```
[dock] press at (…,…) tile=t/n win=2 owner=0xffffff01 was_hidden=true -> raised=true unhid=true
[oringlass] phase=restore … -> GLYPHS-ON-GLASS
[orindock] restore win=2 z=… shellz=… via=dock dockpress=raise parked=…t glass=GLYPHS-ON-GLASS t=… -> RESTORED
[orindock] census seq=… vis=panel tiles=… tiled=1 parks=1 restores=1 viadock=1 painted=1 blank=0 glass=… probes=… -> DOCK-ROUNDTRIP
```

**Rung (b) is CLOSED iff one `park -> PARKED` is followed by one `restore … via=dock … -> RESTORED`
and the census settles on `-> DOCK-ROUNDTRIP`.** Every broken shape, and what each one means:

| what prints instead | reading, and what to do |
| --- | --- |
| census `parks=0 … -> IDLE-NEVER-PARKED` | the disc was never pressed. Read the `[orinclick] edge=press` line for that coordinate: `hit=no` = the press missed the window entirely (re-read `disc=` off the arm line); `hit=yes -> CONSUMED` with no `[wm-act] minimise` = it landed on the window but on a DIFFERENT control — `[clickroute] close=` or `[wm-act] zoom` will name which |
| `park … tiled=0 … -> PARKED-NO-WAY-BACK`, census `-> FAIL reason=park-no-tile` | **the one-way trip §6.1 exists to forbid, realised on metal.** The row parked and the dock's tile model does not contain it. STOP and report: this convicts `dock_scan`'s enumeration or the pin arithmetic, and it means the console window on this image is a trap |
| census `-> PARKED-AWAITING-DOCK`, persisting | the park half is done and the tile has not been pressed. Not a failure — press it. If a tile press produces NO `[dock] press` line at all, the strip is not consuming the point: compare the coordinate against `strip=` on the arm line, and note `dock::Layout::contains` DECLINES the strip's cut CORNERS by design, so aim at a tile centre |
| `[dock] press at (…) -> strip tiles=N raised=none` | the press hit the dock's own BACKGROUND rather than a tile. Consumed, raises nothing, by design. Aim at a tile |
| `restore … via=other … -> RESTORED-OFF-DOCK`, census `-> RESTORED-NOT-VIA-DOCK` | the window came back, but not through a dock tile press (a `<TAB>`, or a focus change). **Rung (b) is NOT closed** and the census refuses to credit it. Re-park and use the tile |
| `restore … glass=BLANK-NO-GLYPHS … -> RESTORED-BLANK`, census `-> FAIL reason=restore-blank` | **"a restore that paints nothing".** The raise moved the row above the shell and no glyph reached the glass. Cross with the `frame=` field on the `[oringlass] phase=restore` line: `frame=6/6` = the chrome repainted and the content did not (a damage/present defect in the restore path); `frame=0/6` = the composite never ran for this row. Either way, report; this is a real defect and the instrument is doing its job |
| census `-> DECLINE reason=no-console-row` AFTER a park | the window was CLOSED, not minimised — check for `[clickroute] close=win2 … settle=` (boot7h showed the close disc `REFUSED furniture`, so this should not happen) |
| no `[orindock]` lines at all after `arm` | the JD2 pump's phase-2 drain loop is dead — the same liveness reading `[orinclick] census` carries. A pump failure, not a rung failure |

Watch also, as rung 6's card says: the `[redzone]` guard (boot7h already absorbed a LOW-REDZONE on
`jd2-console`; the read-back adds ~1030 `read_pixel` calls to the arm and to each restore, and §5's
stack numbers are still Pi numbers), and the `[dock]` ledger line's own `presses= raises= unhides=`
tail — after a successful round trip those must read `presses>=1 raises>=1 unhides>=1`, which is the
independent confirmation of `[orindock]`'s verdict from the dock's OWN counters.

#### What this rung deliberately did NOT do

* **No `video/` edit of any kind** — see the accessor table above. `wm.rs`, `dock.rs`, `strip.rs` and
  `fbcon.rs` are textually untouched, so the `hw-rmbp` sync meets no conflict from this arc.
* **No behaviour.** No new control, no new routing arm, no pixel path. If the round trip does not work
  on metal, this rung did not break it and cannot fix it — it can only say so.
* **No rung 5.** No furniture arming, no `desktop_firmware::activate()`, no tegra `render_service`; §5.2 is
  untouched and `TEGRADESK_CASCADE_OK` is not read.
* **No timed verdict.** `FAIL reason=park-no-tile` is structural (the tile model, asked at the moment
  of the park). There is deliberately no "the operator took too long" arm: a slow hand must never
  read as a broken dock.
* **UNFLOWN.** Every claim above is a build-time or artifact measurement. The two flight cards are the
  adjudicators.

---

### §3.12 LANDED 2026-08-26 — the FURNITURE: the menu bar is enabled and painted. `orinfurn`, DEFAULT OFF, UNFLOWN

**The complaint that opened this arc:** the Orin has no furniture at all. No menu
bar, no crystal, and every click lands on empty desktop. The brief that carried it
diagnosed the cause as "nothing calls `pidesk_activate_maybe` from `tegra_early_stop`".
**That diagnosis is stale and the correction matters more than the fix**, so it is
recorded first.

#### What was actually wrong

`tegra_desk_arm` — rung 2's DESKSEAM (§3.2.1) — *already* calls `desktop_firmware::activate()`
from `tegra_early_stop`'s terminus line, and has since 2026-08-25. It is wired, it is
reached, and it **refuses**, because `TEGRADESK_CASCADE_OK` is a literal `false` in
source and §5.2 says it stays that way until someone can show this board's own
`[u7stk]`/`[redzone]` numbers for the cascade. So the missing bar was never a missing
wire. It was a stop-line doing its job, and the operator could not tell the two apart
from the panel.

Beneath that, the narrower fact: `menubar::ENABLED` starts `false`, and this tree has
exactly **two** `menubar::set_enabled(true)` calls — `video/desktop_uefi.rs:552` (x86_64-only)
and `video/desktop_firmware.rs:292` (inside `activate()`). Neither is reachable on tegra. The
bar was compiled, composed on every pass, and permanently invisible.

#### ⚠ A finding about the stop-line itself

§5.2 asks for `[u7stk]`/`[redzone]` numbers before the cascade may be armed.
**At the terminus, that requirement is structurally unsatisfiable.** `sched::stk_probe`
loads `SCHED[cpu].current` and returns early when it is null — which is exactly the
boot core before `run_capstone_boot_core` drives the queue, i.e. every rung on this
line. The stop-line therefore gates the cascade on evidence that cannot be taken where
the cascade would run. That is not a licence to step over it; it is a note that
clearing §5.2 needs an instrument this ladder does not yet have (a boot-stack
high-water probe, or the cascade moved off the boot stack), and that no rung should
claim to have cleared it by argument.

##### ⚠ AMENDED 2026-08-28 — the DEPTH half is now taken; only HEADROOM is still blocked

The paragraph above is correct about `stk_probe` and **overstated about the machine**, and
the distinction decides what §5.2 can still legitimately ask for.

`stk_probe`'s early return is real and quoted correctly — `arch/aarch64/sched.rs:96-99`
returns when `SCHED[cpu].current` is null, and everything it reports (`base`, `len`) is
derived from `task.stack`, which does not exist on the boot core. It is not adaptable to
this seam.

But the SP itself is readable in three instructions with no scheduler, no `Task` and no
linker symbol, and the exact pattern **already exists in this tree twice** —
`arch/aarch64/mmu_tegra.rs:749-752` and `arch/aarch64/sched.rs:91-94`. Two reads on the
same frame chain subtract to an exact **depth consumed**. `TERMINUS` D4 takes that
measurement at the `[orinfurn] arm` line and publishes `[orinstkdepth] depth-consumed=`. The
instrument itself — wire format, both failure arms, and the full argument for why no headroom number
is derivable — is documented in
[`docs/dev/OS/01_BOOT_HAL/arch_arm64.md`](../01_BOOT_HAL/arch_arm64.md) §ORIN-STKDEPTH; §3.13 D4
carries the ladder-side summary.

**What is still genuinely unavailable is HEADROOM, and the reason is not the scheduler.**
The Orin's boot stack is the one UEFI handed the loader and is never switched:
`crates/bootloader/src/main.rs:1045` calls `kernel_entry` through a transmuted pointer with
no SP manipulation, `arch/aarch64/mmu_tegra.rs:789` states "the boot stack is never switched
on this path", and `arch/aarch64/boot_tegra.rs:186-189` sets `SP_EL1 = SP` so the stack is
continuous across the EL2→EL1 drop. There is no linker script for the `aarch64-unaos.json`
target, so no `__stack_top` is linked into the jetson image. (The symbol *does* appear in
Rust source, at `main.rs:51-52` inside the `global_asm!` — but that block is
`all(aarch64, baremetal)`, i.e. the Pi, and no tegra leg carries `baremetal`.) The
`MemoryRegion` slice that *would* bound it survives `exit_boot_services` and is passed
through, then discarded: `memory::init(boot_info)` at `main.rs:2311` consumes the
`&'static mut BootInfo` and `arch/aarch64/memory.rs` stashes nothing.

So the instrument prints `depth-consumed=` and, for the other half, `DEPTH-UNAVAILABLE`
rather than inventing a number. **A depth that is honest about not being headroom is worth
more than a headroom figure derived from an assumed bound**, and §5.2's clearing condition
should be restated in those terms rather than left asking for something no rung can supply.
Closing the headroom half needs one of: a linker script for the aarch64 kernel target, or
the `MemoryRegion` slice retained past `memory::init`, or the cascade moved off the boot
stack. All three are real arcs; none is a comment change.

#### What landed instead

`orinfurn` — a knob that takes **two** of `activate`'s nine steps and nothing else:

```
menubar::set_enabled(true);
wm::composite();            // then read menubar::owns_pixels() back
```

`main.rs::tegra_desk_furn`, appended to `tegra_early_stop`'s terminus line **after**
`orin_ladder_arm` (so every earlier rung's probe still reads a bar-free panel and its
captures stay comparable to boot7f/7g/7h) and **before** `boot_ok_disarm` (so the
`orinwdt` boot watchdog covers the composite it drives).

Floors, each read live and each with its own named refusal: the panel
(`WRITER::is_ready`), the staging buffer (`wm::reserve_stage` — the F1-F5 masked-heap
argument DESKSEAM states), and the bar's own geometry floor (`menubar::strip_rect`,
`None` below `FLOOR_W`/`FLOOR_H`). The paint is **read back** through
`menubar::owns_pixels()`, never inferred from having called `composite`.

**One deliberate divergence from `pidesk`, flagged for the bench:** on a declined
composite this seam **rolls `ENABLED` back**, where `desktop_firmware::activate` reports the miss
and leaves the bar on. On this board an enabled-but-unpainted bar is a permanent dead
band across the top of the JD2 console — `Screen::present_background` subtracts its
rect on every present and no damage condition can notice. Only a bench boot can say
which behaviour is right here.

#### Why §5.2 is NOT crossed

* `desktop_firmware::activate()` is not called; `TEGRADESK_CASCADE_OK` is not read or written.
  Rung 5 remains blocked and DESKSEAM still prints `REFUSE reason=stop-line-5.2`.
* `orinfurn` does not imply `quarry`, so `quarry::open()` — Pi boot 11's actual 16 KiB
  overflow, at click-router depth — is the `not(feature = "quarry")` stub. Same fact
  rung 4 leaned on.
* Not taken, each for its own reason: the step-1b DESKTOP-CLEAR (whole-panel front-buffer
  write whose soundness argument is an empty window table), the console window (rung 4's),
  `crystal::routed_selftest` (a `witness` fixture that drives presses through the live
  router — the deep arm), `pulsewin::open`, window population, the tegra `render_service`,
  the closing filesystem walk.
* What it adds to the terminus is **one more `wm::composite()`** — the call rungs 0, 4 and
  6 already make from this same line on the boot core's entry frame, all three FLOWN on
  Orin metal. ⚠ **That is an argument from the ledger, not a measurement.** See the
  stop-line finding above: no `[u7stk]` number for this line exists or can be taken.

#### Implications and knob

`orinfurn = ["pidesk", "orinclick"]`; `orinclick` implies `tegra_el0` implies `tegra`, so
the knob is self-sufficient. It **implies `orinclick`** because a bar whose crystal cannot
be pressed is chrome, not furniture. It deliberately does **not** imply `orinconwin` or
`orindesk`. Intended bench image:

```
UNAOS_ORINFURN=1 UNAOS_ORINCONWIN=1 UNAOS_ORINDESK=1 ./arroyo esp-jetson
```

#### The second edit: DEAD-STUB

`pidesk_activate_maybe`'s knob-off twin was gated
`all(aarch64, not(all(pidesk, baremetal)))`, which compiled a constant-`false` stub on
every non-baremetal aarch64 build — including every tegra one, where its only call site
(`kernel_main`'s GUI-handoff line, itself `all(aarch64, baremetal)`) does not exist, and
where `kernel_main` is unreachable anyway. Hence `pidesk_activate_maybe is never used` in
every tegra gate log for weeks. Narrowed to `all(aarch64, baremetal, not(pidesk))` so the
pair exists exactly where its caller does. Behaviourally inert on `baremetal`; line-neutral
(one attribute rewritten in place, `main.rs` still 7276 lines before the tail block).

#### Gate

`arm-tegra-furn` added to `KERNEL_CFG_MATRIX` — `arm-tegra-conwin`'s list plus `orinfurn`,
because the bar's caption field is the focused window's and a leg with no window path would
type-check a configuration nobody boots. It carries `quarry` on purpose: `orinfurn`'s §5.2
argument rests on `quarry::open()` being stubbed, so compiling the bar *with* `quarry` on is
the adversarial half — a future edit that reached `quarry` from the bar path goes red here
rather than on a bench boot.

#### What this rung deliberately did NOT do

* **No `video/` edit of any kind.** `menubar.rs`, `strip.rs`, `crystal.rs`, `dock.rs`,
  `wm.rs` and `desktop_firmware.rs` are textually untouched; every symbol is consumed through its
  existing public accessor.
* **No rung 5.** No `desktop_firmware::activate()`, no DESKTOP-CLEAR, no tegra `render_service`,
  `TEGRADESK_CASCADE_OK` untouched.
* **UNFLOWN, and this is the whole of what is owed.** ~~No Orin has booted an `orinfurn`
  image.~~ **Superseded 2026-08-28: two have — desk1 and desk2, both at `f3df7ff` — and
  neither reached the seam. The rung is still UNFLOWN in the sense that matters (no verdict
  on the bar exists), but the reason is now measured rather than absent. See §3.12.1.**
  Unproven on metal: that the bar paints at all; that it paints at 1920x1200 rather
  than declining on a contended `SCRATCH`; that the crystal press is consumed by the menu
  band rather than falling through to the desktop; that the composite fits the boot stack.
  The falsifiers are `[orinfurn] ARMED … -> BAR-ON-GLASS` with a non-`None` `rect=`, and an
  `[orinclick] edge=press … at (x,y)` inside the printed corner rect that does **not** end
  `-> RAISED` or `MISS-SHELL`.

#### §3.12.1 CORRECTION 2026-08-28 — the flight record, and the conviction it does not support

Five `f3df7ff` metal flights were taken on 2026-08-26 (`~/unaos-bench/capture/line-acm0`,
offsets into `raw.log` from `marks.txt`). Two parked during secondary bring-up; three ran clean:

| leg | `raw.log` offset | knobs | outcome |
| --- | --- | --- | --- |
| desk1 | +7621395 | FURN+FACE+INPUT+EL1AP+LOCKFIX+RAST, conwin OFF | **park** |
| desk2 | +7689131 | noel1ap FURN+FACE+INPUT+TENANT+RAST | **park** |
| desk3 | +7756839 | faceonly (`orinfurn`/`orininput` OFF) | clean, 5/5 secondaries online |
| desk4 | +7842003 | +`orininput` (bisect) | clean, 5/5 secondaries online |
| desk5 | +7920173 | all-but-furn +`orindesk` | clean, 5/5 secondaries online |

The record carried forward from that sitting read: *"`orinfurn` faults trying to composite at
the terminus — an EL3 RAS Uncorrectable right after core 5. It is painting furniture into a
panel whose ownership is undefined at that instant, off the boot core's entry frame, with no
stack number obtainable there."* It was marked `[MEASURED]`, reached
`~/.claude/plans/unaos/batons/orin-9.md:85-87`, and became a phase item there (`:101-103`,
"THEN re-arm `orinfurn`"). **Three of its four clauses are refuted below.** They are struck
rather than deleted so the next reader can see both what was claimed and why it did not hold.

##### ~~"`orinfurn` faults trying to composite at the terminus"~~ — the seam was never entered

`tegra_desk_furn`'s first statement is an unconditional `serial_println!` of
`[orinfurn] arm click=… conwin=… desk=…` (`main.rs:7879-7886`). It precedes the one-shot
`ORINFURN_ENTERED` latch (`:7889`) and every refusal path in the function, and its own comment
gives the reason it exists: "a silent `false` must never be indistinguishable from 'the seam
was never called'".

The token `orinfurn` appears **zero** times in both `raw.log` and `orin.log`
(`grep -ac orinfurn` returns 0 on each). The grep is live on those same files:
`grep -ac 'tegra:'` returns 4138 on both. The only occurrence anywhere in the capture set is
`marks.txt:12`, which is the operator's own label for the desk3 leg.

The seam did not run. It cannot have faulted, declined, or composited.

##### ~~"an EL3 RAS Uncorrectable right after core 5"~~ — after the last *enumeration line*, 96 source lines earlier

"Right after core 5" is positional, not causal. `start_secondaries_tegra` dumps every
enumerated core **before** issuing any PSCI call, deliberately:
`arch/aarch64/smp_virt.rs:865-873` — "so the metal capture has the full set even if a later
`CPU_ON` faults (the JM5 attempt-1 lesson: a RAS fault ate the enumeration)". `enumerated
core 5` is therefore the last line printed before the *first* `CPU_ON` SMC. Both parked legs
die there with **no `CPU_ON` line at all**; all three clean legs print
`CPU_ON AP 1..5 -> SUCCESS` followed by `5/5 secondaries online`.

The seam that faults is `main.rs:2621`, `smp_virt::start_secondaries_tegra(…)` under
`#[cfg(feature = "tegrasmp")]` at `:2620`. The terminus line carrying `tegra_desk_furn()` is
`main.rs:2717` — **96 source lines later**, and reached on neither parked leg.

Note that `main.rs:2717` is a single source line carrying the whole tail wire-in set *and* the
terminus call, by the knob-off byte-identity convention. A panic `Location` cannot discriminate
among the seams on it. That is precisely why the entry line of §3.12.2 was the only instrument
that could answer this question.

##### The base-rate arithmetic, stated because it is the whole of the argument

`arch_arm64.md` §ORIN-SMP-3-PARK (`:6151`) measures the park at **~30% across the record** and
warns at `:6163` that at that rate "a single armed boot has a ~70% chance of returning a *clean*
trace that convicts nothing."

Against a 30% independent base rate over 5 boots:

* expected parks = 5 × 0.30 = **1.5**; observed 2.
* P(exactly 2 parks in 5) = C(5,2) × 0.30² × 0.70³ = 10 × 0.09 × 0.343 = **0.309**. The
  observed split is the single most likely outcome under the null.

The strongest form of the original argument is not the 2-of-5 split but the pairing: exactly two
legs armed `orinfurn`, and those two are the two that parked. Under the null that is
1 / C(5,2) = **0.10**. One chance in ten is not a conviction, and this was **not a bisect**: no
two legs in the set differ in `orinfurn` alone. desk1 and desk2 differ from each other in
`EL1AP`+`LOCKFIX` against `TENANT`, and the nearest FURN-off leg (desk4) differs from desk1 in
three knobs. The set was never constructed to isolate the variable it was read as isolating.

That 0.10 is moot in any case, because the entry line closes the only channel it could have acted
through. A feature cannot act at runtime on a boot that never called it, so the sole remaining
channel is the *image* — a build-level layout effect. **The layout axis is already closed:**
`arch_arm64.md:6187-6190` records boot5c flights #1 (park) and #2 (clean) sharing
`VBAR_EL2 = 0x25b115800` and `TTBR0 = 0x25b26a000` — "same binary, same physical load base,
minutes apart, opposite outcomes."

##### desk2 is the documented intermittent; desk1 is a third variant

desk2's signature is `Exception reason=1 syndrome=0x82000010`, IOB `Status = 0xe4000612`,
`ADDR = 0x8000000000000200`. That is **ORIN-SMP-3-PARK verbatim** — the pre-existing
intermittent, spec'd as a `FORBID` at `unaos/scripts/specs/jetson-sync1.spec:1429` and described
at `:1418-1420` as byte-identical across four metal instances dated 2026-07-15 and 2026-07-17 and
boots 5b and 5c. **All of those predate `orinfurn`, which landed 2026-08-26.**

desk1's signature is neither the park nor the `bg`-verb fault. It is recorded as a new variant in
`arch_arm64.md` §ORIN-SMP-3-PARK.

##### ~~"a panel whose ownership is undefined at that instant"~~ — unsupported by the code

At the terminus every task alive on the tegra path is spawned onto cpu 0 explicitly, and an
explicit core is a pin:

* `jd2-console` — `main.rs:2495`, `cpu` argument `0`.
* `el0-hello` — `main.rs:7022`, `spawn_user(…, 0)`, whose comment reads "Pinned to the boot core
  (cpu 0, not `CPU_AUTO`): `pick_cpu_slot` short-circuits a non-AUTO request, so the EL0 task
  cannot be placed on a secondary."
* `tegra-el0-verdict` — `main.rs:7023-7028`, `cpu` argument `0`.

`arch/aarch64/sched.rs:3658` sets `steal_ok: requested_cpu == CPU_AUTO` beneath the comment "A
task spawned onto an explicit core is pinned there (no-migrate), so stealing never touches it";
`:340` states "Tasks never migrate." `sched::spawn` appears exactly **once** inside
`tegra_early_stop` (`main.rs:2029-2718`), at `:2495`.

No second core can enter `wm::composite()`, `Screen::present_*` or `menubar::*` at that instant,
because no task exists on a second core to do so. **The clause is withdrawn.** This says nothing
about the separate and real ownership problem the same sitting recorded — `JD4`'s timed
`RAST-SUPERSEDED-BY-CONSOLE` seizure of the panel — which is a *sequencing* conflict on one core,
not a concurrency one, and is unaffected by this correction.

##### What is unchanged

The fourth clause, "with no stack number obtainable there", is **true and stands**:
`sched::stk_probe` returns early on the boot core before `run_capstone_boot_core` drives the
queue, exactly as the stop-line finding above records. What does not follow from it is that the
park is a stack fault. It is an instrument gap, not evidence for a mechanism. `orinfurn` remains
UNFLOWN, DEFAULT OFF, and every falsifier listed above is still owed.

#### §3.12.2 METHOD — entry lines and read-back fields, as a rule for future seams

Two instruments in this subsystem were exercised on 2026-08-28, one in each direction. The pair
is worth stating as a rule, because this ladder will keep adding seams shaped like both.

**An entry line printed before any decision distinguishes "never ran" from "ran and refused."**
`tegra_desk_furn` prints `[orinfurn] arm …` unconditionally as its first statement, ahead of the
one-shot latch and every `return false` (`main.rs:7879-7886`). That one line is what turned
§3.12.1's question from unanswerable into arithmetic: a seam that prints nothing was not called,
and no reasoning about the composite was required. The cost of not having such a line is on the
record — the ambiguity it closes is exactly what produced a wrong conviction that propagated
into a session plan as a phase item.

**A status field must be read back, not asserted.** The inverse failure sits in the same
subsystem. `display_tegra.rs:2677` prints `live={}` for the console-window route from a
compile-time literal; the comment immediately below it says the value is "read from the BUILD,
not from this function". Meanwhile `main.rs:6973` calls `fbcon::detach()` unguarded on the RAST
demo path, which runs *after* `orin_conwin()` on the same terminus line (`main.rs:2717`), so a
build carrying both knobs tears the route down immediately after installing it while the
instrument still reports it live. That is a false PASS on the wire. ~~A sibling arc owns the fix;
it is named here for the rule, not claimed as repaired.~~ **UPDATED 2026-08-28: both halves LANDED
the same day — see §3.13 D1.** `live=` is now a read-back of `fbcon::console_is_routed()` at print
time (`display_tegra.rs:2697`) and the RAST-path detach takes the phase-2 guard
(`main.rs:6973`). The rule stated above is unchanged by the repair, and §3.13.1 (PANELOWN) is the
same rule demonstrated in the other direction: an instrument published rather than asserted refuted
one of its own author's states within an hour of existing.

The general form: **an instrument reports either what the author intended or what the machine
did, and only the second is evidence.** `orinfurn`'s own `owns_pixels()` read-back — §3.12,
"never inferred from having called `composite`" — is already the correct shape. Apply both halves
to every new seam on this ladder: print on entry, before any branch; and read every reported
state back out of the subsystem that owns it.

### §3.13 LANDED 2026-08-28 — the TERMINUS fold plus PANELOWN: five instruments that reported the state their author intended. UNFLOWN

Fold `405b21f6` (`d33b6c3e`, `e1bb9b49`, `a4b1b338`, `81f304d2`) and fold `63b86488` (`52137ffc`).
Four defects on the tegra terminus and one unenforced invariant in `video/`, all found by
measurement rather than by review. Nothing below has runtime evidence: **no Orin boot has flown any
of it.** Every file:line citation below was re-verified against the tree at `31129232`.

#### ⚠ NO CHECK LEG IN THIS TREE CAN SCORE TEGRA BEHAVIOUR — proven by doing it, not argued

Read this before the five instruments, because it bounds what every gate result below is worth.

D1(b) and D2 are both behaviour changes in a **type-identical** program. The defects were
deliberately re-introduced and the gate re-run: `arm-tegra-furn` and `arm-tegra-supstate` **both
returned exit 0**. That is not a gap in the fold's diligence; it is the structure of the suite:

* every leg of `KERNEL_CFG_MATRIX` is `cargo +nightly check --release --target … --features …`
  (`unaos/arroyo:3482`) — a type-check that neither links nor runs anything. `arm-tegra-furn` is
  `arroyo:3226`.
* **no QEMU regression in this tree compiles `tegra` at all.** `./arroyo test` is x86; `./arroyo
  test-arm` is the plain `virt` image, and `tegra` is forced on only by the `esp-jetson` media path
  (`arroyo:4046-4056`). QEMU models no Tegra234, so there is nothing to boot.

The fold's fallback was an artifact proof — `llvm-objdump` showing `bl console_is_routed` + `tbnz`
ahead of `bl detach` where the defect had an unconditional call, and `orin_rast_console_owns` absent
from the supstate symbol table before the fix and called after it. **That is the right evidence for
a commit and it does not carry forward.** A disassembly proves one build at one sha; it re-runs on
nobody's next edit.

**The mitigation, and what it costs.** SPECGATE (fold `0ea79938`, landed 2026-08-28) makes the four
TERMINUS instruments scorable against a *metal capture*, which is the only place a tegra behavioural
regression can be caught in this tree. `unaos/scripts/specs/jetson-sync1.spec` gains **five failable
rules, three PENDING arming lines and six OPTIONAL readouts**; `jetson-jd5.spec` is deliberately
untouched (a frozen 43-line historical spec validated against one 2026-07-10 capture).

Every failable row is a **FORBID on a failure literal**, which is the one shape that is
unconditionally safe here: three of the four families are knob-gated and default OFF, so the token
exists only in an image that compiled the instrument. An unarmed boot cannot trip it, and its
silence is never read as a pass — each FORBID is paired with a PENDING arming row so "did not fire"
is distinguishable from "was not armed".

| row | fails when |
| --- | --- |
| `FORBID \[orinconwin\] win=.* live=FROZEN` (`jetson-sync1.spec:1449`) | an `orinconwin` boot reaches the terminus with the route not installed at print time |
| `FORBID console OWNS the panel \(Screen back buffer live\); first key` and `…; screen-on-boot` (`:1490-1491`) | a takeover line carries **no** `path=` token — a stale pre-fold image was flown, or the discriminator was reverted. These key on the PRE-FOLD byte sequence, in which `live);` is followed directly by ` first key` / ` screen-on-boot`; the post-fold line interposes `; path=jd2-…;` and no longer matches |
| `OPTIONAL path=jd2-console-pump;` / `path=jd2-supstate-phase2;` (`:1512-1513`) | readouts, not rules — they say WHICH site printed. The conditional arm ("on a supstate image, forbid `path=jd2-console-pump`") is the grammar limit below |
| `FORBID \[orinstkdepth\] DEPTH-UNAVAILABLE` (`:1572`) | an `orinfurn` boot reaches the furniture seam with no derivable depth |
| `FORBID \[orinrast\] census .* -> RAST-PAINTED-OVERWRITTEN` (`:1601`) | a `rast` boot finds the cube painted at `post`, gone at `late`, latch not set. Not a new string — what changed is that a supstate boot can now reach the correct arm |

**The takeover pair is the only rule in the set whose reachability is guaranteed by the spec
itself**: it sits on the line the existing `REQUIRE JD4.*console OWNS the panel` already demands of
every scored flight, so its silence can never be dismissed as an undriven path.

`live=LIVE` was deliberately **not** keyed on: boot7h's `live=LIVE` is the old compile-time literal,
so such a row would be satisfied by every pre-fold capture and prove nothing. `FROZEN` is the half
that could not print before the fold.

⚠ **The green reference moved, and this is the largest such change the spec file has taken.**
boot7f, boot7g and boot7h all now replay **FAIL** — the delta is exactly the 13 pre-fold takeover
lines and nothing else, so the reds are for the right reason. **There is no green reference for this
ladder until a post-`405b21f6` image flies.** Do not weaken the rows to recover a green.

⚠ **Two limits that remain.** First, a grammar limit named in the spec itself: only
`REQUIRE`/`COUNT`/`FORBID` can fail, and the takeover-path defect is inherently *conditional* — on a
supstate image `path=jd2-console-pump` is a defect, on a default image it is correct. `mbench` has no
`WHEN <guard> FORBID <rx>`, so that arm is a reading table rather than a rule. Second, **PANELOWN
(§3.13.1) has no spec row at all**; SPECGATE covers the four TERMINUS families only. And none of
this changes the finding above: a spec scores a *capture*, and no capture exists.

#### D1 — the console window was detached the instant it was installed, and `live=` could not see it

**The behaviour.** The terminus line (`main.rs:2717`) runs `orin_conwin()` **before**
`tegra_rast_demo_maybe()`. The phase-2 detach in `jd2_console_pump` has been guarded since rung 4
(`main.rs:2873`, `if !tegra_conwin_live() { … }`), but the twin inside `tegra_rast_demo_maybe`
(`main.rs:6964`) was **bare**. So on every image carrying both knobs, rung 4 installed the
console-window route, printed `live=LIVE`, and then reached the unguarded `fbcon::detach()` two
statements later: `GUI_ACTIVE` set, `fbcon::_print` returning at its first test, and the "LIVE"
console window receiving no further glyph for the rest of the boot. **All five legs that carry
`orinconwin` also carry `rast`** — `arm-tegra-conwin`, `arm-tegra-conwin-tenant`,
`arm-tegra-ladder`, `arm-tegra-furn`, `arm-tegra-fbconpar-cross` (verified against
`KERNEL_CFG_MATRIX` at `63b86488`).

⚠ **Scope it exactly, because a metal reader will ask.** Without `rast` the whole helper is the
`#[inline(always)]` empty stub at `main.rs:6983`, so the defect never existed on a knob-off or
`rast`-off image. **No flight taken so far was exposed** — boot7h armed
`UNAOS_ORINCONWIN=1 UNAOS_ORINDESK=1 UNAOS_ORINCLICK=1 UNAOS_NET4=1` and no `UNAOS_RAST`. What was
exposed is every `orinconwin` check leg, and any future flight that arms both knobs. ⚠ Note that
§3.12's stated bench image (`UNAOS_ORINFURN=1 UNAOS_ORINCONWIN=1 UNAOS_ORINDESK=1`) does **not**
carry `UNAOS_RAST`, so the fix is a precondition for a conwin+rast flight rather than a repair to
any flight already planned.

Fixed at `main.rs:6973`: the same `if !tegra_conwin_live() { … }` guard its twin already carried,
folded in place so no source line moves. `tegra_conwin_live()` answers true only when
`fbcon::console_is_routed()` does, and a routed console does not write the panel (`FbCon::draw_fb`
hands back `win_fb`), so the single-writer guarantee the detach exists for is already true of the
mirror path.

**⚠ What the guard costs at THIS site is not what it costs at the phase-2 site,** and the code says
so rather than inheriting the argument. This site's stated purpose is "a straggler can't paint over
the demo frames", and a routed console still reaches glass through `wm`'s paced composite. So a
straggler line printed while the cube spins can now composite over it inside the console window's
rect, and because `orin_rast_console_owns()` is not latched until phase 2 the census would score
that `RAST-PAINTED-OVERWRITTEN`. **That is a true reading of a genuine second writer**, not a false
one, and it is the same trade the phase-2 line already took when it chose a live console over a
frozen one. `d33b6c3e` reverts cleanly on its own if the bench disagrees.

**The instrument half.** `[orinconwin]`'s `live=` field
(`arch/aarch64/display_tegra.rs:2677`) was the compile-time literal `"LIVE"` — an assertion about
the build, on the one line whose own comment four lines above demands every field be "DERIVED from
the outcome CROSSED with the route read back, never asserted". It is now a read-back:
`if fbcon::console_is_routed() { "LIVE" } else { "FROZEN" }` (`display_tegra.rs:2697`), taken at
print time. Not redundant with `route=`, which was sampled before `present_outcome` and `composite`
ran.

**⚠ THE HONEST LIMIT, stated at the site (`display_tegra.rs:2679-2696`) and repeated here so nobody
re-derives it.** `fbcon::detach()` sets `GUI_ACTIVE` and does **not** clear `CONSOLE_WIN`, so after
a detach `console_is_routed()` still answers `true` while `_print` returns at its first test and no
glyph reaches the window. The sample is taken before any terminus detach can have run, so the
strongest thing this field can say is **"the route is installed at this instant"** — never "live",
and never "no later detach freezes it". That second half is a behavioural guarantee owned by the
guards on the two detach sites, not by this field. Closing the gap needs `GUI_ACTIVE` exposed from
`video/fbcon.rs`, which was outside that arc's lane.

#### D2 — the supstate copy of phase 2 dropped the console-owns latch and the rast census

`jd2_supstate_phase2` (`main.rs:7524`) is a verbatim copy of `jd2_console_pump`'s phase 2, **except
for two appends it did not copy, and both are instruments.** It never returns (its drive loop is
non-breaking), so on a `supstate` image the legacy phase 2 is unreachable and neither append ran.

* **The latch.** The legacy line ends with `#[cfg(feature = "rast")] orin_rast_console_owns()`
  (`main.rs:2873`), placed outside the conwin guard because the console takes the panel on both
  routes. The supstate copy had no such append, so `RG_CONSOLE_OWNS` was **never set at all** on a
  supstate image. Read that off the census (`display_tegra.rs:4960`, arms at `:4995-5000`): with
  `owns` false, a console that has taken the panel and repainted the cube away scores
  `RAST-PAINTED-OVERWRITTEN` — which that census's own doc calls "the only arm that indicts a
  repainter" (`display_tegra.rs:4951-4957`) — instead of `RAST-SUPERSEDED-BY-CONSOLE`. **A healthy
  supstate+rast boot was being scored as a defect.** Restored at `main.rs:7550`.
* **The census.** The supstate drive loop swept `vugras::idle_sweep`, `sup_present_census` and
  `orin_click_census` but **not** `orin_rast_census`, so the rast census stopped dead at the phase-1
  boundary and the rung's verdict froze at whatever the last phase-1 sweep said — which, with the
  latch never set, was permanently pre-takeover. Restored at `main.rs:7657`, on the same statement
  and at the same cadence phase 1 advances. The census self-terminates on `RG_DONE`
  (`display_tegra.rs:5005`), so it costs one relaxed load per sweep once the question is answered.

Both appends are line-neutral, matching the shape of the sites they mirror. The `[orinrast]` census
line's `console-owns=` field (`display_tegra.rs:5008`) is what a capture reads to tell the two
states apart.

#### D3 — ORINPATH: two byte-identical takeover literals, and a single `.rodata` run that read as confirmation

`jd2_console_pump` and `jd2_supstate_phase2` emitted **byte-identical** JD2/JD4
`console OWNS the panel` literals. Both copies pass `#[cfg]` on a `supstate` build, so neither a
serial capture nor a grep of the artifact could attribute the line to a site — on the one transition
whose whole purpose is to be adjudicable from the wire.

**⚠ MEASURED at `0ed6fee2`, and the shape is worse than that description.** `LC_ALL=C grep -a -o` on
the `arm-tegra-supstate` `kernel.elf` found **exactly ONE** `.rodata` run for each literal, not two:
`jd2_supstate_phase2` never returns, so LLVM drops the legacy phase 2 as unreachable. **A single
unattributable run reads as confirmation** — one copy is in the image, one site prints it, and
nothing says which.

Each site now names itself: `path=jd2-console-pump` (`main.rs:2889-2898`) and
`path=jd2-supstate-phase2` (`main.rs:7565-7574`). With the token in place the same grep answers the
question. Two placement constraints, both deliberate:

* **Longer than 8 bytes on purpose.** A witness mark of 8 bytes or fewer can be LLVM-immediate-
  encoded and never reach `.rodata` at all, which would defeat the artifact-grep law this tree runs
  on.
* **Placed AFTER `console OWNS the panel`**, so `scripts/specs/jetson-sync1.spec` and
  `jetson-jd5.spec`'s `REQUIRE JD4.*console OWNS the panel` still matches.

⚠ This deliberately breaks the "must read identically knob-on vs knob-off" transcript property
`jd2_supstate_phase2`'s doc comment named as its behavioural falsifier. That property is retired in
`arch_arm64.md` §ORIN-SUPSTATE rather than silently violated: **being unable to tell the two
transcripts apart was never evidence that they were equivalent; it was the absence of evidence
either way.**

#### D4 — ORIN-STKDEPTH: a boot-core stack DEPTH, and the headroom it refuses to invent

Two SP reads on one descending frame chain — the anchor appended to `kernel_main`'s
`bootpace::record("entry")` statement (`main.rs:88`) and the seam read beside `tegra_desk_furn`'s
unconditional `[orinfurn] arm` line (`main.rs:7896-7899`) — subtract to the exact bytes of boot-core
stack that `kernel_main -> tegra_early_stop -> tegra_desk_furn` has consumed. It publishes
`[orinstkdepth] depth-consumed=… -> DEPTH-CONSUMED`, and `[orinstkdepth] DEPTH-UNAVAILABLE` with a
named reason when the anchor is unset or the two reads are not on one descending chain
(`main.rs:7900-7912`).

**DEPTH is not HEADROOM, and the distinction is the instrument.** Headroom is genuinely unavailable
on this board: the Orin boot stack is UEFI's and is never switched, `aarch64-unaos.json` names no
linker script so no `__stack_top` is linked into the jetson image, and the `MemoryRegion` slice that
would bound the region is discarded with `boot_info` by `memory::init`. The instrument prints
`DEPTH-UNAVAILABLE` rather than inventing a number.

§3.12's AMENDED subsection above records the argument against §5.2's clearing condition; **the
instrument itself is documented in
[`docs/dev/OS/01_BOOT_HAL/arch_arm64.md`](../01_BOOT_HAL/arch_arm64.md) §ORIN-STKDEPTH**, with the
wire format, the `#[inline(always)]` floor property, and the full "why no headroom number" argument.
⚠ One correction that must not be re-broken: `__stack_top` **does** appear in Rust source in this
tree, at `main.rs:51-52` inside a `global_asm!` block gated
`all(target_arch = "aarch64", feature = "baremetal")` — the Pi's bare-metal link, which the Orin
never takes. The claim is "not on the jetson link", never "nowhere in the tree".

#### §3.13.1 PANELOWN — an owner word for the panel, and the state it refuted in its first hour

Fold `63b86488` (commit `52137ffc`), `video/mod.rs` + `video/fbcon.rs`, +316 lines. Cross-lane work
in the x86 seat's files, taken under an explicit grant.

**The defect it names is an UNENFORCED INVARIANT, not an instrument gap, and that distinction is the
point.** `video/mod.rs:29-31` states the whole panel-ownership discipline as prose: `WRITER` and
`fbcon` are two handles onto **one** physical framebuffer, "used at different times", "each
serialised by its own lock". Both halves are true and neither is an invariant — the locks give
mutual exclusion WITHIN a handle and none BETWEEN them, so the only thing keeping two writers off
the glass was a temporal convention that nothing enforced and nothing witnessed. A survey of the
Orin panel path found **16 distinct panel writers across 14 transitions where the writer changes;
exactly one announced itself in both directions.** The closest thing to an owner bit, fbcon's
`GUI_ACTIVE`, was a private `AtomicBool` with no public getter anywhere in the tree. ⚠ The 16/14
figures are the authoring arc's survey; this section relays them and has not re-counted them.

**What landed.** `PanelOwner` + `PANEL_OWNER` beside `WRITER` (`video/mod.rs:243`, `:304`),
`panel_owner()` to read it (`:315`), `publish_panel_owner()` to announce a handover naming **both**
sides (`:336`), `note_panel_overpaint()` for a whole-surface paint that is not a handover (`:360`),
and five publish sites in `fbcon.rs`: `init` (`:566`), `detach` (`:1622`),
`panel_console_window_open` (`:2031`), `panel_console_window_closed` (`:2090`, on a successful CAS
only) and `panic_screen` (`:2104`). **It PUBLISHES ONLY.** Nothing consults it before painting, no
writer declines or defers on it, and no pixel moves. A refusal is a change to x86 paint order and is
a separate commit the x86 seat reviews first.

Three constraints from the cross-lane grant, each with a reason worth keeping:

1. **The STORE is unconditional on both arches, no `cfg`; only the EMIT is `witness`-gated**
   (`video/mod.rs:202-209`, `:336-349`). A feature-gated store would make the default image
   structurally differ from the witness image at exactly one site — a fresh unpaired arch gate, on
   the very census §1.2/§1.2.1 spent that day repairing. Proven by disassembling the **knob-off**
   images: x86 `detach` is `movb $1,GUI_ACTIVE / mov $2,%al / xchg %al,PANEL_OWNER / ret`; aarch64
   is an `ldaxrb`/`stlxrb`/`cbnz` pair.
2. **Atomic only, never under a panel lock** (`video/mod.rs:210-217`, the LOCKFIX rule at `:368`).
   LOCKFIX (`7847ceea`) forbids blocking on a raw panel lock from the preemptible input band,
   because that band runs on the same core as the IRQ-context printer and a blocking acquire
   preempted while holding leaves the next masked acquirer spinning forever — the boot-8 wedge.
   Panel handovers fire there: `panel_console_window_closed` is reached from a click on the console
   window's close disc.
3. **`swap(next, AcqRel)` rather than a plain store** (`video/mod.rs:319-336`). A bare store cannot
   name the DEPARTING owner, and a load-then-store pair can be interleaved by a second publisher
   into a line naming a side that never held the panel. One `AcqRel` RMW gets both in a single
   lock-free instruction that cannot block.

The emit fires only on an actual change of owner, so one line per transition rather than per call.
`Unknown` keeps no publisher on purpose: the moment something stores it, "the instrument never ran"
and "the instrument ran and reported its default" print the same word (`video/mod.rs:218-232`).

##### ⚠ THE INSTRUMENT REFUTED ITS OWN AUTHOR, on the arch the word was not written for

A sixth state, `Firmware`, was published at `init_panel` on the reading that this is where the
kernel takes the firmware's surface ahead of any console. **The first armed boot killed it on the
wire** — the aarch64 QEMU wire, per the site's own record, i.e. the arch this `video/` word was not
written for:

```
[panel-owner] ... from=owner-unknown      to=owner-fbcon-panel site=fbcon::init
[panel-owner] ... from=owner-fbcon-panel  to=owner-firmware    site=video::init_panel
[panel-owner] ... from=owner-firmware     to=owner-gui-screen  site=fbcon::detach
```

There is no firmware-owned epoch. `fbcon::init` runs from `main.rs:120` and `init_panel` from
`main.rs:1335` — **1215 lines and an entire boot log later** — so `init_panel`'s whole-surface
`fill_screen(PANEL_BG)` (`video/mod.rs:466`, the fill at `:473`) lands on a console that has owned
the glass since before the heap existed, and overwrites every pixel of the boot log it has been
printing. Worse for the record, publishing `Firmware` there **corrupted the DEPARTING side of
`fbcon::detach`** — the one transition the instrument exists to describe. The state was deleted;
that site now stores nothing and emits `panel-repaint-over-owner` instead (`video/mod.rs:360-366`),
naming whoever was holding the glass. The refutation is recorded at the site
(`video/mod.rs:477-507`) so the next person to reach for a `Firmware` state reads it first.

**This is not Orin-only, and it now has x86 wire evidence.** [CLAIM, rmbp 9 — the rmbp seat's
measurement on their board, relayed here, not re-derived] the seat verified the shape in their tree
and measured the window on a flight capture taken 2026-08-28:

```
[ 25662ms] :: fbcon: glyphs-active base=90020000 pitch=16384 cell=7x16 cols=411 rows=112 ::
[ 26030ms] :: FB Init ::
[ 26043ms] :: Framebuffer painted #1E1E1E ::
[ 26094ms] :: SERWIT-2 tap fbcon: submitted=2642 absorbed=466 ... ::
```

The console owns the glass for 368 ms having submitted 2642 lines, and then every pixel is
overwritten. **The operational cost the rmbp seat named: a boot that dies AFTER `init_panel` leaves
an operator staring at a blank #1E1E1E panel instead of the console text that would say why.**

**The general lesson, stated plainly because it generalises past this instrument: an instrument that
refutes one of its own states in its first hour is working; one that never contradicts its author is
decoration.** Publishing beat asserting, and it did so on the arch the word was not written for.
This is the same rule §3.12.2 states from the other direction, and it is now demonstrated rather
than only argued.

##### Two gaps found on the way, both real, neither fixed

* **`video::init_panel` is never called on tegra.** The Orin seeds `video::WRITER` directly in
  `tegra_early_stop`'s JD1 block; the tree already says so at
  `arch/aarch64/display_tegra.rs:313`. Confirmed by artifact absence **with a control** — that
  function's own older strings `:: FB Init ::` and `:: Framebuffer painted #1E1E1E ::` are absent
  from a tegra artifact too, so the absence is the function's and not the new token's. ⚠ Relayed
  from the authoring arc; this section did not rebuild the artifact.
* **`fbcon::panel_console_window_closed` has exactly ONE caller tree-wide** —
  `arch/x86_64/syscall.rs:5942` — and none on aarch64 (verified by `grep` over `--include=*.rs` at
  `63b86488`). **The Orin can OPEN a console window under `orinconwin` and has no close path that
  calls it.** A real gap on this board; outside that arc, and owed.

##### One deliberate surrender, stated rather than buried

`video/fbcon.rs:567` (the ORIN-FACE arm) carried a same-line append whose **stated purpose** was
preserving line numbering for a knob-off `kernel8.img` byte-identity proof. An unconditional store
surrenders that identity by construction — the default image now contains an instruction it did not
before, whatever the line numbering does. The statement was split onto its own line and both stale
notes corrected in place.

The grantor's ruling, recorded because the reasoning generalises: **a feature-gated store is a
permanent artifact on a census under active repair, while this is a one-time re-baseline of a
proof, and a cost that ends is cheaper than a cost that compounds.** And: **a stale proof claim left
standing is worse than a voided proof, because the next reader trusts it.**

#### Evidence status for §3.13, stated once

* **Gates (build only), as recorded in the fold commit messages — this documentation arc ran no
  gate of its own and re-derived none of these numbers:** `./arroyo check` `CHECK_EXIT=0` both
  arches across the TERMINUS fold, against a baseline run at `0ed6fee2` that was also 0, so the
  delta is attributable; the five `orinconwin` legs and `arm-tegra-supstate` green; PANELOWN green
  both arches in both `witness` polarities, go-red proven four ways.
* **Artifacts:** every new instrument confirmed one-hit in the built `kernel.elf` with
  `LC_ALL=C grep -a -o`; every new token longer than 8 bytes so none can hide as an LLVM immediate.
* **⚠ Runtime: NONE.** No Orin has booted any of the five instruments. `live=FROZEN` has never been
  observed, no `[orinstkdepth]` number exists, no `[orinrast] census … console-owns=1` line has been
  captured from a supstate image, and no `[panel-owner]` line has been seen on Orin metal. Every
  claim in this section is a source or artifact reading.
* **⚠ And no leg can score any of it** — see the finding at the head of this section. The gate
  results above bound compilation, not behaviour.

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

#### The stop-line has not been tested by anything, and one reading of it is retired (2026-08-28)

The 2026-08-26 desk sitting was read as evidence about this stop-line. It is not. Both parked
legs (desk1, desk2) died at `main.rs:2621` inside the first PSCI `CPU_ON`, 96 source lines
before the terminus, and the `orinfurn` seam printed nothing on either — see §3.12.1 for the
measurement. **No flight in this tree has yet driven the cascade far enough to sample the stack
hazard §5 describes.** The Pi's two overflows remain the only evidence that exists, and every
precondition, hazard and placement rule above is unchanged by the correction.

One inherited reading is retired. The record inferred a stack fault at the terminus partly from
the fact that "no stack number is obtainable there". That premise is true — `stk_probe` returns
early on the boot core before `run_capstone_boot_core` drives the queue, which is the same
structural gap §3.12 records as making §5.2's own clearing condition unsatisfiable. **The
inference is not.** An unavailable measurement is an instrument gap; it is not evidence for the
value the measurement would have returned. The stop-line stands on the Pi's two boots and on
§5.1's inventory, and it needs the boot-stack high-water probe §3.12 names before any rung can
claim to have cleared it. ⚠ **PARTIALLY ADDRESSED 2026-08-28:** ORIN-STKDEPTH
(§3.13 D4, and `arch_arm64.md` §ORIN-STKDEPTH) now takes the DEPTH half at the `[orinfurn] arm`
line and prints `DEPTH-UNAVAILABLE` for HEADROOM. That instrument is **UNFLOWN** — no number
exists — and a depth is not the high-water probe this paragraph asks for, so nothing here is
cleared. What has changed is that the clearing condition can now be restated in terms something in
this tree can supply.

---

## §6 The ladder

Seven rungs, each commit-sized, each with the witness that closes it. "Lane"
names the seat that owns the files under the parallel-arc rules in `CLAUDE.md`.

| # | Rung | What lands | Metal witness | Lane |
| --- | --- | --- | --- | --- |
| **0** | **One composited window** — ✅ **LANDED, FLOWN, and CLOSED ON BOTH HALVES 2026-08-25** (§3.8, §3.8.1) | call `wm::reserve_stage` on the tegra path after heap init (§3.3); mint one `wm` row; present it. No furniture, no `pidesk`, no cascade | **wire half CLOSED**: boot7f, capture line 11110 (and again boot7g, capture line 12679), `[orinwm1] win=1 panel=1920x1200 surf=640x400 box=650x444 at (635,378) scale=1 stage=4194304 present=Composited -> COMPOSITED`. **On-glass half CLOSED**: boot7g, capture line 12686, `[orinchrome] win=1 box=650x444 at (635,378) frame=6/6 content=0xff00ff@(960,617) MATCH … -> CHROME-ON-GLASS` — six frame probes and the content probe read back out of the scanout at panel coordinates, all MATCH. ⚠ **at composite time**: the operator measured that the JD2 console blit overdraws the body between composites (§3.8.1), which is rung 4's problem, not rung 0's | jetson |
| **1** | **The cfg leg** — ✅ **LANDED 2026-08-22, less `quarry`** (§3.5.1) | `arm-tegra-desk` leg added (gate 18 → 19 legs); `pidesk`/`quarry`/`livecon` mapped in arroyo's env map; two of the three gate mismatches fixed | `UNAOS_TEGRA=1 ./arroyo check` green 19/19, and green again under `UNAOS_TEGRA_EL0=1 UNAOS_PIDESK=1 UNAOS_LIVECON=1`; the new leg proven to go red on a re-introduced mismatch | jetson (arroyo + `arch/aarch64/syscall.rs`); the `quarry` line is a `video/` edit and is **held** in §3.5.2 |
| **2** | **The desktop seam** — ✅ **LANDED 2026-08-25, and it REFUSES** (§3.2.1) | `tegradesk` feature + `main.rs::tegra_desk_arm` on `tegra_early_stop`'s terminus line + `UNAOS_TEGRADESK` env map + the `arm-tegra-seam` leg (11 → 12 board legs). The seam evaluates its floors and declines at two named stop-lines | **the floors half is UNFLOWN**: `[deskseam] floors …` + `REFUSE reason=…` print on an armed Orin boot, and nobody has taken one. **The `activate()` half is WITHDRAWN, not owed**: `desktop_firmware::activate()` opens the console window and enables the bar, so running it crosses §6.1 *and* §5.2 — it belongs to rungs 3/5, and this row previously asked for something the same document forbids | jetson |
| **3** | **Input routing** — ✅ **LANDED 2026-08-25 as a DEFAULT-OFF knob; FLOWN, ARMED, and ROUTING ON METAL** (§3.7, §3.8, §3.8.1) | `orinclick` (implies `tegra_el0`) wires `jd2_console_pump`'s `Event::Button` arm into `wc_click_route` (§3.4) and adds the `[orinclick]` instrument at the tail of `display_tegra.rs`. **⚠ HANDSHAKE WITH RUNG 2, DISCHARGED IN THIS ARC:** `main.rs`'s `TEGRADESK_CLICK_ROUTED` no longer reads `false` — it reads `cfg!(feature = "orinclick")`, **not** a literal `true`, because `tegradesk` does not imply `orinclick` and a hard `true` would assert a route back on an image that has none: the one-way trip re-entered through the constant meant to prevent it. `arm-tegra-seam` now carries `orinclick` so the assertion is type-checked. COMPILES: gate green 21/21 knob off and on; the new `arm-tegra-orinclick` leg proven to go red. No gate in this tree can boot it — QEMU models no Tegra234 | ✅ **DISCHARGED, boot7g 2026-08-25** (§3.8.1): `[clickroute] press hit asid=4294967042 win=1 (was 0) delivered` (capture line 13084) and `[orinclick] edge=press btn=0x01 at (1009,546) geom=yes hit=yes win=1 owner=0xffffff02 focus 0x0->0xffffff02 consumed=0 -> RAISED` (capture line 13085); release `-> RELEASE-DELIVERED` (13087); census `IDLE-NO-CLICKS -> ROUTING` (13089); a second press on the focused row `-> HIT-SAME` (13092), plus `CONSUMED` (13125), `MISS-SHELL` (13133) and `RELEASE-DROPPED` (13135). Six press/release pairs with `stuck=0 nogeom=0 dropped=0`. **The prior owed item — boot7f's armed-but-unclicked state (`-> ARMED`, capture line 11424, then 48 `IDLE-NO-CLICKS`) — is closed.** Still owed: nothing on the wire; stack cost on this path (§5) is still a Pi number | jetson |
| **4** | **Console as a window** — ✅ **LANDED 2026-08-25 as a DEFAULT-OFF knob; FLOWN AND ROUTED the same day** (§3.9, §3.9.1) | `orinconwin` (implies `pidesk` + `tegra_el0`, and deliberately NOT `orindesk`/`orinclick`) calls the SHARED console-window machinery from `display_tegra::orin_conwin` on `tegra_early_stop`'s terminus line — `panel_console_face_arm` → `panel_console_window_open` → `console_is_routed` — and folds `jd2_console_pump`'s phase-2 `fbcon::detach()` to `if !tegra_conwin_live() { … }` so a routed console stays LIVE. **§6.1 IS NOW A BRANCH:** both ordering terms are read through `cfg!()` and an image missing either gets `[orinconwin] DECLINE reason=ordering-rule held=…` and NO window — measured on the artifact both ways. No `video/` edit; no `desktop_firmware::activate()`, so §5.2 is untouched. Gate green 23/23 knob off and on; `arm-tegra-conwin` proven to go red; knob-off loadable image byte-identical | ✅ **DISCHARGED, boot7h 2026-08-25** (§3.9.1): `[orinconwin] gate … dock=GRANTED … orindesk=1 orinclick=1` (capture line 14828), then `[orinconwin] win=2 panel=1920x1200 cell=7x16 stage=4194304 table=2 present=Composited route=true live=LIVE -> ROUTED` (14833) with the `[wc-x] console-window / console-route first-paint / panic-fallback armed` trio beside it (14830–14832). ⚠ **`live=LIVE` there was a COMPILE-TIME LITERAL, not a measurement** (§3.13 D1) — it would have printed `LIVE` whatever the route did. It became a read-back on 2026-08-28. This flight's image carried no `UNAOS_RAST`, so the unguarded second detach §3.13 D1 describes was the empty stub here and did not affect the capture; the 107-minute sitting, not the field, is what evidences the route staying live. The route stayed LIVE for a ~107-minute sitting — shell banner, keystroke echoes and verb output all landed through the window path; chrome clicks CONSUMED and the close control `REFUSED furniture` (14926–14927). **Still owed:** the dock round-trip (`presses=0` on every `[dock]` line — the minimise disc was never clicked) and a win=2 glyphs-on-glass read-back — ⚠ **both INSTRUMENTED 2026-08-25 under `orinladder`, both still UNFLOWN: see §3.11 for the two flight cards and every broken shape each one reads as** | jetson |
| **5** | **The real desktop** — ⚠ **PARTIALLY LANDED 2026-08-26 as `orinfurn`: the MENU BAR half only** (§3.12) | the full row is unchanged: dock, strip, menubar, crystal armed; the full `pidesk` cascade; a tegra `render_service` (§3.6). What `orinfurn` takes is TWO of `activate`'s nine steps — `menubar::set_enabled(true)` + `wm::composite()` + the `owns_pixels` read-back — on the terminus line, DEFAULT OFF, with `desktop_firmware::activate()` NOT called and `TEGRADESK_CASCADE_OK` NOT touched. The cascade, the DESKTOP-CLEAR, `crystal::routed_selftest`, window population and the render service are all still owed | the Orin comes up to a desktop. **`orinfurn`'s own half is UNFLOWN**: `[orinfurn] ARMED … -> BAR-ON-GLASS` and a crystal press consumed by the menu band are both Orin-metal verdicts nobody has taken | jetson — the CASCADE is still **blocked by §5.2**; ⚠ and see §3.12 for why §5.2's `[u7stk]` evidence requirement is *structurally unsatisfiable at the terminus* (`stk_probe` returns early with no current task), which is a defect in the stop-line's clearing condition, not a reason to step over it |
| **6** | **EL0 tenants** — ✅ **LANDED 2026-08-25 as the CRYSTAL-HD parity fix + a DEFAULT-OFF instrument knob; UNFLOWN** (§3.10) | the `SYS_WIN_*` surface needed NO new verb — the gap was `mmu_tegra_el0.rs` carrying the pre-CRYSTAL-HD FB geometry (128x128 cap, 0x1_0000 slot stride), which refused the shipped vug's `SYS_WIN_CREATE(288,288)` with `-EINVAL` and mis-mapped the WC-B fixture's slot 1. Parity restored (4 slots x 0x51000, 288x288, unconditional under `tegra_el0`); `orintenant = ["tegra_el0"]` arms the terminus `reserve_stage` + the `[orintenant]` arm/create/close/reap/census instrument. Tenant close policy: CLOSE-CLEAN (tenants close; furniture refuses). Gate green 24/24; `arm-tegra-tenant` + the `arm-tegra-conwin-tenant` conjunction cross both go-red-proven; knob-off jetson AND Pi loadable images byte-identical | an EL0 program owns a window on the Orin panel: `run /fat/vug.elf` on the four-knob conjunction image -> `[orintenant] create … surf=288x288 wm-bound=1 -> TENANT-WINDOW`, census `IDLE-NO-TENANTS -> TENANT-LIVE`, and a clean exit reaps (§3.10 flight card) | jetson |

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

The reason is written into the seam itself. `video/desktop_firmware.rs:39-44` states the
CONSOLEWIN law, inherited unchanged from `wcx`:

> the console window carries a minimise disc; the only route back from that park
> is the dock; `dock::Layout::for_panel` returns `None` when the strip will not
> fit at `MAX_WINDOWS` rows; **a control that hides a window with no way back is
> worse than no control**, so a panel that cannot guarantee the dock gets no
> console window.

The dock is only a way back once clicks route. Land rung 4 first and the Orin
ships a console window whose minimise button is a one-way trip — strictly worse
than the full-screen console it replaced. `desktop_firmware.rs` enforces the panel-geometry
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
  branch reaches `desktop_firmware::activate()`; §5.2's stop-line is untouched and is now
  enforced by codegen as well as by source.
- **Only rung 1 is claimed done, and only as a type-check.** Rung 1's claim is exactly
  "the armed tegra desktop configuration compiles and a gate leg compiles it" — nothing
  on this branch arms `desktop_firmware::activate()` at runtime, and §5.2's stop-line is untouched.
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
