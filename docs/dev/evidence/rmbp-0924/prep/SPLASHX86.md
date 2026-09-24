# SPLASHX86 — prep

## The finding

rmbp-ledger B6: "Cross-arch splash: `splash.rs` stays, call sites x86-gated; the `bootpace.rs:168`
same-line trap" — status open, "Peter asked by name". QUEUE.md §4: "B6  cross-arch splash:
`splash.rs` stays, its call sites go x86-gated (Peter asked by name)." `fc2.registry:16` (GATE-FC2's
own note): "Whether the right end state is a gated declaration or a splash that finally runs on both
arches is B6's question... The fix, when that is ruled, is ONE line: fold
`#[cfg(target_arch = "x86_64")]` onto `lib.rs:189`. Owner: the seat that holds B6."

## Mechanism

- `unaos/crates/kernel/src/lib.rs:189` — `pub mod splash;` is declared with **no** cfg gate at all
  (unconditional `pub mod`), annotated with an FC-2 comment explaining GATE-FC2 measured refs=3, all
  under `target_arch = "x86_64"`, and deliberately left ungated pending B6's ruling.
- `unaos/crates/kernel/src/splash.rs:32` — the file itself carries `#![cfg(target_arch = "x86_64")]`
  as an inner attribute, so its body already compiles to nothing on aarch64 regardless of the
  declaration. Doc comment (`splash.rs:16-29`) confirms: "x86 GUI builds only", drawn pre-heap from
  `kernel_main`, before ACPI/SMP/xHCI bring-up, i.e. before the Kepler takeover.
- All three call sites are ALREADY x86-gated, JOB done per B6's row and per fc2.registry's own
  measurement:
  - `unaos/crates/kernel/src/main.rs:224-234` — `#[cfg(all(target_arch = "x86_64", not(any(feature =
    "usbdebug", feature = "bootlog", feature = "witness"))))]` around the `boot_splash` call
    (SPLASH-1, before ACPI/SMP/xHCI).
  - `unaos/crates/kernel/src/bootpace.rs:168` — the "same-line trap": `crate::splash::advance(tag)`
    inlined on the SAME line as `r.len = n + 1`, under
    `#[cfg(all(target_arch = "x86_64", not(any(feature = "usbdebug", feature = "bootlog", feature =
    "witness"))))]`, deliberately not on its own line so following fns' `panic::Location` line
    numbers don't shift on knob-off builds. Ledger row itself says this trap "is now gated by
    GATE-APPEND (B93)" — i.e. already covered, not part of this round's edit.
  - `unaos/crates/kernel/src/video/fbcon.rs:1543-1554` — the whole `milestone()` early-return block
    (including the `crate::splash::active()` check at :1554) sits under `#[cfg(all(target_arch =
    "x86_64", not(any(feature = "usbdebug", feature = "witness"))))]`.
- **aarch64 side**: `unaos/crates/kernel/src/video/desktop_firmware.rs` (597 lines) has NO
  splash/logo/boot-image equivalent — grepped for `splash|logo|boot.*paint|pre-GUI` and found nothing
  matching a boot logo. The Pi4/Orin boards paint nothing before `desktop_firmware`'s own init; there
  is no aarch64 analog to gate or test.
- GATE-FC2 (`unaos/scripts/fc2-check.sh`, `unaos/scripts/fc2.registry:16`) is the census that flags
  `splash` today: it walks declaration cfgs, not inner-file `#![cfg]`, so an unconditional `pub mod
  splash;` whose 3 refs are all x86-only reads as an FC-2 shape even though the file guts itself out
  on aarch64. It is REGISTERED (not failing) only because B6 is open and Peter named it.
- An existing behavioral gate already covers the splash on x86:
  `unaos/scripts/specs/x86-splash.spec` (SPLASHGATE, B149), replayed by `./arroyo test-splash 120`
  under `UNAOS_SPLASHLANE=1 UNAOS_WC=1 UNAOS_QEMU_FULL=1` (witness deliberately unset). It already
  pins `:: SPLASH: crystal cluster traced — 3 shards, N spectrum rays ::` and `:: SPLASH: retired at N
  ms by bootpace gui stamp ... ::`, with FORBIDs against the wrong retirement site and against `0 ms`.
  It also cross-checks the artifact ELF itself (`splash_artifact_cert` in `arroyo`) for both strings,
  not just the replay — the pattern this brief's witness section should reuse.

## Plan

- **M1** — Fold the declaration gate. File: `unaos/crates/kernel/src/lib.rs:189`. Change
  `pub mod splash;` to `#[cfg(target_arch = "x86_64")] pub mod splash;` on the SAME line (no line
  added — keeps `panic::Location` numbers byte-identical on knob-off builds, matching the discipline
  `bootpace.rs:168` already uses). This is a no-op for compiled output on both arches (aarch64 already
  drops the body via the inner `#![cfg]`; x86_64 is unaffected) — it only makes the declaration's cfg
  match its 3 call sites so GATE-FC2 stops needing the registry exception. Requires B6 to be RULED
  first (see Open questions) — this is the "one folded line" the fc2.registry note already names.
  No witness line: this is a structural/compile-time change with no new runtime behavior; the
  existing `x86-splash.spec` REQUIRE/FORBID pair (already passing on x86, already absent on aarch64
  since the module can't exist there) is the fixture that must stay green, unchanged.
  Go-red mutation: revert the fold (restore unconditional `pub mod splash;`) and re-run
  `unaos/scripts/fc2-check.sh` — it must go back to flagging `splash` as measured, or list it in
  `fc2.registry` again; either way the absence of the fold must be visible on the gate, not silent.
- **M2** — Retire the `fc2.registry:16` exception line for `splash` once M1 lands and B6 is closed in
  `docs/dev/RULINGS.md` (flagging it so the next session doesn't leave a stale registry entry).
- **M3** (only if Peter rules the OTHER way — cross-arch splash) — out of scope for a "call sites go
  x86-gated" ruling; if ruled as "run it on both arches" instead, this is much bigger: port
  `splash.rs`'s Q16.16 ray tracer (its only x86 dependency is the inner `#![cfg]` itself — the math is
  already arch-neutral), wire 3 new call sites into `desktop_firmware.rs`'s aarch64 boot path, and add
  an aarch64 witness spec. Not sized here; see Open questions.

## Draft code (unbuilt)

```rust
// unaos/crates/kernel/src/lib.rs — anchor: the existing line 189
// BEFORE:
pub mod splash; // FC-2 (GATE-FC2, 2026-09-15): measured as an instance — refs=3, ALL under target_arch="x86_64" ...
// AFTER (fold, same line, no line count change):
#[cfg(target_arch = "x86_64")] pub mod splash; // FC-2 (GATE-FC2): declaration now matches its 3 x86-only call sites (B6 ruled <date/sha>); see fc2.registry history for the prior exception.
```

## Spec pins

No new spec needed — `unaos/scripts/specs/x86-splash.spec` already pins the behavior this ARC must
not regress. Re-affirming its existing lines as the pins this round is scored against:

```
REQUIRE :: SPLASH: crystal cluster traced — 3 shards, \d+ spectrum rays ::
REQUIRE :: SPLASH: retired at \d+ ms by bootpace gui stamp \(main\.rs, before the desktop's first paint\) ::
FORBID  :: SPLASH: retired at \d+ ms by video/fbcon\.rs panel_console_resume
FORBID  :: SPLASH: retired at 0 ms by
```

If GATE-FC2 itself needs a REQUIRE/FORBID pin against regressing the M1 fold (i.e. against the
declaration drifting unconditional again), that is a `fc2-check.sh` census assertion, not a boot-log
spec — no `REQUIRE`/`FORBID` line applies to it since it isn't a serial-line fixture. No look-around
needed or used above (plain literal + `\d+`/`.*` only, matching the house style).

## Open questions

- Has Peter actually ruled B6 yet, one way or the other? `docs/dev/RULINGS.md` has zero hits for
  "B6" or "splash" as of this checkout (4c1c1d75) — the ledger row still reads "open ... Peter asked
  by name." M1 should not land until a ruling exists; someone must ask/confirm which reading of
  "cross-arch splash" is meant: (a) keep splash x86-only, just gate the declaration to match (small,
  M1 as drafted), or (b) make the splash actually run on aarch64 too (M3, unscoped, much larger).
- Is `docs/dev/RULINGS.md` the only place a ruling would be recorded, or could it already be implicit
  in QUEUE.md's phrasing ("its call sites go x86-gated") — i.e. is QUEUE.md's line itself the de
  facto ruling for reading (a), just not yet mirrored into RULINGS.md? A person should decide whether
  that's sufficient to proceed or whether it still needs a formal RULINGS.md entry before M1 lands.

## Next-session start

1. Confirm B6's ruling in `docs/dev/RULINGS.md` (grep `B6\|splash`); if absent, get Peter's answer
   to the (a)/(b) fork above before touching code.
2. If (a): apply the M1 fold at `unaos/crates/kernel/src/lib.rs:189` exactly as drafted above, then
   `./arroyo check` (both arches) and confirm `unaos/scripts/fc2-check.sh` no longer needs the
   `splash` line in `unaos/scripts/fc2.registry` (remove that registry line in the same commit).
3. Run `./arroyo test-splash 120` and diff against `unaos/scripts/specs/x86-splash.spec`'s existing
   REQUIRE/FORBID set to confirm no regression (this is the fixture already on a wire — it should
   stay exactly green, not need new pins).
