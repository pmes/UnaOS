# KFONTPPI (rmbp-ledger B382) — the panel's ppi from the panel's EDID, firmware or AUX

## Design

**Finding (flight 23, both boots).** `[kfont] load … ppi=0 scale=1.0 cell=7x16 grid=411x112 panel=2880x1800`
and `[ui] metrics ppi=0 scale=1.0`. Two causes, both read from the tree and the wire:

1. `video::edid_block()` is `None`: the firmware carried no valid base block, and the one line that would say so
   (`init_edid`'s `:: video: edid present= …`) is a `bootlog_println!`, absent from every flown build.
2. The iGPU lane read the panel's EDID over AUX (header OK, sum 0 mod 256, `06 10` Apple, DTD hactive 2880,
   image width 331 mm -> 221 ppi) and handed it to nobody. AND — the second gate at 0 — `video::dpi` LATCHES the
   scale the first time `fbcon` arms the console grid, which on the wire is `SPLASH: stage=takeover at_ms=6263`,
   two ms BEFORE the PCI probes reach `igpu::init`. A block published by the lane alone would have been read by
   nobody: the latch already holds ppi 0 / scale 1.0 for the whole boot.

**Seam (R79).** The kernel is the display driver; the EDID carry is one store, `video::EDID_BLOCK`, with one
publisher module. A new child module `video/edidsrc.rs` (CHARTER: Kernel — driver) owns the source tag
(`fw`/`aux`/`none`), the AUX offer, and the KFONTPPI witness; `video::dpi` gains one feed, `relatch_edid`, which
re-latches ONLY a latch that was taken with no EDID (ppi 0) — the scale math (`compute`, `s2_for`) is untouched.

**Milestones.**
- M1 — the firmware EDID witness on every build: `init_edid`'s two lines become `serial_println!` (one line per
  boot, R86) and set `edid_src=fw` when the block passes header + checksum.
- M2 — the AUX offer: `igpu`'s EDID dump hands its 128 bytes to `video::edidsrc::offer_aux`; header + checksum
  checked again there; published to `EDID_BLOCK` with `src=aux` only when the firmware block is absent or BAD;
  when both are good and differ, the finding is printed and the firmware's is kept. Then `dpi::relatch_edid`
  re-latches a ppi-0 latch from the live panel width. One line:
  `:: video: edid-aux hdr=OK sum=OK native=2880x1800 ppi=221 fw=absent -> published relatch=1.0->2.5 ::`
- M3 — the witness, one line after `[kfont] load`:
  `:: KFONTPPI: edid_src=aux ppi=221 scale=2.5 cell=18x40 grid=160x45 -> PASS ::`
  (QEMU: `edid_src=none ppi=0 … -> SKIP reason=no-edid`, not a fault; an EDID read but ppi 0 is FAIL.)

**What the next metal boot should print** (rMBP 15, firmware without EDID, as flight 23):
```
:: video: edid present=0 hdr=- sum=- native=- len=0 ::
:: video: edid-aux hdr=OK sum=OK native=2880x1800 ppi=221 fw=absent -> published relatch=1.0->2.5 ::
[kfont] load … ppi=221 scale=2.5 cell=18x40 grid=160x45 panel=2880x1800 …
[ui] metrics ppi=221 scale=2.5 s2=5 …
:: KFONTPPI: edid_src=aux ppi=221 scale=2.5 cell=18x40 grid=160x45 -> PASS ::
```
Note: the brief's read list said `scale=2.3`; KERNELFONT2's own math rounds 221/96 to the half pixel: 2.5.

**Owed.** The pre-desktop full-screen boot console (`fbcon` glyphs-active, armed at 7x16 before the lane ran)
keeps its 7x16 cell until the desktop's console window is built from `grid_cell()`; `fbcon::regrid` serves the
windowed console only. The AUX read only exists on `intel-ivb` builds; a machine with neither a firmware block
nor the lane stays at `edid_src=none`.
