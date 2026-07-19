# FOX — METAL SITTING 2 (R22) LANDING REPORT — 2026-07-18 evening

**Seat:** Una(Fox) at the metal-support post (Maestro seat cold, stepped in per brief).
**Order run:** Pi → x86 → Orin, per Peter. All three platform ledgers updated (unaos-metal-{pi4,rmbp,orin}.md — the verdicts of record live there). Captures with marked boundaries: `~/unaos-bench/capture/{pi-r22s2,rmbp-r22s2,rmbp-b3,orin-r22s2}/`.

## Headlines
- **Pi: first-ever full-spec metal boot — MBENCH 46/46, 0 forbidden** (K3-BIT5 confirmed `[w=0x1ff]`); V3D enable CONFIRMED (live IDENT0 0x42554856 — poison era over); PRIO-MIX metal-confirmed; **new SError-at-first-tick class isolated** (3 fault + 2 control boots, one diagnosis: pending async abort from poisoned MMIO delivered at first DAIF unmask); RC LINK UP first time; GENET DTB totalsize=0 parse defect found.
- **x86: the Boot-B mysteries all fell.** GUI serial provably up (clean log pair, cable untouched); Boot-B slowness = fbcon scroll → FBCON-QUIET + wrap-around (Peter's no-scroll ruling); vug slowness = the dead SMC stalling the render loop; **the SMC hammer-sweep was wedging the EC — with backoff the battery came alive (soc=78%, real mA)**; first metal-confirmed trackpad CLICK (click-exits-vug witness); cursor sprite + auto-hide; pulse 8-bars honest; KERNEL-CLOCK/AGEREF/CVCAP metal-confirmed; crystal splash v1 on metal.
- **Orin: NET-4b COMPLETE PASS, zero RAS** — the iATU + poison-honest-readback fix cleared where last sitting RAS-walled; **smoltcp bound over the RTL8168 on silicon** (MAC 4c:bb:47:25:49:c8, PHY UP). SDMMC census + armed ladder 7/7 ×2 (first UnaOS write to Orin SD, restore byte-perfect). Destructive install honestly refused (no stick enumeration — cause scoped to Code-11s at JB9i eviction). Card restored to `cad623af…`, verified.

## Merged to main this sitting (Fox reviewing, Maestro seat cold)
`FBCON-QUIET` → `SPLASH+VUG-FIX` (VUG-POLISH-1) → `VUG-POLISH-2` → `SPLASH-2` — each QEMU-gated ×3 before merge, each then metal-verified same night (B2→B5).

## Tooling arc (Fox's own, on UnaOS-unaide)
**squawk-bench v1** shipped and ran the whole sitting: fingerprinted port enumeration, one-reader-per-port enforced, per-port raw capture with marked boundaries, `await` wake mechanism (evolved live: from-last-mark scan, all-ports follow, clean-boot trigger; misses each cost a lesson, all committed). v2 = comscan `caps/squawk/` citizenship + gneiss_pal-on-foreign-soil (queued).

## In flight at close (3 executors, Peter-approved)
1. hw-pi4: SError-drain class + VL805 CFG-window + GENET DTB parse.
2. hw-jetson: install-site xHCI enumeration (Code-11 eviction) + NET-4c DHCP TX-proof instrumentation.
3. hw-rmbp: splash→midden text-flash transition + cursor save-under (midden trails).

## Open items → R23 seed
Splash polish (rays brighter still, drift further from DSotM); pulse thread-migration question (single hot thread never spreads — display honest, scheduler question open); Pi self-clone (INSTALL-2 Pi analogue) + Orin cloned-card bootability (both named unknowns, unreached); DHCP lease + ping (blocked on NET-4c evidence); verde-start regen owed (generator absent from this checkout — Maestro owes the round the regen).

## Process notes (honest)
Fox misses this sitting, all corrected in-flight: media prep not started during idle wait (Peter escalated — right); waker gaps ×3 (no-arm, wrong-log, consumed-without-rearm) — each now a committed squawk fix + the standing re-arm rule; one card ejected before load verified (re-done with on-card SHA-MATCH before every subsequent eject); one wrong expectation promised (46/46 on a quiet build — DEFAULT-QUIET law recalibrated).
