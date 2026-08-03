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
  `[wc-x] backbuffer resync ABSENT (re-land owed; ghost-box hazard open)` witness marks the
  open seam. The x86 re-land arc (CLICK-X86 / FBCON-DMG / FLICKER + vug.rs cfg-strip with the
  ui_status seam) restores callers and callees together.

## Known-opens ledger (carried from both batons; each is arc-sized unless noted)
- SYS_WIN_PRESENT runs the WHOLE compositor pass under IrqGuard+WINDOWS (TABLE/WRITER/STAGE/
  DEFER inside) — bigger than F4; next family-audit arc.
- OVERLAY lacks its micro-guard.
- unafs `with_unafs` has no Busy plumbing (transaction-restart arc).
- x86: ASID-0 dual-live collision; in-bounds surface write to unmapped page 5/16 at four
  windows (EL0 fault `err=0x6`, pre-existing, not the merge's); EL0 SYS_WIN_CREATE no-upscale
  speck (pi lineage likely fixes — prove at the x86 panel before closing).
- x86 sysret scrub: ring-3 stubs under-declare clobbers (`r10` declared once tree-wide) — live
  UB, x86 seat's. aarch64 twin audit queued (no eret GPR scrub exists there; hardening review).
- `[v3d55]` tilestate poison-vs-nonzero instrument bug (rung 0 of the V3D R-ladder, v3d.md §49).
- CBW/IOC history questions (did CBW-FAULT fire on metal; was no-IOC measured or inherited;
  does masked-span depend on the CBW not being awaited) — answered from capture archive, owed.
- kernel8 cross-commit byte-identity is NOT currently a valid instrument (1410-byte diff on an
  x86-confined source change; clean-dirs reproducibility discriminator queued).

## Gate rules earned by this merge (binding; also in CLAUDE.md / LAWS)
- Video-stack checks carry `UNAOS_WC=1`; banner must list `wc`.
- Feature-proof hierarchy: **executed witness > strings-with-positive-control > banner**.
  Banner = compiled; strings = survived codegen; only an executed witness = reachable.
- x86 compositor ignition: `wcx::activate` is called only from `kepler_display.rs` — x86 video
  gating needs the kepler knob set (`UNAOS_IVB/KEPLER/KEPLER_TAKEOVER/KEPLER_FIFO/SMC`), not
  `UNAOS_WC` alone. pi activation is default-path (metal-proven).
- No QEMU suite on an unchanged tree — `mbench.py --replay` the existing capture.
