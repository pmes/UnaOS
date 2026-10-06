//! CHARTER: Kernel — wm (FIRSTUSER B409, R100: the login window's users list — the generic avatar on each user's tile, the lock screen's same look, and the power-row hook DIALOG2 sets)
//!
//! R100 (Peter, 2026-10-06): "we can move to the mac boot process since i don't expect root will be used by people".
//! Later boots show the LOGIN WINDOW: the users with their avatars, a password field, and the power row (DIALOG2,
//! B404). `login.rs` owns the window, its geometry and its presses; this file owns what a user's tile SHOWS
//! beside the name (the generic avatar — APPRES-style per-user pictures later) and the window's self-description
//! the `:: FIRSTUSER:` witness reads (`users`, `power`). Declared from `crystal.rs`'s tail beside `login`.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::video::theme;

/// DIALOG2's power row (Sleep / Restart / Shut Down) is on the login window: its row sets this at its first paint
/// (one line at the fold). Until it does the witness says `login_window=users` and FAILs on `power`.
pub static POWER_ROW: AtomicBool = AtomicBool::new(false);
/// Avatars drawn this boot (the witness's evidence the tiles carry them).
static AVATARS: AtomicU32 = AtomicU32::new(0);

/// Draw the generic avatar — a disc with a head and shoulders — in the `d` x `d` square at (`x`, `y`) through the
/// caller's `fill` (logical px, the caller clips). Returns the width it took (0 when `d` is too small to read).
pub fn avatar(fill: &mut dyn FnMut(usize, usize, usize, usize, u32), x: usize, y: usize, d: usize, picked: bool) -> usize {
    if d < 8 {
        return 0;
    }
    let disc = if picked { theme::BEVEL_LIGHT } else { theme::TITLE_TEXT_INACTIVE };
    let figure = if picked { theme::ACCENT } else { theme::CONTENT_FILL };
    let r = d as isize / 2;
    let (cx, cy) = (r, r);
    // the disc, one horizontal span per row (r² - dy² ≥ dx²)
    for row in 0..d as isize {
        let dy = row - cy;
        let span = isqrt((r * r - dy * dy).max(0) as usize) as isize;
        if span > 0 {
            fill(x + (cx - span) as usize, y + row as usize, (2 * span) as usize, 1, disc);
        }
    }
    // the head: a disc of radius r/3 centred a third of the way down
    let hr = (r / 3).max(1);
    let hy = cy - r / 4;
    for row in (hy - hr)..=(hy + hr) {
        let dy = row - hy;
        let span = isqrt((hr * hr - dy * dy).max(0) as usize) as isize;
        if span > 0 && row >= 0 {
            fill(x + (cx - span) as usize, y + row as usize, (2 * span) as usize, 1, figure);
        }
    }
    // the shoulders: a half-disc of radius r*2/3 rising from the bottom, clipped to the outer disc
    let sr = (r * 2 / 3).max(1);
    let sy = cy + r * 3 / 4;
    for row in (sy - sr)..d as isize {
        let dy = row - sy;
        let s_span = isqrt((sr * sr - dy * dy).max(0) as usize) as isize;
        let dyo = row - cy;
        let o_span = isqrt((r * r - dyo * dyo).max(0) as usize) as isize;
        let span = s_span.min(o_span);
        if span > 0 && row >= 0 {
            fill(x + (cx - span) as usize, y + row as usize, (2 * span) as usize, 1, figure);
        }
    }
    AVATARS.fetch_add(1, Ordering::Relaxed);
    d + 3
}

fn isqrt(v: usize) -> usize {
    let mut x = 0usize;
    while (x + 1) * (x + 1) <= v {
        x += 1;
    }
    x
}

/// The login window as the witness says it: `users+power`, `users`, `power` or `none`, and whether it is whole.
/// `users` = the roster is on the glass and the store has users (each tile carries the avatar above).
pub fn word() -> (&'static str, bool) {
    let users = super::login::roster_on_glass() && crate::fs::users::user_count() > 0;
    let power = POWER_ROW.load(Ordering::Acquire);
    let w = match (users, power) {
        (true, true) => "users+power",
        (true, false) => "users",
        (false, true) => "power",
        (false, false) => "none",
    };
    (w, users && power)
}

/// Avatars drawn this boot (`tests firstuser` evidence; 0 before the window's first paint).
pub fn avatars_drawn() -> u32 {
    AVATARS.load(Ordering::Relaxed)
}
