// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! LOGIN M3 — the login screen (`login` knob, RULINGS R51): the desktop boots to it, a successful
//! login opens the session under `user:<name>` (`fs::users::login`, which also makes `/home/<name>`),
//! and Log Out (M4, the crystal menu) returns to it. Self-drawn, the Mac model: a name field, a
//! password field, Enter = log in — in the shape of `video/instgui.rs`: one cached-RAM surface, one
//! `wm` window, and every key route in `main.rs` offers the key to [`consume_key`] BEFORE its own
//! serial echo and before the console's `handle_key`, so while the screen is up the keyboard belongs
//! to it: the shell gets nothing and NO TYPED BYTE REACHES THE WIRE (a password is typed here).
//!
//! ONE OS: declared from `video/crystal.rs`'s tail under the same gate as the crystal (x86 `wc` or
//! aarch64 `desktop_firmware`); no board name, no `target_arch` of its own. The routes reach it through
//! `fs::users::screen_key` / `screen_open_once`, which are `false`/no-op where no desktop is built.
//!
//! First boot with no users (`users::count() == 0`) is the CREATE-FIRST-USER screen: the same two
//! fields, titled so; Enter creates the user and logs in. R24: Esc does nothing here — it dismisses
//! menus only, and this is not a menu; it never closes an app window. Tab moves between the two fields
//! of ONE form (a form control inside one window, not the retired window focus-cycle).
//!
//! HEADLESS: where `wm` can name no surface (`spawn_geometry` = None), or under the fixture, the screen
//! runs its state machine without a window — the fixture drives it through the same [`consume_key`]
//! the routes call and the witness says `window=no`. The fixture NEVER opens a window: on the shared
//! QEMU ladder a window and a console-present suspend mid-battery would perturb the compositor legs,
//! so the drawing is proven on the glass and the state machine on the ladder.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::super::{fbcon, theme, wm};
use crate::fs::users;

const W: usize = 440;
const H: usize = 240;
const TS: usize = 2;
const CELL: usize = 8 * TS;
const FIELD_MAX: usize = 32;

#[repr(align(64))]
struct Surf([u32; W * H]);
/// SAFETY: written only from `repaint` on the thread that owns keys (the route that called
/// `consume_key`, or the ignition that called `open_once`), read by `wm`'s composite — the same benign
/// present-tear every app surface shares (instgui's contract).
static mut SURF: Surf = Surf([0; W * H]);

#[derive(Clone, Copy, PartialEq)]
enum State {
    Closed,
    /// The form is up (create-first-user or log-in, decided by `users::count()` at paint time).
    Open,
    /// A session is open; the screen is down until Log Out.
    Session,
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Name,
    Password,
}

struct Form {
    state: State,
    focus: Focus,
    name: [u8; FIELD_MAX],
    name_len: usize,
    pw: [u8; FIELD_MAX],
    pw_len: usize,
    message: &'static str,
    /// The screen was drawn with a real `wm` window (else headless).
    windowed: bool,
}

static FORM: spin::Mutex<Form> = spin::Mutex::new(Form {
    state: State::Closed,
    focus: Focus::Name,
    name: [0; FIELD_MAX],
    name_len: 0,
    pw: [0; FIELD_MAX],
    pw_len: 0,
    message: "",
    windowed: false,
});
static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
static OPENED_ONCE: AtomicBool = AtomicBool::new(false);
/// Set for the fixture's duration: `open` never names a surface (see the module doc).
static HEADLESS: AtomicBool = AtomicBool::new(false);
/// Sessions opened through this screen (a counter for the witness).
static LOGINS: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// drawing (the instgui primitives, on this surface)
// ---------------------------------------------------------------------------

fn fill(px: &mut [u32], x: usize, y: usize, w: usize, h: usize, c: u32) {
    for row in y..(y + h).min(H) {
        let base = row * W;
        for col in x..(x + w).min(W) {
            px[base + col] = c;
        }
    }
}

fn rect(px: &mut [u32], x: usize, y: usize, w: usize, h: usize, c: u32) {
    fill(px, x, y, w, 1, c);
    fill(px, x, y + h - 1, w, 1, c);
    fill(px, x, y, 1, h, c);
    fill(px, x + w - 1, y, 1, h, c);
}

fn text(px: &mut [u32], x: usize, y: usize, s: &[u8], fg: u32) {
    let mut cx = x;
    for &ch in s {
        if cx + CELL > W {
            break;
        }
        let bitmap = font8x8::legacy::BASIC_LEGACY[ch.min(127) as usize];
        for (ry, rowbits) in bitmap.iter().enumerate() {
            for rx in 0..8 {
                if rowbits & (1 << rx) != 0 {
                    fill(px, cx + rx * TS, y + ry * TS, TS, TS, fg);
                }
            }
        }
        cx += CELL;
    }
}

fn field(px: &mut [u32], x: usize, y: usize, w: usize, content: &[u8], focused: bool, secret: bool) {
    fill(px, x, y, w, CELL + 8, theme::CONTENT_FILL);
    rect(px, x, y, w, CELL + 8, if focused { theme::ACCENT } else { theme::FRAME_LINE });
    let n = content.len().min(FIELD_MAX);
    if secret {
        let dots = [b'*'; FIELD_MAX];
        text(px, x + 6, y + 4, &dots[..n], theme::CONTENT_TEXT);
    } else {
        text(px, x + 6, y + 4, content, theme::CONTENT_TEXT);
    }
    if focused {
        let cx = x + 6 + n * CELL;
        fill(px, cx, y + 4, 2, CELL, theme::CONTENT_TEXT);
    }
}

fn repaint() {
    let f = FORM.lock();
    if !f.windowed {
        return; // headless: the state machine is the whole screen
    }
    // SAFETY: see `SURF`.
    let px: &mut [u32] = unsafe { &mut (*core::ptr::addr_of_mut!(SURF)).0 };
    fill(px, 0, 0, W, H, theme::CHROME_FACE);
    rect(px, 2, 2, W - 4, H - 4, theme::FRAME_LINE);
    let lx = 24;
    let first = users::count() == 0;
    let title: &[u8] = if first { b"Create the first user" } else { b"Log in to UnaOS" };
    text(px, lx, 20, title, theme::CONTENT_TEXT);
    fill(px, lx, 42, W - 2 * lx, 2, theme::FRAME_LINE);
    text(px, lx, 62, b"Name", theme::TITLE_TEXT_INACTIVE);
    field(px, lx + 100, 56, W - 2 * lx - 100, &f.name[..f.name_len], f.focus == Focus::Name, false);
    text(px, lx, 102, b"Password", theme::TITLE_TEXT_INACTIVE);
    field(px, lx + 100, 96, W - 2 * lx - 100, &f.pw[..f.pw_len], f.focus == Focus::Password, true);
    let hint: &[u8] = if first { b"Enter creates the user and logs in" } else { b"Enter logs in   Tab switches field" };
    text(px, lx, 150, hint, theme::TITLE_TEXT_INACTIVE);
    if !f.message.is_empty() {
        text(px, lx, 190, f.message.as_bytes(), theme::ACCENT);
    }
    drop(f);
    let id = WIN.load(Ordering::Relaxed);
    if id != wm::WIN_NONE {
        let _ = wm::present(id);
    }
}

// ---------------------------------------------------------------------------
// lifecycle
// ---------------------------------------------------------------------------

/// Open the screen at desktop ignition, once per boot; later opens are Log Out's (`reopen`).
pub fn open_once() {
    if OPENED_ONCE.swap(true, Ordering::AcqRel) {
        return;
    }
    open();
}

fn open() {
    {
        let mut f = FORM.lock();
        if f.state == State::Open {
            return;
        }
        f.state = State::Open;
        f.focus = Focus::Name;
        f.name_len = 0;
        f.pw_len = 0;
        f.message = "";
        f.windowed = false;
    }
    if HEADLESS.load(Ordering::Relaxed) {
        serial_println!("[login] screen open window=no (fixture — headless form)");
        return;
    }
    let Some((_s, ow, oh)) = wm::spawn_geometry(W, H) else {
        serial_println!("[login] screen open window=no (no surface yet — headless form)");
        return;
    };
    let (pw, ph) = {
        let fb = *super::super::WRITER.lock();
        let i = fb.info();
        (i.width, i.height)
    };
    let ox = pw.saturating_sub(ow) / 2;
    let oy = ph.saturating_sub(oh) / 3; // upper-third centre: reads as a dialog (instgui's placement)
    // Paint the form BEFORE the window names the surface (instgui's order), so the first composite
    // never shows a blank box.
    FORM.lock().windowed = true;
    repaint();
    let surf = core::ptr::addr_of_mut!(SURF) as usize;
    let id = wm::create_at(0, surf, W * H * 4, W as u32, H as u32, (W * 4) as u32, b"Log in", ox + wm::BORDER, oy + wm::TITLE_H + wm::BORDER);
    if id == wm::WIN_NONE {
        FORM.lock().windowed = false;
        serial_println!("[login] screen open window=no (create refused — headless form)");
        return;
    }
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "login");
    // Modal over the glass: the console keeps taking glyphs, serial keeps every line, but it stops
    // presenting until the session opens (instgui's rule and reason).
    fbcon::console_present_suspend(true);
    serial_println!("[login] screen open window={} box={}x{} at ({},{})", id, ow, oh, ox, oy);
    repaint();
}

fn take_down() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id != wm::WIN_NONE {
        wm::close(id);
        fbcon::console_present_suspend(false);
    }
    let mut f = FORM.lock();
    f.windowed = false;
    for b in f.pw.iter_mut() {
        *b = 0;
    }
    f.pw_len = 0;
}

fn close_into_session() {
    take_down();
    FORM.lock().state = State::Session;
}

/// M4: Log Out — close the session and put the screen back up.
pub fn reopen_after_logout() {
    users::logout();
    FORM.lock().state = State::Closed;
    serial_println!("[login] logged out — screen returns");
    open();
}

pub fn is_open() -> bool {
    FORM.lock().state == State::Open
}

/// Keys are offered here first on every route; `true` = consumed (the screen is up).
pub fn consume_key(c: u8) -> bool {
    if !is_open() {
        return false;
    }
    match c {
        b'\x1b' => {} // R24: Esc dismisses menus only; the screen stays
        b'\t' => {
            let mut f = FORM.lock();
            f.focus = if f.focus == Focus::Name { Focus::Password } else { Focus::Name };
        }
        8 | 0x7f => {
            let mut f = FORM.lock();
            match f.focus {
                Focus::Name => f.name_len = f.name_len.saturating_sub(1),
                Focus::Password => f.pw_len = f.pw_len.saturating_sub(1),
            }
        }
        b'\n' | b'\r' => submit(),
        0x20..=0x7e => {
            let mut f = FORM.lock();
            match f.focus {
                Focus::Name => {
                    if f.name_len < FIELD_MAX {
                        let i = f.name_len;
                        f.name[i] = c;
                        f.name_len += 1;
                    }
                }
                Focus::Password => {
                    if f.pw_len < FIELD_MAX {
                        let i = f.pw_len;
                        f.pw[i] = c;
                        f.pw_len += 1;
                    }
                }
            }
        }
        _ => {}
    }
    repaint();
    true
}

fn submit() {
    let (name, nlen, pw, plen) = {
        let f = FORM.lock();
        (f.name, f.name_len, f.pw, f.pw_len)
    };
    if !users::load_once() {
        FORM.lock().message = "Storage is not ready yet";
        return;
    }
    let n = &name[..nlen];
    let p = &pw[..plen];
    if users::count() == 0 {
        match users::create_user(n, p) {
            Ok(_) => {}
            Err(e) => {
                FORM.lock().message = match e {
                    users::UsersError::BadName => "Name: 1-8 of a-z 0-9 _ -, letter first",
                    _ => "Could not create the user",
                };
                return;
            }
        }
    }
    match users::login(n, p) {
        Ok(()) => {
            LOGINS.fetch_add(1, Ordering::Relaxed);
            serial_println!("[login] session open user={}", core::str::from_utf8(n).unwrap_or("?"));
            close_into_session();
        }
        Err(_) => {
            let mut f = FORM.lock();
            f.message = "Login failed";
            for b in f.pw.iter_mut() {
                *b = 0;
            }
            f.pw_len = 0;
            f.focus = Focus::Password;
        }
    }
}

// ---------------------------------------------------------------------------
// fixture (`loginst`) — drives the same consume_key the routes call, headless
// ---------------------------------------------------------------------------

/// M3 fixture: the screen is driven by keys exactly as a route offers them. Esc must leave the form up;
/// a wrong password must leave it up with a message and the session closed; the right password must
/// open the session and take the screen down; with the screen down a key must pass through; Log Out
/// must put the screen back with the session closed. Headless throughout (module doc); leaves the
/// screen CLOSED and no session open, so the rest of the boot is exactly the pre-fixture world.
/// `logout` is the Log Out route under test (M3: the screen's own; M4: the crystal's row).
#[cfg(feature = "loginst")]
pub fn screen_fixture(name: &[u8], password: &[u8], wrong: &[u8], logout: fn() -> bool) -> bool {
    HEADLESS.store(true, Ordering::Relaxed);
    open();
    let feed = |s: &[u8]| {
        for &b in s {
            let _ = consume_key(b);
        }
    };
    let _ = consume_key(b'\x1b');
    let esc_kept = is_open();
    feed(name);
    let _ = consume_key(b'\t');
    feed(wrong);
    let _ = consume_key(b'\n');
    let mut nb = [0u8; users::NAME_MAX];
    let wrong_kept = is_open() && users::whoami(&mut nb).is_none() && FORM.lock().message == "Login failed";
    feed(password);
    let _ = consume_key(b'\n');
    let opened = !is_open() && matches!(users::whoami(&mut nb), Some(n) if &nb[..n] == name);
    let passes_through = !consume_key(b'x'); // with the screen down, keys reach the console again
    let logout_ok = logout();
    let back = is_open() && users::whoami(&mut nb).is_none();
    // teardown: the screen closed, nothing open, the once-latch left for the real ignition
    take_down();
    FORM.lock().state = State::Closed;
    HEADLESS.store(false, Ordering::Relaxed);
    let ok = esc_kept && wrong_kept && opened && passes_through && logout_ok && back;
    serial_println!(
        ":: LOGIN-SCREEN: window=no esc_kept={} wrong_kept={} opened={} passes_through={} logout={} back_after_logout={} -> {} ::",
        esc_kept, wrong_kept, opened, passes_through, logout_ok, back, if ok { "PASS" } else { "FAIL —" }
    );
    ok
}

/// M3's Log Out route for the fixture: the screen's own reopen (M4 hands the crystal's row in instead).
#[cfg(feature = "loginst")]
pub fn logout_direct() -> bool {
    reopen_after_logout();
    true
}
