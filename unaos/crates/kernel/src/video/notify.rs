//! CHARTER: Kernel — wm
//!
//! NOTIFY (rmbp-ledger B418, MACPARITY row 24) — the Mac's notifications: they slide in at the top right under
//! the bar, STACK (up to [`STACK_MAX`], newest on top), dismiss on a click or after their time, and COLLECT in the
//! Notification Center the bar's bell opens. The toast (`toast.rs`, DIALOG B395) is this module's transient face:
//! its queue-only `post` stays the posting front every DIALOG2 notice uses, and its `service` hands the queue here.
//!
//! - **Posting** is QUEUE-ONLY ([`post_full`], [`post_quiet`], `toast::post`, `crate::video::notify`): `try_lock`,
//!   no heap, no `wm` — safe from a bus verb, a driver's hot-plug path and a fault handler.
//! - **The stack** is compat overlay rows (`wm::overlay_open`, owner 0): `wm::hit_test` never names them and nothing
//!   focuses them (R88). A press reaches them through [`press_at`], the first furniture arm of
//!   `strip::press_route`; it only RECORDS (atomics) and [`service`] acts. A card's action button runs its action; a
//!   press elsewhere on the card dismisses it; the pointer over a card pauses its timeout.
//! - **The ring** is the session's last [`RING`] notifications, in memory (nothing outlives a boot); the bar's bell
//!   carries the unread count (`menubar::bell_*`), and a press on it opens the Center: the ring newest first,
//!   grouped by app, with `Clear`.
//! - **Do Not Disturb** is Principia's `system.notify.dnd` (R79: a preference is Principia's), toggled in Settings >
//!   General; on, a notification collects silently (the badge counts it, no card shows).
//!
//! Wire: `[notify] post app=<a> title=<t> dnd=<0|1> -> banner|collected`, `[notify] show win=<n> slot=<i> title=<t>
//! ms=<ms> focus=kept`, `[notify] closed by=<timeout|click|overflow> title=<t>`, `[notify] action <label> -> <what>`,
//! `[notify] center open items=<n> apps=<n>` / `[notify] center closed by=<bell|outside|clear>`, and `tests notify` →
//! `:: NOTIFY: stack_max=4 center=ok ring=100 badge=ok dnd=<0|1> posted=<n> -> PASS ::`.
//! Design: `docs/dev/evidence/rmbp-1005/notify.md`. The alert sound is owed (MACPARITY row 26).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::{metrics, theme, wm};

/// Cards visible at once.
pub const STACK_MAX: usize = 4;
/// The session ring the Center lists.
pub const RING: usize = 100;
/// How long a card stays (the toast's time).
pub const CARD_MS: u64 = super::toast::TOAST_MS;
/// A hovered card keeps at least this much time once the pointer leaves.
const HOVER_KEEP_MS: u64 = 1500;

/// No action (a press dismisses).
pub const ACT_NONE: u8 = 0;
/// PANICSCREEN's `Show log` (`panicscreen::show_log`).
pub const ACT_SHOW_LOG: u8 = 1;
/// Open Quarry at the path in `arg` (a mounted volume).
pub const ACT_OPEN_DIR: u8 = 2;
/// A program's own action: `arg` = owner (8 bytes LE) + token (4 bytes LE); answered on the dialog bus path.
pub const ACT_ANSWER: u8 = 3;

/// Principia's key (namespace `system`).
pub const KEY_DND: &str = "notify.dnd";

const W: usize = 340;
const H: usize = 64;
const GAP: usize = 8;
const ICON: usize = 40;
const BTN_W: usize = 76;
const BTN_H: usize = 24;
const CW: usize = 360;
const CH: usize = 460;
const HEAD_H: usize = 36;
const GROUP_H: usize = 22;
const ITEM_H: usize = 40;

const AL: usize = 16;
const TL: usize = 32;
const LL: usize = 56;
const BL: usize = 12;
const XL: usize = 64;

#[derive(Clone, Copy)]
pub struct Note {
    app: [u8; AL],
    al: u8,
    title: [u8; TL],
    tl: u8,
    line: [u8; LL],
    ll: u8,
    label: [u8; BL],
    bl: u8,
    act: u8,
    arg: [u8; XL],
    xl: u8,
    quiet: bool,
    seq: u32,
    at: u64,
}

fn put(dst: &mut [u8], src: &[u8], first_line: bool) -> u8 {
    let mut n = 0;
    for &b in src.iter() {
        if n >= dst.len() || (first_line && b == b'\n') {
            break;
        }
        dst[n] = if (0x20..0x7f).contains(&b) { b } else { b'?' };
        n += 1;
    }
    n as u8
}

impl Note {
    const EMPTY: Note = Note { app: [0; AL], al: 0, title: [0; TL], tl: 0, line: [0; LL], ll: 0, label: [0; BL], bl: 0, act: 0, arg: [0; XL], xl: 0, quiet: false, seq: 0, at: 0 };
    fn make(app: &[u8], title: &[u8], line: &[u8], label: &[u8], act: u8, arg: &[u8]) -> Note {
        let mut n = Note::EMPTY;
        n.al = put(&mut n.app, app, true);
        for b in n.app[..n.al as usize].iter_mut() {
            *b = b.to_ascii_lowercase();
        }
        n.tl = put(&mut n.title, title, true);
        n.ll = put(&mut n.line, line, true);
        if act != ACT_NONE {
            n.bl = put(&mut n.label, label, true);
            n.act = act;
            let k = arg.len().min(XL);
            n.arg[..k].copy_from_slice(&arg[..k]);
            n.xl = k as u8;
        }
        n
    }
    fn app(&self) -> &[u8] { &self.app[..self.al as usize] }
    fn title(&self) -> &[u8] { &self.title[..self.tl as usize] }
    fn line(&self) -> &[u8] { &self.line[..self.ll as usize] }
    fn label(&self) -> &[u8] { &self.label[..self.bl as usize] }
    fn arg(&self) -> &[u8] { &self.arg[..self.xl as usize] }
    fn has_button(&self) -> bool { self.act != ACT_NONE && self.bl > 0 }
}

fn s(b: &[u8]) -> &str {
    core::str::from_utf8(b).unwrap_or("?")
}

// ── the inbound queue (post side: try_lock, no heap) ─────────────────────────────────────────────────────

const IQ: usize = 8;
struct In {
    q: [Note; IQ],
    n: usize,
}
static INQ: crate::sync::Mutex<In> = crate::sync::Mutex::new(In { q: [Note::EMPTY; IQ], n: 0 });
static DROPPED: AtomicU32 = AtomicU32::new(0);

fn enqueue(n: Note) -> bool {
    let Some(mut g) = INQ.try_lock() else {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    if g.n >= IQ {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let i = g.n;
    g.q[i] = n;
    g.n += 1;
    true
}

/// Post a notification (QUEUE ONLY). `app` names the icon (an APPRES key; unknown → the generic icon), `label` +
/// `act` + `arg` the optional action button ([`ACT_NONE`] = none). `false` when contended or full (counted).
pub fn post_full(app: &[u8], title: &[u8], line: &[u8], label: &[u8], act: u8, arg: &[u8]) -> bool {
    enqueue(Note::make(app, title, line, label, act, arg))
}

/// Post into the Center only (no card): the event already has its own surface on the glass (a dialog).
pub fn post_quiet(app: &[u8], title: &[u8], line: &[u8], label: &[u8], act: u8, arg: &[u8]) -> bool {
    let mut n = Note::make(app, title, line, label, act, arg);
    n.quiet = true;
    enqueue(n)
}

// ── the state (the window-safe pass) ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Card {
    n: Note,
    win: wm::WinId,
    until: u64,
    surf: usize,
}

struct St {
    /// `cards[0]` is the newest (top).
    cards: [Option<Card>; STACK_MAX],
    ring: Vec<Note>,
    center: wm::WinId,
    seq: u32,
}

static ST: crate::sync::Mutex<St> = crate::sync::Mutex::new(St { cards: [None; STACK_MAX], ring: Vec::new(), center: wm::WIN_NONE, seq: 0 });
static HEADLESS: AtomicBool = AtomicBool::new(false);
static DND: AtomicBool = AtomicBool::new(false);
static DND_LOADED: AtomicBool = AtomicBool::new(false);
static DND_SAVE_OWED: AtomicBool = AtomicBool::new(false);
static UNREAD: AtomicU32 = AtomicU32::new(0);
static CENTER_OPEN: AtomicBool = AtomicBool::new(false);
static POSTED: AtomicU32 = AtomicU32::new(0);
static FOCUS_MOVED: AtomicU32 = AtomicU32::new(0);
/// The fixture's bulk posts keep the wire quiet (one summary line, not a hundred).
static MUTE: AtomicBool = AtomicBool::new(false);
fn wire() -> bool {
    !MUTE.load(Ordering::Relaxed)
}
/// The panel size the cards were laid out on (`w << 32 | h`), for the press router.
static PANEL: AtomicU64 = AtomicU64::new(0);
/// Per stack slot: the card's seq (0 = empty slot), for the press router.
static SLOT_SEQ: [AtomicU32; STACK_MAX] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
/// Per stack slot: the card has a button.
static SLOT_BTN: [AtomicBool; STACK_MAX] = [AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false)];
/// Requests the press router records for [`service`].
static REQ_DISMISS: AtomicU32 = AtomicU32::new(0);
static REQ_ACT: AtomicU32 = AtomicU32::new(0);
static REQ_CENTER: AtomicU32 = AtomicU32::new(0); // 1 toggle (bell), 2 close (outside), 3 clear
/// The ring changed while the Center is open (repaint it on the next pass).
static CENTER_STALE: AtomicBool = AtomicBool::new(false);

const SURF_N: usize = STACK_MAX + 1; // + the Center
static SURF_AT: [AtomicUsize; SURF_N] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

fn surf(i: usize) -> &'static mut [u32] {
    let (w, h) = if i == STACK_MAX { (CW, CH) } else { (W, H) };
    let n = metrics::size(w) * metrics::size(h);
    let mut p = SURF_AT[i].load(Ordering::Acquire);
    if p == 0 {
        let b: &'static mut [u32] = alloc::boxed::Box::leak(alloc::vec![0u32; n].into_boxed_slice());
        p = match SURF_AT[i].compare_exchange(0, b.as_mut_ptr() as usize, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => b.as_mut_ptr() as usize,
            Err(won) => won,
        };
    }
    // SAFETY: one leaked buffer of `n` words per slot (sized from the scale at first use, as the toast's), painted
    // on the window-safe pass, read by `wm`'s composite while the row lives.
    unsafe { core::slice::from_raw_parts_mut(p as *mut u32, n) }
}

/// The unread count the bell's badge shows (lock-free: the bar's painter reads it).
pub fn unread() -> u32 {
    UNREAD.load(Ordering::Relaxed)
}

/// The bar's signature term: the badge and the Center's open state.
pub fn bar_sig() -> u64 {
    unread() as u64 | (CENTER_OPEN.load(Ordering::Relaxed) as u64) << 32
}

/// The Center is open (the bell draws lit).
pub fn center_open() -> bool {
    CENTER_OPEN.load(Ordering::Relaxed)
}

/// Do Not Disturb is on.
pub fn dnd() -> bool {
    DND.load(Ordering::Relaxed)
}

/// Settings' toggle: set DND now; `save` latches ONE store write for the service pass.
pub fn set_dnd(on: bool, save: bool) {
    DND.store(on, Ordering::Relaxed);
    DND_LOADED.store(true, Ordering::Relaxed);
    if save {
        DND_SAVE_OWED.store(true, Ordering::Release);
    }
    serial_println!("[notify] dnd={} via={}", on as u8, if save { "settings" } else { "prefs" });
}

/// Cards on the glass (the toast's `showing`).
pub fn banners() -> usize {
    ST.lock().cards.iter().filter(|c| c.is_some()).count()
}

fn panel() -> (usize, usize) {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        let i = super::WRITER.lock().info();
        return (i.width, i.height);
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        (0, 0)
    }
}

/// Slot `i`'s card rect on the panel (physical px).
fn card_rect(pw: usize, i: usize) -> (usize, usize, usize, usize) {
    let (sw, sh) = (metrics::size(W), metrics::size(H));
    let x = pw.saturating_sub(sw + metrics::size(12));
    let y = wm::TITLE_H() + metrics::size(GAP) + i * (sh + metrics::size(GAP));
    (x, y, sw, sh)
}

fn btn_rect() -> (usize, usize, usize, usize) {
    (W - 12 - BTN_W, (H - BTN_H) / 2, BTN_W, BTN_H)
}

fn center_rect(pw: usize) -> (usize, usize, usize, usize) {
    let (sw, sh) = (metrics::size(CW), metrics::size(CH));
    (pw.saturating_sub(sw + metrics::size(8)), wm::TITLE_H() + metrics::size(4), sw, sh)
}

fn clear_rect() -> (usize, usize, usize, usize) {
    (CW - 12 - 64, (HEAD_H - BTN_H) / 2, 64, BTN_H)
}

fn inside(px: usize, py: usize, r: (usize, usize, usize, usize)) -> bool {
    px >= r.0 && py >= r.1 && px < r.0 + r.2 && py < r.1 + r.3
}

// ── painting ─────────────────────────────────────────────────────────────────────────────────────────────

fn frame(px: &mut [u32], pw: usize, w: usize, h: usize) {
    metrics::fill(px, pw, 0, 0, w, h, theme::chrome_face());
    for (x, y, fw, fh) in [(0, 0, w, 1), (0, h - 1, w, 1), (0, 0, 1, h), (w - 1, 0, 1, h)] {
        metrics::fill(px, pw, x, y, fw, fh, theme::frame_line());
    }
}

fn icon(px: &mut [u32], pw: usize, ph: usize, x: usize, y: usize, size: usize, app: &[u8]) {
    let key = if app.is_empty() { "generic" } else { s(app) };
    if !crate::fs::appres::blit_key_icon(px, pw, ph, metrics::size(x), metrics::size(y), metrics::size(size), key) {
        metrics::fill(px, pw, x, y, size, size, theme::accent());
    }
}

fn button(px: &mut [u32], pw: usize, ph: usize, clip: usize, r: (usize, usize, usize, usize), t: &[u8]) {
    let face = super::text::Face::Ui;
    metrics::fill(px, pw, r.0, r.1, r.2, r.3, theme::button_face());
    for (x, y, w, h) in [(r.0, r.1, r.2, 1), (r.0, r.1 + r.3 - 1, r.2, 1), (r.0, r.1, 1, r.3), (r.0 + r.2 - 1, r.1, 1, r.3)] {
        metrics::fill(px, pw, x, y, w, h, theme::frame_line());
    }
    let ty = r.1 + r.3.saturating_sub(metrics::lcell_h(face)) / 2;
    let _ = metrics::text(px, pw, ph, clip, r.0 + 8, ty, t, theme::content_text(), false, face);
}

fn paint_card(i: usize, n: &Note) {
    let px = surf(i);
    let (pw, ph) = (metrics::size(W), metrics::size(H));
    frame(px, pw, W, H);
    icon(px, pw, ph, 12, (H - ICON) / 2, ICON, n.app());
    let face = super::text::Face::Ui;
    let clip = if n.has_button() { btn_rect().0 - 6 } else { W - 8 };
    let _ = metrics::text(px, pw, ph, clip, 62, 10, n.title(), theme::content_text(), true, face);
    let _ = metrics::text(px, pw, ph, clip, 62, 34, n.line(), theme::title_text_active(), false, face);
    if n.has_button() {
        button(px, pw, ph, W - 4, btn_rect(), n.label());
    }
}

/// The Center's model: the ring grouped by app, groups ordered by their newest notification, each newest first.
/// Returns `(items, apps)` and the group order (indices into `ring`, a group header precedes its items).
fn center_model(ring: &[Note]) -> (usize, usize, Vec<(bool, usize)>) {
    let mut apps: Vec<&[u8]> = Vec::new();
    for n in ring.iter().rev() {
        if !apps.iter().any(|a| *a == n.app()) {
            apps.push(n.app());
        }
    }
    let mut rows = Vec::new();
    for a in apps.iter() {
        let mut first = true;
        for (k, n) in ring.iter().enumerate().rev() {
            if n.app() == *a {
                if first {
                    rows.push((true, k));
                    first = false;
                }
                rows.push((false, k));
            }
        }
    }
    (ring.len(), apps.len(), rows)
}

fn paint_center(ring: &[Note]) -> (usize, usize) {
    let px = surf(STACK_MAX);
    let (pw, ph) = (metrics::size(CW), metrics::size(CH));
    frame(px, pw, CW, CH);
    let face = super::text::Face::Ui;
    let ch = metrics::lcell_h(face);
    let _ = metrics::text(px, pw, ph, CW, 12, (HEAD_H - ch) / 2, b"Notifications", theme::content_text(), true, face);
    button(px, pw, ph, CW - 4, clear_rect(), b"Clear");
    metrics::fill(px, pw, 0, HEAD_H, CW, 1, theme::frame_line());
    let (items, apps, rows) = center_model(ring);
    if items == 0 {
        let _ = metrics::text(px, pw, ph, CW, 12, HEAD_H + 16, b"No Notifications", theme::title_text_inactive(), false, face);
        return (0, 0);
    }
    let mut y = HEAD_H + 4;
    for (head, k) in rows {
        let n = &ring[k];
        let need = if head { GROUP_H } else { ITEM_H };
        if y + need > CH - 4 {
            break;
        }
        if head {
            let _ = metrics::text(px, pw, ph, CW, 12, y + (GROUP_H - ch) / 2, n.app(), theme::title_text_inactive(), true, face);
        } else {
            icon(px, pw, ph, 14, y + 6, 28, n.app());
            let _ = metrics::text(px, pw, ph, CW - 8, 52, y + 2, n.title(), theme::content_text(), true, face);
            let _ = metrics::text(px, pw, ph, CW - 8, 52, y + 2 + ch, n.line(), theme::title_text_active(), false, face);
        }
        y += need;
    }
    (items, apps)
}

// ── opening and closing rows ─────────────────────────────────────────────────────────────────────────────

fn open_row(i: usize, x: usize, y: usize) -> wm::WinId {
    if HEADLESS.load(Ordering::Relaxed) {
        return wm::WIN_NONE;
    }
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        let (w, h) = if i == STACK_MAX { (CW, CH) } else { (W, H) };
        let (sw, sh) = (metrics::size(w), metrics::size(h));
        return wm::overlay_open(surf(i).as_mut_ptr() as usize, sw * sh * 4, sw, sh, x, y);
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        let _ = (i, x, y);
        wm::WIN_NONE // aarch64: wire-only (no chromeless overlay row there yet), the toast's rule
    }
}

fn free_surf(st: &St) -> usize {
    (0..STACK_MAX).find(|k| !st.cards.iter().flatten().any(|c| c.surf == *k)).unwrap_or(0)
}

/// Re-seat every card at its slot (newest on top) and publish the slots for the press router.
fn restack(st: &mut St, pw: usize) {
    let mut packed: [Option<Card>; STACK_MAX] = [None; STACK_MAX];
    let mut k = 0;
    for c in st.cards.iter().flatten() {
        packed[k] = Some(*c);
        k += 1;
    }
    st.cards = packed;
    for i in 0..STACK_MAX {
        match st.cards[i] {
            Some(c) => {
                if c.win != wm::WIN_NONE {
                    let (x, y, _, _) = card_rect(pw, i);
                    wm::move_to(c.win, x, y);
                }
                SLOT_SEQ[i].store(c.n.seq, Ordering::Release);
                SLOT_BTN[i].store(c.n.has_button(), Ordering::Release);
            }
            None => {
                SLOT_SEQ[i].store(0, Ordering::Release);
                SLOT_BTN[i].store(false, Ordering::Release);
            }
        }
    }
}

fn close_card(st: &mut St, i: usize, by: &str) -> Option<Note> {
    let c = st.cards[i].take()?;
    if c.win != wm::WIN_NONE {
        wm::close(c.win);
    }
    if wire() {
        serial_println!("[notify] closed by={} title={}", by, s(c.n.title()));
    }
    Some(c.n)
}

fn show_card(st: &mut St, n: Note, pw: usize) {
    if st.cards[STACK_MAX - 1].is_some() {
        let _ = close_card(st, STACK_MAX - 1, "overflow");
        restack(st, pw);
    }
    let sf = free_surf(st);
    paint_card(sf, &n);
    // newest on top: shift the stack down one slot, then open the new card at slot 0
    for i in (1..STACK_MAX).rev() {
        st.cards[i] = st.cards[i - 1];
    }
    st.cards[0] = None;
    let f0 = wm::focus_asid();
    let (x, y, _, _) = card_rect(pw, 0);
    let win = open_row(sf, x, y);
    st.cards[0] = Some(Card { n, win, until: crate::arch::ms().saturating_add(CARD_MS).max(1), surf: sf });
    restack(st, pw);
    let kept = wm::focus_asid() == f0;
    if !kept {
        FOCUS_MOVED.fetch_add(1, Ordering::Relaxed);
    }
    if wire() { serial_println!("[notify] show win={} slot=0 app={} title={} line={} ms={} focus={}", win, s(n.app()), s(n.title()), s(n.line()), CARD_MS, if kept { "kept" } else { "MOVED" }); }
}

fn center_close(st: &mut St, by: &str) {
    if !CENTER_OPEN.swap(false, Ordering::AcqRel) {
        return;
    }
    let w = core::mem::replace(&mut st.center, wm::WIN_NONE);
    if w != wm::WIN_NONE {
        wm::close(w);
    }
    serial_println!("[notify] center closed by={}", by);
}

fn center_show(st: &mut St, pw: usize) -> (usize, usize) {
    let w = core::mem::replace(&mut st.center, wm::WIN_NONE);
    if w != wm::WIN_NONE {
        wm::close(w);
    }
    let (items, apps) = paint_center(&st.ring);
    let (x, y, _, _) = center_rect(pw);
    st.center = open_row(STACK_MAX, x, y);
    CENTER_OPEN.store(true, Ordering::Release);
    UNREAD.store(0, Ordering::Relaxed);
    (items, apps)
}

fn run_action(n: &Note) {
    let what: &str = match n.act {
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        ACT_SHOW_LOG => {
            super::panicscreen::show_log();
            "show-log"
        }
        ACT_OPEN_DIR => {
            #[cfg(feature = "quarry")]
            let ok = super::quarry::live::open_at(s(n.arg()));
            #[cfg(not(feature = "quarry"))]
            let ok = false;
            if ok { "quarry-opened" } else { "quarry-declined" }
        }
        ACT_ANSWER => "answer-owed(dialog2-bus-fold)",
        _ => "none",
    };
    serial_println!("[notify] action {} app={} title={} -> {}", s(n.label()), s(n.app()), s(n.title()), what);
}

// ── the press router (records only) ──────────────────────────────────────────────────────────────────────

/// `strip::press_route`'s NOTIFY arm, ahead of the menus: the bell toggles the Center; a press in the open
/// Center is its own (Clear, else swallowed), a press outside it closes it (spent); a press on a card runs its
/// button or dismisses it. Atomics only — [`service`] acts on the next pass.
pub fn press_at(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 {
        return false;
    }
    let (px, py) = (x as usize, y as usize);
    let pk = PANEL.load(Ordering::Relaxed);
    let (pw, ph) = ((pk >> 32) as usize, (pk & 0xffff_ffff) as usize);
    if pw == 0 {
        return false;
    }
    if let Some(b) = super::menubar::bell_box_abs(pw, ph) {
        if inside(px, py, b) {
            REQ_CENTER.store(1, Ordering::Release);
            return true;
        }
    }
    if CENTER_OPEN.load(Ordering::Relaxed) {
        let r = center_rect(pw);
        if inside(px, py, r) {
            let (lx, ly) = (metrics::to_logical(px - r.0), metrics::to_logical(py - r.1));
            if inside(lx, ly, clear_rect()) {
                REQ_CENTER.store(3, Ordering::Release);
            }
            return true;
        }
        REQ_CENTER.store(2, Ordering::Release);
        return true;
    }
    for i in 0..STACK_MAX {
        let seq = SLOT_SEQ[i].load(Ordering::Acquire);
        if seq == 0 {
            continue;
        }
        let r = card_rect(pw, i);
        if inside(px, py, r) {
            let (lx, ly) = (metrics::to_logical(px - r.0), metrics::to_logical(py - r.1));
            if SLOT_BTN[i].load(Ordering::Acquire) && inside(lx, ly, btn_rect()) {
                REQ_ACT.store(seq, Ordering::Release);
            } else {
                REQ_DISMISS.store(seq, Ordering::Release);
            }
            return true;
        }
    }
    false
}

// ── the service (the storage pass, beside the dialog's) ──────────────────────────────────────────────────

fn take_inbound(out: &mut [Note; IQ + 4]) -> usize {
    let mut k = 0;
    while k < 4 {
        let Some((title, line)) = super::toast::take() else { break };
        let app = crate::fs::appres::key_of_title(&title).unwrap_or_else(|| String::from("system"));
        out[k] = Note::make(app.as_bytes(), &title, &line, b"", ACT_NONE, b"");
        k += 1;
    }
    if let Some(mut g) = INQ.try_lock() {
        let n = g.n;
        for i in 0..n {
            out[k] = g.q[i];
            k += 1;
        }
        g.n = 0;
    }
    k
}

fn dnd_load_service() {
    if DND_LOADED.load(Ordering::Relaxed) || HEADLESS.load(Ordering::Relaxed) {
        return;
    }
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        if crate::fs::users::whoami(&mut nm).is_none() {
            return;
        }
        let on = crate::prefs_client::sys_flag(KEY_DND).unwrap_or(false);
        DND_LOADED.store(true, Ordering::Relaxed);
        DND.store(on, Ordering::Relaxed);
        serial_println!("[notify] dnd={} via=login", on as u8);
    }
}

/// One pass: take what was posted, show or collect it, act on the presses, expire, pause under the pointer.
pub fn service() {
    static REG: AtomicBool = AtomicBool::new(false);
    if !REG.swap(true, Ordering::Relaxed) {
        crate::tests::register("notify", test);
    }
    dnd_load_service();
    if DND_SAVE_OWED.swap(false, Ordering::AcqRel) {
        let r = crate::prefs_client::pref_set(crate::prefs::NS, KEY_DND, crate::prefs::PrefValue::Bool(dnd()));
        serial_println!("[notify] prefs dnd={} saved={}", dnd() as u8, if r.is_ok() { "ok" } else { "-1" });
    }
    pass(crate::arch::ms());
}

fn pass(now: u64) {
    let headless = HEADLESS.load(Ordering::Relaxed);
    let (pw, ph) = if headless { (1920, 1200) } else { panel() };
    PANEL.store((pw as u64) << 32 | ph as u64, Ordering::Relaxed);
    let mut inb = [Note::EMPTY; IQ + 4];
    let k = take_inbound(&mut inb);
    let creq = REQ_CENTER.swap(0, Ordering::AcqRel);
    let dis = REQ_DISMISS.swap(0, Ordering::AcqRel);
    let act = REQ_ACT.swap(0, Ordering::AcqRel);
    let busy = k > 0 || creq != 0 || dis != 0 || act != 0 || CENTER_STALE.load(Ordering::Relaxed) || ST.try_lock().map(|g| g.cards.iter().any(|c| c.is_some())).unwrap_or(true);
    if !busy {
        return;
    }
    let Some(mut st) = ST.try_lock() else { return };
    let unread0 = UNREAD.load(Ordering::Relaxed);
    for n in inb[..k].iter_mut() {
        st.seq = st.seq.wrapping_add(1).max(1);
        n.seq = st.seq;
        n.at = now;
        if st.ring.len() >= RING {
            st.ring.remove(0);
        }
        st.ring.push(*n);
        if !headless {
            POSTED.fetch_add(1, Ordering::Relaxed);
        }
        let collect = n.quiet || dnd() || CENTER_OPEN.load(Ordering::Relaxed);
        if !CENTER_OPEN.load(Ordering::Relaxed) {
            UNREAD.fetch_add(1, Ordering::Relaxed);
        } else {
            CENTER_STALE.store(true, Ordering::Relaxed);
        }
        if wire() { serial_println!("[notify] post app={} title={} dnd={} -> {}", s(n.app()), s(n.title()), dnd() as u8, if collect { "collected" } else { "banner" }); }
        if !collect {
            show_card(&mut st, *n, pw);
        }
    }
    // the press router's requests
    if act != 0 || dis != 0 {
        for i in 0..STACK_MAX {
            let Some(c) = st.cards[i] else { continue };
            if c.n.seq == act {
                let n = close_card(&mut st, i, "action");
                restack(&mut st, pw);
                if let Some(n) = n {
                    run_action(&n);
                }
                break;
            }
            if c.n.seq == dis {
                let _ = close_card(&mut st, i, "click");
                restack(&mut st, pw);
                break;
            }
        }
    }
    match creq {
        1 if CENTER_OPEN.load(Ordering::Relaxed) => center_close(&mut st, "bell"),
        1 => {
            let (items, apps) = center_show(&mut st, pw);
            serial_println!("[notify] center open items={} apps={} unread=0", items, apps);
        }
        2 => center_close(&mut st, "outside"),
        3 => {
            let n = st.ring.len();
            st.ring.clear();
            UNREAD.store(0, Ordering::Relaxed);
            serial_println!("[notify] clear n={}", n);
            let _ = center_show(&mut st, pw);
        }
        _ => {}
    }
    if CENTER_STALE.swap(false, Ordering::Relaxed) && CENTER_OPEN.load(Ordering::Relaxed) {
        let _ = center_show(&mut st, pw);
    }
    // hover pauses; time closes
    let ptr = if headless {
        None
    } else {
        #[cfg(all(target_arch = "x86_64", feature = "wc"))]
        {
            let (x, y) = crate::pal::cursor::pos(pw as i32, ph as i32);
            Some((x.max(0) as usize, y.max(0) as usize))
        }
        #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
        {
            None::<(usize, usize)>
        }
    };
    let mut closed = false;
    for i in 0..STACK_MAX {
        let Some(c) = st.cards[i].as_mut() else { continue };
        if ptr.map(|(x, y)| inside(x, y, card_rect(pw, i))).unwrap_or(false) {
            c.until = c.until.max(now.saturating_add(HOVER_KEEP_MS));
            continue;
        }
        if now >= c.until {
            let _ = close_card(&mut st, i, "timeout");
            closed = true;
        }
    }
    if closed {
        restack(&mut st, pw);
    }
    drop(st);
    if UNREAD.load(Ordering::Relaxed) != unread0 && !headless {
        wm::composite(); // the bell's badge is bar chrome: a collected notification still repaints the bar
    }
}

// ── `tests notify` (model-only: headless, the real post / service / press code) ──────────────────────────

type Held = ([Option<Card>; STACK_MAX], Vec<Note>, wm::WinId, u32, bool, bool, bool, [u32; STACK_MAX]);

fn hold() -> Held {
    let mut g = ST.lock();
    let cards = core::mem::replace(&mut g.cards, [None; STACK_MAX]);
    let ring = core::mem::take(&mut g.ring);
    let center = core::mem::replace(&mut g.center, wm::WIN_NONE);
    drop(g);
    let mut seqs = [0u32; STACK_MAX];
    for (i, s) in SLOT_SEQ.iter().enumerate() {
        seqs[i] = s.swap(0, Ordering::AcqRel);
    }
    (cards, ring, center, UNREAD.swap(0, Ordering::AcqRel), CENTER_OPEN.swap(false, Ordering::AcqRel), DND.load(Ordering::Relaxed), HEADLESS.swap(true, Ordering::AcqRel), seqs)
}

fn restore(h: Held) {
    {
        let mut g = ST.lock();
        g.cards = h.0;
        g.ring = h.1;
        g.center = h.2;
    }
    UNREAD.store(h.3, Ordering::Relaxed);
    CENTER_OPEN.store(h.4, Ordering::Relaxed);
    DND.store(h.5, Ordering::Relaxed);
    HEADLESS.store(h.6, Ordering::Relaxed);
    for (i, s) in SLOT_SEQ.iter().enumerate() {
        s.store(h.7[i], Ordering::Relaxed);
    }
    REQ_CENTER.store(0, Ordering::Relaxed);
    REQ_DISMISS.store(0, Ordering::Relaxed);
    REQ_ACT.store(0, Ordering::Relaxed);
    CENTER_STALE.store(false, Ordering::Relaxed);
}

fn top_title_is(t: &[u8]) -> bool {
    ST.lock().cards[0].map(|c| c.n.title() == t).unwrap_or(false)
}

/// `tests notify`.
pub fn test() {
    let dnd_real = dnd();
    let h = hold();
    DND.store(false, Ordering::Relaxed);
    let t0 = 1_000_000u64;
    // (1) the stack: six posts, four cards, newest on top, the toast's front included
    let mut posted_ok = true;
    for k in 0..5u8 {
        posted_ok &= post_full(b"quarry", &[b'N', b'0' + k], b"fixture line", b"", ACT_NONE, b"");
    }
    posted_ok &= super::toast::post(b"Screenshot saved", b"fixture toast");
    pass(t0);
    let stack = banners();
    let stack_ok = stack == STACK_MAX && top_title_is(b"Screenshot saved");
    // a press on the top card's body dismisses it; on a button runs the action
    let pk = PANEL.load(Ordering::Relaxed);
    let pw = (pk >> 32) as usize;
    let r0 = card_rect(pw, 0);
    let press_ok = press_at((r0.0 + 4) as i32, (r0.1 + 4) as i32) && { pass(t0 + 1); banners() == STACK_MAX - 1 };
    let _ = post_full(b"usbstor", b"Volume mounted", b"FIXTURE is in Volumes", b"Open", ACT_ANSWER, b"");
    pass(t0 + 2);
    let r0 = card_rect(pw, 0);
    let b = btn_rect();
    let bx = r0.0 + metrics::size(b.0 + 4);
    let by = r0.1 + metrics::size(b.1 + 4);
    let action_ok = SLOT_BTN[0].load(Ordering::Acquire) && press_at(bx as i32, by as i32) && REQ_ACT.load(Ordering::Acquire) != 0 && { pass(t0 + 3); !top_title_is(b"Volume mounted") };
    // hover-free expiry
    pass(t0 + 3 + CARD_MS + 1);
    let expire_ok = banners() == 0;
    // (2) the ring: 105 posts keep the last 100 (the wire muted for the bulk)
    MUTE.store(true, Ordering::Relaxed);
    for k in 0..105u32 {
        let d = [b'0' + (k / 100) as u8, b'0' + (k / 10 % 10) as u8, b'0' + (k % 10) as u8];
        let _ = post_full(if k % 2 == 0 { b"quarry" } else { b"settings" }, b"ring", &d, b"", ACT_NONE, b"");
        if k % 4 == 3 || k == 104 {
            pass(t0 + 10 + k as u64);
        }
    }
    MUTE.store(false, Ordering::Relaxed);
    let ring_n = ST.lock().ring.len();
    let ring_ok = ring_n == RING && ST.lock().ring.last().map(|n| n.line() == b"104").unwrap_or(false);
    let badge_before = unread();
    // (3) the Center: the bell opens it (badge clears), grouped by app, Clear empties it, a press outside closes it
    REQ_CENTER.store(1, Ordering::Release);
    pass(t0 + 200);
    let (items, apps, rows) = { let g = ST.lock(); center_model(&g.ring) };
    let first_newest = ST.lock().ring.get(rows.get(1).map(|r| r.1).unwrap_or(0)).map(|n| n.line() == b"104").unwrap_or(false);
    let center_ok_a = center_open() && unread() == 0 && items == RING && apps >= 2 && first_newest;
    let cr = center_rect(pw);
    let c = clear_rect();
    let clear_press = press_at((cr.0 + metrics::size(c.0 + 4)) as i32, (cr.1 + metrics::size(c.1 + 4)) as i32);
    pass(t0 + 201);
    let cleared = ST.lock().ring.is_empty() && center_open();
    let outside = press_at(4, (cr.1 + cr.3 + 40) as i32);
    pass(t0 + 202);
    let center_ok = center_ok_a && clear_press && cleared && outside && !center_open();
    let badge_ok = badge_before >= RING as u32 && unread() == 0;
    // (4) DND: a post collects silently (no card, the badge counts it)
    DND.store(true, Ordering::Relaxed);
    let _ = post_full(b"system", b"DND fixture", b"collected", b"", ACT_NONE, b"");
    pass(t0 + 300);
    let dnd_ok = banners() == 0 && unread() == 1 && ST.lock().ring.len() == 1;
    let kept = FOCUS_MOVED.load(Ordering::Relaxed) == 0;
    restore(h);
    let posted = POSTED.load(Ordering::Relaxed);
    let ok = posted_ok && stack_ok && press_ok && action_ok && expire_ok && ring_ok && center_ok && badge_ok && dnd_ok && kept;
    serial_println!(
        ":: NOTIFY: stack_max={} center={} ring={} badge={} dnd={} posted={} -> {} :: press={} action={} expire={} dnd_collect={} focus={} dropped={}",
        if stack_ok { STACK_MAX } else { stack }, if center_ok { "ok" } else { "FAIL" }, ring_n, if badge_ok { "ok" } else { "FAIL" },
        dnd_real as u8, posted, if ok { "PASS" } else { "FAIL" },
        if press_ok { "ok" } else { "FAIL" }, if action_ok { "ok" } else { "FAIL" }, if expire_ok { "ok" } else { "FAIL" },
        if dnd_ok { "ok" } else { "FAIL" }, if kept { "kept" } else { "MOVED" }, DROPPED.load(Ordering::Relaxed)
    );
}

/// `tests notice`'s toast leg (DIALOG M4), over NOTIFY: a toast posts, shows without moving focus, closes on its time.
pub fn toast_fixture() -> bool {
    let h = hold();
    let moved0 = FOCUS_MOVED.load(Ordering::Relaxed);
    let posted = super::toast::post(b"holocron", b"holocron start: ok\nsecond line dropped");
    pass(5_000);
    let up = banners() == 1 && ST.lock().cards[0].map(|c| c.n.line() == b"holocron start: ok").unwrap_or(false);
    pass(5_000 + CARD_MS + 1);
    let gone = banners() == 0;
    let kept = FOCUS_MOVED.load(Ordering::Relaxed) == moved0;
    restore(h);
    posted && up && gone && kept
}
