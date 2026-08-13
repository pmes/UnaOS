# MERGE-R23 — pi4 + x86 trunk unification (landed 2026-08-03, trunk 47f955a3)

Candidate `bfa2c174` (`merge-assembly`), gated: `UNAOS_WC=1 ./arroyo check` green both arches;
91/91 pi4-regression by mbench replay; metal boots of the exact tree on both rigs — pi4 boot5
(capture `pi4-r23s1x`: CAPSTONE COMPLETE, `[wedge9]` QUIET, 0 CRIT) and rMBP (capture
`rmbp-s66-cand444`: BOT 100 pumps zero anomaly counters, compositor executed at 2880x1800,
0 CRIT). Co-owned by the pi4 (fox) and x86 (GR) seats; the x86 half of this doc may be
extended by that seat.

## What the merge is
- **Baseline: the pi video stack** (wm.rs, screen.rs, cursor/sprite, compositor). Proven
  executing on x86 silicon at first boot.
- **x86 lineage kept**: BOT-CBW/xhci claim-loan architecture (`cbw=always-awaited` shipped),
  FAT/block/serial_ring/pal, flight-recorder UNAOS.LOG reservation.
- **F1–F4 masked-spinner family complete** (WEDGE-7/8/9/10 + MBOX-1): one discipline, two
  idioms — see `07_USB_STORAGE/xhci_concurrency.md`.
- U11 fixtures are measure-don't-predict: chain head captured via `find_located` at the
  A_OPENED edge, never predicted from a pre-spawn first-fit snapshot.
- **Stripped at merge**: the two orphaned x86-wc call sites (`wm::present_rows`,
  `screen::adopt_desktop_bg`). Banded present degrades to whole-box; the
  `[wc-x] backbuffer resync ABSENT (re-land owed; ghost-box hazard open)` witness marked the
  open seam. The x86 re-land arc (CLICK-X86 / FBCON-DMG / FLICKER + vug.rs cfg-strip with the
  ui_status seam) restores callers and callees together.
  - **WC-BBSYNC re-landed** (x86 seat, 2026-08-03, hand port of `9a68d99d` onto the merged
    anchors — the origin commit is an ancestor of trunk, so its content was overwritten rather
    than reverted and it cannot be cherry-picked). `screen::adopt_desktop_bg` + `DESKTOP_BG_SEED`
    are back and `wcx::activate` arms the latch, so the ghost-box hazard on the wc path is
    CLOSED. The witness flips `ABSENT` -> `[wc-x] backbuffer resync ARMED bg=… (desktop layer
    not yet constructed)` at activation, and the desktop layer answers with
    `[wc-x] backbuffer resync WxH (desktop bg …)` when its `Screen` is constructed. `ABSENT` no
    longer appears in any build. Banded present (`wm::present_rows`) is **no longer stripped** — the
    x86 re-land's FBCON-DMG milestone restored it with its caller (`fbcon::route_present_banded`,
    `#[cfg(all(target_arch = "x86_64", feature = "wc"))]`), so callee and caller are back together
    and aarch64 still produces no band. Its x86 evidence class is `unproven`, not proven: no capture
    yet carries a `[wc-h] scope=window-band` rollup (`08_VIDEO/engine.md`, FBCON-DMG).

## Known-opens ledger (carried from both batons; each is arc-sized unless noted)
- SYS_WIN_PRESENT masked span — **AUDITED** (pi seat, 2026-08-03): 2.8 ms mean / 13.9 ms
  worst-case IRQ-masked per present at 1920x1200 vs the 12 ms quantum (`[comp2] pass_us`,
  engine.md — the number was already measured, never read as masked latency). 6 defect sites
  over 9 locks; 4-milestone remediation sequenced (audit: plans/active/syswinpresent-audit-r24.md).
  **M1 SHIPPED as WEDGE-11** (below); M2 (DEFER guard + STAGE pre-size), M3 (span shrink:
  fb_checksum + sprite tail hoist), M4 (claim-id + revalidate-at-landing, needs fresh brief) open.
- ~~OVERLAY lacks its micro-guard~~ **CLOSED — WEDGE-11** (claim/loan, not micro-guard: the
  audit's own §5 miscounted; cursor.rs:1845/2460 are PRODUCTION panel-walk holds, so the
  three-site micro-guard would mask-spin on preemptible I/O holders. `[wedge11] overlay-claim`
  census, QUIET in QEMU; metal execution pending).
- unafs `with_unafs` has no Busy plumbing (transaction-restart arc) — in flight, pi seat.
- x86: ASID-0 dual-live collision; in-bounds surface write to unmapped page 5/16 at four
  windows (EL0 fault `err=0x6`, pre-existing, not the merge's); EL0 SYS_WIN_CREATE no-upscale
  speck (pi lineage likely fixes — prove at the x86 panel before closing).
- x86 sysret scrub: ring-3 stubs under-declare clobbers (`r10` declared once tree-wide) — live
  UB, x86 seat's. aarch64 twin audit queued (no eret GPR scrub exists there; hardening review).
- `[v3d55]` tilestate poison-vs-nonzero instrument bug (rung 0 of the V3D R-ladder, v3d.md §49).
- CBW/IOC history questions — **ANSWERED and folded** (x86 seat, 2026-08-03,
  `07_USB_STORAGE/usb_xhci.md` §17.9). CBW-FAULT has never fired on any rig; no-IOC was inherited
  from the original stack and re-asserted twice on spec reasoning, never measured, and §17.1's A/B
  (the only one in the history) convicted it; masked-span does **not** depend on the CBW being
  unawaited (family-doc invariants are lock-shaped, the three WEDGE diffs touch no cbw/ioc lines,
  and `block.rs:61` already sizes its bounded retry against the awaited-CBW 25 s hold).
  **Residual open, pi seat's:** the awaited-CBW architecture is unverified on Pi silicon — every pi
  `cbw_fault=0` in the archive reads `n=0 storage_slot=0` and is vacuous. Discriminator: a pi metal
  boot with `storage_slot != 0` and BOT traffic to `n > 0`, SUMMARY captured.
- kernel8 byte-identity — **INSTRUMENT RESTORED, with a usage rule** (discriminator run
  2026-08-03, report plans/review/kernel8-reproducibility-verdict.md): two clean-tree builds of
  one sha are byte-identical (0 differing bytes; path/timestamp do not leak). The 1410-byte
  cross-commit diff was ENTIRELY the embedded 8-char git-sha stamp (`arroyo:47` →
  `genet.rs:2572` `option_env!` → content-addressed rodata symbol reorder → ADRP/ADD ripple);
  a stamp-neutralized control proved the x86-cfg'd source diff contributed ZERO bytes to the
  aarch64 kernel8. RULE: same-sha byte-identity is valid evidence from CLEAN trees only;
  cross-commit comparison requires neutralizing the stamp (detach source, soft-reset to the
  reference sha). Corollary: builds must never run under RAM-backed /tmp (caused a host OOM).
- **Trunk 47f955a3 does not compile x86 under `witness`** (x86 seat, post-landing): two more
  orphaned x86 call sites, gated on `witness` not `wc` (`wcg.rs:130` imports
  `crate::arch::aarch64` unconditionally; `wm::focus_reset` missing at x86 syscall.rs:4142).
  Structural blind spot: arroyo auto-arms `witness` only for test targets, so `esp-x86` media —
  including the Condition-A boot — never contained the broken code (boot valid, tree not fully
  checked). Repair = milestone 1 of the x86 re-land arc (theirs). aarch64 CLEARED: both the
  check invocation and the kernel8-test feature path type-check green (x86 seat, replicated
  invocations). Open question, pi seat's: `pi` WITHOUT `baremetal` (+witness) fails 18×
  "cannot find sched in arch" — unestablished whether that path is ever run or predates the merge.
- x86 sysret scrub: FIXED on trunk post-landing (all six scrubbed registers declared in every
  x86 ring-3 stub; previously `r10` was declared exactly once tree-wide).
- Terminology: the x86 ring-3 fault witness string is **x86-only** (`vec=14`/`cr2`/`err` are
  x86 page-fault artefacts; `arch/aarch64` never emits it). It was spelled `EL0-equiv FAULT`,
  read like an ARM exception and cost a round-trip in review; it is now `:: RING-3 FAULT: … ::`
  and any doc naming that fault says "x86 ring 3" explicitly. No spec or `arroyo` battery step
  ever matched the old spelling (checked repo-wide at the rename), so no gate moved with it.

## Gate rules earned by this merge (binding; also in CLAUDE.md / LAWS)
- Video-stack checks carry `UNAOS_WC=1`; banner must list `wc`.
- Feature-proof hierarchy: **executed witness > strings-with-positive-control > banner**.
  Banner = compiled; strings = survived codegen; only an executed witness = reachable.
- x86 compositor ignition: `wcx::activate` is called only from `kepler_display.rs` — x86 video
  gating needs the kepler knob set (`UNAOS_IVB/KEPLER/KEPLER_TAKEOVER/KEPLER_FIFO/SMC`), not
  `UNAOS_WC` alone. pi activation is default-path (metal-proven).
- No QEMU suite on an unchanged tree — `mbench.py --replay` the existing capture.
- **Fixture-provisioning asymmetry** (the part a future seat will trip on): the two platforms
  had DIFFERENT fixture models — pi plants at image build (arroyo:1652–1673) + the aarch64
  U-family runtime-creates its files; x86 planted only in the QEMU FAT script, so x86 METAL
  boots silently degraded U9x/U10 witnesses to a passing in-memory mode. Fixed 2026-08-03
  (builder plants, contents cross-checked against witness constants). When adding a witness
  with an on-disk fixture, verify the fixture reaches EVERY medium the witness runs from.
- **Commit-message-overclaim** (x86 seat, self-reported; belongs beside the vacuous zero): a
  commit titled "name all six sysret-scrubbed registers in every stub" did not do what it said
  (read "argument registers" as per-stub, not the scrubbed six) and the gap crashed VUG.ELF on
  metal (`rsi` zeroed under a live `&mut` — err=0x5 cr2=0x0). A commit message is a claim;
  gate the claim, not the diff's existence. Fixed and disassembly-proven same day.
- **Syscall-return register handling diverges BY DESIGN — never port a "fix" across arches**:
  aarch64 needs no GPR scrub on the svc return because `RESTORE_GPRS` overwrites the full file
  from the task's own entry frame (kernel values structurally cannot leak; exceptions.rs
  `__vec_svc`); x86 NEEDS its six-register sysret scrub because its dispatch leaves kernel
  values in caller-saved registers. Same invariant, opposite mechanisms. Likewise the saved
  user stack pointer is per-TASK state on both arches — aarch64 banks SP_EL0 on the task's own
  kernel stack (M6e, hazard named in the entry comment); x86's per-CPU slot version of this was
  a cross-task stack-aliasing bug (s68) fixed 2026-08-03 to converge on the aarch64 shape.
- **Vacuous-zero law**: a zero from a counter whose subject never ran is vacuous, not passing —
  every zero-anomaly verdict must be qualified on evidence the subject ran at all (e.g. a
  `cbw_fault=0` claim requires `n>0 storage_slot!=0` beside it, and the capture should carry the
  self-describing knob line, e.g. `cbw=always-awaited`). Round examples of the class: the pi
  WEDGE boots' cbw_fault=0 with n=0; a waker conf written but never started; a strings probe
  reading 0 because the binary wasn't on the probed path. Positive controls or run-evidence,
  always.
- **Spent-budget law**: an instrument with a fixed sample budget that is consumed before its subject
  runs cannot falsify anything — its verdict describes the samples it took, never the behaviour it
  was aimed at. Every capped instrument must be qualified on evidence that a sample was taken *after*
  the subject started, and a rollup that closed early is `unproven`, not PASS and not FAIL. Round
  example of the class: `[wc-h]`'s `SAMPLES = 4` per window id, spent on `win=1`'s whole-box creation
  and first-paint presents, so the ~980 console lines printed after the rollup closed were
  unobservable and the arc they were meant to measure (FBCON-DMG) stayed unproven while the rollup
  read `AT-RISK`. Reachability (`strings`, banner) and observability are two claims; the feature-proof
  hierarchy above settles only the first.
