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
//! - **The ring** is the session's last [`RING`] notifications, in memory (nothing outlives a boot), each with its app,
//!   title, line, time and READ flag (NOTIFYCENTER B622, R116: a card that times out is not gone — the timeout marks
//!   nothing read). The bar's bell carries the ring's unread count (`menubar::bell_*` read [`unread`], the ring's
//!   mirror), and a press on it opens the Center: the ring newest first, grouped by app, `Clear` (all), a per-item
//!   `x` (dismiss), a press on an item runs its action; opening it marks the items it SHOWS read.
//! - **Do Not Disturb** is Principia's `system.notify.dnd` (R79: a preference is Principia's), toggled in Settings >
//!   Notifications (NOTIFYPANE B435; it was a General row); on — or inside its schedule — a notification
//!   collects silently (the badge counts it, no card shows, no sound).
//! - **Per-app rules and the alert sound** (NOTIFYPANE B435, `notifypane.rs`): each post reads its app's cached
//!   rule (allow / banner or center / sound) at post time; an app turned off posts nothing; one alert sound per
//!   pass through the hda tone path.
//!
//! Wire: `[notify] post app=<a> title=<t> dnd=<0|1> -> banner|collected`, `[notify] show win=<n> slot=<i> title=<t>
//! ms=<ms> focus=kept`, `[notify] closed by=<timeout|click|overflow> title=<t>`, `[notify] action <label> -> <what>`,
//! `[notify] bell press at=(<x>,<y>) box=(<x>,<y>,<w>,<h>) -> center-open|center-close|miss`, `[notify] center open
//! items=<n> unread=<n> apps=<n> shown=<n> by=<bell|clock>` / `[notify] center close by=<bell|outside|action|clock>`
//! (SOFTY3 B651: under Softy the taskbar's clock flyout opens and closes the Center with itself, `request_center`), `[notify] center
//! dismiss title=<t> left=<n>`, and `tests notify` → `:: NOTIFY: stack_max=3 center=ok ring=64 badge=ok dnd=<0|1>
//! posted=<n> center_kept=2 unread=2 -> PASS :: …`.
//! Design: `docs/dev/evidence/rmbp-1005/notify.md`; the pane and the sound: `notifypane.md`.
//!
//! TOASTCAP (rmbp-ledger B674; DESIGN-LANGUAGE §3.6, BP-bar-18, BP-video-16): at most [`STACK_MAX`] = 3 cards on the
//! glass, each [`CARD_MS`] = 5 s, its clock PAUSED while the pointer is over it (the time left is frozen, at least
//! `HOVER_KEEP_MS` once it leaves); a post that finds the stack full WAITS in `St::wait` (FIFO) and shows when a
//! slot frees — no card is closed to make room (R116: and every post is already in the ring the Center lists).
//! Wire: `[notify] queued title=<t> waiting=<n>`; `tests toastcap` → `:: TOASTCAP: posted=12 shown_max=3 queued=9
//! dropped=0 hover_pause=ok dialogs=<n> primary_max=1 return=ok esc=ok -> PASS ::` (the dialog half is
//! `dialog::toastcap_leg`).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::{metrics, theme, wm};

/// Cards visible at once (TOASTCAP B674: DESIGN-LANGUAGE §3.6 "at most 3 stacked", BP-bar-18).
pub const STACK_MAX: usize = 3;
/// The session ring the Center lists (NOTIFYCENTER B622: N=64).
pub const RING: usize = 64;
/// How long a card stays (TOASTCAP B674: DESIGN-LANGUAGE §3.6 "auto-dismiss after 5 s"; was the toast's 3 s).
pub const CARD_MS: u64 = 5000;
/// A hovered card keeps at least this much time once the pointer leaves.
const HOVER_KEEP_MS: u64 = 1500;
/// TOASTCAP (B674): the cards waiting for a slot are bounded by the ring (the Center keeps every one of them).
const WAIT_MAX: usize = RING;

/// No action (a press dismisses).
pub const ACT_NONE: u8 = 0;
/// PANICSCREEN's `Show log` (`panicscreen::show_log`).
pub const ACT_SHOW_LOG: u8 = 1;
/// Open Quarry at the path in `arg` (a mounted volume).
pub const ACT_OPEN_DIR: u8 = 2;
/// A program's own action: `arg` = owner (8 bytes LE) + token (4 bytes LE) + button (1); answered on the dialog bus path.
pub const ACT_ANSWER: u8 = 3;
/// NOTIFYCENTER (B622): open the file at the path in `arg` in the image viewer (`facet::request_open`).
pub const ACT_OPEN_FILE: u8 = 4; pub const ACT_UNDO: u8 = 5; // UNDOMOVE (B716): Quarry's undo of the journal entry whose seq is `arg` (8 bytes LE), `quarry::undo::from_toast`. Same-line fold: notify.rs is in every furniture image.

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
/// NOTIFYCENTER (B622): an item row's dismiss box (logical, inside the row) and its age column's left edge.
const DX_W: usize = 16;
const DX_X: usize = CW - 12 - DX_W;
const AGE_X: usize = DX_X - 44;

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
    /// NOTIFYCENTER (B622): seen in the open Center (a card's timeout never sets it).
    read: bool,
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
    const EMPTY: Note = Note { app: [0; AL], al: 0, title: [0; TL], tl: 0, line: [0; LL], ll: 0, label: [0; BL], bl: 0, act: 0, arg: [0; XL], xl: 0, quiet: false, seq: 0, at: 0, read: false };
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

/// DISKFULL (rmbp-ledger B699, BP-storage-18): the full-volume toast — `Disk full: <volume> - <n> MB free` (ASCII: a card
/// carries printable ASCII only, so the sheet's dash is `-`) with an `Open Trash` action that opens Quarry at `trash`.
/// QUEUE ONLY like every post; the once-a-minute floor is the caller's (`fs::unafs::diskfull::after_hold`).
pub fn disk_full(vol: &str, free_mb: u64, trash: &str) -> bool {
    let title = alloc::format!("Disk full: {} - {} MB free", vol, free_mb);
    post_full(b"system", title.as_bytes(), b"Empty the Trash or delete files to make room.", b"Open Trash", ACT_OPEN_DIR, trash.as_bytes())
}

// ── the state (the window-safe pass) ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Card {
    n: Note,
    win: wm::WinId,
    until: u64,
    surf: usize,
    /// NOTICEFIX (B491): the pass clock the card was shown at (the timeout close's `after_ms`).
    at: u64,
    /// TOASTCAP (B674): the time left, frozen while the pointer is over the card (`0` = not hovered: the clock runs).
    left: u64,
}

struct St {
    /// `cards[0]` is the newest (top).
    cards: [Option<Card>; STACK_MAX],
    ring: Vec<Note>,
    center: wm::WinId,
    seq: u32,
    /// TOASTCAP (B674): posts that found the stack full, oldest first; each shows when a slot frees.
    wait: Vec<Note>,
}

static ST: crate::sync::Mutex<St> = crate::sync::Mutex::new(St { cards: [None; STACK_MAX], ring: Vec::new(), center: wm::WIN_NONE, seq: 0, wait: Vec::new() });
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
static SLOT_SEQ: [AtomicU32; STACK_MAX] = [const { AtomicU32::new(0) }; STACK_MAX];
/// Per stack slot: the card has a button.
static SLOT_BTN: [AtomicBool; STACK_MAX] = [const { AtomicBool::new(false) }; STACK_MAX];
/// TOASTCAP (B674): cards shown (every show, a promoted one included), waiting cards shed past [`WAIT_MAX`] (they stay
/// in the ring), and the fixture's pointer (`x << 32 | y`; `u64::MAX` = none) — the headless pass has no cursor.
static SHOWN: AtomicU32 = AtomicU32::new(0);
static WAIT_SHED: AtomicU32 = AtomicU32::new(0);
static FIX_PTR: AtomicU64 = AtomicU64::new(u64::MAX);
/// Requests the press router records for [`service`].
static REQ_DISMISS: AtomicU32 = AtomicU32::new(0);
static REQ_ACT: AtomicU32 = AtomicU32::new(0);
static REQ_CENTER: AtomicU32 = AtomicU32::new(0); // 1 toggle (bell), 2 close (outside), 3 clear
/// The ring changed while the Center is open (repaint it on the next pass).
static CENTER_STALE: AtomicBool = AtomicBool::new(false);
/// NOTIFYCENTER (B622): a press on an open Center item — its `x` (dismiss) or its body (its action), by seq.
static REQ_ITEM_DISMISS: AtomicU32 = AtomicU32::new(0);
static REQ_ITEM_ACT: AtomicU32 = AtomicU32::new(0);
/// The Center's painted item rows for the press router: logical top y, seq, has-action. `ROW_N` rows are live.
const ROWS_MAX: usize = CH / ITEM_H + 1;
static ROW_N: AtomicUsize = AtomicUsize::new(0);
static ROW_Y: [AtomicUsize; ROWS_MAX] = [const { AtomicUsize::new(0) }; ROWS_MAX];
static ROW_SEQ: [AtomicU32; ROWS_MAX] = [const { AtomicU32::new(0) }; ROWS_MAX];
static ROW_ACT: [AtomicBool; ROWS_MAX] = [const { AtomicBool::new(false) }; ROWS_MAX];

/// NOTIFYCENTER (B622): the badge is the RING's unread count — published after every ring change.
fn recount(ring: &[Note]) {
    UNREAD.store(ring.iter().filter(|n| !n.read).count() as u32, Ordering::Relaxed);
}

const SURF_N: usize = STACK_MAX + 1; // + the Center
static SURF_AT: [AtomicUsize; SURF_N] = [const { AtomicUsize::new(0) }; SURF_N];

fn surf(i: usize) -> &'static mut [u32] {
    let (w, h) = if i == STACK_MAX { (CW, CH) } else { (W, H) };
    let n = metrics::size(w) * metrics::size(h);
    metrics::leaked_surf(&SURF_AT[i], n) // SECREVIEW F1: grows with the live scale (was sized once, then sliced at the new scale)
}

/// The unread count the bell's badge shows (lock-free: the bar's painter reads it) — the ring's unread items.
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
    if let Some((x, y)) = softy_center_at(pw, sw, sh) {
        return (x, y, sw, sh); // SOFTY3 (B651): under Softy the Center sits above the taskbar, beside the calendar
    }
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
    metrics::fill(px, pw, 0, 0, w, h, theme::surface_window());
    for (x, y, fw, fh) in [(0, 0, w, 1), (0, h - 1, w, 1), (0, 0, 1, h), (w - 1, 0, 1, h)] {
        metrics::fill(px, pw, x, y, fw, fh, theme::line_subtle());
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
    metrics::fill(px, pw, r.0, r.1, r.2, r.3, theme::surface_raised());
    for (x, y, w, h) in [(r.0, r.1, r.2, 1), (r.0, r.1 + r.3 - 1, r.2, 1), (r.0, r.1, 1, r.3), (r.0 + r.2 - 1, r.1, 1, r.3)] {
        metrics::fill(px, pw, x, y, w, h, theme::line_subtle());
    }
    let ty = r.1 + r.3.saturating_sub(metrics::lcell_h(face)) / 2;
    let _ = metrics::text(px, pw, ph, clip, r.0 + 8, ty, t, theme::text_primary(), false, face);
}

fn paint_card(i: usize, n: &Note) {
    let px = surf(i);
    let (pw, ph) = (metrics::size(W), metrics::size(H));
    craft_card(px, pw, ph); // DESKTOPCRAFT (B631): a rounded, shadowed card over the wallpaper's VEIL (was `frame`: a flat face in a 1 px box)
    craft_icon(px, pw, ph, n.app()); // DESKTOPCRAFT (B631): the app's icon, or a rounded accent tile where the registrar has none
    let face = super::text::Face::Ui;
    let clip = if n.has_button() { btn_rect().0 - 6 } else { W - 8 };
    let _ = metrics::text(px, pw, ph, clip, 62, 10, n.title(), theme::text_primary(), true, face);
    let _ = metrics::text(px, pw, ph, clip, 62, 34, n.line(), theme::text_primary(), false, face);
    if n.has_button() {
        craft_button(px, pw, ph, btn_rect(), n.label()); // DESKTOPCRAFT (B631): the kit's push button (rounded, hairline edge)
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

fn age(now: u64, at: u64) -> String {
    let sec = now.saturating_sub(at) / 1000;
    if sec < 60 { String::from("now") } else if sec < 3600 { alloc::format!("{}m", sec / 60) } else { alloc::format!("{}h", sec / 3600) }
}

/// Paint the Center and publish its item rows for the press router. Returns `(items, apps, shown)` — `shown` the
/// ring indices of the items drawn (what opening the Center marks read).
fn paint_center(ring: &[Note], now: u64) -> (usize, usize, Vec<usize>) {
    ROW_N.store(0, Ordering::Release);
    let px = surf(STACK_MAX);
    let (pw, ph) = (metrics::size(CW), metrics::size(CH));
    frame(px, pw, CW, CH);
    let face = super::text::Face::Ui;
    let ch = metrics::lcell_h(face);
    let _ = metrics::text(px, pw, ph, CW, 12, (HEAD_H - ch) / 2, b"Notifications", theme::text_primary(), true, face);
    button(px, pw, ph, CW - 4, clear_rect(), b"Clear");
    metrics::fill(px, pw, 0, HEAD_H, CW, 1, theme::line_subtle());
    let (items, apps, rows) = center_model(ring);
    if items == 0 {
        let _ = metrics::text(px, pw, ph, CW, 12, HEAD_H + 16, b"No Notifications", theme::text_secondary(), false, face);
        return (0, 0, Vec::new());
    }
    let mut shown = Vec::new();
    let mut y = HEAD_H + 4;
    for (head, k) in rows {
        let n = &ring[k];
        let need = if head { GROUP_H } else { ITEM_H };
        if y + need > CH - 4 {
            break;
        }
        if head {
            let _ = metrics::text(px, pw, ph, CW, 12, y + (GROUP_H - ch) / 2, n.app(), theme::text_secondary(), true, face);
        } else {
            if !n.read {
                metrics::fill(px, pw, 5, y + ITEM_H / 2 - 3, 6, 6, theme::accent()); // unread dot
            }
            icon(px, pw, ph, 14, y + 6, 28, n.app());
            let _ = metrics::text(px, pw, ph, AGE_X - 4, 52, y + 2, n.title(), theme::text_primary(), true, face);
            let _ = metrics::text(px, pw, ph, AGE_X - 4, 52, y + 2 + ch, n.line(), theme::text_primary(), false, face);
            let _ = metrics::text(px, pw, ph, DX_X - 2, AGE_X, y + 2, age(now, n.at).as_bytes(), theme::text_secondary(), false, face);
            let _ = metrics::text(px, pw, ph, CW - 8, DX_X + 4, y + (ITEM_H - ch) / 2, b"x", theme::text_secondary(), true, face);
            let r = shown.len();
            if r < ROWS_MAX {
                ROW_Y[r].store(y, Ordering::Relaxed);
                ROW_SEQ[r].store(n.seq, Ordering::Relaxed);
                ROW_ACT[r].store(n.act != ACT_NONE, Ordering::Relaxed);
                ROW_N.store(r + 1, Ordering::Release);
            }
            shown.push(k);
        }
        y += need;
    }
    (items, apps, shown)
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

fn show_card(st: &mut St, n: Note, pw: usize, now: u64) {
    if st.cards[STACK_MAX - 1].is_some() || !st.wait.is_empty() {
        // TOASTCAP (B674): the stack is full (or older posts already wait) — this one waits its turn; nothing is
        // closed to make room. Past WAIT_MAX the OLDEST waiting card is shed from the wait (never from the ring).
        if st.wait.len() >= WAIT_MAX {
            let old = st.wait.remove(0);
            WAIT_SHED.fetch_add(1, Ordering::Relaxed);
            if wire() { serial_println!("[notify] closed by=overflow title={} (kept in the center)", s(old.title())); }
        }
        st.wait.push(n);
        if wire() { serial_println!("[notify] queued title={} waiting={}", s(n.title()), st.wait.len()); }
        return;
    }
    show_now(st, n, pw, now);
}

/// TOASTCAP (B674): the waiting cards take the free slots, oldest first (each lands on top: it is newer than the
/// cards it joins). Not while the Center is open — the Center lists them, as it does every post made then.
fn promote(st: &mut St, pw: usize, now: u64) {
    while st.cards[STACK_MAX - 1].is_none() && !st.wait.is_empty() && !CENTER_OPEN.load(Ordering::Relaxed) {
        let n = st.wait.remove(0);
        show_now(st, n, pw, now);
    }
}

fn show_now(st: &mut St, n: Note, pw: usize, now: u64) {
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
    st.cards[0] = Some(Card { n, win, until: now.saturating_add(CARD_MS).max(1), surf: sf, at: now, left: 0 }); // NOTICEFIX (B491): the pass's clock, never a second read of it
    SHOWN.fetch_add(1, Ordering::Relaxed);
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
    ROW_N.store(0, Ordering::Release);
    serial_println!("[notify] center close by={}", by);
}

/// Paint and open (or repaint) the Center; the items it SHOWS are marked read and the badge recounted.
/// Returns `(items, apps, shown)`.
fn center_show(st: &mut St, pw: usize, now: u64) -> (usize, usize, usize) {
    let w = core::mem::replace(&mut st.center, wm::WIN_NONE);
    if w != wm::WIN_NONE {
        wm::close(w);
    }
    let (items, apps, shown) = paint_center(&st.ring, now);
    for &k in shown.iter() {
        st.ring[k].read = true;
    }
    recount(&st.ring);
    let (x, y, _, _) = center_rect(pw);
    st.center = open_row(STACK_MAX, x, y);
    CENTER_OPEN.store(true, Ordering::Release);
    (items, apps, shown.len())
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
        ACT_ANSWER => answer_poster(n.arg()), #[cfg(feature = "quarry")] ACT_UNDO => super::quarry::undo::from_toast(n.arg()), // UNDOMOVE (B716): the Trash toast's Undo (same-line fold, code before the comment). REFUSALUI (B468): the bus poster gets INPUT_EV_DIALOG_ANSWER
        ACT_OPEN_FILE => {
            // NOTIFYCENTER (B622): the viewer's click-safe latch (drained by its own service pass).
            #[cfg(feature = "facet")]
            {
                super::facet::request_open(s(n.arg()));
                "viewer-requested"
            }
            #[cfg(not(feature = "facet"))]
            {
                "viewer-absent"
            }
        }
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
            serial_println!("[notify] bell press at=({},{}) box=({},{},{},{}) -> {}", px, py, b.0, b.1, b.2, b.3, if CENTER_OPEN.load(Ordering::Relaxed) { "center-close" } else { "center-open" }); // NOTIFYCENTER (B622)
            bell_switch(); // SMALLFIX4 (STATUSTRAY fold): a status menu that is down closes first — one press switches
            REQ_CENTER.store(1, Ordering::Release);
            return true;
        }
        if py >= b.1 && py < b.1 + b.3 && px + b.2 >= b.0 && px < b.0 + 2 * b.2 {
            // NOTIFYCENTER (B622): a bar press within one bell width of the cell — the geometry witness.
            serial_println!("[notify] bell press at=({},{}) box=({},{},{},{}) -> miss", px, py, b.0, b.1, b.2, b.3);
        }
    }
    if CENTER_OPEN.load(Ordering::Relaxed) && center_switch(pw, ph, px, py) {
        return false; // SMALLFIX4 (STATUSTRAY fold): the Center closes and the status item (the clock's menu too) opens on this press
    }
    if CENTER_OPEN.load(Ordering::Relaxed) {
        let r = center_rect(pw);
        if inside(px, py, r) {
            let (lx, ly) = (metrics::to_logical(px - r.0), metrics::to_logical(py - r.1));
            if inside(lx, ly, clear_rect()) {
                REQ_CENTER.store(3, Ordering::Release);
            } else {
                // NOTIFYCENTER (B622): an item row — its `x` dismisses it, its body runs its action (if any).
                for r in 0..ROW_N.load(Ordering::Acquire).min(ROWS_MAX) {
                    let y0 = ROW_Y[r].load(Ordering::Relaxed);
                    if ly >= y0 && ly < y0 + ITEM_H {
                        let seq = ROW_SEQ[r].load(Ordering::Relaxed);
                        if lx >= DX_X - 4 && lx < DX_X + DX_W + 4 {
                            REQ_ITEM_DISMISS.store(seq, Ordering::Release);
                        } else if ROW_ACT[r].load(Ordering::Relaxed) {
                            REQ_ITEM_ACT.store(seq, Ordering::Release);
                        }
                        break;
                    }
                }
            }
            return true;
        }
        if super::look::is_softy() && super::softy::startmenu::open_kind() == 2 {
            // SOFTY3 (B651): the Center is the CALENDAR's while that flyout is open — the press is the taskbar's
            // (inside the flyout it is the flyout's; on the clock or elsewhere it closes the calendar, which closes
            // the Center through `request_center`), never spent here.
            return false;
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
        let Some((title, line, ans)) = super::toast::take_full() else { break }; // REFUSALUI (B468): the poster's button rides along
        let app = crate::fs::appres::key_of_title(&title).unwrap_or_else(|| String::from("system"));
        out[k] = match ans { Some((label, owner, token, ix)) => Note::make(app.as_bytes(), &title, &line, &label, ACT_ANSWER, &answer_arg(owner, token, ix)), None => Note::make(app.as_bytes(), &title, &line, b"", ACT_NONE, b"") };
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
        crate::tests::register("notify", test); crate::tests::register("refusalui", super::dialog::refusalui_selftest); // REFUSALUI (B468): `tests refusalui` rides this registration
    }
    dnd_load_service();
    super::notifypane::service(); // NOTIFYPANE (B435): the rules' login load and their latched store writes
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
    let idis = REQ_ITEM_DISMISS.swap(0, Ordering::AcqRel);
    let iact = REQ_ITEM_ACT.swap(0, Ordering::AcqRel);
    let busy = k > 0 || creq != 0 || dis != 0 || act != 0 || idis != 0 || iact != 0 || CENTER_STALE.load(Ordering::Relaxed) || ST.try_lock().map(|g| g.cards.iter().any(|c| c.is_some()) || !g.wait.is_empty()).unwrap_or(true);
    if !busy {
        return;
    }
    let Some(mut st) = ST.try_lock() else { return };
    let unread0 = UNREAD.load(Ordering::Relaxed);
    let mut sound: Option<Note> = None;
    for n in inb[..k].iter_mut() {
        // NOTIFYPANE (B435): the app's cached rule and DND (manual or scheduled), read at post time.
        let dnd_now = super::notifypane::dnd_now();
        let (shown, kept, snd) = prefs_core::notify::route(&super::notifypane::rule_at_post(n.app()), n.quiet, dnd_now);
        if !kept {
            if wire() { serial_println!("[notify] post app={} title={} -> blocked(allow=0)", s(n.app()), s(n.title())); }
            continue;
        }
        if snd && sound.is_none() {
            sound = Some(*n);
        }
        st.seq = st.seq.wrapping_add(1).max(1);
        n.seq = st.seq;
        n.at = now;
        n.read = false;
        if st.ring.len() >= RING {
            st.ring.remove(0);
        }
        st.ring.push(*n);
        recount(&st.ring); // NOTIFYCENTER (B622): the badge is the ring's unread count
        if !headless {
            POSTED.fetch_add(1, Ordering::Relaxed);
        }
        let collect = !shown || CENTER_OPEN.load(Ordering::Relaxed);
        if CENTER_OPEN.load(Ordering::Relaxed) {
            CENTER_STALE.store(true, Ordering::Relaxed); // the open Center repaints it (and marks it read)
        }
        if wire() { serial_println!("[notify] post app={} title={} dnd={} -> {}", s(n.app()), s(n.title()), dnd_now as u8, if collect { "collected" } else { "banner" }); }
        if !collect {
            show_card(&mut st, *n, pw, now);
        }
    }
    if let Some(n) = sound {
        let _ = super::notifypane::sound_for(n.app(), headless); // NOTIFYPANE (B435): one alert sound per pass
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
    // NOTIFYCENTER (B622): the open Center's item presses — dismiss one, or run one's action (the Center closes)
    if idis != 0 && CENTER_OPEN.load(Ordering::Relaxed) {
        if let Some(k) = st.ring.iter().position(|n| n.seq == idis) {
            let n = st.ring.remove(k);
            recount(&st.ring);
            serial_println!("[notify] center dismiss title={} left={}", s(n.title()), st.ring.len());
            CENTER_STALE.store(true, Ordering::Relaxed);
        }
    }
    if iact != 0 && CENTER_OPEN.load(Ordering::Relaxed) {
        if let Some(k) = st.ring.iter().position(|n| n.seq == iact) {
            st.ring[k].read = true;
            recount(&st.ring);
            let n = st.ring[k];
            center_close(&mut st, "action");
            run_action(&n);
        }
    }
    match creq {
        1 if CENTER_OPEN.load(Ordering::Relaxed) => center_close(&mut st, "bell"),
        4 if CENTER_OPEN.load(Ordering::Relaxed) => {} // SOFTY3 (B651): the clock's open request on an open Center is spent
        1 | 4 => {
            let (items, apps, shown) = center_show(&mut st, pw, now);
            serial_println!("[notify] center open items={} unread={} apps={} shown={} by={}", items, unread(), apps, shown, if creq == 4 { "clock" } else { "bell" });
        }
        2 => center_close(&mut st, "outside"),
        5 => center_close(&mut st, "clock"), // SOFTY3 (B651): the calendar closed, the Center with it
        3 => {
            let n = st.ring.len();
            st.ring.clear();
            recount(&st.ring);
            serial_println!("[notify] clear n={}", n);
            let _ = center_show(&mut st, pw, now);
        }
        _ => {}
    }
    if CENTER_STALE.swap(false, Ordering::Relaxed) && CENTER_OPEN.load(Ordering::Relaxed) {
        let _ = center_show(&mut st, pw, now);
    }
    // hover pauses; time closes
    let ptr = if headless {
        let f = FIX_PTR.load(Ordering::Relaxed); // TOASTCAP (B674): the fixture's pointer
        if f == u64::MAX { None } else { Some(((f >> 32) as usize, (f & 0xffff_ffff) as usize)) }
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
    let mut hot = 0u32; // MENUSCRAFT (B642): the hovered card's seq — it shows its close button
    for i in 0..STACK_MAX {
        let Some(c) = st.cards[i].as_mut() else { continue };
        if ptr.map(|(x, y)| inside(x, y, card_rect(pw, i))).unwrap_or(false) {
            // TOASTCAP (B674): PAUSED — the time left is frozen at the hover's first pass (at least HOVER_KEEP_MS)
            // and the deadline rides `now` with it, so leaving resumes the clock where it stopped.
            if c.left == 0 {
                c.left = c.until.saturating_sub(now).max(HOVER_KEEP_MS);
            }
            c.until = now.saturating_add(c.left);
            hot = c.n.seq;
            continue;
        }
        c.left = 0;
        if now >= c.until {
            let after = now.saturating_sub(c.at);
            if wire() { serial_println!("[notify] toast close reason=timeout after_ms={}", after); } // NOTICEFIX (B491)
            let _ = close_card(&mut st, i, "timeout");
            closed = true;
        }
    }
    if closed {
        restack(&mut st, pw);
    }
    promote(&mut st, pw, now); // TOASTCAP (B674): a freed slot (time, click, action, a closed Center) takes the oldest waiting card
    craft_hover(&st, hot); // MENUSCRAFT (B642): the toast's hover x, repainted on the edge
    drop(st);
    if UNREAD.load(Ordering::Relaxed) != unread0 && !headless {
        wm::composite(); // the bell's badge is bar chrome: a collected notification still repaints the bar
    }
}

// ── `tests notify` (model-only: headless, the real post / service / press code) ──────────────────────────

type Held = ([Option<Card>; STACK_MAX], Vec<Note>, wm::WinId, u32, bool, bool, bool, [u32; STACK_MAX], Vec<Note>);

fn hold() -> Held {
    super::notifypane::bypass(true, true); // NOTIFYPANE: the fixtures never read the user's rules or schedule
    let mut g = ST.lock();
    let cards = core::mem::replace(&mut g.cards, [None; STACK_MAX]);
    let ring = core::mem::take(&mut g.ring);
    let center = core::mem::replace(&mut g.center, wm::WIN_NONE);
    let wait = core::mem::take(&mut g.wait); // TOASTCAP (B674)
    drop(g);
    let mut seqs = [0u32; STACK_MAX];
    for (i, s) in SLOT_SEQ.iter().enumerate() {
        seqs[i] = s.swap(0, Ordering::AcqRel);
    }
    (cards, ring, center, UNREAD.swap(0, Ordering::AcqRel), CENTER_OPEN.swap(false, Ordering::AcqRel), DND.load(Ordering::Relaxed), HEADLESS.swap(true, Ordering::AcqRel), seqs, wait)
}

fn restore(h: Held) {
    super::notifypane::bypass(false, false);
    {
        let mut g = ST.lock();
        g.cards = h.0;
        g.ring = h.1;
        g.center = h.2;
        g.wait = h.8; // TOASTCAP (B674)
    }
    FIX_PTR.store(u64::MAX, Ordering::Relaxed);
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
    REQ_ITEM_DISMISS.store(0, Ordering::Relaxed);
    REQ_ITEM_ACT.store(0, Ordering::Relaxed);
    ROW_N.store(0, Ordering::Relaxed);
    CENTER_STALE.store(h.4, Ordering::Relaxed); // NOTIFYCENTER (B622): a live open Center repaints (its press rows republish)
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
    // (1) the stack: three posts, three cards (TOASTCAP B674: STACK_MAX is 3 and a fourth would WAIT, `tests toastcap`),
    // newest on top, the toast's front included.
    // NOTIFYCENTER (B622): the toast posts in its OWN pass after the five. One pass drains the toast queue BEFORE the
    // inbound queue (`take_inbound`), so a same-pass toast was the OLDEST card and closed by overflow — flight 34's
    // `-> FAIL` with every printed field ok: `top` was false and `stack_max=` printed the count (4) either way.
    let mut posted_ok = true;
    for k in 0..(STACK_MAX as u8 - 1) {
        posted_ok &= post_full(b"quarry", &[b'N', b'0' + k], b"fixture line", b"", ACT_NONE, b"");
    }
    pass(t0);
    posted_ok &= super::toast::post(b"Screenshot saved", b"fixture toast");
    pass(t0);
    let stack = banners();
    let top_ok = top_title_is(b"Screenshot saved");
    let stack_ok = stack == STACK_MAX && top_ok;
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
    // (2) the ring: 105 posts keep the last RING (the wire muted for the bulk)
    MUTE.store(true, Ordering::Relaxed);
    for k in 0..105u32 {
        let d = [b'0' + (k / 100) as u8, b'0' + (k / 10 % 10) as u8, b'0' + (k % 10) as u8];
        let _ = post_full(if k % 2 == 0 { b"quarry" } else { b"settings" }, b"ring", &d, b"", ACT_NONE, b"");
        if k % 4 == 3 || k == 104 {
            pass(t0 + 10 + k as u64);
        }
    }
    MUTE.store(false, Ordering::Relaxed);
    ST.lock().wait.clear(); // TOASTCAP (B674): the bulk's waiting cards are the ring's (kept there); the legs below count cards
    let ring_n = ST.lock().ring.len();
    let ring_last = ST.lock().ring.last().map(|n| n.line() == b"104").unwrap_or(false);
    let ring_ok = ring_n == RING && ring_last;
    let badge_before = unread();
    // (3) the Center: the bell opens it (the items it SHOWS are read), grouped by app, Clear empties it, a press outside
    // closes it
    REQ_CENTER.store(1, Ordering::Release);
    pass(t0 + 200);
    let (items, apps, rows) = { let g = ST.lock(); center_model(&g.ring) };
    let first_newest = ST.lock().ring.get(rows.get(1).map(|r| r.1).unwrap_or(0)).map(|n| n.line() == b"104").unwrap_or(false);
    let read_n = ST.lock().ring.iter().filter(|n| n.read).count();
    let center_ok_a = center_open() && read_n > 0 && unread() as usize == RING - read_n && items == RING && apps >= 2 && first_newest;
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
    // UIREDS26 (B582): a card's `until` is on the PASS clock since NOTICEFIX (d0061623), so the ring leg's last four
    // cards (shown at t0+13..t0+114) still stand here; one pass past every `until` expires them first, so `banners()`
    // below measures the DND post alone. Before d0061623 `until` was `arch::ms() + CARD_MS`, below t0, and they were gone.
    let t_dnd = t0 + 300 + CARD_MS;
    pass(t_dnd);
    DND.store(true, Ordering::Relaxed);
    let _ = post_full(b"system", b"DND fixture", b"collected", b"", ACT_NONE, b"");
    pass(t_dnd + 1);
    let dnd_ok = banners() == 0 && unread() == 1 && ST.lock().ring.len() == 1;
    // (5) NOTIFYCENTER (B622, R116): two posts, their cards time out — both KEPT and unread=2; the bell's open marks
    // them read (unread=0); an item's `x` dismisses it; a press outside closes the Center.
    DND.store(false, Ordering::Relaxed);
    { let mut g = ST.lock(); g.ring.clear(); recount(&g.ring); }
    let t5 = t_dnd + 10;
    let _ = post_full(b"system", b"Kept one", b"first", b"", ACT_NONE, b"");
    let _ = post_full(b"system", b"Kept two", b"second", b"", ACT_NONE, b"");
    pass(t5);
    let shown2 = banners() == 2;
    pass(t5 + CARD_MS + 1);
    let center_kept = ST.lock().ring.len();
    let unread_kept = unread();
    let kept_ok = shown2 && banners() == 0 && center_kept == 2 && unread_kept == 2;
    REQ_CENTER.store(1, Ordering::Release);
    pass(t5 + CARD_MS + 2);
    let opened_unread = unread();
    let open_ok = center_open() && opened_unread == 0 && ROW_N.load(Ordering::Acquire) == 2;
    let cr = center_rect(pw);
    let dy = ROW_Y[0].load(Ordering::Relaxed) + ITEM_H / 2;
    let dismiss_ok = press_at((cr.0 + metrics::size(DX_X + DX_W / 2)) as i32, (cr.1 + metrics::size(dy)) as i32) && REQ_ITEM_DISMISS.load(Ordering::Acquire) != 0 && {
        pass(t5 + CARD_MS + 3);
        let g = ST.lock();
        let left_oldest = g.ring.len() == 1 && g.ring[0].title() == b"Kept one"; // the newest (row 0) went
        left_oldest
    };
    let _ = press_at(4, (cr.1 + cr.3 + 40) as i32);
    pass(t5 + CARD_MS + 4);
    let center_leg = kept_ok && open_ok && dismiss_ok && !center_open();
    let kept = FOCUS_MOVED.load(Ordering::Relaxed) == 0;
    restore(h);
    let posted = POSTED.load(Ordering::Relaxed);
    let ok = posted_ok && stack_ok && press_ok && action_ok && expire_ok && ring_ok && center_ok && badge_ok && dnd_ok && center_leg && kept;
    let w = |b: bool| if b { "ok" } else { "FAIL" };
    serial_println!(
        ":: NOTIFY: stack_max={} center={} ring={} badge={} dnd={} posted={} center_kept={} unread={} -> {} :: top={} posts={} ring_last={} press={} action={} expire={} dnd_collect={} opened_unread={} dismiss={} focus={} dropped={}",
        if stack_ok { STACK_MAX } else { stack }, w(center_ok), ring_n, w(badge_ok),
        dnd_real as u8, posted, center_kept, unread_kept, if ok { "PASS" } else { "FAIL" },
        w(top_ok), w(posted_ok), w(ring_last), w(press_ok), w(action_ok), w(expire_ok),
        w(dnd_ok), opened_unread, w(dismiss_ok), if kept { "kept" } else { "MOVED" }, DROPPED.load(Ordering::Relaxed)
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

/// `tests toastcap` (TOASTCAP, rmbp-ledger B674; DESIGN-LANGUAGE §3.6/§3.8, BP-bar-18), model-only over the REAL post /
/// pass code: twelve posts, at most three cards on the glass, nine waiting, none lost (every post in the ring, nothing
/// shed or refused); a hovered card outlives four of its times and leaves on its frozen remainder; every card shows
/// in turn. The dialog half is [`super::dialog::toastcap_leg`]: one `accent.fill` button per view, Return accepts and
/// Escape cancels on every view.
pub fn toastcap_selftest() {
    const N: usize = 12;
    let h = hold();
    DND.store(false, Ordering::Relaxed);
    MUTE.store(true, Ordering::Relaxed);
    let (d0, shed0, shown0) = (DROPPED.load(Ordering::Relaxed), WAIT_SHED.load(Ordering::Relaxed), SHOWN.load(Ordering::Relaxed));
    let t0 = 4_000_000u64;
    let mut posted = 0usize;
    let mut shown_max = 0usize;
    for k in 0..N {
        let t = [b'T', b'0' + (k / 10) as u8, b'0' + (k % 10) as u8];
        if post_full(b"system", &t, b"toastcap fixture", b"", ACT_NONE, b"") {
            posted += 1;
        }
        pass(t0); // one pass per post: the inbound queue (IQ) is never the limit measured here
        shown_max = shown_max.max(banners());
    }
    let queued = ST.lock().wait.len();
    let in_ring = ST.lock().ring.len();
    // hover: the top card at t0+1000 (4000 ms left), held over 4x CARD_MS; it stays, its neighbours time out
    let pk = PANEL.load(Ordering::Relaxed);
    let pw = (pk >> 32) as usize;
    let r0 = card_rect(pw, 0);
    let hseq = ST.lock().cards[0].map(|c| c.n.seq).unwrap_or(0);
    FIX_PTR.store(((r0.0 + r0.2 / 2) as u64) << 32 | (r0.1 + r0.3 / 2) as u64, Ordering::Relaxed);
    let up = |seq: u32| ST.lock().cards.iter().flatten().any(|c| c.n.seq == seq);
    pass(t0 + 1000);
    pass(t0 + 1000 + 4 * CARD_MS);
    shown_max = shown_max.max(banners());
    let held = hseq != 0 && up(hseq);
    FIX_PTR.store(u64::MAX, Ordering::Relaxed);
    let t_leave = t0 + 1000 + 4 * CARD_MS;
    pass(t_leave + (CARD_MS - 1000) - 1);
    shown_max = shown_max.max(banners());
    let resumed = up(hseq); // the frozen 4000 ms, not a fresh 5000, and not gone at once
    pass(t_leave + (CARD_MS - 1000) + 1);
    shown_max = shown_max.max(banners());
    let left = !up(hseq);
    let hover_ok = held && resumed && left;
    // drain: every waiting card shows in turn, three at most
    let mut t = t_leave + CARD_MS;
    for _ in 0..(2 * N) {
        if banners() == 0 && ST.lock().wait.is_empty() {
            break;
        }
        t += CARD_MS + 1;
        pass(t);
        shown_max = shown_max.max(banners());
    }
    let shown = (SHOWN.load(Ordering::Relaxed) - shown0) as usize;
    let drained = banners() == 0 && ST.lock().wait.is_empty();
    let dropped = (DROPPED.load(Ordering::Relaxed) - d0) as usize + (WAIT_SHED.load(Ordering::Relaxed) - shed0) as usize + N.saturating_sub(in_ring);
    MUTE.store(false, Ordering::Relaxed);
    restore(h);
    let (dialogs, primary_max, ret_ok, esc_ok) = super::dialog::toastcap_leg();
    let toast_ok = posted == N && shown_max == STACK_MAX && queued == N - STACK_MAX && dropped == 0 && hover_ok && shown == N && drained;
    let ok = toast_ok && dialogs > 0 && primary_max == 1 && ret_ok && esc_ok;
    let w = |b: bool| if b { "ok" } else { "FAIL" };
    serial_println!(
        ":: TOASTCAP: posted={} shown_max={} queued={} dropped={} hover_pause={} dialogs={} primary_max={} return={} esc={} -> {} :: shown={} drained={} card_ms={} held={} resumed={} left={}",
        posted, shown_max, queued, dropped, w(hover_ok), dialogs, primary_max, w(ret_ok), w(esc_ok), if ok { "PASS" } else { "FAIL" },
        shown, w(drained), CARD_MS, w(held), w(resumed), w(left)
    );
}

/// NOTIFYPANE (B435): `tests notifypane`'s leg over the real pass — `(block, center, sound_gate)`: an app turned
/// off posts nothing (no card, not in the ring); a center-only app collects without a card; two posts of a
/// sounding app ask for ONE sound; under DND a post asks for none.
pub fn pane_leg() -> (bool, bool, bool) {
    let h = hold();
    DND.store(false, Ordering::Relaxed);
    super::notifypane::bypass(false, true);
    super::notifypane::fixture_rules(true);
    let t0 = 2_000_000u64;
    let _ = post_full(b"npoff", b"off fixture", b"never shown", b"", ACT_NONE, b"");
    pass(t0);
    let block = banners() == 0 && ST.lock().ring.is_empty();
    let _ = post_full(b"npctr", b"center fixture", b"collected", b"", ACT_NONE, b"");
    pass(t0 + 1);
    let center = banners() == 0 && ST.lock().ring.len() == 1 && unread() == 1;
    let s0 = super::notifypane::sounds();
    let _ = post_full(b"npsnd", b"sound fixture", b"one", b"", ACT_NONE, b"");
    let _ = post_full(b"npsnd", b"sound fixture", b"two", b"", ACT_NONE, b"");
    pass(t0 + 2);
    let once = super::notifypane::sounds() == s0 + 1 && banners() == 2;
    DND.store(true, Ordering::Relaxed);
    let _ = post_full(b"npsnd", b"sound fixture", b"dnd", b"", ACT_NONE, b"");
    pass(t0 + 3);
    let silent = super::notifypane::sounds() == s0 + 1 && banners() == 2;
    super::notifypane::fixture_rules(false);
    // the fixture's cards close with the held state (restore puts the user's back)
    pass(t0 + 3 + CARD_MS + 1);
    restore(h);
    (block, center, once && silent)
}

// ── REFUSALUI (rmbp-ledger B468) — ACT_ANSWER answers the TOAST verb's poster ─────────────────────────────

/// [`ACT_ANSWER`]'s `arg`: owner (8 LE) + token (4 LE) + button (1).
fn answer_arg(owner: u64, token: u8, ix: u8) -> [u8; 13] {
    let mut a = [0u8; 13];
    a[..8].copy_from_slice(&owner.to_le_bytes());
    a[8..12].copy_from_slice(&(token as u32).to_le_bytes());
    a[12] = ix;
    a
}

/// The card's button was pressed: the poster's answer goes down the dialog's own reply path.
fn answer_poster(arg: &[u8]) -> &'static str {
    if arg.len() < 13 {
        return "answer-none(no-poster)";
    }
    let mut o = [0u8; 8];
    o.copy_from_slice(&arg[..8]);
    let token = arg[8];
    super::dialog::toast_answer(u64::from_le_bytes(o), token, arg[12]);
    "answered"
}

/// `tests refusalui`'s toast leg over the real pass: the queued TOAST-verb entry shows as a card with the
/// poster's button; a press on the button runs [`answer_poster`]. `true` when the card showed with a button
/// and the press closed it as an action.
pub fn answer_leg() -> bool {
    let h = hold();
    DND.store(false, Ordering::Relaxed);
    let t0 = 3_000_000u64;
    pass(t0);
    let pk = PANEL.load(Ordering::Relaxed);
    let pw = (pk >> 32) as usize;
    let shown = banners() == 1 && ST.lock().cards[0].map(|c| c.n.act == ACT_ANSWER && c.n.has_button()).unwrap_or(false);
    let r0 = card_rect(pw, 0);
    let b = btn_rect();
    let (bx, by) = (r0.0 + metrics::size(b.0 + 4), r0.1 + metrics::size(b.1 + 4));
    let pressed = shown && SLOT_BTN[0].load(Ordering::Acquire) && press_at(bx as i32, by as i32) && { pass(t0 + 1); banners() == 0 };
    restore(h);
    pressed
}

/// SMALLFIX4 (STATUSTRAY fold) — the bar's two right-end surfaces agree: the Center (the bell) and the status
/// items' menus (the clock's among them) SWITCH on one press, as winmenu's titles do. With the Center open, a
/// press on a status item closes the Center and is handed on (`false` from `press_at`), so `crystal::press_at`
/// opens that item's menu — the clock's menu is reachable from the Center without a dismiss press first.
/// Witness: `[notify] switch from=center to=<item>`.
fn center_switch(pw: usize, ph: usize, px: usize, py: usize) -> bool {
    let Some(i) = super::statusmenu::item_at(pw, ph, px, py) else { return false };
    REQ_CENTER.store(2, Ordering::Release);
    serial_println!("[notify] switch from=center to={}", super::status::item_name(i));
    true
}

/// SMALLFIX4 (STATUSTRAY fold) — the other direction: the bell pressed while a status item's menu is down
/// closes that menu before the Center toggles. Witness: `[notify] switch from=<item> to=center`.
fn bell_switch() {
    let i = super::statusmenu::open_item();
    if i == super::status::ITEM_NONE {
        return;
    }
    super::crystal::dismiss_for_switch();
    serial_println!("[notify] switch from={} to=center", super::status::item_name(i));
}

// ── NOTICEFIX (rmbp-ledger B491) — `tests notice` drives the REAL card store, model-only ──────────────────

/// The live stack, ring and Center set aside while a fixture runs headless ([`fixture_restore`] puts them back).
pub struct FixtureHeld(Held);

/// Hold the live notification state and go headless: a fixture's card never reaches the glass, the ring or the badge.
pub fn fixture_hold() -> FixtureHeld {
    FixtureHeld(hold())
}

pub fn fixture_restore(h: FixtureHeld) {
    restore(h.0)
}

/// Run one pass at the latest card's own `until` (the clock it was shown on), so the time-close is the real one.
pub fn expire_now() {
    let due = ST.lock().cards.iter().flatten().map(|c| c.until).max();
    if let Some(t) = due {
        pass(t.max(crate::arch::ms()));
    }
}

// ── DESKTOPCRAFT (rmbp-ledger B631) — the toast card's paint ─────────────────────────────────────────────────────
// The card is a chromeless overlay row (`wm::overlay_open`) of `W` x `H`, so its rounded corners and its shadow are
// drawn INSIDE that surface: a `CARD_MARGIN` band of the wallpaper's VEIL (sampled where slot 0 sits, the slot every
// card is painted for) under the card's soft shadow, then the card body — `group_fill`, `CARD_RADIUS` corners, a
// hairline edge. Geometry, the press router and the stack are unchanged; the Center keeps its own `frame`.

/// The band around the card body that holds its shadow (logical px).
const CARD_MARGIN: usize = 4;
/// The card body's corner radius (logical px; DESIGN-LANGUAGE `radius.card` + 2, held to the body's half height).
const CARD_RADIUS: usize = 12;

fn craft_card(px: &mut [u32], pw: usize, ph: usize) {
    use super::draw::{self, Ink, Rect, Surf};
    let ph = ph.min(px.len() / pw.max(1));
    let (cx, cy) = match crate::video::panel_info_nonblocking() {
        Some(i) => {
            let r = card_rect(i.width, 0);
            (r.0, r.1)
        }
        None => (0, 0),
    };
    let back = super::wm::desktop::live_back();
    for y in 0..ph {
        let row = &mut px[y * pw..(y + 1) * pw];
        match back.as_ref() {
            Some(b) => b.soft(row, cx, cy + y),
            None => row.fill(theme::DESKTOP_BG),
        }
    }
    let m = metrics::size(CARD_MARGIN) as i32;
    let body = Rect::new(m, m, pw as i32 - 2 * m, ph as i32 - 2 * m);
    let rad = (metrics::size(CARD_RADIUS) as i32).min(body.h / 2);
    let mut s = Surf::new(&mut px[..pw * ph], pw);
    let drop = metrics::size(1) as i32;
    draw::shadow_rrect(&mut s, body.offset(0, drop), rad, (m - drop).max(1), theme::shadow_soft());
    draw::fill_rrect(&mut s, body, rad, theme::group_fill());
    draw::stroke_rrect(&mut s, body, rad, 1, Ink::argb(theme::hairline()));
}

fn craft_icon(px: &mut [u32], pw: usize, ph: usize, app: &[u8]) {
    use super::draw::{self, Rect, Surf};
    let key = if app.is_empty() { "generic" } else { s(app) };
    let (x, y, size) = (metrics::size(12), metrics::size((H - ICON) / 2), metrics::size(ICON));
    if crate::fs::appres::blit_key_icon(px, pw, ph, x, y, size, key) {
        return;
    }
    let ph = ph.min(px.len() / pw.max(1));
    let mut sf = Surf::new(&mut px[..pw * ph], pw);
    draw::fill_rrect(&mut sf, Rect::new(x as i32, y as i32, size as i32, size as i32), draw::icon_radius(size) as i32, theme::accent());
}

fn craft_button(px: &mut [u32], pw: usize, ph: usize, r: (usize, usize, usize, usize), t: &[u8]) {
    use super::draw::{self, Ink, Rect, Surf};
    let face = super::text::Face::Ui;
    {
        let ph = ph.min(px.len() / pw.max(1));
        let mut sf = Surf::new(&mut px[..pw * ph], pw);
        let (x0, y0) = (metrics::edge(r.0) as i32, metrics::edge(r.1) as i32);
        let b = Rect::new(x0, y0, metrics::edge(r.0 + r.2) as i32 - x0, metrics::edge(r.1 + r.3) as i32 - y0);
        let rad = (metrics::size(draw::DIMS.button_radius) as i32).min(b.h / 2);
        draw::shadow_rrect(&mut sf, b.offset(0, metrics::size(draw::DIMS.button_drop) as i32), rad, metrics::size(draw::DIMS.button_blur) as i32, theme::shadow_soft());
        draw::fill_rrect(&mut sf, b, rad, theme::knob_face());
        draw::stroke_rrect(&mut sf, b, rad, 1, Ink::argb(theme::control_edge()));
    }
    let tw = metrics::ladvance(t, false, face);
    let tx = r.0 + r.2.saturating_sub(tw) / 2;
    let ty = r.1 + r.3.saturating_sub(metrics::lcell_h(face)) / 2;
    let _ = metrics::text(px, pw, ph, r.0 + r.2, tx, ty, t, theme::text_primary(), false, face);
}

// ── MENUSCRAFT (rmbp-ledger B642) — the toast's hover x ───────────────────────────────────────────────────────────
// A card under the pointer shows a close button on its top-left corner (a `surface.raised` disc with a `line.subtle`
// edge, a soft `elev.2` lift and a `text.secondary` x), the notification idiom of both looks. The press router is
// unchanged: a press anywhere on a card but its action button already dismisses it (`REQ_DISMISS`), so the x is
// the affordance for the press that was always there. Repainted only on the hover EDGE (`pass` latches the hovered
// seq): the card leaving the pointer repaints without it, the card arriving repaints with it, each presented once.

/// The hovered card's seq (`0` = none), the edge latch.
static HOVER_SEQ: AtomicU32 = AtomicU32::new(0);
/// The close button's diameter (logical px).
const CLOSE_D: usize = 18;

fn craft_hover(st: &St, hot: u32) {
    let was = HOVER_SEQ.swap(hot, Ordering::Relaxed);
    if was == hot {
        return;
    }
    for c in st.cards.iter().flatten() {
        if c.n.seq != was && c.n.seq != hot {
            continue;
        }
        paint_card(c.surf, &c.n);
        if c.n.seq == hot {
            craft_close(c.surf);
        }
        if c.win != wm::WIN_NONE {
            let _ = wm::present(c.win);
        }
    }
}

fn craft_close(i: usize) {
    use super::draw::{self, Ink, Rect, Surf};
    let px = surf(i);
    let pw = metrics::size(W);
    let ph = metrics::size(H).min(px.len() / pw.max(1));
    let mut s = Surf::new(&mut px[..pw * ph], pw);
    let r = (metrics::size(CLOSE_D) as i32 / 2).max(3);
    let c = r + 1; // the disc sits on the card's top-left corner, inside the surface
    draw::shadow_rrect(&mut s, Rect::new(c - r, c - r + metrics::size(1) as i32, 2 * r, 2 * r), r, metrics::size(2) as i32, theme::shadow_soft());
    draw::fill_circle(&mut s, c, c, r, theme::knob_face());
    draw::stroke_circle(&mut s, c, c, r, 1, Ink::argb(theme::hairline()));
    let sub = draw::SUB;
    let arm = r * sub * 2 / 5;
    let (cx, cy) = (c * sub, c * sub);
    let wdt = (metrics::size(3) as i32 * sub / 2).max(sub);
    draw::line_aa(&mut s, cx - arm, cy - arm, cx + arm, cy + arm, wdt, theme::ink_secondary());
    draw::line_aa(&mut s, cx - arm, cy + arm, cx + arm, cy - arm, wdt, theme::ink_secondary());
}

// ── SOFTY3 (rmbp-ledger B651) — the clock's Center under the Softy look ──────────────────────────────────────────
// Windows' clock flyout shows the notifications beside the month. Under Softy the taskbar's clock opens the look's
// calendar (`softy::startmenu`, kind 2) and that flyout opens notify's OWN Center with itself and closes it with
// itself, through the one seam below; the Center's rect follows the look (`center_rect` → [`softy_center_at`]).

/// The Softy calendar's seam: `startmenu::open(2)` requests the Center open, `startmenu::close` of the calendar
/// requests it closed. Atomics only (the press router's rule); [`service`]'s next pass acts (codes 4 open / 5 close,
/// an open request on an open Center is spent). The Center's own closes (an item's action, a press outside under
/// the UnaOS look) leave the calendar alone.
pub fn request_center(open: bool) {
    REQ_CENTER.store(if open { 4 } else { 5 }, Ordering::Release);
}

/// Under Softy the Center has no menu bar to hang from: it sits ABOVE the taskbar (`softy::taskbar::top`) with a
/// `FLY_GAP` band over the bar, at the right edge — and to the LEFT of the calendar flyout while that is open, the
/// two side by side. `None` under the UnaOS look (the caller keeps the bar placement). The panel height is the one
/// the pass published (`PANEL`), the same the press router reads.
fn softy_center_at(pw: usize, sw: usize, sh: usize) -> Option<(usize, usize)> {
    if !super::look::is_softy() {
        return None;
    }
    let ph = (PANEL.load(Ordering::Relaxed) & 0xffff_ffff) as usize;
    let gap = super::softy::palette::px(super::softy::palette::FLY_GAP);
    let top = super::softy::taskbar::top(pw, ph);
    let y = top.saturating_sub(gap + sh);
    let x = match super::softy::startmenu::rect() {
        Some((fx, _, _, _)) if super::softy::startmenu::open_kind() == 2 && fx >= sw + 2 * gap => fx - gap - sw,
        _ => pw.saturating_sub(sw + gap),
    };
    Some((x, y))
}
