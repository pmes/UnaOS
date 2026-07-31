# WHITE BOARD (WB)

**This is a whiteboard, not a record.** It is wiped and rewritten every round. Nothing here
is durable — the durable copies live in commit messages, `docs/dev/`, the track memory, and
the coordinator baton. If you are reading this in a later round, assume it is stale.

Three sections, three sessions: **kepler**, **igpu**, **GR12**. Each is a complete paste —
you do not need to go and read anything else first.

> There is a **second** whiteboard: `~/unaos-bench/PLAYBOOK-x86.md`, which Peter and the x86
> seat share. That one carries **only what Peter physically does at the bench**, plus an
> open-items table. It is delivered with `SendUserFile`, `display: "render"`, every single
> time it is touched. A path or a markdown link is not delivery.

---

## → kepler

You are the **kepler** lane on UnaOS. Fresh session — everything you need is here.

**Your worktree:** `~/src/github.com/pmes/UnaOS-gemini-kepler` — already created, already on
branch `wt/kepler-poke-x86`, already at trunk tip. **Do not create a worktree, do not rebase
onto anything else, do not touch any other tree.**

**Your file:** `unaos/crates/kernel/src/drivers/gpu/kepler.rs`. If a fix needs a file outside
it — `main.rs`, `shell.rs`, `arroyo`, `builder/`, `interrupts.rs` — **stop and say so**. Those
belong to the integrator; a two-line seam is cheap to hand over and expensive to collide on.

**The rig is Linux.** Everything runs on the Fedora host:
`flatpak-spawn --host bash -c 'export PATH=$HOME/.cargo/bin:$PATH; cd <tree>/unaos && …'`

**Your gate** — exactly this, and check the `⚡ kernel features:` banner really lists
`nvidia-kepler` before you believe any result:

```
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check
```

Bare `./arroyo check` does not compile `kepler.rs` at all — a green without those knobs is green
about a build with none of your work in it. Never run a QEMU suite; metal is the verdict. Read
serial captures with `awk`, never `grep` (control bytes break grep).

**Five rules — these are why the last session's round was thrown away:**

1. **Commit your work.** The previous session left its whole arc uncommitted while its HEAD
   still carried the defect the arc existed to remove. Uncommitted work does not exist.
2. **Your artifacts are committed files in the repo** — paths below. Anything living only in a
   session-private scratch directory is invisible and treated as not having happened.
3. **The gate must carry the knobs that compile your code.**
4. **Verify with a tool, not with your comments.** Four malformed falcon instructions survived
   three review rounds because the assertions checked immediates and the comments read right.
5. **State what you did not do.** A commit titled after a fix it does not contain costs more
   than no commit.

| Artifact | Path in your tree |
|---|---|
| Plan, before you write code | `docs/dev/GEMINI/video/Kepler/PROPOSAL-kepler-poke-terminal.md` |
| Walkthrough, after | `docs/dev/GEMINI/video/Kepler/WALKTHROUGH-kepler-poke-terminal.md` |
| Found but not fixed | `docs/dev/GEMINI/video/Kepler/FINDINGS-kepler-poke-terminal.md` |

**THE GOAL.** `ECHO_A_BYTES` reads `0x409504` from inside a microcode image that runs
**mid-sequence**, and that read poisons the FECS unit for the rest of the boot. Split it:

- **`ECHO_A_BYTES`** — the falcon-execution test. Command in, ACK out, phase stamps. **No `$r8`
  setup, no `iord I[$r8]`, no `0x409504` in any form.**
- **`POKE_A_BYTES`** — a second image carrying the `$r8 = falcon_io(0x504)` setup and the read,
  executed **once, at the terminal phase**, immediately before the host's existing terminal
  `fecs_write(bar0, 0x409504, 0)`.

**WHY — the poison law, from your own spec** (`docs/dev/OS/08_VIDEO/falcon_microcode_spec.md`):

```
:68   | +0x504 | WRCMD_CMD | ⛔ FAULTS — poisons the unit | s31, s32, s34
:253  The first access to 0x409504 (WRCMD_CMD) faults immediately and wedges
      every subsequent read in the FECS unit for the rest of the boot
:537  §5.4 established the poison law and pull 28 turned it into a standing ban
:535  ## 10. The terminal poke — 0x409504, once, last
```

Reading it falcon-side, where the host side faults, is a legitimate experiment. It must simply
be the **last** thing the kepler leg does.

**MOST OF THIS IS ALREADY BUILT — start from it, do not redo it.**
`docs/dev/GEMINI/salvage/kepler-echo-poke-split.patch` is the previous session's split, verified
correct at the byte level and then abandoned uncommitted. Its arrays are good: envydis shows
**zero** unknown instructions in either image; all six branch displacements resolve by
arithmetic (ECHO `0x34+0x2b=0x5f`, `0x3a+0x14=0x4e`, `0x54−0x2f=0x25`; POKE `0x3b+0x2e=0x69`,
`0x41+0x17=0x58`, `0x5e−0x32=0x2c`); `$r8 = 0x14100 = falcon_io(0x504)`; and assertion coverage
has **zero unpinned holes** across all 128 bytes of both arrays. Apply it, keep the arrays, fix
what follows — and re-verify with envydis yourself rather than taking any of that on trust.

**⛔ WHAT IS BROKEN IN THAT PATCH:**

**1. The POKE block addresses raw BAR0 with no `0x409000` FECS base.** Every access — `0x104`,
`0x180`, `0x184`, `0x804`, `0x800`, `0x100`, `0x044`, `0x040` — lands in the **PMC master-control
block**, not the FECS falcon: ~66 wild MMIO writes into the GPU's master control at boot, and
because they bypass `fecs_read`/`fecs_write` the FECS access ledger never counts them. The ECHO
block does the identical sequence correctly with `let base = 0x409000;`. Use
`fecs_write(bar0, base + …)` throughout.

**2. CPUCTL and BOOTVEC are swapped.** POKE treats `0x104` as CPUCTL and `0x100` as BOOTVEC. The
map is **CPUCTL = `0x100`, BOOTVEC = `0x104`**, as ECHO has it on the s37-metal-proven path. As
written, POKE writes START_TRIGGER into the wrong register.

**3. The upload omits the IMEMT tag and the page padding.** ECHO writes `0x188` (tag = 0) and
pads to `IMEM_PAGE_WORDS`; POKE writes only IMEMC/IMEMD for its 32 words. Your own comment states
the rule: the code TLB marks a page usable only when the last word of the 0x40-word page is
written.

Any one of those three alone prevents the falcon from executing.

**4. The verdict is too wide.** `if ack != MB_SEED` with only `0xBADF0000` carved out means
`0xFFFFFFFF` (bus float), `0xBAD0BA20` and `0x00000000` all print SUCCESS. This file already
classifies all three correctly — `kepler.rs:697` treats them as ABSENT, `:1129` uses
`(x >> 16) == 0xBADF || (x >> 16) == 0xBAD0`. Reuse that predicate. **A poison read must print
POISON, never SUCCESS.**

**5. `FECS_504_READ_TOUCHED` must be set on the falcon-side read.** The ledger only watches host
`fecs_read`/`fecs_write`, so a falcon `iord` is invisible to it and it will report
`504_read_touched=false` on exactly the boot where the falcon touched it first. A watcher that
certifies the event it cannot see is worse than no watcher.

**6. Restore the three guards the patch deleted** — all stronger than what replaced them:
arithmetic branch assertions (`0x3b + slice3(..)[2] as i8 == 0x69`) rather than byte literals with
the target in a comment; `IO_MAILBOX0/1` and `IO_CC_SCRATCH0/1` derived via `falcon_io()`, which
existed specifically to catch a raw-host-offset listing — exactly what defect 1 is; and delete the
second `falcon_io` the patch adds inside `mod ucode`, which **shadows** `regs::falcon_io` and drops
the `& 0xffc` mask.

**7. Smaller, all real.** `mailbox0={:08X}` prints `ack`, which is CC_SCRATCH[1], not MAILBOX0. The
POKE host poll spins to `ECHO_BOUND` (1,048,576 MMIO reads, ~1 s of boot) with no `spin_loop`,
reusing a falcon instruction bound as a host read count. `#[rustfmt::skip]` was dropped from both
arrays, so `cargo fmt` would destroy the one-instruction-per-line layout the review method depends
on. And `mod tests` does not compile — it calls `pack92` (only `pack128` exists) and asserts POKE
offsets against `ECHO_A_BYTES`; make it compile and get it into a gate, or delete it.

**VERIFICATION — the bar, not a suggestion.** Build `envydis` from the in-repo `envytools` (cmake +
gcc; you may need `-DCMAKE_POLICY_VERSION_MINIMUM=3.5`), extract **both** byte arrays mechanically
from the source, disassemble each as falcon v4, and put both full listings in your walkthrough.
Zero unknown instructions in each executable region, every branch displacement resolved by
arithmetic, every port immediate derived from `falcon_io()`.

**OUT OF SCOPE — record it, do not touch it.** The runlist submitted with `LEN=3` contains beacon
words: `kepler.rs` writes 3 entries, then overwrites `runlist_off+0..+31` with
`0xBEAC0001..0xBEAC0008`, and nothing restores it before the submit. Separate arc — put it in your
FINDINGS file.

**DONE GATE.** Plan committed before you write code · fixes 1–7 · both envydis listings in your
walkthrough · the gate above green on **both** arches · everything committed on
`wt/kepler-poke-x86` · a commit body that says plainly what you did not do.

## → igpu

You are the **igpu** lane on UnaOS. Fresh session — everything you need is here.

**Your worktree:** `~/src/github.com/pmes/UnaOS-gemini-igpu` — already created, already on branch
`wt/gmux-igd-x86`, already at trunk tip. **Do not create a worktree, do not rebase onto anything
else, do not touch any other tree.**

**Your file:** `unaos/crates/kernel/src/drivers/gpu/igpu.rs`. If a fix needs `main.rs`, `shell.rs`,
`interrupts.rs`, `arroyo` or `builder/` — **stop and say so.** The last session wrote into two of
those after being asked twice not to.

**The rig is Linux.** Everything runs on the Fedora host:
`flatpak-spawn --host bash -c 'export PATH=$HOME/.cargo/bin:$PATH; cd <tree>/unaos && …'`

**Your gate** — run it **both** ways, and confirm the armed banner ends `…,unaos_ivb,gmux_igd`:

```
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 UNAOS_GMUX_IGD=1 ./arroyo check
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check
```

Both green, and the knob-off build behaviourally identical to trunk. The knob is already wired in
all three required places — `Cargo.toml`, `arroyo`, `builder/src/main.rs`. Never run a QEMU suite;
metal is the verdict. Read serial captures with `awk`, never `grep`.

**Five rules — these are why the last session's round was thrown away:**

1. **Commit your work.** Uncommitted work does not exist.
2. **Your artifacts are committed files in the repo** — paths below.
3. **The gate must carry the knobs that compile your code.** Every green this pull ever produced
   type-checked only the disarmed build, because the feature reached `Cargo.toml` and nothing else.
4. **Verify with a tool, not with your comments.**
5. **Never title a commit after a fix it does not contain.** The last one was called "Deferred
   GMUX IGD Switch" and deferred nothing — its diff was seven lines of shell verb.

| Artifact | Path in your tree |
|---|---|
| Plan, before you write code | `docs/dev/GEMINI/video/iGUI/PROPOSAL-igpu-gmux-igd.md` |
| Walkthrough, after | `docs/dev/GEMINI/video/iGUI/WALKTHROUGH-igpu-gmux-igd.md` |
| Operator procedure for a black panel | `docs/dev/GEMINI/video/iGUI/RUNBOOK-gmux-igd.md` |
| Found but not fixed | `docs/dev/GEMINI/video/iGUI/FINDINGS-igpu-gmux-igd.md` |

**THE GOAL.** Point the display mux at the integrated GPU, prove the write landed, and **get back**
without human intervention.

**The panel WILL go black, and that is not the experiment failing.** Your own census shows every
iGPU pipe, plane and PLL reading zero, so nothing is driving the panel from that side. **The
deliverable is the read-back proving the mux write landed** — which a future round needs before it
configures pipes. Say that in your proposal so the result is not read as a defeat.

**WHAT IS ALREADY RIGHT — keep it, do not rewrite it.** These were hard-won over three rounds:

- **The ISR hook is trivial and must stay trivial.** `gmux_tick()` loads state, unpacks, compares a
  deadline, sets a flag, stores. **No port I/O, no loop, no blocking wait.** It runs at 1 kHz on an
  interrupt gate with IF=0, before `eoi()`, on the only core that advances the global ms clock —
  anything that blocks there stalls the clock it depends on.
- **`RevertState` pack/unpack.** One encode/decode point, every mutation routed through it. This is
  what stopped a saved byte being lost to a mask.
- **The `0xFFFFFFFF` timeout sentinel refuses to arm.** A pre-switch read that timed out means
  there is no known state to return to.
- **Port constants and write order match upstream** (`0x7C2` value, `0x7D0` read-index, `0x7D4`
  write-index/status; DDC `0x28` → DISPLAY `0x10` → EXTERNAL `0x40`), including the `wait_ready()`
  between the value and index writes. An earlier relay twice told you to remove that wait; **that
  instruction was wrong and is retracted** — upstream `apple-gmux.c` has it exactly there. Keep it
  and cite your reference in a comment so it is not re-raised.

**⛔ WHAT MUST CHANGE:**

**1. The switch must not arm where its revert cannot run.** Today the arm fires from `igpu::init()`
inside `pci::init`, while the only revert executor — `gmux_task_tick()` — lives in `x86_usb_pump`,
spawned ~350 lines of boot later and only when *(not `rast`)* and *(framebuffer non-zero)* and
*(two distinct APs online)*. Three live paths end with the mux switched and no revert, permanently,
until power cycle: the inline-BSP fallback where the pump never spawns; `rast` builds; and any wedge
between the arm and the pump's first pass — SDHC, storage, SMP, xHCI enumeration and the GUI handoff
all sit in that gap. **Arm from a context whose revert driver is provably live, or refuse to arm.**

**2. The manual trigger must complete the revert itself.** `gmux-revert` sets `state.due = true` and
returns; every port write is in `gmux_task_tick()`. On exactly the paths where the automatic revert
is already dead, the operator types the verb, sees *"Manual GMUX revert triggered (if armed)"*, and
nothing moves. **A recovery path that reports success while doing nothing is the worst failure mode
available here** — it will be typed blind at a black panel by someone who then believes it worked.
This rig's serial console is **kernel-TX-only**, so there is no typing over the wire: everything
from EHCI-HID through `handle_key` must still be alive with the mux switched away. Verify that and
state it in the RUNBOOK.

**3. Refuse to arm on an unproven protocol.** `boot_ver_ok`/`kern_ver_ok` are computed and their
if/else closes at `igpu.rs:410`; the arm block opens at `:413`, outside both branches. It arms
whether the driver printed `PROTOCOL PROVEN` or `PROTOCOL UNPROVEN`. A gmux that answers the
handshake but reports an implausible version passes the `0xFFFFFFFF` sentinel and gets its display
mux written anyway.

**4. Leave the knob-off build identical to trunk.** `gmux_wait_ready`/`gmux_wait_complete`/
`gmux_index_read` are gated on `target_arch` only, **not** on `feature = "gmux_igd"`, and
`read_gmux_trace()` calls them on every `unaos_ivb` build. You changed the timeout from a bounded
iteration count to an `arch::ms()` deadline — and `ms()` only advances if the BSP timer ISR is
running. **The old bound could not hang; the new one can.** Gate the helpers behind the feature, or
keep an unconditional iteration cap alongside the deadline.

**5. A failed write must not be treated as a switch.** `gmux_index_write` failures are logged and
ignored: a timed-out DDC write with a landed DISPLAY write leaves the panel on IGD with DDC on
discrete, and the code prints `Revert Complete` regardless. **Compare the read-back against the
intended values** and say on the wire whether it matched. Without that, a black screen cannot be
distinguished from a write that never happened.

**6. `REVERT_STATE` is read-modify-written from three contexts** — the BSP timer ISR, the pump on an
AP, and the shell. `SeqCst` on the individual load and store does not make the sequence atomic; two
contexts can interleave and re-run a revert. Use a compare-exchange loop.

**SINGLE-USE MEDIA — put this in the RUNBOOK.** Nothing guards the knob across boots. `PROBED` only
prevents re-entry within one boot, so **every subsequent boot from that stick switches the mux
again.** The stick must be re-flashed after the sitting.

**DONE GATE.** Plan committed before code · items 1–6 · both gates green on both arches with the
banner checked · a RUNBOOK an operator can follow at a black panel · everything committed on
`wt/gmux-igd-x86` · a commit body that says plainly what you did not do.

---

## → GR12

You are the **x86 seat and Gemini coordinator** on the `UnaOS-gemini` branch, picking up from
GR11 (2026-07-30, late). Direction comes only from Peter, in your chat. Fox is the Pi seat —
facts and diffs only; nothing is owed to them at pickup.

### READ FIRST, in this order, before you touch anything

1. **`~/.claude/projects/-home-pmes-src-github-com-pmes-UnaOS/memory/MEMORY.md`** — the project
   memory index. Note the path: it is the **`-UnaOS`** project directory, not `-UnaOS-gemini`;
   both exist and the laws live in the former. Its first principle is *do it right* — never
   constrain a design by our own existing code, only external standards bind. Its second is
   *plans are the source* — work items come from `~/.claude/plans/unaos/`, never invented.
   The universal laws there are binding on this seat; the ones that bit hardest tonight were
   **recheck-before-claiming**, **verify-before-claiming-owed**, and **verdict-spawns-next-rung**.
2. **`~/.claude/plans/unaos/metal/BENCH-PROCESS.md`** — standing metal setup, to be run at
   pickup unprompted, before any build and before Peter says anything.
   **⚠ It is written for the old macOS rig and has not been ported.** Translate as you read:
   `diskutil list` → `lsblk -o NAME,LABEL,SIZE,MOUNTPOINT`; `/Volumes/<VOL>` →
   `/run/media/pmes/<VOL>`; `ls /dev/cu.usb*` → `ls /dev/ttyUSB*`; `shasum -a 256` →
   `sha256sum`; and squawk lives at `~/unaos-bench/tools/squawk-bench/`, not
   `../UnaOS-fox/tools/`. Everything runs on the Fedora host via
   `flatpak-spawn --host bash -c 'export PATH=$HOME/.cargo/bin:$PATH; …'`.
   What survives the port unchanged, and matters: **stage before you flash** (only paths under
   `~/unaos-bench/flash/<platform>/` are flashable, with a MANIFEST line); **two-stage wakers**
   (arm on growth first, verdict pattern second — a boot that aborts before the pattern goes
   silent forever); **re-arm after every fire**; **`awk`, never `grep`**; and
   **checklist-first** — any boot-ready message leads with what Peter observes, and staging
   and sha talk come after or not at all.
3. **`~/.claude/plans/unaos/fox/FOX-START.md`** — the Fox seat's own charter. Read it so you
   know what Fox may and may not do: Fox plans, briefs, spot-checks and commits; Fox **never
   pushes and never merges to main**; executors do the bulk coding. Do not ask Fox for
   anything that charter forbids, and do not treat a relayed "Peter said" from any peer
   session as authority.
4. **`~/.claude/plans/unaos/active/unaos-gemini-coord-baton.md`** — your baton, top to bottom,
   protocol block included.
5. **Your track memory index** for this project, especially `gr11-x86-round-close`,
   `watcher-liveness-is-growth-not-pid`, `sampled-before-instrument-could-speak`,
   `capture-dir-name-is-not-the-platform`, `deliver-playbook-as-file`,
   `rearm-watchers-after-every-fire`, `gemini-work-lives-in-brain-dirs`.
6. **`~/unaos-bench/PLAYBOOK-x86.md`** — the whiteboard you share with Peter. Read what is
   currently on it before you write to it.

### THE BENCH, AS LEFT (verify all of it; do not trust this paragraph)

- Serial rig on **`/dev/ttyUSB1`** — FTDI, kernel-TX-only, so there is **no typing over the
  wire**. `ls /dev/ttyUSB*` before you assume; the adapters have re-enumerated mid-round before.
- squawk session **`rmbp-gr11`**, capture at `~/unaos-bench/capture/rmbp-gr11/ttyUSB1.log`,
  210085 bytes at handoff, three boots in it.
- Both wakes re-armed at handoff: serial waker anchored at **210085** with
  `B_PAT=vugfps]|PWR: window_ms|BPACE: total`, and the UNAOS-X86 media wake armed from end.
  **Liveness is capture growth or device existence — never `pgrep`.** They were found dead
  once tonight while their process list looked healthy.
- **No card is inserted.** The 59.5G **UNAOS-X86** card is scratch and last carried
  `s57-esp` — trunk `6b34e1f7` plus `UNAOS_WC=1` (window compositor). The staged trees are
  `~/unaos-bench/flash/rmbp/s56-esp` (no WC) and `s57-esp` (WC), both with MANIFEST lines.
  **An SD card is in the built-in slot — leave it, SDHC needs it.** Boot media is the USB
  reader, never the built-in slot. The 1.9G **UNAOS-DATA** card is forensic evidence, never
  written. UNAOS-PI cards are Fox's.

### WHAT LANDED THIS ROUND — all five pushed, `origin` == tip `0913b91e`

| | |
|---|---|
| `ee6bfd97` | deleted the in-kernel vug (1390 lines); meters moved to `ui_status.rs`, `vug::init` to `video::init_panel` |
| `a571254f` | **SCHED-X86** — the BSP joins the scheduler, mirroring the Pi: `input`/`render`/`usb-pump` pinned tasks, then `run_bsp(0)`; `ensure_pat_wc()` added to `ap_entry` |
| `6e7b64d6` | SMC power instrument — `refresh_if_due()` restored to a path a GUI boot reaches; `NO-WINDOW (dropped)` made reachable |
| `ce5c6f49` | wired `UNAOS_GMUX_IGD` into `arroyo` + `builder` |
| `0913b91e` | restarted both Gemini lanes on clean trees with in-repo artifacts |

**None of it has been near metal.** The last boot was 18:36; four kernel commits landed after
it. That is the single most important fact in this handoff.

### PROVEN ON METAL THIS ROUND (do not re-derive)

- **The 7.3 s mystery is solved: `ehci-hid-done d=6324ms`** — the USB-2 HID bring-up is 93% of
  it. Every candidate the old baton named is refuted: `xhci-handoff` 0 ms, `xhci-halt` 0 ms,
  `xhci-hcrst` 1 ms, `xhci-cnr` 0 ms. **That is the next arc.**
- **CCSMARGIN, first ever reading:** `first_assert=21ms latest=21 margin_ms=129` at
  `settle_ms=150`. The trim is safe with 7× headroom and could go lower.
- **`enum:p2-done` 2780 ms → 111 ms.** The old 2.8 s port stall died with the settle trim.
- **Console 12.3 s / desktop 12.0 s / full boot 17.7 s** with all four lanes compiled in;
  8.0 s is the same kernel with the GPU drivers left out. Always state which build.
- **SMC alive and metering** — full key sweep, `BRSC=0x58`, and `:: PWR: ::` rollups every
  ~10 s at ~19.3 W once something drives the sampler. `ac_derived: inferred, no hardware key`
  — there is no AC-present key, so "plugged" is a guess and the unplugged half has never
  been tested.
- **Kepler falcon EXECUTED uploaded microcode**: `ucode EXECUTED img=A mailbox0=F00DFACE`,
  `halt-iters=0`. Transport, power state, IMEM/DMEM ports and mailbox are all proven working.
- **GMUX answers**: version 3.2.19, `SW_DISPLAY=0x03 (DIS)`, `SW_DDC=0x02 (DIS)`,
  `DISC_POWER=0x03 (ON)`. The discrete GPU owns the panel — that explains every zero in the
  iGPU teardown table. `DP_A=0x0000001C` is the one live register.

### THE THEME OF THIS ROUND, AND THE THING TO CARRY FORWARD

Almost nothing tonight was a bug in the OS. It was instruments lying:

`[vugfps]` printing `us` on raw TSC cycles · a capture directory named for the wrong rig ·
me reading a five-minute rollup at ninety seconds and calling the silence a defect ·
`NO-WINDOW (dropped)` unreachable · kepler's `$r8` never loaded so its "reading" was INTR_SET ·
kepler's `ack == 1` verdict unreachable · the `FECS_504` ledger unable to see a falcon-side
read · and **the gate itself never compiling the armed GMUX code for three rounds**.

Ask of every instrument, every time: **can this be wrong in a way that still looks right?**
And before calling a missing witness a defect, find its emission period and compare it to how
long the capture has been running.

### OPEN, IN PRIORITY ORDER

1. **A boot carrying SCHED-X86.** It changes how the machine boots, so the failure mode is a
   machine that does not come up. The witness chain that proves it: `:: SCHED-X86: RENDER on
   core N + INPUT/usb-pump on core M …`, then `:: SCHED-X86: BSP entered run loop cpu=0 ::`,
   then one dispatch line per task, then `[schedx86] depth sent/recv/inflight`. The spawn line
   **without** the run-loop line is the falsifying case.
2. **The unplugged boot** — still never captured. Needs `vug`… except the in-kernel `vug` is
   now deleted, so **find what drives `refresh_if_due()` on an idle desktop and confirm it**
   before asking Peter for the trip. He has already spent one on my bad instruction.
3. **The 6.3 s EHCI-HID arc** — the named next target.
4. Both Gemini lanes, on their clean trees (sections above).
5. Flagged, not fixed: witness builds place a cooperative ring-3 task (IF=0) on
   `online_aps().first()`, which SCHED-X86 made the render core.
6. Known and untouched: the runlist submitted with `LEN=3` contains beacon words.

### HOW PETER WANTS TO BE WORKED WITH

Act, do not ask — he has said so repeatedly and was angry about it tonight. Do not sit waiting
to be told a worker finished; poll them, and remember their real work is in
`~/.gemini/antigravity/brain/<uuid>/`, never judged from a diffstat. When a card wake fires,
**write the card in that same turn**, before the analysis. Re-arm every watcher the instant it
fires. Send the playbook and this whiteboard with `SendUserFile`, render mode, unprompted,
every time you touch them. Keep replies short — verdict and what you need from him; reasoning
belongs in the doc and the commit message. Null hypothesis is always our code; firmware
baseline is the burden of proof.
