# PIKERNEL8TEST — `./arroyo kernel8-test` on the image-24 Pi line (2026-10-13)

Branch `exec-rmbp-local-pikernel8test` off `2c538013f`. Executor run, LOCAL WAVE 2. QEMU raspi4b leg (Pi track).

Command, from `unaos/`, both runs:

    set -a && . ./scripts/image-line-pi.env && set +a && ./arroyo kernel8-test

(default window 60 s; no CARGO_TARGET_DIR; in-tree build.)

## Verdict: NO VERDICT — the leg never reached QEMU. Two build-time reds, one fixed here, one filed.

No `target/serial-pi.log` was written by either run (QEMU was never launched), so there is no serial
log and no `pi4-regression.spec` verdict line to quote. The build output of each run is kept beside
this file (`run1-build.log`, `run2-build.log`, ANSI stripped) in its place.

### Run 1 — at `2c538013f`: rc=1, wall 92 s

Stopped in aarch64 EL0 staging (arroyo:11002), the known ARMBUILD blocker:

    ERROR: VUGK.ELF 16744 bytes > USER_REGION_SIZE (16384)

`readelf` on the product: `.text` size 0x2011, `.note.unaos.app` at 0x2014..0x202c (44 bytes past the
0x2000 page cliff), `.bss` pushed to 0x3000 / file offset 0x4000. Cause: WIREDIET M1
(`wirediet_fps_due` + `VUG_SAID` + `WIREDIET_HEARTBEAT_TICKS`, crates/user-vug/src/main.rs) was compiled
on aarch64 while M2 was already `#[cfg(target_arch = "x86_64")]`.

Fix (`vug: PIKERNEL8TEST`, f5dd50a63, user-vug only, +9/-1): M1 gated to x86_64 like M2; aarch64 keeps
the pre-WIREDIET behaviour (`[vugfps]` on every rate change).

### Run 2 — with the fix: rc=1, wall 240 s

VUG staging green: `VUG.ELF`, `VUGC.ELF`, `VUGX.ELF`, `VUGK.ELF` all 12648 B. kernel8.img built
(`Finished release in 2m 34s`). Then `banner-cert.sh` (arroyo:12085) refused the media, verbatim:

    ⚡ banner-cert: arch=aarch64  (FLAT image (no ELF header); the caller named this arch in argument 3)
    feature=lumen witness=:: LUMEN: hits=0 -> MISSING
    ⚡ banner-cert: ok=20 missing/leak=1 registered-divergences=0 unverifiable=4 nowitness=1 noverdict=0  (table: 92 measured, 3 unmeasured-here)
    ❌ banner-cert: the banner and the artifact DISAGREE — this media is red.

Measured on the artifact: `LC_ALL=C grep -a -o -F ':: LUMEN'` hits (the `:: LUMENAPP:` / `:: LUMENFAST:`
literals, which ARE compiled for aarch64), but `:: LUMEN:` has 0 hits. The only `:: LUMEN:` literal is
`":: LUMEN: start"` in `spawn_run`, crates/kernel/src/lumen.rs:245, under `#[cfg(target_arch = "x86_64")]`.
So `lumen` IS compiled into kernel8.img; the banner-cert table row `lumen|:: LUMEN:|-|measured`
(scripts/banner-cert.sh:365) names an x86-only literal with no `@arch:x86_64` qualifier. This went live
when image-line-pi.env (image 24, 2026-10-13) first put UNAOS_LUMEN on the Pi line. Gate-table defect,
not a kernel regression — but it blocks every Pi image-line build. Filed as pi-ledger PI11. Out of this
executor's lane (fix allowed in user-vug only), and the one allowed second run is spent.

## Reds filed

- PI10 — VUGK.ELF over USER_REGION_SIZE on aarch64 (fixed-unflown here, f5dd50a63).
- PI11 — banner-cert `lumen` witness is x86-only; Pi image line red at banner-cert (open).

## Owed

A third run of the same command once PI11 is fixed (suggested: the lumen row's witness becomes
`:: LUMENAPP:`, present on both arches, as aether's row uses `:: AETHERAPP:`) — that run is the first that
can carry a pi4-regression.spec verdict.
