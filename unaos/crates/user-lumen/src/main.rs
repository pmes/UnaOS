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
// LUMEN2 (LEDGER SR121): Ctrl-N starts a new conversation, Ctrl-K searches the conversations (typed into the
// sidebar's find row; Up/Down pick, Enter opens, Ctrl-K again searches the files' text, Esc closes), Ctrl-B
// shows / hides the conversations sidebar, Ctrl-P / Ctrl-O select the previous / next reply, Ctrl-Y copies the
// last code block of the selected reply. Cmd-C copies the selected reply (default the last), Cmd-V pastes
// (SYS_CLIP_GET, the clipboard verb). Cmd-K (the desktop's ClearView action) starts a new conversation. Esc
// cancels an answer in flight (so does the footer's [stop]) and NEVER closes the window (R24). Ctrl+Enter is
// not deliverable: the HID fold suppresses Ctrl with a non-letter (drivers/xhci/mod.rs hid_key_ascii), so Enter
// alone sends. Commands: /new /list /open N /rename T /delete /model [M] /system [T|-] /search Q /copy
// /copycode /help.
//
// LUMEN2 — THE CHAT A PERSON USES. Replies STREAM (vein_core's incremental SSE decode, Claude's stream or
// Gemini's: the conversation's model picks the dialect through vein_core::models) and are drawn as the tokens
// arrive; a conversations SIDEBAR lists the files LUMENUX's codec defines (`<home>/.config/unaos/lumen/NNNN.md`,
// vein_core::convo), with new / switch / rename / delete-to-trash (RING3RENAME's PATH_W_RENAME, B561, into
// `<home>/.Trash/lumen-NNNN.md`); code blocks are highlighted through syntax_core (SR89's grammars) with a
// [copy] verb on the fence row; tables, task lists and lists render through vein_core::md; the footer shows the
// context window (`ctx used/limit`, the model table); the model and system prompt are per conversation
// (`<!-- lumen:set model|system -->` in its file) over Principia's defaults (`vein.model`, `vein.system_prompt`,
// B317). THE POINTER (B543/B550): INPUT_EV_MOUSE_ABS is the system pointer in window px — the hand over links,
// the sidebar and the footer buttons, the text beam over the input; a click on a link opens it in Aether
// (SYS_SPAWN_PATH /apps/AETHER.ELF <url>).
//
// THEME. UNAKEYS (rmbp-ledger B552): every colour is a field of `theme::Theme` (theme.rs: Principia's
// `system.appearance.*` over the PREFS bus, the kernel palette's Light/Dark rows), read once at start and again when
// the store moves (`theme::due`, PrefChanged's ring-3 stand-in); `:: LUMEN: theme mode=<light|dark> accent=<name>
// bus=<0|-errno> ::` at start and on every change.
//
// WIRE. `:: LUMEN: start provider=<claude|echo> model=<m> key=<holocron|unafs|none|fat-refused> [holocron=<no-fulfiller|not-found|refused>] transport=<tls|http|none>
// trust=<roots|none> clock=<set|unset> history=<N|off> files=<path|open|off> font=<dejavu-sans font_kib=<n>|font8x8 font_why=<w>> ::` once (KERNELFONT2), then per answer `:: LUMEN: reply provider=<p>
// first_token_ms=<n> bytes=<n> stop=<s> [transport=tls verified=<issuer CN>] in=<n> out=<n> ::` or `:: LUMEN: fail stage=<s>
// code=<n> [tls=<why>] ::`; `:: LUMEN: clip set=<n> ::` per copy; history failures as `:: LUMEN: history <op> code=<n> ::`.
// The window's footer shows the provider and the first token's latency.
// LUMEN2 (SR121) adds: the start line ends `convs=<n> system=<principia|builtin>`; the reply line ends
// `model=<m> ctx=<used>/<limit>` (provider=gemini for a Gemini conversation); `:: LUMEN: convo new seq=<n> ::`,
// `:: LUMEN: convo switch seq=<n> turns_bytes=<n> ::`, `:: LUMEN: convo trash seq=<n> rc=<0|-errno> to=<path> ::`,
// `:: LUMEN: convo rename seq=<n> title_len=<n> ::`, `:: LUMEN: convo search deep=1 q_len=<n> hits=<n> ::`,
// `:: LUMEN: clip set=<n> what=code ::`, `:: LUMEN: link open rc=<pid|-errno> url=<u> ::`, and once
// `:: LUMEN: gemini key=<unafs|none|fat-refused> ::` when a Gemini conversation first needs its key.

use una_abi::{
    input_ev_payload, input_ev_type, input_ev_xy, INPUT_EV_ACTION, INPUT_EV_BUTTON, INPUT_EV_KEY_DOWN, INPUT_EV_MOUSE_ABS, INPUT_EV_WHEEL, INPUT_EV_WIN_RESIZE, INPUT_EV_CLOSE_REQ, KEY_DOWN, KEY_ESC,
    KEY_LEFT, KEY_RIGHT, KEY_UP, SYS_EXIT, SYS_INPUT_POLL, SYS_WIN_CREATE, SYS_WIN_PRESENT,
};
use vein_core::claude::{Dialect, Event, Msg, Params, Stop};
use vein_core::history::{self, Setting, Who};
use vein_core::md::{self, Block, Span, State, Tint};
use vein_core::{convo, gemini, models};
use vein_core::prefs::{KeyState, Plan};
use vein_core::scroll::{Rec, Ring, F_FENCE, F_LEAD, F_TURN};
use vein_core::{Out, Role};
use vein_ring3::files::{self, Files, Home};
use vein_ring3::sys::{now_ms, sleep_ms, sys, write};

extern crate alloc;

mod hl;
mod side;
mod theme;
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
const KEY_CTRL_B: u8 = 0x02;
const KEY_CTRL_K: u8 = 0x0b;
const KEY_CTRL_N: u8 = 0x0e;
const KEY_CTRL_O: u8 = 0x0f;
const KEY_CTRL_P: u8 = 0x10;
const KEY_CTRL_Y: u8 = 0x19;

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
    (if fence { Kind::Mono } else { Kind::Sans }, theme::cur().ink, false, false)
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
    /// LUMEN2: the system pointer in window px (INPUT_EV_MOUSE_ABS, B543) and the button mask.
    ptr: (i32, i32),
    buttons: u8,
    /// LUMEN2: the conversations sidebar and its find row.
    side: side::Side,
    /// LUMEN2: this conversation's own model / system prompt / title (empty = Principia's default), from its file.
    conv_model: [u8; 64],
    conv_model_n: usize,
    conv_system: [u8; SYS_CAP],
    conv_system_n: usize,
    conv_title: [u8; convo::TITLE_MAX],
    conv_title_n: usize,
    /// LUMEN2: the last reply's reported usage (input + output tokens) — the context indicator's `used`.
    usage: Option<u32>,
    /// LUMEN2: where the last paint put things (the pointer's hit test reads it).
    geo: Geo,
}

/// LUMEN2: the last paint's layout, for the pointer.
#[derive(Clone, Copy)]
struct Geo {
    /// The first ring row drawn, how many, the line box.
    first: usize,
    rows: usize,
    lh: i32,
    /// The input's top rule, the two footer rows.
    iy: i32,
    fy1: i32,
    fy2: i32,
    /// Left edges of the footer's [chats] and [stop] buttons.
    chats_x: i32,
    stop_x: i32,
}
const GEO0: Geo = Geo { first: 0, rows: 0, lh: 10, iy: 0, fy1: 0, fy2: 0, chats_x: i32::MAX, stop_x: i32::MAX };
/// A conversation's system prompt, at most.
const SYS_CAP: usize = 2048;
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
    ptr: (-1, -1),
    buttons: 0,
    side: side::Side::new(),
    conv_model: [0; 64],
    conv_model_n: 0,
    conv_system: [0; SYS_CAP],
    conv_system_n: 0,
    conv_title: [0; convo::TITLE_MAX],
    conv_title_n: 0,
    usage: None,
    geo: GEO0,
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
        hl::reset(); // LUMEN2: the highlight cache holds transcript offsets
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
        hl::reset();
        self.usage = None;
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
        let mut t: &[u8] = &self.log[s as usize + 1..e as usize];
        while t.last() == Some(&b'\n') {
            t = &t[..t.len() - 1];
        }
        let t: &[u8] = unsafe { &*(t as *const [u8]) }; // the transcript is a static; `clip` borrows self mutably
        self.clip(t, b"");
    }

    /// Put `t` on the clipboard (cut at a line inside CLIP_CAP), say it on the wire and in the footer.
    /// `what` names a non-reply copy on the wire (` what=code`).
    fn clip(&mut self, mut t: &[u8], what: &[u8]) {
        let full = t.len();
        if t.len() > una_abi::CLIP_CAP {
            let cut = t[..una_abi::CLIP_CAP].iter().rposition(|&c| c == b'\n').unwrap_or(una_abi::CLIP_CAP);
            t = &t[..cut];
        }
        let r = sys(una_abi::SYS_CLIP_SET, t.as_ptr() as u64, t.len() as u64, 0, 0);
        let mut l = Line::new(b":: LUMEN: clip set=");
        l.dec(r);
        if !what.is_empty() {
            l.put(b" what=").put(what);
        }
        l.put(b" ::");
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

    /// One input event. While an answer streams, Enter does nothing and Esc (or [stop]) cancels.
    fn event(&mut self, ev: u64) {
        let p = input_ev_payload(ev);
        let was_dirty = self.dirty;
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
            INPUT_EV_WHEEL => {
                let d = p as u8 as i8 as i32;
                if self.side.open && self.ptr.0 >= 0 && self.ptr.0 < side::SIDE_W {
                    let g = side::geom(self.geo.lh, self.geo.iy - 6);
                    self.side.scroll_by(d * 3, g.rows);
                } else {
                    self.scroll_by(d * 3);
                }
            }
            INPUT_EV_MOUSE_ABS => {
                self.ptr = input_ev_xy(p);
                self.dirty = was_dirty; // a move alone repaints nothing (the sprite is the compositor's)
                self.hover();
            }
            INPUT_EV_BUTTON => {
                let (m, was) = (p as u8, self.buttons);
                self.buttons = m;
                if m & 1 != 0 && was & 1 == 0 {
                    self.click();
                }
            }
            INPUT_EV_WIN_RESIZE => {
                self.w = (((p >> 16) & 0xffff) as i32).clamp(64, MAX_W);
                self.h = ((p & 0xffff) as i32).clamp(64, MAX_H);
                self.layout_dirty = true;
            }
            _ => {}
        }
    }

    fn insert(&mut self, k: u8) {
        if self.inp_n < INP_CAP {
            self.inp.copy_within(self.caret..self.inp_n, self.caret + 1);
            self.inp[self.caret] = k;
            self.caret += 1;
            self.inp_n += 1;
        }
    }

    fn key(&mut self, k: u8) {
        // LUMEN2: while the find row is open, typing searches the conversations.
        if self.side.search.is_some() {
            self.search_key(k);
            return;
        }
        match k {
            b'\n' => {}
            0x08 | 0x7f => {
                if self.caret > 0 {
                    self.inp.copy_within(self.caret..self.inp_n, self.caret - 1);
                    self.caret -= 1;
                    self.inp_n -= 1;
                }
            }
            KEY_CTRL_N if !self.busy => new_conversation(),
            KEY_CTRL_K if !self.busy => open_search(),
            KEY_CTRL_B => {
                self.side.open = !self.side.open;
                self.side.by_search = false;
            }
            KEY_ESC => {
                if self.busy {
                    self.cancel = true;
                } else if self.side.open {
                    self.side.open = false;
                }
            }
            KEY_CTRL_P => self.select(true),
            KEY_CTRL_O => self.select(false),
            KEY_CTRL_Y => self.copy_code(None),
            KEY_LEFT => self.caret = self.caret.saturating_sub(1),
            KEY_RIGHT => self.caret = (self.caret + 1).min(self.inp_n),
            KEY_UP => self.scroll_by(1),
            KEY_DOWN => self.scroll_by(-1),
            0x20..=0x7e => self.insert(k),
            _ => {}
        }
    }

    /// LUMEN2: a key while the find row is open.
    fn search_key(&mut self, k: u8) {
        let rows = side::geom(self.geo.lh, self.geo.iy - 6).rows.max(1);
        let Some(sr) = self.side.search.as_mut() else { return };
        match k {
            KEY_ESC => {
                self.side.search = None;
                if self.side.by_search {
                    self.side.open = false;
                }
                self.side.scroll = 0;
            }
            b'\n' => {
                let pick = sr.hits[..sr.hn].get(sr.sel).copied();
                self.side.search = None;
                if self.side.by_search {
                    self.side.open = false;
                }
                self.side.scroll = 0;
                if let Some(seq) = pick {
                    switch_to(seq);
                }
            }
            KEY_CTRL_K => deep_search(),
            KEY_UP => sr.sel = sr.sel.saturating_sub(1),
            KEY_DOWN => sr.sel = (sr.sel + 1).min(sr.hn.saturating_sub(1)),
            0x08 | 0x7f => {
                if sr.n > 0 {
                    sr.n -= 1;
                    sr.refilter();
                }
            }
            0x20..=0x7e => {
                if sr.n < sr.q.len() {
                    sr.q[sr.n] = k;
                    sr.n += 1;
                    sr.sel = 0;
                    sr.refilter();
                }
            }
            _ => {}
        }
        // keep the picked row in view
        if let Some(sr) = self.side.search.as_ref() {
            if sr.sel < self.side.scroll {
                self.side.scroll = sr.sel;
            } else if sr.sel >= self.side.scroll + rows {
                self.side.scroll = sr.sel + 1 - rows;
            }
        }
    }

    // ---- LUMEN2: the pointer (B543 position, B550 shape) ------------------------------------------
    /// What is under the pointer.
    fn hit(&self) -> Hit {
        let (x, y) = self.ptr;
        let g = &self.geo;
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return Hit::Outside;
        }
        if y >= g.fy2 && y < g.fy2 + g.lh && x >= g.stop_x && self.busy {
            return Hit::Stop;
        }
        if y >= g.fy1 && y < g.fy1 + g.lh && x >= g.chats_x {
            return Hit::Chats;
        }
        if y >= g.fy1 - 3 {
            return Hit::Footer;
        }
        if y >= g.iy - 3 {
            return Hit::Input;
        }
        if self.side.open && x < side::SIDE_W {
            let sg = side::geom(g.lh, g.iy - 6);
            return Hit::Side(side::spot(&self.side, &sg, x, y));
        }
        // a transcript row
        if y < 3 {
            return Hit::Text;
        }
        let i = g.first + ((y - 3) / g.lh.max(1)) as usize;
        if i >= g.first + g.rows {
            return Hit::Text;
        }
        let Some(rec) = self.ring.get(i) else { return Hit::Text };
        if rec.role == ROLE_AI {
            let block = Block::from_u8(rec.block);
            if block == Block::Fence && rec.flags & F_FENCE == 0 && rec.flags & F_LEAD != 0 && x >= self.w - 8 - copy_w() {
                return Hit::Copy(rec.src);
            }
        }
        if link_at(self, &rec, x) { Hit::Link } else { Hit::Text }
    }

    /// The pointer moved: ask for the shape that answers what is under it (asked every time, as Aether does —
    /// the kernel resets it behind this program's back on a focus change).
    fn hover(&mut self) {
        let shape = match self.hit() {
            Hit::Stop | Hit::Chats | Hit::Copy(_) | Hit::Link => una_abi::CURSOR_HAND,
            Hit::Side(side::Spot::New | side::Spot::Row(_) | side::Spot::Del(_)) => una_abi::CURSOR_HAND,
            Hit::Input | Hit::Side(side::Spot::Find) => una_abi::CURSOR_TEXT,
            _ => una_abi::CURSOR_ARROW,
        };
        let _ = sys(una_abi::SYS_CURSOR_SHAPE, shape, 0, 0, 0);
    }

    /// A left press.
    fn click(&mut self) {
        let now = now_ms();
        match self.hit() {
            Hit::Stop => self.cancel = true,
            Hit::Chats => {
                self.side.open = !self.side.open;
                self.side.by_search = false;
                if !self.side.open {
                    self.side.search = None;
                }
            }
            Hit::Copy(src) => self.copy_code(Some(src)),
            Hit::Link => {
                let (b, n) = unsafe { (&*core::ptr::addr_of!(LINK), LINK_N) };
                open_link(&b[..n]);
            }
            Hit::Side(side::Spot::New) if !self.busy => new_conversation(),
            Hit::Side(side::Spot::Row(seq)) if !self.busy => {
                self.side.search = None;
                if self.side.by_search {
                    self.side.open = false;
                }
                switch_to(seq);
            }
            Hit::Side(side::Spot::Del(seq)) if !self.busy => match self.side.del_armed {
                Some((s, until)) if s == seq && now < until => {
                    self.side.del_armed = None;
                    trash_conversation(seq);
                }
                _ => {
                    self.side.del_armed = Some((seq, now + side::DEL_CONFIRM_MS));
                    self.flash(&[b"click del? again to move this conversation to the Trash"]);
                }
            },
            Hit::Input => {
                // the caret to the clicked column of the input's first shown row (a click elsewhere keeps it)
                let x = (self.ptr.0 - 4 - 16) as f32;
                if x >= 0.0 && self.inp_n > 0 {
                    let k = fit(&self.inp[..self.inp_n], Kind::Sans, x);
                    if self.geo.iy > 0 && self.ptr.1 < self.geo.iy + self.geo.lh {
                        self.caret = k.min(self.inp_n);
                    }
                }
            }
            _ => {}
        }
        self.dirty = true;
        self.hover();
    }

    // ---- LUMEN2: code blocks -----------------------------------------------------------------------
    /// The opening fence line of the code block the line at `p` is inside: the nearest fence line above it in
    /// the same turn (a closed block's closer would have ended the block, so the nearest fence IS the opener).
    fn fence_opener(&self, p: usize) -> Option<usize> {
        let mut q = p;
        let mut guard = 0;
        while q > 0 && guard < 4096 {
            guard += 1;
            if self.log[q - 1] <= ROLE_NOTE {
                return None; // the turn's tag: no opener in this turn
            }
            let le = q - 1; // the '\n' ending the line above
            let mut ls = le;
            while ls > 0 && self.log[ls - 1] != b'\n' && self.log[ls - 1] > ROLE_NOTE {
                ls -= 1;
            }
            if md::fence_info(&self.log[ls..le]).is_some() {
                return Some(ls);
            }
            q = ls;
        }
        None
    }

    /// Copy a code block's body: the block whose fence line starts at `at`, or (None) the last block of the
    /// selected reply (default the last reply).
    fn copy_code(&mut self, at: Option<u32>) {
        let reply = match at {
            Some(src) => self.ai_before(src as usize + 1),
            None => self.sel.or_else(|| self.ai_before(self.log_n)),
        };
        let Some((s, e)) = reply else {
            self.flash(&[b"no reply to copy from"]);
            return;
        };
        let base = s as usize + 1;
        let text: &[u8] = unsafe { &*(&self.log[base..e as usize] as *const [u8]) };
        let mut pick: Option<(usize, usize)> = None;
        md::code_blocks(text, &mut |b| {
            if at.is_none_or(|src| b.open + base == src as usize) {
                pick = Some(b.body);
            }
        });
        match pick {
            Some((a, b)) => self.clip(&text[a..b], b"code"),
            None => self.flash(&[b"no code block in that reply"]),
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
        let (rows, maxs, geo) = render(self, &s);
        self.rows_vis = rows;
        self.max_scroll = maxs;
        self.geo = geo;
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

// ---- M4: history (LUMEN2: the conversations sidebar's index over it) -------------------------------
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

/// Printable ASCII of `v` into `dst` (a newline becomes a space); the length.
fn copy_clean(dst: &mut [u8], v: &[u8]) -> usize {
    let mut n = 0;
    for &c in v.trim_ascii() {
        if n == dst.len() {
            break;
        }
        let c = if c == b'\n' || c == b'\t' { b' ' } else { c };
        if (0x20..0x7f).contains(&c) {
            dst[n] = c;
            n += 1;
        }
    }
    n
}

/// LUMEN2: this conversation's settings from a file's bytes (a block present overrides; absent leaves).
fn take_settings(file: &[u8]) {
    let a = app();
    if let Some(v) = history::setting(file, Setting::Model) {
        a.conv_model_n = copy_clean(&mut a.conv_model, v);
    }
    if let Some(v) = history::setting(file, Setting::System) {
        a.conv_system_n = copy_clean(&mut a.conv_system, v);
    }
    if let Some(v) = history::setting(file, Setting::Title) {
        a.conv_title_n = copy_clean(&mut a.conv_title, v);
    }
}

/// Reload conversation `seq` into the transcript (its newest FILE_CAP bytes) with its settings.
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
    let mut tail = false;
    if got == FILE_CAP {
        let sz = files::size(a.hist.f, path, &mut fb[..32 * 1024]);
        if sz > 0 {
            size = sz as u64;
            let r2 = files::read(a.hist.f, path, size - FILE_CAP as u64, fb);
            got = r2.max(0) as usize;
            tail = size > FILE_CAP as u64;
        }
    }
    a.clear();
    a.conv_model_n = 0;
    a.conv_system_n = 0;
    a.conv_title_n = 0;
    if tail {
        // the settings live at the head (rewrite) or were appended (a long file): read the head's first
        let ob = unsafe { &mut *core::ptr::addr_of_mut!(OUTBUF) };
        let hr = files::read(a.hist.f, path, 0, &mut ob[..8192]);
        if hr > 0 {
            take_settings(&ob[..hr as usize]);
        }
    }
    take_settings(&fb[..got]);
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
    side::index().cur = seq as u16;
    true
}

/// LUMEN2: the conversation's non-empty settings as blocks (a new file's head).
fn write_settings(o: &mut Out<'_>) {
    let a = app();
    for (s, v) in [(Setting::Title, &a.conv_title[..a.conv_title_n]), (Setting::Model, &a.conv_model[..a.conv_model_n]), (Setting::System, &a.conv_system[..a.conv_system_n])] {
        if !v.is_empty() {
            history::set(s, v, o);
        }
    }
}

/// Append one finished turn to the conversation file (creating it, with its header and settings, on the first).
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
            write_settings(&mut o);
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
    let newly = !a.hist.created;
    a.hist.size += n as u64;
    a.hist.created = true;
    a.hist.max = a.hist.max.max(a.hist.seq);
    // LUMEN2: the sidebar learns the conversation at its first saved turn, titled by its rename or first question
    let ix = side::index();
    let seq = a.hist.seq as u16;
    if newly {
        ix.insert(seq);
        ix.cur = seq;
    }
    if ix.title(seq).is_empty() {
        if a.conv_title_n > 0 {
            ix.set_title(seq, &a.conv_title[..a.conv_title_n]);
        } else if who == Who::User {
            let t = &a.log[text_at.0..text_at.1];
            ix.set_title(seq, t.split(|&c| c == b'\n').next().unwrap_or(b""));
        }
    }
}

/// LUMEN2: change one of this conversation's settings in its file — rewritten whole (the blocks at the head)
/// when the file fits the read buffer on the PATH surface, else appended (the last block wins). A conversation
/// not yet saved carries it in memory and its header writes it.
fn hist_put_setting(s: Setting, v: &[u8]) {
    let a = app();
    if !a.hist.created || matches!(a.hist.f, Files::None(_)) || a.hist.failed {
        return;
    }
    let mut pb = [0u8; 300];
    let pn = hist_path(a.hist.seq, &mut pb);
    let path = &pb[..pn];
    let req = unsafe { &mut *core::ptr::addr_of_mut!(REQ) };
    if a.hist.f == Files::Path && a.hist.size as usize <= FILE_CAP {
        let fb = filebuf();
        let r = files::read(a.hist.f, path, 0, fb);
        if r < 0 {
            hist_fail(b"rewrite-read", r);
            return;
        }
        let file = &fb[..r as usize];
        let mut out = alloc::vec![0u8; file.len() + 2 * (SYS_CAP + 256)];
        let Some(n) = ({
            let mut o = Out::new(&mut out);
            history::rewrite(file, &[(s, Some(v))], &mut o);
            o.done()
        }) else {
            hist_fail(b"rewrite-size", una_abi::ERANGE);
            return;
        };
        let w = files::append(a.hist.f, path, 0, true, &out[..n], req);
        if w != n as i64 {
            hist_fail(b"rewrite", if w < 0 { w } else { una_abi::EIO });
            return;
        }
        a.hist.size = n as u64;
    } else {
        let ob = unsafe { &mut *core::ptr::addr_of_mut!(OUTBUF) };
        let Some(n) = ({
            let mut o = Out::new(ob);
            history::set(s, v, &mut o);
            o.done()
        }) else {
            return;
        };
        let w = files::append(a.hist.f, path, a.hist.size, false, &ob[..n], req);
        if w != n as i64 {
            hist_fail(b"append-setting", if w < 0 { w } else { una_abi::EIO });
            return;
        }
        a.hist.size += n as u64;
    }
}

/// One `SYS_PATH_READ` of a directory listing (`PATH_R_LIST`) from `off`.
fn path_list(path: &[u8], off: u64, out: &mut [u8]) -> i64 {
    let mut req = [0u8; una_abi::PATH_IO_HDR_LEN + una_abi::PATH_IO_PATH_MAX];
    if path.is_empty() || path.len() > una_abi::PATH_IO_PATH_MAX {
        return una_abi::EINVAL;
    }
    req[0..2].copy_from_slice(&(path.len() as u16).to_le_bytes());
    req[2..4].copy_from_slice(&una_abi::PATH_R_LIST.to_le_bytes());
    req[8..16].copy_from_slice(&off.to_le_bytes());
    req[16..16 + path.len()].copy_from_slice(path);
    let cap = out.len().min(una_abi::PATH_IO_MAX);
    sys(una_abi::SYS_PATH_READ, req.as_ptr() as u64, (16 + path.len()) as u64, out.as_mut_ptr() as u64, cap as u64)
}

/// RING3RENAME (B561): `SYS_PATH_WRITE` with `PATH_W_RENAME` — `from` as the path, `to` as the data.
fn path_rename(from: &[u8], to: &[u8]) -> i64 {
    let req = unsafe { &mut *core::ptr::addr_of_mut!(REQ) };
    let n = una_abi::PATH_IO_HDR_LEN + from.len() + to.len();
    if from.is_empty() || from.len() > una_abi::PATH_IO_PATH_MAX || to.len() > una_abi::PATH_IO_PATH_MAX || n > req.len() {
        return una_abi::EINVAL;
    }
    req[..16].fill(0);
    req[0..2].copy_from_slice(&(from.len() as u16).to_le_bytes());
    req[2..4].copy_from_slice(&una_abi::PATH_W_RENAME.to_le_bytes());
    req[16..16 + from.len()].copy_from_slice(from);
    req[16 + from.len()..n].copy_from_slice(to);
    sys(una_abi::SYS_PATH_WRITE, req.as_ptr() as u64, n as u64, 0, 0)
}

/// Create `<home>/.Trash` when it is missing: write a placeholder with `PATH_W_MKDIRS`, then unlink it.
fn make_trash_dir(home: &[u8]) -> i64 {
    let mut p = [0u8; 300];
    let n = {
        let mut o = Out::new(&mut p);
        o.put(home);
        if !home.ends_with(b"/") {
            o.put(b"/");
        }
        o.put(convo::TRASH_DIR.as_bytes());
        o.put(b"/.lumen-mkdir");
        match o.done() {
            Some(n) => n,
            None => return una_abi::EINVAL,
        }
    };
    let req = unsafe { &mut *core::ptr::addr_of_mut!(REQ) };
    let mk = files::append(Files::Path, &p[..n], 0, true, b"", req);
    if mk < 0 {
        return mk;
    }
    req[..16].fill(0);
    req[0..2].copy_from_slice(&(n as u16).to_le_bytes());
    req[2..4].copy_from_slice(&una_abi::PATH_W_UNLINK.to_le_bytes());
    req[16..16 + n].copy_from_slice(&p[..n]);
    sys(una_abi::SYS_PATH_WRITE, req.as_ptr() as u64, (16 + n) as u64, 0, 0)
}

fn hist_init() {
    let a = app();
    a.hist.f = files::probe(home());
    if matches!(a.hist.f, Files::None(_)) {
        return;
    }
    let ix = side::index();
    ix.clear();
    // LUMEN2: the index from the directory listing (PATH); else the numbers probed (OPEN, or a kernel without
    // PATH_R_LIST), stopping after eight misses in a row so a trashed conversation leaves no blind spot.
    let mut listed = false;
    if a.hist.f == Files::Path {
        let h = home();
        let mut dir = [0u8; 300];
        if let Some(dn) = convo::dir_path(&h.b[..h.n], &mut dir) {
            let fb = filebuf();
            let mut got = 0usize;
            loop {
                let r = path_list(&dir[..dn], got as u64, &mut fb[got..]);
                if r < 0 {
                    listed = got > 0 || r == una_abi::ENOENT;
                    break;
                }
                if r == 0 {
                    listed = true;
                    break;
                }
                got += r as usize;
                if got >= fb.len() {
                    listed = true;
                    break;
                }
            }
            if listed {
                convo::parse_listing(&fb[..got], &mut |sq| ix.insert(sq));
            }
        }
    }
    if !listed {
        let (mut k, mut miss) = (1u32, 0);
        while k <= history::SEQ_MAX && miss < 8 {
            if hist_exists(k) {
                ix.insert(k as u16);
                miss = 0;
            } else {
                miss += 1;
            }
            k += 1;
        }
    }
    let mut head = [0u8; 2048];
    for i in 0..ix.len() {
        let seq = ix.entries()[i].seq;
        let mut pb = [0u8; 300];
        let pn = hist_path(seq as u32, &mut pb);
        let r = files::read(a.hist.f, &pb[..pn], 0, &mut head);
        if r > 0 {
            let t = history::title_of(&head[..r as usize], convo::TITLE_MAX);
            ix.set_title(seq, t);
        }
    }
    let max = ix.max_seq() as u32;
    a.hist.max = max;
    if max > 0 && hist_load(max) {
        return;
    }
    a.hist.seq = ix.next_seq() as u32;
}

fn new_conversation() {
    let a = app();
    if a.busy {
        return;
    }
    a.clear();
    let ix = side::index();
    a.hist.seq = ix.next_seq() as u32;
    a.hist.size = 0;
    a.hist.created = false;
    a.hist.failed = false;
    (a.conv_model_n, a.conv_system_n, a.conv_title_n) = (0, 0, 0);
    ix.cur = 0; // not in the index until its first turn is saved
    let mut l = Line::new(b"new conversation");
    if !matches!(a.hist.f, Files::None(_)) {
        l.put(b" ").dec(a.hist.seq as i64);
    }
    a.note(l.s());
    let mut w = Line::new(b":: LUMEN: convo new seq=");
    w.dec(a.hist.seq as i64).put(b" ::");
    w.wire();
    footer(None, (None, None));
}

/// LUMEN2: open conversation `seq` from the sidebar, the find row or `/open`.
fn switch_to(seq: u16) {
    let a = app();
    if a.busy || matches!(a.hist.f, Files::None(_)) {
        return;
    }
    if hist_load(seq as u32) {
        let a = app();
        a.scroll = 0;
        a.sel = None;
        let mut w = Line::new(b":: LUMEN: convo switch seq=");
        w.dec(seq as i64).put(b" turns_bytes=").dec(a.log_n as i64).put(b" ::");
        w.wire();
        footer(None, (None, None));
    }
}

/// LUMEN2: delete-to-trash — rename the conversation's file into `<home>/.Trash/lumen-NNNN.md` (RING3RENAME
/// B561, PATH_W_RENAME: the object keeps its id, so the Trash can give it back), then open its neighbour.
fn trash_conversation(seq: u16) {
    let a = app();
    if a.hist.f != Files::Path {
        a.flash(&[b"delete needs the PATH file surface (SYS_PATH_*)"]);
        return;
    }
    if !side::index().contains(seq) {
        a.flash(&[b"nothing saved to delete yet"]);
        return;
    }
    let mut from = [0u8; 300];
    let fnn = hist_path(seq as u32, &mut from);
    let h = home();
    let hb = &h.b[..h.n];
    let mut to = [0u8; 300];
    let (mut r, mut tn, mut made) = (una_abi::EINVAL, 0usize, false);
    for k in [0u32, 2, 3, 4, 5, 6, 7, 8, 9] {
        let Some(n) = convo::trash_path(hb, seq, k, &mut to) else { break };
        tn = n;
        r = path_rename(&from[..fnn], &to[..n]);
        if r == una_abi::ENOENT && !made {
            made = true;
            make_trash_dir(hb);
            r = path_rename(&from[..fnn], &to[..n]);
        }
        if r != una_abi::EEXIST {
            break;
        }
    }
    let mut w = Line::new(b":: LUMEN: convo trash seq=");
    w.dec(seq as i64).put(b" rc=").dec(r).put(b" to=").put(&to[..tn]).put(b" ::");
    w.wire();
    if r < 0 {
        let mut f = Line::new(b"could not move it to the Trash (");
        f.dec(r).put(b")");
        let n = f.n;
        a.flash(&[&f.b[..n]]);
        return;
    }
    let ix = side::index();
    let was_cur = a.hist.seq == seq as u32;
    ix.remove(seq);
    if was_cur {
        match ix.neighbour(seq) {
            Some(n) => switch_to(n),
            None => new_conversation(),
        }
    }
    app().flash(&[b"moved to the Trash"]);
}

/// LUMEN2: Ctrl-K — the find row over the conversations (the sidebar opens with it).
fn open_search() {
    let a = app();
    let mut sr = side::Search::new();
    sr.refilter();
    a.side.by_search = !a.side.open;
    a.side.open = true;
    a.side.scroll = 0;
    a.side.search = Some(sr);
}

/// LUMEN2: Ctrl-K again in the find row — the query against every conversation's TEXT (each file read whole,
/// up to FILE_CAP), newest first.
fn deep_search() {
    let a = app();
    let Some(sr) = a.side.search.as_mut() else { return };
    if matches!(a.hist.f, Files::None(_)) || sr.n == 0 {
        return;
    }
    let ix = side::index();
    let fb = filebuf();
    let mut k = 0;
    for i in 0..ix.len() {
        if k == side::HITS {
            break;
        }
        let Some(e) = ix.newest(i) else { break };
        let mut pb = [0u8; 300];
        let pn = hist_path(e.seq as u32, &mut pb);
        let r = files::read(a.hist.f, &pb[..pn], 0, fb);
        if r > 0 && (convo::contains_ci(&fb[..r as usize], &sr.q[..sr.n]) || convo::contains_ci(e.title(), &sr.q[..sr.n])) {
            sr.hits[k] = e.seq;
            k += 1;
        }
    }
    sr.hn = k;
    sr.sel = 0;
    sr.deep = true;
    a.side.scroll = 0;
    let mut w = Line::new(b":: LUMEN: convo search deep=1 q_len=");
    w.dec(sr.n as i64).put(b" hits=").dec(k as i64).put(b" ::");
    w.wire();
}

fn list_conversations() {
    let a = app();
    if matches!(a.hist.f, Files::None(_)) {
        a.note(b"history is off: no ring-3 file surface on this kernel");
        return;
    }
    let ix = side::index();
    if ix.is_empty() {
        a.note(b"no saved conversations yet");
        return;
    }
    a.note(b"conversations (/open N, or Ctrl-B for the sidebar):");
    for i in 0..ix.len().min(20) {
        let Some(e) = ix.newest(i) else { break };
        let t = e.title();
        let mut l = Line::new(if e.seq as u32 == a.hist.seq { b"\n* " } else { b"\n  " });
        l.dec(e.seq as i64).put(b"  ").put(if t.is_empty() { b"(empty)" } else { t });
        a.add(l.s());
    }
}

/// A command's argument as a clean single line.
fn arg_line(arg: &[u8], dst: &mut [u8]) -> usize {
    copy_clean(dst, arg)
}

fn command(cmd: &[u8]) {
    let a = app();
    let (verb, arg) = match cmd.iter().position(|&c| c == b' ') {
        Some(p) => (&cmd[..p], cmd[p + 1..].trim_ascii()),
        None => (cmd, &b""[..]),
    };
    match verb {
        b"/new" => new_conversation(),
        b"/list" => list_conversations(),
        b"/copy" => {
            a.sel = None;
            a.copy();
        }
        b"/copycode" => a.copy_code(None),
        b"/open" => {
            let n = arg.iter().take_while(|c| c.is_ascii_digit()).fold(0u32, |v, &c| v.saturating_mul(10).saturating_add((c - b'0') as u32));
            if n == 0 || n > history::SEQ_MAX || matches!(a.hist.f, Files::None(_)) || !hist_exists(n) {
                a.note(b"no such conversation (/list shows them)");
            } else {
                switch_to(n as u16);
            }
        }
        b"/rename" => {
            if arg.is_empty() {
                a.note(b"/rename <title> names this conversation in the sidebar");
                return;
            }
            a.conv_title_n = arg_line(arg, &mut a.conv_title);
            let seq = a.hist.seq as u16;
            if side::index().contains(seq) {
                side::index().set_title(seq, &a.conv_title[..a.conv_title_n]);
            }
            let v: &[u8] = unsafe { &*(&a.conv_title[..a.conv_title_n] as *const [u8]) };
            hist_put_setting(Setting::Title, v);
            let mut w = Line::new(b":: LUMEN: convo rename seq=");
            w.dec(seq as i64).put(b" title_len=").dec(a.conv_title_n as i64).put(b" ::");
            w.wire();
            a.flash(&[b"renamed"]);
        }
        b"/delete" => {
            let seq = a.hist.seq as u16;
            trash_conversation(seq);
        }
        b"/model" => {
            if arg.is_empty() {
                let m = eff_model();
                let info = models::lookup(m);
                let mut l = Line::new(b"model: ");
                l.put(m.as_bytes()).put(if a.conv_model_n > 0 { b" (this conversation)" as &[u8] } else { b" (Principia's vein.model)" });
                l.put(b"; context ").dec(info.context as i64).put(b" tokens; /model <id> changes it, /model - resets");
                a.note(l.s());
                return;
            }
            let mut buf = [0u8; 64];
            let n = arg_line(arg, &mut buf);
            let v = if &buf[..n] == b"-" { &buf[..0] } else { &buf[..n] };
            if !v.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_')) {
                a.note(b"a model id is letters, digits, '.', '-' and '_'");
                return;
            }
            a.conv_model[..v.len()].copy_from_slice(v);
            a.conv_model_n = v.len();
            hist_put_setting(Setting::Model, v);
            let m = eff_model();
            let mut l = Line::new(b"model for this conversation: ");
            l.put(m.as_bytes()).put(if gemini::is_gemini(m) { b" (Gemini)" as &[u8] } else { b"" });
            a.note(l.s());
            footer(None, (None, None));
        }
        b"/system" => {
            if arg.is_empty() {
                let s = eff_system();
                let mut l = Line::new(if a.conv_system_n > 0 { b"system (this conversation): " as &[u8] } else { b"system (default): " });
                l.put(&s.as_bytes()[..s.len().min(150)]);
                a.note(l.s());
                return;
            }
            let n = if arg == b"-" { 0 } else { arg_line(arg, &mut a.conv_system) };
            a.conv_system_n = n;
            let v: &[u8] = unsafe { &*(&a.conv_system[..n] as *const [u8]) };
            hist_put_setting(Setting::System, v);
            a.note(if n == 0 { b"system prompt reset to the default" } else { b"system prompt set for this conversation" });
        }
        b"/search" => {
            open_search();
            if let Some(sr) = app().side.search.as_mut() {
                sr.n = arg_line(arg, &mut sr.q);
                sr.refilter();
            }
        }
        b"/help" => a.note(b"Enter sends; Esc or [stop] stops an answer. Ctrl-N new, Ctrl-K find a conversation, Ctrl-B the sidebar. Ctrl-P/Ctrl-O select a reply, Cmd-C copies it, Ctrl-Y or a block's copy copies its code, Cmd-V pastes. /rename T /delete /model [M] /system [T|-] /list /open N /search Q /copy /copycode."),
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

static mut SESS: Option<Session> = None;
/// The session (set once in `_start`, before the first event).
fn sess() -> &'static Session {
    match unsafe { (*core::ptr::addr_of!(SESS)).as_ref() } {
        Some(s) => s,
        None => exit(4),
    }
}

impl Session {
    fn transport(&self) -> &'static [u8] {
        match (self.plan, self.cfg.endpoint()) {
            (Plan::Claude { .. }, Some(ep)) if ep.tls => b"tls",
            (Plan::Claude { .. }, Some(_)) => b"http",
            _ => b"none",
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

/// LUMEN2: who answers this conversation — its model picks the dialect; the key rule (SR36) decides whether it
/// may run or Echo answers with the reason. `vein.provider = "echo"` keeps every conversation offline.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Claude,
    Gemini,
    Echo(vein_core::prefs::Reason),
}

fn route() -> Route {
    let s = sess();
    if let Plan::Echo(vein_core::prefs::Reason::Chosen) = s.plan {
        return Route::Echo(vein_core::prefs::Reason::Chosen);
    }
    if gemini::is_gemini(eff_model()) {
        return match gemini::plan(gemini_key().0, s.tls.verify()) {
            Plan::Claude { .. } => Route::Gemini,
            Plan::Echo(r) => Route::Echo(r),
        };
    }
    match s.plan {
        Plan::Claude { .. } => Route::Claude,
        Plan::Echo(r) => Route::Echo(r),
    }
}

fn route_name(r: Route) -> &'static [u8] {
    match r {
        Route::Claude => b"claude",
        Route::Gemini => b"gemini",
        Route::Echo(_) => b"echo",
    }
}

/// The model this conversation asks for: its own, else Principia's `vein.model` (else the built-in default).
fn eff_model() -> &'static str {
    let a = app();
    if a.conv_model_n > 0 {
        if let Ok(m) = core::str::from_utf8(&a.conv_model[..a.conv_model_n]) {
            return m;
        }
    }
    sess().cfg.model()
}

/// The system prompt: this conversation's, else Principia's `vein.system_prompt`, else the built-in one.
fn eff_system() -> &'static str {
    let a = app();
    if a.conv_system_n > 0 {
        if let Ok(s) = core::str::from_utf8(&a.conv_system[..a.conv_system_n]) {
            return s;
        }
    }
    let (b, n) = unsafe { (&*core::ptr::addr_of!(DEFSYS), DEFSYS_N) };
    match core::str::from_utf8(&b[..n]) {
        Ok(s) if !s.is_empty() => s,
        _ => SYSTEM,
    }
}

/// Principia's `vein.system_prompt` (B317: Principia owns the defaults), read once at start.
static mut DEFSYS: [u8; SYS_CAP] = [0; SYS_CAP];
static mut DEFSYS_N: usize = 0;

/// The Gemini key: Holocron `vein/gemini.api_key` first, else the key file `vein.gemini_key_file` names (UnaFS
/// only, as Claude's). Read the first time a Gemini conversation needs it, then kept.
static mut GKEY: [u8; 320] = [0; 320];
static mut GKEY_STATE: Option<(KeyState, usize)> = None;

fn gemini_key() -> (KeyState, usize) {
    if let Some(k) = unsafe { *core::ptr::addr_of!(GKEY_STATE) } {
        return k;
    }
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(GKEY) };
    let k = match vein_ring3::holocron::key("vein", "gemini.api_key", buf) {
        vein_ring3::holocron::KeyFrom::Holocron(n) => (KeyState::UnaFs, n),
        vein_ring3::holocron::KeyFrom::Refuse(_) => (KeyState::None, 0),
        vein_ring3::holocron::KeyFrom::Fallback(_) => {
            let mut v = [0u8; 128];
            match vein_ring3::prefs::get("vein.gemini_key_file", &mut v) {
                Ok(Some(n)) => match core::str::from_utf8(vein_core::prefs::unquote(&v[..n])) {
                    Ok(p) if !p.is_empty() => vein_ring3::key::read(Some(p), buf),
                    _ => (KeyState::None, 0),
                },
                _ => (KeyState::None, 0),
            }
        }
    };
    unsafe { *core::ptr::addr_of_mut!(GKEY_STATE) = Some(k) };
    let mut l = Line::new(b":: LUMEN: gemini key=");
    l.put(k.0.as_str().as_bytes()).put(b" ::");
    l.wire();
    k
}

/// The status line's transport for this conversation.
fn transport_label(r: Route) -> &'static [u8] {
    match r {
        Route::Gemini => b"tls verified",
        Route::Claude => match sess().transport() {
            b"tls" => b"tls verified",
            b"http" => b"http relay",
            _ => b"offline",
        },
        Route::Echo(_) => b"offline",
    }
}

/// LUMEN2: the context window in use — the last reply's reported usage, else an estimate of what the next
/// request carries (the newest whole turns and the system prompt) — and the model's limit (0 = unknown).
fn context_use(r: Route) -> (u32, u32) {
    let a = app();
    let limit = match r {
        Route::Echo(_) => 0,
        _ => models::lookup(eff_model()).context,
    };
    let used = a.usage.unwrap_or_else(|| models::estimate_tokens(a.log_n - a.ctx_start() + eff_system().len()));
    (used, limit)
}

/// M5: the two status rows — provider / model, then transport, the context window, first-token latency.
fn footer(first_ms: Option<u64>, tokens: (Option<u32>, Option<u32>)) {
    let r = route();
    let mut r1 = Line::new(route_name(r));
    r1.put(b" / ").put(match r {
        Route::Echo(_) => vein_core::provider::ECHO_MODEL.as_bytes(),
        _ => eff_model().as_bytes(),
    });
    let mut r2 = Line::new(transport_label(r));
    let (used, limit) = context_use(r);
    let mut cb = [0u8; 48];
    let cn = {
        let mut o = Out::new(&mut cb);
        models::indicator(used, limit, &mut o);
        o.len()
    };
    r2.put(b"  ").put(&cb[..cn]);
    if tokens.0.is_none() && tokens.1.is_none() {
        // no usage this time: the estimate stands
    }
    if let Some(ms) = first_ms {
        r2.put(b"  1st ").dec(ms as i64).put(b"ms");
    }
    let a = app();
    let (n1, n2) = (r1.n, r2.n);
    a.set_status(&[&r1.b[..n1]], &[&r2.b[..n2]]);
}

/// LUMEN2: one Gemini exchange — the same resolve / connect / verified TLS path as `vein_ring3::send`, the
/// request from vein_core::gemini, the stream decoded by `exchange_dialect(Dialect::Gemini)` as it arrives.
/// The request head (it holds the key) is zeroed before this returns.
fn send_gemini(model: &str, system: &str, on: &mut dyn FnMut(Event<'_>)) -> Result<vein_ring3::Sent, vein_ring3::Stage> {
    use vein_core::client::{self, Fail};
    use vein_ring3::Stage;
    let bufs = unsafe { &mut *core::ptr::addr_of_mut!(BUFS) };
    let (_, kn) = gemini_key();
    let kb = unsafe { &*core::ptr::addr_of!(GKEY) };
    let key = vein_core::prefs::key_from_file(&kb[..kn]).ok_or(Stage::Encode)?;
    let p = Params::new(model, vein_core::claude::DEFAULT_MAX_TOKENS, system);
    let blen = {
        let a = app();
        let s = a.ctx_start();
        let turns = Turns { log: &a.log[s..a.log_n], i: 0 };
        gemini::encode_body(&p, turns, &mut bufs.body).ok_or(Stage::Encode)?
    };
    let mut pb = [0u8; gemini::PATH_CAP];
    let pn = gemini::path(model, &mut pb).ok_or(Stage::Encode)?;
    let path = core::str::from_utf8(&pb[..pn]).map_err(|_| Stage::Encode)?;
    let hlen = gemini::request_head(gemini::HOST, path, key, blen, &mut bufs.head).ok_or(Stage::Encode)?;
    let s = sess();
    let Some(ctx) = s.tls.context() else {
        bufs.head.fill(0);
        return Err(Stage::Handshake(vein_ring3::TlsFail { why: "no-tls-context", errno: vein_ring3::tls::EPROTO }));
    };
    let ip = match vein_ring3::net::resolve(gemini::HOST) {
        Ok(ip) => ip,
        Err(e) => {
            bufs.head.fill(0);
            return Err(Stage::Resolve(e));
        }
    };
    let mut tcp = match vein_ring3::net::Tcp::connect(ip, 443) {
        Ok(t) => t,
        Err(e) => {
            bufs.head.fill(0);
            return Err(Stage::Connect(e));
        }
    };
    let vein_ring3::Buffers { body, head, rx, line } = bufs;
    let r = vein_ring3::tls::with_session(&mut tcp, gemini::HOST, &ctx, |sn, v| (client::exchange_dialect(sn, Dialect::Gemini, &head[..hlen], &body[..blen], rx, line, on), v));
    head.fill(0);
    drop(tcp);
    let ((out, v), fail) = r.map_err(Stage::Handshake)?;
    if let (Some(f), Some(Fail::Send(_) | Fail::Recv(_))) = (fail, out.fail) {
        return Err(Stage::TlsStream(f));
    }
    match out.fail {
        None | Some(Fail::Http(_)) => Ok(vein_ring3::Sent { out, verified: Some(v) }),
        Some(f) => Err(Stage::Exchange(f)),
    }
}

fn submit() {
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

    let s = sess();
    let rt_ = route();
    let model = eff_model();
    let system = eff_system();
    let mut stop: &[u8] = b"end_turn";
    let mut verified: Option<vein_ring3::Verified> = None;
    let mut tokens: (Option<u32>, Option<u32>) = (None, None);
    let result = match rt_ {
        Route::Echo(_) => {
            let text = core::str::from_utf8(&tmp[..n]).unwrap_or("");
            vein_core::provider::echo(text, 12, &mut |p| on(Event::Text(p)));
            None
        }
        Route::Gemini => Some(send_gemini(model, system, &mut on)),
        Route::Claude => {
            let ep = s.cfg.endpoint().unwrap_or(vein_core::prefs::DEFAULT_ENDPOINT);
            let p = Params::new(model, vein_core::claude::DEFAULT_MAX_TOKENS, system);
            let bufs = unsafe { &mut *core::ptr::addr_of_mut!(BUFS) };
            let prepared = {
                let a = app();
                let st = a.ctx_start();
                let turns = Turns { log: &a.log[st..a.log_n], i: 0 };
                vein_ring3::prepare(&ep, &p, turns, s.key(), bufs)
            };
            let ctx = s.tls.context();
            Some(prepared.and_then(|req| vein_ring3::send(&ep, req, bufs, ctx.as_ref(), &mut on)))
        }
    };
    if let Some(r) = result {
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
                    a.note(b"stopped");
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
    let a = app();
    let stopped = a.cancel;
    a.busy = false;
    a.cancel = false;
    a.turn_open = false;
    if let Some(s0) = a.ai_start.take() {
        let e = a.turn_end(s0 as usize);
        hist_append(Who::Assistant, (s0 as usize + 1, e));
    }
    if tokens.0.is_some() || tokens.1.is_some() {
        app().usage = Some(tokens.0.unwrap_or(0).saturating_add(tokens.1.unwrap_or(0)));
    }
    footer(first, tokens);
    let (used, limit) = context_use(rt_);
    let mut l = Line::new(b":: LUMEN: reply provider=");
    l.put(route_name(rt_)).put(b" first_token_ms=").dec(first.map_or(-1, |m| m as i64)).put(b" bytes=").dec(bytes as i64).put(b" stop=").put(if stopped && stop == b"failed" { b"stopped" as &[u8] } else { stop });
    if let Some(v) = verified {
        l.put(b" transport=tls verified=").put(v.issuer().as_bytes()).put(b" ct=").put(v.ct().as_bytes());
    }
    l.put(b" in=").opt(tokens.0).put(b" out=").opt(tokens.1);
    l.put(b" model=").put(&model.as_bytes()[..model.len().min(48)]).put(b" ctx=").dec(used as i64).put(b"/").dec(limit as i64).put(b" ::");
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
    let th = theme::cur();
    match t {
        Tint::Plain => th.ink,
        Tint::Heading => th.head_ink,
        Tint::Dim => th.note_ink,
        Tint::Code => th.code_ink,
        Tint::Quote => th.quote_ink,
        Tint::Link => th.link_ink,
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
            s.fill(pen as i32, y, width(&text[i..e], k).ceil_px(), lh(), theme::cur().code_bg);
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

/// Draw the window; returns (visible transcript rows, the largest scroll, the layout the pointer reads).
fn render(a: &App, s: &Surf) -> (i32, i32, Geo) {
    let th = theme::cur();
    let (w, h) = (a.w, a.h);
    let lh = lh();
    s.fill(0, 0, w, h, th.bg);
    let tw = (w - 8) as f32; // the text width, px
    // the footer: two status rows, each with its button at the right (LUMEN2: [chats], and [stop] while answering)
    let fy2 = h - lh - 2;
    let fy1 = fy2 - lh;
    s.fill(0, fy1 - 3, w, 1, th.rule);
    let chats: &[u8] = if a.side.open { b"[chats x]" } else { b"[chats]" };
    let chats_x = w - 4 - width(chats, Kind::Sans) as i32;
    let st1 = &a.st1[..a.st1_n];
    s.text(4, fy1, &st1[..fit(st1, Kind::Sans, (chats_x - 10) as f32)], th.ok_ink);
    s.text(chats_x, fy1, chats, th.link_ink);
    let mut stop_x = i32::MAX;
    let (sc, st): (u32, &[u8]) = if a.busy {
        stop_x = w - 4 - width(b"[stop]", Kind::Sans) as i32;
        s.text(stop_x, fy2, b"[stop]", th.caret);
        (th.caret, b"... answering (Esc stops)")
    } else if a.flash_n > 0 && now_ms() < a.flash_until {
        (th.caret, &a.flash[..a.flash_n])
    } else {
        (th.note_ink, &a.st2[..a.st2_n])
    };
    let room2 = if a.busy { (stop_x - 10) as f32 } else { tw };
    s.text(4, fy2, &st[..fit(st, Kind::Sans, room2)], sc);
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
    s.fill(0, iy - 3, w, 1, th.rule);
    let mut r = 0;
    while r < shown {
        let row = top + r;
        let y = iy + r as i32 * lh;
        if row == 0 {
            s.text(4, y, b">", th.user_ink);
        }
        let lo = starts[row].min(a.inp_n);
        let hi = starts.get(row + 1).copied().unwrap_or(a.inp_n).min(a.inp_n);
        s.text(4 + 16, y, &a.inp[lo..hi], th.ink);
        if row == crow && a.side.search.is_none() {
            let cx = 4 + 16 + width(&a.inp[lo..a.caret.clamp(lo, hi)], Kind::Sans) as i32;
            s.fill(cx, y, 2, lh, th.caret);
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
                    s.fill(2, y, w - 8, lh, th.code_bg);
                }
                if matches!(block, Block::Rule | Block::TableRule) {
                    s.fill(4, y + lh / 2, w - 12, 1, th.rule);
                } else if block == Block::Code {
                    // LUMEN2: syntax_core's tokens over the code line (the rendered text IS the source line)
                    let a0 = (rec.dofs as usize).min(cache_len);
                    let b0 = (a0 + rec.len as usize).min(cache_len);
                    let toks = match a.fence_opener(p) {
                        Some(open) => hl::tokens(&a.log[..a.log_n], open, p, &|q| a.line_end(q)),
                        None => &[],
                    };
                    let text = &rt()[..cache_len];
                    let hang = (rec.hang as usize).min(cache_len);
                    let hx: f32 = (0..hang).map(|k| adv(text[k], Kind::Mono)).sum();
                    let mut pen = 4.0 + hx;
                    let mut k = a0;
                    while k < b0 {
                        let ink = hl::ink_at(toks, k);
                        let mut e = k + 1;
                        while e < b0 && hl::ink_at(toks, e) == ink {
                            e += 1;
                        }
                        pen = s.styled_k(pen, y, &text[k..e], ink, Kind::Mono, false);
                        k = e;
                    }
                } else {
                    let text = &rt()[..cache_len];
                    let spans = &rs()[..cache_spans];
                    let a0 = (rec.dofs as usize).min(cache_len);
                    let b0 = (a0 + rec.len as usize).min(cache_len);
                    let hang = (rec.hang as usize).min(cache_len);
                    let hx: f32 = (0..hang).map(|k| adv(text[k], style_at(spans, k, fence).0)).sum();
                    draw_spans(s, 4.0 + hx, y, text, spans, a0, b0, fence);
                    // LUMEN2: the copy verb on a block's opening fence row
                    if block == Block::Fence && rec.flags & F_FENCE == 0 && rec.flags & F_LEAD != 0 {
                        s.text(w - 8 - copy_w(), y, b"copy", th.link_ink);
                    }
                }
            }
            role => {
                let e = a.line_end(p);
                let a0 = (p + rec.dofs as usize).min(e);
                let b0 = (a0 + rec.len as usize).min(e);
                let (ink, mark): (u32, &[u8]) = if role == ROLE_USER { (th.user_ink, b">") } else { (th.note_ink, b"*") };
                if rec.flags & F_TURN != 0 && rec.flags & F_LEAD != 0 {
                    s.text(4, y, mark, ink);
                }
                s.text(4 + 16, y, &a.log[a0..b0], ink);
            }
        }
        if selected {
            s.fill(0, y, 2, lh, th.sel_bar);
        }
        i += 1;
    }
    // M1: the position indicator — a scrollbar on the right edge when there is more than a screen
    if n > rows {
        let track = rows as i32 * lh;
        let thumb = (track * rows as i32 / n as i32).max(6);
        let ty = 3 + ((track - thumb) as i64 * first as i64 / maxs.max(1) as i64) as i32;
        s.fill(w - 3, 3, 2, track, th.rule);
        s.fill(w - 3, ty, 2, thumb, th.caret);
    }
    // LUMEN2: the conversations sidebar, over the transcript's left edge
    if a.side.open {
        draw_side(a, s, lh, iy - 6);
    }
    (rows as i32, maxs as i32, Geo { first, rows: rows.min(n), lh, iy, fy1, fy2, chats_x, stop_x })
}

/// LUMEN2: the sidebar panel — header ([+ new] or the find row), then the conversations newest first.
fn draw_side(a: &App, s: &Surf, lh: i32, bottom: i32) {
    let th = theme::cur();
    let g = side::geom(lh, bottom);
    let sw = side::SIDE_W;
    s.fill(0, 0, sw, bottom + 3, th.bg);
    s.fill(sw, 0, 1, bottom + 3, th.rule);
    let ix = side::index();
    match &a.side.search {
        Some(sr) => {
            let x = s.text(4, g.y0, if sr.deep { b"text:" } else { b"find:" }, th.note_ink) as i32 + 4;
            let q = sr.query();
            let room = (sw - x - 6) as f32;
            let shown = &q[q.len() - fit_rev(q, room)..];
            let end = s.text(x, g.y0, shown, th.ink) as i32;
            s.fill(end + 1, g.y0, 2, lh, th.caret);
        }
        None => {
            s.text(4, g.y0, b"Chats", th.head_ink);
            s.text(side::new_x(), g.y0, b"+ new", th.link_ink);
        }
    }
    s.fill(2, g.ey - 3, sw - 4, 1, th.rule);
    let sel = a.side.search.as_ref().map(|sr| sr.sel);
    let mut i = 0;
    while i < g.rows {
        let Some(seq) = a.side.seq_at(i) else { break };
        let y = g.ey + i as i32 * lh;
        let cur = seq == ix.cur;
        let picked = sel == Some(i + a.side.scroll);
        if picked {
            s.fill(1, y, sw - 2, lh, th.code_bg);
        }
        if cur {
            s.fill(0, y, 2, lh, th.sel_bar);
        }
        let t = ix.title(seq);
        let mut nb = [0u8; 8];
        let label: &[u8] = if t.is_empty() {
            let mut o = Out::new(&mut nb);
            o.put(b"#");
            o.dec(seq as u64);
            let k = o.len();
            &nb[..k]
        } else {
            t
        };
        let right = if cur && a.side.search.is_none() { side::del_x() - 4 } else { sw - 6 };
        let room = (right - 6) as f32;
        let ink = if cur { th.ink } else { th.note_ink };
        s.text(6, y, &label[..fit(label, Kind::Sans, room)], ink);
        if cur && a.side.search.is_none() {
            let armed = a.side.del_armed.is_some_and(|(sq, until)| sq == seq && now_ms() < until);
            s.text(side::del_x(), y, if armed { b"del?" } else { b"del" }, if armed { th.caret } else { th.note_ink });
        }
        i += 1;
    }
    if i == 0 {
        let msg: &[u8] = if a.side.search.is_some() { b"no match (Ctrl-K: search text)" } else { b"no saved conversations" };
        s.text(6, g.ey, &msg[..fit(msg, Kind::Sans, (sw - 10) as f32)], th.note_ink);
    }
}

/// How many TRAILING bytes of `s` fit in `max` px (the find row shows the end of a long query).
fn fit_rev(s: &[u8], max: f32) -> usize {
    let mut x = 0f32;
    for (k, &c) in s.iter().rev().enumerate() {
        x += adv(c, Kind::Sans);
        if x > max + 0.01 {
            return k;
        }
    }
    s.len()
}

/// The [copy] verb's width on a fence row.
fn copy_w() -> i32 {
    width(b"copy", Kind::Sans) as i32
}

/// LUMEN2: what the pointer can be over.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    Outside,
    Text,
    Footer,
    Input,
    Chats,
    Stop,
    /// The [copy] verb of the code block whose fence line starts at this log offset.
    Copy(u32),
    /// A link; its URL is in [`LINK`].
    Link,
    Side(side::Spot),
}

/// The URL under the pointer (the last [`link_at`] that answered true).
static mut LINK: [u8; 256] = [0; 256];
static mut LINK_N: usize = 0;

/// Whether the pointer's x on the transcript row `rec` is over a link: a markdown link (the text, shown as
/// `text (url)`) or a bare `http://` / `https://` word. On true the URL is in [`LINK`].
fn link_at(a: &App, rec: &Rec, x: i32) -> bool {
    let p = rec.src as usize;
    if rec.role == 0 || p >= a.log_n {
        return false;
    }
    let e = a.line_end(p);
    let (text, spans, a0, b0, hx, fence): (&[u8], &[Span], usize, usize, f32, bool) = if rec.role == ROLE_AI {
        let mut st = State { fence: rec.flags & F_FENCE != 0 };
        let l = md::line(&mut st, &a.log[p..e], rt(), rs());
        let block = Block::from_u8(rec.block);
        let fence = matches!(block, Block::Code | Block::Fence);
        if fence || matches!(block, Block::Rule | Block::TableRule) {
            return false;
        }
        let text = &rt()[..l.len];
        let spans = &rs()[..l.spans];
        let a0 = (rec.dofs as usize).min(l.len);
        let b0 = (a0 + rec.len as usize).min(l.len);
        let hang = (rec.hang as usize).min(l.len);
        let hx: f32 = (0..hang).map(|k| adv(text[k], style_at(spans, k, false).0)).sum();
        (text, spans, a0, b0, 4.0 + hx, false)
    } else {
        let a0 = (p + rec.dofs as usize).min(e);
        let b0 = (a0 + rec.len as usize).min(e);
        (&a.log[..e], &[][..], a0, b0, 20.0, false)
    };
    // the byte under x
    let mut pen = hx;
    let mut j = None;
    for k in a0..b0 {
        let d = adv(text[k], if spans.is_empty() { Kind::Sans } else { style_at(spans, k, fence).0 });
        if (x as f32) >= pen && (x as f32) < pen + d {
            j = Some(k);
            break;
        }
        pen += d;
    }
    let Some(j) = j else { return false };
    let put = |u: &[u8]| -> bool {
        if u.is_empty() || u.len() > 256 {
            return false;
        }
        unsafe {
            (&mut *core::ptr::addr_of_mut!(LINK))[..u.len()].copy_from_slice(u);
            LINK_N = u.len();
        }
        true
    };
    // a markdown link's text: its URL is the dim `(url)` md wrote after it
    if let Some(sp) = spans.iter().find(|sp| sp.tint == Tint::Link && (sp.start as usize) <= j && j < sp.end as usize) {
        let after = &text[sp.end as usize..];
        if let Some(rest) = after.strip_prefix(b" (") {
            if let Some(q) = rest.iter().position(|&c| c == b')') {
                return put(&rest[..q]);
            }
        }
        // `[url](url)` is shown as the url alone
        return put(&text[sp.start as usize..sp.end as usize]);
    }
    // a bare URL word (also the `(url)` part md shows)
    let is_stop = |c: u8| c < 0x21 || c == b'(' || c == b')' || c == b'<' || c == b'>' || c == b'"';
    let mut ws = j;
    while ws > 0 && !is_stop(text[ws - 1]) {
        ws -= 1;
    }
    let mut we = j;
    while we < text.len() && !is_stop(text[we]) {
        we += 1;
    }
    let word = &text[ws..we];
    let word = word.strip_suffix(b".").or_else(|| word.strip_suffix(b",")).unwrap_or(word);
    if word.starts_with(b"https://") || word.starts_with(b"http://") { put(word) } else { false }
}

/// LUMEN2: open `url` in Aether (`aether <url>`, SYS_SPAWN_PATH — the program the desktop opens a link with).
fn open_link(url: &[u8]) {
    const AETHER: &[u8] = b"/apps/AETHER.ELF";
    let argv = [una_abi::SpawnArg { ptr: b"aether".as_ptr() as u64, len: 6 }, una_abi::SpawnArg { ptr: url.as_ptr() as u64, len: url.len() as u64 }];
    let r = sys(una_abi::SYS_SPAWN_PATH, AETHER.as_ptr() as u64, AETHER.len() as u64, argv.as_ptr() as u64, 2);
    let mut l = Line::new(b":: LUMEN: link open rc=");
    l.dec(r).put(b" url=").put(&url[..url.len().min(120)]).put(b" ::");
    l.wire();
    let a = app();
    if r < 0 {
        let mut f = Line::new(b"could not open the link (");
        f.dec(r).put(b")");
        let n = f.n;
        a.flash(&[&f.b[..n]]);
    } else {
        a.flash(&[b"opened in Aether"]);
    }
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
    let t_start = now_ms(); // SMALLFIX6 (B495): the start's split, said before the first line
    let win = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0, 0);
    if win < 0 {
        Line::new(b":: LUMEN: SYS_WIN_CREATE refused ::").wire();
        exit(1);
    }
    theme::load(); // UNAKEYS (B552): the look from Principia, before the first paint
    theme_wire();
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

    let t_win = now_ms();
    // The session: Principia's `vein` namespace, the key file, the rule.
    let cfg = vein_ring3::prefs::Config::read();
    // SETTINGSFILES (B407, R98): Lumen's own settings stanza, declared once — `app.lumen.*` in `<home>/settings/lumen`.
    let declared = vein_ring3::prefs::declare(vein_ring3::prefs::LUMEN_STANZA);
    // LUMEN2 (SR121): the default system prompt is Principia's (`vein.system_prompt`, B317); unset = the built-in.
    {
        let mut v = [0u8; SYS_CAP];
        if let Ok(Some(n)) = vein_ring3::prefs::get("vein.system_prompt", &mut v) {
            let q = vein_core::prefs::unquote(&v[..n]);
            let d = unsafe { &mut *core::ptr::addr_of_mut!(DEFSYS) };
            unsafe { DEFSYS_N = copy_clean(d, q) };
        }
    }
    unsafe { DECLARED = declared.err().unwrap_or(0) };
    let t_bus = now_ms();
    let keybuf = unsafe { &mut *core::ptr::addr_of_mut!(KEY) };
    let mut kpath = [0u8; 128];
    // HOLOCRON2 M2 (B355): Holocron first (`vein/claude.api_key`, keysource::decide); the key file only on
    // NotFound or no fulfiller; LOCKED / DENIED / CORRUPT refuse and the window names the fix.
    let hk = vein_ring3::holocron::claude_key(keybuf);
    let t_holo = now_ms(); // LUMENFAST (B507): the Holocron round trips end here; the key file's half follows
    let (key, key_n) = match hk {
        vein_ring3::holocron::KeyFrom::Holocron(n) => (KeyState::UnaFs, n),
        vein_ring3::holocron::KeyFrom::Refuse(_) => (KeyState::None, 0),
        vein_ring3::holocron::KeyFrom::Fallback(_) => {
            let kfile = match cfg.key_file() { Some(k) => Some(k), None => vein_ring3::key::default_path(&mut kpath) }; // RING3ABI2 M3 (B333): unset preference = `<home>/.config/unaos/vein.key`, `<home>` from SYS_WHOAMI
            vein_ring3::key::read(kfile, keybuf)
        }
    };
    let t_key = now_ms();
    let tls = vein_ring3::TlsSetup::load(); // VEINTLS (SR36)
    let t_tls = now_ms();
    let plan = cfg.plan(key, tls.verify());
    unsafe { *core::ptr::addr_of_mut!(SESS) = Some(Session { plan, key, key_n, cfg, tls }) };
    let sess = sess();
    vein_ring3::net::set_tick(Some(tick));

    // M4: the newest conversation comes back.
    hist_init();
    let t_hist = now_ms();

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
    split_line(&[t_start, t_win, t_bus, t_key, t_tls, t_hist, now_ms()], t_holo.saturating_sub(t_bus));

    let mut l = Line::new(b":: LUMEN: start provider=");
    let (sprov, smodel): (&[u8], &[u8]) = match sess.plan {
        Plan::Claude { .. } => (b"claude", sess.cfg.model().as_bytes()),
        Plan::Echo(_) => (b"echo", vein_core::provider::ECHO_MODEL.as_bytes()),
    };
    l.put(sprov).put(b" model=").put(smodel).put(b" key=").put(match hk { vein_ring3::holocron::KeyFrom::Holocron(_) => b"holocron" as &[u8], _ => key.as_str().as_bytes() }).put(b" transport=").put(sess.transport());
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
    // LUMEN2 (SR121): how many conversations the sidebar holds, and whose system prompt is the default
    l.put(b" convs=").dec(side::index().len() as i64).put(if unsafe { DEFSYS_N } > 0 { b" system=principia" as &[u8] } else { b" system=builtin" });
    l.put(b" ::");
    l.wire();

    {
        let a = app();
        if a.log_n == 0 {
            a.note(b"Lumen on UnaOS. Enter sends, Esc stops, Ctrl-N new, Ctrl-K find, Ctrl-B chats, /help for the rest.");
        } else {
            let mut m = Line::new(b"conversation ");
            m.dec(a.hist.seq as i64).put(b" reloaded; Ctrl-N or /new starts a new one, Ctrl-B lists them all.");
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
    footer(None, (None, None));

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
                if input_ev_type(ev as u64) == INPUT_EV_KEY_DOWN && input_ev_payload(ev as u64) as u8 == b'\n' && a.side.search.is_none() {
                    send = true; // LUMEN2: Enter in the find row opens the pick instead (search_key)
                }
                a.event(ev as u64);
                k += 1;
            }
        }
        if send {
            submit();
        }
        let a = app();
        if a.flash_n > 0 && now_ms() >= a.flash_until {
            a.flash_n = 0;
            a.dirty = true;
        }
        if theme::due(now_ms()) && theme::load() {
            a.dirty = true; // UNAKEYS (B552): the store moved — repaint in the new look
            theme_wire();
        }
        a.paint();
        sleep_ms(16);
    }
}

/// `:: LUMEN: theme mode=<light|dark> accent=<name> bus=<0|-errno> ::` — the look in force.
fn theme_wire() {
    let l = theme::cur().look;
    let mut w = Line::new(b":: LUMEN: theme mode=");
    w.put(l.mode_name().as_bytes()).put(b" accent=").put(l.accent_name().as_bytes()).put(b" bus=").dec(theme::bus_err()).put(b" ::");
    w.wire();
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

/// SMALLFIX6 (rmbp-ledger B495): flight 26 read `LUMENCRASH first_line=12262` with no split. One line before the
/// first, the start's cost by phase (ms on the kernel clock): `win` the window, `bus` Principia's prefs and the
/// stanza declaration (bus round trips), `key` Holocron and the key file, `tls` the trust roots, `hist` the newest
/// conversation, `font` the faces read off the volume. `start_at_ms` is `_start` on the kernel clock, so the seat
/// reads the ELF load + APPRES sight as `start_at_ms` minus the spawn (the kernel's `[lumencrash] … spawn_ms=`).
///
/// `[lumen] first_line_ms=<n> start_at_ms=<n> win_ms=<n> bus_ms=<n> key_ms=<n> tls_ms=<n> hist_ms=<n> font_ms=<n> key_holo_ms=<n>`
/// LUMENFAST (B507): `key_holo_ms` = the Holocron round trips inside `key_ms` (the rest is the key file's stat/read).
fn split_line(t: &[u64; 7], key_holo_ms: u64) {
    let d = |i: usize| t[i + 1].saturating_sub(t[i]) as i64;
    let mut l = Line::new(b"[lumen] first_line_ms=");
    l.dec(t[6].saturating_sub(t[0]) as i64).put(b" start_at_ms=").dec(t[0] as i64);
    l.put(b" win_ms=").dec(d(0)).put(b" bus_ms=").dec(d(1)).put(b" key_ms=").dec(d(2));
    l.put(b" tls_ms=").dec(d(3)).put(b" hist_ms=").dec(d(4)).put(b" font_ms=").dec(d(5));
    l.put(b" key_holo_ms=").dec(key_holo_ms as i64);
    l.wire();
}
