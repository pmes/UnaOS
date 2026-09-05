# ARCHITECTURAL CONFORMANCE REPORT — UnaOS, 2026-09-02
Tree: `/home/pmes/src/github.com/pmes/UnaOS-orin`, branch `hw-jetson`, HEAD `a1cf4900`.
23 findings survived adversarial refutation; 5 were killed and are named in §4.

---

## 1. THE SHAPE OF THE PROBLEM

UnaOS has one design implemented two-and-a-half times — an EL0 syscall/ABI layer written once per arch (107 same-named functions and 45 same-named statics across `arch/aarch64/syscall.rs` and `arch/x86_64/syscall.rs`), a scheduler primitive set written twice, a shell host loop written five times and a background service pass written three times inside a single 8,276-line file — and *nothing in the repo names those pairs as pairs*, so a fix landed on one half is invisible on the other. Hardware genuinely forces some of this: the Tegra vendor register block above `0x100` (`drivers/sdmmc_tegra.rs:150-160`), the Broadcom combined-register wrapper vs generic SDHCI widths (`drivers/sdhc.rs:126-131`), the futex key that would need a page-table walk to match on x86 (`arch/x86_64/sched.rs`, WINX-7 header), and rtpi priority inheritance that Cargo declares x86-only — these divergences are stated, reasoned, and correct. Process-caused divergence is a different animal and it is the one that bites: aarch64's `sys_cap_revoke` (`arch/aarch64/syscall.rs:8932`) leaks the file descriptor that x86's byte-for-byte twin was fixed to free at `arch/x86_64/syscall.rs:14717`, and aarch64's `wc_focus_key` (`:13329`) never received the drag-cancel guard x86 got at `:5331` — in both cases the copy inherited the other side's *comments* but not its later *fix*, which is precisely why a reader's eye confirms sameness and moves on. The second failure mode is silent wiring: latches whose only setter is unreachable on the board that reads them (`pulsewin::arm` has one caller tree-wide, inside a function tegra is forbidden to call; `dock::take_shell_reopen` is drained only inside `#[cfg(target_arch = "x86_64")] fn x86_render_service`), producing buttons and windows that are acknowledged on the wire and do nothing. Underneath both sits the real structural defect: the repo enforces *behaviour* rigorously with go-red proofs and a 44-leg cfg matrix, but enforces *structure* with prose — and prose rots, which is why the one structural check that exists (`arroyo:3966`) has been green and unfireable since the day it was written.

---

## 2. THE NON-CONFORMITY LEDGER

| # | Finding | Where | Sev | The one executable check that catches it |
|---|---|---|---|---|
| 1 | Knob→leg coverage check is unreachable — every one of 151 features hits the `continue` at 3977, so the ❌ branch can never run and the allowlist is decorative | `unaos/arroyo:3966-3988` (cause: `_rows` at 3911 includes derived `KERNEL_CFG_MIX`) | **critical** | Add feature `zzz_armprobe = []` + one `#[cfg(all(target_arch="aarch64", feature="zzz_armprobe"))]` site under `arch/aarch64/`; run `./arroyo check`. Must go red. Today: green. |
| 2 | `orin-render` runs on the blanket 16 KiB stack and traversed its redzone on pass ~1 | spawn at `main.rs:8166`; `arch/aarch64/sched.rs:41`; witness `~/unaos-bench/capture/line-acm0/orin.log:41895` | **critical** | Assert no `[redzone] … task=*:orin-render` line for a whole boot, with `sched::stk_probe("orinrender:pass")` reporting non-zero headroom. |
| 3 | Render service publishes a whole-panel present from a back buffer nothing ever painted; `a1cf4900` removed the `clear_screen` that was accidentally seeding it | `main.rs:8191`; `video/screen.rs:967,1002-1006`; cure `adopt_desktop_bg` (`screen.rs:863`) has zero aarch64 callers | **critical** | First `[wc-w] rollup` of an Orin boot must report `presented_px` ≈ the pulse band's area, not 2,304,000. |
| 4 | `pulsewin::service()` is called every pass on the Orin and can never open — sole setter of `ARMED` is inside a `desktop_firmware::activate()` tegra is forbidden to call | call `main.rs:8248`; gate `video/pulsewin.rs:598`; sole setter `video/desktop_firmware.rs:373` | **high** | `grep -rn 'pulsewin::arm' unaos/crates/kernel/src/` → 1 hit, in a module no tegra leg reaches. Generalise: every gate-latch must have a setter reachable under the reader's own cfg. |
| 5 | EL0 syscall/ABI layer written twice: 107 twin fns, 45 twin statics; ~246 arm lines mechanically identical, ~440 with near-twins folded in | `arch/aarch64/syscall.rs` ↔ `arch/x86_64/syscall.rs` | **high** | The `comm -12` symbol intersection, ratcheted against a checked-in ledger (§3, FC-4). |
| 6 | `sys_cap_revoke` leaks the FD on aarch64 — x86's `KIND_FILE` branch was never mirrored back | `arch/aarch64/syscall.rs:8932-8949` vs `arch/x86_64/syscall.rs:14706-14727` | **high** | Twin-body similarity ratchet (§3, FC-5): x86's body grew, arm's did not, similarity dropped, ledger not updated. |
| 7 | `wc_focus_key` diverged — aarch64 never got x86's drag-cancel guard, so TAB mid-drag leaves the grab steering the old window | `arch/aarch64/syscall.rs:13329` vs `arch/x86_64/syscall.rs:5265,5331-5334` | **high** | Same ratchet as #6. 36 of 41 comment-stripped lines identical; the delta is the guard. |
| 8 | Background service pass triplicated in one file, already drifted — 29 shared callees, five 2-of-3 omissions | `main.rs:1135-1325`, `:1629-1815`, `:5840-6018`; repo admits it at `main.rs:1676-1677` | **high** | Sentinel-marked regions + callee-set identity modulo a declared exception list (§3, FC-6). |
| 9 | Shell host loop written five times; `handle_key` is shared, nothing that drives it is | `main.rs:1885, 2949, 5412/5505, 6661, 7695`; cost lands in `selftest.rs:35-41` | **high** | Loop-body census with a ratchet: N distinct drain/present frames, red on N+1 without a note. |
| 10 | Scheduler `Channel<T>` byte-identical across arches (1-line delta) | `arch/aarch64/sched.rs:7422-7465` vs `arch/x86_64/sched.rs:4841-4889` | **high** | Twin ledger extended to the `sched.rs` pair. |
| 11 | Entire Ring-3 host workspace (33 of 35 crates) compiled by no gate; 4 crates aren't even workspace members | root `Cargo.toml:26` (`# "handlers/vug"`), `handlers/helm`, `handlers/junct`, `tools/orin-xhci-repro`; sole gate touch is `arroyo:5979` | **high** | `find . -name Cargo.toml` minus (members ∪ unaos crates ∪ declared-excluded-with-reason) must be empty (§3, FC-7). |
| 12 | `./arroyo test` / `test-arm` are negative-only: a boot that emits 3 lines then wedges exits 0 | `arroyo:2287-2296`, `:2346-2355`; FAULT_PATTERNS at `:2243` | **high** | `./arroyo test-arm 3; echo $?` must be non-zero. Today it is 0. |
| 13 | Bootloader feature `unaos_ivb` — 12 cfg sites reshaping the shared `BootInfo` — type-checked by no leg | `crates/bootloader/src/main.rs:444,987-999,1049,1057,1386,1435`; `$BOOTLOADER_FEATURES` at `arroyo:1768-1776` | **high** | Every feature in `crates/bootloader/Cargo.toml` must be named by ≥1 bootloader check leg (§3, FC-11). |
| 14 | 9 of 11 mbench specs (301 directives) replayed by no command; `mbench.py --self-test` and `scripts/orin-specscore.py` run by nothing | `arroyo:6200,6223` are the only replays | **high** | Every `scripts/specs/*.spec` must be replayed by a named leg or carry a `BENCH-ONLY:` header naming its capture source (§3, FC-9). |
| 15 | `[orinrender] census` is 82.2% of the boot's entire post-terminus capture; its "rate limit" is a pass count on a busy-poll loop | `main.rs:8262-8266` vs the correct pattern at `main.rs:5666-5668` and `:2840-2848` | **high** | Post-boot: `awk '/orinrender\] census/' | wc -l` must be ≤ 1 per 10 s `[pstrip] rollup` window. |
| 16 | Orin shell window is empty by construction — mint discards `_surf_fb`; the surviving `else` arm at 8227 re-mints the same empty box on a knob-solo image | `main.rs:8227-8231`; Pi's seven-line painter at `main.rs:5584-5598` | **high** | On a `UNAOS_ORINRENDER=1`-only image: either `[wcn] win=3 … att>0 dout>0` or an explicit `[orinrender] DECLINE reason=no-painter`. `att=0 comp>0` must never recur. |
| 17 | Kernel README module map names 8 of 38 top-level modules, and one it names (`vug`) isn't compiled on x86 | `unaos/crates/kernel/README.md:20-25` vs `lib.rs:27-234` | **high** | Every `pub mod` in `lib.rs` must appear in the README map or in a `NOT-MAPPED:` block with a reason (§3, FC-10). |
| 18 | The repo's only layering statement is a two-box model with no slot for the on-metal EL0 layer the whole roadmap is about | `CLAUDE.md:8-9`, `README.md:18-26`, `docs/dev/USERLAND/ARCHITECTURE.md:9-14` vs `unaos/crates/user-*` (6 crates) | **high** | `ls -d unaos/crates/user-*` → 6; assert each maps to a named layer. Doc-side; see §4. |
| 19 | "Ring 3"/"userspace" name two different layers; `docs/NAMING_ATLAS.md:15-30` uses only one sense and files `user-blob` as Ring 0 | `README.md:26` + `:91,:98`; `docs/dev/OS/02_KERNEL_CORE/userspace.md:1`; `docs/NAMING_ATLAS.md:20` | **high** | Doc-side; see §4. |
| 20 | `pulsewin::key_escape` has zero call sites, so Esc cannot dismiss the View menu — its own doc asserts a caller that doesn't exist | def `video/pulsewin.rs:900-902`; the router that doesn't call it, `arch/aarch64/syscall.rs:13186` | medium | Dead-`pub` census over `video/` against an allowlist (§3, FC-1). |
| 21 | `dock::press_at` latches `SHELL_REOPEN` on aarch64; the only drain is inside `x86_render_service` — the Pi/Orin pinned shell tile is a dead button | setter `video/dock.rs:997-1003`; drain `main.rs:6763` under `#[cfg(target_arch="x86_64")]` at `:6322` | medium | Arch-asymmetric consumer check (§3, FC-2): a latch set under a both-arch cfg with consumers on one arch only. |
| 22 | `tegra_render_arm` is the only Orin seam that omits `wm::reserve_stage`; a knob-solo image grows the STAGE `Vec` lazily | `main.rs:8115-8172` vs `:7344`, `:7941`, `display_tegra.rs:408,2600,2875` | medium | `grep -rn 'reserve_stage'` — every seam that spawns a compositing task must appear. |
| 23 | `top_chrome_h` and `Console::top_y` docs both assert 0 on aarch64; MENUBAR-PI falsified that, and the body two lines below contradicts the header | `ui_status.rs:629-632`; `console.rs:186-188`; cfg at `video/mod.rs:120` | medium | Doc-vs-cfg conformance: a doc asserting an arch invariant must match the cfg of the code it heads. Partly checkable; see §4. |
| 24 | "Gneiss PAL" names two unrelated components at two rings under two licenses, with no cross-reference | `unaos/crates/kernel/src/pal.rs:63` (GPL-3.0) vs `libs/gneiss_pal/src/lib.rs:1` (LGPL-3.0) | medium | No file in the tree contains both `gneiss_pal` and `GneissPal`. Doc-side; see §4. |

Finding #4 was surfaced twice independently, from `invisible-coupling` and from `orin13-niggles`, by different evidence paths. That is itself a signal: the same defect shape is visible from the seam and from the board, and neither view names the other.

---

## 3. ARCHITECTURE FITNESS CHECKS

The repo already has the right instinct — `arroyo:4109` (GATE-BOOTLOADER) and `arroyo:3966` were both born from "an invariant asserted in a comment rotted, so it is now a check." What is missing is that structure gets the same treatment as behaviour, *and* that new checks are themselves proven to fire. Proposal: one new verb, `./arroyo fit`, with sub-checks, plus `./arroyo fit --prove`.

**FC-0 — The go-red harness. Build this first, before anything else.**
Asserts: every fitness check, and every existing structural check in arroyo, turns red against a named fixture mutation.
Mechanism: `unaos/gates/fixtures/<check>.mutation` — a patch or a generated file that violates exactly the invariant. `./arroyo fit --prove` applies each to a `git worktree add` throwaway tree (never the live tree; never `git stash`, per CLAUDE.md), runs the check, asserts non-zero, discards.
How proven to fire: it *is* the proof mechanism; its own self-test is a no-op mutation that must stay green.
Caught tonight: **finding #1.** The knob→leg fixture (`zzz_armprobe`, aarch64-only site, no leg) leaves `./arroyo check` green today. Cost: seconds — the coverage block runs no cargo.
Why first: the critical finding is a gate that has printed ✅ on every run since `434700ab` and cannot go red. Every check below could be born the same way. Build the falsifier before the checks.

**FC-1 — Dead-`pub` census (`./arroyo fit --deadpub`).**
Asserts: every `pub fn` in `video/`, `ui_status.rs`, `console.rs` has ≥1 non-comment, non-definition call site, or is listed in `unaos/gates/deadpub.allow` with an owner and a reason.
Proven to fire: add `pub fn zzz_probe()` to `video/pulsewin.rs` → red.
Caught tonight: **#20** (`pulsewin::key_escape`, 0 callers against a `pub` definition and a doc claiming a caller). Cost: one grep pass, sub-second.

**FC-2 — Arch-asymmetric consumer (`./arroyo fit --consumers`).**
Asserts: for a `pub fn` in a module whose `video/mod.rs` cfg names *both* arches, if it has consumers under `arch/x86_64/` or x86-cfg'd `main.rs` regions, it must also have one on the aarch64 side — or an allowlist row.
Proven to fire: delete the aarch64 caller of `quarry::service` → red.
Caught tonight: **#21** (`take_shell_reopen`: three hits, all x86), and **#4** in its generalised form. This is `parity-arch-gates.sh` pointed at *call sites* instead of *cfg lines*, and unlike the cfg version it has no over-report problem — a missing consumer is not a judgement call.

**FC-4 — Twin ledger (`./arroyo fit --twins`).**
Asserts: the set of same-named top-level `fn`/`static` symbols across `arch/aarch64/syscall.rs`↔`arch/x86_64/syscall.rs` and `arch/aarch64/sched.rs`↔`arch/x86_64/sched.rs` equals the set in `unaos/gates/twins.tsv`. A new twin is red until the ledger names it and says why the pair exists.
Proven to fire: add `fn zzz_twin()` to both files → red.
Caught tonight: the growth path behind **#5, #10**. Seed value today: 107 fns + 45 statics — note the finding measured 106; the number moved during this survey, which is the argument for a ratchet rather than a comment.

**FC-5 — Twin-body divergence. The highest-value check in this list.**
Asserts: for each ledger row, a comment-stripped, index-alias-canonicalised (`asid as usize` ↔ `row`/`slot`) similarity score. Red when a score *drops* without the ledger row being updated with a reason.
Proven to fire: delete the `KIND_FILE` branch at `arch/x86_64/syscall.rs:14717` → the score for `sys_cap_revoke` moves → red.
Caught tonight: **#6** (x86 gained a branch, arm did not) and **#7** (x86 gained the drag-cancel guard, arm did not) — *at the moment the one-sided fix landed*, which is exactly the maintainer's stated problem. Note honestly what it does not do: it cannot say which side is right. `arch/x86_64/syscall.rs:14711-14716` itself flags that freeing at revoke may be wrong for GRANT-minted duplicates. The check produces a decision point, not a decision.

**FC-6 — Service-pass membership.** Sentinel comments `// SERVICE-PASS BEGIN p1` … `END`; assert the three callee sets are equal modulo `unaos/gates/servicepass.except` (which today would carry `vperf::scenario_tick` with the fbcon-attached reason, and four rows needing an owner). Proven to fire: delete one call from one pass → red. Caught tonight: **#8**'s five silent omissions.

**FC-7 — Compile coverage.** `find . -name Cargo.toml` minus (root members ∪ `unaos/crates/*` ∪ `unaos/gates/nocompile.declared`) must be empty; plus `cargo check --workspace --all-targets` at root as a `check_both` step beside the existing `cargo test -p midden_core` (`arroyo:4167`). Proven to fire: `mkdir handlers/zzz; cargo init` → red. Caught tonight: **#11** — 33 crates, including `handlers/vug` commented out at `Cargo.toml:26`.

**FC-8 — Positive-witness floor.** Two lines, one per leg, after `scan_serial_faults`: `awk 'BEGIN{f=1} /MISSION SUCCESS/{f=0} END{exit f}' "$logf" || return 1`. Proven to fire: `./arroyo test-arm 3` → must exit non-zero. Caught tonight: **#12**.

**FC-9 — Spec inventory.** Every `scripts/specs/*.spec` is either named by an arroyo replay call or carries a `BENCH-ONLY:` header naming its capture. Plus two free battery steps: `mbench.py --self-test` (34/34, sub-second, verified green) and the jetson green-capture replay (17/17, verified green). Proven to fire: add a headerless spec → red. Caught tonight: **#14**.

**FC-10 — Module-map conformance.** `lib.rs`'s `pub mod` set ⊆ (README map ∪ `NOT-MAPPED:` block). Proven to fire: add a module → red. Caught tonight: **#17** — 30 unmapped, one stale.

**FC-11 — Feature-leg completeness for the bootloader.** Every feature in `crates/bootloader/Cargo.toml` named by ≥1 leg in the `arroyo:4110-4120` loop. Proven to fire: add a feature → red. Caught tonight: **#13** (`unaos_ivb`).

**FC-12 — Layering ratchet (free; green today).** `video/*.rs` must contain zero non-comment references to the window verbs (`sys_win_`, `sys_cap_`, `sys_open`, `sys_read(`). Measured today: 0. The rule is already *stated* — `docs/dev/OS/08_VIDEO/engine.md:2889-2891`, `arch/aarch64/syscall.rs:12487-12489` ("`WINDOWS` ⊃ `video::wm::TABLE` ⊃ `video::WRITER`", VERIFIED at WC-INT) — and its rationale is lock-order acyclicity. Note the nuance that keeps this honest: `video/wm.rs:714,718,3099` *do* reference `arch::…::syscall`, but for `set_hidden` (an info-page publish) and `user_input_active` (a read), not window verbs, so the check must be scoped to the verb list rather than to `syscall::` wholesale. Proven to fire: add `crate::arch::syscall::sys_win_present(…)` to `wm.rs` → red.

**Build order and why:** FC-0 first, because tonight's critical finding is a gate that cannot go red and every check here is at risk of the same birth defect. FC-4+FC-5 second, because they address the maintainer's stated problem directly and cover six findings, and because they are the only proposal that catches a one-sided fix *on the commit that lands it*. FC-8, FC-7, FC-11 third — one line, one function and one leg respectively, all trivially provable. FC-1, FC-2, FC-12 fourth: pure grep, sub-second, four more findings. FC-6, FC-9, FC-10 last: each needs a sentinel or a header convention introduced first, which is a small design cost the others don't carry.

---

## 4. WHAT CANNOT BE MADE EXECUTABLE

**Whether a stated reason is true.** A check can demand that an arch gate carry a justification; it cannot evaluate one. This is not hypothetical — `docs/dev/OS/08_VIDEO/PARITY.md:117` and `:2346` strike two previously-stated hardware reasons as false ("aarch64 has no interlock at all"), and `0c8936ef` corrected a stated reason that was arithmetically wrong ("a fifth of the aarch64 heap" → one twelfth). Justification-present does not imply justification-correct.

**Whether a divergence is hardware-forced.** `parity-arch-gates.sh`'s own header says it "DELIBERATELY OVER-REPORTS" and lists shapes it "STILL CANNOT SEE." Every row is a triage candidate — an upper bound on drift, not a measurement of it. The refutation of the SDHCI finding is the case in point: three transcriptions look like triplication until you read `drivers/sdhc.rs:126-131` and find a real, hardware-grounded reason (VideoCore's combined 32-bit views vs a generic SDHCI part's spec-defined widths), and the three drivers are pairwise mutually exclusive at compile time anyway. Do not automate that verdict.

**Which twin is correct.** FC-5 will tell you `sys_cap_revoke` diverged. It cannot tell you whether freeing the descriptor at revoke is right — `arch/x86_64/syscall.rs:14711-14716` argues against its own behaviour for GRANT-minted duplicates. A human decides; the check only guarantees the decision gets made.

**Whether an omission is deliberate.** `vperf::scenario_tick` appears in one service pass of three, plausibly because that pass is the only lane keeping fbcon attached. A checker sees an asymmetry; only a sentence in an exception file makes it a decision rather than a defect. The check's real product is *forcing the sentence to exist*.

**Sufficiency of a resource bound.** Finding #2 is a stack that overflowed. No static check knows that 16 KiB is too little for a compositing task; only a redzone report on real hardware does. The executable half is narrow but worth having: assert that every spawn of a compositing task uses `spawn_stack` with an explicit size and cites a measurement. The measurement itself is bench work.

**Naming and layering vocabulary (#18, #19, #24).** That `Ring 3` means two things, that the two-box model has no slot for `unaos/crates/user-*`, and that `pal.rs`'s `GneissPal` and `libs/gneiss_pal` are unrelated namesakes — these are doc defects with doc fixes. A grep can prove the terms are used both ways; it cannot pick which usage should win, and the answer ("both, and here is the third layer") requires a paragraph someone has to write. One partial exception: **#23** *is* mechanically checkable, because the doc asserts an arch invariant sitting two lines above the cfg that falsifies it (`ui_status.rs:629-632` vs `video/mod.rs:120`) — a doc-vs-cfg comparison catches that specific shape.

**Whether the change matches the request.** The maintainer's stated problem — "satisfies the immediate request while violating the architecture" — has an unavoidable human half. Every check here sees shape. None sees intent.

---

## 5. THE THREE-SEAT QUESTION

**The evidence does not support changing the lane structure.** Three of the five refuted findings were the case *against* it, and each died on its own data:

- The "constant divergence rate — a process constant, not a backlog" claim reproduces numerically but inverts under measurement. The denominator counted comments (comment share rose 34.5% → 48.3% across the window, mechanically flattening the series); on code lines the last four snapshots decline *monotonically* (11.37 → 11.21 → 9.83 → 9.04), and the marginal rate — new unpaired gates per 1000 new code lines, which is what "regenerated proportionally" actually asserts — falls 14.38 → 13.14 → 10.44 and then goes negative. A fixed-cohort test splits it further: the pre-existing 47 files plateaued (6.57 → 11.54), while files added since are getting monotonically *cleaner* (8.24 → 6.32). New work under the current regime is the healthy population.
- The "asymmetric cost makes triplication stable" claim is self-refuting. `0372eba7` (TEGRASD) *adds* 27 `target_arch` lines and zero removals across four shared files, and cost **two** recorded lane grants — adding divergence is not free. Meanwhile the one unification that was genuinely free (`chain_clusters` at `fs/fat.rs:2341` and `collect_chain` at `:2668`, same file, same lane, zero negotiation) was **not done** — `a2281332` copied the cached walk rather than unifying. An asymmetry that is neither necessary nor sufficient is not the cause.
- The "process-caused gates stay silent" asymmetry is half-false: the missing audit, run over the 79 PORT sites, finds 29 with no comment at all — not 81 — and `wm.rs:7544-7551` at `afc9f239^` carries an elaborate *cross-track process* reason at the gate.

**What the evidence does support is narrower and is not an org change.** Nobody owns a *pair*. The lane rule correctly assigns `arch/aarch64/syscall.rs` to one seat and `arch/x86_64/syscall.rs` to another; it assigns the 107-symbol relationship between them to no one. That is where #6 and #7 were born — not from a seat touching the wrong file, but from a seat touching the right file with no artifact telling it a twin exists. A fourth "parity seat" would be worse than the ledger: it re-introduces the integrator seat Peter abolished on 2026-08-18, adds a negotiation to every one-sided fix, and still relies on a human noticing. `unaos/gates/twins.tsv` under FC-4/FC-5 discharges the same duty at zero coordination cost — the pair becomes a file both seats already have to update, and the gate refuses the commit otherwise.

Second observation, also a code change rather than an org one: `main.rs` is the actual collision point. It carries the most arch gates in the tree (207 non-comment sites), the three drifted service passes, five of the six shell loops, and every Orin seam — and while it is nominally rmbp's shared-core lane, all three tracks edit it. Five of tonight's findings live there. The fix is a registration table for service-pass members so a new service is added once instead of three times, plus a size ratchet — not a new seat.

---

## 6. SEQUENCING

**Step 1 — "Make the gate able to fail" (one session, all inside `unaos/arroyo`).**
Land: (a) the FC-0 go-red harness — `./arroyo fit --prove`, `unaos/gates/fixtures/`, throwaway-worktree runner; (b) the one-line fix at `arroyo:3968` (iterate `"${KERNEL_CFG_MATRIX[@]}"` not `"${_rows[@]}"`), with the Cargo implication closed (`tegra_el0 = ["tegra","aarch64_el0"]` — today the corrected rule prints exactly one line, `aarch64_el0`, so it is one implication-closure from green); (c) FC-8's two positive-witness lines. DONE gate: `./arroyo fit --prove` red on all three fixtures before the fixes, green after; `./arroyo check` still green; `./arroyo test-arm 3` now non-zero; `docs/dev/LAWS.md` §Verification updated. This is the only step that must go first, because it is the falsifier everything else is built on.

**Step 2 — "Make the twins visible" (one session).**
Land `./arroyo fit --twins` and `--twin-drift`, plus `unaos/gates/twins.tsv` seeded with today's 107 fns + 45 statics for the `syscall.rs` pair and the same-named set for the `sched.rs` pair, each row carrying arm line, x86 line, canonicalised similarity, and a note. Go-red fixtures: a new twin symbol, and a deleted branch in an existing one. Then file #6 and #7 as ledger rows with owners — the ledger records the divergence; *fixing* either is a separate arc for the arch's own seat, and #6 in particular needs the semantics question in §4 settled first.

**Step 3 — "Make reachability a gate" (one session).**
Land FC-1 (dead-`pub`), FC-2 (arch-asymmetric consumer), FC-12 (layering ratchet, green today), FC-7 (compile coverage), FC-11 (bootloader feature legs). All five are grep-and-set-arithmetic, all five have trivial go-red fixtures, and together they cover findings #4, #11, #13, #20, #21. Ship the `deadpub.allow` and `nocompile.declared` files seeded with today's state and an owner per row.

**In parallel, not behind these — the focus track's own lane.** Findings #2, #3, #15, #16, #22 are Orin code fixes in `main.rs`'s tegra tail, entirely within the focus seat's brief, and two are critical: `spawn_stack("orin-render", …, 32*1024)` (`sched.rs:3721` is ungated; `spawn_prio_stack` is not compilable on tegra), a back-buffer seed after `main.rs:8192`, `pulsewin::arm()` beside `main.rs:8157`, a named `DECLINE` on the `else` arm at `main.rs:8227`, `wm::reserve_stage` in `tegra_render_arm`, and a CNTPCT-driven census. These do not need any gate to exist first, and the stack overflow makes every reading taken later in that capture suspect.

**After the three steps:** FC-6 (service-pass sentinels — pairs naturally with the registration-table change from §5), FC-9 (spec inventory plus the two free battery steps, both verified green and sub-second today), FC-10 (module-map conformance), and the doc arc for #17/#18/#19/#23/#24, which is one writing session against `unaos/crates/kernel/README.md`, `docs/NAMING_ATLAS.md`, `CLAUDE.md:8-9`, `ui_status.rs:629` and `console.rs:186`.