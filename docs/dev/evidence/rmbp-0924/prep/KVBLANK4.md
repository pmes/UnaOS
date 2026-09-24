# KVBLANK4 — prep

## The finding

`docs/dev/OS/rmbp-queue.md:17`: *"KVBLANK4 (rmbp; from FLIGHT13.md §5, B192's falsifier fired):
re-read `0x140` vs `0x640` against `line_or=1 at en_host=0` in `KEPLER-METAL-LOG.md`, and the
per-message MSI re-arm (`irq=1` per ~62 vblanks is the number to move). Metal-measured; the doc
half can be written in the cloud once the register reading is decided."*

Flight 13 §5 / 14-15 wire: `:: kepler: vblank-intr vector close head=0 irq=1 vbl_delta=62 …
mode=irq deliver=msi …`. One MSI delivered per ~62 vblanks. B192's own PRE-REGISTERED note called
this (`KEPLER-METAL-LOG.md:41-42`): *"A reading of `irq=1` … against `vbl_delta≈60` says the
function sent a message and did not send again. The per-message MSI re-arm is `NOT-IN-TREE`, so
that reading is a result and not a failure."* Flight 13-15 landed exactly that. B192's REGISTER
falsifier — `line_or=1` while `en_host=0` (`KEPLER-METAL-LOG.md:30-31`, would refute the `0x140`
reading) — did NOT fire: `mode=irq deliver=msi` requires `en_host` bit 0 armed at 1. The
`0x140`/`0x640` split stands; only the re-arm gap is open.

## Mechanism

- `0x140` = `NV_PMC_INTR_ENABLE_HOST` (`kepler_vblank.rs:210`): bit 0 hw enable, bit 1 sw; global,
  no per-source bits (envytools `pmc.rst:30,345-349`).
- `0x640` = `NV_PMC_INTR_MASK_HOST`: per-source mask, bit 26 = PDISPLAY (`pmc.rst:45,374-377,494`).
- `0x0C0 + head*0x800` (PDISPLAY-relative) = `DISP_INTR_HOST_HEAD_EN` (`kepler_vblank.rs:239-242`)
  — the per-head VBLANK enable, the only register the ISR touches.

`rung3_arm` (`kepler_vblank.rs:896-985`) arms once: `0x640`=bit 26 only, `0x140`|=bit 0,
`DISP_INTR_HOST_HEAD_EN`|=vblank bit, each read back. The GK107 raises PDISPLAY, the MSI (vector
`0x44`) fires. `kepler_vblank_isr` (`:1131-1151`) counts the entry, **disarms**
`DISP_INTR_HOST_HEAD_EN` back to the pre-arm value (`WIN_EN_ENTRY`, `:1138-1142` — "the ack",
`:1118-1130`, since rnndb names no status/trigger ack for this stripe), EOIs (`:1150`), returns. It
never re-arms.

The intended re-arm lives in `rung3_run` (`:1056-1075`): "if the enable bit is clear, set it
again." But `rung3_run` runs only from `ladder_tick`, driven by `note()` (`:689`), fed **only** by
`scanout_beam()` reads from the compositor's own present/wait path (`beam::hold`'s spin,
`wait_next_edge`, `:33,108,129,1219-1231`) — never by the ISR. The re-arm clock is the compositor's
poll cadence, not the vblank/IRQ cadence. At `kepler::init`'s R3 window those polls are sparse (the
PRE-REGISTERED doc flags this needs "the desktop presenting for at least ~20s",
`KEPLER-METAL-LOG.md:15-17`): the ISR disarms after the first message and the next poll-driven
re-arm misses the ~60-vblank window — exactly one delivery per window. The window's close
(`:1077-1086`) then restores the enable to fully OFF, so this ladder is a one-shot bring-up
measurement, not a standing IRQ source. Register identities are right; the disarm and the re-arm
just run on two different, uncoupled clocks.

## Plan

**M1 — per-message re-arm in the ISR.** `kepler_vblank.rs`, `kepler_vblank_isr` (`:1131-1151`):
replace the disarm-only write with one write of `WIN_EN_ENTRY | vb` (OR the vblank bit back in,
same MMIO write, before EOI) — see `## Draft code`. `IRQ_STORM_CAP` (`:1143-1146`) untouched.
`rung3_run`'s re-arm branch becomes a no-op most ticks, stays as a second-chance belt. Witness: the
close line (`:1108`) should read `irq≈vbl_delta`, plus a new
`:: KVBLANK4: irq=<n> vbl=<n> ratio=<f> -> PASS ::` (PASS at `ratio>=0.95`). Go-red: revert the OR
— M2's fixture must FAIL.

**M2 — fixture (QEMU has no Kepler; pure arithmetic, same shape as `selftest_deliver` /
`selftest_period`, `:1362-1426`).** New `selftest_rearm()`: N=62 simulated vblanks; "hardware"
delivers an MSI iff the head enable is set; the simulated ISR mirrors the real one (rearm, or
disarm-only for the broken case). Fixed: deliveries ≈ N. Broken: deliveries == 1 — the flight-13-15
shape. Prints `:: kepler: vblank selftest arm=rearm … -> PASS|FAIL ::` and
`:: KVBLANK4: irq=<n> vbl=<n> ratio=<f> -> PASS|FAIL ::`.

**M3 — metal witness (Peter's call, not run this round).** Close line (`:1108`) on the bench after
M1: expect `irq≈vbl_delta` and `deliver=msi reason=none` every window. Go-red on the bench: revert
M1 for one boot, expect `irq=1` again.

## Spec pins

`unaos/scripts/specs/x86-witness.spec`, after the existing KVBLANK block (`:1643-1647`) — pins the
fixture's arithmetic, not the metal (QEMU has no Kepler):
```
REQUIRE :: kepler: vblank selftest arm=rearm .* :: PASS ::
FORBID  :: KVBLANK4: .* ratio=0\.0 .* -> PASS ::
```
No look-around; the FORBID guards a fixture that reports PASS at a near-zero ratio (the pre-fix
shape). A metal REQUIRE on `:: KVBLANK4: irq=<n> vbl=<n> ratio=<f> -> PASS ::` is owed to whatever
spec carries the next flight replay — not added here since this file only pins what QEMU runs.

## Open questions

1. Edge- or level-behaved per-head vblank line on the GK107 — safe to leave "armed" through the
   ISR, or can it re-trigger before EOI? Metal-only; `IRQ_STORM_CAP` is the fallback. Why M1 needs
   Peter's go before it runs on the bench.
2. After M1, should the vblank interrupt stay armed as a standing IRQ source for `video::beam`, or
   should `rung3_run`'s close (`:1077-1086`) still fully disarm at the bounded window's end as
   today? This doc scopes only the per-message re-arm inside that window.

## Next-session start

1. `grep -n "fn kepler_vblank_isr" -A25 unaos/crates/kernel/src/drivers/gpu/kepler_vblank.rs` —
   confirm line numbers, make the M1 edit.
2. Add `selftest_rearm()` beside `selftest_deliver`/`selftest_period`, wire it into their caller.
3. Add the two spec lines to `x86-witness.spec` after line 1647, then `cd unaos && ./arroyo check`
   and `UNAOS_KEPLER_VBLANK=1 UNAOS_BEAM=1 UNAOS_QEMU_FULL=1 ./arroyo test 240`.

## Draft code (unbuilt)

`kepler_vblank.rs`, inside `kepler_vblank_isr`, replacing the disarm write at `:1138-1142`:

```rust
// KVBLANK4: OR the vblank bit back in on the SAME write, instead of leaving the next arm to
// rung3_run's poll-driven check. Flight 13-15: irq=1 vbl_delta=62.
let vb = 1u32 << DISP_INTR_HEAD_BIT_VBLANK;
unsafe {
    mmio_write(bar0, disp_head(DISP_INTR_HOST_HEAD_EN, head), WIN_EN_ENTRY.load(Ordering::Relaxed) | vb);
    if n >= IRQ_STORM_CAP {
        mmio_write(bar0, regs::NV_PMC_INTR_EN, IRQ_PMC_ENTRY.load(Ordering::Relaxed));
        IRQ_STORMED.store(true, Ordering::Relaxed);
    }
}
```

`kepler_vblank.rs`, new fixture after `selftest_deliver` (`:1405`):

```rust
fn selftest_rearm() {
    const N: u32 = 62;
    fn run(rearm_in_isr: bool) -> u32 {
        let (mut armed, mut irq) = (true, 0u32);
        for _ in 0..N {
            if armed { irq += 1; armed = rearm_in_isr; }
        }
        irq
    }
    let (fixed, broken) = (run(true), run(false));
    let ratio = fixed as f32 / N as f32;
    let pass = fixed >= (N as f32 * 0.95) as u32 && broken == 1;
    serial_println!(":: kepler: vblank selftest arm=rearm fixed={} broken={} n={} :: {} ::",
        fixed, broken, N, if pass { "PASS" } else { "FAIL" });
    serial_println!(":: KVBLANK4: irq={} vbl={} ratio={:.3} -> {} ::",
        fixed, N, ratio, if ratio >= 0.95 { "PASS" } else { "FAIL" });
}
```
