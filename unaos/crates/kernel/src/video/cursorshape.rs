//! CHARTER: Kernel — compositor seam
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CURSORSHAPE (rmbp-ledger B550; R108 — the pane contract's input half) — `tests cursorshape`.
//!
//! The question the row asks: does a shape the focused owner asks for reach the SPRITE TABLE, and does a focus
//! change take it back to the arrow? Answered through the two real seams, not a model of them:
//!
//! 1. **The verb's body.** `crate::arch::syscall::cursor_shape_apply(key, shape)` is `SYS_CURSOR_SHAPE`'s body
//!    with the caller's focus key passed in (x86: `slot + 1`; aarch64: the ASID; 0 = the shell). It is called with
//!    the key that holds focus RIGHT NOW, so the owner it acts for is the real focused owner — no focus is taken
//!    from anyone. The fixture requires the shell to hold focus (key 0, the state `tests` is typed in); with a
//!    program focused it refuses with `reason=focus-held` rather than steal the keyboard from it.
//! 2. **The focus hook.** `crate::arch::syscall::user_input_set_active(0)` — the one function every focus change on
//!    both arches funnels through — re-asserting the shell's own focus. With focus already at the shell this moves
//!    no keystroke (x86: one store; aarch64: one store and a takeover latch that is already clear for the shell),
//!    and it runs the CURSORSHAPE hook exactly as a real change does: the incoming owner's stored shape (the
//!    shell's: always the arrow) is published.
//!
//! Legs, in order: an out-of-range shape is `-EINVAL` and moves nothing; HAND sets the table index from the arrow's
//! to the hand's (and the hand's row is a different glyph from the arrow's, so the sprite really changes); a focus
//! change puts the index back to the arrow. One verdict line:
//!
//! `:: CURSORSHAPE: set=hand index=<before>-><after> einval=<rc> focus_away=<name> rows=<n> distinct=<yes|no>
//!   -> PASS|FAIL ::` (FAIL carries `reason=` first).
//!
//! Not this fixture: the PIXELS. The latch only runs inside a draw, and a draw needs a pointer report (QEMU
//! delivers none — CURSOR-1's arming rule), so what is on the glass is the metal flight's to see.

use core::sync::atomic::{AtomicBool, Ordering};

/// Register `tests cursorshape` once (R77 M3: a fixture registers, the operator fires it).
pub fn ensure() {
    static ONCE: AtomicBool = AtomicBool::new(false);
    if !ONCE.swap(true, Ordering::AcqRel) {
        crate::tests::register("cursorshape", selftest);
    }
}

/// `tests cursorshape` — see the module doc.
pub fn selftest() {
    use super::{shape_index, shape_name, SHAPES};
    let rows = SHAPES.len();
    // Every row a different, non-empty glyph — "the sprite table index changes" must mean the pixels do.
    let mut distinct = true;
    for a in 0..rows {
        if SHAPES[a].mask.iter().all(|&m| m == 0) {
            distinct = false;
        }
        for b in (a + 1)..rows {
            if SHAPES[a].mask == SHAPES[b].mask && SHAPES[a].hot == SHAPES[b].hot {
                distinct = false;
            }
        }
    }
    let focus = crate::arch::syscall::user_input_active();
    if focus != 0 {
        serial_println!(
            ":: CURSORSHAPE: -> FAIL :: reason=focus-held focus={} (run from the shell: the fixture acts for the focused owner and will not take the keyboard from a program)",
            focus
        );
        return;
    }
    let before = shape_index();
    let einval = crate::arch::syscall::cursor_shape_apply(focus, una_abi::CURSOR_SHAPES + 1);
    let after_bad = shape_index();
    let rc = crate::arch::syscall::cursor_shape_apply(focus, una_abi::CURSOR_HAND);
    let after = shape_index();
    crate::arch::syscall::user_input_set_active(0);
    let away = shape_index();
    let hand_differs = SHAPES[una_abi::CURSOR_HAND as usize].mask != SHAPES[una_abi::CURSOR_ARROW as usize].mask;
    let reason = if einval != una_abi::EINVAL || after_bad != before {
        "einval"
    } else if rc != 0 || after != una_abi::CURSOR_HAND as u8 || after == before || !hand_differs {
        "set"
    } else if away != una_abi::CURSOR_ARROW as u8 {
        "focus-away"
    } else if !distinct || rows != una_abi::CURSOR_SHAPES as usize {
        "table"
    } else {
        ""
    };
    if reason.is_empty() {
        serial_println!(
            ":: CURSORSHAPE: set=hand index={}->{} einval={} focus_away={} rows={} distinct=yes -> PASS ::",
            before,
            after,
            einval,
            shape_name(away),
            rows
        );
    } else {
        serial_println!(
            ":: CURSORSHAPE: -> FAIL :: reason={} set=hand index={}->{} rc={} einval={} focus_away={} rows={} distinct={}",
            reason,
            before,
            after,
            rc,
            einval,
            shape_name(away),
            rows,
            if distinct { "yes" } else { "no" }
        );
    }
}
