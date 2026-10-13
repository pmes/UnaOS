# ARMVIRT — the aarch64 virt QEMU leg on 2c538013f (2026-10-13)

LOCAL WAVE 2, executor branch `exec-rmbp-local-armvirt`, cut at `2c538013f`.

## Command

    cd unaos && ./arroyo test-arm 60

That is the plain verb with a 60 s wall instead of its 20 s default. No knobs were set, so the run is
the GICv2 regime, and `test_aarch64` (arroyo:5096) replays no spec. The verb boots QEMU virt once,
fault-scans `target/serial-arm.log`, and runs `ladder_tail_check` on the `test_arm_gicv2` row. The
login-store / net6 / sntp6 / `arm-login.spec` legs are separate knob-gated runs (`UNAOS_GICV3=1
UNAOS_VIRT_EL0=1 ...`) and were not part of this one-run brief.

## Result

- **rc = 101** (cargo's exit code, passed through), **wall 41 s**: the run stopped in the kernel build
  and QEMU never started.
- Features: `⚡ kernel features: witness,ehcihid,kbdwit,sdw,sdhcblk,tests-at-boot,census,smolnet,sdwrite`
  / `⚡ aarch64 effective features: witness,tests-at-boot,census,sdwrite`
- The error, verbatim:

      error[E0433]: cannot find `cursorshape` in `cursor`
          --> crates/kernel/src/tests.rs:149:318
      ... crate::video::cursor::cursorshape::ensure(); ...
      note: found an item that was configured out
          --> crates/kernel/src/video/cursor.rs:5350:9
      5348 | #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
      5349 | #[path = "cursorshape.rs"]
      5350 | pub mod cursorshape;
      error: could not compile `unaos-kernel` (lib) due to 1 previous error; 29 warnings emitted

- **The known VUGK blocker** (`VUGK.ELF 16744 bytes > USER_REGION_SIZE`) was **not reached**. The
  build stopped earlier, in the kernel lib.

## Spec verdicts

None. There is no serial log: `target/serial-arm.log` was never created. This run has no completeness
verdict, so it is not a pass. The ladder (`test_arm_gicv2`, tail `:: BOT: tx single=`) was never
checked.

## Cause (one real red, LEDGER SP19)

Fold `4635423ce` (CURSORSHAPE, B550) added the `ensure()` call to the fixture registry in
`tests.rs:149` with no cfg gate. The module it calls is gated
`any(target_arch = "x86_64", feature = "aarch64_el0")`. The plain virt build (and any aarch64 build
without `aarch64_el0`/`virt_el0`) therefore no longer compiles. The fold records "x86 + Orin kernel +
AETHER + UNA checks rc=0", and no plain-virt build was among them.

## Files

- `build.log`: the whole run output, ANSI stripped (16.7 KB). It stands in for the serial log, which
  does not exist.
