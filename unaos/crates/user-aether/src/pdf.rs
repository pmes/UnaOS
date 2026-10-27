// CHARTER: Aether — shared-core pdf_core; the kernel PDF face (PDF4KERNEL, rmbp-ledger B999)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PDF4KERNEL (rmbp-ledger B999): AETHER.ELF opens a PDF. CODEX §2: Aether owns the Web and the PDF line ("Chrome,
// Acrobat"); seam `shared-core` (BP-handlers-4): pdf_core parses and rasters, the SAME core handlers/aether/src/pdf
// opens with — a page is drawn by pdf_core's `Page::render` through `GlyphSink` exactly as the host's
// `PdfDocument::pixmap` draws it. No parser, filter, rasterizer or font reader lives here.
//
// MECHANISM. `main::load` hands every fetched body to [`take`]: a PDF (its Content-Type — `net::last_ctype` for a
// network page, `chrome::content_type` for a volume path — or the `%PDF-` sniff unless it says HTML: [`is_pdf`]) is
// opened as a [`View`] held by the `Page` beside an EMPTY document titled for it (the host's shape: the session history,
// Back / Forward, the title and every per-page reset stay `load`'s own). `main::lay` re-fits it at every layout
// ([`View::fit`]: the pages stacked 8 px apart, scaled so the widest fills the window less two gaps — the window is at
// most 288 px, so 96 dpi does not fit; PDF4-KERNEL-ZOOM owed) and sets `Page::doc_h`, so the ONE scroll clamp
// (`App::scroll_to`) and every scroll key (arrows, PageUp / PageDown, Space, Home / End, the wheel) move the stack
// unchanged. The frame paints [`View::paint`] in place of the layout tree (pages rendered on demand, the ones off the
// window dropped); the status line shows [`View::status`] ("Page p / n", the host's shape, without the zoom).
//
// CAP. [`MAX_BYTES`] = `net::BODY_MAX` (4 MiB), ONE const: the host's 64 MiB (`handlers/aether/src/pdf/mod.rs`
// MAX_BYTES, a std crate) is unreachable here — heap.rs's largest size class is 8 MiB and pdf_core keeps its own copy
// of the body (PDF4-KERNEL-CAP owed). A volume read stops at `PAGE_MAX` (1 MiB): [`widen`] re-reads a `%PDF-` file that
// filled it, up to the cap.
//
// NAMED DUPLICATION (PDF4-KERNEL-ONE owed): [`is_pdf`] and the stack geometry (GAP, BACKDROP, the tops, `page_at`,
// "Page p / n") restate handlers/aether/src/pdf/mod.rs :70, :182-187, :259-266, whose file is std; they move to one
// std-free host file both rings link by `#[path]`.
//
// WIRE, once per PDF navigation (before its `:: AETHER: nav` line):
// `:: AETHER: pdf -> PASS pages=<n> page=<p>/<n> dpi=<d> w=<px> h=<px> painted=<n> bytes=<n> ::` — page 1 rendered at
// the fit dpi, `w`/`h` its pixmap, `painted` its non-white pixels; or `:: AETHER: pdf -> FAIL reason=<why> bytes=<n> ::`
// (over-cap, syntax, encrypted, unsupported, depth, too-large, loop, missing, inflate, password, no-pages), the window
// then showing a page that names the refusal.

use alloc::string::String;
use alloc::vec::Vec;

use pdf_core::font::{Fallback, GlyphSink};
use pdf_core::render::Pixmap;
use pdf_core::{Document, Page};

use crate::paint::Canvas;
use crate::Line;

/// The largest PDF this program opens: the network body cap, ONE const (see CAP above).
pub const MAX_BYTES: usize = crate::net::BODY_MAX;
/// Device px between pages and around the stack (the host's GAP, CSS px at 100 %).
const GAP: f64 = 8.0;
/// The backdrop behind the pages (the host's BACKDROP, Chromium's PDF viewer grey), 0x00RRGGBB.
const BACKDROP: u32 = (82 << 16) | (86 << 8) | 89;
/// A page with no pixmap yet, or one the renderer refused: white (the host's).
const PAPER: u32 = 0x00FF_FFFF;

/// Whether a response is a PDF: its Content-Type names one, or (unless it says HTML) its body starts with `%PDF-`
/// (handlers/aether/src/pdf/mod.rs `is_pdf`'s rule).
pub fn is_pdf(ctype: &str, bytes: &[u8]) -> bool {
    let t = ctype.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    t == "application/pdf" || t == "application/x-pdf" || (t != "text/html" && bytes.starts_with(b"%PDF-"))
}

/// A volume read of `path` that filled `PAGE_MAX` and sniffs as a PDF, re-read up to one byte past [`MAX_BYTES`] (so an
/// oversized file is refused by name, never opened truncated); any other read is returned as it came.
pub fn widen(path: &str, v: Vec<u8>) -> Vec<u8> {
    if v.len() < crate::PAGE_MAX || !v.starts_with(b"%PDF-") {
        return v;
    }
    match crate::sys::read_file(path.as_bytes(), MAX_BYTES + 1) {
        Ok((w, _)) if w.len() > v.len() => w,
        _ => v,
    }
}

/// pdf_core's refusal, named for the wire.
fn why(e: pdf_core::Error) -> &'static str {
    match e {
        pdf_core::Error::Syntax(..) => "syntax",
        pdf_core::Error::Encrypted => "encrypted",
        pdf_core::Error::Unsupported(_) => "unsupported",
        pdf_core::Error::Depth => "depth",
        pdf_core::Error::TooLarge => "too-large",
        pdf_core::Error::Loop => "loop",
        pdf_core::Error::Missing => "missing",
        pdf_core::Error::Inflate(_) => "inflate",
        pdf_core::Error::Password => "password",
    }
}

/// An open PDF: the document, its pages, the stack at the current fit, and the rendered pages.
pub struct View {
    doc: Document,
    pages: Vec<Page>,
    /// Each page's size in points as rendered (/CropBox, /Rotate applied — the host's `sizes`).
    sizes: Vec<(f64, f64)>,
    /// Each page's top in document px at the current fit.
    tops: Vec<f64>,
    /// Device px per point at the current fit (0 until the first `fit`).
    scale: f64,
    /// The window width the stack was fitted to.
    w: i32,
    /// Page `i`'s pixmap at the current fit, while it is on (or next to) the window.
    cache: Vec<Option<Pixmap>>,
    /// The faces unembedded fonts are drawn with (the DejaVu faces txt.rs read off the volume).
    faces: Fallback<'static>,
}

impl View {
    /// Parse `bytes` (pdf_core) and read its page tree; the refusal's name otherwise.
    pub fn open(bytes: &[u8], faces: Fallback<'static>) -> Result<View, &'static str> {
        if bytes.len() > MAX_BYTES {
            return Err("over-cap");
        }
        let doc = Document::parse(bytes).map_err(why)?;
        let pages = doc.pages().map_err(why)?;
        if pages.is_empty() {
            return Err("no-pages");
        }
        let sizes: Vec<(f64, f64)> = pages
            .iter()
            .map(|p| {
                let [x0, y0, x1, y1] = p.crop_box;
                if p.rotate == 90 || p.rotate == 270 { (y1 - y0, x1 - x0) } else { (x1 - x0, y1 - y0) }
            })
            .collect();
        let cache = (0..pages.len()).map(|_| None).collect();
        Ok(View { doc, pages, sizes, tops: Vec::new(), scale: 0.0, w: 0, cache, faces })
    }

    /// Stack the pages for a window `w` px wide (the widest page fills it less two gaps); the stack's height, which
    /// `main::lay` makes the page's `doc_h`. A new scale drops every rendered page.
    pub fn fit(&mut self, w: i32) -> f32 {
        let widest = self.sizes.iter().fold(1.0f64, |m, s| if s.0 > m { s.0 } else { m });
        let lane = (w as f64 - 2.0 * GAP).max(16.0);
        let scale = lane / widest;
        if scale != self.scale {
            for c in self.cache.iter_mut() {
                *c = None;
            }
        }
        self.scale = scale;
        self.w = w;
        self.tops.clear();
        let mut y = GAP;
        for s in &self.sizes {
            self.tops.push(y);
            y += s.1 * scale + GAP;
        }
        y as f32
    }

    /// The render dpi of the current fit.
    fn dpi(&self) -> f64 {
        self.scale * 72.0
    }

    /// The page whose band holds document y `y` (the host's `page_at`).
    fn page_at(&self, y: f64) -> usize {
        self.tops.partition_point(|&t| t <= y).saturating_sub(1)
    }

    /// "Page p / n": p is the page under the centre of a view `vh` px tall scrolled to `scroll` (the host's `status`).
    pub fn status(&self, scroll: f32, vh: i32) -> String {
        alloc::format!("Page {} / {}", self.page_at(scroll as f64 + vh as f64 / 2.0) + 1, self.pages.len())
    }

    /// Page `i`'s box in document px: centred in the window, at its top.
    fn rect(&self, i: usize) -> (i32, i32, i32, i32) {
        let (pw, ph) = (self.sizes[i].0 * self.scale, self.sizes[i].1 * self.scale);
        (((self.w as f64 - pw) / 2.0) as i32, self.tops[i] as i32, pw as i32, ph as i32)
    }

    /// Page `i` rendered at the current fit (pdf_core `Page::render` through `GlyphSink`, the host's pixmap call); a page
    /// the renderer refused is kept as a 0 x 0 pixmap and painted white (the host's rule).
    fn render(&mut self, i: usize) -> &Pixmap {
        if self.cache[i].is_none() {
            let dpi = self.dpi();
            let pm = {
                let mut sink = GlyphSink::new(&self.doc, self.faces);
                match self.pages[i].render(&self.doc, dpi, &mut sink) {
                    Ok(r) => r.pixmap,
                    Err(_) => Pixmap { w: 0, h: 0, data: Vec::new() },
                }
            };
            self.cache[i] = Some(pm);
        }
        match &self.cache[i] {
            Some(p) => p,
            None => unreachable!(),
        }
    }

    /// The frame: the backdrop, then every page on the window (`scroll` = the document y at its top), rendered on
    /// demand; pages more than one away from the window give their pixmaps back.
    pub fn paint(&mut self, cv: &mut Canvas, scroll: f32) {
        cv.fill(0, 0, cv.w, cv.h, BACKDROP);
        if self.tops.len() != self.pages.len() {
            return; // not fitted yet
        }
        let (a, b) = (self.page_at(scroll as f64), self.page_at(scroll as f64 + cv.h as f64));
        for i in 0..self.cache.len() {
            if i + 1 < a || i > b + 1 {
                self.cache[i] = None;
            }
        }
        for i in a..=b {
            let (x, y, w, h) = self.rect(i);
            let y = y - scroll as i32;
            cv.fill(x, y, w, h, PAPER);
            let pm = self.render(i);
            blit(cv, pm, x, y);
        }
    }

    /// The document's title: /Info /Title when it says anything, else the file's name (the host's shape).
    fn title(&self, got: &str) -> String {
        match self.doc.info().title.map(|t| String::from(t.trim())).filter(|t| !t.is_empty()) {
            Some(t) => t,
            None => String::from(got.rsplit('/').next().unwrap_or(got)),
        }
    }
}

/// A premultiplied RGBA pixmap over white onto the canvas at (`x0`, `y0`), clipped.
fn blit(cv: &mut Canvas, pm: &Pixmap, x0: i32, y0: i32) {
    for py in 0..pm.h {
        let y = y0 + py as i32;
        if y < 0 || y >= cv.h {
            continue;
        }
        for px in 0..pm.w {
            let x = x0 + px as i32;
            if x < 0 || x >= cv.w {
                continue;
            }
            let o = (py * pm.w + px) * 4;
            let inv = 255 - pm.data[o + 3] as u32;
            let c = |v: u8| (v as u32 + inv).min(255);
            cv.px[(y * cv.w + x) as usize] = (c(pm.data[o]) << 16) | (c(pm.data[o + 1]) << 8) | c(pm.data[o + 2]);
        }
    }
}

/// `main::load`'s hook: a fetched PDF becomes (an empty document titled for it, its [`View`]) or (a page naming the
/// refusal, None); any other answer passes through. `w` is the page's width, `vh` its view above the status line.
pub fn take(
    path: &str,
    fetched: Result<(Vec<u8>, String), i64>,
    fonts: &crate::txt::Fonts,
    w: i32,
    vh: i32,
) -> (Result<(Vec<u8>, String), i64>, Option<View>) {
    let (bytes, got) = match fetched {
        Ok(t) => t,
        Err(e) => return (Err(e), None),
    };
    let ctype = if crate::net::is_net(path) { crate::net::last_ctype() } else { crate::chrome::content_type(&got).map(String::from) };
    if !is_pdf(ctype.as_deref().unwrap_or(""), &bytes) {
        return (Ok((bytes, got)), None);
    }
    let n = bytes.len();
    let faces = {
        let t = fonts.0.borrow();
        Fallback { sans: t.sans_face, mono: t.mono_face, serif: None }
    };
    let mut l = Line::new(b":: AETHER: pdf -> ");
    match View::open(&bytes, faces) {
        Ok(mut v) => {
            drop(bytes);
            v.fit(w);
            let pages = v.pages.len() as i64;
            let shown = v.page_at(vh as f64 / 2.0) as i64 + 1;
            let dpi = (v.dpi() + 0.5) as i64;
            let (pw, ph, painted) = {
                let pm = v.render(0);
                let painted = pm.data.chunks_exact(4).filter(|p| !(p[0] == 255 && p[1] == 255 && p[2] == 255)).count();
                (pm.w as i64, pm.h as i64, painted as i64)
            };
            l.put(b"PASS pages=").dec(pages).put(b" page=").dec(shown).put(b"/").dec(pages);
            l.put(b" dpi=").dec(dpi).put(b" w=").dec(pw).put(b" h=").dec(ph).put(b" painted=").dec(painted).put(b" bytes=").dec(n as i64).put(b" ::");
            l.wire();
            let html = alloc::format!("<!doctype html><title>{}</title>", crate::esc(&v.title(&got)));
            (Ok((html.into_bytes(), got)), Some(v))
        }
        Err(r) => {
            l.put(b"FAIL reason=").put(r.as_bytes()).put(b" bytes=").dec(n as i64).put(b" ::");
            l.wire();
            let html = alloc::format!(
                "<!doctype html><title>Aether</title><style>body{{color:#5a1010}}</style><h1>PDF refused</h1>\
<p>The PDF <code>{}</code> could not be opened: open refused {} ({} bytes).</p><p>Backspace goes back.</p>",
                crate::esc(&got),
                r,
                n
            );
            (Ok((html.into_bytes(), got)), None)
        }
    }
}
