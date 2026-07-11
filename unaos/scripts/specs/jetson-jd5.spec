# jetson-jd5.spec — the Jetson Orin Nano JD5 attended-bench serial shape.
#   Metal:  ~/jetson-serial.log (jetson-bench-connect.sh bridge capture)
#   Validated against the REAL 2026-07-10 JD5 PASS capture
#   (~/unaos-bench/jetson-serial-2026-07-10-165211.log — the survive-power-cycle
#   bench: TWO boots in one capture).
#
# Serial-scope caveat: JD5 write-path RESULTS (wrote/rm -ENOENT/subdir -ENOTSUP)
# render to the PANEL, not serial — serial carries the tegra `::` witnesses and
# per-keystroke echoes only. Panel verdicts stay attended-eyeball until a
# kernel-side serial echo of shell verdicts exists (reported to the seat; a spec
# must not invent kernel lines).
#
# Expected noise, deliberately NOT forbidden: `xHCI: >>> COMMAND FAILED (Code 11) <<<`
# (hub-MSC intermittency, graceful fallthrough — see unaos-jetson-resume).

# --- boot bring-up witnesses (the JD/JB chain) --------------------------------------
REQUIRE JD1.*scanout:.*sane=true
REQUIRE JD1.*panel LIVE
REQUIRE JB1b.*MRQ_PING.*-> PASS
REQUIRE JB0.*fan ON.*-> PASS
REQUIRE JB1c.*XUSB ALIVE.*-> PASS
REQUIRE JB2b.*keyboard ARMED.*-> PASS
REQUIRE JD3.*mass storage ready
REQUIRE JD2.*console pump live
REQUIRE JD4.*console OWNS the panel

# --- scheduler capstone -------------------------------------------------------------
REQUIRE CAPSTONE COMPLETE

# --- the JD5 power-cycle shape: the survive-reboot bench boots TWICE ----------------
# (a single-boot smoke run will show these at 1 hit — the COUNTs are the money-shot
# assertion for the survive bench specifically)
COUNT 2 JD1.*panel LIVE
COUNT 2 CAPSTONE COMPLETE

# --- keyboard traffic proves the shell was driven -----------------------------------
REQUIRE xHCI: KEY:

# --- forbidden: storage-path hangs/timeouts (defaults -> FAIL / FAIL :: / PANIC
# --- are always on) ------------------------------------------------------------------
FORBID pump timeout
FORBID timed out
FORBID AARCH64 EXCEPTION
