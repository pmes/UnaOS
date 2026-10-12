#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL M1 (LEDGER SR66): `APPS/AETHER.ELF` — Aether, UnaOS's browser (CODEX §2: Aether owns the Web), as ONE
// ring-3 program on the image, in LUMEN.ELF's shape (LUMENAPP B323, R82). Peter (2026-10-05): "all the more reason
// to get aether working".
//
// WHAT IT IS. A window over one HTML page read off the volume. The parser, the cascade and the text are the shared
// no_std cores (seam `shared-core`): html_core (WHATWG tokenizer + tree construction into an arena DOM), css_core
// (Syntax 3, Selectors 4, the cascade sort — matched over html_core's arena through `style::El`), font_core::ui (the
// engine the kernel's video::text and Lumen draw with, the DejaVu faces off the volume). The CSS2 block + inline
// formatter is this crate's own (src/layout.rs): the host engine (handlers/aether) lays out with taffy, which is
// std, and porting it is M2's question.
//
// PIPELINE. read (/system/doc/welcome.html, SYS_PATH_READ, else SYS_OPEN) → html_core::parse_document →
// style::compute (UA sheet + <style> + style="") → layout::build (box tree) → layout::layout_root (block + inline
// formatting at the window's width) → paint::paint (back buffer) → the surface → SYS_WIN_PRESENT. A resize lays
// out again at the new width; a scroll repaints.
//
// KEYS. Up/Down scroll a line (20 px), the wheel three, PageUp/PageDown (and Space) a window less one line,
// Home/End (and Cmd/Ctrl+Home/End) the top and the bottom. Esc, or the window's close, exits.
//
// WIRE. `:: AETHER: start src=<path|open|none> bytes=<n> font=<dejavu-sans font_kib=<n>|font8x8 font_why=<w>> ::`
// once, then after the first paint ONE report the kernel's `tests aether` reads at the console seam (aether.rs):
// `:: AETHER: page <path> -> PASS|FAIL painted=<n> lines=<n> blocks=<n> elements=<n> rules=<n> doc_h=<n> ms=<n> ::`
// — `painted` the window's pixels that differ from the canvas colour, `lines` the line boxes of the page; PASS when
// the page was read and at least one line and one pixel were drawn (FAIL carries `reason=` first).

extern crate alloc;

mod heap;
mod layout;
mod paint;
mod style;
mod sys;
mod txt;

use alloc::vec::Vec;
use una_abi::{
    input_ev_payload, input_ev_type, INPUT_EV_ACTION, INPUT_EV_CLOSE_REQ, INPUT_EV_KEY_DOWN, INPUT_EV_WHEEL, INPUT_EV_WIN_RESIZE, KEY_DOWN, KEY_ESC, KEY_UP,
    SYS_INPUT_POLL, SYS_WIN_CREATE, SYS_WIN_PRESENT,
};

use sys::{exit, now_ms, sleep_ms, sys, write};

#[global_allocator]
static HEAP: heap::Heap = heap::Heap::sbrk();

/// The compositor's window ceiling (FB_WIN_MAX_W/H on x86): the surface is always this stride.
pub const MAX_W: i32 = 288;
pub const MAX_H: i32 = 288;
const STRIDE: usize = MAX_W as usize * 4;
/// The page this M1 opens (staged by the builder), and its card twin under a UnaFS root.
const PAGES: [&[u8]; 2] = [b"/system/doc/welcome.html", b"/boot/system/doc/welcome.html"];
const PAGE_MAX: usize = 1024 * 1024;
/// One arrow-key step, px.
const STEP: f32 = 20.0;

const ACTION_LINE_START: u64 = 15;
const ACTION_LINE_END: u64 = 16;
const ACTION_PAGE_UP: u64 = 22;
const ACTION_PAGE_DOWN: u64 = 23;
const ACTION_TOP: u64 = 24;
const ACTION_BOTTOM: u64 = 25;

/// The page the window shows when the file cannot be read (the reason is in it, and on the wire).
const MISSING: &str = "<!doctype html><title>Aether</title><style>body{color:#5a1010}</style>\
<h1>Aether</h1><p>The page <code>/system/doc/welcome.html</code> could not be read from the volume.</p>";

// ---- console lines --------------------------------------------------------------------------------
struct Line {
    b: [u8; 240],
    n: usize,
}
impl Line {
    fn new(s: &[u8]) -> Line {
        let mut l = Line { b: [0; 240], n: 0 };
        l.put(s);
        l
    }
    fn put(&mut self, s: &[u8]) -> &mut Self {
        for &c in s {
            if self.n < self.b.len() - 1 {
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
    fn wire(&mut self) {
        self.b[self.n] = b'\n';
        write(&self.b[..self.n + 1]);
    }
}

// ---- the window -------------------------------------------------------------------------------------

/// The window landmarks: the RO info page at base + 0x4000, surface slot 0 at base + 0x5000 — an elf-model
/// program finds them at the FIXED slot base (RING3WIN), as LUMEN.ELF does.
#[cfg(target_arch = "x86_64")]
fn landmark_base() -> u64 {
    una_abi::USER_BASE_X86
}
#[cfg(not(target_arch = "x86_64"))]
fn landmark_base() -> u64 {
    match una_abi::args() {
        Some(a) if a.window_base() != 0 => a.window_base(),
        _ => {
            Line::new(b":: AETHER: no args page (window base unknown) ::").wire();
            exit(1)
        }
    }
}

struct Page {
    doc: html_core::Document,
    sty: style::Styled,
    boxes: Option<layout::BoxNode>,
    laid: Option<layout::Laid>,
    doc_h: f32,
    lines: usize,
    blocks: usize,
    bg: u32,
}

struct App {
    win: i64,
    surf: *mut u8,
    flags: *const u32,
    w: i32,
    h: i32,
    scroll: f32,
    dirty: bool,
    relayout: bool,
}

impl App {
    fn max_scroll(&self, p: &Page) -> f32 {
        (p.doc_h - self.h as f32).max(0.0)
    }
    fn scroll_to(&mut self, v: f32, p: &Page) {
        let v = v.clamp(0.0, self.max_scroll(p));
        if v != self.scroll {
            self.scroll = v;
            self.dirty = true;
        }
    }
    fn visible(&self) -> bool {
        (unsafe { self.flags.read_volatile() } & 2) == 0 // VUGMIN: bit 1 = every window hidden
    }
    /// Copy the back buffer onto the surface (stride MAX_W) and present.
    fn blit(&self, cv: &paint::Canvas) {
        for y in 0..cv.h.min(MAX_H) {
            let row = unsafe { self.surf.add(y as usize * STRIDE) as *mut u32 };
            for x in 0..cv.w.min(MAX_W) {
                let c = cv.px[(y * cv.w + x) as usize];
                unsafe { row.add(x as usize).write_volatile(0xFF00_0000 | c) };
            }
        }
        sys(SYS_WIN_PRESENT, self.win as u64, 0, 0, 0);
    }
}

/// The canvas colour: the root's background, else the body's, else white (CSS2 §14.2).
fn canvas_bg(doc: &html_core::Document, sty: &style::Styled) -> u32 {
    let Some(root) = doc.document_element() else { return 0x00FF_FFFF };
    if let Some(c) = sty.styles[sty.of[root.0]].bg {
        return c;
    }
    for k in doc.element_children(root) {
        if doc.element(k).is_some_and(|e| e.is_html("body")) {
            if let Some(c) = sty.styles[sty.of[k.0]].bg {
                return c;
            }
        }
    }
    0x00FF_FFFF
}

fn lay(p: &mut Page, txt: &mut txt::Txt, w: i32) {
    let Some(b) = p.boxes.as_ref() else {
        p.laid = None;
        p.doc_h = 0.0;
        return;
    };
    let mut cx = layout::Cx { sty: &p.sty, txt, stats: layout::Stats::default() };
    let (l, h) = layout::layout_root(b, w as f32, &mut cx);
    p.lines = cx.stats.lines;
    p.blocks = cx.stats.blocks;
    p.laid = Some(l);
    p.doc_h = h;
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let _ = &APP_NOTE;
    let base = landmark_base();
    let t0 = now_ms();
    // The window first (a launch during the desktop's own start may meet a busy compositor: retry for 5 s).
    let mut win = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0, 0);
    let mut tries = 0;
    while win < 0 && tries < 50 {
        sleep_ms(100);
        win = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0, 0);
        tries += 1;
    }
    if win < 0 {
        let mut l = Line::new(b":: AETHER: SYS_WIN_CREATE refused code=");
        l.dec(win).put(b" ::");
        l.wire();
        exit(1);
    }
    let mut app = App {
        win,
        surf: (base + 0x5000) as *mut u8,
        flags: unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4) },
        w: MAX_W,
        h: MAX_H,
        scroll: 0.0,
        dirty: true,
        relayout: false,
    };

    // The page off the volume.
    let mut src: Option<sys::Src> = None;
    let mut path: &[u8] = PAGES[0];
    let mut read_err = 0i64;
    let mut bytes: Vec<u8> = Vec::new();
    for p in PAGES {
        match sys::read_file(p, PAGE_MAX) {
            Ok((v, s)) if !v.is_empty() => {
                bytes = v;
                src = Some(s);
                path = p;
                break;
            }
            Ok(_) => read_err = 0,
            Err(e) => {
                if read_err == 0 {
                    read_err = e;
                }
            }
        }
    }
    // The faces.
    let (mut txt, why) = txt::load();

    let mut l = Line::new(b":: AETHER: start src=");
    match src {
        Some(s) => l.put(s.as_str()),
        None => l.put(b"none code=").dec(read_err),
    };
    l.put(b" bytes=").dec(bytes.len() as i64);
    match &why {
        None => {
            l.put(b" font=").put(txt.name()).put(b" font_kib=").dec((txt.bytes / 1024) as i64).put(b" faces=").dec(txt.faces as i64);
        }
        Some(w) => {
            l.put(b" font=font8x8 font_why=").put(w.name()).put(b" font_code=").dec(w.code());
        }
    }
    l.put(b" ::");
    l.wire();

    // Parse, cascade, box tree, layout.
    let t_parse = now_ms();
    let html = match src {
        Some(_) => alloc::string::String::from_utf8_lossy(&bytes).into_owned(),
        None => alloc::string::String::from(MISSING),
    };
    drop(bytes);
    let doc = html_core::parse_document(&html, html_core::ParseOpts::default());
    drop(html);
    let sty = style::compute(&doc, MAX_W as f32, MAX_H as f32);
    let boxes = layout::build(&doc, &sty);
    let bg = canvas_bg(&doc, &sty);
    let mut page = Page { doc, sty, boxes, laid: None, doc_h: 0.0, lines: 0, blocks: 0, bg };
    lay(&mut page, &mut txt, app.w);
    let mut cv = paint::Canvas::new(app.w, app.h);
    let mut reported = false;

    loop {
        let mut k = 0;
        while k < 32 {
            let ev = sys(SYS_INPUT_POLL, 0, 0, 0, 0);
            if ev < 0 {
                break;
            }
            let ev = ev as u64;
            let p = input_ev_payload(ev);
            let page_step = (app.h as f32 - STEP).max(STEP);
            match input_ev_type(ev) {
                INPUT_EV_KEY_DOWN => match p as u8 {
                    KEY_ESC => exit(0),
                    KEY_UP => app.scroll_to(app.scroll - STEP, &page),
                    KEY_DOWN => app.scroll_to(app.scroll + STEP, &page),
                    b' ' => app.scroll_to(app.scroll + page_step, &page),
                    _ => {}
                },
                INPUT_EV_ACTION => match p {
                    ACTION_PAGE_UP => app.scroll_to(app.scroll - page_step, &page),
                    ACTION_PAGE_DOWN => app.scroll_to(app.scroll + page_step, &page),
                    ACTION_TOP | ACTION_LINE_START => app.scroll_to(0.0, &page),
                    ACTION_BOTTOM | ACTION_LINE_END => app.scroll_to(f32::MAX, &page),
                    _ => {}
                },
                INPUT_EV_WHEEL => {
                    let n = p as u8 as i8 as f32; // positive = toward the top (Lumen's sense)
                    app.scroll_to(app.scroll - n * 3.0 * STEP, &page);
                }
                INPUT_EV_WIN_RESIZE => {
                    let w = (((p >> 16) & 0xffff) as i32).clamp(64, MAX_W);
                    let h = ((p & 0xffff) as i32).clamp(64, MAX_H);
                    if w != app.w || h != app.h {
                        app.relayout = w != app.w;
                        app.w = w;
                        app.h = h;
                        app.dirty = true;
                    }
                }
                INPUT_EV_CLOSE_REQ => exit(0), // the WM asks us to quit: nothing unsaved
                _ => {}
            }
            k += 1;
        }
        if app.relayout {
            lay(&mut page, &mut txt, app.w);
            app.relayout = false;
            app.scroll = app.scroll.clamp(0.0, app.max_scroll(&page));
        }
        if app.dirty && (app.visible() || !reported) {
            if cv.w != app.w || cv.h != app.h {
                cv = paint::Canvas::new(app.w, app.h);
            }
            match page.laid.as_ref() {
                Some(l) => paint::paint(&mut cv, l, app.scroll, page.bg, &page.sty, &mut txt),
                None => cv.fill(0, 0, cv.w, cv.h, page.bg),
            }
            app.blit(&cv);
            app.dirty = false;
            if !reported {
                reported = true;
                report(path, src.is_some(), &page, &cv, now_ms().saturating_sub(t_parse), now_ms().saturating_sub(t0));
            }
        }
        sleep_ms(16);
    }
}

/// The one line `tests aether` reads (aether.rs): the verdict, then the numbers after it.
fn report(path: &[u8], read: bool, p: &Page, cv: &paint::Canvas, ms: u64, total_ms: u64) {
    let painted = cv.count_not(p.bg);
    let pass = read && p.lines > 0 && painted > 0;
    let mut l = Line::new(b":: AETHER: page ");
    l.put(path).put(if pass { b" -> PASS" as &[u8] } else { b" -> FAIL" });
    if !pass {
        l.put(if !read { b" reason=no-page" as &[u8] } else if p.lines == 0 { b" reason=no-lines" } else { b" reason=nothing-painted" });
    }
    l.put(b" painted=").dec(painted as i64);
    l.put(b" lines=").dec(p.lines as i64);
    l.put(b" blocks=").dec(p.blocks as i64);
    l.put(b" elements=").dec(p.sty.elements as i64);
    l.put(b" rules=").dec(p.sty.rules as i64);
    l.put(b" doc_h=").dec(p.doc_h as i64);
    l.put(b" ms=").dec(ms as i64);
    l.put(b" start_ms=").dec(total_ms as i64);
    l.put(b" heap_kib=").dec((HEAP.usage().1 / 1024) as i64);
    l.put(b" ::");
    l.wire();
    let _ = &p.doc;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: AETHER: panic ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): the launch declaration — it opens a window (SYS_WIN_CREATE), so a bare `aether` detaches
/// like `bg`. Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_WINDOWED);
