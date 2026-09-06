# rmbp 15 — the x86 FOCUS QUEUE

**What this is.** rmbp 15 is a SUPPORT round (R22: support spawns ZERO executors; R23: a stop on
starting jobs is never a stop on being support). orin holds the focus. Peter's order this session:
do not start jobs, keep supporting orin, and **queue the x86 work for the focus time**. This file is
that queue — ordered, with the first command for each item, so the pivot costs no planning.

**When it fires.** The focus pivots to `hw-rmbp` the moment Peter leaves the bench, because the 2012
rMBP is a laptop and travels with him (`docs/dev/LAWS.md` §Focus; memory
`focus-rotates-the-rmbp-travels`). That makes a trip round the **only** round in the fleet where x86
metal — boots, serial capture, card writes, staged media — is live. It is therefore NOT a code-only
round, and the queue is ordered on that: metal first, because nothing else can do it.

**Budget.** Nine executors is the focus seat's CEILING, not a floor (memory `nine-is-a-ceiling`).
Queue past nine; never spawn past it.

**Freshness contract.** Every sha below was resolved in this worktree on 2026-09-06 with
`git log --oneline -1 <sha>` and located with `git branch --contains` / `git branch -r --contains`.
Re-run both at pickup: **three of them are on local branches only** (Q0).

---

## Q0 — BEFORE ANYTHING: three live grant targets are unreachable to everyone but this machine

Verified this turn, `git branch -r --contains <sha>` empty for all three:

| sha | what | carried by | on origin? |
|---|---|---|---|
| `dc683c40` | SHELLRELICS | `exec-orin17-shellrelics`, `exec-orin17-vfsroute`, `exec-orin18-fslayout` | **no** |
| `1aae3459` | VFSROUTE | `exec-orin17-vfsroute`, `exec-orin18-fslayout` | **no** |
| `28899d5c` | DUPGUARD fixture | `exec-orin17-dupguard` | **no** |

By contrast `74b7c764` (VIRTPREEMPT) and `643d2803` (CONSOLETEXT) are on `origin/hw-jetson` — folded
and safe. The three above are one branch-prune from existing only as scratch patch files, and no peer
can fetch them. This is the `unowned-checkouts-are-the-hazard` class: they fall BETWEEN lanes.
**First action of the focus round: get them onto a track branch or confirm orin has.** Ledgered as B59.

---

## Q1 — THE rMBP METAL FLIGHT (J3). Metal-only. Do it first; it is the reason the round exists.

XHCINTD is **accepted** and blocked on nothing but this flight (B56). The completion path of the
keyboard interrupt-IN transfer has coverage **nowhere else in the fleet** (B45): x86's `kbdwit` is
EHCI and its xHCI HID device is a pointer; `test-arm` enumerates a real xHCI keyboard but QEMU sends
no reports without an injector `arroyo` does not have. The rMBP at the glass is the only scorer.

- Apply order is XHCINTD then DUPGUARD, both with `-3`.
- Regenerate the patches from the commits, not from scratch paths:
  `git format-patch -1 28899d5c --stdout > <scratch>/dupguard.patch` (and the XHCINTD commit from
  `exec-orin17-*`), which is also the Q0 rescue.
- Gate before the card write: `./arroyo check` both arches, `UNAOS_WC=1 ./arroyo test 150` with `wc`
  in the `⚡ kernel features:` banner, `./arroyo test-arm`.
- Score the boot by the **loaded image's `max_vaddr`** first, not by the card sha — a 10/10 card sha
  proves the write, not the boot (memory `verify-what-booted-not-what-you-wrote`).
- Ride-along captures, since the machine is open anyway and each is a ledgered open row:
  **A6** `[clickroute] … -> FAIL` deterministic on metal / green in QEMU — bracket it: new-with-arc or
  pre-existing. **A5** shell-window tearing under storm. **A1** the BAR1 wedge recovery path.
  **A9** the FTDI bulk IN 0x81 that is never driven (TX-only serial is what blocks DEV-LOOP).

## Q2 — THE TWO REVIEWS ORIN'S FOLDS ARE BLOCKED ON (J2). Not metal. **Can start before the pivot.**

This is support product, not a job (memory `support-seat-product-is-verification`), so it does not
wait for the focus — it is offered to orin 18 as of this turn.

- **SHELLRELICS `dc683c40`** (46 files, +1274/−712; `shell.rs` +876/−434, `midden_core` +176/−93 —
  measured, B58). One leg owed: proof that the old raw `write <lba> <byte>` no longer REACHES the raw
  path. The fixture at `shell.rs:3135` asserts over the **help text**, not the dispatcher — that is
  the gap, and it is a dispatch-site read, not a rewrite.
- **VFSROUTE `1aae3459`** (`shell.rs` +1806/−2497, `fs/vfs.rs` +314, `fs/fat.rs` +26). Stopped at
  deliberately in a handover's closing minutes; certifiable with time on the clock.
- `libs/sys/midden_core` is **not this lane's to grant** — that half needs its owner.

## Q3 — J1, THE LANDING. Needs the adversarial panel, which is a fleet, which is the focus.

`git rev-list --count main..hw-rmbp` = **121** this turn (**not** the baton's 120 — `5c3dbb7e`, rmbp
14's landing report, landed after the baton was written), and **94 behind**; merge-base `f49ea1e7`.

- The COI guard holds: the author seat never reviews alone. Panel first, then ccd announce, then a
  peer ack from at least one other track seat, then this seat's own `--no-ff` merge, then the trunk
  battery — with a **fresh `ls-remote` in the same turn as the merge**, both seats.
- The known non-union: **`drivers/xhci/mod.rs` has diverged 12/77 (B43)** and two of orin's patches
  fail `git apply --check` here at `xhci/mod.rs:2382`. That is content-delta reconciliation, and it
  is the single largest unknown in the landing — budget an executor for it alone.
- `main` now carries orin's ledger, so B22's deferred `→ SO6` resolves at the trunk sync.

## Q4 — THE `arroyo` SWEEP THIS SEAT OWES (J4/B55). Code-only. One line-neutral commit.

Re-verified this turn, all eleven still restate the gate as `any(baremetal, tegra_el0)` in prose:
`unaos/arroyo` lines 906, 1011, 1046, 1078, 1175, 2802, 2956, 3047, 3235, 3553, 4076.

The executable half is the point: `unaos/arroyo:2143`'s `case ",${_af#--features },"` is still the
hand-maintained enumeration, and `unaos/scripts/k8-reach.py` already computes the closure
(`cargo_implications`, lines 214 and 247). **Derive the case from it.** The commit carries its own
grep, and `panic::Location` means line-neutral or tail-append only (memory
`cfg-does-not-protect-byte-identity`).

## Q5 — GATES THIS LANE OWES (J5). Code-only, parallelizable, one executor each.

- **B47 — `HOST_VERBS` ↔ dispatch-arm, in BOTH set directions.** Two verbs answered "Unknown command"
  for their entire existence while a comment claimed the invariant was checked. Note the shape: the
  table is `unaos/libs/sys/midden_core/src/lib.rs:245` and the arms are in `shell.rs`; a gate that
  **reads** midden_core needs no grant — only an edit would.
- **B53 — the `[wc-d]` / `[wc-g]` / `[wc-h]` / `[wc-k]` fixtures do not model the console window**,
  which gives them a run-to-run VARIABLE forbidden set. Specs live in `unaos/scripts/specs/`
  (`x86-wc.spec`).
- **The `supstate` × `holocron` / `orintenant` / `orinladder` matrix gap.**
- Standing rule for every one of these: a check that cannot fire is not a check — printing is not
  gating, and a zero-hit result indicts the pattern (memory `a-check-that-cannot-fire`). Prove each
  gate by mutation, at the sha it will run on (memory `verification-comes-from-execution`).

## Q6 — SMALLER, ALL ALREADY LEDGERED (J6)

- B53's reopen-path `[deskcascade] fit` reading.
- B58's `video/prtscr.rs` — still FAT-direct; confirmed this turn at `prtscr.rs:17`, and its own
  header documents the crash-consistency direction it depends on (`:84`).
- **B10 — the R19 shut-out register. A READING task, still never started**; its executor was killed at
  minute four two rounds ago. R19: failed paths stay open, so this register is what keeps them open.
- The x86 lane's older rows: A2, A3, B1, B6, B7, B9.

## Q7 — PETER DECISIONS. Surface ONCE; do not chase.

- CONSOLETEXT's first mint stays black (his render6 ruling). Reversing is one line (B54).
- **A4** — the card as default startup volume; not kernel work, it is `bless`/Startup Disk on his
  laptop, and it is what makes unattended reboots into UnaOS possible.
- **B7** — the vug arbiter.

---

## Nine-executor shape, if the pivot comes with the fleet

1 metal (Q1, the seat itself at the glass) · 1 `xhci/mod.rs` 12/77 reconcile (Q3) · 2 landing panel
(Q3, adversarial, independent) · 1 `arroyo` derived-case sweep (Q4) · 3 gates (Q5, one each) · 1 B10
register (Q6). Q2 is the seat's own reading and is expected to be closed **before** the pivot.
