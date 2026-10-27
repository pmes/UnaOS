// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF4 (LEDGER SR324): a PDF response as a document in the engine — the Aether face of the Acrobat line.
//!
//! CHARTER: Aether owns the browser and the PDF line (CODEX §2 Web, "Chrome, Acrobat"); this is inside the live
//! handler, over the shared core pdf_core (BP-handlers-4): no parser, rasterizer or font code lives here.
//!
//! A PDF (`application/pdf`, or the `%PDF-` sniff: [`is_pdf`], `net::fetch_page`) is held as [`PdfDocument`] in
//! `AetherEngine::pdf`, beside an EMPTY DOM document whose `<title>` is the PDF's: `load_impl` still runs, so the
//! session history, Back / Forward / reload, the title and every per-document reset are the HTML path's own. While
//! `pdf` is Some the engine's hooks hand five things here:
//!
//! * paint: [`PdfDocument::present`] — the pages stacked vertically (8 px gaps, centred; 100 % zoom = 96 dpi, one
//!   point = 96/72 CSS px), each rendered by PDF2's `Page::render` at 96 dpi x zoom through PDF3's `GlyphSink` (the
//!   unembedded faces from text_host's font database: [`fallback`]) and cached per (page, dpi): a zoom re-renders crisp
//!   instead of resampling, and only the pages near the viewport keep a pixmap;
//! * the scroll extent: [`PdfDocument::max_scroll`] re-binds the one `Event::Scroll` clamp;
//! * selection: [`select`] — a span-based selection parallel to the DOM one (selection.rs holds DOM boundary points),
//!   over PDF3's `Page::text` spans; copy joins them through PDF3's `spans_to_string`;
//! * find: `chrome::find::find_all` per span, the engine's own `FindState`; hits painted as span sub-boxes;
//! * the status line: `AetherEngine::pdf_status` ("Page p / n").
//!
//! A password-protected file opens a visible page saying so (PDF4-PASSWORD owed: no prompt wire exists).

pub mod select;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use pdf_core::font::{Fallback, GlyphSink, TextSpan};
use pdf_core::render::Pixmap;
use pdf_core::{Document, Page};

use self::select::{PdfSelection, Pos, Range};

/// The dpi of 100 % zoom: one PDF point is 96/72 CSS px (Chromium's viewer).
pub const CSS_DPI: f64 = 96.0;
/// CSS px per point at 100 %.
pub const PT: f64 = CSS_DPI / 72.0;
/// CSS px between pages and around the stack.
pub const GAP: f64 = 8.0;
/// The backdrop behind the pages (Chromium's PDF viewer grey).
pub const BACKDROP: (u8, u8, u8) = (82, 86, 89);
/// The active find hit (Chromium's orange); inactive hits use `selection::FIND_MARK`.
pub const ACTIVE_MARK: (u8, u8, u8) = (255, 150, 50);
/// The selection, multiplied over the page so the glyphs stay legible.
pub const SELECT_MARK: (u8, u8, u8) = (180, 213, 255);

/// The tab's favicon for a PDF: a red page with a folded corner (decoded by `images::get`'s data: path).
pub const GLYPH_URI: &str = "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='32' height='32' viewBox='0 0 32 32'><path d='M6 2h14l6 6v22H6z' fill='%23d93025'/><path d='M20 2v6h6z' fill='%23f6aea9'/><path d='M10 16h12M10 20h12M10 24h8' stroke='%23ffffff' stroke-width='2'/></svg>";

/// Whether a response is a PDF: its Content-Type names one, or (unless it says HTML) its body starts with `%PDF-`.
pub fn is_pdf(ctype: &str, bytes: &[u8]) -> bool {
    let t = ctype.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    t == "application/pdf" || t == "application/x-pdf" || (t != "text/html" && bytes.starts_with(b"%PDF-"))
}

/// One installed face's file for a CSS generic family (a collection's first face only: Fallback takes bytes).
fn face_file(generic: &str) -> Option<Vec<u8>> {
    let db = text_host::db::db();
    let i = match db.resolve_family(generic) {
        Some(fam) => db.select(&fam, text_host::db::Style::default()).map(|(i, _, _)| i),
        None => None,
    }
    .or_else(|| db.fallback_order().first().copied())?;
    let f = db.faces.get(i)?;
    if f.index != 0 {
        return None;
    }
    std::fs::read(&f.path).ok()
}

/// The faces unembedded fonts are drawn with (pdf_core does no I/O; the caller hands it file bytes), read once.
pub fn fallback() -> Fallback<'static> {
    static FACES: OnceLock<[Option<Vec<u8>>; 3]> = OnceLock::new();
    let f = FACES.get_or_init(|| [face_file("sans-serif"), face_file("monospace"), face_file("serif")]);
    Fallback { sans: f[0].as_deref(), mono: f[1].as_deref(), serif: f[2].as_deref() }
}

/// The cache key of a dpi.
pub fn dpi_key(dpi: f64) -> u32 {
    (dpi * 1000.0).round() as u32
}

/// An open PDF.
pub struct PdfDocument {
    pub doc: Document,
    pub pages: Vec<Page>,
    /// Each page's size in points as rendered (/CropBox, /Rotate applied).
    pub sizes: Vec<(f64, f64)>,
    /// Each page's top in document CSS px.
    tops: Vec<f64>,
    /// /Info /Title, when it says anything.
    pub title: Option<String>,
    /// Each page's text spans (PDF3's `Page::text`), extracted on first use.
    text: RefCell<Vec<Option<Rc<Vec<TextSpan>>>>>,
    /// Rendered pages by (page, dpi x 1000); a page the renderer refused is cached as a 0 x 0 pixmap (painted white).
    cache: RefCell<HashMap<(usize, u32), Rc<Pixmap>>>,
    pub sel: PdfSelection,
    /// Find hits, and the active one.
    pub hits: Vec<Range>,
    pub cur: Option<usize>,
}

impl PdfDocument {
    /// Parse `bytes` (the empty user password: `pdf_core::Error::Password` when the file needs another).
    pub fn open(bytes: &[u8]) -> Result<PdfDocument, pdf_core::Error> {
        let doc = Document::parse(bytes)?;
        let pages = doc.pages()?;
        if pages.is_empty() {
            return Err(pdf_core::Error::Missing);
        }
        let sizes: Vec<(f64, f64)> = pages
            .iter()
            .map(|p| {
                let [x0, y0, x1, y1] = p.crop_box;
                if p.rotate == 90 || p.rotate == 270 { (y1 - y0, x1 - x0) } else { (x1 - x0, y1 - y0) }
            })
            .collect();
        let mut tops = Vec::with_capacity(sizes.len());
        let mut y = GAP;
        for s in &sizes {
            tops.push(y);
            y += s.1 * PT + GAP;
        }
        let title = doc.info().title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let n = pages.len();
        Ok(PdfDocument {
            doc,
            pages,
            sizes,
            tops,
            title,
            text: RefCell::new(vec![None; n]),
            cache: RefCell::new(HashMap::new()),
            sel: PdfSelection::default(),
            hits: Vec::new(),
            cur: None,
        })
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Page `i`'s rect in document CSS px `(x, y, w, h)` for a viewport `vw` CSS px wide.
    pub fn page_rect(&self, i: usize, vw: f64) -> (f64, f64, f64, f64) {
        let (w, h) = (self.sizes[i].0 * PT, self.sizes[i].1 * PT);
        (((vw - w) / 2.0).max(GAP), self.tops[i], w, h)
    }

    /// The stack's height in CSS px.
    pub fn extent(&self) -> f64 {
        let last = self.pages.len() - 1;
        self.tops[last] + self.sizes[last].1 * PT + GAP
    }

    /// The viewport's scroll range for a viewport `vh` CSS px tall.
    pub fn max_scroll(&self, vh: f64) -> f64 {
        (self.extent() - vh).max(0.0)
    }

    /// The page at document y (the last whose top is at or above it).
    pub fn page_at(&self, y: f64) -> usize {
        self.tops.partition_point(|&t| t <= y).saturating_sub(1)
    }

    /// "Page p / n": p is the page under the viewport's vertical centre.
    pub fn status(&self, scroll_y: f64, vh: f64) -> String {
        format!("Page {} / {}", self.page_at(scroll_y + vh / 2.0) + 1, self.pages.len())
    }

    /// Page `i`'s text spans.
    pub fn spans(&self, i: usize) -> Rc<Vec<TextSpan>> {
        if let Some(t) = &self.text.borrow()[i] {
            return t.clone();
        }
        let t = Rc::new(self.pages[i].text(&self.doc, &fallback()));
        self.text.borrow_mut()[i] = Some(t.clone());
        t
    }

    /// Page `i` rendered at `dpi` (cached).
    fn pixmap(&self, i: usize, dpi: f64) -> Rc<Pixmap> {
        let key = (i, dpi_key(dpi));
        if let Some(p) = self.cache.borrow().get(&key) {
            return p.clone();
        }
        let mut sink = GlyphSink::new(&self.doc, fallback());
        let pm = match self.pages[i].render(&self.doc, dpi, &mut sink) {
            Ok(r) => r.pixmap,
            Err(_) => Pixmap { w: 0, h: 0, data: Vec::new() },
        };
        let rc = Rc::new(pm);
        self.cache.borrow_mut().insert(key, rc.clone());
        rc
    }

    /// The (page, dpi key) pairs the render cache holds, sorted.
    pub fn cached_keys(&self) -> Vec<(usize, u32)> {
        let mut k: Vec<(usize, u32)> = self.cache.borrow().keys().copied().collect();
        k.sort();
        k
    }

    /// The pages a viewport at `scroll_y`, `vh` CSS px tall, shows.
    fn visible(&self, scroll_y: f64, vh: f64) -> std::ops::RangeInclusive<usize> {
        self.page_at(scroll_y)..=self.page_at(scroll_y + vh)
    }

    /// The frame for the shell: `device` pixels at `zoom`, for a CSS viewport `vw` wide scrolled to `scroll_y`
    /// (`AetherEngine::present_rgba`'s contract: width, height, straight RGBA).
    pub fn present(&self, device: (u32, u32), zoom: f64, vw: f64, scroll_y: f64) -> (u32, u32, Vec<u8>) {
        let (dw, dh) = (device.0 as usize, device.1 as usize);
        let mut out = vec![255u8; dw * dh * 4];
        for p in out.chunks_exact_mut(4) {
            p[0] = BACKDROP.0;
            p[1] = BACKDROP.1;
            p[2] = BACKDROP.2;
        }
        let dpi = CSS_DPI * zoom;
        let vh = dh as f64 / zoom;
        let vis = self.visible(scroll_y, vh);
        for i in vis.clone() {
            let (x, y, w, h) = self.page_rect(i, vw);
            let (ox, oy) = ((x * zoom).round() as i64, ((y - scroll_y) * zoom).round() as i64);
            let pm = self.pixmap(i, dpi);
            let (pw, ph) = if pm.w > 0 { (pm.w, pm.h) } else { ((w * zoom).ceil() as usize, (h * zoom).ceil() as usize) };
            // The page's columns that land on the frame.
            let c0 = (-ox).max(0) as usize;
            let c1 = ((dw as i64 - ox).max(0) as usize).min(pw);
            for py in 0..ph {
                let dy = oy + py as i64;
                if dy < 0 || dy >= dh as i64 || c0 >= c1 {
                    continue;
                }
                let o = (dy as usize * dw + (ox + c0 as i64) as usize) * 4;
                let n = (c1 - c0) * 4;
                if pm.w > 0 {
                    let s = (py * pm.w + c0) * 4;
                    out[o..o + n].copy_from_slice(&pm.data[s..s + n]);
                } else {
                    out[o..o + n].fill(255);
                }
            }
            // Find marks and the selection, multiplied over the page (clipped to it and to the frame).
            let s = dpi / 72.0;
            for (r, col) in self.marks(i) {
                let x0 = ((r[0] * s).floor() as i64).max(0) + ox;
                let x1 = ((r[2] * s).ceil() as i64).min(pw as i64) + ox;
                let y0 = ((r[1] * s).floor() as i64).max(0) + oy;
                let y1 = ((r[3] * s).ceil() as i64).min(ph as i64) + oy;
                for yy in y0.max(0)..y1.min(dh as i64) {
                    for xx in x0.max(0)..x1.min(dw as i64) {
                        let o = (yy as usize * dw + xx as usize) * 4;
                        out[o] = (out[o] as u16 * col.0 as u16 / 255) as u8;
                        out[o + 1] = (out[o + 1] as u16 * col.1 as u16 / 255) as u8;
                        out[o + 2] = (out[o + 2] as u16 * col.2 as u16 / 255) as u8;
                    }
                }
            }
        }
        // Only the pages at this dpi on (or next to) the screen keep a pixmap.
        let (lo, hi) = (vis.start().saturating_sub(1), vis.end() + 1);
        let key = dpi_key(dpi);
        self.cache.borrow_mut().retain(|k, _| k.1 == key && k.0 >= lo && k.0 <= hi);
        (device.0, device.1, out)
    }

    /// Page `i`'s highlight boxes (72-dpi page space) and their colours.
    fn marks(&self, i: usize) -> Vec<([f64; 4], (u8, u8, u8))> {
        let mut out = Vec::new();
        let sel = self.sel.ordered().filter(|(a, b)| a.page <= i && i <= b.page && a != b);
        if sel.is_none() && !self.hits.iter().any(|h| h.page == i) {
            return out;
        }
        let spans = self.spans(i);
        // The active hit IS the selection: it paints orange, not orange under blue.
        let active = self.cur.and_then(|k| self.hits.get(k)).map(|h| h.bounds());
        if let Some(b) = sel {
            if active != Some(b) {
                out.extend(select::rects(&self.sel, i, &spans).into_iter().map(|r| (r, SELECT_MARK)));
            }
        }
        for (k, h) in self.hits.iter().enumerate().filter(|(_, h)| h.page == i) {
            if let Some(sp) = spans.get(h.span) {
                let col = if Some(k) == self.cur { ACTIVE_MARK } else { crate::selection::FIND_MARK };
                out.push((select::sub_box(sp, h.lo, h.hi), col));
            }
        }
        out
    }

    /// The text position under viewport CSS (`x`, `y`), for a viewport `vw` wide scrolled to `scroll_y`.
    pub fn hit_at(&self, x: f64, y: f64, vw: f64, scroll_y: f64) -> Option<Pos> {
        let dy = y + scroll_y;
        let page = self.page_at(dy);
        let (px, py, _, _) = self.page_rect(page, vw);
        let spans = self.spans(page);
        select::hit(&spans, (x - px) / PT, (dy - py) / PT).map(|(span, ch)| Pos { page, span, ch })
    }

    /// A find hit's box on its page (72-dpi page space).
    pub fn range_rect(&self, r: &Range) -> Option<[f64; 4]> {
        self.spans(r.page).get(r.span).map(|s| select::sub_box(s, r.lo, r.hi))
    }

    /// Every case-insensitive match of `query` (the find bar's matcher), span by span.
    pub fn find_all(&self, query: &str) -> Vec<Range> {
        let mut out = Vec::new();
        for page in 0..self.pages.len() {
            for (span, s) in self.spans(page).iter().enumerate() {
                for (a, b) in crate::chrome::find::find_all(&s.text, query) {
                    let (lo, hi) = (s.text[..a].chars().count(), s.text[..b].chars().count());
                    out.push(Range { page, span, lo, hi });
                }
            }
        }
        out
    }

    /// The selection's text (copy, `BrowserCopy`).
    pub fn selected_text(&self) -> String {
        select::text(&self.sel, |i| self.spans(i))
    }

    /// The word around `p` (whitespace-bounded inside its span).
    fn word(&self, p: Pos) -> (Pos, Pos) {
        let spans = self.spans(p.page);
        let (a, b) = spans.get(p.span).map(|s| select::word(&s.text, p.ch)).unwrap_or((p.ch, p.ch));
        (Pos { ch: a, ..p }, Pos { ch: b, ..p })
    }

    /// The whole span around `p`.
    fn span_range(&self, p: Pos) -> (Pos, Pos) {
        let n = self.spans(p.page).get(p.span).map(|s| s.text.chars().count()).unwrap_or(0);
        (Pos { ch: 0, ..p }, Pos { ch: n, ..p })
    }

    /// The press range at granularity `g` (1 caret, 2 word, 3 span).
    fn unit(&self, p: Pos, g: u8) -> (Pos, Pos) {
        match g {
            2 => self.word(p),
            3 => self.span_range(p),
            _ => (p, p),
        }
    }
}

/// The last path segment of `url` (the title of a PDF without /Info /Title).
fn file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let seg = path.rsplit('/').next().unwrap_or("");
    if seg.is_empty() { "PDF".to_string() } else { seg.to_string() }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// =================================================================================================
// The engine's PDF hooks (lib.rs calls each from one line while `pdf` is Some).
// =================================================================================================

impl crate::AetherEngine {
    /// Open a fetched PDF: the empty document carrying its title goes through `load_impl` (history, title, resets),
    /// then the PDF takes over painting, scrolling, selection and find. A file that needs a password, or that pdf_core
    /// refuses, is a visible HTML page saying so.
    pub fn load_pdf(&mut self, url: &str, bytes: Vec<u8>, add_history: bool) {
        match PdfDocument::open(&bytes) {
            Ok(pdf) => {
                let title = pdf.title.clone().unwrap_or_else(|| file_name(url));
                let html = format!("<html><head><title>{}</title></head><body></body></html>", escape(&title));
                self.load_impl(url, &html, &[], None, add_history);
                self.pdf = Some(pdf);
                self.pdf_dirty();
            }
            Err(e) => {
                let msg = match e {
                    pdf_core::Error::Password => {
                        "This PDF is password-protected \u{2014} opening it is not supported yet.".to_string()
                    }
                    e => format!("Could not open this PDF: {e:?}"),
                };
                let html = format!(
                    "<html><head><title>{}</title></head><body style=\"font-family: sans-serif; padding: 20px;\"><h1>PDF</h1><p>{}</p></body></html>",
                    escape(&file_name(url)),
                    escape(&msg)
                );
                self.load_impl(url, &html, &[], None, add_history);
            }
        }
    }

    /// The PDF's frame changed: repaint the whole viewport.
    fn pdf_dirty(&mut self) {
        self.needs_repaint = true;
        self.damage_rects.push((0, 0, self.width, self.height));
    }

    /// `press_at` on a PDF: a caret, the word or the span under the pointer; Shift extends.
    pub(crate) fn pdf_press(&mut self, x: f64, y: f64, clicks: u8, shift: bool) {
        let (vw, sy) = (self.width as f64, self.scroll_y);
        let Some(pdf) = self.pdf.as_mut() else { return };
        match pdf.hit_at(x, y, vw, sy) {
            None => pdf.sel = PdfSelection::default(),
            Some(p) => match pdf.sel.anchor.filter(|_| shift) {
                Some(a) =>pdf.sel = PdfSelection { anchor: Some(a), focus: Some(p), drag: Some((1, a, a)) },
                _ => {
                    let g = clicks.clamp(1, 3);
                    let (lo, hi) = pdf.unit(p, g);
                    pdf.sel = PdfSelection { anchor: Some(lo), focus: Some(hi), drag: Some((g, lo, hi)) };
                }
            },
        }
        self.pdf_dirty();
    }

    /// `selection_pointer_moved` on a PDF (button held): the focus follows the pointer at the press's granularity.
    pub(crate) fn pdf_drag(&mut self, x: f64, y: f64) {
        let (vw, sy) = (self.width as f64, self.scroll_y);
        let Some(pdf) = self.pdf.as_mut() else { return };
        let Some((g, lo, hi)) = pdf.sel.drag else { return };
        let Some(p) = pdf.hit_at(x, y, vw, sy) else { return };
        let (a, b) = pdf.unit(p, g);
        let (anchor, focus) = if p < lo { (hi, a) } else { (lo, b) };
        pdf.sel.anchor = Some(anchor);
        pdf.sel.focus = Some(focus);
        self.pdf_dirty();
    }

    /// Ctrl/Cmd+A on a PDF: every span of every page.
    pub(crate) fn pdf_select_all(&mut self) {
        let Some(pdf) = self.pdf.as_mut() else { return };
        let last = (0..pdf.page_count()).rev().find_map(|i| {
            let sp = pdf.spans(i);
            sp.last().map(|s| Pos { page: i, span: sp.len() - 1, ch: s.text.chars().count() })
        });
        pdf.sel = match last {
            Some(end) => PdfSelection { anchor: Some(Pos { page: 0, span: 0, ch: 0 }), focus: Some(end), drag: None },
            None => PdfSelection::default(),
        };
        self.pdf_dirty();
    }

    /// `find` on a PDF: the same `FindState` (a repeat steps), hits over the spans; the active hit becomes the
    /// selection and is scrolled into view.
    pub(crate) fn pdf_find(&mut self, query: &str, forward: bool) -> (u32, u32) {
        let repeat = self.find.is_repeat(query) && self.pdf.as_ref().is_some_and(|p| !p.hits.is_empty());
        let cur = if repeat {
            self.find.step(forward)
        } else {
            let hits = self.pdf.as_ref().map(|p| p.find_all(query)).unwrap_or_default();
            let n = hits.len();
            if let Some(p) = self.pdf.as_mut() {
                p.hits = hits;
            }
            self.find.begin(query, n, forward)
        };
        if let Some(p) = self.pdf.as_mut() {
            p.cur = cur;
        }
        if let Some(i) = cur {
            self.pdf_show_hit(i);
        }
        self.pdf_dirty();
        self.find.result()
    }

    fn pdf_show_hit(&mut self, i: usize) {
        let (vw, vh) = (self.width as f64, self.height as f64);
        let Some(pdf) = self.pdf.as_mut() else { return };
        let Some(h) = pdf.hits.get(i).copied() else { return };
        let (a, b) = h.bounds();
        pdf.sel = PdfSelection { anchor: Some(a), focus: Some(b), drag: None };
        let Some(r) = pdf.range_rect(&h) else { return };
        let (_, py, _, _) = pdf.page_rect(h.page, vw);
        let (top, bottom) = (py + r[1] * PT, py + r[3] * PT);
        if top < self.scroll_y || bottom > self.scroll_y + vh {
            let target = (top - vh / 3.0).max(0.0);
            self.handle_event(crate::api::events::Event::Scroll(0.0, target - self.scroll_y));
        }
    }

    /// The find bar closed on a PDF: the hits go, and the active hit's selection with them.
    pub(crate) fn pdf_find_close(&mut self) {
        if let Some(p) = self.pdf.as_mut() {
            if p.cur.is_some() {
                p.sel = PdfSelection::default();
            }
            p.hits.clear();
            p.cur = None;
        }
        self.pdf_dirty();
    }

    /// The status line's page indicator ("Page p / n") while a PDF is open.
    pub fn pdf_status(&self) -> Option<String> {
        self.pdf.as_ref().map(|p| p.status(self.scroll_y, self.height as f64))
    }
}
