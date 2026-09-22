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
//! and Log Out (M4: the crystal menu's own row, `crystal::Verb::LogOut`) returns to it, where a second
//! login opens a NEW session under the same or another principal. Self-drawn, the Mac model: a name field, a
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
//! so the drawing is proven on the glass and the state machine on the ladder — with the ONE exception
//! the CLOSE leg below states and pays for, because a claim about a `wm` ROW cannot be proven without
//! one.
//!
//! LOGINCLOSE (SO36's card-blocker half) — **THE SCREEN CANNOT BE CLOSED BY THE POINTER, AND THAT IS
//! A PROPERTY OF THE ROW, NOT OF A ROUTE.** The screen's three pieces of state — the `wm` row
//! ([`WIN`]), [`FORM`]`.state`, and `fbcon`'s console-present suspension — are set together by
//! [`open`] and cleared together by [`take_down`]. Anything that moves ONE of them alone leaves the
//! machine with no login screen drawn, no console presented and every keystroke still swallowed by
//! [`consume_key`]: dead to the user, on the first thing anyone sees. The obvious such thing is a
//! CLOSE BOX — `wm::close` runs `winid_holders_clear`, which resets [`WIN`] to `WIN_NONE` and touches
//! neither of the other two.
//!
//! So the screen is minted in the SHELL/DESKTOP owner band ([`OWNER`] = 0) — the same band the desktop
//! itself is in — and that band is refused BOTH halves of a pointer close:
//!  * `wm::controls` returns `None` for `owner_asid == 0` (`video/wm.rs`, *"the shell/desktop row
//!    (CLICK-SHELL), which `hit_test` never names at all"*), so `control_disc` — the ONE accessor the
//!    painter and `control_hit` share — has no slot to offer and NO control cluster is drawn: no close
//!    disc, no minimise, no zoom. The caption is still drawn; there are simply no buttons.
//!  * `wm::hit_test` skips `owner_asid == 0` rows outright, so the routers' second question
//!    (`close_box_hit` / `control_hit`, asked ONLY after `hit_test` has named an id — x86's
//!    `wc_click_route_at`, aarch64's `wc_click_route`) is never asked about this row at all. Neither
//!    is the title-bar drag, the raise, or the minimise.
//!
//! That property was TRUE and UNWRITTEN at 80070464 — inherited from `instgui.rs:480`, which mints its
//! modal the same way — and an unwritten load-bearing property is one edit from deletion (LAWS §5:
//! *"an undocumented true property is load-bearing and deletable at once: write the contract down"*).
//! It is written here, it is named at the create site, and it is MEASURED every witness boot by the
//! fixture's CLOSE leg against an ARMED CONTROL ROW that differs from the screen's row in exactly one
//! argument — the owner — so "no close box" is a verdict about the BAND and not about the fixture's
//! arithmetic.
//!
//! And the belt, because what matters is the invariant and not the one route that would have broken
//! it: [`heal_if_row_gone`] runs at the head of every key offer. If the row is gone while the form is
//! still `Open` — a close from ANY route, present or future, including one that reaches `wm::close`
//! without a pointer ever passing through `hit_test` — the screen is put back before the key is read,
//! and the three pieces are coherent again. It is a repair-on-use check and NOT a reopen latch:
//! nothing is queued, nothing is re-minted from a remembered id, no route drains it, and no tile
//! launches it (R49 retired exactly that machinery and this does not bring it back). Esc already does
//! not close the screen (R24, proven by M3's `esc_kept` leg); the close box is the same kind of no,
//! and now so is every other way the row could go.
//!
//! LOGINFLOW (SO44's second sentence) — **THE PRESS IS THE SCREEN'S, AND A SCREEN THAT ONLY SWALLOWS
//! IS A WALL.** SO44's rule is two sentences: *a press outside the rectangle belongs to nobody* **and**
//! *a press inside it belongs to IT*. SESSGATE landed the first — [`press_swallow`] answered `true`
//! for every point, both routers stopped there, no tile launched and no row was raised — and that is
//! the whole of the barrier and none of the screen. To the person in front of it the two are not
//! distinguishable from a machine that has hung: press the password field and the caret does not move,
//! press where a button should be and there is no button, and nothing anywhere says why.
//!
//! So the coordinates [`press_swallow`] has always taken and thrown away are USED. [`local_of`] maps
//! the panel point through the row's own `wm` geometry, [`ctl_at`] asks which control is there, and the
//! control acts: the two fields focus, the button submits (the SAME [`submit`] Enter calls — there is
//! no second path to keep in step), a user's row picks that name out of the STORE and moves to the
//! password. The return value is unchanged and MUST stay unchanged: `true` for EVERY press while the
//! screen is up, hit or miss, because the modality is the ROUTER's contract and is not conditioned on
//! there being a control under the point.
//!
//! The screen therefore SHOWS who lives on this machine (`users::name_at`, the Mac model) and carries
//! a button, because Enter is not discoverable and a person who has just chosen a password has no
//! reason to know it is the only way in. The painter and the press read ONE accessor, [`ctl_rect`] —
//! `wm::control_disc`'s discipline, for LOGINCLOSE's reason one layer out: a control drawn from one
//! rect and hit-tested from another is one edit from being drawn where it cannot be pressed.
//!
//! And the DENIAL says one thing. [`submit`] asks `users::verify` — *"one answer for 'no such user'
//! and 'wrong password'"* — rather than reading a `UsersError`, so neither the glass nor the wire can
//! grow a reason that tells someone at the keyboard which names exist on the machine.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::super::{fbcon, theme, wm};
use crate::fs::users;

const W: usize = 440;
const H: usize = 240;
const TS: usize = 2;
const CELL: usize = 8 * TS;
const FIELD_MAX: usize = 32;

/// LOGINCLOSE — **the owner band the screen's row is minted in, and the whole of why the screen has no
/// close box.** `0` is the SHELL/DESKTOP band: `wm::controls` declines it a control cluster and
/// `wm::hit_test` never names it, so no press can reach this row and there is no disc drawn to press.
/// See the LOGINCLOSE section of this module's doc for the two citations and for why this is a `const`
/// with a name rather than the bare `0` `instgui.rs:480` passes — a literal in an argument list is a
/// property nobody can grep for, and this one is the difference between a login screen and a machine
/// the first click kills.
const OWNER: u64 = 0;

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
/// LOGINCLOSE — how many times [`heal_if_row_gone`] has put a stranded screen back (a counter for the
/// witness). NON-ZERO ON A REAL BOOT IS A FINDING: it means something closed the screen's row, and the
/// route that did it has to be named and shut. Zero is the expected reading everywhere except the
/// fixture's own CLOSE leg, which drives `wm::close` at the row on purpose.
static HEALS: AtomicU32 = AtomicU32::new(0);
/// LOGINFLOW — presses the screen ANSWERED (a control was hit) and presses it merely SWALLOWED. The
/// pair is the witness for SO44's second half: `swallowed` alone proves the BARRIER, and only
/// `answered` proves the screen is a screen rather than a wall.
static PRESS_ANSWERED: AtomicU32 = AtomicU32::new(0);
static PRESS_SWALLOWED: AtomicU32 = AtomicU32::new(0);
/// One `control=none` press line per open, not one per press: a trackpad resting on the glass emits
/// presses a human never counts, and a witness that scrolls the boot log away is not a witness. A hit
/// on a CONTROL always prints — those are countable by construction, because a person made each one.
static MISS_SAID: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// LAYOUT — **ONE accessor, read by the painter AND by the press.**
// ---------------------------------------------------------------------------
//
// LOGINFLOW M1 — `wm::control_disc`'s discipline applied to the screen's own face, and it is the same
// argument LOGINCLOSE's doc makes about the window chrome one layer out: *a control DRAWN from one
// rect and HIT-TESTED from another is one edit away from being drawn where it cannot be pressed*, and
// nothing on the glass would say so — the button would simply be dead under the pointer, on the first
// thing anyone sees. So [`repaint`] and [`ctl_at`] both go through [`ctl_rect`] and neither carries a
// literal of its own.

/// The screen's pressable controls, in the order [`ctl_at`] asks about them.
#[derive(Clone, Copy, PartialEq)]
enum Ctl {
    /// The name field — a press focuses it.
    NameField,
    /// The password field — a press focuses it.
    PwField,
    /// Log In / Create — a press IS [`submit`], the same call Enter makes.
    Button,
    /// A user's row: the screen SHOWS who lives on this machine (the Mac model), and a press picks
    /// that name into the field and moves to the password. The `usize` is the store's row index.
    User(usize),
}

const LX: usize = 24;
/// User rows drawn. Four fit the 392 px of content width at `USER_W` + `USER_GAP`; the store holds up
/// to `users::MAX_USERS` (8). A machine with more users than fit still LOGS IN — the name field is the
/// general answer and the rows are the shortcut — so this is a drawing bound and not a limit on
/// anything the flow can do.
const USER_MAX: usize = 4;
const USER_W: usize = 92;
const USER_GAP: usize = 8;
const FIELD_X: usize = LX + 100;
const FIELD_W: usize = W - 2 * LX - 100;
const FIELD_H: usize = CELL + 8;
const BTN_W: usize = 120;
const BTN_H: usize = 28;

/// How many user rows the screen draws. Zero on the create-first-user screen, by construction.
fn user_rows() -> usize {
    users::count().min(USER_MAX)
}

/// The rect of one control, in SURFACE pixels. The ONE place any of these numbers exists.
fn ctl_rect(c: Ctl) -> (usize, usize, usize, usize) {
    match c {
        Ctl::User(i) => (LX + i * (USER_W + USER_GAP), 46, USER_W, FIELD_H),
        Ctl::NameField => (FIELD_X, 76, FIELD_W, FIELD_H),
        Ctl::PwField => (FIELD_X, 112, FIELD_W, FIELD_H),
        Ctl::Button => (W - LX - BTN_W, 150, BTN_W, BTN_H),
    }
}

/// Which control is at a SURFACE point, if any — the press's half of [`ctl_rect`], with no rect of its
/// own. The fields and the button are asked first and the user rows last: the rows are the only
/// controls whose COUNT varies, so asking them last keeps a store that grows from moving the answer
/// anywhere the fixed controls already claim.
fn ctl_at(lx: i32, ly: i32) -> Option<Ctl> {
    let inside = |c: Ctl| {
        let (rx, ry, rw, rh) = ctl_rect(c);
        lx >= rx as i32 && lx < (rx + rw) as i32 && ly >= ry as i32 && ly < (ry + rh) as i32
    };
    for c in [Ctl::NameField, Ctl::PwField, Ctl::Button] {
        if inside(c) {
            return Some(c);
        }
    }
    (0..user_rows()).map(Ctl::User).find(|&c| inside(c))
}

/// The control's name on the wire. NEVER the user's own name: a login screen's serial log must not be
/// a roster, and the row index is enough to read the line back against the store.
fn ctl_name(c: Option<Ctl>) -> &'static str {
    match c {
        Some(Ctl::NameField) => "name-field",
        Some(Ctl::PwField) => "password-field",
        Some(Ctl::Button) => "button",
        Some(Ctl::User(_)) => "user-row",
        None => "none",
    }
}

/// PANEL pixels -> SURFACE pixels, through the row's own `wm` geometry (origin AND integer upscale), or
/// `None` where there is no row (headless) or the point lies outside the surface. The subtraction
/// happens FIRST and its sign is checked BEFORE the divide: Rust's integer division truncates toward
/// zero, so `-1 / 2` reads as `0` and a press one pixel ABOVE the window would land inside its first
/// row.
fn local_of(x: i32, y: i32) -> Option<(i32, i32)> {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return None;
    }
    let info = wm::info(id)?;
    let (dx, dy) = (x - info.x as i32, y - info.y as i32);
    if dx < 0 || dy < 0 {
        return None;
    }
    let s = info.scale.max(1) as i32;
    let (lx, ly) = (dx / s, dy / s);
    if lx >= W as i32 || ly >= H as i32 {
        return None;
    }
    Some((lx, ly))
}

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

/// LOGINFLOW M1 — the push button, drawn from [`ctl_rect`] like every other control. A bevel, a frame
/// and a centred caption in the `theme`'s own button colours: the point is not the styling, it is that
/// there IS a thing to press, because Enter is not discoverable and a person who has just created a
/// password has no reason to know it is the only way in.
fn button(px: &mut [u32], c: Ctl, label: &[u8], primary: bool) {
    let (x, y, w, h) = ctl_rect(c);
    fill(px, x, y, w, h, if primary { theme::ACCENT } else { theme::BUTTON_FACE });
    rect(px, x, y, w, h, theme::FRAME_LINE);
    let tw = label.len() * CELL;
    let tx = x + w.saturating_sub(tw) / 2;
    let ty = y + h.saturating_sub(CELL) / 2;
    text(px, tx, ty, label, if primary { theme::BEVEL_LIGHT } else { theme::BUTTON_TEXT });
}

/// LOGINFLOW M1 — one user's row. The screen says WHO lives on this machine, which is the Mac model
/// and is also the only affordance that makes the name field optional for the person who owns the
/// machine. The name is drawn TRUNCATED to the cell rather than clipped mid-glyph, and it is the only
/// place a user's name is put on the glass before a session exists — deliberate, and the reason the
/// wire line for a press on one says `user-row` and an INDEX, never the name.
fn user_row(px: &mut [u32], i: usize, name: &[u8], picked: bool) {
    let (x, y, w, h) = ctl_rect(Ctl::User(i));
    fill(px, x, y, w, h, if picked { theme::ACCENT } else { theme::CONTENT_FILL });
    rect(px, x, y, w, h, if picked { theme::ACCENT } else { theme::FRAME_LINE });
    let max = (w - 12) / CELL;
    let n = name.len().min(max);
    text(px, x + 6, y + 4, &name[..n], if picked { theme::BEVEL_LIGHT } else { theme::CONTENT_TEXT });
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
    let first = users::count() == 0;
    let title: &[u8] = if first { b"Create the first user" } else { b"Log in to UnaOS" };
    text(px, LX, 14, title, theme::CONTENT_TEXT);
    fill(px, LX, 36, W - 2 * LX, 2, theme::FRAME_LINE);
    // The user rows, when there are users. `name_at` is the store's own accessor, so the row a press
    // picks and the row the painter draws are the same row by construction — the `ctl_rect` argument
    // one layer up, applied to the CONTENT as well as to the geometry.
    let mut nb = [0u8; users::NAME_MAX];
    for i in 0..user_rows() {
        if let Some(n) = users::name_at(i, &mut nb) {
            user_row(px, i, &nb[..n], f.name_len == n && f.name[..n] == nb[..n]);
        }
    }
    let (nx, ny, nw, _) = ctl_rect(Ctl::NameField);
    text(px, LX, ny + 4, b"Name", theme::TITLE_TEXT_INACTIVE);
    field(px, nx, ny, nw, &f.name[..f.name_len], f.focus == Focus::Name, false);
    let (pxf, py, pwf, _) = ctl_rect(Ctl::PwField);
    text(px, LX, py + 4, b"Password", theme::TITLE_TEXT_INACTIVE);
    field(px, pxf, py, pwf, &f.pw[..f.pw_len], f.focus == Focus::Password, true);
    button(px, Ctl::Button, if first { b"Create" } else { b"Log In" }, true);
    let hint: &[u8] = if first { b"Enter or Create makes the user" } else { b"Enter or Log In   Tab switches" };
    text(px, LX, 186, hint, theme::TITLE_TEXT_INACTIVE);
    if !f.message.is_empty() {
        text(px, LX, 212, f.message.as_bytes(), theme::ACCENT);
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
    MISS_SAID.store(false, Ordering::Relaxed); // LOGINFLOW — the one miss line is per OPEN, not per boot
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
    // LOGINCLOSE — the row is minted under [`OWNER`], the shell/desktop band, and THAT is what makes
    // the screen unclosable by the pointer: no control cluster is drawn for it and `wm::hit_test`
    // never names it, so neither router can ask the close question about this row. Changing this
    // argument gives the login screen a close box and gives a one-click route to a machine with no
    // screen, no console and a swallowed keyboard. The fixture's CLOSE leg measures it every witness
    // boot, against a control row that differs only here.
    let id = wm::create_at(OWNER, surf, W * H * 4, W as u32, H as u32, (W * 4) as u32, b"Log in", ox + wm::BORDER, oy + wm::TITLE_H + wm::BORDER);
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

/// SO36 + SO44 — **the screen's answer to a PRESS, and it is the same answer everywhere.**
///
/// `true` while the screen is up, for every `(x, y)` on the panel. Reached from
/// `fs::users::screen_press`, which `video::strip::press_route` asks FIRST — ahead of the window menu,
/// the crystal and the dock, and therefore ahead of every window arm in both routers. See
/// [`crate::fs::users::screen_press`] for the derivation; what belongs HERE is why the coordinates are
/// taken and then not used:
///
///  * **Inside the rectangle** the press must not reach the row beneath. The screen's row is minted under
///    [`OWNER`] (the shell/desktop band), which `wm::hit_test` never names, so a press on the screen's own
///    pixels resolves to whatever is BEHIND it and the router raises that — Peter's *"i click it and it
///    went away"*. Nothing in this module can make the row hit-test without also giving it a close box
///    (LOGINCLOSE's measured defect), so the press is stopped at the router instead.
///  * **Outside the rectangle** the press must not reach the DOCK, the crystal or a window menu (SO36):
///    the Mac model is that the login screen owns the whole glass, and a tile that launches a program with
///    no session open launches it under NO principal.
///
/// So the honest signature is a predicate over the screen's state, and the coordinates are taken only so
/// that a screen which one day grows a live region (a "switch user" affordance, say) narrows this in place
/// rather than through a new seam. There is no press this function may decline while the screen is up.
///
/// It does NOT swallow keys — [`consume_key`] is that seam, unchanged.
///
/// # LOGINFLOW M1 — **AND THEN THE PRESS IS THE SCREEN'S, WHICH IS THE HALF SO44 ASKED FOR.**
///
/// SESSGATE stopped the press at the router and stopped it there COMPLETELY: `true` for every point,
/// nothing raised, nothing focused, nothing launched. That closed SO36's furniture half and the
/// stranding half of SO44 — and it left the screen a WALL. SO44's rule is two sentences and only the
/// second one had landed: *a press outside the rectangle belongs to nobody* **and** *a press inside it
/// BELONGS TO IT*. A barrier that answers every press with silence is indistinguishable, to the person
/// sitting in front of it, from a machine that has hung: Peter presses the password field and the
/// caret does not move, presses where a button should be and there is no button. So the coordinates
/// this function has always taken and thrown away are now USED — translated through the row's own `wm`
/// geometry ([`local_of`]) and asked of [`ctl_at`], the press half of the painter's [`ctl_rect`].
///
/// **The return value does not change and MUST not.** It is still `true` for every press while the
/// screen is up, including a press that hits no control — the modality is the ROUTER's contract and it
/// is not conditioned on the screen having a control under the point. `answered` and `swallowed` are
/// counted separately so the wire can tell a screen that is answering from a barrier that is merely
/// holding; `PRESS_ANSWERED` staying 0 across a boot with presses on it is the reading that means the
/// controls have come unstuck from the paint.
///
/// **Headless is NOT a special case and takes no branch of its own**: [`local_of`] answers `None`
/// where there is no row, `ctl_at` is never asked, and the press is swallowed exactly as before — which
/// is what the ladder's legs have always measured and must keep measuring.
///
/// The RELEASE is still the router's to drop (`CLICK_TARGET_DROP`), so a control fires on the press
/// edge, once, the grammar the close disc and every furniture arm already follow.
pub fn press_swallow(x: i32, y: i32) -> bool {
    if !is_open() {
        return false;
    }
    // LOGINCLOSE — the same belt [`consume_key`] runs, for the same reason and one layer earlier: a
    // press routed into a screen that is not on the glass is a press into nothing, and the row's
    // geometry is exactly what [`local_of`] is about to read.
    heal_if_row_gone();
    let hit = local_of(x, y).and_then(|(lx, ly)| ctl_at(lx, ly));
    match hit {
        Some(Ctl::NameField) => FORM.lock().focus = Focus::Name,
        Some(Ctl::PwField) => FORM.lock().focus = Focus::Password,
        // A press on the button IS Enter. One call, so the two routes into a session cannot drift:
        // there is no second submit path to keep in step with `consume_key`'s.
        Some(Ctl::Button) => submit(),
        Some(Ctl::User(i)) => pick_user(i),
        None => {}
    }
    if hit.is_some() {
        PRESS_ANSWERED.fetch_add(1, Ordering::Relaxed);
    } else {
        PRESS_SWALLOWED.fetch_add(1, Ordering::Relaxed);
    }
    // A CONTROL always says so; a miss says so once per open (see `MISS_SAID`). Neither line carries a
    // typed byte, a user's name or a field length — the whole point of this screen is that what is
    // typed into it does not reach the wire.
    if hit.is_some() || !MISS_SAID.swap(true, Ordering::Relaxed) {
        serial_println!(
            "[login] press at=({},{}) control={} answered={} swallowed={} (SO44: while the screen is open every press is the screen's — the router stops it and THIS decides what it means)",
            x, y, ctl_name(hit),
            PRESS_ANSWERED.load(Ordering::Relaxed),
            PRESS_SWALLOWED.load(Ordering::Relaxed)
        );
    }
    repaint();
    true
}

/// LOGINFLOW M1 — a press on a user's row picks that name. The password is cleared with it and the
/// focus moves to the password field, which is the whole gesture a Mac login is: point at yourself,
/// type the password, Enter. The name is copied from the STORE (`users::name_at`), never from the
/// glass, so a row that cannot be read picks nothing rather than picking a truncated name.
fn pick_user(i: usize) {
    let mut nb = [0u8; users::NAME_MAX];
    let Some(n) = users::name_at(i, &mut nb) else {
        return;
    };
    let mut f = FORM.lock();
    f.name[..n].copy_from_slice(&nb[..n]);
    f.name_len = n;
    for b in f.pw.iter_mut() {
        *b = 0;
    }
    f.pw_len = 0;
    f.focus = Focus::Password;
    f.message = "";
}

/// LOGINCLOSE — **the three pieces of the screen's state move together, or the machine is dead.**
///
/// `WIN`, `FORM.state` and `fbcon`'s console-present suspension are set by [`open`] and cleared by
/// [`take_down`]; nothing else may move one alone. One thing in the tree can, without knowing it does:
/// `wm::close` calls `winid_holders_clear`, which finds the `"login"` holder registered at
/// [`open`]'s tail and resets `WIN` to `WIN_NONE`. The form is then still `Open` — so [`consume_key`]
/// keeps swallowing every keystroke — with no window drawn and the console still suspended. That is a
/// machine with no screen, no console and a dead keyboard, and it is what this function refuses to let
/// stand.
///
/// It is a REPAIR-ON-USE CHECK, not a reopen latch. Nothing is queued, no id is remembered, no service
/// drains it and no tile launches it — R49 retired that machinery for the console and the shell and
/// this does not smuggle it back. The screen is simply re-opened, in place, by the next key offer,
/// which is the first moment the stranding can be OBSERVED by the only party that cares.
///
/// The suspension has no getter, and it does not need one: [`open`] and [`take_down`] are the only two
/// callers of `console_present_suspend` in this module, so the flag is `true` exactly when
/// `WIN != WIN_NONE`, and re-opening restores the pair together. `open` stores `true` again on a flag
/// that is already `true` — idempotent, one relaxed store.
///
/// Returns `true` when it repaired something, which is only ever a finding outside the fixture.
fn heal_if_row_gone() -> bool {
    {
        let f = FORM.lock();
        if f.state != State::Open || !f.windowed {
            return false;
        }
    }
    if WIN.load(Ordering::Relaxed) != wm::WIN_NONE {
        return false;
    }
    HEALS.fetch_add(1, Ordering::Relaxed);
    serial_println!("[login] row closed under an open screen — reopening (heals={})", HEALS.load(Ordering::Relaxed));
    // `open` refuses a form that is already `Open` (it is the boot-ignition guard), so the state is
    // walked back one step first. Every field is cleared by `open` anyway, which is what a person
    // whose login screen just vanished and came back should get: an empty form, focus on Name.
    FORM.lock().state = State::Closed;
    open();
    true
}

/// Keys are offered here first on every route; `true` = consumed (the screen is up).
pub fn consume_key(c: u8) -> bool {
    if !is_open() {
        return false;
    }
    // LOGINCLOSE — ahead of reading the key, because a key read into a screen that is not on the glass
    // is a key typed into nothing. See [`heal_if_row_gone`]; on every ordinary press this is one
    // mutex take and one relaxed load, and it answers `false`.
    heal_if_row_gone();
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
            // LOGINFLOW M2 — **the first boot on a fresh volume, and the reason this screen exists at
            // all rather than a serial verb.** `users::count() == 0` is the whole predicate: there is
            // nobody to log in as, so the same two fields CREATE the person and then log them in, with
            // no console, no cable and nothing to know beforehand. The witness is the id, never the
            // name's length or any part of what was typed.
            Ok(id) => serial_println!("[login] first user created id={} — the fresh-volume path (no console was needed)", id),
            Err(e) => {
                FORM.lock().message = match e {
                    users::UsersError::BadName => "Name: 1-8 of a-z 0-9 _ -, letter first",
                    _ => "Could not create the user",
                };
                return;
            }
        }
    }
    // LOGINFLOW M2 — **THE DENIAL IS DECIDED BY ONE PREDICATE AND IT GIVES ONE ANSWER.**
    //
    // `users::verify` is documented as *"one answer for 'no such user' and 'wrong password'"*, and
    // asking it HERE — ahead of `users::login`, which asks it again internally — is what keeps that
    // property reachable from this screen. The alternative (deciding from `login`'s `UsersError`) puts
    // a variant in the hand of the code that writes the message, and `users_reason` exists and is
    // helpful and would eventually be used: `Refused` vs `Volume` vs `Exists` on the glass tells an
    // attacker at the keyboard which names exist on the machine. There is no cost worth counting — a
    // second salted SHA-256 on the ONE path a human takes a few times a day.
    //
    // So: ONE message, ONE wire line, and NEITHER carries a reason. The name IS on the wire (it is
    // what was typed into a plain field, not a credential, and without it the line cannot be read
    // against the store); nothing else about the attempt is, and the password never touches a serial
    // route on any path — see this module's header.
    if !users::verify(n, p) {
        serial_println!(
            "[login] denied user={} (one answer: a name that does not exist and a wrong password are the SAME refusal here and on the glass — `users::verify`, never `UsersError`)",
            core::str::from_utf8(n).unwrap_or("?")
        );
        let mut f = FORM.lock();
        f.message = "Login failed";
        for b in f.pw.iter_mut() {
            *b = 0;
        }
        f.pw_len = 0;
        f.focus = Focus::Password;
        return;
    }
    match users::login(n, p) {
        Ok(()) => {
            LOGINS.fetch_add(1, Ordering::Relaxed);
            serial_println!("[login] session open user={}", core::str::from_utf8(n).unwrap_or("?"));
            close_into_session();
        }
        Err(_) => {
            // The credential VERIFIED one line above and the session still did not open — a storage or
            // slot-stamp refusal, which is not a denial and must not be reported as one. Said
            // differently on the glass for the same reason the denial is said identically: a person
            // who typed the right password must not be sent to look for a typo.
            serial_println!("[login] verified but the session did not open — storage or slot refusal, NOT a credential refusal");
            let mut f = FORM.lock();
            f.message = "Could not open the session";
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

/// SO43 — **the IGNITION leg: the screen comes up because the DESKTOP exists, and for no other
/// reason.** The leg that would have caught SO43 before five flights did.
///
/// It drives [`crate::fs::users::screen_open_at_ignition`] — the seam every ignition site now calls —
/// with the tuples the two arches actually present, and asserts the CONSEQUENCE ([`is_open`]), never
/// the seam's own return:
///
///  1. **`(desktop_up = false, console_routed = true)`** — a machine with a routed console and no
///     desktop. The screen must stay DOWN. This is the arm that keeps the new rule from being "always
///     open": a check whose corpus can only produce one outcome is not a check (LAWS §5).
///  2. **`(desktop_up = true, console_routed = false)`** — **THE ORIN'S OWN TUPLE**, read off the
///     wire rather than imagined (`docs/dev/evidence/orin28/render14-boot1-desktop-menubar.log`,
///     `awk 'index($0,"[deskcascade]")'`):
///     `[deskcascade] -> CASCADED windows=2 bar=1 owns_pixels=1 route=ROUTED activate=false`
///     — `bar=1` is `desktop_up`, `activate=false` is `console_routed`. The screen MUST open.
///
/// Arm 2 is the go-red: put `console_routed` back into the rule inside `screen_open_at_ignition`
/// (`if desktop_up` -> `if desktop_up && console_routed`) and this leg reads
/// `tegra_opened=false -> FAIL —`, which is a `mbench` DEFAULT_FORBID and reds the run. That is the
/// pre-fix kernel, reproduced and refused, on the ladder — no tegra hardware in the loop, because the
/// seam is arch-neutral by construction and the board only supplies the tuple.
///
/// Headless throughout and side-effect free: the once-latch, the `HEADLESS` flag and the form state
/// are saved and put back, so the real ignition later in the boot is the one that counts.
#[cfg(feature = "loginst")]
fn ignition_leg() -> bool {
    let was_headless = HEADLESS.swap(true, Ordering::Relaxed);
    let was_once = OPENED_ONCE.swap(false, Ordering::Relaxed);
    FORM.lock().state = State::Closed;

    // 1 — a routed console is not a desktop.
    users::screen_open_at_ignition(false, true);
    let no_desktop_held = !is_open();

    // 2 — THE ORIN: the desktop exists and `activate()` returned false.
    users::screen_open_at_ignition(true, false);
    let tegra_opened = is_open();

    take_down();
    FORM.lock().state = State::Closed;
    OPENED_ONCE.store(was_once, Ordering::Relaxed);
    HEADLESS.store(was_headless, Ordering::Relaxed);

    let ok = no_desktop_held && tegra_opened;
    serial_println!(
        ":: LOGIN-IGNITION: no_desktop_held={} tegra_opened={} tuple=desktop_up:true,console_routed:false (render14 `bar=1 … activate=false`) -> {} ::",
        no_desktop_held,
        tegra_opened,
        if ok { "PASS" } else { "FAIL —" }
    );
    ok
}

/// LOGINCLOSE — **the CLOSE leg, and it is the one claim in this file that CANNOT be made headless.**
///
/// "The login screen has no close box and no pointer route" is a statement about a `wm` ROW: every
/// predicate that decides it (`controls`, `control_disc`, `hit_test`) reads a row and nothing else, so
/// with no row there is nothing to ask and a headless leg could only re-assert the source. This leg
/// therefore mints the REAL screen — one window, for the length of the leg — which is the exception to
/// this module's "the fixture NEVER opens a window" rule, and it pays for the exception by putting the
/// panel back exactly as it found it (`take_down`, which also resumes the console) before it returns.
///
/// It answers four questions, in the order a press would ask them:
///  1. **Is a cluster DRAWN?** `close_box_rect` and `control_disc_rect` are the painter's own accessor
///     — a control that is drawn is a control that can be pressed, and the inverse. All three must be
///     `None`.
///  2. **Can a press REACH the row?** `hit_test` is swept over the whole outer box on a 4 px grid,
///     chrome included, and must never name this window; `close_box_hit` is asked at every one of
///     those points and must never say yes. That is both routers' entire path to a close
///     (`wc_click_route_at` on x86, `wc_click_route` on aarch64 both ask `hit_test` FIRST).
///  3. **Could the questions have said YES?** An ARMED CONTROL ROW is minted through the SAME
///     `wm::create_at` call with ONE argument changed — the owner — and every predicate above must
///     answer yes about IT. A check whose corpus can only produce one outcome is not a check
///     (LAWS §5); this is what makes the three `None`s above a fact about the BAND.
///  4. **And if something closes the row anyway?** The leg then does the damage itself — `wm::close`
///     straight at the screen's window, which is exactly what any close route ends in and what runs
///     `winid_holders_clear` — confirms the machine really is stranded (row gone, form still `Open`),
///     and requires the next key offer to put it back ([`heal_if_row_gone`]).
///
/// Returns `(route, reopened)`. `route` is `refused` (the expected reading), `REACHABLE` (the defect:
/// a pointer can close the login screen), or `no-window` where `wm` could name no surface in this
/// harness — never a silent pass, and `no-window` is the reading on the aarch64 `virt` leg, which
/// builds no desktop. The x86 WC ladder is where the claim is proven.
#[cfg(feature = "loginst")]
fn close_leg() -> (&'static str, bool) {
    /// The sweep's pitch in panel pixels. The close disc is `theme::CONTROL_BOX` across (24 px at the
    /// current metrics) and the discs sit a `GAP` apart, so 4 px cannot step over one.
    const STEP: usize = 4;
    // The REAL screen, not the headless form. `open` refuses a form that is already `Open` (the
    // ignition guard), so a screen the boot already put up — headless, because the panel may not have
    // existed then — is walked back one step first and re-opened against the panel that exists now.
    HEADLESS.store(false, Ordering::Relaxed);
    FORM.lock().state = State::Closed;
    open();
    let win = WIN.load(Ordering::Relaxed);
    let Some(info) = wm::info(win) else {
        take_down();
        FORM.lock().state = State::Closed;
        return ("no-window", false);
    };
    let bx = info.x.saturating_sub(wm::BORDER);
    let by = info.y.saturating_sub(wm::TITLE_H + wm::BORDER);
    let bw = info.w.saturating_mul(info.scale).saturating_add(2 * wm::BORDER);
    let bh = info
        .h
        .saturating_mul(info.scale)
        .saturating_add(wm::TITLE_H + 2 * wm::BORDER);
    // 1 — nothing is drawn to press.
    let no_cluster = wm::close_box_rect(win).is_none()
        && wm::control_disc_rect(win, wm::Ctrl::Minimise).is_none()
        && wm::control_disc_rect(win, wm::Ctrl::Zoom).is_none();
    // 2 — and no press reaches the row, anywhere on it.
    let (mut named, mut disc, mut samples) = (false, false, 0usize);
    let mut y = by;
    while y < by + bh {
        let mut x = bx;
        while x < bx + bw {
            samples += 1;
            if matches!(wm::hit_test(x as i32, y as i32), Some((w, _, _)) if w == win) {
                named = true;
            }
            if wm::close_box_hit(win, x as i32, y as i32) {
                disc = true;
            }
            x += STEP;
        }
        y += STEP;
    }
    // 3 — THE ARMED CONTROL. One argument different from the screen's own create: the owner. `1` is an
    // ordinary user ASID (the `slot + 1` bias user owners are built from), so it is neither the
    // shell/desktop band nor the kernel band `close_owner` refuses, and it is the row every app on the
    // glass is. It shares the screen's surface for the instant it exists — it is a stand-in for the
    // ROW, not for the app — and it is closed before anything else happens.
    let ctrl = wm::create_at(
        1,
        core::ptr::addr_of_mut!(SURF) as usize,
        W * H * 4,
        W as u32,
        H as u32,
        (W * 4) as u32,
        b"Log in",
        info.x,
        info.y,
    );
    let control_armed = match wm::close_box_rect(ctrl) {
        Some((cx, cy, d)) => {
            let (hx, hy) = ((cx + d / 2) as i32, (cy + d / 2) as i32);
            wm::close_box_hit(ctrl, hx, hy)
                && matches!(wm::hit_test(hx, hy), Some((w, _, _)) if w == ctrl)
        }
        None => false,
    };
    if ctrl != wm::WIN_NONE {
        wm::close(ctrl);
    }
    let route = if !control_armed {
        // The control could not answer YES about a row that HAS a close box, so this run's three
        // `None`s prove nothing at all. Said as its own token rather than folded into a pass.
        "no-control"
    } else if no_cluster && !named && !disc {
        "refused"
    } else {
        "REACHABLE"
    };
    // SO44 — **AND WHAT DOES A PRESS AT THE CENTRE OF THIS ROW ACTUALLY DO?** LOGINCLOSE asked "can
    // the pointer CLOSE this row" and never asked this one, and the two answers are not the same
    // answer: a press that reaches nothing does not stop, it falls to whatever is UNDERNEATH, and the
    // row under a centred login screen is the desktop's own console/shell window. Both arch routers
    // then RAISE and FOCUS that row (`wc_click_route_at` on x86, `wc_click_route` on aarch64, each
    // after `hit_test` names an id), which puts it above the screen — Peter, render14: *"the login
    // window appeared over the top of the gui and when i click it it went away."*
    //
    // READ-ONLY and no verdict: `hit_test` is a pure read over the window table; nothing is pressed,
    // raised or focused by this line.
    let (cxp, cyp) = (
        info.x.saturating_add(info.w.saturating_mul(info.scale) / 2) as i32,
        info.y.saturating_add(info.h.saturating_mul(info.scale) / 2) as i32,
    );
    let beneath = wm::hit_test(cxp, cyp);
    serial_println!(
        "[login] press-probe win={} centre=({},{}) hit={} verdict={} (SO44: a press inside the screen's rectangle must belong to the screen; today it belongs to the row beneath it, which both routers then raise above the screen)",
        win,
        cxp,
        cyp,
        match beneath {
            Some((w, _, _)) => w,
            None => wm::WIN_NONE,
        },
        match beneath {
            Some((w, _, _)) if w == win => "MODAL",
            Some(_) => "FALLS-THROUGH",
            None => "NOBODY",
        }
    );
    // 4 — the belt. `wm::close` at the screen's own row is what every close route ends in, and it is
    // what runs `winid_holders_clear` on the `"login"` holder.
    wm::close(win);
    let stranded = WIN.load(Ordering::Relaxed) == wm::WIN_NONE && is_open();
    let _ = consume_key(b'\t'); // any key: the offer is where the repair runs
    let reopened = stranded
        && is_open()
        && WIN.load(Ordering::Relaxed) != wm::WIN_NONE
        && FORM.lock().windowed;
    serial_println!(
        "[login] close-leg win={} box={}x{} at ({},{}) samples={} cluster_drawn={} hit_named={} close_disc={} control_armed={} stranded={} reopened={} route={}",
        win, bw, bh, bx, by, samples, !no_cluster, named, disc, control_armed, stranded, reopened, route
    );
    // The panel goes back exactly as it was found: the row freed, the console resumed, the form down.
    take_down();
    FORM.lock().state = State::Closed;
    (route, reopened)
}

/// M3 fixture: the screen is driven by keys exactly as a route offers them. Esc must leave the form up;
/// a wrong password must leave it up with a message and the session closed; the right password must
/// open the session and take the screen down; with the screen down a key must pass through; Log Out
/// must put the screen back with the session closed; and (M4) a SECOND login through the same form
/// must open a NEW session. Headless throughout (module doc) EXCEPT [`close_leg`], which runs first,
/// needs one real `wm` row to make a claim about, and puts the panel back before the rest begins;
/// leaves the
/// screen CLOSED and no session open, so the rest of the boot is exactly the pre-fixture world.
/// `logout` is the Log Out route under test — M4 hands in `crystal::logout_row_fire`, which finds the
/// **Log Out row** in the SHARD tree, resolves it through the menu's own pure `item_at`, and fires it.
#[cfg(feature = "loginst")]
pub fn screen_fixture(name: &[u8], password: &[u8], wrong: &[u8], logout: fn() -> bool) -> bool {
    // SO43 — the IGNITION leg goes FIRST OF ALL: it is headless, it drives the ignition seam rather
    // than the form, and it saves and restores the once-latch, so it must run before anything else
    // has touched either. Its own `:: LOGIN-IGNITION:` line carries its verdict; it is folded into
    // `ok` below as well, because a screen that never ignites makes every leg under it moot.
    let ignition_ok = ignition_leg();
    // LOGINCLOSE — FIRST, because it is the only leg that needs a real window and the rest of the
    // fixture must run on the headless form the ladder expects. It leaves the screen down and the
    // console resumed, which is the state `open` below assumes.
    let (close_route, reopened) = close_leg();
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
    // M4 — a SECOND login opens a NEW session. Driven through the SAME form the first one used:
    // Log Out left the fields cleared and the focus on Name (`open`), so this is exactly what a
    // person does. Another principal is the same path — `submit` re-reads `users::count()` and calls
    // the same `users::login`, which stamps the session principal and ensures that name's home.
    feed(name);
    let _ = consume_key(b'\t');
    feed(password);
    let _ = consume_key(b'\n');
    let second = !is_open()
        && matches!(users::whoami(&mut nb), Some(n) if &nb[..n] == name)
        && LOGINS.load(Ordering::Relaxed) >= 2;
    users::logout();
    // teardown: the screen closed, nothing open, the once-latch left for the real ignition
    take_down();
    FORM.lock().state = State::Closed;
    HEADLESS.store(false, Ordering::Relaxed);
    // LOGINCLOSE — `close_box_refused` is the CLOSE leg's verdict as a boolean the scorer can read:
    // TRUE means the pointer was measured to have no way in (`route=refused`), and it is also true for
    // `no-window`, where there was no row to make a claim about at all — the route token on the same
    // line says WHICH, so a harness that wants the strong reading asks for `close_route=refused`, and
    // a leg the harness could not run never reds a board it did not run on. `REACHABLE` and
    // `no-control` — a defect, and a control that could not fire — are the two readings that FAIL.
    let close_box_refused = matches!(close_route, "refused" | "no-window");
    // And the belt is required exactly where it could run: a harness that had a row must have seen the
    // stranded screen put back by the next key (`reopened`); one that had none has nothing to show.
    let heal_ok = close_route == "no-window" || reopened;
    let ok = esc_kept && wrong_kept && opened && passes_through && logout_ok && back && second && close_box_refused && heal_ok && ignition_ok;
    serial_println!(
        ":: LOGIN-SCREEN: window=no esc_kept={} wrong_kept={} opened={} passes_through={} logout={} back_after_logout={} second_login={} logins={} close_box_refused={} close_route={} reopened={} heals={} ignition={} -> {} ::",
        esc_kept, wrong_kept, opened, passes_through, logout_ok, back, second, LOGINS.load(Ordering::Relaxed), close_box_refused, close_route, reopened, HEALS.load(Ordering::Relaxed), ignition_ok, if ok { "PASS" } else { "FAIL —" }
    );
    ok
}
