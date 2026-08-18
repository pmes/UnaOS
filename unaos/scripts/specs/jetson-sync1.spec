# jetson-sync1.spec — boot 1 of the synced base (hw-jetson post trunk-merge ceaa32b8).
#   Metal: bridge capture per BENCH-PROCESS (never a /dev path; log-file replay/follow only).
#   The ONE question this boot answers: does the merged trunk (a month of desktop/
#   userspace/net work + the scheduler reconciliation) still bring the Orin up through
#   the full JD/JB chain on real silicon?
#
# Derived from jetson-jd5.spec's single-boot shape (that spec's power-cycle second half
# does not apply to boot 1). Serial-scope caveat carried over: panel verdicts render to
# the PANEL; serial carries the tegra `::` witnesses and keystroke echoes only.
#
# Expected noise, deliberately NOT forbidden: `xHCI: >>> COMMAND FAILED (Code 11) <<<`
# (hub-MSC intermittency, graceful fallthrough — unaos-jetson-resume).

# --- boot bring-up witnesses (the JD/JB chain — all previously metal-proven) --------
REQUIRE JD1.*scanout:.*sane=true
REQUIRE JD1.*panel LIVE
REQUIRE JB1b.*MRQ_PING.*-> PASS
REQUIRE JB0.*fan ON.*-> PASS
REQUIRE JB1c.*XUSB ALIVE.*-> PASS
REQUIRE JB2b.*keyboard ARMED.*-> PASS
REQUIRE JD3.*mass storage ready
REQUIRE JD2.*console pump live
REQUIRE JD4.*console OWNS the panel

# --- scheduler: the post-merge trunk scheduler on Orin metal ------------------------
REQUIRE CAPSTONE COMPLETE

# --- NEW this boot: witnesses shipped ahead of their bench (promote on capture) -----
# M1b: the first EL0 round-trip on Orin metal (tegra_el0 knob armed on the image).
PENDING TEGRA-EL0.*el0-hello round-trip -> PASS
# M2 step 1: the microSD becomes block-layer-visible (read-only backend).
PENDING TEGRA-SD.*block backend published

# --- regressions that would convict the merge, not the hardware ---------------------
FORBID PANIC
FORBID Serror
FORBID X200 FLAG
