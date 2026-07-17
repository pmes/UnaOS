# VIDEO-NEXT / VWIT — landing report (us-videonext, R20)

**Branch:** `us-videonext` (worktree off `main` @ `534c55b`). Commits: `d9ae8ce` (M1 code),
`a21ba36` (M2 docs). Not merged/pushed.

## Audit (MILESTONE 0)

Audited the four `future/` video plans (video-stack, vperf, realhw-bringup pt1/pt2) against the repo.
**All feature work is landed on main and metal-confirmed:**
- `video/{framebuffer,screen,fbcon,vperf}.rs` + `vug.rs`: surfaces, damage-tracked `Screen`
  double-buffer, boot/panic fbcon, vug crystal engine + load meters, `pulse` monitor.
- VPERF (cached-RAM shadow) + VPERF-WC (x86 fb Write-Combining): metal-confirmed rMBP (round-6/9).
- Pi 4 bare-metal VideoCore mailbox framebuffer (`arch/aarch64/mailbox.rs` + `boot.rs` Phase 2): landed.
- MacBook Retina GOP/HiDPI (padded stride, EDID fallback): done, metal-confirmed.

**video-vperf branch disposition:** `../UnaOS-vug` tip `e78c038`. `git log main..video-vperf` is
**EMPTY** — `e78c038` is a full ancestor of main, worktree clean. Nothing to recover; the branch is
**consumed** (its VPERF-WC commits merged via `0198ff4`). No seeding/redo needed.

**QEMU-provable gap found:** the damage-tracked `Screen` rasteriser (the steady-state on-screen
renderer) had **no automated regression** — the `tste` `video.geometry` check only exercises the
trait-default primitives on an `OffscreenPal`; the `Screen`/`TargetPal` override was verified only
visually by vug/the GUI console (attended; engine.md §1 + selftest.rs:450 both say so). Remaining
metal legs (Pi ESP USB-boot, EDID-on-EDK2, AP-WC) are **attended metal-ledger items, not
QEMU-provable**. VWIT is the honest single QEMU-provable slice.

## Scope delivered (VWIT, 2 milestones)

- **M1** — `video/witness.rs::run()`: builds a `Screen` over a heap-backed offscreen `FrameBuffer`
  and asserts the real present path: Bgr byte-order decode; **damage-limited blit** (a sentinel poked
  outside the next draw's bbox survives the flush — the invariant `video.geometry` cannot observe);
  idempotent no-op flush; clip safety for signed off-screen line/triangle. Registered as the
  `video.present` tste row (one additive line in shared `selftest.rs`; logic stays in the video lane).
  Arch-neutral (heap+Vec, no cfg/float). Emits `:: VWIT: render present — format=Bgr damage=OK
  noop=OK clip=OK ::`.
- **M2** — engine.md **§7 Headless render-path witnesses** (path table + assertions + evidence lines);
  MILESTONES entry.

## Gates (verbatim)

- `./arroyo check`: `✅ x86_64 OK` + `✅ aarch64 OK` (no warnings from witness.rs).
- Headless x86 (`UNAOS_QEMU_EXTRA="-qmp …" ./arroyo test 45`, `tste` driven via `qmp_type.py`):
  ```
  :: TSTE: video.geometry -> PASS ::
  :: VWIT: render present — format=Bgr damage=OK noop=OK clip=OK ::
  :: TSTE: video.present -> PASS ::
  xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<
  ```
  0 TSTE FAIL.
- `UNAOS_GICV3=1 ./arroyo test-arm 40`: `timer heartbeat live`, 3 online APs idle-heartbeat PASS,
  `CAPSTONE COMPLETE — all 6 sync primitives verified` — unchanged.
- `./arroyo kernel8-test 12` (selftest.rs shared/compiled for pi): exit 0, pi serial **0 FAIL / 29
  PASS**, no PANIC.

## Lens verdict

ONE lens (self-review of the diff). **PASS, 0 must-fix.** The four assertions are correct and the
`video.present -> PASS` headless result empirically confirms every branch (including the
`0 < filled < W*H` triangle-clip bound). The damage-limited-present assertion (sentinel survival) is
the load-bearing one and is sound: `Screen::flush` blits only the damage bbox, so a poke at (0,0)
disjoint from a (10,10) rect must survive — and does. No protection touched (no page-perm/MTRR/PAT/
checksum surface anywhere near this arc). Shared-file touch minimized to one registration line.

## Flagged / carried forward (attended metal ledger — NOT this arc's gate)

- Pi 4 ESP **USB-boot** (EDK2↔microSD block-read limitation; boot OS from USB) — realhw-bringup-pt2 §4.
- **EDID parser on EDK2** (Apple EFI + QEMU OVMF don't publish it; DTD math hand-checked only).
- **AP-WC**: wire `ensure_pat_wc()` into `smp::ap_entry` for uniform WC — needs `smp.rs`, out of the
  video lane; get it into a brief first.

**No further QEMU-provable video arc remains** after VWIT: the render engine now has automated
headless coverage of both primitive paths, and everything else outstanding is attended metal. No next
video baton is queued on that basis (would be manufactured scope).
