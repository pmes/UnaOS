#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// LUMENAPP (rmbp-ledger B323, R82): `APPS/LUMEN.ELF` — Lumen on UnaOS is ONE program, like the Claude
// app. Peter: "just make it work like the claude app. it does not need system services running."
// LUMENUX (B348) made the window one: scrollback, markdown, copy, history, a status line.
//
// WHAT IT IS. A window over a conversation that talks to the provider ITSELF: it links Vein as a library
// (`vein_core`, the pure encoder/decoder/client/rules plus the markdown renderer, the scrollback ring and the conversation-file codec (LUMENUX), and `vein_ring3`, the syscall transport and file surface), resolves
// the endpoint (SYS_RESOLVE), opens TCP (the NETRING3 socket verbs), runs TLS 1.3 over it (UnaOS's own
// tls_core, every server VERIFIED against /system/trust/roots.pem at SYS_TIME — VEINTLS, SR36),
// POSTs /v1/messages with `stream: true` and draws the deltas as they arrive. Nothing else runs: no
// daemon, no bus chat verbs, no autostart. (LUMENBIN's two-program shape — this window relaying to a
// VEIN.BIN service over bus verbs 130..133 — is retired.)
//
// CONFIGURATION is Principia's (BUS_VERB_PREF_GET, namespace `vein`): `vein.provider`, `vein.model`,
// `vein.endpoint`, `vein.key_file`. The KEY is read only from a file on the UnaFS volume (it stats with an
// inode id); on FAT it is refused with the reason in the window. With no key the Echo provider answers, so
// the window works on the FAT card. `vein_core::prefs::plan` is the rule, and the key crosses the wire
// ONLY on a verified TLS connection: no trust store, no clock or no crypto provider = Echo, with the
// missing piece named in the window (there is no `vein.tls = "insecure"` any more).
//
// THE ELF MODEL. Linked at una_abi::USER_XWIN_VA_X86 (user-lumen-x86.ld, the user-big shape), so the
// kernel maps it in the 4 MiB ELF window with a 256 KiB stack (`-z stack-size`): the TLS + X.509 code, the
// ~70 KiB of exchange buffers and the heap (vein_ring3::heap over SYS_SBRK: TLS records, the parsed trust
// store) cannot live in the 16 KiB fixed window. The window landmarks (info page, surface)
// stay at the fixed USER_BASE_X86 + 0x4000 / + 0x5000.
// LUMENUX spends the rest of the window on the transcript (1 MiB, LOG_CAP) and the scrollback ring (LINES_CAP rows
// of 12 bytes) — see LUMENUX.md for the measured fit.
//
// KEYS. Printable ASCII inserts at the caret; Backspace deletes; Left/Right (and the caret actions) move
// it; Enter sends. Up/Down scroll a row, the wheel three, Shift+PgUp/PgDn a page, Cmd/Ctrl+Home/End jump.
// Ctrl-P / Ctrl-N select the previous / next reply; Cmd-C copies the selected reply (default the last),
// Cmd-V pastes. Ctrl-K or Cmd-K starts a new conversation; Esc cancels an answer in flight and NEVER
// closes the window (R24). Commands: /new /list /open N /copy /help.
//
// WIRE. `:: LUMEN: start provider=<claude|echo> model=<m> key=<holocron|unafs|none|fat-refused> [holocron=<no-fulfiller|not-found|refused>] transport=<tls|http|none>
// trust=<roots|none> clock=<set|unset> history=<N|off> files=<path|open|off> font=<dejavu-sans font_kib=<n>|font8x8 font_why=<w>> ::` once (KERNELFONT2), then per answer `:: LUMEN: reply provider=<p>
// first_token_ms=<n> bytes=<n> stop=<s> [transport=tls verified=<issuer CN>] in=<n> out=<n> ::` or `:: LUMEN: fail stage=<s>
// code=<n> [tls=<why>] ::`; `:: LUMEN: clip set=<n> ::` per copy; history failures as `:: LUMEN: history <op> code=<n> ::`.
// The window's footer shows the provider and the first token's latency.

use una_abi::{
    input_ev_payload, input_ev_type, INPUT_EV_ACTION, INPUT_EV_KEY_DOWN, INPUT_EV_WHEEL, INPUT_EV_WIN_RESIZE, INPUT_EV_CLOSE_REQ, KEY_DOWN, KEY_ESC, KEY_LEFT, KEY_RIGHT, KEY_UP,
    SYS_EXIT, SYS_INPUT_POLL, SYS_WIN_CREATE, SYS_WIN_PRESENT,
};
use vein_core::claude::{Event, Msg, Params, Stop};
use vein_core::history::{self, Who};
use vein_core::md::{self, Block, Span, State, Tint};
use vein_core::prefs::{KeyState, Plan};
use vein_core::scroll::{Rec, Ring, F_FENCE, F_LEAD, F_TURN};
use vein_core::{Out, Role};
use vein_ring3::files::{self, Files, Home};
use vein_ring3::sys::{now_ms, sleep_ms, sys, write};

extern crate alloc;

mod txt;
use txt::Kind;

/// VEINTLS (SR36): tls_core allocates (records, transcript, the parsed trust store); the size-class heap
/// over SYS_SBRK frees and reuses, so a long streamed answer stays inside the ELF window.
#[global_allocator]
static HEAP: vein_ring3::heap::Heap = vein_ring3::heap::Heap::sbrk();


const ACTION_CLEAR_VIEW: u64 = 41;
const ACTION_COPY: u64 = 3;
const ACTION_PASTE: u64 = 5;
const ACTION_CURSOR_LEFT: u64 = 13;
const ACTION_CURSOR_RIGHT: u64 = 14;
const ACTION_LINE_START: u64 = 15;
const ACTION_LINE_END: u64 = 16;
const ACTION_PAGE_UP: u64 = 22;
const ACTION_PAGE_DOWN: u64 = 23;
const ACTION_TOP: u64 = 24;
const ACTION_BOTTOM: u64 = 25;
const KEY_CTRL_N: u8 = 0x0e;
const KEY_CTRL_P: u8 = 0x10;

const SYSTEM: &str = "You are Claude, talking with the person through Lumen, the chat window of UnaOS. The window renders markdown (headings, bold, italic, bullet and numbered lists, fenced code, inline code, links) in a narrow window (about 40 characters a line, DejaVu Sans; code in DejaVu Sans Mono): keep lines short, and use no tables, images or non-ASCII symbols.";

fn exit(code: u64) -> ! {
    sys(SYS_EXIT, code, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

// ---- console lines --------------------------------------------------------------------------------
struct Line {
    b: [u8; 200],
    n: usize,
}
impl Line {
    fn new(s: &[u8]) -> Line {
        let mut l = Line { b: [0; 200], n: 0 };
        l.put(s);
        l
    }
    fn put(&mut self, s: &[u8]) -> &mut Self {
        for &c in s {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }
    fn dec(&mut self, v: i64) -> &mut Self {
        if v < 0 {
            self.put(b"-");
        }
        let mut v = v.unsigned_abs();
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..])
    }
    fn opt(&mut self, v: Option<u32>) -> &mut Self {
        match v {
            Some(v) => self.dec(v as i64),
            None => self.put(b"-"),
        }
    }
    fn s(&self) -> &[u8] {
        &self.b[..self.n]
    }
    fn wire(&mut self) {
        self.put(b"\n");
        write(&self.b[..self.n]);
        self.n -= 1;
    }
}

// ---- the app --------------------------------------------------------------------------------------
const MAX_W: i32 = 288;
const MAX_H: i32 = 288;
const STRIDE: usize = MAX_W as usize * 4;
/// KERNELFONT2: the line box is the face's ([`txt::LH_TT`]) or the font8x8 grid's ([`txt::LH_BITMAP`]); see [`lh`].
static mut TXT: Option<txt::Txt> = None;
/// The loaded faces (KERNELFONT2), `None` on the font8x8 grid. Single-threaded, as [`app`].
fn tt() -> Option<&'static mut txt::Txt> {
    unsafe { (*core::ptr::addr_of_mut!(TXT)).as_mut() }
}
fn lh() -> i32 {
    if tt().is_some() { txt::LH_TT } else { txt::LH_BITMAP }
}
/// The advance of display byte `c` drawn as `k`, px (8 on the font8x8 grid).
fn adv(c: u8, k: Kind) -> f32 {
    tt().map_or(8.0, |t| t.adv(c, k))
}
fn width(s: &[u8], k: Kind) -> f32 {
    s.iter().map(|&c| adv(c, k)).sum()
}
/// How many leading bytes of `s` fit in `max` px.
fn fit(s: &[u8], k: Kind, max: f32) -> usize {
    let mut x = 0f32;
    for (i, &c) in s.iter().enumerate() {
        x += adv(c, k);
        if x > max + 0.01 {
            return i;
        }
    }
    s.len()
}
/// What a rendered byte is drawn with: (kind, ink, italic, code ground).
fn style_at(spans: &[Span], i: usize, fence: bool) -> (Kind, u32, bool, bool) {
    for sp in spans {
        if (sp.start as usize) <= i && i < sp.end as usize {
            let code = sp.tint == Tint::Code;
            let k = if code || fence { Kind::Mono } else if sp.bold || sp.tint == Tint::Heading { Kind::Bold } else { Kind::Sans };
            return (k, tint_ink(sp.tint), sp.italic, code);
        }
    }
    (if fence { Kind::Mono } else { Kind::Sans }, INK, false, false)
}
/// The transcript: every turn of the conversation as shown and saved (oldest turns drop first).
const LOG_CAP: usize = 1 << 20;
/// The scrollback ring: rendered rows (12 bytes each). LUMENUX M1's measured budget (LUMENUX.md).
const LINES_CAP: usize = vein_core::scroll::LUMEN_ROWS;
/// The context sent: the newest whole turns that fit (the 48 KiB request body less escaping room).
const CTX_CAP: usize = 32 * 1024;
/// One source line rendered at a time (a longer line renders its first RT_CAP bytes).
const RT_CAP: usize = 8192;
const RS_CAP: usize = 512;
/// A conversation file is read through this (a longer file reloads its newest FILE_CAP bytes).
const FILE_CAP: usize = 256 * 1024;
/// One appended turn.
const OUT_CAP: usize = 64 * 1024;
const INP_CAP: usize = 1024;
const ROLE_USER: u8 = 1;
const ROLE_AI: u8 = 2;
const ROLE_NOTE: u8 = 3;

const BG: u32 = 0xFF14_1218;
const RULE: u32 = 0xFF3A_3550;
const INK: u32 = 0xFFE6_E3EC;
const USER_INK: u32 = 0xFF8F_B8FF;
const NOTE_INK: u32 = 0xFF9A_92A8;
const CARET: u32 = 0xFFB0_8CFF;
const OK_INK: u32 = 0xFF7C_D992;
const HEAD_INK: u32 = 0xFFFF_D27A;
const CODE_INK: u32 = 0xFF9F_E6B0;
const CODE_BG: u32 = 0xFF20_1C2A;
const QUOTE_INK: u32 = 0xFFB8_B0C8;
const LINK_INK: u32 = 0xFF7F_C8FF;
const SEL_BAR: u32 = 0xFFB0_8CFF;

static FONT: [[u8; 8]; 128] = font8x8::legacy::BASIC_LEGACY; // the grid when no face is on the volume (KERNELFONT2)

/// Where the last laid source line starts, and the layout state there (relayout resumes from it).
#[derive(Clone, Copy)]
struct Cursor {
    src: u32,
    role: u8,
    fence: bool,
    turn: bool,
}
const CURSOR0: Cursor = Cursor { src: 0, role: 0, fence: false, turn: false };

/// The conversation file this window appends to.
struct Hist {
    f: Files,
    seq: u32,
    max: u32,
    size: u64,
    created: bool,
    failed: bool,
}

struct App {
    /// The transcript (`LOG`, its own zeroed static so the 1 MiB lands in .bss, not in the file).
    log: &'static mut [u8],
    log_n: usize,
    inp: [u8; INP_CAP],
    inp_n: usize,
    caret: usize,
    st1: [u8; 64],
    st1_n: usize,
    st2: [u8; 64],
    st2_n: usize,
    flash: [u8; 48],
    flash_n: usize,
    flash_until: u64,
    w: i32,
    h: i32,
    /// Rows scrolled back from the bottom.
    scroll: i32,
    max_scroll: i32,
    rows_vis: i32,
    ring: Ring<'static>,
    cur: Cursor,
    cols: usize,
    layout_dirty: bool,
    layout_full: bool,
    /// The selected reply: [tag offset, end).
    sel: Option<(u32, u32)>,
    /// The tag offset of the reply streaming now.
    ai_start: Option<u32>,
    hist: Hist,
    busy: bool,
    cancel: bool,
    turn_open: bool,
    dirty: bool,
    last_paint: u64,
    win: i64,
    flags: *const u32,
    surf: *mut u8,
}
static mut APP: App = App {
    log: &mut [],
    log_n: 0,
    inp: [0; INP_CAP],
    inp_n: 0,
    caret: 0,
    st1: [0; 64],
    st1_n: 0,
    st2: [0; 64],
    st2_n: 0,
    flash: [0; 48],
    flash_n: 0,
    flash_until: 0,
    w: 0,
    h: 0,
    scroll: 0,
    max_scroll: 0,
    rows_vis: 1,
    ring: Ring::new(&mut []),
    cur: CURSOR0,
    cols: 0,
    layout_dirty: true,
    layout_full: true,
    sel: None,
    ai_start: None,
    hist: Hist { f: Files::None(0), seq: 1, max: 0, size: 0, created: false, failed: false },
    busy: false,
    cancel: false,
    turn_open: false,
    dirty: false,
    last_paint: 0,
    win: 0,
    flags: core::ptr::null(),
    surf: core::ptr::null_mut(),
};
/// The one App. The program is single-threaded: the main loop and the network wait hook (`tick`, run
/// from inside vein_ring3's socket waits) are never live at the same time, so each takes it in turn.
fn app() -> &'static mut App {
    unsafe { &mut *core::ptr::addr_of_mut!(APP) }
}

static mut LOG: [u8; LOG_CAP] = [0; LOG_CAP];
static mut RECS: [Rec; LINES_CAP] = [Rec::ZERO; LINES_CAP];
static mut RT: [u8; RT_CAP] = [0; RT_CAP];
static mut RS: [Span; RS_CAP] = [Span::EMPTY; RS_CAP];
static mut FILEBUF: [u8; FILE_CAP] = [0; FILE_CAP];
static mut OUTBUF: [u8; OUT_CAP] = [0; OUT_CAP];
const REQ_CAP: usize = files::next::PATH_IO_HDR_LEN + files::next::PATH_IO_PATH_MAX + files::next::PATH_IO_MAX;
static mut REQ: [u8; REQ_CAP] = [0; REQ_CAP];
static mut HOME: Home = Home { b: [0; 256], n: 0 };
static mut BUFS: vein_ring3::Buffers = vein_ring3::Buffers::new();
static mut KEY: [u8; 320] = [0; 320];

fn rt() -> &'static mut [u8; RT_CAP] {
    unsafe { &mut *core::ptr::addr_of_mut!(RT) }
}
fn rs() -> &'static mut [Span; RS_CAP] {
    unsafe { &mut *core::ptr::addr_of_mut!(RS) }
}
fn filebuf() -> &'static mut [u8; FILE_CAP] {
    unsafe { &mut *core::ptr::addr_of_mut!(FILEBUF) }
}
fn home() -> &'static mut Home {
    unsafe { &mut *core::ptr::addr_of_mut!(HOME) }
}

fn clean(dst: &mut [u8], parts: &[&[u8]]) -> usize {
    let mut n = 0;
    for &c in parts.iter().flat_map(|p| p.iter()) {
        if n < dst.len() {
            dst[n] = if (0x20..0x7f).contains(&c) { c } else { b'?' };
            n += 1;
        }
    }
    n
}

impl App {
    fn set_status(&mut self, row1: &[&[u8]], row2: &[&[u8]]) {
        self.st1_n = clean(&mut self.st1, row1);
        self.st2_n = clean(&mut self.st2, row2);
        self.dirty = true;
    }
    fn flash(&mut self, parts: &[&[u8]]) {
        self.flash_n = clean(&mut self.flash, parts);
        self.flash_until = now_ms() + 2500;
        self.dirty = true;
    }

    /// The transcript dropped its first `cut` bytes: rebase every offset that points into it.
    fn shift(&mut self, cut: u32) {
        self.ring.rebase(cut);
        if self.cur.src < cut {
            self.layout_full = true;
        } else {
            self.cur.src -= cut;
        }
        self.sel = self.sel.and_then(|(s, e)| if s < cut { None } else { Some((s - cut, e.saturating_sub(cut))) });
        self.ai_start = self.ai_start.and_then(|s| s.checked_sub(cut));
    }

    /// Make room for `k` more bytes by dropping the OLDEST turn(s).
    fn room(&mut self, k: usize) {
        while self.log_n + k > LOG_CAP && self.log_n > 0 {
            let mut cut = 1;
            while cut < self.log_n && self.log[cut] > ROLE_NOTE {
                cut += 1;
            }
            if cut >= self.log_n {
                cut = (self.log_n / 2).max(1);
            }
            self.log.copy_within(cut..self.log_n, 0);
            self.log_n -= cut;
            if self.log_n > 0 && self.log[0] > ROLE_NOTE {
                self.log[0] = ROLE_NOTE; // a clipped turn is shown, never sent half
            }
            self.shift(cut as u32);
        }
    }

    /// Append text (printable ASCII + '\n'; a UTF-8 sequence becomes one '?', tabs a space).
    fn add(&mut self, text: &[u8]) {
        for &c in text {
            let c = match c {
                b'\n' | 0x20..=0x7e => c,
                b'\t' => b' ',
                0x80..=0xbf | b'\r' => continue,
                _ => b'?',
            };
            self.room(1);
            self.log[self.log_n] = c;
            self.log_n += 1;
        }
        self.layout_dirty = true;
        self.dirty = true;
    }
    fn turn(&mut self, role: u8, text: &[u8]) {
        self.room(1);
        if role == ROLE_AI {
            self.ai_start = Some(self.log_n as u32);
        }
        self.log[self.log_n] = role;
        self.log_n += 1;
        self.add(text);
        self.turn_open = role == ROLE_AI;
    }
    fn note(&mut self, text: &[u8]) {
        self.turn(ROLE_NOTE, text);
    }

    fn clear(&mut self) {
        self.log_n = 0;
        self.scroll = 0;
        self.turn_open = false;
        self.sel = None;
        self.ai_start = None;
        self.layout_full = true;
        self.layout_dirty = true;
        self.dirty = true;
    }

    /// End of the source line starting at `p`: the next '\n', the next turn tag, or the log end.
    fn line_end(&self, p: usize) -> usize {
        let mut e = p;
        while e < self.log_n && self.log[e] != b'\n' && self.log[e] > ROLE_NOTE {
            e += 1;
        }
        e
    }
    /// End of the turn whose tag is at `s`.
    fn turn_end(&self, s: usize) -> usize {
        let mut e = s + 1;
        while e < self.log_n && self.log[e] > ROLE_NOTE {
            e += 1;
        }
        e
    }

    // ---- M1: layout into the ring -------------------------------------------------------------
    /// KERNELFONT2: the wrap width in px (the field keeps its name: a change re-lays the transcript).
    fn cols_for(&self) -> usize {
        (self.w - 8 - 4).max(64) as usize
    }

    /// Lay the transcript into the ring: incrementally from the last laid source line, or from the start.
    fn relayout(&mut self) {
        let cols = self.cols_for();
        let before = self.ring.len();
        if self.layout_full || cols != self.cols {
            self.ring.clear();
            self.cur = CURSOR0;
            self.cols = cols;
            self.layout_full = false;
        } else {
            self.ring.truncate_from_src(self.cur.src);
        }
        let Cursor { src, mut role, mut fence, mut turn } = self.cur;
        let mut p = src as usize;
        let mut saved = self.cur;
        loop {
            if p < self.log_n && self.log[p] <= ROLE_NOTE {
                if role != 0 {
                    self.ring.push(Rec { src: p as u32, ..Rec::ZERO });
                }
                role = self.log[p];
                fence = false;
                turn = true;
                p += 1;
                continue;
            }
            if role == 0 {
                break;
            }
            saved = Cursor { src: p as u32, role, fence, turn };
            let e = self.line_end(p);
            self.lay_line(p, e, role, &mut fence, turn, cols);
            turn = false;
            if e < self.log_n && self.log[e] == b'\n' {
                p = e + 1;
            } else if e < self.log_n {
                p = e;
            } else {
                break;
            }
        }
        self.cur = saved;
        self.layout_dirty = false;
        // A reader scrolled back keeps the rows they are reading in place while the answer grows.
        let after = self.ring.len();
        if self.scroll > 0 && after > before {
            self.scroll += (after - before) as i32;
        }
    }

    fn lay_line(&mut self, p: usize, e: usize, role: u8, fence: &mut bool, turn: bool, cols: usize) {
        let lead = |first: bool| (if first { F_LEAD } else { 0 }) | (if first && turn { F_TURN } else { 0 });
        if role == ROLE_AI {
            let fbit = if *fence { F_FENCE } else { 0 };
            let mut st = State { fence: *fence };
            let l = md::line(&mut st, &self.log[p..e], rt(), rs());
            *fence = st.fence;
            let text = &rt()[..l.len];
            let block = l.block as u8;
            let fence_row = matches!(l.block, Block::Code | Block::Fence);
            let spans = &rs()[..l.spans];
            let hang = l.hang.min(text.len()).min(u8::MAX as usize);
            let hang_px = (0..hang).map(|i| adv(text[i], style_at(spans, i, fence_row).0)).sum::<f32>();
            let ring = &mut self.ring;
            md::wrap_px(text, cols as f32, hang_px, &|i| adv(text[i], style_at(spans, i, fence_row).0), &mut |a, b, first| {
                ring.push(Rec { src: p as u32, dofs: a as u16, len: (b - a) as u16, role, block, hang: if first { 0 } else { hang as u8 }, flags: fbit | lead(first) });
            });
        } else {
            let e = e.min(p + u16::MAX as usize);
            let text = &self.log[p..e];
            let ring = &mut self.ring;
            md::wrap_px(text, cols as f32 - 16.0, 0.0, &|i| adv(text[i], Kind::Sans), &mut |a, b, first| {
                ring.push(Rec { src: p as u32, dofs: a as u16, len: (b - a) as u16, role, block: 0, hang: 0, flags: lead(first) });
            });
        }
    }

    fn scroll_by(&mut self, d: i32) {
        self.scroll = self.scroll.saturating_add(d).clamp(0, self.max_scroll.max(0));
        self.dirty = true;
    }

    /// Put the first row of the turn whose tag is at `s` at the top of the view.
    fn scroll_to(&mut self, s: u32) {
        if self.layout_dirty {
            self.relayout();
        }
        let total = self.ring.len();
        let mut i = 0;
        while i < total && self.ring.get(i).is_some_and(|r| r.src <= s) {
            i += 1;
        }
        let rows = self.rows_vis.max(1) as usize;
        let max = total.saturating_sub(rows);
        self.max_scroll = max as i32;
        self.scroll = (total.saturating_sub(rows).saturating_sub(i)).min(max) as i32;
        self.dirty = true;
    }

    // ---- M3: replies, copy, paste -------------------------------------------------------------
    fn ai_before(&self, pos: usize) -> Option<(u32, u32)> {
        let mut i = pos.min(self.log_n);
        while i > 0 {
            i -= 1;
            if self.log[i] == ROLE_AI {
                return Some((i as u32, self.turn_end(i) as u32));
            }
        }
        None
    }
    fn ai_after(&self, pos: usize) -> Option<(u32, u32)> {
        let mut i = pos;
        while i < self.log_n {
            if self.log[i] == ROLE_AI {
                return Some((i as u32, self.turn_end(i) as u32));
            }
            i += 1;
        }
        None
    }
    fn select(&mut self, prev: bool) {
        let next = if prev {
            self.ai_before(self.sel.map_or(self.log_n, |(s, _)| s as usize)).or(self.sel)
        } else {
            self.sel.and_then(|(s, _)| self.ai_after(s as usize + 1))
        };
        self.sel = next;
        match next {
            Some((s, _)) => self.scroll_to(s),
            None => self.scroll = 0,
        }
        self.dirty = true;
    }

    fn copy(&mut self) {
        let Some((s, e)) = self.sel.or_else(|| self.ai_before(self.log_n)) else {
            self.flash(&[b"no reply to copy"]);
            return;
        };
        let mut t = &self.log[s as usize + 1..e as usize];
        while t.last() == Some(&b'\n') {
            t = &t[..t.len() - 1];
        }
        let full = t.len();
        if t.len() > una_abi::CLIP_CAP {
            let cut = t[..una_abi::CLIP_CAP].iter().rposition(|&c| c == b'\n').unwrap_or(una_abi::CLIP_CAP);
            t = &t[..cut];
        }
        let r = sys(una_abi::SYS_CLIP_SET, t.as_ptr() as u64, t.len() as u64, 0, 0);
        let mut l = Line::new(b":: LUMEN: clip set=");
        l.dec(r).put(b" ::");
        l.wire();
        let mut f = Line::new(b"");
        if r < 0 {
            f.put(b"copy refused (").dec(r).put(b")");
        } else if (r as usize) < full {
            f.put(b"copied ").dec(r).put(b" of ").dec(full as i64).put(b" bytes");
        } else {
            f.put(b"copied ").dec(r).put(b" bytes");
        }
        let n = f.n;
        self.flash(&[&f.b[..n]]);
    }

    fn paste(&mut self) {
        let mut b = [0u8; una_abi::CLIP_CAP];
        let r = sys(una_abi::SYS_CLIP_GET, b.as_mut_ptr() as u64, b.len() as u64, 0, 0);
        if r < 0 {
            let mut f = Line::new(b"paste refused (");
            f.dec(r).put(b")");
            let n = f.n;
            self.flash(&[&f.b[..n]]);
            return;
        }
        for &c in &b[..(r as usize).min(b.len())] {
            let c = if c == b'\n' || c == b'\t' { b' ' } else { c };
            if (0x20..0x7f).contains(&c) && self.inp_n < INP_CAP {
                self.inp.copy_within(self.caret..self.inp_n, self.caret + 1);
                self.inp[self.caret] = c;
                self.caret += 1;
                self.inp_n += 1;
            }
        }
    }

    /// One input event. While an answer streams, Enter does nothing and Esc cancels.
    fn event(&mut self, ev: u64) {
        let p = input_ev_payload(ev);
        self.dirty = true;
        let page = (self.rows_vis - 1).max(1);
        match input_ev_type(ev) {
            INPUT_EV_KEY_DOWN => self.key(p as u8),
            una_abi::INPUT_EV_DROP => self.dropped(p), INPUT_EV_CLOSE_REQ => exit(0), // APPMENU2 M6: the WM asks us to quit — nothing unsaved, so leave now
            INPUT_EV_ACTION => match p {
                ACTION_CLEAR_VIEW if !self.busy => new_conversation(),
                ACTION_CURSOR_LEFT => self.caret = self.caret.saturating_sub(1),
                ACTION_CURSOR_RIGHT => self.caret = (self.caret + 1).min(self.inp_n),
                ACTION_LINE_START => self.caret = 0,
                ACTION_LINE_END => self.caret = self.inp_n,
                ACTION_PAGE_UP => self.scroll_by(page),
                ACTION_PAGE_DOWN => self.scroll_by(-page),
                ACTION_TOP => self.scroll_by(i32::MAX / 2),
                ACTION_BOTTOM => self.scroll = 0,
                ACTION_COPY => self.copy(),
                ACTION_PASTE => self.paste(),
                _ => {}
            },
            INPUT_EV_WHEEL => self.scroll_by(p as u8 as i8 as i32 * 3),
            INPUT_EV_WIN_RESIZE => {
                self.w = (((p >> 16) & 0xffff) as i32).clamp(64, MAX_W);
                self.h = ((p & 0xffff) as i32).clamp(64, MAX_H);
                self.layout_dirty = true;
            }
            _ => {}
        }
    }

    fn key(&mut self, k: u8) {
        match k {
            b'\n' => {}
            0x08 | 0x7f => {
                if self.caret > 0 {
                    self.inp.copy_within(self.caret..self.inp_n, self.caret - 1);
                    self.caret -= 1;
                    self.inp_n -= 1;
                }
            }
            0x0b if !self.busy => new_conversation(),
            KEY_ESC => {
                if self.busy {
                    self.cancel = true;
                }
            }
            KEY_CTRL_P => self.select(true),
            KEY_CTRL_N => self.select(false),
            KEY_LEFT => self.caret = self.caret.saturating_sub(1),
            KEY_RIGHT => self.caret = (self.caret + 1).min(self.inp_n),
            KEY_UP => self.scroll_by(1),
            KEY_DOWN => self.scroll_by(-1),
            0x20..=0x7e => {
                if self.inp_n < INP_CAP {
                    self.inp.copy_within(self.caret..self.inp_n, self.caret + 1);
                    self.inp[self.caret] = k;
                    self.caret += 1;
                    self.inp_n += 1;
                }
            }
            _ => {}
        }
    }

    /// Repaint now if something changed and the window is visible.
    fn paint(&mut self) {
        if !self.dirty || unsafe { self.flags.read_volatile() } & 2 != 0 {
            return;
        }
        if self.layout_dirty || self.cols != self.cols_for() {
            self.relayout();
        }
        self.dirty = false;
        self.last_paint = now_ms();
        let s = Surf { p: self.surf, w: self.w, h: self.h };
        let (rows, maxs) = render(self, &s);
        self.rows_vis = rows;
        self.max_scroll = maxs;
        sys(SYS_WIN_PRESENT, self.win as u64, 0, 0, 0);
    }

    /// The first turn the context starts at: the newest whole turns within CTX_CAP (at least one).
    fn ctx_start(&self) -> usize {
        let (mut total, mut end, mut start) = (0usize, self.log_n, self.log_n);
        let mut i = self.log_n;
        while i > 0 {
            i -= 1;
            if self.log[i] <= ROLE_NOTE {
                let len = end - i;
                if total + len > CTX_CAP && start != self.log_n {
                    break;
                }
                total += len;
                start = i;
                end = i;
            }
        }
        start
    }
}

/// The conversation as the model sees it: user and assistant turns from the transcript, notes skipped.
#[derive(Clone)]
struct Turns<'a> {
    log: &'a [u8],
    i: usize,
}
impl<'a> Iterator for Turns<'a> {
    type Item = Msg<'a>;
    fn next(&mut self) -> Option<Msg<'a>> {
        while self.i < self.log.len() {
            let role = self.log[self.i];
            let mut e = self.i + 1;
            while e < self.log.len() && self.log[e] > ROLE_NOTE {
                e += 1;
            }
            let text = &self.log[self.i + 1..e];
            self.i = e;
            let role = match role {
                ROLE_USER => Role::User,
                ROLE_AI => Role::Assistant,
                _ => continue,
            };
            if let Ok(t) = core::str::from_utf8(text) {
                return Some(Msg { role, text: t });
            }
        }
        None
    }
}

/// vein_ring3's wait hook: keep the window alive while the network waits.
fn tick() -> bool {
    let a = app();
    let mut k = 0;
    while k < 16 {
        let ev = sys(SYS_INPUT_POLL, 0, 0, 0, 0);
        if ev < 0 {
            break;
        }
        a.event(ev as u64);
        k += 1;
    }
    if now_ms().wrapping_sub(a.last_paint) >= 33 {
        a.paint();
    }
    !a.cancel
}

// ---- M4: history ----------------------------------------------------------------------------------
/// The file path of conversation `seq` for the probed surface (absolute on PATH, home-relative on OPEN).
fn hist_path(seq: u32, out: &mut [u8; 300]) -> usize {
    let mut rel = [0u8; history::PATH_LEN];
    let rn = history::path(seq, &mut rel);
    let mut n = 0;
    if app().hist.f == Files::Path {
        let h = home();
        out[..h.n].copy_from_slice(&h.b[..h.n]);
        n = h.n;
        if n == 0 || out[n - 1] != b'/' {
            out[n] = b'/';
            n += 1;
        }
    }
    out[n..n + rn].copy_from_slice(&rel[..rn]);
    n + rn
}

fn hist_exists(seq: u32) -> bool {
    let mut pb = [0u8; 300];
    let pn = hist_path(seq, &mut pb);
    files::exists(app().hist.f, &pb[..pn])
}

fn hist_fail(op: &[u8], code: i64) {
    let a = app();
    let mut l = Line::new(b":: LUMEN: history ");
    l.put(op).put(b" code=").dec(code).put(b" ::");
    l.wire();
    if !a.hist.failed {
        a.hist.failed = true;
        let mut w = Line::new(b"history: ");
        w.put(op).put(b" failed (").dec(code).put(b"); this conversation is not being saved");
        a.note(w.s());
    }
}

/// Reload conversation `seq` into the transcript (its newest FILE_CAP bytes).
fn hist_load(seq: u32) -> bool {
    let a = app();
    let mut pb = [0u8; 300];
    let pn = hist_path(seq, &mut pb);
    let path = &pb[..pn];
    let fb = filebuf();
    let r = files::read(a.hist.f, path, 0, fb);
    if r < 0 {
        hist_fail(b"load", r);
        return false;
    }
    let (mut size, mut got) = (r as u64, r as usize);
    if got == FILE_CAP {
        let sz = files::size(a.hist.f, path, &mut fb[..32 * 1024]);
        if sz > 0 {
            size = sz as u64;
            let r2 = files::read(a.hist.f, path, size - FILE_CAP as u64, fb);
            got = r2.max(0) as usize;
        }
    }
    a.clear();
    let mut first = true;
    history::parse(&fb[..got], &mut |e| match e {
        history::Ev::Begin(w) => {
            a.turn(
                match w {
                    Who::User => ROLE_USER,
                    Who::Assistant => ROLE_AI,
                    Who::Note => ROLE_NOTE,
                },
                b"",
            );
            first = true;
        }
        history::Ev::Line(l) => {
            if !first {
                a.add(b"\n");
            }
            a.add(l);
            first = false;
        }
    });
    a.turn_open = false;
    a.ai_start = None;
    a.hist.seq = seq;
    a.hist.size = size;
    a.hist.created = true;
    a.hist.failed = false;
    true
}

/// Append one finished turn to the conversation file (creating it, with its header, on the first).
fn hist_append(who: Who, text_at: (usize, usize)) {
    let a = app();
    if matches!(a.hist.f, Files::None(_)) || a.hist.failed {
        return;
    }
    let ob = unsafe { &mut *core::ptr::addr_of_mut!(OUTBUF) };
    let n = {
        let mut o = Out::new(ob);
        if !a.hist.created {
            history::header(a.hist.seq, now_ms(), &mut o);
        }
        history::turn(who, &a.log[text_at.0..text_at.1], &mut o);
        o.done()
    };
    let Some(n) = n else {
        hist_fail(b"turn-too-long", una_abi::ERANGE);
        return;
    };
    let mut pb = [0u8; 300];
    let pn = hist_path(a.hist.seq, &mut pb);
    let req = unsafe { &mut *core::ptr::addr_of_mut!(REQ) };
    let r = files::append(a.hist.f, &pb[..pn], a.hist.size, !a.hist.created, &ob[..n], req);
    if r != n as i64 {
        hist_fail(b"append", if r < 0 { r } else { una_abi::EIO });
        return;
    }
    a.hist.size += n as u64;
    a.hist.created = true;
    a.hist.max = a.hist.max.max(a.hist.seq);
}

fn hist_init() {
    let a = app();
    a.hist.f = files::probe(home());
    if matches!(a.hist.f, Files::None(_)) {
        return;
    }
    let mut max = 0;
    while max < history::SEQ_MAX && hist_exists(max + 1) {
        max += 1;
    }
    a.hist.max = max;
    if max > 0 && hist_load(max) {
        return;
    }
    a.hist.seq = max + 1;
}

fn new_conversation() {
    let a = app();
    if a.busy {
        return;
    }
    a.clear();
    if a.hist.created {
        a.hist.seq = a.hist.max.max(a.hist.seq) + 1;
    }
    a.hist.size = 0;
    a.hist.created = false;
    a.hist.failed = false;
    let mut l = Line::new(b"new conversation");
    if !matches!(a.hist.f, Files::None(_)) {
        l.put(b" ").dec(a.hist.seq as i64);
    }
    a.note(l.s());
}

fn list_conversations() {
    let a = app();
    if matches!(a.hist.f, Files::None(_)) {
        a.note(b"history is off: no ring-3 file surface on this kernel");
        return;
    }
    if a.hist.max == 0 {
        a.note(b"no saved conversations yet");
        return;
    }
    a.note(b"conversations (/open N):");
    let lo = a.hist.max.saturating_sub(19).max(1);
    let mut head = [0u8; 512];
    for n in lo..=a.hist.max {
        let mut pb = [0u8; 300];
        let pn = hist_path(n, &mut pb);
        let r = files::read(a.hist.f, &pb[..pn], 0, &mut head);
        if r < 0 {
            continue;
        }
        let t = history::title(&head[..r as usize], 24);
        let mut l = Line::new(if n == a.hist.seq { b"\n* " } else { b"\n  " });
        l.dec(n as i64).put(b"  ").put(if t.is_empty() { b"(empty)" } else { t });
        a.add(l.s());
    }
}

fn command(cmd: &[u8]) {
    let a = app();
    let (verb, arg) = match cmd.iter().position(|&c| c == b' ') {
        Some(p) => (&cmd[..p], &cmd[p + 1..]),
        None => (cmd, &b""[..]),
    };
    match verb {
        b"/new" => new_conversation(),
        b"/list" => list_conversations(),
        b"/copy" => {
            a.sel = None;
            a.copy();
        }
        b"/open" => {
            let n = arg.iter().take_while(|c| c.is_ascii_digit()).fold(0u32, |v, &c| v.saturating_mul(10).saturating_add((c - b'0') as u32));
            if n == 0 || matches!(a.hist.f, Files::None(_)) || !hist_exists(n) {
                a.note(b"no such conversation (/list shows them)");
            } else {
                hist_load(n);
            }
        }
        b"/help" => a.note(b"Enter sends. Up/Down, wheel, Shift+PgUp/PgDn scroll; Cmd/Ctrl+Home/End jump. Ctrl-P/Ctrl-N select a reply, Cmd-C copies it, Cmd-V pastes. Ctrl-K or /new starts over; /list, /open N, /copy."),
        _ => a.note(b"unknown command (/help lists them)"),
    }
}

// ---- the session ----------------------------------------------------------------------------------
/// What the session runs on, decided once at start.
struct Session {
    plan: Plan,
    key: KeyState,
    key_n: usize,
    cfg: vein_ring3::prefs::Config,
    /// Provider + trust store + clock, gathered once (VEINTLS).
    tls: vein_ring3::TlsSetup,
}

impl Session {
    fn provider(&self) -> &'static [u8] {
        match self.plan {
            Plan::Claude { .. } => b"claude",
            Plan::Echo(_) => b"echo",
        }
    }
    fn transport(&self) -> &'static [u8] {
        match (self.plan, self.cfg.endpoint()) {
            (Plan::Claude { .. }, Some(ep)) if ep.tls => b"tls",
            (Plan::Claude { .. }, Some(_)) => b"http",
            _ => b"none",
        }
    }
    /// The status line's transport: TLS says whether the server was verified (never yet: the trust store
    /// is owed — the verified issuer goes here when it lands).
    fn transport_label(&self) -> &'static [u8] {
        match self.transport() {
            b"tls" if vein_ring3::VERIFIES_CERTS => b"tls verified",
            b"tls" => b"tls unverified",
            b"http" => b"http relay",
            _ => b"offline",
        }
    }
    fn model(&self) -> &[u8] {
        match self.plan {
            Plan::Claude { .. } => self.cfg.model().as_bytes(),
            Plan::Echo(_) => vein_core::provider::ECHO_MODEL.as_bytes(),
        }
    }
    fn key(&self) -> Option<&'static str> {
        match self.plan {
            Plan::Claude { send_key: true } if self.key == KeyState::UnaFs => {
                let b = unsafe { &*core::ptr::addr_of!(KEY) };
                vein_core::prefs::key_from_file(&b[..self.key_n])
            }
            _ => None,
        }
    }
}

/// M5: the two status rows — provider / model, then transport, tokens, first-token latency.
fn footer(sess: &Session, first_ms: Option<u64>, tokens: (Option<u32>, Option<u32>)) {
    let mut r1 = Line::new(sess.provider());
    r1.put(b" / ").put(sess.model());
    let mut r2 = Line::new(sess.transport_label());
    if tokens.0.is_some() || tokens.1.is_some() {
        r2.put(b"  in ").opt(tokens.0).put(b" out ").opt(tokens.1);
    }
    if let Some(ms) = first_ms {
        r2.put(b"  1st ").dec(ms as i64).put(b"ms");
    }
    let a = app();
    let (n1, n2) = (r1.n, r2.n);
    a.set_status(&[&r1.b[..n1]], &[&r2.b[..n2]]);
}

fn submit(sess: &Session) {
    let a = app();
    if a.inp_n == 0 || a.busy {
        return;
    }
    let n = a.inp_n;
    let mut tmp = [0u8; INP_CAP];
    tmp[..n].copy_from_slice(&a.inp[..n]);
    a.inp_n = 0;
    a.caret = 0;
    a.scroll = 0;
    a.sel = None;
    if tmp[0] == b'/' {
        command(&tmp[..n]);
        app().paint();
        return;
    }
    a.turn(ROLE_USER, b"");
    let us = a.log_n;
    a.add(&tmp[..n]);
    hist_append(Who::User, (us, app().log_n));
    let a = app();
    a.ai_start = None;
    a.busy = true;
    a.cancel = false;
    a.paint();

    let t0 = now_ms();
    let mut first: Option<u64> = None;
    let mut bytes = 0usize;
    let mut on = |e: Event<'_>| {
        let a = app();
        match e {
            Event::Text(t) => {
                if first.is_none() {
                    first = Some(now_ms().saturating_sub(t0));
                }
                if !a.turn_open {
                    a.turn(ROLE_AI, b"");
                }
                a.add(t.as_bytes());
                bytes += t.len();
            }
            Event::Stop(Stop::Refusal) => a.note(b"the model declined this request (stop_reason refusal)"),
            Event::Stop(Stop::MaxTokens) => a.note(b"answer cut at max_tokens"),
            Event::Stop(_) | Event::Done => {}
            Event::Error(m) => {
                a.note(b"error: ");
                a.add(m.as_bytes());
            }
            Event::Dropped => a.note(b"(a stream line too long for the buffer was skipped)"),
        }
        if now_ms().wrapping_sub(a.last_paint) >= 33 {
            a.paint();
        }
    };

    let mut stop: &[u8] = b"end_turn";
    let mut verified: Option<vein_ring3::Verified> = None;
    let mut tokens: (Option<u32>, Option<u32>) = (None, None);
    match sess.plan {
        Plan::Echo(_) => {
            let text = core::str::from_utf8(&tmp[..n]).unwrap_or("");
            vein_core::provider::echo(text, 12, &mut |p| on(Event::Text(p)));
        }
        Plan::Claude { .. } => {
            let ep = sess.cfg.endpoint().unwrap_or(vein_core::prefs::DEFAULT_ENDPOINT);
            let p = Params::new(sess.cfg.model(), vein_core::claude::DEFAULT_MAX_TOKENS, SYSTEM);
            let bufs = unsafe { &mut *core::ptr::addr_of_mut!(BUFS) };
            let prepared = {
                let a = app();
                let s = a.ctx_start();
                let turns = Turns { log: &a.log[s..a.log_n], i: 0 };
                vein_ring3::prepare(&ep, &p, turns, sess.key(), bufs)
            };
            let ctx = sess.tls.context();
            let r = prepared.and_then(|req| vein_ring3::send(&ep, req, bufs, ctx.as_ref(), &mut on));
            match r {
                Ok(sent) => {
                    let o = sent.out;
                    tokens = (o.input_tokens, o.output_tokens);
                    if let Some(v) = sent.verified {
                        verified = Some(v);
                        let mut l = Line::new(b"transport=tls verified=");
                        l.put(v.issuer().as_bytes()).put(b" ct=").put(v.ct().as_bytes());
                        let n = l.n;
                        app().note(&l.b[..n]);
                    }
                    stop = o.stop.map_or(if o.status == 200 { b"none" as &[u8] } else { b"http-error" }, |s| s.as_str().as_bytes());
                    if o.status != 200 {
                        let mut l = Line::new(b"HTTP ");
                        l.dec(o.status as i64);
                        if let Some(ra) = o.retry_after {
                            l.put(b", retry after ").dec(ra as i64).put(b" s");
                        }
                        let a = app();
                        a.note(l.s());
                    }
                }
                Err(st) => {
                    stop = b"failed";
                    let a = app();
                    let mut l = Line::new(b":: LUMEN: fail stage=");
                    l.put(st.name().as_bytes()).put(b" code=").dec(st.code());
                    if let Some(why) = st.tls_why() {
                        l.put(b" tls=").put(why.as_bytes());
                    }
                    l.put(b" ::");
                    l.wire();
                    if a.cancel {
                        a.note(b"cancelled");
                    } else {
                        let mut w = Line::new(b"could not reach the provider: ");
                        w.put(st.name().as_bytes()).put(b" (").dec(st.code()).put(b")");
                        if let Some(why) = st.tls_why() {
                            w.put(b" ").put(why.as_bytes());
                        }
                        a.note(w.s());
                    }
                }
            }
        }
    }
    let a = app();
    a.busy = false;
    a.turn_open = false;
    if let Some(s) = a.ai_start.take() {
        let e = a.turn_end(s as usize);
        hist_append(Who::Assistant, (s as usize + 1, e));
    }
    footer(sess, first, tokens);
    let mut l = Line::new(b":: LUMEN: reply provider=");
    l.put(sess.provider()).put(b" first_token_ms=").dec(first.map_or(-1, |m| m as i64)).put(b" bytes=").dec(bytes as i64).put(b" stop=").put(stop);
    if let Some(v) = verified {
        l.put(b" transport=tls verified=").put(v.issuer().as_bytes()).put(b" ct=").put(v.ct().as_bytes());
    }
    l.put(b" in=").opt(tokens.0).put(b" out=").opt(tokens.1).put(b" ::");
    l.wire();
    app().paint();
}

// ---- raster ---------------------------------------------------------------------------------------
struct Surf {
    p: *mut u8,
    w: i32,
    h: i32,
}
impl Surf {
    fn fill(&self, x0: i32, y0: i32, w: i32, h: i32, c: u32) {
        let (xs, ys) = (x0.max(0), y0.max(0));
        let (xe, ye) = ((x0 + w).min(self.w), (y0 + h).min(self.h));
        let mut y = ys;
        while y < ye {
            let row = unsafe { self.p.add(y as usize * STRIDE) as *mut u32 };
            let mut x = xs;
            while x < xe {
                unsafe { row.add(x as usize).write_volatile(c) };
                x += 1;
            }
            y += 1;
        }
    }
    fn get(&self, x: i32, y: i32) -> u32 {
        unsafe { (self.p.add(y as usize * STRIDE) as *const u32).add(x as usize).read_volatile() }
    }
    /// Text in one colour with its line box's top at `y` (KERNELFONT2: through the faces when they loaded, the
    /// font8x8 glyph's top row otherwise). Bold = the bold face (font8x8: a double strike), italic = the top half
    /// sheared one pixel right. `fx` is the pen in px; returns the pen after the text.
    fn styled_k(&self, fx: f32, y: i32, s: &[u8], c: u32, k: Kind, italic: bool) -> f32 {
        if let Some(t) = tt() {
            let (w, h) = (self.w, self.h);
            return t.draw(fx, y, s, k, italic, c, &mut |gx, gy, cov| {
                if gx >= 0 && gy >= 0 && gx < w && gy < h {
                    let bg = self.get(gx, gy);
                    let px = 0xFF00_0000 | font_core::ui::blend(bg & 0x00FF_FFFF, c & 0x00FF_FFFF, cov);
                    unsafe { (self.p.add(gy as usize * STRIDE) as *mut u32).add(gx as usize).write_volatile(px) };
                }
            });
        }
        let x = fx as i32;
        let bold = k == Kind::Bold;
        // the font8x8 grid: the 8-row glyph sits one row down in its 10-row box (the pre-KERNELFONT2 placement)
        let y = y + 1;
        for (n, &ch) in s.iter().enumerate() {
            let g = &FONT[(ch & 0x7f) as usize];
            let gx = x + n as i32 * 8;
            for (r, &bits) in g.iter().enumerate() {
                let sh = if italic && r < 4 { 1 } else { 0 };
                let mut col = 0;
                while col < 8 {
                    if bits & (1 << col) != 0 {
                        self.fill(gx + col + sh, y + r as i32, if bold { 2 } else { 1 }, 1, c);
                    }
                    col += 1;
                }
            }
        }
        fx + s.len() as f32 * 8.0
    }
    fn text(&self, x: i32, y: i32, s: &[u8], c: u32) -> f32 {
        self.styled_k(x as f32, y, s, c, Kind::Sans, false)
    }
}

fn tint_ink(t: Tint) -> u32 {
    match t {
        Tint::Plain => INK,
        Tint::Heading => HEAD_INK,
        Tint::Dim => NOTE_INK,
        Tint::Code => CODE_INK,
        Tint::Quote => QUOTE_INK,
        Tint::Link => LINK_INK,
    }
}

/// Draw `text[a..b]` of a rendered line with its box top at `y`, pen at `x`, styled by `spans` (a fence row is
/// all mono).
fn draw_spans(s: &Surf, x: f32, y: i32, text: &[u8], spans: &[Span], a: usize, b: usize, fence: bool) {
    let mut i = a;
    let mut pen = x;
    while i < b {
        let (k, ink, italic, code) = style_at(spans, i, fence);
        let mut e = i + 1;
        while e < b && style_at(spans, e, fence) == (k, ink, italic, code) {
            e += 1;
        }
        if code {
            s.fill(pen as i32, y, width(&text[i..e], k).ceil_px(), lh(), CODE_BG);
        }
        pen = s.styled_k(pen, y, &text[i..e], ink, k, italic);
        i = e;
    }
}

trait CeilPx {
    fn ceil_px(self) -> i32;
}
impl CeilPx for f32 {
    fn ceil_px(self) -> i32 {
        let t = self as i32;
        if (t as f32) < self { t + 1 } else { t }
    }
}

/// Draw the window; returns (visible transcript rows, the largest scroll) for the next event.
fn render(a: &App, s: &Surf) -> (i32, i32) {
    let (w, h) = (a.w, a.h);
    let lh = lh();
    s.fill(0, 0, w, h, BG);
    let tw = (w - 8) as f32; // the text width, px
    // the footer: two status rows
    let fy2 = h - lh - 2;
    let fy1 = fy2 - lh;
    s.fill(0, fy1 - 3, w, 1, RULE);
    let st1 = &a.st1[..a.st1_n];
    s.text(4, fy1, &st1[..fit(st1, Kind::Sans, tw)], OK_INK);
    let (sc, st): (u32, &[u8]) = if a.busy {
        (CARET, b"... answering (Esc cancels)")
    } else if a.flash_n > 0 && now_ms() < a.flash_until {
        (CARET, &a.flash[..a.flash_n])
    } else {
        (NOTE_INK, &a.st2[..a.st2_n])
    };
    s.text(4, fy2, &st[..fit(st, Kind::Sans, tw)], sc);
    // the input: broken into rows of at most `iw` px (by character, not word: it is being typed)
    let iw = (tw - 16.0).max(16.0);
    let mut starts: alloc::vec::Vec<usize> = alloc::vec![0];
    {
        let mut x = 0f32;
        for (i, &c) in a.inp[..a.inp_n].iter().enumerate() {
            let d = adv(c, Kind::Sans);
            if x + d > iw && i > *starts.last().unwrap_or(&0) {
                starts.push(i);
                x = 0.0;
            }
            x += d;
        }
    }
    let total = starts.len();
    let shown = total.min(3);
    let crow = starts.iter().rposition(|&st| st <= a.caret).unwrap_or(0);
    let crow = if crow + 1 < total && starts[crow + 1] == a.caret { crow + 1 } else { crow };
    let top = (crow + 1).saturating_sub(shown);
    let iy = fy1 - 6 - shown as i32 * lh;
    s.fill(0, iy - 3, w, 1, RULE);
    let mut r = 0;
    while r < shown {
        let row = top + r;
        let y = iy + r as i32 * lh;
        if row == 0 {
            s.text(4, y, b">", USER_INK);
        }
        let lo = starts[row].min(a.inp_n);
        let hi = starts.get(row + 1).copied().unwrap_or(a.inp_n).min(a.inp_n);
        s.text(4 + 16, y, &a.inp[lo..hi], INK);
        if row == crow {
            let cx = 4 + 16 + width(&a.inp[lo..a.caret.clamp(lo, hi)], Kind::Sans) as i32;
            s.fill(cx, y, 2, lh, CARET);
        }
        r += 1;
    }
    // the transcript: the ring's rows, scrolled back `scroll` from the bottom
    let rows = ((iy - 6 - 4) / lh).max(1) as usize;
    let n = a.ring.len();
    let maxs = n.saturating_sub(rows);
    let sc = (a.scroll.max(0) as usize).min(maxs);
    let first = n - rows.min(n) - sc;
    let (mut cache_src, mut cache_len, mut cache_spans) = (u32::MAX, 0usize, 0usize);
    let mut i = first;
    while i < n && i < first + rows {
        let Some(rec) = a.ring.get(i) else { break };
        let y = 3 + (i - first) as i32 * lh;
        let p = rec.src as usize;
        let selected = a.sel.is_some_and(|(ss, se)| rec.role == ROLE_AI && ss < rec.src && rec.src < se);
        match rec.role {
            0 => {}
            ROLE_AI => {
                if cache_src != rec.src {
                    let e = a.line_end(p);
                    let mut st = State { fence: rec.flags & F_FENCE != 0 };
                    let l = md::line(&mut st, &a.log[p..e], rt(), rs());
                    (cache_src, cache_len, cache_spans) = (rec.src, l.len, l.spans);
                }
                let block = Block::from_u8(rec.block);
                let fence = matches!(block, Block::Code | Block::Fence);
                if fence {
                    s.fill(2, y, w - 8, lh, CODE_BG);
                }
                if block == Block::Rule {
                    s.fill(4, y + lh / 2, w - 12, 1, RULE);
                } else {
                    let text = &rt()[..cache_len];
                    let spans = &rs()[..cache_spans];
                    let a0 = (rec.dofs as usize).min(cache_len);
                    let b0 = (a0 + rec.len as usize).min(cache_len);
                    let hang = (rec.hang as usize).min(cache_len);
                    let hx: f32 = (0..hang).map(|k| adv(text[k], style_at(spans, k, fence).0)).sum();
                    draw_spans(s, 4.0 + hx, y, text, spans, a0, b0, fence);
                }
            }
            role => {
                let e = a.line_end(p);
                let a0 = (p + rec.dofs as usize).min(e);
                let b0 = (a0 + rec.len as usize).min(e);
                let (ink, mark): (u32, &[u8]) = if role == ROLE_USER { (USER_INK, b">") } else { (NOTE_INK, b"*") };
                if rec.flags & F_TURN != 0 && rec.flags & F_LEAD != 0 {
                    s.text(4, y, mark, ink);
                }
                s.text(4 + 16, y, &a.log[a0..b0], ink);
            }
        }
        if selected {
            s.fill(0, y, 2, lh, SEL_BAR);
        }
        i += 1;
    }
    // M1: the position indicator — a scrollbar on the right edge when there is more than a screen
    if n > rows {
        let track = rows as i32 * lh;
        let thumb = (track * rows as i32 / n as i32).max(6);
        let ty = 3 + ((track - thumb) as i64 * first as i64 / maxs.max(1) as i64) as i32;
        s.fill(w - 3, 3, 2, track, RULE);
        s.fill(w - 3, ty, 2, thumb, CARET);
    }
    (rows as i32, maxs as i32)
}

/// The window landmarks: the RO info page at base + 0x4000, surface slot 0 at base + 0x5000. An
/// elf-model program finds them at the FIXED slot base (RING3WIN), not at its own `_start`.
#[cfg(target_arch = "x86_64")]
fn landmark_base() -> u64 {
    una_abi::USER_BASE_X86
}
/// RING3ABI2 M5 (B333): the aarch64 LUMEN.ELF is an elf-model image in the extension GiB, and the classic
/// window has no fixed VA on aarch64 — the args page carries its base (`window_base`).
#[cfg(not(target_arch = "x86_64"))]
fn landmark_base() -> u64 {
    match una_abi::args() {
        Some(a) if a.window_base() != 0 => a.window_base(),
        _ => {
            Line::new(b":: LUMEN: no args page (window base unknown) ::").wire();
            exit(1)
        }
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let _ = &APP_NOTE;
    let base = landmark_base();
    let win = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0, 0);
    if win < 0 {
        Line::new(b":: LUMEN: SYS_WIN_CREATE refused ::").wire();
        exit(1);
    }
    {
        let a = app();
        a.ring = Ring::new(unsafe { &mut *core::ptr::addr_of_mut!(RECS) });
        a.log = unsafe { &mut *core::ptr::addr_of_mut!(LOG) };
        a.win = win;
        (a.w, a.h, a.dirty) = (MAX_W, MAX_H, true);
        a.surf = (base + 0x5000) as *mut u8;
        a.flags = unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4) }; // VUGMIN: bit 1 = all hidden
        a.set_status(&[b"reading preferences ..."], &[]);
        a.paint();
    }

    // The session: Principia's `vein` namespace, the key file, the rule.
    let cfg = vein_ring3::prefs::Config::read();
    // SETTINGSFILES (B407, R98): Lumen's own settings stanza, declared once — `app.lumen.*` in `<home>/settings/lumen`.
    let declared = vein_ring3::prefs::declare(vein_ring3::prefs::LUMEN_STANZA);
    unsafe { DECLARED = declared.err().unwrap_or(0) };
    let keybuf = unsafe { &mut *core::ptr::addr_of_mut!(KEY) };
    let mut kpath = [0u8; 128];
    // HOLOCRON2 M2 (B355): Holocron first (`vein/claude.api_key`, keysource::decide); the key file only on
    // NotFound or no fulfiller; LOCKED / DENIED / CORRUPT refuse and the window names the fix.
    let hk = vein_ring3::holocron::claude_key(keybuf);
    let (key, key_n) = match hk {
        vein_ring3::holocron::KeyFrom::Holocron(n) => (KeyState::UnaFs, n),
        vein_ring3::holocron::KeyFrom::Refuse(_) => (KeyState::None, 0),
        vein_ring3::holocron::KeyFrom::Fallback(_) => {
            let kfile = match cfg.key_file() { Some(k) => Some(k), None => vein_ring3::key::default_path(&mut kpath) }; // RING3ABI2 M3 (B333): unset preference = `<home>/.config/unaos/vein.key`, `<home>` from SYS_WHOAMI
            vein_ring3::key::read(kfile, keybuf)
        }
    };
    let tls = vein_ring3::TlsSetup::load(); // VEINTLS (SR36)
    let plan = cfg.plan(key, tls.verify());
    let sess = Session { plan, key, key_n, cfg, tls };
    vein_ring3::net::set_tick(Some(tick));

    // M4: the newest conversation comes back.
    hist_init();

    // KERNELFONT2 (B363): the faces on the volume, into the heap the 64 MiB window holds; else the font8x8 grid.
    let font_why = match txt::load() {
        Ok(t) => {
            unsafe { *core::ptr::addr_of_mut!(TXT) = Some(t) };
            let a = app();
            (a.layout_full, a.layout_dirty, a.dirty) = (true, true, true);
            None
        }
        Err(w) => Some(w),
    };

    let mut l = Line::new(b":: LUMEN: start provider=");
    l.put(sess.provider()).put(b" model=").put(sess.model()).put(b" key=").put(match hk { vein_ring3::holocron::KeyFrom::Holocron(_) => b"holocron" as &[u8], _ => key.as_str().as_bytes() }).put(b" transport=").put(sess.transport());
    match hk { // HOLOCRON2 M2 (B355): why the key did not come from Holocron
        vein_ring3::holocron::KeyFrom::Fallback(w) => { l.put(b" holocron=").put(w.as_bytes()); }
        vein_ring3::holocron::KeyFrom::Refuse(_) => { l.put(b" holocron=refused"); }
        vein_ring3::holocron::KeyFrom::Holocron(_) => {}
    }
    match sess.tls.report {
        Some(r) => l.put(b" trust=").dec(r.loaded as i64),
        None => l.put(b" trust=none"),
    };
    l.put(if vein_ring3::clock::is_set() { b" clock=set" as &[u8] } else { b" clock=unset" });
    if sess.tls.provider.is_none() {
        l.put(b" crypto=").put(sess.tls.provider_why.as_bytes());
    }
    {
        let a = app();
        l.put(b" history=");
        if matches!(a.hist.f, Files::None(_)) {
            l.put(b"off");
        } else {
            l.dec(a.hist.seq as i64);
        }
        l.put(b" files=").put(a.hist.f.as_str().as_bytes());
        if let Files::None(e) = a.hist.f {
            l.put(b" files_code=").dec(e);
        }
    }
    if sess.cfg.bus_err != 0 {
        l.put(b" prefs=").dec(sess.cfg.bus_err);
    }
    l.put(b" declared=").dec(unsafe { DECLARED }); // SETTINGSFILES (B407): 0 = app.lumen held by Principia
    match (tt(), &font_why) {
        (Some(t), _) => {
            l.put(b" font=").put(t.name().as_bytes()).put(b" font_kib=").dec((t.bytes / 1024) as i64);
        }
        (None, Some(w)) => {
            l.put(b" font=font8x8 font_why=");
            w.put(&mut |s, c| {
                l.put(s);
                if let Some(c) = c {
                    l.put(b" font_code=").dec(c);
                }
            });
        }
        (None, None) => {
            l.put(b" font=font8x8");
        }
    }
    l.put(b" ::");
    l.wire();

    {
        let a = app();
        if a.log_n == 0 {
            a.note(b"Lumen on UnaOS. Enter sends, Ctrl-K starts over, Esc cancels, /help for the rest.");
        } else {
            let mut m = Line::new(b"conversation ");
            m.dec(a.hist.seq as i64).put(b" reloaded; Ctrl-K or /new starts a new one.");
            a.note(m.s());
        }
        match sess.plan {
            Plan::Echo(r) => {
                a.note(b"offline: the echo provider answers. ");
                a.add(r.text().as_bytes());
            }
            Plan::Claude { send_key } => {
                if send_key && sess.transport() == b"tls" {
                    a.note(b"TLS 1.3, every server verified against /system/trust/roots.pem; the key goes only over a verified connection.");
                } else if !send_key {
                    a.note(b"relay endpoint: the key stays on the relay.");
                }
            }
        }
        if key == KeyState::OnFat {
            a.note(KeyState::OnFat.as_str().as_bytes());
        }
        match hk { // HOLOCRON2 M2 (B355): the window names where the key came from
            vein_ring3::holocron::KeyFrom::Holocron(_) => a.note(b"key: from Holocron (vein/claude.api_key)."),
            vein_ring3::holocron::KeyFrom::Refuse(why) => { a.note(b"key: Holocron refused: "); a.add(why.as_bytes()); }
            vein_ring3::holocron::KeyFrom::Fallback(w) if key == KeyState::UnaFs => { a.note(b"key: from the key file (Holocron: "); a.add(w.as_bytes()); a.add(b")."); }
            vein_ring3::holocron::KeyFrom::Fallback(_) => {}
        }
        if let Files::None(_) = a.hist.f {
            a.note(b"history off: this kernel offers ring 3 no file surface (SYS_PATH_* or the storage service).");
        }
    }
    footer(&sess, None, (None, None));

    loop {
        let mut send = false;
        {
            let a = app();
            let mut k = 0;
            while k < 16 {
                let ev = sys(SYS_INPUT_POLL, 0, 0, 0, 0);
                if ev < 0 {
                    break;
                }
                if input_ev_type(ev as u64) == INPUT_EV_KEY_DOWN && input_ev_payload(ev as u64) as u8 == b'\n' {
                    send = true;
                }
                a.event(ev as u64);
                k += 1;
            }
        }
        if send {
            submit(&sess);
        }
        let a = app();
        if a.flash_n > 0 && now_ms() >= a.flash_until {
            a.flash_n = 0;
            a.dirty = true;
        }
        a.paint();
        sleep_ms(16);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: LUMEN: panic ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): this program's launch declaration — it opens a window (SYS_WIN_CREATE), so a bare `lumen` detaches like `bg`.
/// Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_WINDOWED);

/// SETTINGSFILES (rmbp-ledger B407): the PrefDeclare status of Lumen's stanza (0 = declared).
static mut DECLARED: i64 = 0;

// DRAGDROP2 (rmbp-ledger B470) — Lumen is the first ring-3 drop participant: a file dropped on its window arrives
// as `INPUT_EV_DROP`; the paths come over `BUS_VERB_DROP_GET` and land in the input line as `open <path> ...`
// (`:: LUMEN: drop n=<n> first=<path> ::`, or `:: LUMEN: drop fail code=<n> ::`).
impl App {
    fn dropped(&mut self, payload: u64) {
        let (token, _) = una_abi::drop_ev_parse(payload);
        let mut body = [0u8; 2048];
        let n = match vein_ring3::drop::get(token, &mut body) {
            Ok(n) => n,
            Err(e) => {
                let mut l = Line::new(b":: LUMEN: drop fail code=");
                l.dec(e).put(b" ::");
                l.wire();
                self.flash(&[b"drop refused"]);
                return;
            }
        };
        let mut count = 0i64;
        let mut first: &[u8] = b"";
        let put = |c: u8, a: &mut App| {
            if a.inp_n < INP_CAP {
                a.inp.copy_within(a.caret..a.inp_n, a.caret + 1);
                a.inp[a.caret] = c;
                a.caret += 1;
                a.inp_n += 1;
            }
        };
        for path in una_abi::drop_paths(&body[..n]) {
            if count == 0 {
                first = path;
                for &c in b"open" {
                    put(c, self);
                }
            }
            put(b' ', self);
            for &c in path {
                if (0x20..0x7f).contains(&c) {
                    put(c, self);
                }
            }
            count += 1;
        }
        let mut l = Line::new(b":: LUMEN: drop n=");
        l.dec(count).put(b" first=").put(first).put(b" ::");
        l.wire();
    }
}
