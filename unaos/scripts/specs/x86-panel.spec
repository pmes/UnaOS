# x86-panel.spec — the x86 compositor leg at the rMBP's PANEL: 2880x1800 at scale 2.0 (ZOOMPANEL, rmbp-ledger B842).
#   QEMU gate:  UNAOS_FBW=2880 UNAOS_FBH=1800 UNAOS_PPI=221 UNAOS_WC=1 UNAOS_FACET=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 \
#               UNAOS_QEMU_FULL=1 ./arroyo test 240
#               -> unaos/target/serial.log, replayed by `x86_spec_replay` (arroyo names this file in code as
#               X86_PANEL_SPEC, picked in `x86_pick_capture_spec` before the WC lane, so GATE-SPECROOTS resolves it).
#
# WHY. ZOOM2 (B829), PERFHUD2 (B828) and TEXTSCALE (B813) each stopped at the same wall: the x86 QEMU panel was
# OVMF's GOP mode on the default VGA (1280x800 at 1.0), so no x86 fixture ran at the metal's geometry. The knobs
# move three things: arroyo arms the default VGA at `xres`/`yres` with `edid=on` and enough `vgamem_mb`; the UEFI
# loader picks the knob's size when OVMF offers it (`GOP: panel knob 2880x1800 -> offered`); `UNAOS_PPI` is the
# effective ppi `video::dpi::compute` answers, so the scale is 2.0 as the rMBP's 221 ppi gives it.
#
# DEPENDENCY. OVMF must offer the EDID-preferred mode (QemuVideoDxe's EDID read). An OVMF that does not prints
# `GOP: panel knob 2880x1800 -> not offered (today's pick)`, the run stays at 1280x800, and the ZOOM pin below
# reds by name (`panel=1280x800`).
#
# NO `COMPLETE` MARKER HERE, for x86-ptr.spec's reason: `x86_test_completion` reads x86-test.spec on every x86 leg.
#
# ── 1. THE PANEL: the knob's size and the knob's scale, latched once ──────────────────────────────────────────
REQUIRE \[dpi\] panel=2880x1800@2\.0 src=knob ppi=221
#
# ── 2. THE FIVE METAL-SHAPED FIXTURES, each at that panel ─────────────────────────────────────────────────────
# ZOOM's line names the panel it presented on; PERFHUD's names the scale it laid out at.
REQUIRE :: ZOOM: z=2 px_ok=3 present_us=\d+ -> PASS :: panel=2880x1800 smooth_us=
REQUIRE :: PERFHUD: visible=1 paint_us=\d+ fields=10 -> PASS :: .* scale=2\.0 rect=
REQUIRE :: TEXTSCALE: steps=5 overlap=0 cap=\d+ -> PASS ::
REQUIRE :: SHIMMERIDLE: idle_ms=\d+ ticks=\d+ max_us=\d+ reduce_motion_ticks=\d+ gate_steals=\d+ -> PASS ::
REQUIRE :: VEILCOST: ratio_p99=\d+\.\d+ shimmer_idle_us=\d+ -> PASS ::
#
# A SKIP at the metal shape certifies nothing (the leg exists to run them), and a FAIL is the default FORBID.
FORBID :: ZOOM: .*-> SKIP
FORBID :: PERFHUD: .*-> SKIP
FORBID :: TEXTSCALE: .*-> SKIP
FORBID :: SHIMMERIDLE: .*-> SKIP
FORBID :: VEILCOST: .*-> SKIP
