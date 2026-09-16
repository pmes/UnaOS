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

# ── CONTRACT (SPECRUN, 2026-09-15) ──────────────────────────────────────────────────────────────
# A PINNED LINE IN THIS FILE IS CHANGED TOGETHER WITH THE KERNEL LINE IT PINS, IN THE SAME COMMIT —
# re-pinned to the new wording (naming the arc that changed it), or dropped with the reason stated.
# It is never worked around by teaching the kernel a SECOND spelling of the same witness. That is
# what an unrun spec cost this tree once: STORWAIT added a second `storage settle:` line rather than
# edit the `[fatverb] storage witness` REQUIRE this file pins verbatim — a pin no command was
# reading, and still expensive.
#
# WHY THIS BLOCK IS AT THE TAIL AND NOT THE HEAD. Ledger rows, queue rows and kernel comments across
# this tree cite pinned lines POSITIONALLY (`x86-fat.spec:238`, `pi4-regression.spec:1549`,
# `jetson-sync1.spec:1839`, `crates/kernel/src/shell.rs:4933` -> `x86-fat.spec:156`). A header insert
# moves every one of them by the same amount, silently — rmbp-ledger PI5 names tail-append as this
# repo's safe form for exactly that reason. The contract is ENFORCED, not merely written: see below.
#
# WHO RUNS THIS FILE, and the gate that makes the answer mandatory:
# RUN-BY: bench — ./arroyo mbench --replay <jetson capture> --spec scripts/specs/jetson-jd5.spec --platform jetson
#   No verb replays it: the capture is an attended Orin bench log, which no QEMU verb can produce.
#
# GATE-SPECROOTS (`scripts/spec-roots.sh`, a leg of `./arroyo check`) reds by name on any spec under
# scripts/specs/ that is neither named in `arroyo`'s CODE nor carries a RUN-BY line above — and a
# `RUN-BY: verb:` claim is cross-checked against `arroyo`, so this file cannot claim a runner it
# does not have. "A replay spec no gate command runs is a silent landmine."
