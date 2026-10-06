//! CHARTER: Kernel — wm
//!
//! DIALOG (rmbp-ledger B395, MACPARITY rows 23/34) — THE alert widget. One anatomy for every alert the
//! desktop raises: the app's icon, a BOLD message, informative text (up to three lines, plus a countdown
//! line), one to three buttons with the DEFAULT rightmost and accent-coloured; Return answers the default,
//! Esc the cancel button. Drawn by the WM as a kernel row (owner [`OWNER`]), free-standing (centred, upper
//! third) or as a SHEET under an owner window's title bar (slid down in [`SLIDE_STEPS`] steps).
//!
//! APP-MODAL, never system-modal: while a dialog is up, a press on a window of its owner app is swallowed
//! and raises the dialog, and the owner's keys (focus on the owner) answer it; every other window, the
//! console and the shell keep their input (R88: input never falters). It TAKES focus only when the person
//! caused it (a crystal pick, `user`) or its owner app held focus — never from another app's typing
//! (`focus_theft`, counted, zero by construction).
//!
//! Posting is QUEUE-ONLY ([`post`]: `try_lock`, no heap, no `wm`) so a fault handler or a bus verb may raise
//! one; the window opens in [`service`] (the storage pass, `login::notice_service`). Shut Down / Restart /
//! Log Out confirm through [`power_confirm`] with a 60 s countdown that counts on the glass and proceeds on
//! expiry: `[power] confirm action=<a> countdown_s=60 answer=<ok/cancel/expired>`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::{metrics, theme, wm};

/// The dialog's kernel owner band (hit-tested, focusable, a control cluster whose close box cancels).
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x70;
/// Logical layout.
const W: usize = 440;
const H: usize = 168;
const BW: usize = 104;
const BH: usize = 26;
const ML: usize = 64;
const IL: usize = 3;
/// The power confirm's countdown.
pub const POWER_COUNTDOWN_S: u32 = 60;
/// The sheet's slide: this many steps of [`SLIDE_PX`] logical px, one per service pass.
const SLIDE_STEPS: u8 = 4;
const SLIDE_PX: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// A program stopped / an error.
    Stop,
    /// A caution (data loss, a refusal).
    Caution,
    /// The system's own mark (the power confirms).
    System,
}

/// What an answer does beyond closing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Act {
    None,
    /// 1 restart, 2 shut down, 3 log out, 9 the fixture's (acts on nothing).
    Power(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Default,
    Cancel,
    Expired,
    Button(u8),
}

#[derive(Clone, Copy)]
pub struct Dlg {
    pub icon: Icon,
    msg: [u8; ML],
    ml: u8,
    info: [[u8; ML]; IL],
    il: [u8; IL],
    pub btn: [&'static str; 3],
    pub nb: u8,
    /// The cancel button's index (Esc); `None` = Esc answers the default (a one-button alert).
    pub cancel: Option<u8>,
    /// The owner app's asid (`0` = free-standing, blocks nobody).
    pub owner: u64,
    /// SHEET: the owner window it slides from (`WIN_NONE` = free-standing).
    pub sheet_win: wm::WinId,
    /// The countdown (`0` = none): the default answers itself on expiry.
    pub countdown_s: u32,
    /// The person caused it (a crystal pick): it may take focus.
    pub user: bool,
    pub act: Act,
    pub title: &'static [u8],
}

fn copy(dst: &mut [u8; ML], s: &[u8]) -> u8 {
    let n = s.len().min(ML);
    for (i, &b) in s[..n].iter().enumerate() {
        dst[i] = if (0x20..0x7f).contains(&b) { b } else { b'?' };
    }
    n as u8
}

impl Dlg {
    /// A dialog with `message` and up to three `\n`-separated informative lines; buttons left to right, the
    /// LAST is the default.
    pub fn new(icon: Icon, title: &'static [u8], message: &[u8], info: &[u8], btn: &[&'static str]) -> Dlg {
        let mut d = Dlg { icon, msg: [0; ML], ml: 0, info: [[0; ML]; IL], il: [0; IL], btn: [""; 3], nb: 0, cancel: None, owner: 0, sheet_win: wm::WIN_NONE, countdown_s: 0, user: false, act: Act::None, title };
        d.ml = copy(&mut d.msg, message);
        for (i, line) in info.split(|&b| b == b'\n').take(IL).enumerate() {
            d.il[i] = copy(&mut d.info[i], line);
        }
        let n = btn.len().clamp(1, 3);
        for i in 0..n {
            d.btn[i] = if i < btn.len() { btn[i] } else { "OK" };
        }
        d.nb = n as u8;
        if n >= 2 {
            d.cancel = Some(0);
        }
        d
    }
    pub fn message(&self) -> &[u8] { &self.msg[..self.ml as usize] }
    pub fn info_lines(&self) -> usize { self.il.iter().filter(|&&l| l != 0).count() }
    pub fn default_ix(&self) -> u8 { self.nb.saturating_sub(1) }
    fn resolve(&self, a: Answer) -> u8 {
        match a {
            Answer::Default | Answer::Expired => self.default_ix(),
            Answer::Cancel => self.cancel.unwrap_or(self.default_ix()),
            Answer::Button(i) => i.min(self.default_ix()),
        }
    }
}

struct St {
    cur: Option<Dlg>,
    pend: Option<Dlg>,
    win: wm::WinId,
    deadline: u64,
    shown_s: u32,
    slide: u8,
    at: (usize, usize),
}

static ST: spin::Mutex<St> = spin::Mutex::new(St { cur: None, pend: None, win: wm::WIN_NONE, deadline: 0, shown_s: 0, slide: 0, at: (0, 0) });
/// The fixture's model-only mode: no `wm` row, no focus change.
static HEADLESS: AtomicBool = AtomicBool::new(false);
static OPENED: AtomicU32 = AtomicU32::new(0);
static FOCUS_THEFT: AtomicU32 = AtomicU32::new(0);
static BLOCKED: AtomicU32 = AtomicU32::new(0);
static DROPPED: AtomicU32 = AtomicU32::new(0);
/// The last answer: 0 none, 1 default/ok, 2 cancel, 3 expired, 4 another button.
static LAST: AtomicU32 = AtomicU32::new(0);
static LAST_ACT_FIRED: AtomicU64 = AtomicU64::new(0);

static SURF_AT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
fn surf() -> &'static mut [u32] {
    let n = metrics::size(W) * metrics::size(H);
    let mut p = SURF_AT.load(Ordering::Acquire);
    if p == 0 {
        let b: &'static mut [u32] = alloc::boxed::Box::leak(alloc::vec![0u32; n].into_boxed_slice());
        p = match SURF_AT.compare_exchange(0, b.as_mut_ptr() as usize, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => b.as_mut_ptr() as usize,
            Err(won) => won,
        };
    }
    // SAFETY: one leaked buffer of `n` words, written by the painter on the window-safe path, read by `wm`'s composite (instgui's contract).
    unsafe { core::slice::from_raw_parts_mut(p as *mut u32, n) }
}

/// Queue a dialog (QUEUE ONLY: `try_lock`, no heap, no `wm`). One pending behind the one on the glass; more are dropped, counted.
pub fn post(d: Dlg) -> bool {
    let Some(mut g) = ST.try_lock() else {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    if g.pend.is_some() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    g.pend = Some(d);
    true
}

/// Is a dialog on the glass (or in the model, headless)?
pub fn is_up() -> bool {
    ST.lock().cur.is_some()
}

/// The focus rule (Mac's): an alert takes focus only when the person caused it or its own app held focus.
pub fn should_focus(user: bool, focus: u64, owner: u64) -> bool {
    user || (owner != 0 && focus == owner)
}

/// App-modal: does the dialog up now block presses on `owner`'s windows? Never the console, never another app.
pub fn blocks_owner(owner: u64) -> bool {
    ST.lock().cur.map(|d| d.owner != 0 && d.owner == owner).unwrap_or(false)
}

/// Button `i` of `nb` in logical px: right-aligned, the default (last) rightmost.
fn btn_rect(i: u8, nb: u8) -> (usize, usize, usize, usize) {
    let from_right = (nb - 1 - i) as usize;
    let x = W - 20 - BW - from_right * (BW + 12);
    (x, H - 20 - BH, BW, BH)
}

fn text(px: &mut [u32], x: usize, y: usize, s: &[u8], ink: u32, bold: bool) {
    let _ = metrics::text(px, metrics::size(W), metrics::size(H), W, x, y, s, ink, bold, crate::video::text::Face::Ui);
}
fn fill(px: &mut [u32], x: usize, y: usize, w: usize, h: usize, c: u32) {
    let (w, h) = (w.min(W.saturating_sub(x)), h.min(H.saturating_sub(y)));
    metrics::fill(px, metrics::size(W), x, y, w, h, c);
}

fn paint(d: &Dlg, left_s: u32) {
    let px = surf();
    fill(px, 0, 0, W, H, theme::chrome_face());
    // the icon: the app's mark (a rounded tile and its glyph)
    let (ic, glyph): (u32, &[u8]) = match d.icon { Icon::Stop => (theme::ctrl_close(), b"!"), Icon::Caution => (theme::ctrl_min(), b"!"), Icon::System => (theme::accent(), b"U") };
    fill(px, 22, 22, 48, 48, ic);
    fill(px, 22, 22, 48, 1, theme::chrome_face());
    fill(px, 22, 69, 48, 1, theme::chrome_face());
    let gw = metrics::ladvance(glyph, true, crate::video::text::Face::Ui);
    text(px, 22 + 24usize.saturating_sub(gw / 2), 36, glyph, theme::bevel_light(), true);
    // the bold message, then the informative text
    text(px, 88, 22, d.message(), theme::content_text(), true);
    let mut y = 48;
    for i in 0..IL {
        if d.il[i] != 0 {
            text(px, 88, y, &d.info[i][..d.il[i] as usize], theme::title_text_active(), false);
            y += 18;
        }
    }
    if d.countdown_s != 0 {
        let line = alloc::format!("automatically in {} second{}.", left_s, if left_s == 1 { "" } else { "s" });
        text(px, 88, y, line.as_bytes(), theme::title_text_active(), false);
    }
    for i in 0..d.nb {
        let (x, by, w, h) = btn_rect(i, d.nb);
        let def = i == d.default_ix();
        fill(px, x, by, w, h, if def { theme::accent() } else { theme::button_face() });
        fill(px, x, by, w, 1, theme::frame_line());
        fill(px, x, by + h - 1, w, 1, theme::frame_line());
        fill(px, x, by, 1, h, theme::frame_line());
        fill(px, x + w - 1, by, 1, h, theme::frame_line());
        let l = d.btn[i as usize].as_bytes();
        let tw = metrics::ladvance(l, false, crate::video::text::Face::Ui);
        text(px, x + w.saturating_sub(tw) / 2, by + 5, l, if def { theme::bevel_light() } else { theme::button_text() }, false);
    }
}

fn left_s(deadline: u64) -> u32 {
    let now = crate::arch::ms();
    (deadline.saturating_sub(now).div_ceil(1000)) as u32
}

/// Open the pending dialog (window-safe callers): the row, its placement, the focus rule, the witness.
fn open_pending() {
    let (d, deadline) = {
        let Some(mut g) = ST.try_lock() else { return };
        if g.cur.is_some() || g.pend.is_none() {
            return;
        }
        let d = g.pend.take().unwrap();
        g.cur = Some(d);
        g.deadline = if d.countdown_s != 0 { crate::arch::ms().saturating_add(d.countdown_s as u64 * 1000) } else { 0 };
        g.shown_s = d.countdown_s;
        g.win = wm::WIN_NONE;
        g.slide = 0;
        (d, g.deadline)
    };
    OPENED.fetch_add(1, Ordering::Relaxed);
    let focus0 = wm::focus_asid();
    let mut win = wm::WIN_NONE;
    let mut sheet = false;
    if !HEADLESS.load(Ordering::Relaxed) {
        if let Some((_s, ow, oh)) = wm::spawn_geometry_native(metrics::size(W), metrics::size(H)) {
            let (pw, ph) = { let i = super::WRITER.lock().info(); (i.width, i.height) };
            let (mut x, mut y) = (pw.saturating_sub(ow) / 2 + wm::BORDER(), ph.saturating_sub(oh) / 4 + wm::TITLE_H() + wm::BORDER());
            let mut slide = 0u8;
            if d.sheet_win != wm::WIN_NONE {
                if let Some((fx, fy, fw, _)) = wm::frame_of(d.sheet_win) {
                    x = fx + fw.saturating_sub(ow) / 2 + wm::BORDER();
                    y = fy + 2 * wm::TITLE_H() + wm::BORDER();
                    sheet = true;
                    slide = SLIDE_STEPS;
                }
            }
            paint(&d, d.countdown_s);
            let (sw, sh) = (metrics::size(W), metrics::size(H));
            let start_y = y.saturating_sub(slide as usize * metrics::size(SLIDE_PX));
            win = wm::create_at_native(OWNER, surf().as_mut_ptr() as usize, sw * sh * 4, sw as u32, sh as u32, (sw * 4) as u32, d.title, x, start_y);
            if win != wm::WIN_NONE {
                let _ = wm::present(win);
                let mut g = ST.lock();
                g.win = win;
                g.slide = slide;
                g.at = (x, y);
            }
        }
    }
    let take = should_focus(d.user, focus0, d.owner);
    if take && win != wm::WIN_NONE {
        wm::focus_changed(OWNER);
    }
    if take && !d.user && focus0 != 0 && focus0 != d.owner {
        FOCUS_THEFT.fetch_add(1, Ordering::Relaxed); // unreachable by `should_focus`; counted so a regression reads on the wire
    }
    let _ = deadline;
    let kind = match d.act { Act::Power(_) => "power", Act::None => "alert" };
    let action = power_word(d.act);
    serial_println!(
        "[dialog] open kind={} action={} owner={} sheet={} focus={} countdown_s={} buttons={} default=right win={}",
        kind, action, d.owner, sheet as u8, if take { "taken" } else { "kept" }, d.countdown_s, d.nb, win
    );
}

fn power_word(a: Act) -> &'static str {
    match a {
        Act::Power(1) => "restart",
        Act::Power(2) => "shutdown",
        Act::Power(3) => "logout",
        Act::Power(_) => "fixture",
        Act::None => "-",
    }
}

/// Answer the dialog on the glass: close its row, say it, act.
pub fn answer(a: Answer) {
    let (d, win) = {
        let mut g = ST.lock();
        let Some(d) = g.cur.take() else { return };
        let w = core::mem::replace(&mut g.win, wm::WIN_NONE);
        g.deadline = 0;
        (d, w)
    };
    if win != wm::WIN_NONE {
        wm::close(win);
    }
    let ix = d.resolve(a);
    let ok = ix == d.default_ix() && !(a == Answer::Cancel && d.cancel.is_some());
    LAST.store(match a { Answer::Expired => 3, _ if ok => 1, Answer::Cancel => 2, _ => if Some(ix) == d.cancel { 2 } else { 4 } }, Ordering::Relaxed);
    let word = match a { Answer::Expired => "expired", _ if ok => "ok", _ => "cancel" };
    match d.act {
        Act::Power(k) => {
            serial_println!("[power] confirm action={} countdown_s={} answer={}", power_word(d.act), d.countdown_s, word);
            if ok && k != 9 {
                LAST_ACT_FIRED.store(k as u64, Ordering::Relaxed);
                crate::video::crystal::power_fire(k); // Shut Down / Restart do not return
            }
        }
        Act::None => {
            serial_println!("[dialog] answer={} button={} title={}", word, d.btn[ix as usize], core::str::from_utf8(d.title).unwrap_or("?"));
        }
    }
    open_pending();
}

/// The service (the storage pass): open a pending dialog, slide a sheet, count down, expire.
pub fn service() {
    open_pending();
    let (cur, win, deadline, shown, slide, at) = {
        let Some(g) = ST.try_lock() else { return };
        (g.cur, g.win, g.deadline, g.shown_s, g.slide, g.at)
    };
    let Some(d) = cur else { return };
    if slide != 0 && win != wm::WIN_NONE {
        let s = slide - 1;
        let _ = wm::move_to(win, at.0, at.1.saturating_sub(s as usize * metrics::size(SLIDE_PX)));
        ST.lock().slide = s;
        if s == 0 {
            serial_println!("[dialog] sheet slid steps={} win={}", SLIDE_STEPS, win);
        }
    }
    if deadline != 0 {
        if crate::arch::ms() >= deadline {
            answer(Answer::Expired);
            return;
        }
        let l = left_s(deadline);
        if l != shown {
            ST.lock().shown_s = l;
            if win != wm::WIN_NONE {
                paint(&d, l);
                let _ = wm::present(win);
            }
        }
    }
}

fn route_key(c: u8, focused: bool) -> bool {
    if !focused || ST.lock().cur.is_none() {
        return false;
    }
    match c {
        b'\r' | b'\n' => answer(Answer::Default),
        0x1b => answer(Answer::Cancel),
        _ => {} // an alert takes no text: the key is the dialog's and goes nowhere
    }
    true
}

/// A key (every key route asks `users::screen_key` first, and it asks this): the dialog's only while it — or its
/// owner app — holds focus. Return answers the default, Esc the cancel.
pub fn key(c: u8) -> bool {
    let owner = match ST.try_lock() { Some(g) => match g.cur { Some(d) => d.owner, None => return false }, None => return false };
    let f = wm::focus_asid();
    route_key(c, f == OWNER || (owner != 0 && f == owner))
}

/// A press (asked from `users::screen_press`, ahead of the router's window arms): a press on the dialog's content
/// is its own (a button answers); its close box cancels; a press on an OWNER window is swallowed and raises the
/// dialog (app-modal). Everything else is not ours.
pub fn press(x: i32, y: i32) -> bool {
    let (cur, win) = match ST.try_lock() { Some(g) => (g.cur, g.win), None => return false };
    let Some(d) = cur else { return false };
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == win && win != wm::WIN_NONE => {
            if wm::close_box_hit(win, x, y) {
                answer(Answer::Cancel);
                return true;
            }
            let Some(info) = wm::info(win) else { return false };
            if x < info.x as i32 || y < info.y as i32 {
                return false; // the title bar: the WM's (a drag)
            }
            let (lx, ly) = (metrics::to_logical(x as usize - info.x), metrics::to_logical(y as usize - info.y));
            if lx >= W || ly >= H {
                return false;
            }
            if wm::focus_asid() != OWNER {
                wm::focus_changed(OWNER);
            }
            for i in 0..d.nb {
                let (bx, by, bw, bh) = btn_rect(i, d.nb);
                if lx >= bx && lx < bx + bw && ly >= by && ly < by + bh {
                    answer(if i == d.default_ix() { Answer::Default } else { Answer::Button(i) });
                    break;
                }
            }
            true
        }
        Some((_, o, _)) if d.owner != 0 && o == d.owner => {
            BLOCKED.fetch_add(1, Ordering::Relaxed);
            serial_println!("[dialog] blocked press owner={} (app-modal: the alert answers first)", o);
            if win != wm::WIN_NONE {
                wm::focus_changed(OWNER);
            }
            true
        }
        _ => false,
    }
}

// ── the unsaved-state hook (Log Out lists what would be lost) ───────────────────────────────────────────

const UN_CAP: usize = 8;
const UN_NAME: usize = 20;
static UNSAVED: spin::Mutex<[(u64, [u8; UN_NAME], u8); UN_CAP]> = spin::Mutex::new([(0, [0; UN_NAME], 0); UN_CAP]);

/// An app declares (`dirty`) or clears its unsaved state; the Log Out confirm lists the declared names. TEXTEDIT's hook.
pub fn unsaved_declare(owner: u64, name: &[u8], dirty: bool) {
    let mut t = UNSAVED.lock();
    if let Some(e) = t.iter_mut().find(|e| e.0 == owner && owner != 0) {
        if !dirty {
            e.0 = 0;
        }
        return;
    }
    if dirty {
        if let Some(e) = t.iter_mut().find(|e| e.0 == 0) {
            let n = name.len().min(UN_NAME);
            e.0 = owner;
            e.1[..n].copy_from_slice(&name[..n]);
            e.2 = n as u8;
        }
    }
}

/// `Unsaved: a, b` (empty when nobody declared).
fn unsaved_line(out: &mut [u8; ML]) -> usize {
    let t = UNSAVED.lock();
    let mut n = 0usize;
    for e in t.iter().filter(|e| e.0 != 0) {
        let pre: &[u8] = if n == 0 { b"Unsaved: " } else { b", " };
        for &b in pre.iter().chain(e.1[..e.2 as usize].iter()) {
            if n < ML {
                out[n] = b;
                n += 1;
            }
        }
    }
    n
}

/// SHUT DOWN / RESTART / LOG OUT (`kind` 1/2/3, 9 = the fixture's, which acts on nothing): the confirm, with its
/// 60 s countdown, opened now (window-safe callers: the crystal's press). `true` when it was raised.
pub fn power_confirm(kind: u8) -> bool {
    let (msg, info, verb): (&[u8], &[u8], &'static str) = match kind {
        1 => (b"Are you sure you want to restart your computer now?", b"If you do nothing, the computer will restart", "Restart"),
        2 => (b"Are you sure you want to shut down your computer now?", b"If you do nothing, the computer will shut down", "Shut Down"),
        3 => (b"Are you sure you want to quit all apps and log out now?", b"If you do nothing, you will be logged out", "Log Out"),
        _ => (b"Fixture: confirm a power action?", b"If you do nothing, the fixture proceeds", "Proceed"),
    };
    let mut d = Dlg::new(Icon::System, b"UnaOS", msg, info, &["Cancel", verb]);
    if kind == 3 {
        let mut l = [0u8; ML];
        let n = unsaved_line(&mut l);
        if n != 0 {
            d.il[1] = copy(&mut d.info[1], &l[..n]);
        }
    }
    d.countdown_s = POWER_COUNTDOWN_S;
    d.user = true;
    d.act = Act::Power(kind);
    if !post(d) {
        serial_println!("[power] confirm action={} refused (a dialog is already pending)", power_word(d.act));
        return false;
    }
    open_pending();
    true
}

/// `Program stopped` for a program the person launched from the glass (queue-only: the fault path).
pub fn post_program_stopped(name: &[u8]) -> bool {
    let mut m = [0u8; ML];
    let mut n = 0usize;
    for &b in b"\"".iter().chain(name.iter().take(40)).chain(b"\" quit unexpectedly.".iter()) {
        if n < ML {
            m[n] = b;
            n += 1;
        }
    }
    post(Dlg::new(Icon::Stop, b"Program stopped", &m[..n], b"The program was stopped by a fault.\nIts windows have closed.", &["OK"]))
}

// ── `tests notice` (M4): the anatomy, the default on the right, Esc, app-modal, the focus rule, the confirm ──

/// Model-only (no `wm` row, no focus change, no power action): returns the `:: DIALOG:` fields' verdicts.
pub fn fixture() -> (bool, bool, bool, bool, u32, bool) {
    let was = HEADLESS.swap(true, Ordering::Relaxed);
    let saved = { let mut g = ST.lock(); (g.cur.take(), g.pend.take(), core::mem::replace(&mut g.win, wm::WIN_NONE), g.deadline) };
    let theft0 = FOCUS_THEFT.load(Ordering::Relaxed);
    // anatomy + default=right
    let d = Dlg::new(Icon::Caution, b"Fixture", b"Bold message", b"informative one\ninformative two", &["Cancel", "OK"]);
    let anatomy = d.icon == Icon::Caution && d.message() == b"Bold message" && d.info_lines() == 2 && d.nb == 2 && d.cancel == Some(0);
    let right = btn_rect(d.default_ix(), d.nb).0 > btn_rect(0, d.nb).0 && btn_rect(d.default_ix(), d.nb).0 + BW + 20 == W;
    // Esc = cancel, Return = default
    let _ = post(d);
    open_pending();
    let esc = route_key(0x1b, true) && LAST.load(Ordering::Relaxed) == 2 && !is_up();
    let _ = post(d);
    open_pending();
    let ret = route_key(b'\r', true) && LAST.load(Ordering::Relaxed) == 1 && !is_up();
    // unfocused: the key is not the dialog's (typing elsewhere is untouched)
    let _ = post(d);
    open_pending();
    let passes = !route_key(b'a', false) && is_up();
    answer(Answer::Cancel);
    // app-modal: the owner's windows are blocked, nobody else's
    let mut s = d;
    s.owner = 0x4242;
    let _ = post(s);
    open_pending();
    let modal = blocks_owner(0x4242) && !blocks_owner(0x4343) && !blocks_owner(0);
    answer(Answer::Cancel);
    // the focus rule: another app's typing keeps focus; the person's pick and the owner's own sheet take it
    let rule = !should_focus(false, 0x4343, 0x4242) && !should_focus(false, 0, 0) && should_focus(true, 0x4343, 0) && should_focus(false, 0x4242, 0x4242);
    let theft = FOCUS_THEFT.load(Ordering::Relaxed) - theft0;
    // the confirm: a fixture power action (acts on nothing), its countdown expires -> proceeds; another is cancelled
    let fired0 = LAST_ACT_FIRED.load(Ordering::Relaxed);
    let up = power_confirm(9) && is_up() && ST.lock().deadline != 0 && ST.lock().cur.map(|d| d.countdown_s) == Some(POWER_COUNTDOWN_S);
    ST.lock().deadline = 1;
    service();
    let expired = !is_up() && LAST.load(Ordering::Relaxed) == 3;
    let _ = power_confirm(9);
    let _ = route_key(0x1b, true);
    let cancelled = !is_up() && LAST.load(Ordering::Relaxed) == 2 && LAST_ACT_FIRED.load(Ordering::Relaxed) == fired0;
    let confirm = up && expired && cancelled;
    {
        let mut g = ST.lock();
        g.cur = saved.0;
        g.pend = saved.1;
        g.win = saved.2;
        g.deadline = saved.3;
    }
    HEADLESS.store(was, Ordering::Relaxed);
    (anatomy && passes, right, esc && ret, modal && rule, theft, confirm)
}
