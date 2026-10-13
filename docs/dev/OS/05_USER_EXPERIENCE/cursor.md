# CURSOR — one pointer on the glass, and its shape

Status: TWOCURSORS (x86) and AETHERPTR **flown on image 25** (flight 33); CURSORARM, AETHERPTR2 and CURSORSHAPE
fixed-unflown (cut from abcb8d33, after the image-25 fold 34927a4c, whose seven arcs FLIGHT33.md:3 lists). Code:
`unaos/crates/kernel/src/video/cursor.rs` (the sprite, `compose_into`, `adopt_overlay`, the TWOCURSORS tail, the
`SHAPES` table), `video/cursorshape.rs` (`tests cursorshape`), `video/theme.rs` (`POINTER_SHAPE_INK`, :1115), the
two `arch/*/syscall.rs` (`pack_input`, `user_input_set_active`, SYS_CURSOR_SHAPE), `una-abi/src/lib.rs`. Ledger
rows (`docs/dev/OS/rmbp-ledger.md`): B542 TWOCURSORS (575), B543 AETHERPTR (576), B547 AETHERPTR2 (583), B548
CURSORARM (582), B550 CURSORSHAPE (585). Pointer motion itself is [`trackpad.md`](trackpad.md).

Flight 32 boot 2, Aether open, Peter: *"there's 2 mouse cursors"* (FLIGHT32.md:12). Two separate causes, two arcs.

## 1. The compositor's orphan arrow (TWOCURSORS, B542)

**Wire, flight 32** (FLIGHT32.md:34; the pinned captures end before these lines, B542):
`[cursor] restore src=flat rect=188x236+691+1360 n=1278 scene=0 flat=1278 deferred=610`,
`[wedge9] sprite-claim scope=desk refused=355 masked=54 retried=6 -> DEFERRED`, `[cursor11] compose-through
px_deferred=349342 px_absorbed=799` (the last as quoted in B542).

**Cause (B542, read from the code).** Only a sprite-loan holder may draw the arrow — except CURSOR-3's
`compose_into`, which draws under the overlay loan only. Its `live_epoch() != plan.epoch` guard narrows, but does
not close, the race with another core's undraw/redraw: a stale-plan arrow lands at the OLD origin after that core
restored it. `adopt_overlay` then DECLINES the install (incoherent, or a refused sprite/overlay claim), resets the
session and refreshes only the LIVE box. The plan's arrow has no owner, no save and no damage, and stands over a
window that does not re-present — a static Aether page. The 188x236 rect is not a window: it is CURSORBG's
`restore_note` union of the live box with every 18x18 handback the 8 ms floor deferred, the bounding box of a sweep.

**Fix.** `adopt_overlay` restores every covered arrow pixel at the SESSION geometry from `ov.saved`
(colour-guarded, both loans held, before the `refresh_locked` every declined tail runs) and hands the session box to
`repair`; a refused sprite or overlay claim hands the box (published lock-free by `compose_into`) to `repair` so the
window re-presents over the orphan. A refused claim on the module's own sprite still keeps it PUT until a claim
lands — one image, late, never two.

**Wire, flight 33** (`docs/dev/evidence/rmbp-0915/flight33/f33-boot2.log`): QUIET until Aether opens (`[dock] launch
app=aether first_window_ms=863`, :2266), then the repair working:

```
:2330  [06:02:42Z] [wedge9] sprite-claim scope=desk refused=61 masked=4 retried=2 owed=0 serviced=39 -> DEFERRED
:2332  [06:02:42Z] [cursorghost] orphan scope=desk declined=7 restored_px=636 restored_on_refuse=0 damaged=7 -> RESTORED
:4666  [06:10:42Z] [cursorghost] orphan scope=desk declined=99 restored_px=11892 restored_on_refuse=24 damaged=123 -> RESTORED
```

`damaged = declined + restored_on_refuse` holds on both (7 = 7 + 0; 123 = 99 + 24). Sixty `[cursorghost]` lines that
boot. FLIGHT33.md carries no second-pointer report from the glass. The race itself remains; what changed is that its
leftover is restored and damaged instead of orphaned.

**aarch64 (CURSORARM, B548).** `compose_into` (cursor.rs:3681) and `adopt_overlay` (:3836) and everything the tail
leans on are ungated and compile into the aarch64 `desktop_firmware` image — only the gates kept the Orin out. The
gates widen from `all(x86_64, wc)` to `any(all(x86_64, wc), all(aarch64, desktop_firmware))`, and `[cursorghost]`
prints on the `[wedge9]` cadence there (`wedge9_rollup`, cursor.rs:528, not `display_tegra.rs` — B548 refuted that
premise). Unflown; witness owed on the Orin bench.

## 2. The program's own arrow (AETHERPTR B543, AETHERPTR2 B547)

**Cause.** AETHER.ELF drew its OWN pointer (an arrow, and a hand over a link) at a position it estimated from
MOUSE_REL deltas and clamped to its window; UNA.ELF did the same. The x86 INPUT_EV producer handed ring 3 no position
it could map — a relative report as the delta alone, a tablet report as the raw 0..=32767 coordinate (`pack_input`).
The compositor sprite and the program's arrow both showed and drifted apart (B543).

**Contract** (`una_abi::INPUT_EV_MOUSE_ABS` = 4, una-abi/src/lib.rs:424): x/y are the system pointer in the
receiving program's surface px — panel point minus the content origin of the owner's frontmost window, divided by its
upscale, unclamped; a windowless program gets panel px. The producer pushes one after every relative report it
delivers and sends every tablet report that way. Aether and Una hit-test from it and paint no pointer: **the system
sprite is the one pointer.**

**AETHERPTR2 (unflown).** (a) aarch64's `pack_input` still forwarded the device coordinate; x86's tail fns are
ported (`ptr_win_origin`, `ptr_win_px`, `ptr_abs_payload_hid`, `ptr_abs_follow`; owner = the ASID), all under
`desktop_firmware`. (b) On both arches a press that MOVES focus landed in a ring `user_input_set_active` had just
cleared, so the program hit-tested at its last point; one MOUSE_ABS is now written at focus arrival
(`ptr_abs_arrival`). Not covered: x86's first-window focus grant in `sys_win_create` bypasses
`user_input_set_active` and gets no MOUSE_ABS; an aarch64 image without `desktop_firmware` still forwards the device
coordinate (B547).

## 3. The pointer's shape (CURSORSHAPE, B550)

AETHERPTR left the hand over a link ABSENT: with one system sprite, a program needs a way to ask for a shape. R108's
pane contract, its input half.

- **The verb.** `SYS_CURSOR_SHAPE(shape)` = 69 (una-abi/src/lib.rs:2506, after SYS_SPAWN_PATH 68) → 0 / -EINVAL.
  Shapes: ARROW 0, HAND 1, TEXT 2, CROSSHAIR 3, MOVE 4, RESIZE_NS 5, RESIZE_EW 6, BUSY 7; `CURSOR_SHAPES` = 8 is the
  first -EINVAL (lib.rs:2508–2520; B550).
- **Who it applies to.** Fulfilled on both arches (x86 keyed by slot+1, aarch64 by ASID); the shape is stored on the
  owner's `procslot::SlotVec` row and published to the sprite ONLY for the caller holding focus. Every focus write
  resets the LEAVER to the arrow and shows the ARRIVER's stored shape; a teardown revoke shows the arrow (B550).
- **One table.** `SHAPES` (cursor.rs:5213): 8x8 mask, hotspot, inks from `POINTER_SHAPE_INK`; the text beam is
  inverted so it reads on a white page. The glyph is LATCHED inside `draw_locked` (cursor.rs:4107) with the sprite
  down, so every restore walks the glyph it drew.
- **Callers.** Aether asks for the hand over a link and the beam over selectable text; Una the beam over its editor
  pane, on every pointer event.
- **Fixture.** `tests cursorshape` (cursorshape.rs): -EINVAL leaves the index, HAND moves it arrow→hand, a focus
  change puts it back: `:: CURSORSHAPE: set=hand index=<before>-><after> einval=<rc> focus_away=<name> rows=<n>
  distinct=<yes|no> -> PASS|FAIL ::`. It cannot see pixels — the latch runs only inside a draw, and QEMU delivers no
  pointer report (cursorshape.rs header). B550 owed its registration on `tests.rs`'s ensure line; this tree carries
  it (`crate::video::cursor::cursorshape::ensure();`, tests.rs:149).

## 4. Open

- The glass for CURSORSHAPE: every glyph but the arrow and crosshair is invented ("theme.rs PA38: the kit defines no
  pointer", B550) and is Peter's to judge.
- The Orin witness for CURSORARM (`[cursorghost]` after each `[wedge9]`, pointer over a static window).
- `compose_into`'s race is narrowed and repaired, not removed (§1).
