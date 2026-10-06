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
    let disc = if picked { theme::bevel_light() } else { theme::title_text_inactive() };
    let figure = if picked { theme::accent() } else { theme::content_fill() };
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

// ── LOGINWINDOW (rmbp-ledger B430, MACPARITY row 33) — each user's OWN avatar ─────────────────────────────────────
//
// The source is the user's home directory's `una:icon` attribute (ASSOC's key, R79: no store of its own), its value
// an APPRES icon key drawn by APPRES's decoder; without one (or with a key APPRES does not know) the tile draws the
// user's INITIALS on a disc, theme tokens only. Resolved once per opening of the log-in form ([`resolve`]), never per
// repaint: the paint reads the table. Witness: `[login] window users=<n> picked=<name> avatar=<attr:KEY or initials>`.

const KEY_MAX: usize = 32;

#[derive(Clone, Copy)]
struct Src {
    name: [u8; crate::fs::users::NAME_MAX],
    nl: u8,
    key: [u8; KEY_MAX],
    kl: u8,
}

const SRC_NONE: Src = Src { name: [0; crate::fs::users::NAME_MAX], nl: 0, key: [0; KEY_MAX], kl: 0 };
static SRC: spin::Mutex<[Src; crate::fs::users::MAX_USERS]> = spin::Mutex::new([SRC_NONE; crate::fs::users::MAX_USERS]);

/// One drawing step a tile's avatar asks of its caller (which owns the surface and its clipping).
pub enum Draw<'a> {
    /// A logical-px rect.
    Fill(usize, usize, usize, usize, u32),
    /// Text centred in the logical rect `(x, y, w, h)`.
    TextIn(usize, usize, usize, usize, &'a [u8], u32),
    /// APPRES's icon of `key`, `d` logical px square at `(x, y)`; the caller answers whether it drew.
    Icon(usize, usize, usize, &'a str),
}

/// Read every user's `una:icon` (the log-in form's opening). Returns `(users, with_attr)`.
pub fn resolve() -> (usize, usize) {
    use crate::fs::users;
    let mt = crate::shell::vfs_mount_table();
    let n = users::count().min(users::MAX_USERS);
    let mut tab = [SRC_NONE; users::MAX_USERS];
    let mut with = 0;
    for (i, s) in tab.iter_mut().enumerate().take(n) {
        let mut nb = [0u8; users::NAME_MAX];
        let Some(nl) = users::name_at(i, &mut nb) else { continue };
        s.name = nb;
        s.nl = nl as u8;
        let mut hb = [0u8; users::HOME_MAX];
        let Some(hl) = users::home_of(&nb[..nl], &mut hb) else { continue };
        let Ok(home) = core::str::from_utf8(&hb[..hl]) else { continue };
        if let Ok(crate::fs::vfs::AttrValue::Str(k)) = mt.get_attr(home, crate::fs::assoc::ICON_KEY, crate::fs::vfs::KERNEL_PRINCIPAL) {
            if !k.is_empty() && k.len() <= KEY_MAX && crate::fs::appres::knows(&k) {
                s.key[..k.len()].copy_from_slice(k.as_bytes());
                s.kl = k.len() as u8;
                with += 1;
            }
        }
    }
    *SRC.lock() = tab;
    (n, with)
}

/// The avatar key resolved for `name` (`None` = initials).
fn key_of(name: &[u8], out: &mut [u8; KEY_MAX]) -> Option<usize> {
    let t = SRC.try_lock()?;
    let s = t.iter().find(|s| s.nl as usize == name.len() && &s.name[..name.len()] == name && s.kl > 0)?;
    out[..s.kl as usize].copy_from_slice(&s.key[..s.kl as usize]);
    Some(s.kl as usize)
}

/// The user's initials: the first letter, and the first after a `.` `_` `-` or space when there is one; upper-cased.
pub fn initials(name: &[u8], out: &mut [u8; 2]) -> usize {
    let up = |b: u8| b.to_ascii_uppercase();
    let Some(&first) = name.iter().find(|b| b.is_ascii_alphanumeric()) else { out[0] = b'?'; return 1 };
    out[0] = up(first);
    let second = name.iter().position(|b| matches!(b, b'.' | b'_' | b'-' | b' ')).and_then(|p| name[p + 1..].iter().find(|b| b.is_ascii_alphanumeric()));
    match second {
        Some(&b) => { out[1] = up(b); 2 }
        None => 1,
    }
}

/// Draw `name`'s avatar in the `d` x `d` square at (`x`, `y`): APPRES's icon of the user's `una:icon`, else the
/// initials on a disc. Returns the width it took (0 when `d` is too small to read).
pub fn tile_avatar(draw: &mut dyn FnMut(Draw) -> bool, name: &[u8], x: usize, y: usize, d: usize, picked: bool) -> usize {
    if d < 8 {
        return 0;
    }
    let mut kb = [0u8; KEY_MAX];
    if let Some(kl) = key_of(name, &mut kb) {
        if let Ok(k) = core::str::from_utf8(&kb[..kl]) {
            if draw(Draw::Icon(x, y, d, k)) {
                AVATARS.fetch_add(1, Ordering::Relaxed);
                return d + 3;
            }
        }
    }
    // the initials on a disc: picked = a light disc with accent letters on the accent tile, else the reverse
    let (disc, ink) = if picked { (theme::bevel_light(), theme::accent()) } else { (theme::accent(), theme::bevel_light()) };
    let r = d as isize / 2;
    for row in 0..d as isize {
        let dy = row - r;
        let span = isqrt((r * r - dy * dy).max(0) as usize) as isize;
        if span > 0 {
            let _ = draw(Draw::Fill(x + (r - span) as usize, y + row as usize, (2 * span) as usize, 1, disc));
        }
    }
    let mut ib = [0u8; 2];
    let il = initials(name, &mut ib);
    let _ = draw(Draw::TextIn(x, y, d, d, &ib[..il], ink));
    AVATARS.fetch_add(1, Ordering::Relaxed);
    d + 3
}

/// The avatar word for the wire: `attr:<key>` or `initials`.
pub fn avatar_word(name: &[u8], out: &mut [u8; 5 + KEY_MAX]) -> usize {
    let mut kb = [0u8; KEY_MAX];
    match key_of(name, &mut kb) {
        Some(kl) => {
            out[..5].copy_from_slice(b"attr:");
            out[5..5 + kl].copy_from_slice(&kb[..kl]);
            5 + kl
        }
        None => {
            out[..8].copy_from_slice(b"initials");
            8
        }
    }
}

/// `[login] window users=<n> picked=<name> avatar=<attr:KEY or initials>` — said on every pick (click or key).
pub fn glass_line(name: &[u8]) {
    let n = crate::fs::users::count();
    let shown = if !name.is_empty() && name.iter().all(|&b| (0x21..0x7f).contains(&b)) { core::str::from_utf8(name).unwrap_or("?") } else { "?" };
    let mut ab = [0u8; 5 + KEY_MAX];
    let al = avatar_word(name, &mut ab);
    serial_println!("[login] window users={} picked={} avatar={}", n, shown, core::str::from_utf8(&ab[..al]).unwrap_or("?"));
}

/// `tests loginwindow`: `:: LOGINWINDOW: users=<n> avatars=<n> power_row=<ok> keys=<ok> sleep=<armed or unarmed> -> PASS ::`.
/// `avatars` = tiles with a drawable avatar (attr or initials) out of the store's users; the power row and the
/// keyboard legs are login.rs's headless fixtures (nothing sleeps or powers off).
pub fn selftest() {
    let (users, with_attr) = resolve();
    let mut avatars = 0;
    for i in 0..users {
        let mut nb = [0u8; crate::fs::users::NAME_MAX];
        if let Some(nl) = crate::fs::users::name_at(i, &mut nb) {
            let mut drew = false;
            let w = tile_avatar(&mut |_| { drew = true; true }, &nb[..nl], 0, 0, 18, false);
            if w > 0 && drew {
                avatars += 1;
            }
        }
    }
    let row = super::login::power_row_selftest();
    let keys = super::login::keys_selftest();
    let ok = users > 0 && avatars == users && row && keys;
    serial_println!(
        ":: LOGINWINDOW: users={} avatars={} attr={} power_row={} keys={} sleep={} -> {} ::",
        users, avatars, with_attr, if row { "ok" } else { "FAIL" }, if keys { "ok" } else { "FAIL" },
        if crate::power::sleep_armed() { "armed" } else { "unarmed" }, if ok { "PASS" } else { "FAIL" }
    );
}

/// Register `tests loginwindow` (once; the login form's first opening calls it — R80: nothing runs at boot).
pub fn register() {
    static REG: AtomicBool = AtomicBool::new(false);
    if !REG.swap(true, Ordering::AcqRel) {
        crate::tests::register("loginwindow", selftest);
    }
}
