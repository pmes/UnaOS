#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// LUMENBIN (rmbp-ledger B305): `APPS/LUMEN.BIN` — Lumen, the chat window, as a ring-3 program on the
// kernel desktop. Peter, 2026-10-04: "port vessels/lumen to UnaOS".
//
// WHAT IT IS. The host Lumen (`vessels/lumen`, GTK/quartzite/Tokio) is a window over a model. On UnaOS
// the window is this static ELF64 program and the model is VEIN.BIN, a ring-3 FULFILLER of the chat
// verbs on the BUS v1 wire (BANDY3 registration, tags 130..=133 in the registrable range). This program
// owns NO model, NO provider key and NO network: it draws a transcript and an input line, and it speaks
// four verbs. That is ROADMAP §3b principle 5 — the desktop is a scene of live capabilities viewed over
// the bus — and it is why there is no second chat implementation in the kernel.
//
// THE VERBS — VEINCORE's wire (docs/dev/evidence/rmbp-1004/VEINCORE.md §THE WIRE, codec of record
// `unaos/libs/sys/vein_core/src/wire.rs`):
//   CHAT_SEND   130  request [conv_id u32 LE][text utf8]
//                    replies a SEQUENCE of ChatReply frames on the one correlation id:
//                    [conv_id u32][seq u16][done u8][rsvd u8][text]; every non-final frame has header
//                    status BUS_STATUS_MORE (= 1) and done = 0, the final one status 0 and done = 1; an
//                    error ends the stream with a negative errno and an empty body (-ECANCELED, -EIO, ..)
//   CHAT_REPLY  131  kernel -> VEIN only (the relay companion's injected answer); never sent from here
//   CHAT_CANCEL 132  request [conv_id u32]; reply empty
//   CHAT_STATUS 133  request empty; reply [ready u8][plen u8][mlen u8][rsvd u8][provider][model]
// STREAMING IS PUSHED: VEINCORE's `more` flag keeps the relay's pending entry open, so the chunks simply
// arrive in this program's mailbox in order. No fulfiller at all is the kernel's own -ENOENT reply (an
// answer, never a hang), rendered as "no provider: start VEIN.BIN".
//
// THE UI STAYS ALIVE. SYS_MRECV blocks while the mailbox is empty and the ABI has no non-blocking form,
// so a RECEIVER THREAD (SYS_THREAD_SPAWN, same address space, same mailbox — the mailbox is per slot)
// blocks in it and hands each frame to the main loop through one atomic length word. The main loop only
// polls input, drains that word, redraws when dirty and sleeps. If the thread cannot be spawned the main
// loop falls back to a blocking receive while a request is outstanding (one chunk of latency at a time).
//
// KEYS. Printable ASCII inserts at the caret; Backspace deletes; Left/Right (and the TERMSEL2 caret
// actions) move the caret; Enter sends; Up/Down and the wheel scroll the transcript; Ctrl-K or ⌘K
// (INPUT_EV_ACTION code 41, ClearView) clears the transcript and starts a new conversation id; Esc
// cancels a reply in flight and NEVER closes the window (R24 — the chrome's close box and `kill` do).
// Shift+Enter cannot insert a newline: INPUT_EV_KEY_DOWN carries the ASCII byte only, no modifier bits,
// and Shift+Enter arrives as the same '\n' as Enter. Stated, not faked.
//
// WINDOW. 288x288 (the FB_WIN_MAX ceiling on both arches); INPUT_EV_WIN_RESIZE re-lays everything out
// to the new content size (the surface slot and stride are unchanged). Text is the 8x8 face of the
// `font8x8` crate — the same crate the kernel links — at 1x: 35 columns at full width.
//
// WITNESS. The first words of the RW segment are a fixed block (`.data.lumenwit`, pinned first by the
// link script) that the kernel's `tests lumen` fixture reads out of the slot after driving this program
// with synthetic keys. The program prints NO verdict itself — only `:: LUMEN: start ..` and one
// `:: LUMEN: window=.. ::` progress line per finished exchange.

use core::sync::atomic::{AtomicUsize, Ordering};
use una_abi::{
    input_ev_payload, input_ev_type, BUS_FRAME_MAX, BUS_HDR_LEN, BUS_KIND_REPLY, BUS_KIND_REQUEST, BUS_MAGIC,
    BUS_VERSION, ENOENT, INPUT_EV_ACTION, INPUT_EV_KEY_DOWN, INPUT_EV_WHEEL, INPUT_EV_WIN_RESIZE, KEY_DOWN,
    KEY_ESC, KEY_LEFT, KEY_RIGHT, KEY_UP, SYS_EXIT, SYS_INPUT_POLL, SYS_MRECV, SYS_MSEND, SYS_SLEEP_MS,
    SYS_THREAD_SPAWN, SYS_WIN_CREATE, SYS_WIN_PRESENT, SYS_WRITE,
};

// The chat verbs. VEINCORE declares these in una-abi in parallel; until that fold they are restated here
// with the SAME values. At the fold: delete these and import them from una_abi.
const BUS_VERB_CHAT_SEND: u8 = 130;
const BUS_STATUS_MORE: i64 = 1;
const BUS_VERB_CHAT_CANCEL: u8 = 132;
const BUS_VERB_CHAT_STATUS: u8 = 133;
/// INPUT_EV_ACTION payload for ⌘K (`video::clipboard::action_code(Action::ClearView)`).
const ACTION_CLEAR_VIEW: u64 = 41;
const ACTION_CURSOR_LEFT: u64 = 13;
const ACTION_CURSOR_RIGHT: u64 = 14;
const ACTION_LINE_START: u64 = 15;
const ACTION_LINE_END: u64 = 16;

#[cfg(target_arch = "aarch64")]
mod sysabi {
    #[inline(always)]
    pub unsafe fn sys4(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!("svc #0", inout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x8") n, options(nostack))
        };
        r
    }
}
/// x86_64: the kernel's `sysretq` tail scrubs rdi/rsi/rdx/r8/r9/r10 on EVERY return, so all six are
/// declared as not surviving (U1b B1; see user-pulse for the long form). One 4-argument stub serves every
/// call: unused argument registers are passed as 0, which every verb here ignores.
#[cfg(target_arch = "x86_64")]
mod sysabi {
    #[inline(always)]
    pub unsafe fn sys4(n: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
        let mut r: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n => r,
                inlateout("rdi") a0 => _,
                inlateout("rsi") a1 => _,
                inlateout("rdx") a2 => _,
                inlateout("r10") a3 => _,
                lateout("rcx") _, lateout("r11") _, lateout("r8") _, lateout("r9") _,
                options(nostack),
            )
        };
        r
    }
}
use sysabi::sys4;

#[inline(never)]
fn sys(n: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    unsafe { sys4(n, a0, a1, a2, 0) as i64 }
}
fn sleep_ms(ms: u64) {
    sys(SYS_SLEEP_MS, ms, 0, 0);
}
fn exit(code: u64) -> ! {
    sys(SYS_EXIT, code, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

// ---- witness block (read by the kernel fixture) -------------------------------------------------
const WIT_MAGIC: u32 = 0x4E4D_554C; // "LUMN" little-endian
const W_WINDOW: usize = 1;
const W_SENT: usize = 2;
const W_REPLIES: usize = 3;
const W_ENOENT: usize = 4;
const W_READY: usize = 5;
const W_CLEARED: usize = 6;
const W_KEYS: usize = 7;
#[used]
#[no_mangle]
#[link_section = ".data.lumenwit"]
static mut WIT: [u32; 8] = [WIT_MAGIC, 0, 0, 0, 0, 0, 0, 0];
fn wit(i: usize, v: u32) {
    unsafe { core::ptr::addr_of_mut!(WIT).cast::<u32>().add(i).write_volatile(v) }
}

// ---- bus ----------------------------------------------------------------------------------------
static mut RX: [u8; BUS_FRAME_MAX] = [0; BUS_FRAME_MAX];
/// Length of the frame waiting in RX (0 = empty; the receiver may refill).
static RX_LEN: AtomicUsize = AtomicUsize::new(0);
const TX_BODY: usize = 8 + INP_CAP;
static mut TX: [u8; BUS_HDR_LEN + TX_BODY] = [0; BUS_HDR_LEN + TX_BODY];
static mut CORR: u32 = 0;

fn recv() -> i64 {
    sys(SYS_MRECV, core::ptr::addr_of_mut!(RX) as u64, BUS_FRAME_MAX as u64, 0)
}

/// One REQUEST frame: header, then `head` ++ `tail` as the body. Returns SYS_MSEND's status.
fn send(verb: u8, head: &[u8], tail: &[u8]) -> i64 {
    let t = unsafe { &mut *core::ptr::addr_of_mut!(TX) };
    let tail = &tail[..tail.len().min(TX_BODY - head.len())];
    let blen = head.len() + tail.len();
    let corr = unsafe {
        CORR = CORR.wrapping_add(1);
        CORR
    };
    t[..BUS_HDR_LEN].fill(0); // status 0, principal zero: the kernel stamps
    t[0..4].copy_from_slice(&BUS_MAGIC);
    t[4] = BUS_VERSION;
    t[5] = BUS_KIND_REQUEST;
    t[6] = verb;
    t[8..12].copy_from_slice(&corr.to_le_bytes());
    t[48..52].copy_from_slice(&(blen as u32).to_le_bytes());
    t[BUS_HDR_LEN..BUS_HDR_LEN + head.len()].copy_from_slice(head);
    t[BUS_HDR_LEN + head.len()..BUS_HDR_LEN + blen].copy_from_slice(tail);
    sys(SYS_MSEND, t.as_ptr() as u64, (BUS_HDR_LEN + blen) as u64, 0)
}

#[repr(C, align(16))]
struct Stack([u8; 384]);
static mut TSTACK: Stack = Stack([0; 384]);

/// The receiver thread: block in SYS_MRECV, publish the frame, wait for the main loop to take it.
extern "C" fn rx_thread(_arg: u64) -> ! {
    loop {
        while RX_LEN.load(Ordering::Acquire) != 0 {
            sleep_ms(4);
        }
        let n = recv();
        if n > 0 {
            RX_LEN.store(n as usize, Ordering::Release);
        } else {
            sleep_ms(50);
        }
    }
}

// ---- serial -------------------------------------------------------------------------------------
struct Line {
    b: [u8; 96],
    n: usize,
}
impl Line {
    fn new(s: &[u8]) -> Line {
        let mut l = Line { b: [0; 96], n: 0 };
        l.put(s);
        l
    }
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn dec(&mut self, mut v: u32) {
        let mut d = [0u8; 10];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..]);
    }
    fn kv(&mut self, k: &[u8], v: u32) {
        self.put(k);
        self.dec(v);
    }
    fn flush(&self) {
        sys(SYS_WRITE, 1, self.b.as_ptr() as u64, self.n as u64);
    }
}

// ---- the app ------------------------------------------------------------------------------------
const MAX_W: i32 = 288;
const MAX_H: i32 = 288;
const STRIDE: usize = MAX_W as usize * 4;
const LH: i32 = 10; // line height: the 8 px glyph + 2 px leading
const LOG_CAP: usize = 1024; // scrollback bytes: the window shows ~800 characters; see the budget note
const INP_CAP: usize = 160;
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

const NO_PROVIDER: &[u8] = b"no provider: start VEIN.BIN";

static FONT: [[u8; 8]; 128] = font8x8::legacy::BASIC_LEGACY;

struct App {
    log: [u8; LOG_CAP],
    log_n: usize,
    inp: [u8; INP_CAP],
    inp_n: usize,
    caret: usize,
    status: [u8; 40],
    status_n: usize,
    w: i32,
    h: i32,
    scroll: i32,
    conv: u32,
    pending: u32,
    waiting: bool,
    turn_open: bool,
    ready: bool,
    dirty: bool,
    sent: u32,
    replies: u32,
    enoent: u32,
    cleared: u32,
    keys: u32,
}
static mut APP: App = App {
    log: [0; LOG_CAP],
    log_n: 0,
    inp: [0; INP_CAP],
    inp_n: 0,
    caret: 0,
    status: [0; 40],
    status_n: 0,
    w: 0,
    h: 0,
    scroll: 0,
    conv: 0,
    pending: 0,
    waiting: false,
    turn_open: false,
    ready: false,
    dirty: false,
    sent: 0,
    replies: 0,
    enoent: 0,
    cleared: 0,
    keys: 0,
};

impl App {
    fn set_status(&mut self, a: &[u8], b: &[u8]) {
        self.status_n = 0;
        for &c in a.iter().chain(b.iter()) {
            if self.status_n < self.status.len() {
                self.status[self.status_n] = if (0x20..0x7f).contains(&c) { c } else { b'?' };
                self.status_n += 1;
            }
        }
    }

    /// Make room for `k` more bytes by dropping the OLDEST turn(s) — never a byte of the newest.
    fn room(&mut self, k: usize) {
        while self.log_n + k > LOG_CAP && self.log_n > 0 {
            let mut cut = 1;
            while cut < self.log_n && self.log[cut] > ROLE_NOTE {
                cut += 1;
            }
            if cut >= self.log_n {
                cut = (self.log_n / 2).max(1); // one turn larger than the log: keep its tail
            }
            self.log.copy_within(cut..self.log_n, 0);
            self.log_n -= cut;
            if self.log_n > 0 && self.log[0] > ROLE_NOTE {
                self.log[0] = ROLE_AI; // a clipped turn still starts with a role byte
            }
        }
    }

    /// Append `text` (sanitised to printable ASCII + '\n'; UTF-8 sequences become one '?') to the log.
    fn add(&mut self, text: &[u8]) {
        for &c in text {
            let c = match c {
                b'\n' | 0x20..=0x7e => c,
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
    }
    fn note(&mut self, text: &[u8]) {
        self.turn(ROLE_NOTE, text);
        self.turn_open = false;
    }

    fn progress(&self) {
        let mut l = Line::new(b":: LUMEN: window=1");
        l.kv(b" sent=", self.sent);
        l.kv(b" replies=", self.replies);
        l.kv(b" enoent=", self.enoent);
        l.put(b" ::\n");
        l.flush();
        wit(W_SENT, self.sent);
        wit(W_REPLIES, self.replies);
        wit(W_ENOENT, self.enoent);
        wit(W_READY, self.ready as u32);
    }

    fn submit(&mut self) {
        if self.inp_n == 0 {
            return;
        }
        let n = self.inp_n;
        let rc = send(BUS_VERB_CHAT_SEND, &self.conv.to_le_bytes(), &self.inp[..n]);
        self.turn(ROLE_USER, b"");
        let mut i = 0;
        while i < n {
            let c = self.inp[i];
            self.add(&[c]);
            i += 1;
        }
        self.inp_n = 0;
        self.caret = 0;
        self.sent += 1;
        if rc == 0 {
            self.pending += 1;
            self.waiting = true;
            self.turn_open = false;
        } else if rc == ENOENT {
            self.enoent = 1;
            self.note(NO_PROVIDER);
        } else {
            self.note(b"bus send refused");
        }
        self.progress();
    }

    fn clear(&mut self) {
        if self.waiting {
            send(BUS_VERB_CHAT_CANCEL, &self.conv.to_le_bytes(), &[]);
            self.pending += 1;
            self.waiting = false;
        }
        self.log_n = 0;
        self.scroll = 0;
        self.turn_open = false;
        self.conv = self.conv.wrapping_add(1);
        self.cleared += 1;
        wit(W_CLEARED, self.cleared);
        self.dirty = true;
    }

    /// One reply frame of `n` bytes in RX.
    fn frame(&mut self, n: usize) {
        let r = unsafe { &*core::ptr::addr_of!(RX) };
        if n < BUS_HDR_LEN || r[0..4] != BUS_MAGIC || r[5] != BUS_KIND_REPLY {
            return; // not a reply: this program fulfils nothing
        }
        let u = |o: usize| u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
        let status = u(12) as i32 as i64;
        let blen = (u(48) as usize).min(n - BUS_HDR_LEN);
        let body = &r[BUS_HDR_LEN..BUS_HDR_LEN + blen];
        self.pending = self.pending.saturating_sub(1);
        self.dirty = true;
        match r[6] {
            BUS_VERB_CHAT_STATUS => {
                if status == 0 && blen >= 4 {
                    self.ready = body[0] != 0;
                    let pe = (4 + body[1] as usize).min(blen);
                    let me = (pe + body[2] as usize).min(blen);
                    let (prov, model) = (&body[4..pe], &body[pe..me]);
                    self.set_status(prov, b"");
                    if !model.is_empty() {
                        let mut tmp = [0u8; 40];
                        let k = self.status_n;
                        tmp[..k].copy_from_slice(&self.status[..k]);
                        self.set_status(&tmp[..k], b" / ");
                        tmp[..self.status_n].copy_from_slice(&self.status[..self.status_n]);
                        let k = self.status_n;
                        self.set_status(&tmp[..k], model);
                    }
                } else {
                    self.ready = false;
                    self.set_status(if status == ENOENT { NO_PROVIDER } else { b"vein: status refused" }, b"");
                }
                wit(W_READY, self.ready as u32);
            }
            BUS_VERB_CHAT_SEND => {
                if status == BUS_STATUS_MORE {
                    self.pending += 1; // a non-final chunk: the request is still open
                }
                if !self.waiting {
                    return; // cancelled or cleared: a late chunk is dropped, and nothing more is pulled
                }
                if status < 0 {
                    self.waiting = false;
                    if status == ENOENT {
                        self.enoent = 1;
                        self.note(NO_PROVIDER);
                    } else {
                        self.note(b"vein: reply refused");
                    }
                    self.progress();
                    return;
                }
                // [conv u32][seq u16][done u8][rsvd u8][text]; the stream is over on done = 1 or a status-0
                // frame. A body shorter than the header is one final chunk of plain text (a tolerant reader).
                let (done, text) = if blen >= 8 { (body[6] != 0 || status == 0, &body[8..]) } else { (status != BUS_STATUS_MORE, body) };
                if !self.turn_open {
                    self.turn(ROLE_AI, b"");
                    self.turn_open = true;
                }
                self.add(text); // `text` borrows the static RX, not `self`
                self.replies += 1;
                if done {
                    self.waiting = false;
                    self.turn_open = false;
                    self.progress();
                }
            }
            _ => {} // CHAT_CANCEL's empty reply and anything else
        }
    }

    fn key(&mut self, k: u8) {
        self.keys += 1;
        wit(W_KEYS, self.keys);
        self.dirty = true;
        match k {
            b'\n' => self.submit(),
            0x08 | 0x7f => {
                if self.caret > 0 {
                    self.inp.copy_within(self.caret..self.inp_n, self.caret - 1);
                    self.caret -= 1;
                    self.inp_n -= 1;
                }
            }
            0x0b => self.clear(), // Ctrl-K
            KEY_ESC => {
                if self.waiting {
                    send(BUS_VERB_CHAT_CANCEL, &self.conv.to_le_bytes(), &[]);
                    self.pending += 1;
                    self.waiting = false;
                    self.note(b"cancelled");
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

    fn event(&mut self, ev: u64) {
        let p = input_ev_payload(ev);
        match input_ev_type(ev) {
            INPUT_EV_KEY_DOWN => self.key(p as u8),
            INPUT_EV_ACTION => {
                match p {
                    ACTION_CLEAR_VIEW => self.clear(),
                    ACTION_CURSOR_LEFT => self.caret = self.caret.saturating_sub(1),
                    ACTION_CURSOR_RIGHT => self.caret = (self.caret + 1).min(self.inp_n),
                    ACTION_LINE_START => self.caret = 0,
                    ACTION_LINE_END => self.caret = self.inp_n,
                    _ => {}
                }
                self.dirty = true;
            }
            INPUT_EV_WHEEL => {
                self.scroll = (self.scroll + (p as u8 as i8 as i32)).max(0);
                self.dirty = true;
            }
            INPUT_EV_WIN_RESIZE => {
                self.w = (((p >> 16) & 0xffff) as i32).clamp(64, MAX_W);
                self.h = ((p & 0xffff) as i32).clamp(64, MAX_H);
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// Lay the transcript out at `cols` columns, calling `emit(role, text, first_line_of_turn)` per line.
    /// User and note turns carry a 2-column prefix; a blank line follows every turn.
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
}

// ---- raster -------------------------------------------------------------------------------------
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
    // Footer: the provider / model status line.
    let fy = h - LH - 2;
    s.fill(0, fy - 3, w, 1, RULE);
    let (sc, st): (u32, &[u8]) = if a.waiting {
        (CARET, b"... thinking (Esc cancels)")
    } else if a.status_n == 0 {
        (NOTE_INK, b"asking vein ...")
    } else {
        (if a.ready { OK_INK } else { NOTE_INK }, &a.status[..a.status_n])
    };
    s.text(4, fy, &st[..st.len().min(cols)], sc);
    // Input: up to three wrapped rows above the footer, scrolled so the caret's row is visible.
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
    // Transcript: bottom-anchored, `scroll` lines up from the newest.
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

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let base = _start as *const () as u64; // the window base (the link script pins e_entry at 0)
    let a = unsafe { &mut *core::ptr::addr_of_mut!(APP) };

    let win = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0);
    if win < 0 {
        Line::new(b":: LUMEN: SYS_WIN_CREATE refused ::\n").flush();
        exit(1);
    }
    wit(W_WINDOW, 1);
    (a.w, a.h, a.conv, a.dirty) = (MAX_W, MAX_H, 1, true); // APP is all-zero so it lands in .bss, not the file
    let surf = Surf { p: (base + 0x5000) as *mut u8, w: MAX_W, h: MAX_H };
    let flags = unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4) }; // VUGMIN: bit 1 = all hidden

    let sp = core::ptr::addr_of_mut!(TSTACK) as u64 + core::mem::size_of::<Stack>() as u64;
    let threaded = sys(SYS_THREAD_SPAWN, rx_thread as *const () as u64, sp, 0) >= 0;

    let mut l = Line::new(b":: LUMEN: start");
    l.kv(b" win=", win as u32);
    l.kv(b" threaded=", threaded as u32);
    l.put(b" verbs=130..133 ::\n");
    l.flush();

    a.note(b"Lumen on UnaOS. Enter sends, Ctrl-K or Cmd-K clears, Esc cancels.");
    let mut status_due = 0u32;
    loop {
        // Ask Vein who is answering: at start, then every ~4 s while nobody is.
        if !a.ready && status_due == 0 {
            if send(BUS_VERB_CHAT_STATUS, &[], &[]) == 0 {
                a.pending += 1;
            }
            status_due = 250;
        }
        status_due = status_due.saturating_sub(1);

        let mut k = 0;
        while k < 16 {
            let ev = sys(SYS_INPUT_POLL, 0, 0, 0);
            if ev < 0 {
                break;
            }
            a.event(ev as u64);
            k += 1;
        }

        if threaded {
            let n = RX_LEN.load(Ordering::Acquire);
            if n != 0 {
                a.frame(n);
                RX_LEN.store(0, Ordering::Release);
            }
        } else if a.pending > 0 {
            let n = recv(); // no receiver thread: one blocking receive per outstanding request
            if n > 0 {
                a.frame(n as usize);
            }
        }

        if a.dirty && unsafe { flags.read_volatile() } & 2 == 0 {
            a.dirty = false;
            let s = Surf { w: a.w, h: a.h, ..surf };
            render(a, &s);
            sys(SYS_WIN_PRESENT, win as u64, 0, 0);
        }
        sleep_ms(16);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(3)
}

/// EXECNAME (B322, R82): this program's launch declaration — it opens a window (SYS_WIN_CREATE), so a bare `lumen` detaches like `bg`.
/// Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_WINDOWED);
