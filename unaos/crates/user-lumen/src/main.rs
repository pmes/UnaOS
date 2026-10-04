#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// LUMENAPP (rmbp-ledger B323, R82): `APPS/LUMEN.ELF` — Lumen on UnaOS is ONE program, like the Claude
// app. Peter: "just make it work like the claude app. it does not need system services running."
//
// WHAT IT IS. A window over a conversation that talks to the provider ITSELF: it links Vein as a library
// (`vein_core`, the pure encoder/decoder/client/rules, and `vein_ring3`, the syscall transport), resolves
// the endpoint (SYS_RESOLVE), opens TCP (the NETRING3 socket verbs), runs TLS 1.3 over it (embedded-tls),
// POSTs /v1/messages with `stream: true` and draws the deltas as they arrive. Nothing else runs: no
// daemon, no bus chat verbs, no autostart. (LUMENBIN's two-program shape — this window relaying to a
// VEIN.BIN service over bus verbs 130..133 — is retired.)
//
// CONFIGURATION is Principia's (BUS_VERB_PREF_GET, namespace `vein`): `vein.provider`, `vein.model`,
// `vein.endpoint`, `vein.tls`, `vein.key_file`. The KEY is read only from a file on the UnaFS volume (it
// stats with an inode id); on FAT it is refused with the reason in the window. With no key the Echo
// provider answers, so the window works on the FAT card. `vein_core::prefs::plan` is the rule.
//
// THE ELF MODEL. Linked at una_abi::USER_XWIN_VA_X86 (user-lumen-x86.ld, the user-big shape), so the
// kernel maps it in the 4 MiB ELF window with a 256 KiB stack (`-z stack-size`): ~80 KiB of TLS code and
// ~90 KiB of buffers cannot live in the 16 KiB fixed window. The window landmarks (info page, surface)
// stay at the fixed USER_BASE_X86 + 0x4000 / + 0x5000.
//
// KEYS. Printable ASCII inserts at the caret; Backspace deletes; Left/Right (and the caret actions) move
// it; Enter sends; Up/Down and the wheel scroll; Ctrl-K or Cmd-K (INPUT_EV_ACTION 41, ClearView) starts a
// new conversation; Esc cancels an answer in flight and NEVER closes the window (R24).
//
// WIRE. `:: LUMEN: start provider=<claude|echo> model=<m> key=<unafs|none|fat-refused> transport=<tls|http|none> ::`
// once, then per answer `:: LUMEN: reply provider=<p> first_token_ms=<n> bytes=<n> stop=<s> ::` or
// `:: LUMEN: fail stage=<s> code=<n> ::`. The window's footer shows the provider and the first token's latency.

use una_abi::{
    input_ev_payload, input_ev_type, INPUT_EV_ACTION, INPUT_EV_KEY_DOWN, INPUT_EV_WHEEL, INPUT_EV_WIN_RESIZE, KEY_DOWN, KEY_ESC, KEY_LEFT, KEY_RIGHT, KEY_UP,
    SYS_EXIT, SYS_INPUT_POLL, SYS_WIN_CREATE, SYS_WIN_PRESENT,
};
use vein_core::claude::{Event, Msg, Params, Stop};
use vein_core::prefs::{KeyState, Plan};
use vein_core::Role;
use vein_ring3::sys::{now_ms, sleep_ms, sys, write};

/// The app note the loader and the shell read (EXECNAME): name "UnaOS", type 1, desc = flags, bit0 =
/// windowed (a bare-word launch detaches it).
#[repr(C, align(4))]
struct AppNote {
    namesz: u32,
    descsz: u32,
    ntype: u32,
    name: [u8; 8],
    flags: u32,
}
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: AppNote = AppNote { namesz: 6, descsz: 4, ntype: 1, name: *b"UnaOS\0\0\0", flags: una_abi::APP_NOTE_WINDOWED };

const ACTION_CLEAR_VIEW: u64 = 41;
const ACTION_CURSOR_LEFT: u64 = 13;
const ACTION_CURSOR_RIGHT: u64 = 14;
const ACTION_LINE_START: u64 = 15;
const ACTION_LINE_END: u64 = 16;

const SYSTEM: &str = "You are Claude, talking with the person through Lumen, the chat window of UnaOS. The window shows plain ASCII text only, 35 columns wide: answer briefly, without markdown tables or images.";

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
const LH: i32 = 10;
/// Scrollback = the conversation sent to the model (oldest turns drop first).
const LOG_CAP: usize = 32 * 1024;
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

static FONT: [[u8; 8]; 128] = font8x8::legacy::BASIC_LEGACY;

struct App {
    log: [u8; LOG_CAP],
    log_n: usize,
    inp: [u8; INP_CAP],
    inp_n: usize,
    caret: usize,
    status: [u8; 64],
    status_n: usize,
    w: i32,
    h: i32,
    scroll: i32,
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
    log: [0; LOG_CAP],
    log_n: 0,
    inp: [0; INP_CAP],
    inp_n: 0,
    caret: 0,
    status: [0; 64],
    status_n: 0,
    w: 0,
    h: 0,
    scroll: 0,
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

static mut BUFS: vein_ring3::Buffers = vein_ring3::Buffers::new();
static mut KEY: [u8; 320] = [0; 320];

impl App {
    fn set_status(&mut self, parts: &[&[u8]]) {
        self.status_n = 0;
        for &c in parts.iter().flat_map(|p| p.iter()) {
            if self.status_n < self.status.len() {
                self.status[self.status_n] = if (0x20..0x7f).contains(&c) { c } else { b'?' };
                self.status_n += 1;
            }
        }
        self.dirty = true;
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
        self.scroll = 0;
        self.dirty = true;
    }
    fn turn(&mut self, role: u8, text: &[u8]) {
        self.room(1);
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
        self.dirty = true;
    }

    /// One input event. While an answer streams, Enter does nothing and Esc cancels.
    fn event(&mut self, ev: u64) {
        let p = input_ev_payload(ev);
        self.dirty = true;
        match input_ev_type(ev) {
            INPUT_EV_KEY_DOWN => self.key(p as u8),
            INPUT_EV_ACTION => match p {
                ACTION_CLEAR_VIEW if !self.busy => self.clear(),
                ACTION_CURSOR_LEFT => self.caret = self.caret.saturating_sub(1),
                ACTION_CURSOR_RIGHT => self.caret = (self.caret + 1).min(self.inp_n),
                ACTION_LINE_START => self.caret = 0,
                ACTION_LINE_END => self.caret = self.inp_n,
                _ => {}
            },
            INPUT_EV_WHEEL => self.scroll = (self.scroll + (p as u8 as i8 as i32)).max(0),
            INPUT_EV_WIN_RESIZE => {
                self.w = (((p >> 16) & 0xffff) as i32).clamp(64, MAX_W);
                self.h = ((p & 0xffff) as i32).clamp(64, MAX_H);
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
            0x0b if !self.busy => self.clear(),
            KEY_ESC => {
                if self.busy {
                    self.cancel = true;
                }
            }
            KEY_LEFT => self.caret = self.caret.saturating_sub(1),
            KEY_RIGHT => self.caret = (self.caret + 1).min(self.inp_n),
            KEY_UP => self.scroll += 1,
            KEY_DOWN => self.scroll = (self.scroll - 1).max(0),
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

    /// Lay the transcript out at `cols`, calling `emit(role, text, first_line_of_turn)` per line.
    fn layout(&self, cols: usize, emit: &mut dyn FnMut(u8, &[u8], bool)) {
        let mut i = 0;
        while i < self.log_n {
            let role = self.log[i];
            let mut e = i + 1;
            while e < self.log_n && self.log[e] > ROLE_NOTE {
                e += 1;
            }
            let width = if role == ROLE_AI { cols } else { cols.saturating_sub(2) }.max(4);
            let mut first = true;
            for para in self.log[i + 1..e].split(|&c| c == b'\n') {
                let mut s = 0;
                loop {
                    let rem = para.len() - s;
                    if rem <= width {
                        emit(role, &para[s..], first);
                        first = false;
                        break;
                    }
                    let mut b = s + width;
                    while b > s && para[b] != b' ' {
                        b -= 1;
                    }
                    if b == s {
                        emit(role, &para[s..s + width], first);
                        s += width;
                    } else {
                        emit(role, &para[s..b], first);
                        s = b + 1;
                    }
                    first = false;
                }
            }
            emit(0, b"", false);
            i = e;
        }
    }

    /// Repaint now if something changed and the window is visible.
    fn paint(&mut self) {
        if !self.dirty || unsafe { self.flags.read_volatile() } & 2 != 0 {
            return;
        }
        self.dirty = false;
        self.last_paint = now_ms();
        let s = Surf { p: self.surf, w: self.w, h: self.h };
        render(self, &s);
        sys(SYS_WIN_PRESENT, self.win as u64, 0, 0, 0);
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

/// What the session runs on, decided once at start.
struct Session {
    plan: Plan,
    key: KeyState,
    key_n: usize,
    cfg: vein_ring3::prefs::Config,
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

fn footer(sess: &Session, first_ms: Option<u64>) {
    let mut l = Line::new(sess.provider());
    l.put(b" / ").put(sess.model());
    if let Some(ms) = first_ms {
        l.put(b"  1st ").dec(ms as i64).put(b"ms");
    }
    let a = app();
    let n = l.n;
    a.set_status(&[&l.b[..n]]);
}

fn submit(sess: &Session) {
    let a = app();
    if a.inp_n == 0 || a.busy {
        return;
    }
    let n = a.inp_n;
    a.turn(ROLE_USER, b"");
    let mut tmp = [0u8; INP_CAP];
    tmp[..n].copy_from_slice(&a.inp[..n]);
    a.add(&tmp[..n]);
    a.inp_n = 0;
    a.caret = 0;
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
                let turns = Turns { log: &a.log[..a.log_n], i: 0 };
                vein_ring3::prepare(&ep, &p, turns, sess.key(), bufs)
            };
            let r = prepared.and_then(|req| vein_ring3::send(&ep, req, bufs, &mut on));
            match r {
                Ok(o) => {
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
                    #[cfg(target_arch = "x86_64")]
                    if matches!(st, vein_ring3::Stage::Handshake(_)) {
                        l.put(b" tls=").put(vein_ring3::tls::last_error().as_bytes());
                    }
                    l.put(b" ::");
                    l.wire();
                    if a.cancel {
                        a.note(b"cancelled");
                    } else {
                        let mut w = Line::new(b"could not reach the provider: ");
                        w.put(st.name().as_bytes()).put(b" (").dec(st.code()).put(b")");
                        a.note(w.s());
                    }
                }
            }
        }
    }
    let a = app();
    a.busy = false;
    a.turn_open = false;
    footer(sess, first);
    let mut l = Line::new(b":: LUMEN: reply provider=");
    l.put(sess.provider()).put(b" first_token_ms=").dec(first.map_or(-1, |m| m as i64)).put(b" bytes=").dec(bytes as i64).put(b" stop=").put(stop).put(b" ::");
    l.wire();
    a.paint();
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
    fn text(&self, x: i32, y: i32, s: &[u8], c: u32) {
        for (k, &ch) in s.iter().enumerate() {
            let g = &FONT[(ch & 0x7f) as usize];
            let gx = x + k as i32 * 8;
            for (r, &bits) in g.iter().enumerate() {
                let mut col = 0;
                while col < 8 {
                    if bits & (1 << col) != 0 {
                        self.fill(gx + col, y + r as i32, 1, 1, c);
                    }
                    col += 1;
                }
            }
        }
    }
}

fn render(a: &App, s: &Surf) {
    let (w, h) = (a.w, a.h);
    s.fill(0, 0, w, h, BG);
    let cols = ((w - 8) / 8).max(4) as usize;
    let fy = h - LH - 2;
    s.fill(0, fy - 3, w, 1, RULE);
    let (sc, st): (u32, &[u8]) = if a.busy { (CARET, b"... answering (Esc cancels)") } else { (OK_INK, &a.status[..a.status_n]) };
    s.text(4, fy, &st[..st.len().min(cols)], sc);
    let iw = cols.saturating_sub(2).max(1);
    let total = a.inp_n / iw + 1;
    let shown = total.min(3);
    let crow = a.caret / iw;
    let top = (crow + 1).saturating_sub(shown);
    let iy = fy - 6 - shown as i32 * LH;
    s.fill(0, iy - 3, w, 1, RULE);
    let mut r = 0;
    while r < shown {
        let row = top + r;
        let y = iy + r as i32 * LH;
        if row == 0 {
            s.text(4, y, b">", USER_INK);
        }
        let lo = (row * iw).min(a.inp_n);
        let hi = ((row + 1) * iw).min(a.inp_n);
        s.text(4 + 16, y, &a.inp[lo..hi], INK);
        if row == crow {
            s.fill(4 + 16 + (a.caret % iw) as i32 * 8, y - 1, 2, 10, CARET);
        }
        r += 1;
    }
    let rows = ((iy - 6 - 4) / LH).max(1) as usize;
    let mut n = 0usize;
    a.layout(cols, &mut |_, _, _| n += 1);
    let maxs = n.saturating_sub(rows);
    let sc = (a.scroll.max(0) as usize).min(maxs);
    let first = n - rows.min(n) - sc;
    let mut i = 0usize;
    a.layout(cols, &mut |role, t, lead| {
        if i >= first && i < first + rows {
            let y = 4 + (i - first) as i32 * LH;
            match role {
                ROLE_USER => {
                    s.text(4, y, if lead { b"> " } else { b"  " }, USER_INK);
                    s.text(4 + 16, y, t, USER_INK);
                }
                ROLE_NOTE => {
                    s.text(4, y, if lead { b"* " } else { b"  " }, NOTE_INK);
                    s.text(4 + 16, y, t, NOTE_INK);
                }
                _ => s.text(4, y, t, INK),
            }
        }
        i += 1;
    });
}

/// The window landmarks: the RO info page at base + 0x4000, surface slot 0 at base + 0x5000. An
/// elf-model program finds them at the FIXED slot base (RING3WIN), not at its own `_start`.
#[cfg(target_arch = "x86_64")]
fn landmark_base() -> u64 {
    una_abi::USER_BASE_X86
}
#[cfg(not(target_arch = "x86_64"))]
fn landmark_base() -> u64 {
    _start as *const () as u64
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
        a.win = win;
        (a.w, a.h, a.dirty) = (MAX_W, MAX_H, true);
        a.surf = (base + 0x5000) as *mut u8;
        a.flags = unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4) }; // VUGMIN: bit 1 = all hidden
        a.set_status(&[b"reading preferences ..."]);
        a.paint();
    }

    // The session: Principia's `vein` namespace, the key file, the rule.
    let cfg = vein_ring3::prefs::Config::read();
    let keybuf = unsafe { &mut *core::ptr::addr_of_mut!(KEY) };
    let (key, key_n) = vein_ring3::key::read(cfg.key_file(), keybuf);
    let plan = cfg.plan(key);
    let sess = Session { plan, key, key_n, cfg };
    vein_ring3::net::set_tick(Some(tick));

    let mut l = Line::new(b":: LUMEN: start provider=");
    l.put(sess.provider()).put(b" model=").put(sess.model()).put(b" key=").put(key.as_str().as_bytes()).put(b" transport=").put(sess.transport());
    if sess.cfg.bus_err != 0 {
        l.put(b" prefs=").dec(sess.cfg.bus_err);
    }
    l.put(b" ::");
    l.wire();

    {
        let a = app();
        a.note(b"Lumen on UnaOS. Enter sends, Ctrl-K or Cmd-K starts over, Esc cancels.");
        match sess.plan {
            Plan::Echo(r) => {
                a.note(b"offline: the echo provider answers. ");
                a.add(r.text().as_bytes());
            }
            Plan::Claude { send_key } => {
                if send_key && sess.transport() == b"tls" {
                    a.note(b"TLS without certificate checks (vein.tls = insecure): the trust store is owed.");
                } else if !send_key {
                    a.note(b"relay endpoint: the key stays on the relay.");
                }
            }
        }
        if key == KeyState::OnFat {
            a.note(KeyState::OnFat.as_str().as_bytes());
        }
    }
    footer(&sess, None);

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
        app().paint();
        sleep_ms(16);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: LUMEN: panic ::\n");
    exit(3)
}
