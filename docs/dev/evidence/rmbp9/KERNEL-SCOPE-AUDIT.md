# KERNEL SCOPE AUDIT — what is in ring 0 that should not be
**rmbp 9 · 2026-08-28 · tree `hw-rmbp @ 2980dbe8` · READ-ONLY, nothing cut**

Commissioned by Peter, 2026-08-28, expanding the CRISPY ruling (*"nothing from crispy should be in
the kernel its a theme"*) to the whole kernel. **This is an inventory with evidence, not a cut
list.** Every candidate below needs its own necessity check before anything moves; several will
have a real reason to stay, the way `pulsewin.rs` has one.

---

## 0. THE CRITERION, because "junk" is not a test
A thing belongs in ring 0 if it needs **privilege** (MMU, interrupts, DMA, ports, scheduling) or
if it **arbitrates a contended shared resource** between mutually distrusting users. Everything
else is a guest.

The sharp case, and the one that decides the vug question: **the vug detail ladder's ARBITER
belongs in the kernel; the crystal it draws does not.** Same subsystem, opposite verdicts. Only
the kernel knows the live renderer count, `wm::live_core_count`, panel cost, whether a steal just
happened, and (soon) SMC power state — that is resource arbitration, which is what a kernel is
for. The gradient on the shard is a guest.

Corollary worth stating: **"it is only reachable behind a knob" is not a defence.** Unconditional
`pub mod` means every shipped build carries it, and a module gated to an arch no leg builds is
worse than either.

---

## 1. HEADLINE NUMBERS
    kernel total          305 227 lines
      arch/               136 388   (45%)
      drivers/             63 990   (21%)
      video/               55 114   (18%)   ← of which wm.rs alone is 25 382
      top-level *.rs       31 326   (10%)
      fs/                  11 097
      wifi/                 3 538
      install/              2 611
      selfhost/             1 163

**Clear relocation/removal candidates identified so far: ~20 700 lines ≈ 6.8% of the kernel.**
That is ~5x the crispy-only figure (4 182 lines, 1.4%) that the original ruling targeted.

---

## 2. CATEGORY A — PRESENTATION IN RING 0, COMPILED UNCONDITIONALLY
Every one of these is `pub mod` with **no cfg gate**: they are in every build, both arches.

| file | lines | what it is (its own words) |
|---|---|---|
| `pal.rs` | 2 211 | the "Gneiss PAL" the in-kernel demos draw through |
| `ui_status.rs` | 1 286 | *"the always-on GUI status strip"* — hostname, IP, wall clock |
| `splash.rs` | 642 | *"the crystal-cluster boot splash"* |
| `vugras.rs` | 468 | vug rasteriser glue |
| **subtotal** | **4 607** | |

`splash.rs` is the clearest single case in the tree. Its own header argues aesthetics: v0's shape
*"read as too literal a Dark Side of the Moon quote and its fan was faint"*, and *"the RAYS are
the star"*. That is art direction, in ring 0, unconditional, on every boot. It is exactly the
class Peter's crispy ruling names, and it is not in crispy.

### video/ presentation, on top of the crispy six
| file | lines | note |
|---|---|---|
| `quarry/live.rs` + `quarry.rs` | 3 000 | **a file manager** (`userland: QUARRY — the file manager`) |
| `instgui.rs` | 692 | installer GUI |
| `png.rs` | 305 | PNG encoder — needed by PRTSCR, but an encoder is not privileged work |
| crispy six (`crystal` `pulsewin` `paper` `theme` `knurl` `ceramic`) | 4 182 | the original ruling's target |
| **subtotal** | **8 179** | |

---

## 3. CATEGORY B — DEAD OR DUPLICATED
| file | lines | finding |
|---|---|---|
| `vug.rs` | 1 304 | ~~compiled by NOTHING, in any leg, ever~~ **STRUCK 2026-08-28 — WRONG, and it was the load-bearing claim. Caught by pi 5, verified here.** The allowlist says "compiled by no **LEG**", which means NO CI LEG TYPE-CHECKS IT — not that it is unreachable. `arroyo:5335-5337`: `if [ -n "${UNAOS_VUGDEMO:-}" ]; then K8_FEATS="${K8_FEATS},vugdemo"` — **`UNAOS_VUGDEMO=1` puts it in a real flashable Pi image.** It is a knob's payload with a gate-coverage hole, NOT dead code. A coverage hole and an unreachable file justify very different actions; cutting on the second when only the first is true would have deleted a shipping knob's payload. Its only callers are `shell.rs` (`run_crystal`, `run_bebox_mode`, `run_pulse`).** And it is **duplicated**: `crates/user-vug` → `VUG.ELF` is the ring-3 renderer that actually ran six instances on flight 5. The kernel copy is the legacy path. |
| `rast_demo.rs` | 704 | `#[cfg(feature = "rast")]`, self-described *"RAST-1 demo: a spinning, flat-shaded, z-buffered cube"* |
| **subtotal** | **2 008** | |

**This is the strongest finding in the audit.** 1 304 lines that no build compiles, kept alive by
a shell command, shadowed by a working ring-3 implementation. It is not a migration question; it
is a deletion question.

---

## 4. CATEGORY C — THE BIG ONES, which are decomposition not eviction
| file | lines | question |
|---|---|---|
| `shell.rs` | 5 921 | An **in-kernel shell**, unconditional, both arches. Ring-3 userspace already exists (`handlers/`, `crates/user-*`). Does the shell need privilege, or is it in ring 0 because that is where it started? |
| `video/wm.rs` | 25 382 | **8.3% of the entire kernel in one file, 46% of `video/`.** Not evictable — it is the compositor, which genuinely arbitrates. But it is compositor + WCSER + wedge instrumentation + tripwires + the steal machinery in a single unit. This is where the *felt* bulk lives, and no crispy migration touches it. |

---

## 5. WHAT I HAVE NOT DONE — read this before acting
- **No necessity check per item.** `png.rs` may be needed in-kernel because PRTSCR writes from a
  privileged path; `ui_status.rs` may be the only thing that can read the clock pre-userspace.
  Each needs its own answer, and some will be "stays".
- **The cross-arch sample path is unsolved for `pulsewin.rs`.** Its header records WHY it is
  ring-0: one sampler, one renderer, and `SYS_CPUPULSE` is x86-only (ABI ledger D2). Moving the
  face to ring 3 without porting that starves it on aarch64. This is the known hard part.
- **`arch/` (136 kloc, 45%) and `drivers/` (64 kloc, 21%) are UNAUDITED.** They are the two
  largest regions and are presumptively legitimate (privilege by definition), but 200 kloc has not
  been looked at and the audit is not complete until it has.
- Crispy provenance markers reach **18 files** including `main.rs` and BOTH arch `syscall.rs`, so
  the crispy six are not a clean lift.

---

## 6. RANKED PLAN
1. **`vug.rs` + `rast_demo.rs` (2 008 lines) — delete or move, lowest risk in the tree.** One is
   compiled by no leg and duplicated by a working ring-3 binary; the other is a demo behind a knob.
   Closes an `arroyo` allowlist entry as a side effect.
2. **`splash.rs` (642) — the clean principle case.** Unconditional art direction in ring 0. Small,
   self-contained, and settles the precedent for everything after it.
3. **`pal.rs` + `vugras.rs` (2 679)** — follow the demos they exist to serve.
4. **The crispy six (4 182)** — Peter's original ruling. Blocked on the `SYS_CPUPULSE` port.
5. **`quarry` (3 000) + `instgui` (692)** — a file manager and an installer GUI; both look like
   ring-3 applications wearing kernel modules.
6. **`shell.rs` (5 921)** — needs the privilege question answered first.
7. **`wm.rs` (25 382)** — decomposition study, not eviction. The largest single lever on how the
   kernel *feels*, and entirely outside the crispy ruling.
8. **`arch/` and `drivers/` sweep** — 200 kloc unaudited.

---

## 7. THE NAMING QUESTION (Q1), which Peter deferred pending this pass
The scoping pass changes what needs naming, which is why it was deferred. First observation, not
yet a recommendation: **`wc` is the most overloaded token in the tree** — it is the window
compositor feature, the `wc-b`/`wc-d`/`wc-h`/`wc-w`/`wc-x`/`wc-fv` witness families, `wcg`, `wcf`,
`wcser`, and `WCDVALVE`, and it is one of the 22 single-leg features. If the compositor keeps the
arbitration role and sheds the presentation, the name should follow the mechanism that stays.
Recommendation deferred until items 1–3 above are decided, since they change the boundary.

---

## APPENDIX A — A SECOND DEFECT CLASS, found while running this audit
**Adjacent to the brief, not inside it** — this is not "code that should not be in the kernel", it
is "claims that should not be trusted". Recording it because it has a census and the audit kept
tripping over it.

**THE CLASS: an invariant asserted in prose, unenforced, restated by each seat that notices it is
wrong.** Three instances, all confirmed 2026-08-28, in three different files:

1. **`arroyo:2491`, the cfg census note — has now rotted FOUR times.** The block records its own
   disease in its own text: *"MEASURED on the SDHC-4b tree (2026-08-07), and the claim was stale"*
   … *"re-counted on the BCMA-S1 tree"* … *"and AGAIN on the BT-L0 tree, GR21"*. Three restatements
   visible in the prose; orin 10 found and corrected a fourth today. **Every fix has been to the
   NUMBER, never to the MECHANISM.**
2. **`arroyo:3156`, coverage-by-comment.** `# supstate + orinclick together — rides
   \`UNAOS_SUPSTATE=1 UNAOS_ORINCLICK=1 ./arroyo check\`, run at this arc's gate.` That is a
   MEASUREMENT wearing coverage's clothes: run once by hand, binding no later edit, re-running
   never — but it reads like a leg to anyone auditing by eye. (orin 10's find.)
3. **`video/mod.rs:27-31`, the panel-writer invariant.** *"`WRITER` and `fbcon` are handles to the
   same physical framebuffer; they are used at different times … each is serialised by its own
   lock."* Own-lock-each means mutual exclusion WITHIN a handle and none BETWEEN them. The only
   thing holding the invariant is the sentence. (orin 10's find; the basis of PANELOWN.)

**Why it belongs in a scope audit anyway:** every one of these is a place where the kernel is
carrying a *belief* instead of a *check*, and beliefs are the cheapest thing to leave in ring 0 and
the most expensive to be wrong about. The fix is the same shape in all three cases — make it a
check — and that is the same argument as §0's criterion, pointed at claims instead of code.

**Owed:** orin 10 is auditing `arroyo` for every other comment of this shape and will send the
table; the x86 entries are this seat's to act on.

---

## APPENDIX B — GRANTS OUT (recorded here so the audit's boundary is visible)
- **`video/mod.rs` → hw-jetson (PANELOWN).** Panel-owner word + witness-gated transition emit.
  Three binding conditions: store UNCONDITIONAL both arches with only the emit gated; the REFUSAL
  design comes to rmbp before it is written; line-neutral on the x86 present path (LOCKFIX).
  Landed as `63b86488` on their branch. Accepted cost: knob-off byte-identity for `fbcon.rs` is
  void by construction — a one-time re-baseline, judged cheaper than a permanent unpaired arch gate.
- **`fs/fat.rs` → hw-jetson (`write_grow` cost fix), 2026-08-28.** Chain-walk hoist only; no format
  change, no layout change, no change to what a write produces. ⚠ **FRGUARD lives in this file and
  is a live PROTECTION** — it fired on metal in flight 5 (`FRGUARD: SUBSTITUTION … the block layer
  refuses Default WRITEs here`). Together with SDHC-4c it is the closed-writable-set guard on the
  boot card; widening it is a STOP tripwire and Peter-only. Grant is conditional on not disturbing
  it. Verified before granting: this seat's unlanded arc does not touch `fs/` at all.
- `wm.rs` / `fbcon.rs` / `screen.rs` / `menubar.rs` → hw-jetson, aarch64 half, parity port
  (pre-existing, re-confirmed 2026-08-28). WCSER family excluded by line range.

---
---

# PART 2 — THE FULL-KERNEL SWEEP (5 read-only agents, 2026-08-28)
All five regions audited. **~200 kloc that Part 1 had not looked at is now covered.**

## THE HEADLINE THAT IS NOT A SCOPE FINDING — THE GMUX BLOCKER IS ANSWERED
Flight 5's `[GMUX] REFUSED … (status: 0x00000000)` was undiagnosable because `igpu.rs:1245`
**hardcodes the status word to 0**. But the four gate terms print on a separate line at `igpu.rs:1214`,
and that line was in the capture all along:

    :: igpu-dpy: pre-switch state DDC=0x02 SW_DISP=0x03 SW_EXT=0x01 DISP=0x03 EXT=0x21
                 sw_ext_state=UNACCEPTED ext_state=kepler-owned ::

Gate (`igpu.rs:1241-1244`), with constants resolved (`:260`,`:262`,`:264`,`:275`,`:296`):

| term | required | actual | |
|---|---|---|---|
| `p_ddc == GMUX_DDC_DIS` | `0x02` | `0x02` | **PASS** |
| `disp == GMUX_DISPLAY_DIS` | `0x03` | `0x03` | **PASS** |
| `ext_ok(p_ext)` | `{0x03, 0x21}` | **`0x01`** | **FAIL** |
| `ext_ok(ext)` | `{0x03, 0x21}` | `0x21` | **PASS** |

**Three of four terms pass. The single blocker is `GMUX_SWITCH_EXTERNAL = 0x01` — a FOURTH state the
code has no name for.** Known names: `IGD 0x02`, `DIS 0x03`, `KEPLER_OWNED 0x21`. `0x01` is none of
them, which is why `ext_state_name` prints `UNACCEPTED`.
⇒ The iGPU residency question is now: **what is `0x01`, and is it safe to admit?** That is a named
one-value question, not a dead end. It is the arc's most actionable open item.

**Second gmux finding — the ladder's `/10` is a fiction.** `igpu.rs:1495` prints `highest={:02}/10`,
but `highest` is only ever assigned `0..5` over a SIX-name axis (`harness`/`census`/`selftest`/
`switch`/`dpcd`/`edid`/`end`), while the ladder it cites (`docs/dev/GEMINI/video/iGUI/
LADDER-igpu-bringup.md:174-637`) has TWELVE headings (0–10 plus 8b). So `highest=05/10` means
"the experiment finished", NOT "halfway up the bring-up ladder" — and `RUNBOOK-gmux-igd.md:160`
encodes that confusion in its operator table. The denominator is a constant in a format string
backed by nothing.

## REGION TOTALS

| region | lines | CANDIDATE | share |
|---|---:|---:|---:|
| `arch/aarch64/` | 96 110 | ~22 350 | 23% |
| `arch/x86_64/` | 40 261 | ~13 170 | 33% |
| `drivers/ehci/` + `xhci/` | 36 410 | ~11 720 | 32% |
| `drivers/gpu/` + `drivers/*.rs` | 27 580 | 6 753 | 24% |
| `fs/` `wifi/` `install/` `selfhost/` `quarry` | 21 400 | 4 163 | 19% |
| video/ + top-level (Part 1) | 86 440 | ~20 715 | 24% |
| **TOTAL (quarry counted once)** | **305 227** | **~73 000** | **~24%** |

**CANDIDATE means "not obviously privileged or arbitration work" — it does NOT mean "delete".**
Several regions are active engineering (the GPU ladders), and several have real stated blockers.

## THE FIVE FINDINGS THAT MATTER MOST

### 1. `e1000.rs` ships an UNCONDITIONALLY ARMED network self-test
`nic.selftest_armed = true` (`e1000.rs:1143`) has **no `#[cfg]`, no feature, no knob**, and
`drivers/mod.rs:22` declares `pub mod e1000;` ungated. From `service_net` (`:1187`) it ARP-resolves
a gateway, ICMP-pings it, TCP-connects to port 7777 (`:751`) and UDP-sends to 9998 (`:778`), to a
hardcoded `GATEWAY_IP = [10,0,2,2]` (`:122`, the QEMU slirp default), with 150 ARP tries (`:141`).
**Every build. Every boot. Unsolicited traffic from ring 0.** This is the actual
benchmark-harness-in-the-kernel that `bench_ride.rs`'s name suggested and did not deliver —
`bench_ride` is triple-knob-gated and read-only; this is gated by nothing.

### 2. `arch/x86_64/syscall.rs` is 53% demo/fixture/witness — and the demos are NOT gated
~12 650 of 23 739 lines are fixtures, launchers and witnesses. **Genuine syscall dispatch is ~67
lines** of `match nr` arms (`:2579-2645`). The 6 554-line demo chain compiles into every x86 build:
`u7x_probe_once` (`:23721`) is `pub fn` with no `#[cfg]`, and chains twenty-plus launchers. Only the
ENTRY is gated (`main.rs:1287`). `sched.rs` repeats it exactly — a 520-line demo block with **zero**
`#[cfg(feature = "sched_demo")]`, gated only at the call site (`main.rs:975`).
Also inside: **a 568-line DNS server written in `global_asm!`** (SINKHOLE-1/zeolite, `:20679-21246`),
self-described as "the Pi-hole concept, done the UnaOS way" — a ring-3 program living in kernel
source while `crates/user-*` proves real ring-3 Rust apps already load from disk.

### 3. The fault-kill path dispatches on demo-fixture NAME STRINGS
`record_ring3_kill` (`syscall.rs:2321-2551`) is **26 `if name == "…"` comparisons** against fixture
names (`u2-tf-syscall`, `zeolite-resolver`, `sock4-grantee`, …). Exactly one arm is genuine.
**The #PF/#GP kill path — a privilege mechanism — is coupled by string equality to the test suite.**
That coupling is the migration's hardest seam: the fixtures cannot leave without the fault path
changing shape.

### 4. `drivers/ehci/` is 62% Bluetooth
12 062 of 19 477 lines are BT; the USB host controller it nominally is accounts for 7 415. About
1 580 of the BT lines are the legitimate USB transport binding (HCI-over-USB); the remaining
**~10 480 are protocol state machines above the transport** — SSP pairing (523), L2CAP signalling,
AVDTP discovery, ATT/GATT, BR/EDR inquiry and paging (`bt_c1_page` alone is 1 004).

### 5. `gen7.rs` — 5 557 lines producing ZERO functional blit
Every rung captures, restores and re-reads every write. The only export is `GtWake`, consumed only
by later `gen7` rungs; all seven call sites run BEFORE `bring_up_blt_ring`. Its own terminal verdict
(`:5537`) names the wiring as still-future work. **The live blit is 227 lines in `igpu.rs`
(`:825-1052`).** A ~900:1 ladder-to-shipped ratio. Active engineering, not dead — but it is
reconnaissance, not product.

## THE PROSE-INVARIANT CLASS — NOW 13 CONFIRMED INSTANCES, AND THE DOMINANT STRUCTURAL FINDING
Appendix A opened this with three. The sweep found ten more. **This is bigger than any single file.**

| # | site | the claim | the reality |
|---|---|---|---|
| 1 | `arroyo:2491` | cfg census counts | rotted **four times**; records its own rot in its own text |
| 2 | `arroyo:3156` | `supstate × orinclick` "rides `./arroyo check`" | a hand-run measurement that reads like build coverage |
| 3 | `video/mod.rs:27-31` | WRITER/fbcon "used at different times" | own-lock-each = no exclusion between them; nothing enforces it |
| 4 | `smmu_tegra.rs:39` | "READ-ONLY — every register touched is a READ" | writes SMR/S2CR **stream-mapping** regs ×7 + TLB invalidate. **File has ZERO `#[cfg]` — every build** |
| 5 | `pcie_probe.rs:41-48` | "no `write_volatile`, no link retrain" | `:1150` `write_volatile(APPL_CTRL)`; own log says `">>> FABRIC WRITE (M2)"`; contradicts itself at `:839` |
| 6 | `quarry/live.rs:21-27` | "EL0 window hard-capped 128x128 … 16 columns" | all three definitions read **288** (`FB_WIN_SLOT_SIZE` grew 5x). 36 columns. Blocker void |
| 7 | `fs/vfs.rs:22-24` | "deliberately UNCONSUMED this arc" | ~20 call sites in `shell.rs` incl. tab-completion `:5263` |
| 8 | `igpu.rs:1495` | `highest=NN/10` | numerator maxes at 5 on a 6-name axis; cited ladder has 12 headings |
| 9 | `xhci/mod.rs:571` | `.lock()` in only 3 fns, "the compiler enforces it" | **four** sites; `vugras_dump:766` also breaks the masked-O(1) property the same comment relies on |
| 10 | `syscall.rs:17191` | "x86 has no ring-3 ELF loader; `run`/`bg` are baremetal-only" | `elf.rs` IS one; neither fn is cfg'd; the two files contradict each other |
| 11 | `emmc2.rs:168` | "invariant, checkable by grep" | currently TRUE, but held by review habit, not the compiler |
| 12 | `bench_ride.rs:131` | per-fn "no config or MMIO writes" | `:155` calls `map_mmio_window` (a page-table write). File-level claim is the accurate one |
| 13 | `e1000.rs:1139-1143` | bounded scope | gating does not deliver it (finding 1) |

**The counter-examples matter as much** — the claim IS keepable when someone keeps it:
`ga10b_probe.rs:26-28` ("ZERO MMIO WRITES … there must not be") is TRUE, its only `write_volatile`
occurrence being that comment's own text. `smp.rs:340` says "It is a SNAPSHOT, not a verdict, **and
says so**". `block.rs:255` states "It is NOT closed by construction" rather than pretending.
`sdhc4c.rs:30-38` **documents its own earlier overclaim** and names the write that escapes the
permit (`drivers/sdhc.rs:3165`, LBA 60799, every armed boot). `xhci/mod.rs:2269` retracts an
overstated correlation *in the source* because "the overstated version was carried to the bench".

⇒ **The fix is never a better sentence. It is a check.** And the tree already knows this — it is
what `arroyo`'s own KNOB→LEG check did for the census note that rotted three times before it.

## CORRECTIONS TO PART 1 AND TO THE SEAT'S OWN BRIEFS
- **`handlers/` is NOT UnaOS ring-3.** It is a separate root workspace of **GTK4 Linux host
  applications** (`gtk4 = "0.10.3"`, `glib`, `gl`, `epoxy`), self-described as "Design-stage / early
  prototype". CLAUDE.md's "Ring 3 host-native userspace" means *host-native* — it runs on Linux, not
  on UnaOS. **The real UnaOS ring-3 line is `unaos/crates/user-{vug,stat,pulse,elf,blob,blob-x86}`.**
  So "move it to ring 3" means `crates/user-*`, and quarry duplicates nothing.
- **`FRGUARD` is NOT implemented in `fs/fat.rs`** — it is `drivers::block::default_writable`
  (`block.rs:877`, arms at `:1066`/`:1089`). `fat.rs` only CONSUMES it (`:669`,`:707`,`:710`).
  Full radius also touches `fs/vfs.rs:500-502`, `flight_recorder.rs:332,578,589,598`, `main.rs:255`,
  `wifi/firmware.rs:626`, `video/prtscr.rs:67,76,333,463`.
- **`unafs` is not a kernel cargo feature at all** — it is a path dependency (`Cargo.toml:2454`);
  the module is gated on ARCH (`fs/mod.rs:35`). **The baton's queued "unafs module-gate collapse
  (~37 gates)" describes something that does not exist in this tree.**
- **`overlay_mode` is NOT hardcoded** — it is runtime self-adaptation, flipped on the first chain
  HSE and permanent thereafter (`ehci/mod.rs:742-745`). The qTD chain path is live code QEMU exercises.
- **`bench_ride.rs` is not a benchmark** — "BENCH" is the workbench/sitting. Read-only, triple-gated.
- **`arch/aarch64` is not bloated.** Its 96 kloc vs x86's 40 kloc is a DIRECTORY CONVENTION: aarch64
  keeps device drivers inside `arch/`, x86 keeps them in `drivers/`. That is 50 600 lines, 53% of the
  region. Board-neutral cores are near-identical (38 729 vs 40 261) and aarch64's `syscall.rs` is
  SMALLER than x86's (23 308 vs 23 739).

## DEAD CODE — A HONEST NEGATIVE
**`vug.rs` remains the ONLY module-scale dead code in the kernel.** All five agents cross-checked
their regions' `#[cfg]` gates against every leg of `KERNEL_CFG_MATRIX`:
- `arch/aarch64`: 36 module decls vs 28 legs — all mapped. Dead = 11 cfg sites (the three allowlisted
  `v3d_*` knobs), 0 modules.
- `arch/x86_64`: 25 features checked — 0 holes. `rtpi`/`rtwit` ride derived mix legs; `baremetal`
  appears only in two stale comments.
- USB: 0 holes. GPU/drivers: `nvidia-kepler-kdisp-hold` is board-unnamed but compiled via mix legs.
- `fs/` etc.: `selfhost` board-unnamed but classified NOT-A-HOLE at `arroyo:2540`.
⇒ ~~The `vug.rs` cut stands as ranked-first~~ **STRUCK — see the DECRUD-1 ruling below. There is NO module-scale dead code in this kernel. The honest negative is stronger than stated: all five regions found zero unreachable modules, and the one candidate turned out to be reachable too.**

## ⚠ THE RANKED-FIRST CUT IS WITHDRAWN — IT WAS ALREADY ADJUDICATED, AND NOT AS A CLEANUP
`vug.rs` is not a lane question and never was. **`docs/dev/OS/kernel-decrud.md` §3.1 item 4** records
that an earlier arc (DECRUD-1) moved it behind the knob, default OFF, having **explicitly considered
and DECLINED deletion**, verbatim:

> *"Gated rather than deleted, because deletion also retires three shell verbs and ~14 documents
> reference them. **That is a product call, not a cleanup call.**"*

So the options this seat put to pi 5 and orin 10 — "I take it / you take it / it waits" — were all
lane moves on a question the tree had already ruled is **Peter's**, not any seat's. Two seats cannot
grant each other what neither owns. Cutting it means retiring the `vug` / `vug wire` / `vug bebox` /
`pulse` shell verbs and re-pointing ~14 documents. **That goes to Peter as a product question or it
does not happen.**

**A trap for whoever eventually does cut it** (pi 5's find, from §3.1's last paragraph): `classify_load`
and `parked_display_witness` were deliberately moved OUT to `ui_status.rs` FIRST, because
`parked_display_witness` is a metal-earned falsifier for the VUG-HONESTY rule and *"leaving it inside
a demo module would have made a live witness hostage to a demo's knob."* `arch/aarch64/sched.rs:7509`
repoints there. Confirm nothing has drifted back in before deleting.

**Doc defect found in passing** (pi 5): §3.1 item 4 says *"See §4.7"* and **there is no §4.7** — in
fact `grep '^### 4\.'` returns nothing at all in that document. Dangling cross-reference.
