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
    /// DOCK2 M6 (B394): a caller's `fn(ok: bool)` (as `usize`), run on the answer — the dock's Empty Trash. Keep it
    /// queue-only: the answer runs in the press/key route.
    Hook(usize),
    /// DIALOG2: a ring-3 (bus) dialog — the answer goes to the owner's input ring with this token.
    Reply(u8),
    /// DIALOG2: TEXTEDIT's close sheet (Don't Save · Cancel · Save).
    EditClose,
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
    bl: [[u8; BL]; 3],
    bll: [u8; 3],
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
    ttl: [u8; TL],
    tl: u8,
    /// DIALOG2: raised over the login screen (the power row): the dialog holds the modal ceiling while up.
    pub screen: bool,
}

/// DIALOG2: a button label's and the window title's bounds (owned: a bus caller's bytes, a notice's title).
const BL: usize = 16;
const TL: usize = 24;

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
    pub fn new(icon: Icon, title: &[u8], message: &[u8], info: &[u8], btn: &[&[u8]]) -> Dlg {
        let mut d = Dlg { icon, msg: [0; ML], ml: 0, info: [[0; ML]; IL], il: [0; IL], bl: [[0; BL]; 3], bll: [0; 3], nb: 0, cancel: None, owner: 0, sheet_win: wm::WIN_NONE, countdown_s: 0, user: false, act: Act::None, ttl: [0; TL], tl: 0, screen: false };
        let n = title.len().min(TL);
        for (i, &b) in title[..n].iter().enumerate() {
            d.ttl[i] = if (0x20..0x7f).contains(&b) { b } else { b'?' };
        }
        d.tl = n as u8;
        d.ml = copy(&mut d.msg, message);
        for (i, line) in info.split(|&b| b == b'\n').take(IL).enumerate() {
            d.il[i] = copy(&mut d.info[i], line);
        }
        let n = btn.len().clamp(1, 3);
        for i in 0..n {
            let l: &[u8] = if i < btn.len() && !btn[i].is_empty() { btn[i] } else { b"OK" };
            let k = l.len().min(BL);
            for (j, &b) in l[..k].iter().enumerate() {
                d.bl[i][j] = if (0x20..0x7f).contains(&b) { b } else { b'?' };
            }
            d.bll[i] = k as u8;
        }
        d.nb = n as u8;
        if n >= 2 {
            d.cancel = Some((0..n as u8).find(|&i| d.btn(i) == b"Cancel").unwrap_or(0)); // DIALOG2: Esc is the button named Cancel (Don't Save · Cancel · Save), else the leftmost
        }
        d
    }
    pub fn message(&self) -> &[u8] { &self.msg[..self.ml as usize] }
    pub fn title(&self) -> &[u8] { &self.ttl[..self.tl as usize] }
    pub fn btn(&self, i: u8) -> &[u8] { &self.bl[i as usize][..self.bll[i as usize] as usize] }
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
    fill(px, 0, 0, W, H, theme::CHROME_FACE);
    // the icon: the app's mark (a rounded tile and its glyph)
    let (ic, glyph): (u32, &[u8]) = match d.icon { Icon::Stop => (theme::CTRL_CLOSE, b"!"), Icon::Caution => (theme::CTRL_MIN, b"!"), Icon::System => (theme::ACCENT, b"U") };
    fill(px, 22, 22, 48, 48, ic);
    fill(px, 22, 22, 48, 1, theme::CHROME_FACE);
    fill(px, 22, 69, 48, 1, theme::CHROME_FACE);
    let gw = metrics::ladvance(glyph, true, crate::video::text::Face::Ui);
    text(px, 22 + 24usize.saturating_sub(gw / 2), 36, glyph, theme::BEVEL_LIGHT, true);
    // the bold message, then the informative text
    text(px, 88, 22, d.message(), theme::CONTENT_TEXT, true);
    let mut y = 48;
    for i in 0..IL {
        if d.il[i] != 0 {
            text(px, 88, y, &d.info[i][..d.il[i] as usize], theme::TITLE_TEXT_ACTIVE, false);
            y += 18;
        }
    }
    if d.countdown_s != 0 {
        let line = alloc::format!("automatically in {} second{}.", left_s, if left_s == 1 { "" } else { "s" });
        text(px, 88, y, line.as_bytes(), theme::TITLE_TEXT_ACTIVE, false);
    }
    for i in 0..d.nb {
        let (x, by, w, h) = btn_rect(i, d.nb);
        let def = i == d.default_ix();
        fill(px, x, by, w, h, if def { theme::ACCENT } else { theme::BUTTON_FACE });
        fill(px, x, by, w, 1, theme::FRAME_LINE);
        fill(px, x, by + h - 1, w, 1, theme::FRAME_LINE);
        fill(px, x, by, 1, h, theme::FRAME_LINE);
        fill(px, x + w - 1, by, 1, h, theme::FRAME_LINE);
        let l = d.btn(i);
        let tw = metrics::ladvance(l, false, crate::video::text::Face::Ui);
        text(px, x + w.saturating_sub(tw) / 2, by + 5, l, if def { theme::BEVEL_LIGHT } else { theme::BUTTON_TEXT }, false);
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
            win = wm::create_at_native(OWNER, surf().as_mut_ptr() as usize, sw * sh * 4, sw as u32, sh as u32, (sw * 4) as u32, d.title(), x, start_y);
            if win != wm::WIN_NONE {
                if d.screen {
                    wm::set_modal_top(win); // DIALOG2: over the login screen — the ceiling is the dialog's while it is up
                    let _ = wm::raise_one(win);
                }
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
    let kind = match d.act { Act::Power(_) => "power", Act::None | Act::Hook(_) => "alert", Act::Reply(_) => "bus", Act::EditClose => "save" };
    let action = power_word(d.act);
    serial_println!(
        "[dialog] open kind={} action={} owner={} sheet={} focus={} countdown_s={} buttons={} default=right screen={} win={}",
        kind, action, d.owner, sheet as u8, if take { "taken" } else { "kept" }, d.countdown_s, d.nb, d.screen as u8, win
    );
}

fn power_word(a: Act) -> &'static str {
    match a {
        Act::Power(1) => "restart",
        Act::Power(2) => "shutdown",
        Act::Power(3) => "logout",
        Act::Power(4) => "sleep",
        Act::Power(_) => "fixture",
        Act::None | Act::Hook(_) | Act::Reply(_) | Act::EditClose => "-",
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
        wm::clear_modal_top(win);
        wm::close(win);
    }
    #[cfg(feature = "login")]
    if d.screen {
        crate::video::crystal::login::screen_regain(); // DIALOG2: the login screen takes its ceiling back
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
            serial_println!("[dialog] answer={} button={} title={}", word, core::str::from_utf8(d.btn(ix)).unwrap_or("?"), core::str::from_utf8(d.title()).unwrap_or("?")); #[cfg(target_arch = "x86_64")] if ok && d.title() == super::panicscreen::DLG_TITLE { super::panicscreen::show_log(); } // PANICSCREEN (B406): `Show log`
        }
        Act::Hook(f) => {
            serial_println!("[dialog] answer={} button={} title={}", word, core::str::from_utf8(d.btn(ix)).unwrap_or("?"), core::str::from_utf8(d.title()).unwrap_or("?"));
            // SAFETY: only `Act::Hook` producers store here, always a valid `fn(bool)` cast to `usize`.
            if f != 0 { let h: fn(bool) = unsafe { core::mem::transmute::<usize, fn(bool)>(f) }; h(ok); }
        }
        Act::Reply(token) => reply(d.owner, token, ix, word),
        Act::EditClose => {
            serial_println!("[dialog] answer={} button={} title={}", word, core::str::from_utf8(d.btn(ix)).unwrap_or("?"), core::str::from_utf8(d.title()).unwrap_or("?"));
            crate::video::textedit::close_answer(ix);
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
    power_confirm_at(kind, false)
}

/// DIALOG2: the same confirm, raised OVER the login screen (its power row): the dialog holds the modal ceiling.
pub fn power_confirm_on_screen(kind: u8) -> bool {
    power_confirm_at(kind, true)
}

fn power_confirm_at(kind: u8, screen: bool) -> bool {
    let (msg, info, verb): (&[u8], &[u8], &'static str) = match kind {
        1 => (b"Are you sure you want to restart your computer now?", b"If you do nothing, the computer will restart", "Restart"),
        2 => (b"Are you sure you want to shut down your computer now?", b"If you do nothing, the computer will shut down", "Shut Down"),
        3 => (b"Are you sure you want to quit all apps and log out now?", b"If you do nothing, you will be logged out", "Log Out"),
        _ => (b"Fixture: confirm a power action?", b"If you do nothing, the fixture proceeds", "Proceed"),
    };
    let mut d = Dlg::new(Icon::System, b"UnaOS", msg, info, &[b"Cancel", verb.as_bytes()]);
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
    d.screen = screen;
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
    post(Dlg::new(Icon::Stop, b"Program stopped", &m[..n], b"The program was stopped by a fault.\nIts windows have closed.", &[b"OK"]))
}

// ── `tests notice` (M4): the anatomy, the default on the right, Esc, app-modal, the focus rule, the confirm ──

/// Model-only (no `wm` row, no focus change, no power action): returns the `:: DIALOG:` fields' verdicts.
pub fn fixture() -> (bool, bool, bool, bool, u32, bool) {
    let was = HEADLESS.swap(true, Ordering::Relaxed);
    let saved = { let mut g = ST.lock(); (g.cur.take(), g.pend.take(), core::mem::replace(&mut g.win, wm::WIN_NONE), g.deadline) };
    let theft0 = FOCUS_THEFT.load(Ordering::Relaxed);
    // anatomy + default=right
    let d = Dlg::new(Icon::Caution, b"Fixture", b"Bold message", b"informative one\ninformative two", &[b"Cancel", b"OK"]);
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

// ── DIALOG2 (rmbp-ledger B404) — every notice SORTED: errors are this alert, information is a toast ──────────
//
// The login screen's Alert window is gone. Every caller that used it (`users::screen_notice` for the paths that
// may not open a window — xHCI, a fault, the store, wincap — and the window-safe Quarry / prtscr / powerui /
// refused Log Out) calls [`notice`], which reads the ONE sorting table [`SORTED`]. Queue-only on every path.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An operation failed or was refused: the alert, app-modal to the owner it concerns.
    Error,
    /// Something happened: a toast (3 s, top right, no focus).
    Info,
}

/// THE sorting table. An untabled title is INFORMATION (the Trash two-step, Get Info).
pub const SORTED: [(&[u8], Kind); 7] = [
    (b"Quarry", Kind::Error),
    (b"Storage read-only", Kind::Error),
    (b"Log Out", Kind::Error),
    (b"Screenshot saved", Kind::Info),
    (b"USB stick removed", Kind::Info),
    (b"Low Battery", Kind::Info),
    (b"Too many windows", Kind::Info),
];

pub fn kind_of(title: &[u8]) -> Kind {
    SORTED.iter().find(|e| e.0 == title).map(|e| e.1).unwrap_or(Kind::Info)
}

static TO_DIALOG: AtomicU32 = AtomicU32::new(0);
static TO_TOAST: AtomicU32 = AtomicU32::new(0);

/// The owner (and sheet window) an error concerns: a Quarry refusal is a sheet on the Quarry window.
fn owner_of_error(title: &[u8]) -> (u64, wm::WinId, bool) {
    #[cfg(feature = "quarry")]
    if title == b"Quarry" {
        return (super::quarry::live::OWNER, super::quarry::live::win(), false);
    }
    (0, wm::WIN_NONE, title == b"Log Out") // the refused Log Out: the person's own pick (crystal or `logout`) — it may take focus
}

/// THE notice entry (queue-only: no heap, no `wm`): `text` is up to three `\n` lines. `Program stopped` keeps
/// DIALOG's rule (a dialog only for a program the person launched from the glass), read from the explicit origin.
pub fn notice(title: &[u8], text: &[u8]) -> bool {
    if title == b"Program stopped" {
        let glass = super::toast::current_is_glass();
        serial_println!("[dialog] program-stopped glass={} -> {}", glass as u8, if glass { "dialog" } else { "toast" });
        return if glass { post_program_stopped(text) } else { super::toast::post(title, text) };
    }
    let kind = kind_of(title);
    serial_println!("[notice] route title={} kind={} -> {}", core::str::from_utf8(title).unwrap_or("?"), if kind == Kind::Error { "error" } else { "info" }, if kind == Kind::Error { "dialog" } else { "toast" });
    match kind {
        Kind::Error => {
            TO_DIALOG.fetch_add(1, Ordering::Relaxed);
            let mut d = Dlg::new(Icon::Caution, title, title, text, &[b"OK"]);
            let (owner, sheet, user) = owner_of_error(title);
            d.owner = owner;
            d.sheet_win = sheet;
            d.user = user;
            post(d)
        }
        Kind::Info => {
            TO_TOAST.fetch_add(1, Ordering::Relaxed);
            super::toast::post(title, text)
        }
    }
}

/// Model-only mode for another module's fixture (the LOGOUT fixture): returns the previous setting.
pub fn headless(on: bool) -> bool {
    HEADLESS.swap(on, Ordering::Relaxed)
}

/// Open what is pending now (window-safe callers that must see it up at once: the refused Log Out, fixtures).
pub fn open_now() {
    open_pending();
}

/// The dialog on the glass, if any: (title, owner, sheet).
pub fn current() -> Option<([u8; TL], usize, u64, bool)> {
    ST.lock().cur.map(|d| (d.ttl, d.tl as usize, d.owner, d.sheet_win != wm::WIN_NONE))
}

// ── over the login screen (the power row's confirm) ──

/// A dialog raised over the login screen is up: every key and press is its (the screen is modal anyway).
pub fn over_screen() -> bool {
    ST.try_lock().map(|g| g.cur.map(|d| d.screen).unwrap_or(false)).unwrap_or(false)
}

/// The screen's keys while [`over_screen`]: Return / Esc answer; the rest is swallowed (an alert takes no text).
pub fn screen_key(c: u8) -> bool {
    let _ = route_key(c, true);
    true
}

// ── the bus verbs: a ring-3 program raises its OWN alert / sheet / toast ──

static BUS_POSTED: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];

/// `BUS_VERB_DIALOG` / `SHEET` / `TOAST` for the kernel-stamped `owner` (the wm key: x86 slot+1, aarch64 asid;
/// `0` = the door). The reply is status-only and immediate; a dialog's ANSWER arrives later as
/// `INPUT_EV_DIALOG_ANSWER` on the owner's input ring. `-16` while another dialog is pending, `-22` malformed.
pub fn bus_fulfil(verb: u8, owner: u64, body: &[u8]) -> i64 {
    let Some(r) = una_abi::dialog_parse(body) else { return -22 };
    let mut name = [0u8; wm::MAX_TITLE];
    let n = if owner != 0 { wm::app_name_of(owner, &mut name) } else { 0 };
    let title: &[u8] = if n == 0 { b"Program" } else { &name[..n] };
    let vw = match verb { una_abi::BUS_VERB_SHEET => "sheet", una_abi::BUS_VERB_TOAST => "toast", _ => "dialog" };
    serial_println!("[dialog] bus verb={} owner={} token={} buttons={}", vw, owner, r.token, r.nb);
    let ix = match verb { una_abi::BUS_VERB_DIALOG => 0, una_abi::BUS_VERB_SHEET => 1, _ => 2 };
    BUS_POSTED[ix].fetch_add(1, Ordering::Relaxed);
    if verb == una_abi::BUS_VERB_TOAST {
        return if super::toast::post(title, r.message) { 0 } else { -16 };
    }
    let mut d = Dlg::new(Icon::Caution, title, r.message, r.info, &r.buttons[..(r.nb.max(1) as usize)]);
    d.owner = owner;
    d.act = Act::Reply(r.token);
    if verb == una_abi::BUS_VERB_SHEET {
        d.sheet_win = front_of(owner);
    }
    if post(d) { 0 } else { -16 }
}

/// The owner's frontmost window (the sheet's anchor), `WIN_NONE` when it has none (the sheet is then free-standing).
fn front_of(owner: u64) -> wm::WinId {
    if owner == 0 {
        return wm::WIN_NONE;
    }
    wm::front_of_owner(owner).unwrap_or(wm::WIN_NONE)
}

static REPLIED: AtomicU32 = AtomicU32::new(0);

/// Deliver a bus dialog's answer to its poster's input ring (by identity, never to the focused slot).
fn reply(owner: u64, token: u8, ix: u8, word: &str) {
    let ev = una_abi::dialog_answer_pack(token, ix);
    let user = owner != 0 && owner < wm::KERNEL_OWNER_BASE;
    #[cfg(target_arch = "x86_64")]
    let delivered = user && !HEADLESS.load(Ordering::Relaxed) && crate::arch::x86_64::syscall::user_input_push_owner(owner, ev);
    #[cfg(all(target_arch = "aarch64", any(feature = "baremetal", feature = "tegra_el0")))]
    let delivered = user && !HEADLESS.load(Ordering::Relaxed) && crate::arch::aarch64::syscall::user_input_push_owner(owner, ev);
    #[cfg(all(target_arch = "aarch64", not(any(feature = "baremetal", feature = "tegra_el0"))))]
    let delivered = { let _ = (user, ev); false };
    REPLIED.fetch_add(1, Ordering::Relaxed);
    serial_println!("[dialog] answer={} button={} token={} owner={} delivered={}", word, ix, token, owner, delivered as u8);
}

/// DIALOG2's model-only proof for `tests notice`: (errors routed to the dialog, information routed to the toast,
/// the three bus verbs each raised their kind and a dialog's answer came back with its token).
pub fn fixture2() -> (u32, u32, u32) {
    let was = HEADLESS.swap(true, Ordering::Relaxed);
    let saved = { let mut g = ST.lock(); (g.cur.take(), g.pend.take(), core::mem::replace(&mut g.win, wm::WIN_NONE), g.deadline) };
    let tsaved = super::toast::fixture_hold(true);
    // every SORTED row: an error opens the alert (and is answered), information queues a toast
    let (mut errs, mut infos) = (0u32, 0u32);
    for (title, kind) in SORTED.iter() {
        let (d0, t0) = (TO_DIALOG.load(Ordering::Relaxed), super::toast::queued());
        let _ = notice(title, b"fixture line");
        open_pending();
        match kind {
            Kind::Error => {
                let up = matches!(current(), Some((t, n, _, _)) if &t[..n] == *title);
                if up && TO_DIALOG.load(Ordering::Relaxed) == d0 + 1 {
                    errs += 1;
                }
                answer(Answer::Default);
            }
            Kind::Info => {
                if !is_up() && super::toast::queued() == t0 + 1 {
                    infos += 1;
                }
                super::toast::fixture_drain();
            }
        }
    }
    // the three bus verbs: dialog (answered with its token), sheet, toast
    let mut verbs = 0u32;
    let mut b = [0u8; 96];
    let r0 = REPLIED.load(Ordering::Relaxed);
    if let Some(n) = una_abi::dialog_body(9, b"Save changes?", b"fixture", &[b"Don't Save", b"Cancel", b"Save"], &mut b) {
        if bus_fulfil(una_abi::BUS_VERB_DIALOG, 0x4242, &b[..n]) == 0 {
            open_pending();
            let ok = ST.lock().cur.map(|d| d.nb == 3 && d.act == Act::Reply(9) && d.btn(2) == b"Save").unwrap_or(false);
            answer(Answer::Default);
            if ok && REPLIED.load(Ordering::Relaxed) == r0 + 1 && LAST.load(Ordering::Relaxed) == 1 {
                verbs += 1;
            }
        }
        if bus_fulfil(una_abi::BUS_VERB_SHEET, 0x4242, &b[..n]) == 0 {
            open_pending();
            if ST.lock().cur.map(|d| d.owner == 0x4242).unwrap_or(false) && blocks_owner(0x4242) {
                verbs += 1;
            }
            answer(Answer::Cancel);
        }
        let t0 = super::toast::queued();
        if bus_fulfil(una_abi::BUS_VERB_TOAST, 0x4242, &b[..n]) == 0 && super::toast::queued() == t0 + 1 && !is_up() {
            verbs += 1;
        }
        super::toast::fixture_drain();
    }
    let malformed = bus_fulfil(una_abi::BUS_VERB_DIALOG, 0x4242, &[1, 3, b'x']) == -22;
    super::toast::fixture_hold_restore(tsaved);
    {
        let mut g = ST.lock();
        g.cur = saved.0;
        g.pend = saved.1;
        g.win = saved.2;
        g.deadline = saved.3;
    }
    HEADLESS.store(was, Ordering::Relaxed);
    (errs, infos, if malformed { verbs } else { 0 })
}

/// `dialog <alert|sheet|toast> <message>` — the door drives the SAME fulfiller the bus verbs reach (owner 0: the
/// answer is said on the wire, `[dialog] answer=… token=0 owner=0 delivered=0`). The shell runs on the render task
/// (window-safe), so the alert opens at once.
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    let verb = match args.first().copied().unwrap_or("") {
        "alert" | "dialog" => una_abi::BUS_VERB_DIALOG,
        "sheet" => una_abi::BUS_VERB_SHEET,
        "toast" => una_abi::BUS_VERB_TOAST,
        _ => {
            console.println("usage: dialog <alert|sheet|toast> <message>");
            return;
        }
    };
    let msg = args[1..].join(" ");
    let mut b = [0u8; 192];
    let Some(n) = una_abi::dialog_body(0, msg.as_bytes(), b"raised at the door", &[b"Cancel", b"OK"], &mut b) else {
        console.println("dialog: the message is too long");
        return;
    };
    let st = bus_fulfil(verb, 0, &b[..n]);
    if st == 0 {
        open_pending();
    }
    console.println(&alloc::format!("dialog: {} status={}", args[0], st));
}
