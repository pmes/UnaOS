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
/// It does NOT swallow keys — [`consume_key`] is that seam, unchanged — and it does not ACT: nothing is
/// raised, focused, launched or closed by a `true` here. The router consumes the press and drops its
/// release, the grammar every furniture arm already follows.
pub fn press_swallow(x: i32, y: i32) -> bool {
    let _ = (x, y);
    is_open()
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
    let ok = esc_kept && wrong_kept && opened && passes_through && logout_ok && back && second && close_box_refused && heal_ok;
    serial_println!(
        ":: LOGIN-SCREEN: window=no esc_kept={} wrong_kept={} opened={} passes_through={} logout={} back_after_logout={} second_login={} logins={} close_box_refused={} close_route={} reopened={} heals={} -> {} ::",
        esc_kept, wrong_kept, opened, passes_through, logout_ok, back, second, LOGINS.load(Ordering::Relaxed), close_box_refused, close_route, reopened, HEALS.load(Ordering::Relaxed), if ok { "PASS" } else { "FAIL —" }
    );
    ok
}
